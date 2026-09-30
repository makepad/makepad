//! Renders a kinetic kit in a hidden window: a contact sheet of frames, or
//! a CPU benchmark of the per-frame work.
//!
//! ```text
//! MAKEPAD_HIDE_WINDOWS=1 kinetic_check strip <kit.splash> --out sheet.png [--text "..."] [--count 8]
//!     [--from 0] [--step 0.25 (beats)] [--bpm 120] [--width 480] [--cols 4] [--sing] [--p1 0.5]..
//! MAKEPAD_HIDE_WINDOWS=1 kinetic_check bench <kit.splash> [--text "..."] [--frames 240]
//! ```
//!
//! `--sing` sweeps the karaoke progress 0..1 across the strip. Frame k is
//! at beat `from + k * step`; time = beat * 60 / bpm.

use makepad_kinetic_type::*;
use makepad_widgets::makepad_zune_png::makepad_zune_core::bit_depth::BitDepth;
use makepad_widgets::makepad_zune_png::makepad_zune_core::colorspace::ColorSpace;
use makepad_widgets::makepad_zune_png::makepad_zune_core::options::EncoderOptions;
use makepad_widgets::makepad_zune_png::PngEncoder;
use makepad_widgets::*;

app_main!(App);

script_mod! {
    use mod.prelude.widgets.*
    use mod.widgets.*

    mod.draw.DrawKineticEncode = mod.std.set_type_default() do #(DrawKineticEncode::script_shader(vm)){
        ..mod.draw.DrawQuad
        tex: texture_2d(float)
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
            let c = self.tex.sample_nearest(self.pos)
            return vec4(clamp(c.xyz, vec3(0.0, 0.0, 0.0), vec3(1.0, 1.0, 1.0)), 1.0)
        }
    }

    mod.widgets.KineticHostBase = #(KineticHost::register_widget(vm))
    mod.widgets.KineticHost = set_type_default() do mod.widgets.KineticHostBase{
        width: Fill
        height: Fill
    }

    startup() do #(App::script_component(vm)){
        ui: Root{
            main_window := Window{
                window.title: "kinetic check"
                window.inner_size: vec2(320, 200)
                body +: {
                    host := mod.widgets.KineticHost{}
                }
            }
        }
    }
}

#[derive(Script, ScriptHook)]
#[repr(C)]
pub struct DrawKineticEncode {
    #[deref]
    draw_super: DrawQuad,
}

fn finish(result: Result<String, String>) -> ! {
    match result {
        Ok(r) => {
            println!("{r}");
            std::process::exit(0)
        }
        Err(e) => {
            eprintln!("kinetic_check: FAILED: {e}");
            std::process::exit(1)
        }
    }
}

