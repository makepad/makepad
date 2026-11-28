//! Native D3D11 coverage (Windows, a hardware device). The same cases as tests/vulkan.rs: passes
//! and draws into a texture and an offscreen window, exact readback, the ledger of a closing
//! session, resize, and the pick variant's ids.
#![cfg(windows)]

use makepad_hal::hlsl::HlslPipeline;
use makepad_hal::ledger::Kind;
use makepad_hal::session::HOST;
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

/// The first vec4 of the pass's constant buffer (`pipeline_binding::PASS` at b1): a view of the
/// pass shows its own block.
const PASS_COLOR_HLSL: &str = "cbuffer passUniforms : register(b1) { float4 pass_color; };
struct VertexIn { float4 g : GEOMA0; };
float4 vertex_main(VertexIn i) : SV_POSITION { return i.g; }
float4 pixel_main() : SV_TARGET0 { return pass_color; }
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
    resources_with_geometry(hal, sid, &[-1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0], &[0, 1, 2]);
}

fn resources_with_geometry(hal: &Hal, sid: u32, vertices: &[f32], indices: &[u16]) {
    let mut w = CmdWriter::new(sid);
    w.texture_create(TEX, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 32, 32, [1, 1], Attr::default());
    let vertices: Vec<u8> = vertices.iter().flat_map(|v| v.to_le_bytes()).collect();
    let indices: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    w.buffer_create(VERTEX, buffer_kind::VERTEX, vertices.len() as u64, Attr::default());
    w.buffer_update(VERTEX, 0, &vertices);
    w.buffer_create(INDEX, buffer_kind::INDEX, indices.len() as u64, Attr::default());
    w.buffer_update(INDEX, 0, &indices);
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
    Draw { pipeline: PIPE, vb: VERTEX, ib: INDEX, index: [0, 3], inst: Inst::none(1), scissor: [f32::NAN; 4], ..Default::default() }
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
    readback(hal, sid, req).1
}

fn readback(hal: &Hal, sid: u32, req: u32) -> ([u32; 4], Vec<u8>) {
    let mut result = ([0; 4], Vec::new());
    until(hal, sid, |e| match e {
        HalEvent::Readback { req: got, status, rect, lease, .. } if got == req => {
            // SAFETY: this process pumped the event and takes its lease once.
            let p = unsafe { makepad_hal::lease::Held::from_payload(lease) }.unwrap();
            assert_eq!(status, readback::OK, "the requested image must be readable");
            result = (rect, p.to_vec());
            true
        }
        _ => false,
    });
    result
}

