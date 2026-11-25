//! Stage H Steps 2-3 — EHCI (USB 2.0) recon and bring-up seam in
//! `FERRIC.JLM` (design: `docs/stage-h/DESIGN.md`).
//!
//! Two diagnostic verbs, and nothing else: this module runs only when
//! `EHCPROBE.COM` calls it. Nothing here is hooked into JLM load, the
//! timer hook, or the OHCI poll path, so a machine that never calls
//! the verbs behaves exactly as it did before Stage H.
//!
//! - Verb `0x0036` (`gate1_ehci_recon`): find the EHCI function by
//!   class code, give it an MMIO window if the BIOS left it disabled
//!   (the DynaBook's `1033:00E0` at `00:0C.2` ships with BAR0 = 0),
//!   map it, and report the capability registers and the legacy
//!   support (BIOS handoff) state. No port is touched: CONFIGFLAG
//!   keeps whatever value it had, so a running OHCI setup is
//!   undisturbed.
//! - Verb `0x0037` (`gate1_ehci_bringup`): BIOS handoff, controller
//!   reset, async schedule on a skeleton queue head in the caller's
//!   DMA block, CONFIGFLAG claimed, then one reset pulse per
//!   connected port with the routing decision from
//!   `ferric_ehci::routing` applied. Every wait is bounded and
//!   reported: a wedged controller must never wedge the DOS box.
//!
//! All protocol logic that can be pure lives in the `ferric-ehci`
//! crate (capability decoding, queue layouts, routing policy); this
//! file is only the hardware-touching seam, mirroring `ohci.rs`.

use core::cell::UnsafeCell;

use ferric_ehci::caps::{HccParams, HcsParams};
use ferric_ehci::queue::QueueHead;
use ferric_ehci::regs::{self, EhciRegs};
use ferric_ehci::routing::{DeviceSpeed, PortOwner};

use crate::{cardbus, ohci, pci_read32, pci_write32, vdmad_lock_dma_region_pci};

// The caller's DMA block is the same 16 KiB shape USBPROBE hands the
// OHCI driver. The EHCI structures occupy its first 1 KiB: a skeleton
// queue head, one control queue head for the Step 4 transport work,
// and a small qTD pool. They live in DMA memory, never in CPU-side
// state, because the controller reads and writes them on its own
// schedule once the async schedule is running.
const EHCI_DMA_SIZE: u32 = 16384;
const SKEL_QH_REL: usize = 0x000;
const CTRL_QH_REL: usize = 0x100;
const QTD_POOL_REL: usize = 0x200;
const QTD_POOL_COUNT: usize = 8;
const SETUP_BUF_REL: usize = 0x400;
const DATA_BUF_REL: usize = 0x500;
const DATA_BUF_MAX: usize = 4096;
const EHCI_STRUCT_SPAN: usize = QTD_POOL_REL + QTD_POOL_COUNT * 32;

// BAR assignment search range. The floor sits in the DynaBook's
// bus-0 MMIO neighbourhood right above OHCI #1 (0xF7EBF000) and
// CardBus (0xF7C00000..0xF7E01FFF), and below the 82550 Ethernet
// (0xF7EFF000) and the AGP bridge window (0xF8000000..0xFDFFFFFF).
// The ceiling stays below 0xFEC00000 (I/O APIC / local APIC).
const BAR_SEARCH_START: u32 = 0xF7EC_0000;
const BAR_SEARCH_END: u32 = 0xFEC0_0000;

/// CPU-side EHCI bookkeeping. Deliberately all scalars: this struct
/// lives in a static cell and is only ever touched by reference, and
/// the Stage E lesson stands — nothing multi-KiB may ride in a struct
/// that any code path copies by value. The DMA structures themselves
/// are in the caller's DMA block (see the offsets above), not here.
struct EhciState {
    present: bool,
    bus: u8,
    dev: u8,
    func: u8,
    vid: u16,
    did: u16,
    bar_phys: u32,
    bar_assigned: bool,
    mapped_phys: u32,
    mapped_lin: u32,
    op_base: u32,
    cap_len: u8,
    hci_version: u16,
    hcsparams: u32,
    hccparams: u32,
    legsup_off: u8,
    brought_up: bool,
    comp1_running: bool,
    comp1_bar_phys: u32,
    dma_lin: u32,
    dma_phys: u32,
}

impl EhciState {
    const fn new() -> Self {
        Self {
            present: false,
            bus: 0,
            dev: 0,
            func: 0,
            vid: 0,
            did: 0,
            bar_phys: 0,
            bar_assigned: false,
            mapped_phys: 0,
            mapped_lin: 0,
            op_base: 0,
            cap_len: 0,
            hci_version: 0,
            hcsparams: 0,
            hccparams: 0,
            legsup_off: 0,
            brought_up: false,
            comp1_running: false,
            comp1_bar_phys: 0,
            dma_lin: 0,
            dma_phys: 0,
        }
    }
}

struct SingleCoreCellEhci(UnsafeCell<EhciState>);
// SAFETY: single-core DOS with CLI/STI discipline.
unsafe impl Sync for SingleCoreCellEhci {}

static EHCI_STATE: SingleCoreCellEhci = SingleCoreCellEhci(UnsafeCell::new(EhciState::new()));

fn ehci_state() -> &'static mut EhciState {
    unsafe { &mut *EHCI_STATE.0.get() }
}

unsafe fn mmio_r32(base: u32, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base as usize + off) as *const u32) }
}

unsafe fn mmio_w32(base: u32, off: usize, val: u32) {
    unsafe {
        core::ptr::write_volatile((base as usize + off) as *mut u32, val);
        let _ = core::ptr::read_volatile((base as usize + off) as *const u32);
    }
}

/// The portable core's register trait over the mapped operational
/// register block, so the seam and any future schedule code share one
/// definition of where the operational registers live.
struct EhciMmio {
    op_base: u32,
}

impl EhciRegs for EhciMmio {
    fn op_read(&mut self, offset: u32) -> u32 {
        unsafe { mmio_r32(self.op_base, offset as usize) }
    }
    fn op_write(&mut self, offset: u32, value: u32) {
        unsafe { mmio_w32(self.op_base, offset as usize, value) }
    }
}

// ---------------------------------------------------------------------
// Report writer. ohci.rs's AsciiOut is module-private and decimal
// only; the EHCI reports are hex-heavy (PCI IDs, raw capability
// dwords), so this module carries its own small writer in the same
// style: bounded, never writes past the caller's capacity.
// ---------------------------------------------------------------------

struct EhciOut<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> EhciOut<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn push_byte(&mut self, b: u8) {
        if self.pos < self.buf.len() {
            self.buf[self.pos] = b;
            self.pos += 1;
        }
    }
    fn push_bytes(&mut self, s: &[u8]) {
        for &b in s {
            self.push_byte(b);
        }
    }
    fn push_dec(&mut self, mut v: u32) {
        let mut tmp = [0u8; 10];
        let mut n = 0usize;
        if v == 0 {
            self.push_byte(b'0');
            return;
        }
        while v > 0 && n < 10 {
            tmp[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            self.push_byte(tmp[n]);
        }
    }
    fn push_hex(&mut self, v: u32, digits: u32) {
        let mut shift = digits * 4;
        while shift > 0 {
            shift -= 4;
            let nib = (v >> shift) & 0xF;
            self.push_byte(if nib < 10 {
                b'0' + nib as u8
            } else {
                b'A' + (nib - 10) as u8
            });
        }
    }
    fn push_hex32(&mut self, v: u32) {
        self.push_bytes(b"0x");
        self.push_hex(v, 8);
    }
    fn push_hex16(&mut self, v: u32) {
        self.push_bytes(b"0x");
        self.push_hex(v, 4);
    }
    fn crlf(&mut self) {
        self.push_bytes(b"\r\n");
    }
    /// The `EHCI: <vid>:<did> at <bus>:<dev>.<func>` header line both
    /// verbs open with, in the census notation (hex bus/device).
    fn push_header(&mut self, st: &EhciState) {
        self.push_bytes(b"EHCI: ");
        self.push_hex(st.vid as u32, 4);
        self.push_byte(b':');
        self.push_hex(st.did as u32, 4);
        self.push_bytes(b" at ");
        self.push_hex(st.bus as u32, 2);
        self.push_byte(b':');
        self.push_hex(st.dev as u32, 2);
        self.push_byte(b'.');
        self.push_dec(st.func as u32);
        self.crlf();
    }
}

// ---------------------------------------------------------------------
// PCI discovery and BAR assignment.
// ---------------------------------------------------------------------

/// Find the first EHCI host controller on PCI bus 0 (class 0x0C,
/// subclass 0x03, prog-if 0x20), mirroring `find_ohci_pci`'s walk.
/// Unlike the OHCI finder this accepts a zero BAR0: a BIOS-disabled
/// EHCI is the normal case on the DynaBook, not a reason to skip it.
/// Returns (bus, dev, func, vid, did, bar0_base).
unsafe fn find_ehci_pci() -> Option<(u8, u8, u8, u16, u16, u32)> {
    for dev in 0..32u8 {
        let id0 = unsafe { pci_read32(0, dev, 0, 0) };
        if (id0 & 0xFFFF) == 0xFFFF {
            continue;
        }
        let hdr = (unsafe { pci_read32(0, dev, 0, 0x0C) } >> 16) & 0xFF;
        let max_fn = if (hdr & 0x80) != 0 { 8u8 } else { 1u8 };
        for func in 0..max_fn {
            let id = unsafe { pci_read32(0, dev, func, 0) };
            let vid = (id & 0xFFFF) as u16;
            let did = (id >> 16) as u16;
            if vid == 0xFFFF || vid == 0x0000 {
                continue;
            }
            let cc = unsafe { pci_read32(0, dev, func, 0x08) };
            let class = ((cc >> 24) & 0xFF) as u8;
            let subclass = ((cc >> 16) & 0xFF) as u8;
            let progif = ((cc >> 8) & 0xFF) as u8;
            if class == 0x0C && subclass == 0x03 && progif == 0x20 {
                let bar0 = unsafe { pci_read32(0, dev, func, 0x10) } & 0xFFFF_F000;
                return Some((0, dev, func, vid, did, bar0));
            }
        }
    }
    None
}

