//! The FL 3.5 / FL 6 TS404 voice. Same oscillators, filter, envelope shape, LFO and
//! distortion as FL 2's (`engine.rs`), but driven by FL's channel engine: it gets a
//! pitch in cents and cutoff/resonance as 0..1 values once per block, and has no step
//! gate or step slides (those are the channel's job, see `channel3.rs`).
//!
//! Field names follow FL's object; every f32 is rounded where FL stores one.

use crate::tables::{Tables, Wave};
use crate::tables3::{DIST3_LEN, Fl3, Tables3, log_scale3, power3};
use crate::x87::{Ext, e, ei};

pub mod idx3 {
    //! Parameter indices: FL 2's (`engine::idx`) for 1..=33.
    pub use crate::engine::idx::{
        ATTACK, CUTOFF, DECAY, DIST_AMOUNT, DIST_THRES, DIST_TYPE, ENV_AMT, FILTER_TYPE, FM, LFO_AMT, LFO_SHAPE,
        LFO_SPEED, LFO_TARGET, OSC_MIX, OSC1_COARSE, OSC1_FINE, OSC1_PW, OSC1_SHAPE, OSC2_COARSE, OSC2_FINE,
        OSC2_PW, OSC2_SHAPE, RELEASE, RESO_INV, RING, SUSTAIN_LEVEL, SUSTAIN_TIME, SYNC,
    };
    /// Parameters 1..=33 (the TTS404's parameter block).
    pub const PARAM_COUNT3: usize = 33;
}

mod k {
    use crate::x87::Ext;
    pub const C0_1: Ext = Ext::from_bits(0xcccc_cccc_cccc_cccd, 0x3ffb);
    pub const C0_00029: Ext = Ext::from_bits(0x980b_2420_70b8_c800, 0x3ff3);
    pub const C0_9997: Ext = Ext::from_bits(0xffec_56d5_cfaa_cd9f, 0x3ffe);
    pub const C0_005: Ext = Ext::from_bits(0xa3d7_0a3d_70a3_d70a, 0x3ff7);
    pub const C6_99E_6: Ext = Ext::from_bits(0xea8b_a48e_6a3b_87ae, 0x3fed);
    pub const C0_99999: Ext = Ext::from_bits(0xffff_583a_53b8_e4b8, 0x3ffe);
    pub const C0_07: Ext = Ext::from_bits(0x8f5c_28f5_c28f_5c29, 0x3ffb);
    pub const LFO_BASE_INC: Ext = Ext::from_bits(0xbe37_c63a_8f8b_b6e7, 0x400f);
    pub const Q_LP12: f32 = 8.544_921_875;
    pub const Q_LP24: f32 = 9.033_203_125;
    /// Release floor: below this the envelope ends (FL 2: ~6e-8).
    pub const ENV_FLOOR: f32 = f32::from_bits(0x38d1_b717);
    /// Cutoff ramp: FL 3.5 limits a block's per-sample step to |change| · 100/44100
    /// once the change exceeds 5e-5; FL 6 caps it at 0.001 · 16384 per sample.
    pub const RAMP_MIN_35: f32 = f32::from_bits(0x3851_b717);
    pub const RAMP_REL_35: f32 = f32::from_bits(0x3b14_9b93);
    pub const RAMP_MAX_6: f32 = f32::from_bits(0x3a83_126f);
    pub const RAMP_SNAP: f32 = f32::from_bits(0x3380_0001);
}

