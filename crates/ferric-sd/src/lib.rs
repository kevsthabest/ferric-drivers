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
}

#[cfg(test)]
unsafe fn sd_map_mmio(_phys: u32, _npages: u32) -> u32 { 0 }

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
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | (((dev & 0x1F) as u32) << 11)
        | (((func & 0x07) as u32) << 8)
        | ((reg & 0xFC) as u32);
    unsafe {
        outl(0x0CF8, addr);
        inl(0x0CFC)
    }
}

pub(crate) unsafe fn pci_write32(bus: u8, dev: u8, func: u8, reg: u8, val: u32) {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | (((dev & 0x1F) as u32) << 11)
        | (((func & 0x07) as u32) << 8)
        | ((reg & 0xFC) as u32);
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

pub(crate) mod ehci {
    use crate::{pci_read32, pci_write32};

    const BAR_SEARCH_START: u32 = 0xF7EC_0000;
    const BAR_SEARCH_END: u32 = 0xF7FF_0000;

    unsafe fn bar_taken(cand: u32, size: u32, self_dev: u8, self_func: u8) -> bool {
        let cand_end = cand.saturating_add(size);
        for dev in 0..32u8 {
            let id0 = unsafe { pci_read32(0, dev, 0, 0) };
            if (id0 & 0xFFFF) == 0xFFFF {
                continue;
            }
            let hdr = (unsafe { pci_read32(0, dev, 0, 0x0C) } >> 16) & 0xFF;
            let max_fn = if (hdr & 0x80) != 0 { 8u8 } else { 1u8 };
            for func in 0..max_fn {
                if dev == self_dev && func == self_func {
                    continue;
                }
                let id = unsafe { pci_read32(0, dev, func, 0) };
                if (id & 0xFFFF) == 0xFFFF {
                    continue;
                }
                let htype = ((unsafe { pci_read32(0, dev, func, 0x0C) } >> 16) & 0x7F) as u8;
                let max_bar = if htype == 0 { 6 } else { 2 };
                for b in 0..max_bar {
                    let raw = unsafe { pci_read32(0, dev, func, 0x10 + (b as u8) * 4) };
                    if (raw & 1) != 0 {
                        continue;
                    }
                    let base = raw & 0xFFFF_F000;
                    if base == 0 {
                        continue;
                    }
                    let end = base.saturating_add(0x1_0000);
                    if cand < end && base < cand_end {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub(crate) unsafe fn assign_mmio_bar(bus: u8, dev: u8, func: u8, size: u32) -> Option<u32> {
        let align = if size > 0x1000 { size } else { 0x1000 };
        let mut cand = (BAR_SEARCH_START + align - 1) & !(align - 1);
        while cand + size <= BAR_SEARCH_END {
            if !unsafe { bar_taken(cand, size, dev, func) } {
                unsafe { pci_write32(bus, dev, func, 0x10, cand) };
                let rb = unsafe { pci_read32(bus, dev, func, 0x10) } & 0xFFFF_F000;
                if rb == cand {
                    return Some(cand);
                }
                return None;
            }
            cand += align;
        }
        None
    }
}
