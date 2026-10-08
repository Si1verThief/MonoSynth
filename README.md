# MonoSynth

A monophonic bass and lead synth plugin (VST3 and CLAP, for Windows, macOS and Linux) that sounds exactly like the **TS404**, the classic acid bass synth from early FruityLoops.

![MonoSynth](docs/screenshot.png)

It plays one note at a time. Overlapping notes glide into each other, and a resonant filter is swept by the envelope. That's the squelchy 303-style sound the TS404 was made for, but it does fat basses, sync leads, bells and wobbles too.

## Installing

Download the zip for your system from [Latest release](../../releases/latest) and copy the plugin into your plugin folder:

| System | VST3 | CLAP |
|---|---|---|
| Windows | `C:\Program Files\Common Files\VST3` | `C:\Program Files\Common Files\CLAP` |
| macOS | `~/Library/Audio/Plug-Ins/VST3` | `~/Library/Audio/Plug-Ins/CLAP` |
| Linux | `~/.vst3` | `~/.clap` |

Then rescan plugins in your DAW and load **MonoSynth** as an instrument.

## The sections

**OSC 1 / OSC 2.** Two oscillators.
- **Shapes:** saw, pulse, sine, square, or **?**, a sample of your own (see *Shapes*).
- **CRS** sets the pitch in semitones (±12) and **FINE** detunes.
- **PW** warps the waveform: it squeezes one half of the cycle and stretches the other.
- **FM** (on osc 2) lets osc 1 modulate osc 2's pitch for metallic and gritty tones.

**OSC 1+2.**
- **MIX** blends the two oscillators.
- **RM** fades in ring modulation, the two oscillators multiplied, for bell-like and clangy sounds.
- **SYNC** restarts osc 1 every cycle of osc 2: the classic tearing sync lead.

**ENVELOPE.** One envelope shapes both the volume and the filter; the display shows its real times.
- **ATT**: attack
- **DEC**: decay
- **SUS**: how long the sustain holds (100 = forever)
- **SL**: sustain level
- **REL**: release
- **GAT**: step gate, which ends a note after a fraction of one step. At 100% notes last as long as you hold them; lower it for short, plucky notes.

**FILTER.**
- **Types:** low-pass (12 or 24 dB/oct), high-pass, band-pass, or off.
- **CUT** sets the cutoff, **RES** the resonance.
- **ENV** sets how far the envelope opens the filter. Most of the acid sound is high RES plus a good amount of ENV.
- The display shows the cutoff frequency and how far the envelope pushes it.

**LFO.** A slow oscillator (sine, square, triangle or saw) that wobbles one target:
- **OSC** (pitch, for vibrato),
- **RES**,
- **CUT** (wah and wobble),
- **PW**.

**AMT** sets the depth and **SPD** the rate.

**DIST.** Two distortion characters, **A** (soft) and **B** (hard). **THR** sets the threshold (0 = off) and **AMT** the dry/wet mix. The display shows the curve.

**DELAY.** **AMT** sends the sound to the delay line.

**SLIDE.**
- **Slide length:** how long a glide takes. The default is one 1/16 note at your project tempo; 1/32, 1/8, 1/4 or a time in ms are also available.
- **TS404 slide timing:** see below.
- **HQ distortion:** a smoother, per-sample distortion.

**DELAY LINE.**
- **FEED** sets the number of repeats (feedback).
- **PAN** and **VOL** place the echoes.
- **TIME** is the delay time in steps of a 1/16 note.

Changing the time clears the echoes, as on the original.

**OUTPUT.** **VOL** and **PAN** for the whole synth.

## Playing it

- **Slides.** Hold a note and press the next one before letting go, or overlap notes in the piano roll. The pitch glides and the envelope doesn't restart. Separate notes restart the envelope each time.
- **TS404 slide timing.** On the original, a slide happens *before* the next note starts. A live keyboard can't know which note is coming, so by default the glide starts when the next note arrives. With TS404 slide timing on, MonoSynth plays one step late, which lets it start each glide early, exactly like the TS404. Your DAW compensates for the delay automatically, so use it for programmed parts; leave it off for playing live.
- **Presets.**
  - The menu has built-in patches.
  - **Load** opens TS404 presets (`.404`). It also opens **FL Studio / FruityLoops projects (`.flp`)** and takes the TS404 settings and the song's delay line. Loading the same project again steps to its next TS404 channel.
  - **Save .404** writes a preset the original can load.
- **Shapes.** **Shape…** loads any WAV as the **?** oscillator shape, and right-click clears it. Without a shape, **?** plays a saw. The shape is saved with your project.

## Compared with the TS404

**Same as the TS404**
- **The sound.** Oscillators, filter, envelope, LFO, distortion and delay behave and sound identical, down to the last sample at 44.1 kHz.
- **Every control on the TS404 panel**, in the same places, plus its song-level delay line settings.
- **Sample shapes (?)**, **HQ distortion**, and its presets and projects.

**Different, and why**
- **No step sequencer.** MonoSynth is played from MIDI like any other plugin, so it works in any DAW. That also means there are no per-step lanes for cutoff or fine pitch; automate the knobs instead.
- **Slide timing.** Slides start when the next note arrives unless *TS404 slide timing* is on (see above). Live input can't look ahead the way the original's step grid did.
- **Filter button labels.** The original panel had BP and HP labelled the wrong way round. The buttons are in the same places but now say what they do.
- **Sample rate.** The engine runs at 44.1 kHz like the original and is resampled to other project rates. At 44.1 kHz the output is untouched.
- **Fixed song options.** The original's song-wide "logarithmic levels" and "circular panning" options are fixed at their defaults (off and on), since a plugin has no song settings.
- **Extras.** Output volume and pan, live displays for every section, a choice of slide length, and a step gate of 100% that follows your note lengths.

## Building from source

Needs Rust (stable). On Linux also the ALSA, JACK, X11/xcb and OpenGL development headers.

```sh
cargo xtask bundle monosynth --release   # -> target/bundled/MonoSynth.vst3 and MonoSynth.clap
cargo test --release
```

## Licence

GPL-3.0-or-later. See `LICENSE`.
