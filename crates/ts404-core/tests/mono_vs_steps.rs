//! MIDI playback through `Synth` (the plugin's code path) must equal step playback of
//! the engine: slide-free patterns in normal mode, slide patterns with TS404 slide timing.

use std::sync::Arc;

use ts404_core::steps::Step;
use ts404_core::synth::{Settings, Synth};
use ts404_core::{Engine, Tables, idx};

pub struct Rng(pub u64);
impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }
}

fn random_params(r: &mut Rng) -> [i32; idx::PARAM_COUNT] {
    let mut p = [0i32; idx::PARAM_COUNT];
    let mut set = |i: usize, v: i32| p[i - 1] = v;
    set(idx::OSC1_COARSE, r.range(-12, 12));
    set(idx::OSC2_COARSE, r.range(-12, 12));
    set(idx::OSC1_FINE, r.range(-128, 128));
    set(idx::OSC2_FINE, r.range(-128, 128));
    set(idx::OSC1_PW, r.range(0, 256));
    set(idx::OSC2_PW, r.range(0, 256));
    set(idx::SYNC, r.range(0, 1));
    set(idx::OSC1_SHAPE, r.range(0, 3));
    set(idx::OSC2_SHAPE, r.range(0, 3));
    set(idx::FILTER_TYPE, r.range(0, 4));
    set(idx::OSC_MIX, r.range(0, 256));
    set(idx::DIST_TYPE, r.range(0, 1));
    set(idx::DIST_AMOUNT, r.range(0, 128));
    set(idx::DIST_THRES, r.range(0, 10));
    set(idx::CUTOFF, r.range(0, 128));
    set(idx::RESO_INV, r.range(0, 128));
    set(idx::ENV_AMT, r.range(0, 256));
    set(idx::LFO_AMT, r.range(0, 256));
    set(idx::LFO_SPEED, r.range(0, 100));
    set(idx::LFO_TARGET, r.range(0, 3));
    set(idx::LFO_SHAPE, r.range(0, 3));
    set(idx::FM, r.range(0, 256));
    set(idx::RING, r.range(0, 256));
    set(idx::ATTACK, r.range(0, 100));
    set(idx::DECAY, r.range(0, 100));
    set(idx::RELEASE, r.range(0, 100));
    set(idx::SUSTAIN_TIME, r.range(0, 100));
    set(idx::SUSTAIN_LEVEL, r.range(0, 100));
    set(idx::STEP_GATE, r.range(0, 256));
    p
}

/// Step playback: a silent lead-in step at the idle pitch (48), then `steps`.
fn step_render(t: &Tables, params: &[i32; idx::PARAM_COUNT], steps: &[Step], sps: i32) -> Vec<f32> {
    let mut all = vec![Step { note: 48, ..Default::default() }];
    all.extend_from_slice(steps);
    let mut fl = Engine::default();
    fl.set_params(params);
    fl.p[idx::ENV_STATE] = 4;
    let mut out = vec![0f32; all.len() * sps as usize];
    for (k, st) in all.iter().enumerate() {
        let next = all.get(k + 1).copied().unwrap_or(Step { note: st.note, ..Default::default() });
        fl.begin_step(st, &next);
        for (i, chunk) in out[k * sps as usize..(k + 1) * sps as usize].chunks_mut(113).enumerate() {
            fl.render(t, chunk, (i * 113) as i32, sps, false, None);
        }
    }
    out
}

/// Play host MIDI (sample times relative to the first real step) through Synth at
/// 44.1 kHz in random block sizes. The step render starts with a one-step lead-in; the
/// synth's output is delayed by its lookahead, so host time 0 maps to output `lead - d`.
fn synth_render(t: &Arc<Tables>, s: &Settings, host: &[(i64, bool, u8)], total: usize, lead: i64, r: &mut Rng) -> Vec<f32> {
    let d = Synth::lookahead_for(s.fl_slides, s.step_len) as i64;
    let mut syn = Synth::new(t.clone(), 44100, 4096, d as u64);
    let shift = lead - d;
    let mut out = vec![0f32; total];
    let mut right = vec![0f32; total];
    let mut pos = 0usize;
    let mut ev = 0;
    while pos < total {
        let n = (r.range(1, 700) as usize).min(total - pos);
        while ev < host.len() && (host[ev].0 + shift) < (pos + n) as i64 {
            let (at, on, note) = host[ev];
            let off = ((at + shift) as usize).saturating_sub(pos) as u32;
            if on { syn.note_on(off, note, s) } else { syn.note_off(off, note) }
            ev += 1;
        }
        syn.process(s, &mut out[pos..pos + n], &mut right[pos..pos + n]);
        pos += n;
    }
    assert!(out.iter().zip(&right).all(|(a, b)| a.to_bits() == b.to_bits()), "centred and dry: L == R");
    out
}

