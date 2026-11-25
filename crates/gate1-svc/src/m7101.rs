//! ALi M7101 Power Management Unit (PCI 10B9:7101) driver for Ferric DOS.
//!
//! Supports both the ALi M1535D+ (DynaBook SS 2000M, `i2c-ali1535` layout:
//! SMBBA at PCI 0xE2, SMBCFG at 0xD1, SMBHSTCFG at 0xF0) and the M1533/M1543
//! fallback layout (PMBA at 0x10, SMBBA at 0x14, SMBHSTCFG at 0xE0):
//! - Smart Battery System (SBS v1.1) telemetry over SMBus (7-bit addr 0x0B)
//! - CPU clock throttling via ACPI `P_CNTRL` (`pm_base + 0x10`)
//! - LCD CCFT backlight PWM via M7101 PCI config `0xB0`
//!
//! Strictly 386-pure (`no_std`, no SSE2 `pause`), read-only with respect to storage.

use crate::{cardbus, inb, inl, outb, outl, pci_read32, pci_write32};

const ALI_VID: u16 = 0x10B9;
const M7101_DID: u16 = 0x7101;

// M1535D+ PCI config offsets (Linux i2c-ali1535.c)
const PCI_SMBCFG_1535: u8 = 0xD1;
const PCI_PMBA_1535: u8 = 0xE0;
const PCI_SMBBA_1535: u8 = 0xE2;
const PCI_SMBHSTCFG_1535: u8 = 0xF0;
const PCI_SMBCLK_1535: u8 = 0xF2;

// M1533/M1543 PCI config offsets (M1533 datasheet v1.56)
const PCI_PMBA_1533: u8 = 0x10;
const PCI_SMBBA_1533: u8 = 0x14;
const PCI_SMBHSTCFG_1533: u8 = 0xE0;

// LCD CCFT PWM control in M7101 PCI config space (M1533 datasheet p.106)
const PCI_LCD_CCFT: u8 = 0xB0;

// SMBus I/O register offsets (identical on M1533 and M1535D+)
const SMB_HSTSTS: u16 = 0x00;
const SMB_HSTTYP: u16 = 0x01;
const SMB_HSTPORT: u16 = 0x02;
const SMB_HSTADD: u16 = 0x03;
const SMB_HSTDAT0: u16 = 0x04;
const SMB_HSTDAT1: u16 = 0x05;
const SMB_HSTCMD: u16 = 0x07;

// SMBus status bits
const STS_IDLE: u8 = 0x04;
const STS_BUSY: u8 = 0x08;
const STS_DONE: u8 = 0x10;
const STS_DEV: u8 = 0x20;
const STS_BUSERR: u8 = 0x40;
const STS_FAIL: u8 = 0x80;
const STS_ERR: u8 = STS_DEV | STS_BUSERR | STS_FAIL;

// SMBus command types
const CMD_WORD_DATA: u8 = 0x30;
const CMD_T_OUT: u8 = 0x08;
const CMD_KILL: u8 = 0x04;

// Smart Battery System (SBS v1.1) 7-bit SMBus address and command codes
const SBS_ADDR_7BIT: u8 = 0x0B; // 8-bit write=0x16, read=0x17
const SBS_CMD_TEMP: u8 = 0x08;
const SBS_CMD_VOLTAGE: u8 = 0x09;
const SBS_CMD_CURRENT: u8 = 0x0A;
const SBS_CMD_RSOC: u8 = 0x0D;
const SBS_CMD_REM_CAP: u8 = 0x0F;
const SBS_CMD_FULL_CAP: u8 = 0x10;
const SBS_CMD_AVG_TTE: u8 = 0x12;
const SBS_CMD_AVG_TTF: u8 = 0x13;
const SBS_CMD_STATUS: u8 = 0x16;
const SBS_CMD_CYCLES: u8 = 0x17;
const SBS_CMD_DESIGN_CAP: u8 = 0x18;
const SBS_CMD_DESIGN_V: u8 = 0x19;

