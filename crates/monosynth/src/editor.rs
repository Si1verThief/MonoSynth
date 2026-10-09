//! Editor laid out like the TS404 panel (a 245x354 grid scaled by `S`), with live
//! previews of the oscillators, filter, LFO and distortion. The window can be any size:
//! the layout follows its shape (the channel settings go under the panel in a tall
//! window and beside it in a wide one) and everything is drawn at the scale that fits.

use std::collections::HashMap;
use std::f32::consts::PI;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

use egui::{
    self, Align2, Color32, CornerRadius, FontId, Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};
use nice_plug::context::gui::GuiContext;
use nice_plug::editor::dpi::{LogicalSize, NativeSize};
use nice_plug::editor::{HostMethods, SpawnedEditor};
use nice_plug::prelude::*;
use nice_plug_egui::baseview::{HandlerError, WindowSize};
use nice_plug_egui::{EguiEditor, EguiEditorHandle, EguiEditorState, EguiNiceSettings, Frame, NiceEguiApp, RepaintNotifier, create_egui_editor};

use crate::params::{ArpDir, DistType, FilterType, FlVersion, LfoShape, LfoTarget, OscShape, StepLen, SynthParams};
use crate::presets::{self, BUILTIN, Loaded, Preset404};
use ts404_core::flchan::ChannelKnobs;
use ts404_core::idx;
use ts404_core::tables3::Fl3;
use ts404_core::tables::{Tables, Wave};

const S: f32 = 2.0;
const PANEL_W: f32 = 245.0;
const PANEL_H: f32 = 354.0;
const HEADER: f32 = 40.0;
const FOOTER: f32 = 118.0;
/// The column beside the panel in a wide window, and the space between them.
const SIDE_W: f32 = 400.0;
const GAP: f32 = 6.0;
/// One channel page in that column.
const PAGE_H: f32 = 106.0;

const BG: Color32 = Color32::from_rgb(0x13, 0x14, 0x18);
const PANEL: Color32 = Color32::from_rgb(0x1e, 0x20, 0x26);
const PANEL_EDGE: Color32 = Color32::from_rgb(0x2c, 0x2f, 0x37);
const SCREEN: Color32 = Color32::from_rgb(0x10, 0x13, 0x14);
const TRACK: Color32 = Color32::from_rgb(0x36, 0x39, 0x42);
const TEXT: Color32 = Color32::from_rgb(0xd8, 0xdb, 0xe2);
const DIM: Color32 = Color32::from_rgb(0x84, 0x89, 0x95);
const ACCENT: Color32 = Color32::from_rgb(0xd4, 0xf5, 0x3c);
const ACCENT_DIM: Color32 = Color32::from_rgb(0x5f, 0x6e, 0x1e);

/// The FL 3.5 / FL 6 channel pages.
#[derive(Clone, Copy, PartialEq, Default)]
enum Tab {
    #[default]
    Channel,
    Poly,
    Echo,
    Arp,
    Track,
    Adjust,
}

const TABS: [(Tab, &str); 6] =
    [(Tab::Channel, "CHANNEL"), (Tab::Poly, "POLY"), (Tab::Echo, "ECHO"), (Tab::Arp, "ARP"), (Tab::Track, "TRACK"), (Tab::Adjust, "ADJUST")];

/// How the editor is arranged in the window.
#[derive(Clone, Copy, PartialEq, Default)]
enum Fit {
    /// The channel settings (or FL 2.71's slide and delay line) under the panel.
    #[default]
    Tall,
    /// Beside the panel, every channel page at once.
    Wide,
}

impl Fit {
    /// The editor's size at 100%, in logical pixels.
    fn size(self) -> Vec2 {
        match self {
            Fit::Tall => vec2(BASE_W, BASE_H),
            Fit::Wide => vec2(PANEL_W * S + GAP + SIDE_W, HEADER + PANEL_H * S),
        }
    }

    /// The arrangement that shows the editor biggest in a window of this size, and its scale.
    fn choose(win: Vec2) -> (Fit, f32) {
        let zoom = |f: Fit| {
            let s = f.size();
            (win.x / s.x).min(win.y / s.y).clamp(MIN_ZOOM, MAX_ZOOM)
        };
        let (tall, wide) = (zoom(Fit::Tall), zoom(Fit::Wide));
        if wide > tall { (Fit::Wide, wide) } else { (Fit::Tall, tall) }
    }
}

/// The preset browser's selection.
struct Browser {
    source: usize,
    group: usize,
    /// Scroll the list to the loaded preset (on opening).
    scroll: bool,
}

#[derive(Default)]
struct UiState {
    /// A message shown in place of the preset name until the next preset loads.
    status: String,
    tab: Tab,
    flp_path: Option<std::path::PathBuf>,
    flp_next: usize,
    browser: Option<Browser>,
    /// The arrangement in use.
    fit: Fit,
}

// ------------------------------------------------------------------ scaling
//
// The editor scales its own geometry: every size below goes through these helpers.

