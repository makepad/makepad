//! Behaviour transcript of `#[derive(Script)]`'s generated `script_apply`: the same documents
//! applied to representative types under every `Apply` kind, each struct dumped field by field.
//! The transcript is the contract of the derive: a codegen change must reproduce it byte for
//! byte (run on the old and the new tree with `APPLY_DIFF_OUT=<file>` and compare the files).
//! The asserts pin the rules the transcript would otherwise only record.

use makepad_script::*;
use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
enum Mode {
    #[pick]
    Fill,
    Fixed,
    Fit,
}

#[derive(Debug, Default, Script, ScriptHook)]
struct Inner {
    #[live(1.0)]
    a: f64,
    #[live]
    b: bool,
    #[live]
    s: String,
    #[rust(7u32)]
    r: u32,
}

#[derive(Debug, Script, ScriptHook)]
struct Base {
    #[live(2.0)]
    x: f64,
    #[live]
    mode: Mode,
    #[live]
    inner: Inner,
}

#[derive(Debug, Script, ScriptHook)]
struct Derived {
    #[deref]
    base: Base,
    #[live(3.0)]
    y: f32,
    #[live]
    opt: Option<Splat>,
    #[live]
    list: Vec<Splat>,
    #[live]
    id: LiveId,
}

/// Two deref levels: Deep -> Derived -> Base; each level resolves its own fields from the same value.
#[derive(Debug, Script, ScriptHook)]
struct Deep {
    #[deref]
    d: Derived,
    #[live(9.0)]
    q: f64,
}

#[derive(Debug, Default, Script, ScriptHook)]
struct Splat {
    #[live(4.0)]
    w: f64,
    #[live]
    h: f64,
}

#[derive(Debug, Script, ScriptHook)]
struct WithSplat {
    #[walk]
    walk: Splat,
    #[layout]
    layout: Inner,
    #[live]
    z: f64,
}

#[derive(Debug, Script)]
struct Hooked {
    #[live]
    a: f64,
    #[live]
    b: f64,
    #[rust]
    log: Vec<String>,
}

impl ScriptHook for Hooked {
    fn on_custom_apply(&mut self, vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, value: ScriptValue) -> bool {
        // a value with `custom: true` is handled here: the generated field code must not run
        let custom = value.as_object().map(|o| get(vm, o, id!(custom))).is_some_and(|v| v.as_bool() == Some(true));
        self.log.push(format!("custom({:?})={}", apply, custom));
        if custom {
            self.a = -1.0;
        }
        custom
    }
    fn on_before_apply(&mut self, _vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        self.log.push(format!("before({:?}) a={} b={}", apply, self.a, self.b));
    }
    fn on_after_apply(&mut self, _vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, _value: ScriptValue) {
        self.log.push(format!("after({:?}) a={} b={}", apply, self.a, self.b));
    }
}

#[derive(Debug, Script, ScriptHook)]
struct State {
    #[live(1.0)]
    #[apply_state]
    kept: f64,
    #[live(true)]
    #[imperative]
    vis: bool,
    #[live(5.0)]
    plain: f64,
}

/// A field type whose apply_default returns the sub-object named by its `state` key (an
/// animator's shape): the owner re-applies itself with that block under Apply::Default(n+1).
#[derive(Debug, Default)]
struct Anim {
    state: LiveId,
    applied: Vec<String>,
}

impl ScriptHook for Anim {}
impl ScriptNew for Anim {
    fn script_new(_vm: &mut ScriptVm) -> Self {
        Self::default()
    }
}
impl ScriptApply for Anim {
    fn script_apply(&mut self, _vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, value: ScriptValue) {
        self.applied.push(format!("{:?}", apply));
        if let Some(id) = value.as_id() {
            self.state = id;
        }
    }
    fn script_to_value(&self, _vm: &mut ScriptVm) -> ScriptValue {
        self.state.into()
    }
}
impl ScriptApplyDefault for Anim {
    fn script_apply_default(&mut self, vm: &mut ScriptVm, apply: &Apply, _scope: &mut Scope, value: ScriptValue) -> Option<ScriptValue> {
        if apply.as_default().is_some() || self.state == LiveId(0) {
            return None;
        }
        let o = value.as_object()?;
        let block = get(vm, o, self.state);
        block.as_object().map(|b| b.into())
    }
}

