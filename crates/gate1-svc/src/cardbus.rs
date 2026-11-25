//! Gate 4b Step 4b.3 — Toshiba ToPIC95 CardBus bridge driver (resident in FERRIC.JLM).
//!
//! Brings up the DynaBook SS 2000M's dual Toshiba ToPIC95 CardBus bridges
//! (`1179:0617` rev `0x32` at PCI `00:11.0` / `00:11.1`, unconfigured at DOS boot)
//! so a 3.3 V CardBus card (D-Link DWL-G630 C2 / Atheros AR2413) can be powered,
//! reset-released, enumerated on its secondary bus, and assigned a non-overlapping
//! MMIO BAR0 window.
//!
//! Register definitions are verified against `vendor/ref/linux-ath/pcmcia/topic.h`
//! and `vendor/ref/linux-ath/pcmcia/yenta_socket.c`. Strictly `#![no_std]`, 386-pure,
//! and read-only with respect to storage.

use core::cell::UnsafeCell;

#[cfg(target_arch = "x86")]
use crate::{inb, outb};
use crate::{pci_read32, pci_write32};

#[cfg(target_arch = "x86")]
unsafe extern "C" {
    /// Ring-0 MMIO mapper in `jlm_stub.asm` (`VMM` `_PageReserve(PR_SYSTEM)` +
    /// `_PageCommitPhys(..., PC_WRITEABLE | PC_CACHEDIS | PC_INCR)`).
    /// Returns mapped flat linear address or 0 on failure.
    fn jlm_map_mmio(phys: u32, npages: u32) -> u32;
}

#[cfg(not(target_arch = "x86"))]
unsafe fn jlm_map_mmio(_phys: u32, _npages: u32) -> u32 {
    0
}

/// Shared Ring-0 uncached MMIO mapper wrapper for crate-internal drivers.
pub(crate) unsafe fn map_mmio(phys: u32, npages: u32) -> u32 {
    unsafe { jlm_map_mmio(phys, npages) }
}

// ---------------------------------------------------------------------
// Verified PCI-to-CardBus & Toshiba ToPIC95 register constants
// (vendor/ref/linux-ath/pcmcia/topic.h & yenta_socket.c)
// ---------------------------------------------------------------------

pub const TOPIC95_VID: u16 = 0x1179;
pub const TOPIC95_DID: u16 = 0x0617;
pub const TOPIC95_PCI_DEV: u8 = 17; // 00:11.0 and 00:11.1

// Standard PCI-to-CardBus bridge config offsets (Header Type 0x02).
const PCI_CMD: u8 = 0x04;
const PCI_CLASS_REV: u8 = 0x08;
const PCI_CLS_LT: u8 = 0x0C;
const PCI_BAR0: u8 = 0x10;
const PCI_BUS_NUM: u8 = 0x18;
const PCI_CB_MEM0_BASE: u8 = 0x1C;
const PCI_CB_MEM0_LIMIT: u8 = 0x20;
const PCI_CB_MEM1_BASE: u8 = 0x24;
const PCI_CB_MEM1_LIMIT: u8 = 0x28;
const PCI_CB_IO0_BASE: u8 = 0x2C;
const PCI_CB_IO0_LIMIT: u8 = 0x30;
const PCI_CB_IO1_BASE: u8 = 0x34;
const PCI_CB_IO1_LIMIT: u8 = 0x38;
const PCI_IRQ_BCTRL: u8 = 0x3C; // [7:0] IRQ line, [15:8] IRQ pin, [31:16] Bridge Control
const PCI_CB_LEGACY_BASE: u8 = 0x44;

// PCI Bridge Control bits (offset 0x3E, high 16 bits of 0x3C dword).
pub const CB_BRIDGE_CRST: u16 = 0x0040; // CardBus reset
pub const CB_BRIDGE_INTR: u16 = 0x0080; // 1 = ISA IRQ, 0 = PCI INTA
pub const CB_BRIDGE_PREFETCH0: u16 = 0x0100;
pub const CB_BRIDGE_PREFETCH1: u16 = 0x0200;
pub const CB_BRIDGE_POSTEN: u16 = 0x0400; // ToPIC95 WBEN (topic.h:108)
pub const CB_BRIDGE_ISAEN: u16 = 0x0004;
pub const CB_BRIDGE_VGAEN: u16 = 0x0008;

// Toshiba ToPIC95/97 config registers (topic.h:36-81).
const TOPIC_SOCKET_CONTROL: u8 = 0x90; // 32-bit
const TOPIC_SCR_IRQSEL: u32 = 0x0000_0001;
const TOPIC_SLOT_CARD_CTRL: u8 = 0xA0; // [7:0]=0xA0 Slot Ctrl, [15:8]=0xA1 Card/Int Ctrl, [31:24]=0xA3 Card Detect
const TOPIC_SLOT_SLOTON: u8 = 0x80;
const TOPIC_SLOT_SLOTEN: u8 = 0x40;
const TOPIC_CCR_INTA: u8 = 0x10;
const TOPIC97_ICR_IRQSEL: u8 = 0x01;
const TOPIC_CDR_MODE_PC32: u8 = 0x80;

