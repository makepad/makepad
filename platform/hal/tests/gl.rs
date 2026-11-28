//! Native GLES coverage (Linux, EGL's surfaceless platform). The cases of tests/vulkan.rs and
//! tests/d3d11.rs: passes and draws into a texture and an offscreen window, exact readback, a
//! sampled BGRA8 upload, the ledger of a closing session, resize, and the pick variant's ids;
//! and session 0's window layer over a lent window (the executor's badge layer).
//! The fixtures follow the GLSL dialect's contract (shader_ir_glsl): packed geometry attributes,
//! the clip-space Y flip under `mp_clip_y_flip`, `mp_pick` = (draw id, twin mask).
#![cfg(target_os = "linux")]

use makepad_hal::glsl::{Attribute, GlslPipeline, PickTwin, Texture};
use makepad_hal::ledger::Kind;
use makepad_hal::session::HOST;
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

/// The first vec4 of the pass's uniform block: a view of the pass shows its own block.
const PASS_COLOR_GLSL: &str = "#version 300 es
precision highp float;
layout(std140) uniform passUniforms { vec4 pass_color; };
layout(location = 0) out vec4 color;
void main() { color = pass_color; }
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

fn open() -> (std::sync::Arc<Hal>, HalInfo) {
    let hal = Hal::new();
    let info = hal.open(&OpenReq { backend: backend_id::GL, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    assert_eq!(info.backend, backend_id::GL, "GLES HAL initialization failed");
    assert_eq!(info.dialect, dialect::GLSL);
    (hal, info)
}

fn until(hal: &Hal, sid: u32, mut accept: impl FnMut(HalEvent<'_>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut store = EventStore::new(sid, 64, 65536);
    loop {
        assert!(Instant::now() < deadline, "GLES event deadline");
        let mut buf = store.buf();
        let n = hal.pump(10_000_000, &mut buf);
        for (_, event) in store.reader(n, buf.arena_len) {
            if accept(event) { return; }
            match event {
                HalEvent::PipelineFailed { log, .. } => panic!("pipeline: {}", String::from_utf8_lossy(log)),
                HalEvent::DeviceLost { .. } => panic!("GLES device lost"),
                _ => {}
            }
        }
    }
}

fn frame_done(hal: &Hal, sid: u32, frame: u64) {
    until(hal, sid, |e| matches!(e, HalEvent::FrameDone { frame: done, .. } if done == frame));
}

const SAMPLED_ARRAY_GLSL: &str = "#version 300 es
precision highp float;
uniform highp sampler2DArray tex_image;
in vec2 uv;
layout(location = 0) out vec4 color;
void main() { color = texture(tex_image, vec3(uv, 0.0)); }
";

fn payload(vertex: &str, fragment: &str, textures: Vec<Texture>, pick: bool) -> Vec<u8> {
    GlslPipeline {
        vertex: vertex.into(), fragment: fragment.into(),
        blocks: ["draw_callUniforms", "passUniforms", "draw_listUniforms", "userUniforms", "liveUniforms"].map(String::from).to_vec(),
        attributes: vec![Attribute { name: "packed_geometry_0".into(), offset: 0, format: 3, lanes: 4, instance: false }],
        textures, pick_twins: Vec::new(), vertex_stride: 16, instance_stride: 0, instance_texture: false, pick,
    }.encode()
}

const LAYOUT: [u32; 4] = [16, 0, 3, 4];
const NONE: u32 = u32::MAX;

fn floats(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|f| f.to_le_bytes()).collect()
}

fn resources(hal: &Hal, sid: u32) {
    resources_with_geometry(hal, sid, &[-1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0], &[0, 1, 2]);
}

fn resources_with_geometry(hal: &Hal, sid: u32, vertices: &[f32], indices: &[u16]) {
    let mut w = CmdWriter::new(sid);
    w.texture_create(TEX, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 32, 32, [1, 1], Attr::default());
    let vertices = floats(vertices);
    let indices: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
    w.buffer_create(VERTEX, buffer_kind::VERTEX, vertices.len() as u64, Attr::default());
    w.buffer_update(VERTEX, 0, &vertices);
    w.buffer_create(INDEX, buffer_kind::INDEX, indices.len() as u64, Attr::default());
    w.buffer_update(INDEX, 0, &indices);
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

    // Seed the retained window image. The right half must stay green through the partial repaint.
    let base_frame = 6;
    w.clear();
    w.window_create(WINDOW, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 32.0, 32.0], false);
    let before = stats();
    w.frame_begin(base_frame, FrameMeta::default());
    w.pass(pass(Id::NONE, 32, 32, [0.0, 1.0, 0.0, 1.0]));
    w.frame_end(WINDOW, target::WINDOW);
    hal.submit(&w.buf());
    frame_done(&hal, sid, base_frame);
    let seed = stats();
    assert_eq!((seed.draws - before.draws, seed.bad_args - before.bad_args, seed.skipped_draws - before.skipped_draws),
        (0, 0, 0), "the seed frame is a clean clear");

    w.clear();
    w.frame_begin(base_frame + 1, FrameMeta::default());
    let damage = w.put_rects(&[[0.0, 0.0, 16.0, 32.0]]);
    w.pass(Pass { color: [Id::NONE; 4], damage, damage_since: base_frame, ..clear });
    w.draw(Draw { flags: Draw::CULL, damage: 1, ..pair });
    w.draw(Draw { flags: both, damage: 0, ..pair });
    w.frame_end(WINDOW, target::WINDOW);
    hal.submit(&w.buf());
    frame_done(&hal, sid, base_frame + 1);
    let after = stats();
    assert_eq!(after.draws - seed.draws, 2, "both submitted draws are counted");
    assert_eq!(after.bad_args - seed.bad_args, 1, "off-damage invalid flags report one error");
    assert_eq!(after.skipped_draws - seed.skipped_draws, 1, "only the invalid draw is skipped");
    w.clear();
    w.snapshot(WINDOW, 7);
    hal.submit(&w.buf());
    assert_eq!(halves(&pixels(&hal, sid, 7), 16), [white, [0, 255, 0, 255]], "only the left half was repainted");
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

fn sampled_2d(name: String, filter: u32, address: u32) -> Texture {
    Texture { name, slot: 0, shape: 0, stages: 2,
        sampler: SpirvSampler { filter, address, compare: false, pixel: false, anisotropy: 1 } }
}

// Allocation runs glGetError on the renderer's own context. Queue it after the operation
// under test, before the readback used to synchronize. A driver error is in the gate log.
fn checked_pixels(hal: &Hal, sid: u32, target: Id, rect: [u32; 4]) -> Vec<u8> {
    let mut w = CmdWriter::new(sid);
    w.texture_create(Id::new(20, 1), usage::SAMPLED, format::RGBA8, 1, 1, [1, 1], Attr::default());
    w.texture_readback(target, 20, rect);
    hal.submit(&w.buf());
    pixels(hal, sid, 20)
}

#[test]
fn same_texture_sampled_with_two_samplers_produces_distinct_filtering() {
    let (hal, info) = open();
    let sid = info.session;
    resources_with_geometry(&hal, sid, &[-1., -1., 0., 1., 3., -1., 0., 1., -1., 3., 0., 1.], &[0, 1, 2]);
    let mut w = CmdWriter::new(sid);
    w.texture_create(IMAGE, usage::SAMPLED, format::RGBA8, 2, 2, [1, 1], Attr::default());
    // Intensity 254 makes the equal-weight linear blend exactly 127 on every driver.
    w.texture_update(IMAGE, 0, [0, 0, 2, 2], 0,
        &[254,0,0,255, 0,0,254,255, 0,0,254,255, 254,0,0,255]);
    let fragment = "#version 300 es
precision highp float;
uniform sampler2D tex_img;
uniform sampler2D mp_s1_img;
out vec4 color;
void main() {
    vec2 uv = vec2(1.5, 0.5);
    color = gl_FragCoord.x < 1.0 ? texture(tex_img, uv) : texture(mp_s1_img, uv);
}";
    let textures = vec![sampled_2d("tex_img".into(), 0, 1), sampled_2d("mp_s1_img".into(), 1, 0)];
    let code = payload(VERTEX_GLSL, fragment, textures, false);
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() }, &code,
        &[NONE, NONE, NONE, NONE, NONE, 1], &LAYOUT);
    w.pipeline_create(SAMPLED_PIPE, dialect::GLSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, sid, |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == SAMPLED_PIPE));
    w.clear();
    w.frame_begin(1, FrameMeta::default());
    w.pass(pass(TEX, 2, 1, [0.0; 4]));
    let tex = w.put_ids(&[IMAGE]);
    w.draw(Draw { pipeline: SAMPLED_PIPE, tex, ..draw() });
    w.frame_end(TEX, target::TEXTURE);
    hal.submit(&w.buf());
    frame_done(&hal, sid, 1);
    // Clamp+nearest selects column 1, row 1. Repeat+linear averages the four texels.
    assert_eq!(checked_pixels(&hal, sid, TEX, [0, 0, 2, 1]), [254, 0, 0, 255, 127, 0, 127, 255]);
    assert!(hal.close(sid));
}

