//! Regression tests for canonical `try protected catch fallback`, the legacy
//! catch-less `try protected fallback [ok success]` form, cross-call
//! unwinding, loop back-edge cleanup of iteration-local try frames, and the
//! logical/comparison precedence and `!` contracts (grammar v0.2).

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
    vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            file: format!("try_catch_{name}.octoscript"),
            code: format!("{code}\n;"),
            ..Default::default()
        })
    })
}

fn number(vm: &mut ScriptVm, name: &str, code: &str) -> f64 {
    let value = eval(vm, name, code);
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{name}: {errors:?}");
    value
        .as_number()
        .unwrap_or_else(|| panic!("{name}: expected a number, got {value:?}"))
}

fn boolean(vm: &mut ScriptVm, name: &str, code: &str) -> bool {
    let value = eval(vm, name, code);
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{name}: {errors:?}");
    value
        .as_bool()
        .unwrap_or_else(|| panic!("{name}: expected a bool, got {value:?}"))
}

fn assertion_failed(vm: &mut ScriptVm, name: &str, code: &str) -> Vec<String> {
    let _ = eval(vm, name, code);
    let errors = vm.take_errors();
    assert!(
        errors.iter().any(|e| e.contains("assertion failed")),
        "{name}: expected an uncaught assertion, got {errors:?}"
    );
    errors
}

#[test]
fn legacy_catch_less_try_still_recovers_and_keeps_its_value() {
    let vm = &mut test_vm();
    assert_eq!(
        number(vm, "legacy_block", "use mod.std.assert\ntry {\n    assert(false)\n} {\n    42\n}"),
        42.0
    );
    // TRY_ERR's success path used to skip one extra opcode: with no `ok`
    // branch that was the enclosing LET, so `value` never bound.
    assert_eq!(
        number(
            vm,
            "legacy_let",
            "use mod.std.assert\nlet value = try { 7 } { assert(false) }\nvalue"
        ),
        7.0
    );
    assert_eq!(
        number(
            vm,
            "legacy_ok_after_success",
            "let marker = 0\ntry { 7 } { marker = 1 } ok { marker = 2 }\nmarker"
        ),
        2.0
    );
    assert_eq!(
        number(
            vm,
            "legacy_ok_after_error",
            "use mod.std.assert\nlet marker = 0\ntry { assert(false) } { marker = 1 } ok { marker = 2 }\nmarker"
        ),
        1.0
    );
}

#[test]
fn canonical_try_catch_returns_the_protected_or_fallback_value() {
    let vm = &mut test_vm();
    assert_eq!(
        number(
            vm,
            "block_fallback",
            "use mod.std.assert\ntry {\n    assert(false)\n} catch {\n    42\n}"
        ),
        42.0
    );
    // Block branches keep their final expression as the try value even when
    // canonical source terminates that expression with a newline.
    assert_eq!(
        number(
            vm,
            "block_protected",
            "use mod.std.assert\nlet value = try {\n    7\n} catch {\n    assert(false)\n}\nvalue"
        ),
        7.0
    );
    assert_eq!(
        number(
            vm,
            "block_fallback_let",
            "use mod.std.assert\nlet value = try {\n    assert(false)\n} catch {\n    42\n}\nvalue"
        ),
        42.0
    );
    assert_eq!(
        number(vm, "expr", "use mod.std.assert\ntry assert(false) catch 9"),
        9.0
    );
    assert_eq!(
        number(
            vm,
            "record",
            "let value = try ({answer: 42}) catch ({answer: 0})\nvalue.answer"
        ),
        42.0
    );
    // Both values participate safely in a larger expression.
    assert_eq!(
        number(vm, "in_expression", "use mod.std.assert\n1 + (try assert(false) catch 2) * 3"),
        7.0
    );
    assert_eq!(
        number(vm, "in_expression_ok", "use mod.std.assert\n1 + (try 5 catch assert(false)) * 3"),
        16.0
    );
}

