//! Device-specific state, independent of packet-driver registrations and Wi-Fi.
//! Public fields are transitional: the legacy adapter still manages operations.
#[derive(Debug)]
pub struct EthernetState {
    pub io_base: u16,
    pub pci_device: u8,
    pub original_command: u16,
    pub irq: u8,
    pub probed: bool,
    pub mac_from_eeprom: bool,
    pub dma_lin: u32,
    pub dma_phys: u32,
    pub rx_clean: usize,
    pub rx_tail: usize,
    pub ru_starved: bool,
    pub ru_restart: i32,
    pub tx_busy: bool,
    pub tx_len: u16,
    pub last_ack: u8,
    pub eth_ready: bool,
    pub eth_mac: [u8; 6],
    pub ee_width: u8,
    pub ee_cksum: bool,
}
impl EthernetState {
    pub const fn new() -> Self {
        Self {
            io_base: 0,
            pci_device: 0,
            original_command: 0,
            irq: 0,
            probed: false,
            mac_from_eeprom: false,
            dma_lin: 0,
            dma_phys: 0,
            rx_clean: 0,
            rx_tail: 0,
            ru_starved: false,
            ru_restart: -1,
            tx_busy: false,
            tx_len: 0,
            last_ack: 0,
            eth_ready: false,
            eth_mac: [0; 6],
            ee_width: 0,
            ee_cksum: false,
        }
    }
}
impl Default for EthernetState {
    fn default() -> Self { Self::new() }
}
/// BAR1 must be assigned I/O space and cover the complete 64-byte SCB
/// register aperture within the 16-bit x86 port address space.
pub fn validated_io_bar(raw: u32, irq: u8) -> Option<u16> {
    if raw & 3 != 1 || raw == u32::MAX || irq < 3 || irq >= 16 { return None; }
    let base = raw & !3;
    if base == 0 || base > 0xFFC0 || base & 63 != 0 { return None; }
    Some(base as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_invalid_probe_resources() {
        assert_eq!(validated_io_bar(0xE001, 11), Some(0xE000));
        assert_eq!(validated_io_bar(0xFFC1, 11), Some(0xFFC0));
        for raw in [0, 1, 0xFFFF_FFFF, 0x10001, 0xE000, 0xE005, 0xFFE1] {
            assert_eq!(validated_io_bar(raw, 11), None);
        }
        for irq in [0, 2, 16, 255] {
            assert_eq!(validated_io_bar(0xE001, irq), None);
        }
    }
    #[test]
    fn reset_has_no_dma_or_active_device() {
        let s = EthernetState::new();
        assert!(!s.eth_ready);
        assert!(!s.tx_busy);
        assert_eq!(s.dma_lin, 0);
        assert_eq!(s.dma_phys, 0);
        assert_eq!(s.ru_restart, -1);
    }
}
