//! Fixed-ratio windowed-sinc resampler from the engine's native 44.1 kHz to the host rate.
//!
//! The engine's tables, envelope rates and LFO speeds are all per-sample at 44.1 kHz,
//! so the engine always runs at that rate. At a 44.1 kHz host the plugin bypasses this
//! entirely and the output is the engine output unchanged.
//!
//! The kernel is centred, so audio is not delayed; note events are applied
//! `HALF + 1` engine samples late (the lookahead the kernel needs), which is the
//! latency reported to the host.

pub const ENGINE_RATE: u32 = 44100;
pub const HALF: usize = 24; // taps on each side
const TAPS: usize = 2 * HALF;
const PHASES: usize = 512;

pub struct Resampler {
    out_rate: u64,
    table: Vec<f32>, // (PHASES + 1) * TAPS
    ring: Vec<f32>,  // power-of-two length
    /// Engine samples pushed so far.
    produced: u64,
    /// Output frames produced so far.
    frame: u64,
}

fn bessel_i0(x: f64) -> f64 {
    let (mut sum, mut term, mut k) = (1.0, 1.0, 1.0);
    while term > 1e-12 * sum {
        term *= (x / (2.0 * k)).powi(2);
        sum += term;
        k += 1.0;
    }
    sum
}

impl Resampler {
    /// `max_block`: largest host buffer, sizes the ring so nothing allocates later.
    pub fn new(out_rate: u32, max_block: usize) -> Resampler {
        let ratio = out_rate as f64 / ENGINE_RATE as f64;
        let cutoff = 0.5 * ratio.min(1.0) * 0.91; // cycles per engine sample
        let beta = 9.0;
        let mut table = vec![0f32; (PHASES + 1) * TAPS];
        for ph in 0..=PHASES {
            let frac = ph as f64 / PHASES as f64;
            let mut tmp = [0f64; TAPS];
            let mut sum = 0.0;
            for (j, t) in tmp.iter_mut().enumerate() {
                // tap j multiplies input n0 - HALF + 1 + j; d = its distance from the output instant
                let d = (j as f64 - HALF as f64 + 1.0) - frac;
                let x = 2.0 * cutoff * d;
                let sinc = if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
                let w = d / HALF as f64;
                let win = if w.abs() >= 1.0 { 0.0 } else { bessel_i0(beta * (1.0 - w * w).sqrt()) / bessel_i0(beta) };
                *t = sinc * win;
                sum += *t;
            }
            for (r, t) in table[ph * TAPS..(ph + 1) * TAPS].iter_mut().zip(tmp) {
                *r = (t / sum) as f32;
            }
        }
        let span = (max_block as f64 / ratio).ceil() as usize + 2 * TAPS + 8;
        Resampler { out_rate: out_rate as u64, table, ring: vec![0.0; span.next_power_of_two()], produced: 0, frame: 0 }
    }

    pub fn latency(&self) -> u32 {
        Self::latency_at(self.out_rate as u32)
    }

    pub fn latency_at(out_rate: u32) -> u32 {
        (((HALF as u64 + 1) * out_rate as u64).div_ceil(ENGINE_RATE as u64)) as u32
    }

    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.produced = 0;
        self.frame = 0;
    }

    /// Engine sample index at which an event `offset` frames into the next buffer applies.
    pub fn event_time(&self, offset: u32) -> u64 {
        ((self.frame + offset as u64) * ENGINE_RATE as u64).div_ceil(self.out_rate) + HALF as u64 + 1
    }

    /// Number of engine samples that must be pushed before `produce` can emit `frames` frames.
    pub fn input_needed(&self, frames: usize) -> u64 {
        if frames == 0 {
            return 0;
        }
        let last = (self.frame + frames as u64 - 1) * ENGINE_RATE as u64 / self.out_rate;
        (last + HALF as u64 + 1).saturating_sub(self.produced)
    }

    pub fn push(&mut self, samples: &[f32]) {
        let mask = self.ring.len() - 1;
        for &s in samples {
            self.ring[self.produced as usize & mask] = s;
            self.produced += 1;
        }
    }

    pub fn produce(&mut self, out: &mut [f32]) {
        let mask = self.ring.len() - 1;
        for o in out.iter_mut() {
            let num = self.frame * ENGINE_RATE as u64;
            let n0 = num / self.out_rate;
            let pos = (num % self.out_rate) as f64 / self.out_rate as f64 * PHASES as f64;
            let ph = pos as usize;
            let a = (pos - ph as f64) as f32;
            let r0 = &self.table[ph * TAPS..(ph + 1) * TAPS];
            let r1 = &self.table[(ph + 1) * TAPS..(ph + 2) * TAPS];
            let first = (n0 + 1).wrapping_sub(HALF as u64); // before the first HALF inputs this reads zeros
            let mut acc = 0f32;
            for j in 0..TAPS {
                let c = r0[j] + (r1[j] - r0[j]) * a;
                acc += self.ring[first.wrapping_add(j as u64) as usize & mask] * c;
            }
            *o = acc;
            self.frame += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_survives_48k() {
        let mut r = Resampler::new(48000, 1024);
        let mut t = 0u64;
        let f = 1000.0;
        let mut out = vec![0f32; 1000];
        let mut all = vec![];
        let mut inbuf = vec![0f32; 2048];
        for _ in 0..48 {
            let n = r.input_needed(out.len()) as usize;
            for s in inbuf[..n].iter_mut() {
                *s = (2.0 * std::f64::consts::PI * f * t as f64 / 44100.0).sin() as f32;
                t += 1;
            }
            r.push(&inbuf[..n]);
            r.produce(&mut out);
            all.extend_from_slice(&out);
        }
        let mut err = 0f64;
        for (k, &y) in all.iter().enumerate().skip(100) {
            let want = (2.0 * std::f64::consts::PI * f * k as f64 / 48000.0).sin();
            err = err.max((y as f64 - want).abs());
        }
        assert!(err < 2e-3, "max err {err}");
    }
}
