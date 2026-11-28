use makepad_script::*;

fn source<'a>(file: &'a str, code: &'a str) -> ScriptModView<'a> {
    ScriptModView { file, code, line: 5, column: 2, ..ScriptModView::empty() }
}

#[test]
fn admission_preserves_the_installed_program_and_bounds_native_callbacks() {
    let mut host = ScriptVmHost::new((), ());
    let mut vm = ScriptVm { host: host.host_context(), bx: Box::new(ScriptVmBase::new()) };
    let mut body = ScriptEvalReq::new(EVAL_SOURCE, source("live.splash", "41"));
    assert_eq!(vm.vm_eval(&mut body).as_number(), Some(41.0), "trusted source uses the existing evaluator");
    let count = vm.bx.body_count();
    let original_id = body.body;
    let admission = AdmissionLimits { source_bytes: 500_000, tokens: 128, parser_states: 24, parse_work: 2_000_000 };
    let limits = EvalLimits { admission, ..EvalLimits::DOC_LOAD };
    // Build each 200,000-level input once, in blocks, even on a JIT whose
    // str::repeat appends its operand in a loop.
    let nested = [("(", ")"), ("[", "]"), ("{", "}")].map(|(open, close)| {
        format!("{}1{}", open.repeat(200).repeat(1000), close.repeat(200).repeat(1000))
    });
    for mode in [EVAL_SOURCE, EVAL_APPEND] {
        let mut check = |text: &str, limits: EvalLimits, why: &str| {
            let module = source("live.splash", if mode == EVAL_APPEND { "" } else { text });
            let mut req = ScriptEvalReq::new(mode, module);
            req.code = text;
            let (result, report) = vm.eval_bounded(&limits, |vm| vm.vm_eval(&mut req));
            assert!(result.is_err(), "refused source must return an error ({why})");
            assert!(report.limit().is_some_and(|m| m.contains(why)), "expected {why}, got {report:?}");
            let records = vm.take_error_records();
            assert_eq!(records.len(), 1, "a refused evaluation reports one diagnostic ({why})");
            let loc = records[0].loc().expect("admission diagnostic has a source location");
            assert_eq!(loc.file, "live.splash", "diagnostic names the refused source");
            assert!(loc.line >= 5, "diagnostic preserves the source offset");
            if why == "source too large to admit" {
                assert_eq!((loc.line, loc.col), (5, 2), "size is checked at entry, before lexing");
            }
            assert_eq!(vm.bx.body_count(), count, "refusal must not retire or install a body ({why})");
            assert_eq!(vm.bx.body_module_read(original_id as usize).view().code, "41", "refusal preserves the original body ({why})");
        };
        check("[[[", EvalLimits { admission: AdmissionLimits { source_bytes: 2, tokens: 0, ..admission }, ..limits }, "source too large to admit");
        for text in &nested {
            check(text, limits, "token count exceeded");
            check(&text[200_000 - 32..200_001 + 32], limits, "nesting deeper");
        }
        check("1 + 2 + 3", EvalLimits { admission: AdmissionLimits { parse_work: 24, ..admission }, ..limits }, "total parse work");
    }

    let append = |vm: &mut ScriptVm, code: &str| {
        let mut req = ScriptEvalReq::new(EVAL_APPEND, source("append.splash", ""));
        req.code = code;
        vm.vm_eval(&mut req)
    };
    let (prefix, _) = vm.eval_bounded(&limits, |vm| append(vm, "21"));
    assert_eq!(prefix.as_number(), Some(21.0), "streaming accepts a provisional token");
    let (refusal, _) = vm.eval_bounded(&limits, |vm| append(vm, &"[".repeat(140)));
    assert!(refusal.is_err(), "an unadmitted replacement fails");
    assert_eq!(append(&mut vm, "21 * 2").as_number(), Some(42.0), "the retained checkpoint continues on the trusted path");

    // EVAL_ADD admits through the normal host entry and leaves execution out of the measurement.
    let mut admits = |text: &str, credits: usize| {
        let mut req = ScriptEvalReq::new(EVAL_ADD, source("work.splash", text));
        let limits = EvalLimits { admission: AdmissionLimits { parse_work: credits, ..AdmissionLimits::UNLIMITED }, ..limits };
        let (value, report) = vm.eval_bounded(&limits, |vm| vm.vm_eval(&mut req));
        vm.take_error_records();
        if value.is_err() {
            assert!(report.limit().is_some_and(|e| e.contains("total parse work")), "the work meter must explain refusal: {report:?}");
        }
        !value.is_err()
    };
    for count in [1_000, 20_000] {
        assert!(admits(&"1;".repeat(count), 32 * count + 32),
            "{count} flat statements must fit the same number of credits per statement");
    }
    // Each new local searches the previously collected slot names. Holding the
    // per-binding allowance fixed distinguishes these real searches from flat emission.
    for (count, fits) in [(96, true), (384, false)] {
        use std::fmt::Write;
        let mut function = String::from("fn locals() {");
        for i in 0..count { writeln!(function, "let local{i:03} = 0").unwrap(); }
        function.push('}');
        assert_eq!(admits(&function, 400 * count), fits, "work must include searches of the {count} local bindings");
    }

    let callbacks = "fn visit(n) { if n > 0 { [n - 1].map(visit) }\nreturn 1 }\ntry visit(5) catch 99\n;";
    let run = |vm: &mut ScriptVm| vm.eval_with_source_view(&source("callbacks.splash", callbacks), ScriptObject::ZERO);
    assert_eq!(run(&mut vm).as_number(), Some(1.0), "the ordinary finite map chain finishes under the default");
    // Map callbacks use script call frames. The call-frame limit bounds their nesting.
    let limits = EvalLimits { native_depth: 3, ..EvalLimits::DOC_LOAD };
    assert_eq!(vm.eval_bounded(&limits, run).0.as_number(), Some(1.0), "nested maps spend no native re-entry depth");

    let probe = vm.new_module(id!(probe));
    vm.add_method(probe, id_lut!(invoke), script_args_def!(cb = NIL, n = NIL), |vm, args| {
        let cb = vm.value(args, id!(cb));
        let n = vm.value(args, id!(n));
        let _ = vm.call(cb, &[n]);
        NIL
    });
    let synchronous = "fn visit(n) { if n > 0 { mod.probe.invoke(visit, n - 1) }\nreturn 1 }\ntry visit(5) catch 99\n;";
    let (result, report) = vm.eval_bounded(&limits, |vm| {
        vm.eval_with_source_view(&source("synchronous.splash", synchronous), ScriptObject::ZERO)
    });
    assert!(result.is_err(), "catch and a native caller cannot swallow the re-entry bail");
    assert!(report.limit().is_some_and(|e| e.contains("native re-entry limit")), "report identifies the failed guard: {report:?}");
    let fresh = vm.eval_with_source_view(&source("fresh.splash", "6 * 7"), ScriptObject::ZERO);
    assert_eq!(fresh.as_number(), Some(42.0), "a fresh program runs after the callback chain unwinds");
}
