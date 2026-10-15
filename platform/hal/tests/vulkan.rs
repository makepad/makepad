//! Native Vulkan coverage. These tests never substitute the CPU rasterizer for a device.
#![cfg(any(target_os = "linux", target_os = "android"))]

use makepad_hal::ledger::Kind;
use makepad_hal::spirv::SpirvPipeline;
use makepad_hal::*;
use makepad_shader::shader_ir::*;
use makepad_shader::shader_ir_spirv::ir_to_spirv;
use makepad_vulkan::{vk, Entry};
use std::time::{Duration, Instant};

const TEX: Id = Id::new(1, 1);
const VERTEX: Id = Id::new(1, 1);
const INDEX: Id = Id::new(2, 1);
const PIPE: Id = Id::new(1, 1);
const WINDOW: Id = Id::new(1, 1);

fn open() -> Option<(std::sync::Arc<Hal>, HalInfo)> {
    // Probe availability separately: once a graphics device exists, a HAL open failure is a
    // test failure, not a skip that could conceal a regression in device initialization.
    // The probe runs once per process: the system loader's first instance creation is not safe
    // against other threads doing the same (it crashed inside vkCreateInstance when the tests'
    // probes raced); the HAL itself creates its instance under Device::shared's lock.
    static PROBE: std::sync::OnceLock<Result<(), String>> = std::sync::OnceLock::new();
    let probe = PROBE.get_or_init(|| (|| {
        let entry = unsafe { Entry::load() }?;
        let app = vk::ApplicationInfo::default().api_version(vk::API_VERSION_1_1);
        let instance = unsafe { entry.create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None) }
            .map_err(|e| format!("Vulkan 1.1 instance unavailable: {e:?}"))?;
        let devices = unsafe { instance.enumerate_physical_devices() };
        let found = devices.as_ref().is_ok_and(|devices| devices.iter().any(|physical| unsafe {
            instance.get_physical_device_properties(*physical).api_version >= vk::API_VERSION_1_1
                && instance.get_physical_device_queue_family_properties(*physical).iter()
                    .any(|q| q.queue_count != 0 && q.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        }));
        unsafe { instance.destroy_instance(None); }
        if found { Ok(()) } else { Err(format!("no Vulkan 1.1 graphics device ({devices:?})")) }
    })());
    if let Err(reason) = probe { eprintln!("SKIP Vulkan: {reason}"); return None; }
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::VULKAN, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_eq!(info.backend, backend_id::VULKAN, "a Vulkan device exists but HAL initialization failed");
    assert_eq!(info.dialect, dialect::SPIRV);
    Some((hal, info))
}

fn until(hal: &Hal, sid: u32, mut accept: impl FnMut(HalEvent<'_>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut store = EventStore::new(sid, 64, 65536);
    loop {
        assert!(Instant::now() < deadline, "Vulkan event deadline");
        let mut buf = store.buf();
        let n = hal.pump(10_000_000, &mut buf);
        for (_, event) in store.reader(n, buf.arena_len) {
            match event {
                HalEvent::PipelineFailed { log, .. } => panic!("pipeline: {}", String::from_utf8_lossy(log)),
                HalEvent::DeviceLost { .. } => panic!("Vulkan device lost"),
                _ => {}
            }
            if accept(event) { return; }
        }
    }
}

fn frame_done(hal: &Hal, sid: u32, frame: u64) -> u64 {
    let mut ns = 0;
    until(hal, sid, |e| match e {
        HalEvent::FrameDone { frame: done, gpu_ns, .. } if done == frame => { ns = gpu_ns; true }
        _ => false,
    });
    ns
}

fn shader() -> Vec<u8> {
    let mut ir = IrModule::new();
    let vec4 = ir.vec_ty(4, IrScalar::F32);
    let mut vertex = IrFunction::new("vertex_main".into(), TY_VOID);
    vertex.entry = Some(IrEntry { stage: IrStage::Vertex,
        inputs: vec![IrEntryIo { name: "geometry".into(), ty: vec4, binding: IrIoBinding::Location(0) }],
        outputs: vec![IrEntryIo { name: "position".into(), ty: vec4, binding: IrIoBinding::Builtin(IrBuiltinIo::Position) }] });
    let input = vertex.expr(ExprKind::EntryIn(0), vec4);
    let output = vertex.expr(ExprKind::EntryOut(0), vec4);
    vertex.body.extend([Stmt::Assign(output, input), Stmt::Return(NO_EXPR)]);
    let mut fragment = IrFunction::new("fragment_main".into(), TY_VOID);
    fragment.entry = Some(IrEntry { stage: IrStage::Fragment, inputs: Vec::new(),
        outputs: vec![IrEntryIo { name: "color".into(), ty: vec4, binding: IrIoBinding::Location(0) }] });
    let one = fragment.expr(ExprKind::Lit(IrLit::F32(1.0)), TY_F32);
    let color = fragment.expr(ExprKind::Construct(vec![one; 4]), vec4);
    let output = fragment.expr(ExprKind::EntryOut(0), vec4);
    fragment.body.extend([Stmt::Assign(output, color), Stmt::Return(NO_EXPR)]);
    ir.functions.extend([vertex, fragment]);
    SpirvPipeline { vertex: ir_to_spirv(&ir, 0, false).unwrap(), fragment: ir_to_spirv(&ir, 1, false).unwrap(),
        texture_base: 0, sampler_base: 0, depth_binding: u32::MAX, instance_binding: u32::MAX,
        pick_binding: u32::MAX, pick_texture_base: u32::MAX, textures: Vec::new(), samplers: Vec::new() }.encode()
}

fn resources(hal: &Hal, sid: u32) {
    let mut w = CmdWriter::new(sid);
    w.texture_create(TEX, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 32, 32, [1, 1], Attr::default());
    let vertices: Vec<u8> = [-1.0f32, -1.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0]
        .into_iter().flat_map(f32::to_le_bytes).collect();
    w.buffer_create(VERTEX, buffer_kind::VERTEX, vertices.len() as u64, Attr::default());
    w.buffer_update(VERTEX, 0, &vertices);
    w.buffer_create(INDEX, buffer_kind::INDEX, 6, Attr::default());
    w.buffer_update(INDEX, 0, &[0, 0, 1, 0, 2, 0]);
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() }, &shader(),
        &[u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 0], &[16]);
    w.pipeline_create(PIPE, dialect::SPIRV, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(hal, sid, |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == PIPE));
    assert_eq!(hal.with_world(|w| w.session(sid).unwrap().stats.bad_args), 0);
}

