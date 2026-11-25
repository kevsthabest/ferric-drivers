//! Gate 2.4 — Rust OHCI 1.0a USB Host Controller, Root Hub, and External USB Hub
//! Enumeration Driver (resident in `FERRIC.JLM`).
//!
//! Register offsets, HCCA/ED/TD layouts, and hub requests are verified against:
//! - `vendor/ref/specs/ohci-1.0a-spec.md`
//! - `vendor/ref/usbddos/USBDDOS/HCD/ohci.h` & `ohci.c`
//! - `vendor/ref/usbddos/USBDDOS/CLASS/hub.h` & `hub.c`
//!
//! Targets both real DynaBook SS 2000M silicon (NEC uPD720100A `1033:0035` at
//! `00:0C.0`, BAR0 `0xF7EBF000`, IRQ 11) and QEMU `-device pci-ohci` (`106B:003F`).
//! Strictly `#![no_std]`, 386-pure, zero heap, and read-only with respect to storage.

use core::cell::UnsafeCell;

use crate::{
    cardbus, fat_ro, inb, mouse_feed_usb, mouse_set_present, outb, pci_read32,
    pci_write32, vdmad_lock_dma_region_pci, vdmad_unlock_dma_region,
};

// ---------------------------------------------------------------------
// Verified OHCI 1.0a Operational Register Offsets (ohci.h:8-91)
// ---------------------------------------------------------------------

const HC_REVISION: usize = 0x00;
const HC_CONTROL: usize = 0x04;
const HC_COMMAND_STATUS: usize = 0x08;
const HC_INTERRUPT_STATUS: usize = 0x0C;
const HC_INTERRUPT_DISABLE: usize = 0x14;
const HC_HCCA: usize = 0x18;
const HC_CONTROL_HEAD_ED: usize = 0x20;
const HC_CONTROL_CURRENT_ED: usize = 0x24;
const HC_BULK_HEAD_ED: usize = 0x28;
const HC_BULK_CURRENT_ED: usize = 0x2C;
const HC_FM_INTERVAL: usize = 0x34;
const HC_PERIODIC_START: usize = 0x40;
const HC_RH_DESCRIPTOR_A: usize = 0x48;
const HC_RH_STATUS: usize = 0x50;
const HC_RH_PORT1_STATUS: usize = 0x54;
const HCE_CONTROL: usize = 0x100; // OHCI 1.0a Appendix B Legacy Support

// HcControl bits
const HC_CTRL_PLE: u32 = 1 << 2; // PeriodicListEnable
const HC_CTRL_CLE: u32 = 1 << 4; // ControlListEnable
const HC_CTRL_BLE: u32 = 1 << 5; // BulkListEnable
const HC_CTRL_HCFS_MASK: u32 = 3 << 6;
const HC_CTRL_HCFS_RESET: u32 = 0 << 6;
const HC_CTRL_HCFS_RESUME: u32 = 1 << 6;
const HC_CTRL_HCFS_OPERATIONAL: u32 = 2 << 6;
const HC_CTRL_IR: u32 = 1 << 8; // InterruptRouting (SMM ownership)

// HcCommandStatus bits
const HC_CS_HCR: u32 = 1 << 0; // HostControllerReset
const HC_CS_CLF: u32 = 1 << 1; // ControlListFilled
const HC_CS_BLF: u32 = 1 << 2; // BulkListFilled
const HC_CS_OCR: u32 = 1 << 3; // OwnershipChangeRequest

// HcRhDescriptorA / HcRhStatus / HcRhPortStatus bits
const RH_PSM: u32 = 1 << 8; // PowerSwitchingMode (1 = per-port)
const RH_LPSC: u32 = 1 << 16; // SetGlobalPower
const PORT_CCS: u32 = 1 << 0; // CurrentConnectStatus
const PORT_PES: u32 = 1 << 1; // PortEnableStatus
const PORT_PRS: u32 = 1 << 4; // PortResetStatus / SetPortReset
const PORT_PPS: u32 = 1 << 8; // PortPowerStatus / SetPortPower
const PORT_LSDA: u32 = 1 << 9; // LowSpeedDeviceAttached
const PORT_CSC: u32 = 1 << 16; // ConnectStatusChange
const PORT_PRSC: u32 = 1 << 20; // PortResetStatusChange

// OHCI FrameInterval default (OHCI 1.0a 7.3.1: 12000 - 1 = 0x2EDF)
const OHCI_FI_DEFAULT: u32 = 0x2EDF;

// OHCI TD PIDs & Condition Codes (ohci.h:94-111)
const TD_PID_SETUP: u32 = 0;
const TD_PID_OUT: u32 = 1;
const TD_PID_IN: u32 = 2;
const TD_TOGGLE_CARRY: u32 = 0;
const TD_TOGGLE_DATA0: u32 = 2;
const TD_TOGGLE_DATA1: u32 = 3;
const TD_NO_INT: u32 = 7;
const TD_CC_NO_ERROR: u32 = 0;
const TD_CC_STALL: u32 = 4;
const TD_CC_NOT_ACCESSED: u32 = 0xE;

// USB Hub Class constants (hub.h:5-67)
const USB_CLASS_HUB: u8 = 0x09;
const USB_DT_HUB: u8 = 0x29;
const HUB_FEAT_PORT_RESET: u16 = 4;
const HUB_FEAT_PORT_POWER: u16 = 8;
const HUB_FEAT_C_PORT_CONN: u16 = 16;
const HUB_FEAT_C_PORT_RESET: u16 = 20;
const HUB_PS_CONNECTION: u16 = 1 << 0;
const HUB_PS_ENABLE: u16 = 1 << 1;
const HUB_PS_RESET: u16 = 1 << 4;
const HUB_PS_LOW_SPEED: u16 = 1 << 9;

// 16 KiB DMA region layout (single contiguous DOS allocation, HCCA 256-byte aligned
// and guaranteed within a single 4 KiB physical page per OHCI 1.0a 3.1.1.1).
const USB_DMA_SIZE: usize = 16384;
const CTRL_ED_REL: usize = 0x0100;
const TD_SETUP_REL: usize = 0x0120;
const TD_DATA_REL: usize = 0x0140;
const TD_STAT_REL: usize = 0x0160;
const TD_DUMMY_REL: usize = 0x0180;
const SETUP_PKT_REL: usize = 0x0200;
const DATA_BUF_REL: usize = 0x0400;
const DATA_BUF_CAP: usize = 512;

// Periodic Interrupt IN EDs/TDs for live USB HID Boot Mouse & Boot Keyboard
// (all within the same single 4 KiB physical page as HCCA: 0x0000..0x06FF).
const INTR_ED_MOUSE_REL: usize = 0x0600;
const INTR_TD_MOUSE_REL: usize = 0x0620;
const INTR_TD_MOUSE_DUMMY_REL: usize = 0x0640;
const MOUSE_BUF_REL: usize = 0x0660;
const INTR_ED_KBD_REL: usize = 0x0680;
const INTR_TD_KBD_REL: usize = 0x06A0;
const INTR_TD_KBD_DUMMY_REL: usize = 0x06C0;
const KBD_BUF_REL: usize = 0x06E0;

// Polled Bulk ED/TD + CBW/CSW + 512B sector bounce buffer for Read-Only USB Mass Storage
// (still within the same single 4 KiB physical page as HCCA: 0x0000..0x09FF).
const BULK_ED_REL: usize = 0x0700;
const BULK_TD_REL: usize = 0x0720;
const BULK_TD_DUMMY_REL: usize = 0x0740;
const BULK_DATA_REL: usize = 0x0800;

// Periodic Interrupt IN ED/TD + report buffer for USB HID Gamepad / Joystick
// (within the same single 4 KiB physical page as HCCA: 0x0000..0x0AFF).
const INTR_ED_JOY_REL: usize = 0x0A00;
const INTR_TD_JOY_REL: usize = 0x0A20;
const INTR_TD_JOY_DUMMY_REL: usize = 0x0A40;
const JOY_BUF_REL: usize = 0x0A60;
const OHCI_PAGE_SPAN: u32 = 0x0B00;

// Configurable `usbdelay` multiplier bounds (default 1, clamped 1..=20) for slow USB flash drives.
pub const USBDELAY_MIN: u32 = 1;
pub const USBDELAY_MAX: u32 = 20;

/// Clamp a user-supplied `usbdelay` multiplier to the valid `1..=20` range.
pub const fn clamp_usb_delay(mult: u32) -> u32 {
    if mult < USBDELAY_MIN {
        USBDELAY_MIN
    } else if mult > USBDELAY_MAX {
        USBDELAY_MAX
    } else {
        mult
    }
}

/// Scale a base millisecond delay by the clamped `usbdelay` multiplier.
pub const fn scale_usb_delay_ms(base_ms: u32, mult: u32) -> u32 {
    base_ms.saturating_mul(clamp_usb_delay(mult))
}

/// Decode a USB HID mouse report into `(dx, dy, dz, buttons)`.
/// When `max_pkt >= 4` and at least 4 bytes are present, byte 3 is decoded as
/// the signed 8-bit scroll wheel delta (`dz`); otherwise `dz` is `0`.
pub fn decode_usb_mouse_report(buf: &[u8], max_pkt: u16) -> Option<(i32, i32, i32, u8)> {
    if buf.len() < 3 {
        return None;
    }
    let buttons = buf[0] & 0x07;
    let dx = (buf[1] as i8) as i32;
    let dy = (buf[2] as i8) as i32;
    let dz = if max_pkt >= 4 && buf.len() >= 4 {
        (buf[3] as i8) as i32
    } else {
        0
    };
    Some((dx, dy, dz, buttons))
}

// ---------------------------------------------------------------------
// Clean-Room PC Gameport (`0x201` + `INT 15h AH=84h`) Emulation
// ---------------------------------------------------------------------

/// PIT Channel 0 clock frequency in Hz (standard 8253/8254 oscillator).
pub const PIT_HZ: u32 = 1_193_182;

/// Decoded USB HID Gamepad / Joystick report (4..8+ byte standard reports).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct GamepadReport {
    /// Four 8-bit axes: `[X1, Y1, X2, Y2]` (`0..=255`, `128` = center).
    pub axes: [u8; 4],
    /// Bitmask of connected axes (bit 0 = X1, bit 1 = Y1, bit 2 = X2, bit 3 = Y2).
    pub axis_mask: u8,
    /// Active-high button state in bits `0..3` (buttons 1..4).
    pub buttons: u8,
}

/// Decode a standard 4-to-8-byte USB HID Gamepad / Joystick Interrupt IN report:
/// - 4-byte report: `[X1, Y1, buttons, ...]` (2 axes, `axis_mask = 0x03`).
/// - 5..8+ byte report: `[X1, Y1, X2, Y2, buttons, ...]` (4 axes, `axis_mask = 0x0F`).
pub fn decode_hid_gamepad_report(report: &[u8]) -> Option<GamepadReport> {
    if report.len() < 4 {
        return None;
    }
    if report.len() == 4 {
        Some(GamepadReport {
            axes: [report[0], report[1], 128, 128],
            axis_mask: 0x03,
            buttons: report[2] & 0x0F,
        })
    } else {
        Some(GamepadReport {
            axes: [report[0], report[1], report[2], report[3]],
            axis_mask: 0x0F,
            buttons: report[4] & 0x0F,
        })
    }
}

/// Standard IBM PC Gameport NE558 one-shot duration in microseconds:
/// `24 us + (axis * 1100) / 255` (`24 us` at 0, `576 us` at 128, `1124 us` at 255).
pub const fn gameport_axis_duration_us(axis: u8) -> u32 {
    24 + ((axis as u32) * 1100) / 255
}

/// Standard IBM PC Gameport one-shot duration in 1.193182 MHz PIT ticks.
/// Uses pure 32-bit integer arithmetic (`1124 * 1_193_182 + 500_000 < 2^31`), 386-pure.
pub const fn gameport_axis_duration_pit_ticks(axis: u8) -> u32 {
    (gameport_axis_duration_us(axis) * PIT_HZ + 500_000) / 1_000_000
}

/// Clean-room PC Gameport (`0x201`) + `INT 15h AH=84h` state machine.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct GameportState {
    pub connected: bool,
    pub axes: [u8; 4],
    pub axis_mask: u8,
    pub buttons: u8, // active-high bits 0..3
    pub triggered: bool,
    pub trigger_tick: u32,
    pub axis_durations: [u32; 4],
    pub last_pit_raw: u16,
    pub pit_accum: u32,
    pub pit_seeded: bool,
}

impl GameportState {
    pub const fn new() -> Self {
        Self {
            connected: false,
            axes: [128, 128, 128, 128],
            axis_mask: 0x03,
            buttons: 0,
            triggered: false,
            trigger_tick: 0,
            axis_durations: [0; 4],
            last_pit_raw: 0,
            pit_accum: 0,
            pit_seeded: false,
        }
    }

    pub fn reset(&mut self) {
        *self = Self::new();
    }

    pub fn set_connected(&mut self, connected: bool) {
        if connected {
            self.connected = true;
            self.axes = [128, 128, 128, 128];
            self.axis_mask = 0x03;
            self.buttons = 0;
            self.triggered = false;
        } else {
            self.reset();
        }
    }

    pub fn update_report(&mut self, rep: GamepadReport) {
        self.connected = true;
        self.axes = rep.axes;
        self.axis_mask = rep.axis_mask & 0x0F;
        self.buttons = rep.buttons & 0x0F;
    }

    /// Trigger all 4 axis one-shots on `OUT 0x201` at PIT tick `now_ticks`.
    pub fn write(&mut self, now_ticks: u32) {
        if !self.connected {
            return;
        }
        self.triggered = true;
        self.trigger_tick = now_ticks;
        let mut i = 0usize;
        while i < 4 {
            self.axis_durations[i] = gameport_axis_duration_pit_ticks(self.axes[i]);
            i += 1;
        }
    }

    /// Read port `0x201` at PIT tick `now_ticks`:
    /// - Disconnected stick returns `0xFF` (all axes open-circuit=1, all buttons released=1).
    /// - Connected stick returns:
    ///   - bits `4..7`: active-low buttons 1..4 (`(!buttons & 0x0F) << 4`)
    ///   - bits `0..3`: `1` while axis one-shot is charging (or axis disconnected), `0` once done.
    pub fn read(&mut self, now_ticks: u32) -> u8 {
        if !self.connected {
            return 0xFF;
        }
        let mut out: u8 = ((!self.buttons) & 0x0F) << 4;
        let elapsed = now_ticks.wrapping_sub(self.trigger_tick);
        let mut any_charging = false;
        let mut i = 0usize;
        while i < 4 {
            let bit = 1u8 << i;
            if (self.axis_mask & bit) == 0 {
                // Disconnected axis stays high (open circuit) indefinitely.
                out |= bit;
            } else if self.triggered && elapsed < self.axis_durations[i] {
                out |= bit;
                any_charging = true;
            }
            i += 1;
        }
        if self.triggered && !any_charging {
            self.triggered = false;
        }
        out
    }

    /// Advance the monotonic PIT tick accumulator from a raw 16-bit PIT Channel 0 read.
    pub fn sample_pit_raw(&mut self, raw: u16) -> u32 {
        if !self.pit_seeded {
            self.last_pit_raw = raw;
            self.pit_seeded = true;
            return self.pit_accum;
        }
        let delta = self.last_pit_raw.wrapping_sub(raw) as u32;
        self.last_pit_raw = raw;
        self.pit_accum = self.pit_accum.wrapping_add(delta);
        self.pit_accum
    }

    /// Active-low button byte for `INT 15h AH=84h DX=0000h` (`AL` bits 4..7).
    pub const fn bios_buttons_byte(&self) -> u8 {
        if !self.connected {
            return 0xF0;
        }
        ((!self.buttons) & 0x0F) << 4
    }

    /// Four 16-bit axis values `[AX, BX, CX, DX]` for `INT 15h AH=84h DX=0001h`.
    /// Disconnected axes return `0`.
    pub const fn bios_axes(&self) -> [u16; 4] {
        if !self.connected {
            return [0; 4];
        }
        [
            if (self.axis_mask & 0x01) != 0 { self.axes[0] as u16 } else { 0 },
            if (self.axis_mask & 0x02) != 0 { self.axes[1] as u16 } else { 0 },
            if (self.axis_mask & 0x04) != 0 { self.axes[2] as u16 } else { 0 },
            if (self.axis_mask & 0x08) != 0 { self.axes[3] as u16 } else { 0 },
        ]
    }
}

unsafe fn read_pit0_raw() -> u16 {
    unsafe {
        outb(0x43, 0x00); // latch Channel 0 counter
        let lo = inb(0x40) as u16;
        let hi = inb(0x40) as u16;
        lo | (hi << 8)
    }
}

// USB Mass Storage BOT / SCSI Transparent Command Set constants
// (verified against vendor/ref/usbddos/USBDDOS/CLASS/msc.h and SBC-3).
// Gate 3 enables Bulk-Out writes ONLY for this qualified USB MSC backend;
// BIOS INT 13h and ATA/IDE ports remain strictly read-only.
const USB_MSC_CBW_SIG: u32 = 0x4342_5355;
const USB_MSC_CSW_SIG: u32 = 0x5342_5355;
const SCSI_TEST_UNIT_READY: u8 = 0x00;
const SCSI_REQUEST_SENSE: u8 = 0x03;
const SCSI_INQUIRY: u8 = 0x12;
const SCSI_START_STOP_UNIT: u8 = 0x1B;
const SCSI_READ_CAPACITY10: u8 = 0x25;
const SCSI_READ10: u8 = 0x28;
const SCSI_WRITE10: u8 = 0x2A;
const SCSI_SYNCHRONIZE_CACHE10: u8 = 0x35;

const SCSI_SENSE_NOT_READY: u8 = 0x02;
const SCSI_SENSE_ILLEGAL_REQ: u8 = 0x05;
const SCSI_SENSE_UNIT_ATTENTION: u8 = 0x06;

/// Bulk-Only Transport data-stage direction.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum BotDirection {
    None,
    DataIn,
    DataOut,
}

/// Classification of a SCSI `TEST UNIT READY` + `REQUEST SENSE` probe.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum TurOutcome {
    Ready,
    MediaChanged,
    MediaRemoved,
    Error,
}

/// Result of a Gate 3 USB MSC media-check probe.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum MediaCheckResult {
    Unchanged(u16),
    Changed(u16),
    NotReady,
}

/// Build a 31-byte USB Mass Storage Bulk-Only Transport Command Block Wrapper (CBW).
pub fn build_msc_cbw(
    tag: u32,
    data_len: u32,
    dir: BotDirection,
    cdb: &[u8],
) -> Option<[u8; 31]> {
    if cdb.is_empty() || cdb.len() > 16 {
        return None;
    }
    if dir == BotDirection::None && data_len != 0 {
        return None;
    }
    let mut cbw = [0u8; 31];
    cbw[0..4].copy_from_slice(&USB_MSC_CBW_SIG.to_le_bytes());
    cbw[4..8].copy_from_slice(&tag.to_le_bytes());
    cbw[8..12].copy_from_slice(&data_len.to_le_bytes());
    cbw[12] = match dir {
        BotDirection::DataIn if data_len > 0 => 0x80,
        _ => 0x00,
    };
    cbw[13] = 0; // LUN 0
    cbw[14] = cdb.len() as u8;
    cbw[15..15 + cdb.len()].copy_from_slice(cdb);
    Some(cbw)
}

/// Validate LBA range and 512-byte sector alignment using checked 64-bit arithmetic
/// (Gate 3 large-drive contract).
pub const fn validate_msc_lba_and_len(
    lba: u32,
    sector_count: u16,
    total_sectors: u32,
    sector_size: u32,
    buf_len: usize,
) -> bool {
    if sector_size != 512 || total_sectors == 0 || sector_count == 0 {
        return false;
    }
    let expected_bytes = (sector_count as usize) * 512;
    if buf_len != expected_bytes {
        return false;
    }
    let end_lba = (lba as u64) + (sector_count as u64);
    end_lba <= (total_sectors as u64)
}

/// Pack a 10-byte SCSI `READ(10)` (`0x28`) CDB (386-pure, avoids `BSWAP`).
pub fn build_scsi_read10_cdb(lba: u32, blocks: u16) -> [u8; 10] {
    [
        SCSI_READ10,
        0,
        ((core::hint::black_box(lba) >> 24) & 0xFF) as u8,
        ((lba >> 16) & 0xFF) as u8,
        ((lba >> 8) & 0xFF) as u8,
        (lba & 0xFF) as u8,
        0,
        ((blocks >> 8) & 0xFF) as u8,
        (blocks & 0xFF) as u8,
        0,
    ]
}

