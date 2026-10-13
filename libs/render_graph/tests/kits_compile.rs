//! Every standard kit evaluates in Splash, reads as passes, and each pass
//! compiles on every backend the shader compiler writes; the accumulator's
//! and the standard block's own shaders compile too. (Shader errors
//! otherwise only show at runtime, as a pass that draws nothing.)

use makepad_draw::*;
use makepad_render_graph::{kits, pass, script, UniformDecl};

const BACKENDS: [&str; 4] = ["metal", "hlsl", "glsl", "wgsl"];

fn compile_value(vm: &mut ScriptVm, file: &str, code: String) -> Result<String, String> {
    vm.capture_errors();
    let v = vm.eval(ScriptMod::new("", "", &file, 0, 0, &code, &[]));
    let errors = vm.take_errors();
    if !errors.is_empty() || v.is_err() {
        return Err(errors.join("; "));
    }
    let text = vm.heap().string_with(v, |_, s| s.to_string()).ok_or("the compiler gave no source")?;
    if text.starts_with("ERRORS") {
        return Err(text);
    }
    Ok(text)
}

/// The pass's shader (an object built from the decl, its fns moved by their
/// tokens) compiled for `backend`.
fn compile_for(vm: &mut ScriptVm, decl: &pass::PassDecl, backend: &str) -> Result<String, String> {
    let obj = makepad_render_graph::program::pass_object(vm, decl)?;
    let shader = vm.value(vm.modules(), LiveId::from_str("shader")).as_object().unwrap();
    let f = vm.value(shader, LiveId::from_str("test_compile_draw_source"));
    let b = vm.new_string_with(|_, s| s.push_str(backend));
    vm.capture_errors();
    let v = vm.call(f, &[obj.into(), b, false.into()]);
    let errors = vm.take_errors();
    if !errors.is_empty() || v.is_err() {
        return Err(errors.join("; "));
    }
    let text = vm.heap().string_with(v, |_, s| s.to_string()).ok_or("the compiler gave no source")?;
    if text.starts_with("ERRORS") {
        return Err(text);
    }
    Ok(text)
}

fn compile_pass(vm: &mut ScriptVm, decl: &pass::PassDecl) {
    for backend in BACKENDS {
        if let Err(e) = compile_for(vm, decl, backend) {
            panic!("{} ({backend}) did not compile: {e}", decl.label);
        }
    }
}

/// A Splash fn of this VM as a pass's code.
fn code(vm: &mut ScriptVm, src: &str) -> pass::PassCode {
    let v = vm.eval(ScriptMod::new("", "", "kits_compile_code", 0, 0, src, &[]));
    pass::PassCode::Fn(pass::PassFn::new(vm.fn_image(v.as_object().unwrap()).unwrap()))
}

