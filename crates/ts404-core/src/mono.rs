//! Realtime monophonic driver for the engine (MIDI notes instead of a step grid).
//!
//! The engine plays steps. Every stretch of MIDI playback becomes one of these:
//!   * `Note`: a new note with nothing held. A gated step of one step length, so the
//!     envelope retriggers and the step gate (GAT) releases it.
//!   * `Glide`: a legato note. A slide step from the sounding note to the new one
//!     (linear glide over one step, no retrigger, no gate).
//!   * `Arrival`: the step after a glide (the slid-to note's own step), again released
//!     by GAT.
//!   * `Hold`: anything after that, while keys are held. An ungated step with the gate
//!     at 100%, so it never releases by itself.
//! Releasing the last key releases the envelope. A GAT of 256 (100%) therefore means
//! "hold until note-off".
//!
//! All storage is fixed-size; nothing allocates.

use crate::engine::{ENV_DONE, Engine, idx};
use crate::steps::Step;
use crate::tables::{Tables, Wave};

const MAX_HELD: usize = 32;
/// Length of a hold step (~95 s, then it simply continues with another one).
const HOLD_LEN: i32 = 1 << 22;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Seg {
    Note,
    Glide,
    Arrival,
    Hold,
}

pub struct MonoVoice {
    pub engine: Engine,
    held: [u8; MAX_HELD],
    n_held: usize,
    /// Semitone index currently sounding (pitch-table units / 32).
    note: i32,
    /// Position in the current step and its length, in engine samples.
    pos: i32,
    len: i32,
    seg: Seg,
    /// The current segment began with an envelope trigger.
    trig: bool,
    step_len: i32,
    /// Step gate, 0..=256 of a step.
    gate: i32,
    pub hq: bool,
}

impl Default for MonoVoice {
    fn default() -> Self {
        let mut engine = Engine::default();
        engine.p[idx::ENV_STATE] = ENV_DONE;
        let mut v = MonoVoice {
            engine,
            held: [0; MAX_HELD],
            n_held: 0,
            note: 48,
            pos: 0,
            len: 0,
            seg: Seg::Hold,
            trig: false,
            step_len: 4410,
            gate: 256,
            hq: false,
        };
        v.hold(); // idle: an ungated step at the idle pitch
        v
    }
}

/// Semitones above C0 (MIDI 12). FL's key range is C1..F8 (MIDI 24..113); notes past it
/// use the extended pitch table.
fn semitone(midi: u8) -> i32 {
    midi as i32 - 12
}

impl MonoVoice {
    /// Step length (slide duration, step-gate reference) in engine samples (normally one 1/16 note).
    pub fn set_step_len(&mut self, samples: i32) {
        self.step_len = samples.clamp(1, HOLD_LEN);
    }

    pub fn step_len(&self) -> i32 {
        self.step_len
    }

    /// Step gate (GAT), 0..=256. 256 holds notes until their note-off.
    pub fn set_gate(&mut self, gate: i32) {
        self.gate = gate.clamp(0, 256);
    }

    pub fn is_silent(&self) -> bool {
        self.engine.env_done()
    }

    pub fn gliding(&self) -> bool {
        self.seg == Seg::Glide
    }

    fn top(&self) -> Option<u8> {
        (self.n_held > 0).then(|| self.held[self.n_held - 1])
    }

    fn begin(&mut self, seg: Seg, cur: Step, next: Step, len: i32) {
        self.engine.begin_step(&cur, &next);
        self.seg = seg;
        self.trig = cur.gate;
        self.pos = 0;
        self.len = len;
    }

    fn hold(&mut self) {
        let s = Step { note: self.note, cut: 0, gate: false, slide: false };
        self.begin(Seg::Hold, s, s, HOLD_LEN);
    }

    fn glide_to(&mut self, target: i32) {
        // A glide starting on the very sample a note was triggered keeps the trigger,
        // like a gated slide step. That holds through any number of glides on that
        // sample (a chord), not just the first.
        let retrigger = self.trig && self.pos == 0;
        let prev_flags = self.engine.p[idx::PREV_FLAGS];
        let from = Step { note: self.note, cut: 0, gate: retrigger, slide: true };
        let to = Step { note: target, cut: 0, gate: false, slide: true };
        self.note = target;
        self.begin(Seg::Glide, from, to, self.step_len);
        if retrigger {
            // The segment being replaced never played, so the step before it is still
            // the previous step (otherwise it would count as a slide and block the trigger).
            self.engine.p[idx::PREV_FLAGS] = prev_flags;
        }
    }

    fn push(&mut self, midi: u8) {
        self.remove(midi);
        if self.n_held == MAX_HELD {
            self.held.copy_within(1.., 0); // forget the oldest key
            self.n_held -= 1;
        }
        self.held[self.n_held] = midi;
        self.n_held += 1;
    }

    pub fn note_on(&mut self, midi: u8) {
        // A note is legato (no retrigger) whenever another key is still held,
        // even if the envelope has already died away.
        let legato = self.n_held > 0;
        self.push(midi);
        let target = semitone(midi);
        if legato {
            self.glide_to(target);
        } else {
            self.note = target;
            self.engine.p[idx::FLAGS_CUR] = 0; // previous step was not a slide -> retrigger
            let s = Step { note: target, cut: 0, gate: true, slide: false };
            self.begin(Seg::Note, s, s, self.step_len);
        }
    }

    pub fn note_off(&mut self, midi: u8) {
        let was_top = self.top() == Some(midi);
        self.remove(midi);
        if !was_top {
            return;
        }
        match self.top() {
            Some(n) => self.glide_to(semitone(n)),
            None => {
                self.hold();
                let st = &mut self.engine.p[idx::ENV_STATE];
                if *st < 3 {
                    *st = 3; // as when the step gate passes
                }
            }
        }
    }

