//! Gate 4b Wi-Fi core: 802.11 frame handling and the WPA2-Personal
//! key hierarchy, written for the freestanding JLM environment
//! (no_std, no heap, integer-only) and verified on the host against
//! published test vectors before any hardware exists.
//!
//! The crypto here is the part of WPA2 that does not depend on the
//! radio: PBKDF2-HMAC-SHA1 derives the PMK from the passphrase, the
//! 802.11 PRF derives the PTK from the PMK and the handshake
//! nonces/addresses, HMAC-SHA1-128 produces the EAPOL-Key MIC, and
//! AES-128 key unwrap (RFC 3394) recovers the GTK from message 3.
//! The bulk CCMP data path is the AR2413's hardware engine (see
//! docs/gate4b/GATE4B-PLAN.md); AES lives here for key unwrap and
//! as the audited primitive a software-CCMP fallback would build
//! on, per the spec's rule that software CCMP is a separate
//! audited component.
//!
//! Test-vector provenance (all in the tests module):
//! - SHA-1: FIPS 180-1 examples. HMAC-SHA1: RFC 2202.
//!   PBKDF2: RFC 6070. AES-128: FIPS-197. Key unwrap: RFC 3394.
//! - PMK: the widely published "password"/"IEEE" example, plus the
//!   PMK/PTK/MIC set from a real captured 4-way handshake (the
//!   aircrack-ng WPA test capture, SSID "Harkonen", passphrase
//!   "12345678"), whose three message MICs anchor the whole chain
//!   end to end. The message-4 frame was reconstructed from the
//!   802.11i layout after a transcription error in the source
//!   page; its MIC verifies, which proves the reconstruction.

#![cfg_attr(not(test), no_std)]

// 386-pure big-endian helpers: LLVM lowers llvm.bswap.i32 to the 486+ `bswap`
// opcode even under `-mcpu=i386`; black_box on each byte prevents bswap fusion.
#[inline(never)]
fn u32_to_be(v: u32) -> [u8; 4] {
    [
        core::hint::black_box((v >> 24) as u8),
        core::hint::black_box((v >> 16) as u8),
        core::hint::black_box((v >> 8) as u8),
        core::hint::black_box(v as u8),
    ]
}

#[inline(never)]
fn u32_from_be(b: [u8; 4]) -> u32 {
    let b0 = core::hint::black_box(b[0]) as u32;
    let b1 = core::hint::black_box(b[1]) as u32;
    let b2 = core::hint::black_box(b[2]) as u32;
    let b3 = core::hint::black_box(b[3]) as u32;
    (b0 << 24) | (b1 << 16) | (b2 << 8) | b3
}

#[inline(never)]
fn u64_to_be(v: u64) -> [u8; 8] {
    let hi = u32_to_be((v >> 32) as u32);
    let lo = u32_to_be(v as u32);
    [hi[0], hi[1], hi[2], hi[3], lo[0], lo[1], lo[2], lo[3]]
}

#[inline(never)]
fn u64_from_be(b: [u8; 8]) -> u64 {
    let hi = u32_from_be([b[0], b[1], b[2], b[3]]) as u64;
    let lo = u32_from_be([b[4], b[5], b[6], b[7]]) as u64;
    (hi << 32) | lo
}

// ---------------------------------------------------------------------
// SHA-1 (FIPS 180-1)
// ---------------------------------------------------------------------

pub mod sha1 {
    /// Streaming SHA-1 state. Buffers at most one 64-byte block.
    pub struct Sha1 {
        state: [u32; 5],
        buf: [u8; 64],
        buf_len: usize,
        total_len: u64,
    }

    const K: [u32; 4] = [0x5A82_7999, 0x6ED9_EBA1, 0x8F1B_BCDC, 0xCA62_C1D6];

    impl Default for Sha1 {
        fn default() -> Self {
            Self::new()
        }
    }

    impl Sha1 {
        pub fn new() -> Self {
            Sha1 {
                state: [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0],
                buf: [0; 64],
                buf_len: 0,
                total_len: 0,
            }
        }

        fn process_block(state: &mut [u32; 5], block: &[u8]) {
            debug_assert_eq!(block.len(), 64);
            let mut w = [0u32; 80];
            for i in 0..16 {
                w[i] = crate::u32_from_be([
                    block[4 * i],
                    block[4 * i + 1],
                    block[4 * i + 2],
                    block[4 * i + 3],
                ]);
            }
            for i in 16..80 {
                w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
            }
            let (mut a, mut b, mut c, mut d, mut e) =
                (state[0], state[1], state[2], state[3], state[4]);
            for (i, &wi) in w.iter().enumerate() {
                let (f, k) = match i {
                    0..=19 => ((b & c) | ((!b) & d), K[0]),
                    20..=39 => (b ^ c ^ d, K[1]),
                    40..=59 => ((b & c) | (b & d) | (c & d), K[2]),
                    _ => (b ^ c ^ d, K[3]),
                };
                let tmp = a
                    .rotate_left(5)
                    .wrapping_add(f)
                    .wrapping_add(e)
                    .wrapping_add(k)
                    .wrapping_add(wi);
                e = d;
                d = c;
                c = b.rotate_left(30);
                b = a;
                a = tmp;
            }
            state[0] = state[0].wrapping_add(a);
            state[1] = state[1].wrapping_add(b);
            state[2] = state[2].wrapping_add(c);
            state[3] = state[3].wrapping_add(d);
            state[4] = state[4].wrapping_add(e);
        }

        pub fn update(&mut self, mut data: &[u8]) {
            self.total_len = self.total_len.wrapping_add(data.len() as u64);
            if self.buf_len > 0 {
                let take = core::cmp::min(64 - self.buf_len, data.len());
                self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
                self.buf_len += take;
                data = &data[take..];
                if self.buf_len == 64 {
                    let block = self.buf;
                    Self::process_block(&mut self.state, &block);
                    self.buf_len = 0;
                }
            }
            while data.len() >= 64 {
                Self::process_block(&mut self.state, &data[..64]);
                data = &data[64..];
            }
            if !data.is_empty() {
                self.buf[..data.len()].copy_from_slice(data);
                self.buf_len = data.len();
            }
        }

        pub fn finalize(mut self) -> [u8; 20] {
            let bit_len = self.total_len.wrapping_mul(8);
            self.update(&[0x80]);
            while self.buf_len != 56 {
                self.update(&[0]);
            }
            // Feed the length directly: update() would count it.
            self.buf[56..64].copy_from_slice(&crate::u64_to_be(bit_len));
            let block = self.buf;
            Self::process_block(&mut self.state, &block);
            let mut out = [0u8; 20];
            for (i, word) in self.state.iter().enumerate() {
                out[4 * i..4 * i + 4].copy_from_slice(&crate::u32_to_be(*word));
            }
            out
        }
    }

    /// One-shot convenience wrapper.
    pub fn sha1(data: &[u8]) -> [u8; 20] {
        let mut h = Sha1::new();
        h.update(data);
        h.finalize()
    }
}

// ---------------------------------------------------------------------
// HMAC-SHA1 (RFC 2104) and PBKDF2 (RFC 2898)
// ---------------------------------------------------------------------

/// HMAC-SHA1 over the concatenation of `parts`.
pub fn hmac_sha1(key: &[u8], parts: &[&[u8]]) -> [u8; 20] {
    let mut key_block = [0u8; 64];
    if key.len() > 64 {
        let hashed = sha1::sha1(key);
        key_block[..20].copy_from_slice(&hashed);
    } else {
        key_block[..key.len()].copy_from_slice(key);
    }
    let mut inner = sha1::Sha1::new();
    let ipad: [u8; 64] = core::array::from_fn(|i| key_block[i] ^ 0x36);
    inner.update(&ipad);
    for part in parts {
        inner.update(part);
    }
    let inner_hash = inner.finalize();
    let mut outer = sha1::Sha1::new();
    let opad: [u8; 64] = core::array::from_fn(|i| key_block[i] ^ 0x5C);
    outer.update(&opad);
    outer.update(&inner_hash);
    outer.finalize()
}

/// PBKDF2-HMAC-SHA1. `out` may be any length; WPA uses 32 bytes.
pub fn pbkdf2_hmac_sha1(password: &[u8], salt: &[u8], iterations: u32, out: &mut [u8]) {
    let mut block_index: u32 = 1;
    let mut written = 0;
    while written < out.len() {
        let be = u32_to_be(block_index);
        let mut u = hmac_sha1(password, &[salt, &be]);
        let mut t = u;
        for _ in 1..iterations {
            u = hmac_sha1(password, &[&u]);
            for i in 0..20 {
                t[i] ^= u[i];
            }
        }
        let take = core::cmp::min(20, out.len() - written);
        out[written..written + take].copy_from_slice(&t[..take]);
        written += take;
        block_index += 1;
    }
}

// ---------------------------------------------------------------------
// AES-128 (FIPS-197), single-block encrypt/decrypt
// ---------------------------------------------------------------------

