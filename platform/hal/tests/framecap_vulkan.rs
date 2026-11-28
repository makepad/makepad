//! Frame capture on Vulkan (Linux, offscreen windows): a recorded frame replays in a private HAL
//! to the same pixels, a cut after draw k shows the first k draws, the texture the frame names is
//! captured with its contents.
#![cfg(target_os = "linux")]

use makepad_hal::framecap::replay::{Cut, Opts, Replayer};
use makepad_hal::framecap::{self, file};
use makepad_hal::spirv::SpirvPipeline;
use makepad_hal::*;
use makepad_shader::shader_ir::*;
use makepad_shader::shader_ir_spirv::ir_to_spirv;
use std::time::{Duration, Instant};

const WIN: Id = Id::new(1, 1);
const TEX: Id = Id::new(1, 1);
const VERTEX: Id = Id::new(1, 1);
const INDEX: Id = Id::new(2, 1);
const ROWS: Id = Id::new(3, 1);
const PIPE: Id = Id::new(1, 1);
const W: u32 = 60;
const H: u32 = 30;

/// A full-target triangle in the colour of its instance row (the geometry is clip space).
fn shader() -> Vec<u8> {
    let mut ir = IrModule::new();
    let vec4 = ir.vec_ty(4, IrScalar::F32);
    let colors = ir.ty(IrTy::Array(vec4, 0));
    let rows = ir.global(IrGlobalKind::InstanceBuffer, Default::default(), colors);
    ir.globals[rows as usize].binding = Some(0);
    let mut vertex = IrFunction::new("vertex_main".into(), TY_VOID);
    vertex.entry = Some(IrEntry { stage: IrStage::Vertex,
        inputs: vec![IrEntryIo { name: "geometry".into(), ty: vec4, binding: IrIoBinding::Location(0) },
            IrEntryIo { name: "row".into(), ty: TY_U32, binding: IrIoBinding::Builtin(IrBuiltinIo::InstanceIndex) }],
        outputs: vec![IrEntryIo { name: "position".into(), ty: vec4, binding: IrIoBinding::Builtin(IrBuiltinIo::Position) },
            IrEntryIo { name: "color".into(), ty: vec4, binding: IrIoBinding::Location(0) }] });
    let input = vertex.expr(ExprKind::EntryIn(0), vec4);
    let output = vertex.expr(ExprKind::EntryOut(0), vec4);
    let table = vertex.expr(ExprKind::Global(rows), colors);
    let row = vertex.expr(ExprKind::EntryIn(1), TY_U32);
    let color = vertex.expr(ExprKind::Index(table, row), vec4);
    let varying = vertex.expr(ExprKind::EntryOut(1), vec4);
    vertex.body.extend([Stmt::Assign(output, input), Stmt::Assign(varying, color), Stmt::Return(NO_EXPR)]);
    let mut fragment = IrFunction::new("fragment_main".into(), TY_VOID);
    fragment.entry = Some(IrEntry { stage: IrStage::Fragment,
        inputs: vec![IrEntryIo { name: "color".into(), ty: vec4, binding: IrIoBinding::Location(0) }],
        outputs: vec![IrEntryIo { name: "color".into(), ty: vec4, binding: IrIoBinding::Location(0) }] });
    let color = fragment.expr(ExprKind::EntryIn(0), vec4);
    let output = fragment.expr(ExprKind::EntryOut(0), vec4);
    fragment.body.extend([Stmt::Assign(output, color), Stmt::Return(NO_EXPR)]);
    ir.functions.extend([vertex, fragment]);
    SpirvPipeline { vertex: ir_to_spirv(&ir, 0, false).unwrap(), fragment: ir_to_spirv(&ir, 1, false).unwrap(),
        texture_base: 0, sampler_base: 0, depth_binding: u32::MAX, instance_binding: 0,
        pick_binding: u32::MAX, pick_texture_base: u32::MAX, textures: Vec::new(), samplers: Vec::new(), sampler_textures: Vec::new(), xr_vertex: Vec::new(), xr_fragment: Vec::new() }.encode()
}

