//! Splat renderer benchmark + capture harness.
//!
//! Loads one splat file into a `ViewSplat` inside an `XrSceneView`, drives a
//! deterministic camera orbit for N frames and prints load / build / upload /
//! sort / frame timings. Optionally writes PNG captures of given frames for
//! pixel-parity diffs between renderer versions.
//!
//! ```text
//! SPLAT_BENCH_FILE=/abs/path.ply \
//! SPLAT_BENCH_FRAMES=300 SPLAT_BENCH_CAPTURE=60,180 SPLAT_BENCH_OUT=/tmp/out \
//! MAKEPAD_NO_VSYNC=1 ./target/release/makepad-example-splat-bench
//! ```
//!
//! Knobs (env): `SPLAT_BENCH_FILE` (required), `SPLAT_BENCH_FRAMES` (240),
//! `SPLAT_BENCH_WARMUP` (30, frames excluded from the frame-time stats),
//! `SPLAT_BENCH_FLIP_Y` (0/1, scan-class plys are y-down), `SPLAT_BENCH_ORBIT_DEG`
//! (yaw per frame, 1.0), `SPLAT_BENCH_DISTANCE` (1.5), `SPLAT_BENCH_MAX_SPLATS`,
//! `SPLAT_BENCH_CAPTURE` (comma list of frame indices), `SPLAT_BENCH_HOLD`
//! (20: frames the camera holds before a capture so the async sort settles),
//! `SPLAT_BENCH_OUT` (dir), `SPLAT_BENCH_MIN_PX` / `SPLAT_BENCH_MAX_PX`
//! (draw_splat min/max_pixel_radius), `SPLAT_BENCH_SORT_ANGLE`
//! (sort_min_camera_angle_deg), `SPLAT_BENCH_CULL_MARGIN` (sort_cull_margin),
//! `SPLAT_BENCH_OCCLUDER` (edge of an opaque cube at the orbit target),
//! `SPLAT_BENCH_SORT=0` (draw in record order, no sorter).
pub use makepad_widgets;
use makepad_widgets::*;
use makepad_xr::obj::ViewSplat;
use makepad_xr::scene::XrSceneView;
use makepad_widgets::makepad_zune_png::{
    makepad_zune_core::{bit_depth::BitDepth, colorspace::ColorSpace, options::EncoderOptions},
    PngEncoder,
};
use std::time::Instant;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    load_all_resources() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.inner_size: vec2(1024, 768)
                body +: {
                    scene := XrSceneView{
                        width: Fill
                        height: Fill
                        camera.distance: 1.5
                        camera.distance_min: 0.03
                        // Optional opaque occluder (SPLAT_BENCH_OCCLUDER=size)
                        // to measure the scene z-buffer killing hidden splats.
                        occluder := Cube{
                            body: mod.widgets.XrBodyKind.Fixed
                            visible: false
                            size: vec3(1.0, 1.0, 1.0)
                            pos: vec3(0.0, 0.0, 0.0)
                            color: #x6a7a8a
                        }
                        splat := ViewSplat{
                            scale: vec3(1.0, 1.0, 1.0)
                        }
                    }
                }
            }
        }
    }
}

struct BenchConfig {
    file: String,
    frames: u64,
    warmup: u64,
    flip_y: bool,
    orbit_deg: f32,
    distance: f32,
    max_splats: Option<u32>,
    captures: Vec<u64>,
    /// Frames the camera holds still before each capture (see camera_for_frame).
    hold: u64,
    out_dir: String,
    /// Shader/sort knob overrides (None = widget defaults).
    min_px: Option<f32>,
    max_px: Option<f32>,
    sort_angle_deg: Option<f32>,
    cull_margin: Option<f32>,
    /// Edge length of an opaque cube at the orbit target (0 = none).
    occluder: f32,
    /// SPLAT_BENCH_SORT=0 draws in record order (no sorter work).
    sort: bool,
}

