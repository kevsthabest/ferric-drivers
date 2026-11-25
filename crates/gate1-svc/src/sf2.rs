//! SoundFont 2.0 (`.SF2`) RIFF/sfbk parser and runtime wavetable bank for the
//! ALi M5451 31-voice hardware wavetable synthesizer.
//!
//! Loaded into locked XMS 3.0 extended physical RAM (up to 256 MiB) by
//! `SF2LOAD.COM` (streaming via XMS `AH=0Bh` Move EMB, zero Ring-0 mapping of
//! the giant `smpl` audio pool). Only the small RIFF/`INFO`/`pdta` metadata
//! headers are temporarily mapped via `cardbus::map_mmio` during commit; at
//! playback time the ALi M5451's 30-bit (1 GiB) PCI bus-master DMA reads the
//! 16-bit signed mono PCM samples directly from physical XMS RAM.
//!
//! Strictly 386-pure (`#![no_std]`, zero heap allocations), read-only w.r.t. storage.

use core::cell::UnsafeCell;

use crate::cardbus;

/// Maximum SoundFont file size supported (256 MiB via XMS 3.0).
pub const SF2_MAX_BYTES: u32 = 256 * 1024 * 1024;
/// Maximum `pdta` metadata chunk size we will map into Ring-0 (4 MiB).
const MAX_PDTA_MAP_BYTES: u32 = 4 * 1024 * 1024;
/// Maximum melodic instrument zones across all 128 GM programs.
const MAX_ZONES: usize = 2048;
/// Drum key range mapped for MIDI channel 10 (Bank 128 Preset 0): notes 24..=119.
const DRUM_KEY_BASE: u8 = 24;
const DRUM_KEY_COUNT: usize = 96;

/// One parsed SoundFont sample zone ready for direct ALi M5451 voice playback.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Sf2Zone {
    pub start: u32,
    pub end: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub sample_rate: u32,
    pub key_lo: u8,
    pub key_hi: u8,
    pub root_key: u8,
    pub loop_mode: u8, // 0 = one-shot, 1/3 = loop
    pub tune_cents: i16,
    pub atten_cb: u16, // centibels (0.1 dB)
}

impl Sf2Zone {
    const EMPTY: Self = Self {
        start: 0,
        end: 0,
        loop_start: 0,
        loop_end: 0,
        sample_rate: 44100,
        key_lo: 0,
        key_hi: 127,
        root_key: 60,
        loop_mode: 0,
        tune_cents: 0,
        atten_cb: 0,
    };
}

/// Resolved hardware playback parameters for a single ALi M5451 voice.
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub struct Sf2VoiceParams {
    pub lba_phys: u32,
    pub cso: u16,
    pub eso: u16,
    pub delta: u16,
    pub looping: bool,
    pub extra_atten: u8, // additional 0.375 dB steps from SF2 initialAttenuation
}

struct Sf2State {
    active: bool,
    xms_handle: u16,
    phys_base: u32,
    file_size: u32,
    smpl_offset: u32,
    smpl_samples: u32,
    bank_name: [u8; 32],
    bank_name_len: u8,
    programs_loaded: u16,
    drums_loaded: u16,
    zone_count: u16,
    melodic_head: [u16; 128],
    melodic_len: [u16; 128],
    zones: [Sf2Zone; MAX_ZONES],
    drum_valid: [bool; DRUM_KEY_COUNT],
    drums: [Sf2Zone; DRUM_KEY_COUNT],
}

impl Sf2State {
    const fn new() -> Self {
        Self {
            active: false,
            xms_handle: 0,
            phys_base: 0,
            file_size: 0,
            smpl_offset: 0,
            smpl_samples: 0,
            bank_name: [0; 32],
            bank_name_len: 0,
            programs_loaded: 0,
            drums_loaded: 0,
            zone_count: 0,
            melodic_head: [0; 128],
            melodic_len: [0; 128],
            zones: [Sf2Zone::EMPTY; MAX_ZONES],
            drum_valid: [false; DRUM_KEY_COUNT],
            drums: [Sf2Zone::EMPTY; DRUM_KEY_COUNT],
        }
    }

    fn clear_parsed(&mut self) {
        self.active = false;
        self.smpl_offset = 0;
        self.smpl_samples = 0;
        self.bank_name = [0; 32];
        self.bank_name_len = 0;
        self.programs_loaded = 0;
        self.drums_loaded = 0;
        self.zone_count = 0;
        for i in 0..128 {
            self.melodic_head[i] = 0;
            self.melodic_len[i] = 0;
        }
        for i in 0..DRUM_KEY_COUNT {
            self.drum_valid[i] = false;
        }
    }
}

struct SyncSf2(UnsafeCell<Sf2State>);
// SAFETY: single-core DOS with CLI/STI discipline.
unsafe impl Sync for SyncSf2 {}

static SF2: SyncSf2 = SyncSf2(UnsafeCell::new(Sf2State::new()));

