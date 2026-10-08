//! 80-bit extended-precision arithmetic (64-bit mantissa, round-to-nearest-even), done
//! in software so the engine computes identically on every CPU and platform.
//!
//! The engine keeps intermediates in extended precision and rounds only when storing
//! to an f32 / f64 / integer. Doing the same arithmetic in f64 would round differently
//! now and then, and the filter's feedback would carry that difference forward, so the
//! per-sample path uses this type.
//!
//! Only finite values are modelled exactly. NaN/Inf fall back to f64 arithmetic; the
//! engine never produces them in its reachable parameter range.

use core::cmp::Ordering;

#[derive(Clone, Copy, Debug)]
pub struct Ext {
    /// value = (-1)^neg * man * 2^(exp - 63). `man` has its top bit set unless the value is zero.
    neg: bool,
    exp: i32,
    man: u64,
    /// Non-finite escape hatch (NaN / Inf).
    special: Option<f64>,
}

impl Ext {
    pub const ZERO: Ext = Ext { neg: false, exp: 0, man: 0, special: None };

    /// Exact 80-bit constant from its in-memory form (mantissa, sign+exponent word).
    pub const fn from_bits(man: u64, se: u16) -> Ext {
        let e = (se & 0x7fff) as i32;
        Ext { neg: se & 0x8000 != 0, exp: e - 16383, man, special: None }
    }

    #[inline]
    fn is_zero(self) -> bool {
        self.man == 0 && self.special.is_none()
    }

    #[inline]
    pub fn from_i64(v: i64) -> Ext {
        if v == 0 {
            return Ext::ZERO;
        }
        let neg = v < 0;
        let m = v.unsigned_abs();
        let lz = m.leading_zeros();
        Ext { neg, exp: 63 - lz as i32, man: m << lz, special: None }
    }

    #[inline]
    pub fn from_i32(v: i32) -> Ext {
        Ext::from_i64(v as i64)
    }

    #[inline]
    pub fn from_f32(v: f32) -> Ext {
        Ext::from_f64(v as f64) // exact
    }

    pub fn from_f64(v: f64) -> Ext {
        if !v.is_finite() {
            return Ext { neg: false, exp: 0, man: 0, special: Some(v) };
        }
        if v == 0.0 {
            return Ext { neg: v.is_sign_negative(), ..Ext::ZERO };
        }
        let bits = v.to_bits();
        let neg = bits >> 63 != 0;
        let be = ((bits >> 52) & 0x7ff) as i32;
        let frac = bits & ((1u64 << 52) - 1);
        let (m, e) = if be == 0 { (frac, -1074) } else { (frac | (1u64 << 52), be - 1075) };
        // value = m * 2^e
        let lz = m.leading_zeros();
        Ext { neg, exp: e + 63 - lz as i32, man: m << lz, special: None }
    }

    /// Exact value as f64 when it fits (used only for comparisons / transcendental helpers).
    pub fn to_f64_lossy(self) -> f64 {
        if let Some(s) = self.special {
            return s;
        }
        if self.man == 0 {
            return if self.neg { -0.0 } else { 0.0 };
        }
        let v = (self.man as f64) * 2f64.powi(self.exp - 63);
        if self.neg { -v } else { v }
    }

    /// Round a magnitude `m * 2^e` (m: u128, any width) to 64 bits, round-to-nearest-even.
    fn round_pack(neg: bool, e: i32, m: u128) -> Ext {
        if m == 0 {
            return Ext { neg, ..Ext::ZERO };
        }
        let lz = m.leading_zeros() as i32;
        let m = m << lz; // top bit at 127
        let e = e - lz + 127; // value = m * 2^(e - 127)
        let hi = (m >> 64) as u64;
        let lo = m as u64;
        let round = lo >> 63 != 0;
        let sticky = lo << 1 != 0;
        let (man, exp) = if round && (sticky || hi & 1 != 0) {
            match hi.checked_add(1) {
                Some(h) => (h, e),
                None => (1u64 << 63, e + 1),
            }
        } else {
            (hi, e)
        };
        Ext { neg, exp, man, special: None }
    }

    #[inline]
    pub fn neg(self) -> Ext {
        if let Some(s) = self.special {
            return Ext { special: Some(-s), ..self };
        }
        Ext { neg: !self.neg, ..self }
    }

    /// FABS.
    #[inline]
    pub fn abs(self) -> Ext {
        if let Some(s) = self.special {
            return Ext { special: Some(s.abs()), ..self };
        }
        Ext { neg: false, ..self }
    }

