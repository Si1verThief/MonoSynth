//! The voice: two wavetable oscillators (with pulse-width warp, FM, ring mod and sync),
//! a resonant biquad filter (LP12 / LP24 / HP / BP), one envelope shared by amp and
//! filter, an LFO, and table- or curve-based distortion.
//!
//! State and parameters live in one 64-int struct (`Engine::p`, float state stored as
//! bits). Every f32 is rounded at a fixed point and the arithmetic between those points
//! runs in extended precision, so output is identical on every platform.

use crate::tables::{DistCurve, Tables, Wave};
use crate::x87::{Ext, e, ei};

/// Engine struct indices.
pub mod idx {
    pub const GATE_SAMPLES: usize = 0;
    pub const OSC1_COARSE: usize = 1;
    pub const OSC2_COARSE: usize = 2;
    pub const OSC1_FINE: usize = 3;
    pub const OSC2_FINE: usize = 4;
    pub const OSC1_PW: usize = 5;
    pub const OSC2_PW: usize = 6;
    pub const SYNC: usize = 7;
    pub const OSC1_SHAPE: usize = 9;
    pub const OSC2_SHAPE: usize = 10;
    pub const FILTER_TYPE: usize = 11;
    pub const OSC_MIX: usize = 12;
    pub const DIST_TYPE: usize = 13;
    pub const DIST_AMOUNT: usize = 14;
    pub const DIST_THRES: usize = 15;
    pub const CUTOFF: usize = 16;
    /// Stored inverted: 128 - knob.
    pub const RESO_INV: usize = 17;
    pub const ENV_AMT: usize = 18;
    pub const LFO_AMT: usize = 19;
    pub const LFO_SPEED: usize = 20;
    pub const LFO_TARGET: usize = 21;
    pub const LFO_SHAPE: usize = 22;
    /// TS404 "DELAY" amount (send to the delay line), 0..=128.
    pub const DELAY_AMT: usize = 35;
    pub const FM: usize = 27;
    pub const RING: usize = 28;
    pub const ATTACK: usize = 29;
    pub const DECAY: usize = 30;
    pub const RELEASE: usize = 31;
    pub const SUSTAIN_TIME: usize = 32;
    pub const SUSTAIN_LEVEL: usize = 33;
    pub const STEP_GATE: usize = 34;
    /// Non-zero: shapes >= 4 read the user wave instead of the saw.
    pub const CUSTOM_WAVE: usize = 0x24;
    // Per-step handshake (see steps.rs).
    pub const PREV_FLAGS: usize = 0x27;
    pub const CUT_CUR: usize = 0x28;
    pub const NOTE_CUR: usize = 0x29;
    pub const FLAGS_CUR: usize = 0x2a;
    pub const CUT_NEXT: usize = 0x2b;
    pub const NOTE_NEXT: usize = 0x2c;
    pub const FLAGS_NEXT: usize = 0x2d;
    // Saved voice state.
    pub const PHASE1: usize = 0x2e;
    pub const PHASE2: usize = 0x2f;
    pub const ENV: usize = 0x30;
    pub const ENV_STATE: usize = 0x31;
    pub const LFO_PHASE: usize = 0x34;
    pub const HOLD: usize = 0x36;
    pub const FILT: usize = 0x37; // 8 f32: x1 x2 y1 y2 (stage 1), x1 x2 y1 y2 (stage 2)

    pub const PARAM_COUNT: usize = 35;
}

pub const FLAG_GATE: i32 = 0x01;
pub const FLAG_SLIDE: i32 = 0x40;
pub const ENV_DONE: i32 = 4;

mod k {
    use crate::x87::Ext;
    pub const C0_1: Ext = Ext::from_bits(0xcccc_cccc_cccc_cccd, 0x3ffb);
    pub const C0_01: Ext = Ext::from_bits(0xa3d7_0a3d_70a3_d70a, 0x3ff8);
    pub const C0_005: Ext = Ext::from_bits(0xa3d7_0a3d_70a3_d70a, 0x3ff7);
    pub const C7E_5: Ext = Ext::from_bits(0x92cc_f6be_37de_939f, 0x3ff1);
    pub const C0_07: Ext = Ext::from_bits(0x8f5c_28f5_c28f_5c29, 0x3ffb);
    pub const LFO_BASE_INC: Ext = Ext::from_bits(0xbe37_c63a_8f8b_b6e7, 0x400f);
    pub const D0_9997: f32 = f32::from_bits(0x3f7f_ec57);
    pub const ENV_FLOOR: f32 = f32::from_bits(0x3380_0001);
    pub const Q_LP12: f32 = 8.544_921_875;
    pub const Q_LP24: f32 = 9.033_203_125;
}

