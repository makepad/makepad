//! Checks of the city:
//! - `city_shaders_compile_for_every_backend` (no GPU): the march, the blit and a prop-like shader reading the city's
//!   depth compile for Metal, HLSL and GLSL ES;
//! - `gpu_march_time` (ignored, needs a GPU): the march's GPU time per view, as the frame time of 2K marches less that
//!   of K, over K, after a warm-up (the page's own figure is its city program under a timer query).
//!   `MAKEPAD_HAL_BACKEND=offscreen cargo test --release -p makepad-skyline --test city -- --include-ignored --nocapture`
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
pub use makepad_skyline::makepad_widgets;

use makepad_skyline::{city, light, params, render, shaders};

use makepad_widgets::makepad_draw::{self, *};
use std::{cell::RefCell, rc::Rc};

#[test]
fn city_shaders_compile_for_every_backend() {
    use makepad_draw::makepad_script::{shader_backend::ShaderBackend, shader_compile::{compile_shader, ShaderCompileOptions}};
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_platform::script::script_mod(vm);
        makepad_draw::script_mod(vm);
        shaders::script_mod(vm);
        city::script_mod(vm);
        let shaders = [
            ("march", vm.eval(script! { mod.draw.DrawSkyCity{} })),
            ("blit", vm.eval(script! { mod.draw.DrawSkyCityBlit{} })),
            ("hide", vm.eval(script! {
                use mod.pod.*
                use mod.math.*
                use mod.shader.*
                mod.draw.DrawQuad{
                    ..mod.draw.SkyScene,
                    ..mod.draw.SkyCityDepth,
                    fragment: fn(){ let h = self.city_hide(300.0); self.fb0 = vec4(h); self.fb1 = vec4(h) }
                }
            })),
        ];
        for (name, shader) in shaders {
            let shader = shader.as_object().unwrap_or_else(|| panic!("{name}: not an object: {shader:?}"));
            for (backend, gles) in [(ShaderBackend::Metal, false), (ShaderBackend::Hlsl, false), (ShaderBackend::Glsl, true)] {
                let mut opts = ShaderCompileOptions::new(backend, false, false, false, false);
                opts.gles_complete = gles;
                let output = compile_shader(vm, shader, &opts);
                assert!(!output.has_errors(), "{name} {backend:?}: {}", output.view().error_report);
            }
        }
    });
}

fn register(vm: &mut ScriptVm) {
    makepad_platform::script::script_mod(vm);
    makepad_draw::script_mod(vm);
    shaders::script_mod(vm);
    city::script_mod(vm);
}

/// One timed case: the march over the view (its product rows, or the whole view as the page draws it), `k` times.
struct Bench { _window: WindowHandle, window_pass: DrawPass, root: DrawList2d, target: render::SceneTarget, draw: city::DrawSkyCity, ring: Texture, case: (params::Params, render::ViewSpec, bool), k: usize }

impl Bench {
    fn draw(&mut self, cx: &mut Cx2d) {
        cx.begin_pass(&self.window_pass, None);
        self.root.begin_always(cx);
        cx.begin_root_turtle(dvec2(64.0, 64.0), Layout::flow_overlay());
        cx.make_child_pass(&self.target.pass);
        let ((p, v, full), k, draw, ring) = (&self.case, self.k, &mut self.draw, &self.ring);
        let rows = if *full { (0.0, v.h as f64) } else { let (t, b) = city::reach(p).unwrap(); ((v.row_of(t) - 4.0).floor().max(0.0), (v.row_of(b) + 4.0).ceil().min(v.h as f64)) };
        self.target.record(cx, v.w, v.h, |cx| {
            let vars = &mut draw.draw_vars;
            p.set_uniforms(cx, vars);
            vars.set_uniform(cx, live_id!(view_size), &[v.w as f32, v.h as f32]);
            vars.set_uniform(cx, live_id!(view_span), &[v.u0 as f32, v.span as f32]);
            vars.set_uniform(cx, live_id!(view_horizon), &[v.horizon as f32]);
            vars.set_uniform(cx, live_id!(sil), &params::SIL);
            render::bind_texture(cx, vars, live_id!(ring_tex), ring); // the example's night: no sun, the fixed haze
            for _ in 0..k { draw.draw_abs(cx, Rect { pos: dvec2(0.0, rows.0), size: dvec2(v.w as f64, rows.1 - rows.0) }); }
        });
        cx.end_pass_sized_turtle();
        self.root.end(cx);
        cx.end_pass(&self.window_pass);
    }
}

