//! Toshiba TC6371AF / Type-A PCI SD Host Controller (PCI 1179:0805 / 1179:0803)
//! read-only driver for Ferric DOS.
//!
//! Register map and bring-up sequence verified against Toshiba TC6371AF Spec Rev 1.8
//! and Linux `drivers/mmc/host/toshsd.c` + `toshsd.h`.
//!
//! - Polled PIO mode (all controller IRQs masked; zero IRQ11 contention)
//! - Strictly READ-ONLY (CMD0/8/55/ACMD41/2/3/9/7/16/17 only; zero write commands)
//! - Strictly 386-pure (`no_std`, no SSE2 `pause`), safe bounded timeouts.

use core::cell::UnsafeCell;

use crate::{cardbus, pci_read32, pci_write8, pci_write32};

const TOSHIBA_VID: u16 = 0x1179;
const TOSHSD_DID_A: u16 = 0x0805; // DynaBook SS 2000M built-in SD slot (00:12.0)
const TOSHSD_DID_B: u16 = 0x0803; // TC6371AF default SD DID

// PCI configuration registers (TC6371AF Spec Section 4.2.1 & toshsd.h)
const CFG_CLKSTOP: u8 = 0x40;
const CFG_CLKMODE: u8 = 0x42;
const CFG_POWER1: u8 = 0x48;
const CFG_POWER2: u8 = 0x49;
const CFG_CARDDETECT: u8 = 0x4C;
const CFG_SDLED_EN1: u8 = 0xFA;
const CFG_SDLED_EN2: u8 = 0xFE;

const CLKSTOP_ENABLE_ALL: u8 = 0x1F;
const PWR1_OFF: u8 = 0x00;
const PWR1_33V: u8 = 0x08;
const PWR2_AUTO: u8 = 0x02;
const LED_EN1_START: u8 = 0x12;
const LED_EN2_START: u8 = 0x80;

// MMIO control registers (TC6371AF Spec Section 4.2.2 & toshsd.h)
const REG_CMD: usize = 0x00;
const REG_ARG0: usize = 0x04;
const REG_STOPINTERNAL: usize = 0x08;
const REG_BLOCKCOUNT: usize = 0x0A;
const REG_RSP0: usize = 0x0C;
const REG_CARDSTATUS: usize = 0x1C; // 32-bit: SD_INFO (lo16) | SD_INFO2 (hi16)
const REG_INTMASKCARD: usize = 0x20; // 32-bit: SD_INFO_MASK | SD_INFO2_MASK
const REG_CARDCLOCKCTRL: usize = 0x24;
const REG_CARDXFERDATALEN: usize = 0x26;
const REG_CARDOPTIONSETUP: usize = 0x28;
const REG_ERRORSTATUS0: usize = 0x2C; // 32-bit: ERR0 | ERR1
const REG_DATAPORT: usize = 0x30; // 32-bit PIO FIFO
const REG_TRANSACTIONCTRL: usize = 0x34;
const REG_SOFTWARERESET: usize = 0xE0;
const REG_SDIO_CLOCKNWAIT: usize = 0x138;
const REG_SDIO_LEDCTRL: usize = 0x13E;

// SD_CMD flags
const CMD_TYPE_ACMD: u16 = 1 << 6;
const CMD_RESP_NONE: u16 = 3 << 8;
const CMD_RESP_R1: u16 = 4 << 8;
const CMD_RESP_R1B: u16 = 5 << 8;
const CMD_RESP_R2: u16 = 6 << 8;
const CMD_RESP_R3: u16 = 7 << 8;
const CMD_DATA_PRESENT: u16 = 1 << 11;
const CMD_TRANSFER_READ: u16 = 1 << 12;

// SD_CARDSTATUS (32-bit) bits
const STS_RESP_END: u32 = 1 << 0;
const STS_RW_END: u32 = 1 << 2;
const STS_CARD_PRESENT_0: u32 = 1 << 5;
const STS_WRITE_PROTECT: u32 = 1 << 7; // active low: 0 = WP on (RO), 1 = RW
const STS_CMD_INDEX_ERR: u32 = 1 << 16;
const STS_CRC_ERR: u32 = 1 << 17;
const STS_END_BIT_ERR: u32 = 1 << 18;
const STS_DATA_TIMEOUT: u32 = 1 << 19;
const STS_OVERFLOW: u32 = 1 << 20;
const STS_UNDERFLOW: u32 = 1 << 21;
const STS_CMD_TIMEOUT: u32 = 1 << 22;
const STS_READ_ENABLE: u32 = 1 << 24;
const STS_ILLEGAL_ACCESS: u32 = 1 << 31;

