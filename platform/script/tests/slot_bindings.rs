//! `var` bindings and lets in while/loop bodies live in frame slots inside a
//! fn body; root code keeps every binding in scope objects. The same code
//! must give the same result both ways.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host, bx: Box::new(ScriptVmBase::new()) }
}

fn run(name: &str, code: &str) -> (ScriptValue, Vec<String>) {
    let mut vm = test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(1_000_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: format!("slot_bindings_{name}"),
            line: 0,
            column: 0,
            code: code.to_string(),
            values: vec![],
        })
    });
    let mut errs: Vec<String> = vm.bx.captured_errors.take().unwrap().into_iter().map(|e| format!("{e:?}")).collect();
    errs.extend(vm.take_errors().into_iter().map(|e| format!("{e:?}")));
    (value, errs)
}

/// `body` ends in an expression: run it as root code (dynamic scopes) and
/// as a fn body (slots) and compare.
fn same_both_ways(name: &str, body: &str) -> f64 {
    let (root, root_errs) = run(name, body);
    let (fnv, fn_errs) = run(name, &format!("let f = fn() {{\n{body}\n}}\nf()"));
    assert!(root_errs.is_empty(), "{name}: root errors {root_errs:?}");
    assert!(fn_errs.is_empty(), "{name}: fn errors {fn_errs:?}");
    let (a, b) = (root.as_number(), fnv.as_number());
    assert_eq!(a, b, "{name}: root {root:?} fn {fnv:?}");
    a.unwrap_or_else(|| panic!("{name}: not a number: {root:?}"))
}

#[test]
fn var_bindings_read_and_assign_like_scope_bindings() {
    assert_eq!(same_both_ways("var", "var x = 2\nx = x * 3\nx += 1\nx -= 2\nx *= 4\nx /= 2\nx %= 7\nx"), 3.0);
    assert_eq!(same_both_ways("let_mut", "let mut x = 5\nx = x + 1\nx"), 6.0);
    assert_eq!(same_both_ways("var_in_for", "var s = 0\nfor i in 0..4 { var t = i * 2\nt += 1\ns = s + t }\ns"), 16.0);
    // a var shadowing an earlier binding of the same name stays dynamic
    assert_eq!(same_both_ways("var_shadow", "let x = 1\nvar x = x + 10\nx = x + 1\nx"), 12.0);
    // a var that is also a closure's free variable keeps the whole body dynamic
    assert_eq!(same_both_ways("var_closure", "var x = 1\nlet g = || x + 1\nx = 5\ng()"), 6.0);
    assert_eq!(same_both_ways("var_string", "var s = \"a\"\ns += \"b\"\ns = s + \"c\"\ns.len()"), 3.0);
}

#[test]
fn lets_in_while_bodies_see_the_previous_pass_like_the_loop_scope() {
    // a let read after its binding in the same pass
    assert_eq!(same_both_ways("while_let", "var s = 0\nvar i = 0\nwhile i < 5 { let k = i * 2\ns = s + k\ni = i + 1 }\ns"), 20.0);
    // read before its let: the previous pass's binding in the loop scope
    assert_eq!(same_both_ways("while_read_before", "var out = 0\nvar i = 0\nwhile i < 3 { if i > 0 { out = out + k }\nlet k = i * 10\ni = i + 1 }\nout"), 0.0);
    // assigned before its let
    assert_eq!(same_both_ways("while_assign_before", "var out = 0\nvar i = 0\nwhile i < 3 { if i > 0 { k = k + 1\nout = out + k }\nlet k = i * 10\ni = i + 1 }\nout"), 3.0);
    // read by the loop condition
    assert_eq!(same_both_ways("while_cond", "let k = 0\nvar n = 0\nwhile k < 3 { let k = 5\nn = n + 1\nif n > 4 { break } }\nn"), 1.0);
    // read in a nested for before the let
    assert_eq!(same_both_ways("while_nested_for", "var out = 0\nvar i = 0\nwhile i < 3 { for j in 0..2 { if i > 0 { out = out + k } }\nlet k = i + 1\ni = i + 1 }\nout"), 4.0);
    // `loop` and a var in the body
    assert_eq!(same_both_ways("loop_var", "var s = 0\nvar i = 0\nloop { var d = i * 3\nd += 1\ns = s + d\ni = i + 1\nif i >= 4 { break } }\ns"), 22.0);
    // a let inside the loop is not visible after it
    assert_eq!(same_both_ways("while_after", "let k = 7\nvar i = 0\nwhile i < 2 { let k = 1\ni = i + k }\nk"), 7.0);
}