pub mod aes {
    const SBOX: [u8; 256] = [
        0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
        0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
        0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
        0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
        0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
        0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
        0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
        0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
        0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
        0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
        0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
        0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
        0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
        0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
        0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
        0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
    ];
    const INV_SBOX: [u8; 256] = [
        0x52, 0x09, 0x6a, 0xd5, 0x30, 0x36, 0xa5, 0x38, 0xbf, 0x40, 0xa3, 0x9e, 0x81, 0xf3, 0xd7, 0xfb,
        0x7c, 0xe3, 0x39, 0x82, 0x9b, 0x2f, 0xff, 0x87, 0x34, 0x8e, 0x43, 0x44, 0xc4, 0xde, 0xe9, 0xcb,
        0x54, 0x7b, 0x94, 0x32, 0xa6, 0xc2, 0x23, 0x3d, 0xee, 0x4c, 0x95, 0x0b, 0x42, 0xfa, 0xc3, 0x4e,
        0x08, 0x2e, 0xa1, 0x66, 0x28, 0xd9, 0x24, 0xb2, 0x76, 0x5b, 0xa2, 0x49, 0x6d, 0x8b, 0xd1, 0x25,
        0x72, 0xf8, 0xf6, 0x64, 0x86, 0x68, 0x98, 0x16, 0xd4, 0xa4, 0x5c, 0xcc, 0x5d, 0x65, 0xb6, 0x92,
        0x6c, 0x70, 0x48, 0x50, 0xfd, 0xed, 0xb9, 0xda, 0x5e, 0x15, 0x46, 0x57, 0xa7, 0x8d, 0x9d, 0x84,
        0x90, 0xd8, 0xab, 0x00, 0x8c, 0xbc, 0xd3, 0x0a, 0xf7, 0xe4, 0x58, 0x05, 0xb8, 0xb3, 0x45, 0x06,
        0xd0, 0x2c, 0x1e, 0x8f, 0xca, 0x3f, 0x0f, 0x02, 0xc1, 0xaf, 0xbd, 0x03, 0x01, 0x13, 0x8a, 0x6b,
        0x3a, 0x91, 0x11, 0x41, 0x4f, 0x67, 0xdc, 0xea, 0x97, 0xf2, 0xcf, 0xce, 0xf0, 0xb4, 0xe6, 0x73,
        0x96, 0xac, 0x74, 0x22, 0xe7, 0xad, 0x35, 0x85, 0xe2, 0xf9, 0x37, 0xe8, 0x1c, 0x75, 0xdf, 0x6e,
        0x47, 0xf1, 0x1a, 0x71, 0x1d, 0x29, 0xc5, 0x89, 0x6f, 0xb7, 0x62, 0x0e, 0xaa, 0x18, 0xbe, 0x1b,
        0xfc, 0x56, 0x3e, 0x4b, 0xc6, 0xd2, 0x79, 0x20, 0x9a, 0xdb, 0xc0, 0xfe, 0x78, 0xcd, 0x5a, 0xf4,
        0x1f, 0xdd, 0xa8, 0x33, 0x88, 0x07, 0xc7, 0x31, 0xb1, 0x12, 0x10, 0x59, 0x27, 0x80, 0xec, 0x5f,
        0x60, 0x51, 0x7f, 0xa9, 0x19, 0xb5, 0x4a, 0x0d, 0x2d, 0xe5, 0x7a, 0x9f, 0x93, 0xc9, 0x9c, 0xef,
        0xa0, 0xe0, 0x3b, 0x4d, 0xae, 0x2a, 0xf5, 0xb0, 0xc8, 0xeb, 0xbb, 0x3c, 0x83, 0x53, 0x99, 0x61,
        0x17, 0x2b, 0x04, 0x7e, 0xba, 0x77, 0xd6, 0x26, 0xe1, 0x69, 0x14, 0x63, 0x55, 0x21, 0x0c, 0x7d,
    ];
    const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1B, 0x36];

    fn xtime(x: u8) -> u8 {
        (x << 1) ^ if x & 0x80 != 0 { 0x1B } else { 0x00 }
    }
    fn mul(mut a: u8, mut b: u8) -> u8 {
        let mut p = 0u8;
        while b != 0 {
            if b & 1 != 0 {
                p ^= a;
            }
            a = xtime(a);
            b >>= 1;
        }
        p
    }

    /// AES-128 with the round keys expanded once.
    pub struct Aes128 {
        round_keys: [[u8; 16]; 11],
    }

    impl Aes128 {
        pub fn new(key: &[u8; 16]) -> Self {
            let mut w = [[0u8; 4]; 44];
            for i in 0..4 {
                w[i] = [key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]];
            }
            for i in 4..44 {
                let mut temp = w[i - 1];
                if i % 4 == 0 {
                    temp = [
                        SBOX[temp[1] as usize] ^ RCON[i / 4 - 1],
                        SBOX[temp[2] as usize],
                        SBOX[temp[3] as usize],
                        SBOX[temp[0] as usize],
                    ];
                }
                for j in 0..4 {
                    w[i][j] = w[i - 4][j] ^ temp[j];
                }
            }
            let mut round_keys = [[0u8; 16]; 11];
            for r in 0..11 {
                for c in 0..4 {
                    round_keys[r][4 * c..4 * c + 4].copy_from_slice(&w[4 * r + c]);
                }
            }
            Aes128 { round_keys }
        }

        fn add_round_key(state: &mut [u8; 16], rk: &[u8; 16]) {
            for i in 0..16 {
                state[i] ^= rk[i];
            }
        }
        fn sub_bytes(state: &mut [u8; 16]) {
            for b in state.iter_mut() {
                *b = SBOX[*b as usize];
            }
        }
        fn inv_sub_bytes(state: &mut [u8; 16]) {
            for b in state.iter_mut() {
                *b = INV_SBOX[*b as usize];
            }
        }
        fn shift_rows(s: &mut [u8; 16]) {
            let t = *s;
            // State is column-major: byte (row r, col c) at s[4c + r].
            for r in 0..4 {
                for c in 0..4 {
                    s[4 * c + r] = t[4 * ((c + r) % 4) + r];
                }
            }
        }
        fn inv_shift_rows(s: &mut [u8; 16]) {
            let t = *s;
            for r in 0..4 {
                for c in 0..4 {
                    s[4 * c + r] = t[4 * ((c + 4 - r) % 4) + r];
                }
            }
        }
        fn mix_columns(s: &mut [u8; 16]) {
            for c in 0..4 {
                let i = 4 * c;
                let (a0, a1, a2, a3) = (s[i], s[i + 1], s[i + 2], s[i + 3]);
                s[i] = mul(a0, 2) ^ mul(a1, 3) ^ a2 ^ a3;
                s[i + 1] = a0 ^ mul(a1, 2) ^ mul(a2, 3) ^ a3;
                s[i + 2] = a0 ^ a1 ^ mul(a2, 2) ^ mul(a3, 3);
                s[i + 3] = mul(a0, 3) ^ a1 ^ a2 ^ mul(a3, 2);
            }
        }
        fn inv_mix_columns(s: &mut [u8; 16]) {
            for c in 0..4 {
                let i = 4 * c;
                let (a0, a1, a2, a3) = (s[i], s[i + 1], s[i + 2], s[i + 3]);
                s[i] = mul(a0, 14) ^ mul(a1, 11) ^ mul(a2, 13) ^ mul(a3, 9);
                s[i + 1] = mul(a0, 9) ^ mul(a1, 14) ^ mul(a2, 11) ^ mul(a3, 13);
                s[i + 2] = mul(a0, 13) ^ mul(a1, 9) ^ mul(a2, 14) ^ mul(a3, 11);
                s[i + 3] = mul(a0, 11) ^ mul(a1, 13) ^ mul(a2, 9) ^ mul(a3, 14);
            }
        }

        pub fn encrypt_block(&self, block: &mut [u8; 16]) {
            Self::add_round_key(block, &self.round_keys[0]);
            for r in 1..10 {
                Self::sub_bytes(block);
                Self::shift_rows(block);
                Self::mix_columns(block);
                Self::add_round_key(block, &self.round_keys[r]);
            }
            Self::sub_bytes(block);
            Self::shift_rows(block);
            Self::add_round_key(block, &self.round_keys[10]);
        }

        pub fn decrypt_block(&self, block: &mut [u8; 16]) {
            Self::add_round_key(block, &self.round_keys[10]);
            for r in (1..10).rev() {
                Self::inv_shift_rows(block);
                Self::inv_sub_bytes(block);
                Self::add_round_key(block, &self.round_keys[r]);
                Self::inv_mix_columns(block);
            }
            Self::inv_shift_rows(block);
            Self::inv_sub_bytes(block);
            Self::add_round_key(block, &self.round_keys[0]);
        }
    }

    /// RFC 3394 AES key unwrap. `wrapped` is n+1 blocks; `out`
    /// receives the n-block plaintext key. Returns false on the
    /// integrity check (the AIV must come back as A6A6...).
    pub fn aes_unwrap(kek: &[u8; 16], wrapped: &[u8], out: &mut [u8]) -> bool {
        if wrapped.len() < 16 || !wrapped.len().is_multiple_of(8) || out.len() != wrapped.len() - 8 {
            return false;
        }
        let n = wrapped.len() / 8 - 1;
        let aes = Aes128::new(kek);
        let mut a = [0u8; 8];
        a.copy_from_slice(&wrapped[..8]);
        // R[i] blocks held in `out` as the working set.
        out.copy_from_slice(&wrapped[8..]);
        // Unwrap runs the wrap loops in reverse (RFC 3394 section
        // 2.2.2): j from 5 down, i from n down, and t is XORed into
        // A before the decryption, not out of the result.
        for j in (0..=5u64).rev() {
            for i in (1..=n as u64).rev() {
                let t = n as u64 * j + i;
                let a_xor = crate::u64_to_be(crate::u64_from_be(a) ^ t);
                let mut block = [0u8; 16];
                block[..8].copy_from_slice(&a_xor);
                block[8..].copy_from_slice(&out[8 * (i as usize - 1)..8 * i as usize]);
                aes.decrypt_block(&mut block);
                a.copy_from_slice(&block[..8]);
                out[8 * (i as usize - 1)..8 * i as usize].copy_from_slice(&block[8..]);
            }
        }
        a == [0xA6; 8]
    }

    /// IEEE 802.11i CCMP (AES-128-CCM, M=8, L=2) in-place encryption for a
    /// 24-byte 3-address non-QoS Data frame (`frame[0..plain_len]` has 24B MAC
    /// header + 8B CCMP header + plaintext payload; `frame` must have at least
    /// `plain_len + 8` bytes). Appends the 8-byte encrypted MIC and returns
    /// `Some(plain_len + 8)`.
    pub fn ccmp_encrypt_frame(tk: &[u8; 16], frame: &mut [u8], plain_len: usize) -> Option<usize> {
        if plain_len < 32 || frame.len() < plain_len + 8 {
            return None;
        }
        let data_len = plain_len - 32;
        if data_len > 0xFFFF {
            return None;
        }
        let aes = Aes128::new(tk);

        // 13-byte CCMP Nonce: [Priority(0), A2[0..6], PN5..PN0].
        let mut nonce = [0u8; 13];
        nonce[0] = 0x00;
        nonce[1..7].copy_from_slice(&frame[10..16]);
        nonce[7] = frame[31]; // PN5
        nonce[8] = frame[30]; // PN4
        nonce[9] = frame[29]; // PN3
        nonce[10] = frame[28]; // PN2
        nonce[11] = frame[25]; // PN1
        nonce[12] = frame[24]; // PN0

        // 22-byte AAD for 24-byte 3-address non-QoS Data frame (IEEE 802.11-2012 11.4.3.3.3).
        let mut aad = [0u8; 22];
        aad[0] = frame[0] & 0x8F;
        aad[1] = (frame[1] & 0xC7) | 0x40;
        aad[2..8].copy_from_slice(&frame[4..10]); // A1
        aad[8..14].copy_from_slice(&frame[10..16]); // A2
        aad[14..20].copy_from_slice(&frame[16..22]); // A3
        aad[20] = frame[22] & 0x0F;
        aad[21] = 0x00;

        // CBC-MAC: B_0 (flags = 0x59: Adata=1, M=8, L=2).
        let mut x = [0u8; 16];
        x[0] = 0x59;
        x[1..14].copy_from_slice(&nonce);
        x[14] = (data_len >> 8) as u8;
        x[15] = (data_len & 0xFF) as u8;
        aes.encrypt_block(&mut x);

        // AAD block 1: [0x00, 22, aad[0..14]].
        let mut blk = [0u8; 16];
        blk[0] = 0x00;
        blk[1] = 22;
        blk[2..16].copy_from_slice(&aad[0..14]);
        for j in 0..16 {
            x[j] ^= blk[j];
        }
        aes.encrypt_block(&mut x);

        // AAD block 2: [aad[14..22], 0..0].
        blk = [0u8; 16];
        blk[0..8].copy_from_slice(&aad[14..22]);
        for j in 0..16 {
            x[j] ^= blk[j];
        }
        aes.encrypt_block(&mut x);

        // Plaintext blocks into CBC-MAC.
        let mut off = 32usize;
        while off < plain_len {
            let chunk = (plain_len - off).min(16);
            for j in 0..chunk {
                x[j] ^= frame[off + j];
            }
            aes.encrypt_block(&mut x);
            off += chunk;
        }
        let mut mic = [0u8; 8];
        mic.copy_from_slice(&x[0..8]);

        // CTR-mode S_0 for MIC encryption (flags = 0x01: L=2).
        let mut ctr = [0u8; 16];
        ctr[0] = 0x01;
        ctr[1..14].copy_from_slice(&nonce);
        ctr[14] = 0;
        ctr[15] = 0;
        let mut s0 = ctr;
        aes.encrypt_block(&mut s0);
        for j in 0..8 {
            frame[plain_len + j] = mic[j] ^ s0[j];
        }

        // CTR-mode encryption of payload (counter i = 1, 2, ...).
        let mut block_idx = 1u16;
        let mut off = 32usize;
        while off < plain_len {
            ctr[14] = (block_idx >> 8) as u8;
            ctr[15] = (block_idx & 0xFF) as u8;
            let mut si = ctr;
            aes.encrypt_block(&mut si);
            let chunk = (plain_len - off).min(16);
            for j in 0..chunk {
                frame[off + j] ^= si[j];
            }
            off += chunk;
            block_idx = block_idx.wrapping_add(1);
        }

        Some(plain_len + 8)
    }

    /// IEEE 802.11i CCMP (AES-128-CCM, M=8, L=2) in-place decryption and MIC
    /// verification for a 24-byte 3-address non-QoS Data frame
    /// (`frame[0..enc_len]` has 24B MAC header + 8B CCMP header + ciphertext +
    /// 8B encrypted MIC). Returns `Some(enc_len - 8)` if the MIC verifies,
    /// else `None`.
    pub fn ccmp_decrypt_frame(tk: &[u8; 16], frame: &mut [u8], enc_len: usize) -> Option<usize> {
        if enc_len < 40 || frame.len() < enc_len {
            return None;
        }
        let plain_len = enc_len - 8;
        let data_len = plain_len - 32;
        if data_len > 0xFFFF {
            return None;
        }
        let aes = Aes128::new(tk);

        // 13-byte CCMP Nonce: [Priority(0), A2[0..6], PN5..PN0].
        let mut nonce = [0u8; 13];
        nonce[0] = 0x00;
        nonce[1..7].copy_from_slice(&frame[10..16]);
        nonce[7] = frame[31]; // PN5
        nonce[8] = frame[30]; // PN4
        nonce[9] = frame[29]; // PN3
        nonce[10] = frame[28]; // PN2
        nonce[11] = frame[25]; // PN1
        nonce[12] = frame[24]; // PN0

        // 22-byte AAD for 24-byte 3-address non-QoS Data frame.
        let mut aad = [0u8; 22];
        aad[0] = frame[0] & 0x8F;
        aad[1] = (frame[1] & 0xC7) | 0x40;
        aad[2..8].copy_from_slice(&frame[4..10]);
        aad[8..14].copy_from_slice(&frame[10..16]);
        aad[14..20].copy_from_slice(&frame[16..22]);
        aad[20] = frame[22] & 0x0F;
        aad[21] = 0x00;

        // CTR-mode S_0 to recover plaintext MIC.
        let mut ctr = [0u8; 16];
        ctr[0] = 0x01;
        ctr[1..14].copy_from_slice(&nonce);
        let mut s0 = ctr;
        aes.encrypt_block(&mut s0);
        let mut rx_mic = [0u8; 8];
        for j in 0..8 {
            rx_mic[j] = frame[plain_len + j] ^ s0[j];
        }

        // CTR-mode decryption of payload (counter i = 1, 2, ...).
        let mut block_idx = 1u16;
        let mut off = 32usize;
        while off < plain_len {
            ctr[14] = (block_idx >> 8) as u8;
            ctr[15] = (block_idx & 0xFF) as u8;
            let mut si = ctr;
            aes.encrypt_block(&mut si);
            let chunk = (plain_len - off).min(16);
            for j in 0..chunk {
                frame[off + j] ^= si[j];
            }
            off += chunk;
            block_idx = block_idx.wrapping_add(1);
        }

        // Recompute CBC-MAC over B_0, AAD, and decrypted plaintext.
        let mut x = [0u8; 16];
        x[0] = 0x59;
        x[1..14].copy_from_slice(&nonce);
        x[14] = (data_len >> 8) as u8;
        x[15] = (data_len & 0xFF) as u8;
        aes.encrypt_block(&mut x);

        let mut blk = [0u8; 16];
        blk[0] = 0x00;
        blk[1] = 22;
        blk[2..16].copy_from_slice(&aad[0..14]);
        for j in 0..16 {
            x[j] ^= blk[j];
        }
        aes.encrypt_block(&mut x);

        blk = [0u8; 16];
        blk[0..8].copy_from_slice(&aad[14..22]);
        for j in 0..16 {
            x[j] ^= blk[j];
        }
        aes.encrypt_block(&mut x);

        let mut off = 32usize;
        while off < plain_len {
            let chunk = (plain_len - off).min(16);
            for j in 0..chunk {
                x[j] ^= frame[off + j];
            }
            aes.encrypt_block(&mut x);
            off += chunk;
        }

        let mut diff = 0u8;
        for j in 0..8 {
            diff |= x[j] ^ rx_mic[j];
        }
        if diff != 0 {
            for b in frame[32..plain_len].iter_mut() {
                *b = 0;
            }
            return None;
        }
        Some(plain_len)
    }
}

// ---------------------------------------------------------------------
// WPA2-Personal key hierarchy (IEEE 802.11i §8.5)
// ---------------------------------------------------------------------

pub mod wpa {
    use crate::{hmac_sha1, pbkdf2_hmac_sha1};

    /// PMK = PBKDF2-HMAC-SHA1(passphrase, SSID, 4096, 32).
    pub fn derive_pmk(passphrase: &[u8], ssid: &[u8]) -> [u8; 32] {
        let mut pmk = [0u8; 32];
        pbkdf2_hmac_sha1(passphrase, ssid, 4096, &mut pmk);
        pmk
    }

    /// The 802.11 PRF: successive HMAC-SHA1 over
    /// label || 0x00 || data || counter (counter from 0), truncated
    /// or extended to fill `out`.
    pub fn prf(key: &[u8], label: &[u8], data: &[u8], out: &mut [u8]) {
        let mut written = 0;
        let mut counter: u8 = 0;
        while written < out.len() {
            let ctr = [counter];
            let digest = hmac_sha1(key, &[label, &[0u8], data, &ctr]);
            let take = core::cmp::min(20, out.len() - written);
            out[written..written + take].copy_from_slice(&digest[..take]);
            written += take;
            counter = counter.wrapping_add(1);
        }
    }

