use std::sync::Arc;
use crate::program::Program;

#[test]
fn deferred_reexport_signatures() {
    // Forced selection and signature-miss collection are process-global. Each
    // case gets a fresh process, as do the runtime fixtures below.
    const CHILD: &str = "MAKEPAD_RUN_REEXPORT_SIGNATURE_TEST";
    let Ok(case) = std::env::var(CHILD) else {
        for case in ["plain", "partial"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::deferred_reexport_signatures", "--nocapture", "--test-threads=1"])
                .env(CHILD, case)
                .env("MAKEPAD_RUN_LAZY_DEFER", "lazy_reexport/child.rs,lazy_reexport/other.rs")
                .output().unwrap();
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
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::gandalf_live_edit_guard_and_reporting", "--nocapture", "--test-threads=1", "--format=terse"])
                .env(CHILD, case).env("MAKEPAD_RUN_GANDALF_ALL", if case.ends_with("all") { "1" } else { "0" })
                .output().unwrap();
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

/// SPEC-V: a patch runs before its verdict as baseline code with `noalias` dropped; a rejected one
/// is never promoted, is reported and reverts; a clean one is promoted with its `noalias` facts.
#[test]
fn specv_patch_before_verdict() {
    const CHILD: &str = "MAKEPAD_RUN_SPECV_TEST";
    let Ok(case) = std::env::var(CHILD) else {
        for case in ["bad", "clean"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::specv_patch_before_verdict", "--nocapture", "--test-threads=1", "--format=terse"])
                .env(CHILD, case).env("MAKEPAD_RUN_GANDALF_ALL", "0")
                .output().unwrap();
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
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::specv_aliased_call_from_unverified_body", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0")
            .output().unwrap();
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
            let output = cmd.output().unwrap();
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
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::d3_disjoint_check_lowered_from_the_verdict", "--nocapture", "--test-threads=1", "--format=terse"])
                .env(CHILD, case).env("MAKEPAD_RUN_GANDALF_ALL", "0")
                .output().unwrap();
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
        for case in ["app_lib", "path_crate", "framework", "embedded", "generic_embedded", "pointer", "bound_template", "bound_template_embedded", "derive_default"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::app_scope_live_layout_edits", "--nocapture", "--test-threads=1"])
                .env(CHILD, case).output().unwrap();
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
    let before = r#"
        pub struct State { pub value: u64 }
        pub struct Dormant { pub n: u32 }
        pub fn init(out: &mut State) { *out = State { value: 41 }; }
        pub fn value(out: &mut State) { out.value += 1; }
        pub fn name(out: &mut &str) { *out = std::any::type_name::<State>(); }
    "#;
    std::fs::write(&path, before).unwrap();
    let mut session = crate::Session::default();
    if case == "derive_default" { session.options.live = true; }
    let session = Arc::new(session);
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
    let mut roots = vec![bin_path.to_string_lossy().into_owned()];
    if case != "framework" { roots.push(path.to_string_lossy().into_owned()); }
    crate::borrowck::event::set_app_scope(roots);
    crate::layout::set_root_crate(bin);
    crate::layout::LIVE_SESSION.store(true, std::sync::atomic::Ordering::Relaxed);
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    assert!(tcx.errors.is_empty(), "{:?}", tcx.errors);
    let ty = tcx.tys.intern(TyKind::Adt(state, tcx.tys.mk_list(&[]), RegionArgsId::EMPTY));
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
    let layout = u.layout_of(ty).clone();
    assert_eq!(layout.ext.is_some(), case != "framework");
    if case == "derive_default" {
        let after = before.replace("pub value: u64 }", "pub value: u64, #[rust(make(vm))] pub later: u64 }")
            .replace("State { value: 41 }", "State { value: 41, later: 5 }")
            + "\nfn make(_: u32) -> u64 { 5 }\n";
        let restart = unsafe { reload::apply(up, vec![Edit { file: u.prog.def(state).file, path: path.to_string_lossy().into_owned(), src: after.as_bytes().to_vec() }]) };
        assert_eq!(restart.rung, Rung::Restart, "{:?}", restart.lines);
        assert!(restart.lines.iter().any(|l| l.contains("#[rust(..)]") && l.contains("restart the app")), "{:?}", restart.lines);
        let after = after.replace("make(vm)", "make(3)");
        let patch = unsafe { reload::apply(up, vec![Edit { file: u.prog.def(state).file, path: path.to_string_lossy().into_owned(), src: after.as_bytes().to_vec() }]) };
        assert_eq!(patch.rung, Rung::Patch, "{:?}", patch.lines);
        let after = after + "\npub struct Fresh { #[rust(make(vm))] pub n: u64 }\n";
        let patch = unsafe { reload::apply(up, vec![Edit { file: u.prog.def(state).file, path: path.to_string_lossy().into_owned(), src: after.into_bytes() }]) };
        assert_eq!(patch.rung, Rung::Patch, "{:?}", patch.lines);
        std::mem::forget(u);
        std::fs::remove_dir_all(dir).unwrap();
        return;
    }
    let mut old_value = vec![0u64; (layout.size as usize + 7) / 8];
    let call = |entry, arg| unsafe { jit::call_jit(up, entry, arg) }.unwrap_or_else(|p| panic!("JIT: {}", p.message));
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
    let after = before.replace("pub value: u64 }", "pub value: u64, pub extra: u64 }")
        .replace("State { value: 41 }", "State { value: 41, extra: 0 }")
        .replace("out.value += 1;", "assert_eq!(out.extra, 0); out.extra = 7; out.value += 1; obsolete_style();")
        // Generated child styles register their own function entry. Removing the child
        // removes this helper too; its old body must not be checked against the new names.
        + "\nfn obsolete_style() { let _entry = obsolete_style as fn(); }\n";
    let patch = unsafe { reload::apply(up, vec![Edit { file: u.prog.def(state).file, path: path.to_string_lossy().into_owned(), src: after.into_bytes() }]) };
    // A struct outside the app scope keeps its layout fixed: restart. One embedded by value in a
    // framework type (`Holder`, like the platform's `Option<App>`) patches: its first layout stays.
    if case == "framework" {
        assert_eq!(patch.rung, Rung::Restart, "{:?}", patch.lines);
        assert!(patch.lines.iter().any(|l| l.contains("in the app scope") && l.contains("restart the app")), "{:?}", patch.lines);
        call(value, old_value.as_mut_ptr() as u64);
    } else {
        assert_eq!(patch.rung, Rung::Patch, "{:?}", patch.lines);
        call(value, old_value.as_mut_ptr() as u64); // old allocation receives extra's default
        let removed = unsafe { reload::apply(up, vec![Edit { file: u.prog.def(state).file, path: path.to_string_lossy().into_owned(), src: before.as_bytes().to_vec() }]) };
        assert_eq!(removed.rung, Rung::Patch, "{:?}", removed.lines);
        call(value, old_value.as_mut_ptr() as u64);
        assert_eq!(old_value[layout.fields[0] as usize / 8], 44);
        // The previously compiled drop entry was rebuilt in the same transaction.
        assert!(patch.names.contains(&u.fns[drop_id as usize].name.to_string()));
        call(drop_entry, old_value.as_mut_ptr() as u64);
        // A layout-only edit must publish even when no function entry changed.
        let dormant = before.replace("pub n: u32 }", "pub n: u32, pub added: bool }");
        let pure = unsafe { reload::apply(up, vec![Edit { file: u.prog.def(state).file, path: path.to_string_lossy().into_owned(), src: dormant.into_bytes() }]) };
        assert_eq!(pure.rung, Rung::Patch, "{:?}", pure.lines);
        assert!(pure.names.is_empty(), "{:?}", pure.names);

    }
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
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::restored_images_take_live_edits", "--nocapture", "--test-threads=1"])
            .env(CHILD, "1").output().unwrap();
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

/// std::relax::realtime for arming a machine (FABARMS): `compiled` is the strict ready (the whole
/// reach compiled, never "the wait ran out"), `lazy` counts what would still compile on a call, and
/// an armed root freezes live edits of real-time code (refused "frozen", applied at the disarm)
/// while edits of other code patch as usual.
#[test]
fn realtime_strict_ready_lazy_and_patch_freeze() {
    const CHILD: &str = "MAKEPAD_RUN_RT_FREEZE_TEST";
    if std::env::var(CHILD).is_err() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_strict_ready_lazy_and_patch_freeze", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0")
            .output().unwrap();
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
    while !crate::jit::__relax_realtime_compiled(ticket) {
        assert!(crate::jit::__relax_realtime_lazy(ticket) > 0, "not compiled, yet nothing lazy");
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

/// FABARMS3's hole: the marked root calls through `dyn Route` (its value holds no trait object),
/// and other code makes the first `Box<Ded> -> Box<dyn Route>` after the mark, and a second type's
/// after the reach is done. The root's lowering names the trait object it calls through, so the
/// reach follows its vtables: the strict ready is false the moment a new one is made (polled
/// right after the unsizing code ran), true once its methods are compiled, and the real-time
/// thread then meets no lazy compile.
#[test]
fn realtime_reach_follows_vtables_made_after_the_mark() {
    const CHILD: &str = "MAKEPAD_RUN_RT_DYN_TEST";
    if std::env::var(CHILD).is_err() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_reach_follows_vtables_made_after_the_mark", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0")
            .output().unwrap();
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
    let wait = |t| {
        for _ in 0..5000 {
            if compiled(t) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("the real-time reach was not compiled in 10 s");
    };
    // the mark, then at once the first unsizing (FABARMS3's order), polled right after it ran
    let ticket = crate::owner::call(crate::owner::Class::Program, || unsafe { crate::jit::realtime_mark_slots(up, vec![root]) });
    call(start_e);
    assert!(!compiled(ticket) || next_built(), "compiled right after the first `Box<Ded> -> Box<dyn Route>`, with `<Ded as Route>::next` lazy");
    wait(ticket);
    assert!(next_built(), "compiled, with `<Ded as Route>::next` lazy");
    assert_eq!(run_rt(cp), (4, 0), "the real-time thread met a lazy compile");
    // a second type's vtable, made after the reach is done: false at once, until it is compiled
    call(switch_e);
    assert!(!compiled(ticket) || next_built(), "compiled right after the first `Box<Arc2> -> Box<dyn Route>`, with `<Arc2 as Route>::next` lazy");
    wait(ticket);
    assert_eq!(run_rt(cp), (42, 0), "the real-time thread met a lazy compile");
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
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_reach_follows_trait_objects_held_deep_in_the_roots_value", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0")
            .output().unwrap();
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
    for _ in 0..5000 {
        if crate::jit::__relax_realtime_compiled(ticket) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(crate::jit::__relax_realtime_compiled(ticket), "the real-time reach was not compiled in 10 s");
    let (out, misses) = std::thread::spawn(move || {
        crate::jit::__relax_realtime_thread();
        let before = crate::jit::__relax_realtime_misses();
        unsafe { crate::jit::call_jit(upn as *mut crate::jit::Unit, run_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        (unsafe { *(cp as *const u32) }, crate::jit::__relax_realtime_misses() - before)
    }).join().unwrap();
    assert_eq!((out, misses), (8, 0), "the real-time thread met lazy compiles (MAKEPAD_RUN_RT_TRACE=1 names them)");
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
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_reach_skips_trait_objects_it_only_makes_itself", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0")
            .output().unwrap();
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
    for _ in 0..5000 {
        if crate::jit::__relax_realtime_compiled(ticket) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(crate::jit::__relax_realtime_compiled(ticket), "the real-time reach was not compiled in 10 s");
    assert!(!crate::jit::realtime_reached(helper), "the reach took in `helper`, which only a closure of other code calls (every vtable of `dyn FnMut(u32)` followed)");
    let (out, misses) = std::thread::spawn(move || {
        crate::jit::__relax_realtime_thread();
        let before = crate::jit::__relax_realtime_misses();
        unsafe { crate::jit::call_jit(upn as *mut crate::jit::Unit, run_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        (unsafe { *(cp as *const u32) }, crate::jit::__relax_realtime_misses() - before)
    }).join().unwrap();
    assert_eq!((out, misses), (6, 0), "the real-time thread met lazy compiles (MAKEPAD_RUN_RT_TRACE=1 names them)");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// FABARMS3's misses on a node (W1207: 24 lazy compiles on the real-time thread, drop glue of generic
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
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_reach_walks_code_that_ran_before_the_mark", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0")
            .output().unwrap();
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
    for _ in 0..5000 {
        if crate::jit::__relax_realtime_compiled(ticket) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(crate::jit::__relax_realtime_compiled(ticket), "the real-time reach was not compiled in 10 s");
    let (out, misses) = std::thread::spawn(move || {
        crate::jit::__relax_realtime_thread();
        let before = crate::jit::__relax_realtime_misses();
        unsafe { crate::jit::call_jit(upn as *mut crate::jit::Unit, run_e, cp) }.unwrap_or_else(|p| panic!("JIT call panicked: {}", p.message));
        (unsafe { *((cp + 4) as *const u32) }, crate::jit::__relax_realtime_misses() - before)
    }).join().unwrap();
    assert_eq!((out, misses), (3 + 5, 0), "the real-time thread met lazy compiles (MAKEPAD_RUN_RT_TRACE=1 names them)");
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}

/// What FABARMS3's misses on a node (W1207) were, on a cold store: the marked root formats floats
/// (core::fmt's float path is reached through the formatter fn pointer an `Argument` holds, data the
/// reach reads) and drops containers (`Vec`, `vec::IntoIter`, `array::IntoIter`, `Weak`: drop glue
/// of generic types). Nothing else ran before the mark: the reach alone compiles them, and the
/// real-time thread meets no lazy compile (the narrowed trait-object reach keeps this).
#[test]
fn realtime_reach_covers_float_formatting_and_container_drops() {
    const CHILD: &str = "MAKEPAD_RUN_RT_FMT_TEST";
    if std::env::var(CHILD).is_err() {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::realtime_reach_covers_float_formatting_and_container_drops", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0")
            .output().unwrap();
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
    for _ in 0..5000 {
        if crate::jit::__relax_realtime_compiled(ticket) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    assert!(crate::jit::__relax_realtime_compiled(ticket), "the real-time reach was not compiled in 10 s");
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
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::live_new_module_types_edit", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0")
            .output().unwrap();
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
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::a_renamed_field_reads_generic_fns_as_typeck_left_them", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0")
            .output().unwrap();
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
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "lazy_reload_tests::a_live_edit_reaches_both_crates_of_a_lib_test_root", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(CHILD, "1").env("MAKEPAD_RUN_GANDALF_ALL", "0")
            .output().unwrap();
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
