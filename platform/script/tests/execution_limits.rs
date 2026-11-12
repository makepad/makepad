//! Regression tests for the operand-stack and call-frame caps that constrained
//! hosts install per evaluation (`with_stack_value_limit`,
//! `with_call_frame_limit`), for `clear_execution_limit_failures`, and for
//! `ScriptNative::clear_type_methods`. Limit hits are uncatchable VM bails:
//! `try`/`catch` cannot recover them, but the next evaluation on the same VM
//! starts from a clean root state. Also sliced runs (`ScriptRunBudget::slice`):
//! a run that yields at its soft deadline and is resumed until it ends.

use makepad_script::*;
use std::time::Duration;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new((), ())));
    let mut vm = ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.bx.captured_errors = Some(Vec::new());
    vm
}

fn eval(vm: &mut ScriptVm, name: &str, code: &str) -> ScriptValue {
    vm.with_instruction_limit(100_000, |vm| {
        vm.eval(ScriptMod::new("", "", &format!("execution_limits_{name}.octoscript"), 0, 0, &format!("{code}\n;"), &[]))
    })
}

/// `take_errors` moves the captured sink out; re-arm it the way a host does
/// before its next evaluation so later diagnostics stay observable.
fn take_errors(vm: &mut ScriptVm) -> Vec<String> {
    let errors = ScriptVm::take_errors(vm);
    vm.bx.captured_errors = Some(Vec::new());
    errors
}

fn errors_mention(vm: &mut ScriptVm, needle: &str) -> bool {
    take_errors(vm).iter().any(|e| e.contains(needle))
}

#[test]
fn operand_stack_limit_is_uncatchable_and_the_vm_recovers() {
    let mut vm = test_vm();
    let exceeded = vm.with_stack_value_limit(1, |vm| eval(vm, "stack", "try (1 + 2) catch 99"));
    assert!(exceeded.is_err(), "{exceeded:?}");
    assert!(errors_mention(&mut vm, "operand stack limit exceeded"));
    assert!(!vm.thread().is_paused());

    let recovered = eval(&mut vm, "stack_recover", "2");
    assert_eq!(recovered.as_number(), Some(2.0));
    assert!(take_errors(&mut vm).is_empty());
}

#[test]
fn operand_stack_limit_restores_the_default_ceiling_after_the_scope() {
    let mut vm = test_vm();
    let _ = vm.with_stack_value_limit(1, |vm| eval(vm, "stack_scope", "1 + 2"));
    let _ = take_errors(&mut vm);
    let value = eval(&mut vm, "stack_after", "1 + 2");
    assert_eq!(value.as_number(), Some(3.0));
}

#[test]
fn call_frame_limit_is_uncatchable_and_the_vm_recovers() {
    let mut vm = test_vm();
    let exceeded = vm.with_call_frame_limit(4, |vm| {
        eval(
            vm,
            "frames",
            "fn recurse(value) {\nrecurse(value + 1)\n}\ntry recurse(0) catch 99",
        )
    });
    assert!(exceeded.is_err(), "{exceeded:?}");
    assert!(errors_mention(&mut vm, "call frame limit exceeded"));

    let recovered = eval(&mut vm, "frames_recover", "2");
    assert_eq!(recovered.as_number(), Some(2.0));
    assert!(take_errors(&mut vm).is_empty());
}

#[test]
fn call_frame_limit_includes_the_root_evaluation_frame() {
    let mut vm = test_vm();
    let root_only = vm.with_call_frame_limit(1, |vm| eval(vm, "root", "2"));
    assert_eq!(root_only.as_number(), Some(2.0));
    assert!(take_errors(&mut vm).is_empty());

    let exceeded =
        vm.with_call_frame_limit(1, |vm| eval(vm, "root_call", "fn one() {\n1\n}\ntry one() catch 99"));
    assert!(exceeded.is_err(), "{exceeded:?}");
    assert!(errors_mention(&mut vm, "call frame limit exceeded"));

    let zero = vm.with_call_frame_limit(0, |vm| eval(vm, "root_zero", "2"));
    assert!(zero.is_err(), "{zero:?}");
    assert!(errors_mention(&mut vm, "call frame limit exceeded"));
}

