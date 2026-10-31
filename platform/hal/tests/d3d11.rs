//! Native D3D11 coverage (Windows, a hardware device). The same cases as tests/vulkan.rs: passes
//! and draws into a texture and an offscreen window, exact readback, the ledger of a closing
//! session, resize, and the pick variant's ids.
#![cfg(windows)]

use makepad_hal::hlsl::HlslPipeline;
use makepad_hal::ledger::Kind;
use makepad_hal::*;
use std::time::{Duration, Instant};

const TEX: Id = Id::new(1, 1);
const PICK_TEX: Id = Id::new(2, 1);
const VERTEX: Id = Id::new(1, 1);
const INDEX: Id = Id::new(2, 1);
const PIPE: Id = Id::new(1, 1);
const PICK_PIPE: Id = Id::new(2, 1);
const WINDOW: Id = Id::new(1, 1);

/// One geometry input (`GEOMA0`, four floats) passed through; the pixel writes white.
const HLSL: &str = "struct VertexIn { float4 g : GEOMA0; };
float4 vertex_main(VertexIn i) : SV_POSITION { return i.g; }
float4 pixel_main() : SV_TARGET0 { return float4(1.0, 1.0, 1.0, 1.0); }
";

/// The pick variant's contract (shader_ir_hlsl): the draw id in `_pk.x` at b12, written as
/// 24-bit colour.
const PICK_HLSL: &str = "cbuffer _mp_pick : register(b12) { uint4 _pk; };
struct VertexIn { float4 g : GEOMA0; };
float4 vertex_main(VertexIn i) : SV_POSITION { return i.g; }
float4 pixel_main() : SV_TARGET0 { uint p = _pk.x; return float4(float(p & 255u), float((p >> 8) & 255u), float((p >> 16) & 255u), 255.0) / 255.0; }
";

fn open() -> (std::sync::Arc<Hal>, HalInfo) {
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::D3D11, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_eq!(info.backend, backend_id::D3D11, "D3D11 HAL initialization failed");
    assert_eq!(info.dialect, dialect::HLSL);
    (hal, info)
}

fn until(hal: &Hal, sid: u32, mut accept: impl FnMut(HalEvent<'_>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut store = EventStore::new(sid, 64, 65536);
    loop {
        assert!(Instant::now() < deadline, "D3D11 event deadline");
        let mut buf = store.buf();
        let n = hal.pump(10_000_000, &mut buf);
        for (_, event) in store.reader(n, buf.arena_len) {
            match event {
                HalEvent::PipelineFailed { log, .. } => panic!("pipeline: {}", String::from_utf8_lossy(log)),
                HalEvent::DeviceLost { .. } => panic!("D3D11 device lost"),
                _ => {}
            }
            if accept(event) {
                return;
            }
        }
    }
}

fn frame_done(hal: &Hal, sid: u32, frame: u64) {
    until(hal, sid, |e| matches!(e, HalEvent::FrameDone { frame: done, .. } if done == frame));
}

fn payload(source: &str) -> Vec<u8> {
    HlslPipeline { source: source.into(), textures: Vec::new(), samplers: Vec::new() }.encode()
}

fn resources(hal: &Hal, sid: u32) {
    let mut w = CmdWriter::new(sid);
    w.texture_create(TEX, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 32, 32, [1, 1], Attr::default());
    let vertices: Vec<u8> = [-1.0f32, -1.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0].into_iter().flat_map(f32::to_le_bytes).collect();
    w.buffer_create(VERTEX, buffer_kind::VERTEX, vertices.len() as u64, Attr::default());
    w.buffer_update(VERTEX, 0, &vertices);
    w.buffer_create(INDEX, buffer_kind::INDEX, 6, Attr::default());
    w.buffer_update(INDEX, 0, &[0, 0, 1, 0, 2, 0]);
    // one geometry input: stride 16, (offset 0, F32x4, 4 lanes)
    let layout = [16, 0, 3, 4];
    let none = u32::MAX;
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() }, &payload(HLSL), &[none, none, none, none, none, 0], &layout);
    w.pipeline_create(PIPE, dialect::HLSL, Attr::default(), &blob);
    let pick = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, blend: pipeline_blend::RAW, color_formats: [format::BGRA8, 0, 0, 0], ..Default::default() },
        &payload(PICK_HLSL), &[none, none, none, none, none, 0], &layout);
    w.pipeline_create(PICK_PIPE, dialect::HLSL, Attr::default(), &pick);
    hal.submit(&w.buf());
    let (mut normal, mut picked) = (false, false);
    until(hal, sid, |e| {
        match e {
            HalEvent::PipelineReady { pipeline } if pipeline == PIPE => normal = true,
            HalEvent::PipelineReady { pipeline } if pipeline == PICK_PIPE => picked = true,
            _ => {}
        }
        normal && picked
    });
    assert_eq!(hal.with_world(|w| w.session(sid).unwrap().stats.bad_args), 0);
}