/// Linear ramp of a smoothed value to `target` over `n` samples, FL 3.5 / FL 6 style.
/// Returns (start, per-sample step) and leaves the end value in `state`.
fn ramp(v: Fl3, state: &mut f32, target: f32, n: i32) -> (f32, f32) {
    let start = *state;
    let mut inc = e(target).sub(e(start)).to_f32();
    if inc.to_bits() == 0 {
        return (start, inc);
    }
    *state = target;
    let diff_abs = e(inc).abs().to_f32();
    inc = e(inc).div(ei(n)).to_f32();
    let limit = match v {
        Fl3::V35 => {
            if e(diff_abs).le(e(k::RAMP_MIN_35)) || diff_abs.is_nan() {
                return (start, inc);
            }
            let lim = e(diff_abs).mul(e(k::RAMP_REL_35)).to_f32();
            if !e(inc).abs().gt(e(lim)) {
                return (start, inc);
            }
            lim
        }
        Fl3::V6 => {
            let max = e(k::RAMP_MAX_6).mul(e(16384.0)).to_f32();
            if !e(f32::from_bits(inc.to_bits() & 0x7fff_ffff)).gt(e(max)) {
                return (start, inc);
            }
            max
        }
    };
    inc = f32::from_bits((inc.to_bits() & 0x8000_0000) | (limit.to_bits() & 0x7fff_ffff));
    *state = ei(n).mul(e(inc)).add(e(start)).to_f32();
    let small = match v {
        Fl3::V35 => e(*state).abs().lt(e(k::RAMP_SNAP)),
        Fl3::V6 => ((state.to_bits() & 0x7fff_ffff) as i32) < k::RAMP_SNAP.to_bits() as i32,
    };
    if small {
        *state = 0.0;
        inc = e(start).neg().div(ei(n)).to_f32();
    }
    (start, inc)
}

#[derive(Clone)]
pub struct Engine3 {
    pub version: Fl3,
    /// Parameters, indexed 1..=33 like FL 2's (`p[0]` unused).
    pub p: [i32; 34],
    /// Voice values FL copies in once per tick: pan and volume (read by the channel
    /// mixer), pitch, cutoff and resonance.
    pub pan_in: i32,
    pub vol_in: f32,
    pub pitch: f32,
    pub pitch_i: i32,
    pub cut: f32,
    pub res: f32,
    pub cut_smooth: f32,
    pub counter: i32,
    pub phase1: u32,
    pub phase2: u32,
    pub prev1: u32,
    pub prev2: u32,
    pub env: f32,
    pub env_state: i32,
    pub lfo_phase: u32,
    pub hold: i32,
    /// x1 x2 y1 y2 (stage 1), x1 x2 y1 y2 (stage 2).
    pub filt: [f32; 8],
}

pub const ENV_IDLE: i32 = 4;

impl Engine3 {
    pub fn new(version: Fl3) -> Engine3 {
        Engine3 {
            version,
            p: [0; 34],
            pan_in: 0,
            vol_in: 0.0,
            pitch: 0.0,
            pitch_i: 0,
            cut: 0.0,
            res: 0.0,
            cut_smooth: -1.0,
            counter: 0,
            phase1: 0,
            phase2: 0,
            prev1: 0,
            prev2: 0,
            env: 0.0,
            env_state: ENV_IDLE,
            lfo_phase: 0,
            hold: 0,
            filt: [0.0; 8],
        }
    }

    /// Set the pitch in cents (FL 3.5 keeps it as an integer, FL 6 as a float).
    pub fn set_pitch(&mut self, cents: f32) {
        self.pitch = cents;
        self.pitch_i = cents as i32;
    }

    /// FL's note start: the envelope restarts only from release or idle.
    pub fn note_start(&mut self) {
        if self.env_state > 2 {
            self.env_state = 0;
            self.hold = 0;
        }
        self.counter = 0;
    }

    /// What FL's voice release does to the synth.
    pub fn note_off(&mut self) {
        if self.env_state < 3 {
            self.env_state = 3;
        }
    }

    pub fn is_idle(&self) -> bool {
        self.env_state >= ENV_IDLE
    }

