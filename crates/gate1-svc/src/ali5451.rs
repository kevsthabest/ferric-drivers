//! Gate 4a — ALi M5451 (`10B9:5451`) AC'97 Audio Driver (resident in FERRIC.JLM).
//!
//! Brings up the Toshiba DynaBook SS 2000M's BIOS-disabled ALi M5451 AC'97 audio
//! controller (`00:06.0`, rev `0x01`, subsys `1179:0004`):
//!   1. Allocates BAR0 I/O space (`0xE400`, 256 bytes) if left unassigned by BIOS.
//!   2. Ensures PCI D0 power state, I/O + Bus Master enable, and Latency Timer.
//!   3. Performs the M1533 (`0x7C`) + M5451 (`0x44`) AC'97 cold reset sequence.
//!   4. Initializes M5451 global control, unmutes the AC'97 codec mixer, and
//!      provides 16-bit signed stereo 48 kHz PCM DMA playback on Voice 0.
//!
//! Register offsets, bitfields, and reset sequences are verified against
//! `vendor/ref/linux-ali5451/ali5451.c` (Linux commit `c44104f3642d`).
//! Strictly `#![no_std]`, 386-pure (no FPU/MMX/SSE/486+), read-only with respect
//! to storage.

use core::cell::UnsafeCell;

#[cfg(target_arch = "x86")]
use core::arch::asm;

use crate::{pci_read32, pci_write32};

// ---------------------------------------------------------------------
// Verified PCI IDs & ALi M5451 / M1533 / M7101 constants
// (vendor/ref/linux-ali5451/ali5451.c)
// ---------------------------------------------------------------------

pub const PCI_VID_ALI: u16 = 0x10B9;
pub const PCI_DID_M5451: u16 = 0x5451;
pub const PCI_DID_M1533: u16 = 0x1533;
pub const PCI_DID_M7101: u16 = 0x7101;

pub const ALI_5451_V02: u8 = 0x02;

/// Default 256-byte aligned PCI I/O BAR0 assigned when BIOS leaves BAR0=0.
pub const DEFAULT_M5451_IO_BASE: u16 = 0xE400;

/// 16 KiB DMA arena:
///   - `0x0000..0x2FFF` (12,288 B = 3,072 stereo 16-bit frames): Voice 0 ring buffer
///     (512 frames = 2,048 B in 86 Hz IRQ mode; 3,072 frames = 139 ms in poll mode).
///   - `0x3000..0x3FFF` (4,096 B = 8 × 256 mono 16-bit samples): Built-in GM wavetable
///     ROM played directly by ALi M5451 hardware voices 1..=31 for MPU-401 MIDI!
pub const AUDIO_DMA_SIZE: usize = 16384;
pub const AUDIO_RING_BYTES: usize = 12288;
pub const AUDIO_DMA_FRAMES: u16 = (AUDIO_RING_BYTES / 4) as u16; // 3072
pub const WAVETABLE_OFFSET: usize = 12288;
pub const WAVETABLE_SAMPLES: usize = 256;
pub const WAVETABLE_STRIDE: usize = WAVETABLE_SAMPLES * 2; // 512 bytes per wave
pub const WAVETABLE_COUNT: usize = 8; // 8 × 512 = 4,096 bytes

// M5451 Direct I/O Register Offsets (ali5451.c lines 80-140)
pub const ALI_MPUR2: u16 = 0x22;
pub const ALI_AC97_WRITE: u16 = 0x40;
pub const ALI_AC97_READ: u16 = 0x44;
pub const ALI_SCTRL: u16 = 0x48;
pub const ALI_START: u16 = 0x80;
pub const ALI_STOP: u16 = 0x84;
pub const ALI_CSPF: u16 = 0x90;
pub const ALI_AINT: u16 = 0x98;
pub const ALI_GC_CIR: u16 = 0xA0;
pub const ALI_AINTEN: u16 = 0xA4;
pub const ALI_VOLUME: u16 = 0xA8;
pub const ALI_MISCINT: u16 = 0xB0;
pub const ALI_MISCINT_ADDR_IRQ: u32 = 0x0000_0020;
pub const ALI_MISCINT_CLEAR: u32 = 0x0000_8C00; // TARGET_REACHED | MIXER_OVERFLOW | MIXER_UNDERFLOW
pub const ALI_GC_CIR_IE: u32 = 0x0000_3000;     // ENDLP_IE | MIDLP_IE
pub const ALI_STIMER: u16 = 0xC8;

/// Low-latency hardware-IRQ ring buffer: 512 frames (2,048 bytes) @ 22,050 Hz
/// = 23.22 ms total, 11.61 ms per half-buffer (86.1 Hz hardware IRQ 11).
pub const SB_IRQ_RING_FRAMES: usize = 512;
pub const SB_IRQ_RING_BYTES: usize = SB_IRQ_RING_FRAMES * 4; // 2048
pub const SB_IRQ_HALF_FRAMES: usize = SB_IRQ_RING_FRAMES / 2; // 256
pub const ALI_GLOBAL_CONTROL: u16 = 0xD4;

// Per-channel banked registers (selected via low 5 bits of ALI_GC_CIR)
pub const ALI_CSO_ALPHA_FMS: u16 = 0xE0;
pub const ALI_LBA: u16 = 0xE4;
pub const ALI_ESO_DELTA: u16 = 0xE8;
pub const ALI_GVSEL_PAN_VOC_CTRL_EC: u16 = 0xF0;
pub const ALI_EBUF1: u16 = 0xF4;
pub const ALI_EBUF2: u16 = 0xF8;

// Standard AC'97 Codec Register Offsets
pub const AC97_RESET: u16 = 0x00;
pub const AC97_MASTER: u16 = 0x02;
pub const AC97_HEADPHONE: u16 = 0x04;
pub const AC97_MASTER_MONO: u16 = 0x06;
pub const AC97_PCM: u16 = 0x18;
pub const AC97_POWERDOWN: u16 = 0x26;
pub const AC97_EXTENDED_ID: u16 = 0x28;
pub const AC97_EXTENDED_STATUS: u16 = 0x2A;
pub const AC97_VENDOR_ID1: u16 = 0x7C;
pub const AC97_VENDOR_ID2: u16 = 0x7E;

// Voice CTRL bits (ali5451.c snd_ali_control_mode, lines 1039-1055)
pub const VOICE_CTRL_LOOP: u32 = 0x01;
pub const VOICE_CTRL_SIGNED: u32 = 0x02;
pub const VOICE_CTRL_STEREO: u32 = 0x04;
pub const VOICE_CTRL_16BIT: u32 = 0x08;
pub const VOICE_CTRL_S16_STEREO_LOOP: u32 =
    VOICE_CTRL_LOOP | VOICE_CTRL_SIGNED | VOICE_CTRL_STEREO | VOICE_CTRL_16BIT; // 0x0F

// Init stage codes for AUDPROBE diagnostics
pub const STAGE_NOT_FOUND: u8 = 0;
pub const STAGE_BAR_FAIL: u8 = 1;
pub const STAGE_DMA_FAIL: u8 = 2;
pub const STAGE_STIMER_DEAD: u8 = 3;
pub const STAGE_CODEC_TIMEOUT: u8 = 4;
pub const STAGE_READY: u8 = 5;

/// 40-byte diagnostic & status structure returned to `AUDPROBE.COM`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct AudioInfo {
    pub present: u8,       // +0x00
    pub stage: u8,         // +0x01
    pub bus: u8,           // +0x02
    pub devfn: u8,         // +0x03
    pub rev: u8,           // +0x04
    pub irq_line: u8,      // +0x05
    pub eapd_inv: u8,      // +0x06
    pub active: u8,        // +0x07
    pub io_base: u16,      // +0x08
    pub pwr_reg: u16,      // +0x0A
    pub codec_id: u32,     // +0x0C
    pub stimer_delta: u32, // +0x10
    pub cso_start: u16,    // +0x14
    pub cso_end: u16,      // +0x16
    pub cso_changes: u32,  // +0x18
    pub dma_phys: u32,     // +0x1C
    // Live stream telemetry, filled from current state on every snapshot
    // (previously the only irq11_count copy was the value frozen at
    // stream start, so AUDPROBE could not tell whether the 86 Hz
    // hardware path was actually pacing a running stream).
    pub irq11_count: u32,  // +0x20
    pub ring_mode: u8,     // +0x24: 0 = 512-frame IRQ ring, 1 = 4096-frame timer-poll ring
}