fn pass(texture: Id, width: u32, height: u32, clear: [f32; 4]) -> Pass {
    Pass { color: [texture, Id::NONE, Id::NONE, Id::NONE], load: [pass_load::CLEAR; 5],
        clear: [clear; 4], viewport: [0.0, 0.0, width as f32, height as f32], dpi: 1.0, ..Default::default() }
}

fn draw() -> Draw {
    Draw { pipeline: PIPE, vb: VERTEX, ib: INDEX, index: [0, 3], inst: Inst::inline(Slice::EMPTY, 1),
        scissor: [f32::NAN; 4], ..Default::default() }
}

// Variant creation is asynchronous. Warm frames are real GPU submissions too; wait until a
// frame encoded every draw, then check that frame's counters and timestamp.
fn rendered(hal: &Hal, sid: u32, target: Id, kind: u32, size: [u32; 2], first: u64) -> (u64, u64) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut frame = first;
    loop {
        assert!(Instant::now() < deadline, "pipeline variant did not become drawable");
        let before = hal.with_world(|w| w.session(sid).unwrap().stats);
        let mut w = CmdWriter::new(sid);
        w.frame_begin(frame, FrameMeta::default());
        w.pass(pass(if kind & !target::FLAGS == target::WINDOW { Id::NONE } else { target }, size[0], size[1], [0.0, 0.0, 0.0, 1.0]));
        let third = size[0] as f32 / 3.0;
        let mut left = draw(); left.scissor = [0.0, 0.0, third, size[1] as f32];
        let mut middle = draw(); middle.scissor = [third, 0.0, third, size[1] as f32];
        w.draw(left); w.draw(middle);
        let mut second = pass(if kind & !target::FLAGS == target::WINDOW { Id::NONE } else { target }, size[0], size[1], [0.0; 4]);
        second.load[0] = pass_load::LOAD;
        w.pass(second);
        let mut right = draw(); right.scissor = [2.0 * third, 0.0, size[0] as f32 - 2.0 * third, size[1] as f32];
        w.draw(right); w.frame_end(target, kind);
        hal.submit(&w.buf());
        let gpu_ns = frame_done(hal, sid, frame);
        let after = hal.with_world(|w| w.session(sid).unwrap().stats);
        assert_eq!(after.bad_args, 0);
        assert_eq!(after.passes - before.passes, 2);
        assert_eq!(after.draws - before.draws, 3);
        if before.skipped_draws == after.skipped_draws { return (frame, gpu_ns); }
        frame += 1;
    }
}

fn pixels(hal: &Hal, sid: u32, req: u32) -> Vec<u8> {
    let mut pixels = Vec::new();
    until(hal, sid, |e| match e {
        HalEvent::Readback { req: got, status, pixels: p, .. } if got == req => {
            assert_eq!(status, readback::OK); pixels = p.to_vec(); true
        }
        _ => false,
    });
    pixels
}

