//! Tables of the FL 3.5 and FL 6 TS404. The wavetables are FL 2's (`Tables`); pitch is
//! in cents, and the cutoff and distortion tables use FL 3's maths (f32 arguments to
//! its power function, and in FL 6 a distortion curve sampled at i/8191).

use crate::fpu;
use crate::x87::{Ext, e, ei};

/// Pitch table entries: cents from C0.
pub const PITCH3_LEN: usize = 12000;
/// Distortion row length (FL 6's layout: 8192 entries plus a copy of the last one;
/// FL 3.5 only uses the first 8192).
pub const DIST3_LEN: usize = 8194;

/// Which later TS404.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fl3 {
    V35,
    V6,
}

pub(crate) mod k {
    use crate::x87::Ext;
    /// 2^32 · 16.3516 Hz (C0): C0's phase increment times the sample rate.
    pub const C0_TIMES_RATE: Ext = Ext::from_bits(0x82d0_1286_15ca_c8e0, 0x4023);
    pub const TWO_PI: Ext = Ext::from_bits(0xc90f_daa2_2168_c235, 0x4001);
    pub const TEN: Ext = Ext::from_bits(0xa000_0000_0000_0000, 0x4002);
    pub const C0_05: Ext = Ext::from_bits(0xcccc_cccc_cccc_cccd, 0x3ffa);
    pub const C0_1: Ext = Ext::from_bits(0xcccc_cccc_cccc_cccd, 0x3ffb);
    pub const C1_6: Ext = Ext::from_bits(0xcccc_cccc_cccc_cccd, 0x3fff);
    pub const C0_02: Ext = Ext::from_bits(0xa3d7_0a3d_70a3_d70a, 0x3ff9);
    pub const C0_8: Ext = Ext::from_bits(0xcccc_cccc_cccc_cccd, 0x3ffe);
}

/// FL 3's Power(base, exponent) on f32 arguments: exp(ln(base) · exponent), as f32.
pub fn power3(base: f32, exponent: f32) -> f32 {
    if e(exponent).cmp(Ext::ZERO) == Some(core::cmp::Ordering::Equal) {
        return 1.0;
    }
    if e(base).cmp(Ext::ZERO) == Some(core::cmp::Ordering::Equal) && e(exponent).gt(Ext::ZERO) {
        return 0.0;
    }
    fpu::exp(fpu::ln(e(base)).mul(e(exponent))).to_f32()
}

/// FL 3's log-scale curve: (a + 1)^b - 1, as f32.
pub fn log_scale3(a: f32, b: f32) -> f32 {
    fpu::exp(fpu::ln(e(a).add(e(1.0))).mul(e(b))).sub(e(1.0)).to_f32()
}

pub struct Tables3 {
    pub version: Fl3,
    pub rate: i32,
    /// 44100 / rate (the envelope and LFO rate correction).
    pub rate_ratio: f32,
    /// Phase increments by cents from C0.
    pub pitch: Box<[i32; PITCH3_LEN]>,
    /// sin(w0)/4 and cos(w0)/4 for the 256 cutoff steps.
    pub cut_sin: [f32; 256],
    pub cut_cos: [f32; 256],
    /// Distortion curves [type][threshold - 1][index].
    pub dist: Box<[[[i16; DIST3_LEN]; 10]; 2]>,
}

pub fn gen_pitch3(rate: i32) -> Box<[i32; PITCH3_LEN]> {
    // fld 2.0; fldln2; fxch; fyl2x; fdiv 1200: f32(ln 2 / 1200), set up at FL's start
    let step = fpu::ln(e(2.0)).div(e(1200.0)).to_f32();
    let c0 = k::C0_TIMES_RATE.div(ei(rate)).to_f64();
    let v: Vec<i32> = (0..PITCH3_LEN as i32)
        .map(|i| {
            let r = fpu::exp(ei(i).mul(e(step))).mul(Ext::from_f64(c0)).round_i64() as i32; // low 32 bits
            r.min(0x7fff_ffff)
        })
        .collect();
    v.into_boxed_slice().try_into().unwrap()
}

