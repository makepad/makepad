//! Every sampled texture format reads the same texel in the vertex stage as in the pixel stage,
//! through a nearest and a linear sampler, on the native HAL GPU backend: one, two and four
//! channel textures of bytes and floats, 3 x 2 texels (a row of 3, 6 and 12 bytes: no row is
//! padded to 4). Each read writes the texel's first two channels into its own cell of the window.
#![cfg(not(target_arch = "wasm32"))]

use makepad_draw::*;
use std::{cell::{Cell, RefCell}, rc::Rc};

/// A cell per read (device pixels), in a row.
const CELL: usize = 4;
/// The four reads of each texture: vertex nearest, vertex linear, pixel nearest, pixel linear.
const READS: [&str; 4] = ["vertex, nearest", "vertex, linear", "pixel, nearest", "pixel, linear"];
/// The textures (their format) and the first two channels of texel (2, 1), the one read, as
/// bytes.
const TEXTURES: [(&str, [u8; 2]); 5] = [
    ("R f32", [191, 0]),
    ("RG u8", [220, 50]),
    ("R u8", [230, 0]),
    ("BGRA u8", [200, 50]),
    ("RGBA f32", [153, 77]),
];
const WIDTH: usize = CELL * READS.len() * TEXTURES.len();
const HEIGHT: usize = CELL;

fn textures(cx: &mut Cx) -> Vec<Texture> {
    let w = 3;
    let h = 2;
    let rf32 = vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.75];
    let rgu8: Vec<u8> = (0..6u8).flat_map(|i| [20 + 40 * i, 200 - 30 * i]).collect();
    let ru8: Vec<u8> = (0..6u8).map(|i| 30 + 40 * i).collect();
    // 0xAARRGGBB
    let bgra: Vec<u32> = (0..6u32).map(|i| if i == 5 { 0xFF_C8_32_00 } else { 0xFF00_0000 | 0x10_10_10 * (i + 1) }).collect();
    let rgbaf32: Vec<f32> = (0..6).flat_map(|i| if i == 5 { [0.6, 0.3, 0.0, 1.0] } else { [0.05 * i as f32, 0.1, 0.2, 1.0] }).collect();
    vec![
        Texture::new_with_format(cx, TextureFormat::VecRf32 { width: w, height: h, data: Some(rf32), updated: TextureUpdated::Full }),
        Texture::new_with_format(cx, TextureFormat::VecRGu8 { width: w, height: h, data: Some(rgu8), unpack_row_length: None, updated: TextureUpdated::Full }),
        Texture::new_with_format(cx, TextureFormat::VecRu8 { width: w, height: h, data: Some(ru8), unpack_row_length: None, updated: TextureUpdated::Full }),
        Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 { width: w, height: h, data: Some(bgra), updated: TextureUpdated::Full }),
        Texture::new_with_format(cx, TextureFormat::VecRGBAf32 { width: w, height: h, data: Some(rgbaf32), updated: TextureUpdated::Full }),
    ]
}

struct State {
    _window: WindowHandle,
    pass: DrawPass,
    list: DrawList2d,
    /// Per texture, its four reads.
    reads: Vec<DrawQuad>,
    _textures: Vec<Texture>,
    ready_frames: u32,
}