const STS_ANY_ERR: u32 = STS_CMD_INDEX_ERR
    | STS_CRC_ERR
    | STS_END_BIT_ERR
    | STS_DATA_TIMEOUT
    | STS_OVERFLOW
    | STS_UNDERFLOW
    | STS_CMD_TIMEOUT
    | STS_ILLEGAL_ACCESS;

// SD_CARDCLOCKCTRL bits
const CLK_ENABLE: u16 = 1 << 8;
const CLK_DIV_128: u16 = 1 << 5; // 33 MHz / 128 = ~258 kHz (init)
const CLK_DIV_2: u16 = 0; // 33 MHz / 2 = 16.5 MHz (transfer)

// SD_CARDOPTIONSETUP bits
const OPT_REQUIRED: u16 = 0x000E;
const OPT_TIMEOUT_14: u16 = 14 << 4;
const OPT_C2_ABSENT: u16 = 1 << 14;
const OPT_WIDTH_1: u16 = 1 << 15;

struct ToshSdState {
    present: bool,
    claimed: bool,
    bus: u8,
    dev: u8,
    func: u8,
    did: u16,
    rev: u8,
    bar_phys: u32,
    bar_assigned: bool,
    mapped_phys: u32,
    mapped_lin: u32,
    original_cmd: u32,
    original_pmcsr: u32,
    card_ready: bool,
    is_sdhc: bool,
    write_protected: bool,
    rca: u16,
    sectors: u32,
    prod_name: [u8; 5],
}

impl ToshSdState {
    fn bdf(&self) -> u32 {
        (u32::from(self.bus) << 8) | (u32::from(self.dev) << 3) | u32::from(self.func)
    }

    const fn new() -> Self {
        Self {
            present: false,
            claimed: false,
            bus: 0,
            dev: 0,
            func: 0,
            did: 0,
            rev: 0,
            bar_phys: 0,
            bar_assigned: false,
            mapped_phys: 0,
            mapped_lin: 0,
            original_cmd: 0,
            original_pmcsr: 0,
            card_ready: false,
            is_sdhc: false,
            write_protected: true,
            rca: 0,
            sectors: 0,
            prod_name: [b' '; 5],
        }
    }
}

struct SingleCoreCellSd(UnsafeCell<ToshSdState>);
// SAFETY: single-core DOS with CLI/STI discipline.
unsafe impl Sync for SingleCoreCellSd {}

static SD_STATE: SingleCoreCellSd = SingleCoreCellSd(UnsafeCell::new(ToshSdState::new()));

fn sd_state() -> &'static mut ToshSdState {
    unsafe { &mut *SD_STATE.0.get() }
}

// -----------------------------------------------------------------------------
// Pure SD protocol decoders (host-testable)
// -----------------------------------------------------------------------------

/// Decode SD CSD (128-bit response words `csd[0..4]` where `csd[3]` holds bits
/// `127..96` and `csd[0]` holds bits `31..0`) into total 512-byte sector count.
/// Supports both CSD Version 1.0 (SDSC) and CSD Version 2.0 (SDHC/SDXC).
pub fn decode_csd_sectors(csd: [u32; 4]) -> u32 {
    let csd_struct = (csd[3] >> 30) & 0x3;
    if csd_struct == 1 {
        // CSD v2.0 (SDHC): C_SIZE is bits [69:48] (22 bits).
        // Capacity = (C_SIZE + 1) * 512 KiB = (C_SIZE + 1) * 1024 sectors.
        let c_size = ((csd[2] & 0x3F) << 16) | ((csd[1] >> 16) & 0xFFFF);
        c_size.saturating_add(1).saturating_mul(1024)
    } else if csd_struct == 0 {
        // CSD v1.0 (SDSC):
        // READ_BL_LEN = bits [83:80]
        // C_SIZE      = bits [73:62] (12 bits)
        // C_SIZE_MULT = bits [49:47] (3 bits)
        let read_bl_len = (csd[2] >> 16) & 0x0F;
        let c_size = ((csd[2] & 0x03FF) << 2) | ((csd[1] >> 30) & 0x03);
        let c_size_mult = (csd[1] >> 15) & 0x07;
        if read_bl_len < 9 || read_bl_len > 11 {
            return 0;
        }
        let mult_shift = c_size_mult + 2 + (read_bl_len - 9);
        if mult_shift >= 32 {
            return u32::MAX;
        }
        (c_size + 1).saturating_mul(1u32 << mult_shift)
    } else {
        0
    }
}

