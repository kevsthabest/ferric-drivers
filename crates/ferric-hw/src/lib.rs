//! Pure, host-testable hardware resource arithmetic shared by Ferric drivers.
//! No privileged operations or global device ownership are implemented here.
#![no_std]
#![forbid(unsafe_code)]
#![deny(warnings)]

/// PCI configuration mechanism #1 address. Reject invalid BDF and unaligned offsets.
pub const fn pci_config_address(bus: u8, device: u8, function: u8, offset: u8) -> Option<u32> {
    if device >= 32 || function >= 8 || offset & 3 != 0 {
        return None;
    }
    Some(0x8000_0000 | ((bus as u32) << 16) | ((device as u32) << 11)
        | ((function as u32) << 8) | offset as u32)
}

/// Decode an assigned 32-bit memory BAR without discarding its subpage offset.
/// I/O BARs, non-32-bit memory BARs, zero addresses and absent functions are rejected.
pub const fn memory_bar32(raw: u32) -> Option<u32> {
    let base = raw & 0xFFFF_FFF0;
    if raw & 7 != 0 || raw == u32::MAX || base == 0 {
        None
    } else {
        Some(base)
    }
}

/// Page-aligned mapping required to reach a physical register window.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MmioWindow {
    pub page_base: u32,
    pub page_count: u32,
    pub offset: u32,
}

impl MmioWindow {
    pub const fn new(phys: u32, span: u32) -> Option<Self> {
        if phys == 0 || span == 0 || (phys as u64) + (span as u64) > 0x1_0000_0000 {
            return None;
        }
        let offset = phys & 0xFFF;
        let pages = ((offset as u64 + span as u64 + 0xFFF) >> 12) as u32;
        Some(Self { page_base: phys & !0xFFF, page_count: pages, offset })
    }

    /// Resolve the register base, rejecting failed or overflowing mappings.
    pub const fn linear_base(self, mapped_page: u32) -> Option<u32> {
        if mapped_page == 0 || mapped_page & 0xFFF != 0 ||
            (mapped_page as u64) + (self.page_count as u64) * 4096 > 0x1_0000_0000 {
            return None;
        }
        mapped_page.checked_add(self.offset)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dynabook_sd_bar_preserves_subpage_offset() {
        let phys = memory_bar32(0xF7EB_DC00).unwrap();
        let window = MmioWindow::new(phys, 0x140).unwrap();
        assert_eq!(window, MmioWindow { page_base: 0xF7EB_D000, page_count: 1, offset: 0xC00 });
        assert_eq!(window.linear_base(0x1234_0000), Some(0x1234_0C00));
    }

    #[test]
    fn cross_page_and_overflow() {
        assert_eq!(MmioWindow::new(0x1234_0FF0, 0x20).unwrap().page_count, 2);
        assert!(MmioWindow::new(0xFFFF_FFF0, 0x20).is_none());
        assert!(MmioWindow::new(0x1000, 0).is_none());
        assert!(MmioWindow::new(0, 1).is_none());
        let window = MmioWindow::new(0x1FF0, 0x20).unwrap();
        assert!(window.linear_base(0xFFFF_F000).is_none());
        assert!(window.linear_base(0).is_none());
        assert!(window.linear_base(0x1234_0001).is_none());
    }

    #[test]
    fn rejects_wrong_bar_types() {
        for raw in [0, 1, 4, 0xFFFF_FFFF, 0x1234_0001, 0x1234_0004] {
            assert_eq!(memory_bar32(raw), None);
        }
        assert_eq!(memory_bar32(0x1234_0008), Some(0x1234_0000));
    }

    #[test]
    fn pci_address_bounds() {
        assert_eq!(pci_config_address(0, 0x12, 0, 0x10), Some(0x8000_9010));
        assert_eq!(pci_config_address(2, 0, 1, 0xFC), Some(0x8002_01FC));
        assert!(pci_config_address(0, 32, 0, 0).is_none());
        assert!(pci_config_address(0, 0, 8, 0).is_none());
        assert!(pci_config_address(0, 0, 0, 3).is_none());
    }
}
