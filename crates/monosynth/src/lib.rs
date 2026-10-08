use std::num::NonZeroU32;
use std::sync::{Arc, OnceLock};

use nih_plug::prelude::*;
use ts404_core::Tables;
use ts404_core::resample::ENGINE_RATE;
use ts404_core::tables::{WAVE_LEN, Wave};
use ts404_core::synth::{Settings, Synth};

mod editor;
pub mod params;
pub mod presets;
mod shape_io;

use params::{StepLen, SynthParams};

pub fn tables() -> &'static Arc<Tables> {
    static T: OnceLock<Arc<Tables>> = OnceLock::new();
    T.get_or_init(|| Arc::new(Tables::generate()))
}

pub struct MonoSynth {
    params: Arc<SynthParams>,
    synth: Option<Synth>,
    left: Vec<f32>,
    right: Vec<f32>,
    /// The "?" shape as last handed to the synth, and its id (0 = none).
    shape_buf: Box<Wave>,
    shape_id: u64,
    /// Step length seen in the last block. Kept across (re)activations so the latency
    /// chosen when the host restarts us matches the one we asked for.
    last_step_len: i32,
}

impl Default for MonoSynth {
    fn default() -> Self {
        Self {
            params: Arc::new(SynthParams::default()),
            synth: None,
            left: Vec::new(),
            right: Vec::new(),
            shape_buf: vec![0f32; WAVE_LEN].into_boxed_slice().try_into().unwrap(),
            shape_id: 0,
            last_step_len: (15.0 / 120.0 * ENGINE_RATE as f64) as i32,
        }
    }
}

impl MonoSynth {
    fn settings(&self, tempo: Option<f64>) -> Settings {
        let p = &self.params;
        let sixteenth = ((15.0 / tempo.unwrap_or(120.0).max(1.0) * ENGINE_RATE as f64) as i32).max(1);
        let step_len = match tempo {
            None => self.last_step_len,
            Some(bpm) => {
                let bpm = bpm.max(1.0);
                let secs = match p.step_len.value() {
                    StepLen::ThirtySecond => 7.5 / bpm,
                    StepLen::Sixteenth => 15.0 / bpm,
                    StepLen::Eighth => 30.0 / bpm,
                    StepLen::Quarter => 60.0 / bpm,
                    StepLen::Free => p.step_ms.value() as f64 / 1000.0,
                };
                ((secs * ENGINE_RATE as f64) as i32).max(1)
            }
        };
        Settings {
            params: p.engine_params(),
            hq: p.hq.value(),
            step_len,
            gate: p.gate.value(),
            fl_slides: p.fl_slides.value(),
            sixteenth,
            delay_ticks: p.delay_time.value(),
            delay_feedback: p.delay_feedback.value(),
            delay_pan: p.delay_pan.value(),
            delay_volume: p.delay_vol.value(),
            pan: p.pan.value(),
        }
    }

    /// Pick up a newly loaded or restored "?" shape (never blocks the audio thread).
    fn sync_shape(&mut self) {
        let Some(synth) = self.synth.as_mut() else { return };
        let Ok(guard) = self.params.shape.try_read() else { return };
        match &*guard {
            Some(sd) if sd.id != self.shape_id => {
                if shape_io::decode_into(&sd.table, &mut self.shape_buf) {
                    synth.set_custom_wave(Some(&self.shape_buf));
                }
                self.shape_id = sd.id;
            }
            None if self.shape_id != 0 => {
                synth.set_custom_wave(None);
                self.shape_id = 0;
            }
            _ => {}
        }
    }
}

impl Plugin for MonoSynth {
    const NAME: &'static str = "MonoSynth";
    const VENDOR: &'static str = "Si1verThief";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout { main_input_channels: None, main_output_channels: NonZeroU32::new(2), ..AudioIOLayout::const_default() },
        AudioIOLayout { main_input_channels: None, main_output_channels: NonZeroU32::new(1), ..AudioIOLayout::const_default() },
    ];
    const MIDI_INPUT: MidiConfig = MidiConfig::Basic;
    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone())
    }

    fn initialize(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl InitContext<Self>) -> bool {
        let sr = config.sample_rate.round() as u32;
        let max = config.max_buffer_size as usize;
        self.left = vec![0.0; max];
        self.right = vec![0.0; max];
        self.shape_id = 0; // re-send the shape to the new synth
        let s = self.settings(None);
        let synth = Synth::new(tables().clone(), sr, max, Synth::lookahead_for(s.fl_slides, s.step_len));
        context.set_latency_samples(synth.latency());
        self.synth = Some(synth);
        true
    }

    fn reset(&mut self) {
        if let Some(s) = &mut self.synth {
            s.reset();
        }
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let n = buffer.samples();
        self.sync_shape();
        let s = self.settings(context.transport().tempo);
        self.last_step_len = s.step_len;
        let Some(synth) = self.synth.as_mut() else { return ProcessStatus::Normal };

        // TS404 slide timing needs a lookahead of one step; if the step got longer than the
        // lookahead we were activated with (or the switch changed), ask the host for the
        // new latency. It restarts us, and `initialize` then builds the same lookahead
        // from `last_step_len`, so this settles after one restart.
        if synth.needs_new_lookahead(&s) {
            let la = Synth::lookahead_for(s.fl_slides, s.step_len);
            context.set_latency_samples(Synth::latency_for(context.transport().sample_rate.round() as u32, la));
        }

        while let Some(event) = context.next_event() {
            let t = event.timing().min(n.saturating_sub(1) as u32);
            match event {
                NoteEvent::NoteOn { note, velocity, .. } if velocity > 0.0 => synth.note_on(t, note, &s),
                NoteEvent::NoteOn { note, .. } | NoteEvent::NoteOff { note, .. } | NoteEvent::Choke { note, .. } => synth.note_off(t, note),
                _ => {}
            }
        }

        synth.process(&s, &mut self.left[..n], &mut self.right[..n]);
        let stereo = buffer.channels() > 1;
        for (i, mut frame) in buffer.iter_samples().enumerate() {
            let g = self.params.gain.smoothed.next();
            let (l, r) = (self.left[i] * g, self.right[i] * g);
            let mut ch = frame.iter_mut();
            if stereo {
                *ch.next().unwrap() = l;
                *ch.next().unwrap() = r;
            } else if let Some(m) = ch.next() {
                *m = 0.5 * (l + r);
            }
        }
        ProcessStatus::Normal
    }
}

impl ClapPlugin for MonoSynth {
    const CLAP_ID: &'static str = "com.si1verthief.monosynth";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Monophonic acid synth with a bit-accurate TS404 engine");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::Instrument, ClapFeature::Synthesizer, ClapFeature::Mono, ClapFeature::Stereo];
}

impl Vst3Plugin for MonoSynth {
    const VST3_CLASS_ID: [u8; 16] = *b"Si1vMonoSynth404";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[Vst3SubCategory::Instrument, Vst3SubCategory::Synth, Vst3SubCategory::Mono];
}

nih_export_clap!(MonoSynth);
nih_export_vst3!(MonoSynth);
