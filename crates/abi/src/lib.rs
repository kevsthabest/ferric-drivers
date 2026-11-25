//! Shared versioned DOS-to-Rust ABI for Ferric DOS (Gate 0 foundation).
//!
//! All types in this crate are `#![no_std]`, fixed-width, `#[repr(C)]`, and use
//! checked integer arithmetic only (no floating-point, no hardware atomics).
//!
//! # Register-Preservation Contract
//!
//! 1. **32-bit protected-mode Rust entry points** use the 32-bit x86 `cdecl`
//!    (`extern "C"`) calling convention:
//!    - Callee preserves: `EBX`, `ESI`, `EDI`, `EBP`, `ESP`, and `EFLAGS.DF = 0`.
//!    - Caller-saved scratch: `EAX`, `ECX`, `EDX`.
//!    - Return value: `EAX` carries a [`StatusCode`] (`u32`). Structured outputs
//!      are written into caller-provided `#[repr(C)]` buffers with explicit
//!      byte capacities.
//! 2. **Real-mode <-> protected-mode assembly/C stubs** must:
//!    - Save and restore all segment registers (`DS`, `ES`, `FS`, `GS`, `SS`),
//!      `EFLAGS` (preserving `IF` across non-ISR calls), and every general-purpose
//!      register not explicitly carrying a return status.
//!    - Never invoke DOS `INT 21h` from ISRs or deferred callbacks (DOS is
//!      non-reentrant).
//!    - Never rely on x87, MMX, SSE, or SSE2 state on the baseline `i386` path.
//!    - Synchronize ISR-to-foreground state via `CLI`/`STI` critical sections
//!      (single-core DOS; `max-atomic-width = 0` on the `i386` target).
//!
//! # Low-Memory Bounce-Buffer Rules
//!
//! 16-bit real-mode callers pass segmented `SEG:OFF` far pointers ([`FarPtr16`]).
//! Before crossing into 32-bit Rust services:
//! - Validate the far pointer and length so the linear range stays strictly
//!   below [`LOW_MEM_LIMIT`] (1 MiB) and does not wrap a 64 KiB real-mode segment.
//! - Copy caller data into a validated low-memory bounce buffer described by
//!   [`BounceBufferDesc`], enforcing alignment and (when requested for ISA DMA)
//!   no 64 KiB physical boundary crossing.

#![no_std]
#![forbid(unsafe_code)]
#![deny(warnings)]

pub mod fs_abi;
pub mod hwscan;
pub mod vbe_abi;

/// ASCII `"FDOS"` in little-endian (`0x534F4446` -> bytes `F`, `D`, `O`, `S`).
pub const ABI_MAGIC: u32 = u32::from_le_bytes(*b"FDOS");

/// Current ABI major version. Breaking layout changes bump major.
pub const ABI_VERSION_MAJOR: u16 = 0;

/// Current ABI minor version. Backward-compatible additions bump minor.
pub const ABI_VERSION_MINOR: u16 = 1;

/// Upper bound of 16-bit real-mode addressable low memory (1 MiB).
pub const LOW_MEM_LIMIT: u32 = 0x0010_0000;

/// Real-mode 16-bit segment size (64 KiB).
pub const REAL_MODE_SEGMENT_LIMIT: u32 = 0x0001_0000;

/// Bounce buffer must reside strictly below 1 MiB (`LOW_MEM_LIMIT`).
pub const BOUNCE_FLAG_BELOW_1M: u32 = 1 << 0;

/// Bounce buffer must not cross a 64 KiB physical address boundary (ISA DMA rule).
pub const BOUNCE_FLAG_NO_64K_CROSS: u32 = 1 << 1;

/// Bounce buffer is read-only from the service's perspective.
pub const BOUNCE_FLAG_READ_ONLY: u32 = 1 << 2;

