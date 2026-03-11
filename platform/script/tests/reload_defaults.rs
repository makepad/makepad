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

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(0i32));
    let std = Box::leak(Box::new(0i32));
    ScriptVm {
        host,
        std,
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
