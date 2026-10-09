//! Host-independent FL 3.5 / FL 6 synth: MIDI notes become FL note events on FL's tick
//! grid (24 ticks per step at the host tempo), FL's channel engine renders at 44.1 kHz in
//! blocks that start at ticks, and the result is resampled to the host rate.
//!
//! A MIDI note starts at the nearest tick that has not started yet. Its end is not known
//! until the note-off, so it starts open-ended and gets its end then: one tick before
//! the note-off's tick, the way FL's piano roll ends a note (the channel engines make a
//! note ended this way play exactly like one queued with that end).

use std::sync::Arc;

use crate::channel3::{velocity_level, Channel3, Event};
use crate::channel6::{Channel6, Event6};
use crate::engine3::Engine3;
use crate::flchan::{ChannelKnobs, TICKS_PER_STEP};
use crate::resample::{ENGINE_RATE, HALF, Resampler};
use crate::tables::{Tables, WAVE_LEN, Wave};
use crate::tables3::{Fl3, Tables3};
use crate::x87::{e, ei};

/// Note end for a note that is still held (far enough away never to be reached).
const OPEN: i32 = 0x4000_0000;

enum Chan {
    V35(Box<Channel3>),
    V6(Box<Channel6>),
}

/// Per-block settings.
#[derive(Clone, Debug)]
pub struct FlSettings {
    pub version: Fl3,
    pub knobs: ChannelKnobs,
    /// Host tempo (BPM).
    pub tempo: f64,
    /// Host transport position in quarter notes while playing (keeps FL's tick grid on
    /// the host's beat grid).
    pub song_pos: Option<f64>,
    /// FL 6's high-quality distortion.
    pub hq: bool,
    /// FL's "alias-free TS404" (its WAV export option).
    pub aa: bool,
}

pub struct FlSynth {
    tables: Arc<Tables>,
    t3: [Box<Tables3>; 2],
    chan: Chan,
    version: Fl3,
    knobs: ChannelKnobs,
    resampler: Option<Resampler>,
    resampler_r: Option<Resampler>,
    /// Engine samples rendered so far.
    clock: u64,
    /// Where the next tick starts (FL accumulates samples per tick in a double).
    tick_acc: f64,
    boundary: u64,
    spt: f32,
    /// Held MIDI keys: (note id, start tick).
    live: [Option<(i32, i32)>; 128],
    note_count: u32,
    custom: Box<Wave>,
    has_custom: bool,
    engine_l: Vec<f32>,
    engine_r: Vec<f32>,
}

impl FlSynth {
    pub fn new(tables: Arc<Tables>, sample_rate: u32, max_block: usize, version: Fl3) -> FlSynth {
        let resampler = (sample_rate != ENGINE_RATE).then(|| Resampler::new(sample_rate, max_block));
        let resampler_r = (sample_rate != ENGINE_RATE).then(|| Resampler::new(sample_rate, max_block));
        let engine_len = match &resampler {
            Some(_) => (max_block as f64 * ENGINE_RATE as f64 / sample_rate as f64).ceil() as usize + 2 * HALF + 8,
            None => max_block,
        };
        let knobs = ChannelKnobs::new(version);
        let mut s = FlSynth {
            tables,
            t3: [Box::new(Tables3::generate(Fl3::V35)), Box::new(Tables3::generate(Fl3::V6))],
            chan: Self::make_chan(version, &knobs),
            version,
            knobs,
            resampler,
            resampler_r,
            clock: 0,
            tick_acc: 0.0,
            boundary: 0,
            spt: Self::samples_per_tick(120.0),
            live: [None; 128],
            note_count: 0,
            custom: vec![0f32; WAVE_LEN].into_boxed_slice().try_into().unwrap(),
            has_custom: false,
            engine_l: vec![0.0; engine_len],
            engine_r: vec![0.0; engine_len],
        };
        s.apply_knobs(&ChannelKnobs::new(version), true);
        s
    }