fn sf2_state() -> &'static mut Sf2State {
    unsafe { &mut *SF2.0.get() }
}

// -----------------------------------------------------------------------------
// Fixed-point pitch computation (386-pure, zero float)
// -----------------------------------------------------------------------------

/// `2^(k/12) * 65536` for `k = 0..11`.
const SEMITONE_FP16: [u32; 12] = [
    65536, 69433, 73562, 77936, 82570, 87480, 92682, 98193, 104032, 110218, 116772, 123715,
];

/// Compute ALi M5451 4.12 `DELTA` register for a sample recorded at `sample_rate` Hz
/// with root MIDI key `root_key`, played at MIDI `note` with pitch bend `bend` (-8192..8191,
/// ±2 semitones) and fine tuning `tune_cents` (in cents, 100 cents = 1 semitone).
pub fn compute_sf2_delta(
    sample_rate: u32,
    root_key: u8,
    note: u8,
    bend: i16,
    tune_cents: i16,
) -> u16 {
    let sr = sample_rate.clamp(4000, 96000) as u64;
    let bend_cents = ((bend as i32) * 200) / 8192;
    let total_cents = ((note as i32) - (root_key as i32)) * 100 + (tune_cents as i32) + bend_cents;
    let clamped = total_cents.clamp(-4800, 4800);

    let mut semis = clamped / 100;
    let mut rem_cents = clamped % 100;
    if rem_cents < 0 {
        rem_cents += 100;
        semis -= 1;
    }

    let mut octs = semis / 12;
    let mut st = semis % 12;
    if st < 0 {
        st += 12;
        octs -= 1;
    }

    let mut d: u64 = ((sr << 12) + 24000) / 48000;
    d = (d * (SEMITONE_FP16[st as usize] as u64) + 32768) >> 16;
    let cent_fp16 = 65536u64 + (rem_cents as u64) * 38;
    d = (d * cent_fp16 + 32768) >> 16;

    if octs > 0 {
        let sh = (octs as u32).min(8);
        d <<= sh;
    } else if octs < 0 {
        let sh = ((-octs) as u32).min(12);
        d >>= sh;
    }

    d.clamp(1, 0xFFFE) as u16
}

/// Convert an `Sf2Zone` + MIDI note/bend into ALi M5451 hardware voice parameters.
pub fn resolve_zone_voice(
    smpl_phys_base: u32,
    zone: &Sf2Zone,
    note: u8,
    bend: i16,
    is_drum: bool,
) -> Option<Sf2VoiceParams> {
    if zone.end <= zone.start + 4 {
        return None;
    }
    let looping = !is_drum
        && (zone.loop_mode == 1 || zone.loop_mode == 3)
        && zone.loop_end > zone.loop_start + 8
        && zone.loop_end <= zone.end;

    let (base_sample, len_samples) = if looping {
        let loop_len = zone.loop_end - zone.loop_start;
        (zone.loop_start, loop_len.min(65535))
    } else {
        let total = zone.end - zone.start;
        (zone.start, total.min(65535))
    };
    if len_samples < 4 {
        return None;
    }

    let lba_phys = smpl_phys_base.wrapping_add(base_sample.wrapping_mul(2));
    if lba_phys >= 0x4000_0000 {
        return None;
    }

    let delta = compute_sf2_delta(zone.sample_rate, zone.root_key, note, bend, zone.tune_cents);
    let extra_atten = ((zone.atten_cb as u32) * 4 / 15).min(120) as u8;

    Some(Sf2VoiceParams {
        lba_phys,
        cso: 0,
        eso: (len_samples - 1) as u16,
        delta,
        looping,
        extra_atten,
    })
}

/// Fast runtime lookup called from `MpuMidi::note_on` in `sb_opl.rs`.
pub fn lookup_note(ch: u8, prog: u8, note: u8, bend: i16) -> Option<Sf2VoiceParams> {
    let st = unsafe { &*SF2.0.get() };
    if !st.active || st.phys_base == 0 || st.smpl_samples == 0 {
        return None;
    }
    let smpl_phys = st.phys_base.wrapping_add(st.smpl_offset);
    if ch == 9 {
        if note >= DRUM_KEY_BASE && (note - DRUM_KEY_BASE) < DRUM_KEY_COUNT as u8 {
            let idx = (note - DRUM_KEY_BASE) as usize;
            if st.drum_valid[idx] {
                return resolve_zone_voice(smpl_phys, &st.drums[idx], note, 0, true);
            }
        }
        return None;
    }

    let p = (prog & 0x7F) as usize;
    let head = st.melodic_head[p] as usize;
    let len = st.melodic_len[p] as usize;
    if len == 0 {
        return None;
    }
    let mut best_idx = head;
    let mut best_dist = 255u8;
    for i in head..(head + len).min(MAX_ZONES) {
        let z = &st.zones[i];
        if note >= z.key_lo && note <= z.key_hi {
            return resolve_zone_voice(smpl_phys, z, note, bend, false);
        }
        let dist = if note < z.key_lo {
            z.key_lo - note
        } else {
            note - z.key_hi
        };
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
    }
    resolve_zone_voice(smpl_phys, &st.zones[best_idx], note, bend, false)
}

