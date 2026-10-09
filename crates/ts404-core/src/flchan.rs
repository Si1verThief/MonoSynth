//! An FL 3.5 / FL 6 TS404 channel as FL stores it: the TS404's parameters plus the
//! channel's knobs (volume, pan, pitch, filter, polyphony, echo, arpeggiator, keyboard
//! tracking, ...), how FL turns those knobs into what the channel engine uses, and FL's
//! loaders for .404 and .fst presets.
//!
//! Knob values use FL 3.5's integer units in both versions (FL 6 stores pan, volume and
//! echo feedback/pan multiplied by 100; its presets are all whole multiples).

use crate::channel3::ChannelSettings;
use crate::channel6::ChannelSettings6;
use crate::fpu;
use crate::tables3::{log_scale3, Fl3};
use crate::x87::{e, ei, Ext};

/// TS404 parameters as a new TS404 gets them (FL's constructor), indexed 1..=33.
pub const TS404_DEFAULT: [i32; 34] = {
    let mut p = [0i32; 34];
    p[1] = -12;
    p[3] = -8;
    p[10] = 3;
    p[11] = 1;
    p[12] = 256;
    p[15] = 10;
    p[18] = 156;
    p[21] = 2;
    p[27] = 256;
    p[29] = 15;
    p[30] = 25;
    p[31] = 32;
    p[32] = 73;
    p[33] = 50;
    p
};

/// The gate knob's "off" position (also the arpeggiator time's).
pub const KNOB_OFF: i32 = 1447;

/// Ticks per step.
pub const TICKS_PER_STEP: i32 = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChannelKnobs {
    /// TS404 parameters 1..=33 (FL 2's layout; cutoff and resonance are the channel's).
    pub ts: [i32; 34],
    /// Pan 0..128 (64 = centre), volume 0..128 (FL 3.5 goes to 160), pitch (cents).
    pub pan: i32,
    pub vol: i32,
    pub pitch: i32,
    /// Filter cutoff and resonance, 0..256.
    pub cut: i32,
    pub res: i32,
    /// Level adjustments (FL's "level adjust" knobs): pan, volume (128 = unity), cutoff, resonance.
    pub pan_adj: i32,
    pub vol_adj: i32,
    pub cut_adj: i32,
    pub res_adj: i32,
    /// Root note (60 = C5) and fine tune (cents).
    pub root: i32,
    pub fine: i32,
    /// Keyboard tracking: velocity mid (0..128) and key mid (note), and their pan /
    /// cutoff / resonance amounts.
    pub vel_mid: i32,
    pub vel_amt: [i32; 3],
    pub key_mid: i32,
    pub key_amt: [i32; 3],
    /// Max polyphony (0 = unlimited), mono, portamento, slide time knob.
    pub max_poly: i32,
    pub mono: bool,
    pub porta: bool,
    pub porta_time: i32,
    /// Echo delay: feedback 0..128, pan (64 = centre), pitch (cents), echoes, time
    /// (1/48 step), cutoff and resonance 0..256 (128 = unchanged), ping-pong, pan bounce.
    pub echo_feed: i32,
    pub echo_pan: i32,
    pub echo_pitch: i32,
    pub echoes: i32,
    pub echo_time: i32,
    pub echo_cut: i32,
    pub echo_res: i32,
    pub echo_pingpong: bool,
    pub echo_bounce: bool,
    /// Arpeggiator: direction (0 off, 1 up, 2 down, 3 up+down, 4 up+down with repeat,
    /// 5 random), range (octaves), chord (FL 6: -1 auto; 0 none; else 1-based into the
    /// chord list), repeat (FL 6), time knob (1447 = off), gate (0..48), slide.
    pub arp_dir: i32,
    pub arp_range: i32,
    pub arp_chord: i32,
    pub arp_repeat: i32,
    pub arp_time: i32,
    pub arp_gate: i32,
    pub arp_slide: bool,
    /// Gate knob (1447 = off) and whether slide notes skip it.
    pub gate: i32,
    pub gate_skips_slides: bool,
    /// Key range (notes).
    pub key_lo: i32,
    pub key_hi: i32,
    /// Time shift knob (delays every note).
    pub shift: i32,
}

