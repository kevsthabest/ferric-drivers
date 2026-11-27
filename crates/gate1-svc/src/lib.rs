//! Ferric DOS Gate 1 Track B — Freestanding 32-bit Resident Rust Service.
//!
//! Compiled for `targets/i386-unknown-none.json` (`-x87,-mmx,-sse,-sse2`,
//! `max-atomic-width = 0`, `panic = "abort"`):
//! - Strictly read-only with respect to storage (ZERO INT 13h writes, ZERO ATA
//!   write commands, ZERO MBR/FAT modification).
//! - Strictly DOS non-reentrant safe: ZERO `INT 21h` calls in ISRs or deferred
//!   callbacks; ISR-to-foreground synchronization uses `CLI`/`STI` critical
//!   sections (single-core DOS).
//! - Versioned `abi` service dispatcher (`ABI_MAGIC`, `AbiVersion`,
//!   `RequestHeader`, `ResponseHeader`, `BounceBufferDesc`, `StatusCode`).
//! - Shared level-triggered PCI IRQ 11 bounded ring-buffer work queue with
//!   overflow accounting and chain-safe disposition.
//! - Low-memory physically contiguous DMA bounce-buffer validator (`< 16 MiB`,
//!   alignment, no 64 KiB physical boundary crossing) and deterministic
//!   multi-pass IEEE CRC32 round-trip integrity stress test.

#![cfg_attr(not(test), no_std)]
#![deny(warnings)]

use core::cell::UnsafeCell;
use core::slice;

pub mod ali5451;
pub mod ath5k;
pub mod ath5k_initvals;
pub mod cardbus;
pub mod ehci;
pub mod fat_ro;
pub mod m7101;
pub mod ohci;
pub mod sb_opl;
pub mod sf2;
pub mod toshsd;

use abi::{
    ABI_MAGIC, AbiVersion, BOUNCE_FLAG_BELOW_1M, BOUNCE_FLAG_NO_64K_CROSS, BounceBufferDesc,
    RequestHeader, ResponseHeader, StatusCode,
};

#[cfg(not(test))]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {}
}

/// JLM VxD Device ID reserved for the Ferric DOS Gate 1 resident service.
pub const GATE1_DEVICE_ID: u16 = 0xFD01;

/// Physical address ceiling for 24-bit ISA/low-DMA bounce buffers (16 MiB).
pub const DMA_24BIT_LIMIT: u32 = 0x0100_0000;

/// Hardware IRQ line under test (shared by Intel 82550, NEC OHCI #1, Trident VGA).
pub const PCI_SHARED_IRQ_LINE: u8 = 11;

/// DOS/BIOS interrupt vector corresponding to slave PIC IRQ 11 (PIC2 base 0x70 + 3).
pub const PCI_SHARED_IRQ_VECTOR: u8 = 0x73;

/// Bitmask indicating our device is asserting the shared level-triggered IRQ 11 line.
pub const DEVICE_IRQ_PENDING_MASK: u32 = 0x0000_0001;

/// Bounded capacity of the shared IRQ 11 ring-buffer work queue (power of two).
pub const IRQ11_QUEUE_CAP: usize = 32;

/// Service opcodes (Gate 1 namespace `0x0100..0x01FF`).
pub const OP_SVC_PING: u16 = 0x0100;
pub const OP_SVC_IRQ11_TELEMETRY: u16 = 0x0101;
pub const OP_SVC_IRQ11_STRESS: u16 = 0x0102;
pub const OP_SVC_DMA_STRESS: u16 = 0x0103;
pub const OP_SVC_COEXIST_HEARTBEAT: u16 = 0x0104;
pub const OP_SVC_FORMAT_REPORT: u16 = 0x0105;

/// Caller execution modes recorded during coexistence heartbeats.
pub const CALLER_MODE_REAL: u8 = 1;
pub const CALLER_MODE_PM32: u8 = 2;
pub const CALLER_MODE_JLM_V86: u8 = 3;
pub const CALLER_MODE_JLM_PM: u8 = 4;

/// Host runtime mode reported in `GATE1:SVC`.
pub const HOST_MODE_PM32_TRAMPOLINE: u8 = 1;
pub const HOST_MODE_JLM_RING0: u8 = 2;

/// Serial probe status codes (matching `crates/abi/src/hwscan.rs`).
pub const SERIAL_ABSENT: u8 = 0;
pub const SERIAL_BDA_ZERO: u8 = 1;
pub const SERIAL_SCRATCH_FAIL: u8 = 2;
pub const SERIAL_LSR_TIMEOUT: u8 = 3;
pub const SERIAL_LIVE_UART: u8 = 4;

/// Disposition returned by the shared level-triggered PCI IRQ 11 handler.
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum IrqDisposition {
    /// Device was not asserting IRQ 11; stub MUST chain to previous handler.
    ChainOnly = 0,
    /// Device asserted IRQ 11 and event was queued (or overflow-counted);
    /// on a shared level-triggered PCI line, chaining remains safe.
    HandledAndChain = 1,
}

impl IrqDisposition {
    #[inline]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }
}

/// Bounded event record pushed by the IRQ 11 handler (zero DOS calls).
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Irq11Event {
    pub irq_line: u8,
    pub device_id: u8,
    pub flags: u16,
    pub device_status: u32,
    pub pit_tick: u32,
    pub seq: u32,
}

impl Irq11Event {
    pub const ZERO: Self = Self {
        irq_line: 0,
        device_id: 0,
        flags: 0,
        device_status: 0,
        pit_tick: 0,
        seq: 0,
    };
}

/// Telemetry snapshot for the shared level-triggered PCI IRQ 11 queue.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Irq11Telemetry {
    pub total_invocations: u32,
    pub owned_events: u32,
    pub chained_events: u32,
    pub enqueued: u32,
    pub drained: u32,
    pub overflows: u32,
    pub current_depth: u32,
    pub max_depth: u32,
    pub last_status: u32,
    pub last_tick: u32,
    pub hook_chained_count: u32,
    pub cli_pit_ticks: u32,
}

impl Irq11Telemetry {
    pub const ZERO: Self = Self {
        total_invocations: 0,
        owned_events: 0,
        chained_events: 0,
        enqueued: 0,
        drained: 0,
        overflows: 0,
        current_depth: 0,
        max_depth: 0,
        last_status: 0,
        last_tick: 0,
        hook_chained_count: 0,
        cli_pit_ticks: 0,
    };
}

/// Bounded, overflow-counted, chain-safe ring buffer for shared PCI IRQ 11.
///
/// Single-core DOS invariant: producers run with `IF=0` (hardware ISR or `CLI`
/// critical section) and foreground consumers drain inside `CLI`/`STI`.
pub struct Irq11Queue {
    ring: [Irq11Event; IRQ11_QUEUE_CAP],
    head: usize,
    tail: usize,
    count: usize,
    pub telemetry: Irq11Telemetry,
}

impl Irq11Queue {
    pub const fn new() -> Self {
        Self {
            ring: [Irq11Event::ZERO; IRQ11_QUEUE_CAP],
            head: 0,
            tail: 0,
            count: 0,
            telemetry: Irq11Telemetry::ZERO,
        }
    }

    pub fn reset(&mut self) {
        self.head = 0;
        self.tail = 0;
        self.count = 0;
        self.telemetry = Irq11Telemetry::ZERO;
    }

    /// Handle a shared level-triggered PCI IRQ 11 invocation.
    ///
    /// - Checks `device_status & DEVICE_IRQ_PENDING_MASK`.
    /// - If not asserting, records a pass-through chain event and returns
    ///   `IrqDisposition::ChainOnly`.
    /// - If asserting, enqueues a bounded `Irq11Event` or increments
    ///   `overflows` when full, and returns `IrqDisposition::HandledAndChain`.
    /// - Strictly O(1), bounded, zero allocations, zero DOS `INT 21h` calls.
    pub fn handle_shared_irq(
        &mut self,
        device_status: u32,
        irq_line: u8,
        pit_tick: u32,
    ) -> IrqDisposition {
        self.telemetry.total_invocations = self.telemetry.total_invocations.wrapping_add(1);
        self.telemetry.last_status = device_status;
        self.telemetry.last_tick = pit_tick;

        if (device_status & DEVICE_IRQ_PENDING_MASK) == 0 {
            self.telemetry.chained_events = self.telemetry.chained_events.wrapping_add(1);
            return IrqDisposition::ChainOnly;
        }

        self.telemetry.owned_events = self.telemetry.owned_events.wrapping_add(1);
        let ev = Irq11Event {
            irq_line,
            device_id: ((device_status >> 8) & 0xFF) as u8,
            flags: ((device_status >> 16) & 0xFFFF) as u16,
            device_status,
            pit_tick,
            seq: self.telemetry.owned_events,
        };

        if self.count < IRQ11_QUEUE_CAP {
            self.ring[self.tail] = ev;
            self.tail = (self.tail + 1) & (IRQ11_QUEUE_CAP - 1);
            self.count += 1;
            self.telemetry.enqueued = self.telemetry.enqueued.wrapping_add(1);
            self.telemetry.current_depth = self.count as u32;
            if self.telemetry.current_depth > self.telemetry.max_depth {
                self.telemetry.max_depth = self.telemetry.current_depth;
            }
        } else {
            self.telemetry.overflows = self.telemetry.overflows.wrapping_add(1);
        }

        IrqDisposition::HandledAndChain
    }

    /// Pop one queued IRQ 11 event (must be called inside `CLI`/`STI`).
    pub fn pop(&mut self) -> Option<Irq11Event> {
        if self.count == 0 {
            return None;
        }
        let ev = self.ring[self.head];
        self.head = (self.head + 1) & (IRQ11_QUEUE_CAP - 1);
        self.count -= 1;
        self.telemetry.drained = self.telemetry.drained.wrapping_add(1);
        self.telemetry.current_depth = self.count as u32;
        Some(ev)
    }

    /// Drain at most `budget` events so foreground work stays strictly bounded.
    pub fn drain_bounded(&mut self, budget: u32) -> u32 {
        let mut drained = 0u32;
        while drained < budget {
            if self.pop().is_none() {
                break;
            }
            drained += 1;
        }
        drained
    }

    #[inline]
    pub const fn len(&self) -> usize {
        self.count
    }

    #[inline]
    pub const fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Request payload for `OP_SVC_PING`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct PingRequest {
    pub host_mode: u8,
    pub com1_status: u8,
    pub com1_port: u16,
}

/// Response payload for `OP_SVC_PING`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct PingResponse {
    pub abi_version: AbiVersion,
    pub device_id: u16,
    pub host_mode: u8,
    pub reserved: u8,
    pub service_calls: u32,
}

/// Request payload for `OP_SVC_IRQ11_STRESS`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Irq11StressRequest {
    pub owned_burst: u32,
    pub chained_burst: u32,
    pub overflow_burst: u32,
    pub drain_budget: u32,
    pub hook_chained_count: u32,
    pub cli_pit_ticks: u32,
}

/// Request payload for `OP_SVC_DMA_STRESS`.
///
/// `buf_offset` is the segment-relative pointer (in the caller's 32-bit data
/// segment) to the low-memory bounce buffer whose physical address is
/// `desc.phys_addr`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct DmaStressRequest {
    pub desc: BounceBufferDesc,
    pub buf_offset: u32,
}

/// Result payload for `OP_SVC_DMA_STRESS`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct DmaStressResult {
    pub phys_addr: u32,
    pub size: u32,
    pub align: u32,
    pub below_16m: u8,
    pub below_1m: u8,
    pub no_64k_cross: u8,
    pub neg_tests_passed: u8,
    pub neg_tests_total: u8,
    pub passes_completed: u8,
    pub integrity_ok: u8,
    pub reserved: u8,
    pub final_crc32: u32,
}

impl DmaStressResult {
    pub const ZERO: Self = Self {
        phys_addr: 0,
        size: 0,
        align: 0,
        below_16m: 0,
        below_1m: 0,
        no_64k_cross: 0,
        neg_tests_passed: 0,
        neg_tests_total: 0,
        passes_completed: 0,
        integrity_ok: 0,
        reserved: 0,
        final_crc32: 0,
    };
}

/// Request payload for `OP_SVC_COEXIST_HEARTBEAT`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct CoexistHeartbeatRequest {
    pub caller_mode: u8,
    pub reserved: u8,
    pub caller_id: u16,
    pub pit_tick: u32,
    pub drain_budget: u32,
}

/// Response payload for `OP_SVC_COEXIST_HEARTBEAT`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct CoexistHeartbeatResponse {
    pub rm_calls: u32,
    pub pm32_calls: u32,
    pub jlm_calls: u32,
    pub heartbeat_seq: u32,
    pub drained_now: u32,
    pub state_preserved: u8,
    pub last_caller_mode: u8,
    pub reserved: u16,
}

// Compile-time ABI layout assertions.
const _: () = {
    assert!(core::mem::size_of::<Irq11Event>() == 16);
    assert!(core::mem::size_of::<Irq11Telemetry>() == 48);
    assert!(core::mem::size_of::<PingRequest>() == 4);
    assert!(core::mem::size_of::<PingResponse>() == 12);
    assert!(core::mem::size_of::<Irq11StressRequest>() == 24);
    assert!(core::mem::size_of::<DmaStressRequest>() == 24);
    assert!(core::mem::size_of::<DmaStressResult>() == 24);
    assert!(core::mem::size_of::<CoexistHeartbeatRequest>() == 12);
    assert!(core::mem::size_of::<CoexistHeartbeatResponse>() == 24);
};

/// Validate a low-memory physically contiguous DMA bounce buffer descriptor.
///
/// Enforces:
/// 1. All `abi::BounceBufferDesc::validate()` invariants (non-null, non-zero
///    capacity, `len <= capacity`, power-of-two alignment, aligned `phys_addr`,
///    checked `phys_addr + capacity`, `< 1 MiB` if `BOUNCE_FLAG_BELOW_1M`,
///    no 64 KiB boundary crossing if `BOUNCE_FLAG_NO_64K_CROSS`).
/// 2. Physical address strictly below 16 MiB (`DMA_24BIT_LIMIT`) for 24-bit
///    DMA / low-memory bounce-buffer safety.
/// 3. Non-crossing of any 64 KiB physical boundary (mandatory for ISA/low DMA).
pub const fn validate_dma_bounce(desc: &BounceBufferDesc) -> Result<(), StatusCode> {
    if let Err(e) = desc.validate() {
        return Err(e);
    }
    let Some(end) = desc.phys_addr.checked_add(desc.capacity) else {
        return Err(StatusCode::BounceRangeInvalid);
    };
    if end > DMA_24BIT_LIMIT {
        return Err(StatusCode::BounceRangeInvalid);
    }
    // Always enforce 64 KiB physical boundary non-crossing for DMA bounce buffers.
    let start_page = desc.phys_addr >> 16;
    let last_page = (end - 1) >> 16;
    if start_page != last_page {
        return Err(StatusCode::BounceRangeInvalid);
    }
    Ok(())
}

/// Pure 386 software IEEE 802.3 CRC32 (polynomial `0xEDB88320`, reflected).
///
/// Uses integer shifts and masks only (zero SIMD, zero x87, zero 486+ `bswap`).
pub fn crc32_ieee(data: &[u8]) -> u32 {
    let mut crc: u32 = 0xFFFF_FFFF;
    let mut i = 0;
    while i < data.len() {
        crc ^= data[i] as u32;
        let mut bit = 0;
        while bit < 8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            bit += 1;
        }
        i += 1;
    }
    !crc
}

/// Deterministic byte generator for DMA bounce-buffer integrity passes.
#[inline]
const fn dma_pattern_byte(pass: u8, idx: usize, seed: u32) -> u8 {
    let i = idx as u32;
    match pass {
        0 => {
            if (idx & 1) == 0 {
                0x55
            } else {
                0xAA
            }
        }
        1 => {
            let x = i.wrapping_mul(37).wrapping_add(seed ^ (i >> 3));
            (x & 0xFF) as u8
        }
        _ => {
            let x = i.wrapping_mul(37).wrapping_add(seed ^ (i >> 3));
            (!(x & 0xFF)) as u8
        }
    }
}