    /// The CCMP pairwise transient key: KCK || KEK || TK.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct Ptk {
        pub kck: [u8; 16],
        pub kek: [u8; 16],
        pub tk: [u8; 16],
    }

    /// PTK = PRF-384(PMK, "Pairwise key expansion",
    /// Min(AA,SA) || Max(AA,SA) || Min(ANonce,SNonce) ||
    /// Max(ANonce,SNonce)). Address/nonce ordering is by raw byte
    /// comparison, per the standard.
    pub fn derive_ptk(
        pmk: &[u8; 32],
        aa: &[u8; 6],
        sa: &[u8; 6],
        anonce: &[u8; 32],
        snonce: &[u8; 32],
    ) -> Ptk {
        #[inline(never)]
        fn bytes_le(a: &[u8], b: &[u8]) -> bool {
            let mut i = 0;
            while i < a.len() && i < b.len() {
                let x = core::hint::black_box(a[i]);
                let y = core::hint::black_box(b[i]);
                if x != y {
                    return x < y;
                }
                i += 1;
            }
            a.len() <= b.len()
        }
        let mut data = [0u8; 76];
        let (lo_a, hi_a) = if bytes_le(aa, sa) { (aa, sa) } else { (sa, aa) };
        let (lo_n, hi_n) = if bytes_le(anonce, snonce) {
            (anonce, snonce)
        } else {
            (snonce, anonce)
        };
        data[0..6].copy_from_slice(lo_a);
        data[6..12].copy_from_slice(hi_a);
        data[12..44].copy_from_slice(lo_n);
        data[44..76].copy_from_slice(hi_n);
        let mut raw = [0u8; 48];
        prf(pmk, b"Pairwise key expansion", &data, &mut raw);
        let mut ptk = Ptk {
            kck: [0; 16],
            kek: [0; 16],
            tk: [0; 16],
        };
        ptk.kck.copy_from_slice(&raw[0..16]);
        ptk.kek.copy_from_slice(&raw[16..32]);
        ptk.tk.copy_from_slice(&raw[32..48]);
        ptk
    }

    /// EAPOL-Key MIC for WPA2/CCMP: the first 16 bytes of
    /// HMAC-SHA1(KCK, pdu), where `pdu` is the whole 802.1X PDU
    /// with its MIC field zeroed.
    pub fn eapol_key_mic(kck: &[u8; 16], pdu: &[u8]) -> [u8; 16] {
        let digest = hmac_sha1(kck, &[pdu]);
        let mut mic = [0u8; 16];
        mic.copy_from_slice(&digest[..16]);
        mic
    }
}

// ---------------------------------------------------------------------
// 802.11 frames: MAC header, management bodies, LLC/SNAP data
// ---------------------------------------------------------------------

pub mod frame {
    /// Frame types (Frame Control bits 2-3).
    pub const TYPE_MGMT: u8 = 0;
    pub const TYPE_DATA: u8 = 2;

    // Management subtypes used by the station path.
    pub const SUB_BEACON: u8 = 8;
    pub const SUB_PROBE_RESP: u8 = 5;
    pub const SUB_PROBE_REQ: u8 = 4;
    pub const SUB_AUTH: u8 = 11;
    pub const SUB_ASSOC_REQ: u8 = 0;
    pub const SUB_ASSOC_RESP: u8 = 1;
    pub const SUB_DEAUTH: u8 = 12;
    pub const SUB_DISASSOC: u8 = 10;

    pub const ETHERTYPE_EAPOL: u16 = 0x888E;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct MacHeader {
        pub frame_type: u8,
        pub subtype: u8,
        pub to_ds: bool,
        pub from_ds: bool,
        pub protected: bool,
        pub duration: u16,
        pub addr1: [u8; 6],
        pub addr2: [u8; 6],
        pub addr3: [u8; 6],
        pub seq: u16,
    }

    /// Parse the fixed 24-byte MAC header (no addr4 / QoS / HT
    /// fields — the AR2413 is a legacy part and the station path
    /// never emits them).
    pub fn parse_header(frame: &[u8]) -> Option<(MacHeader, usize)> {
        if frame.len() < 24 {
            return None;
        }
        let fc = u16::from_le_bytes([frame[0], frame[1]]);
        let mut hdr = MacHeader {
            frame_type: ((fc >> 2) & 0x3) as u8,
            subtype: ((fc >> 4) & 0xF) as u8,
            to_ds: fc & 0x0100 != 0,
            from_ds: fc & 0x0200 != 0,
            protected: fc & 0x4000 != 0,
            duration: u16::from_le_bytes([frame[2], frame[3]]),
            addr1: [0; 6],
            addr2: [0; 6],
            addr3: [0; 6],
            seq: u16::from_le_bytes([frame[22], frame[23]]),
        };
        hdr.addr1.copy_from_slice(&frame[4..10]);
        hdr.addr2.copy_from_slice(&frame[10..16]);
        hdr.addr3.copy_from_slice(&frame[16..22]);
        Some((hdr, 24))
    }

    fn write_header(out: &mut [u8], hdr: &MacHeader) -> usize {
        let mut fc: u16 = ((hdr.frame_type as u16) << 2) | ((hdr.subtype as u16) << 4);
        if hdr.to_ds {
            fc |= 0x0100;
        }
        if hdr.from_ds {
            fc |= 0x0200;
        }
        if hdr.protected {
            fc |= 0x4000;
        }
        out[0..2].copy_from_slice(&fc.to_le_bytes());
        out[2..4].copy_from_slice(&hdr.duration.to_le_bytes());
        out[4..10].copy_from_slice(&hdr.addr1);
        out[10..16].copy_from_slice(&hdr.addr2);
        out[16..22].copy_from_slice(&hdr.addr3);
        out[22..24].copy_from_slice(&hdr.seq.to_le_bytes());
        24
    }

    /// Iterate information elements in a management body:
    /// yields (element id, payload) pairs.
    pub fn for_each_ie<'a>(
        mut body: &'a [u8],
        mut f: impl FnMut(u8, &'a [u8]) -> bool,
    ) {
        while body.len() >= 2 {
            let (id, len) = (body[0], body[1] as usize);
            if body.len() < 2 + len {
                return;
            }
            if !f(id, &body[2..2 + len]) {
                return;
            }
            body = &body[2 + len..];
        }
    }

