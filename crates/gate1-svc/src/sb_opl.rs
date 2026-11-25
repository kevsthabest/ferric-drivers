//! Gate 4a Phase 2 — Resident Sound Blaster Pro (`A220 I5 D1`) + AdLib OPL2 (`0x388`)
//! Emulator (Ring-0 Rust inside `FERRIC.JLM`).
//!
//! Trapped via JEMM386 `Install_IO_Handler` on ports:
//!   - `0x220..0x22F` (Sound Blaster Pro FM, Mixer, DSP)
//!   - `0x388..0x389` (AdLib / OPL2 YM3812)
//!
//! Renders mixed 16-bit signed stereo PCM at `22,050 Hz` directly into the
//! ALi M5451 Voice 0 hardware DMA ring buffer (`ali5451.rs`).
//! Strictly `#![no_std]`, 386-pure (integer-only, zero FPU/MMX/SSE/486+),
//! read-only with respect to storage.

use core::cell::UnsafeCell;

#[cfg(target_arch = "x86")]
use core::arch::asm;

use crate::ali5451::sin_i16;

/// Output sample rate fed into ALi M5451 Voice 0 (hardware SRC upsamples to 48 kHz).
pub const MIX_RATE: u32 = 22050;

/// JEMM386 I/O callback `ecx` flag bit 2: `0 = IN`, `1 = OUT` (`JLM.INC:193`).
pub const IO_FLAG_OUTPUT: u32 = 0x04;

// ---------------------------------------------------------------------
// Pure integer OPL2 (YM3812) 9-channel 2-operator FM synthesizer
// ---------------------------------------------------------------------

/// OPL2 operator slot pairs `(modulator, carrier)` for channels 0..8.
const OPL_CH_OPS: [(usize, usize); 9] = [
    (0, 3),
    (1, 4),
    (2, 5),
    (8, 11),
    (9, 12),
    (10, 13),
    (16, 19),
    (17, 20),
    (18, 21),
];

/// OPL2 frequency multiplier * 2 (`0x20+op` bits 3:0).
const OPL_MULT_X2: [u32; 16] = [1, 2, 4, 6, 8, 10, 12, 14, 16, 18, 20, 20, 24, 24, 30, 30];

/// 64-entry exponential attenuation table (0.75 dB/step, 0..=47.25 dB) scaled 0..=256.
const ATTEN_LUT: [u16; 64] = [
    256, 235, 215, 197, 181, 166, 152, 140,
    128, 117, 108, 99, 91, 83, 76, 70,
    64, 59, 54, 49, 45, 41, 38, 35,
    32, 29, 27, 25, 23, 21, 19, 17,
    16, 15, 13, 12, 11, 10, 9, 9,
    8, 7, 7, 6, 6, 5, 5, 4,
    4, 4, 3, 3, 3, 3, 2, 2,
    2, 2, 2, 2, 1, 1, 1, 0,
];

/// YM3812 Key Scale Level (KSL) base table indexed by `fnum >> 6` (0..15),
/// scaled in 0.75 dB attenuation units (matching `ATTEN_LUT`).
const OPL_KSL_BASE: [u8; 16] = [
    0, 32, 40, 45, 48, 51, 53, 55,
    56, 58, 59, 60, 61, 62, 63, 64,
];

/// YM3812 8-step vibrato (PM) waveform multipliers (signed, in units of fnum>>7).
const OPL_PM_WAVE: [i32; 8] = [0, 1, 2, 1, 0, -1, -2, -1];

/// Envelope stages.
const ENV_OFF: u8 = 0;
const ENV_ATTACK: u8 = 1;
const ENV_DECAY: u8 = 2;
const ENV_SUSTAIN: u8 = 3;
const ENV_RELEASE: u8 = 4;

#[derive(Copy, Clone)]
struct OplOp {
    phase: u32,     // 24-bit phase accumulator (top 8 bits index sine)
    env_lvl: u32,   // 0..=0x3F0000 (0 = loudest, 0x3F0000 = silent, top 6 bits index ATTEN_LUT)
    env_state: u8,
    prev_out: i16,
    prev_out2: i16,
}

impl OplOp {
    const fn zero() -> Self {
        Self {
            phase: 0,
            env_lvl: 0x3F0000,
            env_state: ENV_OFF,
            prev_out: 0,
            prev_out2: 0,
        }
    }
}

#[derive(Copy, Clone)]
struct Opl2 {
    addr: u8,
    regs: [u8; 256],
    status: u8,
    timer_ctrl: u8,
    t1_polls: u8,
    t2_polls: u8,
    key_on: [bool; 9],
    rhythm_bits: u8,   // previous 0xBD bits 4:0 for edge-triggered percussion
    am_phase: u32,     // 3.7 Hz LFO AM accumulator
    pm_phase: u32,     // 6.1 Hz LFO PM accumulator
    noise_lfsr: u32,   // 23-bit YM3812 percussion noise generator
    ops: [OplOp; 22],
}

impl Opl2 {
    const fn new() -> Self {
        Self {
            addr: 0,
            regs: [0; 256],
            status: 0,
            timer_ctrl: 0,
            t1_polls: 0,
            t2_polls: 0,
            key_on: [false; 9],
            rhythm_bits: 0,
            am_phase: 0,
            pm_phase: 0,
            noise_lfsr: 1,
            ops: [OplOp::zero(); 22],
        }
    }

    fn reset(&mut self) {
        *self = Self::new();
    }

    #[inline]
    fn key_op_on(&mut self, op_idx: usize) {
        self.ops[op_idx].phase = 0;
        self.ops[op_idx].env_state = ENV_ATTACK;
    }

    #[inline]
    fn key_op_off(&mut self, op_idx: usize) {
        if self.ops[op_idx].env_state != ENV_OFF {
            self.ops[op_idx].env_state = ENV_RELEASE;
        }
    }

    /// Compute YM3812 KSL attenuation in 0.75 dB units (0..=63) for an operator.
    #[inline]
    fn calc_ksl_atten(r40: u8, block: u32, fnum: u32) -> usize {
        let ksl_bits = (r40 >> 6) & 0x03;
        if ksl_bits == 0 {
            return 0;
        }
        let base = OPL_KSL_BASE[((fnum >> 6) & 0x0F) as usize] as i32;
        let raw = base - ((7 - (block as i32)) * 8);
        if raw <= 0 {
            return 0;
        }
        // YM3812 KSL bits: 01b = 3.0 dB/oct (>>1), 10b = 1.5 dB/oct (>>2), 11b = 6.0 dB/oct (>>0).
        let shift = match ksl_bits {
            1 => 1,
            2 => 2,
            _ => 0,
        };
        (raw >> shift) as usize
    }

    fn read_status(&mut self) -> u8 {
        // Emulate OPL2 Timer 1 / Timer 2 expiration so AdLib detection passes.
        if (self.timer_ctrl & 0x01) != 0 && (self.timer_ctrl & 0x40) == 0 {
            self.t1_polls = self.t1_polls.saturating_add(1);
            if self.t1_polls >= 1 {
                self.status |= 0xC0; // IRQ + Timer 1 expired
            }
        }
        if (self.timer_ctrl & 0x02) != 0 && (self.timer_ctrl & 0x20) == 0 {
            self.t2_polls = self.t2_polls.saturating_add(1);
            if self.t2_polls >= 1 {
                self.status |= 0xA0; // IRQ + Timer 2 expired
            }
        }
        self.status
    }

    fn write_addr(&mut self, val: u8) {
        self.addr = val;
    }

    fn write_data(&mut self, val: u8) {
        let reg = self.addr;
        self.regs[reg as usize] = val;

        if reg == 0x04 {
            if (val & 0x80) != 0 {
                self.status = 0;
                self.t1_polls = 0;
                self.t2_polls = 0;
            } else {
                self.timer_ctrl = val;
                if (val & 0x01) == 0 {
                    self.t1_polls = 0;
                }
                if (val & 0x02) == 0 {
                    self.t2_polls = 0;
                }
            }
            return;
        }

        // Rhythm / Percussion control register (0xBD)
        if reg == 0xBD {
            if (val & 0x20) != 0 {
                let prev = self.rhythm_bits;
                let cur = val & 0x1F;
                self.rhythm_bits = cur;
                // Bit 4: Bass Drum (ch 6 ops 16 & 19)
                if (cur & 0x10) != 0 && (prev & 0x10) == 0 {
                    self.key_op_on(16);
                    self.key_op_on(19);
                } else if (cur & 0x10) == 0 && (prev & 0x10) != 0 {
                    self.key_op_off(16);
                    self.key_op_off(19);
                }
                // Bit 3: Snare Drum (op 20)
                if (cur & 0x08) != 0 && (prev & 0x08) == 0 {
                    self.key_op_on(20);
                } else if (cur & 0x08) == 0 && (prev & 0x08) != 0 {
                    self.key_op_off(20);
                }
                // Bit 2: Tom-Tom (op 18)
                if (cur & 0x04) != 0 && (prev & 0x04) == 0 {
                    self.key_op_on(18);
                } else if (cur & 0x04) == 0 && (prev & 0x04) != 0 {
                    self.key_op_off(18);
                }
                // Bit 1: Top Cymbal (op 21)
                if (cur & 0x02) != 0 && (prev & 0x02) == 0 {
                    self.key_op_on(21);
                } else if (cur & 0x02) == 0 && (prev & 0x02) != 0 {
                    self.key_op_off(21);
                }
                // Bit 0: Hi-Hat (op 17)
                if (cur & 0x01) != 0 && (prev & 0x01) == 0 {
                    self.key_op_on(17);
                } else if (cur & 0x01) == 0 && (prev & 0x01) != 0 {
                    self.key_op_off(17);
                }
            } else {
                self.rhythm_bits = 0;
            }
            return;
        }

        // Key-on / key-off on 0xB0..0xB8
        if (0xB0..=0xB8).contains(&reg) {
            let ch = (reg - 0xB0) as usize;
            let on = (val & 0x20) != 0;
            let was = self.key_on[ch];
            self.key_on[ch] = on;
            let (m, c) = OPL_CH_OPS[ch];
            if on && !was {
                self.key_op_on(m);
                self.key_op_on(c);
            } else if !on && was {
                self.key_op_off(m);
                self.key_op_off(c);
            }
        }
    }

    #[inline]
    fn eval_wave(phase8: u8, ws: u8) -> i16 {
        let s = sin_i16(phase8);
        match ws & 7 {
            0 => s,                                  // full sine
            1 => if phase8 < 128 { s } else { 0 },   // half sine
            2 => if s < 0 { -s } else { s },         // abs sine
            3 => if (phase8 & 0x40) == 0 { if s < 0 { -s } else { s } } else { 0 }, // quarter sine
            4 => if phase8 < 128 { sin_i16(phase8.wrapping_mul(2)) } else { 0 }, // OPL3 alternating sine
            5 => if phase8 < 128 { let v = sin_i16(phase8.wrapping_mul(2)); if v < 0 { -v } else { v } } else { 0 }, // OPL3 camel sine
            6 => if phase8 < 128 { 12000 } else { -12000 }, // OPL3 square
            _ => {                                   // OPL3 derived sawtooth
                if phase8 < 128 {
                    ((127 - phase8 as i32) * 94) as i16
                } else {
                    -(((phase8 as i32 - 128) * 94) as i16)
                }
            }
        }
    }

