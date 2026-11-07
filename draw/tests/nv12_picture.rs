//! `DrawNv12::draw_picture` on the native HAL GPU backend: an NV12 still made display RGB in a
//! texture of its own, with its mip chain built on the GPU, then sampled by later frames after
//! the pass that drew it is gone. The picture drawn 1:1 is the CPU converter's, pixel for pixel
//! (and the right way up); a one-pixel checkerboard drawn an eighth of its size is grey, which
//! only a mip chain gives (level 0 alone samples black or white there). A picture whose width
//! is no multiple of 4 (1430) is the converter's too, row for row: its rows are read at its
//! width, never at a padded one.
#![cfg(not(target_arch = "wasm32"))]

use makepad_draw::makepad_platform::video_file::{nv12::nv12_to_bgra_u32, VideoColor};
use makepad_draw::*;
use std::{cell::{Cell, RefCell}, rc::Rc};

const WIDTH: usize = 200;
const HEIGHT: usize = 100;
/// The picture compared pixel for pixel.
const A: (u32, u32) = (64, 48);
/// The checkerboard, and the side it is drawn at (device pixels), from this corner.
const B: u32 = 64;
const B_SHOWN: usize = 8;
const B_AT: usize = 100;
/// A wide picture whose rows are no multiple of 4 bytes, drawn 1:1 from this row down; the
/// window shows its left columns.
const C: (u32, u32) = (1430, 50);
const C_AT: usize = 50;

/// Limited-range bytes that vary in every direction, so a flip, a shift or a swapped chroma
/// pair shows.
fn picture_a() -> Vec<u8> {
    varied(A)
}

fn varied((w, h): (u32, u32)) -> Vec<u8> {
    let (w, h) = (w as usize, h as usize);
    let mut nv12 = vec![0u8; w * h * 3 / 2];
    for y in 0..h {
        for x in 0..w {
            nv12[y * w + x] = 16 + ((x * 7 + y * 13) % 220) as u8;
        }
    }
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            nv12[w * h + y * w + x * 2] = 16 + ((x * 11 + y * 3) % 225) as u8;
            nv12[w * h + y * w + x * 2 + 1] = 16 + ((x * 5 + y * 17) % 225) as u8;
        }
    }
    nv12
}

/// Black and white pixels in a checkerboard, chroma neutral.
fn picture_b() -> Vec<u8> {
    let s = B as usize;
    let mut nv12 = vec![128u8; s * s * 3 / 2];
    for y in 0..s {
        for x in 0..s {
            nv12[y * s + x] = if (x + y) % 2 == 0 { 16 } else { 235 };
        }
    }
    nv12
}

struct State {
    _window: WindowHandle,
    pass: DrawPass,
    list: DrawList2d,
    convert: DrawNv12,
    /// Samples its texture where it is drawn (1:1 at A).
    show: DrawQuad,
    /// The same, half a source texel off, so at an eighth of the size each output pixel's
    /// sample sits on one texel's centre, not between two.
    show_small: DrawQuad,
    sources: (Texture, Texture, Texture),
    passes: PicturePasses,
    pictures: Option<(Texture, Texture, Texture)>,
}

