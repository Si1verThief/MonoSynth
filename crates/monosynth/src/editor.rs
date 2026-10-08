//! Editor laid out like the TS404 panel (a 245x354 grid scaled by `S`), with live
//! previews of the oscillators, filter, LFO and distortion.

use std::f32::consts::PI;
use std::sync::Arc;

use nih_plug::prelude::*;
use nih_plug_egui::create_egui_editor;
use nih_plug_egui::egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2,
};

use crate::params::{DistType, FilterType, LfoShape, LfoTarget, OscShape, StepLen, SynthParams};
use crate::presets::{self, BUILTIN, Preset404};
use ts404_core::idx;
use ts404_core::tables::{Tables, Wave};

const S: f32 = 2.0;
const PANEL_W: f32 = 245.0;
const PANEL_H: f32 = 354.0;
const HEADER: f32 = 40.0;
const FOOTER: f32 = 118.0;

const BG: Color32 = Color32::from_rgb(0x13, 0x14, 0x18);
const PANEL: Color32 = Color32::from_rgb(0x1e, 0x20, 0x26);
const PANEL_EDGE: Color32 = Color32::from_rgb(0x2c, 0x2f, 0x37);
const SCREEN: Color32 = Color32::from_rgb(0x10, 0x13, 0x14);
const TRACK: Color32 = Color32::from_rgb(0x36, 0x39, 0x42);
const TEXT: Color32 = Color32::from_rgb(0xd8, 0xdb, 0xe2);
const DIM: Color32 = Color32::from_rgb(0x84, 0x89, 0x95);
const ACCENT: Color32 = Color32::from_rgb(0xd4, 0xf5, 0x3c);
const ACCENT_DIM: Color32 = Color32::from_rgb(0x5f, 0x6e, 0x1e);

struct UiState {
    preset: usize,
    status: String,
    flp_path: Option<std::path::PathBuf>,
    flp_next: usize,
}

/// Maps panel-grid coordinates to the screen.
#[derive(Clone, Copy)]
struct Panel {
    o: Pos2,
}

impl Panel {
    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(self.o + vec2(x * S, y * S), vec2(w * S, h * S))
    }
}

pub fn create(params: Arc<SynthParams>) -> Option<Box<dyn Editor>> {
    let state = params.editor_state.clone();
    create_egui_editor(
        state,
        UiState { preset: 0, status: String::new(), flp_path: None, flp_next: 0 },
        |ctx, _| {
            let mut style = (*ctx.style()).clone();
            style.visuals = egui::Visuals::dark();
            style.visuals.panel_fill = BG;
            style.visuals.override_text_color = Some(TEXT);
            style.visuals.selection.bg_fill = ACCENT_DIM;
            style.visuals.widgets.inactive.weak_bg_fill = PANEL_EDGE;
            style.visuals.widgets.hovered.weak_bg_fill = TRACK;
            ctx.set_style(style);
        },
        move |ctx, setter, st| {
            egui::CentralPanel::default().frame(egui::Frame::new().fill(BG).inner_margin(0)).show(ctx, |ui| {
                let top = ui.max_rect().min;
                header(ui, Rect::from_min_size(top, vec2(PANEL_W * S, HEADER)), &params, setter, st);
                let p = Panel { o: top + vec2(0.0, HEADER) };
                panel(ui, p, &params, setter);
                footer(ui, Rect::from_min_size(top + vec2(0.0, HEADER + PANEL_H * S), vec2(PANEL_W * S, FOOTER)), &params, setter);
            });
        },
    )
}

// ------------------------------------------------------------------ the TS404 panel

