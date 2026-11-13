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
            if let HalEvent::Readback { req: req @ 1..=2, status, lease, .. } = e {
                // SAFETY: this process pumped the event and takes its lease once.
                let pixels = unsafe { makepad_hal::lease::Held::from_payload(lease) }.unwrap();
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

/// The quad of `solid_msl` whose fragment writes the first vec4 of the pass's uniform block
/// (`pipeline_binding::PASS` at buffer 2): a view of the pass shows its own block.
fn pass_color_msl() -> String {
    solid_msl("pass[0]").replace("fragment_main(V i [[stage_in]])", "fragment_main(V i [[stage_in]], constant float4 *pass [[buffer(2)]])")
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
        HalEvent::Readback { req: 1, status, lease, .. } => {
            // SAFETY: this process pumped the event and takes its lease once.
            let pixels = unsafe { makepad_hal::lease::Held::from_payload(lease) }.unwrap();
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
/// with depth, the second loading what the first left (their twins are kept, `exec::twins_kept`):
/// the first's half keeps its picture through the second's resolve. Until the pipeline's state for
/// four samples is ready the passes draw single sampled; no draw is left out. The frame that state
/// is ready between the eyes (the first eye single sampled, the second at four samples loading what
/// the first left) draws the second single sampled too, and the twins it made count on the ledger
/// from then: the loading passes' twins are kept, in memory of their own. Then the same target as
/// one pass with two views (`Pass::view_rects`), its twins transient (resolved, never stored): each
/// half shows its own view's block of the pass uniforms, resolved from four samples, nothing of a
/// view lands in the other half, and a frame drawn again reads the same (nothing rests on samples
/// stored before).
#[test]
fn an_msaa_pass_resolves_four_samples_and_a_loading_pass_or_a_second_view_continues_it() {
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
    // the attachments' memory on the ledger: their own, then with the twins they keep
    let memory = || hal.with_world(|wd| [TEX, DEPTH].map(|id| wd.session(sid).unwrap().ledger.get(ledger::Kind::Texture, id.index).map_or(0, |e| e.bytes)));
    let own = memory();
    // the eyes' passes at these samples, the first clearing, the second loading
    let frame = |w: &mut CmdWriter, n: u64, eyes: &[u32]| {
        w.clear();
        w.frame_begin(n, FrameMeta::default());
        for (eye, &samples) in eyes.iter().enumerate() {
            let load = if eye == 0 { pass_load::CLEAR } else { pass_load::LOAD };
            w.pass(Pass { color: [TEX, Id::NONE, Id::NONE, Id::NONE], depth: DEPTH, load: [load; 5], clear: [[0.0, 0.0, 0.0, 1.0]; 4], clear_depth: 1.0, viewport: [8.0 * eye as f32, 0.0, 8.0, 8.0], dpi: 1.0, samples, ..Default::default() });
            let rect = [0.0f32, 0.0, 1.0, 1.0];
            let inst = Inst::inline(w.keep(&rect[..]), 1);
            w.draw(Draw { pipeline: PIPE, vb: TRI, ib: INDEX, index: [0, 3], inst, scissor: [f32::NAN; 4], bounds: [f32::NAN; 4], ..Default::default() });
        }
        w.frame_end(TEX, target::TEXTURE);
    };
    // the first frame, one eye, asks for the four-sample state (drawn single sampled meanwhile);
    // once it is ready, the frame it became ready between the eyes in: the first eye as it was
    // drawn before (single sampled), the second at four samples; then both eyes with it
    frame(&mut w, 1, &[4]);
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
    for (n, eyes) in [(2, &[1u32, 4]), (3, &[4u32, 4])] {
        frame(&mut w, n, eyes);
        hal.submit(&w.buf());
        until(&mut |e| matches!(e, HalEvent::FrameDone { frame, .. } if frame == n));
    }
    w.clear();
    w.texture_readback(TEX, 1, [0, 0, 16, 8]);
    hal.submit(&w.buf());
    let mut red = Vec::new();
    until(&mut |e| match e {
        HalEvent::Readback { req: 1, status, lease, .. } => {
            // SAFETY: this process pumped the event and takes its lease once.
            let pixels = unsafe { makepad_hal::lease::Held::from_payload(lease) }.unwrap();
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
    let kept = memory();
    assert!(kept[0] > own[0] && kept[1] > own[1], "the loading passes' twins are kept, in memory of their own: {own:?} -> {kept:?}");

    // one pass, two views side by side: the left view's block red, the right's green; the
    // triangle starts half a view left of its view, so the right view's reaches into the left half
    const VIEW_PIPE: Id = Id::new(3, 1);
    w.clear();
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, instance_stride: 4, ..Default::default() }, pass_color_msl().as_bytes(), &[none, 2, none, none, none, 0], &[4]);
    w.pipeline_create(VIEW_PIPE, dialect::MSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&mut |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == VIEW_PIPE));
    let view_frame = |w: &mut CmdWriter, n: u64| {
        w.clear();
        w.frame_begin(n, FrameMeta::default());
        let uniforms = w.keep(&[1.0f32, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0][..]);
        let view_rects = w.put_rects(&[[0.0, 0.0, 8.0, 8.0], [8.0, 0.0, 8.0, 8.0]]);
        w.pass(Pass { color: [TEX, Id::NONE, Id::NONE, Id::NONE], depth: DEPTH, load: [pass_load::CLEAR; 5], clear: [[0.0, 0.0, 0.0, 1.0]; 4], clear_depth: 1.0, viewport: [0.0, 0.0, 16.0, 8.0], dpi: 1.0, samples: 4, uniforms, view_rects, ..Default::default() });
        let rect = [-0.5f32, 0.0, 1.5, 1.0];
        let inst = Inst::inline(w.keep(&rect[..]), 1);
        w.draw(Draw { pipeline: VIEW_PIPE, vb: TRI, ib: INDEX, index: [0, 3], inst, scissor: [f32::NAN; 4], bounds: [f32::NAN; 4], ..Default::default() });
        w.frame_end(TEX, target::TEXTURE);
    };
    view_frame(&mut w, 4);
    hal.submit(&w.buf());
    let (mut done, mut variant) = (false, false);
    until(&mut |e| {
        match e {
            HalEvent::FrameDone { frame: 4, .. } => done = true,
            HalEvent::PipelineReady { pipeline } if pipeline == VIEW_PIPE => variant = true,
            _ => {}
        }
        done && variant
    });
    let mut reads: Vec<Vec<[u8; 4]>> = Vec::new();
    for n in [5, 6] {
        view_frame(&mut w, n);
        hal.submit(&w.buf());
        until(&mut |e| matches!(e, HalEvent::FrameDone { frame, .. } if frame == n));
        w.clear();
        w.texture_readback(TEX, 2, [0, 0, 16, 8]);
        hal.submit(&w.buf());
        until(&mut |e| match e {
            HalEvent::Readback { req: 2, status, lease, .. } => {
                // SAFETY: this process pumped the event and takes its lease once.
                let pixels = unsafe { makepad_hal::lease::Held::from_payload(lease) }.unwrap();
                assert_eq!(status, readback::OK);
                reads.push(pixels.chunks(4).map(|p| [p[0], p[1], p[2], p[3]]).collect());
                true
            }
            _ => false,
        });
    }
    let px = &reads[0];
    assert_eq!(px.len(), 16 * 8);
    for (k, p) in px.iter().enumerate() {
        let (x, y) = (k % 16, k / 16);
        let (own, other) = if x < 8 { (p[0], p[1]) } else { (p[1], p[0]) };
        assert_eq!(other, 0, "pixel ({x}, {y}) {p:?}: the other view's colour landed in this half");
        let q = own as f32 * 4.0 / 255.0;
        assert!((q - q.round()).abs() * 255.0 / 4.0 <= 1.5, "pixel ({x}, {y}) {p:?}: not a quarter's step of coverage, so not 4 samples a pixel");
    }
    for (view, (half, channel)) in [("the left", 0), ("the right", 1)].into_iter().enumerate() {
        let at = |x: usize, y: usize| px[y * 16 + view * 8 + x][channel];
        let edge = (0..8).flat_map(|y| (0..8).map(move |x| (x, y))).filter(|&(x, y)| at(x, y) != 0 && at(x, y) != 255).count();
        assert!(at(0, 0) == 255 && at(7, 7) == 0, "{half} view's half holds its triangle in its colour (corner {} and {}): {px:?}", at(0, 0), at(7, 7));
        assert!(edge >= 4, "{half} view's edge is resolved from its samples: {edge} pixels part covered, {px:?}");
    }
    assert!(reads[0] == reads[1], "the frame drawn again reads the same: nothing rests on samples stored before");
    assert_eq!(hal.with_world(|wd| wd.session(sid).unwrap().stats.skipped_draws), 0, "no draw was left out");
    assert_eq!(hal.with_world(|wd| wd.session(sid).unwrap().stats.bad_args), 0, "the views were taken");
    assert!(hal.close(sid));
}

/// A transient MSAA pass (its twins resolved, never stored) with more geometry than the GPU bins
/// at once: one instanced draw of six million small triangles, 48 bytes of varyings a vertex,
/// into a 4x target with depth. A tile GPU spills the pass's samples mid-pass then, which
/// memoryless twins cannot take: Metal failed the command buffer and its frame
/// (`MTLCommandBufferError` 10, "too much geometry to support memoryless render pass
/// attachments"). The frame completes, none fails, the device stays, and its resolve equals the
/// same frame's with kept twins (a later MSAA pass loads the target), byte for byte.
#[test]
fn a_transient_msaa_pass_with_more_geometry_than_the_gpu_bins_resolves_as_a_kept_one() {
    const TEX: Id = Id::new(1, 1);
    const DEPTH: Id = Id::new(2, 1);
    const TRI: Id = Id::new(1, 1);
    const INDEX: Id = Id::new(2, 1);
    const INST: Id = Id::new(3, 1);
    const PIPE: Id = Id::new(1, 1);
    const TRIANGLES: u32 = 6_000_000;
    const SIZE: u32 = 256;
    // each instance's triangle: where it sits and its colour from hashes of its index, the
    // colour carried in three vectors of their own the fragment reads whole
    const MSL: &str = "#include <metal_stdlib>
using namespace metal;
struct V { float4 pos [[position]]; float4 a; float4 b; float4 c; };
uint mix_bits(uint h) { h *= 2654435761u; h ^= h >> 15; h *= 2246822519u; h ^= h >> 13; return h; }
float4 bytes(uint h) { return float4(float(h & 0xffu), float((h >> 8) & 0xffu), float((h >> 16) & 0xffu), float(h >> 24)) / 255.0; }
vertex V vertex_main(uint vid [[vertex_id]], uint iid [[instance_id]], constant float2 *geom [[buffer(0)]], device const float *inst [[buffer(1)]]) {
    uint h = mix_bits(iid + 1u);
    float2 p = float2(float(h & 0xffffu), float(h >> 16)) / 65536.0 + geom[vid] * (3.0 / 256.0);
    V o;
    o.pos = float4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, inst[iid], 1.0);
    o.a = bytes(mix_bits(h));
    o.b = bytes(mix_bits(h ^ 0x9e3779b9u));
    o.c = bytes(mix_bits(h ^ 0x85ebca6bu));
    return o;
}
fragment float4 fragment_main(V i [[stage_in]]) { return float4((i.a.xyz * i.a.w + i.b.xyz * i.b.w + i.c.xyz * i.c.w) / 3.0 + 0.25, 1.0); }
";
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::METAL, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
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
                match e {
                    HalEvent::PipelineFailed { log, .. } => panic!("pipeline: {}", String::from_utf8_lossy(log)),
                    HalEvent::DeviceLost { .. } => panic!("the device was taken for lost"),
                    _ => {}
                }
                if accept(e) {
                    return;
                }
            }
        }
    };
    let mut w = CmdWriter::new(sid);
    w.texture_create(TEX, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, SIZE, SIZE, [1, 1], Attr::default());
    w.texture_create(DEPTH, usage::RENDER_TARGET, format::DEPTH32F, SIZE, SIZE, [1, 1], Attr::default());
    let tri: Vec<u8> = [0.0f32, 0.0, 1.0, 0.0, 0.0, 1.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    w.buffer_create(TRI, buffer_kind::VERTEX, tri.len() as u64, Attr::default());
    w.buffer_update(TRI, 0, &tri);
    let index: Vec<u8> = [0u16, 1, 2].iter().flat_map(|i| i.to_le_bytes()).collect();
    w.buffer_create(INDEX, buffer_kind::INDEX, index.len() as u64, Attr::default());
    w.buffer_update(INDEX, 0, &index);
    // one float an instance (its depth: 0)
    w.buffer_create(INST, buffer_kind::INSTANCE, TRIANGLES as u64 * 4, Attr::default());
    w.buffer_update(INST, 0, &vec![0u8; TRIANGLES as usize * 4]);
    let none = pipeline_binding::NONE;
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, instance_stride: 1, ..Default::default() }, MSL.as_bytes(), &[none, none, none, none, none, 0], &[1]);
    w.pipeline_create(PIPE, dialect::MSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&mut |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == PIPE));
    // `kept`: a second MSAA pass loads the target, so the first one's twins are kept (stored)
    let frame = |w: &mut CmdWriter, n: u64, kept: bool| {
        w.clear();
        w.frame_begin(n, FrameMeta::default());
        let pass = Pass { color: [TEX, Id::NONE, Id::NONE, Id::NONE], depth: DEPTH, load: [pass_load::CLEAR; 5], clear: [[0.0, 0.0, 0.0, 1.0]; 4], clear_depth: 1.0, viewport: [0.0, 0.0, SIZE as f32, SIZE as f32], dpi: 1.0, samples: 4, ..Default::default() };
        w.pass(pass);
        w.draw(Draw { pipeline: PIPE, vb: TRI, ib: INDEX, index: [0, 3], inst: Inst::buffer(INST, 0, TRIANGLES), scissor: [f32::NAN; 4], bounds: [f32::NAN; 4], ..Default::default() });
        if kept {
            w.pass(Pass { load: [pass_load::LOAD; 5], ..pass });
        }
        w.frame_end(TEX, target::TEXTURE);
    };
    // the first frame asks for the four-sample state (drawn single sampled meanwhile)
    frame(&mut w, 1, false);
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
    let mut reads: Vec<Vec<u8>> = Vec::new();
    for (n, kept) in [(2, false), (3, true)] {
        frame(&mut w, n, kept);
        hal.submit(&w.buf());
        until(&mut |e| matches!(e, HalEvent::FrameDone { frame, .. } if frame == n));
        w.clear();
        w.texture_readback(TEX, n as u32, [0, 0, SIZE, SIZE]);
        hal.submit(&w.buf());
        until(&mut |e| match e {
            HalEvent::Readback { req, status, lease, .. } if req == n as u32 => {
                // SAFETY: this process pumped the event and takes its lease once.
                let pixels = unsafe { makepad_hal::lease::Held::from_payload(lease) }.unwrap();
                assert_eq!(status, readback::OK);
                reads.push(pixels.to_vec());
                true
            }
            _ => false,
        });
    }
    let stats = hal.with_world(|wd| wd.session(sid).unwrap().stats);
    assert_eq!(stats.failed_frames, 0, "no frame failed on the GPU (a memoryless twin fails the transient pass)");
    assert_eq!(stats.skipped_draws, 0, "no draw was left out");
    let drawn = reads[0].chunks(4).filter(|p| p[..3] != [0, 0, 0]).count();
    assert!(drawn * 10 >= (SIZE * SIZE * 9) as usize, "the triangles cover the target: {drawn} of {} pixels drawn", SIZE * SIZE);
    let differ = reads[0].chunks(4).zip(reads[1].chunks(4)).filter(|(a, b)| a != b).count();
    assert_eq!(differ, 0, "the transient pass resolves as the kept one does: {differ} pixels differ");
    assert!(hal.close(sid));
}

