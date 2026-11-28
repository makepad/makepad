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

fn instructions_remaining(vm: &ScriptVm) -> u64 {
    vm.bx.vm_stat(&mut vm_ctl::ScriptCtlReq::new(vm_ctl::STAT_LIMIT_REMAINING))
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

    let body = |file| ScriptMod::new("", "", file, 0, 0, "7\n;", &[]);
    let first_cost = vm.with_instruction_limit(2_000, |vm| {
        let first = vm.with_instruction_limit(256, |vm| vm.eval(body("inner_limit.splash")));
        assert_eq!(first.as_number(), Some(7.0), "the first body completed under the inner limit");
        let used = vm.last_limit_consumed();
        assert!((1..256).contains(&used), "the inner scope reports the first body's cost");
        assert_eq!(instructions_remaining(vm), 2_000 - used, "the outer scope paid for the inner body");
        let second = vm.eval(body("outer_limit.splash"));
        assert_eq!(second.as_number(), Some(7.0), "the second body completed under the outer limit");
        used
    });
    assert_eq!(vm.last_limit_consumed(), 2 * first_cost, "both identical bodies count toward the outer scope");
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
    // Map callbacks can yield at every instruction and still deliver each result exactly once.
    let (value, yields) = run_sliced(vm, 1, false, |vm| {
        let code = "let a = [1, 2, 3].map(fn(v) { let t = v * 10\n return t + 1 })\na[0] + a[1] * 100 + a[2] * 10000\n;";
        vm.eval(ScriptMod::new("", "", "execution_limits_sliced_native.octoscript", 0, 0, code, &[]))
    });
    assert!(yields > 3, "yielded only {yields} times");
    assert_eq!(value.as_number(), Some(312111.0), "{value:?} {:#?}", take_errors(vm));

    let probe = vm.new_module(id!(probe));
    vm.add_method(probe, id_lut!(invoke), script_args_def!(cb = NIL), |vm, args| {
        let cb = script_value!(vm, args.cb);
        let value = vm.call(cb, &[]);
        assert_eq!(value.as_number(), Some(7.0), "the native received the script callback's result");
        assert!(instructions_remaining(vm) < 1_000, "the callback returned with the outer budget still installed");
        value
    });
    let value = vm.with_instruction_limit(1_000, |vm| {
        vm.eval(ScriptMod::new("", "", "callback_limit.splash", 0, 0, "mod.probe.invoke(fn() { return 7 })\nloop {}\n;", &[]))
    });
    assert!(value.is_err(), "the loop after the native callback must bail");
    assert!(errors_mention(vm, "instruction limit exceeded"), "the outer budget stopped the loop after the callback");
}