    #[inline]
    fn step_op(
        &mut self,
        op_idx: usize,
        base_step: u32,
        mod_in: i32,
        use_fb: u8,
        block: u32,
        fnum: u32,
        am_atten: usize,
        pm_delta: i32,
    ) -> i16 {
        let r20 = self.regs[0x20 + op_idx];
        let r40 = self.regs[0x40 + op_idx];
        let r60 = self.regs[0x60 + op_idx];
        let r80 = self.regs[0x80 + op_idx];
        let re0 = self.regs[0xE0 + op_idx];

        let mult_x2 = OPL_MULT_X2[(r20 & 0x0F) as usize];
        let tl = (r40 & 0x3F) as usize;
        let ksl = Self::calc_ksl_atten(r40, block, fnum);
        let ksr_boost = if (r20 & 0x10) != 0 {
            ((block << 1) | ((fnum >> 9) & 1)) >> 2
        } else {
            0
        };
        let ar_raw = (r60 >> 4) as u32;
        let dr_raw = (r60 & 0x0F) as u32;
        let rr_raw = (r80 & 0x0F) as u32;
        let ar = if ar_raw > 0 { (ar_raw + ksr_boost).min(15) } else { 0 };
        let dr = if dr_raw > 0 { (dr_raw + ksr_boost).min(15) } else { 0 };
        let sl = ((r80 >> 4) as u32) << 18; // sustain level in 0..0x3C0000
        let rr = if rr_raw > 0 { (rr_raw + ksr_boost).min(15) } else { 0 };
        let eg_typ = (r20 & 0x20) != 0; // 1 = sustaining
        let use_am = (r20 & 0x80) != 0;
        let use_pm = (r20 & 0x40) != 0;

        let op = &mut self.ops[op_idx];
        if op.env_state == ENV_OFF {
            return 0;
        }

        // Advance ADSR envelope
        match op.env_state {
            ENV_ATTACK => {
                if ar >= 15 {
                    op.env_lvl = 0;
                    op.env_state = ENV_DECAY;
                } else if ar > 0 {
                    let rate = 1u32 << (ar + 3);
                    if op.env_lvl <= rate {
                        op.env_lvl = 0;
                        op.env_state = ENV_DECAY;
                    } else {
                        op.env_lvl -= rate;
                    }
                }
            }
            ENV_DECAY => {
                if dr > 0 {
                    let rate = 1u32 << (dr + 1);
                    op.env_lvl = op.env_lvl.saturating_add(rate);
                }
                if op.env_lvl >= sl {
                    op.env_lvl = sl;
                    op.env_state = if eg_typ { ENV_SUSTAIN } else { ENV_RELEASE };
                }
            }
            ENV_SUSTAIN => {}
            ENV_RELEASE => {
                let rate = if rr > 0 { 1u32 << (rr + 1) } else { 16 };
                op.env_lvl = op.env_lvl.saturating_add(rate);
                if op.env_lvl >= 0x3F0000 {
                    op.env_lvl = 0x3F0000;
                    op.env_state = ENV_OFF;
                    return 0;
                }
            }
            _ => return 0,
        }

        // Advance 24-bit phase (with optional 6.1 Hz vibrato PM)
        let eff_base = if use_pm && pm_delta != 0 {
            ((base_step as i32) + pm_delta).max(0) as u32
        } else {
            base_step
        };
        let step = (eff_base * mult_x2) >> 1;
        op.phase = op.phase.wrapping_add(step) & 0x00FF_FFFF;

        let fb_mod = if use_fb != 0 {
            // YM3812 feedback: fb=1..7 -> pi/16 .. 4*pi radians in 8-bit phase units
            // (256 = 2*pi, prev_out in ±32767 -> shift = 13 - fb; fb=7 -> ±512 = ±4*pi).
            let avg = (op.prev_out as i32 + op.prev_out2 as i32) >> 1;
            avg >> (13 - use_fb)
        } else {
            0
        };

        let p8 = (((op.phase >> 16) as i32 + mod_in + fb_mod) & 0xFF) as u8;
        let ws = if (re0 & 4) != 0 {
            re0 & 7
        } else if (self.regs[0x01] & 0x20) != 0 {
            re0 & 3
        } else {
            0
        };
        let raw = Self::eval_wave(p8, ws) as i32;

        let env_atten = (op.env_lvl >> 16) as usize;
        let am_add = if use_am { am_atten } else { 0 };
        let total_atten = (tl + ksl + env_atten + am_add).min(63);
        let gain = ATTEN_LUT[total_atten] as i32;
        let out = ((raw * gain) >> 8) as i16;

        op.prev_out2 = op.prev_out;
        op.prev_out = out;
        out
    }

    #[inline]
    fn step_noise_bit(&mut self) -> i32 {
        // 23-bit YM3812-style Galois LFSR (x^23 + x^18 + 1)
        let bit = self.noise_lfsr & 1;
        self.noise_lfsr >>= 1;
        if bit != 0 {
            self.noise_lfsr ^= 0x0040_0020;
        }
        if bit != 0 { 64 } else { -64 }
    }

    fn next_sample_stereo(&mut self, opl3_mode: bool) -> (i32, i32) {
        // Advance LFO AM (~3.7 Hz @ 22050 Hz -> 2^32 * 3.7 / 22050 ≈ 720_693)
        // and LFO PM (~6.1 Hz @ 22050 Hz -> 2^32 * 6.1 / 22050 ≈ 1_188_170).
        self.am_phase = self.am_phase.wrapping_add(720_693);
        self.pm_phase = self.pm_phase.wrapping_add(1_188_170);
        let am_tri = ((self.am_phase >> 24) & 0xFF) as usize;
        let am_tri = if am_tri < 128 { am_tri } else { 255 - am_tri }; // 0..127
        let rbd = self.regs[0xBD];
        // AM depth: bit 7 = 1 -> 4.8 dB (~6 steps of 0.75 dB), else 1.0 dB (~1 step)
        let am_atten = if (rbd & 0x80) != 0 {
            (am_tri * 6) >> 7
        } else {
            am_tri >> 6
        };
        let pm_idx = ((self.pm_phase >> 29) & 7) as usize;
        let pm_wave = OPL_PM_WAVE[pm_idx];
        let pm_deep = (rbd & 0x40) != 0;
        let rhythm = (rbd & 0x20) != 0;

        let melodic_ch = if rhythm { 6usize } else { 9usize };
        let mut sum_l = 0i32;
        let mut sum_r = 0i32;

        for ch in 0..melodic_ch {
            let (m, c) = OPL_CH_OPS[ch];
            if self.ops[m].env_state == ENV_OFF && self.ops[c].env_state == ENV_OFF {
                continue;
            }
            let a0 = self.regs[0xA0 + ch] as u32;
            let b0 = self.regs[0xB0 + ch] as u32;
            let c0 = self.regs[0xC0 + ch];
            let fnum = a0 | ((b0 & 0x03) << 8);
            let block = (b0 >> 2) & 0x07;
            // Exact 22050 Hz 24-bit phase step for mult=1 (mult_x2=2):
            // 49716 / 22050 * 16 = 36.075 -> (fnum << block) * 36 + ((fnum << block) >> 3)
            let fb_val = fnum << block;
            let base_step = fb_val * 36 + (fb_val >> 3);
            let pm_delta = if pm_wave != 0 {
                let shift = if pm_deep { 8 } else { 9 };
                ((base_step >> shift) as i32) * pm_wave
            } else {
                0
            };

            let fb = (c0 >> 1) & 0x07;
            let additive = (c0 & 0x01) != 0;

            let m_out = self.step_op(m, base_step, 0, fb, block, fnum, am_atten, pm_delta);
            let ch_out = if additive {
                let c_out = self.step_op(c, base_step, 0, 0, block, fnum, am_atten, pm_delta);
                (m_out as i32 + c_out as i32) >> 3
            } else {
                // Smooth phase modulation index scaled to 8-bit sine table (avoids FM aliasing)
                let mod_in = (m_out as i32) >> 7;
                let c_out = self.step_op(c, base_step, mod_in, 0, block, fnum, am_atten, pm_delta);
                (c_out as i32) >> 2
            };
            if opl3_mode {
                if (c0 & 0x10) != 0 {
                    sum_l += ch_out;
                }
                if (c0 & 0x20) != 0 {
                    sum_r += ch_out;
                }
            } else {
                sum_l += ch_out;
                sum_r += ch_out;
            }
        }

        if rhythm {
            let mut rsum = 0i32;
            let noise = self.step_noise_bit();
            // Ch 6: Bass Drum (ops 16 & 19)
            if self.ops[16].env_state != ENV_OFF || self.ops[19].env_state != ENV_OFF {
                let fnum = (self.regs[0xA6] as u32) | (((self.regs[0xB6] & 0x03) as u32) << 8);
                let block = ((self.regs[0xB6] >> 2) & 0x07) as u32;
                let fb_val = fnum << block;
                let base_step = fb_val * 36 + (fb_val >> 3);
                let c0 = self.regs[0xC6];
                let m_out = self.step_op(16, base_step, 0, (c0 >> 1) & 7, block, fnum, am_atten, 0);
                let c_out = if (c0 & 1) != 0 {
                    self.step_op(19, base_step, 0, 0, block, fnum, am_atten, 0)
                } else {
                    self.step_op(19, base_step, (m_out as i32) >> 7, 0, block, fnum, am_atten, 0)
                };
                rsum += (c_out as i32) >> 2;
            }
            // Ch 7: Hi-Hat (op 17) & Snare Drum (op 20)
            if self.ops[17].env_state != ENV_OFF || self.ops[20].env_state != ENV_OFF {
                let fnum = (self.regs[0xA7] as u32) | (((self.regs[0xB7] & 0x03) as u32) << 8);
                let block = ((self.regs[0xB7] >> 2) & 0x07) as u32;
                let fb_val = fnum << block;
                let base_step = fb_val * 36 + (fb_val >> 3);
                if self.ops[17].env_state != ENV_OFF {
                    let hh = self.step_op(17, base_step, noise, 0, block, fnum, am_atten, 0);
                    rsum += (hh as i32) >> 2;
                }
                if self.ops[20].env_state != ENV_OFF {
                    let sd = self.step_op(20, base_step, noise, 0, block, fnum, am_atten, 0);
                    rsum += (sd as i32) >> 2;
                }
            }
            // Ch 8: Tom-Tom (op 18) & Top Cymbal (op 21)
            if self.ops[18].env_state != ENV_OFF || self.ops[21].env_state != ENV_OFF {
                let fnum = (self.regs[0xA8] as u32) | (((self.regs[0xB8] & 0x03) as u32) << 8);
                let block = ((self.regs[0xB8] >> 2) & 0x07) as u32;
                let fb_val = fnum << block;
                let base_step = fb_val * 36 + (fb_val >> 3);
                if self.ops[18].env_state != ENV_OFF {
                    let tt = self.step_op(18, base_step, 0, 0, block, fnum, am_atten, 0);
                    rsum += (tt as i32) >> 2;
                }
                if self.ops[21].env_state != ENV_OFF {
                    let tc = self.step_op(21, base_step, noise, 0, block, fnum, am_atten, 0);
                    rsum += (tc as i32) >> 2;
                }
            }
            sum_l += rsum;
            sum_r += rsum;
        }
        (sum_l, sum_r)
    }
}

// ---------------------------------------------------------------------
// Hardware 8237 ISA DMA Channel 1 reader (Ring-0 direct port access)
// ---------------------------------------------------------------------

#[cfg(target_arch = "x86")]
#[inline]
unsafe fn inb_raw(port: u16) -> u8 {
    let v: u8;
    unsafe { asm!("in al, dx", out("al") v, in("dx") port, options(nomem, nostack, preserves_flags)) };
    v
}

#[cfg(not(target_arch = "x86"))]
#[inline]
unsafe fn inb_raw(_port: u16) -> u8 {
    0
}

#[cfg(target_arch = "x86")]
#[inline]
unsafe fn outb_raw(port: u16, val: u8) {
    unsafe { asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags)) };
}

#[cfg(not(target_arch = "x86"))]
#[inline]
unsafe fn outb_raw(_port: u16, _val: u8) {}

/// Read ISA DMA Channel 1 physical base address and byte count from the
/// 8237 DMA controller (programmed by the DOS game through JEMM386 VDMA).
unsafe fn read_isa_dma1() -> (u32, u32) {
    unsafe {
        outb_raw(0x0C, 0); // clear byte pointer flip-flop
        let lo = inb_raw(0x02) as u32;
        let hi = inb_raw(0x02) as u32;
        let page = inb_raw(0x83) as u32;
        outb_raw(0x0C, 0);
        let clo = inb_raw(0x03) as u32;
        let chi = inb_raw(0x03) as u32;
        let addr = (page << 16) | (hi << 8) | lo;
        let len = ((chi << 8) | clo) + 1;
        (addr, len)
    }
}

// ---------------------------------------------------------------------
// Sound Blaster Pro DSP + Mixer state machine
// ---------------------------------------------------------------------