/// Pack a 10-byte SCSI `WRITE(10)` (`0x2A`) CDB (386-pure, avoids `BSWAP`).
pub fn build_scsi_write10_cdb(lba: u32, blocks: u16) -> [u8; 10] {
    [
        SCSI_WRITE10,
        0,
        ((core::hint::black_box(lba) >> 24) & 0xFF) as u8,
        ((lba >> 16) & 0xFF) as u8,
        ((lba >> 8) & 0xFF) as u8,
        (lba & 0xFF) as u8,
        0,
        ((blocks >> 8) & 0xFF) as u8,
        (blocks & 0xFF) as u8,
        0,
    ]
}

/// Pack a 10-byte SCSI `SYNCHRONIZE CACHE(10)` (`0x35`) CDB (LBA=0, blocks=0 => flush all).
pub const fn build_scsi_sync_cache10_cdb() -> [u8; 10] {
    [SCSI_SYNCHRONIZE_CACHE10, 0, 0, 0, 0, 0, 0, 0, 0, 0]
}

/// Pack a 6-byte SCSI `START STOP UNIT` (`0x1B`) CDB.
/// Safe eject sets `eject=true, start=false` (`LoEj=1, Start=0` => byte 4 = `0x02`).
pub const fn build_scsi_start_stop_unit_cdb(eject: bool, start: bool) -> [u8; 6] {
    let flags = ((eject as u8) << 1) | (start as u8);
    [SCSI_START_STOP_UNIT, 0, 0, 0, flags, 0]
}

/// Pack a 6-byte SCSI `TEST UNIT READY` (`0x00`) CDB.
pub const fn build_scsi_test_unit_ready_cdb() -> [u8; 6] {
    [SCSI_TEST_UNIT_READY, 0, 0, 0, 0, 0]
}

/// Classify a `TEST UNIT READY` result + `REQUEST SENSE` key/ASC into a media-check outcome.
pub const fn classify_tur_sense(tur_ok: bool, sense_key: u8, _asc: u8) -> TurOutcome {
    if tur_ok {
        return TurOutcome::Ready;
    }
    match sense_key & 0x0F {
        SCSI_SENSE_UNIT_ATTENTION => TurOutcome::MediaChanged,
        SCSI_SENSE_NOT_READY => TurOutcome::MediaRemoved,
        _ => TurOutcome::Error,
    }
}

pub const MAX_USB_DEVICES: usize = 16;

/// Build OHCI Endpoint Descriptor word 0 (`ControlFlags`, ohci.h:193-206).
pub const fn encode_ohci_ed_flags(
    func_addr: u8,
    endpoint: u8,
    low_speed: bool,
    skip: bool,
    max_pkt: u16,
) -> u32 {
    ((func_addr as u32) & 0x7F)
        | (((endpoint as u32) & 0x0F) << 7)
        | (if low_speed { 1u32 << 13 } else { 0 })
        | (if skip { 1u32 << 14 } else { 0 })
        | (((max_pkt as u32) & 0x07FF) << 16)
}

/// Build OHCI General Transfer Descriptor word 0 (`ControlFlags`, ohci.h:134-145).
pub const fn encode_ohci_td_flags(
    rounding: bool,
    pid: u32,
    delay_int: u32,
    toggle: u32,
    cc: u32,
) -> u32 {
    (if rounding { 1u32 << 18 } else { 0 })
        | ((pid & 0x03) << 19)
        | ((delay_int & 0x07) << 21)
        | ((toggle & 0x03) << 24)
        | ((cc & 0x0F) << 28)
}

/// Parsed primary interface + composite HID Boot Mouse / Boot Keyboard endpoints
/// from a USB Configuration descriptor.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ParsedConfig {
    pub config_val: u8,
    pub if_class: u8,
    pub if_sub: u8,
    pub if_proto: u8,
    pub int_ep: u8,
    pub int_maxpkt: u16,
    pub int_interval: u8,
    pub kbd_if: u8,
    pub kbd_ep: u8,
    pub kbd_maxpkt: u16,
    pub mouse_if: u8,
    pub mouse_ep: u8,
    pub mouse_maxpkt: u16,
    pub joy_if: u8,
    pub joy_ep: u8,
    pub joy_maxpkt: u16,
    pub msc_if: u8,
    pub msc_ep_in: u8,
    pub msc_ep_out: u8,
    pub msc_maxpkt: u16,
}

/// Parse a USB Configuration descriptor buffer (starting with the 9-byte config
/// header) across ALL interfaces to extract `bConfigurationValue`, the primary
/// Interface's class/sub/proto + Interrupt IN endpoint, AND dedicated HID Boot
/// Keyboard (`03/01/01`), HID Boot Mouse (`03/01/02`), and HID Gamepad/Joystick
/// (`03/00/00`) endpoints on standalone or composite devices.
pub fn parse_usb_config_desc(buf: &[u8]) -> Option<ParsedConfig> {
    if buf.len() < 9 || buf[0] < 9 || buf[1] != 2 {
        return None;
    }
    let total_len = (u16::from_le_bytes([buf[2], buf[3]]) as usize).min(buf.len());
    let mut out = ParsedConfig {
        config_val: buf[5],
        if_class: 0,
        if_sub: 0,
        if_proto: 0,
        int_ep: 0,
        int_maxpkt: 0,
        int_interval: 0,
        kbd_if: 0,
        kbd_ep: 0,
        kbd_maxpkt: 0,
        mouse_if: 0,
        mouse_ep: 0,
        mouse_maxpkt: 0,
        joy_if: 0,
        joy_ep: 0,
        joy_maxpkt: 0,
        msc_if: 0,
        msc_ep_in: 0,
        msc_ep_out: 0,
        msc_maxpkt: 0,
    };
    let mut off = buf[0] as usize;
    let mut have_primary = false;
    let mut cur_if_num: u8 = 0;
    let mut cur_if_class: u8 = 0;
    let mut cur_if_sub: u8 = 0;
    let mut cur_if_proto: u8 = 0;
    let mut in_primary = false;
    while off + 2 <= total_len {
        let dlen = buf[off] as usize;
        let dtype = buf[off + 1];
        if dlen < 2 || off + dlen > total_len {
            break;
        }
        if dtype == 4 && dlen >= 9 {
            // Interface descriptor
            cur_if_num = buf[off + 2];
            cur_if_class = buf[off + 5];
            cur_if_sub = buf[off + 6];
            cur_if_proto = buf[off + 7];
            if !have_primary {
                out.if_class = cur_if_class;
                out.if_sub = cur_if_sub;
                out.if_proto = cur_if_proto;
                have_primary = true;
                in_primary = true;
            } else {
                in_primary = false;
            }
        } else if dtype == 5 && dlen >= 7 && have_primary {
            // Endpoint descriptor: Interrupt IN (HID) or Bulk IN/OUT (Mass Storage BOT)
            let ep_addr = buf[off + 2];
            let bm_attr = buf[off + 3];
            let ep = ep_addr & 0x0F;
            let mp = u16::from_le_bytes([buf[off + 4], buf[off + 5]]) & 0x07FF;
            let iv = buf[off + 6];
            if (ep_addr & 0x80) != 0 && (bm_attr & 0x03) == 3 {
                if in_primary && out.int_ep == 0 {
                    out.int_ep = ep;
                    out.int_maxpkt = mp;
                    out.int_interval = iv;
                }
                if cur_if_class == 0x03 && cur_if_sub == 0x01 && cur_if_proto == 0x01 && out.kbd_ep == 0 {
                    out.kbd_if = cur_if_num;
                    out.kbd_ep = ep;
                    out.kbd_maxpkt = mp;
                } else if cur_if_class == 0x03 && cur_if_sub == 0x01 && cur_if_proto == 0x02 && out.mouse_ep == 0 {
                    out.mouse_if = cur_if_num;
                    out.mouse_ep = ep;
                    out.mouse_maxpkt = mp;
                } else if cur_if_class == 0x03 && cur_if_sub == 0x00 && cur_if_proto == 0x00 && out.joy_ep == 0 {
                    out.joy_if = cur_if_num;
                    out.joy_ep = ep;
                    out.joy_maxpkt = mp;
                }
            } else if (bm_attr & 0x03) == 2 && cur_if_class == 0x08 && cur_if_proto == 0x50 {
                // USB Mass Storage Bulk-Only Transport (class 0x08, protocol 0x50)
                out.msc_if = cur_if_num;
                if (ep_addr & 0x80) != 0 && out.msc_ep_in == 0 {
                    out.msc_ep_in = ep;
                    out.msc_maxpkt = mp;
                } else if (ep_addr & 0x80) == 0 && out.msc_ep_out == 0 {
                    out.msc_ep_out = ep;
                    if out.msc_maxpkt == 0 {
                        out.msc_maxpkt = mp;
                    }
                }
            }
        }
        off += dlen;
    }
    Some(out)
}

/// Translate a USB HID Boot Keyboard Usage ID (`0x04..0x52`) + modifier byte
/// into a PC BIOS `(scancode << 8) | ascii` word for the BDA keyboard buffer.
pub const fn hid_usage_to_bios_key(usage: u8, mods: u8) -> Option<u16> {
    let shift = (mods & 0x22) != 0; // Left or Right Shift
    let ctrl = (mods & 0x11) != 0; // Left or Right Ctrl

    // Letters a..z (HID 0x04..=0x1D)
    if usage >= 0x04 && usage <= 0x1D {
        const LETTER_SC: [u8; 26] = [
            0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32,
            0x31, 0x18, 0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C,
        ];
        let idx = (usage - 0x04) as usize;
        let sc = LETTER_SC[idx] as u16;
        let asc = if ctrl {
            (idx + 1) as u16
        } else if shift {
            (b'A' + idx as u8) as u16
        } else {
            (b'a' + idx as u8) as u16
        };
        return Some((sc << 8) | asc);
    }

    // Digits 1..9, 0 (HID 0x1E..=0x27)
    if usage >= 0x1E && usage <= 0x27 {
        const DIG_LO: [u8; 10] = *b"1234567890";
        const DIG_HI: [u8; 10] = *b"!@#$%^&*()";
        let idx = (usage - 0x1E) as usize;
        let sc = 0x02 + idx as u16;
        let asc = if shift { DIG_HI[idx] } else { DIG_LO[idx] } as u16;
        return Some((sc << 8) | asc);
    }

    // Function keys F1..F10 (HID 0x3A..=0x43)
    if usage >= 0x3A && usage <= 0x43 {
        let sc = 0x3B + (usage - 0x3A) as u16;
        return Some(sc << 8);
    }

    match usage {
        0x28 => Some(0x1C0D), // Enter
        0x29 => Some(0x011B), // Escape
        0x2A => Some(0x0E08), // Backspace
        0x2B => Some(0x0F09), // Tab
        0x2C => Some(0x3920), // Space
        0x2D => Some((0x0C << 8) | if shift { b'_' } else { b'-' } as u16),
        0x2E => Some((0x0D << 8) | if shift { b'+' } else { b'=' } as u16),
        0x2F => Some((0x1A << 8) | if shift { b'{' } else { b'[' } as u16),
        0x30 => Some((0x1B << 8) | if shift { b'}' } else { b']' } as u16),
        0x31 => Some((0x2B << 8) | if shift { b'|' } else { b'\\' } as u16),
        0x33 => Some((0x27 << 8) | if shift { b':' } else { b';' } as u16),
        0x34 => Some((0x28 << 8) | if shift { b'"' } else { b'\'' } as u16),
        0x35 => Some((0x29 << 8) | if shift { b'~' } else { b'`' } as u16),
        0x36 => Some((0x33 << 8) | if shift { b'<' } else { b',' } as u16),
        0x37 => Some((0x34 << 8) | if shift { b'>' } else { b'.' } as u16),
        0x38 => Some((0x35 << 8) | if shift { b'?' } else { b'/' } as u16),
        0x49 => Some(0x5200), // Insert
        0x4A => Some(0x4700), // Home
        0x4B => Some(0x4900), // Page Up
        0x4C => Some(0x5300), // Delete
        0x4D => Some(0x4F00), // End
        0x4E => Some(0x5100), // Page Down
        0x4F => Some(0x4D00), // Right Arrow
        0x50 => Some(0x4B00), // Left Arrow
        0x51 => Some(0x5000), // Down Arrow
        0x52 => Some(0x4800), // Up Arrow
        _ => None,
    }
}

/// Push a BIOS `(scancode << 8) | ascii` key word into the standard PC BIOS Data
/// Area circular keyboard buffer (`0040:001A` head, `0040:001C` tail, `0040:001E..003D`).
/// Zero `INT 21h`/`INT 16h` calls — safe from Ring-0 timer hooks.
#[cfg(target_arch = "x86")]
unsafe fn push_bda_key(word: u16) {
    unsafe {
        let head = core::ptr::read_volatile(0x041A as *const u16);
        let tail = core::ptr::read_volatile(0x041C as *const u16);
        let buf_start = core::ptr::read_volatile(0x0480 as *const u16);
        let buf_end = core::ptr::read_volatile(0x0482 as *const u16);
        let start = if buf_start >= 0x1E && buf_start < 0x3E { buf_start } else { 0x1E };
        let end = if buf_end > start && buf_end <= 0x3E { buf_end } else { 0x3E };
        if tail < start || tail >= end || (tail & 1) != 0 {
            return;
        }
        let mut next = tail + 2;
        if next >= end {
            next = start;
        }
        if next == head {
            return; // buffer full
        }
        core::ptr::write_volatile((0x0400usize + tail as usize) as *mut u16, word);
        core::ptr::write_volatile(0x041C as *mut u16, next);
    }
}

#[cfg(not(target_arch = "x86"))]
unsafe fn push_bda_key(_word: u16) {}

/// Fixed 24-byte `#[repr(C)]` enumerated USB device record returned to `USBPROBE.COM`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct UsbDeviceInfo {
    pub addr: u8,
    pub parent_addr: u8,
    pub port: u8,
    pub speed: u8, // 0 = Full-Speed, 1 = Low-Speed
    pub vid: u16,
    pub pid: u16,
    pub dev_class: u8,
    pub dev_sub: u8,
    pub dev_proto: u8,
    pub if_class: u8,
    pub if_sub: u8,
    pub if_proto: u8,
    pub max_pkt0: u8,
    pub num_ports: u8, // >0 for USB hubs
    pub int_ep: u8,
    pub int_interval: u8,
    pub int_maxpkt: u16,
    pub config_val: u8,
    pub pad: [u8; 3],
}

impl UsbDeviceInfo {
    pub const ZERO: Self = Self {
        addr: 0,
        parent_addr: 0,
        port: 0,
        speed: 0,
        vid: 0,
        pid: 0,
        dev_class: 0,
        dev_sub: 0,
        dev_proto: 0,
        if_class: 0,
        if_sub: 0,
        if_proto: 0,
        max_pkt0: 0,
        num_ports: 0,
        int_ep: 0,
        int_interval: 0,
        int_maxpkt: 0,
        config_val: 0,
        pad: [0; 3],
    };
}

/// Fixed 416-byte `#[repr(C)]` OHCI controller + device tree summary for `USBPROBE.COM`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct UsbSummary {
    pub present: u8,
    pub stage: u8,
    pub pci_bus: u8,
    pub pci_dev_fn: u8,
    pub pci_vid: u16,
    pub pci_did: u16,
    pub bar0_phys: u32,
    pub hc_rev: u32,
    pub num_ports: u8,
    pub dev_count: u8,
    pub hub_count: u8,
    pub irq_line: u8,
    pub rh_desc_a: u32,
    pub reserved: [u32; 2],
    pub devices: [UsbDeviceInfo; MAX_USB_DEVICES],
}

impl UsbSummary {
    pub const ZERO: Self = Self {
        present: 0,
        stage: 0,
        pci_bus: 0,
        pci_dev_fn: 0,
        pci_vid: 0,
        pci_did: 0,
        bar0_phys: 0,
        hc_rev: 0,
        num_ports: 0,
        dev_count: 0,
        hub_count: 0,
        irq_line: 0,
        rh_desc_a: 0,
        reserved: [0; 2],
        devices: [UsbDeviceInfo::ZERO; MAX_USB_DEVICES],
    };
}

const _: () = {
    assert!(core::mem::size_of::<UsbDeviceInfo>() == 24);
    assert!(core::mem::size_of::<UsbSummary>() == 32 + 24 * MAX_USB_DEVICES);
};

#[derive(Copy, Clone)]
struct UsbHidEndpoint {
    active: bool,
    addr: u8,
    low_speed: bool,
    if_num: u8,
    ep: u8,
    max_pkt: u16,
    max_pkt0: u16,
    reports: u32,
    prev_keys: [u8; 6],
}

impl UsbHidEndpoint {
    const ZERO: Self = Self {
        active: false,
        addr: 0,
        low_speed: false,
        if_num: 0,
        ep: 0,
        max_pkt: 0,
        max_pkt0: 8,
        reports: 0,
        prev_keys: [0; 6],
    };
}

struct UsbMscState {
    active: bool,
    ready: bool,
    ejected: bool,
    media_changed: bool,
    via_ehci: bool,
    addr: u8,
    if_num: u8,
    ep_in: u8,
    ep_out: u8,
    max_pkt: u16,
    max_pkt0: u16,
    toggle_in: u8,
    toggle_out: u8,
    tag: u32,
    total_sectors: u32,
    sector_size: u32,
    vendor: [u8; 8],
    product: [u8; 16],
    vol: Option<fat_ro::FatVolume>,
    // Stage F lightweight signature flags for non-FAT partitions on the
    // medium (supports both single-partition and dual-partition drives,
    // e.g. FAT16 + exFAT). Full parsers live in standalone EXFAT.JLM / NTFS.JLM.
    has_exfat: bool,
    has_ntfs: bool,
    bpb_raw: [u8; 25], // raw 25-byte DOS BPB (VBR bytes 11..36)
    media_gen: u16,
    writes_count: u32,
    // Sequential read cursor for gate1_usb_msc_open / gate1_usb_msc_read_chunk
    file_open: bool,
    file_cluster: u32,
    file_sec_in_clus: u8,
    file_remaining: u32,
}

impl UsbMscState {
    const ZERO: Self = Self {
        active: false,
        ready: false,
        ejected: false,
        media_changed: false,
        via_ehci: false,
        addr: 0,
        if_num: 0,
        ep_in: 0,
        ep_out: 0,
        max_pkt: 64,
        max_pkt0: 8,
        toggle_in: 0,
        toggle_out: 0,
        tag: 1,
        total_sectors: 0,
        sector_size: 512,
        vendor: [b' '; 8],
        product: [b' '; 16],
        vol: None,
        has_exfat: false,
        has_ntfs: false,
        bpb_raw: [0; 25],
        media_gen: 0,
        writes_count: 0,
        file_open: false,
        file_cluster: 0,
        file_sec_in_clus: 0,
        file_remaining: 0,
    };

    fn bump_media_gen(&mut self) -> u16 {
        self.media_gen = self.media_gen.wrapping_add(1).max(1);
        self.media_gen
    }

    fn apply_tur_outcome(&mut self, outcome: TurOutcome) -> MediaCheckResult {
        if !self.active || !self.ready || self.ejected {
            return MediaCheckResult::NotReady;
        }
        match outcome {
            TurOutcome::Ready => {
                if self.media_changed {
                    MediaCheckResult::Changed(self.media_gen)
                } else {
                    MediaCheckResult::Unchanged(self.media_gen)
                }
            }
            TurOutcome::MediaChanged => {
                self.media_changed = true;
                self.file_open = false;
                let g = self.bump_media_gen();
                MediaCheckResult::Changed(g)
            }
            TurOutcome::MediaRemoved | TurOutcome::Error => {
                self.ready = false;
                self.media_changed = true;
                self.vol = None;
                self.has_exfat = false;
                self.has_ntfs = false;
                self.file_open = false;
                let _ = self.bump_media_gen();
                MediaCheckResult::NotReady
            }
        }
    }

    fn mark_ejected(&mut self) {
        self.ready = false;
        self.ejected = true;
        self.media_changed = true;
        self.vol = None;
        self.has_exfat = false;
        self.has_ntfs = false;
        self.file_open = false;
        let _ = self.bump_media_gen();
    }
}

struct OhciState {
    mapped_phys: u32,
    mapped_lin: u32,
    dma_lin: u32,
    dma_phys: u32,
    base_off: usize, // offset inside dma block where 4 KiB-contained 256B-aligned HCCA starts
    next_addr: u8,
    hid_active: bool,
    // Re-entrancy guard for gate1_usb_poll: that entry point is reached from
    // both the INT 08h timer hook and the INT 33h dispatch, and the second
    // context can interrupt the first in the middle of consuming a completed
    // TD. Without the guard the inner run can observe the outer run's
    // half-updated TD/buffer state and feed a zeroed mouse buffer as a real
    // report. A plain bool is enough because DOS here is single-core and the
    // two contexts never truly run in parallel, they only nest.
    poll_active: bool,
    mouse: UsbHidEndpoint,
    mouse_wheel_dz: i32,
    kbd: UsbHidEndpoint,
    joy: UsbHidEndpoint,
    gameport: GameportState,
    usb_delay_mult: u32,
    msc: UsbMscState,
    summary: UsbSummary,
}