const RESET_TEX: Id = Id::new(1, 1);
const RESET_QUAD: Id = Id::new(1, 1);
const RESET_INDEX: Id = Id::new(2, 1);
const RESET_PIPE: Id = Id::new(1, 1);
const RESET_WIN: Id = Id::new(1, 1);

/// Pumps the session until `accept` takes an event (a failed pipeline fails the test).
fn pump_until(hal: &Hal, store: &mut EventStore, mut accept: impl FnMut(HalEvent<'_>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(60);
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
}

/// The quad over the left half of the 32x32 texture, or of the window, read back (`req`): frames
/// repeat until the draw was drawn (the window's pipeline variant is made when it is first asked).
fn reset_picture(hal: &Hal, store: &mut EventStore, sid: u32, frame: &mut u64, window: bool, req: u32) -> Vec<u8> {
    loop {
        *frame += 1;
        let n = *frame;
        assert!(n < 200, "the draw stayed skipped");
        let before = hal.with_world(|wd| wd.session(sid).unwrap().stats.skipped_draws);
        let mut w = CmdWriter::new(sid);
        w.frame_begin(n, FrameMeta::default());
        let color = if window { Id::NONE } else { RESET_TEX };
        w.pass(Pass { color: [color, Id::NONE, Id::NONE, Id::NONE], depth: Id::NONE, load: [pass_load::CLEAR; 5], clear: [[0.0, 0.0, 0.0, 1.0]; 4], viewport: [0.0, 0.0, 32.0, 32.0], dpi: 1.0, ..Default::default() });
        let rect = [0.0f32, 0.0, 0.5, 1.0];
        let inst = Inst::inline(w.keep(&rect[..]), 1);
        w.draw(Draw { pipeline: RESET_PIPE, vb: RESET_QUAD, ib: RESET_INDEX, index: [0, 6], inst, scissor: [f32::NAN; 4], bounds: [f32::NAN; 4], ..Default::default() });
        if window { w.frame_end(RESET_WIN, target::WINDOW) } else { w.frame_end(RESET_TEX, target::TEXTURE) }
        hal.submit(&w.buf());
        pump_until(hal, store, |e| matches!(e, HalEvent::FrameDone { frame, .. } if frame == n));
        if hal.with_world(|wd| wd.session(sid).unwrap().stats.skipped_draws) != before {
            continue;
        }
        w.clear();
        if window { w.snapshot(RESET_WIN, req) } else { w.texture_readback(RESET_TEX, req, [0, 0, 32, 32]) }
        hal.submit(&w.buf());
        let mut pixels = Vec::new();
        pump_until(hal, store, |e| match e {
            HalEvent::Readback { req: got, status, lease, .. } if got == req => {
                // SAFETY: this process pumped the event and takes its lease once.
                let held = unsafe { makepad_hal::lease::Held::from_payload(lease) }.unwrap();
                assert_eq!(status, readback::OK);
                pixels = held.to_vec();
                true
            }
            _ => false,
        });
        return pixels;
    }
}

fn reset_ledger(hal: &Hal, sid: u32) -> Vec<(makepad_hal::ledger::Kind, u32, u32, u64)> {
    hal.with_world(|w| {
        let mut rows: Vec<_> = w.session(sid).unwrap().ledger.entries().filter(|r| r.kind != makepad_hal::ledger::Kind::Window)
            .map(|r| (r.kind, r.index, r.gen, r.bytes)).collect();
        rows.sort();
        rows
    })
}

/// A device loss (simulated: the backend makes a new device, as when the system removed the GPU
/// it drew with) keeps every id: the session hears `DeviceLost { 1 }` once, then
/// `DeviceReset { 2 }`, the pipeline answers ready again (compiled on the new device), the
/// buffers take their contents again under the same ids and the same frames render the same
/// pixels into a texture and a window; the ledger is unchanged and nothing counts as stale or bad.
#[test]
fn device_reset_keeps_ids_and_renders_again() {
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::METAL, flags: open_flags::OFFSCREEN_WINDOWS, dpi_hint: 1.0, ..Default::default() });
    assert_eq!(info.backend, backend_id::METAL, "Metal HAL initialization failed");
    assert_ne!(info.flags & info_flags::DEVICE_RESET, 0, "the Metal backend says it replaces a lost device");
    let sid = info.session;
    let mut store = EventStore::new(sid, 64, 1 << 16);
    let quad: Vec<u8> = [0.0f32, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    let index: Vec<u8> = [0u16, 1, 2, 0, 2, 3].iter().flat_map(|i| i.to_le_bytes()).collect();
    let mut w = CmdWriter::new(sid);
    w.window_create(RESET_WIN, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 32.0, 32.0], false);
    w.texture_create(RESET_TEX, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 32, 32, [1, 1], Attr::default());
    w.buffer_create(RESET_QUAD, buffer_kind::VERTEX, quad.len() as u64, Attr::default());
    w.buffer_update(RESET_QUAD, 0, &quad);
    w.buffer_create(RESET_INDEX, buffer_kind::INDEX, index.len() as u64, Attr::default());
    w.buffer_update(RESET_INDEX, 0, &index);
    let none = pipeline_binding::NONE;
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, instance_stride: 4, ..Default::default() }, solid_msl("1.0, 0.0, 0.0, 1.0").as_bytes(), &[none, none, none, none, none, 0], &[4]);
    w.pipeline_create(RESET_PIPE, dialect::MSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    pump_until(&hal, &mut store, |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == RESET_PIPE));
    let mut frame = 0;
    let texture_before = reset_picture(&hal, &mut store, sid, &mut frame, false, 1);
    let window_before = reset_picture(&hal, &mut store, sid, &mut frame, true, 2);
    let at = |x: usize| (16 * 32 + x) * 4;
    assert_eq!((&texture_before[at(4)..][..4], &texture_before[at(28)..][..4]), (&[255, 0, 0, 255][..], &[0, 0, 0, 255][..]), "the quad covers the texture's left half");
    assert_eq!((&window_before[at(4)..][..4], &window_before[at(28)..][..4]), (&[255, 0, 0, 255][..], &[0, 0, 0, 255][..]), "the quad covers the window's left half");
    let rows = reset_ledger(&hal, sid);

    hal.simulate_device_loss();
    let (mut lost, mut reset, mut ready) = (Vec::new(), None, false);
    pump_until(&hal, &mut store, |e| {
        match e {
            HalEvent::DeviceLost { generation, .. } => {
                assert!(reset.is_none(), "DeviceLost after DeviceReset");
                lost.push(generation);
            }
            HalEvent::DeviceReset { generation } => {
                assert!(!lost.is_empty(), "DeviceReset without DeviceLost");
                reset = Some(generation);
            }
            HalEvent::PipelineReady { pipeline } if reset.is_some() && pipeline == RESET_PIPE => ready = true,
            _ => {}
        }
        reset.is_some() && ready
    });
    assert_eq!((lost, reset), (vec![1], Some(2)), "DeviceLost once, then DeviceReset");
    assert_eq!(hal.with_world(|w| w.device_generation), 2);

    // contents are the app's to send again: the same ids, no creates
    w.clear();
    w.buffer_update(RESET_QUAD, 0, &quad);
    w.buffer_update(RESET_INDEX, 0, &index);
    hal.submit(&w.buf());
    assert!(reset_picture(&hal, &mut store, sid, &mut frame, false, 3) == texture_before, "the texture renders the same pixels on the new device");
    assert!(reset_picture(&hal, &mut store, sid, &mut frame, true, 4) == window_before, "the window renders the same pixels on the new device");
    assert_eq!(reset_ledger(&hal, sid), rows);
    let stats = hal.with_world(|w| w.session(sid).unwrap().stats);
    assert_eq!((stats.bad_args, stats.stale_ids), (0, 0));
    assert!(hal.close(sid));
}