#[cfg(target_os = "linux")]
fn sampled_shader() -> Vec<u8> {
    use makepad_shader::shader_output::TextureType;
    let mut ir = IrModule::new();
    let vec2 = ir.vec_ty(2, IrScalar::F32);
    let vec4 = ir.vec_ty(4, IrScalar::F32);
    let texture = ir.ty(IrTy::Texture(TextureType::Texture2d));
    let sampler = ir.ty(IrTy::Sampler(false));
    ir.globals.push(IrGlobal { kind: IrGlobalKind::Io(IrIo::Texture(TextureType::Texture2d)), name: Default::default(), ty: texture, binding: Some(0) });
    ir.globals.push(IrGlobal { kind: IrGlobalKind::Sampler(0), name: Default::default(), ty: sampler, binding: Some(1) });
    let mut vertex = IrFunction::new("vertex_main".into(), TY_VOID);
    vertex.entry = Some(IrEntry { stage: IrStage::Vertex,
        inputs: vec![IrEntryIo { name: "geometry".into(), ty: vec4, binding: IrIoBinding::Location(0) }],
        outputs: vec![IrEntryIo { name: "position".into(), ty: vec4, binding: IrIoBinding::Builtin(IrBuiltinIo::Position) },
            IrEntryIo { name: "uv".into(), ty: vec2, binding: IrIoBinding::Location(0) }] });
    let input = vertex.expr(ExprKind::EntryIn(0), vec4);
    let position = vertex.expr(ExprKind::EntryOut(0), vec4);
    let xy = vertex.expr(ExprKind::Swizzle(input, [0, 1, 0, 0], 2), vec2);
    let half = vertex.expr(ExprKind::Lit(IrLit::F32(0.5)), TY_F32);
    let minus_half = vertex.expr(ExprKind::Lit(IrLit::F32(-0.5)), TY_F32);
    let scale = vertex.expr(ExprKind::Construct(vec![half, minus_half]), vec2);
    let offset = vertex.expr(ExprKind::Construct(vec![half, half]), vec2);
    let scaled = vertex.expr(ExprKind::Binary(BinOp::Mul, xy, scale), vec2);
    let uv = vertex.expr(ExprKind::Binary(BinOp::Add, scaled, offset), vec2);
    let uv_out = vertex.expr(ExprKind::EntryOut(1), vec2);
    vertex.body.extend([Stmt::Assign(position, input), Stmt::Assign(uv_out, uv), Stmt::Return(NO_EXPR)]);
    let mut fragment = IrFunction::new("fragment_main".into(), TY_VOID);
    fragment.entry = Some(IrEntry { stage: IrStage::Fragment,
        inputs: vec![IrEntryIo { name: "uv".into(), ty: vec2, binding: IrIoBinding::Location(0) }],
        outputs: vec![IrEntryIo { name: "color".into(), ty: vec4, binding: IrIoBinding::Location(0) }] });
    let tex = fragment.expr(ExprKind::Global(0), texture);
    let coord = fragment.expr(ExprKind::EntryIn(0), vec2);
    let color = fragment.expr(ExprKind::Sample(Box::new(IrSample { tex, tex_type: TextureType::Texture2d,
        sampler: 0, kind: SampleKind::Implicit, method: Default::default(), coord,
        lod: NO_EXPR, dx: NO_EXPR, dy: NO_EXPR, reference: NO_EXPR })), vec4);
    let output = fragment.expr(ExprKind::EntryOut(0), vec4);
    fragment.body.extend([Stmt::Assign(output, color), Stmt::Return(NO_EXPR)]);
    ir.functions.extend([vertex, fragment]);
    SpirvPipeline { vertex: ir_to_spirv(&ir, 0, false).unwrap(), fragment: ir_to_spirv(&ir, 1, false).unwrap(),
        texture_base: 0, sampler_base: 1, depth_binding: u32::MAX, instance_binding: u32::MAX,
        pick_binding: u32::MAX, pick_texture_base: u32::MAX, textures: vec![0],
        samplers: vec![makepad_hal::spirv::SpirvSampler { filter: 0, address: 1, compare: false, pixel: false }] }.encode()
}

