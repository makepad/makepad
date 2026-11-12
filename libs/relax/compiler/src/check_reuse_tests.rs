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
    let out = crate::test_child::output(&mut cmd);
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

/// A glob import brings every name of its module, but a crate's check finds an item only under a
/// name its text writes: an item added there, by a name the crate with the glob does not write,
/// checks only the library again.
#[test]
fn a_new_pub_item_a_glob_brings_named_nowhere_above_checks_only_its_crate() {
    let bin = format!("use pkg::*;\n{BIN}");
    changed("pubglob", &[("lib.rs", LIB), ("bin.rs", &bin)], &[],
        |d| std::fs::write(d.join("lib.rs"), format!("{LIB}pub fn g() {{}}\n")).unwrap(), &[], 1);
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

/// A changed signature checks again the crates whose text names it.
#[test]
fn a_changed_pub_item_checks_the_crates_above() {
    changed("pubchange", &[("lib.rs", LIB), ("bin.rs", BIN)], &[],
        |d| std::fs::write(d.join("lib.rs"), LIB.replace("-> u32 { 1 }", "-> u64 { 1 }")).unwrap(), &[], 0);
}

/// A changed signature no crate above names, a method added to an impl, a doc line: only the
/// library is checked again.
#[test]
fn an_interface_edit_nothing_above_names_checks_only_its_crate() {
    let lib = format!("/// One.\npub fn g() -> u32 {{ 1 }}\npub struct S;\nimpl S {{\n    pub fn one(&self) -> u32 {{ 1 }}\n}}\n{LIB}");
    let bin = "fn main() { let x = pkg::f(); let y = pkg::S.one(); }\nfn unused_in_bin() {}\n";
    let files = [("lib.rs", lib.as_str()), ("bin.rs", bin)];
    changed("ifsig", &files, &[], |d| std::fs::write(d.join("lib.rs"), lib.replace("g() -> u32 { 1 }", "g() -> u64 { 1 }")).unwrap(), &[], 1);
    changed("ifmethod", &files, &[], |d| std::fs::write(d.join("lib.rs"), lib.replace("impl S {\n", "impl S {\n    pub fn two(&self) -> u32 { 2 }\n")).unwrap(), &[], 1);
    changed("ifdoc", &files, &[], |d| std::fs::write(d.join("lib.rs"), lib.replace("/// One.", "/// One, and more.\n/// A second line.")).unwrap(), &[], 1);
}

/// A verdict that stood after an interface change is stored with that interface: the next change
/// is compared against it.
#[test]
fn a_verdict_that_stood_is_checked_against_the_next_change() {
    if std::env::var_os(CHILD).is_some() { return; }
    let lib = format!("pub fn g() -> u32 {{ 1 }}\n{LIB}");
    let dir = package("stood", &[("lib.rs", &lib), ("bin.rs", BIN)]);
    let first = check_in_child(&dir, true, &[]);
    assert_eq!((first.hits, first.code), (0, 0), "{first:?}");
    let lib = lib.replace("g() -> u32", "g() -> u64");
    std::fs::write(dir.join("lib.rs"), &lib).unwrap();
    let stood = check_in_child(&dir, true, &[]);
    assert_eq!((stood.hits, &stood.diags), (1, &first.diags), "{stood:?}");
    std::fs::write(dir.join("lib.rs"), lib.replace("f() -> u32", "f() -> u64")).unwrap();
    let sig = check_in_child(&dir, true, &[]);
    let cold = check_in_child(&dir, false, &[]);
    assert_eq!((&sig.diags, sig.code, sig.hits), (&cold.diags, cold.code, 0), "reused {sig:?}, cold {cold:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Never stale: an auto trait through a private field. `Arc` to `Rc` in a private field of a pub
/// struct makes it not `Send` for a binary that names only a fn returning it.
#[test]
fn a_private_field_changing_an_auto_trait_checks_the_crates_above() {
    let lib = "pub struct S { inner: std::sync::Arc<u32> }\npub fn make() -> S { S { inner: std::sync::Arc::new(1) } }\nimpl S { pub fn get(&self) -> u32 { *self.inner } }\nfn unused_in_lib() {}\n";
    let bin = "fn needs_send<T: Send>(_: T) {}\nfn main() { needs_send(pkg::make()); }\nfn unused_in_bin() {}\n";
    changed("autotrait", &[("lib.rs", lib), ("bin.rs", bin)], &[], |d| std::fs::write(d.join("lib.rs"), lib.replace("std::sync::Arc", "std::rc::Rc")).unwrap(), &[], 0);
}

/// Never stale: a trait impl that inference and method resolution find for code that names only
/// the fns around the type. `impl From<A> for B` gone: `take_b(make_a().into())` fails; `impl
/// Display` gone: `format!("{}", make())` fails. An impl added for a type nothing above names
/// checks only the library.
#[test]
fn a_trait_impl_change_checks_the_crates_that_select_it() {
    let lib = "pub struct A;\npub struct B;\nimpl From<A> for B { fn from(_: A) -> B { B } }\npub fn make_a() -> A { A }\npub fn take_b(_: B) {}\nfn unused_in_lib() {}\n";
    let bin = "fn main() { pkg::take_b(pkg::make_a().into()); }\nfn unused_in_bin() {}\n";
    changed("fromimpl", &[("lib.rs", lib), ("bin.rs", bin)], &[], |d| std::fs::write(d.join("lib.rs"), lib.replace("impl From<A> for B { fn from(_: A) -> B { B } }\n", "")).unwrap(), &[], 0);
    let lib = "pub struct D;\nimpl std::fmt::Display for D { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str(\"d\") } }\npub fn make() -> D { D }\nfn unused_in_lib() {}\n";
    let bin = "fn main() { let s = format!(\"{}\", pkg::make()); }\nfn unused_in_bin() {}\n";
    changed("displayimpl", &[("lib.rs", lib), ("bin.rs", bin)], &[], |d| std::fs::write(d.join("lib.rs"), lib.replace("impl std::fmt::Display for D { fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result { f.write_str(\"d\") } }\n", "")).unwrap(), &[], 0);
    let lib = format!("pub struct Q;\n{LIB}");
    changed("cloneimpl", &[("lib.rs", &lib), ("bin.rs", BIN)], &[], |d| std::fs::write(d.join("lib.rs"), format!("{lib}impl Clone for Q {{ fn clone(&self) -> Q {{ Q }} }}\n")).unwrap(), &[], 1);
}

/// Never stale: blanket impls and coherence. A blanket impl gone: `5u8.hi()` through a glob (the
/// binary writes the method's name only) fails; a blanket impl added overlaps the binary's own impl.
#[test]
fn a_blanket_impl_change_checks_the_crates_above() {
    let lib = "pub trait Hi { fn hi(&self) -> u32 { 1 } }\nimpl<X: Copy> Hi for X {}\nfn unused_in_lib() {}\n";
    let bin = "use pkg::*;\nfn main() { let _ = 5u8.hi(); }\nfn unused_in_bin() {}\n";
    changed("blanket", &[("lib.rs", lib), ("bin.rs", bin)], &[], |d| std::fs::write(d.join("lib.rs"), lib.replace("impl<X: Copy> Hi for X {}\n", "")).unwrap(), &[], 0);
    let lib = "pub trait T {}\nfn unused_in_lib() {}\n";
    let bin = "struct Mine;\nimpl pkg::T for Mine {}\nfn main() { let _ = Mine; }\nfn unused_in_bin() {}\n";
    changed("overlap", &[("lib.rs", lib), ("bin.rs", bin)], &[], |d| std::fs::write(d.join("lib.rs"), format!("{lib}impl<X> T for X {{}}\n")).unwrap(), &[], 0);
}

/// Never stale: a const value and a type's size used in the binary's const and array length.
#[test]
fn a_const_value_or_size_change_checks_the_crates_above() {
    let lib = "pub const N: usize = 4;\nfn unused_in_lib() {}\n";
    let bin = "const M: usize = pkg::N * 2;\nfn main() { let a: [u8; M] = [0; 8]; let _ = a; }\nfn unused_in_bin() {}\n";
    changed("constval", &[("lib.rs", lib), ("bin.rs", bin)], &[], |d| std::fs::write(d.join("lib.rs"), lib.replace("= 4;", "= 5;")).unwrap(), &[], 0);
    let lib = "pub struct S { pub a: u32 }\nfn unused_in_lib() {}\n";
    let bin = "const SZ: usize = std::mem::size_of::<pkg::S>();\nfn main() { let a: [u8; SZ] = [0; 4]; let _ = a; }\nfn unused_in_bin() {}\n";
    changed("typesize", &[("lib.rs", lib), ("bin.rs", bin)], &[], |d| std::fs::write(d.join("lib.rs"), lib.replace("pub a: u32 }", "pub a: u32, pub b: u32 }")).unwrap(), &[], 0);
}

/// Never stale: a `macro_rules!` change reaching the binary through its expansion: the items it
/// expands to, and a fn its expansion calls that the binary's text does not name.
#[test]
fn a_macro_change_checks_the_crates_its_expansions_reach() {
    let lib = "#[macro_export]\nmacro_rules! make { () => { pub fn made() -> u32 { 1 } } }\nfn unused_in_lib() {}\n";
    let bin = "pkg::make!();\nfn main() { let x: u32 = made(); let _ = x; }\nfn unused_in_bin() {}\n";
    changed("macroitems", &[("lib.rs", lib), ("bin.rs", bin)], &[], |d| std::fs::write(d.join("lib.rs"), lib.replace("-> u32 { 1 }", "-> u64 { 1 }")).unwrap(), &[], 0);
    let lib = "pub fn helper() -> u32 { 1 }\n#[macro_export]\nmacro_rules! call { () => { $crate::helper() } }\nfn unused_in_lib() {}\n";
    let bin = "fn main() { let x: u32 = pkg::call!(); let _ = x; }\nfn unused_in_bin() {}\n";
    changed("macrocall", &[("lib.rs", lib), ("bin.rs", bin)], &[], |d| std::fs::write(d.join("lib.rs"), lib.replace("helper() -> u32 { 1 }", "helper() -> u64 { 1 }")).unwrap(), &[], 0);
}

/// The parent module declaration is unchanged: the attribute and its cache input
/// belong to the child file. A fresh process must see both removal and restoration.
#[test]
fn child_inner_macro_use_edits_invalidate_cached_scope() {
    if std::env::var_os(CHILD).is_some() { return; }
    let lib = "mod defs;\nmod user { pub fn f() -> u32 { value!() } }\npub use user::f;\n";
    let defs = "#![cfg_attr(all(), macro_use)]\nmacro_rules! value { () => { 7u32 }; }\n";
    let dir = package("inner-macro-use", &[("lib.rs", lib), ("defs.rs", defs), ("bin.rs", "fn main() { let _ = pkg::f(); }\n")]);
    let cache = dir.join("cache").to_string_lossy().into_owned();
    let env = [("MAKEPAD_RUN_CACHE_DIR", cache.as_str())];
    let first = check_in_child(&dir, true, &env);
    assert_eq!(first.code, 0, "{first:?}");
    let warm = check_in_child(&dir, true, &env);
    assert!(warm.code == 0 && warm.full, "{warm:?}");
    // The same-length child edit changes only whether macro_use is configured.
    std::fs::write(dir.join("defs.rs"), defs.replace("all()", "any()")).unwrap();
    let removed = check_in_child(&dir, true, &env);
    let cold_cache = dir.join("cold-cache").to_string_lossy().into_owned();
    let cold = check_in_child(&dir, false, &[("MAKEPAD_RUN_CACHE_DIR", cold_cache.as_str())]);
    assert_ne!(cold.code, 0, "the macro must no longer escape: {cold:?}");
    assert_eq!((&removed.diags, removed.code), (&cold.diags, cold.code), "{removed:?} vs {cold:?}");
    std::fs::write(dir.join("defs.rs"), defs).unwrap();
    let restored = check_in_child(&dir, true, &env);
    assert_eq!(restored.code, 0, "{restored:?}");
    let warm_again = check_in_child(&dir, true, &env);
    assert!(warm_again.code == 0 && warm_again.full, "{warm_again:?}");
    assert_eq!(std::fs::read_to_string(dir.join("lib.rs")).unwrap(), lib);
    std::fs::remove_dir_all(dir).unwrap();
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
/// after a late input (an include_str! file) changed; with nothing changed the record is printed,
/// the one a check that checked crates and stored all their verdicts left (cold, after an edit).
#[test]
fn an_edit_keeping_size_and_mtime_gets_a_fresh_verdict() {
    if std::env::var_os(CHILD).is_some() || std::env::var_os(EARLY_CHILD).is_some() { return; }
    let lib = "const DATA: &str = include_str!(\"data.txt\");\nconst _: () = assert!(DATA.len() == 2);\nfn unused_a() {}\n";
    let dir = package("early", &[("Cargo.toml", "[package]\nname = \"early_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n"), ("lib.rs", lib), ("data.txt", "ok")]);
    // (a certificate records a file's stat only once the file has been quiet that long)
    std::thread::sleep(std::time::Duration::from_nanos(crate::vfs::STAT_RACY_NS + 300_000_000));
    let check = || {
        // the child's workspace is the package folder it runs in: never the one an overlay run of
        // this test names (MAKEPAD_RUN_WORKSPACE, the lease's virtual source root); the early record
        // lives in the code cache, its own here, which a run of the suite with the cache off
        // (MAKEPAD_RUN_CACHE=off) does not turn off for the child
        let out = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "check_reuse_tests::early_child", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(EARLY_CHILD, &dir).env("MAKEPAD_RUN_CACHE_DIR", dir.join("cache")).env("MAKEPAD_RUN_CHECK_TIMES", "1")
            .env_remove("MAKEPAD_RUN_LINT_FLAGS").env_remove("MAKEPAD_RUN_WORKSPACE").env_remove("MAKEPAD_RUN_CACHE"));
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
    // the cold check stored every verdict: its output is the early record
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
    // the check after the edit stored its output as the early record
    let again = check();
    assert!(again.0 == Some(0) && again.1 == fresh.1 && again.2, "the edit's early record is printed: {}", again.3);
    std::fs::remove_dir_all(&dir).unwrap();
}

const APP_CHILD: &str = "RELAX_CHECK_APP_CHILD";

/// In a child (APP_CHILD names the workspace folder): `makepad check app_pkg` there (with
/// RELAX_CHECK_APP_WORKSPACE, `makepad check --workspace app_pkg`), as the command line runs it: the
/// checked crates load again, the crates below them come from the collected image.
#[test]
fn app_check_child() {
    let Some(dir) = std::env::var_os(APP_CHILD).map(std::path::PathBuf::from) else { return };
    std::env::set_var("MAKEPAD_RUN_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    std::env::set_current_dir(&dir).unwrap();
    let sess = std::sync::Arc::new(crate::Session::default());
    let mut args = vec!["--check".to_string()];
    if std::env::var_os("RELAX_CHECK_APP_WORKSPACE").is_some() { args.push("--workspace".to_string()); }
    args.push("app_pkg".to_string());
    let code = crate::run::cmd_run(&sess, &args);
    eprintln!("app child exit {code}");
}

/// The check of `app_pkg` in `dir` in a child (app_check_child; `workspace`: --workspace), its cache
/// in `dir`: (exit code, diagnostic lines, restored from the collected image, stderr).
fn app_check(dir: &std::path::Path, workspace: bool) -> (Option<i32>, Vec<String>, bool, String) {
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "check_reuse_tests::app_check_child", "--nocapture", "--test-threads=1", "--format=terse"])
        .env(APP_CHILD, dir).env("MAKEPAD_RUN_CACHE_DIR", dir.join("cache")).env("MAKEPAD_RUN_BOOT_TRACE", "1")
        .env_remove("MAKEPAD_RUN_LINT_FLAGS").env_remove("MAKEPAD_RUN_WORKSPACE").env_remove("MAKEPAD_RUN_CACHE").env_remove("RELAX_CHECK_APP_WORKSPACE");
    if workspace { cmd.env("RELAX_CHECK_APP_WORKSPACE", "1"); }
    let out = crate::test_child::output(&mut cmd);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && stdout.contains("1 passed"), "{stdout}\n{stderr}");
    let code = stderr.lines().find_map(|l| l.strip_prefix("app child exit ")).map(|c| c.parse::<i32>().unwrap());
    let diags: Vec<String> = stderr.lines().filter(|l| l.contains(": warning: ") || l.contains(": error") || l.contains(": E0")).map(str::to_string).collect();
    let restored = stderr.lines().any(|l| l.starts_with("makepad-run collected: restored at"));
    (code, diags, restored, stderr.into_owned())
}

/// A workspace `dep` + `app_pkg` (depending on it) in a new folder: `dep_src` and `app_src` its
/// libraries, `features` the app's dependency features (", features = [..]").
fn app_workspace(name: &str, dep_src: &str, app_src: &str, features: &str) -> std::path::PathBuf {
    let dir = package(name, &[("Cargo.toml", "[workspace]\nmembers = [\"dep\", \"app\"]\nresolver = \"2\"\n")]);
    std::fs::create_dir_all(dir.join("dep")).unwrap();
    std::fs::create_dir_all(dir.join("app")).unwrap();
    std::fs::write(dir.join("dep/Cargo.toml"), "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n[features]\nextra = []\n").unwrap();
    std::fs::write(dir.join("dep/lib.rs"), dep_src).unwrap();
    std::fs::write(dir.join("app/Cargo.toml"), app_toml(features)).unwrap();
    std::fs::write(dir.join("app/lib.rs"), app_src).unwrap();
    dir
}

fn app_toml(features: &str) -> String {
    format!("[package]\nname = \"app_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n[dependencies]\ndep = {{ path = \"../dep\"{features} }}\n")
}

/// The collected image certifies each crate it holds by its fingerprint: an app check restores the
/// crates below its checked ones after every edit of those (a check that restored them writes its
/// graph, not the image again), and no crate that changed since loads from it -- an edited
/// dependency, or one whose features changed (another graph): the verdicts are the sources'.
#[test]
fn an_app_check_restores_the_crates_its_image_holds_as_they_are() {
    if std::env::var_os(CHILD).is_some() || std::env::var_os(EARLY_CHILD).is_some() || std::env::var_os(APP_CHILD).is_some() { return; }
    let dep = |ty: &str| format!("pub fn value() -> {ty} {{ 7 }}\n#[cfg(feature = \"extra\")]\npub fn extra() -> u32 {{ 1 }}\n");
    let app = |unused: &str, body: &str| format!("pub fn top() -> u32 {{ {body} }}\nfn {unused}() {{}}\n");
    let dir = app_workspace("appimage", &dep("u32"), &app("unused_a", "dep::value() + 1"), "");
    let write = |f: &str, src: &str| std::fs::write(dir.join(f), src).unwrap();
    let check = || app_check(&dir, false);
    let named = |diags: &[String], name: &str| diags.iter().any(|d| d.contains(name));
    let cold = check();
    assert!(cold.0 == Some(0) && named(&cold.1, "unused_a") && !cold.2, "{}", cold.3);
    // two edits of the checked crate: both checks restore the crates below it from the image the
    // cold check recorded, the second after the first wrote its graph
    for (before, now) in [("unused_a", "unused_b"), ("unused_b", "unused_c")] {
        write("app/lib.rs", &app(now, "dep::value() + 1"));
        let r = check();
        assert!(r.0 == Some(0) && r.2 && named(&r.1, now) && !named(&r.1, before), "the edit to {now}: {}", r.3);
    }
    // an edit of a kept crate: it loads from its source (the app's sum no longer adds up), and back
    write("dep/lib.rs", &dep("u64"));
    let r = check();
    assert!(r.0.is_some_and(|c| c != 0) && named(&r.1, "E0308"), "{}", r.3);
    write("dep/lib.rs", &dep("u32"));
    write("app/lib.rs", &app("unused_d", "dep::value() + 1"));
    let r = check();
    assert!(r.0 == Some(0) && named(&r.1, "unused_d") && !named(&r.1, "E0308"), "{}", r.3);
    // a feature of a kept crate turned on: another graph, nothing restores from the image
    write("app/Cargo.toml", &app_toml(", features = [\"extra\"]"));
    write("app/lib.rs", &app("unused_d", "dep::value() + dep::extra()"));
    let r = check();
    assert!(r.0 == Some(0) && !r.2 && named(&r.1, "unused_d") && !named(&r.1, "extra"), "{}", r.3);
    // and off again: the crate the image holds is the one without it
    write("app/Cargo.toml", &app_toml(""));
    let r = check();
    assert!(r.0.is_some_and(|c| c != 0) && r.2 && named(&r.1, "extra"), "{}", r.3);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A workspace check loads every crate it checks again -- their lints read the uses their
/// resolution records (a dependency's import used in a body is no unused import) -- and restores
/// the others (std's) from the collected image.
#[test]
fn a_workspace_check_restores_the_crates_it_does_not_check() {
    if std::env::var_os(CHILD).is_some() || std::env::var_os(EARLY_CHILD).is_some() || std::env::var_os(APP_CHILD).is_some() { return; }
    let dep = |text: &str| format!("use std::fmt::Write;\nuse std::collections::HashMap;\npub fn value(s: &mut String) -> u32 {{ s.write_str(\"{text}\").unwrap(); 7 }}\n");
    let app = |unused: &str| format!("pub fn top() -> u32 {{ dep::value(&mut String::new()) + 1 }}\nfn {unused}() {{}}\n");
    let dir = app_workspace("wsimage", &dep("x"), &app("unused_a"), "");
    let named = |diags: &[String], name: &str| diags.iter().any(|d| d.contains(name));
    let cold = app_check(&dir, true);
    assert!(cold.0 == Some(0) && !cold.2 && named(&cold.1, "HashMap") && named(&cold.1, "unused_a") && !named(&cold.1, "Write"), "{}", cold.3);
    // both crates edited: they load again, std is restored from the image the cold check recorded
    std::fs::write(dir.join("dep/lib.rs"), dep("y")).unwrap();
    std::fs::write(dir.join("app/lib.rs"), app("unused_b")).unwrap();
    let r = app_check(&dir, true);
    assert!(r.0 == Some(0) && r.2 && named(&r.1, "HashMap") && named(&r.1, "unused_b") && !named(&r.1, "unused_a") && !named(&r.1, "Write"), "{}", r.3);
    std::fs::remove_dir_all(&dir).unwrap();
}