// -----------------------------------------------------------------------------
// Pure SF2 pdta Hydra Parser (operates on ONLY the pdta chunk slice!)
// -----------------------------------------------------------------------------

#[inline]
fn rd_u16(b: &[u8], off: usize) -> u16 {
    u16::from_le_bytes([b[off], b[off + 1]])
}

#[inline]
fn rd_u32(b: &[u8], off: usize) -> u32 {
    u32::from_le_bytes([b[off], b[off + 1], b[off + 2], b[off + 3]])
}

#[derive(Copy, Clone)]
struct GenAccum {
    key_lo: u8,
    key_hi: u8,
    vel_lo: u8,
    vel_hi: u8,
    coarse_tune: i16,
    fine_tune: i16,
    atten_cb: u16,
    root_key: i16,
    loop_mode: u8,
    start_off: i32,
    end_off: i32,
    loop_start_off: i32,
    loop_end_off: i32,
    inst_id: i32,
    sample_id: i32,
}

impl GenAccum {
    const fn default_global() -> Self {
        Self {
            key_lo: 0,
            key_hi: 127,
            vel_lo: 0,
            vel_hi: 127,
            coarse_tune: 0,
            fine_tune: 0,
            atten_cb: 0,
            root_key: -1,
            loop_mode: 0,
            start_off: 0,
            end_off: 0,
            loop_start_off: 0,
            loop_end_off: 0,
            inst_id: -1,
            sample_id: -1,
        }
    }

    fn apply(&mut self, oper: u16, raw: u16) {
        match oper {
            0 => self.start_off += raw as i16 as i32,
            1 => self.end_off += raw as i16 as i32,
            2 => self.loop_start_off += raw as i16 as i32,
            3 => self.loop_end_off += raw as i16 as i32,
            4 => self.start_off += (raw as i16 as i32) * 32768,
            12 => self.end_off += (raw as i16 as i32) * 32768,
            41 => self.inst_id = raw as i32,
            43 => {
                self.key_lo = (raw & 0xFF) as u8;
                self.key_hi = (raw >> 8) as u8;
            }
            44 => {
                self.vel_lo = (raw & 0xFF) as u8;
                self.vel_hi = (raw >> 8) as u8;
            }
            45 => self.loop_start_off += (raw as i16 as i32) * 32768,
            48 => self.atten_cb = self.atten_cb.saturating_add(raw),
            50 => self.loop_end_off += (raw as i16 as i32) * 32768,
            51 => self.coarse_tune = self.coarse_tune.saturating_add(raw as i16),
            52 => self.fine_tune = self.fine_tune.saturating_add(raw as i16),
            53 => self.sample_id = raw as i32,
            54 => self.loop_mode = (raw & 3) as u8,
            58 => {
                if raw <= 127 {
                    self.root_key = raw as i16;
                }
            }
            _ => {}
        }
    }
}

fn parse_info_slice(info_body: &[u8], st: &mut Sf2State) {
    let mut sub = 0usize;
    while sub + 8 <= info_body.len() {
        let sid = &info_body[sub..sub + 4];
        let ssz = rd_u32(info_body, sub + 4) as usize;
        let sb = sub + 8;
        let se = sb.saturating_add(ssz).min(info_body.len());
        if sid == b"INAM" {
            let mut n = 0usize;
            while n < 31 && sb + n < se && info_body[sb + n] != 0 {
                let c = info_body[sb + n];
                st.bank_name[n] = if c >= 0x20 && c <= 0x7E { c } else { b' ' };
                n += 1;
            }
            st.bank_name_len = n as u8;
            return;
        }
        sub = sb.saturating_add(ssz + (ssz & 1));
    }
}