/// Size BAR0 of the EHCI function itself (all-ones probe). Only ever
/// called while the function is still disabled and unassigned, so
/// the momentary all-ones decode window cannot disturb a running
/// device. Memory decode is held off across the probe for the same
/// reason.
unsafe fn ehci_bar_size(bus: u8, dev: u8, func: u8) -> u32 {
    unsafe {
        let cmd = pci_read32(bus, dev, func, 0x04);
        pci_write32(bus, dev, func, 0x04, cmd & !0x0002);
        let orig = pci_read32(bus, dev, func, 0x10);
        pci_write32(bus, dev, func, 0x10, 0xFFFF_FFFF);
        let mask = pci_read32(bus, dev, func, 0x10);
        pci_write32(bus, dev, func, 0x10, orig);
        pci_write32(bus, dev, func, 0x04, cmd);
        if mask == 0 {
            return 0;
        }
        // The mask's low nibble is flags, not size; the size is the
        // inverse of the writable address bits, plus one.
        (!(mask & 0xFFFF_FFF0)).wrapping_add(1)
    }
}

/// Conservative span a populated BAR is treated as occupying. A
/// BAR's size is a power of two that divides its base, so the size
/// can never exceed the base's lowest set bit; rounding that up to at
/// least one page over-reserves for small BARs, which is the safe
/// direction when the alternative is sizing every live device on
/// the bus.
fn bar_span(base: u32) -> u32 {
    let lowbit = base & base.wrapping_neg();
    if lowbit < 0x1000 { 0x1000 } else { lowbit }
}

fn ranges_overlap(a_base: u32, a_size: u32, b_base: u32, b_size: u32) -> bool {
    (a_base as u64) < (b_base as u64) + (b_size as u64)
        && (b_base as u64) < (a_base as u64) + (a_size as u64)
}

/// Is this PCI class register (offset 0x08) a host bridge — base
/// class 0x06, subclass 0x00?
///
/// Host bridges implement no BARs: the dwords at offsets 0x10 and
/// up in their config space are chipset registers, not decode
/// ranges. The DynaBook's ALi M1644 host bridge reads 0xF0000008
/// at offset 0x10, and treating that value as a BAR base is a real
/// trap: bar_span inflates it into a 256 MiB span starting at
/// 0xF0000000, which swallows the entire BAR assignment search
/// window and leaves the EHCI function nowhere to go. The value
/// cannot be a genuine decode range — the BIOS placed the
/// Ethernet BAR at 0xF7EFF000, OHCI #1 at 0xF7EBF000, and the VGA
/// BAR at 0xFC000000, all inside that span, and those devices all
/// work. So a host bridge's "BARs" are never collected.
fn is_host_bridge(class_reg: u32) -> bool {
    (class_reg >> 24) & 0xFF == 0x06 && (class_reg >> 16) & 0xFF == 0x00
}

/// Would one populated memory BAR block `[candidate, candidate +
/// size)`? Pure, so host tests can drive it with a recorded census:
/// a host bridge's chipset registers never block (see
/// is_host_bridge), USB host controllers (class 0C/03) always use a
/// 4 KiB register page (so an EHCI BAR at 0xFE000000 does not inflate
/// to 32 MiB and block companion OHCI #2 at 0xFE001000), and any
/// other BAR blocks when its conservative span overlaps the window.
fn bar_blocks_window(class_reg: u32, base: u32, candidate: u32, size: u32) -> bool {
    if is_host_bridge(class_reg) {
        return false;
    }
    let is_usb_hc = ((class_reg >> 24) & 0xFF) == 0x0C && ((class_reg >> 16) & 0xFF) == 0x03;
    let is_tosh_sd = ((class_reg >> 24) & 0xFF) == 0x08 && ((class_reg >> 16) & 0xFF) == 0x80;
    let span = if is_usb_hc || is_tosh_sd { 0x1000 } else { bar_span(base) };
    ranges_overlap(candidate, size, base, span)
}

/// Does any entry of a collected bus census — (class register, BAR
/// base) pairs — block the candidate window? This is the decision
/// bar_window_taken reaches once its live scan has collected the
/// census, factored out so the decision itself is host-testable
/// without config-space hardware behind it.
fn window_taken_by_census(candidate: u32, size: u32, census: &[(u32, u32)]) -> bool {
    census
        .iter()
        .any(|&(class_reg, base)| bar_blocks_window(class_reg, base, candidate, size))
}

/// Does a Type-1 PCI-to-PCI bridge Memory Base/Limit dword (at config
/// offset 0x20 or 0x24) forward `[candidate, candidate + size)`?
/// Both fields are 1 MiB-granularity: base = (lo & 0xFFF0) << 16,
/// limit = ((hi & 0xFFF0) << 16) | 0x000F_FFFF; disabled when base > limit.
fn pci_bridge_window_blocks(reg: u32, candidate: u32, size: u32) -> bool {
    let base = (reg & 0xFFF0) << 16;
    let limit = (reg & 0xFFF0_0000) | 0x000F_FFFF;
    if base > limit {
        return false;
    }
    let cand_end = (candidate as u64) + (size as u64);
    (candidate as u64) <= (limit as u64) && (base as u64) < cand_end
}

/// Does any populated memory BAR or bridge forwarding window on bus 0
/// (other than the EHCI function's own BAR0) overlap
/// `[candidate, candidate + size)`?
/// The scan collects BARs into a census and checks Type-1 PCI-to-PCI
/// bridge (e.g. AGP 00:01.0) and Type-2 CardBus bridge memory windows.
unsafe fn bar_window_taken(candidate: u32, size: u32, skip: (u8, u8, u8)) -> bool {
    // Far larger than any real bus-0 census. If one ever overflowed
    // the array, the extra BARs are decided inline instead of
    // collected, so the answer stays exact rather than truncated.
    let mut census = [(0u32, 0u32); 64];
    let mut count = 0usize;
    for dev in 0..32u8 {
        let id0 = unsafe { pci_read32(0, dev, 0, 0) };
        if (id0 & 0xFFFF) == 0xFFFF {
            continue;
        }
        let hdr = (unsafe { pci_read32(0, dev, 0, 0x0C) } >> 16) & 0xFF;
        let max_fn = if (hdr & 0x80) != 0 { 8u8 } else { 1u8 };
        for func in 0..max_fn {
            let id = unsafe { pci_read32(0, dev, func, 0) };
            if (id & 0xFFFF) == 0xFFFF || (id & 0xFFFF) == 0x0000 {
                continue;
            }
            // BAR count by this function's own header layout: type 0
            // has six, a PCI-PCI bridge two, a CardBus bridge one.
            let fn_hdr = (unsafe { pci_read32(0, dev, func, 0x0C) } >> 16) & 0xFF;
            let hdr_type = fn_hdr & 0x7F;
            let bar_count = match hdr_type {
                0x00 => 6,
                0x01 => 2,
                _ => 1,
            };
            // Type-1 PCI-to-PCI bridges (like the ALi M5247 AGP bridge at
            // 00:01.0) claim their Memory and Prefetchable Memory windows
            // on bus 0; placing a bus-0 BAR inside them causes master abort
            // (all-ones MMIO reads).
            if hdr_type == 0x01 {
                let mem_win = unsafe { pci_read32(0, dev, func, 0x20) };
                let pref_win = unsafe { pci_read32(0, dev, func, 0x24) };
                if pci_bridge_window_blocks(mem_win, candidate, size)
                    || pci_bridge_window_blocks(pref_win, candidate, size)
                {
                    return true;
                }
            } else if hdr_type == 0x02 {
                // Type-2 CardBus bridges (ToPIC95 at 00:11.0/.1): check
                // Mem0 (0x1C..0x20) and Mem1 (0x24..0x28) windows.
                for (b_off, l_off) in [(0x1Cu8, 0x20u8), (0x24u8, 0x28u8)] {
                    let base = unsafe { pci_read32(0, dev, func, b_off) } & 0xFFFF_F000;
                    let lim = (unsafe { pci_read32(0, dev, func, l_off) } & 0xFFFF_F000) | 0xFFF;
                    if base != 0 && base <= lim {
                        let cand_end = (candidate as u64) + (size as u64);
                        if (candidate as u64) <= (lim as u64) && (base as u64) < cand_end {
                            return true;
                        }
                    }
                }
            }
            let class_reg = unsafe { pci_read32(0, dev, func, 0x08) };
            let mut bar_idx = 0u8;
            while bar_idx < bar_count {
                let off = 0x10 + bar_idx * 4;
                let val = unsafe { pci_read32(0, dev, func, off) };
                bar_idx += 1;
                if (val & 0x1) != 0 {
                    continue; // I/O BAR: not in MMIO space.
                }
                if ((val >> 1) & 0x3) == 0x2 {
                    bar_idx += 1; // 64-bit BAR: skip its high dword.
                }
                let base = val & 0xFFFF_FFF0;
                if base == 0 {
                    continue;
                }
                if (dev, func, off) == (skip.0, skip.1, skip.2) {
                    continue;
                }
                if count < census.len() {
                    census[count] = (class_reg, base);
                    count += 1;
                } else if bar_blocks_window(class_reg, base, candidate, size) {
                    return true;
                }
            }
        }
    }
    window_taken_by_census(candidate, size, &census[..count])
}