pub fn gen_cutoff3(rate: i32) -> ([f32; 256], [f32; 256]) {
    let (mut s, mut c) = ([0f32; 256], [0f32; 256]);
    let limit = ei(rate).mul(e(0.5)).sub(e(2000.0)).to_f64();
    for i in 0..256 {
        let x = ei(i as i32).div(e(2.0)).add(e(15.0)).to_f32();
        let p = power3(f32::from_bits(0x3f86_6666), x); // 1.05
        let mut f = e(p).mul(e(40.0)).to_f64();
        if Ext::from_f64(f).gt(Ext::from_f64(limit)) {
            f = limit;
        }
        let w = k::TWO_PI.mul(Ext::from_f64(f)).div(ei(rate)).to_f64();
        let (sn, cs) = fpu::sincos_f32(Ext::from_f64(w).to_f32());
        s[i] = e(sn).mul(e(0.25)).to_f32();
        c[i] = e(cs).mul(e(0.25)).to_f32();
    }
    (s, c)
}

pub fn gen_dist3(version: Fl3) -> Box<[[[i16; DIST3_LEN]; 10]; 2]> {
    let mut t: Box<[[[i16; DIST3_LEN]; 10]; 2]> = vec![[[0i16; DIST3_LEN]; 10]; 2].into_boxed_slice().try_into().unwrap();
    let ln10 = fpu::ln(k::TEN);
    let c_db = e(20.0).div(ln10).to_f64();
    let c_lin = ln10.mul(k::C0_05).to_f64();
    let step = if version == Fl3::V6 { 8191.0 } else { 8192.0 };
    for esi in 0..10 {
        let ratio = ei(esi + 1).mul(k::C0_1).to_f64();
        let floor = ei(9 - esi).mul(k::C1_6).to_f64();
        let pexp = ei(esi).mul(k::C0_02).add(k::C0_8).to_f32();
        let row = (9 - esi) as usize; // threshold 10 - esi
        let ln_db = |x: f64, zero: f64| if x == 0.0 { zero } else { fpu::ln(Ext::from_f64(x)).mul(Ext::from_f64(c_db)).to_f64() };
        let to_i16 = |y: f64| Ext::from_f64(y).mul(e(32767.0)).round_i64() as i16;
        for i in 0..8192 {
            let x = ei(i).div(e(step)).to_f64();
            // A: soft knee in dB
            let mut db = ln_db(x, -10000.0);
            if Ext::from_f64(db).ge(e(-25.0)) {
                db = Ext::from_f64(db).sub(e(-25.0)).mul(Ext::from_f64(ratio)).add(e(-25.0)).to_f64();
            }
            if !Ext::from_f64(floor).neg().lt(Ext::from_f64(db)) {
                db = Ext::from_f64(db).add(Ext::from_f64(floor)).to_f64();
            }
            t[0][row][i as usize] = to_i16(fpu::exp(Ext::from_f64(db).mul(Ext::from_f64(c_lin))).to_f64());
            // B: power curve in dB
            let db = ln_db(x, -1000.0);
            let pw = power3(Ext::from_f64(db).neg().to_f32(), pexp);
            let z = e(pw).neg().to_f64();
            t[1][row][i as usize] = to_i16(fpu::exp(Ext::from_f64(z).mul(Ext::from_f64(c_lin))).to_f64());
        }
        if version == Fl3::V6 {
            for ty in 0..2 {
                t[ty][row][8192] = t[ty][row][8191];
            }
        }
    }
    t
}

impl Tables3 {
    pub fn generate(version: Fl3) -> Tables3 {
        let rate = 44100;
        let (cut_sin, cut_cos) = gen_cutoff3(rate);
        Tables3 {
            version,
            rate,
            rate_ratio: e(44100.0).div(ei(rate)).to_f32(),
            pitch: gen_pitch3(rate),
            cut_sin,
            cut_cos,
            dist: gen_dist3(version),
        }
    }
}