fn panel(ui: &mut Ui, p: Panel, params: &SynthParams, setter: &ParamSetter) {
    let t = crate::tables();
    // section boxes
    for (title, x, y, w, h) in [
        ("OSC 1", 0.0, 0.0, 190.0, 56.0),
        ("OSC 2", 0.0, 57.0, 190.0, 56.0),
        ("OSC 1+2", 191.0, 0.0, 54.0, 113.0),
        ("ENVELOPE", 0.0, 114.0, 245.0, 67.0),
        ("FILTER", 0.0, 182.0, 245.0, 56.0),
        ("LFO", 0.0, 239.0, 245.0, 56.0),
        ("DIST", 0.0, 296.0, 190.0, 58.0),
        ("DELAY", 191.0, 296.0, 54.0, 58.0),
    ] {
        let r = p.rect(x + 1.0, y + 1.0, w - 2.0, h - 2.0);
        ui.painter().rect_filled(r, CornerRadius::same(6), PANEL);
        ui.painter().rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0_f32, PANEL_EDGE), StrokeKind::Inside);
        ui.painter().text(r.min + vec2(7.0, 5.0), Align2::LEFT_TOP, title, FontId::proportional(14.0), DIM);
    }

    let shapes = [Cell::Icon(Icon::Saw), Cell::Icon(Icon::Pulse), Cell::Icon(Icon::Sine), Cell::Icon(Icon::Square), Cell::Text("?")];
    // OSC 1
    led_select(ui, p.rect(47.0, 2.0, 140.0, 15.0), &params.osc1_shape, setter, &shapes);
    knob(ui, p, 14.0, 22.0, &params.osc1_coarse, setter, "CRS", true);
    knob(ui, p, 51.0, 22.0, &params.osc1_fine, setter, "FINE", true);
    knob(ui, p, 88.0, 22.0, &params.osc1_pw, setter, "PW", false);
    osc_preview(ui, p.rect(113.0, 19.0, 73.0, 34.0), t, params, params.osc1_shape.value(), params.osc1_pw.value());
    // OSC 2
    led_select(ui, p.rect(47.0, 59.0, 140.0, 15.0), &params.osc2_shape, setter, &shapes);
    knob(ui, p, 14.0, 79.0, &params.osc2_coarse, setter, "CRS", true);
    knob(ui, p, 51.0, 79.0, &params.osc2_fine, setter, "FINE", true);
    knob(ui, p, 88.0, 79.0, &params.osc2_pw, setter, "PW", false);
    knob(ui, p, 129.0, 79.0, &params.fm, setter, "FM", false);
    osc_preview(ui, p.rect(152.0, 76.0, 34.0, 34.0), t, params, params.osc2_shape.value(), params.osc2_pw.value());
    // OSC 1+2
    knob(ui, p, 208.0, 22.0, &params.osc_mix, setter, "MIX", false);
    knob(ui, p, 208.0, 59.0, &params.ring, setter, "RM", false);
    toggle_led(ui, p.rect(199.0, 92.0, 38.0, 17.0), &params.sync, setter, "SYNC", "Restart osc 1 each time osc 2 completes a cycle");
    // ENVELOPE
    env_display(ui, p.rect(114.0, 118.0, 126.0, 21.0), params);
    knob(ui, p, 14.0, 145.0, &params.attack, setter, "ATT", false);
    knob(ui, p, 51.0, 145.0, &params.decay, setter, "DEC", false);
    knob(ui, p, 88.0, 145.0, &params.sustain_time, setter, "SUS", false);
    knob(ui, p, 125.0, 145.0, &params.sustain_level, setter, "SL", false);
    knob(ui, p, 162.0, 145.0, &params.release, setter, "REL", false);
    knob(ui, p, 208.0, 145.0, &params.gate, setter, "GAT", false);
    // FILTER
    led_select(ui, p.rect(57.0, 183.0, 135.0, 14.0), &params.filter_type, setter,
               &[Cell::Text("LP12"), Cell::Text("LP24"), Cell::Text("HP"), Cell::Text("BP"), Cell::Text("OFF")]);
    knob(ui, p, 14.0, 202.0, &params.cutoff, setter, "CUT", false);
    knob(ui, p, 51.0, 202.0, &params.reso, setter, "RES", false);
    knob(ui, p, 88.0, 202.0, &params.env_amt, setter, "ENV", false);
    filter_readout(ui, p.rect(113.0, 200.0, 128.0, 34.0), params);
    // LFO
    led_select(ui, p.rect(38.0, 240.0, 84.0, 15.0), &params.lfo_shape, setter,
               &[Cell::Icon(Icon::Sine), Cell::Icon(Icon::Square), Cell::Icon(Icon::Triangle), Cell::Icon(Icon::Saw)]);
    led_select(ui, p.rect(127.0, 241.0, 112.0, 14.0), &params.lfo_target, setter,
               &[Cell::Text("OSC"), Cell::Text("RES"), Cell::Text("CUT"), Cell::Text("PW")]);
    knob(ui, p, 14.0, 259.0, &params.lfo_amt, setter, "AMT", false);
    knob(ui, p, 51.0, 259.0, &params.lfo_speed, setter, "SPD", false);
    lfo_preview(ui, p.rect(76.0, 257.0, 165.0, 34.0), t, params);
    // DIST
    led_select(ui, p.rect(43.0, 298.0, 30.0, 14.0), &params.dist_type, setter, &[Cell::Text("A"), Cell::Text("B")]);
    knob(ui, p, 14.0, 316.0, &params.dist_amount, setter, "AMT", false);
    knob(ui, p, 51.0, 316.0, &params.dist_thres, setter, "THR", false);
    dist_preview(ui, p.rect(78.0, 313.0, 108.0, 37.0), t, params);
    // DELAY: send to the delay line (its settings are in the strip below)
    knob(ui, p, 208.0, 316.0, &params.delay_amt, setter, "AMT", false);
}

