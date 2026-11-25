//! Standalone VBE + 2D Acceleration JLM (`VBE.JLM`, DDB device ID `0xFD04`).
//!
//! Repo-portable module depending ONLY on `crates/abi` (ZERO dependency on
//! `gate1-svc`). Compiled for `targets/i386-unknown-none.json`
//! (`-x87,-mmx,-sse,-sse2`, `panic = "abort"`):
//! - Strictly read-only with respect to storage (ZERO storage writes, ZERO INT 21h).
//! - Clean software LFB 2D path (`FillRect`, `BitBlt`, `GetCaps`, `SetLfb`, `Sync`).
//! - Trident CyberBlade XP-Ai1 (`1023:8820`) PCI BAR1 MMIO BladeXP 2D GE acceleration
//!   dispatched behind [`trident_2d::Accel2d`] with automatic fallback to `sw_2d`.

#![cfg_attr(not(test), no_std)]
#![deny(warnings)]

pub mod sw_2d;
pub mod trident_2d;

use abi::vbe_abi::{
    FERRIC_VBE_ABI_VERSION, VBE_ACCEL_SW, VBE_ACCEL_TRIDENT_8820, VBE_CAP_BIT_BLT,
    VBE_CAP_FILL_RECT, VBE_CAP_HW_TRIDENT_2D, VBE_CAP_LFB_MAPPED, VBE_CAP_PM32_API,
    VBE_CAP_SW_2D, VBE_ERR_INVALID_PTR, VBE_ERR_MAP_FAILED, VBE_ERR_NOT_CONFIGURED, VBE_OK,
    VbeBlitReq, VbeCaps, VbeFillReq, VbeModeDesc,
};
use core::cell::UnsafeCell;
use trident_2d::{
    Accel2d, LiveMmioBus, TRIDENT_MMIO_PAGES, Trident2dEngine, TridentPciInfo, VgaPio,
};

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

// Ring-0 JEMM VMM physical-to-linear mapping helper provided by asm/vbe_jlm.asm.
#[cfg(not(test))]
unsafe extern "C" {
    fn vbe_map_mmio(phys: u32, npages: u32) -> u32;
}

#[cfg(test)]
unsafe fn vbe_map_mmio(_phys: u32, _npages: u32) -> u32 {
    0
}

#[cfg(target_arch = "x86")]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") val, options(nomem, nostack));
    }
    val
}

#[cfg(target_arch = "x86")]
unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
    }
}

#[cfg(target_arch = "x86")]
unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    unsafe {
        core::arch::asm!("in eax, dx", in("dx") port, out("eax") val, options(nomem, nostack));
    }
    val
}

#[cfg(target_arch = "x86")]
unsafe fn outl(port: u16, val: u32) {
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack));
    }
}

#[cfg(not(target_arch = "x86"))]
unsafe fn inb(_port: u16) -> u8 {
    0
}

#[cfg(not(target_arch = "x86"))]
unsafe fn outb(_port: u16, _val: u8) {}

#[cfg(not(target_arch = "x86"))]
unsafe fn inl(_port: u16) -> u32 {
    0xFFFF_FFFF
}

#[cfg(not(target_arch = "x86"))]
unsafe fn outl(_port: u16, _val: u32) {}

struct LiveVgaPio;

impl VgaPio for LiveVgaPio {
    #[inline]
    fn inb(&self, port: u16) -> u8 {
        unsafe { inb(port) }
    }

    #[inline]
    fn outb(&mut self, port: u16, val: u8) {
        unsafe { outb(port, val) }
    }
}

unsafe fn pci_read32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((off as u32) & 0xFC);
    unsafe {
        outl(0xCF8, addr);
        inl(0xCFC)
    }
}

struct VbeState {
    pci: Option<TridentPciInfo>,
    mmio_lin: u32,
    lfb_mapped: bool,
    lfb_lin: u32,
    lfb_mapped_phys: u32,
    lfb_mapped_size: u32,
    mode: VbeModeDesc,
    trident: Trident2dEngine,
}

