//! Regression tests: parse errors must reach a captured-diagnostics sink.
//!
//! Empirically (host_launcher, 2026-08-14): the parser RECOVERS from errors
//! like a dangling `else` in expression position — it logs, sets `had_error`,
//! and still produces a runnable module. Nothing entered the trap queue, so a
//! validating host (`captured_errors` sink + `take_errors`) reported SUCCESS
//! for scripts that failed to parse, and broken mini-apps sailed through
//! validation with their errors only in the log.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn eval_captured(vm: &mut ScriptVm, name: &str, code: &str) -> Vec<String> {
    vm.bx.captured_errors = Some(Vec::new());
    vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod::new("", "", &format!("parse_error_capture_{name}"), 0, 0, &code, &[]))
    });
    vm.take_errors()
}

/// The exact shape that sailed through host_launcher's validation while
/// failing to parse (isolation_probe's svc_result, 2026-08-14): a fn whose
/// FINAL statement is an if with call-statement branches and the `else` on
/// its own line. The parser reports "Unexpected else" and recovers; the sink
/// must see it.
const DANGLING_ELSE: &str = "\
fn svc_result(r){
    let ok = r.ok
    if ok {
        ui.a.set_text(\"A\")
        ui.b.set_text(\"A\")
    }
    else {
        ui.a.set_text(\"B\")
        ui.b.set_text(\"B\")
    }
}
1 + 2";

#[test]
fn dangling_else_reaches_the_sink() {
    let mut vm = test_vm();
    let errors = eval_captured(&mut vm, "dangling_else", DANGLING_ELSE);
    assert!(
        errors.iter().any(|e| e.contains("Unexpected else")),
        "parse error missing from captured sink: {errors:?}"
    );
}

/// A clean script contributes nothing. (Ends on a parenthesized expression:
/// a final `let` and a final bare ident each trip unrelated quirks.)
#[test]
fn clean_parse_captures_nothing() {
    let mut vm = test_vm();
    let errors = eval_captured(&mut vm, "clean", "let x = 1 + 2\n(x + 1)");
    assert!(errors.is_empty(), "unexpected errors: {errors:?}");
}

/// The append/streaming eval path surfaces parse errors the same way.
#[test]
fn streaming_eval_surfaces_parse_errors() {
    let mut vm = test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let code = DANGLING_ELSE;
    vm.with_instruction_limit(500_000, |vm| {
        vm.eval_with_append_source(
            ScriptMod::new("", "stream#1", "parse_error_capture_stream", 0, 0, "", &[]),
            code,
            NIL.into(),
        )
    });
    let errors = vm.take_errors();
    assert!(
        errors.iter().any(|e| e.contains("Unexpected else")),
        "streaming parse error missing from captured sink: {errors:?}"
    );
}

#[test]
fn unconsumable_loop_body_is_rejected_before_execution() {
    let mut vm = test_vm();
    // Recovery used to emit an empty infinite loop for this missing body.
    let code = "loop + :\n;";
    for streaming in [false, true] {
        for _ in 0..2 {
            vm.bx.captured_errors = Some(Vec::new());
            let value = vm.with_instruction_limit(100, |vm| {
                if streaming {
                    vm.eval_with_append_source(
                        ScriptMod::new("", "", "malformed_stream.splash", 0, 0, "", &[]),
                        code,
                        NIL.into(),
                    )
                } else {
                    vm.eval(ScriptMod::new("", "", "malformed.splash", 0, 0, code, &[]))
                }
            });
            assert!(value.is_err(), "malformed body must return an error");
            assert_eq!(value, ScriptValue::script_err_unexpected(value.as_err().unwrap().ip));
            let errors = vm.take_error_records();
            if !streaming {
                assert!(errors.iter().any(|error| {
                    error.message.contains("Expected expression")
                        // The tokenizer's diagnostic cursor is just after ':'.
                        && error.loc.as_ref().is_some_and(|loc| loc.line == 0 && loc.col == 8)
                }), "parse error must retain its source position: {errors:?}");
            }
            assert!(errors.iter().all(|error| !error.message.contains("instruction limit")),
                "recovery bytecode must never execute: {errors:?}");
        }
    }
    // The rejection must also apply without a diagnostics sink.
    let value = vm.with_instruction_limit(100, |vm| {
        vm.eval(ScriptMod::new("", "", "malformed.splash", 0, 0, code, &[]))
    });
    assert_eq!(value, ScriptValue::script_err_unexpected(value.as_err().unwrap().ip));
    assert_eq!(vm.eval(ScriptMod::new("", "", "recovered.splash", 0, 0, "2 + 3", &[])).as_number(), Some(5.0));
}
