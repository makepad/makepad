//! makepad-example-raytrace — the progressive path tracer's standalone app:
//! the "Rendered" viewport + its settings panel, and the renderer's own
//! selftest / benchmark harness (`--selftest`, `--bench`, `--render`).
//!
//! ```text
//! makepad-example-raytrace [--scene cornell|glass|building|furnace|file.glb]
//!                           [--selftest DIR] [--bench DIR] [--remote]
//! ```

use makepad_raytrace::{RenderSettings, SceneInput};
use makepad_widgets::*;

mod view;
use view::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    let Row = View{ width: Fill height: Fit flow: Right spacing: 6 align: Align{y: 0.5} }
    let Knob = Slider{ width: Fill height: 24 min: 0.0 max: 1.0 }

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1440, 860)
                window.title: "makepad raytrace"
                body +: {
                    flow: Right
                    view := PtView{ width: Fill height: Fill }
                    panel := View{
                        width: 300 height: Fill flow: Down padding: 10 spacing: 6
                        show_bg: true
                        draw_bg +: { color: theme.color_bg_container }
                        H3{ text: "Render" }
                        Row{ Label{ text: "scene" } scene := DropDown{ width: Fill labels: ["cornell" "cornell glass" "building" "furnace" "glb"] } }
                        Row{ render_btn := Button{ text: "Render Image (F12)" } save_btn := Button{ text: "Save PNG" } }
                        Row{ Label{ width: 90 text: "target spp" } spp := Knob{ min: 16 max: 4096 step: 16 default: 1024 } }
                        Row{ Label{ width: 90 text: "scale" } scale := Knob{ min: 0.25 max: 2.0 step: 0.25 default: 1.0 } }
                        Row{ Label{ width: 90 text: "budget ms" } budget := Knob{ min: 4.0 max: 50.0 step: 1.0 default: 20.0 } }
                        Hr{}
                        H3{ text: "Lens" }
                        Row{ Label{ width: 90 text: "f-stop" } fstop := Knob{ min: 0.7 max: 22.0 step: 0.1 default: 8.0 } }
                        Row{ Label{ width: 90 text: "focus dist" } focus := Knob{ min: 0.1 max: 200.0 step: 0.1 default: 4.0 } }
                        Row{ Label{ width: 90 text: "bokeh scale" } bokeh := Knob{ min: 1.0 max: 20.0 step: 0.5 default: 4.0 } }
                        Row{ Label{ width: 90 text: "blades" } blades := Knob{ min: 0 max: 9 step: 1 default: 0 } }
                        Label{ text: "click the image to focus there" }
                        Hr{}
                        H3{ text: "Light" }
                        Row{ Label{ width: 90 text: "sun time" } sun_time := Knob{ min: 5.0 max: 21.0 step: 0.1 default: 10.5 } }
                        Row{ Label{ width: 90 text: "turbidity" } turbidity := Knob{ min: 1.5 max: 8.0 step: 0.1 default: 2.5 } }
                        Row{ Label{ width: 90 text: "exposure" } exposure := Knob{ min: 0.05 max: 8.0 step: 0.05 default: 1.0 } }
                        Hr{}
                        H3{ text: "Integrator" }
                        Row{ denoise := CheckBox{ text: "denoise" } adaptive := CheckBox{ text: "adaptive" } hybrid := CheckBox{ text: "hybrid primary" } }
                        Row{ Label{ width: 90 text: "bounces" } bounces := Knob{ min: 1 max: 16 step: 1 default: 8 } }
                        Row{ Label{ width: 90 text: "view" } view_mode := DropDown{ width: Fill labels: ["image" "spp heatmap" "noise" "normals" "albedo"] } }
                        Hr{}
                        status := Label{ width: Fill text: "..." }
                        status2 := Label{ width: Fill text: "" }
                    }
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    settings: RenderSettings,
    #[rust]
    scale: f64,
    #[rust]
    pending_save: bool,
    #[rust]
    out_dir: Option<std::path::PathBuf>,
}

impl App {
    fn with_view<R>(&self, cx: &mut Cx, f: impl FnOnce(&mut Cx, &mut PtView) -> R) -> Option<R> {
        let w = self.ui.widget(cx, ids!(view));
        let mut b = w.borrow_mut::<PtView>()?;
        Some(f(cx, &mut b))
    }

    fn push_settings(&mut self, cx: &mut Cx) {
        let s = self.settings.clone();
        let scale = self.scale;
        self.with_view(cx, |cx, v| {
            v.settings = s;
            v.scale = scale;
            v.apply(cx);
        });
    }

