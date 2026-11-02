#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
pub use makepad_skyline::makepad_widgets;

use makepad_skyline::{shaders, terrain};

/// The tree shader (its vertex placement through the column texture and its coverage) compiles for Metal, HLSL and GLSL ES.
#[test]
fn tree_shader_compiles_for_every_backend() {
    use makepad_widgets::makepad_draw::{self, makepad_platform, makepad_script::{shader_backend::ShaderBackend, shader_compile::{compile_shader, ShaderCompileOptions}}, *};
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_platform::script::script_mod(vm);
        makepad_draw::script_mod(vm);
        shaders::script_mod(vm);
        terrain::script_mod(vm);
        let shader = vm.eval(script! { use mod.draw.*; mod.draw.DrawSkyTrees{} }).as_object().unwrap();
        for (backend, gles) in [(ShaderBackend::Metal, false), (ShaderBackend::Hlsl, false), (ShaderBackend::Glsl, true)] {
            let mut opts = ShaderCompileOptions::new(backend, false, false, false, false);
            opts.gles_complete = gles;
            let output = compile_shader(vm, shader, &opts);
            assert!(!output.has_errors(), "{backend:?}: {}", output.view().error_report);
        }
    });
}
