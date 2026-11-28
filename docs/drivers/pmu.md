# Power Management — ALi M7101 PMU (`PMU.JLM`)

Standalone Jemm Loadable Module (`PMU.JLM`, DDB `0xFD05`) for the **ALi M7101** Power Management Unit (`10B9:7101`), supporting both the ALi M1535D+ (`i2c-ali1535` layout, e.g. Toshiba DynaBook SS 2000M) and M1533/M1543 layouts.

No dependency on `FERRIC.JLM` — load it on its own under JemmEx/Jemm386.
Do not load both modules together. DynaBook evidence confirms integrated
CPU throttling; smart-battery SMBus did not ACK and LCD PWM behavior is
not yet qualified. The standalone variant requires separate silicon tests:

```bat
JLOAD.EXE PMU.JLM
PWRCTL BATTERY
PWRCTL THROTTLE 4
PWRCTL BRIGHT 6
```

## Capabilities & Verbs

- `AX=0x0000` — Ping / version (`AX=0x0001`, `BX=0xFD05`).
- `AX=0x0039` — Query Smart Battery System (SBS v1.1 over SMBus addr `0x0B`), CPU clock throttle duty, and LCD CCFT backlight level.
- `AX=0x003A` — Set ACPI `P_CNTRL` CPU clock throttling (`BX=0` off, `1..7` duty/8).
- `AX=0x003B` — Set LCD CCFT backlight PWM (`BX=0` off, `1..7` PWM, `8` max).

Client: **`PWRCTL.COM`** (checks `0xFD05` `PMU.JLM` first, falling back to `0xFD01` `FERRIC.JLM`).

## Build

```sh
sh crates/ferric-pmu/build.sh
```
Produces `out/PMU.JLM` (386-pure, zero storage writes).
