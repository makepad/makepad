//! Uploads beside a blinking caret: a cached list of 2,000 rows, two draws of one publication,
//! two slices of a shared block, and a first frame of many new lists. Counted by the HAL
//! (`upload_bytes`, `buffer_creates`, `buffer_updates` / `buffer_parts`, and `instance_rows`
//! after each draw's ranges); headless unless MAKEPAD_HAL_BACKEND names a backend.
#![cfg(not(target_arch = "wasm32"))]

use makepad_draw::makepad_platform::makepad_hal::{open_flags, session::Stats, Hal, OpenReq};
use makepad_draw::makepad_platform::retained_instances::RetainedInstances;
use makepad_draw::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

const SIDE: f64 = 256.0;
const PUBLICATION_ROWS: usize = 1_000;
const BLOCK_ROWS: usize = 16;
const CARET_FRAMES: u64 = 4;
const GRID_LISTS: usize = 32;
const HAL_TIMEOUT: Duration = Duration::from_secs(30);

struct Scene {
    pass: DrawPass,
    main: DrawList2d,
    child: Option<DrawList2d>,
    published_list: DrawList2d,
    publication: Option<RetainedInstances>,
    shared: Option<SharedInstances>,
    /// lists of one quad each, recorded on the frame they appear
    grid: Vec<DrawList2d>,
    quad: DrawQuad,
    blink: bool,
    child_recordings: usize,
}

impl Scene {
    fn new(cx: &mut Cx) -> Self {
        let quad = cx.with_vm(|vm| {
            makepad_platform::script::script_mod(vm);
            makepad_draw::script_mod(vm);
            let solid = vm.eval(script! {
                use mod.pod.*
                use mod.math.*
                use mod.shader.*
                use mod.draw.*
                let STRENGTH = 0.8
                DrawQuad{ pixel: fn() { return vec4(0.2, STRENGTH, 1.0, 1.0) } }
            });
            DrawQuad::script_from_value(vm, solid)
        });
        let texture = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 {
            size: TextureSize::Auto,
            initial: true,
        });
        let pass = DrawPass::new(cx);
        pass.set_size(cx, dvec2(SIDE, SIDE));
        pass.set_color_texture(cx, &texture, DrawPassClearColor::ClearWith(vec4(0.0, 0.0, 0.0, 1.0)));
        Self {
            pass, main: DrawList2d::new(cx), child: None, published_list: DrawList2d::new(cx),
            publication: None, shared: None, grid: Vec::new(), quad, blink: false, child_recordings: 0,
        }
    }

    fn draw(&mut self, cx: &mut Cx, event: &DrawEvent) {
        let mut draw = CxDraw::new(cx, event);
        let mut cx = Cx2d::new(&mut draw);
        cx.begin_pass(&self.pass, Some(1.0));
        self.main.begin_always(&mut cx);
        cx.begin_root_turtle(dvec2(SIDE, SIDE), Layout::flow_overlay());
        self.blink = !self.blink;
        self.quad.draw_abs(&mut cx, Rect {
            pos: dvec2(if self.blink { 2.0 } else { 4.0 }, 248.0),
            size: dvec2(1.0, 6.0),
        });
        if let Some(child) = &mut self.child {
            if child.begin(&mut cx, Walk::fixed(SIDE, SIDE)).is_redrawing() {
                self.child_recordings += 1;
                self.quad.begin_many_instances(&mut cx);
                for i in 0..2_000 {
                    self.quad.draw_abs(&mut cx, Rect {
                        pos: dvec2((i % 50) as f64, (i / 50) as f64),
                        size: dvec2(1.0, 1.0),
                    });
                }
                self.quad.end_many_instances(&mut cx);
                child.end(&mut cx);
            }
        }
        for (i, list) in self.grid.iter_mut().enumerate() {
            if list.begin(&mut cx, Walk::fixed(SIDE, SIDE)).is_redrawing() {
                self.quad.draw_abs(&mut cx, Rect { pos: dvec2(i as f64 * 2.0, 200.0), size: dvec2(1.0, 1.0) });
                list.end(&mut cx);
            }
        }
        if let Some(publication) = &self.publication {
            let list = &mut self.published_list;
            if list.begin(&mut cx, Walk::fixed(SIDE, SIDE)).is_redrawing() {
                for _ in 0..2 {
                    cx.add_retained_instances(&mut self.quad.draw_vars, publication);
                }
                if let Some(block) = &self.shared {
                    for first in [0, 4] {
                        let item = cx.new_draw_call(&mut self.quad.draw_vars).unwrap().draw_item_id;
                        cx.draw_lists.attach_shared(list.draw_list_id(), item, block, first, 8).unwrap();
                    }
                }
                list.end(&mut cx);
            }
        }
        cx.end_pass_sized_turtle();
        self.main.end(&mut cx);
        cx.end_pass(&self.pass);
    }
}

