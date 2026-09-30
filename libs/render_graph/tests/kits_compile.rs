//! Every standard kit evaluates in Splash, reads as passes, and each pass
//! compiles on every backend the shader compiler writes; the accumulator's
//! and the standard block's own shaders compile too. (Shader errors
//! otherwise only show at runtime, as a pass that draws nothing.)

use makepad_draw::*;
use makepad_render_graph::{kits, pass, script, UniformDecl};

const BACKENDS: [&str; 4] = ["metal", "hlsl", "glsl", "wgsl"];

fn compile_value(vm: &mut ScriptVm, file: &str, code: String) -> Result<String, String> {
    vm.bx.captured_errors = Some(Vec::new());
    let v = vm.eval(ScriptMod { file: file.into(), code, ..Default::default() });
    let errors = vm.take_errors();
    if !errors.is_empty() || v.is_err() {
        return Err(errors.join("; "));
    }
    let text = vm.bx.heap.string_with(v, |_, s| s.to_string()).ok_or("the compiler gave no source")?;
    if text.starts_with("ERRORS") {
        return Err(text);
    }
    Ok(text)
}

fn compile_pass(vm: &mut ScriptVm, decl: &pass::PassDecl) {
    let src = decl.source();
    for backend in BACKENDS {
        let code = src.replacen("mod.draw.DrawGraphPass{", "let sh = mod.draw.DrawGraphPass{", 1) + &format!("mod.shader.test_compile_draw_source(sh, \"{backend}\", false)\n");
        if let Err(e) = compile_value(vm, &format!("kit/{}", decl.label), code) {
            panic!("{} ({backend}) did not compile: {e}\n{src}", decl.label);
        }
    }
}

#[test]
fn every_kit_builds_passes_that_compile_everywhere() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_draw::script_mod(vm);
        makepad_render_graph::pass::script_mod(vm);
        makepad_render_graph::accum::script_mod(vm);
        // A host module with the kits' types (the host's own markers).
        let m = vm.new_module(LiveId::from_str("gtest"));
        for k in kits::KITS {
            let proto = vm.bx.heap.new_object();
            vm.bx.heap.set_value_def(proto, LiveId::from_str("__kind").into(), LiveId::from_str(k.kind).into());
            vm.bx.heap.set_value_def(m, LiveId::from_str(k.name).into(), proto.into());
        }
        let errors = script::install_kits(vm, "mod.gtest", None);
        assert!(errors.is_empty(), "kits did not evaluate: {errors:?}");
        let mut total = 0;
        for k in kits::KITS {
            let module = vm.bx.heap.value(vm.bx.heap.modules, LiveId::from_str("gtest").into(), NoTrap).as_object().unwrap();
            let template = vm.bx.heap.value(module, LiveId::from_str(&format!("kit_{}", k.kind)).into(), NoTrap);
            let proto = vm.bx.heap.value(module, LiveId::from_str(k.name).into(), NoTrap);
            // The author's object: the kit with its defaults.
            let author = vm.bx.heap.new_with_proto(proto);
            vm.bx.captured_errors = Some(Vec::new());
            let built = vm.call(template, &[author.into()]);
            let errors = vm.take_errors();
            assert!(errors.is_empty() && !built.is_err(), "{} did not build: {errors:?}", k.name);
            let items: Vec<ScriptValue> = match built.as_array() {
                Some(a) => (0..vm.bx.heap.array_len(a)).map(|i| vm.bx.heap.array_index(a, i, NoTrap)).collect(),
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
            pass::namespace(&mut decls, "k0_");
            for d in &decls {
                compile_pass(vm, d);
                total += 1;
            }
        }
        assert!(total >= 20, "{total} passes");
        // And the check has teeth: a pass reading what it does not declare
        // is refused.
        let bad = pass::PassDecl {
            name: None,
            stage: makepad_render_graph::Stage::Hdr,
            reads: vec!["color".into()],
            slots: Vec::new(),
            scale: 1.0,
            format: None,
            uniforms: Vec::new(),
            pixel: "fn() -> vec4 { return self.nope.sample(self.uv()) }".into(),
            helpers: String::new(),
            label: "bad".into(),
        };
        let code = bad.source().replacen("mod.draw.DrawGraphPass{", "let sh = mod.draw.DrawGraphPass{", 1) + "mod.shader.test_compile_draw_source(sh, \"metal\", false)\n";
        assert!(compile_value(vm, "bad", code).is_err());
        // The accumulator's own shaders.
        for name in ["DrawGraphAccum", "DrawGraphMerge", "DrawGraphAverage", "DrawGraphError", "DrawGraphErrorMax", "DrawGraphPass"] {
            for backend in BACKENDS {
                let code = format!("use mod.draw\nmod.shader.test_compile_draw_source(mod.draw.{name}, \"{backend}\", false)\n");
                compile_value(vm, name, code).unwrap_or_else(|e| panic!("{name} ({backend}): {e}"));
            }
        }
    });
}
