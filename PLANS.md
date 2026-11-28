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
| I.0 | Roadmap, safety fixes, independent build/test entry point | Implemented; qualification ongoing |
| I.1 | Shared no_std PCI/MMIO/IRQ/DMA/time and device ownership interfaces | Active |
| I.2 | Independent Intel 8255x module and packet facade | QEMU qualified; physical qualification pending |
| I.3 | CardBus provider and Atheros module | Design / initial extraction |
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
- I.1: `ferric-hw` provides PCI address and MMIO mapping arithmetic, interrupt-preserving PCI configuration transactions for standalone PMU/SD, direct byte-width writes, and a tested exclusive resource claim-table model. The table is not yet a resident broker; global ownership, shared IRQ/DMA services and calibrated timing remain outstanding.
- CI checkpoint `211a960` passed; subsequent `caf839c` passed all host tests and six JLM builds but failed SDPROBE assembly. Fixed in `6a31ae7`; new CI required.

- CI checkpoint `3bb018b`: successful host tests and target builds. Later changes require a fresh run.
- I.1: experimental FRES.JLM (FD07) resident ownership service committed. Client integration and runtime unload tests pending.
- I.2: extracted the qualified 8255x configuration bytes into ferric-i8255x and wired the legacy core to the same source; no standalone Ethernet JLM yet.

- CI checkpoint `2c0dc50`: host tests and all seven JLM builds passed, including FRES.
- I.2: moved DMA geometry into ferric-i8255x and added checked address arithmetic tests; legacy uses the shared geometry.
- I.1: TOSHSD requires FRES v1.0 and claims its PCI device before writes; releases after shutdown and unregisters at detach. Runtime qualification pending.

- I.2: extracted Microwire EEPROM and MDI/PHY protocols into ferric-i8255x with mock-I/O tests, retaining the legacy port adapter.

- CI checkpoint `0ddac4b`: host tests and seven JLM target builds passed.
- I.2: shared RX/TX descriptor encoding, checked DMA region input, and bounded SCB command submission. Failed bring-up after DMA lock quarantines its DOS allocation until reboot rather than freeing potentially active DMA. Full teardown and retry lifecycle still pending.

- I.2: Ethernet-only DMA/ring/TX/EEPROM state moved into the shared EthernetState struct; packet registration and Wi-Fi state remain in the integrated facade. Lifecycle operations still need extraction.

- CI checkpoint `3e9acda`: host tests and seven JLM target builds passed.
- I.2: Ethernet probe identity and IRQ are independent of Wi-Fi state; validate BAR and IRQ before port access, defer enabling PCI bus mastering until DMA is locked, and disable it on failed bring-up. Physical validation pending.

- I.2: extracted controller initialization, configuration and RX ring programming into the reusable DeviceIo layer with mock DMA/SCB tests. IRQ installation, PCI ownership, DMA locks and teardown remain platform responsibilities.

- I.2: standalone I8255X.JLM (FD08) and ETHINIT.COM implemented; shared packet facade with legacy core; first target build passed at `6f25bd8` (11,776-byte JLM). Subsequent lifecycle fixes and QEMU smoke gate pending validation.

- CI `96f519d`: host tests and all eight target builds passed; QEMU harness stopped before boot because upstream source archive contains two boot.asm files. Select boot/boot.asm explicitly. No emulator result yet. Artifacts uploaded in run 46086383312.

- I.3: extracted ToPIC95 slot profile, socket decoding and 40-byte ABI into ferric-cardbus; invalid slot indices rejected. [CardBus extraction gates](docs/cardbus-extraction.md). No standalone bridge yet.

- I.3: FRES resource kinds 6/7 added for MMIO intervals and secondary bus ownership; CardBus pure layout validation covers alignment, inclusive bridge limits and occupied resource conflicts. Hardware discovery and transactional claim/rollback remain pending.

