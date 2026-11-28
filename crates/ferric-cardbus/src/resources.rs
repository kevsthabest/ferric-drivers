//! Pure validation for bridge resource proposals. A valid layout is not
//! evidence that firmware has actually reserved the physical addresses.
use crate::SlotResources;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Range { pub base: u32, pub len: u32 }
impl Range {
    pub fn valid(self) -> bool {
        self.base != 0 && self.len != 0 &&
            u64::from(self.base) + u64::from(self.len) <= 0x1_0000_0000
    }
    pub fn overlaps(self, other: Self) -> bool {
        u64::from(self.base) < u64::from(other.base) + u64::from(other.len) &&
        u64::from(other.base) < u64::from(self.base) + u64::from(self.len)
    }
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LayoutError { Bus, Irq, Alignment, Range, Conflict }

/// CardBus bridge memory limit registers encode the final 4 KiB page.
pub fn validate(slot: SlotResources, occupied: &[Range], buses: &[u8])
    -> Result<[Range; 2], LayoutError>
{
    if slot.sec_bus == 0 || buses.contains(&slot.sec_bus) { return Err(LayoutError::Bus); }
    if !(3..16).contains(&slot.irq_line) { return Err(LayoutError::Irq); }
    if (slot.bar0_phys | slot.mem0_base | slot.mem0_limit) & 0xfff != 0 {
        return Err(LayoutError::Alignment);
    }
    let len = slot.mem0_limit.checked_sub(slot.mem0_base)
        .and_then(|n| n.checked_add(4096)).ok_or(LayoutError::Range)?;
    let socket = Range { base: slot.bar0_phys, len: 4096 };
    let window = Range { base: slot.mem0_base, len };
    if !socket.valid() || !window.valid() { return Err(LayoutError::Range); }
    if socket.overlaps(window) || occupied.iter().any(|r|
        !r.valid() || socket.overlaps(*r) || window.overlaps(*r)) {
        return Err(LayoutError::Conflict);
    }
    Ok([socket, window])
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::slot_resources;
    #[test]
    fn inclusive_limit_and_adjacent_sockets() {
        let a = slot_resources(0).unwrap();
        let b = slot_resources(1).unwrap();
        let ranges = validate(a, &[], &[]).unwrap();
        assert_eq!(ranges[1].len, 0x100000);
        assert!(validate(b, &ranges, &[a.sec_bus]).is_ok());
        assert_eq!(validate(b, &ranges, &[b.sec_bus]), Err(LayoutError::Bus));
        assert_eq!(validate(a, &[Range { base: a.mem0_limit, len: 1 }], &[]), Err(LayoutError::Conflict));
    }
    #[test]
    fn reject_malformed_proposals() {
        let mut s = slot_resources(0).unwrap();
        s.bar0_phys = s.mem0_base;
        assert_eq!(validate(s, &[], &[]), Err(LayoutError::Conflict));
        s.bar0_phys += 1;
        assert_eq!(validate(s, &[], &[]), Err(LayoutError::Alignment));
        s = slot_resources(0).unwrap();
        s.mem0_limit = 0;
        assert_eq!(validate(s, &[], &[]), Err(LayoutError::Range));
        s = slot_resources(0).unwrap();
        s.irq_line = 255;
        assert_eq!(validate(s, &[], &[]), Err(LayoutError::Irq));
    }
}