#[test]
fn canonical_try_value_survives_end_of_source_auto_close() {
    let vm = &mut test_vm();
    // No trailing terminator: the parser's auto-close path must not discard
    // the fallback block's tail value either.
    let value = vm.eval(ScriptMod {
        file: "try_catch_auto_close.octoscript".to_owned(),
        code: "use mod.std.assert\ntry {\n    assert(false)\n} catch {\n    42\n}".to_owned(),
        ..Default::default()
    });
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(value.as_number(), Some(42.0));
}

#[test]
fn canonical_try_catch_does_not_swallow_a_fallback_error() {
    let vm = &mut test_vm();
    assertion_failed(
        vm,
        "fallback_error",
        "use mod.std.assert\ntry {\n    assert(false)\n} catch {\n    assert(false)\n}",
    );
}

#[test]
fn canonical_try_catch_unwinds_across_script_function_calls() {
    let vm = &mut test_vm();
    assert_eq!(
        number(
            vm,
            "unwind_outer",
            "use mod.std.assert\n\
             fn fail() {\n\
                 assert(false)\n\
             }\n\
             fn middle() {\n\
                 return fail()\n\
             }\n\
             let recovered = try {\n\
                 middle()\n\
                 0\n\
             } catch {\n\
                 42\n\
             }\n\
             recovered"
        ),
        42.0
    );
    assert_eq!(
        number(
            vm,
            "unwind_inner",
            "use mod.std.assert\n\
             fn fail() {\n\
                 assert(false)\n\
             }\n\
             fn recover() {\n\
                 return try {\n\
                     fail()\n\
                 } catch {\n\
                     7\n\
                 }\n\
             }\n\
             recover()"
        ),
        7.0
    );
    // The unwound frames are gone: a later call still works and the
    // try-owning frame's body is the one that continues.
    assert_eq!(
        number(
            vm,
            "unwind_then_call",
            "use mod.std.assert\n\
             fn fail() {\n\
                 assert(false)\n\
             }\n\
             fn twice(x) {\n\
                 return x * 2\n\
             }\n\
             let recovered = try {\n\
                 fail()\n\
             } catch {\n\
                 twice(21)\n\
             }\n\
             recovered + twice(0)"
        ),
        42.0
    );
}

#[test]
fn canonical_nested_try_catch_can_recover_from_an_inner_fallback_error() {
    let vm = &mut test_vm();
    assert_eq!(
        number(
            vm,
            "nested",
            "use mod.std.assert\n\
             try {\n\
                 try {\n\
                     assert(false)\n\
                 } catch {\n\
                     assert(false)\n\
                 }\n\
             } catch {\n\
                 42\n\
             }"
        ),
        42.0
    );
}

#[test]
fn canonical_catch_marker_is_a_one_shot_contextual_separator() {
    let vm = &mut test_vm();
    // The first `catch` is the separator, the second is the fallback identifier.
    assert_eq!(
        number(
            vm,
            "catch_ident",
            "use mod.std.assert\nlet catch = 2\ntry {\n    assert(false)\n} catch catch + 1"
        ),
        3.0
    );
    // A protected identifier named `catch` must be parenthesized.
    assert_eq!(
        number(vm, "catch_protected", "let catch = 2\ntry (catch) catch catch + 1"),
        2.0
    );
    // Outside `try`, `catch` is an ordinary identifier.
    assert_eq!(
        number(vm, "catch_plain", "let catch = 20\nfn f(catch) { return catch + 1 }\nf(catch) * 2"),
        42.0
    );
}

