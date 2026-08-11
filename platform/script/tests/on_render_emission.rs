//! Regression tests: widget/object emission inside literal bodies, closures,
//! branches and loops — the "on_render silently loses widgets" family.
//!
//! Empirically (host_launcher, 2026-08-06/07): expression statements inside an
//! object-literal body (or a closure called with a `me`) commit their value as
//! a child via a POP_TO_ME fused onto the statement's last opcode. Several
//! control-flow shapes broke that contract silently:
//! - an if/else whose branch emits: the statement-level commit was re-fused
//!   onto the ELSE branch's tail, which a taken TRUE branch jumps over (the
//!   same skipped-fused-flag disease as the fixed short-circuit-argument bug);
//! - `elif` chains: the arm's IF_ELSE jump was never patched (relative jump
//!   of 0), spinning the interpreter forever;
//! - `for x in <non-iterable>` silently skipped the body where `while` errors;
//! - a line-leading `{` after a value-ending line was glued onto the previous
//!   expression as a proto instantiation instead of starting a new statement;
//! - int vs float second-class-ness: numeric subtypes (U40/I32/...) didn't
//!   collapse to the number bucket in ScriptTypeRedux, so `6 .abs()` missed
//!   method dispatch and int args tripped float-default type checks with the
//!   absurd message "expected number, got number".

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
    // Bounded: a regression that re-introduces the elif hang must fail the
    // test, not wedge the suite.
    vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: format!("on_render_emission_{name}"),
            line: 0,
            column: 0,
            code: code.to_string(),
            values: vec![],
        })
    })
}

/// The anonymous children (vec entries) of the object `v`, as their `v:` field
/// numbers. Nil for entries without one.
fn child_vs(vm: &mut ScriptVm, v: ScriptValue) -> Vec<Option<f64>> {
    let obj = v.as_object().expect("expected an object result");
    let len = vm.bx.heap.iter_len(obj);
    (0..len)
        .map(|i| {
            let kv = vm.bx.heap.iter_key_value(obj, i, vm.trap());
            let child = kv.value.as_object()?;
            vm.bx
                .heap
                .value(child, id!(v).into(), vm.trap())
                .as_number()
        })
        .collect()
}

// ---------------------------------------------------------------------------
// if / else emission inside a literal body
// ---------------------------------------------------------------------------

#[test]
fn if_else_taken_true_branch_emits() {
    let vm = &mut test_vm();
    let v = eval_str(vm, "if_true", "{ if true { {v: 1} } else { {v: 2} } }");
    assert_eq!(child_vs(vm, v), vec![Some(1.0)]);
}

#[test]
fn if_else_taken_else_branch_emits() {
    let vm = &mut test_vm();
    let v = eval_str(vm, "if_false", "{ if false { {v: 1} } else { {v: 2} } }");
    assert_eq!(child_vs(vm, v), vec![Some(2.0)]);
}

#[test]
fn bare_if_taken_emits_and_untaken_emits_nothing() {
    let vm = &mut test_vm();
    let v = eval_str(vm, "bare_taken", "{ if true { {v: 1} } }");
    assert_eq!(child_vs(vm, v), vec![Some(1.0)]);
    let v = eval_str(vm, "bare_untaken", "{ if false { {v: 1} } }");
    assert_eq!(child_vs(vm, v), Vec::<Option<f64>>::new());
}

/// The exact live shape that broke the calendar/weather apps: branch emission
/// nested in a while loop inside a literal body.
#[test]
fn if_else_inside_while_inside_literal() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "if_in_while",
        r#"{
    let i = 0
    while i < 3 {
        if i < 2 { {v: i} } else { {v: 9} }
        i += 1
    }
}"#,
    );
    assert_eq!(child_vs(vm, v), vec![Some(0.0), Some(1.0), Some(9.0)]);
}

/// Emission where the if/else statement is followed by more statements — the
/// join lands mid-body rather than at the literal's end.
#[test]
fn if_else_followed_by_more_statements() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "if_then_more",
        "{ if true { {v: 1} } else { {v: 2} } {v: 3} }",
    );
    assert_eq!(child_vs(vm, v), vec![Some(1.0), Some(3.0)]);
}

// ---------------------------------------------------------------------------
// elif
// ---------------------------------------------------------------------------

