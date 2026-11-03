//! Runtime fixtures for the overlay correctness gate. All checkouts and outputs belong to one lease.

use super::exec::WorkerRun;
use super::loaders::OverlayBuild;
use super::{overlay_compare as compare, relaxgit, Pin};
use crate::sync::git::{self, Store};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const CASES: &[&str] = &["base", "edit", "add", "delete", "rename", "new-crate", "features", "build-script", "build-input", "build-env", "proc-macro", "generated", "committed-overlay", "concurrent"];

fn io(e: std::io::Error) -> String { e.to_string() }
fn write(root: &Path, path: &str, text: &str) -> Result<(), String> {
    let p = root.join(path);
    std::fs::create_dir_all(p.parent().unwrap()).and_then(|_| std::fs::write(p, text)).map_err(io)
}
fn g(root: &Path, args: &[&str]) -> Result<String, String> { git::run(Some(root), None, &[], args).map(|s| s.trim().to_string()) }

const WORKSPACE: &str = "[workspace]\nmembers = [\"a\", \"b\", \"m\"]\nresolver = \"2\"\n";
const A: &str = "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nm = { path = \"../m\" }\n[features]\ndefault = []\nextra = []\n";
const LIB: &str = r#"mod value;
mod checked_in;
include!(concat!(env!("OUT_DIR"), "/generated.rs"));
pub fn value() -> u32 { value::value() + checked_in::VALUE + m::number!() + if cfg!(feature = "extra") { 100 } else { 0 } }
#[cfg(test)]
mod tests {
    #[test]
    fn value() { println!("unit-value={}", super::value()); assert!(super::value() > 0); }
    #[test]
    fn generated() { println!("build-value={}", super::BUILD_VALUE); assert!(!super::BUILD_VALUE.is_empty()); }
}
"#;
const BUILD: &str = r#"fn main() {
    println!("cargo:rerun-if-changed=data.txt");
    println!("cargo:rerun-if-env-changed=DEVHUB_FIXTURE_VALUE");
    let data = std::fs::read_to_string("data.txt").unwrap();
    let env = std::env::var("DEVHUB_FIXTURE_VALUE").unwrap();
    let value = format!("{}:{}:script1", data.trim(), env);
    std::fs::write(std::path::Path::new(&std::env::var("OUT_DIR").unwrap()).join("generated.rs"), format!("pub const BUILD_VALUE: &str = {:?};", value)).unwrap();
}
"#;
const MACRO: &str = "extern crate proc_macro;\n#[proc_macro]\npub fn number(_: proc_macro::TokenStream) -> proc_macro::TokenStream { \"7u32\".parse().unwrap() }\n";

fn fixture(root: &Path) -> Result<(), String> {
    for (path, text) in [
        ("Cargo.toml", WORKSPACE), (".gitignore", "/target/\n/Cargo.lock\n"),
        ("a/Cargo.toml", A), ("a/src/lib.rs", LIB), ("a/src/value.rs", "pub fn value() -> u32 { 1 }\n"),
        ("a/src/checked_in.rs", "pub const VALUE: u32 = 10;\n"), ("a/build.rs", BUILD), ("a/data.txt", "data1\n"),
        ("a/tests/value.rs", "#[test]\nfn integration_value() { println!(\"integration-value={}\", a::value()); assert!(a::value() > 0); }\n"),
        ("a/tests/deleted.rs", "#[test]\nfn removable() { println!(\"removable=present\"); }\n"),
        ("b/Cargo.toml", "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\na = { path = \"../a\" }\n"),
        ("b/src/lib.rs", "pub fn value() -> u32 { a::value() + 1 }\n#[test]\nfn dependent() { println!(\"dependent-value={}\", value()); assert!(value() > 0); }\n"),
        ("m/Cargo.toml", "[package]\nname = \"m\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[lib]\nproc-macro = true\n"),
        ("m/src/lib.rs", MACRO),
    ] { write(root, path, text)?; }
    g(root, &["init", "-q"])?;
    g(root, &["add", "."])?;
    commit(root, "fixture")?;
    Ok(())
}

fn commit(root: &Path, subject: &str) -> Result<String, String> {
    g(root, &["-c", "user.name=overlay-selftest", "-c", "user.email=overlay-selftest@localhost", "-c", "commit.gpgsign=false", "commit", "-qm", subject])?;
    g(root, &["rev-parse", "HEAD"])
}

/// Use the lane bundle importer, so the feed sees ordinary bare-store refs and objects.
fn import(root: &Path, lease: &str) -> Result<String, String> {
    let branch = format!("overlay-selftest-{lease}");
    g(root, &["branch", "-f", &branch, "HEAD"])?;
    let bundle = root.parent().unwrap().join("fixture.bundle");
    g(root, &["bundle", "create", &bundle.to_string_lossy(), &format!("refs/heads/{branch}")])?;
    Store::open("makepad")?.fetch_bundle(&bundle, &branch)
}

