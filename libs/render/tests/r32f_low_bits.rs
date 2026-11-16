//! An R32F render target keeps every bit of a normal float a pixel shader writes, and a nearest
//! sample in a later pass reads the same bits back: the screen AO's prepass packs a 10-bit
//! normal below a 22-bit depth code, and every later pass unpacks it from the texel.
//! 256 x 256 texels, each a normal float built from integers: either sign, exponent fields
//! 1..=254 (2^-126 through 2^127), 13 hashed mantissa bits, and the texel's index in the
//! low 10 bits (every value 64 times). Sign, exponent and mantissa come from a mixed hash, so
//! every sign and exponent pair appears (96 to 168 times) and each of the low 10 bits takes
//! both values within every pair: no low bit is tied to the exponent or sign.
//! The window shows each texel's four bytes (bytes 0-2 as RGB on the left half, byte 3 as R
//! on the right), compared exactly on the CPU. Native HAL GPU backend.
#![cfg(not(target_arch = "wasm32"))]

use makepad_draw::*;
use std::{cell::{Cell, RefCell}, rc::Rc};

/// The written texture's side (texels), and the window: its two halves side by side.
const SIDE: usize = 256;
const WIDTH: usize = SIDE * 2;
const HEIGHT: usize = SIDE;

/// The bits texel (x, y) holds, as the writing pass builds them.
fn bits(x: usize, y: usize) -> u32 {
    let idx = (y * SIDE + x) as u32;
    let hash = idx.wrapping_mul(2654435761);
    let mixed = (hash ^ (hash >> 15)).wrapping_mul(0x2c1b3c6d);
    let mixed = mixed ^ (mixed >> 12);
    let exponent = 1 + ((mixed >> 13) & 0x3ffff) % 254;
    ((mixed >> 31) << 31) | (exponent << 23) | ((mixed & 0x1fff) << 10) | (idx & 1023)
}

struct State {
    _window: WindowHandle,
    pass: DrawPass,
    list: DrawList2d,
    write_pass: DrawPass,
    write_list: DrawList2d,
    write: DrawQuad,
    read: DrawQuad,
    _target: Texture,
    ready_frames: u32,
}