/// Fixed-width ABI status codes returned in `EAX` and response headers.
#[repr(u32)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum StatusCode {
    Ok = 0,
    InvalidMagic = 1,
    UnsupportedVersion = 2,
    BufferTooSmall = 3,
    InvalidLength = 4,
    InvalidPointer = 5,
    BounceRangeInvalid = 6,
    NotPresent = 7,
    NotSupported = 8,
    Timeout = 9,
    ReadOnlyViolation = 10,
    HardwareError = 11,
    InternalError = 0xFFFF_FFFF,
}

impl StatusCode {
    #[inline]
    pub const fn as_u32(self) -> u32 {
        self as u32
    }

    #[inline]
    pub const fn is_ok(self) -> bool {
        matches!(self, StatusCode::Ok)
    }
}

/// ABI version pair for explicit compatibility negotiation.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct AbiVersion {
    pub major: u16,
    pub minor: u16,
}

impl AbiVersion {
    pub const CURRENT: Self = Self {
        major: ABI_VERSION_MAJOR,
        minor: ABI_VERSION_MINOR,
    };

    /// Returns `true` when a caller requesting `self` is compatible with `provider`.
    /// Major versions must match exactly and caller minor must not exceed provider minor.
    #[inline]
    pub const fn is_compatible_with(self, provider: Self) -> bool {
        self.major == provider.major && self.minor <= provider.minor
    }
}

/// 16-bit real-mode far pointer (`SEG:OFF`, little-endian offset first as in x86 memory).
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct FarPtr16 {
    pub offset: u16,
    pub segment: u16,
}

impl FarPtr16 {
    pub const NULL: Self = Self {
        offset: 0,
        segment: 0,
    };

    #[inline]
    pub const fn is_null(self) -> bool {
        self.segment == 0 && self.offset == 0
    }

    /// Computes the 20-bit linear address `(segment << 4) + offset`, returning `None`
    /// if the result reaches or exceeds `LOW_MEM_LIMIT` (1 MiB, i.e., HMA wrap).
    #[inline]
    pub const fn linear_addr(self) -> Option<u32> {
        let base = (self.segment as u32) << 4;
        let addr = base + (self.offset as u32);
        if addr < LOW_MEM_LIMIT {
            Some(addr)
        } else {
            None
        }
    }

    /// Validates that `[self, self + len)` does not wrap the 64 KiB segment and
    /// stays strictly within conventional/UMA low memory (`< LOW_MEM_LIMIT`).
    pub const fn validate_range(self, len: u32) -> Result<u32, StatusCode> {
        if self.is_null() {
            return Err(StatusCode::InvalidPointer);
        }
        if len == 0 || len > REAL_MODE_SEGMENT_LIMIT {
            return Err(StatusCode::InvalidLength);
        }
        let seg_end = (self.offset as u32).checked_add(len);
        match seg_end {
            Some(end) if end <= REAL_MODE_SEGMENT_LIMIT => {}
            _ => return Err(StatusCode::BounceRangeInvalid),
        }
        let Some(linear) = self.linear_addr() else {
            return Err(StatusCode::BounceRangeInvalid);
        };
        match linear.checked_add(len) {
            Some(end) if end <= LOW_MEM_LIMIT => Ok(linear),
            _ => Err(StatusCode::BounceRangeInvalid),
        }
    }
}

/// Descriptor for a low-memory bounce buffer used to marshal far-pointer data
/// between 16-bit real-mode stubs and 32-bit protected-mode Rust services.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct BounceBufferDesc {
    /// Physical/linear base address of the bounce buffer.
    pub phys_addr: u32,
    /// Total allocated capacity in bytes.
    pub capacity: u32,
    /// Active payload length in bytes (`<= capacity`).
    pub len: u32,
    /// Required byte alignment (non-zero power of two, e.g. 1, 4, 16, 512).
    pub align: u32,
    /// Bitmask of `BOUNCE_FLAG_*` constraints.
    pub flags: u32,
}