/// Run the DMA bounce-buffer validation, negative boundary suite, and 3-pass
/// deterministic fill + IEEE CRC32 round-trip integrity test.
pub fn run_dma_stress(desc: &BounceBufferDesc, buf: &mut [u8]) -> Result<DmaStressResult, StatusCode> {
    validate_dma_bounce(desc)?;
    if (desc.len as usize) == 0 || (desc.len as usize) > buf.len() {
        return Err(StatusCode::InvalidLength);
    }

    let end = desc
        .phys_addr
        .checked_add(desc.capacity)
        .ok_or(StatusCode::BounceRangeInvalid)?;
    let below_16m = u8::from(end <= DMA_24BIT_LIMIT);
    let below_1m = u8::from(end <= abi::LOW_MEM_LIMIT);
    let no_64k_cross = u8::from((desc.phys_addr >> 16) == ((end - 1) >> 16));

    // Negative validation self-tests (must ALL be rejected):
    let mut neg_passed: u8 = 0;
    let neg_total: u8 = 4;

    // 1. Crosses 64 KiB physical boundary (0x0001_FE00 .. 0x0002_0200).
    let neg_64k = BounceBufferDesc {
        phys_addr: 0x0001_FE00,
        capacity: 1024,
        len: 512,
        align: 512,
        flags: BOUNCE_FLAG_BELOW_1M | BOUNCE_FLAG_NO_64K_CROSS,
    };
    if validate_dma_bounce(&neg_64k).is_err() {
        neg_passed += 1;
    }

    // 2. Exceeds 16 MiB 24-bit DMA ceiling (0x0100_0000).
    let neg_16m = BounceBufferDesc {
        phys_addr: 0x0100_0000,
        capacity: 4096,
        len: 4096,
        align: 4096,
        flags: BOUNCE_FLAG_NO_64K_CROSS,
    };
    if validate_dma_bounce(&neg_16m).is_err() {
        neg_passed += 1;
    }

    // 3. Misaligned physical address.
    let neg_align = BounceBufferDesc {
        phys_addr: 0x0002_0008,
        capacity: 512,
        len: 512,
        align: 512,
        flags: BOUNCE_FLAG_BELOW_1M | BOUNCE_FLAG_NO_64K_CROSS,
    };
    if validate_dma_bounce(&neg_align).is_err() {
        neg_passed += 1;
    }

    // 4. Null physical address.
    let neg_null = BounceBufferDesc {
        phys_addr: 0,
        capacity: 512,
        len: 512,
        align: 512,
        flags: BOUNCE_FLAG_BELOW_1M | BOUNCE_FLAG_NO_64K_CROSS,
    };
    if validate_dma_bounce(&neg_null).is_err() {
        neg_passed += 1;
    }

    // 3-pass deterministic fill, byte-by-byte readback, and CRC32 round-trip.
    let active = &mut buf[..(desc.len as usize)];
    let mut integrity_ok = true;
    let mut final_crc = 0u32;
    let mut passes = 0u8;

    let mut pass = 0u8;
    while pass < 3 {
        let mut expected_crc: u32 = 0xFFFF_FFFF;
        let mut i = 0usize;
        while i < active.len() {
            let b = dma_pattern_byte(pass, i, desc.phys_addr);
            active[i] = b;
            // Incrementally compute expected CRC32.
            expected_crc ^= b as u32;
            let mut bit = 0;
            while bit < 8 {
                let mask = 0u32.wrapping_sub(expected_crc & 1);
                expected_crc = (expected_crc >> 1) ^ (0xEDB8_8320 & mask);
                bit += 1;
            }
            i += 1;
        }
        expected_crc = !expected_crc;

        // Verify byte-for-byte readback and full-buffer CRC32 match.
        let mut j = 0usize;
        while j < active.len() {
            if active[j] != dma_pattern_byte(pass, j, desc.phys_addr) {
                integrity_ok = false;
                break;
            }
            j += 1;
        }
        let observed_crc = crc32_ieee(active);
        if observed_crc != expected_crc {
            integrity_ok = false;
        }
        final_crc = observed_crc;
        passes += 1;
        pass += 1;
    }

    Ok(DmaStressResult {
        phys_addr: desc.phys_addr,
        size: desc.len,
        align: desc.align,
        below_16m,
        below_1m,
        no_64k_cross,
        neg_tests_passed: neg_passed,
        neg_tests_total: neg_total,
        passes_completed: passes,
        integrity_ok: u8::from(integrity_ok && neg_passed == neg_total),
        reserved: 0,
        final_crc32: final_crc,
    })
}

/// Persistent resident service state preserved across real-mode and protected-mode calls.
pub struct ResidentServiceState {
    pub initialized: bool,
    pub host_mode: u8,
    pub com1_status: u8,
    pub com1_port: u16,
    pub service_calls: u32,
    pub rm_calls: u32,
    pub pm32_calls: u32,
    pub jlm_calls: u32,
    pub heartbeat_seq: u32,
    pub last_caller_mode: u8,
    pub last_caller_id: u16,
    pub last_pit_tick: u32,
    pub state_canary: u32,
    pub irq11: Irq11Queue,
    pub dma: DmaStressResult,
}

const STATE_CANARY_VALUE: u32 = 0x4644_4F53; // "FDOS"

impl ResidentServiceState {
    pub const fn new() -> Self {
        Self {
            initialized: false,
            host_mode: HOST_MODE_PM32_TRAMPOLINE,
            com1_status: SERIAL_ABSENT,
            com1_port: 0,
            service_calls: 0,
            rm_calls: 0,
            pm32_calls: 0,
            jlm_calls: 0,
            heartbeat_seq: 0,
            last_caller_mode: 0,
            last_caller_id: 0,
            last_pit_tick: 0,
            state_canary: STATE_CANARY_VALUE,
            irq11: Irq11Queue::new(),
            dma: DmaStressResult::ZERO,
        }
    }

    pub fn reset(&mut self) {
        self.initialized = true;
        self.host_mode = HOST_MODE_PM32_TRAMPOLINE;
        self.com1_status = SERIAL_ABSENT;
        self.com1_port = 0;
        self.service_calls = 0;
        self.rm_calls = 0;
        self.pm32_calls = 0;
        self.jlm_calls = 0;
        self.heartbeat_seq = 0;
        self.last_caller_mode = 0;
        self.last_caller_id = 0;
        self.last_pit_tick = 0;
        self.state_canary = STATE_CANARY_VALUE;
        self.irq11.reset();
        self.dma = DmaStressResult::ZERO;
    }

    #[inline]
    pub fn state_preserved(&self) -> bool {
        self.state_canary == STATE_CANARY_VALUE
            && self.heartbeat_seq == self.rm_calls + self.pm32_calls + self.jlm_calls
    }

    /// Dispatch a versioned `abi` request into the resident service.
    ///
    /// `dma_buf` is an optional mutable slice pointing to the caller's low-memory
    /// DMA bounce buffer (used only by `OP_SVC_DMA_STRESS`).
    pub fn dispatch(
        &mut self,
        req_bytes: &[u8],
        resp_bytes: &mut [u8],
        dma_buf: Option<&mut [u8]>,
    ) -> StatusCode {
        if resp_bytes.len() < ResponseHeader::SIZE {
            return StatusCode::BufferTooSmall;
        }
        if req_bytes.len() < RequestHeader::SIZE {
            write_resp_header(
                resp_bytes,
                ResponseHeader::err(0, StatusCode::InvalidLength, ResponseHeader::SIZE as u32),
            );
            return StatusCode::InvalidLength;
        }

        let req = read_request_header(req_bytes);
        if let Err(code) = req.validate(RequestHeader::SIZE as u32) {
            write_resp_header(
                resp_bytes,
                ResponseHeader::err(req.sequence, code, ResponseHeader::SIZE as u32),
            );
            return code;
        }
        if (req.request_len as usize) > req_bytes.len() {
            write_resp_header(
                resp_bytes,
                ResponseHeader::err(
                    req.sequence,
                    StatusCode::InvalidLength,
                    ResponseHeader::SIZE as u32,
                ),
            );
            return StatusCode::InvalidLength;
        }

        self.initialized = true;
        self.service_calls = self.service_calls.wrapping_add(1);
        let payload = &req_bytes[RequestHeader::SIZE..(req.request_len as usize)];
        let resp_cap = resp_bytes.len();
        let (hdr_bytes, out_payload) = resp_bytes.split_at_mut(ResponseHeader::SIZE);

        match req.opcode {
            OP_SVC_PING => {
                if payload.len() >= core::mem::size_of::<PingRequest>() {
                    self.host_mode = payload[0];
                    self.com1_status = payload[1];
                    self.com1_port = u16::from_le_bytes([payload[2], payload[3]]);
                }
                let needed = ResponseHeader::SIZE + core::mem::size_of::<PingResponse>();
                if resp_cap < needed {
                    write_resp_header(
                        hdr_bytes,
                        ResponseHeader::err(
                            req.sequence,
                            StatusCode::BufferTooSmall,
                            needed as u32,
                        ),
                    );
                    return StatusCode::BufferTooSmall;
                }
                let reply = PingResponse {
                    abi_version: AbiVersion::CURRENT,
                    device_id: GATE1_DEVICE_ID,
                    host_mode: self.host_mode,
                    reserved: 0,
                    service_calls: self.service_calls,
                };
                write_ping_response(out_payload, &reply);
                write_resp_header(hdr_bytes, ResponseHeader::ok(req.sequence, needed as u32));
                StatusCode::Ok
            }

            OP_SVC_IRQ11_TELEMETRY => {
                let needed = ResponseHeader::SIZE + core::mem::size_of::<Irq11Telemetry>();
                if resp_cap < needed {
                    write_resp_header(
                        hdr_bytes,
                        ResponseHeader::err(
                            req.sequence,
                            StatusCode::BufferTooSmall,
                            needed as u32,
                        ),
                    );
                    return StatusCode::BufferTooSmall;
                }
                write_irq11_telemetry(out_payload, &self.irq11.telemetry);
                write_resp_header(hdr_bytes, ResponseHeader::ok(req.sequence, needed as u32));
                StatusCode::Ok
            }

            OP_SVC_IRQ11_STRESS => {
                if payload.len() < core::mem::size_of::<Irq11StressRequest>() {
                    write_resp_header(
                        hdr_bytes,
                        ResponseHeader::err(
                            req.sequence,
                            StatusCode::InvalidLength,
                            ResponseHeader::SIZE as u32,
                        ),
                    );
                    return StatusCode::InvalidLength;
                }
                let needed = ResponseHeader::SIZE + core::mem::size_of::<Irq11Telemetry>();
                if resp_cap < needed {
                    write_resp_header(
                        hdr_bytes,
                        ResponseHeader::err(
                            req.sequence,
                            StatusCode::BufferTooSmall,
                            needed as u32,
                        ),
                    );
                    return StatusCode::BufferTooSmall;
                }
                let sreq = read_irq11_stress_req(payload);
                self.irq11.telemetry.hook_chained_count = self
                    .irq11
                    .telemetry
                    .hook_chained_count
                    .wrapping_add(sreq.hook_chained_count);
                self.irq11.telemetry.cli_pit_ticks = self
                    .irq11
                    .telemetry
                    .cli_pit_ticks
                    .wrapping_add(sreq.cli_pit_ticks);

                // 1. Interleaved owned + non-owned (chained) shared PCI IRQ 11 events.
                let mut i = 0u32;
                while i < sreq.owned_burst {
                    let _ = self.irq11.handle_shared_irq(
                        DEVICE_IRQ_PENDING_MASK | ((i & 0xFF) << 8),
                        PCI_SHARED_IRQ_LINE,
                        sreq.cli_pit_ticks.wrapping_add(i),
                    );
                    i += 1;
                }
                let mut c = 0u32;
                while c < sreq.chained_burst {
                    // Device status bit 0 = 0 -> must chain to next IRQ 11 handler.
                    let _ = self.irq11.handle_shared_irq(
                        0x0000_0200,
                        PCI_SHARED_IRQ_LINE,
                        sreq.cli_pit_ticks.wrapping_add(c),
                    );
                    c += 1;
                }
                // Drain first batch under bounded budget.
                let _ = self.irq11.drain_bounded(sreq.drain_budget);

                // 2. Overflow burst (> IRQ11_QUEUE_CAP) to verify boundedness + overflow counting.
                let mut ov = 0u32;
                while ov < sreq.overflow_burst {
                    let _ = self.irq11.handle_shared_irq(
                        DEVICE_IRQ_PENDING_MASK | 0x0001_0000,
                        PCI_SHARED_IRQ_LINE,
                        sreq.cli_pit_ticks.wrapping_add(ov),
                    );
                    ov += 1;
                }
                // Drain remaining queued events cleanly.
                let _ = self.irq11.drain_bounded(IRQ11_QUEUE_CAP as u32);

                write_irq11_telemetry(out_payload, &self.irq11.telemetry);
                write_resp_header(hdr_bytes, ResponseHeader::ok(req.sequence, needed as u32));
                StatusCode::Ok
            }

            OP_SVC_DMA_STRESS => {
                if payload.len() < core::mem::size_of::<DmaStressRequest>() {
                    write_resp_header(
                        hdr_bytes,
                        ResponseHeader::err(
                            req.sequence,
                            StatusCode::InvalidLength,
                            ResponseHeader::SIZE as u32,
                        ),
                    );
                    return StatusCode::InvalidLength;
                }
                let needed = ResponseHeader::SIZE + core::mem::size_of::<DmaStressResult>();
                if resp_cap < needed {
                    write_resp_header(
                        hdr_bytes,
                        ResponseHeader::err(
                            req.sequence,
                            StatusCode::BufferTooSmall,
                            needed as u32,
                        ),
                    );
                    return StatusCode::BufferTooSmall;
                }
                let dreq = read_dma_stress_req(payload);
                let Some(buf) = dma_buf else {
                    write_resp_header(
                        hdr_bytes,
                        ResponseHeader::err(
                            req.sequence,
                            StatusCode::InvalidPointer,
                            ResponseHeader::SIZE as u32,
                        ),
                    );
                    return StatusCode::InvalidPointer;
                };
                match run_dma_stress(&dreq.desc, buf) {
                    Ok(res) => {
                        self.dma = res;
                        write_dma_stress_result(out_payload, &res);
                        write_resp_header(
                            hdr_bytes,
                            ResponseHeader::ok(req.sequence, needed as u32),
                        );
                        StatusCode::Ok
                    }
                    Err(code) => {
                        write_resp_header(
                            hdr_bytes,
                            ResponseHeader::err(req.sequence, code, ResponseHeader::SIZE as u32),
                        );
                        code
                    }
                }
            }

            OP_SVC_COEXIST_HEARTBEAT => {
                if payload.len() < core::mem::size_of::<CoexistHeartbeatRequest>() {
                    write_resp_header(
                        hdr_bytes,
                        ResponseHeader::err(
                            req.sequence,
                            StatusCode::InvalidLength,
                            ResponseHeader::SIZE as u32,
                        ),
                    );
                    return StatusCode::InvalidLength;
                }
                let needed =
                    ResponseHeader::SIZE + core::mem::size_of::<CoexistHeartbeatResponse>();
                if resp_cap < needed {
                    write_resp_header(
                        hdr_bytes,
                        ResponseHeader::err(
                            req.sequence,
                            StatusCode::BufferTooSmall,
                            needed as u32,
                        ),
                    );
                    return StatusCode::BufferTooSmall;
                }
                let hreq = read_coexist_req(payload);
                match hreq.caller_mode {
                    CALLER_MODE_REAL => self.rm_calls = self.rm_calls.wrapping_add(1),
                    CALLER_MODE_PM32 => self.pm32_calls = self.pm32_calls.wrapping_add(1),
                    CALLER_MODE_JLM_V86 | CALLER_MODE_JLM_PM => {
                        self.jlm_calls = self.jlm_calls.wrapping_add(1)
                    }
                    _ => {
                        write_resp_header(
                            hdr_bytes,
                            ResponseHeader::err(
                                req.sequence,
                                StatusCode::NotSupported,
                                ResponseHeader::SIZE as u32,
                            ),
                        );
                        return StatusCode::NotSupported;
                    }
                }
                self.heartbeat_seq = self.heartbeat_seq.wrapping_add(1);
                self.last_caller_mode = hreq.caller_mode;
                self.last_caller_id = hreq.caller_id;
                self.last_pit_tick = hreq.pit_tick;
                let drained_now = self.irq11.drain_bounded(hreq.drain_budget);

                let reply = CoexistHeartbeatResponse {
                    rm_calls: self.rm_calls,
                    pm32_calls: self.pm32_calls,
                    jlm_calls: self.jlm_calls,
                    heartbeat_seq: self.heartbeat_seq,
                    drained_now,
                    state_preserved: u8::from(self.state_preserved()),
                    last_caller_mode: self.last_caller_mode,
                    reserved: 0,
                };
                write_coexist_resp(out_payload, &reply);
                write_resp_header(hdr_bytes, ResponseHeader::ok(req.sequence, needed as u32));
                StatusCode::Ok
            }

            OP_SVC_FORMAT_REPORT => {
                let (written, overflowed) = {
                    let mut w = TextWriter::new(out_payload);
                    self.format_report(&mut w);
                    (w.written, w.overflowed)
                };
                if overflowed {
                    write_resp_header(
                        hdr_bytes,
                        ResponseHeader::err(
                            req.sequence,
                            StatusCode::BufferTooSmall,
                            (ResponseHeader::SIZE + written) as u32,
                        ),
                    );
                    return StatusCode::BufferTooSmall;
                }
                let total_len = (ResponseHeader::SIZE + written) as u32;
                write_resp_header(hdr_bytes, ResponseHeader::ok(req.sequence, total_len));
                StatusCode::Ok
            }

            _ => {
                write_resp_header(
                    hdr_bytes,
                    ResponseHeader::err(
                        req.sequence,
                        StatusCode::NotSupported,
                        ResponseHeader::SIZE as u32,
                    ),
                );
                StatusCode::NotSupported
            }
        }
    }

