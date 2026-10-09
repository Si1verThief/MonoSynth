use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};

use nice_plug::prelude::*;
use ts404_core::channel3::CHORDS;
use ts404_core::flchan::{self, ChannelKnobs};
use ts404_core::idx;
use ts404_core::tables3::Fl3;

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

/// The editor window's size in logical pixels, saved with the plugin state (same layout
/// as earlier versions saved it, so their sizes carry over).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct WindowState {
    pub size: (u32, u32),
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

/// Which FL version's TS404 to be.
#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum FlVersion {
    #[name = "FL 2.71"]
    Fl271,
    #[name = "FL 3.5"]
    Fl35,
    #[name = "FL 6"]
    Fl6,
}

impl FlVersion {
    /// The FL 3.5 / FL 6 engine version (None for FL 2.71).
    pub fn fl3(self) -> Option<Fl3> {
        match self {
            FlVersion::Fl271 => None,
            FlVersion::Fl35 => Some(Fl3::V35),
            FlVersion::Fl6 => Some(Fl3::V6),
        }
    }
}

#[derive(Enum, Debug, PartialEq, Eq, Clone, Copy)]
pub enum ArpDir {
    Off,
    Up,
    Down,
    #[name = "Up+Down"]
    UpDown,
    #[name = "Up+Down (repeat ends)"]
    UpDownRepeat,
    Random,
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
    pub window: Arc<RwLock<WindowState>>,
    /// The preset last chosen in the editor ("source/group/name", or "file:" and its name).
    #[persist = "preset"]
    pub preset: Arc<RwLock<String>>,
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

    // ---- version, and the FL 3.5 / FL 6 channel (FL's channel settings knobs)
    #[id = "version"]
    pub version: EnumParam<FlVersion>,
    #[id = "chvol"]
    pub ch_vol: IntParam,
    #[id = "chpan"]
    pub ch_pan: IntParam,
    #[id = "chpitch"]
    pub ch_pitch: IntParam,
    #[id = "root"]
    pub root: IntParam,
    #[id = "chfine"]
    pub ch_fine: IntParam,
    #[id = "chcut"]
    pub ch_cut: IntParam,
    #[id = "chres"]
    pub ch_res: IntParam,
    #[id = "adjpan"]
    pub adj_pan: IntParam,
    #[id = "adjvol"]
    pub adj_vol: IntParam,
    #[id = "adjcut"]
    pub adj_cut: IntParam,
    #[id = "adjres"]
    pub adj_res: IntParam,
    /// MIDI notes act like keys played into FL (off: like piano-roll notes).
    #[id = "livekeys"]
    pub live_keys: BoolParam,
    #[id = "mono"]
    pub mono: BoolParam,
    #[id = "porta"]
    pub porta: BoolParam,
    #[id = "portatime"]
    pub porta_time: IntParam,
    #[id = "maxpoly"]
    pub max_poly: IntParam,
    #[id = "chgate"]
    pub ch_gate: IntParam,
    #[id = "gateskip"]
    pub gate_skip: BoolParam,
    #[id = "keylo"]
    pub key_lo: IntParam,
    #[id = "keyhi"]
    pub key_hi: IntParam,
    #[id = "shift"]
    pub shift: IntParam,
    #[id = "efeed"]
    pub echo_feed: IntParam,
    #[id = "epan"]
    pub echo_pan: IntParam,
    #[id = "epitch"]
    pub echo_pitch: IntParam,
    #[id = "ecount"]
    pub echoes: IntParam,
    #[id = "etime"]
    pub echo_time: IntParam,
    #[id = "ecut"]
    pub echo_cut: IntParam,
    #[id = "eres"]
    pub echo_res: IntParam,
    #[id = "epp"]
    pub echo_pingpong: BoolParam,
    #[id = "ebounce"]
    pub echo_bounce: BoolParam,
    #[id = "arpdir"]
    pub arp_dir: EnumParam<ArpDir>,
    #[id = "arprange"]
    pub arp_range: IntParam,
    #[id = "arpchord"]
    pub arp_chord: IntParam,
    #[id = "arprep"]
    pub arp_repeat: IntParam,
    #[id = "arptime"]
    pub arp_time: IntParam,
    #[id = "arpgate"]
    pub arp_gate: IntParam,
    #[id = "arpslide"]
    pub arp_slide: BoolParam,
    #[id = "velmid"]
    pub vel_mid: IntParam,
    #[id = "velpan"]
    pub vel_pan: IntParam,
    #[id = "velcut"]
    pub vel_cut: IntParam,
    #[id = "velres"]
    pub vel_res: IntParam,
    #[id = "keymid"]
    pub key_mid: IntParam,
    #[id = "keypan"]
    pub key_pan: IntParam,
    #[id = "keycut"]
    pub key_cut: IntParam,
    #[id = "keyres"]
    pub key_res: IntParam,
    #[id = "aa"]
    pub alias_free: BoolParam,
}

