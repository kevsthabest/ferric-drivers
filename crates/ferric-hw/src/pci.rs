//! PCI mechanism #1 transactions for single-core ring-0 DOS.
//! Interrupt masking protects CF8/CFC against local IRQ handlers; it cannot
//! coordinate SMM, another CPU, or unrelated software outside this contract.

/// Read a PCI configuration dword. Invalid BDF or unaligned offset returns all ones.
/// # Safety
/// On x86 the caller must execute at ring 0 with permission for PCI port I/O.
pub unsafe fn read32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    let Some(addr) = super::pci_config_address(bus, dev, func, off) else {
        return u32::MAX;
    };
    #[cfg(target_arch = "x86")]
    {
        let value: u32;
        unsafe {
            core::arch::asm!(
                "pushfd", "cli",
                "mov dx, 0xcf8", "in eax, dx", "push eax",
                "mov eax, {addr:e}", "out dx, eax",
                "mov dx, 0xcfc", "in eax, dx", "mov {value:e}, eax",
                "pop eax", "mov dx, 0xcf8", "out dx, eax", "popfd",
                addr = in(reg) addr, value = lateout(reg) value,
                out("eax") _, out("edx") _,
            );
        }
        value
    }
    #[cfg(not(target_arch = "x86"))]
    { let _ = addr; u32::MAX }
}

/// Write a PCI configuration dword.
/// # Safety
/// Caller must own the target device, understand register write semantics, and
/// execute at ring 0 with PCI port access.
pub unsafe fn write32(bus: u8, dev: u8, func: u8, off: u8, value: u32) {
    let Some(addr) = super::pci_config_address(bus, dev, func, off) else { return; };
    #[cfg(target_arch = "x86")]
    unsafe {
        core::arch::asm!(
            "pushfd", "cli",
            "mov dx, 0xcf8", "in eax, dx", "push eax",
            "mov eax, {addr:e}", "out dx, eax",
            "mov dx, 0xcfc", "mov eax, {value:e}", "out dx, eax",
            "pop eax", "mov dx, 0xcf8", "out dx, eax", "popfd",
            addr = in(reg) addr, value = in(reg) value,
            out("eax") _, out("edx") _,
        );
    }
    #[cfg(not(target_arch = "x86"))]
    { let _ = (addr, value); }
}

/// Write a configuration byte directly, avoiding dword read-modify-write and
/// accidental writes to adjacent W1C status bits.
/// # Safety
/// Same ownership and privilege requirements as write32.
pub unsafe fn write8(bus: u8, dev: u8, func: u8, off: u8, value: u8) {
    let Some(addr) = super::pci_config_address(bus, dev, func, off & !3) else { return; };
    let port = 0xCFCu16 + (off & 3) as u16;
    #[cfg(target_arch = "x86")]
    unsafe {
        core::arch::asm!(
            "pushfd", "cli",
            "mov dx, 0xcf8", "in eax, dx", "push eax",
            "mov eax, {addr:e}", "out dx, eax",
            "mov dx, {port:x}", "mov eax, {value:e}", "out dx, al",
            "pop eax", "mov dx, 0xcf8", "out dx, eax", "popfd",
            addr = in(reg) addr, port = in(reg) port, value = in(reg) value as u32,
            out("eax") _, out("edx") _,
        );
    }
    #[cfg(not(target_arch = "x86"))]
    { let _ = (addr, port, value); }
}
