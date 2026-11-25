//! Portable EHCI (USB 2.0) host-controller core for Stage H.
//!
//! This crate is the hardware-independent half of the EHCI driver: the
//! register model, capability decoding, queue structures, and the
//! port-routing policy, all as pure logic that host tests can pin down
//! before any of it runs on the DynaBook. The JLM seam that pokes real
//! registers lives in `gate1-svc` (`ehci.rs`) and drives this core
//! through the register-access trait in [`regs`].
//!
//! Written from Intel's *EHCI Specification for Universal Serial Bus*,
//! revision 1.1. Register offsets and structure layouts are spec facts;
//! no code is taken from any existing implementation.
//!
//! The target controller is the NEC uPD720100A EHCI function on the
//! DynaBook (`1033:00E0` at `00:0C.2`, BIOS-disabled), with its two OHCI
//! companions (`1033:0035` at `00:0C.0` / `00:0C.1`).

#![cfg_attr(not(test), no_std)]
#![deny(warnings)]

pub mod caps;
pub mod queue;
pub mod regs;
pub mod routing;

// No panic handler here, deliberately: this is a library crate (the
// wifi-core pattern), and the binary crate that links it — gate1-svc
// today — owns the one panic handler for its image. Defining one
// here as well makes every consumer fail to link with a duplicate
// lang item.
