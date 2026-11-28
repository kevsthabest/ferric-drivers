# Stage I driver qualification

Status: test procedure, not a record of passed tests. Report the exact git
revision, JLM hashes, BIOS/host version, load order, and captured output for
each run. Do not include credentials, serial numbers, or raw firmware dumps
in the public repository.

## Build gate

- Run scripts/test-drivers.sh inside the pinned container; all host tests and
  target builds must pass. Record all eight JLM hashes.
- Boot disposable DOS media first; never replace the known-good Ferric DOS
  configuration. Test standalone modules without FERRIC.JLM.
- Load and unload each module repeatedly; missing devices and dependencies
  must fail without hanging. Verify return to DOS, then reboot to the
  known-good OS. Do not load duplicate hardware controllers.

## Current standalone PMU/SD acceptance

| Case | Required result | Evidence state |
|---|---|---|
| PMU only, battery query | Explicit supported/unavailable result; no hang | Pending standalone |
| PMU only, throttle off / low-risk duty / off | Expected state restored; no freeze | Pending standalone |
| PMU backlight | Separate supervised test, reversible | Not qualified; do not test blindly |
| SD only, valid assigned BAR | Exact physical BAR preserved; controller detected | Pending standalone |
| SD only, no assigned BAR | Fail closed without PCI allocation | Pending standalone |
| SD with empty slot | Explicit empty status | Pending standalone |
| SDSC and SDHC, disposable cards | Sector 0 and multiple LBAs match reference image hashes | Pending |
| Out-of-range LBA | Rejected before issuing command | Host test added; runtime pending |
| Removal, reinsertion and reinit | Stale readiness rejected; successful reinit after insertion | Pending |
| Init failure / retry / unload | No stale mapping or access to unclaimed resources | Pending |
| FERRIC first, then standalone PMU/SD | JLOAD rejects standalone attach before hardware access | Pending |
| Standalone first, then Stage I FERRIC | JLOAD rejects integrated attach before hardware access | Pending |
| Malformed client buffers | Null, segment wrap and HMA addresses rejected | Pending |

Use SDPROBE /R <lba> for selected reads, SDPROBE /U before unloading.
Do not test SD writes: this module is strictly read-only. Use read-only
test media and compare captured sector contents, not merely a success string.
Do not assume hot-swapping without reinitialization is supported.
The temporary load guard does not detect unrelated third-party TSRs and
cannot prevent an older integrated FERRIC build being loaded second.

## Standalone Intel Ethernet acceptance

See [Ethernet](drivers/ethernet.md). CI runs a separate public FreeDOS image
with FRES and I8255X only, two packet exchange/install cycles, missing
broker, absent NIC, and signature removal. Physical tests and DPMI client
coverage remain separate release gates. Record QEMU logs and exact hashes.

## Future extraction gate

Each extracted module needs an independent build, load and unload proof,
negative cases, device-specific physical testing, and comparison with the
integrated known-good profile. Test standard DOS clients on both real-mode
and protected-mode paths as relevant. QEMU results qualify virtual hardware
only; lack of an emulated device is recorded, not counted as a pass.

## Evidence checkpoint

2026-11-27, branch revision `96f519d`: [CI run 46086383312](https://github.com/kevsthabest/ferric-drivers/actions/runs/46086383312) passed host and target build steps; QEMU did not boot due to ambiguous upstream boot.asm archive entry. No runtime pass claimed. [Experimental build artifacts](https://github.com/kevsthabest/ferric-drivers/actions/runs/46086383312/artifacts/15238968623).

2026-11-27, `47c2324`: [CI 46087047148](https://github.com/kevsthabest/ferric-drivers/actions/runs/46087047148) passed host tests, all target builds and both independent Ethernet QEMU scenarios. Two ETHINIT status=0 and NTEST PASS results, removal and reinstall, missing broker status=9 and absent NIC status=1. Physical qualification remains pending.