impl BenchConfig {
    fn from_env() -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let parse = |k: &str, d: f64| -> f64 {
            env(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d)
        };
        Self {
            file: env("SPLAT_BENCH_FILE").unwrap_or_default(),
            frames: parse("SPLAT_BENCH_FRAMES", 240.0) as u64,
            warmup: parse("SPLAT_BENCH_WARMUP", 30.0) as u64,
            flip_y: parse("SPLAT_BENCH_FLIP_Y", 0.0) != 0.0,
            orbit_deg: parse("SPLAT_BENCH_ORBIT_DEG", 1.0) as f32,
            distance: parse("SPLAT_BENCH_DISTANCE", 1.5) as f32,
            max_splats: env("SPLAT_BENCH_MAX_SPLATS").and_then(|v| v.parse().ok()),
            captures: env("SPLAT_BENCH_CAPTURE")
                .map(|v| v.split(',').filter_map(|s| s.trim().parse().ok()).collect())
                .unwrap_or_default(),
            hold: parse("SPLAT_BENCH_HOLD", 20.0) as u64,
            out_dir: env("SPLAT_BENCH_OUT").unwrap_or_else(|| ".".to_string()),
            min_px: env("SPLAT_BENCH_MIN_PX").and_then(|v| v.parse().ok()),
            max_px: env("SPLAT_BENCH_MAX_PX").and_then(|v| v.parse().ok()),
            sort_angle_deg: env("SPLAT_BENCH_SORT_ANGLE").and_then(|v| v.parse().ok()),
            cull_margin: env("SPLAT_BENCH_CULL_MARGIN").and_then(|v| v.parse().ok()),
            occluder: parse("SPLAT_BENCH_OCCLUDER", 0.0) as f32,
            sort: parse("SPLAT_BENCH_SORT", 1.0) != 0.0,
        }
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
    #[rust]
    config: Option<BenchConfig>,
    #[rust]
    next_frame: NextFrame,
    #[rust(0u64)]
    frame_index: u64,
    #[rust(false)]
    scene_ready: bool,
    #[rust]
    started: Option<Instant>,
    #[rust]
    last_frame_at: Option<Instant>,
    #[rust]
    frame_wall_ms: Vec<f64>,
    /// Frame whose render is pending capture on the next NextFrame.
    #[rust]
    pending_capture: Option<u64>,
    #[rust(false)]
    done: bool,
    /// Window paints seen when the last camera step was issued; the next
    /// step waits for a newer paint so at most one scene render is queued
    /// per presented frame (no GPU oversubscription).
    #[rust(0u64)]
    painted_at_step: u64,
}

impl App {
    /// Camera for a frame. Frames in the `hold` window before a capture frame
    /// reuse the capture frame's camera, so the (asynchronous) sort has
    /// settled when the capture is taken and renderer versions compare
    /// pixel-for-pixel instead of sort-staleness-for-sort-staleness.
    fn camera_for_frame(config: &BenchConfig, frame: u64) -> (f32, f32, f32) {
        let frame = config
            .captures
            .iter()
            .copied()
            .find(|&c| frame <= c && frame + config.hold >= c)
            .unwrap_or(frame);
        let f = frame as f32;
        let yaw = (f * config.orbit_deg).to_radians();
        let pitch = 0.35 * (f * 0.02).sin();
        let distance = config.distance * (1.0 + 0.15 * (f * 0.013).sin());
        (yaw, pitch, distance)
    }

    fn write_png(path: &str, width: usize, height: usize, bgra: &[u8]) -> Result<(), String> {
        let mut rgba = bgra.to_vec();
        for px in rgba.chunks_exact_mut(4) {
            px.swap(0, 2);
            px[3] = 255;
        }
        let options = EncoderOptions::default()
            .set_width(width)
            .set_height(height)
            .set_depth(BitDepth::Eight)
            .set_colorspace(ColorSpace::RGBA);
        let mut encoder = PngEncoder::new(&rgba, options);
        let mut out = Vec::new();
        encoder
            .encode(&mut out)
            .map_err(|err| format!("png encode failed: {err:?}"))?;
        std::fs::write(path, out).map_err(|err| format!("write {path}: {err}"))
    }

    fn capture(&mut self, cx: &mut Cx, frame: u64) {
        let Some(config) = self.config.as_ref() else {
            return;
        };
        let texture = {
            let scene = self.ui.widget(cx, ids!(scene));
            let Some(scene) = scene.borrow::<XrSceneView>() else {
                return;
            };
            scene.color_texture().clone()
        };
        let Some((width, height, bgra)) = cx.debug_read_render_texture(&texture) else {
            log!("splat-bench: capture of frame {frame} failed (no readback)");
            return;
        };
        let path = format!("{}/frame_{:04}.png", config.out_dir, frame);
        match Self::write_png(&path, width, height, &bgra) {
            Ok(()) => log!("splat-bench: captured {path} ({width}x{height})"),
            Err(err) => log!("splat-bench: {err}"),
        }
    }