/// DSP copyright string answered to command 0xE3, NUL-terminated as on
/// real hardware. At 45 bytes it sets the read FIFO size below.
const DSP_COPYRIGHT: &[u8] = b"COPYRIGHT (C) CREATIVE TECHNOLOGY LTD, 1992.\0";

struct SbDsp {
    reset_state: u8,
    read_fifo: [u8; 64],
    read_head: usize,
    read_len: usize,
    cmd: u8,
    cmd_expect: u8,
    cmd_args: [u8; 4],
    cmd_got: usize,
    speaker_on: bool,
    test_reg: u8,

    // Mixer
    mixer_addr: u8,
    mixer_regs: [u8; 64],

    // Shadowed ISA DMA Channel 1 registers (trapped via HDPMI32i for PM32 games
    // like DOOM; necessary because DMA page port 0x83 is write-only on ALi M1533).
    dma1_shadow_valid: bool,
    dma1_ff: u8,         // 0 = low byte, 1 = high byte (cleared by OUT 0x0C)
    dma1_base_lo: u8,
    dma1_base_hi: u8,
    dma1_page: u8,
    dma1_cnt_lo: u8,
    dma1_cnt_hi: u8,
    dma1_mode: u8,
    dma1_masked: bool,

    // PCM / DMA playback state
    time_const: u8,
    sample_rate: u32,
    block_len: u32,
    dma_active: bool,
    dma_paused: bool,
    dma_auto: bool,
    dma_phys: u32,
    dma_buf_len: u32,
    dma_pos_fp: u32,    // 16.16 fixed-point byte index within DMA buffer
    dma_rem_block: u32, // bytes remaining until next SB IRQ
    direct_dac: u8,     // last direct DAC byte (DSP cmd 0x10)
    direct_active: bool,
    irq_pending_count: u8,
    irq_count: u32,
    io_count: u32,
    opl_count: u32,
    ack22e_count: u32,
    last_dma_cmd: u8,
    dma_rd_count: u32,
    dma_rd3_count: u32,
    dma_wr_count: u32,
    peak_stereo: u8,
}

/// 32-byte Sound Blaster / OPL2 telemetry struct returned by `AX=0x0027` (`AUDPROBE /D`).
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct SbDiag {
    pub last_cmd: u8,     // +0x00
    pub dma_auto: u8,     // +0x01
    pub stereo: u8,       // +0x02
    pub active: u8,       // +0x03
    pub sample_rate: u32, // +0x04
    pub dma_phys: u32,    // +0x08
    pub dma_buf_len: u32, // +0x0C
    pub block_len: u32,   // +0x10
    pub io_count: u32,    // +0x14
    pub opl_count: u32,   // +0x18
    pub irq_count: u16,   // +0x1C
    pub ack22e_count: u16,// +0x1E
}

const _: () = {
    assert!(core::mem::size_of::<SbDiag>() == 32);
};

impl SbDsp {
    /// Power-on CT1345 mixer defaults: voice, master and FM volumes at
    /// 0xCC (nibble 12 of 15 per channel), everything else zero.
    const fn default_mixer_regs() -> [u8; 64] {
        let mut mixer = [0u8; 64];
        mixer[0x04] = 0xCC; // Voice vol
        mixer[0x22] = 0xCC; // Master vol
        mixer[0x26] = 0xCC; // FM vol
        mixer
    }

    /// Apply one CT1345 stereo volume register (left channel in the high
    /// nibble, right in the low nibble, 0..15 each) to a stereo pair.
    /// The power-on default nibble is 12, so dividing by 12 keeps a
    /// default-programmed mix at unity gain and makes the register a
    /// pure attenuator in normal use, exactly as games expect.
    fn apply_vol(reg: u8, l: i32, r: i32) -> (i32, i32) {
        let lg = ((reg >> 4) & 0x0F) as i32;
        let rg = (reg & 0x0F) as i32;
        ((l * lg) / 12, (r * rg) / 12)
    }

    const fn new() -> Self {
        Self {
            reset_state: 0,
            read_fifo: [0; 64],
            read_head: 0,
            read_len: 0,
            cmd: 0,
            cmd_expect: 0,
            cmd_args: [0; 4],
            cmd_got: 0,
            speaker_on: true,
            test_reg: 0,
            mixer_addr: 0,
            mixer_regs: Self::default_mixer_regs(),
            dma1_shadow_valid: false,
            dma1_ff: 0,
            dma1_base_lo: 0,
            dma1_base_hi: 0,
            dma1_page: 0,
            dma1_cnt_lo: 0,
            dma1_cnt_hi: 0,
            dma1_mode: 0,
            dma1_masked: false,
            time_const: 211, // ~22,222 Hz
            sample_rate: 22050,
            block_len: 0,
            dma_active: false,
            dma_paused: false,
            dma_auto: false,
            dma_phys: 0,
            dma_buf_len: 0,
            dma_pos_fp: 0,
            dma_rem_block: 0,
            direct_dac: 0x80,
            direct_active: false,
            irq_pending_count: 0,
            irq_count: 0,
            io_count: 0,
            opl_count: 0,
            ack22e_count: 0,
            last_dma_cmd: 0,
            dma_rd_count: 0,
            dma_rd3_count: 0,
            dma_wr_count: 0,
            peak_stereo: 0,
        }
    }

    fn reset(&mut self) {
        self.read_head = 0;
        self.read_len = 0;
        self.cmd = 0;
        self.cmd_expect = 0;
        self.cmd_got = 0;
        self.dma_active = false;
        self.dma_paused = false;
        self.dma_auto = false;
        self.dma1_shadow_valid = false;
        self.dma1_masked = false;
        self.dma1_ff = 0;
        self.irq_pending_count = 0;
        self.direct_dac = 0x80;
        self.direct_active = false;
        self.push_read(0xAA);
    }

    fn push_read(&mut self, b: u8) {
        if self.read_len < self.read_fifo.len() {
            let idx = (self.read_head + self.read_len) % self.read_fifo.len();
            self.read_fifo[idx] = b;
            self.read_len += 1;
        }
    }

    fn pop_read(&mut self) -> u8 {
        if self.read_len == 0 {
            return 0xAA;
        }
        let b = self.read_fifo[self.read_head];
        self.read_head = (self.read_head + 1) % self.read_fifo.len();
        self.read_len -= 1;
        b
    }

    fn is_stereo(&self) -> bool {
        (self.mixer_regs[0x0E] & 0x02) != 0
    }

    /// Recompute the effective DMA consumption rate from the time
    /// constant and the CURRENT mixer stereo bit. On real hardware the
    /// stereo division lives in the DAC clock path, so it applies
    /// whenever stereo is on, in either programming order; latching
    /// the rate against the stereo bit at the moment 0x40 was written
    /// made a game that sets its time constant before enabling stereo
    /// play its DMA at double speed until the next 0x40.
    fn refresh_sample_rate(&mut self) {
        let denom = (256u32 - (self.time_const as u32)).max(1);
        let mut rate = 1_000_000 / denom;
        if self.is_stereo() {
            rate /= 2;
        }
        self.sample_rate = rate.clamp(4000, 44100);
    }

    fn start_dma(&mut self, block_bytes: u32, auto_init: bool) {
        let (phys, dma_len) = if self.dma1_shadow_valid {
            let addr = ((self.dma1_page as u32) << 16)
                | ((self.dma1_base_hi as u32) << 8)
                | (self.dma1_base_lo as u32);
            let len = (((self.dma1_cnt_hi as u32) << 8) | (self.dma1_cnt_lo as u32)) + 1;
            (addr, len)
        } else {
            unsafe { read_isa_dma1() }
        };
        let valid_phys = phys != 0 && phys < 0x0010_0000;
        self.dma_phys = if valid_phys { phys } else { 0 };
        self.dma_buf_len = if dma_len > 0 { dma_len } else { block_bytes.max(1) };
        self.dma_pos_fp = 0;
        self.dma_rem_block = block_bytes.max(1);
        self.block_len = block_bytes.max(1);
        // Also honor 8237 DMA auto-init bit (mode bit 4 = 0x10) if shadowed
        self.dma_auto = auto_init || (self.dma1_shadow_valid && (self.dma1_mode & 0x10) != 0);
        self.dma_paused = false;
        self.last_dma_cmd = self.cmd;
        // Always activate for IRQ-probe transfers (block_bytes <= 16) even if phys==0
        self.dma_active = valid_phys || block_bytes <= 16;
        self.direct_active = false;
    }

    fn write_cmd_or_data(&mut self, val: u8) {
        if self.cmd_expect > 0 {
            if self.cmd_got < self.cmd_args.len() {
                self.cmd_args[self.cmd_got] = val;
            }
            self.cmd_got += 1;
            self.cmd_expect -= 1;
            if self.cmd_expect == 0 {
                self.finish_cmd();
            }
            return;
        }

        self.cmd = val;
        self.cmd_got = 0;
        match val {
            0x10 => self.cmd_expect = 1, // Direct 8-bit DAC
            0x14 => self.cmd_expect = 2, // Single-cycle 8-bit DMA
            0x1C => {
                // Auto-init 8-bit DMA using previously set block_len
                let len = self.block_len.max(1);
                self.start_dma(len, true);
            }
            0x40 => self.cmd_expect = 1, // Set Time Constant
            0x48 => self.cmd_expect = 2, // Set DMA Block Size
            0x90 => {
                // High-speed auto-init 8-bit DMA
                let len = self.block_len.max(1);
                self.start_dma(len, true);
            }
            0x91 => {
                // High-speed single-cycle 8-bit DMA
                let len = self.block_len.max(1);
                self.start_dma(len, false);
            }
            0xD0 => self.dma_paused = true,
            0xD1 => self.speaker_on = true,
            0xD3 => self.speaker_on = false,
            0xD4 => self.dma_paused = false,
            0xD8 => self.push_read(if self.speaker_on { 0xFF } else { 0x00 }),
            0xDA => self.dma_auto = false,
            0xE0 => self.cmd_expect = 1, // DSP ID
            0xE1 => {
                // DSP Version: Sound Blaster Pro 2 (3.02)
                self.push_read(0x03);
                self.push_read(0x02);
            }
            0xE2 => self.cmd_expect = 1, // DMA ID (ignore)
            0xE4 => self.cmd_expect = 1, // Write test register
            0xE8 => self.push_read(self.test_reg),
            0xF2 => {
                // Trigger 8-bit SB IRQ immediately (IRQ detection)
                self.irq_pending_count = self.irq_pending_count.saturating_add(1);
                self.irq_count = self.irq_count.wrapping_add(1);
            }
            // Commands below are not implemented (this DSP reports 3.02
            // and has no 16-bit DMA path), but they must still consume
            // their argument bytes: an unconsumed length or mode byte is
            // reparsed as a fresh command -- a stray 0xF2 raises a
            // phantom IRQ, a stray 0x10 turns the next byte into a DAC
            // sample -- and the command stream desyncs from there.
            0xB0..=0xBF | 0xC0..=0xCF => self.cmd_expect = 3, // SB16 16-bit DMA: mode + length lo/hi
            0x38 => self.cmd_expect = 1,                     // MIDI write: one data byte
            0xD5 => self.dma_paused = true,                  // SB16 pause 16-bit DMA (mirrors 0xD0)
            0xD6 => self.dma_paused = false,                 // SB16 continue 16-bit DMA (mirrors 0xD4)
            0xD9 => self.dma_auto = false,                   // SB16 exit auto-init (mirrors 0xDA)
            0xE3 => {
                // DSP copyright string, answered on the read port.
                for &c in DSP_COPYRIGHT.iter() {
                    self.push_read(c);
                }
            }
            _ => {}
        }
    }

