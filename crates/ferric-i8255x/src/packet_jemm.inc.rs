// Shared Jemm packet facade. Included by the integrated and standalone adapters.
pub use ferric_i8255x::dma::{NET_DMA_SIZE, NET_DMA_PARAS};
use ferric_i8255x::dma::{
    DmaRegion, transmit_descriptor,
    RFD_COUNT, RFD_STRIDE, RFD_HDR, FRAME_BUF, TXCB_OFF, TXBUF_OFF, 
    RFD_STATUS, RFD_CMD, RFD_ACTUAL, RFD_SIZE,
    CB_STATUS, CB_CMD, CB_LINK, CB_TBD_ADDR, CB_THRESH, CB_TBD_COUNT, TBD_ADDR, TBD_LEN,
};

// SCB register offsets from the I/O base (BAR1).
const SCB_STATUS: u16 = 0;
const SCB_ACK: u16 = 1;
const SCB_CMD: u16 = 2;
const SCB_CMDHI: u16 = 3;
const SCB_GP: u16 = 4;
const SCB_PORT: u16 = 8;
const SCB_EECTRL: u16 = 0x0E;
const SCB_MDI: u16 = 0x10;

// SCB Stat/Ack bits.
const ACK_CX: u8 = 0x80;
const ACK_FR: u8 = 0x40;
const ACK_CNA: u8 = 0x20;
const ACK_RNR: u8 = 0x10;

// Packet-driver error codes (returned in DH with CF set).
const ERR_BAD_HANDLE: u8 = 1;
const ERR_NO_CLASS: u8 = 2;
const ERR_NO_NUMBER: u8 = 4;
const ERR_BAD_TYPE: u8 = 5;
const ERR_CANT_TERMINATE: u8 = 7;
const ERR_BAD_MODE: u8 = 8;
const ERR_NO_SPACE: u8 = 9;
const ERR_TYPE_INUSE: u8 = 10;
const ERR_BAD_COMMAND: u8 = 11;
const ERR_CANT_SEND: u8 = 12;
const ERR_CANT_SET: u8 = 13;

/// One access_type registration: a client receiver plus the optional
/// EtherType filter it asked for (None = all types, the common case —
/// mTCP registers once with typelen 0 and demultiplexes itself).
#[derive(Clone, Copy)]
struct NetReg {
    active: bool,
    filter: Option<u16>,
    seg: u16,
    off: u16,
}

pub struct NetState {
    ethernet: ferric_i8255x::state::EthernetState,
    present: bool,
    stage: u8,
    irq: u8,
    mac: [u8; 6],
    mac_eeprom: bool,
    rcv_mode: u8,
    promisc: bool,
    regs: [NetReg; 4],
    svc_rfd: i32,
    svc_phase: u8,
    svc_len: u16,
    svc_handle: usize,
    svc_buf_seg: u16,
    svc_buf_off: u16,
    svc_budget: u8,
    rx_packets: u32,
    rx_bytes: u32,
    rx_errors: u32,
    rx_dropped: u32,
    tx_packets: u32,
    tx_bytes: u32,
    tx_errors: u32,
    irq_count: u32,
    #[cfg(not(feature = "ethernet-only"))]
    wifi_mac: [u8; 6],
    wifi_active: bool,
    #[cfg(not(feature = "ethernet-only"))]
    wifi_rx_buf: [u8; 1600],
}

impl NetState {
    const fn new() -> Self {
        Self {
            ethernet: ferric_i8255x::state::EthernetState::new(),
            present: false,
            stage: 0,
            irq: 0,
            mac: [0; 6],
            mac_eeprom: false,
            rcv_mode: 3,
            promisc: false,
            regs: [NetReg {
                active: false,
                filter: None,
                seg: 0,
                off: 0,
            }; 4],
            svc_rfd: -1,
            svc_phase: 0,
            svc_len: 0,
            svc_handle: 0,
            svc_buf_seg: 0,
            svc_buf_off: 0,
            svc_budget: 0,
            rx_packets: 0,
            rx_bytes: 0,
            rx_errors: 0,
            rx_dropped: 0,
            tx_packets: 0,
            tx_bytes: 0,
            tx_errors: 0,
            irq_count: 0,
            #[cfg(not(feature = "ethernet-only"))]
            wifi_mac: [0; 6],
            wifi_active: false,
            #[cfg(not(feature = "ethernet-only"))]
            wifi_rx_buf: [0; 1600],
        }
    }
}

static NET_STATE: SingleCoreCellNet = SingleCoreCellNet(UnsafeCell::new(NetState::new()));

struct SingleCoreCellNet(UnsafeCell<NetState>);
// SAFETY: single-core DOS with CLI/STI discipline, same as the mouse cell.
unsafe impl Sync for SingleCoreCellNet {}

fn net_state() -> &'static mut NetState {
    unsafe { &mut *NET_STATE.0.get() }
}

fn net_reset() {
    *net_state() = NetState::new();
}

/// Switch the INT 60h packet-driver facade to the live AR2413 Wi-Fi backend,
/// preserving the 82550 Ethernet NIC state so `WPROBE /E` can switch back cleanly.
#[cfg(not(feature = "ethernet-only"))]
pub(crate) fn net_activate_wifi(mac: [u8; 6]) {
    let st = net_state();
    st.present = true;
    st.wifi_active = true;
    st.wifi_mac = mac;
    st.stage = 6;
    st.irq = 11;
    st.mac = mac;
    st.mac_eeprom = true;
    // Registrations and the receive mode survive a backend switch: a
    // client that registered a receiver against one adapter must keep
    // receiving after the facade moves to the other, the same promise
    // reset_interface already makes. Only the in-flight delivery state
    // is reset — it describes a frame half-delivered out of the old
    // backend's ring and cannot resume on the new one.
    st.svc_rfd = -1;
    st.svc_phase = 0;
    st.svc_budget = 0;
    if st.ethernet.eth_ready {
        unsafe { outb(st.ethernet.io_base + SCB_CMDHI, 0x01) }; // mask 82550 IRQ while Wi-Fi is active
    }
}

