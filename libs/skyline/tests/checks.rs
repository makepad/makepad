//! Checks of the skyline core against the web page, outside the app:
//! - `scene_helpers_compile_for_every_backend` (no GPU): the scene helpers' shader code for Metal,
//!   HLSL and GLSL ES;
//! - `gpu_hashes_and_views_match_the_cpu` (ignored, needs a GPU): `hcell`, `rnd` and `hb4` drawn by
//!   the scene shader helpers and read back as RGBA32F equal `rng.rs` bit for bit, and `pix_view` and
//!   `city_view` match an f64 reference. `MAKEPAD_HAL_BACKEND=offscreen cargo test --release -p
//!   makepad-skyline --test checks -- --include-ignored --nocapture`;
//! - `dump_layout`: with `SKYLINE_DUMP=FILE` (and `SKYLINE_PARAMS=FILE`, the page's saved
//!   parameters) writes the zones and the strip's columns as JSON for `check_layout.js`.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
pub use makepad_skyline::makepad_widgets;

use makepad_skyline::{params, render, rng, shaders, terrain, zones};

use makepad_widgets::makepad_draw::{self, *};
use rng::{fract, Rng, TAU};
use std::{cell::RefCell, rc::Rc, sync::{Arc, Mutex}};

#[test]
fn dump_layout() {
    let Ok(path) = std::env::var("SKYLINE_DUMP") else { return };
    let mut p = params::Params::default();
    if let Ok(file) = std::env::var("SKYLINE_PARAMS") {
        p.load_web_json(&std::fs::read_to_string(&file).unwrap()).unwrap();
    }
    let z = zones::Zones::new(&p);
    let named: Vec<String> = z.named().iter().map(|(k, v)| format!("\"{k}\":[{},{},{},{}]", v[0], v[1], v[2], v[3])).collect();
    let view = render::ViewSpec::strip(&p, 1600);
    let m = terrain::margin(&p, view.span, view.w);
    let cols: Vec<String> = terrain::columns(&z, &p, view.u0, view.span, view.w, m).iter().map(|c| format!("{},{},{},{}", c[0], c[1], c[2], c[3])).collect();
    std::fs::write(&path, format!("{{\"zones\":{{{}}},\"m\":{m},\"columns\":[{}]}}", named.join(","), cols.join(","))).unwrap();
}

/// The check target: N x N texels, the left half drawn at seed 7, the right at 9999999.
const N: usize = 16;
const SEEDS: [u32; 2] = [7, 9_999_999];
/// The view: from azimuth 0.1, half a turn wide, horizon at mid-height; the camera 0.6 off-centre.
const SPAN: [f64; 2] = [0.1, 0.5];
const OFF: f64 = 0.6;
const OFF_DIR: f64 = 0.3;
const CT_R: f64 = 1.3;

/// Texel (x, y): fb0 = hcell's low and high 16 bits, rnd * 2^24, hb4(hcell).z; fb1 = pix_view of
/// the raw view, city_view's azimuth of the cell (as metres). Cells and ids go negative, so the
/// i32 truncation and the u32 wrap are both exercised.
fn eval_check(vm: &mut ScriptVm) -> ScriptValue {
    vm.eval(script! {
        use mod.pod.*
        use mod.math.*
        use mod.shader.*
        use mod.draw.*
        DrawQuad{
            ..mod.draw.SkyScene,
            alpha_blend: false
            fragment: fn(){
                let pp = floor(self.frag())
                let c = vec2((pp.x - 8.0) * 1237.0 + pp.y * 3.0, (pp.y - 8.0) * 911.0 - pp.x)
                let s = pp.x + pp.y
                let h = self.hcell(c, s - 5.0 * floor(s / 5.0))
                let r = self.rnd(c.x * 0.5 + 0.25, pp.y - 3.0, pp.x * 7.0)
                self.fb0 = vec4(f32(h & 65535u), f32(h >> 16u), r * 16777216.0, self.hb4(h).z)
                let v = self.pix_view(self.view_raw())
                self.fb1 = vec4(v.x, v.y, v.z, self.city_view(c).x)
            }
        }
    })
}

fn register(vm: &mut ScriptVm) {
    makepad_platform::script::script_mod(vm);
    makepad_draw::script_mod(vm);
    shaders::script_mod(vm);
}