    /// What the station needs from a beacon / probe response.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub struct BeaconInfo<'a> {
        pub bssid: [u8; 6],
        pub capability: u16,
        pub ssid: &'a [u8],
        /// The RSN information element's payload, if present.
        pub rsn: Option<&'a [u8]>,
        /// Channel from the DS Parameter Set element, if present.
        pub channel: Option<u8>,
    }

    pub fn parse_beacon(frame: &[u8]) -> Option<BeaconInfo<'_>> {
        let (hdr, off) = parse_header(frame)?;
        if hdr.frame_type != TYPE_MGMT
            || (hdr.subtype != SUB_BEACON && hdr.subtype != SUB_PROBE_RESP)
        {
            return None;
        }
        let body = &frame[off..];
        if body.len() < 12 {
            return None;
        }
        // Fixed parameters: timestamp(8) interval(2) capability(2).
        let capability = u16::from_le_bytes([body[10], body[11]]);
        let mut info = BeaconInfo {
            bssid: hdr.addr3,
            capability,
            ssid: &[],
            rsn: None,
            channel: None,
        };
        for_each_ie(&body[12..], |id, payload| {
            match id {
                // A zero-length SSID element is the wildcard form;
                // padding bytes at the end of a frame can parse as
                // one and must not clobber the real SSID seen first.
                0 if !payload.is_empty() => info.ssid = payload,
                3 => info.channel = payload.first().copied(),
                48 => info.rsn = Some(payload),
                _ => {}
            }
            true
        });
        Some(info)
    }

    /// Build an open-system authentication frame (sequence 1).
    /// Returns the frame length, or 0 if `out` is too small.
    pub fn build_auth(out: &mut [u8], sa: &[u8; 6], bssid: &[u8; 6], seq: u16) -> usize {
        if out.len() < 30 {
            return 0;
        }
        let hdr = MacHeader {
            frame_type: TYPE_MGMT,
            subtype: SUB_AUTH,
            to_ds: false,
            from_ds: false,
            protected: false,
            duration: 0,
            addr1: *bssid,
            addr2: *sa,
            addr3: *bssid,
            seq: 0,
        };
        let mut n = write_header(out, &hdr);
        out[n..n + 2].copy_from_slice(&0u16.to_le_bytes()); // algorithm: open system
        out[n + 2..n + 4].copy_from_slice(&seq.to_le_bytes());
        out[n + 4..n + 6].copy_from_slice(&0u16.to_le_bytes()); // status: success
        n += 6;
        n
    }

    /// Build a probe request for one SSID (empty = broadcast).
    pub fn build_probe_req(out: &mut [u8], sa: &[u8; 6], ssid: &[u8]) -> usize {
        if out.len() < 24 + 2 + ssid.len() + 2 + 8 {
            return 0;
        }
        let broadcast = [0xFF; 6];
        let hdr = MacHeader {
            frame_type: TYPE_MGMT,
            subtype: SUB_PROBE_REQ,
            to_ds: false,
            from_ds: false,
            protected: false,
            duration: 0,
            addr1: broadcast,
            addr2: *sa,
            addr3: broadcast,
            seq: 0,
        };
        let mut n = write_header(out, &hdr);
        out[n] = 0; // SSID element
        out[n + 1] = ssid.len() as u8;
        out[n + 2..n + 2 + ssid.len()].copy_from_slice(ssid);
        n += 2 + ssid.len();
        // Supported rates: 1, 2, 5.5, 11 (basic), 6, 12, 24, 54.
        let rates = [0x82, 0x84, 0x8B, 0x96, 0x0C, 0x18, 0x30, 0x48];
        out[n] = 1;
        out[n + 1] = rates.len() as u8;
        out[n + 2..n + 2 + rates.len()].copy_from_slice(&rates);
        n += 2 + rates.len();
        n
    }

    /// Build an association request. `rsn` is the station's RSN IE
    /// payload (without id/length), or None for an open network.
    pub fn build_assoc_req(
        out: &mut [u8],
        sa: &[u8; 6],
        bssid: &[u8; 6],
        capability: u16,
        ssid: &[u8],
        rsn: Option<&[u8]>,
    ) -> usize {
        let rsn_len = rsn.map_or(0, |r| r.len() + 2);
        if out.len() < 24 + 4 + 2 + ssid.len() + 2 + 8 + rsn_len {
            return 0;
        }
        let hdr = MacHeader {
            frame_type: TYPE_MGMT,
            subtype: SUB_ASSOC_REQ,
            to_ds: false,
            from_ds: false,
            protected: false,
            duration: 0,
            addr1: *bssid,
            addr2: *sa,
            addr3: *bssid,
            seq: 0,
        };
        let mut n = write_header(out, &hdr);
        out[n..n + 2].copy_from_slice(&capability.to_le_bytes());
        out[n + 2..n + 4].copy_from_slice(&10u16.to_le_bytes()); // listen interval
        n += 4;
        out[n] = 0;
        out[n + 1] = ssid.len() as u8;
        out[n + 2..n + 2 + ssid.len()].copy_from_slice(ssid);
        n += 2 + ssid.len();
        let rates = [0x82, 0x84, 0x8B, 0x96, 0x0C, 0x18, 0x30, 0x48];
        out[n] = 1;
        out[n + 1] = rates.len() as u8;
        out[n + 2..n + 2 + rates.len()].copy_from_slice(&rates);
        n += 2 + rates.len();
        if let Some(r) = rsn {
            out[n] = 48;
            out[n + 1] = r.len() as u8;
            out[n + 2..n + 2 + r.len()].copy_from_slice(r);
            n += 2 + r.len();
        }
        n
    }

    /// The LLC/SNAP header that wraps an Ethernet payload in an
    /// 802.11 data frame: AA AA 03, OUI 00:00:00, then EtherType.
    pub const SNAP_LEN: usize = 8;

    /// Encapsulate an Ethernet payload (starting at the EtherType)
    /// as a ToDS data frame body: LLC/SNAP + payload. Returns the
    /// total frame length including the MAC header.
    pub fn build_data_tods(
        out: &mut [u8],
        sa: &[u8; 6],
        bssid: &[u8; 6],
        da: &[u8; 6],
        ethertype: u16,
        payload: &[u8],
    ) -> usize {
        if out.len() < 24 + SNAP_LEN + payload.len() {
            return 0;
        }
        let hdr = MacHeader {
            frame_type: TYPE_DATA,
            subtype: 0,
            to_ds: true,
            from_ds: false,
            protected: false,
            duration: 0,
            addr1: *bssid,
            addr2: *sa,
            addr3: *da,
            seq: 0,
        };
        let mut n = write_header(out, &hdr);
        out[n..n + 6].copy_from_slice(&[0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00]);
        out[n + 6..n + 8].copy_from_slice(&ethertype.to_be_bytes());
        n += SNAP_LEN;
        out[n..n + payload.len()].copy_from_slice(payload);
        n += payload.len();
        n
    }

    /// If `body` (a data frame's payload after the MAC header)
    /// carries LLC/SNAP, return (EtherType, inner payload).
    pub fn snap_decap(body: &[u8]) -> Option<(u16, &[u8])> {
        if body.len() < SNAP_LEN {
            return None;
        }
        if body[0..6] != [0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00] {
            return None;
        }
        let ethertype = u16::from_be_bytes([body[6], body[7]]);
        Some((ethertype, &body[SNAP_LEN..]))
    }

    /// 22-byte WPA2-PSK-CCMP RSN Information Element (Element ID 48):
    /// version 1, CCMP-128 group cipher, 1 pairwise cipher (CCMP-128),
    /// 1 AKM suite (PSK), RSN capabilities 0x0000.
    pub const WPA2_RSN_IE: [u8; 22] = [
        0x30, 0x14, // Element ID = 48 (RSN), Length = 20
        0x01, 0x00, // Version = 1
        0x00, 0x0F, 0xAC, 0x04, // Group Data Cipher: CCMP-128
        0x01, 0x00, // Pairwise Cipher Count = 1
        0x00, 0x0F, 0xAC, 0x04, // Pairwise Cipher: CCMP-128
        0x01, 0x00, // AKM Suite Count = 1
        0x00, 0x0F, 0xAC, 0x02, // AKM Suite: PSK
        0x00, 0x00, // RSN Capabilities = 0x0000
    ];

    /// Build an 802.11 Association Request with Supported Rates
    /// (1/2/5.5/11/6/9/12/18 Mbps), Extended Supported Rates
    /// (24/36/48/54 Mbps), and optional 22-byte WPA2-PSK-CCMP RSN IE.
    /// Returns the frame length, or 0 if `out` is too small or `ssid` > 32 bytes.
    pub fn build_assoc_req_rsn(
        out: &mut [u8],
        sa: &[u8; 6],
        bssid: &[u8; 6],
        ssid: &[u8],
        seq: u16,
        include_rsn: bool,
    ) -> usize {
        if ssid.len() > 32 {
            return 0;
        }
        let rsn_len = if include_rsn { WPA2_RSN_IE.len() } else { 0 };
        // Advertise 802.11b CCK basic rates (1, 2, 5.5, 11 Mbps) with Long
        // Preamble & Long Slot Time (capability 0x0011/0x0001) until per-channel
        // EEPROM RF/IQ calibration is loaded.
        let total = 24 + 4 + (2 + ssid.len()) + (2 + 4) + rsn_len;
        if out.len() < total {
            return 0;
        }
        let hdr = MacHeader {
            frame_type: TYPE_MGMT,
            subtype: SUB_ASSOC_REQ,
            to_ds: false,
            from_ds: false,
            protected: false,
            duration: 0,
            addr1: *bssid,
            addr2: *sa,
            addr3: *bssid,
            seq: (seq & 0x0FFF) << 4,
        };
        let mut n = write_header(out, &hdr);
        // ESS (0x0001) + optional Privacy (0x0010); Long Preamble & Long Slot Time.
        let capability: u16 = if include_rsn { 0x0011 } else { 0x0001 };
        out[n..n + 2].copy_from_slice(&capability.to_le_bytes());
        out[n + 2..n + 4].copy_from_slice(&10u16.to_le_bytes()); // listen interval = 10
        n += 4;
        // SSID IE (id=0)
        out[n] = 0;
        out[n + 1] = ssid.len() as u8;
        out[n + 2..n + 2 + ssid.len()].copy_from_slice(ssid);
        n += 2 + ssid.len();
        // Supported Rates IE (id=1): 1, 2, 5.5, 11 Mbps CCK (basic, long preamble)
        let rates = [0x82, 0x84, 0x8B, 0x96];
        out[n] = 1;
        out[n + 1] = rates.len() as u8;
        out[n + 2..n + 2 + rates.len()].copy_from_slice(&rates);
        n += 2 + rates.len();
        if include_rsn {
            out[n..n + WPA2_RSN_IE.len()].copy_from_slice(&WPA2_RSN_IE);
            n += WPA2_RSN_IE.len();
        }
        n
    }

    /// Parse an 802.11 Authentication response frame, returning
    /// `(auth_seq, status_code)`.
    pub fn parse_auth_resp(frame: &[u8]) -> Option<(u16, u16)> {
        let (hdr, off) = parse_header(frame)?;
        if hdr.frame_type != TYPE_MGMT || hdr.subtype != SUB_AUTH {
            return None;
        }
        let body = &frame[off..];
        if body.len() < 6 {
            return None;
        }
        let auth_seq = u16::from_le_bytes([body[2], body[3]]);
        let status_code = u16::from_le_bytes([body[4], body[5]]);
        Some((auth_seq, status_code))
    }

    /// Parse an 802.11 Association Response frame, returning
    /// `(status_code, aid & 0x3FFF)`.
    pub fn parse_assoc_resp(frame: &[u8]) -> Option<(u16, u16)> {
        let (hdr, off) = parse_header(frame)?;
        if hdr.frame_type != TYPE_MGMT || hdr.subtype != SUB_ASSOC_RESP {
            return None;
        }
        let body = &frame[off..];
        if body.len() < 6 {
            return None;
        }
        let status_code = u16::from_le_bytes([body[2], body[3]]);
        let aid = u16::from_le_bytes([body[4], body[5]]) & 0x3FFF;
        Some((status_code, aid))
    }

    /// Build a 26-byte 802.11 Management Deauthentication frame (`SUB_DEAUTH`,
    /// e.g. `reason = 3` for STA leaving BSS). Returns `26` or `0` if `out` is too small.
    pub fn build_deauth(
        out: &mut [u8],
        sa: &[u8; 6],
        bssid: &[u8; 6],
        seq: u16,
        reason: u16,
    ) -> usize {
        if out.len() < 26 {
            return 0;
        }
        let hdr = MacHeader {
            frame_type: TYPE_MGMT,
            subtype: SUB_DEAUTH,
            to_ds: false,
            from_ds: false,
            protected: false,
            duration: 0,
            addr1: *bssid,
            addr2: *sa,
            addr3: *bssid,
            seq: (seq & 0x0FFF) << 4,
        };
        let n = write_header(out, &hdr);
        out[n..n + 2].copy_from_slice(&reason.to_le_bytes());
        n + 2
    }

    /// Convert an Ethernet II frame (`dst[6] src[6] type[2] payload`) into an
    /// 802.11 ToDS Data frame (`addr1=bssid, addr2=sa, addr3=dst`) with optional
    /// 8-byte CCMP header and 8-byte LLC/SNAP header.
    /// Returns the total 802.11 frame length, or 0 on error / small buffer.
    pub fn encap_eth_to_80211(
        out: &mut [u8],
        eth_frame: &[u8],
        bssid: &[u8; 6],
        sa: &[u8; 6],
        seq: u16,
        ccmp_pn: Option<(u64, u8)>,
    ) -> usize {
        if eth_frame.len() < 14 {
            return 0;
        }
        let payload = &eth_frame[14..];
        let ccmp_len = if ccmp_pn.is_some() { 8 } else { 0 };
        let total = 24 + ccmp_len + SNAP_LEN + payload.len();
        if out.len() < total {
            return 0;
        }
        let mut dst = [0u8; 6];
        dst.copy_from_slice(&eth_frame[0..6]);
        let hdr = MacHeader {
            frame_type: TYPE_DATA,
            subtype: 0,
            to_ds: true,
            from_ds: false,
            protected: ccmp_pn.is_some(),
            duration: 0,
            addr1: *bssid,
            addr2: *sa,
            addr3: dst,
            seq: (seq & 0x0FFF) << 4,
        };
        let mut n = write_header(out, &hdr);
        if let Some((pn, key_idx)) = ccmp_pn {
            out[n] = (pn & 0xFF) as u8;
            out[n + 1] = ((pn >> 8) & 0xFF) as u8;
            out[n + 2] = 0x00;
            out[n + 3] = 0x20 | ((key_idx & 0x03) << 6);
            out[n + 4] = ((pn >> 16) & 0xFF) as u8;
            out[n + 5] = ((pn >> 24) & 0xFF) as u8;
            out[n + 6] = ((pn >> 32) & 0xFF) as u8;
            out[n + 7] = ((pn >> 40) & 0xFF) as u8;
            n += 8;
        }
        out[n..n + 6].copy_from_slice(&[0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00]);
        out[n + 6..n + 8].copy_from_slice(&eth_frame[12..14]);
        n += SNAP_LEN;
        out[n..n + payload.len()].copy_from_slice(payload);
        n += payload.len();
        n
    }

    /// Convert an 802.11 FromDS Data frame (`addr2 == expected_bssid`) into an
    /// Ethernet II frame (`dst=addr1, src=addr3, type, payload`).
    /// Handles QoS Data (subtype 8, 2-byte QoS Control) and skips the 8-byte
    /// CCMP header when the Protected bit is set.
    pub fn decap_80211_to_eth(
        out: &mut [u8],
        frame: &[u8],
        expected_bssid: &[u8; 6],
    ) -> Option<usize> {
        let (hdr, mut off) = parse_header(frame)?;
        if hdr.frame_type != TYPE_DATA || !hdr.from_ds || hdr.to_ds {
            return None;
        }
        if hdr.addr2 != *expected_bssid {
            return None;
        }
        if hdr.subtype == 8 {
            if frame.len() < off + 2 {
                return None;
            }
            off += 2;
        } else if hdr.subtype != 0 {
            return None;
        }
        if hdr.protected {
            if frame.len() < off + 8 {
                return None;
            }
            off += 8;
        }
        // AR5212/AR2413 hardware inserts 2 bytes of alignment padding after a
        // 26-byte QoS MAC header so the payload starts on a 32-bit boundary
        // (vendor/ref/linux-ath/ath5k/base.c:1351-1410, ath5k_remove_padding).
        let (ethertype, payload) = match snap_decap(&frame[off..]) {
            Some(v) => v,
            None if hdr.subtype == 8 && frame.len() >= off + 2 => snap_decap(&frame[off + 2..])?,
            None => return None,
        };
        let eth_len = 14 + payload.len();
        if out.len() < eth_len {
            return None;
        }
        out[0..6].copy_from_slice(&hdr.addr1);
        out[6..12].copy_from_slice(&hdr.addr3);
        out[12..14].copy_from_slice(&ethertype.to_be_bytes());
        out[14..eth_len].copy_from_slice(payload);
        Some(eth_len)
    }
}

pub use frame::WPA2_RSN_IE;

// ---------------------------------------------------------------------
// WPA2-PSK 4-Way Handshake state machine (IEEE 802.11i)
// ---------------------------------------------------------------------

pub mod handshake {
    use crate::aes::aes_unwrap;
    pub use crate::frame::WPA2_RSN_IE;
    use crate::hmac_sha1;
    use crate::wpa::{Ptk, derive_ptk, eapol_key_mic};

    const EAPOL_KEY_FIXED_LEN: usize = 99;
    const EAPOL_KEY_BODY_MIN: usize = 95;
    const M2_LEN: usize = EAPOL_KEY_FIXED_LEN + WPA2_RSN_IE.len(); // 121
    const M4_LEN: usize = EAPOL_KEY_FIXED_LEN; // 99

    const KEY_INFO_VER_MASK: u16 = 0x0007;
    const KEY_INFO_VER_CCMP: u16 = 0x0002;
    const KEY_INFO_PAIRWISE: u16 = 0x0008;
    const KEY_INFO_INSTALL: u16 = 0x0040;
    const KEY_INFO_ACK: u16 = 0x0080;
    const KEY_INFO_MIC: u16 = 0x0100;
    const KEY_INFO_SECURE: u16 = 0x0200;
    const KEY_INFO_ENCRYPTED: u16 = 0x1000;