// ACPI P_CNTRL register offset from pm_base and bit definitions
const PM_P_CNTRL_OFF: u16 = 0x10;
const PCNTRL_THRO_DTY_SHIFT: u32 = 1;
const PCNTRL_THRO_DTY_MASK: u32 = 0x0000_000E;
const PCNTRL_THRO_EN: u32 = 1 << 4;
const PCNTRL_CLK_EN: u32 = 1 << 9;
const PCNTRL_THRO_STS: u32 = 1 << 17;

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct M7101Locate {
    pub bus: u8,
    pub dev: u8,
    pub func: u8,
    pub rev: u8,
    pub pm_base: u16,
    pub smb_base: u16,
    pub is_1535: bool,
    pub smb_enabled: bool,
    pub raw_10: u32,
    pub raw_14: u32,
    pub raw_d0: u32,
    pub raw_e0: u32,
    pub raw_f0: u32,
    pub raw_b0: u8,
}

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SmartBattery {
    pub present: bool,
    pub temp_dk: u16,        // 0.1 Kelvin
    pub voltage_mv: u16,     // millivolts
    pub current_ma: i16,     // milliamps (+chg, -dischg)
    pub rsoc_pct: u16,       // 0..100 %
    pub rem_cap: u16,        // mAh (or 10 mWh)
    pub full_cap: u16,       // mAh (or 10 mWh)
    pub design_cap: u16,     // mAh (or 10 mWh)
    pub design_mv: u16,      // mV
    pub avg_tte_min: u16,    // minutes (0xFFFF = N/A)
    pub avg_ttf_min: u16,    // minutes (0xFFFF = N/A)
    pub status: u16,         // SBS BatteryStatus bitmask
    pub cycles: u16,         // CycleCount
}

impl SmartBattery {
    pub const EMPTY: Self = Self {
        present: false,
        temp_dk: 0,
        voltage_mv: 0,
        current_ma: 0,
        rsoc_pct: 0,
        rem_cap: 0,
        full_cap: 0,
        design_cap: 0,
        design_mv: 0,
        avg_tte_min: 0xFFFF,
        avg_ttf_min: 0xFFFF,
        status: 0,
        cycles: 0,
    };
}

/// Pure helper: convert SBS temperature in 0.1 K to tenths of degrees Celsius.
/// Example: 2982 (298.2 K) -> 251 (25.1 C).
pub const fn sbs_temp_tenths_c(temp_dk: u16) -> i32 {
    (temp_dk as i32) - 2731
}

/// Pure helper: compute instantaneous battery power in milliwatts from mV and mA.
pub const fn sbs_power_mw(voltage_mv: u16, current_ma: i16) -> i32 {
    ((voltage_mv as i32) * (current_ma as i32)) / 1000
}

/// Pure helper: encode P_CNTRL register for CPU clock throttle level `0..=7`.
/// `0` disables throttling (100% performance); `1..=7` enables `level/8` duty.
pub const fn encode_p_cntrl_throttle(prev: u32, level: u8) -> u32 {
    let base = prev & !(PCNTRL_THRO_EN | PCNTRL_THRO_DTY_MASK);
    if level == 0 || level > 7 {
        base
    } else {
        base | PCNTRL_CLK_EN | PCNTRL_THRO_EN | (((level as u32) & 0x7) << PCNTRL_THRO_DTY_SHIFT)
    }
}

/// Pure helper: decode P_CNTRL throttle level (`0` = disabled/100%, `1..=7` = duty/8).
pub const fn decode_p_cntrl_throttle(reg: u32) -> u8 {
    if (reg & PCNTRL_THRO_EN) == 0 {
        0
    } else {
        ((reg & PCNTRL_THRO_DTY_MASK) >> PCNTRL_THRO_DTY_SHIFT) as u8
    }
}

