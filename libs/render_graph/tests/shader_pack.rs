//! Every standard kit's passes and the accumulator's shaders go compile ->
//! shader pack -> mapping and come back equal to the compile, field by
//! field (see `makepad_platform::shader_pack`).

use makepad_draw::makepad_platform::shader_pack::verify_shader_pack_entry;
use makepad_draw::*;
use makepad_render_graph::{kits, pass, script, UniformDecl};

fn shader_object(vm: &mut ScriptVm, file: &str, code: String) -> ScriptObject {
    vm.capture_errors();
    let v = vm.eval(ScriptMod::new("", "", &file, 0, 0, &code, &[]));
    let errors = vm.take_errors();
    assert!(errors.is_empty() && !v.is_err(), "{file} did not evaluate: {errors:?}");
    v.as_object().unwrap()
}

fn verify_pass(vm: &mut ScriptVm, decl: &pass::PassDecl) {
    let shader = makepad_render_graph::program::pass_object(vm, decl).unwrap_or_else(|e| panic!("{}: {e}", decl.label));
    verify_shader_pack_entry(vm, shader).unwrap_or_else(|e| panic!("{}: {e}", decl.label));
}

#[test]
fn kit_passes_round_trip_through_a_pack() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_draw::script_mod(vm);
        // The passes' shared stdlib (`use mod.shared.*` in program and accum).
        makepad_render_graph::pass_stdlib(vm);
        makepad_render_graph::program::script_mod(vm);
        makepad_render_graph::accum::script_mod(vm);
        let m = vm.new_module(LiveId::from_str("gtest"));
        for k in kits::KITS {
            let proto = vm.new_object();
            vm.set_value_def(proto, LiveId::from_str("__kind"), LiveId::from_str(k.kind));
            vm.set_value_def(m, LiveId::from_str(k.name), proto);
        }
        let errors = script::install_kits(vm, "mod.gtest", None);
        assert!(errors.is_empty(), "kits did not evaluate: {errors:?}");
        let mut total = 0;
        let mut chain = Vec::new();
        for (n, k) in kits::KITS.iter().enumerate() {
            let module = vm.value(vm.modules(), LiveId::from_str("gtest")).as_object().unwrap();
            let template = vm.value(module, LiveId::from_str(&format!("kit_{}", k.kind)));
            let proto = vm.value(module, LiveId::from_str(k.name));
            let author = vm.new_with_proto(proto);
            vm.capture_errors();
            let built = vm.call(template, &[author.into()]);
            let errors = vm.take_errors();
            assert!(errors.is_empty() && !built.is_err(), "{} did not build: {errors:?}", k.name);
            let items: Vec<ScriptValue> = match built.as_array() {
                Some(a) => (0..vm.heap().array_len(a)).map(|i| vm.heap().array_index(a, i, NoTrap)).collect(),
                None => panic!("{} returned no list", k.name),
            };
            let mut decls = Vec::new();
            for (i, it) in items.into_iter().enumerate() {
                let read = script::read_pass(vm, it, &format!("{}[{i}]", k.name)).unwrap_or_else(|e| panic!("{e}"));
                let mut decl = read.decl;
                for (name, v) in read.uniforms {
                    let width = if v.as_number().is_some() { 1 } else { 4 };
                    decl.uniforms.push(UniformDecl { name, width });
                }
                decls.push(decl);
            }
            pass::namespace(&mut decls, &format!("k{n}_"));
            for d in &decls {
                verify_pass(vm, d);
                total += 1;
            }
            chain.extend(decls);
        }
        assert!(total >= 20, "{total} passes");
        // The fused passes (their members' fns compiled in namespaces).
        let (fused, members) = pass::fuse(&chain);
        let mut merged = 0;
        for (d, m) in fused.iter().zip(&members).filter(|(_, m)| m.len() > 1) {
            let _ = m;
            verify_pass(vm, d);
            merged += 1;
        }
        assert!(merged > 0, "no kit passes fused");
        for name in ["DrawGraphAccum", "DrawGraphMerge", "DrawGraphAverage", "DrawGraphError", "DrawGraphErrorMax", "DrawGraphPass"] {
            let shader = shader_object(vm, name, format!("use mod.draw\nmod.draw.{name}{{}}\n"));
            verify_shader_pack_entry(vm, shader).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    });
}
