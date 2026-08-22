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
    let host = Box::leak(Box::new(0i32));
    let std = Box::leak(Box::new(0i32));
    ScriptVm {
        host,
        std,
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn eval_captured(vm: &mut ScriptVm, name: &str, code: &str) -> Vec<String> {
    vm.bx.captured_errors = Some(Vec::new());
    vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: format!("parse_error_capture_{name}"),
            line: 0,
            column: 0,
            code: code.to_string(),
            values: vec![],
        })
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
            ScriptMod {
                cargo_manifest_path: String::new(),
                module_path: "stream#1".to_string(),
                file: "parse_error_capture_stream".to_string(),
                line: 0,
                column: 0,
                code: String::new(),
                values: vec![],
            },
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