/// Pure helper: encode M7101 PCI 0xB0 LCD CCFT byte for backlight level `0..=8`.
/// - `0`: CCFT disabled (backlight off)
/// - `1..=7`: CCFT PWM enabled with duty `0x10..=0x3A` (hardware minimum is 0x10)
/// - `8`: CCFT enabled with duty `0x00` (full brightness following FPVEE)
pub const fn encode_ccft_level(level: u8) -> u8 {
    match level {
        0 => 0x00,
        1..=7 => 0x40 | (0x10 + (level - 1) * 7),
        _ => 0x40,
    }
}

/// Pure helper: decode M7101 PCI 0xB0 LCD CCFT byte into `0..=8` level.
pub const fn decode_ccft_level(reg: u8) -> u8 {
    if (reg & 0x40) == 0 {
        return 0;
    }
    let duty = reg & 0x3F;
    if duty == 0 {
        return 8;
    }
    if duty <= 0x10 {
        return 1;
    }
    let step = ((duty - 0x10) / 7) + 1;
    if step > 7 { 7 } else { step }
}

// -----------------------------------------------------------------------------
// Bounded ASCII report writer
// -----------------------------------------------------------------------------

struct Out<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Out<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn byte(&mut self, b: u8) {
        if self.pos < self.buf.len() {
            self.buf[self.pos] = b;
            self.pos += 1;
        }
    }
    fn str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.byte(b);
        }
    }
    fn crlf(&mut self) {
        self.byte(b'\r');
        self.byte(b'\n');
    }
    fn hex8(&mut self, v: u8) {
        const H: &[u8; 16] = b"0123456789ABCDEF";
        self.byte(H[(v >> 4) as usize]);
        self.byte(H[(v & 0xF) as usize]);
    }
    fn hex16(&mut self, v: u16) {
        self.hex8((v >> 8) as u8);
        self.hex8((v & 0xFF) as u8);
    }
    fn hex32(&mut self, v: u32) {
        self.hex16((v >> 16) as u16);
        self.hex16((v & 0xFFFF) as u16);
    }
    fn dec_u32(&mut self, mut v: u32) {
        let mut tmp = [0u8; 10];
        let mut n = 0usize;
        if v == 0 {
            self.byte(b'0');
            return;
        }
        while v > 0 {
            tmp[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            self.byte(tmp[n]);
        }
    }
    fn dec_i32(&mut self, v: i32) {
        if v < 0 {
            self.byte(b'-');
            self.dec_u32(v.wrapping_neg() as u32);
        } else {
            self.dec_u32(v as u32);
        }
    }
}

// -----------------------------------------------------------------------------
// PCI config byte/word helpers (built on dword pci_read32/pci_write32)
// -----------------------------------------------------------------------------

unsafe fn pci_read8(bus: u8, dev: u8, func: u8, off: u8) -> u8 {
    let dw = unsafe { pci_read32(bus, dev, func, off & !3) };
    ((dw >> ((off & 3) * 8)) & 0xFF) as u8
}

unsafe fn pci_read16(bus: u8, dev: u8, func: u8, off: u8) -> u16 {
    let dw = unsafe { pci_read32(bus, dev, func, off & !3) };
    ((dw >> ((off & 2) * 8)) & 0xFFFF) as u16
}

unsafe fn pci_write8(bus: u8, dev: u8, func: u8, off: u8, val: u8) {
    let aligned = off & !3;
    let shift = (off & 3) * 8;
    let mut dw = unsafe { pci_read32(bus, dev, func, aligned) };
    dw &= !(0xFFu32 << shift);
    dw |= (val as u32) << shift;
    unsafe { pci_write32(bus, dev, func, aligned, dw) };
}