#[test]
fn undeclared_pick_twin_is_skipped_at_link() {
    let (hal, info) = open();
    let sid = info.session;
    resources(&hal, sid);
    let mut w = CmdWriter::new(sid);
    let dormant = Id::new(4, 1);
    for id in [IMAGE, dormant] {
        w.texture_create(id, usage::SAMPLED, format::RGBA8, 2, 2, [1, 1], Attr::default());
    }
    let fragment = "#version 300 es
precision highp float;
uniform sampler2D tex_size;
out vec4 color;
void main() { color = vec4(vec2(textureSize(tex_size, 0)) / 255.0, 0.0, 1.0); }";
    let mut code = GlslPipeline::decode(&payload(VERTEX_GLSL, fragment,
        vec![sampled_2d("tex_size".into(), 0, 1)], true)).unwrap();
    code.pick_twins.push(PickTwin { name: "mp_pick_tex_size".into(), slot: 0 });
    let bytes = code.encode();
    let mut touched = Vec::new();
    for (frame, slots) in [(1, 1), (2, 2)] {
        let pipeline_id = Id::new(5 + slots, 1);
        w.pipeline_create(pipeline_id, dialect::GLSL, Attr::default(), &CmdWriter::pipeline_blob(
            PipelineDesc { outputs: 1, blend: pipeline_blend::RAW, ..Default::default() }, &bytes,
            &[NONE, NONE, NONE, NONE, NONE, slots], &LAYOUT));
        hal.submit(&w.buf());
        until(&hal, sid, |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == pipeline_id));
        w.clear();
        let before = hal.with_world(|world| world.session(sid).unwrap().stats.binds);
        w.frame_begin(frame, FrameMeta::default());
        w.pass(pass(TEX, 32, 32, [0.; 4]));
        let tex = w.put_ids(&[IMAGE, dormant]);
        let pick_tex = w.put_ids(&[dormant]);
        w.draw(Draw { pipeline: pipeline_id, tex, pick_tex, ..draw() });
        w.frame_end(TEX, target::TEXTURE | target::PICK);
        hal.submit(&w.buf());
        frame_done(&hal, sid, frame);
        hal.with_world(|world| {
            let s = world.session(sid).unwrap();
            assert_eq!(s.stats.skipped_draws, 0);
            // Program, VAO, vertex and index buffers, plus one texture and one sampler.
            assert_eq!(s.stats.binds - before, 4 + 2);
            touched.push(s.ledger.get(Kind::Texture, dormant.index).unwrap().last_used_frame);
        });
        w.clear();
    }
    assert_eq!(touched, [0, 2], "the shared payload must retain independent one- and two-slot PipeStates");
    assert_eq!(checked_pixels(&hal, sid, TEX, [16, 16, 1, 1]), [2, 2, 0, 255]);
    assert!(hal.close(sid));
}