    /// Format the deterministic Gate 1 Track B evidence report with all required markers.
    pub fn format_report(&self, w: &mut TextWriter<'_>) {
        let mode_str = match self.host_mode {
            HOST_MODE_JLM_RING0 => "JLM_RING0",
            _ => "PM32_RESIDENT",
        };
        let com1_str = match self.com1_status {
            SERIAL_LIVE_UART => "LIVE",
            SERIAL_BDA_ZERO => "BDA_ZERO",
            SERIAL_SCRATCH_FAIL => "SCRATCH_FAIL",
            SERIAL_LSR_TIMEOUT => "LSR_TIMEOUT",
            _ => "ABSENT",
        };

        let svc_pass = self.initialized && self.service_calls > 0;
        let irq_pass = self.irq11.telemetry.owned_events > 0
            && self.irq11.telemetry.chained_events > 0
            && self.irq11.telemetry.overflows > 0
            && self.irq11.telemetry.drained == self.irq11.telemetry.enqueued
            && self.irq11.is_empty();
        let dma_pass = self.dma.integrity_ok == 1
            && self.dma.below_16m == 1
            && self.dma.no_64k_cross == 1
            && self.dma.passes_completed == 3;
        let coexist_pass = self.rm_calls > 0
            && (self.pm32_calls > 0 || self.jlm_calls > 0)
            && self.state_preserved();

        w.push_str("=== FERRIC-DOS GATE1 BEGIN ===\r\n");

        w.push_str("GATE1:SVC abi=");
        w.push_u32(AbiVersion::CURRENT.major as u32);
        w.push_str(".");
        w.push_u32(AbiVersion::CURRENT.minor as u32);
        w.push_str(" magic=FDOS ddb=0x");
        w.push_hex16(GATE1_DEVICE_ID);
        w.push_str(" mode=");
        w.push_str(mode_str);
        w.push_str(" calls=");
        w.push_u32(self.service_calls);
        w.push_str(" status=");
        w.push_str(if svc_pass { "PASS" } else { "FAIL" });
        w.push_str("\r\n");

        w.push_str("GATE1:IRQ11 vec=0x");
        w.push_hex8(PCI_SHARED_IRQ_VECTOR);
        w.push_str(" hook_chain=");
        w.push_u32(self.irq11.telemetry.hook_chained_count);
        w.push_str(" total=");
        w.push_u32(self.irq11.telemetry.total_invocations);
        w.push_str(" owned=");
        w.push_u32(self.irq11.telemetry.owned_events);
        w.push_str(" chained=");
        w.push_u32(self.irq11.telemetry.chained_events);
        w.push_str(" enq=");
        w.push_u32(self.irq11.telemetry.enqueued);
        w.push_str(" drained=");
        w.push_u32(self.irq11.telemetry.drained);
        w.push_str(" overflows=");
        w.push_u32(self.irq11.telemetry.overflows);
        w.push_str(" max_depth=");
        w.push_u32(self.irq11.telemetry.max_depth);
        w.push_str(" cli_pit_ticks=");
        w.push_u32(self.irq11.telemetry.cli_pit_ticks);
        w.push_str(" status=");
        w.push_str(if irq_pass { "PASS" } else { "FAIL" });
        w.push_str("\r\n");

        w.push_str("GATE1:DMA phys=0x");
        w.push_hex32(self.dma.phys_addr);
        w.push_str(" size=");
        w.push_u32(self.dma.size);
        w.push_str(" align=");
        w.push_u32(self.dma.align);
        w.push_str(" below_16m=");
        w.push_u32(self.dma.below_16m as u32);
        w.push_str(" below_1m=");
        w.push_u32(self.dma.below_1m as u32);
        w.push_str(" no_64k_cross=");
        w.push_u32(self.dma.no_64k_cross as u32);
        w.push_str(" neg_tests=");
        w.push_u32(self.dma.neg_tests_passed as u32);
        w.push_str("/");
        w.push_u32(self.dma.neg_tests_total as u32);
        w.push_str(" passes=");
        w.push_u32(self.dma.passes_completed as u32);
        w.push_str(" crc32=0x");
        w.push_hex32(self.dma.final_crc32);
        w.push_str(" status=");
        w.push_str(if dma_pass { "PASS" } else { "FAIL" });
        w.push_str("\r\n");

        w.push_str("GATE1:COEXIST rm_calls=");
        w.push_u32(self.rm_calls);
        w.push_str(" pm32_calls=");
        w.push_u32(self.pm32_calls);
        w.push_str(" jlm_calls=");
        w.push_u32(self.jlm_calls);
        w.push_str(" heartbeat_seq=");
        w.push_u32(self.heartbeat_seq);
        w.push_str(" state_preserved=");
        w.push_u32(u32::from(self.state_preserved()));
        w.push_str(" com1=");
        w.push_str(com1_str);
        w.push_str(" port=0x");
        w.push_hex16(self.com1_port);
        w.push_str(" status=");
        w.push_str(if coexist_pass { "PASS" } else { "FAIL" });
        w.push_str("\r\n");

        w.push_str("=== FERRIC-DOS GATE1 END ===\r\n");
    }
}

/// Bounded ASCII formatter with zero heap allocations and zero floating-point.
pub struct TextWriter<'a> {
    buf: &'a mut [u8],
    pub written: usize,
    pub overflowed: bool,
}

impl<'a> TextWriter<'a> {
    pub fn new(buf: &'a mut [u8]) -> Self {
        Self {
            buf,
            written: 0,
            overflowed: false,
        }
    }

    pub fn push_bytes(&mut self, bytes: &[u8]) {
        let mut i = 0;
        while i < bytes.len() {
            if self.written < self.buf.len() {
                self.buf[self.written] = bytes[i];
                self.written += 1;
            } else {
                self.overflowed = true;
            }
            i += 1;
        }
    }

    #[inline]
    pub fn push_str(&mut self, s: &str) {
        self.push_bytes(s.as_bytes());
    }

    pub fn push_u32(&mut self, mut val: u32) {
        let mut digits = [0u8; 10];
        let mut len = 0usize;
        if val == 0 {
            self.push_bytes(b"0");
            return;
        }
        while val > 0 {
            let rem = (val % 10) as u8;
            digits[len] = b'0' + rem;
            len += 1;
            val /= 10;
        }
        while len > 0 {
            len -= 1;
            self.push_bytes(&digits[len..len + 1]);
        }
    }

    pub fn push_hex_digits(&mut self, val: u32, count: usize) {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let mut i = count;
        while i > 0 {
            i -= 1;
            let nib = ((val >> (i * 4)) & 0x0F) as usize;
            self.push_bytes(&HEX[nib..nib + 1]);
        }
    }

    #[inline]
    pub fn push_hex8(&mut self, val: u8) {
        self.push_hex_digits(val as u32, 2);
    }

    #[inline]
    pub fn push_hex16(&mut self, val: u16) {
        self.push_hex_digits(val as u32, 4);
    }

    #[inline]
    pub fn push_hex32(&mut self, val: u32) {
        self.push_hex_digits(val, 8);
    }
}

// =============================================================================
// Little-endian wire encoding/decoding helpers (alignment-safe, zero unsafe)
// =============================================================================

#[inline]
fn read_u16_le(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

#[inline]
fn read_u32_le(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[inline]
fn write_u16_le(b: &mut [u8], off: usize, v: u16) {
    let bytes = v.to_le_bytes();
    b[off] = bytes[0];
    b[off + 1] = bytes[1];
}

#[inline]
fn write_u32_le(b: &mut [u8], off: usize, v: u32) {
    let bytes = v.to_le_bytes();
    b[off] = bytes[0];
    b[off + 1] = bytes[1];
    b[off + 2] = bytes[2];
    b[off + 3] = bytes[3];
}

fn read_request_header(b: &[u8]) -> RequestHeader {
    RequestHeader {
        magic: read_u32_le(b, 0),
        version: AbiVersion {
            major: read_u16_le(b, 4),
            minor: read_u16_le(b, 6),
        },
        opcode: read_u16_le(b, 8),
        flags: read_u16_le(b, 10),
        request_len: read_u32_le(b, 12),
        response_cap: read_u32_le(b, 16),
        sequence: read_u32_le(b, 20),
    }
}

fn write_resp_header(b: &mut [u8], h: ResponseHeader) {
    write_u32_le(b, 0, h.magic);
    write_u16_le(b, 4, h.version.major);
    write_u16_le(b, 6, h.version.minor);
    write_u32_le(b, 8, h.status);
    write_u32_le(b, 12, h.response_len);
    write_u32_le(b, 16, h.required_len);
    write_u32_le(b, 20, h.sequence);
}

fn write_ping_response(b: &mut [u8], r: &PingResponse) {
    write_u16_le(b, 0, r.abi_version.major);
    write_u16_le(b, 2, r.abi_version.minor);
    write_u16_le(b, 4, r.device_id);
    b[6] = r.host_mode;
    b[7] = r.reserved;
    write_u32_le(b, 8, r.service_calls);
}

fn read_irq11_stress_req(b: &[u8]) -> Irq11StressRequest {
    Irq11StressRequest {
        owned_burst: read_u32_le(b, 0),
        chained_burst: read_u32_le(b, 4),
        overflow_burst: read_u32_le(b, 8),
        drain_budget: read_u32_le(b, 12),
        hook_chained_count: read_u32_le(b, 16),
        cli_pit_ticks: read_u32_le(b, 20),
    }
}

fn write_irq11_telemetry(b: &mut [u8], t: &Irq11Telemetry) {
    write_u32_le(b, 0, t.total_invocations);
    write_u32_le(b, 4, t.owned_events);
    write_u32_le(b, 8, t.chained_events);
    write_u32_le(b, 12, t.enqueued);
    write_u32_le(b, 16, t.drained);
    write_u32_le(b, 20, t.overflows);
    write_u32_le(b, 24, t.current_depth);
    write_u32_le(b, 28, t.max_depth);
    write_u32_le(b, 32, t.last_status);
    write_u32_le(b, 36, t.last_tick);
    write_u32_le(b, 40, t.hook_chained_count);
    write_u32_le(b, 44, t.cli_pit_ticks);
}

fn read_dma_stress_req(b: &[u8]) -> DmaStressRequest {
    DmaStressRequest {
        desc: BounceBufferDesc {
            phys_addr: read_u32_le(b, 0),
            capacity: read_u32_le(b, 4),
            len: read_u32_le(b, 8),
            align: read_u32_le(b, 12),
            flags: read_u32_le(b, 16),
        },
        buf_offset: read_u32_le(b, 20),
    }
}

fn write_dma_stress_result(b: &mut [u8], r: &DmaStressResult) {
    write_u32_le(b, 0, r.phys_addr);
    write_u32_le(b, 4, r.size);
    write_u32_le(b, 8, r.align);
    b[12] = r.below_16m;
    b[13] = r.below_1m;
    b[14] = r.no_64k_cross;
    b[15] = r.neg_tests_passed;
    b[16] = r.neg_tests_total;
    b[17] = r.passes_completed;
    b[18] = r.integrity_ok;
    b[19] = r.reserved;
    write_u32_le(b, 20, r.final_crc32);
}

fn read_coexist_req(b: &[u8]) -> CoexistHeartbeatRequest {
    CoexistHeartbeatRequest {
        caller_mode: b[0],
        reserved: b[1],
        caller_id: read_u16_le(b, 2),
        pit_tick: read_u32_le(b, 4),
        drain_budget: read_u32_le(b, 8),
    }
}

fn write_coexist_resp(b: &mut [u8], r: &CoexistHeartbeatResponse) {
    write_u32_le(b, 0, r.rm_calls);
    write_u32_le(b, 4, r.pm32_calls);
    write_u32_le(b, 8, r.jlm_calls);
    write_u32_le(b, 12, r.heartbeat_seq);
    write_u32_le(b, 16, r.drained_now);
    b[20] = r.state_preserved;
    b[21] = r.last_caller_mode;
    write_u16_le(b, 22, r.reserved);
}

// =============================================================================
// Single-core resident singleton & 32-bit cdecl C-ABI entry points
// =============================================================================

struct SingleCoreCell(UnsafeCell<ResidentServiceState>);

// SAFETY: FreeDOS + 386 baseline is strictly single-core; all entry points
// execute inside `CLI` critical sections or Ring-0 JLM / PM32 trampoline calls.
unsafe impl Sync for SingleCoreCell {}

static RESIDENT_SVC: SingleCoreCell = SingleCoreCell(UnsafeCell::new(ResidentServiceState::new()));

struct SingleCoreFlag(UnsafeCell<bool>);

// SAFETY: same single-core invariant as SingleCoreCell above.
unsafe impl Sync for SingleCoreFlag {}

/// Hosting fact, not service state: whether Jemm's VDMAD services
/// (`INT 20h`, device `0x0004`) exist underneath this copy of the core.
/// The JLM stub sets it at `DLL_PROCESS_ATTACH` (that PE is only ever
/// loaded by JLOAD, i.e. with Jemm resident). The standalone SVCTEST
/// harness links the same core without Jemm and never sets it, so its
/// DMA path keeps validating by descriptor rules alone. The flag
/// deliberately survives `gate1_svc_reset()`: a reset reinitializes
/// service state, not facts about the host.
static PLATFORM_VDMAD: SingleCoreFlag = SingleCoreFlag(UnsafeCell::new(false));

fn platform_vdmad_enabled() -> bool {
    unsafe { *PLATFORM_VDMAD.0.get() }
}

/// Declare whether VDMAD services are available in this hosting context.
///
/// # Safety
/// Must be called with interrupts disabled (`IF=0`), before any service
/// dispatch, and only when the caller has established that Jemm/JLOAD is
/// the host (the JLM stub's `DllMain` attach satisfies this by construction).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_svc_set_platform_vdmad(enabled: u32) {
    unsafe { *PLATFORM_VDMAD.0.get() = enabled != 0; }
}

/// Jemm `VDMAD_Lock_DMA_Region` (`INT 20h`, service 1, device 4): walks
/// Jemm's page tables for the region, verifies physical contiguity and,
/// with `DL=1`, that the region does not cross a 64 KiB physical
/// boundary. Returns the physical base in `EDX` with `CF` clear on
/// success; `CF` set on failure.
///
/// Register contract (JLM.INC / VDMAD.ASM): `ESI` = linear address,
/// `ECX` = size in bytes, `DL` = flags; `EAX`/`ECX`/`EDX` are
/// caller-saved, `EBX`/`ESI`/`EDI`/`EBP` are preserved by the service.
/// The two `.word` operands after `int 0x20` are the VxDCall inline
/// service/device pair; the handler resumes execution after them, so
/// the `setc` reads the service's own carry result.
#[cfg(target_arch = "x86")]
unsafe fn vdmad_lock_dma_region(lin: u32, len: u32) -> Option<u32> {
    let phys: u32;
    let failed: u32;
    unsafe {
        // Rust forbids explicit `in("esi")` on x86, while `out(reg)` CAN still
        // be allocated to ESI (which caused `pop esi` to clobber `phys`!).
        // Pin every operand to explicit non-ESI registers (EAX/ECX/EDX) so
        // `push esi` / `pop esi` can never alias any input or output.
        core::arch::asm!(
            "push esi",
            "mov esi, eax",
            "int 0x20",
            ".word 1", // VDMAD_Lock_DMA_Region
            ".word 4", // device: VDMAD
            "sbb eax, eax",
            "pop esi",
            inlateout("eax") lin => failed,
            inlateout("ecx") len => _,
            inlateout("edx") 1u32 => phys,
        );
    }
    if failed == 0 { Some(phys) } else { None }
}

/// Host builds (unit tests) have no Jemm underneath; the platform flag is
/// never set there, so this body is unreachable in practice.
#[cfg(not(target_arch = "x86"))]
unsafe fn vdmad_lock_dma_region(_lin: u32, _len: u32) -> Option<u32> {
    None
}

/// Jemm `VDMAD_Lock_DMA_Region` with `DL=0` (verify physical contiguity
/// without the legacy 64 KiB ISA DMA boundary restriction). Suitable for
/// 32-bit PCI bus-master NICs such as Atheros AR2413.
#[cfg(target_arch = "x86")]
pub(crate) unsafe fn vdmad_lock_dma_region_pci(lin: u32, len: u32) -> Option<u32> {
    let phys: u32;
    let failed: u32;
    unsafe {
        core::arch::asm!(
            "push esi",
            "mov esi, eax",
            "int 0x20",
            ".word 1", // VDMAD_Lock_DMA_Region
            ".word 4", // device: VDMAD
            "sbb eax, eax",
            "pop esi",
            inlateout("eax") lin => failed,
            inlateout("ecx") len => _,
            inlateout("edx") 0u32 => phys,
        );
    }
    if failed == 0 { Some(phys) } else { None }
}

#[cfg(not(target_arch = "x86"))]
pub(crate) unsafe fn vdmad_lock_dma_region_pci(_lin: u32, _len: u32) -> Option<u32> {
    None
}

/// Jemm `VDMAD_Unlock_DMA_Region` (`INT 20h`, service 2, device 4).
/// Jemm's implementation only re-checks the region; there is no pin to
/// release, but the paired call keeps the lock/unlock contract explicit
/// for hosts whose VDMAD does pin.
#[cfg(target_arch = "x86")]
pub(crate) unsafe fn vdmad_unlock_dma_region(lin: u32, len: u32) {
    unsafe {
        core::arch::asm!(
            "push esi",
            "mov esi, eax",
            "int 0x20",
            ".word 2", // VDMAD_Unlock_DMA_Region
            ".word 4", // device: VDMAD
            "pop esi",
            inlateout("eax") lin => _,
            inlateout("ecx") len => _,
            inlateout("edx") 1u32 => _,
        );
    }
}

#[cfg(not(target_arch = "x86"))]
pub(crate) unsafe fn vdmad_unlock_dma_region(_lin: u32, _len: u32) {}

/// Primary 32-bit cdecl service entry point (placed in `.text.entry` for
/// deterministic VMA `0x1800` in `SVCTEST.COM`, and exported to `FERRIC.DLL`).
///
/// # Safety
/// - `req_ptr` must be valid for `req_len` readable bytes (or null -> error).
/// - `resp_ptr` must be valid for `resp_cap` writable bytes (or null -> error).
/// - For `OP_SVC_DMA_STRESS`, `DmaStressRequest.buf_offset` must point to a
///   valid writable buffer of at least `desc.len` bytes in the active 32-bit
///   data segment.
#[unsafe(link_section = ".text.entry")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_svc_entry(
    req_ptr: *const u8,
    req_len: u32,
    resp_ptr: *mut u8,
    resp_cap: u32,
) -> u32 {
    if req_ptr.is_null() || resp_ptr.is_null() {
        return StatusCode::InvalidPointer.as_u32();
    }
    let req_bytes = unsafe { slice::from_raw_parts(req_ptr, req_len as usize) };
    let resp_bytes = unsafe { slice::from_raw_parts_mut(resp_ptr, resp_cap as usize) };

    // Extract optional DMA bounce-buffer slice when opcode is OP_SVC_DMA_STRESS.
    let mut dma_slice: Option<&mut [u8]> = None;
    let mut dma_locked: Option<(u32, u32)> = None;
    if req_bytes.len() >= RequestHeader::SIZE + core::mem::size_of::<DmaStressRequest>() {
        let hdr = read_request_header(req_bytes);
        if hdr.magic == ABI_MAGIC && hdr.opcode == OP_SVC_DMA_STRESS {
            let dreq = read_dma_stress_req(&req_bytes[RequestHeader::SIZE..]);
            if dreq.buf_offset != 0 && dreq.desc.len > 0 && dreq.desc.len <= dreq.desc.capacity {
                // Resident hosting only: before any byte is touched, ask
                // Jemm's VDMAD to prove the descriptor's physical story
                // (page-table contiguity, no 64 KiB crossing, and that the
                // physical base really is the claimed one) instead of
                // trusting the caller's numbers. A mismatch means the
                // buffer is not the low-memory identity-mapped block the
                // resident path requires, so the request is refused here.
                if platform_vdmad_enabled() && dreq.desc.phys_addr != 0 {
                    match unsafe { vdmad_lock_dma_region(dreq.desc.phys_addr, dreq.desc.capacity) } {
                        Some(phys) if phys == dreq.desc.phys_addr => {
                            dma_locked = Some((dreq.desc.phys_addr, dreq.desc.capacity));
                        }
                        _ => return StatusCode::BounceRangeInvalid.as_u32(),
                    }
                }
                let ptr = dreq.buf_offset as usize as *mut u8;
                dma_slice = Some(unsafe { slice::from_raw_parts_mut(ptr, dreq.desc.len as usize) });
            }
        }
    }

    let state = unsafe { &mut *RESIDENT_SVC.0.get() };
    let status = state.dispatch(req_bytes, resp_bytes, dma_slice).as_u32();
    if let Some((lin, len)) = dma_locked {
        unsafe { vdmad_unlock_dma_region(lin, len) };
    }
    status
}