const _: () = {
    assert!(core::mem::size_of::<AudioInfo>() == 40);
};

struct AudioState {
    initialized: bool,
    present: bool,
    bus: u8,
    dev: u8,
    func: u8,
    rev: u8,
    irq_line: u8,
    io_base: u16,
    m1533_bdf: Option<(u8, u8, u8)>,
    m7101_bdf: Option<(u8, u8, u8)>,
    dma_lin: u32,
    dma_phys: u32,
    eapd_inv: bool,
    codec_id: u32,
    pwr_reg: u16,
    stimer_delta: u32,
    stage: u8,
    ac97_vol: u8,
    active: bool,
    sb_streaming: bool,
    write_frame: usize,
    last_fill_start: usize,
    ring_frames: usize,
    irq11_count: u32,
    irq11_probe_ticks: u8,
    diag_aint: u16,
    diag_misc: u16,
    diag_gc_sts: u32,
}

impl AudioState {
    const fn new() -> Self {
        Self {
            initialized: false,
            present: false,
            bus: 0,
            dev: 0,
            func: 0,
            rev: 0,
            irq_line: 0xFF,
            io_base: 0,
            m1533_bdf: None,
            m7101_bdf: None,
            dma_lin: 0,
            dma_phys: 0,
            eapd_inv: false,
            codec_id: 0,
            pwr_reg: 0,
            stimer_delta: 0,
            stage: STAGE_NOT_FOUND,
            ac97_vol: 7,
            active: false,
            sb_streaming: false,
            write_frame: 0,
            last_fill_start: usize::MAX,
            ring_frames: AUDIO_DMA_FRAMES as usize,
            irq11_count: 0,
            irq11_probe_ticks: 0,
            diag_aint: 0,
            diag_misc: 0,
            diag_gc_sts: 0,
        }
    }
}

struct SyncState(UnsafeCell<AudioState>);
unsafe impl Sync for SyncState {}

static AUDIO: SyncState = SyncState(UnsafeCell::new(AudioState::new()));

// ---------------------------------------------------------------------
// Low-level x86 port I/O & busy-wait delay helpers
// ---------------------------------------------------------------------

#[cfg(target_arch = "x86")]
#[inline]
unsafe fn io_outb(port: u16, val: u8) {
    unsafe { asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags)) };
}

#[cfg(not(target_arch = "x86"))]
#[inline]
unsafe fn io_outb(_port: u16, _val: u8) {}

#[cfg(target_arch = "x86")]
#[inline]
unsafe fn io_inw(port: u16) -> u16 {
    let v: u16;
    unsafe { asm!("in ax, dx", out("ax") v, in("dx") port, options(nomem, nostack, preserves_flags)) };
    v
}

#[cfg(not(target_arch = "x86"))]
#[inline]
unsafe fn io_inw(_port: u16) -> u16 {
    0
}

#[cfg(target_arch = "x86")]
#[inline]
unsafe fn io_inl(port: u16) -> u32 {
    let v: u32;
    unsafe { asm!("in eax, dx", out("eax") v, in("dx") port, options(nomem, nostack, preserves_flags)) };
    v
}

#[cfg(not(target_arch = "x86"))]
#[inline]
unsafe fn io_inl(_port: u16) -> u32 {
    0
}

#[cfg(target_arch = "x86")]
#[inline]
unsafe fn io_outl(port: u16, val: u32) {
    unsafe { asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack, preserves_flags)) };
}

#[cfg(not(target_arch = "x86"))]
#[inline]
unsafe fn io_outl(_port: u16, _val: u32) {}

/// Approximate microsecond delay using legacy ISA port 0x80 (~1 us per I/O write).
#[inline]
unsafe fn udelay(us: u32) {
    for _ in 0..us {
        unsafe { io_outb(0x80, 0) };
    }
}

#[inline]
unsafe fn mdelay(ms: u32) {
    for _ in 0..ms {
        unsafe { udelay(1000) };
    }
}

// ---------------------------------------------------------------------
// Pure register-encoding & rate-conversion helpers (host unit-tested)
// ---------------------------------------------------------------------

/// Convert PCM playback sample rate (Hz) to ALi M5451 16-bit DELTA
/// (verified against `snd_ali_convert_rate` in `ali5451.c` lines 1007-1037).
pub const fn convert_playback_rate(rate: u32) -> u16 {
    let r = if rate < 4000 {
        4000
    } else if rate > 48000 {
        48000
    } else {
        rate
    };
    if r == 44100 {
        0x0EB3
    } else if r == 8000 {
        0x02AB
    } else if r == 48000 {
        0x1000
    } else {
        ((((r << 12) + r) / 48000) & 0xFFFF) as u16
    }
}

/// Pack ALi M5451 AC'97 write command dword (`ali5451.c` lines 340-345).
pub const fn pack_ac97_write(reg: u16, val: u16, secondary: bool, rev: u8) -> u32 {
    let mut dw = ((reg & 0x7F) as u32) | 0x8000 | ((val as u32) << 16);
    if secondary {
        dw |= 0x0080;
    }
    if rev == ALI_5451_V02 {
        dw |= 0x0100;
    }
    dw
}

/// Pack ALi M5451 AC'97 read command dword (`ali5451.c` lines 372-375).
pub const fn pack_ac97_read(reg: u16, secondary: bool) -> u32 {
    let mut dw = ((reg & 0x7F) as u32) | 0x8000;
    if secondary {
        dw |= 0x0080;
    }
    dw
}

/// Pack the 4 voice control dwords for `snd_ali_write_voice_regs` (`ali5451.c` lines 987-994).
pub const fn pack_voice_regs(
    lba: u32,
    cso: u16,
    eso: u16,
    delta: u16,
    gvsel: u32,
    pan: u32,
    vol: u32,
    ctrl: u32,
    ec: u32,
) -> [u32; 4] {
    [
        (cso as u32) << 16,
        lba,
        ((eso as u32) << 16) | (delta as u32),
        ((gvsel & 1) << 31)
            | ((pan & 0x7F) << 24)
            | ((vol & 0xFF) << 16)
            | ((ctrl & 0x0F) << 12)
            | (ec & 0x0FFF),
    ]
}

// Quarter-wave 16-bit sine table: sin(i * pi / 128) * 12000 for i = 0..=64
// Amplitude 12000 (~-8.7 dBFS) is clean, distortion-free, and comfortable on laptop speakers.
const SIN_QTR: [i16; 65] = [
    0, 295, 589, 883, 1176, 1469, 1761, 2051,
    2341, 2629, 2915, 3200, 3483, 3763, 4042, 4318,
    4592, 4863, 5131, 5396, 5658, 5916, 6171, 6423,
    6670, 6913, 7153, 7388, 7618, 7844, 8065, 8282,
    8493, 8699, 8900, 9095, 9285, 9469, 9647, 9820,
    9987, 10147, 10302, 10450, 10592, 10727, 10856, 10979,
    11095, 11204, 11306, 11402, 11491, 11573, 11648, 11716,
    11777, 11831, 11878, 11918, 11951, 11977, 11995, 12007,
    12000,
];

/// Evaluate integer sine from an 8-bit phase (`0..255` = `0..2*pi`).
#[inline]
pub fn sin_i16(phase8: u8) -> i16 {
    let q = phase8 >> 6;
    let idx = (phase8 & 0x3F) as usize;
    match q {
        0 => SIN_QTR[idx],
        1 => SIN_QTR[64 - idx],
        2 => -SIN_QTR[idx],
        _ => -SIN_QTR[64 - idx],
    }
}

/// Fill a 16-bit signed stereo PCM buffer (`buf.len() >= AUDIO_DMA_SIZE`) with a
/// seamless periodic sine wave at approximately `freq_hz` (sampled at 48,000 Hz).
/// Rounds to an integer number of cycles across the buffer so looping is click-free.
pub fn fill_sine_stereo_48k(buf: &mut [u8], freq_hz: u32) {
    let frames = buf.len() / 4;
    if frames == 0 {
        return;
    }
    // Exact integer cycles across `frames` at 48 kHz so wraparound is click-free.
    let mut cycles = (freq_hz * (frames as u32) + 24000) / 48000;
    if cycles == 0 {
        cycles = 1;
    }
    // 16.16 fixed-point phase in [0, 256)
    let step: u32 = ((cycles << 16) * 256) / (frames as u32);
    let mut acc: u32 = 0;
    for f in 0..frames {
        let s = sin_i16((acc >> 16) as u8);
        let lo = (s & 0xFF) as u8;
        let hi = ((s >> 8) & 0xFF) as u8;
        let off = f * 4;
        buf[off] = lo;
        buf[off + 1] = hi;
        buf[off + 2] = lo;
        buf[off + 3] = hi;
        acc = acc.wrapping_add(step);
    }
}