#[test]
fn scene_helpers_compile_for_every_backend() {
    use makepad_draw::makepad_script::{shader_backend::ShaderBackend, shader_compile::{compile_shader, ShaderCompileOptions}};
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        register(vm);
        let shader = eval_check(vm).as_object().unwrap();
        for (backend, gles) in [(ShaderBackend::Metal, false), (ShaderBackend::Hlsl, false), (ShaderBackend::Glsl, true)] {
            let mut opts = ShaderCompileOptions::new(backend, false, false, false, false);
            opts.gles_complete = gles;
            let output = compile_shader(vm, shader, &opts);
            assert!(!output.has_errors(), "{backend:?}: {}", output.view().error_report);
        }
    });
}

/// The CPU's answer for texel (x, y): fb0 exactly, fb1 in f64.
fn expect(x: usize, y: usize) -> ([f32; 4], [f64; 4]) {
    let rng = Rng::new(SEEDS[x * 2 / N]);
    let (px, py) = (x as f64, y as f64);
    let c = ((px - 8.0) * 1237.0 + py * 3.0, (py - 8.0) * 911.0 - px);
    let h = rng.hcell(c.0 as i32, c.1 as i32, ((x + y) % 5) as i32);
    let r = rng.rnd(c.0 * 0.5 + 0.25, py - 3.0, px * 7.0);
    let fb0 = [(h & 65535) as f32, (h >> 16) as f32, (r * 16777216.0) as f32, ((h >> 16) & 255) as f32 * 0.003921569];
    // the page's pixView at the texel's centre
    let (u, px0) = (fract(SPAN[0] + (px + 0.5) / N as f64 * SPAN[1]), SPAN[1] * TAU / N as f64);
    let ang = (0.5 * N as f64 - (py + 0.5)) * px0;
    let (d, cc) = (((u * TAU).cos(), (u * TAU).sin()), (OFF * (OFF_DIR * TAU).cos(), OFF * (OFF_DIR * TAU).sin()));
    let cd = cc.0 * d.0 + cc.1 * d.1;
    let t = -cd + (cd * cd - (cc.0 * cc.0 + cc.1 * cc.1) + 1.0).max(1e-4).sqrt();
    let q = (cc.0 + t * d.0, cc.1 + t * d.1);
    // prCityView: the camera 0.8 off cityD blocks of 70 m
    let cam = 0.8 * OFF * 28.0 * CT_R * 70.0;
    let w = (c.0 - cam * (OFF_DIR * TAU).cos(), c.1 - cam * (OFF_DIR * TAU).sin());
    (fb0, [fract(q.1.atan2(q.0) / TAU), ang * t, px0 * t, fract(w.1.atan2(w.0) / TAU)])
}

struct Check {
    _window: WindowHandle,
    window_pass: DrawPass,
    root: DrawList2d,
    pass: DrawPass,
    list: DrawList2d,
    targets: [Texture; 2],
    draw: DrawQuad,
}

impl Check {
    fn new(cx: &mut Cx) -> Self {
        let draw = cx.with_vm(|vm| {
            register(vm);
            let v = eval_check(vm);
            DrawQuad::script_from_value(vm, v)
        });
        let mut window = WindowHandle::new(cx);
        window.configure_window(cx, dvec2(64.0, 64.0), dvec2(0.0, 0.0), false, "Skyline checks".into());
        let window_pass = DrawPass::new(cx);
        window.set_pass(cx, &window_pass);
        let pass = DrawPass::new_with_name(cx, "skyline_check");
        pass.set_size(cx, dvec2(N as f64, N as f64));
        let targets = std::array::from_fn(|_| {
            let t = Texture::new_with_format(cx, TextureFormat::RenderRGBAf32 { size: TextureSize::Fixed { width: N, height: N }, initial: true });
            pass.add_color_texture(cx, &t, DrawPassClearColor::ClearWith(vec4(-1.0, -1.0, -1.0, -1.0)));
            t
        });
        Self { _window: window, window_pass, root: DrawList2d::new(cx), pass, list: DrawList2d::new(cx), targets, draw }
    }

    fn draw(&mut self, cx: &mut Cx2d) {
        cx.begin_pass(&self.window_pass, None);
        self.root.begin_always(cx);
        cx.begin_root_turtle(dvec2(64.0, 64.0), Layout::flow_overlay());
        cx.make_child_pass(&self.pass);
        cx.begin_pass(&self.pass, Some(1.0));
        self.list.begin_always(cx);
        cx.begin_root_turtle(dvec2(N as f64, N as f64), Layout::flow_overlay());
        let vars = &mut self.draw.draw_vars;
        vars.set_uniform(cx, live_id!(view_size), &[N as f32, N as f32]);
        vars.set_uniform(cx, live_id!(view_span), &[SPAN[0] as f32, SPAN[1] as f32]);
        vars.set_uniform(cx, live_id!(view_horizon), &[0.5]);
        vars.set_uniform(cx, live_id!(off), &[OFF as f32]);
        vars.set_uniform(cx, live_id!(off_dir), &[OFF_DIR as f32]);
        vars.set_uniform(cx, live_id!(ct_r), &[CT_R as f32]);
        for (i, seed) in SEEDS.iter().enumerate() {
            self.draw.draw_vars.set_uniform(cx, live_id!(seed), &[*seed as f32]);
            let half = (N / 2) as f64;
            self.draw.draw_abs(cx, Rect { pos: dvec2(i as f64 * half, 0.0), size: dvec2(half, N as f64) });
        }
        cx.end_pass_sized_turtle();
        self.list.end(cx);
        cx.end_pass(&self.pass);
        cx.end_pass_sized_turtle();
        self.root.end(cx);
        cx.end_pass(&self.window_pass);
    }
}

