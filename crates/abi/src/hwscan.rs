//! Gate 0 read-only HWSCAN inventory record types, headers, and constants.
//!
//! All HWSCAN records are strictly read-only hardware/firmware probes.
//! No storage write commands or MBR/FAT modification opcodes exist in this ABI.

use crate::{AbiVersion, StatusCode};

/// ASCII `"HSCN"` in little-endian for HWSCAN inventory streams.
pub const HWSCAN_MAGIC: u32 = u32::from_le_bytes(*b"HSCN");

/// ABI opcode: query Gate 0 HWSCAN inventory (read-only).
pub const OP_HWSCAN_QUERY: u16 = 0x0001;

/// Platform tier classification determined by HWSCAN (Spec Draft 0.3.1, B1).
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum PlatformTier {
    Unknown = 0,
    /// ISA-era 386/486 baseline: boot, Recovery, Games core.
    Isa386Or486 = 1,
    /// PCI Pentium-class: adds UHCI/OHCI USB and PCI Ethernet.
    PciPentium = 2,
    /// Flagship tier (e.g. DynaBook SS 2000M, qualified ATX desktop).
    Flagship = 3,
}

/// Discriminator for typed records in an HWSCAN inventory stream.
#[repr(u16)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum HwscanRecordKind {
    InventoryHeader = 0x0001,
    CpuInfo = 0x0002,
    FpuInfo = 0x0003,
    MemoryRegion = 0x0004,
    BiosEddProbe = 0x0005,
    PciDevice = 0x0006,
    PowerInterface = 0x0007,
    SerialProbe = 0x0008,
}

/// Common 16-byte header preceding every HWSCAN record payload.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct RecordHeader {
    pub kind: u16,
    pub version: u16,
    pub flags: u32,
    pub payload_len: u32,
    pub status: u32,
}

impl RecordHeader {
    pub const SIZE: usize = core::mem::size_of::<Self>();

    #[inline]
    pub const fn new(kind: HwscanRecordKind, payload_len: u32, status: StatusCode) -> Self {
        Self {
            kind: kind as u16,
            version: 1,
            flags: 0,
            payload_len,
            status: status.as_u32(),
        }
    }
}

/// Top-level HWSCAN inventory header.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct InventoryHeader {
    pub magic: u32,
    pub version: AbiVersion,
    pub record_count: u32,
    pub total_bytes: u32,
    pub tier: u32,
    pub reserved: u32,
    pub build_epoch: u64,
}

// CPU feature flags detected by flag probes and (when available) CPUID.
pub const CPU_FLAG_CPUID: u32 = 1 << 0;
pub const CPU_FLAG_FPU: u32 = 1 << 1;
pub const CPU_FLAG_TSC: u32 = 1 << 2;
pub const CPU_FLAG_MSR: u32 = 1 << 3;
pub const CPU_FLAG_APIC: u32 = 1 << 4;
pub const CPU_FLAG_CMOV: u32 = 1 << 5;
pub const CPU_FLAG_MMX: u32 = 1 << 6;
pub const CPU_FLAG_SSE: u32 = 1 << 7;
pub const CPU_FLAG_SSE2: u32 = 1 << 8;

/// CPU inventory record (flag-probed first for 386/486 safety, then CPUID if present).
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct CpuInfoRecord {
    /// Family number (3 = 386, 4 = 486, 5 = Pentium, 6 = P6/Pentium III, etc.).
    pub family: u8,
    /// Non-zero if EFLAGS.ID toggled and CPUID is safe to execute.
    pub cpuid_supported: u8,
    pub model: u8,
    pub stepping: u8,
    pub max_cpuid_leaf: u32,
    pub cpuid_edx: u32,
    pub cpuid_ecx: u32,
    pub feature_flags: u32,
    /// 12-byte ASCII CPUID vendor string (zero-filled if CPUID absent).
    pub vendor: [u8; 12],
}

/// FPU type constants.
pub const FPU_TYPE_NONE: u8 = 0;
pub const FPU_TYPE_287: u8 = 1;
pub const FPU_TYPE_387: u8 = 2;
pub const FPU_TYPE_INTEGRATED: u8 = 3;

/// x87 FPU inventory record (387 is optional hardware on the 386 baseline tier).
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct FpuInfoRecord {
    pub present: u8,
    pub fpu_type: u8,
    pub reserved: u16,
    pub control_word: u16,
    pub status_word: u16,
}

// Memory map discovery sources.
pub const MEM_SOURCE_E820: u32 = 1;
pub const MEM_SOURCE_E801: u32 = 2;
pub const MEM_SOURCE_INT15_88: u32 = 3;
pub const MEM_SOURCE_INT12: u32 = 4;

/// Single memory map region (E820 or legacy BIOS fallback).
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct MemoryRegionRecord {
    pub base: u64,
    pub length: u64,
    /// E820 region type (1 = usable RAM, 2 = reserved, 3 = ACPI reclaim, 4 = ACPI NVS, 5 = bad).
    pub region_type: u32,
    pub acpi_attrs: u32,
    pub source: u32,
    pub reserved: u32,
}

