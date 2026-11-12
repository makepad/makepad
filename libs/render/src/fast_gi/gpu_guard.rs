//! The persistent field must receive the real scatter draws. CPU scheduling
//! and shader compilation alone left this field empty with every test green.
use super::*;
use makepad_platform::makepad_hal::{self as hal_api, open_flags, Hal, OpenReq};
use makepad_platform::makepad_script::script_eval;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const VIEW: Vec2d = Vec2d { x: 16.0, y: 16.0 };
const CENTER: Vec3f = Vec3f { x: 0.0, y: 1.0, z: 0.0 };

#[derive(Clone, Copy, Debug)]
enum Frame {
    StartVoxels,
    Sweep,
    Counts,
}

impl Frame {
    fn paint_attempts(self) -> usize {
        // One window pass, four GI passes (one pipeline each), and on the
        // count frame one reduction pass/pipeline. Allow the initial paint
        // and a replay for each pipeline at each point in the dependency chain.
        // Startup also creates the four GI shaders, before they have work.
        let pipelines = STAGE_NAMES.len() + usize::from(matches!(self, Self::Counts));
        let passes = if matches!(self, Self::StartVoxels) { 1 } else { pipelines + 1 };
        passes * (pipelines + 1)
    }
}

struct Scene {
    _window: WindowHandle,
    pass: DrawPass,
    root: DrawList2d,
    gi: FastGi,
    sun: crate::sun::SunLight,
    clustered: crate::clustered::ClusteredLights,
    recorded: u64,
}

impl Scene {
    fn painted(&self, cx: &Cx, pass: &DrawPass) -> bool {
        // Relight/gather read the previous field while scatter is still due:
        // that feedback keeps paint_dirty set, even after a whole GPU paint.
        // The window read synchronizes the submission; check its recording,
        // completion and wholeness, not whether another repaint is owed.
        !cx.draw_shaders_pending()
            && pass.holds_whole_paint_since(cx, self.recorded, cx.frame_submission_serial())
    }

    fn passes(&self, frame: Frame) -> Vec<(&'static str, &DrawPass)> {
        let mut passes = Vec::new();
        if self.gi.stats.batch > 0 {
            passes.extend(STAGE_NAMES.into_iter().zip(
                self.gi.gpu.as_ref().expect("GI GPU exists").stages.iter().map(|s| &s.pass),
            ));
        }
        if matches!(frame, Frame::Counts) {
            passes.push(("GI probe counts", self.gi.probe_counts.pass().expect("count pass recorded")));
        }
        passes.push(("GI guard window", &self.pass));
        passes
    }
}

/// Close even when a GPU/readback assertion unwinds the test.
struct Session {
    cx: Cx,
    id: u32,
}

impl Drop for Session {
    fn drop(&mut self) {
        eprintln!("GI guard: closing GPU session");
        self.cx.hal_close_session();
        Hal::global().close(self.id);
        eprintln!("GI guard: GPU session closed");
    }
}

fn snapshot() -> Snapshot {
    // One upward-facing floor quad, and the renderer's ordinary unit box.
    let floor = Mesh::new(
        vec![-2.0, 0.0, -2.0, -2.0, 0.0, 2.0, 2.0, 0.0, 2.0, 2.0, 0.0, -2.0],
        vec![0, 1, 2, 0, 2, 3],
        3, -1, None,
    ).unwrap();
    let (vertices, indices) = crate::geometry::shape_geometry_data(makepad_scene::Shape::Box);
    let cube = Mesh::new(vertices, indices, 12, -1, None).unwrap();
    let mut transform = Mat4f::identity();
    transform.v[13] = 0.5;
    let instances = vec![
        Instance::new(Arc::new(floor), Mat4f::identity(), vec3f(0.6, 0.6, 0.6), Vec3f::default(), 1.0).unwrap(),
        Instance::new(Arc::new(cube), transform, vec3f(0.8, 0.4, 0.2), Vec3f::default(), 1.0).unwrap(),
    ];
    let triangles = instances.iter().map(Instance::triangles).sum();
    Snapshot { instances, triangles }
}