impl OhciState {
    const fn new() -> Self {
        Self {
            mapped_phys: 0,
            mapped_lin: 0,
            dma_lin: 0,
            dma_phys: 0,
            base_off: 0,
            next_addr: 1,
            hid_active: false,
            poll_active: false,
            mouse: UsbHidEndpoint::ZERO,
            mouse_wheel_dz: 0,
            kbd: UsbHidEndpoint::ZERO,
            joy: UsbHidEndpoint::ZERO,
            gameport: GameportState::new(),
            usb_delay_mult: USBDELAY_MIN,
            msc: UsbMscState::ZERO,
            summary: UsbSummary::ZERO,
        }
    }
}

struct SingleCoreCellOhci(UnsafeCell<OhciState>);
// SAFETY: single-core DOS with CLI/STI discipline.
unsafe impl Sync for SingleCoreCellOhci {}

static OHCI_STATE: SingleCoreCellOhci = SingleCoreCellOhci(UnsafeCell::new(OhciState::new()));

fn ohci_state() -> &'static mut OhciState {
    unsafe { &mut *OHCI_STATE.0.get() }
}

pub fn ohci_reset() {
    *ohci_state() = OhciState::new();
}

#[inline]
fn usb_delay_ms(base_ms: u32) {
    let mult = ohci_state().usb_delay_mult;
    cardbus::delay_ms(scale_usb_delay_ms(base_ms, mult));
}

#[inline]
fn usb_delay_mult() -> u32 {
    clamp_usb_delay(ohci_state().usb_delay_mult)
}

/// Stage H consults this before the EHCI bring-up claims port routing:
/// a companion OHCI can only receive the full/low-speed devices the
/// EHCI hands over if it is actually up (controller found, MMIO
/// mapped, root hub live). Read-only query; changes no OHCI state.
pub(crate) fn ohci_running() -> bool {
    let st = ohci_state();
    st.summary.present != 0 && st.mapped_lin != 0 && st.summary.stage >= 5
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

unsafe fn dma_r32(lin: u32, off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((lin as usize + off) as *const u32) }
}

unsafe fn dma_w32(lin: u32, off: usize, val: u32) {
    unsafe { core::ptr::write_volatile((lin as usize + off) as *mut u32, val) }
}

/// Find the first enabled OHCI host controller on PCI Bus 0 (class 0x0C, subclass 0x03,
/// prog-if 0x10) with a non-zero MMIO BAR0.
unsafe fn find_ohci_pci() -> Option<(u8, u8, u8, u16, u16, u32, u8)> {
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
            if class == 0x0C && subclass == 0x03 && progif == 0x10 {
                let bar0 = unsafe { pci_read32(0, dev, func, 0x10) } & 0xFFFF_F000;
                if bar0 == 0 || bar0 == 0xFFFF_F000 {
                    continue;
                }
                let irq = (unsafe { pci_read32(0, dev, func, 0x3C) } & 0xFF) as u8;
                return Some((0, dev, func, vid, did, bar0, irq));
            }
        }
    }
    None
}

/// Execute a synchronous polled USB Control Transfer over OHCI Control List.
/// Returns `Some(bytes_transferred)` on `OHCI_CC_NO_ERROR`, or `None` on error/timeout.
unsafe fn ohci_control_xfer(
    base: u32,
    dma_lin: u32,
    dma_phys: u32,
    addr: u8,
    low_speed: bool,
    max_pkt: u16,
    setup: &[u8; 8],
    data_in: Option<&mut [u8]>,
) -> Option<usize> {
    let mp = if max_pkt == 0 { 8 } else { max_pkt };
    let is_in = (setup[0] & 0x80) != 0;
    let data_len = match &data_in {
        Some(buf) => buf.len().min(DATA_BUF_CAP),
        None => 0,
    };

    // Copy 8-byte SETUP packet into DMA buffer.
    unsafe {
        core::ptr::copy_nonoverlapping(
            setup.as_ptr(),
            (dma_lin as usize + SETUP_PKT_REL) as *mut u8,
            8,
        );
    }

    let ed_phys = dma_phys + CTRL_ED_REL as u32;
    let td_setup_phys = dma_phys + TD_SETUP_REL as u32;
    let td_data_phys = dma_phys + TD_DATA_REL as u32;
    let td_stat_phys = dma_phys + TD_STAT_REL as u32;
    let td_dummy_phys = dma_phys + TD_DUMMY_REL as u32;
    let setup_phys = dma_phys + SETUP_PKT_REL as u32;
    let data_phys = dma_phys + DATA_BUF_REL as u32;

    // 1. Dummy tail TD.
    unsafe {
        dma_w32(dma_lin, TD_DUMMY_REL, 0);
        dma_w32(dma_lin, TD_DUMMY_REL + 4, 0);
        dma_w32(dma_lin, TD_DUMMY_REL + 8, 0);
        dma_w32(dma_lin, TD_DUMMY_REL + 12, 0);
    }

    // 2. SETUP TD (DATA0, 8 bytes).
    let next_after_setup = if data_len > 0 {
        td_data_phys
    } else {
        td_stat_phys
    };
    unsafe {
        dma_w32(
            dma_lin,
            TD_SETUP_REL,
            encode_ohci_td_flags(false, TD_PID_SETUP, TD_NO_INT, TD_TOGGLE_DATA0, TD_CC_NOT_ACCESSED),
        );
        dma_w32(dma_lin, TD_SETUP_REL + 4, setup_phys);
        dma_w32(dma_lin, TD_SETUP_REL + 8, next_after_setup);
        dma_w32(dma_lin, TD_SETUP_REL + 12, setup_phys + 7);
    }

    // 3. Optional DATA TD (DATA1, rounding enabled for short IN packets).
    if data_len > 0 {
        let pid = if is_in { TD_PID_IN } else { TD_PID_OUT };
        unsafe {
            dma_w32(
                dma_lin,
                TD_DATA_REL,
                encode_ohci_td_flags(is_in, pid, TD_NO_INT, TD_TOGGLE_DATA1, TD_CC_NOT_ACCESSED),
            );
            dma_w32(dma_lin, TD_DATA_REL + 4, data_phys);
            dma_w32(dma_lin, TD_DATA_REL + 8, td_stat_phys);
            dma_w32(dma_lin, TD_DATA_REL + 12, data_phys + (data_len as u32) - 1);
        }
    }

    // 4. STATUS TD (DATA1, opposite direction of DATA stage, 0-length).
    let stat_pid = if data_len > 0 && is_in {
        TD_PID_OUT
    } else {
        TD_PID_IN
    };
    unsafe {
        dma_w32(
            dma_lin,
            TD_STAT_REL,
            encode_ohci_td_flags(false, stat_pid, TD_NO_INT, TD_TOGGLE_DATA1, TD_CC_NOT_ACCESSED),
        );
        dma_w32(dma_lin, TD_STAT_REL + 4, 0);
        dma_w32(dma_lin, TD_STAT_REL + 8, td_dummy_phys);
        dma_w32(dma_lin, TD_STAT_REL + 12, 0);
    }

    // 5. Endpoint Descriptor pointing HeadP=TD_SETUP, TailP=TD_DUMMY.
    unsafe {
        dma_w32(
            dma_lin,
            CTRL_ED_REL,
            encode_ohci_ed_flags(addr, 0, low_speed, false, mp),
        );
        dma_w32(dma_lin, CTRL_ED_REL + 4, td_dummy_phys); // TailP
        dma_w32(dma_lin, CTRL_ED_REL + 8, td_setup_phys); // HeadP
        dma_w32(dma_lin, CTRL_ED_REL + 12, 0); // NextED
    }

    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);

    // 6. Submit to OHCI Control List and trigger ControlListFilled.
    unsafe {
        mmio_w32(base, HC_CONTROL_CURRENT_ED, 0);
        mmio_w32(base, HC_CONTROL_HEAD_ED, ed_phys);
        mmio_w32(base, HC_COMMAND_STATUS, HC_CS_CLF);
    }

    // 7. Poll up to 250 * usbdelay ms for STATUS TD completion or ED Halt.
    let mut ok = false;
    let poll_limit = scale_usb_delay_ms(250, usb_delay_mult());
    for _ in 0..poll_limit {
        let headp = unsafe { dma_r32(dma_lin, CTRL_ED_REL + 8) };
        let stat_cc = (unsafe { dma_r32(dma_lin, TD_STAT_REL) } >> 28) & 0x0F;
        if stat_cc != TD_CC_NOT_ACCESSED {
            ok = stat_cc == TD_CC_NO_ERROR;
            break;
        }
        if (headp & 1) != 0 {
            // ED halted on SETUP or DATA error.
            break;
        }
        if (headp & !0x0F) == td_dummy_phys {
            ok = true;
            break;
        }
        cardbus::delay_ms(1);
    }

    // 8. Unlink ED cleanly and wait 2 ms for OHCI frame boundary.
    unsafe {
        dma_w32(
            dma_lin,
            CTRL_ED_REL,
            encode_ohci_ed_flags(addr, 0, low_speed, true, mp),
        );
        mmio_w32(base, HC_CONTROL_HEAD_ED, 0);
        mmio_w32(base, HC_CONTROL_CURRENT_ED, 0);
        // Clear WDH/SOF status bits if set.
        let isr = mmio_r32(base, HC_INTERRUPT_STATUS);
        if isr != 0 {
            mmio_w32(base, HC_INTERRUPT_STATUS, isr);
        }
    }
    cardbus::delay_ms(2);

    if !ok {
        return None;
    }

    if let Some(buf) = data_in {
        if data_len > 0 {
            let cbp = unsafe { dma_r32(dma_lin, TD_DATA_REL + 4) };
            let xferred = if cbp == 0 {
                data_len
            } else {
                cbp.saturating_sub(data_phys) as usize
            }
            .min(data_len);
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (dma_lin as usize + DATA_BUF_REL) as *const u8,
                    buf.as_mut_ptr(),
                    xferred,
                );
            }
            return Some(xferred);
        }
    }
    Some(0)
}

/// Enumerate a single reset & enabled USB device (at address 0), assign it a new
/// address, read Device + Configuration descriptors, and call `SET_CONFIGURATION`.
/// Returns the index in `summary.devices` on success.
unsafe fn enumerate_device(
    base: u32,
    dma_lin: u32,
    dma_phys: u32,
    parent_addr: u8,
    port: u8,
    low_speed: bool,
) -> Option<usize> {
    let st = ohci_state();
    let idx = st.summary.dev_count as usize;
    if idx >= MAX_USB_DEVICES || st.next_addr >= 127 {
        return None;
    }

    // 1. GET_DESCRIPTOR(Device, 8 bytes) at address 0 to learn bMaxPacketSize0.
    let mut dev_desc = [0u8; 18];
    let setup_dev8 = [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x08, 0x00];
    let n8 = unsafe {
        ohci_control_xfer(
            base,
            dma_lin,
            dma_phys,
            0,
            low_speed,
            8,
            &setup_dev8,
            Some(&mut dev_desc[..8]),
        )
    }?;
    if n8 < 8 || dev_desc[1] != 1 {
        return None;
    }
    let max_pkt0 = if dev_desc[7] >= 8 { dev_desc[7] } else { 8 };

    // 2. SET_ADDRESS(new_addr) + 5 ms settle.
    let new_addr = st.next_addr;
    let setup_addr = [0x00, 0x05, new_addr, 0x00, 0x00, 0x00, 0x00, 0x00];
    unsafe {
        ohci_control_xfer(base, dma_lin, dma_phys, 0, low_speed, max_pkt0 as u16, &setup_addr, None)
    }?;
    st.next_addr = new_addr + 1;
    usb_delay_ms(5);

    // 3. GET_DESCRIPTOR(Device, 18 bytes) at new_addr.
    let setup_dev18 = [0x80, 0x06, 0x00, 0x01, 0x00, 0x00, 0x12, 0x00];
    let n18 = unsafe {
        ohci_control_xfer(
            base,
            dma_lin,
            dma_phys,
            new_addr,
            low_speed,
            max_pkt0 as u16,
            &setup_dev18,
            Some(&mut dev_desc),
        )
    }?;
    if n18 < 18 {
        return None;
    }

    let mut info = UsbDeviceInfo::ZERO;
    info.addr = new_addr;
    info.parent_addr = parent_addr;
    info.port = port;
    info.speed = u8::from(low_speed);
    info.vid = u16::from_le_bytes([dev_desc[8], dev_desc[9]]);
    info.pid = u16::from_le_bytes([dev_desc[10], dev_desc[11]]);
    info.dev_class = dev_desc[4];
    info.dev_sub = dev_desc[5];
    info.dev_proto = dev_desc[6];
    info.max_pkt0 = max_pkt0;

    // 4. GET_DESCRIPTOR(Configuration 0): 9-byte header then full descriptor (up to 256 B).
    let mut cfg_buf = [0u8; 256];
    let setup_cfg9 = [0x80, 0x06, 0x00, 0x02, 0x00, 0x00, 0x09, 0x00];
    if let Some(nc) = unsafe {
        ohci_control_xfer(
            base,
            dma_lin,
            dma_phys,
            new_addr,
            low_speed,
            max_pkt0 as u16,
            &setup_cfg9,
            Some(&mut cfg_buf[..9]),
        )
    } {
        if nc >= 9 {
            let total = (u16::from_le_bytes([cfg_buf[2], cfg_buf[3]]) as usize).min(cfg_buf.len());
            let setup_cfg_full = [
                0x80,
                0x06,
                0x00,
                0x02,
                0x00,
                0x00,
                (total & 0xFF) as u8,
                (total >> 8) as u8,
            ];
            let got = unsafe {
                ohci_control_xfer(
                    base,
                    dma_lin,
                    dma_phys,
                    new_addr,
                    low_speed,
                    max_pkt0 as u16,
                    &setup_cfg_full,
                    Some(&mut cfg_buf[..total]),
                )
            }
            .unwrap_or(nc);
            if let Some(parsed) = parse_usb_config_desc(&cfg_buf[..got]) {
                info.config_val = parsed.config_val;
                info.if_class = parsed.if_class;
                info.if_sub = parsed.if_sub;
                info.if_proto = parsed.if_proto;
                info.int_ep = parsed.int_ep;
                info.int_maxpkt = parsed.int_maxpkt;
                info.int_interval = parsed.int_interval;
                // pad[0] carries composite HID flags: bit 0 = Boot Keyboard, bit 1 = Boot Mouse, bit 2 = HID Gamepad.
                let is_standalone_joy = parsed.joy_ep != 0 && parsed.kbd_ep == 0 && parsed.mouse_ep == 0;
                info.pad[0] = u8::from(parsed.kbd_ep != 0)
                    | (u8::from(parsed.mouse_ep != 0) << 1)
                    | (u8::from(is_standalone_joy) << 2);

                // 5. SET_CONFIGURATION(config_val).
                let setup_setcfg = [0x00, 0x09, parsed.config_val, 0x00, 0x00, 0x00, 0x00, 0x00];
                let _ = unsafe {
                    ohci_control_xfer(
                        base,
                        dma_lin,
                        dma_phys,
                        new_addr,
                        low_speed,
                        max_pkt0 as u16,
                        &setup_setcfg,
                        None,
                    )
                };
                usb_delay_ms(5);

                // Bind first discovered HID Boot Keyboard, HID Boot Mouse, and HID Gamepad endpoints
                // (works for both standalone devices and composite wireless dongles).
                if parsed.kbd_ep != 0 && !st.kbd.active {
                    st.kbd = UsbHidEndpoint {
                        active: true,
                        addr: new_addr,
                        low_speed,
                        if_num: parsed.kbd_if,
                        ep: parsed.kbd_ep,
                        max_pkt: if parsed.kbd_maxpkt >= 8 { parsed.kbd_maxpkt.min(16) } else { 8 },
                        max_pkt0: max_pkt0 as u16,
                        reports: 0,
                        prev_keys: [0; 6],
                    };
                }
                if parsed.mouse_ep != 0 && !st.mouse.active {
                    st.mouse = UsbHidEndpoint {
                        active: true,
                        addr: new_addr,
                        low_speed,
                        if_num: parsed.mouse_if,
                        ep: parsed.mouse_ep,
                        max_pkt: if parsed.mouse_maxpkt >= 3 { parsed.mouse_maxpkt.min(16) } else { 4 },
                        max_pkt0: max_pkt0 as u16,
                        reports: 0,
                        prev_keys: [0; 6],
                    };
                }
                if is_standalone_joy && !st.joy.active {
                    st.joy = UsbHidEndpoint {
                        active: true,
                        addr: new_addr,
                        low_speed,
                        if_num: parsed.joy_if,
                        ep: parsed.joy_ep,
                        max_pkt: if parsed.joy_maxpkt >= 4 { parsed.joy_maxpkt.min(32) } else { 8 },
                        max_pkt0: max_pkt0 as u16,
                        reports: 0,
                        prev_keys: [0; 6],
                    };
                }
                if parsed.msc_ep_in != 0 && parsed.msc_ep_out != 0 && !st.msc.active {
                    st.msc = UsbMscState::ZERO;
                    st.msc.active = true;
                    st.msc.addr = new_addr;
                    st.msc.if_num = parsed.msc_if;
                    st.msc.ep_in = parsed.msc_ep_in;
                    st.msc.ep_out = parsed.msc_ep_out;
                    st.msc.max_pkt = if parsed.msc_maxpkt >= 8 { parsed.msc_maxpkt.min(64) } else { 64 };
                    st.msc.max_pkt0 = max_pkt0 as u16;
                }
            }
        }
    }

    st.summary.devices[idx] = info;
    st.summary.dev_count += 1;
    Some(idx)
}

/// Enumerate downstream ports of an external USB Hub (`dev_class == 0x09`).
unsafe fn enumerate_hub_ports(base: u32, dma_lin: u32, dma_phys: u32, hub_idx: usize) {
    let (hub_addr, hub_ls, hub_mp) = {
        let d = &ohci_state().summary.devices[hub_idx];
        (d.addr, d.speed != 0, d.max_pkt0 as u16)
    };

    // 1. GET_DESCRIPTOR(USB_DT_HUB = 0x29, class request 0xA0).
    let mut hub_desc = [0u8; 9];
    let setup_hdesc = [0xA0, 0x06, 0x00, USB_DT_HUB, 0x00, 0x00, 0x09, 0x00];
    let Some(nh) = (unsafe {
        ohci_control_xfer(
            base,
            dma_lin,
            dma_phys,
            hub_addr,
            hub_ls,
            hub_mp,
            &setup_hdesc,
            Some(&mut hub_desc),
        )
    }) else {
        return;
    };
    if nh < 7 || hub_desc[1] != USB_DT_HUB {
        return;
    }
    let nbr_ports = hub_desc[2].min(15);
    let pwr_good_ms = ((hub_desc[5] as u32) * 2).max(50);
    ohci_state().summary.devices[hub_idx].num_ports = nbr_ports;
    ohci_state().summary.hub_count += 1;

    // 2. Power on all downstream hub ports (SET_FEATURE PORT_POWER = 8).
    for p in 1..=nbr_ports {
        let setup_pwr = [
            0x23,
            0x03,
            (HUB_FEAT_PORT_POWER & 0xFF) as u8,
            0,
            p,
            0,
            0,
            0,
        ];
        let _ = unsafe {
            ohci_control_xfer(base, dma_lin, dma_phys, hub_addr, hub_ls, hub_mp, &setup_pwr, None)
        };
    }
    usb_delay_ms(pwr_good_ms + 50);

    // 3. Probe each downstream port, reset attached devices, and enumerate.
    for p in 1..=nbr_ports {
        let mut ps_buf = [0u8; 4];
        let setup_getps = [0xA3, 0x00, 0x00, 0x00, p, 0, 0x04, 0x00];
        let Some(ns) = (unsafe {
            ohci_control_xfer(
                base,
                dma_lin,
                dma_phys,
                hub_addr,
                hub_ls,
                hub_mp,
                &setup_getps,
                Some(&mut ps_buf),
            )
        }) else {
            continue;
        };
        if ns < 4 {
            continue;
        }
        let status = u16::from_le_bytes([ps_buf[0], ps_buf[1]]);
        if (status & HUB_PS_CONNECTION) == 0 {
            continue;
        }

        // Clear C_PORT_CONNECTION if latched.
        let setup_clr_c = [0x23, 0x01, HUB_FEAT_C_PORT_CONN as u8, 0, p, 0, 0, 0];
        let _ = unsafe {
            ohci_control_xfer(base, dma_lin, dma_phys, hub_addr, hub_ls, hub_mp, &setup_clr_c, None)
        };

        // SET_FEATURE(PORT_RESET = 4) and poll until PS_RESET self-clears.
        let setup_rst = [0x23, 0x03, HUB_FEAT_PORT_RESET as u8, 0, p, 0, 0, 0];
        if unsafe {
            ohci_control_xfer(base, dma_lin, dma_phys, hub_addr, hub_ls, hub_mp, &setup_rst, None)
        }
        .is_none()
        {
            continue;
        }
        usb_delay_ms(55);
        let mut cur_status = 0u16;
        for _ in 0..40 {
            if let Some(n) = unsafe {
                ohci_control_xfer(
                    base,
                    dma_lin,
                    dma_phys,
                    hub_addr,
                    hub_ls,
                    hub_mp,
                    &setup_getps,
                    Some(&mut ps_buf),
                )
            } {
                if n >= 4 {
                    cur_status = u16::from_le_bytes([ps_buf[0], ps_buf[1]]);
                    if (cur_status & HUB_PS_RESET) == 0 {
                        break;
                    }
                }
            }
            usb_delay_ms(5);
        }

        // CLEAR_FEATURE(C_PORT_RESET = 20) per USB 2.0 11.24.2 Table 11-17.
        let setup_clr_r = [0x23, 0x01, HUB_FEAT_C_PORT_RESET as u8, 0, p, 0, 0, 0];
        let _ = unsafe {
            ohci_control_xfer(base, dma_lin, dma_phys, hub_addr, hub_ls, hub_mp, &setup_clr_r, None)
        };
        usb_delay_ms(15);

        if (cur_status & HUB_PS_ENABLE) == 0 {
            continue;
        }
        let child_ls = (cur_status & HUB_PS_LOW_SPEED) != 0;
        let _ = unsafe { enumerate_device(base, dma_lin, dma_phys, hub_addr, p, child_ls) };
    }
}

