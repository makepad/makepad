//! Glob snapshots keep fast hits while misses see public names added to their sources later.

use makepad_script::*;
use std::collections::HashSet;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn eval(vm: &mut ScriptVm, file: &str, code: &str) -> ScriptValue {
    vm.capture_errors();
    let value = vm.eval(ScriptMod::new("", "", file, 0, 0, code, &[]));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{file}: evaluation has no errors: {errors:?}");
    value
}

#[test]
fn glob_misses_read_late_public_names_without_scope_bindings() {
    let mut vm = test_vm();
    let source = vm.new_module(id!(globtest));
    vm.new_module(id!(globother));
    vm.bx.heap.set_value_def(source, id!(Known).into(), 3.0f64.into());

    eval(&mut vm, "glob_bindings.splash", "let Own = 1\nuse mod.globtest.*\nlet Tail = 2\nOwn + Tail + Known");
    let end = vm.end_scope_of("glob_bindings.splash").expect("scope inspection: the document retains its end scope");
    let mut keys = Vec::new();
    let mut at = Some(end);
    while let Some(scope) = at {
        keys.extend(vm.bx.heap.map_entries(scope).iter().map(|entry| entry.key.as_id().expect("scope inspection: every binding has an identifier key")));
        at = vm.bx.heap.proto(scope).as_object();
    }
    assert!(keys.iter().all(|key| !vm.bx.heap.is_private_name(*key)), "scope inspection: no private marker binding is present");
    assert_eq!(keys.len(), 4, "scope inspection: only mod, two lets and the glob's one name are bound");
    keys.sort_unstable();
    let mut expected = vec![id!(mod), id!(Own), id!(Tail), id!(Known)];
    expected.sort_unstable();
    assert_eq!(keys, expected, "scope inspection: the bindings are exactly the declared and imported names");

    vm.capture_errors();
    let (_, body) = vm.eval_module(
        ScriptMod::new("", "", "glob_exports.splash", 0, 0, "use mod.globtest.*\nlet FirstExport = 1\nlet SecondExport = 2\nnil", &[]),
        &[],
    );
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "module exports: evaluation has no errors: {errors:?}");
    let exports = vm.scope_exports(body, &[source]).expect("module exports: the module has an end scope");
    let mut keys: Vec<_> = vm.bx.heap.map_entries(exports).iter().map(|entry| entry.key.as_id().expect("module exports: every export has an identifier key")).collect();
    keys.sort_unstable();
    let mut expected = vec![id!(FirstExport), id!(SecondExport)];
    expected.sort_unstable();
    assert_eq!(keys, expected, "module exports: only the module's own lets are exported");

    let late = eval(&mut vm, "glob_late.splash", "use mod.globtest.*\nmod.globtest.Late = 5\nLate");
    assert_eq!(late.as_number(), Some(5.0), "late binding: a name added after use resolves by its bare name");

    let late_fn = eval(
        &mut vm,
        "glob_closure.splash",
        "use mod.globtest.*\nlet read_late = fn() { return Late2 }\nmod.globtest.Late2 = 6\nread_late()",
    );
    assert_eq!(late_fn.as_number(), Some(6.0), "closure: a fn defined before the late name reads it when called later");

    let quiet = eval(&mut vm, "glob_quiet.splash", "use mod.globtest.*\nmod.globtest.Late3 = 7\nLate3 ?? 0");
    assert_eq!(quiet.as_number(), Some(7.0), "quiet read: `??` of a late name gives its value, without a trap");

    let early = eval(&mut vm, "glob_early.splash", "let Early = 1\nuse mod.globtest.*\nmod.globtest.Early = 2\nEarly");
    assert_eq!(early.as_number(), Some(1.0), "existing binding: a scope hit wins over a name later added to the source");

    let both = eval(
        &mut vm,
        "glob_two.splash",
        "use mod.globtest.*\nuse mod.globother.*\nmod.globtest.FirstLate = 8\nmod.globother.SecondLate = 9\nmod.globtest.SharedLate = 10\nmod.globother.SharedLate = 11\n[FirstLate, SecondLate, SharedLate]",
    );
    let both = both.as_array().expect("two globs: the script returns both late names and their shared name");
    assert_eq!(vm.array_index_trap(both, 0).as_number(), Some(8.0), "two globs: the first source's late name resolves");
    assert_eq!(vm.array_index_trap(both, 1).as_number(), Some(9.0), "two globs: the second source's late name resolves");
    assert_eq!(vm.array_index_trap(both, 2).as_number(), Some(11.0), "two globs: the last use wins for a shared late name");

    vm.capture_errors();
    let hidden = vm.eval(ScriptMod::new("", "", "glob_private_late.splash", 0, 0, "use mod.globtest.*\nmod.globtest.__hidden = 1\n__hidden", &[]));
    let errors = vm.take_errors();
    assert!(hidden.as_err().is_some() && errors.iter().any(|error| error.contains("variable __hidden not found in scope")), "private late name: the bare name remains an error: {errors:?} {hidden:?}");

    vm.capture_errors();
    let missing = vm.eval(ScriptMod::new("", "", "glob_missing.splash", 0, 0, "use mod.globtest.*\nNeverDefined", &[]));
    let errors = vm.take_errors();
    assert!(missing.as_err().is_some() && errors.iter().any(|error| error.contains("not found in scope")), "undefined name: the existing not-found error remains: {errors:?} {missing:?}");

    // Only the closure's glob table can keep this source alive once its module entry is gone.
    let gc_source = vm.new_module(id!(globgc));
    vm.capture_errors();
    let closure = vm.eval_transient(ScriptMod::new("", "", "glob_gc.splash", 0, 0, "use mod.globgc.*\nfn() { return AfterGc }", &[]));
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "GC setup: defining the closure has no errors: {errors:?}");
    let closure = vm.bx.heap.new_fn_ref(closure.as_object().expect("GC setup: the script returns a fn"));
    vm.bx.heap.set_value_def(gc_source, id!(AfterGc).into(), 12.0f64.into());
    let modules = vm.bx.heap.modules;
    vm.bx.heap.set_value_def(modules, id!(globgc).into(), NIL);
    let mut reached = HashSet::new();
    vm.bx.heap.reach_count(&[closure.as_object()], &mut reached);
    assert!(reached.contains(&gc_source), "reachability: the closure reaches its detached glob source");
    vm.gc();
    assert!(vm.bx.heap.object_alive(gc_source), "GC retention: the glob table keeps its detached source alive");
    vm.capture_errors();
    let after_gc = vm.call(closure.as_object().into(), &[]);
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "GC lookup: the late name resolves without errors after collection: {errors:?}");
    assert_eq!(after_gc.as_number(), Some(12.0), "GC lookup: the closure still reads its source's late name");
    drop(closure);
    eval(&mut vm, "glob_gc_release.splash", "nil");
    vm.gc();
    assert!(!vm.bx.heap.object_alive(gc_source), "GC release: the table does not root a source once its scope dies");
}