impl ChannelKnobs {
    /// A new TS404 channel in this FL version.
    pub fn new(v: Fl3) -> ChannelKnobs {
        ChannelKnobs {
            ts: TS404_DEFAULT,
            pan: 64,
            vol: 100,
            pitch: 0,
            cut: 120,
            res: 80,
            pan_adj: 0,
            vol_adj: 128,
            cut_adj: 0,
            res_adj: 0,
            root: 60,
            fine: 0,
            vel_mid: 100,
            vel_amt: [0; 3],
            key_mid: 60,
            key_amt: [0; 3],
            max_poly: 0,
            mono: true,
            porta: false,
            porta_time: 500,
            echo_feed: 0,
            echo_pan: 64,
            echo_pitch: 0,
            echoes: 4,
            echo_time: 144,
            echo_cut: 128,
            echo_res: 128,
            echo_pingpong: false,
            echo_bounce: false,
            arp_dir: 0,
            arp_range: if v == Fl3::V35 { 3 } else { 1 },
            arp_chord: if v == Fl3::V35 { 0 } else { -1 },
            arp_repeat: 1,
            arp_time: 1024,
            arp_gate: 48,
            arp_slide: false,
            gate: 800,
            gate_skips_slides: true,
            key_lo: 0,
            key_hi: 256,
            shift: 0,
        }
    }

    /// The channel engine's settings, FL 3.5 (FL's channel "totals").
    pub fn settings3(&self) -> ChannelSettings {
        let k = self;
        let c128 = e(0.007_812_5);
        let c256 = e(0.003_906_25);
        let vol_tab = ei(k.vol.clamp(0, 160)).div(e(128.0)).to_f32();
        let adj = vol_curve(Fl3::V35, ei(k.vol_adj).mul(c128).to_f32());
        let tracking = k.vel_amt.iter().chain(k.key_amt.iter()).any(|&a| a != 0);
        let amt = |a: i32, i: usize, c: Ext| ei(if i == 0 { a.wrapping_shl(6) } else { a }).mul(c).to_f32();
        let key_c = e(f32::from_bits(0x365a_740e));
        ChannelSettings {
            pan: k.pan.wrapping_add(k.pan_adj).wrapping_sub(64),
            vol: e(adj).mul(e(vol_tab)).to_f32(),
            pitch: k.fine.wrapping_add(k.pitch).wrapping_sub(k.root.wrapping_mul(100)),
            modx: ei(k.cut.wrapping_add(k.cut_adj)).mul(c256).to_f32(),
            mody: ei(k.res.wrapping_add(k.res_adj)).mul(c256).to_f32(),
            tracking,
            track_vel_mid: -vol_curve(Fl3::V35, ei(k.vel_mid).mul(c128).to_f32()),
            track_key_mid: ei(k.key_mid.wrapping_sub(60).wrapping_mul(100)).to_f32(),
            track_vel: [0usize, 1, 2].map(|i| amt(k.vel_amt[i], i, c128)),
            track_key: [0usize, 1, 2].map(|i| amt(k.key_amt[i], i, key_c)),
            key_lo: k.key_lo,
            key_hi: k.key_hi,
            max_poly: k.max_poly,
            mono: k.mono,
            porta: k.porta,
            porta_ticks: ticks(k.porta_time),
            gate_ticks: (k.gate < KNOB_OFF).then(|| ticks(k.gate).max(1)),
            gate_skips_slides: k.gate_skips_slides,
            echo_feed: k.echo_feed,
            echo_pan: k.echo_pan.wrapping_add(64),
            echo_pitch: k.echo_pitch,
            echoes: k.echoes,
            echo_time: k.echo_time,
            echo_cut: k.echo_cut,
            echo_res: k.echo_res,
            echo_pingpong: k.echo_pingpong,
            echo_bounce: k.echo_bounce,
            arp_dir: k.arp_dir,
            arp_range: k.arp_range,
            arp_chord: k.arp_chord.max(0),
            arp_time_knob: k.arp_time,
            arp_time_ticks: ticks(k.arp_time.min(KNOB_OFF - 1)),
            arp_gate: k.arp_gate,
            arp_slide: k.arp_slide,
            ticks_per_step: TICKS_PER_STEP,
        }
    }