impl BounceBufferDesc {
    /// Validates address, length, alignment, low-memory ceiling, and 64 KiB
    /// boundary constraints using checked arithmetic.
    pub const fn validate(&self) -> Result<(), StatusCode> {
        if self.phys_addr == 0 {
            return Err(StatusCode::InvalidPointer);
        }
        if self.capacity == 0 || self.len > self.capacity {
            return Err(StatusCode::InvalidLength);
        }
        if self.align == 0 || (self.align & (self.align - 1)) != 0 {
            return Err(StatusCode::BounceRangeInvalid);
        }
        if (self.phys_addr & (self.align - 1)) != 0 {
            return Err(StatusCode::BounceRangeInvalid);
        }
        let Some(end) = self.phys_addr.checked_add(self.capacity) else {
            return Err(StatusCode::BounceRangeInvalid);
        };
        if (self.flags & BOUNCE_FLAG_BELOW_1M) != 0 && end > LOW_MEM_LIMIT {
            return Err(StatusCode::BounceRangeInvalid);
        }
        if (self.flags & BOUNCE_FLAG_NO_64K_CROSS) != 0 {
            let start_page = self.phys_addr >> 16;
            let last_page = (end - 1) >> 16;
            if start_page != last_page {
                return Err(StatusCode::BounceRangeInvalid);
            }
        }
        Ok(())
    }
}

/// Common versioned request header placed at the start of every DOS-to-Rust call buffer.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct RequestHeader {
    pub magic: u32,
    pub version: AbiVersion,
    pub opcode: u16,
    pub flags: u16,
    pub request_len: u32,
    pub response_cap: u32,
    pub sequence: u32,
}

impl RequestHeader {
    pub const SIZE: usize = core::mem::size_of::<Self>();

    /// Validates magic, ABI version compatibility, and minimum header/payload lengths.
    pub const fn validate(&self, min_request_len: u32) -> Result<(), StatusCode> {
        if self.magic != ABI_MAGIC {
            return Err(StatusCode::InvalidMagic);
        }
        if !self.version.is_compatible_with(AbiVersion::CURRENT) {
            return Err(StatusCode::UnsupportedVersion);
        }
        if (self.request_len as usize) < Self::SIZE || self.request_len < min_request_len {
            return Err(StatusCode::InvalidLength);
        }
        Ok(())
    }
}

/// Common versioned response header written at the start of every Rust-to-DOS reply buffer.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct ResponseHeader {
    pub magic: u32,
    pub version: AbiVersion,
    pub status: u32,
    pub response_len: u32,
    pub required_len: u32,
    pub sequence: u32,
}

impl ResponseHeader {
    pub const SIZE: usize = core::mem::size_of::<Self>();

    #[inline]
    pub const fn ok(sequence: u32, response_len: u32) -> Self {
        Self {
            magic: ABI_MAGIC,
            version: AbiVersion::CURRENT,
            status: StatusCode::Ok.as_u32(),
            response_len,
            required_len: response_len,
            sequence,
        }
    }

    #[inline]
    pub const fn err(sequence: u32, status: StatusCode, required_len: u32) -> Self {
        Self {
            magic: ABI_MAGIC,
            version: AbiVersion::CURRENT,
            status: status.as_u32(),
            response_len: Self::SIZE as u32,
            required_len,
            sequence,
        }
    }
}

/// C-ABI validation helper exported for stub integration and 386 codegen verification.
pub extern "C" fn abi_validate_request(header: &RequestHeader, min_len: u32) -> u32 {
    match header.validate(min_len) {
        Ok(()) => StatusCode::Ok.as_u32(),
        Err(code) => code.as_u32(),
    }
}

/// C-ABI bounce-buffer validation helper exported for stub integration and 386 codegen verification.
pub extern "C" fn abi_validate_bounce(desc: &BounceBufferDesc) -> u32 {
    match desc.validate() {
        Ok(()) => StatusCode::Ok.as_u32(),
        Err(code) => code.as_u32(),
    }
}