#[test]
fn every_kit_builds_passes_that_compile_everywhere() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_draw::script_mod(vm);
        // The passes' shared stdlib (`use mod.shared.*` in program and accum).
        makepad_render_graph::pass_stdlib(vm);
        makepad_render_graph::program::script_mod(vm);
        makepad_render_graph::accum::script_mod(vm);
        // A host module with the kits' types (the host's own markers).
        let m = vm.new_module(LiveId::from_str("gtest"));
        for k in kits::KITS {
            let proto = vm.new_object();
            vm.set_value_def(proto, LiveId::from_str("__kind"), LiveId::from_str(k.kind));
            vm.set_value_def(m, LiveId::from_str(k.name), proto);
        }
        let errors = script::install_kits(vm, "mod.gtest", None);
        assert!(errors.is_empty(), "kits did not evaluate: {errors:?}");
        let mut total = 0;
        let mut chain: Vec<pass::PassDecl> = Vec::new();
        // Every kit with its defaults, and the variants a parameter selects.
        let variants: Vec<(&kits::KitDef, Option<(&str, f64)>)> = kits::KITS.iter().map(|k| (k, None)).chain([("vignette", "mode", 1.0), ("frame_post", "mode", 1.0), ("outline", "mode", 1.0)].into_iter().map(|(kind, f, v)| (kits::kit(kind).unwrap(), Some((f, v))))).collect();
        for (k, over) in variants {
            let module = vm.value(vm.modules(), LiveId::from_str("gtest")).as_object().unwrap();
            let template = vm.value(module, LiveId::from_str(&format!("kit_{}", k.kind)));
            let proto = vm.value(module, LiveId::from_str(k.name));
            // The author's object: the kit with its defaults.
            let author = vm.new_with_proto(proto);
            if let Some((field, value)) = over {
                vm.set_value_def(author, LiveId::from_str(field), value);
            }
            vm.capture_errors();
            let built = vm.call(template, &[author.into()]);
            let errors = vm.take_errors();
            assert!(errors.is_empty() && !built.is_err(), "{} did not build: {errors:?}", k.name);
            let items: Vec<ScriptValue> = match built.as_array() {
                Some(a) => (0..vm.heap().array_len(a)).map(|i| vm.heap().array_index(a, i, NoTrap)).collect(),
                None => panic!("{} returned no list", k.name),
            };
            assert!(!items.is_empty());
            let mut decls = Vec::new();
            for (i, it) in items.into_iter().enumerate() {
                let read = script::read_pass(vm, it, &format!("{}[{i}]", k.name)).unwrap_or_else(|e| panic!("{e}"));
                let mut decl = read.decl;
                for (name, v) in read.uniforms {
                    // Every kit parameter is a number or a colour here.
                    let width = if v.as_number().is_some() { 1 } else { 4 };
                    decl.uniforms.push(UniformDecl { name, width });
                }
                decl.validate().unwrap_or_else(|e| panic!("{}: {e}", decl.label));
                decls.push(decl);
            }
            pass::namespace(&mut decls, &format!("k{}_", chain.len()));
            for d in &decls {
                compile_pass(vm, d);
                total += 1;
            }
            chain.extend(decls);
        }
        assert!(total >= 20, "{total} passes");
        // Every kit one after another: the map passes that follow one
        // another fuse, and each fused pass compiles everywhere.
        let (fused, members) = pass::fuse(&chain);
        let merged: Vec<&pass::PassDecl> = fused.iter().zip(&members).filter(|(_, m)| m.len() > 1).map(|(d, _)| d).collect();
        assert!(!merged.is_empty(), "no kit passes fused");
        for d in merged {
            d.validate().unwrap_or_else(|e| panic!("{}: {e}", d.label));
            compile_pass(vm, d);
        }
        // A raymarch-style pass with further outputs (MRT): a G-buffer and
        // a view distance, written as `self.<name>`.
        let mrt = pass::PassDecl {
            name: Some("march".into()),
            stage: makepad_render_graph::Stage::Hdr,
            reads: vec!["color".into()],
            slots: Vec::new(),
            scale: 0.5,
            preview_scale: 1.0, scale_height: None,
            size: None,
            format: None,
            uniforms: Vec::new(),
            pixel: code(vm, "fn() -> vec4 { self.gbuf = vec4(self.uv(), 0.0, 1.0) self.march_depth = vec4(3.0, 0.0, 0.0, 0.0) return vec4(1.0, 0.5, 0.2, 1.0) }"),
            helpers: Vec::new(),
            history: false,
            outputs: vec![
                pass::OutputDecl { name: "gbuf".into(), slot: "gbuf".into(), format: makepad_render_graph::Format::Rgba16f },
                pass::OutputDecl { name: "march_depth".into(), slot: "march_depth".into(), format: makepad_render_graph::Format::R32f },
            ],
            label: "mrt".into(),
            map: false,
            live_literals: false,
        };
        mrt.validate().unwrap();
        compile_pass(vm, &mrt);
        // Analytic-derivative sampling (PDOOM-PARITY R9): a warped lookup
        // with the derivatives of the unwarped coordinate.
        let grad = pass::PassDecl {
            name: None,
            outputs: Vec::new(),
            pixel: code(vm, "fn() -> vec4 { let q = fract(self.uv() * 3.0) return self.color.sample_grad(q, dFdx(self.uv() * 3.0), dFdy(self.uv() * 3.0)) }"),
            label: "grad".into(),
            map: false,
            live_literals: false,
            ..mrt.clone()
        };
        compile_pass(vm, &grad);
        // And the check has teeth: a pass reading what it does not declare
        // is refused.
        let bad = pass::PassDecl {
            name: None,
            stage: makepad_render_graph::Stage::Hdr,
            reads: vec!["color".into()],
            slots: Vec::new(),
            scale: 1.0,
            preview_scale: 1.0, scale_height: None,
            size: None,
            format: None,
            uniforms: Vec::new(),
            pixel: code(vm, "fn() -> vec4 { return self.nope.sample(self.uv()) }"),
            helpers: Vec::new(),
            history: false,
            outputs: Vec::new(),
            label: "bad".into(),
            map: false,
            live_literals: false,
        };
        assert!(compile_for(vm, &bad, "metal").is_err());
        // Short slot and uniform names (a pass named `g`, a uniform `r`)
        // compile: they do not collide with the shader's own members.
        let short = pass::PassDecl {
            name: None,
            stage: makepad_render_graph::Stage::Hdr,
            reads: vec!["color".into(), "g".into()],
            slots: Vec::new(),
            scale: 1.0,
            preview_scale: 1.0, scale_height: None,
            size: None,
            format: None,
            uniforms: vec![UniformDecl { name: "r".into(), width: 1 }, UniformDecl { name: "b".into(), width: 4 }],
            pixel: code(vm, "fn() -> vec4 { return self.color.sample(self.uv()) + self.g.sample(self.uv()) * self.r + self.b }"),
            helpers: Vec::new(),
            history: false,
            outputs: Vec::new(),
            label: "short".into(),
            map: false,
            live_literals: false,
        };
        short.validate().unwrap();
        compile_pass(vm, &short);
        // The accumulator's own shaders.
        for name in ["DrawGraphAccum", "DrawGraphMerge", "DrawGraphAverage", "DrawGraphError", "DrawGraphErrorMax", "DrawGraphPass"] {
            for backend in BACKENDS {
                let code = format!("use mod.draw\nmod.shader.test_compile_draw_source(mod.draw.{name}, \"{backend}\", false)\n");
                compile_value(vm, name, code).unwrap_or_else(|e| panic!("{name} ({backend}): {e}"));
            }
        }
    });
}