    /// The channel engine's settings, FL 6.
    pub fn settings6(&self) -> ChannelSettings6 {
        let k = self;
        let c128 = e(0.007_812_5);
        let c256 = e(0.003_906_25);
        let c12800 = e(f32::from_bits(0x38a3_d70a));
        let level = |knob: i32| vol_curve(Fl3::V6, ei(knob.wrapping_mul(100)).mul(c12800).to_f32());
        let tracking = k.vel_amt.iter().chain(k.key_amt.iter()).any(|&a| a != 0);
        let key_c = e(f32::from_bits(0x365a_740e));
        ChannelSettings6 {
            pan: ei(k.pan.wrapping_add(k.pan_adj).wrapping_sub(64).wrapping_mul(100)).mul(e(f32::from_bits(0x3923_d70a))).to_f32(),
            vol: e(level(k.vol_adj)).mul(e(level(k.vol))).to_f32(),
            pitch: ei(k.fine.wrapping_add(k.pitch).wrapping_sub(k.root.wrapping_mul(100))).to_f32(),
            modx: ei(k.cut.wrapping_add(k.cut_adj)).mul(c256).to_f32(),
            mody: ei(k.res.wrapping_add(k.res_adj)).mul(c256).to_f32(),
            tracking,
            track_vel_mid: -vol_curve(Fl3::V6, ei(k.vel_mid).mul(c128).to_f32()),
            track_key_mid: ei(k.key_mid.wrapping_sub(60).wrapping_mul(100)).to_f32(),
            track_vel: k.vel_amt.map(|a| ei(a).mul(c128).to_f32()),
            track_key: k.key_amt.map(|a| ei(a).mul(key_c).to_f32()),
            key_lo: k.key_lo,
            key_hi: k.key_hi,
            max_poly: k.max_poly,
            mono: k.mono,
            porta: k.porta,
            porta_ticks: ticks(k.porta_time),
            gate_ticks: (k.gate < KNOB_OFF).then(|| ticks(k.gate).max(1)),
            gate_skips_slides: k.gate_skips_slides,
            echo_feed: k.echo_feed.wrapping_mul(100),
            echo_pan: k.echo_pan.wrapping_mul(100).wrapping_add(6400),
            echo_pitch: k.echo_pitch,
            echoes: k.echoes,
            echo_time: k.echo_time,
            echo_cut: k.echo_cut,
            echo_res: k.echo_res,
            echo_pingpong: k.echo_pingpong,
            echo_bounce: k.echo_bounce,
            arp_dir: k.arp_dir,
            arp_range: k.arp_range,
            arp_chord: k.arp_chord,
            arp_repeat: k.arp_repeat,
            arp_time_knob: k.arp_time,
            arp_time_ticks: ticks(k.arp_time.min(KNOB_OFF - 1)),
            arp_gate: k.arp_gate,
            arp_slide: k.arp_slide,
            ticks_per_step: TICKS_PER_STEP,
        }
    }

    /// The time shift every note gets, in ticks.
    pub fn shift_ticks(&self) -> i32 {
        ticks(self.shift)
    }
}

/// FL's volume curve: (11^x - 1) / 10. FL 6 returns exactly 1 for x = 1.
pub fn vol_curve(v: Fl3, x: f32) -> f32 {
    if v == Fl3::V6 && x.to_bits() == 1f32.to_bits() {
        return 1.0;
    }
    fpu::exp(e(x).mul(e(f32::from_bits(0x4019_771e)))).sub(e(1.0)).mul(e(0.1)).to_f32()
}

/// A time knob (0..1024 and beyond) as 1/48 steps: round((27^(knob/1024) - 1) · 48/26).
pub fn units48(knob: i32) -> i32 {
    let x = ei(knob).mul(e(1.0 / 1024.0)).to_f32();
    const K: Ext = Ext::from_bits(0x9d89_d89d_89d8_9d8a, 0x3ffa); // 1/26
    e(log_scale3(26.0, x)).mul(e(48.0)).mul(K).round_i64() as i32
}

/// A time knob as ticks.
pub fn ticks(knob: i32) -> i32 {
    units48(knob).wrapping_mul(TICKS_PER_STEP) / 48
}

/// The time knob for a length in 1/48 steps (`units48`'s inverse, used on old files).
pub fn knob_for_units48(units: i32) -> i32 {
    let x = ei(units.wrapping_mul(26)).div(e(48.0)).to_f32();
    let y = fpu::ln(e(x).add(e(1.0))).div(fpu::ln(e(26.0).add(e(1.0)))).to_f32();
    e(y).mul(e(1024.0)).round_i64() as i32
}