// ---------------------------------------------------------------------
// ALi M5451 hardware access routines
// ---------------------------------------------------------------------

unsafe fn find_pci_device(vid: u16, did: u16) -> Option<(u8, u8, u8)> {
    let want = ((did as u32) << 16) | (vid as u32);
    for dev in 0..32u8 {
        let id0 = unsafe { pci_read32(0, dev, 0, 0x00) };
        if id0 == 0xFFFF_FFFF {
            continue;
        }
        if id0 == want {
            return Some((0, dev, 0));
        }
        let hdr = (unsafe { pci_read32(0, dev, 0, 0x0C) } >> 16) as u8;
        if (hdr & 0x80) != 0 {
            for func in 1..8u8 {
                if unsafe { pci_read32(0, dev, func, 0x00) } == want {
                    return Some((0, dev, func));
                }
            }
        }
    }
    None
}

/// Ensure PCI Power Management D0 state if PM capability is present.
unsafe fn ensure_pci_d0(bus: u8, dev: u8, func: u8) {
    let sts = (unsafe { pci_read32(bus, dev, func, 0x04) } >> 16) as u16;
    if (sts & 0x0010) == 0 {
        return;
    }
    let mut ptr = (unsafe { pci_read32(bus, dev, func, 0x34) } & 0xFC) as u8;
    let mut guard = 0u8;
    while ptr >= 0x40 && guard < 16 {
        guard += 1;
        let cap = unsafe { pci_read32(bus, dev, func, ptr) };
        if (cap & 0xFF) == 0x01 {
            let pmcsr = unsafe { pci_read32(bus, dev, func, ptr + 4) };
            if (pmcsr & 0x03) != 0 {
                unsafe {
                    pci_write32(bus, dev, func, ptr + 4, pmcsr & !0x03);
                    mdelay(10);
                }
            }
            return;
        }
        ptr = ((cap >> 8) & 0xFC) as u8;
    }
}

unsafe fn codec_ready(io_base: u16, reg_port: u16) -> bool {
    for _ in 0..250u32 {
        let res = unsafe { io_inl(io_base + reg_port) };
        if (res & 0x8000) == 0 {
            return true;
        }
        unsafe { udelay(200) };
    }
    let res = unsafe { io_inl(io_base + reg_port) };
    unsafe { io_outl(io_base + reg_port, res & !0x8000) };
    false
}

unsafe fn stimer_ready(io_base: u16) -> bool {
    let t1 = unsafe { io_inl(io_base + ALI_STIMER) };
    for _ in 0..500u32 {
        let t2 = unsafe { io_inl(io_base + ALI_STIMER) };
        if t2 != t1 {
            return true;
        }
        unsafe { udelay(50) };
    }
    false
}

unsafe fn ac97_write(io_base: u16, rev: u8, reg: u16, val: u16) -> bool {
    if reg >= 0x80 {
        return false;
    }
    if !unsafe { codec_ready(io_base, ALI_AC97_WRITE) } {
        return false;
    }
    if !unsafe { stimer_ready(io_base) } {
        return false;
    }
    let dw = pack_ac97_write(reg, val, false, rev);
    unsafe { io_outl(io_base + ALI_AC97_WRITE, dw) };
    true
}

unsafe fn ac97_read(io_base: u16, rev: u8, reg: u16) -> u16 {
    if reg >= 0x80 {
        return 0xFFFF;
    }
    let port = if rev == ALI_5451_V02 {
        ALI_AC97_WRITE
    } else {
        ALI_AC97_READ
    };
    if !unsafe { codec_ready(io_base, port) } {
        return 0xFFFF;
    }
    if !unsafe { stimer_ready(io_base) } {
        return 0xFFFF;
    }
    let dw = pack_ac97_read(reg, false);
    unsafe { io_outl(io_base + port, dw) };
    if !unsafe { stimer_ready(io_base) } {
        return 0xFFFF;
    }
    if !unsafe { codec_ready(io_base, port) } {
        return 0xFFFF;
    }
    ((unsafe { io_inl(io_base + port) } >> 16) & 0xFFFF) as u16
}

unsafe fn reset_5451(st: &AudioState) -> (bool, u16) {
    // 1. Pulse M1533 southbridge AC'97 reset bit (0x7C bit 27)
    if let Some((b, d, f)) = st.m1533_bdf {
        unsafe {
            let v = pci_read32(b, d, f, 0x7C);
            pci_write32(b, d, f, 0x7C, v | 0x0800_0000);
            mdelay(5);
            let v2 = pci_read32(b, d, f, 0x7C);
            pci_write32(b, d, f, 0x7C, v2 & 0xF7FF_FFFF);
            mdelay(5);
        }
    }

    // 2. Pulse M5451 PCI config 0x44 bits 19:18
    unsafe {
        let v = pci_read32(st.bus, st.dev, st.func, 0x44);
        pci_write32(st.bus, st.dev, st.func, 0x44, v | 0x000C_0000);
        udelay(500);
        let v2 = pci_read32(st.bus, st.dev, st.func, 0x44);
        pci_write32(st.bus, st.dev, st.func, 0x44, v2 & 0xFFFB_FFFF);
        mdelay(5);
    }

    // 3. Poll AC97_POWERDOWN (0x26) until ADC/DAC/ANL/REF ready (low nibble == 0xF)
    let mut last = 0xFFFFu16;
    for _ in 0..200u32 {
        last = unsafe { ac97_read(st.io_base, st.rev, AC97_POWERDOWN) };
        if last != 0xFFFF && (last & 0x000F) == 0x000F {
            return (true, last);
        }
        unsafe { mdelay(5) };
    }
    (false, last)
}

unsafe fn configure_mixer(io_base: u16, rev: u8, eapd_inv: bool) -> u16 {
    let pwr_val: u16 = if eapd_inv { 0x8000 } else { 0x0000 };
    unsafe {
        ac97_write(io_base, rev, AC97_POWERDOWN, pwr_val);
        mdelay(5);
        // Unmute Master, Headphone/Aux, Mono Out, and PCM Out at comfortable levels
        ac97_write(io_base, rev, AC97_MASTER, 0x0606);
        ac97_write(io_base, rev, AC97_HEADPHONE, 0x0606);
        ac97_write(io_base, rev, AC97_MASTER_MONO, 0x0006);
        ac97_write(io_base, rev, AC97_PCM, 0x0808);
        ac97_read(io_base, rev, AC97_POWERDOWN)
    }
}