fn arg(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn argf(args: &[String], name: &str, d: f32) -> f32 {
    arg(args, name).and_then(|v| v.parse().ok()).unwrap_or(d)
}

struct Job {
    kit: String,
    file: String,
    text: String,
    out: Option<String>,
    count: usize,
    from: f32,
    step: f32,
    bpm: f32,
    size: (u32, u32),
    cols: usize,
    sing: bool,
    dials: [Option<f32>; 4],
    bench: usize,
}

enum State {
    Load,
    Record,
    Wait(u64, f64),
    Read(ReadbackTicket),
}

#[derive(Script, ScriptHook, Widget)]
pub struct KineticHost {
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
    encode: DrawKineticEncode,
    #[rust]
    job: Option<Job>,
    #[rust]
    view: Option<KineticView>,
    #[rust]
    state: Option<State>,
    #[rust]
    frame: usize,
    #[rust]
    next_frame: NextFrame,
    #[rust]
    pass: Option<(DrawPass, DrawList2d, Texture)>,
    #[rust]
    sheet: Vec<u8>,
    #[rust]
    cpu: Vec<(f32, f32, f32)>,
    #[rust]
    started: f64,
}

impl KineticHost {
    fn start(&mut self, cx: &mut Cx, args: &[String]) {
        let mode = args.get(1).cloned().unwrap_or_default();
        let Some(file) = args.get(2).cloned() else { finish(Err("usage: kinetic_check strip|bench <kit.splash> [options]".into())) };
        let kit = std::fs::read_to_string(&file).unwrap_or_else(|e| finish(Err(format!("{file}: {e}"))));
        let width = argf(args, "--width", 480.0) as u32;
        let mut dials = [None; 4];
        for (k, d) in dials.iter_mut().enumerate() {
            *d = arg(args, &format!("--p{}", k + 1)).and_then(|v| v.parse().ok());
        }
        self.job = Some(Job {
            kit,
            file,
            text: arg(args, "--text").unwrap_or_default(),
            out: arg(args, "--out"),
            count: argf(args, "--count", 8.0) as usize,
            from: argf(args, "--from", 0.0),
            step: argf(args, "--step", 0.5),
            bpm: argf(args, "--bpm", 120.0),
            size: (width, width * 9 / 16),
            cols: argf(args, "--cols", 4.0) as usize,
            sing: args.iter().any(|a| a == "--sing"),
            dials,
            bench: if mode == "bench" { argf(args, "--frames", 240.0) as usize } else { 0 },
        });
        self.state = Some(State::Load);
        self.next_frame = cx.new_next_frame();
    }

    fn frame_of(&self, k: usize) -> KineticFrame {
        let job = self.job.as_ref().unwrap();
        let n = job.count.max(1);
        let beat = job.from + k as f32 * job.step;
        KineticFrame {
            time: beat * 60.0 / job.bpm,
            beat,
            bpm: job.bpm,
            energy: 0.5,
            bands: [0.5, 0.4, 0.3],
            dials: job.dials,
            karaoke: if job.sing { Karaoke::Progress((k as f32 + 0.5) / n as f32) } else { Karaoke::None },
            content: None,
        }
    }

    fn record(&mut self, cx: &mut Cx2d, k: usize) {
        let size = self.job.as_ref().unwrap().size;
        let frame = self.frame_of(k);
        if self.pass.is_none() {
            let tex = Texture::new_with_format(cx.cx, TextureFormat::RenderBGRAu8 { size: TextureSize::Fixed { width: size.0 as usize, height: size.1 as usize }, initial: true });
            self.pass = Some((DrawPass::new_with_name(cx, "encode"), DrawList2d::new(cx), tex));
        }
        let (ep, el, tex) = self.pass.as_mut().unwrap();
        let dsize = dvec2(size.0 as f64, size.1 as f64);
        ep.set_size(cx.cx, dsize);
        ep.clear_color_textures(cx.cx);
        ep.add_color_texture(cx.cx, tex, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)));
        cx.make_child_pass(ep);
        cx.begin_pass(ep, Some(1.0));
        el.begin_always(cx);
        let view = self.view.as_mut().unwrap();
        if let Some(out) = view.render(cx, size, &frame) {
            self.encode.draw_vars.set_texture(0, &out);
        }
        self.encode.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dsize });
        el.end(cx);
        cx.end_pass(ep);
        let id = ep.draw_pass_id();
        cx.cx.repaint_pass(id);
    }

    fn painted(&self, cx: &Cx, serial: u64) -> bool {
        let Some((ep, _, _)) = &self.pass else { return false };
        let p = &cx.passes[ep.draw_pass_id()];
        !cx.draw_shaders_pending() && !p.paint_dirty && p.painted_serial >= serial
    }

    fn step(&mut self, cx: &mut Cx2d) {
        match self.state.take() {
            Some(State::Load) => {
                let job = self.job.as_ref().unwrap();
                let mut view = KineticView::new(cx.cx, &job.kit, &job.file).unwrap_or_else(|e| finish(Err(e)));
                let t0 = std::time::Instant::now();
                view.set_text(cx.cx, &job.text, 0.0);
                let build = t0.elapsed().as_secs_f64() * 1e3;
                if !view.errors.is_empty() {
                    finish(Err(view.errors.join("\n")));
                }
                println!("kinetic_check: {} loaded; shapes built in {build:.2} ms", view.values.name);
                self.view = Some(view);
                let (w, h) = job.size;
                let rows = job.count.div_ceil(job.cols.max(1));
                self.sheet = vec![0; (w as usize * job.cols.min(job.count)) * (h as usize * rows) * 4];
                self.state = Some(State::Record);
                self.started = Cx::monotonic_now();
            }
            Some(State::Record) => {
                let k = self.frame;
                self.record(cx, k);
                let v = self.view.as_ref().unwrap();
                self.cpu.push((v.stats.animate_us, v.stats.kernel_us, v.stats.record_us));
                self.state = Some(State::Wait(cx.cx.repaint_id(), Cx::monotonic_now()));
            }
            Some(State::Wait(serial, since)) => {
                if let Some((ep, _, _)) = &self.pass {
                    cx.make_child_pass(ep);
                }
                if !self.painted(cx.cx, serial) {
                    if Cx::monotonic_now() - since > 60.0 {
                        finish(Err("the frame did not paint in 60 s".into()));
                    }
                    self.state = Some(State::Wait(serial, since));
                    return;
                }
                let job = self.job.as_ref().unwrap();
                if job.bench > 0 {
                    self.frame += 1;
                    if self.frame >= job.bench {
                        self.report_bench();
                    }
                    self.state = Some(State::Record);
                    return;
                }
                let tex = self.pass.as_ref().unwrap().2.clone();
                match tex.read_back(cx, ReadbackRequest::default()) {
                    Ok(t) => self.state = Some(State::Read(t)),
                    Err(ReadbackError::Backpressure) => self.state = Some(State::Wait(serial, since)),
                    Err(e) => finish(Err(format!("readback: {e}"))),
                }
            }
            other => self.state = other,
        }
    }

    fn report_bench(&self) -> ! {
        let n = self.cpu.len().max(1) as f32;
        let mean = |f: &dyn Fn(&(f32, f32, f32)) -> f32| self.cpu.iter().map(f).sum::<f32>() / n;
        let max = |f: &dyn Fn(&(f32, f32, f32)) -> f32| self.cpu.iter().map(f).fold(0.0f32, f32::max);
        let v = self.view.as_ref().unwrap();
        let wall = (Cx::monotonic_now() - self.started) * 1000.0 / n as f64;
        finish(Ok(format!(
            "kinetic_check bench {}: {} elements, {} draw calls; CPU per frame: animate {:.1} µs (kernel {:.1} µs, max {:.1}), record {:.1} µs (max {:.1}); total {:.3} ms mean; {:.2} ms/frame wall incl. GPU paint over {} frames",
            v.values.name,
            v.stats.elements,
            v.stats.draw_calls,
            mean(&|c| c.0),
            mean(&|c| c.1),
            max(&|c| c.1),
            mean(&|c| c.2),
            max(&|c| c.2),
            (mean(&|c| c.0) + mean(&|c| c.2)) / 1000.0,
            wall,
            self.cpu.len()
        )))
    }

    fn take_readback(&mut self, cx: &mut Cx) {
        let Some(State::Read(ticket)) = &self.state else { return };
        let ticket = *ticket;
        for result in cx.try_take_texture_readbacks_for(&[ticket]) {
            let data = result.data.unwrap_or_else(|e| finish(Err(format!("readback: {e}"))));
            let job = self.job.as_ref().unwrap();
            let bgra = result.channel_order == ReadbackChannelOrder::Bgra;
            let flip = result.origin == ReadbackOrigin::BottomLeft;
            let cols = job.cols.min(job.count).max(1);
            let (w, h) = (job.size.0 as usize, job.size.1 as usize);
            let (cx0, cy0) = ((self.frame % cols) * w, (self.frame / cols) * h);
            let sheet_w = w * cols;
            for y in 0..h.min(result.height) {
                let row = if flip { result.height - 1 - y } else { y };
                for x in 0..w.min(result.width) {
                    let s = row * result.stride + x * 4;
                    let px = &data[s..s + 4];
                    let d = ((cy0 + y) * sheet_w + cx0 + x) * 4;
                    let rgba = if bgra { [px[2], px[1], px[0], 255] } else { [px[0], px[1], px[2], 255] };
                    self.sheet[d..d + 4].copy_from_slice(&rgba);
                }
            }
            self.frame += 1;
            if self.frame >= job.count {
                let rows = job.count.div_ceil(cols);
                let out = job.out.clone().unwrap_or_else(|| "kinetic_sheet.png".into());
                let options = EncoderOptions::default().set_width(sheet_w).set_height(h * rows).set_depth(BitDepth::Eight).set_colorspace(ColorSpace::RGBA);
                let mut png = Vec::new();
                PngEncoder::new(&self.sheet, options).encode(&mut png).unwrap_or_else(|e| finish(Err(format!("png: {e:?}"))));
                std::fs::write(&out, png).unwrap_or_else(|e| finish(Err(format!("{out}: {e}"))));
                let v = self.view.as_ref().unwrap();
                let n = self.cpu.len().max(1) as f32;
                let animate = self.cpu.iter().map(|c| c.0).sum::<f32>() / n;
                let record = self.cpu.iter().map(|c| c.2).sum::<f32>() / n;
                finish(Ok(format!("kinetic_check: wrote {out} ({} frames; {} elements, {} draw calls; CPU animate {animate:.1} µs + record {record:.1} µs per frame)", job.count, v.stats.elements, v.stats.draw_calls)));
            }
            self.state = Some(State::Record);
        }
    }
}