/// Parse the `pdta` LIST body (starting right after `b"pdta"`) into `st`.
/// `st.smpl_offset` and `st.smpl_samples` must already be populated.
fn parse_pdta_slice(pdta_body: &[u8], st: &mut Sf2State) -> Result<(), &'static str> {
    let mut phdr: &[u8] = &[];
    let mut pbag: &[u8] = &[];
    let mut pgen: &[u8] = &[];
    let mut inst: &[u8] = &[];
    let mut ibag: &[u8] = &[];
    let mut igen: &[u8] = &[];
    let mut shdr: &[u8] = &[];

    let mut sub = 0usize;
    while sub + 8 <= pdta_body.len() {
        let sid = &pdta_body[sub..sub + 4];
        let ssz = rd_u32(pdta_body, sub + 4) as usize;
        let sb = sub + 8;
        let se = sb.saturating_add(ssz).min(pdta_body.len());
        let slice = &pdta_body[sb..se];
        match sid {
            b"phdr" => phdr = slice,
            b"pbag" => pbag = slice,
            b"pgen" => pgen = slice,
            b"inst" => inst = slice,
            b"ibag" => ibag = slice,
            b"igen" => igen = slice,
            b"shdr" => shdr = slice,
            _ => {}
        }
        sub = sb.saturating_add(ssz + (ssz & 1));
    }

    if phdr.len() < 76
        || pbag.len() < 8
        || pgen.len() < 4
        || inst.len() < 44
        || ibag.len() < 8
        || igen.len() < 4
        || shdr.len() < 92
    {
        return Err("Missing required pdta Hydra subchunks");
    }

    let n_phdr = phdr.len() / 38;
    let n_pbag = pbag.len() / 4;
    let n_pgen = pgen.len() / 4;
    let n_inst = inst.len() / 22;
    let n_ibag = ibag.len() / 4;
    let n_igen = igen.len() / 4;
    let n_shdr = shdr.len() / 46;

    for pi in 0..n_phdr.saturating_sub(1) {
        let poff = pi * 38;
        let preset = rd_u16(phdr, poff + 20);
        let bank = rd_u16(phdr, poff + 22);
        let is_melodic = bank == 0 && preset < 128;
        let is_drums = bank == 128 && preset == 0;
        if !is_melodic && !is_drums {
            continue;
        }
        if is_melodic && st.melodic_len[preset as usize] > 0 {
            continue;
        }

        let pb_start = rd_u16(phdr, poff + 24) as usize;
        let pb_end = rd_u16(phdr, poff + 38 + 24) as usize;
        if pb_start >= pb_end || pb_end > n_pbag {
            continue;
        }

        let head_before = st.zone_count;
        let mut p_global = GenAccum::default_global();
        let mut first_pbag = true;

        for pbi in pb_start..pb_end {
            let pg_start = rd_u16(pbag, pbi * 4) as usize;
            let pg_end = if pbi + 1 < n_pbag {
                rd_u16(pbag, (pbi + 1) * 4) as usize
            } else {
                n_pgen
            };
            if pg_start > pg_end || pg_end > n_pgen {
                continue;
            }
            let mut pg = p_global;
            pg.inst_id = -1;
            for pgi in pg_start..pg_end {
                let oper = rd_u16(pgen, pgi * 4);
                let amt = rd_u16(pgen, pgi * 4 + 2);
                pg.apply(oper, amt);
            }
            if first_pbag && pg.inst_id < 0 {
                p_global = pg;
                first_pbag = false;
                continue;
            }
            first_pbag = false;
            if pg.inst_id < 0 || (pg.inst_id as usize) + 1 >= n_inst {
                continue;
            }

            let ii = pg.inst_id as usize;
            let ib_start = rd_u16(inst, ii * 22 + 20) as usize;
            let ib_end = rd_u16(inst, (ii + 1) * 22 + 20) as usize;
            if ib_start >= ib_end || ib_end > n_ibag {
                continue;
            }

            let mut i_global = GenAccum::default_global();
            let mut first_ibag = true;

            for ibi in ib_start..ib_end {
                let ig_start = rd_u16(ibag, ibi * 4) as usize;
                let ig_end = if ibi + 1 < n_ibag {
                    rd_u16(ibag, (ibi + 1) * 4) as usize
                } else {
                    n_igen
                };
                if ig_start > ig_end || ig_end > n_igen {
                    continue;
                }
                let mut ig = i_global;
                ig.sample_id = -1;
                for igi in ig_start..ig_end {
                    let oper = rd_u16(igen, igi * 4);
                    let amt = rd_u16(igen, igi * 4 + 2);
                    ig.apply(oper, amt);
                }
                if first_ibag && ig.sample_id < 0 {
                    i_global = ig;
                    first_ibag = false;
                    continue;
                }
                first_ibag = false;
                if ig.sample_id < 0 || (ig.sample_id as usize) + 1 >= n_shdr {
                    continue;
                }

                // Velocity-layer filter for huge multi-layer banks (FluidR3, Arachno):
                // intersect preset & instrument velocity ranges and prefer layers that
                // cover typical game velocity (~100) so we don't fill MAX_ZONES with
                // soft-only pianissimo layers.
                let vlo = ig.vel_lo.max(pg.vel_lo);
                let vhi = ig.vel_hi.min(pg.vel_hi);
                if vlo > vhi || vhi < 80 {
                    continue;
                }

                let si = ig.sample_id as usize;
                let soff = si * 46;
                let sample_type = rd_u16(shdr, soff + 44);
                if (sample_type & 0x0F) == 2 {
                    continue;
                }

                let klo = ig.key_lo.max(pg.key_lo);
                let khi = ig.key_hi.min(pg.key_hi);
                if klo > khi {
                    continue;
                }

                let s_start = (rd_u32(shdr, soff + 20) as i32 + ig.start_off).max(0) as u32;
                let s_end = (rd_u32(shdr, soff + 24) as i32 + ig.end_off).max(0) as u32;
                let l_start = (rd_u32(shdr, soff + 28) as i32 + ig.loop_start_off).max(0) as u32;
                let l_end = (rd_u32(shdr, soff + 32) as i32 + ig.loop_end_off).max(0) as u32;
                let s_rate = rd_u32(shdr, soff + 36);
                let orig_pitch = shdr[soff + 40];
                let pitch_corr = shdr[soff + 41] as i8;

                if s_end <= s_start + 4 || s_end > st.smpl_samples {
                    continue;
                }

                let root_key = if ig.root_key >= 0 && ig.root_key <= 127 {
                    ig.root_key as u8
                } else if orig_pitch <= 127 {
                    orig_pitch
                } else {
                    60
                };
                let tune_cents = (pitch_corr as i16)
                    .saturating_add(ig.coarse_tune.saturating_mul(100))
                    .saturating_add(ig.fine_tune)
                    .saturating_add(pg.coarse_tune.saturating_mul(100))
                    .saturating_add(pg.fine_tune);
                let atten_cb = ig.atten_cb.saturating_add(pg.atten_cb);

                let zone = Sf2Zone {
                    start: s_start,
                    end: s_end,
                    loop_start: l_start,
                    loop_end: l_end,
                    sample_rate: if s_rate >= 4000 { s_rate } else { 44100 },
                    key_lo: klo,
                    key_hi: khi,
                    root_key,
                    loop_mode: ig.loop_mode,
                    tune_cents,
                    atten_cb,
                };

                if is_drums {
                    for k in klo..=khi {
                        if k >= DRUM_KEY_BASE && (k - DRUM_KEY_BASE) < DRUM_KEY_COUNT as u8 {
                            let idx = (k - DRUM_KEY_BASE) as usize;
                            if !st.drum_valid[idx] {
                                st.drums[idx] = zone;
                                st.drum_valid[idx] = true;
                                st.drums_loaded = st.drums_loaded.saturating_add(1);
                            }
                        }
                    }
                } else {
                    // Deduplicate identical (key_lo, key_hi) layers within the same program
                    let mut dup = false;
                    for zi in (head_before as usize)..(st.zone_count as usize) {
                        if st.zones[zi].key_lo == klo && st.zones[zi].key_hi == khi {
                            dup = true;
                            break;
                        }
                    }
                    if !dup && (st.zone_count as usize) < MAX_ZONES {
                        st.zones[st.zone_count as usize] = zone;
                        st.zone_count += 1;
                    }
                }
            }
        }

        if is_melodic && st.zone_count > head_before {
            st.melodic_head[preset as usize] = head_before;
            st.melodic_len[preset as usize] = st.zone_count - head_before;
            st.programs_loaded = st.programs_loaded.saturating_add(1);
        }
    }

    if st.programs_loaded == 0 && st.drums_loaded == 0 {
        return Err("No GM Bank 0 / Drum Bank 128 presets found in SF2");
    }

    st.active = true;
    Ok(())
}