unsafe fn locate_m7101() -> Option<M7101Locate> {
    for dev in 0..32u8 {
        let id0 = unsafe { pci_read32(0, dev, 0, 0) };
        if (id0 & 0xFFFF) == 0xFFFF {
            continue;
        }
        let hdr = ((unsafe { pci_read32(0, dev, 0, 0x0C) }) >> 16) & 0xFF;
        let max_fn = if (hdr & 0x80) != 0 { 8u8 } else { 1u8 };
        for func in 0..max_fn {
            let id = unsafe { pci_read32(0, dev, func, 0) };
            let vid = (id & 0xFFFF) as u16;
            let did = (id >> 16) as u16;
            if vid == ALI_VID && did == M7101_DID {
                let rev = (unsafe { pci_read32(0, dev, func, 0x08) } & 0xFF) as u8;
                let raw_10 = unsafe { pci_read32(0, dev, func, PCI_PMBA_1533) };
                let raw_14 = unsafe { pci_read32(0, dev, func, PCI_SMBBA_1533) };
                let raw_d0 = unsafe { pci_read32(0, dev, func, 0xD0) };
                let raw_e0 = unsafe { pci_read32(0, dev, func, 0xE0) };
                let raw_f0 = unsafe { pci_read32(0, dev, func, 0xF0) };
                let raw_b0 = unsafe { pci_read8(0, dev, func, PCI_LCD_CCFT) };

                // Check M1535D+ layout first (PCI 0xE2 SMBBA, 0xE0 PMBA, 0xD1 bit 2, 0xF0 bit 0)
                let smb_1535 = unsafe { pci_read16(0, dev, func, PCI_SMBBA_1535) } & 0xFFE0;
                let pm_1535 = unsafe { pci_read16(0, dev, func, PCI_PMBA_1535) } & 0xFFC0;
                let smbcfg_1535 = unsafe { pci_read8(0, dev, func, PCI_SMBCFG_1535) };
                let smbhst_1535 = unsafe { pci_read8(0, dev, func, PCI_SMBHSTCFG_1535) };

                let smb_1533 = (raw_14 as u16) & 0xFFE0;
                let pm_1533 = (raw_10 as u16) & 0xFFC0;
                let smbhst_1533 = unsafe { pci_read8(0, dev, func, PCI_SMBHSTCFG_1533) };

                let (is_1535, smb_base, smb_enabled) = if smb_1535 != 0 && smb_1535 != 0xFFE0 {
                    (true, smb_1535, (smbcfg_1535 & 0x04) != 0 && (smbhst_1535 & 0x01) != 0)
                } else if smb_1533 != 0 && smb_1533 != 0xFFE0 {
                    (false, smb_1533, (smbhst_1533 & 0x01) != 0)
                } else {
                    (true, 0, false)
                };

                let pm_base = if pm_1533 != 0 && pm_1533 != 0xFFC0 {
                    pm_1533
                } else if pm_1535 != 0 && pm_1535 != 0xFFC0 {
                    pm_1535
                } else {
                    0
                };

                return Some(M7101Locate {
                    bus: 0,
                    dev,
                    func,
                    rev,
                    pm_base,
                    smb_base,
                    is_1535,
                    smb_enabled,
                    raw_10,
                    raw_14,
                    raw_d0,
                    raw_e0,
                    raw_f0,
                    raw_b0,
                });
            }
        }
    }
    None
}

// -----------------------------------------------------------------------------
// ALi M1535D+ / M7101 SMBus Word Read (polled, bounded timeout)
// -----------------------------------------------------------------------------

