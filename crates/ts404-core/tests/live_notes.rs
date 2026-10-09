//! Live (MIDI) notes in the FL 3.5 / FL 6 channel engines: a note that starts without an
//! end and gets one later (`end_note`) must sound exactly like the same note queued with
//! that end, as long as the end is known before it arrives.
use ts404_core::channel3::{Channel3, Event};
use ts404_core::channel6::{Channel6, Event6};
use ts404_core::engine3::Engine3;
use ts404_core::flchan::ChannelKnobs;
use ts404_core::tables::Tables;
use ts404_core::tables3::{Fl3, Tables3};

const OPEN: i32 = 0x4000_0000;

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
    fn chance(&mut self, p: u32) -> bool {
        self.next() % 100 < p
    }
}

fn knobs(v: Fl3, r: &mut Rng) -> ChannelKnobs {
    let mut k = ChannelKnobs::new(v);
    k.mono = r.chance(40);
    k.porta = r.chance(30);
    k.porta_time = r.range(0, 1000);
    k.max_poly = [0, 0, 1, 2][r.range(0, 3) as usize];
    k.gate = if r.chance(50) { 1447 } else { r.range(450, 1300) };
    k.gate_skips_slides = r.chance(50);
    if r.chance(40) {
        k.echo_feed = r.range(1, 128);
        k.echoes = r.range(1, 6);
        k.echo_time = [48, 72, 144][r.range(0, 2) as usize];
        k.echo_pan = r.range(0, 128);
        k.echo_pitch = r.range(-300, 300);
        k.echo_pingpong = r.chance(30);
    }
    if r.chance(60) {
        k.arp_dir = r.range(1, 5);
        k.arp_range = r.range(0, 3);
        k.arp_chord = if v == Fl3::V6 { r.range(-1, 30) } else { r.range(0, 30) };
        k.arp_time = if r.chance(20) { 1447 } else { r.range(300, 1200) };
        k.arp_gate = r.range(6, 48);
        // FL 3.5 decides an arpeggio slide's last step from the note's end
        k.arp_slide = v == Fl3::V6 && r.chance(30);
    }
    k
}

fn notes(r: &mut Rng) -> Vec<(i32, i32, i32)> {
    let mut t = 2;
    (0..r.range(1, 8))
        .map(|_| {
            t += [0, 1, 3, 6, 12][r.range(0, 4) as usize];
            (t, t + r.range(0, 40), r.range(40, 80) * 100)
        })
        .collect()
}

#[test]
fn live_notes_end_like_known_lengths() {
    let t = Tables::generate();
    let mut r = Rng(0x9e37_79b9_7f4a_7c15);
    for v in [Fl3::V35, Fl3::V6] {
        let t3 = Tables3::generate(v);
        for case in 0..60 {
            let k = knobs(v, &mut r);
            let ns = notes(&mut r);
            let ticks = 140;
            let render = |live: bool| -> Vec<f32> {
                let mut eng = Engine3::new(v);
                eng.p[1..34].copy_from_slice(&k.ts[1..34]);
                let mut out = Vec::new();
                match v {
                    Fl3::V35 => {
                        let mut ch = Channel3::new(eng);
                        ch.s = k.settings3();
                        for (n, &(a, e, p)) in ns.iter().enumerate() {
                            let mut ev = Event::note(a, if live { OPEN } else { e }, 64 * (n as i32 + 1), p, 1.0, 0);
                            ev.arp = if k.arp_dir != 0 { 0 } else { -1 };
                            ch.queue(ev);
                        }
                        for tick in 1..=ticks {
                            if live {
                                for (n, &(a, e, _)) in ns.iter().enumerate() {
                                    if e == tick {
                                        ch.end_note(64 * (n as i32 + 1), a, e);
                                    }
                                }
                            }
                            let (mut l, mut rr) = (vec![0f32; 64], vec![0f32; 64]);
                            ch.block(&t, &t3, true, &mut l, &mut rr, None);
                            out.extend(l.iter().chain(rr.iter()));
                        }
                    }
                    Fl3::V6 => {
                        let mut ch = Channel6::new(eng);
                        ch.s = k.settings6();
                        for (n, &(a, e, p)) in ns.iter().enumerate() {
                            ch.queue(Event6::note(a, if live { OPEN } else { e }, 64 * (n as i32 + 1), p as f32, 1.0, 0));
                        }
                        for tick in 1..=ticks {
                            if live {
                                for (n, &(a, e, _)) in ns.iter().enumerate() {
                                    if e == tick {
                                        ch.end_note(64 * (n as i32 + 1), a, e);
                                    }
                                }
                            }
                            let (mut l, mut rr) = (vec![0f32; 64], vec![0f32; 64]);
                            ch.block(&t, &t3, true, &mut l, &mut rr, None);
                            out.extend(l.iter().chain(rr.iter()));
                        }
                    }
                }
                out
            };
            let (a, b) = (render(false), render(true));
            let diff = a.iter().zip(&b).position(|(x, y)| x.to_bits() != y.to_bits());
            assert!(diff.is_none(), "{v:?} case {case}: differs at {} ({k:?}, notes {ns:?})", diff.unwrap());
        }
    }
}