/// Read up to 16 bytes from physical XMS memory at `phys` using a 1-2 page temporary map.
unsafe fn read_phys_header(phys: u32, dst: &mut [u8; 16]) -> bool {
    let page_base = phys & !0xFFF;
    let page_off = (phys & 0xFFF) as usize;
    let pages = if page_off + 16 > 4096 { 2 } else { 1 };
    let lin = unsafe { cardbus::map_mmio(page_base, pages) };
    if lin == 0 {
        return false;
    }
    let src = (lin as usize + page_off) as *const u8;
    for i in 0..16 {
        dst[i] = unsafe { core::ptr::read_volatile(src.add(i)) };
    }
    true
}

/// Walk the RIFF/`sfbk` top-level chunks directly in physical XMS RAM without
/// mapping the huge `smpl` audio data, then map ONLY `INFO` and `pdta` to parse!
unsafe fn parse_sf2_from_phys(st: &mut Sf2State) -> Result<(), &'static str> {
    let phys_base = st.phys_base;
    let file_size = st.file_size;
    st.clear_parsed();
    if file_size < 64 {
        return Err("File too small");
    }

    let mut hdr = [0u8; 16];
    if !unsafe { read_phys_header(phys_base, &mut hdr) } {
        return Err("Failed to map SF2 RIFF header");
    }
    if &hdr[0..4] != b"RIFF" || &hdr[8..12] != b"sfbk" {
        return Err("Not a RIFF/sfbk SoundFont file");
    }

    let mut info_off: u32 = 0;
    let mut info_len: u32 = 0;
    let mut smpl_off: u32 = 0;
    let mut smpl_len: u32 = 0;
    let mut pdta_off: u32 = 0;
    let mut pdta_len: u32 = 0;

    // Walk top-level LIST chunks (only 3-5 chunks in any SF2 file!)
    let mut pos: u32 = 12;
    let mut hops: u32 = 0;
    while pos + 12 <= file_size && hops < 32 {
        hops += 1;
        if !unsafe { read_phys_header(phys_base.wrapping_add(pos), &mut hdr) } {
            return Err("Failed to map LIST header");
        }
        let csz = rd_u32(&hdr, 4);
        let body_start = pos.saturating_add(8);
        let body_end = body_start.saturating_add(csz).min(file_size);
        if &hdr[0..4] == b"LIST" && body_start + 4 <= body_end {
            let ltype = &hdr[8..12];
            if ltype == b"INFO" {
                info_off = body_start + 4;
                info_len = body_end - info_off;
            } else if ltype == b"sdta" {
                // Walk subchunks inside sdta (usually just `smpl`, optionally `sm24`)
                let mut sub = body_start + 4;
                let mut shops = 0u32;
                while sub + 8 <= body_end && shops < 8 {
                    shops += 1;
                    let mut sh = [0u8; 16];
                    if !unsafe { read_phys_header(phys_base.wrapping_add(sub), &mut sh) } {
                        break;
                    }
                    let ssz = rd_u32(&sh, 4);
                    let sb = sub.saturating_add(8);
                    let se = sb.saturating_add(ssz).min(body_end);
                    if &sh[0..4] == b"smpl" {
                        smpl_off = sb;
                        smpl_len = se - sb;
                        break;
                    }
                    sub = sb.saturating_add(ssz.saturating_add(ssz & 1));
                }
            } else if ltype == b"pdta" {
                pdta_off = body_start + 4;
                pdta_len = body_end - pdta_off;
            }
        }
        pos = body_start.saturating_add(csz.saturating_add(csz & 1));
    }

    if smpl_len < 64 {
        return Err("Missing or empty sdta/smpl chunk");
    }
    if pdta_len < 256 || pdta_len > MAX_PDTA_MAP_BYTES {
        return Err("Missing or oversized pdta metadata chunk");
    }

    st.smpl_offset = smpl_off;
    st.smpl_samples = smpl_len / 2;

    // Map INFO chunk (capped to 16 KiB) to extract bank name
    if info_len >= 8 {
        let cap = info_len.min(16384);
        let p = phys_base.wrapping_add(info_off);
        let p_aligned = p & !0xFFF;
        let p_off = (p & 0xFFF) as usize;
        let pages = ((p_off as u32) + cap + 4095) / 4096;
        let lin = unsafe { cardbus::map_mmio(p_aligned, pages) };
        if lin != 0 {
            let slice = unsafe {
                core::slice::from_raw_parts((lin as usize + p_off) as *const u8, cap as usize)
            };
            parse_info_slice(slice, st);
        }
    }

    // Map ONLY the pdta metadata chunk into Ring-0 and parse Hydra tables
    let p = phys_base.wrapping_add(pdta_off);
    let p_aligned = p & !0xFFF;
    let p_off = (p & 0xFFF) as usize;
    let pages = ((p_off as u32) + pdta_len + 4095) / 4096;
    let lin = unsafe { cardbus::map_mmio(p_aligned, pages) };
    if lin == 0 {
        return Err("Failed to map pdta metadata chunk");
    }
    let pdta_slice = unsafe {
        core::slice::from_raw_parts((lin as usize + p_off) as *const u8, pdta_len as usize)
    };
    parse_pdta_slice(pdta_slice, st)
}

