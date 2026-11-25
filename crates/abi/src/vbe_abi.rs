//! Portable VBE + 2D Acceleration JLM ABI (`VBE.JLM`, DDB device ID `0xFD04`).
//!
//! Standalone, `#![no_std]`, `#[repr(C)]` binary contract shared between
//! `VBE.JLM` and real-mode (V86) or 32-bit protected-mode (DPMI/DJGPP) clients
//! such as an SDL3 DOS video backend.

/// JEMM DDB device ID reserved for the standalone VBE + 2D Acceleration JLM (`VBE.JLM`).
pub const FERRIC_VBE_DDB_ID: u16 = 0xFD04;

/// Portable VBE JLM ABI version (`major << 16 | minor`): v1.0.
pub const FERRIC_VBE_ABI_VERSION: u32 = 0x0001_0000;

/// V86 / PM32 API opcodes dispatched via `INT 2Fh AX=1684h BX=0xFD04` entry point.
/// - `AX = 0x0000`: Ping -> `AX = 0x0001`, `BX = 0xFD04`
/// - `AX = 0x0001`: GetCaps (`ES:DI` in V86, flat `EDI` in PM32 -> [`VbeCaps`])
/// - `AX = 0x0002`: SetLfb (`ES:DI` in V86, flat `EDI` in PM32 -> [`VbeModeDesc`])
/// - `AX = 0x0003`: FillRect (`ES:DI` in V86, flat `EDI` in PM32 -> [`VbeFillReq`])
/// - `AX = 0x0004`: BitBlt (`ES:DI` in V86, flat `EDI` in PM32 -> [`VbeBlitReq`])
/// - `AX = 0x0005`: Sync (wait for hardware 2D engine idle)
/// - `AX = 0x0006`: ReleaseHw (sync GE and disable MMIO bit before `INT 10h` mode switch)
pub const VBE_OP_PING: u16 = 0x0000;
pub const VBE_OP_GET_CAPS: u16 = 0x0001;
pub const VBE_OP_SET_LFB: u16 = 0x0002;
pub const VBE_OP_FILL_RECT: u16 = 0x0003;
pub const VBE_OP_BIT_BLT: u16 = 0x0004;
pub const VBE_OP_SYNC: u16 = 0x0005;
pub const VBE_OP_RELEASE_HW: u16 = 0x0006;

/// Status codes returned in `EAX` / `AX` by `VBE.JLM` operations.
pub const VBE_OK: u32 = 0;
pub const VBE_ERR_INVALID_PTR: u32 = 1;
pub const VBE_ERR_NOT_CONFIGURED: u32 = 2;
pub const VBE_ERR_INVALID_MODE: u32 = 3;
pub const VBE_ERR_INVALID_RECT: u32 = 4;
pub const VBE_ERR_MAP_FAILED: u32 = 5;
pub const VBE_ERR_UNSUPPORTED: u32 = 6;
pub const VBE_ERR_HW_TIMEOUT: u32 = 7;

/// `VbeCaps.caps_flags` bit definitions.
pub const VBE_CAP_SW_2D: u32 = 1 << 0;
pub const VBE_CAP_HW_TRIDENT_2D: u32 = 1 << 1;
pub const VBE_CAP_FILL_RECT: u32 = 1 << 2;
pub const VBE_CAP_BIT_BLT: u32 = 1 << 3;
pub const VBE_CAP_LFB_MAPPED: u32 = 1 << 4;
pub const VBE_CAP_PM32_API: u32 = 1 << 5;

/// `VbeCaps.accel_backend` values.
pub const VBE_ACCEL_NONE: u8 = 0;
pub const VBE_ACCEL_SW: u8 = 1;
pub const VBE_ACCEL_TRIDENT_8820: u8 = 2;

/// Standard raster operation codes (Windows ROP3 subset).
pub const VBE_ROP_SRCCOPY: u32 = 0xCC;
pub const VBE_ROP_PATCOPY: u32 = 0xF0;

/// Maximum LFB size accepted by `SetLfb` (32 MiB ceiling).
pub const VBE_MAX_LFB_SIZE: u32 = 32 * 1024 * 1024;

