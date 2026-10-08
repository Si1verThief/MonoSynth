//! Edge cases for the plugin's code path (Synth): hostile MIDI, odd settings, host
//! block sizes and sample rates.

use std::sync::Arc;

use ts404_core::synth::{Settings, Synth};
use ts404_core::{Tables, idx};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn range(&mut self, lo: i64, hi: i64) -> i64 {
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }
    fn chance(&mut self, p: f64) -> bool {
        (self.next() % 10_000) as f64 / 10_000.0 < p
    }
}

const RATES: [u32; 6] = [44100, 48000, 22050, 88200, 96000, 192000];

/// Parameter extremes as well as random values inside the knob ranges.
fn params(r: &mut Rng) -> [i32; idx::PARAM_COUNT] {
    let ranges: [(usize, i64, i64); 30] = [
        (idx::OSC1_COARSE, -12, 12), (idx::OSC2_COARSE, -12, 12), (idx::OSC1_FINE, -128, 128), (idx::OSC2_FINE, -128, 128),
        (idx::OSC1_PW, 0, 256), (idx::OSC2_PW, 0, 256), (idx::SYNC, 0, 1), (idx::OSC1_SHAPE, 0, 4), (idx::OSC2_SHAPE, 0, 4),
        (idx::FILTER_TYPE, 0, 4), (idx::OSC_MIX, 0, 256), (idx::DIST_TYPE, 0, 1), (idx::DIST_AMOUNT, 0, 128),
        (idx::DIST_THRES, 0, 10), (idx::CUTOFF, 0, 128), (idx::RESO_INV, 0, 128), (idx::ENV_AMT, 0, 256),
        (idx::LFO_AMT, 0, 256), (idx::LFO_SPEED, 0, 100), (idx::LFO_TARGET, 0, 3), (idx::LFO_SHAPE, 0, 3),
        (idx::FM, 0, 256), (idx::RING, 0, 256), (idx::ATTACK, 0, 100), (idx::DECAY, 0, 100), (idx::RELEASE, 0, 99),
        (idx::SUSTAIN_TIME, 0, 100), (idx::SUSTAIN_LEVEL, 0, 100), (idx::STEP_GATE, 0, 256), (idx::DELAY_AMT, 0, 128),
    ];
    let mut p = [0i32; idx::PARAM_COUNT];
    for (i, lo, hi) in ranges {
        let v = match r.range(0, 5) {
            0 => lo,
            1 => hi,
            _ => r.range(lo, hi),
        };
        p[i - 1] = v as i32;
    }
    p
}

fn settings(r: &mut Rng) -> Settings {
    let p = params(r);
    Settings {
        params: p,
        hq: r.chance(0.3),
        step_len: match r.range(0, 4) {
            0 => 1,
            1 => r.range(2, 64) as i32,
            2 => 200_000,
            _ => r.range(1000, 12000) as i32,
        },
        gate: p[idx::STEP_GATE - 1],
        fl_slides: r.chance(0.5),
        sixteenth: r.range(1000, 12000) as i32,
        delay_ticks: [0, 1, 7, 48, 144, 768][r.range(0, 5) as usize],
        delay_feedback: r.range(0, 127) as i32,
        delay_pan: r.range(0, 128) as i32,
        delay_volume: r.range(0, 160) as i32,
        pan: if r.chance(0.5) { 64 } else { r.range(0, 128) as i32 },
    }
}

/// Random MIDI: chords, same-note repeats, zero-length notes, offs without ons,
/// bursts at one timestamp, extreme note numbers, more than 32 keys at once.
fn chaos(r: &mut Rng, len: i64) -> Vec<(i64, bool, u8)> {
    let mut ev = vec![];
    let mut t = 0;
    while t < len {
        t += match r.range(0, 9) {
            0 => 0,
            1 => 1,
            2..=5 => r.range(2, 800),
            _ => r.range(800, 20000),
        };
        let note = if r.chance(0.1) { [0u8, 1, 127, 126][r.range(0, 3) as usize] } else { r.range(24, 100) as u8 };
        match r.range(0, 9) {
            0 => {
                for k in 0..r.range(2, 40) {
                    ev.push((t, true, note.wrapping_add(k as u8 * 3) % 128));
                }
            }
            1 => ev.push((t, false, note)),
            2 => {
                ev.push((t, true, note));
                ev.push((t, false, note));
            }
            _ => {
                ev.push((t, true, note));
                ev.push((t + r.range(0, 30000), false, note));
            }
        }
    }
    ev.sort_by_key(|e| e.0);
    ev
}