#[test]
fn an_r32f_target_keeps_every_low_mantissa_bit() {
    // The pattern covers what the doc says before the GPU sees it.
    let mut seen = vec![[[false; 2]; 10]; 512];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let b = bits(x, y);
            for bit in 0..10 {
                seen[(b >> 23) as usize][bit][((b >> bit) & 1) as usize] = true;
            }
        }
    }
    for (pair, bits_seen) in seen.iter().enumerate() {
        let exponent = pair & 255;
        let used = (1..=254).contains(&exponent);
        for (bit, values) in bits_seen.iter().enumerate() {
            assert_eq!(
                values[0] && values[1],
                used,
                "sign {} exponent {exponent}: low bit {bit} must take both values exactly when the exponent is normal",
                pair >> 8
            );
        }
    }
    std::env::set_var("MAKEPAD_HAL_BACKEND", "offscreen");
    let mut state: Option<State> = None;
    let done = Rc::new(Cell::new(false));
    let done_in_draw = done.clone();
    let frames = Rc::new(Cell::new((0u32, 0u32)));
    let frames_in_draw = frames.clone();
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            let (write, read) = cx.with_vm(|vm| {
                makepad_platform::script::script_mod(vm);
                makepad_draw::script_mod(vm);
                (
                    vm.eval(script! {
                        use mod.pod.*
                        use mod.math.*
                        use mod.shader.*
                        use mod.draw.*
                        DrawQuad{
                            pixel: fn() {
                                let p = floor(self.pos * vec2(256.0, 256.0))
                                let idx = u32(p.y) * 256u + u32(p.x)
                                let hash = idx * 2654435761u
                                var mixed = (hash ^ (hash >> 15u)) * 739982445u
                                mixed = mixed ^ (mixed >> 12u)
                                let exponent = 1u + ((mixed >> 13u) & 262143u) % 254u
                                let b = ((mixed >> 31u) << 31u) | (exponent << 23u) | ((mixed & 8191u) << 10u) | (idx & 1023u)
                                return vec4(asfloat(b), 0.0, 0.0, 1.0)
                            }
                        }
                    }),
                    vm.eval(script! {
                        use mod.pod.*
                        use mod.math.*
                        use mod.shader.*
                        use mod.draw.*
                        DrawQuad{
                            image: texture_2d(float)
                            pixel: fn() {
                                let p = floor(self.pos * vec2(512.0, 256.0))
                                let x = p.x - 256.0 * step(256.0, p.x)
                                let b = asuint(self.image.sample_nearest((vec2(x, p.y) + vec2(0.5, 0.5)) / vec2(256.0, 256.0)).x)
                                if p.x >= 256.0 {
                                    return vec4(f32(b >> 24u) / 255.0, 0.0, 0.0, 1.0)
                                }
                                return vec4(f32(b & 255u) / 255.0, f32((b >> 8u) & 255u) / 255.0, f32((b >> 16u) & 255u) / 255.0, 1.0)
                            }
                        }
                    }),
                )
            });
            let write = cx.with_vm(|vm| DrawQuad::script_from_value(vm, write));
            let mut read = cx.with_vm(|vm| DrawQuad::script_from_value(vm, read));
            let target = Texture::new_with_format(cx, TextureFormat::RenderRf32 { size: TextureSize::Auto, initial: true });
            read.draw_vars.set_texture(0, &target);
            let mut window = WindowHandle::new(cx);
            window.configure_window(cx, dvec2(WIDTH as f64, HEIGHT as f64), dvec2(0.0, 0.0), false, "R32F low bits".into());
            let pass = DrawPass::new(cx);
            window.set_pass(cx, &pass);
            pass.set_window_clear_color(cx, vec4(0.0, 0.0, 1.0, 1.0));
            let write_pass = DrawPass::new(cx);
            write_pass.set_color_texture(cx, &target, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 0.0)));
            state = Some(State {
                _window: window,
                pass,
                list: DrawList2d::new(cx),
                write_pass,
                write_list: DrawList2d::new(cx),
                write,
                read,
                _target: target,
                ready_frames: 0,
            });
            cx.redraw_all();
        }
        if matches!(event, Event::NextFrame(_)) {
            cx.redraw_all();
        }
        if let Event::Draw(event) = event {
            let s = state.as_mut().unwrap();
            let ready = [&s.write, &s.read].iter().all(|d| d.draw_vars.draw_shader_id.is_some_and(|id| cx.draw_shader_ready(id, true)));
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(&s.pass, None);
            s.list.begin_always(&mut cx);
            let dpi = cx.current_dpi_factor();
            let side = SIDE as f64 / dpi;
            // the written texture: a pass of its own that the window's pass reads after it
            s.write_pass.set_pass_parent(cx.cx, s.pass.draw_pass_id());
            cx.begin_pass(&s.write_pass, None);
            s.write_pass.set_size(&mut cx, dvec2(side, side));
            s.write_list.begin_always(&mut cx);
            cx.begin_root_turtle(dvec2(side, side), Layout::flow_overlay());
            s.write.draw_abs(&mut cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(side, side) });
            cx.end_pass_sized_turtle();
            s.write_list.end(&mut cx);
            cx.end_pass(&s.write_pass);
            cx.begin_root_turtle(dvec2(WIDTH as f64, HEIGHT as f64), Layout::flow_overlay());
            s.read.draw_abs(&mut cx, Rect { pos: dvec2(0.0, 0.0), size: dvec2(WIDTH as f64 / dpi, HEIGHT as f64 / dpi) });
            cx.end_pass_sized_turtle();
            s.list.end(&mut cx);
            cx.end_pass(&s.pass);
            s.ready_frames = if ready { s.ready_frames + 1 } else { 0 };
            done_in_draw.set(s.ready_frames >= 3);
            frames_in_draw.set((frames_in_draw.get().0 + 1, s.ready_frames));
            if !done_in_draw.get() {
                let _ = cx.new_next_frame();
            }
        }
    }))));
    cx.borrow_mut().init_cx_os();
    let (backend, w, rgba) = capture(&cx, &done, &frames);
    assert!(w >= WIDTH && rgba.len() >= w * HEIGHT * 4, "the window is {w} wide, {} bytes", rgba.len());
    let mut wrong = Vec::new();
    let mut count = 0;
    for y in 0..SIDE {
        for x in 0..SIDE {
            let want = bits(x, y).to_le_bytes();
            let at = |gx: usize| &rgba[(y * w + gx) * 4..(y * w + gx) * 4 + 4];
            let (lo, hi) = (at(x), at(x + SIDE));
            let got = [lo[0], lo[1], lo[2], hi[0]];
            if got != want {
                count += 1;
                if wrong.len() < 16 {
                    wrong.push(format!("texel ({x}, {y}): {:08x}, written {:08x}", u32::from_le_bytes(got), u32::from_le_bytes(want)));
                }
            }
        }
    }
    println!("r32f low bits: backend {backend}, {} texels, {count} differ", SIDE * SIDE);
    assert!(wrong.is_empty(), "{count} of {} texels read back other bits than were written (the first: {wrong:#?})", SIDE * SIDE);
}

/// The window's pixels once `done` holds and the frame that drew it finished: the backend, its
/// width and RGBA rows, top first. The wait is 30 s from the first frame drawn (a cold start's
/// shader builds come before it).
fn capture(cx: &Rc<RefCell<Cx>>, done: &Cell<bool>, frames: &Cell<(u32, u32)>) -> (u32, usize, Vec<u8>) {
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
    (info.backend, pixels.w as usize, pixels.rgba)
}
