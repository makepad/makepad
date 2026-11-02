use makepad_script::*;

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
enum ReloadEnumTest {
    #[pick]
    Fill,
    Fixed,
}

#[derive(Debug, Script, ScriptHook)]
struct ReloadEnumHolderTest {
    #[live]
    value: ReloadEnumTest,
}

#[derive(Debug, Script, ScriptHook)]
struct ReloadDeclaredDefaultTest {
    #[live(ReloadEnumTest::Fixed)]
    value: ReloadEnumTest,
}

#[derive(Debug, Script, ScriptHook)]
struct ReloadObjectInnerTest {
    #[live(1.0)]
    value: f64,
    #[rust(7u32)]
    rust_value: u32,
}

#[derive(Debug, Script, ScriptHook)]
struct ReloadObjectOuterTest {
    #[live]
    inner: ReloadObjectInnerTest,
}

#[derive(Script, ScriptHook)]
struct ReloadRuntimeStateTest {
    #[source]
    source: ScriptObjectRef,
    #[imperative]
    #[live(true)]
    visible: bool,
    #[live]
    n: f64,
}

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    }
}

#[test]
fn reload_missing_live_field_without_type_default_keeps_existing_value() {
    let vm = &mut test_vm();

    let holder_api = ReloadEnumHolderTest::script_api(vm);
    let holder_value = vm.bx.heap.new_with_proto(holder_api);
    let mut holder = ReloadEnumHolderTest {
        value: ReloadEnumTest::Fixed,
    };

    holder.script_apply(vm, &Apply::Reload, &mut Scope::empty(), holder_value.into());
    assert_eq!(holder.value, ReloadEnumTest::Fixed);
}

#[test]
fn reload_missing_enum_field_with_type_default_uses_pick_variant() {
    let vm = &mut test_vm();

    let enum_api = ReloadEnumTest::script_api(vm);
    let enum_default = vm.bx.heap.new_with_proto(enum_api);
    assert!(vm.bx.heap.set_type_default(enum_default));

    let holder_api = ReloadEnumHolderTest::script_api(vm);
    let holder_value = vm.bx.heap.new_with_proto(holder_api);
    let mut holder = ReloadEnumHolderTest {
        value: ReloadEnumTest::Fixed,
    };

    holder.script_apply(vm, &Apply::Reload, &mut Scope::empty(), holder_value.into());
    assert_eq!(holder.value, ReloadEnumTest::Fill);
}

#[test]
fn reload_missing_object_field_with_type_default_refreshes_from_type_default() {
    let vm = &mut test_vm();

    let inner_api = ReloadObjectInnerTest::script_api(vm);
    let inner_default = vm.bx.heap.new_with_proto(inner_api);
    vm.bx.heap.set_value(
        inner_default,
        id!(value).into(),
        42.0.into(),
        vm.bx.threads.cur().trap.pass(),
    );
    assert!(vm.bx.heap.set_type_default(inner_default));

    let outer_api = ReloadObjectOuterTest::script_api(vm);
    let outer_value = vm.bx.heap.new_with_proto(outer_api);
    let mut outer = ReloadObjectOuterTest {
        inner: ReloadObjectInnerTest {
            value: 1.0,
            rust_value: 99,
        },
    };

    outer.script_apply(vm, &Apply::Reload, &mut Scope::empty(), outer_value.into());
    assert_eq!(outer.inner.value, 42.0);
    assert_eq!(outer.inner.rust_value, 99);
}

#[test]
fn reload_missing_field_with_declared_default_goes_back_to_it() {
    let vm = &mut test_vm();

    let enum_api = ReloadEnumTest::script_api(vm);
    let enum_default = vm.bx.heap.new_with_proto(enum_api);
    assert!(vm.bx.heap.set_type_default(enum_default));

    let holder_api = ReloadDeclaredDefaultTest::script_api(vm);
    let holder_value = vm.bx.heap.new_with_proto(holder_api);
    // A new one starts out with the field's own default...
    let mut holder = ReloadDeclaredDefaultTest::script_new(vm);
    assert_eq!(holder.value, ReloadEnumTest::Fixed);

    // ...and a reload that doesn't set the field goes back to that, not to the type's default.
    holder.value = ReloadEnumTest::Fill;
    holder.script_apply(vm, &Apply::Reload, &mut Scope::empty(), holder_value.into());
    assert_eq!(holder.value, ReloadEnumTest::Fixed);
}

#[test]
fn reload_only_changed_template_fields_overwrite_runtime_state() {
    let vm = &mut test_vm();
    let api = ReloadRuntimeStateTest::script_api(vm);
    // Declared-default fallback requires a registered type default; bool's scalar proto
    // has none, so give this fixture a typed default object without changing that rule.
    let bool_default = vm.bx.heap.new_with_proto(false.into());
    register_type_inner(
        vm, bool::script_type_id_static(), bool_default.into(), ScriptTypeProps::default(),
        script_type_check_c::<bool>, Some(id!(bool)), Some("bool"), false,
    );
    assert!(vm.bx.heap.set_type_default(bool_default));
    let template = |vm: &mut ScriptVm, visible: Option<bool>, n: f64| {
        let obj = vm.bx.heap.new_with_proto(api);
        if let Some(visible) = visible {
            vm.bx.heap.set_value(obj, id!(visible).into(), visible.into(), vm.bx.threads.cur().trap.pass());
        }
        vm.bx.heap.set_value(obj, id!(n).into(), n.into(), vm.bx.threads.cur().trap.pass());
        ScriptValue::from(obj)
    };
    let mut holder = ReloadRuntimeStateTest::script_new(vm);
    let value = template(vm, Some(false), 1.0);
    holder.script_apply(vm, &Apply::New, &mut Scope::empty(), value);
    assert_eq!((holder.visible, holder.n), (false, 1.0), "New applies the template");

    holder.visible = true;
    let value = template(vm, Some(false), 1.0);
    holder.script_apply(vm, &Apply::Reload, &mut Scope::empty(), value);
    assert!(holder.visible, "an identical fresh template keeps runtime visibility");

    let value = template(vm, Some(false), 2.0);
    holder.script_apply(vm, &Apply::Reload, &mut Scope::empty(), value);
    assert_eq!((holder.visible, holder.n), (true, 2.0), "an unrelated edit preserves visibility but applies n");

    holder.visible = false;
    let value = template(vm, Some(true), 2.0);
    holder.script_apply(vm, &Apply::Reload, &mut Scope::empty(), value);
    assert!(holder.visible, "an explicit template change overwrites runtime visibility");

    holder.visible = false;
    let value = template(vm, None, 2.0);
    holder.script_apply(vm, &Apply::Reload, &mut Scope::empty(), value);
    assert!(holder.visible, "removing visibility restores its declared default");

    holder.visible = false;
    let value = template(vm, None, 2.0);
    holder.script_apply(vm, &Apply::Reload, &mut Scope::empty(), value);
    assert!(!holder.visible, "visibility absent from both templates keeps runtime state");

    let value = template(vm, Some(true), 2.0);
    holder.script_apply(vm, &Apply::Reload, &mut Scope::empty(), value);
    assert!(holder.visible, "adding visibility to the template applies it");
}
