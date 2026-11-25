# Core — FERRIC.JLM

`FERRIC.JLM` is the resident core module (Jemm DDB device `0xFD01`,
name `FERRIC`). It is built from `crates/gate1-svc` and must be
loaded before any satellite module. Most of the drivers in this
repository are not separate modules — they are hosted inside the
core, because they share its interrupt, DMA, and client plumbing.

## What the core provides

- **Verb dispatch.** DOS clients find the core through Jemm's
  `INT 2Fh AX=1684h` device-API entry and call AX-numbered verbs
  (`0x0003`–`0x0044`, routed in `asm/jlm_stub.asm`). Each driver page
  lists its own verbs; the stub's comments are the authoritative
  map.
- **The block-service table.** The core exports a six-entry service
  table through its DDB, which is the binary contract the
  filesystem modules link against (versioned in
  `crates/abi/src/fs_abi.rs`):
  0. `abi_version`
  1. `info`
  2. `read_sec`
  3. `write_sec`
  4. `flush`
  5. `media_check`

  The implementation lives in the USB mass-storage code
  (`gate1_blk_*` in `src/ohci.rs`): today the block device behind
  the table is a USB stick, and the filesystem modules inherit
  whatever transport the core provides.
- **The packet-driver facade.** One shared front end
  (`src/lib.rs`, roughly lines 3248–4721) presents a Crynwr-style
  packet driver at `INT 60h` with two switchable backends — the
  Intel 82550 Ethernet and the AR2413 Wi-Fi — so DOS network stacks
  (mTCP and friends) see a single adapter. Verb `0x000E` switches
  the active backend without rebooting.
- **Shared machine plumbing.** A shared IRQ 11 chain (Ethernet,
  OHCI, VGA, and audio all hang off it on the development machine),
  VDMAD-validated DMA buffers below 1 MiB, and the INT 08h/INT 1Ch
  polling hooks the USB and Wi-Fi receive paths run from.

## Hosted drivers

Ethernet, Wi-Fi, USB (OHCI and the EHCI seam), and audio all live
in the core; see their own pages. The core also carries smaller
resident pieces that have no page of their own — an ALi M7101
power-management unit driver (verbs `0x0039`–`0x003B`) and a
Toshiba TC6371AF SD-slot reader (verbs `0x003C`–`0x003E`) — with
their sources in `src/m7101.rs` and `src/toshsd.rs`.

## Clients

The core's own diagnostic client is `SVCTEST.COM` (built by
gate1-svc's build script); the per-driver `.COM` clients live in
`crates/cbprobe` and are listed on each driver's page.