thread_local! {
    static ZOOM: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

/// The UI scale: the window's size over the 100% size of the arrangement in use.
fn zf() -> f32 {
    ZOOM.with(|z| z.get())
}
fn zs(x: f32) -> f32 {
    x * zf()
}
fn zv(x: f32, y: f32) -> Vec2 {
    vec2(x, y) * zf()
}
fn font(size: f32) -> FontId {
    FontId::proportional(size * zf())
}
fn mono(size: f32) -> FontId {
    FontId::monospace(size * zf())
}
fn stroke(width: f32, color: Color32) -> Stroke {
    Stroke::new(width * zf(), color)
}
fn radius(r: f32) -> CornerRadius {
    CornerRadius::same((r * zf()).round().clamp(0.0, 255.0) as u8)
}

/// egui's own widgets (buttons, menus, tooltips) at scale `z`.
fn scaled_style(base: &egui::Style, z: f32) -> egui::Style {
    let mut s = base.clone();
    for f in s.text_styles.values_mut() {
        f.size *= z;
    }
    let m = |m: egui::Margin| {
        let k = |v: i8| (v as f32 * z).round().clamp(-128.0, 127.0) as i8;
        egui::Margin { left: k(m.left), right: k(m.right), top: k(m.top), bottom: k(m.bottom) }
    };
    let sp = &mut s.spacing;
    sp.item_spacing *= z;
    sp.button_padding *= z;
    sp.interact_size *= z;
    sp.window_margin = m(sp.window_margin);
    sp.menu_margin = m(sp.menu_margin);
    for v in [&mut sp.indent, &mut sp.icon_width, &mut sp.icon_width_inner, &mut sp.icon_spacing, &mut sp.combo_width,
              &mut sp.combo_height, &mut sp.tooltip_width, &mut sp.menu_width, &mut sp.menu_spacing, &mut sp.text_edit_width] {
        *v *= z;
    }
    s
}

/// Maps panel-grid coordinates to the screen.
#[derive(Clone, Copy)]
struct Panel {
    o: Pos2,
}

impl Panel {
    fn rect(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(self.o + zv(x * S, y * S), zv(w * S, h * S))
    }
}

/// The tall arrangement at 100%, in logical pixels.
pub const BASE_W: f32 = PANEL_W * S;
pub const BASE_H: f32 = HEADER + PANEL_H * S + FOOTER;
const MIN_ZOOM: f32 = 0.25;
const MAX_ZOOM: f32 = 4.0;

/// The window's size when nothing else is known: the tall arrangement at 100%.
pub const DEFAULT_SIZE: (u32, u32) = (BASE_W as u32, BASE_H as u32);
const MIN_SIZE: u32 = 400;
const RESIZE_HINT: ResizeHint =
    ResizeHint::resizable().with_min_logical_size(LogicalSize::new(MIN_SIZE as f32, MIN_SIZE as f32));

// ------------------------------------------------------------------ the editor

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The plugin's editor. The host can resize its window; the size is kept in the plugin
/// state, and the window reopens at it.
pub struct MonoEditor {
    params: Arc<SynthParams>,
    repaint: RepaintNotifier,
    app: Arc<Mutex<App>>,
    /// The egui editor behind the open window (made at the saved size each time it opens).
    egui: Mutex<Option<EguiEditor<AppRef>>>,
}

impl MonoEditor {
    pub fn new(params: Arc<SynthParams>, repaint: RepaintNotifier) -> Self {
        let app = App { params: params.clone(), gui: None, base_style: None, style: Arc::default(), zoom: 0.0, st: UiState::default() };
        Self { params, repaint, app: Arc::new(Mutex::new(app)), egui: Mutex::new(None) }
    }

    fn saved_size(&self) -> (u32, u32) {
        let (w, h) = self.params.window.read().map(|w| w.size).unwrap_or(DEFAULT_SIZE);
        (w.max(MIN_SIZE), h.max(MIN_SIZE))
    }
}

impl Editor for MonoEditor {
    type Handle = EguiEditorHandle;

    fn spawn(
        &self,
        parent: Option<ParentWindowHandle>,
        wait_for_parent: bool,
        fallback_scale_factor: Option<f64>,
        gui_context: GuiContext,
        host: Option<HostMethods>,
    ) -> Result<SpawnedEditor<Self::Handle>, Box<dyn std::error::Error>> {
        let (w, h) = self.saved_size();
        let state = EguiEditorState::from_size(LogicalSize::new(w as f32, h as f32), 1.0);
        let settings = EguiNiceSettings::new().with_tile("MonoSynth").with_resize_hint(RESIZE_HINT);
        let editor = create_egui_editor(state, self.repaint.clone(), settings, AppRef(self.app.clone())).ok_or("no editor")?;
        let spawned = editor.spawn(parent, wait_for_parent, fallback_scale_factor, gui_context, host)?;
        *lock(&self.egui) = Some(editor);
        Ok(spawned)
    }

    fn size(&self) -> NativeSize<u32> {
        match &*lock(&self.egui) {
            Some(e) => e.size(),
            None => {
                let (w, h) = self.saved_size();
                NativeSize::new(w, h)
            }
        }
    }

    fn resize_hint(&self) -> ResizeHint {
        RESIZE_HINT
    }
}

/// The editor's state, kept while its window is closed.
struct App {
    params: Arc<SynthParams>,
    /// The open window's connection to the plugin.
    gui: Option<GuiContext>,
    /// The style at 100%, the scaled one in use and its scale.
    base_style: Option<egui::Style>,
    style: Arc<egui::Style>,
    zoom: f32,
    st: UiState,
}

struct AppRef(Arc<Mutex<App>>);

impl NiceEguiApp for AppRef {
    fn build(&mut self, ctx: egui::Context, gui: GuiContext, _frame: &mut Frame) -> Result<(), HandlerError> {
        lock(&self.0).opened(&ctx, gui);
        Ok(())
    }

    fn ui(&mut self, ui: &mut Ui, _frame: &mut Frame) {
        lock(&self.0).ui(ui);
    }

    fn resized(&mut self, size: WindowSize) {
        let app = lock(&self.0);
        if let Ok(mut w) = app.params.window.write() {
            w.size = (size.logical.width.round() as u32, size.logical.height.round() as u32);
        }
    }

    fn editor_closed(&mut self) {
        lock(&self.0).gui = None;
    }
}

impl App {
    fn opened(&mut self, ctx: &egui::Context, gui: GuiContext) {
        ctx.set_theme(egui::Theme::Dark);
        let mut style = (*ctx.global_style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = BG;
        style.visuals.override_text_color = Some(TEXT);
        style.visuals.selection.bg_fill = ACCENT_DIM;
        style.visuals.widgets.inactive.weak_bg_fill = PANEL_EDGE;
        style.visuals.widgets.hovered.weak_bg_fill = TRACK;
        self.base_style = Some(style);
        self.zoom = 0.0;
        self.gui = Some(gui);
    }

    fn ui(&mut self, ui: &mut Ui) {
        let Some(gui) = self.gui.clone() else { return };
        let setter = gui.param_setter();
        let params = self.params.clone();
        let win = ui.ctx().content_rect();
        let (fit, zoom) = Fit::choose(win.size());
        ZOOM.with(|z| z.set(zoom));
        if (zoom - self.zoom).abs() > 1e-4 {
            self.zoom = zoom;
            if let Some(base) = &self.base_style {
                self.style = Arc::new(scaled_style(base, zoom));
                ui.ctx().set_global_style(self.style.clone());
            }
        }
        ui.set_style(self.style.clone());
        // Also redraw a few times a second when nothing changes: a host can hide and show
        // the window without telling the plugin, and what it showed may be gone by then.
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        ui.painter().rect_filled(win, 0.0, BG);

        let st = &mut self.st;
        st.fit = fit;
        let size = fit.size() * zoom;
        let top = win.min + ((win.size() - size) * 0.5).max(Vec2::ZERO);
        header(ui, Rect::from_min_size(top, vec2(size.x, zs(HEADER))), &params, &setter, st);
        let body = Rect::from_min_max(top + zv(0.0, HEADER), top + size);
        if st.browser.is_some() {
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                st.browser = None;
            } else {
                browser(ui, body, &params, &setter, st);
                return;
            }
        }
        panel(ui, Panel { o: body.min }, &params, &setter);
        let fl3 = params.version.value() != FlVersion::Fl271;
        match fit {
            Fit::Tall => {
                let r = Rect::from_min_size(body.min + zv(0.0, PANEL_H * S), zv(PANEL_W * S, FOOTER));
                if fl3 { channel_footer(ui, r, &params, &setter, st) } else { footer(ui, r, &params, &setter) }
            }
            Fit::Wide => {
                let r = Rect::from_min_size(body.min + zv(PANEL_W * S + GAP, 0.0), zv(SIDE_W, PANEL_H * S));
                if fl3 { channel_side(ui, r, &params, &setter) } else { side_271(ui, r, &params, &setter) }
            }
        }
    }
}

// ------------------------------------------------------------------ the TS404 panel

fn panel(ui: &mut Ui, p: Panel, params: &SynthParams, setter: &ParamSetter) {
    let t = crate::tables();
    // FL 3.5 / FL 6: cutoff, resonance and gate are the channel's
    let fl3 = params.version.value() != FlVersion::Fl271;
    // section boxes
    for (title, x, y, w, h) in [
        ("OSC 1", 0.0, 0.0, 190.0, 56.0),
        ("OSC 2", 0.0, 57.0, 190.0, 56.0),
        ("OSC 1+2", 191.0, 0.0, 54.0, 113.0),
        ("ENVELOPE", 0.0, 114.0, 245.0, 67.0),
        ("FILTER", 0.0, 182.0, 245.0, 56.0),
        ("LFO", 0.0, 239.0, 245.0, 56.0),
        ("DIST", 0.0, 296.0, 190.0, 58.0),
        (if fl3 { "LEVEL" } else { "DELAY" }, 191.0, 296.0, 54.0, 58.0),
    ] {
        let r = p.rect(x + 1.0, y + 1.0, w - 2.0, h - 2.0);
        ui.painter().rect_filled(r, radius(6.0), PANEL);
        ui.painter().rect_stroke(r, radius(6.0), stroke(1.0, PANEL_EDGE), StrokeKind::Inside);
        ui.painter().text(r.min + zv(7.0, 5.0), Align2::LEFT_TOP, title, font(14.0), DIM);
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
    if fl3 {
        knob(ui, p, 208.0, 145.0, &params.ch_gate, setter, "GAT", false);
    } else {
        knob(ui, p, 208.0, 145.0, &params.gate, setter, "GAT", false);
    }
    // FILTER
    led_select(ui, p.rect(57.0, 183.0, 135.0, 14.0), &params.filter_type, setter,
               &[Cell::Text("LP12"), Cell::Text("LP24"), Cell::Text("HP"), Cell::Text("BP"), Cell::Text("OFF")]);
    if fl3 {
        knob(ui, p, 14.0, 202.0, &params.ch_cut, setter, "CUT", false);
        knob(ui, p, 51.0, 202.0, &params.ch_res, setter, "RES", false);
    } else {
        knob(ui, p, 14.0, 202.0, &params.cutoff, setter, "CUT", false);
        knob(ui, p, 51.0, 202.0, &params.reso, setter, "RES", false);
    }
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
    if fl3 {
        knob(ui, p, 208.0, 316.0, &params.ch_vol, setter, "VOL", false);
    } else {
        // DELAY: send to the delay line (its settings are in the strip below)
        knob(ui, p, 208.0, 316.0, &params.delay_amt, setter, "AMT", false);
    }
}

// ------------------------------------------------------------------ header

fn header(ui: &mut Ui, r: Rect, params: &SynthParams, setter: &ParamSetter, st: &mut UiState) {
    let inner = r.shrink2(zv(8.0, 6.0));
    let cy = inner.center().y;
    let title = ui.painter().layout_no_wrap("MONOSYNTH".into(), font(19.0), ACCENT);
    let tw = title.size().x;
    ui.painter().galley(pos2(inner.left(), cy - title.size().y / 2.0), title, ACCENT);
    let vr = Rect::from_min_size(pos2(inner.left() + tw + zs(8.0), cy - zs(11.0)), zv(126.0, 22.0));
    led_select(ui, vr, &params.version, setter, &[Cell::Text("2.71"), Cell::Text("3.5"), Cell::Text("6")]);
    let fr = Rect::from_min_max(pos2(inner.right() - zs(44.0), cy - zs(12.0)), pos2(inner.right(), cy + zs(12.0)));
    let pr = Rect::from_min_max(pos2(vr.right() + zs(6.0), cy - zs(12.0)), pos2(fr.left() - zs(6.0), cy + zs(12.0)));
    preset_picker(ui, pr, params, setter, st);
    ui.scope_builder(egui::UiBuilder::new().max_rect(fr).layout(egui::Layout::right_to_left(egui::Align::Center)), |ui| {
        file_menu(ui, params, setter, st);
    });
}

/// ◀ preset name ▶: the arrows step through every preset, the name opens the browser.
fn preset_picker(ui: &mut Ui, r: Rect, params: &SynthParams, setter: &ParamSetter, st: &mut UiState) {
    let bw = zs(22.0);
    let prev = Rect::from_min_size(r.min, vec2(bw, r.height()));
    let next = Rect::from_min_size(pos2(r.right() - bw, r.top()), vec2(bw, r.height()));
    for (rect, step, tip) in [(prev, -1, "Previous preset"), (next, 1, "Next preset")] {
        let resp = ui.allocate_rect(rect, Sense::click()).on_hover_text(tip);
        let p = ui.painter();
        p.rect_filled(rect, radius(4.0), if resp.hovered() { TRACK } else { PANEL_EDGE });
        let (c, d) = (rect.center(), zs(4.5));
        let tri = if step < 0 {
            vec![pos2(c.x - d, c.y), pos2(c.x + d * 0.8, c.y - d), pos2(c.x + d * 0.8, c.y + d)]
        } else {
            vec![pos2(c.x + d, c.y), pos2(c.x - d * 0.8, c.y - d), pos2(c.x - d * 0.8, c.y + d)]
        };
        p.add(Shape::convex_polygon(tri, if resp.hovered() { ACCENT } else { TEXT }, Stroke::NONE));
        if resp.clicked() {
            step_preset(params, setter, st, step);
        }
    }
    let name_r = Rect::from_min_max(pos2(prev.right() + zs(3.0), r.top()), pos2(next.left() - zs(3.0), r.bottom()));
    let (name, cur) = preset_label(params);
    let (text, tip) = if !st.status.is_empty() {
        (st.status.clone(), st.status.clone())
    } else {
        let path = cur.map(|i| {
            let e = &entries()[i];
            format!("{} › {} › {}", SOURCES[e.source], e.group, e.name)
        });
        (name.clone(), path.unwrap_or(name))
    };
    let open = st.browser.is_some();
    let resp = ui.allocate_rect(name_r, Sense::click()).on_hover_text(format!("{tip}\nClick to browse the presets"));
    let p = ui.painter();
    p.rect_filled(name_r, radius(4.0), if open { ACCENT_DIM } else if resp.hovered() { TRACK } else { PANEL_EDGE });
    let galley = elided(ui, &text, font(13.0), name_r.width() - zs(12.0), if open { ACCENT } else { TEXT });
    p.galley(pos2(name_r.left() + zs(6.0), name_r.center().y - galley.size().y / 2.0), galley, TEXT);
    if resp.clicked() {
        st.browser = if open { None } else { Some(browser_at(cur, params)) };
    }
}

/// One line of text, cut short with "…" if it's wider than `width`.
fn elided(ui: &Ui, text: &str, font: FontId, width: f32, color: Color32) -> Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: width.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    ui.fonts_mut(|f| f.layout_job(job))
}

fn file_menu(ui: &mut Ui, params: &SynthParams, setter: &ParamSetter, st: &mut UiState) {
    ui.menu_button(RichText::new("File").font(font(13.0)), |ui| {
        ui.set_min_width(zs(200.0));
        if ui.button("Load preset / project…").on_hover_text("A .404 TS404 preset, a .fst channel preset or an FL project (.flp).\nIn FL 2.71 mode an .flp loads its TS404 channels and delay line; loading the same .flp again steps to its next TS404 channel.\nIn FL 3.5 / FL 6 mode the file loads onto the channel the way that FL loads it.").clicked() {
            ui.close();
            load_file(params, setter, st);
        }
        if ui.button("Save .404…").on_hover_text("The TS404's own parameters (FL 3.5 / FL 6 channel settings aren't part of a .404).").clicked() {
            ui.close();
            if let Some(path) = rfd::FileDialog::new().add_filter("TS404 preset", &["404"]).set_file_name("preset.404").save_file() {
                if std::fs::write(&path, presets::write_404(&params.engine_params())).is_ok() {
                    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                    set_preset_key(params, format!("file:{stem}"));
                    st.status.clear();
                }
            }
        }
        ui.separator();
        let shape_name = params.shape.read().ok().and_then(|g| g.as_ref().map(|s| s.name.clone()));
        if ui.button("Load \"?\" shape (WAV)…").on_hover_text("Load a WAV as the \"?\" oscillator shape (like dropping a sample on the TS404).\nWithout one, \"?\" plays the saw.").clicked() {
            ui.close();
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
        if let Some(n) = shape_name {
            if ui.button(format!("Clear shape ({n})")).clicked() {
                ui.close();
                if let Ok(mut g) = params.shape.write() {
                    *g = None;
                }
            }
        }
        ui.separator();
        ui.label(RichText::new("Window size").color(DIM));
        ui.horizontal(|ui| {
            for pct in [75, 100, 125, 150, 200] {
                if ui.button(format!("{pct}%")).on_hover_text("Or drag the window's edge").clicked() {
                    let size = st.fit.size() * pct as f32 / 100.0;
                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
                    ui.close();
                }
            }
        });
    });
}

// ------------------------------------------------------------------ presets

/// The browser's first column: MonoSynth's own presets, then each FL version's.
const SOURCES: [&str; 4] = ["MonoSynth", "FL 2.71", "FL 3.5", "FL 6"];

#[derive(Clone, Copy)]
enum Which {
    Builtin(usize),
    Factory(usize),
}

/// A preset in the browser, in browsing order.
struct Entry {
    source: usize,
    group: &'static str,
    name: &'static str,
    key: String,
    which: Which,
}

fn entries() -> &'static [Entry] {
    static E: OnceLock<Vec<Entry>> = OnceLock::new();
    E.get_or_init(|| {
        let mut out = Vec::new();
        let mut add = |source: usize, group: &'static str, name: &'static str, which| {
            out.push(Entry { source, group, name, key: format!("{}/{group}/{name}", SOURCES[source]), which });
        };
        for (i, (name, _)) in BUILTIN.iter().enumerate() {
            add(0, "Built-in", name, Which::Builtin(i));
        }
        for (i, f) in presets::factory().iter().enumerate() {
            let source = match f.version {
                FlVersion::Fl271 => 1,
                FlVersion::Fl35 => 2,
                FlVersion::Fl6 => 3,
            };
            add(source, &f.group, &f.name, Which::Factory(i));
        }
        out
    })
}

fn entry_index(key: &str) -> Option<usize> {
    static I: OnceLock<HashMap<&'static str, usize>> = OnceLock::new();
    I.get_or_init(|| entries().iter().enumerate().map(|(i, e)| (e.key.as_str(), i)).collect()).get(key).copied()
}

/// The groups of a source, in browsing order.
fn groups(source: usize) -> Vec<&'static str> {
    let mut g: Vec<&'static str> = Vec::new();
    for e in entries().iter().filter(|e| e.source == source) {
        if !g.contains(&e.group) {
            g.push(e.group);
        }
    }
    g
}

