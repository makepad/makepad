//! Source evaluated at runtime with `eval_transient`: its body is freed once
//! nothing made from it is alive, and its slot reused; and a fn's content key
//! (what the draw shader cache keys by) follows its text, not its body slot.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host, bx: Box::new(ScriptVmBase::new()) }
}

fn module(file: &str, code: &str) -> ScriptMod {
    ScriptMod { file: file.to_string(), code: code.to_string(), ..Default::default() }
}

fn body_count(vm: &ScriptVm) -> usize {
    vm.bx.code.bodies.borrow().iter().filter(|b| !matches!(b.source, ScriptSource::Free)).count()
}

fn fn_ip(vm: &ScriptVm, obj: ScriptObject, name: LiveId) -> ScriptIp {
    let f = vm.bx.heap.value(obj, name.into(), NoTrap).as_object().expect("a fn");
    match vm.bx.heap.as_fn(f) {
        Some(ScriptFnPtr::Script(ip)) => ip,
        _ => panic!("not a script fn"),
    }
}

#[test]
fn unreferenced_transient_bodies_are_freed_and_reused() {
    let mut vm = test_vm();
    vm.gc();
    let before = body_count(&vm);
    let len_before = vm.bx.code.bodies.borrow().len();
    for k in 0..50 {
        let v = vm.eval_transient(module(&format!("gen://{k}"), &format!("{{a: {k} f: fn() {{ return {k} }}}}")));
        assert!(v.as_object().is_some());
        vm.gc();
    }
    // Nothing kept any of them: every one was freed and its slot reused
    // (the last stays while the thread's ip points into it).
    assert!(body_count(&vm) <= before + 1);
    assert!(vm.bx.code.bodies.borrow().len() <= len_before + 2, "slots are reused");
}

#[test]
fn a_live_fn_keeps_its_body() {
    let mut vm = test_vm();
    let v = vm.eval_transient(module("gen://kept", "{f: fn() { return 7 }}"));
    let obj = v.as_object().unwrap();
    let kept = vm.bx.heap.new_object_ref(obj);
    let ip = fn_ip(&vm, obj, id!(f));
    for k in 0..20 {
        vm.eval_transient(module(&format!("gen://other/{k}"), "{g: fn() { return 1 }}"));
        vm.gc();
    }
    let bodies = vm.bx.code.bodies.borrow();
    let body = &bodies[ip.body as usize];
    assert!(matches!(&body.source, ScriptSource::Mod(m) if m.file == "gen://kept"), "the kept fn's body is still its own");
    drop(bodies);
    drop(kept);
    vm.gc();
    assert!(matches!(vm.bx.code.bodies.borrow()[ip.body as usize].source, ScriptSource::Free));
}

#[test]
fn a_kept_closure_keeps_its_body_and_scope() {
    let mut vm = test_vm();
    // The closure reads `k` from the transient body's scope: holding the fn
    // keeps the body, and the body keeps that scope.
    let f = vm.eval_transient(module("gen://closure", "let k = 41\nfn() { return k + 1 }")).as_object().unwrap();
    let f = vm.bx.heap.new_fn_ref(f);
    for k in 0..20 {
        vm.eval_transient(module(&format!("gen://churn/{k}"), "{g: fn() { return 1 } o: {x: [1 2 3]}}"));
        vm.gc();
    }
    let r = vm.call(f.as_object().into(), &[]);
    assert_eq!(r.as_f64(), Some(42.0));
}

#[test]
fn fn_content_key_follows_the_text() {
    let mut vm = test_vm();
    let a = vm.eval(module("gen://same", "{f: fn() { return 1 }}")).as_object().unwrap();
    let _keep_a = vm.bx.heap.new_object_ref(a);
    let key_a = vm.bx.code.fn_content_key(fn_ip(&vm, a, id!(f)));
    // The same text in another body: the same key.
    let b = vm.eval(module("gen://elsewhere", "{f: fn() { return 1 }}")).as_object().unwrap();
    assert_eq!(vm.bx.code.fn_content_key(fn_ip(&vm, b, id!(f))), key_a);
    // The first body re-evaluated with other text keeps its ips but not its key.
    let c = vm.eval(module("gen://same", "{f: fn() { return 2 }}")).as_object().unwrap();
    let ip_c = fn_ip(&vm, c, id!(f));
    assert_eq!(ip_c, fn_ip(&vm, a, id!(f)));
    assert_ne!(vm.bx.code.fn_content_key(ip_c), key_a);
}