- CI checkpoint `47c2324`: all host tests and eight target builds passed; independent Ethernet QEMU positive and negative cases passed (two installs and ARP exchanges, unload/reinstall, missing broker and NIC). Physical acceptance remains pending.
- I.3: transactional four-resource lease with reverse-order rollback and consumer pinning; mock tests cover every partial acquisition and failed release retry.

- I.3: added mock-tested inactive bridge configuration snapshot, reset-asserted staging and verified restoration, with PCI status W1C protection. Not wired to physical hardware; full bridge/vendor register coverage and power sequencing pending.

- CI checkpoint `34ff59f` passed host/target and Ethernet QEMU gates. Added bounded CardBus socket power sequencing with voltage validation, reset ordering, removal/fault rollback, and explicit delay failure. Platform wiring pending.

- I.3: experimental CARDBUS.JLM FD09 resident discovery adapter and versioned service table added. Activation deliberately refused until firmware resource audit and hardware lifecycle wiring are complete. CI pending.

- CI `d0d44ea`: discovery-only CARDBUS build, host tests and Ethernet QEMU passed. Added shared transaction coordinator for claims/config/map/power/rollback and reverse integrated-core FD09 conflict guard. Hardware activation still disabled.

- CI `da55904`: successful host, target and Ethernet QEMU checks. I.3 resident adapter now links PCI/MMIO/FRES/PIT platform implementation and transaction coordinator; activation entry is intentionally not exported pending an actual firmware-resource audit. Consumer and shutdown paths wired to session state.

- CI `f8c9804` passed. I.3 read-only PCI audit checks known host identity, bridge boot configuration, bus conflicts and visible BAR/window overlaps. Firmware resource authorization remains required; activation still unexported.

- CI `980afef` passed host/target and Ethernet QEMU. CardBus now models the captured unassigned-but-enabled DynaBook bridge with explicit profile capture, disables decoding before reprogramming, and permits PWRCYCLE to clear after confirmed stabilization/reset release. Added regression tests. Activation remains unexported.

- I.3: shared AR2413 child enumeration and reversible memory-only PCI configuration; refuses unsupported/active cards and keeps DMA disabled. Resident child claim and handoff wiring pending.

- CI `670fc4c` passed. Added resident AR2413 child BDF lease, memory-only config, query and teardown integration; consumer FD0A borrows claimed child. Activation remains unexported; DMA and removal recovery pending.

- CI `48a8e0c` passed. I.3 Atheros extraction begins with shared pure EEPROM/RF/descriptor/CCMP helpers; integrated code re-exports them. Standalone hardware backend remains pending.

- I.3: fixed CI dead-code failure from stale bitswap helper; extracted wake/reset/EEPROM register transactions behind shared RegisterIo, with timeout and absent-hardware tests. Target CI pending.

- I.3: shared AR2413 DMA geometry and RX ring descriptor generation. Fixed host/target CI discrepancy by exposing test-only constants from shared crate and importing only under cfg(test). Combined CI pending.

- CI `989331f` passed. Shared bounded RX/TX stop and idle controls; legacy TX timeout preserves descriptor pointer on uncertain stop. DMA release/PCI isolation and standalone connection lifecycle remain pending.

## Immediate backlog
- Ethernet: QEMU passed; qualify physical hardware before release.
- I.3: isolate CardBus provider discovery, bridge windows and ownership before extracting the Atheros backend; preserve existing packet facade.
- SD: run regression tests for BAR mapping, partial initialization and CMD16 failure; validate inserted-card reads, removal and mapping/resource release.
- PMU: distinguish verified throttling from unverified battery/backlight; validate register access against hardware evidence.
- Validate temporary standalone PMU/SD exclusion in both load orders; replace whole-module exclusion with per-device ownership in I.1.
- Execute remaining standalone runtime and physical acceptance matrix. Existing purity checks are not runtime qualification.
- Investigate PCI allocator and calibrated timing before broad hardware claims.