    /// Result of feeding an incoming EAPOL PDU into `Wpa2Handshake::process_eapol`.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum HandshakeStep {
        Ignore,
        SendM2(usize),
        SendM4 {
            len: usize,
            tk: [u8; 16],
            gtk: [u8; 16],
            gtk_idx: u8,
        },
        /// Post-completion group-key rotation: the AP sent a valid group
        /// handshake message carrying a new GTK. The caller installs it at
        /// `gtk_idx` in place of the connect-time GTK and transmits the
        /// group message 2 held in `reply_out[..reply_len]`: the rekey is a
        /// two-message handshake, and an AP that never receives message 2
        /// retries message 1 and then commonly deauthenticates the station
        /// for a failed group handshake.
        RotateGtk {
            gtk: [u8; 16],
            gtk_idx: u8,
            reply_len: usize,
        },
        Failed,
    }

    /// Zero-heap WPA2-PSK 4-Way Handshake state machine (CCMP-128).
    pub struct Wpa2Handshake {
        pmk: [u8; 32],
        sta_mac: [u8; 6],
        bssid: [u8; 6],
        snonce: [u8; 32],
        anonce: [u8; 32],
        ptk: Option<Ptk>,
        last_replay: u64,
        // Set once M4 has been produced. From then on the pairwise exchange
        // is closed: the machine only accepts group-key rotations, and any
        // further M1/M3 is ignored rather than re-deriving keys from the
        // stored SNonce (the KRACK reinstallation pattern).
        completed: bool,
    }

    impl Wpa2Handshake {
        pub fn new(
            pmk: [u8; 32],
            sta_mac: [u8; 6],
            bssid: [u8; 6],
            snonce: [u8; 32],
        ) -> Self {
            Wpa2Handshake {
                pmk,
                sta_mac,
                bssid,
                snonce,
                anonce: [0u8; 32],
                ptk: None,
                last_replay: 0,
                completed: false,
            }
        }

        /// Process an incoming 802.1X EAPOL PDU (without Ethernet/SNAP header).
        /// On `SendM2(len)` or `SendM4 { len, .. }`, the reply EAPOL PDU is written
        /// into `reply_out[..len]`.
        pub fn process_eapol(&mut self, eapol: &[u8], reply_out: &mut [u8]) -> HandshakeStep {
            if eapol.len() < EAPOL_KEY_FIXED_LEN {
                return HandshakeStep::Ignore;
            }
            // Packet type 0x03 = EAPOL-Key, descriptor type 0x02 = RSN (802.11i).
            if eapol[1] != 0x03 || eapol[4] != 0x02 {
                return HandshakeStep::Ignore;
            }
            let body_len = u16::from_be_bytes([eapol[2], eapol[3]]) as usize;
            let pdu_len = 4 + body_len;
            if body_len < EAPOL_KEY_BODY_MIN || eapol.len() < pdu_len {
                return HandshakeStep::Ignore;
            }
            let key_data_len = u16::from_be_bytes([eapol[97], eapol[98]]) as usize;
            if EAPOL_KEY_BODY_MIN + key_data_len != body_len {
                return HandshakeStep::Ignore;
            }

            let key_info = u16::from_be_bytes([eapol[5], eapol[6]]);
            if (key_info & KEY_INFO_VER_MASK) != KEY_INFO_VER_CCMP {
                return HandshakeStep::Ignore;
            }
            // The Pairwise bit separates the 4-way handshake (set) from the
            // group-key handshake (clear); both are processed below.
            let pairwise = (key_info & KEY_INFO_PAIRWISE) != 0;

            let install = (key_info & KEY_INFO_INSTALL) != 0;
            let ack = (key_info & KEY_INFO_ACK) != 0;
            let mic = (key_info & KEY_INFO_MIC) != 0;
            let secure = (key_info & KEY_INFO_SECURE) != 0;
            let encrypted = (key_info & KEY_INFO_ENCRYPTED) != 0;

            let replay = crate::u64_from_be([
                eapol[9], eapol[10], eapol[11], eapol[12], eapol[13], eapol[14], eapol[15],
                eapol[16],
            ]);

            // Message 1: Ack=1, MIC=0, Install=0, Secure=0
            if pairwise && ack && !mic && !install && !secure {
                // After completion the pairwise exchange is closed. Letting a
                // later M1 — a replayed capture, or an AP restarting the
                // exchange — reset the replay floor and re-derive a PTK from
                // the stored SNonce would hand an attacker the KRACK key
                // reinstallation pattern, so every post-completion M1 is
                // ignored. A genuine new session is the caller's job: it
                // builds a fresh machine with a fresh SNonce. Before
                // completion, an M1 older than the one already answered is
                // stale for the same reason; an equal replay is the AP
                // retransmitting because M2 was lost, and is answered again.
                if self.completed || replay < self.last_replay {
                    return HandshakeStep::Ignore;
                }
                if reply_out.len() < M2_LEN {
                    return HandshakeStep::Failed;
                }
                let mut anonce = [0u8; 32];
                anonce.copy_from_slice(&eapol[17..49]);
                let ptk = derive_ptk(&self.pmk, &self.bssid, &self.sta_mac, &anonce, &self.snonce);
                self.anonce = anonce;
                self.ptk = Some(ptk);
                self.last_replay = replay;

                reply_out[..M2_LEN].fill(0);
                reply_out[0] = 0x01; // 802.1X-2001
                reply_out[1] = 0x03; // EAPOL-Key
                reply_out[2..4].copy_from_slice(&((M2_LEN - 4) as u16).to_be_bytes());
                reply_out[4] = 0x02; // RSN Key Descriptor
                let m2_info: u16 = KEY_INFO_VER_CCMP | KEY_INFO_PAIRWISE | KEY_INFO_MIC;
                reply_out[5..7].copy_from_slice(&m2_info.to_be_bytes());
                reply_out[7..9].copy_from_slice(&16u16.to_be_bytes()); // CCMP-128 key len
                reply_out[9..17].copy_from_slice(&crate::u64_to_be(replay));
                reply_out[17..49].copy_from_slice(&self.snonce);
                reply_out[97..99].copy_from_slice(&(WPA2_RSN_IE.len() as u16).to_be_bytes());
                reply_out[99..M2_LEN].copy_from_slice(&WPA2_RSN_IE);

                let m2_mic = eapol_key_mic(&ptk.kck, &reply_out[..M2_LEN]);
                reply_out[81..97].copy_from_slice(&m2_mic);
                return HandshakeStep::SendM2(M2_LEN);
            }

            // Message 3: Ack=1, MIC=1, Install=1, Secure=1, EncryptedKeyData=1
            if pairwise && ack && mic && install && secure && encrypted {
                let ptk = match self.ptk {
                    Some(p) => p,
                    None => return HandshakeStep::Ignore,
                };
                // A second M3 after completion would reinstall the same TK;
                // only the first one counts.
                if self.completed {
                    return HandshakeStep::Ignore;
                }
                if replay <= self.last_replay {
                    return HandshakeStep::Ignore;
                }
                if eapol[17..49] != self.anonce {
                    return HandshakeStep::Failed;
                }

                let (gtk, gtk_idx) = match self.extract_gtk(eapol, pdu_len, key_data_len) {
                    Some(found) => found,
                    None => return HandshakeStep::Failed,
                };

                if reply_out.len() < M4_LEN {
                    return HandshakeStep::Failed;
                }
                self.last_replay = replay;
                self.completed = true;

                reply_out[..M4_LEN].fill(0);
                reply_out[0] = 0x01; // 802.1X-2001
                reply_out[1] = 0x03; // EAPOL-Key
                reply_out[2..4].copy_from_slice(&((M4_LEN - 4) as u16).to_be_bytes());
                reply_out[4] = 0x02; // RSN Key Descriptor
                let m4_info: u16 =
                    KEY_INFO_VER_CCMP | KEY_INFO_PAIRWISE | KEY_INFO_MIC | KEY_INFO_SECURE;
                reply_out[5..7].copy_from_slice(&m4_info.to_be_bytes());
                // Per IEEE 802.11i §11.6.6.5 (Message 4): Key Length MUST be 0
                // (reply_out[7..9] is already zeroed above).
                reply_out[9..17].copy_from_slice(&crate::u64_to_be(replay));

                let m4_mic = eapol_key_mic(&ptk.kck, &reply_out[..M4_LEN]);
                reply_out[81..97].copy_from_slice(&m4_mic);

                return HandshakeStep::SendM4 {
                    len: M4_LEN,
                    tk: ptk.tk,
                    gtk,
                    gtk_idx,
                };
            }

            // Group-key handshake message 1: Pairwise=0, Ack=1, MIC=1,
            // Secure=1, EncryptedKeyData=1. The AP sends this when it rotates
            // the GTK mid-session; the new GTK travels in the key data,
            // wrapped with the same KEK as M3. Accepting it is what gives the
            // session a post-connect life — without it, the first AP rekey
            // silently kills all broadcast/multicast decryption while unicast
            // keeps working. A frame that fails authentication here is
            // ignored, not fatal: it may be a leftover from a prior session
            // whose PTK no longer matches, and the live session must survive.
            // Accepting the key is only half of the exchange: the AP expects
            // message 2 in reply, and an unanswered rekey is retried and then
            // commonly treated as a failed handshake, ending in a deauth —
            // so the reply is built here and RotateGtk carries its length.
            if !pairwise && ack && mic && secure && encrypted {
                if !self.completed || self.ptk.is_none() {
                    return HandshakeStep::Ignore;
                }
                if replay <= self.last_replay {
                    return HandshakeStep::Ignore;
                }
                let (gtk, gtk_idx) = match self.extract_gtk(eapol, pdu_len, key_data_len) {
                    Some(found) => found,
                    None => return HandshakeStep::Ignore,
                };
                if reply_out.len() < M4_LEN {
                    return HandshakeStep::Ignore;
                }
                let ptk = match self.ptk {
                    Some(found) => found,
                    None => return HandshakeStep::Ignore,
                };
                // Group message 2 has the confirmation shape of M4 with the
                // Pairwise bit clear: MIC + Secure, key length zero (the
                // 802.11i rule for confirmations), no key data, and the
                // replay counter echoing message 1's.
                reply_out[..M4_LEN].fill(0);
                reply_out[0] = 0x01; // 802.1X-2001
                reply_out[1] = 0x03; // EAPOL-Key
                reply_out[2..4].copy_from_slice(&((M4_LEN - 4) as u16).to_be_bytes());
                reply_out[4] = 0x02; // RSN Key Descriptor
                let gm2_info: u16 = KEY_INFO_VER_CCMP | KEY_INFO_MIC | KEY_INFO_SECURE;
                reply_out[5..7].copy_from_slice(&gm2_info.to_be_bytes());
                reply_out[9..17].copy_from_slice(&crate::u64_to_be(replay));
                let gm2_mic = eapol_key_mic(&ptk.kck, &reply_out[..M4_LEN]);
                reply_out[81..97].copy_from_slice(&gm2_mic);
                self.last_replay = replay;
                return HandshakeStep::RotateGtk {
                    gtk,
                    gtk_idx,
                    reply_len: M4_LEN,
                };
            }

            HandshakeStep::Ignore
        }

        /// Verify the EAPOL-Key MIC and unwrap the key data, returning the
        /// GTK and its key index. Shared by M3 and the group-key message,
        /// which differ only in the Pairwise bit and in what the caller does
        /// with the result.
        fn extract_gtk(
            &self,
            eapol: &[u8],
            pdu_len: usize,
            key_data_len: usize,
        ) -> Option<([u8; 16], u8)> {
            let ptk = self.ptk?;

            // Verify HMAC-SHA1-128 MIC over the PDU with bytes 81..97 zeroed.
            let zero_mic = [0u8; 16];
            let digest = hmac_sha1(&ptk.kck, &[&eapol[..81], &zero_mic, &eapol[97..pdu_len]]);
            let mut diff = 0u8;
            for i in 0..16 {
                diff |= digest[i] ^ eapol[81 + i];
            }
            if diff != 0 {
                return None;
            }

            // Unwrap Key Data with KEK (RFC 3394).
            let wrapped = &eapol[99..99 + key_data_len];
            if wrapped.len() < 16 || !wrapped.len().is_multiple_of(8) {
                return None;
            }
            let plain_len = wrapped.len() - 8;
            let mut unwrapped = [0u8; 256];
            if plain_len > unwrapped.len() {
                return None;
            }
            if !aes_unwrap(&ptk.kek, wrapped, &mut unwrapped[..plain_len]) {
                return None;
            }

            find_gtk_kde(&unwrapped[..plain_len])
        }
    }

    /// Scan unwrapped M3 Key Data for the GTK KDE (`DD <len> 00 0F AC 01 <key_id> <rsv> <gtk[16]>`).
    fn find_gtk_kde(key_data: &[u8]) -> Option<([u8; 16], u8)> {
        let mut pos = 0;
        while pos < key_data.len() {
            if key_data[pos] == 0xDD && pos + 1 < key_data.len() && key_data[pos + 1] == 0x00 {
                // IEEE 802.11i 0xDD 0x00... padding terminator.
                break;
            }
            if pos + 2 > key_data.len() {
                break;
            }
            let ie_type = key_data[pos];
            let ie_len = key_data[pos + 1] as usize;
            if pos + 2 + ie_len > key_data.len() {
                break;
            }
            if ie_type == 0xDD
                && ie_len >= 22
                && key_data[pos + 2..pos + 6] == [0x00, 0x0F, 0xAC, 0x01]
            {
                let key_id_byte = key_data[pos + 6];
                let gtk_idx = key_id_byte & 0x03;
                let mut gtk = [0u8; 16];
                gtk.copy_from_slice(&key_data[pos + 8..pos + 24]);
                return Some((gtk, gtk_idx));
            }
            pos += 2 + ie_len;
        }
        None
    }
}

