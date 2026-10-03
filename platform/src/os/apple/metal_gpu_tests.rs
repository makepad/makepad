//! GPU tests on the real Metal device (KERNELS.md P0-S and P0 gates):
//!
//! - the hostile-shader suite: Splash shaders with out-of-range indices and
//!   runaway nested loops, compiled by the shader compiler into the same
//!   Metal source the backend builds, run on the GPU, finishing quickly with
//!   in-range (clamped) results;
//! - MRT pixel tests: one pass with RGBA16F, RG16F and R32Uint attachments,
//!   drawn through pipelines described by `describe_mrt_attachments`, then a
//!   one-output shader drawn into the same pass leaving the other
//!   attachments untouched.
//!
//! Run with `cargo test --release -p makepad-platform --lib metal_gpu_tests`.

use super::metal::{describe_mrt_attachments, texture_pixel_to_mtl_pixel};
use crate::makepad_objc_sys::{class, msg_send, sel, sel_impl};
use crate::makepad_script::shader::*;
use crate::makepad_script::shader_backend::*;
use crate::makepad_script::*;
use crate::os::apple::apple_sys::*;
use crate::os::apple::apple_util::{nsstring_to_string, str_to_nsstring};
use crate::texture::TexturePixel;
use std::time::{Duration, Instant};

struct Gpu {
    device: ObjcId,
    queue: ObjcId,
}

impl Gpu {
    fn new() -> Option<Gpu> {
        let device = unsafe { MTLCreateSystemDefaultDevice() };
        if device.is_null() {
            return None;
        }
        let queue: ObjcId = unsafe { msg_send![device, newCommandQueue] };
        Some(Gpu { device, queue })
    }

    fn library(&self, source: &str) -> ObjcId {
        let mut error: ObjcId = nil;
        let library: ObjcId = unsafe {
            msg_send![self.device, newLibraryWithSource: str_to_nsstring(source) options: nil error: &mut error]
        };
        if library.is_null() {
            let message = if error.is_null() { String::new() } else { nsstring_to_string(unsafe { msg_send![error, localizedDescription] }) };
            panic!("Metal library failed: {message}\n{source}");
        }
        library
    }

    fn function(library: ObjcId, name: &str) -> ObjcId {
        let f: ObjcId = unsafe { msg_send![library, newFunctionWithName: str_to_nsstring(name)] };
        assert!(!f.is_null(), "no function {name}");
        f
    }

    fn pipeline(&self, descriptor: ObjcId) -> Result<ObjcId, String> {
        let mut error: ObjcId = nil;
        let pipeline: ObjcId = unsafe {
            msg_send![self.device, newRenderPipelineStateWithDescriptor: descriptor error: &mut error]
        };
        if pipeline.is_null() {
            return Err(if error.is_null() { "no pipeline".into() } else { nsstring_to_string(unsafe { msg_send![error, localizedDescription] }) });
        }
        Ok(pipeline)
    }

    fn target(&self, format: MTLPixelFormat, size: u64) -> ObjcId {
        unsafe {
            let d: ObjcId = msg_send![class!(MTLTextureDescriptor), new];
            let () = msg_send![d, setTextureType: MTLTextureType::D2];
            let () = msg_send![d, setWidth: size];
            let () = msg_send![d, setHeight: size];
            let () = msg_send![d, setPixelFormat: format];
            let () = msg_send![d, setStorageMode: MTLStorageMode::Shared];
            let () = msg_send![d, setUsage: MTLTextureUsage::RenderTarget as u64 | MTLTextureUsage::ShaderRead as u64];
            let t: ObjcId = msg_send![self.device, newTextureWithDescriptor: d];
            assert!(!t.is_null());
            t
        }
    }

    fn buffer(&self, bytes: &[u8]) -> ObjcId {
        unsafe {
            msg_send![self.device, newBufferWithBytes: bytes.as_ptr() as *const std::ffi::c_void
                length: bytes.len() as u64 options: MTLResourceOptions::StorageModeShared]
        }
    }

