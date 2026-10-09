//! FL 3.5's channel engine around one TS404: notes become voices, and once per tick
//! each voice writes its pan, volume, pitch, cutoff and resonance into the synth (the
//! last voice wins). Echo delay, arpeggiator, mono/portamento glides, gate, keyboard
//! tracking and the channel mixer (pan law and gain ramps) are FL's.
//!
//! Ticks are 1/24 of a step. FL processes audio in blocks that end at ticks: events due
//! at a block's tick play first, then the voices update, then the mixer renders.

use crate::engine3::Engine3;
use crate::tables::Tables;
use crate::tables3::Tables3;
use crate::x87::{e, ei};

/// Note end meaning "until it is released".
pub const HELD: i32 = 0x7fff_fffd;
const NO_END: i32 = 0x7fff_fffe;
const NEVER: i32 = 0x7fff_ffff;
const NO_ARP_SLIDE: u32 = 0x7fff_ffff;
pub const MAX_VOICES: usize = 512;

/// Event flags (bits 0..3 are the layer).
pub const F_SLIDE: u32 = 0x10;
pub const F_GENERATED: u32 = 0x1_0000;
pub const F_ECHO: u32 = 0x2_0000;

/// FL 3's arpeggiator chords (its Chords.map: intervals in semitones, stacked).
pub const CHORDS: &[(&str, &[i8])] = &[
    ("Major", &[4, 3]), ("sus2", &[2, 5]), ("sus4", &[5, 2]), ("Majb5", &[4, 2]), ("minor", &[3, 4]),
    ("mb5", &[3, 3]), ("aug", &[4, 4]), ("augsus4", &[5, 3]), ("tri", &[3, 3, 3]),
    ("6", &[4, 3, 2]), ("6sus4", &[5, 2, 2]), ("6add9", &[4, 3, 2, 5]), ("m6", &[3, 4, 2]), ("m6add9", &[3, 4, 2, 5]),
    ("7", &[4, 3, 3]), ("7sus4", &[5, 2, 3]), ("7#5", &[4, 4, 2]), ("7b5", &[4, 2, 4]), ("7#9", &[4, 3, 3, 5]),
    ("7b9", &[4, 3, 3, 3]), ("7#5#9", &[4, 4, 2, 5]), ("7#5b9", &[4, 4, 2, 3]), ("7b5b9", &[4, 2, 4, 3]),
    ("7add11", &[4, 3, 3, 7]), ("7add13", &[4, 3, 3, 11]), ("7#11", &[4, 3, 3, 8]), ("Maj7", &[4, 3, 4]),
    ("Maj7b5", &[4, 2, 5]), ("Maj7#5", &[4, 4, 3]), ("Maj7#11", &[4, 3, 4, 7]), ("Maj7add13", &[4, 3, 4, 10]),
    ("m7", &[3, 4, 3]), ("m7b5", &[3, 3, 4]), ("m7b9", &[3, 4, 3, 3]), ("m7add11", &[3, 4, 3, 7]),
    ("m7add13", &[3, 4, 3, 11]), ("m-Maj7", &[3, 4, 4]), ("m-Maj7add11", &[3, 4, 4, 6]), ("m-Maj7add13", &[3, 4, 4, 10]),
    ("9", &[4, 3, 3, 4]), ("9sus4", &[5, 2, 3, 4]), ("add9", &[4, 3, 7]), ("9#5", &[4, 4, 2, 4]), ("9b5", &[4, 2, 4, 4]),
    ("9#11", &[4, 3, 3, 4, 4]), ("9b13", &[4, 3, 3, 4, 6]), ("Maj9", &[4, 3, 4, 3]), ("Maj9sus4", &[5, 2, 4, 3]),
    ("Maj9#5", &[4, 4, 3, 3]), ("Maj9#11", &[4, 3, 4, 3, 4]), ("m9", &[3, 4, 3, 4]), ("madd9", &[3, 4, 7]),
    ("m9b5", &[3, 3, 4, 4]), ("m9-Maj7", &[3, 4, 4, 3]),
    ("11", &[4, 3, 3, 4, 3]), ("11b9", &[4, 3, 3, 3, 4]), ("Maj11", &[4, 3, 4, 3, 3]), ("m11", &[3, 4, 3, 4, 3]),
    ("m-Maj11", &[3, 4, 4, 3, 3]),
    ("13", &[4, 3, 3, 4, 7]), ("13#9", &[4, 3, 3, 5, 6]), ("13b9", &[4, 3, 3, 3, 8]), ("13b5b9", &[4, 2, 4, 3, 8]),
    ("Maj13", &[4, 3, 4, 3, 7]), ("m13", &[3, 4, 3, 4, 7]), ("m-Maj13", &[3, 4, 4, 3, 7]),
];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NoteParams {
    /// -64..=64 relative to the channel's pan.
    pub pan: i32,
    /// Velocity as a level (velocity / 100).
    pub vol: f32,
    /// Cents (key · 100).
    pub pitch: i32,
    /// Cutoff and resonance offsets added to the channel's (0..1 scale).
    pub modx: f32,
    pub mody: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct Event {
    pub time: i32,
    pub end: i32,
    pub release_id: i32,
    pub id: i32,
    pub p: NoteParams,
    /// Fine pitch in cents, added on top (FL's per-note fine pitch).
    pub fine: i32,
    pub flags: u32,
    /// Arpeggio step (-1: no arpeggio).
    pub arp: i32,
    /// When the note this event comes from (or its echo) starts. Not FL's: it lets a live
    /// note end the way a note of known length would (`Channel3::end_note`).
    pub origin: i32,
}

impl Event {
    pub fn note(time: i32, end: i32, id: i32, pitch: i32, vol: f32, flags: u32) -> Event {
        Event { time, end, release_id: 0, id, p: NoteParams { pan: 0, vol, pitch, modx: 0.0, mody: 0.0 }, fine: 0, flags, arp: -1, origin: time }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    id: i32,
    key: i32,
    start: i32,
    end: i32,
    p: NoteParams,
    pan_t: i32,
    vol_t: f32,
    pitch_t: i32,
    modx_t: f32,
    mody_t: f32,
    fine: i32,
    slide_n: i32,
    pan_f: f32,
    pitch_f: f32,
    d_pan: f32,
    d_vol: f32,
    d_modx: f32,
    d_mody: f32,
    d_pitch: f32,
    arp: i32,
    arp_next: i32,
    arp_end: i32,
    arp_pitch: i32,
    arp_pf: f32,
    arp_inc: f32,
    flags: u32,
    layer: u8,
    state: i32,
    origin: i32,
}

/// The channel's settings, in FL's units (see `Channel3::update_totals` for the knobs).
#[derive(Clone, Debug)]
pub struct ChannelSettings {
    /// Pan total (-64..64), volume total, pitch total (cents), cutoff and resonance totals (0..1).
    pub pan: i32,
    pub vol: f32,
    pub pitch: i32,
    pub modx: f32,
    pub mody: f32,
    /// Keyboard/velocity tracking: (velocity mid, key mid) and their pan/modX/modY amounts.
    pub tracking: bool,
    pub track_vel_mid: f32,
    pub track_key_mid: f32,
    pub track_vel: [f32; 3],
    pub track_key: [f32; 3],
    /// Notes outside this key range don't play.
    pub key_lo: i32,
    pub key_hi: i32,
    pub max_poly: i32,
    pub mono: bool,
    pub porta: bool,
    /// Glide length in ticks (0: none).
    pub porta_ticks: i32,
    /// Gate in ticks (`None`: off) and whether slide notes bypass it.
    pub gate_ticks: Option<i32>,
    pub gate_skips_slides: bool,
    /// Echo: feedback 0..256, pan 0..256 (128 = none), pitch (cents), echoes, time
    /// (1/48 step), cutoff and resonance 0..256 (128 = none), ping-pong, pan bounce.
    pub echo_feed: i32,
    pub echo_pan: i32,
    pub echo_pitch: i32,
    pub echoes: i32,
    pub echo_time: i32,
    pub echo_cut: i32,
    pub echo_res: i32,
    pub echo_pingpong: bool,
    pub echo_bounce: bool,
    /// Arpeggiator: direction (0 off, 1 up, 2 down, 3 up+down, 4 up+down with
    /// repeat, 5 random), range (octaves), chord (0 none, else 1-based into `CHORDS`),
    /// time knob (FL's 450..1447), time in ticks, gate (0..48), slide.
    pub arp_dir: i32,
    pub arp_range: i32,
    pub arp_chord: i32,
    pub arp_time_knob: i32,
    pub arp_time_ticks: i32,
    pub arp_gate: i32,
    pub arp_slide: bool,
    /// Ticks per step (FL: 24).
    pub ticks_per_step: i32,
}

impl Default for ChannelSettings {
    fn default() -> Self {
        ChannelSettings {
            pan: 0,
            vol: 0.781_25,
            pitch: 0,
            modx: 0.5,
            mody: 0.25,
            tracking: false,
            track_vel_mid: 0.0,
            track_key_mid: 0.0,
            track_vel: [0.0; 3],
            track_key: [0.0; 3],
            key_lo: 0,
            key_hi: 131,
            max_poly: 0,
            mono: false,
            porta: false,
            porta_ticks: 0,
            gate_ticks: None,
            gate_skips_slides: false,
            echo_feed: 0,
            echo_pan: 128,
            echo_pitch: 0,
            echoes: 4,
            echo_time: 144,
            echo_cut: 128,
            echo_res: 128,
            echo_pingpong: false,
            echo_bounce: false,
            arp_dir: 0,
            arp_range: 1,
            arp_chord: 0,
            arp_time_knob: 620,
            arp_time_ticks: 6,
            arp_gate: 48,
            arp_slide: false,
            ticks_per_step: 24,
        }
    }
}

/// FL's pan law tables (index 0..=128, 64 = centre): circular is cos(i·pi/256).
pub fn pan_table(circular: bool) -> [f32; 129] {
    let mut t = [0f32; 129];
    if circular {
        let step = f32::from_bits(0x3c49_0fdb); // pi / 256
        for (i, v) in t.iter_mut().enumerate() {
            *v = crate::fpu::cos(ei(i as i32).mul(e(step))).to_f32();
        }
    } else {
        for v in t.iter_mut().take(65) {
            *v = 1.0;
        }
        for i in 0..64 {
            t[128 - i] = ei(i as i32).div(e(64.0)).to_f32();
        }
    }
    t
}

/// FL's velocity levels (linear levels: velocity / 100).
pub fn velocity_level(vel: i32) -> f32 {
    ei(vel.clamp(0, 128)).div(e(100.0)).to_f32()
}

pub struct Channel3 {
    pub synth: Engine3,
    pub s: ChannelSettings,
    voices: Vec<Voice>,
    n_voices: usize,
    /// Pending events, descending time (FL pops from the end).
    events: Vec<Event>,
    /// Last note per layer (glide source); pitch `NEVER` = none yet.
    last: [NoteParams; 16],
    tick: i32,
    gain_l: f32,
    gain_r: f32,
    pan_tab: [f32; 129],
    rand_seed: u32,
    /// Whether FL's "alias-free TS404" (export option) is on.
    pub aa: bool,
}

/// FL's linear ramp of a gain (or cutoff) toward the new value, 3.5 style.
fn ramp35(cur: &mut f32, old: f32, n: i32, min_jump: f32) -> f32 {
    let mut inc = e(*cur).sub(e(old)).to_f32();
    if inc.to_bits() == 0 {
        return inc;
    }
    let absd = e(inc).abs().to_f32();
    inc = e(inc).div(ei(n)).to_f32();
    if !e(absd).gt(e(min_jump)) {
        return inc;
    }
    let lim = e(absd).mul(e(f32::from_bits(0x3b14_9b93))).to_f32();
    if !e(inc).abs().gt(e(lim)) {
        return inc;
    }
    inc = f32::from_bits((inc.to_bits() & 0x8000_0000) | (lim.to_bits() & 0x7fff_ffff));
    *cur = ei(n).mul(e(inc)).add(e(old)).to_f32();
    if !e(*cur).abs().ge(e(f32::from_bits(0x3380_0001))) {
        *cur = 0.0;
        inc = e(old).neg().div(ei(n)).to_f32();
    }
    inc
}

impl Channel3 {
    pub fn new(synth: Engine3) -> Channel3 {
        let mut last = [NoteParams::default(); 16];
        for l in last.iter_mut() {
            l.pitch = NEVER;
        }
        Channel3 {
            synth,
            s: ChannelSettings::default(),
            voices: vec![Voice::default(); MAX_VOICES],
            n_voices: 0,
            events: Vec::with_capacity(4096),
            last,
            tick: 0,
            gain_l: 0.0,
            gain_r: 0.0,
            pan_tab: pan_table(true),
            rand_seed: 0,
            aa: false,
        }
    }

    pub fn set_pan_law(&mut self, circular: bool) {
        self.pan_tab = pan_table(circular);
    }

    pub fn tick(&self) -> i32 {
        self.tick
    }

    pub fn active_voices(&self) -> usize {
        self.n_voices
    }

    /// FL's Random(n) (Delphi's generator).
    fn random(&mut self, n: u32) -> u32 {
        self.rand_seed = self.rand_seed.wrapping_mul(0x0808_8405).wrapping_add(1);
        ((n as u64 * self.rand_seed as u64) >> 32) as u32
    }

    // ------------------------------------------------------------------ events

    /// FL's event queue: an event due at the current tick plays right away,
    /// anything else waits in the list (equal times play newest first).
    pub fn queue(&mut self, ev: Event) {
        if ev.time == self.tick && ev.release_id != NEVER {
            self.note_on(&ev);
            return;
        }
        let mut i = self.events.len();
        while i > 0 && self.events[i - 1].time < ev.time {
            i -= 1;
        }
        self.events.insert(i, ev);
    }

    /// A live note (ids `id..id + 64`: the note and its echoes) that started at `start`
    /// ends at `end`: it, its echoes and arpeggio notes end as if it had been queued with
    /// that end, and arpeggio notes that would not have been queued are dropped.
    pub fn end_note(&mut self, id: i32, start: i32, end: i32) {
        let ours = |x: i32| (x.wrapping_sub(id) as u32) < 64;
        for v in self.voices[..self.n_voices].iter_mut() {
            if ours(v.id) {
                v.end = v.end.min(end.wrapping_add(v.origin.wrapping_sub(start)));
            }
        }
        self.events.retain_mut(|ev| {
            if ours(ev.id) {
                ev.end = ev.end.min(end.wrapping_add(ev.origin.wrapping_sub(start)));
                return ev.time <= ev.end;
            }
            true
        });
    }

    /// Forget all notes (the synth keeps its state).
    pub fn clear_notes(&mut self) {
        self.events.clear();
        self.n_voices = 0;
        for l in self.last.iter_mut() {
            *l = NoteParams { pitch: NEVER, ..Default::default() };
        }
    }

    // ------------------------------------------------------------------ voices

    fn release(&mut self, i: usize) {
        if self.voices[i].state < 1 {
            self.voices[i].state = 1;
            self.synth.note_off();
        }
    }

    fn kill(&mut self, i: usize) {
        if self.voices[i].state < 2 {
            self.voices[i].state = 2;
        }
    }

    /// Glide set-up: per-tick steps from the voice's note to `to`.
    fn slide_to(&mut self, i: usize, to: NoteParams, ticks: i32, pitch: i32) {
        if self.s.echo_feed >= 1 {
            return;
        }
        let v = &mut self.voices[i];
        v.slide_n = ticks.wrapping_add(1);
        let len = ei(v.slide_n).to_f32();
        let delta = |new: f32, old: f32| if e(old).cmp(e(new)) == Some(core::cmp::Ordering::Equal) { 0.0 } else { e(new).sub(e(old)).div(e(len)).to_f32() };
        v.d_pan = delta(ei(to.pan).to_f32(), ei(v.p.pan).to_f32());
        v.d_vol = delta(to.vol, v.p.vol);
        v.d_pitch = delta(ei(pitch).to_f32(), ei(v.p.pitch).to_f32());
        v.d_modx = delta(to.modx, v.p.modx);
        v.d_mody = delta(to.mody, v.p.mody);
        v.pan_f = ei(v.p.pan).to_f32();
        v.pitch_f = ei(v.p.pitch).to_f32();
        let slot = &mut self.last[v.layer as usize & 15];
        *slot = to;
        slot.pitch = pitch;
    }

    /// One glide step.
    fn slide_step(v: &mut Voice) {
        if v.d_pan.to_bits() != 0 {
            v.pan_f = e(v.pan_f).add(e(v.d_pan)).to_f32();
        }
        if v.d_vol.to_bits() != 0 {
            v.p.vol = e(v.p.vol).add(e(v.d_vol)).to_f32();
        }
        if v.d_pitch.to_bits() != 0 {
            v.pitch_f = e(v.pitch_f).add(e(v.d_pitch)).to_f32();
        }
        if v.d_modx.to_bits() != 0 {
            v.p.modx = e(v.p.modx).add(e(v.d_modx)).to_f32();
        }
        if v.d_mody.to_bits() != 0 {
            v.p.mody = e(v.p.mody).add(e(v.d_mody)).to_f32();
        }
        v.p.pan = e(v.pan_f).round_i64() as i32;
        v.p.pitch = e(v.pitch_f).round_i64() as i32;
    }

    /// Where a glide in progress ends.
    fn slide_end(v: &Voice) -> NoteParams {
        let mut o = v.p;
        if v.slide_n > 0 {
            let n = ei(v.slide_n);
            if v.d_pan.to_bits() != 0 {
                o.pan = n.mul(e(v.d_pan)).add(e(v.pan_f)).round_i64() as i32;
            }
            o.vol = n.mul(e(v.d_vol)).add(e(o.vol)).to_f32();
            if v.d_pitch.to_bits() != 0 {
                o.pitch = n.mul(e(v.d_pitch)).add(e(v.pitch_f)).round_i64() as i32;
            }
            o.modx = n.mul(e(v.d_modx)).add(e(o.modx)).to_f32();
            o.mody = n.mul(e(v.d_mody)).add(e(o.mody)).to_f32();
        }
        o
    }

    /// Voice totals: note + channel (+ tracking).
    fn totals(s: &ChannelSettings, v: &mut Voice) {
        v.vol_t = e(v.p.vol).mul(e(s.vol)).to_f32();
        v.pitch_t = v.p.pitch.wrapping_add(s.pitch).wrapping_add(v.arp_pitch);
        v.pan_t = v.p.pan.wrapping_add(s.pan);
        v.modx_t = e(v.p.modx).add(e(s.modx)).to_f32();
        v.mody_t = e(v.p.mody).add(e(s.mody)).to_f32();
        if s.tracking {
            let tv = e(v.vol_t).add(e(s.track_vel_mid)).to_f32();
            let tk = ei(v.pitch_t).add(e(s.track_key_mid)).to_f32();
            let dp = e(s.track_vel[0]).mul(e(tv)).add(e(s.track_key[0]).mul(e(tk))).round_i64() as i32;
            v.pan_t = v.pan_t.wrapping_add(dp);
            v.modx_t = e(s.track_vel[1]).mul(e(tv)).add(e(v.modx_t)).add(e(s.track_key[1]).mul(e(tk))).to_f32();
            v.mody_t = e(s.track_vel[2]).mul(e(tv)).add(e(v.mody_t)).add(e(s.track_key[2]).mul(e(tk))).to_f32();
        }
    }

    /// Arpeggio pitch offset for the voice's current step.
    fn arp_pitch(&mut self, i: usize) -> i32 {
        let s = &self.s;
        let mut n = s.arp_range.max(1);
        let mut base = self.voices[i].fine;
        let chord = (s.arp_chord > 0).then(|| CHORDS.get(s.arp_chord as usize - 1).map(|c| c.1)).flatten();
        if let Some(c) = chord {
            n = (c.len() as i32 + 1).wrapping_mul(n);
        }
        if n > 1 {
            let a = self.voices[i].arp;
            let mut step = match s.arp_dir {
                1 => a % n,
                2 => n - 1 - a % n,
                3 => {
                    let m = 2 * n - 2;
                    let st = a % m;
                    if n > st { st } else { m - st }
                }
                4 => {
                    let m = 2 * n;
                    let st = a % m;
                    if n > st { st } else { 2 * n - 1 - st }
                }
                5 => self.random(n as u32) as i32,
                _ => 0,
            };
            if let Some(c) = chord {
                let k = c.len() as u32 + 1;
                let (q, r) = ((step as u32) / k, (step as u32) % k);
                step = q as i32;
                if self.s.arp_range == 0 {
                    step = step.wrapping_add(r as i32);
                }
                for &iv in c.iter().take(r as usize) {
                    base = base.wrapping_add(iv as i32 * 100);
                }
            }
            base = base.wrapping_add(step.wrapping_mul(1200));
        }
        base
    }

    /// Arpeggio glide to the next step.
    fn arp_slide(&mut self, i: usize) {
        if self.s.arp_slide && self.voices[i].arp_end <= self.voices[i].end {
            let v = &mut self.voices[i];
            v.arp_pf = ei(v.arp_pitch).to_f32();
            v.arp = v.arp.wrapping_add(1);
            let p = self.arp_pitch(i).wrapping_sub(self.voices[i].arp_pitch);
            let v = &mut self.voices[i];
            v.arp_inc = ei(p).div(ei(v.arp_end.wrapping_sub(v.arp_next).wrapping_add(1))).to_f32();
        } else {
            self.voices[i].arp_inc = f32::from_bits(NO_ARP_SLIDE);
        }
    }

    /// Schedule the next arpeggio step.
    fn arp_schedule(&mut self, i: usize) {
        let gate = (self.s.arp_time_ticks.wrapping_mul(self.s.arp_gate) / 48).max(1);
        self.voices[i].arp_next = gate.wrapping_add(self.tick);
        self.voices[i].arp_end = self.tick.wrapping_add(self.s.arp_time_ticks);
        self.arp_slide(i);
    }

    /// An arpeggio step is due.
    fn arp_step(&mut self, i: usize) {
        if self.s.arp_time_knob >= 1447 {
            self.arp_schedule(i);
            return;
        }
        if self.voices[i].arp_inc.to_bits() != NO_ARP_SLIDE {
            let v = &mut self.voices[i];
            v.arp_pf = e(v.arp_pf).add(e(v.arp_inc)).to_f32();
            v.arp_pitch = e(v.arp_pf).round_i64() as i32;
            if self.tick >= v.arp_end {
                self.arp_schedule(i);
            }
            return;
        }
        let end = self.voices[i].end;
        self.release(i);
        let v = self.voices[i];
        if v.arp_end <= end {
            let p = Self::slide_end(&v);
            let ev = Event {
                time: v.arp_end,
                end,
                release_id: 0,
                id: v.id,
                p,
                fine: v.fine,
                flags: v.flags | F_GENERATED,
                arp: v.arp.wrapping_add(1),
                origin: v.origin,
            };
            self.queue(ev);
        }
    }

    /// A note starts (for a TS404 channel without layers).
    pub fn note_on(&mut self, ev: &Event) {
        let mut ev = *ev;
        if ev.flags & F_GENERATED == 0 && !(self.s.key_lo..=self.s.key_hi).contains(&(ev.p.pitch / 100)) {
            return;
        }
        let key = (ev.flags & 0xf) as i32;
        if ev.release_id != 0 && ev.release_id != NEVER {
            for i in 0..self.n_voices {
                if self.voices[i].id == ev.release_id && self.voices[i].start < ev.time {
                    self.release(i);
                }
            }
        }
        let mut end = ev.end;
        if let Some(g) = self.s.gate_ticks {
            if !self.s.gate_skips_slides || ev.flags & F_SLIDE == 0 {
                end = end.min(ev.time.wrapping_add(g));
            }
        }
        if self.s.mono {
            for i in 0..self.n_voices {
                let v = self.voices[i];
                if v.key == key && v.state <= 1 {
                    if v.state == 1 {
                        self.kill(i);
                        break;
                    }
                    if v.end != NO_END {
                        self.voices[i].end = end;
                    }
                    self.voices[i].id = ev.id;
                    self.voices[i].origin = ev.origin;
                    self.voices[i].state = 0;
                    let ticks = self.s.porta_ticks;
                    self.slide_to(i, ev.p, ticks, ev.p.pitch);
                    return;
                }
            }
        } else if self.s.max_poly != 0 {
            let (mut n, mut pick) = (0, None);
            for i in 0..self.n_voices {
                if self.voices[i].state < 2 {
                    n += 1;
                    if self.voices[i].state >= 1 || pick.is_none() {
                        pick = Some(i);
                    }
                }
            }
            if self.s.max_poly <= n {
                if let Some(i) = pick {
                    self.kill(i);
                }
            }
        }

        // echo set-up
        let echo = self.s.echo_feed > 0 && ev.flags & F_GENERATED == 0;
        let mut echo_ev = ev;
        let mut fb = 0f32;
        let pingpong = self.s.echo_pingpong;
        if echo {
            echo_ev.end = end;
            fb = ei(self.s.echo_feed).mul(e(0.007_812_5)).to_f32();
            if pingpong {
                let n = ei(self.s.echoes);
                fb = e(fb).div(n.mul(e(1.5)).add(e(1.0)).mul(e(2.0))).to_f32();
                ev.p.vol = e(1.5).sub(ei(self.s.echoes).mul(e(fb)).mul(e(2.0))).mul(e(ev.p.vol)).to_f32();
            }
        }

        if e(ev.p.vol).gt(e(0.0)) && self.n_voices < MAX_VOICES {
            let i = self.n_voices;
            let layer = (ev.flags & 0xf) as u8;
            let slot = self.last[layer as usize];
            {
                let v = &mut self.voices[i];
                v.start = ev.time;
                v.end = end;
                v.id = ev.id;
                v.origin = ev.origin;
                v.state = 0;
                v.key = key;
                v.layer = layer;
                v.flags = ev.flags;
                v.slide_n = 0;
                v.fine = ev.fine;
            }
            let glide = (self.s.porta != (ev.flags & F_SLIDE != 0))
                && slot.pitch != NEVER
                && self.s.porta_ticks > 0
                && ev.arp <= 0
                && self.s.echo_feed == 0;
            if glide {
                self.voices[i].p = slot;
                let ticks = self.s.porta_ticks;
                self.slide_to(i, ev.p, ticks, ev.p.pitch);
            } else {
                self.voices[i].p = ev.p;
                self.last[layer as usize] = ev.p;
            }
            self.voices[i].arp = ev.arp;
            self.voices[i].arp_next = NEVER;
            if ev.arp >= 0 {
                self.voices[i].arp_pitch = self.arp_pitch(i);
                self.arp_schedule(i);
            } else {
                self.voices[i].arp_pitch = self.voices[i].fine;
            }
            Self::totals(&self.s, &mut self.voices[i]);
            self.synth.note_start();
            self.n_voices += 1;
        }

        if echo {
            if pingpong {
                ev.p.vol = echo_ev.p.vol;
            }
            let mut dpan = self.s.echo_pan - 128;
            let mut dpitch = self.s.echo_pitch;
            echo_ev.flags |= F_GENERATED | F_ECHO;
            for pass in 0..=pingpong as i32 {
                if pingpong {
                    if pass == 1 {
                        echo_ev.p = ev.p;
                        if ev.arp >= 0 {
                            echo_ev.arp = ev.arp - 1;
                        }
                        dpan = -(self.s.echo_pan - 128);
                        dpitch = -dpitch;
                    }
                    echo_ev.p.vol = e(echo_ev.p.vol).mul(e(fb)).to_f32();
                }
                for k in 1..=self.s.echoes {
                    let t = self.s.echo_time.wrapping_mul(k).wrapping_mul(self.s.ticks_per_step) / 48;
                    echo_ev.time = t.wrapping_add(ev.time);
                    echo_ev.origin = echo_ev.time;
                    if end != HELD {
                        echo_ev.end = end.wrapping_sub(ev.time).wrapping_add(echo_ev.time);
                    }
                    if echo_ev.release_id != 0 {
                        echo_ev.release_id += 1;
                    }
                    if echo_ev.id != 0 {
                        echo_ev.id += 1;
                    }
                    echo_ev.p.pan = (echo_ev.p.pan + dpan + self.s.pan).clamp(-64, 64);
                    if self.s.echo_bounce && echo_ev.p.pan.abs() == 64 {
                        dpan = -dpan;
                    }
                    echo_ev.p.pan -= self.s.pan;
                    if pingpong {
                        if ev.arp >= 0 {
                            echo_ev.arp += 2;
                        }
                    } else {
                        echo_ev.p.vol = e(echo_ev.p.vol).mul(e(fb)).to_f32();
                    }
                    echo_ev.p.pitch = echo_ev.p.pitch.wrapping_add(dpitch);
                    echo_ev.p.modx = echo_shift(echo_ev.p.modx, self.s.echo_cut);
                    echo_ev.p.mody = echo_shift(echo_ev.p.mody, self.s.echo_res);
                    self.queue(echo_ev);
                }
            }
        }
    }

    /// FL's voice update, TS404 part.
    fn update_voices(&mut self, new_tick: bool) {
        let mut i = 0;
        while i < self.n_voices {
            if self.tick >= self.voices[i].end {
                if self.voices[i].state == 4 {
                    self.remove(i);
                    continue;
                }
                self.release(i);
            }
            if new_tick {
                if self.voices[i].arp_next <= self.tick && self.voices[i].state < 1 {
                    self.arp_step(i);
                }
                if self.voices[i].slide_n > 0 {
                    self.voices[i].slide_n -= 1;
                    Self::slide_step(&mut self.voices[i]);
                }
                Self::totals(&self.s, &mut self.voices[i]);
                let v = self.voices[i];
                if v.state > 0 && (self.synth.env_state > 3 || v.state > 1) {
                    self.remove(i);
                    continue;
                }
                self.synth.pan_in = v.pan_t;
                self.synth.vol_in = v.vol_t;
                self.synth.set_pitch(ei(v.pitch_t).to_f32());
                self.synth.pitch_i = v.pitch_t;
                self.synth.cut = v.modx_t;
                self.synth.res = v.mody_t;
            }
            i += 1;
        }
    }

    fn remove(&mut self, i: usize) {
        self.n_voices -= 1;
        self.voices.swap(i, self.n_voices);
    }

    /// One FL audio block: events due now, voices, then the channel mixer. Writes the
    /// channel's stereo output (pan law and volume applied) to `out_l`/`out_r`.
    pub fn block(&mut self, t: &Tables, t3: &Tables3, new_tick: bool, out_l: &mut [f32], out_r: &mut [f32], custom: Option<&crate::tables::Wave>) {
        let n = out_l.len();
        if new_tick {
            self.tick = self.tick.wrapping_add(1);
        }
        while let Some(ev) = self.events.last().copied() {
            if ev.time > self.tick {
                break;
            }
            self.events.pop();
            self.note_on(&ev);
        }
        self.update_voices(new_tick);

        // FL's channel mixer
        let (old_l, old_r) = (self.gain_l, self.gain_r);
        let v = e(self.synth.vol_in).mul(e(2.0)).to_f32();
        let pan = self.synth.pan_in.clamp(-64, 64);
        self.gain_l = e(self.pan_tab[(64 + pan) as usize]).mul(e(v)).to_f32();
        self.gain_r = e(self.pan_tab[(64 - pan) as usize]).mul(e(v)).to_f32();
        let min_jump = f32::from_bits(0x3851_b717);
        let inc_l = ramp35(&mut self.gain_l, old_l, n as i32, min_jump);
        let inc_r = ramp35(&mut self.gain_r, old_r, n as i32, min_jump);
        let mut mono = vec![0f32; n];
        self.synth.render(t, t3, &mut mono, self.aa, false, custom);
        // FL adds into the (cleared) mixer track: x·gain + 0.
        let zero = e(0.0);
        if inc_l.to_bits() | inc_r.to_bits() == 0 {
            if old_l.to_bits() | old_r.to_bits() == 0 {
                out_l.fill(0.0);
                out_r.fill(0.0);
            } else {
                for k in 0..n {
                    out_l[k] = e(mono[k]).mul(e(old_l)).add(zero).to_f32();
                    out_r[k] = e(mono[k]).mul(e(old_r)).add(zero).to_f32();
                }
            }
        } else {
            // the gains step in extended precision, as on FL's FPU stack
            let (mut gl, mut gr) = (e(old_l), e(old_r));
            for k in 0..n {
                out_l[k] = e(mono[k]).mul(gl).add(zero).to_f32();
                out_r[k] = e(mono[k]).mul(gr).add(zero).to_f32();
                gr = gr.add(e(inc_r));
                gl = gl.add(e(inc_l));
            }
        }
    }
}

/// Echo cutoff/resonance shift per repeat (128 = none).
pub(crate) fn echo_shift(x: f32, knob: i32) -> f32 {
    if knob == 128 {
        x
    } else if knob < 128 {
        e(x).add(e(1.0)).mul(ei(knob)).mul(e(0.007_812_5)).add(e(-1.0)).to_f32()
    } else {
        e(1.0).sub(e(1.0).sub(e(x)).mul(ei(256 - knob)).mul(e(0.007_812_5))).to_f32()
    }
}