// Standard CardBus Socket MMIO registers (at BAR0) and ExCA (at BAR0 + 0x800).
const CB_SOCKET_EVENT: usize = 0x00;
const CB_SOCKET_MASK: usize = 0x04;
const CB_SOCKET_STATE: usize = 0x08;
const CB_SOCKET_FORCE: usize = 0x0C;
const CB_SOCKET_CONTROL: usize = 0x10;
const EXCA_BASE: usize = 0x800;
const I365_POWER: usize = 0x02;
const I365_GENCTL: usize = 0x16;
const I365_GBLCTL: usize = 0x1E;

// CB_SOCKET_STATE / CB_SOCKET_FORCE bits (verified against Linux yenta_socket.h:20-52).
pub const CB_CDETECT1: u32 = 0x0000_0002;
pub const CB_CDETECT2: u32 = 0x0000_0004;
pub const CB_PWRCYCLE: u32 = 0x0000_0008;
pub const CB_CBCARD: u32 = 0x0000_0020;
pub const CB_BADVCCREQ: u32 = 0x0000_0200;
pub const CB_5VCARD: u32 = 0x0000_0400;
pub const CB_3VCARD: u32 = 0x0000_0800;
pub const CB_CVSTEST: u32 = 0x0000_4000;

// CB_SOCKET_CONTROL values (Vcc=3.3V, Vpp=0V).
pub const CB_SC_VCC_3V: u32 = 0x0000_0030;

// DynaBook SS 2000M non-overlapping MMIO assignments (disjoint from
// 0xF7EBF000 OHCI #1, 0xF7EFF000 82550, and 0xFC000000 Trident VGA).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SlotResources {
    pub sec_bus: u8,
    pub bar0_phys: u32,
    pub mem0_base: u32,
    pub mem0_limit: u32,
    pub irq_line: u8,
}

pub const fn slot_resources(slot: usize) -> SlotResources {
    if slot == 0 {
        SlotResources {
            sec_bus: 2,
            bar0_phys: 0xF7E0_0000,
            mem0_base: 0xF7D0_0000,
            mem0_limit: 0xF7DF_F000,
            irq_line: 11,
        }
    } else {
        SlotResources {
            sec_bus: 3,
            bar0_phys: 0xF7E0_1000,
            mem0_base: 0xF7C0_0000,
            mem0_limit: 0xF7CF_F000,
            irq_line: 11,
        }
    }
}

/// Compute the CardBus Bridge Control word (`0x3E`) for ToPIC95 bring-up.
/// Keeps MEM0/MEM1 non-prefetchable for register MMIO, routes interrupts to
/// PCI INTA (`CB_BRIDGE_INTR = 0`), clears ISA/VGA forwarding, and disables
/// `WBEN` (`0x0400`) on rev <= 7 per `topic.h:158`.
pub const fn topic95_bridge_control(cur: u16, rev: u8, assert_reset: bool) -> u16 {
    let mut b = cur
        & !(CB_BRIDGE_CRST
            | CB_BRIDGE_INTR
            | CB_BRIDGE_PREFETCH0
            | CB_BRIDGE_PREFETCH1
            | CB_BRIDGE_ISAEN
            | CB_BRIDGE_VGAEN);
    if rev <= 7 {
        b &= !CB_BRIDGE_POSTEN;
    }
    if assert_reset {
        b |= CB_BRIDGE_CRST;
    }
    b
}

/// Decoded CardBus socket state from `CB_SOCKET_STATE` (`BAR0 + 0x08`).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SocketDecode {
    pub card_present: bool,
    pub is_cardbus: bool,
    pub supports_3v3: bool,
    pub supports_5v: bool,
}

pub const fn decode_socket_state(state: u32) -> SocketDecode {
    SocketDecode {
        card_present: (state & (CB_CDETECT1 | CB_CDETECT2)) == 0,
        is_cardbus: (state & CB_CBCARD) != 0,
        supports_3v3: (state & CB_3VCARD) != 0,
        supports_5v: (state & CB_5VCARD) != 0,
    }
}

/// Fixed 40-byte `#[repr(C)]` slot summary returned to `CBINIT.COM` via JLM V86 API `AX=0x0009`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct CardBusSlotInfo {
    pub present: u8,
    pub stage: u8,
    pub rev: u8,
    pub sec_bus: u8,
    pub bar0_phys: u32,
    pub mem0_base: u32,
    pub socket_state: u32,
    pub card_present: u8,
    pub is_cardbus: u8,
    pub powered_3v3: u8,
    pub card_rev: u8,
    pub card_vid: u16,
    pub card_did: u16,
    pub card_subsys_vid: u16,
    pub card_subsys_did: u16,
    pub card_class: u32,
    pub card_bar0: u32,
    pub bar0_lin: u32,
}

impl CardBusSlotInfo {
    pub const ZERO: Self = Self {
        present: 0,
        stage: 0,
        rev: 0,
        sec_bus: 0,
        bar0_phys: 0,
        mem0_base: 0,
        socket_state: 0,
        card_present: 0,
        is_cardbus: 0,
        powered_3v3: 0,
        card_rev: 0,
        card_vid: 0,
        card_did: 0,
        card_subsys_vid: 0,
        card_subsys_did: 0,
        card_class: 0,
        card_bar0: 0,
        bar0_lin: 0,
    };
}

