//! Night skyline generator: a transparent, horizontally tiling 360 degree band of night
//! silhouettes (mountains with forests, a city, props) whose lights are emissive HDR, for
//! compositing in front of a sky.
//!
//! The app: the full strip and a detail view (drag to pan, wheel to zoom) over a sky gradient,
//! and a panel with the seed, the exports and every parameter as a grouped slider.
//!
//! Flags: `--params FILE` loads the web page's saved parameters; `--view-px N` renders both views
//! N pixels wide whatever the window; `--bench` times the strip (warm-up, then samples, logged), then quits;
//! `--dump-layout FILE` writes the zones and the strip's columns as JSON (checks against the page).
pub use makepad_widgets;

use makepad_widgets::*;

mod city;
mod export;
mod params;
mod post;
mod props;
mod render;
mod rng;
mod shaders;
mod terrain;
mod zones;

use export::{Export, ExportKind};
use makepad_widgets::makepad_platform::gpu_frame_timer;
use params::{Params, COUNT, P, PARAMS};
use render::{Skyline, ViewSpec};
use shaders::DrawSkyBackdrop;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.widgets.SkylineViewBase = #(SkylineView::register_widget(vm))
    mod.widgets.SkylineView = set_type_default() do mod.widgets.SkylineViewBase{
        width: Fill
        height: Fit
        draw_sky: mod.draw.DrawSkyBackdrop{}
        sky: mod.draw.Skyline{}
    }

    let Caption = Label{
        draw_text +: {color: #x8c95aa text_style +: {font_size: 8}}
        text: ""
    }

    mod.widgets.SkyParamPanelBase = #(ParamPanel::register_widget(vm))
    mod.widgets.SkyParamPanel = set_type_default() do mod.widgets.SkyParamPanelBase{
        width: Fill
        height: Fill
        flow: Down
        param_list := FlatList{
            width: Fill
            height: Fill
            flow: Down
            Header := Caption{margin: Inset{top: 10 bottom: 2}}
            Row := Slider{width: Fill text: "" min: 0.0 max: 1.0 step: 0.01}
        }
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1500, 900)
                pass.clear_color: #x0c1018
                body +: {
                    flow: Right
                    views := ScrollYView{
                        width: Fill
                        height: Fill
                        flow: Down
                        spacing: 4
                        padding: Inset{left: 12 right: 12 top: 10 bottom: 10}
                        Caption{text: "FULL 360\u{b0} STRIP"}
                        strip := mod.widgets.SkylineView{}
                        View{height: 8}
                        detail_label := Caption{text: "DETAIL"}
                        detail := mod.widgets.SkylineView{detail: true}
                    }
                    panel := SolidView{
                        width: 300
                        height: Fill
                        flow: Down
                        spacing: 6
                        padding: Inset{left: 12 right: 12 top: 12 bottom: 12}
                        draw_bg +: {color: #x131825}
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 6
                            align: Align{y: 0.5}
                            Label{text: "Seed" width: 50}
                            seed := TextInput{width: Fill text: "7"}
                            dice := Button{text: "Dice"}
                        }
                        Caption{text: "EXPORT (FULL 360\u{b0} STRIP)" margin: Inset{top: 8}}
                        View{
                            width: Fill
                            height: Fit
                            flow: Right
                            spacing: 6
                            align: Align{y: 0.5}
                            export_w := DropDown{width: 80 labels: ["2048" "4096" "8192"] selected_item: 1}
                            export_png := Button{text: "Save PNG"}
                            export_exr := Button{text: "Save EXR"}
                        }
                        status := Label{width: Fill text: "" draw_text +: {color: #x8c95aa text_style +: {font_size: 8}}}
                        params := mod.widgets.SkyParamPanel{}
                    }
                }
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
pub enum SkylineViewAction {
    #[default]
    None,
    /// The detail view was dragged to this pan.
    Pan(f64),
    /// The detail view was zoomed to this width.
    Zoom(f64),
    /// Export progress.
    Status(String),
}

/// One view of the skyline: the strip, or the detail view at the parameters' pan and zoom.
#[derive(Script, ScriptHook, Widget)]
pub struct SkylineView {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_sky: DrawSkyBackdrop,
    #[live]
    detail: bool,
    #[live]
    sky: Skyline,
    #[rust]
    params: Params,
    /// Render width forced by `--view-px`.
    #[rust]
    fixed_px: Option<usize>,
    #[rust]
    rect: Rect,
    #[rust]
    drag: Option<(f64, f64)>,
    #[rust]
    export: Export,
}

impl SkylineView {
    fn set_params(&mut self, cx: &mut Cx, p: &Params) {
        if self.params != *p {
            self.params = p.clone();
            self.redraw(cx);
        }
    }

    fn view(&self, w: usize) -> ViewSpec {
        if self.detail {
            ViewSpec::detail(&self.params, w)
        } else {
            ViewSpec::strip(&self.params, w)
        }
    }
}

impl Widget for SkylineView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Some(msg) = self.export.handle_event(cx, event) {
            cx.widget_action(self.uid, SkylineViewAction::Status(msg));
        }
        if !self.detail {
            return;
        }
        match event.hits(cx, self.draw_sky.area()) {
            Hit::FingerDown(fe) => self.drag = Some((fe.abs.x, self.params[P::Pan])),
            Hit::FingerMove(fe) => {
                if let Some((x0, pan0)) = self.drag {
                    let pan = rng::fract(pan0 - (fe.abs.x - x0) / self.rect.size.x.max(1.0) * self.params[P::Zoom]);
                    cx.widget_action(self.uid, SkylineViewAction::Pan(pan));
                }
            }
            Hit::FingerUp(_) => self.drag = None,
            Hit::FingerScroll(fs) => {
                let zoom = (self.params[P::Zoom] * (1.0 + 0.002 * fs.scroll.y)).clamp(0.05, 1.0);
                cx.widget_action(self.uid, SkylineViewAction::Zoom(zoom));
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        let dpi = cx.current_dpi_factor();
        let avail = cx.turtle().max_width(walk).unwrap_or(800.0).max(64.0);
        let span = if self.detail { self.params[P::Zoom] } else { 1.0 };
        let aspect = self.params[P::Vfov] / (span * 360.0);
        // the page's budget: at most 4096 px wide and 1.6 (strip) / 2 (detail) megapixels
        let budget = if self.detail { 2.0e6 } else { 1.6e6 };
        let w = self.fixed_px.unwrap_or((avail * dpi).round() as usize).min(4096).min((budget / aspect).sqrt() as usize).max(64);
        let view = self.view(w);
        let (lw, lh) = match self.fixed_px {
            Some(_) => (w as f64 / dpi, view.h as f64 / dpi),
            None => (avail, avail * view.h as f64 / view.w as f64),
        };
        let rect = cx.walk_turtle(Walk { width: Size::Fixed(lw), height: Size::Fixed(lh), ..walk });
        self.rect = rect;
        self.draw_sky.draw_vars.set_uniform(cx, live_id!(horizon), &[view.horizon as f32]);
        self.draw_sky.draw_abs(cx, rect);
        self.sky.draw_view(cx, rect, &view, &self.params, true);
        if !self.detail {
            self.export.draw(cx, &self.params);
        }
        DrawStep::done()
    }
}

/// The parameter sliders, built from the table in `params.rs`: a caption per group, a slider per row.
#[derive(Script, ScriptHook, Widget)]
pub struct ParamPanel {
    #[deref]
    view: View,
    #[rust]
    values: Vec<f64>,
    #[rust]
    configured: Vec<bool>,
}

fn row_id(i: usize) -> LiveId {
    LiveId::from_str_num("param", i as u64)
}

fn header_id(i: usize) -> LiveId {
    LiveId::from_str_num("group", i as u64)
}

impl ParamPanel {
    fn set_values(&mut self, cx: &mut Cx, p: &Params) {
        let list = self.view.flat_list(cx, ids!(param_list));
        if self.configured.len() != COUNT {
            self.configured = vec![false; COUNT];
        }
        for i in 0..COUNT {
            if self.values.get(i) != Some(&p.v[i]) && self.configured[i] {
                if let Some(item) = list.item(cx, row_id(i), id!(Row)) {
                    item.as_slider().set_value(cx, p.v[i]);
                }
            }
        }
        self.values = p.v.to_vec();
    }

    /// Sliders moved by the user: (parameter index, value).
    fn changed(&self, cx: &mut Cx, actions: &Actions) -> Vec<(usize, f64)> {
        let list = self.view.flat_list(cx, ids!(param_list));
        let mut out = Vec::new();
        for (id, item) in list.items_with_actions(actions) {
            if let Some(i) = (0..COUNT).find(|i| row_id(*i) == id) {
                if let Some(v) = item.as_slider().slided(actions) {
                    out.push((i, v));
                }
            }
        }
        out
    }
}

impl Widget for ParamPanel {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, scope: &mut Scope) {
        self.view.handle_event(cx, event, scope);
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, scope: &mut Scope, walk: Walk) -> DrawStep {
        if self.configured.len() != COUNT {
            self.configured = vec![false; COUNT];
        }
        while let Some(item) = self.view.draw_walk(cx, scope, walk).step() {
            let Some(mut list) = item.borrow_mut::<FlatList>() else { continue };
            let mut group = "";
            for (i, d) in PARAMS.iter().enumerate() {
                if d.group != group {
                    group = d.group;
                    if let Some(h) = list.item(cx, header_id(i), id!(Header)) {
                        h.as_label().set_text(cx, &group.to_uppercase());
                        h.draw_all(cx, scope);
                    }
                }
                let Some(mut row) = list.item(cx, row_id(i), id!(Row)) else { continue };
                if !self.configured[i] {
                    let precision = (-d.step.log10()).ceil().max(0.0);
                    let label = d.label.to_string();
                    script_apply_eval!(cx, row, {
                        text: #(label)
                        min: #(d.min)
                        max: #(d.max)
                        step: #(d.step)
                        default: #(d.default)
                        precision: #(precision)
                    });
                    row.as_slider().set_value(cx, self.values.get(i).copied().unwrap_or(d.default));
                    self.configured[i] = true;
                }
                row.draw_all(cx, scope);
            }
        }
        DrawStep::done()
    }
}

/// `--bench`: the strip timed end to end (see `render::Probe`). Ticks alternate a draw that
/// records every pass again with a baseline draw that only reads back; after a warm-up, first with
/// the CPU inputs cached ("passes"), then rebuilt ("cold": zones and columns too).
struct Bench {
    tick: usize,
    probe: render::Probe,
}

const BENCH_WARM: usize = 60;
const BENCH_PAIRS: usize = 40;
const BENCH_REPEAT: usize = 41;

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    params: Params,
    #[rust]
    timer: Timer,
    #[rust]
    bench: Option<Bench>,
    #[rust]
    bench_timer: Timer,
}

impl App {
    fn view_mut<R>(&self, cx: &mut Cx, detail: bool, f: impl FnOnce(&mut SkylineView, &mut Cx) -> R) -> Option<R> {
        let w = self.ui.widget(cx, if detail { ids!(detail) } else { ids!(strip) });
        let mut v = w.borrow_mut::<SkylineView>()?;
        Some(f(&mut v, cx))
    }

    /// Pushes the parameters to both views, the panel and the captions.
    fn apply(&mut self, cx: &mut Cx) {
        let p = self.params.clone();
        self.view_mut(cx, false, |v, cx| v.set_params(cx, &p));
        self.view_mut(cx, true, |v, cx| v.set_params(cx, &p));
        let panel = self.ui.widget(cx, ids!(params));
        if let Some(mut panel) = panel.borrow_mut::<ParamPanel>() {
            panel.set_values(cx, &p);
        }
        let label = format!("DETAIL  {:.0}\u{b0} WIDE, CENTRED {:.0}\u{b0}", p[P::Zoom] * 360.0, p[P::Pan] * 360.0);
        self.ui.label(cx, ids!(detail_label)).set_text(cx, &label);
    }

    fn set_param(&mut self, cx: &mut Cx, p: P, x: f64) {
        if self.params.set(p as usize, x) {
            self.apply(cx);
        }
    }

    /// Hands every finished frame's GPU time of each view's scene pass to that view.
    fn poll_gpu(&mut self, cx: &mut Cx) {
        let frames = gpu_frame_timer::take_frames();
        for detail in [false, true] {
            self.view_mut(cx, detail, |v, _| {
                for f in &frames {
                    let ms: f32 = f.passes.iter().filter(|(n, _)| n == v.sky.scene_pass_name()).map(|(_, ms)| *ms).sum();
                    if f.passes.iter().any(|(n, _)| n == v.sky.scene_pass_name()) {
                        v.sky.add_gpu_sample(ms as f64);
                    }
                }
            });
        }
    }

    fn status(&mut self, cx: &mut Cx) {
        self.poll_gpu(cx);
        let mut parts = Vec::new();
        for (detail, name) in [(false, "Strip"), (true, "Detail")] {
            if let Some(s) = self.view_mut(cx, detail, |v, _| {
                v.sky.stats.gpu_samples.clear();
                v.sky.stats.clone()
            }) {
                let gpu = s.gpu_ms.map(|g| format!("{g:.2}")).unwrap_or_else(|| "-".into());
                parts.push(format!(
                    "{name} {}x{}: zones {:.1}, columns {:.1}, record {:.2}, GPU {gpu} ms; {} renders",
                    s.w, s.h, s.zones_ms, s.cols_ms, s.record_ms, s.renders
                ));
            }
        }
        self.ui.label(cx, ids!(status)).set_text(cx, &parts.join("\n"));
    }

    fn bench_step(&mut self, cx: &mut Cx) {
        let Some(b) = &mut self.bench else { return };
        b.tick += 1;
        let t = b.tick;
        // tag 0 warm-up, 1 passes, 2 passes with the scene list drawn BENCH_REPEAT times, 3 cold, 4 done;
        // odd ticks after the warm-up are baselines
        let tag = if t <= BENCH_WARM { 0 } else { (1 + (t - BENCH_WARM - 1) / (2 * BENCH_PAIRS)).min(4) as u32 };
        let render = t <= BENCH_WARM || (t - BENCH_WARM) % 2 == 1;
        let probe = b.probe.clone();
        if tag < 4 {
            self.view_mut(cx, false, |v, cx| {
                let repeat = if tag == 2 { BENCH_REPEAT } else { 1 };
                v.sky.probe = Some(render::Probe { tag, repeat, samples: probe.samples.clone() });
                if render {
                    v.sky.invalidate(tag == 3);
                }
                v.redraw(cx);
            });
            return;
        }
        if t < BENCH_WARM + 6 * BENCH_PAIRS + 20 {
            return; // the last reads are still on their way
        }
        let (w, h) = self.view_mut(cx, false, |v, _| {
            v.sky.probe = None;
            (v.sky.stats.w, v.sky.stats.h)
        }).unwrap_or_default();
        let samples = probe.samples.lock().unwrap().clone();
        let stat = |tag: u32, rendered: bool| {
            let mut v: Vec<f64> = samples.iter().filter(|s| s.0 == tag && s.1 == rendered).map(|s| s.2).collect();
            v.sort_by(|a, b| a.total_cmp(b));
            if v.is_empty() { (f64::NAN, f64::NAN, 0) } else { (v[v.len() / 2], v[0], v.len()) }
        };
        let repeated = format!("passes, scene list x {BENCH_REPEAT}");
        for (tag, name) in [(1, "passes"), (2, repeated.as_str()), (3, "cold")] {
            let (r, rmin, rn) = stat(tag, true);
            let (base, bmin, bn) = stat(tag, false);
            log!(
                "skyline bench: strip {w}x{h} {name}: draw to read-back median {r:.2} ms (min {rmin:.2}, n {rn}), baseline {base:.2} ms (min {bmin:.2}, n {bn}), difference {:.2} ms",
                r - base
            );
        }
        let (one, two) = (stat(1, true).0, stat(2, true).0);
        log!("skyline bench: each extra scene list (its recording and its GPU work) {:.3} ms", (two - one) / (BENCH_REPEAT - 1) as f64);
        self.bench = None;
        cx.stop_timer(self.bench_timer);
        log!("skyline bench: done");
        cx.quit();
    }

    fn dump_layout(&self, path: &str) {
        let p = &self.params;
        let z = zones::Zones::new(p);
        let mut s = String::from("{\"zones\":{");
        let named = z.named();
        for (i, (k, v)) in named.iter().enumerate() {
            s += &format!("{}\"{k}\":[{},{},{},{}]", if i > 0 { "," } else { "" }, v[0], v[1], v[2], v[3]);
        }
        let view = ViewSpec::strip(p, 1600);
        let m = terrain::margin(p, view.span, view.w);
        let cols = terrain::columns(&z, p, view.u0, view.span, view.w, m);
        s += &format!("}},\"m\":{m},\"columns\":[");
        for (i, c) in cols.iter().enumerate() {
            s += &format!("{}{},{},{},{}", if i > 0 { "," } else { "" }, c[0], c[1], c[2], c[3]);
        }
        s += "]}";
        match std::fs::write(path, s) {
            Ok(_) => log!("skyline: layout written to {path}"),
            Err(e) => error!("skyline: could not write {path}: {e}"),
        }
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let args: Vec<String> = std::env::args().collect();
        let arg = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
        if let Some(path) = arg("--params") {
            match std::fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|t| self.params.load_web_json(&t)) {
                Ok(n) => log!("skyline: loaded {n} settings from {path}"),
                Err(e) => error!("skyline: could not load {path}: {e}"),
            }
        }
        if let Some(px) = arg("--view-px").and_then(|s| s.parse::<usize>().ok()) {
            for detail in [false, true] {
                self.view_mut(cx, detail, |v, _| v.fixed_px = Some(px));
            }
        }
        if let Some(path) = arg("--dump-layout") {
            self.dump_layout(&path);
        }
        self.ui.text_input(cx, ids!(seed)).set_text(cx, &self.params.seed.to_string());
        self.apply(cx);
        gpu_frame_timer::set_enabled(true);
        self.timer = cx.start_interval(0.5);
        if args.iter().any(|a| a == "--bench") {
            self.bench = Some(Bench { tick: 0, probe: render::Probe::default() });
            self.bench_timer = cx.start_interval(0.05);
        }
    }

    fn handle_timer(&mut self, cx: &mut Cx, e: &TimerEvent) {
        if self.timer.is_timer(e).is_some() {
            self.status(cx);
        }
        if self.bench_timer.is_timer(e).is_some() {
            self.bench_step(cx);
        }
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let panel = self.ui.widget(cx, ids!(params));
        let changed = panel.borrow::<ParamPanel>().map(|p| p.changed(cx, actions)).unwrap_or_default();
        let mut any = false;
        for (i, v) in changed {
            any |= self.params.set(i, v);
        }
        if any {
            self.apply(cx);
        }
        if let Some(t) = self.ui.text_input(cx, ids!(seed)).changed(actions) {
            if let Ok(seed) = t.trim().parse::<u32>() {
                if self.params.set_seed(seed) {
                    self.apply(cx);
                }
            }
        }
        if self.ui.button(cx, ids!(dice)).clicked(actions) {
            self.params.set_seed(Params::random_seed());
            self.ui.text_input(cx, ids!(seed)).set_text(cx, &self.params.seed.to_string());
            self.apply(cx);
        }
        for detail in [false, true] {
            let uid = self.ui.widget(cx, if detail { ids!(detail) } else { ids!(strip) }).widget_uid();
            if let Some(item) = actions.find_widget_action(uid) {
                match item.cast::<SkylineViewAction>() {
                    SkylineViewAction::Pan(pan) => self.set_param(cx, P::Pan, pan),
                    SkylineViewAction::Zoom(zoom) => self.set_param(cx, P::Zoom, zoom),
                    SkylineViewAction::Status(msg) => self.ui.label(cx, ids!(status)).set_text(cx, &msg),
                    SkylineViewAction::None => {}
                }
            }
        }
        for (button, kind) in [(ids!(export_png), ExportKind::Png), (ids!(export_exr), ExportKind::Exr)] {
            if self.ui.button(cx, button).clicked(actions) {
                let width = [2048, 4096, 8192][self.ui.drop_down(cx, ids!(export_w)).selected_item().min(2)];
                let p = self.params.clone();
                let msg = self
                    .view_mut(cx, false, |v, cx| v.export.start(cx, kind, width, &p))
                    .unwrap_or_else(|| Err("no strip view".into()))
                    .unwrap_or_else(|e| e);
                self.ui.label(cx, ids!(status)).set_text(cx, &msg);
            }
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        #[cfg(feature = "tweaker")]
        makepad_widgets_tweaker::link(vm);
        crate::makepad_widgets::script_mod(vm);
        crate::shaders::script_mod(vm);
        crate::terrain::script_mod(vm);
        crate::city::script_mod(vm);
        crate::props::kit::script_mod(vm);
        crate::props::script_mod(vm);
        crate::post::script_mod(vm);
        crate::render::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
