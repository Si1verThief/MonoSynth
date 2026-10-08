//! "?" shape persistence: the 16384-entry table as base64 (LE f32). Decoding writes
//! straight into a preallocated table so the audio thread can do it.

use ts404_core::tables::{WAVE_LEN, Wave};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(t: &Wave) -> String {
    let bytes: Vec<u8> = t.iter().flat_map(|v| v.to_le_bytes()).collect();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for c in bytes.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= c.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decode into `t` without allocating. False if the text isn't a 16384-entry table.
pub fn decode_into(s: &str, t: &mut Wave) -> bool {
    let val = |c: u8| ALPHABET.iter().position(|&a| a == c).map(|v| v as u32);
    let b = s.as_bytes();
    if b.len() != (WAVE_LEN * 4).div_ceil(3) * 4 {
        return false;
    }
    let mut word = [0u8; 4];
    let mut fill = 0;
    let mut idx = 0;
    for q in b.chunks_exact(4) {
        let mut n = 0u32;
        let mut k = 0usize;
        for &c in q {
            if c == b'=' {
                break;
            }
            let Some(v) = val(c) else { return false };
            n |= v << (18 - 6 * k);
            k += 1;
        }
        for i in 0..(k.saturating_sub(1)) {
            word[fill] = (n >> (16 - 8 * i)) as u8;
            fill += 1;
            if fill == 4 {
                if idx < WAVE_LEN {
                    t[idx] = f32::from_le_bytes(word);
                }
                idx += 1;
                fill = 0;
            }
        }
    }
    idx == WAVE_LEN
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let mut t: Box<Wave> = vec![0f32; WAVE_LEN].into_boxed_slice().try_into().unwrap();
        for (i, v) in t.iter_mut().enumerate() {
            *v = (i as f32 * 0.37).sin() * if i % 7 == 0 { -1e-30 } else { 1.0 };
        }
        let s = encode(&t);
        let mut u: Box<Wave> = vec![0f32; WAVE_LEN].into_boxed_slice().try_into().unwrap();
        assert!(decode_into(&s, &mut u));
        assert!(t.iter().zip(u.iter()).all(|(a, b)| a.to_bits() == b.to_bits()));
        assert!(!decode_into("abc", &mut u));
    }
}