/// Linear Framebuffer mode descriptor passed to `VBE_OP_SET_LFB`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct VbeModeDesc {
    /// Horizontal resolution in pixels (1..=4096).
    pub width: u16,
    /// Vertical resolution in pixels (1..=4096).
    pub height: u16,
    /// Scanline stride in bytes (`>= width * bytes_per_pixel`).
    pub pitch: u32,
    /// Bits per pixel (`8`, `16`, `24`, or `32`).
    pub bpp: u8,
    /// Red channel bit shift (e.g. 11 for RGB565, 16 for XRGB8888).
    pub red_shift: u8,
    /// Green channel bit shift (e.g. 5 for RGB565, 8 for XRGB8888).
    pub green_shift: u8,
    /// Blue channel bit shift (e.g. 0 for RGB565/XRGB8888).
    pub blue_shift: u8,
    /// Physical base address of the VBE Linear Framebuffer (4 KiB aligned).
    pub lfb_phys: u32,
    /// Total LFB size in bytes (`>= pitch * height`, `<= VBE_MAX_LFB_SIZE`).
    pub lfb_size: u32,
    /// VBE mode number (informational, e.g. `0x4118`).
    pub mode_num: u16,
    /// Reserved for future flags; must be 0.
    pub reserved: u16,
}

impl VbeModeDesc {
    pub const SIZE: usize = 24;

    pub const fn empty() -> Self {
        Self {
            width: 0,
            height: 0,
            pitch: 0,
            bpp: 0,
            red_shift: 0,
            green_shift: 0,
            blue_shift: 0,
            lfb_phys: 0,
            lfb_size: 0,
            mode_num: 0,
            reserved: 0,
        }
    }

    /// Returns bytes per pixel for supported packed-pixel formats, or `None`.
    #[inline]
    pub const fn bytes_per_pixel(&self) -> Option<u32> {
        match self.bpp {
            8 => Some(1),
            16 => Some(2),
            24 => Some(3),
            32 => Some(4),
            _ => None,
        }
    }

    /// Validates dimensions, stride, bpp, and physical LFB bounds with checked arithmetic.
    pub const fn validate(&self) -> Result<(), u32> {
        if self.width == 0 || self.height == 0 || self.width > 4096 || self.height > 4096 {
            return Err(VBE_ERR_INVALID_MODE);
        }
        let Some(bytes_pp) = self.bytes_per_pixel() else {
            return Err(VBE_ERR_INVALID_MODE);
        };
        let Some(min_pitch) = (self.width as u32).checked_mul(bytes_pp) else {
            return Err(VBE_ERR_INVALID_MODE);
        };
        if self.pitch < min_pitch {
            return Err(VBE_ERR_INVALID_MODE);
        }
        let Some(min_size) = self.pitch.checked_mul(self.height as u32) else {
            return Err(VBE_ERR_INVALID_MODE);
        };
        if self.lfb_phys == 0 || (self.lfb_phys & 0x0FFF) != 0 {
            return Err(VBE_ERR_INVALID_MODE);
        }
        if self.lfb_size < min_size || self.lfb_size > VBE_MAX_LFB_SIZE {
            return Err(VBE_ERR_INVALID_MODE);
        }
        if self.lfb_phys.checked_add(self.lfb_size).is_none() {
            return Err(VBE_ERR_INVALID_MODE);
        }
        Ok(())
    }
}

/// Capabilities and active state descriptor filled by `VBE_OP_GET_CAPS`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct VbeCaps {
    /// ABI version (`FERRIC_VBE_ABI_VERSION`).
    pub abi_version: u32,
    /// Bitmask of `VBE_CAP_*` flags.
    pub caps_flags: u32,
    /// Detected PCI display controller vendor ID (e.g. `0x1023`), or `0`.
    pub pci_vendor: u16,
    /// Detected PCI display controller device ID (e.g. `0x8820`), or `0`.
    pub pci_device: u16,
    /// PCI revision ID.
    pub pci_rev: u8,
    /// Active 2D accelerator backend (`VBE_ACCEL_*`).
    pub accel_backend: u8,
    /// Reserved (0).
    pub reserved: u16,
    /// Physical base address of 2D GE MMIO BAR (e.g. Trident BAR1), or `0`.
    pub mmio_phys: u32,
    /// Currently configured LFB physical address, or `0`.
    pub lfb_phys: u32,
    /// Currently configured LFB size in bytes, or `0`.
    pub lfb_size: u32,
    /// Active mode width in pixels.
    pub width: u16,
    /// Active mode height in pixels.
    pub height: u16,
    /// Active mode pitch in bytes.
    pub pitch: u32,
    /// Active mode bits per pixel.
    pub bpp: u8,
    /// Padding to 4-byte alignment.
    pub pad: [u8; 3],
}