    fn load_scene(&mut self, cx: &mut Cx, scene: SceneInput) {
        let focus = scene.camera.focus_dist as f64;
        let fstop = scene.camera.f_stop as f64;
        let bokeh = scene.camera.bokeh_scale as f64;
        self.ui.slider(cx, ids!(focus)).set_value(cx, focus);
        self.ui.slider(cx, ids!(fstop)).set_value(cx, fstop);
        self.ui.slider(cx, ids!(bokeh)).set_value(cx, bokeh);
        self.with_view(cx, |cx, v| v.load(cx, scene));
        self.push_settings(cx);
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.settings = RenderSettings::default();
        self.scale = 1.0;
        let args = Args::parse();
        self.out_dir = args.out_dir.clone();
        self.ui.check_box(cx, ids!(hybrid)).set_active(cx, false, Animate::No);
        self.ui.check_box(cx, ids!(adaptive)).set_active(cx, true, Animate::No);
        let scene = args.scene();
        self.load_scene(cx, scene);
        self.with_view(cx, |_cx, v| {
            v.mode = args.mode.clone();
            v.out_dir = args.out_dir.clone();
            v.bench_spp = args.spp;
            v.render_seconds = args.seconds;
        });
    }

    fn handle_actions(&mut self, cx: &mut Cx, actions: &Actions) {
        let mut dirty = false;
        if let Some(i) = self.ui.drop_down(cx, ids!(scene)).selected(actions) {
            let scene = match i {
                0 => SceneInput::cornell_box(false),
                1 => SceneInput::cornell_box(true),
                2 => makepad_raytrace::building::building(8, 10),
                3 => SceneInput::furnace(),
                _ => Args::parse().glb().unwrap_or_else(|| SceneInput::cornell_box(false)),
            };
            self.load_scene(cx, scene);
        }
        if self.ui.button(cx, ids!(render_btn)).clicked(actions) {
            self.with_view(cx, |cx, v| v.render_image(cx));
        }
        if self.ui.button(cx, ids!(save_btn)).clicked(actions) {
            let dir = self.out_dir.clone();
            self.with_view(cx, |cx, v| v.save_png(cx, dir));
        }
        if let Some(v) = self.ui.slider(cx, ids!(spp)).slided(actions) {
            self.settings.target_spp = v as u32;
            dirty = true;
        }
        if let Some(v) = self.ui.slider(cx, ids!(scale)).slided(actions) {
            self.scale = v;
            dirty = true;
        }
        if let Some(v) = self.ui.slider(cx, ids!(budget)).slided(actions) {
            self.settings.frame_budget = v / 1000.0;
            dirty = true;
        }
        if let Some(v) = self.ui.slider(cx, ids!(bounces)).slided(actions) {
            self.settings.max_bounces = v as u32;
            self.settings.max_diffuse = (v as u32).min(4).max(1);
            dirty = true;
        }
        if let Some(v) = self.ui.slider(cx, ids!(exposure)).slided(actions) {
            self.settings.exposure = v as f32;
            dirty = true;
        }
        if let Some(b) = self.ui.check_box(cx, ids!(denoise)).changed(actions) {
            self.settings.denoise = b;
            dirty = true;
        }
        if let Some(b) = self.ui.check_box(cx, ids!(adaptive)).changed(actions) {
            self.settings.adaptive_min = if b { 64 } else { 0 };
            dirty = true;
        }
        if let Some(b) = self.ui.check_box(cx, ids!(hybrid)).changed(actions) {
            self.settings.hybrid_primary = b;
            dirty = true;
        }
        if let Some(i) = self.ui.drop_down(cx, ids!(view_mode)).selected(actions) {
            match i {
                0 => {
                    self.settings.view_mode = 0;
                    self.settings.debug_mode = 0;
                }
                1 => {
                    self.settings.view_mode = 1;
                    self.settings.debug_mode = 0;
                }
                2 => {
                    self.settings.view_mode = 2;
                    self.settings.debug_mode = 0;
                }
                3 => {
                    self.settings.view_mode = 0;
                    self.settings.debug_mode = 1;
                }
                _ => {
                    self.settings.view_mode = 0;
                    self.settings.debug_mode = 2;
                }
            }
            dirty = true;
        }
        // Lens + light go straight to the view's camera/sun.
        let lens = (
            self.ui.slider(cx, ids!(fstop)).slided(actions),
            self.ui.slider(cx, ids!(focus)).slided(actions),
            self.ui.slider(cx, ids!(bokeh)).slided(actions),
            self.ui.slider(cx, ids!(blades)).slided(actions),
        );
        let light = (
            self.ui.slider(cx, ids!(sun_time)).slided(actions),
            self.ui.slider(cx, ids!(turbidity)).slided(actions),
        );
        if lens.0.is_some() || lens.1.is_some() || lens.2.is_some() || lens.3.is_some() {
            self.with_view(cx, |cx, v| {
                if let Some(t) = v.tracer.as_mut() {
                    let mut cam = t.camera().clone();
                    if let Some(f) = lens.0 {
                        cam.f_stop = f as f32;
                    }
                    if let Some(f) = lens.1 {
                        cam.focus_dist = f as f32;
                    }
                    if let Some(f) = lens.2 {
                        cam.bokeh_scale = f as f32;
                    }
                    if let Some(f) = lens.3 {
                        cam.blades = f as u32;
                    }
                    t.set_camera(cam);
                }
                v.redraw(cx);
            });
        }
        if light.0.is_some() || light.1.is_some() {
            self.with_view(cx, |cx, v| {
                if let Some(t) = light.0 {
                    v.sun_time = t as f32;
                }
                if let Some(t) = light.1 {
                    v.sun.turbidity = t as f32;
                }
                v.update_sun();
                v.redraw(cx);
            });
        }
        if dirty {
            self.push_settings(cx);
        }
        // The focus slider follows click-to-focus.
        if let Some(Some(f)) = self.with_view(cx, |_cx, v| v.focus_changed.take()) {
            self.ui.slider(cx, ids!(focus)).set_value(cx, f as f64);
        }
    }

