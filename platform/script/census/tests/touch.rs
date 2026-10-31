//! Splash use by touches: a definition is used when the run reads it outside
//! the registrations, in any VM, not when Rust merely holds it.
use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn script(module_path: &str, file: &str, code: &str) -> ScriptMod {
    ScriptMod::new("", module_path, file, 1, 1, code, &[])
}

fn register(vm: &mut ScriptVm, module_path: &'static str, code: &str) {
    vm.census_begin(module_path);
    vm.eval(script(module_path, module_path, code));
    vm.census_end();
}

fn uses(blocks: &[makepad_script_census::BlockUse], module: &str) -> Vec<(String, Option<bool>)> {
    blocks
        .iter()
        .filter(|b| b.module_path == module)
        .flat_map(|b| b.statements.iter().map(|(s, u)| (s.path.join("."), *u)))
        .collect()
}

// One test: the touch record is process-wide.
#[test]
fn touches_mark_what_a_run_read_in_any_vm() {
    let vm = &mut test_vm();
    makepad_script_census::touch_record(vm);
    vm.new_module(id!(tz));
    // Base is built on by Flat at registration (no use); Held is held by Rust only.
    register(vm, "t::base", "mod.tz.Base = {fill: 1}\nmod.tz.Flat = mod.tz.Base{fill: 2}\nmod.tz.Alias = mod.tz.Flat");
    register(vm, "t::held", "mod.tz.Held = {x: 3}\nmod.tz.Spare = {y: 4}");
    // Rust takes what it holds at registration (a registry's lookup is no use either).
    vm.census_begin("t::rust");
    let held = vm.bx.heap.value(vm.bx.heap.module(id!(tz)), id!(Held).into(), NoTrap).as_object().unwrap();
    std::mem::forget(vm.bx.heap.new_object_ref(held));
    vm.census_end();
    // A document made after the recording began has its own VM: it reads Alias (= Flat).
    {
        let doc = &mut test_vm();
        doc.new_module(id!(tz));
        register(doc, "t::base", "mod.tz.Base = {fill: 1}\nmod.tz.Flat = mod.tz.Base{fill: 2}\nmod.tz.Alias = mod.tz.Flat");
        doc.eval(script("", "doc", "let a = mod.tz.Alias{fill: 5}\na.fill"));
    }
    // The main VM reads Spare's field.
    vm.eval(script("", "app", "mod.tz.Spare.y"));
    let blocks = makepad_script_census::touch_use(vm);
    let base = uses(&blocks, "t::base");
    // Flat and its alias were read; Base is named by Flat's text, so it stays.
    assert_eq!(base, vec![("mod.tz.Base".into(), Some(true)), ("mod.tz.Flat".into(), Some(true)), ("mod.tz.Alias".into(), Some(true))], "{base:?}");
    let held = uses(&blocks, "t::held");
    // Held is only held (but it is its block's first statement, not the last, so it can go);
    // Spare was read (and is the last statement, which always stays).
    assert_eq!(held, vec![("mod.tz.Held".into(), Some(false)), ("mod.tz.Spare".into(), Some(true))], "{held:?}");
}
