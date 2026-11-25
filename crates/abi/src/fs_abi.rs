//! Portable Block-Transport & Filesystem-Module ABI (Stage F).
//!
//! Designed to be standalone and `#![no_std]` so individual filesystem JLMs
//! (`EXFAT.JLM`, `NTFS.JLM`, or third-party modules) and block-transport JLMs
//! (`FERRIC.JLM` USB OHCI, future SD/IDE/SATA transports) can live in separate
//! public repositories while sharing one versioned binary contract over JEMM's
//! native Ring-0 `INT 20h` DDB service table.

/// JEMM DDB device ID for the Ferric DOS core block transport (`FERRIC.JLM`).
pub const FERRIC_CORE_DDB_ID: u16 = 0xFD01;

/// JEMM DDB device ID reserved for the standalone exFAT module (`EXFAT.JLM`).
pub const FERRIC_EXFAT_DDB_ID: u16 = 0xFD02;

/// JEMM DDB device ID reserved for the standalone NTFS module (`NTFS.JLM`).
pub const FERRIC_NTFS_DDB_ID: u16 = 0xFD03;

/// Portable Block-Transport ABI version (`major << 16 | minor`): v1.0.
pub const FERRIC_BLK_ABI_VERSION: u32 = 0x0001_0000;

/// JEMM `INT 20h` DDB service-table indices exported by `FERRIC_CORE_DDB_ID`:
/// - `0`: `extern "C" fn() -> u32` (`ferric_blk_abi_version`)
/// - `1`: `extern "C" fn(out_lin: u32) -> u32` (`ferric_blk_info`, writes 48B `BlkDevInfo`)
/// - `2`: `extern "C" fn(lba: u32, dst_lin: u32) -> u32` (`ferric_blk_read_sec`)
/// - `3`: `extern "C" fn(lba: u32, src_lin: u32) -> u32` (`ferric_blk_write_sec`)
/// - `4`: `extern "C" fn() -> u32` (`ferric_blk_flush`)
/// - `5`: `extern "C" fn() -> u32` (`ferric_blk_media_check`)
pub const BLK_SVC_ABI_VERSION: u16 = 0;
pub const BLK_SVC_DEV_INFO: u16 = 1;
pub const BLK_SVC_READ_SEC: u16 = 2;
pub const BLK_SVC_WRITE_SEC: u16 = 3;
pub const BLK_SVC_FLUSH: u16 = 4;
pub const BLK_SVC_MEDIA_CHECK: u16 = 5;
pub const BLK_SVC_COUNT: u32 = 6;

/// `BlkDevInfo.flags` bit definitions.
pub const BLK_FLAG_ACTIVE: u8 = 1 << 0;
pub const BLK_FLAG_READY: u8 = 1 << 1;
pub const BLK_FLAG_EJECTED: u8 = 1 << 2;
pub const BLK_FLAG_MEDIA_CHANGED: u8 = 1 << 3;
/// Secondary non-FAT partition presence bits (for multi-partition media, e.g. FAT16 + exFAT).
pub const BLK_FLAG_HAS_EXFAT: u8 = 1 << 4;
pub const BLK_FLAG_HAS_NTFS: u8 = 1 << 5;

/// Fail-closed filesystem classification returned by the core block transport
/// (`AX=0x0035` and `BlkDevInfo.fs_type`). Ambiguous or unrecognized media
/// always report `FsType::Unknown` (0), never a guess.
#[repr(u8)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum FsType {
    Unknown = 0,
    Fat12 = 1,
    Fat16 = 2,
    Fat32 = 3,
    ExFat = 4,
    Ntfs = 5,
}

impl FsType {
    #[inline]
    pub const fn as_u8(self) -> u8 {
        self as u8
    }

    #[inline]
    pub const fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Fat12,
            2 => Self::Fat16,
            3 => Self::Fat32,
            4 => Self::ExFat,
            5 => Self::Ntfs,
            _ => Self::Unknown,
        }
    }

    #[inline]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "UNKNOWN",
            Self::Fat12 => "FAT12",
            Self::Fat16 => "FAT16",
            Self::Fat32 => "FAT32",
            Self::ExFat => "EXFAT",
            Self::Ntfs => "NTFS",
        }
    }

    /// Optional JEMM DDB device ID of the loadable filesystem module that
    /// serves this filesystem (`None` for core-resident FAT or unknown media).
    #[inline]
    pub const fn module_ddb_id(self) -> Option<u16> {
        match self {
            Self::ExFat => Some(FERRIC_EXFAT_DDB_ID),
            Self::Ntfs => Some(FERRIC_NTFS_DDB_ID),
            _ => None,
        }
    }
}

/// 48-byte transport-agnostic block device descriptor filled by `BLK_SVC_DEV_INFO`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct BlkDevInfo {
    pub abi_version: u32,
    pub total_sectors: u32,
    pub sector_size: u32,
    pub media_gen: u16,
    pub fs_type: u8,
    pub flags: u8,
    pub vendor: [u8; 8],
    pub product: [u8; 16],
    pub reserved: [u8; 8],
}

impl BlkDevInfo {
    pub const SIZE: usize = 48;

    pub const fn empty() -> Self {
        Self {
            abi_version: FERRIC_BLK_ABI_VERSION,
            total_sectors: 0,
            sector_size: 0,
            media_gen: 0,
            fs_type: FsType::Unknown as u8,
            flags: 0,
            vendor: [0; 8],
            product: [0; 16],
            reserved: [0; 8],
        }
    }
}

/// Check whether a core transport's `abi_version` satisfies a module's
/// required `(req_major, req_minor)` contract.
#[inline]
pub const fn blk_abi_compatible(provider_ver: u32, req_major: u16, req_minor: u16) -> bool {
    let prov_major = (provider_ver >> 16) as u16;
    let prov_minor = (provider_ver & 0xFFFF) as u16;
    prov_major == req_major && prov_minor >= req_minor
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    #[test]
    fn blk_dev_info_layout_is_48_bytes() {
        assert_eq!(size_of::<BlkDevInfo>(), BlkDevInfo::SIZE);
        let d = BlkDevInfo::empty();
        assert_eq!(d.abi_version, FERRIC_BLK_ABI_VERSION);
        assert_eq!(FsType::from_u8(d.fs_type), FsType::Unknown);
    }

    #[test]
    fn fs_type_round_trips_and_maps_modules() {
        for v in 0u8..=5 {
            assert_eq!(FsType::from_u8(v).as_u8(), v);
        }
        assert_eq!(FsType::from_u8(99), FsType::Unknown);
        assert_eq!(FsType::Fat16.module_ddb_id(), None);
        assert_eq!(FsType::ExFat.module_ddb_id(), Some(FERRIC_EXFAT_DDB_ID));
        assert_eq!(FsType::Ntfs.module_ddb_id(), Some(FERRIC_NTFS_DDB_ID));
        assert!(blk_abi_compatible(FERRIC_BLK_ABI_VERSION, 1, 0));
        assert!(!blk_abi_compatible(FERRIC_BLK_ABI_VERSION, 2, 0));
        assert!(!blk_abi_compatible(FERRIC_BLK_ABI_VERSION, 1, 1));
    }
}