/// Called after the lease's processes stop, including cancellation or a hub restart.
pub fn cleanup(work: &Path, lease: &str) -> Result<(), String> {
    relaxgit::forget(work, &format!("overlay-selftest-{lease}"))?;
    let store = Store::open("makepad")?;
    git::run(None, Some(&store.git_dir), &[], &["update-ref", "-d", &format!("{}overlay-selftest-{lease}", git::PUSHED)])?;
    for p in [work.join("dev/lane").join(format!("overlay-selftest-{lease}")), relaxgit::overlay(work, &format!("overlay-selftest-{lease}"))] {
        if p.exists() { std::fs::remove_dir_all(p).map_err(io)?; }
    }
    Ok(())
}

struct Rig<'a> {
    root: PathBuf,
    repo: PathBuf,
    work: PathBuf,
    /// the box's real devhub work dir (holds the dev JIT tree and its std)
    box_work: PathBuf,
    binary: PathBuf,
    base: String,
    committed: String,
    cores: u32,
    lease: String,
    build: &'a OverlayBuild<'a>,
    spawn: &'a dyn Fn(&Command) -> Result<WorkerRun, String>,
}

enum Verdict { Equal, Different(String), JitError(String) }

impl<'a> Rig<'a> {
    fn new(work: &Path, scratch: &Path, lease: &str, cores: u32, build: &'a OverlayBuild<'a>, spawn: &'a dyn Fn(&Command) -> Result<WorkerRun, String>) -> Result<Self, String> {
        (build.active)()?;
        let root = scratch.join("overlay-selftest");
        std::fs::create_dir(&root).map_err(io)?;
        let repo = root.join("repo");
        fixture(&repo)?;
        let base = g(&repo, &["rev-parse", "HEAD"])?;
        write(&repo, "a/src/value.rs", "pub fn value() -> u32 { 41 }\n")?;
        g(&repo, &["add", "."])?;
        let committed = commit(&repo, "committed edit")?;
        if import(&repo, lease)? != committed { return Err("fixture bundle imported a different commit".into()); }
        let pin = Pin { makepad: Store::open("makepad")?.rev(git::SYNCED_REF).ok_or("selftest: box has no synced makepad pin")?, commercial: Store::open("commercial")?.rev(git::SYNCED_REF) };
        let lane = format!("overlay-selftest-{lease}");
        relaxgit::prepare(work, &lane, &pin)?;
        let empty = makepad_strict_json::Value::Obj(Vec::new());
        relaxgit::finish(work, &lane, &pin, &root.join("empty"), &[], &[], &empty)?;
        let selected = super::loaders::find_overlay(work, &lane, lease, &pin, None, build).map_err(|e| format!("JIT error selecting the dev JIT: {e}"))?;
        Ok(Self { work: root.join("box"), box_work: work.to_path_buf(), root, repo, binary: selected.binary, base, committed, cores, lease: lease.into(), build, spawn })
    }

