#![cfg(not(target_arch = "wasm32"))]

use makepad_draw::*;
use makepad_raytrace::{
    gpu::{Capture, CaptureKind},
    Camera, Material, RayTracer, RenderSettings, SceneInput,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

const WIDTH: usize = 64;
const HEIGHT: usize = 32;

struct State {
    _window: WindowHandle,
    pass: DrawPass,
    list: DrawList2d,
    tracer: RayTracer,
    /// The camera being captured, and the draw count when it was set.
    view: usize,
    switched_at: u32,
    draws: u32,
    capture_requested: bool,
}

/// A perspective camera and the tracer's own orthographic projection (no host
/// view-projection). At distance 3 the 37 degree view is 2.0 tall, so both frame the plane alike.
fn cameras() -> [(&'static str, Camera); 2] {
    let perspective = Camera {
        pos: vec3f(0.0, 0.0, 3.0),
        target: vec3f(0.0, 0.0, 0.0),
        up: vec3f(0.0, 1.0, 0.0),
        fov_y: 37.0f32.to_radians(),
        f_stop: 0.0,
        ..Default::default()
    };
    let orthographic = Camera { ortho_height: Some(2.0), ..perspective.clone() };
    [("perspective", perspective), ("orthographic", orthographic)]
}

#[test]
fn an_untraced_preview_pixel_takes_its_own_materials_albedo() {
    std::env::set_var("MAKEPAD_HAL_BACKEND", "offscreen");
    let mut state: Option<State> = None;
    let captured = Rc::new(RefCell::new(Vec::new()));
    let captured_in_event = captured.clone();
    let frames = Rc::new(Cell::new((0u32, 0u32)));
    let frames_in_draw = frames.clone();
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            let mut tracer = cx.with_vm(|vm| {
                makepad_platform::script::script_mod(vm);
                makepad_draw::script_mod(vm);
                makepad_raytrace::script_mod(vm);
                RayTracer::new(vm)
            });
            let mut scene = SceneInput {
                materials: vec![Material::diffuse([1.0, 0.0, 0.0]), Material::diffuse([0.0, 1.0, 0.0])],
                camera: cameras()[0].1.clone(),
                up: vec3f(0.0, 1.0, 0.0),
                ..Default::default()
            };
            for (material, x) in [(0, -1.0), (1, 1.0)] {
                scene.push_quad(
                    [[x - 0.8, -0.8, 0.0], [x + 0.8, -0.8, 0.0], [x + 0.8, 0.8, 0.0], [x - 0.8, 0.8, 0.0]],
                    material,
                );
            }
            tracer.set_settings(RenderSettings {
                denoise: false,
                progressive: false,
                untraced_transparent: false,
                ..Default::default()
            });
            tracer.set_scene(cx, &scene);
            tracer.set_camera(scene.camera.clone());
            tracer.set_size(WIDTH, HEIGHT);
            tracer.set_skip_trace(true);

            let mut window = WindowHandle::new(cx);
            window.configure_window(
                cx,
                dvec2(WIDTH as f64, HEIGHT as f64),
                dvec2(0.0, 0.0),
                false,
                "Preview materials".into(),
            );
            let pass = DrawPass::new(cx);
            window.set_pass(cx, &pass);
            pass.set_window_clear_color(cx, vec4(0.0, 0.0, 0.0, 1.0));
            tracer.set_parent_pass(Some(pass.draw_pass_id()));
            state = Some(State {
                _window: window,
                pass,
                list: DrawList2d::new(cx),
                tracer,
                view: 0,
                switched_at: 0,
                draws: 0,
                capture_requested: false,
            });
            cx.redraw_all();
        }
        if let Some(s) = &mut state {
            s.tracer.poll_capture(cx);
            for capture in s.tracer.take_captures() {
                if capture.kind == CaptureKind::View {
                    captured_in_event.borrow_mut().push(capture);
                    if s.view + 1 < cameras().len() {
                        s.view += 1;
                        s.tracer.set_camera(cameras()[s.view].1.clone());
                        s.switched_at = s.draws;
                        s.capture_requested = false;
                    }
                }
            }
        }
        if captured_in_event.borrow().len() == cameras().len() {
            return;
        }
        if matches!(event, Event::NextFrame(_)) {
            cx.redraw_all();
        }
        if let Event::Draw(event) = event {
            let s = state.as_mut().unwrap();
            // The tracer advances only after its previous pass chain painted whole, and a camera
            // change restarts its frame count.
            if !s.capture_requested && s.draws - s.switched_at >= 2 && s.tracer.stats.frames >= 3 && cx.pipelines_pending() == 0 {
                s.tracer.request_capture(CaptureKind::View);
                s.capture_requested = true;
            }
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(&s.pass, None);
            s.list.begin_always(&mut cx);
            cx.begin_root_turtle(dvec2(WIDTH as f64, HEIGHT as f64), Layout::flow_overlay());
            s.tracer.draw(&mut cx);
            cx.end_pass_sized_turtle();
            s.list.end(&mut cx);
            cx.end_pass(&s.pass);
            s.draws += 1;
            frames_in_draw.set((frames_in_draw.get().0 + 1, s.tracer.stats.frames));
            let _ = cx.new_next_frame();
        }
    }))));
    cx.borrow_mut().init_cx_os();
    let captures = capture_views(&cx, &captured, &frames);
    for ((name, _), capture) in cameras().iter().zip(&captures) {
        assert_eq!((capture.width, capture.height), (WIDTH, HEIGHT), "{name}: the preview capture has the requested size");
        assert_eq!(capture.bytes.len(), WIDTH * HEIGHT * 4, "{name}: the preview capture contains every pixel");
        assert_eq!(capture.spp, 0.0, "{name}: the preview is captured before any traced sample lands");
        let rgb_at = |x: usize| {
            let offset = (HEIGHT / 2 * WIDTH + x) * 4;
            let bgra = &capture.bytes[offset..offset + 4];
            [bgra[2], bgra[1], bgra[0]]
        };
        let red = |c: [u8; 3]| c[0] > c[1].saturating_add(64) && c[0] > c[2].saturating_add(64);
        let green = |c: [u8; 3]| c[1] > c[0].saturating_add(64) && c[1] > c[2].saturating_add(64);
        let left = rgb_at(WIDTH / 4);
        let right = rgb_at(WIDTH * 3 / 4);
        assert!(red(left), "{name}: the left untraced quad uses material 0's red albedo: RGB {left:?}");
        assert!(green(right), "{name}: the right untraced quad uses material 1's green albedo: RGB {right:?}");
        // The quads span columns 3.2..28.8 and 35.2..60.8 (3.3..28.8 and 35.2..60.7 in
        // perspective): a column 1.3 px inside an edge shows its quad, one 0.7 px or more
        // outside shows neither.
        for (x, want) in [(2, None), (4, Some(0)), (27, Some(0)), (30, None), (33, None), (36, Some(1)), (59, Some(1)), (62, None)] {
            let c = rgb_at(x);
            let got = if red(c) { Some(0) } else if green(c) { Some(1) } else { None };
            assert_eq!(got, want, "{name}: column {x} lies where the raster framing puts it (quad index, or none): RGB {c:?}");
        }
    }
}