// ------------------------------------------------------------------ header / footer

fn header(ui: &mut Ui, r: Rect, params: &SynthParams, setter: &ParamSetter, st: &mut UiState) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r.shrink2(vec2(8.0, 6.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let ui = &mut child;
    ui.label(egui::RichText::new("MONOSYNTH").font(FontId::proportional(19.0)).color(ACCENT).strong());
    ui.add_space(4.0);
    egui::ComboBox::from_id_salt("preset")
        .width(118.0)
        .selected_text(BUILTIN.get(st.preset).map(|p| p.0.to_string()).unwrap_or_else(|| st.status.clone()))
        .show_ui(ui, |ui| {
            for (i, (name, p)) in BUILTIN.iter().enumerate() {
                if ui.selectable_label(st.preset == i, *name).clicked() {
                    st.preset = i;
                    apply_preset(setter, params, p);
                }
            }
        });
    if ui.button("Load").on_hover_text("Load a .404 TS404 preset, or the TS404 channels + delay line of an FL project (.flp).\nLoading the same .flp again steps to its next TS404 channel.").clicked() {
        if let Some(path) = rfd::FileDialog::new().add_filter("TS404 preset / FL project", &["404", "flp", "FLP"]).pick_file() {
            let bytes = std::fs::read(&path).ok();
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let is_flp = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("flp"));
            match (bytes, is_flp) {
                (Some(b), true) => match presets::parse_flp(&b) {
                    Some(f) if !f.channels.is_empty() => {
                        let k = if st.flp_path.as_deref() == Some(path.as_path()) { (st.flp_next) % f.channels.len() } else { 0 };
                        let (cname, p) = &f.channels[k];
                        apply_preset(setter, params, p);
                        if let Some([fb, pan, vol, ticks]) = f.delay_line {
                            for (param, v) in [(&params.delay_feedback, fb), (&params.delay_pan, pan), (&params.delay_vol, vol), (&params.delay_time, ticks)] {
                                setter.begin_set_parameter(param);
                                setter.set_parameter(param, param.preview_plain(param.preview_normalized(v)));
                                setter.end_set_parameter(param);
                            }
                        }
                        st.flp_path = Some(path.clone());
                        st.flp_next = k + 1;
                        st.preset = usize::MAX;
                        st.status = format!("{stem}: {cname} ({}/{})", k + 1, f.channels.len());
                    }
                    Some(_) => st.status = "no TS404 channels in that project".into(),
                    None => st.status = "not an FL project".into(),
                },
                (Some(b), false) => match presets::parse_404(&b) {
                    Some(p) => {
                        apply_preset(setter, params, &p);
                        st.preset = usize::MAX;
                        st.status = stem;
                    }
                    None => st.status = "not a .404 file".into(),
                },
                (None, _) => st.status = "can't read that file".into(),
            }
        }
    }
    let shape_name = params.shape.read().ok().and_then(|g| g.as_ref().map(|s| s.name.clone()));
    let shape_btn = ui.button("Shape…").on_hover_text(match &shape_name {
        Some(n) => format!("\"?\" shape: {n}\nClick to load a WAV, right-click to clear."),
        None => "Load a WAV as the \"?\" oscillator shape (like dropping a sample on the TS404).\nWithout one, \"?\" plays the saw.".to_string(),
    });
    if shape_btn.clicked() {
        if let Some(path) = rfd::FileDialog::new().add_filter("WAV", &["wav", "WAV"]).pick_file() {
            match std::fs::read(&path).map_err(|e| e.to_string()).and_then(|b| ts404_core::shape::read_wav(&b)) {
                Ok(frames) if !frames.is_empty() => {
                    let table = ts404_core::shape::shape_table(&frames);
                    let id = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(1) | 1;
                    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    if let Ok(mut g) = params.shape.write() {
                        *g = Some(crate::params::ShapeData { name: name.clone(), table: crate::shape_io::encode(&table), id });
                    }
                    st.status = format!("shape: {name}");
                }
                Ok(_) => st.status = "empty WAV".into(),
                Err(e) => st.status = e,
            }
        }
    }
    if shape_btn.secondary_clicked() {
        if let Ok(mut g) = params.shape.write() {
            *g = None;
        }
    }
    if ui.button("Save .404").clicked() {
        if let Some(path) = rfd::FileDialog::new().add_filter("TS404 preset", &["404"]).set_file_name("preset.404").save_file() {
            if std::fs::write(&path, presets::write_404(&params.engine_params())).is_ok() {
                st.preset = usize::MAX;
                st.status = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            }
        }
    }
}