#[derive(Debug, Script, ScriptHook)]
struct Animated {
    #[live(10.0)]
    height: f64,
    #[live(1.0)]
    alpha: f64,
    #[apply_default]
    anim: Anim,
}

fn get(vm: &mut ScriptVm, o: ScriptObject, key: LiveId) -> ScriptValue {
    let t = vm.bx.threads.cur().trap.pass();
    vm.bx.heap.value(o, key.into(), t)
}

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) }
}

fn obj(vm: &mut ScriptVm, proto: ScriptValue, kv: &[(LiveId, ScriptValue)]) -> ScriptObject {
    let o = vm.bx.heap.new_with_proto(proto);
    for (k, v) in kv {
        let t = vm.bx.threads.cur().trap.pass();
        vm.bx.heap.set_value(o, (*k).into(), *v, t);
    }
    o
}

fn errors(vm: &mut ScriptVm) -> Vec<String> {
    vm.bx.threads.cur().trap.err_take().into_iter().map(|e| e.message).collect()
}

fn applies() -> [Apply; 7] {
    [Apply::New, Apply::Reload, Apply::Rebake, Apply::ScriptReapply, Apply::Animate, Apply::Eval, Apply::Default(0)]
}

fn transcript() -> String {
    let vm = &mut test_vm();
    let mut out = String::new();
    let derived_api: ScriptValue = Derived::script_api(vm).into();
    let inner_api: ScriptValue = Inner::script_api(vm).into();
    let splat_api: ScriptValue = WithSplat::script_api(vm).into();
    let hooked_api: ScriptValue = Hooked::script_api(vm).into();
    let state_api: ScriptValue = State::script_api(vm).into();
    let anim_api: ScriptValue = Animated::script_api(vm).into();
    let splat_api0: ScriptValue = Splat::script_api(vm).into();
    let mode_api0: ScriptValue = Mode::script_api(vm).into();
    let s_hello = vm.bx.heap.new_string_from_str("hello");
    let s_world = vm.bx.heap.new_string_from_str("world");

    // prototype chain: grand (x, y, inner{a}) <- parent (mode, list) <- child (y, opt)
    let inner_doc = obj(vm, inner_api, &[(id!(a), 11.0.into()), (id!(s), s_hello)]);
    let grand = obj(vm, derived_api, &[(id!(x), 20.0.into()), (id!(y), 30.0.into()), (id!(inner), inner_doc.into())]);
    let arr = vm.bx.heap.new_array();
    for v in [1.0, 2.0, 3.0] {
        let e = obj(vm, splat_api0, &[(id!(w), v.into())]);
        let t = vm.bx.threads.cur().trap.pass();
        vm.bx.heap.array_push(arr, e.into(), t);
    }
    let fit = get(vm, mode_api0.as_object().unwrap(), id!(Fit));
    let parent = obj(vm, grand.into(), &[(id!(mode), fit), (id!(list), arr.into())]);
    let splat_small = obj(vm, splat_api0, &[(id!(h), 0.5.into())]);
    let child = obj(vm, parent.into(), &[(id!(y), 31.0.into()), (id!(opt), splat_small.into()), (id!(id), id!(abc).into())]);
    // partial / explicit nil documents
    let partial = obj(vm, derived_api, &[(id!(y), 7.0.into())]);
    let nils = obj(vm, ScriptValue::NIL, &[(id!(x), NIL), (id!(opt), NIL), (id!(y), NIL), (id!(mode), NIL)]);
    let inner_nil = obj(vm, ScriptValue::NIL, &[(id!(s), NIL), (id!(b), true.into())]);
    let with_inner_nil = obj(vm, ScriptValue::NIL, &[(id!(inner), inner_nil.into())]);
    let inner_world = obj(vm, inner_api, &[(id!(s), s_world)]);
    let with_inner_world = obj(vm, derived_api, &[(id!(inner), inner_world.into())]);

    assert_eq!(errors(vm), Vec::<String>::new(), "building the documents raised errors");
    let docs: Vec<(&str, ScriptValue)> = vec![
        ("api", derived_api),
        ("grand", grand.into()),
        ("parent", parent.into()),
        ("child", child.into()),
        ("partial", partial.into()),
        ("nils", nils.into()),
        ("inner_nil", with_inner_nil.into()),
        ("inner_world", with_inner_world.into()),
        ("nil", NIL),
    ];
    for apply in applies() {
        for (name, doc) in &docs {
            // a non-default prior state: what stays untouched is visible
            let mut d = Derived {
                base: Base { x: -1.0, mode: Mode::Fixed, inner: Inner { a: -2.0, b: false, s: "prior".into(), r: 99 } },
                y: -3.0,
                opt: Some(Splat { w: -4.0, h: -4.5 }),
                list: vec![Splat { w: -5.0, h: -5.5 }],
                id: id!(prior),
            };
            d.script_apply(vm, &apply, &mut Scope::empty(), *doc);
            let _ = writeln!(out, "derived {:?} {name}: {:?}", apply, d);
            // the reverse direction: the struct back to a value, keys in insertion order
            let v = d.script_to_value(vm);
            let mut json = String::new();
            vm.bx.heap.to_json_inner(v, &mut json);
            let _ = writeln!(out, "  to_value: {json}");
        }
    }
    // two deref levels on the same documents
    let deep_api: ScriptValue = Deep::script_api(vm).into();
    let deep_doc = obj(vm, child.into(), &[]);
    let _ = deep_api;
    for apply in applies() {
        for (name, doc) in [("child", ScriptValue::from(child)), ("deep", deep_doc.into())] {
            let mut d = Deep {
                d: Derived {
                    base: Base { x: -1.0, mode: Mode::Fixed, inner: Inner { a: -2.0, b: false, s: "prior".into(), r: 99 } },
                    y: -3.0,
                    opt: None,
                    list: vec![],
                    id: id!(prior),
                },
                q: -9.0,
            };
            d.script_apply(vm, &apply, &mut Scope::empty(), doc);
            let _ = writeln!(out, "deep {:?} {name}: {:?}", apply, d);
        }
    }

    // the missing field on reload takes the registered type default
    let inner_default = obj(vm, inner_api, &[(id!(a), 42.0.into())]);
    assert!(vm.bx.heap.set_type_default(inner_default));
    let mode_api: ScriptValue = Mode::script_api(vm).into();
    let mode_default = vm.bx.heap.new_with_proto(mode_api);
    assert!(vm.bx.heap.set_type_default(mode_default));
    for apply in applies() {
        let mut d = Derived {
            base: Base { x: -1.0, mode: Mode::Fixed, inner: Inner { a: -2.0, b: false, s: "prior".into(), r: 99 } },
            y: -3.0,
            opt: Some(Splat { w: -4.0, h: -4.5 }),
            list: vec![Splat { w: -5.0, h: -5.5 }],
            id: id!(prior),
        };
        d.script_apply(vm, &apply, &mut Scope::empty(), partial.into());
        let _ = writeln!(out, "derived-with-defaults {:?} partial: {:?}", apply, d);
        if matches!(apply, Apply::Reload) {
            assert_eq!(d.base.inner.a, 42.0, "reload default for a missing nested field");
            assert_eq!(d.base.inner.r, 99, "rust fields are never applied");
            assert_eq!(d.base.mode, Mode::Fill, "reload default for a missing enum field");
            assert_eq!(d.y, 7.0);
        }
        if matches!(apply, Apply::New) {
            assert_eq!(d.base.x, -1.0, "a missing field stays untouched");
            assert_eq!(d.base.inner.a, -2.0);
        }
    }

    // splat / walk / layout: the same value is applied to the sub-structs
    let splat_doc = obj(vm, splat_api, &[(id!(w), 40.0.into()), (id!(a), 41.0.into()), (id!(z), 42.0.into())]);
    for apply in applies() {
        let mut s = WithSplat { walk: Splat { w: -1.0, h: -2.0 }, layout: Inner { a: -3.0, b: true, s: "p".into(), r: 1 }, z: -4.0 };
        s.script_apply(vm, &apply, &mut Scope::empty(), splat_doc.into());
        let _ = writeln!(out, "splat {:?}: {:?}", apply, s);
        let v = s.script_to_value(vm);
        let mut json = String::new();
        vm.bx.heap.to_json_inner(v, &mut json);
        let _ = writeln!(out, "  to_value: {json}");
    }

    // on_custom_apply short-circuit and hook order
    let hooked_plain = obj(vm, hooked_api, &[(id!(a), 1.0.into()), (id!(b), 2.0.into())]);
    let hooked_custom = obj(vm, ScriptValue::NIL, &[(id!(a), 1.0.into()), (id!(custom), true.into())]);
    for apply in applies() {
        for (name, doc) in [("plain", hooked_plain), ("custom", hooked_custom)] {
            let mut h = Hooked { a: 0.0, b: 0.0, log: Vec::new() };
            h.script_apply(vm, &apply, &mut Scope::empty(), doc.into());
            let _ = writeln!(out, "hooked {:?} {name}: {:?}", apply, h);
            if name == "custom" {
                assert_eq!(h.b, 0.0, "on_custom_apply skips the generated field code");
            }
        }
    }

    // apply_state / visible preserve runtime state on reapply
    let state_doc = obj(vm, state_api, &[(id!(kept), 2.0.into()), (id!(vis), false.into()), (id!(plain), 6.0.into())]);
    for apply in applies() {
        let mut s = State { kept: 9.0, vis: true, plain: 9.0 };
        s.script_apply(vm, &apply, &mut Scope::empty(), state_doc.into());
        let _ = writeln!(out, "state {:?}: {:?}", apply, s);
    }

    // animator: the state block re-applies a small object to the owner (Apply::Default(n+1))
    let hidden = obj(vm, ScriptValue::NIL, &[(id!(height), 0.0.into())]);
    let _ = anim_api;
    let anim_doc = obj(vm, ScriptValue::NIL, &[(id!(height), 100.0.into()), (id!(anim), id!(hidden).into()), (id!(hidden), hidden.into())]);
    let anim_small = obj(vm, ScriptValue::NIL, &[(id!(alpha), 0.25.into())]);
    for apply in applies() {
        let mut a = Animated { height: -1.0, alpha: -1.0, anim: Anim::default() };
        a.script_apply(vm, &apply, &mut Scope::empty(), anim_doc.into());
        let _ = writeln!(out, "animated {:?} doc: {:?}", apply, a);
        a.script_apply(vm, &Apply::Animate, &mut Scope::empty(), anim_small.into());
        let _ = writeln!(out, "animated {:?} then small block: {:?}", apply, a);
    }
    let _ = writeln!(out, "errors: {:?}", errors(vm));
    out
}

#[test]
fn apply_transcript() {
    let t = transcript();
    if let Ok(path) = std::env::var("APPLY_DIFF_OUT") {
        std::fs::write(path, &t).unwrap();
    }
    // the rules every codegen keeps
    assert!(t.contains("derived New partial: Derived { base: Base { x: -1.0, mode: Fixed"), "missing fields untouched:\n{t}");
}
