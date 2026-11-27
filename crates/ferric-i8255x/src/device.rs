//! Reusable 8255x bring-up, independent of DOS, PCI resource allocation,
//! interrupt hooks and packet-client registration. Caller owns and locks
//! the DMA region, enables I/O and bus mastering, and serializes access.
//! On failure, retain the DMA region until the platform proves quiescence.
use crate::{configure_bytes, dma::*, scb::{self, ScbIo}, state::EthernetState};

pub const POLL_LIMIT: u32 = 1_000_000;

pub trait DeviceIo: ScbIo {
    fn reset(&mut self);
    fn mask_device_irq(&mut self);
    fn read_dma16(&mut self, offset: usize) -> u16;
    fn write_dma8(&mut self, offset: usize, value: u8);
    fn write_dma16(&mut self, offset: usize, value: u16);
    fn write_dma32(&mut self, offset: usize, value: u32);
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitError { InvalidDma, Reset, Configure, Address, Receive }

fn submit(io: &mut impl DeviceIo, gp: u32, cmd: u8) -> bool {
    scb::submit(io, gp, cmd, 2, POLL_LIMIT).is_ok()
}

pub fn clear_command(io: &mut impl DeviceIo) {
    for i in 0..32 { io.write_dma8(TXCB_OFF + i, 0); }
}

fn execute(io: &mut impl DeviceIo, region: DmaRegion) -> bool {
    let Some(gp) = region.physical_at(TXCB_OFF) else { return false; };
    if !submit(io, gp, 0x10) { return false; }
    let start = io.tick();
    for _ in 0..POLL_LIMIT {
        let status = io.read_dma16(TXCB_OFF + CB_STATUS);
        if status & 0x8000 != 0 { return status & 0x2000 != 0; }
        if io.tick().wrapping_sub(start) > 36 { return false; }
    }
    false
}

pub fn configure(io: &mut impl DeviceIo, region: DmaRegion, promisc: bool) -> bool {
    clear_command(io);
    io.write_dma16(TXCB_OFF + CB_CMD, 0x8002);
    for (i, v) in configure_bytes(promisc).iter().enumerate() {
        io.write_dma8(TXCB_OFF + CB_PARAM + i, *v);
    }
    execute(io, region)
}

pub fn initialize_ring(io: &mut impl DeviceIo, state: &mut EthernetState, region: DmaRegion) -> bool {
    for i in 0..RFD_COUNT {
        let Some(d) = receive_descriptor(region, i) else { return false; };
        let off = i * RFD_STRIDE;
        io.write_dma16(off + RFD_STATUS, 0);
        io.write_dma16(off + RFD_CMD, d.command);
        io.write_dma32(off + RFD_LINK, d.link);
        io.write_dma32(off + RFD_RBD, d.buffer_descriptor);
        io.write_dma16(off + RFD_ACTUAL, 0);
        io.write_dma16(off + RFD_SIZE, d.size);
    }
    state.rx_clean = 0;
    state.rx_tail = RFD_COUNT - 1;
    state.ru_restart = -1;
    state.ru_starved = false;
    true
}

/// Start the NIC with its interrupt source masked. Stage values match the
/// legacy NTEST diagnostics. The platform installs/unmasks its IRQ afterward.
pub fn initialize(io: &mut impl DeviceIo, state: &mut EthernetState, stage: &mut u8) -> Result<(), InitError> {
    let region = DmaRegion::new(state.dma_phys, state.dma_lin).ok_or(InitError::InvalidDma)?;
    state.eth_ready = false;
    *stage = 2;
    io.mask_device_irq();
    for i in 0..NET_DMA_SIZE { io.write_dma8(i, 0); }
    io.reset();
    for _ in 0..4096 { let _ = io.command(); }
    io.mask_device_irq();
    if !scb::wait_idle(io, 2, POLL_LIMIT) || !submit(io, 0, 0x60) || !submit(io, 0, 0x06) {
        return Err(InitError::Reset);
    }
    if !configure(io, region, false) { return Err(InitError::Configure); }
    *stage = 3;
    clear_command(io);
    io.write_dma16(TXCB_OFF + CB_CMD, 0x8001);
    for (i, v) in state.eth_mac.iter().enumerate() {
        io.write_dma8(TXCB_OFF + CB_PARAM + i, *v);
    }
    if !execute(io, region) { return Err(InitError::Address); }
    *stage = 4;
    if !submit(io, region.physical_at(STATS_OFF).ok_or(InitError::InvalidDma)?, 0x40)
        || !initialize_ring(io, state, region) || !submit(io, state.dma_phys, 0x01) {
        return Err(InitError::Receive);
    }
    *stage = 5;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Mock {
        mem: [u8; NET_DMA_SIZE],
        commands: [u8; 8],
        count: usize,
        fail_configure: bool,
        masked: bool,
    }
    impl ScbIo for Mock {
        fn command(&mut self) -> u8 { 0 }
        fn set_pointer(&mut self, _: u32) {}
        fn set_command(&mut self, cmd: u8) {
            self.commands[self.count] = cmd;
            self.count += 1;
            if cmd == 0x10 {
                self.write_dma16(TXCB_OFF + CB_STATUS, if self.fail_configure { 0x8000 } else { 0xA000 });
            }
        }
        fn tick(&mut self) -> u32 { 0 }
    }
    impl DeviceIo for Mock {
        fn reset(&mut self) { self.masked = false; }
        fn mask_device_irq(&mut self) { self.masked = true; }
        fn read_dma16(&mut self, off: usize) -> u16 { u16::from_le_bytes([self.mem[off], self.mem[off+1]]) }
        fn write_dma8(&mut self, off: usize, value: u8) { self.mem[off] = value; }
        fn write_dma16(&mut self, off: usize, value: u16) { self.mem[off..off+2].copy_from_slice(&value.to_le_bytes()); }
        fn write_dma32(&mut self, off: usize, value: u32) { self.mem[off..off+4].copy_from_slice(&value.to_le_bytes()); }
    }
    fn fixture() -> (Mock, EthernetState) {
        let mut state = EthernetState::new();
        state.dma_phys = 0x10000;
        state.dma_lin = 0x20000;
        state.eth_mac = [2, 1, 2, 3, 4, 5];
        (Mock { mem: [0; NET_DMA_SIZE], commands: [0;8], count: 0, fail_configure: false, masked: false }, state)
    }
    #[test]
    fn bringup_order_and_mask() {
        let (mut io, mut state) = fixture();
        let mut stage = 1;
        assert_eq!(initialize(&mut io, &mut state, &mut stage), Ok(()));
        assert_eq!(&io.commands[..io.count], &[0x60, 0x06, 0x10, 0x10, 0x40, 0x01]);
        assert_eq!(stage, 5);
        assert!(io.masked);
        assert!(!state.eth_ready);
        assert_eq!(state.rx_tail, 7);
        assert_eq!(&io.mem[TXCB_OFF+CB_PARAM..TXCB_OFF+CB_PARAM+6], &state.eth_mac);
    }
    #[test]
    fn failed_command_stops_before_receive() {
        let (mut io, mut state) = fixture();
        io.fail_configure = true;
        let mut stage = 1;
        assert_eq!(initialize(&mut io, &mut state, &mut stage), Err(InitError::Configure));
        assert_eq!(&io.commands[..io.count], &[0x60, 0x06, 0x10]);
        assert_eq!(stage, 2);
        assert!(io.masked);
        assert!(!state.eth_ready);
    }
}