/// Deactivate the Wi-Fi packet-driver backend cleanly: if the 82550 Ethernet
/// NIC is ready, switch `INT 60h` back to Ethernet (unmasking its IRQ and
/// re-arming RU); otherwise mark the facade inactive.
#[cfg(not(feature = "ethernet-only"))]
pub(crate) fn net_deactivate_wifi() {
    let st = net_state();
    if st.ethernet.eth_ready {
        let _ = gate1_net_switch_backend(0);
    } else {
        st.wifi_active = false;
        st.present = false;
        // Registrations survive here too: the facade is inactive, not
        // reset, and a later activation must find clients as they left
        // them. Only the in-flight delivery state is dropped.
        st.svc_rfd = -1;
        st.svc_phase = 0;
        st.svc_budget = 0;
    }
}

#[cfg(not(feature = "ethernet-only"))]
pub(crate) fn net_diag_counts() -> (u32, u32, u32, u32) {
    let st = net_state();
    (st.rx_packets, st.rx_dropped, st.tx_packets, st.tx_errors)
}

/// FFI entry for JLM V86 API `AX=0x000E` (`WPROBE /E` / `WPROBE /W`):
/// switch `INT 60h` between Ethernet (`use_wifi == 0`) and Wi-Fi (`use_wifi != 0`).
/// Returns `1` on success, `0` if the requested adapter is not ready.
#[cfg(not(feature = "ethernet-only"))]
#[unsafe(no_mangle)]
pub extern "C" fn gate1_net_switch_backend(use_wifi: u32) -> u32 {
    let st = net_state();
    // Registrations deliberately survive the switch in both directions
    // (see net_activate_wifi): only per-backend delivery state resets.
    if use_wifi == 0 {
        if !st.ethernet.eth_ready {
            return 0;
        }
        st.wifi_active = false;
        st.present = true;
        st.mac = st.ethernet.eth_mac;
        st.irq = st.ethernet.irq;
        st.mac_eeprom = st.ethernet.mac_from_eeprom;
        st.svc_rfd = -1;
        st.svc_phase = 0;
        st.svc_budget = 0;
        st.ethernet.tx_busy = false;
        // Re-arm 82550 RX ring, unmask device & PIC IRQ line cleanly.
        unsafe {
            let pending = inb(st.ethernet.io_base + SCB_ACK);
            if pending != 0 && pending != 0xFF {
                outb(st.ethernet.io_base + SCB_ACK, pending);
            }
            let ru = inb(st.ethernet.io_base + SCB_STATUS) & 0x3C;
            if ru != 0x10 {
                net_ring_init(st);
                if scb_cmd(st.ethernet.io_base, st.ethernet.dma_phys, 0x01).is_err() {
                    st.present = false;
                    return 0;
                }
            }
            outb(st.ethernet.io_base + SCB_CMDHI, 0x00); // unmask 82550 interrupts
            if st.irq >= 8 {
                let mask = inb(0xA1) & !(1u8 << (st.irq - 8));
                outb(0xA1, mask);
                let master = inb(0x21) & !0x04;
                outb(0x21, master);
            } else {
                let mask = inb(0x21) & !(1u8 << st.irq);
                outb(0x21, mask);
            }
        }
        1
    } else {
        if !ath5k::wifi_is_connected() {
            return 0;
        }
        if st.ethernet.eth_ready {
            unsafe { outb(st.ethernet.io_base + SCB_CMDHI, 0x01) };
        }
        st.wifi_active = true;
        st.present = true;
        st.mac = st.wifi_mac;
        st.irq = 11;
        st.mac_eeprom = true;
        st.svc_rfd = -1;
        st.svc_phase = 0;
        st.svc_budget = 0;
        1
    }
}

// ---------------------------------------------------------------------
// DMA region access (volatile: the device writes these fields too).
// ---------------------------------------------------------------------

fn dma_r8(lin: u32, off: usize) -> u8 {
    unsafe { core::ptr::read_volatile((lin as usize + off) as *const u8) }
}

fn dma_r16(lin: u32, off: usize) -> u16 {
    unsafe { core::ptr::read_volatile((lin as usize + off) as *const u16) }
}

fn dma_w8(lin: u32, off: usize, v: u8) {
    unsafe { core::ptr::write_volatile((lin as usize + off) as *mut u8, v) }
}

fn dma_w16(lin: u32, off: usize, v: u16) {
    unsafe { core::ptr::write_volatile((lin as usize + off) as *mut u16, v) }
}

fn dma_w32(lin: u32, off: usize, v: u32) {
    unsafe { core::ptr::write_volatile((lin as usize + off) as *mut u32, v) }
}

// ---------------------------------------------------------------------
// SCB primitives.
// ---------------------------------------------------------------------

struct NetScbIo(u16);
impl ferric_i8255x::scb::ScbIo for NetScbIo {
    fn command(&mut self) -> u8 { unsafe { inb(self.0 + SCB_CMD) } }
    fn set_pointer(&mut self, p: u32) { unsafe { outl(self.0 + SCB_GP, p) }; }
    fn set_command(&mut self, cmd: u8) { unsafe { outb(self.0 + SCB_CMD, cmd) }; }
    fn tick(&mut self) -> u32 { bios_tick() }
}

const SCB_POLL_LIMIT: u32 = 1_000_000;

unsafe fn scb_cmd(io: u16, gp: u32, cmd: u8) -> Result<(), ferric_i8255x::scb::CommandError> {
    ferric_i8255x::scb::submit(&mut NetScbIo(io), gp, cmd, 2, SCB_POLL_LIMIT)
}

