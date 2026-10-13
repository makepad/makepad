//! `use mod.x.*` brings a module's public names into a file's scope and
//! leaves its private markers (`__*` keys) in the module.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn eval(vm: &mut ScriptVm, code: &str) -> ScriptValue {
    vm.capture_errors();
    let v = vm.eval(ScriptMod::new("", "", "glob.splash", 0, 0, code, &[]));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{errors:?}");
    v
}

#[test]
fn a_glob_skips_private_keys() {
    let mut vm = test_vm();
    vm.new_module(id!(lib));
    eval(&mut vm, "mod.lib.shown = 1\nmod.lib.__hidden = 2\nnil");
    // The public name arrives; the private one does not (it reads as an error value).
    let seen = eval(&mut vm, "use mod.lib.*\nshown");
    assert_eq!(seen.as_number(), Some(1.0));
    vm.capture_errors();
    let hidden = vm.eval(ScriptMod::new("", "", "glob2.splash", 0, 0, "use mod.lib.*\n__hidden", &[]));
    let errors = vm.take_errors();
    assert!(!errors.is_empty() && hidden.as_number() != Some(2.0), "{errors:?} {hidden:?}");
    // Reached through the module it is still there.
    let direct = eval(&mut vm, "mod.lib.__hidden");
    assert_eq!(direct.as_number(), Some(2.0));
}
