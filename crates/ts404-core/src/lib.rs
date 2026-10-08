//! Sound engine of MonoSynth: oscillators, filter, envelope, LFO, distortion, delay line
//! and the MIDI/step logic that drives them.

pub mod delay;
pub mod engine;
pub mod fpu;
pub mod mono;
pub mod shape;
pub mod resample;
pub mod steps;
pub mod synth;
pub mod tables;
pub mod x87;

pub use engine::{Engine, FLAG_GATE, FLAG_SLIDE, idx};
pub use tables::Tables;