fn footer(ui: &mut Ui, r: Rect, params: &SynthParams, setter: &ParamSetter) {
    let section = |ui: &mut Ui, rect: Rect, title: &str| {
        ui.painter().rect_filled(rect, CornerRadius::same(6), PANEL);
        ui.painter().rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0_f32, PANEL_EDGE), StrokeKind::Inside);
        ui.painter().text(rect.min + vec2(7.0, 5.0), Align2::LEFT_TOP, title, FontId::proportional(14.0), DIM);
    };
    // slide / play options
    let a = Rect::from_min_size(r.min + vec2(2.0, 2.0), vec2(r.width() - 4.0, 50.0));
    section(ui, a, "SLIDE");
    led_select(ui, Rect::from_min_size(a.min + vec2(58.0, 4.0), vec2(230.0, 20.0)), &params.step_len, setter,
               &[Cell::Text("1/32"), Cell::Text("1/16"), Cell::Text("1/8"), Cell::Text("1/4"), Cell::Text("ms")]);
    if params.step_len.value() == StepLen::Free {
        knob_in(ui, Rect::from_min_size(a.min + vec2(296.0, 2.0), vec2(22.0, 22.0)), &params.step_ms, setter, "", false);
    }
    ui.painter().text(a.min + vec2(330.0, 8.0), Align2::LEFT_TOP, "play legato to slide", FontId::proportional(11.0), DIM);
    toggle_led(ui, Rect::from_min_size(a.min + vec2(4.0, 27.0), vec2(228.0, 20.0)), &params.fl_slides, setter,
               "TS404 slide timing (+1 step delay)",
               "Slides start one step early, like the TS404's slide steps.\nAdds one step of latency (the DAW compensates): for sequenced parts, not live playing.");
    toggle_led(ui, Rect::from_min_size(a.min + vec2(244.0, 27.0), vec2(230.0, 20.0)), &params.hq, setter,
               "HQ distortion", "Per-sample distortion, as the TS404 used when rendering in HQ. Off is its realtime sound.");
    // delay line and output
    let b = Rect::from_min_size(r.min + vec2(2.0, 55.0), vec2(318.0, 60.0));
    section(ui, b, "DELAY LINE");
    let o = Rect::from_min_size(r.min + vec2(323.0, 55.0), vec2(r.width() - 325.0, 60.0));
    section(ui, o, "OUTPUT");
    let k = |x: f32, rr: Rect| Rect::from_min_size(rr.min + vec2(x, 24.0), vec2(22.0, 22.0));
    knob_in(ui, k(100.0, b), &params.delay_feedback, setter, "FEED", false);
    knob_in(ui, k(150.0, b), &params.delay_pan, setter, "PAN", true);
    knob_in(ui, k(200.0, b), &params.delay_vol, setter, "VOL", false);
    knob_in(ui, k(250.0, b), &params.delay_time, setter, "TIME", false);
    ui.painter().text(b.min + vec2(8.0, 28.0), Align2::LEFT_TOP,
                      params.delay_time.normalized_value_to_string(params.delay_time.unmodulated_normalized_value(), true),
                      FontId::monospace(11.0), ACCENT);
    knob_in(ui, k(30.0, o), &params.gain, setter, "VOL", false);
    knob_in(ui, k(95.0, o), &params.pan, setter, "PAN", true);
}

// ------------------------------------------------------------------ widgets

#[derive(Clone, Copy)]
enum Icon {
    Saw,
    Pulse,
    Sine,
    Square,
    Triangle,
}

