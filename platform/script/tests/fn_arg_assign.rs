//! Assigning to a fn parameter in the fn's body rebinds it for the rest of the call, as for a
//! `let`: also from inside an `if` arm or a loop, and also for a parameter with a default.

use makepad_script::*;

fn eval(code: &str) -> (ScriptValue, Vec<String>) {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    vm.bx.captured_errors = Some(Vec::new());
    let value = vm.with_instruction_limit(500_000, |vm| vm.eval(ScriptMod::new("", "", "fn_arg_assign", 0, 0, &code, &[])));
    let mut errs: Vec<String> = vm.bx.captured_errors.take().unwrap().into_iter().map(|e| format!("{e:?}")).collect();
    errs.extend(vm.take_errors().into_iter().map(|e| format!("{e:?}")));
    (value, errs)
}

#[test]
fn a_parameter_takes_an_assignment() {
    for (code, want) in [
        ("fn f(x) { x = x + 1\n x }\nf(1)", 2.0),
        ("fn f(x) { if x > 0 { x = 5 }\n x }\nf(1)", 5.0),
        ("fn f(x) { for i in 0..3 { x = x + i }\n x }\nf(1)", 4.0),
        ("fn f(x = 7) { x = x * 2\n x }\nf()", 14.0),
        ("fn f(x, o = nil) { if o == nil { o = {k: 3} }\n o.k + x }\nf(1)", 4.0),
        ("let f = fn(x, o = nil) { if o == nil { o = {k: 3} }\n o.k + x }\nf(1)", 4.0),
        ("let f = fn(x, o = nil) { if o == nil { o = {k: 3} }\n o.k + x }\nf(1, {k: 10})", 11.0),
        // Compound assignment, and a closure writing its defining call's parameter.
        ("fn f(x = 7) { x += 1\n x }\nf()", 8.0),
        ("fn f(x = 1) { let g = || { x = 5 }\n g()\n x }\nf()", 5.0),
        // The default stays the default for the next call.
        ("fn f(x = 7) { x = x * 2\n x }\nf()\nf()", 14.0),
    ] {
        let (value, errs) = eval(code);
        assert!(errs.is_empty(), "{code}: {errs:?}");
        assert_eq!(value.as_number(), Some(want), "{code}: the assigned parameter is read back");
    }
}
