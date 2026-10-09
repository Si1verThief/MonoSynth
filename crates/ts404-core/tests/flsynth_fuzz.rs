//! The FL 3.5 / FL 6 synth under hostile MIDI, odd block sizes, tempo and settings
//! changes (piano-roll and live-key notes, switching between them): no panics, finite
//! output, and silence once every note is off.
use std::sync::Arc;
use ts404_core::flchan::ChannelKnobs;
use ts404_core::flsynth::{FlSettings, FlSynth};
use ts404_core::tables::Tables;
use ts404_core::tables3::Fl3;

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u32) as i32
    }
}

fn tweak(k: &mut ChannelKnobs, r: &mut Rng, echo: bool) {
    match r.range(0, 9) {
        0 => k.mono = !k.mono,
        1 => k.porta = !k.porta,
        2 => k.porta_time = r.range(0, 1446),
        3 => k.gate = r.range(0, 1447),
        4 => k.arp_dir = r.range(0, 5),
        5 => k.arp_chord = r.range(-1, 66),
        6 => k.arp_time = r.range(0, 1447),
        7 if echo => {
            k.echo_feed = r.range(0, 128);
            k.echoes = r.range(1, 20);
            k.echo_time = r.range(1, 96);
            k.echo_pingpong = r.next() % 2 == 0;
        }
        8 => k.max_poly = r.range(0, 4),
        _ => k.arp_range = r.range(0, 5),
    }
}

#[test]
fn flsynth_survives_and_goes_quiet() {
    let t = Arc::new(Tables::generate());
    let mut r = Rng(0x2545_f491_4f6c_dd1d);
    for &(rate, v) in &[(44100, Fl3::V35), (48000, Fl3::V6), (96000, Fl3::V35), (44100, Fl3::V6)] {
        let mut s = FlSynth::new(t.clone(), rate, 1024, v);
        let mut k = ChannelKnobs::new(v);
        let mut tempo = 120.0;
        let mut live = false;
        let (mut l, mut rr) = (vec![0f32; 1024], vec![0f32; 1024]);
        // play: ~20 s of random notes, blocks, tempo and settings
        let mut played = 0usize;
        while played < rate as usize * 20 {
            let n = [1, 7, 64, 333, 512, 1024][r.range(0, 5) as usize];
            if r.next() % 8 == 0 {
                tweak(&mut k, &mut r, true);
            }
            if r.next() % 50 == 0 {
                tempo = r.range(40, 300) as f64;
            }
            if r.next() % 40 == 0 {
                live = !live;
            }
            s.set_settings(&FlSettings { version: v, knobs: k.clone(), tempo, song_pos: None, hq: r.next() % 2 == 0, aa: false, live_keys: live });
            for _ in 0..r.range(0, 3) {
                let off = r.range(0, n as i32 - 1) as u32;
                let note = r.range(0, 127) as u8;
                match r.range(0, 3) {
                    0 | 1 => s.note_on(off, note, r.range(0, 127) as f32 / 127.0),
                    _ => s.note_off(off, note),
                }
            }
            s.process(&mut l[..n], &mut rr[..n]);
            assert!(l[..n].iter().chain(&rr[..n]).all(|x| x.is_finite() && x.abs() < 64.0), "{v:?} @{rate}: bad output");
            played += n;
        }
        // everything off, echo off: the synth releases and goes quiet
        k.echo_feed = 0;
        s.set_settings(&FlSettings { version: v, knobs: k.clone(), tempo, song_pos: None, hq: false, aa: false, live_keys: live });
        s.all_notes_off(0);
        let mut tail = 0f32;
        for b in 0..(rate as usize * 30 / 512) {
            s.process(&mut l[..512], &mut rr[..512]);
            if b * 512 > rate as usize * 25 {
                tail = l[..512].iter().chain(&rr[..512]).fold(tail, |m, x| m.max(x.abs()));
            }
        }
        assert!(tail < 1e-3, "{v:?} @{rate}: still sounding after all notes off ({tail})");
    }
}
