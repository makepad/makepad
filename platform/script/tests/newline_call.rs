//! Regression tests for the postfix-call same-line rule: a `(` on a new line
//! starts a new expression instead of calling the previous line's value
//! (previously `x % 7` followed by `(h + 6) % 7` on the next line parsed as
//! calling the number 7 — found via host_launcher's calendar mini-app).

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host,
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn eval_str(vm: &mut ScriptVm, name: &str, code: &str) -> ScriptValue {
    vm.eval(ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: format!("newline_call_{name}"),
        line: 0,
        column: 0,
        code: code.to_string(),
        values: vec![],
    })
}

/// The calendar repro: a parenthesized expression on its own line after a
/// value-producing statement must be a new statement, not a call.
#[test]
fn paren_on_new_line_is_not_a_call() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "zeller",
        r#"
fn weekday(){
    let h = 16.0 % 7.0
    (h + 6.0) % 7.0
}
let keep = weekday()
keep
"#,
    );
    assert_eq!(v.as_number(), Some(1.0), "got {:?}", v);
}

/// Same-line calls must keep working, including with a space before the paren.
#[test]
fn same_line_calls_still_work() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "sameline",
        r#"
fn double(x){ x * 2.0 }
let a = double(4.0)
let b = double (5.0)
a + b
"#,
    );
    assert_eq!(v.as_number(), Some(18.0), "got {:?}", v);
}

/// Multi-line argument lists (paren on the callee's line) keep working.
#[test]
fn multiline_args_still_work() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "multiline",
        r#"
fn add(x, y){ x + y }
add(
    1.0,
    2.0
)
"#,
    );
    assert_eq!(v.as_number(), Some(3.0), "got {:?}", v);
}

// -- Consistency across ALL continuation tokens --
// A leading continuation token on a new line begins a new statement; the same
// token on the same line still continues the expression; a TRAILING operator
// still continues onto the next line.

fn n(vm: &mut ScriptVm, name: &str, code: &str) -> Option<f64> {
    eval_str(vm, name, code).as_number()
}

/// Wrap the body in a function and call it, so the last expression is an
/// unambiguous implicit return (top-level multi-bare-statement return value is
/// its own thing and would muddy these assertions).
fn in_fn(vm: &mut ScriptVm, name: &str, body: &str) -> Option<f64> {
    eval_str(vm, name, &format!("fn f(){{\n{body}\n}}\nf()")).as_number()
}

#[test]
fn index_bracket_newline_is_new_statement() {
    let vm = &mut test_vm();
    // A standalone `[9 8]` on its own line does not index the previous value,
    // so `a` keeps its line-1 value.
    assert_eq!(in_fn(vm, "idx_nl", "let a = 3.0\n[9.0 8.0]\na"), Some(3.0));
    // Same line: indexing still continues.
    assert_eq!(in_fn(vm, "idx_same", "let a = [5.0 6.0 7.0]\na[1]"), Some(6.0));
}

#[test]
fn leading_binary_operator_newline_continues_expression() {
    let vm = &mut test_vm();
    // A leading INFIX operator on a new line CONTINUES the expression (it can't
    // start a statement), so `let a = 3\n <op> 2` folds into one binding. This is
    // load-bearing: makepad's shader DSL breaks long math this way, e.g.
    // `let color = sample() * 0.125\n + (...) * 0.03125` (widgets/src/window.rs).
    assert_eq!(in_fn(vm, "minus_nl", "let a = 3.0\n- 2.0\na"), Some(1.0));
    assert_eq!(in_fn(vm, "plus_nl", "let a = 3.0\n+ 2.0\na"), Some(5.0));
    assert_eq!(in_fn(vm, "star_nl", "let a = 3.0\n* 2.0\na"), Some(6.0));
    assert_eq!(in_fn(vm, "pct_nl", "let a = 7.0\n% 2.0\na"), Some(1.0));
    // A multi-line chain of leading `+` (the shader-DSL pattern), at statement
    // level (not inside brackets).
    assert_eq!(
        in_fn(vm, "chain", "let a = 1.0\n+ 2.0\n+ 3.0\n+ 4.0\na"),
        Some(10.0)
    );
    // Same line still computes.
    assert_eq!(in_fn(vm, "minus_same", "let a = 3.0 - 2.0\na"), Some(1.0));
}

#[test]
fn trailing_operator_still_continues_across_newline() {
    let vm = &mut test_vm();
    assert_eq!(in_fn(vm, "trail_minus", "let a = 3.0 -\n2.0\na"), Some(1.0));
    assert_eq!(in_fn(vm, "trail_plus", "let a = 3.0 +\n2.0\na"), Some(5.0));
    // Trailing open-paren keeps a multi-line arg list working.
    assert_eq!(n(vm, "trail_paren", "fn add(x,y){x+y}\nadd(\n  4.0,\n  5.0\n)"), Some(9.0));
}

#[test]
fn field_access_same_line_still_reads() {
    let vm = &mut test_vm();
    // Same-line field access keeps working (the common `obj.field` form the
    // mini-apps use, e.g. `tracks[current].title`).
    assert_eq!(in_fn(vm, "field_same", "let ts = [{v: 5.0}]\nts[0].v"), Some(5.0));
}

#[test]
fn leading_operator_inside_parens_still_continues() {
    // Inside `( )` newlines are insignificant: a long expression can break across
    // lines with a LEADING operator (makepad's shader DSL relies on this).
    let vm = &mut test_vm();
    assert_eq!(
        in_fn(vm, "paren_multiline", "let u = (\n  3.0\n  - 2.0\n  - 4.0\n)\nu"),
        Some(-3.0)
    );
    // Nested: leading op inside an inner paren, several lines.
    assert_eq!(
        in_fn(vm, "paren_nested", "let u = (\n  10.0\n  * (1.0\n     + 1.0)\n)\nu"),
        Some(20.0)
    );
    // But a call `(` at statement level inside the SAME function still diverts.
    assert_eq!(
        in_fn(vm, "stmt_paren", "let h = 16.0 % 7.0\n(h + 6.0) % 7.0\nh"),
        Some(2.0)
    );
}
