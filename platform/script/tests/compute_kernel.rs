//! Splash compute kernels compile to SPIR-V that validates (spirv-val with
//! Vulkan 1.2 rules, when the tool is installed), with the push-constant
//! layout their host binds them by.
use makepad_script::shader_ir_compute::*;
use makepad_script::*;

fn kernel(body: &str) -> Result<CompiledKernel, String> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let code = format!("use mod.pod.*\nuse mod.math.*\nuse mod.shader\nlet k = {{\n{body}\n}}\nk");
    let value = vm.with_instruction_limit(50_000_000, |vm| vm.eval(ScriptMod::new("", "", "x", 0, 0, &code, &[])));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{errors:?}");
    let obj = value.as_object().expect("kernel object");
    let r = compile_compute_kernel(&mut vm, obj);
    let errors = vm.take_errors();
    if !errors.is_empty() {
        return Err(format!("{errors:?}"));
    }
    r
}

fn validate(k: &CompiledKernel) {
    let mut bytes = Vec::new();
    for w in &k.spirv {
        bytes.extend_from_slice(&w.to_le_bytes());
    }
    let path = std::env::temp_dir().join(format!("makepad-kernel-{}-{}.spv", std::process::id(), k.spirv.len()));
    std::fs::write(&path, &bytes).unwrap();
    // MAKEPAD_KERNEL_DUMP=dir keeps the modules for spirv-dis.
    if let Ok(dir) = std::env::var("MAKEPAD_KERNEL_DUMP") {
        std::fs::write(std::path::Path::new(&dir).join(format!("kernel-{}.spv", k.spirv.len())), &bytes).unwrap();
    }
    let out = std::process::Command::new("spirv-val").arg("--target-env").arg("vulkan1.2").arg(&path).output();
    if let Ok(o) = out {
        assert!(o.status.success(), "spirv-val: {}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
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
        let compiled = compile_compute_kernel(&mut vm, field.value.as_object().unwrap());
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