    fn make_chan(v: Fl3, k: &ChannelKnobs) -> Chan {
        let mut eng = Engine3::new(v);
        eng.p[1..34].copy_from_slice(&k.ts[1..34]);
        match v {
            Fl3::V35 => {
                let mut c = Channel3::new(eng);
                c.s = k.settings3();
                Chan::V35(Box::new(c))
            }
            Fl3::V6 => {
                let mut c = Channel6::new(eng);
                c.s = k.settings6();
                Chan::V6(Box::new(c))
            }
        }
    }

    /// FL's samples per tick at `tempo`: rate·15 / (24·tempo), as f32.
    fn samples_per_tick(tempo: f64) -> f32 {
        (ENGINE_RATE as f64 * 15.0 / (TICKS_PER_STEP as f64 * tempo.clamp(10.0, 999.0))) as f32
    }

    /// Latency in host samples (the resampler's).
    pub fn latency(&self) -> u32 {
        self.resampler.as_ref().map_or(0, |r| r.latency())
    }

    pub fn version(&self) -> Fl3 {
        self.version
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

    pub fn reset(&mut self) {
        self.chan = Self::make_chan(self.version, &self.knobs);
        if let Some(r) = &mut self.resampler {
            r.reset();
        }
        if let Some(r) = &mut self.resampler_r {
            r.reset();
        }
        self.clock = 0;
        self.tick_acc = 0.0;
        self.boundary = 0;
        self.live = [None; 128];
    }

    fn tick(&self) -> i32 {
        match &self.chan {
            Chan::V35(c) => c.tick(),
            Chan::V6(c) => c.tick(),
        }
    }

    fn event_time(&self, offset: u32) -> u64 {
        match &self.resampler {
            Some(r) => r.event_time(offset),
            None => self.clock + offset as u64,
        }
    }

    /// The tick a MIDI event at engine sample `t` belongs to: the nearest tick start
    /// that is still ahead.
    fn tick_at(&self, t: u64) -> i32 {
        let next = self.tick().wrapping_add(1);
        let ahead = ((t as f64 - self.boundary as f64) / self.spt as f64).round();
        next.wrapping_add(ahead.max(0.0) as i32)
    }

    fn apply_knobs(&mut self, k: &ChannelKnobs, force: bool) {
        if !force && *k == self.knobs {
            return;
        }
        match &mut self.chan {
            Chan::V35(c) => {
                c.s = k.settings3();
                c.synth.p[1..34].copy_from_slice(&k.ts[1..34]);
            }
            Chan::V6(c) => {
                c.s = k.settings6();
                c.synth.p[1..34].copy_from_slice(&k.ts[1..34]);
                c.settings_changed();
            }
        }
        self.knobs = k.clone();
    }

    /// Settings that apply from the next event on (call before the block's events).
    pub fn set_settings(&mut self, s: &FlSettings) {
        if s.version != self.version {
            self.version = s.version;
            self.knobs = s.knobs.clone();
            self.chan = Self::make_chan(s.version, &s.knobs);
            self.live = [None; 128];
        }
        self.apply_knobs(&s.knobs, false);
        match &mut self.chan {
            Chan::V35(c) => c.aa = s.aa,
            Chan::V6(c) => {
                c.aa = s.aa;
                c.hq = s.hq;
            }
        }
        let spt = Self::samples_per_tick(s.tempo);
        if spt.to_bits() != self.spt.to_bits() {
            self.spt = spt;
        }
        // keep the tick grid on the host's: re-align when the transport jumps
        if let Some(pos) = s.song_pos {
            let t0 = self.event_time(0) as f64;
            let ticks = pos * 4.0 * TICKS_PER_STEP as f64;
            let frac = ticks - ticks.floor();
            let want = t0 + if frac > 1e-9 { (1.0 - frac) * spt as f64 } else { 0.0 };
            let have = self.tick_acc.max(self.clock as f64);
            if (want - have).abs() > 4.0 && want >= self.clock as f64 {
                self.tick_acc = want;
                self.boundary = want.round_ties_even() as u64;
            }
        }
    }

    /// Note-on `offset` samples into the next `process` block; `velocity` 0..1.
    pub fn note_on(&mut self, offset: u32, note: u8, velocity: f32) {
        let key = note as usize & 127;
        if self.live[key].is_some() {
            self.note_off(offset, note);
        }
        let tick = self.tick_at(self.event_time(offset)).wrapping_add(self.knobs.shift_ticks());
        self.note_count = self.note_count.wrapping_add(1) % 0x00ff_ffff;
        let id = (self.note_count as i32 + 1) * 64;
        let vel = (velocity.clamp(0.0, 1.0) * 127.0).round() as i32;
        let pitch = note as i32 * 100;
        match &mut self.chan {
            Chan::V35(c) => {
                let mut ev = Event::note(tick, OPEN, id, pitch, velocity_level(vel), 0);
                ev.arp = if c.s.arp_dir != 0 { 0 } else { -1 };
                c.queue(ev);
            }
            Chan::V6(c) => {
                let vol = ei(vel).div(e(100.0)).to_f32();
                c.queue(Event6::note(tick, OPEN, id, pitch as f32, vol, 0));
            }
        }
        self.live[key] = Some((id, tick));
    }

    pub fn note_off(&mut self, offset: u32, note: u8) {
        let Some((id, start)) = self.live[note as usize & 127].take() else { return };
        let off = self.tick_at(self.event_time(offset)).wrapping_add(self.knobs.shift_ticks());
        let end = off.wrapping_sub(1).max(start);
        match &mut self.chan {
            Chan::V35(c) => c.end_note(id, start, end),
            Chan::V6(c) => c.end_note(id, start, end),
        }
    }

    pub fn all_notes_off(&mut self, offset: u32) {
        for n in 0..128u8 {
            self.note_off(offset, n);
        }
    }

    /// Output level that puts a new channel (volume 100, centred) at unity.
    fn norm(&self) -> f32 {
        let d = ChannelKnobs::new(self.version);
        let v = match self.version {
            Fl3::V35 => d.settings3().vol,
            Fl3::V6 => d.settings6().vol,
        };
        (1.0 / (2.0 * v as f64 * std::f64::consts::FRAC_1_SQRT_2)) as f32
    }

    fn generate(&mut self, len: usize) {
        let custom = if self.has_custom { Some(&*self.custom) } else { None };
        let t = &self.tables;
        let mut pos = 0;
        while pos < len {
            let new_tick = self.clock >= self.boundary;
            if new_tick {
                self.tick_acc += self.spt as f64;
                self.boundary = self.tick_acc.round_ties_even().max(self.clock as f64 + 1.0) as u64;
            }
            let n = (len - pos).min((self.boundary - self.clock) as usize);
            let (l, r) = (&mut self.engine_l[pos..pos + n], &mut self.engine_r[pos..pos + n]);
            match &mut self.chan {
                Chan::V35(c) => c.block(t, &self.t3[0], new_tick, l, r, custom),
                Chan::V6(c) => c.block(t, &self.t3[1], new_tick, l, r, custom),
            }
            self.clock += n as u64;
            pos += n;
        }
        let g = self.norm();
        for x in self.engine_l[..len].iter_mut().chain(self.engine_r[..len].iter_mut()) {
            *x *= g;
        }
    }

    /// Render one host block (stereo).
    pub fn process(&mut self, out_l: &mut [f32], out_r: &mut [f32]) {
        let n = out_l.len();
        match (self.resampler.take(), self.resampler_r.take()) {
            (Some(mut rl), Some(mut rr)) => {
                let need = rl.input_needed(n) as usize;
                self.generate(need);
                rl.push(&self.engine_l[..need]);
                rr.push(&self.engine_r[..need]);
                rl.produce(out_l);
                rr.produce(out_r);
                self.resampler = Some(rl);
                self.resampler_r = Some(rr);
            }
            _ => {
                self.generate(n);
                out_l.copy_from_slice(&self.engine_l[..n]);
                out_r.copy_from_slice(&self.engine_r[..n]);
            }
        }
    }

    /// Whether nothing sounds or is waiting to.
    pub fn is_silent(&self) -> bool {
        match &self.chan {
            Chan::V35(c) => c.active_voices() == 0 && c.synth.is_idle(),
            Chan::V6(c) => c.active_voices() == 0 && c.synth.is_idle(),
        }
    }
}
