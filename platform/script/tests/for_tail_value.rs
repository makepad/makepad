//! A fn whose last statement is a `for` statement returns nothing: a `for` statement is a loop,
//! not the list of its body's values (for_list.rs). Using such a call's result as a value is an
//! error that names the fn and its loop; a call whose result is discarded stays silent.

use makepad_script::*;

fn eval(code: &str) -> (ScriptValue, Vec<String>, ScriptVm<'static>) {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(500_000, |vm| vm.eval(ScriptMod::new("", "", "tail", 0, 0, &code, &[])));
    let mut errs: Vec<String> = vm.bx.captured_errors.take().unwrap().into_iter().map(|e| format!("{e:?}")).collect();
    errs.extend(vm.take_errors().into_iter().map(|e| format!("{e:?}")));
    (value, errs, vm)
}

/// Lines 0 and 1; the code under test starts on line 2.
const CLAWS: &str = "let tips = [1 2 3]\nfn claws(k) { for q in tips { q * k } }\n";

fn errors(code: &str) -> Vec<String> {
    eval(&format!("{CLAWS}{code}")).1
}

/// One error, at `line` (the use), naming `claws` and its loop on line 1.
fn reports(code: &str, line: u32) {
    let errs = errors(code);
    assert_eq!(errs.len(), 1, "{code}: {errs:?}");
    let e = &errs[0];
    assert!(e.starts_with(&format!("\"tail:{line}:")), "{code}: {e}");
    assert!(e.contains("`claws` returns nothing"), "{code}: {e}");
    assert!(e.contains("the `for` at tail:1"), "{code}: {e}");
}

#[test]
fn a_loop_tail_used_as_a_value_is_an_error() {
    // Assigned.
    reports("let c = claws(2)\nc", 2);
    // A call's argument, of a script fn and of a native.
    reports("fn f(a, b) { return b }\nf(0, claws(2))", 3);
    reports("let xs = []\nxs.push(claws(2))\nxs", 3);
    // A list element and an object field.
    reports("let xs = [0 claws(2)]\nxs", 2);
    reports("let o = {c: claws(2)}\no", 2);
    // A loop's source.
    reports("let n = 0\nfor c in claws(2) { n += 1 }\nn", 3);
    // In an expression.
    reports("let c = claws(2) == nil\nc", 2);
    // Handed on as another fn's result, then used.
    reports("fn g() { claws(2) }\nlet c = g()\nc", 3);
    reports("fn g() { return claws(2) }\nlet c = g()\nc", 3);
}

#[test]
fn a_loop_tail_whose_result_is_discarded_is_silent() {
    for code in [
        // Never called, and called as statements.
        "",
        "claws(2)\nlet z = 0\n",
        "claws(2)\n",
        "for k in 0..3 { claws(k) }\nlet z = 0\n",
        "fn g() { claws(2)\n 5 }\nlet z = g()\n",
        // Handed on as another fn's result, which is discarded.
        "fn g() { claws(2) }\ng()\nlet z = 0\n",
        // The tail of an if arm whose value is discarded.
        "fn g(c) { if c { claws(1) } else { claws(2) } }\ng(true)\ng(false)\nlet z = 0\n",
        "fn g(c) { if c { claws(1) } }\ng(true)\nlet z = 0\n",
        "fn g(c) { if c { claws(1) }\n 5 }\nlet z = g(true)\n",
        // A child of an object body: a nil adds nothing there, as from any fn that returns nothing.
        "let o = {claws(2)}\n",
    ] {
        let errs = errors(code);
        assert!(errs.is_empty(), "{code}: {errs:?}");
    }
}

#[test]
fn a_loop_that_is_a_value_returns_its_list() {
    for code in [
        "fn f() { return for i in 0..3 { i } }\nlet xs = f()\nxs",
        "fn f() { let xs = for i in 0..3 { i }\n xs }\nlet ys = f()\nys",
    ] {
        let (value, errs, vm) = eval(code);
        assert!(errs.is_empty(), "{code}: {errs:?}");
        let a = value.as_array().unwrap_or_else(|| panic!("{code}: not a list: {value:?}"));
        assert_eq!(vm.bx.heap_ref().array_len(a), 3, "{code}");
    }
    // A `while` or `loop` at the end is not a `for`: no list was ever expected of it.
    let (_, errs, _) = eval("fn f() { let i = 0\n while i < 3 { i += 1 } }\nlet x = f()\nx");
    assert!(errs.is_empty(), "{errs:?}");
}
