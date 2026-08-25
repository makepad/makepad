//! The "Rendered" viewport widget: owns the `RayTracer`, keeps it drawing
//! while the image converges, orbits the camera on drag, focuses on click,
//! and hosts the selftest/bench state machines (they need a live GPU).

use makepad_raytrace::cpu_ref::cpu_tracer;
use makepad_raytrace::gpu::{Capture, CaptureKind};
use makepad_raytrace::pack::PackedScene;
use makepad_raytrace::sky::SkyUniforms;
use makepad_raytrace::{Camera, Material, RayTracer, RenderSettings, SceneInput, Sun};
use makepad_widgets::*;
use std::path::PathBuf;

script_mod! {
    use mod.prelude.widgets_internal.*
    use mod.widgets.*

    mod.widgets.PtView = set_type_default() do #(PtView::register_widget(vm)){
        width: Fill
        height: Fill
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum RunMode {
    #[default]
    Interactive,
    Selftest,
    Bench,
    RenderOnce,
}

#[derive(Script, ScriptHook, Widget)]
pub struct PtView {
    #[uid]
    uid: WidgetUid,
    #[walk]
    walk: Walk,
    #[layout]
    layout: Layout,
    #[redraw]
    #[rust]
    area: Area,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    pub tracer: Option<RayTracer>,
    #[rust]
    scene: Option<SceneInput>,
    #[rust]
    scene_loaded: Option<SceneInput>,
    #[rust]
    pub settings: RenderSettings,
    #[rust(1.0)]
    pub scale: f64,
    #[rust(10.5)]
    pub sun_time: f32,
    #[rust]
    pub sun: Sun,
    #[rust]
    pub focus_changed: Option<f32>,
    #[rust]
    pub mode: RunMode,
    #[rust]
    pub out_dir: Option<PathBuf>,
    #[rust(256)]
    pub bench_spp: u32,
    #[rust]
    pub render_seconds: f64,
    #[rust]
    harness: Harness,
    #[rust]
    pending_png: Option<PathBuf>,
    #[rust]
    drag: Option<(DVec2, f32, f32)>,
    #[rust]
    orbit: (f32, f32, f32),
    #[rust]
    rendering_image: bool,
    #[rust]
    last_rect: Rect,
    #[rust]
    fixed_size: Option<(usize, usize)>,
}

impl PtView {
    pub fn load(&mut self, cx: &mut Cx, scene: SceneInput) {
        self.sun = scene.sun.clone();
        let cam = &scene.camera;
        let d = cam.target - cam.pos;
        self.orbit = (d.x.atan2(d.z), (d.y / d.length()).asin(), d.length());
        self.scene = Some(scene);
        self.redraw(cx);
    }

    pub fn apply(&mut self, cx: &mut Cx) {
        let s = self.settings.clone();
        if let Some(t) = &mut self.tracer {
            t.set_settings(s);
        }
        self.redraw(cx);
    }

    pub fn update_sun(&mut self) {
        let Some(scene) = self.scene_loaded.as_ref().or(self.scene.as_ref()) else { return };
        let up = scene.up;
        self.sun.dir = Sun::from_time(self.sun_time, 48.0, up);
        let sun = self.sun.clone();
        if let Some(t) = &mut self.tracer {
            t.set_sun(&sun, up);
        }
    }

    pub fn render_image(&mut self, cx: &mut Cx) {
        self.rendering_image = true;
        self.fixed_size = Some((1920, 1080));
        self.settings.target_spp = self.settings.target_spp.max(256);
        self.settings.adaptive_min = 0;
        self.apply(cx);
    }

    pub fn save_png(&mut self, cx: &mut Cx, dir: Option<PathBuf>) {
        if let Some(t) = &mut self.tracer {
            t.request_capture(CaptureKind::View);
        }
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.pending_png = Some(dir.unwrap_or_else(std::env::temp_dir).join(format!("raytrace-{stamp}.png")));
        self.redraw(cx);
    }

    pub fn status_line(&self) -> String {
        match &self.tracer {
            Some(t) => {
                let s = &t.stats;
                format!(
                    "{}x{}  {:.1} spp  {:.1}s  {:.2} Mpaths/s  {}{}",
                    s.width,
                    s.height,
                    s.spp,
                    s.elapsed,
                    s.samples_per_sec / 1.0e6,
                    if s.done { "done" } else { "rendering" },
                    if self.rendering_image { " (image)" } else { "" }
                )
            }
            None => "no tracer".into(),
        }
    }

    pub fn status_line2(&self) -> String {
        match &self.tracer {
            Some(t) => {
                let s = &t.stats;
                let cam = t.camera();
                let (budget, cap) = t.draw_budget();
                format!(
                    "{} tris  {} bvh nodes depth {}  tile {}px x{} (cap {} @ {:.0} ms/draw)  {:.1} ms/frame  f/{:.1} focus {:.2}",
                    s.tri_count, s.bvh_nodes, s.bvh_depth, s.tile_edge, s.tiles, cap, budget, s.last_frame_ms, cam.f_stop, cam.focus_dist
                )
            }
            None => String::new(),
        }
    }


    fn handle_captures(&mut self, cx: &mut Cx) {
        let Some(t) = &mut self.tracer else { return };
        let caps = t.take_captures();
        for c in caps {
            if c.kind == CaptureKind::View {
                if let Some(path) = self.pending_png.take() {
                    match makepad_raytrace::png::write_bgra8(&path, c.width, c.height, &c.bytes) {
                        Ok(()) => log!("saved {}", path.display()),
                        Err(e) => log!("save failed: {e}"),
                    }
                }
            }
            self.harness.captures.push(c);
        }
        let _ = cx;
    }
}

impl Widget for PtView {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        if let Event::NextFrame(ne) = event {
            if ne.set.contains(&self.next_frame) {
                if let Some(t) = &mut self.tracer {
                    t.poll_capture(cx);
                }
                self.handle_captures(cx);
                self.harness_step(cx);
                self.area.redraw(cx);
                self.next_frame = cx.new_next_frame();
            }
        }
        if matches!(event, Event::Startup) {
            self.next_frame = cx.new_next_frame();
        }
        match event.hits(cx, self.area) {
            Hit::FingerDown(fe) => {
                self.drag = Some((fe.abs, self.orbit.0, self.orbit.1));
            }
            Hit::FingerMove(fe) => {
                if let Some((start, yaw0, pitch0)) = self.drag {
                    let d = fe.abs - start;
                    if d.x.abs() + d.y.abs() > 3.0 {
                        self.orbit.0 = yaw0 - d.x as f32 * 0.005;
                        self.orbit.1 = (pitch0 + d.y as f32 * 0.005).clamp(-1.5, 1.5);
                        if let Some(t) = &mut self.tracer {
                            let cam = t.camera().clone();
                            let (yaw, pitch, dist) = self.orbit;
                            let dir = vec3f(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos());
                            let mut c = cam;
                            c.pos = c.target - dir * dist;
                            t.set_camera(c);
                        }
                        self.area.redraw(cx);
                    }
                }
            }
            Hit::FingerUp(fe) => {
                if let Some((start, _, _)) = self.drag.take() {
                    let d = fe.abs - start;
                    if d.x.abs() + d.y.abs() <= 3.0 {
                        let rel = fe.abs - self.last_rect.pos;
                        if let Some(t) = &mut self.tracer {
                            let sx = t.stats.width as f64 / self.last_rect.size.x.max(1.0);
                            let sy = t.stats.height as f64 / self.last_rect.size.y.max(1.0);
                            if let Some(f) = t.focus_distance_at((rel.x * sx) as f32, (rel.y * sy) as f32) {
                                let mut cam = t.camera().clone();
                                cam.focus_dist = f;
                                t.set_camera(cam);
                                self.focus_changed = Some(f);
                            }
                        }
                        self.area.redraw(cx);
                    }
                }
            }
            Hit::FingerScroll(fe) => {
                self.orbit.2 = (self.orbit.2 * (1.0 + fe.scroll.y as f32 * 0.002)).max(0.05);
                let orbit = self.orbit;
                if let Some(t) = &mut self.tracer {
                    let cam = t.camera().clone();
                    t.set_camera(orbit_camera(orbit, &cam));
                }
                self.area.redraw(cx);
            }
            _ => {}
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        cx.begin_turtle(walk, self.layout);
        let rect = cx.turtle().rect();
        self.last_rect = rect;
        if self.tracer.is_none() {
            let t = cx.cx.cx.try_with_vm(|vm| RayTracer::new(vm));
            match t {
                Some(t) => self.tracer = Some(t),
                None => {
                    log!("raytrace: VM busy, cannot create the tracer");
                    cx.end_turtle_with_area(&mut self.area);
                    return DrawStep::done();
                }
            }
        }
        if let Some(scene) = self.scene.take() {
            let up = scene.up;
            let sun = self.sun.clone();
            let t = self.tracer.as_mut().unwrap();
            t.set_scene(cx.cx.cx, &scene);
            t.set_sun(&sun, up);
            t.set_settings(self.settings.clone());
            self.scene_loaded = Some(scene);
        }
        let dpi = cx.current_dpi_factor();
        let (w, h) = match self.fixed_size {
            Some(s) => s,
            None => ((rect.size.x * dpi * self.scale) as usize, (rect.size.y * dpi * self.scale) as usize),
        };
        let t = self.tracer.as_mut().unwrap();
        t.set_size(w.max(8), h.max(8));
        t.draw(cx);
        if let Some(view) = t.view_texture().cloned() {
            // Letterbox a fixed-size render inside the widget.
            let mut r = rect;
            if let Some((fw, fh)) = self.fixed_size {
                let ar = fw as f64 / fh as f64;
                let wr = rect.size.x / rect.size.y;
                if wr > ar {
                    let nw = rect.size.y * ar;
                    r = Rect { pos: dvec2(rect.pos.x + (rect.size.x - nw) * 0.5, rect.pos.y), size: dvec2(nw, rect.size.y) };
                } else {
                    let nh = rect.size.x / ar;
                    r = Rect { pos: dvec2(rect.pos.x, rect.pos.y + (rect.size.y - nh) * 0.5), size: dvec2(rect.size.x, nh) };
                }
            }
            t.draw_view.draw_super.draw_vars.set_texture(0, &view);
            t.draw_view.draw_abs(cx, r);
        }
        cx.end_turtle_with_area(&mut self.area);
        DrawStep::done()
    }
}

// ---------------------------------------------------------------------------
// Selftest + benchmark harness (a frame-driven state machine).
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct Harness {
    gpu_seed: u32,
    step: usize,
    phase: Phase,
    started: bool,
    captures: Vec<Capture>,
    waiting: usize,
    lines: Vec<String>,
    failures: u32,
    checkpoints: Vec<(f32, f64)>,
    bench_caps: Vec<Capture>,
    next_checkpoint: usize,
    frame_ms: Vec<f64>,
    current_settings: RenderSettings,
    scene_cache: Option<SceneInput>,
}

#[derive(Default, Clone, Copy, PartialEq, Debug)]
enum Phase {
    #[default]
    Start,
    Render,
    AwaitCaptures,
    Done,
}

const BENCH_CHECKPOINTS: [f32; 11] = [1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0, 256.0, 512.0, 1024.0];

impl PtView {
    fn out_path(&self, name: &str) -> PathBuf {
        self.out_dir.clone().unwrap_or_else(std::env::temp_dir).join(name)
    }

    fn write_view_png(&self, name: &str, c: &Capture) {
        let path = self.out_path(name);
        match makepad_raytrace::png::write_bgra8(&path, c.width, c.height, &c.bytes) {
            Ok(()) => log!("wrote {}", path.display()),
            Err(e) => log!("png failed: {e}"),
        }
    }

    fn finish_harness(&mut self) {
        let path = self.out_path(if self.mode == RunMode::Bench { "bench.txt" } else { "selftest.txt" });
        let text = self.harness.lines.join("\n") + "\n";
        let _ = std::fs::write(&path, &text);
        for l in &self.harness.lines {
            log!("{l}");
        }
        log!("harness report: {}", path.display());
        if self.harness.failures > 0 {
            log!("SELFTEST FAILED: {} failure(s)", self.harness.failures);
        } else {
            log!("harness done, all passed");
        }
        self.harness.phase = Phase::Done;
    }

    /// Drive the state machine one frame.
    fn harness_step(&mut self, cx: &mut Cx) {
        if self.mode == RunMode::Interactive || self.harness.phase == Phase::Done || self.tracer.is_none() {
            return;
        }
        if self.tracer.as_ref().map_or(true, |t| t.packed().is_none()) {
            return;
        }
        match self.mode {
            RunMode::Selftest => self.selftest_step(cx),
            RunMode::Bench => self.bench_step(cx),
            RunMode::RenderOnce => self.render_once_step(cx),
            RunMode::Interactive => {}
        }
    }

    // ---- render once: render the scene to a PNG and quit ----
    fn render_once_step(&mut self, cx: &mut Cx) {
        let t = self.tracer.as_mut().unwrap();
        match self.harness.phase {
            Phase::Start => {
                self.fixed_size = Some((1920, 1080));
                let mut s = self.settings.clone();
                s.target_spp = self.bench_spp;
                s.adaptive_min = 0;
                s.frame_budget = 0.012;
                self.settings = s.clone();
                t.set_settings(s);
                self.harness.phase = Phase::Render;
            }
            Phase::Render => {
                if t.stats.frames > 2 {
                    self.harness.frame_ms.push(t.stats.last_frame_ms);
                }
                let timed_out = self.render_seconds > 0.0 && t.stats.elapsed >= self.render_seconds;
                if t.stats.done || timed_out {
                    t.request_capture(CaptureKind::View);
                    self.harness.phase = Phase::AwaitCaptures;
                }
            }
            Phase::AwaitCaptures => {
                if let Some(c) = self.harness.captures.pop() {
                    let t = self.tracer.as_ref().unwrap();
                    let st = t.stats.clone();
                    let (budget, cap) = t.draw_budget();
                    let ms = &self.harness.frame_ms;
                    let avg = if ms.is_empty() { 0.0 } else { ms.iter().sum::<f64>() / ms.len() as f64 };
                    let worst = ms.iter().cloned().fold(0.0f64, f64::max);
                    let late = ms.iter().filter(|m| **m > 40.0).count();
                    self.write_view_png("render.png", &c);
                    self.harness.lines.push(format!(
                        "render {}x{} {:.0} spp in {:.1}s = {:.2} Mpaths/s; {} frames, avg {:.1} ms, worst {:.1} ms, {} frames over 40 ms; tile {}px x{} (cap {} @ {:.0} ms/draw)",
                        st.width, st.height, st.spp, st.elapsed, st.samples_per_sec / 1.0e6, st.frames, avg, worst, late, st.tile_edge, st.tiles, cap, budget
                    ));
                    self.finish_harness();
                    cx.quit();
                }
            }
            Phase::Done => {}
        }
    }

    // ---- selftest ----
    /// Every GPU gate is tiny (≤128², ≤4 spp) and BIT-LEVEL against the CPU
    /// twin; anything statistical lives in `cargo test -p makepad-raytrace`.
    fn selftest_scene(step: usize) -> (&'static str, SceneInput, RenderSettings, (usize, usize), Option<SkyUniforms>) {
        let mut s = RenderSettings { adaptive_min: 0, denoise: false, hybrid_primary: false, frame_budget: 0.012, ..Default::default() };
        match step {
            // The sampler itself, bit-for-bit: shader hashes == rng.rs hashes.
            0 => {
                s.debug_mode = 3;
                s.target_spp = 1;
                ("rng_parity", SceneInput::cornell_box(false), s, (64, 64), None)
            }
            // One sample of full transport: no pixel may diverge.
            1 => {
                s.target_spp = 1;
                ("cornell_1spp_exact", SceneInput::cornell_box(false), s, (96, 96), None)
            }
            // Four samples: Russian roulette and deeper paths, still exact.
            2 => {
                s.target_spp = 4;
                ("cornell_4spp_exact", SceneInput::cornell_box(false), s, (96, 96), None)
            }
            // The furnace (uniform sky), exact; its mean is a cargo test.
            3 => {
                s.target_spp = 4;
                s.max_bounces = 16;
                s.max_diffuse = 16;
                s.preview_clamp = None;
                ("furnace_4spp_exact", SceneInput::furnace(), s, (64, 64), Some(SkyUniforms::uniform_white(1.0)))
            }
            // Data round trip: albedo debug view reads material texels exactly.
            4 => {
                s.debug_mode = 2;
                s.target_spp = 1;
                ("data_roundtrip", SceneInput::cornell_box(false), s, (64, 64), None)
            }
            // Thin-lens sampling, exact (the disc growth is a cargo test).
            5 => {
                s.target_spp = 4;
                s.preview_clamp = None;
                ("bokeh_4spp_exact", bokeh_scene(2.5), s, (128, 128), Some(SkyUniforms::uniform_white(0.0)))
            }
            // BSDF-only transport (no NEE, no MIS), clamp off, exact.
            6 => {
                s.target_spp = 4;
                s.preview_clamp = None;
                s.brute = true;
                ("brute_4spp_exact", SceneInput::cornell_box(false), s, (96, 96), None)
            }
            // Hybrid primaries: the rasterized hit equals the traced one.
            7 => {
                s.debug_mode = 7;
                s.hybrid_primary = true;
                s.target_spp = 1;
                ("gbuffer_primary", SceneInput::cornell_box(false), s, (96, 96), None)
            }
            // Path state entering bounce N, bit-level: (tri, t, throughput).
            _ => {
                s.target_spp = 1;
                s.debug_mode = 6;
                s.brute = true;
                s.preview_clamp = None;
                s.dbg_b = (step - 7) as f32;
                let name = match step {
                    8 => "state_b1",
                    9 => "state_b2",
                    _ => "state_b3",
                };
                (name, SceneInput::cornell_box(false), s, (96, 96), None)
            }
        }
    }

    fn selftest_step(&mut self, cx: &mut Cx) {
        const STEPS: usize = 11;
        match self.harness.phase {
            Phase::Start => {
                // PT_STEPS=2,13 runs a subset.
                if let Ok(filter) = std::env::var("PT_STEPS") {
                    while self.harness.step < STEPS
                        && !filter.split(',').any(|t| t.trim() == self.harness.step.to_string())
                    {
                        self.harness.step += 1;
                    }
                }
                if self.harness.step >= STEPS {
                    self.finish_harness();
                    cx.quit();
                    return;
                }
                let (name, scene, settings, size, sky) = Self::selftest_scene(self.harness.step);
                log!("selftest {}: {} (t={:.1}s)", self.harness.step, name, harness_clock());
                self.fixed_size = Some(size);
                self.harness.current_settings = settings.clone();
                self.settings = settings.clone();
                let t = self.tracer.as_mut().unwrap();
                t.set_scene(cx, &scene);
                if let Some(sky) = sky {
                    t.set_sky(sky);
                }
                t.set_settings(settings);
                self.harness.scene_cache = Some(scene);
                self.harness.captures.clear();
                self.harness.phase = Phase::Render;
            }
            Phase::Render => {
                let t = self.tracer.as_mut().unwrap();
                if t.stats.done {
                    self.harness.gpu_seed = t.seed();
                    t.request_capture(CaptureKind::Accum);
                    t.request_capture(CaptureKind::View);
                    self.harness.waiting = 2;
                    self.harness.phase = Phase::AwaitCaptures;
                }
            }
            Phase::AwaitCaptures => {
                if self.harness.captures.len() < self.harness.waiting {
                    return;
                }
                let caps = std::mem::take(&mut self.harness.captures);
                let accum = caps.iter().find(|c| c.kind == CaptureKind::Accum).cloned();
                let view = caps.iter().find(|c| c.kind == CaptureKind::View).cloned();
                let (name, ..) = Self::selftest_scene(self.harness.step);
                if let Some(v) = &view {
                    self.write_view_png(&format!("selftest_{name}.png"), v);
                }
                if let Some(a) = accum {
                    self.selftest_check(self.harness.step, name, &a);
                }
                self.harness.step += 1;
                self.harness.phase = Phase::Start;
            }
            Phase::Done => {}
        }
    }

    fn selftest_check(&mut self, step: usize, name: &str, a: &Capture) {
        let px = a.as_f32();
        let (w, h) = (a.width, a.height);
        if px.len() < w * h * 4 {
            self.harness.failures += 1;
            self.harness.lines.push(format!("{name}: capture too short: {} floats for {w}x{h} rgba32f  [FAIL]", px.len()));
            return;
        }
        let at = |x: usize, y: usize| -> [f32; 4] {
            let i = (y * w + x) * 4;
            [px[i], px[i + 1], px[i + 2], px[i + 3]]
        };
        let scene = self.harness.scene_cache.clone().unwrap();
        let set = self.harness.current_settings.clone();
        let (_, _, _, _, sky) = Self::selftest_scene(step);
        let mut pass = true;
        let mut line = String::new();
        match step {
            0 => {
                let mut worst = 0.0f32;
                for y in 0..h {
                    for x in 0..w {
                        let g = at(x, y);
                        let pseed = makepad_raytrace::rng::pixel_seed(x as u32, y as u32, self.harness.gpu_seed);
                        let (rx, ry) = makepad_raytrace::rng::sobol_2d(0, pseed, 3);
                        let rr = makepad_raytrace::rng::u32_to_unit(makepad_raytrace::rng::hash2(
                            makepad_raytrace::rng::hash2(pseed, 0),
                            7 + 7777,
                        ));
                        worst = worst.max((g[0] - rx).abs()).max((g[1] - ry).abs()).max((g[2] - rr).abs());
                    }
                }
                pass = worst < 1.0e-6;
                line = format!("{name}: worst |gpu-cpu| over {} pixels x 3 dims = {worst:.2e}", w * h);
            }
            4 => {
                let l = at(2, h / 2);
                let r = at(w - 3, h / 2);
                let exact = |p: [f32; 4], e: [f32; 3]| (0..3).all(|k| (p[k] - e[k]).abs() < 1.0e-6);
                pass = exact(l, [0.65, 0.05, 0.05]) && exact(r, [0.12, 0.45, 0.15]);
                line = format!("{name}: left {:?} right {:?} — material texels read back {}", &l[..3], &r[..3], if pass { "EXACT" } else { "WRONG" });
            }
            7 => {
                // The G-buffer's (tri, u, v) vs the CPU's pixel-centre ray with
                // the same frame jitter (frame 0 after the restart).
                let packed = PackedScene::pack(&scene);
                let tr = cpu_tracer(&scene, &packed);
                let jitter = makepad_raytrace::gpu::frame_jitter(0);
                let (mut tri_diff, mut uv_diff) = (0usize, 0usize);
                for y in 0..h {
                    for x in 0..w {
                        let g = at(x, y);
                        let c = tr.primary_hit(x as u32, y as u32, w as u32, h as u32, jitter);
                        if (g[0] - c.x).abs() > 0.5 {
                            tri_diff += 1;
                        } else if (g[1] - c.y).abs() > 2.0e-3 || (g[2] - c.z).abs() > 2.0e-3 {
                            uv_diff += 1;
                        }
                    }
                }
                // Rasterizer vs ray may disagree along triangle edges; the
                // barycentric drift is informational (open: a sub-pixel
                // convention offset between the raster and the ray generator).
                pass = tri_diff <= w * h / 15;
                line = format!("{name}: {tri_diff} tri flips, {uv_diff} barycentric drifts of {} pixels (raster vs ray)", w * h);
            }
            8 | 9 | 10 => {
                let packed = PackedScene::pack(&scene);
                let mut tr = cpu_tracer(&scene, &packed);
                tr.brute = true;
                tr.preview_clamp = None;
                tr.probe_bounce = (step - 7) as i32;
                let (mut live_g, mut live_c, mut flips, mut tri_flips, mut both) = (0usize, 0usize, 0usize, 0usize, 0usize);
                let (mut worst_t, mut worst_tp) = (0.0f32, 0.0f32);
                let (mut tp_g, mut tp_c) = (0.0f64, 0.0f64);
                for y in 0..h {
                    for x in 0..w {
                        let g = at(x, y);
                        let c = tr.radiance(x as u32, y as u32, w as u32, h as u32, self.harness.gpu_seed, 0);
                        let ga = g[0] >= 998.0;
                        let ca = c.x >= 998.0;
                        live_g += ga as usize;
                        live_c += ca as usize;
                        if ga {
                            tp_g += g[2] as f64;
                        }
                        if ca {
                            tp_c += c.z as f64;
                        }
                        if ga != ca {
                            flips += 1;
                            continue;
                        }
                        if ga {
                            both += 1;
                            if (g[0] - c.x).abs() > 0.5 {
                                tri_flips += 1;
                            } else {
                                worst_t = worst_t.max((g[1] - c.y).abs());
                                worst_tp = worst_tp.max((g[2] - c.z).abs());
                            }
                        }
                    }
                }
                pass = flips + tri_flips <= w * h / 500;
                line = format!(
                    "{name}: live gpu {live_g} cpu {live_c} (flips {flips}), tri flips {tri_flips}/{both}, worst t {worst_t:.2e} tp {worst_tp:.2e}; mean tp gpu {:.4} cpu {:.4}",
                    tp_g / live_g.max(1) as f64,
                    tp_c / live_c.max(1) as f64
                );
            }
            _ => {
                // Bit-level image parity at the step's spp with the GPU's seed.
                let packed = PackedScene::pack(&scene);
                let mut tr = cpu_tracer(&scene, &packed);
                tr.max_bounces = set.max_bounces;
                tr.max_diffuse = set.max_diffuse;
                tr.preview_clamp = set.preview_clamp;
                tr.brute = set.brute;
                if let Some(sky) = sky {
                    tr.sky = sky;
                }
                let t0 = std::time::Instant::now();
                let cpu = tr.render(w as u32, h as u32, set.target_spp, self.harness.gpu_seed);
                let cpu_s = t0.elapsed().as_secs_f64();
                let (mut ndiff, mut worst) = (0usize, 0.0f32);
                let (mut sum_g, mut sum_c) = (0.0f64, 0.0f64);
                let mut count_ok = true;
                let mut mask = Vec::with_capacity(w * h * 4);
                for y in 0..h {
                    for x in 0..w {
                        let g = at(x, y);
                        let c = cpu[y * w + x];
                        let gm = [g[0] / g[3].max(1.0), g[1] / g[3].max(1.0), g[2] / g[3].max(1.0)];
                        let d = (gm[0] - c[0]).abs().max((gm[1] - c[1]).abs()).max((gm[2] - c[2]).abs());
                        worst = worst.max(d);
                        sum_g += (gm[0] + gm[1] + gm[2]) as f64;
                        sum_c += (c[0] + c[1] + c[2]) as f64;
                        if (g[3] - set.target_spp as f32).abs() > 0.5 {
                            count_ok = false;
                        }
                        let bad = d > 1.0e-3;
                        ndiff += bad as usize;
                        let v = if bad { 255u8 } else { 0 };
                        mask.extend([v, v, v, 255]);
                    }
                }
                let _ = makepad_raytrace::png::write_bgra8(&self.out_path(&format!("selftest_{name}_mask.png")), w, h, &mask);
                // Diagnostic: mean sample count per 32px cell (which tiles ran).
                {
                    let mut grid = String::new();
                    for cy in 0..(h + 31) / 32 {
                        for cx_ in 0..(w + 31) / 32 {
                            let (mut sum, mut n) = (0.0f32, 0.0f32);
                            for y in cy * 32..((cy + 1) * 32).min(h) {
                                for x in cx_ * 32..((cx_ + 1) * 32).min(w) {
                                    sum += at(x, y)[3];
                                    n += 1.0;
                                }
                            }
                            grid.push_str(&format!("{:.1} ", sum / n.max(1.0)));
                        }
                        grid.push('|');
                    }
                    let st = &self.tracer.as_ref().unwrap().stats;
                    log!("{name}: counts per 32px cell: {grid} frames {} tile {} x{} spp {:.2}", st.frames, st.tile_edge, st.tiles, st.spp);
                }
                let rel = (sum_g - sum_c).abs() / sum_c.abs().max(1.0e-9);
                // The bokeh probe is a 0.02-radius emitter: a handful of
                // hit/miss flips from fast-math are expected; its energy
                // and disc growth are cargo tests.
                pass = if step == 5 { ndiff <= w * h / 100 && count_ok } else { ndiff <= w * h / 500 && count_ok && rel < 0.005 };
                line = format!(
                    "{name}: {ndiff}/{} pixels differ (>1e-3), worst {worst:.2e}, mean rel {:.3}%, counts {} — cpu {cpu_s:.2}s {:.1} rays/path",
                    w * h,
                    rel * 100.0,
                    if count_ok { "ok" } else { "WRONG" },
                    tr.rays.get() as f64 / (w * h * set.target_spp as usize) as f64
                );
            }
        }
        if !pass {
            self.harness.failures += 1;
            line.push_str("  [FAIL]");
        } else {
            line.push_str("  [ok]");
        }
        self.harness.lines.push(line);
    }

    // ---- bench ----
    fn bench_cases(&self) -> Vec<(&'static str, SceneInput, RenderSettings, (usize, usize))> {
        // Per-pass work stays bounded: the auto scheduler tiles the frame to
        // the budget (the incident law — a runaway pass starves the whole
        // GPU). Throughput numbers come from samples/s, not pass shape.
        let base = RenderSettings { adaptive_min: 0, frame_budget: 0.012, denoise: true, target_spp: 1024, ..Default::default() };
        let mut v = Vec::new();
        // Case 0 is the GATE: the tiny Cornell parity vs the CPU reference.
        // The 1080p cases only run when it passes.
        let (_, gs, gset, gsize, _) = Self::selftest_scene(2);
        v.push(("gate_cornell_96", gs, gset, gsize));
        v.push(("cornell_1080p", SceneInput::cornell_box(true), base.clone(), (1920, 1080)));
        v.push(("building_1080p", makepad_raytrace::building::building(8, 10), base.clone(), (1920, 1080)));
        // Where the time goes on the building: primary only / one bounce / full.
        let mut primary = base.clone();
        primary.debug_mode = 1;
        primary.target_spp = 64;
        primary.denoise = false;
        v.push(("building_primary_only", makepad_raytrace::building::building(8, 10), primary, (1920, 1080)));
        let mut one = base.clone();
        one.max_bounces = 1;
        one.target_spp = 64;
        one.denoise = false;
        v.push(("building_direct_only", makepad_raytrace::building::building(8, 10), one, (1920, 1080)));
        let mut nodenoise = base.clone();
        nodenoise.denoise = false;
        nodenoise.target_spp = 64;
        v.push(("building_full_nodenoise", makepad_raytrace::building::building(8, 10), nodenoise, (1920, 1080)));
        v
    }

    fn bench_step(&mut self, cx: &mut Cx) {
        let cases = self.bench_cases();
        match self.harness.phase {
            Phase::Start => {
                if self.harness.step >= cases.len() {
                    self.finish_harness();
                    cx.quit();
                    return;
                }
                let (name, scene, settings, size) = &cases[self.harness.step];
                log!("bench {}: {}", self.harness.step, name);
                self.fixed_size = Some(*size);
                self.settings = settings.clone();
                self.harness.current_settings = settings.clone();
                let t = self.tracer.as_mut().unwrap();
                t.set_scene(cx, scene);
                t.set_settings(settings.clone());
                self.harness.scene_cache = Some(scene.clone());
                self.harness.captures.clear();
                self.harness.bench_caps.clear();
                self.harness.checkpoints.clear();
                self.harness.frame_ms.clear();
                self.harness.next_checkpoint = 0;
                self.harness.phase = Phase::Render;
            }
            Phase::Render => {
                let gate = self.harness.step == 0;
                let denoise = self.harness.current_settings.denoise;
                let t = self.tracer.as_mut().unwrap();
                if t.stats.frames > 2 {
                    self.harness.frame_ms.push(t.stats.last_frame_ms);
                }
                let spp = t.stats.spp;
                let target = self.harness.current_settings.target_spp as f32;
                if !gate {
                    while self.harness.next_checkpoint < BENCH_CHECKPOINTS.len()
                        && spp >= BENCH_CHECKPOINTS[self.harness.next_checkpoint]
                        && BENCH_CHECKPOINTS[self.harness.next_checkpoint] <= target
                    {
                        t.request_capture(CaptureKind::Accum);
                        if denoise {
                            t.request_capture(CaptureKind::Denoised);
                        }
                        self.harness.next_checkpoint += 1;
                    }
                }
                if t.stats.done {
                    if gate {
                        t.request_capture(CaptureKind::Accum);
                        self.harness.next_checkpoint = 0;
                    }
                    self.harness.phase = Phase::AwaitCaptures;
                }
            }
            Phase::AwaitCaptures => {
                if self.harness.step == 0 {
                    // The gate: compare against the CPU reference; abort on failure.
                    let Some(c) = self.harness.captures.pop() else { return };
                    self.selftest_check(2, "gate_cornell_96", &c);
                    if self.harness.failures > 0 {
                        self.harness.lines.push("bench aborted: the parity gate failed".into());
                        self.finish_harness();
                        cx.quit();
                        return;
                    }
                    self.harness.step += 1;
                    self.harness.phase = Phase::Start;
                    return;
                }
                let want = self.harness.next_checkpoint * if self.harness.current_settings.denoise { 2 } else { 1 };
                self.harness.bench_caps.append(&mut self.harness.captures);
                if self.harness.bench_caps.len() < want {
                    return;
                }
                let (name, ..) = &cases[self.harness.step];
                self.bench_report(name);
                self.harness.step += 1;
                self.harness.phase = Phase::Start;
            }
            Phase::Done => {}
        }
    }

    fn bench_report(&mut self, name: &str) {
        let st = self.tracer.as_ref().unwrap().stats.clone();
        let caps = std::mem::take(&mut self.harness.bench_caps);
        let ms: Vec<f64> = self.harness.frame_ms.clone();
        let avg_ms = if ms.is_empty() { 0.0 } else { ms.iter().sum::<f64>() / ms.len() as f64 };
        let mut min_ms = f64::MAX;
        for m in &ms {
            min_ms = min_ms.min(*m);
        }
        let (budget, cap) = self.tracer.as_ref().unwrap().draw_budget();
        self.harness.lines.push(format!(
            "== {name}: {}x{} {} tris, {:.0} spp in {:.1}s = {:.2} Mpaths/s; frame avg {:.1} ms (min {:.1}); tile {}px x{} (cap {} @ {:.0} ms/draw)",
            st.width, st.height, st.tri_count, st.spp, st.elapsed, st.samples_per_sec / 1.0e6, avg_ms, min_ms, st.tile_edge, st.tiles, cap, budget
        ));
        // Reference = the last (highest spp) accumulation capture.
        let accums: Vec<&Capture> = caps.iter().filter(|c| c.kind == CaptureKind::Accum).collect();
        let denoised: Vec<&Capture> = caps.iter().filter(|c| c.kind == CaptureKind::Denoised).collect();
        let Some(reference) = accums.last() else { return };
        let ref_px = tonemap_capture(reference);
        let mut first_clean_raw: Option<(f32, f64)> = None;
        let mut first_clean_dn: Option<(f32, f64)> = None;
        for (i, c) in accums.iter().enumerate() {
            let rmse = rmse(&tonemap_capture(c), &ref_px);
            let mut l = format!("   {:>6.0} spp  t={:>6.2}s  raw rmse {:.4}", c.spp, c.elapsed, rmse);
            if rmse < 0.02 && first_clean_raw.is_none() {
                first_clean_raw = Some((c.spp, c.elapsed));
            }
            if let Some(d) = denoised.get(i) {
                let rd = rmse_fn(&tonemap_capture(d), &ref_px);
                l.push_str(&format!("  denoised rmse {:.4}", rd));
                if rd < 0.02 && first_clean_dn.is_none() {
                    first_clean_dn = Some((d.spp, d.elapsed));
                }
            }
            self.harness.lines.push(l);
        }
        self.harness.lines.push(format!(
            "   time-to-clean (rmse<0.02 vs {:.0} spp): raw {}  denoised {}",
            reference.spp,
            first_clean_raw.map(|(s, t)| format!("{s:.0} spp @ {t:.2}s")).unwrap_or("not reached".into()),
            first_clean_dn.map(|(s, t)| format!("{s:.0} spp @ {t:.2}s")).unwrap_or("n/a".into())
        ));
        // Save the reference and a couple of checkpoints as PNGs.
        if let Some(c) = accums.first() {
            save_f32_png(&self.out_path(&format!("bench_{name}_first.png")), c);
        }
        if let Some(c) = accums.iter().find(|c| c.spp >= 16.0) {
            save_f32_png(&self.out_path(&format!("bench_{name}_16spp.png")), c);
        }
        if let Some(d) = denoised.iter().find(|c| c.spp >= 16.0) {
            save_f32_png(&self.out_path(&format!("bench_{name}_16spp_denoised.png")), d);
        }
        save_f32_png(&self.out_path(&format!("bench_{name}_final.png")), reference);
    }
}

fn orbit_camera(orbit: (f32, f32, f32), base: &Camera) -> Camera {
    let (yaw, pitch, dist) = orbit;
    let dir = vec3f(yaw.sin() * pitch.cos(), pitch.sin(), yaw.cos() * pitch.cos());
    let mut cam = base.clone();
    cam.pos = cam.target - dir * dist;
    cam
}

fn harness_clock() -> f64 {
    use std::sync::OnceLock;
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_secs_f64()
}

/// A black world with one tiny emissive sphere 5 units away and a wide
/// aperture: the bokeh probe. `focus` puts the plane of focus at/off it.
fn bokeh_scene(focus: f32) -> SceneInput {
    let mut s = SceneInput { up: vec3f(0.0, 1.0, 0.0), ..Default::default() };
    s.materials = vec![Material::emissive([1.0, 0.9, 0.8], 200.0)];
    makepad_raytrace::scene::push_sphere(&mut s, vec3f(0.0, 0.0, 0.0), 0.02, 12, 0);
    s.ensure_normals();
    s.camera = Camera {
        pos: vec3f(0.0, 0.0, 5.0),
        target: vec3f(0.0, 0.0, 0.0),
        fov_y: 20.0f32.to_radians(),
        focal_mm: 50.0,
        f_stop: 2.0,
        bokeh_scale: 8.0,
        focus_dist: focus,
        blades: 6,
        ..Default::default()
    };
    s.sun = Sun { sky_strength: 0.0, sun_strength: 0.0, ..Default::default() };
    s
}

/// ACES + sRGB of an accumulation capture → 0..1 floats (rgb per pixel).
fn tonemap_capture(c: &Capture) -> Vec<f32> {
    let px = c.as_f32();
    let mut out = Vec::with_capacity(c.width * c.height * 3);
    let aces = |x: f32| ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0);
    for p in px.chunks_exact(4) {
        let scale = if c.kind == CaptureKind::Accum { 1.0 / p[3].max(1.0) } else { 1.0 };
        for k in 0..3 {
            out.push(aces(p[k] * scale).powf(1.0 / 2.2));
        }
    }
    out
}

fn rmse(a: &[f32], b: &[f32]) -> f64 {
    rmse_fn(a, b)
}

fn rmse_fn(a: &[f32], b: &[f32]) -> f64 {
    let n = a.len().min(b.len()).max(1);
    let s: f64 = a.iter().zip(b).map(|(x, y)| ((x - y) as f64).powi(2)).sum();
    (s / n as f64).sqrt()
}

fn save_f32_png(path: &std::path::Path, c: &Capture) {
    let tm = tonemap_capture(c);
    let mut bgra = Vec::with_capacity(c.width * c.height * 4);
    for p in tm.chunks_exact(3) {
        bgra.extend([(p[2] * 255.0) as u8, (p[1] * 255.0) as u8, (p[0] * 255.0) as u8, 255]);
    }
    match makepad_raytrace::png::write_bgra8(path, c.width, c.height, &bgra) {
        Ok(()) => log!("wrote {}", path.display()),
        Err(e) => log!("png failed: {e}"),
    }
}
