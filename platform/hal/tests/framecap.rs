//! Frame capture on a real device: a frame recorded from one HAL replays in a private HAL to the
//! same pixels, and the per-draw cut gives each draw's own pixels. Metal (offscreen windows: no
//! window shows).
#![cfg(target_vendor = "apple")]

use makepad_hal::framecap::replay::{Cut, Opts, Replayer};
use makepad_hal::framecap::{self, file};
use makepad_hal::*;
use std::time::{Duration, Instant};

const WIN: Id = Id::new(1, 1);
const TEX: Id = Id::new(1, 1);
const QUAD: Id = Id::new(1, 1);
const INDEX: Id = Id::new(2, 1);
const SOLID: Id = Id::new(1, 1);
const SAMPLED: Id = Id::new(2, 1);
const W: u32 = 64;
const H: u32 = 48;

const MSL_SOLID: &str = r#"
#include <metal_stdlib>
using namespace metal;
struct V { float4 pos [[position]]; float4 color; };
vertex V vertex_main(uint vid [[vertex_id]], uint iid [[instance_id]], constant float2 *geom [[buffer(0)]], constant float4 *inst [[buffer(1)]]) {
    float4 r = inst[iid * 2];
    V o;
    float2 p = r.xy + geom[vid] * r.zw;
    o.pos = float4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    o.color = inst[iid * 2 + 1];
    return o;
}
fragment float4 fragment_main(V i [[stage_in]]) { return i.color; }
"#;

const MSL_SAMPLED: &str = r#"
#include <metal_stdlib>
using namespace metal;
struct V { float4 pos [[position]]; float2 uv; };
vertex V vertex_main(uint vid [[vertex_id]], uint iid [[instance_id]], constant float2 *geom [[buffer(0)]], constant float4 *inst [[buffer(1)]]) {
    float4 r = inst[iid * 2];
    V o;
    float2 p = r.xy + geom[vid] * r.zw;
    o.pos = float4(p.x * 2.0 - 1.0, 1.0 - p.y * 2.0, 0.0, 1.0);
    o.uv = geom[vid];
    return o;
}
fragment float4 fragment_main(V i [[stage_in]], texture2d<float> t [[texture(0)]]) {
    constexpr sampler s(filter::nearest);
    return t.sample(s, i.uv);
}
"#;

fn until(hal: &Hal, _sid: u32, store: &mut EventStore, mut accept: impl FnMut(HalEvent<'_>) -> bool) {
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

/// The instance data of the three draws (red quarter, textured right half, green middle): it is
/// the writer's borrowed memory until the submit, so it lives in a static.
static INSTANCES: [[f32; 8]; 3] = [
    [0.0, 0.0, 0.5, 0.5, 1.0, 0.0, 0.0, 1.0],
    [0.5, 0.0, 0.5, 1.0, 0.0, 0.0, 0.0, 0.0],
    [0.25, 0.25, 0.5, 0.5, 0.0, 1.0, 0.0, 1.0],
];

fn frame(w: &mut CmdWriter, n: u64) {
    w.clear();
    w.frame_begin(n, FrameMeta::default());
    w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, load: [pass_load::CLEAR; 5], clear: [[0.0, 0.0, 0.0, 1.0]; 4], viewport: [0.0, 0.0, W as f32, H as f32], dpi: 1.0, ..Default::default() });
    let base = Draw { vb: QUAD, ib: INDEX, index: [0, 6], scissor: [f32::NAN; 4], bounds: [f32::NAN; 4], ..Default::default() };
    let ids = w.put_ids(&[TEX]);
    for (i, (pipe, tex)) in [(SOLID, Range::default()), (SAMPLED, ids), (SOLID, Range::default())].into_iter().enumerate() {
        w.draw(Draw { pipeline: pipe, inst: Inst::inline(Slice::of(&INSTANCES[i][..]), 1), tex, ..base });
    }
    w.frame_end(WIN, target::WINDOW);
}

