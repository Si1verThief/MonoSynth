//! Standalone host (JACK/ALSA) for trying the synth without a DAW: `cargo run --release --features standalone`.
use monosynth::MonoSynth;
use nih_plug::prelude::*;

fn main() {
    nih_export_standalone::<MonoSynth>();
}