/// Direct ISR-safe C-ABI entry point for shared level-triggered PCI IRQ 11.
///
/// Never invokes DOS `INT 21h`; bounded O(1); returns `IrqDisposition as u32`.
///
/// # Safety
/// Must be called with interrupts disabled (`IF=0`, hardware ISR or `CLI`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_irq11_isr(
    device_status: u32,
    irq_line: u8,
    pit_tick: u32,
) -> u32 {
    let state = unsafe { &mut *RESIDENT_SVC.0.get() };
    state
        .irq11
        .handle_shared_irq(device_status, irq_line, pit_tick)
        .as_u32()
}

/// Reset resident service state (called on module init / test harness start).
///
/// # Safety
/// Must be called with interrupts disabled (`IF=0`).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_svc_reset() -> u32 {
    let state = unsafe { &mut *RESIDENT_SVC.0.get() };
    state.reset();
    net_reset();
    cardbus::cb_reset();
    ath5k::ath5k_reset();
    ohci::ohci_reset();
    StatusCode::Ok.as_u32()
}

// =============================================================================
// Host unit tests (`rustc --test`)
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_req_header(opcode: u16, payload_len: usize, seq: u32) -> [u8; RequestHeader::SIZE] {
        let mut buf = [0u8; RequestHeader::SIZE];
        write_u32_le(&mut buf, 0, ABI_MAGIC);
        write_u16_le(&mut buf, 4, AbiVersion::CURRENT.major);
        write_u16_le(&mut buf, 6, AbiVersion::CURRENT.minor);
        write_u16_le(&mut buf, 8, opcode);
        write_u16_le(&mut buf, 10, 0);
        write_u32_le(&mut buf, 12, (RequestHeader::SIZE + payload_len) as u32);
        write_u32_le(&mut buf, 16, 4096);
        write_u32_le(&mut buf, 20, seq);
        buf
    }

    #[test]
    fn crc32_matches_ieee_standard_vector() {
        assert_eq!(crc32_ieee(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32_ieee(b""), 0x0000_0000);
    }

    #[test]
    fn shared_irq11_queue_handles_chaining_overflow_and_drain() {
        let mut q = Irq11Queue::new();

        // Non-owned IRQ must return ChainOnly and not enqueue.
        assert_eq!(
            q.handle_shared_irq(0x0000_0000, PCI_SHARED_IRQ_LINE, 10),
            IrqDisposition::ChainOnly
        );
        assert_eq!(q.len(), 0);
        assert_eq!(q.telemetry.chained_events, 1);

        // Fill queue to capacity (32) + 5 overflows.
        for i in 0..(IRQ11_QUEUE_CAP as u32 + 5) {
            assert_eq!(
                q.handle_shared_irq(DEVICE_IRQ_PENDING_MASK, PCI_SHARED_IRQ_LINE, 100 + i),
                IrqDisposition::HandledAndChain
            );
        }
        assert_eq!(q.len(), IRQ11_QUEUE_CAP);
        assert_eq!(q.telemetry.enqueued, IRQ11_QUEUE_CAP as u32);
        assert_eq!(q.telemetry.overflows, 5);
        assert_eq!(q.telemetry.max_depth, IRQ11_QUEUE_CAP as u32);

        // Bounded drain: 10 first, then remaining 22.
        assert_eq!(q.drain_bounded(10), 10);
        assert_eq!(q.len(), IRQ11_QUEUE_CAP - 10);
        assert_eq!(q.drain_bounded(64), (IRQ11_QUEUE_CAP - 10) as u32);
        assert!(q.is_empty());
        assert_eq!(q.telemetry.drained, IRQ11_QUEUE_CAP as u32);
    }

    #[test]
    fn dma_bounce_validation_and_stress_integrity() {
        let good = BounceBufferDesc {
            phys_addr: 0x0003_1000,
            capacity: 4096,
            len: 4096,
            align: 4096,
            flags: BOUNCE_FLAG_BELOW_1M | BOUNCE_FLAG_NO_64K_CROSS,
        };
        assert_eq!(validate_dma_bounce(&good), Ok(()));

        // Crossing 64 KiB boundary rejected even if flag omitted (mandatory for DMA).
        let cross = BounceBufferDesc {
            phys_addr: 0x0002_FC00,
            capacity: 2048,
            len: 2048,
            align: 1024,
            flags: BOUNCE_FLAG_BELOW_1M,
        };
        assert_eq!(validate_dma_bounce(&cross), Err(StatusCode::BounceRangeInvalid));

        // Above 16 MiB rejected.
        let above_16m = BounceBufferDesc {
            phys_addr: 0x0100_0000,
            capacity: 4096,
            len: 4096,
            align: 4096,
            flags: BOUNCE_FLAG_NO_64K_CROSS,
        };
        assert_eq!(
            validate_dma_bounce(&above_16m),
            Err(StatusCode::BounceRangeInvalid)
        );

        let mut buf = [0u8; 4096];
        let res = run_dma_stress(&good, &mut buf).expect("DMA stress should pass");
        assert_eq!(res.integrity_ok, 1);
        assert_eq!(res.below_16m, 1);
        assert_eq!(res.below_1m, 1);
        assert_eq!(res.no_64k_cross, 1);
        assert_eq!(res.neg_tests_passed, 4);
        assert_eq!(res.neg_tests_total, 4);
        assert_eq!(res.passes_completed, 3);
        assert_ne!(res.final_crc32, 0);
    }

    #[test]
    fn full_service_dispatch_coexistence_and_report_markers() {
        let mut svc = ResidentServiceState::new();
        svc.reset();

        let mut resp = [0u8; 2048];

        // 1. Ping
        let mut ping_req = [0u8; RequestHeader::SIZE + 4];
        ping_req[..RequestHeader::SIZE].copy_from_slice(&make_req_header(OP_SVC_PING, 4, 1));
        ping_req[RequestHeader::SIZE] = HOST_MODE_PM32_TRAMPOLINE;
        ping_req[RequestHeader::SIZE + 1] = SERIAL_LIVE_UART;
        write_u16_le(&mut ping_req, RequestHeader::SIZE + 2, 0x03F8);
        assert_eq!(svc.dispatch(&ping_req, &mut resp, None), StatusCode::Ok);

        // 2. IRQ 11 stress
        let mut irq_req = [0u8; RequestHeader::SIZE + 24];
        irq_req[..RequestHeader::SIZE]
            .copy_from_slice(&make_req_header(OP_SVC_IRQ11_STRESS, 24, 2));
        write_u32_le(&mut irq_req, RequestHeader::SIZE + 0, 12);
        write_u32_le(&mut irq_req, RequestHeader::SIZE + 4, 6);
        write_u32_le(&mut irq_req, RequestHeader::SIZE + 8, 40);
        write_u32_le(&mut irq_req, RequestHeader::SIZE + 12, 16);
        write_u32_le(&mut irq_req, RequestHeader::SIZE + 16, 3);
        write_u32_le(&mut irq_req, RequestHeader::SIZE + 20, 64);
        assert_eq!(svc.dispatch(&irq_req, &mut resp, None), StatusCode::Ok);
        assert_eq!(svc.irq11.telemetry.overflows, 8);
        assert!(svc.irq11.is_empty());

        // 3. DMA stress
        let mut dma_buf = [0u8; 4096];
        let mut dma_req = [0u8; RequestHeader::SIZE + 24];
        dma_req[..RequestHeader::SIZE].copy_from_slice(&make_req_header(OP_SVC_DMA_STRESS, 24, 3));
        write_u32_le(&mut dma_req, RequestHeader::SIZE + 0, 0x0004_1000);
        write_u32_le(&mut dma_req, RequestHeader::SIZE + 4, 4096);
        write_u32_le(&mut dma_req, RequestHeader::SIZE + 8, 4096);
        write_u32_le(&mut dma_req, RequestHeader::SIZE + 12, 4096);
        write_u32_le(
            &mut dma_req,
            RequestHeader::SIZE + 16,
            BOUNCE_FLAG_BELOW_1M | BOUNCE_FLAG_NO_64K_CROSS,
        );
        write_u32_le(&mut dma_req, RequestHeader::SIZE + 20, 0x9000);
        assert_eq!(
            svc.dispatch(&dma_req, &mut resp, Some(&mut dma_buf)),
            StatusCode::Ok
        );

        // 4. Coexistence heartbeats (RM + PM32)
        for (seq, mode) in [(4u32, CALLER_MODE_REAL), (5u32, CALLER_MODE_PM32)] {
            let mut hb_req = [0u8; RequestHeader::SIZE + 12];
            hb_req[..RequestHeader::SIZE]
                .copy_from_slice(&make_req_header(OP_SVC_COEXIST_HEARTBEAT, 12, seq));
            hb_req[RequestHeader::SIZE] = mode;
            write_u16_le(&mut hb_req, RequestHeader::SIZE + 2, seq as u16);
            write_u32_le(&mut hb_req, RequestHeader::SIZE + 4, 200 + seq);
            write_u32_le(&mut hb_req, RequestHeader::SIZE + 8, 8);
            assert_eq!(svc.dispatch(&hb_req, &mut resp, None), StatusCode::Ok);
        }

        // 5. Format report and verify all required Gate 1 markers
        let fmt_req = make_req_header(OP_SVC_FORMAT_REPORT, 0, 6);
        assert_eq!(svc.dispatch(&fmt_req, &mut resp, None), StatusCode::Ok);
        let total_len = read_u32_le(&resp, 12) as usize;
        let text = core::str::from_utf8(&resp[ResponseHeader::SIZE..total_len])
            .expect("UTF-8 report");

        assert!(text.contains("=== FERRIC-DOS GATE1 BEGIN ==="));
        assert!(text.contains("GATE1:SVC "));
        assert!(text.contains("GATE1:IRQ11 "));
        assert!(text.contains("GATE1:DMA "));
        assert!(text.contains("GATE1:COEXIST "));
        assert!(text.contains("=== FERRIC-DOS GATE1 END ==="));
        assert!(!text.contains("status=FAIL"));
    }

    #[test]
    fn wifi_packet_driver_activation_preserves_regs_and_rcv_mode() {
        net_reset();
        let wifi_mac = [0x00, 0x13, 0x46, 0xD7, 0x84, 0x8A];
        net_activate_wifi(wifi_mac);

        // Register a wildcard Ethernet receiver via Crynwr access_type (AH=0x02).
        let mut c = [0u32; 19];
        c[CR_EAX] = (0x02 << 8) | 0x01; // AH=2 access_type, AL=1 Ethernet class
        c[CR_ECX] = 0; // typelen = 0 (all EtherTypes)
        c[CR_EDX] = 0; // interface number 0
        c[CR_ES] = 0x1234;
        c[CR_EDI] = 0x0056;
        unsafe { gate1_net_api(c.as_mut_ptr()) };
        assert_eq!(c[CR_EFLAGS] & 1, 0);
        let handle = c[CR_EAX] & 0xFFFF;
        assert_eq!(handle, 1);

        // set_rcv_mode (AH=0x14) to mode 2 (unicast-only) and read back via get_rcv_mode (AH=0x15).
        c = [0u32; 19];
        c[CR_EAX] = 0x14 << 8;
        c[CR_EBX] = handle;
        c[CR_ECX] = 2;
        unsafe { gate1_net_api(c.as_mut_ptr()) };
        assert_eq!(c[CR_EFLAGS] & 1, 0);

        c = [0u32; 19];
        c[CR_EAX] = 0x15 << 8;
        c[CR_EBX] = handle;
        unsafe { gate1_net_api(c.as_mut_ptr()) };
        assert_eq!(c[CR_EFLAGS] & 1, 0);
        assert_eq!(c[CR_EAX] & 0xFFFF, 2);

        // Deactivating Wi-Fi and re-activating preserves the client registration and mode.
        net_deactivate_wifi();
        assert!(!net_state().wifi_active);
        assert!(net_state().regs[0].active);
        net_activate_wifi(wifi_mac);
        assert!(net_state().wifi_active);
        assert_eq!(net_state().mac, wifi_mac);
        assert!(net_state().regs[0].active);
        assert_eq!(net_state().rcv_mode, 2);
        net_reset();
    }

    static MOUSE_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn reset_mouse_for_test(present: bool) {
        let st = mouse_state();
        *st = MouseState::new();
        st.present = present;
    }

    fn int33(c: &mut [u32; 19]) -> u32 {
        unsafe { gate1_mouse_api(c.as_mut_ptr()) }
    }

    #[test]
    fn int33_ax0014_swaps_interrupt_subroutines_and_returns_es_dx() {
        let _guard = MOUSE_TEST_LOCK.lock().unwrap();
        reset_mouse_for_test(true);

        // Install initial handler via AX=000Ch.
        let mut c = [0u32; 19];
        c[CR_EAX] = 0x000C;
        c[CR_ECX] = 0x001F;
        c[CR_ES] = 0x1234;
        c[CR_EDX] = 0x0100;
        assert_eq!(int33(&mut c), 1);

        // Swap via AX=0014h -> must return previous CX, ES, DX and install new ones.
        c = [0u32; 19];
        c[CR_EAX] = 0x0014;
        c[CR_ECX] = 0x007F;
        c[CR_ES] = 0xABCD;
        c[CR_EDX] = 0x0456;
        assert_eq!(int33(&mut c), 1);
        assert_eq!(c[CR_ECX] & 0xFFFF, 0x001F);
        assert_eq!(c[CR_ES] & 0xFFFF, 0x1234);
        assert_eq!(c[CR_EDX] & 0xFFFF, 0x0100);

        // Swap back to 0 -> must return the second handler.
        c = [0u32; 19];
        c[CR_EAX] = 0x0014;
        c[CR_ECX] = 0;
        c[CR_ES] = 0;
        c[CR_EDX] = 0;
        assert_eq!(int33(&mut c), 1);
        assert_eq!(c[CR_ECX] & 0xFFFF, 0x007F);
        assert_eq!(c[CR_ES] & 0xFFFF, 0xABCD);
        assert_eq!(c[CR_EDX] & 0xFFFF, 0x0456);
    }

    #[test]
    fn int33_ax0015_returns_driver_state_size() {
        let _guard = MOUSE_TEST_LOCK.lock().unwrap();
        reset_mouse_for_test(true);

        let mut c = [0u32; 19];
        c[CR_EAX] = 0x0015;
        assert_eq!(int33(&mut c), 1);
        assert_eq!(c[CR_EBX], core::mem::size_of::<MouseState>() as u32);
        assert!(c[CR_EBX] > 0 && c[CR_EBX] < 4096);
    }

    #[test]
    fn int33_ax001a_001b_set_get_sensitivity_and_double_speed() {
        let _guard = MOUSE_TEST_LOCK.lock().unwrap();
        reset_mouse_for_test(true);

        // Defaults: 50 / 50 / 50.
        let mut c = [0u32; 19];
        c[CR_EAX] = 0x001B;
        assert_eq!(int33(&mut c), 1);
        assert_eq!(c[CR_EBX] & 0xFFFF, 50);
        assert_eq!(c[CR_ECX] & 0xFFFF, 50);
        assert_eq!(c[CR_EDX] & 0xFFFF, 50);

        // Set sensitivity via AX=001Ah (with out-of-range value clamped to 100).
        c = [0u32; 19];
        c[CR_EAX] = 0x001A;
        c[CR_EBX] = 75;
        c[CR_ECX] = 150; // clamped to 100
        c[CR_EDX] = 40;
        assert_eq!(int33(&mut c), 1);

        c = [0u32; 19];
        c[CR_EAX] = 0x001B;
        assert_eq!(int33(&mut c), 1);
        assert_eq!(c[CR_EBX] & 0xFFFF, 75);
        assert_eq!(c[CR_ECX] & 0xFFFF, 100);
        assert_eq!(c[CR_EDX] & 0xFFFF, 40);

        // AX=0013h sets double-speed threshold directly.
        c = [0u32; 19];
        c[CR_EAX] = 0x0013;
        c[CR_EDX] = 64;
        assert_eq!(int33(&mut c), 1);

        c = [0u32; 19];
        c[CR_EAX] = 0x001B;
        assert_eq!(int33(&mut c), 1);
        assert_eq!(c[CR_EDX] & 0xFFFF, 64);
    }

    #[test]
    fn int33_ax0024_returns_version_ps2_type_and_irq() {
        let _guard = MOUSE_TEST_LOCK.lock().unwrap();
        reset_mouse_for_test(true);

        let mut c = [0u32; 19];
        c[CR_EAX] = 0x0024;
        // Pass bogus ES:DX to confirm standard AX=0024h never dereferences ES:DX.
        c[CR_ES] = 0xDEAD;
        c[CR_EDX] = 0xBEEF;
        assert_eq!(int33(&mut c), 1);
        assert_eq!(c[CR_EAX] & 0xFFFF, 0x0024);
        assert_eq!(c[CR_EBX] & 0xFFFF, 0x0820); // v8.20
        assert_eq!((c[CR_ECX] >> 8) & 0xFF, 4); // PS/2 mouse type
        assert_eq!(c[CR_ECX] & 0xFF, 0);        // IRQ 0 = PS/2 per MS-MOUSE spec

        // When absent (with empty init log), returns AX=FFFFh without touching ES:DX.
        reset_mouse_for_test(false);
        c = [0u32; 19];
        c[CR_EAX] = 0x0024;
        assert_eq!(int33(&mut c), 1);
        assert_eq!(c[CR_EAX] & 0xFFFF, 0xFFFF);
    }

    #[test]
    fn int33_ax001f_0020_disable_and_enable_driver() {
        let _guard = MOUSE_TEST_LOCK.lock().unwrap();
        reset_mouse_for_test(true);

        // Register callback and disable driver via AX=001Fh.
        let mut c = [0u32; 19];
        c[CR_EAX] = 0x000C;
        c[CR_ECX] = 0x007F;
        c[CR_ES] = 0x2000;
        c[CR_EDX] = 0x0010;
        assert_eq!(int33(&mut c), 1);

        c = [0u32; 19];
        c[CR_EAX] = 0x001F;
        assert_eq!(int33(&mut c), 1);
        assert_eq!(c[CR_EAX] & 0xFFFF, 0x001F);
        assert_eq!(c[CR_ES] & 0xFFFF, 0x0040);
        assert_eq!(c[CR_EBX] & 0xFFFF, 0x00F0);

        // While disabled, AX=0026h reports disabled=1 and motion/callbacks are suppressed.
        c = [0u32; 19];
        c[CR_EAX] = 0x0026;
        assert_eq!(int33(&mut c), 1);
        assert_eq!(c[CR_EBX] & 0xFFFF, 1);

        mouse_feed_usb(20, 10, 1);
        let mut cb_out = [0u32; 7];
        assert_eq!(unsafe { gate1_mouse_cb_info(cb_out.as_mut_ptr()) }, 0);

        // Re-enable via AX=0020h.
        c = [0u32; 19];
        c[CR_EAX] = 0x0020;
        assert_eq!(int33(&mut c), 1);
        assert_eq!(c[CR_EAX] & 0xFFFF, 0x0020);

        mouse_feed_usb(20, 10, 1);
        assert_eq!(unsafe { gate1_mouse_cb_info(cb_out.as_mut_ptr()) }, 1);
    }

    #[test]
    fn int33_graphics_mode_ranges_800x600_1024x768_1280x1024() {
        let _guard = MOUSE_TEST_LOCK.lock().unwrap();
        reset_mouse_for_test(true);

        for (w, h) in [(800i32, 600i32), (1024, 768), (1280, 1024)] {
            let max_x = w - 1;
            let max_y = h - 1;

            // Set horizontal range 0..max_x (AX=0007h) and vertical range 0..max_y (AX=0008h).
            let mut c = [0u32; 19];
            c[CR_EAX] = 0x0007;
            c[CR_ECX] = 0;
            c[CR_EDX] = max_x as u32;
            assert_eq!(int33(&mut c), 1);

            c = [0u32; 19];
            c[CR_EAX] = 0x0008;
            c[CR_ECX] = 0;
            c[CR_EDX] = max_y as u32;
            assert_eq!(int33(&mut c), 1);

            // Set 1:1 mickey/pixel ratio (AX=000Fh) and graphics cursor block (AX=0009h).
            c = [0u32; 19];
            c[CR_EAX] = 0x000F;
            c[CR_ECX] = 8;
            c[CR_EDX] = 8;
            assert_eq!(int33(&mut c), 1);

            c = [0u32; 19];
            c[CR_EAX] = 0x0009;
            c[CR_EBX] = 4;
            c[CR_ECX] = 4;
            assert_eq!(int33(&mut c), 1);

            // Position near bottom-right corner beyond 640x200 via AX=0004h.
            let target_x = max_x - 10;
            let target_y = max_y - 10;
            c = [0u32; 19];
            c[CR_EAX] = 0x0004;
            c[CR_ECX] = target_x as u32;
            c[CR_EDX] = target_y as u32;
            assert_eq!(int33(&mut c), 1);

            // Query position via AX=0003h -> must NOT clamp to 639x199.
            c = [0u32; 19];
            c[CR_EAX] = 0x0003;
            assert_eq!(int33(&mut c), 1);
            assert_eq!(c[CR_ECX] & 0xFFFF, target_x as u32);
            assert_eq!(c[CR_EDX] & 0xFFFF, target_y as u32);

            // Move past edge via USB HID report -> clamps cleanly to (max_x, max_y).
            mouse_feed_usb(50, 50, 0);
            c = [0u32; 19];
            c[CR_EAX] = 0x0003;
            assert_eq!(int33(&mut c), 1);
            assert_eq!(c[CR_ECX] & 0xFFFF, max_x as u32);
            assert_eq!(c[CR_EDX] & 0xFFFF, max_y as u32);

            // Verify AX=0026h and AX=0031h report the active graphics bounds.
            c = [0u32; 19];
            c[CR_EAX] = 0x0026;
            assert_eq!(int33(&mut c), 1);
            assert_eq!(c[CR_EBX] & 0xFFFF, 0);
            assert_eq!(c[CR_ECX] & 0xFFFF, max_x as u32);
            assert_eq!(c[CR_EDX] & 0xFFFF, max_y as u32);

            c = [0u32; 19];
            c[CR_EAX] = 0x0031;
            assert_eq!(int33(&mut c), 1);
            assert_eq!(c[CR_EAX] & 0xFFFF, 0);
            assert_eq!(c[CR_EBX] & 0xFFFF, 0);
            assert_eq!(c[CR_ECX] & 0xFFFF, max_x as u32);
            assert_eq!(c[CR_EDX] & 0xFFFF, max_y as u32);
        }
    }
}

