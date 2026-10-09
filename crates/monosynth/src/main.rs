//! Standalone host (JACK/ALSA) for trying the synth without a DAW: `cargo run --release --features standalone`.
use monosynth::MonoSynth;
use nice_plug::prelude::*;

fn main() {
    nice_export_standalone::<MonoSynth>();
}
