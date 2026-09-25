//! What a Makepad host (widgets, Studio, the DSL of every app) relies on and a
//! sandboxing host opts out of: an uncaught error is reported and the
//! evaluation continues, an error raised while no script ran belongs to no
//! evaluation, and an error inside a callback a native invokes never unwinds
//! into the frames of the run that called the native.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new((), ())));
    let mut vm = ScriptVm {
        host,
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.bx.captured_errors = Some(Vec::new());
    vm
}

fn eval(vm: &mut ScriptVm, name: &str, code: &str) -> ScriptValue {
    vm.eval(ScriptMod {
        file: format!("trusted_host_{name}.splash"),
        code: format!("{code}\n;"),
        ..Default::default()
    })
}

/// A host-side error with no script running: Rust calls a value that is not a
/// function (a missing callback).
fn raise_a_host_side_error(vm: &mut ScriptVm) {
    let not_a_fn: ScriptValue = 7.into();
    assert!(vm.call(not_a_fn, &[]).is_err());
}

/// `retain` calls its closure back on the same thread, so the closure runs in
/// a nested root frame while the outer run is still live under the native.
/// The closure fails on element 2; `n` counts the elements it finished.
const CALLBACK_FAILS_ON_2: &str = "fn(v){ if v == 2 { let f = 1\n f() }\n n = n + 1\n true }";

#[test]
fn the_module_keeps_evaluating_past_an_uncaught_error() {
    let vm = &mut test_vm();
    let value = eval(vm, "continue", "let f = 1\nf()\n42");
    assert_eq!(value.as_number(), Some(42.0), "{value:?}");
    assert_eq!(vm.take_errors().len(), 1);
}

#[test]
fn a_returned_error_value_can_be_inspected_without_try() {
    let vm = &mut test_vm();
    let value = eval(vm, "inspect", "let f = 1\nlet r = f()\nif r == nil { 1 } else { 2 }");
    assert_eq!(value.as_number(), Some(2.0), "{value:?}");
}

#[test]
fn a_host_side_error_does_not_end_the_next_evaluation() {
    for bail_on_uncaught_error in [false, true] {
        let vm = &mut test_vm();
        vm.bx.bail_on_uncaught_error = bail_on_uncaught_error;
        raise_a_host_side_error(vm);
        let value = eval(vm, "stale", "40 + 2");
        assert_eq!(value.as_number(), Some(42.0), "bail={bail_on_uncaught_error} {value:?}");
        // reported once, at the boundary, not lost
        assert_eq!(vm.take_errors().len(), 1);
    }
}

#[test]
fn a_host_side_error_is_not_caught_by_the_next_evaluations_try() {
    let vm = &mut test_vm();
    raise_a_host_side_error(vm);
    let value = eval(vm, "stale_try", "try { 1 } catch { 2 }");
    assert_eq!(value.as_number(), Some(1.0), "{value:?}");
}

#[test]
fn a_host_side_error_does_not_end_the_next_callback() {
    let vm = &mut test_vm();
    vm.bx.bail_on_uncaught_error = true;
    let callback = eval(vm, "callback", "fn(x){ return x + 1 }");
    raise_a_host_side_error(vm);
    let value = vm.call(callback, &[41.into()]);
    assert_eq!(value.as_number(), Some(42.0), "{value:?}");
}

#[test]
fn an_error_in_a_native_callback_does_not_unwind_into_the_outer_run() {
    let vm = &mut test_vm();
    // the outer frame owns a try while the native calls back
    let value = eval(
        vm,
        "outer_try",
        &format!(
            "let a = [1, 2, 3]\nlet n = 0\nlet r = try {{ a.retain({CALLBACK_FAILS_ON_2})\n 7 }} catch {{ 42 }}\nr * 1000 + n"
        ),
    );
    assert_eq!(value.as_number(), Some(7003.0), "{value:?} {:#?}", vm.take_errors());

    // a script fn owns the try, so a non-root frame sits under the native too
    let vm = &mut test_vm();
    let value = eval(
        vm,
        "fn_owns_try",
        &format!(
            "let a = [1, 2, 3]\nlet n = 0\nlet g = fn(){{ return try {{ a.retain({CALLBACK_FAILS_ON_2})\n 7 }} catch {{ 42 }} }}\nlet r = g()\nr * 1000 + n"
        ),
    );
    assert_eq!(value.as_number(), Some(7003.0), "{value:?} {:#?}", vm.take_errors());
}

#[test]
fn an_opted_in_bail_inside_a_native_callback_ends_the_callback_only() {
    let vm = &mut test_vm();
    vm.bx.bail_on_uncaught_error = true;
    let value = eval(
        vm,
        "bail_in_callback",
        &format!(
            "let a = [1, 2, 3]\nlet n = 0\nlet r = try {{ a.retain({CALLBACK_FAILS_ON_2})\n 7 }} catch {{ 42 }}\nr * 1000 + n"
        ),
    );
    // `retain` stops at the failed callback; the outer run is intact and its
    // frames were not unwound from inside the nested one.
    assert_eq!(value.as_number(), Some(7001.0), "{value:?} {:#?}", vm.take_errors());
    let errors = vm.take_errors();
    assert!(
        !errors.iter().any(|e| e.contains("root call reached")),
        "{errors:#?}"
    );
}

#[test]
fn checked_index_accepts_exactly_the_integral_in_range_numbers() {
    let index = |v: f64| ScriptValue::from_f64(v).checked_index();
    assert_eq!(index(0.0), Some(0));
    assert_eq!(index(-0.0), Some(0));
    assert_eq!(index(7.0), Some(7));
    assert_eq!(index(9_007_199_254_740_992.0), Some(9_007_199_254_740_992));
    for rejected in [
        1.5,
        -1.0,
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        18_446_744_073_709_551_616.0, // 2^64: saturates to usize::MAX
        1.0e300,
    ] {
        assert_eq!(index(rejected), None, "{rejected}");
    }
    assert_eq!(ScriptValue::from_bool(true).checked_index(), None);
}