#[test]
fn cull_mode_front_and_back_pixel_guard() {
    let (hal, info) = open();
    let sid = info.session;
    // Two clip-space rows, bottom then top. Left indices wind CCW, right CW.
    resources_with_geometry(&hal, sid, &[
        -1.0, -1.0, 0.0, 1.0,  0.0, -1.0, 0.0, 1.0,  1.0, -1.0, 0.0, 1.0,
        -1.0,  1.0, 0.0, 1.0,  0.0,  1.0, 0.0, 1.0,  1.0,  1.0, 0.0, 1.0,
    ], &[0, 1, 4, 0, 4, 3, 1, 5, 2, 1, 4, 5]);
    let mut w = CmdWriter::new(sid);
    let pair = Draw { index: [0, 12], ..draw() };
    let clear = pass(TEX, 32, 32, [0.0, 0.0, 0.0, 1.0]);
    let finish_texture = |w: &mut CmdWriter, frame: u64| {
        w.frame_end(TEX, target::TEXTURE);
        hal.submit(&w.buf());
        frame_done(&hal, sid, frame);
        w.clear();
        w.texture_readback(TEX, frame as u32, [0, 0, 32, 32]);
        hal.submit(&w.buf());
        pixels(&hal, sid, frame as u32)
    };
    let stats = || hal.with_world(|world| world.session(sid).unwrap().stats);
    let halves = |rgba: &[u8], y: usize| -> [[u8; 4]; 2] {
        assert_eq!(rgba.len(), 32 * 32 * 4);
        [8, 24].map(|x| {
            let at = (y * 32 + x) * 4;
            rgba[at..at + 4].try_into().unwrap()
        })
    };
    let white = [255; 4];
    let black = [0, 0, 0, 255];
    let both = Draw::CULL | Draw::CULL_FRONT;
    for (frame, flags, expected) in [
        (1, 0, [white, white]),
        (2, Draw::CULL, [white, black]),
        (3, Draw::CULL_FRONT, [black, white]),
        (4, both, [black, black]),
    ] {
        let before = stats();
        w.clear();
        w.frame_begin(frame, FrameMeta::default());
        w.pass(clear);
        w.draw(Draw { flags, ..pair });
        let rgba = finish_texture(&mut w, frame);
        assert_eq!(halves(&rgba, 16), expected, "cull flags {flags:#x}");
        let after = stats();
        let invalid = u64::from(flags == both);
        assert_eq!(after.bad_args - before.bad_args, invalid, "bad arguments for {flags:#x}");
        assert_eq!(after.skipped_draws - before.skipped_draws, invalid, "skipped draws for {flags:#x}");
    }

    // Keep the first pass's strip. Each following strip isolates a change of cull mode.
    w.clear();
    w.frame_begin(5, FrameMeta::default());
    w.pass(clear);
    w.draw(Draw { flags: Draw::CULL_FRONT, scissor: [0.0, 0.0, 32.0, 8.0], ..pair });
    let mut load = clear;
    load.load[0] = pass_load::LOAD;
    w.pass(load);
    for (y, flags) in [(8.0, Draw::CULL), (16.0, Draw::CULL_FRONT), (24.0, 0)] {
        w.draw(Draw { flags, scissor: [0.0, y, 32.0, 8.0], ..pair });
    }
    let rgba = finish_texture(&mut w, 5);
    for (y, expected) in [(4, [black, white]), (12, [white, black]), (20, [black, white]), (28, [white, white])] {
        assert_eq!(halves(&rgba, y), expected, "cross-pass and in-pass cull transitions at y={y}");
    }
    assert!(hal.close(sid));
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
fn a_resized_window_layer_draws_past_the_apps_last_frame() {
    const APP_WINDOW: Id = Id::new(7, 1);
    let hal = Hal::new();
    let req = OpenReq { backend: backend_id::D3D11, flags: open_flags::OFFSCREEN_WINDOWS | open_flags::RETAIN_WINDOWS, ..Default::default() };
    assert_eq!(hal.open_host(&req).backend, backend_id::D3D11, "the host must use D3D11");
    let mut w = CmdWriter::new(HOST);
    w.window_create(WINDOW, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 32.0, 32.0], false);
    w.texture_create(TEX, usage::RENDER_TARGET | usage::SAMPLED, format::BGRA8, 32, 32, [1, 1], Attr::default());
    w.window_overlay(WINDOW, true, [0.0, 0.0, 32.0, 32.0], TEX, false);
    hal.submit(&w.buf());

    let sid = hal.open(&OpenReq { app_key: 99, ..req }).session;
    let mut app = CmdWriter::new(sid);
    app.window_create(APP_WINDOW, window_kind::NORMAL, Id::NONE, [0.0; 4], true);
    app.frame_begin(1, FrameMeta::default());
    app.pass(pass(Id::NONE, 32, 32, [1.0; 4]));
    app.frame_end(APP_WINDOW, target::WINDOW);
    hal.submit(&app.buf());
    frame_done(&hal, sid, 1);

    w.clear();
    w.window_geom(WINDOW, [0.0, 0.0, 48.0, 40.0]);
    let mut layer_frame = |serial: u32, load| {
        w.frame_begin(serial.into(), FrameMeta::default());
        let mut p = pass(Id::NONE, 48, 40, [0.0, 1.0, 0.0, 1.0]);
        p.load[0] = load;
        w.pass(p);
        w.frame_end(WINDOW, target::WINDOW);
        w.snapshot(WINDOW, serial);
        hal.submit(&w.buf());
        w.clear();
        readback(&hal, HOST, serial)
    };
    let (rect, resized) = layer_frame(2, pass_load::CLEAR);
    assert_eq!(rect, [0, 0, 48, 40], "the host snapshot follows the layer frame's new size");
    assert_eq!(&resized[(35 * 48 + 40) * 4..][..4], &[0, 255, 0, 255], "the host's green layer extends past the app's old edge");
    let (_, loaded) = layer_frame(3, pass_load::LOAD);
    assert_eq!(&loaded[(31 * 48 + 31) * 4..][..4], &[255; 4], "the copy includes the last pixel of the app's retained frame");
    assert_eq!(&loaded[(35 * 48 + 40) * 4..][..4], &[0; 4], "the uncovered area is cleared when the green composed image is reused");
    assert!(hal.close(sid), "the borrower's session must close");
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
    w.texture_readback(PICK_TEX, 7, [0, 0, 32, 32]);
    hal.submit(&w.buf());
    let pixels = pixels(&hal, sid, 7);
    let id_at = |x: usize, y: usize| {
        let p = &pixels[(y * 32 + x) * 4..];
        p[0] as u32 | (p[1] as u32) << 8 | (p[2] as u32) << 16
    };
    assert_eq!(id_at(16, 28), 0x12_3456, "the triangle's draw id");
    assert_eq!(id_at(1, 1), 0, "nothing drawn at the corner");
    assert!(hal.close(sid));
}

