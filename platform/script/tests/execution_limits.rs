//! Regression tests for the operand-stack and call-frame caps that constrained
//! hosts install per evaluation (`with_stack_value_limit`,
//! `with_call_frame_limit`), for `clear_execution_limit_failures`, and for
//! `ScriptNative::clear_type_methods`. Limit hits are uncatchable VM bails:
//! `try`/`catch` cannot recover them, but the next evaluation on the same VM
//! starts from a clean root state.

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
    vm.with_instruction_limit(100_000, |vm| {
        vm.eval(ScriptMod {
            file: format!("execution_limits_{name}.octoscript"),
            code: format!("{code}\n;"),
            ..Default::default()
        })
    })
}

/// `take_errors` moves the captured sink out; re-arm it the way a host does
/// before its next evaluation so later diagnostics stay observable.
fn take_errors(vm: &mut ScriptVm) -> Vec<String> {
    let errors = ScriptVm::take_errors(vm);
    vm.bx.captured_errors = Some(Vec::new());
    errors
}

fn errors_mention(vm: &mut ScriptVm, needle: &str) -> bool {
    take_errors(vm).iter().any(|e| e.contains(needle))
}

#[test]
fn operand_stack_limit_is_uncatchable_and_the_vm_recovers() {
    let mut vm = test_vm();
    let exceeded = vm.with_stack_value_limit(1, |vm| eval(vm, "stack", "try (1 + 2) catch 99"));
    assert!(exceeded.is_err(), "{exceeded:?}");
    assert!(errors_mention(&mut vm, "operand stack limit exceeded"));
    assert!(!vm.thread().is_paused());

    let recovered = eval(&mut vm, "stack_recover", "2");
    assert_eq!(recovered.as_number(), Some(2.0));
    assert!(take_errors(&mut vm).is_empty());
}

#[test]
fn operand_stack_limit_restores_the_default_ceiling_after_the_scope() {
    let mut vm = test_vm();
    let _ = vm.with_stack_value_limit(1, |vm| eval(vm, "stack_scope", "1 + 2"));
    let _ = take_errors(&mut vm);
    let value = eval(&mut vm, "stack_after", "1 + 2");
    assert_eq!(value.as_number(), Some(3.0));
}

#[test]
fn call_frame_limit_is_uncatchable_and_the_vm_recovers() {
    let mut vm = test_vm();
    let exceeded = vm.with_call_frame_limit(4, |vm| {
        eval(
            vm,
            "frames",
            "fn recurse(value) {\nrecurse(value + 1)\n}\ntry recurse(0) catch 99",
        )
    });
    assert!(exceeded.is_err(), "{exceeded:?}");
    assert!(errors_mention(&mut vm, "call frame limit exceeded"));

    let recovered = eval(&mut vm, "frames_recover", "2");
    assert_eq!(recovered.as_number(), Some(2.0));
    assert!(take_errors(&mut vm).is_empty());
}

#[test]
fn call_frame_limit_includes_the_root_evaluation_frame() {
    let mut vm = test_vm();
    let root_only = vm.with_call_frame_limit(1, |vm| eval(vm, "root", "2"));
    assert_eq!(root_only.as_number(), Some(2.0));
    assert!(take_errors(&mut vm).is_empty());

    let exceeded =
        vm.with_call_frame_limit(1, |vm| eval(vm, "root_call", "fn one() {\n1\n}\ntry one() catch 99"));
    assert!(exceeded.is_err(), "{exceeded:?}");
    assert!(errors_mention(&mut vm, "call frame limit exceeded"));

    let zero = vm.with_call_frame_limit(0, |vm| eval(vm, "root_zero", "2"));
    assert!(zero.is_err(), "{zero:?}");
    assert!(errors_mention(&mut vm, "call frame limit exceeded"));
}

#[test]
fn nested_limits_only_narrow() {
    let mut vm = test_vm();
    let value = vm.with_call_frame_limit(2, |vm| {
        vm.with_call_frame_limit(100, |vm| {
            eval(vm, "nested", "fn one() {\nfn two() {\n1\n}\ntwo()\n}\ntry one() catch 99")
        })
    });
    assert!(value.is_err(), "{value:?}");
    assert!(errors_mention(&mut vm, "call frame limit exceeded"));
}

#[test]
fn clear_execution_limit_failures_discards_stale_signals() {
    let mut vm = test_vm();
    vm.with_stack_value_limit(1, |vm| {
        vm.thread_mut().push_stack_unchecked(1.into());
        vm.thread_mut().push_stack_unchecked(2.into());
        vm.thread_mut().pop_stack_value();
    });
    vm.clear_execution_limit_failures();
    let value = eval(&mut vm, "stale", "2");
    assert_eq!(value.as_number(), Some(2.0));
    assert!(take_errors(&mut vm).is_empty());
}

#[test]
fn clear_type_methods_removes_primitive_methods() {
    let mut vm = test_vm();
    let before = eval(&mut vm, "string_len_before", "\"abc\".len()");
    assert_eq!(before.as_number(), Some(3.0));
    assert!(take_errors(&mut vm).is_empty());

    let removed = vm
        .bx
        .code
        .native
        .borrow_mut()
        .clear_type_methods(ScriptValueType::REDUX_STRING);
    assert!(removed > 0);
    assert_eq!(
        vm.bx
            .code
            .native
            .borrow_mut()
            .clear_type_methods(ScriptValueType::REDUX_STRING),
        0
    );

    let after = eval(&mut vm, "string_len_after", "\"abc\".len()");
    assert!(after.is_err(), "{after:?}");
    let _ = take_errors(&mut vm);
    // Other types keep their surface.
    let number = eval(&mut vm, "number_ty", "(1).ty()");
    assert!(!number.is_err(), "{number:?}");
}
