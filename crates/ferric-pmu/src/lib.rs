//! Standalone ALi M7101 Power Management Unit JLM (`PMU.JLM`, DDB `0xFD05`).
//!
//! Freestanding 32-bit Rust JLM (`i386-unknown-none`, no MMX/SSE, read-only):
//! - Smart Battery System (SBS v1.1) telemetry over M7101 SMBus (addr 0x0B)
//! - CPU clock throttling via ACPI `P_CNTRL` (`pm_base + 0x10`)
//! - LCD CCFT backlight PWM via M7101 PCI config `0xB0`

#![cfg_attr(not(test), no_std)]
#![deny(warnings)]

pub mod m7101;

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

#[cfg(target_arch = "x86")]
pub(crate) unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") val, options(nomem, nostack));
    }
    val
}

#[cfg(target_arch = "x86")]
pub(crate) unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
    }
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
pub(crate) unsafe fn outb(_port: u16, _val: u8) {}
#[cfg(not(target_arch = "x86"))]
pub(crate) unsafe fn inl(_port: u16) -> u32 { 0xFFFF_FFFF }
#[cfg(not(target_arch = "x86"))]
pub(crate) unsafe fn outl(_port: u16, _val: u32) {}

pub(crate) use ferric_hw::pci::{read32 as pci_read32, write8 as pci_write8};

pub(crate) mod cardbus {
    pub fn delay_ms(ms: u32) {
        let iters = ms.saturating_mul(1000);
        let mut i = 0u32;
        while i < iters {
            let _ = unsafe { crate::inb(0x80) };
            i += 1;
        }
    }
}