#[test]
fn a_captured_frame_replays_to_the_same_pixels() {
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::METAL, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    if info.backend != backend_id::METAL {
        eprintln!("SKIP: no Metal device");
        return;
    }
    let sid = info.session;
    let mut store = EventStore::new(sid, 128, 1 << 22);
    let mut w = CmdWriter::new(sid);
    w.window_create(WIN, window_kind::NORMAL, Id::NONE, [0.0, 0.0, W as f32, H as f32], false);
    let texels: Vec<u8> = (0..16u32).flat_map(|i| [(i * 16) as u8, 0, 255 - (i * 16) as u8, 255]).collect();
    w.texture_create(TEX, usage::SAMPLED, format::RGBA8, 4, 4, [1, 1], Attr::default());
    w.texture_update(TEX, 0, [0, 0, 4, 4], 0, &texels);
    let quad: Vec<u8> = [0.0f32, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    w.buffer_create(QUAD, buffer_kind::VERTEX, quad.len() as u64, Attr::default());
    w.buffer_update(QUAD, 0, &quad);
    let idx: Vec<u8> = [0u16, 1, 2, 0, 2, 3].iter().flat_map(|i| i.to_le_bytes()).collect();
    w.buffer_create(INDEX, buffer_kind::INDEX, idx.len() as u64, Attr::default());
    w.buffer_update(INDEX, 0, &idx);
    let none = pipeline_binding::NONE;
    for (id, code, ntex) in [(SOLID, MSL_SOLID, 0u32), (SAMPLED, MSL_SAMPLED, 1)] {
        let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, instance_stride: 8, color_formats: [format::BGRA8, 0, 0, 0], ..Default::default() }, code.as_bytes(), &[none, none, none, none, none, ntex], &[8]);
        w.pipeline_create(id, dialect::MSL, Attr::default(), &blob);
    }
    hal.submit(&w.buf());
    let mut ready = 0;
    until(&hal, sid, &mut store, |e| {
        ready += matches!(e, HalEvent::PipelineReady { .. }) as u32;
        ready == 2
    });
    // frames until every draw is drawn (pipeline variants for the window's layout are built lazily)
    let mut n = 1;
    loop {
        let before = hal.with_world(|wd| wd.session(sid).unwrap().stats.skipped_draws);
        frame(&mut w, n);
        hal.submit(&w.buf());
        let mut partial = false;
        until(&hal, sid, &mut store, |e| match e {
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
    // record the next frame
    let seq = framecap::arm(Some(sid), None);
    frame(&mut w, n);
    w.snapshot(WIN, 99);
    hal.submit(&w.buf());
    let mut live = Vec::new();
    until(&hal, sid, &mut store, |e| match e {
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
    assert_eq!((cap.passes.len(), cap.draws.len(), cap.pipelines.len(), cap.textures.len(), cap.buffers.len()), (1, 3, 2, 1, 2));
    assert_eq!(cap.textures[0].contents.as_deref(), Some(&texels[..]), "texture contents as uploaded");
    assert_eq!(cap.size, [W, H]);

    // the file round trip, then the replay in a private HAL
    let cap = file::decode(&file::encode(&cap)).unwrap();
    let mut r = Replayer::new(&cap).unwrap();
    assert!(r.failed.is_empty(), "{:?}", r.failed);
    r.run(&cap, Cut::all(&cap), Opts::default()).unwrap();
    let rt = r.target_of(&cap, 0);
    let img = r.read(&cap, rt, None).unwrap();
    assert_eq!((img.w, img.h), (W, H));
    assert_eq!(img.rgba, live, "the replay draws what the app drew");

    // cut after the first draw: only the red quarter
    r.run(&cap, Cut::after(&cap, 0), Opts::default()).unwrap();
    let first = r.read(&cap, rt, None).unwrap();
    assert_eq!(first.px(5, 5), [255, 0, 0, 255]);
    assert_eq!(first.px(40, 5), [0, 0, 0, 255], "the textured draw is not drawn yet");
    assert_eq!(first.px(20, 20), [255, 0, 0, 255], "the green draw is not drawn yet");
    // the whole frame: green over red in the middle, the texture on the right
    assert_eq!(img.px(20, 20), [0, 255, 0, 255]);
    assert_ne!(img.px(60, 40), [0, 0, 0, 255]);
    // the answers an inspector gets, from the file
    let path = std::env::temp_dir().join(format!("framecap-test-{}.mkcap", std::process::id()));
    std::fs::write(&path, file::encode(&cap)).unwrap();
    let ask = |q: &str| {
        fn d(req: &makepad_remote::Req, out: &mut makepad_remote::Out) {
            framecap::dispatch(req, out);
        }
        let (bytes, _) = makepad_remote::cmd::answer(d, &format!("{q} file={}", path.display()));
        String::from_utf8(bytes).unwrap()
    };
    let frame = ask("cap frame");
    assert!(frame.contains("1 passes, 3 draws"), "{frame}");
    for (draw, px) in [(0, 768), (1, 1536), (2, 768)] {
        let a = ask(&format!("cap draw D{draw}"));
        assert!(a.contains(&format!("changed {px} px")), "{a}");
    }
    let pixel = ask("cap pixel 20,20");
    assert!(pixel.contains("2 draws changed it; the last is D2"), "{pixel}");
    let od = ask("cap overdraw");
    assert!(od.contains("peak 2"), "{od}");
    let shown = ask("cap rt D1 wire=1");
    assert!(shown.contains("png: "), "{shown}");
    let _ = std::fs::remove_file(&path);
    // a frame of two packets: a texture pass (an offscreen view) then the window that samples it
    const RT: Id = Id::new(2, 1);
    w.clear();
    w.texture_create(RT, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 32, 32, [1, 1], Attr::default());
    hal.submit(&w.buf());
    let two = |w: &mut CmdWriter, n: u64| {
        w.clear();
        w.frame_begin(n, FrameMeta::default());
        w.pass(Pass { color: [RT, Id::NONE, Id::NONE, Id::NONE], depth: Id::NONE, load: [pass_load::CLEAR; 5], clear: [[0.0, 0.0, 1.0, 1.0]; 4], viewport: [0.0, 0.0, 32.0, 32.0], dpi: 1.0, ..Default::default() });
        let base = Draw { vb: QUAD, ib: INDEX, index: [0, 6], scissor: [f32::NAN; 4], bounds: [f32::NAN; 4], ..Default::default() };
        w.draw(Draw { pipeline: SOLID, inst: Inst::inline(Slice::of(&INSTANCES[0][..]), 1), ..base });
        w.frame_end(RT, target::TEXTURE);
        w.frame_begin(n + 1, FrameMeta::default());
        w.pass(Pass { color: [Id::NONE; 4], depth: Id::NONE, load: [pass_load::CLEAR; 5], clear: [[0.0, 0.0, 0.0, 1.0]; 4], viewport: [0.0, 0.0, W as f32, H as f32], dpi: 1.0, ..Default::default() });
        let ids = w.put_ids(&[RT]);
        w.draw(Draw { pipeline: SAMPLED, inst: Inst::inline(Slice::of(&INSTANCES[1][..]), 1), tex: ids, ..base });
        w.frame_end(WIN, target::WINDOW);
    };
    let mut n = n + 10;
    for _ in 0..30 {
        two(&mut w, n);
        hal.submit(&w.buf());
        until(&hal, sid, &mut store, |e| matches!(e, HalEvent::FrameDone { frame, .. } if frame == n + 1));
        n += 2;
    }
    let seq = framecap::arm(Some(sid), None);
    two(&mut w, n);
    w.snapshot(WIN, 98);
    hal.submit(&w.buf());
    let mut live2 = Vec::new();
    until(&hal, sid, &mut store, |e| match e {
        HalEvent::Readback { req: 98, pixels, .. } => {
            live2 = pixels.to_vec();
            true
        }
        _ => false,
    });
    let mut cap2 = (*framecap::wait(seq, Duration::from_secs(10)).expect("the two-packet capture")).clone();
    cap2.backend = info.backend;
    cap2.dialect = info.dialect;
    assert_eq!((cap2.passes.len(), cap2.draws.len()), (2, 2), "both packets recorded");
    assert!(cap2.texture(RT).unwrap().contents.is_none(), "the record clears the offscreen target itself");
    let mut r2 = Replayer::new(&cap2).unwrap();
    r2.run(&cap2, Cut::all(&cap2), Opts::default()).unwrap();
    let rt2 = r2.target_of(&cap2, 1);
    assert_eq!(r2.read(&cap2, rt2, None).unwrap().rgba, live2, "two packets replay as drawn");
    r2.run(&cap2, Cut { pass: 0, draws: 0 }, Opts::default()).unwrap();
    let off = r2.target_of(&cap2, 0);
    assert_eq!(r2.read(&cap2, off, Some([30, 30, 1, 1])).unwrap().px(0, 0), [0, 0, 255, 255], "the offscreen pass's clear");

    // skipping the last draw leaves the red
    r.run(&cap, Cut::all(&cap), Opts { skip: Some(2), ..Default::default() }).unwrap();
    assert_eq!(r.read(&cap, rt, Some([20, 20, 1, 1])).unwrap().px(0, 0), [255, 0, 0, 255]);
}
