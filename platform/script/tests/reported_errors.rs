use makepad_script::*;

#[test]
fn a_reported_error_is_reported_once_however_far_it_travels() {
    // Marking changes provenance only, including at the ends of the error range
    // and for the sentinel. The queue and the caller receive identical bits.
    let trap = ScriptTrapInner::default();
    let ip = ScriptIp { body: 7, index: 123 };
    for unmarked in [
        ScriptValue::script_err_not_found(ip),
        ScriptValue::script_err_limit(ip),
        ScriptValue::NOT_FOUND,
    ] {
        assert!(unmarked.is_err() && !unmarked.is_reported_err());
        let marked = trap.push_err(unmarked, "original failure", file!(), line!());
        let errors = trap.err_take();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].value.raw(), marked.raw());
        assert!(marked.is_err() && marked.is_reported_err());
        assert!(!marked.is_id() && !marked.is_handle() && !marked.is_f64());
        assert_eq!(marked.as_err(), unmarked.as_err(), "type and ip survive marking");
        assert_eq!(marked.value_type(), unmarked.value_type());
        assert_ne!(marked, NIL);
    }
    for ordinary in [NIL, TRUE, 1.5.into(), (-2.0).into(), f64::NAN.into(), id!(key).into()] {
        assert!(!ordinary.is_err() && !ordinary.is_reported_err());
    }

    let host = Box::leak(Box::new(ScriptVmHost::new((), ())));
    let mut vm = ScriptVm {
        host: host.host_context(),
        bx: Box::new(ScriptVmBase::new()),
    };
    vm.bx.captured_errors = Some(Vec::new());
    let module = vm.new_module(id!(error_flow));
    let result = vm.eval(ScriptMod::new("", "", "reported_error.splash", 0, 0, r#"
        use mod.error_flow
        fn pass(value) { value }
        let source = {}
        let r = source.missing_origin
        let field = r.A
        let method = field.missing_method(1, named: 2)
        let called = method(3, named: 4)
        let indexed = called[0]
        let holder = {saved: indexed}
        let looked_up = holder.saved()
        error_flow.values = [r, field, method, called, indexed, looked_up]
        pass(looked_up).B.missing_again()()[1]
    "#, &[]));
    // The interpreter drains its trap after each instruction. Its captured
    // records count every queued diagnostic, without the console's deduplication.
    let errors = vm.take_error_records();
    assert_eq!(errors.len(), 1, "one missing property: {errors:?}");
    assert!(errors[0].message.contains("missing_origin"), "{errors:?}");
    assert!(result.is_reported_err());
    let original = result.as_err().unwrap();
    assert_eq!(original.ty, ScriptValueType::ERR_NOT_FOUND);
    let loc = vm.bx.code.ip_to_loc(original.ip).unwrap();
    let reported_loc = errors[0].loc.as_ref().unwrap();
    assert_eq!((loc.file, loc.line, loc.col), (reported_loc.file.clone(), reported_loc.line, reported_loc.col));
    let values = vm.heap().value(module, id!(values).into(), NoTrap).as_array().unwrap();
    assert_eq!(vm.heap().array_len(values), 6);
    for index in 0..6 {
        assert_eq!(vm.heap().array_index(values, index, NoTrap).raw(), result.raw(), "read {index} preserves the entire error");
    }

    // A silent host probe can escape both through a native return and through
    // a stored field. Neither route acquires a mark until a failing script use.
    let empty = vm.new_object();
    let unmarked = vm.heap().value(empty, id!(absent).into(), NoTrap);
    assert!(unmarked.is_err() && !unmarked.is_reported_err());
    assert!(vm.take_errors().is_empty(), "NoTrap must stay silent");
    vm.set_value_def(module, id!(probe_value), unmarked);
    vm.add_method(module, id!(probe), script_args!(), move |_, _| unmarked);
    for (expression, diagnostic) in [
        ("error_flow.probe().A", "no getter for field"),
        ("error_flow.probe_value.missing_method()", "method"),
        ("error_flow.probe()()", "call target is not a function"),
        ("error_flow.probe_value[0]", "cannot index"),
    ] {
        let source = format!(r#"
            use mod.error_flow
            fn pass(value) {{ value }}
            let first = {expression}
            error_flow.first = first
            pass(first).B.missing_again()()[0]
        "#);
        // Taking the records removes the sink: install a fresh one for each script.
        vm.bx.captured_errors = Some(Vec::new());
        let result = vm.eval(ScriptMod::new("", "", "unreported_error.splash", 0, 0, &source, &[]));
        let errors = vm.take_error_records();
        assert_eq!(errors.len(), 1, "{expression}: {errors:?}");
        assert!(errors[0].message.contains(diagnostic), "{expression}: {errors:?}");
        assert!(result.is_reported_err(), "{expression}");
        let first = vm.heap().value(module, id!(first).into(), NoTrap);
        assert_eq!(result.raw(), first.raw(), "{expression}: later reads preserve the first report");
        assert!(!unmarked.is_reported_err(), "reporting a use does not mutate its source");
    }
}