struct NetDeviceIo { port: NetScbIo, lin: u32 }
impl ferric_i8255x::scb::ScbIo for NetDeviceIo {
    fn command(&mut self) -> u8 { self.port.command() }
    fn set_pointer(&mut self, p: u32) { self.port.set_pointer(p); }
    fn set_command(&mut self, cmd: u8) { self.port.set_command(cmd); }
    fn tick(&mut self) -> u32 { self.port.tick() }
}
impl ferric_i8255x::device::DeviceIo for NetDeviceIo {
    fn reset(&mut self) { unsafe { outl(self.port.0 + SCB_PORT, 0) }; }
    fn mask_device_irq(&mut self) { unsafe { outb(self.port.0 + SCB_CMDHI, 1) }; }
    fn read_dma16(&mut self, off: usize) -> u16 { dma_r16(self.lin, off) }
    fn write_dma8(&mut self, off: usize, v: u8) { dma_w8(self.lin, off, v); }
    fn write_dma16(&mut self, off: usize, v: u16) { dma_w16(self.lin, off, v); }
    fn write_dma32(&mut self, off: usize, v: u32) { dma_w32(self.lin, off, v); }
}
fn net_device_io(st: &NetState) -> NetDeviceIo {
    NetDeviceIo { port: NetScbIo(st.ethernet.io_base), lin: st.ethernet.dma_lin }
}

unsafe fn zero_cb(st: &NetState) {
    for i in 0..32 {
        dma_w8(st.ethernet.dma_lin, TXCB_OFF + i, 0);
    }
}

unsafe fn net_configure(st: &mut NetState, promisc: bool) -> bool {
    let Some(region) = DmaRegion::new(st.ethernet.dma_phys, st.ethernet.dma_lin) else { return false; };
    ferric_i8255x::device::configure(&mut net_device_io(st), region, promisc)
}

// Hardware adapters for the shared 8255x serial protocols.
struct NetSerialIo(u16);
impl ferric_i8255x::serial::EepromIo for NetSerialIo {
    fn write_control(&mut self, value: u8) { unsafe { outb(self.0 + SCB_EECTRL, value) }; }
    fn read_control(&mut self) -> u8 { unsafe { inb(self.0 + SCB_EECTRL) } }
}
impl ferric_i8255x::serial::MdiIo for NetSerialIo {
    fn write_mdi(&mut self, value: u32) { unsafe { outl(self.0 + SCB_MDI, value) }; }
    fn read_mdi(&mut self) -> u32 { unsafe { inl(self.0 + SCB_MDI) } }
}
unsafe fn ee_read_mac(io: u16) -> Option<([u8; 6], u8, bool)> {
    ferric_i8255x::serial::read_mac(&mut NetSerialIo(io))
}
unsafe fn link_is_up(io: u16) -> bool {
    ferric_i8255x::serial::link_is_up(&mut NetSerialIo(io), 1)
}

// ---------------------------------------------------------------------
// Probe and bring-up (FFI for the JLM stub).
// ---------------------------------------------------------------------

/// Find the 82550 (8086:1229 hardware, 8086:1209 in QEMU), enable PCI
/// I/O + bus mastering if the BIOS left them off, and read the MAC.
/// Runs no DOS calls and touches no DMA; NETINIT's install step does
/// the rest once it owns two DOS blocks.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_net_probe() -> u32 {
    let st = net_state();
    if st.ethernet.probed {
        return 1;
    }
    for dev in 0..32u8 {
        let id = unsafe { pci_read32(0, dev, 0, 0) };
        let vid = (id & 0xFFFF) as u16;
        let did = (id >> 16) as u16;
        if vid != 0x8086 || (did != 0x1229 && did != 0x1209) {
            continue;
        }
        let bar1 = unsafe { pci_read32(0, dev, 0, 0x14) };
        let irq = (unsafe { pci_read32(0, dev, 0, 0x3C) } & 0xFF) as u8;
        let Some(io) = ferric_i8255x::state::validated_io_bar(bar1, irq) else { continue; };
        if !net_claim_resources(dev, io) { continue; }
        let cmd = unsafe { pci_read32(0, dev, 0, 0x04) } as u16;
        // Enable I/O only at probe; bus mastering starts after DMA validation.
        // Zero the upper (W1C PCI status) half of the command dword.
        if cmd & 1 == 0 {
            unsafe { pci_write32(0, dev, 0, 0x04, u32::from(cmd | 1)) };
        }
        st.ethernet.pci_device = dev;
        st.ethernet.original_command = cmd;
        st.ethernet.io_base = io;
        st.ethernet.irq = irq;
        st.irq = irq;
        match unsafe { ee_read_mac(st.ethernet.io_base) } {
            Some((mac, width, cksum)) => {
                st.ethernet.eth_mac = mac;
                st.ethernet.mac_from_eeprom = true;
                st.mac = mac;
                st.mac_eeprom = true;
                st.ethernet.ee_width = width;
                st.ethernet.ee_cksum = cksum;
            }
            None => {
                // Locally administered fallback ("FERRI" in the low
                // bytes); NTEST's macsrc field reports 0 so a fallback
                // is never mistaken for a real station address.
                st.ethernet.eth_mac = [0x02, 0x46, 0x45, 0x52, 0x52, 0x49];
                st.ethernet.mac_from_eeprom = false;
                st.mac = st.ethernet.eth_mac;
                st.mac_eeprom = false;
            }
        }
        st.ethernet.probed = true;
        st.stage = 1;
        return 1;
    }
    0
}

/// Describe the embedded real-mode stub for the installer:
/// out = [data pointer (JLM linear), length bytes, length paragraphs].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_net_stub_info(out: *mut u32) -> u32 {
    let o = unsafe { &mut *(out as *mut [u32; 3]) };
    o[0] = PKT_STUB.as_ptr() as u32;
    o[1] = PKT_STUB.len() as u32;
    o[2] = (PKT_STUB.len() as u32 + 15) / 16;
    PKT_STUB.len() as u32
}

/// Copy the packet stub into a caller-supplied conventional-memory
/// buffer (linear address below 1 MiB); returns the byte length.
/// The copy is done from here because the stub bytes are embedded in
/// this crate; the caller (NETINIT) owns the destination block and
/// patches the stub's self-segment word once it lands.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_net_stub_copy(dest: u32) -> u32 {
    let dst = dest as *mut u8;
    unsafe { core::ptr::copy_nonoverlapping(PKT_STUB.as_ptr(), dst, PKT_STUB.len()) };
    PKT_STUB.len() as u32
}

