//! Sound engine of MonoSynth: oscillators, filter, envelope, LFO, distortion, delay line
//! and the MIDI/step logic that drives them, in its FL 2.71 form (`engine`) and its
//! FL 3.5 / FL 6 form (`engine3`).

pub mod channel3;
pub mod channel6;
pub mod delay;
pub mod engine;
pub mod engine3;
pub mod flchan;
pub mod flsynth;
pub mod fpu;
pub mod mono;
pub mod shape;
pub mod resample;
pub mod steps;
pub mod synth;
pub mod tables;
pub mod tables3;
pub mod x87;

pub use engine::{Engine, FLAG_GATE, FLAG_SLIDE, idx};
pub use tables::Tables;