/// Play `ev` through a Synth; `blocks` picks the host block sizes.
fn play(t: &Arc<Tables>, sr: u32, s: &Settings, ev: &[(i64, bool, u8)], total: usize, blocks: &mut Rng, all_off_at: Option<usize>) -> Vec<f32> {
    let mut syn = Synth::new(t.clone(), sr, 4096, Synth::lookahead_for(s.fl_slides, s.step_len));
    let mut out = vec![0f32; 2 * total];
    let (mut l, mut r) = (vec![0f32; 4096], vec![0f32; 4096]);
    let (mut pos, mut i) = (0usize, 0usize);
    let mut sounding = [false; 128];
    while pos < total {
        let n = (blocks.range(1, 4096) as usize).min(total - pos);
        if all_off_at.is_some_and(|a| a >= pos && a < pos + n) {
            for k in 0..128u8 {
                if sounding[k as usize] {
                    syn.note_off((all_off_at.unwrap() - pos) as u32, k);
                    sounding[k as usize] = false;
                }
            }
            i = ev.len();
        }
        while i < ev.len() && (ev[i].0 as usize) < pos + n {
            let off = (ev[i].0 as usize - pos) as u32;
            if ev[i].1 { syn.note_on(off, ev[i].2, s) } else { syn.note_off(off, ev[i].2) }
            sounding[ev[i].2 as usize] = ev[i].1;
            i += 1;
        }
        syn.process(s, &mut l[..n], &mut r[..n]);
        for k in 0..n {
            out[2 * (pos + k)] = l[k];
            out[2 * (pos + k) + 1] = r[k];
        }
        pos += n;
    }
    out
}

#[test]
fn hostile_midi_stays_finite_bounded_and_ends_silent() {
    let t = Arc::new(Tables::generate());
    let mut r = Rng(0xbad5eed);
    for case in 0..80 {
        let sr = RATES[case % RATES.len()];
        let mut s = settings(&mut r);
        s.delay_feedback = s.delay_feedback.min(64); // echoes must die out within the tail
        s.delay_ticks = s.delay_ticks.min(48);
        let len = r.range(20_000, 120_000);
        let ev = chaos(&mut r, len);
        // let the longest release (99: ~5 s to the floor) and any lookahead finish
        let delay_tail = Synth::delay_len(&s) * 30 * sr as usize / 44100;
        let tail = (6.5 * sr as f64) as usize + (s.step_len as usize * sr as usize / 44100) * 2 + delay_tail;
        let total = len as usize + 30_000 + tail;
        let out = play(&t, sr, &s, &ev, total, &mut Rng(case as u64 + 1), Some(len as usize + 30_000));
        assert!(out.iter().all(|x| x.is_finite()), "case {case}: non-finite output");
        let peak = out.iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(peak < 16.0, "case {case}: peak {peak}");
        let end = &out[2 * (total - 2000)..];
        assert!(end.iter().all(|&x| x.abs() < 1e-6), "case {case} (sr {sr}, {s:?}): not silent after all notes off");
    }
}

#[test]
fn output_does_not_depend_on_host_block_size() {
    let t = Arc::new(Tables::generate());
    let mut r = Rng(0xb10c);
    for case in 0..30 {
        let sr = RATES[case % RATES.len()];
        let s = settings(&mut r);
        let ev = chaos(&mut r, 60_000);
        let a = play(&t, sr, &s, &ev, 80_000, &mut Rng(1), None);
        let b = play(&t, sr, &s, &ev, 80_000, &mut Rng(2), None);
        let d = a.iter().zip(&b).position(|(x, y)| x.to_bits() != y.to_bits());
        assert!(d.is_none(), "case {case} (sr {sr}): block-size dependent at sample {d:?}");
    }
}

#[test]
fn fl_slide_timing_without_legato_is_a_pure_delay() {
    let t = Arc::new(Tables::generate());
    let mut r = Rng(0xde1a);
    for case in 0..30 {
        let mut s = settings(&mut r);
        s.step_len = r.range(100, 9000) as i32;
        // separated notes only
        let mut ev = vec![];
        let mut tm = 0;
        for _ in 0..r.range(2, 12) {
            tm += r.range(1, 5000);
            let note = r.range(24, 100) as u8;
            let dur = r.range(0, 8000);
            ev.push((tm, true, note));
            ev.push((tm + dur, false, note));
            tm += dur;
        }
        let d = s.step_len as i64;
        let total = tm as usize + 50_000 + d as usize;
        s.pan = 64;
        s.fl_slides = false;
        let shifted: Vec<_> = ev.iter().map(|&(t, on, n)| (t + d, on, n)).collect();
        let plain = play(&t, 44100, &s, &shifted, total, &mut Rng(3), None);
        s.fl_slides = true;
        let delayed = play(&t, 44100, &s, &ev, total, &mut Rng(4), None);
        let diff = plain.iter().zip(&delayed).position(|(a, b)| a.to_bits() != b.to_bits());
        assert!(diff.is_none(), "case {case}: differs at {diff:?}");
    }
}