/// Record one logical frame. While pipelines compile, keep its draw lists,
/// batch texture and retained scatter instances attached: HAL repaints the
/// same recording until every pass is whole, without advancing the scheduler.
fn render_frame(session: &mut Session, scene: &RefCell<Option<Scene>>, next: &Cell<Option<Frame>>, frame: Frame) {
    let cx = &mut session.cx;
    next.set(Some(frame));
    cx.redraw_all();
    let attempts = frame.paint_attempts();
    for attempt in 0..attempts {
        let previous = cx.frame_submission_serial();
        eprintln!("GI guard: {frame:?} submitting, paint {} of {attempts}", attempt + 1);
        assert!(cx.hal_session_turn(0, true), "HAL ended submitting {frame:?}");
        if next.get().is_some() {
            continue;
        }
        if cx.frame_submission_serial() > previous {
            // Scope's host-only offscreen read, on the same GPU queue. On
            // Vulkan host_window_pixels flushes its copy and waits for the
            // completion worker, after all earlier frame/readback tickets.
            // This is a synchronization point, not a new logical GI frame.
            eprintln!("GI guard: {frame:?} submitted; reading window, paint {} of {attempts}", attempt + 1);
            let pixels = Hal::global().window_pixels(session.id, None)
                .expect("the offscreen GI window must have pixels");
            // The host may give a window its own minimum size: only that it has pixels matters here.
            assert!(pixels.w > 0 && pixels.h > 0, "the offscreen GI window has no pixels");
            assert!(cx.hal_session_turn(0, false), "HAL ended draining {frame:?}");
            assert!(cx.frame_completion_serial() >= cx.frame_submission_serial(),
                "{frame:?}: window read returned before frame {} completed (completed {})",
                cx.frame_submission_serial(), cx.frame_completion_serial());
        }
        {
            let guard = scene.borrow();
            let s = guard.as_ref().expect("startup made the GI scene");
            if s.passes(frame).iter().all(|(_, pass)| s.painted(cx, pass)) {
                return;
            }
        }
        if attempt + 1 < attempts {
            // Wait for a compiler event, not a succession of 1 ms polls that
            // can spend the entire paint budget before a worker is scheduled.
            // No elapsed-time deadline decides whether this test succeeds.
            if cx.pipelines_pending() > 0 {
                eprintln!("GI guard: {frame:?} awaiting {} pipelines", cx.pipelines_pending());
                assert!(cx.hal_session_turn(u64::MAX, false), "HAL ended awaiting {frame:?} pipelines");
            }
            // Retain this recording and its batch. Running gi.run again here
            // would advance the sweep and replace the scatter being checked.
            let guard = scene.borrow();
            for (_, pass) in guard.as_ref().unwrap().passes(frame) {
                cx.repaint_pass(pass.draw_pass_id());
            }
        }
    }
    assert!(next.get().is_none(), "{frame:?} was never recorded in {attempts} paint attempts");
    let guard = scene.borrow();
    let s = guard.as_ref().unwrap();
    let unfinished: Vec<_> = s.passes(frame).into_iter()
        .filter(|(_, pass)| !s.painted(cx, pass))
        .map(|(name, _)| name).collect();
    panic!("{frame:?}: after {attempts} paint attempts, unfinished passes: {unfinished:?}; pending pipelines: {}",
        cx.pipelines_pending());
}

