//! Compute and draw resources through the real compilers: spirv-val with
//! Vulkan 1.2 rules on Linux, Metal on macOS, and D3DCompile on Windows.
//! Each platform requires its native validator; no missing tool is a pass.
use makepad_script::shader_ir_compute::*;
use makepad_script::*;

#[path = "support/native_shader.rs"]
mod native_shader;

#[test]
fn ai_kernel_corpus() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../libs/ai/vulkan/kernels");
    let mut files: Vec<_> = std::fs::read_dir(root).unwrap().map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "splash")).collect();
    files.sort();
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    for file in files {
        let source = std::fs::read_to_string(&file).unwrap();
        let code = format!("use mod.pod.*\nuse mod.math.*\nuse mod.shader\n{source}");
        let value = vm.with_instruction_limit(50_000_000, |vm| vm.eval(ScriptMod::new("", "", "corpus", 0, 0, &code, &[])));
        assert!(vm.take_errors().is_empty(), "{}", file.display());
        for field in vm.bx.heap.map_entries_ordered(value.as_object().unwrap()) {
            let name = format!("{}-{}", file.file_stem().unwrap().to_str().unwrap(), field.key.as_id().unwrap());
            let result = compile_compute_kernel(&mut vm, field.value.as_object().unwrap(), ComputeTarget::Spirv);
            let errors = vm.take_errors();
            if name.contains("-reject_") {
                assert!(result.is_err() || !errors.is_empty(), "{name}");
                continue;
            }
            assert!(errors.is_empty(), "{name}: {errors:?}");
            let k = result.unwrap_or_else(|e| panic!("{name}: {e}"));
            let bytes: Vec<u8> = k.code.as_spirv().unwrap().iter().flat_map(|w| w.to_le_bytes()).collect();
            for (variable, compare) in [("MAKEPAD_CORPUS_DUMP", false), ("MAKEPAD_CORPUS_BASE", true)] {
                if let Ok(dir) = std::env::var(variable) {
                    let path = std::path::Path::new(&dir).join(format!("{name}.spv"));
                    if compare {
                        assert_eq!(bytes, std::fs::read(path).unwrap(), "SPIR-V changed: {name}");
                    } else {
                        std::fs::create_dir_all(dir).unwrap();
                        std::fs::write(path, &bytes).unwrap();
                    }
                }
            }
            validate(&k);
            for target in [ComputeTarget::Msl, ComputeTarget::Hlsl] {
                let text = compile_compute_kernel(&mut vm, field.value.as_object().unwrap(), target);
                assert!(vm.take_errors().is_empty(), "{name} {target:?}");
                if !k.layout.cooperative_matrices.is_empty() {
                    assert!(text.unwrap_err().contains("cooperative matrices"), "{name}");
                    continue;
                }
                if let Err(e) = &text {
                    assert!(target == ComputeTarget::Hlsl && e.contains("subgroup ops need shader model 6"), "{name} {target:?}: {e}");
                    continue;
                }
                let text = text.unwrap();
                validate_text(&text, &name);
            }
            println!("validated {name}");
        }
    }
}

fn validate_text(k: &CompiledKernel, name: &str) {
    let text = k.code.as_text().unwrap();
    assert!(!text.contains("compute_kernel_builtin_unsupported"), "{name}");
    #[cfg(target_os = "macos")]
    if k.layout.target == ComputeTarget::Msl { native_shader::metal(text, k.layout.precise_math).unwrap_or_else(|e| panic!("{name}: {e}")); }
    #[cfg(target_os = "windows")]
    if k.layout.target == ComputeTarget::Hlsl { native_shader::hlsl(text, "kernel_main", "cs_5_0", k.layout.precise_math).unwrap_or_else(|e| panic!("{name}: {e}")); }
    if let Ok(dir) = std::env::var("MAKEPAD_TEXT_DUMP") {
        std::fs::create_dir_all(&dir).unwrap();
        let ext = if k.layout.target == ComputeTarget::Msl { "metal" } else { "hlsl" };
        std::fs::write(std::path::Path::new(&dir).join(format!("{name}.{ext}")), text).unwrap();
    }
}

fn kernel(body: &str) -> Result<CompiledKernel, String> {
    kernel_target(body, ComputeTarget::Spirv)
}

fn kernel_target(body: &str, target: ComputeTarget) -> Result<CompiledKernel, String> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let code = format!("use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet k = {{\n{body}\n}}\nk");
    let value = vm.with_instruction_limit(50_000_000, |vm| vm.eval(ScriptMod::new("", "", "x", 0, 0, &code, &[])));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{errors:?}");
    let obj = value.as_object().expect("kernel object");
    let r = compile_compute_kernel_result(&mut vm, obj, target).view().to_kernel();
    let errors = vm.take_errors();
    if !errors.is_empty() {
        return Err(format!("{errors:?}"));
    }
    r
}

