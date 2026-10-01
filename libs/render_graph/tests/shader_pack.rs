//! Every standard kit's passes and the accumulator's shaders go compile ->
//! shader pack -> mapping and come back equal to the compile, field by
//! field (see `makepad_platform::shader_pack`).

use makepad_draw::makepad_platform::shader_pack::verify_shader_pack_entry;
use makepad_draw::*;
use makepad_render_graph::{kits, pass, script, UniformDecl};

fn shader_object(vm: &mut ScriptVm, file: &str, code: String) -> ScriptObject {
    vm.bx.captured_errors = Some(Vec::new());
    let v = vm.eval(ScriptMod { file: file.into(), code, ..Default::default() });
    let errors = vm.take_errors();
    assert!(errors.is_empty() && !v.is_err(), "{file} did not evaluate: {errors:?}");
    v.as_object().unwrap()
}

fn verify_pass(vm: &mut ScriptVm, decl: &pass::PassDecl) {
    let code = decl.source().replacen("mod.draw.DrawGraphPass{", "let sh = mod.draw.DrawGraphPass{", 1) + "sh\n";
    let shader = shader_object(vm, &format!("kit/{}", decl.label), code);
    verify_shader_pack_entry(vm, shader).unwrap_or_else(|e| panic!("{}: {e}", decl.label));
}

#[test]
fn kit_passes_round_trip_through_a_pack() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_draw::script_mod(vm);
        makepad_render_graph::program::script_mod(vm);
        makepad_render_graph::accum::script_mod(vm);
        let m = vm.new_module(LiveId::from_str("gtest"));
        for k in kits::KITS {
            let proto = vm.bx.heap.new_object();
            vm.bx.heap.set_value_def(proto, LiveId::from_str("__kind").into(), LiveId::from_str(k.kind).into());
            vm.bx.heap.set_value_def(m, LiveId::from_str(k.name).into(), proto.into());
        }
        let errors = script::install_kits(vm, "mod.gtest", None);
        assert!(errors.is_empty(), "kits did not evaluate: {errors:?}");
        let mut total = 0;
        for (n, k) in kits::KITS.iter().enumerate() {
            let module = vm.bx.heap.value(vm.bx.heap.modules, LiveId::from_str("gtest").into(), NoTrap).as_object().unwrap();
            let template = vm.bx.heap.value(module, LiveId::from_str(&format!("kit_{}", k.kind)).into(), NoTrap);
            let proto = vm.bx.heap.value(module, LiveId::from_str(k.name).into(), NoTrap);
            let author = vm.bx.heap.new_with_proto(proto);
            vm.bx.captured_errors = Some(Vec::new());
            let built = vm.call(template, &[author.into()]);
            let errors = vm.take_errors();
            assert!(errors.is_empty() && !built.is_err(), "{} did not build: {errors:?}", k.name);
            let items: Vec<ScriptValue> = match built.as_array() {
                Some(a) => (0..vm.bx.heap.array_len(a)).map(|i| vm.bx.heap.array_index(a, i, NoTrap)).collect(),
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
        }
        assert!(total >= 20, "{total} passes");
        for name in ["DrawGraphAccum", "DrawGraphMerge", "DrawGraphAverage", "DrawGraphError", "DrawGraphErrorMax", "DrawGraphPass"] {
            let shader = shader_object(vm, name, format!("use mod.draw\nmod.draw.{name}{{}}\n"));
            verify_shader_pack_entry(vm, shader).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    });
}
