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

/// A glob's names come in at once (one reserve, no charge per name), with the order rules of
/// one `let` per name: a later glob shadows an earlier one and a name defined before it, a name
/// defined after it shadows the glob, and a budgeted VM (each name charged) reads the same.
#[test]
fn a_glob_shadows_and_is_shadowed_in_order() {
    let names = |m: &str, n: usize, v: usize| (0..n).map(|i| format!("mod.{m}.n{i} = {v}")).collect::<Vec<_>>().join("\n");
    let code = "let n3 = 7\nuse mod.one.*\nlet before = n3 + n29\nuse mod.two.*\nlet n5 = 9\n[before, n3, n29, n30, n5, n0]";
    let check = |vm: &mut ScriptVm| {
        vm.new_module(id!(one));
        vm.new_module(id!(two));
        // 40 names: past the scope map's spill point; two holds the first 30 of them again
        eval(vm, &format!("{}\n{}\nnil", names("one", 40, 1), names("two", 30, 2)));
        let v = eval(vm, code);
        let arr = v.as_array().expect("an array");
        let got: Vec<Option<f64>> = (0..6).map(|i| vm.array_index_trap(arr, i).as_number()).collect();
        // before: one's n3 and n29 (1 + 1); then two's n3, n29; one's n30; the let's n5; two's n0
        assert_eq!(got, [Some(2.0), Some(2.0), Some(2.0), Some(1.0), Some(9.0), Some(2.0)]);
    };
    check(&mut test_vm());
    let mut vm = test_vm();
    let ((), report) = vm.with_heap_allocation_limit(64 << 20, |vm| check(vm));
    assert!(!report.exceeded, "{report:?}");
}
