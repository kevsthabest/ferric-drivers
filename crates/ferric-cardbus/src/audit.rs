//! Read-only PCI preflight for the legacy DynaBook resource proposal.
//! BAR reads cannot establish aperture size or ownership. A clear result is
//! not authorization to program hardware; firmware/platform ranges must
//! still be supplied independently.
use crate::{SlotResources, resources::{self,Range}, TOPIC95_PCI_DEV};

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
#[repr(u32)]
pub enum Audit { ClearNeedsFirmware=0, Invalid=1, WrongHost=2, ActiveBridge=3, BusConflict=4, BarConflict=5, UnknownHeader=6 }

pub trait PciRead { fn read(&mut self,bus:u8,dev:u8,func:u8,off:u8)->u32; }

pub fn inspect(io:&mut impl PciRead,slot:u8,r:SlotResources)->Audit {
    if slot>1{return Audit::Invalid;}
    let Ok(ranges)=resources::validate(r,&[],&[]) else{return Audit::Invalid;};
    if io.read(0,0,0,0)!=0x164410b9 || io.read(0,1,0,0)!=0x524710b9 ||
       io.read(0,TOPIC95_PCI_DEV,slot,0)!=0x06171179 {return Audit::WrongHost;}
    // BIOS capture: command 0007, BAR/window zero, secondary/subordinate 3 or 4.
    // Recognize that exact unassigned layout, not an arbitrary live bridge.
    if io.read(0,TOPIC95_PCI_DEV,slot,8)!=0x06070032 ||
       io.read(0,TOPIC95_PCI_DEV,slot,0x10)!=0 ||
       [0x1c,0x20,0x24,0x28,0x2c,0x30,0x34,0x38].iter().any(|&o|io.read(0,TOPIC95_PCI_DEV,slot,o)!=0) {
        return Audit::ActiveBridge;
    }
    // Require the actual captured boot command/header/interrupt profile.
    if io.read(0,TOPIC95_PCI_DEV,slot,4)&0xffff!=7 ||
        (io.read(0,TOPIC95_PCI_DEV,slot,12)>>16)&0xff!=0x82 ||
        io.read(0,TOPIC95_PCI_DEV,slot,0x3c)&0xffff != (u32::from(slot+1)<<8)|0xff {
        return Audit::ActiveBridge;
    }
    let bus=io.read(0,TOPIC95_PCI_DEV,slot,0x18);
    if bus&0x00ff_ffff != if slot==0{0x00030300}else{0x00040400} {return Audit::ActiveBridge;}
    // Scan bus 0 and the known graphics bus, respecting multifunction headers.
    for b in 0..=1 {
        for d in 0..32 {
            let id=io.read(b,d,0,0);
            if id as u16==0xffff || id as u16==0 {continue;}
            let count=if io.read(b,d,0,12)&0x00800000!=0{8}else{1};
            for f in 0..count {
                let id=io.read(b,d,f,0);
                if id as u16==0xffff || id as u16==0 {continue;}
                if b==0 && d==TOPIC95_PCI_DEV && f==slot{continue;}
                let header=(io.read(b,d,f,12)>>16)&0x7f;
                let bars=match header{0=>6,1=>2,2=>1,_=>return Audit::UnknownHeader};
                if header==1 || header==2 {
                    let buses=io.read(b,d,f,0x18);
                    let first=(buses>>8) as u8;let last=(buses>>16) as u8;
                    if first!=0 && first<=r.sec_bus && r.sec_bus<=last{return Audit::BusConflict;}
                    // Prefetchable memory windows can conflict even when no BAR
                    // begins inside the proposed aperture. Refuse unknown 64-bit
                    // window encodings rather than silently ignoring them.
                    if header==1{
                        let w=io.read(b,d,f,0x24);
                        let base=(w&0xfff0)<<16;let limit=(w&0xfff00000)|0xfffff;
                        let is64=w&0x00010001!=0;
                        if is64&&(io.read(b,d,f,0x28)!=0||io.read(b,d,f,0x2c)!=0){return Audit::UnknownHeader;}
                        if base!=0&&limit>=base{
                            let len=u64::from(limit)-u64::from(base)+1;
                            if len>u64::from(u32::MAX)||ranges.iter().any(|a|a.overlaps(Range{base,len:len as u32})){return Audit::BarConflict;}
                        }
                    }
                    // CardBus windows encode 4 KiB pages, PCI bridges 1 MiB.
                    if header==2 {
                        for o in [0x1c,0x24] {
                            let base=io.read(b,d,f,o)&!0xfff;
                            let limit=io.read(b,d,f,o+4)&!0xfff;
                            if base!=0 && limit>=base {
                                let len=u64::from(limit)-u64::from(base)+4096;
                                if len>u64::from(u32::MAX){return Audit::BarConflict;}
                                if ranges.iter().any(|a|a.overlaps(Range{base,len:len as u32})){return Audit::BarConflict;}
                            }
                        }
                    } else {
                        let w=io.read(b,d,f,0x20);
                        let base=(w&0xfff0)<<16;let limit=(w&0xfff00000)|0xfffff;
                        if base!=0 && limit>=base {
                            let len=u64::from(limit)-u64::from(base)+1;
                            if len>u64::from(u32::MAX){return Audit::BarConflict;}
                            if ranges.iter().any(|a|a.overlaps(Range{base,len:len as u32})){return Audit::BarConflict;}
                        }
                    }
                }
                let mut i=0;
                while i<bars {
                    let raw=io.read(b,d,f,0x10+i*4);i+=1;
                    if raw==0 || raw==u32::MAX || raw&1!=0{continue;}
                    let base=raw&!0xf;
                    if raw&6==4 {
                        let hi=io.read(b,d,f,0x10+i*4);i+=1;
                        if hi!=0{continue;}
                    }
                    if base!=0 && ranges.iter().any(|a|a.overlaps(Range{base,len:1})){return Audit::BarConflict;}
                }
            }
        }
    }
    Audit::ClearNeedsFirmware
}

#[cfg(test)]
mod tests{
    use super::*;
    struct Mock{bar:u32,bridge:u32}
    impl PciRead for Mock{
        fn read(&mut self,b:u8,d:u8,f:u8,o:u8)->u32{
            if b!=0||f!=0{return u32::MAX;}
            match(d,o){
                (0,0)=>0x164410b9,(1,0)=>0x524710b9,(17,0)=>0x06171179,(10,0)=>0x12298086,
                (17,4)=>0x04900007,(17,8)=>0x06070032,(17,12)=>0x00820000,(17,0x3c)=>0x000001ff,(17,0x18)=>0x00030300,
                (1,12)=>0x00010000,(1,0x18)=>self.bridge,(10,0x10)=>self.bar,
                (_,0)=>u32::MAX,_=>0,
            }
        }
    }
    #[test]fn conflicts_and_incomplete_evidence(){
        let r=crate::slot_resources(0).unwrap();
        let mut m=Mock{bar:0xf7eff000,bridge:0x00010100};
        assert_eq!(inspect(&mut m,0,r),Audit::ClearNeedsFirmware);
        m.bar=r.mem0_base;assert_eq!(inspect(&mut m,0,r),Audit::BarConflict);
        m.bar=0;m.bridge=0x00030200;assert_eq!(inspect(&mut m,0,r),Audit::BusConflict);
    }
}