#[cfg(target_os = "linux")]
#[test]
fn dmabuf_export_import_second_session_samples_identical_pixels() {
    let Some((hal, owner)) = open() else { return };
    if let Err(reason) = hal.dmabuf_supported(owner.session) {
        eprintln!("SKIP DMA-BUF: {reason}");
        assert!(hal.close(owner.session));
        return;
    }
    let reader = hal.open(&OpenReq { backend: backend_id::VULKAN, ..Default::default() });
    assert_ne!(reader.session, 0);
    let input: Vec<u8> = (0..16u8).flat_map(|i| [i * 11, 255 - i * 13, i * 7, 255]).collect();
    let expected: Vec<u8> = input.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]).collect();
    let mut w = CmdWriter::new(owner.session);
    w.texture_create(TEX, usage::SAMPLED | usage::RENDER_TARGET | usage::SHARED, format::BGRA8, 4, 4, [1, 1], Attr::default());
    w.texture_update(TEX, 0, [0, 0, 4, 4], 0, &input);
    w.texture_share(TEX);
    hal.submit(&w.buf());
    let mut ticket = 0;
    until(&hal, owner.session, |e| match e {
        HalEvent::TextureShared { texture, handle } if texture == TEX => { ticket = handle; true }
        _ => false,
    });
    assert_ne!(ticket, 0, "advertised DMA-BUF export failed");
    let native = hal.dmabuf_handle(owner.session, ticket).expect("export descriptor");
    // Register the duplicated FD, not the export ticket: this forces vkAllocateMemory
    // import and a second VkImage instead of the in-process Arc<Image> fast path.
    let descriptor = dmabuf::DmaBuf { fd: native.fd.try_clone().unwrap(), width: native.width, height: native.height,
        fourcc: native.fourcc, modifier: native.modifier, offset: native.offset, row_pitch: native.row_pitch,
        allocation_size: native.allocation_size, sample_only: true, opaque: None, timeline_fd: None };
    let imported = unsafe { hal.dmabuf_register(reader.session, descriptor) }.unwrap();
    drop(native);
    let source = Id::new(2, 1);
    let mut w = CmdWriter::new(reader.session);
    w.texture_import(source, imported, texture_handle::NATIVE);
    hal.submit(&w.buf());
    until(&hal, reader.session, |e| match e {
        HalEvent::TextureImported { texture, ok } if texture == source => { assert!(ok); true }
        _ => false,
    });
    hal.dmabuf_release(reader.session, imported);
    assert!(hal.close(owner.session)); // imported storage must survive its exporter
    w.clear();
    w.texture_create(TEX, usage::SAMPLED | usage::RENDER_TARGET, format::RGBA8, 4, 4, [1, 1], Attr::default());
    let vertices: Vec<u8> = [-1.0f32, -1.0, 0.0, 1.0, 3.0, -1.0, 0.0, 1.0, -1.0, 3.0, 0.0, 1.0]
        .into_iter().flat_map(f32::to_le_bytes).collect();
    w.buffer_create(VERTEX, buffer_kind::VERTEX, vertices.len() as u64, Attr::default());
    w.buffer_update(VERTEX, 0, &vertices);
    w.buffer_create(INDEX, buffer_kind::INDEX, 6, Attr::default());
    w.buffer_update(INDEX, 0, &[0, 0, 1, 0, 2, 0]);
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() }, &sampled_shader(),
        &[u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 1], &[16]);
    w.pipeline_create(PIPE, dialect::SPIRV, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, reader.session, |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == PIPE));
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut frame = 0;
    loop {
        assert!(Instant::now() < deadline, "DMA-BUF sampling pipeline did not become drawable");
        let before = hal.with_world(|world| world.session(reader.session).unwrap().stats);
        frame += 1;
        w.clear(); w.frame_begin(frame, FrameMeta::default());
        w.pass(pass(TEX, 4, 4, [0.0; 4]));
        let mut d = draw(); d.tex = w.put_ids(&[source]); w.draw(d);
        w.frame_end(TEX, target::TEXTURE); hal.submit(&w.buf());
        frame_done(&hal, reader.session, frame);
        let after = hal.with_world(|world| world.session(reader.session).unwrap().stats);
        assert_eq!(after.bad_args, 0);
        if before.skipped_draws == after.skipped_draws { break; }
    }
    w.clear(); w.texture_readback(TEX, 41, [0, 0, 4, 4]); hal.submit(&w.buf());
    assert_eq!(pixels(&hal, reader.session, 41), expected);
    assert!(hal.close(reader.session));
}

#[test]
fn offscreen_two_passes_three_draws_and_timestamp() {
    let Some((hal, info)) = open() else { return };
    resources(&hal, info.session);
    let (_, ns) = rendered(&hal, info.session, TEX, target::TEXTURE, [32, 32], 1);
    if info.flags & info_flags::GPU_TIMESTAMPS != 0 { assert!(ns > 0); }
    else { eprintln!("SKIP timestamp assertion: graphics queue has no timestamp bits"); }
    let mut w = CmdWriter::new(info.session);
    w.texture_readback(TEX, 1, [0, 0, 32, 32]); hal.submit(&w.buf());
    let image = pixels(&hal, info.session, 1);
    // the triangle's base (clip y = -1) is the bottom of the target: row 0 is the top (the Y law)
    for x in [5, 16, 26] { assert_eq!(&image[(27 * 32 + x) * 4..][..4], &[255; 4]); }
    for x in [5, 26] { assert_eq!(&image[(2 * 32 + x) * 4..][..4], &[0, 0, 0, 255]); }
    assert!(hal.close(info.session));
}

#[test]
fn cleared_target_readback_is_exact() {
    let Some((hal, info)) = open() else { return };
    let mut w = CmdWriter::new(info.session);
    w.texture_create(TEX, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 7, 5, [1, 1], Attr::default());
    w.frame_begin(1, FrameMeta::default());
    w.pass(pass(TEX, 7, 5, [17.0 / 255.0, 34.0 / 255.0, 51.0 / 255.0, 1.0]));
    w.frame_end(TEX, target::TEXTURE); hal.submit(&w.buf());
    frame_done(&hal, info.session, 1);
    w.clear(); w.texture_readback(TEX, 2, [0, 0, 7, 5]); hal.submit(&w.buf());
    assert_eq!(pixels(&hal, info.session, 2), [17, 34, 51, 255].repeat(35));
    assert!(hal.close(info.session));
}