/// Pick the first window of `size` bytes at or above BAR_SEARCH_START
/// that no populated BAR overlaps, aligned to at least 4 KiB (and to
/// the BAR's own size when that is larger), and program it into the
/// EHCI function's BAR0.
pub(crate) unsafe fn assign_mmio_bar(bus: u8, dev: u8, func: u8, size: u32) -> Option<u32> {
    unsafe { assign_ehci_bar(bus, dev, func, size) }
}

unsafe fn assign_ehci_bar(bus: u8, dev: u8, func: u8, size: u32) -> Option<u32> {
    let align = if size > 0x1000 { size } else { 0x1000 };
    let mut candidate = (BAR_SEARCH_START + align - 1) & !(align - 1);
    while candidate + size <= BAR_SEARCH_END {
        if !unsafe { bar_window_taken(candidate, size, (dev, func, 0x10)) } {
            unsafe { pci_write32(bus, dev, func, 0x10, candidate) };
            let readback = unsafe { pci_read32(bus, dev, func, 0x10) } & 0xFFFF_F000;
            if readback == candidate {
                return Some(candidate);
            }
            return None;
        }
        candidate += align;
    }
    None
}

/// Walk the EHCI extended capability list in PCI config space from
/// `eecp` looking for the USB Legacy Support capability (ID 1).
/// Returns its config offset. The walk is hop-bounded and refuses
/// offsets below 0x40, so a garbage EECP cannot send it wandering
/// through the standard header.
unsafe fn find_legacy_support(bus: u8, dev: u8, func: u8, eecp: u8) -> Option<u8> {
    let mut off = eecp;
    for _ in 0..8 {
        if off < 0x40 {
            return None;
        }
        let dword = unsafe { pci_read32(bus, dev, func, off) };
        let id = (dword & 0xFF) as u8;
        let next = ((dword >> 8) & 0xFF) as u8;
        if id == regs::legacysup::CAP_ID {
            return Some(off);
        }
        if next == 0 {
            return None;
        }
        off = next;
    }
    None
}

// ---------------------------------------------------------------------
// Shared preparation: find, assign, enable, map, read capabilities.
// Both verbs start here; it is idempotent, so a second call on a
// prepared controller just reuses the recorded state.
// ---------------------------------------------------------------------

unsafe fn ehci_prepare() -> bool {
    let st = ehci_state();
    if st.present && st.mapped_lin != 0 {
        return true;
    }
    let Some((bus, dev, func, vid, did, bar0)) = (unsafe { find_ehci_pci() }) else {
        return false;
    };
    st.bus = bus;
    st.dev = dev;
    st.func = func;
    st.vid = vid;
    st.did = did;
    st.present = true;

    // Give the function an MMIO window if the BIOS left it disabled.
    // When the BIOS did assign one, its size is not probed: sizing
    // writes all-ones into the BAR, which is only safe while nothing
    // can be using the window, and one page covers every real EHCI
    // register block anyway.
    let mut bar = bar0;
    let mut map_pages = 1u32;
    if bar == 0 {
        let size = unsafe { ehci_bar_size(bus, dev, func) };
        if size == 0 {
            return false;
        }
        match unsafe { assign_ehci_bar(bus, dev, func, size) } {
            Some(base) => {
                bar = base;
                st.bar_assigned = true;
                map_pages = size.div_ceil(0x1000).max(1);
            }
            None => return false,
        }
    }
    st.bar_phys = bar;

    // Wake the function from PCI D3hot/D2/D1 into D0 if it implements
    // PCI Power Management (cap ID 0x01); BIOS-disabled EHCI functions
    // are often left in D3hot where MMIO reads return 0xFFFFFFFF.
    unsafe {
        let cmd_stat = pci_read32(bus, dev, func, 0x04);
        if ((cmd_stat >> 16) & 0x0010) != 0 {
            let mut cap_ptr = (pci_read32(bus, dev, func, 0x34) & 0xFC) as u8;
            for _ in 0..16 {
                if cap_ptr < 0x40 {
                    break;
                }
                let cap_hdr = pci_read32(bus, dev, func, cap_ptr);
                if (cap_hdr & 0xFF) == 0x01 {
                    let pmcsr = pci_read32(bus, dev, func, cap_ptr + 4);
                    if (pmcsr & 0x0003) != 0 {
                        pci_write32(bus, dev, func, cap_ptr + 4, pmcsr & !0x0003);
                        cardbus::delay_ms(10);
                    }
                    break;
                }
                cap_ptr = ((cap_hdr >> 8) & 0xFC) as u8;
            }
        }
    }

    // Enable PCI Memory Space + Bus Master (same idiom as the OHCI
    // init: preserve the low command word, force MEM + BM, clear the
    // interrupt-disable bit).
    unsafe {
        let cmd = pci_read32(bus, dev, func, 0x04);
        pci_write32(bus, dev, func, 0x04, ((cmd & 0xFFFF) | 0x0006) & !0x0400);
    }

    // Map the register block (cached across calls, like the OHCI
    // driver's mapping).
    let lin = if st.mapped_phys == bar && st.mapped_lin != 0 {
        st.mapped_lin
    } else {
        let lin = unsafe { cardbus::map_mmio(bar, map_pages) };
        if lin != 0 {
            st.mapped_phys = bar;
            st.mapped_lin = lin;
        }
        lin
    };
    if lin == 0 {
        return false;
    }

    // Capability block: dword 0 packs CAPLENGTH (byte 0) and
    // HCIVERSION (high half); the parameter registers follow.
    let d0 = unsafe { mmio_r32(lin, regs::cap::CAPLENGTH as usize) };
    let cap_len = (d0 & 0xFF) as u8;
    if d0 == 0xFFFF_FFFF || cap_len < 0x10 || cap_len == 0xFF {
        st.mapped_lin = 0;
        return false;
    }
    st.cap_len = cap_len;
    st.hci_version = ((d0 >> 16) & 0xFFFF) as u16;
    st.hcsparams = unsafe { mmio_r32(lin, regs::cap::HCSPARAMS as usize) };
    st.hccparams = unsafe { mmio_r32(lin, regs::cap::HCCPARAMS as usize) };
    st.op_base = lin + st.cap_len as u32;

    let eecp = HccParams::from_raw(st.hccparams).eecp();
    st.legsup_off = if eecp >= 0x40 {
        unsafe { find_legacy_support(bus, dev, func, eecp) }.unwrap_or(0)
    } else {
        0
    };
    true
}

// ---------------------------------------------------------------------
// Verb 0x0036 — recon report.
// ---------------------------------------------------------------------

/// FFI entry for JLM V86 API `AX=0x0036` (`EHCPROBE.COM`): find the
/// EHCI function, assign + map its BAR if needed, and format the
/// capability recon report into `dst_lin[..cap]`. Returns the report
/// length. Read-only with respect to ports and schedules.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_ehci_recon(dst_lin: u32, cap: u32) -> u32 {
    if dst_lin == 0 || cap == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(dst_lin as *mut u8, cap as usize) };
    let mut w = EhciOut::new(slice);
    if !unsafe { ehci_prepare() } {
        let st = ehci_state();
        if st.present {
            w.push_header(st);
            w.push_bytes(b"EHCI: setup failed (BAR/MMIO)\r\n");
        } else {
            w.push_bytes(b"EHCI: none found\r\n");
        }
        return w.pos as u32;
    }
    let st = ehci_state();
    w.push_header(st);
    w.push_bytes(b"EHCI: bar0=");
    w.push_hex32(st.bar_phys);
    w.push_bytes(if st.bar_assigned {
        b" assigned\r\n"
    } else {
        b" bios\r\n"
    });
    w.push_bytes(b"EHCI: hciversion=");
    w.push_hex16(st.hci_version as u32);
    w.push_bytes(b" caplen=");
    w.push_dec(st.cap_len as u32);
    w.crlf();

    let hs = HcsParams::from_raw(st.hcsparams);
    w.push_bytes(b"EHCI: hcsparams=");
    w.push_hex32(st.hcsparams);
    w.push_bytes(b" ports=");
    w.push_dec(hs.n_ports() as u32);
    w.push_bytes(b" companions=");
    w.push_dec(hs.n_companions() as u32);
    w.push_bytes(b" per-companion=");
    w.push_dec(hs.n_ports_per_companion() as u32);
    w.push_bytes(b" ppc=");
    w.push_dec(hs.port_power_control() as u32);
    w.push_bytes(b" routing=");
    w.push_dec(hs.routing_rules() as u32);
    w.crlf();

    let hc = HccParams::from_raw(st.hccparams);
    w.push_bytes(b"EHCI: hccparams=");
    w.push_hex32(st.hccparams);
    w.push_bytes(b" eecp=0x");
    w.push_hex(hc.eecp() as u32, 2);
    w.push_bytes(b" addr64=");
    w.push_dec(hc.addr64() as u32);
    w.push_bytes(b" park=");
    w.push_dec(hc.async_park() as u32);
    w.push_bytes(b" pfl=");
    w.push_dec(hc.programmable_frame_list() as u32);
    w.crlf();

    if st.legsup_off != 0 {
        let dword = unsafe { pci_read32(st.bus, st.dev, st.func, st.legsup_off) };
        w.push_bytes(b"EHCI: legsup=0x");
        w.push_hex(st.legsup_off as u32, 2);
        w.push_bytes(b" bios=");
        w.push_dec((dword & regs::legacysup::BIOS_SEMAPHORE != 0) as u32);
        w.push_bytes(b" os=");
        w.push_dec((dword & regs::legacysup::OS_SEMAPHORE != 0) as u32);
        w.crlf();
    } else {
        w.push_bytes(b"EHCI: legsup=none\r\n");
    }
    w.pos as u32
}