impl Widget for KineticHost {
    fn handle_event(&mut self, cx: &mut Cx, event: &Event, _scope: &mut Scope) {
        self.take_readback(cx);
        if self.next_frame.is_event(event).is_some() {
            self.draw_bg.redraw(cx);
            self.next_frame = cx.new_next_frame();
        }
    }

    fn draw_walk(&mut self, cx: &mut Cx2d, _scope: &mut Scope, walk: Walk) -> DrawStep {
        self.draw_bg.draw_walk(cx, walk);
        if matches!(self.state, Some(State::Read(_))) {
            if let Some((ep, _, _)) = &self.pass {
                cx.make_child_pass(ep);
            }
        }
        if self.state.is_some() {
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
        if let Some(mut host) = self.ui.widget(cx, ids!(host)).borrow_mut::<KineticHost>() {
            host.start(cx, &args);
        }
    }
}

impl AppMain for App {
    fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
        makepad_widgets::script_mod(vm);
        makepad_render_graph::script_mod_passes(vm);
        makepad_kinetic_type::script_mod(vm);
        self::script_mod(vm)
    }

    fn handle_event(&mut self, cx: &mut Cx, event: &Event) {
        self.match_event(cx, event);
        self.ui.handle_event(cx, event, &mut Scope::empty());
    }
}
