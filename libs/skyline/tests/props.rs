//! The props against the web page:
//! - `part_shaders_compile_for_every_backend` (no GPU): the part shader and the props layer's blit for Metal, HLSL and GLSL ES;
//! - `prop_counts_at_seed_7`: the instance and part counts of the page at seed 7 (defaults, and every prop at 1);
//! - `dump_props`: with `SKYLINE_PROPS=FILE` (and `SKYLINE_PARAMS=FILE`, the page's saved parameters) writes every
//!   instance and part as JSON for `check_props.js`.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
pub use makepad_skyline::makepad_widgets;

use makepad_skyline::{city, params, props, shaders, zones};

use makepad_widgets::makepad_draw::{self, *};
use params::{Params, P, PARAMS};

fn build(p: &Params) -> props::kit::PropSet { props::build(&zones::Zones::new(p), p) }

#[test]
fn prop_counts_at_seed_7() {
    let mut p = Params::default();
    let set = build(&p);
    assert_eq!((set.inst.len(), set.parts.len()), (465, 3492), "defaults");
    for (i, d) in PARAMS.iter().enumerate() { if d.group == "Props" && i != P::PrRefl as usize && i != P::PrRipple as usize { p.v[i] = 1.0; } }
    let set = build(&p);
    assert_eq!((set.inst.len(), set.parts.len()), (855, 6439), "every prop at 1");
}

#[test]
fn dump_props() {
    let Ok(path) = std::env::var("SKYLINE_PROPS") else { return };
    let mut p = Params::default();
    if let Ok(file) = std::env::var("SKYLINE_PARAMS") { p.load_web_json(&std::fs::read_to_string(&file).unwrap()).unwrap(); }
    let t = Cx::monotonic_now();
    let set = build(&p);
    eprintln!("{} instances, {} parts in {:.2} ms", set.inst.len(), set.parts.len(), (Cx::monotonic_now() - t) * 1e3);
    let n = |x: f64| if x.is_nan() { "null".to_string() } else { format!("{x:?}") };
    let inst: Vec<String> = set.inst.iter().map(|i| {
        let v = [i.slot as f64, i.range as f64, i.view as f64, i.hide_c as u8 as f64, i.hide_m as u8 as f64, i.hide_i as u8 as f64, i.line as u8 as f64, i.uc, i.d, i.base, i.hw,
            i.o[0], i.o[1], i.ang, i.len, i.smar, i.top, i.td, (i.seed & 8191) as f64, i.glow, i.p0 as f64, i.np as f64];
        format!("[{}]", v.map(n).join(","))
    }).collect();
    let parts: Vec<String> = set.parts.iter().map(|q| {
        let v = [q.t as u8 as f64, q.c[0], q.c[1], q.c[2], q.a[0], q.a[1], q.a[2], q.a[3], q.b[0], q.b[1], q.b[2], q.b[3]];
        format!("[{}]", v.map(n).join(","))
    }).collect();
    std::fs::write(&path, format!("{{\"inst\":[{}],\"parts\":[{}]}}", inst.join(","), parts.join(","))).unwrap();
}

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
