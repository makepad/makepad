//! `build_typed_edit`: a property edit built from data against the property's declared type,
//! never evaluated. The edits a collaborator sends to a shared app go this way, so nothing in
//! them may run in the app: an app's own capitalised function is a name like any other, and a
//! value that names it is refused without a call.

use makepad_script::makepad_math::Vec4f;
use makepad_script::syntax::parse_value;
use makepad_script::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
enum Bound {
    #[pick(100.0)]
    Abs(f64),
    Fit,
}

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
enum Extent {
    #[pick {
        weight: 100.0,
        basis: Bound::Abs(0.0),
        shrink: 0.0,
    }]
    Fill { weight: f64, basis: Bound, shrink: f64 },
    #[live(200.0)]
    Fixed(f64),
    Fit,
}

#[derive(Clone, Copy, Debug, PartialEq, Script, ScriptHook)]
enum Dir {
    #[pick { wrap: false }]
    Right { wrap: bool },
    Down,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Script, ScriptHook)]
struct Inset {
    #[live]
    left: f64,
    #[live]
    right: f64,
}

#[derive(Debug, PartialEq, Script, ScriptHook)]
struct Branch {
    #[live]
    children: Vec<Branch>,
}

#[derive(Script, ScriptHook)]
struct Panel {
    #[live]
    width: Extent,
    #[live]
    flow: Dir,
    #[live]
    padding: Inset,
    #[live]
    label: String,
    #[live]
    labels: Vec<String>,
    #[live]
    branches: Vec<Branch>,
    #[live]
    gap: f64,
    #[live]
    tint: Vec4f,
    #[live]
    on_click: ScriptFnRef,
}

static CALLS: AtomicUsize = AtomicUsize::new(0);

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) }
}

fn edit(vm: &mut ScriptVm, target: ScriptObject, path: &str, value: &str) -> Result<ScriptObject, TypedRefusal> {
    let node = parse_value(value).unwrap_or_else(|e| panic!("`{value}` does not parse: {e:?}"));
    let path: Vec<LiveId> = path.split('.').map(LiveId::from_str).collect();
    vm.build_typed_edit(target, &path, value, &node)
}

/// No function anywhere in what the builder made.
fn assert_data(vm: &ScriptVm, value: ScriptValue, what: &str) {
    if let Some(array) = value.as_array() {
        for i in 0..vm.bx.heap.array_len(array) {
            assert_data(vm, vm.bx.heap.array_index_unchecked(array, i), what);
        }
    }
    let Some(object) = value.as_object() else { return };
    assert!(!vm.bx.heap.is_fn(object), "{what}: the built value holds a function");
    for entry in vm.bx.heap.map_entries(object) {
        assert_data(vm, entry.value, what);
    }
    for i in 0..vm.bx.heap.vec_len(object) {
        if let Some(item) = vm.bx.heap.vec_value_if_exist(object, i) {
            assert_data(vm, item, what);
        }
    }
}

fn eval(vm: &mut ScriptVm, source: &str) -> ScriptValue {
    vm.with_instruction_limit(100_000, |vm| vm.eval(ScriptMod::new("", "", "typed_values", 0, 0, source, &[])))
}

