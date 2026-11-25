//! Decoding of the EHCI capability parameter registers.
//!
//! HCSPARAMS is what the routing policy is built from: it says how
//! many ports exist, how many companion controllers share them, and
//! how the ports divide among the companions. HCCPARAMS says where
//! the legacy-support capability lives (the BIOS handoff) and whether
//! the controller can address 64-bit DMA (Ferric's resident arena is
//! below 4 GiB either way, but the seam must know which mode to
//! program).

/// Decoded HCSPARAMS (capability register at offset 0x04).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HcsParams {
    raw: u32,
}

impl HcsParams {
    pub fn from_raw(raw: u32) -> Self {
        Self { raw }
    }

    pub fn raw(&self) -> u32 {
        self.raw
    }

    /// Number of downstream ports (bits 3:0).
    pub fn n_ports(&self) -> u8 {
        (self.raw & 0xF) as u8
    }

    /// Port Power Control (bit 4, EHCI r1.1 Table 2-10): ports have
    /// individual power switches behind PORTSC.POWER. The uPD720100A
    /// implements it.
    pub fn port_power_control(&self) -> bool {
        self.raw & (1 << 4) != 0
    }

    /// Port Routing Rules (bit 7, EHCI r1.1 Table 2-10): 0 = the
    /// first N_PCC ports belong to the first companion, the next
    /// N_PCC to the second, etc.; 1 = the HCSP_PORTROUTE array
    /// assigns each port to a companion explicitly.
    pub fn routing_rules(&self) -> u8 {
        ((self.raw >> 7) & 0x1) as u8
    }

    /// Ports Per Companion Controller (bits 11:8, EHCI r1.1 Table 2-10).
    pub fn n_ports_per_companion(&self) -> u8 {
        ((self.raw >> 8) & 0xF) as u8
    }

    /// Number of Companion Controllers (bits 15:12, EHCI r1.1 Table 2-10).
    /// Zero on a controller with no USB 1.1 companions (then every
    /// device must be high-speed or reached through a hub's
    /// transaction translator, which this driver does not implement).
    pub fn n_companions(&self) -> u8 {
        ((self.raw >> 12) & 0xF) as u8
    }
}

/// Decoded HCCPARAMS (capability register at offset 0x08).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HccParams {
    raw: u32,
}

impl HccParams {
    pub fn from_raw(raw: u32) -> Self {
        Self { raw }
    }

    pub fn raw(&self) -> u32 {
        self.raw
    }

    /// 64-bit Addressing Capability (bit 0). When clear, all DMA
    /// structures and buffers must live below 4 GiB — which Ferric's
    /// resident DMA arena already guarantees, so both values are
    /// usable; the seam still has to know, because with 64-bit
    /// addressing the data-structure segment register is in play.
    pub fn addr64(&self) -> bool {
        self.raw & 1 != 0
    }

    /// Programmable Frame List Flag (bit 1): the periodic frame list
    /// size can be programmed smaller than 1024 entries.
    pub fn programmable_frame_list(&self) -> bool {
        self.raw & (1 << 1) != 0
    }

    /// Asynchronous Schedule Park Capability (bit 2).
    pub fn async_park(&self) -> bool {
        self.raw & (1 << 2) != 0
    }

    /// EHCI Extended Capabilities Pointer (bits 23:16): a byte offset
    /// into PCI configuration space where the extended capability
    /// list starts (the legacy-support capability is first), or 0
    /// when the controller has no extended capabilities.
    pub fn eecp(&self) -> u8 {
        ((self.raw >> 16) & 0xFF) as u8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real DynaBook SS 2000M NEC uPD720100A (1033:00E0 at 00:0C.2)
    // silicon capture (docs/hardware/dynabook-ss2000m-ehci.txt):
    // 5 ports, PPC=1, routing_rules=1, 3 ports/companion, 2 companions.
    const DYNABOOK_HCSPARAMS: u32 = 0x0000_2395;

    #[test]
    fn dynabook_hcsparams_decodes() {
        let p = HcsParams::from_raw(DYNABOOK_HCSPARAMS);
        assert_eq!(p.n_ports(), 5);
        assert!(p.port_power_control());
        assert_eq!(p.routing_rules(), 1);
        assert_eq!(p.n_ports_per_companion(), 3);
        assert_eq!(p.n_companions(), 2);
    }

    #[test]
    fn ich9_style_hcsparams_decodes() {
        // QEMU's ich9-usb-ehci1 shape: 6 ports, 2 companions with
        // 3 ports each.
        let p = HcsParams::from_raw(0x0000_2306);
        assert_eq!(p.n_ports(), 6);
        assert_eq!(p.n_ports_per_companion(), 3);
        assert_eq!(p.n_companions(), 2);
    }

    #[test]
    fn hccparams_decodes() {
        // 64-bit capable, park capable, EECP at PCI offset 0xC0
        // (where the uPD720100A family puts its legacy support).
        let p = HccParams::from_raw(0x00C0_0005);
        assert!(p.addr64());
        assert!(!p.programmable_frame_list());
        assert!(p.async_park());
        assert_eq!(p.eecp(), 0xC0);
    }

    #[test]
    fn hccparams_no_eecp() {
        let p = HccParams::from_raw(0);
        assert!(!p.addr64());
        assert_eq!(p.eecp(), 0);
    }
}
