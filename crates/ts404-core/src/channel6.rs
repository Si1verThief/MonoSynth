//! FL 6's channel engine around one TS404. Same shape as FL 3.5's (`channel3`): notes
//! become voices, and once per tick each voice writes its pan, volume, pitch, cutoff and
//! resonance into the synth (the last voice wins). FL 6 keeps all of these as floats,
//! glides without rounding, and has a new arpeggiator: an arpeggio is a silent voice that
//! plays the notes itself, and in "auto" chord mode it arpeggiates the held notes.
//!
//! Ticks are 1/24 of a step. FL processes audio in blocks that end at ticks: events due
//! at a block's tick play first, then the voices update, then the mixer renders.

use crate::channel3::{echo_shift, CHORDS};
use crate::engine3::Engine3;
use crate::tables::Tables;
use crate::tables3::Tables3;
use crate::x87::{e, ei, Ext};

/// Note end meaning "until it is released".
pub const HELD: i32 = 0x7fff_fffd;
const NEVER: i32 = 0x7fff_ffff;
const NO_PITCH: u32 = 0x7fff_ffff;
const NO_SLIDE: u32 = 0x7fff_ffff;
const FAR: f32 = 2_147_483_648.0;

/// Event flags (bits 0..3 are the layer).
pub const F_SLIDE: u32 = 0x10;
pub const F_GENERATED: u32 = 0x1_0000;
pub const F_ARP_NOTE: u32 = 0x2_0000;
pub const F_ECHO: u32 = 0x4_0000;
pub const F_NO_ARP: u32 = 0x8_0000;

/// Voice kinds: sounding, or silent (a held mono note, an arpeggio's held note, an
/// arpeggio that plays notes of its own).
const SOUNDING: i32 = 1;
const MONO_HELD: i32 = -10;
const ARP_HELD: i32 = -20;
const ARP_PLAYER: i32 = -21;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct NoteParams6 {
    /// -1..1 relative to the channel's pan.
    pub pan: f32,
    /// Velocity as a level.
    pub vol: f32,
    /// Cents (key · 100).
    pub pitch: f32,
    /// Cutoff and resonance offsets added to the channel's.
    pub modx: f32,
    pub mody: f32,
}