#[test]
fn elif_chain_values() {
    let vm = &mut test_vm();
    // Results read through `echo(r)` — a call closes cleanly as the final
    // statement. A bare ident there is consumed by a pre-existing do-call
    // glue quirk, and `let out = r\nout` only ever worked because auto_close
    // used to DROP the trailing let (fixed now: the let binds, so the idiom
    // returns nil).
    for (name, code, expect) in [
        (
            "t_t",
            "fn echo(x){ x }\nlet r = 0\nif true { r = 1 } elif true { r = 2 }\necho(r)",
            1.0,
        ),
        (
            "f_t",
            "fn echo(x){ x }\nlet r = 0\nif false { r = 1 } elif true { r = 2 }\necho(r)",
            2.0,
        ),
        (
            "f_f",
            "fn echo(x){ x }\nlet r = 0\nif false { r = 1 } elif false { r = 2 }\necho(r)",
            0.0,
        ),
        (
            "f_t_else",
            "fn echo(x){ x }\nlet r = 0\nif false { r = 1 } elif true { r = 2 } else { r = 3 }\necho(r)",
            2.0,
        ),
        (
            "f_f_else",
            "fn echo(x){ x }\nlet r = 0\nif false { r = 1 } elif false { r = 2 } else { r = 3 }\necho(r)",
            3.0,
        ),
        (
            "chain",
            "fn echo(x){ x }\nlet r = 0\nif false { r = 1 } elif false { r = 2 } elif true { r = 3 } else { r = 4 }\necho(r)",
            3.0,
        ),
    ] {
        let v = eval_str(vm, name, code);
        assert_eq!(v.as_number(), Some(expect), "{name}: got {v:?}");
    }
}

#[test]
fn elif_as_expression_value() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "elif_expr",
        "fn echo(v){ v }\nlet x = if false { 1 } elif true { 2 } else { 3 }\necho(x)",
    );
    assert_eq!(v.as_number(), Some(2.0), "got {v:?}");
}

#[test]
fn elif_arm_emits_into_literal() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "elif_emit",
        "{ let n = 1 if n == 0 { {v: 1} } elif n == 1 { {v: 2} } else { {v: 3} } }",
    );
    assert_eq!(child_vs(vm, v), vec![Some(2.0)]);
}

// ---------------------------------------------------------------------------
// match / try emission joins
// ---------------------------------------------------------------------------

#[test]
fn match_arm_emits_into_literal() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "match_emit",
        r#"{
    let n = 2
    match n {
        1 => { {v: 1} }
        2 => { {v: 2} }
        _ => { {v: 9} }
    }
}"#,
    );
    assert_eq!(child_vs(vm, v), vec![Some(2.0)]);
}

// ---------------------------------------------------------------------------
// Closure contract: children committed while running with a `me`, final
// statement RETURNED (the host is responsible for committing it if desired).
// ---------------------------------------------------------------------------

#[test]
fn closure_final_statement_is_returned() {
    let vm = &mut test_vm();
    let _ = eval_str(
        vm,
        "closure_final",
        r#"
fn build(){
    {v: 7}
}
"#,
    );
    let scope = {
        let bodies = vm.bx.code.bodies.borrow();
        bodies
            .iter()
            .find_map(|body| match &body.source {
                ScriptSource::Mod(m) if m.file == "on_render_emission_closure_final" => {
                    Some(body.scope.as_object())
                }
                _ => None,
            })
            .expect("body scope")
    };
    let fnval = vm.bx.heap.scope_value(scope, live_id!(build), vm.trap());
    let result = vm.call(fnval, &[]);
    let obj = result.as_object().expect("closure should return the literal");
    let v = vm.bx.heap.value(obj, id!(v).into(), vm.trap());
    assert_eq!(v.as_number(), Some(7.0));
}

// ---------------------------------------------------------------------------
// for-in
// ---------------------------------------------------------------------------

#[test]
fn for_in_array_emits_into_literal() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "for_arr",
        "{ let xs = [4 5 6] for x in xs { {v: x} } }",
    );
    assert_eq!(child_vs(vm, v), vec![Some(4.0), Some(5.0), Some(6.0)]);
}

/// A for-in over something non-iterable must ERROR, not silently skip the
/// body — the silent skip is what made `for`-based renders "emit nothing"
/// with no diagnostic while a `while` rewrite errored visibly.
#[test]
fn for_in_non_iterable_errors() {
    let vm = &mut test_vm();
    for (name, code) in [
        (
            "for_string",
            "fn echo(x){ x }\nlet r = 0\nfor x in \"abc\" { r = 1 }\necho(r)",
        ),
        (
            "for_bool",
            "fn echo(x){ x }\nlet r = 0\nfor x in true { r = 1 }\necho(r)",
        ),
    ] {
        // The error is reported through the trap (the loop body is skipped
        // and the script continues), so capture the error stream.
        vm.bx.captured_errors = Some(Vec::new());
        let _ = eval_str(vm, name, code);
        let errors = vm.bx.captured_errors.take().unwrap();
        assert!(
            errors.iter().any(|e| e.contains("not iterable")),
            "{name}: expected a not-iterable error, got {errors:?}"
        );
    }
}