#[test]
fn nested_limits_only_narrow() {
    let mut vm = test_vm();
    let value = vm.with_call_frame_limit(2, |vm| {
        vm.with_call_frame_limit(100, |vm| {
            eval(vm, "nested", "fn one() {\nfn two() {\n1\n}\ntwo()\n}\ntry one() catch 99")
        })
    });
    assert!(value.is_err(), "{value:?}");
    assert!(errors_mention(&mut vm, "call frame limit exceeded"));
}

#[test]
fn clear_execution_limit_failures_discards_stale_signals() {
    let mut vm = test_vm();
    vm.with_stack_value_limit(1, |vm| {
        vm.thread_mut().push_stack_unchecked(1.into());
        vm.thread_mut().push_stack_unchecked(2.into());
        vm.thread_mut().pop_stack_value();
    });
    vm.clear_execution_limit_failures();
    let value = eval(&mut vm, "stale", "2");
    assert_eq!(value.as_number(), Some(2.0));
    assert!(take_errors(&mut vm).is_empty());
}

#[test]
fn clear_type_methods_removes_primitive_methods() {
    let mut vm = test_vm();
    let before = eval(&mut vm, "string_len_before", "\"abc\".len()");
    assert_eq!(before.as_number(), Some(3.0));
    assert!(take_errors(&mut vm).is_empty());

    let removed = vm
        .bx
        .code
        .native
        .borrow_mut()
        .clear_type_methods(ScriptValueType::REDUX_STRING);
    assert!(removed > 0);
    assert_eq!(
        vm.bx
            .code
            .native
            .borrow_mut()
            .clear_type_methods(ScriptValueType::REDUX_STRING),
        0
    );

    let after = eval(&mut vm, "string_len_after", "\"abc\".len()");
    assert!(after.is_err(), "{after:?}");
    let _ = take_errors(&mut vm);
    // Other types keep their surface.
    let number = eval(&mut vm, "number_ty", "(1).ty()");
    assert!(!number.is_err(), "{number:?}");
}

// ---------------------------------------------------------------------------
// sliced runs

/// Starts a run with `start` under a sliced run's budget that is always past its soft deadline
/// (the clock read every `interval` instructions and after each native), and resumes it until
/// it ends, collecting between slices when `gc`: the run's value and how often it yielded.
fn run_sliced(vm: &mut ScriptVm, interval: u32, gc: bool, start: impl FnOnce(&mut ScriptVm) -> ScriptValue) -> (ScriptValue, usize) {
    vm.set_run_budget(Some(ScriptRunBudget::slice(Duration::ZERO, interval)));
    let mut value = start(vm);
    let mut yields = 0;
    while vm.run_yielded() {
        yields += 1;
        assert!(yields < 1_000_000, "a sliced run that never ends");
        if gc {
            vm.gc();
        }
        vm.set_run_budget(Some(ScriptRunBudget::slice(Duration::ZERO, interval)));
        value = vm.resume();
    }
    vm.set_run_budget(None);
    assert!(!vm.is_paused(), "the run ended on a paused thread");
    (value, yields)
}

/// Loops (one at the top level, one in a fn), try frames (one catching an error every fifth
/// turn), closures over module state, a recursive fn, and natives that call script back
/// (`retain`, `map`). The module defines `compute`; the evals end on `compute()`.
const SLICED_PROGRAM: &str = "use mod.std.assert
let k = 3
let add_k = fn(x) { return x + k }
fn fib(n) { if n < 2 { return n }\n return fib(n - 1) + fib(n - 2) }
let warm = 0
for j in 0..10 { warm = warm + j }
let compute = fn() {
    let caught = 0
    let total = 0
    for i in 0..30 {
        total = total + add_k(i)
        let r = try { if i % 5 == 0 { assert(false) }\n i } catch { caught = caught + 1\n 0 }
        total = total + r
    }
    let kept = [1, 2, 3, 4, 5, 6]
    kept.retain(fn(v) { return v % 2 == 0 })
    let doubled = kept.map(fn(v) { return add_k(v) * 2 })
    return {total: total + warm, caught: caught, fib: fib(12), n: doubled.len(), first: doubled[0], last: doubled[2]}
}";

