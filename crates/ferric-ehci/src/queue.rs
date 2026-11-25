//! EHCI queue structures: the link-pointer encoding shared by every
//! schedule structure, the queue element transfer descriptor (qTD),
//! and the queue head (QH).
//!
//! Everything here is a DMA structure the controller reads and writes
//! on its own schedule, so the layouts are fixed by the spec (r1.1
//! sections 3.4–3.6): 32-byte alignment for both structures, physical
//! addresses in every pointer field, and the overlay area of the QH
//! mirroring the qTD the controller is currently executing. The types
//! below are plain data built and inspected by the seam; nothing here
//! touches hardware.

/// Structure type codes carried in bits 2:1 of every link pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum LinkType {
    IsochronousTd = 0,
    QueueHead = 1,
    SplitIsochronousTd = 2,
    FrameSpanNode = 3,
}

/// Encode a link pointer: a 32-byte-aligned physical address, the
/// structure type, and the Terminate bit. A terminated pointer ends
/// a schedule branch; the address bits are then ignored by the
/// controller but kept clean here so dumps stay readable.
pub fn link_ptr(phys: u32, ty: LinkType, terminate: bool) -> u32 {
    debug_assert!(phys & 0x1F == 0, "EHCI structures are 32-byte aligned");
    (phys & !0x1F) | ((ty as u32) << 1) | (terminate as u32)
}

/// The canonical terminated pointer.
pub const LINK_TERMINATED: u32 = 1;

/// PID codes for the qTD token (bits 9:8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Pid {
    Out = 0,
    In = 1,
    Setup = 2,
}

/// qTD status bits (token bits 7:0) the completion path reads. The
/// controller writes these; software only interprets them.
pub mod qtd_status {
    /// Data arrived via a split transaction (interrupt endpoints on
    /// a hub); unused by bulk/control on a root port.
    pub const SPLIT_STATE: u32 = 1 << 0;
    /// A micro-frame was missed (isochronous only).
    pub const MISSED_UFRAME: u32 = 1 << 1;
    /// Transaction error: the device did not respond correctly
    /// (timeout, CRC, wrong toggle, ...). Retried per CERR.
    pub const XACT_ERR: u32 = 1 << 2;
    /// Babble detected: the device talked past its turn.
    pub const BABBLE: u32 = 1 << 3;
    /// Data buffer error: the controller could not keep up with DMA.
    pub const DATA_BUF_ERR: u32 = 1 << 4;
    /// Halted: set with an error status when the error counter
    /// reaches zero, and by the controller on a STALL handshake.
    /// A halted qTD is the error terminal state.
    pub const HALTED: u32 = 1 << 6;
    /// The controller is still working on this qTD. Completion, in
    /// the polled driver, is exactly "ACTIVE cleared".
    pub const ACTIVE: u32 = 1 << 7;
    /// Any bit that means the transfer failed.
    pub const ANY_ERROR: u32 = HALTED | DATA_BUF_ERR | BABBLE | XACT_ERR;
}

/// Build a qTD token (spec r1.1 table 3-11). All fields are packed in
/// this one place so the layout has a single point of truth; the
/// recon step pins these positions against real controller behaviour.
///
/// - `cerr`: error counter reload (3 = retry three times; 0 disables
///   retries and is only meaningful for isochronous traffic).
/// - `total_bytes`: up to 0x7FFF across the buffer pages; the caller
///   owns splitting longer transfers.
/// - `data_toggle`: the DT bit the controller seeds its toggle from
///   when the QH's DTC says to take it from the qTD.
pub fn qtd_token(pid: Pid, cerr: u8, ioc: bool, total_bytes: u16, data_toggle: bool) -> u32 {
    debug_assert!(cerr <= 3);
    debug_assert!(total_bytes <= 0x7FFF);
    qtd_status::ACTIVE
        | ((pid as u32) << 8)
        | ((cerr as u32) << 10)
        | ((ioc as u32) << 15)
        | ((total_bytes as u32) << 16)
        | ((data_toggle as u32) << 31)
}

