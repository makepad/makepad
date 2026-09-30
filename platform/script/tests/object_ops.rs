//! Arithmetic on host objects (`ScriptVmBase::object_op_hook`): the VM has
//! none on objects (such a sum is NaN); a host can give `a op b` a meaning
//! for its own objects. Without a hook nothing changes, and a hook never
//! sees numbers, vectors or strings.

use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host, bx: Box::new(ScriptVmBase::new()) }
}

const DOC: &str = "
let o = {k: 1}
{
    add: o + 2
    radd: 2 + o
    sub: o - 2
    mul: o * 2
    div: o / 2
    num: 1 + 2
    vec: vec3(1, 2, 3) + vec3(1, 1, 1)
    text: \"a\" + 1
    plain: {k: 2} * 3
}
";

fn eval(vm: &mut ScriptVm) -> ScriptObject {
    let v = vm.eval(ScriptMod { file: "object_ops".into(), code: DOC.into(), ..Default::default() });
    v.as_object().expect("the document is an object")
}

fn field(vm: &ScriptVm, o: ScriptObject, name: &str) -> ScriptValue {
    vm.bx.heap.value(o, LiveId::from_str(name).into(), NoTrap)
}

/// The test host: an object with `k` combines to `k * 1000 + op` (1 add,
/// 2 sub, 3 mul, 4 div) plus the other operand; objects with `k == 2` are
/// declined (the VM's own result stands).
fn hook(heap: &mut ScriptHeap, op: ScriptBinOp, a: ScriptValue, b: ScriptValue, _ip: ScriptIp) -> Option<ScriptValue> {
    let (obj, other) = if a.as_object().is_some() { (a, b) } else { (b, a) };
    let k = heap.value(obj.as_object()?, LiveId::from_str("k").into(), NoTrap).as_number()?;
    if k == 2.0 {
        return None;
    }
    let code = match op {
        ScriptBinOp::Add => 1.0,
        ScriptBinOp::Sub => 2.0,
        ScriptBinOp::Mul => 3.0,
        ScriptBinOp::Div => 4.0,
    };
    Some((k * 1000.0 + code + other.as_number().unwrap_or(0.0)).into())
}

#[test]
fn without_a_hook_object_arithmetic_stays_nan() {
    let vm = &mut test_vm();
    let o = eval(vm);
    for name in ["add", "radd", "sub", "mul", "div", "plain"] {
        assert!(field(vm, o, name).as_number().is_some_and(f64::is_nan), "{name}");
    }
    assert_eq!(field(vm, o, "num").as_number(), Some(3.0));
}

#[test]
fn a_hook_gives_object_arithmetic_a_meaning_and_nothing_else() {
    let vm = &mut test_vm();
    vm.bx.object_op_hook = Some(hook);
    let o = eval(vm);
    let n = |name: &str| field(vm, o, name).as_number();
    assert_eq!(n("add"), Some(1003.0));
    assert_eq!(n("radd"), Some(1003.0));
    assert_eq!(n("sub"), Some(1004.0));
    assert_eq!(n("mul"), Some(1005.0));
    assert_eq!(n("div"), Some(1006.0));
    // Declined: the VM's own NaN.
    assert!(n("plain").is_some_and(f64::is_nan));
    // Numbers, vectors and strings never reach the hook.
    assert_eq!(n("num"), Some(3.0));
    let mut plain = test_vm();
    let p = eval(&mut plain);
    for name in ["vec", "text"] {
        let (a, b) = (field(vm, o, name), field(&plain, p, name));
        let show = |vm: &ScriptVm, v: ScriptValue| {
            let mut out = String::new();
            vm.bx.heap.to_debug_string(v, &mut Vec::new(), &mut out, false, 0);
            out
        };
        assert_eq!(show(vm, a), show(&plain, b), "{name}");
    }
}
