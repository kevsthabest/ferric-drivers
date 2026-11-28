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