/// One queue element transfer descriptor, in its in-memory form.
/// `pages` are physical page bases: the transfer buffer is described
/// as a starting offset in page 0 (bits 11:0 of `pages[0]`) plus up
/// to four following whole pages, which is how a buffer of up to
/// 20 KiB is described without a scatter list.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Qtd {
    pub next: u32,
    pub alt_next: u32,
    pub token: u32,
    pub pages: [u32; 5],
}

const _: () = assert!(core::mem::size_of::<Qtd>() == 32);
const _: () = assert!(core::mem::align_of::<Qtd>() == 4);

impl Qtd {
    /// An idle descriptor: both links terminated, token zeroed (in
    /// particular ACTIVE clear, so a stray schedule walk over it
    /// completes immediately instead of hanging the controller).
    pub const IDLE: Self = Self {
        next: LINK_TERMINATED,
        alt_next: LINK_TERMINATED,
        token: 0,
        pages: [0; 5],
    };

    /// Point the buffer pages at a physical buffer. `phys` need not
    /// be page-aligned; its offset lands in page 0 and the following
    /// pages are the consecutive physical pages the buffer spans.
    /// The caller guarantees the buffer is physically contiguous —
    /// the resident DMA arena is, by construction.
    pub fn set_buffer(&mut self, phys: u32) {
        let page = phys & !0xFFF;
        let offset = phys & 0xFFF;
        self.pages[0] = page | offset;
        for i in 1..5 {
            self.pages[i] = page + (i as u32) * 0x1000;
        }
    }

    /// Bytes the controller reports as still untransferred, from the
    /// token of a completed (or halted) qTD.
    pub fn bytes_remaining(token: u32) -> u16 {
        ((token >> 16) & 0x7FFF) as u16
    }

    pub fn is_active(token: u32) -> bool {
        token & qtd_status::ACTIVE != 0
    }

    pub fn error_bits(token: u32) -> u32 {
        token & qtd_status::ANY_ERROR
    }
}

/// Endpoint speed codes for the QH characteristics dword.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Speed {
    Full = 0,
    Low = 1,
    High = 2,
}

/// Build the QH endpoint-characteristics dword (spec r1.1 table 3-19).
///
/// - `head`: this QH is the head of the async reclamation list (the
///   skeleton QH the async schedule starts from).
/// - `dtc`: Data Toggle Control — 1 = take the toggle from the qTD,
///   0 = take it from the QH. Bulk/control drivers set 1 and manage
///   the toggle per-qTD; the seam decides per endpoint type.
/// - `nak_reload`: how many NAKs the controller tolerates per
///   micro-frame before yielding (0 = no limit under flow control;
///   interrupt endpoints use a real value).
pub fn qh_characteristics(
    device_addr: u8,
    endpoint: u8,
    speed: Speed,
    max_packet: u16,
    head: bool,
    dtc: bool,
    control_endpoint: bool,
    nak_reload: u8,
) -> u32 {
    debug_assert!(device_addr <= 0x7F);
    debug_assert!(endpoint <= 0xF);
    debug_assert!(max_packet <= 1024, "max packet length field is 10 bits");
    (device_addr as u32)
        | ((endpoint as u32) << 8)
        | ((speed as u32) << 12)
        | ((dtc as u32) << 14)
        | ((head as u32) << 15)
        | ((max_packet as u32) << 16)
        | ((control_endpoint as u32) << 27)
        | ((nak_reload as u32) << 28)
}

/// Build the QH endpoint-capabilities dword (spec r1.1 table 3-20).
/// `mult` is transactions-per-micro-frame (1..=3; 0 is reserved by the
/// EHCI spec and must not be used on active queue heads). `s_mask`/`c_mask`
/// live in the low bytes for periodic-schedule QHs.
pub fn qh_capabilities(mult: u8, port: u8, hub_addr: u8) -> u32 {
    debug_assert!(mult <= 3);
    ((mult as u32) << 30) | ((port as u32) << 16) | ((hub_addr as u32) << 8)
}