const _: () = {
    assert!(core::mem::size_of::<CardBusSlotInfo>() == 40);
};

struct CardBusState {
    slots: [CardBusSlotInfo; 2],
    bar0_lin_cache: [u32; 2],
    bridges_found: u32,
    cards_found: u32,
}

impl CardBusState {
    const fn new() -> Self {
        Self {
            slots: [CardBusSlotInfo::ZERO; 2],
            bar0_lin_cache: [0; 2],
            bridges_found: 0,
            cards_found: 0,
        }
    }
}

struct SingleCoreCellCb(UnsafeCell<CardBusState>);
// SAFETY: single-core DOS with CLI/STI discipline.
unsafe impl Sync for SingleCoreCellCb {}

static CB_STATE: SingleCoreCellCb = SingleCoreCellCb(UnsafeCell::new(CardBusState::new()));

fn cb_state() -> &'static mut CardBusState {
    unsafe { &mut *CB_STATE.0.get() }
}

pub fn cb_reset() {
    *cb_state() = CardBusState::new();
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

unsafe fn exca_w8(base: u32, reg: usize, val: u8) {
    unsafe {
        core::ptr::write_volatile((base as usize + EXCA_BASE + reg) as *mut u8, val);
        let _ = core::ptr::read_volatile((base as usize + EXCA_BASE + reg) as *const u8);
    }
}

/// Busy-wait `ms` milliseconds using the hardware PIT Channel 0 counter latch.
/// Safe in Ring 0 with `IF=0` (where BIOS tick `0040:006C` does not advance)
/// and bounded by an iteration cap so a stuck timer can never hang the machine.
#[cfg(target_arch = "x86")]
pub(crate) fn delay_ms(ms: u32) {
    unsafe fn pit0_read() -> u16 {
        unsafe {
            outb(0x43, 0x00); // latch Channel 0 count without altering mode/reload
            let lo = inb(0x40) as u16;
            let hi = inb(0x40) as u16;
            lo | (hi << 8)
        }
    }
    // PIT runs at 1,193,182 Hz (Mode 3 square-wave decrements by 2 per tick ->
    // ~2,386 counts/ms; Mode 2 rate-generator -> ~1,193 counts/ms).
    let target = ms.saturating_mul(2386);
    let max_iters = ms.saturating_mul(200_000).max(200_000);
    let mut prev = unsafe { pit0_read() };
    let mut elapsed: u32 = 0;
    let mut iters: u32 = 0;
    while elapsed < target && iters < max_iters {
        let cur = unsafe { pit0_read() };
        elapsed = elapsed.saturating_add(prev.wrapping_sub(cur) as u32);
        prev = cur;
        iters += 1;
    }
}

#[cfg(not(target_arch = "x86"))]
pub(crate) fn delay_ms(_ms: u32) {}

unsafe fn bring_up_slot(slot: usize) -> CardBusSlotInfo {
    let mut info = CardBusSlotInfo::ZERO;
    let func = slot as u8;
    let id = unsafe { pci_read32(0, TOPIC95_PCI_DEV, func, 0) };
    let vid = (id & 0xFFFF) as u16;
    let did = (id >> 16) as u16;
    if vid != TOPIC95_VID || did != TOPIC95_DID {
        return info;
    }

    let res = slot_resources(slot);
    let rev = (unsafe { pci_read32(0, TOPIC95_PCI_DEV, func, PCI_CLASS_REV) } & 0xFF) as u8;
    info.present = 1;
    info.stage = 1;
    info.rev = rev;
    info.sec_bus = res.sec_bus;
    info.bar0_phys = res.bar0_phys;
    info.mem0_base = res.mem0_base;

    unsafe {
        // 1. Standard CardBus bridge PCI configuration (yenta_config_init).
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_LEGACY_BASE, 0);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_BAR0, res.bar0_phys);
        let cls_lt = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_CLS_LT);
        pci_write32(
            0,
            TOPIC95_PCI_DEV,
            func,
            PCI_CLS_LT,
            (cls_lt & 0xFFFF_0000) | (168u32 << 8) | 0x08,
        );
        let bus_reg = (176u32 << 24) | ((res.sec_bus as u32) << 16) | ((res.sec_bus as u32) << 8);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_BUS_NUM, bus_reg);

        // Window 0 covers card MMIO; disable window 1 and I/O windows.
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_MEM0_BASE, res.mem0_base);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_MEM0_LIMIT, res.mem0_limit);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_MEM1_BASE, 0xFFFF_F000);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_MEM1_LIMIT, 0);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_IO0_BASE, 0x0000_FFFF);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_IO0_LIMIT, 0);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_IO1_BASE, 0x0000_FFFF);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_IO1_LIMIT, 0);

        // Route PCI IRQ line 11 and clear CardBus reset before CVSTEST interrogation
        // (matches Linux yenta_config_init; CRST is asserted during 3.3V power-up below).
        let irq_bc = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_IRQ_BCTRL);
        let cur_bc = (irq_bc >> 16) as u16;
        let new_bc = topic95_bridge_control(cur_bc, rev, false);
        let new_irq_bc = ((new_bc as u32) << 16) | (irq_bc & 0x0000_FF00) | (res.irq_line as u32);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_IRQ_BCTRL, new_irq_bc);

        // 2. Toshiba ToPIC95-specific registers (topic.h).
        let scr = pci_read32(0, TOPIC95_PCI_DEV, func, TOPIC_SOCKET_CONTROL);
        pci_write32(
            0,
            TOPIC95_PCI_DEV,
            func,
            TOPIC_SOCKET_CONTROL,
            scr | TOPIC_SCR_IRQSEL,
        );

        let a0_a3 = pci_read32(0, TOPIC95_PCI_DEV, func, TOPIC_SLOT_CARD_CTRL);
        let slot_ctrl = ((a0_a3 & 0xFF) as u8) | TOPIC_SLOT_SLOTON | TOPIC_SLOT_SLOTEN;
        // Preserve BIOS INTA (fn=0, 0x10) vs INTB (fn=1, 0x20) wiring and enable PCI IRQSEL.
        let int_pin_bit = if func == 0 { TOPIC_CCR_INTA } else { 0x20 };
        let card_ctrl = (((a0_a3 >> 8) & 0xFF) as u8 & !0x30) | int_pin_bit | TOPIC97_ICR_IRQSEL;
        let a2_byte = ((a0_a3 >> 16) & 0xFF) as u8;
        let card_det = (((a0_a3 >> 24) & 0xFF) as u8) | TOPIC_CDR_MODE_PC32;
        let new_a0_a3 = (slot_ctrl as u32)
            | ((card_ctrl as u32) << 8)
            | ((a2_byte as u32) << 16)
            | ((card_det as u32) << 24);
        pci_write32(0, TOPIC95_PCI_DEV, func, TOPIC_SLOT_CARD_CTRL, new_a0_a3);

        // Enable I/O, Memory, and Bus Master on the bridge (write 0 to upper 16-bit
        // W1C PCI Status register so latched abort flags are preserved).
        let cmd = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_CMD);
        pci_write32(
            0,
            TOPIC95_PCI_DEV,
            func,
            PCI_CMD,
            (cmd & 0xFFFF) | 0x0007,
        );
    }
    info.stage = 2;

    // 3. Map 4 KiB socket MMIO BAR0 uncached in Ring 0 (reusing any cached mapping
    //    so repeated power-down / reconnect cycles never leak Jemm PR_SYSTEM pages).
    let bar0_lin = {
        let cached = cb_state().bar0_lin_cache[slot];
        if cached != 0 {
            cached
        } else {
            let mapped = unsafe { jlm_map_mmio(res.bar0_phys, 1) };
            if mapped != 0 {
                cb_state().bar0_lin_cache[slot] = mapped;
            }
            mapped
        }
    };
    if bar0_lin == 0 {
        return info;
    }
    info.bar0_lin = bar0_lin;
    info.stage = 3;

    // 4. Reset ExCA/socket power (matches Linux yenta_sock_init), trigger Card
    //    Voltage Sense interrogation (CB_CVSTEST), and poll up to ~500 ms until
    //    either no card is present or voltage interrogation completes.
    //    If the socket was already powered (e.g. after a warm RELOAD or direct
    //    re-init), assert CRST, cut 3.3 V Vcc, and wait 200 ms for decoupling
    //    capacitors to discharge before re-interrogating.
    unsafe {
        let was_powered = mmio_r32(bar0_lin, CB_SOCKET_CONTROL) != 0;
        if was_powered {
            let irq_bc = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_IRQ_BCTRL);
            let cur_bc = (irq_bc >> 16) as u16;
            let rst_bc = topic95_bridge_control(cur_bc, rev, true);
            pci_write32(
                0,
                TOPIC95_PCI_DEV,
                func,
                PCI_IRQ_BCTRL,
                ((rst_bc as u32) << 16) | (irq_bc & 0x0000_FFFF),
            );
        }
        exca_w8(bar0_lin, I365_POWER, 0x00);
        exca_w8(bar0_lin, I365_GBLCTL, 0x00);
        exca_w8(bar0_lin, I365_GENCTL, 0x00);
        mmio_w32(bar0_lin, CB_SOCKET_CONTROL, 0);
        mmio_w32(bar0_lin, CB_SOCKET_EVENT, 0xFFFF_FFFF);
        mmio_w32(bar0_lin, CB_SOCKET_MASK, 0);
        if was_powered {
            delay_ms(200);
            let irq_bc = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_IRQ_BCTRL);
            let cur_bc = (irq_bc >> 16) as u16;
            let rel_bc = topic95_bridge_control(cur_bc, rev, false);
            pci_write32(
                0,
                TOPIC95_PCI_DEV,
                func,
                PCI_IRQ_BCTRL,
                ((rel_bc as u32) << 16) | (irq_bc & 0x0000_FFFF),
            );
        }
        mmio_w32(bar0_lin, CB_SOCKET_FORCE, CB_CVSTEST);
    }
    let mut state = 0u32;
    for attempt in 0..2 {
        if attempt == 1 {
            unsafe {
                mmio_w32(bar0_lin, CB_SOCKET_FORCE, CB_CVSTEST);
            }
        }
        for _ in 0..10 {
            delay_ms(25);
            state = unsafe { mmio_r32(bar0_lin, CB_SOCKET_STATE) };
            let cd_present = (state & (CB_CDETECT1 | CB_CDETECT2)) == 0;
            let vs_done = (state & (CB_5VCARD | CB_3VCARD)) != 0;
            if cd_present && vs_done {
                break;
            }
        }
        let cd_present = (state & (CB_CDETECT1 | CB_CDETECT2)) == 0;
        let vs_done = (state & (CB_5VCARD | CB_3VCARD)) != 0;
        if (cd_present && vs_done) || !cd_present {
            break;
        }
    }
    info.socket_state = state;
    let dec = decode_socket_state(state);
    info.card_present = u8::from(dec.card_present);
    info.is_cardbus = u8::from(dec.is_cardbus);

    if !dec.card_present || !dec.is_cardbus || !dec.supports_3v3 {
        return info;
    }

    // Assert CardBus reset, apply 3.3 V Vcc, poll CB_PWRCYCLE (without CB_BADVCCREQ)
    // up to 300 ms to confirm the power cycle completed, hold reset 100 ms, then release.
    unsafe {
        let irq_bc = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_IRQ_BCTRL);
        let cur_bc = (irq_bc >> 16) as u16;
        let new_bc = topic95_bridge_control(cur_bc, rev, true);
        let new_irq_bc = ((new_bc as u32) << 16) | (irq_bc & 0x0000_FFFF);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_IRQ_BCTRL, new_irq_bc);
        mmio_w32(bar0_lin, CB_SOCKET_CONTROL, CB_SC_VCC_3V);
    }
    let mut pwr_state = 0u32;
    let mut pwr_ok = false;
    for _ in 0..30 {
        delay_ms(10);
        pwr_state = unsafe { mmio_r32(bar0_lin, CB_SOCKET_STATE) };
        if (pwr_state & CB_BADVCCREQ) != 0 {
            break;
        }
        if (pwr_state & CB_PWRCYCLE) != 0 {
            pwr_ok = true;
            break;
        }
    }
    info.socket_state = pwr_state;
    if !pwr_ok {
        return info;
    }
    info.powered_3v3 = 1;
    delay_ms(100); // preserve 100 ms CRST hold time after Vcc stable

    // Release CardBus reset (CB_BRIDGE_CRST = 0) and let the card initialize.
    unsafe {
        let irq_bc = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_IRQ_BCTRL);
        let cur_bc = (irq_bc >> 16) as u16;
        let new_bc = topic95_bridge_control(cur_bc, rev, false);
        let new_irq_bc = ((new_bc as u32) << 16) | (irq_bc & 0x0000_FFFF);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_IRQ_BCTRL, new_irq_bc);
    }
    delay_ms(100);
    info.stage = 4;

    // 5. Enumerate child CardBus PCI device at (sec_bus, 0, 0).
    let card_id = unsafe { pci_read32(res.sec_bus, 0, 0, 0) };
    let cvid = (card_id & 0xFFFF) as u16;
    let cdid = (card_id >> 16) as u16;
    if cvid == 0xFFFF || cvid == 0x0000 {
        return info;
    }

    let cclass = unsafe { pci_read32(res.sec_bus, 0, 0, 0x08) };
    let csub = unsafe { pci_read32(res.sec_bus, 0, 0, 0x2C) };
    info.card_vid = cvid;
    info.card_did = cdid;
    info.card_rev = (cclass & 0xFF) as u8;
    info.card_class = cclass >> 8;
    info.card_subsys_vid = (csub & 0xFFFF) as u16;
    info.card_subsys_did = (csub >> 16) as u16;

    // Assign card BAR0 inside the bridge's MEM0 window, force PCI D0, set
    // cache-line/latency/retry-timeout per ath5k/pci.c, re-assert bridge MEM0
    // decoding after 3.3 V power-up, and enable Memory + Bus Master.
    unsafe {
        // Re-assert ToPIC95 bridge MEM0 window + Memory/BM enable after power-up
        // (write 0 to upper 16-bit W1C PCI Status so latched flags are preserved).
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_MEM0_BASE, res.mem0_base);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_MEM0_LIMIT, res.mem0_limit);
        let b_cmd = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_CMD);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CMD, (b_cmd & 0xFFFF) | 0x0007);

        // Force child CardBus device into PCI D0 via Power Management Cap (ID 0x01).
        let c_status = (pci_read32(res.sec_bus, 0, 0, PCI_CMD) >> 16) as u16;
        if (c_status & 0x0010) != 0 {
            let mut cap_ptr = (pci_read32(res.sec_bus, 0, 0, 0x34) & 0xFC) as u8;
            for _ in 0..16 {
                if cap_ptr < 0x40 {
                    break;
                }
                let cap_hdr = pci_read32(res.sec_bus, 0, 0, cap_ptr);
                let cap_id = (cap_hdr & 0xFF) as u8;
                if cap_id == 0x01 {
                    let pmcsr = pci_read32(res.sec_bus, 0, 0, cap_ptr + 4);
                    if (pmcsr & 0x03) != 0 {
                        pci_write32(res.sec_bus, 0, 0, cap_ptr + 4, pmcsr & !0x03);
                        delay_ms(10);
                    }
                    break;
                }
                cap_ptr = ((cap_hdr >> 8) & 0xFC) as u8;
            }
        }

        // Set cache-line size (8 dwords) and latency timer (0xA8) per ath5k/pci.c:216,223.
        let c_clslt = pci_read32(res.sec_bus, 0, 0, PCI_CLS_LT);
        pci_write32(
            res.sec_bus,
            0,
            0,
            PCI_CLS_LT,
            (c_clslt & 0xFFFF_0000) | 0x0000_A808,
        );
        // Disable PCI RETRY_TIMEOUT (byte 0x41 = 0) per ath5k/pci.c:232.
        let c_40 = pci_read32(res.sec_bus, 0, 0, 0x40);
        pci_write32(res.sec_bus, 0, 0, 0x40, c_40 & 0xFFFF_00FF);

        pci_write32(res.sec_bus, 0, 0, PCI_BAR0, res.mem0_base);
        let c_irq = pci_read32(res.sec_bus, 0, 0, 0x3C);
        pci_write32(
            res.sec_bus,
            0,
            0,
            0x3C,
            (c_irq & 0xFFFF_FF00) | (res.irq_line as u32),
        );
        let c_cmd = pci_read32(res.sec_bus, 0, 0, PCI_CMD);
        pci_write32(
            res.sec_bus,
            0,
            0,
            PCI_CMD,
            (c_cmd & 0xFFFF) | 0x0006,
        );
        delay_ms(10);
        info.card_bar0 = pci_read32(res.sec_bus, 0, 0, PCI_BAR0) & 0xFFFF_FFF0;
    }
    info.stage = 5;
    info
}

