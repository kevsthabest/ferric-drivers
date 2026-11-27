//! Device-specific state, independent of packet-driver registrations and Wi-Fi.
//! Public fields are transitional: the legacy adapter still manages operations.
#[derive(Debug)]
pub struct EthernetState {
    pub io_base: u16,
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
#[cfg(test)]
mod tests {
    use super::*;
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
