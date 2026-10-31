//! The view chain's timing against the page's (`local/agent_state/skyline/reference/reference.md`), on the GPU:
//! - passes: every pass of the view again on cached CPU data (the city march and the trees included), as when a view
//!   parameter changes; cold: the zones, columns, trees and props rebuilt too; cached: only the final composite.
//! - Frames alternate: a heater (one GPU-heavy quad, so the GPU keeps its full clock as the page's warm-up did; a lone
//!   view leaves it idling at a fraction of it), then the view. Per view frame: GPU ms (the HAL's GPU timestamps of every
//!   pass packet of the frame, summed) and the draw's CPU ms (recording the passes). The HAL's frame profile (stderr, each
//!   case) adds the frame's CPU build.
//!   `MAKEPAD_HAL_BACKEND=offscreen cargo test --release -p makepad-skyline --test bench -- --include-ignored --nocapture`
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
pub use makepad_skyline::makepad_widgets;

use makepad_skyline::{params, render};

use makepad_widgets::makepad_draw::{self, *};
use std::{cell::RefCell, rc::Rc};

#[derive(Clone, Copy, PartialEq, Debug)]
enum Mode { Passes, Cold, Cached }

struct Bench {
    _window: WindowHandle, window_pass: DrawPass, root: DrawList2d, core: (render::SceneTarget, render::Skyline),
    heater: (render::SceneTarget, DrawQuad), heat: bool, case: (params::Params, render::ViewSpec), mode: Mode, draw_ms: f64,
}

impl Bench {
    fn draw(&mut self, cx: &mut Cx2d) {
        let t0 = Cx::monotonic_now();
        cx.begin_pass(&self.window_pass, None);
        self.root.begin_always(cx);
        cx.begin_root_turtle(dvec2(64.0, 64.0), Layout::flow_overlay());
        let (p, v) = &self.case;
        let rect = Rect { pos: dvec2(0.0, 0.0), size: dvec2(v.w as f64, v.h as f64) };
        if self.heat {
            let (t, h) = &mut self.heater;
            cx.make_child_pass(&t.pass);
            t.record(cx, 1024, 1024, |cx| for _ in 0..6 { h.draw_abs(cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(1024.0, 1024.0) }) });
        } else {
            let (target, s) = &mut self.core;
            if self.mode != Mode::Cached { (s.drawn, s.city.key) = (None, None); }
            if self.mode == Mode::Cold { (s.zones, s.cols, s.props.key, s.trees.mesh) = (None, None, None, None); makepad_skyline::terrain::forget_shared_trees(); }
            cx.make_child_pass(&target.pass);
            target.record(cx, v.w, v.h, |cx| s.draw_view(cx, rect, v, p, true, &makepad_skyline::SkyLight::example()));
        }
        cx.end_pass_sized_turtle();
        self.root.end(cx);
        cx.end_pass(&self.window_pass);
        self.draw_ms = (Cx::monotonic_now() - t0) * 1000.0;
    }
}

fn register(vm: &mut ScriptVm) {
    makepad_platform::script::script_mod(vm);
    makepad_draw::script_mod(vm);
    makepad_skyline::script_mod(vm);
}

fn preset(name: &str) -> params::Params {
    let path = format!("{}/../../local/agent_state/skyline/reference/params_{name}.json", env!("CARGO_MANIFEST_DIR"));
    let mut p = params::Params::default();
    p.load_web_json(&std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))).unwrap();
    p
}

fn median(mut s: Vec<f64>) -> f64 { s.sort_by(f64::total_cmp); s[s.len() / 2] }

/// One view frame: (wall, the longest turn, GPU) ms, the draw's CPU ms, the heater's GPU ms.
type Row = ((f64, f64, f64), f64, f64);

#[test]
#[ignore = "requires a native HAL GPU release build"]
fn gpu_view_times() {
    let (def, soft, heavy, off) = (preset("default"), preset("soft"), preset("props_heavy"), preset("offcentre"));
    let strip = |p: &params::Params, w| render::ViewSpec::strip(p, w);
    let detail = |p: &params::Params| render::ViewSpec::detail(p, 1600);
    let mut cases: Vec<(String, Mode, params::Params, render::ViewSpec)> = Vec::new();
    for (name, v) in [("strip 1600", strip(&def, 1600)), ("strip 4096", strip(&def, 4096)), ("detail 1600", detail(&def))] {
        for mode in [Mode::Passes, Mode::Cold, Mode::Cached] { cases.push((format!("{name} {mode:?}"), mode, def.clone(), v)); }
    }
    for (name, p, v) in [("soft strip 4096", &soft, strip(&soft, 4096)), ("soft detail 1600", &soft, detail(&soft)),
        ("props_heavy strip 4096", &heavy, strip(&heavy, 4096)), ("offcentre strip 4096", &off, strip(&off, 4096))] {
        cases.push((format!("{name} Passes"), Mode::Passes, p.clone(), v));
    }
    time_cases(cases);
}

/// The strip's GPU time at Size 0.5, 1 and 2 (the defaults, 1024 px, every pass again): at 0.5, the most elements,
/// within 1.5x of Size 1's (plan hdri-5 Task 9).
#[test]
#[ignore = "requires a native HAL GPU release build"]
fn gpu_size_times() {
    // every pass again (asserted), then cold (the zones, columns, trees and props rebuilt too: a Size drag; printed)
    let cases: Vec<(String, Mode, params::Params, render::ViewSpec)> = [(1.0, Mode::Passes), (0.5, Mode::Passes), (2.0, Mode::Passes), (1.0, Mode::Cold), (0.5, Mode::Cold), (2.0, Mode::Cold)]
        .into_iter().map(|(k, mode)| {
            let mut p = params::Params::default();
            p.set(params::P::Size as usize, k);
            let v = render::ViewSpec::strip(&p, 1024);
            (format!("size {k} strip 1024 {mode:?}"), mode, p, v)
        }).collect();
    let t = time_cases(cases);
    let (one, half) = (t[0].1, t[1].1);
    assert!(half <= 1.5 * one, "Size 0.5: {half:.3} ms of GPU, Size 1 {one:.3} ms");
}