#[test]
fn texture_unit_overflow_fails_pipeline_cleanly() {
    let (hal, info) = open();
    let sid = info.session;
    let mut share = [0; 3];
    until(&hal, sid, |e| match e {
        HalEvent::GlShare { display, config, context } => { share = [display, config, context]; true }
        _ => false,
    });
    let context = gl_interop::GlContext::new(share).unwrap();
    assert!(context.make_current());
    let address = context.proc_address("glGetIntegerv");
    assert!(!address.is_null());
    // SAFETY: glGetIntegerv writes a single GLint for each of these GLES 3 limits.
    let get: unsafe extern "C" fn(u32, *mut i32) = unsafe { std::mem::transmute(address) };
    let [vertex, fragment, combined] = [0x8B4C, 0x8872, 0x8B4D].map(|limit| {
        let mut value = 0;
        unsafe { get(limit, &mut value) };
        assert!(value > 0);
        value
    });
    resources(&hal, sid);
    let count = fragment + 1;
    let mut declarations = String::from("#version 300 es\nprecision highp float;\nout vec4 color;\n");
    let mut body = String::from("void main() { color = vec4(0.0);\n");
    let mut textures = Vec::new();
    for i in 0..count {
        let name = format!("tex_img_{i}");
        declarations.push_str(&format!("uniform sampler2D {name};\n"));
        body.push_str(&format!("color += texture({name}, vec2(0.5));\n"));
        textures.push(sampled_2d(name, 0, 1));
    }
    declarations.push_str(&body);
    declarations.push('}');
    let mut w = CmdWriter::new(sid);
    w.str_def(1, b"sampler-overflow");
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, name: 1, ..Default::default() },
        &payload(VERTEX_GLSL, &declarations, textures, false), &[NONE, NONE, NONE, NONE, NONE, 1], &LAYOUT);
    w.pipeline_create(SAMPLED_PIPE, dialect::GLSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, sid, |e| {
        let HalEvent::PipelineFailed { pipeline, log } = e else { return false; };
        if pipeline != SAMPLED_PIPE { return false; }
        let log = String::from_utf8_lossy(log);
        for expected in ["sampler-overflow".into(), format!("vertex=0 (max {vertex})"),
            format!("fragment={count} (max {fragment})"), format!("combined={count} (max {combined})"),
            format!("logical_textures=1, extra_sampler_pairs={fragment}"), "inst_data=0".into()] {
            assert!(log.contains(&expected), "missing {expected} in {log}");
        }
        assert!(log.contains(if log.contains("GLES link failed:") { "pick_twins=up to 0" } else { "pick_twins=0" }), "{log}");
        true
    });
    rendered(&hal, sid, TEX, target::TEXTURE, [32, 32], 1);
    assert_eq!(checked_pixels(&hal, sid, TEX, [16, 16, 1, 1]), [255; 4]);
    assert!(hal.close(sid));
}