/// Read-only BIOS INT 13h EDD/LBA probe record (`AH=41h`, `AH=48h`, `AH=08h` only).
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct BiosEddProbeRecord {
    /// BIOS drive number (e.g., `0x80` for first fixed disk).
    pub drive_number: u8,
    /// Non-zero if `INT 13h AH=41h` returned `BX=0xAA55` with CF=0.
    pub edd_present: u8,
    /// EDD major version from `AH` on `INT 13h AH=41h` success.
    pub edd_version: u8,
    /// Raw BIOS status byte returned in `AH` on error (0 on success).
    pub bios_status: u8,
    /// Interface support bitmap from `CX` (`bit 0` = packet/LBA read-write subset).
    pub interface_bitmap: u16,
    /// Logical bytes per sector reported by `AH=48h` (typically 512).
    pub bytes_per_sector: u16,
    /// Total 64-bit sector count from `AH=48h` (0 if EDD absent).
    pub total_sectors: u64,
    pub legacy_cylinders: u32,
    pub legacy_heads: u32,
    pub legacy_sectors_per_track: u32,
    pub reserved: u32,
}

/// PCI configuration space device record (read-only Type 1 / PCI BIOS scan).
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct PciDeviceRecord {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub revision_id: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class_code: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
    pub subsystem_vendor_id: u16,
    pub subsystem_id: u16,
    pub irq_line: u8,
    pub irq_pin: u8,
    pub command_reg: u16,
    pub status_reg: u16,
    pub reserved: u16,
    pub bars: [u32; 6],
}

/// Firmware power interface inventory record (APM 1.2, static ACPI RSDP, VBE/PM 1.0).
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct PowerInterfaceRecord {
    pub apm_present: u8,
    pub apm_major: u8,
    pub apm_minor: u8,
    pub apm_flags: u8,
    pub acpi_rsdp_present: u8,
    pub acpi_revision: u8,
    pub vbe_pm_present: u8,
    pub vbe_pm_caps: u8,
    pub acpi_rsdp_phys: u32,
    pub ac_status: u8,
    pub battery_status: u8,
    pub battery_percent: u8,
    pub reserved: u8,
    pub battery_remaining_secs: u32,
}

// Serial UART probe outcomes (Spec Draft 0.3.1: BDA 0040:0000 + scratch/LSR probe).
pub const SERIAL_PROBE_ABSENT: u8 = 0;
pub const SERIAL_PROBE_BDA_ZERO: u8 = 1;
pub const SERIAL_PROBE_SCRATCH_FAILED: u8 = 2;
pub const SERIAL_PROBE_LSR_TIMEOUT: u8 = 3;
pub const SERIAL_PROBE_LIVE_UART: u8 = 4;

/// COM1 UART probe record verifying a live UART before mirroring serial output.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SerialProbeRecord {
    /// I/O port read from BDA `0040:0000` (0 if absent, e.g. DynaBook SS 2000M).
    pub bda_com1_port: u16,
    /// Outcome code (`SERIAL_PROBE_*`).
    pub probe_status: u8,
    pub lsr_value: u8,
    pub scratch_readback: u8,
    pub reserved: [u8; 3],
}

// Compile-time layout invariants for all Gate 0 HWSCAN records.
const _: () = {
    assert!(core::mem::size_of::<RecordHeader>() == 16);
    assert!(core::mem::size_of::<InventoryHeader>() == 32);
    assert!(core::mem::size_of::<CpuInfoRecord>() == 32);
    assert!(core::mem::size_of::<FpuInfoRecord>() == 8);
    assert!(core::mem::size_of::<MemoryRegionRecord>() == 32);
    assert!(core::mem::size_of::<BiosEddProbeRecord>() == 32);
    assert!(core::mem::size_of::<PciDeviceRecord>() == 48);
    assert!(core::mem::size_of::<PowerInterfaceRecord>() == 20);
    assert!(core::mem::size_of::<SerialProbeRecord>() == 8);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hwscan_record_header_construction() {
        let hdr = RecordHeader::new(
            HwscanRecordKind::BiosEddProbe,
            core::mem::size_of::<BiosEddProbeRecord>() as u32,
            StatusCode::Ok,
        );
        assert_eq!(hdr.kind, HwscanRecordKind::BiosEddProbe as u16);
        assert_eq!(hdr.version, 1);
        assert_eq!(hdr.payload_len, 32);
        assert_eq!(hdr.status, 0);
    }

    #[test]
    fn hwscan_sizes_and_alignment() {
        assert_eq!(core::mem::size_of::<RecordHeader>(), 16);
        assert_eq!(core::mem::size_of::<InventoryHeader>(), 32);
        assert_eq!(core::mem::size_of::<CpuInfoRecord>(), 32);
        assert_eq!(core::mem::size_of::<FpuInfoRecord>(), 8);
        assert_eq!(core::mem::size_of::<MemoryRegionRecord>(), 32);
        assert_eq!(core::mem::size_of::<BiosEddProbeRecord>(), 32);
        assert_eq!(core::mem::size_of::<PciDeviceRecord>(), 48);
        assert_eq!(core::mem::size_of::<PowerInterfaceRecord>(), 20);
        assert_eq!(core::mem::size_of::<SerialProbeRecord>(), 8);
    }
}