/// FL's note names (C5 = 60).
pub fn note_name(n: i32) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", N[n.rem_euclid(12) as usize], n.div_euclid(12))
}

/// Parse a note name ("C#5", "a3") or a note number.
pub fn parse_note(s: &str) -> Option<i32> {
    let s = s.trim();
    if let Ok(n) = s.parse::<i32>() {
        return Some(n);
    }
    let mut c = s.chars();
    let base = match c.next()?.to_ascii_uppercase() {
        'C' => 0,
        'D' => 2,
        'E' => 4,
        'F' => 5,
        'G' => 7,
        'A' => 9,
        'B' => 11,
        _ => return None,
    };
    let rest = c.as_str();
    let (sharp, oct) = if let Some(r) = rest.strip_prefix('#') { (1, r) } else { (0, rest) };
    Some(base + sharp + 12 * oct.trim().parse::<i32>().ok()?)
}

/// A time knob's length in steps, and the knob value.
fn time_text(knob: i32) -> String {
    let t = flchan::ticks(knob);
    let steps = if t % 24 == 0 { format!("{} step{}", t / 24, if t == 24 { "" } else { "s" }) } else { format!("{:.2} steps", t as f32 / 24.0) };
    format!("{steps} ({knob})")
}

/// Parse a time: "… (knob)", "off", a length in steps ("0.5 steps") or a knob value.
fn parse_time(s: &str) -> Option<i32> {
    let s = s.trim().to_ascii_lowercase();
    if s.starts_with("off") {
        return Some(flchan::KNOB_OFF);
    }
    if let (Some(a), Some(b)) = (s.rfind('('), s.rfind(')')) {
        return s.get(a + 1..b)?.trim().parse().ok();
    }
    if let Some(n) = s.strip_suffix("steps").or_else(|| s.strip_suffix("step")) {
        let steps: f32 = n.trim().parse().ok()?;
        return Some(flchan::knob_for_units48((steps * 48.0).round() as i32));
    }
    s.parse().ok()
}

fn chord_name(c: i32) -> String {
    match c {
        ..=-1 => "Auto (held notes)".into(),
        0 => "None".into(),
        c => CHORDS.get(c as usize - 1).map(|c| c.0.to_string()).unwrap_or_else(|| "?".into()),
    }
}

fn parse_chord(s: &str) -> Option<i32> {
    let s = s.trim();
    if s.to_ascii_lowercase().starts_with("auto") {
        return Some(-1);
    }
    if s.eq_ignore_ascii_case("none") {
        return Some(0);
    }
    if let Some(i) = CHORDS.iter().position(|c| c.0 == s) {
        return Some(i as i32 + 1);
    }
    if let Some(i) = CHORDS.iter().position(|c| c.0.eq_ignore_ascii_case(s)) {
        return Some(i as i32 + 1);
    }
    s.parse().ok()
}

fn int(name: &str, default: i32, min: i32, max: i32) -> IntParam {
    IntParam::new(name, default, IntRange::Linear { min, max })
}