// ---------------------------------------------------------------------
// Verb 0x0037 — bring-up + status report.
// ---------------------------------------------------------------------

/// BIOS handoff through the USBLEGSUP semaphores. The BIOS may be
/// emulating a PS/2 keyboard through this controller; resetting it
/// without asking first can hang the machine in SMM, so the OS
/// semaphore is set and the BIOS semaphore is polled — bounded, and
/// the outcome is reported either way. Proceeding after a timeout
/// matches the behavioural references (some BIOSes never answer) and
/// is safe here because the next step stops the controller before
/// resetting it.
unsafe fn ehci_bios_handoff(w: &mut EhciOut) {
    let st = ehci_state();
    if st.legsup_off == 0 {
        w.push_bytes(b"EHCI: handoff n/a (no legacy support)\r\n");
        return;
    }
    let (bus, dev, func, off) = (st.bus, st.dev, st.func, st.legsup_off);
    let dword = unsafe { pci_read32(bus, dev, func, off) };
    if dword & regs::legacysup::BIOS_SEMAPHORE == 0 {
        w.push_bytes(b"EHCI: handoff not needed (BIOS semaphore clear)\r\n");
        return;
    }
    unsafe {
        pci_write32(
            bus,
            dev,
            func,
            off,
            dword | regs::legacysup::OS_SEMAPHORE,
        )
    };
    for _ in 0..100 {
        let now = unsafe { pci_read32(bus, dev, func, off) };
        if now & regs::legacysup::BIOS_SEMAPHORE == 0 {
            w.push_bytes(b"EHCI: handoff ok\r\n");
            return;
        }
        cardbus::delay_ms(10);
    }
    w.push_bytes(b"EHCI: handoff timeout (proceeding)\r\n");
}

/// Lay out the EHCI DMA structures in the caller's block on first
/// bring-up: skeleton async QH, one idle control QH (Step 4 links
/// it), and an idle qTD pool. Returns false when there is no usable
/// DMA block.
unsafe fn ehci_setup_dma(w: &mut EhciOut, dma_seg: u32) -> bool {
    let st = ehci_state();
    if st.dma_phys != 0 {
        return true;
    }
    if dma_seg == 0 {
        w.push_bytes(b"EHCI: no DMA block (client passed segment 0)\r\n");
        return false;
    }
    let raw_lin = dma_seg << 4;
    let Some(raw_phys) = (unsafe { vdmad_lock_dma_region_pci(raw_lin, EHCI_DMA_SIZE) }) else {
        w.push_bytes(b"EHCI: DMA lock failed\r\n");
        return false;
    };
    // Align the structures area to 256 bytes inside the block, the
    // same discipline the OHCI driver uses for its HCCA: queue heads
    // need 32-byte alignment and the margin keeps the whole area
    // clear of the block's ragged start.
    let off = ((256 - (raw_phys & 0xFF)) & 0xFF) as usize;
    let dma_lin = raw_lin + off as u32;
    let dma_phys = raw_phys + off as u32;
    unsafe {
        core::ptr::write_bytes(dma_lin as *mut u8, 0, EHCI_STRUCT_SPAN);
        // Skeleton async QH: a reclamation head pointing at itself,
        // so the controller idles on it until Step 4 splices real
        // queue heads behind it.
        let skel = QueueHead::skeleton_async(dma_phys + SKEL_QH_REL as u32);
        core::ptr::write((dma_lin as usize + SKEL_QH_REL) as *mut QueueHead, skel);
        // Control QH, initialised idle and unlinked for now.
        let mut ctrl = QueueHead::skeleton_async(dma_phys + CTRL_QH_REL as u32);
        ctrl.horiz_link = ferric_ehci::queue::LINK_TERMINATED;
        ctrl.characteristics = ferric_ehci::queue::qh_characteristics(
            0, // device address 0 until Step 4 addresses a real device
            0, // endpoint 0
            ferric_ehci::queue::Speed::High,
            64,    // max packet for a high-speed control endpoint
            false, // not a reclamation head
            true,  // data toggle comes from the qTD
            false, // EHCI r1.1 Table 3-19: C bit is 0 for high-speed control endpoints
            0,     // NAK reload: no limit
        );
        core::ptr::write((dma_lin as usize + CTRL_QH_REL) as *mut QueueHead, ctrl);
        // qTD pool, all idle (token zeroed: ACTIVE clear).
        for i in 0..QTD_POOL_COUNT {
            core::ptr::write(
                (dma_lin as usize + QTD_POOL_REL + i * 32) as *mut ferric_ehci::queue::Qtd,
                ferric_ehci::queue::Qtd::IDLE,
            );
        }
    }
    let st = ehci_state();
    st.dma_lin = dma_lin;
    st.dma_phys = dma_phys;
    true
}

/// Reset the controller and start the async schedule on the skeleton
/// QH. Every wait is bounded; a false return means the controller did
/// not reach the running state and `brought_up` stays clear, so a
/// later call retries from the top.
unsafe fn ehci_reset_and_run(w: &mut EhciOut) -> bool {
    let st = ehci_state();
    let mut regs = EhciMmio { op_base: st.op_base };

    // Stop first if a previous run (or the BIOS) left it running:
    // HCRESET is only meaningful from the halted state.
    if regs.op_read(regs::op::USBCMD) & regs::usbcmd::RUN != 0 {
        let cmd = regs.op_read(regs::op::USBCMD);
        regs.op_write(regs::op::USBCMD, cmd & !regs::usbcmd::RUN);
        let mut stopped = false;
        for _ in 0..100 {
            if regs.op_read(regs::op::USBSTS) & regs::usbsts::HCHALTED != 0 {
                stopped = true;
                break;
            }
            cardbus::delay_ms(1);
        }
        if !stopped {
            w.push_bytes(b"EHCI: stop timeout\r\n");
            return false;
        }
    }

    regs.op_write(regs::op::USBCMD, regs::usbcmd::HCRESET);
    let mut reset_done = false;
    for _ in 0..100 {
        if regs.op_read(regs::op::USBCMD) & regs::usbcmd::HCRESET == 0 {
            reset_done = true;
            break;
        }
        cardbus::delay_ms(1);
    }
    if !reset_done {
        w.push_bytes(b"EHCI: reset timeout\r\n");
        return false;
    }
    // After HCRESET the controller sits in the halted state with all
    // operational registers at defaults; confirm that before
    // programming the schedule, so a half-reset controller is
    // reported rather than driven.
    let mut halted = false;
    for _ in 0..100 {
        if regs.op_read(regs::op::USBSTS) & regs::usbsts::HCHALTED != 0 {
            halted = true;
            break;
        }
        cardbus::delay_ms(1);
    }
    if !halted {
        w.push_bytes(b"EHCI: no halt after reset\r\n");
        return false;
    }

    let skel_phys = ehci_state().dma_phys + SKEL_QH_REL as u32;
    regs.op_write(regs::op::CTRLDSSEGMENT, 0);
    regs.op_write(regs::op::ASYNCLISTADDR, skel_phys);
    // Polled driver: no EHCI interrupts are enabled in v1, matching
    // the design's no-IRQ stance.
    regs.op_write(regs::op::USBINTR, 0);
    // Preserve the interrupt-threshold field (USBCMD 23:16) across
    // the start write; the reset default is the conservative value.
    let cmd = regs.op_read(regs::op::USBCMD);
    regs.op_write(
        regs::op::USBCMD,
        (cmd & 0x00FF_0000) | regs::usbcmd::ASE | regs::usbcmd::RUN,
    );
    let mut running = false;
    for _ in 0..100 {
        let sts = regs.op_read(regs::op::USBSTS);
        if sts & regs::usbsts::HCHALTED == 0 && sts & regs::usbsts::ASYNC_STATUS != 0 {
            running = true;
            break;
        }
        cardbus::delay_ms(1);
    }
    if !running {
        w.push_bytes(b"EHCI: run timeout\r\n");
        return false;
    }
    w.push_bytes(b"EHCI: running\r\n");
    true
}