    fn state(&self, lane: &str, case: &str, second: bool) -> Result<(PathBuf, Vec<(String, String)>), String> {
        (self.build.active)()?;
        let sha = if case == "committed-overlay" || (case == "concurrent" && second) { &self.committed } else { &self.base };
        let tree = self.root.join(lane);
        if tree.exists() {
            g(&tree, &["reset", "--hard", sha])?;
            g(&tree, &["clean", "-fdx"])?;
        } else { g(&self.repo, &["worktree", "add", "--detach", &tree.to_string_lossy(), sha])?; }
        match case {
            "edit" => write(&tree, "a/src/value.rs", "pub fn value() -> u32 { 2 }\n")?,
            "add" => {
                write(&tree, "a/src/added.rs", "pub fn added() -> u32 { 31 }\n")?;
                write(&tree, "a/src/lib.rs", &format!("mod added;\n{}", LIB.replace("value::value()", "value::value() + added::added()")))?;
            }
            "delete" => std::fs::remove_file(tree.join("a/tests/deleted.rs")).map_err(io)?,
            "rename" => {
                std::fs::rename(tree.join("a/src/value.rs"), tree.join("a/src/renamed.rs")).map_err(io)?;
                write(&tree, "a/src/lib.rs", &LIB.replace("mod value;", "#[path = \"renamed.rs\"] mod value;"))?;
            }
            "new-crate" => {
                write(&tree, "Cargo.toml", &WORKSPACE.replace("\"m\"]", "\"m\", \"c\"]"))?;
                write(&tree, "c/Cargo.toml", "[package]\nname = \"c\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\nb = { path = \"../b\" }\n")?;
                write(&tree, "c/src/lib.rs", "#[test]\nfn new_crate() { println!(\"new-crate={}\", b::value()); assert!(b::value() > 0); }\n")?;
            }
            "features" => write(&tree, "a/Cargo.toml", &A.replace("default = []", "default = [\"extra\"]"))?,
            "build-script" => write(&tree, "a/build.rs", &BUILD.replace("script1", "script2"))?,
            "build-input" => write(&tree, "a/data.txt", "data2\n")?,
            "proc-macro" => write(&tree, "m/src/lib.rs", &MACRO.replace("7u32", "17u32"))?,
            "generated" | "committed-overlay" => write(&tree, "a/src/checked_in.rs", "pub const VALUE: u32 = 20;\n")?,
            "concurrent" => write(&tree, "a/src/checked_in.rs", if second { "pub const VALUE: u32 = 102;\n" } else { "pub const VALUE: u32 = 101;\n" })?,
            "base" | "build-env" => {}
            _ => return Err(format!("unknown case {case}")),
        }
        let pin = Pin { makepad: sha.clone(), commercial: None };
        let mut env = relaxgit::prepare(&self.work, lane, &pin)?;
        let (shipped, deleted) = super::ship::uncommitted(&tree, &pin)?;
        let bases = super::ship::bases_json(&tree, &pin, &shipped.iter().chain(&deleted).cloned().collect::<Vec<_>>())?;
        let bases = makepad_strict_json::parse(bases.as_bytes()).map_err(|e| format!("fixture bases: {e:?}"))?;
        // finish owns its staging directory; the real checkout remains the independent oracle.
        let staged = self.root.join(format!("{lane}-staged"));
        std::fs::create_dir_all(&staged).map_err(io)?;
        for path in &shipped {
            let dst = staged.join(path);
            std::fs::create_dir_all(dst.parent().unwrap()).and_then(|_| std::fs::copy(tree.join(path), dst)).map_err(io)?;
        }
        relaxgit::finish(&self.work, lane, &pin, &staged, &shipped, &deleted, &bases)?;
        std::fs::create_dir_all(self.work.join("dev/target")).map_err(io)?;
        env.push(("CARGO_TARGET_DIR".into(), self.work.join("dev/target").to_string_lossy().into_owned()));
        Ok((tree, env))
    }

    fn spawn(&self, tree: &Path, env: &[(String, String)], overlay: bool, case: &str, pkg: &str, target: &str) -> Result<WorkerRun, String> {
        (self.build.active)()?;
        let mut cmd = Command::new(&self.binary);
        let lane = tree.file_name().unwrap().to_string_lossy();
        let session = &self.lease;
        cmd.args(["test", pkg, target, "--", "--nocapture", "--test-threads=1"])
            .env("MAKEPAD_LOADER", &self.binary)
            .env("MAKEPAD_TEST_LANE", lane.as_ref()).env("MAKEPAD_JIT_SESSION", format!("{session}-{lane}"))
            .env("DEVHUB_FIXTURE_VALUE", if case == "build-env" { "env2" } else { "env1" });
        if case == "concurrent" {
            let jobs = (self.cores / 2).max(1);
            cmd.env("MAKEPAD_RUN_JOBS", jobs.to_string()).env("MAKEPAD_RUN_O3_THREADS", (jobs / 2).max(1).to_string());
        }
        // The fixture repositories hold no makepad run std: both sides name the box's own explicitly, so
        // the overlay's automatic std check (which would print a disk-std notice) is not part of the diff.
        cmd.env("MAKEPAD_RUN_STD", self.box_work.join("dev/tree/libs/relax/compiler/std"));
        for key in ["MAKEPAD_RUN_VFS", "MAKEPAD_RUN_WORKSPACE", "MAKEPAD_RUN_EDITS", "MAKEPAD_RUN_HOME", "MAKEPAD_RUN_CACHE_DIR"] { cmd.env_remove(key); }
        if overlay {
            cmd.envs(env.iter().cloned()).current_dir(self.work.join("dev/lane").join(tree.file_name().unwrap()).join("run"));
        } else {
            cmd.current_dir(tree).env("CARGO_TARGET_DIR", self.root.join("tree-target")).env("MAKEPAD_RUN_HOME", self.root.join("tree-home"));
        }
        (self.spawn)(&cmd)
    }

    fn wait(&self, mut run: WorkerRun, tree: &Path) -> Result<compare::Output, String> {
        let exit = run.wait().map(crate::os::exit_code)?;
        let text = std::fs::read_to_string(run.log()).map_err(io)?;
        let roots = [(tree.to_string_lossy().into_owned(), "<src>"), (self.work.join("dev/src").to_string_lossy().into_owned(), "<src>"), (self.root.to_string_lossy().into_owned(), "<scratch>")];
        Ok(compare::Output { exit, text: compare::normalize(&text, &roots.iter().map(|(p, n)| (p.as_str(), *n)).collect::<Vec<_>>()).join("\n") })
    }