// -----------------------------------------------------------------------------
// Bounded ASCII report writer
// -----------------------------------------------------------------------------

struct Out<'a> {
    buf: &'a mut [u8],
    pos: usize,
}

impl<'a> Out<'a> {
    fn new(buf: &'a mut [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn byte(&mut self, b: u8) {
        if self.pos < self.buf.len() {
            self.buf[self.pos] = b;
            self.pos += 1;
        }
    }
    fn str(&mut self, s: &str) {
        for &b in s.as_bytes() {
            self.byte(b);
        }
    }
    fn crlf(&mut self) {
        self.byte(b'\r');
        self.byte(b'\n');
    }
    fn hex32(&mut self, v: u32) {
        const H: &[u8; 16] = b"0123456789ABCDEF";
        for i in (0..8).rev() {
            self.byte(H[((v >> (i * 4)) & 0xF) as usize]);
        }
    }
    fn dec_u32(&mut self, mut v: u32) {
        let mut tmp = [0u8; 10];
        let mut n = 0usize;
        if v == 0 {
            self.byte(b'0');
            return;
        }
        while v > 0 {
            tmp[n] = b'0' + (v % 10) as u8;
            v /= 10;
            n += 1;
        }
        while n > 0 {
            n -= 1;
            self.byte(tmp[n]);
        }
    }
}