// =====================================================================
// Gate 2 Step 2.2 — PS/2 mouse + DOS INT 33h, resident in the JLM.
// =====================================================================
//
// Why the mouse lives in the resident Rust service rather than in a
// classic real-mode TSR: the platform's execution model is one
// protected-mode service beside the foreground DOS program (the spec's
// USB paragraph states the architecture for all resident drivers, not
// just USB). The JLM stub hooks two V86 chains to reach it:
//
//   - INT 74h (slave-PIC IRQ 12) is the packet path. The stub reads the
//     byte from the 8042 only when the status port says auxiliary data
//     is waiting, and hands it to `gate1_mouse_feed`. The hook then
//     CHAINS (returns CF set) even though the byte is consumed:
//     claiming left the slave PIC's in-service bit set under Jemm and
//     the stream stalled after one byte (proven in the 2.2 hardware
//     rounds; the stub's own comment at the hook tells the story).
//     The handlers behind us are EOI stubs that never read the data
//     port, so chaining after consuming is both safe and required.
//   - INT 33h is the API path. The stub passes the client register
//     block to `gate1_mouse_api`, which implements the functions DOS
//     programs actually call. Unknown functions return "not handled"
//     and the stub chains, so a second mouse driver could still sit
//     behind us.
//
// Event callbacks (function 000Ch) are dispatched from the INT 33h
// hook, not from interrupt context: packet events accumulate in
// `pending_events`, and the next INT 33h entry fires the registered
// callback through Jemm's nested execution (Begin_Nest_Exec /
// Simulate_Far_Call / Resume_Exec / End_Nest_Exec) before servicing
// the call itself. Polling clients — nearly every DOS program — see
// callback latency of one INT 33h call, and nested execution never
// has to be re-entered from inside an interrupt reflection.
//
// The software cursor is state-only in this step: show/hide nesting,
// graphics/text cursor shapes are accepted and recorded, but nothing
// is drawn. Rendering into text and graphics modes is its own piece
// of work; the motion/button/limit/callback contract below is what
// games and GUIs actually depend on to function.

/// Client_Reg_Struc dword indices (vendor/ref/jemm/Include/JLM.INC).
const CR_EBX: usize = 4;
const CR_EDX: usize = 5;
const CR_ECX: usize = 6;
const CR_EAX: usize = 7;
const CR_ES: usize = 15;

/// INT 33h event-mask bits (also the callback's AX on entry).
const EV_MOVE: u32 = 0x01;
const EV_PRESS_BASE: u32 = 0x02; // press bit for button i:   EV_PRESS_BASE   << (2*i)
const EV_RELEASE_BASE: u32 = 0x04; // release bit for button i: EV_RELEASE_BASE << (2*i)

#[derive(Clone, Copy)]
pub struct MouseState {
    present: bool,
    packet: [u8; 3],
    packet_idx: u8,
    #[allow(dead_code)]
    last_byte_tick: u32,
    dropped_bytes: u32,
    packets: u32,
    x: i32,
    y: i32,
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
    buttons: u8,
    mickey_x: i32,
    mickey_y: i32,
    frac_x: i32,
    frac_y: i32,
    cb_mickey_x: i32,
    cb_mickey_y: i32,
    press_count: [u16; 3],
    press_x: [i32; 3],
    press_y: [i32; 3],
    release_count: [u16; 3],
    release_x: [i32; 3],
    release_y: [i32; 3],
    cb_mask: u16,
    cb_seg: u16,
    cb_off: u16,
    pending_events: u32,
    fed_total: u32,
    byte_log: [u8; 48],
    byte_log_len: u8,
    init_stage: u8,
    init_last: u16,
    init_cfg: u16,
    init_strays: u8,
    init_attempts: u8,
    init_log: [u16; 32],
    init_log_len: u8,
    cursor_flag: i16,
    cursor_drawn: bool,
    #[allow(dead_code)]
    cursor_cell_off: usize,
    #[allow(dead_code)]
    cursor_saved: u16,
    cursor_and_mask: u16,
    cursor_xor_mask: u16,
    ratio_x: u16,
    ratio_y: u16,
    sens_x: u16,
    sens_y: u16,
    double_speed_threshold: u16,
    disabled: bool,
    gfx_hot_x: i16,
    gfx_hot_y: i16,
    display_page: u16,
}

impl MouseState {
    const fn new() -> Self {
        Self {
            present: false,
            packet: [0; 3],
            packet_idx: 0,
            last_byte_tick: 0,
            dropped_bytes: 0,
            packets: 0,
            x: 320,
            y: 100,
            min_x: 0,
            max_x: 639,
            min_y: 0,
            max_y: 199,
            buttons: 0,
            mickey_x: 0,
            mickey_y: 0,
            frac_x: 0,
            frac_y: 0,
            cb_mickey_x: 0,
            cb_mickey_y: 0,
            press_count: [0; 3],
            press_x: [0; 3],
            press_y: [0; 3],
            release_count: [0; 3],
            release_x: [0; 3],
            release_y: [0; 3],
            cb_mask: 0,
            cb_seg: 0,
            cb_off: 0,
            pending_events: 0,
            fed_total: 0,
            byte_log: [0; 48],
            byte_log_len: 0,
            init_stage: 0,
            init_last: 0xFFFF,
            init_cfg: 0xFFFF,
            init_strays: 0,
            init_attempts: 0,
            init_log: [0; 32],
            init_log_len: 0,
            cursor_flag: -1,
            cursor_drawn: false,
            cursor_cell_off: 0,
            cursor_saved: 0,
            cursor_and_mask: 0xFFFF,
            cursor_xor_mask: 0x7700,
            ratio_x: 8,
            ratio_y: 16,
            sens_x: 50,
            sens_y: 50,
            double_speed_threshold: 50,
            disabled: false,
            gfx_hot_x: 0,
            gfx_hot_y: 0,
            display_page: 0,
        }
    }

    /// INT 33h function 0000h semantics: motion state, counters,
    /// callback and limits return to defaults; hardware presence and
    /// the init diagnostics survive (the device does not re-init).
    /// The packet assembler restarts at byte 0: the packet bytes are
    /// zeroed with the rest of the state, so resuming at the old
    /// index would assemble one phantom packet out of zeros — the
    /// bit-3 resync only guards index 0.
    fn reset_runtime(&mut self) {
        let present = self.present;
        let fed_total = self.fed_total;
        let byte_log = self.byte_log;
        let byte_log_len = self.byte_log_len;
        let init_stage = self.init_stage;
        let init_last = self.init_last;
        let init_cfg = self.init_cfg;
        let init_strays = self.init_strays;
        let init_attempts = self.init_attempts;
        let init_log = self.init_log;
        let init_log_len = self.init_log_len;
        *self = Self::new();
        self.present = present;
        self.fed_total = fed_total;
        self.byte_log = byte_log;
        self.byte_log_len = byte_log_len;
        self.init_stage = init_stage;
        self.init_last = init_last;
        self.init_cfg = init_cfg;
        self.init_strays = init_strays;
        self.init_attempts = init_attempts;
        self.init_log = init_log;
        self.init_log_len = init_log_len;
    }
}

static MOUSE_STATE: SingleCoreCellMouse = SingleCoreCellMouse(UnsafeCell::new(MouseState::new()));

struct SingleCoreCellMouse(UnsafeCell<MouseState>);
// SAFETY: single-core DOS with CLI/STI discipline, same as SingleCoreCell.
unsafe impl Sync for SingleCoreCellMouse {}

fn mouse_state() -> &'static mut MouseState {
    unsafe { &mut *MOUSE_STATE.0.get() }
}

fn clamp_i32(v: i32, lo: i32, hi: i32) -> i32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// Restore the VGA text cell under the software mouse cursor if drawn.
#[cfg(target_arch = "x86")]
fn mouse_cursor_erase(st: &mut MouseState) {
    if !st.cursor_drawn {
        return;
    }
    unsafe {
        let mode = core::ptr::read_volatile(0x0449 as *const u8);
        let vram = match mode {
            0x02 | 0x03 => 0x000B_8000usize,
            0x07 => 0x000B_0000usize,
            _ => {
                st.cursor_drawn = false;
                return;
            }
        };
        core::ptr::write_volatile((vram + st.cursor_cell_off) as *mut u16, st.cursor_saved);
    }
    st.cursor_drawn = false;
}

#[cfg(not(target_arch = "x86"))]
fn mouse_cursor_erase(st: &mut MouseState) {
    st.cursor_drawn = false;
}