    pub fn add(self, o: Ext) -> Ext {
        if self.special.is_some() || o.special.is_some() {
            return Ext::from_f64(self.to_f64_lossy() + o.to_f64_lossy());
        }
        if self.is_zero() {
            return if o.is_zero() { Ext { neg: self.neg && o.neg, ..Ext::ZERO } } else { o };
        }
        if o.is_zero() {
            return self;
        }
        let (a, b) = if (self.exp, self.man) >= (o.exp, o.man) { (self, o) } else { (o, self) };
        let d = (a.exp - b.exp) as u32;
        // 62 guard bits below the mantissa, plus a sticky lsb for anything shifted further.
        let am = (a.man as u128) << 62;
        let bm_full = (b.man as u128) << 62;
        let bm = if d >= 127 {
            1
        } else {
            let s = bm_full >> d;
            if s << d != bm_full { s | 1 } else { s }
        };
        let e = a.exp - 63 - 62;
        if a.neg == b.neg {
            Ext::round_pack(a.neg, e, am + bm)
        } else {
            let r = am - bm; // |a| >= |b|
            if r == 0 {
                return Ext::ZERO; // RNE: x - x = +0
            }
            Ext::round_pack(a.neg, e, r)
        }
    }

    #[inline]
    pub fn sub(self, o: Ext) -> Ext {
        self.add(o.neg())
    }

    pub fn mul(self, o: Ext) -> Ext {
        if self.special.is_some() || o.special.is_some() {
            return Ext::from_f64(self.to_f64_lossy() * o.to_f64_lossy());
        }
        let neg = self.neg != o.neg;
        if self.is_zero() || o.is_zero() {
            return Ext { neg, ..Ext::ZERO };
        }
        let m = (self.man as u128) * (o.man as u128);
        Ext::round_pack(neg, self.exp + o.exp - 126, m)
    }

    pub fn div(self, o: Ext) -> Ext {
        if self.special.is_some() || o.special.is_some() || o.is_zero() {
            return Ext::from_f64(self.to_f64_lossy() / o.to_f64_lossy());
        }
        let neg = self.neg != o.neg;
        if self.is_zero() {
            return Ext { neg, ..Ext::ZERO };
        }
        let a = (self.man as u128) << 64;
        let b = o.man as u128;
        let q1 = a / b;
        let r1 = a % b;
        let q2 = (r1 << 2) / b;
        let r2 = (r1 << 2) % b;
        let q = (q1 << 2) | q2;
        let q = if r2 != 0 { (q << 1) | 1 } else { q << 1 };
        // a = man_a * 2^64, value_a = man_a * 2^(ea-63); q ≈ a/b * 8
        Ext::round_pack(neg, self.exp - o.exp - 64 - 3, q)
    }

    /// FSTP to an f32 (round-to-nearest-even, denormals produced).
    pub fn to_f32(self) -> f32 {
        if let Some(s) = self.special {
            return s as f32;
        }
        if self.man == 0 {
            return if self.neg { -0.0 } else { 0.0 };
        }
        let sign = if self.neg { 0x8000_0000u32 } else { 0 };
        // normal f32: exponent range [-126, 127]; denormal below.
        let shift: i32 = if self.exp >= -126 { 40 } else { 40 + (-126 - self.exp) };
        if shift > 64 + 1 {
            return f32::from_bits(sign); // rounds to zero
        }
        let m = self.man;
        let (mut q, round, sticky) = if shift >= 64 {
            (0u64, if shift == 64 { m >> 63 != 0 } else { false }, if shift == 64 { m << 1 != 0 } else { m != 0 })
        } else {
            let q = m >> shift;
            let rem = m & ((1u64 << shift) - 1);
            let half = 1u64 << (shift - 1);
            (q, rem & half != 0, rem & (half - 1) != 0)
        };
        if round && (sticky || q & 1 != 0) {
            q += 1;
        }
        let bits = if self.exp >= -126 {
            // q has 24 bits (or 25 after carry)
            let (q, e) = if q >> 24 != 0 { (q >> 1, self.exp + 1) } else { (q, self.exp) };
            if e > 127 {
                return f32::from_bits(sign | 0x7f80_0000);
            }
            (((e + 127) as u32) << 23) | (q as u32 & 0x7f_ffff)
        } else {
            q as u32 // denormal (carry into bit 23 makes it the smallest normal, which is correct)
        };
        f32::from_bits(sign | bits)
    }

    /// FSTP to an f64.
    pub fn to_f64(self) -> f64 {
        if let Some(s) = self.special {
            return s;
        }
        if self.man == 0 {
            return if self.neg { -0.0 } else { 0.0 };
        }
        assert!(self.exp >= -1022 && self.exp <= 1023, "f64 range");
        let m = self.man;
        let mut q = m >> 11;
        let rem = m & 0x7ff;
        if rem > 0x400 || (rem == 0x400 && q & 1 != 0) {
            q += 1;
        }
        let (q, e) = if q >> 53 != 0 { (q >> 1, self.exp + 1) } else { (q, self.exp) };
        let bits = ((self.neg as u64) << 63) | (((e + 1023) as u64) << 52) | (q & ((1u64 << 52) - 1));
        f64::from_bits(bits)
    }

    /// Round to nearest even integer. Out of range -> i64::MIN.
    pub fn round_i64(self) -> i64 {
        self.to_int(false)
    }

    /// Truncate toward zero.
    pub fn trunc_i64(self) -> i64 {
        self.to_int(true)
    }

