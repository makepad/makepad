//! Regression tests for object map keys as seen from script: `delete(@key)`
//! removes the field and returns the removed value, `for k, v in obj` keys
//! compare equal to the `@name` id literal (also after a round trip through
//! an array), and a loop that ends a module's source runs at top level as it
//! does inside a `fn`.

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
            file: format!("object_keys_{name}.octoscript"),
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

#[test]
fn delete_id_literal_removes_the_field_and_returns_its_value() {
    let vm = &mut test_vm();
    assert_eq!(
        number(vm, "delete_ret", "let o = {a: 1 b: 2}\no.delete(@a)"),
        1.0
    );
    assert_eq!(
        number(vm, "delete_len", "let o = {a: 1 b: 2}\no.delete(@a)\no.map_len()"),
        1.0
    );
    assert!(boolean(
        vm,
        "delete_rest",
        "let o = {a: 1 b: 2}\no.delete(@a)\no.b == 2 && o.map_len() == 1"
    ));
    // Deleting a key held in a variable (as `for k in obj` hands it out).
    assert_eq!(
        number(
            vm,
            "delete_var",
            "let o = {a: 1 b: 2}\nlet k = @b\no.delete(k)\no.map_len()"
        ),
        1.0
    );
    // A missing key removes nothing and returns nil.
    assert!(boolean(
        vm,
        "delete_missing",
        "let o = {a: 1}\no.delete(@zz) == nil && o.map_len() == 1"
    ));
}

#[test]
fn delete_of_iteration_key_removes_the_field() {
    let vm = &mut test_vm();
    assert_eq!(
        number(
            vm,
            "delete_iter",
            "fn f(o){ let ks = []\n for k, v in o { ks.push(k) }\n for k in ks { o.delete(k) }\n o.map_len() }\nf({a: 1 b: 2 c: 3})"
        ),
        0.0
    );
}

#[test]
fn iteration_keys_equal_id_literals() {
    let vm = &mut test_vm();
    assert_eq!(
        number(
            vm,
            "iter_eq_fn",
            "fn f(o){ let n = 0\n for k, v in o { if k == @name { n = v } }\n n }\nf({other: 1 name: 7})"
        ),
        7.0
    );
    assert!(boolean(
        vm,
        "iter_ne_fn",
        "fn f(o){ let hit = false\n for k, v in o { if k != @name { hit = true } }\n hit }\nf({name: 7})"
    ) == false);
    assert!(boolean(
        vm,
        "iter_str",
        "fn f(o){ let same = false\n for k, v in o { same = k.to_string() == \"name\" }\n same }\nf({name: 7})"
    ));
}

/// Evaluates `code` as given, without the `\n;` terminator the
/// `script_mod!` path appends: a host such as FlowVm evaluates raw source,
/// so the source ends directly on its last statement.
fn eval_raw(vm: &mut ScriptVm, name: &str, code: &str) -> (ScriptValue, Vec<String>) {
    let value = vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            file: format!("object_keys_{name}.octoscript"),
            code: code.to_string(),
            ..Default::default()
        })
    });
    (value, vm.take_errors())
}

/// A loop that ends the source leaves no value: auto-close must not pop it
/// as the module's result ("pop_stack_resolved on empty stack").
#[test]
fn loop_as_last_statement_at_top_level() {
    let vm = &mut test_vm();
    for (name, code) in [
        ("for_map", "let o = {a: 1 b: 2}\nlet r = []\nfor k, v in o { r.push([k, v]) }"),
        ("for_map_nl", "let o = {a: 1 b: 2}\nlet r = []\nfor k, v in o { r.push([k, v]) }\n"),
        ("for_range", "let r = []\nfor i in 0..2 { r.push(i) }"),
        ("for_assign", "let r = 0\nfor i in 0..2 { r = i }"),
        ("while", "let r = []\nlet i = 0\nwhile i < 2 { r.push(i) i += 1 }"),
        ("loop", "let r = []\nloop { r.push(1) break }"),
    ] {
        let (value, errors) = eval_raw(vm, name, code);
        assert!(errors.is_empty(), "{name}: {errors:?}");
        assert!(value.is_nil(), "{name}: expected nil, got {value:?}");
    }
    // A value statement after the loop is still the module's result.
    let (value, errors) = eval_raw(
        vm,
        "for_then_value",
        "let o = {a: 1 b: 2}\nlet r = []\nfor k, v in o { r.push([k, v]) }\nr.len()",
    );
    assert!(errors.is_empty(), "for_then_value: {errors:?}");
    assert_eq!(value.as_number(), Some(2.0));
}

#[test]
fn map_for_loop_building_arrays_matches_fn_body() {
    let vm = &mut test_vm();
    assert_eq!(
        number(
            vm,
            "top_level_push",
            "let o = {a: 1 b: 2}\nlet r = []\nfor k, v in o { r.push([k, v]) }\nr.len()"
        ),
        2.0
    );
    assert_eq!(
        number(
            vm,
            "top_level_eq",
            "let o = {a: 1 name: 5}\nlet n = 0\nfor k, v in o { if k == @name { n = v } }\nn"
        ),
        5.0
    );
    assert_eq!(
        number(
            vm,
            "fn_push",
            "fn f(o){ let r = []\n for k, v in o { r.push([k, v]) }\n r.len() }\nf({a: 1 b: 2})"
        ),
        2.0
    );
}

#[test]
fn id_literal_index_reads_and_writes_the_plain_key() {
    let vm = &mut test_vm();
    assert_eq!(number(vm, "index_read", "let o = {a: 1 b: 2}\no[@b]"), 2.0);
    assert_eq!(
        number(vm, "index_write", "let o = {a: 1}\no[@a] = 5\no.a + o.map_len()"),
        6.0
    );
    // Keys pushed into an array keep their identity when read back.
    assert!(boolean(
        vm,
        "key_in_array",
        "let o = {a: 1 b: 2}\nlet ks = []\nfor k, v in o { ks.push(k) }\nks[1] == @b"
    ));
    assert_eq!(
        number(
            vm,
            "key_lookup",
            "let o = {a: 1 b: 2}\nlet n = 0\nfor k, v in o { n += o[k] }\nn"
        ),
        3.0
    );
}