/// FFI entry for JLM V86 API `AX=0x0010` (`USBPROBE.COM`): discover the primary
/// OHCI USB host controller, take ownership from BIOS/SMM, reset and initialize
/// HCCA + Root Hub, enumerate all attached root-hub and external-hub devices, and
/// copy the 416-byte `UsbSummary` into `dst_lin`. Returns `dev_count` (or 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_usb_init(dma_seg: u32, dst_lin: u32) -> u32 {
    // Shut down any prior resident periodic HID polling before re-initializing.
    unsafe {
        let _ = gate1_usb_shutdown();
    }
    let st = ohci_state();
    st.summary = UsbSummary::ZERO;
    st.next_addr = 1;
    st.hid_active = false;
    st.mouse = UsbHidEndpoint::ZERO;
    st.mouse_wheel_dz = 0;
    st.kbd = UsbHidEndpoint::ZERO;
    st.joy = UsbHidEndpoint::ZERO;
    st.gameport.reset();
    st.msc = UsbMscState::ZERO;

    let Some((bus, dev, func, vid, did, bar0_phys, irq)) = (unsafe { find_ohci_pci() }) else {
        if dst_lin != 0 {
            unsafe {
                core::ptr::copy_nonoverlapping(
                    &st.summary as *const UsbSummary as *const u8,
                    dst_lin as *mut u8,
                    core::mem::size_of::<UsbSummary>(),
                );
            }
        }
        return 0;
    };

    st.summary.present = 1;
    st.summary.stage = 1;
    st.summary.pci_bus = bus;
    st.summary.pci_dev_fn = (dev << 3) | (func & 7);
    st.summary.pci_vid = vid;
    st.summary.pci_did = did;
    st.summary.bar0_phys = bar0_phys;
    st.summary.irq_line = irq;

    // 1. Enable PCI Memory Space + Bus Master (preserve W1C PCI Status bits).
    unsafe {
        let cmd = pci_read32(bus, dev, func, 0x04);
        pci_write32(bus, dev, func, 0x04, ((cmd & 0xFFFF) | 0x0006) & !0x0400);
    }

    // 2. Map 4 KiB OHCI operational MMIO BAR0 uncached in Ring 0 (cached across runs).
    let base = if st.mapped_phys == bar0_phys && st.mapped_lin != 0 {
        st.mapped_lin
    } else {
        let lin = unsafe { cardbus::map_mmio(bar0_phys, 1) };
        if lin != 0 {
            st.mapped_phys = bar0_phys;
            st.mapped_lin = lin;
        }
        lin
    };
    if base == 0 {
        return 0;
    }
    st.summary.stage = 2;
    st.summary.hc_rev = unsafe { mmio_r32(base, HC_REVISION) };

    // 3. Lock caller's 16 KiB DMA block via VDMAD (PCI bus-master, DL=0) and
    //    choose a 4 KiB-contained, 256-byte-aligned base inside it for HCCA + EDs + TDs.
    let raw_lin = dma_seg << 4;
    let Some(raw_phys) = (unsafe { vdmad_lock_dma_region_pci(raw_lin, USB_DMA_SIZE as u32) }) else {
        return 0;
    };
    let mut off = ((256 - (raw_phys & 0xFF)) & 0xFF) as usize;
    // Ensure HCCA..KBD_BUF (0x700 bytes) does not cross a 4 KiB physical page boundary.
    if ((raw_phys + off as u32) & !0xFFF) != ((raw_phys + off as u32 + OHCI_PAGE_SPAN - 1) & !0xFFF) {
        off = (4096 - (raw_phys & 0xFFF)) as usize;
    }
    let dma_lin = raw_lin + off as u32;
    let dma_phys = raw_phys + off as u32;
    st.dma_lin = raw_lin;
    st.dma_phys = raw_phys;
    st.base_off = off;

    // Zero the 3 KiB HCCA/ED/TD/buffer area.
    unsafe {
        core::ptr::write_bytes(dma_lin as *mut u8, 0, 3072);
    }

    // 4. OHCI ownership handoff (BIOS / SMM) + disable Legacy Support emulation.
    unsafe {
        let ctrl = mmio_r32(base, HC_CONTROL);
        if (ctrl & HC_CTRL_IR) != 0 {
            mmio_w32(base, HC_COMMAND_STATUS, HC_CS_OCR);
            for _ in 0..20 {
                if (mmio_r32(base, HC_CONTROL) & HC_CTRL_IR) == 0 {
                    break;
                }
                cardbus::delay_ms(50);
            }
        } else {
            let hcfs = ctrl & HC_CTRL_HCFS_MASK;
            if hcfs != HC_CTRL_HCFS_RESET && hcfs != HC_CTRL_HCFS_OPERATIONAL {
                mmio_w32(base, HC_CONTROL, (ctrl & !HC_CTRL_HCFS_MASK) | HC_CTRL_HCFS_RESUME);
                cardbus::delay_ms(30);
            } else if hcfs == HC_CTRL_HCFS_RESET {
                cardbus::delay_ms(20);
            }
        }
        // Disable Legacy Support (OHCI 1.0a Appendix B; RAZ on non-legacy controllers).
        mmio_w32(base, HCE_CONTROL, 0);
        // Disable all OHCI interrupts for clean polled enumeration.
        mmio_w32(base, HC_INTERRUPT_DISABLE, 0xC000_007F);
    }

    // 5. Save HcFmInterval (skipping ALi M5237 10B9:5237 lockup quirk), reset HC.
    let skip_fi = vid == 0x10B9 && did == 0x5237;
    let saved_fi = if skip_fi {
        OHCI_FI_DEFAULT
    } else {
        let raw = unsafe { mmio_r32(base, HC_FM_INTERVAL) } & 0x3FFF;
        if raw == 0 { OHCI_FI_DEFAULT } else { raw }
    };
    let fsmps = ((saved_fi.saturating_sub(210)) * 6) / 7;
    let fi_reg = (fsmps << 16) | (saved_fi & 0x3FFF);

    unsafe {
        mmio_w32(base, HC_COMMAND_STATUS, HC_CS_HCR);
        for _ in 0..20 {
            if (mmio_r32(base, HC_COMMAND_STATUS) & HC_CS_HCR) == 0 {
                break;
            }
            cardbus::delay_ms(1);
        }
        mmio_w32(base, HC_HCCA, dma_phys);
        mmio_w32(base, HC_CONTROL_HEAD_ED, 0);
        mmio_w32(base, HC_BULK_HEAD_ED, 0);
        if !skip_fi {
            mmio_w32(base, HC_FM_INTERVAL, fi_reg);
        }
        mmio_w32(base, HC_PERIODIC_START, (saved_fi * 9) / 10);
        mmio_w32(base, HC_CONTROL, HC_CTRL_HCFS_OPERATIONAL | HC_CTRL_CLE);
        if !skip_fi && (mmio_r32(base, HC_FM_INTERVAL) & 0x3FFF_0000) == 0 {
            // INITRESET quirk fallback (ohci.c:288-308).
            mmio_w32(base, HC_FM_INTERVAL, fi_reg);
            mmio_w32(base, HC_PERIODIC_START, (saved_fi * 9) / 10);
        }
    }
    st.summary.stage = 3;

    // 6. Power on Root Hub ports and wait POTPGT * 2 ms + settle.
    let rh_a = unsafe { mmio_r32(base, HC_RH_DESCRIPTOR_A) };
    let num_ports = ((rh_a & 0xFF) as u8).min(15);
    let potpgt = (rh_a >> 24) & 0xFF;
    st.summary.rh_desc_a = rh_a;
    st.summary.num_ports = num_ports;

    unsafe {
        if (rh_a & RH_PSM) != 0 {
            for p in 0..(num_ports as usize) {
                mmio_w32(base, HC_RH_PORT1_STATUS + p * 4, PORT_PPS);
            }
        } else {
            mmio_w32(base, HC_RH_STATUS, RH_LPSC);
        }
    }
    usb_delay_ms((potpgt * 2).max(20) + 50);
    st.summary.stage = 4;

    // 7. Enumerate all attached Root Hub devices.
    for p in 0..(num_ports as usize) {
        let port_reg = HC_RH_PORT1_STATUS + p * 4;
        let ps = unsafe { mmio_r32(base, port_reg) };
        if (ps & PORT_CCS) == 0 {
            continue;
        }
        unsafe {
            mmio_w32(base, port_reg, PORT_CSC);
            mmio_w32(base, port_reg, PORT_PRS);
        }
        usb_delay_ms(50);
        for _ in 0..40 {
            let ps_cur = unsafe { mmio_r32(base, port_reg) };
            if (ps_cur & PORT_PRS) == 0 {
                break;
            }
            usb_delay_ms(5);
        }
        unsafe { mmio_w32(base, port_reg, PORT_PRSC) };
        usb_delay_ms(15);
        let ps_after = unsafe { mmio_r32(base, port_reg) };
        if (ps_after & PORT_PES) == 0 {
            continue;
        }
        let low_speed = (ps_after & PORT_LSDA) != 0;
        let _ = unsafe { enumerate_device(base, dma_lin, dma_phys, 0, (p + 1) as u8, low_speed) };
    }

    // 8. Enumerate downstream ports of any discovered external USB Hubs (supports nested hubs).
    let mut scan_idx = 0usize;
    while scan_idx < (ohci_state().summary.dev_count as usize) {
        let is_hub = ohci_state().summary.devices[scan_idx].dev_class == USB_CLASS_HUB;
        if is_hub {
            unsafe { enumerate_hub_ports(base, dma_lin, dma_phys, scan_idx) };
        }
        scan_idx += 1;
    }

    st.summary.stage = 5;

    // 9. If any HID Boot Mouse or Boot Keyboard was found, switch it to Boot Protocol,
    //    arm OHCI Periodic Interrupt IN EDs/TDs on HCCA.InterruptTable[0..32], and keep
    //    the 16 KiB DMA block resident for live INT 1Ch timer polling.
    let mut head_intr_ed_phys: u32 = 0;
    if st.kbd.active {
        let k = st.kbd;
        // SET_PROTOCOL(0 = Boot) and SET_IDLE(0)
        let setup_boot = [0x21, 0x0B, 0x00, 0x00, k.if_num, 0x00, 0x00, 0x00];
        let setup_idle = [0x21, 0x0A, 0x00, 0x00, k.if_num, 0x00, 0x00, 0x00];
        unsafe {
            let _ = ohci_control_xfer(base, dma_lin, dma_phys, k.addr, k.low_speed, k.max_pkt0, &setup_boot, None);
            let _ = ohci_control_xfer(base, dma_lin, dma_phys, k.addr, k.low_speed, k.max_pkt0, &setup_idle, None);
            let td_phys = dma_phys + INTR_TD_KBD_REL as u32;
            let dummy_phys = dma_phys + INTR_TD_KBD_DUMMY_REL as u32;
            let buf_phys = dma_phys + KBD_BUF_REL as u32;
            dma_w32(dma_lin, INTR_TD_KBD_DUMMY_REL, 0);
            dma_w32(dma_lin, INTR_TD_KBD_DUMMY_REL + 4, 0);
            dma_w32(dma_lin, INTR_TD_KBD_DUMMY_REL + 8, 0);
            dma_w32(dma_lin, INTR_TD_KBD_DUMMY_REL + 12, 0);
            dma_w32(
                dma_lin,
                INTR_TD_KBD_REL,
                encode_ohci_td_flags(true, TD_PID_IN, TD_NO_INT, TD_TOGGLE_DATA0, TD_CC_NOT_ACCESSED),
            );
            dma_w32(dma_lin, INTR_TD_KBD_REL + 4, buf_phys);
            dma_w32(dma_lin, INTR_TD_KBD_REL + 8, dummy_phys);
            dma_w32(dma_lin, INTR_TD_KBD_REL + 12, buf_phys + (k.max_pkt as u32) - 1);
            dma_w32(
                dma_lin,
                INTR_ED_KBD_REL,
                encode_ohci_ed_flags(k.addr, k.ep, k.low_speed, false, k.max_pkt),
            );
            dma_w32(dma_lin, INTR_ED_KBD_REL + 4, dummy_phys);
            dma_w32(dma_lin, INTR_ED_KBD_REL + 8, td_phys);
            dma_w32(dma_lin, INTR_ED_KBD_REL + 12, head_intr_ed_phys);
            head_intr_ed_phys = dma_phys + INTR_ED_KBD_REL as u32;
        }
    }
    if st.mouse.active {
        mouse_set_present();
        let m = st.mouse;
        let setup_boot = [0x21, 0x0B, 0x00, 0x00, m.if_num, 0x00, 0x00, 0x00];
        let setup_idle = [0x21, 0x0A, 0x00, 0x00, m.if_num, 0x00, 0x00, 0x00];
        unsafe {
            let _ = ohci_control_xfer(base, dma_lin, dma_phys, m.addr, m.low_speed, m.max_pkt0, &setup_boot, None);
            let _ = ohci_control_xfer(base, dma_lin, dma_phys, m.addr, m.low_speed, m.max_pkt0, &setup_idle, None);
            let td_phys = dma_phys + INTR_TD_MOUSE_REL as u32;
            let dummy_phys = dma_phys + INTR_TD_MOUSE_DUMMY_REL as u32;
            let buf_phys = dma_phys + MOUSE_BUF_REL as u32;
            dma_w32(dma_lin, INTR_TD_MOUSE_DUMMY_REL, 0);
            dma_w32(dma_lin, INTR_TD_MOUSE_DUMMY_REL + 4, 0);
            dma_w32(dma_lin, INTR_TD_MOUSE_DUMMY_REL + 8, 0);
            dma_w32(dma_lin, INTR_TD_MOUSE_DUMMY_REL + 12, 0);
            dma_w32(
                dma_lin,
                INTR_TD_MOUSE_REL,
                encode_ohci_td_flags(true, TD_PID_IN, TD_NO_INT, TD_TOGGLE_DATA0, TD_CC_NOT_ACCESSED),
            );
            dma_w32(dma_lin, INTR_TD_MOUSE_REL + 4, buf_phys);
            dma_w32(dma_lin, INTR_TD_MOUSE_REL + 8, dummy_phys);
            dma_w32(dma_lin, INTR_TD_MOUSE_REL + 12, buf_phys + (m.max_pkt as u32) - 1);
            dma_w32(
                dma_lin,
                INTR_ED_MOUSE_REL,
                encode_ohci_ed_flags(m.addr, m.ep, m.low_speed, false, m.max_pkt),
            );
            dma_w32(dma_lin, INTR_ED_MOUSE_REL + 4, dummy_phys);
            dma_w32(dma_lin, INTR_ED_MOUSE_REL + 8, td_phys);
            dma_w32(dma_lin, INTR_ED_MOUSE_REL + 12, head_intr_ed_phys);
            head_intr_ed_phys = dma_phys + INTR_ED_MOUSE_REL as u32;
        }
    }

    if st.joy.active {
        let j = st.joy;
        // SET_IDLE(0) on HID Gamepad interface (do NOT force Boot protocol on 03/00/00).
        let setup_idle = [0x21, 0x0A, 0x00, 0x00, j.if_num, 0x00, 0x00, 0x00];
        unsafe {
            let _ = ohci_control_xfer(base, dma_lin, dma_phys, j.addr, j.low_speed, j.max_pkt0, &setup_idle, None);
            let td_phys = dma_phys + INTR_TD_JOY_REL as u32;
            let dummy_phys = dma_phys + INTR_TD_JOY_DUMMY_REL as u32;
            let buf_phys = dma_phys + JOY_BUF_REL as u32;
            dma_w32(dma_lin, INTR_TD_JOY_DUMMY_REL, 0);
            dma_w32(dma_lin, INTR_TD_JOY_DUMMY_REL + 4, 0);
            dma_w32(dma_lin, INTR_TD_JOY_DUMMY_REL + 8, 0);
            dma_w32(dma_lin, INTR_TD_JOY_DUMMY_REL + 12, 0);
            dma_w32(
                dma_lin,
                INTR_TD_JOY_REL,
                encode_ohci_td_flags(true, TD_PID_IN, TD_NO_INT, TD_TOGGLE_DATA0, TD_CC_NOT_ACCESSED),
            );
            dma_w32(dma_lin, INTR_TD_JOY_REL + 4, buf_phys);
            dma_w32(dma_lin, INTR_TD_JOY_REL + 8, dummy_phys);
            dma_w32(dma_lin, INTR_TD_JOY_REL + 12, buf_phys + (j.max_pkt as u32) - 1);
            dma_w32(
                dma_lin,
                INTR_ED_JOY_REL,
                encode_ohci_ed_flags(j.addr, j.ep, j.low_speed, false, j.max_pkt),
            );
            dma_w32(dma_lin, INTR_ED_JOY_REL + 4, dummy_phys);
            dma_w32(dma_lin, INTR_ED_JOY_REL + 8, td_phys);
            dma_w32(dma_lin, INTR_ED_JOY_REL + 12, head_intr_ed_phys);
            head_intr_ed_phys = dma_phys + INTR_ED_JOY_REL as u32;
        }
        st.gameport.set_connected(true);
    }

    if st.msc.active {
        unsafe {
            msc_init_drive(base, dma_lin, dma_phys);
        }
    }

    if head_intr_ed_phys != 0 || st.msc.ready {
        unsafe {
            for i in 0..32usize {
                dma_w32(dma_lin, i * 4, head_intr_ed_phys);
            }
            core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
            mmio_w32(base, HC_CONTROL_HEAD_ED, 0);
            let ple = if head_intr_ed_phys != 0 { HC_CTRL_PLE } else { 0 };
            mmio_w32(base, HC_CONTROL, HC_CTRL_HCFS_OPERATIONAL | ple);
        }
        st.hid_active = head_intr_ed_phys != 0;
        // reserved[0]: bit 0 = mouse, bit 1 = kbd, bit 2 = read-only MSC, bit 3 = HID gamepad (tells USBPROBE.COM to stay TSR).
        st.summary.reserved[0] = u32::from(st.mouse.active)
            | (u32::from(st.kbd.active) << 1)
            | (u32::from(st.msc.ready) << 2)
            | (u32::from(st.joy.active) << 3);
    } else {
        // No HID or MSC device attached: unlock temporary probe DMA region cleanly.
        unsafe {
            mmio_w32(base, HC_CONTROL_HEAD_ED, 0);
            mmio_w32(base, HC_CONTROL, HC_CTRL_HCFS_OPERATIONAL);
            vdmad_unlock_dma_region(raw_lin, USB_DMA_SIZE as u32);
        }
        st.dma_lin = 0;
        st.dma_phys = 0;
    }

    if dst_lin != 0 {
        unsafe {
            core::ptr::copy_nonoverlapping(
                &st.summary as *const UsbSummary as *const u8,
                dst_lin as *mut u8,
                core::mem::size_of::<UsbSummary>(),
            );
        }
    }
    st.summary.dev_count as u32
}

