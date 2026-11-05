//! Native Metal coverage (offscreen windows: no window shows).
#![cfg(target_vendor = "apple")]

use makepad_hal::*;
use std::time::{Duration, Instant};

/// An upload from a temporary (`&vec![..]`: freed at the end of its statement, its block handed
/// out again before the submit) lands the bytes of the call: the writer copied them at the call.
#[test]
fn uploads_from_freed_temporaries_land_the_bytes_of_the_call() {
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::METAL, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_eq!(info.backend, backend_id::METAL, "Metal HAL initialization failed");
    let sid = info.session;
    let (buffer, texture) = (Id::new(9, 1), Id::new(9, 1));
    let want_buffer: Vec<u8> = (0..4096u32).map(|i| (i * 7 + 3) as u8).collect();
    let want_texels: Vec<u8> = (0..16 * 16 * 4u32).map(|i| (i * 13 + 5) as u8).collect();
    let mut w = CmdWriter::new(sid);
    w.buffer_create(buffer, buffer_kind::INSTANCE, 4096, Attr::default());
    w.texture_create(texture, usage::SAMPLED | usage::RENDER_TARGET, format::RGBA8, 16, 16, [1, 1], Attr::default());
    w.buffer_update(buffer, 0, &want_buffer.clone());
    w.texture_update(texture, 0, [0, 0, 16, 16], 0, &want_texels.clone());
    let scribble = [vec![0xeeu8; 4096], vec![0xee; 1024]];
    w.read_buffers(1, &[BufferReadRange { id: buffer, offset: 0, len: 4096 }]);
    w.texture_readback(texture, 2, [0, 0, 16, 16]);
    hal.submit(&w.buf());
    drop(scribble);
    let mut got: [Vec<u8>; 2] = Default::default();
    let mut store = EventStore::new(sid, 64, 1 << 16);
    let deadline = Instant::now() + Duration::from_secs(30);
    while got.iter().any(|g| g.is_empty()) {
        assert!(Instant::now() < deadline, "Metal readback deadline");
        let mut buf = store.buf();
        let n = hal.pump(10_000_000, &mut buf);
        for (_, e) in store.reader(n, buf.arena_len) {
            if let HalEvent::BufferLeases { req: 1, status, records } = e {
                use makepad_hal::buffers::Wire;
                assert_eq!(status, readback::OK);
                let record = makepad_hal::lease::Record::read(records);
                got[0] = unsafe { std::slice::from_raw_parts(record.ptr as *const u8, record.len as usize) }.to_vec();
                hal_lease_release(record.token);
            }
            if let HalEvent::Readback { req: req @ 1..=2, status, pixels, .. } = e {
                assert_eq!(status, readback::OK);
                got[req as usize - 1] = pixels.to_vec();
            }
        }
    }
    assert!(got[0] == want_buffer, "the buffer holds other bytes than its upload's");
    assert!(got[1] == want_texels, "the texture holds other bytes than its upload's");
    assert_eq!(hal.with_world(|w| w.session(sid).unwrap().stats.bad_args), 0);
    assert!(hal.close(sid));
}

/// A full-target quad (instance: rect) whose fragment writes `color`: the shader's own colour.
fn solid_msl(color: &str) -> String {
    format!(
        "#include <metal_stdlib>
using namespace metal;
struct V {{ float4 pos [[position]]; }};
vertex V vertex_main(uint vid [[vertex_id]], uint iid [[instance_id]], constant float2 *geom [[buffer(0)]], constant float4 *inst [[buffer(1)]]) {{
    float4 r = inst[iid];
    float2 p = r.xy + geom[vid] * r.zw;
    V o;
    o.pos = float4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    return o;
}}
fragment float4 fragment_main(V i [[stage_in]]) {{ return float4({color}); }}
"
    )
}

