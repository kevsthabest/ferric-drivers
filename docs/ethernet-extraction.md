# Intel 8255x extraction (Stage I.2)

The first slice moves the qualified Configure command bytes unchanged into
ferric-i8255x. Legacy FERRIC.JLM consumes this library, so the eventual
standalone module and integrated core will not maintain independent copies.
No new hardware IDs or packet API claims follow from this change.

Next slices, in order:
1. Move DMA ring geometry and descriptor encoding, with checked physical
   addresses and explicit alignment/boundary contracts.
2. Separate EEPROM and MDI access behind a hardware I/O interface; preserve
   checksum selection and latch-low double reads.
3. Isolate hardware state from NetState's packet registrations and Wi-Fi
   backend selection. Keep packet facade registration and callback delivery
   outside device-specific code.
4. Add standalone bring-up/teardown, resource claims and DMA quiescence;
   do not activate the NIC until all claims and memory locks succeed.
5. Add JLM adapter and DOS packet client, with safe INT 60h vector ownership.
   Shared IRQ dispatch must not mask unrelated IRQ11 devices.
6. Validate QEMU 82559/82550-compatible emulation and real 8086:1229 hardware
   using mTCP DHCP, bidirectional checksummed transfers and unload/reload.
   Compare packet loss and throughput to the integrated baseline; include
   multi-file and 100 MiB transfer stress, not just ping.

Do not remove the integrated backend until physical qualification and
opt-in Ferric DOS integration are approved.