/// One queue head, in-memory form: the five hardware dwords followed
/// by the current-qTD pointer and the eight-dword qTD overlay the
/// controller writes while executing. The overlay is controller-owned
/// scratch; software stages the next qTD into it only while the QH is
/// unlinked.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct QueueHead {
    pub horiz_link: u32,
    pub characteristics: u32,
    pub capabilities: u32,
    pub current_qtd: u32,
    pub overlay: Qtd,
    pub overlay_pages_hi: [u32; 5],
}

const _: () = assert!(core::mem::size_of::<QueueHead>() == 68);

impl QueueHead {
    /// A skeleton head-of-reclamation QH for the async schedule: its
    /// horizontal link points at itself (the circular list of one),
    /// marked H, so the controller idles on it until real queue heads
    /// are spliced in behind it.
    pub fn skeleton_async(phys: u32) -> Self {
        Self {
            horiz_link: link_ptr(phys, LinkType::QueueHead, false),
            characteristics: qh_characteristics(0, 0, Speed::High, 64, true, true, false, 0),
            capabilities: qh_capabilities(1, 0, 0),
            current_qtd: 0,
            overlay: Qtd::IDLE,
            overlay_pages_hi: [0; 5],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_pointer_encoding() {
        assert_eq!(link_ptr(0x1000, LinkType::QueueHead, false), 0x1002);
        assert_eq!(link_ptr(0x1000, LinkType::IsochronousTd, true), 0x1001);
        assert_eq!(LINK_TERMINATED, 1);
    }

    #[test]
    fn token_roundtrip_fields() {
        let t = qtd_token(Pid::In, 3, true, 512, true);
        assert!(Qtd::is_active(t));
        assert_eq!((t >> 8) & 0x3, Pid::In as u32);
        assert_eq!((t >> 10) & 0x3, 3);
        assert!(t & (1 << 15) != 0, "IOC set");
        assert_eq!(Qtd::bytes_remaining(t), 512);
        assert!(t & (1 << 31) != 0, "data toggle set");
        assert_eq!(Qtd::error_bits(t), 0);
    }

    #[test]
    fn token_halted_error_is_visible() {
        let t = qtd_status::HALTED | qtd_status::XACT_ERR | (37u32 << 16);
        assert!(!Qtd::is_active(t));
        assert_eq!(Qtd::error_bits(t), qtd_status::HALTED | qtd_status::XACT_ERR);
        assert_eq!(Qtd::bytes_remaining(t), 37);
    }

    #[test]
    fn qtd_buffer_pages_span_from_offset() {
        let mut q = Qtd::IDLE;
        q.set_buffer(0x2_4123);
        assert_eq!(q.pages[0], 0x2_4000 | 0x123);
        assert_eq!(q.pages[1], 0x2_5000);
        assert_eq!(q.pages[4], 0x2_8000);
    }

    #[test]
    fn qh_characteristics_pack() {
        // High-speed bulk endpoint: device 3, endpoint 2 IN would be
        // endpoint field 2, 512-byte packets, DTC on, NAK reload 0.
        let c = qh_characteristics(3, 2, Speed::High, 512, false, true, false, 0);
        assert_eq!(c & 0x7F, 3);
        assert_eq!((c >> 8) & 0xF, 2);
        assert_eq!((c >> 12) & 0x3, Speed::High as u32);
        assert_eq!((c >> 16) & 0x7FF, 512);
        assert!(c & (1 << 14) != 0, "DTC set");
        assert!(c & (1 << 15) == 0, "not head");
    }

    #[test]
    fn skeleton_qh_links_to_itself() {
        let qh = QueueHead::skeleton_async(0x8000);
        assert_eq!(qh.horiz_link, link_ptr(0x8000, LinkType::QueueHead, false));
        assert!(qh.characteristics & (1 << 15) != 0, "H bit set");
        assert_eq!(qh.overlay.token, 0);
    }
}