/// Populate the 4 KiB built-in GM wavetable ROM at `dma_lin + WAVETABLE_OFFSET`
/// (8 × 256 mono 16-bit signed looped waveforms) using pure integer math:
///   0: Piano / Bright Harmonic    4: Brass / Reed
///   1: Organ / Drawbar            5: Flute / Pipe
///   2: Guitar / Pluck / Bass      6: Synth Lead (Pulse)
///   3: Warm Strings / Pad         7: Percussion Noise / Drum
unsafe fn populate_wavetable_rom(dma_lin: u32) {
    if dma_lin == 0 {
        return;
    }
    let wt = unsafe {
        core::slice::from_raw_parts_mut(
            (dma_lin as usize + WAVETABLE_OFFSET) as *mut u8,
            WAVETABLE_COUNT * WAVETABLE_STRIDE,
        )
    };
    let mut lfsr: u32 = 0xACE1_BEEF;
    for w in 0..WAVETABLE_COUNT {
        let base = w * WAVETABLE_STRIDE;
        for i in 0..WAVETABLE_SAMPLES {
            let p1 = i as u8;
            let p2 = p1.wrapping_mul(2);
            let p3 = p1.wrapping_mul(3);
            let p4 = p1.wrapping_mul(4);
            let p6 = p1.wrapping_mul(6);
            let s1 = sin_i16(p1) as i32;
            let s2 = sin_i16(p2) as i32;
            let s3 = sin_i16(p3) as i32;
            let s4 = sin_i16(p4) as i32;
            let s6 = sin_i16(p6) as i32;
            let sample: i32 = match w {
                // 0: Piano / Bright Harmonic
                0 => (s1 * 5 + s2 * 3 + s3 * 2 + s4) / 10,
                // 1: Organ / Drawbar
                1 => (s1 * 4 + s2 * 3 + s3 * 2 + s4 * 2 + s6) / 11,
                // 2: Guitar / Pluck / Bass (rich saw + tri blend)
                2 => {
                    let saw = ((i as i32) - 128) * 70;
                    (saw + s1 + (s2 >> 1)) / 2
                }
                // 3: Warm Strings / Pad (soft band-limited sawtooth)
                3 => (s1 * 6 - s2 * 3 + s3 * 2 - s4) / 9,
                // 4: Brass / Reed (strong odd + even upper harmonics)
                4 => (s1 * 4 + s2 * 3 + s3 * 3 + s4 * 2) / 11,
                // 5: Flute / Pipe (pure fundamental + gentle breath)
                5 => (s1 * 7 + s2) / 8,
                // 6: Synth Lead (30% pulse wave, band-softened)
                6 => {
                    let sq = if i < 77 { 8000 } else { -8000 };
                    (sq * 2 + s1) / 3
                }
                // 7: Percussion (LFSR noise + low thump)
                _ => {
                    let bit = (lfsr ^ (lfsr >> 2) ^ (lfsr >> 3) ^ (lfsr >> 5)) & 1;
                    lfsr = (lfsr >> 1) | (bit << 31);
                    let noise = ((lfsr & 0x3FFF) as i32) - 8192;
                    let thump = s1 >> 1;
                    (noise + thump).clamp(-12000, 12000)
                }
            };
            let s16 = sample.clamp(-14000, 14000) as i16;
            let off = base + i * 2;
            wt[off] = (s16 & 0xFF) as u8;
            wt[off + 1] = ((s16 >> 8) & 0xFF) as u8;
        }
    }
}

/// Start one ALi M5451 hardware wavetable voice (`voice` in `1..=31`) for
/// MPU-401 General MIDI playback. Uses the 4DWAVE-DX Bank A address, pitch
/// (`DELTA`), volume, pan, and 26-bit digital mixer directly in silicon.
pub fn hw_midi_note_on(
    voice: u8,
    wave_idx: u8,
    delta: u16,
    vol_atten: u8,
    pan: u8,
    looping: bool,
) {
    let st = unsafe { &*AUDIO.0.get() };
    if !st.initialized || !st.present || st.io_base == 0 || st.dma_phys == 0 {
        return;
    }
    let lba = st.dma_phys + WAVETABLE_OFFSET as u32 + ((wave_idx as u32 & 7) * WAVETABLE_STRIDE as u32);
    hw_midi_note_on_pcm(
        voice,
        lba,
        0,
        (WAVETABLE_SAMPLES - 1) as u16,
        delta,
        vol_atten,
        pan,
        looping,
    );
}

/// Start one ALi M5451 hardware wavetable voice (`voice` in `1..=31`) directly
/// from an arbitrary 30-bit physical 16-bit signed mono PCM buffer (used by
/// both the built-in 4 KiB ROM and XMS-loaded `.SF2` SoundFont banks).
pub fn hw_midi_note_on_pcm(
    voice: u8,
    lba_phys: u32,
    cso: u16,
    eso: u16,
    delta: u16,
    vol_atten: u8,
    pan: u8,
    looping: bool,
) {
    let st = unsafe { &*AUDIO.0.get() };
    if !st.initialized || !st.present || st.io_base == 0 || lba_phys == 0 {
        return;
    }
    if voice == 0 || voice >= 32 {
        return;
    }
    let ctrl = VOICE_CTRL_SIGNED | VOICE_CTRL_16BIT | if looping { VOICE_CTRL_LOOP } else { 0 };
    let regs = pack_voice_regs(
        lba_phys,
        cso,
        eso,
        delta.max(1),
        1,                // GVSEL = 1 (global music/wave volume)
        pan as u32,       // 7-bit 4DWAVE pan
        vol_atten as u32, // 8-bit 0.375 dB attenuation (0 = loudest, 0xFF = silent)
        ctrl,
        0,
    );
    let mask = 1u32 << voice;
    let io_base = st.io_base;
    unsafe {
        io_outl(io_base + ALI_STOP, mask);
        io_outb(io_base + ALI_GC_CIR, voice);
        io_outl(io_base + ALI_CSO_ALPHA_FMS, regs[0]);
        io_outl(io_base + ALI_LBA, regs[1]);
        io_outl(io_base + ALI_ESO_DELTA, regs[2]);
        io_outl(io_base + ALI_GVSEL_PAN_VOC_CTRL_EC, regs[3]);
        io_outl(io_base + ALI_EBUF1, 0x3000_0000);
        io_outl(io_base + ALI_EBUF2, 0x3000_0000);
        io_outl(io_base + ALI_START, mask);
    }
}

/// Stop one ALi M5451 hardware MIDI voice (`voice` in `1..=31`).
pub fn hw_midi_note_off(voice: u8) {
    let st = unsafe { &*AUDIO.0.get() };
    if !st.initialized || !st.present || st.io_base == 0 || voice == 0 || voice >= 32 {
        return;
    }
    unsafe {
        io_outl(st.io_base + ALI_STOP, 1u32 << voice);
    }
}

/// Stop all ALi M5451 hardware MIDI voices (`1..=31`), leaving Voice 0 untouched.
pub fn hw_midi_stop_all() {
    let st = unsafe { &*AUDIO.0.get() };
    if !st.initialized || !st.present || st.io_base == 0 {
        return;
    }
    unsafe {
        io_outl(st.io_base + ALI_STOP, 0xFFFF_FFFE);
    }
}

unsafe fn start_voice0(io_base: u16, lba_phys: u32, eso: u16, delta: u16) {
    let regs = pack_voice_regs(
        lba_phys,
        0,
        eso,
        delta,
        1, // GVSEL = 1 (global volume)
        0, // PAN = center
        0, // VOL = 0 dB attenuation
        VOICE_CTRL_S16_STEREO_LOOP,
        0, // EC = 0
    );
    unsafe {
        io_outl(io_base + ALI_STOP, 1);
        io_outb(io_base + ALI_GC_CIR, 0);
        io_outl(io_base + ALI_CSO_ALPHA_FMS, regs[0]);
        io_outl(io_base + ALI_LBA, regs[1]);
        io_outl(io_base + ALI_ESO_DELTA, regs[2]);
        io_outl(io_base + ALI_GVSEL_PAN_VOC_CTRL_EC, regs[3]);
        io_outl(io_base + ALI_EBUF1, 0x3000_0000);
        io_outl(io_base + ALI_EBUF2, 0x3000_0000);
        io_outl(io_base + ALI_START, 1);
    }
}

unsafe fn stop_voice0(io_base: u16) {
    unsafe {
        io_outl(io_base + ALI_STOP, 1);
    }
}

unsafe fn read_voice0_cso(io_base: u16) -> u16 {
    unsafe {
        io_outb(io_base + ALI_GC_CIR, 0);
        io_inw(io_base + ALI_CSO_ALPHA_FMS + 2)
    }
}

fn snapshot_info(st: &AudioState, cso_start: u16, cso_end: u16, cso_changes: u32) -> AudioInfo {
    AudioInfo {
        present: if st.present { 1 } else { 0 },
        stage: st.stage,
        bus: st.bus,
        devfn: (st.dev << 3) | (st.func & 7),
        rev: st.rev,
        irq_line: st.irq_line,
        eapd_inv: if st.eapd_inv { 1 } else { 0 },
        active: if st.active { 1 } else { 0 },
        io_base: st.io_base,
        pwr_reg: st.pwr_reg,
        codec_id: st.codec_id,
        stimer_delta: st.stimer_delta,
        cso_start,
        cso_end,
        cso_changes,
        dma_phys: st.dma_phys,
        irq11_count: st.irq11_count,
        ring_mode: if st.ring_frames == SB_IRQ_RING_FRAMES { 0 } else { 1 },
    }
}

// ---------------------------------------------------------------------
// Exported Ring-0 C ABI entry points called from jlm_stub.asm
// ---------------------------------------------------------------------

