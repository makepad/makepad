//! check_reuse.rs: a check that reuses verdicts prints what a check of every crate prints, warm
//! and after each kind of input change, and checks again the crates the change can reach. Each
//! check runs in a child process (the lint sink is the process's) on a package on disk: crate
//! `pkg` (lib.rs, the root) and crate `bin0` (bin.rs, a binary that uses it), the verdict store in
//! the package's `store` folder.

const CHILD: &str = "RELAX_CHECK_REUSE_CHILD";

/// One check's output: its diagnostic lines (as printed), how many crates it reused, whether it
/// reused all (nothing checked), its exit code.
#[derive(Debug)]
struct Run { diags: Vec<String>, hits: usize, full: bool, code: i32 }

/// The check of the package in `dir` in a child (`reuse_child`), with the store when `reuse`,
/// the variables `env` set.
fn check_in_child(dir: &std::path::Path, reuse: bool, env: &[(&str, &str)]) -> Run {
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "check_reuse_tests::reuse_child", "--nocapture", "--test-threads=1", "--format=terse"])
        .env(CHILD, dir).env_remove("MAKEPAD_RUN_LINT_FLAGS").env_remove("RELAX_CHECK_REUSE_STORE");
    if reuse { cmd.env("RELAX_CHECK_REUSE_STORE", dir.join("store")); }
    for (k, v) in env { cmd.env(k, v); }
    let out = cmd.output().unwrap();
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && stdout.contains("1 passed"), "{stdout}\n{stderr}");
    let mut run = Run { diags: Vec::new(), hits: 0, full: false, code: -1 };
    for l in stderr.lines().chain(stdout.lines()) {
        if l.contains(": warning: ") || l.contains(": error: ") || l.contains(": E0") { run.diags.push(l.to_string()); }
        // "N of M crates unchanged since a passing check" (some checked), "M crates unchanged .." (none)
        if let Some((n, _)) = l.strip_prefix("makepad check: ").and_then(|r| r.split_once(" crates unchanged since a passing check")) {
            match n.split_once(" of ") {
                Some((h, _)) => run.hits = h.parse().unwrap(),
                None => (run.hits, run.full) = (n.parse().unwrap(), true),
            }
        }
        if let Some(c) = l.strip_prefix("reuse child exit ") { run.code = c.parse().unwrap(); }
    }
    run
}

/// In a child (CHILD names the package folder): `makepad check` of the package, as run.rs runs
/// it -- the verdicts looked up after the load, the stored diagnostics printed when every crate is
/// unchanged, else the check reusing them. RELAX_CHECK_REUSE_FEATURES: the root's features.
#[test]
fn reuse_child() {
    let Some(dir) = std::env::var_os(CHILD).map(std::path::PathBuf::from) else { return };
    crate::jit::CHECK_FIRST.store(false, std::sync::atomic::Ordering::Relaxed);
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    let mut sess = crate::Session::default();
    sess.options.const_fuel = true;
    let sess = std::sync::Arc::new(sess);
    relax_types::macros::uses::record(true);
    relax_front::program::set_record_import_uses(true);
    crate::borrowck::event::set_check_scope();
    let mut prog = crate::program::Program::new(&sess);
    let (core, std_deps) = crate::load_std_crates(&sess, &mut prog).unwrap();
    let features: Vec<String> = std::env::var("RELAX_CHECK_REUSE_FEATURES").unwrap_or_default().split(',').filter(|f| !f.is_empty()).map(str::to_string).collect();
    let root = prog.load_crate_deferred("pkg", dir.join("lib.rs").to_str().unwrap(), 2021, crate::program_cfg(&sess, &features), std_deps.clone()).unwrap();
    let mut deps = std_deps.clone();
    deps.push((prog.syms.intern("pkg"), root));
    let bin = prog.load_crate_deferred("bin0", dir.join("bin.rs").to_str().unwrap(), 2021, crate::program_cfg(&sess, &[]), deps).unwrap();
    prog.crates[bin as usize].is_bin = true;
    let crates = vec![root, bin];
    prog.prelude_crate = Some(core);
    prog.resolve_imports();
    let pre = std::mem::take(&mut prog.errors);
    let mut reuse = std::env::var_os("RELAX_CHECK_REUSE_STORE").map(|s| crate::check_reuse::Reuse::open_in(&prog, root, &crates, std::path::Path::new(&s), true));
    if let Some(r) = reuse.as_ref().filter(|r| r.full()) {
        eprintln!("reuse child exit {}", crate::check_reuse::print_reused_texts(&r.reused_texts(&prog), r.crates()));
        return;
    }
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    if let Some(r) = reuse.as_mut() { r.sig_clean = tcx.errors.is_empty(); }
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let code = match reuse.as_mut() {
        Some(r) => crate::check::check_crate_reusing(&mut u, root, &crates, pre, r),
        None => crate::check::check_crate(&mut u, root, &crates, pre, false),
    };
    eprintln!("reuse child exit {code}");
    std::mem::forget(u);
}

