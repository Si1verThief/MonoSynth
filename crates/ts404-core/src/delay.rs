//! The TS404 delay line: a stereo feedback delay fed by the voice.
//!
//! Per block:
//!   1. the send level ramps towards `amount * vol / 128`,
//!   2. `voice * level` is added into the line at its write head,
//!   3. the line is rendered: each sample the slot under the read head is panned to L/R
//!      (circular pan law x volume, ramped), cleared, and fed back (`x * feedback/128`)
//!      into the slot under the write head, `length` samples ahead.
//! Values whose magnitude drops to 6e-8 or below are flushed to zero.
//!
//! `block` processes one block that way. `Delay::tick` is the same thing streamed one
//! sample at a time over fixed chunks; it is identical whenever the delay is at least one
//! chunk long (the sends of a chunk then never land where the chunk reads), so the synth
//! needs no extra latency.

use crate::x87::{Ext, e, ei};

/// The ring holds the delay length plus one block; also the longest chunk.
pub const MAX_BLOCK: usize = 4096;
/// Longest delay we keep (~47 s at 44.1 kHz).
pub const MAX_LEN: usize = 1 << 21;
/// Chunk length used by the streaming form.
pub const CHUNK: usize = 256;

const FLOOR: f32 = f32::from_bits(0x3380_0001); // ~5.96e-8
const INV128: f32 = 1.0 / 128.0;

/// Level tables: circular pan law pan[i] = cos(i * pi/256), linear volume vol[i] = i/128.
pub struct LevelTables {
    pub pan: [f32; 129],
    pub vol: [f32; 161],
}

impl LevelTables {
    pub fn generate() -> LevelTables {
        let step = f32::from_bits(0x3c49_0fdb); // pi/256 as f32
        let mut pan = [0f32; 129];
        for (i, v) in pan.iter_mut().enumerate() {
            *v = Ext::from_f64(libm::cos(ei(i as i32).mul(e(step)).to_f64())).to_f32();
        }
        let mut vol = [0f32; 161];
        for (i, v) in vol.iter_mut().enumerate() {
            *v = ei(i as i32).div(e(128.0)).to_f32();
        }
        LevelTables { pan, vol }
    }

    /// Gains for pan offset -64..=64 (0 = centre) at volume `v`.
    pub fn pan_gains(&self, pan: i32, v: f32) -> (f32, f32) {
        let p = pan.clamp(-64, 64);
        (e(self.pan[(64 + p) as usize]).mul(e(v)).to_f32(), e(self.pan[(64 - p) as usize]).mul(e(v)).to_f32())
    }
}

/// Per-sample increment that moves `start` to `*target` over a block of `n`.
/// Blocks shorter than 128 samples only move 1/128 per sample and leave `*target` where
/// that ends (flushing to 0 below the floor).
pub fn ramp(target: &mut f32, n: usize, start: f32) -> f32 {
    if e(*target).cmp(e(start)) == Some(core::cmp::Ordering::Equal) {
        return 0.0;
    }
    if n >= 128 {
        return e(*target).sub(e(start)).div(ei(n as i32)).to_f32();
    }
    let inc = e(*target).sub(e(start)).mul(e(INV128)).to_f32();
    *target = ei(n as i32).mul(e(inc)).add(e(start)).to_f32();
    if e(*target).lt(e(FLOOR)) {
        *target = 0.0;
        return e(start).neg().mul(e(INV128)).to_f32();
    }
    inc
}

/// Settings of the delay line plus the channel's send.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DelaySettings {
    /// Delay time in samples.
    pub length: usize,
    /// 0..=127 (default 64).
    pub feedback: i32,
    /// 0..=128, 64 = centre.
    pub pan: i32,
    /// 0..=128 (index into the volume table; default 128).
    pub volume: i32,
    /// TS404 "DELAY" knob, 0..=128.
    pub amount: i32,
    /// Channel volume factor the send is scaled by.
    pub chan_vol: f32,
}

pub struct Delay {
    buf: Vec<f32>,
    cap: usize,
    read: usize,
    write: usize,
    len: Option<usize>,
    /// Current output gains and send level.
    gl: f32,
    gr: f32,
    level: f32,
    // streaming chunk state
    chunk_pos: usize,
    chunk_n: usize,
    sending: bool,
    send_start: f32,
    send_inc: f32,
    send_g: Ext,
    send_base: usize,
    fb: f32,
    /// Samples since anything non-zero was written into the line.
    quiet: usize,
    g_l: Ext,
    g_r: Ext,
    inc_l: f32,
    inc_r: f32,
}