#[derive(Clone, Copy)]
enum Cell {
    Icon(Icon),
    Text(&'static str),
}

fn draw_icon(painter: &egui::Painter, r: Rect, icon: Icon, color: Color32) {
    let (x0, x1, y0, y1) = (r.left(), r.right(), r.top(), r.bottom());
    let mid = (y0 + y1) / 2.0;
    let at = |f: f32| x0 + (x1 - x0) * f;
    let pts: Vec<Pos2> = match icon {
        Icon::Saw => vec![pos2(x0, y1), pos2(x1, y0), pos2(x1, y1)],
        Icon::Pulse => vec![pos2(x0, y1), pos2(at(0.2), y0), pos2(at(0.55), y0), pos2(at(0.55), y1), pos2(x1, y1)],
        Icon::Square => vec![pos2(x0, y1), pos2(x0, y0), pos2(at(0.5), y0), pos2(at(0.5), y1), pos2(x1, y1), pos2(x1, y0)],
        Icon::Triangle => vec![pos2(x0, y1), pos2(at(0.5), y0), pos2(x1, y1)],
        Icon::Sine => (0..=16).map(|i| {
            let f = i as f32 / 16.0;
            pos2(at(f), mid - (f * 2.0 * PI).sin() * (y1 - y0) / 2.0)
        }).collect(),
    };
    painter.add(Shape::line(pts, Stroke::new(1.6_f32, color)));
}

/// Row of LED buttons, one per enum value.
fn led_select<T: Enum + PartialEq + 'static>(ui: &mut Ui, r: Rect, p: &EnumParam<T>, setter: &ParamSetter, cells: &[Cell]) {
    let cur = T::to_index(p.value());
    let w = r.width() / cells.len() as f32;
    for (i, cell) in cells.iter().enumerate() {
        let cr = Rect::from_min_size(r.min + vec2(i as f32 * w, 0.0), vec2(w - 3.0, r.height()));
        let resp = ui.allocate_rect(cr, Sense::click());
        let sel = cur == i;
        let painter = ui.painter();
        painter.rect_filled(cr, CornerRadius::same(4), if sel { ACCENT_DIM } else if resp.hovered() { TRACK } else { PANEL_EDGE });
        let led = Rect::from_center_size(pos2(cr.left() + 7.0, cr.center().y), vec2(6.0, 6.0));
        painter.rect_filled(led, CornerRadius::same(1), if sel { ACCENT } else { BG });
        let content = Rect::from_min_max(pos2(cr.left() + 14.0, cr.top() + 6.0), pos2(cr.right() - 4.0, cr.bottom() - 6.0));
        let color = if sel { ACCENT } else { TEXT };
        match cell {
            Cell::Icon(icon) => draw_icon(painter, content, *icon, color),
            Cell::Text(s) => {
                painter.text(content.center(), Align2::CENTER_CENTER, *s, FontId::proportional(11.5), color);
            }
        }
        if resp.clicked() && !sel {
            setter.begin_set_parameter(p);
            setter.set_parameter(p, T::from_index(i));
            setter.end_set_parameter(p);
        }
    }
}

fn toggle_led(ui: &mut Ui, r: Rect, p: &BoolParam, setter: &ParamSetter, label: &str, help: &str) {
    let on = p.value();
    let resp = ui.allocate_rect(r, Sense::click()).on_hover_text(help);
    let painter = ui.painter();
    let led = Rect::from_center_size(pos2(r.left() + 8.0, r.center().y), vec2(9.0, 9.0));
    painter.rect_filled(led, CornerRadius::same(2), if on { ACCENT } else { TRACK });
    painter.text(pos2(r.left() + 18.0, r.center().y), Align2::LEFT_CENTER, label, FontId::proportional(12.0),
                 if on { ACCENT } else if resp.hovered() { TEXT } else { DIM });
    if resp.clicked() {
        setter.begin_set_parameter(p);
        setter.set_parameter(p, !on);
        setter.end_set_parameter(p);
    }
}

/// Knob at a panel-grid position (19x19 cell) with its label underneath.
fn knob<P: Param>(ui: &mut Ui, p: Panel, x: f32, y: f32, param: &P, setter: &ParamSetter, label: &str, bipolar: bool) {
    knob_in(ui, p.rect(x - 1.0, y - 1.0, 21.0, 21.0), param, setter, label, bipolar);
}

