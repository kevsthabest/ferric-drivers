# FRES.JLM — experimental resident resource broker

DDB FD07; version 1.0. Stage I reserves this previously unused ID.
Build with `sh crates/ferric-res/build.sh`; load via JLOAD before future
clients. No dependency on FERRIC.JLM. This is not yet wired into PMU,
SD or Ethernet; the existing module exclusion guard remains necessary.

Ring-0 service table, cdecl, u32 return values:

| Index | Arguments | Return |
|---|---|---|
| 0 | none | 0x00010000 |
| 1 | owner DDB ID | status, register client |
| 2 | owner DDB ID | status, unregister (reject active claims) |
| 3 | owner, kind, a, b | status, exclusive claim |
| 4 | owner, kind, a, b | status, release |

Status: 0 success, 1 invalid, 2 busy, 3 capacity exhausted, 4 not owned.
Kind 1 PCI: a = bus<<8 | device<<3 | function, b=0.
Kind 2 I/O: a=base, b=length. Kind 3 IRQ (exclusive only), 4 ISA DMA,
5 vector: a=number, b=0. Shared IRQ dispatch is a future contract.

Clients must look up DDB via Get_DDB and validate service table count
and version before registration; do not cache pointers after unregister.
Registration pins the broker against ordinary JLOAD unload until every
client unregisters. Claims must be released only after device quiescence.
This is cooperative bookkeeping, not protection against arbitrary TSRs
or malicious ring-0 code. Owner IDs are caller-supplied; no authentication.
Jemm's JLOAD32.ASM checks AX after DLL_PROCESS_DETACH and refuses unload
on zero. Service wrappers preserve IF and serialize on single-core DOS.
No DOS calls or allocation occur in the broker. Actual JLOAD runtime
qualification is pending.

Acceptance: load FRES alone; register two test clients; conflicting PCI
and I/O claims fail; unload while a client exists fails; release and
unregister; unload succeeds. Repeat and verify conventional memory.
