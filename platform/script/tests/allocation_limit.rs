use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn script(name: &str, code: &str) -> ScriptMod {
    ScriptMod::new("", "", &format!("allocation_limit_{name}"), 0, 0, &code, &[])
}

fn eval_limited(name: &str, code: &str, limit: usize) -> (ScriptAllocationReport, Vec<String>) {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let (_, report) = vm.with_heap_allocation_limit(limit, |vm| vm.eval(script(name, code)));
    (report, vm.take_errors())
}

fn call_probe(vm: &mut ScriptVm, params: &[(LiveId, ScriptValue)]) -> (ScriptValue, std::rc::Rc<std::cell::Cell<usize>>) {
    let calls = std::rc::Rc::new(std::cell::Cell::new(0));
    let count = calls.clone();
    let module = vm.new_module(id!(probe));
    vm.add_method(module, id_lut!(visit), params, move |_, _| {
        count.set(count.get() + 1);
        42.into()
    });
    (vm.value(module, id!(visit)), calls)
}

fn drain_free_objects(vm: &mut ScriptVm) {
    let free = vm.heap().objects_len() - vm.heap().gc_live_counts().objects;
    for _ in 0..free { vm.new_object(); }
}

fn assert_call_limit(vm: &mut ScriptVm, result: ScriptValue, report: ScriptAllocationReport, operation: &str) {
    let errors = vm.take_error_records();
    assert!(result.is_err(), "a refusal while {operation} must return an error: {result:?}");
    assert!(report.exceeded, "the refusal while {operation} must appear in the budget report");
    assert_eq!(errors.len(), 1, "expected exactly one diagnostic: {errors:?}");
    assert!(errors[0].message().contains(&format!("script allocation limit exceeded while {operation}")),
        "the host must receive the original refusal: {errors:?}");
    vm.capture_errors();
}

fn drain_free_arrays(vm: &mut ScriptVm) {
    while !vm.with_heap_allocation_limit(0, |vm| vm.heap_mut().new_array()).1.exceeded {}
    vm.take_errors();
    vm.capture_errors();
}

#[test]
fn rust_map_and_derive_enum_to_value_stop_on_refusal() {
    use std::cell::Cell;
    use std::collections::{BTreeMap, HashMap};
    use std::rc::Rc;

    #[derive(Clone, Default)]
    struct CountingVal(Rc<Cell<usize>>);
    impl ScriptDeriveMarker for CountingVal {}
    impl ScriptHook for CountingVal {}
    impl ScriptNew for CountingVal {
        fn script_new(_: &mut ScriptVm) -> Self { Self::default() }
    }
    impl ScriptApply for CountingVal {
        fn script_to_value(&self, _: &mut ScriptVm) -> ScriptValue {
            self.0.set(self.0.get() + 1);
            1.into()
        }
    }

    #[derive(Script, ScriptHook)]
    struct CountedFields {
        #[live] a: CountingVal,
        #[live] b: CountingVal,
    }

    let vm = &mut test_vm();
    vm.capture_errors();
    let value = CountingVal::default();
    let fields = CountedFields { a: value.clone(), b: value.clone() };
    CountedFields::script_proto(vm);
    let args = vm.new_object();
    let mut hash = HashMap::new();
    let mut tree = BTreeMap::new();
    let mut ids = LiveIdMap::default();
    for i in 0..8 {
        let key = format!("k{i}");
        ids.insert(LiveId::from_str(&key), value.clone());
        hash.insert(key.clone(), value.clone());
        tree.insert(key, value.clone());
    }
    let items = vec![value.clone(); 8];
    drain_free_objects(vm);
    drain_free_arrays(vm);
    let obj = size_of::<ScriptObjectData>();
    let entry = size_of::<(ScriptValue, ScriptMapValue)>() + 2 * size_of::<usize>();
    let maps: [&dyn ScriptApply; 3] = [&hash, &tree, &ids];
    let mut convert = |source: &dyn ScriptApply, budget| {
        value.0.set(0);
        let (result, report) = vm.with_heap_allocation_limit(budget, |vm| source.script_to_value(vm));
        assert!(report.exceeded, "the converter must hit its {budget}-byte budget");
        let len = if let Some(object) = result.as_object() {
            vm.heap().map_len(object)
        } else {
            vm.heap().array_len(result.as_array().expect("a vector converter returns an array"))
        };
        (value.0.get(), len, result)
    };
    let empty = maps.map(|map| convert(map, 0).0);
    let partial = maps.map(|map| { let (count, len, _) = convert(map, obj + entry); (count, len) });
    let (first_count, first_len, first) = convert(&tree, obj);
    let empty_vec = convert(&items, 0).0;
    let (vec_count, vec_len, _) = convert(&items, size_of::<ScriptArrayData>() + size_of::<ScriptValue>());
    let tagged = vm.heap().obj_info(first.as_object().unwrap()).tag.is_string_keys();
    let (tuple, report) = vm.with_heap_allocation_limit(0, |vm| {
        let ret = vm.script_derive_enum_tuple_new(args, id!(Pair), "MyEnum",
            &[script_type_check_c::<f64>, script_type_check_c::<f64>], file!(), line!());
        (ret.is_nil(), vm.bx.parts().trap.err_is_empty())
    });
    assert!(report.exceeded, "the tuple constructor must refuse its object");
    value.0.set(0);
    let (_, report) = vm.with_heap_allocation_limit(0, |vm| fields.script_to_value(vm));
    assert!(report.exceeded, "the derived struct must refuse its object");
    assert_eq!((empty, partial, (first_count, first_len, tagged), (empty_vec, vec_count, vec_len), tuple, value.0.get()),
        ([0; 3], [(2, 1); 3], (1, 0, false), (0, 2, 1), (true, true), 0),
        "map and vector conversions must stop at the first refusal, an empty map must stay untagged, a refused tuple must return NIL with a clean trap, and derived fields must stay unvisited");
}