fn pass(texture: Id, width: u32, height: u32, clear: [f32; 4]) -> Pass {
    Pass { color: [texture, Id::NONE, Id::NONE, Id::NONE], load: [pass_load::CLEAR; 5], clear: [clear; 4],
        viewport: [0.0, 0.0, width as f32, height as f32], dpi: 1.0, ..Default::default() }
}

fn draw() -> Draw {
    Draw { pipeline: PIPE, vb: VERTEX, ib: INDEX, index: [0, 3], inst: Inst::inline(Slice::EMPTY, 1), scissor: [f32::NAN; 4], ..Default::default() }
}

fn rendered(hal: &Hal, sid: u32, target: Id, kind: u32, size: [u32; 2], frame: u64) {
    let before = hal.with_world(|w| w.session(sid).unwrap().stats);
    let mut w = CmdWriter::new(sid);
    w.frame_begin(frame, FrameMeta::default());
    w.pass(pass(if kind == target::WINDOW { Id::NONE } else { target }, size[0], size[1], [0.0, 0.0, 0.0, 1.0]));
    let third = size[0] as f32 / 3.0;
    let mut left = draw();
    left.scissor = [0.0, 0.0, third, size[1] as f32];
    let mut middle = draw();
    middle.scissor = [third, 0.0, third, size[1] as f32];
    w.draw(left);
    w.draw(middle);
    let mut second = pass(if kind == target::WINDOW { Id::NONE } else { target }, size[0], size[1], [0.0; 4]);
    second.load[0] = pass_load::LOAD;
    w.pass(second);
    let mut right = draw();
    right.scissor = [2.0 * third, 0.0, size[0] as f32 - 2.0 * third, size[1] as f32];
    w.draw(right);
    w.frame_end(target, kind);
    hal.submit(&w.buf());
    frame_done(hal, sid, frame);
    let after = hal.with_world(|w| w.session(sid).unwrap().stats);
    assert_eq!(after.bad_args, 0);
    assert_eq!(after.passes - before.passes, 2);
    assert_eq!(after.draws - before.draws, 3);
    assert_eq!(after.skipped_draws, before.skipped_draws, "every draw encoded (pipelines were ready)");
}

fn pixels(hal: &Hal, sid: u32, req: u32) -> Vec<u8> {
    let mut pixels = Vec::new();
    until(hal, sid, |e| match e {
        HalEvent::Readback { req: got, status, pixels: p, .. } if got == req => {
            assert_eq!(status, readback::OK);
            pixels = p.to_vec();
            true
        }
        _ => false,
    });
    pixels
}

#[test]
fn offscreen_two_passes_three_draws() {
    let (hal, info) = open();
    resources(&hal, info.session);
    rendered(&hal, info.session, TEX, target::TEXTURE, [32, 32], 1);
    let mut w = CmdWriter::new(info.session);
    w.texture_readback(TEX, 1, [0, 0, 32, 32]);
    hal.submit(&w.buf());
    let image = pixels(&hal, info.session, 1);
    // the triangle's base (clip y = -1) is the bottom of the target: row 0 is the top
    for x in [5, 16, 26] {
        assert_eq!(&image[(27 * 32 + x) * 4..][..4], &[255; 4]);
    }
    for x in [5, 26] {
        assert_eq!(&image[(2 * 32 + x) * 4..][..4], &[0, 0, 0, 255]);
    }
    assert!(hal.close(info.session));
}