impl Delay {
    pub fn new() -> Delay {
        Delay {
            buf: vec![0.0; MAX_LEN + MAX_BLOCK],
            cap: MAX_BLOCK,
            read: 0,
            write: 0,
            len: None,
            gl: 0.0,
            gr: 0.0,
            level: 0.0,
            chunk_pos: 0,
            chunk_n: 0,
            sending: false,
            send_start: 0.0,
            send_inc: 0.0,
            send_g: Ext::ZERO,
            send_base: 0,
            fb: 0.0,
            quiet: usize::MAX / 2,
            g_l: Ext::ZERO,
            g_r: Ext::ZERO,
            inc_l: 0.0,
            inc_r: 0.0,
        }
    }

    pub fn reset(&mut self) {
        let len = self.len.take();
        self.level = 0.0;
        self.chunk_pos = 0;
        self.chunk_n = 0;
        self.sending = false;
        if let Some(l) = len {
            self.buf[..l + MAX_BLOCK].fill(0.0);
            self.read = 0;
            self.write = l;
            self.len = Some(l);
        }
    }

    /// A new length clears the line and snaps the pan
    /// gains to their target.
    fn set_length(&mut self, len: usize, s: &DelaySettings, t: &LevelTables) {
        let len = len.min(MAX_LEN);
        if self.len == Some(len) {
            return;
        }
        self.cap = len + MAX_BLOCK;
        self.buf[..self.cap].fill(0.0);
        self.quiet = self.cap;
        self.chunk_pos = 0;
        self.chunk_n = 0;
        self.read = 0;
        self.write = len;
        self.len = Some(len);
        let (l, r) = t.pan_gains(s.pan - 64, t.vol[s.volume.clamp(0, 160) as usize]);
        self.gl = l;
        self.gr = r;
    }

    fn send_target(s: &DelaySettings) -> f32 {
        ei(s.amount).mul(e(s.chan_vol)).mul(e(INV128)).to_f32()
    }

    /// One block: send `voice`, then add the line's output into `out` (interleaved L/R).
    pub fn block(&mut self, voice: &[f32], s: &DelaySettings, t: &LevelTables, out: &mut [f32]) {
        let n = voice.len();
        assert!(n <= MAX_BLOCK && out.len() == 2 * n);
        self.set_length(s.length, s, t);
        // send
        if s.amount > 0 || e(self.level).gt(e(0.0)) {
            let old = self.level;
            let mut target = Self::send_target(s);
            let inc = ramp(&mut target, n, old);
            self.level = target;
            // up to the end of the ring, then again from its start; each part starts the
            // ramp over from `old`
            let first = n.min(self.cap - self.write);
            let mut g = e(old);
            for i in 0..first {
                let d = &mut self.buf[self.write + i];
                *d = e(voice[i]).mul(g).add(e(*d)).to_f32();
                g = g.add(e(inc));
            }
            let mut g = e(old);
            for i in first..n {
                let d = &mut self.buf[i - first];
                *d = e(voice[i]).mul(g).add(e(*d)).to_f32();
                g = g.add(e(inc));
            }
        }
        // render
        let (mut tl, mut tr) = t.pan_gains(s.pan - 64, t.vol[s.volume.clamp(0, 160) as usize]);
        let (old_l, old_r) = (self.gl, self.gr);
        let inc_l = ramp(&mut tl, n, old_l);
        let inc_r = ramp(&mut tr, n, old_r);
        self.gl = tl;
        self.gr = tr;
        let fb = ei(s.feedback).mul(e(INV128)).to_f32();
        let (mut gl, mut gr) = (e(old_l), e(old_r));
        for i in 0..n {
            let x = self.buf[self.read];
            self.buf[self.read] = 0.0;
            out[2 * i] = e(x).mul(gl).add(e(out[2 * i])).to_f32();
            out[2 * i + 1] = e(x).mul(gr).add(e(out[2 * i + 1])).to_f32();
            let y = e(x).mul(e(fb)).add(e(self.buf[self.write]));
            self.buf[self.write] = flush(y);
            gr = gr.add(e(inc_r));
            gl = gl.add(e(inc_l));
            self.read += 1;
            if self.read >= self.cap {
                self.read = 0;
            }
            self.write += 1;
            if self.write >= self.cap {
                self.write = 0;
            }
        }
    }