// --------------------------------------------------------------------- loaders

/// FL's file version number: "a.b.c" -> a·1000000 + b·1000 + c.
pub fn version_number(s: &str) -> i32 {
    let mut parts = s.trim_end_matches('\0').splitn(3, '.');
    let mut v: i32 = 0;
    for k in 0..3 {
        let n = parts.next().and_then(|p| p.trim().parse::<i32>().ok()).unwrap_or(0);
        v = v.wrapping_add(n);
        if k < 2 {
            v = v.wrapping_mul(1000);
        }
    }
    v
}

/// The channel as FL lays it out in memory, for applying file events the way FL does
/// (raw copies of event data over fields, then per-file-version fixes).
struct Raw {
    v: Fl3,
    chan: [u8; 0x170],
    /// TS404 parameters at +4·i, and the slot after p33 (where a .404's gate lands).
    ts: [i32; 36],
    root: i32,
    fine: i32,
    tracking_events: usize,
}

/// Field offsets that differ between the versions (FL 6 shifted the block event 215 fills).
struct Layout {
    block215: usize,
    echo_flags: usize,
    arp_dir: usize,
    arp_slide: usize,
    gate_skip: usize,
    gate: usize,
    key_lo: usize,
    repeat: Option<usize>,
    block215_max: usize,
}

const L35: Layout = Layout { block215: 0x108, echo_flags: 0x112, arp_dir: 0x130, arp_slide: 0x144, gate_skip: 0x146, gate: 0x148, key_lo: 0x14c, repeat: None, block215_max: 0x50 };
const L6: Layout = Layout { block215: 0x104, echo_flags: 0x10e, arp_dir: 0x12c, arp_slide: 0x140, gate_skip: 0x142, gate: 0x144, key_lo: 0x148, repeat: Some(0x160), block215_max: 0x70 };