fn format_status(st: &Sf2State, out_ptr: u32, out_cap: u32) -> u32 {
    if out_ptr == 0 || out_cap == 0 {
        return 0;
    }
    let slice = unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_cap as usize) };
    let mut o = Out::new(slice);
    if !st.active {
        o.str("SF2: none loaded (using built-in 4 KiB wavetable ROM)");
        o.crlf();
        return o.pos as u32;
    }
    o.str("SF2: \"");
    let n = (st.bank_name_len as usize).min(31);
    if n == 0 {
        o.str("Unnamed Bank");
    } else {
        for i in 0..n {
            o.byte(st.bank_name[i]);
        }
    }
    o.str("\"  phys=0x");
    o.hex32(st.phys_base);
    o.str("  size=");
    o.dec_u32(st.file_size / 1024);
    o.str(" KiB");
    o.crlf();
    o.str("  GM programs=");
    o.dec_u32(st.programs_loaded as u32);
    o.str("/128  zones=");
    o.dec_u32(st.zone_count as u32);
    o.str("  drums=");
    o.dec_u32(st.drums_loaded as u32);
    o.str("  samples=");
    o.dec_u32(st.smpl_samples);
    o.crlf();
    o.pos as u32
}

// -----------------------------------------------------------------------------
// V86 API entry points (AX=0x0040..0x0044)
// -----------------------------------------------------------------------------

/// Verb `0x0040`: Register a locked XMS 3.0 SoundFont buffer (`phys_base`, `file_size`,
/// `xms_handle`) without mapping the entire file into Ring-0.
/// Returns `0` in low 16 bits on success (and previous `xms_handle` in upper 16 bits).
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sf2_begin(phys_base: u32, file_size: u32, xms_handle: u32) -> u32 {
    if phys_base == 0 || file_size < 64 || file_size > SF2_MAX_BYTES {
        return 1;
    }
    if phys_base.saturating_add(file_size) >= 0x4000_0000 {
        return 2;
    }
    let st = sf2_state();
    crate::ali5451::hw_midi_stop_all();
    let prev_handle = st.xms_handle as u32;
    st.clear_parsed();
    st.phys_base = phys_base;
    st.file_size = file_size;
    st.xms_handle = (xms_handle & 0xFFFF) as u16;
    prev_handle << 16
}

/// Verb `0x0041`: Legacy chunk copy (kept for ABI compatibility; `SF2LOAD` now
/// streams directly into XMS via XMS `AH=0Bh` Move EMB).
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sf2_write_chunk(_offset: u32, _src_lin: u32, _len: u32) -> u32 {
    0
}

/// Verb `0x0042`: Parse the `.SF2` bank in physical XMS (mapping only RIFF/`INFO`/`pdta`)
/// and activate it for ALi M5451 hardware wavetable MIDI playback.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sf2_commit(out_ptr: u32, out_cap: u32) -> u32 {
    let st = sf2_state();
    if st.phys_base == 0 || st.file_size < 64 {
        return 0x0001_0000;
    }
    match unsafe { parse_sf2_from_phys(st) } {
        Ok(()) => format_status(st, out_ptr, out_cap),
        Err(msg) => {
            if out_ptr != 0 && out_cap != 0 {
                let slice =
                    unsafe { core::slice::from_raw_parts_mut(out_ptr as *mut u8, out_cap as usize) };
                let mut o = Out::new(slice);
                o.str("SF2 ERROR: ");
                o.str(msg);
                o.crlf();
                return 0x0001_0000 | (o.pos as u32);
            }
            0x0001_0000
        }
    }
}

/// Verb `0x0043`: Unload active SoundFont (reverts to built-in 4 KiB ROM).
/// Returns the previously held XMS handle (or `0` if none).
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sf2_unload() -> u32 {
    let st = sf2_state();
    crate::ali5451::hw_midi_stop_all();
    let h = st.xms_handle as u32;
    st.clear_parsed();
    st.xms_handle = 0;
    st.phys_base = 0;
    st.file_size = 0;
    h
}

