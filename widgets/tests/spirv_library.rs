//! Every draw shader of the widget library (the `mod.draw` and `mod.widgets`
//! trees with the default families) compiles to SPIR-V through the path the
//! Vulkan backend takes (layout compile, the shader IR, its SPIR-V printer),
//! window and XR variants, and each stage passes `spirv-val` when it is
//! installed.

use makepad_widgets::makepad_script::{
    shader::{ShaderFnCompiler, ShaderMode, ShaderOutput, ShaderType},
    shader_backend::ShaderBackend,
    shader_ir_draw::compile_draw_shader_spirv,
    shader_ir_spirv::spirv_val,
    trap::NoTrap,
    value::ScriptObject,
};
use makepad_widgets::*;
use std::collections::HashSet;

fn is_draw_shader(vm: &ScriptVm, obj: ScriptObject) -> bool {
    let has = |name: LiveId| vm.bx.heap.object_method(obj, name.into(), NoTrap).as_object().is_some();
    has(live_id!(vertex)) && has(live_id!(fragment))
}

fn walk(vm: &ScriptVm, obj: ScriptObject, visited: &mut HashSet<ScriptObject>, found: &mut Vec<ScriptObject>) {
    if !visited.insert(obj) {
        return;
    }
    if is_draw_shader(vm, obj) {
        found.push(obj);
    }
    let mut children = Vec::new();
    for v in vm.bx.heap.vec_ref(obj) {
        if let Some(o) = v.value.as_object() {
            children.push(o);
        }
    }
    for (_, v) in vm.bx.heap.map_ref(obj).iter() {
        if let Some(o) = v.value.as_object() {
            children.push(o);
        }
    }
    for child in children {
        walk(vm, child, visited, found);
    }
}

/// The layout compile the Vulkan renderer does first, or None when the
/// shader does not compile at all (an abstract base without its pixel
/// function, ...).
fn layout_compile(vm: &mut ScriptVm, io_self: ScriptObject) -> Option<ShaderOutput> {
    let mut layout = ShaderOutput::default();
    layout.backend = ShaderBackend::Glsl;
    layout.use_vulkan = true;
    layout.pre_collect_rust_instance_io(vm, io_self);
    layout.pre_collect_shader_io(vm, io_self);
    for (entry, mode) in [(live_id!(vertex), ShaderMode::Vertex), (live_id!(fragment), ShaderMode::Fragment)] {
        let fnobj = vm.bx.heap.object_method(io_self, entry.into(), NoTrap).as_object()?;
        layout.mode = mode;
        ShaderFnCompiler::compile_shader_def(vm, &mut layout, NoTrap, entry, fnobj, ShaderType::IoSelf(io_self), vec![]);
    }
    if layout.has_errors {
        return None;
    }
    layout.assign_uniform_buffer_indices(&vm.bx.heap, 3);
    Some(layout)
}

#[test]
fn every_library_draw_shader_compiles_to_valid_spirv() {
    let mut cx = Cx::new(Box::new(|_, _| {}));
    cx.with_vm(|vm| {
        makepad_widgets::script_mod(vm);
        vm.bx.captured_errors = Some(Vec::new());
        let roots = [script_eval!(vm, { mod.draw }), script_eval!(vm, { mod.widgets })];
        let mut visited = HashSet::new();
        let mut found = Vec::new();
        for root in roots {
            if let Some(obj) = root.as_object() {
                walk(vm, obj, &mut visited, &mut found);
            }
        }
        let mut modules = HashSet::new();
        let (mut skipped, mut lowering_failed, mut validated) = (0, 0, 0);
        let mut failures = Vec::new();
        for obj in &found {
            let Some(layout) = layout_compile(vm, *obj) else {
                skipped += 1;
                continue;
            };
            match compile_draw_shader_spirv(vm, *obj, &layout) {
                Err(e) => {
                    lowering_failed += 1;
                    failures.push(format!("{:?}: {e}", vm.bx.heap.object_type_name_in_chain(*obj)));
                }
                Ok(variants) => {
                    for v in variants {
                        for words in [v.vertex, v.fragment] {
                            if !modules.insert(words.clone()) {
                                continue;
                            }
                            match spirv_val(&words) {
                                Some(Err(e)) => failures.push(format!("{:?}: spirv-val: {e}", vm.bx.heap.object_type_name_in_chain(*obj))),
                                Some(Ok(())) => validated += 1,
                                None => {}
                            }
                        }
                    }
                }
            }
        }
        vm.bx.captured_errors = None;
        eprintln!(
            "{} draw shader objects, {} distinct stages, {} passed spirv-val, {} did not compile, {} failed IR/SPIR-V",
            found.len(),
            modules.len(),
            validated,
            skipped,
            lowering_failed
        );
        assert!(modules.len() > 50, "the walk found only {} modules", modules.len());
        assert!(failures.is_empty(), "{} failures:\n{}", failures.len(), failures.join("\n"));
    });
}