#[test]
#[ignore = "requires a native HAL GPU release build"]
fn gpu_march_time() {
    use makepad_platform::makepad_hal::{self as hal_api, open_flags, Hal, OpenReq};
    let mut big = params::Params::default();
    for (k, x) in [(params::P::CtH, 2.2), (params::P::CtD, 1.0), (params::P::CtW, 0.6), (params::P::CtR, 0.6)] { big.v[k as usize] = x; }
    let def = params::Params::default();
    let mut cases = Vec::new();
    for (name, p) in [("default", &def), ("big", &big)] {
        for (view, v) in [("strip 1600", render::ViewSpec::strip(p, 1600)), ("detail 1600", render::ViewSpec::detail(p, 1600)), ("strip 4096", render::ViewSpec::strip(p, 4096))] {
            for full in [false, true] { cases.push((format!("{name} {view} {}", if full { "whole view" } else { "city rows" }), (p.clone(), v, full))); }
        }
    }
    let bench: Rc<RefCell<Option<Bench>>> = Rc::new(RefCell::new(None));
    let handler = bench.clone();
    let first = cases[0].1.clone();
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            let draw = cx.with_vm(|vm| { register(vm); let v = vm.eval(script! { mod.draw.DrawSkyCity{} }); city::DrawSkyCity::script_from_value(vm, v) });
            let mut window = WindowHandle::new(cx);
            window.configure_window(cx, dvec2(64.0, 64.0), dvec2(0.0, 0.0), false, "City timing".into());
            let window_pass = DrawPass::new(cx);
            window.set_pass(cx, &window_pass);
            let target = render::SceneTarget::new(cx, "city_bench", true);
            let format = TextureFormat::VecRGBAf32 { width: light::RING, height: 2, data: Some(light::SkyLight::example().ring_texels()), updated: TextureUpdated::Full };
            let ring = Texture::new_with_format(cx, format);
            *handler.borrow_mut() = Some(Bench { _window: window, window_pass, root: DrawList2d::new(cx), target, draw, ring, case: first.clone(), k: 1 });
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
    // one frame: record, paint, wait for the GPU; its wall time in ms
    let frame = |cx: &mut Cx| -> f64 {
        let (t0, after) = (Cx::monotonic_now(), cx.frame_submission_serial());
        cx.redraw_all();
        loop {
            assert!(cx.hal_session_turn(1_000_000, true));
            let submitted = cx.frame_submission_serial();
            if submitted > after && cx.frame_completion_serial() == submitted && cx.pipelines_pending() == 0 { break; }
        }
        (Cx::monotonic_now() - t0) * 1000.0
    };
    while bench.borrow().is_none() { assert!(cx.borrow_mut().hal_session_turn(1_000_000, true)); }
    // the bench marches the real city: the default detail view's coverage, read back
    {
        { let mut b = bench.borrow_mut(); let b = b.as_mut().unwrap(); (b.case, b.k) = (cases[2].1.clone(), 1); }
        let mut cx = cx.borrow_mut();
        frame(&mut cx);
        let got: std::sync::Arc<std::sync::Mutex<Option<(usize, usize)>>> = Default::default();
        let slot = got.clone();
        let tex = bench.borrow().as_ref().unwrap().target.color.clone();
        cx.read_render_texture(&tex, move |r| {
            let (w, h, bytes) = r.expect("readback");
            let a = bytes.as_chunks::<8>().0.iter().filter(|t| makepad_draw::makepad_script::pod_heap::f16_to_f32(u16::from_le_bytes([t[6], t[7]])) > 0.5).count();
            *slot.lock().unwrap() = Some((a, w * h));
        }).expect("readback request");
        while got.lock().unwrap().is_none() { assert!(cx.hal_session_turn(1_000_000, true)); }
        let (covered, all) = got.lock().unwrap().unwrap();
        println!("{}: {covered} of {all} texels more than half covered", cases[2].0);
        assert!(covered > all / 50, "the bench drew no city");
    }
    // frames are paced (60 Hz), so K grows until a frame takes over 40 ms; the slope from K to 2K is one march
    for (name, case) in cases {
        let time = |k: usize| -> f64 {
            { let mut b = bench.borrow_mut(); let b = b.as_mut().unwrap(); (b.case, b.k) = (case.clone(), k); }
            let mut cx = cx.borrow_mut();
            let t = Cx::monotonic_now();
            while Cx::monotonic_now() - t < 1.0 { frame(&mut cx); }
            let mut s: Vec<f64> = (0..15).map(|_| frame(&mut cx)).collect();
            s.sort_by(f64::total_cmp);
            s[7]
        };
        let mut k = 16;
        let mut t1 = time(k);
        while t1 < 40.0 && k < 8192 { k *= 2; t1 = time(k); }
        let t2 = time(2 * k);
        println!("{name}: {:.4} ms a march ({k} marches {t1:.1} ms, {} marches {t2:.1} ms)", (t2 - t1) / k as f64, 2 * k);
    }
    hal.close(info.session);
}