/// FFI entry called from `_timer_hookproc` (18.2 Hz `INT 1Ch`): polls completed
/// OHCI Periodic Interrupt IN TDs for USB HID Boot Mouse (`INT 33h`) and USB HID
/// Boot Keyboard (BDA ring buffer `0040:001E..003D`), then re-arms the TDs.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_usb_poll() {
    let st = ohci_state();
    // This poll runs from two nesting contexts (the INT 08h timer hook and
    // the INT 33h status dispatch in lib.rs). If a poll is already in
    // progress, the nested call must do nothing: the outer run owns the
    // TD/buffer state, and a second consumer would read the mouse report
    // buffer after the outer run zeroed it for re-arming, inventing a
    // (0, 0, 0) report with phantom button releases.
    if st.poll_active {
        return;
    }
    st.poll_active = true;
    if !st.hid_active || st.mapped_lin == 0 || st.dma_lin == 0 {
        st.poll_active = false;
        return;
    }
    let dma_lin = st.dma_lin + st.base_off as u32;
    let dma_phys = st.dma_phys + st.base_off as u32;

    // 1. Poll USB HID Boot Mouse TD.
    if st.mouse.active {
        let td_flags = unsafe { dma_r32(dma_lin, INTR_TD_MOUSE_REL) };
        let cc = (td_flags >> 28) & 0x0F;
        if cc != TD_CC_NOT_ACCESSED {
            let buf_phys = dma_phys + MOUSE_BUF_REL as u32;
            let td_cbp = unsafe { dma_r32(dma_lin, INTR_TD_MOUSE_REL + 4) };
            // OHCI sets CBP=0 when the full buffer was transferred, or CBP > buf_phys
            // on a short packet; CBP == buf_phys means 0 bytes were transferred.
            if cc == TD_CC_NO_ERROR && td_cbp != buf_phys {
                let xferred = if td_cbp == 0 {
                    st.mouse.max_pkt as usize
                } else {
                    td_cbp.saturating_sub(buf_phys) as usize
                }
                .min(st.mouse.max_pkt as usize)
                .min(16);
                let ptr = (dma_lin as usize + MOUSE_BUF_REL) as *const u8;
                let mut raw = [0u8; 16];
                let mut i = 0usize;
                while i < xferred {
                    raw[i] = unsafe { core::ptr::read_volatile(ptr.add(i)) };
                    i += 1;
                }
                if let Some((dx, dy, dz, buttons)) =
                    decode_usb_mouse_report(&raw[..xferred], st.mouse.max_pkt)
                {
                    st.mouse.reports = st.mouse.reports.wrapping_add(1);
                    if dz != 0 {
                        st.mouse_wheel_dz = st.mouse_wheel_dz.saturating_add(dz);
                    }
                    mouse_feed_usb(dx, dy, buttons);
                }
            }
            // Re-arm mouse Interrupt IN TD (zero report buffer first, preserve DataToggle carry bit).
            let td_phys = dma_phys + INTR_TD_MOUSE_REL as u32;
            let dummy_phys = dma_phys + INTR_TD_MOUSE_DUMMY_REL as u32;
            unsafe {
                dma_w32(dma_lin, MOUSE_BUF_REL, 0);
                let headp = dma_r32(dma_lin, INTR_ED_MOUSE_REL + 8);
                let toggle_carry = headp & 0x02;
                dma_w32(
                    dma_lin,
                    INTR_TD_MOUSE_REL,
                    encode_ohci_td_flags(true, TD_PID_IN, TD_NO_INT, TD_TOGGLE_CARRY, TD_CC_NOT_ACCESSED),
                );
                dma_w32(dma_lin, INTR_TD_MOUSE_REL + 4, buf_phys);
                dma_w32(dma_lin, INTR_TD_MOUSE_REL + 8, dummy_phys);
                dma_w32(dma_lin, INTR_TD_MOUSE_REL + 12, buf_phys + (st.mouse.max_pkt as u32) - 1);
                core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
                dma_w32(dma_lin, INTR_ED_MOUSE_REL + 8, td_phys | toggle_carry);
            }
        }
    }

    // 2. Poll USB HID Boot Keyboard TD.
    if st.kbd.active {
        let td_flags = unsafe { dma_r32(dma_lin, INTR_TD_KBD_REL) };
        let cc = (td_flags >> 28) & 0x0F;
        if cc != TD_CC_NOT_ACCESSED {
            if cc == TD_CC_NO_ERROR {
                let ptr = (dma_lin as usize + KBD_BUF_REL) as *const u8;
                let mods = unsafe { core::ptr::read_volatile(ptr) };
                let mut cur = [0u8; 6];
                for i in 0..6usize {
                    cur[i] = unsafe { core::ptr::read_volatile(ptr.add(2 + i)) };
                }
                st.kbd.reports = st.kbd.reports.wrapping_add(1);
                // Emit newly pressed keys (in `cur` but not in `prev_keys`).
                for &u in &cur {
                    if u < 0x04 {
                        continue;
                    }
                    let mut was_down = false;
                    for &p in &st.kbd.prev_keys {
                        if p == u {
                            was_down = true;
                            break;
                        }
                    }
                    if !was_down {
                        if let Some(bios_word) = hid_usage_to_bios_key(u, mods) {
                            unsafe { push_bda_key(bios_word) };
                        }
                    }
                }
                st.kbd.prev_keys = cur;
            }
            // Re-arm keyboard Interrupt IN TD.
            let td_phys = dma_phys + INTR_TD_KBD_REL as u32;
            let dummy_phys = dma_phys + INTR_TD_KBD_DUMMY_REL as u32;
            let buf_phys = dma_phys + KBD_BUF_REL as u32;
            unsafe {
                let headp = dma_r32(dma_lin, INTR_ED_KBD_REL + 8);
                let toggle_carry = headp & 0x02;
                dma_w32(
                    dma_lin,
                    INTR_TD_KBD_REL,
                    encode_ohci_td_flags(true, TD_PID_IN, TD_NO_INT, TD_TOGGLE_CARRY, TD_CC_NOT_ACCESSED),
                );
                dma_w32(dma_lin, INTR_TD_KBD_REL + 4, buf_phys);
                dma_w32(dma_lin, INTR_TD_KBD_REL + 8, dummy_phys);
                dma_w32(dma_lin, INTR_TD_KBD_REL + 12, buf_phys + (st.kbd.max_pkt as u32) - 1);
                core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
                dma_w32(dma_lin, INTR_ED_KBD_REL + 8, td_phys | toggle_carry);
            }
        }
    }

    // 3. Poll USB HID Gamepad / Joystick TD.
    if st.joy.active {
        let td_flags = unsafe { dma_r32(dma_lin, INTR_TD_JOY_REL) };
        let cc = (td_flags >> 28) & 0x0F;
        if cc != TD_CC_NOT_ACCESSED {
            let buf_phys = dma_phys + JOY_BUF_REL as u32;
            let td_cbp = unsafe { dma_r32(dma_lin, INTR_TD_JOY_REL + 4) };
            if cc == TD_CC_NO_ERROR && td_cbp != buf_phys {
                let xferred = if td_cbp == 0 {
                    st.joy.max_pkt as usize
                } else {
                    td_cbp.saturating_sub(buf_phys) as usize
                }
                .min(st.joy.max_pkt as usize)
                .min(32);
                let ptr = (dma_lin as usize + JOY_BUF_REL) as *const u8;
                let mut raw = [0u8; 32];
                let mut i = 0usize;
                while i < xferred {
                    raw[i] = unsafe { core::ptr::read_volatile(ptr.add(i)) };
                    i += 1;
                }
                if let Some(rep) = decode_hid_gamepad_report(&raw[..xferred]) {
                    st.joy.reports = st.joy.reports.wrapping_add(1);
                    st.gameport.update_report(rep);
                }
            }
            // Re-arm gamepad Interrupt IN TD.
            let td_phys = dma_phys + INTR_TD_JOY_REL as u32;
            let dummy_phys = dma_phys + INTR_TD_JOY_DUMMY_REL as u32;
            unsafe {
                let headp = dma_r32(dma_lin, INTR_ED_JOY_REL + 8);
                let toggle_carry = headp & 0x02;
                dma_w32(
                    dma_lin,
                    INTR_TD_JOY_REL,
                    encode_ohci_td_flags(true, TD_PID_IN, TD_NO_INT, TD_TOGGLE_CARRY, TD_CC_NOT_ACCESSED),
                );
                dma_w32(dma_lin, INTR_TD_JOY_REL + 4, buf_phys);
                dma_w32(dma_lin, INTR_TD_JOY_REL + 8, dummy_phys);
                dma_w32(dma_lin, INTR_TD_JOY_REL + 12, buf_phys + (st.joy.max_pkt as u32) - 1);
                core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
                dma_w32(dma_lin, INTR_ED_JOY_REL + 8, td_phys | toggle_carry);
            }
        }
    }

    st.poll_active = false;
}

/// Read and clear the accumulated USB HID mouse scroll wheel delta (`dz`).
pub(crate) fn usb_mouse_take_wheel() -> i32 {
    let st = ohci_state();
    let dz = st.mouse_wheel_dz;
    st.mouse_wheel_dz = 0;
    dz
}

/// FFI entry for JLM V86 API `AX=0x001C`: return accumulated USB mouse scroll
/// wheel delta (`dz`) and reset the accumulator to 0.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_usb_mouse_take_wheel() -> i32 {
    usb_mouse_take_wheel()
}

/// FFI entry for JLM V86 API `AX=0x001B`: set the `usbdelay` multiplier
/// (clamped to `1..=20`) for slow USB flash drives and return the clamped value.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_usb_set_delay(mult: u32) -> u32 {
    let clamped = clamp_usb_delay(mult);
    ohci_state().usb_delay_mult = clamped;
    clamped
}

/// FFI entry: query the active `usbdelay` multiplier (`1..=20`).
#[unsafe(no_mangle)]
pub extern "C" fn gate1_usb_get_delay() -> u32 {
    clamp_usb_delay(ohci_state().usb_delay_mult)
}

/// FFI entry: returns `1` if a USB HID gamepad is active and connected to the
/// virtual PC gameport (`0x201` / `INT 15h AH=84h`), or `0` when absent.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_joy_present() -> u32 {
    u32::from(ohci_state().gameport.connected)
}

/// FFI entry trapped on I/O port `0x201` via JEMM `Install_IO_Handler` ONLY when
/// a USB gamepad is detected. Returns passive `0xFF` when absent or for other ports.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_joy_io(port: u32, _val: u32, is_write: u32) -> u32 {
    if port != 0x201 {
        return 0xFF;
    }
    if is_write != 0 {
        // Refresh USB HID gamepad state once per trigger cycle (never inside IN timing loop).
        unsafe { gate1_usb_poll() };
        let st = ohci_state();
        if !st.gameport.connected {
            return 0xFF;
        }
        let now = st.gameport.sample_pit_raw(unsafe { read_pit0_raw() });
        st.gameport.write(now);
        0
    } else {
        let st = ohci_state();
        if !st.gameport.connected {
            return 0xFF;
        }
        let now = st.gameport.sample_pit_raw(unsafe { read_pit0_raw() });
        st.gameport.read(now) as u32
    }
}

/// FFI entry for `INT 15h AH=84h` BIOS Joystick Support (called from `_int15_hookproc`).
/// Returns `1` when handled (stub claims with `CF=0`), or `0` to chain to BIOS.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_joy_int15(client: *mut u32) -> u32 {
    if client.is_null() {
        return 0;
    }
    if !ohci_state().gameport.connected {
        return 0;
    }
    let c = unsafe { &mut *(client as *mut [u32; 19]) };
    const CR_EBX: usize = 4;
    const CR_EDX: usize = 5;
    const CR_ECX: usize = 6;
    const CR_EAX: usize = 7;
    const CR_EFLAGS: usize = 12;
    let ah = ((c[CR_EAX] >> 8) & 0xFF) as u8;
    if ah != 0x84 {
        return 0;
    }
    let dx = (c[CR_EDX] & 0xFFFF) as u16;
    if dx > 1 {
        return 0;
    }
    unsafe { gate1_usb_poll() };
    let gp = &ohci_state().gameport;
    if !gp.connected {
        return 0;
    }
    match dx {
        0 => {
            let al = gp.bios_buttons_byte() as u32;
            c[CR_EAX] = (c[CR_EAX] & !0xFF) | al;
            c[CR_EFLAGS] &= !1;
            1
        }
        1 => {
            let axes = gp.bios_axes();
            c[CR_EAX] = (c[CR_EAX] & !0xFFFF) | (axes[0] as u32);
            c[CR_EBX] = (c[CR_EBX] & !0xFFFF) | (axes[1] as u32);
            c[CR_ECX] = (c[CR_ECX] & !0xFFFF) | (axes[2] as u32);
            c[CR_EDX] = (c[CR_EDX] & !0xFFFF) | (axes[3] as u32);
            c[CR_EFLAGS] &= !1;
            1
        }
        _ => 0,
    }
}

/// FFI entry for JLM V86 API `AX=0x0011` and `DllMain(DLL_PROCESS_DETACH)`:
/// cleanly disable OHCI lists & interrupts and unlock any active DMA block.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_usb_shutdown() -> u32 {
    let st = ohci_state();
    st.hid_active = false;
    st.mouse.active = false;
    st.mouse_wheel_dz = 0;
    st.kbd.active = false;
    st.joy.active = false;
    st.gameport.reset();
    st.msc.active = false;
    st.msc.ready = false;
    st.msc.file_open = false;
    if st.mapped_lin != 0 {
        unsafe {
            mmio_w32(st.mapped_lin, HC_INTERRUPT_DISABLE, 0xC000_007F);
            mmio_w32(st.mapped_lin, HC_CONTROL_HEAD_ED, 0);
            mmio_w32(st.mapped_lin, HC_BULK_HEAD_ED, 0);
            let ctrl = mmio_r32(st.mapped_lin, HC_CONTROL);
            mmio_w32(
                st.mapped_lin,
                HC_CONTROL,
                ctrl & !(HC_CTRL_CLE | HC_CTRL_PLE | HC_CTRL_BLE),
            );
        }
        cardbus::delay_ms(2);
    }
    if st.dma_lin != 0 {
        unsafe {
            let hcca_lin = st.dma_lin + st.base_off as u32;
            for i in 0..32usize {
                dma_w32(hcca_lin, i * 4, 0);
            }
            vdmad_unlock_dma_region(st.dma_lin, USB_DMA_SIZE as u32);
        }
        st.dma_lin = 0;
        st.dma_phys = 0;
    }
    1
}

// ---------------------------------------------------------------------
// Gate 2.6 — Polled OHCI Bulk Transfers + Read-Only USB Mass Storage (BOT/SCSI)
// ---------------------------------------------------------------------

/// Execute a single synchronous polled OHCI Bulk IN or OUT transfer (up to 512 bytes).
/// Updates `toggle` from OHCI hardware `ED.HeadP` bit 1 (`toggleCarry`) and
/// reports the TD completion code in `cc_out`: callers need the actual code,
/// not just success/failure, because a STALL completion is recoverable
/// (clear the endpoint halt and retry) while the other codes are not.
unsafe fn ohci_bulk_xfer(
    base: u32,
    dma_lin: u32,
    dma_phys: u32,
    addr: u8,
    ep: u8,
    max_pkt: u16,
    is_in: bool,
    toggle: &mut u8,
    cc_out: &mut u32,
    buf: &mut [u8],
) -> Option<usize> {
    let len = buf.len();
    if len == 0 || len > 512 {
        return None;
    }
    *cc_out = TD_CC_NOT_ACCESSED;
    let mp = if max_pkt >= 8 { max_pkt } else { 64 };
    let ed_phys = dma_phys + BULK_ED_REL as u32;
    let td_phys = dma_phys + BULK_TD_REL as u32;
    let dummy_phys = dma_phys + BULK_TD_DUMMY_REL as u32;
    let data_phys = dma_phys + BULK_DATA_REL as u32;

    if !is_in {
        unsafe {
            core::ptr::copy_nonoverlapping(
                buf.as_ptr(),
                (dma_lin as usize + BULK_DATA_REL) as *mut u8,
                len,
            );
        }
    }

    let pid = if is_in { TD_PID_IN } else { TD_PID_OUT };
    let tog = if *toggle == 0 { TD_TOGGLE_DATA0 } else { TD_TOGGLE_DATA1 };

    unsafe {
        dma_w32(dma_lin, BULK_TD_DUMMY_REL, 0);
        dma_w32(dma_lin, BULK_TD_DUMMY_REL + 4, 0);
        dma_w32(dma_lin, BULK_TD_DUMMY_REL + 8, 0);
        dma_w32(dma_lin, BULK_TD_DUMMY_REL + 12, 0);

        dma_w32(
            dma_lin,
            BULK_TD_REL,
            encode_ohci_td_flags(is_in, pid, TD_NO_INT, tog, TD_CC_NOT_ACCESSED),
        );
        dma_w32(dma_lin, BULK_TD_REL + 4, data_phys);
        dma_w32(dma_lin, BULK_TD_REL + 8, dummy_phys);
        dma_w32(dma_lin, BULK_TD_REL + 12, data_phys + (len as u32) - 1);

        dma_w32(dma_lin, BULK_ED_REL, encode_ohci_ed_flags(addr, ep, false, false, mp));
        dma_w32(dma_lin, BULK_ED_REL + 4, dummy_phys);
        dma_w32(dma_lin, BULK_ED_REL + 8, td_phys);
        dma_w32(dma_lin, BULK_ED_REL + 12, 0);

        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);

        let ctrl = mmio_r32(base, HC_CONTROL);
        mmio_w32(base, HC_BULK_CURRENT_ED, 0);
        mmio_w32(base, HC_BULK_HEAD_ED, ed_phys);
        mmio_w32(base, HC_CONTROL, ctrl | HC_CTRL_BLE);
        mmio_w32(base, HC_COMMAND_STATUS, HC_CS_BLF);
    }

    let mut ok = false;
    let mut final_headp = 0u32;
    let bulk_limit = scale_usb_delay_ms(500, usb_delay_mult());
    for _ in 0..bulk_limit {
        final_headp = unsafe { dma_r32(dma_lin, BULK_ED_REL + 8) };
        let halted = (final_headp & 1) != 0;
        let retired = (final_headp & !0x0F) == dummy_phys;
        if retired || halted {
            let cc = (unsafe { dma_r32(dma_lin, BULK_TD_REL) } >> 28) & 0x0F;
            *cc_out = cc;
            // Re-read HeadP after observing TD completion so toggleCarry is final.
            final_headp = unsafe { dma_r32(dma_lin, BULK_ED_REL + 8) };
            ok = cc == TD_CC_NO_ERROR || (retired && cc == TD_CC_NOT_ACCESSED);
            break;
        }
        cardbus::delay_ms(1);
    }

    // Update data toggle from hardware HeadP bit 1 and unlink Bulk ED cleanly,
    // waiting for OHCI Start-of-Frame (SF) per OHCI 1.0a §5.2.7.1.2 before reusing ED/TD.
    *toggle = ((final_headp >> 1) & 1) as u8;
    unsafe {
        dma_w32(dma_lin, BULK_ED_REL, encode_ohci_ed_flags(addr, ep, false, true, mp));
        let ctrl = mmio_r32(base, HC_CONTROL);
        mmio_w32(base, HC_CONTROL, ctrl & !HC_CTRL_BLE);
        mmio_w32(base, HC_BULK_HEAD_ED, 0);
        mmio_w32(base, HC_BULK_CURRENT_ED, 0);
        mmio_w32(base, HC_INTERRUPT_STATUS, 1 << 2); // clear SF
        for _ in 0..3 {
            if (mmio_r32(base, HC_INTERRUPT_STATUS) & (1 << 2)) != 0 {
                break;
            }
            cardbus::delay_ms(1);
        }
    }

    if !ok {
        return None;
    }

    if is_in {
        let cbp = unsafe { dma_r32(dma_lin, BULK_TD_REL + 4) };
        let xferred = if cbp == 0 {
            len
        } else {
            cbp.saturating_sub(data_phys) as usize
        }
        .min(len);
        unsafe {
            core::ptr::copy_nonoverlapping(
                (dma_lin as usize + BULK_DATA_REL) as *const u8,
                buf.as_mut_ptr(),
                xferred,
            );
        }
        Some(xferred)
    } else {
        Some(len)
    }
}

/// Host-controller dispatch for MSC control transfers (OHCI or EHCI).
unsafe fn msc_control_xfer(
    base: u32,
    dma_lin: u32,
    dma_phys: u32,
    addr: u8,
    max_pkt0: u16,
    setup: &[u8; 8],
) -> Option<usize> {
    if ohci_state().msc.via_ehci {
        unsafe { crate::ehci::ehci_control_xfer(addr, max_pkt0, setup, None) }
    } else {
        unsafe { ohci_control_xfer(base, dma_lin, dma_phys, addr, false, max_pkt0, setup, None) }
    }
}

/// Host-controller dispatch for MSC bulk transfers (OHCI or EHCI).
unsafe fn msc_bulk_xfer(
    base: u32,
    dma_lin: u32,
    dma_phys: u32,
    addr: u8,
    ep: u8,
    max_pkt: u16,
    is_in: bool,
    toggle: &mut u8,
    out_cc: &mut u32,
    buf: &mut [u8],
) -> Option<usize> {
    if ohci_state().msc.via_ehci {
        unsafe { crate::ehci::ehci_bulk_xfer(addr, ep, max_pkt, is_in, toggle, out_cc, buf) }
    } else {
        unsafe {
            ohci_bulk_xfer(
                base, dma_lin, dma_phys, addr, ep, max_pkt, is_in, toggle, out_cc, buf,
            )
        }
    }
}