impl VbeState {
    const fn new() -> Self {
        Self {
            pci: None,
            mmio_lin: 0,
            lfb_mapped: false,
            lfb_lin: 0,
            lfb_mapped_phys: 0,
            lfb_mapped_size: 0,
            mode: VbeModeDesc::empty(),
            trident: Trident2dEngine::new(),
        }
    }

    fn reset(&mut self) {
        self.pci = None;
        self.mmio_lin = 0;
        self.lfb_mapped = false;
        self.lfb_lin = 0;
        self.lfb_mapped_phys = 0;
        self.lfb_mapped_size = 0;
        self.mode = VbeModeDesc::empty();
        self.trident = Trident2dEngine::new();
    }
}

struct SyncVbeState(UnsafeCell<VbeState>);
unsafe impl Sync for SyncVbeState {}

static VBE_STATE: SyncVbeState = SyncVbeState(UnsafeCell::new(VbeState::new()));

#[inline]
fn state() -> &'static mut VbeState {
    unsafe { &mut *VBE_STATE.0.get() }
}

/// Validate a caller buffer pointer.
/// - V86 (`is_pm32 == 0`): must reside strictly in conventional memory (`0x0500..=0x000A_0000`).
/// - PM32 (`is_pm32 != 0`): must be above IVT/BDA (`>= 0x0500`) and not overflow 32-bit address space.
#[inline]
pub const fn is_valid_caller_buf(lin: u32, len: u32, is_pm32: bool) -> bool {
    if lin < 0x0500 || len == 0 {
        return false;
    }
    match lin.checked_add(len) {
        Some(end) => {
            if is_pm32 {
                true
            } else {
                end <= 0x000A_0000
            }
        }
        None => false,
    }
}

/// Called from `DllMain` (`DLL_PROCESS_ATTACH`): probe PCI for Trident `1023:8820`
/// and map its BAR1 MMIO window if present. Does NOT enable MMIO bit 0 yet so BIOS
/// `INT 10h` VBE mode setting works cleanly prior to `SetLfb`.
#[unsafe(no_mangle)]
pub extern "C" fn vbe_jlm_init() -> u32 {
    let st = state();
    st.reset();
    let pci = trident_2d::probe_trident_pci_with(|b, d, f, o| unsafe { pci_read32(b, d, f, o) });
    if let Some(info) = pci {
        st.pci = Some(info);
        if info.mmio_bar1_phys != 0 {
            let mapped = unsafe { vbe_map_mmio(info.mmio_bar1_phys, TRIDENT_MMIO_PAGES) };
            st.mmio_lin = mapped;
        }
    }
    1
}

/// Build a [`VbeCaps`] snapshot from the current JLM state.
fn build_caps(st: &VbeState) -> VbeCaps {
    let mut caps = VbeCaps::empty();
    caps.abi_version = FERRIC_VBE_ABI_VERSION;
    let mut flags = VBE_CAP_SW_2D | VBE_CAP_FILL_RECT | VBE_CAP_BIT_BLT | VBE_CAP_PM32_API;
    if st.lfb_mapped {
        flags |= VBE_CAP_LFB_MAPPED;
    }
    if let Some(info) = st.pci {
        caps.pci_vendor = info.vendor;
        caps.pci_device = info.device;
        caps.pci_rev = info.rev;
        caps.mmio_phys = info.mmio_bar1_phys;
    }
    if st.mmio_lin != 0 && st.trident.is_hw_ready() {
        flags |= VBE_CAP_HW_TRIDENT_2D;
        caps.accel_backend = VBE_ACCEL_TRIDENT_8820;
    } else {
        caps.accel_backend = VBE_ACCEL_SW;
    }
    caps.caps_flags = flags;
    caps.lfb_phys = st.mode.lfb_phys;
    caps.lfb_size = st.mode.lfb_size;
    caps.width = st.mode.width;
    caps.height = st.mode.height;
    caps.pitch = st.mode.pitch;
    caps.bpp = st.mode.bpp;
    caps
}