/// Times each case (`gpu_view_times`'s frames); the medians of their GPU ms, by name.
fn time_cases(cases: Vec<(String, Mode, params::Params, render::ViewSpec)>) -> Vec<(String, f64)> {
    use makepad_platform::makepad_hal::{self as hal_api, open_flags, Hal, OpenReq};
    let bench: Rc<RefCell<Option<Bench>>> = Rc::new(RefCell::new(None));
    let handler = bench.clone();
    let first = (cases[0].2.clone(), cases[0].3);
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            let (sky, heat) = cx.with_vm(|vm| {
                register(vm);
                let v = vm.eval(script! { mod.draw.Skyline{} });
                let h = vm.eval(script! {
                    use mod.pod.*
                    use mod.math.*
                    use mod.shader.*
                    mod.draw.DrawQuad{ pixel: fn(){ var x = self.pos.x; for i in 0..3000 { x = sin(x * 1.7 + self.pos.y) } return vec4(x, x, x, 1.0) } }
                });
                (render::Skyline::script_from_value(vm, v), DrawQuad::script_from_value(vm, h))
            });
            let mut window = WindowHandle::new(cx);
            window.configure_window(cx, dvec2(64.0, 64.0), dvec2(0.0, 0.0), false, "Skyline timing".into());
            let window_pass = DrawPass::new(cx);
            window.set_pass(cx, &window_pass);
            let (core, heater) = ((render::SceneTarget::new(cx, "bench_view", false), sky), (render::SceneTarget::new(cx, "bench_heat", false), heat));
            *handler.borrow_mut() = Some(Bench { _window: window, window_pass, root: DrawList2d::new(cx), core, heater, heat: false, case: first.clone(), mode: Mode::Passes, draw_ms: 0.0 });
            cx.redraw_all();
        }
        if let Event::Draw(event) = event {
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            handler.borrow_mut().as_mut().unwrap().draw(&mut cx);
        }
    }))));
    cx.borrow_mut().init_cx_os();
    let hal = Hal::global();
    let mut req = OpenReq::from_env(0, open_flags::OFFSCREEN_WINDOWS, 1.0);
    req.flags |= open_flags::OFFSCREEN_WINDOWS | open_flags::RETAIN_WINDOWS;
    let info = hal.open(&req);
    assert_ne!(info.backend, hal_api::backend_id::HEADLESS, "the timing needs a GPU backend");
    cx.borrow_mut().hal_attach_session(info);
    println!("backend {}", info.backend);
    let gpu = || hal.with_world(|w| w.session(info.session).map(|s| s.stats.gpu_ns_total).unwrap_or_default());
    // one frame: redraw, then turns until the GPU finished it; (wall ms, the longest turn's CPU ms, GPU ms)
    let frame = |cx: &mut Cx| -> (f64, f64, f64) {
        let (t0, after, g0) = (Cx::monotonic_now(), cx.frame_submission_serial(), gpu());
        cx.redraw_all();
        let mut turn = 0.0f64;
        loop {
            let t = Cx::monotonic_now();
            assert!(cx.hal_session_turn(1_000_000, true));
            turn = turn.max(Cx::monotonic_now() - t);
            let submitted = cx.frame_submission_serial();
            if submitted > after && cx.frame_completion_serial() == submitted && cx.pipelines_pending() == 0 { break; }
        }
        let wall = (Cx::monotonic_now() - t0) * 1000.0;
        (wall, turn * 1000.0, (gpu() - g0) as f64 / 1e6)
    };
    while bench.borrow().is_none() { assert!(cx.borrow_mut().hal_session_turn(1_000_000, true)); }
    let heat = |on: bool| bench.borrow_mut().as_mut().unwrap().heat = on;
    println!("{:<30} {:>8} {:>8} {:>8}  (medians of 20 view frames, ms)", "case", "GPU", "draw", "heater");
    let mut out = vec![];
    for (name, mode, p, v) in cases {
        { let mut b = bench.borrow_mut(); let b = b.as_mut().unwrap(); b.core.1.release(); (b.case, b.mode) = ((p.clone(), v), mode); }
        let mut cx = cx.borrow_mut();
        let t = Cx::monotonic_now();
        let pair = |cx: &mut Cx| { heat(true); let h = frame(cx); heat(false); (h, frame(cx)) };
        while Cx::monotonic_now() - t < 1.5 { pair(&mut cx); }
        eprintln!("== {name}");
        hal_api::profile::set(true);
        let mut rows = Vec::new();
        for _ in 0..20 { let (h, f) = pair(&mut cx); rows.push((f, bench.borrow().as_ref().unwrap().draw_ms, h.2)); }
        hal_api::profile::set(false);
        let m = |f: &dyn Fn(&Row) -> f64| median(rows.iter().map(f).collect());
        println!("{name:<30} {:>8.3} {:>8.3} {:>8.2}  ({}x{})", m(&|r| r.0 .2), m(&|r| r.1), m(&|r| r.2), v.w, v.h);
        out.push((name, m(&|r| r.0 .2)));
    }
    hal.close(info.session);
    out
}
