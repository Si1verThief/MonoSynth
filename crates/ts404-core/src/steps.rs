//! Step handshake: what the engine needs to know at every step boundary.

use crate::engine::{Engine, FLAG_GATE, FLAG_SLIDE, idx};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Step {
    /// Semitone index into the pitch table (MIDI note - 12), 12..=101.
    pub note: i32,
    /// Per-step cutoff offset, in 1/128 cutoff-table steps.
    pub cut: i32,
    pub gate: bool,
    pub slide: bool,
}

impl Step {
    pub fn flags(&self) -> i32 {
        (if self.gate { FLAG_GATE } else { 0 }) | (if self.slide { FLAG_SLIDE } else { 0 })
    }
}

impl Engine {
    /// Called at every step boundary before rendering the step: `cur` plays now, `next`
    /// is the lookahead (slide target and cutoff interpolation end point).
    pub fn begin_step(&mut self, cur: &Step, next: &Step) {
        let p = &mut self.p;
        p[idx::PREV_FLAGS] = p[idx::FLAGS_CUR];
        p[idx::CUT_CUR] = cur.cut;
        p[idx::NOTE_CUR] = cur.note << 5;
        p[idx::FLAGS_CUR] = cur.flags();
        p[idx::CUT_NEXT] = next.cut;
        p[idx::NOTE_NEXT] = next.note << 5;
        p[idx::FLAGS_NEXT] = next.flags();
    }
}