// ---------------------------------------------------------------------
// Tests: every assertion is anchored to a published vector or a real
// captured handshake, never to this implementation's own output.
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::aes::*;
    use super::frame::*;
    use super::wpa::*;
    use super::*;

    fn hex(s: &str) -> std::vec::Vec<u8> {
        let b = s.as_bytes();
        assert!(b.len() % 2 == 0);
        (0..b.len() / 2)
            .map(|i| {
                let hi = (b[2 * i] as char).to_digit(16).unwrap() as u8;
                let lo = (b[2 * i + 1] as char).to_digit(16).unwrap() as u8;
                (hi << 4) | lo
            })
            .collect()
    }

    fn hex32(s: &str) -> [u8; 32] {
        let v = hex(s);
        let mut out = [0u8; 32];
        out.copy_from_slice(&v);
        out
    }

    #[test]
    fn sha1_fips_vectors() {
        assert_eq!(
            sha1::sha1(b"abc"),
            hex("a9993e364706816aba3e25717850c26c9cd0d89d").as_slice()
        );
        assert_eq!(
            sha1::sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            hex("84983e441c3bd26ebaae4aa1f95129e5e54670f1").as_slice()
        );
        // Streaming in odd chunks must agree with the one-shot path.
        let mut h = sha1::Sha1::new();
        for chunk in b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq".chunks(7) {
            h.update(chunk);
        }
        assert_eq!(
            h.finalize(),
            hex("84983e441c3bd26ebaae4aa1f95129e5e54670f1").as_slice()
        );
    }

    #[test]
    fn hmac_sha1_rfc2202() {
        assert_eq!(
            hmac_sha1(&[0x0B; 20], &[b"Hi There"]),
            hex("b617318655057264e28bc0b6fb378c8ef146be00").as_slice()
        );
        assert_eq!(
            hmac_sha1(b"Jefe", &[b"what do ya want for nothing?"]),
            hex("effcdf6ae5eb2fa2d27416d5f184df9c259a7c79").as_slice()
        );
        // Key longer than the block size (hash-first path).
        assert_eq!(
            hmac_sha1(
                &[0xAA; 80],
                &[b"Test Using Larger Than Block-Size Key - Hash Key First"]
            ),
            hex("aa4ae5e15272d00e95705637ce8a3b55ed402112").as_slice()
        );
    }

    #[test]
    fn pbkdf2_rfc6070() {
        let mut out = [0u8; 20];
        pbkdf2_hmac_sha1(b"password", b"salt", 1, &mut out);
        assert_eq!(out, hex("0c60c80f961f0e71f3a9b524af6012062fe037a6").as_slice());
        pbkdf2_hmac_sha1(b"password", b"salt", 2, &mut out);
        assert_eq!(out, hex("ea6c014dc72d6f8ccd1ed92ace1d41f0d8de8957").as_slice());
        pbkdf2_hmac_sha1(b"password", b"salt", 4096, &mut out);
        assert_eq!(out, hex("4b007901b765489abead49d926f721d065a429c1").as_slice());
    }

    #[test]
    fn pmk_published_vectors() {
        // The widely published WPA example vector.
        assert_eq!(
            derive_pmk(b"password", b"IEEE"),
            hex32("f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e")
        );
        // From the captured "Harkonen" handshake (see module docs).
        assert_eq!(
            derive_pmk(b"12345678", b"Harkonen"),
            hex32("ee51883793a6f68e9615fe73c80a3aa6f2dd0ea537bce627b929183cc6e57925")
        );
    }

    #[test]
    fn aes_fips197() {
        let key: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let aes = Aes128::new(&key);
        let mut block: [u8; 16] =
            hex("00112233445566778899aabbccddeeff").try_into().unwrap();
        aes.encrypt_block(&mut block);
        assert_eq!(
            block,
            hex("69c4e0d86a7b0430d8cdb78070b4c55a").as_slice()
        );
        aes.decrypt_block(&mut block);
        assert_eq!(
            block,
            hex("00112233445566778899aabbccddeeff").as_slice()
        );
    }

    #[test]
    fn aes_unwrap_rfc3394() {
        let kek: [u8; 16] = hex("000102030405060708090a0b0c0d0e0f").try_into().unwrap();
        let wrapped = hex("1fa68b0a8112b447aef34bd8fb5a7b829d3e862371d2cfe5");
        let mut out = [0u8; 16];
        assert!(aes_unwrap(&kek, &wrapped, &mut out));
        assert_eq!(out, hex("00112233445566778899aabbccddeeff").as_slice());
        // A corrupted wrap must fail the integrity check.
        let mut bad = wrapped.clone();
        bad[3] ^= 1;
        assert!(!aes_unwrap(&kek, &bad, &mut out));
    }

    // The captured-handshake vector set: PMK, nonces, addresses and
    // the three EAPOL PDUs (MIC fields zeroed) from a real WPA2
    // 4-way handshake. The MICs are the ground truth: if PMK, PRF,
    // PTK or MIC handling is wrong anywhere, all three fail.
    struct Harkonen {
        pmk: [u8; 32],
        aa: [u8; 6],
        sa: [u8; 6],
        anonce: [u8; 32],
        snonce: [u8; 32],
    }

    fn harkonen() -> Harkonen {
        Harkonen {
            pmk: derive_pmk(b"12345678", b"Harkonen"),
            aa: hex("00146c7e4080").try_into().unwrap(),
            sa: hex("001346fe320c").try_into().unwrap(),
            anonce: hex32("225854b0444de3af06d1492b852984f04cf6274c0e3218b8681756864db7a055"),
            snonce: hex32("59168bc3a5df18d71efb6423f340088dab9e1ba2bbc58659e07b3764b0de8570"),
        }
    }

    #[test]
    fn ptk_from_captured_handshake() {
        let h = harkonen();
        let ptk = derive_ptk(&h.pmk, &h.aa, &h.sa, &h.anonce, &h.snonce);
        // First 48 bytes of the published 64-byte PTK for this
        // capture (the PRF stream's prefix is length-independent).
        let mut raw = [0u8; 48];
        raw[0..16].copy_from_slice(&ptk.kck);
        raw[16..32].copy_from_slice(&ptk.kek);
        raw[32..48].copy_from_slice(&ptk.tk);
        assert_eq!(
            raw,
            hex("ea0e404633c802450302868ccaa749de5cba5abcb267e2de1d5e21e57accd5079b31e9ff220e132ae4f6ed9ef1acc885")
                .as_slice()
        );
    }

    #[test]
    fn mics_from_captured_handshake() {
        let h = harkonen();
        let ptk = derive_ptk(&h.pmk, &h.aa, &h.sa, &h.anonce, &h.snonce);
        let msg2 = hex("0103007502010a0010000000000000000159168bc3a5df18d71efb6423f340088dab9e1ba2bbc58659e07b3764b0de8570000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000001630140100000fac040100000fac040100000fac020100");
        assert_eq!(
            eapol_key_mic(&ptk.kck, &msg2),
            hex("d5355382b8a9b806dcaf99cdaf564eb6").as_slice()
        );
        let msg3 = hex("010300970213ca00100000000000000002225854b0444de3af06d1492b852984f04cf6274c0e3218b8681756864db7a055192eeef7fd968ec80aee3dfb875e8222370000000000000000000000000000000000000000000000000000000000000000383ca9185462eca4ab7ff51cd3a3e6179a8391f5ad824c9e09763794c680902ad3bf0703452fbb7c1f5f1ee9f5bbd388ae559e78d27e6b121f");
        assert_eq!(
            eapol_key_mic(&ptk.kck, &msg3),
            hex("1e228672d2dee930714f688c5746028d").as_slice()
        );
        let msg4 = hex("0103005f02030a0010000000000000000200000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000");
        assert_eq!(
            eapol_key_mic(&ptk.kck, &msg4),
            hex("9dc81ca6c4c729648de7f00b436335c8").as_slice()
        );
        // The wrong passphrase must NOT reproduce the MICs: this is
        // the negative test the Gate 4b acceptance exercises live.
        let wrong = derive_pmk(b"abcdefgh", b"Harkonen");
        let wrong_ptk = derive_ptk(&wrong, &h.aa, &h.sa, &h.anonce, &h.snonce);
        assert_ne!(
            eapol_key_mic(&wrong_ptk.kck, &msg2),
            hex("d5355382b8a9b806dcaf99cdaf564eb6").as_slice()
        );
    }

    #[test]
    fn beacon_parse_synthetic() {
        // Hand-built beacon: BSSID 00:14:6c:7e:40:80, SSID
        // "Harkonen", channel 6, RSN IE present, privacy set.
        let mut frame = [0u8; 24 + 12 + 2 + 8 + 2 + 1 + 2 + 22];
        let bssid = [0x00, 0x14, 0x6C, 0x7E, 0x40, 0x80];
        frame[0] = 0x80; // mgmt, beacon
        frame[4..10].copy_from_slice(&[0xFF; 6]);
        frame[10..16].copy_from_slice(&bssid);
        frame[16..22].copy_from_slice(&bssid);
        let body = 24;
        frame[body + 10] = 0x31; // capability: ESS + privacy
        frame[body + 11] = 0x04;
        let mut n = body + 12;
        frame[n] = 0;
        frame[n + 1] = 8;
        frame[n + 2..n + 10].copy_from_slice(b"Harkonen");
        n += 10;
        frame[n] = 3;
        frame[n + 1] = 1;
        frame[n + 2] = 6;
        n += 3;
        frame[n] = 48;
        frame[n + 1] = 20;
        frame[n + 2] = 1; // RSN version 1 (rest zeros for this test)
        let info = parse_beacon(&frame).expect("beacon parses");
        assert_eq!(info.bssid, bssid);
        assert_eq!(info.ssid, b"Harkonen");
        assert_eq!(info.channel, Some(6));
        assert!(info.rsn.is_some());
        assert_eq!(info.capability & 0x10, 0x10);
    }

    #[test]
    fn frame_builders_round_trip() {
        let sa = [0x00, 0x13, 0x46, 0xFE, 0x32, 0x0C];
        let bssid = [0x00, 0x14, 0x6C, 0x7E, 0x40, 0x80];
        let mut buf = [0u8; 256];
        let n = build_auth(&mut buf, &sa, &bssid, 1);
        assert_eq!(n, 30);
        let (hdr, _) = parse_header(&buf[..n]).unwrap();
        assert_eq!(hdr.subtype, SUB_AUTH);
        assert_eq!(hdr.addr2, sa);
        assert_eq!(&buf[24..30], &[0, 0, 1, 0, 0, 0]);

        let n = build_data_tods(
            &mut buf,
            &sa,
            &bssid,
            &[0x00, 0x14, 0x6C, 0x7E, 0x40, 0x80],
            ETHERTYPE_EAPOL,
            &[1, 3, 0, 95],
        );
        let (hdr, off) = parse_header(&buf[..n]).unwrap();
        assert!(hdr.to_ds && !hdr.from_ds);
        let (et, payload) = snap_decap(&buf[off..n]).unwrap();
        assert_eq!(et, ETHERTYPE_EAPOL);
        assert_eq!(payload, &[1, 3, 0, 95]);
    }

    #[test]
    fn assoc_req_rsn_open_and_wpa2() {
        let sa = [0x00, 0x13, 0x46, 0xFE, 0x32, 0x0C];
        let bssid = [0x00, 0x14, 0x6C, 0x7E, 0x40, 0x80];
        let ssid = b"FerricNet";
        let mut buf = [0u8; 256];

        // With RSN IE
        let n = build_assoc_req_rsn(&mut buf, &sa, &bssid, ssid, 0x123, true);
        let expected_len = 24 + 4 + (2 + ssid.len()) + (2 + 4) + 22;
        assert_eq!(n, expected_len);
        let (hdr, off) = parse_header(&buf[..n]).unwrap();
        assert_eq!(hdr.frame_type, TYPE_MGMT);
        assert_eq!(hdr.subtype, SUB_ASSOC_REQ);
        assert_eq!(hdr.addr1, bssid);
        assert_eq!(hdr.addr2, sa);
        assert_eq!(hdr.addr3, bssid);
        assert_eq!(hdr.seq, 0x123 << 4);
        assert_eq!(u16::from_le_bytes([buf[off], buf[off + 1]]), 0x0011);
        assert_eq!(u16::from_le_bytes([buf[off + 2], buf[off + 3]]), 10);

        let mut saw_ssid = false;
        let mut saw_rates = false;
        let mut saw_rsn = false;
        for_each_ie(&buf[off + 4..n], |id, payload| {
            match id {
                0 => {
                    assert_eq!(payload, ssid);
                    saw_ssid = true;
                }
                1 => {
                    assert_eq!(payload, &[0x82, 0x84, 0x8B, 0x96]);
                    saw_rates = true;
                }
                48 => {
                    assert_eq!(payload, &WPA2_RSN_IE[2..]);
                    saw_rsn = true;
                }
                _ => panic!("unexpected IE id {id}"),
            }
            true
        });
        assert!(saw_ssid && saw_rates && saw_rsn);

        // Open network (no RSN IE)
        let n_open = build_assoc_req_rsn(&mut buf, &sa, &bssid, ssid, 7, false);
        assert_eq!(n_open, expected_len - 22);
        assert_eq!(u16::from_le_bytes([buf[24], buf[25]]), 0x0001);

        // Buffer too small returns 0
        let mut tiny = [0u8; 32];
        assert_eq!(build_assoc_req_rsn(&mut tiny, &sa, &bssid, ssid, 1, true), 0);
    }

    #[test]
    fn parse_auth_and_assoc_responses() {
        let mut frame = [0u8; 30];
        // Auth response: subtype 11 (0xB0), algo=0, seq=2, status=0
        frame[0] = SUB_AUTH << 4;
        frame[24..26].copy_from_slice(&0u16.to_le_bytes());
        frame[26..28].copy_from_slice(&2u16.to_le_bytes());
        frame[28..30].copy_from_slice(&0u16.to_le_bytes());
        assert_eq!(parse_auth_resp(&frame), Some((2, 0)));
        assert_eq!(parse_assoc_resp(&frame), None);

        // Auth failure status
        frame[28..30].copy_from_slice(&13u16.to_le_bytes());
        assert_eq!(parse_auth_resp(&frame), Some((2, 13)));
        assert_eq!(parse_auth_resp(&frame[..29]), None);

        // Assoc response: subtype 1 (0x10), cap=0x0431, status=0, aid=0xC005
        frame[0] = SUB_ASSOC_RESP << 4;
        frame[24..26].copy_from_slice(&0x0431u16.to_le_bytes());
        frame[26..28].copy_from_slice(&0u16.to_le_bytes());
        frame[28..30].copy_from_slice(&0xC005u16.to_le_bytes());
        assert_eq!(parse_assoc_resp(&frame), Some((0, 5)));
        assert_eq!(parse_auth_resp(&frame), None);
        assert_eq!(parse_assoc_resp(&frame[..29]), None);
    }

    #[test]
    fn eth_80211_encap_decap_open_and_ccmp_qos() {
        let bssid = [0x00, 0x14, 0x6C, 0x7E, 0x40, 0x80];
        let sa = [0x00, 0x13, 0x46, 0xFE, 0x32, 0x0C];
        let dst = [0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01];

        let mut eth = [0u8; 14 + 6];
        eth[0..6].copy_from_slice(&dst);
        eth[6..12].copy_from_slice(&sa);
        eth[12..14].copy_from_slice(&0x0806u16.to_be_bytes()); // ARP
        eth[14..20].copy_from_slice(&[1, 2, 3, 4, 5, 6]);

        // 1. Open (unprotected) encap
        let mut wlan = [0u8; 128];
        let n = encap_eth_to_80211(&mut wlan, &eth, &bssid, &sa, 42, None);
        assert_eq!(n, 24 + 8 + 6);
        let (hdr, off) = parse_header(&wlan[..n]).unwrap();
        assert_eq!(hdr.frame_type, TYPE_DATA);
        assert_eq!(hdr.subtype, 0);
        assert!(hdr.to_ds && !hdr.from_ds && !hdr.protected);
        assert_eq!(hdr.addr1, bssid);
        assert_eq!(hdr.addr2, sa);
        assert_eq!(hdr.addr3, dst);
        assert_eq!(hdr.seq, 42 << 4);
        let (et, body) = snap_decap(&wlan[off..n]).unwrap();
        assert_eq!(et, 0x0806);
        assert_eq!(body, &[1, 2, 3, 4, 5, 6]);

        // Flip to FromDS (AP -> STA: addr1=sa, addr2=bssid, addr3=dst) and decap
        let mut rx_open = [0u8; 128];
        rx_open[..n].copy_from_slice(&wlan[..n]);
        let fc_fromds: u16 = ((TYPE_DATA as u16) << 2) | 0x0200;
        rx_open[0..2].copy_from_slice(&fc_fromds.to_le_bytes());
        rx_open[4..10].copy_from_slice(&sa);
        rx_open[10..16].copy_from_slice(&bssid);
        rx_open[16..22].copy_from_slice(&dst);
        let mut eth_out = [0u8; 64];
        let eth_len = decap_80211_to_eth(&mut eth_out, &rx_open[..n], &bssid).unwrap();
        assert_eq!(eth_len, 20);
        assert_eq!(&eth_out[0..6], &sa);
        assert_eq!(&eth_out[6..12], &dst);
        assert_eq!(&eth_out[12..14], &0x0806u16.to_be_bytes());
        assert_eq!(&eth_out[14..20], &[1, 2, 3, 4, 5, 6]);

        // Wrong BSSID must be rejected
        assert_eq!(
            decap_80211_to_eth(&mut eth_out, &rx_open[..n], &[0xFF; 6]),
            None
        );

        // 2. CCMP-protected encap
        let pn: u64 = 0x0000_0605_0403_0201;
        let key_idx: u8 = 2;
        let n_ccmp = encap_eth_to_80211(&mut wlan, &eth, &bssid, &sa, 99, Some((pn, key_idx)));
        assert_eq!(n_ccmp, 24 + 8 + 8 + 6);
        let (hdr, off) = parse_header(&wlan[..n_ccmp]).unwrap();
        assert!(hdr.to_ds && !hdr.from_ds && hdr.protected);
        assert_eq!(hdr.seq, 99 << 4);
        // Check 8-byte CCMP header: PN0, PN1, 0x00, 0x20|(key_idx<<6), PN2..PN5
        assert_eq!(
            &wlan[off..off + 8],
            &[0x01, 0x02, 0x00, 0x20 | (2 << 6), 0x03, 0x04, 0x05, 0x06]
        );
        let (et, body) = snap_decap(&wlan[off + 8..n_ccmp]).unwrap();
        assert_eq!(et, 0x0806);
        assert_eq!(body, &[1, 2, 3, 4, 5, 6]);

        // 3. Decap FromDS QoS Data (subtype 8) + Protected (CCMP header)
        let mut rx_qos = [0u8; 128];
        let fc_qos_ccmp: u16 = ((TYPE_DATA as u16) << 2) | (8u16 << 4) | 0x0200 | 0x4000;
        rx_qos[0..2].copy_from_slice(&fc_qos_ccmp.to_le_bytes());
        rx_qos[4..10].copy_from_slice(&sa);
        rx_qos[10..16].copy_from_slice(&bssid);
        rx_qos[16..22].copy_from_slice(&dst);
        rx_qos[24..26].copy_from_slice(&0x0000u16.to_le_bytes()); // 2-byte QoS Control
        rx_qos[26..34].copy_from_slice(&wlan[24..32]); // 8-byte CCMP header
        rx_qos[34..48].copy_from_slice(&wlan[32..46]); // 8-byte SNAP + 6-byte payload
        let eth_len = decap_80211_to_eth(&mut eth_out, &rx_qos[..48], &bssid).unwrap();
        assert_eq!(eth_len, 20);
        assert_eq!(&eth_out[0..6], &sa);
        assert_eq!(&eth_out[6..12], &dst);
        assert_eq!(&eth_out[12..14], &0x0806u16.to_be_bytes());
        assert_eq!(&eth_out[14..20], &[1, 2, 3, 4, 5, 6]);
    }

    /// RFC 3394 AES Key Wrap helper for synthetic AP test frames.
    fn aes_wrap_test(kek: &[u8; 16], plain: &[u8]) -> std::vec::Vec<u8> {
        assert!(plain.len() >= 16 && plain.len() % 8 == 0);
        let n = plain.len() / 8;
        let aes = Aes128::new(kek);
        let mut a = [0xA6u8; 8];
        let mut r = plain.to_vec();
        for j in 0..=5u64 {
            for i in 1..=n as u64 {
                let mut block = [0u8; 16];
                block[..8].copy_from_slice(&a);
                block[8..].copy_from_slice(&r[8 * (i as usize - 1)..8 * i as usize]);
                aes.encrypt_block(&mut block);
                let t = n as u64 * j + i;
                let a_val = u64::from_be_bytes(block[..8].try_into().unwrap()) ^ t;
                a = a_val.to_be_bytes();
                r[8 * (i as usize - 1)..8 * i as usize].copy_from_slice(&block[8..]);
            }
        }
        let mut out = std::vec::Vec::with_capacity(8 + plain.len());
        out.extend_from_slice(&a);
        out.extend_from_slice(&r);
        out
    }

    #[test]
    fn full_wpa2_4way_handshake_and_wrong_passphrase() {
        use super::handshake::{HandshakeStep, Wpa2Handshake};

        let pmk = derive_pmk(b"correct-horse-battery", b"FerricAP");
        let sta_mac = [0x00, 0x13, 0x46, 0xFE, 0x32, 0x0C];
        let bssid = [0x00, 0x14, 0x6C, 0x7E, 0x40, 0x80];
        let snonce = [0x5Au8; 32];
        let anonce = [0xA5u8; 32];
        let expected_gtk: [u8; 16] = [
            0x10, 0x21, 0x32, 0x43, 0x54, 0x65, 0x76, 0x87, 0x98, 0xA9, 0xBA, 0xCB, 0xDC, 0xED,
            0xFE, 0x0F,
        ];
        let expected_gtk_idx: u8 = 2;

        // AP-side expected PTK
        let ap_ptk = derive_ptk(&pmk, &bssid, &sta_mac, &anonce, &snonce);

        // Build M1 (AP -> STA): replay=1, ANonce, KeyInfo=0x008A
        let mut m1 = [0u8; 99];
        m1[0] = 0x01;
        m1[1] = 0x03;
        m1[2..4].copy_from_slice(&95u16.to_be_bytes());
        m1[4] = 0x02;
        m1[5..7].copy_from_slice(&0x008Au16.to_be_bytes());
        m1[7..9].copy_from_slice(&16u16.to_be_bytes());
        m1[9..17].copy_from_slice(&1u64.to_be_bytes());
        m1[17..49].copy_from_slice(&anonce);

        let mut hs = Wpa2Handshake::new(pmk, sta_mac, bssid, snonce);
        let mut reply = [0u8; 256];

        // Step 1: Process M1 -> SendM2
        let m2_len = match hs.process_eapol(&m1, &mut reply) {
            HandshakeStep::SendM2(len) => len,
            other => panic!("expected SendM2, got {other:?}"),
        };
        assert_eq!(m2_len, 121);
        assert_eq!(&reply[9..17], &1u64.to_be_bytes());
        assert_eq!(&reply[17..49], &snonce);
        assert_eq!(&reply[99..121], &WPA2_RSN_IE);

        // AP verifies M2 MIC
        let mut m2_zeroed = [0u8; 121];
        m2_zeroed.copy_from_slice(&reply[..121]);
        m2_zeroed[81..97].fill(0);
        assert_eq!(eapol_key_mic(&ap_ptk.kck, &m2_zeroed), reply[81..97]);

        // Build M3 (AP -> STA): replay=2, ANonce, KeyInfo=0x13CA,
        // Key Data = WPA2_RSN_IE (22) + GTK KDE (24) + padding DD 00 (2) = 48 bytes
        let mut kd_plain = [0u8; 48];
        kd_plain[0..22].copy_from_slice(&WPA2_RSN_IE);
        kd_plain[22] = 0xDD;
        kd_plain[23] = 22;
        kd_plain[24..28].copy_from_slice(&[0x00, 0x0F, 0xAC, 0x01]);
        kd_plain[28] = expected_gtk_idx; // key_id_byte
        kd_plain[29] = 0x00; // reserved
        kd_plain[30..46].copy_from_slice(&expected_gtk);
        kd_plain[46] = 0xDD; // padding
        kd_plain[47] = 0x00;
        let kd_wrapped = aes_wrap_test(&ap_ptk.kek, &kd_plain);
        assert_eq!(kd_wrapped.len(), 56);

        let mut m3 = std::vec![0u8; 99 + kd_wrapped.len()];
        m3[0] = 0x01;
        m3[1] = 0x03;
        m3[2..4].copy_from_slice(&((95 + kd_wrapped.len()) as u16).to_be_bytes());
        m3[4] = 0x02;
        m3[5..7].copy_from_slice(&0x13CAu16.to_be_bytes());
        m3[7..9].copy_from_slice(&16u16.to_be_bytes());
        m3[9..17].copy_from_slice(&2u64.to_be_bytes());
        m3[17..49].copy_from_slice(&anonce);
        m3[97..99].copy_from_slice(&(kd_wrapped.len() as u16).to_be_bytes());
        m3[99..].copy_from_slice(&kd_wrapped);
        let m3_mic = eapol_key_mic(&ap_ptk.kck, &m3);
        m3[81..97].copy_from_slice(&m3_mic);

        // Step 2: Process M3 -> SendM4 with TK, GTK, GTK index
        match hs.process_eapol(&m3, &mut reply) {
            HandshakeStep::SendM4 {
                len,
                tk,
                gtk,
                gtk_idx,
            } => {
                assert_eq!(len, 99);
                assert_eq!(tk, ap_ptk.tk);
                assert_eq!(gtk, expected_gtk);
                assert_eq!(gtk_idx, expected_gtk_idx);

                // AP verifies M4 MIC
                let mut m4_zeroed = [0u8; 99];
                m4_zeroed.copy_from_slice(&reply[..99]);
                m4_zeroed[81..97].fill(0);
                assert_eq!(eapol_key_mic(&ap_ptk.kck, &m4_zeroed), reply[81..97]);
                assert_eq!(&reply[9..17], &2u64.to_be_bytes());
            }
            other => panic!("expected SendM4, got {other:?}"),
        }

        // Replaying the same M3 (replay <= last_replay) must be ignored
        assert_eq!(hs.process_eapol(&m3, &mut reply), HandshakeStep::Ignore);

        // Wrong-passphrase STA must reject M3 MIC with Failed
        let wrong_pmk = derive_pmk(b"wrong-passphrase", b"FerricAP");
        let mut wrong_hs = Wpa2Handshake::new(wrong_pmk, sta_mac, bssid, snonce);
        assert!(matches!(
            wrong_hs.process_eapol(&m1, &mut reply),
            HandshakeStep::SendM2(121)
        ));
        assert_eq!(
            wrong_hs.process_eapol(&m3, &mut reply),
            HandshakeStep::Failed
        );
    }

    #[test]
    fn group_rekey_rotates_gtk_and_replays_are_ignored() {
        use super::handshake::{HandshakeStep, Wpa2Handshake};

        let pmk = derive_pmk(b"correct-horse-battery", b"FerricAP");
        let sta_mac = [0x00, 0x13, 0x46, 0xFE, 0x32, 0x0C];
        let bssid = [0x00, 0x14, 0x6C, 0x7E, 0x40, 0x80];
        let snonce = [0x5Au8; 32];
        let anonce = [0xA5u8; 32];
        let ap_ptk = derive_ptk(&pmk, &bssid, &sta_mac, &anonce, &snonce);

        let build_m1 = |replay: u64| -> [u8; 99] {
            let mut m1 = [0u8; 99];
            m1[0] = 0x01;
            m1[1] = 0x03;
            m1[2..4].copy_from_slice(&95u16.to_be_bytes());
            m1[4] = 0x02;
            m1[5..7].copy_from_slice(&0x008Au16.to_be_bytes());
            m1[7..9].copy_from_slice(&16u16.to_be_bytes());
            m1[9..17].copy_from_slice(&replay.to_be_bytes());
            m1[17..49].copy_from_slice(&anonce);
            m1
        };
        let build_m3 = |replay: u64, gtk: [u8; 16], gtk_idx: u8| -> std::vec::Vec<u8> {
            let mut kd_plain = [0u8; 48];
            kd_plain[0..22].copy_from_slice(&WPA2_RSN_IE);
            kd_plain[22] = 0xDD;
            kd_plain[23] = 22;
            kd_plain[24..28].copy_from_slice(&[0x00, 0x0F, 0xAC, 0x01]);
            kd_plain[28] = gtk_idx;
            kd_plain[30..46].copy_from_slice(&gtk);
            kd_plain[46] = 0xDD;
            kd_plain[47] = 0x00;
            let kd_wrapped = aes_wrap_test(&ap_ptk.kek, &kd_plain);
            let mut m3 = std::vec![0u8; 99 + kd_wrapped.len()];
            m3[0] = 0x01;
            m3[1] = 0x03;
            m3[2..4].copy_from_slice(&((95 + kd_wrapped.len()) as u16).to_be_bytes());
            m3[4] = 0x02;
            m3[5..7].copy_from_slice(&0x13CAu16.to_be_bytes());
            m3[7..9].copy_from_slice(&16u16.to_be_bytes());
            m3[9..17].copy_from_slice(&replay.to_be_bytes());
            m3[17..49].copy_from_slice(&anonce);
            m3[97..99].copy_from_slice(&(kd_wrapped.len() as u16).to_be_bytes());
            m3[99..].copy_from_slice(&kd_wrapped);
            let mic = eapol_key_mic(&ap_ptk.kck, &m3);
            m3[81..97].copy_from_slice(&mic);
            m3
        };
        // Group-key message 1: Pairwise=0, KeyInfo 0x1382, key data is a
        // lone GTK KDE (24 bytes plain -> 32 wrapped).
        let build_group = |replay: u64, gtk: [u8; 16], gtk_idx: u8| -> std::vec::Vec<u8> {
            let mut kd_plain = [0u8; 24];
            kd_plain[0] = 0xDD;
            kd_plain[1] = 22;
            kd_plain[2..6].copy_from_slice(&[0x00, 0x0F, 0xAC, 0x01]);
            kd_plain[6] = gtk_idx;
            kd_plain[8..24].copy_from_slice(&gtk);
            let kd_wrapped = aes_wrap_test(&ap_ptk.kek, &kd_plain);
            let mut g = std::vec![0u8; 99 + kd_wrapped.len()];
            g[0] = 0x01;
            g[1] = 0x03;
            g[2..4].copy_from_slice(&((95 + kd_wrapped.len()) as u16).to_be_bytes());
            g[4] = 0x02;
            g[5..7].copy_from_slice(&0x1382u16.to_be_bytes());
            g[7..9].copy_from_slice(&16u16.to_be_bytes());
            g[9..17].copy_from_slice(&replay.to_be_bytes());
            g[97..99].copy_from_slice(&(kd_wrapped.len() as u16).to_be_bytes());
            g[99..].copy_from_slice(&kd_wrapped);
            let mic = eapol_key_mic(&ap_ptk.kck, &g);
            g[81..97].copy_from_slice(&mic);
            g
        };

        let gtk1: [u8; 16] = [0x11; 16];
        let gtk2: [u8; 16] = [0x22; 16];
        let mut reply = [0u8; 256];
        let mut hs = Wpa2Handshake::new(pmk, sta_mac, bssid, snonce);

        // A group frame before the pairwise handshake completes is ignored.
        assert_eq!(
            hs.process_eapol(&build_group(3, gtk2, 1), &mut reply),
            HandshakeStep::Ignore
        );

        // Complete the 4-way handshake (M1 replay 5, M3 replay 6).
        assert!(matches!(
            hs.process_eapol(&build_m1(5), &mut reply),
            HandshakeStep::SendM2(121)
        ));
        // An M1 older than the one already answered must not reset the floor.
        assert_eq!(
            hs.process_eapol(&build_m1(3), &mut reply),
            HandshakeStep::Ignore
        );
        assert!(matches!(
            hs.process_eapol(&build_m3(6, gtk1, 2), &mut reply),
            HandshakeStep::SendM4 { .. }
        ));

        // A replayed M1 after completion must not re-derive the PTK or reset
        // the replay floor (KRACK pattern).
        assert_eq!(
            hs.process_eapol(&build_m1(5), &mut reply),
            HandshakeStep::Ignore
        );
        assert_eq!(
            hs.process_eapol(&build_m1(9), &mut reply),
            HandshakeStep::Ignore
        );

        // Rekey with a higher replay counter yields the new GTK, plus the
        // group message 2 the AP expects in reply.
        match hs.process_eapol(&build_group(7, gtk2, 1), &mut reply) {
            HandshakeStep::RotateGtk {
                gtk,
                gtk_idx,
                reply_len,
            } => {
                assert_eq!(gtk, gtk2);
                assert_eq!(gtk_idx, 1);
                assert_eq!(reply_len, 99);
                // Message 2: MIC + Secure set, Pairwise and Ack clear, the
                // replay counter echoing message 1, no key data, and a MIC
                // the AP verifies with the session KCK.
                let info = u16::from_be_bytes([reply[5], reply[6]]);
                assert_eq!(info & 0x0100, 0x0100);
                assert_eq!(info & 0x0200, 0x0200);
                assert_eq!(info & 0x0008, 0);
                assert_eq!(info & 0x0080, 0);
                assert_eq!(&reply[9..17], &7u64.to_be_bytes());
                assert_eq!(&reply[97..99], &[0, 0]);
                let mut gm2_zeroed = [0u8; 99];
                gm2_zeroed.copy_from_slice(&reply[..99]);
                gm2_zeroed[81..97].fill(0);
                assert_eq!(eapol_key_mic(&ap_ptk.kck, &gm2_zeroed), reply[81..97]);
            }
            other => panic!("expected RotateGtk, got {other:?}"),
        }

        // Replaying the same group frame is ignored (replay not > floor).
        assert_eq!(
            hs.process_eapol(&build_group(7, gtk2, 1), &mut reply),
            HandshakeStep::Ignore
        );
        // A group frame with a lower replay than the accepted rekey too.
        assert_eq!(
            hs.process_eapol(&build_group(6, gtk1, 2), &mut reply),
            HandshakeStep::Ignore
        );
    }

    #[test]
    fn ccmp_encrypt_matches_ieee_80211i_vector() {
        // IEEE 802.11-2012 Annex M.1.2 (CCMP test vector, non-QoS 24B MAC hdr).
        let tk: [u8; 16] = [
            0xc9, 0x7c, 0x1f, 0x67, 0xce, 0x37, 0x11, 0x85, 0x51, 0x4a, 0x8a, 0x19, 0xf2, 0xbd,
            0xd5, 0x2f,
        ];
        let mut frame = [0u8; 64];
        let hdr: [u8; 24] = [
            0x08, 0x48, 0xc3, 0x2c, 0x0f, 0xd2, 0xe1, 0x28, 0xa5, 0x7c, 0x50, 0x30, 0xf1, 0x84,
            0x44, 0x08, 0xab, 0xae, 0xa5, 0xb8, 0xfc, 0xba, 0x80, 0x33,
        ];
        let ccmp_hdr: [u8; 8] = [0x0c, 0xe7, 0x00, 0x20, 0x76, 0x97, 0x03, 0xb5];
        let plain: [u8; 20] = [
            0xf8, 0xba, 0x1a, 0x55, 0xd0, 0x2f, 0x85, 0xae, 0x96, 0x7b, 0xb6, 0x2f, 0xb6, 0xcd,
            0xa8, 0xeb, 0x7e, 0x78, 0xa0, 0x50,
        ];
        frame[0..24].copy_from_slice(&hdr);
        frame[24..32].copy_from_slice(&ccmp_hdr);
        frame[32..52].copy_from_slice(&plain);

        let out_len = aes::ccmp_encrypt_frame(&tk, &mut frame, 52).unwrap();
        assert_eq!(out_len, 60);
        let expected_ct_mic: [u8; 28] = [
            0xf3, 0xd0, 0xa2, 0xfe, 0x9a, 0x3d, 0xbf, 0x23, 0x42, 0xa6, 0x43, 0xe4, 0x32, 0x46,
            0xe8, 0x0c, 0x3c, 0x04, 0xd0, 0x19, 0x78, 0x45, 0xce, 0x0b, 0x16, 0xf9, 0x76, 0x23,
        ];
        assert_eq!(&frame[32..60], &expected_ct_mic);
    }

    #[test]
    fn deauth_frame_has_expected_header_and_reason() {
        // Synthetic locally-administered fixture address, not a real card.
        let sa = [0x02, 0x11, 0x22, 0x33, 0x44, 0x55];
        let bssid = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF];
        let mut buf = [0u8; 32];
        assert_eq!(frame::build_deauth(&mut buf[..20], &sa, &bssid, 7, 3), 0);
        let len = frame::build_deauth(&mut buf, &sa, &bssid, 7, 3);
        assert_eq!(len, 26);
        let (hdr, off) = frame::parse_header(&buf[..len]).unwrap();
        assert_eq!(off, 24);
        assert_eq!(hdr.frame_type, frame::TYPE_MGMT);
        assert_eq!(hdr.subtype, frame::SUB_DEAUTH);
        assert_eq!(hdr.addr1, bssid);
        assert_eq!(hdr.addr2, sa);
        assert_eq!(hdr.addr3, bssid);
        assert_eq!(hdr.seq, 7 << 4);
        assert_eq!(u16::from_le_bytes([buf[24], buf[25]]), 3);
    }

    #[test]
    fn ccmp_decrypt_matches_ieee_80211i_vector_and_rejects_tamper() {
        let tk: [u8; 16] = [
            0xc9, 0x7c, 0x1f, 0x67, 0xce, 0x37, 0x11, 0x85, 0x51, 0x4a, 0x8a, 0x19, 0xf2, 0xbd,
            0xd5, 0x2f,
        ];
        let hdr: [u8; 24] = [
            0x08, 0x48, 0xc3, 0x2c, 0x0f, 0xd2, 0xe1, 0x28, 0xa5, 0x7c, 0x50, 0x30, 0xf1, 0x84,
            0x44, 0x08, 0xab, 0xae, 0xa5, 0xb8, 0xfc, 0xba, 0x80, 0x33,
        ];
        let ccmp_hdr: [u8; 8] = [0x0c, 0xe7, 0x00, 0x20, 0x76, 0x97, 0x03, 0xb5];
        let expected_plain: [u8; 20] = [
            0xf8, 0xba, 0x1a, 0x55, 0xd0, 0x2f, 0x85, 0xae, 0x96, 0x7b, 0xb6, 0x2f, 0xb6, 0xcd,
            0xa8, 0xeb, 0x7e, 0x78, 0xa0, 0x50,
        ];
        let ct_mic: [u8; 28] = [
            0xf3, 0xd0, 0xa2, 0xfe, 0x9a, 0x3d, 0xbf, 0x23, 0x42, 0xa6, 0x43, 0xe4, 0x32, 0x46,
            0xe8, 0x0c, 0x3c, 0x04, 0xd0, 0x19, 0x78, 0x45, 0xce, 0x0b, 0x16, 0xf9, 0x76, 0x23,
        ];

        let mut frame = [0u8; 64];
        frame[0..24].copy_from_slice(&hdr);
        frame[24..32].copy_from_slice(&ccmp_hdr);
        frame[32..60].copy_from_slice(&ct_mic);

        let dec_len = aes::ccmp_decrypt_frame(&tk, &mut frame, 60).unwrap();
        assert_eq!(dec_len, 52);
        assert_eq!(&frame[32..52], &expected_plain);

        // Tampering with ciphertext or MIC must fail verification.
        frame[0..24].copy_from_slice(&hdr);
        frame[24..32].copy_from_slice(&ccmp_hdr);
        frame[32..60].copy_from_slice(&ct_mic);
        frame[40] ^= 0x01;
        assert_eq!(aes::ccmp_decrypt_frame(&tk, &mut frame, 60), None);
    }

    #[test]
    fn decap_handles_ar2413_qos_hw_padding() {
        let bssid = [0x00, 0x14, 0x6C, 0x7E, 0x40, 0x80];
        let sa = [0x00, 0x13, 0x46, 0xFE, 0x32, 0x0C];
        let src = [0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0x01];

        // AR2413 inserts 2 alignment bytes right after the 26-byte QoS MAC
        // header (before the 8-byte CCMP header): 24 + 2 + 2 + 8 + 8 + 4 = 48.
        let mut rx = [0u8; 64];
        let fc: u16 = ((TYPE_DATA as u16) << 2) | (8u16 << 4) | 0x0200 | 0x4000;
        rx[0..2].copy_from_slice(&fc.to_le_bytes());
        rx[4..10].copy_from_slice(&sa);
        rx[10..16].copy_from_slice(&bssid);
        rx[16..22].copy_from_slice(&src);
        rx[24..26].copy_from_slice(&[0x00, 0x00]); // QoS Control
        rx[26..28].copy_from_slice(&[0x00, 0x00]); // AR2413 2-byte alignment pad
        rx[28..36].copy_from_slice(&[1, 0, 0, 0x20, 0, 0, 0, 0]); // CCMP header
        rx[36..44].copy_from_slice(&[0xAA, 0xAA, 0x03, 0x00, 0x00, 0x00, 0x08, 0x06]); // SNAP ARP
        rx[44..48].copy_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF]);

        let mut eth_out = [0u8; 64];
        let elen = decap_80211_to_eth(&mut eth_out, &rx[..48], &bssid).unwrap();
        assert_eq!(elen, 18);
        assert_eq!(&eth_out[0..6], &sa);
        assert_eq!(&eth_out[6..12], &src);
        assert_eq!(&eth_out[12..14], &[0x08, 0x06]);
        assert_eq!(&eth_out[14..18], &[0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn handshake_replay_zero_start_and_encrypted_group_m2_roundtrip() {
        use super::handshake::{HandshakeStep, Wpa2Handshake};

        let pmk = derive_pmk(b"correct-horse-battery", b"FerricAP");
        let sta_mac = [0x00, 0x13, 0x46, 0xFE, 0x32, 0x0C];
        let bssid = [0x00, 0x14, 0x6C, 0x7E, 0x40, 0x80];
        let snonce = [0x33u8; 32];
        let anonce = [0xCCu8; 32];
        let ap_ptk = derive_ptk(&pmk, &bssid, &sta_mac, &anonce, &snonce);

        // M1 starting at replay counter 0 must be accepted.
        let mut m1 = [0u8; 99];
        m1[0] = 0x02; // 802.1X-2004
        m1[1] = 0x03;
        m1[2..4].copy_from_slice(&95u16.to_be_bytes());
        m1[4] = 0x02;
        m1[5..7].copy_from_slice(&0x008Au16.to_be_bytes());
        m1[7..9].copy_from_slice(&16u16.to_be_bytes());
        m1[9..17].copy_from_slice(&0u64.to_be_bytes());
        m1[17..49].copy_from_slice(&anonce);

        let mut hs = Wpa2Handshake::new(pmk, sta_mac, bssid, snonce);
        let mut reply = [0u8; 256];
        assert!(matches!(
            hs.process_eapol(&m1, &mut reply),
            HandshakeStep::SendM2(121)
        ));

        // M3 at replay=1
        let mut kd_plain = [0u8; 24];
        kd_plain[0] = 0xDD;
        kd_plain[1] = 22;
        kd_plain[2..6].copy_from_slice(&[0x00, 0x0F, 0xAC, 0x01]);
        kd_plain[6] = 1;
        kd_plain[8..24].copy_from_slice(&[0x44; 16]);
        let kd_wrapped = aes_wrap_test(&ap_ptk.kek, &kd_plain);
        let mut m3 = std::vec![0u8; 99 + kd_wrapped.len()];
        m3[0] = 0x02;
        m3[1] = 0x03;
        m3[2..4].copy_from_slice(&((95 + kd_wrapped.len()) as u16).to_be_bytes());
        m3[4] = 0x02;
        m3[5..7].copy_from_slice(&0x13CAu16.to_be_bytes());
        m3[7..9].copy_from_slice(&16u16.to_be_bytes());
        m3[9..17].copy_from_slice(&1u64.to_be_bytes());
        m3[17..49].copy_from_slice(&anonce);
        m3[97..99].copy_from_slice(&(kd_wrapped.len() as u16).to_be_bytes());
        m3[99..].copy_from_slice(&kd_wrapped);
        let m3_mic = eapol_key_mic(&ap_ptk.kck, &m3);
        m3[81..97].copy_from_slice(&m3_mic);

        let tk = match hs.process_eapol(&m3, &mut reply) {
            HandshakeStep::SendM4 { tk, .. } => tk,
            other => panic!("expected SendM4, got {other:?}"),
        };

        // Group M1 at replay=2
        let mut gm1 = std::vec![0u8; 99 + kd_wrapped.len()];
        gm1[0] = 0x02;
        gm1[1] = 0x03;
        gm1[2..4].copy_from_slice(&((95 + kd_wrapped.len()) as u16).to_be_bytes());
        gm1[4] = 0x02;
        gm1[5..7].copy_from_slice(&0x1382u16.to_be_bytes());
        gm1[7..9].copy_from_slice(&16u16.to_be_bytes());
        gm1[9..17].copy_from_slice(&2u64.to_be_bytes());
        gm1[97..99].copy_from_slice(&(kd_wrapped.len() as u16).to_be_bytes());
        gm1[99..].copy_from_slice(&kd_wrapped);
        let gm1_mic = eapol_key_mic(&ap_ptk.kck, &gm1);
        gm1[81..97].copy_from_slice(&gm1_mic);

        let reply_len = match hs.process_eapol(&gm1, &mut reply) {
            HandshakeStep::RotateGtk { reply_len, .. } => reply_len,
            other => panic!("expected RotateGtk, got {other:?}"),
        };

        // Encapsulate Group M2 as CCMP-protected ToDS frame and verify AP-side decrypt.
        let mut eth = [0u8; 14 + 99];
        eth[0..6].copy_from_slice(&bssid);
        eth[6..12].copy_from_slice(&sta_mac);
        eth[12..14].copy_from_slice(&0x888Eu16.to_be_bytes());
        eth[14..14 + reply_len].copy_from_slice(&reply[..reply_len]);

        let mut dot11 = [0u8; 256];
        let plain_len = encap_eth_to_80211(&mut dot11, &eth, &bssid, &sta_mac, 55, Some((42, 0)));
        let enc_len = aes::ccmp_encrypt_frame(&tk, &mut dot11, plain_len).unwrap();
        assert_eq!(enc_len, plain_len + 8);
        assert_ne!(dot11[1] & 0x40, 0); // Protected bit set

        let dec_len = aes::ccmp_decrypt_frame(&ap_ptk.tk, &mut dot11, enc_len).unwrap();
        assert_eq!(dec_len, plain_len);
        let (et, payload) = snap_decap(&dot11[32..dec_len]).unwrap();
        assert_eq!(et, ETHERTYPE_EAPOL);
        assert_eq!(payload, &reply[..reply_len]);
    }
}