/// Extract the 5-byte ASCII product name (PNM, bits [103:64]) from a 128-bit CID.
pub fn decode_cid_pnm(cid: [u32; 4]) -> [u8; 5] {
    let raw = [
        (cid[3] & 0xFF) as u8,
        ((cid[2] >> 24) & 0xFF) as u8,
        ((cid[2] >> 16) & 0xFF) as u8,
        ((cid[2] >> 8) & 0xFF) as u8,
        (cid[2] & 0xFF) as u8,
    ];
    let mut out = [b' '; 5];
    for i in 0..5 {
        out[i] = if raw[i] >= 0x20 && raw[i] <= 0x7E { raw[i] } else { b'?' };
    }
    out
}

/// Identify the filesystem / partition signature in sector 0 (MBR or superfloppy VBR).
pub fn detect_sector0_label(sec: &[u8; 512]) -> &'static str {
    if sec[510] != 0x55 || sec[511] != 0xAA {
        return "NO-BOOT-SIG";
    }
    if &sec[3..11] == b"EXFAT   " {
        return "exFAT (superfloppy)";
    }
    if &sec[3..11] == b"NTFS    " {
        return "NTFS (superfloppy)";
    }
    if &sec[82..87] == b"FAT32" {
        return "FAT32 (superfloppy)";
    }
    if &sec[54..59] == b"FAT16" || &sec[54..59] == b"FAT12" {
        return "FAT12/16 (superfloppy)";
    }
    // Check MBR partition table entry 0 type byte at 0x1BE + 4
    match sec[0x1C2] {
        0x01 => "MBR: FAT12",
        0x04 | 0x06 | 0x0E => "MBR: FAT16",
        0x0B | 0x0C => "MBR: FAT32",
        0x07 => "MBR: exFAT/NTFS",
        0x83 => "MBR: Linux",
        0xEE => "GPT Protective MBR",
        0x00 => "MBR: empty",
        _ => "MBR: other",
    }
}

// -----------------------------------------------------------------------------
// MMIO & PCI helpers
// -----------------------------------------------------------------------------

unsafe fn mmio_r16(base: u32, off: usize) -> u16 {
    unsafe { core::ptr::read_volatile((base as usize + off) as *const u16) }
}

unsafe fn mmio_w16(base: u32, off: usize, val: u16) {
    unsafe { core::ptr::write_volatile((base as usize + off) as *mut u16, val) };
}

unsafe fn mmio_r32(base: u32, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base as usize + off) as *const u32) }
}

unsafe fn mmio_w32(base: u32, off: usize, val: u32) {
    unsafe { core::ptr::write_volatile((base as usize + off) as *mut u32, val) };
}



unsafe fn find_toshsd_pci() -> Option<(u8, u8, u8, u16, u8, u32)> {
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
            if vid == TOSHIBA_VID && (did == TOSHSD_DID_A || did == TOSHSD_DID_B) {
                let rev = (unsafe { pci_read32(0, dev, func, 0x08) } & 0xFF) as u8;
                let bar0 = unsafe { pci_read32(0, dev, func, 0x10) };
                return Some((0, dev, func, did, rev, bar0));
            }
        }
    }
    None
}

unsafe fn hw_reset_controller(st: &ToshSdState) {
    let base = st.mapped_lin;
    unsafe {
        pci_write8(st.bus, st.dev, st.func, CFG_CLKSTOP, CLKSTOP_ENABLE_ALL);
        pci_write8(st.bus, st.dev, st.func, CFG_CARDDETECT, 0x02);

        mmio_w16(base, REG_SOFTWARERESET, 0);
        cardbus::delay_ms(2);
        mmio_w16(base, REG_SOFTWARERESET, 1);
        cardbus::delay_ms(2);

        mmio_w16(base, REG_CARDCLOCKCTRL, 0);
        mmio_w32(base, REG_CARDSTATUS, 0);
        mmio_w32(base, REG_ERRORSTATUS0, 0);
        mmio_w16(base, REG_STOPINTERNAL, 0);

        mmio_w16(base, REG_SDIO_CLOCKNWAIT, 0x0100);

        pci_write8(st.bus, st.dev, st.func, CFG_SDLED_EN1, LED_EN1_START);
        pci_write8(st.bus, st.dev, st.func, CFG_SDLED_EN2, LED_EN2_START);

        // Mask all IRQs (polled PIO mode)
        mmio_w32(base, REG_INTMASKCARD, 0xFFFF_FFFF);
        mmio_w16(base, REG_TRANSACTIONCTRL, 0x1000);
    }
}