    /// Render `out.len()` samples. `aa`: FL's "alias-free TS404" (export option);
    /// `hq`: FL 6's high-quality mode (interpolated distortion).
    pub fn render(&mut self, t: &Tables, t3: &Tables3, out: &mut [f32], aa: bool, hq: bool, custom: Option<&Wave>) {
        let n = out.len() as i32;
        if n <= 0 {
            return;
        }
        let v = self.version;
        let p = self.p;
        let c1 = p[1].wrapping_mul(100);
        let c2 = p[2].wrapping_mul(100);
        let ring = ei(p[28]).mul(e(0.001_953_125)).to_f32();
        let half_minus_ring = e(0.5).sub(e(ring)).to_f32();
        let mix2 = ei(p[12]).mul(e(half_minus_ring)).mul(e(0.003_906_25)).to_f32();
        let mix1 = e(half_minus_ring).sub(e(mix2)).to_f32();
        let env_amt = ei(p[18].wrapping_shl(7)).to_f32();
        let fm = ei(p[27]).mul(e(0.003_906_25)).to_f32();
        let mut lfo_amt = ei(p[19]).mul(e(0.003_906_25)).to_f32();
        let lfo_tab: &Wave = &t.lfo[p[22].clamp(0, 3) as usize];
        let wave_for = |shape: i32| -> &Wave {
            if (0..4).contains(&shape) {
                &t.osc[shape as usize]
            } else {
                custom.unwrap_or(&t.osc[0])
            }
        };
        let wave1 = wave_for(p[9]);
        let wave2 = wave_for(p[10]);

        // cutoff (0..16384 for modX 0..1), smoothed across the block
        let mut c = e(self.cut).mul(e(16384.0)).to_f32();
        if !e(c).ge(e(0.0)) {
            c = 0.0;
        }
        if self.cut_smooth.to_bits() == 0xbf80_0000 {
            self.cut_smooth = c;
        }
        let (start, cut_inc) = ramp(v, &mut self.cut_smooth, c, n);
        let mut c = start;

        // resonance
        let mut q = e(1.0).sub(e(self.res)).mul(e(8.0)).to_f32();
        if !e(q).ge(e(0.0)) {
            q = 0.0;
        } else if e(q).gt(e(8.0)) {
            q = 8.0;
        }
        let k_for = |q: f32, ftype: i32, k: f32| match ftype {
            0 | 2 => e(1.0).div(e(k::Q_LP12).sub(e(q))).to_f32(),
            1 => e(1.0).div(e(k::Q_LP24).sub(e(q))).to_f32(),
            _ => k,
        };
        let mut kq = k_for(q, p[11], 0.0);

        // pitch: cents from C0, folded into C1..B7 by octaves
        let mut idx = match v {
            Fl3::V35 => self.pitch_i.wrapping_add(6000),
            Fl3::V6 => e(self.pitch).add(e(6000.0)).round_i64() as i32,
        };
        if idx < 1200 {
            idx = idx.wrapping_add(0x4b000) % 1200 + 1200;
        } else if idx > 0x257f {
            idx = idx % 1200 + 0x20d0;
        }
        let pitch = |i: i32| t3.pitch[i.clamp(0, t3.pitch.len() as i32 - 1) as usize];
        let base1 = pitch(idx.wrapping_add(c1));
        let base2 = pitch(idx.wrapping_add(c2));

        let mut pw1 = (p[5] as u32).wrapping_shl(23);
        let pw2 = (p[6] as u32).wrapping_shl(23);
        let pw1_base = pw1 >> 8;

        // envelope and LFO rates
        let ratio = t3.rate_ratio;
        let attack = crate::fpu::exp(ei((-25i32).wrapping_sub(p[29])).mul(k::C0_1)).mul(e(0.5)).to_f32();
        let decay = power3(ei(p[30]).mul(k::C0_00029).mul(e(0.01)).add(k::C0_9997).to_f32(), ratio);
        let hold_max = if p[32] >= 100 { i32::MAX } else { t3.rate.wrapping_mul(p[32]) / 10 };
        let sustain = ei(p[33]).mul(k::C0_005).to_f32();
        let g = log_scale3(1000.0, e(1.0).sub(ei(p[31]).mul(e(0.01))).to_f32());
        let release = power3(k::C0_99999.sub(e(g).mul(k::C6_99E_6)).to_f32(), ratio);
        let lfo_inc = crate::fpu::exp(ei(p[20].wrapping_sub(50)).mul(k::C0_07)).mul(k::LFO_BASE_INC).mul(e(ratio)).round_i64() as i32;
        match p[21] {
            0 => lfo_amt = e(lfo_amt).mul(e(600.0)).to_f32(),
            1 => lfo_amt = e(lfo_amt).mul(e(q)).to_f32(),
            3 => lfo_amt = e(lfo_amt).mul(e(16_777_216.0)).to_f32(),
            _ => {}
        }

        let [mut x1, mut x2, mut y1, mut y2, mut x1b, mut x2b, mut y1b, mut y2b] = self.filt;
        let mut env = self.env;
        let mut state = self.env_state;
        let mut y_prev = 0f32;
        self.counter = self.counter.wrapping_add(n);

        // box-filtered wavetable read between the previous and current phase
        let aa_read = |wave: &Wave, prev: u32, cur: u32| -> f32 {
            let mut acc = 0f32;
            let mut count = 0i32;
            let mut x = cur;
            let mut wrapped = prev >= cur;
            loop {
                if !(prev < x || wrapped) {
                    break;
                }
                count += 1;
                acc = e(acc).add(e(wave[(x >> 18) as usize])).to_f32();
                let old = x;
                x = x.wrapping_sub(0x40_0000);
                if old < x {
                    wrapped = false;
                    if old >= prev {
                        break;
                    }
                }
            }
            e(acc).div(ei(count)).to_f32()
        };

        for o in out.iter_mut() {
            self.lfo_phase = self.lfo_phase.wrapping_add(lfo_inc as u32);
            let lfo = lfo_tab[(self.lfo_phase >> 18) as usize];
            let (mut inc1, mut inc2);
            if p[21] == 0 && e(lfo).cmp(Ext::ZERO).is_some_and(|o| o != core::cmp::Ordering::Equal) {
                let d = (e(lfo).mul(e(lfo_amt)).round_i64() as i32).wrapping_add(idx);
                inc1 = pitch(c1.wrapping_add(d).max(0));
                inc2 = pitch(c2.wrapping_add(d).max(0));
            } else {
                inc1 = base1;
                inc2 = base2;
            }
            inc1 = inc1.wrapping_add((((inc1 as u32) >> 8) as i32).wrapping_mul(p[3]) >> 5);
            inc2 = inc2.wrapping_add((((inc2 as u32) >> 8) as i32).wrapping_mul(p[4]) >> 5);
            if p[21] == 3 {
                let w = (e(lfo).mul(e(lfo_amt)).round_i64() as i32).wrapping_add(pw1_base as i32);
                pw1 = if w < 0 { 0 } else { (w as u32).wrapping_shl(8) };
            }

            // oscillators
            self.phase1 = self.phase1.wrapping_add(inc1 as u32);
            let r1 = if self.phase1 >= pw1 { self.phase1 - pw1 } else { self.phase1 };
            let s1 = if aa {
                let s = aa_read(wave1, self.prev1, r1);
                self.prev1 = r1;
                s
            } else {
                wave1[(r1 >> 18) as usize]
            };
            let step2 = if fm.to_bits() != 0 {
                ei(inc2).mul(e(fm)).mul(e(s1)).round_i32().wrapping_add(inc2) as u32
            } else {
                inc2 as u32
            };
            let (np2, carry) = self.phase2.overflowing_add(step2);
            let sync_on = match v {
                Fl3::V35 => p[7] & 0xffff != 0,
                Fl3::V6 => p[7] != 0,
            };
            if carry && sync_on {
                self.phase1 = 0;
            }
            self.phase2 = np2;
            let r2 = if np2 >= pw2 { np2 - pw2 } else { np2 };
            let s2 = if aa {
                let s = aa_read(wave2, self.prev2, r2);
                self.prev2 = r2;
                s
            } else {
                wave2[(r2 >> 18) as usize]
            };

            if p[21] == 1 {
                let mut q2 = e(lfo).mul(e(lfo_amt)).add(e(q)).to_f32();
                if e(q2).gt(e(8.0)) {
                    q2 = 8.0;
                }
                kq = k_for(q2, p[11], kq);
            }
            let x = e(s1).mul(e(s2)).mul(e(ring)).add(e(mix1).mul(e(s1))).add(e(mix2).mul(e(s2))).to_f32();

            // cutoff index
            let raw = if p[21] != 2 {
                e(env_amt).mul(e(env)).add(e(c)).round_i64()
            } else {
                e(lfo).mul(e(lfo_amt)).add(e(1.0)).mul(e(c)).add(e(env_amt).mul(e(env))).round_i64()
            };
            let ci = (((raw as u64) >> 7) as i32).clamp(0, 255) as usize;
            c = e(c).add(e(cut_inc)).to_f32();
            let (sn, cs) = (t3.cut_sin[ci], t3.cut_cos[ci]);

            // filter
            let y = match p[11] as u32 {
                0 | 2 => {
                    let alpha = e(sn).mul(e(kq)).to_f32();
                    let (b0, b1) = if p[11] == 0 {
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
                    let alpha = e(sn).mul(e(kq)).to_f32();
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
                    let y = e(s).mul(e(x)).sub(e(s).mul(e(x2))).sub(e(a1).mul(e(y1))).sub(e(a2).mul(e(y2)))
                        .div(e(a0)).to_f32();
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

            // envelope
            if state < ENV_IDLE {
                let yo = e(y).mul(e(env)).to_f32();
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
                        }
                    }
                    2 => {
                        if self.hold < hold_max {
                            self.hold += 1;
                        } else {
                            state = 3;
                        }
                    }
                    3 => {
                        if e(env).ge(e(k::ENV_FLOOR)) {
                            env = e(env).mul(e(release)).to_f32();
                        } else {
                            env = 0.0;
                            state = ENV_IDLE;
                        }
                    }
                    _ => {}
                }
                *o = yo;
            } else {
                *o = 0.0;
            }
        }
        self.filt = [x1, x2, y1, y2, x1b, x2b, y1b, y2b];
        self.env = env;
        self.env_state = state;

        // distortion, over the block
        if p[15] != 0 {
            let wet = ei(p[14]).mul(e(0.007_812_5)).to_f32();
            if wet.to_bits() != 0 {
                let dry = e(1.0).sub(e(wet)).to_f32();
                let tab = &t3.dist[(p[13] & 1) as usize][(p[15].clamp(1, 10) - 1) as usize];
                distort(v, hq, tab, out, wet, dry);
            }
        }
    }
}

