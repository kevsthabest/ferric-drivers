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

## Shared MAC probe

CI fca637b passed. Both integrated and resident backends now use the shared 44-byte WifiProbeInfo and MAC/EEPROM probe sequence. The standalone provider reports AX=0A status 0 for completed probe or 8 for partial probe, and AX=0E ES:DI copies the diagnostic record (privileged linear address hidden). AX=0D releases mapping/consumer. Late PCI configuration recovery remains integrated-only; standalone reports partial probe rather than performing unqualified bridge rewrites. DMA remains disabled. Physical activation still gated.

## Resident DMA adapter

CI 44ac8d5 passed. The VDMAD adapter and shared radio/engine functions are wired into the resident lease; unlock checks carry and uses PCI flags 0 consistently with lock. Radio tuning reports calibration and delay failure. DMA start is deliberately not exported through DOS and fails before lock while DRAIN_QUALIFIED is false. No physical drain or DOS allocation lifetime proof is claimed.

## Passive scan

Shared scan ABI and bounded passive scan engine (channels 1–11) use injected register/DMA I/O. Resident scan transaction uses the DMA lease and always attempts shutdown, retaining the lock on cleanup failure. Host mocks cover empty scan and timer abort. Integrated scan uses shared result ABI but retains its original hardware loop. Scan remains internal and activation-gated; no DOS client is permitted to free a possibly active DMA allocation.

## WPA2 connection orchestration

Shared `connection::connect` coordinates bounded open-system authentication, WPA2 association and the existing wifi-core four-way handshake over an injected LinkIo transport. It validates response address/identity, rejects short credentials and zero SNonce, requires ACK before installing keys, and retains the handshake object for group rekey. The caller must provide fresh unpredictable SNonce, verified AP/security selection and a leased transport. Integrated runtime still uses its original connection loop with the shared request ABI; resident hardware transport and full successful handshake tests remain pending.

## Packet boundary

Shared packet conversion/classification now serves integrated TX/RX classification and the future standalone data path. Protected TX refuses zero or exhausted 48-bit CCMP packet numbers rather than wrapping. The standalone receive helper validates AP/destination/DS direction, strips FCS and hardware-verified CCMP MIC, separates EAPOL from DOS payloads and pads short Ethernet frames. It requires the transport to verify descriptor success and decryption. DOS INT 60h installation, receiver callbacks and rekey handling are still pending; no standalone packet vector is installed.

## Integration status

The resident probe checkpoint 44ac8d5 passed CI. The subsequent DMA, scan, WPA2 and packet-boundary work is experimental; aggregate CI must pass before physical use. CardBus independently refuses activation and DMA mastering until authorization and qualification. No DOS packet stub is installed by ATH5K.JLM. Existing integrated runtime remains separate for now.

## Polled transport

Shared transport implements TX descriptor preparation, QCU idle check, bounded ACK polling and RX ring consumption; station address and key-cache setup follow legacy register sequences. Resident ScanAdapter implements the transport DMA copy contract. Hardware connection not yet invoked. Legacy hardware routines remain in place pending regression qualification.

## Resident connection and data path

Internal entry points now bind the shared connection engine and polled transport to resident DMA and station state. Explicit BSSID/channel and externally supplied fresh SNonce are required; no DOS entry exports credentials or activates hardware. TX uses software CCMP, RX uses the protected frame boundary. EAPOL currently invalidates the application session until group rekey is wired. Errors retain the DMA lease for shutdown. No claim of operational Wi-Fi or qualified randomness.

## Group rekey and nonce contract

Resident session now processes authenticated group-key rotations using the existing replay floor, verifies key cache metadata and sends encrypted group message 2 with a fresh CCMP PN on each retry. Failed key installation or ACK invalidates the session. The external nonce contract rejects zero/constant and immediate duplicate seeds; this is not an entropy source or a proof of uniqueness across reboot. A qualified external random source is still required. DMA and DOS connection remain gated.

## DOS packet API

Standalone packet API core implements the Crynwr register interface, four exclusive receiver registrations, bounded conventional buffers disjoint from DMA, transmit, MAC, receive modes 1–3 and counters. Two-phase callback state pins active registration against release/unload. The Jemm interrupt hook and real-mode stub installation remain separate and disabled. No hardware activation path was opened.

## Real-mode packet bridge

Experimental AX=20h returns stub length/paragraphs, 21h copies the stub, 22h installs against a connected resident session and 23h removes it. INT 60h and private INT F1h are exclusively claimed via FRES; INT 1Ch polls receive and performs two-stage callbacks with a bounded budget. Removal refuses active receivers, restores the incumbent vector only if still owned, tombstones the stub and retries failed unhooks/releases. The caller must retain its DOS allocation until removal succeeds. No DOS installer or connection activation is enabled yet. Combined CI pending.

## DOS utilities and QEMU gate smoke

CI fbb1c0d passed. ATHDIAG.COM queries the standalone DDBs, reports bridge count/audit, attempts slot 0 probe, reports probe stage, tests disconnected packet install rejection and releases the mapping. It is not a passive diagnostic on an already activated socket, so use only in the experimental test configuration. ATHINIT.COM allocates and retains a 512-byte packet stub for an existing connected resident, quarantines uncertain installation failures and otherwise frees the allocation. No DOS connection or DMA allocation service is enabled. QEMU no-NIC smoke tests cover dependencies, no child, rejected install, absent packet signature and unload. No real ToPIC/AR2413 is emulated. CI pending.

## DOS scan/connect transaction boundary

FD0A AX=24h (BX DMA segment, ES:DI 720-byte scan result) and AX=25h (BX DMA segment, ES:DI 192-byte request containing WifiConnectReq and 32 external random bytes) validate conventional memory and disjoint DMA ranges. Connection wipes accepted caller request and local passphrase/nonce copies; compiler-generated copies and handshake storage are not yet guaranteed erased. Status 7 means retain allocation; status 0 on connection also means retain allocation. Duplicate DMA start returns 2. Hardware enable remains blocked by provider and DMA qualification gates. DOS allocation owner must retain on uncertainty, and no current utility calls these operations yet.

ATHSCAN.COM now owns a 16 KiB DOS allocation, calls the bounded scan ABI, frees only after non-retained status, and stays resident on uncertain cleanup. See [wireless qualification](wireless-qualification.md). No connection loader or live activation yet.

## Lifecycle review checkpoint

24f8d20 passed combined CI (46105692782). Review hardening: failed DMA initialization now requires verified engine stop and isolation before unlock; partially stopped resident sessions invalidate probe stage to prevent restart with an unmapped BAR; packet API reports driver name from the actual stub segment and rejects buffers overlapping the live packet stub. Physical activation remains disabled. Hardware key-cache clearing on shutdown is best effort and not proof of complete secret erasure.
