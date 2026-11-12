//! A pass that holds linear light (`DrawPass::set_linear_light`) on the native HAL GPU backend:
//! the shaders keep writing display colour (gamma 2.2) and the output epilogue decodes it as it
//! is written. A call that blends over writes premultiplied colour: unpremultiplied, decoded and
//! premultiplied again. A call without blending (the cut-coverage edge: in an MSAA pass its alpha
//! is its coverage, its colour as it is) is decoded as it is. Alpha 0 (an additive glow) is kept.
//! The same draws into a pass without the flag keep the values the shaders wrote.
#![cfg(not(target_arch = "wasm32"))]

use makepad_draw::*;
use std::{cell::{Cell, RefCell}, rc::Rc};

/// The window (device pixels): each pass's target shown in its own half.
const WIDTH: usize = 96;
const HEIGHT: usize = 16;
/// Each pass's target: three draws side by side, a third each.
const TARGET: (f64, f64) = (24.0, 8.0);

struct Pass {
    pass: DrawPass,
    list: DrawList2d,
    texture: Texture,
}

struct State {
    _window: WindowHandle,
    window_pass: DrawPass,
    list: DrawList2d,
    /// over (premultiplied grey 0.5 at alpha 0.5), raw (grey 0.5, alpha 0.5, no blending), additive (0.25, alpha 0)
    draws: [DrawQuad; 3],
    /// Shows a target: its colour in red, its alpha in green, as they are.
    show: DrawQuad,
    linear: Pass,
    plain: Pass,
    ready_frames: u32,
}

fn target_pass(cx: &mut Cx, linear: bool) -> Pass {
    let texture = Texture::new_with_format(cx, TextureFormat::RenderRGBAf16 { size: TextureSize::Auto, initial: true });
    let pass = DrawPass::new_with_name(cx, if linear { "linear light" } else { "display colour" });
    pass.set_color_texture(cx, &texture, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
    pass.set_linear_light(cx, linear);
    Pass { pass, list: DrawList2d::new(cx), texture }
}

fn paint(cx: &mut Cx2d, p: &mut Pass, draws: &mut [DrawQuad; 3]) {
    cx.make_child_pass(&p.pass);
    cx.begin_pass(&p.pass, Some(1.0));
    p.pass.set_size(cx, dvec2(TARGET.0, TARGET.1));
    p.list.begin_always(cx);
    cx.begin_root_turtle(dvec2(TARGET.0, TARGET.1), Layout::flow_overlay());
    let third = TARGET.0 / 3.0;
    for (k, d) in draws.iter_mut().enumerate() {
        d.draw_abs(cx, Rect { pos: dvec2(third * k as f64, 0.0), size: dvec2(third, TARGET.1) });
    }
    cx.end_pass_sized_turtle();
    p.list.end(cx);
    cx.end_pass(&p.pass);
}

#[test]
fn a_linear_light_pass_decodes_each_draw_as_it_blends() {
    std::env::set_var("MAKEPAD_HAL_BACKEND", "offscreen");
    let mut state: Option<State> = None;
    let done = Rc::new(Cell::new(false));
    let done_in_draw = done.clone();
    // (frames drawn, of them in a row with every pipeline in)
    let frames = Rc::new(Cell::new((0u32, 0u32)));
    let frames_in_draw = frames.clone();
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            let (draws, show) = cx.with_vm(|vm| {
                makepad_platform::script::script_mod(vm);
                makepad_draw::script_mod(vm);
                let over = vm.eval(script! {
                    use mod.pod.*
                    use mod.math.*
                    use mod.shader.*
                    use mod.draw.*
                    DrawQuad{
                        pixel: fn() {
                            return vec4(0.25, 0.25, 0.25, 0.5)
                        }
                    }
                });
                let raw = vm.eval(script! {
                    use mod.pod.*
                    use mod.math.*
                    use mod.shader.*
                    use mod.draw.*
                    DrawQuad{
                        alpha_blend: false
                        pixel: fn() {
                            return vec4(0.5, 0.5, 0.5, 0.5)
                        }
                    }
                });
                let additive = vm.eval(script! {
                    use mod.pod.*
                    use mod.math.*
                    use mod.shader.*
                    use mod.draw.*
                    DrawQuad{
                        pixel: fn() {
                            return vec4(0.25, 0.25, 0.25, 0.0)
                        }
                    }
                });
                let show = vm.eval(script! {
                    use mod.pod.*
                    use mod.math.*
                    use mod.shader.*
                    use mod.draw.*
                    DrawQuad{
                        image: texture_2d(float)
                        pixel: fn() {
                            let c = self.image.sample(self.pos)
                            return vec4(c.x, c.w, 0.0, 1.0)
                        }
                    }
                });
                let draws = [over, raw, additive].map(|v| DrawQuad::script_from_value(vm, v));
                (draws, DrawQuad::script_from_value(vm, show))
            });
            let mut window = WindowHandle::new(cx);
            window.configure_window(cx, dvec2(WIDTH as f64, HEIGHT as f64), dvec2(0.0, 0.0), false, "Linear light".into());
            let window_pass = DrawPass::new(cx);
            window.set_pass(cx, &window_pass);
            window_pass.set_window_clear_color(cx, vec4(0.0, 0.0, 0.0, 1.0));
            let (linear, plain) = (target_pass(cx, true), target_pass(cx, false));
            let list = DrawList2d::new(cx);
            state = Some(State { _window: window, window_pass, list, draws, show, linear, plain, ready_frames: 0 });
            cx.redraw_all();
        }
        if matches!(event, Event::NextFrame(_)) {
            cx.redraw_all();
        }
        if let Event::Draw(event) = event {
            let s = state.as_mut().unwrap();
            let ready = s.draws.iter().chain([&s.show]).all(|d| d.draw_vars.draw_shader_id.is_some_and(|id| cx.draw_shader_ready(id, true)));
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(&s.window_pass, None);
            s.list.begin_always(&mut cx);
            cx.begin_root_turtle(dvec2(WIDTH as f64, HEIGHT as f64), Layout::flow_overlay());
            // the targets: child passes of the window's, recorded by its list (painted before it)
            paint(&mut cx, &mut s.linear, &mut s.draws);
            paint(&mut cx, &mut s.plain, &mut s.draws);
            // device pixels at the window's density
            let dpi = cx.current_dpi_factor();
            let half = dvec2(WIDTH as f64 / 2.0 / dpi, HEIGHT as f64 / dpi);
            for (k, t) in [&s.linear.texture, &s.plain.texture].into_iter().enumerate() {
                s.show.draw_vars.set_texture(0, t);
                s.show.new_draw_call(&mut cx);
                s.show.draw_abs(&mut cx, Rect { pos: dvec2(half.x * k as f64, 0.0), size: half });
            }
            cx.end_pass_sized_turtle();
            s.list.end(&mut cx);
            cx.end_pass(&s.window_pass);
            // a few frames once every pipeline is in: the pictures are whole
            s.ready_frames = if ready { s.ready_frames + 1 } else { 0 };
            done_in_draw.set(s.ready_frames >= 3);
            frames_in_draw.set((frames_in_draw.get().0 + 1, s.ready_frames));
            // drawn again till then (no frame in flight after it: the capture waits for its completion)
            if !done_in_draw.get() {
                let _ = cx.new_next_frame();
            }
        }
    }))));
    cx.borrow_mut().init_cx_os();
    let (w, rgba) = capture(&cx, &done, &frames);
    // per pass and draw: the colour and alpha bytes read in the middle of the draw's third
    let at = |pass: usize, draw: usize| {
        let third = WIDTH / 6;
        let (x, y) = (pass * WIDTH / 2 + draw * third + third / 2, HEIGHT / 2);
        let p = &rgba[(y * w + x) * 4..(y * w + x) * 4 + 2];
        [p[0], p[1]]
    };
    let byte = |v: f64| (v * 255.0).round() as u8;
    let decoded = 0.5f64.powf(2.2);
    // (what, got, expected [colour, alpha])
    let checks = [
        ("linear light, over: unpremultiplied, decoded, premultiplied", at(0, 0), [byte(decoded * 0.5), byte(0.5)]),
        ("linear light, no blending (the cut-coverage edge): decoded as it is", at(0, 1), [byte(decoded), byte(0.5)]),
        ("linear light, alpha 0 (additive): kept", at(0, 2), [byte(0.25), 0]),
        ("display colour, over: as written", at(1, 0), [byte(0.25), byte(0.5)]),
        ("display colour, no blending: as written", at(1, 1), [byte(0.5), byte(0.5)]),
        ("display colour, alpha 0: as written", at(1, 2), [byte(0.25), 0]),
    ];
    for (what, got, want) in &checks {
        println!("linear light: {what}: colour {} alpha {} (expected {} {})", got[0], got[1], want[0], want[1]);
    }
    for (what, got, want) in &checks {
        assert!(got.iter().zip(want).all(|(g, w)| g.abs_diff(*w) <= 2), "{what}: got colour {} alpha {}, expected {} {}", got[0], got[1], want[0], want[1]);
    }
}

