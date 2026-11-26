# Optional DynaBook audio with VSBHDA

This is an experimental 32-bit external driver for the ALi M5451 (PCI 10B9:5451).
After VSBHDA checks for a resident Sound Blaster, ALI5451.DRV asks FERRIC.JLM (AX=002Bh) to initialize the codec, amplifier and PCI
route. ALI5451.DRV then supplies a signed 16-bit stereo DMA ring on voice 0,
acknowledges the hardware interrupt and exposes the AC'97 mixer. VSBHDA supplies
the game's DSP, DMA, virtual IRQ and OPL emulation. The game sees IRQ 5 and the
PCI device uses IRQ 11. Each hardware half-ring is one /PS period in bytes.

## Build on Windows

From the Ferric repo in PowerShell, using the project's ferric-dos-build:gate0
image, run the following. The two dependency directories below must be fresh.

```powershell
$vs = Join-Path $env:TEMP vsbhda-d141496
$asm = Join-Path $env:TEMP jwasm-b6c61b3
git clone https://github.com/Baron-von-Riedesel/VSBHDA $vs
git -C $vs checkout --detach d14149613d26caa5680d65748c6b8ed0283a6d12
git clone https://github.com/Baron-von-Riedesel/JWasm $asm
git -C $asm checkout --detach b6c61b357e01d9068606db6b0f38d84132b7bce8
$drop = (New-Item -ItemType Directory -Force out/vsbhda).FullName
docker run --rm -v "${vs}:/vsbhda:ro" -v "${asm}:/jwasm:ro" -v "${PWD}:/src:ro" -v "${drop}:/drop" ferric-dos-build:gate0 sh /src/drivers/vsbhda/build.sh /drop
```

## Manual trial

Use a **copy** of the boot media. On that copy place the generated FERRIC.JLM
where Menu 1 loads it and put the other four files in C:\VSB. Boot Menu 1,
which supplies JemmEx, FERRIC.JLM and HDPMI32i. Start from a fresh boot with
AUDPROBE /SB inactive. The old JLM lacks the preparation call. Menu 2 starts
Ferric's own sound emulation automatically.

```bat
CD \VSB
C:\JLOAD.EXE FERRIC.JLM
C:\JLOAD.EXE QPIEMU.DLL
LH HDPMI32I.EXE -r
SET BLASTER=A220 I5 D1 H5 T6 P330
VSBHDA /DF:ALIDET.DRV /B2 /PS2048 /F48000 /OPL1 /SF:C:\SF2\MT32.SF2
```

For Sound Blaster Pro (`T4`) / 22 kHz fallback without a SoundFont, use:
```bat
SET BLASTER=A220 I5 D1 T4
VSBHDA /DF:ALIDET.DRV /B2 /PS1024 /OPL1
```

`MIXER.COM` works live alongside VSBHDA (controlling the ALi M5451 AC'97 hardware volume via `FERRIC.JLM` verb `0x002A` and VSBHDA's SB Master / Voice / FM mixer via ports `0x224`/`0x225`).

On a fresh boot, launch VSBHDA directly. Running VSBPREP first makes the
DynaBook answer the upstream DSP reset check and aborts installation.
Check that VSBHDA reports the ALI5451 and protected-mode and real-mode support.
For Transport Tycoon Deluxe, run its SOUNDRV.COM detection, then try digital
effects alone at 220/5/1 with four channels, followed by effects plus AdLib FM.
For Doom, listen for a single pistol shot decaying fully while music continues.
A repeating last fragment or loss of FM is a failed playback test. Reboot from
the regular media to return to Ferric audio.

Upstream documents Jemm 5.84+ with QPIEMU for real-mode traps and HDPMI32i
3.21+ for protected-mode games. This build uses the external module ABI in
[VSBHDA 2.1 source](https://github.com/Baron-von-Riedesel/VSBHDA/tree/d14149613d26caa5680d65748c6b8ed0283a6d12).
