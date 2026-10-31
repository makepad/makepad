//! Verification rig for the MilkDrop renderer: plays preset files from the
//! command line with synthetic music, stepping on a key or a timer, and logs
//! each preset's load report and the frame's CPU cost.
//!
//! ```text
//! cargo run -p makepad-milkdrop --release --example milkdrop_render_check -- \
//!     [--seconds=4] [--pixels=1280x720] <preset.milk | dir>...
//! ```
//!
//! N / P step to the next / previous preset. Run it with `--remote` to grab
//! frames; `MAKEPAD_TRACE=present,gpu.pass` adds GPU time per pass.

pub use makepad_widgets;

use makepad_milkdrop::MilkdropViewWidgetRefExt;
use makepad_widgets::*;
use std::path::PathBuf;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*
    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "MilkDrop render check"
                window.inner_size: vec2(1280, 720)
                body +: {
                    flow: Overlay
                    milk := MilkdropView{}
                    status := Label{
                        margin: 8
                        text: ""
                        draw_text.text_style.font_size: 10
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
    presets: Vec<PathBuf>,
    #[rust]
    index: usize,
    #[rust]
    seconds_per_preset: f64,
    #[rust]
    since_switch: f64,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    last_time: Option<f64>,
    #[rust]
    synth: Synth,
    #[rust]
    frames: u64,
    #[rust]
    cpu_sum: f64,
    #[rust]
    cpu_max: f64,
    #[rust]
    reported: bool,
    /// Wanted render size; the view's resolution scale is set to reach it.
    #[rust]
    pixels: Option<(f64, f64)>,
    #[rust(1.0)]
    scale: f64,
    /// Sums of the engine's stage times since the preset loaded.
    #[rust]
    stages: [f64; 5],
}

/// A deterministic groove: kick, hats, bass line and a pad chord, so the
/// band levels and waveform move like music.
#[derive(Default)]
struct Synth {
    t: f64,
    noise: u32,
}

impl Synth {
    fn render(&mut self, rate: f64, n: usize, out: &mut Vec<f32>) {
        out.clear();
        for _ in 0..n {
            let t = self.t;
            let beat = t * 2.0; // 120 bpm
            let phase = beat.fract();
            let kick = (-phase * 18.0).exp() * (std::f64::consts::TAU * (50.0 + 90.0 * (-phase * 30.0).exp()) * t).sin();
            self.noise ^= self.noise << 13;
            self.noise ^= self.noise >> 17;
            self.noise ^= self.noise << 5;
            let white = (self.noise as f64 / u32::MAX as f64) * 2.0 - 1.0;
            let hat_phase = (beat * 2.0 + 0.5).fract();
            let hat = white * (-hat_phase * 40.0).exp() * 0.25;
            let bar = (beat / 4.0).floor() as i64;
            let root = [55.0, 55.0 * 1.335, 55.0 * 1.498, 55.0 * 1.26][(bar.rem_euclid(4)) as usize];
            let bass = (std::f64::consts::TAU * root * t).sin().signum() * 0.18 * (1.0 - phase * 0.5);
            let pad = [1.0, 1.26, 1.498]
                .iter()
                .map(|k| (std::f64::consts::TAU * root * 4.0 * k * t).sin())
                .sum::<f64>()
                * 0.05;
            out.push((kick * 0.7 + hat + bass + pad) as f32);
            self.t += 1.0 / rate;
        }
    }
}

impl App {
    fn load_current(&mut self, cx: &mut Cx) {
        let Some(path) = self.presets.get(self.index).cloned() else { return };
        self.ui.milkdrop_view(cx, ids!(milk)).load_preset(cx, &path);
        self.since_switch = 0.0;
        self.reported = false;
        self.frames = 0;
        self.cpu_sum = 0.0;
        self.cpu_max = 0.0;
        self.stages = [0.0; 5];
        log!("milkdrop: [{}/{}] {}", self.index + 1, self.presets.len(), path.display());
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        self.seconds_per_preset = 0.0;
        for arg in std::env::args().skip(1) {
            if let Some(s) = arg.strip_prefix("--seconds=") {
                self.seconds_per_preset = s.parse().unwrap_or(0.0);
            } else if let Some((w, h)) = arg.strip_prefix("--pixels=").and_then(|s| s.split_once('x')) {
                self.pixels = w.parse().ok().zip(h.parse().ok());
            } else if arg.starts_with("--") {
                continue;
            } else {
                let p = PathBuf::from(&arg);
                if p.is_dir() {
                    let mut found = Vec::new();
                    let mut stack = vec![p];
                    while let Some(d) = stack.pop() {
                        for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                            let q = e.path();
                            if q.is_dir() {
                                stack.push(q);
                            } else if q.extension().is_some_and(|x| x == "milk") {
                                found.push(q);
                            }
                        }
                    }
                    found.sort();
                    self.presets.extend(found);
                } else {
                    self.presets.push(p);
                }
            }
        }
        self.synth.noise = 0x1234_5678;
        self.ui.milkdrop_view(cx, ids!(milk)).set_playing(cx, true);
        self.load_current(cx);
        self.next_frame = cx.new_next_frame();
    }

    fn handle_key_down(&mut self, cx: &mut Cx, e: &KeyEvent) {
        match e.key_code {
            KeyCode::KeyN => {
                self.index = (self.index + 1) % self.presets.len().max(1);
                self.load_current(cx);
            }
            KeyCode::KeyP => {
                self.index = (self.index + self.presets.len().max(1) - 1) % self.presets.len().max(1);
                self.load_current(cx);
            }
            _ => {}
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_milkdrop::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        if let Some(ne) = self.next_frame.is_event(event) {
            let dt = self.last_time.replace(ne.time).map(|t| (ne.time - t).clamp(0.0, 0.1)).unwrap_or(0.0);
            let milk = self.ui.milkdrop_view(cx, ids!(milk));
            let (rw, rh) = milk.render_size();
            if let (Some((w, _)), true) = (self.pixels, rw > 0) {
                // Aim the render width at the wanted pixels; the height
                // follows the window's shape.
                if (rw as f64 - w).abs() > 1.0 {
                    self.scale *= w / rw as f64;
                    milk.set_resolution_scale(cx, self.scale);
                }
            }
            let rate = 44100.0;
            let mut buf = Vec::new();
            self.synth.render(rate, (dt * rate) as usize, &mut buf);
            milk.feed_audio(&buf, rate as f32);
            // Frame cost of the previous frame.
            let cpu = milk.cpu_ms();
            if cpu > 0.0 {
                self.frames += 1;
                self.cpu_sum += cpu;
                self.cpu_max = self.cpu_max.max(cpu);
                let t = milk.stage_times();
                for (sum, v) in self.stages.iter_mut().zip([t.per_frame, t.waves, t.shapes, t.per_vertex, t.rest]) {
                    *sum += v;
                }
            }
            self.since_switch += dt;
            let report_after = if self.seconds_per_preset > 0.0 { (self.seconds_per_preset * 0.8).min(6.0) } else { 6.0 };
            if !self.reported && self.since_switch > report_after {
                self.reported = true;
                if let Some(status) = milk.status() {
                    log!(
                        "milkdrop: {} | {}x{} | warp shader {} comp shader {} | cpu per-vertex {:?} | cpu waves {:?} | errors {:?} | shader errors {:?} | load {:.1} ms | cpu {:.3} ms/frame avg, {:.3} max | per-frame {:.3} waves {:.3} shapes {:.3} per-vertex {:.3} rest {:.3} ms avg",
                        status.name,
                        rw,
                        rh,
                        status.warp_shader,
                        status.comp_shader,
                        status.equations.vertex_cpu,
                        status.equations.waves_cpu,
                        status.equations.errors,
                        status.shader_errors,
                        status.load_ms,
                        self.cpu_sum / self.frames.max(1) as f64,
                        self.cpu_max,
                        self.stages[0] / self.frames.max(1) as f64,
                        self.stages[1] / self.frames.max(1) as f64,
                        self.stages[2] / self.frames.max(1) as f64,
                        self.stages[3] / self.frames.max(1) as f64,
                        self.stages[4] / self.frames.max(1) as f64,
                    );
                    let label = format!("{}  ·  {:.2} ms cpu", status.name, self.cpu_sum / self.frames.max(1) as f64);
                    self.ui.label(cx, ids!(status)).set_text(cx, &label);
                }
            }
            if self.seconds_per_preset > 0.0 && self.since_switch > self.seconds_per_preset && !self.presets.is_empty() {
                self.index = (self.index + 1) % self.presets.len();
                self.load_current(cx);
            }
            self.next_frame = cx.new_next_frame();
        }
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
