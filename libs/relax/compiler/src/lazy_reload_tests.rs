use std::sync::Arc;
use crate::program::Program;

#[test]
fn deferred_reexport_signatures() {
    // Forced selection and signature-miss collection are process-global. Each
    // case gets a fresh process, as do the runtime fixtures below.
    const CHILD: &str = "MAKEPAD_RUN_REEXPORT_SIGNATURE_TEST";
    let Ok(case) = std::env::var(CHILD) else {
        for case in ["plain", "partial"] {
            let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::deferred_reexport_signatures", "--nocapture", "--test-threads=1"])
                .env(CHILD, case)
                .env("MAKEPAD_RUN_LAZY_DEFER", "lazy_reexport/child.rs,lazy_reexport/other.rs"));
            assert!(output.status.success(), "{case}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        return;
    };
    let dir = std::env::temp_dir().join(format!("relax-reexport-signatures-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("lazy_reexport")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname=\"lazy_reexport\"\nversion=\"0.1.0\"\nedition=\"2021\"\n[lib]\npath=\"main.rs\"\n").unwrap();
    let source = include_str!("../tests/lazy_reexport.rs").to_string();
    let child = if case == "plain" { "pub(super) use crate::other::T;\n" }
        else { include_str!("../tests/lazy_reexport/child.rs") };
    std::fs::write(dir.join("lazy_reexport/child.rs"), child).unwrap();
    let other = "lazy_reexport/other.rs";
    std::fs::write(dir.join(other), include_str!("../tests/lazy_reexport/other.rs")).unwrap();
    let root = dir.join("main.rs").to_string_lossy().into_owned();
    std::fs::write(&root, source).unwrap();
    let session = Arc::new(crate::Session::default());
    let cfg = crate::cfg::CfgSet::host(&session, &[]);
    let mut prog = Program::new(&session);
    prog.preload(vec![crate::preload::PreCrate {
        name: "lazy_reexport".into(), root: root.clone(), edition: 2021, cfg: cfg.clone(),
        deps: Vec::new(),  proc_macro: false,
        image_deps: Vec::new(), image_dep_names: Vec::new(),
    }]);
    let krate = prog.load_crate("lazy_reexport", &root, 2021, cfg, Vec::new()).unwrap();
    prog.resolve_imports();
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    assert_eq!(prog.skipped_files.len(), 2);
    // The first miss loads the re-exporting child. The next must still reach the
    // target after the parent's import has found T in the value namespace.
    for suffix in ["lazy_reexport/child.rs", other] {
        crate::program::collect_misses_begin();
        let mut tcx = crate::tcx::Tcx::new();
        tcx.collect(&prog);
        let misses = crate::program::collect_misses_take();
        assert!(tcx.errors.iter().any(|e| e.contains("E0412")), "{:?}", tcx.errors);
        let modules = prog.misses_to_load(&misses);
        assert_eq!(modules.len(), 1, "{misses:?}");
        assert!(prog.skipped_files[&modules[0]].0.ends_with(suffix));
        prog.load_deferred_module(modules[0]).unwrap();
    }
    crate::program::collect_misses_begin();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    assert!(crate::program::collect_misses_take().is_empty());
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    assert!(prog.skipped_files.is_empty());
    let module = prog.crates[krate as usize].root_mod;
    let name = prog.syms.get("T").unwrap();
    let ty = prog.lookup_in_scope(module, name, true).unwrap();
    assert_eq!(prog.def_path(ty), "lazy_reexport::other::T");
    if case != "plain" {
        let value = prog.lookup_in_scope(module, name, false).unwrap();
        assert_eq!(prog.def_path(value), "lazy_reexport::fallback::T");
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn gandalf_live_edit_guard_and_reporting() {
    // Unit/runtime and diagnostic sinks are process-global; each guard setting gets a
    // fresh runtime, and the parent observes the actual console and JSON streams.
    const CHILD: &str = "MAKEPAD_RUN_GANDALF_GATE_TEST";
    let Ok(case) = std::env::var(CHILD) else {
        for case in ["app", "framework", "all", "std", "std_all", "lazy"] {
            let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::gandalf_live_edit_guard_and_reporting", "--nocapture", "--test-threads=1", "--format=terse"])
                .env(CHILD, case).env("MAKEPAD_RUN_GANDALF_ALL", if case.ends_with("all") { "1" } else { "0" }));
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{case}: {stdout}\n{stderr}");
            if case == "lazy" {
                // D3: without --check-first the patch is installed and the verdict is reported as soon as the
                // background check has it (Gandalf's console line), never as a refusal
                assert!(stderr.lines().any(|l| l.contains("borrow check failed: ") && l.contains("::value at ") && !l.ends_with("(live edit refused)")), "{case}: {stderr}");
                continue;
            }
            let reported = stderr.lines().any(|l| l.starts_with("borrow check failed: ") && l.contains("::value at ") && l.ends_with("(live edit refused)"));
            assert_eq!(reported, !matches!(case, "framework" | "std"), "{case}: {stderr}");
            let diagnostics: Vec<_> = stdout.lines().filter_map(|l| crate::json::parse(l.as_bytes()).ok())
                .filter(|j| j.get("phase").is_some_and(|p| p.str() == "borrowck")).collect();
            assert_eq!(!diagnostics.is_empty(), reported, "{case}: {stdout}");
            for d in diagnostics {
                assert!(d.get("line").unwrap().num() > 0.0);
                assert!(d.get("col").unwrap().num() > 0.0);
                // the event's own diagnostic: its code, owner and the refusal note
                assert!(!d.get("code").unwrap().str().is_empty());
                assert!(d.get("item").unwrap().str().ends_with("::value"));
                assert!(d.get("notes").unwrap().arr().iter().any(|n| n.str().starts_with("live edit refused in ") && n.str().ends_with("::value")));
            }
        }
        return;
    };
    if case != "lazy" { crate::borrowck::lazy::disable_gandalf(); }
    // `makepad run --check-first` refuses a bad live edit synchronously; the default never blocks the patch
    crate::jit::CHECK_FIRST.store(case != "lazy", std::sync::atomic::Ordering::Relaxed);
    crate::diag::enable();
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-gandalf-gate-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    let dependency = if case.starts_with("std") { "std" } else { "widgets" };
    std::fs::create_dir_all(dir.join(dependency).join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), format!("[workspace]\nmembers = [\"app\", \"{dependency}\"]\n")).unwrap();
    for name in ["app", dependency] {
        std::fs::write(dir.join(name).join("Cargo.toml"), format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")).unwrap();
        std::fs::write(dir.join(name).join("src/lib.rs"), "pub fn value(out: &mut u32) { *out = 11; }\n").unwrap();
    }
    let log = dir.join("live.jsonl");
    std::env::set_var("MAKEPAD_RUN_LIVE_LOG", &log);
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let framework = dir.join(dependency).join("src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (mut prog, core, app_crate) = crate::load_with_std(&session, &app, "gate_app", false).unwrap();
    let cfg = crate::cfg::CfgSet::host(&session, &[]);
    let framework_crate = prog.load_crate(dependency, &framework, 2021, cfg, vec![]).unwrap();
    let graph = format!(r#"{{"units":[{{"pkg_id":"path+file://{}/app#app@0.1.0","target":{{"name":"app","src_path":{}}}}},{{"pkg_id":"path+file://{}/{dependency}#{dependency}@0.1.0","target":{{"name":"{dependency}","src_path":{}}}}}]}}"#,
        dir.display(), crate::diag::quote(&app), dir.display(), crate::diag::quote(&framework));
    let roots = crate::reload::gandalf_app_roots(&crate::unitgraph::Units::from_json(graph.as_bytes()).unwrap(), dir.join("app").to_str().unwrap());
    assert_eq!(roots, vec![crate::vfs::normalize_path(&app)]);
    let target = if case == "app" || case == "lazy" { app_crate } else { framework_crate };
    let def = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == target && prog.name(id) == "value").then_some(id)
    }).unwrap();
    let path = if case == "app" || case == "lazy" { app } else { framework };
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    crate::borrowck::event::set_app_scope(roots);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let entry = crate::owner::call(crate::owner::Class::Program, || {
        let id = u.fn_id(crate::jit::FnKey::Inst(def, vec![]));
        u.entry(id)
    });
    let run = |expected| {
        let mut result = 0u32;
        // call_jit returns a recovery status, so observe the function's output via its arg.
        unsafe { crate::jit::call_jit(up, entry, &mut result as *mut u32 as u64) }
            .unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        assert_eq!(result, expected);
    };
    run(11);
    let old_file = u.prog.def(def).file;
    let bad = b"pub fn value(out: &mut u32) { let mut x = 1u32; let r = &x; x = 2; *out = *r; }\n";
    let console = (case == "lazy").then(|| crate::gandalf_out::Console::start(false, false));
    let outcome = unsafe { crate::reload::apply(up, vec![crate::reload::Edit { file: old_file, path: path.clone(), src: bad.to_vec() }]) };
    if case == "lazy" {
        assert_eq!(outcome.rung, crate::reload::Rung::Patch, "{:?}", outcome.lines);
        assert!(!outcome.lines.iter().any(|l| l.starts_with("refused by the borrow checker")));
        // the worker acknowledges a settle request between checks: the second one is answered after the queued check ran
        crate::borrowck::settle().recv().unwrap();
        crate::borrowck::settle().recv().unwrap();
        assert!(console.unwrap().finish() >= 1, "the background check reported the borrow error");
    } else if matches!(case.as_str(), "framework" | "std") {
        assert_eq!(outcome.rung, crate::reload::Rung::Patch, "{:?}", outcome.lines);
        assert_ne!(u.prog.def(def).file, old_file);
        assert!(!log.exists(), "guarded-out bodies must not emit Gandalf events");
    } else {
        assert_eq!(outcome.rung, crate::reload::Rung::Refuse, "{:?}", outcome.lines);
        assert!(outcome.lines.iter().any(|l| l.starts_with("refused by the borrow checker")));
        assert_eq!(u.prog.def(def).file, old_file);
        run(11); // Refusal kept the installed code running.
        let events = std::fs::read_to_string(&log).unwrap();
        for line in events.lines() {
            let event = crate::json::parse(line.as_bytes()).unwrap();
            assert_eq!(event.get("event").unwrap().str(), "gandalf");
            assert_eq!(event.get("origin").unwrap().str(), "LiveEdit");
            let rendered = event.get("rendered").unwrap().str();
            assert!(outcome.lines.iter().any(|l| l == rendered), "the live log carries the outcome's exact rendering");
        }
    }
    let fixed = unsafe { crate::reload::apply(up, vec![crate::reload::Edit {
        file: u.prog.def(def).file, path, src: b"pub fn value(out: &mut u32) { *out = 23; }\n".to_vec(),
    }]) };
    assert_eq!(fixed.rung, crate::reload::Rung::Patch, "{:?}", fixed.lines);
    run(23);
    // Like settle_tests: process exit owns the runtime worker/executable-memory teardown.
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A panic in the background checker stays in that check and is reported; the query it left
/// unfinished is not poisoned, so the app's compile of the same body runs that query itself. If
/// that panics too, it is this compile's own failure with its own message, and the app runs on.
/// (One Windows run: the check of a Drop body panicked, the app's first call of that body failed
/// with `typeck query: Poisoned`, parked, and the run's restart failed.)
#[test]
fn a_background_check_panic_leaves_the_query_to_the_apps_compile() {
    // The checker's worker and its panic counters are process-global: each case has its process.
    const CHILD: &str = "MAKEPAD_RUN_CHECK_PANIC_TEST";
    let Ok(case) = std::env::var(CHILD) else {
        for (case, panics_in) in [("check", "typeck_panic_probe:gandalf"), ("both", "typeck_panic_probe")] {
            // (its own home: the injected panics are no record for this machine's ledger; the body's
            // name is the panic's only key, and the JIT running this test checks its own fns by it
            // too when its code cache is off: a name no fn of the compiler has, unlike `target`)
            let home = std::env::temp_dir().join(format!("relax-check-panic-home-{}-{case}", std::process::id()));
            let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::a_background_check_panic_leaves_the_query_to_the_apps_compile", "--nocapture", "--test-threads=1"])
                .env(CHILD, case).env("MAKEPAD_RUN_TEST_TYPECK_PANIC", panics_in).env("MAKEPAD_RUN_PANIC_PARK", "0").env("MAKEPAD_HOME", &home));
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{case}: {}\n{stderr}", String::from_utf8_lossy(&output.stdout));
            std::fs::remove_dir_all(&home).unwrap();
            // the check's panic is on the run's log when it happens
            assert!(stderr.contains("thread 'gandalf'") && stderr.contains("forced panic type-checking `typeck_panic_probe`"), "{case}: {stderr}");
        }
        return;
    };
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-check-panic-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"check_panic\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n").unwrap();
    let root = dir.join("lib.rs").to_string_lossy().into_owned();
    std::fs::write(&root, "pub fn typeck_panic_probe(out: &mut u32) { *out = 11; }\npub fn other(out: &mut u32) { *out = 3; }\n").unwrap();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &root, "check_panic", false).unwrap();
    let named = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let (target, other) = (named("typeck_panic_probe"), named("other"));
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    crate::borrowck::event::set_app_scope(vec![crate::vfs::normalize_path(&root)]);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    // The check gets there first, as discovery does for a body the app has not called yet.
    let (target_entry, other_entry) = crate::owner::call(crate::owner::Class::Program, || {
        let (t, o) = (u.fn_id(crate::jit::FnKey::Inst(target, vec![])), u.fn_id(crate::jit::FnKey::Inst(other, vec![])));
        u.note_first_borrowck(t, None);
        (u.entry(t), u.entry(o))
    });
    let epoch = u.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst);
    // no wall-clock limit (as lazy_tests): a worker that never delivers hangs the test
    let why = loop {
        match crate::borrowck::lazy::verdict(u.front.checker_id, target, &[], epoch) {
            crate::borrowck::lazy::FnVerdict::Unchecked(why) if why.starts_with("checker failure") => break why,
            _ => std::thread::sleep(std::time::Duration::from_millis(5)),
        }
    };
    assert!(why.contains("forced panic type-checking `typeck_panic_probe`"), "{why}");
    assert_eq!(crate::borrowck::lazy::PANICS.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(crate::borrowck::lazy::PANIC_NOTES.lock().unwrap()[0].contains("typeck_panic_probe"));
    // devhub's ledger gets it, naming the body and what panicked, with no hot load counted
    let records: Vec<_> = std::fs::read_dir(makepad_remote::hot::outbox()).unwrap().map(|f| std::fs::read_to_string(f.unwrap().path()).unwrap()).collect();
    assert_eq!(records.len(), 1, "{records:?}");
    let record = makepad_remote::hot::Record::parse(&records[0]).unwrap();
    assert_eq!(record.outcome, makepad_remote::hot::Outcome::CheckerPanic);
    assert_eq!(record.reason, "checker panic in `check_panic::typeck_panic_probe`: MAKEPAD_RUN_TEST_TYPECK_PANIC: forced panic type-checking `typeck_panic_probe`");
    assert!(!record.outcome.failure());
    // call_jit returns a recovery status, so the function's output comes back via its argument.
    let call = |entry| {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, entry, &mut result as *mut u32 as u64) }.map(|_| result)
    };
    if case == "check" {
        let result = call(target_entry).unwrap_or_else(|p| panic!("the app's compile of the checked body failed: {}", p.message));
        assert_eq!(result, 11);
    } else {
        let failed = call(target_entry).expect_err("the provider panics on the app's thread as well");
        assert!(failed.message.contains("internal compiler error") && failed.message.contains("forced panic type-checking `typeck_panic_probe`"), "{}", failed.message);
    }
    assert_eq!(call(other_entry).unwrap_or_else(|p| panic!("another body after it: {}", p.message)), 3);
    // Like settle_tests: process exit owns the runtime worker/executable-memory teardown.
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// SPEC-V: a patch runs before its verdict as baseline code with `noalias` dropped; a rejected one
/// is never promoted, is reported and reverts; a clean one is promoted with its `noalias` facts.
#[test]
fn specv_patch_before_verdict() {
    const CHILD: &str = "MAKEPAD_RUN_SPECV_TEST";
    let Ok(case) = std::env::var(CHILD) else {
        for case in ["bad", "clean"] {
            let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::specv_patch_before_verdict", "--nocapture", "--test-threads=1", "--format=terse"])
                .env(CHILD, case).env("MAKEPAD_RUN_GANDALF_ALL", "0"));
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{case}: {stdout}\n{stderr}");
            if case == "bad" {
                assert!(stderr.lines().any(|l| l.contains("REVERTED") && l.contains("::value")), "{case}: {stderr}");
            }
        }
        return;
    };
    use std::sync::atomic::Ordering;
    crate::jit::CHECK_FIRST.store(false, Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-specv-{}-{}", case, std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("app/src/lib.rs"), "pub fn value(out: &mut u32) { *out = 11; }\n").unwrap();
    let log = dir.join("live.jsonl");
    std::env::set_var("MAKEPAD_RUN_LIVE_LOG", &log);
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "specv_app", false).unwrap();
    let graph = format!(r#"{{"units":[{{"pkg_id":"path+file://{}/app#app@0.1.0","target":{{"name":"app","src_path":{}}}}}]}}"#, dir.display(), crate::diag::quote(&app));
    let roots = crate::reload::gandalf_app_roots(&crate::unitgraph::Units::from_json(graph.as_bytes()).unwrap(), dir.join("app").to_str().unwrap());
    let def = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == "value").then_some(id)
    }).unwrap();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    crate::borrowck::event::set_app_scope(roots);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let (id, entry) = crate::owner::call(crate::owner::Class::Program, || {
        let id = u.fn_id(crate::jit::FnKey::Inst(def, vec![]));
        (id, u.entry(id))
    });
    let run = |expected| {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, entry, &mut result as *mut u32 as u64) }
            .unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        assert_eq!(result, expected);
    };
    run(11);
    let text: &[u8] = if case == "bad" {
        b"pub fn value(out: &mut u32) { let mut x = 1u32; let r = &x; x = 2; *out = *r; }\n"
    } else {
        b"pub fn value(out: &mut u32) { *out = 23; }\n"
    };
    let old_file = u.prog.def(def).file;
    let outcome = unsafe { crate::reload::apply(up, vec![crate::reload::Edit { file: old_file, path: app.clone(), src: text.to_vec() }]) };
    assert_eq!(outcome.rung, crate::reload::Rung::Patch, "{:?}", outcome.lines);
    // the patch applied at once, before any verdict: baseline code, no noalias, flagged unverified
    {
        let f = &u.fns[id as usize];
        assert!(f.unverified.load(Ordering::Relaxed), "the patch ran without a clean verdict");
        assert_eq!(f.patch_gen, outcome.group);
    }
    run(if case == "bad" { 2 } else { 23 });
    // the RIR of the patched body is built on demand (the first run built the code, not necessarily kept RIR)
    unsafe { crate::jit::build_rir(up, id) }.unwrap();
    {
        let r = u.fns[id as usize].rir.as_ref().expect("RIR");
        assert!(r.baseline && r.noalias.is_empty() && !r.withheld_noalias.is_empty(), "unverified: baseline, noalias dropped (baseline {}, noalias {})", r.baseline, r.noalias.len());
    }
    if case == "bad" {
        assert!(!unsafe { crate::jit::compile_tier1(up, id) }, "no O3 build of a body without a clean verdict");
    }
    let mut undos = vec![outcome.undo.unwrap()];
    let ids = vec![id];
    let mut pending = vec![crate::reload::PendingPatch { group: outcome.group, what: "lib.rs".into(), ids, event_seq: crate::inspect::events::last_seq() }];
    crate::reload::UNVERIFIED_PATCHES.fetch_add(1, Ordering::AcqRel);
    assert!(!crate::reload::observations_verified());
    // the worker acknowledges a settle request between checks: the second one is answered after the queued check ran
    crate::borrowck::settle().recv().unwrap();
    crate::borrowck::settle().recv().unwrap();
    let t0 = std::time::Instant::now();
    while !pending.is_empty() && t0.elapsed().as_secs() < 20 {
        crate::reload::settle_verdicts(up, &mut pending, &mut undos);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(pending.is_empty(), "the verdict arrived");
    assert!(crate::reload::observations_verified());
    if case == "bad" {
        assert!(undos.is_empty(), "the failed patch group was rolled back");
        run(11); // the previous code runs again
        assert_eq!(u.fns[id as usize].patch_gen, 0);
        let events = std::fs::read_to_string(&log).unwrap();
        assert!(events.lines().any(|l| l.contains("patch_reverted") && l.contains("::value")), "{events}");
    } else {
        run(23);
        let flagged = u.fns[id as usize].unverified.load(Ordering::Relaxed);
        assert!(u.refresh_verified(id), "a clean verdict verifies the slot");
        // a clear outside sweep_verdicts goes to the next sweep (what waited for the verdict runs)
        assert!(!flagged || crate::jit::verified_since_sweep(&u, id), "cleared outside a sweep: handed to the next one");
        // promotion: the normal RIR O3 starts from keeps the `&mut` parameter's noalias
        unsafe { crate::jit::build_normal(up, id) }.unwrap();
        let r = u.fns[id as usize].rir.as_ref().expect("RIR");
        assert!(!r.baseline && !r.noalias.is_empty(), "verified: promoted with its noalias facts (baseline {}, noalias {})", r.baseline, r.noalias.len());
    }
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// SPEC-V: an unverified body that breaks the borrow rules (`callee(&mut x, &mut x)`) calls a verified
/// callee compiled with `noalias`: the call goes to the callee's unverified-mode twin, so the result is the
/// machine's (2), not the one the `noalias` code computes (1, as the same call made from verified code
/// shows).
#[test]
fn specv_aliased_call_from_unverified_body() {
    const CHILD: &str = "MAKEPAD_RUN_SPECV_ALIAS_TEST";
    if std::env::var_os(CHILD).is_none() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::specv_aliased_call_from_unverified_body", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    use std::sync::atomic::Ordering;
    crate::jit::CHECK_FIRST.store(false, Ordering::Relaxed);
    // `through_raw` must have its clean verdict before its first run: the checker's whole-app discovery
    // (`--ahead=aot-max`; the levels below aot check only the bodies that ran)
    crate::jit::set_ahead_level(crate::options::AheadLevel::AotMax);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-specv-alias-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let original = "pub fn callee(a: &mut u32, b: &mut u32) -> u32 { *a = 1; *b = 2; *a }\n\
        pub fn through_raw(out: &mut u32) { let mut x = 0u32; let p = &mut x as *mut u32; *out = callee(unsafe { &mut *p }, unsafe { &mut *p }); }\n\
        pub fn value(out: &mut u32) { *out = 11; }\n\
        pub fn early(out: &mut u32) { let mut x = 0u32; let p = &mut x as *mut u32; *out = callee(unsafe { &mut *p }, unsafe { &mut *p }); }\n\
        pub fn value_ptr(out: &mut u32) { *out = 11; }\n\
        pub trait Go { fn go(&self, a: &mut u32, b: &mut u32) -> u32; }\n\
        pub struct S;\n\
        impl Go for S { fn go(&self, a: &mut u32, b: &mut u32) -> u32 { *a = 1; *b = 2; *a } }\n\
        pub fn value_dyn(out: &mut u32) { *out = 11; }\n";
    std::fs::write(dir.join("app/src/lib.rs"), original).unwrap();
    std::env::set_var("MAKEPAD_RUN_LIVE_LOG", dir.join("live.jsonl"));
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "specv_alias", false).unwrap();
    let graph = format!(r#"{{"units":[{{"pkg_id":"path+file://{}/app#app@0.1.0","target":{{"name":"app","src_path":{}}}}}]}}"#, dir.display(), crate::diag::quote(&app));
    let roots = crate::reload::gandalf_app_roots(&crate::unitgraph::Units::from_json(graph.as_bytes()).unwrap(), dir.join("app").to_str().unwrap());
    let find = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let (callee_def, raw_def, value_def, early_def) = (find("callee"), find("through_raw"), find("value"), find("early"));
    let (ptr_def, dyn_def) = (find("value_ptr"), find("value_dyn"));
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    crate::borrowck::event::set_app_scope(roots);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    crate::syms::set_unit(up);
    let ids = crate::owner::call(crate::owner::Class::Program, || {
        let c = u.fn_id(crate::jit::FnKey::Inst(callee_def, vec![]));
        let r = u.fn_id(crate::jit::FnKey::Inst(raw_def, vec![]));
        let v = u.fn_id(crate::jit::FnKey::Inst(value_def, vec![]));
        let e = u.fn_id(crate::jit::FnKey::Inst(early_def, vec![]));
        (c, r, v, u.entry(r), u.entry(v), e, u.entry(e))
    });
    let (callee, raw, value, raw_entry, value_entry, early, early_entry) = ids;
    let (value_ptr, value_dyn) = crate::owner::call(crate::owner::Class::Program, || {
        (u.fn_id(crate::jit::FnKey::Inst(ptr_def, vec![])), u.fn_id(crate::jit::FnKey::Inst(dyn_def, vec![])))
    });
    let (ptr_entry, dyn_entry) = crate::owner::call(crate::owner::Class::Program, || (u.entry(value_ptr), u.entry(value_dyn)));
    let run = |label: &str, entry, expected| {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, entry, &mut result as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        assert_eq!(result, expected, "{label}");
    };
    // the checker queues the program at the first compile; wait until the originals are verified.
    // `early` runs before its verdict: unverified code, its aliasing call goes to the twin (2)
    run("value", value_entry, 11);
    run("early", early_entry, 2);
    assert!(u.fns[early as usize].rir.as_ref().is_some_and(|r| r.plain_calls), "the unverified lowering calls the twin");
    let t0 = std::time::Instant::now();
    loop {
        crate::borrowck::settle().recv().unwrap();
        if [callee, raw, value, early, value_ptr, value_dyn].iter().all(|&i| u.verdict_clean(i)) { break; }
        assert!(t0.elapsed().as_secs() < 20, "the originals never got a clean verdict");
    }
    // verified now: its promotion lowers it again, calling `callee` itself (no twin calls left)
    assert!(u.refresh_verified(early));
    unsafe { crate::jit::build_normal(up, early) }.unwrap();
    {
        let r = u.fns[early as usize].rir.as_ref().expect("RIR");
        assert!(!r.plain_calls && !r.baseline, "verified: lowered again without twin calls");
        let twin = u.plain_twin_of(callee).expect("the twin of callee");
        assert!(!r.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, relax_backend::rir::Inst::Call(relax_backend::rir::Callee::Fn(t), _, _) if *t == twin))));
    }
    // verified code: `callee` is compiled with noalias, and a call that aliases (through raw pointers, so
    // the checker accepts it) gets the answer the noalias code computes
    run("raw", raw_entry, 1);
    assert!(!u.fns[callee as usize].unverified.load(Ordering::Relaxed));
    // an unverified body that aliases the same way, safely written (a borrow error)
    let patched = "pub fn callee(a: &mut u32, b: &mut u32) -> u32 { *a = 1; *b = 2; *a }\n\
        pub fn through_raw(out: &mut u32) { let mut x = 0u32; let p = &mut x as *mut u32; *out = callee(unsafe { &mut *p }, unsafe { &mut *p }); }\n\
        pub fn value(out: &mut u32) { let mut x = 0u32; *out = callee(&mut x, &mut x); }\n\
        pub fn value_ptr(out: &mut u32) { let f: fn(&mut u32, &mut u32) -> u32 = std::hint::black_box(callee); let mut x = 0u32; *out = f(&mut x, &mut x); }\n\
        pub trait Go { fn go(&self, a: &mut u32, b: &mut u32) -> u32; }\n\
        pub struct S;\n\
        impl Go for S { fn go(&self, a: &mut u32, b: &mut u32) -> u32 { *a = 1; *b = 2; *a } }\n\
        pub fn value_dyn(out: &mut u32) { let s: &dyn Go = std::hint::black_box(&S); let mut x = 0u32; *out = s.go(&mut x, &mut x); }\n";
    let file = u.prog.def(value_def).file;
    let outcome = unsafe { crate::reload::apply(up, vec![crate::reload::Edit { file, path: app.clone(), src: patched.as_bytes().to_vec() }]) };
    assert_eq!(outcome.rung, crate::reload::Rung::Patch, "{:?}", outcome.lines);
    assert!(u.fns[value as usize].unverified.load(Ordering::Relaxed));
    run("value", value_entry, 2);
    run("ptr", ptr_entry, 2);
    run("dyn", dyn_entry, 2);
    let twin = u.plain_twin_of(callee).expect("the aliasing call went to the callee's twin");
    assert!(u.fns[twin as usize].compiled);
    // the twin tiers up (O3, built without noalias): the aliasing call still gets the machine's answer
    assert!(unsafe { crate::jit::compile_tier1(up, twin) }, "the twin gets O3 code");
    assert!(u.fns[twin as usize].o3.load(Ordering::Relaxed) && u.fns[twin as usize].unverified.load(Ordering::Relaxed));
    run("value", value_entry, 2);
    // ... but the patched body itself, awaiting its verdict, does not
    assert!(!unsafe { crate::jit::compile_tier1(up, value) }, "no O3 build of a patch without its verdict");
    // the verdict rejects the patch: it reverts
    let mut undos = vec![outcome.undo.unwrap()];
    let mut pending = vec![crate::reload::PendingPatch { group: outcome.group, what: "lib.rs".into(), ids: vec![value, value_ptr, value_dyn], event_seq: crate::inspect::events::last_seq() }];
    crate::reload::UNVERIFIED_PATCHES.fetch_add(1, Ordering::AcqRel);
    let t0 = std::time::Instant::now();
    while !pending.is_empty() && t0.elapsed().as_secs() < 20 {
        crate::reload::settle_verdicts(up, &mut pending, &mut undos);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(pending.is_empty() && undos.is_empty());
    run("value", value_entry, 11);
    run("ptr", ptr_entry, 11);
    run("dyn", dyn_entry, 11);
    // the twin is never promoted
    assert!(u.fns[twin as usize].unverified.load(Ordering::Relaxed));
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// SPEC-V (lane SPECHITCH): a caller that ran as unverified code (its aliasing-prone calls went to
/// the callees' twins) and is rebuilt as verified code once its verdict is clean calls the base
/// slots, which never had code. Before: its next call compiled them lazily on the calling thread
/// (the Sandbox's frame path: ~550 lazy installs in one 70-560 ms frame). Now they run their twins'
/// code until their own builds (background) are installed: no lazy compile on the program thread.
/// The verdicts are held (SPECV_HOLD) so the first run is unverified whatever the checker's speed;
#[test]
fn specv_verified_rebuild_runs_twin_code_not_lazy_compiles() {
    const CHILD: &str = "MAKEPAD_RUN_SPECV_BORROW_TEST";
    let Ok(case) = std::env::var(CHILD) else {
        for case in ["borrow"] {
            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            cmd.args(["--exact", "lazy_reload_tests::specv_verified_rebuild_runs_twin_code_not_lazy_compiles", "--nocapture", "--test-threads=1", "--format=terse"])
                .env(CHILD, case).env("MAKEPAD_RUN_GANDALF_ALL", "0");
            let output = crate::test_child::output(&mut cmd);
            assert!(output.status.success(), "{case}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        return;
    };
    use std::sync::atomic::Ordering;
    crate::jit::CHECK_FIRST.store(false, Ordering::Relaxed);
    crate::jit::set_ahead_level(crate::options::AheadLevel::AotMax);
    crate::jit::SPECV_HOLD.store(1, Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-specv-borrow-{}-{}", case, std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let src = "#[inline(never)] pub fn leaf(a: &mut u32, b: &u32) -> u32 { *a += *b; *a }\n\
        #[inline(never)] pub fn mid(a: &mut u32, b: &u32) -> u32 { leaf(a, b) + leaf(a, b) }\n\
        pub fn root(out: &mut u32) { let mut x = 1u32; let y = 2u32; *out = mid(&mut x, &y); }\n";
    std::fs::write(dir.join("app/src/lib.rs"), src).unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "specv_borrow", false).unwrap();
    let graph = format!(r#"{{"units":[{{"pkg_id":"path+file://{}/app#app@0.1.0","target":{{"name":"app","src_path":{}}}}}]}}"#, dir.display(), crate::diag::quote(&app));
    let roots = crate::reload::gandalf_app_roots(&crate::unitgraph::Units::from_json(graph.as_bytes()).unwrap(), dir.join("app").to_str().unwrap());
    let find = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let (leaf_def, mid_def, root_def) = (find("leaf"), find("mid"), find("root"));
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    crate::borrowck::event::set_app_scope(roots);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    crate::syms::set_unit(up);
    let (leaf, mid, root, root_entry) = crate::owner::call(crate::owner::Class::Program, || {
        let l = u.fn_id(crate::jit::FnKey::Inst(leaf_def, vec![]));
        let m = u.fn_id(crate::jit::FnKey::Inst(mid_def, vec![]));
        let r = u.fn_id(crate::jit::FnKey::Inst(root_def, vec![]));
        (l, m, r, u.entry(r))
    });
    let run = |label: &str| {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, root_entry, &mut result as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        assert_eq!(result, 8, "{label}");
    };
    // held verdicts: `root` runs as unverified code, its call goes to mid's twin (and that one's to leaf's)
    run("unverified");
    assert!(u.fns[root as usize].rir.as_ref().is_some_and(|r| r.plain_calls), "the unverified lowering calls the twin");
    let twin = u.plain_twin_of(mid).expect("mid's twin");
    assert!(u.fns[twin as usize].compiled && !u.fns[mid as usize].compiled, "the twin ran, the base never did");
    // the release: one verdict change for every body
    crate::jit::specv_release();
    let t0 = std::time::Instant::now();
    loop {
        crate::borrowck::settle().recv().unwrap();
        if [leaf, mid, root].iter().all(|&i| u.verdict_clean(i)) { break; }
        assert!(t0.elapsed().as_secs() < 20, "no clean verdicts");
    }
    assert!(u.refresh_verified(root));
    // the verified rebuild (what sweep_verdicts queues: retier at O3), installed: it calls the base slots
    assert!(unsafe { crate::jit::compile_tier1(up, root) }, "root gets verified O3 code");
    {
        let r = u.fns[root as usize].rir.as_ref().expect("RIR");
        assert!(!r.plain_calls, "verified: lowered again without twin calls");
        assert!(r.blocks.iter().any(|b| b.insts.iter().any(|i| matches!(i, relax_backend::rir::Inst::Call(relax_backend::rir::Callee::Fn(t), _, _) if *t == mid))), "it calls mid itself");
    }
    let lazy0 = crate::jit::LAZY_FRONTS.load(Ordering::Relaxed);
    run("verified");
    let lazy = crate::jit::LAZY_FRONTS.load(Ordering::Relaxed) - lazy0;
    assert_eq!(lazy, 0, "no lazy compile on the calling thread: mid runs its twin's code");
    assert!(u.fns[mid as usize].borrowed || u.fns[mid as usize].compiled, "mid borrows its twin's code (or got its own build)");
    // a patch-free rerun stays on code
    run("again");
    assert_eq!(crate::jit::LAZY_FRONTS.load(Ordering::Relaxed) - lazy0, 0);
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// SPEC-V (lane JITLIVEO3): a twin runs in its base's place until the base's verdict is clean, so
/// the base's own record and counters saw none of that run. The twin gets a counted build (its
/// unverified baseline code is its code for good), and the base's O3 build takes the twin's counts,
/// mapped onto its own blocks by source position. Before, a base reached O3 with static
/// frequencies (the twin's heat thrown away) or waited for counts of its own.
#[test]
fn specv_base_o3_takes_its_twins_counts() {
    const CHILD: &str = "MAKEPAD_RUN_SPECV_TWIN_PGO_TEST";
    let Ok(case) = std::env::var(CHILD) else {
        for case in ["twinpgo"] {
            let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
            cmd.args(["--exact", "lazy_reload_tests::specv_base_o3_takes_its_twins_counts", "--nocapture", "--test-threads=1", "--format=terse"])
                .env(CHILD, case).env("MAKEPAD_RUN_GANDALF_ALL", "0");
            let output = crate::test_child::output(&mut cmd);
            assert!(output.status.success(), "{case}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        return;
    };
    use std::sync::atomic::Ordering;
    crate::jit::CHECK_FIRST.store(false, Ordering::Relaxed);
    crate::jit::set_ahead_level(crate::options::AheadLevel::AotMax);
    crate::jit::SPECV_HOLD.store(1, Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-specv-twinpgo-{}-{}", case, std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    // mid's loop runs 12 times a call, its `leaf` arm 4 of them
    let src = "#[inline(never)] pub fn leaf(a: &mut u32, b: &u32) -> u32 { *a += *b; *a }\n\
        #[inline(never)] pub fn mid(a: &mut u32, b: &u32) -> u32 { let mut s = 0u32; let mut i = 0u32; while i < 12 { if i % 3 == 0 { s = s.wrapping_add(leaf(a, b)); } else { s = s.wrapping_add(i); } i += 1; } s }\n\
        pub fn root(out: &mut u32) { let mut x = 1u32; let y = 2u32; *out = mid(&mut x, &y); }\n";
    std::fs::write(dir.join("app/src/lib.rs"), src).unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "specv_twinpgo", false).unwrap();
    let graph = format!(r#"{{"units":[{{"pkg_id":"path+file://{}/app#app@0.1.0","target":{{"name":"app","src_path":{}}}}}]}}"#, dir.display(), crate::diag::quote(&app));
    let roots = crate::reload::gandalf_app_roots(&crate::unitgraph::Units::from_json(graph.as_bytes()).unwrap(), dir.join("app").to_str().unwrap());
    let find = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let (leaf_def, mid_def, root_def) = (find("leaf"), find("mid"), find("root"));
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    crate::borrowck::event::set_app_scope(roots);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    crate::syms::set_unit(up);
    let (leaf, mid, root, root_entry) = crate::owner::call(crate::owner::Class::Program, || {
        let l = u.fn_id(crate::jit::FnKey::Inst(leaf_def, vec![]));
        let m = u.fn_id(crate::jit::FnKey::Inst(mid_def, vec![]));
        let r = u.fn_id(crate::jit::FnKey::Inst(root_def, vec![]));
        (l, m, r, u.entry(r))
    });
    // x = 1 + 2k at the k-th leaf call: s = 3 + 5 + 7 + 9 + (1 + 2 + 4 + 5 + 7 + 8 + 10 + 11)
    let run = || {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, root_entry, &mut result as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        assert_eq!(result, 72);
    };
    // held verdicts: root's call runs mid's twin
    run();
    let twin = u.plain_twin_of(mid).expect("mid's twin");
    assert!(u.fns[twin as usize].compiled && !u.fns[mid as usize].compiled, "the twin ran, the base never did");
    // the twin's counted build: its unverified code is the twin's code for good
    assert!(unsafe { crate::jit::counted_build(up, twin, false) }, "the twin gets a counted build");
    const RUNS: u32 = 5;
    for _ in 0..RUNS {
        run();
    }
    // the release: mid's verdict is clean, its own (verified) RIR differs from the twin's
    crate::jit::specv_release();
    let t0 = std::time::Instant::now();
    loop {
        crate::borrowck::settle().recv().unwrap();
        if [leaf, mid, root].iter().all(|&i| u.verdict_clean(i)) { break; }
        assert!(t0.elapsed().as_secs() < 20, "no clean verdicts");
    }
    assert!(u.refresh_verified(mid));
    unsafe { crate::jit::build_normal(up, mid) }.unwrap();
    let r = u.fns[mid as usize].rir.clone().expect("mid's RIR");
    assert!(!r.baseline, "verified: mid's own RIR");
    // its O3 input gets the twin's counts: the loop's blocks ran 12 per call, the leaf arm 4
    let counts = u.pgo_counts_for(mid, &r).expect("the base's O3 build takes its twin's counts");
    let top = counts.iter().copied().fold(0.0f32, f32::max);
    assert!(top >= (12 * RUNS) as f32, "the loop's counts reach the base: {counts:?}\n{}", r.dump());
    assert!(counts.iter().any(|&c| c >= (4 * RUNS) as f32 && c < (12 * RUNS) as f32), "the leaf arm ran a third of the loop: {counts:?}\n{}", r.dump());
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Relaxed Rust D3: a pair of index borrows the checker accepts only with a run-time check
/// (`&mut v[i]`, `&mut v[j]`). Before the verdict the body runs as written (SPEC-V: unverified);
/// once it is clean, the verified code is lowered with the check: an overlap panics as
/// `get_disjoint_mut(..).unwrap()` does, distinct indices run as before. Without the relaxed feature
/// the verdict is an error and no check is lowered.
#[test]
fn d3_disjoint_check_lowered_from_the_verdict() {
    const CHILD: &str = "MAKEPAD_RUN_D3_TEST";
    let Ok(case) = std::env::var(CHILD) else {
        for case in ["relaxed", "strict", "check-first"] {
            let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::d3_disjoint_check_lowered_from_the_verdict", "--nocapture", "--test-threads=1", "--format=terse"])
                .env(CHILD, case).env("MAKEPAD_RUN_GANDALF_ALL", "0"));
            assert!(output.status.success(), "{case}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        return;
    };
    use std::sync::atomic::Ordering;
    // `makepad run --check-first`: the check runs before the first lowering, so the first code has the check
    crate::jit::CHECK_FIRST.store(case == "check-first", Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-d3-{}-{}", case, std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    // io = [i, j, result]
    // io = [i, j, result]; `pair` indexes an array (built-in), `pair_vec` a Vec (IndexMut)
    std::fs::write(dir.join("app/src/lib.rs"), "pub fn pair(io: &mut [usize; 3]) {\n    let (i, j) = (io[0], io[1]);\n    let mut v = [1usize, 2, 3];\n    let a = &mut v[i];\n    let b = &mut v[j];\n    *a += 10;\n    *b += 100;\n    io[2] = v[0] + v[1] + v[2];\n}\n\
        pub fn pair_vec(io: &mut [usize; 3]) {\n    let (i, j) = (io[0], io[1]);\n    let mut v = vec![1usize, 2, 3];\n    let a = &mut v[i];\n    let b = &mut v[j];\n    *a += 10;\n    *b += 100;\n    io[2] = v[0] + v[1] + v[2];\n}\n").unwrap();
    std::env::set_var("MAKEPAD_RUN_LIVE_LOG", dir.join("live.jsonl"));
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let mut session = crate::Session::default();
    session.options.relaxed = Some(if case == "strict" { "none" } else { "disjoint-index" }.to_string());
    let session = Arc::new(session);
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "d3_app", false).unwrap();
    let graph = format!(r#"{{"units":[{{"pkg_id":"path+file://{}/app#app@0.1.0","target":{{"name":"app","src_path":{}}}}}]}}"#, dir.display(), crate::diag::quote(&app));
    let roots = crate::reload::gandalf_app_roots(&crate::unitgraph::Units::from_json(graph.as_bytes()).unwrap(), dir.join("app").to_str().unwrap());
    let find = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let defs = [find("pair"), find("pair_vec")];
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    crate::borrowck::event::set_app_scope(roots);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    for def in defs {
    let (id, entry) = crate::owner::call(crate::owner::Class::Program, || {
        let id = u.fn_id(crate::jit::FnKey::Inst(def, vec![]));
        (id, u.entry(id))
    });
    let run = |i: usize, j: usize| -> Result<usize, String> {
        let mut io = [i, j, 0usize];
        unsafe { crate::jit::call_jit(up, entry, &mut io as *mut [usize; 3] as u64) }.map(|_| io[2]).map_err(|p| p.message)
    };
    if case == "check-first" {
        assert_eq!(run(0, 1), Ok(116));
        assert!(u.fns[id as usize].d3_lowered.load(Ordering::Relaxed), "checked before the first lowering");
        assert!(run(1, 1).expect_err("the first code checks").contains("OverlappingIndices"));
        continue;
    }
    // unverified: the code runs as written (an overlap writes the one element twice). (The program's
    // other fns are checked in the background meanwhile: a later fn may have its verdict before
    // its first call, and then its first code is verified code.)
    assert_eq!(run(0, 1), Ok(116));
    if !u.fns[id as usize].d3_lowered.load(Ordering::Relaxed) {
        assert_eq!(run(1, 1), Ok(116));
    }
    let t0 = std::time::Instant::now();
    loop {
        crate::borrowck::settle().recv().unwrap();
        if !matches!(u.verdict_state(id), crate::jit::Verdict::Pending) { break; }
        assert!(t0.elapsed().as_secs() < 20, "no verdict");
    }
    if case == "strict" {
        assert!(matches!(u.verdict_state(id), crate::jit::Verdict::Failed(_)), "strict: NLL rejects the pair");
        assert!(u.disjoint_checks(id).is_none());
        // SAFE1 (JITSAFE 11/12): first-run code the checker rejected never reaches O3
        assert!(!u.o3_allowed(id), "a rejected body is not O3-eligible");
    } else {
        assert!(u.verdict_clean(id), "relaxed: accepted with a run-time check");
        let checks = u.disjoint_checks(id).expect("the verdict asks for the check");
        assert_eq!(checks.len(), 1);
        assert!(u.refresh_verified(id));
        // the verified code: lowered again with the check, and installed
        let _ = run(0, 1);
        assert!(unsafe { crate::jit::compile_tier1(up, id) }, "O3 build of the verified body");
        assert!(u.fns[id as usize].d3_lowered.load(Ordering::Relaxed), "the verified code was lowered with the verdict's checks");
        assert_eq!(run(0, 1), Ok(116));
        assert_eq!(run(2, 0), Ok(116));
        let overlap = run(1, 1).expect_err("an overlap panics");
        assert!(overlap.contains("OverlappingIndices"), "{overlap}");
    }
    }
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn gandalf_scope_uses_package_source_and_project_boundaries() {
    let tree = "/virtual/gandalf-project";
    let entries = [
        ("path+file://", "app", "apps/app/src/main.rs", true),
        ("path+file://", "helper", "shared/helper/src/lib.rs", true),
        ("registry+https://registry", "vendored", "vendor/vendored/src/lib.rs", false),
        ("git+https://host/repo", "gitdep", "vendor/gitdep/src/lib.rs", false),
        ("path+file://", "core", "toolchain/core/src/lib.rs", false),
        ("path+file://", "std", "toolchain/std/src/lib.rs", false),
        ("path+file://", "alloc", "toolchain/alloc/src/lib.rs", false),
        ("path+file://", "platform", "platform/src/lib.rs", false),
        ("path+file://", "draw", "draw/src/lib.rs", false),
        ("path+file://", "widgets", "widgets/core/src/lib.rs", false),
        ("path+file://", "math", "libs/math/src/lib.rs", false),
        ("path+file://", "relax_base", "libs/relax/base/src/lib.rs", false),
        ("path+file://", "engine", "apps/commercial/engine/motion/src/lib.rs", false),
        ("path+file://", "external", "../external/src/lib.rs", false),
        ("path+file://", "prefix", "../gandalf-project-extra/src/lib.rs", false),
        ("path+file://", "app_lib", "apps/app/libs/helper/src/lib.rs", true),
    ];
    let units: Vec<_> = entries.iter().map(|(source, name, path, _)| format!(
        r#"{{"pkg_id":"{source}#pkg@1.0","target":{{"name":"{name}","src_path":"{tree}/{path}"}}}}"#)).collect();
    let graph = format!(r#"{{"units":[{}]}}"#, units.join(","));
    let selected = crate::reload::gandalf_app_roots(&crate::unitgraph::Units::from_json(graph.as_bytes()).unwrap(), tree);
    let expected: Vec<_> = entries.iter().filter(|e| e.3).map(|e| format!("{tree}/{}", e.2)).collect();
    assert_eq!(selected, expected);
}


#[test]
fn app_scope_live_layout_edits() {
    const CHILD: &str = "MAKEPAD_RUN_APP_LAYOUT_TEST";
    let Ok(case) = std::env::var(CHILD) else {
        for case in ["app_lib", "path_crate", "framework", "embedded", "generic_embedded", "pointer", "bound_template", "bound_template_embedded", "enums"] {
            let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::app_scope_live_layout_edits", "--nocapture", "--test-threads=1"])
                .env(CHILD, case));
            assert!(output.status.success(), "{case}: {}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        }
        return;
    };
    use crate::{jit::{self, FnKey}, program::DefId, reload::{self, Edit, Rung}, types::{TyKind, RegionArgsId}};
    crate::borrowck::lazy::disable_gandalf();
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-app-layout-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(if case == "path_crate" { "helper/lib.rs" } else { "lib.rs" });
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let base = r#"
        pub struct State { pub value: u64 }
        pub struct Dormant { pub n: u32 }
        pub fn init(out: &mut State) { *out = State { value: 41 }; }
        pub fn value(out: &mut State) { out.value += 1; }
        pub fn name(out: &mut &str) { *out = std::any::type_name::<State>(); }
        pub const SIZE: u64 = std::mem::size_of::<State>() as u64;
        pub fn size(out: &mut [u64; 2]) { let s = &std::mem::size_of::<State>(); *out = [*s as u64, SIZE]; }
        pub struct Measured { pub n: u32 }
        static MEASURED: usize = std::mem::size_of::<Measured>();
        pub fn measured(out: &mut u64) { *out = MEASURED as u64; }
        pub fn adder(n: u64) -> impl Fn(u64) -> u64 { move |x| x + n }
        pub fn add_dyn(out: &mut u64) { let f = adder(2); let d: &dyn Fn(u64) -> u64 = &f; *out = d(*out); }
        pub struct Kept { pub n: u32 }
        static KEPT: std::sync::Mutex<Vec<Kept>> = std::sync::Mutex::new(Vec::new());
        pub fn keep(out: &mut u64) { let mut k = KEPT.lock().unwrap(); k.push(Kept { n: 5 }); *out = k.iter().map(|x| x.n as u64).sum(); }
        pub struct Mixed { pub a: u8, pub s: String, pub c: u16, pub d: u32 }
        pub fn mixed(out: &mut [u64; 7]) {
            let m = Mixed { a: 1, s: String::from("xy"), c: 3, d: 4 };
            *out = [std::mem::size_of::<Mixed>() as u64, std::mem::align_of::<Mixed>() as u64, std::mem::offset_of!(Mixed, a) as u64, std::mem::offset_of!(Mixed, s) as u64,
                std::mem::offset_of!(Mixed, c) as u64, std::mem::offset_of!(Mixed, d) as u64, m.a as u64 + m.s.len() as u64 + m.c as u64 + m.d as u64];
        }
        pub enum Ease { Linear, Pow(f64), Curve(f32, f32, f32, f32) }
        pub enum Yoyo { Invert, Ease(Ease) }
        pub enum Held { Nothing, Has(Yoyo) }
    "#;
    // enums whose values the program holds, for in-place variant edits (M84): E's values in a
    // Vec, in Options and in Options of Options; F's in Option<F> and in W, which keeps its own
    // variants in F's spare tag values; a niche-filled enum (Yoyo), a single-variant one, one of
    // 256 variants (the tag's full width) and a repr(u8) one. `read` folds every value into a
    // decimal digit, and the sizes rustc gives E and its Options into the last. N's values live
    // only in `transient`'s locals
    let before = if case == "enums" {
        let g: String = (0..256).map(|i| format!("V{i}, ")).collect();
        base.to_string() + &format!(r#"
        use std::sync::Mutex;
        pub enum E {{ A, B(u32), C(u8) }}
        pub enum F {{ A, B(u32) }}
        pub enum W {{ A(F), B, C }}
        pub enum One {{ Only(u32) }}
        pub enum G {{ {g} }}
        #[repr(u8)] pub enum R {{ A, B(u16), Q }}
        pub enum N {{ A(u32), B }}
        static ES: Mutex<Vec<E>> = Mutex::new(Vec::new());
        static OS: Mutex<Vec<Option<E>>> = Mutex::new(Vec::new());
        static OOS: Mutex<Vec<Option<Option<E>>>> = Mutex::new(Vec::new());
        static FS: Mutex<Vec<(Option<F>, W)>> = Mutex::new(Vec::new());
        static YS: Mutex<Vec<(Held, One, G)>> = Mutex::new(Vec::new());
        static RS: Mutex<Vec<R>> = Mutex::new(Vec::new());
        const _: () = assert!(std::mem::size_of::<E>() == 8 && std::mem::size_of::<Option<E>>() == 8 && std::mem::size_of::<Option<Option<E>>>() == 8);
        pub fn make(_: &mut u64) {{
            ES.lock().unwrap().extend([E::A, E::B(7), E::C(3)]);
            OS.lock().unwrap().extend([Some(E::C(3)), None]);
            OOS.lock().unwrap().extend([Some(None), None, Some(Some(E::B(7)))]);
            FS.lock().unwrap().extend([(Some(F::B(7)), W::A(F::A)), (None, W::C)]);
            YS.lock().unwrap().push((Held::Has(Yoyo::Ease(Ease::Pow(2.0))), One::Only(5), G::V255));
            RS.lock().unwrap().extend([R::A, R::B(9)]);
        }}
        pub fn step(_: &mut u64) {{}}
        pub fn transient(out: &mut u64) {{
            let mut n = 0u64;
            for x in [N::A(7), N::B] {{ n = n * 10 + match x {{ N::A(k) => k as u64, N::B => 5 }}; }}
            *out = n * 100 + std::mem::size_of::<N>() as u64;
        }}
        fn e(x: &E) -> u64 {{ match x {{ E::A => 1, E::B(7) => 2, E::C(3) => 3, _ => 9 }} }}
        fn r(x: &R) -> u64 {{ match x {{ R::A => 1, R::B(9) => 2, _ => 9 }} }}
        pub fn read(out: &mut u64) {{
            let mut n = 0u64;
            for x in ES.lock().unwrap().iter() {{ n = n * 10 + e(x); }}
            for x in OS.lock().unwrap().iter() {{ n = n * 10 + x.as_ref().map_or(0, e); }}
            for x in OOS.lock().unwrap().iter() {{ n = n * 10 + match x {{ Some(Some(x)) => e(x), Some(None) => 8, None => 0 }}; }}
            for (a, b) in FS.lock().unwrap().iter() {{ n = n * 10 + match (a, b) {{ (Some(F::B(7)), W::A(F::A)) => 1, (None, W::C) => 2, _ => 9 }}; }}
            for (h, o, g) in YS.lock().unwrap().iter() {{ n = n * 10 + match (h, o, g) {{ (Held::Has(Yoyo::Ease(Ease::Pow(p))), One::Only(5), G::V255) if *p == 2.0 => 1, _ => 9 }}; }}
            for x in RS.lock().unwrap().iter() {{ n = n * 10 + r(x); }}
            let rustc = std::mem::size_of::<E>() == 8 && std::mem::size_of::<Option<E>>() == 8 && std::mem::size_of::<Option<Option<E>>>() == 8;
            *out = n * 10 + if rustc {{ 1 }} else {{ 9 }};
        }}
    "#)
    } else {
        base.to_string()
    };
    let before = before.as_str();
    std::fs::write(&path, before).unwrap();
    let session = Arc::new(crate::Session::default());
    let (mut prog, core, lib) = crate::load_with_std(&session, path.to_str().unwrap(), "app_lib", false).unwrap();
    let cfg = crate::cfg::CfgSet::host(&session, &[]);
    let bin_path = dir.join("main.rs");
    std::fs::write(&bin_path, "fn main() {}\n").unwrap();
    let bin = prog.load_crate("app_bin", bin_path.to_str().unwrap(), 2021, cfg.clone(), vec![]).unwrap();
    let fw_path = dir.join("framework.rs");
    let wrapper = match case.as_str() {
        "generic_embedded" => "pub struct Holder<T> { pub value: ([T; 1], u64) }",
        "bound_template" | "bound_template_embedded" => "pub trait Project<'a> { type Out; } impl<'a> Project<'a> for u8 { type Out = u64; } pub struct Holder<T> { pub value: ([T; 1], u64) }",
        "pointer" => "pub struct Holder { pub value: *mut app_lib::State }",
        "embedded" => "pub struct Holder { pub value: app_lib::State }",
        _ => "pub struct Holder { pub value: u64 }",
    };
    std::fs::write(&fw_path, wrapper).unwrap();
    let lib_name = prog.syms.intern("app_lib");
    let framework = prog.load_crate("framework", fw_path.to_str().unwrap(), 2021, cfg, vec![(lib_name, lib)]).unwrap();
    prog.resolve_imports();
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    let def = |name: &str, krate| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = DefId(i as u32);
        (d.krate == krate && d.kind != crate::program::DefKind::Field && prog.name(id) == name).then_some(id)
    }).unwrap();
    let state = def("State", lib);
    let holder = def("Holder", framework);
    let init = def("init", lib);
    let value = def("value", lib);
    let name = def("name", lib);
    let keep = def("keep", lib);
    let mixed = def("mixed", lib);
    let size = def("size", lib);
    let measured = def("measured", lib);
    let add_dyn = def("add_dyn", lib);
    let mixed_def = def("Mixed", lib);
    let enums = [def("Ease", lib), def("Yoyo", lib), def("Held", lib)];
    let enum_fns = (case == "enums").then(|| [def("make", lib), def("step", lib), def("read", lib), def("transient", lib)]);
    let mut roots = vec![bin_path.to_string_lossy().into_owned()];
    // (the "framework" case: the edited crate is outside the app scope, which changes nothing of
    // its layouts or edits)
    if case != "framework" { roots.push(path.to_string_lossy().into_owned()); }
    crate::borrowck::event::set_app_scope(roots);
    crate::layout::set_root_crate(bin);
    crate::layout::LIVE_SESSION.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let ty = tcx.tys.intern(TyKind::Adt(state, tcx.tys.mk_list(&[]), RegionArgsId::EMPTY));
    let mixed_ty = tcx.tys.intern(TyKind::Adt(mixed_def, tcx.tys.mk_list(&[]), RegionArgsId::EMPTY));
    let enums = enums.map(|e| tcx.tys.intern(TyKind::Adt(e, tcx.tys.mk_list(&[]), RegionArgsId::EMPTY)));
    if case.starts_with("bound_template") {
        use crate::consts::GenericArg;
        use crate::types::{AliasTyKind, BoundVar, RegionKind};
        let region = tcx.tys.intern_region(RegionKind::ReBound(0, BoundVar(0)));
        let projection = tcx.alias_ty_with_args(AliasTyKind::Projection { def_id: def("Out", framework) },
            &[GenericArg::Type(tcx.tys.u8_), GenericArg::Lifetime(region)]);
        let template = tcx.tys.intern(TyKind::Adt(holder, tcx.tys.mk_list(&[projection]), RegionArgsId::EMPTY));
        assert!(tcx.tys.is_concrete(template));
        assert!(tcx.tys.has_escaping_bound_vars(template));
    }
    if matches!(case.as_str(), "embedded" | "generic_embedded" | "pointer" | "bound_template_embedded") {
        let args = if matches!(case.as_str(), "generic_embedded" | "bound_template_embedded") { vec![ty] } else { vec![] };
        tcx.tys.intern(TyKind::Adt(holder, tcx.tys.mk_list(&args), RegionArgsId::EMPTY));
    }
    let mut u = jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut jit::Unit;
    let entry = |u: &mut jit::Unit, d| {
        crate::owner::call(crate::owner::Class::Program, || {
            let id = u.fn_id(FnKey::Inst(d, vec![]));
            u.entry(id)
        })
    };
    let init = entry(&mut u, init);
    let value = entry(&mut u, value);
    let name = entry(&mut u, name);
    let keep = entry(&mut u, keep);
    let mixed = entry(&mut u, mixed);
    let size = entry(&mut u, size);
    let measured = entry(&mut u, measured);
    let add_dyn = entry(&mut u, add_dyn);
    let layout = u.layout_of(ty).clone();
    assert_eq!((layout.size, &layout.fields[..]), (8, &[0][..]), "State");
    // an enum is laid out as rustc lays it in a live session too (M84: a tagged layout with no
    // niche to give made the program's own size assert fail): Yoyo and Held fill Ease's tag niche
    let sizes = enums.map(|t| u.layout_of(t).size);
    assert_eq!(sizes, [24, 24, 24], "Ease, Yoyo, Held");
    let mut old_value = vec![0u64; (layout.size as usize + 7) / 8];
    let call = |entry, arg| unsafe { jit::call_jit(up, entry, arg) }.unwrap_or_else(|p| panic!("JIT: {}", p.message));
    // R11 (M84): a struct of the program that is not Copy is laid out as rustc lays it in a live
    // session too: its layout, and size_of, align_of and offset_of! in its code, are this test
    // binary's own for the same definition
    struct Mixed { a: u8, s: String, c: u16, d: u32 }
    let m = Mixed { a: 1, s: String::from("xy"), c: 3, d: 4 };
    let rustc = [std::mem::size_of::<Mixed>(), std::mem::align_of::<Mixed>(), std::mem::offset_of!(Mixed, a), std::mem::offset_of!(Mixed, s),
        std::mem::offset_of!(Mixed, c), std::mem::offset_of!(Mixed, d)].map(|x| x as u64);
    let mut out = [0u64; 7];
    call(mixed, out.as_mut_ptr() as u64);
    assert_eq!((&out[..6], out[6]), (&rustc[..], m.a as u64 + m.s.len() as u64 + m.c as u64 + m.d as u64), "Mixed in the JIT's code");
    let ml = u.layout_of(mixed_ty);
    let laid: Vec<u64> = [ml.size, ml.align].into_iter().chain(ml.fields.iter().copied()).map(u64::from).collect();
    assert_eq!(laid, rustc, "Mixed's layout");
    if let Some(fns) = enum_fns {
        // M84: a variant added to an enum the program holds values of applies in place when
        // rustc's layout of the new text, or that layout with the running variants' tags kept,
        // reads every value made before as it was; otherwise the edit restarts, naming the type
        let [make, step, read, transient] = fns.map(|d| entry(&mut u, d));
        let file = u.prog.def(state).file;
        let lib_path = path.to_string_lossy().into_owned();
        let apply = |src: &str| unsafe { reload::apply(up, vec![Edit { file, path: lib_path.clone(), src: src.as_bytes().to_vec() }]) };
        let run = |f| {
            let mut n = 0u64;
            call(f, &mut n as *mut u64 as u64);
            n
        };
        run(make);
        assert_eq!(run(read), 12330802121121, "the values before any edit");
        // R3, R8: a variant appended: rustc's layout reads the running values as before (the
        // Options keep their None where it was), the new one constructs and matches, and the
        // const assert of rustc's sizes in the new text holds
        let v1 = before.replace("C(u8) }", "C(u8), D(u16) }").replace("E::C(3) => 3,", "E::C(3) => 3, E::D(4) => 4,")
            .replace("pub fn step(_: &mut u64) {}", "pub fn step(_: &mut u64) { ES.lock().unwrap().push(E::D(4)); }");
        let out = apply(&v1);
        assert_eq!(out.rung, Rung::Patch, "appended: {:?}", out.lines);
        run(step);
        assert_eq!(run(read), 123430802121121, "appended");
        // R3, R4: a variant inserted before running ones: rustc's tags would move theirs, so the
        // running tags stay (pinned) and the new one takes the next free value
        let v2 = v1.replace("pub enum E { A, B(u32)", "pub enum E { A, X, B(u32)").replace("E::D(4) => 4,", "E::D(4) => 4, E::X => 5,")
            .replace("ES.lock().unwrap().push(E::D(4));", "ES.lock().unwrap().push(E::X); OS.lock().unwrap().push(Some(E::X));");
        let out = apply(&v2);
        assert_eq!(out.rung, Rung::Patch, "inserted: {:?}", out.lines);
        run(step);
        assert_eq!(run(read), 12345305802121121, "inserted");
        // R9: a repr(u8) enum's tag is its discriminant: an append applies, an insert restarts
        let v3 = v2.replace("pub enum R { A, B(u16), Q }", "pub enum R { A, B(u16), Q, C }").replace("R::B(9) => 2,", "R::B(9) => 2, R::C => 3,")
            .replace("ES.lock().unwrap().push(E::X); OS.lock().unwrap().push(Some(E::X));", "RS.lock().unwrap().push(R::C);");
        let out = apply(&v3);
        assert_eq!(out.rung, Rung::Patch, "repr(u8) appended: {:?}", out.lines);
        run(step);
        let held = 123453058021211231;
        assert_eq!(run(read), held, "repr(u8) appended");
        // R10: N's values live only in a fn's locals: a variant that makes rustc fill a niche
        // (C's char) where it had a tag takes that new layout, and the code using N is rebuilt
        assert_eq!(run(transient), 7508, "N before its edit");
        let v4 = v3.replace("pub enum N { A(u32), B }", "pub enum N { A(u32), B, C(u32, char) }").replace("[N::A(7), N::B]", "[N::A(7), N::B, N::C(3, 'x')]")
            .replace("N::B => 5 }", "N::B => 5, N::C(k, c) => k as u64 + (c == 'x') as u64 }");
        let out = apply(&v4);
        assert_eq!(out.rung, Rung::Patch, "no running value: {:?}", out.lines);
        assert!(out.lines.iter().any(|l| l.contains("enum `app_lib::N` takes its new layout")), "{:?}", out.lines);
        assert_eq!(run(transient), 75408, "N in its new layout");
        assert_eq!(run(read), held, "the running values after N's edit");
        let restarts = |src: String, what: &str, says: &[&str]| {
            let out = apply(&src);
            assert_eq!(out.rung, Rung::Restart, "{what}: {:?}", out.lines);
            assert!(out.lines.iter().any(|l| says.iter().all(|s| l.contains(s)) && l.contains("restart the app")), "{what}: {:?}", out.lines);
            assert_eq!(run(read), held, "{what}: the running values after the restart is offered");
        };
        restarts(v4.replace("pub enum R { A,", "pub enum R { Z, A,"), "repr(u8) inserted", &["variant `Z` was added", "app_lib::R", "another tag"]);
        // R6, R7: a niche-filled enum, a single-variant one, a tag of full width, a payload that
        // grows the enum: restart, and the running sizes stay rustc's
        restarts(v4.replace("pub enum Yoyo { Invert, Ease(Ease) }", "pub enum Yoyo { Invert, Ease(Ease), Back }"), "niche-filled", &["variant `Back` was added", "Yoyo"]);
        restarts(v4.replace("pub enum One { Only(u32) }", "pub enum One { Only(u32), Two }"), "single variant", &["variant `Two` was added", "would change from"]);
        restarts(v4.replace("V255, ", "V255, V256, "), "257 variants", &["variant `V256` was added", "would change from"]);
        restarts(v4.replace("C(u8), D(u16) }", "C(u8), D(u16), Big([u64; 4]) }"), "grown payload", &["variant `Big` was added", "would change from"]);
        assert_eq!(enums.map(|t| u.layout_of(t).size), [24, 24, 24], "Ease, Yoyo, Held after the restarts");
        // R5: F's spare tag values hold Option<F>'s None and W's own variants. With two variants
        // rustc's layout of a third moves them (Option<F>'s None from 2 to the top value; with
        // tags kept, above or below, one of them moves too), so the running Options and Ws would
        // be read wrong: a restart naming a type holding F, F's own values being fine
        let out = apply(&v4.replace("pub enum F { A, B(u32) }", "pub enum F { A, B(u32), C(u32) }"));
        assert_eq!(out.rung, Rung::Restart, "two-variant append: {:?}", out.lines);
        assert!(out.lines.iter().any(|l| l.contains("variant `C` was added") && l.contains("made before the edit would be read wrong") && !l.contains("values of `app_lib::F` ")),
            "two-variant append names a type holding F: {:?}", out.lines);
        assert_eq!(run(read), held, "two-variant append: the running values after the restart is offered");
        std::mem::forget(u);
        std::fs::remove_dir_all(dir).unwrap();
        return;
    }
    call(init, old_value.as_mut_ptr() as u64);
    call(value, old_value.as_mut_ptr() as u64);
    assert_eq!(old_value[layout.fields[0] as usize / 8], 42);
    let (drop_id, drop_entry) = crate::owner::call(crate::owner::Class::Program, || {
        let drop_id = u.fn_id(FnKey::Glue(crate::jit::GLUE_DROP, ty));
        (drop_id, u.entry(drop_id))
    });
    let mut empty = vec![0u64; old_value.len()];
    call(drop_entry, empty.as_mut_ptr() as u64);
    let mut actual_name = [0usize; 2];
    call(name, actual_name.as_mut_ptr() as u64);
    let actual_name = unsafe { std::slice::from_raw_parts(actual_name[0] as *const u8, actual_name[1]) };
    let actual_name = std::str::from_utf8(actual_name).unwrap();
    assert_eq!(actual_name, crate::typeck::type_name(&u.prog, &u.tcx, ty));
    // M84 step 5, R12: an edit applies in place when no field moves (a rename); one that moves
    // fields takes rustc's layout of the new text when no running value of the struct is left (the
    // test holds its values in its own memory, which no static shows), and restarts when one may be
    let file = u.prog.def(state).file;
    let lib_path = path.to_string_lossy().into_owned();
    let apply = |src: &str| unsafe { reload::apply(up, vec![Edit { file, path: lib_path.clone(), src: src.as_bytes().to_vec() }]) };
    // size_of in code whose text no edit changes (`size`): rebuilt for each new layout, and in a
    // const it reads (`SIZE`): evaluated again
    let size_now = || {
        let mut n = [0u64; 2];
        call(size, n.as_mut_ptr() as u64);
        (n[0], n[1])
    };
    assert_eq!(size_now(), (8, 8), "size_of::<State>() before the edits");
    // a trait object of a closure its fn returns (`-> impl Fn`): it holds no State, whatever the
    // closure's owner returns (that cycle once counted as a holder and restarted every struct edit)
    let mut n = 1u64;
    call(add_dyn, &mut n as *mut u64 as u64);
    assert_eq!(n, 3);
    let added = before.replace("pub value: u64 }", "pub value: u64, pub extra: u64 }")
        .replace("State { value: 41 }", "State { value: 41, extra: 0 }")
        .replace("out.value += 1;", "out.value += 1; out.extra += 7; obsolete_style();")
        // Generated child styles register their own function entry. Removing the child
        // removes this helper too; its old body must not be checked against the new names.
        + "\nfn obsolete_style() { let _entry = obsolete_style as fn(); }\n";
    let patch = apply(&added);
    assert_eq!(patch.rung, Rung::Patch, "field added: {:?}", patch.lines);
    assert!(patch.lines.iter().any(|l| l.contains("struct `app_lib::State` takes its new layout")), "{:?}", patch.lines);
    // the drop glue compiled before was rebuilt in the same transaction
    assert!(patch.names.contains(&u.fns[drop_id as usize].name.to_string()), "{:?}", patch.names);
    let grown = u.layout_of(ty).clone();
    assert_eq!((grown.size, size_now()), (16, (16, 16)), "State with its field added: its layout, and size_of in its code and its const (rebuilt: {:?})", patch.names);
    let mut fresh = vec![0u64; 2];
    call(init, fresh.as_mut_ptr() as u64);
    call(value, fresh.as_mut_ptr() as u64);
    let at = |l: &crate::layout::Layout, v: &[u64], i: usize| v[l.fields[i] as usize / 8];
    assert_eq!((at(&grown, &fresh, 0), at(&grown, &fresh, 1)), (42, 7), "a value made in the new layout");
    // a rename keeps every field in its place: the value made before reads on
    let renamed = added.replace("pub extra: u64", "pub more: u64").replace("extra: 0", "more: 0").replace("out.extra += 7", "out.more += 7");
    let patch = apply(&renamed);
    assert_eq!(patch.rung, Rung::Patch, "field renamed: {:?}", patch.lines);
    assert!(patch.lines.iter().any(|l| l.contains("field `extra` of `app_lib::State` renamed to `more` (value kept)")), "{:?}", patch.lines);
    call(value, fresh.as_mut_ptr() as u64);
    assert_eq!((at(&grown, &fresh, 0), at(&grown, &fresh, 1)), (43, 14), "the renamed field's value");
    // the field removed again: the first layout, which the value made before the edits has
    let removed = apply(before);
    assert_eq!(removed.rung, Rung::Patch, "field removed: {:?}", removed.lines);
    assert_eq!((u.layout_of(ty).size, size_now()), (8, (8, 8)), "State with its field removed: its layout, and size_of in its code and its const");
    call(value, old_value.as_mut_ptr() as u64);
    assert_eq!(old_value[layout.fields[0] as usize / 8], 43);
    call(drop_entry, old_value.as_mut_ptr() as u64);
    // a static holds values of Kept: a field added to it restarts, naming the static, and the
    // running code reads them on
    let mut sum = 0u64;
    call(keep, &mut sum as *mut u64 as u64);
    assert_eq!(sum, 5);
    let held = before.replace("pub struct Kept { pub n: u32 }", "pub struct Kept { pub n: u32, pub m: u32 }").replace("Kept { n: 5 }", "Kept { n: 5, m: 1 }");
    let restart = apply(&held);
    assert_eq!(restart.rung, Rung::Restart, "a held struct's field added: {:?}", restart.lines);
    assert!(restart.lines.iter().any(|l| l.contains("struct `Kept` changed its layout, and `app_lib::KEPT` may hold values of it in the old one") && l.contains("restart the app")),
        "{:?}", restart.lines);
    call(keep, &mut sum as *mut u64 as u64);
    assert_eq!(sum, 10, "Kept's values after the restart is offered");
    // a held field retyped to one of its size (the same bytes, read as another type) restarts too
    let retyped = before.replace("pub struct Kept { pub n: u32 }", "pub struct Kept { pub n: f32 }").replace("Kept { n: 5 }", "Kept { n: 5.0 }");
    let restart = apply(&retyped);
    assert_eq!(restart.rung, Rung::Restart, "a held struct's field retyped: {:?}", restart.lines);
    assert!(restart.lines.iter().any(|l| l.contains("struct `Kept` changed its layout, and `app_lib::KEPT` may hold values of it in the old one")), "{:?}", restart.lines);
    call(keep, &mut sum as *mut u64 as u64);
    assert_eq!(sum, 15, "Kept's values after the retype's restart is offered");
    // a static's value was computed from Measured's layout (an initializer runs once): a field
    // added to it restarts, naming the static, and the running code reads the old value on
    let mut n = 0u64;
    call(measured, &mut n as *mut u64 as u64);
    assert_eq!(n, 4);
    let restart = apply(&before.replace("pub struct Measured { pub n: u32 }", "pub struct Measured { pub n: u32, pub m: u32 }"));
    assert_eq!(restart.rung, Rung::Restart, "a struct a static's value measured: {:?}", restart.lines);
    assert!(restart.lines.iter().any(|l| l.contains("struct `Measured` changed its layout, and the value of `app_lib::MEASURED` was computed from the old one")), "{:?}", restart.lines);
    call(measured, &mut n as *mut u64 as u64);
    assert_eq!(n, 4, "MEASURED after the restart is offered");
    // no code uses Dormant: its new layout publishes with no fn rebuilt
    let dormant = before.replace("pub struct Dormant { pub n: u32 }", "pub struct Dormant { pub n: u32, pub added: bool }");
    let pure = apply(&dormant);
    assert_eq!(pure.rung, Rung::Patch, "{:?}", pure.lines);
    assert!(pure.lines.iter().any(|l| l.contains("struct `app_lib::Dormant` takes its new layout")), "{:?}", pure.lines);
    assert!(pure.names.is_empty(), "{:?}", pure.names);
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Gandalf's verdicts carry over a live edit only for owners whose crate cannot see the edited
/// files, and only when the edit keeps every field list and macro (a layout another crate sees
/// drops all verdicts, as before). Lead decision 52 (R1): an item or signature edit is scoped by
/// crate like a body edit -- a crate that cannot see it keeps everything.
#[test]
fn gandalf_carry_keeps_only_crates_that_cannot_see_the_edit() {
    use crate::reload::{classify, crates_seeing_edit, gandalf_carries, gandalf_keeps};
    let session = Arc::new(crate::Session::default());
    let cfg = crate::cfg::CfgSet::host(&session, &[]);
    let dir = std::env::temp_dir().join(format!("relax-gandalf-carry-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a_src = "pub struct S { pub x: u32 }\npub fn f() -> u32 { 1 }\n";
    let write = |name: &str, src: &str| { let p = dir.join(name); std::fs::write(&p, src).unwrap(); p.to_str().unwrap().to_string() };
    let (a_path, b_path, c_path) = (write("a.rs", a_src), write("b.rs", "pub fn g() -> u32 { 2 }\n"), write("c.rs", "pub fn h() -> u32 { 3 }\n"));
    let mut p = Program::new(&session);
    let a = p.load_crate("carry_a", &a_path, 2021, cfg.clone(), vec![]).unwrap();
    let a_sym = p.syms.intern("carry_a");
    p.load_crate("carry_b", &b_path, 2021, cfg.clone(), vec![(a_sym, a)]).unwrap();
    p.load_crate("carry_c", &c_path, 2021, cfg, vec![]).unwrap();
    p.resolve_imports();
    let file = p.mods[p.crates[a as usize].root_mod as usize].file;
    let def = |name: &str| (0..p.defs.len()).map(|i| crate::program::DefId(i as u32)).find(|d| p.name(*d) == name).unwrap();
    let visible = crates_seeing_edit(&p, [file].into_iter());
    assert!(!gandalf_keeps(&p, &visible, def("f")), "the edited crate's owners are checked again");
    assert!(!gandalf_keeps(&p, &visible, def("g")), "a crate that sees the edit is checked again");
    assert!(gandalf_keeps(&p, &visible, def("h")), "a crate that cannot see the edit keeps its verdicts");
    let plan_of = |src: &str| {
        let v = p.parse_snapshot(file, &a_path).parse(src.as_bytes().to_vec()).unwrap();
        classify(&p, file, &v.pf, &a_path, &Default::default())
    };
    let body = plan_of(&a_src.replace("{ 1 }", "{ 7 }")).unwrap();
    assert!(gandalf_carries(&body.changes, [&body].into_iter()), "a body edit carries");
    let signature = plan_of(&a_src.replace("-> u32 { 1 }", "-> u64 { 1 }")).unwrap();
    assert!(!signature.added.is_empty() && !signature.removed.is_empty());
    assert!(gandalf_carries(&signature.changes, [&signature].into_iter()), "a signature edit carries the verdicts of crates that cannot see it");
    if let Ok(layout) = plan_of(&a_src.replace("pub x: u32", "pub x: u32, pub y: u32")) {
        assert!(!gandalf_carries(&layout.changes, [&layout].into_iter()), "a layout edit drops every verdict");
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A warm launch restores Program and Tcx from the collected image, whose tables are read in
/// place (ImageMap/ImageVec, mapped interner). Live edits then mutate them: a body patch, and
/// an edit that adds an item (collection of the new definitions makes the tables owned).
#[test]
fn restored_images_take_live_edits() {
    const CHILD: &str = "MAKEPAD_RUN_RESTORED_EDIT_TEST";
    if std::env::var(CHILD).is_err() {
        // the unit and its runtime are process-global
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::restored_images_take_live_edits", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1"));
        assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
        return;
    }
    crate::borrowck::lazy::disable_gandalf();
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-restored-edit-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"restored\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let path = dir.join("src/lib.rs").to_string_lossy().into_owned();
    // (a `global_asm!` block: a restore runs no collection, so the image must carry it -- a warm
    // run of a crate with one could not resolve the labels it defines)
    const ASM: &str = "std::arch::global_asm!(\".p2align 2\", \"restored_asm_label:\", \"  ret\");\n";
    const W0: &str = "pub struct W<T>(pub T);\nimpl<T> W<T> { pub fn gone(&self) -> usize { std::mem::size_of::<T>() } pub fn kept(&self) {} }\n";
    std::fs::write(&path, format!("pub struct S {{ pub a: u32 }}\nimpl S {{ pub fn get(&self) -> u32 {{ self.a }} }}\npub fn value(out: &mut u32) {{ *out = S {{ a: 11 }}.get(); }}\n{W0}{ASM}")).unwrap();
    let session = Arc::new(crate::Session::default());
    let (mut prog, core, app) = crate::load_with_std(&session, &path, "restored", false).unwrap();
    assert!(prog.global_asms.iter().any(|g| prog.file_crate[g.0 as usize] == app), "the app's global_asm! block is collected");
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    // the store hands out 8-byte aligned payloads
    let mapped = |bytes: Vec<u8>| -> &'static [u8] {
        let words: &'static mut [u64] = Box::leak(vec![0u64; bytes.len().div_ceil(8)].into_boxed_slice());
        let b: &'static mut [u8] = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, bytes.len()) };
        b.copy_from_slice(&bytes);
        b
    };
    let program = mapped(prog.collected_image().expect("Program image"));
    let types = mapped(tcx.collected_image().expect("Tcx image"));
    let mut files: std::collections::HashMap<String, crate::parser::ParsedFile> = std::mem::take(&mut prog.files).into_iter()
        .zip(prog.file_paths.clone()).map(|(f, p)| (p, Arc::try_unwrap(f).ok().expect("unshared file"))).collect();
    let restored = Program::restore_collected(&session, program, &prog.load_recipe, |_, _, _, meta| files.remove(&meta.path)).expect("Program restore");
    let restored_tcx = crate::tcx::Tcx::restore_collected(types, restored.files.len(), restored.crates.len()).expect("Tcx restore");
    assert_eq!(restored.defs.len(), prog.defs.len());
    assert!(restored.syms.names().eq(prog.syms.names()));
    assert_eq!(restored.global_asms, prog.global_asms);
    let def = restored.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app && restored.name(id) == "value").then_some(id)
    }).unwrap();
    drop(prog);
    let mut u = crate::jit::Unit::new(restored, restored_tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let entry = crate::owner::call(crate::owner::Class::Program, || {
        let id = u.fn_id(crate::jit::FnKey::Inst(def, vec![]));
        u.entry(id)
    });
    let run = |expected| {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, entry, &mut result as *mut u32 as u64) }
            .unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        assert_eq!(result, expected);
    };
    run(11);
    let edit = |src: &str| unsafe {
        crate::reload::apply(up, vec![crate::reload::Edit { file: (*up).prog.def(def).file, path: path.clone(), src: format!("{src}{ASM}").into_bytes() }])
    };
    // the restored image's first edit removes a method from a generic impl and adds items ahead of
    // it: the def of `gone` stays in the version before (no definition collection recorded its
    // calls since the restore, so the legacy-call pass walks it), its impl's moves on, and the pass
    // reads the impl's generics in `gone`'s own version -- not at the new version's item number,
    // outside that tree (an internal compiler error)
    let w1: String = (0..16).map(|i| format!("pub fn more{i}() {{}}\n")).collect::<String>() + "pub struct W<T>(pub T);\nimpl<T> W<T> { pub fn kept(&self) {} }\n";
    let removed = edit(&format!("pub struct S {{ pub a: u32 }}\nimpl S {{ pub fn get(&self) -> u32 {{ self.a }} }}\npub fn value(out: &mut u32) {{ *out = S {{ a: 11 }}.get(); }}\n{w1}"));
    assert!(removed.failure.is_none(), "{:?}", removed.lines);
    assert_eq!(removed.rung, crate::reload::Rung::Patch, "{:?}", removed.lines);
    run(11);
    let body = edit(&format!("pub struct S {{ pub a: u32 }}\nimpl S {{ pub fn get(&self) -> u32 {{ self.a + 1 }} }}\npub fn value(out: &mut u32) {{ *out = S {{ a: 22 }}.get(); }}\n{w1}"));
    assert_eq!(body.rung, crate::reload::Rung::Patch, "{:?}", body.lines);
    run(23);
    const S2: &str = "pub struct S { pub a: u32 }\nimpl S { pub fn get(&self) -> u32 { self.a + extra() } }\npub fn extra() -> u32 { 9 }\npub fn value(out: &mut u32) { *out = S { a: 22 }.get(); }\n";
    let added = edit(&format!("{S2}{w1}"));
    assert_eq!(added.rung, crate::reload::Rung::Patch, "{:?}", added.lines);
    run(31);
    // Like settle_tests: process exit owns the runtime worker/executable-memory teardown.
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// std::relax::realtime for arming a machine (the Fab arms experiment): `compiled` is the strict ready (the whole
/// reach compiled, never "the wait ran out"), `lazy` counts what would still compile on a call, and
/// an armed root freezes live edits of real-time code (refused "frozen", applied at the disarm)
/// while edits of other code patch as usual.
#[test]
fn realtime_strict_ready_lazy_and_patch_freeze() {
    const CHILD: &str = "MAKEPAD_RUN_RT_FREEZE_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_strict_ready_lazy_and_patch_freeze", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    use crate::reload::{apply, Edit, Rung};
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-rt-freeze-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let src = |gain: u32, other: u32| format!("pub fn value(out: &mut u32) {{ *out = gain(); }}\n#[inline(never)]\npub fn gain() -> u32 {{ {gain} }}\npub fn other(out: &mut u32) {{ *out = {other}; }}\n");
    std::fs::write(dir.join("app/src/lib.rs"), src(11, 5)).unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "rt_freeze_app", false).unwrap();
    let def = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let (value, other) = (def("value"), def("other"));
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let (root, value_entry, other_entry) = crate::owner::call(crate::owner::Class::Program, || {
        let root = u.fn_id(crate::jit::FnKey::Inst(value, vec![]));
        let o = u.fn_id(crate::jit::FnKey::Inst(other, vec![]));
        (root, u.entry(root), u.entry(o))
    });
    let run = |entry| {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, entry, &mut result as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        result
    };
    // the strict ready: false (lazy > 0) until the reach is compiled, then true with nothing lazy
    let ticket = crate::owner::call(crate::owner::Class::Program, || unsafe { crate::jit::realtime_mark_slots(up, vec![root]) });
    loop {
        // (in this order: the reach finishing between the two reads is no failure)
        let lazy = crate::jit::__relax_realtime_lazy(ticket);
        if crate::jit::__relax_realtime_compiled(ticket) {
            break;
        }
        assert!(lazy > 0, "not compiled, yet nothing lazy");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert_eq!(crate::jit::__relax_realtime_lazy(ticket), 0, "compiled with lazy fns left");
    assert_eq!(run(value_entry), 11);
    let file = u.prog.def(value).file;
    let edit = |text: String| unsafe { apply(up, vec![Edit { file, path: app.clone(), src: text.into_bytes() }]) };
    // armed: an edit of real-time code (`gain`, which `value` calls) is held, nothing installs
    crate::jit::__relax_realtime_arm(true);
    let o = edit(src(12, 5));
    assert_eq!(o.rung, Rung::Refuse, "an armed root's code was patched: {:?}", o.lines);
    assert!(o.lines.iter().any(|l| l.starts_with("frozen:") && l.contains("gain")), "{:?}", o.lines);
    assert_eq!(run(value_entry), 11, "the frozen edit changed real-time code");
    // ... an edit of other code patches as usual
    let o = edit(src(11, 6));
    assert_eq!(o.rung, Rung::Patch, "an edit of non-real-time code was held: {:?}", o.lines);
    assert_eq!(run(other_entry), 6);
    // disarmed: the held edit applies
    crate::jit::__relax_realtime_arm(false);
    let o = edit(src(12, 6));
    assert_eq!(o.rung, Rung::Patch, "{:?}", o.lines);
    assert_eq!(run(value_entry), 12);
    assert_eq!(crate::jit::__relax_realtime_holes(), 0, "the real-time set had holes (said on stderr)");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// std::relax::realtime_thread marks the calling thread only, for good: is_realtime_thread
/// (what counts a lazy compile in realtime_misses) answers so there, not on other threads.
#[test]
fn realtime_thread_marks_the_calling_thread() {
    let marked = std::thread::spawn(|| {
        let before = crate::jit::is_realtime_thread();
        crate::jit::__relax_realtime_thread();
        (before, crate::jit::is_realtime_thread())
    }).join().unwrap();
    assert_eq!(marked, (false, true));
    assert!(!std::thread::spawn(crate::jit::is_realtime_thread).join().unwrap(), "another thread is not marked");
}

/// A Fab arms run's hole: the marked root calls through `dyn Route` (its value holds no trait object),
/// and other code makes the first `Box<Ded> -> Box<dyn Route>` after the mark, and a second type's
/// after the reach is done. The root's lowering names the trait object it calls through, so the
/// reach follows its vtables: the strict ready is false the moment a new one is made (polled
/// right after the unsizing code ran), true once its methods are compiled, and the real-time
/// thread then meets no lazy compile.
#[test]
fn realtime_reach_follows_vtables_made_after_the_mark() {
    const CHILD: &str = "MAKEPAD_RUN_RT_DYN_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_reach_follows_vtables_made_after_the_mark", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-rt-dyn-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("app/src/lib.rs"), "pub trait Route { fn next(&mut self) -> u32; }\n\
        pub struct Ded(pub u32);\n\
        impl Route for Ded { fn next(&mut self) -> u32 { self.0 += 3; self.0 } }\n\
        pub struct Arc2(pub u32);\n\
        impl Route for Arc2 { fn next(&mut self) -> u32 { self.0 * 7 } }\n\
        #[repr(C)]\n\
        pub struct Cell { pub route: Option<Box<dyn Route>>, pub out: u32 }\n\
        pub fn run(c: &mut Cell) { if let Some(r) = c.route.as_mut() { c.out = r.next(); } }\n\
        pub fn start(c: &mut Cell) { c.route = Some(Box::new(Ded(1))); }\n\
        pub fn switch(c: &mut Cell) { c.route = Some(Box::new(Arc2(6))); }\n").unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "rt_dyn_app", false).unwrap();
    let def = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let (run_d, start_d, switch_d) = (def("run"), def("start"), def("switch"));
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let upn = up as usize;
    let (root, [run_e, start_e, switch_e]) = crate::owner::call(crate::owner::Class::Program, || {
        let ids = [run_d, start_d, switch_d].map(|d| u.fn_id(crate::jit::FnKey::Inst(d, vec![])));
        (ids[0], ids.map(|id| u.entry(id)))
    });
    // the Cell (#[repr(C)]: the Option<Box<dyn Route>> as data and vtable words, then `out`), None
    let mut cell = [0u64; 3];
    let cp = cell.as_mut_ptr() as u64;
    let call = |entry: u64| unsafe { crate::jit::call_jit(upn as *mut crate::jit::Unit, entry, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
    // `run` on a real-time thread: its result and the lazy compiles it met
    let run_rt = move |cp: u64| std::thread::spawn(move || {
        crate::jit::__relax_realtime_thread();
        let before = crate::jit::__relax_realtime_misses();
        unsafe { crate::jit::call_jit(upn as *mut crate::jit::Unit, run_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        (unsafe { *((cp + 16) as *const u32) }, crate::jit::__relax_realtime_misses() - before)
    }).join().unwrap();
    // the method `next` of the vtable the cell holds now (its second word): compiled?
    let next_built = || crate::owner::call(crate::owner::Class::Program, || {
        let vtable = unsafe { *((cp + 8) as *const u64) };
        let id = u.thunk_slot(unsafe { *((vtable + 24) as *const u64) });
        id.is_some_and(|id| u.fns[id as usize].compiled)
    });
    let compiled = |t| crate::jit::__relax_realtime_compiled(t);
    // the mark, then at once the first unsizing (a Fab arms run's order), polled right after it ran
    let ticket = crate::owner::call(crate::owner::Class::Program, || unsafe { crate::jit::realtime_mark_slots(up, vec![root]) });
    call(start_e);
    assert!(!compiled(ticket) || next_built(), "compiled right after the first `Box<Ded> -> Box<dyn Route>`, with `<Ded as Route>::next` lazy");
    wait_realtime_compiled(ticket);
    assert!(next_built(), "compiled, with `<Ded as Route>::next` lazy");
    assert_eq!(run_rt(cp), (4, 0), "the real-time thread met a lazy compile");
    // a second type's vtable, made after the reach is done: false at once, until it is compiled
    call(switch_e);
    assert!(!compiled(ticket) || next_built(), "compiled right after the first `Box<Arc2> -> Box<dyn Route>`, with `<Arc2 as Route>::next` lazy");
    wait_realtime_compiled(ticket);
    assert_eq!(run_rt(cp), (42, 0), "the real-time thread met a lazy compile");
    assert_eq!(crate::jit::__relax_realtime_holes(), 0, "the real-time set had holes (said on stderr)");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// The reach follows the trait objects its root is handed however deep they are held: the root's
/// `&mut Cell` holds a `Vec<Stage>`, each stage a `Box<dyn Route>` (in a struct, in a vector's
/// buffer behind its raw pointer). Other code makes the vtable after the mark; the real-time
/// thread meets no lazy compile.
#[test]
fn realtime_reach_follows_trait_objects_held_deep_in_the_roots_value() {
    const CHILD: &str = "MAKEPAD_RUN_RT_DEEP_DYN_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_reach_follows_trait_objects_held_deep_in_the_roots_value", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-rt-deep-dyn-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("app/src/lib.rs"), "pub trait Route { fn next(&mut self) -> u32; }\n\
        pub struct Ded(pub u32);\n\
        impl Route for Ded { fn next(&mut self) -> u32 { self.0 += 3; self.0 } }\n\
        pub struct Stage { pub k: u32, pub route: Box<dyn Route> }\n\
        #[repr(C)]\n\
        pub struct Cell { pub out: u32, pub stages: Vec<Stage> }\n\
        pub fn run(c: &mut Cell) { let mut n = 0; for s in c.stages.iter_mut() { n += s.k * s.route.next(); } c.out = n; }\n\
        pub fn start(c: &mut Cell) { unsafe { std::ptr::write(&mut c.stages, vec![Stage { k: 2, route: Box::new(Ded(1)) }]) } }\n").unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "rt_deep_dyn_app", false).unwrap();
    let def = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let (run_d, start_d) = (def("run"), def("start"));
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let upn = up as usize;
    let (root, [run_e, start_e]) = crate::owner::call(crate::owner::Class::Program, || {
        let ids = [run_d, start_d].map(|d| u.fn_id(crate::jit::FnKey::Inst(d, vec![])));
        (ids[0], ids.map(|id| u.entry(id)))
    });
    // the Cell (#[repr(C)]: `out`, then the Vec's words)
    let mut cell = [0u64; 8];
    let cp = cell.as_mut_ptr() as u64;
    // the mark, then the first `Box<Ded> -> Box<dyn Route>` (made by other code, after it)
    let ticket = crate::owner::call(crate::owner::Class::Program, || unsafe { crate::jit::realtime_mark_slots(up, vec![root]) });
    unsafe { crate::jit::call_jit(up, start_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
    wait_realtime_compiled(ticket);
    let (out, misses) = std::thread::spawn(move || {
        crate::jit::__relax_realtime_thread();
        let before = crate::jit::__relax_realtime_misses();
        unsafe { crate::jit::call_jit(upn as *mut crate::jit::Unit, run_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        (unsafe { *(cp as *const u32) }, crate::jit::__relax_realtime_misses() - before)
    }).join().unwrap();
    assert_eq!((out, misses), (8, 0), "the real-time thread met lazy compiles (MAKEPAD_RUN_RT_TRACE=1 names them)");
    assert_eq!(crate::jit::__relax_realtime_holes(), 0, "the real-time set had holes (said on stderr)");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// The fast bucket's red Sandbox present (`<ScriptErrorRecord as Debug>` built ahead, not provided):
/// the reach followed every vtable of every trait-object type real-time code calls through, and
/// std's own are the whole program (a formatter's `dyn Write`, derived Debug's `dyn Debug`, a
/// `Once`'s `dyn FnMut`, a table's hasher, a thread's `dyn FnOnce`). Here the root passes its own
/// closure as `&mut dyn FnMut(u32)`, and other code made a `Box<dyn FnMut(u32)>` of another
/// closure before the mark: that closure is no real-time code (its callee stays out of the reach),
/// and the real-time thread still meets no lazy compile.
#[test]
fn realtime_reach_skips_trait_objects_it_only_makes_itself() {
    const CHILD: &str = "MAKEPAD_RUN_RT_OWN_DYN_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_reach_skips_trait_objects_it_only_makes_itself", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-rt-own-dyn-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("app/src/lib.rs"), "#[inline(never)]\n\
        pub fn each(f: &mut dyn FnMut(u32), n: u32) { for i in 0..n { f(i) } }\n\
        #[inline(never)]\n\
        pub fn helper(i: u32) -> u32 { i * 9 }\n\
        #[repr(C)]\n\
        pub struct Cell { pub out: u32 }\n\
        pub fn run(c: &mut Cell) { let mut s = 0; each(&mut |i| s += i, 4); c.out = s; }\n\
        pub fn make(c: &mut Cell) { let f: Box<dyn FnMut(u32)> = Box::new(|i| { c_out(helper(i)); }); c.out = 1; std::mem::forget(f); }\n\
        #[inline(never)]\n\
        pub fn c_out(_: u32) {}\n").unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "rt_own_dyn_app", false).unwrap();
    let def = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let (run_d, make_d, helper_d) = (def("run"), def("make"), def("helper"));
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let upn = up as usize;
    let (root, run_e, make_e, helper) = crate::owner::call(crate::owner::Class::Program, || {
        let [r, m, h] = [run_d, make_d, helper_d].map(|d| u.fn_id(crate::jit::FnKey::Inst(d, vec![])));
        (r, u.entry(r), u.entry(m), h)
    });
    let mut cell = [0u32];
    let cp = cell.as_mut_ptr() as u64;
    // the other closure's `Box<dyn FnMut(u32)>`: its vtable exists before the mark
    unsafe { crate::jit::call_jit(up, make_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
    assert_eq!(cell[0], 1);
    let ticket = crate::owner::call(crate::owner::Class::Program, || unsafe { crate::jit::realtime_mark_slots(up, vec![root]) });
    wait_realtime_compiled(ticket);
    assert!(!crate::jit::realtime_reached(helper), "the reach took in `helper`, which only a closure of other code calls (every vtable of `dyn FnMut(u32)` followed)");
    let (out, misses) = std::thread::spawn(move || {
        crate::jit::__relax_realtime_thread();
        let before = crate::jit::__relax_realtime_misses();
        unsafe { crate::jit::call_jit(upn as *mut crate::jit::Unit, run_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        (unsafe { *(cp as *const u32) }, crate::jit::__relax_realtime_misses() - before)
    }).join().unwrap();
    assert_eq!((out, misses), (6, 0), "the real-time thread met lazy compiles (MAKEPAD_RUN_RT_TRACE=1 names them)");
    assert_eq!(crate::jit::__relax_realtime_holes(), 0, "the real-time set had holes (said on stderr)");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A Fab run's misses on a node (24 lazy compiles on the real-time thread, drop glue of generic
/// containers and core::fmt's float path): code ran before the mark (the planner's simulation), so
/// the reach found slots compiled already whose RIR was dropped after their install. A drop glue's
/// references were never read again (only fns' RIR was rebuilt), and with a disk cache a lookup
/// that missed left the last entry's references in place. Here `warm` compiles `take` and the drop
/// glue of `Option<Item>` (dropping `None`: the glue of `Item`, its `Vec` and its `Weak` stay
/// lazy) and formats an integer; the marked `run` drops a `Some` and formats a float.
#[test]
fn realtime_reach_walks_code_that_ran_before_the_mark() {
    const CHILD: &str = "MAKEPAD_RUN_RT_WARM_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_reach_walks_code_that_ran_before_the_mark", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-rt-warm-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("app/src/lib.rs"), "use std::rc::{Rc, Weak};\n\
        pub struct Item(pub Vec<u32>, pub Weak<u32>);\n\
        #[inline(never)]\n\
        pub fn take(o: Option<Item>) -> u32 { match &o { Some(i) => i.0.len() as u32, None => 0 } }\n\
        #[repr(C)]\n\
        pub struct Cell { pub x: u32, pub out: u32 }\n\
        pub fn warm(c: &mut Cell) { c.out = take(None) + format!(\"{}\", c.x).len() as u32; }\n\
        pub fn run(c: &mut Cell) {\n\
            let rc = Rc::new(c.x);\n\
            let n = take(Some(Item(vec![c.x, 2, 3], Rc::downgrade(&rc))));\n\
            let s = format!(\"{:.3}\", c.x as f64 * 1.5);\n\
            c.out = n + s.len() as u32;\n\
        }\n").unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "rt_warm_app", false).unwrap();
    let def = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let (run_d, warm_d) = (def("run"), def("warm"));
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let upn = up as usize;
    let (root, [run_e, warm_e]) = crate::owner::call(crate::owner::Class::Program, || {
        let ids = [run_d, warm_d].map(|d| u.fn_id(crate::jit::FnKey::Inst(d, vec![])));
        (ids[0], ids.map(|id| u.entry(id)))
    });
    let mut cell = [2u32, 0];
    let cp = cell.as_mut_ptr() as u64;
    // what ran before the mark
    unsafe { crate::jit::call_jit(up, warm_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
    assert_eq!(cell[1], 1);
    let ticket = crate::owner::call(crate::owner::Class::Program, || unsafe { crate::jit::realtime_mark_slots(up, vec![root]) });
    wait_realtime_compiled(ticket);
    let (out, misses) = std::thread::spawn(move || {
        crate::jit::__relax_realtime_thread();
        let before = crate::jit::__relax_realtime_misses();
        unsafe { crate::jit::call_jit(upn as *mut crate::jit::Unit, run_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        (unsafe { *((cp + 4) as *const u32) }, crate::jit::__relax_realtime_misses() - before)
    }).join().unwrap();
    assert_eq!((out, misses), (3 + 5, 0), "the real-time thread met lazy compiles (MAKEPAD_RUN_RT_TRACE=1 names them)");
    assert_eq!(crate::jit::__relax_realtime_holes(), 0, "the real-time set had holes (said on stderr)");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Waits until the real-time reach of `ticket` is compiled, with no wall-clock limit (as lazy_tests):
/// a reach that never finishes hangs the test, and a loaded machine only makes it slower (a 10 s
/// cap failed three suite copies at once with the code cache off).
fn wait_realtime_compiled(ticket: u64) {
    while !crate::jit::__relax_realtime_compiled(ticket) {
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

/// What a Fab run's misses on a node were, on a cold store: the marked root formats floats
/// (core::fmt's float path is reached through the formatter fn pointer an `Argument` holds, data the
/// reach reads) and drops containers (`Vec`, `vec::IntoIter`, `array::IntoIter`, `Weak`: drop glue
/// of generic types). Nothing else ran before the mark: the reach alone compiles them, and the
/// real-time thread meets no lazy compile (the narrowed trait-object reach keeps this).
#[test]
fn realtime_reach_covers_float_formatting_and_container_drops() {
    const CHILD: &str = "MAKEPAD_RUN_RT_FMT_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_reach_covers_float_formatting_and_container_drops", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-rt-fmt-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("app/src/lib.rs"), "use std::rc::Rc;\n\
        #[repr(C)]\n\
        pub struct Cell { pub x: u32, pub out: u32 }\n\
        pub fn run(c: &mut Cell) {\n\
            let s = format!(\"{:.3} {}\", c.x as f64 * 1.5, c.x as f32 / 3.0);\n\
            let v: Vec<u32> = vec![c.x, 2, 3];\n\
            let mut n = 0;\n\
            for a in v.clone().into_iter() { n += a; }\n\
            for a in [c.x, 7] { n += a; }\n\
            let rc = Rc::new(c.x);\n\
            let w = Rc::downgrade(&rc);\n\
            drop(rc);\n\
            drop(w);\n\
            drop(v);\n\
            c.out = s.len() as u32 + n;\n\
        }\n").unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "rt_fmt_app", false).unwrap();
    let run_d = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == "run").then_some(id)
    }).unwrap();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let upn = up as usize;
    let (root, run_e) = crate::owner::call(crate::owner::Class::Program, || {
        let id = u.fn_id(crate::jit::FnKey::Inst(run_d, vec![]));
        (id, u.entry(id))
    });
    let ticket = crate::owner::call(crate::owner::Class::Program, || unsafe { crate::jit::realtime_mark_slots(up, vec![root]) });
    wait_realtime_compiled(ticket);
    let mut cell = [2u32, 0];
    let cp = cell.as_mut_ptr() as u64;
    let (out, misses) = std::thread::spawn(move || {
        crate::jit::__relax_realtime_thread();
        let before = crate::jit::__relax_realtime_misses();
        unsafe { crate::jit::call_jit(upn as *mut crate::jit::Unit, run_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        (unsafe { *((cp + 4) as *const u32) }, crate::jit::__relax_realtime_misses() - before)
    }).join().unwrap();
    let want = format!("{:.3} {}", 2f64 * 1.5, 2f32 / 3.0).len() as u32 + (2 + 2 + 3) + (2 + 7);
    assert_eq!((out, misses), (want, 0), "the real-time thread met lazy compiles (MAKEPAD_RUN_RT_TRACE=1 names them)");
    assert_eq!(crate::jit::__relax_realtime_holes(), 0, "the real-time set had holes (said on stderr)");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A tier-up of real-time code whose new code calls fns that never had code (an O2 build, then an
/// O3 build, each calling a fn nothing called before, which calls another such fn): those are
/// compiled before the new code is installed, the old code running meanwhile, so the real-time
/// thread meets no lazy compile. An install whose references were not walked before it is a hole
/// the install says and counts (realtime_holes), so a pass never rests on the walk after it
/// winning the race against the real-time thread. (Fab's arm lab met such compiles of thread-local
/// accessors in its armed loop.)
#[test]
fn realtime_tier_up_compiles_new_callees_before_its_install() {
    const CHILD: &str = "MAKEPAD_RUN_RT_TIER_UP_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_tier_up_compiles_new_callees_before_its_install", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    use relax_backend::rir::{Callee, Func, Inst};
    use std::sync::atomic::Ordering;
    crate::jit::CHECK_FIRST.store(false, Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-rt-tier-up-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("app/src/lib.rs"), "#[inline(never)] pub fn twenty() -> u32 { 20 }\n\
        #[inline(never)] pub fn three_hundred() -> u32 { 300 }\n\
        #[inline(never)] pub fn first(out: &mut u32) { *out += 1; }\n\
        #[inline(never)] pub fn second(out: &mut u32) { *out += twenty(); }\n\
        #[inline(never)] pub fn third(out: &mut u32) { *out += three_hundred(); }\n\
        pub fn value(out: &mut u32) { *out = 0; first(out); }\n").unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "rt_tier_up_app", false).unwrap();
    let def = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let defs: Vec<_> = ["value", "first", "second", "third", "twenty", "three_hundred"].iter().map(|n| def(n)).collect();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let upn = up as usize;
    let (slots, root_e) = crate::owner::call(crate::owner::Class::Program, || {
        let s: Vec<u32> = defs.iter().map(|&d| u.fn_id(crate::jit::FnKey::Inst(d, vec![]))).collect();
        let e = u.entry(s[0]);
        (s, e)
    });
    let [root, first, second, third, twenty, three_hundred] = slots[..] else { unreachable!() };
    let ticket = crate::owner::call(crate::owner::Class::Program, || unsafe { crate::jit::realtime_mark_slots(up, vec![root]) });
    wait_realtime_compiled(ticket);
    let mut cell = [0u32];
    let cp = cell.as_mut_ptr() as u64;
    // (value, lazy compiles) of a call on a real-time thread
    let rt_call = || std::thread::spawn(move || {
        crate::jit::__relax_realtime_thread();
        let before = crate::jit::__relax_realtime_misses();
        unsafe { crate::jit::call_jit(upn as *mut crate::jit::Unit, root_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        (unsafe { *(cp as *const u32) }, crate::jit::__relax_realtime_misses() - before)
    }).join().unwrap();
    assert_eq!(rt_call(), (1, 0), "the marked code met lazy compiles");
    for (s, name) in [(second, "second"), (third, "third"), (twenty, "twenty"), (three_hundred, "three_hundred")] {
        assert!(!u.fns[s as usize].compiled, "`{name}` has code before any code calls it");
    }
    // the new code's call goes to `to` where the code it replaces called `from`
    let redirect = |f: &mut Func, from: u32, to: u32| {
        let mut n = 0;
        for b in &mut f.blocks {
            for i in &mut b.insts {
                if let Inst::Call(Callee::Fn(t), _, _) = i {
                    if *t == from {
                        *t = to;
                        n += 1;
                    }
                }
            }
        }
        n
    };
    // O2 (full, as after the first present): its build starts from the slot's RIR, which now calls `second`
    crate::jit::FIRST_PRESENT.store(true, Ordering::Release);
    let n = crate::owner::call(crate::owner::Class::Program, || {
        let mut f = (**u.fns[root as usize].rir.as_ref().expect("the reach keeps the root's RIR")).clone();
        let n = redirect(&mut f, first, second);
        u.fns[root as usize].rir = Some(Arc::new(f));
        n
    });
    assert_eq!(n, 1, "the root's RIR calls `first` once");
    assert!(unsafe { crate::jit::o2_build_now(up, root) }, "the O2 build was dropped at its front");
    assert_eq!(rt_call(), (20, 0), "O2: the new code (it calls `second`, which calls `twenty`) runs without lazy compiles");
    assert_eq!(crate::jit::__relax_realtime_holes(), 0, "O2: the new code was installed before its references were walked");
    // O3 (inspect's edit hook): its code calls `third`
    let mut n = 0;
    let mut edit = |f: &mut Func, _: &crate::jit::Unit| n += redirect(f, second, third);
    assert!(unsafe { crate::jit::compile_tier1_inspect(up, root, Some(&mut edit)) }, "the O3 build was not installed");
    assert_eq!(n, 1, "the O2 code's RIR calls `second` once");
    assert_eq!(rt_call(), (300, 0), "O3: the new code (it calls `third`, which calls `three_hundred`) runs without lazy compiles");
    assert_eq!(crate::jit::__relax_realtime_holes(), 0, "O3: the new code was installed before its references were walked");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// The reach reads the references of the code a real-time slot runs, not those of another version
/// of it. An earlier run stored `run`'s O2 code (the generic `twice` inlined, `more` called); this
/// run installs baseline code for it, which calls `twice` (code built before its key's entry was
/// there, or placed from a startup image older than the entry). The walk must follow the installed
/// code's calls: the fab arm lab's loop met lazy compiles of closures that only the code it ran
/// called (11-11).
#[test]
fn realtime_reach_reads_the_installed_code() {
    const CHILD: &str = "MAKEPAD_RUN_RT_INSTALLED_TEST";
    const DIR: &str = "MAKEPAD_RUN_RT_INSTALLED_TEST_DIR";
    let Ok(phase) = std::env::var(CHILD) else {
        let dir = std::env::temp_dir().join(format!("relax-rt-installed-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("app/src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
        std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        std::fs::write(dir.join("app/src/lib.rs"), "#[repr(C)]\n\
            pub struct Cell { pub x: u32, pub out: u32 }\n\
            fn twice<F: Fn(u32) -> u32>(f: F, x: u32) -> u32 { f(f(x)) }\n\
            #[inline(never)] pub fn more(x: u32) -> u32 { x + 1 }\n\
            pub fn run(c: &mut Cell) { let k = c.x; c.out = more(twice(|v| v * 3 + k, c.x)); }\n").unwrap();
        for phase in ["earlier", "now"] {
            let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::realtime_reach_reads_the_installed_code", "--nocapture", "--test-threads=1", "--format=terse"])
                .env(CHILD, phase).env(DIR, &dir).env("MAKEPAD_RUN_GANDALF_ALL", "0").env("MAKEPAD_RUN_SPEC", "0"));
            let out = String::from_utf8_lossy(&output.stdout);
            assert!(output.status.success() && out.contains("1 passed"), "{phase}: {out}\n{}", String::from_utf8_lossy(&output.stderr));
        }
        std::fs::remove_dir_all(dir).unwrap();
        return;
    };
    use std::sync::atomic::Ordering;
    crate::jit::CHECK_FIRST.store(false, Ordering::Relaxed);
    crate::borrowck::lazy::disable_gandalf();
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::path::PathBuf::from(std::env::var_os(DIR).unwrap());
    std::env::set_var("MAKEPAD_RUN_CACHE_DIR", dir.join("cache"));
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "rt_installed_app", false).unwrap();
    let run_d = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == "run").then_some(id)
    }).unwrap();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let upn = up as usize;
    let run = crate::owner::call(crate::owner::Class::Program, || {
        let mut cache = crate::cache::Cache::open(&u, crate::cache::Mode::ReadOnly).unwrap();
        cache.mode = crate::cache::Mode::On;
        u.cache = Some(Box::new(crate::cache::CacheCell::new(cache)));
        u.fn_id(crate::jit::FnKey::Inst(run_d, vec![]))
    });
    let names = |u: &crate::jit::Unit, ids: &[u32]| ids.iter().map(|&s| u.fns[s as usize].name.clone()).collect::<Vec<_>>();
    if phase == "earlier" {
        // the earlier run: `run`'s O2 code, `twice` and the closure inlined, is its stored entry
        crate::owner::call(crate::owner::Class::Program, || {
            let (code, map, relocs) = unsafe { crate::jit::compile_fn(up, run) }.unwrap();
            u.install(run, &code, &relocs, map, 0).unwrap();
        });
        crate::jit::FIRST_PRESENT.store(true, Ordering::Release);
        assert!(unsafe { crate::jit::o2_build_now(up, run) }, "the O2 build was dropped at its front");
        let spill = crate::owner::call(crate::owner::Class::Program, || {
            let inlined = names(&u, u.body_deps.get(&run).map_or(&[][..], |v| &v[..]));
            assert!(inlined.iter().any(|n| n.contains("twice")), "the O2 build inlines `twice` (else this fixture proves nothing): it took in {inlined:?}");
            u.flush_cache_records();
            u.cache.as_mut().unwrap().spill_take().unwrap()
        });
        assert!(spill.write().unwrap() > 0, "the O2 entry was not written");
        std::mem::forget(u);
        return;
    }
    // this run: baseline code for `run` (no inlining), built without the stored entry; its RIR
    // dropped at the install, as drop_completed_body drops it
    let (run_e, callees) = crate::owner::call(crate::owner::Class::Program, || {
        let cache = u.cache.take();
        u.fns[run as usize].baseline_hint.store(true, Ordering::Relaxed);
        let (code, map, relocs) = unsafe { crate::jit::compile_fn(up, run) }.unwrap();
        u.cache = cache;
        u.install(run, &code, &relocs, map, 0).unwrap();
        let f = &mut u.fns[run as usize];
        f.rir = None;
        f.normal_rir = None;
        f.lowering = None;
        let mut callees: Vec<u32> = relocs.iter().filter_map(|r| match r.target {
            relax_backend::rir::Reloc::Call(c) | relax_backend::rir::Reloc::Thunk(c) | relax_backend::rir::Reloc::Slot(c) if c != run => Some(c),
            _ => None,
        }).collect();
        callees.sort_unstable();
        callees.dedup();
        (u.entry(run), callees)
    });
    let called = names(&u, &callees);
    assert!(called.iter().any(|n| n.contains("twice")), "the installed code calls `twice` (else this fixture proves nothing): it calls {called:?}");
    // the earlier run's entry is what a lookup of `run` links now, and it does not call `twice`
    let stored = crate::owner::call(crate::owner::Class::Program, || {
        let linked = unsafe { crate::jit::cache_lookup(up, run) }.is_some();
        (linked, names(&u, &u.cache.as_ref().unwrap().last_callees))
    });
    assert!(stored.0 && stored.1.iter().any(|n| n.contains("more")) && !stored.1.iter().any(|n| n.contains("twice")), "the disk cache holds `run`'s O2 entry, which calls `more` and not `twice` (else this fixture proves nothing): linked {}, it calls {:?}", stored.0, stored.1);
    let ticket = crate::owner::call(crate::owner::Class::Program, || unsafe { crate::jit::realtime_mark_slots(up, vec![run]) });
    wait_realtime_compiled(ticket);
    let missed: Vec<String> = callees.iter().filter(|&&c| !crate::jit::realtime_reached(c)).map(|&c| u.fns[c as usize].name.to_string()).collect();
    assert!(missed.is_empty(), "the reach did not take in what the installed code calls: {missed:?} (it read another version's references)");
    let mut cell = [2u32, 0];
    let cp = cell.as_mut_ptr() as u64;
    let (out, misses) = std::thread::spawn(move || {
        crate::jit::__relax_realtime_thread();
        let before = crate::jit::__relax_realtime_misses();
        unsafe { crate::jit::call_jit(upn as *mut crate::jit::Unit, run_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        (unsafe { *((cp + 4) as *const u32) }, crate::jit::__relax_realtime_misses() - before)
    }).join().unwrap();
    assert_eq!((out, misses), (27, 0), "the real-time thread met lazy compiles (MAKEPAD_RUN_RT_TRACE=1 names them)");
    assert_eq!(crate::jit::__relax_realtime_holes(), 0, "the real-time set had holes (said on stderr)");
    std::mem::forget(u);
}

/// Calls the root's code makes through data other code filled: a trait object in a static (Fab's
/// slicer `FAN_OUT`, set by the app, read on the cell's loop), filled after the mark, and a fn
/// pointer in the root's value, filled before it. Neither target is named by code the reach
/// walks; the static's type (the code names the static) and the fn pointer's signature (every
/// slot whose address code took with it) close them over, and the real-time thread meets no lazy
/// compile.
#[test]
fn realtime_reach_follows_calls_through_statics_and_fn_pointers() {
    const CHILD: &str = "MAKEPAD_RUN_RT_DATA_CALLS_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_reach_follows_calls_through_statics_and_fn_pointers", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-rt-data-calls-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::write(dir.join("app/src/lib.rs"), "use std::sync::Mutex;\n\
        pub trait Route { fn next(&mut self) -> u32; }\n\
        pub struct Ded(pub u32);\n\
        impl Route for Ded { fn next(&mut self) -> u32 { self.0 += 3; self.0 } }\n\
        pub static ROUTE: Mutex<Option<Box<dyn Route + Send>>> = Mutex::new(None);\n\
        #[inline(never)] pub fn more(x: u32) -> u32 { x * 5 + 1 }\n\
        #[repr(C)]\n\
        pub struct Cell { pub x: u32, pub out: u32, pub f: Option<fn(u32) -> u32> }\n\
        pub fn run(c: &mut Cell) {\n\
            let n = match ROUTE.lock() { Ok(mut r) => r.as_mut().map_or(0, |r| r.next()), Err(_) => 0 };\n\
            let m = match c.f { Some(f) => f(c.x), None => 0 };\n\
            c.out = n * 100 + m;\n\
        }\n\
        pub fn start(c: &mut Cell) { c.f = Some(more); }\n\
        pub fn later() { if let Ok(mut r) = ROUTE.lock() { *r = Some(Box::new(Ded(1))); } }\n").unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "rt_data_calls_app", false).unwrap();
    let def = |name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let (run_d, start_d, later_d, more_d) = (def("run"), def("start"), def("later"), def("more"));
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let upn = up as usize;
    let (root, more, [run_e, start_e, later_e]) = crate::owner::call(crate::owner::Class::Program, || {
        let ids = [run_d, start_d, later_d, more_d].map(|d| u.fn_id(crate::jit::FnKey::Inst(d, vec![])));
        (ids[0], ids[3], [ids[0], ids[1], ids[2]].map(|id| u.entry(id)))
    });
    // the Cell (#[repr(C)]: `x`, `out`, then the fn pointer)
    let mut cell = [0u64; 2];
    cell[0] = 2;
    let cp = cell.as_mut_ptr() as u64;
    // the fn pointer is stored before the mark, the trait object after it (other code, both)
    unsafe { crate::jit::call_jit(up, start_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
    let ticket = crate::owner::call(crate::owner::Class::Program, || unsafe { crate::jit::realtime_mark_slots(up, vec![root]) });
    unsafe { crate::jit::call_jit(up, later_e, 0) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
    wait_realtime_compiled(ticket);
    let next = crate::owner::call(crate::owner::Class::Program, || u.fns.iter().position(|f| f.name.contains("Ded as") && f.name.ends_with("::next")));
    // (each hole on its own: both are said when both are open)
    let holes: Vec<&str> = [
        (!next.is_some_and(|n| crate::jit::realtime_reached(n as u32))).then_some("the trait object in the static: `<Ded as Route>::next` not reached"),
        (!crate::jit::realtime_reached(more)).then_some("the fn pointer in the root's value: `more` not reached"),
    ].into_iter().flatten().collect();
    assert!(holes.is_empty(), "{}", holes.join("; "));
    let (out, misses) = std::thread::spawn(move || {
        crate::jit::__relax_realtime_thread();
        let before = crate::jit::__relax_realtime_misses();
        unsafe { crate::jit::call_jit(upn as *mut crate::jit::Unit, run_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        (unsafe { *((cp + 4) as *const u32) }, crate::jit::__relax_realtime_misses() - before)
    }).join().unwrap();
    assert_eq!((out, misses), (4 * 100 + 11, 0), "the real-time thread met lazy compiles (MAKEPAD_RUN_RT_TRACE=1 names them)");
    assert_eq!(crate::jit::__relax_realtime_holes(), 0, "the real-time set had holes (said on stderr)");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// Fab's live edit on a node (ICEs "key in collected table", "variance declaration"): a new module
/// file whose structs (a `#[cfg(relax)]` field of a std type among them) an existing struct holds
/// in `Option` fields, filled through `get_or_insert_with`. The edit patches, the new code runs.
#[test]
fn live_new_module_types_edit() {
    const CHILD: &str = "MAKEPAD_RUN_NEW_MODULE_EDIT_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::live_new_module_types_edit", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    use crate::reload::{apply, Edit, Rung};
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-new-module-edit-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let v1 = r#"pub struct App { pub n: u32 }
impl App { pub fn step(&mut self) -> u32 { self.n += 1; self.n } }
pub fn value(out: &mut u32) {
    let mut a = App { n: 10 };
    *out = a.step();
}
"#;
    let v2 = r#"pub mod extra;
pub struct App { pub n: u32, pub rt: Option<extra::RtPath>, pub armed: Option<extra::RtArmed> }
impl App {
    pub fn step(&mut self) -> u32 {
        let rt = self.rt.get_or_insert_with(|| extra::RtPath::mark(Some(wake)));
        match rt.arm_for(extra::Mode::Real) {
            Ok(armed) => self.armed = armed,
            Err(_) => return 0,
        }
        self.n += rt.lazy() + 10 + self.armed.is_some() as u32;
        self.n
    }
}
extern "C" fn wake() {}
pub fn value(out: &mut u32) {
    let mut a = App { n: 10, rt: None, armed: None };
    *out = a.step();
}
"#;
    let extra = r#"pub enum Mode { Sim, Real }
pub struct RtPath {
    #[cfg(relax)]
    ticket: u64,
    wake: Option<extern "C" fn()>,
}
pub struct RtArmed {
    #[cfg(relax)]
    _guard: Vec<u8>,
}
impl RtPath {
    pub fn mark(wake: Option<extern "C" fn()>) -> RtPath {
        #[cfg(relax)]
        { RtPath { ticket: 1, wake } }
        #[cfg(not(relax))]
        { RtPath { wake } }
    }
    pub fn lazy(&self) -> u32 { if self.wake.is_some() { 0 } else { 1 } }
    pub fn arm(&self) -> Option<RtArmed> {
        #[cfg(relax)]
        { Some(RtArmed { _guard: vec![self.ticket as u8] }) }
        #[cfg(not(relax))]
        { Some(RtArmed {}) }
    }
    pub fn arm_for(&self, mode: Mode) -> Result<Option<RtArmed>, String> {
        match mode {
            Mode::Sim => Ok(None),
            Mode::Real => self.arm().map(Some).ok_or_else(|| format!("{} lazy", self.lazy())),
        }
    }
}
"#;
    std::fs::write(dir.join("app/src/lib.rs"), v1).unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "new_module_edit_app", false).unwrap();
    // a live session of this app (its structs take field edits in place)
    crate::borrowck::event::set_app_scope(vec![app.clone()]);
    crate::layout::set_root_crate(app_crate);
    crate::layout::LIVE_SESSION.store(true, std::sync::atomic::Ordering::Relaxed);
    let value = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == "value").then_some(id)
    }).unwrap();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let entry = crate::owner::call(crate::owner::Class::Program, || { let f = u.fn_id(crate::jit::FnKey::Inst(value, vec![])); u.entry(f) });
    let run = || {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, entry, &mut result as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        result
    };
    assert_eq!(run(), 11);
    let file = u.prog.def(value).file;
    std::fs::write(dir.join("app/src/extra.rs"), extra).unwrap();
    let o = unsafe { apply(up, vec![Edit { file, path: app.clone(), src: v2.as_bytes().to_vec() }]) };
    assert_eq!(o.rung, Rung::Patch, "{:?}", o.lines);
    assert_eq!(run(), 21);
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A live edit that renames a struct field finds the fns using it from the field uses typeck
/// resolved. A generic fn whose body names the old field's name (`window`'s local `plans`, fab
/// slice's `FieldPlans::window`) is read as typeck left it: instantiating it at its own
/// parameters for codegen has no answer (`<impl IntoIterator as IntoIterator>::IntoIter`), and
/// the edit failed as an internal compiler error (NoSolution).
#[test]
fn a_renamed_field_reads_generic_fns_as_typeck_left_them() {
    const CHILD: &str = "MAKEPAD_RUN_RENAMED_FIELD_EDIT_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::a_renamed_field_reads_generic_fns_as_typeck_left_them", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    use crate::reload::{apply, Edit, Rung};
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-renamed-field-edit-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let v1 = r#"pub struct Boundary { pub plans: bool }
pub fn window(layers: impl IntoIterator<Item = u32>) -> u32 {
    let mut plans = 0;
    for l in layers { plans += l; }
    plans
}
pub fn value(out: &mut u32) {
    let b = Boundary { plans: true };
    *out = if b.plans { window([1u32, 2, 3]) } else { 0 };
}
"#;
    let v2 = v1.replace("pub plans: bool", "pub ready: bool").replace("Boundary { plans: true }", "Boundary { ready: true }")
        .replace("if b.plans { window([1u32, 2, 3]) }", "if b.ready { window([1u32, 2, 3]) + 10 }");
    std::fs::write(dir.join("app/src/lib.rs"), v1).unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "renamed_field_edit_app", false).unwrap();
    crate::borrowck::event::set_app_scope(vec![app.clone()]);
    crate::layout::set_root_crate(app_crate);
    crate::layout::LIVE_SESSION.store(true, std::sync::atomic::Ordering::Relaxed);
    let value = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == "value").then_some(id)
    }).unwrap();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let entry = crate::owner::call(crate::owner::Class::Program, || { let f = u.fn_id(crate::jit::FnKey::Inst(value, vec![])); u.entry(f) });
    let run = || {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, entry, &mut result as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        result
    };
    assert_eq!(run(), 6);
    let file = u.prog.def(value).file;
    let o = unsafe { apply(up, vec![Edit { file, path: app.clone(), src: v2.into_bytes() }]) };
    assert!(o.failure.is_none() && o.rung == Rung::Patch, "{:?}", o.lines);
    assert_eq!(run(), 16);
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// `--lib` tests of a package whose dev-dependency depends back on it (M50): the harness crate
/// and the plain library the dependency links are two crates of one file. A live edit of that
/// file patches both, so a call through the dependency runs the new body too.
#[test]
fn a_live_edit_reaches_both_crates_of_a_lib_test_root() {
    const CHILD: &str = "MAKEPAD_RUN_TWIN_CRATE_EDIT_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::a_live_edit_reaches_both_crates_of_a_lib_test_root", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    use crate::reload::{apply, Edit, Rung};
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-twin-crate-edit-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("twinlib/src")).unwrap();
    std::fs::create_dir_all(dir.join("twinuser/src")).unwrap();
    let v1 = "pub fn f() -> u32 { 1 }\n#[cfg(test)]\npub fn value(out: &mut u32) { *out = f() * 100 + twinuser::through(); }\n";
    std::fs::write(dir.join("twinlib/src/lib.rs"), v1).unwrap();
    std::fs::write(dir.join("twinuser/src/lib.rs"), "pub fn through() -> u32 { twinlib::f() }\n").unwrap();
    let lib = dir.join("twinlib/src/lib.rs").to_string_lossy().into_owned();
    let user = dir.join("twinuser/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    crate::pmhost::install();
    let mut prog = Program::new(&session);
    let (core, std_deps) = crate::load_std_crates(&session, &mut prog).unwrap();
    let plain = prog.load_crate_deferred("twinlib", &lib, 2021, crate::program_cfg(&session, &[]), std_deps.clone()).unwrap();
    let mut deps = std_deps.clone();
    deps.push((prog.syms.intern("twinlib"), plain));
    let through = prog.load_crate_deferred("twinuser", &user, 2021, crate::program_cfg(&session, &[]), deps).unwrap();
    let mut cfg = crate::program_cfg(&session, &[]);
    cfg.set.push("test".to_string());
    let mut deps = std_deps;
    deps.push((prog.syms.intern("twinuser"), through));
    let harness = prog.load_crate_deferred("twinlib", &lib, 2021, cfg, deps).unwrap();
    prog.prelude_crate = Some(core);
    prog.resolve_imports();
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    crate::borrowck::event::set_app_scope(vec![lib.clone(), user.clone()]);
    crate::layout::set_root_crate(harness);
    crate::layout::LIVE_SESSION.store(true, std::sync::atomic::Ordering::Relaxed);
    let value = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == harness && prog.name(id) == "value").then_some(id)
    }).unwrap();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let entry = crate::owner::call(crate::owner::Class::Program, || { let f = u.fn_id(crate::jit::FnKey::Inst(value, vec![])); u.entry(f) });
    let run = || {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, entry, &mut result as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        result
    };
    assert_eq!(run(), 101);
    let file = u.prog.def(value).file;
    let o = unsafe { apply(up, vec![Edit { file, path: lib.clone(), src: v1.replace("{ 1 }", "{ 2 }").into_bytes() }]) };
    assert!(o.failure.is_none() && o.rung == Rung::Patch, "{:?}", o.lines);
    // the harness copy's f and the plain copy's f (reached through twinuser) both changed
    assert_eq!(run(), 202);
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A closure stored by the running version (a boxed handler) runs every later edit of its body,
/// and an edit's work stays flat however many came before: a patch installs at most two
/// forwarders per closure (the lineage's cell jumps to the newest version, the previous newest to
/// the cell) and copies no code into the older versions; every older version reaches the newest in
/// at most two jumps; a build of an older version's own body never lands in it (a tier-up would
/// bring back old code); a rolled-back patch brings back the forwarders it replaced.
#[test]
fn stored_closure_follows_edits_with_flat_work() {
    const CHILD: &str = "MAKEPAD_RUN_STORED_CLOSURE_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::stored_closure_follows_edits_with_flat_work", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    use crate::jit::{FnKey, Unit};
    use crate::reload::{apply, Edit, Rung};
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-stored-closure-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let src = |i: u32| format!(r#"use std::cell::RefCell;
thread_local! {{ static H: RefCell<Option<Box<dyn Fn(u32) -> u32>>> = RefCell::new(None); }}
fn make() -> Box<dyn Fn(u32) -> u32> {{
    let k = 3;
    Box::new(move |x| x * k + {i})
}}
pub fn value(out: &mut u32) {{
    H.with(|h| {{
        if h.borrow().is_none() {{ *h.borrow_mut() = Some(make()); }}
        *out = (h.borrow().as_ref().unwrap())(5);
    }});
}}
"#);
    std::fs::write(dir.join("app/src/lib.rs"), src(1)).unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "stored_closure_app", false).unwrap();
    let def = |prog: &Program, name: &str| prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == name).then_some(id)
    }).unwrap();
    let (value, make) = (def(&prog, "value"), def(&prog, "make"));
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    let (entry, make_slot) = crate::owner::call(crate::owner::Class::Program, || {
        let f = u.fn_id(FnKey::Inst(value, vec![]));
        (u.entry(f), u.fn_id(FnKey::Inst(make, vec![])))
    });
    let run = || {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, entry, &mut result as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        result
    };
    // the versions of make's closure, oldest first, and where each one's calls end up
    let versions = |u: &Unit| -> Vec<u32> { (0..u.fns.len() as u32).filter(|&id| matches!(u.fns[id as usize].key, FnKey::Closure(_, _, p) if p == make_slot) && u.fns[id as usize].compiled).collect() };
    let reaches = |u: &Unit, mut s: u32| -> u32 {
        for _ in 0..2 {
            match u.forward_of(s) { Some((t, _)) => s = t, None => return s }
        }
        assert!(u.forward_of(s).is_none(), "more than two jumps from an older version to the newest");
        s
    };
    assert_eq!(run(), 16);
    let first = versions(&u)[0];
    let mut files = vec![u.prog.def(value).file];
    let mut undo = None;
    let mut names_first: Option<Vec<String>> = None;
    for i in 2..=6u32 {
        let before = u.forwards.len();
        let o = unsafe { apply(up, vec![Edit { file: *files.last().unwrap(), path: app.clone(), src: src(i).into_bytes() }]) };
        assert_eq!(o.rung, Rung::Patch, "edit {i}: {:?}", o.lines);
        // (make, and value's closure while it inlines make)
        let first_names = names_first.get_or_insert_with(|| o.names.clone());
        assert!(o.names.iter().all(|n| first_names.contains(n)), "edit {i}: more fns installed than by the first edit: {:?}", o.names);
        files.push(o.files[0]);
        assert_eq!(run(), 15 + i, "edit {i}: the box stored at the first run runs the edited closure");
        assert_eq!(u.forwards.len() - before, if i == 2 { 1 } else { 2 }, "edit {i}: forwarders installed (the cell, the previous version)");
        let vs = versions(&u);
        let newest = *vs.last().unwrap();
        assert!(vs[..vs.len() - 1].iter().all(|&v| reaches(&u, v) == newest), "edit {i}: every older version reaches the newest");
        assert_eq!(u.forward_of(first), Some((newest, true)), "edit {i}: the first version is the lineage's cell");
        undo = o.undo;
    }
    // the old version's own body, built and installed as a tier-up would, does not replace its
    // forwarder; nor does it get an O3 build
    let at = u.fns[first as usize].code;
    let (code, map, relocs) = unsafe { crate::jit::compile_fn(up, first) }.unwrap();
    crate::owner::call(crate::owner::Class::Program, || u.install(first, &code, &relocs, map, 0)).unwrap();
    assert_eq!(u.fns[first as usize].code, at, "a build of the cell's own body was installed");
    assert!(!unsafe { crate::jit::compile_tier1(up, first) }, "an O3 build of a forwarder");
    assert_eq!(run(), 21);
    // edit 6 rolled back: the cell jumps to edit 5's closure again, which runs its own code
    let previous = versions(&u)[versions(&u).len() - 2];
    unsafe { crate::reload::rollback(up, undo.take().unwrap(), "test") };
    files.pop();
    assert_eq!(run(), 20, "after the rollback the stored box runs edit 5's closure");
    assert_eq!(u.forward_of(first), Some((previous, true)));
    assert_eq!(u.forward_of(previous), None, "edit 5's closure runs its own code again");
    let o = unsafe { apply(up, vec![Edit { file: *files.last().unwrap(), path: app.clone(), src: src(7).into_bytes() }]) };
    assert_eq!(o.rung, Rung::Patch, "{:?}", o.lines);
    assert_eq!(run(), 22, "an edit after the rollback reaches the stored box");
    assert_eq!(u.forward_of(previous), Some((first, false)), "edit 5's closure forwards to the cell");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A closure stored by the running version whose captures change keeps its old code (its values
/// hold the old environment). The edit lands, and the stored closure waits for a restart: a HELD
/// line, so the watcher offers the restart and `session view` lists it. A closure only called in
/// its parent's frame takes the new code with its parent and holds nothing back.
#[test]
fn a_stored_closure_whose_captures_change_waits_for_a_restart() {
    const CHILD: &str = "MAKEPAD_RUN_STORED_CAPTURES_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::a_stored_closure_whose_captures_change_waits_for_a_restart", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    use crate::jit::{FnKey, Unit};
    use crate::reload::{apply, Edit, Rung, HELD};
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-stored-captures-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("app/src")).unwrap();
    std::fs::write(dir.join("Cargo.toml"), "[workspace]\nmembers = [\"app\"]\n").unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), "[package]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    let src = |made: &str, summed: &str| format!(r#"use std::cell::RefCell;
thread_local! {{ static H: RefCell<Option<Box<dyn Fn(u32) -> u32>>> = RefCell::new(None); }}
#[inline(never)]
fn make() -> Box<dyn Fn(u32) -> u32> {{
    let k = 3;
    {made}
}}
#[inline(never)]
fn sum() -> u32 {{
    let k = 2;
    {summed}
}}
pub fn value(out: &mut u32) {{
    H.with(|h| {{
        if h.borrow().is_none() {{ *h.borrow_mut() = Some(make()); }}
        *out = (h.borrow().as_ref().unwrap())(5) * 100 + sum();
    }});
}}
"#);
    std::fs::write(dir.join("app/src/lib.rs"), src("Box::new(move |x| x * k + 1)", "(0..3u32).map(|x| x * k).sum()")).unwrap();
    let app = dir.join("app/src/lib.rs").to_string_lossy().into_owned();
    let session = Arc::new(crate::Session::default());
    let (prog, core, app_crate) = crate::load_with_std(&session, &app, "stored_captures_app", false).unwrap();
    let value = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == "value").then_some(id)
    }).unwrap();
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut Unit;
    let entry = crate::owner::call(crate::owner::Class::Program, || { let f = u.fn_id(FnKey::Inst(value, vec![])); u.entry(f) });
    let run = || {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, entry, &mut result as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        result
    };
    // 16 from the stored closure, 0 + 2 + 4 summed
    assert_eq!(run(), 1606);
    let file = u.prog.def(value).file;
    let edited = src("let j = 4;\n    Box::new(move |x| x * k + j)", "let j = 1;\n    (0..3u32).map(|x| x * k + j).sum()");
    let o = unsafe { apply(up, vec![Edit { file, path: app.clone(), src: edited.into_bytes() }]) };
    assert_eq!(o.rung, Rung::Patch, "{:?}", o.lines);
    let held: Vec<&String> = o.lines.iter().filter(|l| l.starts_with(HELD)).collect();
    assert!(held.len() == 1 && held[0].contains("make`") && held[0].contains("keep its old code (its captures or signature changed)"), "the stored closure, and only it, waits for a restart: {:?}", o.lines);
    // the stored box keeps its old code; `sum` runs its new one
    assert_eq!(run(), 1609);
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// A type alias in a fn body is part of that body: its edit is the body's edit (the alias
/// goes and a new one comes with the body). An alias a nested item names restarts as before:
/// the nested item would keep the old type.
#[test]
fn body_type_aliases_are_part_of_their_body() {
    use crate::reload::{classify, Change};
    let session = Arc::new(crate::Session::default());
    let cfg = crate::cfg::CfgSet::host(&session, &[]);
    let dir = std::env::temp_dir().join(format!("relax-body-aliases-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a_src = "pub fn f() -> u64 {\n    type Found = (u64, u64);\n    let p: Found = (1, 2);\n    p.0 + p.1\n}\npub struct S;\nimpl S {\n    pub fn g(&self) -> u64 {\n        type Pair = [u64; 2];\n        fn first(p: Pair) -> u64 { p[0] }\n        first([3, 4])\n    }\n}\n";
    let a_path = dir.join("a.rs");
    std::fs::write(&a_path, a_src).unwrap();
    let a_path = a_path.to_str().unwrap().to_string();
    let mut p = Program::new(&session);
    let a = p.load_crate("body_aliases", &a_path, 2021, cfg, vec![]).unwrap();
    p.resolve_imports();
    let file = p.mods[p.crates[a as usize].root_mod as usize].file;
    let plan_of = |src: String| {
        let v = p.parse_snapshot(file, &a_path).parse(src.into_bytes()).unwrap();
        classify(&p, file, &v.pf, &a_path, &Default::default())
    };
    let body = plan_of(a_src.replace("type Found = (u64, u64);\n    let p: Found = (1, 2);", "type Found = (u64, u64, u64);\n    let p: Found = (1, 2, 3);")).unwrap_or_else(|why| panic!("{why:?}"));
    let names: Vec<String> = body.changes.iter().map(|c| match c { Change::Body(d) => p.name(*d).to_string(), c => format!("{c:?}") }).collect();
    assert_eq!(names, ["f"], "the holder's body edit");
    assert!(body.added.is_empty() && body.removed.is_empty(), "the alias comes and goes with the body");
    let named = plan_of(a_src.replace("type Pair = [u64; 2];", "type Pair = [u64; 3];").replace("first([3, 4])", "first([3, 4, 5])")).err().expect("a nested fn names the alias");
    assert!(named.iter().any(|w| w.contains("type alias `Pair` changed")), "{named:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// An inline module's body is its items, classified one by one as a file's are: an edit of the
/// text between them (an item macro's arguments, an item its cfg leaves out, a test fn added to
/// a test module) patches, and so does a module made more visible. A module made less visible,
/// or given other attributes, restarts and says so (each said "a module's item list changed").
#[test]
fn module_bodies_classify_by_item() {
    use crate::reload::{classify, Change};
    let session = Arc::new(crate::Session::default());
    let mut cfg = crate::cfg::CfgSet::host(&session, &[]);
    cfg.set.push("test".into());
    let dir = std::env::temp_dir().join(format!("relax-module-bodies-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let a_src = "mod other;\nmacro_rules! mk { ($n:ident, $v:expr) => { pub fn $n() -> u32 { $v } } }\npub fn f() -> u32 { 1 }\npub mod inner {\n    pub fn g() -> u32 { 2 }\n    mk!(h, 3);\n    #[cfg(any())]\n    fn off() -> u32 { 4 }\n    thread_local! { static T: u32 = 5; }\n}\n#[cfg(test)]\nmod tests {\n    use super::*;\n    #[test]\n    fn one() { assert_eq!(f(), 1); }\n}\n";
    let a_path = dir.join("a.rs");
    std::fs::write(&a_path, a_src).unwrap();
    std::fs::write(dir.join("other.rs"), "pub fn o() -> u32 { 9 }\n").unwrap();
    let a_path = a_path.to_str().unwrap().to_string();
    let mut p = Program::new(&session);
    let a = p.load_crate("module_bodies", &a_path, 2021, cfg, vec![]).unwrap();
    p.resolve_imports();
    let file = p.mods[p.crates[a as usize].root_mod as usize].file;
    let plan_of = |src: String| {
        let v = p.parse_snapshot(file, &a_path).parse(src.into_bytes()).unwrap();
        classify(&p, file, &v.pf, &a_path, &Default::default())
    };
    let name = |c: &Change| match c { Change::Body(d) | Change::Const(d) | Change::StaticInit(d) | Change::Vis(d) => p.name(*d).to_string(), _ => String::new() };
    let patched = |what: &str, src: String| plan_of(src).unwrap_or_else(|why| panic!("{what}: {why:?}"));
    let macro_arg = patched("an item macro's argument", a_src.replace("mk!(h, 3)", "mk!(h, 33)"));
    assert_eq!(macro_arg.changes.iter().map(name).collect::<Vec<_>>(), ["h"], "the macro's fn has a new body");
    let off = patched("an item its cfg leaves out", a_src.replace("{ 4 }", "{ 44 }"));
    assert!(off.changes.is_empty() && off.added.is_empty() && off.removed.is_empty());
    let tls = patched("a thread_local! initializer", a_src.replace("= 5;", "= 55;"));
    assert!(!tls.changes.is_empty());
    let test = patched("a test fn added to a test module", a_src.replace("    fn one() { assert_eq!(f(), 1); }\n", "    fn one() { assert_eq!(f(), 1); }\n    #[test]\n    fn two() { assert_eq!(f(), 1); }\n"));
    assert_eq!((test.added.len(), test.changes.len()), (1, 0));
    let wider = patched("a module in a file of its own made public", a_src.replace("mod other;", "pub mod other;"));
    assert_eq!(wider.changes.iter().map(name).collect::<Vec<_>>(), ["other"]);
    assert!(matches!(wider.changes[0], Change::Vis(_)));
    let restart = |what: &str, src: String, says: &str| {
        let why = plan_of(src).err().unwrap_or_else(|| panic!("{what}: patched"));
        assert!(why.len() == 1 && why[0].contains(says), "{what}: {why:?}");
    };
    restart("a module made less visible", a_src.replace("pub mod inner {", "pub(crate) mod inner {"), "mod `inner` is less visible than before (`pub` to `pub(crate)`)");
    restart("a module given an attribute", a_src.replace("pub mod inner {", "#[allow(dead_code)]\npub mod inner {"), "mod `inner` changed: its attributes changed");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A new item in one JIT-compiled crate and its first use in another, saved together, patch in one
/// edit: the dependent crate's new code calls the new fn (no restart, nothing refused). The
/// counterpart, an item a crate the host provides lacks (its items are the running makepad's):
/// with makepad's inputs unchanged a use of it is the edit's own error, and once they changed it
/// waits for the rebuilt makepad, the restart offered -- also where this process cannot build
/// makepad and nothing on the ring builds (a dock app's sync, 11-10: E0425 on a new `makepad_hal`
/// op stood as the edit's own error). So does a new variant, associated fn or method of a provided
/// crate's type (a dock app's sync, 11-12: E0599 on a new `HalEvent` variant and `HalInfo` fn,
/// and E0425 on the binding of the variant's pattern, refused the landed edit); a missing item
/// of a type another crate also names stays the edit's own error.
#[test]
fn a_new_item_in_another_crate_patches_unless_the_host_provides_that_crate() {
    const CHILD: &str = "MAKEPAD_RUN_CROSS_ITEM_TEST";
    if std::env::var(CHILD).is_err() {
        let output = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::a_new_item_in_another_crate_patches_unless_the_host_provides_that_crate", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0"));
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success() && out.contains("1 passed"), "{out}\n{}", String::from_utf8_lossy(&output.stderr));
        return;
    }
    use crate::reload::{apply, Edit, Rung};
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let dir = std::env::temp_dir().join(format!("relax-cross-item-{}", std::process::id()));
    std::fs::write({ std::fs::create_dir_all(&dir).unwrap(); dir.join("Cargo.toml") }, "[workspace]\nmembers = [\"app\", \"parts\"]\n").unwrap();
    let src = |name: &str, text: &str| {
        std::fs::create_dir_all(dir.join(name).join("src")).unwrap();
        std::fs::write(dir.join(name).join("Cargo.toml"), format!("[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n")).unwrap();
        let path = dir.join(name).join("src/lib.rs");
        std::fs::write(&path, text).unwrap();
        path.to_string_lossy().into_owned()
    };
    let parts_src = "pub fn base() -> u32 { 11 }\npub enum Ev { Reset { n: u32 } }\npub struct Info(pub u32);\nimpl Info { pub fn new(n: u32) -> Info { Info(n) } }\n";
    let app_src = |call: &str| format!("pub fn value(out: &mut u32) {{ *out = parts::{call}(); }}\n");
    let (parts_path, app_path) = (src("parts", parts_src), src("app", &app_src("base")));
    let session = Arc::new(crate::Session::default());
    crate::pmhost::install();
    let mut prog = Program::new(&session);
    let (core, deps) = crate::load_std_crates(&session, &mut prog).unwrap();
    let cfg = crate::program_cfg(&session, &[]);
    let parts = prog.load_crate_deferred("parts", &parts_path, 2021, cfg.clone(), deps.clone()).unwrap();
    let mut app_deps = deps;
    app_deps.push((prog.syms.intern("parts"), parts));
    let app_crate = prog.load_crate_deferred("app", &app_path, 2021, cfg, app_deps).unwrap();
    prog.prelude_crate = Some(core);
    prog.resolve_imports();
    assert!(prog.errors.is_empty(), "{:?}", prog.errors);
    let value = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app_crate && prog.name(id) == "value").then_some(id)
    }).unwrap();
    let parts_file = prog.mods[prog.crates[parts as usize].root_mod as usize].file;
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let entry = crate::owner::call(crate::owner::Class::Program, || {
        let id = u.fn_id(crate::jit::FnKey::Inst(value, vec![]));
        u.entry(id)
    });
    let run = || {
        let mut result = 0u32;
        unsafe { crate::jit::call_jit(up, entry, &mut result as *mut u32 as u64) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        result
    };
    assert_eq!(run(), 11);
    let app_file = u.prog.def(value).file;
    let o = unsafe { apply(up, vec![
        Edit { file: parts_file, path: parts_path, src: format!("{parts_src}pub fn fresh() -> u32 {{ 29 }}\n").into_bytes() },
        Edit { file: app_file, path: app_path.clone(), src: app_src("fresh").into_bytes() },
    ]) };
    assert_eq!(o.rung, Rung::Patch, "a new fn and its first use in another crate, saved together: {:?}", o.lines);
    assert_eq!(run(), 29, "the dependent crate's new code calls the new fn");
    unsafe extern "C" fn no_panic(_: *mut *const u8, _: *mut usize) {}
    crate::provided::register("parts", Default::default(), Default::default(), Default::default(), Default::default(), no_panic, 0, &[]);
    let use_later = || vec![Edit { file: app_file, path: app_path.clone(), src: app_src("later").into_bytes() }];
    let o = unsafe { apply(up, use_later()) };
    assert_eq!(o.rung, Rung::Refuse, "a fn the provided crate lacks, with makepad's inputs unchanged: {:?}", o.lines);
    // makepad's inputs changed, and this process cannot build it (no loader package here)
    assert!(crate::reload::host::request("a provided crate changed".into(), Vec::new()).is_err(), "the test process has no makepad to rebuild");
    assert!(crate::reload::host::route() == crate::reload::host::Route::Plain, "nothing builds: the ring has no rebuild to offer");
    let o = unsafe { apply(up, use_later()) };
    assert!(o.lines.iter().any(|l| l.contains("E0425") && l.contains("needs the rebuilt makepad: restart to apply")), "once makepad's inputs changed, the use waits for the rebuilt makepad: {:?} {:?}", o.rung, o.lines);
    assert_eq!(run(), 29, "the running code stays until the restart");
    let read = |local: &str| vec![Edit { file: app_file, path: app_path.clone(), src: format!("{}{local}pub fn read(e: parts::Ev) -> u32 {{ match e {{ parts::Ev::Reset {{ n }} => parts::Info::new(n).width(), parts::Ev::Changed {{ info }} => parts::Info::from_bytes(info).0 }} }}\n", app_src("fresh")).into_bytes() }];
    let o = unsafe { apply(up, read("")) };
    assert!(!o.lines.iter().any(|l| l.contains("E0425")), "the binding of a pattern whose variant is not found still exists: {:?}", o.lines);
    for item in ["`Changed`", "`from_bytes`", "`width`"] {
        assert!(o.lines.iter().any(|l| l.contains("E0599") && l.contains(item) && l.contains("needs the rebuilt makepad: restart to apply")), "{item}, new in the provided crate's type, waits for the rebuilt makepad: {:?}", o.lines);
    }
    assert_eq!(o.rung, Rung::Restart, "new items of a provided crate's types: {:?}", o.lines);
    let o = unsafe { apply(up, read("pub struct Info;\nimpl Info { pub fn width(&self) -> u32 { 0 } }\n")) };
    assert_eq!(o.rung, Rung::Refuse, "`Info` names a type of the app too: its missing item is the edit's own error: {:?}", o.lines);
    assert_eq!(run(), 29, "the running code stays until the restart");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}
