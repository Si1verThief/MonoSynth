//! Lookup tables: oscillator and LFO wavetables, pitch increments, filter cutoff
//! coefficients and the distortion curves. Generated at startup.

use crate::fpu;
use crate::x87::{Ext, e, ei};

pub const WAVE_LEN: usize = 16384;
pub const PITCH_LEN: usize = 0xF00;
pub const DIST_LEN: usize = 8192;

pub type Wave = [f32; WAVE_LEN];

pub(crate) mod k {
    use crate::x87::Ext;
    pub const PI: Ext = Ext::from_bits(0xc90f_daa2_2168_c235, 0x4000);
    pub const TWO_PI: Ext = Ext::from_bits(0xc90f_daa2_2168_c235, 0x4001);
    pub const LN2_OVER_384: Ext = Ext::from_bits(0xec98_1ff5_17bf_2f2b, 0x3ff5);
    pub const C0_INC: Ext = Ext::from_bits(0xc265_db6c_7cf6_d49a, 0x4013); // 2^32 * 16.3516 Hz / 44100
    pub const CUT_BASE: Ext = Ext::from_bits(0x8666_6666_6666_6666, 0x3fff); // 1.05
    pub const RC_DECAY: Ext = Ext::from_bits(0xff3b_645a_1cac_0831, 0x3ffe); // 0.997
    pub const TEN: Ext = Ext::from_bits(0xa000_0000_0000_0000, 0x4002);
    pub const C0_05: Ext = Ext::from_bits(0xcccc_cccc_cccc_cccd, 0x3ffa);
    pub const C0_1: Ext = Ext::from_bits(0xcccc_cccc_cccc_cccd, 0x3ffb);
    pub const C1_6: Ext = Ext::from_bits(0xcccc_cccc_cccc_cccd, 0x3fff);
    pub const C0_02: Ext = Ext::from_bits(0xa3d7_0a3d_70a3_d70a, 0x3ff9);
    pub const C0_8: Ext = Ext::from_bits(0xcccc_cccc_cccc_cccd, 0x3ffe);
}

pub struct Tables {
    /// Oscillator shapes in selector order: saw, RC pulse, sine, square.
    pub osc: [Box<Wave>; 4],
    /// LFO shapes in selector order: sine, square, triangle, saw.
    pub lfo: [Box<Wave>; 4],
    /// sin(w0)/4 and cos(w0)/4 for the 256 cutoff steps.
    pub cut_sin: [f32; 256],
    pub cut_cos: [f32; 256],
    /// Phase increments: C0 * 2^(i/384), 32 steps per semitone.
    pub pitch: Box<[i32; PITCH_LEN]>,
    /// Distortion curves [type][threshold-1][|x| * 8191.75].
    pub dist: Box<[[[i16; DIST_LEN]; 10]; 2]>,
}

fn boxed_wave() -> Box<Wave> {
    vec![0f32; WAVE_LEN].into_boxed_slice().try_into().unwrap()
}

pub fn gen_waves() -> [Box<Wave>; 5] {
    let mut sine = boxed_wave();
    let mut tri = boxed_wave();
    let mut square = boxed_wave();
    let mut saw = boxed_wave();
    let mut rc = boxed_wave();
    let n = e(16384.0);
    for i in 0..WAVE_LEN {
        sine[i] = fpu::sin(ei(2 * i as i32).mul(k::PI).div(n)).to_f32();
    }
    for i in 0..0x2000 {
        tri[(0x3000 + i) % WAVE_LEN] = ei(4 * i as i32).div(n).add(e(-1.0)).to_f32();
    }
    for i in 0x2000..WAVE_LEN {
        tri[(0x3000 + i) % WAVE_LEN] = e(1.0).sub(ei(4 * (i - 0x2000) as i32).div(n)).to_f32();
    }
    for i in 0..WAVE_LEN {
        square[i] = if i < 0x2000 { 1.0 } else { -1.0 };
        saw[i] = ei(2 * i as i32).div(n).add(e(-1.0)).to_f32();
    }
    // RC pulse: exponential charge from -1 towards +1, then +1, then -1.
    let mut x = 32768.0f32;
    for v in rc.iter_mut().take(0x1db0) {
        let t = e(32768.0).sub(e(x).mul(e(2.0))).trunc_i64() as i16;
        *v = ei(t as i32).div(e(32768.0)).to_f32();
        x = k::RC_DECAY.mul(e(x)).to_f32();
    }
    for (i, v) in rc.iter_mut().enumerate().skip(0x1db0) {
        *v = if i < 0x3555 { 1.0 } else { -1.0 };
    }
    [sine, tri, square, saw, rc]
}