    fn finish_cmd(&mut self) {
        match self.cmd {
            0x10 => {
                self.direct_dac = self.cmd_args[0];
                self.direct_active = true;
            }
            0x14 => {
                let len = ((self.cmd_args[1] as u32) << 8 | (self.cmd_args[0] as u32)) + 1;
                self.start_dma(len, false);
                if len <= 16 && !self.dma1_masked {
                    // Short single-cycle DMA (e.g. Scream Tracker 3's 5-byte 0x14 IRQ
                    // autodetect probe at 8 kHz = 0.625 ms) while DMA Ch 1 is unmasked.
                    // Complete immediately: advance dma_pos_fp by `len` bytes (so IN 0x03
                    // sees the 8237 counter decrement from buf-1 to buf-1-len) and raise
                    // IRQ 5 right away. When DMA Ch 1 is masked (probing DMA 0 or DMA 3),
                    // do NOT fire IRQ 5 so autodetectors correctly identify DMA 1 only.
                    let stride = if self.is_stereo() { 2u32 } else { 1u32 };
                    let frames = (len / stride).max(1);
                    self.dma_pos_fp = frames << 16;
                    self.dma_active = false;
                    self.dma_rem_block = 0;
                    self.irq_pending_count = self.irq_pending_count.saturating_add(1);
                    self.irq_count = self.irq_count.wrapping_add(1);
                }
            }
            0x40 => {
                self.time_const = self.cmd_args[0];
                self.refresh_sample_rate();
            }
            0x48 => {
                self.block_len = ((self.cmd_args[1] as u32) << 8 | (self.cmd_args[0] as u32)) + 1;
            }
            0xE0 => {
                self.push_read(!self.cmd_args[0]);
            }
            0xE4 => {
                self.test_reg = self.cmd_args[0];
            }
            _ => {}
        }
    }

    /// Sample next stereo PCM frame `(left, right)` in `i16` range from SB DMA / direct DAC.
    /// Note: DMA progression and end-of-block IRQ 5 MUST advance even while `speaker_on` is
    /// false, because real-mode autodetectors (e.g. Scream Tracker 3) probe IRQ 5 with a
    /// short single-cycle DMA (`0x14 0x00 0x00`) before issuing `0xD1` (Speaker On).
    fn next_pcm_frame(&mut self) -> (i32, i32) {
        if self.dma_active && !self.dma_paused && !self.dma1_masked && self.dma_buf_len > 0 {
            let stereo = self.is_stereo();
            let stride = if stereo { 2u32 } else { 1u32 };
            let buf_frames = (self.dma_buf_len / stride).max(1);
            let f_idx = (self.dma_pos_fp >> 16) % buf_frames;
            let f_nxt = (f_idx + 1) % buf_frames;
            let b_idx = f_idx * stride;
            let b_nxt = f_nxt * stride;
            let frac8 = ((self.dma_pos_fp >> 8) & 0xFF) as i32;
            let (l_cur, r_cur, l_nxt, r_nxt) = if self.speaker_on && self.dma_phys != 0 {
                unsafe {
                    let p = self.dma_phys as *const u8;
                    let l0 = core::ptr::read_volatile(p.add(b_idx as usize)) as i32;
                    let l1 = core::ptr::read_volatile(p.add(b_nxt as usize)) as i32;
                    if stereo {
                        let r0 = core::ptr::read_volatile(p.add(((b_idx + 1) % self.dma_buf_len) as usize)) as i32;
                        let r1 = core::ptr::read_volatile(p.add(((b_nxt + 1) % self.dma_buf_len) as usize)) as i32;
                        (l0, r0, l1, r1)
                    } else {
                        (l0, l0, l1, l1)
                    }
                }
            } else {
                (128, 128, 128, 128)
            };

            // Advance 16.16 fixed-point FRAME cursor (always aligned to `stride` bytes)
            let step_fp = (self.sample_rate << 16) / MIX_RATE;
            let old_frame = self.dma_pos_fp >> 16;
            self.dma_pos_fp = self.dma_pos_fp.wrapping_add(step_fp);
            let new_frame = self.dma_pos_fp >> 16;
            let frac = self.dma_pos_fp & 0xFFFF;
            let adv_bytes = new_frame.wrapping_sub(old_frame) * stride;
            if adv_bytes > 0 {
                if self.dma_rem_block <= adv_bytes {
                    self.irq_pending_count = self.irq_pending_count.saturating_add(1);
                    self.irq_count = self.irq_count.wrapping_add(1);
                    if self.dma_auto {
                        // Continuous auto-init playback: raise IRQ and continue into next block
                        self.dma_rem_block = self.block_len.max(1);
                        if new_frame >= buf_frames {
                            self.dma_pos_fp = ((new_frame % buf_frames) << 16) | frac;
                        }
                    } else {
                        self.dma_active = false;
                        self.dma_rem_block = 0;
                    }
                } else {
                    self.dma_rem_block -= adv_bytes;
                    if new_frame >= buf_frames {
                        self.dma_pos_fp = ((new_frame % buf_frames) << 16) | frac;
                    }
                }
            }

            if !self.speaker_on {
                return (0, 0);
            }

            // Linear interpolation + headroom-safe gain (±24576 max so SFX + FM never clips)
            let li = ((l_cur - 128) << 8) + ((l_nxt - l_cur) * frac8);
            let ri = ((r_cur - 128) << 8) + ((r_nxt - r_cur) * frac8);
            let l = (li * 3) >> 2;
            let r = (ri * 3) >> 2;
            return (l, r);
        }

        if !self.speaker_on {
            return (0, 0);
        }

        if self.direct_active {
            let s = ((self.direct_dac as i32) - 128) << 7;
            return (s, s);
        }

        (0, 0)
    }
}

// ---------------------------------------------------------------------
// MPU-401 UART (0x330/0x331) + ALi M5451 31-Voice Hardware MIDI Engine
// ---------------------------------------------------------------------

/// MIDI note frequencies in Hz * 16 for octave 5 (MIDI notes 72..=83, C5..B5).
/// Exact 12-TET integer table, shifted by octave for any MIDI note 0..=127.
const MIDI_OCT5_HZ_X16: [u32; 12] = [
    8372,  // C5  523.25 Hz
    8870,  // C#5 554.37 Hz
    9397,  // D5  587.33 Hz
    9956,  // D#5 622.25 Hz
    10548, // E5  659.25 Hz
    11175, // F5  698.46 Hz
    11840, // F#5 739.99 Hz
    12544, // G5  783.99 Hz
    13290, // G#5 830.61 Hz
    14080, // A5  880.00 Hz
    14917, // A#5 932.33 Hz
    15804, // B5  987.77 Hz
];

/// Convert MIDI note (0..=127) + pitch bend (-8192..=8191, ±2 semitones)
/// to ALi M5451 16-bit `DELTA` for a 256-sample wavetable cycle at 48 kHz:
///   `DELTA = (freq_hz * 256 * 4096) / 48000 = (freq_hz_x16 * 4096) / 3000`.
pub fn midi_note_to_m5451_delta(note: u8, bend: i16) -> u16 {
    let n = note.min(127) as i32;
    let oct = n / 12;
    let semi = (n % 12) as usize;
    let base_hz16 = MIDI_OCT5_HZ_X16[semi];
    let hz16 = if oct >= 6 {
        base_hz16 << (oct - 6).min(4)
    } else {
        base_hz16 >> (6 - oct)
    };
    // Apply ±2 semitone pitch bend (~±12.2% frequency): bend / 8192 * 1/8
    let adj = if bend != 0 {
        let delta_hz = ((hz16 as i64) * (bend as i64)) >> 16;
        ((hz16 as i64) + delta_hz).max(16) as u32
    } else {
        hz16.max(16)
    };
    let d = ((adj as u64) * 4096) / 3000;
    d.clamp(1, 0xFFFF) as u16
}

/// Map General MIDI program (0..=127) or Channel 10 percussion to one of
/// the 8 built-in ALi M5451 DMA wavetables (0..=7).
pub const fn gm_program_to_wave(ch: u8, prog: u8) -> u8 {
    if ch == 9 {
        return 7; // Channel 10 (0-based 9): GM Percussion
    }
    match prog {
        0..=15 => 0,   // Piano / Chromatic Percussion
        16..=23 => 1,  // Organ
        24..=39 => 2,  // Guitar / Bass
        40..=55 => 3,  // Strings / Ensemble
        56..=71 => 4,  // Brass / Reed
        72..=79 => 5,  // Pipe / Flute
        80..=87 => 6,  // Synth Lead
        88..=95 => 3,  // Synth Pad
        _ => 6,        // Synth FX / Ethnic / Percussive
    }
}

#[derive(Copy, Clone)]
struct MidiVoice {
    active: bool,
    ch: u8,
    note: u8,
    age: u32,
}

impl MidiVoice {
    const ZERO: Self = Self {
        active: false,
        ch: 0,
        note: 0,
        age: 0,
    };
}

struct MpuMidi {
    uart_mode: bool,
    rx_fifo: [u8; 8],
    rx_head: usize,
    rx_len: usize,
    // MIDI stream parser
    running_status: u8,
    in_sysex: bool,
    data_buf: [u8; 2],
    data_got: u8,
    data_need: u8,
    // 16 MIDI channels
    ch_prog: [u8; 16],
    ch_vol: [u8; 16],
    ch_pan: [u8; 16],
    ch_bend: [i16; 16],
    // 31 hardware voices (indices 0..31 map to ALi M5451 voices 1..=31)
    voices: [MidiVoice; 31],
    age_seq: u32,
    notes_on_total: u32,
}

impl MpuMidi {
    const fn new() -> Self {
        Self {
            uart_mode: false,
            rx_fifo: [0; 8],
            rx_head: 0,
            rx_len: 0,
            running_status: 0,
            in_sysex: false,
            data_buf: [0; 2],
            data_got: 0,
            data_need: 0,
            ch_prog: [0; 16],
            ch_vol: [100; 16],
            ch_pan: [64; 16],
            ch_bend: [0; 16],
            voices: [MidiVoice::ZERO; 31],
            age_seq: 1,
            notes_on_total: 0,
        }
    }

    fn reset(&mut self) {
        crate::ali5451::hw_midi_stop_all();
        *self = Self::new();
    }

    fn push_rx(&mut self, b: u8) {
        if self.rx_len < self.rx_fifo.len() {
            let idx = (self.rx_head + self.rx_len) % self.rx_fifo.len();
            self.rx_fifo[idx] = b;
            self.rx_len += 1;
        }
    }

    fn read_status(&self) -> u8 {
        // Bit 7 (0x80): 0 = RX data ready (DSR), 1 = RX empty
        // Bit 6 (0x40): 0 = TX ready (DRR), 1 = TX busy (never busy)
        if self.rx_len > 0 { 0x00 } else { 0x80 }
    }

    fn read_data(&mut self) -> u8 {
        if self.rx_len == 0 {
            return 0xFF;
        }
        let b = self.rx_fifo[self.rx_head];
        self.rx_head = (self.rx_head + 1) % self.rx_fifo.len();
        self.rx_len -= 1;
        b
    }

    fn write_cmd(&mut self, cmd: u8) {
        match cmd {
            0xFF => {
                // MPU-401 Reset -> stop all voices, return to intelligent mode, queue ACK 0xFE
                self.reset();
                self.push_rx(0xFE);
            }
            0x3F => {
                // Enter UART mode -> queue ACK 0xFE
                self.uart_mode = true;
                self.push_rx(0xFE);
            }
            _ => {
                // Acknowledge other harmless MPU-401 commands cleanly
                self.push_rx(0xFE);
            }
        }
    }

    fn write_data(&mut self, b: u8) {
        // Real-time messages (0xF8..=0xFF) do not affect running status
        if b >= 0xF8 {
            return;
        }
        if b == 0xF0 {
            self.in_sysex = true;
            self.running_status = 0;
            self.data_got = 0;
            self.data_need = 0;
            return;
        }
        if b == 0xF7 {
            self.in_sysex = false;
            return;
        }
        if self.in_sysex {
            if (b & 0x80) != 0 {
                self.in_sysex = false;
            } else {
                return;
            }
        }
        if (b & 0x80) != 0 {
            if b >= 0xF0 {
                self.running_status = 0;
                self.data_need = match b {
                    0xF1 | 0xF3 => 1,
                    0xF2 => 2,
                    _ => 0,
                };
                self.data_got = 0;
                return;
            }
            self.running_status = b;
            self.data_got = 0;
            self.data_need = match b & 0xF0 {
                0xC0 | 0xD0 => 1,
                _ => 2,
            };
            return;
        }
        // Data byte
        if self.data_need == 0 {
            if self.running_status == 0 {
                return;
            }
            self.data_need = match self.running_status & 0xF0 {
                0xC0 | 0xD0 => 1,
                _ => 2,
            };
            self.data_got = 0;
        }
        let idx = self.data_got as usize;
        if idx < 2 {
            self.data_buf[idx] = b;
        }
        self.data_got += 1;
        if self.data_got >= self.data_need {
            let status = self.running_status;
            let d0 = self.data_buf[0];
            let d1 = self.data_buf[1];
            self.data_got = 0;
            if status != 0 {
                self.dispatch_msg(status, d0, d1);
            }
        }
    }

