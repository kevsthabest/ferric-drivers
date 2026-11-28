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
