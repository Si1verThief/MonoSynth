use std::num::NonZeroU32;
use std::sync::{Arc, OnceLock};

use nice_plug::midi::Key;
use nice_plug::prelude::*;
use nice_plug_egui::RepaintNotifier;
use ts404_core::Tables;
use ts404_core::resample::ENGINE_RATE;
use ts404_core::tables::{WAVE_LEN, Wave};
use ts404_core::synth::{Settings, Synth};
use ts404_core::flsynth::{FlSettings, FlSynth};
use ts404_core::tables3::Fl3;

mod editor;
pub mod params;
pub mod presets;
mod shape_io;

use params::{FlVersion, StepLen, SynthParams};

pub fn tables() -> &'static Arc<Tables> {
    static T: OnceLock<Arc<Tables>> = OnceLock::new();
    T.get_or_init(|| Arc::new(Tables::generate()))
}

pub struct MonoSynth {
    params: Arc<SynthParams>,
    synth: Option<Synth>,
    /// The FL 3.5 / FL 6 engine.
    fl: Option<FlSynth>,
    /// Version seen in the last block (a switch silences the other engine).
    last_version: FlVersion,
    left: Vec<f32>,
    right: Vec<f32>,
    /// The "?" shape as last handed to the synth, and its id (0 = none).
    shape_buf: Box<Wave>,
    shape_id: u64,
    /// Step length seen in the last block. Kept across (re)activations so the latency
    /// chosen when the host restarts us matches the one we asked for.
    last_step_len: i32,
    /// Tells an open editor to redraw.
    repaint: RepaintNotifier,
}

impl Default for MonoSynth {
    fn default() -> Self {
        Self {
            params: Arc::new(SynthParams::default()),
            synth: None,
            fl: None,
            last_version: FlVersion::Fl271,
            left: Vec::new(),
            right: Vec::new(),
            shape_buf: vec![0f32; WAVE_LEN].into_boxed_slice().try_into().unwrap(),
            shape_id: 0,
            last_step_len: (15.0 / 120.0 * ENGINE_RATE as f64) as i32,
            repaint: RepaintNotifier::new(),
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
        let Ok(guard) = self.params.shape.try_read() else { return };
        let wave = match &*guard {
            Some(sd) if sd.id != self.shape_id => {
                self.shape_id = sd.id;
                if !shape_io::decode_into(&sd.table, &mut self.shape_buf) {
                    return;
                }
                Some(&*self.shape_buf)
            }
            None if self.shape_id != 0 => {
                self.shape_id = 0;
                None
            }
            _ => return,
        };
        if let Some(s) = self.synth.as_mut() {
            s.set_custom_wave(wave);
        }
        if let Some(f) = self.fl.as_mut() {
            f.set_custom_wave(wave);
        }
    }

    fn fl_settings(&self, v: Fl3, transport: &Transport) -> FlSettings {
        let p = &self.params;
        FlSettings {
            version: v,
            knobs: p.knobs(v),
            tempo: transport.tempo.unwrap_or(120.0),
            song_pos: if transport.playing { transport.pos_beats() } else { None },
            hq: p.hq.value(),
            aa: p.alias_free.value(),
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

    type Editor = editor::MonoEditor;
    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Self::Editor> {
        Some(editor::MonoEditor::new(self.params.clone(), self.repaint.clone()))
    }

    fn activate(&mut self, _layout: &AudioIOLayout, config: &BufferConfig, context: &mut impl ActivateContext<Self>) -> bool {
        let sr = config.sample_rate.round() as u32;
        let max = config.max_buffer_size as usize;
        self.left = vec![0.0; max];
        self.right = vec![0.0; max];
        self.shape_id = 0; // re-send the shape to the new synth
        let s = self.settings(None);
        let synth = Synth::new(tables().clone(), sr, max, Synth::lookahead_for(s.fl_slides, s.step_len));
        let version = self.params.version.value();
        let fl = FlSynth::new(tables().clone(), sr, max, version.fl3().unwrap_or(Fl3::V6));
        context.set_latency_samples(if version.fl3().is_some() { fl.latency() } else { synth.latency() });
        self.synth = Some(synth);
        self.fl = Some(fl);
        self.last_version = version;
        true
    }

    fn reset(&mut self) {
        if let Some(s) = &mut self.synth {
            s.reset();
        }
        if let Some(f) = &mut self.fl {
            f.reset();
        }
    }

    fn process(&mut self, buffer: &mut Buffer, _aux: &mut AuxiliaryBuffers, context: &mut impl ProcessContext<Self>) -> ProcessStatus {
        let n = buffer.samples();
        self.sync_shape();

        // FL 3.5 / FL 6
        let version = self.params.version.value();
        if version != self.last_version {
            if let Some(s) = &mut self.synth {
                s.reset();
            }
            if let Some(f) = &mut self.fl {
                f.reset();
            }
            let sr = context.transport().sample_rate.round() as u32;
            let latency = match version.fl3() {
                Some(_) => self.fl.as_ref().map_or(0, |f| f.latency()),
                None => self.synth.as_ref().map_or(0, |s| Synth::latency_for(sr, s.lookahead())),
            };
            context.set_latency_samples(latency);
            self.last_version = version;
        }
        if let Some(v) = version.fl3() {
            let fs = self.fl_settings(v, context.transport());
            let Some(fl) = self.fl.as_mut() else { return ProcessStatus::Normal };
            fl.set_settings(&fs);
            while let Some(event) = context.next_event() {
                let t = event.timing().min(n.saturating_sub(1) as u32);
                match event {
                    NoteEvent::NoteOn { key: Key::Number(note), velocity, .. } if velocity > 0.0 => fl.note_on(t, note, velocity),
                    NoteEvent::NoteOn { key, .. } | NoteEvent::NoteOff { key, .. } | NoteEvent::Choke { key, .. } => match key {
                        Key::Number(note) => fl.note_off(t, note),
                        Key::Wildcard => fl.all_notes_off(t),
                    },
                    _ => {}
                }
            }
            fl.process(&mut self.left[..n], &mut self.right[..n]);
            self.write_output(buffer);
            return ProcessStatus::Normal;
        }

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
                NoteEvent::NoteOn { key: Key::Number(note), velocity, .. } if velocity > 0.0 => synth.note_on(t, note, &s),
                NoteEvent::NoteOn { key, .. } | NoteEvent::NoteOff { key, .. } | NoteEvent::Choke { key, .. } => match key {
                    Key::Number(note) => synth.note_off(t, note),
                    // a note-off for every key
                    Key::Wildcard => (0..128).for_each(|n| synth.note_off(t, n)),
                },
                _ => {}
            }
        }

        synth.process(&s, &mut self.left[..n], &mut self.right[..n]);
        self.write_output(buffer);
        ProcessStatus::Normal
    }
}

impl MonoSynth {
    fn write_output(&mut self, buffer: &mut Buffer) {
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

nice_export_clap!(MonoSynth);
nice_export_vst3!(MonoSynth);