    fn dispatch_msg(&mut self, status: u8, d0: u8, d1: u8) {
        let ch = status & 0x0F;
        match status & 0xF0 {
            0x80 => self.note_off(ch, d0),
            0x90 => {
                if d1 == 0 {
                    self.note_off(ch, d0);
                } else {
                    self.note_on(ch, d0, d1);
                }
            }
            0xB0 => match d0 {
                7 => self.ch_vol[ch as usize] = d1 & 0x7F,
                10 => self.ch_pan[ch as usize] = d1 & 0x7F,
                120 | 123 => self.all_notes_off_ch(ch),
                _ => {}
            },
            0xC0 => self.ch_prog[ch as usize] = d0 & 0x7F,
            0xE0 => {
                let raw = ((d1 as i16) << 7) | (d0 as i16);
                self.ch_bend[ch as usize] = raw - 8192;
            }
            _ => {}
        }
    }

    fn note_on(&mut self, ch: u8, note: u8, vel: u8) {
        // Reuse existing voice for same (ch, note), else first free, else oldest.
        let mut slot = 0usize;
        let mut found = false;
        for i in 0..self.voices.len() {
            if self.voices[i].active && self.voices[i].ch == ch && self.voices[i].note == note {
                slot = i;
                found = true;
                break;
            }
        }
        if !found {
            for i in 0..self.voices.len() {
                if !self.voices[i].active {
                    slot = i;
                    found = true;
                    break;
                }
            }
        }
        if !found {
            let mut oldest_age = u32::MAX;
            for i in 0..self.voices.len() {
                if self.voices[i].age < oldest_age {
                    oldest_age = self.voices[i].age;
                    slot = i;
                }
            }
        }

        let seq = self.age_seq;
        self.age_seq = self.age_seq.wrapping_add(1).max(1);
        self.voices[slot] = MidiVoice {
            active: true,
            ch,
            note,
            age: seq,
        };
        self.notes_on_total = self.notes_on_total.wrapping_add(1);

        let wave = gm_program_to_wave(ch, self.ch_prog[ch as usize]);
        let delta = if ch == 9 {
            // Percussion pitch scaled from drum key number (35..81)
            let drum_note = 48u8.saturating_add(note.saturating_sub(35).min(36));
            midi_note_to_m5451_delta(drum_note, 0)
        } else {
            midi_note_to_m5451_delta(note, self.ch_bend[ch as usize])
        };
        // Attenuation: 0x00 = loudest, 0xFF = silent; keep comfortable headroom (~24..140)
        let eff = ((vel as u32) * (self.ch_vol[ch as usize] as u32)) >> 7; // 0..127
        let atten = (140u32.saturating_sub(eff)).clamp(20, 220) as u8;
        // 4DWAVE-DX PAN: bit 6 = direction (0=left, 1=right), bits 5:0 = attenuation
        let p = self.ch_pan[ch as usize];
        let pan = if p >= 64 {
            ((p - 64) >> 1) & 0x3F
        } else {
            0x40 | (((64 - p) >> 1) & 0x3F)
        };
        let hw_voice = (slot + 1) as u8;
        // If an .SF2 SoundFont bank is loaded in XMS, play the real 16-bit
        // multi-sampled instrument directly via ALi M5451 30-bit hardware DMA;
        // otherwise fall back seamlessly to the built-in 4 KiB 8-waveform ROM.
        if let Some(vp) = crate::sf2::lookup_note(
            ch,
            self.ch_prog[ch as usize],
            note,
            self.ch_bend[ch as usize],
        ) {
            let total_atten = atten.saturating_add(vp.extra_atten).min(240);
            crate::ali5451::hw_midi_note_on_pcm(
                hw_voice,
                vp.lba_phys,
                vp.cso,
                vp.eso,
                vp.delta,
                total_atten,
                pan,
                vp.looping,
            );
        } else {
            let looping = ch != 9; // Percussion plays one-shot; melodic instruments loop
            crate::ali5451::hw_midi_note_on(hw_voice, wave, delta, atten, pan, looping);
        }
    }

    fn note_off(&mut self, ch: u8, note: u8) {
        for i in 0..self.voices.len() {
            if self.voices[i].active && self.voices[i].ch == ch && self.voices[i].note == note {
                self.voices[i].active = false;
                crate::ali5451::hw_midi_note_off((i + 1) as u8);
            }
        }
    }

    fn all_notes_off_ch(&mut self, ch: u8) {
        for i in 0..self.voices.len() {
            if self.voices[i].active && self.voices[i].ch == ch {
                self.voices[i].active = false;
                crate::ali5451::hw_midi_note_off((i + 1) as u8);
            }
        }
    }
}

struct SbOplState {
    opl: Opl2,
    opl1: Opl2,
    opl3_mode: bool,
    sb: SbDsp,
    mpu: MpuMidi,
}

impl SbOplState {
    const fn new() -> Self {
        Self {
            opl: Opl2::new(),
            opl1: Opl2::new(),
            opl3_mode: false,
            sb: SbDsp::new(),
            mpu: MpuMidi::new(),
        }
    }
}

struct SyncSb(UnsafeCell<SbOplState>);
unsafe impl Sync for SyncSb {}

static SB_OPL: SyncSb = SyncSb(UnsafeCell::new(SbOplState::new()));

/// Reset Sound Blaster Pro DSP + OPL3 FM + MPU-401 hardware MIDI state.
pub fn reset_all() {
    let st = unsafe { &mut *SB_OPL.0.get() };
    st.opl.reset();
    st.opl1.reset();
    st.opl3_mode = false;
    st.sb = SbDsp::new();
    st.mpu.reset();
}

/// Render up to `max_frames` of mixed 16-bit signed stereo PCM at `22,050 Hz`
/// into `ring`, stopping immediately as soon as an SB DMA block completes and
/// raises `irq_pending_count > 0`. This lets `_timer_hookproc` fire `INT 0Dh`
/// between every single DMA block so games like DOOM refill the next 256-byte
/// block BEFORE the renderer reads it (eliminating stale/repeating SFX).
pub fn render_into_ring_until_irq(
    ring: &mut [u8],
    mut write_frame: usize,
    max_frames: usize,
) -> (usize, usize, bool) {
    let ring_frames = ring.len() / 4;
    if ring_frames == 0 || max_frames == 0 {
        return (write_frame, 0, false);
    }
    let st = unsafe { &mut *SB_OPL.0.get() };
    let mut rendered = 0usize;

    for _ in 0..max_frames {
        let (mut fm_raw_l, mut fm_raw_r) = st.opl.next_sample_stereo(st.opl3_mode);
        if st.opl3_mode {
            let (b1_l, b1_r) = st.opl1.next_sample_stereo(true);
            fm_raw_l += b1_l;
            fm_raw_r += b1_r;
        }
        let (pcm_l, pcm_r) = st.sb.next_pcm_frame();

        let (fm_l, fm_r) = SbDsp::apply_vol(st.sb.mixer_regs[0x26], fm_raw_l, fm_raw_r);
        let (v_l, v_r) = SbDsp::apply_vol(st.sb.mixer_regs[0x04], pcm_l, pcm_r);
        let (mix_l, mix_r) = SbDsp::apply_vol(st.sb.mixer_regs[0x22], fm_l + v_l, fm_r + v_r);

        let l = mix_l.clamp(-32767, 32767) as i16;
        let r = mix_r.clamp(-32767, 32767) as i16;

        let off = write_frame * 4;
        ring[off] = (l & 0xFF) as u8;
        ring[off + 1] = ((l >> 8) & 0xFF) as u8;
        ring[off + 2] = (r & 0xFF) as u8;
        ring[off + 3] = ((r >> 8) & 0xFF) as u8;

        write_frame += 1;
        if write_frame >= ring_frames {
            write_frame = 0;
        }
        rendered += 1;

        if st.sb.irq_pending_count > 0 {
            return (write_frame, rendered, true);
        }
    }

    (write_frame, rendered, st.sb.irq_pending_count > 0)
}

/// Render `nframes` of mixed 16-bit signed stereo PCM at `22,050 Hz` into a
/// circular DMA buffer `ring` starting at frame index `write_frame` (`0..ring_frames`).
/// Returns the updated `write_frame` and whether an SB IRQ 5 is pending.
pub fn render_into_ring(ring: &mut [u8], mut write_frame: usize, nframes: usize) -> (usize, bool) {
    let ring_frames = ring.len() / 4;
    if ring_frames == 0 || nframes == 0 {
        return (write_frame, false);
    }
    let st = unsafe { &mut *SB_OPL.0.get() };

    for _ in 0..nframes {
        let (mut fm_raw_l, mut fm_raw_r) = st.opl.next_sample_stereo(st.opl3_mode);
        if st.opl3_mode {
            let (b1_l, b1_r) = st.opl1.next_sample_stereo(true);
            fm_raw_l += b1_l;
            fm_raw_r += b1_r;
        }
        let (pcm_l, pcm_r) = st.sb.next_pcm_frame();

        // The CT1345 mixer registers are the game's only volume control;
        // OPL3 stereo (or OPL2 mono) takes the FM register's per-channel gains,
        // the voice register scales the DMA/direct-DAC path, and the master
        // register scales the final sum.
        let (fm_l, fm_r) = SbDsp::apply_vol(st.sb.mixer_regs[0x26], fm_raw_l, fm_raw_r);
        let (v_l, v_r) = SbDsp::apply_vol(st.sb.mixer_regs[0x04], pcm_l, pcm_r);
        let (mix_l, mix_r) = SbDsp::apply_vol(st.sb.mixer_regs[0x22], fm_l + v_l, fm_r + v_r);

        let l = mix_l.clamp(-32767, 32767) as i16;
        let r = mix_r.clamp(-32767, 32767) as i16;

        let off = write_frame * 4;
        ring[off] = (l & 0xFF) as u8;
        ring[off + 1] = ((l >> 8) & 0xFF) as u8;
        ring[off + 2] = (r & 0xFF) as u8;
        ring[off + 3] = ((r >> 8) & 0xFF) as u8;

        write_frame += 1;
        if write_frame >= ring_frames {
            write_frame = 0;
        }
    }

    (write_frame, st.sb.irq_pending_count > 0)
}

/// Return and clear the number of pending SB IRQ 5 events (never drops multi-block IRQs).
pub fn drain_irq_count() -> u8 {
    let st = unsafe { &mut *SB_OPL.0.get() };
    let n = st.sb.irq_pending_count;
    st.sb.irq_pending_count = 0;
    n
}

/// Peek whether an SB IRQ 5 is pending (without clearing it).
pub fn peek_irq_pending() -> bool {
    let st = unsafe { &*SB_OPL.0.get() };
    st.sb.irq_pending_count > 0
}

/// `AX=0x0027`: Copy 32-byte `SbDiag` telemetry snapshot to `out_ptr`.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sb_diag(out_ptr: u32) -> u32 {
    let st = unsafe { &*SB_OPL.0.get() };
    let diag = SbDiag {
        last_cmd: st.sb.last_dma_cmd,
        dma_auto: if st.sb.dma_auto { 1 } else { 0 },
        stereo: if st.sb.is_stereo() { 1 } else { 0 },
        active: if st.sb.dma_active { 1 } else { 0 },
        sample_rate: st.sb.sample_rate,
        dma_phys: st.sb.dma_phys,
        dma_buf_len: st.sb.dma_buf_len,
        block_len: st.sb.block_len,
        io_count: st.sb.io_count,
        opl_count: st.sb.opl_count,
        irq_count: (st.sb.irq_count & 0xFFFF) as u16,
        ack22e_count: (st.sb.ack22e_count & 0xFFFF) as u16,
    };
    if out_ptr != 0 {
        unsafe { core::ptr::write_unaligned(out_ptr as *mut SbDiag, diag) };
    }
    0
}

