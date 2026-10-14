//! Native GLES coverage (Linux, EGL's surfaceless platform). The cases of tests/vulkan.rs and
//! tests/d3d11.rs: passes and draws into a texture and an offscreen window, exact readback, a
//! sampled BGRA8 upload, the ledger of a closing session, resize, and the pick variant's ids.
//! The fixtures follow the GLSL dialect's contract (shader_ir_glsl): packed geometry attributes,
//! the clip-space Y flip under `mp_clip_y_flip`, `mp_pick` = (draw id, twin mask).
#![cfg(target_os = "linux")]

use makepad_hal::glsl::{Attribute, GlslPipeline, Texture};
use makepad_hal::ledger::Kind;
use makepad_hal::spirv::SpirvSampler;
use makepad_hal::*;
use std::time::{Duration, Instant};

const TEX: Id = Id::new(1, 1);
const PICK_TEX: Id = Id::new(2, 1);
const IMAGE: Id = Id::new(3, 1);
const VERTEX: Id = Id::new(1, 1);
const INDEX: Id = Id::new(2, 1);
const QUAD: Id = Id::new(3, 1);
const QUAD_INDEX: Id = Id::new(4, 1);
const PIPE: Id = Id::new(1, 1);
const PICK_PIPE: Id = Id::new(2, 1);
const SAMPLED_PIPE: Id = Id::new(3, 1);
const WINDOW: Id = Id::new(1, 1);

const VERTEX_GLSL: &str = "#version 300 es
precision highp float;
in vec4 packed_geometry_0;
uniform float mp_clip_y_flip;
void main() { gl_Position = vec4(packed_geometry_0.x, mp_clip_y_flip > 0.5 ? -packed_geometry_0.y : packed_geometry_0.y, packed_geometry_0.zw); }
";

const WHITE_GLSL: &str = "#version 300 es
precision highp float;
layout(location = 0) out vec4 color;
void main() { color = vec4(1.0); }
";

const PICK_GLSL: &str = "#version 300 es
precision highp float;
uniform uvec2 mp_pick;
layout(location = 0) out vec4 color;
void main() { uint p = mp_pick.x; color = vec4(float(p & 255u), float((p >> 8u) & 255u), float((p >> 16u) & 255u), 255.0) / 255.0; }
";

/// uv from the clip position before the flip: clip y = +1 is the top row (v = 0)
const SAMPLED_VERTEX_GLSL: &str = "#version 300 es
precision highp float;
in vec4 packed_geometry_0;
uniform float mp_clip_y_flip;
out vec2 uv;
void main() {
    uv = vec2(packed_geometry_0.x * 0.5 + 0.5, 0.5 - packed_geometry_0.y * 0.5);
    gl_Position = vec4(packed_geometry_0.x, mp_clip_y_flip > 0.5 ? -packed_geometry_0.y : packed_geometry_0.y, packed_geometry_0.zw);
}
";

const SAMPLED_GLSL: &str = "#version 300 es
precision highp float;
uniform sampler2D tex_image;
in vec2 uv;
layout(location = 0) out vec4 color;
void main() { color = texture(tex_image, uv); }
";

fn open() -> Option<(std::sync::Arc<Hal>, HalInfo)> {
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::GL, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    if info.backend == backend_id::NONE {
        eprintln!("SKIP GLES: no EGL surfaceless GLES 3 context on this machine");
        return None;
    }
    assert_eq!(info.backend, backend_id::GL);
    assert_eq!(info.dialect, dialect::GLSL);
    Some((hal, info))
}