/// One reset pulse per connected port, then the routing decision for
/// full/low-speed devices. High-speed devices stay with the EHCI;
/// full/low-speed devices are handed to their companion by setting
/// Port Owner — but only when that companion is actually running,
/// because handing a port to a disabled companion strands the device
/// on it (the design's never-strand rule).
unsafe fn ehci_port_pass() {
    let st = ehci_state();
    let params = HcsParams::from_raw(st.hcsparams);
    let mut regs = EhciMmio { op_base: st.op_base };
    // Companion 0 is the OHCI this build already drives; companion 1
    // (the DynaBook's OHCI #2) is never enabled by this step, so it
    // honestly reports as not running.
    let companions = [ohci::ohci_running(), st.comp1_running];
    let n_ports = params.n_ports().min(15);
    // When Port Power Control (PPC) is implemented (as on the NEC
    // uPD720100A), HCRESET leaves PORTSC.POWER clear and unpowered
    // ports never assert CONNECT. Power all ports first and wait
    // for VBUS ramp + USB 2.0 T_ATTDB debounce before scanning.
    if params.port_power_control() {
        let mut powered_any = false;
        for port in 0..n_ports {
            let cur = regs.port_read(port);
            if (cur & regs::portsc::POWER) == 0 {
                let keep = cur & regs::portsc::OWNER;
                regs.port_write(port, keep | regs::portsc::POWER);
                powered_any = true;
            }
        }
        if powered_any {
            cardbus::delay_ms(100);
        }
    }
    for port in 0..n_ports {
        let cur = regs.port_read(port);
        if cur & regs::portsc::CONNECT == 0 {
            continue;
        }
        // Preserve power and current ownership across the pulse;
        // everything else in PORTSC is status or write-to-clear.
        let keep = cur & (regs::portsc::POWER | regs::portsc::OWNER);
        regs.port_write(port, keep | regs::portsc::RESET);
        // Root-port reset is software-timed on EHCI: hold it long
        // enough for the device's chirp handshake, then release.
        cardbus::delay_ms(50);
        regs.port_write(port, keep);
        // Settle: wait for the enable to latch (high-speed) or the
        // device to drop, bounded so a confused port cannot hang
        // the verb.
        for _ in 0..20 {
            let v = regs.port_read(port);
            if v & regs::portsc::ENABLED != 0 || v & regs::portsc::CONNECT == 0 {
                break;
            }
            cardbus::delay_ms(5);
        }
        let after = regs.port_read(port);
        if after & regs::portsc::CONNECT == 0 || after & regs::portsc::ENABLED != 0 {
            continue; // gone, or high-speed: the EHCI keeps it.
        }
        match routing_decide(&params, port, DeviceSpeed::FullOrLow, &companions) {
            PortOwner::Companion(_) => {
                regs.port_write(port, keep | regs::portsc::OWNER);
            }
            _ => {
                // Stranded (or no companion exists): leave the port
                // EHCI-owned and unserved rather than handing it to
                // hardware that cannot answer for it. The status
                // report names it stranded.
            }
        }
    }
}

/// Thin indirection over the portable routing policy so the port
/// pass and the status report can never disagree about a decision.
fn routing_decide(
    params: &HcsParams,
    port: u8,
    speed: DeviceSpeed,
    companions: &[bool],
) -> PortOwner {
    ferric_ehci::routing::decide_owner(params, port, speed, companions)
}

/// The status half of verb 0x0037, shared by the first bring-up and
/// repeat calls: controller status decoded, CONFIGFLAG, and one line
/// per port with its classification and routing decision.
unsafe fn ehci_report_status(w: &mut EhciOut) {
    let st = ehci_state();
    let params = HcsParams::from_raw(st.hcsparams);
    let mut regs = EhciMmio { op_base: st.op_base };
    let sts = regs.op_read(regs::op::USBSTS);
    w.push_bytes(b"EHCI: usbsts=");
    w.push_hex32(sts);
    w.push_bytes(b" halted=");
    w.push_dec((sts & regs::usbsts::HCHALTED != 0) as u32);
    w.push_bytes(b" async=");
    w.push_dec((sts & regs::usbsts::ASYNC_STATUS != 0) as u32);
    w.push_bytes(b" periodic=");
    w.push_dec((sts & regs::usbsts::PERIODIC_STATUS != 0) as u32);
    w.crlf();
    let cf = regs.op_read(regs::op::CONFIGFLAG);
    w.push_bytes(b"EHCI: configflag=");
    w.push_dec(cf & 1);
    w.crlf();

    let companions = [ohci::ohci_running(), st.comp1_running];
    let n_ports = params.n_ports().min(15);
    for port in 0..n_ports {
        let v = regs.port_read(port);
        let connect = v & regs::portsc::CONNECT != 0;
        let enabled = v & regs::portsc::ENABLED != 0;
        let line = (v & regs::portsc::LINE_STATUS_MASK) >> regs::portsc::LINE_STATUS_SHIFT;
        w.push_bytes(b"EHCI: port");
        w.push_dec(port as u32);
        w.push_bytes(b" connect=");
        w.push_dec(connect as u32);
        w.push_bytes(b" enabled=");
        w.push_dec(enabled as u32);
        w.push_bytes(b" line=");
        match line {
            x if x == regs::portsc::LINE_J => w.push_bytes(b"J"),
            x if x == regs::portsc::LINE_K => w.push_bytes(b"K"),
            _ => w.push_bytes(b"SE0"),
        }
        w.push_bytes(b" speed=");
        let speed = if !connect {
            DeviceSpeed::Absent
        } else if enabled {
            DeviceSpeed::High
        } else {
            DeviceSpeed::FullOrLow
        };
        match speed {
            DeviceSpeed::Absent => w.push_bytes(b"none"),
            DeviceSpeed::High => w.push_bytes(b"high"),
            DeviceSpeed::FullOrLow => w.push_bytes(b"full/low"),
        }
        w.push_bytes(b" owner=");
        if v & regs::portsc::OWNER != 0 {
            // The bit is the ground truth once applied: name the
            // companion the routing arithmetic assigns to this port.
            match ferric_ehci::routing::companion_for_port(&params, port) {
                Some(c) => {
                    w.push_bytes(b"companion");
                    w.push_dec(c as u32);
                }
                None => w.push_bytes(b"companion"),
            }
        } else {
            match routing_decide(&params, port, speed, &companions) {
                PortOwner::Ehci => w.push_bytes(b"ehci"),
                PortOwner::Companion(c) => {
                    w.push_bytes(b"companion");
                    w.push_dec(c as u32);
                }
                PortOwner::Stranded => w.push_bytes(b"stranded"),
            }
        }
        w.crlf();
    }
}

/// Is the EHCI controller brought up with its async schedule and DMA
/// block live? Consulted by `ohci.rs`'s MSC/block layer when a drive
/// is bound through EHCI.
pub(crate) fn ehci_ready() -> bool {
    let st = ehci_state();
    st.brought_up && st.op_base != 0 && st.dma_lin != 0 && st.dma_phys != 0
}

