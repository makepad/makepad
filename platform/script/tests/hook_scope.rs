//! Host->script hook lookups against a Splash-shaped body: the script is
//! wrapped in an auto-closed object literal, and a `let`/`fn` that shadows
//! an existing name opens a child scope the module scope cannot see into.
//! The body remembers the scope it ended in for exactly that.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(0i32));
    let std = Box::leak(Box::new(0i32));
    ScriptVm { host, std, bx: Box::new(ScriptVmBase::new()) }
}

fn scopes(vm: &mut ScriptVm, file: &str) -> (ScriptObject, Option<ScriptObject>) {
    let bodies = vm.bx.code.bodies.borrow();
    bodies.iter().find_map(|body| match &body.source {
        ScriptSource::Mod(m) if m.file == file => {
            Some((body.scope.as_object(), body.end_scope.as_ref().map(|s| s.as_object())))
        }
        _ => None,
    }).expect("body")
}

#[test]
fn hooks_resolve_in_the_scope_the_body_ended_in() {
    let mut vm = test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    // `fs` is defined twice: the second `let` shadows and opens a child scope.
    let code = "let fs = 1\n{height: 1, let fs = 2\nfn before(){ 7 }\nfn on_x(){ 42 }\n{}\n";
    vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: "hook_scope".to_string(),
            line: 0, column: 0,
            code: code.to_string(),
            values: vec![],
        })
    });
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "eval errored: {errors:?}");

    let (module, end) = scopes(&mut vm, "hook_scope");
    let end = end.expect("the body recorded the scope it ended in");
    let from_module = vm.bx.heap.scope_value(module, id!(on_x), NoTrap);
    assert!(from_module.is_nil() || from_module.is_err(), "the module scope should not see past the shadowing let");
    let on_x = vm.bx.heap.scope_value(end, id!(on_x), NoTrap);
    assert!(!on_x.is_nil() && !on_x.is_err(), "on_x not found in the end scope");
    let result = vm.with_instruction_limit(500_000, |vm| vm.call(on_x, &[]));
    assert_eq!(format!("{result:?}"), "42", "on_x returned {result:?}");
    let before = vm.bx.heap.scope_value(end, id!(before), NoTrap);
    assert!(!before.is_nil() && !before.is_err(), "before not found in the end scope");
}