fn until(hal: &Hal, store: &mut EventStore, mut accept: impl FnMut(HalEvent<'_>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        assert!(Instant::now() < deadline, "event deadline");
        let mut buf = store.buf();
        let n = hal.pump(10_000_000, &mut buf);
        let len = buf.arena_len;
        for (_, e) in store.reader(n, len) {
            if let HalEvent::PipelineFailed { log, .. } = e {
                panic!("pipeline: {}", String::from_utf8_lossy(log));
            }
            if accept(e) {
                return;
            }
        }
    }
}

fn frame(w: &mut CmdWriter, n: u64, row: u32) {
    w.frame_begin(n, FrameMeta::default());
    w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, load: [pass_load::CLEAR; 5], clear: [[0.0, 0.0, 0.0, 1.0]; 4], viewport: [0.0, 0.0, W as f32, H as f32], dpi: 1.0, ..Default::default() });
    let ids = w.put_ids(&[TEX]);
    for i in 0..3 {
        w.draw(Draw { pipeline: PIPE, vb: VERTEX, ib: INDEX, index: [0, 3], inst: Inst::buffer(ROWS, row, 1), tex: ids,
            scissor: [i as f32 * 20.0, 0.0, 10.0, H as f32], bounds: [f32::NAN; 4], ..Default::default() });
    }
    w.frame_end(WIN, target::WINDOW);
}

fn snapshot(hal: &Hal, store: &mut EventStore, w: &mut CmdWriter) -> Vec<u8> {
    w.snapshot(WIN, 99);
    hal.submit(&w.buf());
    let mut live = Vec::new();
    until(hal, store, |e| match e {
        HalEvent::Readback { req: 99, status, lease, .. } => {
            // SAFETY: this process pumped the event and takes its lease once.
            let pixels = unsafe { makepad_hal::lease::Held::from_payload(lease) }.unwrap();
            assert_eq!(status, readback::OK);
            live = pixels.to_vec();
            true
        }
        _ => false,
    });
    live
}