unsafe fn smb_read_word(smb_base: u16, addr_7bit: u8, cmd: u8) -> Option<u16> {
    // Wait for bus idle (up to 40 ms)
    let mut sts = unsafe { inb(smb_base + SMB_HSTSTS) };
    let mut idle_tries = 0u32;
    while (sts & STS_IDLE) == 0 && idle_tries < 40 {
        cardbus::delay_ms(1);
        sts = unsafe { inb(smb_base + SMB_HSTSTS) };
        idle_tries += 1;
    }

    // Clear all sticky status bits
    unsafe { outb(smb_base + SMB_HSTSTS, 0xFF) };

    // Program read-word transaction
    unsafe {
        outb(smb_base + SMB_HSTADD, ((addr_7bit & 0x7F) << 1) | 0x01);
        outb(smb_base + SMB_HSTTYP, CMD_WORD_DATA);
        outb(smb_base + SMB_HSTCMD, cmd);
    }

    // Ensure host is not wedged busy before starting
    sts = unsafe { inb(smb_base + SMB_HSTSTS) };
    if (sts & STS_BUSY) != 0 {
        unsafe {
            outb(smb_base + SMB_HSTTYP, CMD_T_OUT);
            outb(smb_base + SMB_HSTSTS, 0xFF);
        }
        sts = unsafe { inb(smb_base + SMB_HSTSTS) };
        if (sts & (STS_ERR | STS_BUSY)) != 0 {
            return None;
        }
    } else if (sts & (STS_ERR | STS_DONE)) != 0 {
        unsafe { outb(smb_base + SMB_HSTSTS, 0xFF) };
    }

    // Trigger transaction
    unsafe { outb(smb_base + SMB_HSTPORT, 0xFF) };

    // Poll until not busy or idle (up to 60 ms)
    let mut tries = 0u32;
    loop {
        cardbus::delay_ms(1);
        sts = unsafe { inb(smb_base + SMB_HSTSTS) };
        if (sts & STS_BUSY) == 0 || (sts & STS_IDLE) != 0 {
            break;
        }
        tries += 1;
        if tries >= 60 {
            unsafe {
                outb(smb_base + SMB_HSTTYP, CMD_KILL);
                outb(smb_base + SMB_HSTSTS, 0xFF);
            }
            return None;
        }
    }

    if (sts & STS_ERR) != 0 || (sts & STS_DONE) == 0 {
        if (sts & STS_DONE) == 0 {
            unsafe { outb(smb_base + SMB_HSTTYP, CMD_KILL) };
        } else {
            unsafe { outb(smb_base + SMB_HSTTYP, CMD_T_OUT) };
        }
        unsafe { outb(smb_base + SMB_HSTSTS, 0xFF) };
        return None;
    }

    let lo = unsafe { inb(smb_base + SMB_HSTDAT0) } as u16;
    let hi = unsafe { inb(smb_base + SMB_HSTDAT1) } as u16;
    unsafe { outb(smb_base + SMB_HSTSTS, 0xFF) };
    Some(lo | (hi << 8))
}

unsafe fn query_smart_battery(loc: &M7101Locate) -> SmartBattery {
    let mut b = SmartBattery::EMPTY;
    if loc.smb_base == 0 || !loc.smb_enabled {
        return b;
    }
    if loc.is_1535 {
        // Ensure recommended 74 kHz SMBus clock (per i2c-ali1535.c)
        let clk = unsafe { pci_read8(loc.bus, loc.dev, loc.func, PCI_SMBCLK_1535) };
        if clk == 0 {
            unsafe { pci_write8(loc.bus, loc.dev, loc.func, PCI_SMBCLK_1535, 0x20) };
        }
    }

    let Some(v) = (unsafe { smb_read_word(loc.smb_base, SBS_ADDR_7BIT, SBS_CMD_VOLTAGE) }) else {
        return b;
    };
    b.present = true;
    b.voltage_mv = v;
    if let Some(c) = unsafe { smb_read_word(loc.smb_base, SBS_ADDR_7BIT, SBS_CMD_CURRENT) } {
        b.current_ma = c as i16;
    }
    if let Some(t) = unsafe { smb_read_word(loc.smb_base, SBS_ADDR_7BIT, SBS_CMD_TEMP) } {
        b.temp_dk = t;
    }
    if let Some(r) = unsafe { smb_read_word(loc.smb_base, SBS_ADDR_7BIT, SBS_CMD_RSOC) } {
        b.rsoc_pct = r;
    }
    if let Some(rc) = unsafe { smb_read_word(loc.smb_base, SBS_ADDR_7BIT, SBS_CMD_REM_CAP) } {
        b.rem_cap = rc;
    }
    if let Some(fc) = unsafe { smb_read_word(loc.smb_base, SBS_ADDR_7BIT, SBS_CMD_FULL_CAP) } {
        b.full_cap = fc;
    }
    if let Some(dc) = unsafe { smb_read_word(loc.smb_base, SBS_ADDR_7BIT, SBS_CMD_DESIGN_CAP) } {
        b.design_cap = dc;
    }
    if let Some(dv) = unsafe { smb_read_word(loc.smb_base, SBS_ADDR_7BIT, SBS_CMD_DESIGN_V) } {
        b.design_mv = dv;
    }
    if let Some(tte) = unsafe { smb_read_word(loc.smb_base, SBS_ADDR_7BIT, SBS_CMD_AVG_TTE) } {
        b.avg_tte_min = tte;
    }
    if let Some(ttf) = unsafe { smb_read_word(loc.smb_base, SBS_ADDR_7BIT, SBS_CMD_AVG_TTF) } {
        b.avg_ttf_min = ttf;
    }
    if let Some(st) = unsafe { smb_read_word(loc.smb_base, SBS_ADDR_7BIT, SBS_CMD_STATUS) } {
        b.status = st;
    }
    if let Some(cy) = unsafe { smb_read_word(loc.smb_base, SBS_ADDR_7BIT, SBS_CMD_CYCLES) } {
        b.cycles = cy;
    }
    b
}