/// Return `true` if any CardBus slot currently has 3.3 V power applied.
pub(crate) fn any_slot_powered() -> bool {
    let st = cb_state();
    st.slots[0].powered_3v3 != 0 || st.slots[1].powered_3v3 != 0
}

/// Live ToPIC100 Slot 0 + child PCI config snapshot for `WPROBE /D`.
#[derive(Copy, Clone)]
pub(crate) struct SlotDiag {
    pub sock_state: u32,
    pub sock_ctrl: u32,
    pub br_cmd_stat: u32,
    pub br_sec_stat: u16,
    pub br_bctrl: u16,
    pub br_mem0_base: u32,
    pub br_mem0_lim: u32,
    pub topic_rcr: u32,
    pub ch_cmd_stat: u32,
    pub ch_bar0: u32,
    pub ch_pmcsr: u16,
}

/// Re-assert Stage-5 bridge MEM0 + child BAR0/PMCSR/CMD programming for `slot`
/// as a self-heal when an initial `AR5K_SREV` read returns `0xFFFFFFFF` (e.g.
/// after a late internal card reset / rail bounce). Preserves W1C status bits.
pub(crate) unsafe fn reassert_stage5(slot: u8) {
    let s = (slot as usize).min(1);
    let func = s as u8;
    let res = slot_resources(s);
    unsafe {
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_MEM0_BASE, res.mem0_base);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CB_MEM0_LIMIT, res.mem0_limit);
        let b_cmd = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_CMD);
        pci_write32(0, TOPIC95_PCI_DEV, func, PCI_CMD, (b_cmd & 0xFFFF) | 0x0007);

        let c_status = (pci_read32(res.sec_bus, 0, 0, PCI_CMD) >> 16) as u16;
        if c_status != 0xFFFF && (c_status & 0x0010) != 0 {
            let mut cap_ptr = (pci_read32(res.sec_bus, 0, 0, 0x34) & 0xFC) as u8;
            for _ in 0..16 {
                if cap_ptr < 0x40 {
                    break;
                }
                let cap_hdr = pci_read32(res.sec_bus, 0, 0, cap_ptr);
                if (cap_hdr & 0xFF) == 0x01 {
                    let pmcsr = pci_read32(res.sec_bus, 0, 0, cap_ptr + 4);
                    if (pmcsr & 0x03) != 0 {
                        pci_write32(res.sec_bus, 0, 0, cap_ptr + 4, pmcsr & !0x03);
                        delay_ms(10);
                    }
                    break;
                }
                cap_ptr = ((cap_hdr >> 8) & 0xFC) as u8;
            }
        }

        let c_clslt = pci_read32(res.sec_bus, 0, 0, PCI_CLS_LT);
        pci_write32(
            res.sec_bus,
            0,
            0,
            PCI_CLS_LT,
            (c_clslt & 0xFFFF_0000) | 0x0000_A808,
        );
        let c_40 = pci_read32(res.sec_bus, 0, 0, 0x40);
        pci_write32(res.sec_bus, 0, 0, 0x40, c_40 & 0xFFFF_00FF);
        pci_write32(res.sec_bus, 0, 0, PCI_BAR0, res.mem0_base);
        let c_cmd = pci_read32(res.sec_bus, 0, 0, PCI_CMD);
        pci_write32(res.sec_bus, 0, 0, PCI_CMD, (c_cmd & 0xFFFF) | 0x0006);
        delay_ms(10);
    }
}