#[test]
fn an_nv12_still_becomes_a_mipped_picture_on_the_gpu() {
    std::env::set_var("MAKEPAD_HAL_BACKEND", "offscreen");
    let mut state: Option<State> = None;
    // The pictures are painted whole and their passes are gone: what is on screen now is
    // sampled from the kept pictures alone.
    let done = Rc::new(Cell::new(false));
    let done_in_draw = done.clone();
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            let (convert, show, show_small) = cx.with_vm(|vm| {
                makepad_platform::script::script_mod(vm);
                makepad_draw::script_mod(vm);
                let convert = DrawNv12::script_new_with_default(vm);
                let show = vm.eval(script! {
                    use mod.pod.*
                    use mod.math.*
                    use mod.shader.*
                    use mod.draw.*
                    DrawQuad{
                        image: texture_2d(float)
                        pixel: fn() {
                            return self.image.sample(self.pos)
                        }
                    }
                });
                let show_small = vm.eval(script! {
                    use mod.pod.*
                    use mod.math.*
                    use mod.shader.*
                    use mod.draw.*
                    DrawQuad{
                        image: texture_2d(float)
                        pixel: fn() {
                            return self.image.sample(self.pos + vec2(0.5 / 64.0, 0.5 / 64.0))
                        }
                    }
                });
                (convert, DrawQuad::script_from_value(vm, show), DrawQuad::script_from_value(vm, show_small))
            });
            let mut window = WindowHandle::new(cx);
            window.configure_window(cx, dvec2(WIDTH as f64, HEIGHT as f64), dvec2(0.0, 0.0), false, "NV12 picture".into());
            let pass = DrawPass::new(cx);
            window.set_pass(cx, &pass);
            pass.set_window_clear_color(cx, vec4(0.0, 0.0, 0.0, 1.0));
            let sources = (Texture::new_nv12(cx, A.0, A.1, picture_a()), Texture::new_nv12(cx, B, B, picture_b()), Texture::new_nv12(cx, C.0, C.1, varied(C)));
            let list = DrawList2d::new(cx);
            state = Some(State { _window: window, pass, list, convert, show, show_small, sources, passes: PicturePasses::default(), pictures: None });
            cx.redraw_all();
        }
        if matches!(event, Event::NextFrame(_)) {
            cx.redraw_all();
        }
        if let Event::Draw(event) = event {
            let s = state.as_mut().unwrap();
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(&s.pass, None);
            s.list.begin_always(&mut cx);
            cx.begin_root_turtle(dvec2(WIDTH as f64, HEIGHT as f64), Layout::flow_overlay());
            s.passes.next_frame(&mut cx);
            if s.pictures.is_none() {
                let a = s.convert.draw_picture(&mut cx, &mut s.passes, &s.sources.0, A.0, A.1, VideoColor::BT709);
                let b = s.convert.draw_picture(&mut cx, &mut s.passes, &s.sources.1, B, B, VideoColor::BT709);
                let c = s.convert.draw_picture(&mut cx, &mut s.passes, &s.sources.2, C.0, C.1, VideoColor::BT709);
                s.pictures = Some((a, b, c));
            }
            let (a, b, c) = s.pictures.clone().unwrap();
            // device pixels at the window's density
            let dpi = cx.current_dpi_factor();
            s.show.draw_vars.set_texture(0, &a);
            s.show.draw_abs(&mut cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(A.0 as f64 / dpi, A.1 as f64 / dpi) });
            s.show_small.draw_vars.set_texture(0, &b);
            let small = B_SHOWN as f64 / dpi;
            s.show_small.draw_abs(&mut cx, Rect { pos: dvec2(B_AT as f64 / dpi, 0.0), size: dvec2(small, small) });
            s.show.draw_vars.set_texture(0, &c);
            s.show.draw_abs(&mut cx, Rect { pos: dvec2(0.0, C_AT as f64 / dpi), size: dvec2(C.0 as f64 / dpi, C.1 as f64 / dpi) });
            let whole = s.passes.painted(&cx, &a) && s.passes.painted(&cx, &b) && s.passes.painted(&cx, &c);
            done_in_draw.set(whole && s.passes.passes() == 0);
            cx.end_pass_sized_turtle();
            s.list.end(&mut cx);
            cx.end_pass(&s.pass);
            if !done_in_draw.get() {
                let _ = cx.new_next_frame();
            }
        }
    }))));
    cx.borrow_mut().init_cx_os();
    let (w, rgba) = capture(&cx, &done);
    let mut expected = Vec::new();
    nv12_to_bgra_u32(&picture_a(), A.0, A.1, VideoColor::BT709, &mut expected);
    let (mut off, mut worst) = (0, 0u8);
    for y in 0..A.1 as usize {
        for x in 0..A.0 as usize {
            let p = &rgba[(y * w + x) * 4..(y * w + x) * 4 + 3];
            let e = expected[y * A.0 as usize + x];
            let want = [(e >> 16) as u8, (e >> 8) as u8, e as u8];
            let d = p.iter().zip(want).map(|(a, b)| a.abs_diff(b)).max().unwrap();
            worst = worst.max(d);
            off += usize::from(d > 1);
        }
    }
    let mut not_grey = 0;
    let mut grey = Vec::new();
    for y in 0..B_SHOWN {
        for x in B_AT..B_AT + B_SHOWN {
            let p = &rgba[(y * w + x) * 4..(y * w + x) * 4 + 3];
            grey.push(p[0]);
            not_grey += usize::from(p.iter().any(|c| !(60..=200).contains(c)));
        }
    }
    let mut wide = Vec::new();
    nv12_to_bgra_u32(&varied(C), C.0, C.1, VideoColor::BT709, &mut wide);
    let mut wide_off = 0;
    for y in 0..C.1 as usize {
        for x in 0..w {
            let p = &rgba[((C_AT + y) * w + x) * 4..((C_AT + y) * w + x) * 4 + 3];
            let e = wide[y * C.0 as usize + x];
            let want = [(e >> 16) as u8, (e >> 8) as u8, e as u8];
            wide_off += usize::from(p.iter().zip(want).any(|(a, b)| a.abs_diff(b) > 1));
        }
    }
    println!("nv12 picture: {off} of {} pixels off the CPU converter (worst {worst}); checkerboard at 1/8: {not_grey} of {} not grey ({:?}…); {} wide: {wide_off} of {} shown off", A.0 * A.1, B_SHOWN * B_SHOWN, &grey[..4], C.0, C.1 as usize * w);
    assert_eq!(off, 0, "the GPU conversion is the CPU converter's, the right way up");
    assert_eq!(wide_off, 0, "a picture {} wide is read at its own width, row for row", C.0);
    assert_eq!(not_grey, 0, "a minified picture samples its mip chain");
}

/// The window's pixels once `done` holds and the frame that drew it finished: its width and
/// RGBA rows, top first.
fn capture(cx: &Rc<RefCell<Cx>>, done: &Cell<bool>) -> (usize, Vec<u8>) {
    use makepad_draw::makepad_platform::makepad_hal::{self as hal_api, Hal, OpenReq, open_flags};
    let hal = Hal::global();
    let mut req = OpenReq::from_env(0, open_flags::OFFSCREEN_WINDOWS, 1.0);
    req.flags |= open_flags::OFFSCREEN_WINDOWS | open_flags::RETAIN_WINDOWS;
    let info = hal.open(&req);
    assert_ne!(info.backend, hal_api::backend_id::HEADLESS, "pixel comparisons require a GPU");
    cx.borrow_mut().hal_attach_session(info);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let pixels = loop {
        assert!(std::time::Instant::now() < deadline, "the pictures never painted whole");
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