/// Bind a high-speed USB Mass Storage device enumerated by Stage H EHCI
/// into the shared MSC/SCSI/block layer and run INQUIRY/TUR/CAPACITY/MBR probe.
/// Returns `(ready, total_sectors, has_fat, has_exfat, has_ntfs)`.
pub(crate) unsafe fn bind_ehci_msc(
    addr: u8,
    if_num: u8,
    ep_in: u8,
    ep_out: u8,
    max_pkt: u16,
    max_pkt0: u16,
) -> (bool, u32, bool, bool, bool) {
    {
        let m = &mut ohci_state().msc;
        *m = UsbMscState::ZERO;
        m.active = true;
        m.via_ehci = true;
        m.addr = addr;
        m.if_num = if_num;
        m.ep_in = ep_in;
        m.ep_out = ep_out;
        m.max_pkt = if max_pkt == 0 { 512 } else { max_pkt };
        m.max_pkt0 = if max_pkt0 == 0 { 64 } else { max_pkt0 };
        m.tag = 1;
    }
    unsafe { msc_init_drive(0, 0, 0) };
    let m = &ohci_state().msc;
    (
        m.ready,
        m.total_sectors,
        m.vol.is_some(),
        m.has_exfat,
        m.has_ntfs,
    )
}

/// Unbind any EHCI-backed MSC drive when EHCI is shut down (`EHCPROBE /U`
/// or JLM unload) so subsequent block calls fail closed rather than touching
/// a halted controller.
pub(crate) fn unbind_ehci_msc() {
    let m = &mut ohci_state().msc;
    if m.via_ehci {
        *m = UsbMscState::ZERO;
    }
}

/// Clear an endpoint halt with CLEAR_FEATURE(ENDPOINT_HALT).
unsafe fn msc_clear_endpoint_halt(
    base: u32,
    dma_lin: u32,
    dma_phys: u32,
    addr: u8,
    max_pkt0: u16,
    ep: u8,
    is_in: bool,
) {
    let w_index = if is_in { ep | 0x80 } else { ep };
    let setup = [0x02, 0x01, 0x00, 0x00, w_index, 0x00, 0x00, 0x00];
    unsafe {
        let _ = msc_control_xfer(base, dma_lin, dma_phys, addr, max_pkt0, &setup);
    }
}

/// Issue USB Mass Storage Bulk-Only Reset Recovery (BOT 1.0 §5.3.4):
/// Bulk-Only Mass Storage Reset (`0x21, 0xFF`) followed by `CLEAR_FEATURE(ENDPOINT_HALT)`
/// on both Bulk-IN and Bulk-OUT endpoints and resetting both host data toggles to DATA0.
unsafe fn msc_bot_reset_recovery(base: u32, dma_lin: u32, dma_phys: u32) {
    let (addr, ep_in, ep_out, mp0, if_num) = {
        let m = &ohci_state().msc;
        (m.addr, m.ep_in, m.ep_out, m.max_pkt0, m.if_num)
    };
    let reset_setup = [0x21, 0xFF, 0x00, 0x00, if_num, 0x00, 0x00, 0x00];
    unsafe {
        let _ = msc_control_xfer(base, dma_lin, dma_phys, addr, mp0, &reset_setup);
        msc_clear_endpoint_halt(base, dma_lin, dma_phys, addr, mp0, ep_in, true);
        msc_clear_endpoint_halt(base, dma_lin, dma_phys, addr, mp0, ep_out, false);
    }
    let m = &mut ohci_state().msc;
    m.toggle_in = 0;
    m.toggle_out = 0;
}

/// Issue a USB Mass Storage SCSI Bulk-Only Transport command per BOT 1.0 §6.7:
/// - On DATA STALL: clear halt on the stalled data EP, reset its toggle to DATA0, and read CSW.
/// - On CSW STALL: clear Bulk-IN halt, reset `toggle_in=0`, and retry reading CSW once.
/// - On Phase Error (`bCSWStatus == 2`), invalid CSW, or double CSW STALL: run BOT Reset Recovery.
/// - On sector I/O (`dlen == 512`), reject non-zero `dCSWDataResidue` to prevent silent partial I/O.
unsafe fn msc_bot_cmd(
    base: u32,
    dma_lin: u32,
    dma_phys: u32,
    cdb: &[u8],
    dir: BotDirection,
    data: Option<&mut [u8]>,
) -> bool {
    let st = ohci_state();
    if !st.msc.active || cdb.is_empty() || cdb.len() > 16 {
        return false;
    }
    let dlen = match &data {
        Some(b) => b.len() as u32,
        None => 0,
    };
    let tag = st.msc.tag;
    st.msc.tag = st.msc.tag.wrapping_add(1).max(1);

    let Some(mut cbw) = build_msc_cbw(tag, dlen, dir, cdb) else {
        return false;
    };

    let (addr, ep_out, ep_in, mp, mp0) = {
        let m = &ohci_state().msc;
        (m.addr, m.ep_out, m.ep_in, m.max_pkt, m.max_pkt0)
    };

    let mut t_out = ohci_state().msc.toggle_out;
    let mut t_in = ohci_state().msc.toggle_in;
    let mut bulk_cc = TD_CC_NOT_ACCESSED;

    // 1. CBW (Bulk OUT)
    if unsafe { msc_bulk_xfer(base, dma_lin, dma_phys, addr, ep_out, mp, false, &mut t_out, &mut bulk_cc, &mut cbw) }.is_none() {
        ohci_state().msc.toggle_out = t_out;
        if bulk_cc == TD_CC_STALL {
            unsafe { msc_bot_reset_recovery(base, dma_lin, dma_phys) };
        }
        return false;
    }
    ohci_state().msc.toggle_out = t_out;

    // 2. Optional DATA stage (Bulk IN or Bulk OUT); on STALL clear halt and proceed to CSW per BOT §6.7.2/6.7.3.
    let mut data_stalled = false;
    if let Some(buf) = data {
        if !buf.is_empty() {
            match dir {
                BotDirection::DataIn => {
                    if unsafe { msc_bulk_xfer(base, dma_lin, dma_phys, addr, ep_in, mp, true, &mut t_in, &mut bulk_cc, buf) }.is_none() {
                        ohci_state().msc.toggle_in = t_in;
                        if bulk_cc == TD_CC_STALL {
                            unsafe { msc_clear_endpoint_halt(base, dma_lin, dma_phys, addr, mp0, ep_in, true) };
                            t_in = 0;
                            ohci_state().msc.toggle_in = 0;
                            data_stalled = true;
                        } else {
                            return false;
                        }
                    } else {
                        ohci_state().msc.toggle_in = t_in;
                    }
                }
                BotDirection::DataOut => {
                    if unsafe { msc_bulk_xfer(base, dma_lin, dma_phys, addr, ep_out, mp, false, &mut t_out, &mut bulk_cc, buf) }.is_none() {
                        ohci_state().msc.toggle_out = t_out;
                        if bulk_cc == TD_CC_STALL {
                            unsafe { msc_clear_endpoint_halt(base, dma_lin, dma_phys, addr, mp0, ep_out, false) };
                            ohci_state().msc.toggle_out = 0;
                            data_stalled = true;
                        } else {
                            return false;
                        }
                    } else {
                        ohci_state().msc.toggle_out = t_out;
                    }
                }
                BotDirection::None => {}
            }
        }
    }

    // 3. CSW (Bulk IN, 13 bytes) — retry once after CLEAR_FEATURE(ENDPOINT_HALT) on STALL.
    let mut csw = [0u8; 13];
    let mut csw_len = None;
    for csw_try in 0..2 {
        bulk_cc = TD_CC_NOT_ACCESSED;
        let res = unsafe {
            msc_bulk_xfer(base, dma_lin, dma_phys, addr, ep_in, mp, true, &mut t_in, &mut bulk_cc, &mut csw)
        };
        ohci_state().msc.toggle_in = t_in;
        if let Some(n) = res {
            csw_len = Some(n);
            break;
        }
        if bulk_cc == TD_CC_STALL && csw_try == 0 {
            unsafe { msc_clear_endpoint_halt(base, dma_lin, dma_phys, addr, mp0, ep_in, true) };
            t_in = 0;
            ohci_state().msc.toggle_in = 0;
        } else {
            unsafe { msc_bot_reset_recovery(base, dma_lin, dma_phys) };
            return false;
        }
    }
    let Some(n) = csw_len else {
        unsafe { msc_bot_reset_recovery(base, dma_lin, dma_phys) };
        return false;
    };
    if n < 13 {
        unsafe { msc_bot_reset_recovery(base, dma_lin, dma_phys) };
        return false;
    }
    let sig = u32::from_le_bytes([csw[0], csw[1], csw[2], csw[3]]);
    let ctag = u32::from_le_bytes([csw[4], csw[5], csw[6], csw[7]]);
    let residue = u32::from_le_bytes([csw[8], csw[9], csw[10], csw[11]]);
    let status = csw[12];
    if sig != USB_MSC_CSW_SIG || ctag != tag || status == 2 {
        unsafe { msc_bot_reset_recovery(base, dma_lin, dma_phys) };
        return false;
    }
    if status != 0 || data_stalled {
        return false;
    }
    // Reject silent partial sector I/O (dCSWDataResidue != 0 on 512-byte READ/WRITE).
    if dlen == 512 && residue != 0 {
        return false;
    }
    true
}

/// Convenience wrapper for read-only / no-data SCSI BOT commands.
unsafe fn msc_bot_cmd_ro(
    base: u32,
    dma_lin: u32,
    dma_phys: u32,
    cdb: &[u8],
    data_in: Option<&mut [u8]>,
) -> bool {
    let dir = if data_in.is_some() {
        BotDirection::DataIn
    } else {
        BotDirection::None
    };
    unsafe { msc_bot_cmd(base, dma_lin, dma_phys, cdb, dir, data_in) }
}

/// Issue SCSI `REQUEST SENSE` (`0x03`, 18 bytes) and return `(sense_key, asc)` on success.
unsafe fn msc_request_sense(base: u32, dma_lin: u32, dma_phys: u32) -> Option<(u8, u8)> {
    let cdb_rs = [SCSI_REQUEST_SENSE, 0, 0, 0, 18, 0];
    let mut sense = [0u8; 18];
    if unsafe { msc_bot_cmd_ro(base, dma_lin, dma_phys, &cdb_rs, Some(&mut sense)) } {
        Some((sense[2] & 0x0F, sense[12]))
    } else {
        None
    }
}

/// Initialize the bound USB Mass Storage drive (INQUIRY, TEST UNIT READY, READ CAPACITY(10),
/// and read-only MBR/FAT mount).
unsafe fn msc_init_drive(base: u32, dma_lin: u32, dma_phys: u32) {
    let (addr, ep_in, ep_out, mp0) = {
        let m = &ohci_state().msc;
        (m.addr, m.ep_in, m.ep_out, m.max_pkt0)
    };
    // Clear any stale HALT on Bulk IN/OUT endpoints.
    let clr_in = [0x02, 0x01, 0x00, 0x00, ep_in | 0x80, 0x00, 0x00, 0x00];
    let clr_out = [0x02, 0x01, 0x00, 0x00, ep_out, 0x00, 0x00, 0x00];
    unsafe {
        let _ = msc_control_xfer(base, dma_lin, dma_phys, addr, mp0, &clr_in);
        let _ = msc_control_xfer(base, dma_lin, dma_phys, addr, mp0, &clr_out);
    }

    // 1. SCSI INQUIRY (36 bytes)
    let mut inq = [0u8; 36];
    let cdb_inq = [SCSI_INQUIRY, 0, 0, 0, 36, 0];
    if !unsafe { msc_bot_cmd_ro(base, dma_lin, dma_phys, &cdb_inq, Some(&mut inq)) } {
        return;
    }
    {
        let m = &mut ohci_state().msc;
        m.vendor.copy_from_slice(&inq[8..16]);
        m.product.copy_from_slice(&inq[16..32]);
    }

    // 2. SCSI TEST UNIT READY (up to 4 retries with REQUEST SENSE for Unit Attention)
    let cdb_tur = [SCSI_TEST_UNIT_READY, 0, 0, 0, 0, 0];
    let cdb_rs = [SCSI_REQUEST_SENSE, 0, 0, 0, 18, 0];
    let mut sense = [0u8; 18];
    for _ in 0..4 {
        if unsafe { msc_bot_cmd_ro(base, dma_lin, dma_phys, &cdb_tur, None) } {
            break;
        }
        let _ = unsafe { msc_bot_cmd_ro(base, dma_lin, dma_phys, &cdb_rs, Some(&mut sense)) };
        usb_delay_ms(50);
    }

    // 3. SCSI READ CAPACITY(10) (8 bytes)
    let mut cap = [0u8; 8];
    let cdb_cap = [SCSI_READ_CAPACITY10, 0, 0, 0, 0, 0, 0, 0, 0, 0];
    if !unsafe { msc_bot_cmd_ro(base, dma_lin, dma_phys, &cdb_cap, Some(&mut cap)) } {
        return;
    }
    let last_lba = ((core::hint::black_box(cap[0]) as u32) << 24)
        | ((cap[1] as u32) << 16)
        | ((cap[2] as u32) << 8)
        | (cap[3] as u32);
    let blk_sz = ((core::hint::black_box(cap[4]) as u32) << 24)
        | ((cap[5] as u32) << 16)
        | ((cap[6] as u32) << 8)
        | (cap[7] as u32);
    if blk_sz != 512 {
        return;
    }
    {
        let m = &mut ohci_state().msc;
        m.total_sectors = last_lba.saturating_add(1);
        m.sector_size = blk_sz;
        m.ready = true;
    }

    // 4. Mount MBR + FAT12/16/32 read-only volume and snapshot raw 25-byte DOS BPB.
    let vol = fat_ro::mount_volume(|lba, out| unsafe { msc_read_sector(lba, out) });
    if let Some(v) = vol {
        let mut vbr = [0u8; 512];
        if unsafe { msc_read_sector(v.part_lba, &mut vbr) } {
            let m = &mut ohci_state().msc;
            m.bpb_raw.copy_from_slice(&vbr[11..36]);
            m.media_gen = m.media_gen.wrapping_add(1).max(1);
        }
    }
    ohci_state().msc.vol = vol;
    // Always scan all MBR/superfloppy entries for non-FAT partitions so
    // dual-partition drives (e.g. FAT16 + exFAT) advertise both filesystems.
    let (has_exfat, has_ntfs) =
        probe_non_fat_partitions(|lba, out| unsafe { msc_read_sector(lba, out) });
    {
        let m = &mut ohci_state().msc;
        m.has_exfat = has_exfat;
        m.has_ntfs = has_ntfs;
        if vol.is_none() && (has_exfat || has_ntfs) {
            m.media_gen = m.media_gen.wrapping_add(1).max(1);
        }
    }
}

/// Fail-closed Stage F VBR signature check for non-FAT filesystems.
/// Full mount & validation live in the standalone EXFAT.JLM / NTFS.JLM drivers.
fn classify_non_fat_vbr(vbr: &[u8; 512]) -> abi::fs_abi::FsType {
    if vbr[510] != 0x55 || vbr[511] != 0xAA {
        return abi::fs_abi::FsType::Unknown;
    }
    if &vbr[3..11] == b"EXFAT   " && vbr[108] == 9 {
        return abi::fs_abi::FsType::ExFat;
    }
    if &vbr[3..11] == b"NTFS    " && u16::from_le_bytes([vbr[11], vbr[12]]) == 512 {
        return abi::fs_abi::FsType::Ntfs;
    }
    abi::fs_abi::FsType::Unknown
}

/// Lightweight MBR/superfloppy signature probe returning `(has_exfat, has_ntfs)`
/// across all primary MBR partitions (or superfloppy sector 0).
fn probe_non_fat_partitions<F>(mut read_sec: F) -> (bool, bool)
where
    F: FnMut(u32, &mut [u8; 512]) -> bool,
{
    let mut sec0 = [0u8; 512];
    if !read_sec(0, &mut sec0) || sec0[510] != 0x55 || sec0[511] != 0xAA {
        return (false, false);
    }
    match classify_non_fat_vbr(&sec0) {
        abi::fs_abi::FsType::ExFat => return (true, false),
        abi::fs_abi::FsType::Ntfs => return (false, true),
        _ => {}
    }
    let mut has_exfat = false;
    let mut has_ntfs = false;
    let mut vbr = [0u8; 512];
    for i in 0..4usize {
        let off = 446 + i * 16;
        let ptype = sec0[off + 4];
        let start_lba = u32::from_le_bytes([
            sec0[off + 8],
            sec0[off + 9],
            sec0[off + 10],
            sec0[off + 11],
        ]);
        let sectors = u32::from_le_bytes([
            sec0[off + 12],
            sec0[off + 13],
            sec0[off + 14],
            sec0[off + 15],
        ]);
        if ptype != 0 && start_lba != 0 && sectors != 0 && read_sec(start_lba, &mut vbr) {
            match classify_non_fat_vbr(&vbr) {
                abi::fs_abi::FsType::ExFat => has_exfat = true,
                abi::fs_abi::FsType::Ntfs => has_ntfs = true,
                _ => {}
            }
        }
    }
    (has_exfat, has_ntfs)
}

/// 512-byte sector read via SCSI `READ(10)` (`0x28`).
pub(crate) unsafe fn msc_read_sector(lba: u32, out: &mut [u8; 512]) -> bool {
    let (base, dma_lin, dma_phys, ready, ejected, via_ehci, total, sector_size) = {
        let st = ohci_state();
        (
            st.mapped_lin,
            st.dma_lin + st.base_off as u32,
            st.dma_phys + st.base_off as u32,
            st.msc.ready,
            st.msc.ejected,
            st.msc.via_ehci,
            st.msc.total_sectors,
            st.msc.sector_size,
        )
    };
    if !ready || ejected || (!via_ehci && (base == 0 || dma_lin == 0)) || (via_ehci && !crate::ehci::ehci_ready()) {
        return false;
    }
    if !validate_msc_lba_and_len(lba, 1, total, sector_size, out.len()) {
        return false;
    }
    let cdb = build_scsi_read10_cdb(lba, 1);
    unsafe { msc_bot_cmd_ro(base, dma_lin, dma_phys, &cdb, Some(out)) }
}

/// Gate 3: 512-byte sector write via USB MSC SCSI `WRITE(10)` (`0x2A`) Bulk-Out.
/// Strictly bounded by `msc.total_sectors` and 512-byte sector alignment, and
/// rejected immediately if the drive is not ready or has been safely ejected.
pub(crate) unsafe fn msc_write_sector(lba: u32, data: &[u8; 512]) -> bool {
    let (base, dma_lin, dma_phys, ready, ejected, via_ehci, total, sector_size) = {
        let st = ohci_state();
        (
            st.mapped_lin,
            st.dma_lin + st.base_off as u32,
            st.dma_phys + st.base_off as u32,
            st.msc.ready,
            st.msc.ejected,
            st.msc.via_ehci,
            st.msc.total_sectors,
            st.msc.sector_size,
        )
    };
    if !ready || ejected || (!via_ehci && (base == 0 || dma_lin == 0)) || (via_ehci && !crate::ehci::ehci_ready()) {
        return false;
    }
    if !validate_msc_lba_and_len(lba, 1, total, sector_size, data.len()) {
        return false;
    }
    let cdb = build_scsi_write10_cdb(lba, 1);
    let mut bounce = *data;
    if unsafe { msc_bot_cmd(base, dma_lin, dma_phys, &cdb, BotDirection::DataOut, Some(&mut bounce)) } {
        ohci_state().msc.writes_count = ohci_state().msc.writes_count.wrapping_add(1);
        return true;
    }
    // Clear any Check Condition via REQUEST SENSE so a write-protected or
    // removed stick does not leave the BOT state machine wedged.
    if let Some((sense_key, asc)) = unsafe { msc_request_sense(base, dma_lin, dma_phys) } {
        let outcome = classify_tur_sense(false, sense_key, asc);
        if outcome == TurOutcome::MediaChanged || outcome == TurOutcome::MediaRemoved {
            let _ = ohci_state().msc.apply_tur_outcome(outcome);
        }
    }
    false
}

/// Gate 3: Flush USB Mass Storage write cache via SCSI `SYNCHRONIZE CACHE(10)` (`0x35`).
/// USB flash sticks without a write-back cache may report `ILLEGAL REQUEST` (`0x05`);
/// that is cleared via `REQUEST SENSE` and treated as a benign flush success.
pub(crate) unsafe fn msc_sync_cache() -> bool {
    let (base, dma_lin, dma_phys, ready, ejected, via_ehci) = {
        let st = ohci_state();
        (
            st.mapped_lin,
            st.dma_lin + st.base_off as u32,
            st.dma_phys + st.base_off as u32,
            st.msc.ready,
            st.msc.ejected,
            st.msc.via_ehci,
        )
    };
    if !ready || ejected || (!via_ehci && (base == 0 || dma_lin == 0)) || (via_ehci && !crate::ehci::ehci_ready()) {
        return false;
    }
    let cdb = build_scsi_sync_cache10_cdb();
    if unsafe { msc_bot_cmd(base, dma_lin, dma_phys, &cdb, BotDirection::None, None) } {
        return true;
    }
    if let Some((sense_key, asc)) = unsafe { msc_request_sense(base, dma_lin, dma_phys) } {
        let outcome = classify_tur_sense(false, sense_key, asc);
        if outcome == TurOutcome::MediaChanged || outcome == TurOutcome::MediaRemoved {
            let _ = ohci_state().msc.apply_tur_outcome(outcome);
            return false;
        }
        return sense_key == SCSI_SENSE_ILLEGAL_REQ;
    }
    false
}