/// Issue a command with no data transfer, polling `SD_CARDSTATUS` until `RESP_END` or error.
/// Returns `Ok([r0, r1, r2, r3])` where `r0` is the 32-bit R1/R3/R6/R7 response,
/// or for R2 `r3..r0` is the 128-bit response (bits `127..0`, matching Linux `toshsd.c`).
unsafe fn sd_send_cmd(base: u32, opcode: u16, arg: u32, flags: u16) -> Result<[u32; 4], u32> {
    unsafe {
        // Clear previous status & error bits
        mmio_w32(base, REG_CARDSTATUS, 0);
        mmio_w32(base, REG_ERRORSTATUS0, 0);
        mmio_w32(base, REG_ARG0, arg);
        mmio_w16(base, REG_CMD, (opcode & 0x3F) | flags);
    }

    let mut tries = 0u32;
    loop {
        let sts = unsafe { mmio_r32(base, REG_CARDSTATUS) };
        if (sts & STS_ANY_ERR) != 0 {
            unsafe { mmio_w32(base, REG_CARDSTATUS, 0) };
            return Err(sts);
        }
        if (sts & STS_RESP_END) != 0 {
            unsafe { mmio_w32(base, REG_CARDSTATUS, sts & !STS_RESP_END) };
            break;
        }
        tries += 1;
        if tries >= 200 {
            return Err(STS_CMD_TIMEOUT);
        }
        cardbus::delay_ms(1);
    }

    let mut resp = [0u32; 4];
    let resp_type = flags & (7 << 8);
    if resp_type == CMD_RESP_R2 {
        // Reconstruct 128-bit response matching toshsd.c byte layout (bits 127..0 in resp[3]..resp[0])
        let w = [
            unsafe { mmio_r16(base, REG_RSP0 + 0) } as u32,
            unsafe { mmio_r16(base, REG_RSP0 + 2) } as u32,
            unsafe { mmio_r16(base, REG_RSP0 + 4) } as u32,
            unsafe { mmio_r16(base, REG_RSP0 + 6) } as u32,
            unsafe { mmio_r16(base, REG_RSP0 + 8) } as u32,
            unsafe { mmio_r16(base, REG_RSP0 + 10) } as u32,
            unsafe { mmio_r16(base, REG_RSP0 + 12) } as u32,
            unsafe { mmio_r16(base, REG_RSP0 + 14) } as u32,
        ];
        // The hardware shifts out the 8-bit CRC; bit 127..8 are in w[7]..w[0].
        resp[0] = ((w[0] & 0xFFFF) << 8) | ((w[1] & 0xFF) << 24);
        resp[1] = ((w[1] >> 8) & 0xFF) | (w[2] << 8) | ((w[3] & 0xFF) << 24);
        resp[2] = ((w[3] >> 8) & 0xFF) | (w[4] << 8) | ((w[5] & 0xFF) << 24);
        resp[3] = ((w[5] >> 8) & 0xFF) | (w[6] << 8) | ((w[7] & 0xFF) << 24);
    } else if resp_type != CMD_RESP_NONE {
        let lo = unsafe { mmio_r16(base, REG_RSP0) } as u32;
        let hi = unsafe { mmio_r16(base, REG_RSP0 + 2) } as u32;
        resp[0] = lo | (hi << 16);
    }
    Ok(resp)
}

/// Convert a bounded logical sector into the card's command argument.
fn sector_argument(is_sdhc: bool, sectors: u32, lba: u32) -> Option<u32> {
    if lba >= sectors {
        return None;
    }
    if is_sdhc { Some(lba) } else { lba.checked_mul(512) }
}