/// The name to show for the last chosen preset, and its place in the browser.
fn preset_label(params: &SynthParams) -> (String, Option<usize>) {
    let key = params.preset.read().map(|k| k.clone()).unwrap_or_default();
    if let Some(i) = entry_index(&key) {
        return (entries()[i].name.to_string(), Some(i));
    }
    match key.strip_prefix("file:") {
        Some(name) => (name.to_string(), None),
        None => ("Presets".to_string(), None),
    }
}

fn set_preset_key(params: &SynthParams, key: String) {
    if let Ok(mut k) = params.preset.write() {
        *k = key;
    }
}

fn load_entry(params: &SynthParams, setter: &ParamSetter, st: &mut UiState, i: usize) {
    let e = &entries()[i];
    match e.which {
        Which::Builtin(b) => {
            set_version(setter, params, FlVersion::Fl271);
            apply_preset(setter, params, &BUILTIN[b].1);
        }
        Which::Factory(f) => {
            if let Some(l) = presets::factory()[f].load() {
                apply_loaded(setter, params, l);
            }
        }
    }
    set_preset_key(params, e.key.clone());
    st.status.clear();
}

/// Load the next (`d` = 1) or previous (-1) preset, wrapping around. Without a current
/// preset, start at the current version's first one.
fn step_preset(params: &SynthParams, setter: &ParamSetter, st: &mut UiState, d: i32) {
    let n = entries().len() as i32;
    let i = match preset_label(params).1 {
        Some(i) => (i as i32 + d).rem_euclid(n) as usize,
        None => {
            let source = version_source(params);
            entries().iter().position(|e| e.source == source).unwrap_or(0)
        }
    };
    load_entry(params, setter, st, i);
    if let Some(b) = &mut st.browser {
        *b = browser_at(Some(i), params);
    }
}