impl Raw {
    fn layout(&self) -> &'static Layout {
        if self.v == Fl3::V35 { &L35 } else { &L6 }
    }

    fn get(&self, o: usize) -> i32 {
        i32::from_le_bytes(self.chan[o..o + 4].try_into().unwrap())
    }

    fn set(&mut self, o: usize, x: i32) {
        self.chan[o..o + 4].copy_from_slice(&x.to_le_bytes());
    }

    fn mv(&mut self, o: usize, data: &[u8], max: usize) {
        let n = data.len().min(max).min(self.chan.len() - o);
        self.chan[o..o + n].copy_from_slice(&data[..n]);
    }

    /// FL 6 keeps pan, volume and echo feedback/pan ×100.
    fn scale(&self) -> i32 {
        if self.v == Fl3::V35 { 1 } else { 100 }
    }

    fn from_knobs(v: Fl3, k: &ChannelKnobs) -> Raw {
        let mut r = Raw { v, chan: [0; 0x170], ts: [0; 36], root: k.root, fine: k.fine, tracking_events: 0 };
        r.ts[..34].copy_from_slice(&k.ts);
        let s = r.scale();
        let l = r.layout();
        r.set(0x0c, k.pan * s);
        r.set(0x10, k.vol * s);
        r.set(0x14, k.pitch);
        r.set(0x18, k.cut);
        r.set(0x1c, k.res);
        r.set(0x24, k.pan_adj * s);
        r.set(0x28, k.vol_adj * s);
        r.set(0x30, k.cut_adj);
        r.set(0x34, k.res_adj);
        r.set(0x4c, k.vel_mid);
        r.set(0x6c, k.key_mid);
        for i in 0..3 {
            r.set(0x50 + 4 * i, k.vel_amt[i]);
            r.set(0x70 + 4 * i, k.key_amt[i]);
        }
        r.set(0x8c, k.max_poly);
        r.set(0x90, k.porta_time);
        r.chan[0x94] = k.mono as u8 | (k.porta as u8) << 1;
        r.set(0x98, k.echo_feed * s);
        r.set(0x9c, if v == Fl3::V35 { k.echo_pan + 64 } else { k.echo_pan * 100 + 6400 });
        r.set(0xa0, k.echo_pitch);
        r.set(0xa4, k.echoes);
        r.set(0xa8, k.echo_time);
        r.set(0xac, k.echo_res);
        r.set(0xb0, k.echo_cut);
        r.set(0xb4, k.shift);
        r.chan[l.echo_flags] = (k.echo_bounce as u8) << 1 | (k.echo_pingpong as u8) << 2;
        r.set(l.arp_dir, k.arp_dir);
        r.set(l.arp_dir + 4, k.arp_range);
        r.set(l.arp_dir + 8, k.arp_chord);
        r.set(l.arp_dir + 12, k.arp_time);
        r.set(l.arp_dir + 16, k.arp_gate);
        r.chan[l.arp_slide] = k.arp_slide as u8;
        r.chan[l.gate_skip] = k.gate_skips_slides as u8;
        r.set(l.gate, k.gate);
        r.set(l.key_lo, k.key_lo);
        r.set(l.key_lo + 4, k.key_hi);
        if let Some(o) = l.repeat {
            r.set(o, k.arp_repeat);
        }
        r
    }

    fn to_knobs(&self) -> ChannelKnobs {
        let s = self.scale();
        let l = self.layout();
        let mut ts = [0i32; 34];
        ts.copy_from_slice(&self.ts[..34]);
        ChannelKnobs {
            ts,
            pan: self.get(0x0c) / s,
            vol: self.get(0x10) / s,
            pitch: self.get(0x14),
            cut: self.get(0x18),
            res: self.get(0x1c),
            pan_adj: self.get(0x24) / s,
            vol_adj: self.get(0x28) / s,
            cut_adj: self.get(0x30),
            res_adj: self.get(0x34),
            root: self.root,
            fine: self.fine,
            vel_mid: self.get(0x4c),
            vel_amt: [0usize, 1, 2].map(|i| self.get(0x50 + 4 * i)),
            key_mid: self.get(0x6c),
            key_amt: [0usize, 1, 2].map(|i| self.get(0x70 + 4 * i)),
            max_poly: self.get(0x8c),
            mono: self.chan[0x94] & 1 != 0,
            porta: self.chan[0x94] & 2 != 0,
            porta_time: self.get(0x90),
            echo_feed: self.get(0x98) / s,
            echo_pan: if self.v == Fl3::V35 { self.get(0x9c) - 64 } else { (self.get(0x9c) - 6400) / 100 },
            echo_pitch: self.get(0xa0),
            echoes: self.get(0xa4),
            echo_time: self.get(0xa8),
            echo_cut: self.get(0xb0),
            echo_res: self.get(0xac),
            echo_pingpong: self.chan[l.echo_flags] & 4 != 0,
            echo_bounce: self.chan[l.echo_flags] & 2 != 0,
            arp_dir: self.get(l.arp_dir),
            arp_range: self.get(l.arp_dir + 4),
            arp_chord: self.get(l.arp_dir + 8),
            arp_repeat: l.repeat.map_or(1, |o| self.get(o)),
            arp_time: self.get(l.arp_dir + 12),
            arp_gate: self.get(l.arp_dir + 16),
            arp_slide: self.chan[l.arp_slide] != 0,
            gate: self.get(l.gate),
            gate_skips_slides: self.chan[l.gate_skip] != 0,
            key_lo: self.get(l.key_lo),
            key_hi: self.get(l.key_lo + 4),
            shift: self.get(0xb4),
        }
    }

    /// One .fst/.flp channel event, as FL's loader applies it (file version `fv`).
    fn event(&mut self, ev: u8, data: &[u8], value: u32, fv: i32) {
        let l = self.layout();
        let v6 = self.v == Fl3::V6;
        match ev {
            // echo flags
            16 => self.chan[l.echo_flags] = value as u8,
            // time shift
            89 => {
                let mut x = value as i32;
                if fv < 3_005_002 {
                    x = knob_for_units48(x);
                }
                self.set(0xb4, x);
            }
            // root note, fine tune
            135 => self.root = value as i32,
            142 => self.fine = value as i32,
            // echo resonance / cutoff
            138 => {
                self.set(0xac, (value & 0xffff) as i32);
                self.set(0xb0, (value >> 16) as i32);
            }
            // echo
            209 => {
                self.mv(0x98, data, if v6 { 0x14 } else { usize::MAX });
                if fv < 3_003_001 {
                    let p = self.get(0xa0);
                    self.set(0xa0, p.wrapping_sub(0x78).wrapping_mul(10));
                }
                if v6 {
                    if fv < 5_003_007 {
                        self.set(0x98, self.get(0x98).wrapping_mul(100));
                        self.set(0x9c, self.get(0x9c).wrapping_mul(100));
                    }
                    self.set(0x9c, self.get(0x9c).wrapping_add(0x1900));
                } else {
                    self.set(0x9c, self.get(0x9c).wrapping_add(0x40));
                }
            }
            // TS404 parameters (FL 3.5 copies the whole block, FL 6 at most p1..p33)
            210 => {
                let n = if v6 { data.len().min(0x84) } else { data.len().min(4 * 35) };
                for (i, c) in data[..n].chunks_exact(4).enumerate() {
                    self.ts[i + 1] = i32::from_le_bytes(c.try_into().unwrap());
                }
                if fv < 3_002_091 {
                    self.ts[17] = 0x80 - self.ts[17];
                }
            }
            // channel parameters (arpeggiator, gate, key range, ...)
            215 => {
                self.mv(l.block215, data, l.block215_max);
                if fv < 3_003_004 {
                    for o in 1..4 {
                        self.chan[l.arp_slide + o] = 0;
                    }
                }
                if fv < 3_005_002 {
                    let t = clamp_knob(knob_for_units48(self.get(l.arp_dir + 12)));
                    self.set(l.arp_dir + 12, t);
                    let g = clamp_knob(knob_for_units48(self.get(l.gate)));
                    self.set(l.gate, g);
                    // the TS404's own gate, cutoff and resonance become the channel's
                    self.chan[l.gate_skip] = 1;
                    let g = self.ts[34].wrapping_mul(48);
                    let g = if g < 0 { g + 0xff } else { g } >> 8;
                    self.set(l.gate, knob_for_units48(g));
                    self.set(0x18, self.ts[16].wrapping_mul(2));
                    self.set(0x1c, self.ts[17].wrapping_mul(2));
                }
                if v6 && fv < 5_003_007 {
                    self.set(0x10, self.get(0x10).wrapping_mul(100));
                    self.set(0x0c, self.get(0x0c).wrapping_mul(100));
                }
            }
            // levels: pan, volume, pitch, cutoff, resonance, filter type
            219 => self.mv(0x0c, data, usize::MAX),
            // filter: cutoff, resonance, type
            220 => {
                let n = if fv < 2_005_001 { 8 } else { usize::MAX };
                self.mv(0x18, data, n);
                if fv < 3_002_002 {
                    self.set(0x1c, (self.get(0x1c) << 8) / 0x50);
                }
            }
            // polyphony: max, slide time, flags
            221 => {
                self.mv(0x8c, data, usize::MAX);
                if fv < 3_005_002 {
                    let t = knob_for_units48(self.get(0x90));
                    self.set(0x90, t);
                }
            }
            // keyboard tracking (velocity, then key)
            228 => {
                let o = 0x4c + 0x20 * self.tracking_events;
                if o < 0x8c {
                    self.mv(o, data, 0x20);
                }
                self.tracking_events += 1;
            }
            // level adjustments
            229 => {
                self.mv(0x24, data, usize::MAX);
                if v6 && fv < 5_003_007 {
                    self.set(0x28, self.get(0x28).wrapping_mul(100));
                    self.set(0x24, self.get(0x24).wrapping_mul(100));
                }
            }
            _ => {}
        }
    }
}