impl VbeCaps {
    pub const SIZE: usize = 40;

    pub const fn empty() -> Self {
        Self {
            abi_version: FERRIC_VBE_ABI_VERSION,
            caps_flags: 0,
            pci_vendor: 0,
            pci_device: 0,
            pci_rev: 0,
            accel_backend: VBE_ACCEL_NONE,
            reserved: 0,
            mmio_phys: 0,
            lfb_phys: 0,
            lfb_size: 0,
            width: 0,
            height: 0,
            pitch: 0,
            bpp: 0,
            pad: [0; 3],
        }
    }
}

/// Solid-color rectangle fill request passed to `VBE_OP_FILL_RECT`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct VbeFillReq {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
    /// Packed pixel color in the active mode's native format (low 8/16/24/32 bits used).
    pub color: u32,
    /// Raster operation (0 or `VBE_ROP_PATCOPY` / `VBE_ROP_SRCCOPY` for solid fill).
    pub rop: u32,
}

impl VbeFillReq {
    pub const SIZE: usize = 16;

    /// Validates that the rectangle lies completely within `mode`'s dimensions.
    pub const fn validate_bounds(&self, mode: &VbeModeDesc) -> Result<(), u32> {
        let Some(x_end) = (self.x as u32).checked_add(self.w as u32) else {
            return Err(VBE_ERR_INVALID_RECT);
        };
        let Some(y_end) = (self.y as u32).checked_add(self.h as u32) else {
            return Err(VBE_ERR_INVALID_RECT);
        };
        if x_end > mode.width as u32 || y_end > mode.height as u32 {
            return Err(VBE_ERR_INVALID_RECT);
        }
        Ok(())
    }
}

/// Screen-to-screen BitBLT request passed to `VBE_OP_BIT_BLT`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct VbeBlitReq {
    pub src_x: u16,
    pub src_y: u16,
    pub dst_x: u16,
    pub dst_y: u16,
    pub w: u16,
    pub h: u16,
    /// Raster operation (0 or `VBE_ROP_SRCCOPY` for straight copy).
    pub rop: u32,
}

impl VbeBlitReq {
    pub const SIZE: usize = 16;

    /// Validates that both source and destination rectangles lie within `mode`'s dimensions.
    pub const fn validate_bounds(&self, mode: &VbeModeDesc) -> Result<(), u32> {
        let Some(sx_end) = (self.src_x as u32).checked_add(self.w as u32) else {
            return Err(VBE_ERR_INVALID_RECT);
        };
        let Some(sy_end) = (self.src_y as u32).checked_add(self.h as u32) else {
            return Err(VBE_ERR_INVALID_RECT);
        };
        let Some(dx_end) = (self.dst_x as u32).checked_add(self.w as u32) else {
            return Err(VBE_ERR_INVALID_RECT);
        };
        let Some(dy_end) = (self.dst_y as u32).checked_add(self.h as u32) else {
            return Err(VBE_ERR_INVALID_RECT);
        };
        if sx_end > mode.width as u32
            || sy_end > mode.height as u32
            || dx_end > mode.width as u32
            || dy_end > mode.height as u32
        {
            return Err(VBE_ERR_INVALID_RECT);
        }
        Ok(())
    }
}

/// Check whether a provider's VBE ABI version satisfies `(req_major, req_minor)`.
#[inline]
pub const fn vbe_abi_compatible(provider_ver: u32, req_major: u16, req_minor: u16) -> bool {
    let prov_major = (provider_ver >> 16) as u16;
    let prov_minor = (provider_ver & 0xFFFF) as u16;
    prov_major == req_major && prov_minor >= req_minor
}

