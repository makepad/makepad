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
        for case in ["app", "framework", "all", "std", "std_all"] {
            let output = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "lazy_reload_tests::gandalf_live_edit_guard_and_reporting", "--nocapture", "--test-threads=1", "--format=terse"])
                .env(CHILD, case).env("MAKEPAD_RUN_GANDALF_ALL", if case.ends_with("all") { "1" } else { "0" })
                .output().unwrap();
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(output.status.success(), "{case}: {stdout}\n{stderr}");
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
    crate::borrowck::lazy::disable_gandalf();
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
    let target = if case == "app" { app_crate } else { framework_crate };
    let def = prog.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == target && prog.name(id) == "value").then_some(id)
    }).unwrap();
    let path = if case == "app" { app } else { framework };
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    crate::borrowck::event::set_app_scope(roots);
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let entry = crate::owner::call(crate::owner::Class::App, || {
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
    let outcome = unsafe { crate::reload::apply(up, vec![crate::reload::Edit { file: old_file, path: path.clone(), src: bad.to_vec() }]) };
    if matches!(case.as_str(), "framework" | "std") {
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
        for case in ["app_lib", "path_crate", "framework", "embedded", "generic_embedded", "pointer", "bound_template", "bound_template_embedded"] {
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
        crate::owner::call(crate::owner::Class::App, || {
            let id = u.fn_id(FnKey::Inst(d, vec![]));
            u.entry(id)
        })
    };
    let init = entry(&mut u, init);
    let value = entry(&mut u, value);
    let name = entry(&mut u, name);
    let layout = u.layout_of(ty).clone();
    assert_eq!(layout.ext.is_some(), case != "framework");
    let mut old_value = vec![0u64; (layout.size as usize + 7) / 8];
    let call = |entry, arg| unsafe { jit::call_jit(up, entry, arg) }.unwrap_or_else(|p| panic!("JIT: {}", p.message));
    call(init, old_value.as_mut_ptr() as u64);
    call(value, old_value.as_mut_ptr() as u64);
    assert_eq!(old_value[layout.fields[0] as usize / 8], 42);
    let (drop_id, drop_entry) = crate::owner::call(crate::owner::Class::App, || {
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
    if matches!(case.as_str(), "framework" | "embedded" | "generic_embedded" | "bound_template_embedded") {
        assert_eq!(patch.rung, Rung::Restart, "{:?}", patch.lines);
        let reason = if case == "framework" { "in the app scope" } else { "embedded by value" };
        assert!(patch.lines.iter().any(|l| l.contains(reason) && l.contains("restart the app")), "{:?}", patch.lines);
        if case != "framework" { assert!(patch.lines.iter().any(|l| l.contains("framework::Holder") && l.contains("outside the app scope"))); }
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
        classify(&p, file, &v.pf, &a_path)
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
    std::fs::write(&path, "pub struct S { pub a: u32 }\nimpl S { pub fn get(&self) -> u32 { self.a } }\npub fn value(out: &mut u32) { *out = S { a: 11 }.get(); }\n").unwrap();
    let session = Arc::new(crate::Session::default());
    let (mut prog, core, app) = crate::load_with_std(&session, &path, "restored", false).unwrap();
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
    let def = restored.defs.iter().enumerate().find_map(|(i, d)| {
        let id = crate::program::DefId(i as u32);
        (d.krate == app && restored.name(id) == "value").then_some(id)
    }).unwrap();
    drop(prog);
    let mut u = crate::jit::Unit::new(restored, restored_tcx, core);
    let up = &mut *u as *mut crate::jit::Unit;
    let entry = crate::owner::call(crate::owner::Class::App, || {
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
        crate::reload::apply(up, vec![crate::reload::Edit { file: (*up).prog.def(def).file, path: path.clone(), src: src.as_bytes().to_vec() }])
    };
    let body = edit("pub struct S { pub a: u32 }\nimpl S { pub fn get(&self) -> u32 { self.a + 1 } }\npub fn value(out: &mut u32) { *out = S { a: 22 }.get(); }\n");
    assert_eq!(body.rung, crate::reload::Rung::Patch, "{:?}", body.lines);
    run(23);
    let added = edit("pub struct S { pub a: u32 }\nimpl S { pub fn get(&self) -> u32 { self.a + extra() } }\npub fn extra() -> u32 { 9 }\npub fn value(out: &mut u32) { *out = S { a: 22 }.get(); }\n");
    assert_eq!(added.rung, crate::reload::Rung::Patch, "{:?}", added.lines);
    run(31);
    // Like settle_tests: process exit owns the runtime worker/executable-memory teardown.
    std::mem::forget(u);
    std::fs::remove_dir_all(dir).unwrap();
}
