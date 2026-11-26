# USB — OHCI host stack + EHCI (USB 2.0)

Two halves: a full OHCI (USB 1.1) host stack resident in the core,
and an EHCI (USB 2.0) implementation split between a portable crate
and a seam in the core. On the development machine both halves
serve the NEC uPD720100A cluster (two OHCI functions + one EHCI
function), including the awkward part: the EHCI function starts
BIOS-disabled with no BAR assigned, and the second OHCI is its
companion for full/low-speed devices.

## Licence — read this first

The OHCI core is a derivative of **USBDDOS** and is
**GPL-2.0-only**, whatever the workspace metadata says. See
`NOTICES.md` for the full statement.

## OHCI stack — `crates/gate1-svc/src/ohci.rs`

Host-controller driver, root hub and external-hub enumeration, and
two device classes:

- **HID** — keyboards and mice; the USB mouse uses a ping-pong
  2×4-TD hardware FIFO with OHCI `WritebackDoneHead` draining so
  125 Hz wireless mice stay stall-free and capture full motion
  between 35 Hz game polls, feeding the core's `INT 33h` mouse
  (verb `0x001C` reads and clears the accumulated scroll-wheel delta).
- **Mass storage** — Bulk-Only Transport / SCSI. Its sector
  functions (`gate1_blk_*`) are the core's block-service table,
  which is what the filesystem modules read and write through.
  A small read-only FAT implementation (`src/fat_ro.rs`) backs the
  directory/open verbs.

Verbs: `0x0010` (init), `0x0011` (shutdown), `0x0012`–`0x0015`
(MSC directory/open/read/BPB), `0x0016` (read sector), `0x0017`
(write sector), `0x0018` (flush), `0x0019` (eject), `0x001A`
(media check), `0x001B` (usbdelay multiplier for slow flash
drives), `0x001C` (wheel delta).

Clients (in `crates/cbprobe`): **USBPROBE.COM**, **USBDRV.SYS**
(a DOS block device over the MSC verbs), **USBCP.COM**,
**USBEJECT.COM**.

## EHCI — `crates/ferric-ehci` + `crates/gate1-svc/src/ehci.rs`

- **`ferric-ehci`** is a portable, host-testable protocol core
  (capability decoding, queue/qTD construction, port-routing
  decisions) written from Intel's EHCI Specification r1.1 — no
  code taken from any existing implementation.
- The **seam** in the core does the hardware work: PCI discovery
  by class code, BAR sizing and assignment when the BIOS left the
  function disabled, the USBLEGSUP BIOS handoff, schedule
  bring-up, port power/reset, and per-port owner decisions
  (EHCI vs companion OHCI). Verbs `0x0036` (recon), `0x0037`
  (bring-up), `0x0038` (clean shutdown, companions take the ports
  back). Client: **EHCPROBE.COM**.

Because discovery is by PCI class code rather than a hardwired
device ID, the EHCI code is among the most portable in this
repository.

## Evidence

OHCI/HID/MSC passed the Gate 2 hardware legs; EHCI passed Stage H
on the DynaBook, including a 480 Mbps mount of a USB 2.0 exFAT
drive and a clean shutdown handing ports back to the companion.
See
[`docs/hardware/dynabook-ss2000m-ehci.txt`](../hardware/dynabook-ss2000m-ehci.txt)
and the
[support-matrix excerpt](../hardware/SUPPORT-MATRIX-EXCERPT.md).
