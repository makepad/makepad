//! The bounded, pure document contract (`bounded.rs`): a document's load
//! and each per-frame call run under instruction, time and heap limits
//! that bail loudly, and after the load the document is frozen, so a frame
//! call can neither keep state nor depend on the order frames run in.

use makepad_script::*;
use std::time::{Duration, Instant};

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host, bx: Box::new(ScriptVmBase::new()) }
}

fn script(name: &str, code: &str) -> ScriptMod {
    ScriptMod { file: format!("doc_contract_{name}"), code: code.to_string(), ..Default::default() }
}

fn call(vm: &mut ScriptVm, root: ScriptValue, name: LiveId, args: &[ScriptValue]) -> (ScriptValue, EvalReport, Vec<String>) {
    let f = vm.bx.heap.value(root.as_object().unwrap(), name.into(), NoTrap);
    let (v, report) = vm.eval_bounded(&EvalLimits::DOC_FRAME, |vm| vm.call(f, args));
    (v, report, vm.take_errors())
}

#[test]
fn runaway_loads_hit_a_limit_and_report_it() {
    for (name, code) in [
        ("spin", "loop {}\n;"),
        ("spin_caught", "try { loop {} } catch { 1 }\n;"),
        ("recursion", "fn f(x) { f(x + 1) + 1 }\nf(0)\n;"),
        ("allocation", "let a = []\nfor i in 0..100000000 { a.push({x: i}) }\n;"),
    ] {
        let vm = &mut test_vm();
        let limits = EvalLimits { heap_bytes: 8 << 20, ..EvalLimits::DOC_LOAD.with_instructions(5_000_000) };
        let t0 = Instant::now();
        let (_, report) = vm.eval_bounded(&limits, |vm| vm.eval(script(name, code)));
        assert!(report.limit.is_some(), "{name}: no limit reported ({report:?}, errors {:?})", vm.take_errors());
        assert!(t0.elapsed() < Duration::from_secs(5), "{name} took {:?}", t0.elapsed());
    }
}

#[test]
fn the_time_limit_is_a_backstop_behind_the_instruction_count() {
    let vm = &mut test_vm();
    let limits = EvalLimits { time: Duration::from_millis(20), ..EvalLimits::DOC_LOAD.with_instructions(usize::MAX / 2) };
    let t0 = Instant::now();
    let (_, report) = vm.eval_bounded(&limits, |vm| vm.eval(script("time", "loop {}\n;")));
    assert!(report.limit.as_deref().is_some_and(|l| l.contains("time")), "{report:?}");
    assert!(t0.elapsed() < Duration::from_millis(500));
    // The previous (absent) run budget is restored.
    assert!(vm.bx.run_budget.is_none());
}

const DOC: &str = "
var count = 0
let state = {n: 0}
let list = [1, 2, 3]
let base = 10
{
    pure: fn(t) { let o = {v: base + t} o.v * 2 }
    counter: fn(t) { count = count + 1 count }
    poke: fn(t) { state.n = t state.n }
    grow: fn(t) { list.push(t) list.len() }
    fresh: fn(t) { let a = [] a.push(t) a.push(base) a.len() + t }
}
";