/// The last session closing with frames still on the GPU: the close runs the backend's queue
/// workers out (their batches free buffers and descriptor pools), so the process can exit
/// without those frees racing the driver's exit handlers -- the test binary exits right after.
#[test]
fn closing_the_last_session_with_frames_in_flight() {
    let Some((hal, info)) = open() else { return };
    resources(&hal, info.session);
    let (frame, _) = rendered(&hal, info.session, TEX, target::TEXTURE, [32, 32], 1);
    let mut w = CmdWriter::new(info.session);
    for f in frame + 1..frame + 9 {
        w.clear();
        w.frame_begin(f, FrameMeta::default());
        w.pass(pass(TEX, 32, 32, [0.0, 0.0, 0.0, 1.0]));
        w.draw(draw());
        w.frame_end(TEX, target::TEXTURE);
        hal.submit(&w.buf());
    }
    let start = Instant::now();
    assert!(hal.close(info.session));
    assert!(start.elapsed() < Duration::from_secs(6), "closing waited for the workers longer than their limits");
}

/// A device loss (simulated: a new device is made on the same GPU, as after the driver's
/// VK_ERROR_DEVICE_LOST) keeps every id: the session hears `DeviceLost { 1 }` once, then
/// `DeviceReset { 2 }`, the pipeline answers ready again, the buffers take their contents again
/// under the same ids and the same frames render the same pixels (a texture and a window, read
/// back by a snapshot in the window frame's own batch); the ledger is unchanged and nothing
/// counts as stale or bad.
#[test]
fn device_reset_keeps_ids_and_renders_again() {
    let Some((hal, info)) = open() else { return };
    let sid = info.session;
    resources(&hal, sid);
    let mut w = CmdWriter::new(sid);
    w.window_create(WINDOW, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 32.0, 32.0], false);
    hal.submit(&w.buf());
    let (frame, _) = rendered(&hal, sid, TEX, target::TEXTURE, [32, 32], 1);
    let (frame, _) = rendered(&hal, sid, WINDOW, target::WINDOW | target::SNAPSHOT, [32, 32], frame + 1);
    w.clear();
    w.texture_readback(TEX, 1, [0, 0, 32, 32]);
    hal.submit(&w.buf());
    let before = pixels(&hal, sid, 1);
    w.clear();
    w.snapshot(WINDOW, 2);
    hal.submit(&w.buf());
    let window_before = pixels(&hal, sid, 2);
    let rows = ledger(&hal, sid);

    hal.simulate_device_loss();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut store = EventStore::new(sid, 64, 65536);
    let (mut lost, mut reset, mut ready) = (Vec::new(), None, false);
    while !(reset.is_some() && ready) {
        assert!(Instant::now() < deadline, "no device reset");
        let mut buf = store.buf();
        let n = hal.pump(10_000_000, &mut buf);
        for (_, event) in store.reader(n, buf.arena_len) {
            match event {
                HalEvent::DeviceLost { generation } => {
                    assert!(reset.is_none(), "DeviceLost after DeviceReset");
                    lost.push(generation);
                }
                HalEvent::DeviceReset { generation } => {
                    assert!(!lost.is_empty(), "DeviceReset without DeviceLost");
                    reset = Some(generation);
                }
                HalEvent::PipelineReady { pipeline } if reset.is_some() && pipeline == PIPE => ready = true,
                HalEvent::PipelineFailed { log, .. } => panic!("pipeline: {}", String::from_utf8_lossy(log)),
                _ => {}
            }
        }
    }
    assert_eq!((lost, reset), (vec![1], Some(2)), "DeviceLost once, then DeviceReset");
    assert_eq!(hal.with_world(|w| w.device_generation), 2);

    // contents are the app's to send again: the same ids, no creates
    let vertices: Vec<u8> = [-1.0f32, -1.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0].into_iter().flat_map(f32::to_le_bytes).collect();
    w.clear();
    w.buffer_update(VERTEX, 0, &vertices);
    w.buffer_update(INDEX, 0, &[0, 0, 1, 0, 2, 0]);
    hal.submit(&w.buf());
    let (frame, _) = rendered(&hal, sid, TEX, target::TEXTURE, [32, 32], frame + 1);
    rendered(&hal, sid, WINDOW, target::WINDOW | target::SNAPSHOT, [32, 32], frame + 1);
    w.clear();
    w.texture_readback(TEX, 3, [0, 0, 32, 32]);
    hal.submit(&w.buf());
    assert_eq!(pixels(&hal, sid, 3), before, "the texture renders the same pixels on the new device");
    w.clear();
    w.snapshot(WINDOW, 4);
    hal.submit(&w.buf());
    assert_eq!(pixels(&hal, sid, 4), window_before, "the window renders the same pixels on the new device");
    assert_eq!(&before[(27 * 32 + 16) * 4..][..4], &[255; 4]);
    assert_eq!(ledger(&hal, sid), rows);
    let stats = hal.with_world(|w| w.session(sid).unwrap().stats);
    assert_eq!((stats.bad_args, stats.stale_ids), (0, 0));
    assert!(hal.close(sid));
}

fn ledger(hal: &Hal, sid: u32) -> Vec<(Kind, u32, u32, u64)> {
    hal.with_world(|w| {
        let mut rows: Vec<_> = w.session(sid).unwrap().ledger.entries().filter(|r| r.kind != Kind::Window)
            .map(|r| (r.kind, r.index, r.gen, r.bytes)).collect();
        rows.sort(); rows
    })
}

