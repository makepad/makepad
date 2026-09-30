//! Regression tests for the language gaps found while authoring the
//! reference-film plates (FP1, 2026-09-30).

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host, bx: Box::new(ScriptVmBase::new()) }
}

fn run(name: &str, code: &str) -> (ScriptValue, Vec<String>, ScriptVm<'static>) {
    let mut vm = test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: format!("fp1_{name}"),
            line: 0,
            column: 0,
            code: code.to_string(),
            values: vec![],
        })
    });
    let mut errs: Vec<String> = vm.bx.captured_errors.take().unwrap().into_iter().map(|e| format!("{e:?}")).collect();
    errs.extend(vm.take_errors().into_iter().map(|e| format!("{e:?}")));
    (value, errs, vm)
}

fn number(name: &str, code: &str) -> f64 {
    let (value, errs, _vm) = run(name, code);
    assert!(errs.is_empty(), "{name}: eval errors: {errs:?}");
    value.as_number().unwrap_or_else(|| panic!("{name}: not a number: {value:?}"))
}

/// A binary op folds a constant right operand into its immediate. The fold
/// took the last constant of an if/match arm or a `||` operand too, and
/// popping it moved their jump targets: `var z = 1.0 * (if v { 0.2 } else
/// { 1.0 })` skipped the declaration when the true arm ran.
#[test]
fn an_op_after_an_if_ending_in_a_whole_float() {
    let f = |expr: &str, arg: &str| format!("let v = 2\nlet f = fn(c) {{\n    var z = {expr}\n    return z\n}}\nf({arg})");
    for (name, expr, t, e) in [
        ("fp1", "1.0 * (if v == 2 { 0.2 } else { 1.0 })", 0.2, 0.2),
        ("param", "1.0 * (if c { 0.2 } else { 1.0 })", 0.2, 1.0),
        ("mul", "3 * (if c { 0.5 } else { 2.0 })", 1.5, 6.0),
        ("add", "1 + (if c { 2 } else { 3.0 })", 3.0, 4.0),
        ("sub", "10 - (if c { 2.5 } else { 4.0 })", 7.5, 6.0),
        ("nested", "2 * (1 + (if c { 1 } else { 3.0 }))", 4.0, 8.0),
        ("elif", "2 * (if c { 1 } elif v == 3 { 5 } else { 3.0 })", 2.0, 6.0),
        ("match", "2 * (match c { true => 1.5 _ => 3.0 })", 3.0, 6.0),
        ("or", "2 * (c || 3.0)", 2.0, 6.0),
        ("nil_or", "2 * ((if c { 1.5 } else { nil }) |? 3.0)", 3.0, 6.0),
        ("cam", "1.0 * (1.0 + 0.5 * 2.0 * (if c { 0.2 } else { 1.0 }))", 1.2, 2.0),
    ] {
        assert_eq!(number(name, &f(expr, "true")), t, "{name} (true)");
        assert_eq!(number(name, &f(expr, "false")), e, "{name} (false)");
        let with_let = f(expr, "true").replace("var z", "let z");
        assert_eq!(number(name, &with_let), t, "{name} (let)");
    }
    // Constants still fold where they are the whole operand.
    assert_eq!(number("plain", "let a = 7\na * 2.0 + a - 3.0"), 18.0);
}
