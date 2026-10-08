use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use nih_plug::prelude::*;
use nih_plug_egui::EguiState;
use ts404_core::idx;

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum OscShape {
    Saw,
    #[name = "RC Pulse"]
    RcPulse,
    Sine,
    Square,
    /// "?": the loaded sample as a wavetable (the saw when none is loaded).
    Sample,
}

/// A loaded "?" shape, saved with the plugin state.
#[derive(Serialize, Deserialize, Clone, Default)]
pub struct ShapeData {
    pub name: String,
    /// 16384 little-endian f32, base64.
    pub table: String,
    /// Changes whenever a different shape is loaded.
    pub id: u64,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum FilterType {
    #[name = "LP 12"]
    Lp12,
    #[name = "LP 24"]
    Lp24,
    #[name = "HP"]
    Hp,
    #[name = "BP"]
    Bp,
    Off,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum LfoShape {
    Sine,
    Square,
    Triangle,
    Saw,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum LfoTarget {
    Pitch,
    Reso,
    Cutoff,
    #[name = "PW"]
    Pw,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum DistType {
    Soft,
    Hard,
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum StepLen {
    #[name = "1/32"]
    ThirtySecond,
    #[name = "1/16"]
    Sixteenth,
    #[name = "1/8"]
    Eighth,
    #[name = "1/4"]
    Quarter,
    #[name = "Free (ms)"]
    Free,
}

#[derive(Params)]
pub struct SynthParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<EguiState>,
    #[persist = "shape"]
    pub shape: Arc<RwLock<Option<ShapeData>>>,

    #[id = "o1shape"]
    pub osc1_shape: EnumParam<OscShape>,
    #[id = "o1coarse"]
    pub osc1_coarse: IntParam,
    #[id = "o1fine"]
    pub osc1_fine: IntParam,
    #[id = "o1pw"]
    pub osc1_pw: IntParam,
    #[id = "o2shape"]
    pub osc2_shape: EnumParam<OscShape>,
    #[id = "o2coarse"]
    pub osc2_coarse: IntParam,
    #[id = "o2fine"]
    pub osc2_fine: IntParam,
    #[id = "o2pw"]
    pub osc2_pw: IntParam,
    #[id = "mix"]
    pub osc_mix: IntParam,
    #[id = "ring"]
    pub ring: IntParam,
    #[id = "fm"]
    pub fm: IntParam,
    #[id = "sync"]
    pub sync: BoolParam,

    #[id = "ftype"]
    pub filter_type: EnumParam<FilterType>,
    #[id = "cutoff"]
    pub cutoff: IntParam,
    #[id = "reso"]
    pub reso: IntParam,
    #[id = "envamt"]
    pub env_amt: IntParam,

    #[id = "attack"]
    pub attack: IntParam,
    #[id = "decay"]
    pub decay: IntParam,
    #[id = "suslvl"]
    pub sustain_level: IntParam,
    #[id = "sustime"]
    pub sustain_time: IntParam,
    #[id = "release"]
    pub release: IntParam,
    #[id = "gate"]
    pub gate: IntParam,

    #[id = "lfoshape"]
    pub lfo_shape: EnumParam<LfoShape>,
    #[id = "lfotgt"]
    pub lfo_target: EnumParam<LfoTarget>,
    #[id = "lfoamt"]
    pub lfo_amt: IntParam,
    #[id = "lfospd"]
    pub lfo_speed: IntParam,

    #[id = "dtype"]
    pub dist_type: EnumParam<DistType>,
    #[id = "dthres"]
    pub dist_thres: IntParam,
    #[id = "damt"]
    pub dist_amount: IntParam,
    #[id = "hq"]
    pub hq: BoolParam,

    #[id = "delayamt"]
    pub delay_amt: IntParam,
    #[id = "dlfeed"]
    pub delay_feedback: IntParam,
    #[id = "dlpan"]
    pub delay_pan: IntParam,
    #[id = "dlvol"]
    pub delay_vol: IntParam,
    #[id = "dltime"]
    pub delay_time: IntParam,
    #[id = "pan"]
    pub pan: IntParam,

    #[id = "steplen"]
    pub step_len: EnumParam<StepLen>,
    #[id = "stepms"]
    pub step_ms: FloatParam,
    #[id = "flslide"]
    pub fl_slides: BoolParam,
    #[id = "gain"]
    pub gain: FloatParam,
}

fn int(name: &str, default: i32, min: i32, max: i32) -> IntParam {
    IntParam::new(name, default, IntRange::Linear { min, max })
}

impl Default for SynthParams {
    fn default() -> Self {
        // Defaults are the TS404's default patch.
        Self {
            editor_state: EguiState::from_size(490, 866),
            shape: Arc::new(RwLock::new(None)),
            osc1_shape: EnumParam::new("Osc 1 Shape", OscShape::Saw),
            osc1_coarse: int("Osc 1 Coarse", 0, -12, 12).with_unit(" st"),
            osc1_fine: int("Osc 1 Fine", 0, -128, 128),
            osc1_pw: int("Osc 1 PW", 0, 0, 256),
            osc2_shape: EnumParam::new("Osc 2 Shape", OscShape::Saw),
            osc2_coarse: int("Osc 2 Coarse", 0, -12, 12).with_unit(" st"),
            osc2_fine: int("Osc 2 Fine", 0, -128, 128),
            osc2_pw: int("Osc 2 PW", 0, 0, 256),
            osc_mix: int("Osc Mix", 128, 0, 256),
            ring: int("Ring Mod", 0, 0, 256),
            fm: int("FM", 0, 0, 256),
            sync: BoolParam::new("Sync", false),
            filter_type: EnumParam::new("Filter Type", FilterType::Lp12),
            cutoff: int("Cutoff", 64, 0, 128),
            reso: int("Resonance", 64, 0, 128),
            env_amt: int("Env Amount", 64, 0, 256),
            attack: int("Attack", 25, 0, 100),
            decay: int("Decay", 0, 0, 100),
            sustain_level: int("Sustain Level", 0, 0, 100),
            sustain_time: int("Sustain Time", 0, 0, 100),
            release: int("Release", 0, 0, 100),
            gate: int("Step Gate", 256, 0, 256),
            lfo_shape: EnumParam::new("LFO Shape", LfoShape::Sine),
            lfo_target: EnumParam::new("LFO Target", LfoTarget::Cutoff),
            lfo_amt: int("LFO Amount", 0, 0, 256),
            lfo_speed: int("LFO Speed", 0, 0, 100),
            dist_type: EnumParam::new("Dist Type", DistType::Soft),
            dist_thres: int("Dist Threshold", 0, 0, 10),
            dist_amount: int("Dist Amount", 0, 0, 128),
            hq: BoolParam::new("HQ Distortion", false),
            step_len: EnumParam::new("Slide Length", StepLen::Sixteenth),
            step_ms: FloatParam::new("Slide Time", 115.0, FloatRange::Skewed { min: 5.0, max: 2000.0, factor: 0.35 })
                .with_unit(" ms")
                .with_value_to_string(formatters::v2s_f32_rounded(0)),
            fl_slides: BoolParam::new("TS404 Slide Timing", false),
            delay_amt: int("Delay Amount", 0, 0, 128),
            delay_feedback: int("Delay Feedback", 64, 0, 127),
            delay_pan: int("Delay Pan", 64, 0, 128),
            delay_vol: int("Delay Volume", 128, 0, 128),
            delay_time: int("Delay Time", 48, 0, 768).with_value_to_string(Arc::new(|v| {
                // delay time counts in 1/48 of a step (a step = a 1/16 note)
                if v % 48 == 0 { format!("{} step{}", v / 48, if v == 48 { "" } else { "s" }) } else { format!("{:.2} steps", v as f32 / 48.0) }
            }))
            .with_string_to_value(Arc::new(|s| {
                let n: f32 = s.trim().trim_end_matches("steps").trim_end_matches("step").trim().parse().ok()?;
                Some((n * 48.0).round() as i32)
            })),
            pan: int("Pan", 64, 0, 128),
            gain: FloatParam::new(
                "Output",
                1.0,
                FloatRange::Skewed { min: util::db_to_gain(-30.0), max: util::db_to_gain(12.0), factor: FloatRange::gain_skew_factor(-30.0, 12.0) },
            )
            .with_smoother(SmoothingStyle::Logarithmic(30.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),
        }
    }
}

impl SynthParams {
    /// Engine ints 1..=35 (the .404 layout).
    pub fn engine_params(&self) -> [i32; idx::PARAM_COUNT] {
        let mut p = [0i32; idx::PARAM_COUNT];
        let mut set = |i: usize, v: i32| p[i - 1] = v;
        set(idx::OSC1_COARSE, self.osc1_coarse.value());
        set(idx::OSC2_COARSE, self.osc2_coarse.value());
        set(idx::OSC1_FINE, self.osc1_fine.value());
        set(idx::OSC2_FINE, self.osc2_fine.value());
        set(idx::OSC1_PW, self.osc1_pw.value());
        set(idx::OSC2_PW, self.osc2_pw.value());
        set(idx::SYNC, self.sync.value() as i32);
        set(idx::OSC1_SHAPE, self.osc1_shape.value() as i32);
        set(idx::OSC2_SHAPE, self.osc2_shape.value() as i32);
        set(idx::FILTER_TYPE, self.filter_type.value() as i32);
        set(idx::OSC_MIX, self.osc_mix.value());
        set(idx::DIST_TYPE, self.dist_type.value() as i32);
        set(idx::DIST_AMOUNT, self.dist_amount.value());
        set(idx::DIST_THRES, self.dist_thres.value());
        set(idx::CUTOFF, self.cutoff.value());
        set(idx::RESO_INV, 128 - self.reso.value());
        set(idx::ENV_AMT, self.env_amt.value());
        set(idx::LFO_AMT, self.lfo_amt.value());
        set(idx::LFO_SPEED, self.lfo_speed.value());
        set(idx::LFO_TARGET, self.lfo_target.value() as i32);
        set(idx::LFO_SHAPE, self.lfo_shape.value() as i32);
        set(idx::FM, self.fm.value());
        set(idx::RING, self.ring.value());
        set(idx::ATTACK, self.attack.value());
        set(idx::DECAY, self.decay.value());
        set(idx::RELEASE, self.release.value());
        set(idx::SUSTAIN_TIME, self.sustain_time.value());
        set(idx::SUSTAIN_LEVEL, self.sustain_level.value());
        set(idx::STEP_GATE, self.gate.value());
        set(idx::DELAY_AMT, self.delay_amt.value());
        p
    }

    /// Every parameter that a .404 preset carries, paired with its engine index.
    pub fn int_params(&self) -> [(&IntParam, usize, bool); 23] {
        [
            (&self.osc1_coarse, idx::OSC1_COARSE, false),
            (&self.osc2_coarse, idx::OSC2_COARSE, false),
            (&self.osc1_fine, idx::OSC1_FINE, false),
            (&self.osc2_fine, idx::OSC2_FINE, false),
            (&self.osc1_pw, idx::OSC1_PW, false),
            (&self.osc2_pw, idx::OSC2_PW, false),
            (&self.osc_mix, idx::OSC_MIX, false),
            (&self.dist_amount, idx::DIST_AMOUNT, false),
            (&self.dist_thres, idx::DIST_THRES, false),
            (&self.cutoff, idx::CUTOFF, false),
            (&self.reso, idx::RESO_INV, true),
            (&self.env_amt, idx::ENV_AMT, false),
            (&self.lfo_amt, idx::LFO_AMT, false),
            (&self.lfo_speed, idx::LFO_SPEED, false),
            (&self.fm, idx::FM, false),
            (&self.ring, idx::RING, false),
            (&self.attack, idx::ATTACK, false),
            (&self.decay, idx::DECAY, false),
            (&self.release, idx::RELEASE, false),
            (&self.sustain_time, idx::SUSTAIN_TIME, false),
            (&self.sustain_level, idx::SUSTAIN_LEVEL, false),
            (&self.gate, idx::STEP_GATE, false),
            (&self.delay_amt, idx::DELAY_AMT, false),
        ]
    }
}
