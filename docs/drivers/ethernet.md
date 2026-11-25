# Ethernet — Intel 82550 packet driver

A packet driver for the **Intel 82550** Fast Ethernet controller
(`8086:1229`), resident in the core JLM (`crates/gate1-svc/src/lib.rs`,
the packet-driver section) rather than a module of its own. DOS
network stacks talk to it through the classic Crynwr packet-driver
interface at `INT 60h`, exactly as they would to a conventional
packet driver — mTCP's DHCP, PING, and FTPSRV were the acceptance
workload on real hardware.

## Scope

- **Intel 82550 only.** No other Ethernet chip is supported; the
  register programming is 8255x-specific throughout.
- The driver is one backend of the core's shared packet-driver
  facade (see [core.md](core.md)); the Wi-Fi backend can take over
  `INT 60h` at runtime (verb `0x000E`) and hand it back.

## Provenance

Spec-derived, not a port: the driver was written from Intel's
**8255x Open Source Software Developer's Manual** (the SDM's
interrupt discipline — read SCB Stat/Ack, ignore the interrupt if
it isn't ours, acknowledge at the device, chain always — is the
design's law). See `NOTICES.md`.

## Verbs and clients

- `0x0003` — packet-driver probe/info
- `0x0005` — copy the real-mode packet stub into the caller's buffer
- `0x0006` — bring the NIC up on the caller's DMA block
- `0x0007` — record the incumbent `INT 60h` vector for clean detach

Clients (in `crates/cbprobe`): **NETINIT.COM** installs the driver
at boot; **NTEST.COM** is the diagnostic client (link state, MAC,
IRQ count, ARP probe).

## Evidence

Gate 2.3 passed on the DynaBook on 2026-11-21: link up on IRQ 11,
live mTCP DHCP lease and FTPSRV transfers. See
[`docs/hardware/dynabook-ss2000m-eth.txt`](../hardware/dynabook-ss2000m-eth.txt)
(MAC redacted) and the
[support-matrix excerpt](../hardware/SUPPORT-MATRIX-EXCERPT.md).