// A failed bring-up after DMA lock may have submitted a command. Mask the
// device and request reset, but retain the backing DOS allocation until reboot.
// Return 2 so NETINIT quarantines its allocation instead of freeing live DMA.
unsafe fn net_init_failed(st: &mut NetState) -> u32 {
    unsafe {
        outb(st.ethernet.io_base + SCB_CMDHI, 1);
        outl(st.ethernet.io_base + SCB_PORT, 0);
        // Disable PCI bus mastering as an additional containment measure.
        let dev = st.ethernet.pci_device;
        let cmd = pci_read32(0, dev, 0, 0x04) as u16;
        pci_write32(0, dev, 0, 0x04, u32::from(cmd & !4));
    }
    st.present = false;
    st.ethernet.eth_ready = false;
    2
}


/// Build the receive ring in the DMA block (shared by bring-up and
/// the packet driver's reset_interface).
unsafe fn net_ring_init(st: &mut NetState) {
    let Some(region) = DmaRegion::new(st.ethernet.dma_phys, st.ethernet.dma_lin) else { return; };
    let mut io = net_device_io(st);
    if ferric_i8255x::device::initialize_ring(&mut io, &mut st.ethernet, region) {
        st.svc_rfd = -1;
        st.svc_phase = 0;
    }
}

/// Full device bring-up against the DOS DMA block NETINIT allocated.
/// Stage numbers are the diagnostic contract (NTEST function 80h):
/// 1 probed, 2 DMA validated, 3 reset + configured, 4 IA set,
/// 5 RU started, 6 ready (present).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_net_init(dma_seg: u32) -> u32 {
    let st = net_state();
    if !st.ethernet.probed || st.wifi_active {
        return 0;
    }
    let io = st.ethernet.io_base;
    if dma_seg == 0 || dma_seg > 0xFFFF || st.ethernet.dma_lin != 0 {
        return 0;
    }
    let lin = dma_seg << 4;
    if lin > 0x100000 - NET_DMA_SIZE as u32 {
        return 0;
    }
    let Some(phys) = (unsafe { vdmad_lock_dma_region(lin, NET_DMA_SIZE as u32) }) else {
        return 0;
    };
    if DmaRegion::new(phys, lin).is_none() {
        unsafe { vdmad_unlock_dma_region(lin, NET_DMA_SIZE as u32) };
        return 0;
    }
    st.ethernet.dma_lin = lin;
    st.ethernet.dma_phys = phys;
    let dev = st.ethernet.pci_device;
    let cmd = unsafe { pci_read32(0, dev, 0, 0x04) } as u16;
    unsafe { pci_write32(0, dev, 0, 0x04, u32::from(cmd | 5)) };
    let mut device_io = net_device_io(st);
    if ferric_i8255x::device::initialize(&mut device_io, &mut st.ethernet, &mut st.stage).is_err() {
        return unsafe { net_init_failed(st) };
    }
    st.svc_rfd = -1;
    st.svc_phase = 0;
    st.stage = 5;
    unsafe {
        // Clear any pending CX/CNA status bits left over from Configure / IA
        // setup before unmasking device interrupts.
        let pending = inb(io + SCB_ACK);
        if pending != 0 && pending != 0xFF {
            outb(io + SCB_ACK, pending);
        }
        outb(io + SCB_CMDHI, 0x00); // unmask device interrupts
        // Unmask the IRQ line at the PIC as well: DOS leaves most
        // slave-PIC lines masked, and a driver that wants interrupts
        // unmasks its own. The line is shared (on the DynaBook IRQ 11
        // also carries VGA and OHCI), which is safe because the
        // service routine checks the device's Ack byte before
        // touching anything and chains when the interrupt is not ours.
        if st.irq >= 8 {
            let mask = inb(0xA1) & !(1u8 << (st.irq - 8));
            outb(0xA1, mask);
            let master = inb(0x21) & !0x04; // ensure slave PIC cascade (IRQ 2) is unmasked
            outb(0x21, master);
        } else {
            let mask = inb(0x21) & !(1u8 << st.irq);
            outb(0x21, mask);
        }
    }
    st.present = true;
    st.ethernet.eth_ready = true;
    st.ethernet.eth_mac = st.mac;
    st.stage = 6;
    1
}

// ---------------------------------------------------------------------
// Receive path: IRQ bookkeeping, the upcall state machine, recycling.
// ---------------------------------------------------------------------

/// Timer-poll entry for active Wi-Fi (`INT 1Ch` 18.2 Hz hook): arms the
/// upcall budget when Wi-Fi is active so RX frames are delivered cleanly
/// even if the CardBus slot's PCI INT pin is not routed to IRQ 11 by BIOS.
#[cfg(not(feature = "ethernet-only"))]
#[unsafe(no_mangle)]
pub extern "C" fn gate1_net_poll_wifi() -> u32 {
    let st = net_state();
    if !st.present || !st.wifi_active {
        return 0;
    }
    ath5k::wifi_timer_tick_mark();
    st.svc_budget = 24;
    1
}

/// Front half of IRQ service, called from the INT 73h chain hook
/// before the upcall loop. Implements the SDM shared-IRQ discipline:
/// a silent or absent device means the interrupt belongs to someone
/// else — acknowledge nothing, change nothing, return 0 (chain).
/// Otherwise acknowledge at the device, account TX completion, arm
/// the frame drain, and return 1 (claimed: stub issues slave+master
/// PIC EOI and returns CF=0 so a BIOS default INT 73h handler cannot
/// re-mask IRQ 11 on real hardware).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_net_service() -> u32 {
    let st = net_state();
    if !st.present {
        return 0;
    }
    #[cfg(not(feature = "ethernet-only"))]
    if st.wifi_active {
        if unsafe { ath5k::wifi_isr_ack() } {
            st.irq_count += 1;
            st.svc_budget = 24;
            return 1;
        }
        return 0;
    }
    let io = st.ethernet.io_base;
    let ack = unsafe { inb(io + SCB_ACK) };
    st.ethernet.last_ack = ack;
    if ack == 0 || ack == 0xFF {
        return 0;
    }
    unsafe { outb(io + SCB_ACK, ack) };
    st.irq_count += 1;
    if ack & (ACK_CX | ACK_CNA) != 0 && st.ethernet.tx_busy {
        let status = dma_r16(st.ethernet.dma_lin, TXCB_OFF + CB_STATUS);
        if status & 0x8000 != 0 {
            st.ethernet.tx_busy = false;
            if status & 0x2000 != 0 {
                st.tx_packets += 1;
                st.tx_bytes += st.ethernet.tx_len as u32;
            } else {
                st.tx_errors += 1;
            }
        }
    }
    if ack & ACK_FR != 0 {
        st.svc_budget = RFD_COUNT as u8;
    }
    if ack & ACK_RNR != 0 {
        st.ethernet.ru_starved = true;
    }
    1
}