#[test]
fn cleared_target_readback_is_exact() {
    let (hal, info) = open();
    let mut w = CmdWriter::new(info.session);
    w.texture_create(TEX, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 7, 5, [1, 1], Attr::default());
    w.frame_begin(1, FrameMeta::default());
    w.pass(pass(TEX, 7, 5, [17.0 / 255.0, 34.0 / 255.0, 51.0 / 255.0, 1.0]));
    w.frame_end(TEX, target::TEXTURE);
    hal.submit(&w.buf());
    frame_done(&hal, info.session, 1);
    w.clear();
    w.texture_readback(TEX, 2, [0, 0, 7, 5]);
    hal.submit(&w.buf());
    assert_eq!(pixels(&hal, info.session, 2), [17, 34, 51, 255].repeat(35));
    assert!(hal.close(info.session));
}

fn ledger(hal: &Hal, sid: u32) -> Vec<(Kind, u32, u32, u64)> {
    hal.with_world(|w| {
        let mut rows: Vec<_> = w.session(sid).unwrap().ledger.entries().filter(|r| r.kind != Kind::Window).map(|r| (r.kind, r.index, r.gen, r.bytes)).collect();
        rows.sort();
        rows
    })
}

#[test]
fn session_close_frees_exactly_its_ledger() {
    let (hal, info) = open();
    let other = hal.open(&OpenReq { backend: backend_id::D3D11, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() }).session;
    resources(&hal, info.session);
    resources(&hal, other);
    let own = ledger(&hal, info.session);
    let survivor = ledger(&hal, other);
    assert_eq!(own.len(), 5);
    assert_eq!(survivor.len(), 5);
    let total = hal.with_world(|w| Kind::ALL.into_iter().map(|k| w.count(k)).sum::<usize>());
    assert!(hal.close(info.session));
    assert!(hal.with_world(|w| w.session(info.session).is_none()));
    assert_eq!(ledger(&hal, other), survivor);
    assert_eq!(hal.with_world(|w| Kind::ALL.into_iter().map(|k| w.count(k)).sum::<usize>()), total - own.len());
    assert_eq!(hal.backend_inits(), 1);
    assert!(hal.close(other));
}

#[test]
fn offscreen_window_resize_keeps_resources() {
    let (hal, info) = open();
    resources(&hal, info.session);
    let mut w = CmdWriter::new(info.session);
    w.window_create(WINDOW, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 32.0, 32.0], false);
    hal.submit(&w.buf());
    rendered(&hal, info.session, WINDOW, target::WINDOW, [32, 32], 1);
    let retained = ledger(&hal, info.session);
    w.clear();
    w.window_geom(WINDOW, [0.0, 0.0, 80.0, 48.0]);
    hal.submit(&w.buf());
    rendered(&hal, info.session, WINDOW, target::WINDOW, [80, 48], 2);
    assert_eq!(ledger(&hal, info.session), retained);
    w.clear();
    w.snapshot(WINDOW, 3);
    hal.submit(&w.buf());
    let image = pixels(&hal, info.session, 3);
    assert_eq!(image.len(), 80 * 48 * 4);
    assert_eq!(&image[(24 * 80 + 40) * 4..][..4], &[255; 4]);
    assert!(hal.close(info.session));
}

#[test]
fn pick_frame_answers_the_draw_id() {
    let (hal, info) = open();
    let sid = info.session;
    resources(&hal, sid);
    let mut w = CmdWriter::new(sid);
    w.window_create(WINDOW, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 32.0, 32.0], false);
    w.texture_create(PICK_TEX, usage::RENDER_TARGET | usage::SAMPLED, format::BGRA8, 32, 32, [1, 1], Attr::default());
    w.frame_begin(1, FrameMeta::default());
    w.pass(pass(PICK_TEX, 32, 32, [0.0; 4]));
    let mut d = draw();
    d.pipeline = PICK_PIPE;
    d.pick = 0x12_3456;
    w.draw(d);
    w.frame_end(WINDOW, target::WINDOW | target::PICK);
    hal.submit(&w.buf());
    frame_done(&hal, sid, 1);
    w.clear();
    // inside the triangle (its base is the bottom rows) and outside it (a top corner)
    w.pick(WINDOW, 7, [16.0, 28.0]);
    w.pick(WINDOW, 8, [1.0, 1.0]);
    hal.submit(&w.buf());
    let mut answers = std::collections::HashMap::new();
    until(&hal, sid, |e| {
        if let HalEvent::PickResult { req, ids } = e {
            answers.insert(req, ids.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect::<Vec<_>>());
        }
        answers.len() == 2
    });
    assert_eq!(answers[&7].first(), Some(&0x12_3456));
    assert!(answers[&8].is_empty(), "nothing drawn near the corner: {:?}", answers[&8]);
    assert!(hal.close(sid));
}