// Compile-time layout invariants (verified on both i386-unknown-none and host targets).
const _: () = {
    assert!(core::mem::size_of::<VbeModeDesc>() == VbeModeDesc::SIZE);
    assert!(core::mem::align_of::<VbeModeDesc>() == 4);
    assert!(core::mem::size_of::<VbeCaps>() == VbeCaps::SIZE);
    assert!(core::mem::align_of::<VbeCaps>() == 4);
    assert!(core::mem::size_of::<VbeFillReq>() == VbeFillReq::SIZE);
    assert!(core::mem::align_of::<VbeFillReq>() == 4);
    assert!(core::mem::size_of::<VbeBlitReq>() == VbeBlitReq::SIZE);
    assert!(core::mem::align_of::<VbeBlitReq>() == 4);
};

#[cfg(test)]
mod tests {
    use super::*;
    use core::mem::size_of;

    fn sample_mode_640x480x32() -> VbeModeDesc {
        VbeModeDesc {
            width: 640,
            height: 480,
            pitch: 640 * 4,
            bpp: 32,
            red_shift: 16,
            green_shift: 8,
            blue_shift: 0,
            lfb_phys: 0xE000_0000,
            lfb_size: 640 * 480 * 4,
            mode_num: 0x4112,
            reserved: 0,
        }
    }

    #[test]
    fn vbe_abi_struct_sizes_and_constants() {
        assert_eq!(FERRIC_VBE_DDB_ID, 0xFD04);
        assert_eq!(FERRIC_VBE_ABI_VERSION, 0x0001_0000);
        assert_eq!(size_of::<VbeModeDesc>(), 24);
        assert_eq!(size_of::<VbeCaps>(), 40);
        assert_eq!(size_of::<VbeFillReq>(), 16);
        assert_eq!(size_of::<VbeBlitReq>(), 16);
        assert!(vbe_abi_compatible(FERRIC_VBE_ABI_VERSION, 1, 0));
        assert!(!vbe_abi_compatible(FERRIC_VBE_ABI_VERSION, 1, 1));
        assert!(!vbe_abi_compatible(FERRIC_VBE_ABI_VERSION, 2, 0));
    }

    #[test]
    fn vbe_mode_desc_validation() {
        let good = sample_mode_640x480x32();
        assert_eq!(good.bytes_per_pixel(), Some(4));
        assert_eq!(good.validate(), Ok(()));

        let bad_bpp = VbeModeDesc { bpp: 15, ..good };
        assert_eq!(bad_bpp.validate(), Err(VBE_ERR_INVALID_MODE));

        let bad_pitch = VbeModeDesc {
            pitch: 640 * 4 - 4,
            ..good
        };
        assert_eq!(bad_pitch.validate(), Err(VBE_ERR_INVALID_MODE));

        let unaligned_lfb = VbeModeDesc {
            lfb_phys: 0xE000_0004,
            ..good
        };
        assert_eq!(unaligned_lfb.validate(), Err(VBE_ERR_INVALID_MODE));

        let too_small = VbeModeDesc {
            lfb_size: 4096,
            ..good
        };
        assert_eq!(too_small.validate(), Err(VBE_ERR_INVALID_MODE));

        let wrap_phys = VbeModeDesc {
            lfb_phys: 0xFFFF_F000,
            ..good
        };
        assert_eq!(wrap_phys.validate(), Err(VBE_ERR_INVALID_MODE));
    }

    #[test]
    fn vbe_fill_and_blit_bounds_validation() {
        let mode = sample_mode_640x480x32();
        let ok_fill = VbeFillReq {
            x: 600,
            y: 440,
            w: 40,
            h: 40,
            color: 0x00FF_00FF,
            rop: VBE_ROP_PATCOPY,
        };
        assert_eq!(ok_fill.validate_bounds(&mode), Ok(()));

        let oob_fill = VbeFillReq { w: 41, ..ok_fill };
        assert_eq!(oob_fill.validate_bounds(&mode), Err(VBE_ERR_INVALID_RECT));

        let ok_blit = VbeBlitReq {
            src_x: 10,
            src_y: 20,
            dst_x: 600,
            dst_y: 440,
            w: 40,
            h: 40,
            rop: VBE_ROP_SRCCOPY,
        };
        assert_eq!(ok_blit.validate_bounds(&mode), Ok(()));

        let oob_blit = VbeBlitReq { dst_y: 441, ..ok_blit };
        assert_eq!(oob_blit.validate_bounds(&mode), Err(VBE_ERR_INVALID_RECT));
    }
}
