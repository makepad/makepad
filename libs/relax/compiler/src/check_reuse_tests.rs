//! check_reuse.rs: a check that reuses verdicts prints what a check of every crate prints, warm
//! and after each kind of input change, and checks again the crates the change can reach. Each
//! check runs in a child process (the lint sink is the process's) on a package on disk: crate
//! `pkg` (lib.rs, the root) and crate `bin0` (bin.rs, a binary that uses it), the verdict store in
//! the package's `store` folder. The check of every crate it is compared with verifies the stored
//! body verdicts (check_bodies.rs, Verify mode): one that differs from its body's check fails it.

const CHILD: &str = "RELAX_CHECK_REUSE_CHILD";

/// One check's output: its diagnostic lines (as printed), how many crates it reused, whether it
/// reused all (nothing checked), how many bodies it replayed from stored verdicts, how many
/// stored body verdicts it verified, how many replayed bodies it checked after all for an import
/// report their stored uses decided and how many of those candidates that cleared, how many item
/// cells of a checked crate a body computed first, its exit code.
#[derive(Debug)]
struct Run { diags: Vec<String>, hits: usize, full: bool, replayed: usize, verified: usize, rechecked: usize, cleared: usize, in_body: usize, code: i32 }

/// The check of the package in `dir` in a child (`reuse_child`), with the store when `reuse`,
/// the variables `env` set.
fn check_in_child(dir: &std::path::Path, reuse: bool, env: &[(&str, &str)]) -> Run {
    check_in_child_store(dir, reuse, &dir.join("store"), env)
}