fn finish_frame(cx: &mut Cx) {
    let before = cx.frame_submission_serial();
    let deadline = Instant::now() + HAL_TIMEOUT;
    loop {
        let submitted = cx.frame_submission_serial();
        if submitted > before && cx.frame_completion_serial() == submitted {
            return;
        }
        assert!(Instant::now() < deadline, "offscreen frame after {before} did not complete");
        assert!(cx.hal_session_turn(1_000_000, submitted == before), "offscreen session ended");
    }
}

fn transferred(stats: Stats) -> u64 {
    stats.upload_bytes
}

#[test]
fn only_dirty_rows_reach_the_gpu() {
    if std::env::var_os("MAKEPAD_HAL_BACKEND").is_none() {
        std::env::set_var("MAKEPAD_HAL_BACKEND", "headless");
    }
    let state = Rc::new(RefCell::new(None::<Scene>));
    let events = state.clone();
    let mut cx = Cx::new(Box::new(move |cx, event| match event {
        Event::Startup => *events.borrow_mut() = Some(Scene::new(cx)),
        Event::Draw(event) => events.borrow_mut().as_mut().unwrap().draw(cx, event),
        _ => {}
    }));
    cx.init_cx_os();
    let hal = Hal::global();
    let info = hal.open(&OpenReq::from_env(0, open_flags::OFFSCREEN_WINDOWS, 1.0));
    cx.hal_attach_session(info);
    let stats = || hal.with_world(|world| world.session(info.session).unwrap().stats);

    // Let startup redraws settle before recording the cached child.
    finish_frame(&mut cx);
    let (main, pass, shader) = {
        let mut state = state.borrow_mut();
        let scene = state.as_mut().unwrap();
        let shader = scene.quad.draw_vars.draw_shader_id.expect("caret needs a quad shader");
        scene.child = Some(DrawList2d::new(&mut cx));
        (scene.main.draw_list_id(), scene.pass.draw_pass_id(), shader)
    };
    let deadline = Instant::now() + HAL_TIMEOUT;
    while !cx.draw_shader_ready(shader, false) || cx.pipelines_pending() != 0 {
        assert!(Instant::now() < deadline, "quad pipeline did not become ready");
        assert!(cx.hal_session_turn(1_000_000, false), "offscreen session ended during shader setup");
    }
    let stride = cx.draw_shaders[shader.index].mapping.instances.stride_bytes as u64;
    for _ in 0..3 {
        cx.redraw_list(main);
        finish_frame(&mut cx);
    }
    let slots = cx.draw_shaders[shader.index].mapping.instances.total_slots;
    let publication = |value| RetainedInstances::new(slots, vec![value; PUBLICATION_ROWS * slots].into()).unwrap();
    let published_id = {
        let mut state = state.borrow_mut();
        let scene = state.as_mut().unwrap();
        scene.publication = Some(publication(1.0));
        scene.published_list.draw_list_id()
    };
    let initial = stats();
    cx.redraw_list(main);
    finish_frame(&mut cx);
    let first = stats();
    let published_bytes = PUBLICATION_ROWS as u64 * stride;
    assert_eq!(transferred(first) - transferred(initial), published_bytes + stride,
        "two retained draws upload their publication once plus one caret row on the first paint");

    // Warm the slab growth used by replacements before measuring steady-state creates.
    state.borrow_mut().as_mut().unwrap().publication = Some(publication(2.0));
    cx.redraw_list(published_id);
    finish_frame(&mut cx);
    let warm = stats();
    let block = cx.publications.publish_ready(slots, vec![3.0; BLOCK_ROWS * slots].into(), Default::default()).unwrap();
    state.borrow_mut().as_mut().unwrap().shared = Some(block);
    cx.redraw_list(published_id);
    finish_frame(&mut cx);
    let before = stats();
    let rows_with_both = before.instance_rows - warm.instance_rows;
    assert_eq!(transferred(before) - transferred(warm), BLOCK_ROWS as u64 * stride + stride,
        "both shared slices upload the whole block once plus one caret row");
    assert_eq!(before.draws - warm.draws, 6, "the caret, cached list, two publications and two slices all draw");
    for _ in 0..CARET_FRAMES {
        cx.redraw_list(main);
        finish_frame(&mut cx);
    }
    let caret = stats();
    let main_recording = cx.draw_lists[main].redraw_id;
    cx.repaint_pass(pass);
    finish_frame(&mut cx);
    let clean = stats();

    assert_eq!(state.borrow().as_ref().unwrap().child_recordings, 1, "cached child must record exactly once");
    assert_eq!(caret.frames - before.frames, CARET_FRAMES, "every caret frame must paint");
    let draws = caret.draws - before.draws;
    assert!(draws >= 2 * CARET_FRAMES, "caret frames issued {draws} draws, expected at least one per list per frame");
    let caret_bytes = transferred(caret) - transferred(before);
    let bound = CARET_FRAMES * stride;
    assert_eq!(caret_bytes, bound, "caret frames upload exactly one {stride}-byte row each");
    let caret_creates = caret.buffer_creates - before.buffer_creates;
    assert_eq!(caret_creates, 0, "caret frames created {caret_creates} buffers: their rows go into the slabs there are");
    assert_eq!(cx.draw_lists[main].redraw_id, main_recording, "clean repaint must reuse the main list");
    assert_eq!(clean.frames - caret.frames, 1, "clean repaint must paint one frame");
    assert!(clean.draws - caret.draws >= 2, "clean repaint must draw both cached lists");
    let clean_bytes = transferred(clean) - transferred(caret);
    assert_eq!(clean_bytes, 0, "clean repaint sent {clean_bytes} bytes, bound 0");
    assert_eq!(clean.buffer_creates, caret.buffer_creates, "clean repaint created buffers");

    let replacement = publication(4.0);
    for item in 0..2 {
        assert!(cx.draw_lists.set_retained_publication(published_id, item, &replacement),
            "both retained items switch to the new publication");
    }
    state.borrow_mut().as_mut().unwrap().publication = Some(replacement);
    cx.repaint_pass(pass);
    finish_frame(&mut cx);
    let replaced = stats();
    assert_eq!(transferred(replaced) - transferred(clean), published_bytes,
        "replacing both retained draws uploads exactly one new publication");
    assert_eq!(replaced.buffer_creates - warm.buffer_creates, 0, "every frame after warm-up reuses existing buffers");

    // Keep just the second slice and repaint the same recording. The other four draws
    // contain the caret, 2,000 cached rows and two draws of PUBLICATION_ROWS.
    assert!(cx.draw_lists.detach_shared(published_id, 2), "the first shared item held a lease");
    cx.repaint_pass(pass);
    finish_frame(&mut cx);
    let with_second = stats().instance_rows - replaced.instance_rows;
    let other_rows = 1 + 2_000 + 2 * PUBLICATION_ROWS as u64;
    assert_eq!(stats().draws - replaced.draws, 5, "only the detached slice stops drawing");
    assert_eq!(with_second, other_rows + 8, "the shared item at 4..12 encodes all eight rows");
    assert_eq!(rows_with_both, with_second + 8, "the shared item at 0..8 also encodes eight rows");
    assert_eq!(cx.gpu_inventory().instance_gpu_bytes(), published_bytes + BLOCK_ROWS as u64 * stride,
        "one remaining slice keeps the whole block resident, counted once");
    assert!(cx.draw_lists.detach_shared(published_id, 3), "the second shared item held the last lease");
    cx.repaint_pass(pass);
    finish_frame(&mut cx);
    assert_eq!(cx.gpu_inventory().instance_gpu_bytes(), published_bytes, "the last detached slice releases the block's region");
    let creates = stats().buffer_creates - warm.buffer_creates;
    assert_eq!(creates, 0, "detaching the shared slices creates no buffers");

    // A first frame of many items: the new lists' rows and the caret's reach the HAL as one
    // update per instance slab written (the young slab, and the next if it filled), not one each.
    {
        let mut state = state.borrow_mut();
        state.as_mut().unwrap().grid = (0..GRID_LISTS).map(|_| DrawList2d::new(&mut cx)).collect();
    }
    let before_grid = stats();
    cx.redraw_list(main);
    finish_frame(&mut cx);
    let grid = stats();
    assert_eq!(transferred(grid) - transferred(before_grid), (GRID_LISTS as u64 + 1) * stride, "each new list and the caret upload one row");
    assert_eq!(grid.buffer_parts - before_grid.buffer_parts, GRID_LISTS as u64 + 1, "one write per row");
    let updates = grid.buffer_updates - before_grid.buffer_updates;
    assert!(updates <= 2, "{} rows took {updates} buffer updates: a frame sends one per slab it writes", GRID_LISTS + 1);

    eprintln!("dirty_rows_only: first {} B, shared {} B, {} caret frames {} B, clean {} B, replacement {} B, shared slices {}/{} rows, {creates} creates after warm-up",
        transferred(first) - transferred(initial), transferred(before) - transferred(warm), CARET_FRAMES, caret_bytes,
        clean_bytes, transferred(replaced) - transferred(clean), rows_with_both - with_second, with_second - other_rows);
    hal.close(info.session);
}

