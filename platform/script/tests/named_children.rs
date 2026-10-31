//! Named children as slots: `n +: {..}` merges into what `n` holds, `n := v` replaces the named
//! child in place (or appends it), and a dotted left side is the nested-block form
//! (`a.b.c +: {..}` is `a +: { b +: { c +: {..} } }`, `a.b.c := v` is `a +: { b +: { c := v } }`).
//! The template never changes when an instance overrides a child.

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
    let v = vm.eval(ScriptMod::new("", "", "named_children", 0, 0, code, &[]));
    (v, vm.take_errors())
}

fn eval_ok(vm: &mut ScriptVm, code: &str) -> ScriptObject {
    let (v, errors) = eval(vm, code);
    assert!(errors.is_empty(), "{errors:?}");
    v.as_object().expect("the document is an object")
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

fn child(vm: &ScriptVm, obj: ScriptObject, name: &str) -> ScriptObject {
    let heap = &vm.bx.heap;
    let key: ScriptValue = LiveId::from_str(name).into();
    (0..heap.vec_len(obj))
        .map(|i| heap.vec_key_value(obj, i, NoTrap))
        .find(|kv| kv.key == key)
        .and_then(|kv| kv.value.as_object())
        .unwrap_or_else(|| panic!("no child {name}"))
}

fn num(vm: &ScriptVm, obj: ScriptObject, name: &str) -> f64 {
    vm.bx.heap.value(obj, LiveId::from_str(name).into(), NoTrap).as_number().unwrap_or(f64::NAN)
}

#[test]
fn plus_colon_merges_into_the_named_child() {
    let vm = &mut test_vm();
    let o = eval_ok(vm, "let T = { a := {v: 1 w: 2} b := {v: 3} }\nT{ a +: {v: 5} }");
    assert_eq!(kids(vm, o), "a=5,b=3");
    assert_eq!(num(vm, child(vm, o, "a"), "w"), 2.0);
}

#[test]
fn colon_equals_replaces_in_place_or_appends() {
    let vm = &mut test_vm();
    let o = eval_ok(vm, "let T = { a := {v: 1} b := {v: 2} c := {v: 3} }\nT{ b := {v: 9} }");
    assert_eq!(kids(vm, o), "a=1,b=9,c=3");
    let o = eval_ok(vm, "let T = { a := {v: 1} b := {v: 2} }\nT{ d := {v: 4} }");
    assert_eq!(kids(vm, o), "a=1,b=2,d=4");
    // a replaced child is the new value, not a merge
    let o = eval_ok(vm, "let T = { a := {v: 1 w: 2} }\nT{ a := {v: 7} }");
    assert!(vm.bx.heap.value(child(vm, o, "a"), LiveId::from_str("w").into(), NoTrap).is_err());
}

#[test]
fn plus_colon_adds_children_to_a_slot() {
    let vm = &mut test_vm();
    let o = eval_ok(vm, "let T = { slot := { a := {v: 1} } }\nT{ slot +: { b := {v: 2} } }");
    assert_eq!(kids(vm, child(vm, o, "slot")), "a=1,b=2");
}

#[test]
fn dotted_plus_colon_two_and_three_levels() {
    let vm = &mut test_vm();
    let t = "let T = { body := { slot := {v: 1 w: 2} } }\n";
    let o = eval_ok(vm, &format!("{t}T{{ body.slot +: {{v: 7}} }}"));
    let slot = child(vm, child(vm, o, "body"), "slot");
    assert_eq!((num(vm, slot, "v"), num(vm, slot, "w")), (7.0, 2.0));
    // the same, as the nested blocks
    let o2 = eval_ok(vm, &format!("{t}T{{ body +: {{ slot +: {{v: 7}} }} }}"));
    let slot2 = child(vm, child(vm, o2, "body"), "slot");
    assert_eq!((num(vm, slot2, "v"), num(vm, slot2, "w")), (7.0, 2.0));

    let t3 = "let T = { x := { y := { z := {v: 1 w: 2} } } }\n";
    let o = eval_ok(vm, &format!("{t3}T{{ x.y.z +: {{v: 4}} }}"));
    let z = child(vm, child(vm, child(vm, o, "x"), "y"), "z");
    assert_eq!((num(vm, z, "v"), num(vm, z, "w")), (4.0, 2.0));
    // adding a child through the path
    let o = eval_ok(vm, &format!("{t3}T{{ x.y.z +: {{ k := {{v: 5}} }} }}"));
    let z = child(vm, child(vm, child(vm, o, "x"), "y"), "z");
    assert_eq!(kids(vm, z), "k=5");
    assert_eq!(num(vm, z, "w"), 2.0);
}

#[test]
fn dotted_colon_equals_replaces_or_appends_two_and_three_levels() {
    let vm = &mut test_vm();
    let t = "let T = { body := { s1 := {v: 1} s2 := {v: 2} } }\n";
    let o = eval_ok(vm, &format!("{t}T{{ body.s1 := {{v: 9}} }}"));
    assert_eq!(kids(vm, child(vm, o, "body")), "s1=9,s2=2");
    let o = eval_ok(vm, &format!("{t}T{{ body.s3 := {{v: 3}} }}"));
    assert_eq!(kids(vm, child(vm, o, "body")), "s1=1,s2=2,s3=3");

    let t3 = "let T = { x := { y := { a := {v: 1} b := {v: 2} } } }\n";
    let o = eval_ok(vm, &format!("{t3}T{{ x.y.b := {{v: 8}} }}"));
    assert_eq!(kids(vm, child(vm, child(vm, o, "x"), "y")), "a=1,b=8");
    let o = eval_ok(vm, &format!("{t3}T{{ x.y.c := {{v: 6}} }}"));
    assert_eq!(kids(vm, child(vm, child(vm, o, "x"), "y")), "a=1,b=2,c=6");
}

#[test]
fn a_property_through_a_path_still_sets() {
    let vm = &mut test_vm();
    let o = eval_ok(vm, "let T = { body := { slot := {v: 1} } }\nT{ body.slot.v: 3 }");
    assert_eq!(num(vm, child(vm, child(vm, o, "body"), "slot"), "v"), 3.0);
}

#[test]
fn the_template_is_unchanged_and_instances_are_independent() {
    let vm = &mut test_vm();
    let o = eval_ok(
        vm,
        "let T = { body := { slot := { a := {v: 1} } } }
{
    t: T
    p1: T{ body.slot +: { b := {v: 2} } }
    p2: T{ body.slot +: { c := {v: 3} } body.slot.a := {v: 10} }
}",
    );
    let get = |name: &str| vm.bx.heap.value(o, LiveId::from_str(name).into(), NoTrap).as_object().unwrap();
    let slot = |obj| child(vm, child(vm, obj, "body"), "slot");
    assert_eq!(kids(vm, slot(get("t"))), "a=1");
    assert_eq!(kids(vm, slot(get("p1"))), "a=1,b=2");
    assert_eq!(kids(vm, slot(get("p2"))), "a=10,c=3");
}

#[test]
fn a_name_that_is_not_a_direct_child_is_an_error_naming_where_it_is() {
    let vm = &mut test_vm();
    let (_, errors) = eval(
        vm,
        "let Shell = { title := {v: 1} split := { a := { left := {v: 2} } } }\nShell{ left +: {v: 3} }",
    );
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(
        errors[0].contains("no property or named child `left` on object (named children: title, split); `left` is at split.a.left"),
        "{errors:?}"
    );
    // through a path the failing segment is named the same way
    let (_, errors) = eval(
        vm,
        "let Shell = { title := {v: 1} split := { a := { left := {v: 2} } } }\nShell{ split.left.v: 3 }",
    );
    assert!(errors.iter().any(|e| e.contains("no property or named child `left` on object (named children: a)")), "{errors:?}");
    // a name found nowhere has no location
    let (_, errors) = eval(vm, "let Shell = { title := {v: 1} }\nShell{ nope +: {v: 3} }");
    assert!(
        errors.iter().any(|e| e.contains("no property or named child `nope` on object (named children: title)") && !e.contains("is at")),
        "{errors:?}"
    );
}