#[test]
fn volume_sampling_pipeline_answers_its_layout() {
    use makepad_hal::cmds::PipelineLayout;
    let (hal, info) = open();
    let sid = info.session;
    let code = HlslPipeline {
        source: "Texture3D<float4> image : register(t0);
            struct VertexIn { float4 g : GEOMA0; };
            float4 vertex_main(VertexIn i) : SV_POSITION { return i.g; }
            float4 pixel_main() : SV_TARGET0 { return image.Load(int4(0,0,0,0)); }".into(),
        textures: vec![TEXTURE_SHAPE_3D], samplers: Vec::new(),
    }.encode();
    let none = u32::MAX;
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() }, &code,
        &[none, none, none, none, none, 1], &[16, 0, 3, 4]);
    let mut w = CmdWriter::new(sid);
    w.pipeline_create(PIPE, dialect::HLSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    w.clear();
    w.pipeline_wanted_in(PIPE, 3, Id::NONE, [format::BGRA8, none, none, none], none, u16::from_le_bytes([1, 1]));
    hal.submit(&w.buf());
    if info.max_volume_dimension > 0 && info.volume_formats != 0 {
        let (mut base, mut layout) = (false, false);
        until(&hal, sid, |event| {
            match event {
                HalEvent::PipelineReady { pipeline: PIPE } => base = true,
                HalEvent::PipelineLayoutReady { pipeline: PIPE, layout: 3 } => layout = true,
                HalEvent::PipelineLayoutFailed { log, .. } => panic!("volume layout: {}", String::from_utf8_lossy(log)),
                _ => {}
            }
            base && layout
        });
    } else {
        let reason = b"volume textures aren't available on D3D11 yet";
        // Both submissions are answered synchronously, without a draw or compiler wakeup.
        let mut store = EventStore::new(sid, 64, 65536);
        let mut buf = store.buf();
        let n = hal.pump(0, &mut buf);
        let (mut base, mut creation, mut probe) = (false, false, false);
        for (_, event) in store.reader(n, buf.arena_len) {
            match event {
                HalEvent::PipelineFailed { pipeline: PIPE, log } => base = log == reason,
                HalEvent::PipelineLayoutFailed { pipeline: PIPE, layout: PipelineLayout::UNPROBED, log } => creation = log == reason,
                HalEvent::PipelineLayoutFailed { pipeline: PIPE, layout: 3, log } => probe = log == reason,
                HalEvent::PipelineReady { pipeline: PIPE } | HalEvent::PipelineLayoutReady { pipeline: PIPE, .. } =>
                    panic!("unsupported sampled volume was acknowledged"),
                _ => {}
            }
        }
        assert!(base && creation && probe, "creation and the later layout probe must all carry the capability reason");
    }
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
                HalEvent::DeviceLost { generation, .. } => {
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

/// An upload from a temporary (`&vec![..]`: freed at the end of its statement, its block handed
/// out again before the submit) lands the bytes of the call: the writer copied them at the call.
#[test]
fn uploads_from_freed_temporaries_land_the_bytes_of_the_call() {
    let (hal, info) = open();
    let sid = info.session;
    let (buffer, slab, texture) = (Id::new(9, 1), Id::new(10, 1), Id::new(9, 1));
    let want_buffer: Vec<u8> = (0..4096u32).map(|i| (i * 7 + 3) as u8).collect();
    let want_texels: Vec<u8> = (0..16 * 16 * 4u32).map(|i| (i * 13 + 5) as u8).collect();
    let mut w = CmdWriter::new(sid);
    w.buffer_create(buffer, buffer_kind::INSTANCE, 4096, Attr::default());
    w.buffer_create(slab, buffer_kind::INSTANCE | buffer_kind::APPEND, 4096, Attr::default());
    w.texture_create(texture, usage::SAMPLED | usage::RENDER_TARGET, format::RGBA8, 16, 16, [1, 1], Attr::default());
    w.buffer_update(buffer, 0, &want_buffer.clone());
    w.buffer_update_parts(slab, &[(0, &want_buffer[..1000]), (1000, &want_buffer[1000..2048]), (2048, &want_buffer[2048..])]);
    w.texture_update(texture, 0, [0, 0, 16, 16], 0, &want_texels.clone());
    let scribble = [vec![0xeeu8; 4096], vec![0xee; 1024]];
    let before = hal.with_world(|w| w.session(sid).unwrap().stats.driver_uploads);
    w.read_buffers(1, &[buffer, slab].map(|id| BufferReadRange { id, offset: 0, len: 4096 }));
    w.texture_readback(texture, 2, [0, 0, 16, 16]);
    hal.submit(&w.buf());
    drop(scribble);
    let mut got: [Vec<u8>; 3] = Default::default();
    until(&hal, sid, |e| {
        if let HalEvent::BufferLeases { req: 1, status, records } = e {
            use makepad_hal::{buffers::Wire, lease::{Held, Record}};
            assert_eq!(status, readback::OK);
            assert_eq!(records.len(), 2 * Record::SIZE, "one lease for each buffer");
            for (out, record) in got[..2].iter_mut().zip(records.chunks_exact(Record::SIZE)) {
                // SAFETY: this process pumped the event and takes each lease once.
                *out = unsafe { Held::from_payload(record) }.unwrap().to_vec();
            }
        }
        if let HalEvent::Readback { req: 2, status, lease, .. } = e {
            // SAFETY: this process pumped the event and takes its lease once.
            let pixels = unsafe { makepad_hal::lease::Held::from_payload(lease) }.unwrap();
            assert_eq!(status, readback::OK);
            got[2] = pixels.to_vec();
        }
        got.iter().all(|g| !g.is_empty())
    });
    assert!(got[0] == want_buffer, "the buffer holds other bytes than its upload's");
    assert!(got[1] == want_buffer, "the APPEND slab holds all three parts in order");
    let uploads = hal.with_world(|w| w.session(sid).unwrap().stats.driver_uploads) - before;
    // The shared-texture device uses the HAL's adapter. The bindings omit D3D11_OPTIONS,
    // whose tenth BOOL (of 14) is MapNoOverwriteOnDynamicBufferSRV.
    let device = shared_texture::device().unwrap();
    let mut options = [0i32; 14];
    let queried = unsafe { device.CheckFeatureSupport(makepad_windows_sys::Win32::Graphics::Direct3D11::D3D11_FEATURE(5),
        options.as_mut_ptr().cast(), std::mem::size_of_val(&options) as u32) };
    if queried.is_ok() && options[9] != 0 {
        assert_eq!(uploads, 2, "one UpdateSubresource for the plain buffer and one Map for the dynamic slab");
    } else {
        assert_eq!(uploads, 4, "one UpdateSubresource for the plain buffer and three for the default-usage slab");
    }
    assert!(got[2] == want_texels, "the texture holds other bytes than its upload's");
    assert_eq!(hal.with_world(|w| w.session(sid).unwrap().stats.bad_args), 0);
    assert!(hal.close(sid));
}

#[test]
fn uniform_stream_renders_pass_list_draw_and_short_block() {
    const MONO: Id = Id::new(5, 1);
    const STEREO: Id = Id::new(6, 1);
    const QUAD: Id = Id::new(5, 1);
    const INDICES: Id = Id::new(6, 1);
    const PIPELINE: Id = Id::new(5, 1);
    let (hal, info) = open();
    let sid = info.session;
    let mut w = CmdWriter::new(sid);
    w.set_uniform_align(info.effective_uniform_align());
    for target in [MONO, STEREO] {
        w.texture_create(target, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 16, 8, [1, 1], Attr::default());
    }
    // Extend past the left edge so each stereo view must clip to its own rectangle.
    let quad: Vec<u8> = [-3.0f32, -1.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0,
        1.0, 1.0, 0.0, 1.0, -3.0, 1.0, 0.0, 1.0].into_iter().flat_map(f32::to_le_bytes).collect();
    let indices: Vec<u8> = [0u16, 1, 2, 0, 2, 3].into_iter().flat_map(u16::to_le_bytes).collect();
    w.buffer_create(QUAD, buffer_kind::VERTEX, quad.len() as u64, Attr::default());
    w.buffer_update(QUAD, 0, &quad);
    w.buffer_create(INDICES, buffer_kind::INDEX, indices.len() as u64, Attr::default());
    w.buffer_update(INDICES, 0, &indices);
    let source = payload("cbuffer passUniforms : register(b1) { float4 pass_color; };
cbuffer listUniforms : register(b2) { float4 list_color; };
cbuffer callUniforms : register(b0) { float4 call_color; };
struct VertexIn { float4 g : GEOMA0; };
float4 vertex_main(VertexIn i) : SV_POSITION { return i.g; }
float4 pixel_main() : SV_TARGET0 { return pass_color + list_color + call_color; }");
    let none = pipeline_binding::NONE;
    let desc = PipelineDesc { outputs: 1, color_formats: [format::RGBA8, 0, 0, 0], ..Default::default() };
    let blob = CmdWriter::pipeline_blob(desc, &source, &[0, 1, 2, none, none, 0], &[16, 0, 3, 4]);
    w.pipeline_create(PIPELINE, dialect::HLSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, sid, |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == PIPELINE));
    let finish = |w: &mut CmdWriter, frame: u64, target: Id, expected: &[(Id, [u8; 4], [u8; 4])]| {
        w.frame_end(target, target::TEXTURE);
        hal.submit(&w.buf());
        frame_done(&hal, sid, frame);
        for &(target, left, right) in expected {
            w.clear();
            w.texture_readback(target, frame as u32, [0, 0, 16, 8]);
            hal.submit(&w.buf());
            let image = pixels(&hal, sid, frame as u32);
            assert_eq!(image.len(), 16 * 8 * 4);
            for (k, pixel) in image.chunks_exact(4).enumerate() {
                let want = if k % 16 < 8 { left } else { right };
                assert!(pixel.iter().zip(want).all(|(&a, b)| a.abs_diff(b) <= 1),
                    "frame {frame}, target {target:?}, pixel ({}, {}) was {pixel:?}, expected {want:?}", k % 16, k / 16);
            }
        }
    };
    w.clear();
    w.frame_begin(1, FrameMeta::default());
    let pass = Pass { viewport: [0.0, 0.0, 16.0, 8.0], dpi: 1.0, load: [pass_load::CLEAR; 5], ..Default::default() };
    let mono = Pass { color: [MONO, Id::NONE, Id::NONE, Id::NONE], ..pass };
    let stereo = Pass { color: [STEREO, Id::NONE, Id::NONE, Id::NONE], ..pass };
    let uniforms = w.put_uniform(&[0.0f32, 0.0, 0.0, 1.0]);
    w.pass(Pass { uniforms, ..mono });
    let list = w.put_uniform(&[0.0f32, 0.5]);
    let draw = Draw { pipeline: PIPELINE, vb: QUAD, ib: INDICES, index: [0, 6], inst: Inst::none(1),
        scissor: [f32::NAN; 4], ..Default::default() };
    for (x, red) in [(0.0, 0.25f32), (8.0, 0.75)] {
        let call = w.put_uniform(&[red, 0.0, 0.0, 0.0]);
        w.draw(Draw { scissor: [x, 0.0, 8.0, 8.0], uniforms: [call, list, Range::default(), Range::default()], ..draw });
    }

    let left = [0.0, 0.0, 0.0, 1.0].map(f32::to_ne_bytes);
    let right = [0.0, 0.0, 1.0, 1.0].map(f32::to_ne_bytes);
    let (uniforms, uniform_stride) = w.put_pass_views_uniforms(&[left.as_flattened(), right.as_flattened()]);
    let view_rects = w.put_rects(&[[0.0, 0.0, 8.0, 8.0], [8.0, 0.0, 8.0, 8.0]]);
    w.pass(Pass { uniforms, uniform_stride, view_rects, ..stereo });
    let call = w.put_uniform(&[0.25f32, 0.0, 0.0, 0.0]);
    w.draw(Draw { uniforms: [call, list, Range::default(), Range::default()], ..draw });
    finish(&mut w, 1, STEREO, &[
        (MONO, [64, 128, 0, 255], [191, 128, 0, 255]),
        (STEREO, [64, 128, 0, 255], [64, 128, 255, 255]),
    ]);
    let stats = || hal.with_world(|wd| wd.session(sid).unwrap().stats);
    let first = stats();
    assert_eq!((first.draws, first.skipped_draws, first.bad_args), (3, 0, 0));

    w.clear();
    w.frame_begin(2, FrameMeta::default());
    let uniforms = w.put_uniform(&[0.0f32, 0.0, 0.5, 1.0]);
    w.pass(Pass { uniforms, ..mono });
    let list = w.put_uniform(&[0.0f32, 0.5]);
    let call = w.put_uniform(&[0.75f32, 0.0, 0.0, 0.0]);
    let left_draw = Draw { scissor: [0.0, 0.0, 8.0, 8.0], ..draw };
    let right_draw = Draw { scissor: [8.0, 0.0, 8.0, 8.0], ..draw };
    let empty = Range::default();
    w.draw(Draw { uniforms: [call, list, empty, empty], ..left_draw });
    w.draw(Draw { uniforms: [empty; 4], ..right_draw });
    w.pass(stereo);
    let call = w.put_uniform(&[0.25f32, 0.0, 0.0, 0.0]);
    w.draw(Draw { uniforms: [call, empty, empty, empty], ..draw });
    finish(&mut w, 2, STEREO, &[
        (MONO, [191, 128, 128, 255], [0, 0, 128, 255]),
        (STEREO, [64, 0, 0, 0], [64, 0, 0, 0]),
    ]);
    let before = stats();
    assert_eq!((before.bad_args, before.skipped_draws), (0, 0));

    w.clear();
    w.frame_begin(3, FrameMeta::default());
    let uniforms = w.put_uniform(&[0.0f32, 0.0, 0.0, 1.0]);
    w.pass(Pass { uniforms, ..mono });
    let list = w.put_uniform(&[0.0f32, 0.5]);
    let call = w.put_uniform(&[0.5f32, 0.0, 0.0, 0.0]);
    w.draw(Draw { uniforms: [call, list, empty, empty], ..left_draw });
    for off in [4, 16 * w.uniform_align()] {
        w.draw(Draw { uniforms: [Range { off, len: 16 }, list, empty, empty], ..right_draw });
    }
    finish(&mut w, 3, MONO, &[(MONO, [128, 128, 0, 255], [0, 0, 0, 0])]);
    let after = stats();
    assert_eq!(after.bad_args - before.bad_args, 2, "rejected uniform arguments");
    assert_eq!(after.skipped_draws - before.skipped_draws, 2, "skipped invalid draws");
    assert!(hal.close(sid));
}

/// HDR addition includes alpha; a raw pipeline and a non-blending draw still replace texels.
#[test]
fn rgba16f_add_and_raw_writes() {
    let (hal, info) = open();
    let sid = info.session;
    const TARGET: Id = Id::new(30, 1);
    const VERTICES: Id = Id::new(30, 1);
    const INDICES: Id = Id::new(31, 1);
    const ADD: Id = Id::new(30, 1);
    const RAW: Id = Id::new(31, 1);
    let mut w = CmdWriter::new(sid);
    w.texture_create(TARGET, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA16F, 2, 2, [1, 1], Attr::default());
    let vertices: Vec<u8> = [-1.0f32, -1.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, -1.0, 1.0, 0.0, 1.0].into_iter().flat_map(f32::to_le_bytes).collect();
    let indices: Vec<u8> = [0u16, 1, 2, 0, 2, 3].into_iter().flat_map(u16::to_le_bytes).collect();
    w.buffer_create(VERTICES, buffer_kind::VERTEX, vertices.len() as u64, Attr::default());
    w.buffer_update(VERTICES, 0, &vertices);
    w.buffer_create(INDICES, buffer_kind::INDEX, indices.len() as u64, Attr::default());
    w.buffer_update(INDICES, 0, &indices);
    let source = payload(PASS_COLOR_HLSL);
    let none = pipeline_binding::NONE;
    for (id, blend) in [(ADD, pipeline_blend::ADD), (RAW, pipeline_blend::RAW)] {
        let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, color_formats: [format::RGBA16F, 0, 0, 0], blend, instance_stride: 0, ..Default::default() },
            &source, &[none, 1, none, none, none, 0], &[16, 0, 3, 4]);
        w.pipeline_create(id, dialect::HLSL, Attr::default(), &blob);
    }
    hal.submit(&w.buf());
    let mut ready = [false; 2];
    until(&hal, sid, |e| {
        if let HalEvent::PipelineReady { pipeline } = e {
            if pipeline == ADD { ready[0] = true; }
            if pipeline == RAW { ready[1] = true; }
        }
        ready.iter().all(|v| *v)
    });
    // Half encodings of (3.75, 0.75, 4.125, 2.0) and (2.25, 0.5, 0.125, 1.0).
    let sum = [0x4380u16, 0x3a00, 0x4420, 0x4000];
    let second = [0x4080u16, 0x3800, 0x3000, 0x3c00];
    for (case, (pipeline, flags, want)) in [(ADD, Draw::BLEND, sum), (RAW, Draw::BLEND, second), (ADD, 0, second)].into_iter().enumerate() {
        let frame = case as u64 + 1;
        w.clear();
        w.frame_begin(frame, FrameMeta::default());
        for (i, color) in [[1.5f32, 0.25, 4.0, 1.0], [2.25, 0.5, 0.125, 1.0]].iter().enumerate() {
            let mut pass = pass(TARGET, 2, 2, [0.0; 4]);
            if i != 0 { pass.load[0] = pass_load::LOAD; }
            pass.uniforms = w.put_uniform(color);
            w.pass(pass);
            let inst = Inst::none(1);
            w.draw(Draw { pipeline, vb: VERTICES, ib: INDICES, index: [0, 6], inst, flags,
                scissor: [f32::NAN; 4], bounds: [f32::NAN; 4], ..Default::default() });
        }
        w.frame_end(TARGET, target::TEXTURE);
        hal.submit(&w.buf());
        frame_done(&hal, sid, frame);
        w.clear();
        w.texture_readback(TARGET, frame as u32, [0, 0, 2, 2]);
        hal.submit(&w.buf());
        until(&hal, sid, |e| {
            let HalEvent::Readback { req, status, lease, .. } = e else { return false };
            if req != frame as u32 { return false; }
            // SAFETY: this process pumped the event and takes its lease once.
            let pixels = unsafe { makepad_hal::lease::Held::from_payload(lease) }.unwrap();
            assert_eq!(status, readback::NATIVE, "float readback preserves HDR values");
            assert_eq!(pixels.len(), 2 * 2 * 8);
            for (channel, bytes) in pixels.chunks_exact(2).enumerate() {
                let got = u16::from_le_bytes([bytes[0], bytes[1]]);
                assert!(got.abs_diff(want[channel % 4]) <= 1, "case {case}, channel {channel}: half {got:#06x}, expected {:#06x} within one half ULP", want[channel % 4]);
            }
            true
        });
    }
    let stats = hal.with_world(|world| world.session(sid).unwrap().stats);
    assert_eq!(stats.bad_args, 0);
    assert_eq!(stats.skipped_draws, 0, "all six quads were drawn");
    assert!(hal.close(sid));
}
