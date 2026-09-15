use makepad_script::*;

fn test_vm() -> ScriptVm<'static> {
    let host = Box::leak(Box::new(ScriptVmHost::new(0i32, ())));
    ScriptVm {
        host,
        bx: Box::new(ScriptVmBase::new()),
    }
}

fn script(name: &str, code: &str) -> ScriptMod {
    ScriptMod {
        cargo_manifest_path: String::new(),
        module_path: String::new(),
        file: format!("allocation_limit_{name}"),
        line: 0,
        column: 0,
        code: code.to_string(),
        values: vec![],
    }
}

fn eval_limited(name: &str, code: &str, limit: usize) -> (ScriptAllocationReport, Vec<String>) {
    let vm = &mut test_vm();
    vm.bx.captured_errors = Some(Vec::new());
    let (_, report) = vm.with_heap_allocation_limit(limit, |vm| vm.eval(script(name, code)));
    (report, vm.take_errors())
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