/// Draw the standard INT 33h VGA text-mode software cursor `(orig & and_mask) ^ xor_mask`
/// when `cursor_flag == 0` and the active BIOS video mode is text (02h/03h/07h).
#[cfg(target_arch = "x86")]
fn mouse_cursor_draw(st: &mut MouseState) {
    if st.cursor_flag < 0 || st.disabled {
        return;
    }
    unsafe {
        let mode = core::ptr::read_volatile(0x0449 as *const u8);
        let vram = match mode {
            0x02 | 0x03 => 0x000B_8000usize,
            0x07 => 0x000B_0000usize,
            _ => return,
        };
        let cols = (core::ptr::read_volatile(0x044A as *const u16) as usize).clamp(40, 132);
        let rows_m1 = core::ptr::read_volatile(0x0484 as *const u8) as usize;
        let rows = if rows_m1 >= 24 && rows_m1 <= 60 { rows_m1 + 1 } else { 25 };
        let page_off = (core::ptr::read_volatile(0x044E as *const u16) as usize) & 0x3FFE;
        let col = ((st.x.max(0) as usize) / 8).min(cols - 1);
        let row = ((st.y.max(0) as usize) / 8).min(rows - 1);
        let off = page_off + (row * cols + col) * 2;
        if st.cursor_drawn && st.cursor_cell_off == off {
            return;
        }
        if st.cursor_drawn {
            core::ptr::write_volatile((vram + st.cursor_cell_off) as *mut u16, st.cursor_saved);
        }
        let orig = core::ptr::read_volatile((vram + off) as *const u16);
        st.cursor_saved = orig;
        st.cursor_cell_off = off;
        st.cursor_drawn = true;
        let cur = (orig & st.cursor_and_mask) ^ st.cursor_xor_mask;
        core::ptr::write_volatile((vram + off) as *mut u16, cur);
    }
}

#[cfg(not(target_arch = "x86"))]
fn mouse_cursor_draw(_st: &mut MouseState) {}

// ---------------------------------------------------------------------
// 8042 keyboard-controller access (PS/2 auxiliary device = the mouse).
// Init runs once at JLM attach, before the INT 74h hook is installed,
// so responses are collected by polling the output buffer; afterwards
// the hook owns the data port.
// ---------------------------------------------------------------------

#[cfg(target_arch = "x86")]
unsafe fn inb(port: u16) -> u8 {
    let v: u8;
    unsafe {
        core::arch::asm!("in al, dx", in("dx") port, out("al") v, options(nomem, nostack));
    }
    v
}

#[cfg(target_arch = "x86")]
unsafe fn outb(port: u16, val: u8) {
    unsafe {
        core::arch::asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack));
    }
}

#[cfg(target_arch = "x86")]
fn kbc_wait_input_empty() -> bool {
    for _ in 0..100_000 {
        if unsafe { inb(0x64) } & 0x02 == 0 {
            return true;
        }
    }
    false
}

#[cfg(target_arch = "x86")]
fn bios_tick() -> u32 {
    // The BIOS tick at 0040:006C is identity-mapped low memory; the
    // JLM stub's IRQ hook reads the same cell. It is the only clock
    // available in this context (no DOS, no rdtsc guarantee on a 386).
    unsafe { core::ptr::read_volatile(0x46C as *const u32) }
}

#[cfg(not(target_arch = "x86"))]
fn bios_tick() -> u32 {
    // Host-test stand-in: the network module's SCB/CB poll budgets
    // call bios_tick without being x86-gated, so host builds need a
    // clock too. A simple counter keeps the polls terminating.
    use core::sync::atomic::{AtomicU32, Ordering};
    static TICK: AtomicU32 = AtomicU32::new(0);
    TICK.fetch_add(1, Ordering::Relaxed)
}

/// Wait for one byte from the controller, budgeted in BIOS ticks
/// (~55 ms each). Tick budgets, not spin counts: a real PS/2 reset
/// self-test takes hundreds of milliseconds, while a spin count that
/// covers it on a 750 MHz part would be pure luck on anything slower.
#[cfg(target_arch = "x86")]
fn kbc_read_data_ticks(max_ticks: u32) -> Option<u8> {
    let start = bios_tick();
    loop {
        if unsafe { inb(0x64) } & 0x01 != 0 {
            return Some(unsafe { inb(0x60) });
        }
        if bios_tick().wrapping_sub(start) > max_ticks {
            return None;
        }
    }
}

/// Discard whatever the output buffer already holds. At JLOAD time
/// a stale byte (a boot-menu keystroke is the classic) can be sitting
/// there; if a command response is read while it lingers, the stale
/// byte is mistaken for the response.
#[cfg(target_arch = "x86")]
fn kbc_drain() {
    for _ in 0..16 {
        if unsafe { inb(0x64) } & 0x01 == 0 {
            break;
        }
        let _ = unsafe { inb(0x60) };
    }
}

#[cfg(not(target_arch = "x86"))]
fn kbc_drain() {}

/// Init-path read: records the byte (or FFFFh on timeout) in the
/// state so function 0023h can report how far bring-up actually got
/// on a machine where the mouse never came up.
fn init_read(st: &mut MouseState, ticks: u32) -> Option<u8> {
    #[cfg(target_arch = "x86")]
    {
        let r = kbc_read_data_ticks(ticks);
        st.init_last = r.map(|b| b as u16).unwrap_or(0xFFFF);
        r
    }
    #[cfg(not(target_arch = "x86"))]
    {
        let _ = ticks;
        st.init_last = 0xFFFF;
        None
    }
}

/// Record one byte the init path read, with its provenance: bit 8
/// set when the status port attributed the byte to the auxiliary
/// device, clear when it was keyboard traffic. The log is the
/// ground truth the 0024h diagnostic serves — after two DynaBook
/// runs failed at stage 5 with last_byte 0x83, "which byte, from
/// which device, in which order" is the question that decides the
/// next fix, and it cannot be answered from a single last byte.
#[cfg(target_arch = "x86")]
fn init_log_push(st: &mut MouseState, val: u16) {
    if (st.init_log_len as usize) < st.init_log.len() {
        st.init_log[st.init_log_len as usize] = val;
        st.init_log_len += 1;
    }
}

/// Init-path read of the next byte from either device, budgeted in
/// BIOS ticks. Returns the byte and whether the status port
/// attributed it to the auxiliary device. Every byte is logged and
/// becomes init_last. Controller responses (the configuration
/// reads) keep using init_read: their provenance bit is the
/// controller's own business and differs between chipsets, while
/// device responses are what must not be confused with keyboard
/// traffic.
fn init_read_any(st: &mut MouseState, ticks: u32) -> Option<(u8, bool)> {
    #[cfg(target_arch = "x86")]
    {
        let start = bios_tick();
        loop {
            let status = unsafe { inb(0x64) };
            if status & 0x01 != 0 {
                let b = unsafe { inb(0x60) };
                st.init_last = b as u16;
                let aux = status & 0x20 != 0;
                init_log_push(st, b as u16 | if aux { 0x100 } else { 0 });
                return Some((b, aux));
            }
            if bios_tick().wrapping_sub(start) > ticks {
                return None;
            }
        }
    }
    #[cfg(not(target_arch = "x86"))]
    {
        let _ = ticks;
        st.init_last = 0xFFFF;
        None
    }
}

/// Wait for one specific device response, skipping and counting
/// whatever else arrives first. A step whose answer is on its way
/// must not fail because a late ID byte, a packet fragment from a
/// half-awake touchpad, or a boot-menu keystroke's break code got
/// there first — the 2026-11-20/21 DynaBook failures were exactly
/// that shape (stage 5 read 0x83, the "2" key's break code, where
/// the self-test's 0xAA belonged). The wanted byte is accepted
/// from either provenance: a keyboard byte that equals it (a
/// left-shift break is 0xAA) is a theoretical collision, and if
/// one ever slips through, the following steps' own expectations
/// fail honestly and the log shows what happened.
fn init_expect(st: &mut MouseState, want: u8, ticks: u32) -> bool {
    let start = bios_tick();
    loop {
        let elapsed = bios_tick().wrapping_sub(start);
        if elapsed >= ticks {
            return false;
        }
        match init_read_any(st, ticks - elapsed) {
            Some((b, _)) if b == want => return true,
            Some(_) => {
                st.init_strays = st.init_strays.saturating_add(1);
            }
            None => return false,
        }
    }
}

#[cfg(target_arch = "x86")]
fn kbc_cmd(cmd: u8) -> bool {
    if !kbc_wait_input_empty() {
        return false;
    }
    unsafe { outb(0x64, cmd) };
    true
}

#[cfg(target_arch = "x86")]
fn kbc_write_data(val: u8) -> bool {
    if !kbc_wait_input_empty() {
        return false;
    }
    unsafe { outb(0x60, val) };
    true
}

#[cfg(not(target_arch = "x86"))]
fn kbc_cmd(_cmd: u8) -> bool {
    false
}

#[cfg(not(target_arch = "x86"))]
fn kbc_write_data(_val: u8) -> bool {
    false
}

/// Send one byte to the mouse and consume its 0xFA acknowledgement,
/// recording the exchange for the 0023h diagnostic.
fn mouse_cmd_rec(st: &mut MouseState, cmd: u8) -> bool {
    #[cfg(target_arch = "x86")]
    {
        if !kbc_cmd(0xD4) || !kbc_write_data(cmd) {
            return false;
        }
        init_expect(st, 0xFA, 18)
    }
    #[cfg(not(target_arch = "x86"))]
    {
        let _ = (st, cmd);
        false
    }
}

/// One PS/2 mouse bring-up attempt: enable the auxiliary port,
/// reset the device, take the defaults (100 samples/s, 4 counts/mm,
/// 1:1 scaling), enable data reporting, and only then unmask
/// IRQ 12 in the controller configuration. Every step records its
/// stage so a failure on real hardware names the step (function
/// 0023h). `original_cfg` captures the first configuration byte
/// read across all attempts, for the caller's restore-on-failure.
fn mouse_init_attempt(st: &mut MouseState, original_cfg: &mut Option<u8>) -> bool {
    st.init_stage = 1; // enable the auxiliary device
    if !kbc_cmd(0xA8) {
        return false;
    }
    // Drain BEFORE the first attributed response (see kbc_drain).
    kbc_drain();
    st.init_stage = 2; // read the controller configuration
    if !kbc_cmd(0x20) {
        return false;
    }
    let Some(cfg) = init_read(st, 18) else {
        return false;
    };
    if original_cfg.is_none() {
        *original_cfg = Some(cfg);
        st.init_cfg = cfg as u16;
    }
    // Auxiliary clock on (bit 5 clear). IRQ 12 (bit 1) stays OFF for
    // the whole command exchange: until the stub installs our chain
    // hook, an IRQ 12 reflection lands in the BIOS default handler,
    // which may read the data port and steal a response byte this
    // init is polling for.
    //
    // The write base is trusted because of the drain above and the
    // readback verify below, NOT because the byte "looks like" a
    // configuration: a sanity gate on the system-flag bit was tried
    // and measured a false positive on QEMU/SeaBIOS, whose perfectly
    // ordinary configuration is 0x41 (system flag clear) — the gate
    // suppressed the IRQ 12 enable and the mouse went silent. Shape
    // checks on other people's firmware state are how drivers break
    // machines they have never met; drain + verify + restore is the
    // protection that does not guess.
    st.init_stage = 3; // write the configuration back
    if !kbc_cmd(0x60) || !kbc_write_data(cfg & !0x20 & !0x02) {
        return false;
    }
    // Verify by reading back: aux clock on, IRQ 12 off, and — the
    // two bits that protect the keyboard — IRQ 1 on and the keyboard
    // clock on. Anything else means this controller does not hold
    // what we wrote; restore the original byte and stop rather than
    // leave a stranger's configuration behind. (The first hardware
    // run wrote back a byte that had been sitting stale in the
    // output buffer and took the keyboard with it; the drain keeps
    // that from recurring, and this verify is the net under it.)
    let verified = kbc_cmd(0x20)
        && match init_read(st, 18) {
            Some(rb) => {
                (rb & 0x20) == 0
                    && (rb & 0x02) == 0
                    && (rb & 0x01) != 0
                    && (rb & 0x10) == 0
            }
            None => false,
        };
    if !verified {
        if kbc_cmd(0x60) {
            let _ = kbc_write_data(cfg);
        }
        st.init_stage = 8; // configuration verify failed, restored
        return false;
    }
    kbc_drain();
    st.init_stage = 4; // device reset, expect the ACK
    if !mouse_cmd_rec(st, 0xFF) {
        return false;
    }
    st.init_stage = 5; // self-test pass (0xAA) + device ID
    if !init_expect(st, 0xAA, 60) {
        return false;
    }
    let _ = init_read_any(st, 18);
    st.init_stage = 6; // set defaults
    if !mouse_cmd_rec(st, 0xF6) {
        return false;
    }
    st.init_stage = 7; // enable data reporting
    if !mouse_cmd_rec(st, 0xF4) {
        return false;
    }
    // Only now — reporting on, hooks installed by the stub immediately
    // after this returns — enable IRQ 12 in the configuration.
    if kbc_cmd(0x20) {
        if let Some(c2) = init_read(st, 18) {
            if kbc_cmd(0x60) {
                let _ = kbc_write_data(c2 | 0x02);
            }
        }
    }
    st.reset_runtime();
    st.present = true;
    st.init_stage = 9;
    true
}

/// PS/2 mouse bring-up: up to three attempts at the exchange
/// above, because a single stray byte in the wrong slot failed
/// whole boots on the DynaBook while the same code passed QEMU
/// every time — the hardware gets transient states the emulator
/// never produces, and one retry from a drained buffer is cheap.
/// On total failure the controller configuration is restored to
/// the byte found at entry: the keyboard's survival must never
/// depend on how far mouse bring-up got. Returns 1 when a mouse
/// answered and reporting is on.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_mouse_init() -> u32 {
    let st = mouse_state();
    st.present = false;
    st.init_last = 0xFFFF;
    st.init_strays = 0;
    st.init_attempts = 0;
    st.init_log_len = 0;

    let mut original_cfg: Option<u8> = None;
    for attempt in 1..=3u8 {
        st.init_attempts = attempt;
        if mouse_init_attempt(st, &mut original_cfg) {
            return 1;
        }
        kbc_drain();
    }
    if let Some(cfg) = original_cfg {
        if kbc_cmd(0x60) {
            let _ = kbc_write_data(cfg);
        }
    }
    0
}

/// Convert one report's mickeys to whole pixels, carrying the
/// sub-pixel remainder per axis in the mouse state. The ratio is
/// mickeys per 8 pixels, so a report contributes mickeys * 8 / ratio
/// pixels; truncating that per report throws the fraction away every
/// time, and at the default Y ratio of 16 a 1-mickey move is exactly
/// 0 pixels — slow motion never accumulates into movement at all.
fn mickey_pixels(st: &mut MouseState, dx: i32, dy: i32) -> (i32, i32) {
    let ratio_x = st.ratio_x.max(1) as i32;
    let total_x = st.frac_x + dx * 8;
    let px = total_x / ratio_x;
    st.frac_x = total_x - px * ratio_x;
    let ratio_y = st.ratio_y.max(1) as i32;
    let total_y = st.frac_y + dy * 8;
    let py = total_y / ratio_y;
    st.frac_y = total_y - py * ratio_y;
    (px, py)
}

