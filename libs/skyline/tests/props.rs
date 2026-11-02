//! The props against the web page:
//! - `part_shaders_compile_for_every_backend` (no GPU): the part shader and the props layer's blit for Metal, HLSL and GLSL ES;
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
pub use makepad_skyline::makepad_widgets;

use makepad_skyline::{city, props, shaders};

use makepad_widgets::makepad_draw::{self, *};
#[test]
fn part_shaders_compile_for_every_backend() {
    use makepad_draw::makepad_script::{shader_backend::ShaderBackend, shader_compile::{compile_shader, ShaderCompileOptions}};
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_platform::script::script_mod(vm);
        makepad_draw::script_mod(vm);
        shaders::script_mod(vm);
        city::script_mod(vm);
        props::kit::script_mod(vm);
        let part = vm.eval(script! { use mod.draw.*; mod.draw.DrawSkyPart{} }).as_object().unwrap();
        let blit = vm.eval(script! { use mod.draw.*; mod.draw.DrawSkyPartBlit{} }).as_object().unwrap();
        for (backend, gles) in [(ShaderBackend::Metal, false), (ShaderBackend::Hlsl, false), (ShaderBackend::Glsl, true)] {
            let mut opts = ShaderCompileOptions::new(backend, false, false, false, false);
            opts.gles_complete = gles;
            for shader in [part, blit] {
                let output = compile_shader(vm, shader, &opts);
                assert!(!output.has_errors(), "{backend:?}: {}", output.view().error_report);
            }
        }
    });
}
