//! Transactional ownership for a bridge socket. Platform code supplies
//! FRES operations and a hardware snapshot. No hardware is touched here.
use crate::{SlotResources, resources::{self, Range}};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Claim { pub kind: u32, pub a: u32, pub b: u32 }
pub trait Broker {
    fn claim(&mut self, resource: Claim) -> bool;
    fn release(&mut self, resource: Claim) -> bool;
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error { Invalid, Busy, Rollback, Consumer }
pub struct Lease {
    claims: [Claim; 4],
    held: u8,
    consumer: Option<u16>,
}
impl Lease {
    pub fn new(slot: u8, r: SlotResources, occupied: &[Range], buses: &[u8]) -> Result<Self, Error> {
        if slot > 1 { return Err(Error::Invalid); }
        let [socket, window] = resources::validate(r, occupied, buses).map_err(|_| Error::Invalid)?;
        Ok(Self { claims: [
            Claim { kind: 1, a: (17 << 3) | u32::from(slot), b: 0 },
            Claim { kind: 7, a: u32::from(r.sec_bus), b: 0 },
            Claim { kind: 6, a: socket.base, b: socket.len },
            Claim { kind: 6, a: window.base, b: window.len },
        ], held: 0, consumer: None })
    }
    pub fn acquire(&mut self, broker: &mut impl Broker) -> Result<(), Error> {
        if self.held != 0 { return Err(Error::Busy); }
        for i in 0..4 {
            if !broker.claim(self.claims[i]) {
                return if self.release(broker).is_ok() { Err(Error::Busy) } else { Err(Error::Rollback) };
            }
            self.held |= 1 << i;
        }
        Ok(())
    }
    /// Call only after verified hardware quiescence. Retain failed releases
    /// for retry; never silently drop bookkeeping after a partial failure.
    pub fn release(&mut self, broker: &mut impl Broker) -> Result<(), Error> {
        if self.consumer.is_some() { return Err(Error::Consumer); }
        for i in (0..4).rev() {
            if self.held & (1 << i) != 0 {
                if !broker.release(self.claims[i]) { return Err(Error::Rollback); }
                self.held &= !(1 << i);
            }
        }
        Ok(())
    }
    pub fn attach_consumer(&mut self, id: u16) -> Result<(), Error> {
        if id == 0 || self.held != 15 { return Err(Error::Invalid); }
        if self.consumer.is_some() { return Err(Error::Busy); }
        self.consumer = Some(id);
        Ok(())
    }
    pub fn detach_consumer(&mut self, id: u16) -> Result<(), Error> {
        if self.consumer != Some(id) { return Err(Error::Consumer); }
        self.consumer = None;
        Ok(())
    }
    pub fn is_held(&self) -> bool { self.held != 0 }
    pub fn has_consumer(&self) -> bool { self.consumer.is_some() }
    pub fn consumer_is(&self,id:u16)->bool{self.consumer==Some(id)}
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::slot_resources;
    struct Mock { count: u8, fail_at: u8, fail_release: bool }
    impl Broker for Mock {
        fn claim(&mut self, _: Claim) -> bool {
            if self.count == self.fail_at { return false; }
            self.count += 1; true
        }
        fn release(&mut self, _: Claim) -> bool {
            if self.fail_release { return false; }
            self.count -= 1; true
        }
    }
    #[test]
    fn partial_acquisition_rolls_back() {
        for fail_at in 0..4 {
            let mut b = Mock { count: 0, fail_at, fail_release: false };
            let mut l = Lease::new(0,slot_resources(0).unwrap(),&[],&[]).unwrap();
            assert_eq!(l.acquire(&mut b),Err(Error::Busy));
            assert_eq!(b.count,0);
            assert!(!l.is_held());
        }
    }
    #[test]
    fn consumer_pins_bridge_and_release_is_retryable() {
        let mut b = Mock { count: 0, fail_at: 5, fail_release: false };
        let mut l = Lease::new(0,slot_resources(0).unwrap(),&[],&[]).unwrap();
        l.acquire(&mut b).unwrap();
        l.attach_consumer(0xfd0a).unwrap();
        assert_eq!(l.release(&mut b),Err(Error::Consumer));
        assert_eq!(l.detach_consumer(1),Err(Error::Consumer));
        l.detach_consumer(0xfd0a).unwrap();
        b.fail_release = true;
        assert_eq!(l.release(&mut b),Err(Error::Rollback));
        assert!(l.is_held());
        b.fail_release = false;
        l.release(&mut b).unwrap();
        assert_eq!(b.count,0);
    }
}
