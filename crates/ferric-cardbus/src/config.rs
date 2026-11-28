//! Reversible bridge configuration for a claimed, initially inactive socket.
//! No allocation, power control or child DMA is performed by this layer.
use crate::{SlotResources, TOPIC95_VID, TOPIC95_DID, topic95_bridge_control};

pub trait ConfigIo {
    fn read(&mut self, offset: u8) -> u32;
    fn write(&mut self, offset: u8, value: u32);
}
const OFFSETS: [u8; 15] = [4, 0x10, 0x18, 0x1c, 0x20, 0x24, 0x28, 0x2c, 0x30, 0x3c, 0x34, 0x38, 0x44, 0x90, 0xa0];
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error { Absent, Active, Invalid, Readback }
pub struct Snapshot { values: [u32; 15] }
impl Snapshot {
    pub fn capture(io: &mut impl ConfigIo) -> Result<Self, Error> {
        Self::capture_inner(io,None)
    }
    /// Only for a separately authorized DynaBook boot profile. The BIOS
    /// leaves decode/master bits set despite unassigned BARs and windows.
    pub fn capture_dynabook(io: &mut impl ConfigIo, slot:u8) -> Result<Self, Error> {
        if slot>1{return Err(Error::Invalid);}
        Self::capture_inner(io,Some(slot))
    }
    fn capture_inner(io: &mut impl ConfigIo, slot:Option<u8>) -> Result<Self, Error> {
        if io.read(0) != (u32::from(TOPIC95_DID) << 16 | u32::from(TOPIC95_VID)) {
            return Err(Error::Absent);
        }
        let mut values = [0; 15];
        for (i, off) in OFFSETS.iter().enumerate() {
            values[i] = io.read(*off);
            if values[i] == u32::MAX { return Err(Error::Absent); }
        }
        // Refuse existing live decoding/mastering; do not seize a configured bridge.
        if values[0] & 7 != 0 {
            let expected=match slot{Some(0)=>0x00030300,Some(1)=>0x00040400,_=>return Err(Error::Active)};
            if values[0]&0xffff!=7 || values[1]!=0 || values[2]&0x00ff_ffff!=expected ||
                [3,4,5,6,7,8,10,11].iter().any(|&i|values[i]!=0) ||
                io.read(8)!=0x06070032 || values[9]&0xff!=0xff {return Err(Error::Active);}
        }
        Ok(Self { values })
    }
    /// Stage an inactive bridge with reset asserted and decoding disabled.
    /// Caller must validate resources against firmware and acquire the lease.
    pub fn stage(&self, io: &mut impl ConfigIo, r: SlotResources, rev: u8) -> Result<(), Error> {
        crate::resources::validate(r, &[], &[]).map_err(|_| Error::Invalid)?;
        let cmd=io.read(4);
        if cmd==u32::MAX || cmd&0xffff!=self.values[0]&0xffff {return Err(Error::Active);}
        io.write(4,cmd&0xfff8);
        if io.read(4)&7!=0{return Err(Error::Readback);}
        let ctrl = topic95_bridge_control((self.values[9] >> 16) as u16, rev, true);
        io.write(0x3c, (u32::from(ctrl) << 16) | (self.values[9] & 0xff00) | u32::from(r.irq_line));
        io.write(0x10, r.bar0_phys);
        io.write(0x18, (self.values[2] & 0xff00_0000) | u32::from(r.sec_bus) * 0x10100);
        io.write(0x1c, r.mem0_base);
        io.write(0x20, r.mem0_limit);
        io.write(0x24, 0xffff_f000);
        io.write(0x28, 0);
        io.write(0x2c, 0xfffc);
        io.write(0x30, 0);
        io.write(0x34, 0xfffc);
        io.write(0x38, 0);
        io.write(0x44, 0);
        io.write(0x90, self.values[13] | 1);
        let pin = if self.values[14] & 0x2000 != 0 { 0x2000 } else { 0x1000 };
        io.write(0xa0, (self.values[14] & !0x3000) | pin | 0x800001c0);
        if io.read(0x10) & !0xfff != r.bar0_phys ||
            io.read(0x18) & 0x00ff_ffff != u32::from(r.sec_bus) * 0x10100 ||
            io.read(0x1c) & !0xfff != r.mem0_base ||
            io.read(0x20) & !0xfff != r.mem0_limit {
            return Err(Error::Readback);
        }
        Ok(())
    }
    /// Only after child DMA is stopped and socket power is off. Keep decoding
    /// disabled throughout restoration; never echo PCI status W1C bits.
    pub fn restore(&self, io: &mut impl ConfigIo) -> Result<(), Error> {
        if io.read(0) != (u32::from(TOPIC95_DID) << 16 | u32::from(TOPIC95_VID)) {
            return Err(Error::Absent);
        }
        io.write(4, self.values[0] & 0xfff8);
        if io.read(4) & 7 != 0 { return Err(Error::Readback); }
        for i in 1..OFFSETS.len() { io.write(OFFSETS[i], self.values[i]); }
        for i in 1..OFFSETS.len() {
            if io.read(OFFSETS[i]) != self.values[i] { return Err(Error::Readback); }
        }
        io.write(4, self.values[0] & 0xffff);
        if io.read(4) & 0xffff != self.values[0] & 0xffff { return Err(Error::Readback); }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Mock { regs: [u32;64], ignore: Option<u8>, bad_status_write: bool }
    impl ConfigIo for Mock {
        fn read(&mut self, o:u8)->u32 { self.regs[o as usize/4] }
        fn write(&mut self,o:u8,v:u32) {
            if o==4 && v>>16!=0 { self.bad_status_write=true; }
            if self.ignore!=Some(o) { self.regs[o as usize/4]=v; }
        }
    }
    fn fixture()->Mock {
        let mut m=Mock{regs:[0;64],ignore:None,bad_status_write:false};
        m.regs[0]=0x06171179; m.regs[1]=0xabcd0000; m
    }
    #[test]
    fn stage_and_restore() {
        let mut m=fixture(); let s=Snapshot::capture(&mut m).unwrap();
        let r=crate::slot_resources(0).unwrap();
        s.stage(&mut m,r,0x32).unwrap();
        assert_eq!(m.read(4)&7,0);
        assert_eq!(m.read(0x10),r.bar0_phys);
        s.restore(&mut m).unwrap();
        assert_eq!(m.read(0x10),0);
        assert!(!m.bad_status_write);
    }
    #[test]
    fn exact_dynabook_unassigned_boot_profile() {
        let mut m=fixture();m.regs[1]=0x04900007;m.regs[2]=0x06070032;
        m.regs[6]=0x00030300;m.regs[15]=0x000001ff;
        assert!(matches!(Snapshot::capture(&mut m),Err(Error::Active)));
        let s=Snapshot::capture_dynabook(&mut m,0).unwrap();
        s.stage(&mut m,crate::slot_resources(0).unwrap(),0x32).unwrap();
        assert_eq!(m.read(4)&7,0);
        s.restore(&mut m).unwrap();
        assert_eq!(m.read(4)&7,7);
        m.regs[4]=0xf7e00000;
        assert!(matches!(Snapshot::capture_dynabook(&mut m,0),Err(Error::Active)));
    }
    #[test]
    fn refuses_active_and_detects_failed_write() {
        let mut m=fixture(); m.regs[1]|=4;
        assert!(matches!(Snapshot::capture(&mut m),Err(Error::Active)));
        m.regs[1]&=!4; let s=Snapshot::capture(&mut m).unwrap();
        m.ignore=Some(0x10);
        assert_eq!(s.stage(&mut m,crate::slot_resources(0).unwrap(),0x32),Err(Error::Readback));
        m.ignore=None; s.restore(&mut m).unwrap();
    }
}