/// 32-byte DMA Channel 1 diagnostic snapshot returned by `AX=0x0028`.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default)]
pub struct SbDmaDiag {
    pub dma_rd_count: u32,         // +0x00 (port 0x02 reads)
    pub dma_wr_count: u32,         // +0x04 (port 0x02/0x03 writes)
    pub dma_pos_bytes: u16,        // +0x08
    pub dma1_mode: u8,             // +0x0A
    pub peak_stereo: u8,           // +0x0B
    pub nonzero_in_buf: u16,       // +0x0C
    pub time_const: u8,            // +0x0E
    pub shadow_valid: u8,          // +0x0F
    pub dma_rd3_count: u32,        // +0x10 (port 0x03 reads)
    pub sample_b0: [u8; 6],        // +0x14 (first 6 bytes at dma_phys+0)
    pub sample_b1: [u8; 6],        // +0x1A (first 6 bytes at dma_phys+256)
}

/// `AX=0x0028`: Copy 32-byte `SbDmaDiag` telemetry snapshot to `out_ptr`.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sb_dma_diag(out_ptr: u32) -> u32 {
    let st = unsafe { &*SB_OPL.0.get() };
    let stride = if st.sb.is_stereo() { 2u32 } else { 1u32 };
    let pos_bytes = if st.sb.dma_buf_len > 0 {
        (((st.sb.dma_pos_fp >> 16) * stride) % st.sb.dma_buf_len) as u16
    } else {
        0
    };
    let mut nz: u16 = 0;
    let mut b0 = [0u8; 6];
    let mut b1 = [0u8; 6];
    if st.sb.dma_phys != 0 && st.sb.dma_buf_len > 0 && st.sb.dma_buf_len <= 65536 {
        let p = st.sb.dma_phys as *const u8;
        for i in 0..(st.sb.dma_buf_len as usize) {
            let b = unsafe { core::ptr::read_volatile(p.add(i)) };
            if b != 0x80 && b != 0x00 {
                nz = nz.saturating_add(1);
            }
            if i < 6 {
                b0[i] = b;
            }
            if i >= 256 && i < 262 {
                b1[i - 256] = b;
            }
        }
    }
    let diag = SbDmaDiag {
        dma_rd_count: st.sb.dma_rd_count,
        dma_wr_count: st.sb.dma_wr_count,
        dma_pos_bytes: pos_bytes,
        dma1_mode: st.sb.dma1_mode,
        peak_stereo: st.sb.peak_stereo,
        nonzero_in_buf: nz,
        time_const: st.sb.time_const,
        shadow_valid: if st.sb.dma1_shadow_valid { 1 } else { 0 },
        dma_rd3_count: st.sb.dma_rd3_count,
        sample_b0: b0,
        sample_b1: b1,
    };
    if out_ptr != 0 {
        unsafe { core::ptr::write_unaligned(out_ptr as *mut SbDmaDiag, diag) };
    }
    0
}

/// `AX=0x002A`: Get/set live AC'97 + Sound Blaster Pro mixer levels for `MIXER.COM`.
/// - `cmd == 0`: read current levels (no change)
/// - `cmd == 1`: set AC'97 speaker volume (`val = 0..=9`)
/// - `cmd == 2`: set SB Master volume (`val = 0..=15`)
/// - `cmd == 3`: set SB Voice/SFX volume (`val = 0..=15`)
/// - `cmd == 4`: set SB FM/Music volume (`val = 0..=15`)
/// - `cmd == 5`: reset all channels to defaults (`AC97=7, Master=12, Voice=12, FM=12`)
/// Returns packed dword: `ac97 | (master << 8) | (voice << 16) | (fm << 24)`.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sb_mixer_ctl(cmd: u32, val: u32) -> u32 {
    let st = unsafe { &mut *SB_OPL.0.get() };
    match cmd {
        1 => {
            let _ = crate::ali5451::gate1_audio_set_vol(val.min(9));
        }
        2 => {
            let v = (val.min(15) as u8) & 0x0F;
            st.sb.mixer_regs[0x22] = (v << 4) | v;
        }
        3 => {
            let v = (val.min(15) as u8) & 0x0F;
            st.sb.mixer_regs[0x04] = (v << 4) | v;
        }
        4 => {
            let v = (val.min(15) as u8) & 0x0F;
            st.sb.mixer_regs[0x26] = (v << 4) | v;
        }
        5 => {
            let _ = crate::ali5451::gate1_audio_set_vol(7);
            st.sb.mixer_regs[0x22] = 0xCC;
            st.sb.mixer_regs[0x04] = 0xCC;
            st.sb.mixer_regs[0x26] = 0xCC;
        }
        _ => {}
    }
    let ac97 = crate::ali5451::get_ac97_vol() as u32;
    let master = ((st.sb.mixer_regs[0x22] >> 4) & 0x0F) as u32;
    let voice = ((st.sb.mixer_regs[0x04] >> 4) & 0x0F) as u32;
    let fm = ((st.sb.mixer_regs[0x26] >> 4) & 0x0F) as u32;
    ac97 | (master << 8) | (voice << 16) | (fm << 24)
}

/// Consume up to `max` pending SB IRQ 5 events, leaving any remainder queued
/// for the next delivery pass so auto-init DMA games never lose a block IRQ.
pub fn drain_irq_up_to(max: u8) -> u8 {
    let st = unsafe { &mut *SB_OPL.0.get() };
    let take = st.sb.irq_pending_count.min(max);
    st.sb.irq_pending_count -= take;
    take
}

/// Consume up to `max` pending SB IRQ 5 events (exported to jlm_stub.asm).
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sb_take_up_to(max: u32) -> u32 {
    drain_irq_up_to(max.min(255) as u8) as u32
}

/// Consume and return the count of pending SB IRQ 5 events (exported to jlm_stub.asm).
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sb_take_irq() -> u32 {
    drain_irq_count() as u32
}

/// Consume exactly one pending SB IRQ 5 event (exported to jlm_stub.asm).
/// The I/O trap sites deliver a single INT 0Dh per trap; draining the
/// whole counter there, as they used to through gate1_sb_take_irq,
/// silently discarded every other pending block IRQ -- in auto-init
/// DMA a lost block IRQ leaves the game refilling the wrong half of
/// its buffer. The tick/IRQ delivery loop is the only site that may
/// drain, because it is also the only one that loops the delivery.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sb_take_one_irq() -> u32 {
    if take_irq_pending() { 1 } else { 0 }
}

/// Consume one pending SB IRQ 5 event.
pub fn take_irq_pending() -> bool {
    let st = unsafe { &mut *SB_OPL.0.get() };
    if st.sb.irq_pending_count > 0 {
        st.sb.irq_pending_count -= 1;
        true
    } else {
        false
    }
}

/// Return diagnostic counters `(io_count, irq_count)`.
pub fn diag_counts() -> (u32, u32) {
    let st = unsafe { &*SB_OPL.0.get() };
    (st.sb.io_count, st.sb.irq_count)
}

/// Ring-0 JEMM386 I/O trap callback for ports `0x220..0x22F` and `0x388..0x389`.
/// Called from `_sb_io_handler` in `jlm_stub.asm`.
/// - `port`: 16-bit I/O port number
/// - `io_type`: JEMM386 `ecx` flags (bit 2 = `IO_FLAG_OUTPUT`)
/// - `val`: data byte (for `OUT`)
/// Returns the updated client `EAX` for `IN` (ignored by JEMM for `OUT`):
/// JEMM's `@@io_handled` writes the entire 32-bit `EAX` back into `Client_EAX`,
/// so byte `IN` must preserve bits 31:8 of `val`, word `IN` must preserve bits
/// 31:16, and dword `IN` replaces all 32 bits.
#[unsafe(no_mangle)]
pub extern "C" fn gate1_sb_io(port: u32, io_type: u32, val: u32) -> u32 {
    // Split 16-bit (WORD_IO=0x08) and 32-bit (DWORD_IO=0x10) port accesses into
    // sequential byte accesses (e.g. OUT 0x388, AX or OUT 0x224, AX).
    const IO_FLAG_WORD: u32 = 0x08;
    const IO_FLAG_DWORD: u32 = 0x10;
    let byte_flags = io_type & !(IO_FLAG_WORD | IO_FLAG_DWORD);
    if (io_type & IO_FLAG_DWORD) != 0 {
        let b0 = gate1_sb_io_byte(port, byte_flags, val & 0xFF) & 0xFF;
        let b1 = gate1_sb_io_byte(port + 1, byte_flags, (val >> 8) & 0xFF) & 0xFF;
        let b2 = gate1_sb_io_byte(port + 2, byte_flags, (val >> 16) & 0xFF) & 0xFF;
        let b3 = gate1_sb_io_byte(port + 3, byte_flags, (val >> 24) & 0xFF) & 0xFF;
        return b0 | (b1 << 8) | (b2 << 16) | (b3 << 24);
    }
    if (io_type & IO_FLAG_WORD) != 0 {
        let b0 = gate1_sb_io_byte(port, byte_flags, val & 0xFF) & 0xFF;
        let b1 = gate1_sb_io_byte(port + 1, byte_flags, (val >> 8) & 0xFF) & 0xFF;
        return (val & 0xFFFF_0000) | b0 | (b1 << 8);
    }
    let b0 = gate1_sb_io_byte(port, byte_flags, val) & 0xFF;
    (val & 0xFFFF_FF00) | b0
}