    /// One pass over `targets` (clear or load each), drawing a full-screen
    /// triangle with each pipeline in turn; returns the GPU wall time.
    fn draw(&self, targets: &[(ObjcId, Option<[f64; 4]>)], pipelines: &[ObjcId], buffers: &[ObjcId]) -> Duration {
        unsafe {
            let pass: ObjcId = msg_send![class!(MTLRenderPassDescriptor), renderPassDescriptor];
            let attachments: ObjcId = msg_send![pass, colorAttachments];
            for (i, (texture, clear)) in targets.iter().enumerate() {
                let a: ObjcId = msg_send![attachments, objectAtIndexedSubscript: i as u64];
                let () = msg_send![a, setTexture: *texture];
                let () = msg_send![a, setStoreAction: MTLStoreAction::Store];
                match clear {
                    Some(c) => {
                        let () = msg_send![a, setLoadAction: MTLLoadAction::Clear];
                        let () = msg_send![a, setClearColor: MTLClearColor { red: c[0], green: c[1], blue: c[2], alpha: c[3] }];
                    }
                    None => {
                        let () = msg_send![a, setLoadAction: MTLLoadAction::Load];
                    }
                }
            }
            let cb: ObjcId = msg_send![self.queue, commandBuffer];
            let enc: ObjcId = msg_send![cb, renderCommandEncoderWithDescriptor: pass];
            for pipeline in pipelines {
                let () = msg_send![enc, setRenderPipelineState: *pipeline];
                for (i, b) in buffers.iter().enumerate() {
                    let () = msg_send![enc, setVertexBuffer: *b offset: 0u64 atIndex: i as u64];
                    let () = msg_send![enc, setFragmentBuffer: *b offset: 0u64 atIndex: i as u64];
                }
                let () = msg_send![enc, drawPrimitives: MTLPrimitiveType::Triangle vertexStart: 0u64 vertexCount: 3u64];
            }
            let () = msg_send![enc, endEncoding];
            let started = Instant::now();
            let () = msg_send![cb, commit];
            let () = msg_send![cb, waitUntilCompleted];
            let error: ObjcId = msg_send![cb, error];
            assert!(error.is_null(), "command buffer failed: {}", nsstring_to_string(msg_send![error, localizedDescription]));
            started.elapsed()
        }
    }

    fn read(texture: ObjcId, bytes_per_pixel: u64, size: u64) -> Vec<u8> {
        let mut out = vec![0u8; (bytes_per_pixel * size * size) as usize];
        let region = MTLRegion { origin: MTLOrigin { x: 0, y: 0, z: 0 }, size: MTLSize { width: size, height: size, depth: 1 } };
        unsafe {
            let () = msg_send![texture, getBytes: out.as_mut_ptr() bytesPerRow: bytes_per_pixel * size fromRegion: region mipmapLevel: 0u64];
        }
        out
    }
}

fn f16_to_f32(h: u16) -> f32 {
    let sign = if h & 0x8000 != 0 { -1.0 } else { 1.0 };
    let exp = ((h >> 10) & 0x1f) as i32;
    let mant = (h & 0x3ff) as f32;
    sign * match exp {
        0 => mant * 2f32.powi(-24),
        31 => f32::INFINITY,
        e => (1.0 + mant / 1024.0) * 2f32.powi(e - 15),
    }
}