/// Gate 3: Safe eject — flush device cache (`SYNCHRONIZE CACHE(10)` `0x35`),
/// issue SCSI `START STOP UNIT` (`0x1B`, `LoEj=1, Start=0`), and invalidate
/// mounted volume state + bump `media_gen` so subsequent DOS I/O fails cleanly.
pub(crate) unsafe fn msc_eject() -> bool {
    let (base, dma_lin, dma_phys, active, ready, ejected, via_ehci) = {
        let st = ohci_state();
        (
            st.mapped_lin,
            st.dma_lin + st.base_off as u32,
            st.dma_phys + st.base_off as u32,
            st.msc.active,
            st.msc.ready,
            st.msc.ejected,
            st.msc.via_ehci,
        )
    };
    if !active || !ready || ejected || (!via_ehci && (base == 0 || dma_lin == 0)) || (via_ehci && !crate::ehci::ehci_ready()) {
        return false;
    }
    if !unsafe { msc_sync_cache() } {
        // Do not claim "Safe to unplug" after a genuine cache-flush failure;
        // keep the device mounted/available for retry.
        return false;
    }
    let cdb = build_scsi_start_stop_unit_cdb(true, false);
    if !unsafe { msc_bot_cmd(base, dma_lin, dma_phys, &cdb, BotDirection::None, None) } {
        // Flash sticks without a motor/ejector may return Illegal Request;
        // clear the sense condition so the device stays clean before unplug.
        let _ = unsafe { msc_request_sense(base, dma_lin, dma_phys) };
    }
    ohci_state().msc.mark_ejected();
    true
}

/// Gate 3: Probe live USB media state via SCSI `TEST UNIT READY` (`0x00`) and
/// `REQUEST SENSE` (`0x03`) to detect media removal or Unit Attention media change.
pub(crate) unsafe fn msc_media_check() -> MediaCheckResult {
    let (base, dma_lin, dma_phys, active, ready, ejected, via_ehci) = {
        let st = ohci_state();
        (
            st.mapped_lin,
            st.dma_lin + st.base_off as u32,
            st.dma_phys + st.base_off as u32,
            st.msc.active,
            st.msc.ready,
            st.msc.ejected,
            st.msc.via_ehci,
        )
    };
    if !active || !ready || ejected || (!via_ehci && (base == 0 || dma_lin == 0)) || (via_ehci && !crate::ehci::ehci_ready()) {
        return MediaCheckResult::NotReady;
    }
    let cdb = build_scsi_test_unit_ready_cdb();
    let outcome = if unsafe { msc_bot_cmd(base, dma_lin, dma_phys, &cdb, BotDirection::None, None) } {
        TurOutcome::Ready
    } else if let Some((sense_key, asc)) = unsafe { msc_request_sense(base, dma_lin, dma_phys) } {
        classify_tur_sense(false, sense_key, asc)
    } else {
        TurOutcome::Error
    };
    ohci_state().msc.apply_tur_outcome(outcome)
}

struct AsciiOut<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> AsciiOut<'a> {
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
    fn push_u32(&mut self, mut v: u32) {
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
}

/// FFI entry for JLM V86 API `AX=0x0012` (`USBCP` with no args): format a read-only
/// USB Mass Storage volume summary + root directory listing into `dst_lin[..cap]`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_usb_msc_dir(dst_lin: u32, cap: u32) -> u32 {
    if dst_lin == 0 || cap == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(dst_lin as *mut u8, cap as usize) };
    let mut w = AsciiOut::new(slice);
    let m = &ohci_state().msc;
    if !m.ready {
        return 0;
    }
    w.push_bytes(b"USBMSC: ");
    w.push_bytes(&m.vendor);
    w.push_byte(b' ');
    w.push_bytes(&m.product);
    w.push_bytes(b" (");
    w.push_u32(m.total_sectors / 2048);
    w.push_bytes(b" MiB) ");
    let Some(vol) = m.vol else {
        w.push_bytes(b"NO-FAT\r\n");
        return w.pos as u32;
    };
    match vol.kind {
        fat_ro::FatKind::Fat12 => w.push_bytes(b"FAT12\r\n"),
        fat_ro::FatKind::Fat16 => w.push_bytes(b"FAT16\r\n"),
        fat_ro::FatKind::Fat32 => w.push_bytes(b"FAT32\r\n"),
    }
    fat_ro::for_each_dir_entry(
        &vol,
        0,
        |lba, out| unsafe { msc_read_sector(lba, out) },
        |e| {
            let mut nbuf = [0u8; 12];
            let nlen = fat_ro::format_83_name(&e.name83, &mut nbuf);
            w.push_bytes(b"  ");
            w.push_bytes(&nbuf[..nlen]);
            for _ in nlen..14 {
                w.push_byte(b' ');
            }
            if (e.attr & 0x10) != 0 {
                w.push_bytes(b"<DIR>\r\n");
            } else {
                w.push_u32(e.file_size);
                w.push_bytes(b" B\r\n");
            }
            w.pos + 32 < w.buf.len()
        },
    );
    w.pos as u32
}

/// FFI entry for JLM V86 API `AX=0x0013` (`USBCP <src> [dst]`): open a file on
/// the read-only USB Mass Storage volume by NUL-terminated 8.3 path at `name_lin`.
/// Returns `1` if found, `0` otherwise.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_usb_msc_open(name_lin: u32) -> u32 {
    if name_lin == 0 {
        return 0;
    }
    let Some(vol) = ohci_state().msc.vol else {
        return 0;
    };
    let ptr = name_lin as *const u8;
    let mut len = 0usize;
    while len < 128 {
        if unsafe { core::ptr::read_volatile(ptr.add(len)) } == 0 {
            break;
        }
        len += 1;
    }
    let path = unsafe { core::slice::from_raw_parts(ptr, len) };
    let Some(ent) = fat_ro::find_path(&vol, path, |lba, out| unsafe { msc_read_sector(lba, out) }) else {
        return 0;
    };
    if (ent.attr & 0x10) != 0 {
        return 0; // directory, not a file
    }
    let m = &mut ohci_state().msc;
    m.file_open = true;
    m.file_cluster = ent.first_cluster;
    m.file_sec_in_clus = 0;
    m.file_remaining = ent.file_size;
    1
}

/// FFI entry for JLM V86 API `AX=0x0014`: read the next up-to-512-byte chunk of
/// the opened USB file into `dst_lin`. Returns bytes read (`0` = EOF, `0xFFFF` = error).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_usb_msc_read_chunk(dst_lin: u32) -> u32 {
    if dst_lin == 0 {
        return 0xFFFF;
    }
    let (open, vol, mut clus, mut sec_in_clus, rem) = {
        let m = &ohci_state().msc;
        (m.file_open, m.vol, m.file_cluster, m.file_sec_in_clus, m.file_remaining)
    };
    let Some(vol) = vol else {
        return 0xFFFF;
    };
    if !open || rem == 0 {
        ohci_state().msc.file_open = false;
        return 0;
    }
    let Some(base_lba) = fat_ro::cluster_to_lba(&vol, clus) else {
        ohci_state().msc.file_open = false;
        return 0xFFFF;
    };
    let mut sec = [0u8; 512];
    if !unsafe { msc_read_sector(base_lba + sec_in_clus as u32, &mut sec) } {
        ohci_state().msc.file_open = false;
        return 0xFFFF;
    }
    let take = (rem as usize).min(512);
    unsafe {
        core::ptr::copy_nonoverlapping(sec.as_ptr(), dst_lin as *mut u8, take);
    }
    let new_rem = rem - take as u32;
    sec_in_clus += 1;
    if new_rem > 0 && sec_in_clus >= vol.sec_per_clus {
        sec_in_clus = 0;
        match fat_ro::next_cluster(&vol, clus, |lba, out| unsafe { msc_read_sector(lba, out) }) {
            Some(next) => clus = next,
            None => {
                ohci_state().msc.file_open = false;
                ohci_state().msc.file_remaining = 0;
                return take as u32;
            }
        }
    }
    let m = &mut ohci_state().msc;
    m.file_cluster = clus;
    m.file_sec_in_clus = sec_in_clus;
    m.file_remaining = new_rem;
    if new_rem == 0 {
        m.file_open = false;
    }
    take as u32
}

/// FFI entry for JLM V86 API `AX=0x0015` (`USBDRV.SYS` BUILD BPB / MEDIA CHECK):
/// writes 33 bytes at `dst_lin` (`bpb_raw[25]` + `part_lba: u32` + `media_gen: u32`).
/// Returns `(media_gen << 16) | 1` when a FAT12/FAT16 USB volume is mounted, else `0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_usb_msc_bpb(dst_lin: u32) -> u32 {
    if ohci_state().msc.ejected || !ohci_state().msc.ready {
        return 0;
    }
    if ohci_state().msc.media_changed {
        // Invalidate the previous card's cached layout immediately so a partial
        // or failed refresh can never hand DOS the old geometry with the new gen.
        {
            let m = &mut ohci_state().msc;
            m.vol = None;
            m.has_exfat = false;
            m.has_ntfs = false;
            m.bpb_raw = [0u8; 25];
        }
        // Refresh READ CAPACITY(10) and volume BPB after a Unit Attention media-change event.
        let (base, dma_lin, dma_phys) = {
            let st = ohci_state();
            (st.mapped_lin, st.dma_lin + st.base_off as u32, st.dma_phys + st.base_off as u32)
        };
        let mut cap = [0u8; 8];
        let rc_cdb = [0x25u8, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        if base == 0
            || dma_lin == 0
            || !unsafe { msc_bot_cmd_ro(base, dma_lin, dma_phys, &rc_cdb, Some(&mut cap)) }
        {
            return 0;
        }
        let last_lba = ((core::hint::black_box(cap[0]) as u32) << 24)
            | ((cap[1] as u32) << 16)
            | ((cap[2] as u32) << 8)
            | (cap[3] as u32);
        let blk_len = ((core::hint::black_box(cap[4]) as u32) << 24)
            | ((cap[5] as u32) << 16)
            | ((cap[6] as u32) << 8)
            | (cap[7] as u32);
        {
            let m = &mut ohci_state().msc;
            m.total_sectors = last_lba.saturating_add(1);
            m.sector_size = blk_len;
        }
        let (has_exfat, has_ntfs) =
            probe_non_fat_partitions(|lba, out| unsafe { msc_read_sector(lba, out) });
        let Some(v) = fat_ro::mount_volume(|lba, out| unsafe { msc_read_sector(lba, out) }) else {
            // Not FAT: record non-FAT signatures (exFAT / NTFS) for FSLOAD.
            let m = &mut ohci_state().msc;
            m.has_exfat = has_exfat;
            m.has_ntfs = has_ntfs;
            m.media_changed = false;
            return 0;
        };
        let mut vbr = [0u8; 512];
        if !unsafe { msc_read_sector(v.part_lba, &mut vbr) } {
            return 0;
        }
        let m = &mut ohci_state().msc;
        m.bpb_raw.copy_from_slice(&vbr[11..36]);
        m.vol = Some(v);
        m.has_exfat = has_exfat;
        m.has_ntfs = has_ntfs;
        m.media_changed = false;
    }
    let m = &ohci_state().msc;
    let Some(vol) = m.vol else {
        return 0;
    };
    if !m.ready || m.ejected || vol.kind == fat_ro::FatKind::Fat32 {
        // DOS block drivers use standard 25-byte BPB (FAT12/FAT16); FAT32 is handled by USBCP.
        return 0;
    }
    if dst_lin != 0 {
        unsafe {
            let dst = dst_lin as *mut u8;
            core::ptr::copy_nonoverlapping(m.bpb_raw.as_ptr(), dst, 25);
            // Zero hidden_sectors in the BPB exposed to DOS (USBDRV.SYS adds part_lba itself).
            core::ptr::write_bytes(dst.add(17), 0, 4);
            let plba = vol.part_lba.to_le_bytes();
            core::ptr::copy_nonoverlapping(plba.as_ptr(), dst.add(25), 4);
            let mg = (m.media_gen as u32).to_le_bytes();
            core::ptr::copy_nonoverlapping(mg.as_ptr(), dst.add(29), 4);
        }
    }
    ((m.media_gen as u32) << 16) | 1
}

/// FFI entry for JLM V86 API `AX=0x0016` (`USBDRV.SYS` INPUT / READ):
/// 512-byte sector read at `lba` into `dst_lin`. Returns 1 or 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_usb_msc_read_sec(lba: u32, dst_lin: u32) -> u32 {
    if dst_lin == 0 {
        return 0;
    }
    let mut sec = [0u8; 512];
    if !unsafe { msc_read_sector(lba, &mut sec) } {
        return 0;
    }
    unsafe {
        core::ptr::copy_nonoverlapping(sec.as_ptr(), dst_lin as *mut u8, 512);
    }
    1
}

/// FFI entry for JLM V86 API `AX=0x0017` (`USBDRV.SYS` OUTPUT / WRITE):
/// Gate 3 512-byte USB MSC sector write at `lba` from `src_lin`. Returns 1 or 0.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_usb_msc_write_sec(lba: u32, src_lin: u32) -> u32 {
    if src_lin == 0 {
        return 0;
    }
    let mut sec = [0u8; 512];
    unsafe {
        core::ptr::copy_nonoverlapping(src_lin as *const u8, sec.as_mut_ptr(), 512);
    }
    if unsafe { msc_write_sector(lba, &sec) } {
        1
    } else {
        0
    }
}

/// FFI entry for JLM V86 API `AX=0x0018`: flush USB MSC write cache
/// via SCSI `SYNCHRONIZE CACHE(10)` (`0x35`). Returns 1 on success, 0 on error.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_usb_msc_flush() -> u32 {
    if unsafe { msc_sync_cache() } { 1 } else { 0 }
}

/// FFI entry for JLM V86 API `AX=0x0019` (`USBEJECT.COM` / `USBCP /E`):
/// flush cache, send SCSI `START STOP UNIT` (`0x1B`, safe eject), and invalidate
/// mounted volume state. Returns 1 on success, 0 if no USB drive is mounted.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_usb_msc_eject() -> u32 {
    if unsafe { msc_eject() } { 1 } else { 0 }
}

/// Stage F: classify the currently mounted USB MSC volume into the
/// portable `abi::fs_abi::FsType` enum (fail-closed: `Unknown` when absent,
/// ejected, or unrecognized).
pub(crate) fn detect_msc_fs_type() -> abi::fs_abi::FsType {
    let m = &ohci_state().msc;
    if !m.ready || m.ejected {
        return abi::fs_abi::FsType::Unknown;
    }
    if let Some(v) = m.vol {
        return match v.kind {
            fat_ro::FatKind::Fat12 => abi::fs_abi::FsType::Fat12,
            fat_ro::FatKind::Fat16 => abi::fs_abi::FsType::Fat16,
            fat_ro::FatKind::Fat32 => abi::fs_abi::FsType::Fat32,
        };
    }
    if m.has_exfat {
        return abi::fs_abi::FsType::ExFat;
    }
    if m.has_ntfs {
        return abi::fs_abi::FsType::Ntfs;
    }
    abi::fs_abi::FsType::Unknown
}

fn blk_flags(m: &UsbMscState) -> u8 {
    let mut flags = 0u8;
    if m.active {
        flags |= abi::fs_abi::BLK_FLAG_ACTIVE;
    }
    if m.ready {
        flags |= abi::fs_abi::BLK_FLAG_READY;
    }
    if m.ejected {
        flags |= abi::fs_abi::BLK_FLAG_EJECTED;
    }
    if m.media_changed {
        flags |= abi::fs_abi::BLK_FLAG_MEDIA_CHANGED;
    }
    if m.ready && !m.ejected {
        if m.has_exfat {
            flags |= abi::fs_abi::BLK_FLAG_HAS_EXFAT;
        }
        if m.has_ntfs {
            flags |= abi::fs_abi::BLK_FLAG_HAS_NTFS;
        }
    }
    flags
}

/// Stage F DDB Service 0: return portable Block-Transport ABI version (`0x0001_0000`).
#[unsafe(no_mangle)]
pub extern "C" fn gate1_blk_abi_version() -> u32 {
    abi::fs_abi::FERRIC_BLK_ABI_VERSION
}

/// Stage F DDB Service 1: fill a 48-byte `abi::fs_abi::BlkDevInfo` at `out_lin`.
/// Returns `1` on success, `0` if `out_lin == 0`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_blk_info(out_lin: u32) -> u32 {
    if out_lin == 0 {
        return 0;
    }
    let m = &ohci_state().msc;
    let flags = blk_flags(m);
    let info = abi::fs_abi::BlkDevInfo {
        abi_version: abi::fs_abi::FERRIC_BLK_ABI_VERSION,
        total_sectors: m.total_sectors,
        sector_size: m.sector_size,
        media_gen: m.media_gen,
        fs_type: detect_msc_fs_type().as_u8(),
        flags,
        vendor: m.vendor,
        product: m.product,
        reserved: [0; 8],
    };
    unsafe { core::ptr::write_unaligned(out_lin as *mut abi::fs_abi::BlkDevInfo, info) };
    1
}

/// FFI entry for JLM V86 API `AX=0x0035` (`USBPROBE` / `FSLOAD` filesystem detection):
/// returns `(media_gen << 16) | (flags << 8) | fs_type`.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_usb_msc_fs_type() -> u32 {
    let m = &ohci_state().msc;
    let flags = blk_flags(m);
    ((m.media_gen as u32) << 16) | ((flags as u32) << 8) | (detect_msc_fs_type().as_u8() as u32)
}