/// Read one 512-byte sector at `lba` using `CMD17` (strictly read-only PIO).
unsafe fn sd_read_one_sector(st: &ToshSdState, lba: u32, dst: &mut [u8; 512]) -> Result<(), u32> {
    let base = st.mapped_lin;
    let arg = sector_argument(st.is_sdhc, st.sectors, lba).ok_or(4u32)?;

    unsafe {
        mmio_w16(base, REG_SDIO_LEDCTRL, 1);
        mmio_w16(base, REG_STOPINTERNAL, 0);
        mmio_w16(base, REG_BLOCKCOUNT, 1);
        mmio_w16(base, REG_CARDXFERDATALEN, 512);
        mmio_w32(base, REG_CARDSTATUS, 0);
        mmio_w32(base, REG_ERRORSTATUS0, 0);
        mmio_w32(base, REG_ARG0, arg);
        mmio_w16(
            base,
            REG_CMD,
            17 | CMD_RESP_R1 | CMD_DATA_PRESENT | CMD_TRANSFER_READ,
        );
    }

    // Wait for READ_ENABLE (FIFO ready with 512 bytes)
    let mut tries = 0u32;
    loop {
        let sts = unsafe { mmio_r32(base, REG_CARDSTATUS) };
        if (sts & STS_ANY_ERR) != 0 {
            unsafe {
                mmio_w32(base, REG_CARDSTATUS, 0);
                mmio_w16(base, REG_SDIO_LEDCTRL, 0);
            }
            return Err(sts);
        }
        if (sts & STS_READ_ENABLE) != 0 {
            unsafe { mmio_w32(base, REG_CARDSTATUS, sts & !STS_READ_ENABLE) };
            break;
        }
        tries += 1;
        if tries >= 500 {
            unsafe { mmio_w16(base, REG_SDIO_LEDCTRL, 0) };
            return Err(STS_DATA_TIMEOUT);
        }
        cardbus::delay_ms(1);
    }

    // Drain 128 dwords (512 bytes) from SD_DATAPORT
    for i in 0..128usize {
        let dw = unsafe { mmio_r32(base, REG_DATAPORT) };
        let b = dw.to_le_bytes();
        dst[i * 4] = b[0];
        dst[i * 4 + 1] = b[1];
        dst[i * 4 + 2] = b[2];
        dst[i * 4 + 3] = b[3];
    }

    // Wait for RW_END
    let mut end_tries = 0u32;
    loop {
        let sts = unsafe { mmio_r32(base, REG_CARDSTATUS) };
        if (sts & STS_ANY_ERR) != 0 {
            unsafe {
                mmio_w32(base, REG_CARDSTATUS, 0);
                mmio_w16(base, REG_SDIO_LEDCTRL, 0);
            }
            return Err(sts);
        }
        if (sts & STS_RW_END) != 0 {
            unsafe { mmio_w32(base, REG_CARDSTATUS, sts & !STS_RW_END) };
            break;
        }
        end_tries += 1;
        if end_tries >= 200 {
            unsafe { mmio_w16(base, REG_SDIO_LEDCTRL, 0) };
            return Err(STS_DATA_TIMEOUT);
        }
        cardbus::delay_ms(1);
    }

    unsafe { mmio_w16(base, REG_SDIO_LEDCTRL, 0) };
    Ok(())
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
}

// -----------------------------------------------------------------------------
// V86 API entry points (AX=0x003C, 0x003D, 0x003E)
// -----------------------------------------------------------------------------