// -----------------------------------------------------------------------------
// V86 API entry points (AX=0x0039, 0x003A, 0x003B)
// -----------------------------------------------------------------------------

/// Verb `0x0039`: Probe ALi M7101 PMU, Smart Battery SMBus, CPU throttle, and LCD CCFT.
/// Writes a human-readable ASCII report into `out_ptr[..out_cap]`, returns bytes written.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_m7101_info(out_ptr: u32, out_cap: u32) -> u32 {
    if out_ptr == 0 || out_cap == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_cap as usize) };
    let mut o = Out::new(slice);

    let Some(loc) = (unsafe { locate_m7101() }) else {
        o.str("M7101: not present on PCI bus 0");
        o.crlf();
        return o.pos as u32;
    };

    o.str("M7101: bdf=00:");
    o.hex8(loc.dev);
    o.byte(b'.');
    o.dec_u32(loc.func as u32);
    o.str(" rev=0x");
    o.hex8(loc.rev);
    o.str(if loc.is_1535 { " (M1535D+)" } else { " (M1533)" });
    o.str(" pm_io=0x");
    o.hex16(loc.pm_base);
    o.str(" smb_io=0x");
    o.hex16(loc.smb_base);
    o.str(" smb_en=");
    o.byte(if loc.smb_enabled { b'1' } else { b'0' });
    o.crlf();

    // CPU Throttle state via ACPI P_CNTRL (pm_base + 0x10)
    if loc.pm_base != 0 {
        let p_cntrl = unsafe { inl(loc.pm_base + PM_P_CNTRL_OFF) };
        let lvl = decode_p_cntrl_throttle(p_cntrl);
        o.str("CPU Throttle: ");
        if lvl == 0 {
            o.str("OFF (100% speed)");
        } else {
            o.str("ON level=");
            o.dec_u32(lvl as u32);
            o.str("/8 (");
            o.dec_u32((lvl as u32) * 125 / 10);
            o.str("%)");
        }
        o.str(" P_CNTRL=0x");
        o.hex32(p_cntrl);
        if (p_cntrl & PCNTRL_THRO_STS) != 0 {
            o.str(" [ACTIVE]");
        }
        o.crlf();
    } else {
        o.str("CPU Throttle: PM I/O base unassigned (raw10=0x");
        o.hex32(loc.raw_10);
        o.str(" rawE0=0x");
        o.hex32(loc.raw_e0);
        o.byte(b')');
        o.crlf();
    }

    // LCD Backlight CCFT PWM via PCI 0xB0
    let bl = decode_ccft_level(loc.raw_b0);
    o.str("LCD Backlight: level=");
    o.dec_u32(bl as u32);
    o.str("/8 (PCI 0xB0=0x");
    o.hex8(loc.raw_b0);
    if (loc.raw_b0 & 0x80) != 0 {
        o.str(" HW-PIN");
    }
    o.byte(b')');
    o.crlf();

    // Smart Battery over SMBus (0x0B / 0x16)
    let batt = unsafe { query_smart_battery(&loc) };
    if batt.present {
        o.str("Smart Battery (SMBus 0x0B): ");
        o.dec_u32(batt.rsoc_pct as u32);
        o.str("%  ");
        o.dec_u32((batt.voltage_mv / 1000) as u32);
        o.byte(b'.');
        let mv_frac = batt.voltage_mv % 1000;
        if mv_frac < 100 {
            o.byte(b'0');
        }
        if mv_frac < 10 {
            o.byte(b'0');
        }
        o.dec_u32(mv_frac as u32);
        o.str(" V  ");
        o.dec_i32(batt.current_ma as i32);
        o.str(" mA (");
        let mw = sbs_power_mw(batt.voltage_mv, batt.current_ma);
        o.dec_i32(mw / 1000);
        o.byte(b'.');
        let mw_abs = if mw < 0 { -mw } else { mw };
        o.dec_u32(((mw_abs % 1000) / 100) as u32);
        o.str(" W)");
        o.crlf();

        o.str("  Capacity: ");
        o.dec_u32(batt.rem_cap as u32);
        o.str(" / ");
        o.dec_u32(batt.full_cap as u32);
        o.str(" mAh (design ");
        o.dec_u32(batt.design_cap as u32);
        o.str(" mAh, ");
        o.dec_u32(batt.design_mv as u32);
        o.str(" mV)  cycles=");
        o.dec_u32(batt.cycles as u32);
        o.crlf();

        o.str("  Temp: ");
        if batt.temp_dk > 2000 {
            let tc = sbs_temp_tenths_c(batt.temp_dk);
            o.dec_i32(tc / 10);
            o.byte(b'.');
            o.dec_u32((if tc < 0 { -tc } else { tc } % 10) as u32);
            o.str(" C");
        } else {
            o.str("N/A");
        }
        o.str("  State: ");
        if (batt.status & 0x0020) != 0 {
            o.str("FULL");
        } else if (batt.status & 0x0010) != 0 {
            o.str("EMPTY");
        } else if (batt.status & 0x0040) != 0 {
            o.str("DISCHARGING");
            if batt.avg_tte_min != 0xFFFF {
                o.str(" (");
                o.dec_u32(batt.avg_tte_min as u32);
                o.str(" min left)");
            }
        } else {
            o.str("CHARGING/AC");
            if batt.avg_ttf_min != 0xFFFF {
                o.str(" (");
                o.dec_u32(batt.avg_ttf_min as u32);
                o.str(" min to full)");
            }
        }
        o.str(" [0x");
        o.hex16(batt.status);
        o.byte(b']');
        o.crlf();
    } else {
        o.str("Smart Battery: no SMBus ACK at 0x0B (cfgD0=0x");
        o.hex32(loc.raw_d0);
        o.str(" cfgE0=0x");
        o.hex32(loc.raw_e0);
        o.str(" cfgF0=0x");
        o.hex32(loc.raw_f0);
        o.byte(b')');
        o.crlf();
    }

    o.pos as u32
}

