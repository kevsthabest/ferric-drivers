# Intel 8255x standalone packet driver — I8255X.JLM

Experimental Stage I.2 module, DDB FD08. Requires FreeDOS/JemmEx or
Jemm386 with JLOAD 5.87, and FRES.JLM ABI 1.0. Does not require FERRIC.JLM.
The same packet facade and hardware implementation are shared with the
integrated driver. Supported PCI IDs initially remain 8086:1229 and
8086:1209, bus 0 function 0, firmware-assigned 64-byte I/O BAR and IRQ.
Other chip variants/bridged devices have not been qualified.

## Load

Boot a separate DOS configuration with `DEVICE=JEMMEX.EXE NOEMS D=64`.
Do not load the integrated FERRIC.JLM or another packet driver at INT 60h.
Copy FRES.JLM, I8255X.JLM, ETHINIT.COM and optionally NTEST.COM from the
same build. Load from a normal DOS prompt:

```bat
JLOAD FRES.JLM
JLOAD I8255X.JLM
ETHINIT
```

ETHINIT status 0 means initialized; 1 no usable NIC, 2 allocation failure,
3 initialization failed before DMA ownership was retained, 4 already
installed or occupied vector, 5 DMA quarantined (reboot before retry),
9 module unavailable. Configure mTCP with PACKETINT 0x60; use DHCP
as appropriate for the LAN. NTEST specifically targets QEMU's slirp
gateway (10.0.2.2), not an arbitrary physical LAN.

## Stop

Exit packet clients first so their receiver handles are released.
```bat
JLOAD -u I8255X.JLM
JLOAD -u FRES.JLM
```
A busy or uncertain teardown refuses unload. The packet stub and DOS
allocations are deliberately retained until reboot, even after successful
device teardown, to avoid freeing cached vector targets. This is not a
zero-leak TSR lifecycle; repeated installations consume conventional RAM.
The broker coordinates participating modules, not unrelated third-party TSRs.
The IRQ hook claims only NIC-owned interrupts; non-owned interrupts chain.
The initial release supports real-mode DOS clients. DPMI clients and physical
IRQ sharing still require qualification.

## Build and test

```sh
sh crates/ferric-res/build.sh
sh crates/ferric-ethernet/build.sh
sh crates/gate1-svc/build.sh # builds NTEST.COM, not a runtime dependency
sh scripts/test-ethernet-qemu.sh
```

QEMU smoke boots public FreeDOS 2046 with pinned Jemm, verifies a packet
exchange with the emulated 82550, removal, reinstallation, missing
dependency and absent NIC. No Ferric DOS system image is required.
Physical acceptance: use separate boot media on the DynaBook, obtain DHCP,
compare MAC against firmware, run bidirectional checksummed transfers
including a 100 MiB file and multi-file workloads, then unload and reboot.
Do not replace the known-good OS until regression qualification passes.