fn version_source(params: &SynthParams) -> usize {
    match params.version.value() {
        FlVersion::Fl271 => 1,
        FlVersion::Fl35 => 2,
        FlVersion::Fl6 => 3,
    }
}

/// The browser opened at a preset (or at the current version).
fn browser_at(cur: Option<usize>, params: &SynthParams) -> Browser {
    match cur {
        Some(i) => {
            let e = &entries()[i];
            let group = groups(e.source).iter().position(|g| *g == e.group).unwrap_or(0);
            Browser { source: e.source, group, scroll: true }
        }
        None => Browser { source: version_source(params), group: 0, scroll: true },
    }
}

/// A clickable line in the browser.
fn browser_row(ui: &mut Ui, r: Rect, text: &str, count: Option<usize>, selected: bool, current: bool) -> Response {
    let resp = ui.allocate_rect(r, Sense::click());
    let p = ui.painter();
    if selected {
        p.rect_filled(r, radius(4.0), ACCENT_DIM);
    } else if resp.hovered() {
        p.rect_filled(r, radius(4.0), TRACK);
    }
    let mut w = r.width() - zs(14.0);
    if let Some(n) = count {
        let g = p.layout_no_wrap(n.to_string(), font(11.0), DIM);
        w -= g.size().x + zs(6.0);
        p.galley(pos2(r.right() - zs(7.0) - g.size().x, r.center().y - g.size().y / 2.0), g, DIM);
    }
    let color = if selected || current { ACCENT } else { TEXT };
    let g = elided(ui, text, font(13.0), w, color);
    ui.painter().galley(pos2(r.left() + zs(7.0), r.center().y - g.size().y / 2.0), g, color);
    resp
}

