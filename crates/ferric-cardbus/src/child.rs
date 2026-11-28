//! AR2413 child enumeration and DMA enable contract. Caller owns bridge
//! and child PCI function, and has audited the containing aperture.
use crate::{config::ConfigIo, SlotResources};
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub enum Error{Absent,Unsupported,Active,Range,Readback}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub struct Child {pub vendor:u16,pub device:u16,pub revision:u8,pub subsystem:u32,pub bar:u32,pub irq:u8}
pub const AR2413_MMIO_BYTES:u32=65536;
pub fn enumerate(io:&mut impl ConfigIo,r:SlotResources)->Result<Child,Error>{
    let id=io.read(0);
    if id==u32::MAX||id as u16==0{return Err(Error::Absent);}
    if id!=0x001a168c || io.read(8)>>8!=0x020000 || (io.read(12)>>16)&0xff!=0{return Err(Error::Unsupported);}
    let cmd=io.read(4);
    if cmd==u32::MAX||cmd&7!=0{return Err(Error::Active);}
    let bar=io.read(0x10);
    if bar==u32::MAX||bar&7!=0||bar&!0xf!=0{return Err(Error::Active);}
    let end=u64::from(r.mem0_base)+u64::from(AR2413_MMIO_BYTES);
    if r.mem0_base==0||r.mem0_base&(AR2413_MMIO_BYTES-1)!=0||end>u64::from(r.mem0_limit)+4096{return Err(Error::Range);}
    Ok(Child{vendor:id as u16,device:(id>>16) as u16,revision:io.read(8) as u8,subsystem:io.read(0x2c),bar:r.mem0_base,irq:r.irq_line})
}
pub struct ChildConfig{command:u16,bar:u32,irq:u32}
impl ChildConfig{
    /// Configure memory decoding only. No DMA permission is granted here.
    pub fn prepare(io:&mut impl ConfigIo,c:Child)->Result<Self,Error>{
        if io.read(0)!=0x001a168c{return Err(Error::Absent);}
        let cmd=io.read(4);
        if cmd==u32::MAX||cmd&7!=0{return Err(Error::Active);}
        let bar=io.read(0x10);let irq=io.read(0x3c);
        if bar!=0||irq==u32::MAX{return Err(Error::Active);}
        let old=Self{command:cmd as u16,bar,irq};
        io.write(0x10,c.bar);
        io.write(0x3c,(irq&0xffffff00)|u32::from(c.irq));
        io.write(4,u32::from(old.command)|2);
        // Preserve snapshot even on readback failure: caller must retain
        // ownership and use restore before releasing its child claim.
        Ok(old)
    }
    pub fn verify(io:&mut impl ConfigIo,c:Child)->Result<(),Error>{
        if io.read(0)!=0x001a168c || io.read(0x10)&!0xf!=c.bar || io.read(4)&7!=2 || io.read(0x3c)&0xff!=u32::from(c.irq){return Err(Error::Readback);}
        Ok(())
    }
    /// Caller must have masked interrupts, stopped engines and verified idle.
    pub fn restore(&self,io:&mut impl ConfigIo)->Result<(),Error>{
        if io.read(0)!=0x001a168c{return Err(Error::Absent);}
        io.write(4,u32::from(self.command)&!7);
        if io.read(4)&7!=0{return Err(Error::Readback);}
        io.write(0x10,self.bar);
        io.write(0x3c,self.irq);
        if io.read(0x10)!=self.bar||io.read(0x3c)!=self.irq{return Err(Error::Readback);}
        io.write(4,u32::from(self.command));
        if io.read(4)&0xffff!=u32::from(self.command){return Err(Error::Readback);}
        Ok(())
    }
}
#[cfg(test)]
mod tests{
    use super::*;
    struct Mock{regs:[u32;64]}
    impl ConfigIo for Mock{
        fn read(&mut self,o:u8)->u32{self.regs[o as usize/4]}
        fn write(&mut self,o:u8,v:u32){if o==4{assert_eq!(v>>16,0);}self.regs[o as usize/4]=v;}
    }
    fn fixture()->Mock{let mut m=Mock{regs:[0;64]};m.regs[0]=0x001a168c;m.regs[2]=0x02000001;m.regs[11]=0x3b081186;m}
    #[test]fn enumeration_and_memory_only(){
        let mut m=fixture();let c=enumerate(&mut m,crate::slot_resources(0).unwrap()).unwrap();
        assert_eq!(c.bar,0xf7d00000);assert_eq!(c.subsystem,0x3b081186);
        let s=ChildConfig::prepare(&mut m,c).unwrap();ChildConfig::verify(&mut m,c).unwrap();
        assert_eq!(m.read(4)&7,2);s.restore(&mut m).unwrap();assert_eq!(m.read(4)&7,0);
    }
    #[test]fn reject_wrong_card_and_active_dma(){
        let mut m=fixture();let r=crate::slot_resources(0).unwrap();m.regs[1]=4;
        assert_eq!(enumerate(&mut m,r),Err(Error::Active));m.regs[1]=0;m.regs[0]=0x12345678;
        assert_eq!(enumerate(&mut m,r),Err(Error::Unsupported));
    }
}

