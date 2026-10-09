//! .404 preset files (TS404 format: parameters 1..=35 as little-endian ints), the
//! built-in presets and the factory presets of FL 2.71, FL 3.5 and FL 6.

use std::sync::OnceLock;

use ts404_core::flchan::{self, ChannelKnobs};
use ts404_core::idx;
use ts404_core::tables3::Fl3;

use crate::params::FlVersion;

/// A factory preset: the version it belongs to, its group and name, and the original
/// preset data (.404 parameters or a .fst channel preset).
pub struct Factory {
    pub version: FlVersion,
    pub group: String,
    pub name: String,
    pub fst: bool,
    pub data: Vec<u8>,
}

/// What a preset sets: FL 2.71 parameters, or an FL 3.5 / FL 6 channel.
pub enum Loaded {
    Fl271(Preset404),
    Channel(Fl3, ChannelKnobs),
}

impl Factory {
    /// The preset as its FL version loads it onto a new channel.
    pub fn load(&self) -> Option<Loaded> {
        match self.version.fl3() {
            None if self.fst => parse_flp(&self.data).and_then(|f| f.channels.into_iter().next()).map(|c| Loaded::Fl271(c.1)),
            None => parse_404(&self.data).map(Loaded::Fl271),
            Some(v) => {
                let base = ChannelKnobs::new(v);
                let k = if self.fst { flchan::load_fst(v, &base, &self.data) } else { flchan::load_404(v, &base, &self.data) };
                k.map(|k| Loaded::Channel(v, k))
            }
        }
    }
}

pub fn factory() -> &'static [Factory] {
    static F: OnceLock<Vec<Factory>> = OnceLock::new();
    F.get_or_init(|| parse_factory(include_bytes!("../presets/factory.bin")).unwrap_or_default())
}

fn parse_factory(b: &[u8]) -> Option<Vec<Factory>> {
    if b.get(..4)? != b"MSP1" {
        return None;
    }
    let n = u32::from_le_bytes(b.get(4..8)?.try_into().ok()?) as usize;
    let mut p = 8;
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let version = match *b.get(p)? {
            0 => FlVersion::Fl271,
            1 => FlVersion::Fl35,
            _ => FlVersion::Fl6,
        };
        let fst = *b.get(p + 1)? == 1;
        p += 2;
        let mut text = || -> Option<String> {
            let len = *b.get(p)? as usize;
            let t = String::from_utf8_lossy(b.get(p + 1..p + 1 + len)?).into_owned();
            p += 1 + len;
            Some(t)
        };
        let group = text()?;
        let name = text()?;
        let len = u32::from_le_bytes(b.get(p..p + 4)?.try_into().ok()?) as usize;
        let data = b.get(p + 4..p + 4 + len)?.to_vec();
        p += 4 + len;
        out.push(Factory { version, group, name, fst, data });
    }
    Some(out)
}

pub type Preset404 = [i32; idx::PARAM_COUNT];

pub fn parse_404(bytes: &[u8]) -> Option<Preset404> {
    if bytes.len() < 4 * idx::PARAM_COUNT {
        return None;
    }
    let mut p = [0i32; idx::PARAM_COUNT];
    for (i, v) in p.iter_mut().enumerate() {
        *v = i32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap());
    }
    Some(p)
}

pub fn write_404(p: &Preset404) -> Vec<u8> {
    p.iter().flat_map(|v| v.to_le_bytes()).collect()
}

/// Build a preset from (engine index, value) pairs on top of the default patch.
const fn preset(over: &[(usize, i32)]) -> Preset404 {
    let mut p = [0i32; idx::PARAM_COUNT];
    p[idx::OSC_MIX - 1] = 128;
    p[idx::CUTOFF - 1] = 64;
    p[idx::RESO_INV - 1] = 64;
    p[idx::ENV_AMT - 1] = 64;
    p[idx::LFO_TARGET - 1] = 2;
    p[idx::ATTACK - 1] = 25;
    p[idx::STEP_GATE - 1] = 256; // built-ins follow MIDI note lengths; .404 imports keep their own gate
    let mut i = 0;
    while i < over.len() {
        let (k, v) = over[i];
        p[k - 1] = v;
        i += 1;
    }
    p
}

use idx::*;

/// Resonance values here are knob values; presets store them inverted.
const fn reso(r: i32) -> (usize, i32) {
    (RESO_INV, 128 - r)
}

