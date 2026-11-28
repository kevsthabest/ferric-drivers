# Stage I.3 — CardBus and Atheros extraction

Status: initial pure-library extraction; not a standalone bridge yet.
Preserve the known-good integrated implementation while extracting its
register decoding, 40-byte slot ABI and DynaBook-specific resource profile.

## Gates
1. Shared pure profile and socket decoding (implemented).
2. Bridge provider: exclusive PCI function ownership, MMIO and secondary-bus
   window validation, no unqualified fixed addresses on other hosts.
3. Snapshot and rollback configuration; bounded power/reset; refuse removal
   while an Atheros consumer is registered. Separate child-specific setup from
   bridge enumeration. Do not enable child bus mastering before DMA ownership.
4. Standalone bridge JLM with versioned slot query/acquire/release and mapping
   lifetime. Reserve a DDB only after reviewing existing registry.
5. Atheros backend: shared register tables, DMA lifecycle, WPA2 state machine,
   credential zeroing and packet facade. Keep FRES coordination and avoid
   duplicate INT 60h ownership.
6. Physical qualification on the DynaBook, including empty socket, power failure,
   card removal/reinsert, reconnect and checksummed large/multiple-file transfers.

The legacy resource profile is specific to the DynaBook and is not proof
that a physical window is free on another machine. The current integrated
bridge programs fixed bus numbers and windows, and enables child mastering
early; those behaviors must not be copied blindly into the standalone module.
QEMU does not emulate the target AR2413, so host tests and physical evidence
are essential. No changes to Ferric DOS main.

## Resource validation checkpoint

FRES now recognizes physical MMIO interval and secondary-bus claims (kinds 6 and 7). The shared CardBus library validates proposed socket and bridge apertures, inclusive page limits, alignment, overlaps, bus collisions and IRQ bounds. This is pure validation only: discovering trustworthy occupied firmware ranges and wiring claims into a resident bridge remain prerequisites to hardware writes. No standalone power-up is enabled yet.

## Resident discovery adapter

CARDBUS.JLM (FD09) requires FRES. Its service table 0=version, 1=query(slot, destination), 2=acquire(slot, consumer), 3=release(slot, consumer). Query returns the 40-byte slot record; stage 1 means PCI identity/header verified, not powered. Acquire currently returns 5 (not enabled), release 4 (not owned). No PCI configuration writes, MMIO mapping or power transitions occur in this build. Existing CBINIT/CBPROBE clients remain integrated-core clients until updated. No Atheros consumer should treat this as an operational provider.

## Transaction coordinator

The pure provider Session composes the resource lease, configuration snapshot, socket mapping contract and power state machine. It pins resources when cleanup cannot be verified and refuses shutdown with a consumer attached. The configuration snapshot now includes both I/O windows and ToPIC vendor configuration. The resident module remains discovery-only pending the actual platform resource audit, mapping and physical tests.

## Platform adapter checkpoint

The resident crate now links the Jemm page mapping/free services, FRES claims, serialized PCI I/O, MMIO power/reset operations and a bounded PIT delay. The internal activation routine is intentionally not accessible from the Jemm service table or DOS verbs: it still requires a real resource audit rather than assuming the legacy profile is safe. The shutdown and consumer paths use the transaction coordinator. This is not an operational bridge release.

## Read-only audit checkpoint

V86 AX=000A, BX=slot returns an audit result: 0=no detected PCI conflict, firmware authorization still required; 1=invalid; 2=wrong host; 3=unexpected bridge configuration; 4=bus conflict; 5=BAR/window conflict; 6=unknown header. No sizing writes. The historical DynaBook capture has command 0007 with zero BAR/windows and secondary bus 3/4, rather than command 0000. Successful socket state 30000820 does not contain PWRCYCLE; physical timing behavior must be reconciled before activation. The scan cannot determine all BAR sizes, prefetch windows, chipset or firmware reservations. Activation remains unexported.

## BIOS reconciliation

The specific pre-CBINIT capture (command 0007, BAR/windows zero, bus 3/4, rev 32, IRQ FF) can now be captured by the DynaBook-only snapshot path. Generic capture still rejects active decoding. Stage first disables I/O/memory/master and verifies readback; restore verifies windows before restoring the saved command. Power startup still requires PWRCYCLE during stabilization but does not require it to persist after reset release; the final recorded state 30000820 is accepted only with 3.3 V selection readback and valid card presence/type. This does not establish resource authorization or physical qualification.

## Child enumeration

The shared child layer recognizes only AR2413 (168C:001A, network class, type-0 single-function header), checks a 64 KiB aligned aperture inside the bridge window, and rejects preassigned BARs or active decode/mastering. Its reversible configuration enables memory decoding only; bus mastering remains the future Atheros DMA owner's responsibility after locked descriptors and quiescence checks. The adapter must acquire a child PCI claim before prepare, retain the returned snapshot even if verify fails, and release only after restoration. This layer is host-testable and not yet invoked by the resident provider.

## Resident child lease

The transaction coordinator now acquires the AR2413 BDF, retains a reversible child snapshot, verifies memory-only configuration, and restores the child before powering down the socket. The provider retains the BDF claim through failed cleanup; the Atheros consumer (FD0A) borrows the bridge-owned child through the session. Query reports stage 5 only for a present powered child. Activation remains unexported; child bus mastering and physical removal recovery are not implemented. CI checkpoint 670fc4c passed.

Service 4 (additive ABI 1.0; require service count >=5) sets child and bridge bus mastering for the registered FD0A consumer. Release first disables mastering. A failed readback prevents release. Caller remains responsible for DMA lock, engine shutdown and outstanding transaction drain. No DOS activation path is exported.