fn validate(k: &CompiledKernel) {
    validate_spirv(k.code.as_spirv().unwrap());
}

fn validate_spirv(words: &[u32]) {
    let mut bytes = Vec::new();
    for w in words {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!("makepad-kernel-{}-{serial}.spv", std::process::id()));
    std::fs::write(&path, &bytes).unwrap();
    // MAKEPAD_KERNEL_DUMP=dir keeps the modules for spirv-dis.
    if let Ok(dir) = std::env::var("MAKEPAD_KERNEL_DUMP") {
        std::fs::write(std::path::Path::new(&dir).join(format!("kernel-{serial}.spv")), &bytes).unwrap();
    }
    let out = std::process::Command::new("spirv-val").arg("--target-env").arg("vulkan1.2").arg(&path).output();
    if cfg!(target_os = "linux") || out.is_ok() {
        let o = out.expect("spirv-val unavailable: required Linux validation did not run");
        assert!(o.status.success(), "spirv-val: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    }
    std::fs::remove_file(path).unwrap();
}

#[test]
fn text_targets_resources_params_and_precise() {
    for target in [ComputeTarget::Spirv, ComputeTarget::Msl, ComputeTarget::Hlsl] {
        for (scalar, vector) in [("f32", "f"), ("u32", "u"), ("i32", "i")] {
            for lanes in 1..=4 {
                let ty = if lanes == 1 { scalar.into() } else { format!("vec{lanes}{vector}") };
                let body = format!("workgroup_size: 32\nprecise_math: true\nsrc: shader.buffer({ty})\ndst: shader.buffer_rw({ty})\np1: shader.uniform({scalar})\np2: shader.uniform(vec2{vector})\np3: shader.uniform(vec3{vector})\np4: shader.uniform(vec4{vector})\nmain: fn() {{ let i = global_id().x\n self.dst[i] = self.src[i] + {ty}(self.p1 + self.p2.y + self.p3.z + self.p4.w) }}");
                let body = if lanes >= 2 { format!("{}\n self.dst[i].xy += vec2{vector}(self.p1) }}", body.strip_suffix('}').unwrap()) } else { body };
                let k = kernel_target(&body, target).unwrap();
                assert!(k.layout.precise_math);
                assert_eq!(k.layout.buffers[0].stride, if lanes == 3 { 16 } else { lanes * 4 });
                if target == ComputeTarget::Spirv { validate(&k); } else { validate_text(&k, &format!("params-{ty}")); }
            }
        }
        let k = kernel_target("workgroup_size: 32\n count: shader.buffer_rw(u32)\n temp: shader.workgroup(u32, 32)\n main: fn() { let i = local_index()\n self.temp[i] = atomic_add(self.count[0], u32(1))\n barrier()\n var sum = u32(0)\n for j in 0..32 { sum += self.temp[j] }\n self.count[global_id().x] = sum + group_count().x + group_id().x + local_id().x }", target).unwrap();
        assert_eq!(k.layout.shared_bytes, 128);
        if target == ComputeTarget::Hlsl { assert_eq!(k.layout.group_count_offset, Some(16)); }
        if target == ComputeTarget::Spirv { validate(&k); } else { validate_text(&k, "atomic-barrier"); }
        let k = kernel_target("precise_math: true\n a: shader.texture_2d_rw(@r16f)\n b: shader.texture_2d_rw(@r8unorm)\n c: shader.texture_2d_rw(@r32f)\n d: shader.texture_2d_rw(@r32uint)\n e: shader.texture_2d_rw(@rgba8unorm)\n f: shader.texture_2d_rw(@rgba16f)\n main: fn() { let p = global_id().xy\n self.a.write(p, f32(0.5))\n self.b.write(p, f32(0.25))\n self.c.write(p, f32(1.0) / sqrt(f32(2.0)))\n self.d.write(p, u32(9))\n self.e.write(p, vec4f(1.0))\n self.f.write(p, vec4f(0.5)) }", target).unwrap();
        assert_eq!(k.layout.storage_textures.len(), 6);
        if target == ComputeTarget::Spirv { validate(&k); } else { validate_text(&k, "storage-textures"); }
    }
}

#[test]
fn invalid_kernel_resources_are_rejected() {
    for target in [ComputeTarget::Spirv, ComputeTarget::Msl, ComputeTarget::Hlsl] {
        for body in [
            "a: shader.texture_2d_rw(@r16f)\n main: fn() { let x = self.a.read(vec2u(0)) }",
            "a: shader.texture_2d_rw(@r16f)\n main: fn() { self.a.write(vec2u(0), vec4f(1.0)) }",
            "a: shader.buffer(u32)\n main: fn() { let x = atomic_add(self.a[0], u32(1)) }",
            "a: shader.buffer(vec4f)\n main: fn() { self.a[0].x = 1.0 }",
            "a: shader.buffer(vec4f)\n main: fn() { self.a[0].xy += vec2f(1.0) }",
        ] { assert!(kernel_target(body, target).is_err()); }
    }
    let mut body = String::new();
    for i in 0..8 { body.push_str(&format!("b{i}: shader.buffer_rw(u32)\n")); }
    body.push_str("tex: shader.texture_2d_rw(@r32uint)\n main: fn() {\n");
    for i in 0..8 { body.push_str(&format!("self.b{i}[0] = u32(1)\n")); }
    body.push_str("self.tex.write(vec2u(0), u32(1)) }");
    assert!(kernel_target(&body, ComputeTarget::Hlsl).unwrap_err().contains("8 compute UAVs"));
    let k = kernel_target(&body, ComputeTarget::Msl).unwrap();
    assert_eq!(k.layout.param_binding, Some(8));
    assert_eq!(k.layout.storage_textures[0].index, 0);
    validate_text(&k, "separate-metal-bindings");
}

#[test]
fn draw_storage_buffers_both_stages() {
    use makepad_script::shader_backend::ShaderBackend;
    for (backend, vulkan) in [(ShaderBackend::Metal, false), (ShaderBackend::Hlsl, false), (ShaderBackend::Glsl, true), (ShaderBackend::Glsl, false), (ShaderBackend::Wgsl, false)] {
        for count in [2, 17] {
            let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
            let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
            vm.capture_errors();
            let extra_fields: String = (2..count).map(|i| format!("c{i}: shader.buffer(f32)\n")).collect();
            let extra_reads: String = (2..count).map(|i| format!(" + vec4f(self.c{i}[0])")).collect();
            let source = format!("use mod.pod.*\nuse mod.math.*\nuse mod.shader\n{{ geom: shader.vertex_buffer(vec2f, nil)\n vertex_pos: shader.vertex_position(vec4f)\n pixel: shader.fragment_output(0, vec4f)\n a: shader.buffer(vec4f)\n b: shader.buffer(u32)\n tex: shader.texture_2d(f32)\n {extra_fields}vertex: fn() {{ self.vertex_pos = vec4f(self.geom, 0.0, 1.0) + self.a[self.b[0]] {extra_reads} }}\n fragment: fn() {{ self.pixel = (self.a[self.b[1]] {extra_reads}) * self.tex.sample(vec2f(0.5)) }} }}");
            let obj = vm.eval(ScriptMod::new("", "", "draw_buffers", 0, 0, &source, &[])).as_object().unwrap();
            assert!(vm.take_errors().is_empty());
            let opts = shader_ir_draw::IrCompileOptions::new(backend, false, vulkan, false, false);
            let output = shader_ir_draw::compile_draw_shader_ir_with(&mut vm, obj, &opts);
            if !vulkan && matches!(backend, ShaderBackend::Glsl | ShaderBackend::Wgsl) {
                assert!(output.errors.iter().any(|e| e.contains("storage buffers in draw shaders need Metal, Vulkan or D3D11")));
                continue;
            }
            if vulkan && count > 16 {
                assert!(output.errors.iter().any(|e| e.contains("128 every Vulkan device has")));
                continue;
            }
            assert!(!output.has_errors, "{:?}", output.errors);
            assert!(vm.take_errors().is_empty());
            assert_eq!(output.storage_buffers.len(), count);
            assert_eq!(output.storage_buffers[0].name, makepad_live_id::id!(a));
            if vulkan {
                let compiled = makepad_shader::shader_ir_draw::draw_spirv(&output, &vm).unwrap();
                assert_eq!(output.storage_push_size, 16);
                for compiled in compiled { validate_spirv(&compiled.vertex); validate_spirv(&compiled.fragment); }
            } else if backend == ShaderBackend::Metal {
                let (source, info) = makepad_shader::shader_ir_metal::metal_draw(&output, &vm, true);
                assert_eq!(info.storage_buffers[0].index, 4);
                assert!(source.contains("const device float4 *kernel_a [[buffer(4)]]"));
                #[cfg(target_os = "macos")]
                native_shader::metal(&source, false).unwrap();
            } else {
                let (source, info) = makepad_shader::shader_ir_hlsl::hlsl_draw(&output, &vm, true);
                assert_eq!(info.storage_buffers[0].index, 3);
                assert!(source.contains("ByteAddressBuffer kernel_a : register(t3)"));
                #[cfg(target_os = "windows")]
                for (entry, target) in [("vertex_main", "vs_5_0"), ("pixel_main", "ps_5_0")] { native_shader::hlsl(&source, entry, target, false).unwrap(); }
            }
        }
    }
}

#[test]
fn scale_kernel_compiles_and_validates() {
    let k = kernel(
        "workgroup_size: 64\n\
         src: shader.buffer(f32)\n\
         dst: shader.buffer_rw(f32)\n\
         n: shader.uniform(u32)\n\
         scale: shader.uniform(f32)\n\
         main: fn() {\n\
             let i = global_id().x\n\
             if i < self.n { self.dst[i] = self.src[i] * self.scale }\n\
         }",
    )
    .unwrap();
    assert_eq!(k.layout.workgroup, [64, 1, 1]);
    let names: Vec<String> = k.layout.slots.iter().map(|s| format!("{}@{}", s.name, s.offset)).collect();
    assert_eq!(names, ["dst@0", "src@8", "n@16", "scale@20"].map(String::from).to_vec(), "{:?}", k.layout);
    assert_eq!(k.layout.push_size, 24);
    validate(&k);
}

#[test]
fn row_sum_with_workgroup_memory_and_subgroups_validates() {
    let k = kernel(
        "workgroup_size: 128\n\
         x: shader.buffer(f32)\n\
         y: shader.buffer_rw(f32)\n\
         cols: shader.uniform(u32)\n\
         partial: shader.workgroup(f32, 32)\n\
         main: fn() {\n\
             let row = group_id().x\n\
             let t = local_id().x\n\
             var acc = 0.0\n\
             var c = t\n\
             while c < self.cols {\n\
                 acc += self.x[row * self.cols + c]\n\
                 c += self.workgroup_size\n\
             }\n\
             acc = subgroup_add(acc)\n\
             if subgroup_lane() == 0 { self.partial[subgroup_id()] = acc }\n\
             barrier()\n\
             if t == 0 {\n\
                 var s = 0.0\n\
                 for k in 0..self.workgroup_size / subgroup_size() { s += self.partial[k] }\n\
                 self.y[row] = s\n\
             }\n\
         }",
    )
    .unwrap();
    validate(&k);
}

#[test]
fn packed_words_unpack_and_dot() {
    let k = kernel(
        "workgroup_size: 32\n\
         q: shader.buffer(u32)\n\
         h: shader.buffer(u32)\n\
         out: shader.buffer_rw(f32)\n\
         main: fn() {\n\
             let i = global_id().x\n\
             let w = self.q[i]\n\
             let lo = extract_bits(w, 0, 4)\n\
             let d = unpack_half2(self.h[i]).x\n\
             self.out[i] = d * float(lo) + float(dot4_u8(w, w))\n\
         }",
    )
    .unwrap();
    validate(&k);
}

#[test]
fn writing_a_read_only_buffer_is_refused() {
    let r = kernel(
        "src: shader.buffer(f32)\n\
         main: fn() { self.src[0] = 1.0 }",
    );
    assert!(r.is_err(), "{r:?}");
}

#[test]
fn cooperative_matrices_validate_and_reject_invalid_operands() {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let source = include_str!("../../../libs/ai/vulkan/kernels/cooperative_test.splash");
    let value = vm.eval(ScriptMod::new("", "", "cooperative_test.splash", 0, 0, source, &[]));
    assert!(vm.take_errors().is_empty());
    for field in vm.bx.heap.map_entries_ordered(value.as_object().unwrap()) {
        let name = format!("{}", field.key.as_id().unwrap());
        let compiled = compile_compute_kernel(&mut vm, field.value.as_object().unwrap(), ComputeTarget::Spirv);
        let errors = vm.take_errors();
        if name.starts_with("reject_") {
            assert!(compiled.is_err() || !errors.is_empty(), "{name} compiled");
        } else {
            assert!(errors.is_empty(), "{name}: {errors:?}");
            let k = compiled.unwrap_or_else(|e| panic!("{name}: {e}"));
            validate(&k);
            assert!(!k.layout.cooperative_matrices.is_empty());
            if name == "float_matrices" {
                let mut output = shader_output::ShaderOutput::default();
                output.mode = shader::ShaderMode::Vertex;
                let obj = field.value.as_object().unwrap();
                let main = vm.bx.heap.object_method(obj, makepad_live_id::id!(main).into(), NoTrap).as_object().unwrap();
                shader_lower::IrFnCompiler::compile_shader_def(&mut vm, &mut output, NoTrap, makepad_live_id::id!(main), main, shader::ShaderType::IoSelf(obj), vec![]);
                assert!(output.has_errors && output.compiled.errors.iter().any(|e| e.contains("cooperative matrices exist only in compute kernels")));
                vm.take_errors();
            }
        }
    }
}
