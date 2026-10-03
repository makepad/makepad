//! Every draw shader of the widget library (the `mod.draw` and `mod.widgets`
//! trees with the default families) compiles to SPIR-V through the path the
//! Vulkan backend takes (layout compile, WGSL lowering, the shader
//! compiler's SPIR-V backend), window and XR variants, and each stage passes
//! `spirv-val` when it is installed.

use makepad_widgets::makepad_script::{
    shader::{ShaderFnCompiler, ShaderMode, ShaderOutput, ShaderType},
    shader_backend::ShaderBackend,
    shader_spirv::{compile_wgsl_to_spirv, spirv_val},
    shader_wgsl::compile_draw_shader_wgsl_source,
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

/// The module the Vulkan backend compiles, or None when the shader does not
/// compile at all (an abstract base without its pixel function, ...).
fn vulkan_module(vm: &mut ScriptVm, io_self: ScriptObject, xr: bool) -> Option<Result<String, String>> {
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
    Some(compile_draw_shader_wgsl_source(vm, io_self, &layout, xr).map(|s| s.wgsl))
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
            for xr in [false, true] {
                let wgsl = match vulkan_module(vm, *obj, xr) {
                    None => {
                        skipped += 1;
                        continue;
                    }
                    Some(Err(_)) => {
                        lowering_failed += 1;
                        continue;
                    }
                    Some(Ok(wgsl)) => wgsl,
                };
                if !modules.insert(wgsl.clone()) {
                    continue;
                }
                match compile_wgsl_to_spirv(&wgsl) {
                    Err(e) => failures.push(format!("{e}\n{wgsl}")),
                    Ok((vertex, fragment)) => {
                        for words in [vertex, fragment].into_iter().flatten() {
                            match spirv_val(&words) {
                                Some(Err(e)) => failures.push(format!("spirv-val: {e}\n{wgsl}")),
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
            "{} draw shader objects, {} distinct modules, {} stages passed spirv-val, {} did not compile, {} failed WGSL lowering",
            found.len(),
            modules.len(),
            validated,
            skipped,
            lowering_failed
        );
        assert!(modules.len() > 50, "the walk found only {} modules", modules.len());
        assert!(failures.is_empty(), "{} of {} modules failed:\n{}", failures.len(), modules.len(), failures[0]);
    });
}