/// Capture live bridge primary/secondary PCI status `(pri_stat, sec_stat)` for slot 0.
pub(crate) unsafe fn read_bridge_pci_status(slot: u8) -> (u16, u16) {
    unsafe {
        let pri = (pci_read32(0, TOPIC95_PCI_DEV, slot, PCI_CMD) >> 16) as u16;
        let sec = (pci_read32(0, TOPIC95_PCI_DEV, slot, 0x14) >> 16) as u16;
        (pri, sec)
    }
}

/// Capture a live non-destructive snapshot of CardBus Slot `slot` bridge + child registers.
pub(crate) unsafe fn read_slot_diag(slot: u8) -> SlotDiag {
    let s = (slot as usize).min(1);
    let func = s as u8;
    let res = slot_resources(s);
    let bar0_lin = cb_state().bar0_lin_cache[s];
    unsafe {
        let (sock_state, sock_ctrl) = if bar0_lin != 0 {
            (
                mmio_r32(bar0_lin, CB_SOCKET_STATE),
                mmio_r32(bar0_lin, CB_SOCKET_CONTROL),
            )
        } else {
            (0, 0)
        };
        let br_cmd_stat = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_CMD);
        let br_sec_stat = (pci_read32(0, TOPIC95_PCI_DEV, func, 0x14) >> 16) as u16;
        let br_bctrl = (pci_read32(0, TOPIC95_PCI_DEV, func, PCI_IRQ_BCTRL) >> 16) as u16;
        let br_mem0_base = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_CB_MEM0_BASE);
        let br_mem0_lim = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_CB_MEM0_LIMIT);
        let topic_rcr = pci_read32(0, TOPIC95_PCI_DEV, func, 0xA4);
        let ch_cmd_stat = pci_read32(res.sec_bus, 0, 0, PCI_CMD);
        let ch_bar0 = pci_read32(res.sec_bus, 0, 0, PCI_BAR0);
        let mut ch_pmcsr: u16 = 0xFFFF;
        if ch_cmd_stat != 0xFFFF_FFFF && ((ch_cmd_stat >> 16) & 0x0010) != 0 {
            let mut cap_ptr = (pci_read32(res.sec_bus, 0, 0, 0x34) & 0xFC) as u8;
            for _ in 0..16 {
                if cap_ptr < 0x40 {
                    break;
                }
                let cap_hdr = pci_read32(res.sec_bus, 0, 0, cap_ptr);
                if (cap_hdr & 0xFF) == 0x01 {
                    ch_pmcsr = (pci_read32(res.sec_bus, 0, 0, cap_ptr + 4) & 0xFFFF) as u16;
                    break;
                }
                cap_ptr = ((cap_hdr >> 8) & 0xFC) as u8;
            }
        }
        SlotDiag {
            sock_state,
            sock_ctrl,
            br_cmd_stat,
            br_sec_stat,
            br_bctrl,
            br_mem0_base,
            br_mem0_lim,
            topic_rcr,
            ch_cmd_stat,
            ch_bar0,
            ch_pmcsr,
        }
    }
}

