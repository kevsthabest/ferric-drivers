//! Read-only Microwire EEPROM and MDI PHY protocol. Port I/O is supplied by
//! the device adapter, allowing host tests without hardware access.

const EE_CS: u8 = 0x02;
const EE_SK: u8 = 0x01;
const EE_DI: u8 = 0x04;
const EE_DO: u8 = 0x08;

pub trait EepromIo {
    fn write_control(&mut self, value: u8);
    fn read_control(&mut self) -> u8;
}

fn clock(io: &mut impl EepromIo, base: u8) {
    io.write_control(base | EE_SK);
    for _ in 0..16 { let _ = io.read_control(); }
    io.write_control(base);
}

/// Read a single word. Only supported address widths are accepted.
pub fn read_word(io: &mut impl EepromIo, addr: u8, width: u8) -> Option<u16> {
    if ![6, 7, 8].contains(&width) || (u16::from(addr) >= (1u16 << width)) {
        return None;
    }
    io.write_control(0);
    io.write_control(EE_CS);
    for bit in [true, true, false] {
        clock(io, EE_CS | if bit { EE_DI } else { 0 });
    }
    for i in (0..width).rev() {
        clock(io, EE_CS | if (addr >> i) & 1 != 0 { EE_DI } else { 0 });
    }
    let mut data = 0u16;
    for _ in 0..16 {
        io.write_control(EE_CS | EE_SK);
        for _ in 0..16 { let _ = io.read_control(); }
        let sample = io.read_control() & EE_DO;
        io.write_control(EE_CS);
        data = (data << 1) | u16::from(sample != 0);
    }
    io.write_control(0);
    for _ in 0..16 { let _ = io.read_control(); }
    Some(data)
}

fn words_to_mac(w: [u16; 3]) -> [u8; 6] {
    [w[0] as u8, (w[0] >> 8) as u8, w[1] as u8, (w[1] >> 8) as u8,
     w[2] as u8, (w[2] >> 8) as u8]
}

/// Preserve legacy width selection: checksum-valid image first, then
/// a plausible unicast MAC from the same ordered widths.
pub fn select_mac(mut read: impl FnMut(u8, u8) -> u16) -> Option<([u8; 6], u8, bool)> {
    for width in [6u8, 8, 7] {
        let mut sum = 0u16;
        let mut words = [0u16; 3];
        for addr in 0..64u8 {
            let w = read(addr, width);
            if addr < 3 { words[addr as usize] = w; }
            sum = sum.wrapping_add(w);
        }
        if sum == 0xBABA {
            return Some((words_to_mac(words), width, true));
        }
    }
    for width in [6u8, 8, 7] {
        let w0 = read(0, width);
        let w1 = read(1, width);
        let w2 = read(2, width);
        let mac = words_to_mac([w0, w1, w2]);
        if w0 != 0 && w1 != 0 && mac != [0; 6] && mac != [0xFF; 6] && mac[0] & 1 == 0 {
            return Some((mac, width, false));
        }
    }
    None
}

pub fn read_mac(io: &mut impl EepromIo) -> Option<([u8; 6], u8, bool)> {
    select_mac(|addr, width| read_word(io, addr, width).unwrap_or(0))
}

pub trait MdiIo {
    fn write_mdi(&mut self, value: u32);
    fn read_mdi(&mut self) -> u32;
}

/// Return None on timeout or invalid address.
pub fn mdi_read(io: &mut impl MdiIo, phy: u8, reg: u8, budget: u32) -> Option<u16> {
    if phy > 31 || reg > 31 { return None; }
    io.write_mdi(0x0800_0000 | (u32::from(phy) << 21) | (u32::from(reg) << 16));
    for _ in 0..budget {
        let v = io.read_mdi();
        if v & 0x1000_0000 != 0 { return Some(v as u16); }
    }
    None
}

/// BMSR link status is latch-low: sample twice to obtain the live bit.
pub fn link_is_up(io: &mut impl MdiIo, phy: u8) -> bool {
    let _ = mdi_read(io, phy, 1, 100_000);
    mdi_read(io, phy, 1, 100_000).is_some_and(|v| v & 4 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn checksum_width_precedes_fallback() {
        let words = [0x1202u16, 0x3456, 0x789A];
        let checksum = 0xBABAu16.wrapping_sub(words[0]).wrapping_sub(words[1]).wrapping_sub(words[2]);
        let got = select_mac(|addr, width| {
            if addr < 3 { words[addr as usize] }
            else if width == 8 && addr == 63 { checksum }
            else { 0 }
        });
        assert_eq!(got, Some(([2, 0x12, 0x56, 0x34, 0x9A, 0x78], 8, true)));
    }
    #[test]
    fn fallback_and_absent_eeprom() {
        assert_eq!(select_mac(|addr, _| match addr { 0 => 0x1202, 1 => 0x3456, 2 => 0x789A, _ => 0 }),
            Some(([2, 0x12, 0x56, 0x34, 0x9A, 0x78], 6, false)));
        assert_eq!(select_mac(|_, _| 0xFFFF), None);
        assert_eq!(select_mac(|_, _| 0), None);
    }
    struct Ee { writes: [u8; 64], count: usize }
    impl EepromIo for Ee {
        fn write_control(&mut self, v: u8) { self.writes[self.count] = v; self.count += 1; }
        fn read_control(&mut self) -> u8 { EE_DO }
    }
    #[test]
    fn microwire_read_only_sequence() {
        let mut ee = Ee { writes: [0; 64], count: 0 };
        assert_eq!(read_word(&mut ee, 64, 6), None);
        assert_eq!(ee.count, 0);
        assert_eq!(read_word(&mut ee, 0, 6), Some(0xFFFF));
        assert_eq!(&ee.writes[..8], &[0, EE_CS, EE_CS|EE_DI|EE_SK, EE_CS|EE_DI,
            EE_CS|EE_DI|EE_SK, EE_CS|EE_DI, EE_CS|EE_SK, EE_CS]);
        assert_eq!(ee.writes[ee.count-1], 0);
    }
    struct Mdi { writes: u32, reads: u32, last: u32, ready: bool }
    impl MdiIo for Mdi {
        fn write_mdi(&mut self, v: u32) { self.writes += 1; self.last = v; }
        fn read_mdi(&mut self) -> u32 {
            self.reads += 1;
            if !self.ready { 0 } else if self.writes == 1 { 0x1000_0000 } else { 0x1000_0004 }
        }
    }
    #[test]
    fn phy_latch_low_and_timeout() {
        let mut m = Mdi { writes: 0, reads: 0, last: 0, ready: true };
        assert!(link_is_up(&mut m, 1));
        assert_eq!(m.writes, 2);
        assert_eq!(m.last, 0x0821_0000);
        m.ready = false;
        let before = m.reads;
        assert_eq!(mdi_read(&mut m, 1, 1, 3), None);
        assert_eq!(m.reads - before, 3);
    }
}
