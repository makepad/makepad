//! The post shaders (the final composite, the blur, the water by night and by day, a band's choice of the water's
//! distance, the blur of a band's light plane), the ground band, which gives way to a host's wet floor (plan hdri-6
//! fix wave 1: the cut the final made before), and the city's blit and the props' parts, which give way with it below
//! the horizon (fix wave 1, round 2), compile for Metal, HLSL and GLSL ES (no GPU needed).
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
pub use makepad_skyline::makepad_widgets;

use makepad_skyline::{city, post, props, shaders};

use makepad_widgets::makepad_draw::{self, *};

#[test]
fn post_shaders_compile_for_every_backend() {
    use makepad_draw::makepad_script::{shader_backend::ShaderBackend, shader_compile::{compile_shader, ShaderCompileOptions}};
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_platform::script::script_mod(vm);
        makepad_draw::script_mod(vm);
        shaders::script_mod(vm);
        city::script_mod(vm);
        props::kit::script_mod(vm);
        post::script_mod(vm);
        let broken = vm.eval(script! { use mod.draw.*; DrawQuad{ pixel: fn(){ return self.no_such_field } } });
        let shaders = [("final", vm.eval(script! { mod.draw.DrawSkyFinal })), ("ground", vm.eval(script! { mod.draw.DrawSkyGround })), ("blur", vm.eval(script! { mod.draw.DrawSkyBlur })),
            ("water", vm.eval(script! { mod.draw.DrawSkyWater })), ("water_day", vm.eval(script! { mod.draw.DrawSkyWaterDay })), ("aurora_water", vm.eval(script! { mod.draw.DrawSkyAuroraWater })),
            ("water_dist", vm.eval(script! { mod.draw.DrawSkyWaterDist })), ("dist_blur", vm.eval(script! { mod.draw.DrawSkyDistBlur })),
            ("city_blit", vm.eval(script! { mod.draw.DrawSkyCityBlit })), ("part", vm.eval(script! { mod.draw.DrawSkyPart })), ("broken", broken)];
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