/// Rotary knob: drag vertically (shift = fine), wheel steps, double-click resets.
fn knob_in<P: Param>(ui: &mut Ui, r: Rect, p: &P, setter: &ParamSetter, label: &str, bipolar: bool) -> Response {
    let resp = ui.allocate_rect(r, Sense::click_and_drag());
    let id = resp.id;
    let norm = p.unmodulated_normalized_value();
    if resp.drag_started() {
        setter.begin_set_parameter(p);
        ui.memory_mut(|m| m.data.insert_temp(id, norm));
    }
    if resp.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let mut v: f32 = ui.memory(|m| m.data.get_temp(id)).unwrap_or(norm);
        v = (v - resp.drag_delta().y * if fine { 0.0015 } else { 0.0055 }).clamp(0.0, 1.0);
        ui.memory_mut(|m| m.data.insert_temp(id, v));
        setter.set_parameter_normalized(p, v);
    }
    if resp.drag_stopped() {
        setter.end_set_parameter(p);
    }
    if resp.double_clicked() {
        setter.begin_set_parameter(p);
        setter.set_parameter_normalized(p, p.default_normalized_value());
        setter.end_set_parameter(p);
    }
    if resp.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        if scroll != 0.0 {
            let step = p.step_count().map(|n| 1.0 / n as f32).unwrap_or(0.01);
            setter.begin_set_parameter(p);
            setter.set_parameter_normalized(p, (norm + step * scroll.signum()).clamp(0.0, 1.0));
            setter.end_set_parameter(p);
        }
    }

    let painter = ui.painter();
    let c = r.center();
    let rad = r.width() / 2.0 - 2.0;
    let start = 0.75 * PI;
    let sweep = 1.5 * PI;
    let arc = |from: f32, to: f32, color: Color32| {
        let pts: Vec<Pos2> = (0..=32).map(|i| {
            let a = from + (to - from) * i as f32 / 32.0;
            c + vec2(a.cos(), a.sin()) * rad
        }).collect();
        painter.add(Shape::line(pts, Stroke::new(3.0_f32, color)));
    };
    arc(start, start + sweep, TRACK);
    let val_a = start + sweep * norm;
    let from_a = if bipolar { start + sweep * 0.5 } else { start };
    if (val_a - from_a).abs() > 1e-3 {
        arc(from_a.min(val_a), from_a.max(val_a), ACCENT);
    }
    let active = resp.hovered() || resp.dragged();
    painter.circle_filled(c, rad - 5.0, if active { PANEL_EDGE } else { BG });
    let dir = vec2(val_a.cos(), val_a.sin());
    painter.line_segment([c + dir * (rad - 11.0), c + dir * (rad - 5.0)], Stroke::new(2.5_f32, TEXT));
    if !label.is_empty() {
        let text = if active { p.normalized_value_to_string(norm, true) } else { label.to_string() };
        painter.text(pos2(c.x, r.bottom() + 7.0), Align2::CENTER_CENTER, text, FontId::proportional(11.0), if active { ACCENT } else { DIM });
    }
    resp.on_hover_text(format!("{}: {}", p.name(), p.normalized_value_to_string(norm, true)))
}

fn screen(ui: &Ui, r: Rect) {
    ui.painter().rect_filled(r, CornerRadius::same(4), SCREEN);
    ui.painter().rect_stroke(r, CornerRadius::same(4), Stroke::new(1.0_f32, PANEL_EDGE), StrokeKind::Inside);
}

fn trace(ui: &Ui, r: Rect, n: usize, f: impl Fn(f32) -> f32, color: Color32) {
    let pts: Vec<Pos2> = (0..=n).map(|i| {
        let x = i as f32 / n as f32;
        pos2(r.left() + x * r.width(), r.center().y - f(x).clamp(-1.0, 1.0) * r.height() / 2.0)
    }).collect();
    ui.painter().add(Shape::line(pts, Stroke::new(1.5_f32, color)));
}

// ------------------------------------------------------------------ previews

/// One cycle of the oscillator exactly as the engine reads it (pulse width included).
fn osc_preview(ui: &Ui, r: Rect, t: &Tables, params: &SynthParams, shape: OscShape, pw: i32) {
    screen(ui, r);
    let mut custom: Option<Box<Wave>> = None;
    if shape == OscShape::Sample {
        if let Ok(g) = params.shape.read() {
            if let Some(sd) = &*g {
                let mut w: Box<Wave> = vec![0f32; ts404_core::tables::WAVE_LEN].into_boxed_slice().try_into().unwrap();
                if crate::shape_io::decode_into(&sd.table, &mut w) {
                    custom = Some(w);
                }
            }
        }
    }
    let wave: &Wave = match (&custom, shape) {
        (Some(w), _) => w,
        (None, OscShape::Sample) => &t.osc[0],
        (None, s) => &t.osc[s as usize],
    };
    let pw = (pw << 23) as u32;
    trace(ui, r.shrink(4.0), 96, |x| {
        let phase = (x as f64 * 4_294_967_295.0) as u32;
        let rd = if phase >= pw { phase - pw } else { phase };
        wave[(rd >> 18) as usize]
    }, ACCENT);
}

fn cutoff_hz(index: i32) -> f64 {
    (40.0 * 1.05f64.powf(index.clamp(0, 255) as f64 / 2.0 + 15.0)).min(20050.0)
}

fn fmt_hz(f: f64) -> String {
    if f >= 1000.0 { format!("{:.2}k", f / 1000.0) } else { format!("{f:.0}") }
}