/// Cleanly power down all brought-up ToPIC95 CardBus slots: disable child PCI
/// I/O+Memory+BusMaster, assert CardBus `CRST`, cut 3.3 V Vcc and ExCA power,
/// wait 250 ms for card decoupling capacitors to discharge, and clear slot state
/// so the next `find_card_bar0` / `WCONN` / `CBINIT` performs a fresh cold-rail
/// bring-up (and the card is safe to hot-eject).
pub(crate) unsafe fn power_down_slots() {
    let st = cb_state();
    let mut any_cut = false;
    for slot in 0..2 {
        let info = st.slots[slot];
        if info.stage < 3 || info.bar0_lin == 0 {
            continue;
        }
        let func = slot as u8;
        unsafe {
            if info.stage >= 5 && info.sec_bus != 0 {
                let c_cmd = pci_read32(info.sec_bus, 0, 0, PCI_CMD);
                if c_cmd != 0xFFFF_FFFF {
                    pci_write32(
                        info.sec_bus,
                        0,
                        0,
                        PCI_CMD,
                        (c_cmd & 0xFFFF_0000) | ((c_cmd & 0xFFFF) & !0x0007),
                    );
                }
            }
            let irq_bc = pci_read32(0, TOPIC95_PCI_DEV, func, PCI_IRQ_BCTRL);
            let cur_bc = (irq_bc >> 16) as u16;
            let rst_bc = topic95_bridge_control(cur_bc, info.rev, true);
            pci_write32(
                0,
                TOPIC95_PCI_DEV,
                func,
                PCI_IRQ_BCTRL,
                ((rst_bc as u32) << 16) | (irq_bc & 0x0000_FFFF),
            );
            exca_w8(info.bar0_lin, I365_POWER, 0x00);
            mmio_w32(info.bar0_lin, CB_SOCKET_CONTROL, 0);
            mmio_w32(info.bar0_lin, CB_SOCKET_MASK, 0);
            mmio_w32(info.bar0_lin, CB_SOCKET_EVENT, 0xFFFF_FFFF);
        }
        any_cut = true;
    }
    if any_cut {
        delay_ms(250);
    }
    st.slots = [CardBusSlotInfo::ZERO; 2];
    st.bridges_found = 0;
    st.cards_found = 0;
}

