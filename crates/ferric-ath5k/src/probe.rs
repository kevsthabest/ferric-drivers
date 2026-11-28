//! Shared MAC/EEPROM probe ABI and hardware sequence. Caller owns an
//! isolated, mapped AR2413; no DMA descriptors or radio transmission.
use crate::{decode_aes_support,decode_eeprom_mac,registers::{RegisterIo,wake_chip,nic_reset,eeprom_read}};
const AR5K_RESET_CTL_PCU: u32 = 0x0000_0001;
const AR5K_RESET_CTL_BASEBAND: u32 = 0x0000_0002;
const AR5K_RESET_CTL_PCI: u32 = 0x0000_0010;
const AR5K_PCICFG: usize = 0x4010;
const AR5K_PCICFG_RETRY_FIX: u32 = 0x0000_1000;
const AR5K_SREV: usize = 0x4020;
const AR5K_PHY_CHIP_ID: usize = 0x9818;
const AR5K_EEPROM_MAGIC: u32 = 0x003D;
const AR5K_EEPROM_REG_DOMAIN: u32 = 0x00BF;
const AR5K_EEPROM_VERSION: u32 = 0x00C1;
const AR5K_EEPROM_HDR: u32 = 0x00C2;
const AR5K_EEPROM_MISC5: u32 = 0x00C9;
const AR5K_SREV_AR5213A: u32 = 0x59;
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct WifiProbeInfo {
    pub present: u8,
    pub stage: u8,
    pub slot: u8,
    pub aes_hw: u8,
    pub pci_vid: u16,
    pub pci_did: u16,
    pub bar0_phys: u32,
    pub bar0_lin: u32,
    pub srev: u32,
    pub phy_id: u32,
    pub ee_magic: u16,
    pub ee_version: u16,
    pub ee_hdr: u16,
    pub ee_misc5: u16,
    pub ee_regdom: u16,
    pub mac_ok: u8,
    pub reserved: u8,
    pub mac: [u8; 6],
    pub pad: [u8; 2],
}

impl WifiProbeInfo {
    pub const ZERO: Self = Self {
        present: 0,
        stage: 0,
        slot: 0,
        aes_hw: 0,
        pci_vid: 0,
        pci_did: 0,
        bar0_phys: 0,
        bar0_lin: 0,
        srev: 0,
        phy_id: 0,
        ee_magic: 0,
        ee_version: 0,
        ee_hdr: 0,
        ee_misc5: 0,
        ee_regdom: 0,
        mac_ok: 0,
        reserved: 0,
        mac: [0; 6],
        pad: [0; 2],
    };
}

const _: () = {
    assert!(core::mem::size_of::<WifiProbeInfo>() == 44);
};

/// Preserve partial stage information on failure. A platform-specific
/// late PCI reset recovery, if needed, must happen outside this routine.
pub fn probe_mapped(io:&mut impl RegisterIo,info:&mut WifiProbeInfo,srev:u32){
    info.srev=srev;
    if srev==0||srev==u32::MAX{return;}
    if !wake_chip(io)||!nic_reset(io,AR5K_RESET_CTL_PCU|AR5K_RESET_CTL_BASEBAND|AR5K_RESET_CTL_PCI)
       ||!wake_chip(io)||!nic_reset(io,0){return;}
    if srev&0xff>=AR5K_SREV_AR5213A{
        let v=io.read(AR5K_PCICFG);
        if v==u32::MAX{return;}
        io.write(AR5K_PCICFG,v|AR5K_PCICFG_RETRY_FIX);
    }
    info.phy_id=io.read(AR5K_PHY_CHIP_ID);
    if info.phy_id==u32::MAX{return;}
    info.stage=3;
    let Some(magic)=eeprom_read(io,AR5K_EEPROM_MAGIC)else{return;};
    let Some(ver)=eeprom_read(io,AR5K_EEPROM_VERSION)else{return;};
    let Some(hdr)=eeprom_read(io,AR5K_EEPROM_HDR)else{return;};
    let Some(misc5)=eeprom_read(io,AR5K_EEPROM_MISC5)else{return;};
    let Some(regdom)=eeprom_read(io,AR5K_EEPROM_REG_DOMAIN)else{return;};
    info.ee_magic=magic;info.ee_version=ver;info.ee_hdr=hdr;info.ee_misc5=misc5;info.ee_regdom=regdom;
    info.aes_hw=u8::from(decode_aes_support(srev,ver,misc5));
    let _=eeprom_read(io,0x20);
    if let (Some(a),Some(b),Some(c))=(eeprom_read(io,0x1f),eeprom_read(io,0x1e),eeprom_read(io,0x1d)){
        if let Some(mac)=decode_eeprom_mac(a,b,c){info.mac=mac;info.mac_ok=1;}
    }
    info.stage=4;
}
pub fn read_revision(io:&mut impl RegisterIo)->u32{io.read(AR5K_SREV)}
#[cfg(test)]
mod tests{
    use super::*;
    struct Mock{offset:u32,reset:u32,absent:bool}
    impl RegisterIo for Mock{
        fn read(&mut self,o:usize)->u32{
            if self.absent{return u32::MAX;}
            match o{0x4020=>0x78,0x9818=>0x45,0x4000=>self.reset,0x600c=>2,0x6004=>match self.offset{0x3d=>0x5aa5,0xc1=>0x5001,0xc2=>0x0a6e,0xc9=>5,0xbf=>0x10,0x1f=>0x0013,0x1e=>0x46d7,0x1d=>0x848a,_=>0},_=>0}
        }
        fn write(&mut self,o:usize,v:u32){match o{0x4000=>self.reset=v,0x6000=>self.offset=v,_=>{}}}
        fn delay_ms(&mut self,_:u32)->bool{true}
    }
    #[test]fn captured_identity(){let mut m=Mock{offset:0,reset:0,absent:false};let mut i=WifiProbeInfo::ZERO;i.stage=2;let s=read_revision(&mut m);probe_mapped(&mut m,&mut i,s);assert_eq!((i.stage,i.phy_id,i.ee_magic,i.aes_hw,i.mac_ok),(4,0x45,0x5aa5,1,1));assert_eq!(i.mac,[0,0x13,0x46,0xd7,0x84,0x8a]);}
    #[test]fn absent_does_not_advance(){let mut m=Mock{offset:0,reset:0,absent:true};let mut i=WifiProbeInfo::ZERO;i.stage=2;probe_mapped(&mut m,&mut i,u32::MAX);assert_eq!(i.stage,2);}
}