    fn percentile(sorted: &[f64], p: f64) -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    }

    fn report(&mut self, cx: &mut Cx) {
        let Some(config) = self.config.as_ref() else {
            return;
        };
        let stats = {
            let splat = self.ui.widget(cx, ids!(splat));
            splat
                .borrow::<ViewSplat>()
                .map(|view| view.stats().clone())
                .unwrap_or_default()
        };
        let mut wall: Vec<f64> = self.frame_wall_ms.clone();
        wall.sort_by(|a, b| a.total_cmp(b));
        let wall_mean = if wall.is_empty() {
            0.0
        } else {
            wall.iter().sum::<f64>() / wall.len() as f64
        };

        // Platform monitor: last 240 painted frames (gap = paint-to-paint,
        // event = dispatch incl. our draw, draw = pass encode + uploads, gpu).
        let mut frames = Vec::new();
        cx.perf_monitor.read(&mut frames);
        let channel_index = |name: &str| {
            cx.perf_monitor
                .channels()
                .iter()
                .position(|c| c.name == name)
                .unwrap_or(0)
        };
        let (ch_event, ch_draw, ch_gpu) = (
            channel_index("event"),
            channel_index("draw"),
            channel_index("gpu"),
        );
        let painted: Vec<_> = frames.iter().filter(|f| f.gap_ms > 0.0).collect();
        let take = painted.len().saturating_sub(config.warmup as usize);
        let recent = &painted[painted.len() - take..];
        let avg = |pick: &dyn Fn(&PerfMonitorFrame) -> f64| {
            if recent.is_empty() {
                0.0
            } else {
                recent.iter().map(|f| pick(f)).sum::<f64>() / recent.len() as f64
            }
        };
        let gap_mean = avg(&|f| f.gap_ms as f64);
        let event_mean = avg(&|f| f.channel_us[ch_event] as f64 / 1000.0);
        let draw_mean = avg(&|f| f.channel_us[ch_draw] as f64 / 1000.0);
        let gpu_mean = avg(&|f| f.channel_us[ch_gpu] as f64 / 1000.0);
        let gpu_max = recent
            .iter()
            .map(|f| f.channel_us[ch_gpu] as f64 / 1000.0)
            .fold(0.0, f64::max);

        println!("SPLAT_BENCH_RESULT");
        println!("  file:                  {}", config.file);
        println!("  splats:                {}", stats.splat_count);
        println!("  visible (last sort):   {}", stats.visible_count);
        println!("  est_quad_overdraw:     {:.1} fragments/px (upper bound)", stats.est_quad_overdraw);
        println!("  load_ms:               {:.1}", stats.load_ms);
        println!("  build_ms:              {:.1}", stats.build_ms);
        println!(
            "  static_upload_mb:      {:.1}",
            stats.static_upload_bytes as f64 / 1e6
        );
        println!(
            "  sort_upload_mb:        {:.1}",
            stats.last_sort_upload_bytes as f64 / 1e6
        );
        println!(
            "  frame_instance_mb:     {:.2}",
            stats.last_frame_instance_bytes as f64 / 1e6
        );
        println!("  sorts_applied:         {}", stats.sorts_applied);
        println!(
            "  sort_ms (last/avg):    {:.1} / {:.1}",
            stats.last_sort_ms,
            if stats.sorts_applied > 0 {
                stats.total_sort_ms / stats.sorts_applied as f64
            } else {
                0.0
            }
        );
        println!("  sort_latency_ms:       {:.1}", stats.last_sort_latency_ms);
        println!("  sort_apply_ms:         {:.2}", stats.last_sort_apply_ms);
        println!("  draw_ms (main thread): {:.2}", stats.last_draw_ms);
        println!(
            "  frames:                {} (stats over {} after warmup)",
            self.frame_index,
            wall.len()
        );
        println!(
            "  wall_ms mean/p50/p95/max: {:.2} / {:.2} / {:.2} / {:.2}",
            wall_mean,
            Self::percentile(&wall, 0.5),
            Self::percentile(&wall, 0.95),
            wall.last().copied().unwrap_or(0.0)
        );
        println!(
            "  monitor ms gap/event/draw/gpu(mean) gpu(max): {:.2} / {:.2} / {:.2} / {:.2}  {:.2}  (n={})",
            gap_mean,
            event_mean,
            draw_mean,
            gpu_mean,
            gpu_max,
            recent.len()
        );
        println!(
            "  total_s:               {:.1}",
            self.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
        );
    }
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let config = BenchConfig::from_env();
        if config.file.is_empty() {
            eprintln!("splat-bench: set SPLAT_BENCH_FILE to a .ply/.sog path");
            cx.quit();
            return;
        }
        let _ = std::fs::create_dir_all(&config.out_dir);
        cx.perf_monitor.set_enabled(true);
        let mut splat = self.ui.widget(cx, ids!(splat));
        let path = config.file.clone();
        script_apply_eval!(cx, splat, {
            src: mod.res.file_resource(#(path))
        });
        if let Some(max_splats) = config.max_splats {
            script_apply_eval!(cx, splat, {
                max_splats: #(max_splats)
            });
        }
        if let Some(min_px) = config.min_px {
            script_apply_eval!(cx, splat, {
                draw_splat +: { min_pixel_radius: #(min_px) }
            });
        }
        if let Some(max_px) = config.max_px {
            script_apply_eval!(cx, splat, {
                draw_splat +: { max_pixel_radius: #(max_px) }
            });
        }
        if let Some(angle) = config.sort_angle_deg {
            script_apply_eval!(cx, splat, {
                sort_min_camera_angle_deg: #(angle)
            });
        }
        if let Some(margin) = config.cull_margin {
            script_apply_eval!(cx, splat, {
                sort_cull_margin: #(margin)
            });
        }
        if !config.sort {
            script_apply_eval!(cx, splat, {
                sort_back_to_front: false
            });
        }
        if config.occluder > 0.0 {
            // `vec3` is not in an eval fragment's scope; scale the unit cube.
            let size = config.occluder;
            let mut occluder = self.ui.widget(cx, ids!(occluder));
            script_apply_eval!(cx, occluder, {
                visible: true
                scale: mod.pod.vec3(#(size), #(size), #(size))
            });
        }
        if let Some(mut view) = splat.borrow_mut::<ViewSplat>() {
            let sy = if config.flip_y { -1.0 } else { 1.0 };
            view.set_scale(vec3(1.0, sy, 1.0));
        }
        self.config = Some(config);
        self.started = Some(Instant::now());
        self.next_frame = cx.new_next_frame();
        cx.redraw_all();
    }

    fn handle_next_frame(&mut self, cx: &mut Cx, e: &NextFrameEvent) {
        if self.done || !e.set.contains(&self.next_frame) {
            return;
        }
        let mut captured_now = false;
        if let Some(frame) = self.pending_capture.take() {
            self.capture(cx, frame);
            captured_now = true;
        }
        let Some(config) = self.config.as_ref() else {
            return;
        };
        if !self.scene_ready {
            let ready = self
                .ui
                .widget(cx, ids!(splat))
                .borrow::<ViewSplat>()
                .map(|view| view.is_scene_ready())
                .unwrap_or(false);
            if ready {
                self.scene_ready = true;
                log!(
                    "splat-bench: scene ready after {:.1}s",
                    self.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
                );
            } else {
                // Loading happens inside draw; keep redrawing until the mesh exists.
                cx.redraw_all();
                self.next_frame = cx.new_next_frame();
                return;
            }
        }
        if self.frame_index >= config.frames {
            self.done = true;
            self.report(cx);
            cx.quit();
            return;
        }
        let painted = cx.perf_monitor.frames_painted();
        if self.frame_index > 0 && painted == self.painted_at_step {
            // Previous step not presented yet: wait, don't queue another render.
            self.next_frame = cx.new_next_frame();
            return;
        }
        self.painted_at_step = painted;
        let frame = self.frame_index;
        let (yaw, pitch, distance) = Self::camera_for_frame(config, frame);
        let captures_this = config.captures.contains(&frame);
        let scene_ref = self.ui.widget(cx, ids!(scene));
        if let Some(mut scene) = scene_ref.borrow_mut::<XrSceneView>() {
            let camera = scene.camera_mut();
            camera.orbit_yaw = yaw;
            camera.orbit_pitch = pitch;
            camera.distance = distance;
        }
        drop(scene_ref);
        let now = Instant::now();
        if let Some(last) = self.last_frame_at {
            // A capture blocks on the GPU readback; that frame is not a
            // render-time sample.
            if frame >= config.warmup && !captured_now {
                self.frame_wall_ms.push(last.elapsed().as_secs_f64() * 1000.0);
            }
        }
        self.last_frame_at = Some(now);
        if captures_this {
            self.pending_capture = Some(frame);
        }
        self.frame_index += 1;
        cx.redraw_all();
        self.next_frame = cx.new_next_frame();
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        crate::makepad_widgets::script_mod(vm);
        makepad_xr::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
