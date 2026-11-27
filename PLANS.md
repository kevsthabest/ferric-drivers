# Ferric Drivers — Stage I: independent driver distribution

Status: ACTIVE (2026-11-27). Owner-approved direction: develop drivers here first; preserve Ferric DOS main until independently qualified releases can replace its bundled implementations.

## Baselines
- Ferric Drivers: 26bc95de71936d169937955807218333b508b11e.
- Ferric DOS: ab07302c3eb0d5f6b35a748f2e46bd68a8eecec8.
- Existing DDB IDs FD01–FD06 and client verbs remain compatible. New IDs require a registry review before assignment.
- FreeDOS/Jemm is the initial qualified host; other DOS versions are candidates, not claims.

## Execution order
| Phase | Deliverable | Status |
|---|---|---|
| I.0 | Roadmap, safety fixes, independent build/test entry point | Active |
| I.1 | Shared no_std PCI/MMIO/IRQ/DMA/time and device ownership interfaces | Planned |
| I.2 | Independent Intel 8255x module and packet facade | Planned |
| I.3 | CardBus provider and Atheros module | Planned |
| I.4 | PS/2 and USB mouse service with INT 33h ownership | Planned |
| I.5 | OHCI and EHCI providers, common USB enumeration/storage/HID | Planned |
| I.6 | Native ALi audio and optional SB/OPL/MIDI services | Planned |
| I.7 | Public release packaging and independent host qualification | Planned |
| I.8 | Opt-in Ferric DOS integration, then remove superseded bundled code | Deferred |

## Architecture and invariants
- Prefer static shared libraries for pure helpers, resident shared service only for global ownership or cross-module state. A simple driver must not need a full OS runtime.
- One authoritative implementation per device; integrated compatibility wrappers may temporarily coexist in source but not control the same hardware concurrently.
- Claim PCI BDF before writes, release only after DMA/IRQ quiescence and successful detach. Track legacy ports, PIC vectors and DMA channels separately. Reject unload with active consumers.
- Prefer firmware-assigned resources, fail closed on missing/invalid allocations until a bridge-aware allocator is qualified. Preserve BAR subpage offsets and full register spans.
- Serialize PCI CF8/CFC transactions and preserve interrupt state; shared IRQ handlers identify/ack only their sources and never call DOS from ISR.
- Maintain i386 instruction purity, bounded waits, no DOS reentrancy, and existing storage write boundaries. No new firmware writes.
- Block providers expose versioned capabilities, sector geometry, media generation, read/write/flush and explicit ownership. Filesystems must not be hard-wired to USB.
- Keep packet INT 60h and mouse INT 33h facades distinct from hardware drivers. Document dependencies and load/unload order for every module.
- Public repository must not receive private firmware captures, credentials, identifiers or unredistributable vendor sources.

## Acceptance per extraction
1. Host tests of protocol/state/error paths; target build and 386/assembly checks.
2. Module loads without FERRIC.JLM where promised; missing dependency fails cleanly.
3. Repeated init/shutdown, failed init retry, duplicate claim and unload ordering tested.
4. QEMU where device exists; physical DynaBook evidence for actual device, with exact hashes and logs.
5. Test combined profiles, conventional memory and throughput against known-good baseline. No regressions accepted without explicit owner decision.
6. Publish versioned artifacts, supported IDs, dependencies, limitations and license notices. Preserve previous known-good release.
7. Ferric DOS adopts pinned release in opt-in profile only; default remains untouched until owner accepts physical regression tests.

## Progress (2026-11-27)

- I.0: SD BAR subpage mapping, checked LBA/address arithmetic, readiness checks and standalone fail-closed allocation landed on the development branch. Integrated compatibility allocator retained. PCI configuration/power rollback on map failure is partial; mapping release and full lifecycle are still outstanding.
- I.0: independent build/host-test entry points, qualification procedure and PR CI workflow added; execution pending.
- I.0: added bidirectional FERRIC/standalone module conflict checks for Stage I builds, low-memory client-buffer bounds checks, and standalone SD page release and PCI command/power restoration. Runtime qualification remains pending.
- I.1: pure `ferric-hw` crate introduced (PCI address and MMIO mapping arithmetic); host tests added. Runtime PCI serialization, global ownership, IRQ/DMA and calibrated timing are not implemented yet.

## Immediate backlog
- SD: run regression tests for BAR mapping, partial initialization and CMD16 failure; validate inserted-card reads, removal and mapping/resource release.
- PMU: distinguish verified throttling from unverified battery/backlight; validate register access against hardware evidence.
- Validate temporary standalone PMU/SD exclusion in both load orders; replace whole-module exclusion with per-device ownership in I.1.
- Execute standalone build/test entry point and module acceptance matrix. Existing purity checks are not runtime qualification.
- Investigate PCI allocator and calibrated timing before broad hardware claims.