/// Splice `qh` behind the skeleton async QH, wait (bounded) for the
/// final qTD at `last_qtd_rel` to retire or halt, unlink `qh`, and
/// flush the controller's async-schedule cache via the IAA doorbell.
/// Returns `Ok(final_last_token)` on completion or `Err(error_token)`
/// on halt/error/timeout.
unsafe fn ehci_run_qh(mut qh: QueueHead, last_qtd_rel: usize, timeout_ms: u32) -> Result<u32, u32> {
    let st = ehci_state();
    if !st.brought_up || st.op_base == 0 || st.dma_lin == 0 || st.dma_phys == 0 {
        return Err(ferric_ehci::queue::qtd_status::HALTED);
    }
    let dma_lin = st.dma_lin as usize;
    let dma_phys = st.dma_phys;
    let skel_phys = dma_phys + SKEL_QH_REL as u32;
    let xfer_phys = dma_phys + CTRL_QH_REL as u32;

    qh.horiz_link = ferric_ehci::queue::link_ptr(
        skel_phys,
        ferric_ehci::queue::LinkType::QueueHead,
        false,
    );
    let xfer_ptr = (dma_lin + CTRL_QH_REL) as *mut QueueHead;
    let skel_ptr = (dma_lin + SKEL_QH_REL) as *mut QueueHead;
    let last_qtd_ptr = (dma_lin + last_qtd_rel) as *const ferric_ehci::queue::Qtd;

    unsafe {
        core::ptr::write_volatile(xfer_ptr, qh);
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        core::ptr::write_volatile(
            &raw mut (*skel_ptr).horiz_link,
            ferric_ehci::queue::link_ptr(
                xfer_phys,
                ferric_ehci::queue::LinkType::QueueHead,
                false,
            ),
        );
    }

    let mut regs = EhciMmio { op_base: st.op_base };
    let mut final_tok = ferric_ehci::queue::qtd_status::ACTIVE;
    let mut done = false;
    let mut halted_tok = 0u32;

    // Fast spin first so high-speed 512 B transfers finish in microseconds
    // without paying a 1 ms PIT sleep per sector.
    for _ in 0..4000u32 {
        let ltok = unsafe { core::ptr::read_volatile(&raw const (*last_qtd_ptr).token) };
        let otok = unsafe { core::ptr::read_volatile(&raw const (*xfer_ptr).overlay.token) };
        if (ltok & ferric_ehci::queue::qtd_status::ACTIVE) == 0 {
            final_tok = ltok;
            done = true;
            break;
        }
        if (otok & ferric_ehci::queue::qtd_status::HALTED) != 0 {
            halted_tok = otok;
            break;
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }

    if !done && halted_tok == 0 {
        for _ in 0..timeout_ms {
            let ltok = unsafe { core::ptr::read_volatile(&raw const (*last_qtd_ptr).token) };
            let otok = unsafe { core::ptr::read_volatile(&raw const (*xfer_ptr).overlay.token) };
            if (ltok & ferric_ehci::queue::qtd_status::ACTIVE) == 0 {
                final_tok = ltok;
                done = true;
                break;
            }
            if (otok & ferric_ehci::queue::qtd_status::HALTED) != 0 {
                halted_tok = otok;
                break;
            }
            if (regs.op_read(regs::op::USBSTS) & regs::usbsts::HOST_ERROR) != 0 {
                halted_tok = ferric_ehci::queue::qtd_status::HALTED
                    | ferric_ehci::queue::qtd_status::DATA_BUF_ERR;
                break;
            }
            cardbus::delay_ms(1);
        }
    }

    // Unlink XFER_QH from the async schedule and ring IAA doorbell so
    // hardware flushes its cached pointer before we reuse CTRL_QH_REL.
    unsafe {
        core::ptr::write_volatile(
            &raw mut (*skel_ptr).horiz_link,
            ferric_ehci::queue::link_ptr(
                skel_phys,
                ferric_ehci::queue::LinkType::QueueHead,
                false,
            ),
        );
    }
    regs.op_write(regs::op::USBSTS, regs::usbsts::ASYNC_ADVANCE);
    let cmd = regs.op_read(regs::op::USBCMD);
    regs.op_write(regs::op::USBCMD, cmd | regs::usbcmd::IAA_DOORBELL);
    let mut iaa_done = false;
    for _ in 0..2000u32 {
        if (regs.op_read(regs::op::USBSTS) & regs::usbsts::ASYNC_ADVANCE) != 0 {
            iaa_done = true;
            break;
        }
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
    }
    if !iaa_done {
        for _ in 0..20u32 {
            if (regs.op_read(regs::op::USBSTS) & regs::usbsts::ASYNC_ADVANCE) != 0 {
                break;
            }
            cardbus::delay_ms(1);
        }
    }
    regs.op_write(regs::op::USBSTS, regs::usbsts::ASYNC_ADVANCE);

    if done && ferric_ehci::queue::Qtd::error_bits(final_tok) == 0 {
        Ok(final_tok)
    } else if halted_tok != 0 {
        Err(halted_tok)
    } else {
        Err(final_tok)
    }
}

/// High-speed USB 2.0 control transfer (EP0) over the EHCI async schedule.
/// Supports no-data requests (SET_ADDRESS, SET_CONFIGURATION, CLEAR_FEATURE)
/// and IN-data requests (GET_DESCRIPTOR) up to `DATA_BUF_MAX` bytes.
pub(crate) unsafe fn ehci_control_xfer(
    addr: u8,
    max_pkt0: u16,
    setup: &[u8; 8],
    data_in: Option<&mut [u8]>,
) -> Option<usize> {
    if !ehci_ready() {
        return None;
    }
    let st = ehci_state();
    let dma_lin = st.dma_lin as usize;
    let dma_phys = st.dma_phys;
    let mp0 = if max_pkt0 == 0 { 64 } else { max_pkt0 };

    // Copy 8-byte SETUP packet into DMA arena.
    unsafe {
        core::ptr::copy_nonoverlapping(setup.as_ptr(), (dma_lin + SETUP_BUF_REL) as *mut u8, 8);
    }

    let qtd0_rel = QTD_POOL_REL;
    let qtd1_rel = QTD_POOL_REL + 32;
    let qtd2_rel = QTD_POOL_REL + 64;
    let qtd0_phys = dma_phys + qtd0_rel as u32;
    let qtd1_phys = dma_phys + qtd1_rel as u32;
    let qtd2_phys = dma_phys + qtd2_rel as u32;

    let data_len = match &data_in {
        Some(buf) => {
            if buf.len() > DATA_BUF_MAX {
                return None;
            }
            buf.len()
        }
        None => 0,
    };

    let mut q_setup = ferric_ehci::queue::Qtd::IDLE;
    q_setup.next = qtd1_phys;
    q_setup.alt_next = ferric_ehci::queue::LINK_TERMINATED;
    q_setup.token = ferric_ehci::queue::qtd_token(ferric_ehci::queue::Pid::Setup, 3, false, 8, false);
    q_setup.set_buffer(dma_phys + SETUP_BUF_REL as u32);

    let last_rel = if data_len > 0 {
        let mut q_data = ferric_ehci::queue::Qtd::IDLE;
        q_data.next = qtd2_phys;
        q_data.alt_next = qtd2_phys; // Short packet advances cleanly to STATUS
        q_data.token = ferric_ehci::queue::qtd_token(
            ferric_ehci::queue::Pid::In,
            3,
            false,
            data_len as u16,
            true,
        );
        q_data.set_buffer(dma_phys + DATA_BUF_REL as u32);

        let mut q_status = ferric_ehci::queue::Qtd::IDLE;
        q_status.next = ferric_ehci::queue::LINK_TERMINATED;
        q_status.alt_next = ferric_ehci::queue::LINK_TERMINATED;
        q_status.token =
            ferric_ehci::queue::qtd_token(ferric_ehci::queue::Pid::Out, 3, true, 0, true);

        unsafe {
            core::ptr::write_volatile((dma_lin + qtd0_rel) as *mut ferric_ehci::queue::Qtd, q_setup);
            core::ptr::write_volatile((dma_lin + qtd1_rel) as *mut ferric_ehci::queue::Qtd, q_data);
            core::ptr::write_volatile((dma_lin + qtd2_rel) as *mut ferric_ehci::queue::Qtd, q_status);
        }
        qtd2_rel
    } else {
        let mut q_status = ferric_ehci::queue::Qtd::IDLE;
        q_status.next = ferric_ehci::queue::LINK_TERMINATED;
        q_status.alt_next = ferric_ehci::queue::LINK_TERMINATED;
        q_status.token =
            ferric_ehci::queue::qtd_token(ferric_ehci::queue::Pid::In, 3, true, 0, true);

        unsafe {
            core::ptr::write_volatile((dma_lin + qtd0_rel) as *mut ferric_ehci::queue::Qtd, q_setup);
            core::ptr::write_volatile((dma_lin + qtd1_rel) as *mut ferric_ehci::queue::Qtd, q_status);
        }
        qtd1_rel
    };

    let mut qh = QueueHead::skeleton_async(dma_phys + CTRL_QH_REL as u32);
    qh.characteristics = ferric_ehci::queue::qh_characteristics(
        addr,
        0,
        ferric_ehci::queue::Speed::High,
        mp0,
        false,
        true,
        false,
        0,
    );
    qh.capabilities = ferric_ehci::queue::qh_capabilities(1, 0, 0);
    qh.overlay.next = qtd0_phys;
    qh.overlay.alt_next = ferric_ehci::queue::LINK_TERMINATED;
    qh.overlay.token = 0;

    unsafe { ehci_run_qh(qh, last_rel, 500) }.ok()?;

    if let Some(buf) = data_in {
        let data_tok = unsafe {
            core::ptr::read_volatile(&raw const (*((dma_lin + qtd1_rel) as *const ferric_ehci::queue::Qtd)).token)
        };
        let rem = ferric_ehci::queue::Qtd::bytes_remaining(data_tok) as usize;
        let xferred = data_len.saturating_sub(rem);
        unsafe {
            core::ptr::copy_nonoverlapping(
                (dma_lin + DATA_BUF_REL) as *const u8,
                buf.as_mut_ptr(),
                xferred,
            );
        }
        Some(xferred)
    } else {
        Some(0)
    }
}

/// High-speed USB 2.0 bulk transfer (Bulk IN or Bulk OUT) over the EHCI
/// async schedule, matching `ohci_bulk_xfer`'s contract (`out_cc = 4` on
/// STALL so BOT reset recovery in `ohci.rs` works identically).
pub(crate) unsafe fn ehci_bulk_xfer(
    addr: u8,
    ep: u8,
    max_pkt: u16,
    is_in: bool,
    toggle: &mut u8,
    out_cc: &mut u32,
    buf: &mut [u8],
) -> Option<usize> {
    const OHCI_CC_STALL: u32 = 4;
    const OHCI_CC_NOT_ACCESSED: u32 = 15;
    *out_cc = OHCI_CC_NOT_ACCESSED;

    if !ehci_ready() || buf.len() > DATA_BUF_MAX {
        return None;
    }
    if buf.is_empty() {
        *out_cc = 0;
        return Some(0);
    }

    let st = ehci_state();
    let dma_lin = st.dma_lin as usize;
    let dma_phys = st.dma_phys;
    let mp = if max_pkt == 0 { 512 } else { max_pkt };
    let len = buf.len();

    if !is_in {
        unsafe {
            core::ptr::copy_nonoverlapping(buf.as_ptr(), (dma_lin + DATA_BUF_REL) as *mut u8, len);
        }
    }

    let qtd0_rel = QTD_POOL_REL;
    let qtd0_phys = dma_phys + qtd0_rel as u32;
    let pid = if is_in {
        ferric_ehci::queue::Pid::In
    } else {
        ferric_ehci::queue::Pid::Out
    };
    let mut q_bulk = ferric_ehci::queue::Qtd::IDLE;
    q_bulk.next = ferric_ehci::queue::LINK_TERMINATED;
    q_bulk.alt_next = ferric_ehci::queue::LINK_TERMINATED;
    q_bulk.token = ferric_ehci::queue::qtd_token(pid, 3, true, len as u16, (*toggle & 1) != 0);
    q_bulk.set_buffer(dma_phys + DATA_BUF_REL as u32);

    unsafe {
        core::ptr::write_volatile((dma_lin + qtd0_rel) as *mut ferric_ehci::queue::Qtd, q_bulk);
    }

    let mut qh = QueueHead::skeleton_async(dma_phys + CTRL_QH_REL as u32);
    qh.characteristics = ferric_ehci::queue::qh_characteristics(
        addr,
        ep & 0x0F,
        ferric_ehci::queue::Speed::High,
        mp,
        false,
        true,
        false,
        0,
    );
    qh.capabilities = ferric_ehci::queue::qh_capabilities(1, 0, 0);
    qh.overlay.next = qtd0_phys;
    qh.overlay.alt_next = ferric_ehci::queue::LINK_TERMINATED;
    qh.overlay.token = 0;

    match unsafe { ehci_run_qh(qh, qtd0_rel, 1500) } {
        Ok(tok) => {
            *toggle = ((tok >> 31) & 1) as u8;
            let rem = ferric_ehci::queue::Qtd::bytes_remaining(tok) as usize;
            let xferred = len.saturating_sub(rem);
            if is_in {
                unsafe {
                    core::ptr::copy_nonoverlapping(
                        (dma_lin + DATA_BUF_REL) as *const u8,
                        buf.as_mut_ptr(),
                        xferred,
                    );
                }
            }
            *out_cc = 0;
            Some(xferred)
        }
        Err(err_tok) => {
            let pure_stall = (err_tok & ferric_ehci::queue::qtd_status::HALTED) != 0
                && (err_tok
                    & (ferric_ehci::queue::qtd_status::XACT_ERR
                        | ferric_ehci::queue::qtd_status::BABBLE
                        | ferric_ehci::queue::qtd_status::DATA_BUF_ERR))
                    == 0;
            if pure_stall {
                *out_cc = OHCI_CC_STALL;
            }
            None
        }
    }
}

/// Parse a USB configuration descriptor for a Bulk-Only Mass Storage
/// interface (`class=0x08`, `subclass=0x06`, `protocol=0x50`).
/// Returns `(config_val, if_num, ep_in, ep_out, max_pkt)`.
fn parse_ehci_msc_config(cfg: &[u8]) -> Option<(u8, u8, u8, u8, u16)> {
    if cfg.len() < 9 || cfg[1] != 2 {
        return None;
    }
    let cfg_val = cfg[5];
    let mut off = 0usize;
    let mut in_msc_if = false;
    let mut if_num = 0u8;
    let mut ep_in = 0u8;
    let mut ep_out = 0u8;
    let mut max_pkt = 512u16;

    while off + 2 <= cfg.len() {
        let dlen = cfg[off] as usize;
        let dtype = cfg[off + 1];
        if dlen < 2 || off + dlen > cfg.len() {
            break;
        }
        if dtype == 4 && dlen >= 9 {
            // Interface descriptor
            let cls = cfg[off + 5];
            let sub = cfg[off + 6];
            let proto = cfg[off + 7];
            if cls == 0x08 && sub == 0x06 && proto == 0x50 {
                in_msc_if = true;
                if_num = cfg[off + 2];
                ep_in = 0;
                ep_out = 0;
                max_pkt = 512;
            } else if in_msc_if && ep_in != 0 && ep_out != 0 {
                break;
            } else {
                in_msc_if = false;
            }
        } else if dtype == 5 && dlen >= 7 && in_msc_if {
            // Endpoint descriptor: bulk = attr & 3 == 2
            let addr = cfg[off + 2];
            let attr = cfg[off + 3];
            let mp = u16::from_le_bytes([cfg[off + 4], cfg[off + 5]]) & 0x07FF;
            if (attr & 0x03) == 0x02 {
                if (addr & 0x80) != 0 {
                    ep_in = addr & 0x0F;
                    if mp != 0 {
                        max_pkt = mp;
                    }
                } else {
                    ep_out = addr & 0x0F;
                    if mp != 0 {
                        max_pkt = mp;
                    }
                }
            }
        }
        off += dlen;
    }
    if ep_in != 0 && ep_out != 0 {
        Some((cfg_val, if_num, ep_in, ep_out, max_pkt))
    } else {
        None
    }
}

/// Enumerate high-speed USB 2.0 Mass Storage devices on EHCI-owned ports
/// and bind the first MSC drive into the shared block layer (`ohci.rs`).
unsafe fn ehci_enumerate_msc(w: &mut EhciOut) {
    let st = ehci_state();
    let params = HcsParams::from_raw(st.hcsparams);
    let mut regs = EhciMmio { op_base: st.op_base };
    let n_ports = params.n_ports().min(15);
    let mut next_addr = 1u8;

    for port in 0..n_ports {
        let v = regs.port_read(port);
        if (v & regs::portsc::CONNECT) == 0
            || (v & regs::portsc::ENABLED) == 0
            || (v & regs::portsc::OWNER) != 0
        {
            continue;
        }
        // High-speed device on EHCI root port: GET_DESCRIPTOR(Device, 8) at addr 0.
        let mut dev8 = [0u8; 8];
        let get_dev8 = [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x08, 0x00];
        if unsafe { ehci_control_xfer(0, 64, &get_dev8, Some(&mut dev8)) }.is_none() {
            continue;
        }
        let mp0 = if dev8[7] == 0 { 64 } else { dev8[7] as u16 };
        let addr = next_addr;
        next_addr = next_addr.saturating_add(1);

        let set_addr = [0x00, 0x05, addr, 0x00, 0x00, 0x00, 0x00, 0x00];
        if unsafe { ehci_control_xfer(0, mp0, &set_addr, None) }.is_none() {
            continue;
        }
        cardbus::delay_ms(10);

        // Read 9-byte config header to learn wTotalLength.
        let mut cfg_hdr = [0u8; 9];
        let get_cfg9 = [0x80, 0x06, 0x00, 0x02, 0x00, 0x00, 0x09, 0x00];
        if unsafe { ehci_control_xfer(addr, mp0, &get_cfg9, Some(&mut cfg_hdr)) }.is_none() {
            continue;
        }
        let total_len = u16::from_le_bytes([cfg_hdr[2], cfg_hdr[3]]).min(256);
        let mut cfg_buf = [0u8; 256];
        let get_cfg = [
            0x80,
            0x06,
            0x00,
            0x02,
            0x00,
            0x00,
            (total_len & 0xFF) as u8,
            (total_len >> 8) as u8,
        ];
        let Some(got) = (unsafe {
            ehci_control_xfer(addr, mp0, &get_cfg, Some(&mut cfg_buf[..total_len as usize]))
        }) else {
            continue;
        };
        let Some((cfg_val, if_num, ep_in, ep_out, max_pkt)) =
            parse_ehci_msc_config(&cfg_buf[..got])
        else {
            continue;
        };

        let set_cfg = [0x00, 0x09, cfg_val, 0x00, 0x00, 0x00, 0x00, 0x00];
        if unsafe { ehci_control_xfer(addr, mp0, &set_cfg, None) }.is_none() {
            continue;
        }
        cardbus::delay_ms(20);

        let (ready, sectors, has_fat, has_exfat, has_ntfs) =
            unsafe { ohci::bind_ehci_msc(addr, if_num, ep_in, ep_out, max_pkt, mp0) };
        if ready {
            w.push_bytes(b"EHCI: msc port=");
            w.push_dec(port as u32);
            w.push_bytes(b" addr=");
            w.push_dec(addr as u32);
            w.push_bytes(b" sectors=");
            w.push_dec(sectors);
            w.push_bytes(b" fs=");
            if has_exfat {
                w.push_bytes(b"EXFAT");
            } else if has_ntfs {
                w.push_bytes(b"NTFS");
            } else if has_fat {
                w.push_bytes(b"FAT");
            } else {
                w.push_bytes(b"RAW");
            }
            w.crlf();
            break;
        }
    }
}

/// Bring up sibling companion OHCI #2 (function 1 on the same PCI device,
/// e.g. `00:0C.1` on the DynaBook's NEC uPD720100A) if it exists and was
/// left BIOS-disabled (`BAR0 = 0`). Once running, full/low-speed devices
/// on companion 1's ports (ports 3..4) route cleanly to `companion1`
/// instead of being stranded when EHCI claims `CONFIGFLAG = 1`.
unsafe fn ehci_enable_companion1(w: &mut EhciOut) {
    let (bus, dev) = {
        let st = ehci_state();
        if st.comp1_running {
            return;
        }
        (st.bus, st.dev)
    };
    let id = unsafe { pci_read32(bus, dev, 1, 0) };
    let vid = (id & 0xFFFF) as u16;
    let did = (id >> 16) as u16;
    if vid == 0xFFFF || vid == 0x0000 {
        return;
    }
    let cc = unsafe { pci_read32(bus, dev, 1, 0x08) };
    let class = ((cc >> 24) & 0xFF) as u8;
    let subclass = ((cc >> 16) & 0xFF) as u8;
    let progif = ((cc >> 8) & 0xFF) as u8;
    if !(class == 0x0C && subclass == 0x03 && progif == 0x10) {
        return;
    }

    let mut bar = unsafe { pci_read32(bus, dev, 1, 0x10) } & 0xFFFF_F000;
    if bar == 0 {
        let size = unsafe { ehci_bar_size(bus, dev, 1) };
        if size == 0 {
            return;
        }
        let Some(assigned) = (unsafe { assign_ehci_bar(bus, dev, 1, size) }) else {
            return;
        };
        bar = assigned;
    }

    // Wake PMCSR from D3hot -> D0 if present, then enable MEM + Bus Master.
    unsafe {
        let cmd_stat = pci_read32(bus, dev, 1, 0x04);
        if ((cmd_stat >> 16) & 0x0010) != 0 {
            let mut cap_ptr = (pci_read32(bus, dev, 1, 0x34) & 0xFC) as u8;
            for _ in 0..16 {
                if cap_ptr < 0x40 {
                    break;
                }
                let cap_hdr = pci_read32(bus, dev, 1, cap_ptr);
                if (cap_hdr & 0xFF) == 0x01 {
                    let pmcsr = pci_read32(bus, dev, 1, cap_ptr + 4);
                    if (pmcsr & 0x0003) != 0 {
                        pci_write32(bus, dev, 1, cap_ptr + 4, pmcsr & !0x0003);
                        cardbus::delay_ms(10);
                    }
                    break;
                }
                cap_ptr = ((cap_hdr >> 8) & 0xFC) as u8;
            }
        }
        let cmd = pci_read32(bus, dev, 1, 0x04);
        pci_write32(bus, dev, 1, 0x04, ((cmd & 0xFFFF) | 0x0006) & !0x0400);
    }

    let lin = unsafe { cardbus::map_mmio(bar, 1) };
    if lin == 0 {
        return;
    }
    let rev = unsafe { mmio_r32(lin, 0x00) };
    if rev == 0xFFFF_FFFF || (rev & 0xFF) == 0 {
        return;
    }
    // Soft reset OHCI #2 (HcCommandStatus.HCR = bit 0), move to
    // USBOPERATIONAL (HcControl = 0x80), and assert global port power
    // (HcRhStatus.LPSC = bit 16) so handed-over full/low-speed ports stay live.
    unsafe {
        mmio_w32(lin, 0x08, 0x0000_0001);
        for _ in 0..20 {
            if (mmio_r32(lin, 0x08) & 1) == 0 {
                break;
            }
            cardbus::delay_ms(1);
        }
        mmio_w32(lin, 0x04, 0x0000_0080); // USBOPERATIONAL
        mmio_w32(lin, 0x50, 0x0001_0000); // HcRhStatus: SetGlobalPower (LPSC)
    }

    let st = ehci_state();
    st.comp1_running = true;
    st.comp1_bar_phys = bar;
    w.push_bytes(b"EHCI: comp1 ");
    w.push_hex(vid as u32, 4);
    w.push_byte(b':');
    w.push_hex(did as u32, 4);
    w.push_bytes(b" at ");
    w.push_hex(bus as u32, 2);
    w.push_byte(b':');
    w.push_hex(dev as u32, 2);
    w.push_bytes(b".1 bar0=");
    w.push_hex32(bar);
    w.push_bytes(b" running\r\n");
}

/// FFI entry for JLM V86 API `AX=0x0038` (`EHCPROBE /U`) and `DllMain`
/// `DLL_PROCESS_DETACH`: halt the EHCI async schedule, clear `CONFIGFLAG`
/// back to 0 so all root ports return to the OHCI companions, and unbind
/// any EHCI-backed MSC drive. Returns 1 if EHCI was active and shut down,
/// or 0 if it was not running.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_ehci_shutdown() -> u32 {
    let st = ehci_state();
    if !st.brought_up || st.op_base == 0 {
        return 0;
    }
    let mut regs = EhciMmio { op_base: st.op_base };
    let cmd = regs.op_read(regs::op::USBCMD);
    regs.op_write(
        regs::op::USBCMD,
        cmd & !(regs::usbcmd::RUN | regs::usbcmd::ASE | regs::usbcmd::PSE),
    );
    for _ in 0..50 {
        if (regs.op_read(regs::op::USBSTS) & regs::usbsts::HCHALTED) != 0 {
            break;
        }
        cardbus::delay_ms(1);
    }
    regs.op_write(regs::op::CONFIGFLAG, regs::CONFIGFLAG_COMPANIONS_OWN);
    ohci::unbind_ehci_msc();
    st.brought_up = false;
    st.dma_lin = 0;
    st.dma_phys = 0;
    1
}

