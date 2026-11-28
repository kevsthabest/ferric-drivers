//! Resident resource broker. All callers must use the same resident instance.
//! JLM service wrappers serialize calls with interrupts disabled.
#![cfg_attr(not(test), no_std)]
#![deny(warnings)]

use core::cell::UnsafeCell;
use ferric_hw::ownership::{ClaimError, ClaimTable, Resource};

const VERSION: u32 = 0x0001_0000;
const CLIENTS: usize = 16;
const CLAIMS: usize = 64;
const INVALID: u32 = 1;
const BUSY: u32 = 2;
const FULL: u32 = 3;
const NOT_OWNED: u32 = 4;

struct Broker {
    clients: [u16; CLIENTS],
    claims: ClaimTable<CLAIMS>,
}

impl Broker {
    const fn new() -> Self {
        Self { clients: [0; CLIENTS], claims: ClaimTable::new() }
    }
    fn register(&mut self, owner: u16) -> u32 {
        if owner == 0 || owner == 0xFD07 { return INVALID; }
        if self.clients.contains(&owner) { return BUSY; }
        let Some(slot) = self.clients.iter_mut().find(|s| **s == 0) else { return FULL; };
        *slot = owner;
        0
    }
    fn unregister(&mut self, owner: u16) -> u32 {
        if owner == 0 { return INVALID; }
        if self.claims.has_claims(owner) { return BUSY; }
        let Some(slot) = self.clients.iter_mut().find(|s| **s == owner) else { return NOT_OWNED; };
        *slot = 0;
        0
    }
    fn claim(&mut self, owner: u16, resource: Resource) -> u32 {
        if owner == 0 || !self.clients.contains(&owner) { return NOT_OWNED; }
        code(self.claims.claim(owner, resource))
    }
    fn release(&mut self, owner: u16, resource: Resource) -> u32 {
        if owner == 0 || !self.clients.contains(&owner) { return NOT_OWNED; }
        code(self.claims.release(owner, resource))
    }
    fn can_unload(&self) -> bool { self.clients.iter().all(|c| *c == 0) }
}

fn code(r: Result<(), ClaimError>) -> u32 {
    match r {
        Ok(()) => 0,
        Err(ClaimError::Invalid) => INVALID,
        Err(ClaimError::Busy) => BUSY,
        Err(ClaimError::Full) => FULL,
        Err(ClaimError::NotOwned) => NOT_OWNED,
    }
}

fn decode(kind: u32, a: u32, b: u32) -> Option<Resource> {
    match kind {
        1 if a <= 0xFFFF && b == 0 => Some(Resource::Pci {
            bus: (a >> 8) as u8, device: ((a >> 3) & 31) as u8, function: (a & 7) as u8,
        }),
        2 if a <= 0xFFFF => Some(Resource::Io { base: a as u16, len: b }),
        3 if a < 16 && b == 0 => Some(Resource::Irq(a as u8)),
        4 if a < 8 && b == 0 => Some(Resource::Dma(a as u8)),
        5 if a <= 255 && b == 0 => Some(Resource::Vector(a as u8)),
        6 => Some(Resource::Mmio { base: a, len: b }),
        7 if a > 0 && a <= 255 && b == 0 => Some(Resource::PciBus(a as u8)),
        _ => None,
    }
}

struct Cell(UnsafeCell<Broker>);
// Single-core ring-0 contract: all entry points are serialized by the JLM
// wrappers. No Rust references escape a service call.
unsafe impl Sync for Cell {}
static BROKER: Cell = Cell(UnsafeCell::new(Broker::new()));

#[cfg(not(test))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! { loop {} }

#[unsafe(no_mangle)]
pub extern "C" fn res_version() -> u32 { VERSION }

#[unsafe(no_mangle)]
pub extern "C" fn res_register(owner: u32) -> u32 {
    if owner > 0xFFFF { return INVALID; }
    unsafe { (&mut *BROKER.0.get()).register(owner as u16) }
}
#[unsafe(no_mangle)]
pub extern "C" fn res_unregister(owner: u32) -> u32 {
    if owner > 0xFFFF { return INVALID; }
    unsafe { (&mut *BROKER.0.get()).unregister(owner as u16) }
}
#[unsafe(no_mangle)]
pub extern "C" fn res_claim(owner: u32, kind: u32, a: u32, b: u32) -> u32 {
    if owner > 0xFFFF { return INVALID; }
    let Some(resource) = decode(kind, a, b) else { return INVALID; };
    unsafe { (&mut *BROKER.0.get()).claim(owner as u16, resource) }
}
#[unsafe(no_mangle)]
pub extern "C" fn res_release(owner: u32, kind: u32, a: u32, b: u32) -> u32 {
    if owner > 0xFFFF { return INVALID; }
    let Some(resource) = decode(kind, a, b) else { return INVALID; };
    unsafe { (&mut *BROKER.0.get()).release(owner as u16, resource) }
}
#[unsafe(no_mangle)]
pub extern "C" fn res_can_unload() -> u32 {
    unsafe { u32::from((&*BROKER.0.get()).can_unload()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lifecycle_refuses_unload_and_unregister() {
        let mut b = Broker::new();
        let p = decode(1, 0x90, 0).unwrap();
        assert!(b.can_unload());
        assert_eq!(b.claim(0xFD06, p), NOT_OWNED);
        assert_eq!(b.register(0xFD06), 0);
        assert_eq!(b.register(0xFD06), BUSY);
        assert_eq!(b.register(0xFD05), 0);
        assert_eq!(b.claim(0xFD06, p), 0);
        assert_eq!(b.claim(0xFD05, p), BUSY);
        assert_eq!(b.unregister(0xFD06), BUSY);
        assert!(!b.can_unload());
        assert_eq!(b.release(0xFD05, p), NOT_OWNED);
        assert_eq!(b.release(0xFD06, p), 0);
        assert_eq!(b.unregister(0xFD06), 0);
        assert_eq!(b.unregister(0xFD05), 0);
        assert!(b.can_unload());
    }
    #[test]
    fn decode_rejects_truncation_and_unused_fields() {
        assert_eq!(decode(1, 0x10000, 0), None);
        assert_eq!(decode(1, 0x90, 1), None);
        assert_eq!(decode(5, 256, 0), None);
        assert_eq!(decode(1, 0x90, 0), Some(Resource::Pci { bus: 0, device: 18, function: 0 }));
    }
    #[test]
    fn client_capacity() {
        let mut b = Broker::new();
        for i in 1..=CLIENTS as u16 { assert_eq!(b.register(i), 0); }
        assert_eq!(b.register(100), FULL);
        assert_eq!(b.register(0), INVALID);
        assert_eq!(b.unregister(1), 0);
        assert_eq!(b.register(100), 0);
    }
}