/// The preset browser, over the panel: source, group and preset, all by clicking.
fn browser(ui: &mut Ui, r: Rect, params: &SynthParams, setter: &ParamSetter, st: &mut UiState) {
    let bx = r.shrink(zs(2.0));
    section(ui, bx, Some("PRESETS"));
    ui.painter().text(bx.min + zv(84.0, 7.0), Align2::LEFT_TOP, "click to load, double-click to load and close", font(11.0), DIM);
    let close = Rect::from_min_size(pos2(bx.right() - zs(68.0), bx.top() + zs(5.0)), zv(62.0, 20.0));
    let resp = ui.allocate_rect(close, Sense::click());
    ui.painter().rect_filled(close, radius(4.0), if resp.hovered() { TRACK } else { PANEL_EDGE });
    ui.painter().text(close.center(), Align2::CENTER_CENTER, "Close", font(12.0), TEXT);
    if resp.clicked() {
        st.browser = None;
        return;
    }
    let cur = preset_label(params).1;
    let cur_e = cur.map(|i| &entries()[i]);
    let Some(b) = st.browser.as_mut() else { return };

    let top = bx.top() + zs(32.0);
    let bottom = bx.bottom() - zs(8.0);
    let col = |x0: f32, x1: f32| Rect::from_min_max(pos2(x0, top), pos2(x1, bottom));
    let src_c = col(bx.left() + zs(6.0), bx.left() + zs(118.0));
    let grp_c = col(src_c.right() + zs(6.0), src_c.right() + zs(124.0));
    let list_c = col(grp_c.right() + zs(6.0), bx.right() - zs(6.0));
    for (c, title) in [(src_c, "SOURCE"), (grp_c, "GROUP"), (list_c, "PRESET")] {
        ui.painter().text(c.min + zv(7.0, 0.0), Align2::LEFT_TOP, title, font(11.0), DIM);
        let line = c.top() + zs(16.0);
        ui.painter().line_segment([pos2(c.left(), line), pos2(c.right(), line)], stroke(1.0, PANEL_EDGE));
    }
    let row_h = zs(24.0);
    let row = |c: Rect, i: usize| Rect::from_min_size(pos2(c.left(), c.top() + zs(20.0) + i as f32 * (row_h + zs(2.0))), vec2(c.width(), row_h));

    for (i, name) in SOURCES.iter().enumerate() {
        let n = entries().iter().filter(|e| e.source == i).count();
        if browser_row(ui, row(src_c, i), name, Some(n), b.source == i, cur_e.is_some_and(|e| e.source == i)).clicked() && b.source != i {
            b.source = i;
            b.group = cur_e.filter(|e| e.source == i).and_then(|e| groups(i).iter().position(|g| *g == e.group)).unwrap_or(0);
            b.scroll = true;
        }
    }
    let gs = groups(b.source);
    for (i, g) in gs.iter().enumerate() {
        let n = entries().iter().filter(|e| e.source == b.source && e.group == *g).count();
        let current = cur_e.is_some_and(|e| e.source == b.source && e.group == *g);
        if browser_row(ui, row(grp_c, i), g, Some(n), b.group == i, current).clicked() && b.group != i {
            b.group = i;
            b.scroll = true;
        }
    }

    let group = gs.get(b.group).copied().unwrap_or("");
    let list = Rect::from_min_max(pos2(list_c.left(), list_c.top() + zs(20.0)), list_c.max);
    let (source, scroll) = (b.source, std::mem::take(&mut b.scroll));
    let mut load = None;
    let mut close = false;
    ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, zs(2.0));
        egui::ScrollArea::vertical().id_salt(("presets", source, group)).auto_shrink(false).show(ui, |ui| {
            // as many columns as fit, filled top to bottom
            let items: Vec<usize> = (0..entries().len()).filter(|&i| entries()[i].source == source && entries()[i].group == group).collect();
            let cols = ((ui.available_width() / zs(190.0)).floor() as usize).clamp(1, items.len().max(1));
            let rows = items.len().div_ceil(cols);
            let (cw, step) = (ui.available_width() / cols as f32, row_h + zs(2.0));
            let (area, _) = ui.allocate_exact_size(vec2(ui.available_width(), rows as f32 * step), Sense::hover());
            for (n, &i) in items.iter().enumerate() {
                let e = &entries()[i];
                let (c, r) = (n / rows, n % rows);
                let rr = Rect::from_min_size(area.min + vec2(c as f32 * cw, r as f32 * step), vec2(cw - zs(4.0), row_h));
                let is_cur = cur == Some(i);
                let resp = browser_row(ui, rr, e.name, None, is_cur, is_cur);
                if is_cur && scroll {
                    ui.scroll_to_rect(rr, Some(egui::Align::Center));
                }
                if resp.clicked() {
                    load = Some(i);
                }
                if resp.double_clicked() {
                    close = true;
                }
            }
        });
    });
    if let Some(i) = load {
        load_entry(params, setter, st, i);
    }
    if close {
        st.browser = None;
    }
}

/// Load a preset or project file chosen by the user, the way the current version loads it.
fn load_file(params: &SynthParams, setter: &ParamSetter, st: &mut UiState) {
    let Some(path) = rfd::FileDialog::new().add_filter("TS404 preset / FL channel preset / FL project", &["404", "fst", "FST", "flp", "FLP"]).pick_file() else {
        return;
    };
    let Ok(b) = std::fs::read(&path) else {
        st.status = "can't read that file".into();
        return;
    };
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    let loaded = |st: &mut UiState, name: String| {
        set_preset_key(params, format!("file:{name}"));
        st.status.clear();
    };
    if let Some(v) = params.version.value().fl3() {
        let cur = params.knobs(v);
        let k = if ext == "404" { ts404_core::flchan::load_404(v, &cur, &b) } else { ts404_core::flchan::load_fst(v, &cur, &b) };
        match k {
            Some(k) => {
                apply_loaded(setter, params, Loaded::Channel(v, k));
                loaded(st, stem);
            }
            None => st.status = "no TS404 in that file".into(),
        }
        return;
    }
    if ext == "404" {
        match presets::parse_404(&b) {
            Some(p) => {
                apply_preset(setter, params, &p);
                loaded(st, stem);
            }
            None => st.status = "not a .404 file".into(),
        }
        return;
    }
    match presets::parse_flp(&b) {
        Some(f) if !f.channels.is_empty() => {
            let k = if st.flp_path.as_deref() == Some(path.as_path()) { st.flp_next % f.channels.len() } else { 0 };
            let (cname, p) = &f.channels[k];
            apply_preset(setter, params, p);
            if let Some([fb, pan, vol, ticks]) = f.delay_line {
                for (param, v) in [(&params.delay_feedback, fb), (&params.delay_pan, pan), (&params.delay_vol, vol), (&params.delay_time, ticks)] {
                    set_int(setter, param, v);
                }
            }
            st.flp_path = Some(path.clone());
            st.flp_next = k + 1;
            let name = if f.channels.len() > 1 { format!("{stem}: {cname} ({}/{})", k + 1, f.channels.len()) } else { stem };
            loaded(st, name);
        }
        Some(_) => st.status = "no TS404 channels in that file".into(),
        None => st.status = "not an FL file".into(),
    }
}

// ------------------------------------------------------------------ settings beside / under the panel

/// A section box, optionally titled.
fn section(ui: &Ui, r: Rect, title: Option<&str>) {
    ui.painter().rect_filled(r, radius(6.0), PANEL);
    ui.painter().rect_stroke(r, radius(6.0), stroke(1.0, PANEL_EDGE), StrokeKind::Inside);
    if let Some(t) = title {
        ui.painter().text(r.min + zv(7.0, 5.0), Align2::LEFT_TOP, t, font(14.0), DIM);
    }
}

/// FL 2.71, tall window: slide options, the delay line and the output under the panel.
fn footer(ui: &mut Ui, r: Rect, params: &SynthParams, setter: &ParamSetter) {
    let a = Rect::from_min_size(r.min + zv(2.0, 2.0), vec2(r.width() - zs(4.0), zs(50.0)));
    section(ui, a, Some("SLIDE"));
    slide_options(ui, a, params, setter, false);
    let b = Rect::from_min_size(r.min + zv(2.0, 55.0), zv(318.0, 60.0));
    section(ui, b, Some("DELAY LINE"));
    delay_line(ui, b, params, setter);
    let o = Rect::from_min_size(r.min + zv(323.0, 55.0), vec2(r.width() - zs(325.0), zs(60.0)));
    section(ui, o, Some("OUTPUT"));
    let k = |x: f32| Rect::from_min_size(o.min + zv(x, 24.0), zv(22.0, 22.0));
    knob_in(ui, k(30.0), &params.gain, setter, "VOL", false);
    knob_in(ui, k(95.0), &params.pan, setter, "PAN", true);
}

