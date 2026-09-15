//! Mirrors the splash_host bridge: a native fn stores a script closure as a
//! ScriptFnRef; the host later builds a result object and calls it.

use std::cell::RefCell;
use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host,
        bx: Box::new(ScriptVmBase::new()),
    }
}

std::thread_local! {
    static STORED: RefCell<Option<ScriptFnRef>> = RefCell::new(None);
}

#[test]
fn stored_fn_ref_calls_back_with_built_object() {
    let mut vm = test_vm();
    let m = vm.new_module(id!(hostx));
    vm.add_method(
        m,
        id_lut!(request),
        script_args_def!(service = NIL, args = NIL, on_result = NIL),
        |vm, args| {
            let on_result = script_value!(vm, args.on_result);
            let Some(obj) = on_result.as_object() else {
                panic!("callback is not an object");
            };
            assert!(vm.bx.heap.is_fn(obj), "callback object is not fn-tagged");
            let fnref = vm.bx.heap.new_fn_ref(obj);
            STORED.with(|s| *s.borrow_mut() = Some(fnref));
            1.0.into()
        },
    );

    vm.bx.captured_errors = Some(Vec::new());
    vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: "fn_ref_callback".to_string(),
            line: 0,
            column: 0,
            // `is_ok`, not `ok`: `ok` is the ok-test keyword and `r.ok` does
            // not parse as field access — the exact bug that motivated the
            // bridge's result-field name.
            code: "let got = {value: -1}\nlet _rid = mod.hostx.request(\"svc\", {}, fn(r) { got.value = r.is_ok })\n(got)"
                .to_string(),
            values: vec![],
        })
    });

    // The host answers later: build {ok: true, ...} and invoke the callback,
    // exactly as splash_host_respond does.
    let callback = STORED.with(|s| s.borrow_mut().take()).expect("stored");
    let obj = vm.bx.heap.new_object();
    let trap = vm.bx.threads.cur().trap.pass();
    vm.bx.heap.set_value(obj, id!(is_ok).into(), true.into(), trap);
    vm.bx.heap.set_value(obj, id!(data).into(), NIL, trap);
    vm.with_instruction_limit(500_000, |vm| {
        vm.call(callback.as_object().into(), &[obj.into()]);
    });

    let errors = vm.take_errors();
    assert!(errors.is_empty(), "callback errored: {errors:?}");

    // The closure wrote r.ok into module state.
    let scope = {
        let bodies = vm.bx.code.bodies.borrow();
        bodies
            .iter()
            .find_map(|body| match &body.source {
                ScriptSource::Mod(m) if m.file == "fn_ref_callback" => {
                    Some(body.scope.as_object())
                }
                _ => None,
            })
            .expect("body scope")
    };
    let got = vm.bx.heap.scope_value(scope, id!(got), vm.trap());
    let got_obj = got.as_object().expect("got object");
    let value = vm.bx.heap.value(got_obj, id!(value).into(), vm.trap());
    assert_eq!(value.as_bool(), Some(true), "callback did not run: {value:?}");
}

/// The isolation_probe shape: the request is made inside a closure, the
/// callback is a `fn(r)` expression argument, and the result lands in a
/// module scalar through a named fn.
#[test]
fn nested_fn_arg_callback_mutates_module_scalar() {
    let mut vm = test_vm();
    let m = vm.new_module(id!(hostx));
    vm.add_method(
        m,
        id_lut!(request),
        script_args_def!(service = NIL, args = NIL, on_result = NIL),
        |vm, args| {
            let on_result = script_value!(vm, args.on_result);
            let obj = on_result.as_object().expect("callback object");
            assert!(vm.bx.heap.is_fn(obj), "callback object is not fn-tagged");
            let fnref = vm.bx.heap.new_fn_ref(obj);
            STORED.with(|s| *s.borrow_mut() = Some(fnref));
            1.0.into()
        },
    );
    vm.bx.captured_errors = Some(Vec::new());
    let code = "\
let svc_text = \"PENDING\"
fn svc_result(r){
    if r.is_ok {
        svc_text = \"ALLOWED\"
        return nil
    }
    svc_text = \"DENIED\"
}
let run = || {
    let _rid = mod.hostx.request(\"svc\", {}, fn(r) { svc_result(r) })
}
run()";
    vm.with_instruction_limit(500_000, |vm| {
        vm.eval(ScriptMod {
            cargo_manifest_path: String::new(),
            module_path: String::new(),
            file: "nested_fn_cb".to_string(),
            line: 0,
            column: 0,
            code: code.to_string(),
            values: vec![],
        })
    });
    let callback = STORED.with(|s| s.borrow_mut().take()).expect("stored");
    let obj = vm.bx.heap.new_object();
    let trap = vm.bx.threads.cur().trap.pass();
    vm.bx.heap.set_value(obj, id!(is_ok).into(), false.into(), trap);
    vm.with_instruction_limit(500_000, |vm| {
        vm.call(callback.as_object().into(), &[obj.into()]);
    });
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "callback errored: {errors:?}");
    let scope = {
        let bodies = vm.bx.code.bodies.borrow();
        bodies
            .iter()
            .find_map(|body| match &body.source {
                ScriptSource::Mod(m) if m.file == "nested_fn_cb" => Some(body.scope.as_object()),
                _ => None,
            })
            .expect("body scope")
    };
    let text = vm.bx.heap.scope_value(scope, id!(svc_text), vm.trap());
    let mut out = String::new();
    vm.string_with(text, |_vm, s| out = s.to_string());
    assert_eq!(out, "DENIED", "callback chain did not update the module var");
}
