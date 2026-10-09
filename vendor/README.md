# Vendored crates

- `baseview/`: baseview 0.3.7 (MIT OR Apache-2.0, https://github.com/RustAudio/baseview) with three
  fixes to its Linux (X11) code, marked "MonoSynth patch":
  - a plugin window inside an already visible host window now starts drawing (it stayed blank);
  - closing the editor waits for the window's thread to finish;
  - moving the editor into another host window waits until the move is done.

  Used through `[patch.crates-io]` in the workspace `Cargo.toml`. baseview 0.4 reworked this
  code; drop this copy once nice-plug-egui moves to it.
