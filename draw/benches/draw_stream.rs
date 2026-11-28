//! What a window pass costs per frame on the GPU backend, from the HAL's counters (`hal stats`):
//! A, 10,000 draw calls with a texture change each, kept in a clean list while a caret blinks;
//! B, a caret over a clean 50,000-instance list; C, B's list recorded again every frame (a full
//! repaint); D, replacing one of 1,000 retained 64-row tiles each frame. Prints a readable line
//! and a `draw_stream_json` line per case. `--frames N` (default 4) follows 3 warm-up frames;
//! `--scenes ABC` picks the cases and their order (default ABC). `--idle-ms T
//! --idle-repeats N` then paints one C frame after T ms with nothing painted, N times, and prints a
//! `draw_stream_idle_json` line for each (the first frame after idle). Every json line carries the
//! wall clock (`t0_ms`, `t1_ms`, Unix ms) of its span, so a rig can match it to link-state samples.
//! Times are only meaningful from a native release build.

#[cfg(target_arch = "wasm32")]
fn main() {
    panic!("the draw stream needs a native HAL GPU backend");
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    native::run();
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use makepad_draw::makepad_platform::makepad_hal::{
        backend_id, open_flags, session::Stats, Hal, OpenReq,
    };
    use makepad_draw::makepad_platform::retained_instances::RetainedInstances;
    use makepad_draw::*;
    use std::cell::RefCell;
    use std::rc::Rc;
    use std::time::{Duration, Instant};

    const SIDE: f64 = 512.0;
    const TEXTURED_DRAWS: usize = 10_000;
    const SOLID_INSTANCES: usize = 50_000;
    const PUBLICATIONS: usize = 1_000;
    const TILE_SIDE: usize = 8;
    const WARMUP_FRAMES: u64 = 3;

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Scenario {
        A,
        B,
        C,
        D,
    }

    fn grid_rect(index: usize, columns: usize, side: f64) -> Rect {
        Rect {
            pos: dvec2((index % columns) as f64 * side, (index / columns) as f64 * side),
            size: dvec2(side, side),
        }
    }

    fn tile_publication(quad: &mut DrawQuad, tile: usize, shift: f32) -> RetainedInstances {
        let slots = quad.draw_vars.as_slice().len();
        let mut data = Vec::with_capacity(TILE_SIDE * TILE_SIDE * slots);
        quad.rect_size = vec2(2.0, 2.0);
        let clip = quad.draw_clip;
        quad.draw_clip = vec4(0.0, 0.0, SIDE as f32, SIDE as f32);
        for y in 0..TILE_SIDE {
            for x in 0..TILE_SIDE {
                quad.rect_pos = vec2(((tile % 32 * TILE_SIDE + x) * 2) as f32 + shift,
                    ((tile / 32 * TILE_SIDE + y) * 2) as f32);
                data.extend_from_slice(quad.draw_vars.as_slice());
            }
        }
        quad.draw_clip = clip;
        RetainedInstances::new(slots, data.into()).unwrap()
    }

    struct Scene {
        _window: WindowHandle,
        pass: DrawPass,
        main_list: DrawList2d,
        textured_list: DrawList2d,
        solid_list: DrawList2d,
        publication_list: Option<DrawList2d>,
        next_tile: usize,
        solid: DrawQuad,
        textured: DrawQuad,
        textures: [Texture; 2],
        first_draw: Option<Instant>,
        target_serial: u64,
        scenario: Option<Scenario>,
        child_recordings: u64,
        blink: bool,
    }

    impl Scene {
        fn new(cx: &mut Cx) -> Self {
            let (solid, textured) = cx.with_vm(|vm| {
                makepad_platform::script::script_mod(vm);
                makepad_draw::script_mod(vm);
                let solid = vm.eval(script! {
                    use mod.pod.*
                    use mod.math.*
                    use mod.shader.*
                    use mod.draw.*
                    DrawQuad{
                        pixel: fn() { return vec4(0.2, 0.8, 1.0, 1.0) }
                    }
                });
                let textured = vm.eval(script! {
                    use mod.pod.*
                    use mod.math.*
                    use mod.shader.*
                    use mod.draw.*
                    DrawQuad{
                        image: texture_2d(float)
                        pixel: fn() { return self.image.sample(self.pos) }
                    }
                });
                (
                    DrawQuad::script_from_value(vm, solid),
                    DrawQuad::script_from_value(vm, textured),
                )
            });
            let textures = [0xff4488cc, 0xffcc8844].map(|color| {
                Texture::new_with_format(cx, TextureFormat::VecBGRAu8_32 {
                    width: 2,
                    height: 2,
                    data: Some(vec![color; 4]),
                    updated: TextureUpdated::Full,
                })
            });
            let mut window = WindowHandle::new(cx);
            window.configure_window(cx, dvec2(SIDE, SIDE), dvec2(0.0, 0.0), false, "Draw stream".into());
            let pass = DrawPass::new(cx);
            window.set_pass(cx, &pass);
            pass.set_size(cx, dvec2(SIDE, SIDE));
            pass.set_window_clear_color(cx, vec4(0.0, 0.0, 0.0, 1.0));
            Self {
                _window: window,
                pass,
                main_list: DrawList2d::new(cx),
                textured_list: DrawList2d::new(cx),
                solid_list: DrawList2d::new(cx),
                publication_list: None,
                next_tile: 0,
                solid,
                textured,
                textures,
                first_draw: None,
                target_serial: 0,
                scenario: None,
                child_recordings: 0,
                blink: false,
            }
        }

        fn ready(&self, cx: &Cx) -> bool {
            [&self.solid, &self.textured].into_iter().all(|quad| {
                quad.draw_vars.draw_shader_id.is_some_and(|id| cx.draw_shader_ready(id, false))
            }) && cx.pipelines_pending() == 0
        }

        fn draw(&mut self, cx: &mut Cx, event: &DrawEvent) {
            self.first_draw.get_or_insert_with(Instant::now);
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(&self.pass, Some(1.0));
            self.main_list.begin_always(&mut cx);
            cx.begin_root_turtle(dvec2(SIDE, SIDE), Layout::flow_overlay());
            self.blink = !self.blink;
            self.solid.draw_abs(&mut cx, Rect {
                pos: dvec2(if self.blink { 2.0 } else { 4.0 }, 504.0),
                size: dvec2(1.0, 6.0),
            });
            if let Some(scenario) = self.scenario {
                self.draw_child(&mut cx, scenario);
            } else {
                // Let the window and both pipelines settle before recording the large lists.
                self.textured.draw_vars.set_texture(0, &self.textures[0]);
                self.textured.draw_abs(&mut cx, grid_rect(0, 1, 4.0));
            }
            cx.end_pass_sized_turtle();
            self.main_list.end(&mut cx);
            cx.end_pass(&self.pass);
        }

        fn draw_child(&mut self, cx: &mut Cx2d, scenario: Scenario) {
            let child = match scenario {
                Scenario::A => &mut self.textured_list,
                Scenario::B | Scenario::C => &mut self.solid_list,
                Scenario::D => self.publication_list.as_mut().unwrap(),
            };
            if child.begin(cx, Walk::fixed(SIDE, SIDE)).is_not_redrawing() {
                if scenario == Scenario::D {
                    let list = child.draw_list_id();
                    let tile = self.next_tile;
                    let item = cx.draw_lists[list].draw_item_id_at_order_index(tile).unwrap();
                    let publication = tile_publication(&mut self.solid, tile, if self.blink { 0.5 } else { 0.0 });
                    cx.draw_lists.set_retained_publication(list, item, &publication);
                    self.next_tile = (tile + 1) % PUBLICATIONS;
                }
                return;
            }
            self.child_recordings += 1;
            match scenario {
                Scenario::A => {
                    for i in 0..TEXTURED_DRAWS {
                        self.textured.draw_vars.set_texture(0, &self.textures[i % 2]);
                        self.textured.new_draw_call(cx);
                        self.textured.draw_abs(cx, grid_rect(i, 100, 4.0));
                    }
                }
                Scenario::B | Scenario::C => {
                    self.solid.begin_many_instances(cx);
                    for i in 0..SOLID_INSTANCES {
                        self.solid.draw_abs(cx, grid_rect(i, 250, 1.0));
                    }
                    self.solid.end_many_instances(cx);
                }
                Scenario::D => {
                    for tile in 0..PUBLICATIONS {
                        let publication = tile_publication(&mut self.solid, tile, 0.0);
                        cx.add_retained_instances(&mut self.solid.draw_vars, &publication);
                    }
                }
            }
            child.end(cx);
        }
    }

    /// The value after `flag`, parsed, or `default` without the flag.
    fn arg<T: std::str::FromStr>(flag: &str, default: T) -> T {
        let mut args = std::env::args().skip(1);
        if args.any(|arg| arg == flag) {
            args.next().and_then(|arg| arg.parse().ok()).unwrap_or_else(|| panic!("{flag} needs a value"))
        } else {
            default
        }
    }

    fn scenes() -> Vec<Scenario> {
        let letters: String = arg("--scenes", "ABC".to_string());
        let scenes: Vec<Scenario> = letters.chars().map(|c| match c {
            'A' => Scenario::A,
            'B' => Scenario::B,
            'C' => Scenario::C,
            'D' => Scenario::D,
            c => panic!("unknown --scenes letter {c:?}; expected A, B, C or D"),
        }).collect();
        assert!(!scenes.is_empty(), "--scenes names no case");
        scenes
    }

    fn now_ms() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
    }

    // NextFrame stops asking for redraws at the target serial. Keep pumping until the GPU catches up.
    fn run_frames(cx: &mut Cx, state: &RefCell<Option<Scene>>, count: u64) {
        let target = cx.frame_submission_serial().checked_add(count)
            .expect("requested frame count exceeds the renderer's serial range");
        if let Some(scene) = state.borrow_mut().as_mut() {
            scene.target_serial = target;
            cx.new_next_frame();
        }
        loop {
            assert!(cx.hal_session_turn(1_000_000, true), "draw stream session ended before its frames completed");
            let submitted = cx.frame_submission_serial();
            let completed = cx.frame_completion_serial();
            let state = state.borrow();
            if let Some(scene) = state.as_ref() {
                if let Some(start) = scene.first_draw {
                    assert!(start.elapsed() < Duration::from_secs(30),
                        "window frames missed their deadline in {:?}: target {target}, submitted {submitted}, completed {completed}, {} pipelines pending", scene.scenario, cx.pipelines_pending());
                }
                if submitted >= target && completed == submitted && scene.ready(cx) {
                    return;
                }
            }
        }
    }

    fn session_stats(hal: &Hal, sid: u32) -> Stats {
        hal.with_world(|world| world.session(sid).map(|session| session.stats))
            .expect("draw stream session disappeared before its counters were read")
    }

    fn print_stats(scenario: Scenario, backend: &str, frames: u64, before: Stats, after: Stats, span: (u128, u128), tag: &str) {
        let mean = |end: u64, start: u64| (end - start) as f64 / frames as f64;
        let draws = mean(after.draws, before.draws);
        let commands = mean(after.commands, before.commands);
        let buffer_updates = mean(after.buffer_updates, before.buffer_updates);
        let buffer_parts = mean(after.buffer_parts, before.buffer_parts);
        let arenas_taken = mean(after.arenas_taken, before.arenas_taken);
        let copy_cmds = mean(after.copy_cmds, before.copy_cmds);
        let copy_regions = mean(after.copy_regions, before.copy_regions);
        let upload_barriers = mean(after.upload_barriers, before.upload_barriers);
        let submissions = mean(after.submissions, before.submissions);
        let driver_uploads = mean(after.driver_uploads, before.driver_uploads);
        let binds = mean(after.binds, before.binds);
        let set_writes = mean(after.set_writes, before.set_writes);
        let upload_bytes = mean(after.upload_bytes, before.upload_bytes);
        let uniform_bytes = mean(after.uniform_bytes, before.uniform_bytes);
        let build_ns = mean(after.build_ns, before.build_ns);
        let walk_ns = mean(after.walk_ns, before.walk_ns);
        let exec_ns = mean(after.exec_ns, before.exec_ns);
        let upload_ns = mean(after.upload_ns, before.upload_ns);
        // a total, not a mean: a steady stream creates no buffers
        let buffer_creates = after.buffer_creates - before.buffer_creates;
        let arena_creates = after.arena_creates - before.arena_creates;
        // instance bytes the backend sent through an upload arena (Vulkan on a discrete GPU)
        let staged_bytes = mean(after.staged_bytes, before.staged_bytes);
        // GPU time per frame the backend timed (NaN without GPU timestamps)
        let gpu_frames = after.gpu_frames - before.gpu_frames;
        let gpu_ns = if gpu_frames == 0 { f64::NAN } else { (after.gpu_ns_total - before.gpu_ns_total) as f64 / gpu_frames as f64 };
        let description = match scenario {
            Scenario::A => "10000 draws, a texture change each",
            Scenario::B => "caret over 50000 clean instances",
            Scenario::C => "full repaint of 50000 instances",
            Scenario::D => "one replacement among 1000 retained 64-row tiles",
        };
        print!("draw_stream {scenario:?} ({description}) on {backend}, per frame over {frames}: \
            draws {draws}, binds {binds}, set writes {set_writes}, upload {upload_bytes} B, \
            commands {commands}, buffer updates {buffer_updates}, buffer parts {buffer_parts}, arenas taken {arenas_taken}, \
            copy commands {copy_cmds}, copy regions {copy_regions}, upload barriers {upload_barriers}, submissions {submissions}, driver uploads {driver_uploads}, \
            uniforms {uniform_bytes} B, buffers created {buffer_creates}, arenas created {arena_creates}, staged {staged_bytes} B; \
            build {:.2} us, walk {:.2} us, exec {:.2} us, upload {:.2} us, gpu {:.2} us", build_ns / 1000.0, walk_ns / 1000.0, exec_ns / 1000.0, upload_ns / 1000.0, gpu_ns / 1000.0);
        if scenario == Scenario::A {
            print!("; per draw: build {:.2} ns, exec {:.2} ns", build_ns / draws, exec_ns / draws);
        }
        println!();
        println!("{tag} {{\"t0_ms\":{},\"t1_ms\":{},\"scenario\":\"{scenario:?}\",\"backend\":\"{backend}\",\"frames\":{frames},\
            \"draws\":{draws:?},\"binds\":{binds:?},\"set_writes\":{set_writes:?},\"upload_bytes\":{upload_bytes:?},\
            \"commands\":{commands:?},\"buffer_updates\":{buffer_updates:?},\"buffer_parts\":{buffer_parts:?},\"arenas_taken\":{arenas_taken:?},\
            \"copy_cmds\":{copy_cmds:?},\"copy_regions\":{copy_regions:?},\"upload_barriers\":{upload_barriers:?},\"submissions\":{submissions:?},\"driver_uploads\":{driver_uploads:?},\
            \"uniform_bytes\":{uniform_bytes:?},\
            \"build_ns\":{build_ns:?},\"walk_ns\":{walk_ns:?},\"exec_ns\":{exec_ns:?},\"upload_ns\":{upload_ns:?},\"buffer_creates\":{buffer_creates},\"arena_creates\":{arena_creates},\"staged_bytes\":{staged_bytes:?},\"gpu_ns\":{}}}", span.0, span.1, if gpu_ns.is_nan() { "null".to_string() } else { format!("{gpu_ns:?}") });
    }

    pub(super) fn run() {
        std::env::set_var("MAKEPAD_HIDE_WINDOWS", "1");
        if std::env::var_os("MAKEPAD_HAL_BACKEND").is_none() {
            std::env::set_var("MAKEPAD_HAL_BACKEND", "offscreen");
        }
        let frames: u64 = arg("--frames", 4);
        assert!(frames > 0, "--frames must be positive");
        let scenes = scenes();
        let idle_ms: u64 = arg("--idle-ms", 0);
        let idle_repeats: u64 = arg("--idle-repeats", 0);
        let state = Rc::new(RefCell::new(None::<Scene>));
        let events = state.clone();
        let mut cx = Cx::new(Box::new(move |cx, event| match event {
            Event::Startup => {
                let scene = Scene::new(cx);
                scene.main_list.redraw(cx);
                *events.borrow_mut() = Some(scene);
            }
            Event::NextFrame(_) => {
                if let Some(scene) = events.borrow().as_ref() {
                    if cx.frame_submission_serial() < scene.target_serial {
                        scene.main_list.redraw(cx);
                        if scene.scenario == Some(Scenario::C) {
                            scene.solid_list.redraw(cx);
                        }
                        cx.new_next_frame();
                    }
                }
            }
            Event::Draw(event) => events.borrow_mut().as_mut().unwrap().draw(cx, event),
            _ => {}
        }));
        cx.init_cx_os();
        let hal = Hal::global();
        let req = OpenReq::from_env(0, open_flags::OFFSCREEN_WINDOWS, 1.0);
        let info = hal.open(&req);
        assert_ne!(info.backend, backend_id::HEADLESS, "the draw stream needs a GPU backend");
        let backend = match info.backend {
            backend_id::METAL => "metal",
            backend_id::D3D11 => "d3d11",
            backend_id::GL => "gl",
            backend_id::VULKAN => "vulkan",
            id => panic!("draw stream session did not open on a supported GPU backend (id {id})"),
        };
        cx.hal_attach_session(info);
        run_frames(&mut cx, &state, 1);
        for &scenario in &scenes {
            {
                let mut state = state.borrow_mut();
                let scene = state.as_mut().unwrap();
                if scenario == Scenario::D {
                    scene.publication_list = Some(DrawList2d::new(&mut cx));
                    scene.next_tile = 0;
                }
                scene.scenario = Some(scenario);
                scene.first_draw = None;
                scene.child_recordings = 0;
            }
            run_frames(&mut cx, &state, WARMUP_FRAMES);
            let before = session_stats(hal, info.session);
            let t0 = now_ms();
            run_frames(&mut cx, &state, frames);
            let after = session_stats(hal, info.session);
            let t1 = now_ms();
            assert_eq!(after.frames - before.frames, frames, "{scenario:?} must complete every measured frame");
            assert_eq!(after.window_frames - before.window_frames, frames,
                "{scenario:?} must present every measured frame to the window");
            print_stats(scenario, backend, frames, before, after, (t0, t1), "draw_stream_json");

            // A, B and D retain their list. D replaces one publication in place.
            // C records every frame, and A issues its 10,000 draws.
            let state = state.borrow();
            let scene = state.as_ref().unwrap();
            let expected = if scenario == Scenario::C { WARMUP_FRAMES + frames } else { 1 };
            assert_eq!(scene.child_recordings, expected,
                "{scenario:?} recorded its child list {} times, the scene expects {expected}", scene.child_recordings);
            if scenario == Scenario::A {
                let draws = after.draws - before.draws;
                assert!(draws >= frames * TEXTURED_DRAWS as u64,
                    "A issued {draws} draws over {frames} frames, the scene draws {TEXTURED_DRAWS} a frame");
            }
            if scenario == Scenario::D {
                assert_eq!(after.draws - before.draws, frames * (PUBLICATIONS as u64 + 1),
                    "D draws every tile and the caret in each measured frame");
            }
        }
        // the first frame after idle: nothing painted for idle_ms, then one C frame (C records
        // its list every frame), timed alone
        if idle_repeats > 0 {
            {
                let mut state = state.borrow_mut();
                let scene = state.as_mut().unwrap();
                scene.scenario = Some(Scenario::C);
                scene.first_draw = None;
            }
            run_frames(&mut cx, &state, WARMUP_FRAMES);
            for _ in 0..idle_repeats {
                std::thread::sleep(Duration::from_millis(idle_ms));
                state.borrow_mut().as_mut().unwrap().first_draw = None;
                let before = session_stats(hal, info.session);
                let t0 = now_ms();
                run_frames(&mut cx, &state, 1);
                let after = session_stats(hal, info.session);
                let t1 = now_ms();
                assert_eq!(after.frames - before.frames, 1, "an after-idle take must complete its one frame");
                print_stats(Scenario::C, backend, 1, before, after, (t0, t1), "draw_stream_idle_json");
            }
        }
        hal.close(info.session);
    }
}
