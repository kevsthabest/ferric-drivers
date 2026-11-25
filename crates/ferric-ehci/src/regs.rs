//! EHCI register model: capability and operational register offsets
//! and the bit definitions the driver logic depends on.
//!
//! All offsets are from the EHCI spec (r1.1, sections 2.2 and 2.3).
//! Capability registers sit at the start of the MMIO BAR; operational
//! registers start at BAR + CAPLENGTH, which is why every operational
//! offset here is relative to a base the caller computes from the
//! capability block it already read.

/// Byte offsets of the capability registers, from the BAR base.
pub mod cap {
    /// Length of the capability block; operational base = BAR + this.
    pub const CAPLENGTH: u32 = 0x00;
    /// EHCI interface version (BCD), as a u16 at this offset.
    pub const HCIVERSION: u32 = 0x02;
    /// Structural parameters (see `caps::HcsParams`).
    pub const HCSPARAMS: u32 = 0x04;
    /// Capability parameters (see `caps::HccParams`).
    pub const HCCPARAMS: u32 = 0x08;
    /// Port routing rule array (8 bytes, one nibble per port).
    pub const HCSP_PORTROUTE: u32 = 0x0C;
}

/// Byte offsets of the operational registers, from BAR + CAPLENGTH.
pub mod op {
    pub const USBCMD: u32 = 0x00;
    pub const USBSTS: u32 = 0x04;
    pub const USBINTR: u32 = 0x08;
    pub const FRINDEX: u32 = 0x0C;
    pub const CTRLDSSEGMENT: u32 = 0x10;
    pub const PERIODICLISTBASE: u32 = 0x14;
    pub const ASYNCLISTADDR: u32 = 0x18;
    pub const CONFIGFLAG: u32 = 0x40;
    /// First PORTSC register; port n is at PORTSC + 4 * n.
    pub const PORTSC: u32 = 0x44;
}

/// USBCMD bits the bring-up and schedule control use.
pub mod usbcmd {
    /// Run/Stop: the controller executes schedules while set.
    pub const RUN: u32 = 1 << 0;
    /// Host Controller Reset: self-clearing; software must wait for it.
    pub const HCRESET: u32 = 1 << 1;
    /// Periodic Schedule Enable.
    pub const PSE: u32 = 1 << 4;
    /// Async Schedule Enable.
    pub const ASE: u32 = 1 << 5;
    /// Interrupt on Async Advance Doorbell (software rings it after
    /// unlinking queue heads so it can learn when they are safe to
    /// reuse; the status arrives in USBSTS).
    pub const IAA_DOORBELL: u32 = 1 << 6;
    /// Light Host Controller Reset (does not reset port state).
    pub const LIGHT_RESET: u32 = 1 << 7;
}

/// USBSTS bits the polled driver watches. Ferric polls completion from
/// the timer hook rather than taking the EHCI IRQ, so these are read
/// as status, never enabled in USBINTR for v1.
pub mod usbsts {
    /// A transfer completed (or short packet arrived).
    pub const USBINT: u32 = 1 << 0;
    /// A transfer completed with an error.
    pub const USBERRINT: u32 = 1 << 1;
    /// A port connect/enable status changed.
    pub const PORT_CHANGE: u32 = 1 << 2;
    /// Host System Error: the controller hit a fatal PCI/DMA error.
    /// Polling must treat this as "the schedule state is unknown".
    pub const HOST_ERROR: u32 = 1 << 4;
    /// Interrupt on Async Advance: the doorbell answer.
    pub const ASYNC_ADVANCE: u32 = 1 << 5;
    /// Set while the controller is in the reset state; schedules only
    /// run once this clears after RUN is set.
    pub const HCHALTED: u32 = 1 << 12;
    /// 1 while the periodic schedule is enabled and running.
    pub const PERIODIC_STATUS: u32 = 1 << 14;
    /// 1 while the async schedule is enabled and running.
    pub const ASYNC_STATUS: u32 = 1 << 15;
}

