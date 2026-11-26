# Ferric Drivers

Device drivers for DOS, written in freestanding Rust, from the Ferric
DOS project. The drivers are **Jemm Loadable Modules (JLMs)**: they
load with JLOAD under JemmEx/Jemm386 and run in Ring 0 alongside one
foreground DOS program, serving DOS clients through a V86 verb API.

This repository publishes the driver set on its own. The drivers were
developed as part of a larger DOS platform and keep that shape: one
resident core module hosts several drivers, and a few satellite
modules plug into the core. Each driver set has its own section under
[`docs/drivers/`](docs/drivers/).

## Prerequisites

- **FreeDOS** (or a compatible DOS) as the host system.
- **Jemm** — JemmEx or Jemm386 plus **JLOAD**, Jemm's module loader.
  JLM is Jemm's own module format; nothing here loads under EMM386 or
  QEMM. Jemm is an external prerequisite and is not included in this
  repository.
- **HDPMI32i** where a driver path needs the DPMI host (the parent
  project's resident configuration uses it; also an external
  prerequisite, not included).
- For building: the pinned Rust nightly and container described in
  [`docs/BUILDING.md`](docs/BUILDING.md). A plain `cargo build` does
  **not** produce a usable module.

## Driver index

| Section | Module(s) | What it drives |
| --- | --- | --- |
| [Core](docs/drivers/core.md) | `FERRIC.JLM` (DDB `0xFD01`) | The resident core: verb dispatch, the block-service table, the shared packet-driver facade, and the host for the Ethernet, Wi-Fi, USB, and audio drivers below |
| [Ethernet](docs/drivers/ethernet.md) | in the core | Intel 82550 Fast Ethernet behind a Crynwr-style packet-driver interface (`INT 60h`) |
| [Wi-Fi](docs/drivers/wifi.md) | in the core + `wifi-core` crate | Toshiba ToPIC95-family CardBus bridge + Atheros AR2413 (D-Link DWL-G630) with a WPA2-PSK supplicant |
| [USB](docs/drivers/usb.md) | in the core + `ferric-ehci` crate | OHCI host controller with HID and mass-storage classes; EHCI (USB 2.0) bring-up and companion routing |
| [Filesystems](docs/drivers/filesystems.md) | `EXFAT.JLM` (`0xFD02`), `NTFS.JLM` (`0xFD03`) | exFAT (read/write, with an INT 2Fh redirector) and NTFS (read-only) on top of the core's block services |
| [VBE](docs/drivers/vbe.md) | `VBE.JLM` (`0xFD04`) | VBE linear-framebuffer 2D acceleration: a generic software engine plus Trident CyberBlade XP hardware acceleration |
| [Audio](docs/drivers/audio.md) | in the core + [`drivers/vsbhda/`](drivers/vsbhda/) | ALi M5451 AC'97 with built-in Sound Blaster Pro / OPL / SoundFont support, plus a standalone 32-bit VSBHDA backend (`ALI5451.DRV`) |
| [Power Management](docs/drivers/pmu.md) | `PMU.JLM` (`0xFD05`) | Standalone ALi M7101 PMU driver: Smart Battery telemetry (SMBus), CPU clock throttling (`P_CNTRL`), and LCD CCFT backlight PWM (`PWRCTL.COM`) |
| [Toshiba SD Slot](docs/drivers/sd.md) | `TOSHSD.JLM` (`0xFD06`) | Standalone read-only Toshiba TC6371AF (`1179:0805` / `1179:0803`) built-in SD card slot driver (`SDPROBE.COM`) |

Hardware evidence — what was proven on the development machine, and
the limits that come with it — is in
[`docs/hardware/`](docs/hardware/).

## Hardware scope — read this first

These are drivers for **specific chips**, developed and proven on one
machine (a Toshiba DynaBook SS 2000M, Pentium III-M). They are not
drop-in drivers for arbitrary FreeDOS + Jemm machines:

- **Ethernet** supports the **Intel 82550 only** (`8086:1229`).
- **Wi-Fi** supports the **Toshiba ToPIC95-family CardBus bridge**
  (`1179:0617`) with an **Atheros AR2413** card (developed against the
  D-Link DWL-G630 Rev C2, `168C:001A`), 2.4 GHz, **WPA2-PSK only**.
- **VBE acceleration** is **Trident CyberBlade XP-specific**; the
  software 2D engine underneath it is generic VBE.
- **EXFAT.JLM / NTFS.JLM** are standalone modules, but they read and
  write sectors through **FERRIC.JLM's block services**, so the core
  must be loaded first.
- The most portable pieces are the **OHCI/EHCI** code (PCI class-code
  driven) and the standalone crates **`wifi-core`** (802.11 frames +
  WPA2 key hierarchy, no hardware dependencies) and **`ferric-ehci`**
  (EHCI protocol core), both host-tested against published vectors.

## Licence

The project licence is **GPL-2.0-or-later** (see [`LICENSE`](LICENSE)),
with one explicit exception: the **USB core is GPL-2.0-only**, because
it is a derivative of the GPL-2.0-only USBDDOS. Third-party provenance
— including the ISC notices for the ath5k-derived Wi-Fi code — is
recorded in [`NOTICES.md`](NOTICES.md). Read it before redistributing;
the per-driver pages flag their own provenance where it differs.