/// The program's result fields, in a fixed order.
fn sliced_fields(vm: &ScriptVm, value: ScriptValue) -> Vec<Option<f64>> {
    let obj = value.as_object().unwrap_or_else(|| panic!("not an object: {value:?}"));
    [id!(total), id!(caught), id!(fib), id!(n), id!(first), id!(last)]
        .iter()
        .map(|key| vm.heap().value(obj, (*key).into(), NoTrap).as_number())
        .collect()
}

#[test]
fn a_sliced_run_ends_as_the_whole_run_does_through_every_entry() {
    let expected: Vec<Option<f64>> = [930.0, 6.0, 144.0, 3.0, 10.0, 18.0].iter().map(|v| Some(*v)).collect();
    let module = |name: &str| ScriptMod::new("", "", &format!("execution_limits_sliced_{name}.octoscript"), 0, 0, &format!("{SLICED_PROGRAM}\ncompute()\n;"), &[]);
    // (every case: the clock read every instruction, then every 97 with a collection between slices)
    for (interval, gc) in [(1, false), (97, true)] {
        let what = format!("interval {interval}, gc {gc}");
        // eval: the module's value is an eval's
        let vm = &mut test_vm();
        let whole = vm.eval(module("whole"));
        assert_eq!(sliced_fields(vm, whole), expected, "the whole run: {:#?}", take_errors(vm));
        let vm = &mut test_vm();
        let (sliced, yields) = run_sliced(vm, interval, gc, |vm| vm.eval(module("eval")));
        assert!(yields > 10, "{what}: eval yielded only {yields} times");
        assert_eq!(sliced_fields(vm, sliced), expected, "{what}: sliced eval: {:#?}", take_errors(vm));
        assert!(vm.heap().is_from_eval(sliced.as_object().unwrap()), "{what}: the sliced eval's end did not run");
        assert!(take_errors(vm).is_empty(), "{what}: the caught error leaked");

        // the streaming eval a host evaluates whole sources with
        let vm = &mut test_vm();
        let code = format!("{SLICED_PROGRAM}\ncompute()\n;");
        let (streamed, yields) = run_sliced(vm, interval, gc, |vm| vm.eval_with_append_source(module("append"), &code, ScriptObject::ZERO));
        assert!(yields > 10, "{what}: the streaming eval yielded only {yields} times");
        assert_eq!(sliced_fields(vm, streamed), expected, "{what}: sliced streaming eval");
        assert!(vm.heap().is_from_eval(streamed.as_object().unwrap()), "{what}: the streaming eval's end did not run");
        assert!(!vm.bx.silence_errors, "{what}: the streaming eval left errors silenced");

        // a host's call of a script fn
        let vm = &mut test_vm();
        let f = vm.eval(ScriptMod::new("", "", "execution_limits_sliced_fn.octoscript", 0, 0, &format!("{SLICED_PROGRAM}\ncompute\n;"), &[]));
        assert!(f.as_object().is_some(), "{f:?} {:#?}", take_errors(vm));
        let (called, yields) = run_sliced(vm, interval, gc, |vm| vm.call(f, &[]));
        assert!(yields > 10, "{what}: the call yielded only {yields} times");
        assert_eq!(sliced_fields(vm, called), expected, "{what}: sliced call: {:#?}", take_errors(vm));
    }
}