fn until(hal: &Hal, sid: u32, mut accept: impl FnMut(HalEvent<'_>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut store = EventStore::new(sid, 64, 65536);
    loop {
        assert!(Instant::now() < deadline, "GLES event deadline");
        let mut buf = store.buf();
        let n = hal.pump(10_000_000, &mut buf);
        for (_, event) in store.reader(n, buf.arena_len) {
            match event {
                HalEvent::PipelineFailed { log, .. } => panic!("pipeline: {}", String::from_utf8_lossy(log)),
                HalEvent::DeviceLost { .. } => panic!("GLES device lost"),
                _ => {}
            }
            if accept(event) { return; }
        }
    }
}

fn frame_done(hal: &Hal, sid: u32, frame: u64) {
    until(hal, sid, |e| matches!(e, HalEvent::FrameDone { frame: done, .. } if done == frame));
}

fn payload(vertex: &str, fragment: &str, textures: Vec<Texture>, pick: bool) -> Vec<u8> {
    GlslPipeline {
        vertex: vertex.into(), fragment: fragment.into(),
        blocks: ["draw_callUniforms", "passUniforms", "draw_listUniforms", "userUniforms", "liveUniforms"].map(String::from).to_vec(),
        attributes: vec![Attribute { name: "packed_geometry_0".into(), offset: 0, format: 3, lanes: 4, instance: false }],
        textures, vertex_stride: 16, instance_stride: 0, instance_texture: false, pick,
    }.encode()
}

const LAYOUT: [u32; 4] = [16, 0, 3, 4];
const NONE: u32 = u32::MAX;

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn resources(hal: &Hal, sid: u32) {
    let mut w = CmdWriter::new(sid);
    w.texture_create(TEX, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 32, 32, [1, 1], Attr::default());
    let vertices = floats(&[-1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0]);
    w.buffer_create(VERTEX, buffer_kind::VERTEX, vertices.len() as u64, Attr::default());
    w.buffer_update(VERTEX, 0, &vertices);
    w.buffer_create(INDEX, buffer_kind::INDEX, 6, Attr::default());
    w.buffer_update(INDEX, 0, &[0, 0, 1, 0, 2, 0]);
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() }, &payload(VERTEX_GLSL, WHITE_GLSL, Vec::new(), false),
        &[NONE, NONE, NONE, NONE, NONE, 0], &LAYOUT);
    w.pipeline_create(PIPE, dialect::GLSL, Attr::default(), &blob);
    let pick = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, blend: pipeline_blend::RAW, color_formats: [format::BGRA8, 0, 0, 0], ..Default::default() },
        &payload(VERTEX_GLSL, PICK_GLSL, Vec::new(), true), &[NONE, NONE, NONE, NONE, NONE, 0], &LAYOUT);
    w.pipeline_create(PICK_PIPE, dialect::GLSL, Attr::default(), &pick);
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
    let Some((hal, info)) = open() else { return };
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
    let Some((hal, info)) = open() else { return };
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

