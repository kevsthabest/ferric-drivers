# Hardware Support Matrix — Excerpt

This is an excerpt of the Ferric DOS project's hardware support
matrix, limited to the machines and devices the drivers in this
repository cover. It records what was actually proven on physical
hardware; emulator results qualify virtual devices only and never
substitute for physical hardware qualification.

Redacted for publication: the unit serial and the station MAC
addresses were replaced with placeholders, and LAN addresses were
moved to a placeholder subnet. The full matrix lives in the parent
Ferric DOS project.

## Physical Machines

| Machine | Tier | Firmware / Boot Path | Profile | Feature / Test | Status | Evidence |
| --- | --- | --- | --- | --- | --- | --- |
| **Toshiba DynaBook SS 2000M** (Pentium III-M 750 MHz CPUID `06B1`, 256 MB SDRAM) | Flagship | PXE (`memdisk floppy raw`, commit `f628dfa` / `369babc`) | Gate 2 Connected (`Option 2`: `FERRIC.JLM` + `APMSTAT` + `MTEST` + `NETINIT`) | Gate 2.1 `APMSTAT.COM` (APM 1.2), Gate 2.2 PS/2 Mouse (`INT 33h` + IRQ 12), Gate 2.3 Intel 82550 Ethernet (`8086:1229` rev `0D`, MAC `<redacted>`, IRQ 11, mTCP DHCP + FTPSRV file transfers) | **PASS (Functional)** — Gate 2.1–2.3 passed on silicon 21 Nov 2026 | [`dynabook-ss2000m-eth.txt`](dynabook-ss2000m-eth.txt): `stage=6 io=EEC0 irq=11 link=1 macsrc=1 irqcnt=19`, live DHCP lease + FTP transfers confirmed |
| **Toshiba DynaBook SS 2000M** | Flagship | PXE (`memdisk floppy raw`, commit `30094b0`) | Gate 4b.3/4b.4a (`CBINIT` + `WPROBE`) | Toshiba ToPIC95/100 CardBus bridge (`1179:0617` rev `0x32`, 3.3 V power + reset + bus 2 enum) + D-Link DWL-G630 Rev C2 (`168C:001A` subsys `1186:3B08`, AR2413 `srev=0x78`, `phy=0x45`, EEPROM v5.1 `0x5AA5`, MAC `<redacted>`, `aes_hw=1`) | **PASS (Functional)** — Gate 4b.3 & 4b.4a passed on silicon 21 Nov 2026 | [`dynabook-ss2000m-cardbus.txt`](dynabook-ss2000m-cardbus.txt): `state=0x30000820 card=1 cb32=1 pwr3v=1`, `id=168C:001A`, `aes_hw=1` |
| **Toshiba DynaBook SS 2000M** | Flagship | PXE (`memdisk floppy raw`, commit `af1a0a5`) | Gate 4b.4b/4b.5/4b.6 (`WSCAN` + `WCONN` + `INT 60h` Wi-Fi) | Passive 2.4 GHz scan, WPA2-PSK 4-Way Handshake, AR2413 EEPROM 11g RF calibration + HW AES-CCM RX + SW IEEE 802.11i AES-CCM TX, `INT 1Ch` RX polling, live mTCP `DHCP` (`192.168.0.215`), DNS, `PING`, and `FTPSRV` file transfer over Wi-Fi with Ethernet unplugged | **PASS (Functional)** — Gate 4b.4b–4b.6 passed on silicon 22 Nov 2026 | [`dynabook-ss2000m-wifi.txt`](dynabook-ss2000m-wifi.txt): `TX: 22/22 ok rets=0`, `RX: 140/140 ok` (0 errors), live DHCP lease + FTP transfer over Wi-Fi |
| **Toshiba DynaBook SS 2000M** | Flagship | PXE / HDD (`c15f925` / `fd70243` / `f4ec9aa`) | Gate 4a Audio (`AUDPROBE /SB` + `MIXER` + `SF2LOAD`) | ALi M5451 AC'97 (`10B9:5451`, 86 Hz HW IRQ 11), SB Pro (`0x220` I5 D1), dual-bank OPL3 (`0x388..0x38B`), MPU-401 General MIDI (`0x330`) 31-voice HW wavetable; DOOM (SFX + General MIDI) and Transport Tycoon Deluxe 2.01.119 (DIGPAK) confirmed on silicon; XMS 3.0 `.SF2` loader (`SF2LOAD.COM`, `a2367e7`) pending silicon test | **PASS (Functional)** — Gate 4a passed on silicon 24 Nov 2026 | `dynabook-ss2000m-audio.txt` (parent project); DOOM General MIDI (`0x330`) & TTDX DIGPAK confirmed live on silicon |
| **Toshiba DynaBook SS 2000M** | Flagship | PXE / HDD (`d685197`) | Stage G Phase 1 (`VBE.JLM` + `VBEPROBE` + `VBECTL`) | Standalone `VBE.JLM` (`0xFD04`), VBE 2.0+ mode enumeration, 800×600 & 1024×768 LFB switch on Trident CyberBlade XP (`1023:8820`) | **PASS (Functional)** — Stage G Phase 1 passed on silicon 24 Nov 2026 | `dynabook-ss2000m-vbe.txt` (parent project) |
| **Toshiba DynaBook SS 2000M** | Flagship | PXE / HDD (`82de659` / `fd70243` / `f4ec9aa`) | Stage H USB 2.0 EHCI (`EHCPROBE /B` + `/U` + `FSLOAD`) | NEC uPD720100A EHCI (`00:0C.2`, `BAR0=0xFE000000`, `HCIVERSION=0x0095`, `PPC=1`) + OHCI #2 companion (`00:0C.1`, `BAR0=0xFE001000`), 61.87 GB USB 2.0 exFAT drive mounted at 480 Mbps on port 2, port 3 routed cleanly to `companion1`, clean `/U` shutdown | **PASS (Functional)** — Stage H Steps 1–5 passed on silicon 24 Nov 2026 | [`dynabook-ss2000m-ehci.txt`](dynabook-ss2000m-ehci.txt): 0 stranded ports, `120,845,300` sectors exFAT over EHCI |