#[test]
fn a_native_calling_script_in_a_sliced_run_gets_the_real_value() {
    let vm = &mut test_vm();
    // Past its deadline at every instruction: the closures `map` calls must still run to their
    // ends (a yield there would hand the native nil), and the outer run yields around the call.
    let (value, yields) = run_sliced(vm, 1, false, |vm| {
        let code = "let a = [1, 2, 3].map(fn(v) { let t = v * 10\n return t + 1 })\na[0] + a[1] * 100 + a[2] * 10000\n;";
        vm.eval(ScriptMod::new("", "", "execution_limits_sliced_native.octoscript", 0, 0, code, &[]))
    });
    assert!(yields > 3, "yielded only {yields} times");
    assert_eq!(value.as_number(), Some(312111.0), "{value:?} {:#?}", take_errors(vm));
}

#[test]
fn the_instruction_limit_counts_across_slices_and_still_bails() {
    let vm = &mut test_vm();
    let (previous, applied) = vm.instruction_limit_begin(5_000);
    let (value, yields) = run_sliced(vm, 1, false, |vm| {
        vm.eval(ScriptMod::new("", "", "execution_limits_sliced_limit.octoscript", 0, 0, "loop {}\n;", &[]))
    });
    vm.instruction_limit_end(previous, applied);
    assert!(value.is_err(), "{value:?}");
    assert!(errors_mention(vm, "instruction limit exceeded"));
    // a resumed slice runs at most two instructions here: the one limit ran out over thousands
    // of slices (a limit restarted per slice would never run out)
    assert!((2_400..=5_100).contains(&yields), "{yields} slices for a 5000-instruction limit");
}

static SLICED_LAZY_RUNS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// A registration longer than any slice: a loop, then the module.
unsafe extern "C-unwind" fn expand_sliced_lazy(vm: *mut ScriptVm) {
    let vm = unsafe { &mut *vm };
    SLICED_LAZY_RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let code = "let t = 0\nfor i in 0..200 { t = t + i }\nmod.sliced_lazy = {x: 5, sum: t}\ntrue\n;";
    vm.eval(ScriptMod::new("", "", "execution_limits_sliced_lazy_mod.octoscript", 0, 0, code, &[]));
}

#[test]
fn a_lazy_module_touched_in_a_sliced_run_expands_whole_and_the_run_slices_on() {
    let vm = &mut test_vm();
    vm.declare_lazy_modules("sliced lazy", &[id!(sliced_lazy)], expand_sliced_lazy);
    let code = "let a = 0\nfor i in 0..20 { a = a + i }\nlet m = mod.sliced_lazy\nlet b = 0\nfor i in 0..20 { b = b + m.x }\na + b + m.sum\n;";
    // Past its deadline at every instruction, collecting between slices (as the widget pump may):
    // the expansion is the host's registration and runs whole inside one slice, and the run's
    // budget is back after it.
    let slice = || Some(ScriptRunBudget::slice(Duration::ZERO, 1));
    vm.set_run_budget(slice());
    let mut value = vm.eval(ScriptMod::new("", "", "execution_limits_sliced_lazy.octoscript", 0, 0, code, &[]));
    let (mut before, mut after) = (0, 0);
    while vm.run_yielded() {
        match SLICED_LAZY_RUNS.load(std::sync::atomic::Ordering::Relaxed) {
            0 => before += 1,
            _ => after += 1,
        }
        assert!(before + after < 1_000_000, "a sliced run that never ends");
        vm.gc();
        vm.set_run_budget(slice());
        value = vm.resume();
    }
    vm.set_run_budget(None);
    assert_eq!(SLICED_LAZY_RUNS.load(std::sync::atomic::Ordering::Relaxed), 1, "the group expanded once");
    assert!(before > 10 && after > 10, "yields before the touch {before}, after it {after}");
    assert_eq!(value.as_number(), Some(190.0 + 100.0 + 19_900.0), "{value:?} {:#?}", take_errors(vm));
    assert!(take_errors(vm).is_empty());
    assert!(!vm.is_paused());
}