    fn handle_key_down(&mut self, cx: &mut Cx, e: &KeyEvent) {
        if e.key_code == KeyCode::F12 {
            self.with_view(cx, |cx, v| v.render_image(cx));
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_raytrace::script_mod(vm);
        crate::view::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
        if let Event::NextFrame(_) = event {
            let (s1, s2) = self
                .with_view(cx, |_cx, v| (v.status_line(), v.status_line2()))
                .unwrap_or_default();
            self.ui.label(cx, ids!(status)).set_text(cx, &s1);
            self.ui.label(cx, ids!(status2)).set_text(cx, &s2);
        }
    }
}

/// Command line.
#[derive(Clone, Debug, Default)]
pub struct Args {
    pub scene: String,
    pub mode: RunMode,
    pub out_dir: Option<std::path::PathBuf>,
    pub spp: u32,
    /// `--seconds N`: a `--render` stops after N seconds even if the spp
    /// target is not reached (the soak test).
    pub seconds: f64,
    /// `--zup`: re-express the scene as Z-up (diagnostic).
    pub zup: bool,
}

impl Args {
    pub fn parse() -> Args {
        let mut a = Args { scene: "cornell".into(), spp: 256, ..Default::default() };
        let mut it = std::env::args().skip(1);
        while let Some(arg) = it.next() {
            match arg.as_str() {
                "--scene" => a.scene = it.next().unwrap_or_default(),
                "--selftest" => {
                    a.mode = RunMode::Selftest;
                    a.out_dir = it.next().map(Into::into);
                }
                "--bench" => {
                    a.mode = RunMode::Bench;
                    a.out_dir = it.next().map(Into::into);
                }
                "--render" => {
                    a.mode = RunMode::RenderOnce;
                    a.out_dir = it.next().map(Into::into);
                }
                "--spp" => a.spp = it.next().and_then(|s| s.parse().ok()).unwrap_or(256),
                "--seconds" => a.seconds = it.next().and_then(|s| s.parse().ok()).unwrap_or(0.0),
                "--zup" => a.zup = true,
                "--out" => a.out_dir = it.next().map(Into::into),
                _ => {}
            }
        }
        a
    }
    pub fn glb(&self) -> Option<SceneInput> {
        if self.scene.ends_with(".glb") || self.scene.ends_with(".gltf") {
            match makepad_raytrace::glb::load_glb(std::path::Path::new(&self.scene)) {
                Ok(s) => return Some(s),
                Err(e) => log!("glb load failed: {e}"),
            }
        }
        None
    }
    pub fn scene(&self) -> SceneInput {
        let mut s = match self.scene.as_str() {
            "cornell" => SceneInput::cornell_box(false),
            "glass" => SceneInput::cornell_box(true),
            "building" => makepad_raytrace::building::building(8, 10),
            "furnace" => SceneInput::furnace(),
            _ => self.glb().unwrap_or_else(|| SceneInput::cornell_box(false)),
        };
        if self.zup {
            s.to_z_up();
        }
        s
    }
}