/// A BGRA8 image (stored as uploaded, sampled swizzled) drawn 1:1 into an RGBA8 target reads back
/// as the same colours, top row first; a BGRA8 render target reads back as RGBA too.
#[test]
fn sampled_bgra_upload_round_trips() {
    let Some((hal, info)) = open() else { return };
    let sid = info.session;
    let mut w = CmdWriter::new(sid);
    // BGRA bytes of red, green / blue, white
    let image: [u8; 16] = [0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255];
    w.texture_create(IMAGE, usage::SAMPLED, format::BGRA8, 2, 2, [1, 1], Attr::default());
    w.texture_update(IMAGE, 0, [0, 0, 2, 2], 0, &image);
    for (id, fmt) in [(TEX, format::RGBA8), (PICK_TEX, format::BGRA8)] {
        w.texture_create(id, usage::RENDER_TARGET | usage::SAMPLED, fmt, 2, 2, [1, 1], Attr::default());
    }
    let quad = floats(&[-1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, -1.0, 1.0, 0.0, 1.0]);
    w.buffer_create(QUAD, buffer_kind::VERTEX, quad.len() as u64, Attr::default());
    w.buffer_update(QUAD, 0, &quad);
    let indices: Vec<u8> = [0u16, 1, 2, 0, 2, 3].iter().flat_map(|i| i.to_le_bytes()).collect();
    w.buffer_create(QUAD_INDEX, buffer_kind::INDEX, indices.len() as u64, Attr::default());
    w.buffer_update(QUAD_INDEX, 0, &indices);
    let nearest = SpirvSampler { filter: 0, address: 1, compare: false, pixel: false };
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() },
        &payload(SAMPLED_VERTEX_GLSL, SAMPLED_GLSL, vec![Texture { name: "tex_image".into(), shape: 0, sampler: nearest }], false),
        &[NONE, NONE, NONE, NONE, NONE, 1], &LAYOUT);
    w.pipeline_create(SAMPLED_PIPE, dialect::GLSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, sid, |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == SAMPLED_PIPE));
    for (frame, target) in [(1, TEX), (2, PICK_TEX)] {
        w.clear();
        w.frame_begin(frame, FrameMeta::default());
        w.pass(pass(target, 2, 2, [0.0; 4]));
        let tex = w.put_ids(&[IMAGE]);
        w.draw(Draw { pipeline: SAMPLED_PIPE, vb: QUAD, ib: QUAD_INDEX, index: [0, 6], inst: Inst::inline(Slice::EMPTY, 1), tex,
            scissor: [f32::NAN; 4], ..Default::default() });
        w.frame_end(target, target::TEXTURE);
        hal.submit(&w.buf());
        frame_done(&hal, sid, frame);
        w.clear();
        w.texture_readback(target, 10 + frame as u32, [0, 0, 2, 2]);
        hal.submit(&w.buf());
        assert_eq!(pixels(&hal, sid, 10 + frame as u32), [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]);
    }
    assert_eq!(hal.with_world(|w| w.session(sid).unwrap().stats.bad_args), 0);
    assert!(hal.close(sid));
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
    let Some((hal, info)) = open() else { return };
    let other = hal.open(&OpenReq { backend: backend_id::GL, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() }).session;
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
    let Some((hal, info)) = open() else { return };
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
    let host = hal.window_pixels(info.session, Some(WINDOW)).expect("host pixels of the window");
    assert_eq!((host.w, host.h), (80, 48));
    assert_eq!(host.rgba, image);
    assert!(hal.close(info.session));
}