    /// TS404 slide timing (used with a one-step lookahead): start gliding to `midi` now, one
    /// step before its note-on arrives via `push_held`, so the glide ends exactly where
    /// a slide step ends.
    pub fn slide_ahead(&mut self, midi: u8) {
        if self.n_held > 0 {
            self.glide_to(semitone(midi));
        }
    }

    /// The note-on whose glide already ran through `slide_ahead`.
    pub fn push_held(&mut self, midi: u8) {
        if self.n_held == 0 || self.note != semitone(midi) {
            return self.note_on(midi);
        }
        self.push(midi);
    }

    pub fn all_notes_off(&mut self) {
        self.n_held = 0;
        self.hold();
        self.engine.p[idx::ENV_STATE] = ENV_DONE;
    }

    fn remove(&mut self, midi: u8) {
        if let Some(i) = self.held[..self.n_held].iter().position(|&n| n == midi) {
            self.held.copy_within(i + 1..self.n_held, i);
            self.n_held -= 1;
        }
    }

    /// Render engine-rate samples. Splits at step boundaries.
    pub fn render(&mut self, t: &Tables, out: &mut [f32], custom: Option<&Wave>) {
        let mut done = 0;
        while done < out.len() {
            if self.pos >= self.len {
                match self.seg {
                    Seg::Glide => {
                        let s = Step { note: self.note, cut: 0, gate: false, slide: false };
                        self.begin(Seg::Arrival, s, s, self.step_len);
                    }
                    _ => self.hold(),
                }
            }
            self.engine.p[idx::STEP_GATE] = match self.seg {
                Seg::Note | Seg::Arrival => self.gate,
                Seg::Glide | Seg::Hold => 256,
            };
            let n = (out.len() - done).min((self.len - self.pos) as usize);
            self.engine.render(t, &mut out[done..done + n], self.pos, self.len, self.hq, custom);
            self.pos += n as i32;
            done += n;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legato_glides_and_release_decays() {
        let t = Tables::generate();
        let mut v = MonoVoice::default();
        let mut params = [0i32; idx::PARAM_COUNT];
        params[idx::OSC_MIX - 1] = 128;
        params[idx::CUTOFF - 1] = 100;
        params[idx::RESO_INV - 1] = 64;
        params[idx::SUSTAIN_LEVEL - 1] = 60;
        params[idx::SUSTAIN_TIME - 1] = 100;
        params[idx::RELEASE - 1] = 50;
        v.engine.set_params(&params);
        v.set_step_len(5000);
        let mut buf = vec![0f32; 20000];
        v.note_on(48);
        v.render(&t, &mut buf, None);
        assert!(buf[5000..].iter().any(|x| x.abs() > 0.05), "held note sounds");
        v.note_on(55);
        assert!(v.gliding());
        v.render(&t, &mut buf, None);
        assert_ne!(v.engine.p[idx::ENV_STATE], 0, "legato must not retrigger");
        v.note_off(55);
        v.note_off(48);
        for _ in 0..40 {
            v.render(&t, &mut buf, None);
        }
        assert!(v.is_silent());
        assert!(buf.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn every_midi_note_has_its_own_pitch() {
        let t = Tables::generate();
        let inc = |midi: u8, coarse: i32| t.pitch_at((semitone(midi) << 5) + (coarse << 5));
        for coarse in [-12, 0, 12] {
            for midi in 1..=127u8 {
                let (a, b) = (inc(midi - 1, coarse), inc(midi, coarse));
                assert!(b > a || b == 0x7fff_ffff, "MIDI {midi} (coarse {coarse}) is not above MIDI {}", midi - 1);
            }
        }
        // Rendered: low notes keep getting lower (zero crossings of a sine over 2 s).
        let crossings = |midi: u8| {
            let mut v = MonoVoice::default();
            let mut params = [0i32; idx::PARAM_COUNT];
            params[idx::OSC1_SHAPE - 1] = 2;
            params[idx::OSC2_SHAPE - 1] = 2;
            params[idx::FILTER_TYPE - 1] = 4;
            params[idx::SUSTAIN_LEVEL - 1] = 100;
            params[idx::SUSTAIN_TIME - 1] = 100;
            v.engine.set_params(&params);
            v.note_on(midi);
            let mut buf = vec![0f32; 88200];
            v.render(&t, &mut buf, None);
            buf.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count()
        };
        let counts: Vec<usize> = [0u8, 6, 12, 18, 24, 30].iter().map(|&m| crossings(m)).collect();
        assert!(counts.windows(2).all(|w| w[1] > w[0]), "{counts:?}");
    }

    #[test]
    fn chords_sound() {
        let t = Tables::generate();
        for size in 1..=6u8 {
            let mut v = MonoVoice::default();
            let mut params = [0i32; idx::PARAM_COUNT];
            params[idx::OSC_MIX - 1] = 128;
            params[idx::CUTOFF - 1] = 100;
            params[idx::RESO_INV - 1] = 64;
            params[idx::SUSTAIN_LEVEL - 1] = 60;
            params[idx::SUSTAIN_TIME - 1] = 100;
            v.engine.set_params(&params);
            v.set_step_len(5000);
            for k in 0..size {
                v.note_on(48 + 4 * k);
            }
            let mut buf = vec![0f32; 12000];
            v.render(&t, &mut buf, None);
            assert!(buf.iter().any(|x| x.abs() > 0.05), "{size}-note chord is silent");
            assert_eq!(v.note, semitone(48 + 4 * (size - 1)), "plays the last note");
        }
    }
}
