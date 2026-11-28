//! Shared ToPIC95 register decoding, slot profile and stable slot-info ABI.
#![no_std]
#![forbid(unsafe_code)]
#![deny(warnings)]

pub mod resources;
pub mod lifecycle;
pub mod config;
pub mod power;
pub mod provider;
pub mod audit;
pub mod authorization;
pub mod dynabook;
pub mod child;

pub const TOPIC95_VID: u16 = 0x1179;
pub const TOPIC95_DID: u16 = 0x0617;
pub const TOPIC95_PCI_DEV: u8 = 17; // 00:11.0 and 00:11.1

// Standard PCI-to-CardBus bridge config offsets (Header Type 0x02).

// PCI Bridge Control bits (offset 0x3E, high 16 bits of 0x3C dword).
pub const CB_BRIDGE_CRST: u16 = 0x0040; // CardBus reset
pub const CB_BRIDGE_INTR: u16 = 0x0080; // 1 = ISA IRQ, 0 = PCI INTA
pub const CB_BRIDGE_PREFETCH0: u16 = 0x0100;
pub const CB_BRIDGE_PREFETCH1: u16 = 0x0200;
pub const CB_BRIDGE_POSTEN: u16 = 0x0400; // ToPIC95 WBEN (topic.h:108)
pub const CB_BRIDGE_ISAEN: u16 = 0x0004;
pub const CB_BRIDGE_VGAEN: u16 = 0x0008;

// Toshiba ToPIC95/97 config registers (topic.h:36-81).

// Standard CardBus Socket MMIO registers (at BAR0) and ExCA (at BAR0 + 0x800).

// CB_SOCKET_STATE / CB_SOCKET_FORCE bits (verified against Linux yenta_socket.h:20-52).
pub const CB_CDETECT1: u32 = 0x0000_0002;
pub const CB_CDETECT2: u32 = 0x0000_0004;
pub const CB_PWRCYCLE: u32 = 0x0000_0008;
pub const CB_CBCARD: u32 = 0x0000_0020;
pub const CB_BADVCCREQ: u32 = 0x0000_0200;
pub const CB_5VCARD: u32 = 0x0000_0400;
pub const CB_3VCARD: u32 = 0x0000_0800;
pub const CB_CVSTEST: u32 = 0x0000_4000;

// CB_SOCKET_CONTROL values (Vcc=3.3V, Vpp=0V).
pub const CB_SC_VCC_3V: u32 = 0x0000_0030;

// DynaBook SS 2000M legacy candidate MMIO assignments, observed working
// with the integrated driver. BAR base disjointness is not proof of
// firmware ownership or complete aperture non-overlap.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SlotResources {
    pub sec_bus: u8,
    pub bar0_phys: u32,
    pub mem0_base: u32,
    pub mem0_limit: u32,
    pub irq_line: u8,
}

pub const fn slot_resources(slot: usize) -> Option<SlotResources> {
    if slot == 0 {
        Some(SlotResources {
            sec_bus: 2,
            bar0_phys: 0xF7E0_0000,
            mem0_base: 0xF7D0_0000,
            mem0_limit: 0xF7DF_F000,
            irq_line: 11,
        })
    } else if slot == 1 {
        Some(SlotResources {
            sec_bus: 3,
            bar0_phys: 0xF7E0_1000,
            mem0_base: 0xF7C0_0000,
            mem0_limit: 0xF7CF_F000,
            irq_line: 11,
        })
    } else { None }
}

/// Compute the CardBus Bridge Control word (`0x3E`) for ToPIC95 bring-up.
/// Keeps MEM0/MEM1 non-prefetchable for register MMIO, routes interrupts to
/// PCI INTA (`CB_BRIDGE_INTR = 0`), clears ISA/VGA forwarding, and disables
/// `WBEN` (`0x0400`) on rev <= 7 per `topic.h:158`.
pub const fn topic95_bridge_control(cur: u16, rev: u8, assert_reset: bool) -> u16 {
    let mut b = cur
        & !(CB_BRIDGE_CRST
            | CB_BRIDGE_INTR
            | CB_BRIDGE_PREFETCH0
            | CB_BRIDGE_PREFETCH1
            | CB_BRIDGE_ISAEN
            | CB_BRIDGE_VGAEN);
    if rev <= 7 {
        b &= !CB_BRIDGE_POSTEN;
    }
    if assert_reset {
        b |= CB_BRIDGE_CRST;
    }
    b
}

/// Decoded CardBus socket state from `CB_SOCKET_STATE` (`BAR0 + 0x08`).
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct SocketDecode {
    pub card_present: bool,
    pub is_cardbus: bool,
    pub supports_3v3: bool,
    pub supports_5v: bool,
}

pub const fn decode_socket_state(state: u32) -> SocketDecode {
    if state == u32::MAX { return SocketDecode { card_present: false, is_cardbus: false, supports_3v3: false, supports_5v: false }; }
    SocketDecode {
        card_present: (state & (CB_CDETECT1 | CB_CDETECT2)) == 0,
        is_cardbus: (state & CB_CBCARD) != 0,
        supports_3v3: (state & CB_3VCARD) != 0,
        supports_5v: (state & CB_5VCARD) != 0,
    }
}

/// Fixed 40-byte `#[repr(C)]` slot summary returned to `CBINIT.COM` via JLM V86 API `AX=0x0009`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct CardBusSlotInfo {
    pub present: u8,
    pub stage: u8,
    pub rev: u8,
    pub sec_bus: u8,
    pub bar0_phys: u32,
    pub mem0_base: u32,
    pub socket_state: u32,
    pub card_present: u8,
    pub is_cardbus: u8,
    pub powered_3v3: u8,
    pub card_rev: u8,
    pub card_vid: u16,
    pub card_did: u16,
    pub card_subsys_vid: u16,
    pub card_subsys_did: u16,
    pub card_class: u32,
    pub card_bar0: u32,
    pub bar0_lin: u32,
}

impl CardBusSlotInfo {
    pub const ZERO: Self = Self {
        present: 0,
        stage: 0,
        rev: 0,
        sec_bus: 0,
        bar0_phys: 0,
        mem0_base: 0,
        socket_state: 0,
        card_present: 0,
        is_cardbus: 0,
        powered_3v3: 0,
        card_rev: 0,
        card_vid: 0,
        card_did: 0,
        card_subsys_vid: 0,
        card_subsys_did: 0,
        card_class: 0,
        card_bar0: 0,
        bar0_lin: 0,
    };
}

const _: () = {
    assert!(core::mem::size_of::<CardBusSlotInfo>() == 40);
};


#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_slot_and_absent_socket() {
        assert!(slot_resources(2).is_none());
        assert!(slot_resources(usize::MAX).is_none());
        assert!(!decode_socket_state(u32::MAX).card_present);
        assert!(!decode_socket_state(u32::MAX).supports_3v3);
    }
    #[test]
    fn known_profile_is_disjoint() {
        let a = slot_resources(0).unwrap();
        let b = slot_resources(1).unwrap();
        assert_ne!(a.sec_bus,b.sec_bus);
        assert!(b.mem0_limit + 4095 < a.mem0_base);
        assert_ne!(a.bar0_phys,b.bar0_phys);
        assert_eq!(core::mem::size_of::<CardBusSlotInfo>(),40);
    }
}