pub const BUILTIN: &[(&str, Preset404)] = &[
    ("Init", preset(&[])),
    ("Acid Line", preset(&[(FILTER_TYPE, 1), (CUTOFF, 38), reso(112), (ENV_AMT, 210), (ATTACK, 0), (DECAY, 45), (SUSTAIN_LEVEL, 0), (RELEASE, 40), (OSC_MIX, 0)])),
    ("Acid Square", preset(&[(OSC1_SHAPE, 3), (OSC2_SHAPE, 3), (FILTER_TYPE, 1), (CUTOFF, 30), reso(118), (ENV_AMT, 230), (ATTACK, 0), (DECAY, 55), (RELEASE, 35), (OSC_MIX, 0)])),
    ("Squelch Dist", preset(&[(FILTER_TYPE, 0), (CUTOFF, 34), reso(122), (ENV_AMT, 240), (ATTACK, 0), (DECAY, 50), (RELEASE, 30), (OSC_MIX, 0), (DIST_TYPE, 0), (DIST_THRES, 6), (DIST_AMOUNT, 110)])),
    ("Sub Bass", preset(&[(OSC1_SHAPE, 2), (OSC2_SHAPE, 3), (OSC2_COARSE, -12), (OSC_MIX, 60), (FILTER_TYPE, 1), (CUTOFF, 45), reso(10), (ENV_AMT, 40), (ATTACK, 0), (DECAY, 80), (SUSTAIN_LEVEL, 70), (SUSTAIN_TIME, 100), (RELEASE, 55)])),
    ("Detuned Saws", preset(&[(OSC2_FINE, 18), (OSC1_FINE, -14), (FILTER_TYPE, 0), (CUTOFF, 70), reso(40), (ENV_AMT, 90), (ATTACK, 0), (DECAY, 70), (SUSTAIN_LEVEL, 55), (SUSTAIN_TIME, 100), (RELEASE, 70)])),
    ("Wobble", preset(&[(FILTER_TYPE, 1), (CUTOFF, 40), reso(90), (ENV_AMT, 30), (LFO_TARGET, 2), (LFO_SHAPE, 0), (LFO_AMT, 230), (LFO_SPEED, 52), (ATTACK, 0), (SUSTAIN_LEVEL, 100), (SUSTAIN_TIME, 100), (RELEASE, 60)])),
    ("Ring Bell", preset(&[(OSC1_SHAPE, 2), (OSC2_SHAPE, 2), (OSC2_COARSE, 7), (OSC2_FINE, 40), (RING, 220), (FILTER_TYPE, 4), (ATTACK, 0), (DECAY, 75), (SUSTAIN_LEVEL, 0), (RELEASE, 80)])),
    ("FM Grit", preset(&[(OSC1_SHAPE, 2), (OSC2_SHAPE, 0), (FM, 150), (OSC_MIX, 256), (FILTER_TYPE, 0), (CUTOFF, 75), reso(70), (ENV_AMT, 120), (ATTACK, 0), (DECAY, 50), (SUSTAIN_LEVEL, 30), (SUSTAIN_TIME, 100), (RELEASE, 45)])),
    ("Sync Lead", preset(&[(SYNC, 1), (OSC2_COARSE, -12), (OSC1_COARSE, 7), (OSC_MIX, 0), (FILTER_TYPE, 0), (CUTOFF, 85), reso(50), (ENV_AMT, 60), (LFO_TARGET, 0), (LFO_AMT, 12), (LFO_SPEED, 60), (ATTACK, 0), (DECAY, 60), (SUSTAIN_LEVEL, 60), (SUSTAIN_TIME, 100), (RELEASE, 65)])),
    ("PW Pad", preset(&[(OSC1_SHAPE, 3), (OSC2_SHAPE, 3), (OSC2_FINE, 10), (OSC1_PW, 80), (OSC2_PW, 160), (LFO_TARGET, 3), (LFO_SHAPE, 2), (LFO_AMT, 120), (LFO_SPEED, 30), (FILTER_TYPE, 1), (CUTOFF, 60), reso(30), (ENV_AMT, 50), (ATTACK, 45), (DECAY, 90), (SUSTAIN_LEVEL, 80), (SUSTAIN_TIME, 100), (RELEASE, 85)])),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        for (_, p) in BUILTIN {
            assert_eq!(parse_404(&write_404(p)).unwrap(), *p);
        }
    }

    #[test]
    fn factory_presets_load() {
        let f = factory();
        assert!(f.len() > 300, "{} factory presets", f.len());
        for p in f {
            assert!(p.load().is_some(), "{} / {} doesn't load", p.group, p.name);
        }
        for v in [FlVersion::Fl271, FlVersion::Fl35, FlVersion::Fl6] {
            assert!(f.iter().any(|p| p.version == v));
        }
    }
}

/// The TS404 parts of an FL project (.flp): each TS404 channel's parameters (the .404
/// layout) and the song's TS404 delay line (feedback, pan, volume, time in 1/48 steps).
#[derive(Debug, Default)]
pub struct FlpTs404 {
    pub channels: Vec<(String, Preset404)>,
    pub delay_line: Option<[i32; 4]>,
}

pub fn parse_flp(b: &[u8]) -> Option<FlpTs404> {
    let u32le = |o: usize| b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()));
    if b.get(0..4)? != b"FLhd" {
        return None;
    }
    let mut p = 8 + u32le(4)? as usize;
    if b.get(p..p + 4)? != b"FLdt" {
        return None;
    }
    let end = (p + 8 + u32le(p + 4)? as usize).min(b.len());
    p += 8;
    let mut out = FlpTs404::default();
    let mut name = String::new();
    let mut chan = -1i32;
    while p < end {
        let ev = b[p];
        p += 1;
        let data: &[u8] = match ev {
            0..=63 => { p += 1; &[] }
            64..=127 => {
                if ev == 64 {
                    chan = u16::from_le_bytes([*b.get(p)?, *b.get(p + 1)?]) as i32;
                    name.clear();
                }
                p += 2;
                &[]
            }
            128..=191 => { p += 4; &[] }
            _ => {
                let (mut n, mut shift) = (0usize, 0);
                loop {
                    let c = *b.get(p)?;
                    p += 1;
                    n |= ((c & 0x7f) as usize) << shift;
                    shift += 7;
                    if c & 0x80 == 0 {
                        break;
                    }
                }
                let d = b.get(p..p + n)?;
                p += n;
                d
            }
        };
        match ev {
            192 => name = String::from_utf8_lossy(data).trim_end_matches('\0').to_string(),
            210 => {
                if let Some(pr) = parse_404(data) {
                    let label = if name.is_empty() { format!("channel {}", chan + 1) } else { name.clone() };
                    out.channels.push((label, pr));
                }
            }
            211 if data.len() >= 16 => {
                let v = |i: usize| i32::from_le_bytes(data[4 * i..4 * i + 4].try_into().unwrap());
                out.delay_line = Some([v(0), v(1), v(2), v(3)]);
            }
            _ => {}
        }
    }
    Some(out)
}
