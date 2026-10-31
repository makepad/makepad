//! The syntax tree reads lists the way the VM does: the same number of elements.

use makepad_script::syntax::{self, Kind};
use makepad_script::*;

fn vm_len(code: &str) -> f64 {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.capture_errors();
    let source = format!("let a = 0\nlet f = fn(x) {{ return x }}\nlet v = {code}\nv.len()\n;");
    let value = vm.eval(ScriptMod::new("", "", "t", 0, 0, &source, &[]));
    assert!(vm.take_errors().is_empty(), "{code}");
    value.as_number().unwrap_or(-1.0)
}

fn tree_len(code: &str) -> usize {
    let tree = syntax::parse(code).unwrap();
    let Kind::Array { items, .. } = &tree.statements()[0].kind else { panic!("{code}: not an array") };
    items.len()
}

#[test]
fn list_elements_are_counted_alike() {
    for code in [
        "[1 -2]",
        "[1, -2]",
        "[1 - 2, 3]",
        "[-1 -2]",
        "[[1 2] [3 4]]",
        "[a [1]]",
        "[1\n-2\n]",
        "[1 +2]",
        "[1 2 3]",
        "[f(1) f(2)]",
        "[1, 2,\n 3]",
        "[{a: 1} {a: 2}]",
        "[\"a\" \"b\"]",
        "[a.b a]",
    ] {
        let tree = tree_len(code);
        // (`a.b` on a number is not evaluable: only the shapes are compared for it.)
        if code.contains("a.b") {
            assert_eq!(tree, 2, "{code}");
            continue;
        }
        assert_eq!(tree as f64, vm_len(code), "{code}");
    }
}