// Compile-time layout invariants (verified on both i386-unknown-none and host targets).
const _: () = {
    assert!(core::mem::size_of::<StatusCode>() == 4);
    assert!(core::mem::size_of::<AbiVersion>() == 4);
    assert!(core::mem::size_of::<FarPtr16>() == 4);
    assert!(core::mem::size_of::<BounceBufferDesc>() == 20);
    assert!(core::mem::size_of::<RequestHeader>() == 24);
    assert!(core::mem::size_of::<ResponseHeader>() == 24);
    assert!(core::mem::align_of::<RequestHeader>() == 4);
    assert!(core::mem::align_of::<ResponseHeader>() == 4);
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn far_ptr_validation() {
        let p = FarPtr16 {
            segment: 0x1000,
            offset: 0x0000,
        };
        assert_eq!(p.linear_addr(), Some(0x0001_0000));
        assert_eq!(p.validate_range(512), Ok(0x0001_0000));

        // Segment wrap rejected.
        let wrap = FarPtr16 {
            segment: 0x1000,
            offset: 0xFF00,
        };
        assert_eq!(wrap.validate_range(512), Err(StatusCode::BounceRangeInvalid));

        // Above 1 MiB rejected.
        let hma = FarPtr16 {
            segment: 0xFFFF,
            offset: 0x0010,
        };
        assert_eq!(hma.linear_addr(), None);
        assert_eq!(hma.validate_range(16), Err(StatusCode::BounceRangeInvalid));

        // Null pointer rejected.
        assert_eq!(FarPtr16::NULL.validate_range(16), Err(StatusCode::InvalidPointer));
    }

    #[test]
    fn bounce_buffer_validation() {
        let good = BounceBufferDesc {
            phys_addr: 0x0002_0000,
            capacity: 4096,
            len: 512,
            align: 512,
            flags: BOUNCE_FLAG_BELOW_1M | BOUNCE_FLAG_NO_64K_CROSS | BOUNCE_FLAG_READ_ONLY,
        };
        assert_eq!(good.validate(), Ok(()));
        assert_eq!(abi_validate_bounce(&good), StatusCode::Ok.as_u32());

        // Crosses 64 KiB boundary (0x0001_FE00 .. 0x0002_0200).
        let crosses_64k = BounceBufferDesc {
            phys_addr: 0x0001_FE00,
            capacity: 1024,
            len: 512,
            align: 512,
            flags: BOUNCE_FLAG_BELOW_1M | BOUNCE_FLAG_NO_64K_CROSS,
        };
        assert_eq!(crosses_64k.validate(), Err(StatusCode::BounceRangeInvalid));

        // Misaligned address.
        let misaligned = BounceBufferDesc {
            phys_addr: 0x0002_0004,
            capacity: 512,
            len: 512,
            align: 512,
            flags: BOUNCE_FLAG_BELOW_1M,
        };
        assert_eq!(misaligned.validate(), Err(StatusCode::BounceRangeInvalid));
    }

    #[test]
    fn request_and_response_headers() {
        let req = RequestHeader {
            magic: ABI_MAGIC,
            version: AbiVersion::CURRENT,
            opcode: hwscan::OP_HWSCAN_QUERY,
            flags: 0,
            request_len: RequestHeader::SIZE as u32,
            response_cap: 4096,
            sequence: 7,
        };
        assert_eq!(req.validate(RequestHeader::SIZE as u32), Ok(()));
        assert_eq!(abi_validate_request(&req, RequestHeader::SIZE as u32), 0);

        let bad_magic = RequestHeader { magic: 0, ..req };
        assert_eq!(
            bad_magic.validate(RequestHeader::SIZE as u32),
            Err(StatusCode::InvalidMagic)
        );

        let resp = ResponseHeader::ok(req.sequence, 64);
        assert_eq!(resp.status, StatusCode::Ok.as_u32());
        assert_eq!(resp.sequence, 7);
        assert_eq!(resp.response_len, 64);
    }
}