/// A BGRA8 image (stored as uploaded, sampled swizzled) drawn 1:1 into an RGBA8 target reads back
/// as the same colours, top row first; a BGRA8 render target reads back as RGBA too.
#[test]
fn sampled_bgra_upload_round_trips() {
    let (hal, info) = open();
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
    let nearest = SpirvSampler { filter: 0, address: 1, compare: false, pixel: false, anisotropy: 1 };
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() },
        &payload(SAMPLED_VERTEX_GLSL, SAMPLED_GLSL, vec![Texture { name: "tex_image".into(), slot: 0, stages: 2, shape: 0, sampler: nearest }], false),
        &[NONE, NONE, NONE, NONE, NONE, 1], &LAYOUT);
    w.pipeline_create(SAMPLED_PIPE, dialect::GLSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, sid, |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == SAMPLED_PIPE));
    for (frame, target) in [(1, TEX), (2, PICK_TEX)] {
        w.clear();
        w.frame_begin(frame, FrameMeta::default());
        w.pass(pass(target, 2, 2, [0.0; 4]));
        let tex = w.put_ids(&[IMAGE]);
        w.draw(Draw { pipeline: SAMPLED_PIPE, vb: QUAD, ib: QUAD_INDEX, index: [0, 6], inst: Inst::none(1), tex,
            scissor: [f32::NAN; 4], ..Default::default() });
        w.frame_end(target, target::TEXTURE);
        hal.submit(&w.buf());
        frame_done(&hal, sid, frame);
        w.clear();
        w.texture_readback(target, 10 + frame as u32, [0, 0, 2, 2]);
        hal.submit(&w.buf());
        assert_eq!(pixels(&hal, sid, 10 + frame as u32), [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255]);
    }
    // A one-layer texture array (usage::ARRAY, depth 1) is a GL 2D array: an array sampler reads
    // its texels, not the transparent black fallback a 2D texture would get.
    let (array, array_pipe) = (Id::new(11, 1), Id::new(12, 1));
    w.clear();
    w.texture_create(array, usage::SAMPLED | usage::ARRAY, format::BGRA8, 2, 2, [1, 1], Attr::default());
    w.texture_update(array, 0, [0, 0, 2, 2], 0, &image);
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() },
        &payload(SAMPLED_VERTEX_GLSL, SAMPLED_ARRAY_GLSL, vec![Texture { name: "tex_image".into(), slot: 0, stages: 2, shape: 2, sampler: nearest }], false),
        &[NONE, NONE, NONE, NONE, NONE, 1], &LAYOUT);
    w.pipeline_create(array_pipe, dialect::GLSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, sid, |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == array_pipe));
    w.clear();
    w.frame_begin(3, FrameMeta::default());
    w.pass(pass(TEX, 2, 2, [0.0; 4]));
    let tex = w.put_ids(&[array]);
    w.draw(Draw { pipeline: array_pipe, vb: QUAD, ib: QUAD_INDEX, index: [0, 6], inst: Inst::none(1), tex,
        scissor: [f32::NAN; 4], ..Default::default() });
    w.frame_end(TEX, target::TEXTURE);
    hal.submit(&w.buf());
    frame_done(&hal, sid, 3);
    w.clear();
    w.texture_readback(TEX, 13, [0, 0, 2, 2]);
    hal.submit(&w.buf());
    assert_eq!(pixels(&hal, sid, 13), [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255],
        "a one-layer texture array sampled as an array");
    let (buffer, slab) = (Id::new(9, 1), Id::new(10, 1));
    let want_buffer: Vec<u8> = (0..4096u32).map(|i| (i * 7 + 3) as u8).collect();
    let before = hal.with_world(|w| w.session(sid).unwrap().stats.driver_uploads);
    w.clear();
    w.buffer_create(buffer, buffer_kind::INSTANCE, 4096, Attr::default());
    w.buffer_create(slab, buffer_kind::INSTANCE | buffer_kind::APPEND, 4096, Attr::default());
    w.buffer_update(buffer, 0, &want_buffer);
    w.buffer_update_parts(slab, &[(0, &want_buffer[..1000]), (1000, &want_buffer[1000..2048]), (2048, &want_buffer[2048..])]);
    w.read_buffers(1, &[buffer, slab].map(|id| BufferReadRange { id, offset: 0, len: 4096 }));
    hal.submit(&w.buf());
    until(&hal, sid, |e| {
        let HalEvent::BufferLeases { req: 1, status, records } = e else { return false; };
        use makepad_hal::{buffers::Wire, lease::{Held, Record}};
        assert_eq!(status, readback::OK);
        assert_eq!(records.len(), 2 * Record::SIZE, "one lease for each buffer");
        for (id, record) in [buffer, slab].into_iter().zip(records.chunks_exact(Record::SIZE)) {
            // SAFETY: this process pumped the event and takes each lease once.
            let bytes = unsafe { Held::from_payload(record) }.unwrap();
            assert!(*bytes == want_buffer[..], "buffer {id:?} holds other bytes than its upload's");
        }
        true
    });
    assert_eq!(hal.with_world(|w| w.session(sid).unwrap().stats.driver_uploads) - before, 2,
        "one glBufferSubData for the plain buffer and one for the slab's three adjacent parts");
    assert_eq!(hal.with_world(|w| w.session(sid).unwrap().stats.bad_args), 0);
    assert!(hal.close(sid));
}

#[test]
fn gl_render_thread_key_pool_recycles_safely() {
    const CYCLES: u32 = 256;
    let (hal, info) = open();
    let sid = info.session;
    let mut w = CmdWriter::new(sid);
    w.texture_create(TEX, usage::RENDER_TARGET, format::RGBA8, CYCLES, 1, [1, 1], Attr::default());
    let indices: Vec<_> = [0u16, 1, 2, 0, 2, 3].into_iter().flat_map(u16::to_le_bytes).collect();
    w.buffer_create(QUAD_INDEX, buffer_kind::INDEX, indices.len() as u64, Attr::default());
    w.buffer_update(QUAD_INDEX, 0, &indices);
    let sampler = SpirvSampler { filter: 0, address: 1, compare: false, pixel: false, anisotropy: 1 };
    let source = payload(SAMPLED_VERTEX_GLSL, SAMPLED_GLSL, vec![Texture { name: "tex_image".into(), slot: 0, stages: 2, shape: 0, sampler }], false);
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() }, &source,
        &[NONE, NONE, NONE, NONE, NONE, 1], &LAYOUT);
    w.pipeline_create(SAMPLED_PIPE, dialect::GLSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, sid, |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == SAMPLED_PIPE));

    // The texture chooses the color and the vertex buffer chooses the one pixel it can paint.
    let color = |column: u32| [column as u8, !(column as u8), 90, 255];
    let clip_x = |column: u32| 2.0 * column as f32 / CYCLES as f32 - 1.0;
    for first in (0..CYCLES).step_by(4) {
        let end = first + 4;
        for column in first..end {
            let generation = column + 1;
            let image = Id::new(IMAGE.index, generation);
            let vb = Id::new(QUAD.index, generation);
            let (left, right) = (clip_x(column), clip_x(column + 1));
            let vertices = floats(&[left, -1.0, 0.0, 1.0, right, -1.0, 0.0, 1.0,
                right, 1.0, 0.0, 1.0, left, 1.0, 0.0, 1.0]);
            w.clear();
            w.texture_create(image, usage::SAMPLED, format::RGBA8, 1, 1, [1, 1], Attr::default());
            w.texture_update(image, 0, [0, 0, 1, 1], 0, &color(column));
            w.buffer_create(vb, buffer_kind::VERTEX, vertices.len() as u64, Attr::default());
            w.buffer_update(vb, 0, &vertices);
            w.frame_begin(u64::from(generation), FrameMeta::default());
            let mut target_pass = pass(TEX, CYCLES, 1, [0.0; 4]);
            if column != first { target_pass.load[0] = pass_load::LOAD; }
            w.pass(target_pass);
            let tex = w.put_ids(&[image]);
            w.draw(Draw { pipeline: SAMPLED_PIPE, vb, ib: QUAD_INDEX, index: [0, 6], tex, ..draw() });
            w.frame_end(TEX, target::TEXTURE);
            w.texture_destroy(image);
            w.buffer_destroy(vb);
            hal.submit(&w.buf());
        }
        // Wait after the HAL has destroyed the source texture and buffer for every queued draw.
        frame_done(&hal, sid, u64::from(end));
        for column in first..end {
            let req = column + 1;
            w.clear();
            w.texture_readback(TEX, req, [column, 0, 1, 1]);
            hal.submit(&w.buf());
            assert_eq!(pixels(&hal, sid, req), color(column), "churn frame {req}, column {column}");
        }
    }
    let stats = hal.with_world(|world| world.session(sid).unwrap().stats);
    assert_eq!(stats.draws, u64::from(CYCLES));
    assert_eq!(stats.bad_args, 0);
    assert_eq!(stats.skipped_draws, 0);
    assert!(hal.close(sid));
}

