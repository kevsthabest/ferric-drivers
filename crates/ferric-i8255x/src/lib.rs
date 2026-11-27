//! Intel 8255x device-specific primitives, shared by the legacy core and future JLM.
#![no_std]
#![forbid(unsafe_code)]
#![deny(warnings)]

pub mod dma;

/// The 8255x Configure command (22 bytes, verified against Linux
/// `e100.c` `e100_configure()` and the Intel 8255x OpenSDM):
///   [0]  0x16 = 22 bytes
///   [1]  0x08 = RX FIFO limit 8, TX FIFO limit 0
///   [6]  0x32 = direct_rx_dma | standard_tcb | standard_stat_counter
///   [7]  0x07 = rx_discard_short_frames | tx_underrun_retry=3
///   [8]  0x01 = MII mode (1 = MII PHY, 0 = legacy 503 serial)
///   [10] 0x2E = pad10=6 | no_source_addr_insertion | 7-byte preamble
///   [12] 0x60 = 96-bit interframe spacing (ifs=6)
///   [14] 0xF2 = ip_addr_hi
///   [15] 0x48 = mandatory-1 bits (bit 0 = promiscuous)
///   [17] 0x40 = fc_delay_hi
///   [18] 0xF2 = tx_padding | fc_priority_threshold=7 | pad18=1
///   [19] 0x82 = magic_packet_disable | full_duplex_pin
///   [20] 0x3F = pad20_1=0x1F | fc_priority_location=1
///   [21] 0x05 = pad21_1=5
pub fn configure_bytes(promisc: bool) -> [u8; 22] {
    let mut cfg: [u8; 22] = [
        0x16, 0x08, 0x00, 0x00, 0x00, 0x00, 0x32, 0x07, 0x01, 0x00, 0x2E, 0x00, 0x60, 0x00, 0xF2,
        0x48, 0x00, 0x40, 0xF2, 0x82, 0x3F, 0x05,
    ];
    if promisc {
        cfg[6] |= 0x80; // rx_save_bad_frames
        cfg[7] &= !0x01; // pass short frames in promiscuous mode
        cfg[15] |= 0x01; // promiscuous_mode (byte 15 bit 0)
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn configuration_matches_qualified_baseline() {
        assert_eq!(configure_bytes(false), [
            0x16, 0x08, 0, 0, 0, 0, 0x32, 0x07, 0x01, 0, 0x2E, 0, 0x60, 0, 0xF2,
            0x48, 0, 0x40, 0xF2, 0x82, 0x3F, 0x05,
        ]);
    }
    #[test]
    fn promiscuous_mode_changes_only_three_bytes() {
        let a = configure_bytes(false);
        let b = configure_bytes(true);
        for i in 0..22 {
            assert_eq!(b[i], match i { 6 => a[i] | 0x80, 7 => a[i] & !1, 15 => a[i] | 1, _ => a[i] });
        }
    }
}