#[test]
fn session_close_frees_exactly_its_ledger() {
    let Some((hal, info)) = open() else { return };
    let other = hal.open(&OpenReq { backend: backend_id::VULKAN, ..Default::default() }).session;
    resources(&hal, info.session); resources(&hal, other);
    let own = ledger(&hal, info.session); let survivor = ledger(&hal, other);
    assert_eq!(own.len(), 4); assert_eq!(survivor.len(), 4);
    let total = hal.with_world(|w| Kind::ALL.into_iter().map(|k| w.count(k)).sum::<usize>());
    assert!(hal.close(info.session));
    assert!(hal.with_world(|w| w.session(info.session).is_none()));
    assert_eq!(ledger(&hal, other), survivor);
    assert_eq!(hal.with_world(|w| Kind::ALL.into_iter().map(|k| w.count(k)).sum::<usize>()), total - own.len());
    assert_eq!(hal.backend_inits(), 1);
    assert!(hal.close(other));
}

#[test]
fn swapchainless_resize_keeps_textures_buffers_and_pipelines() {
    let Some((hal, info)) = open() else { return };
    resources(&hal, info.session);
    let mut w = CmdWriter::new(info.session);
    w.window_create(WINDOW, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 32.0, 32.0], false); hal.submit(&w.buf());
    let (frame, _) = rendered(&hal, info.session, WINDOW, target::WINDOW, [32, 32], 1);
    let retained = ledger(&hal, info.session);
    w.clear(); w.window_geom(WINDOW, [0.0, 0.0, 80.0, 48.0]); hal.submit(&w.buf());
    rendered(&hal, info.session, WINDOW, target::WINDOW, [80, 48], frame + 1);
    assert_eq!(ledger(&hal, info.session), retained);
    assert_eq!(hal.backend_inits(), 1);
    w.clear(); w.snapshot(WINDOW, 3); hal.submit(&w.buf());
    let image = pixels(&hal, info.session, 3);
    assert_eq!(image.len(), 80 * 48 * 4);
    assert_eq!(&image[(24 * 80 + 40) * 4..][..4], &[255; 4]);
    // The same retained pipeline and geometry also still render the pre-resize texture.
    rendered(&hal, info.session, TEX, target::TEXTURE, [32, 32], frame + 100);
    assert!(hal.close(info.session));
}

const UPLOAD: Id = Id::new(2, 1);

/// One frame that first uploads `bytes` into UPLOAD (the upload batch the frame flushes) and
/// then draws into TEX; returns once the frame is done (its batches' arenas are back by then).
fn upload_frame(hal: &Hal, sid: u32, frame: u64, size: [u32; 2], uploads: &[Vec<u8>]) {
    let mut w = CmdWriter::new(sid);
    for bytes in uploads { w.texture_update(UPLOAD, 0, [0, 0, size[0], size[1]], 0, bytes); }
    w.frame_begin(frame, FrameMeta::default());
    w.pass(pass(TEX, 32, 32, [0.0, 0.0, 0.0, 1.0]));
    w.draw(draw());
    w.frame_end(TEX, target::TEXTURE);
    hal.submit(&w.buf());
    frame_done(hal, sid, frame);
    assert_eq!(hal.with_world(|w| w.session(sid).unwrap().stats.bad_args), 0);
}

fn arena_allocs(hal: &Hal, sid: u32) -> u64 { hal.with_world(|w| w.session(sid).unwrap().stats.arena_allocs) }

/// An RGBA8 image of `size` whose rows differ (a misplaced copy offset shows) and whose `n`
/// differs from the uploads before it.
fn rows(size: [u32; 2], n: u64) -> Vec<u8> {
    (0..size[1] as u64).flat_map(|y| std::iter::repeat_n(((y + 3 * n) % 251) as u8, size[0] as usize * 4)).collect()
}

/// Reads back the last 16 rows' first 16 pixels of UPLOAD (the end of the upload: the arena's
/// last bytes) and checks them against `expect`.
fn check_tail(hal: &Hal, sid: u32, req: u32, size: [u32; 2], expect: &[u8]) {
    let mut w = CmdWriter::new(sid);
    w.texture_readback(UPLOAD, req, [0, size[1] - 16, 16, 16]); hal.submit(&w.buf());
    let got = pixels(hal, sid, req);
    let row = size[0] as usize * 4;
    let want: Vec<u8> = (size[1] as usize - 16..size[1] as usize).flat_map(|y| expect[y * row..][..64].to_vec()).collect();
    assert!(got == want, "the readback is not the newest upload");
}