#[test]
fn window_damage_uses_image_identity_after_key_reuse() {
    let (hal, info) = open();
    let sid = info.session;
    let mut w = CmdWriter::new(sid);
    w.window_create(WINDOW, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 32.0, 32.0], false);
    hal.submit(&w.buf());

    let clear_window = |frame, kind, size, color, damage_since| {
        let mut w = CmdWriter::new(sid);
        w.frame_begin(frame, FrameMeta::default());
        let mut p = pass(Id::NONE, size, size, color);
        p.damage_since = damage_since;
        if damage_since != 0 { p.damage = w.put_rects(&[[0.0, 0.0, 8.0, 8.0]]); }
        w.pass(p);
        w.frame_end(WINDOW, kind);
        hal.submit(&w.buf());
        frame_done(&hal, sid, frame);
    };
    let mut snapshot = |req| {
        w.clear();
        w.snapshot(WINDOW, req);
        hal.submit(&w.buf());
        pixels(&hal, sid, req)
    };

    let blue = [0.0, 0.0, 1.0, 1.0];
    clear_window(1, target::WINDOW, 32, [0.0, 1.0, 0.0, 1.0], 0);
    // The larger pick pass releases frame 1's image while preserving its damage record.
    // By FrameDone(2) the render thread has returned that image's key to the pool.
    clear_window(2, target::WINDOW | target::PICK, 48, blue, 0);
    clear_window(3, target::WINDOW, 32, [1.0, 0.0, 0.0, 1.0], 1);
    assert_eq!(snapshot(1), [255, 0, 0, 255].repeat(32 * 32), "the image with the recycled key needs a full clear");

    clear_window(4, target::WINDOW, 32, blue, 3);
    let image = snapshot(2);
    assert_eq!(image.len(), 32 * 32 * 4);
    for (i, pixel) in image.chunks_exact(4).enumerate() {
        let (x, y) = (i % 32, i / 32);
        let expected = if x < 8 && y < 8 { [0, 0, 255, 255] } else { [255, 0, 0, 255] };
        assert_eq!(pixel, expected, "partial clear at ({x}, {y}) on frame 4");
    }
    let stats = hal.with_world(|world| world.session(sid).unwrap().stats);
    assert_eq!(stats.bad_args, 0);
    assert_eq!(stats.skipped_draws, 0);
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
    let (hal, info) = open();
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
    let host = hal.window_pixels(info.session, Some(WINDOW)).expect("host pixels of the window");
    assert_eq!((host.w, host.h), (80, 48));
    assert_eq!(host.rgba, image);
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