/// V86/PM32 API `AX=0x0001`: write [`VbeCaps`] into caller buffer at `out_lin`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vbe_jlm_get_caps(out_lin: u32, is_pm32: u32) -> u32 {
    if !is_valid_caller_buf(out_lin, VbeCaps::SIZE as u32, is_pm32 != 0) {
        return VBE_ERR_INVALID_PTR;
    }
    let caps = build_caps(state());
    unsafe {
        core::ptr::write_unaligned(out_lin as *mut VbeCaps, caps);
    }
    VBE_OK
}

/// V86/PM32 API `AX=0x0002`: configure active VBE LFB mode from [`VbeModeDesc`] at `desc_lin`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vbe_jlm_set_lfb(desc_lin: u32, is_pm32: u32) -> u32 {
    if !is_valid_caller_buf(desc_lin, VbeModeDesc::SIZE as u32, is_pm32 != 0) {
        return VBE_ERR_INVALID_PTR;
    }
    let desc = unsafe { core::ptr::read_unaligned(desc_lin as *const VbeModeDesc) };
    if let Err(code) = desc.validate() {
        return code;
    }

    let st = state();
    // Reuse existing mapping if physical base and mapped size already cover this mode.
    if !(st.lfb_mapped
        && st.lfb_mapped_phys == desc.lfb_phys
        && st.lfb_mapped_size >= desc.lfb_size)
    {
        let npages = (desc.lfb_size.saturating_add(4095)) >> 12;
        let mapped = unsafe { vbe_map_mmio(desc.lfb_phys, npages) };
        if mapped == 0 {
            return VBE_ERR_MAP_FAILED;
        }
        st.lfb_lin = mapped;
        st.lfb_mapped_phys = desc.lfb_phys;
        st.lfb_mapped_size = npages << 12;
        st.lfb_mapped = true;
    }

    st.mode = desc;
    if st.mmio_lin != 0 {
        let mut bus = LiveMmioBus::new(st.mmio_lin);
        let mut pio = LiveVgaPio;
        let _ = st.trident.configure_mode(&mut bus, &mut pio, &st.mode);
    }
    VBE_OK
}

/// V86/PM32 API `AX=0x0003`: solid-color rectangle fill from [`VbeFillReq`] at `req_lin`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vbe_jlm_fill_rect(req_lin: u32, is_pm32: u32) -> u32 {
    if !is_valid_caller_buf(req_lin, VbeFillReq::SIZE as u32, is_pm32 != 0) {
        return VBE_ERR_INVALID_PTR;
    }
    let req = unsafe { core::ptr::read_unaligned(req_lin as *const VbeFillReq) };
    let st = state();
    if !st.lfb_mapped || st.lfb_lin == 0 {
        return VBE_ERR_NOT_CONFIGURED;
    }
    if let Err(code) = req.validate_bounds(&st.mode) {
        return code;
    }
    if req.w == 0 || req.h == 0 {
        return VBE_OK;
    }

    if st.mmio_lin != 0 && st.trident.is_hw_ready() {
        let mut bus = LiveMmioBus::new(st.mmio_lin);
        if st.trident.fill_rect(&mut bus, &st.mode, &req).is_ok() {
            return VBE_OK;
        }
    }

    let lfb =
        unsafe { core::slice::from_raw_parts_mut(st.lfb_lin as *mut u8, st.mode.lfb_size as usize) };
    match sw_2d::sw_fill_rect(lfb, &st.mode, &req) {
        Ok(()) => VBE_OK,
        Err(code) => code,
    }
}