fn check(case: usize, got: &[f32], want: &[f32], sps: i32) {
    if let Some(d) = got.iter().zip(want).position(|(a, b)| a.to_bits() != b.to_bits()) {
        panic!("case {case}: first difference at sample {d} (step {}, pos {}): got {:?} want {:?}",
               d / sps as usize, d % sps as usize, &got[d..(d + 3).min(got.len())], &want[d..(d + 3).min(want.len())]);
    }
}

#[test]
fn midi_playback_equals_fl_steps_without_slides() {
    let t = Arc::new(Tables::generate());
    let mut r = Rng(0x404);
    for case in 0..60 {
        let params = random_params(&mut r);
        let sps = r.range(800, 6000);
        let gate_end = (params[idx::STEP_GATE - 1] * sps) >> 8;
        let hold_keys = case % 2 == 1; // keys held until the next note: GAT must do the release
        let mut steps = vec![];
        let mut note = r.range(24, 70);
        for _ in 0..r.range(3, 12) {
            let gate = steps.is_empty() || r.next() % 4 != 0;
            if gate {
                note = r.range(24, 70);
            }
            steps.push(Step { note, cut: 0, gate, slide: false });
        }
        steps.extend([Step { note, ..Default::default() }; 3]);
        let want = step_render(&t, &params, &steps, sps);

        let mut host = vec![];
        let gated: Vec<usize> = (0..steps.len()).filter(|&k| steps[k].gate).collect();
        for (j, &k) in gated.iter().enumerate() {
            let m = (steps[k].note + 12) as u8;
            let start = k as i64 * sps as i64;
            let end = if hold_keys {
                gated.get(j + 1).map(|&k2| k2 as i64 * sps as i64).unwrap_or(start + 3 * sps as i64)
            } else {
                start + (gate_end + 1).min(sps) as i64
            };
            host.push((start, true, m));
            host.push((end, false, m));
        }
        host.sort_by_key(|&(t, on, _)| (t, on)); // offs before ons at the same time
        let s = Settings { params, hq: false, step_len: sps, gate: params[idx::STEP_GATE - 1], fl_slides: false, ..Default::default() };
        let got = synth_render(&t, &s, &host, want.len(), sps as i64, &mut r);
        check(case, &got, &want, sps);
    }
}

#[test]
fn fl_slide_timing_equals_fl_slide_steps() {
    let t = Arc::new(Tables::generate());
    let mut r = Rng(0x303);
    for case in 0..60 {
        let params = random_params(&mut r);
        let sps = r.range(800, 6000);
        let gate_end = (params[idx::STEP_GATE - 1] * sps) >> 8;
        let hold_keys = case % 2 == 1;
        let n_steps = r.range(3, 14) as usize;
        let mut steps: Vec<Step> = (0..n_steps)
            .map(|_| Step { note: r.range(24, 70), cut: 0, gate: true, slide: r.next() % 3 == 0 })
            .collect();
        steps.last_mut().unwrap().slide = false;
        // a slide into the same pitch is a tie, which MIDI writes as one longer note
        for k in 1..steps.len() {
            if steps[k - 1].slide && steps[k].note == steps[k - 1].note {
                steps[k].note += 1;
            }
        }
        let last = steps.last().unwrap().note;
        steps.extend([Step { note: last, ..Default::default() }; 3]);
        let want = step_render(&t, &params, &steps, sps);

        // each gated step is a note; a slide step's key is held into the next note
        let mut host = vec![];
        for (k, st) in steps.iter().enumerate().filter(|(_, s)| s.gate) {
            let m = (st.note + 12) as u8;
            let start = k as i64 * sps as i64;
            let end = if st.slide {
                start + sps as i64 + 1
            } else if hold_keys {
                start + sps as i64
            } else {
                start + (gate_end + 1).min(sps) as i64
            };
            host.push((start, true, m));
            host.push((end, false, m));
        }
        host.sort_by_key(|&(t, on, _)| (t, on));
        if std::env::var("CASE").ok().and_then(|c| c.parse::<usize>().ok()) == Some(case) {
            eprintln!("sps {sps} gate_end {gate_end} hold {hold_keys} steps {:?}", steps.iter().map(|s| (s.note, s.gate, s.slide)).collect::<Vec<_>>());
            eprintln!("params {:?}\nhost {:?}", params, host);
        }
        let s = Settings { params, hq: false, step_len: sps, gate: params[idx::STEP_GATE - 1], fl_slides: true, ..Default::default() };
        let got = synth_render(&t, &s, &host, want.len(), sps as i64, &mut r);
        check(case, &got, &want, sps);
    }
}