fn route_audio_irq(st: &mut AudioState) {
    // Route ALi M5451 (00:06.0 INTA#) onto shared level-triggered PCI IRQ 11
    // (nibble 0x09 in ALi M1533 irqmap). Scan BIOS ROM 0xF0000..0xFFFF0 for
    // the $PIR PCI IRQ Routing Table to get the exact M1533 link byte for
    // 00:06.0 INTA#, then apply Linux's exact pirq_ali_set(0x48, link-1, 9).
    let mut pir_link: u8 = 0;
    let mut pir_bmap: u16 = 0;
    unsafe {
        let mut off = 0x000F_0000usize;
        while off < 0x000F_FFF0 {
            let sig = core::ptr::read_volatile(off as *const u32);
            if sig == 0x5249_5024 {
                let sz = core::ptr::read_volatile((off + 6) as *const u16) as usize;
                if sz >= 32 && sz <= 4096 && (off + sz) <= 0x0010_0000 {
                    let mut sum: u8 = 0;
                    for i in 0..sz {
                        sum = sum.wrapping_add(core::ptr::read_volatile((off + i) as *const u8));
                    }
                    if sum == 0 {
                        let mut e = off + 32;
                        while e + 16 <= off + sz {
                            let eb = core::ptr::read_volatile(e as *const u8);
                            let ed = core::ptr::read_volatile((e + 1) as *const u8) >> 3;
                            if eb == st.bus && ed == st.dev {
                                pir_link = core::ptr::read_volatile((e + 2) as *const u8);
                                pir_bmap = core::ptr::read_volatile((e + 3) as *const u16);
                                break;
                            }
                            e += 16;
                        }
                        break;
                    }
                }
            }
            off += 16;
        }
    }
    st.diag_gc_sts = ((pir_link as u32) << 16) | (pir_bmap as u32);

    if let Some((b, d, f)) = st.m1533_bdf {
        unsafe {
            if pir_link >= 1 && pir_link <= 16 {
                let nr = (pir_link - 1) as u8;
                let reg = 0x48u8 + (nr >> 1);
                let aligned = reg & 0xFC;
                let byte_shift = ((reg & 3) as u32) * 8;
                let nib_shift = byte_shift + if (nr & 1) != 0 { 4 } else { 0 };
                let dw = pci_read32(b, d, f, aligned);
                let new_dw = (dw & !(0x0Fu32 << nib_shift)) | (0x09u32 << nib_shift);
                pci_write32(b, d, f, aligned, new_dw);
            } else if pir_link >= 0x40 && pir_link <= 0x7F {
                let aligned = pir_link & 0xFC;
                let byte_shift = ((pir_link & 3) as u32) * 8;
                let dw = pci_read32(b, d, f, aligned);
                let new_dw = (dw & !(0x0Fu32 << byte_shift)) | (0x09u32 << byte_shift);
                pci_write32(b, d, f, aligned, new_dw);
            } else {
                let r74 = pci_read32(b, d, f, 0x74);
                let new74 = (r74 & 0xFFFF_F0F0) | 0x0000_0909;
                pci_write32(b, d, f, 0x74, new74);
            }
            let r3c = pci_read32(st.bus, st.dev, st.func, 0x3C);
            pci_write32(st.bus, st.dev, st.func, 0x3C, (r3c & 0xFFFF_FF00) | 11);
            st.irq_line = 11;
            // Unmask IRQ 11 on slave PIC (0xA1 bit 3) and cascade IRQ 2 on master PIC (0x21 bit 2)
            let m2 = crate::inb(0xA1);
            io_outb(0xA1, m2 & !0x08);
            let m1 = crate::inb(0x21);
            io_outb(0x21, m1 & !0x04);
        }
    }
}

/// Prepare the codec and PCI interrupt route on a fresh boot. The external
/// driver owns the DMA allocation, interrupt handler and voice from here.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_audio_external_prepare() -> u32 {
    if unsafe { (*AUDIO.0.get()).initialized } {
        return 6;
    }
    let rc = gate1_audio_init(0, 0, 0);
    if rc != 0 {
        return rc;
    }
    let st = unsafe { &mut *AUDIO.0.get() };
    route_audio_irq(st);
    if st.irq_line == 11 { 0 } else { 7 }
}

/// `AX=0x0020`: Initialize ALi M5451 AC'97 audio controller.
/// `dma_seg`: real-mode segment of a 16 KiB DOS DMA buffer (or 0 to reuse existing).
/// `flags`: bit 0 = invert EAPD bit 15 in AC97_POWERDOWN.
/// `out_ptr`: flat linear pointer to a 40-byte `AudioInfo` struct.
/// Returns `0` on success (`STAGE_READY`), non-zero error/stage code otherwise.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_audio_init(dma_seg: u32, flags: u32, out_ptr: u32) -> u32 {
    let st = unsafe { &mut *AUDIO.0.get() };
    let eapd_inv = (flags & 1) != 0;

    // 1. Locate ALi M5451 + M1533 + M7101
    let (bus, dev, func) = match unsafe { find_pci_device(PCI_VID_ALI, PCI_DID_M5451) } {
        Some(bdf) => bdf,
        None => {
            st.present = false;
            st.stage = STAGE_NOT_FOUND;
            if out_ptr != 0 {
                unsafe { core::ptr::write_unaligned(out_ptr as *mut AudioInfo, snapshot_info(st, 0, 0, 0)) };
            }
            return 1;
        }
    };

    st.present = true;
    st.bus = bus;
    st.dev = dev;
    st.func = func;
    st.m1533_bdf = unsafe { find_pci_device(PCI_VID_ALI, PCI_DID_M1533) };
    st.m7101_bdf = unsafe { find_pci_device(PCI_VID_ALI, PCI_DID_M7101) };
    st.rev = (unsafe { pci_read32(bus, dev, func, 0x08) } & 0xFF) as u8;
    st.irq_line = (unsafe { pci_read32(bus, dev, func, 0x3C) } & 0xFF) as u8;
    st.eapd_inv = eapd_inv;

    // 2. Ensure PCI D0, BAR0 I/O space, Command (IO | MASTER), and Latency Timer
    unsafe { ensure_pci_d0(bus, dev, func) };

    let mut bar0 = unsafe { pci_read32(bus, dev, func, 0x10) };
    let mut io_base = (bar0 & 0xFF00) as u16;
    if io_base == 0 {
        unsafe {
            pci_write32(bus, dev, func, 0x10, (DEFAULT_M5451_IO_BASE as u32) | 1);
            bar0 = pci_read32(bus, dev, func, 0x10);
        }
        io_base = (bar0 & 0xFF00) as u16;
    }
    if io_base == 0 || io_base == 0xFF00 {
        st.stage = STAGE_BAR_FAIL;
        if out_ptr != 0 {
            unsafe { core::ptr::write_unaligned(out_ptr as *mut AudioInfo, snapshot_info(st, 0, 0, 0)) };
        }
        return 2;
    }
    st.io_base = io_base;

    unsafe {
        let cmd = (pci_read32(bus, dev, func, 0x04) & 0xFFFF) as u16;
        pci_write32(bus, dev, func, 0x04, (cmd | 0x0005) as u32);
        let clt = pci_read32(bus, dev, func, 0x0C);
        if ((clt >> 8) & 0xFF) == 0 {
            pci_write32(bus, dev, func, 0x0C, (clt & 0xFFFF_00FF) | 0x0000_4000);
        }
    }

    // 3. Lock 16 KiB DMA buffer via VDMAD (PCI 32-bit contiguity, no 64 KiB ISA limit)
    if dma_seg != 0 {
        let lin = (dma_seg & 0xFFFF) << 4;
        if st.dma_lin != 0 && st.dma_lin != lin {
            unsafe { crate::vdmad_unlock_dma_region(st.dma_lin, AUDIO_DMA_SIZE as u32) };
            st.dma_lin = 0;
            st.dma_phys = 0;
        }
        if st.dma_lin == 0 {
            let phys = match unsafe { crate::vdmad_lock_dma_region_pci(lin, AUDIO_DMA_SIZE as u32) } {
                Some(p) => p,
                None => lin, // Fallback when VDMAD not active (identity-mapped low RAM)
            };
            if phys == 0 || (phys & 0x8000_0000) != 0 {
                st.stage = STAGE_DMA_FAIL;
                if out_ptr != 0 {
                    unsafe { core::ptr::write_unaligned(out_ptr as *mut AudioInfo, snapshot_info(st, 0, 0, 0)) };
                }
                return 3;
            }
            st.dma_lin = lin;
            st.dma_phys = phys;
        }
    }

    // 4. Cold reset M1533 + M5451 and verify STIMER + AC'97 codec
    let (reset_ok, pwr_after_reset) = unsafe { reset_5451(st) };

    // V02 chipset hooks (ali5451.c lines 1909-1919)
    if st.rev == ALI_5451_V02 {
        if let Some((b, d, f)) = st.m1533_bdf {
            unsafe {
                let r = pci_read32(b, d, f, 0x58);
                pci_write32(b, d, f, 0x58, r | 0x0000_8000); // byte 0x59 |= 0x80
            }
        }
        if let Some((b, d, f)) = st.m7101_bdf {
            unsafe {
                let r = pci_read32(b, d, f, 0xB8);
                pci_write32(b, d, f, 0xB8, r | 0x0000_0020); // byte 0xB8 |= 0x20
            }
        }
    }

    // Legacy & global control init (ali5451.c lines 1921-1930)
    unsafe {
        let mut legacy = pci_read32(bus, dev, func, 0x44);
        legacy &= 0xFF00_FF00;
        legacy |= 0x0008_00AA;
        pci_write32(bus, dev, func, 0x44, legacy);
    }

    unsafe {
        io_outl(io_base + ALI_STOP, 0xFFFF_FFFF);
        io_outl(io_base + ALI_GLOBAL_CONTROL, 0x8000_0001);
        io_outl(io_base + ALI_AINTEN, 0x0000_0000);
        io_outl(io_base + ALI_AINT, 0xFFFF_FFFF);
        io_outl(io_base + ALI_VOLUME, 0x0000_0000);
        io_outb(io_base + ALI_MPUR2, 0x10);
    }

    // Measure STIMER tick delta over ~2 ms
    let t1 = unsafe { io_inl(io_base + ALI_STIMER) };
    unsafe { mdelay(2) };
    let t2 = unsafe { io_inl(io_base + ALI_STIMER) };
    st.stimer_delta = t2.wrapping_sub(t1);
    if st.stimer_delta == 0 {
        st.pwr_reg = pwr_after_reset;
        st.stage = STAGE_STIMER_DEAD;
        if out_ptr != 0 {
            unsafe { core::ptr::write_unaligned(out_ptr as *mut AudioInfo, snapshot_info(st, 0, 0, 0)) };
        }
        return 4;
    }

    // Read AC'97 Vendor ID1/ID2 and configure mixer
    let id1 = unsafe { ac97_read(io_base, st.rev, AC97_VENDOR_ID1) };
    let id2 = unsafe { ac97_read(io_base, st.rev, AC97_VENDOR_ID2) };
    st.codec_id = ((id1 as u32) << 16) | (id2 as u32);
    st.pwr_reg = unsafe { configure_mixer(io_base, st.rev, eapd_inv) };

    st.initialized = true;
    st.active = false;
    st.stage = if reset_ok || (st.pwr_reg != 0xFFFF && (st.pwr_reg & 0x000F) == 0x000F) {
        STAGE_READY
    } else {
        STAGE_CODEC_TIMEOUT
    };

    if out_ptr != 0 {
        unsafe { core::ptr::write_unaligned(out_ptr as *mut AudioInfo, snapshot_info(st, 0, 0, 0)) };
    }
    if st.stage == STAGE_READY { 0 } else { 5 }
}