/// Verb `0x003A`: Set M7101 CPU clock throttle level (`0` = OFF/100%, `1..=7` = duty/8).
#[unsafe(no_mangle)]
pub extern "C" fn gate1_m7101_set_throttle(level: u32, out_ptr: u32, out_cap: u32) -> u32 {
    if out_ptr == 0 || out_cap == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_cap as usize) };
    let mut o = Out::new(slice);

    let Some(loc) = (unsafe { locate_m7101() }) else {
        o.str("M7101: not present");
        o.crlf();
        return o.pos as u32;
    };
    if loc.pm_base == 0 {
        o.str("M7101: PM I/O base unassigned; cannot set throttle");
        o.crlf();
        return o.pos as u32;
    }
    let lvl = if level > 7 { 7 } else { level as u8 };
    let prev = unsafe { inl(loc.pm_base + PM_P_CNTRL_OFF) };
    let next = encode_p_cntrl_throttle(prev, lvl);
    unsafe { outl(loc.pm_base + PM_P_CNTRL_OFF, next) };
    let readback = unsafe { inl(loc.pm_base + PM_P_CNTRL_OFF) };

    o.str("M7101 CPU Throttle set: ");
    if lvl == 0 {
        o.str("OFF (100%)");
    } else {
        o.dec_u32(lvl as u32);
        o.str("/8 (");
        o.dec_u32((lvl as u32) * 125 / 10);
        o.str("%)");
    }
    o.str("  P_CNTRL=0x");
    o.hex32(readback);
    o.crlf();
    o.pos as u32
}

