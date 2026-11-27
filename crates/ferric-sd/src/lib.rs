//! Standalone Toshiba TC6371AF Built-in SD Slot JLM (`TOSHSD.JLM`, DDB `0xFD06`).
//!
//! Freestanding 32-bit Rust JLM (`i386-unknown-none`, no MMX/SSE, strictly read-only):
//! - Polled PIO mode for PCI `1179:0805` / `1179:0803` SD host controllers
//! - Verbs `0x003C` (init/identify), `0x003D` (read 512B sector), `0x003E` (shutdown)

#![cfg_attr(not(test), no_std)]
#![deny(warnings)]

pub mod toshsd;

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[cfg(not(test))]
unsafe extern "C" {
    fn sd_map_mmio(phys: u32, npages: u32) -> u32;
    fn sd_unmap_mmio(mapped_page: u32) -> u32;
}

#[cfg(test)]
unsafe fn sd_map_mmio(_phys: u32, _npages: u32) -> u32 { 0 }

#[cfg(test)]
unsafe fn sd_unmap_mmio(_mapped_page: u32) -> u32 { 1 }

#[cfg(target_arch = "x86")]
pub(crate) unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") val, options(nomem, nostack));
    }
    val
}

#[cfg(target_arch = "x86")]
pub(crate) unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    unsafe {
        core::arch::asm!("in eax, dx", in("dx") port, out("eax") val, options(nomem, nostack));
    }
    val
}

#[cfg(target_arch = "x86")]
pub(crate) unsafe fn outl(port: u16, val: u32) {
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack));
    }
}

#[cfg(not(target_arch = "x86"))]
pub(crate) unsafe fn inb(_port: u16) -> u8 { 0 }
#[cfg(not(target_arch = "x86"))]
pub(crate) unsafe fn inl(_port: u16) -> u32 { 0xFFFF_FFFF }
#[cfg(not(target_arch = "x86"))]
pub(crate) unsafe fn outl(_port: u16, _val: u32) {}

pub(crate) unsafe fn pci_read32(bus: u8, dev: u8, func: u8, reg: u8) -> u32 {
    let Some(addr) = ferric_hw::pci_config_address(bus, dev, func, reg & 0xFC) else {
        return u32::MAX;
    };
    unsafe {
        outl(0x0CF8, addr);
        inl(0x0CFC)
    }
}

pub(crate) unsafe fn pci_write32(bus: u8, dev: u8, func: u8, reg: u8, val: u32) {
    let Some(addr) = ferric_hw::pci_config_address(bus, dev, func, reg & 0xFC) else {
        return;
    };
    unsafe {
        outl(0x0CF8, addr);
        outl(0x0CFC, val);
    }
}

pub(crate) mod cardbus {
    pub fn delay_ms(ms: u32) {
        let iters = ms.saturating_mul(1000);
        let mut i = 0u32;
        while i < iters {
            let _ = unsafe { crate::inb(0x80) };
            i += 1;
        }
    }

    pub unsafe fn map_mmio(phys: u32, npages: u32) -> u32 {
        unsafe { crate::sd_map_mmio(phys, npages) }
    }
}

