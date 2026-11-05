//! The `fragment_position()` probe the GPU tests share (frag_position.rs, frag_position_gl.rs):
//! a quad drawn at clip w = 2 reads, at each of its pixels, that pixel's centre from the
//! target's top-left, the window depth `depth_of` gives and 1 / clip w.

use makepad_draw::*;
use std::{cell::RefCell, rc::Rc};

const WIDTH: usize = 320;
const HEIGHT: usize = 240;
/// The quad, in pixels: left, top, right, bottom (not centred, so a flipped row order shows).
const QUAD: [usize; 4] = [37, 23, 291, 171];

/// Draws the probe on the HAL backend `MAKEPAD_HAL_BACKEND` names (`backend`, set before the
/// HAL opens) and checks every pixel.
pub fn check(backend: &str) {
    std::env::set_var("MAKEPAD_HAL_BACKEND", backend);
    let mut state = None;
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            let quad = cx.with_vm(|vm| {
                makepad_platform::script::script_mod(vm);
                makepad_draw::script_mod(vm);
                let value = vm.eval(script! {
                    use mod.pod.*
                    use mod.math.*
                    use mod.shader.*
                    use mod.draw.*
                    DrawQuad{
                        at: fragment_position()
                        vertex: fn() {
                            // the same place, at clip w 2
                            self.vertex_pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size) * 2.0
                        }
                        // White where all three hold: red the pixel's x, green its y from the
                        // top, blue its depth and 1 / clip w.
                        pixel: fn() {
                            let px = (self.rect_pos + self.pos * self.rect_size) * self.draw_pass.dpi_factor
                            let x = 1.0 - step(0.05, abs(self.at.x - px.x))
                            let y = 1.0 - step(0.05, abs(self.at.y - px.y))
                            let z = 1.0 - step(0.0001, abs(self.at.z - self.depth_of(self.world)))
                            let w = 1.0 - step(0.0001, abs(self.at.w - 0.5))
                            return vec4(x, y, z * w, 1.0)
                        }
                    }
                });
                DrawQuad::script_from_value(vm, value)
            });
            let mut window = WindowHandle::new(cx);
            window.configure_window(cx, dvec2(WIDTH as f64, HEIGHT as f64), dvec2(0.0, 0.0), false, "Fragment position".into());
            let pass = DrawPass::new(cx);
            window.set_pass(cx, &pass);
            pass.set_window_clear_color(cx, vec4(0.0, 0.0, 0.0, 1.0));
            state = Some((window, pass, DrawList2d::new(cx), quad));
            cx.redraw_all();
        }
        if let Event::Draw(event) = event {
            let (_, pass, list, quad) = state.as_mut().unwrap();
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(pass, None);
            list.begin_always(&mut cx);
            cx.begin_root_turtle(dvec2(WIDTH as f64, HEIGHT as f64), Layout::flow_overlay());
            let [left, top, right, bottom] = QUAD.map(|v| v as f64);
            quad.draw_abs(&mut cx, Rect { pos: dvec2(left, top), size: dvec2(right - left, bottom - top) });
            cx.end_pass_sized_turtle();
            list.end(&mut cx);
            cx.end_pass(pass);
        }
    }))));
    cx.borrow_mut().init_cx_os();
    let (opened, w, h, rgba) = capture(&cx);
    use makepad_draw::makepad_platform::makepad_hal::backend_id;
    let expected = match backend {
        "gl" => backend_id::GL,
        _ if cfg!(target_vendor = "apple") => backend_id::METAL,
        _ if cfg!(windows) => backend_id::D3D11,
        _ => backend_id::VULKAN,
    };
    assert_eq!(opened, expected, "MAKEPAD_HAL_BACKEND={backend} opened another backend");
    let dpi = w / WIDTH;
    assert_eq!((w, h), (WIDTH * dpi, HEIGHT * dpi), "the window's pixels");
    let [left, top, right, bottom] = QUAD.map(|v| v * dpi);
    let (mut wrong_x, mut wrong_y, mut wrong_zw, mut outside) = (0, 0, 0, 0);
    for y in 0..h {
        for x in 0..w {
            let p = &rgba[(y * w + x) * 4..(y * w + x) * 4 + 3];
            if x >= left && x < right && y >= top && y < bottom {
                wrong_x += usize::from(p[0] < 128);
                wrong_y += usize::from(p[1] < 128);
                wrong_zw += usize::from(p[2] < 128);
            } else {
                outside += usize::from(p != [0, 0, 0]);
            }
        }
    }
    let inside = (right - left) * (bottom - top);
    println!("fragment_position ({backend}, HAL backend {opened}): {inside} pixels at dpi {dpi}: x wrong at {wrong_x}, y at {wrong_y}, depth or w at {wrong_zw}; {outside} drawn outside the quad");
    assert_eq!((wrong_x, wrong_y, wrong_zw, outside), (0, 0, 0, 0), "fragment_position() off its convention");
}

/// The window's first finished frame: the backend that drew it, its size and RGBA rows, top
/// first.
fn capture(cx: &Rc<RefCell<Cx>>) -> (u32, usize, usize, Vec<u8>) {
    use makepad_draw::makepad_platform::makepad_hal::{self as hal_api, Hal, OpenReq, open_flags};
    let hal = Hal::global();
    let mut req = OpenReq::from_env(0, open_flags::OFFSCREEN_WINDOWS, 1.0);
    req.flags |= open_flags::OFFSCREEN_WINDOWS | open_flags::RETAIN_WINDOWS;
    let info = hal.open(&req);
    assert_ne!(info.backend, hal_api::backend_id::HEADLESS, "pixel comparisons require a GPU");
    cx.borrow_mut().hal_attach_session(info);
    let pixels = loop {
        let mut cx = cx.borrow_mut();
        assert!(cx.hal_session_turn(1_000_000, true));
        let submitted = cx.frame_submission_serial();
        if submitted != 0 && cx.frame_completion_serial() == submitted && cx.pipelines_pending() == 0 {
            if let Some(pixels) = hal.window_pixels(info.session, None) {
                break pixels;
            }
        }
    };
    hal.close(info.session);
    (info.backend, pixels.w as usize, pixels.h as usize, pixels.rgba)
}
