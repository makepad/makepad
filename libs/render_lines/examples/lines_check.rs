//! GPU checks and a benchmark for `makepad-render-lines`, in a hidden window.
//!
//! ```text
//! MAKEPAD_HIDE_WINDOWS=1 cargo run --release -p makepad-render-lines --example lines_check -- check
//! MAKEPAD_HIDE_WINDOWS=1 cargo run --release -p makepad-render-lines --example lines_check -- bench [segments]
//! ```
//!
//! Every scene draws into an RGBA16F target (the render graph's HDR
//! accumulation format); an encode pass writes `value * scale` into an 8-bit
//! target that is read back, so values above 1 can be checked.

use makepad_render_lines::*;
use makepad_widgets::*;
use std::collections::VecDeque;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.draw.DrawLinesEncode = mod.std.set_type_default() do #(DrawLinesEncode::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex: texture_2d(float)
        // A full-target quad, free of the host's turtle clip.
        vertex: fn() {
            let p = self.geom.pos
            self.pos = p
            self.world = vec4(p.x, p.y, 0.0, 1.0)
            self.vertex_pos = vec4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.5, 1.0)
        }
        fragment: fn() {
            self.fb0 = self.pixel()
        }
        pixel: fn() {
            return self.tex.sample_nearest(self.pos) * self.scale
        }
    }

    mod.widgets.LinesHostBase = #(LinesHost::register_widget(vm))
    mod.widgets.LinesHost = set_type_default() do mod.widgets.LinesHostBase{
        width: Fill
        height: Fill
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "render-lines check"
                window.inner_size: vec2(320, 200)
                body +: {
                    host := mod.widgets.LinesHost{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawLinesEncode {
    #[deref]
    draw_super: DrawQuad,
    #[live(1.0)]
    scale: f32,
}

/// An 8-bit picture read back from the encode target, top row first.
struct Picture {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl Picture {
    fn px(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.width + x) * 4;
        [self.rgba[i], self.rgba[i + 1], self.rgba[i + 2], self.rgba[i + 3]]
    }
    /// Channel `c` summed down column `x` over rows `y0..y1`, as 0..1.
    fn column(&self, x: usize, y0: usize, y1: usize, c: usize) -> f32 {
        (y0..y1.min(self.height)).map(|y| self.px(x, y)[c] as f32 / 255.0).sum()
    }
}

struct Job {
    name: String,
    size: (usize, usize),
    view: LineView,
    lines: Vec<LineBatch>,
    points: Vec<PointBatch>,
    scale: f32,
    /// Bench frames: record and paint this many times, no readback.
    frames: usize,
}

type Check = Box<dyn Fn(&[Picture]) -> Result<String, String>>;

enum State {
    Idle,
    Recorded(u64),
    Reading(ReadbackTicket),
}

#[derive(Script, ScriptHook, Widget)]
pub struct LinesHost {
    #[uid]
    uid: WidgetUid,
    #[source]
    source: ScriptObjectRef,
    #[walk]
    walk: Walk,
    #[redraw]
    #[live]
    draw_bg: DrawQuad,
    #[live]
    encode: DrawLinesEncode,
    #[rust]
    renderer: Option<LineRenderer>,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    jobs: VecDeque<Job>,
    #[rust]
    checks: Vec<(String, usize, Check)>,
    #[rust]
    pics: Vec<Picture>,
    #[rust]
    state: Option<State>,
    #[rust]
    passes: Option<(DrawPass, DrawList2d, DrawPass, DrawList2d)>,
    #[rust]
    textures: Option<((usize, usize), Texture, Texture)>,
    #[rust]
    since: f64,
    #[rust]
    bench: Vec<f64>,
    #[rust]
    started: bool,
}

fn finish(result: Result<String, String>) -> ! {
    match result {
        Ok(r) => {
            println!("{r}");
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("lines_check: FAILED: {e}");
            std::process::exit(1)
        }
    }
}

const WHITE: [f32; 4] = [1.0, 1.0, 1.0, 1.0];

/// Horizontal screen lines of these logical widths, one per 16-px band,
/// centred at fractional offsets.
fn ink_job(scale: f32) -> Job {
    let widths = [0.35f32, 0.7, 1.0, 1.5, 2.5, 4.0];
    let mut b = LineBatch::new(LineStyle { space: Space::Screen, ..LineStyle::default() });
    for (k, w) in widths.iter().enumerate() {
        let y = 8.0 + 16.0 * k as f32 + 0.3 * k as f32;
        b.push_path(&[[8.0, y, 0.0], [120.0, y, 0.0]], *w, WHITE, false);
    }
    let size = ((128.0 * scale) as usize, (100.0 * scale) as usize);
    Job { name: format!("ink x{scale}"), size, view: LineView::screen((size.0 as f32, size.1 as f32), scale), lines: vec![b], points: vec![], scale: 1.0, frames: 0 }
}

fn ink_check(pics: &[Picture]) -> Result<String, String> {
    let widths = [0.35f32, 0.7, 1.0, 1.5, 2.5, 4.0];
    let mut out = Vec::new();
    for (k, w) in widths.iter().enumerate() {
        let mut inks = Vec::new();
        for (p, s) in pics.iter().zip([1.0f32, 2.0]) {
            let (y0, y1) = (((16 * k) as f32 * s) as usize, ((16 * k + 16) as f32 * s) as usize);
            let x = (64.0 * s) as usize;
            inks.push(p.column(x, y0, y1, 0) / s);
        }
        // 8-bit quantisation: up to half a level per covered pixel.
        let tol = 0.03 + 0.01 * w;
        if (inks[0] - w).abs() > tol || (inks[1] - w).abs() > tol {
            return Err(format!("width {w}: ink {:.3} at 1x, {:.3} at 2x", inks[0], inks[1]));
        }
        out.push(format!("{w}: {:.3}/{:.3}", inks[0], inks[1]));
    }
    Ok(format!("ink kept at 1x and 2x (logical px): {}", out.join(", ")))
}

/// One additive line at intensity 3 over another: the target must hold
/// values above 1 (read at a quarter).
fn hdr_job() -> Job {
    let style = LineStyle { space: Space::Screen, blend: Blend::Add, intensity: 3.0, ..LineStyle::default() };
    let mut b = LineBatch::new(style);
    b.push_path(&[[4.0, 16.0, 0.0], [60.0, 16.0, 0.0]], 6.0, [1.0, 0.5, 0.25, 1.0], false);
    b.push_path(&[[32.0, 4.0, 0.0], [32.0, 28.0, 0.0]], 6.0, [1.0, 0.5, 0.25, 1.0], false);
    Job { name: "hdr".into(), size: (64, 32), view: LineView::screen((64.0, 32.0), 1.0), lines: vec![b], points: vec![], scale: 0.25, frames: 0 }
}

fn hdr_check(p: &[Picture]) -> Result<String, String> {
    let p = &p[0];
    let one = p.px(10, 16);
    let two = p.px(32, 16);
    // 3.0 * 1.0 * 0.25 = 0.75 on red for one line; 1.5 (clamped to 1) where
    // they cross; green one line 3 * 0.5 * 0.25 = 0.375, two lines 0.75.
    let (r1, g1, g2) = (one[0] as f32 / 255.0, one[1] as f32 / 255.0, two[1] as f32 / 255.0);
    if (r1 - 0.75).abs() > 0.02 || (g1 - 0.375).abs() > 0.02 || (g2 - 0.75).abs() > 0.02 {
        return Err(format!("HDR values clamped or wrong: one line {one:?}, crossing {two:?}"));
    }
    if one[3] != 0 {
        return Err(format!("additive lines must leave alpha alone: {one:?}"));
    }
    Ok(format!("HDR kept: one line red {r1:.3} (3.0 x 0.25), crossing green {g2:.3} (adds)"))
}

/// Dashes, trim and a tail on screen lines.
fn style_job() -> Job {
    let solid = LineStyle { space: Space::Screen, ..LineStyle::default() };
    let mut dash = LineBatch::new(LineStyle { dash: 6.0, gap: 2.0, ..solid });
    dash.push_path(&[[0.0, 8.0, 0.0], [256.0, 8.0, 0.0]], 3.0, WHITE, false);
    let mut trim = LineBatch::new(LineStyle { trim_start: 64.0, trim_end: 192.0, ..solid });
    trim.push_path(&[[0.0, 24.0, 0.0], [256.0, 24.0, 0.0]], 3.0, WHITE, false);
    let mut tail = LineBatch::new(LineStyle { trim_end: 200.0, tail: 100.0, ..solid });
    tail.push_path(&[[0.0, 40.0, 0.0], [256.0, 40.0, 0.0]], 3.0, WHITE, false);
    let mut born = LineBatch::new(LineStyle { time: 0.5, ..solid });
    let pts: Vec<LinePoint> = (0..=16).map(|k| LinePoint { pos: [k as f32 * 16.0, 56.0, 0.0], width: 3.0, color: WHITE, birth: k as f32 / 16.0 }).collect();
    born.push_polyline(&pts, 0.0);
    Job { name: "styles".into(), size: (256, 64), view: LineView::screen((256.0, 64.0), 1.0), lines: vec![dash, trim, tail, born], points: vec![], scale: 1.0, frames: 0 }
}

fn style_check(p: &[Picture]) -> Result<String, String> {
    let p = &p[0];
    let row = |y: usize| (0..p.width).map(|x| p.px(x, y)[3] as f32 / 255.0).collect::<Vec<_>>();
    let dash = row(8);
    let on = dash[8..248].iter().sum::<f32>() / 240.0;
    if (on - 0.75).abs() > 0.05 {
        return Err(format!("dash 6 gap 2 covers {on:.3} of the line, not 0.75"));
    }
    let trim = row(24);
    if trim[32] > 0.01 || trim[128] < 0.99 || trim[224] > 0.01 {
        return Err(format!("trim 64..192: {:.2} {:.2} {:.2}", trim[32], trim[128], trim[224]));
    }
    let tail = row(40);
    if tail[90] > 0.01 || !(tail[150] > 0.4 && tail[150] < 0.6) || tail[198] < 0.95 || tail[210] > 0.01 {
        return Err(format!("tail 100 to head 200: {:.2} {:.2} {:.2} {:.2}", tail[90], tail[150], tail[198], tail[210]));
    }
    let born = row(56);
    if born[64] < 0.99 || born[200] > 0.01 {
        return Err(format!("births 0..1 at t 0.5: {:.2} at 64, {:.2} at 200", born[64], born[200]));
    }
    Ok(format!("dash coverage {on:.3}; trim, tail and birth windows hold"))
}

/// A 3D line crossing the camera plane and a world-width line: nothing
/// behind the camera, no NaN smear, world widths thin with distance.
fn world_job() -> Job {
    let view = Mat4f::look_at(vec3(0.0, 0.0, 5.0), vec3(0.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0));
    let proj = Mat4f::perspective(40.0, 2.0, 0.1, 100.0);
    let v = LineView::world(&view, &proj, (256.0, 128.0), 1.0);
    let mut through = LineBatch::new(LineStyle::default());
    through.push_path(&[[-1.0, 0.5, 0.0], [-1.0, 0.5, 20.0]], 3.0, [1.0, 0.0, 0.0, 1.0], false);
    let mut world = LineBatch::new(LineStyle { width_unit: WidthUnit::World, ..LineStyle::default() });
    world.push_path(&[[0.5, -0.5, 2.0], [0.5, -0.5, -40.0]], 0.1, [0.0, 1.0, 0.0, 1.0], false);
    let mut pts = PointBatch::new(PointStyle { shape: SpriteShape::Disc, blend: Blend::Over, ..PointStyle::default() });
    pts.push([1.5, 0.5, 0.0], 12.0, [0.0, 0.0, 1.0, 1.0], 0.0, f32::MIN);
    Job { name: "world".into(), size: (256, 128), view: v, lines: vec![through, world], points: vec![pts], scale: 1.0, frames: 0 }
}

fn world_check(p: &[Picture]) -> Result<String, String> {
    let p = &p[0];
    let red: usize = (0..p.height).flat_map(|y| (0..p.width).map(move |x| (x, y))).filter(|&(x, y)| p.px(x, y)[0] > 128).count();
    if red == 0 || red > p.width * p.height / 3 {
        return Err(format!("the line through the camera covers {red} pixels"));
    }
    // World width: wide near the camera, thin far away (measure the green
    // line's vertical extent at two columns).
    let extent = |x: usize| (0..p.height).filter(|&y| p.px(x, y)[1] > 60).count();
    // The line runs from x = 157 px (depth 3) toward the vanishing point at 128.
    let (near, far) = (extent(154), extent(131));
    let blue = (0..p.height).flat_map(|y| (0..p.width).map(move |x| (x, y))).filter(|&(x, y)| p.px(x, y)[2] > 128).count();
    if !(near > far && far >= 1) || !(80..400).contains(&blue) {
        return Err(format!("world width {near} px near vs {far} px far, sprite {blue} px"));
    }
    Ok(format!("clipped at the camera ({red} px), world width {near} px near vs {far} px far, sprite {blue} px", ))
}

fn bench_job(segments: usize, frames: usize) -> (Job, f64) {
    let t0 = std::time::Instant::now();
    let mut b = LineBatch::new(LineStyle { blend: Blend::Add, intensity: 1.5, tail: 0.0, ..LineStyle::default() });
    b.data.reserve(segments * SEGMENT_FLOATS);
    // Isoline-like polylines over a plane.
    let per = 1000;
    let mut pts = Vec::with_capacity(per + 1);
    for line in 0..segments.div_ceil(per) {
        pts.clear();
        let z = -4.0 + 8.0 * line as f32 / (segments / per).max(1) as f32;
        for k in 0..=per {
            let x = -4.0 + 8.0 * k as f32 / per as f32;
            pts.push(LinePoint::new([x, (x * 3.0 + z * 2.0).sin() * 0.3, z], 1.2, [0.9, 0.5, 0.2, 1.0]));
        }
        b.push_polyline(&pts, 0.0);
    }
    b.data.truncate(segments * SEGMENT_FLOATS);
    let build = t0.elapsed().as_secs_f64() * 1000.0;
    let view = Mat4f::look_at(vec3(0.0, 3.0, 7.0), vec3(0.0, 0.0, 0.0), vec3(0.0, 1.0, 0.0));
    let proj = Mat4f::perspective(40.0, 16.0 / 9.0, 0.1, 100.0);
    let v = LineView::world(&view, &proj, (1920.0, 1080.0), 1.0);
    (Job { name: format!("bench {segments}"), size: (1920, 1080), view: v, lines: vec![b], points: vec![], scale: 1.0, frames }, build)
}

impl LinesHost {
    fn start(&mut self, cx: &mut Cx, args: &[String]) {
        let mode = args.iter().skip(1).find(|a| !a.starts_with("--")).map(|s| s.as_str()).unwrap_or("check");
        match mode {
            "check" => {
                self.jobs.push_back(ink_job(1.0));
                self.jobs.push_back(ink_job(2.0));
                self.checks.push(("ink".into(), 2, Box::new(ink_check)));
                self.jobs.push_back(hdr_job());
                self.checks.push(("hdr".into(), 1, Box::new(hdr_check)));
                self.jobs.push_back(style_job());
                self.checks.push(("styles".into(), 1, Box::new(style_check)));
                self.jobs.push_back(world_job());
                self.checks.push(("world".into(), 1, Box::new(world_check)));
            }
            "bench" => {
                let n = args.iter().skip(2).find_map(|a| a.parse().ok()).unwrap_or(200_000);
                let (job, build) = bench_job(n, 32);
                println!("lines_check: built {n} segments in {build:.2} ms (CPU packing)");
                self.jobs.push_back(job);
            }
            _ => finish(Err("usage: lines_check check | bench [segments]".into())),
        }
        self.state = Some(State::Idle);
        self.since = Cx::monotonic_now();
        self.next_frame = cx.new_next_frame();
    }

    fn targets(&mut self, cx: &mut Cx, size: (usize, usize)) -> (Texture, Texture) {
        if self.textures.as_ref().map_or(true, |t| t.0 != size) {
            let hdr = Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 { size: TextureSize::Fixed { width: size.0, height: size.1 }, initial: true });
            let out = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Fixed { width: size.0, height: size.1 }, initial: true });
            self.textures = Some((size, hdr, out));
        }
        let t = self.textures.as_ref().unwrap();
        (t.1.clone(), t.2.clone())
    }

    fn record(&mut self, cx: &mut Cx2d) -> bool {
        if self.renderer.is_none() {
            self.renderer = LineRenderer::new(cx.cx);
        }
        if self.renderer.is_none() {
            return false;
        }
        if self.passes.is_none() {
            self.passes = Some((DrawPass::new_with_name(cx, "lines"), DrawList2d::new(cx), DrawPass::new_with_name(cx, "encode"), DrawList2d::new(cx)));
        }
        let job = self.jobs.front().unwrap();
        let size = job.size;
        let (hdr, out) = self.targets(cx.cx, size);
        let job = self.jobs.front().unwrap();
        let (lp, ll, ep, el) = self.passes.as_mut().unwrap();
        let dsize = dvec2(size.0 as f64, size.1 as f64);
        lp.set_size(cx.cx, dsize);
        lp.clear_color_textures(cx.cx);
        lp.add_color_texture(cx.cx, &hdr, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
        ep.set_size(cx.cx, dsize);
        ep.clear_color_textures(cx.cx);
        ep.add_color_texture(cx.cx, &out, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
        cx.begin_pass(lp, Some(1.0));
        ll.begin_always(cx);
        let r = self.renderer.as_mut().unwrap();
        for b in &job.lines {
            r.draw_lines(cx, &job.view, b);
        }
        for b in &job.points {
            r.draw_points(cx, &job.view, b);
        }
        ll.end(cx);
        cx.end_pass(lp);
        cx.begin_pass(ep, Some(1.0));
        el.begin_always(cx);
        self.encode.scale = job.scale;
        self.encode.draw_vars.set_texture(0, &hdr);
        self.encode.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dsize });
        el.end(cx);
        cx.end_pass(ep);
        let (l, e) = (lp.draw_pass_id(), ep.draw_pass_id());
        self.attach(cx);
        cx.cx.repaint_pass(l);
        cx.cx.repaint_pass(e);
        true
    }

    /// Hang the encode pass under the window's pass and the lines pass
    /// under the encode pass, attached by the host's current list: call on
    /// every draw while a job's frame is out (an attachment by a list that
    /// was since re-recorded without it is stale and does not paint).
    fn attach(&self, cx: &mut Cx2d) {
        let Some((lp, _, ep, _)) = &self.passes else { return };
        cx.make_child_pass(ep);
        let by = cx.draw_list_stack.last().cloned();
        cx.cx.attach_child_pass(lp.draw_pass_id(), ep.draw_pass_id(), by);
    }

    fn painted(&self, cx: &Cx, serial: u64) -> bool {
        let Some((lp, _, ep, _)) = &self.passes else { return false };
        !cx.draw_shaders_pending() && [lp, ep].iter().all(|p| !cx.passes[p.draw_pass_id()].paint_dirty && cx.passes[p.draw_pass_id()].painted_serial >= serial)
    }

    fn step(&mut self, cx: &mut Cx2d) {
        match self.state {
            Some(State::Idle) => {
                if self.jobs.is_empty() {
                    return;
                }
                if !self.record(cx) {
                    return;
                }
                self.state = Some(State::Recorded(cx.cx.repaint_id()));
                self.since = Cx::monotonic_now();
            }
            Some(State::Recorded(serial)) => {
                self.attach(cx);
                if !self.painted(cx.cx, serial) {
                    if Cx::monotonic_now() - self.since > 60.0 {
                        finish(Err(format!("{}: did not paint in 60 s", self.jobs.front().unwrap().name)));
                    }
                    return;
                }
                let job = self.jobs.front_mut().unwrap();
                if job.frames > 0 {
                    let now = Cx::monotonic_now();
                    if self.started {
                        self.bench.push(now);
                    } else {
                        self.started = true;
                        self.bench.push(now);
                    }
                    job.frames -= 1;
                    if job.frames == 0 {
                        let n = self.bench.len();
                        let per = (self.bench[n - 1] - self.bench[0]) / (n - 1).max(1) as f64 * 1000.0;
                        let calls = self.renderer.as_mut().map_or(0, |r| r.take_calls()) / n.max(1);
                        finish(Ok(format!("lines_check: {}: {per:.2} ms/frame (record + upload + GPU paint, 1920x1080 RGBA16F, {} frames, {calls} draw call per frame)", job.name, n - 1)));
                    }
                    self.state = Some(State::Idle);
                    return;
                }
                let out = self.textures.as_ref().unwrap().2.clone();
                match out.read_back(cx, ReadbackRequest::default()) {
                    Ok(ticket) => self.state = Some(State::Reading(ticket)),
                    Err(ReadbackError::Backpressure) => self.attach(cx),
                    Err(e) => finish(Err(format!("readback: {e}"))),
                }
            }
            _ => {}
        }
    }

    fn take_readback(&mut self, cx: &mut Cx) {
        let Some(State::Reading(ticket)) = self.state else { return };
        for result in cx.try_take_texture_readbacks_for(&[ticket]) {
            let data = result.data.unwrap_or_else(|e| finish(Err(format!("readback: {e}"))));
            let bgra = result.channel_order == ReadbackChannelOrder::Bgra;
            let flip = result.origin == ReadbackOrigin::BottomLeft;
            let mut rgba = Vec::with_capacity(result.width * result.height * 4);
            for y in 0..result.height {
                let row = if flip { result.height - 1 - y } else { y };
                for px in data[row * result.stride..row * result.stride + result.width * 4].chunks_exact(4) {
                    if bgra {
                        rgba.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
                    } else {
                        rgba.extend_from_slice(px);
                    }
                }
            }
            self.pics.push(Picture { width: result.width, height: result.height, rgba });
            self.jobs.pop_front();
            self.state = Some(State::Idle);
            if self.jobs.is_empty() {
                self.done();
            }
        }
    }

    fn done(&mut self) -> ! {
        let mut at = 0;
        let mut report = Vec::new();
        let mut failed = false;
        for (name, n, check) in &self.checks {
            match check(&self.pics[at..at + n]) {
                Ok(r) => report.push(format!("  ok   {name}: {r}")),
                Err(e) => {
                    failed = true;
                    report.push(format!("  FAIL {name}: {e}"))
                }
            }
            at += n;
        }
        let text = format!("lines_check:\n{}", report.join("\n"));
        if failed {
            finish(Err(text))
        }
        finish(Ok(text))
    }
}

impl Widget for LinesHost {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.take_readback(cx);
        if self.next_frame.is_event(event).is_some() {
            self.draw_bg.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.draw_walk(cx, walk);
        if self.state.is_some() {
            if matches!(self.state, Some(State::Reading(_))) {
                // Keep the passes attached while the readback is out.
                self.attach(cx);
            }
            self.step(cx);
        }
        DrawStep::done()
    }
}

#[derive(Script, ScriptHook)]
pub struct App {
    #[live]
    ui: WidgetRef,
}

impl MatchEvent for App {
    fn handle_startup(&mut self, cx: &mut Cx) {
        let args: Vec<String> = std::env::args().collect();
        if let Some(mut host) = self.ui.widget(cx, ids!(host)).borrow_mut::<LinesHost>() {
            host.start(cx, &args);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_render_lines::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