fn clamp_knob(x: i32) -> i32 {
    x.clamp(450, KNOB_OFF)
}

/// One FL file event: (id, data for 192+, value for the fixed-size ones).
pub fn fl_events(b: &[u8]) -> Option<Vec<(u8, &[u8], u32)>> {
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
    let mut out = Vec::new();
    while p < end {
        let ev = b[p];
        p += 1;
        match ev {
            0..=63 => {
                out.push((ev, &b[0..0], *b.get(p)? as u32));
                p += 1;
            }
            64..=127 => {
                out.push((ev, &b[0..0], u16::from_le_bytes([*b.get(p)?, *b.get(p + 1)?]) as u32));
                p += 2;
            }
            128..=191 => {
                out.push((ev, &b[0..0], u32le(p)?));
                p += 4;
            }
            _ => {
                let (mut n, mut shift) = (0usize, 0);
                loop {
                    let c = *b.get(p)?;
                    p += 1;
                    n |= ((c & 0x7f) as usize) << shift;
                    shift += 7;
                    if c & 0x80 == 0 || shift > 28 {
                        break;
                    }
                }
                out.push((ev, b.get(p..p + n)?, 0));
                p += n;
            }
        }
    }
    Some(out)
}

/// Apply a .fst channel preset (or the first TS404 channel of an .flp) onto `base`, the
/// way FL `v` loads it. Returns `None` when the file has no TS404 parameters.
pub fn load_fst(v: Fl3, base: &ChannelKnobs, bytes: &[u8]) -> Option<ChannelKnobs> {
    let evs = fl_events(bytes)?;
    let fv = evs
        .iter()
        .find(|(id, _, _)| *id == 199)
        .map(|(_, d, _)| version_number(&String::from_utf8_lossy(d)))
        .unwrap_or(0);
    if !evs.iter().any(|(id, _, _)| *id == 210) {
        return None;
    }
    // a project holds several channels (each starts with event 64): take the first
    // one with TS404 parameters, plus the file-wide events before the first channel
    let starts: Vec<usize> = evs.iter().enumerate().filter(|(_, e)| e.0 == 64).map(|(i, _)| i).collect();
    let first = starts.first().copied().unwrap_or(0);
    let mut seg = (first, evs.len());
    for (k, &s) in starts.iter().enumerate() {
        let end = starts.get(k + 1).copied().unwrap_or(evs.len());
        if evs[s..end].iter().any(|e| e.0 == 210) {
            seg = (s, end);
            break;
        }
    }
    let mut raw = Raw::from_knobs(v, base);
    for &(id, data, value) in evs[..first].iter().chain(evs[seg.0..seg.1].iter()) {
        raw.event(id, data, value, fv);
    }
    Some(raw.to_knobs())
}

