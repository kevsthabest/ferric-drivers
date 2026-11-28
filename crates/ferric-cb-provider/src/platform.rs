//! Jemm platform adapter for the CardBus transaction coordinator.
//! Activation is restricted to explicitly supplied, audited resource layouts.
use ferric_cardbus::{config::ConfigIo, lifecycle::{Broker,Claim}, power::SocketIo, provider::Platform, SlotResources, TOPIC95_PCI_DEV, CB_BRIDGE_CRST};
use ferric_hw::pci::{read32,write32};
use ferric_cardbus::child::{ChildLease,Child};
struct ChildIo(u8);
impl ConfigIo for ChildIo {
    fn read(&mut self,o:u8)->u32{unsafe{read32(self.0,0,0,o)}}
    fn write(&mut self,o:u8,v:u32){unsafe{write32(self.0,0,0,o,v)}}
}
impl Broker for ChildIo {
    fn claim(&mut self,c:Claim)->bool{unsafe{cb_claim(c.kind,c.a,c.b)==0}}
    fn release(&mut self,c:Claim)->bool{unsafe{cb_unclaim(c.kind,c.a,c.b)==0}}
}

#[cfg(not(test))]
unsafe extern "C" {
    fn cb_claim(kind:u32,a:u32,b:u32)->u32;
    fn cb_unclaim(kind:u32,a:u32,b:u32)->u32;
    fn cb_map(phys:u32,pages:u32)->u32;
    fn cb_unmap(linear:u32)->u32;
}
#[cfg(test)]
unsafe fn cb_claim(_:u32,_:u32,_:u32)->u32{1}
#[cfg(test)]
unsafe fn cb_unclaim(_:u32,_:u32,_:u32)->u32{1}
#[cfg(test)]
unsafe fn cb_map(_:u32,_:u32)->u32{0}
#[cfg(test)]
unsafe fn cb_unmap(_:u32)->u32{0}

pub struct Hardware { slot:u8, resources:SlotResources, linear:u32, child:ChildLease }
impl Hardware {
    pub fn new(slot:u8,resources:SlotResources)->Self{Self{slot,resources,linear:0,child:ChildLease::new(resources.sec_bus)}}
    pub fn mapped(&self)->u32{self.linear}
    pub fn set_dma_master(&mut self,enable:bool)->bool{
        use ferric_cardbus::child::set_master;
        let mut child=ChildIo(self.resources.sec_bus);
        if enable {
            if self.child.child().is_none()||child.read(0)!=0x001a168c{return false;}
            if set_master(self,true).is_err(){return false;}
            if set_master(&mut child,true).is_err(){
                let _=set_master(&mut child,false);
                let _=set_master(self,false);
                return false;
            }
            true
        }else{
            let a=set_master(&mut child,false);
            let b=set_master(self,false);
            a.is_ok()&&b.is_ok()
        }
    }
    pub fn child(&self)->Option<Child>{self.child.child()}
    pub fn socket_state(&self)->u32{self.r32(8)}
    pub fn socket_control(&self)->u32{self.r32(0x10)}
    fn r32(&self,off:usize)->u32 {
        if self.linear==0{return u32::MAX;}
        unsafe{core::ptr::read_volatile((self.linear as usize+off) as *const u32)}
    }
    fn w32(&self,off:usize,value:u32){
        if self.linear!=0{unsafe{core::ptr::write_volatile((self.linear as usize+off) as *mut u32,value)};let _=self.r32(off);}
    }
    fn w8(&self,off:usize,value:u8){
        if self.linear!=0{unsafe{core::ptr::write_volatile((self.linear as usize+off) as *mut u8,value)};}
    }
}
impl ConfigIo for Hardware {
    fn read(&mut self,o:u8)->u32{unsafe{read32(0,TOPIC95_PCI_DEV,self.slot,o)}}
    fn write(&mut self,o:u8,v:u32){unsafe{write32(0,TOPIC95_PCI_DEV,self.slot,o,v)}}
}
impl Broker for Hardware {
    fn claim(&mut self,c:Claim)->bool{unsafe{cb_claim(c.kind,c.a,c.b)==0}}
    fn release(&mut self,c:Claim)->bool{unsafe{cb_unclaim(c.kind,c.a,c.b)==0}}
}
impl SocketIo for Hardware {
    fn state(&mut self)->u32{self.r32(8)}
    fn control(&mut self)->u32{self.r32(0x10)}
    fn set_control(&mut self,v:u32){self.w32(0x10,v)}
    fn set_reset(&mut self,asserted:bool)->bool{
        let v=self.read(0x3c);if v==u32::MAX{return false;}
        let mask=u32::from(CB_BRIDGE_CRST)<<16;
        self.write(0x3c,if asserted{v|mask}else{v&!mask});
        let got=self.read(0x3c);got!=u32::MAX && (got&mask!=0)==asserted
    }
    fn mask_events(&mut self){self.w32(4,0)}
    fn clear_events(&mut self){self.w32(0,u32::MAX)}
    fn disable_exca_power(&mut self){for o in [0x802,0x816,0x81e]{self.w8(o,0)}}
    fn force(&mut self,v:u32){self.w32(0xc,v)}
    fn delay_ms(&mut self,ms:u32)->bool{delay(ms)}
}
impl Platform for Hardware {
    fn open_socket(&mut self,physical:u32)->bool{
        if self.linear!=0 || physical!=self.resources.bar0_phys{return false;}
        let cmd=self.read(4);if cmd==u32::MAX{return false;}
        self.write(4,(cmd&0xffff)|2);
        if self.read(4)&7!=2{return false;}
        self.linear=unsafe{cb_map(physical,1)};
        self.linear!=0
    }
    fn close_socket(&mut self)->bool{
        let cmd=self.read(4);if cmd==u32::MAX{return false;}
        self.write(4,cmd&0xfff8);
        if self.read(4)&7!=0{return false;}
        if self.linear!=0 {
            if unsafe{cb_unmap(self.linear)}==0{return false;}
            self.linear=0;
        }
        true
    }
    fn prepare_child(&mut self)->bool{
        self.child.setup(&mut ChildIo(self.resources.sec_bus),self.resources).is_ok()
    }
    fn release_child(&mut self)->bool{
        self.child.cleanup(&mut ChildIo(self.resources.sec_bus)).is_ok()
    }
    fn child_idle(&mut self)->bool{
        // No consumer may be attached here. Refuse if the child still masters.
        let cmd=unsafe{read32(self.resources.sec_bus,0,0,4)};
        cmd==u32::MAX || cmd&4==0
    }
}
#[cfg(target_arch="x86")]
fn delay(ms:u32)->bool{
    // PIT channel 0 mode 3 counts down by two. Reject implausibly large
    // deltas at the mode-3 reload boundary; undercount is safe.
    fn sample()->u16{let lo:u8;let hi:u8;unsafe{core::arch::asm!("out 0x43, al","in al, 0x40",inout("al") 0u8=>lo,options(nomem,nostack));core::arch::asm!("in al, 0x40",out("al") hi,options(nomem,nostack));}u16::from(lo)|(u16::from(hi)<<8)}
    let target=ms.saturating_mul(2387);let mut elapsed=0u32;let mut prev=sample();
    for _ in 0..ms.saturating_mul(200_000).max(200_000){
        let now=sample();let delta=prev.wrapping_sub(now);
        if delta<0x8000{elapsed=elapsed.saturating_add(u32::from(delta));}
        if elapsed>=target{return true;}prev=now;
    }
    false
}
#[cfg(not(target_arch="x86"))]
fn delay(_:u32)->bool{false}