/// V86/PM32 API `AX=0x0004`: screen-to-screen BitBLT from [`VbeBlitReq`] at `req_lin`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn vbe_jlm_bit_blt(req_lin: u32, is_pm32: u32) -> u32 {
    if !is_valid_caller_buf(req_lin, VbeBlitReq::SIZE as u32, is_pm32 != 0) {
        return VBE_ERR_INVALID_PTR;
    }
    let req = unsafe { core::ptr::read_unaligned(req_lin as *const VbeBlitReq) };
    let st = state();
    if !st.lfb_mapped || st.lfb_lin == 0 {
        return VBE_ERR_NOT_CONFIGURED;
    }
    if let Err(code) = req.validate_bounds(&st.mode) {
        return code;
    }
    if req.w == 0 || req.h == 0 || (req.src_x == req.dst_x && req.src_y == req.dst_y) {
        return VBE_OK;
    }

    if st.mmio_lin != 0 && st.trident.is_hw_ready() {
        let mut bus = LiveMmioBus::new(st.mmio_lin);
        if st.trident.bit_blt(&mut bus, &st.mode, &req).is_ok() {
            return VBE_OK;
        }
    }

    let lfb =
        unsafe { core::slice::from_raw_parts_mut(st.lfb_lin as *mut u8, st.mode.lfb_size as usize) };
    match sw_2d::sw_bit_blt(lfb, &st.mode, &req) {
        Ok(()) => VBE_OK,
        Err(code) => code,
    }
}

/// V86/PM32 API `AX=0x0005`: wait for hardware 2D engine idle (no-op on SW path).
#[unsafe(no_mangle)]
pub extern "C" fn vbe_jlm_sync() -> u32 {
    let st = state();
    if st.mmio_lin != 0 && st.trident.is_hw_ready() {
        let mut bus = LiveMmioBus::new(st.mmio_lin);
        return match st.trident.sync(&mut bus) {
            Ok(()) => VBE_OK,
            Err(code) => code,
        };
    }
    VBE_OK
}

/// V86/PM32 API `AX=0x0006`: sync hardware 2D GE and disable Trident MMIO bit 0
/// so subsequent VGA PIO / BIOS `INT 10h` mode switches succeed cleanly.
#[unsafe(no_mangle)]
pub extern "C" fn vbe_jlm_release_hw() -> u32 {
    let st = state();
    if st.mmio_lin != 0 {
        let mut bus = LiveMmioBus::new(st.mmio_lin);
        return match st.trident.release_hw(&mut bus) {
            Ok(()) => VBE_OK,
            Err(code) => code,
        };
    }
    VBE_OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_pointer_validation_v86_and_pm32() {
        // Null / IVT / BDA rejected for both V86 and PM32.
        assert!(!is_valid_caller_buf(0x0000, 16, false));
        assert!(!is_valid_caller_buf(0x04FF, 16, false));
        assert!(!is_valid_caller_buf(0x04FF, 16, true));
        // Conventional memory valid for V86.
        assert!(is_valid_caller_buf(0x1000, 40, false));
        // Above 640 KiB rejected for V86, allowed for PM32.
        assert!(!is_valid_caller_buf(0x000A_0000, 40, false));
        assert!(is_valid_caller_buf(0x0010_0000, 40, true));
        // 32-bit wraparound rejected for PM32.
        assert!(!is_valid_caller_buf(0xFFFF_FFF0, 32, true));
    }

    #[test]
    fn init_and_caps_work_without_trident_hardware() {
        assert_eq!(vbe_jlm_init(), 1);
        let caps = build_caps(state());
        assert_eq!(caps.abi_version, FERRIC_VBE_ABI_VERSION);
        assert_eq!(caps.accel_backend, VBE_ACCEL_SW);
        assert_ne!(caps.caps_flags & VBE_CAP_SW_2D, 0);
        assert_ne!(caps.caps_flags & VBE_CAP_FILL_RECT, 0);
        assert_ne!(caps.caps_flags & VBE_CAP_BIT_BLT, 0);
        assert_eq!(vbe_jlm_sync(), VBE_OK);
        assert_eq!(vbe_jlm_release_hw(), VBE_OK);
    }
}
