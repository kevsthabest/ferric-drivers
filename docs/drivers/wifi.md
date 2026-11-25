# Wi-Fi — ToPIC95 CardBus + Atheros AR2413 + WPA2 supplicant

DOS Wi-Fi for one specific card in one specific bridge, built
because nothing else would drive it: DOS had packet drivers for a
few 802.11b FullMAC cards, but the AR2413 is a SoftMAC — the host
owes the chip its MAC layer, its association state machine, and its
WPA2 supplicant, and no DOS implementation of any of that existed.

## Scope limits — up front

- **Bridge:** Toshiba ToPIC95-family CardBus bridge (`1179:0617`)
  only.
- **Card:** Atheros **AR2413** (`168C:001A`), developed and proven
  against the D-Link DWL-G630 Rev C2. No other Wi-Fi chip.
- **Band/security:** 2.4 GHz only; **WPA2-PSK (AES-CCMP) only** —
  no WEP, no WPA3, no enterprise EAP.
- The passphrase is a WCONN command-line argument, packed into the
  request record and zeroed after the connect call; nothing
  persists credentials anywhere.

## Pieces

- **Bridge driver** — `crates/gate1-svc/src/cardbus.rs`: ToPIC95
  slot power, reset, and bus enumeration. Verbs `0x0008` (bring-up)
  and `0x0009` (slot info). Clients: **CBINIT.COM**, **CBPROBE.COM**.
- **MAC driver** — `crates/gate1-svc/src/ath5k.rs` (+ generated
  tables in `ath5k_initvals.rs`): AR2413 probe/EEPROM, RF init,
  scan, the connect orchestration that drives the 4-way handshake,
  key-cache programming (hardware AES-CCM receive; software
  IEEE 802.11i AES-CCM transmit), and the Ethernet encap/decap data
  path behind the core's packet-driver facade. Verbs `0x000A`
  (probe), `0x000B` (scan), `0x000C` (connect), `0x000D`
  (diagnostics), `0x000F` (disconnect).
- **Supplicant** — `crates/wifi-core`: a standalone, zero-dependency
  crate with the 802.11 frame layer and the WPA2-Personal key
  hierarchy (PBKDF2-HMAC-SHA1 PMK, PTK derivation, EAPOL-Key MIC,
  AES-128 key unwrap) plus the `Wpa2Handshake` state machine,
  host-tested against published vectors (FIPS/RFC examples and a
  public captured handshake). It has no hardware dependencies and
  is the most reusable piece of this driver.
- **Clients** (in `crates/cbprobe`): **WPROBE.COM** (probe and
  diagnostics), **WSCAN.COM** (beacon scan), **WCONN.COM**
  (connect; also installs the `INT 60h` TSR for the Wi-Fi backend).

Receive is polled from the core's timer hook; there is no dedicated
Wi-Fi IRQ handler.

## Provenance

The register definitions derive from the ISC-licensed Linux ath5k /
OpenBSD ar5k code; the required notices are reproduced verbatim in
`NOTICES.md`, and the two derived files carry provenance headers.
The supplicant itself is original work — see `NOTICES.md` for the
record of what was evaluated and not used.

## Evidence

Gate 4b passed on the DynaBook in November 2026: scan, WPA2-PSK
association and 4-way handshake, then mTCP DHCP/DNS/PING/FTPSRV
over Wi-Fi with the Ethernet cable unplugged. See
[`docs/hardware/dynabook-ss2000m-wifi.txt`](../hardware/dynabook-ss2000m-wifi.txt)
and
[`docs/hardware/dynabook-ss2000m-cardbus.txt`](../hardware/dynabook-ss2000m-cardbus.txt)
(identifiers redacted) and the
[support-matrix excerpt](../hardware/SUPPORT-MATRIX-EXCERPT.md).
There is no QEMU smoke for Wi-Fi — no emulated AR2413 exists — so
the evidence is host unit tests plus these silicon captures.