/// FL 2.71, wide window: the same, stacked beside the panel.
fn side_271(ui: &mut Ui, r: Rect, params: &SynthParams, setter: &ParamSetter) {
    let a = Rect::from_min_size(r.min, vec2(r.width(), zs(100.0)));
    section(ui, a, Some("SLIDE"));
    slide_options(ui, a, params, setter, true);
    let b = Rect::from_min_size(pos2(r.left(), a.bottom() + zs(4.0)), vec2(r.width(), zs(60.0)));
    section(ui, b, Some("DELAY LINE"));
    delay_line(ui, b, params, setter);
    let o = Rect::from_min_size(pos2(r.left(), b.bottom() + zs(4.0)), vec2(r.width(), zs(60.0)));
    section(ui, o, Some("OUTPUT"));
    let k = |x: f32| Rect::from_min_size(o.min + zv(x, 24.0), zv(22.0, 22.0));
    knob_in(ui, k(100.0), &params.gain, setter, "VOL", false);
    knob_in(ui, k(150.0), &params.pan, setter, "PAN", true);
}

/// Slide length, TS404 slide timing and HQ distortion; one row of toggles, or two when
/// `stacked` (the narrower column beside the panel).
fn slide_options(ui: &mut Ui, a: Rect, params: &SynthParams, setter: &ParamSetter, stacked: bool) {
    led_select(ui, Rect::from_min_size(a.min + zv(58.0, 4.0), zv(230.0, 20.0)), &params.step_len, setter,
               &[Cell::Text("1/32"), Cell::Text("1/16"), Cell::Text("1/8"), Cell::Text("1/4"), Cell::Text("ms")]);
    if params.step_len.value() == StepLen::Free {
        knob_in(ui, Rect::from_min_size(a.min + zv(296.0, 2.0), zv(22.0, 22.0)), &params.step_ms, setter, "", false);
    }
    let (hint, timing, hq) = if stacked {
        (zv(8.0, 30.0), zv(4.0, 50.0), zv(4.0, 74.0))
    } else {
        (zv(330.0, 8.0), zv(4.0, 27.0), zv(244.0, 27.0))
    };
    ui.painter().text(a.min + hint, Align2::LEFT_TOP, "play legato to slide", font(11.0), DIM);
    toggle_led(ui, Rect::from_min_size(a.min + timing, zv(228.0, 20.0)), &params.fl_slides, setter,
               "TS404 slide timing (+1 step delay)",
               "Slides start one step early, like the TS404's slide steps.\nAdds one step of latency (the DAW compensates): for sequenced parts, not live playing.");
    toggle_led(ui, Rect::from_min_size(a.min + hq, zv(230.0, 20.0)), &params.hq, setter,
               "HQ distortion", "Per-sample distortion, as the TS404 used when rendering in HQ. Off is its realtime sound.");
}

/// FL 2.71's delay line (the DELAY knob sends to it).
fn delay_line(ui: &mut Ui, b: Rect, params: &SynthParams, setter: &ParamSetter) {
    let k = |x: f32| Rect::from_min_size(b.min + zv(x, 24.0), zv(22.0, 22.0));
    knob_in(ui, k(100.0), &params.delay_feedback, setter, "FEED", false);
    knob_in(ui, k(150.0), &params.delay_pan, setter, "PAN", true);
    knob_in(ui, k(200.0), &params.delay_vol, setter, "VOL", false);
    knob_in(ui, k(250.0), &params.delay_time, setter, "TIME", false);
    ui.painter().text(b.min + zv(8.0, 28.0), Align2::LEFT_TOP,
                      params.delay_time.normalized_value_to_string(params.delay_time.unmodulated_normalized_value(), true),
                      mono(11.0), ACCENT);
}

/// FL 3.5 / FL 6, tall window: the channel's settings one page at a time.
fn channel_footer(ui: &mut Ui, r: Rect, params: &SynthParams, setter: &ParamSetter, st: &mut UiState) {
    let tw = zs(64.0);
    for (i, (tab, label)) in TABS.iter().enumerate() {
        let tr = Rect::from_min_size(r.min + vec2(zs(2.0) + i as f32 * tw, zs(2.0)), vec2(tw - zs(3.0), zs(20.0)));
        let resp = ui.allocate_rect(tr, Sense::click());
        let sel = st.tab == *tab;
        ui.painter().rect_filled(tr, radius(4.0), if sel { ACCENT_DIM } else if resp.hovered() { TRACK } else { PANEL_EDGE });
        ui.painter().text(tr.center(), Align2::CENTER_CENTER, *label, font(11.5), if sel { ACCENT } else { TEXT });
        if resp.clicked() {
            st.tab = *tab;
        }
    }
    let b = Rect::from_min_size(r.min + zv(2.0, 25.0), zv(398.0, 90.0));
    section(ui, b, None);
    channel_page(ui, st.tab, b, params, setter);
    let o = Rect::from_min_size(r.min + zv(403.0, 25.0), vec2(r.width() - zs(405.0), zs(90.0)));
    section(ui, o, Some("OUTPUT"));
    knob_in(ui, Rect::from_min_size(o.min + zv(30.0, 46.0), zv(22.0, 22.0)), &params.gain, setter, "VOL", false);
}

/// FL 3.5 / FL 6, wide window: every page at once beside the panel, and the output.
fn channel_side(ui: &mut Ui, r: Rect, params: &SynthParams, setter: &ParamSetter) {
    let mut y = r.top();
    for (tab, label) in TABS {
        let bx = Rect::from_min_size(pos2(r.left(), y), vec2(r.width(), zs(PAGE_H)));
        section(ui, bx, Some(label));
        channel_page(ui, tab, Rect::from_min_size(bx.min + zv(0.0, 14.0), zv(SIDE_W, 92.0)), params, setter);
        y += zs(PAGE_H + 4.0);
    }
    let o = Rect::from_min_max(pos2(r.left(), y), r.max);
    section(ui, o, Some("OUTPUT"));
    knob_in(ui, Rect::from_min_size(pos2(o.left() + zs(110.0), o.top() + zs(8.0)), zv(22.0, 22.0)), &params.gain, setter, "VOL", false);
}