/// A device loss (simulated: the device is dropped and a new one made, as on a removal) keeps
/// every id: the session hears `DeviceLost { 1 }` then `DeviceReset { 2 }`, the pipelines answer
/// ready again, the buffers take their contents again under the same ids and the same frame
/// renders the same pixels; the ledger is unchanged and nothing counts as stale or bad.
#[test]
fn device_reset_keeps_ids_and_renders_again() {
    let (hal, info) = open();
    let sid = info.session;
    resources(&hal, sid);
    let mut w = CmdWriter::new(sid);
    w.window_create(WINDOW, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 32.0, 32.0], false);
    hal.submit(&w.buf());
    rendered(&hal, sid, TEX, target::TEXTURE, [32, 32], 1);
    rendered(&hal, sid, WINDOW, target::WINDOW, [32, 32], 2);
    w.clear();
    w.texture_readback(TEX, 1, [0, 0, 32, 32]);
    hal.submit(&w.buf());
    let before = pixels(&hal, sid, 1);
    let rows = ledger(&hal, sid);

    hal.simulate_device_loss();
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut store = EventStore::new(sid, 64, 65536);
    let (mut lost, mut reset, mut ready) = (None, None, Vec::new());
    while !(reset.is_some() && ready.contains(&PIPE) && ready.contains(&PICK_PIPE)) {
        assert!(Instant::now() < deadline, "no device reset");
        let mut buf = store.buf();
        let n = hal.pump(10_000_000, &mut buf);
        for (_, event) in store.reader(n, buf.arena_len) {
            match event {
                HalEvent::DeviceLost { generation } => {
                    assert!(reset.is_none(), "DeviceLost after DeviceReset");
                    lost = Some(generation);
                }
                HalEvent::DeviceReset { generation } => {
                    assert!(lost.is_some(), "DeviceReset without DeviceLost");
                    reset = Some(generation);
                }
                HalEvent::PipelineReady { pipeline } if reset.is_some() => ready.push(pipeline),
                HalEvent::PipelineFailed { log, .. } => panic!("pipeline: {}", String::from_utf8_lossy(log)),
                _ => {}
            }
        }
    }
    assert_eq!((lost, reset), (Some(1), Some(2)));
    assert_eq!(hal.with_world(|w| w.device_generation), 2);

    // contents are the app's to send again: the same ids, no creates
    let vertices: Vec<u8> = [-1.0f32, -1.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0].into_iter().flat_map(f32::to_le_bytes).collect();
    w.clear();
    w.buffer_update(VERTEX, 0, &vertices);
    w.buffer_update(INDEX, 0, &[0, 0, 1, 0, 2, 0]);
    hal.submit(&w.buf());
    rendered(&hal, sid, TEX, target::TEXTURE, [32, 32], 3);
    rendered(&hal, sid, WINDOW, target::WINDOW, [32, 32], 4);
    w.clear();
    w.texture_readback(TEX, 2, [0, 0, 32, 32]);
    w.snapshot(WINDOW, 3);
    hal.submit(&w.buf());
    assert_eq!(pixels(&hal, sid, 2), before);
    let window = pixels(&hal, sid, 3);
    assert_eq!(&window[(27 * 32 + 16) * 4..][..4], &[255; 4]);
    assert_eq!(ledger(&hal, sid), rows);
    let stats = hal.with_world(|w| w.session(sid).unwrap().stats);
    assert_eq!((stats.bad_args, stats.stale_ids), (0, 0));
    assert!(hal.close(sid));
}