fn capture_views(cx: &Rc<RefCell<Cx>>, captured: &RefCell<Vec<Capture>>, frames: &Cell<(u32, u32)>) -> Vec<Capture> {
    use makepad_platform::makepad_hal::{self as hal_api, open_flags, Hal, OpenReq};

    let hal = Hal::global();
    let mut req = OpenReq::from_env(0, open_flags::OFFSCREEN_WINDOWS, 1.0);
    req.flags |= open_flags::OFFSCREEN_WINDOWS | open_flags::RETAIN_WINDOWS;
    let info = hal.open(&req);
    assert_ne!(info.backend, hal_api::backend_id::HEADLESS, "preview colour assertions require a GPU");
    cx.borrow_mut().hal_attach_session(info);
    let mut deadline = None;
    while captured.borrow().len() < cameras().len() {
        let (drawn, recorded) = frames.get();
        if drawn > 0 {
            let at = *deadline.get_or_insert_with(|| Instant::now() + Duration::from_secs(30));
            assert!(
                Instant::now() < at,
                "preview captures did not finish: {} of {}, {drawn} draw events, {recorded} tracer frames, {} pipelines pending",
                captured.borrow().len(),
                cameras().len(),
                cx.borrow().pipelines_pending(),
            );
        }
        assert!(cx.borrow_mut().hal_session_turn(1_000_000, true));
    }
    hal.close(info.session);
    captured.take()
}