/// `AX=0x0021`: Play a pleasant 3-note chime (C5-E5-G5) through ALi M5451 Voice 0
/// and report DMA `CSO` advancement in `out_ptr` (`AudioInfo`).
/// `flags`: bit 0 = invert EAPD bit 15 before playing.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_audio_chime(flags: u32, out_ptr: u32) -> u32 {
    let st = unsafe { &mut *AUDIO.0.get() };
    if !st.initialized || st.dma_lin == 0 || st.dma_phys == 0 || st.io_base == 0 {
        if out_ptr != 0 {
            unsafe { core::ptr::write_unaligned(out_ptr as *mut AudioInfo, snapshot_info(st, 0, 0, 0)) };
        }
        return 1;
    }

    let eapd_inv = (flags & 1) != 0;
    if eapd_inv != st.eapd_inv {
        st.eapd_inv = eapd_inv;
        st.pwr_reg = unsafe { configure_mixer(st.io_base, st.rev, eapd_inv) };
    }

    let dma_slice = unsafe { core::slice::from_raw_parts_mut(st.dma_lin as *mut u8, AUDIO_RING_BYTES) };
    let notes: [u32; 3] = [523, 659, 784]; // C5, E5, G5
    let delta = convert_playback_rate(48000);
    let eso = AUDIO_DMA_FRAMES - 1;

    let mut cso_start = 0u16;
    let mut cso_end = 0u16;
    let mut cso_changes = 0u32;

    st.active = true;
    for (idx, &freq) in notes.iter().enumerate() {
        fill_sine_stereo_48k(dma_slice, freq);
        unsafe { start_voice0(st.io_base, st.dma_phys, eso, delta) };
        let first = unsafe { read_voice0_cso(st.io_base) };
        if idx == 0 {
            cso_start = first;
        }
        let mut prev = first;
        // Play each note for ~180 ms while sampling CSO progression
        for _ in 0..90u32 {
            unsafe { mdelay(2) };
            let cur = unsafe { read_voice0_cso(st.io_base) };
            if cur != prev {
                cso_changes = cso_changes.wrapping_add(1);
                prev = cur;
            }
            cso_end = cur;
        }
        unsafe { stop_voice0(st.io_base) };
        unsafe { mdelay(20) };
    }
    st.active = false;

    if out_ptr != 0 {
        unsafe {
            core::ptr::write_unaligned(
                out_ptr as *mut AudioInfo,
                snapshot_info(st, cso_start, cso_end, cso_changes),
            );
        }
    }
    if cso_changes > 0 { 0 } else { 2 }
}

/// `AX=0x0022`: Read current ALi M5451 audio status without resetting hardware.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_audio_status(out_ptr: u32) -> u32 {
    let st = unsafe { &*AUDIO.0.get() };
    let (v1, v2, v3) = if st.sb_streaming && st.diag_gc_sts != 0 {
        (st.diag_aint, st.diag_misc, st.diag_gc_sts)
    } else if st.initialized && st.io_base != 0 {
        let cso = unsafe { read_voice0_cso(st.io_base) };
        (cso, cso, st.irq11_count)
    } else {
        (0, 0, 0)
    };
    if out_ptr != 0 {
        unsafe {
            core::ptr::write_unaligned(
                out_ptr as *mut AudioInfo,
                snapshot_info(st, v1, v2, v3),
            );
        }
    }
    if st.initialized { 0 } else { 1 }
}