type Texels = Result<Vec<[f32; 4]>, String>;

#[test]
#[ignore = "requires a native HAL GPU release build"]
fn gpu_hashes_and_views_match_the_cpu() {
    use makepad_platform::makepad_hal::{self as hal_api, open_flags, Hal, OpenReq};
    let check: Rc<RefCell<Option<Check>>> = Rc::new(RefCell::new(None));
    let handler = check.clone();
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            *handler.borrow_mut() = Some(Check::new(cx));
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
    assert_ne!(info.backend, hal_api::backend_id::HEADLESS, "the check needs a GPU backend");
    cx.borrow_mut().hal_attach_session(info);
    println!("backend {}", info.backend);

    // paint until no pipeline is compiling and two reads agree (Metal adds a pipeline variant
    // for a new target layout on first use and paints again)
    let mut turns = 0;
    let mut turn = |cx: &mut Cx| {
        turns += 1;
        assert!(turns < 60_000, "timed out");
        assert!(cx.hal_session_turn(1_000_000, true));
    };
    let mut last: Option<Vec<Texels>> = None;
    let read = loop {
        let mut cx = cx.borrow_mut();
        let after = cx.frame_submission_serial();
        cx.redraw_all();
        loop {
            turn(&mut cx);
            let submitted = cx.frame_submission_serial();
            if submitted > after && cx.frame_completion_serial() == submitted && cx.pipelines_pending() == 0 {
                break;
            }
        }
        let slots: Arc<Mutex<Vec<Option<Texels>>>> = Arc::new(Mutex::new(vec![None, None]));
        for (i, texture) in check.borrow().as_ref().unwrap().targets.iter().enumerate() {
            let slots = slots.clone();
            cx.read_render_texture(texture, move |result| {
                let texels = result.map_err(|e| format!("{e:?}")).map(|(_, _, bytes)| {
                    bytes.as_chunks::<16>().0.iter().map(|t| std::array::from_fn(|c| f32::from_le_bytes(t[c * 4..c * 4 + 4].try_into().unwrap()))).collect()
                });
                slots.lock().unwrap()[i] = Some(texels);
            })
            .expect("readback request");
        }
        while slots.lock().unwrap().iter().any(Option::is_none) {
            turn(&mut cx);
        }
        let now: Vec<Texels> = slots.lock().unwrap().iter().map(|t| t.clone().unwrap()).collect();
        if last.as_ref() == Some(&now) {
            break now;
        }
        last = Some(now);
    };
    hal.close(info.session);

    let (fb0, fb1) = (read[0].as_ref().unwrap(), read[1].as_ref().unwrap());
    let (mut bad, mut worst) = (Vec::new(), [0f64; 4]);
    for y in 0..N {
        for x in 0..N {
            let (want0, want1) = expect(x, y);
            let (got0, got1) = (fb0[y * N + x], fb1[y * N + x]);
            if got0 != want0 {
                bad.push(format!("({x}, {y}) hashes: GPU {got0:?}, CPU {want0:?}"));
            }
            for c in 0..4 {
                let mut d = got1[c] as f64 - want1[c];
                if c == 0 || c == 3 {
                    d -= d.round(); // azimuths wrap
                }
                let rel = d.abs() / want1[c].abs().max(if c == 0 || c == 3 { 1.0 } else { 1e-3 });
                worst[c] = worst[c].max(rel);
            }
        }
    }
    println!("hashes: {} of {} texels differ; views: worst relative error u {:.1e}, ang {:.1e}, px {:.1e}, city azimuth {:.1e}", bad.len(), N * N, worst[0], worst[1], worst[2], worst[3]);
    assert!(bad.is_empty(), "{bad:#?}");
    assert!(worst.iter().all(|e| *e < 1e-5), "views off: {worst:?}");
}
