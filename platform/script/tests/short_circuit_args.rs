//! Regression tests: short-circuit `&&`/`||` used directly as a call argument.
//!
//! Empirically (host_launcher, 2026-07-16): when an `&&` whose short-circuit
//! jump is taken (falsy LHS) skips a multi-opcode RHS (e.g. a comparison), and
//! the expression is the top-level argument of a call, the argument arrives as
//! NIL instead of the LHS value. Let-bound forms, `if` conditions, and concat
//! sub-expressions are unaffected.

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
    vm.eval(ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: format!("short_circuit_args_{name}"),
        line: 0,
        column: 0,
        code: code.to_string(),
        values: vec![],
    })
}

/// The core repro: falsy `&&` with a comparison RHS, passed straight to a fn.
#[test]
fn and_short_circuit_as_call_argument() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "and_arg",
        r#"
fn echo(x){ x }
let w = 170.0
echo((w >= 280.0) && (w < 999.0))
"#,
    );
    assert_eq!(v.as_bool(), Some(false), "expected false, got {:?}", v);
}

/// Same expression, unparenthesized chain.
#[test]
fn and_short_circuit_unparenthesized_as_call_argument() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "and_arg_unparen",
        r#"
fn echo(x){ x }
let w = 170.0
echo(w >= 280.0 && w < 999.0)
"#,
    );
    assert_eq!(v.as_bool(), Some(false), "expected false, got {:?}", v);
}

/// Var LHS falsy, comparison RHS.
#[test]
fn and_short_circuit_var_lhs_as_call_argument() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "and_arg_var_lhs",
        r#"
fn echo(x){ x }
let w = 170.0
let b = w >= 280.0
echo(b && (w < 999.0))
"#,
    );
    assert_eq!(v.as_bool(), Some(false), "expected false, got {:?}", v);
}

/// || with its short-circuit taken (truthy LHS), comparison RHS, as argument.
#[test]
fn or_short_circuit_as_call_argument() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "or_arg",
        r#"
fn echo(x){ x }
let w = 170.0
echo((w < 999.0) || (w >= 280.0))
"#,
    );
    assert_eq!(v.as_bool(), Some(true), "expected true, got {:?}", v);
}

/// Second argument position must work too.
#[test]
fn and_short_circuit_as_second_call_argument() {
    let vm = &mut test_vm();
    let v = eval_str(
        vm,
        "and_arg_second",
        r#"
fn second(a, b){ b }
let w = 170.0
second(1.0, (w >= 280.0) && (w < 999.0))
"#,
    );
    assert_eq!(v.as_bool(), Some(false), "expected false, got {:?}", v);
}

/// Control cases that were already correct, pinned so they stay correct.
#[test]
fn short_circuit_control_cases() {
    let vm = &mut test_vm();
    // Jump over a bare-variable RHS.
    let v = eval_str(
        vm,
        "ctl_var_rhs",
        r#"
fn echo(x){ x }
let w = 170.0
let a = w < 190.0
let b = w >= 280.0
echo(b && a)
"#,
    );
    assert_eq!(v.as_bool(), Some(false), "var RHS: got {:?}", v);

    // No jump taken (truthy LHS).
    let v = eval_str(
        vm,
        "ctl_no_jump",
        r#"
fn echo(x){ x }
let w = 170.0
echo((w < 999.0) && (w >= 280.0))
"#,
    );
    assert_eq!(v.as_bool(), Some(false), "no-jump: got {:?}", v);

    // Let-bound then passed.
    let v = eval_str(
        vm,
        "ctl_let_bound",
        r#"
fn echo(x){ x }
let w = 170.0
let md = (w >= 280.0) && (w < 999.0)
echo(md)
"#,
    );
    assert_eq!(v.as_bool(), Some(false), "let-bound: got {:?}", v);

    // Concat sub-expression.
    let v = eval_str(
        vm,
        "ctl_concat",
        r#"
let w = 170.0
"" + ((w >= 280.0) && (w < 999.0))
"#,
    );
    assert!(
        vm.bx.heap.temp_string_with(|heap, out| {
            heap.cast_to_string(v, out);
            out == "false"
        }),
        "concat: expected \"false\""
    );

    // If condition.
    let v = eval_str(
        vm,
        "ctl_if",
        r#"
let w = 170.0
let r = 1.0
if (w >= 280.0) && (w < 999.0) { r = 2.0 }
r
"#,
    );
    assert_eq!(v.as_number(), Some(1.0), "if: got {:?}", v);
}