#[test]
fn typed_edits_apply_real_values_and_refuse_everything_else() {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());

    // The app's own capitalised function, callable by its bare name and as a key of an enum's
    // API object, the places a variant name is looked up.
    let app = vm.new_module(id!(app));
    vm.add_method(app, id_lut!(Danger), script_args_def!(x = NIL), |_vm, _args| {
        CALLS.fetch_add(1, Ordering::SeqCst);
        NIL
    });
    let danger = vm.bx.heap.value(app, id!(Danger).into(), NoTrap);
    assert!(danger.as_object().is_some_and(|f| vm.bx.heap.is_fn(f)), "Danger is a function");
    vm.set_injected_global(id!(Danger), danger);
    let extent_api = <Extent as ScriptNew>::script_proto(vm).as_object().unwrap();
    vm.bx.heap.map_insert_own(extent_api, id!(Danger).into(), danger);
    vm.bx.heap.set_value(app, id!(Extent).into(), extent_api.into(), NoTrap);
    let theme = vm.new_module(id!(theme));
    vm.bx.heap.set_value(theme, id!(space).into(), 6.0.into(), NoTrap);
    vm.bx.heap.set_value(theme, id!(hook).into(), danger, NoTrap);
    // Control: both are live and counted when script code really calls them.
    eval(vm, "Danger(1)\nmod.app.Extent.Danger(2)");
    assert_eq!(CALLS.load(Ordering::SeqCst), 2, "the control calls ran: {:?}", vm.take_errors());

    // As the draw prelude does (`Inset: set_type_default() do #(Inset::script_api(vm))`): the
    // default a nested edit such as `padding.left` copies.
    let inset_api = <Inset as ScriptNew>::script_api(vm).as_object().unwrap();
    vm.bx.heap.set_type_default(inset_api);
    let api = <Panel as ScriptNew>::script_proto(vm);
    let branch = vm.heap().type_info(TYPE_REGISTERED, Branch::script_type_id_static(), ScriptObject::ZERO);
    assert!(branch.proto.is_object(), "registering the vector field registers its element's prototype");
    vm.set_injected_global(id!(Branch), branch.proto);
    vm.set_injected_global(id!(outer_label), ScriptValue::from_inline_string("outer").unwrap());
    let target = vm.bx.heap.new_with_proto(api);
    let shader_input = vm.heap_mut().new_with_proto(1.0.into());
    vm.heap_mut().set_shader_io(shader_input, makepad_script::mod_shader::SHADER_IO_DYN_INSTANCE);
    vm.heap_mut().map_insert_own(target, id!(intensity).into(), shader_input.into());
    let mut panel = <Panel as ScriptNew>::script_new(vm);
    panel.script_apply(vm, &Apply::New, &mut Scope::empty(), target.into());
    let apply = |vm: &mut ScriptVm, panel: &mut Panel, path: &str, value: &str| {
        let built = edit(vm, target, path, value).unwrap_or_else(|why| panic!("{path}: {value} was refused: {why}"));
        assert_data(vm, built.into(), value);
        panel.script_apply(vm, &Apply::Eval, &mut Scope::empty(), built.into());
        built
    };

    apply(vm, &mut panel, "width", "Extent.Fixed(200)");
    assert_eq!(panel.width, Extent::Fixed(200.0), "a tuple variant");
    apply(vm, &mut panel, "width", "Fixed(-3)");
    assert_eq!(panel.width, Extent::Fixed(-3.0), "a tuple variant without its type's name");
    apply(vm, &mut panel, "width", "Extent.Fit");
    assert_eq!(panel.width, Extent::Fit, "a bare variant");
    apply(vm, &mut panel, "width", "Extent.Fill{weight: 50 basis: Bound.Abs(0) shrink: 0}");
    assert_eq!(panel.width, Extent::Fill { weight: 50.0, basis: Bound::Abs(0.0), shrink: 0.0 }, "a named variant row as listed");
    apply(vm, &mut panel, "flow", "Dir.Down");
    assert_eq!(panel.flow, Dir::Down, "a bare variant of another enum");
    apply(vm, &mut panel, "flow", "Dir.Right{wrap: true}");
    assert_eq!(panel.flow, Dir::Right { wrap: true }, "a named variant with a field");
    apply(vm, &mut panel, "padding.left", "17");
    assert_eq!(panel.padding, Inset { left: 17.0, right: 0.0 }, "a nested field");
    apply(vm, &mut panel, "padding", "Inset{right: 4}");
    assert_eq!(panel.padding.right, 4.0, "a struct value");
    apply(vm, &mut panel, "label", "\"hi\"");
    assert_eq!(panel.label, "hi", "a string");
    for (path, literal) in [
        ("labels", r#"["first", "second label"]"#),
        ("branches", "[Branch{children: [Branch{}]}, Branch{}]"),
    ] {
        let evaluated = eval(vm, literal);
        let built = apply(vm, &mut panel, path, literal);
        let value = vm.heap().value(built, LiveId::from_str(path).into(), NoTrap);
        assert!(evaluated.is_array() && vm.heap().deep_eq(value, evaluated),
            "{path}: the typed array must match the VM literal, including element prototypes and storage");
    }
    assert_eq!(panel.labels, ["first", "second label"], "a vector applies string elements in order");
    assert_eq!(panel.branches, [Branch { children: vec![Branch { children: vec![] }] }, Branch { children: vec![] }],
        "a vector applies struct elements with nested vector fields");
    apply(vm, &mut panel, "labels", "[]");
    assert!(panel.labels.is_empty(), "an empty array clears a populated vector");
    apply(vm, &mut panel, "gap", "theme.space");
    assert_eq!(panel.gap, 6.0, "a theme value");
    apply(vm, &mut panel, "tint", "#f00");
    assert_eq!(panel.tint, Vec4f { x: 1.0, y: 0.0, z: 0.0, w: 1.0 }, "a colour");
    apply(vm, &mut panel, "tint", "vec4f(0 1 0 1)");
    assert_eq!(panel.tint, Vec4f { x: 0.0, y: 1.0, z: 0.0, w: 1.0 }, "a vector literal");
    apply(vm, &mut panel, "tint", "vec4f(0, -1, 0, 1)");
    assert_eq!(panel.tint, Vec4f { x: 0.0, y: -1.0, z: 0.0, w: 1.0 }, "a vector literal with a negative component");
    let errors = vm.take_errors();
    assert!(errors.is_empty(), "the applies reported errors: {errors:?}");

    use TypedRefusal::*;
    let refused: &[(&str, &str, fn(&TypedRefusal) -> bool)] = &[
        // the app's function, wherever a variant name is read
        ("width", "Danger(1)", |r| matches!(r, UnknownVariant(_))),
        ("width", "Danger{}", |r| matches!(r, WrongType(_))),
        ("width", "Extent.Danger(1)", |r| matches!(r, UnknownVariant(_))),
        ("width", "Danger", |r| matches!(r, WrongType(_))),
        ("width", "mod.app.Danger(1)", |r| matches!(r, NotData(_))),
        ("gap", "theme.hook", |r| matches!(r, NotData(_))),
        // types, variants and fields that are not declared
        ("width", "Dir.Down", |r| matches!(r, WrongType(_))),
        ("width", "Extent.Nope", |r| matches!(r, UnknownVariant(_))),
        ("width", "Extent.Fixed(\"x\")", |r| matches!(r, WrongType(_))),
        ("width", "Extent.Fixed(1, 2)", |r| matches!(r, WrongType(_))),
        ("width", "Extent.Fixed", |r| matches!(r, WrongType(_))),
        ("width", "Extent.Fill{weight: 1 nope: 2}", |r| matches!(r, UnknownProp(_))),
        ("padding.nope", "1", |r| matches!(r, UnknownProp(_))),
        ("padding", "vec2f(1, 1)", |r| matches!(r, WrongType(why) if why == "Inset does not take vec2f(1, 1)")),
        ("gap", "\"wide\"", |r| matches!(r, WrongType(_))),
        ("tint", "vec4f(1 2)", |r| matches!(r, WrongType(why) if why == "vec4f takes 4 numbers, separated by commas")),
        ("tint", "vec4f(1 -1 0 1)", |r| matches!(r, WrongType(why) if why == "vec4f takes 4 numbers, separated by commas")),
        ("tint", "vec2f(1 2)", |r| matches!(r, WrongType(_))),
        ("width", "vec4f(1 2 3 4)", |r| matches!(r, WrongType(_))),
        ("tint", "vec4f(1 loop{} 0 0)", |r| matches!(r, NotData(_))),
        ("tint", "vec4f(1 \"a\" 0 0)", |r| matches!(r, NotData(_))),
        ("tint", "vec4f(1 #f00 0 0)", |r| matches!(r, NotData(_))),
        // a function-typed property takes nothing built from data
        ("on_click", "fn(){ 1 }", |r| matches!(r, NotData(_))),
        ("on_click", "Danger", |r| matches!(r, UnknownVariant(_))),
        ("on_click.x", "1", |r| matches!(r, UnknownProp(_))),
        // code, at the top or nested
        ("gap", "loop{}", |r| matches!(r, NotData(_))),
        ("width", "Extent.Fill{weight: loop{}}", |r| matches!(r, NotData(_))),
        ("width", "Extent.Fixed(1 + 2)", |r| matches!(r, NotData(_))),
        ("gap", "1 + 2", |r| matches!(r, NotData(_))),
        ("label", "mod.std.print(1)", |r| matches!(r, NotData(_))),
        ("padding", "Inset{left := 3}", |r| matches!(r, NotData(_))),
        ("width", "[1, 2]", |r| matches!(r, WrongType(_))),
        ("gap", "[]", |r| matches!(r, WrongType(_))),
        ("intensity", "[]", |r| matches!(r, WrongType(_))),
        ("labels", r#"["first", 1]"#, |r| matches!(r, WrongType(_))),
        ("labels", r#"["first", Danger(1)]"#, |r| matches!(r, WrongType(_))),
        ("labels", r#"["first", outer_label]"#, |r| matches!(r, WrongType(_))),
        ("labels", r#"["first", fn(){ 1 }]"#, |r| matches!(r, NotData(_))),
        ("branches", "[Branch{children: [theme.hook]}]", |r| matches!(r, NotData(_))),
    ];
    for (path, value, expected) in refused {
        match edit(vm, target, path, value) {
            Ok(_) => panic!("{path}: {value} was built"),
            Err(why) => assert!(expected(&why), "{path}: {value} was refused for another reason: {why:?}"),
        }
    }
    let vector = format!("vec4f(1, \"{}\", 0, 0)", "é".repeat(80));
    assert_eq!(edit(vm, target, "tint", &vector).unwrap_err(), NotData(format!("\"{}", "é".repeat(79))),
        "a refusal quotes up to 80 characters of the offending argument");
    // 100k levels deep: the parser keeps an over-nested subtree as text and the build refuses
    // it, never a stack overflow; the VM goes on applying edits after.
    let deep = 100_000;
    for (path, value) in [
        ("width", format!("{}{}", "[".repeat(deep), "]".repeat(deep))),
        ("padding", format!("{}0{}", "Inset{left: ".repeat(deep), "}".repeat(deep))),
        ("width", format!("{}0{}", "Extent.Fixed(".repeat(deep), ")".repeat(deep))),
        ("width", format!("{}0{}", "Extent.Fill{weight: ".repeat(deep), "}".repeat(deep))),
        ("gap", format!("{}1", "-".repeat(deep))),
    ] {
        let built = parse_value(&value).ok().map(|node| {
            let path: Vec<LiveId> = path.split('.').map(LiveId::from_str).collect();
            vm.build_typed_edit(target, &path, &value, &node)
        });
        assert!(!built.is_some_and(|built| built.is_ok()), "{path}: a value {deep} deep was built");
    }
    // A recursive type (`Branch` holds `Vec<Branch>`) takes a value as deep as the stack allows:
    // the parser and the build each stop at half of the stack left where they start. 10k levels
    // of at least three frames each overflow a native test thread's 2 MiB stack without those
    // guards, and are refused with them; JIT code runs on a larger stack, where the value is
    // built. Either is right. What must hold is that it never overflows.
    let rec = 10_000;
    let branches = format!("{}[]{}", "[Branch{children: ".repeat(rec), "}]".repeat(rec));
    if let Ok(node) = parse_value(&branches) {
        let _ = vm.build_typed_edit(target, &[id!(branches)], &branches, &node);
    }
    apply(vm, &mut panel, "gap", "7");
    assert_eq!(panel.gap, 7.0, "edits apply after the deep values");
    assert_eq!(CALLS.load(Ordering::SeqCst), 2, "a refused edit called the app's function");
    assert_eq!(panel.width, Extent::Fill { weight: 50.0, basis: Bound::Abs(0.0), shrink: 0.0 }, "refusals leave the panel as it was");
}
