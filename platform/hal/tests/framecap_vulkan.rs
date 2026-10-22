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
const PIPE: Id = Id::new(1, 1);
const W: u32 = 60;
const H: u32 = 30;

/// A full-target triangle in white (the geometry is clip space).
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

fn frame(w: &mut CmdWriter, n: u64) {
    w.clear();
    w.frame_begin(n, FrameMeta::default());
    w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, load: [pass_load::CLEAR; 5], clear: [[0.0, 0.0, 0.0, 1.0]; 4], viewport: [0.0, 0.0, W as f32, H as f32], dpi: 1.0, ..Default::default() });
    let ids = w.put_ids(&[TEX]);
    for i in 0..3 {
        w.draw(Draw { pipeline: PIPE, vb: VERTEX, ib: INDEX, index: [0, 3], inst: Inst::inline(Slice::EMPTY, 1), tex: ids,
            scissor: [i as f32 * 20.0, 0.0, 10.0, H as f32], bounds: [f32::NAN; 4], ..Default::default() });
    }
    w.frame_end(WIN, target::WINDOW);
}

#[test]
fn a_captured_vulkan_frame_replays_to_the_same_pixels() {
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::VULKAN, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    if info.backend != backend_id::VULKAN {
        eprintln!("SKIP: no Vulkan device");
        return;
    }
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
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() }, &shader(), &[u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 0], &[16]);
    w.pipeline_create(PIPE, dialect::SPIRV, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, &mut store, |e| matches!(e, HalEvent::PipelineReady { .. }));
    let mut n = 1;
    loop {
        let before = hal.with_world(|wd| wd.session(sid).unwrap().stats.skipped_draws);
        frame(&mut w, n);
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
    let seq = framecap::arm(Some(sid), None);
    frame(&mut w, n);
    w.snapshot(WIN, 99);
    hal.submit(&w.buf());
    let mut live = Vec::new();
    until(&hal, &mut store, |e| match e {
        HalEvent::Readback { req: 99, status, pixels, .. } => {
            assert_eq!(status, readback::OK);
            live = pixels.to_vec();
            true
        }
        _ => false,
    });
    let cap = framecap::wait(seq, Duration::from_secs(10)).expect("the capture finished");
    let mut cap = (*cap).clone();
    cap.backend = info.backend;
    cap.dialect = info.dialect;
    assert_eq!((cap.passes.len(), cap.draws.len(), cap.pipelines.len(), cap.textures.len(), cap.buffers.len()), (1, 3, 1, 1, 2));
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
