//! Analytic surfaces through the complete CORTAO chain on a native offscreen GPU.
//! Five independent cells share one table and one window/session; CPU ray hits
//! supply hardware depth and the floor's distance bins, not expected shader data.
#![cfg(not(target_arch = "wasm32"))]

use makepad_draw::*;
use makepad_render::cortao::*;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const SIDE: usize = 192;
const CELLS: usize = 5;
const WIDTH: usize = SIDE * CELLS;
const HEIGHT: usize = SIDE;
const RADIUS: f64 = 0.5;
const CREASE: usize = 2;

struct Scene {
    name: &'static str,
    camera: CortaoCamera,
    depth: Vec<f32>,
    /// Only nearest hits on the crease's floor have a wall distance.
    floor_distance: Vec<Option<f64>>,
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| a[i] - b[i])
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn unit(a: [f64; 3]) -> [f64; 3] {
    let norm = dot(a, a).sqrt();
    a.map(|v| v / norm)
}

fn from_rows(rows: [[f64; 4]; 4]) -> [f32; 16] {
    std::array::from_fn(|i| rows[i % 4][i / 4] as f32)
}

fn mul(m: &[f32; 16], p: [f64; 4]) -> [f64; 4] {
    std::array::from_fn(|row| (0..4).map(|col| m[col * 4 + row] as f64 * p[col]).sum())
}

fn clip_camera(eye: [f64; 3], target: [f64; 3], ortho: bool) -> [f32; 16] {
    let f = unit(sub(target, eye));
    let r = unit(cross(f, [0.0, 1.0, 0.0]));
    let u = cross(r, f);
    let view = from_rows([
        [r[0], r[1], r[2], -dot(r, eye)],
        [u[0], u[1], u[2], -dot(u, eye)],
        [-f[0], -f[1], -f[2], dot(f, eye)],
        [0.0, 0.0, 0.0, 1.0],
    ]);
    let (near, far) = (0.1, 60.0);
    let projection = if ortho {
        from_rows([
            [0.5, 0.0, 0.0, 0.0],
            [0.0, 0.5, 0.0, 0.0],
            [0.0, 0.0, 1.0 / (near - far), near / (near - far)],
            [0.0, 0.0, 0.0, 1.0],
        ])
    } else {
        let cot = 1.0 / (60.0_f64.to_radians() * 0.5).tan();
        from_rows([
            [cot, 0.0, 0.0, 0.0],
            [0.0, cot, 0.0, 0.0],
            [0.0, 0.0, far / (near - far), far * near / (near - far)],
            [0.0, 0.0, -1.0, 0.0],
        ])
    };
    std::array::from_fn(|i| {
        (0..4)
            .map(|k| projection[k * 4 + i % 4] as f64 * view[(i / 4) * 4 + k] as f64)
            .sum::<f64>() as f32
    })
}

