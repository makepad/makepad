//! The post shaders (the final composite, the blur, the water) compile for Metal, HLSL and GLSL ES (no GPU needed).
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
pub use makepad_widgets;

#[path = "../src/city.rs"]
mod city;
#[path = "../src/export.rs"]
mod export;
#[path = "../src/params.rs"]
mod params;
#[path = "../src/post.rs"]
mod post;
#[path = "../src/props/mod.rs"]
mod props;
#[path = "../src/render.rs"]
mod render;
#[path = "../src/rng.rs"]
mod rng;
#[path = "../src/shaders.rs"]
mod shaders;
#[path = "../src/terrain.rs"]
mod terrain;
#[path = "../src/zones.rs"]
mod zones;

use makepad_widgets::makepad_draw::{self, *};

#[test]
fn post_shaders_compile_for_every_backend() {
    use makepad_draw::makepad_script::{shader_backend::ShaderBackend, shader_compile::{compile_shader, ShaderCompileOptions}};
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_platform::script::script_mod(vm);
        makepad_draw::script_mod(vm);
        shaders::script_mod(vm);
        post::script_mod(vm);
        let broken = vm.eval(script! { use mod.draw.*; DrawQuad{ pixel: fn(){ return self.no_such_field } } });
        let shaders = [("final", vm.eval(script! { mod.draw.DrawSkyFinal })), ("blur", vm.eval(script! { mod.draw.DrawSkyBlur })), ("water", vm.eval(script! { mod.draw.DrawSkyWater })), ("broken", broken)];
        for (name, shader) in shaders {
            let shader = shader.as_object().unwrap_or_else(|| panic!("{name}: no shader object"));
            for (backend, gles) in [(ShaderBackend::Metal, false), (ShaderBackend::Hlsl, false), (ShaderBackend::Glsl, true)] {
                let mut opts = ShaderCompileOptions::new(backend, false, false, false, false);
                opts.gles_complete = gles;
                let output = compile_shader(vm, shader, &opts);
                // the control: a shader reading a field it lacks must fail, so a pass means the code was compiled
                assert_eq!(output.has_errors(), name == "broken", "{name} {backend:?}: {}", output.view().error_report);
            }
        }
    });
}
