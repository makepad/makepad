//! `devhub ci`: CI as many small shards over every free box, the verify stage of a push (was
//! tools/ci on the CI mini: one long serial run of 1.5-3.5 h). A run plans the shards of a landed
//! pair from its tree, then one worker per box pulls them, longest first, each shard its own lease
//! at verify's priority on that box (landings and lanes go first; a running shard finishes), so the
//! boxes balance by measured time and a busy box simply pulls nothing.
//!
//! The shards (everything under the JIT, `makepad check|test|run`, except where it cannot be yet):
//! - `jit:<n>`: `makepad check` of the workspace members, in groups (Linux); `mjit:<n>` the apps'
//!   macOS check (a Mac), `wjit:<n>` their Windows check (105);
//! - `test:<pkg>:<target>`: one test binary of the platform, draw and widgets crates (`makepad test
//!   <pkg> --lib|--test <t>`);
//! - `check:<target>`: cargo's `check --release --target` of every app the scripts check there:
//!   the cross targets the JIT does not build (Apple-SDK targets on a Mac, the rest on Linux);
//! - `slice:<app>`: a licensed commercial slice, cut, linted and checked (`ci-leg slice`);
//! - `smoke:<dir>`: one app's `ci.splash` (launch hidden, drive it, grab, the vision judge on the AI
//!   fleet: `ci-leg smoke`) on any box that shows windows; a rotating third of them on a Mac.
//!
//! What is checked where comes from the tree: the apps (`apps/<x>/Cargo.toml`), their
//! `ci.splash` (`ci.check_targets({packages, targets})`), the workspace members, the commercial
//! `slices.json`. The run's state is [`crate::state::Ci`]; [`gate`] is what `devhub verify` runs.

use super::leg::{Held, Spec};
use super::verify::Verdict;
use super::{stamp, Ctx};
use crate::lease::Kind;
use crate::proto::{wall_ms, Sev};
use crate::state::{CiRun, CiShard, Pair};
use makepad_strict_json::{obj, s, Value};
use makepad_toml_parser::{parse_toml, Toml};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Verify's priority: the lowest; landings and lanes go first.
pub const PRIO: i32 = super::verify::PRIO;
/// The lane and agent the shards' leases are under.
const LANE: &str = "ci";
const AGENT: &str = "devhub-ci";
/// A shard whose lease is not granted within this goes back to the pool (another box takes it).
const QUEUE_WAIT: Duration = Duration::from_secs(45);
/// A box that did not grant a lease waits this long before asking again.
const BACKOFF: Duration = Duration::from_secs(60);

/// The apps the CI box skipped (its ci.toml `skip_apps`): not apps, or not launchable alone.
pub const SKIP_APPS: &[&str] = &["source-library", "mixer", "mail", "wm-all", "wm-dyn", "asset-server", "flow-server", "ai-hub"];
/// The platform's own crates: their tests run in every CI run.
pub const TEST_DIRS: &[&str] = &["platform", "draw", "widgets", "tools/ci_leg"];
/// The default smoke script of an app without its own.
pub const DEFAULT_SCRIPT: &str = "tools/ci/default.ci.splash";
pub const COMMERCIAL: &str = "apps/commercial";

/// The target matrix (tools/ci cargo.rs, mirrored from cargo_makepad): target, how it is checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Build {
    Bin,
    /// the lib only (`--lib`): web and Android build an app as a library
    Lib,
    /// tier 3: nightly with `-Z build-std=std`
    Std,
}

pub const TARGETS: [(&str, Build); 15] = [
    ("aarch64-apple-darwin", Build::Bin),
    ("x86_64-pc-windows-msvc", Build::Bin),
    ("x86_64-unknown-linux-gnu", Build::Bin),
    ("wasm32-unknown-unknown", Build::Lib),
    ("aarch64-linux-android", Build::Lib),
    ("aarch64-apple-ios", Build::Bin),
    ("x86_64-linux-android", Build::Lib),
    ("aarch64-apple-tvos", Build::Std),
    ("aarch64-apple-tvos-sim", Build::Std),
    ("i686-linux-android", Build::Lib),
    ("aarch64-apple-ios-sim", Build::Bin),
    ("x86_64-apple-ios", Build::Bin),
    ("x86_64-apple-tvos", Build::Std),
    ("x86_64-apple-darwin", Build::Bin),
    ("x86_64-pc-windows-gnu", Build::Bin),
];

/// The targets the JIT checks natively on a box of that OS (`makepad check` there) instead of
/// cargo: (target, os).
pub const JIT_TARGETS: [(&str, Os); 3] = [("x86_64-unknown-linux-gnu", Os::Linux), ("aarch64-apple-darwin", Os::Macos), ("x86_64-pc-windows-msvc", Os::Windows)];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Os {
    Any,
    Linux,
    Macos,
    Windows,
}

impl Os {
    pub fn as_str(self) -> &'static str {
        match self {
            Os::Any => "any",
            Os::Linux => "linux",
            Os::Macos => "macos",
            Os::Windows => "windows",
        }
    }

    pub fn of(caps_os: &str) -> Os {
        match caps_os.to_ascii_lowercase().as_str() {
            "linux" => Os::Linux,
            "macos" | "darwin" => Os::Macos,
            "windows" => Os::Windows,
            _ => Os::Any,
        }
    }
}

// ---------------------------------------------------------------- the tree of a pair

/// The files of a pair's tree (makepad `work` with commercial `main` at `apps/commercial`), read
/// from git objects: no checkout moves.
pub struct Tree {
    pub files: Vec<String>,
    read: Box<dyn Fn(&str) -> Option<String> + Send + Sync>,
}

impl Tree {
    pub fn read(&self, path: &str) -> Option<String> {
        (self.read)(path)
    }

    pub fn has(&self, path: &str) -> bool {
        self.files.binary_search_by(|f| f.as_str().cmp(path)).is_ok()
    }

    /// An in-memory tree (tests).
    pub fn mem(files: Vec<(&str, &str)>) -> Tree {
        let map: BTreeMap<String, String> = files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect();
        let names: Vec<String> = map.keys().cloned().collect();
        Tree { files: names, read: Box::new(move |p| map.get(p).cloned()) }
    }

    /// The tree of `pair` in `repo` (and `repo/apps/commercial`).
    pub fn at(repo: &Path, pair: &Pair) -> Result<Tree, String> {
        let com = repo.join(COMMERCIAL);
        let mut files: Vec<String> = ls_tree(repo, &pair.work)?;
        let has_com = !pair.main.is_empty() && com.join(".git").exists();
        if has_com {
            files.extend(ls_tree(&com, &pair.main)?.into_iter().map(|f| format!("{COMMERCIAL}/{f}")));
        }
        files.sort();
        files.dedup();
        let main = Batch::start(repo)?;
        let commercial = if has_com { Some(Batch::start(&com)?) } else { None };
        let (work, cmain) = (pair.work.clone(), pair.main.clone());
        let main = Mutex::new(main);
        let commercial = commercial.map(Mutex::new);
        Ok(Tree {
            files,
            read: Box::new(move |p| match (p.strip_prefix("apps/commercial/"), &commercial) {
                (Some(rest), Some(c)) => c.lock().ok()?.blob(&cmain, rest),
                _ => main.lock().ok()?.blob(&work, p),
            }),
        })
    }
}