/// One channel page in box `b` (398x90 at 100%): toggles along the top, knobs below.
fn channel_page(ui: &mut Ui, tab: Tab, b: Rect, params: &SynthParams, setter: &ParamSetter) {
    let v6 = params.version.value() == FlVersion::Fl6;
    let k = |i: usize| Rect::from_min_size(b.min + zv(14.0 + 48.0 * i as f32, 46.0), zv(22.0, 22.0));
    let t = |x: f32, w: f32| Rect::from_min_size(b.min + zv(x, 8.0), zv(w, 20.0));
    let note = |ui: &mut Ui, text: &str| {
        ui.painter().text(b.min + zv(8.0, 12.0), Align2::LEFT_TOP, text, font(11.0), DIM);
    };
    match tab {
        Tab::Channel => {
            knob_in(ui, k(0), &params.ch_vol, setter, "VOL", false);
            knob_in(ui, k(1), &params.ch_pan, setter, "PAN", true);
            knob_in(ui, k(2), &params.ch_pitch, setter, "PITCH", true);
            knob_in(ui, k(3), &params.root, setter, "ROOT", false);
            knob_in(ui, k(4), &params.ch_fine, setter, "FINE", true);
            knob_in(ui, k(5), &params.shift, setter, "SHIFT", false);
            toggle_led(ui, t(8.0, 150.0), &params.alias_free, setter, "Alias-free",
                       "FL's \"alias-free TS404\" (its WAV export option): smoother oscillators.");
            if v6 {
                toggle_led(ui, t(160.0, 150.0), &params.hq, setter, "HQ distortion",
                           "FL 6's high-quality distortion (interpolated), as with HQ on in FL's mixer.");
            }
            ui.painter().text(b.min + zv(312.0, 12.0), Align2::LEFT_TOP, format!("root {}", crate::params::note_name(params.root.value())), font(11.0), DIM);
        }
        Tab::Poly => {
            toggle_led(ui, t(8.0, 70.0), &params.mono, setter, "Mono", "One voice: a new note takes over the playing one (and glides when slide is on).");
            toggle_led(ui, t(82.0, 80.0), &params.porta, setter, "Porta", "Glide to every new note (without it only slide notes glide).");
            toggle_led(ui, t(166.0, 150.0), &params.gate_skip, setter, "Slides skip gate", "Notes marked as slides ignore the gate.");
            knob_in(ui, k(0), &params.porta_time, setter, "SLIDE", false);
            knob_in(ui, k(1), &params.max_poly, setter, "MAX", false);
            knob_in(ui, k(2), &params.key_lo, setter, "LOW", false);
            knob_in(ui, k(3), &params.key_hi, setter, "HIGH", false);
            ui.painter().text(b.min + zv(206.0, 50.0), Align2::LEFT_TOP, "SLIDE: glide time   MAX: voices\nLOW/HIGH: key range", font(10.5), DIM);
        }
        Tab::Echo => {
            toggle_led(ui, t(8.0, 100.0), &params.echo_pingpong, setter, "Ping-pong", "Echoes alternate between two pan positions.");
            toggle_led(ui, t(112.0, 100.0), &params.echo_bounce, setter, "Bounce", "The echo pan bounces back at the edges.");
            knob_in(ui, k(0), &params.echo_feed, setter, "FEED", false);
            knob_in(ui, k(1), &params.echo_pan, setter, "PAN", true);
            knob_in(ui, k(2), &params.echo_pitch, setter, "PITCH", true);
            knob_in(ui, k(3), &params.echoes, setter, "COUNT", false);
            knob_in(ui, k(4), &params.echo_time, setter, "TIME", false);
            knob_in(ui, k(5), &params.echo_cut, setter, "CUT", true);
            knob_in(ui, k(6), &params.echo_res, setter, "RES", true);
        }
        Tab::Arp => {
            led_select(ui, t(8.0, 300.0), &params.arp_dir, setter,
                       &[Cell::Text("OFF"), Cell::Text("UP"), Cell::Text("DN"), Cell::Text("U+D"), Cell::Text("U+D+"), Cell::Text("RND")]);
            toggle_led(ui, t(314.0, 80.0), &params.arp_slide, setter, "Slide", "Glide from one arpeggio note to the next.");
            knob_in(ui, k(0), &params.arp_range, setter, "RANGE", false);
            knob_in(ui, k(1), &params.arp_chord, setter, "CHORD", false);
            knob_in(ui, k(2), &params.arp_time, setter, "TIME", false);
            knob_in(ui, k(3), &params.arp_gate, setter, "GATE", false);
            if v6 {
                knob_in(ui, k(4), &params.arp_repeat, setter, "REP", false);
            }
            // FL 3.5 has no Auto chord: there it means none
            let chord = if !v6 && params.arp_chord.value() < 0 {
                "None".to_string()
            } else {
                params.arp_chord.normalized_value_to_string(params.arp_chord.unmodulated_normalized_value(), true)
            };
            ui.painter().text(b.min + zv(if v6 { 254.0 } else { 206.0 }, 50.0), Align2::LEFT_TOP, chord, font(11.0), ACCENT);
        }
        Tab::Track => {
            note(ui, "Velocity and key move pan, cutoff and resonance (from the MID point)");
            knob_in(ui, k(0), &params.vel_mid, setter, "V MID", false);
            knob_in(ui, k(1), &params.vel_pan, setter, "V>PAN", true);
            knob_in(ui, k(2), &params.vel_cut, setter, "V>CUT", true);
            knob_in(ui, k(3), &params.vel_res, setter, "V>RES", true);
            knob_in(ui, k(4), &params.key_mid, setter, "K MID", false);
            knob_in(ui, k(5), &params.key_pan, setter, "K>PAN", true);
            knob_in(ui, k(6), &params.key_cut, setter, "K>CUT", true);
            knob_in(ui, k(7), &params.key_res, setter, "K>RES", true);
        }
        Tab::Adjust => {
            note(ui, "FL's level adjustments, added to the channel's knobs");
            knob_in(ui, k(0), &params.adj_pan, setter, "PAN", true);
            knob_in(ui, k(1), &params.adj_vol, setter, "VOL", false);
            knob_in(ui, k(2), &params.adj_cut, setter, "CUT", true);
            knob_in(ui, k(3), &params.adj_res, setter, "RES", true);
        }
    }
}

fn set_int(setter: &ParamSetter, param: &IntParam, v: i32) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, param.preview_plain(param.preview_normalized(v)));
    setter.end_set_parameter(param);
}

fn set_bool(setter: &ParamSetter, param: &BoolParam, v: bool) {
    setter.begin_set_parameter(param);
    setter.set_parameter(param, v);
    setter.end_set_parameter(param);
}

fn set_version(setter: &ParamSetter, params: &SynthParams, v: FlVersion) {
    setter.begin_set_parameter(&params.version);
    setter.set_parameter(&params.version, v);
    setter.end_set_parameter(&params.version);
}

/// Apply a loaded preset: FL 2.71 parameters, or an FL 3.5 / FL 6 channel (TS404 and
/// channel settings), switching to its version.
fn apply_loaded(setter: &ParamSetter, params: &SynthParams, l: Loaded) {
    match l {
        Loaded::Fl271(p) => {
            set_version(setter, params, FlVersion::Fl271);
            apply_preset(setter, params, &p);
        }
        Loaded::Channel(v, k) => {
            set_version(setter, params, if v == Fl3::V35 { FlVersion::Fl35 } else { FlVersion::Fl6 });
            apply_channel(setter, params, &k);
        }
    }
}

fn apply_channel(setter: &ParamSetter, params: &SynthParams, k: &ChannelKnobs) {
    // the TS404's own parameters (its step gate and delay send are FL 2.71's and stay)
    let mut p: Preset404 = params.engine_params();
    p[..33].copy_from_slice(&k.ts[1..34]);
    apply_preset(setter, params, &p);
    for (param, v) in params.channel_ints(k) {
        set_int(setter, param, v);
    }
    for (param, v) in params.channel_bools(k) {
        set_bool(setter, param, v);
    }
    let dirs = [ArpDir::Off, ArpDir::Up, ArpDir::Down, ArpDir::UpDown, ArpDir::UpDownRepeat, ArpDir::Random];
    setter.begin_set_parameter(&params.arp_dir);
    setter.set_parameter(&params.arp_dir, dirs[k.arp_dir.clamp(0, 5) as usize]);
    setter.end_set_parameter(&params.arp_dir);
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
    painter.add(Shape::line(pts, stroke(1.6, color)));
}