/// GL interop: a texture the app's context of the HAL's share group made (named by the `GlShare`
/// event) is adopted by name and sampled after the app's fence.
#[test]
fn adopted_app_texture_is_sampled_after_its_fence() {
    let (hal, info) = open();
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
    let linear = SpirvSampler { filter: 1, address: 1, compare: false, pixel: false, anisotropy: 1 };
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() },
        &payload(SAMPLED_VERTEX_GLSL, SAMPLED_GLSL, vec![Texture { name: "tex_image".into(), slot: 0, stages: 2, shape: 0, sampler: linear }], false),
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
    w.draw(Draw { pipeline: SAMPLED_PIPE, vb: QUAD, ib: QUAD_INDEX, index: [0, 6], inst: Inst::none(1), tex,
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
/// the pass drew nothing every frame (Scope's Glass look on a node; GLES too).
#[test]
fn a_pass_a_pixel_larger_than_its_target_still_draws() {
    let (hal, info) = open();
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

/// Session 0's window layer over a window lent to an app (`makepad run`'s load screen over the
/// app), composed by the executor through the window's badge layer: the app's frames stay in the
/// window's retained image, the layer's texture takes the newest one before session 0's next
/// frame, which samples it, and the window shows session 0's frame; once the layer goes the window
/// shows the app's own frame again.
#[test]
fn a_window_layer_samples_the_apps_newest_frame_and_goes() {
    const LAYER: Id = Id::new(20, 1);
    const APP_WINDOW: Id = Id::new(7, 1);
    let hal = Hal::new();
    let req = OpenReq { backend: backend_id::GL, flags: open_flags::OFFSCREEN_WINDOWS | open_flags::RETAIN_WINDOWS, ..Default::default() };
    let info = hal.open_host(&req);
    assert_eq!(info.backend, backend_id::GL, "GLES HAL initialization failed");
    assert_ne!(info.flags & info_flags::WINDOW_LAYERS, 0, "the backend must support window layers");
    let mut w = CmdWriter::new(HOST);
    w.window_create(WINDOW, window_kind::NORMAL, Id::NONE, [0.0, 0.0, 32.0, 32.0], false);
    w.texture_create(LAYER, usage::RENDER_TARGET | usage::SAMPLED, format::BGRA8, 32, 32, [1, 1], Attr::default());
    let quad = floats(&[-1.0, -1.0, 0.0, 1.0, 1.0, -1.0, 0.0, 1.0, 1.0, 1.0, 0.0, 1.0, -1.0, 1.0, 0.0, 1.0]);
    w.buffer_create(QUAD, buffer_kind::VERTEX, quad.len() as u64, Attr::default());
    w.buffer_update(QUAD, 0, &quad);
    let indices: Vec<u8> = [0u16, 1, 2, 0, 2, 3].iter().flat_map(|i| i.to_le_bytes()).collect();
    w.buffer_create(QUAD_INDEX, buffer_kind::INDEX, indices.len() as u64, Attr::default());
    w.buffer_update(QUAD_INDEX, 0, &indices);
    let nearest = SpirvSampler { filter: 0, address: 1, compare: false, pixel: false, anisotropy: 1 };
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, ..Default::default() },
        &payload(SAMPLED_VERTEX_GLSL, SAMPLED_GLSL, vec![Texture { name: "tex_image".into(), slot: 0, stages: 2, shape: 0, sampler: nearest }], false),
        &[NONE, NONE, NONE, NONE, NONE, 1], &LAYOUT);
    w.pipeline_create(SAMPLED_PIPE, dialect::GLSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, HOST, |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == SAMPLED_PIPE));
    // The new background distinguishes the borrower's frame under the layer from its first frame.
    let sid = hal.open(&OpenReq { app_key: 99, ..req }).session;
    assert_ne!(sid, HOST, "the borrower must have its own session");
    resources(&hal, sid);
    let app_frame = |serial: u64| {
        let mut w = CmdWriter::new(sid);
        if serial == 11 { w.window_create(APP_WINDOW, window_kind::NORMAL, Id::NONE, [0.0; 4], true); }
        w.frame_begin(serial, FrameMeta::default());
        let clear = if serial == 11 { [1.0, 0.0, 0.0, 1.0] } else { [0.0, 0.0, 0.0, 1.0] };
        w.pass(pass(Id::NONE, 32, 32, clear));
        w.draw(draw());
        w.frame_end(APP_WINDOW, target::WINDOW);
        hal.submit(&w.buf());
        frame_done(&hal, sid, serial);
    };
    app_frame(11);
    let snapshot = |session, window, req| {
        let mut w = CmdWriter::new(session);
        w.snapshot(window, req);
        hal.submit(&w.buf());
        pixels(&hal, session, req)
    };
    let at = |image: &[u8], x: usize, y: usize| image[(y * 32 + x) * 4..][..4].to_vec();
    assert_eq!(at(&snapshot(HOST, WINDOW, 1), 26, 27), [255; 4], "the app's frame shows");
    // the layer comes on; the app draws under it
    w.clear();
    w.window_overlay(WINDOW, true, [0.0, 0.0, 32.0, 32.0], LAYER, false);
    hal.submit(&w.buf());
    app_frame(12);
    // session 0's frame: blue, the layer texture (the app's frame) over its left half
    w.clear();
    w.frame_begin(101, FrameMeta::default());
    w.pass(pass(Id::NONE, 32, 32, [0.0, 0.0, 1.0, 1.0]));
    let tex = w.put_ids(&[LAYER]);
    w.draw(Draw { pipeline: SAMPLED_PIPE, vb: QUAD, ib: QUAD_INDEX, index: [0, 6], inst: Inst::none(1), tex,
        scissor: [0.0, 0.0, 16.0, 32.0], ..Default::default() });
    w.frame_end(WINDOW, target::WINDOW);
    hal.submit(&w.buf());
    frame_done(&hal, HOST, 101);
    let image = snapshot(HOST, WINDOW, 2);
    assert_eq!(at(&image, 5, 27), [255; 4], "the app's frame, sampled from the layer texture");
    assert_eq!(at(&image, 5, 2), [0, 0, 0, 255], "the layer samples the newest background");
    assert_eq!(at(&image, 26, 27), [0, 0, 255, 255], "session 0's frame covers the app's");
    assert_eq!(at(&image, 26, 2), [0, 0, 255, 255], "the host sees blue over the app's background");
    let borrower = snapshot(sid, APP_WINDOW, 2);
    assert_eq!(borrower.len(), 32 * 32 * 4, "the borrower gets the full window under the layer");
    assert_eq!(at(&borrower, 26, 27), [255; 4], "the borrower sees its white triangle under the layer");
    assert_eq!(at(&borrower, 26, 2), [0, 0, 0, 255], "the borrower sees its newest black background under the layer");
    // The host resizes and draws its layer before the borrower has drawn at the new size.
    w.clear();
    w.window_geom(WINDOW, [0.0, 0.0, 48.0, 40.0]);
    w.frame_begin(102, FrameMeta::default());
    w.pass(pass(Id::NONE, 48, 40, [0.0, 0.0, 1.0, 1.0]));
    w.frame_end(WINDOW, target::WINDOW);
    w.snapshot(WINDOW, 4);
    hal.submit(&w.buf());
    let (rect, resized) = readback(&hal, HOST, 4);
    assert_eq!(rect, [0, 0, 48, 40], "the host snapshot follows the layer frame's new size");
    assert_eq!(&resized[(35 * 48 + 40) * 4..][..4], &[0, 0, 255, 255], "the host's blue layer extends past the borrower's old edge");
    // Loading the same composed image exposes what was copied and cleared before the pass.
    w.clear();
    w.frame_begin(103, FrameMeta::default());
    let mut load = pass(Id::NONE, 48, 40, [0.0; 4]);
    load.load[0] = pass_load::LOAD;
    w.pass(load);
    w.frame_end(WINDOW, target::WINDOW);
    w.snapshot(WINDOW, 5);
    hal.submit(&w.buf());
    let loaded = pixels(&hal, HOST, 5);
    assert_eq!(&loaded[(27 * 48 + 26) * 4..][..4], &[255; 4], "the retained triangle is copied at its original coordinates");
    assert_eq!(&loaded[(35 * 48 + 40) * 4..][..4], &[0; 4], "the uncovered area is cleared even when the blue composed image is reused");
    // the layer goes: the app's frame again, never session 0's
    w.clear();
    w.window_overlay(WINDOW, false, [0.0; 4], Id::NONE, false);
    hal.submit(&w.buf());
    app_frame(13);
    let image = snapshot(HOST, WINDOW, 3);
    assert_eq!(at(&image, 26, 27), [255; 4], "the host sees the app's triangle after the layer goes");
    assert_eq!(at(&image, 26, 2), [0, 0, 0, 255], "the host sees the app's background after the layer goes");
    assert_eq!(snapshot(sid, APP_WINDOW, 3), image, "borrower and host snapshots match after the layer goes");
    hal.with_world(|w| {
        assert_eq!(w.session(HOST).unwrap().stats.bad_args, 0, "the host's commands must be valid");
        assert_eq!(w.session(sid).unwrap().stats.bad_args, 0, "the borrower's commands must be valid");
    });
    assert!(hal.close(sid), "the borrower's session must close");
}