    fn case(&self, case: &str) -> Result<Verdict, String> {
        let a = self.state("first", case, false)?;
        let b = if case == "concurrent" { Some(self.state("second", case, true)?) } else { None };
        let mut targets = vec![("a", "--lib"), ("a", "--tests"), ("b", "--lib")];
        if case == "new-crate" { targets.push(("c", "--lib")); }
        let mut difference = None;
        for (pkg, target) in targets {
            // Both overlays start before either is waited: different pins, one virtual root/cache.
            let oa = self.spawn(&a.0, &a.1, true, case, pkg, target)?;
            let ob = b.as_ref().map(|b| self.spawn(&b.0, &b.1, true, case, pkg, target)).transpose()?;
            let mut pairs = vec![(&a, self.wait(oa, &a.0)?)];
            if let (Some(b), Some(ob)) = (&b, ob) { pairs.push((b, self.wait(ob, &b.0)?)); }
            for ((tree, env), overlay) in pairs {
                let real = self.wait(self.spawn(tree, env, false, case, pkg, target)?, tree)?;
                for (side, output) in [("tree", &real), ("overlay", &overlay)] {
                    if let Some(e) = compare::jit_error(&output.text) { return Ok(Verdict::JitError(format!("{side} {pkg} {target}: {e}"))); }
                }
                let diff = if !real.text.lines().any(|l| l.starts_with("test result:")) || !overlay.text.lines().any(|l| l.starts_with("test result:")) {
                    Some(format!("missing test verdict: tree exit={} {:?}; overlay exit={} {:?}", real.exit, real.text.lines().find(|l| !l.is_empty()).unwrap_or("no output"), overlay.exit, overlay.text.lines().find(|l| !l.is_empty()).unwrap_or("no output")))
                } else if real.exit != overlay.exit { Some(format!("exit: tree={}; overlay={}", real.exit, overlay.exit)) }
                    else { compare::first_diff(&real.text.lines().map(str::to_string).collect::<Vec<_>>(), &overlay.text.lines().map(str::to_string).collect::<Vec<_>>()) };
                if difference.is_none() { difference = diff.map(|d| format!("{} {pkg} {target}: {d}", tree.file_name().unwrap().to_string_lossy())); }
            }
        }
        Ok(difference.map_or(Verdict::Equal, Verdict::Different))
    }
}

/// Run on the hub's lease worker. JIT selection/builds and fixture processes use the
/// same lease-owned callbacks as ordinary overlays; no helper executable is involved.
pub(super) fn run(work: &Path, scratch: &Path, lease: &str, selected: &str, cores: u32, build: &OverlayBuild<'_>, spawn: &dyn Fn(&Command) -> Result<WorkerRun, String>, say: &dyn Fn(&str)) -> i32 {
    let run = || -> Result<i32, String> {
        let _cache = super::disk::dev_cache_hold(work, build.active)?;
        let rig = Rig::new(work, scratch, lease, cores, build, spawn);
        let (mut equal, mut different, mut errors) = (0, 0, 0);
        for case in CASES.iter().filter(|c| selected.is_empty() || **c == selected) {
            (build.active)()?;
            let result = match &rig {
                Ok(rig) => {
                    // Warm the identical baseline before every edit, including a selected case.
                    let warmup = if *case == "base" { Ok(Verdict::Equal) } else { rig.case("base") };
                    let result = rig.case(case);
                    match warmup {
                        Ok(Verdict::JitError(e)) => Ok(Verdict::JitError(format!("warmup: {e}"))),
                        Err(e) => Err(format!("warmup: {e}")),
                        _ => result,
                    }
                }
                Err(e) if compare::jit_error(e).is_some() => Ok(Verdict::JitError(e.clone())),
                Err(e) => Err(e.clone()),
            };
            match result {
                Ok(Verdict::Equal) => { say(&format!("case {case}: equal")); equal += 1; }
                Ok(Verdict::Different(diff)) => { say(&format!("case {case}: DIFFERENT {diff}")); different += 1; }
                Ok(Verdict::JitError(e)) => { say(&format!("case {case}: JIT error {}", e.replace(['\n', '\r'], " | "))); errors += 1; }
                Err(e) => { say(&format!("case {case}: DIFFERENT tree/overlay evidence unavailable: {}", e.replace(['\n', '\r'], " | "))); different += 1; }
            }
        }
        say(&format!("selftest: {errors} JIT errors"));
        say(&format!("selftest: {equal} equal, {different} different"));
        Ok(i32::from(different != 0 || errors != 0))
    };
    match run() { Ok(code) => code, Err(e) => { say(&format!("selftest: {e}")); 1 } }
}
