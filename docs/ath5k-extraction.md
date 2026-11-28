# AR2413 extraction

The shared `ferric-ath5k` crate extracts existing EEPROM decoding, RF channel/timing calculations, RX and TX descriptor encoding, and CCMP key table encoding. The integrated driver re-exports these functions, preserving its existing callers. No radio register sequences or packet facade have been moved yet. The standalone FD0A module will depend on CardBus FD09 and use the same shared backend. Physical activation remains gated.
