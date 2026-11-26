# Audio — ALi M5451, Sound Blaster / OPL, SoundFont

Audio support resident in the core JLM, for the **ALi M5451**
AC'97 controller (`10B9:5451`). This page records the facts —
what the code is, where its register knowledge came from, and how
clients reach it. It makes no readiness claims beyond pointing at
the hardware evidence in the
[support-matrix excerpt](../hardware/SUPPORT-MATRIX-EXCERPT.md).

## Pieces

- **`crates/gate1-svc/src/ali5451.rs`** — the M5451 AC'97 backend:
  controller bring-up, voice playback, mixer. Verbs `0x0020`
  (init), `0x0021` (chime test), `0x0022` (status), `0x0023`
  (stop/mute), `0x0025` (volume), `0x0029` (IRQ service), `0x002A`
  (mixer levels), `0x002B` (external driver codec + PCI IRQ prepare).
- **`drivers/vsbhda/`** — standalone 32-bit external backend
  (`ALI5451.C` -> `ALI5451.DRV` / `ALIDET.DRV`) for upstream
  [VSBHDA](https://github.com/Baron-von-Riedesel/VSBHDA) v2.1. Calls
  `FERRIC.JLM` verb `0x002B` during detection to bring up the ALi
  M5451 codec, amplifier, and PCI IRQ 11 route, then hands voice 0
  DMA and AC'97 mixer control directly to VSBHDA (supporting SB16,
  48 kHz mixing, OPL3 FM, and MPU-401 TinySoundFont `.SF2` playback).
  See [`drivers/vsbhda/README.md`](../../drivers/vsbhda/README.md) for
  build and usage instructions.
- **`crates/gate1-svc/src/sb_opl.rs`** — Sound Blaster Pro
  (`0x220`) and AdLib/OPL2/OPL3 emulation presented to DOS
  programs through I/O traps, plus MPU-401 support. Verbs
  `0x0024` (start resident SB/OPL), `0x0026` (PM32 I/O trap
  bridge), `0x0027`/`0x0028` (diagnostics).
- **`crates/gate1-svc/src/sf2.rs`** — a SoundFont (`.SF2`) loader
  that stages banks in XMS memory. Verbs `0x0040`–`0x0044`
  (begin load, copy, parse/activate, unload, status).

Clients (in `crates/cbprobe`): **AUDPROBE.COM**, **MIXER.COM**,
**SF2LOAD.COM**.

## Provenance

Facts only, per the code's own headers and `NOTICES.md`:

- `ali5451.rs` register definitions and reset sequences were
  verified against the Linux `ali5451.c` driver (GPL-2.0).
- SBEMU (GPL-2.0-only) is a listed design reference for the Sound
  Blaster emulation work.
- `sb_opl.rs` presents its synthesizer as an original
  implementation.