fn filter_readout(ui: &Ui, r: Rect, params: &SynthParams) {
    screen(ui, r);
    let cut = params.cutoff.value();
    let env_peak = (cut * 128 + 1 + ((params.env_amt.value() << 7) as f64 * 0.5).round() as i32) >> 7;
    let ft = params.filter_type.value();
    let base = if ft == FilterType::Lp24 { 9.033_203 } else { 8.544_922 };
    let q = (base - (128 - params.reso.value()) as f64 / 16.0) / 2.0;
    let line1 = format!("{} → {} Hz", fmt_hz(cutoff_hz(cut)), fmt_hz(cutoff_hz(env_peak.min(255))));
    let line2 = match ft {
        FilterType::Off => "filter off".to_string(),
        FilterType::Bp => "band-pass, fixed Q (RES unused)".to_string(),
        _ => format!("Q {q:.2}   (cutoff, then env peak)"),
    };
    let p = ui.painter();
    p.text(r.left_top() + vec2(8.0, 6.0), Align2::LEFT_TOP, line1, FontId::monospace(12.5), ACCENT);
    p.text(r.left_top() + vec2(8.0, 40.0), Align2::LEFT_TOP, line2, FontId::proportional(11.0), DIM);
}

fn lfo_preview(ui: &Ui, r: Rect, t: &Tables, params: &SynthParams) {
    screen(ui, r);
    let wave = &t.lfo[params.lfo_shape.value() as usize];
    let inner = Rect::from_min_max(r.min + vec2(6.0, 6.0), pos2(r.left() + r.width() * 0.45, r.bottom() - 6.0));
    let amt = params.lfo_amt.value() as f32 / 256.0;
    trace(ui, inner, 64, |x| wave[((x * 16383.0) as usize).min(16383)] * amt.max(0.08), if amt > 0.0 { ACCENT } else { DIM });
    let inc = (((params.lfo_speed.value() - 50) as f64 * 0.07).exp() * 97391.548_662_131_5).round();
    let hz = inc * 44100.0 / 4_294_967_296.0;
    let target = match params.lfo_target.value() {
        LfoTarget::Pitch => "pitch",
        LfoTarget::Reso => "resonance",
        LfoTarget::Cutoff => "cutoff",
        LfoTarget::Pw => "osc 1 PW",
    };
    let p = ui.painter();
    p.text(pos2(r.left() + r.width() * 0.5, r.top() + 8.0), Align2::LEFT_TOP, format!("{hz:.2} Hz"), FontId::monospace(12.5), ACCENT);
    p.text(pos2(r.left() + r.width() * 0.5, r.top() + 40.0), Align2::LEFT_TOP, format!("to {target}"), FontId::proportional(11.0), DIM);
}

/// The distortion curve the engine applies (|x| -> |y|, dry/wet mixed).
fn dist_preview(ui: &Ui, r: Rect, t: &Tables, params: &SynthParams) {
    screen(ui, r);
    let thr = params.dist_thres.value();
    if thr == 0 {
        ui.painter().text(r.center(), Align2::CENTER_CENTER, "off (THR 0)", FontId::proportional(11.0), DIM);
        return;
    }
    let inner = r.shrink(5.0);
    let ty = if params.dist_type.value() == DistType::Soft { 0 } else { 1 };
    let tab = &t.dist[ty][(thr - 1) as usize];
    let a = params.dist_amount.value() as f32 / 128.0;
    let pts: Vec<Pos2> = (0..=48).map(|i| {
        let x = i as f32 / 48.0;
        let y = x * (1.0 - a) + tab[((x * 8191.75) as usize).min(8191)] as f32 / 32768.0 * a;
        pos2(inner.left() + x * inner.width(), inner.bottom() - y.clamp(0.0, 1.0) * inner.height())
    }).collect();
    ui.painter().add(Shape::line(pts, Stroke::new(1.5_f32, ACCENT)));
}

