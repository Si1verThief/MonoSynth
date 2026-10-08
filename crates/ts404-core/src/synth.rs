//! Host-independent synth: MIDI event scheduling, the slide-timing lookahead and
//! resampling around a `MonoVoice`. The plugin is a thin wrapper around this, so
//! everything here is testable without a host (tests/synth_fuzz.rs).

use std::sync::Arc;

use crate::delay::{Delay, DelaySettings, LevelTables};
use crate::engine::idx;
use crate::mono::MonoVoice;
use crate::resample::{ENGINE_RATE, HALF, Resampler};
use crate::tables::{Tables, WAVE_LEN, Wave};

const MAX_EVENTS: usize = 1024;
const MAX_HELD: usize = 32;

#[derive(Clone, Copy, Debug)]
enum Ev {
    On(u8),
    Off(u8),
    /// TS404 slide timing: begin the glide one step before the note arrives.
    SlideAhead(u8),
    /// TS404 slide timing: the note-on whose glide already ran.
    Push(u8),
}

/// Per-block settings (read from the host's parameters).
#[derive(Clone, Copy, Debug)]
pub struct Settings {
    /// One 1/16 note at the host tempo, in engine samples (one step; the delay time unit).
    pub sixteenth: i32,
    /// Delay line: time in 1/48 steps (0..=768), feedback 0..=127,
    /// pan 0..=128, volume 0..=128. The send amount is engine param 35.
    pub delay_ticks: i32,
    pub delay_feedback: i32,
    pub delay_pan: i32,
    pub delay_volume: i32,
    /// Channel pan 0..=128 (64 = centre), circular pan law.
    pub pan: i32,
    /// Engine ints 1..=35 (the .404 layout).
    pub params: [i32; idx::PARAM_COUNT],
    pub hq: bool,
    /// One step (slide length / step-gate reference) in engine samples.
    pub step_len: i32,
    /// Step gate, 0..=256 (256 = hold until note-off).
    pub gate: i32,
    pub fl_slides: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let mut params = [0i32; idx::PARAM_COUNT];
        params[idx::OSC_MIX - 1] = 128;
        params[idx::CUTOFF - 1] = 64;
        params[idx::RESO_INV - 1] = 64;
        params[idx::ENV_AMT - 1] = 64;
        params[idx::LFO_TARGET - 1] = 2;
        params[idx::ATTACK - 1] = 25;
        params[idx::STEP_GATE - 1] = 256;
        Settings {
            sixteenth: 5512,
            delay_ticks: 48,
            delay_feedback: 64,
            delay_pan: 64,
            delay_volume: 128,
            pan: 64,
            params,
            hq: false,
            step_len: 5512,
            gate: 256,
            fl_slides: false,
        }
    }
}

impl Settings {
    fn delay(&self, chan_vol: f32) -> DelaySettings {
        DelaySettings {
            length: ((self.sixteenth.max(1) as i64 * self.delay_ticks.clamp(0, 768) as i64) / 48) as usize,
            feedback: self.delay_feedback,
            pan: self.delay_pan,
            volume: self.delay_volume,
            amount: self.params[idx::DELAY_AMT - 1],
            chan_vol,
        }
    }
}

pub struct Synth {
    tables: Arc<Tables>,
    levels: LevelTables,
    voice: MonoVoice,
    delay: Delay,
    custom: Box<Wave>,
    has_custom: bool,
    /// Channel volume the delay send is scaled by: 1/pan[centre], so a centred channel's
    /// dry signal is the voice itself while the dry/wet balance stays the TS404's.
    chan_vol: f32,
    resampler: Option<Resampler>,
    resampler_r: Option<Resampler>,
    sample_rate: u32,
    /// Engine samples rendered so far: the event clock.
    clock: u64,
    events: Box<[(u64, Ev); MAX_EVENTS]>,
    n_events: usize,
    /// Notes held right now in host time (playback may be delayed behind it).
    rt_held: [u8; MAX_HELD],
    n_rt_held: usize,
    /// Fixed playback delay for TS404 slide timing, in engine samples (0 when off).
    lookahead: u64,
    engine_buf: Vec<f32>,
    engine_l: Vec<f32>,
    engine_r: Vec<f32>,
}

