//! Real GPU compute/draw interoperability: run on the native backend with hidden windows.
#[cfg(not(target_arch = "wasm32"))]
mod native {
use makepad_hal::{self as hal, *};
use hal::codec::ArenaSink;
use makepad_script::{*, shader_ir_compute::*};
use std::time::{Duration, Instant};

const DATA: Id = Id::new(1, 1);
const RESULT: Id = Id::new(2, 1);
const ARGS: Id = Id::new(3, 1);
const VB: Id = Id::new(4, 1);
const IB: Id = Id::new(5, 1);
const HALF: Id = Id::new(1, 1);
const BYTE: Id = Id::new(2, 1);
const TARGET: Id = Id::new(3, 1);
const DRAW: Id = Id::new(4, 1);
fn check(name: &str, ok: bool) { println!("{} {name}", if ok { "PASS" } else { "FAIL" }); assert!(ok, "{name}"); }
fn until(hal: &Hal, sid: u32, mut done: impl FnMut(HalEvent<'_>) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(90);
    let mut events = EventStore::new(sid, 256, 1024 * 1024);
    loop {
        assert!(Instant::now() < deadline, "GPU completion deadline");
        let mut out = events.buf(); let count = hal.pump(10_000_000, &mut out);
        for (_, event) in events.reader(count, out.arena_len) {
            match &event {
                HalEvent::PipelineFailed { log, .. } => panic!("pipeline: {}", String::from_utf8_lossy(log)),
                HalEvent::DeviceLost { .. } => panic!("device lost"), _ => {}
            }
            if done(event) { return; }
        }
    }
}
fn object(vm: &mut ScriptVm, body: &str) -> ScriptObject {
    let source = format!("use mod.pod.*\nuse mod.math.*\nuse mod.shader\n{{{body}}}");
    let value = vm.eval(ScriptMod::new("", "", "compute_smoke", 0, 0, &source, &[]));
    let errors = vm.take_errors(); assert!(errors.is_empty(), "{errors:?}");
    value.as_object().unwrap()
}
fn kernel(vm: &mut ScriptVm, body: &str, target: ComputeTarget) -> CompiledKernel {
    let obj = object(vm, body);
    compile_compute_kernel_result(vm, obj, target).view().to_kernel().unwrap()
}
fn image_format(f: StorageTextureFormat) -> u32 {
    match f { StorageTextureFormat::R16f => format::R16F, StorageTextureFormat::R8unorm => format::R8, _ => panic!("unexpected image") }
}
fn upload_kernel(w: &mut CmdWriter, k: &CompiledKernel, id: Id, dialect: u32) {
    let l = &k.layout;
    let code = if let Some(words) = k.code.as_spirv() { words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>() } else { k.code.as_text().unwrap().as_bytes().to_vec() };
    let buffers: Vec<_> = l.buffers.iter().map(|b| StorageBinding { index: b.index, offset: b.offset, writable: b.writable as u32 }).collect();
    let textures: Vec<_> = l.storage_textures.iter().map(|t| StorageImageBinding { index: t.index, format: image_format(t.format) }).collect();
    let blob = CmdWriter::compute_pipeline_blob(PipelineDesc::default(), &code, ComputeDesc {
        push_size: l.push_size, param_binding: l.param_binding.unwrap_or(u32::MAX), param_binding_size: l.param_binding_size,
        group_count_offset: l.group_count_offset.unwrap_or(u32::MAX), workgroup: l.workgroup, precise_math: l.precise_math as u32, ..Default::default()
    }, &buffers, &textures);
    w.pipeline_create(id, dialect, Attr::default(), &blob);
}
fn dispatch(w: &mut CmdWriter, k: &CompiledKernel, pipeline: Id, groups: [u32; 3], indirect: bool) -> Dispatch {
    let buffers: Vec<_> = k.layout.buffers.iter().map(|b| match b.name.to_string().as_str() { "data" => DATA, "result" => RESULT, "args" => ARGS, n => panic!("buffer {n}") }).collect();
    let textures: Vec<_> = k.layout.storage_textures.iter().map(|b| if b.name.to_string() == "half_image" { HALF } else { BYTE }).collect();
    let b = w.put_ids(&buffers); let t = w.put_ids(&textures);
    let mut params = vec![0; k.layout.push_size as usize];
    for slot in &k.layout.slots {
        if slot.name.to_string() == "base" {
            params[slot.offset as usize..slot.offset as usize + 4].copy_from_slice(&10u32.to_le_bytes());
        }
    }
    Dispatch { pipeline, timer: 0, groups, indirect_buffer: if indirect { ARGS } else { Id::NONE }, indirect_offset: 0,
        params: w.put_bytes(&params), buffers: Range { off: b.off * 8, len: b.len * 8 }, textures: Range { off: t.off * 8, len: t.len * 8 } }
}
fn draw_pipeline(vm: &mut ScriptVm, dialect: u32) -> (Vec<u8>, Vec<Id>, Vec<Id>) {
    use makepad_script::shader_backend::ShaderBackend;
    let obj = object(vm, "geom: shader.vertex_buffer(vec2f, nil)\nvertex_pos: shader.vertex_position(vec4f)\npixel: shader.fragment_output(0, vec4f)\nuv: shader.varying(vec2f)\ndata: shader.buffer(u32)\nresult: shader.buffer(u32)\nhalf_image: shader.texture_2d(f32)\nbyte_image: shader.texture_2d(f32)\nvertex: fn() { self.vertex_pos = vec4f(self.geom + vec2f(f32(self.data[0]) - 16.0), 0.0, 1.0)\n self.uv = self.geom * 0.5 + vec2f(0.5) }\nfragment: fn() { self.pixel = vec4f(self.half_image.sample(self.uv).x, self.byte_image.sample(self.uv).x, f32(self.result[0]) / 280.0, 1.0) }");
    let backend = match dialect { dialect::MSL => ShaderBackend::Metal, dialect::HLSL => ShaderBackend::Hlsl, _ => ShaderBackend::Glsl };
    let opts = shader_compile::ShaderCompileOptions::new(backend, false, dialect == dialect::SPIRV, false, false);
    let output = shader_compile::compile_shader(vm, obj, &opts);
    let view = output.view(); assert!(!view.has_errors, "{}", view.error_report);
    let reflection = shader_reflection::DrawShaderDesc::from_output(vm.heap(), &output);
    let textures = reflection.io().iter().filter(|io| io.kind().as_texture().is_some()).map(|io| match io.name().to_string().as_str() {
        "half_image" => HALF, "byte_image" => BYTE, name => panic!("unexpected draw texture {name}"),
    }).collect();
    let sampler = spirv::SpirvSampler { filter: 0, address: 1, compare: false, pixel: false };
    let code = if dialect == dialect::SPIRV {
        let spirv = view.spirv.unwrap().view(); let stage = &spirv.stages[0];
        spirv::SpirvPipeline { vertex: stage.vertex.to_vec(), fragment: stage.fragment.to_vec(), xr_vertex: vec![], xr_fragment: vec![],
            texture_base: stage.info.texture_binding_base(), sampler_base: stage.info.sampler_binding_base(), depth_binding: stage.info.xr_depth_binding(),
            instance_binding: stage.info.instance_binding().unwrap_or(u32::MAX), pick_binding: u32::MAX, pick_texture_base: u32::MAX,
            textures: vec![0, 0], samplers: vec![sampler] }.encode()
    } else if dialect == dialect::HLSL { hlsl::HlslPipeline { source: view.combined.into(), textures: vec![0, 0], samplers: vec![sampler] }.encode() }
    else { view.combined.as_bytes().to_vec() };
    let storage: Vec<_> = view.storage_buffers.iter().map(|b| StorageBinding { index: b.index, offset: b.offset, writable: 0 }).collect();
    let ids = view.storage_buffers.iter().map(|b| if b.name.to_string() == "data" { DATA } else { RESULT }).collect();
    let blob = CmdWriter::draw_pipeline_blob(PipelineDesc { storage_push_size: view.storage_push_size, outputs: 1, color_formats: [format::RGBA8, 0, 0, 0], blend: pipeline_blend::RAW, ..Default::default() }, &code,
        &[u32::MAX, u32::MAX, u32::MAX, u32::MAX, u32::MAX, 2], &[8, 0, 1, 2], &storage);
    (blob, ids, textures)
}
pub fn run() {
    let hal = Hal::new();
    let backend = if cfg!(target_vendor = "apple") { backend_id::METAL } else if cfg!(windows) { backend_id::D3D11 } else { backend_id::VULKAN };
    let info = hal.open(&OpenReq { backend, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    check("native compute capability", info.backend == backend && info.flags & info_flags::COMPUTE != 0);
    check("R16F and R8 storage capability", info.storage_formats & ((1 << format::R16F) | (1 << format::R8)) == (1 << format::R16F) | (1 << format::R8));
    let target = match info.dialect { dialect::MSL => ComputeTarget::Msl, dialect::HLSL => ComputeTarget::Hlsl, _ => ComputeTarget::Spirv };
    let mut host = ScriptVmHost::new(0i32, ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    let fill = kernel(&mut vm, "precise_math: true\nworkgroup_size: 4\nbase: shader.uniform(u32)\ndata: shader.buffer_rw(u32)\nhalf_image: shader.texture_2d_rw(@r16f)\nbyte_image: shader.texture_2d_rw(@r8unorm)\nmain: fn() { let i = global_id().x\nlet old = atomic_add(self.data[0], u32(1))\nself.data[i + u32(1)] = i + self.base\nif i != u32(0) { let p = vec2u(i % u32(4), i / u32(4))\nself.half_image.write(p, f32(1.0))\nself.byte_image.write(p, f32(0.0)) } }", target);
    let reduce = kernel(&mut vm, "data: shader.buffer(u32)\nresult: shader.buffer_rw(u32)\nargs: shader.buffer_rw(u32)\nmain: fn() { var sum = u32(0)\nfor i in 1..17 { sum += self.data[i] }\nself.result[0] = sum\nself.args[0] = u32(2)\nself.args[1] = u32(1)\nself.args[2] = u32(1)\nself.args[3] = u32(3)\nself.args[4] = u32(1)\nself.args[5] = u32(0)\nself.args[6] = u32(0)\nself.args[7] = u32(0) }", target);
    let indirect = kernel(&mut vm, "workgroup_size: 4\nresult: shader.buffer_rw(u32)\nmain: fn() { self.result[global_id().x + u32(1)] = global_id().x + group_count().x }", target);
    let (draw_blob, storage, sampled) = draw_pipeline(&mut vm, info.dialect);
    let mut w = CmdWriter::new(info.session);
    for (id, size, flags) in [(DATA, 68, 0), (RESULT, 36, 0), (ARGS, 32, buffer_kind::INDIRECT)] {
        w.buffer_create(id, buffer_kind::STORAGE | buffer_kind::READBACK | flags, size, Attr::default());
    }
    let zero = [0u8; 68]; w.buffer_update(DATA, 0, &zero);
    for (id, format) in [(HALF, format::R16F), (BYTE, format::R8)] { w.texture_create(id, usage::STORAGE | usage::SAMPLED, format, 4, 4, [1, 1], Attr::default()); }
    w.texture_create(TARGET, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 4, 4, [1, 1], Attr::default());
    w.texture_update(HALF, 0, [0, 0, 4, 4], 0, &[0; 32]);
    w.texture_update(BYTE, 0, [0, 0, 4, 4], 0, &[255; 16]);
    w.texture_update(HALF, 0, [0, 0, 1, 1], 0, &[0, 60]);
    w.texture_update(BYTE, 0, [0, 0, 1, 1], 0, &[0]);
    let vertices: Vec<u8> = [-1.0f32, -1.0, 3.0, -1.0, -1.0, 3.0].into_iter().flat_map(f32::to_le_bytes).collect();
    w.buffer_create(VB, buffer_kind::VERTEX, vertices.len() as u64, Attr::default()); w.buffer_update(VB, 0, &vertices);
    w.buffer_create(IB, buffer_kind::INDEX, 6, Attr::default()); w.buffer_update(IB, 0, &[0, 0, 1, 0, 2, 0]);
    for (i, k) in [&fill, &reduce, &indirect].into_iter().enumerate() { upload_kernel(&mut w, k, Id::new(i as u32 + 1, 1), info.dialect); }
    w.pipeline_create(DRAW, info.dialect, Attr::default(), &draw_blob);
    hal.submit(&w.buf());
    let mut ready = std::collections::HashSet::new();
    until(&hal, info.session, |e| { if let HalEvent::PipelineReady { pipeline } = e { ready.insert(pipeline); } ready.len() == 4 });
    let mut w = CmdWriter::new(info.session);
    w.str_def(1, b"compute_fill");
    w.str_def(2, b"compute_reduce");
    w.frame_begin(1, FrameMeta::default());
    let first = dispatch(&mut w, &fill, Id::new(1, 1), [4, 1, 1], false);
    w.compute_pass(1, 1, &[first]);
    w.frame_end(Id::NONE, target::COMPUTE);
    w.frame_begin(2, FrameMeta::default());
    let dispatches = [dispatch(&mut w, &reduce, Id::new(2, 1), [1, 1, 1], false), dispatch(&mut w, &indirect, Id::new(3, 1), [0; 3], true)];
    w.compute_pass(2, 2, &dispatches);
    w.pass(Pass { color: [TARGET, Id::NONE, Id::NONE, Id::NONE], viewport: [0., 0., 4., 4.], load: [pass_load::CLEAR; 5], ..Default::default() });
    let buffers = w.put_ids(&storage); let textures = w.put_ids(&sampled);
    w.draw(Draw { pipeline: DRAW, vb: VB, ib: IB, index: [0, 3], inst: Inst::inline(Slice::EMPTY, 1), scissor: [f32::NAN; 4], storage_buffers: buffers, tex: textures, indirect_buffer: ARGS, indirect_offset: 12, ..Default::default() });
    w.frame_end(TARGET, target::TEXTURE);
    w.buffer_readback(DATA, 1, 0, 68); w.buffer_readback(RESULT, 2, 0, 36); w.texture_readback(TARGET, 3, [0, 0, 4, 4]);
    let submitted = Instant::now();
    hal.submit(&w.buf());
    println!("CPU encode+submit three dispatches and one draw: {} us", submitted.elapsed().as_micros());
    let mut read = std::collections::HashMap::new();
    let mut times = std::collections::HashMap::new();
    until(&hal, info.session, |e| {
        match e {
            HalEvent::Readback { req, status, pixels, .. } => { assert_eq!(status, readback::OK); read.insert(req, pixels.to_vec()); }
            HalEvent::FrameDone { frame, gpu_ns, .. } => { times.insert(frame, gpu_ns); }
            _ => {}
        }
        read.len() == 3 && times.len() == 2
    });
    println!("GPU work plus async completion delivery: {} us", submitted.elapsed().as_micros());
    let words = |bytes: &[u8]| bytes.chunks_exact(4).map(|b| u32::from_le_bytes(b.try_into().unwrap())).collect::<Vec<_>>();
    check("atomic counter and values", words(&read[&1]) == std::iter::once(16).chain(10..26).collect::<Vec<_>>());
    check("dispatch visibility and indirect group counts", words(&read[&2]) == std::iter::once(280).chain(2..10).collect::<Vec<_>>());
    check("compute-only packet completion and GPU timestamp", times[&1] > 0 && times[&2] > 0);
    if read[&3] != [255, 0, 255, 255].repeat(16) { eprintln!("render target bytes: {:?}", read[&3]); }
    check("storage-buffer draw, R16F/R8 sampling, CPU subrect and indirect draw", read[&3] == [255, 0, 255, 255].repeat(16));
    check("no rejected HAL commands", hal.with_world(|w| w.session(info.session).unwrap().stats.bad_args) == 0);
    hal.close(info.session);
}
}
#[cfg(not(target_arch = "wasm32"))]
fn main() { native::run(); }
#[cfg(target_arch = "wasm32")]
fn main() {}