/// Recycle a consumed RFD: clear it, and move the EL marker onto it —
/// it becomes the tail of the available list. The new EL is set before
/// the old one is cleared, so the list never exists without an end.
unsafe fn net_recycle_rfd(st: &mut NetState, idx: usize) {
    let lin = st.ethernet.dma_lin;
    let off = idx * RFD_STRIDE;
    dma_w16(lin, off + RFD_STATUS, 0);
    dma_w16(lin, off + RFD_ACTUAL, 0);
    dma_w16(lin, off + RFD_SIZE, FRAME_BUF as u16);
    dma_w16(lin, off + RFD_CMD, 0x8000);
    if st.ethernet.rx_tail != idx {
        let toff = st.ethernet.rx_tail * RFD_STRIDE;
        dma_w16(lin, toff + RFD_CMD, 0);
    }
    st.ethernet.rx_tail = idx;
    if st.ethernet.ru_starved && st.ethernet.ru_restart < 0 {
        st.ethernet.ru_restart = idx as i32;
    }
}

/// If the RU stopped for lack of buffers and the drain has since
/// recycled some, restart it at the oldest recycled descriptor.
unsafe fn net_maybe_restart_ru(st: &mut NetState) {
    if !st.ethernet.ru_starved {
        return;
    }
    let io = st.ethernet.io_base;
    let ru = unsafe { inb(io + SCB_STATUS) } & 0x3C;
    if ru == 0x08 && st.ethernet.ru_restart >= 0 {
        let gp = st.ethernet.dma_phys + (st.ethernet.ru_restart as usize * RFD_STRIDE) as u32;
        if unsafe { scb_cmd(io, gp, 0x01) }.is_err() { return; }
        st.ethernet.ru_restart = -1;
    }
    st.ethernet.ru_starved = false;
}

/// Upcall sequencer for the INT 73h stub loop. Fills `out` —
/// [receiver addr (seg<<16|off), phase, length, handle, buf seg,
/// buf off, 0, 0] — and returns 1 when the stub must nest-call the
/// receiver, 0 when this interrupt's receive work is done. Frames
/// with no matching registration, bad status, or a receive mode of
/// "off" are recycled in place without troubling the client.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_net_upcall_next(out: *mut u32) -> u32 {
    let st = net_state();
    let o = unsafe { &mut *(out as *mut [u32; 8]) };
    if !st.present {
        return 0;
    }
    if st.svc_rfd >= 0 && st.svc_phase == 1 {
        let reg = st.regs[st.svc_handle];
        o[0] = ((reg.seg as u32) << 16) | reg.off as u32;
        o[1] = 1;
        o[2] = st.svc_len as u32;
        o[3] = (st.svc_handle + 1) as u32;
        o[4] = st.svc_buf_seg as u32;
        o[5] = st.svc_buf_off as u32;
        o[6] = 0;
        o[7] = 0;
        return 1;
    }
    loop {
        if st.svc_budget == 0 {
            if !st.wifi_active {
                unsafe { net_maybe_restart_ru(st) };
            }
            return 0;
        }
        #[cfg(not(feature = "ethernet-only"))]
        if st.wifi_active {
            let Some(len) = (unsafe { ath5k::wifi_poll_rx_eth(&mut st.wifi_rx_buf) }) else {
                return 0;
            };
            let mut target: i32 = -1;
            let mode_allows = st.rcv_mode != 1
                && !(st.rcv_mode == 2 && (st.wifi_rx_buf[0] & 0x01) != 0);
            if (14..=1518).contains(&len) && mode_allows {
                let et = ((st.wifi_rx_buf[12] as u16) << 8) | (st.wifi_rx_buf[13] as u16);
                for h in 0..4 {
                    let r = st.regs[h];
                    if r.active && (r.filter.is_none() || r.filter == Some(et)) {
                        target = h as i32;
                        break;
                    }
                }
            }
            if target < 0 {
                st.rx_dropped += 1;
                st.svc_budget -= 1;
                continue;
            }
            let h = target as usize;
            st.svc_rfd = 1;
            st.svc_handle = h;
            st.svc_len = len as u16;
            st.svc_phase = 0;
            let reg = st.regs[h];
            o[0] = ((reg.seg as u32) << 16) | reg.off as u32;
            o[1] = 0;
            o[2] = len as u32;
            o[3] = (h + 1) as u32;
            o[4] = 0;
            o[5] = 0;
            o[6] = 0;
            o[7] = 0;
            return 1;
        }
        let idx = st.ethernet.rx_clean;
        let lin = st.ethernet.dma_lin;
        let off = idx * RFD_STRIDE;
        let status = dma_r16(lin, off + RFD_STATUS);
        if status & 0x8000 == 0 {
            unsafe { net_maybe_restart_ru(st) };
            return 0;
        }
        let len = (dma_r16(lin, off + RFD_ACTUAL) & 0x3FFF) as usize;
        let ok = status & 0x2000 != 0;
        let mut target: i32 = -1;
        if ok && (14..=1518).contains(&len) && st.rcv_mode != 1 {
            let et = ((dma_r8(lin, off + RFD_HDR + 12) as u16) << 8)
                | dma_r8(lin, off + RFD_HDR + 13) as u16;
            for h in 0..4 {
                let r = st.regs[h];
                if r.active && (r.filter.is_none() || r.filter == Some(et)) {
                    target = h as i32;
                    break;
                }
            }
        }
        if target < 0 {
            if ok {
                st.rx_dropped += 1;
            } else {
                st.rx_errors += 1;
            }
            unsafe { net_recycle_rfd(st, idx) };
            st.ethernet.rx_clean = (idx + 1) % RFD_COUNT;
            st.svc_budget -= 1;
            continue;
        }
        let h = target as usize;
        st.svc_rfd = idx as i32;
        st.svc_handle = h;
        st.svc_len = len as u16;
        st.svc_phase = 0;
        let reg = st.regs[h];
        o[0] = ((reg.seg as u32) << 16) | reg.off as u32;
        o[1] = 0;
        o[2] = len as u32;
        o[3] = (h + 1) as u32;
        o[4] = 0;
        o[5] = 0;
        o[6] = 0;
        o[7] = 0;
        return 1;
    }
}