impl Synth {
    /// `lookahead`: engine-sample delay used for TS404 slide timing (see `lookahead_for`).
    /// It is fixed for the lifetime of the instance so the reported latency never moves
    /// while the host is processing.
    pub fn new(tables: Arc<Tables>, sample_rate: u32, max_block: usize, lookahead: u64) -> Synth {
        let resampler = (sample_rate != ENGINE_RATE).then(|| Resampler::new(sample_rate, max_block));
        let resampler_r = (sample_rate != ENGINE_RATE).then(|| Resampler::new(sample_rate, max_block));
        let engine_len = match &resampler {
            Some(_) => (max_block as f64 * ENGINE_RATE as f64 / sample_rate as f64).ceil() as usize + 2 * HALF + 8,
            None => max_block,
        };
        let levels = LevelTables::generate();
        let chan_vol = crate::x87::e(1.0).div(crate::x87::e(levels.pan[64])).to_f32();
        Synth {
            tables,
            levels,
            voice: MonoVoice::default(),
            delay: Delay::new(),
            custom: vec![0f32; WAVE_LEN].into_boxed_slice().try_into().unwrap(),
            has_custom: false,
            chan_vol,
            resampler,
            resampler_r,
            sample_rate,
            clock: 0,
            events: Box::new([(0, Ev::Off(0)); MAX_EVENTS]),
            n_events: 0,
            rt_held: [0; MAX_HELD],
            n_rt_held: 0,
            lookahead,
            engine_buf: vec![0.0; engine_len],
            engine_l: vec![0.0; engine_len],
            engine_r: vec![0.0; engine_len],
        }
    }

    /// The "?" oscillator shape (None = the saw). Copies; no allocation.
    pub fn set_custom_wave(&mut self, w: Option<&Wave>) {
        match w {
            Some(w) => {
                self.custom.copy_from_slice(&w[..]);
                self.has_custom = true;
            }
            None => self.has_custom = false,
        }
    }

    /// Lookahead needed for TS404 slide timing with this step length (engine samples).
    pub fn lookahead_for(fl_slides: bool, step_len: i32) -> u64 {
        if fl_slides { step_len.max(1) as u64 } else { 0 }
    }

    pub fn lookahead(&self) -> u64 {
        self.lookahead
    }

    /// Whether these settings need a different lookahead than this instance was built
    /// with (the host then has to restart the plugin with the new latency). A longer
    /// lookahead than needed is fine, so this only triggers for a shorter one when the
    /// excess is large.
    pub fn needs_new_lookahead(&self, s: &Settings) -> bool {
        let want = Self::lookahead_for(s.fl_slides, s.step_len);
        want > self.lookahead || (want == 0) != (self.lookahead == 0) || want * 2 < self.lookahead
    }

    /// Latency in host samples.
    pub fn latency(&self) -> u32 {
        Self::latency_for(self.sample_rate, self.lookahead)
    }

    /// Latency (host samples) an instance at `sample_rate` with `lookahead` reports.
    pub fn latency_for(sample_rate: u32, lookahead: u64) -> u32 {
        let res = if sample_rate == ENGINE_RATE { 0 } else { Resampler::latency_at(sample_rate) };
        res + (lookahead * sample_rate as u64).div_ceil(ENGINE_RATE as u64) as u32
    }

    pub fn reset(&mut self) {
        self.voice = MonoVoice::default();
        self.delay.reset();
        if let Some(r) = &mut self.resampler {
            r.reset();
        }
        if let Some(r) = &mut self.resampler_r {
            r.reset();
        }
        self.clock = 0;
        self.n_events = 0;
        self.n_rt_held = 0;
    }

    fn event_time(&self, offset: u32) -> u64 {
        match &self.resampler {
            Some(r) => r.event_time(offset),
            None => self.clock + offset as u64,
        }
    }

    fn queue(&mut self, at: u64, ev: Ev) {
        if self.n_events == MAX_EVENTS {
            // Never drop events (a lost note-off would hang a note): apply the oldest early.
            let first = self.events[0].1;
            self.events.copy_within(1..self.n_events, 0);
            self.n_events -= 1;
            self.apply(first);
        }
        let mut i = self.n_events;
        while i > 0 && self.events[i - 1].0 > at {
            self.events[i] = self.events[i - 1];
            i -= 1;
        }
        self.events[i] = (at, ev);
        self.n_events += 1;
    }

    fn rt_remove(&mut self, note: u8) {
        if let Some(i) = self.rt_held[..self.n_rt_held].iter().position(|&x| x == note) {
            self.rt_held.copy_within(i + 1..self.n_rt_held, i);
            self.n_rt_held -= 1;
        }
    }