/// Frames with the same upload volume reuse the upload arenas of the frames before them: after
/// a warm-up no frame allocates one (an allocation is a vkAllocateMemory + map; NVIDIA spent
/// whole frames in it when every frame with more than a small spare's bytes made a new one).
/// Reused arenas carry the previous frames' bytes: the readback shows the newest upload.
#[test]
fn steady_upload_frames_allocate_no_arenas() {
    let Some((hal, info)) = open() else { return };
    let sid = info.session;
    resources(&hal, sid);
    let size = [1024, 512]; // 2 MiB a frame
    let mut w = CmdWriter::new(sid);
    w.texture_create(UPLOAD, usage::SAMPLED, format::RGBA8, size[0], size[1], [1, 1], Attr::default());
    hal.submit(&w.buf());
    let (first, _) = rendered(&hal, sid, TEX, target::TEXTURE, [32, 32], 1);
    let fill = |i: u64| rows(size, i);
    for f in first + 1..first + 5 { upload_frame(&hal, sid, f, size, &[fill(f)]); }
    let warm = arena_allocs(&hal, sid);
    assert!(warm > 0, "the first upload frames allocate their arenas");
    let last = first + 25;
    for f in first + 5..=last { upload_frame(&hal, sid, f, size, &[fill(f)]); }
    assert_eq!(arena_allocs(&hal, sid), warm, "steady frames allocated upload arenas");
    check_tail(&hal, sid, 7, size, &fill(last));
    assert!(hal.close(sid));
}

/// A frame whose uploads exceed what the pool keeps (and the largest arena a batch asks for)
/// spreads them over several arenas and still lands every byte; the steady frames after it
/// allocate nothing again.
#[test]
fn a_frame_larger_than_the_arena_pool_still_uploads() {
    let Some((hal, info)) = open() else { return };
    let sid = info.session;
    resources(&hal, sid);
    let size = [2048, 2048]; // 16 MiB an upload, 80 MiB in the big frame
    let mut w = CmdWriter::new(sid);
    w.texture_create(UPLOAD, usage::SAMPLED, format::RGBA8, size[0], size[1], [1, 1], Attr::default());
    hal.submit(&w.buf());
    let (first, _) = rendered(&hal, sid, TEX, target::TEXTURE, [32, 32], 1);
    let fill = |i: u64| rows(size, i);
    let big: Vec<Vec<u8>> = (1..=5).map(fill).collect();
    upload_frame(&hal, sid, first + 1, size, &big);
    check_tail(&hal, sid, 8, size, &fill(5));
    for f in first + 2..first + 5 { upload_frame(&hal, sid, f, size, &[fill(f)]); }
    let warm = arena_allocs(&hal, sid);
    for f in first + 5..first + 15 { upload_frame(&hal, sid, f, size, &[fill(f)]); }
    check_tail(&hal, sid, 9, size, &fill(first + 14));
    assert_eq!(arena_allocs(&hal, sid), warm, "steady frames after the big one allocated upload arenas");
    assert!(hal.close(sid));
}

#[test]
fn a_child_frame_wakes_the_parent_at_its_completion_without_the_child_pumping() {
    // the WM pumps; the child presents an embedded window frame and then does not pump (it is
    // building its next frame): the completion itself must wake the WM with the ChildFrame
    let Some((hal, info)) = open() else { return };
    let parent = info.session;
    let child = hal.open_domain(&OpenReq { backend: backend_id::VULKAN, app_key: 77, flags: open_flags::OFFSCREEN_WINDOWS, dpi_hint: 0.0, parent }, Caps::default()).session;
    assert_ne!(child, 0);
    let mut w = CmdWriter::new(child);
    w.window_create(WINDOW, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 32.0, 32.0], false);
    hal.submit(&w.buf());
    until(&hal, parent, |e| matches!(e, HalEvent::ChildWindow { child: c, .. } if c == child));
    // the parent's queue is empty and its pump blocked before the child presents
    let waiter = {
        let hal = hal.clone();
        std::thread::spawn(move || {
            let mut store = EventStore::new(parent, 64, 65536);
            let deadline = Instant::now() + Duration::from_secs(5);
            while Instant::now() < deadline {
                let mut buf = store.buf();
                let n = hal.pump(5_000_000_000, &mut buf);
                if store.reader(n, buf.arena_len).any(|(_, e)| matches!(e, HalEvent::ChildFrame { frame: 1, .. })) {
                    return Some(Instant::now());
                }
            }
            None
        })
    };
    std::thread::sleep(Duration::from_millis(100));
    let turns_before = hal.pump_counts().into_iter().find(|(id, _)| *id == parent).map(|(_, c)| c.turns).unwrap_or(0);
    let mut w = CmdWriter::new(child);
    w.frame_begin(1, FrameMeta::default());
    w.pass(pass(Id::NONE, 32, 32, [0.0, 1.0, 0.0, 1.0]));
    w.frame_end(WINDOW, target::WINDOW);
    let sent = Instant::now();
    hal.submit(&w.buf());
    let got = waiter.join().unwrap().expect("the parent heard the child's frame while the child did not pump");
    let turns = hal.pump_counts().into_iter().find(|(id, _)| *id == parent).map(|(_, c)| c.turns).unwrap_or(0) - turns_before;
    eprintln!("child frame -> parent ChildFrame in {:?}, {turns} parent turns", got - sent);
    assert!(got - sent < Duration::from_secs(2), "{:?}", got - sent);
    assert!(hal.close(child));
    assert!(hal.close(parent));
}