fn ls_tree(dir: &Path, sha: &str) -> Result<Vec<String>, String> {
    let o = makepad_remote::group::command("git")
        .arg("-C")
        .arg(dir)
        .args(["ls-tree", "-r", "-z", "--name-only", sha])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("git ls-tree: {e}"))?;
    if !o.status.success() {
        return Err(format!("git ls-tree {}: {}", crate::state::short(sha), String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(o.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()).map(|p| String::from_utf8_lossy(p).into_owned()).collect())
}

/// One `git cat-file --batch` per repository: blobs by `<sha>:<path>`.
struct Batch {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
}

impl Batch {
    fn start(dir: &Path) -> Result<Batch, String> {
        let mut child = makepad_remote::group::command("git")
            .arg("-C")
            .arg(dir)
            .args(["cat-file", "--batch"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("git cat-file: {e}"))?;
        let out = BufReader::new(child.stdout.take().ok_or("git cat-file: no stdout")?);
        Ok(Batch { child, out })
    }

    fn blob(&mut self, sha: &str, path: &str) -> Option<String> {
        if path.contains('\n') {
            return None;
        }
        let stdin = self.child.stdin.as_mut()?;
        writeln!(stdin, "{sha}:{path}").ok()?;
        stdin.flush().ok()?;
        let mut head = String::new();
        self.out.read_line(&mut head).ok()?;
        // `<oid> <type> <size>` or `<name> missing`
        let mut w = head.split_whitespace();
        let (_, kind, size) = (w.next()?, w.next()?, w.next());
        let size: usize = size?.parse().ok()?;
        let mut body = vec![0u8; size + 1];
        self.out.read_exact(&mut body).ok()?;
        body.pop();
        (kind == "blob").then(|| String::from_utf8_lossy(&body).into_owned())
    }
}

impl Drop for Batch {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
    }
}

// ---------------------------------------------------------------- discovery

/// A crate of the tree: its package, binaries and whether it has a lib.
#[derive(Clone, Debug, PartialEq)]
pub struct Krate {
    pub package: String,
    pub dir: String,
    pub bins: Vec<String>,
    pub lib: bool,
    /// integration tests: `tests/*.rs` and `[[test]]` names
    pub tests: Vec<String>,
}

fn dir_of(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(d, _)| d)
}

fn join(dir: &str, rel: &str) -> String {
    if dir.is_empty() {
        rel.to_string()
    } else {
        format!("{dir}/{rel}")
    }
}

/// The crate a `Cargo.toml` declares (None: no `[package]`, or one with a workspace of its own).
pub fn krate(tree: &Tree, manifest: &str) -> Option<Krate> {
    let doc = parse_toml(&tree.read(manifest)?).ok()?;
    let package = doc.get_path(&["package", "name"])?.as_str()?.to_string();
    let dir = dir_of(manifest).to_string();
    let mut bins: Vec<String> = doc
        .root
        .get("bin")
        .and_then(Toml::as_array_of_tables)
        .unwrap_or(&[])
        .iter()
        .map(|t| t.get("name").and_then(Toml::as_str).unwrap_or(&package).to_string())
        .collect();
    if bins.is_empty() && tree.has(&join(&dir, "src/main.rs")) {
        bins.push(package.clone());
    }
    let lib = doc.root.get("lib").is_some() || tree.has(&join(&dir, "src/lib.rs"));
    let mut tests: BTreeSet<String> = doc
        .root
        .get("test")
        .and_then(Toml::as_array_of_tables)
        .unwrap_or(&[])
        .iter()
        .filter_map(|t| t.get("name").and_then(Toml::as_str).map(str::to_string))
        .collect();
    let prefix = join(&dir, "tests/");
    for f in &tree.files {
        if let Some(name) = f.strip_prefix(&prefix).and_then(|r| r.strip_suffix(".rs")) {
            if !name.contains('/') {
                tests.insert(name.to_string());
            }
        }
    }
    Some(Krate { package, dir, bins, lib, tests: tests.into_iter().collect() })
}

fn skipped(package: &str, dir: &str) -> bool {
    let short = dir.rsplit('/').next().unwrap_or(dir);
    SKIP_APPS.iter().any(|s| *s == package || *s == short || Some(*s) == package.strip_prefix("makepad-") || *s == dir)
}

/// The workspace members (root `Cargo.toml` `workspace.members`, globs with `*` segments, minus
/// `workspace.exclude`) that are packages in the tree.
pub fn members(tree: &Tree) -> Vec<Krate> {
    let Some(doc) = tree.read("Cargo.toml").and_then(|t| parse_toml(&t).ok()) else { return Vec::new() };
    let list = |k: &str| -> Vec<String> {
        doc.get_path(&["workspace", k]).and_then(Toml::as_array).unwrap_or(&[]).iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
    };
    let (patterns, exclude) = (list("members"), list("exclude"));
    let manifests: Vec<&String> = tree.files.iter().filter(|f| f.ends_with("/Cargo.toml")).collect();
    let mut dirs = BTreeSet::new();
    for p in patterns.iter().filter(|p| !p.contains("..")) {
        let segs: Vec<&str> = p.trim_end_matches('/').split('/').collect();
        for m in &manifests {
            let d = dir_of(m);
            let ds: Vec<&str> = d.split('/').collect();
            if ds.len() == segs.len() && ds.iter().zip(&segs).all(|(a, b)| *b == "*" || a == b) {
                dirs.insert(d.to_string());
            }
        }
    }
    dirs.into_iter()
        .filter(|d| !exclude.iter().any(|e| d == e || d.starts_with(&format!("{e}/"))))
        .filter_map(|d| krate(tree, &join(&d, "Cargo.toml")))
        .collect()
}

/// A smoke script: its file, the folder it is named after, and the app it launches by default.
#[derive(Clone, Debug, PartialEq)]
pub struct Script {
    pub path: String,
    pub dir: String,
    /// `package:binary:cwd` for the default script
    pub target: Option<String>,
}

/// The apps (`apps/<x>/Cargo.toml` with a binary) and the scripts: every `ci.splash` outside
/// hidden, `target` and `local` folders and outside skipped apps, then the default script for each
/// app without one (ported from tools/ci smoke.rs `discover`).
pub fn discover(tree: &Tree) -> (Vec<Krate>, Vec<Script>) {
    let hidden = |p: &str| p.split('/').any(|s| s.starts_with('.') || s == "target" || s == "local" || s.starts_with("target-"));
    let mut apps = Vec::new();
    let mut skipped_dirs: Vec<String> = Vec::new();
    for m in tree.files.iter().filter(|f| f.starts_with("apps/") && f.ends_with("/Cargo.toml") && f.matches('/').count() == 2) {
        let Some(k) = krate(tree, m) else { continue };
        if skipped(&k.package, &k.dir) {
            skipped_dirs.push(k.dir.clone());
            continue;
        }
        if !k.bins.is_empty() {
            apps.push(k);
        }
    }
    let under_skipped = |p: &str| skipped_dirs.iter().any(|d| p.starts_with(&format!("{d}/")));
    let mut scripts = Vec::new();
    let mut covered = BTreeSet::new();
    for f in tree.files.iter().filter(|f| (f.ends_with("/ci.splash") || f.as_str() == "ci.splash") && !hidden(f) && !under_skipped(f)) {
        let dir = dir_of(f).to_string();
        covered.insert(dir.clone());
        scripts.push(Script { path: f.clone(), dir: if dir.is_empty() { ".".into() } else { dir }, target: None });
    }
    for a in &apps {
        if covered.contains(&a.dir) {
            continue;
        }
        let bin = a.bins.iter().find(|b| !SKIP_APPS.iter().any(|s| s == b || Some(*s) == b.strip_prefix("makepad-"))).unwrap_or(&a.bins[0]);
        scripts.push(Script { path: DEFAULT_SCRIPT.into(), dir: a.dir.clone(), target: Some(format!("{}:{bin}:.", a.package)) });
    }
    scripts.sort_by(|a, b| a.dir.cmp(&b.dir));
    (apps, scripts)
}