/// Look up an enumerated CardBus card by PCI vendor ID.
/// If CardBus bridges have not been initialized yet, runs `gate1_cb_init()` once.
/// Returns `Some((slot, card_did, card_bar0_phys))` on match.
pub(crate) fn find_card_bar0(vid: u16) -> Option<(u8, u16, u32)> {
    let st = cb_state();
    if st.bridges_found == 0 {
        unsafe {
            let _ = gate1_cb_init();
        }
    }
    let st = cb_state();
    for slot in 0..2 {
        let info = &st.slots[slot];
        if info.stage >= 5 && info.card_vid == vid && info.card_bar0 != 0 {
            return Some((slot as u8, info.card_did, info.card_bar0));
        }
    }
    None
}

/// FFI entry for JLM V86 API `AX=0x0008`: bring up both ToPIC95 CardBus bridges.
/// Returns `(cards_found << 16) | bridges_found`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_cb_init() -> u32 {
    let st = cb_state();
    st.bridges_found = 0;
    st.cards_found = 0;
    for slot in 0..2 {
        let info = unsafe { bring_up_slot(slot) };
        if info.present != 0 {
            st.bridges_found += 1;
        }
        if info.stage >= 5 {
            st.cards_found += 1;
        }
        st.slots[slot] = info;
    }
    (st.cards_found << 16) | (st.bridges_found & 0xFFFF)
}