/// Replicates the Splash host->script path: the fn is resolved from the body
/// scope and invoked via `vm.call` (as `Splash::call_script_fn` does), rather
/// than being called from script code.
#[test]
fn and_short_circuit_in_host_called_fn() {
    let vm = &mut test_vm();
    let _ = eval_str(
        vm,
        "host_call",
        r#"
fn echo(x){ x }
fn probe(w){
    echo((w >= 280.0) && (w < 999.0))
}
"#,
    );
    // Resolve `probe` from the body scope, mirroring call_script_fn.
    let scope = {
        let bodies = vm.bx.code.bodies.borrow();
        bodies
            .iter()
            .find_map(|body| match &body.source {
                ScriptSource::Mod(m) if m.file == "short_circuit_args_host_call" => {
                    Some(body.scope.as_object())
                }
                _ => None,
            })
            .expect("body scope")
    };
    let fnval = vm.bx.heap.scope_value(scope, live_id!(probe), vm.trap());
    assert!(!fnval.is_nil(), "probe fn not found in scope");
    let result = vm.call(fnval, &[170.0f64.into()]);
    assert_eq!(
        result.as_bool(),
        Some(false),
        "host-called fn: expected false, got {:?}",
        result
    );
}

/// Narrower probes inside a host-called fn: which contexts break under vm.call?
#[test]
fn host_called_fn_context_matrix() {
    let vm = &mut test_vm();
    let _ = eval_str(
        vm,
        "host_matrix",
        r#"
fn echo(x){ x }
fn bare(w){
    (w >= 280.0) && (w < 999.0)
}
fn letbound(w){
    let v = (w >= 280.0) && (w < 999.0)
    v
}
fn letbound_arg(w){
    let v = (w >= 280.0) && (w < 999.0)
    echo(v)
}
fn arg_var_rhs(w){
    let b = w < 190.0
    echo((w >= 280.0) && b)
}
"#,
    );
    let scope = {
        let bodies = vm.bx.code.bodies.borrow();
        bodies
            .iter()
            .find_map(|body| match &body.source {
                ScriptSource::Mod(m) if m.file == "short_circuit_args_host_matrix" => {
                    Some(body.scope.as_object())
                }
                _ => None,
            })
            .expect("body scope")
    };
    let call = |vm: &mut ScriptVm, name: LiveId| -> ScriptValue {
        let f = vm.bx.heap.scope_value(scope, name, vm.trap());
        vm.call(f, &[170.0f64.into()])
    };
    let bare = call(vm, live_id!(bare));
    let letb = call(vm, live_id!(letbound));
    let letb_arg = call(vm, live_id!(letbound_arg));
    let var_rhs = call(vm, live_id!(arg_var_rhs));
    println!("bare={bare:?} letbound={letb:?} letbound_arg={letb_arg:?} arg_var_rhs={var_rhs:?}");
    assert_eq!(bare.as_bool(), Some(false), "bare: {:?}", bare);
    assert_eq!(letb.as_bool(), Some(false), "letbound: {:?}", letb);
    assert_eq!(letb_arg.as_bool(), Some(false), "letbound_arg: {:?}", letb_arg);
    assert_eq!(var_rhs.as_bool(), Some(false), "arg_var_rhs: {:?}", var_rhs);
}

/// Diagnostic (not a regression test): dump opcodes + execution trace.
#[test]
#[ignore]
fn zz_debug_dump() {
    let vm = &mut test_vm();
    let _ = eval_str(
        vm,
        "dump",
        r#"
fn echo(x){ x }
fn probe(w){
    echo((w >= 280.0) && (w < 999.0))
}
"#,
    );
    {
        let bodies = vm.bx.code.bodies.borrow();
        for body in bodies.iter() {
            if let ScriptSource::Mod(m) = &body.source {
                if m.file == "short_circuit_args_dump" {
                    for (i, op) in body.parser.opcodes.iter().enumerate() {
                        if let Some((o, a)) = op.as_opcode() {
                            eprintln!("{i:3}: {o:?} {a:?}");
                        } else {
                            eprintln!("{i:3}: VALUE {op:?}");
                        }
                    }
                }
            }
        }
    }
    let scope = {
        let bodies = vm.bx.code.bodies.borrow();
        bodies
            .iter()
            .find_map(|body| match &body.source {
                ScriptSource::Mod(m) if m.file == "short_circuit_args_dump" => {
                    Some(body.scope.as_object())
                }
                _ => None,
            })
            .expect("body scope")
    };
    let fnval = vm.bx.heap.scope_value(scope, live_id!(probe), vm.trap());
    vm.bx.debug_trace = true;
    let result = vm.call(fnval, &[170.0f64.into()]);
    vm.bx.debug_trace = false;
    eprintln!("RESULT: {result:?}");
}

/// Diagnostic: same fn, called FROM SCRIPT, traced.
#[test]
#[ignore]
fn zz_debug_dump_scriptcall() {
    let vm = &mut test_vm();
    vm.bx.debug_trace = true;
    let v = eval_str(
        vm,
        "dump2",
        r#"
fn echo(x){ x }
fn probe(w){
    echo((w >= 280.0) && (w < 999.0))
}
probe(170.0)
"#,
    );
    vm.bx.debug_trace = false;
    eprintln!("RESULT: {v:?}");
}