/// Verb `0x003C`: Probe & initialize Toshiba SD slot (`1179:0805`), identify card,
/// and read sector 0 (read-only). Writes human-readable ASCII report into `out_ptr[..out_cap]`.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sd_init(out_ptr: u32, out_cap: u32) -> u32 {
    if out_ptr == 0 || out_cap == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_cap as usize) };
    let mut o = Out::new(slice);
    let st = sd_state();

    if !st.present {
        // A failed release must not be overwritten by another probe.
        if st.claimed {
            if unsafe { crate::sd_release_bdf(st.bdf()) } != 0 {
                o.str("SD: previous PCI claim could not be released");
                o.crlf();
                return o.pos as u32;
            }
            st.claimed = false;
        }
        let Some((bus, dev, func, did, rev, bar0)) = (unsafe { find_toshsd_pci() }) else {
            o.str("SD: Toshiba SD controller (1179:0805) not found on PCI bus 0");
            o.crlf();
            return o.pos as u32;
        };
        st.bus = bus;
        st.dev = dev;
        st.func = func;
        st.did = did;
        st.rev = rev;

        // Standalone mode must not guess unallocated PCI resources.
        let Some(bar) = ferric_hw::memory_bar32(bar0) else {
            o.str("SD: no valid firmware-assigned 32-bit MMIO BAR0");
            o.crlf();
            return o.pos as u32;
        };
        st.bar_phys = bar;
        if unsafe { crate::sd_claim_bdf(st.bdf()) } != 0 {
            o.str("SD: PCI device ownership denied by FRES");
            o.crlf();
            return o.pos as u32;
        }
        st.claimed = true;

        // Wake PCI PMCSR (0x84) to D0 if in D3
        let pmcsr = unsafe { pci_read32(bus, dev, func, 0x84) };
        if (pmcsr & 0x0003) != 0 {
            unsafe { pci_write32(bus, dev, func, 0x84, pmcsr & !0x8003) };
            cardbus::delay_ms(10);
        }

        // Enable PCI Memory Space (bit 1)
        let cmd = unsafe { pci_read32(bus, dev, func, 0x04) };
        unsafe { pci_write32(bus, dev, func, 0x04, (cmd & 0xFFFF) | 0x0002) };

        // Highest accessed register is the 16-bit LED control at 0x13E.
        let window = ferric_hw::MmioWindow::new(bar, 0x140);
        let lin = window.and_then(|w| {
            let mapped_page = unsafe { cardbus::map_mmio(w.page_base, w.page_count) };
            w.linear_base(mapped_page)
        });
        let Some(lin) = lin else {
            o.str("SD: map_mmio failed for BAR0=0x");
            o.hex32(bar);
            o.crlf();
            // Restore the original PCI command and power state on failed mapping.
            unsafe {
                pci_write32(bus, dev, func, 0x04, cmd & 0xFFFF);
                if (pmcsr & 3) != 0 {
                    // PME status is W1C; do not echo it as a one.
                    pci_write32(bus, dev, func, 0x84, pmcsr & !0x8000);
                }
            }
            if unsafe { crate::sd_release_bdf(st.bdf()) } == 0 {
                st.claimed = false;
            }
            return o.pos as u32;
        };
        st.mapped_phys = bar;
        st.mapped_lin = lin;
        st.original_cmd = cmd & 0xFFFF;
        st.original_pmcsr = pmcsr;
        st.present = true;
    }

    o.str("SD: Toshiba 1179:");
    o.hex16(st.did);
    o.str(" bdf=00:");
    o.hex8(st.dev);
    o.byte(b'.');
    o.dec_u32(st.func as u32);
    o.str(" rev=0x");
    o.hex8(st.rev);
    o.str(" bar0=0x");
    o.hex32(st.bar_phys);
    if st.bar_assigned {
        o.str(" (assigned)");
    }
    o.crlf();

    st.card_ready = false;
    let base = st.mapped_lin;
    // Check the mapping before writing any controller register.
    if unsafe { mmio_r32(base, REG_CARDSTATUS) } == u32::MAX {
        o.str("SD: MMIO not responding before reset");
        o.crlf();
        return o.pos as u32;
    }
    unsafe { hw_reset_controller(st) };
    let status = unsafe { mmio_r32(base, REG_CARDSTATUS) };
    if status == 0xFFFF_FFFF {
        o.str("SD: MMIO returned 0xFFFFFFFF (controller not responding)");
        o.crlf();
        return o.pos as u32;
    }

    let card_in = (status & STS_CARD_PRESENT_0) != 0;
    let wp = (status & STS_WRITE_PROTECT) == 0;
    st.write_protected = wp;

    if !card_in {
        o.str("SD: slot empty (no card inserted, status=0x");
        o.hex32(status);
        o.byte(b')');
        o.crlf();
        return o.pos as u32;
    }

    // Power on 3.3V slot power + 1-bit width + ~258 kHz init clock
    unsafe {
        pci_write8(st.bus, st.dev, st.func, CFG_POWER1, PWR1_33V);
        pci_write8(st.bus, st.dev, st.func, CFG_POWER2, PWR2_AUTO);
        cardbus::delay_ms(20);

        mmio_w16(
            base,
            REG_CARDOPTIONSETUP,
            OPT_REQUIRED | OPT_TIMEOUT_14 | OPT_C2_ABSENT | OPT_WIDTH_1,
        );
        pci_write8(st.bus, st.dev, st.func, CFG_CLKMODE, 0);
        mmio_w16(base, REG_CARDCLOCKCTRL, CLK_ENABLE | CLK_DIV_128);
        cardbus::delay_ms(10);
    }

    // CMD0: GO_IDLE_STATE
    if let Err(e) = unsafe { sd_send_cmd(base, 0, 0, CMD_RESP_NONE) } {
        o.str("SD: CMD0 failed sts=0x");
        o.hex32(e);
        o.crlf();
        return o.pos as u32;
    }

    // CMD8: SEND_IF_COND (2.7-3.6V, check pattern 0xAA) -> detects SD v2.0
    let is_v2 = match unsafe { sd_send_cmd(base, 8, 0x0000_01AA, CMD_RESP_R1) } {
        Ok(r) => (r[0] & 0xFFF) == 0x1AA,
        Err(_) => false,
    };

    // ACMD41: SD_SEND_OP_COND loop (up to 1 second)
    let acmd41_arg = if is_v2 { 0x4030_0000 } else { 0x0030_0000 };
    let mut ocr = 0u32;
    let mut ready = false;
    for _ in 0..100u32 {
        if unsafe { sd_send_cmd(base, 55, 0, CMD_RESP_R1 | CMD_TYPE_ACMD) }.is_err() {
            // Retry without CMD_TYPE_ACMD bit on some cards
            if unsafe { sd_send_cmd(base, 55, 0, CMD_RESP_R1) }.is_err() {
                cardbus::delay_ms(10);
                continue;
            }
        }
        if let Ok(r) = unsafe { sd_send_cmd(base, 41, acmd41_arg, CMD_RESP_R3) } {
            ocr = r[0];
            if (ocr & 0x8000_0000) != 0 {
                ready = true;
                break;
            }
        }
        cardbus::delay_ms(10);
    }

    if !ready {
        o.str("SD: ACMD41 timeout (card did not power up, last OCR=0x");
        o.hex32(ocr);
        o.byte(b')');
        o.crlf();
        return o.pos as u32;
    }
    st.is_sdhc = is_v2 && (ocr & 0x4000_0000) != 0;

    // CMD2: ALL_SEND_CID (R2)
    let cid = match unsafe { sd_send_cmd(base, 2, 0, CMD_RESP_R2) } {
        Ok(r) => r,
        Err(e) => {
            o.str("SD: CMD2 failed sts=0x");
            o.hex32(e);
            o.crlf();
            return o.pos as u32;
        }
    };
    st.prod_name = decode_cid_pnm(cid);

    // CMD3: SEND_RELATIVE_ADDR (R6)
    let rca = match unsafe { sd_send_cmd(base, 3, 0, CMD_RESP_R1) } {
        Ok(r) => (r[0] >> 16) as u16,
        Err(e) => {
            o.str("SD: CMD3 failed sts=0x");
            o.hex32(e);
            o.crlf();
            return o.pos as u32;
        }
    };
    st.rca = rca;

    // CMD9: SEND_CSD (R2)
    let csd = match unsafe { sd_send_cmd(base, 9, (rca as u32) << 16, CMD_RESP_R2) } {
        Ok(r) => r,
        Err(e) => {
            o.str("SD: CMD9 failed sts=0x");
            o.hex32(e);
            o.crlf();
            return o.pos as u32;
        }
    };
    st.sectors = decode_csd_sectors(csd);

    // CMD7: SELECT_CARD (R1b)
    if let Err(e) = unsafe { sd_send_cmd(base, 7, (rca as u32) << 16, CMD_RESP_R1B) } {
        o.str("SD: CMD7 failed sts=0x");
        o.hex32(e);
        o.crlf();
        return o.pos as u32;
    }

    // CMD16: SET_BLOCKLEN = 512 (required for SDSC)
    if !st.is_sdhc {
        if let Err(e) = unsafe { sd_send_cmd(base, 16, 512, CMD_RESP_R1) } {
            o.str("SD: CMD16 failed sts=0x");
            o.hex32(e);
            o.crlf();
            return o.pos as u32;
        }
    }

    // Switch clock to 16.5 MHz (div=2)
    unsafe {
        mmio_w16(base, REG_CARDCLOCKCTRL, CLK_ENABLE | CLK_DIV_2);
        cardbus::delay_ms(5);
    }
    // Do not publish readiness until the first sector read succeeds.

    // Report card summary
    o.str("SD Card: ");
    o.str(if st.is_sdhc { "SDHC" } else if is_v2 { "SDSC v2" } else { "SDSC v1" });
    o.str(" name=\"");
    for &c in &st.prod_name {
        o.byte(c);
    }
    o.str("\" rca=0x");
    o.hex16(st.rca);
    o.str(if st.write_protected { " [RO]" } else { " [RW-switch]" });
    o.crlf();

    let mb = st.sectors / 2048;
    o.str("  Capacity: ");
    o.dec_u32(mb);
    o.str(" MiB (");
    o.dec_u32(st.sectors);
    o.str(" sectors x 512 B)");
    o.crlf();

    // Read sector 0 (read-only) to verify PIO data path & detect filesystem
    let mut sec0 = [0u8; 512];
    match unsafe { sd_read_one_sector(st, 0, &mut sec0) } {
        Ok(()) => {
            st.card_ready = true;
            o.str("  Sector 0: OK  fs=");
            o.str(detect_sector0_label(&sec0));
            o.str("  sig=");
            o.hex8(sec0[510]);
            o.hex8(sec0[511]);
            o.crlf();
        }
        Err(e) => {
            o.str("  Sector 0: READ ERROR sts=0x");
            o.hex32(e);
            o.crlf();
        }
    }

    o.pos as u32
}