/// FL 3.5's / FL 6's block distortion: y = x·dry + curve(|x|)·sign(x)·wet/32768.
fn distort(v: Fl3, hq: bool, tab: &[i16; DIST3_LEN], out: &mut [f32], wet: f32, dry: f32) {
    let w = e(wet).mul(e(1.0 / 32768.0));
    let index = |k: i32| (k.unsigned_abs() as usize).min(0x1fff);
    match (v, hq) {
        (Fl3::V35, _) | (Fl3::V6, false) => {
            let scale = if v == Fl3::V35 { e(8192.0) } else { e(8191.0) };
            for x in out.iter_mut() {
                let k = e(*x).mul(scale).round_i32();
                let s = ei(tab[index(k)] as i32);
                let s = if k < 0 { s.neg() } else { s };
                *x = e(*x).mul(e(dry)).add(s.mul(w)).to_f32();
            }
        }
        (Fl3::V6, true) => {
            // FL 6 runs this loop with the FPU set to truncate.
            for x in out.iter_mut() {
                let xk = e(*x).mul(e(8191.0)); // exact
                let k = xk.trunc_i64() as i32;
                let frac = xk.sub(ei(k)); // exact
                let xd = e(*x).mul(e(dry)); // exact (48 bits)
                let i = index(k);
                let (t0, t1) = (ei(tab[i] as i32), ei(tab[i + 1] as i32));
                let interp = if k >= 0 {
                    t0.add_t(t1.sub_t(t0).mul_t(frac))
                } else {
                    t0.sub_t(t1.sub_t(t0).mul_t(frac)).neg()
                };
                *x = xd.add_t(interp.mul_t(w)).to_f32_t();
            }
        }
    }
}