#[test]
fn uniform_stream_renders_pass_list_draw_and_short_block() {
    const MONO: Id = Id::new(5, 1);
    const STEREO: Id = Id::new(6, 1);
    const QUAD: Id = Id::new(5, 1);
    const INDICES: Id = Id::new(6, 1);
    const PIPELINE: Id = Id::new(5, 1);
    const SHORT_PIPE: Id = Id::new(6, 1);
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
    let none = pipeline_binding::NONE;
    let desc = PipelineDesc { outputs: 1, color_formats: [format::RGBA8, 0, 0, 0], ..Default::default() };
    for (id, fields, extra) in [(PIPELINE, "", ""), (SHORT_PIPE, "vec4 pad[19];", " + pad[18]")] {
        let fragment = format!("#version 300 es
precision highp float;
layout(std140) uniform passUniforms {{ vec4 pass_color; }};
layout(std140) uniform draw_listUniforms {{ vec4 list_color; {fields} }};
layout(std140) uniform draw_callUniforms {{ vec4 call_color; }};
layout(location = 0) out vec4 color;
void main() {{ color = pass_color + list_color + call_color{extra}; }}");
        let source = payload(VERTEX_GLSL, &fragment, Vec::new(), false);
        let blob = CmdWriter::pipeline_blob(desc, &source, &[0, 1, 2, none, none, 0], &LAYOUT);
        w.pipeline_create(id, dialect::GLSL, Attr::default(), &blob);
    }
    hal.submit(&w.buf());
    let mut ready = [false; 2];
    until(&hal, sid, |e| {
        if let HalEvent::PipelineReady { pipeline } = e {
            if pipeline == PIPELINE { ready[0] = true; }
            if pipeline == SHORT_PIPE { ready[1] = true; }
        }
        ready == [true; 2]
    });
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
    let short = w.put_uniform(&[0.0f32, 0.5, 0.0, 0.0]);
    w.draw(Draw { pipeline: SHORT_PIPE, uniforms: [call, short, empty, empty], ..right_draw });
    finish(&mut w, 3, MONO, &[(MONO, [128, 128, 0, 255], [0, 0, 0, 0])]);
    let after = stats();
    assert_eq!(after.bad_args - before.bad_args, 3, "rejected uniform arguments");
    assert_eq!(after.skipped_draws - before.skipped_draws, 3, "skipped invalid draws");
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
    let source = payload(VERTEX_GLSL, PASS_COLOR_GLSL, Vec::new(), false);
    let none = pipeline_binding::NONE;
    for (id, blend) in [(ADD, pipeline_blend::ADD), (RAW, pipeline_blend::RAW)] {
        let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, color_formats: [format::RGBA16F, 0, 0, 0], blend, instance_stride: 0, ..Default::default() },
            &source, &[none, 1, none, none, none, 0], &[16, 0, 3, 4]);
        w.pipeline_create(id, dialect::GLSL, Attr::default(), &blob);
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

/// Instance words read through the word texture's array layers: `_mp_inst_word` as
/// shader_ir_glsl emits it for a 24-word record.
const WORDS_VERTEX_GLSL: &str = "#version 300 es
precision highp float;
in vec4 packed_geometry_0;
uniform float mp_clip_y_flip;
uniform highp usampler2DArray mp_inst_data;
uniform int mp_inst_base;
flat out vec3 words;
float _mp_inst_word(int slot) { int i = (gl_InstanceID + mp_inst_base) * 24 + slot; return uintBitsToFloat(texelFetch(mp_inst_data, ivec3(i & 2047, (i >> 11) & 2047, i >> 22), 0).r); }
void main() {
    words = vec3(_mp_inst_word(0), _mp_inst_word(22), _mp_inst_word(23));
    gl_Position = vec4(packed_geometry_0.x, mp_clip_y_flip > 0.5 ? -packed_geometry_0.y : packed_geometry_0.y, packed_geometry_0.zw);
}
";

const WORDS_GLSL: &str = "#version 300 es
precision highp float;
flat in vec3 words;
layout(location = 0) out vec4 color;
void main() { color = vec4(words, 1.0); }
";

#[test]
fn instance_word_texture_spans_two_layers() {
    // One texture window is 2048 x 2048 words. Record A (index 174,762 of 24 words) starts 16
    // words before the window's end, so its word 0 is in layer 0 and words 22 and 23 in layer 1;
    // record B (index 174,848) is layer 1's row 1, a partial last row.
    const TARGET: Id = Id::new(30, 1);
    const TRIANGLE: Id = Id::new(30, 1);
    const TRIANGLE_INDEX: Id = Id::new(31, 1);
    const RECORDS: Id = Id::new(32, 1);
    const WORDS_PIPE: Id = Id::new(30, 1);
    const STRIDE: usize = 24;
    const RECORD_A: usize = 174_762;
    const RECORD_B: usize = 174_848;
    let (hal, info) = open();
    let sid = info.session;
    let mut words = vec![0f32; (RECORD_B + 1) * STRIDE];
    assert_eq!(words.len() * 4, 16_785_504, "two layers of the word texture");
    assert!(RECORD_A * STRIDE < 1 << 22 && (RECORD_A + 1) * STRIDE > 1 << 22, "record A straddles the layer edge");
    for (record, [w0, w22, w23]) in [(RECORD_A, [0.2, 0.4, 0.6]), (RECORD_B, [0.8, 0.1, 0.3])] {
        words[record * STRIDE] = w0;
        words[record * STRIDE + 22] = w22;
        words[record * STRIDE + 23] = w23;
    }
    let mut w = CmdWriter::new(sid);
    w.texture_create(TARGET, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 4, 1, [1, 1], Attr::default());
    let vertices = floats(&[-1.0, -1.0, 0.0, 1.0, 3.0, -1.0, 0.0, 1.0, -1.0, 3.0, 0.0, 1.0]);
    w.buffer_create(TRIANGLE, buffer_kind::VERTEX, vertices.len() as u64, Attr::default());
    w.buffer_update(TRIANGLE, 0, &vertices);
    w.buffer_create(TRIANGLE_INDEX, buffer_kind::INDEX, 6, Attr::default());
    w.buffer_update(TRIANGLE_INDEX, 0, &[0, 0, 1, 0, 2, 0]);
    w.buffer_create(RECORDS, buffer_kind::INSTANCE | buffer_kind::APPEND, words.len() as u64 * 4, Attr::default());
    w.buffer_update(RECORDS, 0, &floats(&words));
    let code = GlslPipeline {
        vertex: WORDS_VERTEX_GLSL.into(), fragment: WORDS_GLSL.into(),
        blocks: ["draw_callUniforms", "passUniforms", "draw_listUniforms", "userUniforms", "liveUniforms"].map(String::from).to_vec(),
        attributes: vec![Attribute { name: "packed_geometry_0".into(), offset: 0, format: 3, lanes: 4, instance: false }],
        textures: Vec::new(), pick_twins: Vec::new(), vertex_stride: 16, instance_stride: (STRIDE * 4) as u32, instance_texture: true, pick: false,
    }.encode();
    let blob = CmdWriter::pipeline_blob(PipelineDesc { outputs: 1, instance_stride: STRIDE as u32, ..Default::default() }, &code,
        &[NONE, NONE, NONE, NONE, NONE, 0], &LAYOUT);
    w.pipeline_create(WORDS_PIPE, dialect::GLSL, Attr::default(), &blob);
    hal.submit(&w.buf());
    until(&hal, sid, |e| matches!(e, HalEvent::PipelineReady { pipeline } if pipeline == WORDS_PIPE));

    let byte = |v: f32| (v * 255.0).round() as u8;
    let mut want = [[0.2, 0.4, 0.6], [0.8, 0.1, 0.3]];
    for frame in 1..=2u64 {
        let mut w = CmdWriter::new(sid);
        if frame == 2 {
            // only record B's word 23 changes: a dirty range inside layer 1
            want[1][2] = 0.9;
            w.buffer_update(RECORDS, ((RECORD_B * STRIDE + 23) * 4) as u64, &0.9f32.to_le_bytes());
        }
        w.frame_begin(frame, FrameMeta::default());
        w.pass(pass(TARGET, 4, 1, [0.0, 0.0, 0.0, 1.0]));
        for (record, x) in [(RECORD_A, 0.0), (RECORD_B, 2.0)] {
            w.draw(Draw { pipeline: WORDS_PIPE, vb: TRIANGLE, ib: TRIANGLE_INDEX, index: [0, 3], inst: Inst::buffer(RECORDS, record as u32, 1),
                scissor: [x, 0.0, 2.0, 1.0], ..Default::default() });
        }
        w.frame_end(TARGET, target::TEXTURE);
        hal.submit(&w.buf());
        frame_done(&hal, sid, frame);
        w.clear();
        w.texture_readback(TARGET, frame as u32, [0, 0, 4, 1]);
        hal.submit(&w.buf());
        let image = pixels(&hal, sid, frame as u32);
        for (px, rgb) in [want[0], want[0], want[1], want[1]].iter().enumerate() {
            let got = &image[px * 4..][..4];
            for (c, v) in rgb.iter().enumerate() {
                assert!(got[c].abs_diff(byte(*v)) <= 1, "frame {frame}, pixel {px}, channel {c}: {got:?}, expected {rgb:?}");
            }
            assert_eq!(got[3], 255);
        }
    }
    let stats = hal.with_world(|world| world.session(sid).unwrap().stats);
    assert_eq!(stats.bad_args, 0);
    assert_eq!(stats.skipped_draws, 0, "both records were drawn in both frames");
    assert!(hal.close(sid));
}