    /// Nothing to send and the whole line has held zeros for a full lap: the line would
    /// only add zeros, so the caller may skip `tick` (see `idle_update`).
    pub fn idle(&self, s: &DelaySettings) -> bool {
        s.amount <= 0 && self.level == 0.0 && self.quiet >= self.cap && self.chunk_pos == self.chunk_n
    }

    /// Keep the length and pan gains current while idle.
    pub fn idle_update(&mut self, s: &DelaySettings, t: &LevelTables) {
        self.set_length(s.length, s, t);
        let (l, r) = t.pan_gains(s.pan - 64, t.vol[s.volume.clamp(0, 160) as usize]);
        self.gl = l;
        self.gr = r;
    }

    /// Chunk length the streaming form uses for these settings.
    pub fn chunk_len(len: usize) -> usize {
        len.clamp(1, CHUNK)
    }

    /// Streaming form of `block` over consecutive chunks of `chunk_len(length)` samples.
    /// `s` is read at chunk starts only. Adds the line's output to `l`/`r`.
    pub fn tick(&mut self, x: f32, s: &DelaySettings, t: &LevelTables, l: &mut f32, r: &mut f32) {
        if self.chunk_pos == self.chunk_n {
            self.start_chunk(s, t);
        }
        let i = self.chunk_pos;
        // send this sample (block() sends the whole chunk first, but none of it lands
        // where this chunk reads)
        if self.sending {
            let w = self.send_base + i;
            if w == self.cap {
                self.send_g = e(self.send_start); // past the ring's end the send ramp restarts
            }
            let idx = if w < self.cap { w } else { w - self.cap };
            let d = &mut self.buf[idx];
            *d = e(x).mul(self.send_g).add(e(*d)).to_f32();
            if *d != 0.0 {
                self.quiet = 0;
            }
            self.send_g = self.send_g.add(e(self.send_inc));
        }
        // render this sample
        let v = self.buf[self.read];
        self.buf[self.read] = 0.0;
        *l = e(v).mul(self.g_l).add(e(*l)).to_f32();
        *r = e(v).mul(self.g_r).add(e(*r)).to_f32();
        let y = e(v).mul(e(self.fb)).add(e(self.buf[self.write]));
        self.buf[self.write] = flush(y);
        if self.buf[self.write] != 0.0 {
            self.quiet = 0;
        } else {
            self.quiet = self.quiet.saturating_add(1);
        }
        self.g_r = self.g_r.add(e(self.inc_r));
        self.g_l = self.g_l.add(e(self.inc_l));
        self.read += 1;
        if self.read >= self.cap {
            self.read = 0;
        }
        self.write += 1;
        if self.write >= self.cap {
            self.write = 0;
        }
        self.chunk_pos += 1;
    }

    fn start_chunk(&mut self, s: &DelaySettings, t: &LevelTables) {
        self.set_length(s.length, s, t);
        let n = Self::chunk_len(self.len.unwrap_or(0));
        self.chunk_n = n;
        self.chunk_pos = 0;
        self.sending = s.amount > 0 || e(self.level).gt(e(0.0));
        if self.sending {
            let old = self.level;
            let mut target = Self::send_target(s);
            self.send_inc = ramp(&mut target, n, old);
            self.level = target;
            self.send_start = old;
            self.send_g = e(old);
            self.send_base = self.write;
        }
        let (mut tl, mut tr) = t.pan_gains(s.pan - 64, t.vol[s.volume.clamp(0, 160) as usize]);
        let (old_l, old_r) = (self.gl, self.gr);
        self.inc_l = ramp(&mut tl, n, old_l);
        self.inc_r = ramp(&mut tr, n, old_r);
        self.gl = tl;
        self.gr = tr;
        self.g_l = e(old_l);
        self.g_r = e(old_r);
        self.fb = ei(s.feedback).mul(e(INV128)).to_f32();
    }
}

/// Store a feedback sum, flushing |y| <= ~6e-8 to zero.
#[inline]
fn flush(y: Ext) -> f32 {
    if y.abs().gt(e(FLOOR)) { y.to_f32() } else { 0.0 }
}

impl Default for Delay {
    fn default() -> Self {
        Self::new()
    }
}