/// One app session that draws the quad with `msl` into an 8x8 texture: (the session's stats
/// once the pipeline is ready, the centre pixel).
fn draw_solid(hal: &Hal, msl: &str) -> (makepad_hal::session::Stats, [u8; 4]) {
    const TEX: Id = Id::new(1, 1);
    const QUAD: Id = Id::new(1, 1);
    const INDEX: Id = Id::new(2, 1);
    const PIPE: Id = Id::new(1, 1);
    // the same app each time: opening it again closes the session before (a relaunch)
    let info = hal.open(&OpenReq { backend: backend_id::METAL, flags: open_flags::OFFSCREEN_WINDOWS, app_key: 7, ..Default::default() });
    assert_eq!(info.backend, backend_id::METAL, "Metal HAL initialization failed");
    let sid = info.session;
    let mut store = EventStore::new(sid, 64, 1 << 16);
    let mut until = |accept: &mut dyn FnMut(HalEvent<'_>) -> bool| {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            assert!(Instant::now() < deadline, "Metal event deadline");
            let mut buf = store.buf();
            let n = hal.pump(10_000_000, &mut buf);
            for (_, e) in store.reader(n, buf.arena_len) {
                if let HalEvent::PipelineFailed { log, .. } = e {
                    panic!("pipeline: {}", String::from_utf8_lossy(log));
                }
                if accept(e) {
                    return;
                }
            }
        }
    };
    let mut w = CmdWriter::new(sid);
    w.texture_create(TEX, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 8, 8, [1, 1], Attr::default());
    let quad: Vec<u8> = [0.0f32, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    w.buffer_create(QUAD, buffer_kind::VERTEX, quad.len() as u64, Attr::default());
    w.buffer_update(QUAD, 0, &quad);
    let index: Vec<u8> = [0u16, 1, 2, 0, 2, 3].iter().flat_map(|i| i.to_le_bytes()).collect();
    w.buffer_create(INDEX, buffer_kind::INDEX, index.len() as u64, Attr::default());
    w.buffer_update(INDEX, 0, &index);
    let none = pipeline_binding::NONE;
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, instance_stride: 4, ..Default::default() }, msl.as_bytes(), &[none, none, none, none, none, 0], &[4]);
    w.pipeline_create(PIPE, dialect::MSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&mut |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == PIPE));
    let stats = hal.with_world(|wd| wd.session(sid).unwrap().stats);
    w.clear();
    w.frame_begin(1, FrameMeta::default());
    w.pass(Pass { color: [TEX, Id::NONE, Id::NONE, Id::NONE], depth: Id::NONE, load: [pass_load::CLEAR; 5], clear: [[0.0, 0.0, 0.0, 1.0]; 4], viewport: [0.0, 0.0, 8.0, 8.0], dpi: 1.0, ..Default::default() });
    let rect = [0.0f32, 0.0, 1.0, 1.0];
    let inst = Inst::inline(w.keep(&rect[..]), 1);
    w.draw(Draw { pipeline: PIPE, vb: QUAD, ib: INDEX, index: [0, 6], inst, scissor: [f32::NAN; 4], bounds: [f32::NAN; 4], ..Default::default() });
    w.frame_end(TEX, target::TEXTURE);
    hal.submit(&w.buf());
    until(&mut |e| matches!(e, HalEvent::FrameDone { frame: 1, .. }));
    w.clear();
    w.texture_readback(TEX, 1, [0, 0, 8, 8]);
    hal.submit(&w.buf());
    let mut pixel = [0u8; 4];
    until(&mut |e| match e {
        HalEvent::Readback { req: 1, status, pixels, .. } => {
            assert_eq!(status, readback::OK);
            pixel.copy_from_slice(&pixels[(4 * 8 + 4) * 4..][..4]);
            true
        }
        _ => false,
    });
    assert_eq!(hal.with_world(|wd| wd.session(sid).unwrap().stats.skipped_draws), 0, "the draw was drawn");
    (stats, pixel)
}

/// A restart in place makes its pipelines again: one whose source and description equal one of
/// the app's session before takes its compiled states (nothing compiles) and draws as it did; one
/// whose source changed compiles and draws its own colour; and what a session left that the next
/// did not take goes with that session's close.
#[test]
fn a_relaunch_takes_the_compiled_pipelines_of_the_session_before_and_never_a_changed_one() {
    let hal = Hal::new();
    let (red, green) = (solid_msl("1.0, 0.0, 0.0, 1.0"), solid_msl("0.0, 1.0, 0.0, 1.0"));
    let (first, pixel) = draw_solid(&hal, &red);
    assert_eq!((first.pipelines_taken, pixel), (0, [255, 0, 0, 255]), "the first session compiles");
    let (again, pixel) = draw_solid(&hal, &red);
    assert_eq!((again.pipelines_taken, pixel), (1, [255, 0, 0, 255]), "the relaunch takes the states of the session before");
    let (changed, pixel) = draw_solid(&hal, &green);
    assert_eq!((changed.pipelines_taken, pixel), (0, [0, 255, 0, 255]), "a changed source compiles");
    let (back, pixel) = draw_solid(&hal, &red);
    assert_eq!((back.pipelines_taken, pixel), (0, [255, 0, 0, 255]), "the red states went with the close of the session that did not take them");
}