/// The `ci.check_targets({packages: [...] targets: ci.host.targets | [...]})` calls of a script:
/// (packages, targets; None = every target). `ci.target.package` is `default_package`.
pub fn check_specs(text: &str, default_package: Option<&str>) -> Vec<(Vec<String>, Option<Vec<String>>)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("ci.check_targets(") {
        rest = &rest[at + "ci.check_targets(".len()..];
        let call = &rest[..rest.find(')').unwrap_or(rest.len())];
        let list = |key: &str| -> Option<&str> {
            let at = call.find(key)? + key.len();
            let v = call[at..].trim_start();
            if v.starts_with('[') {
                Some(&v[1..v.find(']').unwrap_or(v.len())])
            } else {
                Some("")
            }
        };
        let strings = |body: &str| -> Vec<String> { body.split('"').skip(1).step_by(2).map(str::to_string).collect() };
        let packages: Vec<String> = match list("packages:") {
            Some(body) if body.contains("ci.target.package") => default_package.map(|p| vec![p.to_string()]).unwrap_or_default(),
            Some(body) => strings(body),
            None => Vec::new(),
        };
        let targets = match list("targets:") {
            Some(body) if !body.is_empty() => Some(strings(body)),
            _ => None,
        };
        if !packages.is_empty() {
            out.push((packages, targets));
        }
    }
    out
}

/// target -> the packages checked there (from every script's `check_targets`).
pub fn matrix(tree: &Tree, scripts: &[Script]) -> BTreeMap<String, BTreeSet<String>> {
    let mut m: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for sc in scripts {
        let Some(text) = tree.read(&sc.path) else { continue };
        let default = sc.target.as_deref().and_then(|t| t.split(':').next());
        for (packages, targets) in check_specs(&text, default) {
            for (t, _) in TARGETS {
                if targets.as_ref().is_none_or(|ts| ts.iter().any(|x| x == t)) {
                    m.entry(t.to_string()).or_default().extend(packages.iter().cloned());
                }
            }
        }
    }
    m
}

/// The licensed slices of the commercial repository (`slices.json`): (app, roots).
pub fn slices(tree: &Tree) -> Vec<(String, Vec<String>)> {
    let Some(text) = tree.read("apps/commercial/slices.json") else { return Vec::new() };
    let Ok(Value::Obj(apps)) = makepad_strict_json::parse(text.as_bytes()) else { return Vec::new() };
    apps.into_iter()
        .map(|(app, roots)| (app, roots.as_arr().unwrap_or(&[]).iter().filter_map(|r| r.as_str().map(str::to_string)).collect::<Vec<_>>()))
        .filter(|(_, r)| !r.is_empty())
        .collect()
}

// ---------------------------------------------------------------- the plan

/// How a step's output is judged beyond its exit code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Judge {
    /// the exit code
    Exit,
    /// cargo `--message-format short`: a compiler warning fails it too (0 warnings)
    Cargo,
    /// `makepad check`: its `N errors` line too
    Check,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shard {
    pub id: String,
    pub kind: Kind,
    pub os: Os,
    /// needs a display (smoke)
    pub gui: bool,
    /// the commands, run one after the other in the lease's tree
    pub steps: Vec<Vec<String>>,
    pub judge: Judge,
    /// seconds it is expected to take (its measured average, else the kind's guess)
    pub est: f64,
}

fn argv(words: &[&str]) -> Vec<String> {
    words.iter().map(|w| w.to_string()).collect()
}

/// The JIT through cargo, as every fixture runs it: the node's one loader, a no-op when warm.
pub fn jit(args: &[&str]) -> Vec<String> {
    let mut v = argv(&["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--"]);
    v.extend(args.iter().map(|a| a.to_string()));
    v
}

/// `ci-leg` (tools/ci_leg) under the JIT.
pub fn ci_leg(args: &[&str]) -> Vec<String> {
    let mut v = jit(&["run", "makepad-ci-leg", "--"]);
    v.extend(args.iter().map(|a| a.to_string()));
    v
}

/// Groups of `packages` whose summed estimates stay near `per` seconds.
fn groups(items: Vec<(String, f64)>, per: f64) -> Vec<Vec<String>> {
    let mut out: Vec<Vec<String>> = Vec::new();
    let mut acc = per;
    for (p, est) in items {
        if acc + est > per || out.is_empty() {
            out.push(Vec::new());
            acc = 0.0;
        }
        out.last_mut().unwrap().push(p);
        acc += est;
    }
    out
}