fn words_f32(bytes: &[u8]) -> Vec<f32> {
    bytes.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

// ---------------------------------------------------------------------------
// Hostile shaders.

/// Compile a Splash draw shader (a full-screen triangle from a vec2 vertex
/// buffer, one `vec4f` output) into the Metal library source the backend
/// builds, with `u_n` a uniform the GPU reads as 0.
fn splash_metal_source(fragment_body: &str, extra: &str) -> String {
    splash_metal_source_with(fragment_body, extra, "")
}

fn splash_metal_source_with(fragment_body: &str, extra: &str, vertex_extra: &str) -> String {
    splash_metal_compile(fragment_body, extra, vertex_extra).0
}

/// The Metal source, and the instance record's word offset per field name
/// and its stride (`ShaderOutput::instance_record`).
fn splash_metal_compile(fragment_body: &str, extra: &str, vertex_extra: &str) -> (String, Vec<(LiveId, usize)>, usize) {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host, bx: Box::new(ScriptVmBase::new()) };
    // `base:` in `extra` splits the members: those before it go on a base
    // object the shader extends (as kits extend DrawVjFxBase).
    let (base, extra) = extra.split_once("base:").map(|(b, e)| (b, e)).unwrap_or(("", extra));
    let code = format!(
        "use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet base = {{\n\
         geom: shader.vertex_buffer(vec2f, nil)\n\
         vertex_pos: shader.vertex_position(vec4f)\n\
         pixel: shader.fragment_output(0, vec4f)\n\
         u_n: shader.uniform(0.0)\n\
         {base}\n}}\nbase{{\n\
         {extra}\n\
         vertex: fn() {{ self.vertex_pos = vec4(self.geom.x, self.geom.y, 0.0, 1.0)\n{vertex_extra}\n}}\n\
         fragment: fn() {{\n{fragment_body}\n}}\n}}"
    );
    let value = vm.with_instruction_limit(5_000_000, |vm| {
        vm.eval(ScriptMod { file: "metal_gpu_tests".into(), code, ..Default::default() })
    });
    let io_self = value.as_object().expect("shader object");
    let mut output = ShaderOutput::default();
    output.backend = ShaderBackend::Metal;
    output.use_vulkan = false;
    output.pre_collect_rust_instance_io(&mut vm, io_self);
    output.pre_collect_shader_io(&mut vm, io_self);
    for (entry, mode) in [(id!(vertex), ShaderMode::Vertex), (id!(fragment), ShaderMode::Fragment)] {
        let fnobj = vm.bx.heap.object_method(io_self, entry.into(), NoTrap).as_object().expect("entry");
        output.mode = mode;
        ShaderFnCompiler::compile_shader_def(&mut vm, &mut output, NoTrap, entry, fnobj, ShaderType::IoSelf(io_self), vec![]);
    }
    assert!(!output.has_errors, "{}", output.error_report());
    output.assign_uniform_buffer_indices(&vm.bx.heap, 3);
    let (fields, stride) = output.instance_record(&vm);
    let offsets = fields.iter().map(|(index, offset)| (output.io[*index].name, *offset)).collect();
    (output.metal_draw_source(&vm), offsets, stride)
}

/// Run a Splash fragment on a 4x4 RGBA32Float target: the pixel it wrote
/// and the GPU time.
fn run_splash(gpu: &Gpu, fragment_body: &str, extra: &str) -> ([f32; 4], Duration) {
    run_splash_with(gpu, fragment_body, extra, "")
}

