//! Regression: a callback minted inside a closure that was invoked with MORE
//! args than it declares must still be callable with any type.
//!
//! Empirically (host_launcher, 2026-08-14): a mini-app's `host.request`
//! callback created inside a `start_timeout(0.05, || { ... })` body died with
//! "arg 0 (nil) type mismatch: expected number, got object" when the host
//! answered. The timer invokes the zero-arg closure with one number (the
//! time), which lands in the frame as an EXTRA arg keyed NIL; a closure
//! created in that body inherits the frame as its prototype, so the leftover
//! number became a "declared default" and typechecked every later call.
//!
//! The `(nil)` in the message is the tell: a declared parameter has a real
//! key, so only a pushed extra arg can produce it.

use std::cell::RefCell;

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

std::thread_local! {
    static OUTER: RefCell<Option<ScriptFnRef>> = RefCell::new(None);
    static INNER: RefCell<Option<ScriptFnRef>> = RefCell::new(None);
}

#[test]
fn callback_minted_in_an_over_called_closure_survives() {
    let mut vm = test_vm();
    let m = vm.new_module(id!(hostx));
    // Stands in for start_timeout: keeps the closure to fire later.
    vm.add_method(m, id_lut!(later), script_args_def!(cb = NIL), |vm, args| {
        let cb = script_value!(vm, args.cb);
        let obj = cb.as_object().expect("closure object");
        let fnref = vm.bx.heap.new_fn_ref(obj);
        OUTER.with(|s| *s.borrow_mut() = Some(fnref));
        NIL
    });
    // Stands in for host.request: keeps the result callback.
    vm.add_method(
        m,
        id_lut!(request),
        script_args_def!(service = NIL, on_result = NIL),
        |vm, args| {
            let on_result = script_value!(vm, args.on_result);
            let obj = on_result.as_object().expect("callback object");
            let fnref = vm.bx.heap.new_fn_ref(obj);
            INNER.with(|s| *s.borrow_mut() = Some(fnref));
            NIL
        },
    );

    vm.bx.captured_errors = Some(Vec::new());
    let code = "\
let out = \"PENDING\"
fn done(r){
    out = \"GOT\"
}
let _t = mod.hostx.later(|| {
    let _r = mod.hostx.request(\"svc\", |r| done(r))
})";
    vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: "extra_arg_typecheck".to_string(),
            line: 0,
            column: 0,
            code: code.to_string(),
            values: vec![],
        })
    });

    // The timer fires the zero-arg closure WITH a time number, exactly like
    // handle_script_timer does. This is what poisons the frame.
    let outer = OUTER.with(|s| s.borrow_mut().take()).expect("outer closure");
    vm.with_instruction_limit(500_000, |vm| {
        vm.call(outer.as_object().into(), &[1.5.into()]);
    });

    // Now the host answers with an OBJECT, as every bridge response does.
    let inner = INNER.with(|s| s.borrow_mut().take()).expect("inner callback");
    let obj = vm.bx.heap.new_object();
    let trap = vm.bx.threads.cur().trap.pass();
    vm.bx.heap.set_value(obj, id!(is_ok).into(), true.into(), trap);
    vm.with_instruction_limit(500_000, |vm| {
        vm.call(inner.as_object().into(), &[obj.into()]);
    });

    let errors = vm.take_errors();
    assert!(errors.is_empty(), "callback errored: {errors:?}");

    let scope = {
        let bodies = vm.bx.code.bodies.borrow();
        bodies
            .iter()
            .find_map(|body| match &body.source {
                ScriptSource::Mod(m) if m.file == "extra_arg_typecheck" => {
                    Some(body.scope.as_object())
                }
                _ => None,
            })
            .expect("body scope")
    };
    let out = vm.bx.heap.scope_value(scope, id!(out), vm.trap());
    let mut got = String::new();
    vm.string_with(out, |_vm, s| got = s.to_string());
    assert_eq!(got, "GOT", "the callback never ran");
}