/// Every shard of a run on `tree`. `timing` gives a shard's measured seconds; `rotation` picks
/// which smoke scripts must run on a Mac this time (every third, shifting by one each run).
pub fn plan(tree: &Tree, timing: &dyn Fn(&str) -> Option<f64>, rotation: u64) -> Vec<Shard> {
    let (_apps, scripts) = discover(tree);
    let m = matrix(tree, &scripts);
    let mut shards = Vec::new();
    let est = |id: &str, guess: f64| timing(id).unwrap_or(guess);
    let mut push = |id: String, kind: Kind, os: Os, gui: bool, steps: Vec<Vec<String>>, judge: Judge, guess: f64| {
        let e = est(&id, guess);
        shards.push(Shard { id, kind, os, gui, steps, judge, est: e });
    };
    // JIT checks: every workspace member on Linux, in groups of about a minute
    let mut ms = members(tree);
    ms.sort_by(|a, b| a.package.cmp(&b.package));
    for (i, g) in groups(ms.iter().map(|k| (k.package.clone(), 4.0)).collect(), 60.0).into_iter().enumerate() {
        let steps = g.iter().map(|p| jit(&["check", p])).collect();
        push(format!("jit:{:02}", i + 1), Kind::Build, Os::Linux, false, steps, Judge::Check, 60.0);
    }
    // the apps' own OS checks under the JIT on that OS (a Mac, Windows)
    for (target, os) in JIT_TARGETS.iter().filter(|(_, os)| *os != Os::Linux) {
        let Some(pkgs) = m.get(*target) else { continue };
        let tag = if *os == Os::Macos { "mjit" } else { "wjit" };
        for (i, g) in groups(pkgs.iter().map(|p| (p.clone(), 6.0)).collect(), 60.0).into_iter().enumerate() {
            let steps = g.iter().map(|p| jit(&["check", p])).collect();
            push(format!("{tag}:{:02}", i + 1), Kind::Build, *os, false, steps, Judge::Check, 60.0);
        }
    }
    // the cross targets under cargo: one shard per target
    for (target, build) in TARGETS {
        if JIT_TARGETS.iter().any(|(t, _)| *t == target) {
            continue;
        }
        let Some(pkgs) = m.get(target) else { continue };
        let mut pkgs: Vec<&String> = pkgs.iter().collect();
        if build == Build::Lib {
            // web and Android check an app's lib: a package without one is not asked
            let libs: BTreeSet<String> = tree.files.iter().filter(|f| f.ends_with("/Cargo.toml")).filter_map(|f| krate(tree, f)).filter(|k| k.lib).map(|k| k.package).collect();
            pkgs.retain(|p| libs.contains(*p));
        }
        if pkgs.is_empty() {
            continue;
        }
        let mode = match build {
            Build::Bin => "bin",
            Build::Lib => "lib",
            Build::Std => "std",
        };
        let mut step = argv(&["bash", "tools/run/fixture/ci_xcheck.sh", target, mode]);
        step.extend(pkgs.into_iter().cloned());
        let os = if target.contains("-apple-") { Os::Macos } else { Os::Linux };
        push(format!("check:{target}"), Kind::Build, os, false, vec![step], Judge::Cargo, 60.0);
    }
    // the platform's own tests: one shard per test binary
    for k in members(tree).into_iter().filter(|k| TEST_DIRS.iter().any(|d| k.dir == *d || k.dir.starts_with(&format!("{d}/")))) {
        if k.lib {
            push(format!("test:{}:lib", k.package), Kind::Test, Os::Linux, false, vec![jit(&["test", &k.package, "--lib"])], Judge::Exit, 20.0);
        }
        for t in &k.tests {
            push(format!("test:{}:{t}", k.package), Kind::Test, Os::Linux, false, vec![jit(&["test", &k.package, "--test", t])], Judge::Exit, 20.0);
        }
    }
    // the licensed slices
    for (app, roots) in slices(tree) {
        let roots = roots.join(",");
        push(format!("slice:{app}"), Kind::Build, Os::Linux, false, vec![ci_leg(&["slice", &app, &roots])], Judge::Exit, 40.0);
    }
    // the smoke scripts: any box with a display; every third (rotating) on a Mac
    for (i, sc) in scripts.iter().enumerate() {
        if sc.dir == "." {
            continue; // the workspace script: its checks and tests are the shards above
        }
        let os = if (i as u64 + rotation) % 3 == 0 { Os::Macos } else { Os::Any };
        let mut step = ci_leg(&["smoke", &sc.path, "--dir", &sc.dir]);
        if let Some(t) = &sc.target {
            step.extend(["--target".to_string(), t.clone()]);
        }
        push(format!("smoke:{}", sc.dir), Kind::Screen, os, true, vec![step], Judge::Exit, 30.0);
    }
    shards
}

// ---------------------------------------------------------------- judging a step