impl Default for SynthParams {
    fn default() -> Self {
        // Defaults are the TS404's default patch.
        Self {
            window: Arc::new(RwLock::new(WindowState { size: crate::editor::DEFAULT_SIZE })),
            preset: Arc::new(RwLock::new(String::new())),
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

            version: EnumParam::new("FL Version", FlVersion::Fl271),
            ch_vol: int("Channel Volume", 100, 0, 128),
            ch_pan: int("Channel Pan", 64, 0, 128),
            ch_pitch: int("Channel Pitch", 0, -1200, 1200).with_unit(" ct"),
            root: int("Root Note", 60, 0, 131).with_value_to_string(Arc::new(note_name)).with_string_to_value(Arc::new(parse_note)),
            ch_fine: int("Fine Tune", 0, -100, 100).with_unit(" ct"),
            ch_cut: int("Channel Cutoff", 120, 0, 256),
            ch_res: int("Channel Resonance", 80, 0, 256),
            adj_pan: int("Pan Adjust", 0, -64, 64),
            adj_vol: int("Volume Adjust", 128, 0, 128),
            adj_cut: int("Cutoff Adjust", 0, -256, 256),
            adj_res: int("Resonance Adjust", 0, -256, 256),
            live_keys: BoolParam::new("Live Keys", false),
            mono: BoolParam::new("Mono", true),
            porta: BoolParam::new("Portamento", false),
            porta_time: int("Slide Time", 500, 0, 1446).with_value_to_string(Arc::new(time_text)).with_string_to_value(Arc::new(parse_time)),
            max_poly: int("Max Polyphony", 0, 0, 32)
                .with_value_to_string(Arc::new(|v| if v == 0 { "∞".into() } else { v.to_string() }))
                .with_string_to_value(Arc::new(|s| if s.trim() == "∞" { Some(0) } else { s.trim().parse().ok() })),
            ch_gate: int("Gate", 800, 0, flchan::KNOB_OFF)
                .with_value_to_string(Arc::new(|v| if v >= flchan::KNOB_OFF { "Off".into() } else { time_text(v) }))
                .with_string_to_value(Arc::new(parse_time)),
            gate_skip: BoolParam::new("Gate Skips Slides", true),
            key_lo: int("Key Range Low", 0, 0, 256).with_value_to_string(Arc::new(note_name)).with_string_to_value(Arc::new(parse_note)),
            key_hi: int("Key Range High", 256, 0, 256).with_value_to_string(Arc::new(note_name)).with_string_to_value(Arc::new(parse_note)),
            shift: int("Time Shift", 0, 0, 1446).with_value_to_string(Arc::new(time_text)).with_string_to_value(Arc::new(parse_time)),
            echo_feed: int("Echo Feedback", 0, 0, 128),
            echo_pan: int("Echo Pan", 64, 0, 128),
            echo_pitch: int("Echo Pitch", 0, -1200, 1200).with_unit(" ct"),
            echoes: int("Echoes", 4, 1, 20),
            echo_time: int("Echo Time", 144, 1, 768)
                .with_value_to_string(Arc::new(|v| format!("{:.2} steps", v as f32 / 48.0)))
                .with_string_to_value(Arc::new(|s| {
                    let s = s.trim().to_ascii_lowercase();
                    let n = s.trim_end_matches("steps").trim_end_matches("step").trim();
                    n.parse::<f32>().ok().map(|x| (x * 48.0).round() as i32)
                })),
            echo_cut: int("Echo Cutoff", 128, 0, 256),
            echo_res: int("Echo Resonance", 128, 0, 256),
            echo_pingpong: BoolParam::new("Echo Ping-Pong", false),
            echo_bounce: BoolParam::new("Echo Pan Bounce", false),
            arp_dir: EnumParam::new("Arpeggio", ArpDir::Off),
            arp_range: int("Arp Range", 1, 0, 5).with_unit(" oct"),
            arp_chord: int("Arp Chord", -1, -1, CHORDS.len() as i32).with_value_to_string(Arc::new(chord_name)).with_string_to_value(Arc::new(parse_chord)),
            arp_repeat: int("Arp Repeat", 1, 1, 8),
            arp_time: int("Arp Time", 1024, 0, flchan::KNOB_OFF)
                .with_value_to_string(Arc::new(|v| if v >= flchan::KNOB_OFF { "Off (per note)".into() } else { time_text(v) }))
                .with_string_to_value(Arc::new(parse_time)),
            arp_gate: int("Arp Gate", 48, 0, 48),
            arp_slide: BoolParam::new("Arp Slide", false),
            vel_mid: int("Velocity Mid", 100, 0, 128),
            vel_pan: int("Velocity > Pan", 0, -128, 128),
            vel_cut: int("Velocity > Cutoff", 0, -128, 128),
            vel_res: int("Velocity > Resonance", 0, -128, 128),
            key_mid: int("Key Mid", 60, 0, 131).with_value_to_string(Arc::new(note_name)).with_string_to_value(Arc::new(parse_note)),
            key_pan: int("Key > Pan", 0, -256, 256),
            key_cut: int("Key > Cutoff", 0, -256, 256),
            key_res: int("Key > Resonance", 0, -256, 256),
            alias_free: BoolParam::new("Alias-Free", false),
        }
    }
}