/// `AX=0x0024`: Start continuous 22,050 Hz S16 stereo Sound Blaster Pro + AdLib
/// OPL2 streaming on ALi M5451 Voice 0 using a resident 16 KiB DOS DMA buffer.
/// Routes M5451 onto shared PCI IRQ 11 (86.1 Hz / 11.6 ms half-buffer interrupts)
/// with automatic fallback to 18.2 Hz timer polling if IRQ 11 does not fire.
/// Returns `0` on success, non-zero if hardware init failed.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_audio_sb_start(dma_seg: u32, out_ptr: u32) -> u32 {
    // Refuse a second start while a stream is live: re-running the
    // bring-up underneath a running stream would re-point the voice at
    // a new ring while the old one is still being rendered into.
    // AUDPROBE /SB is refused at the tool level as well; this is the
    // same guard for any other caller. A stop (gate1_audio_shutdown)
    // clears sb_streaming, so stop/start cycles are unaffected.
    {
        let st = unsafe { &*AUDIO.0.get() };
        if st.sb_streaming {
            return 5;
        }
    }
    let rc = gate1_audio_init(dma_seg, 0, out_ptr);
    if rc != 0 {
        return rc;
    }
    let st = unsafe { &mut *AUDIO.0.get() };
    if st.dma_lin == 0 || st.dma_phys == 0 || st.io_base == 0 {
        return 3;
    }
    // Zero Voice 0 ring buffer (0..AUDIO_RING_BYTES) and populate the built-in
    // 4 KiB GM wavetable ROM at 0x3000..0x3FFF for hardware MIDI voices 1..=31.
    let ring = unsafe { core::slice::from_raw_parts_mut(st.dma_lin as *mut u8, AUDIO_RING_BYTES) };
    for b in ring.iter_mut() {
        *b = 0;
    }
    unsafe { populate_wavetable_rom(st.dma_lin) };
    crate::sb_opl::reset_all();

    route_audio_irq(st);

    let delta = convert_playback_rate(crate::sb_opl::MIX_RATE);
    // Start in 86.1 Hz hardware IRQ 11 mode (512-frame / 23.2 ms ring, 256 frames
    // per half-buffer = 11.61 ms). In V86 DOS, JEMM's _irq11_hookproc services it;
    // in PM32 (HDPMI32i + DOOM), HDPMI32i reflects IRQ 11 directly to AUDPROBE's
    // real-mode IVT 0x73 handler (AX=0x0029), avoiding the BIOS default stub that
    // previously masked IRQ 11 on 0xA1. Falls back to 18Hz-POLL if IRQ 11 is absent.
    st.ring_frames = SB_IRQ_RING_FRAMES;
    st.irq11_count = 0;
    st.irq11_probe_ticks = 0;
    unsafe {
        io_outl(st.io_base + ALI_AINT, 0xFFFF_FFFF);
        io_outl(st.io_base + ALI_MISCINT, ALI_MISCINT_CLEAR);
        let gc = io_inl(st.io_base + ALI_GC_CIR);
        io_outl(st.io_base + ALI_GC_CIR, gc | ALI_GC_CIR_IE);
        io_outl(st.io_base + ALI_AINTEN, 1);
        start_voice0(st.io_base, st.dma_phys, (SB_IRQ_RING_FRAMES - 1) as u16, delta);
    }
    st.active = true;
    st.sb_streaming = true;
    st.write_frame = SB_IRQ_HALF_FRAMES;
    st.last_fill_start = usize::MAX;
    if out_ptr != 0 {
        let cso = unsafe { read_voice0_cso(st.io_base) };
        unsafe { core::ptr::write_unaligned(out_ptr as *mut AudioInfo, snapshot_info(st, cso, cso, 0)) };
    }
    0
}

/// Called from `_irq11_hookproc` (Ring-0 shared IRQ 11 ISR) at 86.1 Hz (11.61 ms).
/// Returns `0` if ALi M5451 did not assert IRQ 11 (chain to 82550 Ethernet), or
/// `1 | ((sb_irq_count as u32) << 8)` when M5451 owned and serviced the interrupt.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_audio_irq11() -> u32 {
    let st = unsafe { &mut *AUDIO.0.get() };
    if !st.sb_streaming || !st.initialized || st.dma_lin == 0 || st.io_base == 0 {
        return 0;
    }
    let misc = unsafe { io_inl(st.io_base + ALI_MISCINT) };
    if misc == 0 {
        return 0;
    }
    // Any nonzero MISCINT is this chip asserting, and the sticky
    // TARGET_REACHED / MIXER_OVERFLOW / MIXER_UNDERFLOW bits in it keep the
    // level-triggered IRQ 11 line asserted until they are written off --
    // even when the event is not a Voice 0 address interrupt (a mixer
    // underflow from a stalled ring is the realistic case, and a stalled
    // voice produces no further address interrupts to heal it). The Linux
    // reference handler therefore clears the mask on every nonzero
    // MISCINT and claims the interrupt; returning "not ours" without
    // clearing, as this code used to, leaves a permanent IRQ storm on a
    // line shared with the 82550 and the AR2413.
    let aint = unsafe { io_inl(st.io_base + ALI_AINT) };
    let voice_event = (misc & ALI_MISCINT_ADDR_IRQ) != 0 && (aint & 1) != 0;
    unsafe {
        if voice_event {
            io_outl(st.io_base + ALI_AINT, 1);
        }
        io_outl(st.io_base + ALI_MISCINT, ALI_MISCINT_CLEAR);
    }
    if !voice_event {
        return 1;
    }
    if st.ring_frames != SB_IRQ_RING_FRAMES {
        // The timer-poll fallback owns the voice and its 4096-frame ring.
        // The interrupt is acknowledged above, but rendering the 256-frame
        // IRQ halves into the big ring would fight the poll path over the
        // shared write position, and bumping irq11_count would make the
        // poll stand down under the mistaken belief that the hardware
        // path took over -- leaving nobody rendering at all.
        return 1;
    }
    st.irq11_count = st.irq11_count.wrapping_add(1);

    // Refill the half-buffer that Voice 0 JUST FINISHED playing.
    // M5451 fires MIDLP_IE at CSO == 255 and ENDLP_IE at CSO == 511 (1 sample
    // BEFORE the 256 / 0 wrap!), so testing raw `cso < 256` was dead-on the
    // boundary and refilled the wrong half right under the playhead. Shifting
    // by +128 frames (half a half-buffer) gives a full ±5.8 ms guard band:
    //   cso in [511, 0..127]  -> shifted in [127..255] (< 256) -> playing 0..255, refill 256..511
    //   cso in [255, 256..383]-> shifted in [383..511] (>= 256)-> playing 256..511, refill 0..255
    let cso = (unsafe { read_voice0_cso(st.io_base) } as usize) % SB_IRQ_RING_FRAMES;
    let shifted = (cso + (SB_IRQ_HALF_FRAMES / 2)) % SB_IRQ_RING_FRAMES;
    let fill_start = if shifted < SB_IRQ_HALF_FRAMES {
        SB_IRQ_HALF_FRAMES
    } else {
        0
    };
    // Guard against a spurious/late double-IRQ landing in the same half-buffer
    // twice in a row (which would overwrite the half we just rendered and skip
    // the next half, causing a brief stutter/click).
    if fill_start != st.last_fill_start {
        st.last_fill_start = fill_start;
        let ring = unsafe { core::slice::from_raw_parts_mut(st.dma_lin as *mut u8, SB_IRQ_RING_BYTES) };
        let (new_wf, _) = crate::sb_opl::render_into_ring(ring, fill_start, SB_IRQ_HALF_FRAMES);
        st.write_frame = new_wf;
    }

    let sb_irqs = if crate::sb_opl::peek_irq_pending() { 1u32 } else { 0u32 };
    1 | (sb_irqs << 8)
}

/// Lightweight high-frequency pump called from `gate1_sb_io` (e.g. DOOM's
/// 140 Hz OPL2 writes and 35 Hz `IN 0x03` DMA polls). Only renders when M5451
/// `cso` has consumed at least 128 frames (~5.8 ms), keeping `dma_pos_fp` and
/// `IN 0x03` smoothly advancing in real time without overhead on burst writes.
#[allow(dead_code)]
pub fn pump_if_needed() {
    let st = unsafe { &mut *AUDIO.0.get() };
    if !st.sb_streaming || !st.initialized || st.dma_lin == 0 || st.io_base == 0 {
        return;
    }
    if st.ring_frames != AUDIO_DMA_FRAMES as usize {
        return;
    }
    let ring_frames = AUDIO_DMA_FRAMES as usize;
    let cso = (unsafe { read_voice0_cso(st.io_base) } as usize) % ring_frames;
    let mut lead = (st.write_frame + ring_frames - cso) % ring_frames;
    if lead > 3200 {
        st.write_frame = (cso + 800) % ring_frames;
        lead = 800;
    }
    let target_lead = 1800usize;
    if lead + 128 <= target_lead {
        let to_fill = (target_lead - lead).min(512);
        let ring = unsafe { core::slice::from_raw_parts_mut(st.dma_lin as *mut u8, AUDIO_RING_BYTES) };
        let (new_wf, _) = crate::sb_opl::render_into_ring(ring, st.write_frame, to_fill);
        st.write_frame = new_wf;
    }
}