/// FFI entry for JLM V86 API `AX=0x0009`: copy the 40-byte `CardBusSlotInfo`
/// for `slot` (`0` or `1`) into caller's flat linear address `dst_lin`.
/// Returns 1 on success, 0 if `slot >= 2`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_cb_slot_info(slot: u32, dst_lin: u32) -> u32 {
    if slot >= 2 || dst_lin == 0 {
        return 0;
    }
    let st = cb_state();
    let info = &st.slots[slot as usize];
    unsafe {
        core::ptr::copy_nonoverlapping(
            info as *const CardBusSlotInfo as *const u8,
            dst_lin as *mut u8,
            core::mem::size_of::<CardBusSlotInfo>(),
        );
    }
    1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn socket_state_decode_handles_empty_16bit_and_cardbus_3v3() {
        // Empty socket: CD1=1, CD2=1.
        let empty = decode_socket_state(CB_CDETECT1 | CB_CDETECT2);
        assert!(!empty.card_present);
        assert!(!empty.is_cardbus);

        // 3.3 V CardBus card inserted: CD1=0, CD2=0, CBCARD=1, 3VCARD=1.
        let cb33 = decode_socket_state(CB_CBCARD | CB_3VCARD);
        assert!(cb33.card_present);
        assert!(cb33.is_cardbus);
        assert!(cb33.supports_3v3);
        assert!(!cb33.supports_5v);

        // 16-bit 5 V card: CD1=0, CD2=0, CBCARD=0, 5VCARD=1.
        let pc16 = decode_socket_state(CB_5VCARD);
        assert!(pc16.card_present);
        assert!(!pc16.is_cardbus);
        assert!(!pc16.supports_3v3);
        assert!(pc16.supports_5v);
    }

    #[test]
    fn topic95_bridge_control_enforces_wben_rev_and_non_prefetch() {
        let initial = CB_BRIDGE_INTR | CB_BRIDGE_PREFETCH0 | CB_BRIDGE_POSTEN | CB_BRIDGE_VGAEN;
        // Rev 7: WBEN cleared, reset asserted.
        let r7 = topic95_bridge_control(initial, 7, true);
        assert_eq!(r7 & CB_BRIDGE_POSTEN, 0);
        assert_ne!(r7 & CB_BRIDGE_CRST, 0);
        assert_eq!(r7 & (CB_BRIDGE_INTR | CB_BRIDGE_PREFETCH0 | CB_BRIDGE_VGAEN), 0);

        // Rev 0x32 (DynaBook SS 2000M): WBEN preserved, reset released.
        let r32 = topic95_bridge_control(initial, 0x32, false);
        assert_ne!(r32 & CB_BRIDGE_POSTEN, 0);
        assert_eq!(r32 & CB_BRIDGE_CRST, 0);
        assert_eq!(r32 & (CB_BRIDGE_INTR | CB_BRIDGE_PREFETCH0 | CB_BRIDGE_VGAEN), 0);
    }

    #[test]
    fn slot_resources_are_aligned_and_disjoint_from_dynabook_mmio() {
        let s0 = slot_resources(0);
        let s1 = slot_resources(1);
        assert_ne!(s0.sec_bus, s1.sec_bus);
        assert_eq!(s0.bar0_phys & 0xFFF, 0);
        assert_eq!(s1.bar0_phys & 0xFFF, 0);
        assert_eq!(s0.mem0_base & 0xFFF, 0);
        assert_eq!(s1.mem0_base & 0xFFF, 0);
        assert!(s0.mem0_base < s0.mem0_limit);
        assert!(s1.mem0_base < s1.mem0_limit);
        // Must not overlap OHCI #1 (0xF7EBF000), Intel 82550 (0xF7EFF000), or VGA (0xFC000000).
        for phys in [0xF7EB_F000u32, 0xF7EF_F000u32, 0xFC00_0000u32] {
            assert!(phys < s0.mem0_base || phys > s0.mem0_limit);
            assert!(phys < s1.mem0_base || phys > s1.mem0_limit);
            assert_ne!(phys, s0.bar0_phys);
            assert_ne!(phys, s1.bar0_phys);
        }
    }
}