/// Feed one byte from the 8042 data port (auxiliary data only; the
/// stub checks the status port first). Returns the INT 33h event mask
/// the completed packet produced (0 while a packet is incomplete).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_mouse_feed(byte: u32) -> u32 {
    let st = mouse_state();
    if !st.present {
        return 0;
    }
    let b = byte as u8;
    st.fed_total += 1;
    if (st.byte_log_len as usize) < st.byte_log.len() {
        st.byte_log[st.byte_log_len as usize] = b;
        st.byte_log_len += 1;
    }
    // If a partial 3-byte packet stalled for >110 ms (>2 BDA ticks at 18.2 Hz),
    // resync to byte 0 so lifting/re-touching a touchpad never misaligns the next
    // packet into phantom clicks or cursor teleports.
    #[cfg(target_arch = "x86")]
    {
        let now_tick = unsafe { core::ptr::read_volatile(0x046C as *const u32) };
        if st.packet_idx != 0 && now_tick.wrapping_sub(st.last_byte_tick) > 2 {
            st.dropped_bytes += st.packet_idx as u32;
            st.packet_idx = 0;
        }
        st.last_byte_tick = now_tick;
    }
    // Byte 0 of a packet always has bit 3 set; anything else while we
    // expect a first byte is a dropped/resynced byte (hot-plug noise,
    // a byte the keyboard path left behind, a lost IRQ).
    if st.packet_idx == 0 && (b & 0x08) == 0 {
        st.dropped_bytes += 1;
        return 0;
    }
    st.packet[st.packet_idx as usize] = b;
    st.packet_idx += 1;
    if st.packet_idx < 3 {
        return 0;
    }
    st.packet_idx = 0;
    if st.disabled {
        return 0;
    }
    let b0 = st.packet[0];
    let mut dx = st.packet[1] as i32;
    if b0 & 0x10 != 0 {
        dx -= 256;
    }
    let mut dy = st.packet[2] as i32;
    if b0 & 0x20 != 0 {
        dy -= 256;
    }
    // Desync guard: if a byte was dropped in the 8042 1-byte buffer (e.g. during
    // a PM32 audio IRQ), a movement byte with bit 3 set can be misread as b0,
    // producing impossible sign/magnitude mismatches (e.g. b0_sign=0 with b1=0xFF
    // -> dx=+255, or a real b0=0x38 landing in b2 -> dy=-200) and phantom button
    // bits (btn=4/5/6/7). Reject such packets and slide the 3-byte window forward
    // to re-lock onto the real b0 byte without teleporting or clicking.
    let overflow = (b0 & 0xC0) != 0;
    if overflow || dx < -96 || dx > 96 || dy < -96 || dy > 96 {
        st.dropped_bytes += 1;
        let p1 = st.packet[1];
        let p2 = st.packet[2];
        if (p1 & 0x08) != 0 && (p1 & 0xC0) == 0 {
            st.packet[0] = p1;
            st.packet[1] = p2;
            st.packet_idx = 2;
        } else if (p2 & 0x08) != 0 && (p2 & 0xC0) == 0 {
            st.packet[0] = p2;
            st.packet_idx = 1;
        } else {
            st.packet_idx = 0;
        }
        return 0;
    }
    st.packets += 1;
    // PS/2 +Y points at the top of the screen; DOS Y grows downward.
    let move_y = -dy;
    st.mickey_x += dx;
    st.mickey_y += move_y;
    st.cb_mickey_x += dx;
    st.cb_mickey_y += move_y;
    // Mickeys -> virtual pixels: the ratio is mickeys per 8 pixels
    // (defaults 8 and 16); the remainder carries between packets.
    let (px, py) = mickey_pixels(st, dx, move_y);
    st.x = clamp_i32(st.x + px, st.min_x, st.max_x);
    st.y = clamp_i32(st.y + py, st.min_y, st.max_y);
    if dx != 0 || dy != 0 {
        mouse_cursor_draw(st);
    }

    let mut ev: u32 = 0;
    if dx != 0 || dy != 0 {
        ev |= EV_MOVE;
    }
    let new_buttons = b0 & 0x07;
    for i in 0..3usize {
        let bit = 1u8 << i;
        let was = st.buttons & bit != 0;
        let now = new_buttons & bit != 0;
        if now && !was {
            st.press_count[i] = st.press_count[i].wrapping_add(1);
            st.press_x[i] = st.x;
            st.press_y[i] = st.y;
            ev |= EV_PRESS_BASE << (2 * i);
        } else if !now && was {
            st.release_count[i] = st.release_count[i].wrapping_add(1);
            st.release_x[i] = st.x;
            st.release_y[i] = st.y;
            ev |= EV_RELEASE_BASE << (2 * i);
        }
    }
    st.buttons = new_buttons;
    st.pending_events |= ev;
    ev
}

/// Mark the resident `INT 33h` mouse driver as present (called as soon as a USB
/// HID boot mouse is enumerated, even before its first movement packet arrives).
pub(crate) fn mouse_set_present() {
    let st = mouse_state();
    if !st.present {
        st.reset_runtime();
        st.present = true;
    }
}

/// Feed one relative movement + button report from a USB HID Boot Mouse
/// directly into the resident `INT 33h` mouse state. USB HID +Y already
/// grows downward, matching DOS screen coordinates.
pub(crate) fn mouse_feed_usb(dx: i32, dy: i32, buttons: u8) {
    let st = mouse_state();
    if !st.present {
        st.reset_runtime();
        st.present = true;
    }
    if st.disabled {
        return;
    }
    st.packets = st.packets.wrapping_add(1);
    st.mickey_x += dx;
    st.mickey_y += dy;
    st.cb_mickey_x += dx;
    st.cb_mickey_y += dy;
    // Same conversion as the PS/2 path, remainder carried per axis.
    let (px, py) = mickey_pixels(st, dx, dy);
    st.x = clamp_i32(st.x + px, st.min_x, st.max_x);
    st.y = clamp_i32(st.y + py, st.min_y, st.max_y);
    if dx != 0 || dy != 0 {
        mouse_cursor_draw(st);
    }
    let mut ev: u32 = 0;
    if dx != 0 || dy != 0 {
        ev |= EV_MOVE;
    }
    let new_buttons = buttons & 0x07;
    for i in 0..3usize {
        let bit = 1u8 << i;
        let was = st.buttons & bit != 0;
        let now = new_buttons & bit != 0;
        if now && !was {
            st.press_count[i] = st.press_count[i].wrapping_add(1);
            st.press_x[i] = st.x;
            st.press_y[i] = st.y;
            ev |= EV_PRESS_BASE << (2 * i);
        } else if !now && was {
            st.release_count[i] = st.release_count[i].wrapping_add(1);
            st.release_x[i] = st.x;
            st.release_y[i] = st.y;
            ev |= EV_RELEASE_BASE << (2 * i);
        }
    }
    st.buttons = new_buttons;
    st.pending_events |= ev;
}

/// If a callback is registered and pending events intersect its mask,
/// fill `out` for the stub's nested-exec dispatch and clear the
/// pending state: out = [addr (seg<<16|off), report_mask, buttons,
/// x, y, mickey_x, mickey_y]. Returns 1 when a callback must fire.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_mouse_cb_info(out: *mut u32) -> u32 {
    let st = mouse_state();
    if !st.present || st.disabled || (st.cb_seg == 0 && st.cb_off == 0) {
        return 0;
    }
    let report = st.pending_events & st.cb_mask as u32;
    if report == 0 {
        return 0;
    }
    let out = unsafe { &mut *(out as *mut [u32; 7]) };
    out[0] = ((st.cb_seg as u32) << 16) | st.cb_off as u32;
    out[1] = report;
    out[2] = st.buttons as u32;
    out[3] = st.x as u32;
    out[4] = st.y as u32;
    out[5] = st.cb_mickey_x as u32;
    out[6] = st.cb_mickey_y as u32;
    st.pending_events = 0;
    st.cb_mickey_x = 0;
    st.cb_mickey_y = 0;
    1
}

/// 1 when a mouse was initialized at attach time.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_mouse_present() -> u32 {
    if mouse_state().present {
        1
    } else {
        0
    }
}

/// INT 33h API dispatch. `client` is the Jemm Client_Reg_Struc as a
/// flat dword array; results are written back into it. Returns 1 when
/// the function was handled (the stub then claims the interrupt),
/// 0 to chain to the next handler untouched.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_mouse_api(client: *mut u32) -> u32 {
    let c = unsafe { &mut *(client as *mut [u32; 19]) };
    let func = (c[CR_EAX] & 0xFFFF) as u16;
    if matches!(func, 0x0003 | 0x0005 | 0x0006 | 0x000B) {
        // Drain any completed USB HID mouse TD immediately so 125 Hz wireless
        // dongles don't build a FIFO backlog at the 18.2 Hz timer rate.
        unsafe { crate::ohci::gate1_usb_poll() };
    }
    let st = mouse_state();
    match func {
        0x0000 => {
            // Reset / status. AX=FFFFh + BX=button count when a mouse
            // is present, AX=0 when it is not.
            if !st.present {
                c[CR_EAX] = 0;
                return 1;
            }
            mouse_cursor_erase(st);
            st.reset_runtime();
            c[CR_EAX] = 0xFFFF;
            c[CR_EBX] = 3;
            1
        }
        0x0001 => {
            st.cursor_flag = (st.cursor_flag + 1).min(0);
            mouse_cursor_draw(st);
            1
        }
        0x0002 => {
            st.cursor_flag = st.cursor_flag.saturating_sub(1);
            mouse_cursor_erase(st);
            1
        }
        0x0003 => {
            c[CR_EBX] = st.buttons as u32;
            c[CR_ECX] = (st.x as i16 as u16) as u32;
            c[CR_EDX] = (st.y as i16 as u16) as u32;
            1
        }
        0x0004 => {
            st.x = clamp_i32((c[CR_ECX] & 0xFFFF) as i16 as i32, st.min_x, st.max_x);
            st.y = clamp_i32((c[CR_EDX] & 0xFFFF) as i16 as i32, st.min_y, st.max_y);
            mouse_cursor_draw(st);
            1
        }
        0x0005 | 0x0006 => {
            let idx = (c[CR_EBX] & 0xFFFF) as usize;
            if idx >= 3 {
                return 0;
            }
            let (count, px, py) = if func == 0x0005 {
                (st.press_count[idx], st.press_x[idx], st.press_y[idx])
            } else {
                (st.release_count[idx], st.release_x[idx], st.release_y[idx])
            };
            c[CR_EAX] = st.buttons as u32;
            c[CR_EBX] = count as u32;
            c[CR_ECX] = (px as i16 as u16) as u32;
            c[CR_EDX] = (py as i16 as u16) as u32;
            if func == 0x0005 {
                st.press_count[idx] = 0;
            } else {
                st.release_count[idx] = 0;
            }
            1
        }
        0x0007 | 0x0008 => {
            let mut lo = (c[CR_ECX] & 0xFFFF) as i16 as i32;
            let mut hi = (c[CR_EDX] & 0xFFFF) as i16 as i32;
            if lo > hi {
                core::mem::swap(&mut lo, &mut hi);
            }
            if func == 0x0007 {
                st.min_x = lo;
                st.max_x = hi;
                st.x = clamp_i32(st.x, lo, hi);
            } else {
                st.min_y = lo;
                st.max_y = hi;
                st.y = clamp_i32(st.y, lo, hi);
            }
            mouse_cursor_draw(st);
            1
        }
        0x0009 => {
            // Define Graphics Cursor Block: BX=hot_x, CX=hot_y, ES:DX=64-byte mask.
            // Graphics-mode apps (GRX/Allegro/Links) render their own cursor in VBE LFB;
            // record the hotspot and accept cleanly.
            st.gfx_hot_x = (c[CR_EBX] & 0xFFFF) as i16;
            st.gfx_hot_y = (c[CR_ECX] & 0xFFFF) as i16;
            1
        }
        0x000A => {
            // Define Text Cursor: BX=0 software cursor (CX=AND mask, DX=XOR mask).
            if (c[CR_EBX] & 0xFFFF) == 0 {
                mouse_cursor_erase(st);
                st.cursor_and_mask = (c[CR_ECX] & 0xFFFF) as u16;
                st.cursor_xor_mask = (c[CR_EDX] & 0xFFFF) as u16;
                mouse_cursor_draw(st);
            }
            1
        }
        0x0010 => {
            // Conditional-off region: accepted.
            1
        }
        0x000B => {
            c[CR_ECX] = (st.mickey_x as i16 as u16) as u32;
            c[CR_EDX] = (st.mickey_y as i16 as u16) as u32;
            st.mickey_x = 0;
            st.mickey_y = 0;
            1
        }
        0x000C => {
            st.cb_mask = (c[CR_ECX] & 0xFFFF) as u16;
            st.cb_seg = (c[CR_ES] & 0xFFFF) as u16;
            st.cb_off = (c[CR_EDX] & 0xFFFF) as u16;
            st.pending_events = 0;
            st.cb_mickey_x = 0;
            st.cb_mickey_y = 0;
            1
        }
        0x000F => {
            let rx = (c[CR_ECX] & 0xFFFF) as u16;
            let ry = (c[CR_EDX] & 0xFFFF) as u16;
            if rx != 0 {
                st.ratio_x = rx;
            }
            if ry != 0 {
                st.ratio_y = ry;
            }
            1
        }
        0x0013 => {
            // Define Double-Speed Threshold: DX = threshold in mickeys/sec (0 -> default 64).
            let t = (c[CR_EDX] & 0xFFFF) as u16;
            st.double_speed_threshold = if t == 0 { 64 } else { t };
            1
        }
        0x0014 => {
            // Swap Interrupt Subroutines: install new (CX, ES:DX) and return previous (CX, ES:DX).
            let old_mask = st.cb_mask;
            let old_seg = st.cb_seg;
            let old_off = st.cb_off;
            st.cb_mask = (c[CR_ECX] & 0xFFFF) as u16;
            st.cb_seg = (c[CR_ES] & 0xFFFF) as u16;
            st.cb_off = (c[CR_EDX] & 0xFFFF) as u16;
            st.pending_events = 0;
            st.cb_mickey_x = 0;
            st.cb_mickey_y = 0;
            c[CR_ECX] = old_mask as u32;
            c[CR_EDX] = old_off as u32;
            c[CR_ES] = (c[CR_ES] & 0xFFFF_0000) | old_seg as u32;
            1
        }
        0x0015 => {
            // Return Driver Storage Requirements in BX.
            c[CR_EBX] = core::mem::size_of::<MouseState>() as u32;
            1
        }
        0x0016 | 0x0017 => {
            // Save/restore the whole state block at ES:DX. The state
            // is plain data (no pointers), so a byte copy is exact.
            #[cfg(target_arch = "x86")]
            {
                let lin = (((c[CR_ES] & 0xFFFF) << 4) + (c[CR_EDX] & 0xFFFF)) as *mut u8;
                let size = core::mem::size_of::<MouseState>();
                if !lin.is_null() {
                    unsafe {
                        if func == 0x0016 {
                            core::ptr::copy_nonoverlapping(
                                st as *const MouseState as *const u8,
                                lin,
                                size,
                            );
                        } else {
                            core::ptr::copy_nonoverlapping(
                                lin,
                                st as *mut MouseState as *mut u8,
                                size,
                            );
                        }
                    }
                }
            }
            1
        }
        0x001A => {
            // Set Mouse Sensitivity: BX=horiz (1..100), CX=vert (1..100), DX=threshold (1..100).
            let sx = (c[CR_EBX] & 0xFFFF) as u16;
            let sy = (c[CR_ECX] & 0xFFFF) as u16;
            let th = (c[CR_EDX] & 0xFFFF) as u16;
            if sx != 0 {
                st.sens_x = sx.clamp(1, 100);
            }
            if sy != 0 {
                st.sens_y = sy.clamp(1, 100);
            }
            if th != 0 {
                st.double_speed_threshold = th.clamp(1, 100);
            }
            1
        }
        0x001B => {
            // Return Mouse Sensitivity: BX=horiz, CX=vert, DX=double-speed threshold.
            c[CR_EBX] = st.sens_x as u32;
            c[CR_ECX] = st.sens_y as u32;
            c[CR_EDX] = st.double_speed_threshold as u32;
            1
        }
        0x001D => {
            // Define Display Page Number.
            st.display_page = (c[CR_EBX] & 0xFFFF) as u16;
            1
        }
        0x001E => {
            // Return Display Page Number.
            c[CR_EBX] = st.display_page as u32;
            1
        }
        0x001F => {
            // Disable Mouse Driver: returns AX=001Fh and ES:BX = previous INT 33h vector
            // (0040:00F0 stub), or AX=FFFFh if no mouse is present.
            if !st.present {
                c[CR_EAX] = 0xFFFF;
                return 1;
            }
            mouse_cursor_erase(st);
            st.disabled = true;
            c[CR_EAX] = 0x001F;
            c[CR_EBX] = 0x00F0;
            c[CR_ES] = (c[CR_ES] & 0xFFFF_0000) | 0x0040;
            1
        }
        0x0020 => {
            // Enable Mouse Driver: returns AX=0020h, or AX=FFFFh if no mouse is present.
            if !st.present {
                c[CR_EAX] = 0xFFFF;
                return 1;
            }
            st.disabled = false;
            mouse_cursor_draw(st);
            c[CR_EAX] = 0x0020;
            1
        }
        0x0021 => {
            if (c[CR_ECX] & 0xFFFF) == 0xFD01 {
                // Ferric diagnostic (called by MTEST.COM with CX=0xFD01):
                // BX = completed packets, CX = dropped/resync bytes,
                // DX = packet assembler index.
                c[CR_EBX] = st.packets;
                c[CR_ECX] = st.dropped_bytes;
                c[CR_EDX] = st.packet_idx as u32;
                return 1;
            }
            // Standard MS-MOUSE 0021h Software Reset (used by FreeDOS EDIT /
            // D-Flat and many DOS TUI apps): returns AX=FFFFh, BX=buttons when
            // installed, or AX=0021h when no mouse is present.
            if !st.present {
                return 1;
            }
            mouse_cursor_erase(st);
            st.reset_runtime();
            c[CR_EAX] = 0xFFFF;
            c[CR_EBX] = 3;
            1
        }
        0x0022 => {
            // Ferric diagnostic: copy the raw byte log (every byte the
            // packet hook was fed, before resync) to ES:DX and report
            // BX = logged bytes, CX = total bytes fed. The byte stream
            // is the ground truth when the assembled state looks wrong.
            #[cfg(target_arch = "x86")]
            {
                let lin = (((c[CR_ES] & 0xFFFF) << 4) + (c[CR_EDX] & 0xFFFF)) as *mut u8;
                let n = st.byte_log_len as usize;
                if !lin.is_null() && n > 0 {
                    unsafe {
                        core::ptr::copy_nonoverlapping(st.byte_log.as_ptr(), lin, n);
                    }
                }
            }
            c[CR_EBX] = st.byte_log_len as u32;
            c[CR_ECX] = st.fed_total;
            1
        }
        0x0023 => {
            // Ferric diagnostic: AX = init stage reached at attach
            // (1..7 = the step that failed, 8 = configuration verify
            // failed and the original was restored, 9 = success),
            // BX = last byte the init read (FFFFh = none), CX = the
            // controller configuration byte as first read. The stub installs
            // the INT 33h hook even when init failed, precisely so
            // this stays answerable on a machine whose mouse never
            // came up — a bare "no mouse" would waste a hardware trip.
            c[CR_EAX] = st.init_stage as u32;
            c[CR_EBX] = st.init_last as u32;
            c[CR_ECX] = st.init_cfg as u32;
            c[CR_EDX] =
                (st.init_attempts as u32) | ((st.init_strays as u32) << 8);
            1
        }
        0x0024 => {
            let is_ferric_diag = (c[CR_ECX] & 0xFFFF) == 0xFD01
                || (!st.present && st.init_log_len > 0);
            if is_ferric_diag {
                // Ferric diagnostic (MTEST.COM status_absent / explicit CX=0xFD01):
                // copy the init exchange log to ES:DX as u16 entries.
                #[cfg(target_arch = "x86")]
                {
                    let lin = (((c[CR_ES] & 0xFFFF) << 4) + (c[CR_EDX] & 0xFFFF)) as *mut u8;
                    let n = st.init_log_len as usize;
                    if !lin.is_null() && n > 0 {
                        unsafe {
                            core::ptr::copy_nonoverlapping(
                                st.init_log.as_ptr() as *const u8,
                                lin,
                                n * 2,
                            );
                        }
                    }
                }
                c[CR_EBX] = st.init_log_len as u32;
                c[CR_ECX] =
                    (st.init_attempts as u32) | ((st.init_strays as u32) << 8);
                return 1;
            }
            // Standard MS-MOUSE 0024h: Get Software Version, Mouse Type, and IRQ.
            // Returns AX=0024h, BX=0820h (v8.20), CH=04h (PS/2), CL=00h (PS/2 IRQ).
            if !st.present {
                c[CR_EAX] = 0xFFFF;
                return 1;
            }
            c[CR_EAX] = 0x0024;
            c[CR_EBX] = 0x0820;
            c[CR_ECX] = 0x0400;
            1
        }
        0x0026 => {
            // Get Maximum Virtual Coordinates:
            // BX = mouse-disabled flag (0 = enabled, 1 = disabled),
            // CX = max X, DX = max Y.
            c[CR_EBX] = if st.disabled || !st.present { 1 } else { 0 };
            c[CR_ECX] = (st.max_x as i16 as u16) as u32;
            c[CR_EDX] = (st.max_y as i16 as u16) as u32;
            1
        }
        0x0031 => {
            // Get Current Minimum/Maximum Virtual Coordinates (MS-MOUSE 7.05+):
            // AX = min X, BX = min Y, CX = max X, DX = max Y.
            c[CR_EAX] = (st.min_x as i16 as u16) as u32;
            c[CR_EBX] = (st.min_y as i16 as u16) as u32;
            c[CR_ECX] = (st.max_x as i16 as u16) as u32;
            c[CR_EDX] = (st.max_y as i16 as u16) as u32;
            1
        }
        _ => 0,
    }
}

