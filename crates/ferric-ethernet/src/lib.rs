//! Standalone Intel 8255x Jemm packet driver.
#![cfg_attr(not(test), no_std)]
#![deny(warnings)]
use core::cell::UnsafeCell;
use ferric_hw::pci::{read32 as pci_read32, write32 as pci_write32};

#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

const CR_EBX: usize = 4;
const CR_EDX: usize = 5;
const CR_ECX: usize = 6;
const CR_EAX: usize = 7;
const CR_ES: usize = 15;
const CR_EDI: usize = 0;
const CR_ESI: usize = 1;
const CR_CS: usize = 11;
const CR_EFLAGS: usize = 12;
const CR_DS: usize = 16;
#[cfg(target_arch = "x86")]
static PKT_STUB: &[u8] = include_bytes!("../../../out/pktstub.bin");
#[cfg(not(target_arch = "x86"))]
static PKT_STUB: &[u8] = &[];

#[cfg(target_arch = "x86")]
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack));
    }
    v
}

#[cfg(target_arch = "x86")]
unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
    }
}

#[cfg(target_arch = "x86")]
unsafe fn inl(port: u16) -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("in eax, dx", in("dx") port, out("eax") v, options(nomem, nostack));
    }
    v
}

#[cfg(target_arch = "x86")]
unsafe fn outl(port: u16, val: u32) {
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack));
    }
}


#[cfg(not(target_arch = "x86"))]
unsafe fn inb(_: u16) -> u8 { 0 }
#[cfg(not(target_arch = "x86"))]
unsafe fn outb(_: u16, _: u8) {}
#[cfg(not(target_arch = "x86"))]
unsafe fn inl(_: u16) -> u32 { 0 }
#[cfg(not(target_arch = "x86"))]
unsafe fn outl(_: u16, _: u32) {}

#[cfg(target_arch = "x86")]
unsafe fn vdmad_lock_dma_region(lin: u32, len: u32) -> Option<u32> {
    let phys: u32;
    let failed: u32;
    unsafe {
        // Rust forbids explicit `in("esi")` on x86, while `out(reg)` CAN still
        // be allocated to ESI (which caused `pop esi` to clobber `phys`!).
        // Pin every operand to explicit non-ESI registers (EAX/ECX/EDX) so
        // `push esi` / `pop esi` can never alias any input or output.
        core::arch::asm!(
            "push esi",
            "mov esi, eax",
            "int 0x20",
            ".word 1", // VDMAD_Lock_DMA_Region
            ".word 4", // device: VDMAD
            "sbb eax, eax",
            "pop esi",
            inlateout("eax") lin => failed,
            inlateout("ecx") len => _,
            inlateout("edx") 1u32 => phys,
        );
    }
    if failed == 0 { Some(phys) } else { None }
}

#[cfg(not(target_arch = "x86"))]
unsafe fn vdmad_lock_dma_region(_: u32, _: u32) -> Option<u32> { None }

#[cfg(target_arch = "x86")]
pub(crate) unsafe fn vdmad_unlock_dma_region(lin: u32, len: u32) {
    unsafe {
        core::arch::asm!(
            "push esi",
            "mov esi, eax",
            "int 0x20",
            ".word 2", // VDMAD_Unlock_DMA_Region
            ".word 4", // device: VDMAD
            "pop esi",
            inlateout("eax") lin => _,
            inlateout("ecx") len => _,
            inlateout("edx") 1u32 => _,
        );
    }
}

#[cfg(not(target_arch = "x86"))]
unsafe fn vdmad_unlock_dma_region(_: u32, _: u32) {}

#[cfg(target_arch = "x86")]
fn bios_tick() -> u32 {
    // The BIOS tick at 0040:006C is identity-mapped low memory; the
    // JLM stub's IRQ hook reads the same cell. It is the only clock
    // available in this context (no DOS, no rdtsc guarantee on a 386).
    unsafe { core::ptr::read_volatile(0x46C as *const u32) }
}

#[cfg(not(target_arch = "x86"))]
fn bios_tick() -> u32 { 0 }

#[cfg(not(test))]
unsafe extern "C" {
    fn eth_claim(kind: u32, a: u32, b: u32) -> u32;
    fn eth_release(kind: u32, a: u32, b: u32) -> u32;
}
#[cfg(test)]
unsafe fn eth_claim(_: u32, _: u32, _: u32) -> u32 { 0 }
#[cfg(test)]
unsafe fn eth_release(_: u32, _: u32, _: u32) -> u32 { 0 }

fn net_claim_resources(device: u8, io: u16) -> bool {
    let bdf = u32::from(device) << 3;
    if unsafe { eth_claim(1, bdf, 0) } != 0 { return false; }
    if unsafe { eth_claim(2, u32::from(io), 64) } != 0 {
        // Release failure is not expected under the registered broker contract.
        let _ = unsafe { eth_release(1, bdf, 0) };
        return false;
    }
    true
}

include!("../../ferric-i8255x/src/packet_jemm.inc.rs");

/// IRQ number for the Jemm adapter, after successful probe.
#[unsafe(no_mangle)]
pub extern "C" fn eth_irq() -> u32 { u32::from(net_state().ethernet.irq) }

/// Poll and arm receive work as a fallback for missed hardware interrupts.
#[unsafe(no_mangle)]
pub extern "C" fn eth_poll() {
    let st = net_state();
    if st.present && st.svc_rfd < 0 {
        st.svc_budget = RFD_COUNT as u8;
    }
}

/// Stop hardware before removing hooks. Refuse active packet receivers.
/// Retain DOS blocks until reboot because callers may cache the packet stub.
/// Disable bus mastering and verify its readback before releasing DMA.
#[unsafe(no_mangle)]
pub extern "C" fn eth_shutdown() -> u32 {
    let st = net_state();
    if st.regs.iter().any(|r| r.active) || st.svc_rfd >= 0 { return 1; }
    st.present = false;
    st.ethernet.eth_ready = false;
    if !st.ethernet.probed { return 0; }
    let e = &mut st.ethernet;
    unsafe {
        outb(e.io_base + SCB_CMDHI, 1);
        outl(e.io_base + SCB_PORT, 0);
        for _ in 0..4096 { let _ = inb(e.io_base + SCB_CMD); }
        outb(e.io_base + SCB_CMDHI, 1);
        let cmd = pci_read32(0, e.pci_device, 0, 4) as u16;
        pci_write32(0, e.pci_device, 0, 4, u32::from(cmd & !4));
        let readback = pci_read32(0, e.pci_device, 0, 4);
        if readback == u32::MAX || readback & 4 != 0 ||
           inb(e.io_base + SCB_CMD) != 0 || inb(e.io_base + SCB_STATUS) & 0xFC != 0 {
            return 2;
        }
        if e.dma_lin != 0 {
            vdmad_unlock_dma_region(e.dma_lin, NET_DMA_SIZE as u32);
            e.dma_lin = 0;
            e.dma_phys = 0;
        }
        // Do not restore original bus mastering without restoring firmware DMA state.
        pci_write32(0, e.pci_device, 0, 4, u32::from(e.original_command & !4));
    }
    // Resource release is separate, after the assembly adapter removes hooks.
    0
}

#[unsafe(no_mangle)]
pub extern "C" fn eth_release_resources() -> u32 {
    let e = &net_state().ethernet;
    if e.probed {
        if unsafe { eth_release(2, u32::from(e.io_base), 64) } != 0 { return 1; }
        if unsafe { eth_release(1, u32::from(e.pci_device) << 3, 0) } != 0 { return 1; }
    }
    net_reset();
    0
}
