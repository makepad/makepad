//! Kernels that sample textures, on the native backend with hidden windows: an uploaded image,
//! a storage image an earlier dispatch wrote and a render target an earlier pass cleared, read
//! with nearest and linear samplers into a buffer read back to the CPU; and a kernel whose
//! params are past the 128 bytes of push constants, in a parameter block.
#[cfg(not(target_arch = "wasm32"))]
mod native {
use makepad_hal::{self as hal, *};
use hal::codec::ArenaSink;
use makepad_script::{*, shader_ir_compute::*};
use std::time::{Duration, Instant};

const OUT: Id = Id::new(1, 1);
const IMAGE: Id = Id::new(1, 1);
const HALF: Id = Id::new(2, 1);
const TARGET: Id = Id::new(3, 1);
const WRITE: Id = Id::new(1, 1);
const SAMPLE: Id = Id::new(2, 1);
const BIG: Id = Id::new(3, 1);
/// vec4 params of the parameter-block kernel: 160 bytes (and a buffer address)
const BIG_PARAMS: usize = 10;
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
fn kernel(vm: &mut ScriptVm, body: &str, target: ComputeTarget) -> CompiledKernel {
    let source = format!("use mod.pod.*\nuse mod.math.*\nuse mod.shader\n{{{body}}}");
    let value = vm.eval(ScriptMod::new("", "", "compute_sampled", 0, 0, &source, &[]));
    let errors = vm.take_errors(); assert!(errors.is_empty(), "{errors:?}");
    compile_compute_kernel_result(vm, value.as_object().unwrap(), target).view().to_kernel().unwrap()
}
fn upload_kernel(w: &mut CmdWriter, k: &CompiledKernel, id: Id, dialect: u32) {
    let l = &k.layout;
    let code = if let Some(words) = k.code.as_spirv() { words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>() } else { k.code.as_text().unwrap().as_bytes().to_vec() };
    let buffers: Vec<_> = l.buffers.iter().map(|b| StorageBinding { index: b.index, offset: b.offset, writable: b.writable as u32 }).collect();
    let textures: Vec<_> = l.storage_textures.iter().map(|t| StorageImageBinding { index: t.index, format: format::R16F }).collect();
    let sampled: Vec<_> = l.textures.iter().map(|t| SampledImageBinding { index: t.index }).collect();
    let samplers: Vec<_> = l.samplers.iter().map(|s| ComputeSamplerDesc { filter: s.filter, address: s.address }).collect();
    let blob = CmdWriter::compute_pipeline_blob_sampled(PipelineDesc::default(), &code, ComputeDesc {
        push_size: l.push_size, param_binding: l.param_binding.unwrap_or(u32::MAX), param_binding_size: l.param_binding_size,
        group_count_offset: l.group_count_offset.unwrap_or(u32::MAX), workgroup: l.workgroup, precise_math: l.precise_math as u32, ..Default::default()
    }, &buffers, &textures, &sampled, &samplers);
    w.pipeline_create(id, dialect, Attr::default(), &blob);
}
fn dispatch(w: &mut CmdWriter, params: &[u8], pipeline: Id, buffers: &[Id], textures: &[Id], sampled: &[Id]) -> Dispatch {
    let b = w.put_ids(buffers); let t = w.put_ids(textures); let s = w.put_ids(sampled);
    Dispatch { pipeline, timer: 0, groups: [1, 1, 1], indirect_buffer: Id::NONE, indirect_offset: 0, params: w.put_bytes(params),
        buffers: Range { off: b.off * 8, len: b.len * 8 }, textures: Range { off: t.off * 8, len: t.len * 8 }, sampled: Range { off: s.off * 8, len: s.len * 8 } }
}
pub fn run() {
    let hal = Hal::new();
    let backend = if cfg!(target_vendor = "apple") { backend_id::METAL } else if cfg!(windows) { backend_id::D3D11 } else { backend_id::VULKAN };
    let info = hal.open(&OpenReq { backend, flags: open_flags::OFFSCREEN_WINDOWS, ..Default::default() });
    check("native compute capability", info.backend == backend && info.flags & info_flags::COMPUTE != 0);
    let target = match info.dialect { dialect::MSL => ComputeTarget::Msl, dialect::HLSL => ComputeTarget::Hlsl, _ => ComputeTarget::Spirv };
    let mut host = ScriptVmHost::new(0i32, ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    // the half image: x / 4 at every texel
    let write = kernel(&mut vm, "workgroup_size: 4\nworkgroup_size_y: 4\nhalf: shader.texture_2d_rw(@r16f)\nmain: fn() { let p = global_id().xy\nself.half.write(p, f32(p.x) * 0.25) }", target);
    // thread i reads column i of row 0 from each texture (nearest, then the target linearly),
    // thread 0 also between the image's first two texels (linear)
    let sample = kernel(&mut vm, "workgroup_size: 4\nimage: shader.texture_2d(float)\nhalf: shader.texture_2d(float)\ntarget: shader.texture_2d(float)\nout: shader.buffer_rw(vec4f)\nmain: fn() { let i = global_id().x\nlet uv = vec2((f32(i) + 0.5) / 4.0, 0.125)\nself.out[i * u32(3)] = self.image.sample_nearest(uv)\nself.out[i * u32(3) + u32(1)] = self.half.sample_nearest(uv)\nself.out[i * u32(3) + u32(2)] = self.target.sample_lod(uv, 0.0)\nif i == u32(0) { self.out[12] = self.image.sample_lod(vec2(0.25, 0.125), 0.0) } }", target);
    check("the kernel reflects three sampled textures and two samplers", sample.layout.textures.len() == 3 && sample.layout.samplers.len() == 2);
    // writes its params p<i> = (i, i + 0.5, 100 + i, -i) to out[13 + i]
    let big_source = format!("workgroup_size: 1\nout: shader.buffer_rw(vec4f)\n{}main: fn() {{ {} }}",
        (0..BIG_PARAMS).map(|i| format!("p{i}: shader.uniform(vec4f)\n")).collect::<String>(),
        (0..BIG_PARAMS).map(|i| format!("self.out[{}] = self.p{i}\n", 13 + i)).collect::<String>());
    let big = kernel(&mut vm, &big_source, target);
    check("the kernel's params are past the 128 bytes of push constants, in a parameter block", big.layout.push_size > 128 && big.layout.param_binding.is_some());
    let mut big_params = vec![0u8; big.layout.push_size as usize];
    for i in 0..BIG_PARAMS {
        let slot = big.layout.slot(LiveId::from_str(&format!("p{i}"))).unwrap();
        let value = [i as f32, i as f32 + 0.5, 100.0 + i as f32, -(i as f32)];
        big_params[slot.offset as usize..slot.offset as usize + 16].copy_from_slice(&value.iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<_>>());
    }
    let out_len = (13 + BIG_PARAMS) * 16;
    let mut w = CmdWriter::new(info.session);
    w.buffer_create(OUT, buffer_kind::STORAGE | buffer_kind::READBACK, out_len as u64, Attr::default());
    w.buffer_update(OUT, 0, &vec![0u8; out_len]);
    w.texture_create(IMAGE, usage::SAMPLED, format::RGBA8, 4, 4, [1, 1], Attr::default());
    let pixels: Vec<u8> = (0..16).flat_map(|i| [(i % 4) as u8 * 80, (i / 4) as u8 * 80, 255, 255]).collect();
    w.texture_update(IMAGE, 0, [0, 0, 4, 4], 0, &pixels);
    w.texture_create(HALF, usage::STORAGE | usage::SAMPLED, format::R16F, 4, 4, [1, 1], Attr::default());
    w.texture_create(TARGET, usage::RENDER_TARGET | usage::SAMPLED, format::RGBA8, 4, 4, [1, 1], Attr::default());
    upload_kernel(&mut w, &write, WRITE, info.dialect);
    upload_kernel(&mut w, &sample, SAMPLE, info.dialect);
    upload_kernel(&mut w, &big, BIG, info.dialect);
    hal.submit(&w.buf());
    let mut ready = std::collections::HashSet::new();
    until(&hal, info.session, |e| { if let HalEvent::PipelineReady { pipeline } = e { ready.insert(pipeline); } ready.len() == 3 });
    let mut w = CmdWriter::new(info.session);
    w.str_def(1, b"clear_target");
    w.str_def(2, b"sample");
    // an earlier packet's pass clears the render target the kernel samples
    w.frame_begin(1, FrameMeta::default());
    w.pass(Pass { color: [TARGET, Id::NONE, Id::NONE, Id::NONE], viewport: [0., 0., 4., 4.], load: [pass_load::CLEAR; 5], clear: [[0.25, 0.5, 0.75, 1.0]; 4], name: 1, ..Default::default() });
    w.frame_end(TARGET, target::TEXTURE);
    // one dispatch writes the half image, the next samples it; the last takes its params in a block
    w.frame_begin(2, FrameMeta::default());
    let zeros = |k: &CompiledKernel| vec![0; k.layout.push_size as usize];
    let dispatches = [dispatch(&mut w, &zeros(&write), WRITE, &[], &[HALF], &[]), dispatch(&mut w, &zeros(&sample), SAMPLE, &[OUT], &[], &[IMAGE, HALF, TARGET]),
        dispatch(&mut w, &big_params, BIG, &[OUT], &[], &[])];
    w.compute_pass(2, 2, &dispatches);
    w.frame_end(Id::NONE, target::COMPUTE);
    w.read_buffers(1, &[BufferReadRange { id: OUT, offset: 0, len: out_len as u64 }]);
    hal.submit(&w.buf());
    let mut out = Vec::new();
    until(&hal, info.session, |e| {
        if let HalEvent::BufferLeases { req: 1, status, records } = e {
            use makepad_hal::buffers::Wire;
            assert_eq!(status, readback::OK);
            let record = makepad_hal::lease::Record::read(records);
            out = unsafe { std::slice::from_raw_parts(record.ptr as *const u8, record.len as usize) }.to_vec();
            hal_lease_release(record.token);
            true
        } else { false }
    });
    let v: Vec<[f32; 4]> = out.chunks_exact(16).map(|c| std::array::from_fn(|k| f32::from_le_bytes(c[k * 4..k * 4 + 4].try_into().unwrap()))).collect();
    let near = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 0.01);
    for (i, t) in v.iter().enumerate() { println!("  out[{i}] = {t:?}"); }
    check("an uploaded image sampled nearest", (0..4).all(|i| near(v[i * 3], [i as f32 * 80.0 / 255.0, 0.0, 1.0, 1.0])));
    check("a storage image the dispatch before wrote", (0..4).all(|i| (v[i * 3 + 1][0] - i as f32 * 0.25).abs() < 0.01));
    check("a render target an earlier pass cleared", (0..4).all(|i| near(v[i * 3 + 2], [0.25, 0.5, 0.75, 1.0])));
    check("linear filtering between two texels", near(v[12], [40.0 / 255.0, 0.0, 1.0, 1.0]));
    check("params read from the parameter block", (0..BIG_PARAMS).all(|i| near(v[13 + i], [i as f32, i as f32 + 0.5, 100.0 + i as f32, -(i as f32)])));
    check("no rejected HAL commands", hal.with_world(|w| w.session(info.session).unwrap().stats.bad_args) == 0);
    hal.close(info.session);
}
}
#[cfg(not(target_arch = "wasm32"))]
fn main() { native::run(); }
#[cfg(target_arch = "wasm32")]
fn main() {}