/// Called from `_timer_hookproc` (18.2 Hz) as an automatic fallback when M5451
/// hardware IRQ 11 is not firing. Returns `1` if SB IRQ 5 is pending, else `0`.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_audio_poll() -> u32 {
    let st = unsafe { &mut *AUDIO.0.get() };
    if !st.sb_streaming || !st.initialized || st.dma_lin == 0 || st.io_base == 0 {
        return 0;
    }
    // The hardware path owns rendering only while the voice sits on the
    // small IRQ ring. Once the fallback below has switched rings, this
    // poll is the only renderer left, even if a late IRQ 11 got serviced
    // in between -- standing down here on irq11_count alone would mute
    // the stream for the rest of the session.
    if st.ring_frames == SB_IRQ_RING_FRAMES && st.irq11_count > 0 {
        // Hardware IRQ 11 owns rendering, but still let _timer_hookproc
        // deliver any pending SB IRQ 5 if the game is idle on I/O ports
        // (e.g. waiting for a cmd=0x14 IRQ probe).
        return if crate::sb_opl::peek_irq_pending() { 1 } else { 0 };
    }
    // Probe window: give hardware IRQ 11 about a second of ticks to fire
    // before falling back. The fallback's bursty 18.2 Hz block-IRQ
    // delivery makes games refill DMA blocks the emulator has already
    // consumed, which is heard as echoing, repeating effects -- so the
    // whole session should only be condemned to it when the first
    // interrupt is truly absent, not merely late. Two ticks (~110 ms)
    // was short enough that a slow first interrupt (cold codec, shared
    // line busy at stream start) tripped it.
    if st.ring_frames == SB_IRQ_RING_FRAMES {
        st.irq11_probe_ticks = st.irq11_probe_ticks.saturating_add(1);
        if st.irq11_probe_ticks < 18 {
            return 0;
        }
        // Fallback: switch Voice 0 to 4096-frame (186 ms) buffer for 18.2 Hz polling.
        // AINTEN and the ENDLP/MIDLP enables are masked first so a late
        // IRQ 11 cannot re-enter the render path against the big ring;
        // gate1_audio_irq11 also refuses to render outside IRQ-ring mode.
        let delta = convert_playback_rate(crate::sb_opl::MIX_RATE);
        unsafe {
            st.diag_aint = (io_inl(st.io_base + ALI_AINT) & 0xFFFF) as u16;
            st.diag_misc = (io_inl(st.io_base + ALI_MISCINT) & 0xFFFF) as u16;
            io_outl(st.io_base + ALI_AINTEN, 0);
            let gc = io_inl(st.io_base + ALI_GC_CIR);
            io_outl(st.io_base + ALI_GC_CIR, gc & !ALI_GC_CIR_IE);
            start_voice0(st.io_base, st.dma_phys, AUDIO_DMA_FRAMES - 1, delta);
        }
        st.ring_frames = AUDIO_DMA_FRAMES as usize;
        st.write_frame = 1800;
    }

    let ring_frames = AUDIO_DMA_FRAMES as usize;
    let cso = (unsafe { read_voice0_cso(st.io_base) } as usize) % ring_frames;
    let mut lead = (st.write_frame + ring_frames - cso) % ring_frames;
    if lead > 3200 {
        st.write_frame = (cso + 800) % ring_frames;
        lead = 800;
    }
    let target_lead = 1800usize;
    if lead < target_lead {
        let to_fill = (target_lead - lead).min(1536);
        let ring = unsafe { core::slice::from_raw_parts_mut(st.dma_lin as *mut u8, AUDIO_RING_BYTES) };
        let (new_wf, rendered, irq_fired) =
            crate::sb_opl::render_into_ring_until_irq(ring, st.write_frame, to_fill);
        st.write_frame = new_wf;
        let more = rendered < to_fill;
        return match (more, irq_fired) {
            (false, false) => 0,
            (false, true) => 1,
            (true, true) => 2,  // deliver INT 0Dh NOW, then call again for next block!
            (true, false) => 3,
        };
    }
    if crate::sb_opl::peek_irq_pending() { 1 } else { 0 }
}

/// `AX=0x0025`: Set live AC'97 speaker/headphone volume (`0` = mute, `1..=9` = quiet..loud).
/// Writes `AC97_MASTER (0x02)`, `AC97_HEADPHONE (0x04)`, and `AC97_MASTER_MONO (0x06)`
/// without resetting the controller or interrupting active playback.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_audio_set_vol(level: u32) -> u32 {
    let st = unsafe { &mut *AUDIO.0.get() };
    if !st.initialized || st.io_base == 0 {
        return 1;
    }
    let lvl = level.min(9);
    st.ac97_vol = lvl as u8;
    let (stereo_val, mono_val) = if lvl == 0 {
        (0x8000u16, 0x8000u16)
    } else {
        // 9 -> 0 dB (0x00), 7 -> -9 dB (0x06), 5 -> -18 dB (0x0C), 1 -> -36 dB (0x18)
        let atten = ((9 - lvl) * 3) as u16;
        ((atten << 8) | atten, atten)
    };
    unsafe {
        ac97_write(st.io_base, st.rev, AC97_MASTER, stereo_val);
        ac97_write(st.io_base, st.rev, AC97_HEADPHONE, stereo_val);
        ac97_write(st.io_base, st.rev, AC97_MASTER_MONO, mono_val);
    }
    0
}

/// Read current AC'97 volume level (`0..=9`).
pub fn get_ac97_vol() -> u8 {
    let st = unsafe { &*AUDIO.0.get() };
    st.ac97_vol
}

/// `AX=0x0023` (and called on `DllMain(DLL_PROCESS_DETACH)`): Stop all M5451 voices
/// and release DMA lock cleanly.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_audio_shutdown() -> u32 {
    let st = unsafe { &mut *AUDIO.0.get() };
    st.sb_streaming = false;
    if st.initialized && st.io_base != 0 {
        unsafe {
            io_outl(st.io_base + ALI_STOP, 0xFFFF_FFFF);
            io_outl(st.io_base + ALI_AINTEN, 0x0000_0000);
            io_outl(st.io_base + ALI_AINT, 0xFFFF_FFFF);
            // Mute Master & PCM cleanly on unload
            ac97_write(st.io_base, st.rev, AC97_MASTER, 0x8000);
            ac97_write(st.io_base, st.rev, AC97_PCM, 0x8000);
        }
    }
    if st.dma_lin != 0 {
        unsafe { crate::vdmad_unlock_dma_region(st.dma_lin, AUDIO_DMA_SIZE as u32) };
        st.dma_lin = 0;
        st.dma_phys = 0;
    }
    st.active = false;
    st.initialized = false;
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate_conversion_matches_linux_ali5451() {
        assert_eq!(convert_playback_rate(48000), 0x1000);
        assert_eq!(convert_playback_rate(44100), 0x0EB3);
        assert_eq!(convert_playback_rate(8000), 0x02AB);
        assert_eq!(convert_playback_rate(22050), (((22050u32 << 12) + 22050) / 48000) as u16);
        assert_eq!(convert_playback_rate(1000), (((4000u32 << 12) + 4000) / 48000) as u16);
        assert_eq!(convert_playback_rate(96000), 0x1000);
    }

    #[test]
    fn ac97_command_packing_rev1_and_rev2() {
        // Rev 0x01 primary write to AC97_MASTER (0x02) = 0x0606
        assert_eq!(pack_ac97_write(0x02, 0x0606, false, 0x01), 0x0606_8002);
        // Rev 0x02 sets bit 8 (0x0100)
        assert_eq!(pack_ac97_write(0x02, 0x0606, false, 0x02), 0x0606_8102);
        // Primary read of AC97_POWERDOWN (0x26)
        assert_eq!(pack_ac97_read(0x26, false), 0x0000_8026);
        // Secondary codec sets bit 7 (0x0080)
        assert_eq!(pack_ac97_read(0x26, true), 0x0000_80A6);
    }

    #[test]
    fn voice_register_packing_matches_ali5451() {
        let regs = pack_voice_regs(
            0x0004_0000,
            0,
            4095,
            0x1000,
            1,
            0,
            0,
            VOICE_CTRL_S16_STEREO_LOOP,
            0,
        );
        assert_eq!(regs[0], 0x0000_0000);
        assert_eq!(regs[1], 0x0004_0000);
        assert_eq!(regs[2], (4095u32 << 16) | 0x1000);
        assert_eq!(regs[3], (1u32 << 31) | (0x0Fu32 << 12));
    }

    #[test]
    fn sine_generator_produces_bounded_stereo_wave() {
        let mut buf = [0u8; 4096];
        fill_sine_stereo_48k(&mut buf, 440);
        let mut min_v = i16::MAX;
        let mut max_v = i16::MIN;
        for chunk in buf.chunks_exact(4) {
            let l = i16::from_le_bytes([chunk[0], chunk[1]]);
            let r = i16::from_le_bytes([chunk[2], chunk[3]]);
            assert_eq!(l, r);
            if l < min_v { min_v = l; }
            if l > max_v { max_v = l; }
        }
        assert!(max_v >= 11500 && max_v <= 12050);
        assert!(min_v <= -11500 && min_v >= -12050);
    }
}