#[test]
fn dirty_rows_only_uniform_stream_dedup() {
    check_uniform_stream_dedup(false);
}

#[test]
fn dirty_rows_only_empty_container_list_emits_no_list_uniforms() {
    check_uniform_stream_dedup(true);
}

fn check_uniform_stream_dedup(container_root: bool) {
    const DRAWS: usize = 50;
    if std::env::var_os("MAKEPAD_HAL_BACKEND").is_none() {
        std::env::set_var("MAKEPAD_HAL_BACKEND", "headless");
    }
    let state = Rc::new(RefCell::new(None::<Scene>));
    let events = state.clone();
    let mut cx = Cx::new(Box::new(move |cx, event| match event {
        Event::Startup => *events.borrow_mut() = Some(Scene::new(cx)),
        Event::Draw(event) => {
            let mut state = events.borrow_mut();
            let scene = state.as_mut().unwrap();
            let Some(publication) = &scene.publication else { return scene.draw(cx, event) };
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(&scene.pass, Some(1.0));
            scene.main.begin_always(&mut cx);
            cx.begin_root_turtle(dvec2(SIDE, SIDE), Layout::flow_overlay());
            if container_root {
                let empty = scene.child.get_or_insert_with(|| DrawList2d::new(&mut cx));
                empty.begin_always(&mut cx);
                empty.end(&mut cx);
                scene.published_list.begin_always(&mut cx);
            }
            for _ in 0..DRAWS {
                cx.add_retained_instances(&mut scene.quad.draw_vars, publication);
            }
            if container_root {
                scene.published_list.end(&mut cx);
            }
            cx.end_pass_sized_turtle();
            scene.main.end(&mut cx);
            cx.end_pass(&scene.pass);
        }
        _ => {}
    }));
    cx.init_cx_os();
    let hal = Hal::global();
    let info = hal.open(&OpenReq::from_env(0, open_flags::OFFSCREEN_WINDOWS, 1.0));
    cx.hal_attach_session(info);
    finish_frame(&mut cx);
    let (pass, list, shader) = {
        let state = state.borrow();
        let scene = state.as_ref().unwrap();
        (scene.pass.draw_pass_id(), scene.main.draw_list_id(), scene.quad.draw_vars.draw_shader_id.unwrap())
    };
    let deadline = Instant::now() + HAL_TIMEOUT;
    while !cx.draw_shader_ready(shader, false) || cx.pipelines_pending() != 0 {
        assert!(Instant::now() < deadline, "uniform stream pipeline did not become ready");
        assert!(cx.hal_session_turn(1_000_000, false));
    }
    let mapping = &cx.draw_shaders[shader.index].mapping;
    let slots = mapping.instances.total_slots;
    state.borrow_mut().as_mut().unwrap().publication = Some(RetainedInstances::new(slots, vec![1.0; slots].into()).unwrap());
    let align = info.effective_uniform_align() as usize;
    let padded = |floats: usize| (4 * floats).next_multiple_of(align);
    let scope = mapping.scope_uniforms_buf.len();
    assert!(scope > 0, "the shader must read its shared scope value");
    assert_eq!(mapping.dyn_uniforms.total_slots, 0);
    let shared = padded(cx.passes[pass].pass_uniforms.as_slice().len())
        + padded(cx.draw_lists[list].draw_list_uniforms.as_slice().len()) + padded(scope);
    cx.redraw_list(list);
    finish_frame(&mut cx);
    if container_root {
        let state = state.borrow();
        let scene = state.as_ref().unwrap();
        let empty = scene.child.as_ref().unwrap().draw_list_id();
        let active = scene.published_list.draw_list_id();
        assert_eq!(cx.draw_lists[list].draw_item_order_len(), 2, "the root holds both children");
        assert_eq!(cx.draw_lists[empty].draw_item_order_len(), 0, "the first child is empty");
        assert_eq!(cx.draw_lists[active].draw_item_order_len(), DRAWS, "the second child holds all draws");
    }
    let stats = || hal.with_world(|world| world.session(info.session).unwrap().stats);
    // A nonzero zbias step gives each draw a distinct call block.
    assert!(cx.passes[pass].zbias_step > 0.0);
    let before = stats();
    cx.repaint_pass(pass);
    finish_frame(&mut cx);
    let after = stats();
    assert_eq!(after.draws - before.draws, DRAWS as u64);
    assert_eq!((after.skipped_draws, after.bad_args), (before.skipped_draws, before.bad_args));
    assert_eq!(after.uniform_bytes - before.uniform_bytes, (shared + DRAWS * align) as u64,
        "only the list that emits draws writes a list block, and all draws share one scope block");
    hal.close(info.session);
}
