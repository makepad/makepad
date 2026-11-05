//! A `precise_math` draw shader's fixed vector forms (normalize, length, distance, dot) and,
//! where the hardware does not round them correctly, its divides and roots are calls of helpers
//! taking the operands: an operand expression is written once, at the call, not once per lane
//! that reads it (normalize read its operand nine times, its corrected root three more).
use makepad_draw::makepad_script::{
    shader_backend::ShaderBackend,
    shader_ir_draw::{compile_draw_shader_ir_with, draw_spirv, IrCompileOptions},
    shader_ir_glsl::glsl_draw_sources,
    shader_ir_hlsl::hlsl_draw,
    shader_ir_metal::metal_draw,
};
use makepad_draw::*;

/// The pixel function's normalized operand is `operand`; `bound` binds it to a local first.
fn document(bound: bool) -> String {
    let n = if bound { "let w = vec3(self.pos, 0.25) * self.scale\n let n = normalize(w)" } else { "let n = normalize(vec3(self.pos, 0.25) * self.scale)" };
    format!(
        "use mod.pod.*\nuse mod.math.*\nuse mod.shader.*\nuse mod.draw.*\n\
         DrawQuad{{\n\
             precise_math: true\n\
             scale: uniform(0.5)\n\
             pixel: fn() {{\n\
                 let v = vec4(self.pos, 0.25, 2.0)\n\
                 {n}\n\
                 let q = (v.xyz + n) / v.w\n\
                 return vec4(n + q + sqrt(abs(q)) + inverseSqrt(abs(n) + vec3(1.0)), length(v.xy) + distance(v.xyz, q) + dot(v, v))\n\
             }}\n\
         }}\n"
    )
}

fn shader(vm: &mut ScriptVm, bound: bool) -> ScriptObject {
    vm.capture_errors();
    let value = vm.eval(ScriptMod::new("", "", if bound { "precise_bound" } else { "precise_inline" }, 0, 0, &document(bound), &[]));
    let errors = vm.take_errors();
    assert!(errors.is_empty() && !value.is_err(), "the shader did not evaluate: {errors:?}");
    value.as_object().unwrap()
}

/// The pixel stage's text of `obj` on `backend`.
fn text(vm: &mut ScriptVm, obj: ScriptObject, backend: ShaderBackend) -> String {
    let mut out = compile_draw_shader_ir_with(vm, obj, &IrCompileOptions::new(backend, false, false, false, false));
    assert!(!out.has_errors, "{backend:?}: {}", out.error_report());
    match backend {
        ShaderBackend::Metal => metal_draw(&out, vm, false).0,
        ShaderBackend::Hlsl => hlsl_draw(&out, vm, false).0,
        _ => glsl_draw_sources(&mut out, vm).fragment,
    }
}

#[test]
fn precise_vector_forms_read_their_operands_once() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_draw::script_mod(vm);
        let (inline, bound) = (shader(vm, false), shader(vm, true));
        for backend in [ShaderBackend::Metal, ShaderBackend::Hlsl, ShaderBackend::Glsl] {
            let t = text(vm, inline, backend);
            for helper in ["_mp_precise_normalize_3_0", "_mp_precise_length_2_0", "_mp_precise_distance_3_3", "_mp_precise_dot_4_4"] {
                assert!(t.contains(helper), "{backend:?}: no {helper}:\n{t}");
            }
            // Metal divides and roots correctly itself: only the fixed forms are helpers there
            if backend != ShaderBackend::Metal {
                for helper in ["_mp_precise_div_3_1", "_mp_precise_sqrt_3_0", "_mp_precise_inversesqrt_3_0"] {
                    assert!(t.contains(helper), "{backend:?}: no {helper}:\n{t}");
                }
            }
            // the uniform is read as often as when the product is bound to a local first
            let reads = |t: &str| t.matches("scale").count();
            let b = text(vm, bound, backend);
            assert!(reads(&b) > 0, "{backend:?}: the uniform's name is not in the text:\n{b}");
            assert_eq!(reads(&t), reads(&b), "{backend:?}: the normalized product written again per lane:\n{t}");
            // the helpers compile on the Metal device, as the HAL builds them
            #[cfg(target_os = "macos")]
            if backend == ShaderBackend::Metal {
                makepad_draw::makepad_platform::makepad_hal::backend::apple::metal::compile_msl(&t).unwrap_or_else(|e| panic!("Metal: {e}\n{t}"));
            }
        }
        // Vulkan: the same lowering into SPIR-V, which validates
        let out = compile_draw_shader_ir_with(vm, inline, &IrCompileOptions::new(ShaderBackend::Glsl, false, true, false, false));
        assert!(!out.has_errors, "Vulkan: {}", out.error_report());
        for module in draw_spirv(&out, vm).unwrap() {
            for words in [&module.vertex, &module.fragment] {
                spirv_val(words);
            }
        }
    });
}

fn spirv_val(words: &[u32]) {
    assert!(!words.is_empty());
    #[cfg(target_os = "linux")]
    {
        static SERIAL: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = SERIAL.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file = std::env::temp_dir().join(format!("precise-forms-{}-{serial}.spv", std::process::id()));
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        std::fs::write(&file, bytes).unwrap();
        let out = std::process::Command::new("spirv-val").args(["--target-env", "vulkan1.2"]).arg(&file).output().expect("spirv-val required");
        std::fs::remove_file(file).unwrap();
        assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }
}