/// Verb `0x0044`: Print current SoundFont status to `out_ptr[..out_cap]`.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sf2_status(out_ptr: u32, out_cap: u32) -> u32 {
    let st = unsafe { &*SF2.0.get() };
    format_status(st, out_ptr, out_cap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_delta_math_48k_and_octaves() {
        assert_eq!(compute_sf2_delta(48000, 60, 60, 0, 0), 0x1000);
        assert_eq!(compute_sf2_delta(48000, 60, 72, 0, 0), 0x2000);
        assert_eq!(compute_sf2_delta(48000, 60, 48, 0, 0), 0x0800);
        let d22 = compute_sf2_delta(22050, 60, 60, 0, 0);
        assert!(d22 >= 0x0750 && d22 <= 0x0760);
    }

    #[test]
    fn parse_minimal_valid_sf2_pdta_and_velocity_dedup() {
        fn push_chunk(dst: &mut std::vec::Vec<u8>, id: &[u8; 4], body: &[u8]) {
            dst.extend_from_slice(id);
            dst.extend_from_slice(&(body.len() as u32).to_le_bytes());
            dst.extend_from_slice(body);
            if (body.len() & 1) != 0 {
                dst.push(0);
            }
        }

        let mut info = std::vec::Vec::new();
        push_chunk(&mut info, b"INAM", b"TestSC55\0");

        let mut phdr = std::vec::Vec::new();
        let mut rec = [0u8; 38];
        rec[20..22].copy_from_slice(&0u16.to_le_bytes());
        rec[22..24].copy_from_slice(&0u16.to_le_bytes());
        rec[24..26].copy_from_slice(&0u16.to_le_bytes());
        phdr.extend_from_slice(&rec);
        rec[22..24].copy_from_slice(&128u16.to_le_bytes());
        rec[24..26].copy_from_slice(&1u16.to_le_bytes());
        phdr.extend_from_slice(&rec);
        rec[24..26].copy_from_slice(&2u16.to_le_bytes());
        phdr.extend_from_slice(&rec);

        let mut pbag = std::vec::Vec::new();
        for i in 0..=2u16 {
            pbag.extend_from_slice(&i.to_le_bytes());
            pbag.extend_from_slice(&0u16.to_le_bytes());
        }
        let mut pgen = std::vec::Vec::new();
        pgen.extend_from_slice(&41u16.to_le_bytes());
        pgen.extend_from_slice(&0u16.to_le_bytes());
        pgen.extend_from_slice(&41u16.to_le_bytes());
        pgen.extend_from_slice(&1u16.to_le_bytes());

        let mut inst = std::vec::Vec::new();
        // inst 0 has 2 ibags (soft vel 0..60 skipped + main vel 61..127 kept), inst 1 has 1 ibag
        let mut r = [0u8; 22];
        r[20..22].copy_from_slice(&0u16.to_le_bytes());
        inst.extend_from_slice(&r);
        r[20..22].copy_from_slice(&2u16.to_le_bytes());
        inst.extend_from_slice(&r);
        r[20..22].copy_from_slice(&3u16.to_le_bytes());
        inst.extend_from_slice(&r);

        let mut ibag = std::vec::Vec::new();
        for &g in &[0u16, 2, 4, 6] {
            ibag.extend_from_slice(&g.to_le_bytes());
            ibag.extend_from_slice(&0u16.to_le_bytes());
        }
        let mut igen = std::vec::Vec::new();
        // bag0 (soft vel 0..60 -> filtered out)
        igen.extend_from_slice(&44u16.to_le_bytes());
        igen.extend_from_slice(&((60u16 << 8) | 0u16).to_le_bytes());
        igen.extend_from_slice(&53u16.to_le_bytes());
        igen.extend_from_slice(&0u16.to_le_bytes());
        // bag1 (main vel 61..127, loop=1)
        igen.extend_from_slice(&54u16.to_le_bytes());
        igen.extend_from_slice(&1u16.to_le_bytes());
        igen.extend_from_slice(&53u16.to_le_bytes());
        igen.extend_from_slice(&0u16.to_le_bytes());
        // bag2 (drum key 36)
        igen.extend_from_slice(&43u16.to_le_bytes());
        igen.extend_from_slice(&((36u16 << 8) | 36u16).to_le_bytes());
        igen.extend_from_slice(&53u16.to_le_bytes());
        igen.extend_from_slice(&0u16.to_le_bytes());

        let mut shdr = std::vec::Vec::new();
        let mut sr = [0u8; 46];
        sr[20..24].copy_from_slice(&0u32.to_le_bytes());
        sr[24..28].copy_from_slice(&200u32.to_le_bytes());
        sr[28..32].copy_from_slice(&10u32.to_le_bytes());
        sr[32..36].copy_from_slice(&190u32.to_le_bytes());
        sr[36..40].copy_from_slice(&22050u32.to_le_bytes());
        sr[40] = 60;
        sr[44..46].copy_from_slice(&1u16.to_le_bytes());
        shdr.extend_from_slice(&sr);
        shdr.extend_from_slice(&[0u8; 46]);

        let mut pdta = std::vec::Vec::new();
        push_chunk(&mut pdta, b"phdr", &phdr);
        push_chunk(&mut pdta, b"pbag", &pbag);
        push_chunk(&mut pdta, b"pgen", &pgen);
        push_chunk(&mut pdta, b"inst", &inst);
        push_chunk(&mut pdta, b"ibag", &ibag);
        push_chunk(&mut pdta, b"igen", &igen);
        push_chunk(&mut pdta, b"shdr", &shdr);

        let mut st = Sf2State::new();
        st.smpl_offset = 0x100;
        st.smpl_samples = 256;
        parse_info_slice(&info, &mut st);
        parse_pdta_slice(&pdta, &mut st).expect("pdta parse should succeed");
        assert!(st.active);
        assert_eq!(&st.bank_name[..8], b"TestSC55");
        assert_eq!(st.programs_loaded, 1);
        assert_eq!(st.drums_loaded, 1);
        assert_eq!(st.zone_count, 1);

        let vp = resolve_zone_voice(0x0100_0000, &st.zones[0], 60, 0, false).unwrap();
        assert!(vp.looping);
        assert_eq!(vp.lba_phys, 0x0100_0000 + 20);
        assert_eq!(vp.eso, 179);
    }
}
