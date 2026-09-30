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

fn fails(name: &str, code: &str) -> Vec<String> {
    let (_value, errs, _vm) = run(name, code);
    assert!(!errs.is_empty(), "{name}: expected an error");
    errs
}

const PRELUDE: &str = "use mod.math.*\nuse mod.pod.*\n";

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

/// `int(x)` (and `i32`, `u32`, `f32`, `f16`) make a scalar pod: it indexes
/// arrays and does arithmetic as the number it holds.
#[test]
fn int_values_index_arrays() {
    let a = format!("{PRELUDE}let a = [5, 6, 7]\n");
    assert_eq!(number("int_lit", &format!("{a}a[int(1.7)]")), 6.0);
    assert_eq!(number("int_var", &format!("{a}let i = int(2)\na[i]")), 7.0);
    assert_eq!(number("i32", &format!("{a}a[i32(0)]")), 5.0);
    assert_eq!(number("u32", &format!("{a}a[u32(2)]")), 7.0);
    assert_eq!(number("f32", &format!("{a}a[f32(1.0)]")), 6.0);
    assert_eq!(number("sum", &format!("{a}a[int(0.9) + 1]")), 6.0);
    assert_eq!(number("arith", &format!("{PRELUDE}int(2.9) * 1.5 + u32(1)")), 4.0);
    assert_eq!(number("cmp", &format!("{PRELUDE}if int(2.5) < 3 {{ 1 }} else {{ 0 }}")), 1.0);
    assert_eq!(number("loop", &format!("{PRELUDE}let a = [1, 2, 3]\nvar s = 0\nfor k in 0..3 {{ s += a[int(k)] }}\ns")), 6.0);
    assert_eq!(number("assign", &format!("{a}a[int(1)] = 9\na[1]")), 9.0);
    // Still only whole, non-negative, in-range indices.
    fails("negative", &format!("{a}a[int(-1)]"));
    fails("fraction", &format!("{a}a[f32(0.5)]"));
    // `float(1.0)` stays a pod, which shader declarations take their type from.
    let (value, errs, _vm) = run("pod", &format!("{PRELUDE}float(1.0)"));
    assert!(errs.is_empty() && value.as_pod().is_some(), "{value:?} {errs:?}");
}
