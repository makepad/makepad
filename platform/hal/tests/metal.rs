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

/// An MSAA pass (`Pass::samples`: 4, which every Apple GPU has) draws four samples a pixel and
/// resolves them into its attachments: a slanted edge leaves pixels a quarter, a half and three
/// quarters covered, and no other shade. Two passes side by side, as the eyes of an XR frame, both
/// with depth, the second loading what the first left: the first's half keeps its picture through
/// the second's resolve. Until the pipeline's state for four samples is ready the passes draw
/// single sampled; no draw is left out.
#[test]
fn an_msaa_pass_resolves_four_samples_and_a_loading_pass_continues_it() {
    const TEX: Id = Id::new(1, 1);
    const DEPTH: Id = Id::new(2, 1);
    const TRI: Id = Id::new(1, 1);
    const INDEX: Id = Id::new(2, 1);
    const PIPE: Id = Id::new(1, 1);
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::METAL, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_eq!(info.backend, backend_id::METAL, "Metal HAL initialization failed");
    assert!(info.msaa & 4 != 0, "the GPU has 4 samples a pixel, and the HAL says so (msaa {:#b})", info.msaa);
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
    w.texture_create(TEX, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 16, 8, [1, 1], Attr::default());
    w.texture_create(DEPTH, usage::RENDER_TARGET, format::DEPTH32F, 16, 8, [1, 1], Attr::default());
    // a triangle whose long edge is not at 45 degrees: its pixels are covered by every quarter
    let tri: Vec<u8> = [0.0f32, 0.0, 1.0, 0.0, 0.0, 0.7].iter().flat_map(|f| f.to_le_bytes()).collect();
    w.buffer_create(TRI, buffer_kind::VERTEX, tri.len() as u64, Attr::default());
    w.buffer_update(TRI, 0, &tri);
    let index: Vec<u8> = [0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
    w.buffer_create(INDEX, buffer_kind::INDEX, index.len() as u64, Attr::default());
    w.buffer_update(INDEX, 0, &index);
    let none = pipeline_binding::NONE;
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, instance_stride: 4, ..Default::default() }, solid_msl("1.0, 0.0, 0.0, 1.0").as_bytes(), &[none, none, none, none, none, 0], &[4]);
    w.pipeline_create(PIPE, dialect::MSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&mut |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == PIPE));
    let frame = |w: &mut CmdWriter, n: u64| {
        w.clear();
        w.frame_begin(n, FrameMeta::default());
        for eye in 0..2 {
            let load = if eye == 0 { pass_load::CLEAR } else { pass_load::LOAD };
            w.pass(Pass { color: [TEX, Id::NONE, Id::NONE, Id::NONE], depth: DEPTH, load: [load; 5], clear: [[0.0, 0.0, 0.0, 1.0]; 4], clear_depth: 1.0, viewport: [8.0 * eye as f32, 0.0, 8.0, 8.0], dpi: 1.0, samples: 4, ..Default::default() });
            let rect = [0.0f32, 0.0, 1.0, 1.0];
            let inst = Inst::inline(w.keep(&rect[..]), 1);
            w.draw(Draw { pipeline: PIPE, vb: TRI, ib: INDEX, index: [0, 3], inst, scissor: [f32::NAN; 4], bounds: [f32::NAN; 4], ..Default::default() });
        }
        w.frame_end(TEX, target::TEXTURE);
    };
    // the first frame asks for the four-sample state (drawn single sampled meanwhile); the second draws with it
    frame(&mut w, 1);
    hal.submit(&w.buf());
    let (mut done, mut variant) = (false, false);
    until(&mut |e| {
        match e {
            HalEvent::FrameDone { frame: 1, .. } => done = true,
            HalEvent::PipelineReady { pipeline } if pipeline == PIPE => variant = true,
            _ => {}
        }
        done && variant
    });
    frame(&mut w, 2);
    hal.submit(&w.buf());
    until(&mut |e| matches!(e, HalEvent::FrameDone { frame: 2, .. }));
    w.clear();
    w.texture_readback(TEX, 1, [0, 0, 16, 8]);
    hal.submit(&w.buf());
    let mut red = Vec::new();
    until(&mut |e| match e {
        HalEvent::Readback { req: 1, status, pixels, .. } => {
            assert_eq!(status, readback::OK);
            red = pixels.chunks(4).map(|p| p[0]).collect();
            true
        }
        _ => false,
    });
    assert_eq!(red.len(), 16 * 8);
    for (k, r) in red.iter().enumerate() {
        let q = *r as f32 * 4.0 / 255.0;
        assert!((q - q.round()).abs() * 255.0 / 4.0 <= 1.5, "pixel ({}, {}) red {r}: not a quarter's step of coverage, so not 4 samples a pixel", k % 16, k / 16);
    }
    for (eye, half) in ["the first", "the second"].iter().enumerate() {
        let px = |x: usize, y: usize| red[y * 16 + eye * 8 + x];
        let edge = (0..8).flat_map(|y| (0..8).map(move |x| (x, y))).filter(|&(x, y)| px(x, y) != 0 && px(x, y) != 255).count();
        assert!(px(0, 0) == 255 && px(7, 7) == 0, "{half} pass's half holds its triangle (corner {} and {}): {red:?}", px(0, 0), px(7, 7));
        assert!(edge >= 4, "{half} pass's edge is resolved from its samples: {edge} pixels part covered, {red:?}");
    }
    assert_eq!(hal.with_world(|wd| wd.session(sid).unwrap().stats.skipped_draws), 0, "no draw was left out while the four-sample state compiled");
    assert!(hal.close(sid));
}