fn gate1_sb_io_byte(port: u32, io_type: u32, val: u32) -> u32 {
    let is_out = (io_type & IO_FLAG_OUTPUT) != 0;
    let p16 = port as u16;
    let st = unsafe { &mut *SB_OPL.0.get() };
    let b = (val & 0xFF) as u8;
    st.sb.io_count = st.sb.io_count.wrapping_add(1);

    match p16 {
        // Shadowed ISA DMA Channel 1 ports (from HDPMI32i PM32 trap for DOOM/DOS4GW)
        0x02 => {
            if is_out {
                st.sb.dma_wr_count = st.sb.dma_wr_count.wrapping_add(1);
                st.sb.dma_pos_fp = 0;
                if st.sb.dma1_ff == 0 {
                    st.sb.dma1_base_lo = b;
                    st.sb.dma1_ff = 1;
                } else {
                    st.sb.dma1_base_hi = b;
                    st.sb.dma1_ff = 0;
                    st.sb.dma1_shadow_valid = true;
                }
                unsafe { outb_raw(0x02, b) };
                0
            } else {
                st.sb.dma_rd_count = st.sb.dma_rd_count.wrapping_add(1);
                let base = ((st.sb.dma1_base_hi as u32) << 8) | (st.sb.dma1_base_lo as u32);
                let cur_off = if st.sb.dma_buf_len > 0 && (st.sb.dma_active || st.sb.dma_pos_fp > 0) {
                    let stride = if st.sb.is_stereo() { 2u32 } else { 1u32 };
                    let raw_pos = (st.sb.dma_pos_fp >> 16) * stride;
                    let pos = if st.sb.dma_auto {
                        raw_pos % st.sb.dma_buf_len
                    } else {
                        raw_pos.min(st.sb.dma_buf_len)
                    };
                    base.wrapping_add(pos)
                } else {
                    base
                };
                let res = if st.sb.dma1_ff == 0 {
                    st.sb.dma1_ff = 1;
                    cur_off & 0xFF
                } else {
                    st.sb.dma1_ff = 0;
                    (cur_off >> 8) & 0xFF
                };
                res
            }
        }
        0x03 => {
            if is_out {
                st.sb.dma_wr_count = st.sb.dma_wr_count.wrapping_add(1);
                st.sb.dma_pos_fp = 0;
                if st.sb.dma1_ff == 0 {
                    st.sb.dma1_cnt_lo = b;
                    st.sb.dma1_ff = 1;
                } else {
                    st.sb.dma1_cnt_hi = b;
                    st.sb.dma1_ff = 0;
                    st.sb.dma1_shadow_valid = true;
                }
                unsafe { outb_raw(0x03, b) };
                0
            } else {
                st.sb.dma_rd3_count = st.sb.dma_rd3_count.wrapping_add(1);
                let rem = if st.sb.dma_buf_len > 0 && (st.sb.dma_active || st.sb.dma_pos_fp > 0) {
                    let stride = if st.sb.is_stereo() { 2u32 } else { 1u32 };
                    let raw_pos = (st.sb.dma_pos_fp >> 16) * stride;
                    if st.sb.dma_auto {
                        let pos = raw_pos % st.sb.dma_buf_len;
                        st.sb.dma_buf_len.saturating_sub(pos + 1)
                    } else if raw_pos < st.sb.dma_buf_len {
                        st.sb.dma_buf_len - 1 - raw_pos
                    } else {
                        0xFFFF
                    }
                } else {
                    ((st.sb.dma1_cnt_hi as u32) << 8) | (st.sb.dma1_cnt_lo as u32)
                };
                let res = if st.sb.dma1_ff == 0 {
                    st.sb.dma1_ff = 1;
                    rem & 0xFF
                } else {
                    st.sb.dma1_ff = 0;
                    (rem >> 8) & 0xFF
                };
                res
            }
        }
        0x0A => {
            if is_out {
                if (b & 0x03) == 1 {
                    let was_masked = st.sb.dma1_masked;
                    st.sb.dma1_masked = (b & 0x04) != 0;
                    // If a short IRQ-probe DMA (<= 16 B) was armed before unmasking Ch 1,
                    // complete it immediately now that Ch 1 is unmasked.
                    if was_masked && !st.sb.dma1_masked && st.sb.dma_active && st.sb.block_len <= 16 {
                        let stride = if st.sb.is_stereo() { 2u32 } else { 1u32 };
                        let frames = (st.sb.block_len / stride).max(1);
                        st.sb.dma_pos_fp = frames << 16;
                        st.sb.dma_active = false;
                        st.sb.dma_rem_block = 0;
                        st.sb.irq_pending_count = st.sb.irq_pending_count.saturating_add(1);
                        st.sb.irq_count = st.sb.irq_count.wrapping_add(1);
                    }
                }
                unsafe { outb_raw(0x0A, b) };
                0
            } else {
                unsafe { inb_raw(0x0A) as u32 }
            }
        }
        0x0B => {
            if is_out {
                if (b & 0x03) == 1 {
                    st.sb.dma1_mode = b;
                }
                unsafe { outb_raw(0x0B, b) };
                0
            } else {
                unsafe { inb_raw(0x0B) as u32 }
            }
        }
        0x0C => {
            if is_out {
                st.sb.dma1_ff = 0;
                unsafe { outb_raw(0x0C, b) };
                0
            } else {
                0xFF
            }
        }
        0x83 => {
            if is_out {
                st.sb.dma1_page = b;
                st.sb.dma1_shadow_valid = true;
                unsafe { outb_raw(0x83, b) };
                0
            } else {
                st.sb.dma1_page as u32
            }
        }
        // AdLib / SB Pro / OPL3 Bank 0 Address & Status
        0x388 | 0x220 | 0x228 => {
            if is_out {
                st.opl.write_addr(b);
                0
            } else {
                st.opl.read_status() as u32
            }
        }
        // AdLib / SB Pro / OPL3 Bank 0 Data
        0x389 | 0x221 | 0x229 => {
            if is_out {
                st.sb.opl_count = st.sb.opl_count.wrapping_add(1);
                st.opl.write_data(b);
                0
            } else {
                0xFF
            }
        }
        // OPL3 Bank 1 Address & Status
        0x38A | 0x222 => {
            if is_out {
                st.opl1.write_addr(b);
                0
            } else {
                0x00
            }
        }
        // OPL3 Bank 1 Data (reg 0x05 bit 0 enables OPL3 18-ch stereo mode)
        0x38B | 0x223 => {
            if is_out {
                st.sb.opl_count = st.sb.opl_count.wrapping_add(1);
                if st.opl1.addr == 0x05 {
                    st.opl3_mode = (b & 0x01) != 0;
                }
                st.opl1.write_data(b);
                0
            } else {
                0xFF
            }
        }
        // MPU-401 UART Data Port (General MIDI -> ALi M5451 HW voices 1..=31)
        0x330 => {
            if is_out {
                st.mpu.write_data(b);
                0
            } else {
                st.mpu.read_data() as u32
            }
        }
        // MPU-401 UART Status / Command Port
        0x331 => {
            if is_out {
                st.mpu.write_cmd(b);
                0
            } else {
                st.mpu.read_status() as u32
            }
        }
        // SB Pro Mixer Address
        0x224 => {
            if is_out {
                st.sb.mixer_addr = b & 0x3F;
                0
            } else {
                st.sb.mixer_addr as u32
            }
        }
        // SB Pro Mixer Data
        0x225 => {
            let idx = (st.sb.mixer_addr & 0x3F) as usize;
            if is_out {
                if idx == 0x00 {
                    // A write to the reset register restores every mixer
                    // register to its power-on default; the value written
                    // carries no meaning. Without this, a game that
                    // resets the mixer before programming it inherits
                    // whatever levels a previous program left behind.
                    st.sb.mixer_regs = SbDsp::default_mixer_regs();
                } else {
                    st.sb.mixer_regs[idx] = b;
                }
                if idx == 0x00 || idx == 0x0E {
                    // The stereo bit (or its reset) changes the effective
                    // sample rate; see refresh_sample_rate.
                    if st.sb.is_stereo() {
                        st.sb.peak_stereo = 1;
                    }
                    st.sb.refresh_sample_rate();
                }
                0
            } else {
                st.sb.mixer_regs[idx] as u32
            }
        }
        // SB DSP Reset
        0x226 => {
            if is_out {
                if (b & 1) != 0 {
                    st.sb.reset_state = 1;
                } else if st.sb.reset_state == 1 {
                    st.sb.reset_state = 0;
                    st.sb.reset();
                }
                0
            } else {
                0xFF
            }
        }
        // SB DSP Read Data
        0x22A => {
            if !is_out {
                st.sb.pop_read() as u32
            } else {
                0
            }
        }
        // SB DSP Write Command/Data & Write Buffer Status
        0x22C => {
            if is_out {
                st.sb.write_cmd_or_data(b);
                0
            } else {
                0x00 // bit 7 = 0 -> ready to write
            }
        }
        // SB DSP Data Available Status & 8-bit IRQ Acknowledge
        0x22E => {
            if !is_out {
                st.sb.ack22e_count = st.sb.ack22e_count.wrapping_add(1);
                if st.sb.read_len > 0 { 0x80 } else { 0x00 }
            } else {
                0
            }
        }
        _ => 0xFF,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every test in this module drives the one shared SB_OPL state
    // through gate1_sb_io, and rustc runs tests on parallel threads:
    // one test's reset_all() or mixer write landing mid-sequence in
    // another corrupts its expectations (the mixer reset test flaked
    // exactly this way in the container suite after passing
    // repeatedly). Each test holds this lock for its whole body.
    static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn adlib_detection_timer_handshake_passes() {
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        // Standard AdLib detect sequence: reset timers + IRQ
        gate1_sb_io(0x388, IO_FLAG_OUTPUT, 0x04);
        gate1_sb_io(0x389, IO_FLAG_OUTPUT, 0x60);
        gate1_sb_io(0x388, IO_FLAG_OUTPUT, 0x04);
        gate1_sb_io(0x389, IO_FLAG_OUTPUT, 0x80);
        let s1 = gate1_sb_io(0x388, 0, 0) & 0xE0;
        assert_eq!(s1, 0x00);

        // Arm Timer 1 = 0xFF and start
        gate1_sb_io(0x388, IO_FLAG_OUTPUT, 0x02);
        gate1_sb_io(0x389, IO_FLAG_OUTPUT, 0xFF);
        gate1_sb_io(0x388, IO_FLAG_OUTPUT, 0x04);
        gate1_sb_io(0x389, IO_FLAG_OUTPUT, 0x21);
        let s2 = gate1_sb_io(0x388, 0, 0) & 0xE0;
        assert_eq!(s2, 0xC0);
    }

    #[test]
    fn sb_pro_reset_and_version_handshake() {
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        // Reset DSP: write 1 then 0 to 0x226
        gate1_sb_io(0x226, IO_FLAG_OUTPUT, 1);
        gate1_sb_io(0x226, IO_FLAG_OUTPUT, 0);
        assert_eq!(gate1_sb_io(0x22E, 0, 0) & 0x80, 0x80);
        assert_eq!(gate1_sb_io(0x22A, 0, 0), 0xAA);

        // Query DSP version (0xE1 -> 3.02)
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0xE1);
        assert_eq!(gate1_sb_io(0x22A, 0, 0), 0x03);
        assert_eq!(gate1_sb_io(0x22A, 0, 0), 0x02);

        // Trigger IRQ (0xF2) and verify 0x22E ack clears it
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0xF2);
        assert!(take_irq_pending());
        assert!(!take_irq_pending());
    }

    #[test]
    fn sb16_command_arguments_are_consumed_not_reparsed() {
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        // Reset DSP and drain the 0xAA reset reply.
        gate1_sb_io(0x226, IO_FLAG_OUTPUT, 1);
        gate1_sb_io(0x226, IO_FLAG_OUTPUT, 0);
        assert_eq!(gate1_sb_io(0x22A, 0, 0), 0xAA);

        // 0xB6 = SB16 16-bit auto-init output: mode byte + length lo/hi.
        // The low length byte is 0xF2 on purpose: if it were reparsed as
        // a command it would raise an IRQ.
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0xB6);
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0x00);
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0xF2);
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0x00);
        assert!(!take_irq_pending(), "SB16 length byte must not be reparsed as 0xF2");

        // The stream is still in sync: the version query answers in order.
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0xE1);
        assert_eq!(gate1_sb_io(0x22A, 0, 0), 0x03);
        assert_eq!(gate1_sb_io(0x22A, 0, 0), 0x02);

        // 0xE3 answers the copyright string on the read port.
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0xE3);
        assert_eq!(gate1_sb_io(0x22A, 0, 0), b'C' as u32);
    }

    #[test]
    fn mixer_master_volume_applies_and_reset_restores_defaults() {
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        // Direct DAC at full scale gives a constant non-zero PCM level.
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0x10);
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0xFF);
        let mut ring = [0u8; 256];
        let _ = render_into_ring(&mut ring, 0, 64);
        assert!(ring.iter().any(|&b| b != 0), "default mixer must pass audio");

        // Master volume 0x00 (both nibbles zero) must silence the mix.
        gate1_sb_io(0x224, IO_FLAG_OUTPUT, 0x22);
        gate1_sb_io(0x225, IO_FLAG_OUTPUT, 0x00);
        let mut ring = [0u8; 256];
        let _ = render_into_ring(&mut ring, 0, 64);
        assert!(ring.iter().all(|&b| b == 0), "master volume 0 must mute");

        // A write to mixer register 0x00 restores the power-on defaults.
        gate1_sb_io(0x224, IO_FLAG_OUTPUT, 0x00);
        gate1_sb_io(0x225, IO_FLAG_OUTPUT, 0x00);
        let mut ring = [0u8; 256];
        let _ = render_into_ring(&mut ring, 0, 64);
        assert!(ring.iter().any(|&b| b != 0), "mixer reset must restore defaults");
    }

    #[test]
    fn opl2_key_on_renders_nonzero_fm_audio() {
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        // Program channel 0 (A4 = 440 Hz) simple FM tone
        let seq: [(u8, u8); 10] = [
            (0x20, 0x01), // mod mult=1
            (0x40, 0x10), // mod level
            (0x60, 0xF0), // mod AR=15, DR=0
            (0x80, 0x77), // mod SL=7, RR=7
            (0x23, 0x21), // car sustaining, mult=1
            (0x43, 0x00), // car max level
            (0x63, 0xF0), // car AR=15, DR=0
            (0x83, 0x07), // car SL=0, RR=7
            (0xA0, 0x41), // fnum low
            (0xB0, 0x32), // key-on, block=4, fnum high=2
        ];
        for (r, v) in seq {
            gate1_sb_io(0x388, IO_FLAG_OUTPUT, r as u32);
            gate1_sb_io(0x389, IO_FLAG_OUTPUT, v as u32);
        }
        let mut ring = [0u8; 2048];
        let (wf, _) = render_into_ring(&mut ring, 0, 256);
        assert_eq!(wf, 256);
        let peak = ring
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(peak > 1000, "expected audible OPL2 FM peak, got {peak}");
    }

    #[test]
    fn opl2_ksl_attenuates_high_notes() {
        // KSL=0 -> 0 attenuation; KSL=3 (6 dB/oct) at block 7, fnum 0x3C0 -> strong attenuation
        assert_eq!(Opl2::calc_ksl_atten(0x00, 7, 0x3C0), 0);
        let hi_ksl = Opl2::calc_ksl_atten(0xC0, 7, 0x3C0);
        let lo_ksl = Opl2::calc_ksl_atten(0xC0, 1, 0x100);
        assert!(hi_ksl > lo_ksl && hi_ksl >= 40, "expected KSL high={hi_ksl} > low={lo_ksl}");
    }

    #[test]
    fn opl2_rhythm_mode_0xbd_renders_percussion() {
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        // Program Snare Drum (op 20 = 0x14) and Hi-Hat (op 17 = 0x11) on ch 7
        let seq: &[(u8, u8)] = &[
            (0x20 + 20, 0x01), // SD mult=1
            (0x40 + 20, 0x00), // SD TL=0
            (0x60 + 20, 0xF4), // SD AR=15, DR=4
            (0x80 + 20, 0x07), // SD SL=0, RR=7
            (0xA7, 0x50),
            (0xB7, 0x10),      // block=4
            (0xBD, 0x28),      // Rhythm mode ON + Snare Drum key-on
        ];
        for &(r, v) in seq {
            gate1_sb_io(0x388, IO_FLAG_OUTPUT, r as u32);
            gate1_sb_io(0x389, IO_FLAG_OUTPUT, v as u32);
        }
        let mut ring = [0u8; 2048];
        let _ = render_into_ring(&mut ring, 0, 256);
        let peak = ring
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]).unsigned_abs())
            .max()
            .unwrap_or(0);
        assert!(peak > 500, "expected audible OPL2 snare percussion, got {peak}");
    }

    #[test]
    fn mixer_ctl_packs_and_updates_channels() {
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        let def = gate1_sb_mixer_ctl(0, 0);
        assert_eq!((def >> 8) & 0xFF, 12);  // Master
        assert_eq!((def >> 16) & 0xFF, 12); // Voice
        assert_eq!((def >> 24) & 0xFF, 12); // FM

        let s1 = gate1_sb_mixer_ctl(3, 9);  // Voice = 9
        assert_eq!((s1 >> 16) & 0xFF, 9);
        let s2 = gate1_sb_mixer_ctl(4, 15); // FM = 15
        assert_eq!((s2 >> 24) & 0xFF, 15);
        let rst = gate1_sb_mixer_ctl(5, 0); // Reset defaults
        assert_eq!((rst >> 8) & 0xFF, 12);
        assert_eq!((rst >> 16) & 0xFF, 12);
        assert_eq!((rst >> 24) & 0xFF, 12);
    }

    #[test]
    fn st3_short_dma_probe_advances_dma1_counter_and_fires_irq() {
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        // Program DMA Channel 1 like Scream Tracker 3: base=0x3EBC0, count=21 (buf=22)
        gate1_sb_io(0x0C, IO_FLAG_OUTPUT, 0);
        gate1_sb_io(0x02, IO_FLAG_OUTPUT, 0xC0);
        gate1_sb_io(0x02, IO_FLAG_OUTPUT, 0xEB);
        gate1_sb_io(0x83, IO_FLAG_OUTPUT, 0x03);
        gate1_sb_io(0x0C, IO_FLAG_OUTPUT, 0);
        gate1_sb_io(0x03, IO_FLAG_OUTPUT, 21);
        gate1_sb_io(0x03, IO_FLAG_OUTPUT, 0);
        // Send DSP 0x14 0x04 0x00 (5-byte single-cycle IRQ/DMA probe)
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0x14);
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0x04);
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0x00);
        // IRQ 5 must be pending immediately
        assert_eq!(gate1_sb_take_one_irq(), 1);
        // Port 0x03 remaining count must have decremented from 21 to 16
        gate1_sb_io(0x0C, IO_FLAG_OUTPUT, 0);
        let lo = gate1_sb_io(0x03, 0, 0);
        let hi = gate1_sb_io(0x03, 0, 0);
        assert_eq!((hi << 8) | lo, 16);
    }

    #[test]
    fn drain_irq_up_to_preserves_remainder_above_cap() {
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        for _ in 0..6 {
            gate1_sb_io(0x22C, IO_FLAG_OUTPUT, 0xF2);
        }
        assert_eq!(gate1_sb_take_up_to(4), 4);
        assert_eq!(gate1_sb_take_up_to(4), 2);
        assert_eq!(gate1_sb_take_up_to(4), 0);
    }

    #[test]
    fn sb_io_preserves_upper_eax_bits_by_access_width() {
        const IO_FLAG_WORD: u32 = 0x08;
        const IO_FLAG_DWORD: u32 = 0x10;
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        // Reset DSP so 0x22A has 0xAA ready and 0x22C reports 0x00 (ready).
        gate1_sb_io(0x226, IO_FLAG_OUTPUT, 1);
        gate1_sb_io(0x226, IO_FLAG_OUTPUT, 0);

        // Byte IN: replaces AL only; bits 31:8 must survive untouched.
        let byte_res = gate1_sb_io(0x22A, 0, 0xDEAD_BE00);
        assert_eq!(byte_res, 0xDEAD_BEAA);

        // Word IN: replaces AX only; bits 31:16 must survive untouched.
        let word_res = gate1_sb_io(0x22C, IO_FLAG_WORD, 0xCAFE_1234);
        assert_eq!(word_res & 0xFFFF_0000, 0xCAFE_0000);

        // Dword IN: replaces all 32 bits (upper 16 bits are overwritten by ports +2/+3).
        let dword_res = gate1_sb_io(0x22C, IO_FLAG_DWORD, 0xCAFE_1234);
        assert_ne!(dword_res & 0xFFFF_0000, 0xCAFE_0000);
    }

    #[test]
    fn digpak_soundrv_e0_identification_preserves_ah_across_status_poll() {
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        // Reset DSP and drain 0xAA.
        gate1_sb_io(0x226, IO_FLAG_OUTPUT, 1);
        gate1_sb_io(0x226, IO_FLAG_OUTPUT, 0);
        assert_eq!(gate1_sb_io(0x22A, 0, 0) & 0xFF, 0xAA);

        // DIGPAK SOUNDRV.COM helper pattern:
        //   MOV AH, byte_to_send
        //   IN  AL, 0x22C          ; poll write-buffer status (returns 0x00 in AL)
        //   MOV AL, AH             ; recover saved byte from AH!
        //   OUT 0x22C, AL
        let mut eax = 0x1234_E0FFu32; // AH = 0xE0 (DSP Identification)
        eax = gate1_sb_io(0x22C, 0, eax);
        assert_eq!(eax & 0x80, 0, "DSP write buffer should be ready");
        assert_eq!((eax >> 8) & 0xFF, 0xE0, "AH must survive IN AL, 0x22C");
        assert_eq!(eax & 0xFFFF_0000, 0x1234_0000, "upper EAX must survive");
        let cmd = (eax >> 8) & 0xFF;
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, cmd);

        eax = 0x5678_AAFFu32; // AH = 0xAA (argument to 0xE0)
        eax = gate1_sb_io(0x22C, 0, eax);
        assert_eq!((eax >> 8) & 0xFF, 0xAA, "AH must survive second IN AL, 0x22C");
        let arg = (eax >> 8) & 0xFF;
        gate1_sb_io(0x22C, IO_FLAG_OUTPUT, arg);

        // DSP must now have 0x55 (~0xAA) waiting on 0x22A (with 0x22E bit 7 set).
        assert_eq!(gate1_sb_io(0x22E, 0, 0xABCD_0000), 0xABCD_0080);
        assert_eq!(gate1_sb_io(0x22A, 0, 0xABCD_0000), 0xABCD_0055);
    }

    #[test]
    fn opl3_bank1_enable_and_stereo_panning_renders_right_only() {
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        // Enable OPL3 mode via Bank 1 reg 0x05 = 0x01
        gate1_sb_io(0x38A, IO_FLAG_OUTPUT, 0x05);
        gate1_sb_io(0x38B, IO_FLAG_OUTPUT, 0x01);
        // Key-on Bank 1 channel 0 panned RIGHT only (C0 = 0x21: right bit 5 + additive bit 0)
        // using OPL3 square waveform (ws = 6)
        for &(r, v) in &[
            (0x20u8, 0x21u8),
            (0x40, 0x10),
            (0x60, 0xF0),
            (0x80, 0x0F),
            (0xE0, 0x06),
            (0x23, 0x21),
            (0x43, 0x00),
            (0x63, 0xF0),
            (0x83, 0x0F),
            (0xE3, 0x06),
            (0xC0, 0x21), // RIGHT only
            (0xA0, 0x98),
            (0xB0, 0x31), // key-on
        ] {
            gate1_sb_io(0x38A, IO_FLAG_OUTPUT, r as u32);
            gate1_sb_io(0x38B, IO_FLAG_OUTPUT, v as u32);
        }
        let mut ring = [0u8; 512];
        render_into_ring(&mut ring, 0, 128);
        let mut peak_l = 0i32;
        let mut peak_r = 0i32;
        for f in 0..128 {
            let l = i16::from_le_bytes([ring[f * 4], ring[f * 4 + 1]]) as i32;
            let r = i16::from_le_bytes([ring[f * 4 + 2], ring[f * 4 + 3]]) as i32;
            peak_l = peak_l.max(l.abs());
            peak_r = peak_r.max(r.abs());
        }
        assert_eq!(peak_l, 0, "left channel must stay silent when panned right-only");
        assert!(peak_r > 200, "right channel must render OPL3 Bank 1 audio, peak_r={peak_r}");
    }

    #[test]
    fn mpu401_uart_handshake_and_hw_midi_voice_allocation() {
        let _sb_test_guard = TEST_LOCK.lock().unwrap();
        reset_all();
        // Reset (0xFF) -> status bit 7 clear (RX ready) -> data 0xFE ACK
        gate1_sb_io(0x331, IO_FLAG_OUTPUT, 0xFF);
        assert_eq!(gate1_sb_io(0x331, 0, 0) & 0x80, 0);
        assert_eq!(gate1_sb_io(0x330, 0, 0) & 0xFF, 0xFE);
        assert_eq!(gate1_sb_io(0x331, 0, 0) & 0x80, 0x80);

        // Enter UART mode (0x3F) -> 0xFE ACK
        gate1_sb_io(0x331, IO_FLAG_OUTPUT, 0x3F);
        assert_eq!(gate1_sb_io(0x330, 0, 0) & 0xFF, 0xFE);

        // Program Change (Piano = 0) + Note On A4 (69, vel 100) + Note Off
        gate1_sb_io(0x330, IO_FLAG_OUTPUT, 0xC0);
        gate1_sb_io(0x330, IO_FLAG_OUTPUT, 0x00);
        gate1_sb_io(0x330, IO_FLAG_OUTPUT, 0x90);
        gate1_sb_io(0x330, IO_FLAG_OUTPUT, 69);
        gate1_sb_io(0x330, IO_FLAG_OUTPUT, 100);
        {
            let st = unsafe { &*SB_OPL.0.get() };
            assert!(st.mpu.voices[0].active);
            assert_eq!(st.mpu.voices[0].note, 69);
            assert_eq!(st.mpu.notes_on_total, 1);
        }
        // Running-status Note Off (vel=0)
        gate1_sb_io(0x330, IO_FLAG_OUTPUT, 69);
        gate1_sb_io(0x330, IO_FLAG_OUTPUT, 0);
        {
            let st = unsafe { &*SB_OPL.0.get() };
            assert!(!st.mpu.voices[0].active);
        }
        // Exact M5451 DELTA check for A4 (440 Hz): ~9612
        let d_a4 = midi_note_to_m5451_delta(69, 0);
        assert!((9500..=9750).contains(&d_a4), "A4 DELTA expected ~9612, got {d_a4}");
    }
}
