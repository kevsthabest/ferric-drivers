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

#[cfg(not(target_arch = "x86"))]
pub(crate) unsafe fn inb(_port: u16) -> u8 { 0 }
pub(crate) use ferric_hw::pci::{read32 as pci_read32, write8 as pci_write8};
pub(crate) use ferric_hw::pci::write32 as pci_write32;

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

