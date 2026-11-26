# Toshiba Built-in SD Slot — TC6371AF (`TOSHSD.JLM`)

Standalone read-only Jemm Loadable Module (`TOSHSD.JLM`, DDB `0xFD06`) for the **Toshiba TC6371AF / Type-A PCI SD Host Controller** (`1179:0805` / `1179:0803`, polled PIO mode, zero IRQ 11 contention).

No dependency on `FERRIC.JLM` — load it on its own under JemmEx/Jemm386:

```bat
JLOAD.EXE TOSHSD.JLM
SDPROBE
SDPROBE /R 0
SDPROBE /U
```

## Capabilities & Verbs

- `AX=0x0000` — Ping / version (`AX=0x0001`, `BX=0xFD06`).
- `AX=0x003C` — Initialize controller, power SD slot (3.3 V), identify SDSC/SDHC card (CID/CSD), and return summary text.
- `AX=0x003D` — Read one 512-byte sector (`EBX` = LBA, `ES:DI` = 512-byte buffer). Strictly read-only (`CMD17` only).
- `AX=0x003E` — Power down SD slot and stop clocks cleanly (also runs automatically on `JLOAD -u TOSHSD.JLM`).

Client: **`SDPROBE.COM`** (checks `0xFD06` `TOSHSD.JLM` first, falling back to `0xFD01` `FERRIC.JLM`).

## Build

```sh
sh crates/ferric-sd/build.sh
```
Produces `out/TOSHSD.JLM` (386-pure, strictly read-only).