/// A child check with an explicit verdict store, so separate workspaces can use the same one.
fn check_in_child_store(dir: &std::path::Path, reuse: bool, store: &std::path::Path, env: &[(&str, &str)]) -> Run {
    let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "check_reuse_tests::reuse_child", "--nocapture", "--test-threads=1", "--format=terse"])
        .env(CHILD, dir).env_remove("RELAX_LINT_FLAGS").env_remove("RELAX_CHECK_REUSE_STORE").env_remove("RELAX_CACHE");
    if reuse { cmd.env("RELAX_CHECK_REUSE_STORE", store); }
    for (k, v) in env { cmd.env(k, v); }
    let out = crate::test_child::output(&mut cmd);
    let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    assert!(out.status.success() && stdout.contains("1 passed"), "{stdout}\n{stderr}");
    let mut run = Run { diags: Vec::new(), hits: 0, full: false, replayed: 0, verified: 0, rechecked: 0, cleared: 0, in_body: 0, code: -1 };
    for l in stderr.lines().chain(stdout.lines()) {
        if l.contains(": warning: ") || l.contains(": error: ") || l.contains(": E0") || l.contains("internal: ") { run.diags.push(l.to_string()); }
        let count = |what: &str| l.strip_prefix("relax check: ").and_then(|r| r.split_once(what)).and_then(|(n, _)| n.split(' ').next()?.parse().ok());
        if let Some(n) = count(" bodies replayed from stored verdicts") { run.replayed = n; }
        if let Some(n) = count(" stored body verdicts verified") { run.verified = n; }
        if let Some((_, r)) = l.strip_prefix("relax check: unused-import candidates in ").and_then(|r| r.split_once(" with replayed bodies: ")) {
            run.rechecked = r.split(' ').next().unwrap().parse().unwrap();
        }
        if let Some(n) = l.strip_prefix("relax check: unused-import candidates cleared by a recheck: ") { run.cleared = n.parse().unwrap(); }
        if let Some(n) = l.strip_prefix("relax check: items resolved in a body: ") { run.in_body = n.split(' ').next().unwrap().parse().unwrap(); }
        // "N of M crates unchanged since a passing check" (some checked), "M crates unchanged .." (none)
        if let Some((n, _)) = l.strip_prefix("relax check: ").and_then(|r| r.split_once(" crates unchanged since a passing check")) {
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
    std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
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
    // name lookups memoized across bodies, as in the check (jit.rs: the unit's program)
    prog.body_memos.store(true, std::sync::atomic::Ordering::Relaxed);
    let pre = std::mem::take(&mut prog.errors);
    // the store's mode as a check reads it: RELAX_CACHE=verify, a Verify check
    let mode = if std::env::var("RELAX_CACHE").as_deref() == Ok("verify") { crate::cache::Mode::Verify } else { crate::cache::Mode::On };
    let mut reuse = std::env::var_os("RELAX_CHECK_REUSE_STORE").map(|s| crate::check_reuse::Reuse::open_in(&prog, root, &crates, std::path::Path::new(&s), mode));
    if let Some(r) = reuse.as_ref().filter(|r| r.full()) {
        eprintln!("reuse child exit {}", crate::check_reuse::print_reused_texts(&r.reused_texts(&prog), r.crates()));
        return;
    }
    let mut tcx = crate::tcx::Tcx::new();
    tcx.collect(&prog);
    if let Some(r) = reuse.as_mut() { r.sig_clean = tcx.errors.is_empty(); }
    let mut u = crate::jit::Unit::new(prog, tcx, core);
    let code = match reuse.as_mut() {
        Some(r) => crate::check::check_crate_reusing(&mut u, root, &crates, pre, r).0,
        None => crate::check::check_crate(&mut u, root, &crates, pre, false).0,
    };
    // a check memoizes every lookup with the import uses it went through: no hit can take an
    // answer that holds none for "no uses" (memo_hit looks such an answer up again)
    let unrecorded = u.prog.scope_cache.snapshot().values().filter(|v| v.1.is_none()).count()
        + u.prog.glob_cache.snapshot().values().filter(|v| v.1.is_none()).count();
    assert_eq!(unrecorded, 0, "memo answers without their import uses");
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

/// The check reusing verdicts, and the check of every crate verifying the stored body verdicts
/// (with the variables `env`, and RELAX_CACHE=verify: the store's mode is no part of the
/// keys it looks up): they print the same.
fn reused_and_verified(dir: &std::path::Path, env: &[(&str, &str)], what: &str) -> (Run, Run) {
    let reused = check_in_child(dir, true, env);
    let verify: Vec<(&str, &str)> = env.iter().copied().chain([("RELAX_CACHE", "verify")]).collect();
    let cold = check_in_child(dir, true, &verify);
    assert!(cold.code >= 0 && !cold.diags.is_empty() && cold.hits == 0, "{what}: {cold:?}");
    assert_eq!((&reused.diags, reused.code), (&cold.diags, cold.code), "{what}: reused {reused:?}, cold {cold:?}");
    (reused, cold)
}

/// The check before an input change (storing verdicts), and after it: what the check after the
/// change prints is what a check of every crate prints then, and it reuses `hits` crates.
fn changed(name: &str, files: &[(&str, &str)], before: &[(&str, &str)], change: impl FnOnce(&std::path::Path), after: &[(&str, &str)], hits: usize) {
    if std::env::var_os(CHILD).is_some() { return; }
    let dir = package(name, files);
    let first = check_in_child(&dir, true, before);
    assert_eq!(first.hits, 0, "{first:?}");
    change(&dir);
    let (reused, _) = reused_and_verified(&dir, after, "after the change");
    assert_eq!(reused.hits, hits, "{reused:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

const LIB: &str = "pub fn f() -> u32 { 1 }\nfn unused_in_lib() {}\n";
const BIN: &str = "fn main() { let x = pkg::f(); }\nfn unused_in_bin() {}\n";

/// A verdict stored in one worktree never prints another worktree's paths; when Target B
/// anchors the stored texts this test's hits become 2 with the same diagnostics.
#[test]
fn a_copy_in_another_worktree_prints_its_own_paths() {
    if std::env::var_os(CHILD).is_some() { return; }
    let lib = format!("{LIB}fn another_unused() {{}}\n");
    let files = [("lib.rs", lib.as_str()), ("bin.rs", BIN), ("Cargo.toml", "[workspace]\n")];
    let a = package("wt-a", &files);
    let b = package("wt-b", &files);
    let store = a.join("store");
    let (a_path, b_path) = (a.to_str().unwrap(), b.to_str().unwrap());
    let first = check_in_child_store(&a, true, &store, &[("RELAX_WORKSPACE", a_path)]);
    assert_eq!((first.hits, first.code), (0, 0), "{first:?}");
    let env = [("RELAX_WORKSPACE", b_path)];
    let reused = check_in_child_store(&b, true, &store, &env);
    let cold = check_in_child_store(&b, false, &store, &env);
    assert_eq!((reused.hits, reused.code, cold.code), (0, 0, 0), "{reused:?}, {cold:?}");
    assert_eq!(reused.diags, cold.diags, "{reused:?}, {cold:?}");
    assert!(reused.diags.iter().any(|d| d.contains(b_path)), "{reused:?}");
    assert!(reused.diags.iter().all(|d| !d.contains(a_path)), "{reused:?}");
    std::fs::remove_dir_all(&a).unwrap();
    std::fs::remove_dir_all(&b).unwrap();
}

/// Warm: every crate unchanged, the stored diagnostics are the check's, nothing is checked; a
/// body edit in the library (its lines moved) checks the library again, not the binary above it.
/// Of the library, only the edited body and the one with a warning (its facts have a position)
/// are checked: the others are replayed from their stored verdicts, whose references, imports
/// and traits used keep `helper` and the imports used. An attribute on a parent module is in
/// every body verdict's key: removed, no body is replayed and the one it allowed warns.
/// Two bodies name one import and one glob's name: the first one checked looks them up, the other
/// takes the memoized answers, and records the uses they went through as well. Edited, the first
/// one stops using them, and the other, replayed, keeps them used (one job: cur_a checks first).
/// A replayed verdict that lacks the uses (a test hook drops them) prints no false warning: the
/// imports it would call unused hold the reports back, the crate's replayed bodies are checked
/// after all, and the candidates clear. With the last uses edited away, the warnings print once.
/// Item text a binary body computes first (the library's generics, bounds and signature, with the
/// library reused, so its own check did not compute them) records its import uses in no body's
/// verdict: the binary's verdicts stored then are the ones a check of every crate makes.
#[test]
fn a_dependency_body_edit_checks_only_that_dependency() {
    if std::env::var_os(CHILD).is_some() { return; }
    let lib = "use std::collections::HashMap;\nuse std::fmt::Write;\npub fn f() -> u32 { 1 }\nfn helper() -> usize { 2 }\n\
        pub fn uses() -> usize {\n    let mut s = String::new();\n    s.write_str(\"x\").unwrap();\n    HashMap::<u8, u8>::new().len() + helper() + s.len()\n}\n\
        pub fn warned() -> usize { let unused = 1; 2 }\n#[allow(unused_variables)]\npub mod quiet {\n    pub fn hidden() -> usize { let unused = 1; 2 }\n}\n";
    let dir = package("body", &[("lib.rs", lib), ("bin.rs", BIN)]);
    let first = check_in_child(&dir, true, &[]);
    assert_eq!((first.hits, first.replayed, first.code), (0, 0, 0), "{first:?}");
    assert_eq!(first.diags.len(), 3, "{first:?}");
    let warm = check_in_child(&dir, true, &[]);
    assert!(warm.full && warm.hits == 2, "{warm:?}");
    assert_eq!((&warm.diags, warm.code), (&first.diags, first.code));
    std::fs::write(dir.join("lib.rs"), lib.replace("pub fn f() -> u32 { 1 }", "pub fn f() -> u32 {\n    2\n}")).unwrap();
    let (reused, cold) = reused_and_verified(&dir, &[], "a body edit");
    assert_eq!((reused.hits, reused.replayed, cold.verified), (1, 3, 5), "a body edit: {reused:?}, {cold:?}");
    std::fs::write(dir.join("lib.rs"), lib.replace("#[allow(unused_variables)]\n", "")).unwrap();
    let (reused, cold) = reused_and_verified(&dir, &[], "a parent module's attribute removed");
    assert_eq!((reused.hits, reused.replayed, cold.verified), (1, 0, 3), "a parent module's attribute removed: {reused:?}, {cold:?}");
    std::fs::remove_dir_all(&dir).unwrap();
    let uses = format!("use std::io::Cursor;\nuse std::sync::*;\n{LIB}pub fn cur_a() -> u64 {{ Cursor::new(Arc::new(1u8)).position() }}\n\
        pub fn cur_b() -> u64 {{ Cursor::new(Arc::new(2u8)).position() }}\n");
    let one = [("RELAX_JOBS", "1")];
    let dir = package("body-uses", &[("lib.rs", &uses), ("bin.rs", BIN)]);
    let first = check_in_child(&dir, true, &one);
    assert_eq!((first.hits, first.code), (0, 0), "{first:?}");
    std::fs::write(dir.join("lib.rs"), uses.replace("Cursor::new(Arc::new(1u8)).position()", "0")).unwrap();
    let (reused, cold) = reused_and_verified(&dir, &one, "the body that looked up an import first edited");
    // replayed: f, unused_in_lib, cur_b; verified: those, cur_a's new verdict, the binary's unused_in_bin
    assert_eq!((reused.hits, reused.replayed, cold.verified), (1, 3, 5), "the body that looked up an import first edited: {reused:?}, {cold:?}");
    assert_eq!(reused.rechecked, 0, "no import report waits on a replayed body: {reused:?}");
    std::fs::write(dir.join("lib.rs"), uses.replace("Cursor::new(Arc::new(1u8)).position()", "1")).unwrap();
    let dropped = [("RELAX_JOBS", "1"), ("RELAX_CHECK_REUSE_DROP_USES", "cur_b")];
    let (reused, _) = reused_and_verified(&dir, &dropped, "a replayed verdict without its uses");
    // both imports candidates; f, unused_in_lib and cur_b checked after all
    assert_eq!((reused.replayed, reused.rechecked, reused.cleared), (3, 3, 2), "a replayed verdict without its uses: {reused:?}");
    std::fs::write(dir.join("lib.rs"), uses.replace("Cursor::new(Arc::new(1u8)).position()", "1").replace("Cursor::new(Arc::new(2u8)).position()", "2")).unwrap();
    let (reused, cold) = reused_and_verified(&dir, &one, "the last uses of two imports edited away");
    // f, unused_in_lib and cur_a (stored by the check before) replayed, then checked after all
    assert_eq!((reused.replayed, reused.rechecked, reused.cleared), (3, 3, 0), "the last uses of two imports edited away: {reused:?}");
    assert_eq!(cold.diags.iter().filter(|d| d.contains("unused import")).count(), 2, "{cold:?}");
    std::fs::remove_dir_all(&dir).unwrap();
    let lib = "use std::fmt::Debug;\nuse std::io::Cursor;\npub fn show<T: Debug>(t: T) -> Cursor<T> { Cursor::new(t) }\n";
    let bin = "fn a() -> u64 { pkg::show(1u8).position() }\nfn b() -> u64 { pkg::show(2u8).position() }\nfn main() { a(); b(); }\nfn unused_in_bin() {}\n";
    let dir = package("item-uses", &[("lib.rs", lib), ("bin.rs", bin)]);
    let first = check_in_child(&dir, true, &one);
    assert_eq!((first.hits, first.code, first.diags.len()), (0, 0, 1), "{first:?}");
    let bin = format!("{bin}pub fn g() {{}}\n");
    std::fs::write(dir.join("bin.rs"), &bin).unwrap();
    let (reused, cold) = reused_and_verified(&dir, &one, "the binary's interface edited");
    // checked: the binary's five bodies, `a` computing `show`'s item text first; verified: those and `show`
    assert_eq!((reused.hits, reused.replayed, cold.verified), (1, 0, 6), "the binary's interface edited: {reused:?}, {cold:?}");
    std::fs::write(dir.join("bin.rs"), bin.replace("pkg::show(1u8).position()", "0")).unwrap();
    let (reused, cold) = reused_and_verified(&dir, &one, "the body that computed the library's item text first edited");
    assert_eq!((reused.hits, reused.replayed, cold.verified), (1, 4, 6), "the body that computed the library's item text first edited: {reused:?}, {cold:?}");
    std::fs::remove_dir_all(&dir).unwrap();
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
    let env = [("RELAX_CACHE_DIR", cache.as_str())];
    let first = check_in_child(&dir, true, &env);
    assert_eq!(first.code, 0, "{first:?}");
    let warm = check_in_child(&dir, true, &env);
    assert!(warm.code == 0 && warm.full, "{warm:?}");
    // The same-length child edit changes only whether macro_use is configured.
    std::fs::write(dir.join("defs.rs"), defs.replace("all()", "any()")).unwrap();
    let removed = check_in_child(&dir, true, &env);
    let cold_cache = dir.join("cold-cache").to_string_lossy().into_owned();
    let cold = check_in_child(&dir, false, &[("RELAX_CACHE_DIR", cold_cache.as_str())]);
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
    let lib = format!("{LIB}pub const V: &str = env!(\"CHECK_REUSE_V\");\n");
    changed("env", &[("lib.rs", &lib), ("bin.rs", BIN)], &[("CHECK_REUSE_V", "a")], |_| {}, &[("CHECK_REUSE_V", "b")], 0);
}

/// An `include_str!` file is a late input: edited, the crates reading it are checked again.
/// Undoing the edit reuses the verdicts stored for its earlier value.
#[test]
fn an_included_file_edit_checks_again() {
    let lib = format!("{LIB}pub const T: &str = include_str!(\"data.txt\");\n");
    changed("include", &[("lib.rs", &lib), ("bin.rs", BIN), ("data.txt", "one")], &[], |d| std::fs::write(d.join("data.txt"), "two").unwrap(), &[], 0);
    if std::env::var_os(CHILD).is_some() { return; }
    let dir = package("include-undo", &[("lib.rs", &lib), ("bin.rs", BIN), ("data.txt", "one")]);
    let first = check_in_child(&dir, true, &[]);
    assert_eq!((first.hits, first.code), (0, 0), "{first:?}");
    std::fs::write(dir.join("data.txt"), "two").unwrap();
    let edited = check_in_child(&dir, true, &[]);
    assert_eq!(edited.hits, 0, "{edited:?}");
    std::fs::write(dir.join("data.txt"), "one").unwrap();
    let undone = check_in_child(&dir, true, &[]);
    assert!(undone.full && undone.hits == 2, "undoing a late input's change finds the verdicts of its earlier value: {undone:?}");
    assert_eq!((&undone.diags, undone.code), (&first.diags, first.code));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A late input only a hole body reads (a fn body no caller's check sees: not const, not async, no
/// `impl Trait` return) is the library's own: changed, the library is checked again, not the
/// binary above it, and the body that read it is checked again too (its stored verdict rests on
/// the value: here an array length's). Read in a const fn's body, or by an item as well (the hole's
/// read of the same file does not hide it), it reaches the binary, whose array length rests on it.
/// A body that includes a file (`include!`) has no stored verdict: an edit of another body of its
/// crate replays the others, and checks it again. The package's variable is an ordinary one: every
/// `RELAX_*` variable steers the whole check (env_key).
#[test]
fn a_late_input_only_a_body_reads_checks_only_its_crate() {
    let hole = format!("{LIB}pub fn g() -> usize {{\n    let a: [u8; include_str!(\"data.txt\").len()] = [0; 3];\n    a.len() + env!(\"CHECK_REUSE_V\").len()\n}}\n");
    changed("late-hole", &[("lib.rs", &hole), ("bin.rs", BIN), ("data.txt", "one")], &[("CHECK_REUSE_V", "a")],
        |d| std::fs::write(d.join("data.txt"), "four").unwrap(), &[("CHECK_REUSE_V", "b")], 1);
    let konst = format!("{LIB}pub const fn g() -> usize {{ env!(\"CHECK_REUSE_V\").len() }}\n");
    let bin = "fn main() { let x = pkg::f(); let a: [u8; pkg::g()] = [0; 1]; }\nfn unused_in_bin() {}\n";
    changed("late-const-fn", &[("lib.rs", &konst), ("bin.rs", bin)], &[("CHECK_REUSE_V", "a")], |_| {}, &[("CHECK_REUSE_V", "bb")], 0);
    let both = format!("{LIB}pub const N: usize = include!(\"n.in\");\npub fn g() -> usize {{ include_str!(\"n.in\").len() }}\n");
    let bin = "fn main() { let x = pkg::f(); let a: [u8; pkg::N] = [0; 1]; }\nfn unused_in_bin() {}\n";
    changed("late-both", &[("lib.rs", &both), ("bin.rs", bin), ("n.in", "1")], &[], |d| std::fs::write(d.join("n.in"), "2").unwrap(), &[], 0);
    if std::env::var_os(CHILD).is_some() { return; }
    let inc = format!("{LIB}pub fn h() -> usize {{\n    include!(\"n.in\")\n}}\npub fn k() -> usize {{ 1 }}\n");
    let dir = package("late-include", &[("lib.rs", &inc), ("bin.rs", BIN), ("n.in", "1")]);
    let first = check_in_child(&dir, true, &[]);
    assert_eq!((first.hits, first.replayed), (0, 0), "{first:?}");
    std::fs::write(dir.join("lib.rs"), inc.replace("pub fn k() -> usize { 1 }", "pub fn k() -> usize { 2 }")).unwrap();
    let (reused, cold) = reused_and_verified(&dir, &[], "a body edit beside an include!");
    // the library checked, the binary reused; of the library's bodies f and unused_in_lib are
    // replayed (k was edited, h includes a file); verified: those two, k's verdict the reused
    // check stored, and the binary's unused_in_bin (main's facts have a position)
    assert_eq!((reused.hits, reused.replayed, cold.verified), (1, 2, 4), "a body edit beside an include!: {reused:?}, {cold:?}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A build script's output (code `include!`d from OUT_DIR, at item position): edited, its warnings
/// are the new code's. Its text is the library's interface text: the binary, which names none of
/// its items, is not checked again; one whose array length names a changed const is.
#[test]
fn a_build_output_edit_checks_again() {
    let lib = format!("{LIB}include!(concat!(env!(\"CHECK_REUSE_OUT\"), \"/gen.rs\"));\n");
    let changed_out = |d: &std::path::Path| std::fs::write(d.join("gen.rs"), "fn generated_two() {}\n").unwrap();
    // (the package's folder stands for OUT_DIR)
    let out = std::env::temp_dir().join(format!("relax-check-reuse-buildout-{}", std::process::id())).display().to_string();
    changed("buildout", &[("lib.rs", &lib), ("bin.rs", BIN), ("gen.rs", "fn generated_one() {}\n")], &[("CHECK_REUSE_OUT", &out)], changed_out, &[("CHECK_REUSE_OUT", &out)], 1);
    let lib = format!("{LIB}include!(\"gen.rs\");\n");
    let bin = "fn main() { let x = pkg::f(); let a: [u8; pkg::N] = [0; 1]; }\nfn unused_in_bin() {}\n";
    changed("buildout-const", &[("lib.rs", &lib), ("bin.rs", bin), ("gen.rs", "pub const N: usize = 1;\n")], &[],
        |d| std::fs::write(d.join("gen.rs"), "pub const N: usize = 2;\n").unwrap(), &[], 0);
}

/// A command-line lint level is in every key: set, every crate is checked again under it.
#[test]
fn a_lint_flag_checks_again() {
    changed("lintflag", &[("lib.rs", LIB), ("bin.rs", BIN)], &[], |_| {}, &[("RELAX_LINT_FLAGS", "-A dead_code")], 0);
}

/// Under `-D warnings` a warning fails the check: nothing is stored, the next check checks all.
#[test]
fn a_check_denying_its_warnings_stores_nothing() {
    changed("denywarn", &[("lib.rs", LIB), ("bin.rs", BIN)], &[("RELAX_LINT_FLAGS", "-D warnings")], |_| {}, &[("RELAX_LINT_FLAGS", "-D warnings")], 0);
}

const EARLY_CHILD: &str = "RELAX_CHECK_EARLY_CHILD";

/// In a child (EARLY_CHILD names the package folder): `makepad check --workspace early_pkg` there,
/// as the command line runs it (run.rs: the early record before the load).
#[test]
fn early_child() {
    let Some(dir) = std::env::var_os(EARLY_CHILD).map(std::path::PathBuf::from) else { return };
    std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
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
        // this test names (RELAX_WORKSPACE, the lease's virtual source root); the early record
        // lives in the code cache, its own here, which a run of the suite with the cache off
        // (RELAX_CACHE=off) does not turn off for the child
        let out = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "check_reuse_tests::early_child", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(EARLY_CHILD, &dir).env("RELAX_CACHE_DIR", dir.join("cache")).env("RELAX_CHECK_TIMES", "1")
            .env_remove("RELAX_LINT_FLAGS").env_remove("RELAX_WORKSPACE").env_remove("RELAX_CACHE"));
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && stdout.contains("1 passed"), "{stdout}\n{stderr}");
        let code = stderr.lines().find_map(|l| l.strip_prefix("early child exit ")).map(|c| c.parse::<i32>().unwrap());
        let diags: Vec<String> = stderr.lines().filter(|l| l.contains(": warning: ") || l.contains(": error") || l.contains(": E0")).map(str::to_string).collect();
        let replayed = stderr.lines().any(|l| l.starts_with("relax check: early record (") && l.ends_with(": current"));
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

/// In a child (APP_CHILD names the workspace folder): `makepad check app_pkg` there, as the command
/// line runs it (verbose: the type counts of the image it records and restores): the checked crates
/// load again, the crates below them come from the collected image.
#[test]
fn app_check_child() {
    let Some(dir) = std::env::var_os(APP_CHILD).map(std::path::PathBuf::from) else { return };
    std::env::set_var("RELAX_STD", concat!(env!("CARGO_MANIFEST_DIR"), "/std"));
    std::env::set_current_dir(&dir).unwrap();
    let sess = std::sync::Arc::new(crate::Session::default());
    let code = crate::run::cmd_run(&sess, &["--verbose".to_string(), "--check".to_string(), "app_pkg".to_string()]);
    eprintln!("app child exit {code}");
}

/// The collected image certifies each crate it holds by its fingerprint: an app check restores the
/// crates below its checked ones after every edit of those (a check that restored them writes its
/// graph, not the image again), and no crate that changed since loads from it -- an edited
/// dependency, or one whose features changed (another graph): the verdicts are the sources'. The
/// image is the program before its bodies were checked: it holds none of the types they made.
#[test]
fn an_app_check_restores_the_crates_its_image_holds_as_they_are() {
    if std::env::var_os(CHILD).is_some() || std::env::var_os(EARLY_CHILD).is_some() || std::env::var_os(APP_CHILD).is_some() { return; }
    let dir = package("appimage", &[("Cargo.toml", "[workspace]\nmembers = [\"dep\", \"app\"]\nresolver = \"2\"\n")]);
    let write = |f: &str, src: &str| {
        let path = dir.join(f);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, src).unwrap();
    };
    let dep = |ty: &str| format!("pub fn value() -> {ty} {{ 7 }}\n#[cfg(feature = \"extra\")]\npub fn extra() -> u32 {{ 1 }}\n");
    let app_toml = |features: &str| format!("[package]\nname = \"app_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n[dependencies]\ndep = {{ path = \"../dep\"{features} }}\n");
    // the nested tuple is a type only the body's check makes
    let app = |unused: &str, body: &str| format!("pub fn top() -> u32 {{ let t: (u8, (u16, (u32, (u64, i8)))) = (1, (2, (3, (4, 5)))); {body} + t.1.1.0 }}\nfn {unused}() {{}}\n");
    write("dep/Cargo.toml", "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n[features]\nextra = []\n");
    write("dep/lib.rs", &dep("u32"));
    write("app/Cargo.toml", &app_toml(""));
    write("app/lib.rs", &app("unused_a", "dep::value() + 1"));
    // (exit code, diagnostic lines, restored from the collected image, stderr)
    let check = || {
        let out = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "check_reuse_tests::app_check_child", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(APP_CHILD, &dir).env("RELAX_CACHE_DIR", dir.join("cache")).env("RELAX_BOOT_TRACE", "1")
            .env_remove("RELAX_LINT_FLAGS").env_remove("RELAX_WORKSPACE").env_remove("RELAX_CACHE"));
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && stdout.contains("1 passed"), "{stdout}\n{stderr}");
        let code = stderr.lines().find_map(|l| l.strip_prefix("app child exit ")).map(|c| c.parse::<i32>().unwrap());
        let diags: Vec<String> = stderr.lines().filter(|l| l.contains(": warning: ") || l.contains(": error") || l.contains(": E0")).map(str::to_string).collect();
        let restored = stderr.lines().any(|l| l.starts_with("relax collected: restored at"));
        (code, diags, restored, stderr.into_owned())
    };
    let named = |diags: &[String], name: &str| diags.iter().any(|d| d.contains(name));
    // the number before `before` in the stderr line that starts with `prefix` and holds `before`
    let count = |stderr: &str, prefix: &str, before: &str| -> usize {
        let line = stderr.lines().find(|l| l.starts_with(prefix) && l.contains(before)).unwrap_or_else(|| panic!("no `{prefix}` line: {stderr}"));
        let head = &line[..line.rfind(before).unwrap()];
        head[head.rfind(|c: char| !c.is_ascii_digit()).unwrap() + 1..].parse().unwrap()
    };
    let cold = check();
    assert!(cold.0 == Some(0) && named(&cold.1, "unused_a") && !cold.2, "{}", cold.3);
    // the cold check snapshots the program before its bodies and records that as the image
    let boot = count(&cold.3, "relax check: boot snapshot of ", " types");
    let checked = count(&cold.3, "relax check: collected program recorded (true)", " types");
    assert!(boot < checked, "the bodies made types ({boot} before, {checked} after): {}", cold.3);
    // two edits of the checked crate: both checks restore the crates below it from the image the
    // cold check recorded, the second after the first wrote its graph
    for (before, now) in [("unused_a", "unused_b"), ("unused_b", "unused_c")] {
        write("app/lib.rs", &app(now, "dep::value() + 1"));
        let r = check();
        assert!(r.0 == Some(0) && r.2 && named(&r.1, now) && !named(&r.1, before), "the edit to {now}: {}", r.3);
        let held = count(&r.3, "relax collected: restored ", " types");
        assert!(held <= boot, "the image holds {held} types, the program before its bodies {boot}: {}", r.3);
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

/// relax#39: an error in a crate the check does not check -- an unresolved import or a signature
/// error of a dependency outside the workspace -- fails the check, as cargo check fails compiling
/// that dependency: on the cold check, on the same check again (no run record, no collected image
/// of that load hides it) and after an edit of the checked crate; with the dependency fixed the
/// check passes. A published crate's error (its source under the cargo home's registry) also says
/// it is most likely a makepad run divergence and asks for a report; a path dependency's does not.
#[test]
fn a_dependency_error_fails_the_check() {
    if std::env::var_os(CHILD).is_some() || std::env::var_os(EARLY_CHILD).is_some() || std::env::var_os(APP_CHILD).is_some() { return; }
    let dir = package("deperror", &[]);
    let write = |f: &str, src: &str| {
        let path = dir.join(f);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, src).unwrap();
    };
    write("ws/Cargo.toml", "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n");
    let app_toml = |dep: &str| format!("[package]\nname = \"app_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n[dependencies]\ndep = {{ path = \"../../{dep}\" }}\n");
    write("ws/app/Cargo.toml", &app_toml("dep"));
    write("dep/Cargo.toml", "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n");
    let app = |n: u32| format!("pub fn top() -> u32 {{ dep::value() + {n} }}\n");
    // (exit code, stderr)
    let check = || {
        let out = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "check_reuse_tests::app_check_child", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(APP_CHILD, dir.join("ws")).env("RELAX_CACHE_DIR", dir.join("cache")).env("CARGO_HOME", dir.join("cargo"))
            .env_remove("RELAX_LINT_FLAGS").env_remove("RELAX_WORKSPACE").env_remove("RELAX_CACHE"));
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && stdout.contains("1 passed"), "{stdout}\n{stderr}");
        let code = stderr.lines().find_map(|l| l.strip_prefix("app child exit ")).map(|c| c.parse::<i32>().unwrap());
        (code, stderr.into_owned())
    };
    // (`named`: the check names the crate whose error the load or check printed, as cargo does;
    // `published`: and asks for a report)
    let fails = |what: &str, code: &str, named: bool, published: bool| {
        let r = check();
        assert!(r.0.is_some_and(|c| c != 0) && r.1.contains(code) && (!named || r.1.contains("could not compile `dep`")), "{what}: {}", r.1);
        assert!(!r.1.contains(": internal:"), "{what}: a dependency's error is its own, not internal: {}", r.1);
        assert_eq!(r.1.contains("`dep` is a published crate"), published, "{what}: only a published crate's error asks for a report: {}", r.1);
    };
    write("ws/app/lib.rs", &app(1));
    for (what, dep, code, named) in [
        ("an unresolved import", "use std::collections::NoSuchMap;\npub fn value() -> u32 { 7 }\n", "E0432", true),
        ("a field type", "pub struct S { pub f: NoSuchType }\npub fn value() -> u32 { 7 }\n", "E0412", true),
        // (a fn signature is collected when a body reaches it)
        ("a signature the app reaches", "pub fn value() -> NoSuchType { loop {} }\n", "E0412", true),
    ] {
        write("dep/lib.rs", dep);
        fails(&format!("{what}, cold"), code, named, false);
        fails(&format!("{what}, again"), code, named, false);
        write("ws/app/lib.rs", &app(2));
        fails(&format!("{what}, after an app edit"), code, named, false);
        write("ws/app/lib.rs", &app(1));
    }
    // the same dependency as a published crate: its source under the cargo home's registry
    let registry_dep = "cargo/registry/src/index.example-0/dep-0.1.0";
    write(&format!("{registry_dep}/Cargo.toml"), "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n");
    write(&format!("{registry_dep}/lib.rs"), "pub fn value() -> NoSuchType { loop {} }\n");
    write("ws/app/Cargo.toml", &app_toml(registry_dep));
    fails("a published crate's signature the app reaches", "E0412", true, true);
    write("ws/app/Cargo.toml", &app_toml("dep"));
    write("dep/lib.rs", "pub fn value() -> u32 { 7 }\n");
    for what in ["fixed", "fixed, again"] {
        let r = check();
        assert!(r.0 == Some(0) && !r.1.contains("could not compile"), "{what}: {}", r.1);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// An app check reads the crates below it through globs from their flattened export tables (rsim.rs
/// ExtTable): through a glob cycle (the root's `pub use inner::*` and inner's `use super::*`), an
/// enum's variants, and past a name two globs of the cycle give different items (that one is looked
/// up alone). Debug builds compare every answer of a table with the lookup of that one name (and
/// every proc macro name the reach filter skips with the walk for it); the verdicts are rustc's:
/// the names resolve, and a name the app's own two globs both provide is ambiguous where it is
/// used (E0659). A `pub(crate)` glob of the dependency's module brings the app nothing (relax
/// issue 186: its `Scalar` met the app's own through the app's two globs, E0659, and in a type
/// it named the dependency's `f32`, E0308).
#[test]
fn an_app_check_reads_the_globs_of_unchecked_crates_as_rustc_does() {
    if std::env::var_os(CHILD).is_some() || std::env::var_os(EARLY_CHILD).is_some() || std::env::var_os(APP_CHILD).is_some() { return; }
    // (the dependency lives outside the workspace: an app check does not check it)
    let dir = package("appglobs", &[]);
    let write = |f: &str, src: &str| {
        let path = dir.join(f);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, src).unwrap();
    };
    write("ws/Cargo.toml", "[workspace]\nmembers = [\"app\"]\nresolver = \"2\"\n");
    write("dep/Cargo.toml", "[package]\nname = \"dep\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n");
    write("dep/lib.rs", "pub mod inner;\nmod a { pub struct Dup; }\npub use inner::*;\npub use a::*;\npub enum Shape { Round, Square }\npub use Shape::*;\npub struct Thing;\npub fn root_fn() -> u32 { 1 }\nmod units { pub type Scalar = f32; }\npub mod prelude { pub struct Body; #[allow(unused_imports)] pub(crate) use crate::units::*; }\n");
    write("dep/inner.rs", "use super::*;\nmod b { pub struct Dup; }\npub use b::*;\npub struct Inner;\npub fn inner_fn() -> u32 { root_fn() + 1 }\n");
    write("ws/app/Cargo.toml", "[package]\nname = \"app_pkg\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\npath = \"lib.rs\"\n[dependencies]\ndep = { path = \"../../dep\" }\n");
    let app = |clash: &str| format!("use dep::*;\npub fn top(s: Shape) -> u32 {{ let _ = Inner; root_fn() + inner_fn() + match s {{ Round => 1, Square => 2 }} }}\nuse dep::prelude::*;\nmod clock {{ pub struct Scalar; }}\nuse clock::*;\npub fn body() -> Body {{ let _s: Scalar = Scalar; Body }}\n{clash}");
    let check = || {
        let out = crate::test_child::output(std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "check_reuse_tests::app_check_child", "--nocapture", "--test-threads=1", "--format=terse"])
            .env(APP_CHILD, dir.join("ws")).env("RELAX_CACHE_DIR", dir.join("cache"))
            .env_remove("RELAX_LINT_FLAGS").env_remove("RELAX_WORKSPACE").env_remove("RELAX_CACHE"));
        let (stdout, stderr) = (String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        assert!(out.status.success() && stdout.contains("1 passed"), "{stdout}\n{stderr}");
        let code = stderr.lines().find_map(|l| l.strip_prefix("app child exit ")).map(|c| c.parse::<i32>().unwrap());
        (code, stderr.into_owned())
    };
    // the dep's names through the cycle and the enum glob, and the app's `Scalar` beside the dep's
    // `pub(crate)` glob: no diagnostic
    write("ws/app/lib.rs", &app(""));
    let clean = check();
    assert!(clean.0 == Some(0) && !clean.1.contains(": error") && !clean.1.contains(": warning:"), "{}", clean.1);
    // `Thing`: the dep's (its table) and the app's own module's, both through a glob
    write("ws/app/lib.rs", &app("mod own { pub struct Thing; }\nuse own::*;\npub fn clash() -> u32 { let _ = Thing; 0 }\n"));
    let clash = check();
    assert!(clash.0.is_some_and(|c| c != 0) && clash.1.contains("E0659") && clash.1.contains("`Thing` is ambiguous"), "{}", clash.1);
    std::fs::remove_dir_all(&dir).unwrap();
}