/// The window's pixels once `done` holds and the frame that drew it finished: its width and
/// RGBA rows, top first.
/// The wait is 30 s from the first frame drawn (a cold start's shader builds come before it).
fn capture(cx: &Rc<RefCell<Cx>>, done: &Cell<bool>, frames: &Cell<(u32, u32)>) -> (usize, Vec<u8>) {
    use makepad_draw::makepad_platform::makepad_hal::{self as hal_api, Hal, OpenReq, open_flags};
    let hal = Hal::global();
    let mut req = OpenReq::from_env(0, open_flags::OFFSCREEN_WINDOWS, 1.0);
    req.flags |= open_flags::OFFSCREEN_WINDOWS | open_flags::RETAIN_WINDOWS;
    let info = hal.open(&req);
    assert_ne!(info.backend, hal_api::backend_id::HEADLESS, "pixel comparisons require a GPU");
    cx.borrow_mut().hal_attach_session(info);
    let mut deadline = None;
    let pixels = loop {
        if frames.get().0 > 0 {
            let at = *deadline.get_or_insert_with(|| std::time::Instant::now() + std::time::Duration::from_secs(30));
            let (drawn, ready) = frames.get();
            assert!(std::time::Instant::now() < at, "the passes never painted whole: {drawn} frames drawn, the last {ready} with every pipeline in, {} pipelines pending", cx.borrow().pipelines_pending());
        }
        let mut cx = cx.borrow_mut();
        assert!(cx.hal_session_turn(1_000_000, true));
        let submitted = cx.frame_submission_serial();
        if done.get() && submitted != 0 && cx.frame_completion_serial() == submitted && cx.pipelines_pending() == 0 {
            if let Some(pixels) = hal.window_pixels(info.session, None) {
                break pixels;
            }
        }
    };
    hal.close(info.session);
    (pixels.w as usize, pixels.rgba)
}