/// PORTSC bits (EHCI spec r1.1 table 2-16). One register per port.
pub mod portsc {
    pub const CONNECT: u32 = 1 << 0;
    pub const CONNECT_CHANGE: u32 = 1 << 1;
    /// For an EHCI-owned port this reads 1 only when a high-speed
    /// device passed the reset chirp handshake. A full/low-speed
    /// device never sets it, which is exactly the signal the routing
    /// policy uses to hand the port to its companion.
    pub const ENABLED: u32 = 1 << 2;
    pub const ENABLE_CHANGE: u32 = 1 << 3;
    pub const OVERCURRENT: u32 = 1 << 4;
    pub const OVERCURRENT_CHANGE: u32 = 1 << 5;
    pub const RESUME: u32 = 1 << 6;
    pub const SUSPEND: u32 = 1 << 7;
    /// Writing 1 starts a port reset; the controller clears it when
    /// the reset completes. For a high-speed device the chirp
    /// handshake runs during this reset.
    pub const RESET: u32 = 1 << 8;
    /// Line state (bits 11:10): the pre-reset speed hint. SE0 = idle
    /// bus, J = full-speed idle, K = low-speed idle.
    pub const LINE_STATUS_SHIFT: u32 = 10;
    pub const LINE_STATUS_MASK: u32 = 0x3 << LINE_STATUS_SHIFT;
    pub const LINE_SE0: u32 = 0;
    pub const LINE_J: u32 = 1;
    pub const LINE_K: u32 = 2;
    /// Port Power: meaningful only when HCSPARAMS.PPC says the ports
    /// have individual power control.
    pub const POWER: u32 = 1 << 12;
    /// Port Owner: 1 = the companion controller owns the port,
    /// 0 = the EHCI owns it. This bit is the routing decision itself;
    /// the routing module computes it, the seam applies it.
    pub const OWNER: u32 = 1 << 13;
}

/// CONFIGFLAG values (op register 0x40). Writing 1 routes all ports
/// under EHCI control (per-port ownership then applies); writing 0
/// hands every port back to the companions, which is the pre-Stage-H
/// state of the machine and the required end state whenever the EHCI
/// path is disabled.
pub const CONFIGFLAG_COMPANIONS_OWN: u32 = 0;
pub const CONFIGFLAG_EHCI_ROUTES: u32 = 1;

/// The EHCI extended capabilities live in PCI configuration space at
/// the byte offset HCCPARAMS.EECP names. The first one on every
/// shipping controller is the legacy-support capability, which holds
/// the BIOS/OS ownership semaphores the bring-up must honour before
/// resetting the controller (spec appendix / section 2.1.7 in the
/// extended-capabilities sense — USBLEGSUP).
pub mod legacysup {
    /// Capability ID of the USB Legacy Support capability.
    pub const CAP_ID: u8 = 0x01;
    /// In USBLEGSUP: the BIOS Owned Semaphore (byte 2 of the dword).
    pub const BIOS_SEMAPHORE: u32 = 1 << 16;
    /// In USBLEGSUP: the OS Owned Semaphore (byte 3 of the dword).
    /// Setting it asks the BIOS to hand over; ownership is complete
    /// when BIOS_SEMAPHORE reads back clear.
    pub const OS_SEMAPHORE: u32 = 1 << 24;
}

/// Register access the portable core needs from its host environment.
/// The JLM seam implements this over the mapped MMIO BAR; host tests
/// implement it over an array. Keeping the core behind this trait is
/// what lets the schedule and routing logic be verified without
/// hardware, the same split the filesystem parser cores use.
pub trait EhciRegs {
    /// Read one operational register (offset from BAR + CAPLENGTH).
    fn op_read(&mut self, offset: u32) -> u32;
    /// Write one operational register.
    fn op_write(&mut self, offset: u32, value: u32);
    /// Read one PORTSC register by port index.
    fn port_read(&mut self, port: u8) -> u32 {
        self.op_read(op::PORTSC + 4 * port as u32)
    }
    /// Write one PORTSC register by port index. Callers must preserve
    /// the write-to-clear semantics of the change bits themselves;
    /// this helper intentionally does no masking so tests see exactly
    /// what the seam would write.
    fn port_write(&mut self, port: u8, value: u32) {
        self.op_write(op::PORTSC + 4 * port as u32, value);
    }
}