#[test]
fn pick_frame_answers_the_draw_id() {
    let Some((hal, info)) = open() else { return };
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

/// GL interop: a texture the app's context of the HAL's share group made (named by the `GlShare`
/// event) is adopted by name and sampled after the app's fence.
#[test]
fn adopted_app_texture_is_sampled_after_its_fence() {
    let Some((hal, info)) = open() else { return };
    let sid = info.session;
    let mut share = None;
    until(&hal, sid, |e| match e {
        HalEvent::GlShare { display, config, context } => { share = Some([display, config, context]); true }
        _ => false,
    });
    let context = makepad_hal::gl_interop::GlContext::new(share.unwrap()).expect("a context of the share group");
    assert!(context.make_current());
    type GenTextures = unsafe extern "C" fn(i32, *mut u32);
    type BindTexture = unsafe extern "C" fn(u32, u32);
    type TexImage2D = unsafe extern "C" fn(u32, i32, i32, i32, i32, i32, u32, u32, *const std::ffi::c_void);
    let (gen, bind, image): (GenTextures, BindTexture, TexImage2D) = unsafe {
        (std::mem::transmute(context.proc_address("glGenTextures")), std::mem::transmute(context.proc_address("glBindTexture")),
            std::mem::transmute(context.proc_address("glTexImage2D")))
    };
    let mut name = 0;
    let green = [0u8, 200, 0, 255].repeat(4);
    unsafe {
        gen(1, &mut name);
        bind(0x0DE1, name);
        image(0x0DE1, 0, 0x8058, 2, 2, 0, 0x1908, 0x1401, green.as_ptr() as _);
        bind(0x0DE1, 0);
    }
    let sync = context.fence();
    let mut w = CmdWriter::new(sid);
    w.texture_adopt_gl(IMAGE, name, format::RGBA8, 2, 2, false);
    w.gl_fence(sync);
    w.texture_create(TEX, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 2, 2, [1, 1], Attr::default());
    let quad = floats(&[-1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, -1.0, 1.0, 0.0, 1.0]);
    w.buffer_create(QUAD, buffer_kind::VERTEX, quad.len() as u64, Attr::default());
    w.buffer_update(QUAD, 0, &quad);
    let indices: Vec<u8> = [0u16, 1, 2, 0, 2, 3].iter().flat_map(|i| i.to_le_bytes()).collect();
    w.buffer_create(QUAD_INDEX, buffer_kind::INDEX, indices.len() as u64, Attr::default());
    w.buffer_update(QUAD_INDEX, 0, &indices);
    let linear = SpirvSampler { filter: 1, address: 1, compare: false, pixel: false };
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() },
        &payload(SAMPLED_VERTEX_GLSL, SAMPLED_GLSL, vec![Texture { name: "tex_image".into(), shape: 0, sampler: linear }], false),
        &[NONE, NONE, NONE, NONE, NONE, 1], &LAYOUT);
    w.pipeline_create(SAMPLED_PIPE, dialect::GLSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    let mut imported = None;
    until(&hal, sid, |e| match e {
        HalEvent::TextureImported { texture, ok } if texture == IMAGE => { imported = Some(ok); false }
        HalEvent::PipelineReady { pipeline } if pipeline == SAMPLED_PIPE => true,
        _ => false,
    });
    assert_eq!(imported, Some(true));
    w.clear();
    w.frame_begin(1, FrameMeta::default());
    w.pass(pass(TEX, 2, 2, [0.0; 4]));
    let tex = w.put_ids(&[IMAGE]);
    w.draw(Draw { pipeline: SAMPLED_PIPE, vb: QUAD, ib: QUAD_INDEX, index: [0, 6], inst: Inst::inline(Slice::EMPTY, 1), tex,
        scissor: [f32::NAN; 4], ..Default::default() });
    w.frame_end(TEX, target::TEXTURE);
    hal.submit(&w.buf());
    frame_done(&hal, sid, 1);
    w.clear();
    w.texture_readback(TEX, 5, [0, 0, 2, 2]);
    hal.submit(&w.buf());
    assert_eq!(pixels(&hal, sid, 5), green);
    assert_eq!(hal.with_world(|w| w.session(sid).unwrap().stats.bad_args), 0);
    assert!(hal.close(sid));
}

/// A pass a pixel larger than its colour target (a target floored where its pass was ceiled, at
/// a fractional scale such as 1.41) still draws: clipped to the target, never refused. Refused,
/// the pass drew nothing every frame (Scope's Glass look on 149; GLES too).
#[test]
fn a_pass_a_pixel_larger_than_its_target_still_draws() {
    let Some((hal, info)) = open() else { return };
    let sid = info.session;
    resources(&hal, sid);
    let before = hal.with_world(|w| w.session(sid).unwrap().stats);
    let mut w = CmdWriter::new(sid);
    w.frame_begin(1, FrameMeta::default());
    let mut p = pass(TEX, 33, 33, [17.0 / 255.0, 34.0 / 255.0, 51.0 / 255.0, 1.0]);
    p.dpi = 1.41;
    w.pass(p);
    w.draw(draw());
    w.frame_end(TEX, target::TEXTURE);
    hal.submit(&w.buf());
    frame_done(&hal, sid, 1);
    let after = hal.with_world(|w| w.session(sid).unwrap().stats);
    assert_eq!(after.passes - before.passes, 1, "the pass was refused");
    assert_eq!(after.clamped_passes - before.clamped_passes, 1);
    w.clear();
    w.texture_readback(TEX, 3, [0, 0, 32, 32]);
    hal.submit(&w.buf());
    let image = pixels(&hal, sid, 3);
    // the clear in the top corners, the triangle's base along the bottom
    for x in [0, 31] { assert_eq!(&image[x * 4..][..4], &[17, 34, 51, 255]); }
    for x in [6, 16, 26] { assert_eq!(&image[(29 * 32 + x) * 4..][..4], &[255; 4], "x {x}"); }
    assert!(hal.close(sid));
}
