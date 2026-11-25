//! The persistent field must receive the real scatter draws. CPU scheduling
//! and shader compilation alone left this field empty with every test green.
use super::*;
use makepad_platform::makepad_hal::{self as hal_api, open_flags, Hal, OpenReq};
use makepad_platform::makepad_script::script_eval;
use makepad_platform::{ReadbackRequest, ReadbackTicket, TextureReadback};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const VIEW: Vec2d = Vec2d { x: 16.0, y: 16.0 };
const CENTER: Vec3f = Vec3f { x: 0.0, y: 1.0, z: 0.0 };

#[derive(Clone, Copy, Debug)]
enum Frame {
    StartVoxels,
    /// The frame that publishes the built voxels: it draws only the slot clears.
    Commit,
    Sweep,
    Counts,
    Irradiance { first: bool },
    Idle,
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
    field_ticket: Option<ReadbackTicket>,
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
    Snapshot { instances, triangles, region: None }
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

fn wait_for_voxels(cx: &mut Cx, scene: &RefCell<Option<Scene>>) {
    // Compiler and worker wakes can arrive in either order. Service them
    // without advancing the scheduler before the first publication.
    while !scene.borrow().as_ref().unwrap().gi.voxel_job.as_ref()
        .expect("the setup frame must launch voxelization").is_finished() {
        assert!(cx.hal_session_turn(u64::MAX, false), "HAL ended before the voxel job finished");
    }
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
            gi.submit_scene([1; 6], CENTER, snapshot());
            *draw_scene.borrow_mut() = Some(Scene {
                _window: window, pass, root: DrawList2d::new(cx), gi,
                sun: crate::sun::SunLight::default(),
                clustered: crate::clustered::ClusteredLights::default(),
                recorded: 0, field_ticket: None,
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
            if matches!(frame, Frame::Irradiance { .. }) {
                // One frame is one complete sweep, independent of GPU timing samples.
                let c = s.gi.config();
                s.gi.controller = BatchController::new(c.probe_count(), c.gpu_budget_ms);
            }
            // This is the renderer's entry point: voxel publication, scheduler,
            // batch upload, trace -> relight -> gather -> DrawGiScatter, including
            // ScatterCache and add_retained_instances_count. No test-only scatter.
            s.gi.run(&mut cx, CENTER, &s.sun, &s.clustered);
            if matches!(frame, Frame::Commit) {
                assert_eq!(s.gi.stats.batch, 0, "the publication's commit frame issues no probes before its slot clears land");
                assert!(s.gi.gpu.as_ref().unwrap().clears.is_empty(), "the commit frame draws its slot clears");
            }
            if matches!(frame, Frame::Idle) {
                let batch = &s.gi.gpu.as_ref().unwrap().batch;
                assert!(matches!(batch.get_format(cx.cx), TextureFormat::VecRGBAf32 { updated: TextureUpdated::Empty, .. }),
                    "the idle draw marked the probe batch for upload");
            }
            if s.gi.stats.batch > 0 {
                if let Some(parent) = s.gi.relight_pass() {
                    s.clustered.parent_shadows(cx.cx, parent);
                }
            }
            assert!(!s.gi.stats.rejected_scene, "the GI guard needs float GPU targets");
            if let Frame::Irradiance { first } = frame {
                assert!(s.gi.stats.batch > 0, "the reference sweep must issue probes");
                // Sample the curve's first issue and the issue that actually drains each block.
                if first || s.gi.stats.unconverged_probes == 0 {
                    let field = &s.gi.gpu.as_ref().unwrap().field;
                    s.field_ticket = Some(field.read_back(cx.cx, ReadbackRequest { next_render: true })
                        .expect("request the field produced by this reference sweep"));
                }
            }
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
    assert_eq!(scene.borrow().as_ref().unwrap().gi.owes(), Some("voxels building"),
        "a host that waits for the field's light waits while its voxels build");
    wait_for_voxels(&mut session.cx, &scene);
    eprintln!("GI guard: voxels done");
    render_frame(&mut session, &scene, &next, Frame::Commit);
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
        assert_eq!(gi.owes(), None,
            "windows committed where the centre wants them and every first trace issued: nothing is owed");
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

    // Reuse the native session and its compiled trace/relight/gather/scatter pipelines.
    check_irradiance_curves(&mut session, &scene, &next);
}

// Acceptance tolerances for the measured field, independent of the scheduler.
const COLD_EPSILON: f64 = 0.02;
const WARM_EPSILON: f64 = 0.05;

fn bright_room() -> Snapshot {
    // The inner eight probes see a closed, diffuse, emissive lining. An even
    // source on the air side of all six walls keeps ray rotation noise small.
    // Leave a ring of probes around the walls so relight can gather at a hit.
    let (vertices, mut indices) = crate::geometry::shape_geometry_data(makepad_scene::Shape::Box);
    for triangle in indices.chunks_exact_mut(3) {
        triangle.swap(1, 2); // The voxeliser takes the normal from winding.
    }
    let mesh = Mesh::new(vertices, indices, 12, -1, None).expect("closed room mesh");
    let mut transform = Mat4f::identity();
    for axis in [0, 5, 10] { transform.v[axis] = 1.75; }
    let lining = Instance::new(Arc::new(mesh), transform, vec3f(1.0, 1.0, 1.0),
        vec3f(1.0, 1.0, 1.0), 1.0).expect("the white lining reaches relight's 0.9 albedo cap");
    Snapshot { triangles: lining.triangles(), instances: vec![lining], region: None }
}

fn mean_irradiance(config: GiConfig, sample: TextureReadback) -> f64 {
    let [width, height, tiles_across, tile_rows] = config.atlas();
    assert_eq!((sample.width, sample.height), (width, height), "readback must cover the field atlas");
    let bytes = sample.data.expect("read the scatter's RGBA16F attachment");
    assert_eq!(bytes.len(), width * height * 8, "field must have packed RGBA16F rows");
    // Average the linear RGB of every resident tile's 6x6 directional samples.
    // The 8x8 tile duplicates edges for filtering. Alpha, moments, state and
    // atlas padding carry no irradiance. Keep the denominator fixed across sweeps.
    let mut sum = 0.0;
    for level in 0..config.cascades {
        for slot in 0..config.probe_count() {
            let x = (slot % tiles_across) * 8 + 1;
            let y = level * tile_rows * 19 + (slot / tiles_across) * 8 + 1;
            for row in y..y + 6 {
                let start = (row * width + x) * 8;
                for pixel in bytes[start..start + 6 * 8].chunks_exact(8) {
                    for channel in pixel[..6].chunks_exact(2) {
                        let bits = u16::from_le_bytes([channel[0], channel[1]]);
                        sum += f64::from(makepad_platform::makepad_script::pod_heap::f16_to_f32(bits));
                    }
                }
            }
        }
    }
    let mean = sum / (config.cascades * config.probe_count() * 36 * 3) as f64;
    assert!(mean.is_finite() && mean >= 0.0, "invalid mean field irradiance {mean}");
    mean
}

fn completed_reference_frame(cx: &mut Cx, scene: &RefCell<Option<Scene>>, next: &Cell<Option<Frame>>, frame: Frame) -> u64 {
    next.set(Some(frame));
    cx.redraw_all();
    let previous = cx.frame_submission_serial();
    assert!(cx.hal_session_turn(0, true), "HAL ended submitting {frame:?}");
    let submitted = cx.frame_submission_serial();
    assert!(next.get().is_none() && submitted > previous, "{frame:?} was never submitted");
    // Consume GPU events without presenting or repainting: replaying a whole
    // scatter would apply a second history blend to one scheduler issue.
    while cx.frame_completion_serial() < submitted {
        assert!(cx.hal_session_turn(u64::MAX, false), "HAL ended awaiting {frame:?}");
    }
    let guard = scene.borrow();
    let s = guard.as_ref().unwrap();
    for (name, pass) in s.passes(frame) {
        assert!(s.painted(cx, pass), "{name} omitted draws in {frame:?} submission {submitted}");
    }
    previous
}

fn reference_sweep(cx: &mut Cx, scene: &RefCell<Option<Scene>>, next: &Cell<Option<Frame>>, first: bool) -> (Option<f64>, GiStats) {
    let previous = completed_reference_frame(cx, scene, next, Frame::Irradiance { first });
    let (ticket, stats) = {
        let mut guard = scene.borrow_mut();
        let s = guard.as_mut().unwrap();
        (s.field_ticket.take(), s.gi.stats)
    };
    let Some(ticket) = ticket else { return (None, stats) };
    let sample = loop {
        if let Some(sample) = cx.try_take_texture_readbacks_for(&[ticket]).pop() { break sample; }
        assert!(cx.hal_session_turn(u64::MAX, false), "HAL ended awaiting the scatter readback");
    };
    let guard = scene.borrow();
    let s = guard.as_ref().unwrap();
    let scatter = &s.gi.gpu.as_ref().unwrap().stages[3].pass;
    assert!(sample.producer_serial > previous
        && scatter.holds_whole_paint_since(cx, s.recorded, sample.producer_serial),
        "field ticket must belong to this sweep's completed scatter");
    (Some(mean_irradiance(s.gi.config(), sample)), stats)
}

fn check_sleep_frame(cx: &mut Cx, scene: &RefCell<Option<Scene>>, next: &Cell<Option<Frame>>, label: &str) {
    let (last_sweep, field) = {
        let guard = scene.borrow();
        let s = guard.as_ref().unwrap();
        assert!(s.gi.wants_frames(), "{label} must request the draw that withdraws its producers");
        (s.recorded, s.gi.gpu.as_ref().unwrap().field.texture_id())
    };
    let before = completed_reference_frame(cx, scene, next, Frame::Idle);
    let guard = scene.borrow();
    let gi = &guard.as_ref().unwrap().gi;
    assert_eq!(gi.stats.batch, 0, "{label} sleep issued a probe batch");
    assert_eq!(gi.stats.uploads_bytes, 0, "{label} sleep uploaded GI data");
    assert_eq!(gi.stats.unconverged_probes, 0, "{label} sleep acquired new probe debt");
    assert!(!gi.wants_frames(), "{label} sleep requested another frame");
    let gpu = gi.gpu.as_ref().unwrap();
    assert!(gi.shader_features().0 && gpu.field.texture_id() == field, "{label} receivers lost their field");
    let mut receiver = gpu.gather.quad.draw_vars.clone();
    let shader = receiver.draw_shader_id.expect("gather uses the receiver's field sampler");
    let slot = cx.draw_shaders[shader.index].mapping.textures.iter().position(|t| t.id == live_id!(gi_field))
        .expect("the receiver must have a field texture slot");
    receiver.empty_texture(slot);
    gi.bind(cx, &mut receiver);
    assert_eq!(receiver.texture_slots[slot].as_ref().map(Texture::texture_id), Some(field),
        "{label} idle receiver must bind the last scatter's field");
    for (name, stage) in STAGE_NAMES.into_iter().zip(&gpu.stages) {
        assert!(stage.pass.holds_whole_paint_since(cx, last_sweep, before),
            "{label} sleep repainted {name} after the last sweep");
        assert!(cx.passes[stage.pass.draw_pass_id()].main_draw_list_id.is_none(),
            "{label} sleep left {name} recorded for replay");
    }
}

struct Curve {
    first: f64,
    sleep: f64,
    reference: f64,
    sleep_residual: f64,
}

fn measure_curve(cx: &mut Cx, scene: &RefCell<Option<Scene>>, next: &Cell<Option<Frame>>, cold: bool) -> Curve {
    let config = scene.borrow().as_ref().unwrap().gi.config();
    let rho = 0.9 * f64::from(config.bounce) * f64::from(config.feedback);
    let transport = |h: f64, epsilon: f64| (epsilon.ln() / (h + (1.0 - h) * rho).ln()).ceil() as usize;
    let warm_k = transport(f64::from(cascades::REFRESH_HYSTERESIS), WARM_EPSILON);
    let (label, k, predicted_sleep) = if cold {
        let k = transport(0.5, COLD_EPSILON);
        ("cold", k, 1 + k + 6)
    } else {
        ("warm", warm_k, warm_k)
    };

    let (first, stats) = reference_sweep(cx, scene, next, true);
    let first = first.expect("the curve must sample its first sweep");
    let required = stats.batch;
    assert_eq!(stats.unconverged_probes, required, "{label} first sweep must visit every owed probe once");
    let finish_block = |cx: &mut Cx, mut sweeps: usize, expected: usize| {
        loop {
            assert!(sweeps < expected, "{label} scheduler still owes probes after {expected} sweeps");
            let (mean, after) = reference_sweep(cx, scene, next, false);
            sweeps += 1;
            assert_eq!(after.batch, required, "{label} sweep must cover the same required probes");
            if after.unconverged_probes == 0 {
                assert_eq!(sweeps, expected, "{label} scheduler stopped before its predicted issue count");
                return (sweeps, mean.expect("the sweep that pays the last debt must sample the field"));
            }
        }
    };
    let (sleep_sweeps, sleep) = finish_block(cx, 1, predicted_sleep);
    check_sleep_frame(cx, scene, next, label);

    // One full warm block on the same field leaves at most 5% of the
    // transport error present at sleep in the scalar model.
    for cascade in &mut scene.borrow_mut().as_mut().unwrap().gi.cascades { cascade.restart(); }
    let (reference_sweeps, reference) = finish_block(cx, 0, warm_k);
    assert!(reference > 0.0, "{label} reference must receive light inside the room");
    let residual = |mean: f64| (mean - reference).abs() / reference;
    let sleep_residual = residual(sleep);
    eprintln!("GI reference {label} feedback={} rho={rho:.4} K={k} predicted_sleep={predicted_sleep} actual_sleep={sleep_sweeps} first={first:.6} first_residual={:.5} sleep_mean={sleep:.6} sleep_residual={sleep_residual:.5} reference_at={} reference={reference:.6} reference_residual=0",
        config.feedback, residual(first), sleep_sweeps + reference_sweeps);
    Curve { first, sleep, reference, sleep_residual }
}

fn check_irradiance_curves(session: &mut Session, scene: &RefCell<Option<Scene>>, next: &Cell<Option<Frame>>) {
    let center = Vec3f::default();
    let config = GiConfig {
        grid: [4; 3], cascades: 1, spacing: 1.0, home: [0.5; 3], anchor: Some(center),
        probes_per_frame: 64, feedback: 0.85, bounce: 1.0, settle: true, ..GiConfig::default()
    };
    {
        let mut guard = scene.borrow_mut();
        let s = guard.as_mut().unwrap();
        s.sun.color = Vec3f::default();
        s.sun.sky = Vec3f::default();
        s.sun.ground = Vec3f::default();
        s.gi.set_config(config);
        s.gi.submit_scene([2; 6], center, bright_room());
    }
    render_frame(session, scene, next, Frame::StartVoxels);
    wait_for_voxels(&mut session.cx, scene);
    render_frame(session, scene, next, Frame::Commit);
    let cold = measure_curve(&mut session.cx, scene, next, true);
    {
        let mut guard = scene.borrow_mut();
        let gi = &mut guard.as_mut().unwrap().gi;
        let field = gi.gpu.as_ref().unwrap().field.texture_id();
        gi.set_config(GiConfig { feedback: 0.5, ..config });
        assert_eq!(gi.gpu.as_ref().expect("feedback wake must preserve the field").field.texture_id(), field,
            "warm convergence must start from the cold reference allocation");
        assert!(gi.cascades.iter().any(|c| c.owed() > 0), "changing feedback must wake the settled field");
    }
    let warm = measure_curve(&mut session.cx, scene, next, false);
    assert!(cold.first > 0.0 && cold.reference > 2.0 * cold.first,
        "most of the room's steady irradiance must come from feedback: first={}, reference={}", cold.first, cold.reference);
    assert!(warm.reference < cold.reference * (1.0 - WARM_EPSILON),
        "reducing feedback must move the reference by more than the warm tolerance");
    assert!(cold.sleep_residual <= COLD_EPSILON,
        "cold field slept outside its 2% tolerance. The countdown under-counts transport issues. first={}, sleep={}, reference={}",
        cold.first, cold.sleep, cold.reference);
    assert!(warm.sleep_residual <= WARM_EPSILON,
        "warm field slept outside its 5% tolerance. The countdown under-counts warm issues. first={}, sleep={}, reference={}",
        warm.first, warm.sleep, warm.reference);
}