#[test]
fn every_note_sounds_in_both_modes() {
    let t = Arc::new(Tables::generate());
    let mut r = Rng(0x50d);
    for case in 0..40 {
        let mut p = params(&mut r);
        // a patch that is audible: fast attack, open filter, some sustain, no gate cut
        p[idx::ATTACK - 1] = 0;
        p[idx::FILTER_TYPE - 1] = 0;
        p[idx::CUTOFF - 1] = 100;
        p[idx::SUSTAIN_LEVEL - 1] = 60;
        p[idx::SUSTAIN_TIME - 1] = 100;
        p[idx::DIST_THRES - 1] = 0;
        p[idx::OSC_MIX - 1] = 0;
        p[idx::RING - 1] = 0;
        let sr = RATES[case % RATES.len()];
        for fl in [false, true] {
            let s = Settings { params: p, hq: false, step_len: 5000, gate: 256, fl_slides: fl, ..Default::default() };
            let d = (Synth::lookahead_for(fl, 5000) as usize * sr as usize).div_ceil(44100);
            let mut ev = vec![];
            for k in 0..6 {
                let t0 = 10_000 + k * 20_000;
                ev.push((t0 as i64, true, 36 + 5 * k as u8));
                ev.push((t0 as i64 + 8000, false, 36 + 5 * k as u8));
            }
            let out = play(&t, sr, &s, &ev, 140_000 + d, &mut Rng(case as u64), None);
            for k in 0..6 {
                let t0 = 10_000 + k * 20_000 + d + 500;
                let rms = (out[2 * t0..2 * (t0 + 4000)].iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / 8000.0).sqrt();
                assert!(rms > 1e-3, "case {case} sr {sr} fl {fl}: note {k} silent (rms {rms})");
            }
        }
    }
}

#[test]
fn latency_is_stable_across_restarts() {
    // The plugin rebuilds the Synth on every (re)activation from the same settings;
    // the latency it reports must not change, or hosts restart it forever.
    let t = Arc::new(Tables::generate());
    let mut r = Rng(0x1a7);
    for _ in 0..200 {
        let s = settings(&mut r);
        let sr = RATES[r.range(0, 5) as usize];
        let a = Synth::new(t.clone(), sr, 512, Synth::lookahead_for(s.fl_slides, s.step_len));
        let b = Synth::new(t.clone(), sr, 512, Synth::lookahead_for(s.fl_slides, s.step_len));
        assert_eq!(a.latency(), b.latency());
        assert!(!a.needs_new_lookahead(&s), "{s:?}");
    }
}

#[test]
fn synth_delay_equals_fl_block_delay_on_the_dry_voice() {
    use ts404_core::delay::{Delay, DelaySettings, LevelTables};
    let t = Arc::new(Tables::generate());
    let lv = LevelTables::generate();
    let mut r = Rng(0xde1);
    for case in 0..12 {
        let mut s = settings(&mut r);
        s.fl_slides = false;
        s.pan = 64;
        s.params[idx::DELAY_AMT - 1] = r.range(1, 128) as i32;
        s.delay_ticks = [12, 48, 100, 144][case % 4];
        let ev = chaos(&mut r, 60_000);
        let total = 120_000;
        let wet = play(&t, 44100, &s, &ev, total, &mut Rng(9), None);
        let mut dry_s = s;
        dry_s.params[idx::DELAY_AMT - 1] = 0;
        let dry = play(&t, 44100, &dry_s, &ev, total, &mut Rng(9), None);
        let voice: Vec<f32> = dry.iter().step_by(2).copied().collect();
        // dry first, then the delay line adds into it, block by block
        let chan_vol = ts404_core::x87::e(1.0).div(ts404_core::x87::e(lv.pan[64])).to_f32();
        let ds = DelaySettings {
            length: Synth::delay_len(&s),
            feedback: s.delay_feedback,
            pan: s.delay_pan,
            volume: s.delay_volume,
            amount: s.params[idx::DELAY_AMT - 1],
            chan_vol,
        };
        let n = Delay::chunk_len(ds.length);
        let mut d = Delay::new();
        let mut want = dry.clone();
        let mut k = 0;
        while k < total {
            let m = n.min(total - k);
            d.block(&voice[k..k + m], &ds, &lv, &mut want[2 * k..2 * (k + m)]);
            k += m;
        }
        let diff = wet.iter().zip(&want).position(|(a, b)| a.to_bits() != b.to_bits());
        assert!(diff.is_none(), "case {case}: first difference at {diff:?}");
    }
}