/// A package folder with `files` (lib.rs, bin.rs, ..), empty store.
fn package(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("relax-check-reuse-{}-{}", name, std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for (f, src) in files { std::fs::write(dir.join(f), src).unwrap(); }
    dir
}

/// The check before an input change (storing verdicts), and after it: what the check after the
/// change prints is what a check of every crate prints then, and it reuses `hits` crates.
fn changed(name: &str, files: &[(&str, &str)], before: &[(&str, &str)], change: impl FnOnce(&std::path::Path), after: &[(&str, &str)], hits: usize) {
    if std::env::var_os(CHILD).is_some() { return; }
    let dir = package(name, files);
    let first = check_in_child(&dir, true, before);
    assert_eq!(first.hits, 0, "{first:?}");
    change(&dir);
    let reused = check_in_child(&dir, true, after);
    let cold = check_in_child(&dir, false, after);
    assert!(cold.code >= 0 && !cold.diags.is_empty(), "{cold:?}");
    assert_eq!((&reused.diags, reused.code), (&cold.diags, cold.code), "after the change: reused {reused:?}, cold {cold:?}");
    assert_eq!(reused.hits, hits, "{reused:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

const LIB: &str = "pub fn f() -> u32 { 1 }\nfn unused_in_lib() {}\n";
const BIN: &str = "fn main() { let x = pkg::f(); }\nfn unused_in_bin() {}\n";

/// Warm: every crate unchanged, the stored diagnostics are the check's, nothing is checked; a
/// body edit in the library (its lines moved) checks the library again, not the binary above it.
#[test]
fn a_dependency_body_edit_checks_only_that_dependency() {
    if std::env::var_os(CHILD).is_some() { return; }
    let dir = package("body", &[("lib.rs", LIB), ("bin.rs", BIN)]);
    let first = check_in_child(&dir, true, &[]);
    assert_eq!((first.hits, first.code), (0, 0), "{first:?}");
    assert_eq!(first.diags.len(), 3, "{first:?}");
    let warm = check_in_child(&dir, true, &[]);
    assert!(warm.full && warm.hits == 2, "{warm:?}");
    assert_eq!((&warm.diags, warm.code), (&first.diags, first.code));
    std::fs::remove_dir_all(&dir).unwrap();
    changed("body2", &[("lib.rs", LIB), ("bin.rs", BIN)], &[],
        |d| std::fs::write(d.join("lib.rs"), "pub fn f() -> u32 {\n    2\n}\nfn unused_in_lib() {}\n").unwrap(), &[], 1);
}

/// A public item added to the library, by a name nothing above it can have seen (no glob brings
/// the library's names, no module it sees through globs binds the name): only the library is
/// checked again.
#[test]
fn a_new_pub_item_nothing_can_see_checks_only_its_crate() {
    changed("pubitem", &[("lib.rs", LIB), ("bin.rs", BIN)], &[],
        |d| std::fs::write(d.join("lib.rs"), format!("{LIB}pub fn g() {{}}\n")).unwrap(), &[], 1);
}

/// A glob import brings every name of its module: an item added there checks the crate with the
/// glob again.
#[test]
fn a_new_pub_item_a_glob_brings_checks_the_crate_with_the_glob() {
    let bin = format!("use pkg::*;\n{BIN}");
    changed("pubglob", &[("lib.rs", LIB), ("bin.rs", &bin)], &[],
        |d| std::fs::write(d.join("lib.rs"), format!("{LIB}pub fn g() {{}}\n")).unwrap(), &[], 0);
}

/// An added item that shadows a name its module's glob import brought (`pkg::g` named the
/// glob's `g`, now the new one): every crate above it is checked again.
#[test]
fn a_new_pub_item_shadowing_a_glob_name_checks_the_crates_above() {
    let lib = format!("pub mod inner {{ pub fn g() -> u32 {{ 1 }} }}\npub use inner::*;\n{LIB}");
    let bin = "fn main() { let x = pkg::f(); let y = pkg::g(); }\nfn unused_in_bin() {}\n";
    changed("pubshadow", &[("lib.rs", &lib), ("bin.rs", bin)], &[],
        |d| std::fs::write(d.join("lib.rs"), format!("{lib}pub fn g() -> u32 {{ 2 }}\n")).unwrap(), &[], 0);
}

/// An added item whose body defines an impl adds more than its name (the impl is global): the
/// crates above it are checked again.
#[test]
fn a_new_pub_item_defining_an_impl_checks_the_crates_above() {
    let lib = format!("{LIB}pub struct W;\n");
    changed("pubimpl", &[("lib.rs", &lib), ("bin.rs", BIN)], &[],
        |d| std::fs::write(d.join("lib.rs"), format!("{lib}pub fn g() {{ impl W {{ pub fn two(&self) -> u32 {{ 2 }} }} }}\n")).unwrap(), &[], 0);
}

/// A changed signature is no added item: the crates above it are checked again.
#[test]
fn a_changed_pub_item_checks_the_crates_above() {
    changed("pubchange", &[("lib.rs", LIB), ("bin.rs", BIN)], &[],
        |d| std::fs::write(d.join("lib.rs"), LIB.replace("-> u32 { 1 }", "-> u64 { 1 }")).unwrap(), &[], 0);
}

/// A feature is in the crate's cfg: flipped, the library (and so the binary) is checked again.
#[test]
fn a_feature_flip_checks_again() {
    changed("feature", &[("lib.rs", &format!("{LIB}#[cfg(feature = \"x\")]\nfn gated() {{}}\n")), ("bin.rs", BIN)], &[], |_| {}, &[("RELAX_CHECK_REUSE_FEATURES", "x")], 0);
}

/// An `env!` value is a late input of the library: changed, it and the binary are checked again.
#[test]
fn an_env_value_change_checks_again() {
    let lib = format!("{LIB}pub const V: &str = env!(\"RELAX_CHECK_REUSE_V\");\n");
    changed("env", &[("lib.rs", &lib), ("bin.rs", BIN)], &[("RELAX_CHECK_REUSE_V", "a")], |_| {}, &[("RELAX_CHECK_REUSE_V", "b")], 0);
}

/// An `include_str!` file is a late input: edited, the crates reading it are checked again.
#[test]
fn an_included_file_edit_checks_again() {
    let lib = format!("{LIB}pub const T: &str = include_str!(\"data.txt\");\n");
    changed("include", &[("lib.rs", &lib), ("bin.rs", BIN), ("data.txt", "one")], &[], |d| std::fs::write(d.join("data.txt"), "two").unwrap(), &[], 0);
}

/// A build script's output (code `include!`d from OUT_DIR): edited, its warnings are the new code's.
#[test]
fn a_build_output_edit_checks_again() {
    let lib = format!("{LIB}include!(concat!(env!(\"RELAX_CHECK_REUSE_OUT\"), \"/gen.rs\"));\n");
    let changed_out = |d: &std::path::Path| std::fs::write(d.join("gen.rs"), "fn generated_two() {}\n").unwrap();
    // (the package's folder stands for OUT_DIR)
    let out = std::env::temp_dir().join(format!("relax-check-reuse-buildout-{}", std::process::id())).display().to_string();
    changed("buildout", &[("lib.rs", &lib), ("bin.rs", BIN), ("gen.rs", "fn generated_one() {}\n")], &[("RELAX_CHECK_REUSE_OUT", &out)], changed_out, &[("RELAX_CHECK_REUSE_OUT", &out)], 0);
}

/// A command-line lint level is in every key: set, every crate is checked again under it.
#[test]
fn a_lint_flag_checks_again() {
    changed("lintflag", &[("lib.rs", LIB), ("bin.rs", BIN)], &[], |_| {}, &[("MAKEPAD_RUN_LINT_FLAGS", "-A dead_code")], 0);
}

/// Under `-D warnings` a warning fails the check: nothing is stored, the next check checks all.
#[test]
fn a_check_denying_its_warnings_stores_nothing() {
    changed("denywarn", &[("lib.rs", LIB), ("bin.rs", BIN)], &[("MAKEPAD_RUN_LINT_FLAGS", "-D warnings")], |_| {}, &[("MAKEPAD_RUN_LINT_FLAGS", "-D warnings")], 0);
}

const EARLY_CHILD: &str = "RELAX_CHECK_EARLY_CHILD";

/// In a child (EARLY_CHILD names the package folder): `makepad check --workspace early_pkg` there,
/// as the command line runs it (run.rs: the early record before the load).
#[test]
fn early_child() {
    let Some(dir) = std::env::var_os(EARLY_CHILD).map(std::path::PathBuf::from) else { return };
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    std::env::set_current_dir(&dir).unwrap();
    let sess = std::sync::Arc::new(crate::Session::default());
    let code = crate::run::cmd_run(&sess, &["--check".to_string(), "--workspace".to_string(), "early_pkg".to_string()]);
    eprintln!("early child exit {code}");
}

/// The early record (check_reuse.rs Early) stands only while the load's graph certificate does:
/// a check after an edit that keeps the file's size and mtime prints a fresh verdict, as does one
/// after a late input (an include_str! file) changed; with nothing changed the record is printed.
#[test]
fn an_edit_keeping_size_and_mtime_gets_a_fresh_verdict() {
    if std::env::var_os(CHILD).is_some() || std::env::var_os(EARLY_CHILD).is_some() { return; }
    let lib = "const DATA: &str = include_str!(\"data.txt\");\nconst _: () = assert!(DATA.len() == 2);\nfn unused_a() {}\n";
    let dir = package("early", &[("Cargo.toml", "[package]\nname = \"early_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n"), ("lib.rs", lib), ("data.txt", "ok")]);
    // (a certificate records a file's stat only once the file has been quiet that long)
    std::thread::sleep(std::time::Duration::from_nanos(crate::vfs::STAT_RACY_NS + 300_000_000));
    let check = || {
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "check_reuse_tests::early_child", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(EARLY_CHILD, &dir).env("MAKEPAD_RUN_CACHE_DIR", dir.join("cache")).env("MAKEPAD_RUN_CHECK_TIMES", "1")
            .env_remove("MAKEPAD_RUN_LINT_FLAGS").output().unwrap();
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && stdout.contains("1 passed"), "{stdout}\n{stderr}");
        let code = stderr.lines().find_map(|l| l.strip_prefix("early child exit ")).map(|c| c.parse::<i32>().unwrap());
        let diags: Vec<String> = stderr.lines().filter(|l| l.contains(": warning: ") || l.contains(": error") || l.contains(": E0")).map(str::to_string).collect();
        let replayed = stderr.lines().any(|l| l.starts_with("makepad check: early record (") && l.ends_with(": current"));
        (code, diags, replayed, stderr.into_owned())
    };
    let named = |diags: &[String], name: &str| diags.iter().any(|d| d.contains(name));
    let cold = check();
    assert!(cold.0 == Some(0) && named(&cold.1, "unused_a") && !cold.2, "{}", cold.3);
    // every verdict reused: that output is stored as the early record
    let warm = check();
    assert!(warm.0 == Some(0) && warm.1 == cold.1 && !warm.2 && warm.3.contains("their verdicts reused"), "{}", warm.3);
    let early = check();
    assert!(early.0 == Some(0) && early.1 == cold.1 && early.2, "the early record is printed: {}", early.3);
    // a late input changed: checked again (the assertion fails)
    std::fs::write(dir.join("data.txt"), "bad").unwrap();
    let late = check();
    assert!(late.0 != Some(0) && !late.2 && late.1.iter().any(|d| d.contains("E0080")), "{}", late.3);
    std::fs::write(dir.join("data.txt"), "ok").unwrap();
    let back = check();
    assert!(back.0 == Some(0) && back.1 == cold.1, "{}", back.3);
    // same size, mtime restored: the stat differs in its ctime only
    let path = dir.join("lib.rs");
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    std::fs::write(&path, lib.replace("unused_a", "unused_b")).unwrap();
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(mtime).unwrap();
    assert_eq!(std::fs::metadata(&path).unwrap().len() as usize, lib.len());
    let fresh = check();
    assert!(fresh.0 == Some(0) && !fresh.2 && named(&fresh.1, "unused_b") && !named(&fresh.1, "unused_a"), "a fresh verdict: {}", fresh.3);
    std::fs::remove_dir_all(&dir).unwrap();
}