/// Nil and empty sources stay a silent no-iteration (the "no data yet" case).
#[test]
fn for_in_nil_and_empty_are_silent() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "for_nil",
        "fn echo(x){ x }\nlet r = 0\nfor x in nil { r = 1 }\necho(r)",
    );
    assert_eq!(v.as_number(), Some(0.0), "nil: got {v:?}");
    let v = eval_str(
        vm,
        "for_empty",
        "fn echo(x){ x }\nlet r = 0\nlet xs = []\nfor x in xs { r = 1 }\necho(r)",
    );
    assert_eq!(v.as_number(), Some(0.0), "empty: got {v:?}");
    let v = eval_str(
        vm,
        "for_zero",
        "fn echo(x){ x }\nlet r = 0\nfor x in 0 { r = 1 }\necho(r)",
    );
    assert_eq!(v.as_number(), Some(0.0), "zero count: got {v:?}");
}

/// `for k v in <number>` used to bind key/index through swapped arguments.
/// Two-var number loops follow FOR_2 semantics: k = index, v = value.
#[test]
fn for_two_vars_over_number_binds_index_and_value() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "for_kv_num",
        "fn echo(x){ x }\nlet xs = []\nfor k v in 3 { xs.push(k) xs.push(v) }\necho(xs.len())",
    );
    assert_eq!(v.as_number(), Some(6.0), "got {v:?}");
    let v = eval_str(
        vm,
        "for_kv_num_sum",
        "fn echo(x){ x }\nlet s = 0\nfor k v in 3 { s = s * 10 + k }\necho(s)",
    );
    assert_eq!(v.as_number(), Some(12.0), "keys should be 0,1,2: got {v:?}");
}

// ---------------------------------------------------------------------------
// Line-leading `{` starts a new statement (like the `(`/`[` newline rule)
// ---------------------------------------------------------------------------

#[test]
fn line_leading_brace_is_a_new_statement() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "nl_brace",
        "{\n    let x = 5\n    {v: x}\n}",
    );
    assert_eq!(child_vs(vm, v), vec![Some(5.0)]);
}

/// Same-line proto instantiation must keep working.
#[test]
fn same_line_proto_instantiation_still_works() {
    let vm = &mut test_vm();
    // Reads go through further `let`s: a bare ident line after a `}`- or
    // `;`-ending line trips separate, pre-existing statement-glue quirks that
    // are out of this fix's scope. The assertion here is only that SAME-LINE
    // proto instantiation keeps working with the newline-`{` divert in place.
    let v = eval_str(
        vm,
        "same_line_proto",
        "fn echo(x){ x }\nlet p = {a: 1}\nlet q = p{b: 2}\necho(q.b)",
    );
    assert_eq!(v.as_number(), Some(2.0), "got {v:?}");
    let v = eval_str(
        vm,
        "proto_field",
        "fn echo(x){ x }\nlet p = {a: 1}\nlet q = p{b: 2}\necho(q.a)",
    );
    assert_eq!(v.as_number(), Some(1.0), "proto chain: got {v:?}");
}

// ---------------------------------------------------------------------------
// Numeric subtypes are all "number"
// ---------------------------------------------------------------------------

#[test]
fn int_receiver_dispatches_number_methods() {
    let vm = &mut test_vm();
    // is_number is registered under the number bucket; an int receiver used
    // to miss dispatch entirely ("method not found on unknown(6)").
    let v = eval_str(vm, "int_isnum", "let x = 6\nx.is_number()");
    assert_eq!(v.as_bool(), Some(true), "int receiver: got {v:?}");
    let v = eval_str(vm, "float_isnum", "let y = 6.0\ny.is_number()");
    assert_eq!(v.as_bool(), Some(true), "float receiver: got {v:?}");
}

#[test]
fn int_argument_accepted_for_float_default() {
    let vm = &mut test_vm();
    let v = eval_str(vm, "int_arg", "fn f(a = 1.0){ a }\nf(2)");
    assert_eq!(v.as_number(), Some(2.0), "got {v:?}");
    let v = eval_str(vm, "float_arg_int_default", "fn f(a = 1){ a }\nf(2.5)");
    assert_eq!(v.as_number(), Some(2.5), "got {v:?}");
}
