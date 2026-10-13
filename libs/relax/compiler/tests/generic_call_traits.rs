use relax_compiler::{program::DefId, tcx::{EvalCx, EvalState, Tcx, TypingEnv}, typeck};

fn check_fixture(file: &str, reject: bool) {
    let session = std::sync::Arc::new(relax_compiler::Session::default());
    let path = format!("{}/tests/borrowck/{file}.rs", env!("CARGO_MANIFEST_DIR"));
    let (prog, _, _) = relax_compiler::load_with_std(&session, &path, file, true).unwrap();
    let mut tcx = Tcx::new();
    tcx.collect(&prog);

    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let state = EvalState::new();
    let cx = EvalCx::new(&prog, &tcx, TypingEnv::fully_monomorphized(&prog), &state);
    let main = (0..prog.defs.len()).map(|i| DefId(i as u32)).find(|&d| prog.name(d) == "main").unwrap();
    let body = typeck::instantiate_body(&cx, &prog, &tcx, main, &[]);
    assert!(body.errors.is_empty(), "{:?}", body.errors);
    let mut checked = 0;
    for (e, args) in &body.res_args {
        let Some(typeck::Res::Def(d)) = body.res.get(e) else { continue };
        if !matches!(prog.name(*d), "once" | "field" | "deref" | "shared" | "bad") { continue; }
        let instance = typeck::instantiate_body(&cx, &prog, &tcx, *d, args);
        assert!(instance.errors.is_empty(), "{}: {:?}", prog.name(*d), instance.errors);
        if prog.name(*d) == "field" {
            assert_eq!(instance.min_captures.values().next().unwrap().kind, relax_compiler::upvar::ClosureKind::FnOnce);
        }
        let verdict = relax_compiler::borrowck::check_body(&cx, &prog, &tcx, &instance);
        assert!(verdict.unchecked.is_empty(), "{}: {:?}", prog.name(*d), verdict.unchecked);
        if reject {
            assert!(verdict.errors.iter().any(|e| e.code == "E0596"), "{}: {:?}", prog.name(*d), verdict.errors);
        } else {
            assert!(verdict.errors.is_empty(), "{}: {:?}", prog.name(*d), verdict.errors);
        }
        checked += 1;
    }
    assert_eq!(checked, if reject { 2 } else { 6 });
    // Exercise the actual std body from the live failure with both closure kinds.
    let mut std_checked = 0;
    for method in body.methods.values().filter(|m| prog.name(m.def) == "get_or_init") {
        let parent = prog.def(method.def).parent;
        let initialize = (0..prog.defs.len()).map(|i| DefId(i as u32))
            .find(|&d| prog.def(d).parent == parent && prog.name(d) == "initialize").unwrap();
        let instance = typeck::instantiate_body(&cx, &prog, &tcx, initialize, &method.args);
        assert!(instance.errors.is_empty(), "{:?}", instance.errors);
        let verdict = relax_compiler::borrowck::check_body(&cx, &prog, &tcx, &instance);
        assert!(verdict.unchecked.is_empty(), "{:?}", verdict.unchecked);
        assert!(verdict.errors.is_empty(), "{:?}", verdict.errors);
        std_checked += 1;
    }
    assert_eq!(std_checked, if reject { 0 } else { 2 });
}

#[test]
fn generic_once_instances() { check_fixture("generic_call_once_accept", false); }
#[test]
fn generic_mut_instances_reject() { check_fixture("generic_call_mut_reject", true); }