#[test]
fn gpu_guard_the_scatter_fills_the_field() {
    let mut config = GiConfig {
        grid: [4, 2, 4], cascades: 1, spacing: 1.0, anchor: Some(CENTER),
        ..GiConfig::default()
    }.clamped();
    let probes = config.probe_count() * config.cascades;
    config.probes_per_frame = probes;
    let sweep_frames = probes.div_ceil(config.probes_per_frame);
    let scene: Rc<RefCell<Option<Scene>>> = Rc::new(RefCell::new(None));
    let next = Rc::new(Cell::new(None));
    let (draw_scene, draw_next) = (scene.clone(), next.clone());
    let mut cx = Cx::new(Box::new(move |cx, event| {
        if matches!(event, Event::Startup) {
            cx.with_vm(|vm| {
                vm.capture_errors();
                makepad_platform::script::script_mod(vm);
                makepad_draw::script_mod(vm);
                // The same minimal prelude as gi_shaders_compile_without_errors:
                // no widget crate, app startup, fonts or external assets.
                vm.heap_mut().new_module(id!(prelude));
                script_eval!(vm, {
                    mod.prelude.widgets_internal = {..mod.std, ..mod.pod, ..mod.math, ..mod.sdf, ..mod.shader, draw:mod.draw}
                });
                vm.heap_mut().new_module(id!(widgets));
                makepad_render_graph::pass_stdlib(vm);
                crate::local_shadows::sampling::script_mod(vm);
                if crate::local_shadows::hardware_shadow_maps() {
                    crate::local_shadows::hardware_sampling::script_mod(vm);
                }
                crate::clustered::script_mod(vm);
                super::script_mod(vm);
                crate::shaders::script_mod(vm);
                crate::local_shadows::script_mod(vm);
                let errors = vm.take_errors();
                assert!(errors.is_empty(), "GI guard script setup errors: {errors:#?}");
            });
            let mut window = WindowHandle::new(cx);
            window.configure_window(cx, VIEW, dvec2(0.0, 0.0), false, "GI scatter guard".into());
            let pass = DrawPass::new_with_name(cx, "GI guard window");
            window.set_pass(cx, &pass);
            pass.set_window_clear_color(cx, vec4(0.0, 0.0, 0.0, 1.0));
            let mut gi = FastGi::default();
            gi.set_config(config);
            gi.set_mode(GiMode::Fast);
            gi.submit_scene([1; 5], CENTER, snapshot());
            *draw_scene.borrow_mut() = Some(Scene {
                _window: window, pass, root: DrawList2d::new(cx), gi,
                sun: crate::sun::SunLight::default(),
                clustered: crate::clustered::ClusteredLights::default(),
                recorded: 0,
            });
            cx.redraw_all();
        }
        if let Event::Draw(event) = event {
            let Some(frame) = draw_next.take() else { return };
            let mut guard = draw_scene.borrow_mut();
            let s = guard.as_mut().expect("startup made the GI scene");
            let mut draw = CxDraw::new(cx, event);
            let mut cx = Cx2d::new(&mut draw);
            cx.begin_pass(&s.pass, None);
            s.root.begin_always(&mut cx);
            cx.begin_root_turtle(VIEW, Layout::flow_overlay());
            // Renderer::draw_scene_full prepares these before run_fast_gi,
            // even with no local lights. Use its uploads and shadow bindings.
            s.clustered.update(cx.cx, &[], None);
            s.clustered.render_shadows(&mut cx, CENTER, &[], &[]);
            // This is the renderer's entry point: voxel publication, scheduler,
            // batch upload, trace -> relight -> gather -> DrawGiScatter, including
            // ScatterCache and add_retained_instances_count. No test-only scatter.
            s.gi.run(&mut cx, CENTER, &s.sun, &s.clustered);
            if s.gi.stats.batch > 0 {
                if let Some(parent) = s.gi.relight_pass() {
                    s.clustered.parent_shadows(cx.cx, parent);
                }
            }
            assert!(!s.gi.stats.rejected_scene, "the GI guard needs float GPU targets");
            if matches!(frame, Frame::Counts) {
                eprintln!("GI guard: count requested");
                assert!(s.gi.request_probe_counts(&mut cx).expect("request GI probe counts"),
                    "the GI guard must issue its own readback");
            }
            cx.end_pass_sized_turtle();
            s.root.end(&mut cx);
            cx.end_pass(&s.pass);
            s.recorded = cx.repaint_id();
        }
    }));
    cx.init_cx_os();
    // Scope's offscreen/retained HAL window, with an explicit native backend:
    // an inherited headless setting cannot turn this into a CPU-only pass.
    let req = OpenReq {
        backend: if cfg!(target_vendor = "apple") { hal_api::backend_id::METAL }
            else if cfg!(windows) { hal_api::backend_id::D3D11 }
            else { hal_api::backend_id::AUTO },
        flags: open_flags::OFFSCREEN_WINDOWS | open_flags::RETAIN_WINDOWS,
        dpi_hint: 1.0,
        ..OpenReq::default()
    };
    let info = Hal::global().open(&req);
    assert_ne!(info.session, 0, "the GI guard could not open a GPU session");
    let mut session = Session { cx, id: info.session };
    session.cx.hal_attach_session(info);
    assert_ne!(info.backend, hal_api::backend_id::HEADLESS, "the GI guard needs a real GPU");
    eprintln!("GI guard: opened GPU, backend {}", info.backend);

    // One setup frame launches the production pool job. Service its completion
    // without rendering extra frames or depending on the worker's wall time.
    eprintln!("GI guard: starting voxels");
    render_frame(&mut session, &scene, &next, Frame::StartVoxels);
    // One voxel job and the four shaders created by ensure(). Their wakes
    // can arrive in either order; no extra scene frames are needed.
    let voxel_turns = 1 + STAGE_NAMES.len();
    let voxels_done = || scene.borrow().as_ref().unwrap().gi.voxel_job.as_ref()
        .expect("the first frame launched voxelization").is_finished();
    for _ in 0..voxel_turns {
        if voxels_done() { break; }
        assert!(session.cx.hal_session_turn(u64::MAX, false), "HAL ended during voxelization");
    }
    assert!(voxels_done(), "voxel job unfinished after {voxel_turns} completion turns");
    eprintln!("GI guard: voxels done");
    // 32 / 32 = one sweep frame. No prior GPU batch exists to change the
    // controller's initial budget. Wait for its LAST scatter to finish.
    let mut issued = vec![false; probes];
    for k in 0..sweep_frames {
        eprintln!("GI guard: sweep frame {} of {sweep_frames}", k + 1);
        render_frame(&mut session, &scene, &next, Frame::Sweep);
        let guard = scene.borrow();
        let gi = &guard.as_ref().unwrap().gi;
        assert_eq!(gi.stats.batch, config.probes_per_frame,
            "sweep frame {}: batch must match configured probes_per_frame", k + 1);
        for item in &gi.batch {
            issued[item.cascade * config.probe_count() + item.slot] = true;
        }
    }
    if let Some(first) = issued.iter().position(|issued| !issued) {
        panic!("sweep never issued cascade {} probe {}", first / config.probe_count(), first % config.probe_count());
    }
    {
        let guard = scene.borrow();
        let gi = &guard.as_ref().unwrap().gi;
        assert_eq!(gi.stats.batch, probes, "the initial batch must issue every probe");
        assert!(gi.cascades.iter().all(Cascade::full_sweep_issued), "the scheduler must finish its sweep");
    }

    // A third frame requests the normal reduction/readback after that sweep.
    // It also performs the renderer's ordinary refresh; the count pass depends
    // on its scatter, so it cannot read the gather or an unfinished last batch.
    eprintln!("GI guard: recording counts");
    render_frame(&mut session, &scene, &next, Frame::Counts);
    // No readback-latency-in-frames constant exists in this platform.
    // frame_build emits the copy immediately after its producer; Vulkan's
    // completion worker delivers it before the later host window read above.
    // That read plus the single non-presenting intake replaces the poll loop:
    // zero additional frames, and a missing ticket is an immediate failure.
    let stats = scene.borrow_mut().as_mut().unwrap().gi
        .poll_probe_counts(&mut session.cx).expect("read GI probe counts")
        .expect("GI probe-count ticket missing after the count pass and window read completed");
    eprintln!("GI guard: count read");
    assert!(stats.probe_counts_swept && stats.valid_probes > 0 && stats.untraced_probes == 0,
        "the scatter must fill its field: valid={}, invalid={}, untraced={}; {stats:?}",
        stats.valid_probes, stats.invalid_probes, stats.untraced_probes);
}
