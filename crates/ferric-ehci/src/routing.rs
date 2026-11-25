//! Port-routing policy: for each downstream port, decide whether the
//! EHCI or a companion OHCI owns it.
//!
//! The rule the hardware imposes is simple — the EHCI can only serve
//! high-speed devices; anything full/low-speed must be handed to a
//! companion. The complication the DynaBook adds is that companions
//! can be *disabled*: OHCI #2 ships BIOS-disabled, so a port whose
//! companion is disabled cannot be handed over without stranding the
//! device on it. This module computes the whole routing table up
//! front, from the capability parameters and the set of companions
//! the seam has actually enabled, so the bring-up enables companions
//! before it ever sets CONFIGFLAG — and can refuse to claim a port
//! whose companion story is unresolved instead of discovering the
//! problem with a dead keyboard.

use crate::caps::HcsParams;

/// Which controller should own a port once the device on it is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortOwner {
    /// The EHCI serves this port (high-speed device, or no device —
    /// an empty port stays with the EHCI so a high-speed hot-plug is
    /// seen immediately).
    Ehci,
    /// The companion OHCI with this index (0-based, in PCI function
    /// order) serves this port.
    Companion(u8),
    /// A full/low-speed device is present but its companion is not
    /// enabled: the port cannot be served. The bring-up must enable
    /// the companion (or leave CONFIGFLAG clear entirely) rather than
    /// claim this port — see the module docs.
    Stranded,
}

/// Device speed as the EHCI can observe it around a port reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceSpeed {
    /// Nothing connected.
    Absent,
    /// Connected; after reset the port enabled itself, which only a
    /// high-speed device achieves (it chirps during the reset).
    High,
    /// Connected but the port did not enable on reset: a full- or
    /// low-speed device. The line state (J vs K) distinguishes the
    /// two for the companion's benefit, but the routing decision is
    /// the same, so they are one case here.
    FullOrLow,
}

/// Which companion serves a port, per HCSPARAMS. With routing rule 1
/// the ports divide evenly: companion c takes ports
/// [c * N_PCC, (c + 1) * N_PCC). With rule 0 the HCSP_PORTROUTE array
/// assigns each port individually; the seam reads that array from the
/// capability block and passes the per-port companion in directly, so
/// this function covers the rule-1 arithmetic the uPD720100A uses.
pub fn companion_for_port(params: &HcsParams, port: u8) -> Option<u8> {
    let per = params.n_ports_per_companion();
    if per == 0 || params.n_companions() == 0 || port >= params.n_ports() {
        return None;
    }
    let idx = port / per;
    if idx < params.n_companions() {
        Some(idx)
    } else {
        None
    }
}

/// Decide one port's owner.
///
/// `companion_enabled` is indexed by companion number and reports
/// what the seam has actually brought up (BAR assigned, controller
/// running) — not what exists on the die. That distinction is the
/// whole point: routing to a disabled companion strands the device.
pub fn decide_owner(
    params: &HcsParams,
    port: u8,
    speed: DeviceSpeed,
    companion_enabled: &[bool],
) -> PortOwner {
    match speed {
        DeviceSpeed::Absent | DeviceSpeed::High => PortOwner::Ehci,
        DeviceSpeed::FullOrLow => match companion_for_port(params, port) {
            Some(c) if companion_enabled.get(c as usize).copied().unwrap_or(false) => {
                PortOwner::Companion(c)
            }
            _ => PortOwner::Stranded,
        },
    }
}

/// Can CONFIGFLAG be set safely at all? Only when every port has an
/// enabled companion to fall back to — otherwise claiming the routing
/// would strand any full/low-speed device plugged into an uncovered
/// port later, including devices hot-plugged after bring-up. When
/// this returns false the correct behaviour is today's behaviour:
/// leave the companions owning everything.
pub fn configflag_safe(params: &HcsParams, companion_enabled: &[bool]) -> bool {
    if params.n_companions() == 0 {
        // No companions exist on this controller: nothing can be
        // stranded by claiming the ports (full/low-speed devices
        // simply will not work, as before Stage H existed — hubs
        // with transaction translators are out of scope).
        return true;
    }
    (0..params.n_ports()).all(|port| match companion_for_port(params, port) {
        Some(c) => companion_enabled.get(c as usize).copied().unwrap_or(false),
        None => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Real DynaBook NEC uPD720100A shape (0x00002395):
    // 5 ports, 2 companions, 3 ports per companion (0..=2 -> c0, 3..=4 -> c1).
    fn dynabook() -> HcsParams {
        HcsParams::from_raw(0x0000_2395)
    }

    #[test]
    fn dynabook_companion_mapping() {
        let p = dynabook();
        assert_eq!(companion_for_port(&p, 0), Some(0));
        assert_eq!(companion_for_port(&p, 2), Some(0));
        assert_eq!(companion_for_port(&p, 3), Some(1));
        assert_eq!(companion_for_port(&p, 4), Some(1));
        assert_eq!(companion_for_port(&p, 5), None);
    }

    #[test]
    fn ich9_companion_mapping() {
        // 6 ports, 2 companions, 3 ports each (QEMU ich9 shape).
        let p = HcsParams::from_raw(0x0000_2306);
        assert_eq!(companion_for_port(&p, 0), Some(0));
        assert_eq!(companion_for_port(&p, 2), Some(0));
        assert_eq!(companion_for_port(&p, 3), Some(1));
        assert_eq!(companion_for_port(&p, 5), Some(1));
    }

    #[test]
    fn high_speed_and_empty_ports_stay_with_ehci() {
        let p = dynabook();
        assert_eq!(
            decide_owner(&p, 0, DeviceSpeed::High, &[true, true]),
            PortOwner::Ehci
        );
        assert_eq!(
            decide_owner(&p, 3, DeviceSpeed::Absent, &[true, true]),
            PortOwner::Ehci
        );
    }

    #[test]
    fn full_speed_device_goes_to_its_companion() {
        let p = dynabook();
        assert_eq!(
            decide_owner(&p, 0, DeviceSpeed::FullOrLow, &[true, true]),
            PortOwner::Companion(0)
        );
        assert_eq!(
            decide_owner(&p, 3, DeviceSpeed::FullOrLow, &[true, true]),
            PortOwner::Companion(1)
        );
    }

    #[test]
    fn disabled_companion_strands_instead_of_misrouting() {
        // The DynaBook at boot: OHCI #1 enabled (ports 0..=2),
        // OHCI #2 disabled (ports 3..=4). A full-speed device on
        // port 3 must be reported stranded — never silently routed
        // to companion 0 (wrong hardware) and never claimed by EHCI.
        let p = dynabook();
        assert_eq!(
            decide_owner(&p, 3, DeviceSpeed::FullOrLow, &[true, false]),
            PortOwner::Stranded
        );
        // And CONFIGFLAG must stay clear until companion 1 is up.
        assert!(!configflag_safe(&p, &[true, false]));
        assert!(configflag_safe(&p, &[true, true]));
    }

    #[test]
    fn companionless_controller_may_claim() {
        let p = HcsParams::from_raw(0x0000_0004);
        assert_eq!(p.n_companions(), 0);
        assert!(configflag_safe(&p, &[]));
    }
}
