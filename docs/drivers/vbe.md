# VBE — VBE.JLM

A standalone JLM (`crates/ferric-vbe`, DDB device `0xFD04`) that
gives DOS programs a VBE linear framebuffer with 2D acceleration.
It is the most self-contained module in this repository: its only
dependency is `crates/abi` — zero dependency on the core — and it
needs nothing at runtime beyond Jemm itself. The versioned client
contract is `crates/abi/src/vbe_abi.rs`.

## Two engines, one interface

- **Software engine** (`src/sw_2d.rs`) — an original, generic
  linear-framebuffer implementation of `FillRect`, `BitBlt`,
  `GetCaps`, `SetLfb`, and `Sync`. It works against any VBE 2.0+
  mode with a linear framebuffer.
- **Trident hardware engine** (`src/trident_2d.rs`) — register-level
  acceleration for the **Trident CyberBlade XP-Ai1** (`1023:8820`)
  Graphics Engine, dispatched behind the same interface with
  automatic fallback to the software engine. This part is
  chip-specific by nature; the register definitions were verified
  from the Linux tridentfb driver and the X.Org xf86-video-trident
  driver (see `NOTICES.md`).

## Clients

In `crates/cbprobe`: **VBEPROBE.COM** (mode enumeration and
diagnostics) and **VBECTL.COM** (mode setting / control).

## Evidence

Stage G Phase 1 passed on the DynaBook: VBE 2.0+ mode enumeration
and 800×600 / 1024×768 LFB mode switches on the CyberBlade XP.
See the
[support-matrix excerpt](../hardware/SUPPORT-MATRIX-EXCERPT.md).