#[test]
fn a_shared_program_compile_wakes_every_session_that_waits_for_it() {
    // Programs are shared by shader content: a second session that creates the same pipeline
    // while the first one's compile runs waits on that compile and must be woken by its answer,
    // not only the session that started it.
    let Some((hal, first)) = open() else { return };
    let second = hal.open(&OpenReq { backend: backend_id::VULKAN, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_ne!(second.session, 0);
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() }, &shader(),
        &[u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 0], &[16]);
    for sid in [first.session, second.session] {
        let mut w = CmdWriter::new(sid);
        w.pipeline_create(PIPE, dialect::SPIRV, Attr::default(), &blob);
        hal.submit(&w.buf());
    }
    // one long pump: the answer must arrive by a wake, well before the pump's own timeout
    let start = Instant::now();
    let mut store = EventStore::new(second.session, 64, 65536);
    let mut ready = false;
    while !ready && start.elapsed() < Duration::from_secs(20) {
        let mut buf = store.buf();
        let n = hal.pump(20_000_000_000, &mut buf);
        for (_, event) in store.reader(n, buf.arena_len) {
            if matches!(event, HalEvent::PipelineReady { pipeline } if pipeline == PIPE) { ready = true; }
        }
    }
    assert!(ready, "the second session never heard its pipeline was ready");
    assert!(start.elapsed() < Duration::from_secs(10), "the second session was not woken by the shared compile ({:?})", start.elapsed());
    assert!(hal.close(second.session));
    assert!(hal.close(first.session));
}

#[test]
fn a_pipeline_on_an_already_compiled_shared_program_is_answered_without_another_submit() {
    // Session A compiles program P and hears it is ready; then an otherwise idle session B
    // creates a pipeline on the same P. B must hear PipelineReady from its own pump, with no
    // other session submitting anything.
    let Some((hal, first)) = open() else { return };
    let second = hal.open(&OpenReq { backend: backend_id::VULKAN, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_ne!(second.session, 0);
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() }, &shader(),
        &[u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 0], &[16]);
    let ready = |sid: u32| {
        let start = Instant::now();
        let mut store = EventStore::new(sid, 64, 65536);
        while start.elapsed() < Duration::from_secs(20) {
            let mut buf = store.buf();
            let n = hal.pump(20_000_000_000, &mut buf);
            for (_, event) in store.reader(n, buf.arena_len) {
                if matches!(event, HalEvent::PipelineReady { pipeline } if pipeline == PIPE) { return Some(start.elapsed()); }
            }
        }
        None
    };
    let mut w = CmdWriter::new(first.session);
    w.pipeline_create(PIPE, dialect::SPIRV, Attr::default(), &blob);
    hal.submit(&w.buf());
    assert!(ready(first.session).is_some(), "the first session never heard its pipeline was ready");
    let mut w = CmdWriter::new(second.session);
    w.pipeline_create(PIPE, dialect::SPIRV, Attr::default(), &blob);
    hal.submit(&w.buf());
    let took = ready(second.session).expect("the second session never heard its pipeline was ready");
    assert!(took < Duration::from_secs(2), "the second session heard it only after {took:?}");
    assert!(hal.close(second.session));
    assert!(hal.close(first.session));
}

/// A pass a pixel larger than its colour target (a target floored where its pass was ceiled, at
/// a fractional scale such as 1.41) still draws: clipped to the target, never refused. Refused,
/// the pass drew nothing every frame (Scope's Glass look and History view on 149).
#[test]
fn a_pass_a_pixel_larger_than_its_target_still_draws() {
    let Some((hal, info)) = open() else { return };
    let sid = info.session;
    resources(&hal, sid);
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut frame = 1;
    loop {
        assert!(Instant::now() < deadline, "the oversized pass never drew");
        let before = hal.with_world(|w| w.session(sid).unwrap().stats);
        let mut w = CmdWriter::new(sid);
        w.frame_begin(frame, FrameMeta::default());
        let mut p = pass(TEX, 33, 33, [17.0 / 255.0, 34.0 / 255.0, 51.0 / 255.0, 1.0]);
        p.dpi = 1.41;
        w.pass(p);
        w.draw(draw());
        w.frame_end(TEX, target::TEXTURE);
        hal.submit(&w.buf());
        frame_done(&hal, sid, frame);
        let after = hal.with_world(|w| w.session(sid).unwrap().stats);
        assert_eq!(after.passes - before.passes, 1, "the pass was refused");
        assert_eq!(after.clamped_passes - before.clamped_passes, 1);
        if before.skipped_draws == after.skipped_draws { break; }
        frame += 1;
    }
    let mut w = CmdWriter::new(sid);
    w.texture_readback(TEX, 3, [0, 0, 32, 32]); hal.submit(&w.buf());
    let image = pixels(&hal, sid, 3);
    // the clear in the top corners, the triangle's base along the bottom
    for x in [0, 31] { assert_eq!(&image[x * 4..][..4], &[17, 34, 51, 255]); }
    for x in [6, 16, 26] { assert_eq!(&image[(29 * 32 + x) * 4..][..4], &[255; 4], "x {x}"); }
    assert!(hal.close(sid));
}
