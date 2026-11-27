# Intel 8255x extraction (Stage I.2)

The first slice moves the qualified Configure command bytes unchanged into
ferric-i8255x. Legacy FERRIC.JLM consumes this library, so the eventual
standalone module and integrated core will not maintain independent copies.
No new hardware IDs or packet API claims follow from this change.

DMA geometry and serial protocols (EEPROM, MDI/PHY) now also live in the shared crate. The legacy core uses thin port-I/O adapters. Host tests cover descriptor wrap and overflow, EEPROM checksum and fallback, Microwire read sequence, and PHY latch-low and timeout behavior.

Remaining slices, in order:
1. Complete descriptor encoding and wire the checked DMA address model into hardware initialization.
2. Isolate hardware state from NetState's packet registrations and Wi-Fi
   backend selection. Keep packet facade registration and callback delivery
   outside device-specific code.
3. Add standalone bring-up/teardown, resource claims and DMA quiescence;
   do not activate the NIC until all claims and memory locks succeed.
4. Add JLM adapter and DOS packet client, with safe INT 60h vector ownership.
   Shared IRQ dispatch must not mask unrelated IRQ11 devices.
5. Validate QEMU 82559/82550-compatible emulation and real 8086:1229 hardware
   using mTCP DHCP, bidirectional checksummed transfers and unload/reload.
   Compare packet loss and throughput to the integrated baseline; include
   multi-file and 100 MiB transfer stress, not just ping.

Do not remove the integrated backend until physical qualification and
opt-in Ferric DOS integration are approved.