use crate::lifecycle::{Broker,Claim};

/// The bridge owns the child BDF while configured. The consumer borrows it
/// through the session, rather than making a conflicting FRES claim.
pub struct ChildLease {
    claim:Claim,
    held:bool,
    snapshot:Option<ChildConfig>,
    child:Option<Child>,
}
impl ChildLease {
    pub fn new(bus:u8)->Self{
        Self{claim:Claim{kind:1,a:u32::from(bus)<<8,b:0},held:false,snapshot:None,child:None}
    }
    pub fn setup(&mut self,io:&mut (impl ConfigIo+Broker),r:SlotResources)->Result<Child,Error>{
        if self.held || self.claim.a!=u32::from(r.sec_bus)<<8{return Err(Error::Active);}
        let c=enumerate(io,r)?;
        if !io.claim(self.claim){return Err(Error::Active);}
        self.held=true;
        // Keep ownership even on an error, until cleanup is verified.
        let result=(||{
            let s=ChildConfig::prepare(io,c)?;
            self.snapshot=Some(s);
            ChildConfig::verify(io,c)?;
            self.child=Some(c);
            Ok(c)
        })();
        if result.is_err() && self.cleanup(io).is_err(){return Err(Error::Readback);}
        result
    }
    pub fn cleanup(&mut self,io:&mut (impl ConfigIo+Broker))->Result<(),Error>{
        if let Some(s)=&self.snapshot{
            // Do not tear down an active bus master. If removed, the
            // platform must prove isolation before releasing its claim.
            let cmd=io.read(4);
            if cmd==u32::MAX || cmd&4!=0{return Err(Error::Active);}
            s.restore(io)?;
            self.snapshot=None;
            self.child=None;
        }
        if self.held{
            if !io.release(self.claim){return Err(Error::Readback);}
            self.held=false;
        }
        Ok(())
    }
    pub fn child(&self)->Option<Child>{self.child}
    pub fn is_held(&self)->bool{self.held}
}
#[cfg(test)]
mod lease_tests {
    use super::*;
    struct Mock { regs:[u32;64],held:bool,fail_release:bool,ignore_bar:bool }
    impl ConfigIo for Mock{
        fn read(&mut self,o:u8)->u32{self.regs[o as usize/4]}
        fn write(&mut self,o:u8,v:u32){if !(o==0x10 && self.ignore_bar){self.regs[o as usize/4]=v;}}
    }
    impl Broker for Mock{
        fn claim(&mut self,_:Claim)->bool{if self.held{return false;}self.held=true;true}
        fn release(&mut self,_:Claim)->bool{if self.fail_release{return false;}self.held=false;true}
    }
    fn fixture()->Mock{
        let mut m=Mock{regs:[0;64],held:false,fail_release:false,ignore_bar:false};
        m.regs[0]=0x001a168c;m.regs[2]=0x02000001;m
    }
    #[test]fn active_dma_and_failed_release_pin_claim(){
        let mut m=fixture();let r=crate::slot_resources(0).unwrap();let mut l=ChildLease::new(r.sec_bus);
        assert!(l.setup(&mut m,r).is_ok());
        m.regs[1]|=4;assert_eq!(l.cleanup(&mut m),Err(Error::Active));assert!(l.is_held());
        m.regs[1]&=!4;m.fail_release=true;assert!(l.cleanup(&mut m).is_err());assert!(l.is_held());
        m.fail_release=false;l.cleanup(&mut m).unwrap();assert!(!m.held);
    }
    #[test]fn readback_failure_reverses_setup(){
        let mut m=fixture();m.ignore_bar=true;let r=crate::slot_resources(0).unwrap();let mut l=ChildLease::new(r.sec_bus);
        assert_eq!(l.setup(&mut m,r),Err(Error::Readback));assert!(!l.is_held());assert_eq!(m.regs[1]&7,0);
    }
}