/// Apply a .404 preset onto `base`, the way FL 3.5 loads it (FL 6 does the same): the
/// TS404 parameters, the channel's cutoff and resonance from p16/p17, the gate from p34.
pub fn load_404(v: Fl3, base: &ChannelKnobs, bytes: &[u8]) -> Option<ChannelKnobs> {
    if bytes.len() < 0x84 {
        return None;
    }
    let mut raw = Raw::from_knobs(v, base);
    raw.ts = [0; 36];
    raw.ts[..34].copy_from_slice(&TS404_DEFAULT);
    for i in 0..33 {
        raw.ts[i + 1] = i32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap());
    }
    // FL reads two bytes more: the low half of the step gate
    if bytes.len() >= 0x86 {
        raw.ts[34] = u16::from_le_bytes([bytes[0x84], bytes[0x85]]) as i32;
    }
    raw.set(0x18, raw.ts[16].wrapping_mul(2));
    raw.set(0x1c, (0x80 - raw.ts[17]).wrapping_mul(2));
    let g = raw.ts[34].wrapping_mul(0x30);
    let g = if g < 0 { g + 0xff } else { g } >> 8;
    let l = raw.layout();
    raw.set(l.gate, knob_for_units48(g));
    Some(raw.to_knobs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_fl() {
        // FL 6's new TS404 channel: volume 0.5510106, pitch -6000, slide 3 ticks
        let s = ChannelKnobs::new(Fl3::V6).settings6();
        assert_eq!(s.vol.to_bits(), 0.551_010_6f32.to_bits());
        assert_eq!(s.pitch, -6000.0);
        assert_eq!(s.porta_ticks, 3);
        assert_eq!(s.track_vel_mid.to_bits(), (-0.551_010_6f32).to_bits());
        assert_eq!(s.echo_pan, 12800);
        assert_eq!(ticks(800), 11);
    }

    #[test]
    fn version_numbers() {
        assert_eq!(version_number("2.5.4"), 2_005_004);
        assert_eq!(version_number("4.1.0\0"), 4_001_000);
        assert_eq!(version_number("3.6.5"), 3_006_005);
    }

    #[test]
    fn time_knob_roundtrip() {
        for u in [0, 1, 6, 12, 24, 48, 96] {
            let k = knob_for_units48(u);
            assert!((units48(k) - u).abs() <= 1, "{u} -> {k} -> {}", units48(k));
        }
    }
}