    /// Note-on `offset` samples into the next `process` block. `fl_slides`/`step_len`
    /// must be the values the block will be processed with.
    pub fn note_on(&mut self, offset: u32, note: u8, s: &Settings) {
        let at = self.event_time(offset);
        let legato = self.n_rt_held > 0;
        self.rt_remove(note);
        if self.n_rt_held == MAX_HELD {
            self.rt_held.copy_within(1.., 0); // forget the oldest key
            self.n_rt_held -= 1;
        }
        self.rt_held[self.n_rt_held] = note;
        self.n_rt_held += 1;
        let d = self.lookahead;
        if s.fl_slides && d > 0 && legato {
            let ahead = (s.step_len.max(1) as u64).min(d);
            self.queue(at + d - ahead, Ev::SlideAhead(note));
            self.queue(at + d, Ev::Push(note));
        } else {
            self.queue(at + d, Ev::On(note));
        }
    }

    pub fn note_off(&mut self, offset: u32, note: u8) {
        let at = self.event_time(offset);
        self.rt_remove(note);
        self.queue(at + self.lookahead, Ev::Off(note));
    }

    fn apply(&mut self, ev: Ev) {
        match ev {
            Ev::On(n) => self.voice.note_on(n),
            Ev::Off(n) => self.voice.note_off(n),
            Ev::SlideAhead(n) => self.voice.slide_ahead(n),
            Ev::Push(n) => self.voice.push_held(n),
        }
    }

    /// Voice for `len` engine samples into `engine_buf`, applying events at their sample.
    fn generate(&mut self, len: usize) {
        let custom = if self.has_custom { Some(&*self.custom) } else { None };
        let mut pos = 0;
        while pos < len {
            while self.n_events > 0 && self.events[0].0 <= self.clock {
                let ev = self.events[0].1;
                self.events.copy_within(1..self.n_events, 0);
                self.n_events -= 1;
                match ev {
                    Ev::On(n) => self.voice.note_on(n),
                    Ev::Off(n) => self.voice.note_off(n),
                    Ev::SlideAhead(n) => self.voice.slide_ahead(n),
                    Ev::Push(n) => self.voice.push_held(n),
                }
            }
            let until = if self.n_events > 0 { (self.events[0].0 - self.clock) as usize } else { usize::MAX };
            let n = (len - pos).min(until);
            self.voice.render(&self.tables, &mut self.engine_buf[pos..pos + n], custom);
            self.clock += n as u64;
            pos += n;
        }
    }

    /// Channel stage: pan the voice (circular law, normalised so centre = unity)
    /// and run the TS404 delay line. Writes `engine_l/r[..len]`.
    fn mix(&mut self, s: &Settings, len: usize) {
        let off = s.pan.clamp(0, 128) - 64;
        let (gl, gr) = if off == 0 {
            (1.0f32, 1.0f32)
        } else {
            let c = self.levels.pan[64] as f64;
            ((self.levels.pan[(64 + off) as usize] as f64 / c) as f32, (self.levels.pan[(64 - off) as usize] as f64 / c) as f32)
        };
        let ds = s.delay(self.chan_vol);
        for i in 0..len {
            let v = self.engine_buf[i];
            let (mut l, mut r) = if off == 0 { (v, v) } else { (v * gl, v * gr) };
            if self.delay.idle(&ds) {
                self.delay.idle_update(&ds, &self.levels);
            } else {
                self.delay.tick(v, &ds, &self.levels, &mut l, &mut r);
            }
            self.engine_l[i] = l;
            self.engine_r[i] = r;
        }
    }

    /// Render one host block (stereo). Settings apply from the start of the block, like
    /// the TS404's per-block parameter updates.
    pub fn process(&mut self, s: &Settings, out_l: &mut [f32], out_r: &mut [f32]) {
        self.voice.engine.set_params(&s.params);
        if self.has_custom {
            self.voice.engine.p[idx::CUSTOM_WAVE] = 1;
        }
        self.voice.hq = s.hq;
        self.voice.set_step_len(s.step_len);
        self.voice.set_gate(s.gate);
        let n = out_l.len();
        match (self.resampler.take(), self.resampler_r.take()) {
            (Some(mut rl), Some(mut rr)) => {
                let need = rl.input_needed(n) as usize;
                self.generate(need);
                self.mix(s, need);
                rl.push(&self.engine_l[..need]);
                rr.push(&self.engine_r[..need]);
                rl.produce(out_l);
                rr.produce(out_r);
                self.resampler = Some(rl);
                self.resampler_r = Some(rr);
            }
            _ => {
                self.generate(n);
                self.mix(s, n);
                out_l.copy_from_slice(&self.engine_l[..n]);
                out_r.copy_from_slice(&self.engine_r[..n]);
            }
        }
    }

    pub fn is_silent(&self) -> bool {
        self.voice.is_silent() && self.n_events == 0
    }

    /// Delay length (engine samples) these settings ask for.
    pub fn delay_len(s: &Settings) -> usize {
        s.delay(1.0).length
    }
}
