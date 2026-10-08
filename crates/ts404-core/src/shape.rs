//! The "?" oscillator shape: a sample turned into a 16384-entry wavetable. A sample of
//! any other length is first resampled to 16384 stereo frames with a cubic (Catmull-Rom)
//! resampler, then each frame becomes (L + R) / 65536.

use crate::tables::{WAVE_LEN, Wave};
use crate::x87::{Ext, e, ei};

/// Sample -> wavetable. `frames` are stereo int16 (mono = L = R).
pub fn shape_table(frames: &[[i16; 2]]) -> Box<Wave> {
    let resampled;
    let src: &[[i16; 2]] = if frames.len() == WAVE_LEN {
        frames
    } else {
        resampled = resample(frames, WAVE_LEN);
        &resampled
    };
    let mut t: Box<Wave> = vec![0f32; WAVE_LEN].into_boxed_slice().try_into().unwrap();
    let scale = 1.0 / 65536.0f32;
    for (v, f) in t.iter_mut().zip(src) {
        *v = ei(f[0] as i32 + f[1] as i32).mul(e(scale)).to_f32();
    }
    t
}

/// Cubic resampler: 32.32 fixed-point stepping,
/// Catmull-Rom interpolation per channel with zeros beyond the sample's ends.
pub fn resample(src: &[[i16; 2]], out_len: usize) -> Vec<[i16; 2]> {
    let mut out = vec![[0i16; 2]; out_len];
    let n = src.len();
    if n == 0 {
        return out;
    }
    let step_int = (n / out_len) as i64;
    let step_frac = ((((n % out_len) as u64) << 32) / out_len as u64) as u32;
    let end = n as i64 - 1;
    let (mut pos, mut frac) = (0i64, 0u32);
    let two_m24 = Ext::from_f64(1.0 / 16_777_216.0);
    let s = |i: i64, c: usize| src[i as usize][c] as i32;
    for o in out.iter_mut() {
        let f = ei((frac >> 8) as i32).mul(two_m24).to_f32();
        for (c, dst) in o.iter_mut().enumerate() {
            let y0 = s(pos, c);
            let ym1 = if pos > 0 { s(pos - 1, c) } else { 0 };
            let y1 = if end > pos { s(pos + 1, c) } else { 0 };
            let y2 = if end > pos + 1 { s(pos + 2, c) } else { 0 };
            let a = ei(3 * (y0 - y1) - ym1 + y2).mul(e(0.5)).to_f32();
            let b = ei(2 * y1 + ym1).sub(ei(5 * y0 + y2).mul(e(0.5))).to_f32();
            let cc = ei(y1 - ym1).mul(e(0.5)).to_f32();
            let d = e(a).mul(e(f)).add(e(b)).mul(e(f)).add(e(cc)).mul(e(f)).round_i64() as i32;
            *dst = (y0.wrapping_add(d)).clamp(-32768, 32767) as i16;
        }
        let (nf, carry) = frac.overflowing_add(step_frac);
        frac = nf;
        pos += step_int + carry as i64;
        if pos > end {
            break;
        }
    }
    out
}

/// Minimal WAV reader for shape files: PCM 8/16/24/32-bit or 32-bit float, any channel
/// count (first two used; mono is duplicated).
/// Non-16-bit data is converted to 16-bit by truncation.
pub fn read_wav(bytes: &[u8]) -> Result<Vec<[i16; 2]>, String> {
    let u16le = |b: &[u8], o: usize| u16::from_le_bytes([b[o], b[o + 1]]);
    let u32le = |b: &[u8], o: usize| u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]]);
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("not a WAV file".into());
    }
    let (mut fmt, mut data) = (None, None);
    let mut p = 12;
    while p + 8 <= bytes.len() {
        let id = &bytes[p..p + 4];
        let len = u32le(bytes, p + 4) as usize;
        let body = &bytes[p + 8..(p + 8 + len).min(bytes.len())];
        match id {
            b"fmt " if body.len() >= 16 => fmt = Some((u16le(body, 0), u16le(body, 2) as usize, u16le(body, 14) as usize)),
            b"data" => data = Some(body),
            _ => {}
        }
        p += 8 + len + (len & 1);
    }
    let (format, channels, bits) = fmt.ok_or("no fmt chunk")?;
    let data = data.ok_or("no data chunk")?;
    if channels == 0 {
        return Err("no channels".into());
    }
    let width = bits / 8;
    if width == 0 {
        return Err("bad sample width".into());
    }
    let frame = width * channels;
    let sample = |b: &[u8]| -> Result<i16, String> {
        Ok(match (format, bits) {
            (1, 8) => ((b[0] as i16) - 128) << 8,
            (1, 16) => i16::from_le_bytes([b[0], b[1]]),
            (1, 24) => i16::from_le_bytes([b[1], b[2]]),
            (1, 32) => i16::from_le_bytes([b[2], b[3]]),
            (3, 32) => (f32::from_le_bytes([b[0], b[1], b[2], b[3]]).clamp(-1.0, 1.0) * 32767.0) as i16,
            (0xFFFE, 16) => i16::from_le_bytes([b[0], b[1]]),
            (0xFFFE, 24) => i16::from_le_bytes([b[1], b[2]]),
            _ => return Err(format!("unsupported WAV format {format}/{bits}-bit")),
        })
    };
    data.chunks_exact(frame)
        .map(|f| {
            let l = sample(&f[..width])?;
            let r = if channels > 1 { sample(&f[width..2 * width])? } else { l };
            Ok([l, r])
        })
        .collect()
}
