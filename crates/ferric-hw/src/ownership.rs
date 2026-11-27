//! Fixed-capacity ownership bookkeeping for a future resident broker.
//! A table is authoritative only when shared by all participating modules.
//! The caller serializes access and quiesces hardware before release.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Resource {
    Pci { bus: u8, device: u8, function: u8 },
    Io { base: u16, len: u32 },
    Irq(u8),
    Dma(u8),
    Vector(u8),
}

impl Resource {
    fn valid(self) -> bool {
        match self {
            Self::Pci { device, function, .. } => device < 32 && function < 8,
            Self::Io { base, len } => len != 0 && len <= 65536 - u32::from(base),
            Self::Irq(n) => n < 16,
            Self::Dma(n) => n < 8 && n != 4,
            Self::Vector(_) => true,
        }
    }

    fn conflicts(self, other: Self) -> bool {
        match (self, other) {
            (Self::Io { base: a, len: al }, Self::Io { base: b, len: bl }) => {
                u32::from(a) < u32::from(b) + bl && u32::from(b) < u32::from(a) + al
            }
            (a, b) => a == b,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Claim { owner: u16, resource: Resource }

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClaimError { Invalid, Busy, Full, NotOwned }

pub struct ClaimTable<const N: usize> {
    slots: [Option<Claim>; N],
}

impl<const N: usize> Default for ClaimTable<N> {
    fn default() -> Self { Self::new() }
}

impl<const N: usize> ClaimTable<N> {
    pub const fn new() -> Self { Self { slots: [None; N] } }

    /// Exclusive claims only. Shared PCI IRQ subscriptions require a separate
    /// dispatcher contract; do not use this method for shared IRQ handlers.
    pub fn claim(&mut self, owner: u16, resource: Resource) -> Result<(), ClaimError> {
        if owner == 0 || !resource.valid() { return Err(ClaimError::Invalid); }
        for entry in self.slots.iter().flatten() {
            if entry.resource.conflicts(resource) {
                return if entry.owner == owner && entry.resource == resource {
                    Ok(())
                } else { Err(ClaimError::Busy) };
            }
        }
        let slot = self.slots.iter_mut().find(|s| s.is_none()).ok_or(ClaimError::Full)?;
        *slot = Some(Claim { owner, resource });
        Ok(())
    }

    /// Release exactly one owned resource after hardware has been quiesced.
    pub fn release(&mut self, owner: u16, resource: Resource) -> Result<(), ClaimError> {
        let slot = self.slots.iter_mut().find(|s| **s == Some(Claim { owner, resource }))
            .ok_or(ClaimError::NotOwned)?;
        *slot = None;
        Ok(())
    }

    pub fn has_claims(&self, owner: u16) -> bool {
        self.slots.iter().flatten().any(|c| c.owner == owner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pci_exclusivity_and_release() {
        let mut t = ClaimTable::<2>::new();
        let p = Resource::Pci { bus: 0, device: 16, function: 0 };
        assert_eq!(t.claim(1, p), Ok(()));
        assert_eq!(t.claim(1, p), Ok(()));
        assert_eq!(t.claim(2, p), Err(ClaimError::Busy));
        assert_eq!(t.release(2, p), Err(ClaimError::NotOwned));
        assert!(t.has_claims(1));
        assert_eq!(t.release(1, p), Ok(()));
        assert!(!t.has_claims(1));
        assert_eq!(t.claim(2, p), Ok(()));
    }
    #[test]
    fn io_overlap_and_adjacency() {
        let mut t = ClaimTable::<3>::new();
        assert_eq!(t.claim(1, Resource::Io { base: 0x300, len: 16 }), Ok(()));
        assert_eq!(t.claim(2, Resource::Io { base: 0x30F, len: 2 }), Err(ClaimError::Busy));
        assert_eq!(t.claim(2, Resource::Io { base: 0x310, len: 16 }), Ok(()));
        assert_eq!(t.claim(2, Resource::Io { base: 0xFFFF, len: 2 }), Err(ClaimError::Invalid));
    }
    #[test]
    fn full_table_fails_closed() {
        let mut t = ClaimTable::<1>::new();
        assert_eq!(t.claim(1, Resource::Vector(0x60)), Ok(()));
        assert_eq!(t.claim(2, Resource::Dma(1)), Err(ClaimError::Full));
        assert_eq!(t.claim(2, Resource::Dma(4)), Err(ClaimError::Invalid));
        assert_eq!(t.release(1, Resource::Vector(0x60)), Ok(()));
        assert_eq!(t.claim(2, Resource::Dma(1)), Ok(()));
    }
}