/// Compiler warnings in cargo's short format (`path:line:col: warning: ...`).
pub fn cargo_warnings(log: &str) -> Vec<String> {
    log.lines()
        .filter(|l| {
            let Some((head, _)) = l.split_once(": warning: ") else { return false };
            let mut parts = head.rsplitn(3, ':');
            let (col, line) = (parts.next(), parts.next());
            col.is_some_and(|c| c.bytes().all(|b| b.is_ascii_digit())) && line.is_some_and(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|l| crate::proto::clean(l, 300))
        .collect()
}

/// `makepad check`'s errors count (`..., N errors`), when it printed one.
pub fn check_errors(log: &str) -> Option<u64> {
    log.lines().rev().find_map(|l| l.trim_end().strip_suffix(" errors").and_then(|h| h.rsplit([' ', ',']).next()).and_then(|n| n.parse().ok()))
}

/// A step's verdict and summary from its exit code and output.
pub fn judge(judge: Judge, step: &str, exit: i32, log: &str) -> (bool, String) {
    if exit != 0 {
        return (false, format!("{step}: exit {exit}"));
    }
    match judge {
        Judge::Exit => (true, String::new()),
        Judge::Cargo => {
            let w = cargo_warnings(log);
            if w.is_empty() {
                (true, String::new())
            } else {
                (false, format!("{step}: {} warning{}", w.len(), if w.len() == 1 { "" } else { "s" }))
            }
        }
        Judge::Check => match check_errors(log) {
            Some(0) | None => (true, String::new()),
            Some(n) => (false, format!("{step}: {n} errors")),
        },
    }
}

/// A short name of a step for summaries: `check makepad-app-x`, `xcheck aarch64-apple-ios`.
fn step_name(step: &[String]) -> String {
    let words: Vec<&str> = step.iter().map(String::as_str).collect();
    match words.iter().position(|w| *w == "--") {
        Some(i) => words[i + 1..].iter().take(4).copied().collect::<Vec<_>>().join(" "),
        None if words.get(1) == Some(&"tools/run/fixture/ci_xcheck.sh") => format!("xcheck {}", words.get(2).unwrap_or(&"")),
        None => words.iter().take(4).copied().collect::<Vec<_>>().join(" "),
    }
}

// ---------------------------------------------------------------- the run

/// A box the run can use: its name, OS, arch and whether it shows windows.
#[derive(Clone, Debug, PartialEq)]
pub struct BoxInfo {
    pub node: String,
    pub os: Os,
    pub arch: String,
    pub gui: bool,
}

impl BoxInfo {
    pub fn fits(&self, s: &Shard) -> bool {
        (s.os == Os::Any || s.os == self.os) && (!s.gui || self.gui) && (s.os != Os::Any || self.os != Os::Windows)
    }

    fn needs(&self, s: &Shard) -> String {
        let mut n = format!("os={},arch={}", self.os.as_str(), self.arch);
        if s.gui {
            n.push_str(",gui=1");
        }
        n
    }
}

/// The boxes that take CI shards now: up, dev on, with capabilities.
pub fn boxes(ctx: &Ctx) -> Vec<BoxInfo> {
    ctx.leases
        .nodes()
        .into_iter()
        .filter(|n| n.up && n.dev)
        .filter_map(|n| {
            let c = n.caps?;
            if c.role().users() {
                return None;
            }
            let os = Os::of(&c.os);
            (os != Os::Any).then(|| BoxInfo { node: n.name.clone(), os, arch: c.arch.clone(), gui: c.display.is_some() })
        })
        .collect()
}

struct Pool {
    shards: Vec<Shard>,
    state: Vec<CiShard>,
    /// a shard's lease is being asked for on that box
    reserved: Vec<Option<String>>,
    /// boxes a shard came back incomplete from (it goes elsewhere next)
    lost: Vec<BTreeSet<String>>,
    /// per box: (shard index, start s, end s) in order
    done: BTreeMap<String, Vec<(usize, f64, f64)>>,
    stopped: bool,
}

impl Pool {
    fn finished(&self, i: usize) -> bool {
        matches!(self.state[i].state.as_str(), "ok" | "fail" | "incomplete")
    }

    fn all_done(&self) -> bool {
        (0..self.state.len()).all(|i| self.finished(i))
    }

    /// The longest pending shard this box can run.
    fn next_for(&self, b: &BoxInfo) -> Option<usize> {
        (0..self.shards.len())
            .filter(|&i| self.state[i].state == "wait" && self.reserved[i].is_none() && b.fits(&self.shards[i]) && !self.lost[i].contains(&b.node))
            .max_by(|&a, &c| self.shards[a].est.partial_cmp(&self.shards[c].est).unwrap_or(std::cmp::Ordering::Equal))
    }
}

/// A run's shard states for the state document.
fn publish(ctx: &Ctx, pool: &Pool) {
    let shards = pool.state.clone();
    ctx.store.update(|d| {
        if let Some(r) = d.ci.run.as_mut() {
            r.shards = shards;
        }
    });
}

/// The run's critical path (the box that finished last, its shards in order) and per-box load.
pub fn critical(done: &BTreeMap<String, Vec<(usize, f64, f64)>>, ids: &dyn Fn(usize) -> String) -> (String, Vec<String>) {
    let last = done.iter().max_by(|a, b| {
        let end = |v: &Vec<(usize, f64, f64)>| v.last().map_or(0.0, |x| x.2);
        end(a.1).partial_cmp(&end(b.1)).unwrap_or(std::cmp::Ordering::Equal)
    });
    let crit = match last {
        Some((node, v)) => format!(
            "{node} ends at {:.0} s: {}",
            v.last().map_or(0.0, |x| x.2),
            v.iter().map(|(i, a, b)| format!("{} {:.0} s", ids(*i), b - a)).collect::<Vec<_>>().join(", ")
        ),
        None => String::new(),
    };
    let boxes = done.iter().map(|(n, v)| format!("{n} {:.0} s busy, {} shards", v.iter().map(|(_, a, b)| b - a).sum::<f64>(), v.len())).collect();
    (crit, boxes)
}

/// Runs every shard of `pair` over the free boxes; the verdict and the shards as verify legs.
/// `stop` gives the run up (shards running finish their lease; nothing new starts).
pub fn gate(ctx: &Arc<Ctx>, pair: &Pair, dry: bool, stop: Arc<dyn Fn() -> bool + Send + Sync>) -> (Verdict, Vec<crate::state::Leg>) {
    let id = stamp(wall_ms());
    let dir = ctx.logs("ci").join(&id);
    let _ = std::fs::create_dir_all(&dir);
    super::prune(&ctx.logs("ci"), 40);
    let rotation = ctx.store.update(|d| {
        d.ci.rotation += 1;
        d.ci.rotation
    });
    let tree = match Tree::at(&ctx.repo, pair) {
        Ok(t) => t,
        Err(e) => {
            ctx.store.event("ci", Sev::Warn, format!("ci {}: the tree: {e}", pair.short()), None);
            return (Verdict::Incomplete, Vec::new());
        }
    };
    let shards = ctx.store.read(|d| plan(&tree, &|s| d.ci.timing(s), rotation));
    let state: Vec<CiShard> = shards
        .iter()
        .map(|s| CiShard { id: s.id.clone(), state: "wait".into(), log: dir.join(format!("{}.log", safe(&s.id))).to_string_lossy().into_owned(), ..Default::default() })
        .collect();
    let n = shards.len();
    let started = wall_ms();
    ctx.store.update(|d| d.ci.run = Some(CiRun { id: id.clone(), pair: pair.clone(), dry, started_ms: started, verdict: "running".into(), shards: state.clone(), ..Default::default() }));
    ctx.store.event("ci", Sev::Info, format!("ci {} started: {n} shards (run {id})", pair.short()), None);
    let pool = Arc::new((Mutex::new(Pool { reserved: vec![None; n], lost: vec![BTreeSet::new(); n], shards, state, done: BTreeMap::new(), stopped: false }), Condvar::new()));
    let t0 = Instant::now();
    let mut workers = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    // boxes join while the run goes (a box that comes up or gets free takes shards too)
    loop {
        {
            let (p, cv) = &*pool;
            let mut g = p.lock().unwrap();
            if stop() {
                g.stopped = true;
            }
            if g.all_done() || g.stopped {
                cv.notify_all();
                break;
            }
            // shards no box can ever take (no box of that OS) end incomplete
            let bs = boxes(ctx);
            for i in 0..g.shards.len() {
                if g.state[i].state == "wait" && g.reserved[i].is_none() && !bs.iter().any(|b| b.fits(&g.shards[i]) && !g.lost[i].contains(&b.node)) && t0.elapsed() > Duration::from_secs(120) {
                    g.state[i].state = "incomplete".into();
                    g.state[i].summary = format!("no free box of {}{}", g.shards[i].os.as_str(), if g.shards[i].gui { " with a display" } else { "" });
                }
            }
            drop(g);
            for b in bs {
                if seen.insert(b.node.clone()) {
                    let (ctx, pool, pair, dir) = (ctx.clone(), pool.clone(), pair.clone(), dir.clone());
                    workers.push(std::thread::Builder::new().name(format!("devhub-ci-{}", b.node)).spawn(move || worker(&ctx, &pool, &pair, &dir, b, t0)).ok());
                }
            }
        }
        let (p, cv) = &*pool;
        let g = p.lock().unwrap();
        let _ = cv.wait_timeout(g, Duration::from_secs(5));
        publish(ctx, &pool.0.lock().unwrap());
    }
    for w in workers.into_iter().flatten() {
        let _ = w.join();
    }
    let g = pool.0.lock().unwrap();
    let verdict = if g.state.iter().any(|s| s.state == "fail") {
        Verdict::Fail
    } else if g.stopped || g.state.iter().any(|s| s.state != "ok") {
        Verdict::Incomplete
    } else {
        Verdict::Ok
    };
    let ids = |i: usize| g.state[i].id.clone();
    let (crit, boxes) = critical(&g.done, &ids);
    let wall = t0.elapsed().as_secs_f64();
    let failed: Vec<String> = g.state.iter().filter(|s| s.state == "fail").map(|s| s.id.clone()).collect();
    let word = match verdict {
        Verdict::Ok => "ok",
        Verdict::Fail => "fail",
        Verdict::Incomplete => "incomplete",
    };
    let state = g.state.clone();
    let legs = state.iter().map(CiShard::leg).collect();
    ctx.store.update(|d| {
        if let Some(mut r) = d.ci.run.take() {
            r.shards = state;
            r.verdict = word.into();
            r.ended_ms = wall_ms();
            r.critical = crit.clone();
            r.boxes = boxes.clone();
            d.ci.runs.push(r);
        }
    });
    let line = format!(
        "ci {} {word}: {n} shards in {:.0} s wall{}; critical path {crit}",
        pair.short(),
        wall,
        if failed.is_empty() { String::new() } else { format!("; failed {}", failed.join(" ")) }
    );
    ctx.store.event("ci", if verdict == Verdict::Fail { Sev::Err } else { Sev::Info }, line, None);
    (verdict, legs)
}

fn safe(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect()
}

/// One box: pull the longest shard it can run, take a lease on it, run its steps, again.
fn worker(ctx: &Arc<Ctx>, pool: &Arc<(Mutex<Pool>, Condvar)>, pair: &Pair, _dir: &Path, b: BoxInfo, t0: Instant) {
    let (p, cv) = &**pool;
    let mut idle_until: Option<Instant> = None;
    loop {
        let i = {
            let mut g = p.lock().unwrap();
            loop {
                if g.all_done() || g.stopped {
                    return;
                }
                if idle_until.is_none_or(|t| Instant::now() >= t) {
                    if let Some(i) = g.next_for(&b) {
                        g.reserved[i] = Some(b.node.clone());
                        break i;
                    }
                }
                g = cv.wait_timeout(g, Duration::from_secs(5)).unwrap().0;
            }
        };
        let shard = p.lock().unwrap().shards[i].clone();
        let spec = Spec {
            lane: LANE.into(),
            agent: AGENT.into(),
            kind: shard.kind,
            needs: b.needs(&shard),
            prio: PRIO,
            rev: pair.rev(),
            repo_dir: ctx.repo.to_string_lossy().into_owned(),
            node: Some(b.node.clone()),
        };
        let asked = Instant::now();
        let stop = || asked.elapsed() > QUEUE_WAIT || p.lock().map(|g| g.stopped).unwrap_or(true);
        let held = match Held::acquire(&spec, &mut |_| {}, &stop) {
            Ok(h) => h,
            Err(_) => {
                // busy or held: the shard goes back, this box rests a while
                let mut g = p.lock().unwrap();
                g.reserved[i] = None;
                idle_until = Some(Instant::now() + BACKOFF);
                cv.notify_all();
                continue;
            }
        };
        {
            let mut g = p.lock().unwrap();
            g.state[i].state = "run".into();
            g.state[i].node = held.node.clone();
            g.state[i].lease = held.id.clone();
        }
        let log_path = p.lock().unwrap().state[i].log.clone();
        let start = t0.elapsed().as_secs_f64();
        let (state, summary) = run_steps(&held, &shard, Path::new(&log_path));
        let end = t0.elapsed().as_secs_f64();
        let lease = held.id.clone();
        held.release(Some(if state == "ok" { 0 } else { 1 }));
        // lane work took the box back (the master preempts background work): the shard waits again
        if state == "incomplete" && super::leg::was_preempted(&lease) {
            let mut g = p.lock().unwrap();
            g.reserved[i] = None;
            g.state[i].state = "wait".into();
            g.state[i].summary = format!("preempted on {} by lane work; asks again", b.node);
            idle_until = Some(Instant::now() + BACKOFF);
            cv.notify_all();
            continue;
        }
        let errors = if state == "fail" { super::ci_status::error_lines(&std::fs::read_to_string(&log_path).unwrap_or_default()) } else { Vec::new() };
        let mut g = p.lock().unwrap();
        g.reserved[i] = None;
        if state == "incomplete" && g.lost[i].len() < 2 {
            // a lost lease: the shard goes to another box once more
            g.lost[i].insert(b.node.clone());
            g.state[i].state = "wait".into();
            g.state[i].summary = summary;
        } else {
            g.state[i].state = state.into();
            g.state[i].summary = summary;
            g.state[i].errors = errors;
            g.state[i].secs = end - start;
            let node = g.state[i].node.clone();
            g.done.entry(node).or_default().push((i, start, end));
            if state != "incomplete" {
                let id = g.state[i].id.clone();
                ctx.store.update(|d| d.ci.measured(&id, end - start));
            }
        }
        cv.notify_all();
    }
}

/// The steps of a shard in its lease, their output appended to its log: (state, summary).
fn run_steps(held: &Held, shard: &Shard, log: &Path) -> (&'static str, String) {
    let Ok(mut f) = std::fs::File::create(log) else { return ("incomplete", "cannot write its log".into()) };
    let mut failed = Vec::new();
    for step in &shard.steps {
        let _ = writeln!(f, "$ {}", step.join(" "));
        let mut out: Vec<u8> = Vec::new();
        let r = held.run(step, &mut out);
        let _ = f.write_all(&out);
        let text = String::from_utf8_lossy(&out);
        match r {
            Ok(code) => {
                let (ok, why) = judge(shard.judge, &step_name(step), code, &text);
                let _ = writeln!(f, "== exit {code}{}", if ok { String::new() } else { format!(": {why}") });
                if !ok {
                    failed.push(why);
                }
            }
            Err(e) => {
                let _ = writeln!(f, "== lost: {e}");
                return ("incomplete", format!("{}: {e}", step_name(step)));
            }
        }
    }
    if failed.is_empty() {
        ("ok", format!("{} step{} ok", shard.steps.len(), if shard.steps.len() == 1 { "" } else { "s" }))
    } else {
        ("fail", failed.join("; "))
    }
}

// ---------------------------------------------------------------- service and CLI

pub const HELP: &str = "  ci [--json]                             the CI run in progress (or the last): shards, boxes, critical path
  ci run [<work> <main>] [--dry]          run CI on that pair now (default: the newest landed pair)
  ci plan [<work> <main>]                 the shards a run would have, with their boxes and estimates
  ci stop                                 give the run up (running shards finish)";

static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn pair_of(ctx: &Ctx, w: &[&str]) -> Result<Pair, String> {
    match w {
        [work, main, ..] if !work.starts_with('-') => {
            let p = Pair::new(&super::git(&ctx.repo, &["rev-parse", work])?, &super::git(&ctx.commercial(), &["rev-parse", main])?);
            p.valid().then_some(p).ok_or_else(|| "not a commit pair".to_string())
        }
        _ => super::snapshot(&ctx.repo).ok_or_else(|| "no landed pair now (a landing moves the tips)".to_string()),
    }
}

pub fn serve(ctx: &Arc<Ctx>, line: &str) -> Result<String, String> {
    let w: Vec<&str> = line.split_whitespace().skip(1).collect();
    match w.first().copied() {
        None | Some("status") => Ok(ctx.store.read(|d| {
            let r = d.ci.run.as_ref().or(d.ci.runs.last());
            r.map_or(obj(vec![("note", s("no CI run yet"))]), |r| crate::state::Doc { ci: crate::state::Ci { run: Some(r.clone()), ..Default::default() }, ..Default::default() }.to_value().get("ci").and_then(|c| c.get("run")).cloned().unwrap_or(Value::Null))
                .to_json()
        })),
        Some("plan") => {
            let pair = pair_of(ctx, &w[1..])?;
            let tree = Tree::at(&ctx.repo, &pair)?;
            let rotation = ctx.store.read(|d| d.ci.rotation + 1);
            let shards = ctx.store.read(|d| plan(&tree, &|s| d.ci.timing(s), rotation));
            let bs = boxes(ctx);
            let rows: Vec<Value> = shards
                .iter()
                .map(|sh| {
                    let fits: Vec<Value> = bs.iter().filter(|b| b.fits(sh)).map(|b| s(&b.node)).collect();
                    obj(vec![("id", s(&sh.id)), ("os", s(sh.os.as_str())), ("gui", Value::Bool(sh.gui)), ("est", Value::F64(sh.est)), ("steps", Value::Int(sh.steps.len() as i64)), ("boxes", Value::Arr(fits))])
                })
                .collect();
            Ok(obj(vec![("pair", s(pair.short())), ("shards", Value::Arr(rows))]).to_json())
        }
        Some("run") => {
            let dry = w.contains(&"--dry");
            let rest: Vec<&str> = w[1..].iter().copied().filter(|x| *x != "--dry").collect();
            let pair = pair_of(ctx, &rest)?;
            if RUNNING.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return Err("a CI run is going (devhub ci; devhub ci stop)".into());
            }
            STOP.store(false, std::sync::atomic::Ordering::SeqCst);
            let c = ctx.clone();
            let p = pair.clone();
            let _ = std::thread::Builder::new().name("devhub-ci".into()).spawn(move || {
                let _ = gate(&c, &p, dry, Arc::new(|| STOP.load(std::sync::atomic::Ordering::SeqCst)));
                RUNNING.store(false, std::sync::atomic::Ordering::SeqCst);
            });
            Ok(obj(vec![("started", s(pair.short())), ("dry", Value::Bool(dry))]).to_json())
        }
        Some("stop") => {
            STOP.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(obj(vec![("stopping", Value::Bool(RUNNING.load(std::sync::atomic::Ordering::SeqCst)))]).to_json())
        }
        Some(x) => Err(format!("ci: unknown word {x} (ci [status] | run | plan | stop)")),
    }
}

/// The run as lines: verdict, wall, critical path, boxes, then the shards not ok.
pub fn render(v: &Value) -> String {
    let t = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    if let Some(n) = v.get("note").and_then(|x| x.as_str()) {
        return format!("{n}\n");
    }
    let num = |x: &Value, k: &str| x.get(k).and_then(|y| y.as_u64()).unwrap_or(0);
    let shards = v.get("shards").and_then(|x| x.as_arr()).unwrap_or(&[]);
    let count = |st: &str| shards.iter().filter(|x| x.get("state").and_then(|y| y.as_str()) == Some(st)).count();
    let (start, end) = (num(v, "started_ms"), num(v, "ended_ms"));
    let wall = if end > 0 { format!("{:.0} s", (end - start) as f64 / 1000.0) } else { format!("{} running", super::age(start)) };
    let pair = v.get("pair").map(|p| crate::state::Pair::new(p.get("work").and_then(|x| x.as_str()).unwrap_or(""), p.get("main").and_then(|x| x.as_str()).unwrap_or("")).short()).unwrap_or_default();
    let mut out = format!(
        "ci {} {pair}: {} ({} shards: {} ok, {} fail, {} running, {} waiting, {} incomplete), wall {wall}\n",
        t("id"),
        t("verdict"),
        shards.len(),
        count("ok"),
        count("fail"),
        count("run"),
        count("wait"),
        count("incomplete")
    );
    if !t("critical").is_empty() {
        out.push_str(&format!("critical path: {}\n", t("critical")));
    }
    for b in v.get("boxes").and_then(|x| x.as_arr()).unwrap_or(&[]) {
        out.push_str(&format!("  box {}\n", b.as_str().unwrap_or("")));
    }
    for sh in shards.iter().filter(|x| !matches!(x.get("state").and_then(|y| y.as_str()), Some("ok") | Some("wait"))) {
        let g = |k: &str| sh.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        out.push_str(&format!("  {:10} {:40} {:8} {}\n", g("state"), g("id"), g("node"), g("summary")));
        for e in sh.get("errors").and_then(|x| x.as_arr()).unwrap_or(&[]).iter().take(6) {
            out.push_str(&format!("      {}\n", e.as_str().unwrap_or("")));
        }
    }
    out
}

/// `devhub ci ...` (None: not a ci word).
pub fn cli(args: &[&str]) -> Option<i32> {
    if args.first() != Some(&"ci") {
        return None;
    }
    let rest: Vec<&str> = args[1..].to_vec();
    if rest.first().is_some_and(|a| matches!(*a, "-h" | "--help" | "help")) {
        println!("{HELP}");
        return Some(0);
    }
    let json = rest.contains(&"--json");
    let words: Vec<&str> = rest.into_iter().filter(|a| *a != "--json").collect();
    let c = match crate::cli::control() {
        Ok(c) if crate::cli::has(&c, "ci") => c,
        _ => {
            eprintln!("makepad devhub ci: the devhub here is not the master, or an older one without ci");
            return Some(2);
        }
    };
    let line = format!("ci {}", words.join(" "));
    let r = crate::cli::ask(&c, None, "ci", &line).and_then(|lines| {
        let first = lines.first().cloned().ok_or("ci: no answer")?;
        let v = makepad_strict_json::parse_depth(first.as_bytes(), 16).map_err(|_| "ci: bad answer".to_string())?;
        match v.get("err").and_then(|e| e.as_str()) {
            Some(e) => Err(e.to_string()),
            None => Ok(v),
        }
    });
    Some(match r {
        Ok(v) => {
            if json || words.first().is_some_and(|w| *w != "status") {
                println!("{}", v.to_json());
            } else {
                print!("{}", render(&v));
            }
            0
        }
        Err(e) => {
            eprintln!("makepad devhub ci: {e}");
            1
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree() -> Tree {
        Tree::mem(vec![
            ("Cargo.toml", "[workspace]\nmembers = [\"platform\", \"widgets\", \"apps/*\", \"tools/ci_leg\"]\nexclude = [\"apps/gone\"]\n"),
            ("platform/Cargo.toml", "[package]\nname = \"makepad-platform\"\n"),
            ("platform/src/lib.rs", ""),
            ("platform/tests/remote.rs", ""),
            ("widgets/Cargo.toml", "[package]\nname = \"makepad-widgets\"\n[[test]]\nname = \"ui\"\npath = \"t/ui.rs\"\n"),
            ("widgets/src/lib.rs", ""),
            ("tools/ci_leg/Cargo.toml", "[package]\nname = \"makepad-ci-leg\"\n[[bin]]\nname = \"ci-leg\"\npath = \"src/main.rs\"\n"),
            ("tools/ci_leg/src/main.rs", ""),
            ("apps/calc/Cargo.toml", "[package]\nname = \"makepad-app-calc\"\n"),
            ("apps/calc/src/main.rs", ""),
            ("apps/calc/src/lib.rs", ""),
            ("apps/calc/ci.splash", "ci.step(\"x\", fn(){\n    ci.check_targets({packages: [\"makepad-app-calc\"] targets: ci.host.targets})\n})\n"),
            ("apps/tool/Cargo.toml", "[package]\nname = \"makepad-app-tool\"\n"),
            ("apps/tool/src/main.rs", ""),
            ("apps/hub/Cargo.toml", "[package]\nname = \"makepad-app-hub\"\n"),
            ("apps/hub/src/main.rs", ""),
            ("apps/hub/ci.splash", "ci.check_targets({packages: [\"makepad-app-hub\"] targets: [\"aarch64-apple-darwin\", \"x86_64-unknown-linux-gnu\"]})\n"),
            ("apps/mail/Cargo.toml", "[package]\nname = \"makepad-app-mail\"\n"),
            ("apps/mail/src/main.rs", ""),
            ("apps/mail/ci.splash", "nil\n"),
            ("apps/gone/Cargo.toml", "[package]\nname = \"gone\"\n"),
            ("ci.splash", "ci.check_targets({\n    packages: [\"makepad-platform\", \"makepad-widgets\"]\n    targets: ci.host.targets\n})\n"),
            (DEFAULT_SCRIPT, "ci.check_targets({packages: [ci.target.package] targets: ci.host.targets})\n"),
            ("apps/commercial/slices.json", "{\"scope\": [\"makepad-scope\"]}"),
            (".claude/x/ci.splash", "nil"),
        ])
    }

    #[test]
    fn discovery_finds_scripts_defaults_and_skips_like_the_ci_box() {
        let t = tree();
        let (apps, scripts) = discover(&t);
        assert_eq!(apps.iter().map(|a| a.package.as_str()).collect::<Vec<_>>(), ["makepad-app-calc", "makepad-app-hub", "makepad-app-tool"]);
        let dirs: Vec<&str> = scripts.iter().map(|s| s.dir.as_str()).collect();
        assert_eq!(dirs, [".", "apps/calc", "apps/hub", "apps/tool"]);
        assert_eq!(scripts[3].target.as_deref(), Some("makepad-app-tool:makepad-app-tool:."));
        assert_eq!(members(&t).iter().map(|k| k.package.as_str()).collect::<Vec<_>>(), ["makepad-app-calc", "makepad-app-hub", "makepad-app-mail", "makepad-app-tool", "makepad-platform", "makepad-ci-leg", "makepad-widgets"]);
    }

    #[test]
    fn the_matrix_follows_each_scripts_targets() {
        let t = tree();
        let (_, scripts) = discover(&t);
        let m = matrix(&t, &scripts);
        assert!(m["wasm32-unknown-unknown"].contains("makepad-app-tool"));
        assert!(m["wasm32-unknown-unknown"].contains("makepad-platform"));
        assert!(!m["wasm32-unknown-unknown"].contains("makepad-app-hub"));
        assert!(m["aarch64-apple-darwin"].contains("makepad-app-hub"));
        assert_eq!(check_specs("ci.check_targets({packages: [ci.target.package] targets: ci.host.targets})", Some("p")), vec![(vec!["p".to_string()], None)]);
    }

    #[test]
    fn the_plan_has_every_kind_of_shard_on_the_right_os() {
        let t = tree();
        let p = plan(&t, &|_| None, 0);
        let ids: Vec<&str> = p.iter().map(|s| s.id.as_str()).collect();
        for want in ["jit:01", "mjit:01", "wjit:01", "check:wasm32-unknown-unknown", "check:aarch64-apple-tvos", "test:makepad-platform:lib", "test:makepad-platform:remote", "test:makepad-widgets:ui", "slice:scope", "smoke:apps/calc", "smoke:apps/tool"] {
            assert!(ids.contains(&want), "{want} in {ids:?}");
        }
        assert!(!ids.iter().any(|i| i.starts_with("check:x86_64-unknown-linux-gnu") || i.starts_with("check:aarch64-apple-darwin") || *i == "smoke:."));
        let get = |id: &str| p.iter().find(|s| s.id == id).unwrap();
        assert_eq!(get("check:aarch64-apple-ios").os, Os::Macos);
        assert_eq!(get("check:x86_64-pc-windows-gnu").os, Os::Linux);
        assert_eq!(get("check:aarch64-apple-tvos").steps[0][3], "std");
        // wasm checks libs only: the tool app has no lib
        let wasm = &get("check:wasm32-unknown-unknown").steps[0];
        assert!(wasm.contains(&"makepad-app-calc".to_string()) && !wasm.contains(&"makepad-app-tool".to_string()));
        assert_eq!(get("wjit:01").os, Os::Windows);
        assert!(get("smoke:apps/calc").gui);
        // a third of the smoke scripts on a Mac, rotating
        let macs = |r: u64| plan(&t, &|_| None, r).into_iter().filter(|s| s.id.starts_with("smoke:") && s.os == Os::Macos).map(|s| s.id).collect::<Vec<_>>();
        assert_ne!(macs(0), macs(1));
    }

    /// The plan of the checkout this test runs in (skipped outside a git checkout): every kind of
    /// shard is there in the numbers the tree has.
    #[test]
    fn plans_the_real_tree() {
        let repo = super::super::repo();
        let head = |dir: &Path| super::super::git(dir, &["rev-parse", "HEAD"]).ok();
        let (Some(work), main) = (head(&repo), head(&repo.join(COMMERCIAL))) else { return };
        let t = Tree::at(&repo, &Pair::new(&work, &main.unwrap_or_default())).unwrap();
        let p = plan(&t, &|_| None, 0);
        let count = |pre: &str| p.iter().filter(|s| s.id.starts_with(pre)).count();
        let summary = ["jit:", "mjit:", "wjit:", "check:", "test:", "slice:", "smoke:"].map(|k| format!("{k}{}", count(k))).join(" ");
        println!("plan of {}: {} shards: {summary}", crate::state::short(&work), p.len());
        for s in p.iter().filter(|s| s.id.starts_with("check:")) {
            println!("  {} {} packages on {}", s.id, s.steps[0].len() - 4, s.os.as_str());
        }
        assert!(count("jit:") > 0 && count("mjit:") > 0 && count("wjit:") > 0 && count("test:") > 10 && count("smoke:") > 20, "{summary}");
        assert_eq!(count("check:"), 12, "{summary}");
    }

    #[test]
    fn steps_are_judged_by_exit_warnings_and_error_counts() {
        assert_eq!(judge(Judge::Exit, "x", 1, "").0, false);
        assert!(judge(Judge::Cargo, "x", 0, "warning: unused manifest key\n    Finished").0);
        assert!(!judge(Judge::Cargo, "x", 0, "src/a.rs:3:9: warning: unused variable: `x`\n").0);
        assert!(!judge(Judge::Check, "x", 0, "makepad check: 1193 body owners of the checked crates, 9 compiled in all, 2 errors\n").0);
        assert!(judge(Judge::Check, "x", 0, "makepad check: 1193 body owners, 9 compiled in all, 0 errors\n").0);
    }

    #[test]
    fn boxes_take_what_fits_and_the_critical_path_is_the_last_box() {
        let lin = BoxInfo { node: "101".into(), os: Os::Linux, arch: "x86_64".into(), gui: false };
        let win = BoxInfo { node: "105".into(), os: Os::Windows, arch: "x86_64".into(), gui: true };
        let sh = |os, gui| Shard { id: "s".into(), kind: Kind::Build, os, gui, steps: Vec::new(), judge: Judge::Exit, est: 1.0 };
        assert!(lin.fits(&sh(Os::Linux, false)) && !lin.fits(&sh(Os::Any, true)) && !win.fits(&sh(Os::Any, true)) && win.fits(&sh(Os::Windows, false)));
        let mut done = BTreeMap::new();
        done.insert("101".to_string(), vec![(0, 0.0, 30.0), (1, 30.0, 50.0)]);
        done.insert("106".to_string(), vec![(2, 0.0, 70.0)]);
        let (crit, boxes) = critical(&done, &|i| format!("s{i}"));
        assert_eq!(crit, "106 ends at 70 s: s2 70 s");
        assert_eq!(boxes, vec!["101 50 s busy, 2 shards", "106 70 s busy, 1 shards"]);
        assert_eq!(groups(vec![("a".into(), 30.0), ("b".into(), 30.0), ("c".into(), 30.0)], 60.0), vec![vec!["a".to_string(), "b".into()], vec!["c".into()]]);
    }
}