/// Row of LED buttons, one per enum value.
fn led_select<T: Enum + PartialEq + 'static>(ui: &mut Ui, r: Rect, p: &EnumParam<T>, setter: &ParamSetter, cells: &[Cell]) {
    let cur = T::to_index(p.value());
    let w = r.width() / cells.len() as f32;
    for (i, cell) in cells.iter().enumerate() {
        let cr = Rect::from_min_size(r.min + vec2(i as f32 * w, 0.0), vec2(w - zs(3.0), r.height()));
        let resp = ui.allocate_rect(cr, Sense::click());
        let sel = cur == i;
        let painter = ui.painter();
        painter.rect_filled(cr, radius(4.0), if sel { ACCENT_DIM } else if resp.hovered() { TRACK } else { PANEL_EDGE });
        let led = Rect::from_center_size(pos2(cr.left() + zs(7.0), cr.center().y), zv(6.0, 6.0));
        painter.rect_filled(led, radius(1.0), if sel { ACCENT } else { BG });
        let content = Rect::from_min_max(pos2(cr.left() + zs(14.0), cr.top() + zs(6.0)), pos2(cr.right() - zs(4.0), cr.bottom() - zs(6.0)));
        let color = if sel { ACCENT } else { TEXT };
        match cell {
            Cell::Icon(icon) => draw_icon(painter, content, *icon, color),
            Cell::Text(s) => {
                painter.text(content.center(), Align2::CENTER_CENTER, *s, font(11.5), color);
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
    let led = Rect::from_center_size(pos2(r.left() + zs(8.0), r.center().y), zv(9.0, 9.0));
    painter.rect_filled(led, radius(2.0), if on { ACCENT } else { TRACK });
    painter.text(pos2(r.left() + zs(18.0), r.center().y), Align2::LEFT_CENTER, label, font(12.0),
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
        // Same feel at any size: sensitivity per 100%-scale pixel.
        v = (v - resp.drag_delta().y / zf() * if fine { 0.0015 } else { 0.0055 }).clamp(0.0, 1.0);
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
        let scroll: f32 = ui.input(|i| i.events.iter().map(|e| if let egui::Event::MouseWheel { delta, .. } = e { delta.y } else { 0.0 }).sum());
        if scroll != 0.0 {
            let step = p.step_count().map(|n| 1.0 / n as f32).unwrap_or(0.01);
            setter.begin_set_parameter(p);
            setter.set_parameter_normalized(p, (norm + step * scroll.signum()).clamp(0.0, 1.0));
            setter.end_set_parameter(p);
        }
    }

    let painter = ui.painter();
    let c = r.center();
    let rad = r.width() / 2.0 - zs(2.0);
    let start = 0.75 * PI;
    let sweep = 1.5 * PI;
    let arc = |from: f32, to: f32, color: Color32| {
        let pts: Vec<Pos2> = (0..=32).map(|i| {
            let a = from + (to - from) * i as f32 / 32.0;
            c + vec2(a.cos(), a.sin()) * rad
        }).collect();
        painter.add(Shape::line(pts, stroke(3.0, color)));
    };
    arc(start, start + sweep, TRACK);
    let val_a = start + sweep * norm;
    let from_a = if bipolar { start + sweep * 0.5 } else { start };
    if (val_a - from_a).abs() > 1e-3 {
        arc(from_a.min(val_a), from_a.max(val_a), ACCENT);
    }
    let active = resp.hovered() || resp.dragged();
    painter.circle_filled(c, rad - zs(5.0), if active { PANEL_EDGE } else { BG });
    let dir = vec2(val_a.cos(), val_a.sin());
    painter.line_segment([c + dir * (rad - zs(11.0)), c + dir * (rad - zs(5.0))], stroke(2.5, TEXT));
    if !label.is_empty() {
        let text = if active { p.normalized_value_to_string(norm, true) } else { label.to_string() };
        painter.text(pos2(c.x, r.bottom() + zs(7.0)), Align2::CENTER_CENTER, text, font(11.0), if active { ACCENT } else { DIM });
    }
    resp.on_hover_text(format!("{}: {}", p.name(), p.normalized_value_to_string(norm, true)))
}

fn screen(ui: &Ui, r: Rect) {
    ui.painter().rect_filled(r, radius(4.0), SCREEN);
    ui.painter().rect_stroke(r, radius(4.0), stroke(1.0, PANEL_EDGE), StrokeKind::Inside);
}

fn trace(ui: &Ui, r: Rect, n: usize, f: impl Fn(f32) -> f32, color: Color32) {
    let pts: Vec<Pos2> = (0..=n).map(|i| {
        let x = i as f32 / n as f32;
        pos2(r.left() + x * r.width(), r.center().y - f(x).clamp(-1.0, 1.0) * r.height() / 2.0)
    }).collect();
    ui.painter().add(Shape::line(pts, stroke(1.5, color)));
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
    trace(ui, r.shrink(zs(4.0)), 96, |x| {
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
    if params.version.value() != FlVersion::Fl271 {
        // FL 3.5 / FL 6: cutoff index = (cutoff + adjust)/2, plus ENV/2 at the envelope peak
        let cut = params.ch_cut.value() + params.adj_cut.value();
        let base = (cut * 64) >> 7;
        let peak = (cut * 64 + params.env_amt.value() * 64) >> 7;
        let res = params.ch_res.value() + params.adj_res.value();
        let p = ui.painter();
        p.text(r.left_top() + zv(8.0, 6.0), Align2::LEFT_TOP, format!("{} → {} Hz", fmt_hz(cutoff_hz(base)), fmt_hz(cutoff_hz(peak.min(255)))),
               mono(12.5), ACCENT);
        let line2 = match params.filter_type.value() {
            FilterType::Off => "filter off".to_string(),
            _ => format!("res {:.0}%   (cutoff, then env peak)", res.clamp(0, 256) as f32 / 2.56),
        };
        p.text(r.left_top() + zv(8.0, 40.0), Align2::LEFT_TOP, line2, font(11.0), DIM);
        return;
    }
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
    p.text(r.left_top() + zv(8.0, 6.0), Align2::LEFT_TOP, line1, mono(12.5), ACCENT);
    p.text(r.left_top() + zv(8.0, 40.0), Align2::LEFT_TOP, line2, font(11.0), DIM);
}

fn lfo_preview(ui: &Ui, r: Rect, t: &Tables, params: &SynthParams) {
    screen(ui, r);
    let wave = &t.lfo[params.lfo_shape.value() as usize];
    let inner = Rect::from_min_max(r.min + zv(6.0, 6.0), pos2(r.left() + r.width() * 0.45, r.bottom() - zs(6.0)));
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
    p.text(pos2(r.left() + r.width() * 0.5, r.top() + zs(8.0)), Align2::LEFT_TOP, format!("{hz:.2} Hz"), mono(12.5), ACCENT);
    p.text(pos2(r.left() + r.width() * 0.5, r.top() + zs(40.0)), Align2::LEFT_TOP, format!("to {target}"), font(11.0), DIM);
}

/// The distortion curve the engine applies (|x| -> |y|, dry/wet mixed).
fn dist_preview(ui: &Ui, r: Rect, t: &Tables, params: &SynthParams) {
    screen(ui, r);
    let thr = params.dist_thres.value();
    if thr == 0 {
        ui.painter().text(r.center(), Align2::CENTER_CENTER, "off (THR 0)", font(11.0), DIM);
        return;
    }
    let inner = r.shrink(zs(5.0));
    let ty = if params.dist_type.value() == DistType::Soft { 0 } else { 1 };
    let tab = &t.dist[ty][(thr - 1) as usize];
    let a = params.dist_amount.value() as f32 / 128.0;
    let pts: Vec<Pos2> = (0..=48).map(|i| {
        let x = i as f32 / 48.0;
        let y = x * (1.0 - a) + tab[((x * 8191.75) as usize).min(8191)] as f32 / 32768.0 * a;
        pos2(inner.left() + x * inner.width(), inner.bottom() - y.clamp(0.0, 1.0) * inner.height())
    }).collect();
    ui.painter().add(Shape::line(pts, stroke(1.5, ACCENT)));
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
    let inner = r.shrink2(zv(5.0, 4.0));
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
    ui.painter().add(Shape::line(pts, stroke(1.5, ACCENT)));
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
