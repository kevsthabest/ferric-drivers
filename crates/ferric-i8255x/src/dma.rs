//! Shared descriptor geometry and checked bus addresses.
pub const RFD_COUNT: usize = 8;
pub const RFD_STRIDE: usize = 1552; // 16-byte header + 1536-byte frame buffer
pub const RFD_HDR: usize = 16;
pub const FRAME_BUF: usize = 1536;
pub const TXCB_OFF: usize = RFD_COUNT * RFD_STRIDE; // 12416
pub const TXBUF_OFF: usize = TXCB_OFF + 32; // 12448
pub const STATS_OFF: usize = 14000;
pub const NET_DMA_SIZE: usize = 16384;
pub const NET_DMA_PARAS: u32 = (NET_DMA_SIZE / 16) as u32;

// RFD field offsets (simplified mode).
pub const RFD_STATUS: usize = 0;
pub const RFD_CMD: usize = 2;
pub const RFD_LINK: usize = 4;
pub const RFD_RBD: usize = 8;
pub const RFD_ACTUAL: usize = 12;
pub const RFD_SIZE: usize = 14;

// Command-block field offsets (TXCB_OFF); the parameter word for
// Configure / IA-setup lives at +8.
pub const CB_STATUS: usize = 0;
pub const CB_CMD: usize = 2;
pub const CB_LINK: usize = 4;
pub const CB_PARAM: usize = 8;
pub const CB_TBD_ADDR: usize = 8;
pub const CB_THRESH: usize = 14;
pub const CB_TBD_COUNT: usize = 15;
pub const TBD_ADDR: usize = 16;
pub const TBD_LEN: usize = 20;


/// A contiguous, aligned 16 KiB DMA region. Physical and linear addresses
/// may differ. The caller must separately lock/verify physical contiguity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DmaRegion { physical: u32, linear: u32 }

impl DmaRegion {
    pub fn new(physical: u32, linear: u32) -> Option<Self> {
        if physical == 0 || linear == 0 || (physical | linear) & 15 != 0 {
            return None;
        }
        physical.checked_add(NET_DMA_SIZE as u32 - 1)?;
        linear.checked_add(NET_DMA_SIZE as u32 - 1)?;
        Some(Self { physical, linear })
    }
    pub fn physical_at(self, off: usize) -> Option<u32> {
        if off >= NET_DMA_SIZE { return None; }
        self.physical.checked_add(off as u32)
    }
    pub fn linear_at(self, off: usize) -> Option<u32> {
        if off >= NET_DMA_SIZE { return None; }
        self.linear.checked_add(off as u32)
    }
    pub fn next_rfd(self, index: usize) -> Option<u32> {
        if index >= RFD_COUNT { return None; }
        self.physical_at(((index + 1) % RFD_COUNT) * RFD_STRIDE)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn geometry_and_ring_wrap() {
        let r = DmaRegion::new(0x10000, 0x20000).unwrap();
        assert_eq!(r.next_rfd(0), Some(0x10000 + 1552));
        assert_eq!(r.next_rfd(7), Some(0x10000));
        assert_eq!(r.next_rfd(8), None);
        assert_eq!(r.linear_at(TXBUF_OFF), Some(0x20000 + 12448));
        assert_eq!(r.physical_at(STATS_OFF), Some(0x10000 + 14000));
        assert!(TXBUF_OFF + FRAME_BUF <= STATS_OFF);
    }
    #[test]
    fn invalid_regions() {
        assert!(DmaRegion::new(0, 0x20000).is_none());
        assert!(DmaRegion::new(0x10001, 0x20000).is_none());
        assert!(DmaRegion::new(0xFFFF_FFF0, 0x20000).is_none());
        assert!(DmaRegion::new(0x10000, 0xFFFF_FFF0).is_none());
        assert!(DmaRegion::new(0x10000, 0x20000).unwrap().physical_at(NET_DMA_SIZE).is_none());
    }
}

/// Device-visible descriptor values. Writes remain the adapter's responsibility.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceiveDescriptor {
    pub command: u16,
    pub link: u32,
    pub buffer_descriptor: u32,
    pub size: u16,
}

pub fn receive_descriptor(region: DmaRegion, index: usize) -> Option<ReceiveDescriptor> {
    Some(ReceiveDescriptor {
        command: if index == RFD_COUNT - 1 { 0x8000 } else { 0 },
        link: region.next_rfd(index)?,
        buffer_descriptor: u32::MAX,
        size: FRAME_BUF as u16,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransmitDescriptor {
    pub command: u16,
    pub link: u32,
    pub tbd: u32,
    pub buffer: u32,
    pub wire_len: u16,
    pub threshold: u8,
}

pub fn transmit_descriptor(region: DmaRegion, len: usize) -> Option<TransmitDescriptor> {
    if !(1..=1514).contains(&len) { return None; }
    Some(TransmitDescriptor {
        command: 0xA00C,
        link: region.physical_at(TXCB_OFF)?,
        tbd: region.physical_at(TXCB_OFF + 16)?,
        buffer: region.physical_at(TXBUF_OFF)?,
        wire_len: len.max(60) as u16,
        threshold: 0xE0,
    })
}

#[cfg(test)]
mod descriptor_tests {
    use super::*;
    #[test]
    fn last_receive_descriptor_wraps_and_ends_list() {
        let r = DmaRegion::new(0x10000, 0x20000).unwrap();
        assert_eq!(receive_descriptor(r, 0).unwrap().command, 0);
        let last = receive_descriptor(r, 7).unwrap();
        assert_eq!(last.command, 0x8000);
        assert_eq!(last.link, 0x10000);
        assert_eq!(last.buffer_descriptor, u32::MAX);
        assert_eq!(last.size, 1536);
        assert_eq!(receive_descriptor(r, 8), None);
    }
    #[test]
    fn tx_flexible_mode_and_padding() {
        let r = DmaRegion::new(0x10000, 0x20000).unwrap();
        let d = transmit_descriptor(r, 42).unwrap();
        assert_eq!(d.command, 0xA00C);
        assert_eq!(d.link, 0x10000 + 12416);
        assert_eq!(d.tbd, d.link + 16);
        assert_eq!(d.buffer, 0x10000 + 12448);
        assert_eq!(d.wire_len, 60);
        assert_eq!(d.threshold, 0xE0);
        assert_eq!(transmit_descriptor(r, 1514).unwrap().wire_len, 1514);
        assert_eq!(transmit_descriptor(r, 0), None);
        assert_eq!(transmit_descriptor(r, 1515), None);
    }
}