// =============================================================================
// Gate 2 Step 2.3 — Intel 82550 packet driver, resident in the JLM.
// =============================================================================
//
// The platform's third resident device (after the IRQ 11 telemetry of
// Gate 1 and the PS/2 mouse of step 2.2), and the first one DOS
// programs talk to through a classic real-mode contract: the packet
// driver specification (Crynwr / FTP Software). That contract shapes
// the whole design:
//
//   - Clients find the driver by the "PKT DRVR" signature at offset 3
//     of the INT 60h handler and call through the vector. A Jemm chain
//     hook cannot satisfy that — the vector does not point at hook
//     code — so a tiny real-mode stub (asm/pktstub.asm, embedded in
//     this module) is copied into a self-owned DOS block at install
//     time; INT 60h points at the stub, and the stub forwards every
//     call to gate1_net_api through INT 0F1h, which the JLM does hook.
//   - Received frames are delivered by a two-phase upcall to the
//     client's receiver (AX=0: offer a buffer; AX=1: frame copied,
//     consume it). The upcall runs through Jemm nested execution from
//     inside the IRQ 11 chain hook, under the same save/restore
//     discipline as the INT 33h mouse callback: the client register
//     block is saved before the nest and restored after it, because
//     nested execution returns the callback's registers in the block.
//   - IRQ 11 is shared and level-triggered (82550 + NEC OHCI #1 +
//     VGA). The SDM's discipline is the law here: read SCB Stat/Ack;
//     zero or FFh means the interrupt is not ours — touch nothing and
//     chain. Otherwise write the byte back to acknowledge at the
//     device, do bounded work (at most one ring of frames), and still
//     chain: the handlers behind us own the PIC EOI path.
//   - DMA structures (RFD ring, TX command block, statistics) live in
//     a DOS-allocated block below 1 MiB whose physical contiguity is
//     proven through VDMAD before a single descriptor is trusted —
//     the Gate 1 DMA discipline, applied to a working device.
//     Descriptors carry the VDMAD-returned physical base; CPU access
//     goes through the block's linear address (identical below 1 MiB,
//     kept as separate values so neither assumption is load-bearing).
//
// Installation is lazy, driven by NETINIT.COM through the JLM's V86
// API (AX=0003h): the stub's dispatch proc owns a live client context
// there, which is what the nested INT 21h block allocations need.
// DllMain therefore never touches DOS, exactly as before.

/// Additional Client_Reg_Struc dword indices (JLM.INC).
const CR_EDI: usize = 0;
const CR_ESI: usize = 1;
const CR_CS: usize = 11;
const CR_EFLAGS: usize = 12;
const CR_DS: usize = 16;

/// The real-mode stub, assembled by build.sh before cargo runs.
#[cfg(target_arch = "x86")]
static PKT_STUB: &[u8] = include_bytes!("../../../out/pktstub.bin");
#[cfg(not(target_arch = "x86"))]
static PKT_STUB: &[u8] = &[];

// ---------------------------------------------------------------------
// 32-bit port I/O (the mouse module owns the 8-bit pair).
// ---------------------------------------------------------------------

#[cfg(target_arch = "x86")]
unsafe fn inl(port: u16) -> u32 {
    let v: u32;
    unsafe {
        core::arch::asm!("in eax, dx", in("dx") port, out("eax") v, options(nomem, nostack));
    }
    v
}

#[cfg(not(target_arch = "x86"))]
unsafe fn inl(_port: u16) -> u32 {
    0
}

#[cfg(target_arch = "x86")]
unsafe fn outl(port: u16, val: u32) {
    unsafe {
        core::arch::asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack));
    }
}

#[cfg(not(target_arch = "x86"))]
unsafe fn outl(_port: u16, _val: u32) {}

#[cfg(not(target_arch = "x86"))]
unsafe fn inb(_port: u16) -> u8 {
    0
}

#[cfg(not(target_arch = "x86"))]
unsafe fn outb(_port: u16, _val: u8) {}

// ---------------------------------------------------------------------
// PCI configuration space (mechanism 1, ports CF8h/CFCh).
// ---------------------------------------------------------------------

unsafe fn pci_read32(bus: u8, dev: u8, func: u8, off: u8) -> u32 {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((off as u32) & 0xFC);
    unsafe {
        outl(0xCF8, addr);
        inl(0xCFC)
    }
}

unsafe fn pci_write32(bus: u8, dev: u8, func: u8, off: u8, val: u32) {
    let addr = 0x8000_0000u32
        | ((bus as u32) << 16)
        | ((dev as u32) << 11)
        | ((func as u32) << 8)
        | ((off as u32) & 0xFC);
    unsafe {
        outl(0xCF8, addr);
        outl(0xCFC, val);
    }
}

// ---------------------------------------------------------------------
// DMA region layout (one 16 KiB block).
// ---------------------------------------------------------------------

pub use ferric_i8255x::dma::{NET_DMA_SIZE, NET_DMA_PARAS};
use ferric_i8255x::dma::{
    DmaRegion, receive_descriptor, transmit_descriptor,
    RFD_COUNT, RFD_STRIDE, RFD_HDR, FRAME_BUF, TXCB_OFF, TXBUF_OFF, STATS_OFF,
    RFD_STATUS, RFD_CMD, RFD_LINK, RFD_RBD, RFD_ACTUAL, RFD_SIZE,
    CB_STATUS, CB_CMD, CB_LINK, CB_PARAM, CB_TBD_ADDR, CB_THRESH, CB_TBD_COUNT, TBD_ADDR, TBD_LEN,
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
    dev_ok: bool,
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
    wifi_mac: [u8; 6],
    wifi_active: bool,
    wifi_rx_buf: [u8; 1600],
}

impl NetState {
    const fn new() -> Self {
        Self {
            ethernet: ferric_i8255x::state::EthernetState::new(),
            present: false,
            stage: 0,
            dev_ok: false,
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
            wifi_mac: [0; 6],
            wifi_active: false,
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
pub(crate) fn net_activate_wifi(mac: [u8; 6]) {
    let st = net_state();
    st.present = true;
    st.wifi_active = true;
    st.wifi_mac = mac;
    st.dev_ok = true;
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

pub(crate) fn net_diag_counts() -> (u32, u32, u32, u32) {
    let st = net_state();
    (st.rx_packets, st.rx_dropped, st.tx_packets, st.tx_errors)
}

/// FFI entry for JLM V86 API `AX=0x000E` (`WPROBE /E` / `WPROBE /W`):
/// switch `INT 60h` between Ethernet (`use_wifi == 0`) and Wi-Fi (`use_wifi != 0`).
/// Returns `1` on success, `0` if the requested adapter is not ready.
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

unsafe fn scb_wait_cmd(io: u16) -> bool {
    ferric_i8255x::scb::wait_idle(&mut NetScbIo(io), 2, SCB_POLL_LIMIT)
}

/// Issue a command only if the previous command has been accepted.
unsafe fn scb_cmd(io: u16, gp: u32, cmd: u8) -> Result<(), ferric_i8255x::scb::CommandError> {
    ferric_i8255x::scb::submit(&mut NetScbIo(io), gp, cmd, 2, SCB_POLL_LIMIT)
}

/// Run the command block built at TXCB_OFF to completion (polled).
/// Callers zero and fill the block first; EL is always set on it.
unsafe fn cu_exec(st: &mut NetState) -> bool {
    let io = st.ethernet.io_base;
    let lin = st.ethernet.dma_lin;
    if unsafe { scb_cmd(io, st.ethernet.dma_phys + TXCB_OFF as u32, 0x10) }.is_err() { return false; }
    let start = bios_tick();
    for _ in 0..SCB_POLL_LIMIT {
        if dma_r16(lin, TXCB_OFF + CB_STATUS) & 0x8000 != 0 {
            return true;
        }
        if bios_tick().wrapping_sub(start) > 36 {
            return false;
        }
    }
    false
}

unsafe fn zero_cb(st: &NetState) {
    for i in 0..32 {
        dma_w8(st.ethernet.dma_lin, TXCB_OFF + i, 0);
    }
}

unsafe fn net_configure(st: &mut NetState, promisc: bool) -> bool {
    let cfg = ferric_i8255x::configure_bytes(promisc);
    unsafe {
        zero_cb(st);
        dma_w16(st.ethernet.dma_lin, TXCB_OFF + CB_CMD, 0x8002); // Configure | EL
        for (i, b) in cfg.iter().enumerate() {
            dma_w8(st.ethernet.dma_lin, TXCB_OFF + CB_PARAM + i, *b);
        }
        cu_exec(st)
    }
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
    if st.dev_ok {
        return 1;
    }
    for dev in 0..32u8 {
        let id = unsafe { pci_read32(0, dev, 0, 0) };
        let vid = (id & 0xFFFF) as u16;
        let did = (id >> 16) as u16;
        if vid != 0x8086 || (did != 0x1229 && did != 0x1209) {
            continue;
        }
        let cmd = unsafe { pci_read32(0, dev, 0, 0x04) };
        if cmd & 0x0005 != 0x0005 {
            unsafe {
                pci_write32(0, dev, 0, 0x04, (cmd & 0xFFFF_0000) | ((cmd & 0xFFFF) | 0x0005));
            }
        }
        let bar1 = unsafe { pci_read32(0, dev, 0, 0x14) };
        st.ethernet.io_base = (bar1 & 0xFFFC) as u16;
        st.irq = (unsafe { pci_read32(0, dev, 0, 0x3C) } & 0xFF) as u8;
        match unsafe { ee_read_mac(st.ethernet.io_base) } {
            Some((mac, width, cksum)) => {
                st.mac = mac;
                st.mac_eeprom = true;
                st.ethernet.ee_width = width;
                st.ethernet.ee_cksum = cksum;
            }
            None => {
                // Locally administered fallback ("FERRI" in the low
                // bytes); NTEST's macsrc field reports 0 so a fallback
                // is never mistaken for a real station address.
                st.mac = [0x02, 0x46, 0x45, 0x52, 0x52, 0x49];
                st.mac_eeprom = false;
            }
        }
        st.dev_ok = true;
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
    }
    st.present = false;
    st.ethernet.eth_ready = false;
    2
}


/// Build the receive ring in the DMA block (shared by bring-up and
/// the packet driver's reset_interface).
unsafe fn net_ring_init(st: &mut NetState) {
    let lin = st.ethernet.dma_lin;
    let Some(region) = DmaRegion::new(st.ethernet.dma_phys, lin) else { return; };
    for i in 0..RFD_COUNT {
        let off = i * RFD_STRIDE;
        let Some(desc) = receive_descriptor(region, i) else { return; };
        dma_w16(lin, off + RFD_STATUS, 0);
        dma_w16(lin, off + RFD_CMD, desc.command);
        dma_w32(lin, off + RFD_LINK, desc.link);
        dma_w32(lin, off + RFD_RBD, desc.buffer_descriptor);
        dma_w16(lin, off + RFD_ACTUAL, 0);
        dma_w16(lin, off + RFD_SIZE, desc.size);
    }
    st.ethernet.rx_clean = 0;
    st.ethernet.rx_tail = RFD_COUNT - 1;
    st.svc_rfd = -1;
    st.svc_phase = 0;
    st.ethernet.ru_restart = -1;
    st.ethernet.ru_starved = false;
}

/// Full device bring-up against the DOS DMA block NETINIT allocated.
/// Stage numbers are the diagnostic contract (NTEST function 80h):
/// 1 probed, 2 DMA validated, 3 reset + configured, 4 IA set,
/// 5 RU started, 6 ready (present).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn gate1_net_init(dma_seg: u32) -> u32 {
    let st = net_state();
    if !st.dev_ok {
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
    st.stage = 2;
    for i in 0..NET_DMA_SIZE {
        dma_w8(st.ethernet.dma_lin, i, 0);
    }
    unsafe {
        // Software reset, then let the SCB settle.
        outl(io + SCB_PORT, 0);
        for _ in 0..4096 {
            let _ = inb(io + SCB_CMD);
        }
        if !scb_wait_cmd(io) || scb_cmd(io, 0, 0x60).is_err() || scb_cmd(io, 0, 0x06).is_err() {
            return net_init_failed(st);
        }
    }
    if !unsafe { net_configure(st, false) } {
        return unsafe { net_init_failed(st) };
    }
    st.stage = 3;
    unsafe {
        zero_cb(st);
        dma_w16(st.ethernet.dma_lin, TXCB_OFF + CB_CMD, 0x8001); // IA setup | EL
        for i in 0..6 {
            dma_w8(st.ethernet.dma_lin, TXCB_OFF + CB_PARAM + i, st.mac[i]);
        }
        if !cu_exec(st) {
            return net_init_failed(st);
        }
    }
    st.stage = 4;
    unsafe {
        // Statistics dump target (counters read on demand later).
        if scb_cmd(io, st.ethernet.dma_phys + STATS_OFF as u32, 0x40).is_err() {
            return net_init_failed(st);
        }
        net_ring_init(st);
        if scb_cmd(io, st.ethernet.dma_phys, 0x01).is_err() {
            return net_init_failed(st);
        }
    }
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
        if bseg == 0 && boff == 0 {
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
        if st.wifi_active {
            unsafe {
                core::ptr::copy_nonoverlapping(st.wifi_rx_buf.as_ptr(), dst as *mut u8, n);
            }
        } else {
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
        loop {
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
    while unsafe { inb(io + SCB_STATUS) } & 0xC0 != 0 {
        if bios_tick().wrapping_sub(start) > 2 {
            break;
        }
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

fn v86_lin(seg: u32, off: u32) -> usize {
    (((seg & 0xFFFF) << 4) + (off & 0xFFFF)) as usize
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
                let lin = v86_lin(c[CR_DS], c[CR_ESI]);
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
            let src = v86_lin(c[CR_DS], c[CR_ESI]) as u32;
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
            let dst = v86_lin(c[CR_ES], c[CR_EDI]);
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
                        return 0;
                    }
                }
            }
            net_ok(c);
        }
        0x0A => {
            // get_parameters: DS:SI = 14-byte structure.
            let dst = v86_lin(c[CR_DS], c[CR_ESI]);
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
            let dst = v86_lin(c[CR_DS], c[CR_ESI]);
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
                    core::ptr::write_volatile((dst + i * 4) as *mut u32, *v);
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