#[test]
fn typed_value_and_edit_surface_allocation_refusal() {
    use makepad_script::syntax::parse_value;

    #[derive(Script, ScriptHook)]
    struct Sub { #[live] x: f64 }
    #[derive(Script, ScriptHook)]
    enum Choice { #[pick(0.0, 0.0)] Pair(f64, f64) }
    #[derive(Script, ScriptHook)]
    struct Fields {
        #[live] a: f64,
        #[live] b: f64,
        #[live] label: String,
        #[live] sub: Sub,
        #[live] labels: Vec<String>,
        #[live] choice: Choice,
    }

    fn build<T: ScriptNew>(vm: &mut ScriptVm, src: &str) -> Result<(), TypedRefusal> {
        let node = parse_value(src).expect("valid typed value fixture");
        vm.build_typed(T::script_type_id_static(), src, &node).map(|_| ())
    }
    fn edit(vm: &mut ScriptVm, target: ScriptObject, path: &[LiveId], src: &str) -> Result<(), TypedRefusal> {
        let node = parse_value(src).expect("valid typed edit fixture");
        vm.build_typed_edit(target, path, src, &node).map(|_| ())
    }

    let vm = &mut test_vm();
    vm.capture_errors();
    let sub = Sub::script_api(vm).as_object().unwrap();
    vm.heap_mut().set_type_default(sub);
    let proto = Fields::script_proto(vm);
    let target = vm.heap_mut().new_with_proto(proto);
    let native_saw_limit = std::rc::Rc::new(std::cell::Cell::new(false));
    let saw_limit = native_saw_limit.clone();
    let module = vm.new_module(id!(probe));
    vm.add_method(module, id_lut!(try_edit), &[], move |vm, _| {
        let result = edit(vm, target, &[id!(label)], r#""too_long""#);
        saw_limit.set(matches!(result, Err(TypedRefusal::AllocationLimit(_))));
        99.into()
    });
    let try_edit = vm.value(module, id!(try_edit));
    let object = size_of::<ScriptObjectData>();
    let entry = size_of::<(ScriptValue, ScriptMapValue)>() + 2 * size_of::<usize>();
    vm.heap_mut().set_max_string_bytes(Some(4));
    let string = edit(vm, target, &[id!(label)], r#""too_long""#);
    let pending = vm.heap().allocation_error_pending();
    let recovered = vm.eval(script("recovery_string", "6 * 7")).as_number();
    assert!(vm.heap_mut().take_string_limit_exceeded(), "the string field must reach its string limit");
    vm.heap_mut().set_max_string_bytes(None);
    drain_free_objects(vm);
    drain_free_arrays(vm);

    let (fields, rf) = vm.with_heap_allocation_limit(object + entry, |vm| build::<Fields>(vm, "{a: 1, b: 2}"));
    let (tuple, rt) = vm.with_heap_allocation_limit(0, |vm| build::<Choice>(vm, "Pair(1, 2)"));
    let (store, rs) = vm.with_heap_allocation_limit(object, |vm| edit(vm, target, &[id!(a)], "1"));
    let (array, ra) = vm.with_heap_allocation_limit(0, |vm| build::<Vec<String>>(vm, r#"["a", "b", "c"]"#));
    let (nested, rn) = vm.with_heap_allocation_limit(object, |vm| edit(vm, target, &[id!(sub), id!(x)], "1"));
    assert_eq!([rf.exceeded, rt.exceeded, rs.exceeded, ra.exceeded, rn.exceeded], [true; 5],
        "fields, tuple, edit store, array and nested edit must each reach the scoped budget");
    assert_eq!(vm.eval(script("recovery_budget", "6 * 7")).as_number(), Some(42.0),
        "the next eval must succeed after the scoped host refusals");
    vm.take_errors();
    vm.capture_errors();

    vm.heap_mut().set_max_string_bytes(Some(4));
    let returned = vm.call(try_edit, &[]);
    assert!(returned.is_err(), "a native which ignores a refused typed edit must still bail");
    assert!(vm.heap_mut().take_string_limit_exceeded(), "the native call must report the string limit");
    let errors = vm.take_error_records();
    assert_eq!(errors.len(), 1, "the enclosing VM entry must deliver exactly one diagnostic: {errors:?}");
    assert!(errors[0].message().contains("script string allocation limit exceeded"),
        "the enclosing VM entry must deliver the original string limit: {errors:?}");
    let results = [string, fields, tuple, store, array, nested];
    assert_eq!((results.each_ref().map(|r| matches!(r, Err(TypedRefusal::AllocationLimit(_)))),
                pending, recovered, native_saw_limit.get()), ([true; 6], false, Some(42.0), true),
        "typed string, fields, tuple, store, array and nested edit must return the allocation refusal; direct calls must consume it and nested calls must leave it for the VM: {results:?}");
}

#[test]
fn call_with_args_object_refused_receiver_skips_callee() {
    let object = size_of::<ScriptObjectData>();
    let entry = size_of::<(ScriptValue, ScriptMapValue)>() + 2 * size_of::<usize>();
    let vm = &mut test_vm();
    vm.capture_errors();
    let (f, calls) = call_probe(vm, script_args_def!(x = NIL));
    let args = vm.heap_mut().new_object();
    vm.heap_mut().vec_push(args, NIL, 1.into(), NoTrap);
    vm.heap_mut().set_value_def(args, id!(self).into(), 123.into());
    drain_free_objects(vm);
    for (budget, operation) in [(0, "creating an object with a prototype"), (object + entry, "adding an object property")] {
        let (result, report) = vm.with_heap_allocation_limit(budget, |vm| vm.call_with_args_object(f, args));
        assert_call_limit(vm, result, report, operation);
        assert_eq!(calls.get(), 0, "the native must wait for a complete argument object");
    }
    assert_eq!(vm.call_with_args_object(f, args), 42.into(), "the complete call must return its value");
    assert_eq!(calls.get(), 1, "a successful call must dispatch exactly once");
}

#[test]
fn callback_scope_and_native_call_refusal_skip_callee() {
    let vm = &mut test_vm();
    vm.capture_errors();
    let (f, calls) = call_probe(vm, script_args_def!(a = NIL, b = NIL));
    let script_fn = vm.eval(script("callback", "fn(a = 0) { a }"));
    let args = [1.into(), 2.into()];
    let object = size_of::<ScriptObjectData>();
    let entry = size_of::<(ScriptValue, ScriptMapValue)>() + 2 * size_of::<usize>();
    drain_free_objects(vm);
    let (result, report) = vm.with_heap_allocation_limit(0, |vm| vm.call(script_fn, &args[..1]));
    assert_call_limit(vm, result, report, "creating an object with a prototype");
    let (result, report) = vm.with_heap_allocation_limit(object + entry, |vm| vm.call(f, &args));
    assert_call_limit(vm, result, report, "binding function arguments");
    assert_eq!(calls.get(), 0, "a refused second argument must prevent dispatch");

    // The host can also enter with a refusal pending from earlier work in the same budget.
    let (result, report) = vm.with_heap_allocation_limit(0, |vm| {
        vm.heap_mut().new_string_from_str("pending call refusal");
        vm.call(f, &[])
    });
    assert_call_limit(vm, result, report, "creating a string");
    assert_eq!(calls.get(), 0, "a pending refusal must prevent dispatch without an entry policy");

    let module = vm.new_module(id!(driver));
    vm.add_method(module, id_lut!(dispatch), script_args_def!(), move |vm, _| {
        drain_free_objects(vm);
        let heap = vm.heap_mut();
        heap.reconcile_heap_bytes();
        heap.set_max_heap_bytes(Some(heap.accounted_heap_bytes() + object));
        vm.call_back(f, &[1.into()], [NIL; 4]).unwrap_or(NIL)
    });
    let value = vm.eval(script("active_callback", "mod.driver.dispatch()"));
    assert!(value.is_err(), "the enclosing interpreter must bail on a refused callback argument");
    assert!(vm.heap_mut().take_heap_limit_exceeded(), "the callback argument must reach the retained cap");
    let errors = vm.take_error_records();
    assert_eq!(errors.len(), 1, "one refusal must escape the callback");
    assert!(errors[0].message().contains("script heap allocation limit exceeded while binding function arguments"),
        "the callback must report the argument binding: {errors:?}");
    assert_eq!(calls.get(), 0, "a callback must receive every argument before dispatch");
    vm.heap_mut().set_max_heap_bytes(None);
    assert_eq!(vm.call(f, &args), 42.into(), "a later call with both arguments must succeed");
    assert_eq!(calls.get(), 1, "only the successful call must reach the native");

    // With a policy the host entry can reject the call before scope construction.
    let mut policy = ScriptVmPolicy::default();
    policy.string_bytes = Some(4);
    vm.bx.set_policy(policy).expect("install the string limit");
    vm.capture_errors();
    vm.heap_mut().new_string_from_str("too long");
    assert!(vm.call(f, &args).is_err(), "policy admission must return the pending refusal");
    assert_eq!(vm.bx.poisoned(), Some(&Poison::Limit(Limit::StringBytes)), "admission must retain the limit kind");
    let errors = vm.take_error_records();
    assert_eq!(errors.len(), 1, "the pending host refusal must be delivered at admission");
    assert!(errors[0].message().contains("script string allocation limit exceeded"), "admission must deliver the string refusal");
    assert_eq!(calls.get(), 1, "policy admission must suppress the refused native");
}

#[test]
fn sparse_array_index_is_refused_before_resize() {
    let (report, errors) = eval_limited(
        "sparse_array",
        "fn run(){ let values = [] values[1000000000] = 1 42 }\nrun()",
        64 * 1024,
    );

    assert!(report.exceeded, "report={report:?}, errors={errors:#?}");
    assert!(
        errors
            .iter()
            .any(|error| error.contains("script allocation limit exceeded")),
        "{errors:#?}"
    );
}

#[test]
fn sparse_object_index_is_refused_before_resize() {
    let (report, errors) = eval_limited(
        "sparse_object",
        "fn run(){ let values = {} values[1000000000] = 1 42 }\nrun()",
        64 * 1024,
    );

    assert!(report.exceeded, "report={report:?}, errors={errors:#?}");
    assert!(errors.iter().any(|error| error.contains("sparse object index")));
}

#[test]
fn repeated_concat_stops_at_the_cumulative_string_budget() {
    let mut code = "let text = \"abcdefgh\"\n".to_string();
    for _ in 0..20 {
        code.push_str("text = text + text\n");
    }
    code.push_str("text");

    let (report, errors) = eval_limited("concat", &code, 32 * 1024);

    assert!(report.exceeded);
    assert!(errors.iter().any(|error| error.contains("concatenating strings")));
}

#[test]
fn raw_string_builder_fails_closed_only_while_budgeted() {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let mut called = false;
    let (_, report) = vm.with_heap_allocation_limit(1024, |vm| {
        let _ = vm.bx.heap.new_string_with(|_, out| {
            called = true;
            out.push_str("unbounded");
        });
    });
    let errors = vm.take_errors();
    assert!(!called, "sandboxed raw builder ran before it was preflighted");
    assert!(report.exceeded);
    assert!(errors.iter().any(|error| error.contains("without an allocation preflight")));

    let value = vm
        .bx
        .heap
        .new_string_with(|_, out| out.push_str("trusted builder unchanged"));
    let text = vm
        .bx
        .heap
        .string_with(value, |_, value| value.to_owned());
    assert_eq!(text.as_deref(), Some("trusted builder unchanged"));
}

#[test]
fn native_refusal_before_callback_or_pop_to_me_bails_immediately() {
    const BUDGET: usize = 4096;
    let vm = &mut test_vm();
    vm.bx.bail_on_uncaught_error = true;
    let probe = vm.new_module(id!(probe));
    vm.add_method(probe, id_lut!(reserve), script_args_def!(pause = NIL), |vm, args| {
        let pause = vm.value(args, id!(pause)).as_bool() == Some(true);
        let array = vm.bx.heap.new_array();
        vm.bx.heap.set_array_index(array, BUDGET, NIL, NoTrap);
        if pause { vm.pause(); }
        NIL
    });
    vm.add_method(probe, id_lut!(visit), script_args_def!(cb = NIL), |vm, args| {
        let callback = vm.value(args, id!(cb));
        vm.bx.heap.new_string_from_str("0123456789");
        vm.call_back(callback, &[1.into()], [NIL; 4]).unwrap_or(NIL)
    });
    let seen = vm.bx.heap.new_array();
    vm.set_injected_global(id!(seen), seen.into());

    for (name, source, line, expected) in [
        ("pop", "use mod.pod.*\nfn() {\n    vec2(mod.probe.reserve(false), 0)\n}", 3,
            "script allocation limit exceeded while growing a sparse array index"),
        ("pause", "fn() {\n    mod.probe.reserve(true)\n}", 2,
            "script allocation limit exceeded while growing a sparse array index"),
        ("callback", "fn() {\n    let cb = fn(x) { seen.push(x) }\n    mod.probe.visit(cb)\n}", 3,
            "script string allocation limit exceeded while creating a string"),
    ] {
        vm.bx.captured_errors = Some(Vec::new());
        let f = vm.eval(ScriptMod::new("", "", name, 1, 0, source, &[]));
        assert!(f.as_object().is_some(), "{name}: closure must compile before enabling the cap");
        let result = if name == "callback" {
            vm.bx.heap.set_max_string_bytes(Some(8));
            let result = vm.call(f, &[]);
            assert!(vm.bx.heap.take_string_limit_exceeded(), "the callback native must reach the string limit");
            result
        } else {
            let (result, report) = vm.with_heap_allocation_limit(BUDGET, |vm| vm.call(f, &[]));
            assert!(report.exceeded, "{name}: the native must reach the allocation budget");
            result
        };
        let errors = vm.take_error_records();
        assert!(result.is_err(), "{name}: the native refusal must bail: {result:?}, {errors:?}");
        assert!(!vm.is_paused(), "{name}: the refusal must unwind the call without pausing");
        assert_eq!(errors.len(), 1, "{name}: only the refusal should be reported: {errors:?}");
        assert!(errors[0].message().contains(expected), "{name}: wrong refusal: {errors:?}");
        assert_eq!(errors[0].view().line, line, "{name}: the refusal belongs to the native call");
    }
    assert_eq!(vm.bx.heap.array_len(seen), 0, "the pending callback must never run");
}
