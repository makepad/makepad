//! Regression tests: parser auto_close at end-of-source.
//!
//! Empirically (host_launcher, 2026-08-07): when the LAST statement of a
//! source is `let c = <lambda>` (or `let c = <call>`), auto_close dropped the
//! still-open EndFnExpr/EndFnBlock and EmitLetDyn states via its `_ => {}`
//! catch-all. Consequences:
//! - an unbraced lambda body's FN_BODY_DYN jump stayed 0, so at eval time the
//!   opcode re-ran, found its me popped ("me stack is empty"), fell INTO the
//!   body and executed it inline — ending the module eval early;
//! - the `let` opcode was never emitted, so the binding silently didn't exist.
//!
//! Also here: the optional-hook probe contract used by Splash::call_script_fn
//! — a NoTrap scope_value must not queue a NotFound into the error log (a
//! trapping probe spammed "variable <raw id> not found" for every script that
//! doesn't define an optional hook like on_app_resize).

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(0i32));
    let std = Box::leak(Box::new(0i32));
    ScriptVm {
        host,
        std,
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn eval_str(vm: &mut ScriptVm, name: &str, code: &str) -> ScriptValue {
    vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: format!("auto_close_eof_{name}"),
            line: 0,
            column: 0,
            code: code.to_string(),
            values: vec![],
        })
    })
}

fn resolve(vm: &mut ScriptVm, file: &str, key: LiveId) -> ScriptValue {
    let scope = {
        let bodies = vm.bx.code.bodies.borrow();
        bodies
            .iter()
            .find_map(|body| match &body.source {
                ScriptSource::Mod(m) if m.file == file => Some(body.scope.as_object()),
                _ => None,
            })
            .expect("body scope")
    };
    vm.bx.heap.scope_value(scope, key, vm.trap())
}

/// `let` + unbraced lambda as the FINAL statement: the lambda must bind and
/// its body must NOT run at eval time.
#[test]
fn eof_unbraced_lambda_let_binds() {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let _ = eval_str(vm, "unbraced", "\nlet hits = 0\nfn bump(){ hits += 7 hits }\nlet c = || bump()");
    let mut errs = vm.bx.captured_errors.take().unwrap();
    errs.extend(vm.take_errors());
    assert!(errs.is_empty(), "eval errors: {errs:?}");
    // Body must not have run at eval: hits still 0.
    let hits = resolve(vm, "auto_close_eof_unbraced", live_id!(hits));
    assert_eq!(hits.as_number(), Some(0.0), "lambda body ran at eval time");
    let c = resolve(vm, "auto_close_eof_unbraced", live_id!(c));
    let is_fn = c.as_object().map(|o| vm.bx.heap.is_fn(o)).unwrap_or(false);
    assert!(is_fn, "c did not bind to a fn (got {c:?})");
    let out = vm.call(c, &[NIL]);
    assert_eq!(out.as_number(), Some(7.0), "call got {out:?}");
}

/// Braced variant: the block closes via its `}`, but the `let` still has to
/// bind (EmitLetDyn was dropped by auto_close's catch-all too).
#[test]
fn eof_braced_lambda_let_binds() {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let _ = eval_str(vm, "braced", "\nfn seven(){ 7 }\nlet c = || { seven() }");
    let mut errs = vm.bx.captured_errors.take().unwrap();
    errs.extend(vm.take_errors());
    assert!(errs.is_empty(), "eval errors: {errs:?}");
    let c = resolve(vm, "auto_close_eof_braced", live_id!(c));
    let is_fn = c.as_object().map(|o| vm.bx.heap.is_fn(o)).unwrap_or(false);
    assert!(is_fn, "c did not bind to a fn (got {c:?})");
    let out = vm.call(c, &[NIL]);
    assert_eq!(out.as_number(), Some(7.0), "call got {out:?}");
}

/// `let` bound to a plain call result as the final statement.
#[test]
fn eof_call_result_let_binds() {
    let vm = &mut test_vm();
    let _ = eval_str(vm, "callres", "\nfn nine(){ 9 }\nlet c = nine()");
    let c = resolve(vm, "auto_close_eof_callres", live_id!(c));
    assert_eq!(c.as_number(), Some(9.0), "got {c:?}");
}

/// The launcher boot-timer shape: a closure passed as a call argument, whose
/// VALUE the host stores and calls later (like std.start_timeout's fire path).
/// Not an EOF case — insurance that the deferred-call path stays healthy.
#[test]
fn deferred_arg_closure_calls_module_fn() {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let c = eval_str(
        vm,
        "deferred",
        "\nlet hits = 0\nfn refresh(){ hits += 1 hits }\nfn st(d, cb){ cb }\nst(0.05, || refresh())\n",
    );
    let mut errs = vm.bx.captured_errors.take().unwrap();
    errs.extend(vm.take_errors());
    assert!(errs.is_empty(), "eval errors: {errs:?}");
    let is_fn = c.as_object().map(|o| vm.bx.heap.is_fn(o)).unwrap_or(false);
    assert!(is_fn, "closure not returned (got {c:?})");
    let out = vm.call(c, &[0.5f64.into()]);
    assert_eq!(out.as_number(), Some(1.0), "timer-fire call got {out:?}");
}

/// Non-last lambda-let in the middle of a module: distinguishes a correct
/// bind+call (17) from the buggy inline-early-return (7).
#[test]
fn nonlast_lambda_let_discriminating() {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let v = eval_str(
        vm,
        "nonlast",
        "\nfn echo(x){ x }\nfn seven(){ 7 }\nlet c = || seven()\nlet out = c() + 10\necho(out)",
    );
    let mut errs = vm.bx.captured_errors.take().unwrap();
    errs.extend(vm.take_errors());
    assert!(errs.is_empty(), "eval errors: {errs:?}");
    assert_eq!(v.as_number(), Some(17.0), "got {v:?}");
}

/// Optional-hook probe contract: a NoTrap scope_value miss must not queue an
/// error, a trapping one must — Splash::call_script_fn relies on the former.
#[test]
fn notrap_scope_probe_is_silent() {
    let vm = &mut test_vm();
    let _ = eval_str(vm, "probe", "\nlet x = 1\nx");
    let scope = {
        let bodies = vm.bx.code.bodies.borrow();
        bodies
            .iter()
            .find_map(|body| match &body.source {
                ScriptSource::Mod(m) if m.file == "auto_close_eof_probe" => {
                    Some(body.scope.as_object())
                }
                _ => None,
            })
            .expect("body scope")
    };
    vm.bx.captured_errors = Some(Vec::new());
    let miss = vm.bx.heap.scope_value(scope, live_id!(on_app_resize), NoTrap);
    assert!(miss.is_err() || miss.is_nil(), "expected a miss, got {miss:?}");
    let mut errs = vm.bx.captured_errors.take().unwrap();
    errs.extend(vm.take_errors());
    assert!(errs.is_empty(), "NoTrap probe queued errors: {errs:?}");

    vm.bx.captured_errors = Some(Vec::new());
    let trap = vm.trap();
    let _miss = vm.bx.heap.scope_value(scope, live_id!(on_app_resize), trap);
    let mut errs = vm.bx.captured_errors.take().unwrap();
    errs.extend(vm.take_errors());
    assert!(
        !errs.is_empty(),
        "trapping probe should queue (contract check that this test can detect the difference)"
    );
}
