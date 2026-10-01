//! The module census keeps every module whose definitions a used module
//! names, so a web build cut by it still registers all it evaluates.
use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host, bx: Box::new(ScriptVmBase::new()) }
}

fn script(module_path: &str, file: &str, code: &str) -> ScriptMod {
    ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: module_path.to_string(),
        file: file.to_string(),
        line: 0,
        column: 0,
        code: code.to_string(),
        values: vec![],
    }
}

fn register(vm: &mut ScriptVm, module_path: &'static str, code: &str) {
    vm.census_begin(module_path);
    vm.eval(script(module_path, module_path, code));
    vm.census_end();
}

#[test]
fn a_spread_definition_is_used_by_the_module_that_spreads_it() {
    let vm = &mut test_vm();
    vm.new_module(id!(cz));
    vm.census_record();
    // `vector` is only ever spread by `svg` (its fields are copied at
    // registration: no heap edge leads to it); `unused` nobody names.
    register(vm, "t::vector", "mod.cz.Vector = {fill: 1}");
    register(vm, "t::svg", "mod.cz.Svg = {stroke: 2, ..mod.cz.Vector}");
    register(vm, "t::unused", "mod.cz.Unused = {x: 3}");
    // The app holds an Svg.
    let app = vm.eval(script("app", "app", "let held = mod.cz.Svg{stroke: 4}\nheld"));
    if let Some(obj) = app.as_object() {
        std::mem::forget(vm.bx.heap.new_object_ref(obj));
    }
    let used = makepad_script::census::census_used(vm).expect("recording");
    assert!(used.used.contains_key("t::svg"), "{used:?}");
    assert!(used.used.contains_key("t::vector"), "a spread definition must stay: {used:?}");
    assert!(!used.used.contains_key("t::unused"), "{used:?}");
}

#[test]
fn the_app_crate_is_a_root_with_everything_it_names() {
    let vm = &mut test_vm();
    vm.new_module(id!(cz));
    vm.census_record();
    register(vm, "lib::window", "mod.cz.Window = {title: 1}");
    register(vm, "lib::unused", "mod.cz.Unused = {x: 3}");
    // The app registers its UI inside its own script_mod, and Rust holds it
    // from there on: no walk from the app's objects starts at it.
    vm.census_begin("app");
    let ui = vm.eval(script("app", "app", "mod.cz.Ui = mod.cz.Window{title: 2}\nmod.cz.Ui"));
    if let Some(obj) = ui.as_object() {
        std::mem::forget(vm.bx.heap.new_object_ref(obj));
    }
    vm.census_end();
    let used = makepad_script::census::census_used_from(vm, &["app"]).expect("recording");
    assert!(used.used.contains_key("app"), "{used:?}");
    assert!(used.used.contains_key("lib::window"), "what the app names is used: {used:?}");
    assert!(!used.used.contains_key("lib::unused"), "{used:?}");
}