impl NoteParams6 {
    fn map(self, o: NoteParams6, f: impl Fn(f32, f32) -> f32) -> NoteParams6 {
        NoteParams6 { pan: f(self.pan, o.pan), vol: f(self.vol, o.vol), pitch: f(self.pitch, o.pitch), modx: f(self.modx, o.modx), mody: f(self.mody, o.mody) }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Event6 {
    pub time: i32,
    pub end: i32,
    pub release_id: i32,
    pub id: i32,
    pub p: NoteParams6,
    /// Fine pitch in cents, added on top.
    pub fine: i32,
    pub flags: u32,
    /// Arpeggio position the note starts from (-1: the start).
    pub arp: i32,
    /// When the note this event comes from (or its echo) starts. Not FL's: it lets a live
    /// note end the way a note of known length would (`Channel6::end_note`).
    pub origin: i32,
}

impl Event6 {
    pub fn note(time: i32, end: i32, id: i32, pitch: f32, vol: f32, flags: u32) -> Event6 {
        Event6 { time, end, release_id: 0, id, p: NoteParams6 { pan: 0.0, vol, pitch, modx: 0.0, mody: 0.0 }, fine: 0, flags, arp: -1, origin: time }
    }
}

/// A voice. FL keeps them as objects that are reused, so fields a new note doesn't set
/// keep what the last note left in them.
#[derive(Clone, Copy, Debug, Default)]
struct Voice {
    id: i32,
    start: i32,
    end: i32,
    layer: usize,
    flags: u32,
    state: i32,
    kind: i32,
    released: bool,
    p: NoteParams6,
    orig: NoteParams6,
    tot: NoteParams6,
    fine: i32,
    slide_n: i32,
    d: NoteParams6,
    mono: i32,
    arp_mode: i32,
    arp_state: i32,
    arp_idx: i32,
    arp_next: i32,
    arp_end: i32,
    arp_slide_at: i32,
    arp_pitch: f32,
    arp_acc: f32,
    arp_inc: f32,
    arp_p: NoteParams6,
    arp_cur: f32,
    arp_nxt: f32,
    arp_voice: Option<usize>,
    arp_resync: bool,
    origin: i32,
}

/// Per-layer state: the last note (glide source), the held notes (mono and auto
/// arpeggio), the voice playing them, and the arpeggio's note list.
#[derive(Clone, Debug)]
struct Slot {
    last: NoteParams6,
    held: Vec<usize>,
    voice: Option<usize>,
    arp: Vec<NoteParams6>,
    chord: i32,
    dirty: bool,
}

/// The channel's settings, in FL 6's units.
#[derive(Clone, Debug)]
pub struct ChannelSettings6 {
    /// Pan total (-1..1), volume total, pitch total (cents; FL subtracts the root note
    /// ·100), cutoff and resonance totals.
    pub pan: f32,
    pub vol: f32,
    pub pitch: f32,
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
    /// Glide length in ticks.
    pub porta_ticks: i32,
    /// Gate in ticks (`None`: off) and whether slide notes bypass it.
    pub gate_ticks: Option<i32>,
    pub gate_skips_slides: bool,
    /// Echo: feedback 0..12800, pan 0..25600 (12800 = none), pitch (cents), echoes,
    /// time (1/48 step), cutoff and resonance 0..256 (128 = none), ping-pong, pan bounce.
    pub echo_feed: i32,
    pub echo_pan: i32,
    pub echo_pitch: i32,
    pub echoes: i32,
    pub echo_time: i32,
    pub echo_cut: i32,
    pub echo_res: i32,
    pub echo_pingpong: bool,
    pub echo_bounce: bool,
    /// Arpeggiator: direction (0 off, 1 up, 2 down, 3 up+down, 4 up+down with repeat,
    /// 5 random), range (octaves), chord (-1 auto: the held notes, 0 none, else 1-based
    /// into `CHORDS`), repeat, time knob (1447 = off), time in ticks, gate (0..48), slide.
    pub arp_dir: i32,
    pub arp_range: i32,
    pub arp_chord: i32,
    pub arp_repeat: i32,
    pub arp_time_knob: i32,
    pub arp_time_ticks: i32,
    pub arp_gate: i32,
    pub arp_slide: bool,
    /// Ticks per step (FL: 24).
    pub ticks_per_step: i32,
}

impl Default for ChannelSettings6 {
    fn default() -> Self {
        ChannelSettings6 {
            pan: 0.0,
            vol: 0.551_010_6,
            pitch: -6000.0,
            modx: 120.0 / 256.0,
            mody: 80.0 / 256.0,
            tracking: false,
            track_vel_mid: -0.551_010_6,
            track_key_mid: 0.0,
            track_vel: [0.0; 3],
            track_key: [0.0; 3],
            key_lo: 0,
            key_hi: 256,
            max_poly: 0,
            mono: true,
            porta: false,
            porta_ticks: 3,
            gate_ticks: None,
            gate_skips_slides: true,
            echo_feed: 0,
            echo_pan: 12800,
            echo_pitch: 0,
            echoes: 4,
            echo_time: 144,
            echo_cut: 128,
            echo_res: 128,
            echo_pingpong: false,
            echo_bounce: false,
            arp_dir: 0,
            arp_range: 1,
            arp_chord: -1,
            arp_repeat: 1,
            arp_time_knob: 1024,
            arp_time_ticks: 24,
            arp_gate: 48,
            arp_slide: false,
            ticks_per_step: 24,
        }
    }
}

/// FL 6's linear ramp of a gain toward its new value: at most 0.001 per sample.
fn ramp6(cur: &mut f32, old: f32, n: i32) -> f32 {
    let max = f32::from_bits(0x3a83_126f);
    let mut inc = e(*cur).sub(e(old)).to_f32();
    if inc.to_bits() == 0 {
        return inc;
    }
    inc = e(inc).div(ei(n)).to_f32();
    if !e(f32::from_bits(inc.to_bits() & 0x7fff_ffff)).gt(e(max)) {
        return inc;
    }
    inc = f32::from_bits((inc.to_bits() & 0x8000_0000) | max.to_bits());
    *cur = ei(n).mul(e(inc)).add(e(old)).to_f32();
    if ((cur.to_bits() & 0x7fff_ffff) as i32) < 0x3380_0001 {
        *cur = 0.0;
        inc = e(old).neg().div(ei(n)).to_f32();
    }
    inc
}

/// x87 equality as FL's `fcomp; jnz` tests it (unordered counts as equal).
fn same(a: f32, b: f32) -> bool {
    !matches!(e(a).cmp(e(b)), Some(core::cmp::Ordering::Less | core::cmp::Ordering::Greater))
}

/// |round(a - b)|, as the low 32 bits FL compares.
fn pitch_dist(a: f32, b: f32) -> i32 {
    let d = e(a).sub(e(b)).round_i64();
    d.wrapping_abs() as i32
}

pub struct Channel6 {
    pub synth: Engine3,
    pub s: ChannelSettings6,
    /// Voice objects, FL's voice array (pointers into `objs`) and the active count.
    objs: Vec<Voice>,
    arr: Vec<usize>,
    count: usize,
    /// Pending events, descending time (FL pops from the end).
    events: Vec<Event6>,
    slots: Vec<Slot>,
    tick: i32,
    /// What the voices last wrote into the synth for the mixer: pan and volume.
    synth_pan: f32,
    synth_vol: f32,
    gain_l: f32,
    gain_r: f32,
    pub circular_pan: bool,
    rand_seed: u32,
    /// FL's "alias-free TS404" (export option) and HQ mode.
    pub aa: bool,
    pub hq: bool,
}

impl Channel6 {
    pub fn new(synth: Engine3) -> Channel6 {
        let slot = Slot {
            last: NoteParams6 { pitch: f32::from_bits(NO_PITCH), ..Default::default() },
            held: Vec::new(),
            voice: None,
            arp: Vec::new(),
            chord: 0,
            dirty: true,
        };
        Channel6 {
            synth,
            s: ChannelSettings6::default(),
            objs: Vec::with_capacity(64),
            arr: Vec::with_capacity(64),
            count: 0,
            events: Vec::with_capacity(4096),
            slots: vec![slot; 16],
            tick: 0,
            synth_pan: 0.0,
            synth_vol: 0.0,
            gain_l: 0.0,
            gain_r: 0.0,
            circular_pan: true,
            rand_seed: 0,
            aa: false,
            hq: false,
        }
    }

    /// Call after changing the arpeggiator settings: the arpeggios rebuild their notes.
    pub fn settings_changed(&mut self) {
        for s in self.slots.iter_mut() {
            s.dirty = true;
        }
    }

    pub fn tick(&self) -> i32 {
        self.tick
    }

    pub fn active_voices(&self) -> usize {
        self.count
    }

    /// FL's Random(n) (Delphi's generator).
    fn random(&mut self, n: u32) -> u32 {
        self.rand_seed = self.rand_seed.wrapping_mul(0x0808_8405).wrapping_add(1);
        ((n as u64 * self.rand_seed as u64) >> 32) as u32
    }

    fn voice_at(&self, i: usize) -> usize {
        self.arr[i]
    }

    // ------------------------------------------------------------------ events

    /// FL's event queue: an event due at the current tick plays right away,
    /// anything else waits in the list (equal times play newest first).
    pub fn queue(&mut self, ev: Event6) {
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
    /// ends at `end`: it and its echoes end as if they had been queued with that end.
    pub fn end_note(&mut self, id: i32, start: i32, end: i32) {
        let ours = |x: i32| (x.wrapping_sub(id) as u32) < 64;
        for i in 0..self.count {
            let v = &mut self.objs[self.arr[i]];
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
        self.count = 0;
        for s in self.slots.iter_mut() {
            s.last = NoteParams6 { pitch: f32::from_bits(NO_PITCH), ..Default::default() };
            s.held.clear();
            s.voice = None;
            s.dirty = true;
        }
    }

    // ------------------------------------------------------------------ voices

    /// What a release does besides the state change. Returns whether
    /// the voice may be released (the voice playing held notes may not while any are).
    fn pre_release(&mut self, o: usize) -> bool {
        let layer = self.objs[o].layer;
        let can = !(self.slots[layer].voice == Some(o) && !self.slots[layer].held.is_empty());
        if can {
            self.objs[o].arp_state = 0;
        } else {
            self.objs[o].end = HELD;
        }
        if self.objs[o].arp_mode != 0 {
            if self.objs[o].arp_mode > 0 {
                self.objs[o].arp_mode = -1;
                self.unhold(o);
                self.arp_dirty(layer);
            }
            if let Some(g) = self.objs[o].arp_voice {
                if self.slots[layer].held.is_empty() {
                    self.release(g);
                }
            }
        }
        if !self.objs[o].released {
            if self.objs[o].mono > 0 {
                self.objs[o].mono = -1;
                self.unhold(o);
                self.mono_glide(layer);
            }
            // every voice's release sends the synth into its release
            self.synth.note_off();
            if self.objs[o].flags & F_ARP_NOTE != 0 {
                for i in 0..self.count {
                    let w = self.voice_at(i);
                    if self.objs[w].arp_voice == Some(o) {
                        self.objs[w].arp_voice = None;
                    }
                }
                self.objs[o].flags &= !F_ARP_NOTE;
            }
            self.objs[o].released = true;
        }
        can
    }

    /// Release a voice.
    fn release(&mut self, o: usize) {
        if self.objs[o].state < 1 && self.pre_release(o) {
            self.objs[o].state = 1;
        }
    }

    /// Kill a voice (it goes at the next update).
    fn kill(&mut self, o: usize) {
        if self.objs[o].state < 2 {
            self.objs[o].state = 2;
        }
    }

    /// A held note is let go: when none are left, the voice that
    /// played them is released.
    fn unhold(&mut self, o: usize) {
        let layer = self.objs[o].layer;
        let held = &mut self.slots[layer].held;
        if let Some(k) = held.iter().position(|&x| x == o) {
            held.remove(k);
        }
        if self.slots[layer].held.is_empty() {
            if let Some(v) = self.slots[layer].voice {
                self.release(v);
            }
        }
    }

    /// Mono: the playing voice glides to the newest held note.
    fn mono_glide(&mut self, layer: usize) {
        let (Some(v), Some(&last)) = (self.slots[layer].voice, self.slots[layer].held.last()) else {
            return;
        };
        self.objs[v].state = 0;
        let to = self.objs[last].orig;
        let ticks = self.s.porta_ticks;
        self.slide_to(v, to, ticks, to.pitch);
    }

    /// Glide set-up: per-tick steps from the voice's note to `to`.
    fn slide_to(&mut self, o: usize, to: NoteParams6, ticks: i32, pitch: f32) {
        if self.s.echo_feed > 0 {
            return;
        }
        let v = &mut self.objs[o];
        v.slide_n = ticks.wrapping_add(1);
        let len = ei(v.slide_n).to_f32();
        let delta = |new: f32, old: f32| if same(old, new) { 0.0 } else { e(new).sub(e(old)).div(e(len)).to_f32() };
        let target = NoteParams6 { pitch, ..to };
        v.d = target.map(v.p, delta);
        self.slots[v.layer].last = target;
    }

    /// One glide step.
    fn slide_step(v: &mut Voice) {
        let step = |x: f32, d: f32| if d.to_bits() != 0 { e(x).add(e(d)).to_f32() } else { x };
        v.p = v.p.map(v.d, step);
    }

    /// Where a glide in progress ends.
    fn slide_end(v: &Voice) -> NoteParams6 {
        if v.slide_n > 0 {
            let n = ei(v.slide_n);
            v.p.map(v.d, |x, d| n.mul(e(d)).add(e(x)).to_f32())
        } else {
            v.p
        }
    }

    /// Voice totals: note + channel (+ tracking).
    fn totals(s: &ChannelSettings6, v: &mut Voice) {
        let t = &mut v.tot;
        t.vol = e(v.p.vol).mul(e(s.vol)).to_f32();
        t.pitch = e(v.p.pitch).add(e(s.pitch)).add(e(v.arp_pitch)).to_f32();
        t.pan = e(v.p.pan).add(e(s.pan)).to_f32();
        t.modx = e(v.p.modx).add(e(s.modx)).to_f32();
        t.mody = e(v.p.mody).add(e(s.mody)).to_f32();
        if s.tracking {
            let tv = e(t.vol).add(e(s.track_vel_mid)).to_f32();
            let tk = e(t.pitch).add(e(s.track_key_mid)).to_f32();
            let track = |x: f32, k: usize| e(s.track_vel[k]).mul(e(tv)).add(e(x)).add(e(s.track_key[k]).mul(e(tk))).to_f32();
            t.pan = track(t.pan, 0);
            t.modx = track(t.modx, 1);
            t.mody = track(t.mody, 2);
        }
    }

    // --------------------------------------------------------------- arpeggio

    /// The layer's held notes changed: rebuild its arpeggio, and every
    /// arpeggio on it finds its place again. (FL compares against 2147483647.0, which a
    /// float never equals, so they always do.)
    fn arp_dirty(&mut self, layer: usize) {
        self.slots[layer].dirty = true;
        for i in 0..self.count {
            let o = self.voice_at(i);
            if self.objs[o].layer == layer {
                self.objs[o].arp_resync = true;
            }
        }
    }

    /// The arpeggio's notes, in playing order.
    fn build_arp(&mut self, layer: usize) {
        let s = &self.s;
        let mut chord: Option<&[i8]> = None;
        let mut n_ch: i32 = 1;
        let mut slot_chord = s.arp_chord;
        if s.arp_chord > 0 {
            chord = CHORDS.get(s.arp_chord as usize - 1).map(|c| c.1);
            n_ch = chord.map_or(0, |c| c.len() as i32) + 1;
        } else if s.arp_chord < 0 {
            n_ch = self.slots[layer].held.len() as i32;
            if n_ch <= 0 {
                slot_chord = 1;
                chord = Some(CHORDS[0].1);
                n_ch = CHORDS[0].1.len() as i32 + 1;
            }
        }
        let n_base = n_ch;
        let n_rep = n_ch.wrapping_mul(s.arp_repeat);
        let total = s.arp_range.max(1).wrapping_mul(n_rep);
        let mut cnt = total;
        if total > 1 {
            match s.arp_dir {
                3 => cnt = 2 * total - 2,
                4 => cnt = 2 * total,
                _ => {}
            }
        }
        let mut list = Vec::with_capacity(cnt.max(0) as usize);
        for i in 0..cnt.max(0) {
            let mut n = NoteParams6::default();
            if total >= 1 {
                let mut k = i;
                if total >= 2 {
                    match s.arp_dir {
                        2 => k = total - 1 - i,
                        3 if total <= k => k = 2 * total - 2 - k,
                        4 if total <= k => k = 2 * total - 1 - k,
                        _ => {}
                    }
                }
                let mut q = ((k as u32) / (n_rep as u32)) as i32;
                let r = (((k as u32) % (n_rep as u32)) as i32) % n_base;
                if s.arp_range == 0 {
                    q = q.wrapping_add(r.min(5));
                }
                n.pitch = 0.0;
                if slot_chord > 0 {
                    if let Some(c) = chord {
                        for &iv in c.iter().take(r as usize) {
                            n.pitch = ei(iv as i32 * 100).add(e(n.pitch)).to_f32();
                        }
                    }
                } else if slot_chord < 0 {
                    n = self.objs[self.slots[layer].held[r as usize]].p;
                }
                n.pitch = ei(q.wrapping_mul(1200)).add(e(n.pitch)).to_f32();
            }
            list.push(n);
        }
        let slot = &mut self.slots[layer];
        slot.arp = list;
        slot.chord = slot_chord;
        slot.dirty = false;
    }

    /// Move to the arpeggio's next note: `arp_p` becomes its note
    /// parameters (pitch relative to the voice's).
    fn arp_next_note(&mut self, o: usize) {
        self.objs[o].arp_p = Self::slide_end(&self.objs[o]);
        let layer = self.objs[o].layer;
        if self.slots[layer].dirty {
            self.build_arp(layer);
        }
        let up = (self.s.arp_dir > 0) as i32;
        if self.objs[o].arp_resync {
            let v = &mut self.objs[o];
            let mut best = i32::MAX;
            for (i, n) in self.slots[layer].arp.iter().enumerate() {
                let d = pitch_dist(n.pitch, v.arp_cur);
                if best >= d {
                    v.arp_idx = i as i32;
                    best = d;
                }
                let d = pitch_dist(n.pitch, v.arp_nxt);
                if best > d {
                    v.arp_idx = i as i32 - up;
                    best = d;
                }
            }
            v.arp_resync = false;
        }
        let n = self.slots[layer].arp.len() as i32;
        if n > 0 {
            let idx = if self.s.arp_dir == 5 {
                self.random(n as u32) as i32
            } else {
                // (a position far below the start would make FL read outside the list)
                n.wrapping_add(self.objs[o].arp_idx).wrapping_add(up).wrapping_rem(n).rem_euclid(n)
            };
            let slot = &self.slots[layer];
            let v = &mut self.objs[o];
            v.arp_idx = idx;
            let note = slot.arp[idx as usize];
            if slot.chord < 0 {
                v.arp_p = note;
            } else {
                v.arp_p.pitch = note.pitch;
            }
            v.arp_cur = v.arp_p.pitch;
            v.arp_nxt = slot.arp[(idx.wrapping_add(1) % n) as usize].pitch;
            v.arp_p.pitch = if slot.chord < 0 {
                e(v.arp_p.pitch).sub(e(v.p.pitch)).to_f32()
            } else {
                ei(v.fine).add(e(v.arp_p.pitch)).to_f32()
            };
        }
    }

    /// Schedule the next arpeggio step: note off after the gate, next
    /// note after the time.
    fn arp_schedule(&mut self, o: usize) {
        let gate = (self.s.arp_time_ticks.wrapping_mul(self.s.arp_gate) / 48).max(1);
        let v = &mut self.objs[o];
        v.arp_next = gate.wrapping_add(self.tick);
        v.arp_end = self.tick.wrapping_add(self.s.arp_time_ticks);
    }

    /// Arpeggio glide to the next note.
    fn arp_slide_setup(&mut self, o: usize) {
        if self.objs[o].arp_state == 2 {
            self.objs[o].arp_acc = self.objs[o].arp_pitch;
            self.arp_next_note(o);
            let v = &mut self.objs[o];
            v.arp_inc = e(v.arp_p.pitch).sub(e(v.arp_pitch)).div(ei(v.arp_end.wrapping_sub(v.arp_next).wrapping_add(1))).to_f32();
        } else {
            self.objs[o].arp_inc = f32::from_bits(NO_SLIDE);
        }
    }

    /// An arpeggio's tick.
    fn arp_tick(&mut self, o: usize) {
        let tick = self.tick;
        match self.objs[o].arp_state {
            // time off: each note takes the next arpeggio note once
            3 => {
                if tick >= self.objs[o].arp_next {
                    self.arp_next_note(o);
                    let v = &mut self.objs[o];
                    v.arp_pitch = v.arp_p.pitch;
                    v.arp_next = NEVER;
                }
            }
            // slide: the voice glides from note to note
            2 => {
                if tick >= self.objs[o].arp_next {
                    if tick >= self.objs[o].arp_slide_at {
                        self.arp_slide_setup(o);
                        self.objs[o].arp_slide_at = NEVER;
                    }
                    if tick >= self.objs[o].arp_end {
                        self.arp_schedule(o);
                        self.objs[o].arp_slide_at = self.objs[o].arp_next;
                    }
                    let v = &mut self.objs[o];
                    v.arp_acc = e(v.arp_acc).add(e(v.arp_inc)).to_f32();
                    v.arp_pitch = Ext::from_i64(e(v.arp_acc).round_i64()).to_f32();
                }
            }
            // the arpeggio plays notes of its own
            1 => {
                if tick < self.objs[o].arp_next {
                    return;
                }
                let layer = self.objs[o].layer;
                if self.slots[layer].chord == -1 && self.objs[o].arp_voice.is_some() {
                    if self.slots[layer].dirty {
                        self.build_arp(layer);
                    }
                    let slot = &self.slots[layer];
                    if slot.arp.len() == 1 && same(self.objs[o].arp_cur, slot.arp[0].pitch) {
                        // one held note: it keeps sounding
                        if tick >= self.objs[o].arp_end {
                            self.arp_schedule(o);
                        }
                        return;
                    }
                }
                if let Some(g) = self.objs[o].arp_voice {
                    self.release(g);
                    self.objs[o].arp_voice = None;
                }
                if self.objs[o].arp_end <= tick {
                    let v = self.objs[o];
                    let mut ev = Event6 {
                        time: v.arp_end,
                        end: HELD,
                        release_id: 0,
                        id: 0,
                        p: NoteParams6::default(),
                        fine: 0,
                        flags: v.flags | F_GENERATED | F_ARP_NOTE | F_NO_ARP,
                        arp: -1,
                        origin: v.arp_end,
                    };
                    self.arp_next_note(o);
                    let v = &self.objs[o];
                    ev.p = v.arp_p;
                    ev.p.pitch = e(ev.p.pitch).add(e(v.p.pitch)).to_f32();
                    let g = self.note_on(&ev);
                    self.objs[o].arp_voice = g;
                    self.arp_schedule(o);
                }
            }
            _ => {}
        }
    }

    // ------------------------------------------------------------------ notes

    /// FL's voice array: a new note takes the object after the active ones.
    fn new_voice(&mut self) -> usize {
        if self.arr.len() > self.count {
            self.arr[self.count]
        } else {
            self.objs.push(Voice::default());
            self.arr.push(self.objs.len() - 1);
            self.objs.len() - 1
        }
    }

    /// A note starts (for a TS404 channel without layers). Returns its voice.
    pub fn note_on(&mut self, ev: &Event6) -> Option<usize> {
        let mut ev = *ev;
        if ev.flags & F_GENERATED == 0 {
            let key = e(ev.p.pitch).mul(e(f32::from_bits(0x3c23_d70a))).round_i64() as i32;
            if !(self.s.key_lo <= key && key <= self.s.key_hi) {
                return None;
            }
        }
        let layer = (ev.flags & 0xf) as usize;
        if ev.release_id > 0 {
            if ev.release_id == NEVER {
                return None;
            }
            for i in 0..self.count {
                let o = self.voice_at(i);
                if self.objs[o].id == ev.release_id && self.objs[o].start < ev.time {
                    self.release(o);
                }
            }
        }
        let mut end = ev.end;
        if let Some(g) = self.s.gate_ticks {
            if !(self.s.gate_skips_slides && ev.flags & F_SLIDE != 0) {
                end = end.min(ev.time.wrapping_add(g));
            }
        }
        let mut mode = (self.s.arp_dir != 0 && ev.flags & F_NO_ARP == 0) as i32;
        if mode > 0 {
            if self.s.arp_chord < 0 {
                ev.flags |= F_GENERATED;
                if !self.slots[layer].held.is_empty() {
                    mode = 2;
                }
            } else {
                mode = -1;
            }
        }
        if !self.s.mono && self.s.max_poly != 0 {
            let (mut n, mut pick) = (0, None);
            for i in 0..self.count {
                let o = self.voice_at(i);
                let v = &self.objs[o];
                if v.state < 2 && v.kind >= 0 {
                    n += 1;
                    if v.state >= 1 || pick.is_none() {
                        pick = Some(o);
                    }
                }
            }
            if self.s.max_poly <= n {
                if let Some(o) = pick {
                    self.kill(o);
                }
            }
        }

        // echo set-up
        let mut echo = false;
        let mut echo_ev = ev;
        let mut fb = 0f32;
        let pingpong = self.s.echo_pingpong;
        if self.s.echo_feed > 0 && ev.flags & F_GENERATED == 0 {
            echo_ev.end = end;
            echo = true;
            fb = ei(self.s.echo_feed).mul(e(f32::from_bits(0x38a3_d70a))).to_f32();
            if pingpong {
                let n = ei(self.s.echoes);
                fb = e(fb).div(n.mul(e(1.5)).add(e(1.0)).mul(e(2.0))).to_f32();
                ev.p.vol = e(1.5).sub(ei(self.s.echoes).mul(e(fb)).mul(e(2.0))).mul(e(ev.p.vol)).to_f32();
            }
        }

        let o = self.new_voice();
        {
            let v = &mut self.objs[o];
            v.start = ev.time;
            v.end = end;
            v.id = ev.id;
            v.origin = ev.origin;
            v.state = 0;
            v.layer = layer;
            v.flags = ev.flags;
            v.arp_voice = None;
            v.released = false;
            v.slide_n = 0;
            v.fine = ev.fine;
            v.kind = 0;
            v.arp_state = 0;
            v.arp_mode = mode;
            v.p = ev.p;
            v.orig = ev.p;
        }
        let mut glide = (self.s.porta != (ev.flags & F_SLIDE != 0))
            && self.slots[layer].last.pitch.to_bits() != NO_PITCH
            && self.s.porta_ticks > 0
            && mode == 0
            && self.s.echo_feed == 0;
        self.objs[o].mono = (self.s.mono && ev.flags & (F_GENERATED | F_NO_ARP) == 0 && mode <= 0) as i32;
        if self.objs[o].mono != 0 {
            if self.objs[o].end == HELD {
                let n = self.slots[layer].held.len();
                for i in (1..n).rev() {
                    if let Some(&w) = self.slots[layer].held.get(i) {
                        self.release(w);
                    }
                }
            }
            self.slots[layer].held.push(o);
            if self.slots[layer].held.len() > 1 {
                self.objs[o].kind = MONO_HELD;
                self.mono_glide(layer);
                self.objs[o].arp_mode = 0;
            } else {
                self.slots[layer].voice = Some(o);
            }
            echo = false;
        }
        {
            let v = &mut self.objs[o];
            v.arp_idx = ev.arp;
            v.arp_next = NEVER;
            v.arp_pitch = ei(v.fine).to_f32();
            v.arp_acc = v.arp_pitch;
            v.arp_inc = 0.0;
            v.arp_cur = FAR;
            v.arp_nxt = FAR;
            v.arp_resync = false;
        }
        if self.objs[o].arp_mode != 0 {
            if self.objs[o].arp_mode > 0 {
                let pitch = self.objs[o].p.pitch;
                let held = &self.slots[layer].held;
                let mut i = 0;
                while i < held.len() && !(self.objs[held[i]].p.pitch >= pitch) {
                    i += 1;
                }
                self.slots[layer].held.insert(i, o);
                self.arp_dirty(layer);
            }
            if self.objs[o].arp_mode == 2 {
                self.objs[o].kind = ARP_HELD;
                glide = false;
            } else {
                if self.objs[o].arp_mode > 0 {
                    self.slots[layer].voice = Some(o);
                }
                let tick = self.tick;
                let v = &mut self.objs[o];
                v.arp_end = tick;
                v.arp_next = tick;
                if self.s.arp_time_knob >= 1447 {
                    v.arp_state = 3;
                } else if self.s.arp_slide {
                    v.arp_state = 2;
                    v.arp_slide_at = tick;
                } else {
                    v.kind = ARP_PLAYER;
                    v.arp_state = 1;
                }
            }
        }
        Self::totals(&self.s, &mut self.objs[o]);
        if self.objs[o].kind >= 0 {
            self.objs[o].kind = SOUNDING;
            self.synth.note_start();
        }
        if glide {
            self.objs[o].p = self.slots[layer].last;
            let ticks = self.s.porta_ticks;
            self.slide_to(o, ev.p, ticks, ev.p.pitch);
        } else {
            self.slots[layer].last = ev.p;
        }
        self.count += 1;

        if echo {
            if pingpong {
                ev.p.vol = echo_ev.p.vol;
            }
            let step = e(f32::from_bits(0x3923_d70a));
            let mut dpan = ei(self.s.echo_pan.wrapping_sub(12800)).mul(step).to_f32();
            let mut dpitch = self.s.echo_pitch;
            echo_ev.flags |= F_GENERATED | F_ECHO;
            for pass in 0..=pingpong as i32 {
                if pingpong {
                    if pass == 1 {
                        echo_ev.p = ev.p;
                        echo_ev.arp = ev.arp.wrapping_sub(1);
                        dpan = ei(self.s.echo_pan.wrapping_sub(12800).wrapping_neg()).mul(step).to_f32();
                        dpitch = dpitch.wrapping_neg();
                    }
                    echo_ev.p.vol = e(echo_ev.p.vol).mul(e(fb)).to_f32();
                }
                for k in 1..=self.s.echoes {
                    let t = self.s.echo_time.wrapping_mul(k).wrapping_mul(self.s.ticks_per_step) / 48;
                    echo_ev.time = t.wrapping_add(ev.time);
                    echo_ev.origin = echo_ev.time;
                    if end < 0x7fff_fffc {
                        echo_ev.end = end.wrapping_sub(ev.time).wrapping_add(echo_ev.time);
                    }
                    if echo_ev.release_id != 0 {
                        echo_ev.release_id = echo_ev.release_id.wrapping_add(1);
                    }
                    if echo_ev.id != 0 {
                        echo_ev.id = echo_ev.id.wrapping_add(1);
                    }
                    let pan = e(echo_ev.p.pan).add(e(dpan)).add(e(self.s.pan)).to_f32();
                    let pan = if !e(pan).ge(e(-1.0)) {
                        -1.0
                    } else if e(pan).gt(e(1.0)) {
                        1.0
                    } else {
                        pan
                    };
                    if self.s.echo_bounce && same(e(pan).abs().to_f32(), 1.0) {
                        dpan = -dpan;
                    }
                    echo_ev.p.pan = e(pan).sub(e(self.s.pan)).to_f32();
                    if pingpong {
                        echo_ev.arp = echo_ev.arp.wrapping_add(2);
                    } else {
                        echo_ev.p.vol = e(echo_ev.p.vol).mul(e(fb)).to_f32();
                    }
                    echo_ev.p.pitch = ei(dpitch).add(e(echo_ev.p.pitch)).to_f32();
                    echo_ev.p.modx = echo_shift(echo_ev.p.modx, self.s.echo_cut);
                    echo_ev.p.mody = echo_shift(echo_ev.p.mody, self.s.echo_res);
                    self.queue(echo_ev);
                }
            }
        }
        Some(o)
    }

    /// Remove the voice at `i`.
    fn remove(&mut self, i: usize) {
        self.count -= 1;
        self.arr.swap(i, self.count);
        let o = self.arr[self.count];
        let layer = self.objs[o].layer;
        if self.slots[layer].voice == Some(o) {
            self.slots[layer].voice = None;
        }
        self.pre_release(o);
    }

    /// FL's voice update, TS404 part.
    fn update_voices(&mut self, new_tick: bool) {
        let mut i = 0;
        while i < self.count {
            let o = self.voice_at(i);
            if self.tick >= self.objs[o].end {
                if self.objs[o].state == 4 {
                    self.remove(i);
                    continue;
                }
                self.release(o);
            }
            if self.objs[o].kind < 0 {
                if self.objs[o].state >= 1 {
                    self.remove(i);
                    continue;
                }
                if new_tick && self.objs[o].kind == ARP_PLAYER {
                    self.arp_tick(o);
                }
                i += 1;
                continue;
            }
            if new_tick {
                self.arp_tick(o);
                if self.objs[o].slide_n > 0 {
                    self.objs[o].slide_n -= 1;
                    Self::slide_step(&mut self.objs[o]);
                }
                Self::totals(&self.s, &mut self.objs[o]);
                let v = self.objs[o];
                if v.state >= 1 && (self.synth.env_state >= 4 || v.state >= 2 || self.slots[v.layer].voice.is_some()) {
                    self.remove(i);
                    continue;
                }
                self.synth_pan = v.tot.pan;
                self.synth_vol = v.tot.vol;
                self.synth.set_pitch(v.tot.pitch);
                self.synth.cut = v.tot.modx;
                self.synth.res = v.tot.mody;
            }
            i += 1;
        }
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
        let v = e(self.synth_vol).mul(e(2.0)).to_f32();
        let pan = self.synth_pan;
        let pan = if !e(pan).ge(e(-1.0)) {
            -1.0
        } else if e(pan).gt(e(1.0)) {
            1.0
        } else {
            pan
        };
        if self.circular_pan {
            let a = e(1.0).add(e(pan)).mul(e(f32::from_bits(0x3f49_0fdb)));
            self.gain_l = crate::fpu::cos(a).mul(e(v)).to_f32();
            self.gain_r = crate::fpu::sin(a).mul(e(v)).to_f32();
        } else if pan.to_bits() & 0x8000_0000 != 0 {
            self.gain_l = v;
            self.gain_r = e(1.0).add(e(pan)).mul(e(v)).to_f32();
        } else {
            self.gain_r = v;
            self.gain_l = e(1.0).sub(e(pan)).mul(e(v)).to_f32();
        }
        let inc_l = ramp6(&mut self.gain_l, old_l, n as i32);
        let inc_r = ramp6(&mut self.gain_r, old_r, n as i32);
        let mut mono = vec![0f32; n];
        self.synth.render(t, t3, &mut mono, self.aa, self.hq, custom);
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