/// Envelope shape, from the engine's own rate formulas (amp and filter share it).
fn env_display(ui: &mut Ui, r: Rect, params: &SynthParams) {
    screen(ui, r);
    let sr = 44100.0f64;
    let atk_inc = 0.5 * (0.1 * (-25.0 - params.attack.value() as f64)).exp();
    let t_a = 0.5 / atk_inc / sr;
    let dec = 0.9997 + (1.0 - 0.9997) * params.decay.value() as f64 * 0.01;
    let sus = params.sustain_level.value() as f64 * 0.005;
    let t_d = if sus >= 0.5 { 0.0 } else { (sus.max(5e-4) / 0.5).ln() / dec.ln() / sr };
    let hold_inf = params.sustain_time.value() == 100;
    let t_h = params.sustain_time.value() as f64 * 0.1;
    let rel = (7e-5 * (params.release.value() as f64 - 100.0)).exp();
    let t_r = if rel >= 1.0 { f64::INFINITY } else { (6e-8f64 / sus.max(1e-4)).ln() / rel.ln() / sr };
    let seg = |t: f64| if t.is_finite() { (t.max(0.0) * 1000.0 + 1.0).ln() } else { 3.0 };
    let total = seg(t_a) + seg(t_d) + if hold_inf { 1.5 } else { seg(t_h) } + seg(t_r);
    let inner = r.shrink2(vec2(5.0, 4.0));
    let x_of = |acc: f64| inner.left() + (acc / total) as f32 * inner.width();
    let y_of = |lvl: f64| inner.bottom() - (lvl / 0.5) as f32 * inner.height();
    let a0 = seg(t_a);
    let mut pts = vec![pos2(x_of(0.0), y_of(0.0)), pos2(x_of(a0), y_of(0.5))];
    for i in 1..=12 {
        let f = i as f64 / 12.0;
        pts.push(pos2(x_of(a0 + seg(t_d) * f), y_of((0.5 * dec.powf(f * t_d * sr)).max(sus))));
    }
    let r0 = a0 + seg(t_d) + if hold_inf { 1.5 } else { seg(t_h) };
    pts.push(pos2(x_of(r0), y_of(sus)));
    for i in 1..=12 {
        let f = i as f64 / 12.0;
        let lvl = if t_r.is_finite() { sus * rel.powf(f * t_r * sr) } else { sus };
        pts.push(pos2(x_of(r0 + seg(t_r) * f), y_of(lvl)));
    }
    ui.painter().add(Shape::line(pts, Stroke::new(1.5_f32, ACCENT)));
    let ms = |t: f64| if !t.is_finite() { "∞".into() } else if t >= 1.0 { format!("{t:.2}s") } else { format!("{:.0}ms", t * 1000.0) };
    let tip = format!(
        "Attack {}   Decay {}   Sustain level {:.0}%   Sustain time {}   Release {}\nShapes the amp, and the filter by ENV.",
        ms(t_a), ms(t_d), sus / 0.5 * 100.0, if hold_inf { "∞".into() } else { ms(t_h) }, ms(t_r)
    );
    ui.allocate_rect(r, Sense::hover()).on_hover_text(tip);
}

// ------------------------------------------------------------------ presets

fn apply_preset(setter: &ParamSetter, params: &SynthParams, p: &Preset404) {
    let get = |i: usize| p[i - 1];
    for (param, i, inverted) in params.int_params() {
        let v = if inverted { 128 - get(i) } else { get(i) };
        setter.begin_set_parameter(param);
        setter.set_parameter(param, param.preview_plain(param.preview_normalized(v)));
        setter.end_set_parameter(param);
    }
    let shape = |v: i32| match v {
        0 => OscShape::Saw,
        1 => OscShape::RcPulse,
        2 => OscShape::Sine,
        3 => OscShape::Square,
        _ => OscShape::Sample,
    };
    for (param, i) in [(&params.osc1_shape, idx::OSC1_SHAPE), (&params.osc2_shape, idx::OSC2_SHAPE)] {
        setter.begin_set_parameter(param);
        setter.set_parameter(param, shape(get(i)));
        setter.end_set_parameter(param);
    }
    let ft = [FilterType::Lp12, FilterType::Lp24, FilterType::Hp, FilterType::Bp, FilterType::Off];
    setter.begin_set_parameter(&params.filter_type);
    setter.set_parameter(&params.filter_type, ft[get(idx::FILTER_TYPE).clamp(0, 4) as usize]);
    setter.end_set_parameter(&params.filter_type);
    setter.begin_set_parameter(&params.dist_type);
    setter.set_parameter(&params.dist_type, if get(idx::DIST_TYPE) == 0 { DistType::Soft } else { DistType::Hard });
    setter.end_set_parameter(&params.dist_type);
    let lt = [LfoTarget::Pitch, LfoTarget::Reso, LfoTarget::Cutoff, LfoTarget::Pw];
    setter.begin_set_parameter(&params.lfo_target);
    setter.set_parameter(&params.lfo_target, lt[get(idx::LFO_TARGET).clamp(0, 3) as usize]);
    setter.end_set_parameter(&params.lfo_target);
    let ls = [LfoShape::Sine, LfoShape::Square, LfoShape::Triangle, LfoShape::Saw];
    setter.begin_set_parameter(&params.lfo_shape);
    setter.set_parameter(&params.lfo_shape, ls[get(idx::LFO_SHAPE).clamp(0, 3) as usize]);
    setter.end_set_parameter(&params.lfo_shape);
    setter.begin_set_parameter(&params.sync);
    setter.set_parameter(&params.sync, get(idx::SYNC) & 0xffff != 0);
    setter.end_set_parameter(&params.sync);
}
