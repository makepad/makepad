//! Structural equality (cycle-aware, fuel-metered, deadline-checked), the
//! uncatchable bail on an uncaught script error, and the host-controlled
//! `allow_debug_output` switch for the `~` LOG operator.

use makepad_script::*;
use std::time::Duration;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new((), ())));
    ScriptVm {
        host,
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn script(name: &str, code: &str) -> ScriptMod {
    ScriptMod {
        file: format!("equality_fuel_bail_{name}"),
        // The streaming parser closes a statement on the next token, so the
        // tail expression needs the same terminator production eval appends.
        code: format!("{code}\n;"),
        ..Default::default()
    }
}

fn eval_bool(name: &str, code: &str) -> Option<bool> {
    let vm = &mut test_vm();
    let result = vm.eval(script(name, code));
    assert!(!result.is_err(), "{name}: {:?} {:#?}", result, vm.take_errors());
    result.as_bool()
}

/// Two arrays of `len` numbers, injected as globals `left` and `right`.
fn inject_arrays(vm: &mut ScriptVm, len: usize, differ: bool) {
    let left = vm.heap_mut().new_array();
    let right = vm.heap_mut().new_array();
    for i in 0..len {
        vm.heap_mut().array_push_unchecked(left, (i as f64).into());
        let value = if differ && i + 1 == len { -1.0 } else { i as f64 };
        vm.heap_mut().array_push_unchecked(right, value.into());
    }
    vm.set_injected_global(id!(left), left.into());
    vm.set_injected_global(id!(right), right.into());
}

/// A numeric field of `obj`, or `None` when it was never assigned (a missing
/// key reads back as a not-found error value, and integer literals are stored
/// as u40, so `as_number` is the right reader for both cases).
fn field(vm: &ScriptVm, obj: ScriptObject, key: LiveId) -> Option<f64> {
    vm.heap().value(obj, key.into(), NoTrap).as_number()
}

// ---------------------------------------------------------------------------
// equality semantics

#[test]
fn cyclic_objects_compare_by_visiting_each_pair_once() {
    assert_eq!(
        eval_bool(
            "cycle_equal",
            "let a = {}\na.next = a\nlet b = {}\nb.next = b\na == b",
        ),
        Some(true)
    );
    assert_eq!(
        eval_bool(
            "cycle_unequal",
            "let a = {}\na.next = a\na.v = 1\nlet b = {}\nb.next = b\nb.v = 2\na != b",
        ),
        Some(true)
    );
    // A two-node cycle unrolls to the same infinite sequence as a one-node
    // cycle: bisimulation, not recursion depth, decides.
    assert_eq!(
        eval_bool(
            "cycle_shapes",
            "let a = {}\na.next = a\nlet b = {}\nlet c = {}\nb.next = c\nc.next = b\na == b",
        ),
        Some(true)
    );
}

#[test]
fn shared_dags_compare_structurally() {
    assert_eq!(
        eval_bool(
            "dag",
            "let leaf = [1, 2, [3]]\n\
             let x = {l: leaf, r: leaf}\n\
             let y = {l: [1, 2, [3]], r: [1, 2, [3]]}\n\
             x == y",
        ),
        Some(true)
    );
    assert_eq!(
        eval_bool(
            "dag_unequal",
            "let leaf = [1, 2, [3]]\n\
             let x = {l: leaf, r: leaf}\n\
             let y = {l: [1, 2, [3]], r: [1, 2, [4]]}\n\
             x == y",
        ),
        Some(false)
    );
}

#[test]
fn nan_stays_unequal_to_itself() {
    assert_eq!(eval_bool("nan_eq", "let n = 0.0 / 0.0\nn == n"), Some(false));
    assert_eq!(eval_bool("nan_neq", "let n = 0.0 / 0.0\nn != n"), Some(true));
    assert_eq!(
        eval_bool("nan_nested", "let n = 0.0 / 0.0\n[n] == [n]"),
        Some(false)
    );
}

#[test]
fn strings_and_numbers_keep_their_value_semantics() {
    assert_eq!(
        eval_bool("string_eq", "let s = \"hello world\"\ns == \"hello \" + \"world\""),
        Some(true)
    );
    assert_eq!(eval_bool("string_neq", "\"kind_a\" == \"kind_b\""), Some(false));
    assert_eq!(eval_bool("string_vs_number", "\"1\" == 1"), Some(false));
    assert_eq!(eval_bool("int_vs_float", "1 == 1.0"), Some(true));
    assert_eq!(
        eval_bool("string_keys", "{a: 1, b: [2]} == {a: 1, b: [2]}"),
        Some(true)
    );
}

#[test]
fn host_deep_eq_is_iterative_and_handles_typed_arrays() {
    let vm = &mut test_vm();
    let a = vm.eval(script("host_a", "let a = {}\na.next = a\na"));
    let b = vm.eval(script("host_b", "let b = {}\nb.next = b\nb"));
    assert!(a.is_object() && b.is_object());
    assert!(vm.heap().deep_eq(a, b));
    assert!(vm.heap().deep_eq(a, a));

    let bytes_a = vm.heap_mut().new_array_from_vec_u8(vec![1, 2, 3]);
    let bytes_b = vm.heap_mut().new_array_from_vec_u8(vec![1, 2, 3]);
    let bytes_c = vm.heap_mut().new_array_from_vec_u8(vec![1, 2, 4]);
    assert!(vm.heap().deep_eq(bytes_a.into(), bytes_b.into()));
    assert!(!vm.heap().deep_eq(bytes_a.into(), bytes_c.into()));

    // The raw host entry point has no work ceiling: a comparison larger than
    // MAX_EQUALITY_WORK still completes.
    inject_arrays(vm, equality::MAX_EQUALITY_WORK, false);
    let left = vm.bx.injected_globals[&id!(left)];
    let right = vm.bx.injected_globals[&id!(right)];
    assert!(vm.heap().deep_eq(left, right));
    // But the bounded form reports exhaustion instead of a result.
    assert_eq!(vm.heap().deep_eq_bounded(left, right, 16, || true), None);
    assert_eq!(vm.heap().deep_eq_bounded(left, right, usize::MAX, || false), None);
}

// ---------------------------------------------------------------------------
// fuel, work ceiling and deadline

#[test]
fn equality_charges_instruction_fuel() {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    inject_arrays(vm, 2_000, false);

    let ok = vm.with_instruction_limit(100_000, |vm| vm.eval(script("fuel_ok", "left == right")));
    assert_eq!(ok.as_bool(), Some(true), "{:#?}", vm.take_errors());
    assert!(
        vm.last_limit_consumed() > 2_000,
        "comparison must charge per element, charged {}",
        vm.last_limit_consumed()
    );

    vm.bx.captured_errors = Some(Vec::new());
    let result = vm.with_instruction_limit(500, |vm| vm.eval(script("fuel_out", "left == right")));
    assert!(result.is_err());
    let errors = vm.take_errors();
    assert!(
        errors
            .iter()
            .any(|e| e.contains("script equality work, instruction, or time limit exceeded")),
        "{errors:#?}"
    );
    assert!(vm.thread().trap.err_is_empty());
}

#[test]
fn an_unbounded_evaluation_compares_past_the_work_ceiling() {
    // No instruction limit, run budget or allocation budget: the host asked
    // for no bound, so large structural comparisons are legitimate.
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    inject_arrays(vm, equality::MAX_EQUALITY_WORK, false);

    let result = vm.eval(script("unbounded", "left == right"));

    assert_eq!(result.as_bool(), Some(true), "{:#?}", vm.take_errors());
    assert!(vm.take_errors().is_empty());
}

#[test]
fn equality_work_ceiling_bails_and_cannot_be_caught() {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    // One unit per queued edge plus one per processed pair: past the ceiling.
    inject_arrays(vm, equality::MAX_EQUALITY_WORK, false);
    let state = vm.heap_mut().new_object();
    vm.set_injected_global(id!(state), state.into());

    // The ceiling belongs to bounded evaluations; the instruction limit is far
    // above it so the work ceiling is what stops the comparison.
    let result = vm.with_instruction_limit(10_000_000, |vm| {
        vm.eval(script(
            "work_ceiling",
            "try { left == right } { state.caught = 1 }\nstate.after = 1",
        ))
    });

    assert!(result.is_err());
    let errors = vm.take_errors();
    assert!(
        errors
            .iter()
            .any(|e| e.contains("script equality work, instruction, or time limit exceeded")),
        "{errors:#?}"
    );
    assert!(field(vm, state, id!(caught)).is_none());
    assert!(field(vm, state, id!(after)).is_none());
    assert!(vm.thread().trap.err_is_empty());

    // Just under the ceiling still produces a result.
    let vm = &mut test_vm();
    inject_arrays(vm, equality::MAX_EQUALITY_WORK / 4, true);
    let result = vm.eval(script("work_under_ceiling", "left == right"));
    assert_eq!(result.as_bool(), Some(false), "{:#?}", vm.take_errors());
}

#[test]
fn equality_checks_the_hard_deadline_while_comparing() {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    inject_arrays(vm, 4_096, false);
    // The interpreter loop samples the clock only every million instructions,
    // so this short script only observes the expired hard deadline from
    // inside the comparison itself.
    vm.bx.run_budget = Some(ScriptRunBudget::from_durations(
        Duration::from_secs(3600),
        Duration::ZERO,
        1_000_000,
    ));

    let result = vm.eval(script("deadline", "left == right"));

    assert!(result.is_err());
    let errors = vm.take_errors();
    assert!(
        errors
            .iter()
            .any(|e| e.contains("script equality work, instruction, or time limit exceeded")),
        "{errors:#?}"
    );
    assert!(vm.thread().trap.err_is_empty());
}

#[test]
fn hard_time_budget_drains_its_uncatchable_error() {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    vm.bx.run_budget = Some(ScriptRunBudget::from_durations(
        Duration::ZERO,
        Duration::ZERO,
        1,
    ));

    let result = vm.eval(script("hard_time_budget", "loop {}"));

    assert!(result.is_err());
    assert!(vm.thread().trap.err_is_empty());
    assert!(vm
        .take_errors()
        .iter()
        .any(|diagnostic| diagnostic.contains("script time budget exceeded")));
}

// ---------------------------------------------------------------------------
// uncaught errors bail; try still recovers

#[test]
fn uncaught_error_bails_before_the_next_instruction() {
    let vm = &mut test_vm();
    vm.bx.bail_on_uncaught_error = true;
    vm.bx.captured_errors = Some(Vec::new());
    let state = vm.heap_mut().new_object();
    vm.set_injected_global(id!(state), state.into());

    let result = vm.eval(script(
        "uncaught",
        "state.before = 1\nundefined_fn()\nstate.after = 1\n42",
    ));

    assert!(result.is_err(), "{result:?}");
    assert_eq!(
        field(vm, state, id!(before)),
        Some(1.0)
    );
    assert!(
        field(vm, state, id!(after)).is_none(),
        "no instruction may run after an uncaught error"
    );
    // The diagnostic reached the sink exactly once and the queue is clean.
    let errors = vm.take_errors();
    assert_eq!(errors.len(), 1, "{errors:#?}");
    assert!(vm.thread().trap.err_is_empty());

    // The VM is reusable afterwards.
    let again = vm.eval(script("uncaught_again", "1 + 1"));
    assert_eq!(again.as_number(), Some(2.0));
}

#[test]
fn active_try_handlers_still_recover_normally() {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let state = vm.heap_mut().new_object();
    vm.set_injected_global(id!(state), state.into());

    let result = vm.eval(script(
        "recover",
        "state.before = 1\n\
         try { undefined_fn() } { state.caught = 1 }\n\
         state.after = 1\n\
         42",
    ));

    assert_eq!(result.as_number(), Some(42.0), "{:#?}", vm.take_errors());
    for key in [id!(before), id!(caught), id!(after)] {
        assert_eq!(field(vm, state, key), Some(1.0));
    }
    assert!(vm.take_errors().is_empty());
}

// ---------------------------------------------------------------------------
// allow_debug_output

#[test]
fn debug_output_is_allowed_by_default() {
    let vm = &mut test_vm();
    assert!(vm.bx.allow_debug_output);
    vm.bx.captured_errors = Some(Vec::new());
    let result = vm.eval(script("log_default", "let v = ~41\nv + 1"));
    assert_eq!(result.as_number(), Some(42.0), "{:#?}", vm.take_errors());
    assert!(vm.take_errors().is_empty());
}

#[test]
fn disabled_debug_output_rejects_log_as_a_catchable_error() {
    let vm = &mut test_vm();
    vm.bx.bail_on_uncaught_error = true;
    vm.bx.allow_debug_output = false;
    vm.bx.captured_errors = Some(Vec::new());
    let state = vm.heap_mut().new_object();
    vm.set_injected_global(id!(state), state.into());

    let result = vm.eval(script("log_denied", "let v = ~41\nstate.after = 1\nv + 1"));
    assert!(result.is_err(), "{result:?}");
    assert!(field(vm, state, id!(after)).is_none());
    let errors = vm.take_errors();
    assert!(
        errors
            .iter()
            .any(|e| e.contains("direct script logging is disabled by this host")),
        "{errors:#?}"
    );

    let result = vm.eval(script(
        "log_denied_caught",
        "try { ~41 } { state.caught = 1 }\nstate.done = 1\n7",
    ));
    assert_eq!(result.as_number(), Some(7.0), "{:#?}", vm.take_errors());
    assert_eq!(
        field(vm, state, id!(caught)),
        Some(1.0)
    );
    assert_eq!(
        field(vm, state, id!(done)),
        Some(1.0)
    );
}