#[test]
fn a_frozen_document_cannot_keep_state_between_calls() {
    let vm = &mut test_vm();
    let (root, report) = vm.eval_bounded(&EvalLimits::DOC_LOAD, |vm| vm.eval(script("doc", DOC)));
    assert!(report.limit.is_none() && vm.take_errors().is_empty());
    let (objects, _) = vm.freeze_document(&[root]);
    assert!(objects > 3, "froze {objects} objects");

    // Pure calls work and give the same result whatever the order.
    let pure = |vm: &mut ScriptVm, t: f64| call(vm, root, id!(pure), &[t.into()]).0.as_number();
    let forward: Vec<_> = [0.0, 1.0, 2.0].iter().map(|t| pure(vm, *t)).collect();
    let backward: Vec<_> = [2.0, 1.0, 0.0].iter().map(|t| pure(vm, *t)).rev().collect();
    assert_eq!(forward, vec![Some(20.0), Some(22.0), Some(24.0)]);
    assert_eq!(forward, backward);
    // Fresh containers made inside a call are the call's own.
    assert_eq!(call(vm, root, id!(fresh), &[1.0f64.into()]).0.as_number(), Some(3.0));

    // Writes into the document are errors, and repeat calls see no change.
    for name in [id!(counter), id!(poke), id!(grow)] {
        let (first, _, errors) = call(vm, root, name, &[5.0f64.into()]);
        assert!(!errors.is_empty(), "{name}: the write into the frozen document was not refused (got {first:?})");
        let (_, _, again) = call(vm, root, name, &[6.0f64.into()]);
        assert!(!again.is_empty());
    }
    let state = vm.bx.heap.value(root.as_object().unwrap(), id!(poke).into(), NoTrap);
    assert!(state.as_object().is_some());
}

#[test]
fn freezing_a_document_leaves_the_host_modules_alone() {
    let vm = &mut test_vm();
    let (root, _) = vm.eval_bounded(&EvalLimits::DOC_LOAD, |vm| vm.eval(script("doc2", "use mod.math.*\n{ f: fn(t) { sin(t) + abs(t) } }")));
    vm.freeze_document(&[root]);
    // A later document in the same VM still builds and mutates its own state.
    let (v, report) = vm.eval_bounded(&EvalLimits::DOC_LOAD, |vm| vm.eval(script("doc3", "use mod.math.*\nvar a = [] a.push(abs(-2)) let o = {x: 1} o.x = 3 a[0] + o.x")));
    assert!(report.limit.is_none(), "{report:?}");
    assert!(vm.take_errors().is_empty());
    assert_eq!(v.as_number(), Some(5.0));
    assert_eq!(call(vm, root, id!(f), &[0.0f64.into()]).0.as_number(), Some(0.0));
}

#[test]
fn frame_limits_bound_each_call() {
    let vm = &mut test_vm();
    let (root, _) = vm.eval_bounded(&EvalLimits::DOC_LOAD, |vm| vm.eval(script("doc4", "{ spin: fn(t) { loop {} } fine: fn(t) { t + 1 } }")));
    vm.freeze_document(&[root]);
    let t0 = Instant::now();
    let (_, report, _) = call(vm, root, id!(spin), &[0.0f64.into()]);
    assert!(report.limit.is_some());
    assert!(t0.elapsed() < Duration::from_secs(2));
    // The VM is usable after a limit.
    let (v, report, errors) = call(vm, root, id!(fine), &[1.0f64.into()]);
    assert_eq!(v.as_number(), Some(2.0), "{report:?} {errors:?}");
}


#[test]
fn host_natives_that_build_strings_work_under_the_bounds() {
    // A host native with no allocation preflight: an Octoscript budget
    // refuses it; a document evaluation charges its real length instead.
    let vm = &mut test_vm();
    let module = vm.new_module(id!(doctest));
    vm.add_method(module, id!(greet), script_args!(), |vm, _args| vm.new_string_with(|_, s| s.push_str("hello, document")));
    let (v, report) = vm.eval_bounded(&EvalLimits::DOC_LOAD, |vm| vm.eval(script("strings", "use mod.doctest.*\ngreet()")));
    assert!(report.limit.is_none(), "{report:?} {:?}", vm.take_errors());
    assert_eq!(vm.string_with(v, |_, s| s.to_string()).as_deref(), Some("hello, document"));
    let (_, octo) = vm.with_heap_allocation_limit(1 << 20, |vm| vm.eval(script("strings2", "use mod.doctest.*\ngreet()")));
    assert!(octo.exceeded, "outside eval_bounded the strict budget still refuses it");
}