#[inline]
fn f(bits: i32) -> f32 {
    f32::from_bits(bits as u32)
}
#[inline]
fn b(v: f32) -> i32 {
    v.to_bits() as i32
}

/// (a * b) / c with a 64-bit intermediate, truncating.
#[inline]
fn muldiv(a: i32, b: i32, c: i32) -> i32 {
    ((a as i64 * b as i64) / c as i64) as i32
}

#[derive(Clone)]
pub struct Engine {
    pub p: [i32; 64],
}

impl Default for Engine {
    fn default() -> Self {
        Engine { p: [0; 64] }
    }
}

impl Engine {
    /// Load parameters 1..=35 (the layout of a .404 preset file).
    pub fn set_params(&mut self, params: &[i32; idx::PARAM_COUNT]) {
        self.p[1..=idx::PARAM_COUNT].copy_from_slice(params);
    }

    pub fn params(&self) -> [i32; idx::PARAM_COUNT] {
        self.p[1..=idx::PARAM_COUNT].try_into().unwrap()
    }

    pub fn env_done(&self) -> bool {
        self.p[idx::ENV_STATE] as u32 >= ENV_DONE as u32
    }

    /// Render `out.len()` samples. `pos` is the sample position within the current step
    /// at the start of this block, `sps` the step length in samples, `hq` selects
    /// per-sample distortion instead of the curve tables.
    /// `custom` is the user wave used by shapes >= 4 when `p[CUSTOM_WAVE]` is non-zero.
    pub fn render(&mut self, t: &Tables, out: &mut [f32], pos: i32, sps: i32, hq: bool, custom: Option<&Wave>) {
        use idx::*;
        let p = &mut self.p;
        p[GATE_SAMPLES] = p[STEP_GATE].wrapping_mul(sps) >> 8; // gate point within the step

        let c1 = p[OSC1_COARSE] << 5;
        let c2 = p[OSC2_COARSE] << 5;
        let fine1 = p[OSC1_FINE];
        let fine2 = p[OSC2_FINE];
        let ring = ei(p[RING]).mul(e(1.0 / 512.0)).to_f32();
        let half_minus_ring = e(0.5).sub(e(ring)).to_f32();
        let mix2 = ei(p[OSC_MIX]).mul(e(half_minus_ring)).mul(e(1.0 / 256.0)).to_f32();
        let mix1 = e(half_minus_ring).sub(e(mix2)).to_f32();
        let ftype = p[FILTER_TYPE] as u32;
        let reso_base = ei(p[RESO_INV]).mul(e(0.0625)).to_f32();
        let env_amt = ei(p[ENV_AMT] << 7).to_f32();
        let fm = ei(p[FM]).mul(e(1.0 / 256.0)).to_f32();
        let target = p[LFO_TARGET];
        let mut lfo_amt = ei(p[LFO_AMT]).mul(e(1.0 / 256.0)).to_f32();
        let mut lfo_phase = p[LFO_PHASE] as u32;
        let lfo_tab: &Wave = &t.lfo[p[LFO_SHAPE].clamp(0, 3) as usize];
        let wave_for = |shape: i32| -> &Wave {
            if (0..4).contains(&shape) {
                &t.osc[shape as usize]
            } else {
                match custom {
                    Some(w) if p[CUSTOM_WAVE] != 0 => w,
                    _ => &t.osc[0],
                }
            }
        };
        let wave1 = wave_for(p[OSC1_SHAPE]);
        let wave2 = wave_for(p[OSC2_SHAPE]);
        let mut phase1 = p[PHASE1] as u32;
        let mut phase2 = p[PHASE2] as u32;
        let mut hold = p[HOLD];
        let cut_base = (p[CUTOFF] << 7) + 1;
        let cut_start = (p[CUT_CUR] + cut_base).max(0);
        let cut_delta = (p[CUT_NEXT] + cut_base).max(0) - cut_start;
        let note = p[NOTE_CUR];
        let prev_slide = if p[PREV_FLAGS] & FLAG_SLIDE != 0 { 0 } else { -1 };
        let slide_to = if p[FLAGS_CUR] & FLAG_SLIDE != 0 { p[NOTE_NEXT] } else { -1 };
        let mut fs = [0f32; 8];
        for (i, v) in fs.iter_mut().enumerate() {
            *v = f(p[FILT + i]);
        }
        let [mut x1, mut x2, mut y1, mut y2, mut x1b, mut x2b, mut y1b, mut y2b] = fs;
        let mut env = f(p[ENV]);
        let mut state = p[ENV_STATE];
        let sync = p[SYNC];

        let dist_thr = p[DIST_THRES];
        let (mut dry, wet, dist_tab): (f32, f32, Option<&[i16; 8192]>) = if dist_thr == 0 || hq {
            (0.0, 0.0, None)
        } else {
            let a = ei(p[DIST_AMOUNT]).mul(e(1.0 / 128.0)).to_f32();
            let wet = e(a).mul(e(1.0 / 32768.0)).to_f32();
            let ty = (p[DIST_TYPE] & 1) as usize;
            let tab = (b(wet) != 0).then(|| &t.dist[ty][(dist_thr.clamp(1, 10) - 1) as usize]);
            (e(1.0).sub(e(a)).to_f32(), wet, tab)
        };
        let mut pw1 = (p[OSC1_PW] << 23) as u32;
        let pw2 = (p[OSC2_PW] << 23) as u32;
        let pw1_base = pw1 >> 8;

        let attack = crate::fpu::exp(ei(-25 - p[ATTACK]).mul(k::C0_1)).mul(e(0.5)).to_f32();
        let decay = e(1.0).sub(e(k::D0_9997)).mul(ei(p[DECAY])).mul(k::C0_01).add(e(k::D0_9997)).to_f32();
        let hold_max = if p[SUSTAIN_TIME] == 100 { i32::MAX } else { ei(p[SUSTAIN_TIME]).mul(e(4410.0)).round_i64() as i32 };
        let sustain = ei(p[SUSTAIN_LEVEL]).mul(k::C0_005).to_f32();
        let release = crate::fpu::exp(ei(p[RELEASE] - 100).mul(k::C7E_5)).to_f32();
        let lfo_inc = crate::fpu::exp(ei(p[LFO_SPEED] - 50).mul(k::C0_07)).mul(k::LFO_BASE_INC).round_i64() as u32;

        if pos == 0 && prev_slide < 0 && p[FLAGS_CUR] & FLAG_GATE != 0 {
            state = 0; // retrigger
        }
        let gate_end = if slide_to < 0 { p[GATE_SAMPLES] } else { i32::MAX };
        match target {
            0 => lfo_amt = e(lfo_amt).mul(e(192.0)).to_f32(),
            1 => lfo_amt = e(lfo_amt).mul(e(reso_base)).to_f32(),
            3 => lfo_amt = e(lfo_amt).mul(e(16_777_216.0)).to_f32(),
            _ => {}
        }

        let pitch = |i: i32| t.pitch[i.clamp(0, crate::tables::PITCH_LEN as i32 - 1) as usize];
        let mut y_prev = 0f32; // kept for an out-of-range filter type
        for (i, o) in out.iter_mut().enumerate() {
            let spos = pos + i as i32;
            lfo_phase = lfo_phase.wrapping_add(lfo_inc);
            let lfo = lfo_tab[(lfo_phase >> 18) as usize];

            // --- pitch
            let mut esi = note;
            let (mut inc1, mut inc2);
            if target == 0 && lfo != 0.0 {
                let d = e(lfo).mul(e(lfo_amt)).round_i64() as i32;
                esi = (note + c1 + d).max(0);
                inc1 = pitch(esi);
                inc2 = pitch((note + c2 + d).max(0));
            } else {
                inc1 = pitch(c1 + esi);
                inc2 = pitch(c2 + esi);
            }
            if slide_to >= 0 && esi != slide_to {
                inc1 = inc1.wrapping_add(muldiv(pitch(slide_to + c1).wrapping_sub(inc1), spos, sps));
                inc2 = inc2.wrapping_add(muldiv(pitch(slide_to + c2).wrapping_sub(inc2), spos, sps));
            }
            inc1 = inc1.wrapping_add((((inc1 as u32) >> 8) as i32).wrapping_mul(fine1) >> 5);
            inc2 = inc2.wrapping_add((((inc2 as u32) >> 8) as i32).wrapping_mul(fine2) >> 5);
            if target == 3 {
                let v = (e(lfo).mul(e(lfo_amt)).round_i64() as i32).wrapping_add(pw1_base as i32);
                pw1 = if v < 0 { 0 } else { (v as u32) << 8 };
            }

            // --- oscillators
            phase1 = phase1.wrapping_add(inc1 as u32);
            let r1 = if phase1 >= pw1 { phase1 - pw1 } else { phase1 };
            let s1 = wave1[(r1 >> 18) as usize];
            let step2 = if b(fm) != 0 {
                let t = ei(inc2).mul(e(fm)).mul(e(s1)).round_i32();
                t.wrapping_add(inc2) as u32
            } else {
                inc2 as u32
            };
            let (np2, carry) = phase2.overflowing_add(step2);
            if carry && sync & 0xffff != 0 {
                phase1 = 0;
            }
            phase2 = np2;
            let r2 = if phase2 >= pw2 { phase2 - pw2 } else { phase2 };
            let s2 = wave2[(r2 >> 18) as usize];

            let mut q = reso_base;
            if target == 1 {
                q = e(lfo).mul(e(lfo_amt)).add(e(reso_base)).to_f32();
                if e(q).gt(e(8.0)) {
                    q = 8.0;
                }
            }
            let x = e(s1).mul(e(s2)).mul(e(ring)).add(e(mix1).mul(e(s1))).add(e(mix2).mul(e(s2))).to_f32();

            // --- cutoff index
            let mut c = cut_start;
            if cut_delta != 0 {
                // 32-bit product, truncating division
                c += cut_delta.wrapping_mul(spos) / sps;
            }
            let r = if target != 2 {
                e(env_amt).mul(e(env)).round_i64()
            } else {
                e(lfo).mul(e(lfo_amt)).mul(ei(c)).add(e(env_amt).mul(e(env))).round_i64()
            };
            let ci = ((c as i64).wrapping_add(r) >> 7) as i32;
            let ci = ci.clamp(0, 255) as usize;
            let (sn, cs) = (t.cut_sin[ci], t.cut_cos[ci]);

            // --- filter (RBJ biquads, coefficients scaled by 1/4)
            let y = match ftype {
                0 | 2 => {
                    let alpha = e(sn).div(e(k::Q_LP12).sub(e(q))).to_f32();
                    let (b0, b1) = if ftype == 0 {
                        let b1 = e(0.25).sub(e(cs)).to_f32();
                        (e(b1).mul(e(0.5)).to_f32(), b1)
                    } else {
                        let b1 = e(0.25).add(e(cs)).to_f32();
                        (e(b1).mul(e(0.5)).to_f32(), -b1)
                    };
                    let a0 = e(0.25).add(e(alpha)).to_f32();
                    let a1 = e(cs).mul(e(-2.0)).to_f32();
                    let a2 = e(0.25).sub(e(alpha)).to_f32();
                    let y = e(b0).mul(e(x)).add(e(b1).mul(e(x1))).add(e(b0).mul(e(x2)))
                        .sub(e(a1).mul(e(y1))).sub(e(a2).mul(e(y2))).div(e(a0)).to_f32();
                    y2 = y1;
                    y1 = y;
                    x2 = x1;
                    x1 = x;
                    y
                }
                1 => {
                    let alpha = e(sn).div(e(k::Q_LP24).sub(e(q))).to_f32();
                    let b1 = e(0.25).sub(e(cs)).to_f32();
                    let b0 = e(b1).mul(e(0.5)).to_f32();
                    let a0i = e(1.0).div(e(0.25).add(e(alpha))).to_f32();
                    let a1 = e(cs).mul(e(-2.0)).to_f32();
                    let a2 = e(0.25).sub(e(alpha)).to_f32();
                    let ys = e(b0).mul(e(x)).add(e(b1).mul(e(x1))).add(e(b0).mul(e(x2)))
                        .sub(e(a1).mul(e(y1))).sub(e(a2).mul(e(y2))).mul(e(a0i)).to_f32();
                    y2 = y1;
                    y1 = ys;
                    x2 = x1;
                    x1 = x;
                    let xs = e(ys).mul(e(0.75)).to_f32();
                    let y = e(b0).mul(e(xs)).add(e(b1).mul(e(x1b))).add(e(b0).mul(e(x2b)))
                        .sub(e(a1).mul(e(y1b))).sub(e(a2).mul(e(y2b))).mul(e(a0i)).to_f32();
                    y2b = y1b;
                    y1b = y;
                    x2b = x1b;
                    x1b = xs;
                    y
                }
                3 => {
                    let s = e(sn).mul(e(0.5)).to_f32();
                    let a0 = e(0.25).add(e(s)).to_f32();
                    let a1 = e(cs).mul(e(-2.0)).to_f32();
                    let a2 = e(0.25).sub(e(s)).to_f32();
                    let y = e(s).mul(e(x)).sub(e(s).mul(e(x2)))
                        .sub(e(a1).mul(e(y1))).sub(e(a2).mul(e(y2))).div(e(a0)).to_f32();
                    y2 = y1;
                    y1 = y;
                    x2 = x1;
                    x1 = x;
                    y
                }
                4 => x,
                _ => y_prev,
            };
            y_prev = y;

            // --- amp envelope, distortion
            if (state as u32) < ENV_DONE as u32 {
                let mut y = e(y).mul(e(env)).to_f32();
                if spos > gate_end {
                    state = 3;
                }
                match state {
                    0 => {
                        env = e(env).add(e(attack)).to_f32();
                        if e(env).ge(e(0.5)) {
                            env = 0.5;
                            state = 1;
                        }
                    }
                    1 => {
                        env = e(env).mul(e(decay)).to_f32();
                        if !e(env).gt(e(sustain)) {
                            state = 2;
                            hold = 0;
                        }
                    }
                    2 => {
                        if hold >= hold_max {
                            state = 3;
                        } else {
                            hold += 1;
                        }
                    }
                    3 => {
                        if e(env).ge(e(k::ENV_FLOOR)) {
                            env = e(env).mul(e(release)).to_f32();
                        } else {
                            env = 0.0;
                            state = ENV_DONE;
                        }
                    }
                    _ => {}
                }
                if let Some(tab) = dist_tab {
                    let k = e(y).mul(e(8191.75)).round_i64() as i32;
                    y = if k >= 0 {
                        let s = tab[k.min(0x1fff) as usize];
                        e(y).mul(e(dry)).add(ei(s as i32).mul(e(wet))).to_f32()
                    } else {
                        let s = tab[k.wrapping_neg().min(0x1fff) as usize];
                        e(y).mul(e(dry)).sub(ei(s as i32).mul(e(wet))).to_f32()
                    };
                }
                *o = y;
            } else {
                *o = 0.0;
            }
        }

        if hq && dist_thr != 0 && p[DIST_AMOUNT] > 0 {
            let amt = ei(p[DIST_AMOUNT]).mul(e(1.0 / 128.0)).to_f32();
            dry = e(1.0).sub(e(amt)).to_f32();
            let curve = DistCurve::new(dist_thr);
            let ty = if p[DIST_TYPE] == 0 { 0 } else { 1 };
            for o in out.iter_mut() {
                let s = *o;
                let x = (s as f64).abs().min(1.0);
                let mut y = curve.shape(ty, x);
                if s < 0.0 {
                    y = -y;
                }
                *o = e(dry).mul(e(s)).add(e(amt).mul(Ext::from_f64(y))).to_f32();
            }
        }

        p[ENV] = b(env);
        p[ENV_STATE] = state;
        p[PHASE1] = phase1 as i32;
        p[PHASE2] = phase2 as i32;
        for (i, v) in [x1, x2, y1, y2, x1b, x2b, y1b, y2b].into_iter().enumerate() {
            p[FILT + i] = b(v);
        }
        p[LFO_PHASE] = lfo_phase as i32;
        p[HOLD] = hold;
    }
}