/// Second half of each upcall, called by the stub after the nest
/// returns. After phase 0 it reads the offered buffer from out[4..5]
/// (the stub harvested ES:DI there), copies the frame into it, and
/// arms phase 1; a zero offset means the client declined the frame.
/// After phase 1 it recycles the descriptor.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_net_upcall_after(out: *mut u32) {
    let st = net_state();
    let o = unsafe { &*(out as *const [u32; 8]) };
    if st.svc_rfd < 0 {
        return;
    }
    if st.svc_phase == 0 {
        let bseg = (o[4] & 0xFFFF) as u16;
        let boff = (o[5] & 0xFFFF) as u16;
        let idx = st.svc_rfd as usize;
        if v86_buffer(u32::from(bseg), u32::from(boff), st.svc_len as usize).is_none() {
            st.rx_dropped += 1;
            if !st.wifi_active {
                unsafe { net_recycle_rfd(st, idx) };
                st.ethernet.rx_clean = (idx + 1) % RFD_COUNT;
            }
            st.svc_rfd = -1;
            st.svc_budget = st.svc_budget.saturating_sub(1);
            return;
        }
        st.svc_buf_seg = bseg;
        st.svc_buf_off = boff;
        let dst = (((bseg as u32) << 4) + boff as u32) as usize;
        let n = st.svc_len as usize;
        #[cfg(not(feature = "ethernet-only"))]
        if st.wifi_active {
            unsafe { core::ptr::copy_nonoverlapping(st.wifi_rx_buf.as_ptr(), dst as *mut u8, n); }
        }
        if !st.wifi_active {
            let src = st.ethernet.dma_lin as usize + idx * RFD_STRIDE + RFD_HDR;
            unsafe {
                core::ptr::copy_nonoverlapping(src as *const u8, dst as *mut u8, n);
            }
        }
        st.rx_packets += 1;
        st.rx_bytes += n as u32;
        st.svc_phase = 1;
    } else {
        if !st.wifi_active {
            let idx = st.svc_rfd as usize;
            unsafe { net_recycle_rfd(st, idx) };
            st.ethernet.rx_clean = (idx + 1) % RFD_COUNT;
        }
        st.svc_rfd = -1;
        st.svc_phase = 0;
        st.svc_budget = st.svc_budget.saturating_sub(1);
        if !st.wifi_active {
            unsafe { net_maybe_restart_ru(st) };
        }
    }
}

// ---------------------------------------------------------------------
// Transmit path.
// ---------------------------------------------------------------------

/// Copy the client's frame into the TX buffer (zero-padding to the
/// 60-byte Ethernet minimum — runt padding is the driver's job in the
/// packet-driver world), build a one-TBD transmit block, and kick the
/// command unit. One frame in flight at a time: a busy transmitter is
/// waited out with a tick budget, then reported, never spun on.
unsafe fn net_send(st: &mut NetState, src_lin: u32, len: usize) -> bool {
    if len == 0 || len > 1514 {
        return false;
    }
    #[cfg(not(feature = "ethernet-only"))]
    if st.wifi_active {
        if unsafe { ath5k::wifi_tx_eth(src_lin, len) } {
            st.tx_packets += 1;
            st.tx_bytes += len as u32;
            return true;
        }
        st.tx_errors += 1;
        return false;
    }
    if st.ethernet.tx_busy {
        let start = bios_tick();
        for _ in 0..SCB_POLL_LIMIT {
            let status = dma_r16(st.ethernet.dma_lin, TXCB_OFF + CB_STATUS);
            if status & 0x8000 != 0 {
                st.ethernet.tx_busy = false;
                if status & 0x2000 != 0 {
                    st.tx_packets += 1;
                    st.tx_bytes += st.ethernet.tx_len as u32;
                } else {
                    st.tx_errors += 1;
                }
                break;
            }
            if bios_tick().wrapping_sub(start) > 2 {
                return false;
            }
        }
    }
    if st.ethernet.tx_busy { return false; }
    let lin = st.ethernet.dma_lin;
    let Some(region) = DmaRegion::new(st.ethernet.dma_phys, lin) else { return false; };
    let Some(desc) = transmit_descriptor(region, len) else { return false; };
    let wire_len = usize::from(desc.wire_len);
    unsafe {
        core::ptr::copy_nonoverlapping(
            src_lin as *const u8,
            (lin as usize + TXBUF_OFF) as *mut u8,
            len,
        );
        for i in len..wire_len {
            core::ptr::write_volatile((lin as usize + TXBUF_OFF + i) as *mut u8, 0);
        }
        zero_cb(st);
        // Flexible Mode (SF = 0x0008) is required on real 82550 silicon so
        // the CU reads CB_TBD_ADDR / CB_TBD_COUNT / TBD instead of treating
        // the block as Simplified Mode with a zero TCB byte count.
        dma_w16(lin, TXCB_OFF + CB_CMD, desc.command);
        dma_w32(lin, TXCB_OFF + CB_LINK, desc.link);
        dma_w32(lin, TXCB_OFF + CB_TBD_ADDR, desc.tbd);
        dma_w8(lin, TXCB_OFF + CB_THRESH, desc.threshold);
        dma_w8(lin, TXCB_OFF + CB_TBD_COUNT, 1);
        dma_w32(lin, TXCB_OFF + TBD_ADDR, desc.buffer);
        dma_w32(lin, TXCB_OFF + TBD_LEN, u32::from(desc.wire_len));
    }
    st.ethernet.tx_busy = true;
    st.ethernet.tx_len = wire_len as u16;
    let io = st.ethernet.io_base;
    // Because every TX CB sets EL (never S), the CU always returns to Idle
    // (0x00) on completion. Wait briefly for Idle and always CUC_START.
    let start = bios_tick();
    for _ in 0..SCB_POLL_LIMIT {
        if unsafe { inb(io + SCB_STATUS) } & 0xC0 == 0 { break; }
        if bios_tick().wrapping_sub(start) > 2 { break; }
    }
    if unsafe { inb(io + SCB_STATUS) } & 0xC0 != 0 {
        st.ethernet.tx_busy = false;
        return false;
    }
    match unsafe { scb_cmd(io, st.ethernet.dma_phys + TXCB_OFF as u32, 0x10) } {
        Ok(()) => true,
        Err(ferric_i8255x::scb::CommandError::Busy) => { st.ethernet.tx_busy = false; false }
        // A submitted command may still complete; retain the TX buffer.
        Err(ferric_i8255x::scb::CommandError::AcceptanceUnknown) => false,
    }
}