impl SynthParams {
    /// The FL 3.5 / FL 6 channel these parameters describe (the TS404's own parameters
    /// included).
    pub fn knobs(&self, v: Fl3) -> ChannelKnobs {
        let mut k = ChannelKnobs::new(v);
        let ep = self.engine_params();
        k.ts[1..34].copy_from_slice(&ep[..33]);
        k.pan = self.ch_pan.value();
        k.vol = self.ch_vol.value();
        k.pitch = self.ch_pitch.value();
        k.root = self.root.value();
        k.fine = self.ch_fine.value();
        k.cut = self.ch_cut.value();
        k.res = self.ch_res.value();
        k.pan_adj = self.adj_pan.value();
        k.vol_adj = self.adj_vol.value();
        k.cut_adj = self.adj_cut.value();
        k.res_adj = self.adj_res.value();
        k.mono = self.mono.value();
        k.porta = self.porta.value();
        k.porta_time = self.porta_time.value();
        k.max_poly = self.max_poly.value();
        k.gate = self.ch_gate.value();
        k.gate_skips_slides = self.gate_skip.value();
        k.key_lo = self.key_lo.value();
        k.key_hi = self.key_hi.value();
        k.shift = self.shift.value();
        k.echo_feed = self.echo_feed.value();
        k.echo_pan = self.echo_pan.value();
        k.echo_pitch = self.echo_pitch.value();
        k.echoes = self.echoes.value();
        k.echo_time = self.echo_time.value();
        k.echo_cut = self.echo_cut.value();
        k.echo_res = self.echo_res.value();
        k.echo_pingpong = self.echo_pingpong.value();
        k.echo_bounce = self.echo_bounce.value();
        k.arp_dir = self.arp_dir.value() as i32;
        k.arp_range = self.arp_range.value();
        k.arp_chord = self.arp_chord.value();
        k.arp_repeat = self.arp_repeat.value();
        k.arp_time = self.arp_time.value();
        k.arp_gate = self.arp_gate.value();
        k.arp_slide = self.arp_slide.value();
        k.vel_mid = self.vel_mid.value();
        k.vel_amt = [self.vel_pan.value(), self.vel_cut.value(), self.vel_res.value()];
        k.key_mid = self.key_mid.value();
        k.key_amt = [self.key_pan.value(), self.key_cut.value(), self.key_res.value()];
        k
    }

    /// The channel parameters (not the TS404's), paired with their value in `k`.
    pub fn channel_ints<'a>(&'a self, k: &ChannelKnobs) -> Vec<(&'a IntParam, i32)> {
        vec![
            (&self.ch_pan, k.pan), (&self.ch_vol, k.vol), (&self.ch_pitch, k.pitch), (&self.root, k.root), (&self.ch_fine, k.fine),
            (&self.ch_cut, k.cut), (&self.ch_res, k.res), (&self.adj_pan, k.pan_adj), (&self.adj_vol, k.vol_adj),
            (&self.adj_cut, k.cut_adj), (&self.adj_res, k.res_adj), (&self.porta_time, k.porta_time), (&self.max_poly, k.max_poly),
            (&self.ch_gate, k.gate), (&self.key_lo, k.key_lo), (&self.key_hi, k.key_hi), (&self.shift, k.shift),
            (&self.echo_feed, k.echo_feed), (&self.echo_pan, k.echo_pan), (&self.echo_pitch, k.echo_pitch), (&self.echoes, k.echoes),
            (&self.echo_time, k.echo_time), (&self.echo_cut, k.echo_cut), (&self.echo_res, k.echo_res), (&self.arp_range, k.arp_range),
            (&self.arp_chord, k.arp_chord), (&self.arp_repeat, k.arp_repeat), (&self.arp_time, k.arp_time), (&self.arp_gate, k.arp_gate),
            (&self.vel_mid, k.vel_mid), (&self.vel_pan, k.vel_amt[0]), (&self.vel_cut, k.vel_amt[1]), (&self.vel_res, k.vel_amt[2]),
            (&self.key_mid, k.key_mid), (&self.key_pan, k.key_amt[0]), (&self.key_cut, k.key_amt[1]), (&self.key_res, k.key_amt[2]),
        ]
    }

    pub fn channel_bools<'a>(&'a self, k: &ChannelKnobs) -> Vec<(&'a BoolParam, bool)> {
        vec![
            (&self.mono, k.mono), (&self.porta, k.porta), (&self.gate_skip, k.gate_skips_slides),
            (&self.echo_pingpong, k.echo_pingpong), (&self.echo_bounce, k.echo_bounce), (&self.arp_slide, k.arp_slide),
        ]
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every parameter's text roundtrips (hosts type values in).
    #[test]
    fn value_text_roundtrips() {
        let p = SynthParams::default();
        for (id, ptr, _) in p.param_map() {
            for k in 0..=200 {
                let norm = k as f32 / 200.0;
                let text = unsafe { ptr.normalized_value_to_string(norm, false) };
                let back = unsafe { ptr.string_to_normalized_value(&text) };
                assert!(back.is_some(), "{id}: can't parse {text:?}");
                let again = unsafe { ptr.normalized_value_to_string(back.unwrap(), false) };
                assert_eq!(text, again, "{id}");
            }
        }
    }
}