fn run_splash_with(gpu: &Gpu, fragment_body: &str, extra: &str, vertex_extra: &str) -> ([f32; 4], Duration) {
    let source = splash_metal_source_with(fragment_body, extra, vertex_extra);
    let library = gpu.library(&source);
    let descriptor: ObjcId = unsafe { msg_send![class!(MTLRenderPipelineDescriptor), new] };
    unsafe {
        let () = msg_send![descriptor, setVertexFunction: Gpu::function(library, "vertex_main")];
        let () = msg_send![descriptor, setFragmentFunction: Gpu::function(library, "fragment_main")];
        let attachments: ObjcId = msg_send![descriptor, colorAttachments];
        let a: ObjcId = msg_send![attachments, objectAtIndexedSubscript: 0u64];
        let () = msg_send![a, setPixelFormat: MTLPixelFormat::RGBA32Float];
    }
    let pipeline = gpu.pipeline(descriptor).unwrap_or_else(|e| panic!("{e}\n{source}"));
    let triangle: Vec<u8> = [-1.0f32, -1.0, 3.0, -1.0, -1.0, 3.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    let zeros = vec![0u8; 4096];
    let buffers = [gpu.buffer(&triangle), gpu.buffer(&zeros), gpu.buffer(&zeros)];
    let target = gpu.target(MTLPixelFormat::RGBA32Float, 4);
    let time = gpu.draw(&[(target, Some([0.0; 4]))], &[pipeline], &buffers);
    let px = words_f32(&Gpu::read(target, 16, 4));
    ([px[0], px[1], px[2], px[3]], time)
}

#[test]
fn hostile_indices_stay_in_bounds_on_the_gpu() {
    let Some(gpu) = Gpu::new() else { return };
    // i = 1000000, j = -5, k = 7u: every access is clamped into range.
    let body = "var arr = array(1f, 2f, 3f, 4f)\n\
                let i = int(self.u_n) + 1000000\n\
                let j = int(self.u_n) - 5\n\
                let a = arr[i]\n\
                arr[i] = 9f\n\
                arr[j] += 10f\n\
                var v = vec3(5f, 6f, 7f)\n\
                let k = uint(self.u_n) + 7u\n\
                v[k] *= 2f\n\
                self.pixel = vec4(a, arr[0], arr[3], v[k])";
    let (px, _) = run_splash(&gpu, body, "");
    assert_eq!(px, [4.0, 11.0, 9.0, 14.0]);
}

#[test]
fn runaway_nested_loops_finish_within_the_budget_on_the_gpu() {
    let Some(gpu) = Gpu::new() else { return };
    let huge = "uint(self.u_n) + 4000000000u";
    // Two nested runtime loops of ~4e9 each: 1.6e19 passes unguarded.
    let (px, time) = run_splash(&gpu, &format!("var s = 0f\nfor i in 0..{huge} {{ for j in 0..{huge} {{ s += 1f }} }}\nself.pixel = vec4(s, 0.0, 0.0, 1.0)"), "");
    let budget = crate::makepad_script::shader_control::SHADER_ITERATION_BUDGET as f32;
    assert!(px[0] > 1000.0 && px[0] <= budget, "{px:?}");
    assert!(time < Duration::from_secs(10), "{time:?}");
    // A loop that never breaks, and one called from inside another loop.
    let (px, time) = run_splash(&gpu, "var s = 0f\nloop { s += 1f }\nself.pixel = vec4(s, 0.0, 0.0, 1.0)", "");
    assert_eq!(px[0], crate::makepad_script::shader_control::LOOP_GUARD_MAX_ITERS as f32);
    assert!(time < Duration::from_secs(10), "{time:?}");
    let extra = "spin: fn() { var t = 0f\n loop { t += 1f }\n return t }";
    let (px, time) = run_splash(&gpu, &format!("var s = 0f\nfor i in 0..{huge} {{ s += self.spin() }}\nself.pixel = vec4(s, 0.0, 0.0, 1.0)"), extra);
    assert!(px[0] > 1000.0 && px[0] <= budget + 65536.0, "{px:?}");
    assert!(time < Duration::from_secs(10), "{time:?}");
}

#[test]
fn while_with_a_called_condition_reruns_it_each_pass_on_the_gpu() {
    let Some(gpu) = Gpu::new() else { return };
    // The cascade search shape: the condition calls a fn on a var the body
    // reassigns. proj(ci).x = 2 - ci, inside below 0.99: stops at ci = 2.
    let extra = "inside: fn(q: vec3, m: float) -> float {\nif q.x > m { return 0.0 }\nreturn 1.0\n}\n\
                 proj: fn(ci: float) -> vec3 {\nreturn vec3(2.0 - ci + self.u_n, 0.0, 0.0)\n}";
    let body = "var ci = 0.0\nvar q = self.proj(0.0)\n\
                while ci < 3.5 && self.inside(q, 0.99) < 0.5 {\nci = ci + 1.0\nq = self.proj(min(ci, 3.0))\n}\n\
                self.pixel = vec4(ci, q.x, 0.0, 1.0)";
    let (px, _) = run_splash(&gpu, body, extra);
    assert_eq!(px, [2.0, 0.0, 0.0, 1.0]);
    // Nested whiles stepping float vars (the exposure meter's 8x8 grid),
    // and a condition that calls with the var the body steps.
    let body = "var n = 0.0\nvar y = 0.0\nwhile y < 8.0 {\nvar x = 0.0\nwhile x < 8.0 {\nn = n + 1.0\nx = x + 1.0\n}\ny = y + 1.0\n}\n\
                var j = 0.0\nwhile self.inside(self.proj(j), 0.99) < 0.5 && j < 10.0 { j = j + 1.0 }\n\
                self.pixel = vec4(n, j, 0.0, 1.0)";
    let (px, _) = run_splash(&gpu, body, extra);
    assert_eq!(px, [64.0, 2.0, 0.0, 1.0]);
}

#[test]
fn scalar_varyings_after_vector_varyings_interpolate_correctly() {
    let Some(gpu) = Gpu::new() else { return };
    // VJ5's case: a float varying declared after a vec2 one.
    for (extra, vertex, fragment, want) in [
        ("v_uv: shader.varying(vec2f)\nv_f: shader.varying(f32)", "self.v_uv = vec2(0.25, 0.5)\nself.v_f = 0.75", "self.pixel = vec4(self.v_uv.x, self.v_uv.y, self.v_f, 1.0)", [0.25, 0.5, 0.75, 1.0]),
        ("v_f: shader.varying(f32)\nv_uv: shader.varying(vec2f)", "self.v_uv = vec2(0.25, 0.5)\nself.v_f = 0.75", "self.pixel = vec4(self.v_uv.x, self.v_uv.y, self.v_f, 1.0)", [0.25, 0.5, 0.75, 1.0]),
        ("v_a: shader.varying(vec3f)\nv_f: shader.varying(f32)\nv_uv: shader.varying(vec2f)\nv_g: shader.varying(f32)", "self.v_a = vec3(0.1, 0.2, 0.3)\nself.v_uv = vec2(0.25, 0.5)\nself.v_f = 0.75\nself.v_g = 0.125", "self.pixel = vec4(self.v_uv.y, self.v_f, self.v_g, self.v_a.z)", [0.5, 0.75, 0.125, 0.3]),
        // VJ's spelling: `float` (and the vec4 before the vec2 as in the
        // firefly engine).
        ("v_c: shader.varying(vec4f)\nv_uv: shader.varying(vec2f)\nv_f: shader.varying(float)", "self.v_c = vec4(0.1, 0.2, 0.3, 0.4)\nself.v_uv = vec2(0.25, 0.5)\nself.v_f = 0.75", "self.pixel = vec4(self.v_uv.x, self.v_uv.y, self.v_f, self.v_c.w)", [0.25, 0.5, 0.75, 0.4]),
        // Written in branches that return early, read through comparisons
        // (the firefly kit's shape).
        ("v_c: shader.varying(vec4f)\nv_uv: shader.varying(vec2f)\nv_f: shader.varying(float)", "if self.geom.x > 100.0 {\n self.v_uv = vec2(9.0, 9.0)\n self.v_f = 2.0\n self.v_c = vec4(1.0)\n } else {\n self.v_c = vec4(0.1, 0.2, 0.3, 0.4)\n self.v_uv = vec2(0.25, 0.5)\n self.v_f = 1.0\n }\n self.v_c.w = 0.4", "var o = vec4(1.0)\nif self.v_f > 1.5 { o = vec4(0.0) }\nif self.v_f > 0.5 && self.v_f < 1.5 { o = vec4(self.v_uv.x, self.v_uv.y, self.v_f, self.v_c.w) }\nself.pixel = o", [0.25, 0.5, 1.0, 0.4]),
        // The ribbons kit's shape (VJ5): v_color on the base, then float,
        // vec2, float on the kit; the vertex returns its position.
        ("v_color: shader.varying(vec4f)\nbase:\nv_side: shader.varying(float)\nv_cuv: shader.varying(vec2f)\nv_head: shader.varying(float)", "self.v_color = vec4(0.1, 0.2, 0.3, 0.4)\n self.v_head = self.geom.y\n self.v_side = self.geom.x\n self.v_cuv = vec2(0.25, 0.5)\n return self.vertex_pos", "self.pixel = vec4(self.v_side, self.v_head, self.v_cuv.y, self.v_color.w)", [-0.75, 0.75, 0.5, 0.4]),
        // The heightmap kit's shape (VJ5): vec2, float, vec3, vec3, float,
        // after the base's vec4, interpolated.
        ("v_color: shader.varying(vec4f)\nbase:\nv_uv: shader.varying(vec2f)\nv_h: shader.varying(float)\nv_world: shader.varying(vec3f)\nv_n: shader.varying(vec3f)\nv_sun: shader.varying(float)", "self.v_color = vec4(0.1, 0.2, 0.3, 0.4)\n self.v_uv = vec2(self.geom.x, 0.5)\n self.v_h = self.geom.y\n self.v_world = vec3(1.0, 2.0, self.geom.x)\n self.v_n = vec3(0.0, 1.0, 0.0)\n self.v_sun = self.geom.x + self.geom.y\n return self.vertex_pos", "self.pixel = vec4(self.v_h, self.v_sun, self.v_world.z + self.v_n.y, self.v_uv.x)", [0.75, 0.0, 0.25, -0.75]),
    ] {
        let (px, _) = run_splash_with(&gpu, fragment, extra, vertex);
        for (got, want) in px.iter().zip(want) {
            assert!((got - want).abs() < 1e-5, "{extra}: {px:?} != {want:?}");
        }
    }
}

// ---------------------------------------------------------------------------
// MRT.

const MRT_SOURCE: &str = "#include <metal_stdlib>
using namespace metal;
struct V { float4 pos [[position]]; };
vertex V vs(uint vid [[vertex_id]]) {
    float2 p[3] = {float2(-1.0, -1.0), float2(3.0, -1.0), float2(-1.0, 3.0)};
    V v; v.pos = float4(p[vid], 0.0, 1.0); return v;
}
struct Out3 { float4 c0 [[color(0)]]; float2 c1 [[color(1)]]; uint c2 [[color(2)]]; };
fragment Out3 fs_three(V in [[stage_in]]) {
    Out3 o; o.c0 = float4(0.25, 0.5, 0.75, 0.5); o.c1 = float2(-2.5, 0.125); o.c2 = 0xDEADBEEFu; return o;
}
struct Out1 { float4 c0 [[color(0)]]; };
fragment Out1 fs_one(V in [[stage_in]]) { Out1 o; o.c0 = float4(1.0, 0.0, 0.0, 1.0); return o; }
";

#[test]
fn mrt_attachments_get_their_own_formats_blend_and_write_masks() {
    let Some(gpu) = Gpu::new() else { return };
    let formats = [TexturePixel::RGBAf16, TexturePixel::RGf16, TexturePixel::Ru32].map(|p| texture_pixel_to_mtl_pixel(&p));
    assert_eq!(formats, [MTLPixelFormat::RGBA16Float, MTLPixelFormat::RG16Float, MTLPixelFormat::R32Uint]);
    let library = gpu.library(MRT_SOURCE);
    let make = |fragment: &str, written: u8, blend: bool| {
        let descriptor: ObjcId = unsafe { msg_send![class!(MTLRenderPipelineDescriptor), new] };
        unsafe {
            let () = msg_send![descriptor, setVertexFunction: Gpu::function(library, "vs")];
            let () = msg_send![descriptor, setFragmentFunction: Gpu::function(library, fragment)];
            describe_mrt_attachments(descriptor, &formats, written, blend, false);
        }
        gpu.pipeline(descriptor)
    };
    // Blending on: only the RGBA16F attachment blends; the RG16F and
    // R32Uint ones write raw (a blended integer attachment is a pipeline
    // error, a blended RG16F one reads back as zeros).
    let three = make("fs_three", 0b111, true).expect("MRT pipeline with an integer attachment");
    let one = make("fs_one", 0b001, false).expect("one-output pipeline in an MRT pass");
    let targets = formats.map(|f| gpu.target(f, 2));

    // Pass 1: clear c0 to blue, then the three-output shader blends over it.
    gpu.draw(
        &[(targets[0], Some([0.0, 0.0, 1.0, 1.0])), (targets[1], Some([7.0, 7.0, 0.0, 0.0])), (targets[2], Some([0.0; 4]))],
        &[three],
        &[],
    );
    let c0 = Gpu::read(targets[0], 8, 2);
    let c0: Vec<f32> = c0[..8].chunks_exact(2).map(|b| f16_to_f32(u16::from_le_bytes([b[0], b[1]]))).collect();
    assert_eq!(c0, [0.25, 0.5, 1.25, 1.0], "premultiplied over onto the blue clear");
    let c1 = Gpu::read(targets[1], 4, 2);
    let c1: Vec<f32> = c1[..4].chunks_exact(2).map(|b| f16_to_f32(u16::from_le_bytes([b[0], b[1]]))).collect();
    assert_eq!(c1, [-2.5, 0.125]);
    let c2 = Gpu::read(targets[2], 4, 2);
    assert_eq!(u32::from_le_bytes([c2[0], c2[1], c2[2], c2[3]]), 0xDEADBEEF);

    // Pass 2 (load): a one-output shader writes c0 only.
    gpu.draw(&[(targets[0], None), (targets[1], None), (targets[2], None)], &[one], &[]);
    let c0 = Gpu::read(targets[0], 8, 2);
    let c0: Vec<f32> = c0[..8].chunks_exact(2).map(|b| f16_to_f32(u16::from_le_bytes([b[0], b[1]]))).collect();
    assert_eq!(c0, [1.0, 0.0, 0.0, 1.0]);
    let c1 = Gpu::read(targets[1], 4, 2);
    let c1: Vec<f32> = c1[..4].chunks_exact(2).map(|b| f16_to_f32(u16::from_le_bytes([b[0], b[1]]))).collect();
    assert_eq!(c1, [-2.5, 0.125], "unwritten attachment kept");
    let c2 = Gpu::read(targets[2], 4, 2);
    assert_eq!(u32::from_le_bytes([c2[0], c2[1], c2[2], c2[3]]), 0xDEADBEEF, "unwritten attachment kept");
}

// ---------------------------------------------------------------------------
// Instance records read by instance index.

/// A record far over every backend's vertex attributes (40 mat4s, then a
/// vec3, a vec2i, a u32 and a float: 650 words) is read from the instance
/// buffer by `[[instance_id]]` at the word offsets the draw list writes
/// (`DrawShaderInputs`), here the second of two records via baseInstance.
#[test]
fn instance_records_past_every_attribute_limit_read_by_instance_index() {
    let Some(gpu) = Gpu::new() else { return };
    let mut extra = String::new();
    let mut sum = String::new();
    for k in 0..40 {
        extra.push_str(&format!("m{k}: shader.instance(mat4x4f)\n"));
        sum.push_str(&format!("{}(self.m{k} * vec4(0.0, 0.0, 0.0, 1.0)).z", if k == 0 { "" } else { " + " }));
    }
    extra.push_str("p: shader.instance(vec3f)\nn: shader.instance(vec2i)\nu: shader.instance(u32)\nf: shader.instance(0.5)\nv_s: shader.varying(f32)");
    let (source, offsets, stride) = splash_metal_compile(
        "self.pixel = vec4(self.v_s, self.p.z + float(self.n.y), float(self.u), self.f)",
        &extra,
        &format!("self.v_s = {sum}"),
    );

    // The draw list's packing of the same fields agrees.
    let mut inputs = crate::draw_shader::DrawShaderInputs::new(crate::draw_shader::DrawShaderInputPacking::Attribute);
    let pod = |name: LiveId| -> crate::makepad_script::pod::ScriptPodTy {
        use crate::makepad_script::pod::{ScriptPodMat, ScriptPodTy, ScriptPodVec};
        match name {
            n if n == id!(p) => ScriptPodTy::Vec(ScriptPodVec::Vec3f),
            n if n == id!(n) => ScriptPodTy::Vec(ScriptPodVec::Vec2i),
            n if n == id!(u) => ScriptPodTy::U32,
            n if n == id!(f) => ScriptPodTy::F32,
            _ => ScriptPodTy::Mat(ScriptPodMat::Mat4x4f),
        }
    };
    for (name, _) in &offsets {
        crate::draw_shader::CxDrawShaderMapping::push_pod_fields(&mut inputs, &pod(*name), *name);
    }
    inputs.finalize();
    assert_eq!(inputs.total_slots, stride);
    for ((name, offset), input) in offsets.iter().zip(&inputs.inputs) {
        assert_eq!((*name, *offset), (input.id, input.offset));
    }
    assert_eq!(stride, 650);

    // Two records; the second one is drawn.
    let mut words = vec![0u32; stride * 2];
    for record in 0..2usize {
        let w = &mut words[record * stride..];
        let at = |name: LiveId| offsets.iter().find(|(n, _)| *n == name).unwrap().1;
        for k in 0..40 {
            // Column 3, row 2 of each matrix.
            w[at(LiveId::from_str(&format!("m{k}"))) + 14] = ((record as f32 + 1.0) * 0.5).to_bits();
        }
        w[at(id!(p)) + 2] = 3.25f32.to_bits();
        w[at(id!(n)) + 1] = (-7i32) as u32;
        w[at(id!(u))] = 123456 + record as u32;
        w[at(id!(f))] = 0.625f32.to_bits();
    }
    let library = gpu.library(&source);
    let descriptor: ObjcId = unsafe { msg_send![class!(MTLRenderPipelineDescriptor), new] };
    unsafe {
        let () = msg_send![descriptor, setVertexFunction: Gpu::function(library, "vertex_main")];
        let () = msg_send![descriptor, setFragmentFunction: Gpu::function(library, "fragment_main")];
        let attachments: ObjcId = msg_send![descriptor, colorAttachments];
        let a: ObjcId = msg_send![attachments, objectAtIndexedSubscript: 0u64];
        let () = msg_send![a, setPixelFormat: MTLPixelFormat::RGBA32Float];
    }
    let pipeline = gpu.pipeline(descriptor).unwrap_or_else(|e| panic!("{e}\n{source}"));
    let triangle: Vec<u8> = [-1.0f32, -1.0, 3.0, -1.0, -1.0, 3.0].iter().flat_map(|f| f.to_le_bytes()).collect();
    let instance_bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let buffers = [gpu.buffer(&triangle), gpu.buffer(&instance_bytes), gpu.buffer(&vec![0u8; 4096])];
    let target = gpu.target(MTLPixelFormat::RGBA32Float, 4);
    unsafe {
        let pass: ObjcId = msg_send![class!(MTLRenderPassDescriptor), renderPassDescriptor];
        let attachments: ObjcId = msg_send![pass, colorAttachments];
        let a: ObjcId = msg_send![attachments, objectAtIndexedSubscript: 0u64];
        let () = msg_send![a, setTexture: target];
        let () = msg_send![a, setStoreAction: MTLStoreAction::Store];
        let () = msg_send![a, setLoadAction: MTLLoadAction::Clear];
        let cb: ObjcId = msg_send![gpu.queue, commandBuffer];
        let enc: ObjcId = msg_send![cb, renderCommandEncoderWithDescriptor: pass];
        let () = msg_send![enc, setRenderPipelineState: pipeline];
        for (i, b) in buffers.iter().enumerate() {
            let () = msg_send![enc, setVertexBuffer: *b offset: 0u64 atIndex: i as u64];
            let () = msg_send![enc, setFragmentBuffer: *b offset: 0u64 atIndex: i as u64];
        }
        let () = msg_send![enc, drawPrimitives: MTLPrimitiveType::Triangle vertexStart: 0u64 vertexCount: 3u64 instanceCount: 1u64 baseInstance: 1u64];
        let () = msg_send![enc, endEncoding];
        let () = msg_send![cb, commit];
        let () = msg_send![cb, waitUntilCompleted];
    }
    let px = words_f32(&Gpu::read(target, 16, 4));
    assert_eq!([px[0], px[1], px[2], px[3]], [40.0, -3.75, 123457.0, 0.625]);
}