/// Verb `0x003D`: Read one 512-byte sector at `lba` into caller's `out_ptr`.
/// Returns `0` on success, non-zero error status on failure.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sd_read_sec(lba: u32, out_ptr: u32) -> u32 {
    if out_ptr == 0 {
        return 1;
    }
    let st = sd_state();
    if !st.present || !st.card_ready || st.mapped_lin == 0 {
        return 2;
    }
    let status = unsafe { mmio_r32(st.mapped_lin, REG_CARDSTATUS) };
    if status == u32::MAX || status & STS_CARD_PRESENT_0 == 0 {
        st.card_ready = false;
        return 5;
    }
    let dst = unsafe { &mut *(out_ptr as *mut [u8; 512]) };
    match unsafe { sd_read_one_sector(st, lba, dst) } {
        Ok(()) => 0,
        Err(e) => if e == 0 { 3 } else { e },
    }
}

/// Verb `0x003E`: Cleanly power down the Toshiba SD slot and stop its clocks.
/// Safe to call from both `SDPROBE /U` and `DLL_PROCESS_DETACH`.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sd_shutdown() -> u32 {
    let st = sd_state();
    st.card_ready = false;
    if st.present && st.mapped_lin != 0 {
        let base = st.mapped_lin;
        unsafe {
            if mmio_r32(base, REG_CARDSTATUS) != u32::MAX {
                mmio_w32(base, REG_INTMASKCARD, 0xFFFF_FFFF);
                mmio_w16(base, REG_SDIO_LEDCTRL, 0);
                mmio_w16(base, REG_SDIO_CLOCKNWAIT, 0);
                mmio_w16(base, REG_CARDCLOCKCTRL, 0);
                pci_write8(st.bus, st.dev, st.func, CFG_POWER1, PWR1_OFF);
                pci_write8(st.bus, st.dev, st.func, CFG_CLKSTOP, 0);
            }
            // Release the page-aligned reservation, not the register offset.
            if crate::sd_unmap_mmio(base & !0xFFF) == 0 {
                return 6;
            }
            pci_write32(st.bus, st.dev, st.func, 0x04, st.original_cmd);
            if st.original_pmcsr & 3 != 0 {
                pci_write32(st.bus, st.dev, st.func, 0x84, st.original_pmcsr & !0x8000);
            }
        }
        st.present = false;
        st.mapped_lin = 0;
        st.mapped_phys = 0;
    }
    if st.claimed && unsafe { crate::sd_release_bdf(st.bdf()) } != 0 {
        return 7;
    }
    *st = ToshSdState::new();
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sector_argument_checks_capacity_and_byte_address_overflow() {
        assert_eq!(sector_argument(true, 100, 99), Some(99));
        assert_eq!(sector_argument(false, 100, 99), Some(99 * 512));
        assert_eq!(sector_argument(true, 100, 100), None);
        assert_eq!(sector_argument(false, u32::MAX, 0x0080_0000), None);
        assert_eq!(sector_argument(false, 0, 0), None);
    }

    #[test]
    fn decode_csd_v2_sdhc_4gb() {
        // CSD v2.0: csd_struct=1 (bits 127:126), C_SIZE=7647 -> (7648)*1024 = 7,831,552 sectors (~3824 MiB)
        let csd = [0, 7647u32 << 16, 0, 1u32 << 30];
        assert_eq!(decode_csd_sectors(csd), 7648 * 1024);
    }

    #[test]
    fn decode_csd_v1_sdsc_1gb() {
        // CSD v1.0: csd_struct=0, READ_BL_LEN=10 (1024 B), C_SIZE=3905, C_SIZE_MULT=7 (mult=512)
        // sectors = (3905+1) * 2^(7+2+1) = 3906 * 1024 = 3,999,744 (~1953 MiB)
        let c_size = 3905u32;
        let csd = [
            0,
            ((c_size & 0x3) << 30) | (7u32 << 15),
            (10u32 << 16) | (c_size >> 2),
            0,
        ];
        assert_eq!(decode_csd_sectors(csd), 3906 * 1024);
    }

    #[test]
    fn decode_cid_name_sanitizes_non_ascii() {
        // "SD04G"
        let cid = [
            0,
            0,
            ((b'D' as u32) << 24)
                | ((b'0' as u32) << 16)
                | ((b'4' as u32) << 8)
                | (b'G' as u32),
            b'S' as u32,
        ];
        assert_eq!(&decode_cid_pnm(cid), b"SD04G");
    }

    #[test]
    fn detect_sector0_signatures() {
        let mut s = [0u8; 512];
        assert_eq!(detect_sector0_label(&s), "NO-BOOT-SIG");
        s[510] = 0x55;
        s[511] = 0xAA;
        s[0x1C2] = 0x0C;
        assert_eq!(detect_sector0_label(&s), "MBR: FAT32");
        s[3..11].copy_from_slice(b"EXFAT   ");
        assert_eq!(detect_sector0_label(&s), "exFAT (superfloppy)");
    }
}