/// FFI entry for JLM V86 API `AX=0x001A` (`USBDRV.SYS` MEDIA CHECK):
/// probes live USB media via SCSI `TEST UNIT READY` (`0x00`).
/// Returns `0` (not ready/removed/ejected), `(gen << 16) | 1` (unchanged),
/// or `(gen << 16) | 2` (media changed).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_usb_msc_media_check() -> u32 {
    match unsafe { msc_media_check() } {
        MediaCheckResult::NotReady => 0,
        MediaCheckResult::Unchanged(mgen) => ((mgen as u32) << 16) | 1,
        MediaCheckResult::Changed(mgen) => ((mgen as u32) << 16) | 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ohci_ed_and_td_flag_encoding_matches_spec() {
        // Addr 5, EP 0, LowSpeed=1, Skip=0, MaxPkt=8
        let ed = encode_ohci_ed_flags(5, 0, true, false, 8);
        assert_eq!(ed & 0x7F, 5);
        assert_eq!((ed >> 7) & 0x0F, 0);
        assert_ne!(ed & (1 << 13), 0);
        assert_eq!(ed & (1 << 14), 0);
        assert_eq!((ed >> 16) & 0x7FF, 8);

        // SETUP TD: rounding=0, PID=0, DI=7, Toggle=DATA0 (2), CC=0xE
        let td = encode_ohci_td_flags(false, TD_PID_SETUP, TD_NO_INT, TD_TOGGLE_DATA0, TD_CC_NOT_ACCESSED);
        assert_eq!(td & (1 << 18), 0);
        assert_eq!((td >> 19) & 3, 0);
        assert_eq!((td >> 21) & 7, 7);
        assert_eq!((td >> 24) & 3, 2);
        assert_eq!((td >> 28) & 0xF, 0xE);
    }

    #[test]
    fn parse_usb_config_desc_extracts_hid_boot_mouse_and_ep() {
        // Minimal 9B Config + 9B Interface (HID Boot Mouse 03/01/02) + 7B Endpoint (EP1 IN Intr 4B 10ms)
        let desc: [u8; 25] = [
            9, 2, 25, 0, 1, 1, 0, 0xA0, 50, // Config
            9, 4, 0, 0, 1, 0x03, 0x01, 0x02, 0, // Interface: HID Boot Mouse
            7, 5, 0x81, 0x03, 0x04, 0x00, 10, // Endpoint: EP1 IN Interrupt, 4B, 10ms
        ];
        let parsed = parse_usb_config_desc(&desc).unwrap();
        assert_eq!(parsed.config_val, 1);
        assert_eq!(parsed.if_class, 0x03);
        assert_eq!(parsed.if_sub, 0x01);
        assert_eq!(parsed.if_proto, 0x02);
        assert_eq!(parsed.int_ep, 1);
        assert_eq!(parsed.int_maxpkt, 4);
        assert_eq!(parsed.int_interval, 10);
    }

    #[test]
    fn usb_summary_size_matches_asm_contract() {
        assert_eq!(core::mem::size_of::<UsbDeviceInfo>(), 24);
        assert_eq!(core::mem::size_of::<UsbSummary>(), 416);
    }

    #[test]
    fn parse_composite_hid_kbd_plus_mouse_dongle() {
        // Composite 2.4 GHz wireless dongle: Interface 0 = Boot KBD (EP1), Interface 1 = Boot Mouse (EP2)
        let desc: [u8; 41] = [
            9, 2, 41, 0, 2, 1, 0, 0xA0, 50, // Config (2 interfaces)
            9, 4, 0, 0, 1, 0x03, 0x01, 0x01, 0, // IF0: HID Boot Keyboard
            7, 5, 0x81, 0x03, 0x08, 0x00, 10, // EP1 IN Interrupt, 8B
            9, 4, 1, 0, 1, 0x03, 0x01, 0x02, 0, // IF1: HID Boot Mouse
            7, 5, 0x82, 0x03, 0x04, 0x00, 10, // EP2 IN Interrupt, 4B
        ];
        let parsed = parse_usb_config_desc(&desc).unwrap();
        assert_eq!(parsed.kbd_if, 0);
        assert_eq!(parsed.kbd_ep, 1);
        assert_eq!(parsed.kbd_maxpkt, 8);
        assert_eq!(parsed.mouse_if, 1);
        assert_eq!(parsed.mouse_ep, 2);
        assert_eq!(parsed.mouse_maxpkt, 4);
    }

    #[test]
    fn hid_usage_translates_letters_shift_ctrl_and_special_keys() {
        assert_eq!(hid_usage_to_bios_key(0x04, 0x00), Some(0x1E61)); // 'a'
        assert_eq!(hid_usage_to_bios_key(0x04, 0x02), Some(0x1E41)); // 'A' (LShift)
        assert_eq!(hid_usage_to_bios_key(0x06, 0x01), Some(0x2E03)); // Ctrl+C
        assert_eq!(hid_usage_to_bios_key(0x28, 0x00), Some(0x1C0D)); // Enter
        assert_eq!(hid_usage_to_bios_key(0x29, 0x00), Some(0x011B)); // Esc
        assert_eq!(hid_usage_to_bios_key(0x52, 0x00), Some(0x4800)); // Up Arrow
    }

    #[test]
    fn scsi_write10_cbw_and_cdb_packing() {
        // LBA above 2 GiB (0x0045_6789 sectors ~= 2.16 GiB) with 1 sector (512 bytes)
        let cdb = build_scsi_write10_cdb(0x0045_6789, 1);
        assert_eq!(cdb, [0x2A, 0x00, 0x00, 0x45, 0x67, 0x89, 0x00, 0x00, 0x01, 0x00]);

        let cbw = build_msc_cbw(0x1122_3344, 512, BotDirection::DataOut, &cdb).unwrap();
        assert_eq!(&cbw[0..4], &USB_MSC_CBW_SIG.to_le_bytes());
        assert_eq!(&cbw[4..8], &0x1122_3344u32.to_le_bytes());
        assert_eq!(&cbw[8..12], &512u32.to_le_bytes());
        assert_eq!(cbw[12], 0x00); // Bulk-Out direction flag
        assert_eq!(cbw[13], 0x00); // LUN 0
        assert_eq!(cbw[14], 10);   // CDB length
        assert_eq!(&cbw[15..25], &cdb);

        // Compare against READ(10) Bulk-In flag
        let rcdb = build_scsi_read10_cdb(0x0045_6789, 1);
        assert_eq!(rcdb[0], 0x28);
        let rcbw = build_msc_cbw(7, 512, BotDirection::DataIn, &rcdb).unwrap();
        assert_eq!(rcbw[12], 0x80); // Bulk-In direction flag

        // Invalid CDB lengths and inconsistent direction/length are rejected
        assert_eq!(build_msc_cbw(1, 0, BotDirection::None, &[]), None);
        assert_eq!(build_msc_cbw(1, 0, BotDirection::None, &[0u8; 17]), None);
        assert_eq!(build_msc_cbw(1, 512, BotDirection::None, &cdb), None);
    }

    #[test]
    fn msc_lba_bounds_and_sector_alignment_validation() {
        let total = 16_384u32; // 8 MiB stick
        // Valid boundaries: first sector and last sector
        assert!(validate_msc_lba_and_len(0, 1, total, 512, 512));
        assert!(validate_msc_lba_and_len(total - 1, 1, total, 512, 512));
        // Out-of-bounds LBA rejections
        assert!(!validate_msc_lba_and_len(total, 1, total, 512, 512));
        assert!(!validate_msc_lba_and_len(total + 100, 1, total, 512, 512));
        // u32 overflow protection (checked 64-bit arithmetic)
        assert!(!validate_msc_lba_and_len(u32::MAX, 1, u32::MAX, 512, 512));
        assert!(!validate_msc_lba_and_len(u32::MAX - 1, 2, u32::MAX, 512, 1024));
        // Sector alignment and geometry rejections
        assert!(!validate_msc_lba_and_len(0, 0, total, 512, 0));
        assert!(!validate_msc_lba_and_len(0, 1, total, 512, 511));
        assert!(!validate_msc_lba_and_len(0, 1, total, 512, 513));
        assert!(!validate_msc_lba_and_len(0, 1, total, 4096, 512));
        assert!(!validate_msc_lba_and_len(0, 1, 0, 512, 512));
    }

    #[test]
    fn scsi_sync_cache10_and_start_stop_unit_eject_cbw_packing() {
        let sync_cdb = build_scsi_sync_cache10_cdb();
        assert_eq!(sync_cdb, [0x35, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        let sync_cbw = build_msc_cbw(42, 0, BotDirection::None, &sync_cdb).unwrap();
        assert_eq!(sync_cbw[12], 0x00);
        assert_eq!(sync_cbw[14], 10);
        assert_eq!(sync_cbw[15], 0x35);

        let eject_cdb = build_scsi_start_stop_unit_cdb(true, false);
        assert_eq!(eject_cdb, [0x1B, 0, 0, 0, 0x02, 0]);
        let eject_cbw = build_msc_cbw(43, 0, BotDirection::None, &eject_cdb).unwrap();
        assert_eq!(eject_cbw[12], 0x00);
        assert_eq!(eject_cbw[14], 6);
        assert_eq!(&eject_cbw[15..21], &[0x1B, 0, 0, 0, 0x02, 0]);

        let tur_cdb = build_scsi_test_unit_ready_cdb();
        assert_eq!(tur_cdb, [0x00, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn scsi_tur_sense_classification_and_media_change_state_machine() {
        assert_eq!(classify_tur_sense(true, 0, 0), TurOutcome::Ready);
        assert_eq!(classify_tur_sense(false, 0x06, 0x28), TurOutcome::MediaChanged);
        assert_eq!(classify_tur_sense(false, 0x02, 0x3A), TurOutcome::MediaRemoved);
        assert_eq!(classify_tur_sense(false, 0x05, 0x20), TurOutcome::Error);

        let mut msc = UsbMscState::ZERO;
        msc.active = true;
        msc.ready = true;
        msc.media_gen = 1;

        // Steady-state TUR ready -> Unchanged(1)
        assert_eq!(msc.apply_tur_outcome(TurOutcome::Ready), MediaCheckResult::Unchanged(1));

        // Unit Attention (media swap) -> Changed(2), latches media_changed
        assert_eq!(msc.apply_tur_outcome(TurOutcome::MediaChanged), MediaCheckResult::Changed(2));
        assert!(msc.media_changed);
        assert_eq!(msc.media_gen, 2);
        // Subsequent Ready while media_changed is still latched keeps reporting Changed until BPB rebuild
        assert_eq!(msc.apply_tur_outcome(TurOutcome::Ready), MediaCheckResult::Changed(2));

        // Clear latch as BUILD BPB does, then simulate safe eject
        msc.media_changed = false;
        assert_eq!(msc.apply_tur_outcome(TurOutcome::Ready), MediaCheckResult::Unchanged(2));
        msc.mark_ejected();
        assert!(msc.ejected);
        assert!(!msc.ready);
        assert_eq!(msc.media_gen, 3);
        assert_eq!(msc.apply_tur_outcome(TurOutcome::Ready), MediaCheckResult::NotReady);

        // Fresh mount followed by surprise removal -> NotReady + ready=false + bumped gen
        msc.ready = true;
        msc.ejected = false;
        msc.media_changed = false;
        assert_eq!(msc.apply_tur_outcome(TurOutcome::MediaRemoved), MediaCheckResult::NotReady);
        assert!(!msc.ready);
        assert_eq!(msc.media_gen, 4);
    }

    #[test]
    fn dual_partition_mbr_detects_secondary_exfat_and_ntfs() {
        let mut mbr = [0u8; 512];
        mbr[510] = 0x55;
        mbr[511] = 0xAA;
        // Partition 1: FAT16 at LBA 63
        mbr[446 + 4] = 0x06;
        mbr[446 + 8..446 + 12].copy_from_slice(&63u32.to_le_bytes());
        mbr[446 + 12..446 + 16].copy_from_slice(&4096u32.to_le_bytes());
        // Partition 2: exFAT at LBA 4160
        mbr[462 + 4] = 0x07;
        mbr[462 + 8..462 + 12].copy_from_slice(&4160u32.to_le_bytes());
        mbr[462 + 12..462 + 16].copy_from_slice(&8192u32.to_le_bytes());
        // Partition 3: NTFS at LBA 12352
        mbr[478 + 4] = 0x07;
        mbr[478 + 8..478 + 12].copy_from_slice(&12352u32.to_le_bytes());
        mbr[478 + 12..478 + 16].copy_from_slice(&8192u32.to_le_bytes());

        let mut fat_vbr = [0u8; 512];
        fat_vbr[510] = 0x55;
        fat_vbr[511] = 0xAA;

        let mut exfat_vbr = [0u8; 512];
        exfat_vbr[3..11].copy_from_slice(b"EXFAT   ");
        exfat_vbr[108] = 9;
        exfat_vbr[510] = 0x55;
        exfat_vbr[511] = 0xAA;

        let mut ntfs_vbr = [0u8; 512];
        ntfs_vbr[3..11].copy_from_slice(b"NTFS    ");
        ntfs_vbr[11..13].copy_from_slice(&512u16.to_le_bytes());
        ntfs_vbr[510] = 0x55;
        ntfs_vbr[511] = 0xAA;

        let res = probe_non_fat_partitions(|lba, out| {
            match lba {
                0 => out.copy_from_slice(&mbr),
                63 => out.copy_from_slice(&fat_vbr),
                4160 => out.copy_from_slice(&exfat_vbr),
                12352 => out.copy_from_slice(&ntfs_vbr),
                _ => return false,
            }
            true
        });
        assert_eq!(res, (true, true));
    }

    #[test]
    fn parse_usb_config_desc_extracts_hid_gamepad_interface() {
        // 9B Config + 9B Interface (HID Gamepad 03/00/00) + 7B Endpoint (EP1 IN Intr 8B 10ms)
        let desc: [u8; 25] = [
            9, 2, 25, 0, 1, 1, 0, 0x80, 50,
            9, 4, 0, 0, 1, 0x03, 0x00, 0x00, 0,
            7, 5, 0x81, 0x03, 0x08, 0x00, 10,
        ];
        let parsed = parse_usb_config_desc(&desc).unwrap();
        assert_eq!(parsed.if_class, 0x03);
        assert_eq!(parsed.if_sub, 0x00);
        assert_eq!(parsed.if_proto, 0x00);
        assert_eq!(parsed.joy_if, 0);
        assert_eq!(parsed.joy_ep, 1);
        assert_eq!(parsed.joy_maxpkt, 8);
        assert_eq!(parsed.kbd_ep, 0);
        assert_eq!(parsed.mouse_ep, 0);
    }

    #[test]
    fn hid_gamepad_report_decoding_4_to_8_bytes() {
        // Short report rejected
        assert_eq!(decode_hid_gamepad_report(&[0x80, 0x80, 0x01]), None);

        // 4-byte 2-axis gamepad report: [X1, Y1, buttons, pad]
        let r4 = decode_hid_gamepad_report(&[0x00, 0xFF, 0x05, 0x00]).unwrap();
        assert_eq!(r4.axes, [0x00, 0xFF, 128, 128]);
        assert_eq!(r4.axis_mask, 0x03);
        assert_eq!(r4.buttons, 0x05); // buttons 1 & 3 pressed

        // 8-byte 4-axis gamepad report: [X1, Y1, X2, Y2, buttons, ...]
        let r8 = decode_hid_gamepad_report(&[0x10, 0x80, 0xF0, 0x40, 0xFA, 0, 0, 0]).unwrap();
        assert_eq!(r8.axes, [0x10, 0x80, 0xF0, 0x40]);
        assert_eq!(r8.axis_mask, 0x0F);
        assert_eq!(r8.buttons, 0x0A); // low 4 bits = buttons 2 & 4
    }

    #[test]
    fn gameport_disconnected_returns_0xff_and_ignores_writes() {
        let mut gp = GameportState::new();
        assert!(!gp.connected);
        assert_eq!(gp.read(0), 0xFF);
        gp.write(100);
        assert_eq!(gp.read(100), 0xFF);
        assert_eq!(gp.read(10_000), 0xFF);
        assert_eq!(gp.bios_buttons_byte(), 0xF0);
        assert_eq!(gp.bios_axes(), [0, 0, 0, 0]);

        // FFI gate1_joy_io also returns 0xFF when disconnected or wrong port
        ohci_reset();
        assert_eq!(gate1_joy_present(), 0);
        assert_eq!(unsafe { gate1_joy_io(0x201, 0, 0) }, 0xFF);
        assert_eq!(unsafe { gate1_joy_io(0x201, 0xFF, 1) }, 0xFF);
        assert_eq!(unsafe { gate1_joy_io(0x202, 0, 0) }, 0xFF);
    }

    #[test]
    fn gameport_pit_timing_and_button_encoding() {
        // Verify IBM PC Gameport timing formula: 24 us + axis * 1100 / 255
        assert_eq!(gameport_axis_duration_us(0), 24);
        assert_eq!(gameport_axis_duration_us(128), 576);
        assert_eq!(gameport_axis_duration_us(255), 1124);

        let t_min = gameport_axis_duration_pit_ticks(0);
        let t_mid = gameport_axis_duration_pit_ticks(128);
        let t_max = gameport_axis_duration_pit_ticks(255);
        assert_eq!(t_min, 29);   // 24 us * 1.193182 MHz
        assert_eq!(t_mid, 687);  // 576 us * 1.193182 MHz
        assert_eq!(t_max, 1341); // 1124 us * 1.193182 MHz

        // 2-axis stick: X1=0 (29 ticks), Y1=255 (1341 ticks), X2/Y2 disconnected, buttons 1 & 4 pressed (0b1001)
        let mut gp = GameportState::new();
        gp.update_report(GamepadReport {
            axes: [0, 255, 128, 128],
            axis_mask: 0x03,
            buttons: 0b1001,
        });
        // Active-low buttons in bits 4..7: !0b1001 & 0xF = 0b0110 -> 0x60
        assert_eq!(gp.bios_buttons_byte(), 0x60);
        assert_eq!(gp.bios_axes(), [0, 255, 0, 0]);

        // Before trigger: connected axes 0..1 are 0, disconnected axes 2..3 are 1 -> 0x6C
        assert_eq!(gp.read(1000), 0x6C);

        // Trigger at tick 1000: all 4 axis bits high immediately (0x6F)
        gp.write(1000);
        assert_eq!(gp.read(1000), 0x6F);
        assert_eq!(gp.read(1000 + t_min - 1), 0x6F);

        // At t0 + t_min: X1 finishes (bit 0 -> 0), Y1 still charging (bit 1 -> 1), X2/Y2 stay 1 -> 0x6E
        assert_eq!(gp.read(1000 + t_min), 0x6E);
        assert_eq!(gp.read(1000 + t_max - 1), 0x6E);

        // At t0 + t_max: Y1 finishes (bit 1 -> 0), X2/Y2 stay 1 -> 0x6C
        assert_eq!(gp.read(1000 + t_max), 0x6C);
        assert!(!gp.triggered);

        // 4-axis stick with all buttons released (0xF0) and PIT wraparound across u32::MAX
        gp.update_report(GamepadReport {
            axes: [0, 128, 255, 128],
            axis_mask: 0x0F,
            buttons: 0,
        });
        let start = u32::MAX - 10;
        gp.write(start);
        assert_eq!(gp.read(start.wrapping_add(t_min - 1)), 0xFF);
        assert_eq!(gp.read(start.wrapping_add(t_min)), 0xFE); // X1 done
        assert_eq!(gp.read(start.wrapping_add(t_mid)), 0xF4); // X1, Y1, Y2 done; X2 still active
        assert_eq!(gp.read(start.wrapping_add(t_max)), 0xF0); // all 4 done
    }

    #[test]
    fn gameport_bios_int15_ah84_dispatch() {
        ohci_reset();
        let mut regs = [0u32; 19];
        regs[7] = 0x8400; // AH=84h
        regs[5] = 0x0000; // DX=0 (buttons)
        // Disconnected -> chains (returns 0)
        assert_eq!(unsafe { gate1_joy_int15(regs.as_mut_ptr()) }, 0);

        // Connect gamepad with axes [42, 200, 128, 128], 2-axis, button 2 pressed (0b0010)
        ohci_state().gameport.update_report(GamepadReport {
            axes: [42, 200, 128, 128],
            axis_mask: 0x03,
            buttons: 0b0010,
        });
        assert_eq!(gate1_joy_present(), 1);

        // DX=0: read buttons -> AL = (!0b0010 & 0xF) << 4 = 0xD0, CF cleared
        regs[12] = 1; // CF set initially
        assert_eq!(unsafe { gate1_joy_int15(regs.as_mut_ptr()) }, 1);
        assert_eq!(regs[7] & 0xFF, 0xD0);
        assert_eq!(regs[12] & 1, 0);

        // DX=1: read axes -> AX=42, BX=200, CX=0, DX=0, CF cleared
        regs[7] = 0x8400;
        regs[5] = 0x0001;
        regs[12] = 1;
        assert_eq!(unsafe { gate1_joy_int15(regs.as_mut_ptr()) }, 1);
        assert_eq!(regs[7] & 0xFFFF, 42);
        assert_eq!(regs[4] & 0xFFFF, 200);
        assert_eq!(regs[6] & 0xFFFF, 0);
        assert_eq!(regs[5] & 0xFFFF, 0);
        assert_eq!(regs[12] & 1, 0);

        // Non-84h AH chains
        regs[7] = 0x8800;
        assert_eq!(unsafe { gate1_joy_int15(regs.as_mut_ptr()) }, 0);
        ohci_reset();
    }

    #[test]
    fn usb_mouse_scroll_wheel_decoding_and_usbdelay_clamping() {
        // 3-byte boot mouse report -> dz == 0
        assert_eq!(decode_usb_mouse_report(&[0x01, 0x05, 0xFB], 3), Some((5, -5, 0, 1)));
        // 4-byte mouse report with max_pkt=3 -> ignores 4th byte
        assert_eq!(decode_usb_mouse_report(&[0x02, 0x00, 0x00, 0xFF], 3), Some((0, 0, 0, 2)));
        // 4-byte mouse report with max_pkt=4 -> decodes signed dz (+1 scroll up, -2 scroll down)
        assert_eq!(decode_usb_mouse_report(&[0x00, 0x01, 0xFF, 0x01], 4), Some((1, -1, 1, 0)));
        assert_eq!(decode_usb_mouse_report(&[0x04, 0x00, 0x00, 0xFE], 4), Some((0, 0, -2, 4)));
        // Too short -> None
        assert_eq!(decode_usb_mouse_report(&[0x00, 0x01], 4), None);

        // usbdelay clamping (1..=20) and FFI getter/setter
        ohci_reset();
        assert_eq!(clamp_usb_delay(0), 1);
        assert_eq!(clamp_usb_delay(1), 1);
        assert_eq!(clamp_usb_delay(7), 7);
        assert_eq!(clamp_usb_delay(20), 20);
        assert_eq!(clamp_usb_delay(99), 20);
        assert_eq!(scale_usb_delay_ms(50, 0), 50);
        assert_eq!(scale_usb_delay_ms(50, 5), 250);
        assert_eq!(scale_usb_delay_ms(50, 99), 1000);

        assert_eq!(gate1_usb_get_delay(), 1);
        assert_eq!(gate1_usb_set_delay(0), 1);
        assert_eq!(gate1_usb_set_delay(8), 8);
        assert_eq!(gate1_usb_get_delay(), 8);
        assert_eq!(gate1_usb_set_delay(50), 20);
        assert_eq!(gate1_usb_get_delay(), 20);
        ohci_reset();
    }
}
