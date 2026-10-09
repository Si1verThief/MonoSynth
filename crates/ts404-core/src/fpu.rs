//! Extended-precision `exp`, `ln`, `power`, `int_power`, `sin` and `sincos`, as used to
//! build the lookup tables and per-block constants.
//!
//! The transcendental steps are evaluated through f64 with the pure-Rust `libm` crate,
//! so every platform builds identical tables.

use crate::x87::Ext;

/// log2(e) and ln(2) in extended precision.
const L2E: Ext = Ext::from_bits(0xb8aa_3b29_5c17_f0bc, 0x3fff);
const LN2: Ext = Ext::from_bits(0xb172_17f7_d1cf_79ac, 0x3ffe);

/// FRNDINT (round to nearest even).
fn frndint(x: Ext) -> Ext {
    Ext::from_i64(x.round_i64())
}

/// F2XM1: 2^x - 1, evaluated in f64.
fn f2xm1(x: Ext) -> Ext {
    Ext::from_f64(libm::pow(2.0, x.to_f64()) - 1.0)
}

/// FYL2X: y * log2(x), evaluated in f64.
fn fyl2x(y: Ext, x: Ext) -> Ext {
    let t = libm::log(x.to_f64()) / libm::log(2.0);
    Ext::from_f64(t * y.to_f64())
}

/// FSCALE with an integral st(1).
fn fscale(x: Ext, n: Ext) -> Ext {
    x.mul(Ext::from_f64(libm::ldexp(1.0, n.trunc_i64() as i32)))
}

/// e^x: split x·log2(e) into integer and fraction, 2^fraction - 1, add 1, scale.
pub fn exp(x: Ext) -> Ext {
    let t = x.mul(L2E);
    let n = frndint(t);
    let f = t.sub(n);
    fscale(f2xm1(f).add(Ext::from_i32(1)), n)
}

/// Natural logarithm: ln(2)·log2(x).
pub fn ln(x: Ext) -> Ext {
    fyl2x(LN2, x)
}

/// FSIN, evaluated in f64.
pub fn sin(x: Ext) -> Ext {
    Ext::from_f64(libm::sin(x.to_f64()))
}

/// FCOS, evaluated in f64.
pub fn cos(x: Ext) -> Ext {
    Ext::from_f64(libm::cos(x.to_f64()))
}

/// sin and cos of an f32 argument, each rounded to f32.
pub fn sincos_f32(x: f32) -> (f32, f32) {
    let d = x as f64;
    (Ext::from_f64(libm::sin(d)).to_f32(), Ext::from_f64(libm::cos(d)).to_f32())
}

/// base^n for integer n by repeated squaring.
pub fn int_power(base: Ext, exponent: i32) -> Ext {
    let one = Ext::from_i32(1);
    let mut result = one;
    let mut n = exponent.unsigned_abs();
    if n != 0 {
        let mut x = base;
        loop {
            let bit = n & 1;
            n >>= 1;
            if bit != 0 {
                result = result.mul(x);
                if n == 0 {
                    break;
                }
            }
            x = x.mul(x);
        }
    }
    if exponent < 0 { one.div(result) } else { result }
}

/// base^exponent: integer exponents by repeated squaring, otherwise exp(exponent·ln(base)).
pub fn power(base: Ext, exponent: Ext) -> Ext {
    let zero = Ext::ZERO;
    if exponent.cmp(zero) == Some(core::cmp::Ordering::Equal) {
        return Ext::from_i32(1);
    }
    if base.cmp(zero) == Some(core::cmp::Ordering::Equal) && exponent.gt(zero) {
        return zero;
    }
    let t = exponent.trunc_i64();
    let frac_zero = exponent.sub(Ext::from_i64(t)).cmp(zero) == Some(core::cmp::Ordering::Equal);
    if frac_zero && t.unsigned_abs() <= i32::MAX as u64 {
        int_power(base, t as i32)
    } else {
        exp(exponent.mul(ln(base)))
    }
}
