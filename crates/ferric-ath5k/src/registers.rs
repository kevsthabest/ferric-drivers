//! Bounded MAC wake, reset and EEPROM transactions with injected register I/O.
pub trait RegisterIo {
    fn read(&mut self,offset:usize)->u32;
    fn write(&mut self,offset:usize,value:u32);
    fn delay_ms(&mut self,ms:u32)->bool;
}
const AR5K_STA_ID1: usize = 0x8004;
const AR5K_STA_ID1_PWR_SV: u32 = 0x0004_0000;
const AR5K_SLEEP_CTL: usize = 0x4004;
const AR5K_SLEEP_CTL_SLE: u32 = 0x0003_0000;
const AR5K_SLEEP_CTL_SLE_WAKE: u32 = 0x0000_0000;
const AR5K_PCICFG: usize = 0x4010;
const AR5K_PCICFG_SPWR_DN: u32 = 0x0001_0000;
const AR5K_RXDP: usize = 0x000C;
const AR5K_RESET_CTL: usize = 0x4000;
const AR5K_RESET_CTL_PCU: u32 = 0x0000_0001;
const AR5K_RESET_CTL_BASEBAND: u32 = 0x0000_0002;
const AR5K_CFG: usize = 0x0014;
const AR5K_EEPROM_BASE: usize = 0x6000;
const AR5K_EEPROM_CMD: usize = 0x6008;
const AR5K_EEPROM_CMD_READ: u32 = 0x0000_0001;
const AR5K_EEPROM_STATUS: usize = 0x600C;
const AR5K_EEPROM_STAT_RDDONE: u32 = 0x0000_0002;
const AR5K_EEPROM_STAT_RDERR: u32 = 0x0000_0001;
const AR5K_EEPROM_DATA: usize = 0x6004;

pub fn wake_chip(io: &mut impl RegisterIo) -> bool {
    let mut staid = io.read(AR5K_STA_ID1);
    if staid == u32::MAX { return false; }
    staid &= !AR5K_STA_ID1_PWR_SV;

    let mut data = io.read(AR5K_SLEEP_CTL);
    if data == u32::MAX { return false; }
    if (data & 0xFFC0_0000) != 0 {
        data = 0;
    } else {
        data &= !AR5K_SLEEP_CTL_SLE;
    }
    io.write(AR5K_SLEEP_CTL, data | AR5K_SLEEP_CTL_SLE_WAKE);
    if !io.delay_ms(1) { return false; }

    let mut awake = false;
    for _ in 0..100 {
        let status=io.read(AR5K_PCICFG);
        if status==u32::MAX{return false;}
        if (status & AR5K_PCICFG_SPWR_DN) == 0 {
            awake = true;
            break;
        }
        if !io.delay_ms(1) { return false; }
        io.write(AR5K_SLEEP_CTL, data | AR5K_SLEEP_CTL_SLE_WAKE);
    }
    if !awake {
        return false;
    }
    io.write(AR5K_STA_ID1, staid);
    true
}


pub fn nic_reset(io: &mut impl RegisterIo, val: u32) -> bool {
    if io.read(AR5K_RXDP)==u32::MAX{return false;}
    io.write(AR5K_RESET_CTL, val);
    if !io.delay_ms(1) { return false; }

    let mask = AR5K_RESET_CTL_PCU | AR5K_RESET_CTL_BASEBAND;
    let expected = val & mask;
    let mut ok = false;
    for _ in 0..100 {
        let status=io.read(AR5K_RESET_CTL);
        if status==u32::MAX{return false;}
        if (status & mask) == expected {
            ok = true;
            break;
        }
        if !io.delay_ms(1) { return false; }
    }
    if (val & AR5K_RESET_CTL_PCU) == 0 {
        io.write(AR5K_CFG, 0);
    }
    ok
}


pub fn eeprom_read(io: &mut impl RegisterIo, offset: u32) -> Option<u16> {
    io.write(AR5K_EEPROM_BASE, offset);
    let cmd = io.read(AR5K_EEPROM_CMD);
    if cmd==u32::MAX{return None;}
    io.write(AR5K_EEPROM_CMD, cmd | AR5K_EEPROM_CMD_READ);

    for _ in 0..5000 {
        let status = io.read(AR5K_EEPROM_STATUS);
        if status==u32::MAX{return None;}
        if (status & AR5K_EEPROM_STAT_RDDONE) != 0 {
            if (status & AR5K_EEPROM_STAT_RDERR) != 0 {
                return None;
            }
            return Some((io.read(AR5K_EEPROM_DATA) & 0xFFFF) as u16);
        }
    }
    None
}

#[cfg(test)]
mod tests{
    use super::*;
    struct Mock{absent:bool,stuck:bool,timer:bool,writes:usize,reset:u32}
    impl RegisterIo for Mock{
        fn read(&mut self,o:usize)->u32{
            if self.absent{return u32::MAX;}
            match o {AR5K_PCICFG=>if self.stuck{AR5K_PCICFG_SPWR_DN}else{0},AR5K_RESET_CTL=>self.reset,AR5K_EEPROM_STATUS=>if self.stuck{0}else{AR5K_EEPROM_STAT_RDDONE},AR5K_EEPROM_DATA=>0x5aa5,_=>0}
        }
        fn write(&mut self,o:usize,v:u32){self.writes+=1;if o==AR5K_RESET_CTL{self.reset=v;}}
        fn delay_ms(&mut self,_:u32)->bool{self.timer}
    }
    fn fixture()->Mock{Mock{absent:false,stuck:false,timer:true,writes:0,reset:0}}
    #[test]fn wake_reset_eeprom(){let mut m=fixture();assert!(wake_chip(&mut m));assert!(nic_reset(&mut m,3));assert!(nic_reset(&mut m,0));assert_eq!(eeprom_read(&mut m,0),Some(0x5aa5));}
    #[test]fn absent_and_timeout(){let mut m=fixture();m.absent=true;assert!(!wake_chip(&mut m));assert!(!nic_reset(&mut m,3));assert_eq!(eeprom_read(&mut m,0),None);m.absent=false;m.stuck=true;assert!(!wake_chip(&mut m));assert_eq!(eeprom_read(&mut m,0),None);m.stuck=false;m.timer=false;assert!(!wake_chip(&mut m));}
}