#[test]
fn a_captured_vulkan_frame_replays_to_the_same_pixels() {
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::VULKAN, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_eq!(info.backend, backend_id::VULKAN, "Vulkan HAL initialization failed");
    let sid = info.session;
    let mut store = EventStore::new(sid, 128, 1 << 22);
    let mut w = CmdWriter::new(sid);
    w.window_create(WIN, window_kind::NORMAL, Id::NONE, [0.0, 0.0, W as f32, H as f32], false);
    let texels: Vec<u8> = (0..16u32).flat_map(|i| [(i * 16) as u8, 7, 9, 255]).collect();
    w.texture_create(TEX, usage::SAMPLED, format::RGBA8, 4, 4, [1, 1], Attr::default());
    w.texture_update(TEX, 0, [0, 0, 4, 4], 0, &texels);
    let vertices: Vec<u8> = [-1.0f32, -1.0, 0.0, 1.0, 3.0, -1.0, 0.0, 1.0, -1.0, 3.0, 0.0, 1.0].into_iter().flat_map(f32::to_le_bytes).collect();
    w.buffer_create(VERTEX, buffer_kind::VERTEX, vertices.len() as u64, Attr::default());
    w.buffer_update(VERTEX, 0, &vertices);
    w.buffer_create(INDEX, buffer_kind::INDEX, 6, Attr::default());
    w.buffer_update(INDEX, 0, &[0, 0, 1, 0, 2, 0]);
    let [red, white] = [[1.0f32, 0.0, 0.0, 1.0], [1.0; 4]].map(|c| c.into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>());
    w.buffer_create(ROWS, buffer_kind::INSTANCE | buffer_kind::APPEND, 32, Attr::default());
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, instance_stride: 4, ..Default::default() }, &shader(), &[u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 0], &[16]);
    w.pipeline_create(PIPE, dialect::SPIRV, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, &mut store, |e| matches!(e, HalEvent::PipelineReady { .. }));
    let mut n = 1;
    loop {
        let before = hal.with_world(|wd| wd.session(sid).unwrap().stats.skipped_draws);
        w.clear();
        if n == 1 { w.buffer_update(ROWS, 0, &red); }
        frame(&mut w, n, 0);
        hal.submit(&w.buf());
        let mut partial = false;
        until(&hal, &mut store, |e| match e {
            HalEvent::FramePartial { frame, .. } if frame == n => {
                partial = true;
                false
            }
            HalEvent::FrameDone { frame, .. } => frame == n,
            _ => false,
        });
        n += 1;
        if !partial && hal.with_world(|wd| wd.session(sid).unwrap().stats.skipped_draws) == before {
            break;
        }
        assert!(n < 200, "draws stayed skipped");
    }
    w.clear();
    let first = snapshot(&hal, &mut store, &mut w);
    let at = ((10 * W + 45) * 4) as usize;
    assert_eq!(&first[at..at + 4], &[255, 0, 0, 255], "the first frame draws the red row");
    let seq = framecap::arm(Some(sid), None);
    w.clear();
    let before = hal.with_world(|w| w.session(sid).unwrap().stats);
    w.buffer_update_parts(ROWS, &[(16, &white[..8]), (24, &white[8..])]);
    frame(&mut w, n, 1);
    let live = snapshot(&hal, &mut store, &mut w);
    assert_eq!(&live[at..at + 4], &[255; 4], "the next frame draws the newly appended white row");
    let stats = hal.with_world(|w| w.session(sid).unwrap().stats);
    assert_eq!((stats.buffer_updates - before.buffer_updates, stats.buffer_parts - before.buffer_parts), (1, 2),
        "the white row reaches the backend as one update with two parts");
    assert!(stats.arena_creates >= stats.arena_allocs, "arena creations include the session's synchronous allocations");
    let staged = stats.staged_bytes;
    let copies = u64::from(staged != 0);
    assert_eq!((stats.copy_cmds - before.copy_cmds, stats.copy_regions - before.copy_regions, stats.upload_barriers - before.upload_barriers),
        (copies, copies, copies), "the staged white row needs one copy command, one merged region and one barrier");
    if info.device_kind != device_kind::DISCRETE { assert_eq!(staged, 0, "only a discrete device stages APPEND writes"); }
    assert!(staged == 0 || staged == 32, "both 16-byte writes use the slab's route: {staged} bytes staged");
    eprintln!("Vulkan capture on {}: {} APPEND", info.device_name(), if staged == 0 { "mapped" } else { "staged" });
    let cap = framecap::wait(seq, Duration::from_secs(10)).expect("the capture finished");
    let mut cap = (*cap).clone();
    cap.backend = info.backend;
    cap.dialect = info.dialect;
    assert_eq!((cap.passes.len(), cap.draws.len(), cap.pipelines.len(), cap.textures.len(), cap.buffers.len()), (1, 3, 1, 1, 3));
    assert_eq!(cap.buffer(ROWS).unwrap().contents.as_deref(), Some([red, white].concat().as_slice()), "the packet sees both appended rows");
    assert_eq!(cap.textures[0].contents.as_deref(), Some(&texels[..]));
    let cap = file::decode(&file::encode(&cap)).unwrap();
    let mut r = Replayer::new(&cap).unwrap();
    r.run(&cap, Cut::all(&cap), Opts::default()).unwrap();
    let rt = r.target_of(&cap, 0);
    let img = r.read(&cap, rt, None).unwrap();
    assert_eq!(img.rgba, live, "the replay draws what the app drew");
    assert_eq!(img.px(45, 10), [255; 4]);
    r.run(&cap, Cut::after(&cap, 1), Opts::default()).unwrap();
    let two = r.read(&cap, rt, None).unwrap();
    assert_eq!((two.px(5, 10), two.px(25, 10), two.px(45, 10)), ([255; 4], [255; 4], [0, 0, 0, 255]));
}