/// FFI entry for JLM V86 API `AX=0x0037` (`EHCPROBE.COM /B`): bring
/// the EHCI up (handoff, reset, async schedule, companion OHCI #2,
/// CONFIGFLAG, port routing, MSC enumeration) and format the status
/// report into `dst_lin[..cap]`.
/// Returns `len | (fresh_bringup << 16)` so `jlm_stub.asm` places the
/// report length in `AX` and `1` in `BX` iff the caller must stay resident.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_ehci_bringup(dma_seg: u32, dst_lin: u32, cap: u32) -> u32 {
    if dst_lin == 0 || cap == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(dst_lin as *mut u8, cap as usize) };
    let mut w = EhciOut::new(slice);
    if !unsafe { ehci_prepare() } {
        let st = ehci_state();
        if st.present {
            w.push_header(st);
            w.push_bytes(b"EHCI: setup failed (BAR/MMIO)\r\n");
        } else {
            w.push_bytes(b"EHCI: none found\r\n");
        }
        return w.pos as u32;
    }
    {
        let st = ehci_state();
        w.push_header(st);
    }
    let mut fresh = false;
    if ehci_state().brought_up {
        w.push_bytes(b"EHCI: already up\r\n");
    } else {
        unsafe { ehci_bios_handoff(&mut w) };
        let mut ok = unsafe { ehci_setup_dma(&mut w, dma_seg) };
        if ok {
            ok = unsafe { ehci_reset_and_run(&mut w) };
        }
        if ok {
            unsafe { ehci_enable_companion1(&mut w) };
            // Claim routing only once the async schedule is live:
            // before that, flipping CONFIGFLAG would take ports away
            // from the companions with nothing ready to serve the
            // high-speed ones.
            let st = ehci_state();
            let mut regs = EhciMmio { op_base: st.op_base };
            regs.op_write(regs::op::CONFIGFLAG, regs::CONFIGFLAG_EHCI_ROUTES);
            unsafe { ehci_port_pass() };
            ehci_state().brought_up = true;
            fresh = true;
            unsafe { ehci_enumerate_msc(&mut w) };
        }
    }
    unsafe { ehci_report_status(&mut w) };
    (w.pos as u32) | if fresh { 0x0001_0000 } else { 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_span_is_a_safe_overestimate() {
        // OHCI #1's 4 KiB BAR on the DynaBook.
        assert_eq!(bar_span(0xF7EB_F000), 0x1000);
        // The Trident's 32 MiB framebuffer BAR: lowbit gives 64 MiB,
        // an over-reserve, which is the safe direction.
        assert_eq!(bar_span(0xFC00_0000), 0x0400_0000);
        // A small BAR at an odd page: at least one page is reserved.
        assert_eq!(bar_span(0xF7F0_0400), 0x1000);
    }

    #[test]
    fn overlap_against_the_dynabook_neighbourhood() {
        // The expected Stage H assignment: the first window above
        // the search floor must not collide with OHCI #1's page.
        assert!(!ranges_overlap(
            0xF7F0_0000,
            0x1000,
            0xF7EB_F000,
            bar_span(0xF7EB_F000)
        ));
        // A candidate directly on top of a populated BAR is taken.
        assert!(ranges_overlap(
            0xF7EB_F000,
            0x1000,
            0xF7EB_F000,
            bar_span(0xF7EB_F000)
        ));
        // Adjacent pages do not overlap.
        assert!(!ranges_overlap(0x1000, 0x1000, 0x2000, 0x1000));
    }

    #[test]
    fn dynabook_census_leaves_the_search_floor_free() {
        // The DynaBook's real bus-0 census, from
        // docs/hardware/dynabook-ss2000m-hwscan.txt, as (class
        // register, BAR base) pairs exactly as the scan collects
        // them. The host bridge's offset-0x10 value is a chipset
        // register, not a BAR; counting its 256 MiB phantom span
        // used to swallow the whole search window and leave the
        // EHCI function unassignable on silicon.
        let census: [(u32, u32); 3] = [
            (0x0600_0001, 0xF000_0000), // 00:00.0 ALi M1644 host bridge, "bar0" 0xF0000008
            (0x0200_000D, 0xF7EF_F000), // 00:0A.0 Intel 82550 Ethernet
            (0x0C03_1041, 0xF7EB_F000), // 00:0C.0 NEC OHCI #1
        ];
        // The first window assign_ehci_bar tries is free.
        assert!(!window_taken_by_census(0xF7F0_0000, 0x1000, &census));
        // A candidate on top of the Ethernet BAR is still taken:
        // the host-bridge skip must not blunt the real checks.
        assert!(window_taken_by_census(0xF7EF_F000, 0x1000, &census));
        // An EHCI BAR placed at 0xFE000000 occupies only 4 KiB, leaving
        // 0xFE001000 free for companion OHCI #2 (00:0C.1).
        let with_ehci = [(0x0C03_2002u32, 0xFE00_0000u32)];
        assert!(window_taken_by_census(0xFE00_0000, 0x1000, &with_ehci));
        assert!(!window_taken_by_census(0xFE00_1000, 0x1000, &with_ehci));
    }

    #[test]
    fn agp_bridge_window_blocks_candidate() {
        // Type-1 PCI-to-PCI bridge Memory Base/Limit = 0xFDF0_F800
        // (forwards 0xF800_0000..=0xFDFF_FFFF to bus 1, e.g. AGP VGA).
        let agp_win = 0xFDF0_F800u32;
        assert!(pci_bridge_window_blocks(agp_win, 0xF800_0000, 0x1000));
        assert!(pci_bridge_window_blocks(agp_win, 0xFC00_0000, 0x1000));
        // Bus-0 neighbourhood below the AGP bridge is free.
        assert!(!pci_bridge_window_blocks(agp_win, 0xF7EC_0000, 0x1000));
        // Disabled bridge window (base > limit) blocks nothing.
        assert!(!pci_bridge_window_blocks(0x0000_FFF0, 0xF800_0000, 0x1000));
    }

    #[test]
    fn parse_ehci_msc_config_extracts_hs_bulk_endpoints() {
        // Minimal USB 2.0 MSC Bulk-Only config descriptor: 9B config + 9B if + 7B EP IN + 7B EP OUT.
        let cfg: [u8; 32] = [
            9, 2, 32, 0, 1, 1, 0, 0x80, 50,
            9, 4, 0, 0, 2, 0x08, 0x06, 0x50, 0,
            7, 5, 0x81, 0x02, 0x00, 0x02, 0,
            7, 5, 0x02, 0x02, 0x00, 0x02, 0,
        ];
        assert_eq!(parse_ehci_msc_config(&cfg), Some((1, 0, 1, 2, 512)));
    }
}