### DynaBook SS 2000M Device Inventory (from Gate 0 `HWSCAN.COM`)

| Device | Bus / ID | State at DOS Boot | Status | Notes |
| --- | --- | --- | --- | --- |
| Intel 82550 10/100 Ethernet | `00:0A.0` (`8086:1229` rev `0D`, BAR0 `0xF7EFF000`, **IRQ 11**) | BIOS-enabled | Functional (Gate 2.3) | Verified on real silicon (`f628dfa`): MAC `<redacted>`, link up, IRQ 11 EOI/claim, live mTCP DHCP + FTPSRV transfers |
| NEC uPD720100A OHCI #1 | `00:0C.0` (`1033:0035` rev `41`, BAR0 `0xF7EBF000`, **IRQ 11**) | BIOS-enabled | Detected | Shared IRQ 11 with Ethernet + VGA; Gate 2 primary USB controller |
| Trident CyberBlade XP-Ai1 AGP | `01:00.0` (`1023:8820` rev `82`, BAR0 `0xFC000000`, **IRQ 11**) | BIOS-enabled | Functional (VGA console + VBE/PM 1.0) | Shared IRQ 11; `PWRPROBE` confirmed VBE/PM 1.0 (`pm_ax=0x004F pm_bx=0x0710`, `STANDBY,SUSPEND,OFF`) when called with `ES:DI=0`, `CX=0` |
| ALi M5451 AC'97 Audio | `00:06.0` (`10B9:5451` rev `01`, subsys `1179:0004`, BAR0 `0x00000001`, IRQ `255`, `INTA`) | BIOS-disabled (`cmd=0x0000`, `cfg44=0xE20A0000`) | Functional (Gate 4a) | Native Rust M5451 backend (`ali5451.rs` + `sb_opl.rs` + `sf2.rs`): `$PIR` IRQ 11 routing (86 Hz), SB Pro, dual-bank OPL3, MPU-401 31-voice HW wavetable + XMS 3.0 `.SF2` loader |
| NEC OHCI #2 & EHCI | `00:0C.1` (`1033:0035`), `00:0C.2` (`1033:00E0` rev `02`), IRQ `255` | BIOS-disabled | Functional (Stage H) | Brought up by `EHCPROBE /B` (`0xFE000000` EHCI + `0xFE001000` OHCI #2 companion); 480 Mbps USB 2.0 MSC verified (`EHC7.TXT`), clean `/U` shutdown |
| Dual Toshiba ToPIC CardBus | `00:11.0`, `00:11.1` (`1179:0617` rev `32`, IRQ `255`) | BIOS-disabled | Functional (Gate 4b.3) | Brought up by `CBINIT` (`BAR0 0xF7E00000`/`0xF7E01000`, `MEM0 0xF7D00000`/`0xF7C00000`, IRQ 11); powers & enumerates D-Link DWL-G630 C2 (`168C:001A`, AR2413 `srev=0x78`, `aes_hw=1`) |