#[test]
fn loop_back_edges_discard_iteration_local_try_frames() {
    for (name, source) in [
        (
            "plain_loop_continue",
            "use mod.std.assert\n\
             let first = true\n\
             loop {\n\
                 if first {\n\
                     first = false\n\
                     try {\n\
                         continue\n\
                         nil\n\
                     } catch {\n\
                         state.caught = state.caught + 1\n\
                     }\n\
                 }\n\
                 assert(false)\n\
                 break\n\
             }\n\
             state.caught",
        ),
        (
            "while_continue",
            "use mod.std.assert\n\
             let index = 0\n\
             while index < 2 {\n\
                 index += 1\n\
                 if index == 1 {\n\
                     try {\n\
                         continue\n\
                         nil\n\
                     } catch {\n\
                         state.caught = state.caught + 1\n\
                     }\n\
                 }\n\
                 assert(false)\n\
                 break\n\
             }\n\
             state.caught",
        ),
        (
            "for_continue",
            "use mod.std.assert\n\
             for i in 2 {\n\
                 if i == 0 {\n\
                     try {\n\
                         continue\n\
                         nil\n\
                     } catch {\n\
                         state.caught = state.caught + 1\n\
                     }\n\
                 }\n\
                 assert(false)\n\
                 break\n\
             }\n\
             state.caught",
        ),
        (
            "loop_break",
            "use mod.std.assert\n\
             loop {\n\
                 try {\n\
                     break\n\
                     nil\n\
                 } catch {\n\
                     state.caught = state.caught + 1\n\
                 }\n\
             }\n\
             assert(false)\n\
             state.caught",
        ),
    ] {
        let vm = &mut test_vm();
        let state = vm.heap_mut().new_object();
        vm.heap_mut()
            .set_value_def(state, id!(caught).into(), ScriptValue::from_f64(0.0));
        vm.set_injected_global(id!(state), state.into());

        assertion_failed(vm, name, source);
        // The abandoned handler must never have re-entered its fallback.
        let caught = vm.heap().value(state, id!(caught).into(), NoTrap);
        assert_eq!(caught.as_number(), Some(0.0), "{name}: stale try frame caught a later error");
    }
    // The enclosing try frame outside the loop stays intact.
    let vm = &mut test_vm();
    assert_eq!(
        number(
            vm,
            "enclosing_try",
            "use mod.std.assert\n\
             try {\n\
                 let i = 0\n\
                 while i < 3 {\n\
                     i += 1\n\
                     if i < 3 { continue }\n\
                     assert(false)\n\
                 }\n\
                 0\n\
             } catch {\n\
                 42\n\
             }"
        ),
        42.0
    );
}

#[test]
fn logical_precedence_matches_boolean_algebra_and_skips_effects() {
    let vm = &mut test_vm();
    for a in [false, true] {
        for b in [false, true] {
            for c in [false, true] {
                assert_eq!(
                    boolean(vm, "or_and", &format!("{a} || {b} && {c}")),
                    a || b && c,
                    "{a} || {b} && {c}"
                );
                assert_eq!(
                    boolean(vm, "and_or", &format!("{a} && {b} || {c}")),
                    a && b || c,
                    "{a} && {b} || {c}"
                );
            }
        }
    }
    // `||` short-circuits its entire right-hand `&&` expression.
    assert_eq!(number(vm, "effect", "let n = 0\ntrue || false && (n = 1)\nn"), 0.0);
    assert_eq!(number(vm, "effect_and", "let n = 0\nfalse && (n = 1) || true\nn"), 0.0);
    // Comparisons bind tighter than equality.
    assert!(boolean(vm, "cmp_eq_1", "true == 2 > 1"));
    assert!(boolean(vm, "cmp_eq_2", "false != 2 > 1"));
    assert!(boolean(vm, "cmp_eq_3", "1 < 2 == true"));
    assert!(boolean(vm, "cmp_eq_4", "(1 < 2 == true) && (2 >= 3 != true)"));
}

#[test]
fn logical_not_always_negates_truth_conversion() {
    let vm = &mut test_vm();
    for (source, expected) in [
        ("!1", false),
        ("!(1 + 0)", false),
        ("!0", true),
        ("!(0 + 0)", true),
        ("!(-1)", false),
        ("!1.5", false),
        ("!true", false),
        ("!false", true),
        ("!nil", true),
        ("!!7", true),
    ] {
        assert_eq!(boolean(vm, "not", source), expected, "{source}");
    }
}
