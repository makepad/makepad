use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn eval(vm: &mut ScriptVm, code: &str) -> (ScriptValue, Vec<String>) {
    vm.capture_errors();
    let v = vm.eval(ScriptMod::new("", "", "keyword_fields", 0, 0, code, &[]));
    (v, vm.take_errors())
}

fn eval_ok(vm: &mut ScriptVm, code: &str, property: &str) -> ScriptObject {
    let (v, errors) = eval(vm, code);
    assert!(errors.is_empty(), "{property}: evaluates without errors: {errors:?}");
    v.as_object().unwrap_or_else(|| panic!("{property}: evaluates to an object"))
}

/// The vec children of `obj` as `name=v` (the child's `v` field, or `-`), in order.
fn kids(vm: &ScriptVm, obj: ScriptObject) -> String {
    let heap = &vm.bx.heap;
    let mut out = Vec::new();
    for i in 0..heap.vec_len(obj) {
        let kv = heap.vec_key_value(obj, i, NoTrap);
        let name = kv.key.as_id().and_then(|id| id.as_string(|s| s.map(|s| s.to_string()))).unwrap_or_else(|| "_".into());
        let v = kv
            .value
            .as_object()
            .map(|c| heap.value(c, LiveId::from_str("v").into(), NoTrap))
            .and_then(|v| v.as_number())
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".into());
        out.push(format!("{name}={v}"));
    }
    out.join(",")
}

#[test]
fn keywords_are_field_names_before_field_operators() {
    let vm = &mut test_vm();

    let o = eval_ok(vm, "{scope := {v: 1}}", "scope named child");
    assert_eq!(kids(vm, o), "scope=1", "scope := creates a named vec child with v = 1");

    let o = eval_ok(
        vm,
        "{a := {v: 1} scope := {v: 2} b := {v: 3}}",
        "scope between siblings",
    );
    assert_eq!(kids(vm, o), "a=1,scope=2,b=3", "scope := keeps every sibling in order");

    // The template and instance override form from named_children.rs.
    let o = eval_ok(
        vm,
        "let T = { scope := {v: 1 w: 2} b := {v: 3} }\nT{ scope +: {v: 5} }",
        "scope named child merge",
    );
    assert_eq!(kids(vm, o), "scope=5,b=3", "scope +: overrides the named child's v in place");
    let scope = vm.bx.heap.vec_key_value(o, 0, NoTrap).value.as_object().expect("merged scope child is an object");
    assert_eq!(
        vm.bx.heap.value(scope, LiveId::from_str("w").into(), NoTrap).as_number(),
        Some(2.0),
        "scope +: preserves the child's inherited properties",
    );

    let o = eval_ok(vm, "{me := {v: 1} loop := {v: 1}}", "me and loop named children");
    assert_eq!(kids(vm, o), "me=1,loop=1", "me := and loop := create named vec children");

    let o = eval_ok(vm, "{scope: 4}", "scope colon property");
    assert_eq!(
        vm.bx.heap.value(o, LiveId::from_str("scope").into(), NoTrap).as_number(),
        Some(4.0),
        "scope: keeps the existing keyword property behavior",
    );

    // Copy the scope-returning function from loop_heap_steady.rs.
    let (value, errors) = eval(
        vm,
        "let sc = fn() { for i in 0..2 { let q = 5 + i return scope } }\nlet s = sc()\ns.q + s.i",
    );
    assert!(errors.is_empty(), "scope expression evaluates without errors: {errors:?}");
    assert_eq!(value.as_number(), Some(5.0), "scope expression still exposes the loop's q and i");

    let o = eval_ok(vm, "{v: 7 let f = me; result: f.v}", "me expression");
    assert_eq!(
        vm.bx.heap.value(o, LiveId::from_str("result").into(), NoTrap).as_number(),
        Some(7.0),
        "me expression still reads the current object",
    );
}