pub fn gen_pitch() -> Box<[i32; PITCH_LEN]> {
    let mut t: Box<[i32; PITCH_LEN]> = vec![0i32; PITCH_LEN].into_boxed_slice().try_into().unwrap();
    for (i, v) in t.iter_mut().enumerate() {
        let r = fpu::exp(ei(i as i32).mul(k::LN2_OVER_384)).mul(k::C0_INC).round_i64() as i32;
        *v = r.min(0x7fff_ffff);
    }
    t
}

pub fn gen_cutoff() -> ([f32; 256], [f32; 256]) {
    let (mut s, mut c) = ([0f32; 256], [0f32; 256]);
    for i in 0..256 {
        let x = ei(i as i32).div(e(2.0)).add(e(15.0));
        let mut f = fpu::power(k::CUT_BASE, x).mul(Ext::from_f64(40.0)).to_f64();
        if Ext::from_f64(f).gt(e(20050.0)) {
            f = 20050.0;
        }
        let w = k::TWO_PI.mul(Ext::from_f64(f)).div(e(44100.0)).to_f64();
        let (sn, cs) = fpu::sincos_f32(Ext::from_f64(w).to_f32());
        s[i] = e(sn).mul(e(0.25)).to_f32();
        c[i] = e(cs).mul(e(0.25)).to_f32();
    }
    (s, c)
}

/// dB-domain distortion curve, shared by the curve tables and the HQ (per-sample) path.
pub(crate) struct DistCurve {
    c_db: f64,   // 20 / ln 10
    c_lin: f64,  // ln 10 / 20
    ratio: f64,  // (11 - thr) * 0.1
    floor: f64,  // (thr - 1) * 1.6
    power: Ext,  // (10 - thr) * 0.02 + 0.8
}

impl DistCurve {
    pub(crate) fn new(thr: i32) -> DistCurve {
        DistCurve {
            c_db: e(20.0).div(fpu::ln(k::TEN)).to_f64(),
            c_lin: fpu::ln(k::TEN).mul(k::C0_05).to_f64(),
            ratio: ei(11 - thr).mul(k::C0_1).to_f64(),
            floor: ei(thr - 1).mul(k::C1_6).to_f64(),
            power: ei(10 - thr).mul(k::C0_02).add(k::C0_8),
        }
    }

    /// |x| in [0, 1] -> shaped magnitude.
    pub(crate) fn shape(&self, ty: i32, x: f64) -> f64 {
        let db = if ty == 0 {
            let mut db = if x == 0.0 { -10000.0 } else { fpu::ln(Ext::from_f64(x)).mul(Ext::from_f64(self.c_db)).to_f64() };
            if Ext::from_f64(db).ge(e(-25.0)) {
                db = Ext::from_f64(db).sub(e(-25.0)).mul(Ext::from_f64(self.ratio)).add(e(-25.0)).to_f64();
            }
            if !Ext::from_f64(-self.floor).lt(Ext::from_f64(db)) {
                db = Ext::from_f64(db).add(Ext::from_f64(self.floor)).to_f64();
            }
            db
        } else {
            let db = if x == 0.0 { -1000.0 } else { fpu::ln(Ext::from_f64(x)).mul(Ext::from_f64(self.c_db)).to_f64() };
            fpu::power(Ext::from_f64(-db), self.power).neg().to_f64()
        };
        fpu::exp(Ext::from_f64(db).mul(Ext::from_f64(self.c_lin))).to_f64()
    }
}

pub fn gen_dist() -> Box<[[[i16; DIST_LEN]; 10]; 2]> {
    let mut t: Box<[[[i16; DIST_LEN]; 10]; 2]> =
        vec![[[0i16; DIST_LEN]; 10]; 2].into_boxed_slice().try_into().unwrap();
    for thr in 1..=10 {
        let c = DistCurve::new(thr);
        for i in 0..DIST_LEN {
            let x = ei(i as i32).div(e(8192.0)).to_f64();
            for ty in 0..2 {
                let y = c.shape(ty, x);
                t[ty as usize][(thr - 1) as usize][i] = Ext::from_f64(y).mul(e(32767.0)).round_i64() as i16;
            }
        }
    }
    t
}

impl Tables {
    pub fn generate() -> Tables {
        let [sine, tri, square, saw, rc] = gen_waves();
        let (cut_sin, cut_cos) = gen_cutoff();
        Tables {
            osc: [saw.clone(), rc, sine.clone(), square.clone()],
            lfo: [sine, square, tri, saw],
            cut_sin,
            cut_cos,
            pitch: gen_pitch(),
            dist: gen_dist(),
        }
    }
}