fn scene(index: usize) -> Scene {
    let name = [
        "front plane",
        "70-degree plane",
        "90-degree crease",
        "1 mm checker layers",
        "orthographic plane",
    ][index];
    let clip = if index == CREASE {
        clip_camera([2.2, 1.6, 2.6], [0.4, 0.0, 0.0], false)
    } else {
        clip_camera([0.0, 0.0, 0.0], [0.0, 0.0, -1.0], index == 4)
    };
    let inverse = Mat4f { v: clip }.invert().v;
    let camera = CortaoCamera::from_clip(&clip).expect("analytic camera is representable");
    let mut depth = vec![1.0; SIDE * SIDE];
    let mut floor_distance = vec![None; SIDE * SIDE];
    for y in 0..SIDE {
        for x in 0..SIDE {
            let nx = 2.0 * (x as f64 + 0.5) / SIDE as f64 - 1.0;
            let ny = 1.0 - 2.0 * (y as f64 + 0.5) / SIDE as f64;
            let unproject = |z| {
                let h = mul(&inverse, [nx, ny, z, 1.0]);
                [h[0] / h[3], h[1] / h[3], h[2] / h[3]]
            };
            let origin = unproject(0.0);
            let dir = sub(unproject(1.0), origin);
            let mut best = f64::INFINITY;
            let mut on_floor = false;
            // Analytic ray/plane intersections; t is measured from near to far.
            let mut intersect = |normal: [f64; 3], offset: f64, floor: bool| {
                let denominator = dot(normal, dir);
                if denominator.abs() > 1e-12 {
                    let t = (offset - dot(normal, origin)) / denominator;
                    if t >= 0.0 && t <= 1.0 && t < best {
                        best = t;
                        on_floor = floor;
                    }
                }
            };
            match index {
                CREASE => {
                    intersect([0.0, 1.0, 0.0], 0.0, true);
                    intersect([1.0, 0.0, 0.0], 0.0, false);
                }
                1 => {
                    let angle = 70.0_f64.to_radians();
                    intersect([angle.sin(), 0.0, angle.cos()], -4.0 * angle.cos(), false);
                }
                3 => {
                    let distance = 3.0 + ((x + y) & 1) as f64 * 0.001;
                    intersect([0.0, 0.0, 1.0], -distance, false);
                }
                _ => intersect([0.0, 0.0, 1.0], -4.0, false),
            }
            if best.is_finite() {
                let p: [f64; 3] = std::array::from_fn(|i| origin[i] + best * dir[i]);
                let h = mul(&clip, [p[0], p[1], p[2], 1.0]);
                let d = (h[2] / h[3]) as f32;
                if (0.0..0.999999).contains(&d) {
                    depth[y * SIDE + x] = d;
                    if on_floor {
                        floor_distance[y * SIDE + x] = Some(p[0].abs());
                    }
                }
            }
        }
    }
    Scene {
        name,
        camera,
        depth,
        floor_distance,
    }
}

struct CellDraw {
    ao: CortaoPass,
    depth: Texture,
    camera: CortaoCamera,
    display: DrawQuad,
}

struct State {
    _window: WindowHandle,
    pass: DrawPass,
    list: DrawList2d,
    table: CortaoTable,
    cells: Vec<CellDraw>,
    ready_frames: u32,
}

