//! The single trees against the web page: `SKYLINE_TREES=FILE [SKYLINE_PARAMS=FILE] [SKYLINE_W=4096] cargo test --release
//! -p makepad-skyline --test trees` writes the strip's tree mesh (`terrain::trees`) as raw little-endian f32
//! vertices and u32 indices for `trees_check.js`, and prints its size and build time.
#![cfg(not(target_arch = "wasm32"))]
#![allow(dead_code)]
pub use makepad_skyline::makepad_widgets;

use makepad_skyline::{params, rng, shaders, terrain};

use makepad_widgets::makepad_draw::Cx;

#[test]
fn dump_trees() {
    let Ok(path) = std::env::var("SKYLINE_TREES") else { return };
    let mut p = params::Params::default();
    if let Ok(file) = std::env::var("SKYLINE_PARAMS") { p.load_web_json(&std::fs::read_to_string(&file).unwrap()).unwrap(); }
    let w: f64 = std::env::var("SKYLINE_W").ok().and_then(|s| s.parse().ok()).unwrap_or(4096.0);
    let rng = rng::Rng::new(p.seed);
    let px0 = rng::TAU / w;
    let mut best = f64::MAX;
    let mut m = terrain::trees(&rng, &p, px0, None);
    for _ in 0..10 {
        let t = Cx::monotonic_now();
        m = terrain::trees(&rng, &p, px0, None);
        best = best.min((Cx::monotonic_now() - t) * 1000.0);
    }
    println!("trees {} verts {} tris {} build {:.2} ms (best of 10)", m.trees, m.v.len() / 9, m.i.len() / 3, best);
    // the detail view's build: twice its warped arc at its pixel size (as `Trees::draw_pass`)
    let (span, u0) = (p[params::P::Zoom], p[params::P::Pan] - p[params::P::Zoom] / 2.0);
    let a = terrain::warp(&p, rng::fract(u0 - 0.5 * span)).0;
    let len = rng::fract(terrain::warp(&p, rng::fract(u0 + 1.5 * span)).0 - a) + 0.024;
    let arc = (len < 1.0).then(|| (rng::fract(a - 0.012), rng::fract(a - 0.012 + len)));
    let (mut best_d, mut md) = (f64::MAX, terrain::TreeMesh::default());
    for _ in 0..10 { let t = Cx::monotonic_now(); md = terrain::trees(&rng, &p, span * rng::TAU / 1600.0, arc); best_d = best_d.min((Cx::monotonic_now() - t) * 1000.0); }
    println!("detail 1600: trees {} verts {} tris {} build {:.2} ms (best of 10)", md.trees, md.v.len() / 9, md.i.len() / 3, best_d);
    let mut out = Vec::new();
    out.extend((m.v.len() as u32).to_le_bytes());
    out.extend((m.i.len() as u32).to_le_bytes());
    for x in &m.v { out.extend(x.to_le_bytes()); }
    for x in &m.i { out.extend(x.to_le_bytes()); }
    std::fs::write(&path, out).unwrap();
}

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