    fn to_int(self, trunc: bool) -> i64 {
        if self.special.is_some() {
            return i64::MIN;
        }
        if self.man == 0 {
            return 0;
        }
        if self.exp >= 63 {
            return i64::MIN;
        }
        let mag: u64 = if self.exp < -1 {
            0
        } else {
            let shift = (63 - self.exp) as u32; // 1..=64
            let q = if shift == 64 { 0 } else { self.man >> shift };
            let rem = if shift == 64 { self.man } else { self.man & ((1u64 << shift) - 1) };
            let half = 1u64 << (shift - 1);
            if !trunc && (rem > half || (rem == half && q & 1 != 0)) { q + 1 } else { q }
        };
        if mag > i64::MAX as u64 {
            return i64::MIN;
        }
        if self.neg { -(mag as i64) } else { mag as i64 }
    }

    /// Round to nearest even i32; out of range -> i32::MIN.
    pub fn round_i32(self) -> i32 {
        let v = self.round_i64();
        if v < i32::MIN as i64 || v > i32::MAX as i64 { i32::MIN } else { v as i32 }
    }

    /// FCOM ordering (NaN compares as unordered → None).
    pub fn cmp(self, o: Ext) -> Option<Ordering> {
        if self.special.is_some() || o.special.is_some() {
            return self.to_f64_lossy().partial_cmp(&o.to_f64_lossy());
        }
        let key = |x: Ext| -> (i8, i32, u64) {
            if x.man == 0 { (0, 0, 0) } else if x.neg { (-1, -x.exp, !x.man) } else { (1, x.exp, x.man) }
        };
        Some(key(self).cmp(&key(o)))
    }

    #[inline]
    pub fn lt(self, o: Ext) -> bool {
        self.cmp(o) == Some(Ordering::Less)
    }
    #[inline]
    pub fn le(self, o: Ext) -> bool {
        matches!(self.cmp(o), Some(Ordering::Less | Ordering::Equal))
    }
    #[inline]
    pub fn gt(self, o: Ext) -> bool {
        self.cmp(o) == Some(Ordering::Greater)
    }
    #[inline]
    pub fn ge(self, o: Ext) -> bool {
        matches!(self.cmp(o), Some(Ordering::Greater | Ordering::Equal))
    }
}

/// Shorthand: f32 operand loaded onto the x87 stack.
#[inline]
pub fn e(v: f32) -> Ext {
    Ext::from_f32(v)
}

/// Shorthand: integer operand (FILD).
#[inline]
pub fn ei(v: i32) -> Ext {
    Ext::from_i32(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    fn rf32(seed: &mut u64) -> f32 {
        loop {
            let v = f32::from_bits(rng(seed) as u32);
            if v.is_finite() && v.abs() < 1e30 && v.abs() > 1e-30 {
                return v;
            }
        }
    }

    // f32 op f32 rounded straight to f32 equals the correctly rounded result whether the
    // intermediate is 64-bit or 53-bit (both have > 2*24+2 bits), so f64 is an oracle here.
    #[test]
    fn single_ops_match_f64_oracle() {
        let mut s = 0x1234_5678_9abc_def0u64;
        for _ in 0..200_000 {
            let (a, b) = (rf32(&mut s), rf32(&mut s));
            assert_eq!(e(a).add(e(b)).to_f32(), ((a as f64) + (b as f64)) as f32, "{a} + {b}");
            assert_eq!(e(a).sub(e(b)).to_f32(), ((a as f64) - (b as f64)) as f32, "{a} - {b}");
            assert_eq!(e(a).mul(e(b)).to_f32(), ((a as f64) * (b as f64)) as f32, "{a} * {b}");
            assert_eq!(e(a).div(e(b)).to_f32(), ((a as f64) / (b as f64)) as f32, "{a} / {b}");
        }
    }

    #[test]
    fn f64_roundtrip_and_ints() {
        let mut s = 99u64;
        for _ in 0..100_000 {
            let v = f64::from_bits(rng(&mut s) >> 2);
            if v.is_finite() && v.abs() > 1e-300 && v.abs() < 1e300 {
                assert_eq!(Ext::from_f64(v).to_f64(), v);
            }
        }
        assert_eq!(Ext::from_f64(2.5).round_i64(), 2);
        assert_eq!(Ext::from_f64(3.5).round_i64(), 4);
        assert_eq!(Ext::from_f64(-2.5).round_i64(), -2);
        assert_eq!(Ext::from_f64(-2.7).trunc_i64(), -2);
        assert_eq!(Ext::from_f64(0.49).round_i64(), 0);
        assert_eq!(Ext::from_f64(3e9).round_i32(), i32::MIN);
        assert_eq!(e(1e-40).to_f32(), 1e-40f32);
        assert_eq!(e(-1e-45).to_f32(), -1e-45f32);
        assert_eq!(ei(-7).to_f32(), -7.0);
    }

    #[test]
    fn extended_constants() {
        // 0.1 in extended precision (cd cc cc cc cc cc cc cc fb 3f)
        let c = Ext::from_bits(0xcccc_cccc_cccc_cccd, 0x3ffb);
        assert!((c.to_f64() - 0.1).abs() < 1e-17);
        // 1/3 has a 64-bit mantissa different from f64's
        let third = ei(1).div(ei(3));
        assert_eq!(third.man, 0xaaaa_aaaa_aaaa_aaab);
    }
}