/// Verb `0x003B`: Set M7101 LCD CCFT backlight level (`0` = off, `1..=7` = PWM, `8` = max).
#[unsafe(no_mangle)]
pub extern "C" fn gate1_m7101_set_backlight(level: u32, out_ptr: u32, out_cap: u32) -> u32 {
    if out_ptr == 0 || out_cap == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_cap as usize) };
    let mut o = Out::new(slice);

    let Some(loc) = (unsafe { locate_m7101() }) else {
        o.str("M7101: not present");
        o.crlf();
        return o.pos as u32;
    };
    let lvl = if level > 8 { 8 } else { level as u8 };
    let val = encode_ccft_level(lvl);
    unsafe { pci_write8(loc.bus, loc.dev, loc.func, PCI_LCD_CCFT, val) };
    let rb = unsafe { pci_read8(loc.bus, loc.dev, loc.func, PCI_LCD_CCFT) };

    o.str("M7101 LCD Backlight set: level=");
    o.dec_u32(lvl as u32);
    o.str("/8  PCI 0xB0=0x");
    o.hex8(rb);
    o.crlf();
    o.pos as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sbs_temp_and_power_conversion() {
        assert_eq!(sbs_temp_tenths_c(2982), 251); // 25.1 C
        assert_eq!(sbs_temp_tenths_c(2731), 0);   // 0.0 C
        assert_eq!(sbs_power_mw(11100, -1500), -16650); // -16.65 W
        assert_eq!(sbs_power_mw(12400, 1000), 12400);   // +12.40 W
    }

    #[test]
    fn p_cntrl_throttle_roundtrip() {
        let r0 = encode_p_cntrl_throttle(0x0000_0200, 0);
        assert_eq!(decode_p_cntrl_throttle(r0), 0);
        assert_eq!(r0 & PCNTRL_THRO_EN, 0);

        for lvl in 1..=7u8 {
            let r = encode_p_cntrl_throttle(0, lvl);
            assert_eq!(decode_p_cntrl_throttle(r), lvl);
            assert_ne!(r & PCNTRL_CLK_EN, 0);
            assert_ne!(r & PCNTRL_THRO_EN, 0);
        }
    }

    #[test]
    fn ccft_backlight_roundtrip() {
        assert_eq!(encode_ccft_level(0), 0x00);
        assert_eq!(decode_ccft_level(0x00), 0);
        assert_eq!(encode_ccft_level(8), 0x40);
        assert_eq!(decode_ccft_level(0x40), 8);
        for lvl in 1..=7u8 {
            let reg = encode_ccft_level(lvl);
            assert_ne!(reg & 0x40, 0);
            assert!((reg & 0x3F) >= 0x10);
            assert_eq!(decode_ccft_level(reg), lvl);
        }
    }
}