#[test]
fn analytic_scenes_preserve_planes_and_resolve_a_crease() {
    std::env::set_var("MAKEPAD_HAL_BACKEND", "offscreen");
    let scenes = Rc::new((0..CELLS).map(scene).collect::<Vec<_>>());
    let scenes_in_draw = scenes.clone();
    let mut state: Option<State> = None;
    let done = Rc::new(Cell::new(false));
    let done_in_draw = done.clone();
    let frames = Rc::new(Cell::new((0u32, 0u32)));
    let frames_in_draw = frames.clone();
    let cx = Rc::new(RefCell::new(Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            let display = cx.with_vm(|vm| {
                makepad_platform::script::script_mod(vm);
                makepad_draw::script_mod(vm);
                makepad_render::cortao::script_mod(vm);
                vm.eval(script! {
                    use mod.pod.*
                    use mod.math.*
                    use mod.shader.*
                    use mod.draw.*
                    DrawQuad{
                        image: texture_2d(float)
                        pixel: fn() {
                            let v = self.image.sample_nearest(self.pos).x
                            return vec4(v, v, v, 1.0)
                        }
                    }
                })
            });
            let cells = scenes_in_draw
                .iter()
                .map(|s| CellDraw {
                    ao: CortaoPass::default(),
                    depth: Texture::new_with_format(
                        cx,
                        TextureFormat::VecRf32 {
                            width: SIDE,
                            height: SIDE,
                            data: Some(s.depth.clone()),
                            updated: TextureUpdated::Full,
                        },
                    ),
                    camera: s.camera,
                    display: cx.with_vm(|vm| DrawQuad::script_from_value(vm, display)),
                })
                .collect();
            let table = CortaoTable::new(cx, CortaoTier::High.rays());
            let mut window = WindowHandle::new(cx);
            window.configure_window(
                cx,
                dvec2(WIDTH as f64, HEIGHT as f64),
                dvec2(0.0, 0.0),
                false,
                "CORTAO scenes".into(),
            );
            let pass = DrawPass::new(cx);
            window.set_pass(cx, &pass);
            // A missing composite must not accidentally pass the white-plane assertions.
            pass.set_window_clear_color(cx, vec4(0.0, 0.0, 1.0, 1.0));
            state = Some(State {
                _window: window,
                pass,
                list: DrawList2d::new(cx),
                table,
                cells,
                ready_frames: 0,
            });
            cx.redraw_all();
        }
        if matches!(event, Event::NextFrame(_) | Event::DrawShadersReady) {
            cx.redraw_all();
        }
        if let Event::Draw(event) = event {
            let s = state.as_mut().expect("startup creates the scene state");
            let mut ready = s.cells.iter().all(|cell| {
                cell.display
                    .draw_vars
                    .draw_shader_id
                    .is_some_and(|id| cx.draw_shader_ready(id, false))
            });
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(&s.pass, None);
            s.list.begin_always(&mut cx);
            let dpi = cx.current_dpi_factor();
            let size = dvec2(SIDE as f64 / dpi, SIDE as f64 / dpi);
            for cell in &mut s.cells {
                let input = CortaoInput {
                    size,
                    depth: &cell.depth,
                    depth_frame: CortaoDepth::Current,
                    camera: cell.camera,
                    tier: CortaoTier::High,
                    config: CortaoConfig {
                        radius: RADIUS as f32,
                        ..CortaoConfig::default()
                    },
                    frame: 0,
                    history: false,
                    reset: false,
                    table: &s.table,
                };
                if let Some(CortaoOutput::Visibility(output)) =
                    cell.ao.run(&mut cx, &input, s.pass.draw_pass_id())
                {
                    cell.display.draw_vars.set_texture(0, &output);
                } else {
                    ready = false;
                }
            }
            cx.begin_root_turtle(
                dvec2(WIDTH as f64 / dpi, HEIGHT as f64 / dpi),
                Layout::flow_overlay(),
            );
            if ready {
                for (i, cell) in s.cells.iter_mut().enumerate() {
                    cell.display.draw_abs(
                        &mut cx,
                        Rect {
                            pos: dvec2(i as f64 * size.x, 0.0),
                            size,
                        },
                    );
                }
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
    let (backend, width, rgba) = capture(&cx, &done, &frames);
    assert!(
        width >= WIDTH && rgba.len() >= width * HEIGHT * 4,
        "scene window extent: {width} wide, {} bytes",
        rgba.len()
    );
    println!(
        "CORTAO scenes: backend {backend}, {WIDTH}x{HEIGHT}, {} frames",
        frames.get().0
    );

    for (index, scene) in scenes.iter().enumerate() {
        let mut min = 255u8;
        let mut max = 0u8;
        let mut count = 0usize;
        let mut sum = 0u64;
        let mut background_min = 255u8;
        // Keep all floor bins, including those beyond the AO radius.
        let mut bins: Vec<(f64, f64, usize)> = Vec::new();
        for y in 0..SIDE {
            for x in 0..SIDE {
                let at = (y * width + index * SIDE + x) * 4;
                let value = rgba[at];
                assert_eq!(
                    &rgba[at..at + 3],
                    &[value; 3],
                    "{}: composite is grey at ({x},{y})",
                    scene.name
                );
                if scene.depth[y * SIDE + x] >= 0.999999 {
                    background_min = background_min.min(value);
                    continue;
                }
                min = min.min(value);
                max = max.max(value);
                sum += value as u64;
                count += 1;
                if let Some(distance) = scene.floor_distance[y * SIDE + x] {
                    let a = distance / RADIUS;
                    let bin = (a * 10.0).floor() as usize;
                    if bins.len() <= bin {
                        bins.resize(bin + 1, (0.0, 0.0, 0));
                    }
                    let expected = if a < 1.0 {
                        1.0 - (a.acos() - a * (1.0 - a * a).sqrt()) / std::f64::consts::PI
                    } else {
                        1.0
                    };
                    bins[bin].0 += value as f64 / 255.0;
                    bins[bin].1 += expected;
                    bins[bin].2 += 1;
                }
            }
        }
        assert!(
            count > SIDE * SIDE / 4,
            "{}: meaningful covered area ({count} pixels)",
            scene.name
        );
        println!("{}: covered={count}, min={min}, max={max}, mean={:.6}, range={}, background_min={background_min}",
            scene.name, sum as f64 / count as f64 / 255.0, max - min);
        assert_eq!(
            background_min, 255,
            "{}: uncovered pixels are fully visible",
            scene.name
        );
        if index != CREASE {
            assert!(
                min as f64 >= 0.99 * 255.0,
                "{}: no self occlusion, minimum {min}/255",
                scene.name
            );
            if index == 3 {
                assert!(
                    max - min <= 3,
                    "{}: coplanar visibility range is {} (at most 3)",
                    scene.name,
                    max - min
                );
            }
        } else {
            for bin in 2..=9 {
                assert!(
                    bins.get(bin).is_some_and(|b| b.2 > 0),
                    "90-degree crease: bin [{:.1},{:.1}) has floor samples",
                    bin as f64 * 0.1,
                    (bin + 1) as f64 * 0.1
                );
            }
            assert!(
                bins.iter().skip(12).any(|b| b.2 > 0),
                "90-degree crease: far floor samples exist"
            );
            for (i, (sum, expected_sum, n)) in bins.into_iter().enumerate() {
                if n == 0 {
                    continue;
                }
                let mean = sum / n as f64;
                // Average the closed form at the actual hits to avoid a bin-centre bias.
                let expected = expected_sum / n as f64;
                println!("90-degree crease: a=[{:.1},{:.1}), n={n}, mean={mean:.6}, expected={expected:.6}, error={:.6}",
                    i as f64 * 0.1, (i + 1) as f64 * 0.1, (mean - expected).abs());
                if (2..=9).contains(&i) {
                    assert!((mean - expected).abs() <= 0.12, "90-degree crease: bin {i} visibility {mean:.6}, expected {expected:.6} (tolerance 0.12)");
                }
                if i >= 12 {
                    assert!(
                        mean >= 0.97,
                        "90-degree crease: far bin {i} visibility {mean:.6} (at least 0.97)"
                    );
                }
            }
        }
    }
}

/// As in r32f_low_bits: wait for completed submission and read native GPU pixels.
fn capture(
    cx: &Rc<RefCell<Cx>>,
    done: &Cell<bool>,
    frames: &Cell<(u32, u32)>,
) -> (u32, usize, Vec<u8>) {
    use makepad_draw::makepad_platform::makepad_hal::{self as hal_api, open_flags, Hal, OpenReq};
    let hal = Hal::global();
    let mut req = OpenReq::from_env(0, open_flags::OFFSCREEN_WINDOWS, 1.0);
    req.flags |= open_flags::OFFSCREEN_WINDOWS | open_flags::RETAIN_WINDOWS;
    let info = hal.open(&req);
    assert_ne!(
        info.backend,
        hal_api::backend_id::HEADLESS,
        "CORTAO scene comparisons require a GPU"
    );
    cx.borrow_mut().hal_attach_session(info);
    let mut compile_deadline = None;
    let mut ready_since = None;
    let result = loop {
        let (drawn, ready) = frames.get();
        if drawn > 0 {
            let at = *compile_deadline.get_or_insert_with(|| {
                std::time::Instant::now() + std::time::Duration::from_secs(30)
            });
            if std::time::Instant::now() >= at {
                break Err(format!("CORTAO passes never painted whole: {drawn} frames, {ready} ready, {} pipelines pending", cx.borrow().pipelines_pending()));
            }
        }
        if ready > 0 {
            let start = *ready_since.get_or_insert_with(std::time::Instant::now);
            if start.elapsed() >= std::time::Duration::from_secs(8) {
                break Err(
                    "CORTAO scene capture exceeded eight seconds after shaders became ready".into(),
                );
            }
        }
        let mut cx = cx.borrow_mut();
        if !cx.hal_session_turn(1_000_000, true) {
            break Err("CORTAO HAL session ended before capture".into());
        }
        let submitted = cx.frame_submission_serial();
        if done.get()
            && submitted != 0
            && cx.frame_completion_serial() == submitted
            && cx.pipelines_pending() == 0
        {
            if let Some(pixels) = hal.window_pixels(info.session, None) {
                break Ok(pixels);
            }
        }
    };
    hal.close(info.session);
    let pixels = result.unwrap_or_else(|error| panic!("{error}"));
    println!(
        "CORTAO capture: {:.3} s after shader readiness",
        ready_since.map_or(0.0, |start| start.elapsed().as_secs_f64())
    );
    (info.backend, pixels.w as usize, pixels.rgba)
}