// ---------------------------------------------------------------------
// The packet-driver API itself (INT 60h, via the stub's INT 0F1h).
// Function number in AH, subfunction in AL; errors set CF and put the
// code in DH, per the specification. Returns 1 always: this driver
// owns its vector, and even an unknown function gets a spec-shaped
// refusal rather than a chain into the unknown.
// ---------------------------------------------------------------------

fn net_err(c: &mut [u32; 19], code: u8) {
    c[CR_EFLAGS] |= 1;
    c[CR_EDX] = (c[CR_EDX] & 0xFFFF_00FF) | ((code as u32) << 8);
}

fn net_ok(c: &mut [u32; 19]) {
    c[CR_EFLAGS] &= !1;
}

fn v86_buffer(seg: u32, off: u32, len: usize) -> Option<usize> {
    let off = (off & 0xFFFF) as usize;
    let lin = (((seg & 0xFFFF) << 4) as usize) + off;
    if lin == 0 || len == 0 || off.checked_add(len)? > 0x10000 ||
        lin.checked_add(len)? > 0x100000 { return None; }
    Some(lin)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_net_api(client: *mut u32) -> u32 {
    let st = net_state();
    let c = unsafe { &mut *(client as *mut [u32; 19]) };
    let eax = c[CR_EAX];
    let func = ((eax >> 8) & 0xFF) as u8;
    let al = (eax & 0xFF) as u8;
    if !st.present && func != 0x80 {
        net_err(c, ERR_BAD_COMMAND);
        return 1;
    }
    match func {
        0x01 => {
            // driver_info: AL must be FFh. Returns Crynwr v1.09 fields:
            //   AL = functionality (2 = basic + extended; 0xFF means NOT installed!)
            //   BX = version (0x0101), CH = class (1 = Ethernet), CL = number (0),
            //   DX = type (1), DS:SI = driver name in the real-mode stub block.
            if al != 0xFF {
                net_err(c, ERR_BAD_COMMAND);
                return 1;
            }
            c[CR_EAX] = (c[CR_EAX] & 0xFFFF_FF00) | 0x02;
            c[CR_EBX] = (c[CR_EBX] & 0xFFFF_0000) | 0x0101;
            c[CR_ECX] = (c[CR_ECX] & 0xFFFF_0000) | (1 << 8);
            c[CR_EDX] = (c[CR_EDX] & 0xFFFF_0000) | 0x0001;
            c[CR_DS] = (c[CR_DS] & 0xFFFF_0000) | (c[CR_CS] & 0xFFFF);
            c[CR_ESI] = (c[CR_ESI] & 0xFFFF_0000) | 0x0020;
            net_ok(c);
        }
        0x02 => {
            // access_type: AL=class, BX=type (FFFFh = any), DL=number,
            // DS:SI = type bytes, CX = typelen, ES:DI = receiver.
            if al != 1 {
                net_err(c, ERR_NO_CLASS);
                return 1;
            }
            if c[CR_EDX] & 0xFF != 0 {
                net_err(c, ERR_NO_NUMBER);
                return 1;
            }
            let typelen = (c[CR_ECX] & 0xFFFF) as usize;
            let filter: Option<u16> = if typelen == 0 {
                None
            } else if typelen == 2 {
                let Some(lin) = v86_buffer(c[CR_DS], c[CR_ESI], 2) else { net_err(c, ERR_BAD_TYPE); return 1; };
                let b0 = unsafe { core::ptr::read_volatile(lin as *const u8) };
                let b1 = unsafe { core::ptr::read_volatile((lin + 1) as *const u8) };
                Some(((b0 as u16) << 8) | b1 as u16)
            } else {
                net_err(c, ERR_BAD_TYPE);
                return 1;
            };
            for r in st.regs.iter() {
                if r.active && r.filter == filter {
                    net_err(c, ERR_TYPE_INUSE);
                    return 1;
                }
            }
            match st.regs.iter().position(|r| !r.active) {
                None => net_err(c, ERR_NO_SPACE),
                Some(h) => {
                    st.regs[h] = NetReg {
                        active: true,
                        filter,
                        seg: (c[CR_ES] & 0xFFFF) as u16,
                        off: (c[CR_EDI] & 0xFFFF) as u16,
                    };
                    c[CR_EAX] = (c[CR_EAX] & 0xFFFF_0000) | (h + 1) as u32;
                    // A fresh receiver can unstick a starved ring.
                    st.svc_budget = RFD_COUNT as u8;
                    net_ok(c);
                }
            }
        }
        0x03 => {
            // release_type: BX = handle.
            let h = (c[CR_EBX] & 0xFFFF) as usize;
            if h == 0 || h > 4 || !st.regs[h - 1].active {
                net_err(c, ERR_BAD_HANDLE);
                return 1;
            }
            st.regs[h - 1].active = false;
            net_ok(c);
        }
        0x04 => {
            // send_pkt: DS:SI = frame, CX = length.
            let len = (c[CR_ECX] & 0xFFFF) as usize;
            let Some(src) = v86_buffer(c[CR_DS], c[CR_ESI], len) else { net_err(c, ERR_CANT_SEND); return 1; };
            let src = src as u32;
            if unsafe { net_send(st, src, len) } {
                net_ok(c);
            } else {
                net_err(c, ERR_CANT_SEND);
            }
        }
        0x05 => {
            // terminate: a resident JLM service cannot unload itself
            // out from under the platform; refuse in spec shape.
            net_err(c, ERR_CANT_TERMINATE);
        }
        0x06 => {
            // get_address: BX = handle, ES:DI = buffer, CX = length.
            let h = (c[CR_EBX] & 0xFFFF) as usize;
            if h > 4 || (h >= 1 && !st.regs[h - 1].active) {
                net_err(c, ERR_BAD_HANDLE);
                return 1;
            }
            if (c[CR_ECX] & 0xFFFF) < 6 {
                net_err(c, ERR_NO_SPACE);
                return 1;
            }
            let Some(dst) = v86_buffer(c[CR_ES], c[CR_EDI], 6) else { net_err(c, ERR_NO_SPACE); return 1; };
            for i in 0..6 {
                unsafe { core::ptr::write_volatile((dst + i) as *mut u8, st.mac[i]) };
            }
            c[CR_ECX] = (c[CR_ECX] & 0xFFFF_0000) | 6;
            net_ok(c);
        }
        0x07 => {
            // reset_interface: re-arm the receive ring and restart
            // the RU. Registrations survive — the specification's
            // reset is about the hardware, and dropping a client's
            // receiver out from under it helps nobody.
            if !st.wifi_active {
                unsafe {
                    net_ring_init(st);
                    if scb_cmd(st.ethernet.io_base, st.ethernet.dma_phys, 0x01).is_err() {
                        net_err(c, ERR_CANT_SEND);
                        return 1;
                    }
                }
            }
            net_ok(c);
        }
        0x0A => {
            // get_parameters: DS:SI = 14-byte structure.
            let Some(dst) = v86_buffer(c[CR_DS], c[CR_ESI], 14) else { net_err(c, ERR_NO_SPACE); return 1; };
            let params: [u8; 14] = [
                1, 1, // version major/minor
                14, // structure length
                6,  // address length
                0xDC, 0x05, // MTU 1500
                0, 0, // multicast slots (none advertised)
                8, 0, // receive buffers
                1, 0, // transmit buffers
                0x60, 0, // interrupt number
            ];
            for (i, b) in params.iter().enumerate() {
                unsafe { core::ptr::write_volatile((dst + i) as *mut u8, *b) };
            }
            net_ok(c);
        }
        0x14 => {
            // set_rcv_mode: BX = handle, CX = mode (1..6).
            let h = (c[CR_EBX] & 0xFFFF) as usize;
            if h == 0 || h > 4 || !st.regs[h - 1].active {
                net_err(c, ERR_BAD_HANDLE);
                return 1;
            }
            let mode = (c[CR_ECX] & 0xFFFF) as u8;
            if mode == 0 || mode > 6 {
                net_err(c, ERR_BAD_MODE);
                return 1;
            }
            let want_promisc = mode == 6;
            if !st.wifi_active && want_promisc != st.promisc {
                if st.ethernet.tx_busy {
                    net_err(c, ERR_CANT_SET);
                    return 1;
                }
                if unsafe { net_configure(st, want_promisc) } {
                    st.promisc = want_promisc;
                } else {
                    net_err(c, ERR_CANT_SET);
                    return 1;
                }
            }
            st.rcv_mode = mode;
            net_ok(c);
        }
        0x15 => {
            // get_rcv_mode: BX = handle, returns AX = mode.
            let h = (c[CR_EBX] & 0xFFFF) as usize;
            if h == 0 || h > 4 || !st.regs[h - 1].active {
                net_err(c, ERR_BAD_HANDLE);
                return 1;
            }
            c[CR_EAX] = (c[CR_EAX] & 0xFFFF_0000) | st.rcv_mode as u32;
            net_ok(c);
        }
        0x18 => {
            // get_statistics: BX = handle, DS:SI = 7 dwords
            // (in, out, bytes in, bytes out, errors in, errors out,
            // packets dropped).
            let h = (c[CR_EBX] & 0xFFFF) as usize;
            if h == 0 || h > 4 || !st.regs[h - 1].active {
                net_err(c, ERR_BAD_HANDLE);
                return 1;
            }
            let Some(dst) = v86_buffer(c[CR_DS], c[CR_ESI], 28) else { net_err(c, ERR_NO_SPACE); return 1; };
            let stats: [u32; 7] = [
                st.rx_packets,
                st.tx_packets,
                st.rx_bytes,
                st.tx_bytes,
                st.rx_errors,
                st.tx_errors,
                st.rx_dropped,
            ];
            for (i, v) in stats.iter().enumerate() {
                unsafe {
                    for (j, b) in v.to_le_bytes().iter().enumerate() {
                        core::ptr::write_volatile((dst + i * 4 + j) as *mut u8, *b);
                    }
                }
            }
            net_ok(c);
        }
        0x80 => {
            // Ferric diagnostic: AX = bring-up stage, BX = I/O base,
            // CX = IRQ (low byte) | link<<8 | mac-from-EEPROM<<9 |
            // EEPROM address width<<10 | checksum-valid<<13,
            // DX = IRQ service count (low word).
            let link = st.present && (st.wifi_active || unsafe { link_is_up(st.ethernet.io_base) });
            c[CR_EAX] = (c[CR_EAX] & 0xFFFF_0000) | st.stage as u32;
            c[CR_EBX] = (c[CR_EBX] & 0xFFFF_0000) | st.ethernet.io_base as u32;
            c[CR_ECX] = (c[CR_ECX] & 0xFFFF_0000)
                | (st.irq as u32)
                | ((link as u32) << 8)
                | ((st.mac_eeprom as u32) << 9)
                | ((st.ethernet.ee_width as u32) << 10)
                | ((st.ethernet.ee_cksum as u32) << 13);
            c[CR_EDX] = (c[CR_EDX] & 0xFFFF_0000) | (st.irq_count & 0xFFFF);
            net_ok(c);
        }
        _ => {
            net_err(c, ERR_BAD_COMMAND);
        }
    }
    1
}
