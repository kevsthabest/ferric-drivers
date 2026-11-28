# AR2413 extraction

The shared `ferric-ath5k` crate extracts existing EEPROM decoding, RF channel/timing calculations, RX and TX descriptor encoding, and CCMP key table encoding. The integrated driver re-exports these functions, preserving its existing callers. No radio register sequences or packet facade have been moved yet. The standalone FD0A module will depend on CardBus FD09 and use the same shared backend. Physical activation remains gated.

## Register transaction extraction

The shared RegisterIo interface now owns bounded MAC wake, reset and EEPROM read sequences; the integrated adapter supplies its existing volatile MMIO and PIT delay. All-ones reads and timer failure abort the shared operations. The legacy delay adapter cannot report failure yet and retains its existing behavior. Tests simulate success, absent hardware, stuck status and delay failure. No standalone radio activation is enabled.

## RF initialization

The radio module shares the register table upload, EEPROM checksum/calibration, RF gain and bias setup, channel programming, ADC/AGC calibration, and receive filter setup. The generated table source moves into ferric-ath5k with a compatibility re-export. Legacy error diagnostics are retained. The radio delay calls preserve legacy behavior and do not yet propagate timer failure. CI pending.

## DMA ring geometry

Shared 16 KiB layout, checked linear/physical region and RX descriptor generation replace legacy ring arithmetic. The integrated adapter retains its existing DMA locking and stop behavior; standalone DMA ownership and shutdown are not yet implemented. Test-only descriptor constants now import from the shared library to resolve host/target lint differences.

## Engine control

Shared bounded RX stop, TX idle wait and TX queue stop return explicit absent/timeout/timer errors. The integrated TX timeout recovery no longer clears TXDP or re-enables a queue if stop cannot be verified. Shared stop_all attempts both engines. This is not proof of PCI bus-master isolation: the legacy disconnect and scan paths still release DMA without checking all stop results. Standalone must retain the locked allocation until engine and PCI isolation are verified. CI checkpoint 989331f passed host, target and Ethernet QEMU.

## DMA lifetime contract

The standalone-oriented DmaPlatform/Lease state machine requires isolation before lock, checked DOS memory range, descriptor preparation before enable, and verified stop plus isolation before unlock. Failure keeps the lock until successful retry. The provider adds ring-0 service 4 `dma_master(slot,consumer,enable)` with owner checks and verified PCI command bits on both bridge and AR2413. PCI readback alone is not a DMA drain guarantee; the future resident platform must implement the drain contract. Existing integrated scan/disconnect paths have not been migrated.

## Resident module checkpoint

Experimental ATH5K.JLM (FD0A) is now in the aggregate build. It checks FRES and CardBus ABI/service counts, rejects integrated FERRIC, registers its broker identity, queries and borrows only a powered AR2413, verifies mastering disabled, maps 64 KiB and retains ownership on cleanup failure. V86 AX=0A BX=slot requests a mapping-only probe; AX=0D releases it, status 0 success. CardBus discovery-only currently returns no eligible child, so this is not a working Wi-Fi driver. No DMA start, MAC probe, scan, connection or packet facade is exported yet. Provider services are rechecked at each call. CI pending.