#[test]
fn the_instruction_limit_counts_across_slices_and_still_bails() {
    let vm = &mut test_vm();
    let (value, yields) = vm.with_instruction_limit(5_000, |vm| {
        let first = vm.eval(ScriptMod::new("", "", "first_limit.splash", 0, 0, "7\n;", &[]));
        assert_eq!(first.as_number(), Some(7.0), "the first body completed");
        assert!((1..5_000).contains(&instructions_remaining(vm)), "the second body inherits the first body's remaining allowance");
        run_sliced(vm, 1, false, |vm| {
            vm.eval(ScriptMod::new("", "", "execution_limits_sliced_limit.octoscript", 0, 0, "loop {}\n;", &[]))
        })
    });
    assert!(value.is_err(), "the second body must bail: {value:?}");
    assert!(errors_mention(vm, "instruction limit exceeded"), "the shared limit stopped the second body");
    assert_eq!(vm.last_limit_consumed(), 5_000, "the two bodies exhausted one allowance");
    assert_eq!(instructions_remaining(vm), u64::MAX, "the exhausted allowance ended with its scope");
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

// ---------------------------------------------------------------------------
// a native that pauses a sliced run

thread_local! {
    /// The probe's pauses still to make, and what `can_pause` answered at each of its calls.
    static WAIT: std::cell::RefCell<(usize, Vec<bool>)> = const { std::cell::RefCell::new((0, Vec::new())) };
}

/// `mod.probe.wait()`: pauses its thread while pauses remain and it may, else answers 7. A
/// paused call is executed again by the resume, so it is a poll.
fn add_wait_probe(vm: &mut ScriptVm) {
    let m = vm.new_module(id!(probe));
    vm.add_method(m, id_lut!(wait), script_args_def!(), |vm, _| {
        let can = vm.can_pause();
        let pause = WAIT.with(|w| {
            let mut w = w.borrow_mut();
            w.1.push(can);
            let pause = can && w.0 > 0;
            w.0 -= pause as usize;
            pause
        });
        if pause {
            vm.pause();
            return NIL;
        }
        7.0.into()
    });
}

/// Starts `start` under a sliced run's budget past its deadline (read every 97 instructions)
/// with `pauses` pauses for the probe to make, and resumes the run's thread while it is paused,
/// by a yield or by the probe, as a host's eval steps do: the run's value and its resumes.
fn run_paused(vm: &mut ScriptVm, pauses: usize, start: impl FnOnce(&mut ScriptVm) -> ScriptValue) -> (ScriptValue, usize) {
    WAIT.with(|w| *w.borrow_mut() = (pauses, Vec::new()));
    let slice = || Some(ScriptRunBudget::slice(Duration::ZERO, 97));
    vm.set_run_budget(slice());
    let thread = vm.current_thread();
    let mut value = start(vm);
    let mut resumes = 0;
    loop {
        vm.set_current_thread(thread);
        if !vm.is_paused() {
            break;
        }
        resumes += 1;
        assert!(resumes < 1_000_000, "a paused run that never ends");
        vm.set_run_budget(slice());
        value = vm.resume();
    }
    vm.set_run_budget(None);
    (value, resumes)
}

fn append_source(file: &str) -> ScriptMod {
    ScriptMod::new("", "", file, 1, 0, "", &[])
}

#[test]
fn a_native_pausing_a_sliced_streaming_eval_leaves_the_eval_end_to_the_resume() {
    let code = "let a = 20\nlet w = mod.probe.wait()\nlet m = [1, 2].map(fn(v) { return mod.probe.wait() + v })\nlet b = a + w + m[0] + m[1]\n{b: b}\n;";
    let vm = &mut test_vm();
    add_wait_probe(vm);
    let (previous, applied) = vm.instruction_limit_begin(5_000);
    let (value, resumes) = run_paused(vm, 3, |vm| {
        vm.with_instruction_limit(1_000, |vm| vm.eval_with_append_source(append_source("pause_streaming.splash"), code, ScriptObject::ZERO))
    });
    let before_resume = vm.last_limit_consumed();
    vm.instruction_limit_end(previous, applied);
    let total = vm.last_limit_consumed();
    assert!(before_resume < total && total < 1_000, "the parent reports the fuel spent before and after the pause");
    assert_eq!(instructions_remaining(vm), u64::MAX, "the successful continuation released its remaining fuel");
    let calls = WAIT.with(|w| w.borrow().1.clone());
    // Both map callbacks run in the outer interpreter. No synchronous native is waiting below them.
    assert_eq!(calls, [true, true, true, true, true, true], "can_pause includes the two map callbacks");
    assert!(resumes >= 3, "resumed {resumes} times for three pauses");
    let obj = value.as_object().unwrap_or_else(|| panic!("the run's value: {value:?} {:#?}", take_errors(vm)));
    assert_eq!(vm.heap().value(obj, id!(b).into(), NoTrap).as_number(), Some(44.0), "the paused call's answer after its pauses");
    assert!(vm.heap().is_from_eval(obj), "the streaming eval's end ran at the run's real end");
    assert!(!vm.bx.silence_errors, "the streaming eval left errors silenced");
    let end = vm.end_scope_of("pause_streaming.splash").expect("the body kept the scope it ended in");
    assert_eq!(vm.heap().value(end, id!(b).into(), NoTrap).as_number(), Some(44.0), "the end scope holds the body's last lets");
    assert!(take_errors(vm).is_empty());

    let module = vm.value(vm.modules(), id!(probe)).as_object().unwrap();
    vm.add_method(module, id_lut!(sync), script_args_def!(cb = NIL), |vm, args| {
        let cb = vm.value(args, id!(cb));
        vm.call(cb, &[])
    });
    let (value, _) = run_paused(vm, 1, |vm| {
        eval(vm, "synchronous_wait", "let cb = fn() { return mod.probe.wait() }\nmod.probe.sync(cb) + mod.probe.sync(cb)")
    });
    assert_eq!(value.as_number(), Some(14.0), "both synchronous callbacks returned their real values");
    assert_eq!(WAIT.with(|w| w.borrow().1.clone()), [false, false], "a waiting vm.call prevents a callback from pausing");
    assert_eq!(nested_script_runs(vm), 2, "each synchronous script entry was counted across slices");
}

#[test]
fn a_paused_sliced_run_still_ends_on_its_instruction_and_heap_limits() {
    let vm = &mut test_vm();
    add_wait_probe(vm);
    let mut before_resume = 0;
    let (value, _) = run_paused(vm, 3, |vm| {
        let code = "mod.probe.wait()\nlet a = [1].map(fn(v) { return v + 1 })\nfor i in 0..5000 { a[0] }\n;";
        let value = vm.with_instruction_limit(5_000, |vm| vm.eval_with_append_source(append_source("pause_limit.splash"), code, ScriptObject::ZERO));
        assert!(vm.is_paused(), "the limited call suspended before leaving its scope");
        before_resume = vm.last_limit_consumed();
        assert!((1..5_000).contains(&before_resume), "the scope charged only work before the pause");
        assert_eq!(instructions_remaining(vm), 5_000 - before_resume, "the continuation kept its unspent allowance");
        value
    });
    assert!(value.is_err() || value.is_nil(), "the resumed loop must bail: {value:?}");
    assert!(errors_mention(vm, "instruction limit exceeded"), "the instruction limit across pauses");
    assert_eq!(vm.last_limit_consumed(), before_resume, "out-of-scope resumes preserve the scope's reported cost");
    assert_eq!(instructions_remaining(vm), u64::MAX, "the continuation's allowance ended with its run");
    let next = vm.eval(ScriptMod::new("", "", "after_resume.splash", 0, 0, "7\n;", &[]));
    assert_eq!(next.as_number(), Some(7.0), "the same thread runs unrelated code without a limit");

    let vm = &mut test_vm();
    add_wait_probe(vm);
    vm.heap_allocation_limit_begin(32 * 1024);
    let code = "mod.probe.wait()\nlet text = \"abcdefgh\"\nfor i in 0..20 { text = text + text }\ntext\n;";
    let _ = run_paused(vm, 3, |vm| vm.eval_with_append_source(append_source("pause_heap.splash"), code, ScriptObject::ZERO));
    let report = vm.heap_allocation_limit_end();
    assert!(report.exceeded, "the heap limit across pauses");
    assert!(errors_mention(vm, "concatenating strings"), "the heap limit's error");
}

/// Invoke the callback on 1, 2 and 3, stopping on the first error. The callback can
/// collect or yield between those calls; only the continuation holds the index.
fn visit_three(vm: &mut ScriptVm, cb: ScriptValue) -> ScriptValue {
    let mut index = 1;
    if let Some((keep, ret)) = vm.take_call_back() {
        if ret.is_err() { return ret; }
        index = keep[0].as_number().unwrap() as i32 + 1;
        if index == 4 { return ret; }
    }
    loop {
        let n = ScriptValue::from_f64(index as f64);
        match vm.call_back(cb, &[n], [n, NIL, NIL, NIL]) {
            None => return NIL,
            Some(ret) if ret.is_err() || index == 3 => return ret,
            Some(_) => index += 1,
        }
    }
}

#[test]
fn continuation_callbacks_keep_their_returns_error_boundaries_and_roots() {
    let vm = &mut test_vm();
    let module = vm.new_module(id!(probe));
    vm.add_method(module, id_lut!(visit), script_args_def!(cb = NIL), |vm, args| {
        let cb = vm.value(args, id!(cb));
        visit_three(vm, cb)
    });
    // A method without a fn proto reaches the type's own call handler.
    let receiver = ScriptValue::from_f64(17.0);
    vm.bx.code.native.borrow_mut().set_type_call(receiver.value_type().to_redux(), |vm, args, method| {
        assert_eq!(method, id!(visit), "re-entry preserves the dynamic method name");
        assert_eq!(vm.value(args, id!(self)).as_number(), Some(17.0), "re-entry preserves the receiver");
        let cb = vm.vec_value(args, 0);
        visit_three(vm, cb)
    });
    for source in [
        "let visit = mod.probe.visit\nlet n = 0\nlet last = visit(fn(v){n = n + v; return n})\nlast * 10 + n",
        "let n = 0\nlet r = 17.0\nlet last = r.visit(fn(v){n = n + v; return n})\nlast * 10 + n",
    ] {
        let (value, yields) = run_sliced(vm, 7, false, |vm| eval(vm, "visit", source));
        assert!(yields > 3, "the callback steps can yield");
        assert_eq!(value.as_number(), Some(66.0), "each callback returned once: {:?}", take_errors(vm));
    }

    // Change the type's registration while the callback is suspended. The
    // admitted call owns its target; a subsequent call sees the replacement.
    vm.set_run_budget(Some(ScriptRunBudget::slice(Duration::ZERO, 1)));
    eval(vm, "replace_type_call", "let r = 17.0\nr.visit(fn(x){return x})");
    while vm.bx.vm_stat(&mut vm_ctl::ScriptCtlReq::new(vm_ctl::STAT_NATIVE_CONTINUATIONS)) == 0 {
        assert!(vm.run_yielded(), "the old type handler must have requested its callback");
        vm.resume();
    }
    vm.bx.code.native.borrow_mut().set_type_call(receiver.value_type().to_redux(), |_, _, _| 99.into());
    vm.set_run_budget(None);
    assert_eq!(vm.resume().as_number(), Some(3.0), "the admitted handler finishes its own callbacks");
    assert_eq!(eval(vm, "new_type_call", "let r = 17.0\nr.visit()").as_number(), Some(99.0), "a new call uses the replacement handler");

    vm.add_method(module, id_lut!(arguments), script_args_def!(held = NIL), |vm, args| {
        vm.gc();
        args.into()
    });
    let args = eval(vm, "return_own_args", "mod.probe.arguments(47)").as_object().unwrap();
    assert_eq!(vm.value(args, id!(held)).as_number(), Some(47.0), "a native can return its live argument object");
    assert_eq!(vm.thread_footprint(vm.current_thread()).unwrap().0, [0; 5], "returning args releases the builder and roots");

    vm.bx.code.native.borrow_mut().set_type_call(receiver.value_type().to_redux(), |vm, args, method| {
        assert_eq!(method, id!(retry));
        if let Some((_, result)) = vm.take_call_back() {
            assert_eq!(result.as_number(), Some(23.0), "the callback completed before the pause");
            vm.pause();
            return NIL;
        }
        let cb = vm.vec_value(args, 0);
        vm.call_back(cb, &[], [NIL; 4]).unwrap_or(NIL)
    });
    eval(vm, "pause_after_callback", "let r = 17.0\nr.retry(fn(){return 23})");
    assert!(vm.is_paused(), "the resumed handler paused its own call");
    vm.bx.code.native.borrow_mut().set_type_call(receiver.value_type().to_redux(), |_, _, method| {
        assert_eq!(method, id!(retry), "retry resolves the original method");
        73.into()
    });
    assert_eq!(vm.resume().as_number(), Some(73.0), "a pause after a callback retries with the current handler");
    assert_eq!(vm.thread_footprint(vm.current_thread()).unwrap().0, [0; 5], "retry released its builder and roots");

    let visit = vm.value(module, id!(visit));
    let cb = eval(vm, "direct_fn", "fn(v) { return v + 4 }");
    vm.set_run_budget(Some(ScriptRunBudget::slice(Duration::ZERO, 1)));
    let result = vm.call(visit, &[cb]);
    assert_eq!(result.as_number(), Some(7.0), "a Rust caller receives the final callback value");
    assert!(!vm.run_yielded(), "the direct driver cannot yield through its native");
    vm.set_run_budget(None);

    let nested_error = "let n = 0\nlet r = try { mod.probe.visit(fn(v){ if v == 2 { let f = 1\n f() }\n n = n + 1\n true })\n 7 } catch { 42 }\nr * 1000 + n";
    for (bail, expected) in [(false, 7003.0), (true, 7001.0)] {
        vm.bx.bail_on_uncaught_error = bail;
        let value = eval(vm, "outer_try", nested_error);
        assert_eq!(value.as_number(), Some(expected), "the caller's try cannot catch the callback's error, bail={bail}");
        assert_eq!(take_errors(vm).len(), 1, "the callback problem is reported once");
    }
    vm.bx.bail_on_uncaught_error = false;
    let v = eval(vm, "inner_try", "mod.probe.visit(fn(v){return try { mod.std.assert(false) } catch { v }})");
    assert_eq!(v.as_number(), Some(3.0), "a callback still catches its own error");
    assert!(take_errors(vm).is_empty(), "a caught error stays local");
    let v = eval(vm, "return_err", "mod.probe.visit(fn(v){let f = 1\n f()?\n 42})");
    assert!(v.is_err(), "a callback's ? returns the error to the native: {v:?}");
    assert_eq!(take_errors(vm).len(), 1, "the first error ends the iteration");

    // The return operand is first resolved by RETURN. Catching its error abandons
    // the pending call, including any native effect after the callback.
    let completed = std::rc::Rc::new(std::cell::Cell::new(false));
    let observed = completed.clone();
    vm.add_method(module, id_lut!(observe), script_args_def!(cb = NIL), move |vm, args| {
        if let Some((_, ret)) = vm.take_call_back() {
            observed.set(true);
            return ret;
        }
        vm.call_back(vm.value(args, id!(cb)), &[], [NIL; 4]).unwrap_or(NIL)
    });
    let value = eval(vm, "return_operand", "try { mod.probe.observe(fn(){return missing_operand}) } catch {42}");
    assert_eq!(value.as_number(), Some(42.0), "the return operand's error reaches its catch");
    assert!(!completed.get(), "an unwound call cannot run its native completion");

    let conts = |vm: &ScriptVm| vm.bx.vm_stat(&mut vm_ctl::ScriptCtlReq::new(vm_ctl::STAT_NATIVE_CONTINUATIONS));
    for by_word in [false, true] {
        let slice = || Some(ScriptRunBudget::slice(Duration::ZERO, 1));
        vm.set_run_budget(slice());
        let _ = eval(vm, "runaway", "mod.probe.visit(fn(v){loop {}})\n42");
        while conts(vm) == 0 {
            assert!(vm.run_yielded(), "the run must reach the callback: {:?}", take_errors(vm));
            vm.set_run_budget(slice());
            vm.resume();
        }
        assert!(vm.run_yielded(), "a continuation was suspended inside the callback");
        if by_word {
            vm.bx.interrupt_handle().stop();
            vm.set_run_budget(None); // run through the next interrupt sample
        } else {
            vm.bx.stop_run();
            vm.set_run_budget(slice());
        }
        assert!(vm.resume().is_nil(), "Stop ends the entire run");
        assert!(!vm.run_yielded(), "the stopped callback never returns to its native");
        assert!(take_errors(vm).is_empty(), "owner Stop is silent");
        assert_eq!(conts(vm), 0, "Stop released every continuation record");
        assert_eq!(vm.thread_footprint(vm.current_thread()).unwrap().0, [0; 5], "Stop unwound every stack");
        vm.set_run_budget(None);
        vm.bx.interrupt_handle().clear();
        assert_eq!(eval(vm, "after_stop", "1 + 1").as_number(), Some(2.0), "the same VM is reusable");
    }

    let value = vm.with_instruction_limit(128, |vm| {
        eval(vm, "callback_fuel", "try { mod.probe.visit(fn(v){ loop {} }) } catch { 7 }\n42")
    });
    assert!(value.is_err() && errors_mention(vm, "instruction limit exceeded"), "fuel exhaustion crosses the callback and the outer try");
    assert_eq!(conts(vm), 0, "the hard instruction limit released every continuation");

    vm.bx.set_policy(ScriptVmPolicy::untrusted()).unwrap();
    let value = eval(vm, "callback_poison", "mod.probe.visit(fn(v){ mod.std.assert(false, \"callback stopped\") })\n42");
    assert!(value.is_err() && matches!(vm.bx.poisoned(), Some(Poison::Error(s)) if s.contains("callback stopped")), "a poisoned callback ends the whole run with its reason");
    assert_eq!(conts(vm), 0, "poison released every continuation");
    assert_eq!(vm.thread_footprint(vm.current_thread()).unwrap().0, [0; 5], "poison unwound through the physical root");
}

fn nested_script_runs(vm: &ScriptVm) -> u64 {
    vm.bx.vm_stat(&mut vm_ctl::ScriptCtlReq::new(vm_ctl::STAT_NESTED_SCRIPT_RUNS))
}

#[test]
fn unwinding_a_native_releases_only_its_own_threads_roots() {
    use std::panic::{catch_unwind, AssertUnwindSafe};

    let vm = &mut test_vm();
    let probe = vm.new_module(id!(probe));
    let caller = vm.current_thread();
    vm.add_method(probe, id_lut!(switch_and_panic), script_args_def!(item = NIL), move |vm, _| {
        vm.set_current_thread(caller);
        panic!("native switched back to the waiting caller");
    });
    let switch_and_panic = vm.value(probe, id!(switch_and_panic));
    vm.add_method(probe, id_lut!(recover), script_args_def!(item = NIL), move |vm, args| {
        // map calls this native synchronously. Its argument scope is rooted in the caller's slots.
        vm.bx.select_first_unpaused_thread();
        assert_ne!(vm.current_thread(), caller, "the nested native starts on a different thread");
        let temporary = vm.new_object();
        let unwind = catch_unwind(AssertUnwindSafe(|| vm.call(switch_and_panic, &[temporary.into()])));
        assert!(unwind.is_err(), "the nested native's panic reaches its caller");
        assert_eq!(vm.current_thread(), caller, "the unwind ended on the waiting thread");

        vm.gc();
        assert!(vm.heap().object_alive(args), "the waiting callback's argument scope survives collection");
        assert!(!vm.heap().object_alive(temporary), "the unwound call released its argument on its own thread");
        vm.value(args, id!(item))
    });

    let token = vm.new_object();
    vm.set_injected_global(id!(token), token.into());
    let (value, yields) = run_sliced(vm, 1, false, |vm| {
        eval(vm, "native_root_unwind", "[token].map(mod.probe.recover)[0]")
    });
    assert!(yields > 0, "the caller resumed from a sliced run");
    assert_eq!(value.as_object(), Some(token), "the resumed map returned its original item");
}

#[test]
fn collection_callbacks_yield_in_order_and_stop_inside_the_callback() {
    let vm = &mut test_vm();
    let probe = vm.new_module(id!(probe));
    let entered = std::rc::Rc::new(std::cell::Cell::new(false));
    let witness = entered.clone();
    vm.add_method(probe, id_lut!(entered), script_args_def!(value = NIL), move |vm, args| {
        assert_eq!(vm.value(args, id!(value)).as_number(), Some(2.0), "map advanced after returning its first callback");
        witness.set(true);
        NIL
    });
    // A native callback collects before reading its argument or returning to map.
    vm.add_method(probe, id_lut!(collect), script_args_def!(item = NIL), |vm, args| {
        vm.gc();
        vm.value(args, id!(item))
    });

    for by_word in [false, true] {
        entered.set(false);
        vm.set_run_budget(Some(ScriptRunBudget::slice(Duration::ZERO, 1)));
        eval(vm, "endless_map", "[1, 2].map(fn(v) { if v == 1 { return v }\n mod.probe.entered(v)\n loop {} })\n42");
        while !entered.get() {
            assert!(vm.run_yielded(), "the bounded test must enter the endless callback: {:?}", take_errors(vm));
            vm.resume();
        }
        if by_word {
            vm.bx.interrupt_handle().stop();
            vm.set_run_budget(None); // reach the next interrupt sample
        } else {
            for _ in 0..50 {
                vm.resume();
                assert!(vm.run_yielded(), "the endless callback yielded again");
                assert_eq!(nested_script_runs(vm), 0, "the callback used its caller's interpreter");
            }
            vm.bx.stop_run();
        }
        assert!(vm.resume().is_nil() && !vm.run_yielded(), "one resume finished the stopped map (word only: {by_word})");
        assert!(take_errors(vm).is_empty(), "Stop produced no diagnostic");
        assert_eq!(vm.thread_footprint(vm.current_thread()).unwrap().0, [0; 5], "Stop cleared all five stacks");
        assert_eq!(vm.bx.vm_stat(&mut vm_ctl::ScriptCtlReq::new(vm_ctl::STAT_NATIVE_CONTINUATIONS)), 0, "Stop released the suspended map");
        vm.set_run_budget(None);
        vm.bx.interrupt_handle().clear();
        assert_eq!(eval(vm, "reuse", "1 + 1").as_number(), Some(2.0), "the stopped VM can run again");
    }

    let program = r#"
let trail = []
let a = [1, 2, 3]
let mapped = a.map(fn(x) {
    trail.push(10 + x)
    let inner = [4].map(fn(y){ trail.push(20 + x); return x + y })
    return inner[0]
})
let filtered = a.filter(fn(x){ trail.push(30 + x); a[x - 1] = 0; return x != 2 })
let b = [1, 2, 3]
let found = b.find(fn(x){ trail.push(40 + x); b[x - 1] = 0; return x == 2 })
let any = [1, 2, 3].any(fn(x){ trail.push(50 + x); return x == 2 })
let all = [1, 2, 3].all(fn(x){ trail.push(60 + x); return x < 2 })
let kept = [1, 2, 3]
kept.retain(fn(x){ trail.push(70 + x); return x == 3 })
let obj = {1, 2, 3}
obj.retain(fn(x){ trail.push(80 + x); return x == 2 })
let caught = [9].map(fn(x){return try { mod.std.assert(false) } catch { trail.push(90 + x); x }})
let tail = [0].map(fn(x){ for i in 0..1 { trail.push(100 + i) } })
let collected = [mapped[0], found].map(mod.probe.collect)
mod.std.assert(any && !all && tail[0] == nil)
{trail: trail, values: [mapped[1], filtered.len(), filtered[0], filtered[1], found, kept.len(), kept[0], obj.vec_len(), obj[0], caught[0], collected[0], collected[1]]}"#;

    for interval in [0, 1, 97] {
        let value = if interval == 0 { eval(vm, "collections", program) } else {
            let (value, yields) = run_sliced(vm, interval, true, |vm| eval(vm, "collections", program));
            assert!(yields > 0, "the collection ran in slices at interval {interval}");
            value
        };
        let errors = take_errors(vm);
        assert!(errors.is_empty(), "collection diagnostics at interval {interval}: {errors:?}");
        let obj = value.as_object().expect("the program returned its values and callback trail");
        let numbers = |key| {
            let a = vm.heap().value(obj, key, NoTrap).as_array().unwrap();
            (0..vm.heap().array_len(a))
                .map(|i| vm.heap().array_index(a, i, NoTrap).as_number().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(numbers(id!(values).into()), [6, 2, 1, 3, 2, 1, 3, 1, 2, 9, 5, 2].map(f64::from), "collection values at interval {interval}");
        assert_eq!(numbers(id!(trail).into()), [11, 21, 12, 22, 13, 23, 31, 32, 33, 41, 42, 51, 52, 61, 62, 71, 72, 73, 81, 82, 83, 99, 100].map(f64::from), "callback order and early exits at interval {interval}");
        assert_eq!(nested_script_runs(vm), 0, "no collection re-entered the interpreter");
    }
}

#[test]
fn interrupt_sampling_stops_runs_across_slices_and_preserves_real_limits() {
    let vm = &mut test_vm();
    let module = |name: &str, code: &str| ScriptMod::new("", "", name, 0, 0, code, &[]);
    let finite = "let n = 0\nfor i in 0..2048 { n += 1 }\nn\n;";
    let endless = "loop {}\n;";
    let stop = vm.bx.interrupt_handle();
    stop.stop();
    // The finite witness fails promptly if the sample is missing, before entering the loop.
    vm.with_interrupt_sampling(|vm| {
        assert!(vm.eval(module("stop_finite.splash", finite)).is_nil(), "Stop preempts a finite result");
        assert!(vm.eval(module("stop_loop.splash", endless)).is_nil(), "Stop ends an unlimited loop with nil");
    });
    assert!(take_errors(vm).is_empty(), "owner Stop produces no diagnostic");
    stop.clear();
    let whole = vm.eval(module("whole.splash", finite));
    assert_eq!(whole.as_number(), Some(2048.0), "the VM is reusable after clearing Stop");
    let last_limit = (vm.last_limit_consumed(), vm.bx.last_limit_exit_remaining);
    vm.with_interrupt_sampling(|vm| {
        let sampled = vm.eval(module("sampled.splash", finite));
        assert_eq!(sampled, whole, "sampling preserves the finite result");
        assert_eq!(instructions_remaining(vm), u64::MAX, "sampling alone reports unlimited fuel");
        assert_eq!((vm.last_limit_consumed(), vm.bx.last_limit_exit_remaining), last_limit,
            "sampling alone leaves the most recent limit report intact");
        vm.with_instruction_limit(u64::MAX, |vm| {
            assert_eq!(instructions_remaining(vm), u64::MAX, "prior sampling cannot narrow a new real allowance");
        });
    });

    // Each slice ends before a fresh counter would sample. The third reaches the original
    // counter's first sample only if resume kept the work done in the first two slices.
    vm.set_run_budget(Some(ScriptRunBudget::slice(Duration::ZERO, 400)));
    vm.with_interrupt_sampling(|vm| vm.eval(module("sliced_interrupt.splash", endless)));
    assert!(vm.run_yielded(), "the sampling scope returned at the first yield");
    vm.resume();
    assert!(vm.run_yielded(), "the loop continued through a second slice");
    assert_eq!(instructions_remaining(vm), u64::MAX, "the paused sampling counter is not a limit");
    stop.stop();
    assert!(vm.resume().is_nil() && !vm.is_paused(), "the third slice samples Stop and finishes");
    assert!(take_errors(vm).is_empty(), "the resumed Stop is silent");
    vm.set_run_budget(None);
    stop.clear();

    let result = vm.with_instruction_limit(64, |vm| {
        vm.with_interrupt_sampling(|vm| vm.eval(module("outer_limit.splash", finite)))
    });
    assert!(result.is_err() && errors_mention(vm, "script instruction limit exceeded"),
        "the enclosing real limit still bails with its usual diagnostic");
    assert_eq!(vm.last_limit_consumed(), 64, "the enclosing limit charged every instruction");
}

#[test]
fn yielding_does_not_spend_instruction_fuel() {
    let mut vm = test_vm();
    let module = || ScriptMod::new("", "", "fuel.splash", 1, 0, "let n = 0\nfor i in 0..8 { n += i }\nn\n;", &[]);
    let whole = vm.with_instruction_limit(u64::MAX, |vm| vm.eval(module()));
    let cost = vm.last_limit_consumed();
    assert_eq!(whole.as_number(), Some((0..8).sum::<i32>() as f64), "the measured document sums its range");

    let mut whole_line = None;
    for sliced in [false, true] {
        for allowance in [cost, cost - 1] {
            vm.set_run_budget(sliced.then(|| ScriptRunBudget::slice(Duration::ZERO, 2)));
            let mut result = vm.with_instruction_limit(allowance, |vm| vm.eval(module()));
            assert_eq!(vm.run_yielded(), sliced, "only the sliced run suspends");
            while vm.run_yielded() { result = vm.resume(); }
            let hit = vm.bx.take_limit_hit();
            let errors = take_errors(&mut vm);
            if allowance == cost {
                assert_eq!(result, whole, "the exact allowance completes, sliced={sliced}");
                assert!(hit.is_none() && errors.is_empty(), "an exact fit raises no limit or diagnostic");
            } else {
                assert!(result.is_err() && !errors.is_empty(), "one instruction short must bail with a diagnostic");
                let (limit, ip) = hit.expect("the bail records its cause without a host policy");
                assert_eq!(limit, Limit::Fuel, "the unexecuted instruction ran out of fuel");
                let line = vm.bx.code_ref().ip_to_loc(ip).expect("the fuel hit has a source line").line();
                assert_eq!(line, *whole_line.get_or_insert(line), "slicing preserves the failing line");
            }
        }
    }
}