#[test]
fn each_texture_format_reads_the_same_in_the_vertex_and_the_pixel_stage() {
    std::env::set_var("MAKEPAD_HAL_BACKEND", "offscreen");
    let mut state: Option<State> = None;
    let done = Rc::new(Cell::new(false));
    let done_in_draw = done.clone();
    let frames = Rc::new(Cell::new((0u32, 0u32)));
    let frames_in_draw = frames.clone();
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            let shaders = cx.with_vm(|vm| {
                makepad_platform::script::script_mod(vm);
                makepad_draw::script_mod(vm);
                // texel (2, 1) of 3 x 2: its centre
                [
                    vm.eval(script! {
                        use mod.pod.*
                        use mod.math.*
                        use mod.shader.*
                        use mod.draw.*
                        DrawQuad{
                            image: texture_2d(float)
                            v_val: varying(vec4f)
                            vertex: fn() {
                                self.vertex_pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size)
                                self.v_val = self.image.sample_nearest(vec2(0.8333333, 0.75), 0.0)
                            }
                            pixel: fn() {
                                return vec4(self.v_val.x, self.v_val.y, 0.0, 1.0)
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
                            v_val: varying(vec4f)
                            vertex: fn() {
                                self.vertex_pos = self.clip_and_transform_vertex(self.rect_pos, self.rect_size)
                                self.v_val = self.image.sample_lod(vec2(0.8333333, 0.75), 0.0)
                            }
                            pixel: fn() {
                                return vec4(self.v_val.x, self.v_val.y, 0.0, 1.0)
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
                                let v = self.image.sample_nearest(vec2(0.8333333, 0.75))
                                return vec4(v.x, v.y, 0.0, 1.0)
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
                                let v = self.image.sample(vec2(0.8333333, 0.75))
                                return vec4(v.x, v.y, 0.0, 1.0)
                            }
                        }
                    }),
                ]
            });
            let textures = textures(cx);
            let mut reads = Vec::new();
            for t in &textures {
                for s in &shaders {
                    let mut d = cx.with_vm(|vm| DrawQuad::script_from_value(vm, *s));
                    d.draw_vars.set_texture(0, t);
                    reads.push(d);
                }
            }
            let mut window = WindowHandle::new(cx);
            window.configure_window(cx, dvec2(WIDTH as f64, HEIGHT as f64), dvec2(0.0, 0.0), false, "Vertex texture formats".into());
            let pass = DrawPass::new(cx);
            window.set_pass(cx, &pass);
            pass.set_window_clear_color(cx, vec4(0.0, 0.0, 1.0, 1.0));
            state = Some(State { _window: window, pass, list: DrawList2d::new(cx), reads, _textures: textures, ready_frames: 0 });
            cx.redraw_all();
        }
        if matches!(event, Event::NextFrame(_)) {
            cx.redraw_all();
        }
        if let Event::Draw(event) = event {
            let s = state.as_mut().unwrap();
            let ready = s.reads.iter().all(|d| d.draw_vars.draw_shader_id.is_some_and(|id| cx.draw_shader_ready(id, true)));
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(&s.pass, None);
            s.list.begin_always(&mut cx);
            cx.begin_root_turtle(dvec2(WIDTH as f64, HEIGHT as f64), Layout::flow_overlay());
            let dpi = cx.current_dpi_factor();
            let cell = CELL as f64 / dpi;
            for (k, d) in s.reads.iter_mut().enumerate() {
                d.new_draw_call(&mut cx);
                d.draw_abs(&mut cx, Rect { pos: dvec2(cell * k as f64, 0.0), size: dvec2(cell, cell) });
            }
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
    println!("vertex texture formats: backend {backend}");
    let mut wrong = Vec::new();
    for (t, (name, want)) in TEXTURES.iter().enumerate() {
        for (r, read) in READS.iter().enumerate() {
            let k = t * READS.len() + r;
            let (x, y) = (k * CELL + CELL / 2, CELL / 2);
            let p = &rgba[(y * w + x) * 4..(y * w + x) * 4 + 2];
            let ok = p.iter().zip(want).all(|(g, w)| g.abs_diff(*w) <= 2);
            println!("vertex texture formats: {name}, {read}: {} {} (expected {} {}){}", p[0], p[1], want[0], want[1], if ok { "" } else { "  WRONG" });
            if !ok {
                wrong.push(format!("{name}, {read}: {} {} (expected {} {})", p[0], p[1], want[0], want[1]));
            }
        }
    }
    assert!(wrong.is_empty(), "texel reads that differ: {wrong:#?}");
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
            assert!(std::time::Instant::now() < at, "the reads never painted whole: {drawn} frames drawn, the last {ready} with every pipeline in, {} pipelines pending", cx.borrow().pipelines_pending());
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
