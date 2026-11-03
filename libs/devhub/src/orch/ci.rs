//! `devhub ci`: independent regression and sweep runs, each planned from its pair's tree. One worker
//! per allowed CI box (a lease.conf with `slots.ci`: a Linux and a Windows box) pulls shards
//! longest first, behind lane work.
//! Smoke boots use the separate CI tree;
//! sweep shards use their own lease kinds at background priority, with the background cap and
//! preemption. A sweep never holds the gate's run slot or takes priority over lane work.
//!
//! The shards (everything under the JIT, `makepad check|test|run`, except where it cannot be yet):
//! - `jit:<n>`: `makepad check` of the workspace members, in groups (Linux); `mjit:<n>` the apps'
//!   macOS check (a Mac), `wjit:<n>` their Windows check (the Windows CI box);
//! - `test:<pkg>:<target>`: one test binary of the platform, draw and widgets crates (`makepad test
//!   <pkg> --lib|--test <t>`);
//! - `wtest:<pkg>:<target>`: a rotating third of them natively on Windows (the Windows CI box);
//! - `check:<target>`: cargo's `check --release --target` of every app the scripts check there:
//!   the cross targets the JIT does not build (Apple-SDK targets on a Mac, the rest on Linux);
//! - `slice:<app>`: a licensed commercial slice, cut, linted and checked (`ci-leg slice`);
//! - `smoke:<dir>`: one app's `ci.splash` (launch hidden, drive it, grab, the vision judge on the AI
//!   fleet: `ci-leg smoke`) on any box that shows windows; a rotating third of them on a Mac, a third on Windows.
//!
//! What is checked where comes from the tree: the apps (`apps/<x>/Cargo.toml`), their
//! `ci.splash` (`ci.check_targets({packages, targets})`), the workspace members, the commercial
//! `slices.json`. The run's state is [`crate::state::Ci`]; [`ci`] runs it for the CI bucket.
//!
//! A master restart resumes both slots: each follows its steps on the leases that came back,
//! from the logs of its own run. Later pairs read passes from each crate's committed `ci.toml`;
//! the master holds only run state.
//!
//! A shard that fails asks a second box of that OS before it counts as failed: one bad box (its
//! toolchain, its cache) must not fail every shard it pulls. A pass there is a pass, reported loudly
//! with the first box and its error, and a box that [`BOX_OUT`] shards passed elsewhere after
//! failing on it pulls nothing more in this run.

use super::leg::{Held, Spec};
use super::verify::Verdict;
use super::{stamp, Ctx};
use crate::lease::Kind;
use crate::proto::{wall_ms, Sev};
use crate::state::{Ci, CiRun, CiShard, CiSpan, Pair};
use makepad_strict_json::{obj, s, Value};
use makepad_toml_parser::{parse_toml, Toml};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Verify's priority: the lowest; landings and lanes go first (and background work holds back
/// while lane work waits).
pub const PRIO: i32 = super::verify::PRIO;
/// The lane and agent the shards' leases are under.
const LANE: &str = crate::lease::sched::CI_LANE;
const AGENT: &str = "devhub-ci";
/// Sweeps stay background work.
const SWEEP_AGENT: &str = "devhub-ci-sweep";
/// A shard whose lease is not granted within this goes back to the pool (another box takes it). Long
/// enough to keep its place in a busy box's queue: lanes come and go in minutes.
const QUEUE_WAIT: Duration = Duration::from_secs(300);
/// A step's exit code for "this box cannot run me" (EX_TEMPFAIL): the shard goes to another box.
pub const UNFIT: i32 = 75;
/// A shard lost this often on one box goes to another (or ends incomplete).
const LOST_TRIES: u32 = 2;
/// A box that failed this many shards which passed on another box is out of the run.
const BOX_OUT: usize = 3;
/// A waiting shard's summary while its lease is asked for.
const QUEUED: &str = "queued on ";
/// A box that did not grant a lease waits this long before asking again.
const BACKOFF: Duration = Duration::from_secs(15);
/// After a master starts, this long for its nodes to be heard: until then no box is known to be gone.
const HEAR: Duration = Duration::from_secs(120);
/// A shard with no box of its kind here waits this long for one to come back, once nothing else is
/// left to run (a box updating or restarting is back in minutes).
const NO_BOX_WAIT: Duration = Duration::from_secs(15 * 60);
/// A run with no shard progress this long reports what its asks wait for.
const STALLED: Duration = Duration::from_secs(30 * 60);

/// A run no shard of which ended for [`STALLED`]: what runs and what the waiting ones wait for.
fn stall_line(pair: &str, state: &[CiShard], ended: usize) -> String {
    let running: Vec<String> = state.iter().filter(|s| s.state == "run").map(|s| format!("{} on {} ({})", s.id, s.node, s.lease)).take(4).collect();
    let mut why: Vec<&str> = state.iter().filter(|s| s.state == "wait" && !s.summary.is_empty()).map(|s| s.summary.as_str()).collect();
    why.sort_unstable();
    why.dedup();
    let waiting = state.iter().filter(|s| s.state == "wait").count();
    format!("ci {pair}: no shard ended in {} min ({ended} of {} ended): running {}; {waiting} waiting: {}", STALLED.as_secs() / 60, state.len(), running.join(", "), why[..why.len().min(3)].join("; "))
}

/// A test binary that takes longer than this on Linux stays off the one Windows box.
const WINDOWS_TEST_MAX: f64 = 300.0;

/// The apps the CI box skipped (its ci.toml `skip_apps`): not apps, or not launchable alone.
pub const SKIP_APPS: &[&str] = &["source-library", "mixer", "mail", "wm-all", "wm-dyn", "asset-server", "flow-server", "ai-hub"];
/// The platform's own crates: their tests run in every CI run.
pub const TEST_DIRS: &[&str] = &["platform", "draw", "widgets", "tools/ci_leg", "libs/remote", "libs/devhub", "libs/ai/services"];
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
    /// the pair the tree is (`<work>,<main>`; empty for an in-memory tree): digests of a pair's
    /// tree never change, so they are kept by it for the process's life
    id: String,
    pub files: Vec<String>,
    blobs: BTreeMap<String, String>,
    read: Box<dyn Fn(&str) -> Option<String> + Send + Sync>,
    cache: Mutex<BTreeMap<String, Option<String>>>,
}

impl Tree {
    pub fn blob(&self, path: &str) -> Option<&str> {
        self.blobs.get(path).map(String::as_str)
    }

    pub fn read(&self, path: &str) -> Option<String> {
        if let Some(text) = self.cache.lock().unwrap().get(path) { return text.clone(); }
        let text = (self.read)(path);
        self.cache.lock().unwrap().insert(path.into(), text.clone());
        text
    }

    pub fn has(&self, path: &str) -> bool {
        self.files.binary_search_by(|f| f.as_str().cmp(path)).is_ok()
    }

    /// An in-memory tree (tests).
    pub fn mem(files: Vec<(&str, &str)>) -> Tree {
        let map: BTreeMap<String, String> = files.iter().map(|(p, t)| (p.to_string(), t.to_string())).collect();
        let names: Vec<String> = map.keys().cloned().collect();
        Tree { id: String::new(), cache: Mutex::new(BTreeMap::new()), blobs: map.iter().map(|(p, t)| (p.clone(), hex_digest(t.as_bytes()))).collect(), files: names, read: Box::new(move |p| map.get(p).cloned()) }
    }

    /// The tree of `pair` in `repo` (and `repo/apps/commercial`).
    pub fn at(repo: &Path, pair: &Pair) -> Result<Tree, String> {
        let com = repo.join(COMMERCIAL);
        let mut blobs = ls_tree(repo, &pair.work)?;
        let has_com = !pair.main.is_empty() && com.join(".git").exists();
        if has_com {
            blobs.extend(ls_tree(&com, &pair.main)?.into_iter().map(|(p, h)| (format!("{COMMERCIAL}/{p}"), h)));
        }
        let files = blobs.keys().cloned().collect();
        let main = Batch::start(repo)?;
        let commercial = if has_com { Some(Batch::start(&com)?) } else { None };
        let (work, cmain) = (pair.work.clone(), pair.main.clone());
        let main = Mutex::new(main);
        let commercial = commercial.map(Mutex::new);
        Ok(Tree {
            id: format!("{},{}", pair.work, pair.main),
            cache: Mutex::new(BTreeMap::new()),
            files,
            blobs,
            read: Box::new(move |p| match (p.strip_prefix("apps/commercial/"), &commercial) {
                (Some(rest), Some(c)) => c.lock().ok()?.blob(&cmain, rest),
                _ => main.lock().ok()?.blob(&work, p),
            }),
        })
    }
}

fn ls_tree(dir: &Path, sha: &str) -> Result<BTreeMap<String, String>, String> {
    let o = crate::git_exec::command()
        .arg("-C")
        .arg(dir)
        .args(["ls-tree", "-r", "-z", sha])
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("git ls-tree: {e}"))?;
    if !o.status.success() {
        return Err(format!("git ls-tree {}: {}", crate::state::short(sha), String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&o.stdout).split('\0').filter_map(|line| {
        let (head, path) = line.split_once('\t')?;
        (head.split_whitespace().nth(1) == Some("blob")).then(|| (path.to_string(), head.split_whitespace().nth(2).unwrap_or("").to_string()))
    }).collect())
}

/// One `git cat-file --batch` per repository: blobs by `<sha>:<path>`.
struct Batch {
    child: std::process::Child,
    out: BufReader<std::process::ChildStdout>,
}

impl Batch {
    fn start(dir: &Path) -> Result<Batch, String> {
        let mut child = crate::git_exec::command()
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

/// Every package in the tree; a duplicate name keeps the shorter directory.
pub fn package_dirs(tree: &Tree) -> BTreeMap<String, String> {
    let mut dirs = BTreeMap::<String, String>::new();
    for path in tree.files.iter().filter(|p| p.as_str() == "Cargo.toml" || p.ends_with("/Cargo.toml")) {
        // (the name alone: `krate` walks every file of the tree per manifest)
        let Some(name) = tree.read(path).and_then(|t| parse_toml(&t).ok()).and_then(|d| d.get_path(&["package", "name"]).and_then(Toml::as_str).map(str::to_string)) else { continue };
        let d = dir_of(path).to_string();
        let dir = dirs.entry(name).or_insert_with(|| d.clone());
        if d.len() < dir.len() { *dir = d; }
    }
    dirs
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

/// Every crate cargo builds in the workspace: the members and, as cargo takes them, the path
/// dependencies they reach (the commercial apps come in so, through `apps/commercial`'s
/// `cfg(any())` dependencies), minus `workspace.exclude` and crates with a workspace of their own.
pub fn workspace_crates(tree: &Tree) -> Vec<Krate> {
    let Some(doc) = tree.read("Cargo.toml").and_then(|t| parse_toml(&t).ok()) else { return Vec::new() };
    let exclude: Vec<String> = doc.get_path(&["workspace", "exclude"]).and_then(Toml::as_array).unwrap_or(&[]).iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
    let excluded = |d: &str| exclude.iter().any(|e| d == e || d.starts_with(&format!("{e}/")));
    let mut todo: Vec<String> = members(tree).into_iter().map(|k| k.dir).collect();
    // `apps/*/src/..`: the dirs of the matches' parents
    for p in doc.get_path(&["workspace", "members"]).and_then(Toml::as_array).unwrap_or(&[]).iter().filter_map(|v| v.as_str()).filter(|p| p.ends_with("/src/..")) {
        let segs: Vec<&str> = p.trim_end_matches("/src/..").split('/').collect();
        for m in tree.files.iter().filter(|f| f.ends_with("/Cargo.toml")) {
            let d = dir_of(m);
            let ds: Vec<&str> = d.split('/').collect();
            if ds.len() == segs.len() && ds.iter().zip(&segs).all(|(a, b)| *b == "*" || a == b) && tree.files.iter().any(|f| f.starts_with(&format!("{d}/src/"))) {
                todo.push(d.to_string());
            }
        }
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    while let Some(dir) = todo.pop() {
        if !seen.insert(dir.clone()) || excluded(&dir) {
            continue;
        }
        let manifest = join(&dir, "Cargo.toml");
        let Some(doc) = tree.read(&manifest).and_then(|t| parse_toml(&t).ok()) else { continue };
        if dir != "" && doc.root.get("workspace").is_some() {
            continue;
        }
        todo.extend(path_deps_of(tree, &dir, &doc));
        if let Some(k) = krate(tree, &manifest) {
            out.push(k);
        }
    }
    out.sort_by(|a, b| a.package.cmp(&b.package));
    out.dedup_by(|a, b| a.package == b.package);
    out
}

/// The dirs of a crate's path dependencies (every kind, every target) that are crates of the tree.
fn path_deps(tree: &Tree, dir: &str) -> Vec<String> {
    tree.read(&join(dir, "Cargo.toml")).and_then(|t| parse_toml(&t).ok()).map_or(Vec::new(), |doc| path_deps_of(tree, dir, &doc))
}

fn path_deps_of(tree: &Tree, dir: &str, doc: &makepad_toml_parser::TomlDocument) -> Vec<String> {
    let kinds = ["dependencies", "dev-dependencies", "build-dependencies"];
    let mut tables: Vec<&Toml> = kinds.iter().filter_map(|k| doc.root.get(*k)).collect();
    if let Some(targets) = doc.root.get("target").and_then(Toml::as_table) {
        for t in targets.values().filter_map(Toml::as_table) {
            tables.extend(kinds.iter().filter_map(|k| t.get(*k)));
        }
    }
    let mut out = Vec::new();
    for deps in tables.into_iter().filter_map(Toml::as_table) {
        for dep in deps.values().filter_map(Toml::as_table) {
            if let Some(path) = dep.get("path").and_then(Toml::as_str) {
                let d = normalize(&join(dir, path));
                if tree.has(&join(&d, "Cargo.toml")) {
                    out.push(d);
                }
            }
        }
    }
    out
}

fn normalize(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for c in p.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out.join("/")
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
    /// a test binary: its exit code, and it ran at least one test (a target that runs none on its
    /// box tested nothing, which is no pass; the plan leaves out what has no test there)
    Tests,
    /// cargo `--message-format short`: a compiler warning fails it too (0 warnings)
    Cargo,
    /// `makepad check`: its `N errors` line too, and a warning fails it (0 warnings: two dead-code
    /// warnings of the JIT build stood from 10-31 while only errors counted)
    Check,
    /// a measurement: its `pass rate` line is the result and a miss (exit 3) does not fail the run;
    /// any other exit does (it could not measure)
    Report,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Shard {
    pub id: String,
    pub kind: Kind,
    pub os: Os,
    /// needs a display (smoke)
    pub gui: bool,
    /// needs a GPU (tests of crates that reach the HAL or a GPU runtime)
    pub gpu: bool,
    /// the commands, run one after the other in the lease's tree
    pub steps: Vec<Vec<String>>,
    pub judge: Judge,
    /// seconds it is expected to take (its measured average, else the kind's guess)
    pub est: f64,
    /// the crates it reads, by their dirs (and through them their path dependencies); None: it
    /// reads more than crates, so its result holds for its own pair only
    pub dirs: Option<Vec<String>>,
    /// the box alone ([`CiConf::whole`])
    pub whole: bool,
    /// its cores ([`CiConf::cores`]): the lease's cost
    pub cores: Option<f64>,
}

impl Shard {
    /// What it runs: a result stands only for the same steps.
    pub fn sig(&self) -> String {
        let text = self.steps.iter().map(|s| s.join("\0")).collect::<Vec<_>>().join("\n");
        makepad_network::digest::sha256_hash(text.as_bytes())[..8].iter().map(|b| format!("{b:02x}")).collect()
    }
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

/// The OS a smoke script is pinned to: its line `// ci os: <os>`.
pub(crate) fn pinned_os(text: &str) -> Option<Os> {
    text.lines().find_map(|l| l.strip_prefix("// ci os:")).filter(|os| os.trim() != "").map(|os| Os::of(os.trim())).filter(|os| *os != Os::Any || text.lines().any(|l| l.trim_end() == "// ci os: any"))
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

/// About how long a batch of crate tests runs (by the crates' measured times).
const GROUP_SECS: f64 = 240.0;
/// A crate's tests with no measured time yet.
const GUESS_CRATE_SECS: f64 = 20.0;

/// The dirs of the crates whose path dependencies reach the HAL or a GPU runtime (their tests may
/// draw or compute on a GPU).
fn gpu_reaching(tree: &Tree, all: &[Krate]) -> BTreeSet<String> {
    const GPU: [&str; 4] = ["platform/hal", "libs/vulkan", "libs/cuda", "libs/ai/vulkan"];
    let deps: BTreeMap<String, Vec<String>> = all.iter().map(|k| (k.dir.clone(), path_deps(tree, &k.dir))).collect();
    let mut out = BTreeSet::new();
    for k in all {
        let (mut todo, mut seen) = (vec![k.dir.clone()], BTreeSet::new());
        while let Some(d) = todo.pop() {
            if GPU.iter().any(|g| d == *g || d.starts_with(&format!("{g}/"))) {
                out.insert(k.dir.clone());
                break;
            }
            if seen.insert(d.clone()) {
                todo.extend(deps.get(&d).cloned().unwrap_or_else(|| path_deps(tree, &d)));
            }
        }
    }
    out
}

/// How often a run commits the proofs of the units that passed since the last batch.
const PROOF_BATCH: Duration = Duration::from_secs(600);

/// A shard running this many times its measured time gets the overdue verdict ...
const OVERDUE_TIMES: f64 = 10.0;
/// ... and at least this long ...
const OVERDUE_MIN_SECS: f64 = 1800.0;
/// ... and at most this long (a measured time can come from a run that hung, then was ended)
const OVERDUE_MAX_SECS: f64 = 7200.0;

/// The apps the live replay takes in turn, one per CI run.
const REPLAY_APPS: [&str; 5] = ["makepad-app-storybook", "makepad-fab", "makepad-stage", "makepad-app-synth", "makepad-app-amp"];

/// The gate's candidates on `tree`: every end app booted under the JIT with its smoke script
/// (hidden window, its grabs judged), one shard per platform it ships on (`// ci os: <os>` names
/// one, `any` Linux and Mac, none all three); [`select`] keeps one platform of each app whose
/// inputs changed. No crate tests, no cargo builds: those are the sweep's ([`sweep_plan`]).
pub fn plan(tree: &Tree, timing: &dyn Fn(&str) -> Option<f64>, _rotation: u64) -> Vec<Shard> {
    // what runs through ci-leg: a tree from before it (a bisect's baseline) has none to ask
    if !tree.has("tools/ci_leg/Cargo.toml") {
        return Vec::new();
    }
    let (_apps, scripts) = discover(tree);
    let mut shards = Vec::new();
    for sc in scripts.iter().filter(|sc| sc.dir != ".") {
        let oses: Vec<Os> = match tree.read(&sc.path).and_then(|text| pinned_os(&text)) {
            Some(Os::Any) => vec![Os::Linux, Os::Macos],
            Some(os) => vec![os],
            None => vec![Os::Linux, Os::Macos, Os::Windows],
        };
        for os in oses {
            // the boot only grabs: run_shard copies the grabs back before release and judges here
            let mut step = ci_leg(&["smoke", &sc.path, "--dir", &sc.dir]);
            if let Some(t) = &sc.target {
                step.extend(["--target".to_string(), t.clone()]);
            }
            let id = format!("smoke:{}:{}", os.as_str(), sc.dir);
            let est = timing(&id).unwrap_or(30.0);
            let conf = conf_at(tree, &sc.dir);
            shards.push(Shard { id, kind: Kind::Ci, os, gui: true, gpu: false, steps: vec![step], judge: Judge::Exit, est, dirs: Some(vec![sc.dir.clone()]), whole: conf.whole, cores: conf.cores });
        }
    }
    shards
}

/// The app a gate shard boots (`app:<dir>`), from its id `smoke:<os>:<dir>`.
pub fn unit_of(id: &str) -> Option<String> {
    let rest = id.strip_prefix("smoke:")?;
    let (os, dir) = rest.split_once(':')?;
    (Os::of(os) != Os::Any).then(|| format!("app:{dir}"))
}

/// The OS a path is for, when it is OS-specific code (a path segment names it).
fn os_of_path(path: &str) -> Option<Os> {
    path.split('/').find_map(|seg| match seg.trim_end_matches(".rs") {
        "windows" | "win32" | "d3d11" | "d3d12" => Some(Os::Windows),
        "macos" | "apple" | "metal" | "cocoa" => Some(Os::Macos),
        "linux" | "wayland" | "x11" => Some(Os::Linux),
        _ => None,
    })
}

/// A proof belongs to the crate it exercises.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Unit {
    pub dir: String,
    pub section: String,
}

impl Unit {
    fn app(dir: &str) -> Unit {
        Unit { dir: dir.into(), section: "app".into() }
    }

    fn file(&self) -> String {
        join(&self.dir, "ci.toml")
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proof {
    pub digest: String,
    pub pair: Pair,
    pub capture: String,
    pub date: String,
}

pub type Proofs = BTreeMap<(String, Os), Proof>;
pub const PROOF_HEADER: &str = "# CI (devhub ci): [ci] is hand-written settings; the other sections are proofs devhub writes.\n";

/// A crate's hand-written CI settings (the `[ci]` section of its ci.toml; strings, so the merge
/// driver reads them like proofs). They are not inputs: ci.toml is out of every digest.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CiConf {
    /// `box = "whole"`: its gate boot takes the box alone (no lease but inspect beside it)
    pub whole: bool,
    /// `cores = "<n>"`: what its gate boot uses (the lease's cost)
    pub cores: Option<f64>,
    /// `test_features = "a,b"`: the features the sweep tests the crate with
    pub test_features: Vec<String>,
    /// `os = "linux,windows"`: the OSes its tests run on (empty: every OS)
    pub os: Vec<Os>,
}

impl CiConf {
    fn is_empty(&self) -> bool {
        *self == CiConf::default()
    }

    fn fields(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if self.whole {
            out.push(("box", "whole".to_string()));
        }
        if let Some(c) = self.cores {
            out.push(("cores", format!("{c}")));
        }
        if !self.test_features.is_empty() {
            out.push(("test_features", self.test_features.join(",")));
        }
        if !self.os.is_empty() {
            out.push(("os", self.os.iter().map(|os| os.as_str()).collect::<Vec<_>>().join(",")));
        }
        out
    }
}

pub fn read_conf(text: &str) -> Result<CiConf, String> {
    let doc = parse_toml(text).map_err(|e| format!("ci.toml: {e:?}"))?;
    let Some(t) = doc.root.get("ci").and_then(Toml::as_table) else { return Ok(CiConf::default()) };
    let mut c = CiConf::default();
    for (k, v) in t {
        let v = v.as_str().ok_or_else(|| format!("ci.toml: [ci] {k} is a string"))?;
        match k.as_str() {
            "box" if v == "whole" => c.whole = true,
            "cores" => c.cores = Some(v.parse::<f64>().ok().filter(|n| *n > 0.0).ok_or_else(|| format!("ci.toml: [ci] cores = {v:?}: a positive number"))?),
            "test_features" => c.test_features = v.split(',').map(str::trim).filter(|f| !f.is_empty()).map(str::to_string).collect(),
            "os" => {
                c.os = v.split(',').map(str::trim).filter(|os| !os.is_empty()).map(Os::of).collect();
                if c.os.contains(&Os::Any) {
                    return Err(format!("ci.toml: [ci] os = {v:?}: linux, macos or windows"));
                }
            }
            _ => return Err(format!("ci.toml: [ci] {k} = {v:?}: unknown (box = \"whole\", cores, test_features, os)")),
        }
    }
    Ok(c)
}

/// A crate's settings at this tree (none: defaults).
pub fn conf_at(tree: &Tree, dir: &str) -> CiConf {
    tree.read(&join(dir, "ci.toml")).and_then(|t| read_conf(&t).ok()).unwrap_or_default()
}

fn runs_on(conf: &CiConf, os: Os) -> bool {
    conf.os.is_empty() || conf.os.contains(&os)
}

fn crate_runs_on(tree: &Tree, dir: &str, os: Os) -> bool {
    runs_on(&conf_at(tree, dir), os)
}

/// The features a crate's tests run with: its defaults plus its `test_features`, and the
/// `--features` words that turn the latter on.
fn test_features(tree: &Tree, dir: &str, mut feats: BTreeSet<String>) -> (BTreeSet<String>, Vec<String>) {
    let extra = conf_at(tree, dir).test_features;
    let with = if extra.is_empty() { Vec::new() } else { vec!["--features".to_string(), extra.join(",")] };
    feats.extend(extra);
    (feats, with)
}

pub fn proof_path(path: &str) -> bool {
    path.rsplit('/').next() == Some("ci.toml")
}

pub fn read_proofs(text: &str) -> Result<Proofs, String> {
    let doc = parse_toml(text).map_err(|e| format!("ci.toml: {e:?}"))?;
    let mut out = Proofs::new();
    let mut sections = vec!["app".to_string(), "tests".to_string()];
    if let Some(jobs) = doc.root.get("job").and_then(Toml::as_table) {
        sections.extend(jobs.keys().map(|name| format!("job.{name}")));
    }
    for section in sections {
        for os in [Os::Linux, Os::Macos, Os::Windows] {
            let mut path: Vec<&str> = section.split('.').collect();
            path.push(os.as_str());
            let Some(table) = doc.get_path(&path).and_then(Toml::as_table) else { continue };
            let get = |key: &str| table.get(key).and_then(Toml::as_str).unwrap_or("").to_string();
            let proof = Proof { digest: get("digest"), pair: Pair::new(&get("work"), &get("main")), capture: get("capture"), date: get("date") };
            if proof.digest.len() != 64 || !proof.digest.bytes().all(|b| b.is_ascii_hexdigit()) || !crate::state::hex40(&proof.pair.work)
                || (!proof.pair.main.is_empty() && !crate::state::hex40(&proof.pair.main))
                || proof.date.len() != 10 || !proof.date.bytes().enumerate().all(|(i, b)| if i == 4 || i == 7 { b == b'-' } else { b.is_ascii_digit() }) {
                return Err(format!("ci.toml: invalid [{section}.{}]", os.as_str()));
            }
            out.insert((section.clone(), os), proof);
        }
    }
    Ok(out)
}

/// The file as the merge driver writes it too: sections by name, keys by name.
pub fn write_proofs(conf: &CiConf, proofs: &Proofs) -> String {
    let mut sections: BTreeMap<String, BTreeMap<&str, String>> = BTreeMap::new();
    if !conf.is_empty() {
        sections.insert("ci".into(), conf.fields().into_iter().collect());
    }
    for ((section, os), p) in proofs {
        let mut f = BTreeMap::from([("digest", p.digest.clone()), ("work", p.pair.work.clone()), ("main", p.pair.main.clone()), ("date", p.date.clone())]);
        if section == "app" {
            f.insert("capture", p.capture.clone());
        }
        sections.insert(format!("{section}.{}", os.as_str()), f);
    }
    let mut out = PROOF_HEADER.to_string();
    for (name, f) in sections {
        out.push_str(&format!("\n[{name}]\n"));
        for (k, v) in f {
            out.push_str(&format!("{k} = {v:?}\n"));
        }
    }
    out
}

fn proofs_at(tree: &Tree, unit: &Unit) -> Result<Proofs, String> {
    tree.read(&unit.file()).map_or_else(|| Ok(Proofs::new()), |t| read_proofs(&t))
}

fn hex_digest(bytes: &[u8]) -> String {
    makepad_network::digest::sha256_hash(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// `ls-tree -r` rows, already scoped to a unit; proof blobs never enter the hash.
pub fn digest_lines(lines: &str, external: &[String], os: Os) -> String {
    digest_platform(&digest_input(lines, external), os)
}

fn digest_input(lines: &str, external: &[String]) -> String {
    makepad_ci_merge::digest_input(lines, external)
}

fn digest_platform(input: &str, os: Os) -> String {
    makepad_ci_merge::digest_platform(input, os.as_str())
}

struct Locked {
    name: String,
    version: String,
    source: String,
    checksum: String,
    deps: Vec<String>,
}

struct Lock {
    text: String,
    packages: Vec<Locked>,
    names: BTreeMap<String, Vec<usize>>,
}

impl Lock {
    fn parse(text: String) -> Result<Lock, String> {
        let doc = parse_toml(&text).map_err(|e| format!("Cargo.lock: {e:?}"))?;
        let mut packages = Vec::new();
        let mut names: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for t in doc.root.get("package").and_then(Toml::as_array_of_tables).ok_or("Cargo.lock: no packages")? {
            let get = |k: &str| t.get(k).and_then(Toml::as_str).unwrap_or("").to_string();
            let name = get("name");
            names.entry(name.clone()).or_default().push(packages.len());
            packages.push(Locked { name, version: get("version"), source: get("source"), checksum: get("checksum"), deps: t.get("dependencies").and_then(Toml::as_array).unwrap_or(&[]).iter().filter_map(|v| v.as_str().map(str::to_string)).collect() });
        }
        Ok(Lock { text, packages, names })
    }

    fn external(&self, roots: &BTreeSet<String>) -> Result<Vec<String>, String> {
        let mut todo = Vec::new();
        for name in roots {
            let local: Vec<usize> = self.names.get(name).into_iter().flatten().copied().filter(|&i| self.packages[i].source.is_empty()).collect();
            // (a crate outside the workspace's lock, e.g. an excluded or patched one: its files are in
            // the digest, what it pulls from a registry is not known here)
            todo.extend(local);
        }
        let mut seen = BTreeSet::new();
        let mut out = BTreeSet::new();
        while let Some(i) = todo.pop() {
            if !seen.insert(i) {
                continue;
            }
            let p = &self.packages[i];
            if !p.source.is_empty() {
                out.insert(format!("{}@{} {}", p.name, p.version, if p.checksum.is_empty() { &p.source } else { &p.checksum }));
            }
            for dep in &p.deps {
                let mut words = dep.split_whitespace();
                let name = words.next().ok_or("Cargo.lock: empty dependency")?;
                let version = words.next();
                let source = words.next().map(|s| s.trim_matches(['(', ')']));
                let ids: Vec<usize> = self.names.get(name).into_iter().flatten().copied().filter(|&j| {
                    let q = &self.packages[j];
                    version.is_none_or(|v| q.version == v) && source.is_none_or(|s| q.source == s)
                }).collect();
                // (an entry the lock does not resolve uniquely: what each candidate pulls counts)
                todo.extend(ids);
            }
        }
        Ok(out.into_iter().collect())
    }
}

/// A unit's digests (Linux, macOS, Windows) and its closure, by (pair, lock, dir, app section).
type DigestKey = (String, String, String, bool);
static DIGESTS: LazyLock<Mutex<HashMap<DigestKey, ([String; 3], BTreeSet<String>)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Entries kept before the cache starts over (about a hundred pairs of every unit).
const DIGESTS_KEPT: usize = 100_000;

/// One tree walk per repo, shared by discovery and every unit; no per-app git processes.
pub struct Digests {
    pub(crate) lock: String,
    hashes: BTreeMap<(Unit, Os), String>,
    closures: BTreeMap<Unit, BTreeSet<String>>,
}

impl Digests {
    pub fn at(repo: &Path, tree: &Tree, units: &[Unit]) -> Result<Digests, String> {
        let lock = if units.is_empty() { String::new() } else { std::fs::read_to_string(repo.join("Cargo.lock")).map_err(|e| format!("Cargo.lock: {e}"))? };
        Self::with_lock(tree, units, lock)
    }

    pub(crate) fn with_lock(tree: &Tree, units: &[Unit], text: String) -> Result<Digests, String> {
        if units.is_empty() { return Ok(Digests { lock: text, hashes: BTreeMap::new(), closures: BTreeMap::new() }); }
        let lock_id = if tree.id.is_empty() { String::new() } else { hex_digest(text.as_bytes()) };
        let lock = Lock::parse(text)?;
        let manifests: BTreeMap<String, String> = tree.files.iter().filter(|p| p.ends_with("Cargo.toml")).filter_map(|p| tree.read(p).map(|t| (p.clone(), t))).collect();
        let package_names: BTreeMap<String, Result<Option<String>, String>> = manifests.iter().map(|(path, text)| {
            (dir_of(path).to_string(), parse_toml(text).map(|doc| doc.get_path(&["package", "name"]).and_then(Toml::as_str).map(str::to_string)).map_err(|e| format!("{path}: {e:?}")))
        }).collect();
        let read = |p: &str| manifests.get(p).cloned();
        let jit: BTreeSet<String> = super::roll::crate_dirs_from(JIT_DIR, &read).into_iter().filter(|d| !NOT_JIT.iter().any(|n| under(d, n))).collect();
        let mut hashes: BTreeMap<(Unit, Os), String> = BTreeMap::new();
        let mut closures: BTreeMap<Unit, BTreeSet<String>> = BTreeMap::new();
        let mut computed: BTreeMap<(String, bool), Unit> = BTreeMap::new();
        for unit in units.iter().collect::<BTreeSet<_>>() {
            let key = (unit.dir.clone(), unit.section == "app");
            let kept_key = (tree.id.clone(), lock_id.clone(), unit.dir.clone(), unit.section == "app");
            if !tree.id.is_empty() {
                if let Some((h, dirs)) = DIGESTS.lock().unwrap().get(&kept_key) {
                    for (os, hash) in [Os::Linux, Os::Macos, Os::Windows].into_iter().zip(h) {
                        hashes.insert((unit.clone(), os), hash.clone());
                    }
                    closures.insert(unit.clone(), dirs.clone());
                    continue;
                }
            }
            if let Some(old) = computed.get(&key) {
                for os in [Os::Linux, Os::Macos, Os::Windows] {
                    let hash = hashes[&(old.clone(), os)].clone();
                    hashes.insert((unit.clone(), os), hash);
                }
                closures.insert(unit.clone(), closures[old].clone());
                continue;
            }
            let mut dirs = super::roll::crate_dirs_from(&unit.dir, &read);
            if unit.section == "app" {
                dirs.extend(jit.iter().cloned());
            }
            let mut names = BTreeSet::new();
            for dir in &dirs {
                match package_names.get(dir).ok_or_else(|| format!("no Cargo.toml for {dir}"))? {
                    Ok(Some(name)) => { names.insert(name.clone()); }
                    Ok(None) => {}
                    Err(e) => return Err(e.clone()),
                }
            }
            let external = lock.external(&names)?;
            let mut files = BTreeMap::new();
            for dir in &dirs {
                let prefix = format!("{}/", dir.trim_end_matches('/'));
                // a package nested in this one's directory is an input only when it is in the closure
                let nested: Vec<&str> = package_names.range(prefix.clone()..).take_while(|(d, _)| d.starts_with(&prefix)).filter(|(d, name)| matches!(name, Ok(Some(_))) && !dirs.contains(*d)).map(|(d, _)| d.as_str()).collect();
                for (path, hash) in tree.blobs.range(prefix.clone()..).take_while(|(p, _)| p.starts_with(&prefix)) {
                    if !proof_path(path) && !nested.iter().any(|n| under(path, n)) {
                        files.insert(path, hash);
                    }
                }
            }
            let lines = files.iter().map(|(p, h)| format!("100644 blob {h}\t{p}\n")).collect::<String>();
            let input = digest_input(&lines, &external);
            let h = [Os::Linux, Os::Macos, Os::Windows].map(|os| digest_platform(&input, os));
            for (os, hash) in [Os::Linux, Os::Macos, Os::Windows].into_iter().zip(&h) {
                hashes.insert((unit.clone(), os), hash.clone());
            }
            if !tree.id.is_empty() {
                let mut kept = DIGESTS.lock().unwrap();
                if kept.len() >= DIGESTS_KEPT { kept.clear(); }
                kept.insert(kept_key, (h, dirs.clone()));
            }
            closures.insert(unit.clone(), dirs);
            computed.insert(key, unit.clone());
        }
        Ok(Digests { lock: lock.text, hashes, closures })
    }

    pub fn of(&self, unit: &Unit, os: Os) -> &str {
        self.hashes.get(&(unit.clone(), os)).map_or("", String::as_str)
    }
}

fn units_of(shard: &Shard) -> Vec<Unit> {
    let section = if shard.id.starts_with("smoke:") { "app".to_string() }
        else if shard.id.starts_with("test:") || shard.id.starts_with("wtest:") { "tests".to_string() }
        else { format!("job.{}", safe(&shard.id).replace('.', "_")) };
    if shard.id.starts_with("live:") {
        return vec![Unit { dir: JIT_DIR.into(), section }];
    }
    shard.dirs.iter().flatten().map(|d| Unit { dir: d.clone(), section: section.clone() }).collect()
}

fn newest<'a>(proofs: &'a Proofs, section: &str) -> Option<&'a Proof> {
    proofs.iter().filter(|((s, _), _)| s == section).map(|(_, p)| p).max_by_key(|p| &p.date)
}

type Changes = BTreeMap<(String, String), Option<Inputs>>;

fn changed_paths(repo: &Path, pair: &Pair, last: Option<&Proof>, dirs: Option<&BTreeSet<String>>, cache: &mut Changes) -> Vec<String> {
    let (Some(proof), Some(dirs)) = (last, dirs) else { return Vec::new() };
    let input = cache.entry((proof.pair.work.clone(), proof.pair.main.clone())).or_insert_with(|| Inputs::between(repo, &proof.pair, pair));
    input.as_ref().map(|x| x.changed.iter().filter(|p| dirs.iter().any(|d| under(p, d))).cloned().collect()).unwrap_or_default()
}

/// An app runs on one OS only; a matching proof on any shipped OS keeps it. All available
/// platforms, including Macs, rotate by oldest proof date, then load per box.
pub fn select(repo: &Path, pair: &Pair, tree: &Tree, digests: &Digests, candidates: Vec<Shard>, boxes_of: &dyn Fn(Os) -> usize, going: &BTreeSet<String>) -> Result<(Vec<Shard>, Vec<(String, String)>), String> {
    let mut changes = Changes::new();
    let mut by_unit: BTreeMap<Unit, Vec<Shard>> = BTreeMap::new();
    for shard in candidates {
        if let Some(unit) = units_of(&shard).into_iter().next() {
            by_unit.entry(unit).or_default().push(shard);
        }
    }
    let mut assigned: BTreeMap<Os, usize> = BTreeMap::new();
    let (mut out, mut lines) = (Vec::new(), Vec::new());
    for (unit, options) in by_unit {
        let proofs = proofs_at(tree, &unit)?;
        if let Some((sh, proof)) = options.iter().find_map(|sh| proofs.get(&(unit.section.clone(), sh.os)).filter(|p| p.digest == digests.of(&unit, sh.os)).map(|p| (sh, p))) {
            lines.push((sh.id.clone(), format!("kept: proof on {} from {} {}", sh.os.as_str(), proof.pair.short(), proof.date)));
            out.push(sh.clone());
            continue;
        }
        let changed = changed_paths(repo, pair, newest(&proofs, &unit.section), digests.closures.get(&unit), &mut changes);
        let only_os = changed.iter().map(|p| os_of_path(p)).collect::<Option<BTreeSet<Os>>>().filter(|s| s.len() == 1).and_then(|s| s.into_iter().next());
        let pick = match options.iter().find(|s| going.contains(&s.id)).or_else(|| only_os.and_then(|os| options.iter().find(|s| s.os == os))) {
            Some(s) => s.clone(),
            None => {
                let load = |os: Os| *assigned.get(&os).unwrap_or(&0) as f64 / boxes_of(os).max(1) as f64;
                let date = |os: Os| proofs.get(&(unit.section.clone(), os)).map_or("", |p| p.date.as_str());
                let usable: Vec<&Shard> = options.iter().filter(|s| boxes_of(s.os) > 0).collect();
                let from = if usable.is_empty() { options.iter().collect() } else { usable };
                (*from.iter().min_by(|a, b| (date(a.os), load(a.os)).partial_cmp(&(date(b.os), load(b.os))).unwrap_or(std::cmp::Ordering::Equal)).unwrap()).clone()
            }
        };
        *assigned.entry(pick.os).or_default() += 1;
        let why = changed.first().map_or_else(|| format!("runs on {}: no proof", pick.os.as_str()), |p| format!("runs on {}: {p} changed", pick.os.as_str()));
        lines.push((pick.id.clone(), why));
        out.push(pick);
    }
    Ok((out, lines))
}

/// Every shard of the sweep on `tree` (`devhub ci sweep`: background work alongside the gate,
/// never a gate): the JIT checks of every crate, the cross targets, every crate's tests, the
/// slices, the smoke scripts and the live replays. Its report lists the failures by crate.
/// `timing` gives a shard's measured seconds; `rotation` picks which smoke scripts must run on a
/// Mac this time (every third, shifting by one each run).
pub fn sweep_plan(tree: &Tree, timing: &dyn Fn(&str) -> Option<f64>, rotation: u64) -> Vec<Shard> {
    let (_apps, scripts) = discover(tree);
    let m = matrix(tree, &scripts);
    let own = jit_own_dirs(tree);
    let test_step = |dir: &str, args: &[&str]| {
        if !own.contains(dir) { return jit(args); }
        let mut step = argv(&["cargo", "test", "--release", "-p", args[1]]);
        step.extend(args[2..].iter().map(|a| a.to_string()));
        step
    };
    let mut shards = Vec::new();
    let est = |id: &str, guess: f64| timing(id).unwrap_or(guess);
    // a package's dir (a package not found keeps its shard to its own pair)
    let dirs_of: BTreeMap<String, String> = tree.files.iter().filter(|f| f.ends_with("/Cargo.toml")).filter_map(|f| krate(tree, f)).map(|k| (k.package, k.dir)).collect();
    let dirs = |pkgs: &mut dyn Iterator<Item = &String>| -> Option<Vec<String>> { pkgs.map(|p| dirs_of.get(p).cloned()).collect() };
    let mut push = |id: String, kind: Kind, os: Os, gui: bool, steps: Vec<Vec<String>>, judge: Judge, guess: f64, dirs: Option<Vec<String>>| {
        let e = est(&id, guess);
        shards.push(Shard { id, kind, os, gui, gpu: kind == Kind::Test, steps, judge, est: e, dirs, whole: false, cores: None });
    };
    // JIT checks: every workspace member on Linux, in groups of about a minute
    let mut ms = members(tree);
    ms.sort_by(|a, b| a.package.cmp(&b.package));
    for (i, g) in groups(ms.iter().map(|k| (k.package.clone(), 4.0)).collect(), 60.0).into_iter().enumerate() {
        let steps = g.iter().map(|p| jit(&["check", p])).collect();
        push(format!("jit:{:02}", i + 1), Kind::Build, Os::Linux, false, steps, Judge::Check, 60.0, dirs(&mut g.iter()));
    }
    // the apps' own OS checks under the JIT on that OS (a Mac, Windows)
    for (target, os) in JIT_TARGETS.iter().filter(|(_, os)| *os != Os::Linux) {
        let Some(pkgs) = m.get(*target) else { continue };
        let tag = if *os == Os::Macos { "mjit" } else { "wjit" };
        for (i, g) in groups(pkgs.iter().map(|p| (p.clone(), 6.0)).collect(), 60.0).into_iter().enumerate() {
            let steps = g.iter().map(|p| jit(&["check", p])).collect();
            push(format!("{tag}:{:02}", i + 1), Kind::Build, *os, false, steps, Judge::Check, 60.0, dirs(&mut g.iter()));
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
        let read = dirs(&mut pkgs.iter().copied());
        step.extend(pkgs.into_iter().cloned());
        let os = if target.contains("-apple-") { Os::Macos } else { Os::Linux };
        push(format!("check:{target}"), Kind::Build, os, false, vec![step], Judge::Cargo, 60.0, read);
    }
    // the platform's own tests: one shard per test binary on Linux, a rotating third on Windows
    // (one box: the whole set would be most of a run's wall)
    let mut tests = 0u64;
    for k in members(tree).into_iter().filter(|k| TEST_DIRS.iter().any(|d| k.dir == *d || k.dir.starts_with(&format!("{d}/")))) {
        let lib = k.lib.then(|| ("lib".to_string(), vec!["--lib".to_string()]));
        let (feats, with) = test_features(tree, &k.dir, default_features(tree, &k));
        for (target, select) in lib.into_iter().chain(k.tests.iter().map(|t| (t.clone(), vec!["--test".to_string(), t.clone()]))) {
            let mut args = vec!["test", k.package.as_str()];
            args.extend(with.iter().map(String::as_str));
            args.extend(select.iter().map(String::as_str));
            let id = format!("test:{}:{target}", k.package);
            let long = timing(&id).is_some_and(|secs| secs > WINDOWS_TEST_MAX);
            // a target with no test that runs on an OS gets no shard there (it would pass on nothing)
            let files = test_files(tree, &k, &target);
            if crate_runs_on(tree, &k.dir, Os::Linux) && has_tests(tree, files.as_deref(), Os::Linux, &feats) {
                push(id, Kind::Test, Os::Linux, false, vec![test_step(&k.dir, &args)], Judge::Tests, 20.0, Some(vec![k.dir.clone()]));
            }
            if (tests + rotation) % 3 == 0 && !long && crate_runs_on(tree, &k.dir, Os::Windows) && has_tests(tree, files.as_deref(), Os::Windows, &feats) {
                push(format!("wtest:{}:{target}", k.package), Kind::Test, Os::Windows, false, vec![test_step(&k.dir, &args)], Judge::Tests, 60.0, Some(vec![k.dir.clone()]));
            }
            tests += 1;
        }
    }
    // every other crate of the workspace with tests (the commercial apps, libs, tools): its
    // `makepad test` (cargo for JIT-own crates), one step per crate, batched into shards of about
    // [`GROUP_SECS`] by their measured times; a GPU box only for crates that reach the HAL or a
    // GPU runtime; a rotating third of the batches on Windows too
    let mut extra: Vec<Shard> = Vec::new();
    let all = workspace_crates(tree);
    let gpu_dirs = gpu_reaching(tree, &all);
    let (mut on_gpu, mut plain) = (Vec::new(), Vec::new());
    let mut with_of: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for k in all.iter().filter(|k| !TEST_DIRS.iter().any(|d| k.dir == *d || k.dir.starts_with(&format!("{d}/")))) {
        let (feats, with) = test_features(tree, &k.dir, default_features(tree, k));
        let args: Vec<String> = [vec!["test".to_string(), k.package.clone()], with.clone()].concat();
        with_of.insert(k.package.clone(), with);
        let targets: Vec<String> = k.lib.then(|| "lib".to_string()).into_iter().chain(k.tests.iter().cloned()).collect();
        let files: Vec<Option<Vec<String>>> = targets.iter().map(|t| test_files(tree, k, t)).collect();
        let on = |os: Os| crate_runs_on(tree, &k.dir, os) && files.iter().any(|f| has_tests(tree, f.as_deref(), os, &feats));
        let item = (k.package.clone(), k.dir.clone(), timing(&format!("test:{}", k.package)).unwrap_or(GUESS_CRATE_SECS), on(Os::Linux), on(Os::Windows));
        let gpu = gpu_dirs.contains(&k.dir);
        if timing(&format!("failing:{}", k.package)).is_some() {
            // alone, under its own name
            let step = vec![test_step(&k.dir, &args.iter().map(String::as_str).collect::<Vec<_>>())];
            if item.3 {
                extra.push(Shard { id: format!("test:{}", k.package), kind: Kind::Test, os: Os::Linux, gui: false, gpu, steps: step.clone(), judge: Judge::Tests, est: item.2, dirs: Some(vec![k.dir.clone()]), whole: false, cores: None });
            }
            continue;
        }
        if gpu { on_gpu.push(item) } else { plain.push(item) }
    }
    for (tag, gpu, list) in [("gpu", true, on_gpu), ("cpu", false, plain)] {
        for (os, pre, keep) in [(Os::Linux, "test", 3usize), (Os::Windows, "wtest", 4)] {
            let items: Vec<(String, String, f64)> = list.iter().filter(|it| if keep == 3 { it.3 } else { it.4 }).map(|it| (it.0.clone(), it.1.clone(), it.2)).collect();
            let est: BTreeMap<String, (String, f64)> = items.iter().map(|(p, d, e)| (p.clone(), (d.clone(), *e))).collect();
            for (i, g) in groups(items.iter().map(|(p, _, e)| (p.clone(), *e)).collect(), GROUP_SECS).into_iter().enumerate() {
                if os == Os::Windows && (i as u64 + rotation) % 3 != 0 {
                    continue;
                }
                let steps = g.iter().map(|p| {
                    let args: Vec<String> = [vec!["test".to_string(), p.clone()], with_of[p].clone()].concat();
                    test_step(&est[p].0, &args.iter().map(String::as_str).collect::<Vec<_>>())
                }).collect();
                let secs: f64 = g.iter().map(|p| est[p].1).sum();
                let dirs = g.iter().map(|p| est[p].0.clone()).collect();
                extra.push(Shard { id: format!("{pre}:{tag}:{:02}", i + 1), kind: Kind::Test, os, gui: false, gpu, steps, judge: Judge::Tests, est: secs, dirs: Some(dirs), whole: false, cores: None });
            }
        }
    }
    // what runs through ci-leg: a tree from before it (a bisect's baseline) has none to ask
    if !tree.has("tools/ci_leg/Cargo.toml") {
        shards.extend(extra);
        return shards;
    }
    // the licensed slices
    for (app, roots) in slices(tree) {
        let read = dirs(&mut roots.iter());
        let roots = roots.join(",");
        push(format!("slice:{app}"), Kind::Build, Os::Linux, false, vec![ci_leg(&["slice", &app, &roots])], Judge::Exit, 40.0, read);
    }
    // the smoke scripts: any box with a display; every third (rotating) on a Mac
    for (i, sc) in scripts.iter().enumerate() {
        if sc.dir == "." {
            continue; // the workspace script: its checks and tests are the shards above
        }
        // a script may name its one OS (`// ci os: linux`); otherwise a rotating third on a Mac,
        // a third on Windows, the rest on any Linux or Mac box
        let pinned = tree.read(&sc.path).and_then(|text| pinned_os(&text));
        let os = pinned.unwrap_or(match (i as u64 + rotation) % 3 {
            0 => Os::Macos,
            1 => Os::Windows,
            _ => Os::Any,
        });
        let mut step = ci_leg(&["smoke", &sc.path, "--dir", &sc.dir]);
        if let Some(t) = &sc.target {
            step.extend(["--target".to_string(), t.clone()]);
        }
        push(format!("smoke:{}", sc.dir), Kind::Screen, os, true, vec![step], Judge::Exit, 30.0, Some(vec![sc.dir.clone()]));
    }
    // nothing breaks a running app: every edit kind, and the real landings replayed as live edits
    if tree.has("tools/run/fixture/live_kinds.py") {
        push("live:kinds".to_string(), Kind::Screen, Os::Linux, true, vec![argv(&["python3", "tools/run/fixture/live_kinds.py", "--jobs", "6"])], Judge::Exit, 300.0, None);
    }
    // a restart-rung edit is the exec restart: one process, the app's state kept, no leftover threads
    if tree.has("tools/run/fixture/restart_exec.py") {
        push("live:restart".to_string(), Kind::Screen, Os::Linux, true, vec![argv(&["python3", "tools/run/fixture/restart_exec.py"])], Judge::Exit, 240.0, None);
    }
    if tree.has("tools/run/fixture/live_replay.py") {
        // the apps in turn: the newest landings as live edits, measured (old landings' bugs are no
        // new landing's to clear, so a miss reports and does not fail the run), and a landing into
        // what makepad is built from (offer, click, back), under a WM every other run, which gates
        let app = REPLAY_APPS[rotation as usize % REPLAY_APPS.len()];
        push("live:replay".to_string(), Kind::Screen, Os::Linux, true, vec![argv(&["python3", "tools/run/fixture/live_replay.py", "--app", app, "--count", "40"])], Judge::Report, 900.0, None);
        let mut rebuild = vec!["python3", "tools/run/fixture/live_replay.py", "--host", "--app", app];
        if rotation % 2 == 0 {
            rebuild.push("--wm");
        }
        push("live:rebuild".to_string(), Kind::Screen, Os::Linux, true, vec![argv(&rebuild)], Judge::Exit, 500.0, None);
    }
    shards.extend(extra);
    shards
}

// ---------------------------------------------------------------- which targets test anything

/// The source files of a crate's test target: the lib's `src` (its binaries' files left out), or
/// `tests/<name>.rs` with its `tests/<name>/` modules. None: not known (a `[[test]]` path elsewhere).
fn test_files(tree: &Tree, k: &Krate, target: &str) -> Option<Vec<String>> {
    let rs = |f: &&String| f.ends_with(".rs");
    if target == "lib" {
        let src = join(&k.dir, "src/");
        let (main, bins) = (join(&k.dir, "src/main.rs"), join(&k.dir, "src/bin/"));
        return Some(tree.files.iter().filter(|f| f.starts_with(&src) && **f != main && !f.starts_with(&bins)).filter(rs).cloned().collect());
    }
    let root = join(&k.dir, &format!("tests/{target}.rs"));
    if !tree.has(&root) {
        return None;
    }
    let sub = join(&k.dir, &format!("tests/{target}/"));
    Some(std::iter::once(root).chain(tree.files.iter().filter(|f| f.starts_with(&sub)).filter(rs).cloned()).collect())
}

/// The features a plain build of the crate has: `default` and what it turns on.
fn default_features(tree: &Tree, k: &Krate) -> BTreeSet<String> {
    let doc = tree.read(&join(&k.dir, "Cargo.toml")).and_then(|t| parse_toml(&t).ok());
    let list = |name: &str| -> Vec<String> {
        doc.as_ref().and_then(|d| d.get_path(&["features", name])).and_then(Toml::as_array).unwrap_or(&[]).iter().filter_map(|v| v.as_str().map(str::to_string)).collect()
    };
    let (mut on, mut todo) = (BTreeSet::new(), list("default"));
    while let Some(f) = todo.pop() {
        // (`dep:x` and `x/y` turn on dependencies, not features of this crate)
        if f.contains(':') || f.contains('/') || !on.insert(f.clone()) {
            continue;
        }
        todo.extend(list(&f));
    }
    on
}

/// Whether a test target may run a test on `os`: a `#[test]` without `#[ignore]` whose `#[cfg]`s
/// and whose file's `#![cfg]` can hold there. False only when sure (no files known: true): a
/// target this misses runs none, which fails its shard ([`Judge::Tests`]).
fn has_tests(tree: &Tree, files: Option<&[String]>, os: Os, feats: &BTreeSet<String>) -> bool {
    let Some(files) = files else { return true };
    files.iter().any(|f| tree.read(f).is_some_and(|text| file_tests_on(&text, os, feats)))
}

fn file_tests_on(text: &str, os: Os, feats: &BTreeSet<String>) -> bool {
    let lines: Vec<&str> = text.lines().map(str::trim).collect();
    // the file's own `#![cfg(..)]` (inner attributes come first, after comments)
    for l in lines.iter().filter(|l| !l.is_empty() && !l.starts_with("//")).take_while(|l| l.starts_with("#![")) {
        if let Some(p) = l.strip_prefix("#![cfg(").and_then(|r| r.strip_suffix(")]")) {
            if cfg_holds(p, os, feats) == Some(false) {
                return false;
            }
        }
    }
    let attr = |l: &str| l.starts_with("#[") || l.starts_with("//");
    (0..lines.len()).filter(|&i| lines[i] == "#[test]").any(|i| {
        // the test's attributes: the lines of attributes and comments around it
        let up = lines[..i].iter().rev().take_while(|l| attr(l));
        let down = lines[i + 1..].iter().take_while(|l| attr(l));
        let mut around = up.chain(down);
        !around.any(|l| l.starts_with("#[ignore") || l.strip_prefix("#[cfg(").and_then(|r| r.strip_suffix(")]")).is_some_and(|p| cfg_holds(p, os, feats) == Some(false)))
    })
}

/// A `cfg` predicate on a box of `os` with the crate's default features `feats`: None when it
/// depends on more than these.
fn cfg_holds(pred: &str, os: Os, feats: &BTreeSet<String>) -> Option<bool> {
    let pred = pred.trim();
    let args = |inner: &str| -> Vec<Option<bool>> {
        let (mut depth, mut start, mut out) = (0i32, 0usize, Vec::new());
        for (i, c) in inner.char_indices() {
            match c {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    out.push(cfg_holds(&inner[start..i], os, feats));
                    start = i + 1;
                }
                _ => {}
            }
        }
        if !inner[start..].trim().is_empty() {
            out.push(cfg_holds(&inner[start..], os, feats));
        }
        out
    };
    let call = |name: &str| pred.strip_prefix(name).map(str::trim_start).and_then(|r| r.strip_prefix('(')).and_then(|r| r.strip_suffix(')'));
    if let Some(inner) = call("not") {
        return cfg_holds(inner, os, feats).map(|b| !b);
    }
    if let Some(inner) = call("any") {
        let v = args(inner);
        return if v.contains(&Some(true)) { Some(true) } else if v.iter().all(|b| *b == Some(false)) { Some(false) } else { None };
    }
    if let Some(inner) = call("all") {
        let v = args(inner);
        return if v.contains(&Some(false)) { Some(false) } else if v.iter().all(|b| *b == Some(true)) { Some(true) } else { None };
    }
    let (key, value) = match pred.split_once('=') {
        Some((k, v)) => (k.trim(), Some(v.trim().trim_matches('"'))),
        None => (pred, None),
    };
    // (a test shard runs with the crate's default features: a test behind another never runs)
    if let ("feature", Some(f)) = (key, value) {
        return Some(feats.contains(f));
    }
    if os == Os::Any {
        return None;
    }
    let unix = os != Os::Windows;
    match (key, value) {
        ("unix", None) => Some(unix),
        ("windows", None) => Some(!unix),
        ("test", None) => Some(true),
        ("target_os", Some(v)) => Some(v == os.as_str()),
        ("target_family", Some(v)) => Some(v == if unix { "unix" } else { "windows" }),
        ("target_vendor", Some(v)) => Some(v == match os {
            Os::Macos => "apple",
            Os::Windows => "pc",
            _ => "unknown",
        }),
        // (the fleet's Linux and Windows boxes are x86_64, its Macs arm64)
        ("target_arch", Some(v)) => Some(v == if os == Os::Macos { "aarch64" } else { "x86_64" }),
        _ => None,
    }
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
    if judge == Judge::Report && exit == 3 {
        return (true, log.lines().rev().find(|l| l.contains("pass rate")).unwrap_or(step).to_string());
    }
    if exit != 0 {
        return (false, format!("{step}: exit {exit}"));
    }
    match judge {
        Judge::Exit => (true, String::new()),
        Judge::Tests => match tests_passed(log) {
            0 => (false, format!("{step}: ran no tests")),
            _ => (true, String::new()),
        },
        Judge::Report => (true, log.lines().rev().find(|l| l.contains("pass rate")).unwrap_or("").to_string()),
        Judge::Cargo => {
            let w = cargo_warnings(log);
            if w.is_empty() {
                (true, String::new())
            } else {
                (false, format!("{step}: {} warning{}", w.len(), if w.len() == 1 { "" } else { "s" }))
            }
        }
        Judge::Check => match (check_errors(log), cargo_warnings(log).len()) {
            (Some(n), _) if n > 0 => (false, format!("{step}: {n} errors")),
            (_, 0) => (true, String::new()),
            (_, w) => (false, format!("{step}: {w} warning{}", if w == 1 { "" } else { "s" })),
        },
    }
}

/// The tests a test binary's run passed, from its `test result: ok. N passed; ...` lines.
pub fn tests_passed(log: &str) -> u64 {
    log.lines().filter_map(|l| l.trim().strip_prefix("test result: ")).filter_map(|r| r.split_once(". ")?.1.split_whitespace().next()?.parse::<u64>().ok()).sum()
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
    /// it has a GPU (test shards need one: apps' GPU tests run on a real one)
    pub gpu: bool,
    /// it takes a lease on the separate CI tree
    pub ci: bool,
}

impl BoxInfo {
    fn needs_gui(s: &Shard) -> bool {
        s.gui || s.id.starts_with("smoke:")
    }

    pub fn fits(&self, s: &Shard) -> bool {
        (s.kind != Kind::Ci || self.ci) && (s.os == Os::Any || s.os == self.os) && (!Self::needs_gui(s) || self.gui) && (s.os != Os::Any || self.os != Os::Windows) && (!s.gpu || self.gpu)
    }

    fn needs(&self, s: &Shard) -> String {
        let mut n = format!("os={},arch={}", self.os.as_str(), self.arch);
        if Self::needs_gui(s) {
            n.push_str(",gui=1");
        }
        if s.gpu {
            // (any GPU: the name contains "")
            n.push_str(",gpu=");
        }
        n
    }
}

/// A CI box: its lease.conf opts in (`slots.ci`).
fn takes_ci(n: &crate::lease::sched::NodeView) -> bool {
    n.caps.as_ref().is_some_and(|c| crate::lease::sched::slots(c, crate::lease::Kind::Ci) > 0)
}

/// The allowed CI boxes that are up, dev on, with capabilities.
pub fn boxes(ctx: &Ctx) -> Vec<BoxInfo> {
    ctx.leases
        .nodes()
        .into_iter()
        .filter(|n| n.up && n.dev && takes_ci(n))
        .filter_map(|n| {
            let c = n.caps?;
            if c.role().users() {
                return None;
            }
            let os = Os::of(&c.os);
            // a box on another toolchain than the fleet's takes no shard
            if super::fleet_rustc(ctx, &c.os).is_some_and(|f| !c.rustc.is_empty() && c.rustc != f) {
                return None;
            }
            // Reject older nodes' generic Wayland capability too: Linux boots belong to the
            // real WM session, even while the fleet is updating its capability probe.
            let gui = c.display.as_deref().is_some_and(|d| os != Os::Linux || d.strip_prefix("wm:makepad-wm-").is_some_and(|n| n.parse::<u32>().is_ok()));
            (os != Os::Any).then(|| BoxInfo { node: n.name.clone(), os, arch: c.arch.clone(), gui, gpu: !c.gpus.is_empty(), ci: crate::lease::sched::slots(&c, Kind::Ci) > 0 })
        })
        .collect()
}

struct Pool {
    proofs_collected: bool,
    shards: Vec<Shard>,
    state: Vec<CiShard>,
    /// a shard's lease is being asked for on that box
    reserved: Vec<Option<String>>,
    stopped: bool,
    /// the lease priority of the run's shards
    prio: i32,
    /// boxes the run may hold at once
    max_active: usize,
    /// shards given the overdue verdict while their step still runs ([`OVERDUE_TIMES`])
    overdue: Vec<bool>,
    /// the run's id: a step that ends after its run did updates that run's record only
    run_id: String,
    sweep: bool,
}

impl Pool {
    fn finished(&self, i: usize) -> bool {
        matches!(self.state[i].state.as_str(), "ok" | "fail" | "incomplete")
    }

    fn all_done(&self) -> bool {
        (0..self.state.len()).all(|i| self.finished(i))
    }

    /// Whether shard `i` may go to this box: it fits, was not lost there too often, the box is still
    /// in the run, and after a failure it is another box of that OS.
    fn can(&self, b: &BoxInfo, i: usize) -> bool {
        let s = &self.state[i];
        b.fits(&self.shards[i]) && lost_on(s, &b.node) < LOST_TRIES && !self.out(&b.node) && (s.failed_on.is_empty() || (s.failed_on != b.node && s.failed_os == b.os.as_str()))
    }

    /// The shards of this run that failed on `node` and passed on another box.
    fn failed_only_on(&self, node: &str) -> usize {
        self.state.iter().filter(|s| s.state == "ok" && s.failed_on == node && s.kept.is_empty()).count()
    }

    /// Whether `node` is out of the run.
    fn out(&self, node: &str) -> bool {
        self.failed_only_on(node) >= BOX_OUT
    }

    /// Shard `i` failed on `node`. With no failure before and another box of that OS in the run it
    /// waits for that box (true); otherwise the failure stands.
    fn second_box(&mut self, i: usize, node: &str, boxes: &[BoxInfo], why: &str, errors: Vec<String>) -> bool {
        let Some(os) = boxes.iter().find(|b| b.node == node).map(|b| b.os.as_str()) else { return false };
        if !self.state[i].failed_on.is_empty() {
            return false;
        }
        let s = &mut self.state[i];
        s.failed_on = node.to_string();
        s.failed_os = os.to_string();
        if !boxes.iter().any(|b| self.can(b, i)) {
            let s = &mut self.state[i];
            s.failed_on.clear();
            s.failed_os.clear();
            return false;
        }
        let s = &mut self.state[i];
        s.state = "wait".into();
        s.failed_why = why.to_string();
        s.errors = errors;
        s.summary = format!("failed on {node}; asks a second box: {why}");
        true
    }

    /// The longest pending shard this box can run (none while the run holds its most boxes).
    fn next_for(&self, b: &BoxInfo) -> Option<usize> {
        // the boxes the run holds: a request queued on a busy box holds none (counting those left a
        // free box idle while three asks waited elsewhere, 11-01)
        let active = (0..self.shards.len()).filter(|&i| self.state[i].state == "run").count();
        if active >= self.max_active {
            return None;
        }
        (0..self.shards.len())
            .filter(|&i| self.state[i].state == "wait" && self.reserved[i].is_none() && self.can(b, i))
            .max_by(|&a, &c| self.shards[a].est.partial_cmp(&self.shards[c].est).unwrap_or(std::cmp::Ordering::Equal))
    }
}

fn lost_on(s: &CiShard, node: &str) -> u32 {
    s.lost.iter().find(|(n, _)| n == node).map_or(0, |(_, k)| *k)
}

/// The attempt in progress on `node` ends now.
fn close_span(s: &mut CiShard, node: &str) {
    // (a run a state from before these fields left on a box has no times: nothing to count)
    if s.granted_ms > 0 {
        let asked_ms = if s.asked_ms > 0 { s.asked_ms } else { s.granted_ms };
        s.spans.push(CiSpan { node: node.to_string(), asked_ms, granted_ms: s.granted_ms, ended_ms: wall_ms() });
    }
    s.asked_ms = 0;
    s.granted_ms = 0;
}

/// What becomes of a shard no box can take, with nothing else left to run: a state and a summary,
/// or None while it waits. `here`: the boxes of its kind in the run now. `waited`: how long nothing
/// else has been running, `heard`: whether this master has heard its nodes, `gone`: a box of the
/// fleet is away now (it may be the one that can).
fn no_box(shard: &Shard, s: &CiShard, here: &[&BoxInfo], out: &[String], waited: Duration, heard: bool, gone: bool) -> Option<(&'static str, String)> {
    let kind = format!("{}{}", shard.os.as_str(), if shard.gui { " with a display" } else { "" });
    if !heard {
        return None;
    }
    // its second box went away: the failure it has stands
    if !s.failed_on.is_empty() {
        return Some(("fail", s.failed_why.clone()));
    }
    if !here.is_empty() {
        let names = here.iter().map(|b| b.node.as_str()).collect::<Vec<_>>().join(", ");
        if here.iter().all(|b| out.contains(&b.node)) {
            return Some(("incomplete", format!("every box of {kind} is out of this run ({names}): their failures passed elsewhere")));
        }
        // every box here said it cannot run it: none ever will
        if s.summary.starts_with("unfit:") {
            if gone && waited < NO_BOX_WAIT {
                return None;
            }
            return Some(("fail", format!("no box can run it ({names}): {}", s.summary)));
        }
        let lost = s.lost.iter().map(|(n, _)| n.as_str()).collect::<Vec<_>>().join(", ");
        return Some(("incomplete", format!("lost on {lost} ({})", s.summary)));
    }
    (waited >= NO_BOX_WAIT).then(|| ("incomplete", format!("no box of {kind} came in {} min", NO_BOX_WAIT.as_secs() / 60)))
}

/// A run's shard states for the state document.
fn publish(ci: &mut Ci, pool: &Pool) {
    let run = if ci.slot(pool.sweep).as_ref().is_some_and(|r| r.id == pool.run_id) {
        ci.slot_mut(pool.sweep).as_mut()
    } else {
        ci.runs.iter_mut().rev().find(|r| r.id == pool.run_id && r.sweep == pool.sweep)
    };
    if let Some(r) = run {
        r.shards = pool.state.clone();
    }
}

fn publish_changed(ctx: &Ctx, pool: &Pool) {
    let changed = ctx.store.read(|d| {
        d.ci.slot(pool.sweep).as_ref().filter(|r| r.id == pool.run_id)
            .or_else(|| d.ci.runs.iter().rev().find(|r| r.id == pool.run_id && r.sweep == pool.sweep))
            .is_some_and(|r| r.shards != pool.state)
    });
    if changed {
        if let Err(e) = ctx.store.update(|d| publish(&mut d.ci, pool)) { eprintln!("devhub CI state: {e}"); }
    }
}

/// The run's critical path (the box that finished last, its shards in order), per-box load, and
/// where the wall went, all from the attempts the shards keep (a master restart loses none of them).
pub fn critical(state: &[CiShard], started_ms: u64, ended_ms: u64) -> (String, Vec<String>, String) {
    let secs = |ms: u64| ms as f64 / 1000.0;
    let mut per: BTreeMap<&str, Vec<(&str, &CiSpan)>> = BTreeMap::new();
    for s in state {
        for x in &s.spans {
            per.entry(x.node.as_str()).or_default().push((s.id.as_str(), x));
        }
    }
    let mut on_box: Vec<&CiSpan> = Vec::new();
    let mut queued = 0;
    let mut boxes = Vec::new();
    let mut last: Option<(&str, u64)> = None;
    for (node, v) in per.iter_mut() {
        v.sort_by_key(|(_, x)| x.granted_ms);
        let held: Vec<&CiSpan> = v.iter().map(|(_, x)| *x).filter(|x| x.granted_ms > 0).collect();
        let busy: u64 = held.iter().map(|x| x.ended_ms.saturating_sub(x.granted_ms)).sum();
        let waited: u64 = v.iter().map(|(_, x)| if x.granted_ms > 0 { x.granted_ms } else { x.ended_ms }.saturating_sub(x.asked_ms)).sum();
        queued += waited;
        boxes.push(format!("{node} {:.0} s busy, {} shards, {:.0} s queued", secs(busy), held.len(), secs(waited)));
        if let Some(end) = held.iter().map(|x| x.ended_ms).max() {
            if last.is_none_or(|(_, e)| end > e) {
                last = Some((node, end));
            }
        }
        on_box.extend(held);
    }
    let crit = last.map_or(String::new(), |(node, end)| {
        let shards = per[node].iter().filter(|(_, x)| x.granted_ms > 0).map(|(id, x)| format!("{id} {:.0} s", secs(x.ended_ms.saturating_sub(x.granted_ms)))).collect::<Vec<_>>();
        format!("{node} ends at {:.0} s: {}", secs(end.saturating_sub(started_ms)), shards.join(", "))
    });
    // the wall with no shard on any box
    on_box.sort_by_key(|x| x.granted_ms);
    let (mut covered, mut upto) = (0, started_ms);
    for x in &on_box {
        let (from, to) = (x.granted_ms.max(upto), x.ended_ms.max(upto));
        covered += to - from;
        upto = to;
    }
    let wall = ended_ms.saturating_sub(started_ms);
    let busy: u64 = on_box.iter().map(|x| x.ended_ms.saturating_sub(x.granted_ms)).sum();
    let fleet = format!("wall {:.0} s: {:.0} s on {} boxes ({:.1} at once), {:.0} s queued for leases, {:.0} s with nothing on any box", secs(wall), secs(busy), per.len(), if covered > 0 { busy as f64 / covered as f64 } else { 0.0 }, secs(queued), secs(wall.saturating_sub(covered)));
    (crit, boxes, fleet)
}

const ENDED_WITH_MASTER: &str = "ended with the master";

/// What every shard reads, whatever its crates: a change here is the JIT's too.
const SHARED_PATHS: [&str; 5] = ["Cargo.lock", "Cargo.toml", "tools/ci", "tools/ci_leg", "tools/run"];
/// The crate whose closure the JIT is.
const JIT_DIR: &str = "tools/run";
/// Not read by any shard through the JIT.
const NOT_JIT: [&str; 2] = ["libs/devhub", "apps/devhub"];

/// Crates compiled only into the JIT binary: closure(JIT_DIR) minus the union of the
/// closures of workspace members outside it that do not embed the JIT. Shared dependencies still test under the JIT.
pub fn jit_own_dirs(tree: &Tree) -> BTreeSet<String> {
    let read = |p: &str| tree.read(p);
    let jit = super::roll::crate_dirs_from(JIT_DIR, &read);
    let mut shared = BTreeSet::new();
    for k in workspace_crates(tree) {
        // Closures are transitive: a member already reached adds nothing.
        if !jit.contains(&k.dir) && !shared.contains(&k.dir) {
            let closure = super::roll::crate_dirs_from(&k.dir, &read);
            if closure.contains(JIT_DIR) { continue; }
            shared.extend(closure);
        }
    }
    jit.difference(&shared).cloned().collect()
}

/// Which shards' inputs a range of landings left alone.
pub(crate) struct Inputs {
    changed: Vec<String>,
    /// a changed path the JIT reads (its own crates, or what every shard reads)
    pub(crate) jit_changed: bool,
}

fn under(path: &str, dir: &str) -> bool {
    path == dir || (path.starts_with(dir) && path.as_bytes().get(dir.len()) == Some(&b'/'))
}

impl Inputs {
    /// None when the range cannot be read.
    pub(crate) fn between(repo: &Path, from: &Pair, to: &Pair) -> Option<Inputs> {
        let mut changed: Vec<String> = super::git(repo, &["diff", "--name-only", &from.work, &to.work]).ok()?.lines().map(str::to_string).collect();
        if from.main != to.main {
            let args = if from.main.is_empty() { vec!["ls-tree", "-r", "--name-only", &to.main] }
                else if to.main.is_empty() { vec!["ls-tree", "-r", "--name-only", &from.main] }
                else { vec!["diff", "--name-only", &from.main, &to.main] };
            changed.extend(super::git(&repo.join(COMMERCIAL), &args).ok()?.lines().map(|p| format!("{COMMERCIAL}/{p}")));
        }
        changed.retain(|p| !proof_path(p));
        changed.sort();
        changed.dedup();
        let jit = super::roll::crate_dirs(repo, JIT_DIR);
        let read_by_jit = |p: &String| jit.iter().any(|d| under(p, d)) && !NOT_JIT.iter().any(|d| under(p, d));
        let jit_changed = changed.iter().any(|p| read_by_jit(p) || SHARED_PATHS.iter().any(|d| under(p, d)));
        Some(Inputs { changed, jit_changed })
    }
}

/// Both the executable plan and `ci plan` use the same pair-wide Mac decision. Full verification
/// is the weekly escape hatch; ordinary sweeps retain their rotations only for relevant changes.
fn arm_decision(ctx: &Ctx, pair: &Pair) -> super::arm::Decision {
    let (base, full) = ctx.store.read(|d| (
        d.verify.verified.iter().rev().find(|t| t.pair != *pair).map(|t| t.pair.clone()),
        d.verify.run.as_ref().is_some_and(|r| r.pair == *pair && r.diff == "all"),
    ));
    super::arm::classify(&ctx.repo, base.as_ref(), pair, full)
}

/// Leave classified Mac shards as explicit passes, outside the runnable candidates. Rotating
/// smoke scripts still run on x64; a script pinned to macOS cannot be moved to Linux.
fn arm_plan(tree: &Tree, shards: &mut Vec<Shard>, arm: &super::arm::Decision) -> Vec<Shard> {
    if arm.required { return Vec::new(); }
    let (_, scripts) = discover(tree);
    let mut skipped = Vec::new();
    shards.retain_mut(|s| {
        if s.os == Os::Any { s.os = Os::Linux; }
        if s.os != Os::Macos { return true; }
        if let Some(dir) = s.id.strip_prefix("smoke:").filter(|d| !d.starts_with("macos:")) {
            let pinned = scripts.iter().find(|sc| sc.dir == dir).and_then(|sc| tree.read(&sc.path)).as_deref().and_then(pinned_os);
            if pinned != Some(Os::Macos) { s.os = Os::Linux; return true; }
        }
        skipped.push(s.clone());
        false
    });
    skipped
}

/// Resume all existing runs in their own slots before accepting requests.
pub fn start(ctx: &Arc<Ctx>) {
    let runs = ctx.store.read(|d| d.ci.run.iter().chain(d.ci.sweep_run.iter()).cloned().collect::<Vec<_>>());
    let mut slots = Vec::new();
    for run in runs {
        let (held, slot) = std::sync::mpsc::channel();
        let c = if run.checkout.is_empty() { ctx.clone() } else { calling_ctx(ctx, run.checkout.clone().into()) };
        let _ = std::thread::Builder::new().name("devhub-ci".into()).spawn(move || {
            let one = run_slot(run.sweep).lock().unwrap_or_else(|e| e.into_inner());
            // a run stopped before the restart only finishes what runs (nothing new starts)
            run_stop(run.sweep).store(run.stopping, std::sync::atomic::Ordering::SeqCst);
            let _ = held.send(());
            let _ = gate_held(one, &c, &run.pair, run.dry, run.sweep, !run.checkout.is_empty());
        });
        slots.push(slot);
    }
    for slot in slots { let _ = slot.recv(); }
    RESUMED.store(true, std::sync::atomic::Ordering::Release);
}

/// Set once [`start`] has the runs of a previous master in their slots: a bucket run waits for it,
/// or it would take the slot of the run it adopts.
static RESUMED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The rolling runs: neither the sweep nor a calling checkout's run.
fn rolling(r: &CiRun) -> bool {
    !r.sweep && !r.dry && r.checkout.is_empty()
}

/// The newest rolling run's start and pair: the CI bucket's last run when its ledger has none
/// newer (the bucket that asked for it ended with a master; the run went on).
pub fn last_rolling(d: &crate::state::Doc) -> Option<(u64, Pair)> {
    d.ci.run.iter().chain(&d.ci.runs).filter(|r| rolling(r)).max_by_key(|r| r.started_ms).map(|r| (r.started_ms, r.pair.clone()))
}

/// A finished rolling run of `tip` with a verdict that started at or after `since`.
fn judged<'a>(ci: &'a Ci, tip: &Pair, since: u64) -> Option<&'a CiRun> {
    ci.runs.iter().rev().find(|r| rolling(r) && r.pair == *tip && r.started_ms >= since && matches!(r.verdict.as_str(), "ok" | "fail"))
}

/// One full CI run on the bucket's tip, waiting for any manual or resumed run first. A bucket run
/// a previous master started (adopted from its `.log.run`) answers from that run's verdict: the
/// run itself outlived the master and went on in its slot.
pub fn ci(repo: PathBuf) -> crate::bucket::Run {
    Arc::new(move |base, tip, log| {
        let Some(ctx) = super::ctx() else {
            return crate::bucket::Outcome { ok: false, failing: vec!["CI: master not started".into()], log: String::new(), summary: "CI incomplete: master not started".into(), lost: false };
        };
        let ctx = calling_ctx(ctx, repo.clone());
        while !RESUMED.load(std::sync::atomic::Ordering::Acquire) { std::thread::sleep(Duration::from_millis(200)); }
        // the bucket's start, its log's name (none: a run asked outside a bucket)
        let since = crate::bucket::log_started(log);
        if let (Some(since), Some(base)) = (since, base) {
            if let Err(e) = crate::bucket::save_run(log, std::process::id(), since, base, tip) {
                ctx.store.event("ci", Sev::Warn, format!("ci bucket {}: noting its run: {e}", tip.short()), None);
            }
        }
        // The shared slot stays locked through the verdict and proof publication.
        let (verdict, legs) = {
            let one = run_slot(false).lock().unwrap_or_else(|e| e.into_inner());
            match since.and_then(|since| ctx.store.read(|d| judged(&d.ci, tip, since).map(|r| (if r.verdict == "ok" { Verdict::Ok } else { Verdict::Fail }, r.shards.iter().map(CiShard::leg).collect::<Vec<_>>())))) {
                Some(judged) => judged,
                None => {
                    run_stop(false).store(false, std::sync::atomic::Ordering::SeqCst);
                    gate_held(one, &ctx, tip, false, false, false)
                }
            }
        };
        let ok = legs.iter().filter(|l| l.state == "ok").count();
        let fail = legs.iter().filter(|l| l.state == "fail").count();
        let word = match verdict { Verdict::Ok => "ok", Verdict::Fail => "fail", Verdict::Incomplete => "incomplete" };
        let mut summary = format!("{} shards: {ok} ok, {fail} fail, {} incomplete; {word}", legs.len(), legs.len() - ok - fail);
        let mut failing: Vec<String> = legs.iter().filter(|l| l.state != "ok").map(|l| l.name.clone()).collect();
        if verdict != Verdict::Ok && failing.is_empty() {
            failing.push(format!("CI {word}: see ci events"));
        }
        let mut text = format!("CI {}: {summary}\n", tip.short());
        for leg in &legs {
            text.push_str(&format!("{} {}: {} ({})\n", leg.state, leg.name, leg.summary, leg.log));
            for error in &leg.errors { text.push_str(&format!("{error}\n")); }
        }
        if let Err(e) = std::fs::write(log, text) {
            summary.push_str(&format!("; log: {e}"));
        }
        crate::bucket::Outcome { ok: verdict == Verdict::Ok, failing, log: log.display().to_string(), summary, lost: false }
    })
}

fn gate_for(ctx: &Arc<Ctx>, pair: &Pair, dry: bool, sweep: bool, calling: bool) -> (Verdict, Vec<crate::state::Leg>) {
    let one = run_slot(sweep).lock().unwrap_or_else(|e| e.into_inner());
    run_stop(sweep).store(false, std::sync::atomic::Ordering::SeqCst);
    gate_held(one, ctx, pair, dry, sweep, calling)
}

/// Keep a matching run in its slot while planning; archive only that slot's superseded run.
fn resume_run(ci: &mut Ci, pair: &Pair, dry: bool, sweep: bool, lock_digest: &str, checkout: &str) -> Option<CiRun> {
    if let Some(r) = ci.slot(sweep).as_ref().filter(|r| r.lock_digest == lock_digest && r.pair == *pair && r.dry == dry && r.sweep == sweep && Path::new(&r.checkout) == Path::new(checkout)) {
        return Some(r.clone());
    }
    if let Some(mut r) = ci.slot_mut(sweep).take() {
        r.verdict = "incomplete".into();
        r.ended_ms = wall_ms();
        r.critical = ENDED_WITH_MASTER.into();
        ci.runs.push(r);
    }
    None
}

/// Prune finished runs only; the other slot and overdue workers still need their logs and lock.
fn prune_runs(ctx: &Ctx) {
    let old = ctx.store.read(|d| d.ci.runs.iter().rev().skip(40).filter(|r| !r.shards.iter().any(|s| s.state == "fail" && s.summary.contains("overdue:"))).map(|r| r.id.clone()).collect::<Vec<_>>());
    for id in old {
        let _ = std::fs::remove_dir_all(ctx.logs("ci").join(id));
    }
}

/// Runs the shards of `pair` with its slot held through proof publication.
fn gate_held(_one: std::sync::MutexGuard<'static, ()>, ctx: &Arc<Ctx>, pair: &Pair, dry: bool, sweep: bool, calling: bool) -> (Verdict, Vec<crate::state::Leg>) {
    let checkout = if calling { ctx.repo.to_string_lossy().into_owned() } else { String::new() };
    let prior_lock = ctx.store.read(|d| d.ci.slot(sweep).as_ref().filter(|r| r.pair == *pair && r.dry == dry && r.sweep == sweep && Path::new(&r.checkout) == Path::new(&checkout)).map(|r| ctx.logs("ci").join(&r.id).join("Cargo.lock")));
    let lock = prior_lock.and_then(|p| std::fs::read_to_string(p).ok()).or_else(|| std::fs::read_to_string(ctx.repo.join("Cargo.lock")).ok());
    let Some(lock) = lock else {
        ctx.store.event("ci", Sev::Err, "Cargo.lock is missing; no CI proof can be computed".into(), None);
        return (Verdict::Incomplete, Vec::new());
    };
    let lock_digest = hex_digest(lock.as_bytes());
    let going = ctx.store.read(|d| d.ci.slot(sweep).as_ref().filter(|r| r.lock_digest == lock_digest && r.pair == *pair && r.dry == dry && r.sweep == sweep && Path::new(&r.checkout) == Path::new(&checkout)).cloned());
    if going.is_none() {
        if let Err(e) = ctx.store.update(|d| resume_run(&mut d.ci, pair, dry, sweep, &lock_digest, &checkout)) {
            ctx.store.event("ci", Sev::Err, e, None);
            return (Verdict::Incomplete, Vec::new());
        }
    }
    let rotation = match &going {
        Some(r) => r.rotation,
        None => match ctx.store.update(|d| { d.ci.rotation += 1; d.ci.rotation }) {
            Ok(rotation) => rotation,
            Err(e) => { ctx.store.event("ci", Sev::Err, e, None); return (Verdict::Incomplete, Vec::new()); }
        },
    };
    let id = going.as_ref().map_or_else(|| format!("{}-{rotation}", stamp(wall_ms())), |r| r.id.clone());
    let dir = ctx.logs("ci").join(&id);
    let _ = std::fs::create_dir_all(&dir);
    prune_runs(ctx);
    let tree = match Tree::at(&ctx.repo, pair) {
        Ok(t) => t,
        Err(e) => {
            ctx.store.event("ci", Sev::Warn, format!("ci {}: the tree: {e}", pair.short()), None);
            return (Verdict::Incomplete, Vec::new());
        }
    };
    // a run that goes on plans with the timings it began with: its batches stay as they were
    // (planned again with newer timings, they changed and what had passed started over)
    let timings: Vec<(String, f64)> = ctx.store.read(|d| going.as_ref().map(|r| r.plan_timings.clone()).unwrap_or_else(|| d.ci.timings.clone()));
    let timing_of = |id: &str| timings.iter().find(|(k, _)| k == id).map(|(_, s)| *s);
    let arm = going.as_ref().and_then(|_| super::arm::Decision::read(&dir.join("arm64.log"))).unwrap_or_else(|| arm_decision(ctx, pair));
    let mut shards = if sweep { sweep_plan(&tree, &timing_of, rotation) } else { plan(&tree, &timing_of, rotation) };
    if !sweep && arm.full {
        shards.extend(sweep_plan(&tree, &timing_of, rotation).into_iter().filter(|s| s.id.starts_with("mjit:")));
    }
    if sweep || calling {
        // Whole crate proofs need every test target, independently of sweep rotation.
        shards.retain(|s| !s.id.starts_with("test:") && !s.id.starts_with("wtest:"));
        shards.extend(test_shards(&tree, &timing_of));
    }
    let units: Vec<Unit> = shards.iter().flat_map(units_of).collect();
    let digests = match Digests::with_lock(&tree, &units, lock) {
        Ok(d) => d,
        Err(e) => {
            ctx.store.event("ci", Sev::Err, format!("ci {}: {e}", pair.short()), None);
            return (Verdict::Incomplete, Vec::new());
        }
    };
    if !digests.lock.is_empty() {
        if hex_digest(digests.lock.as_bytes()) != lock_digest {
            ctx.store.event("ci", Sev::Err, "Cargo.lock changed while planning CI".into(), None);
            return (Verdict::Incomplete, Vec::new());
        }
        if let Err(e) = std::fs::write(dir.join("Cargo.lock"), &digests.lock) {
            ctx.store.event("ci", Sev::Err, format!("CI lock snapshot: {e}"), None);
            return (Verdict::Incomplete, Vec::new());
        }
    }
    let mut why_lines = Vec::new();
    let classified = arm_plan(&tree, &mut shards, &arm);
    ctx.store.event("ci", Sev::Info, format!("ci {}: {}", pair.short(), arm.summary), None);
    let _ = std::fs::write(dir.join("arm64.log"), format!("{}\n", arm.summary));
    if !sweep {
        let bs = boxes(ctx);
        let was: BTreeSet<String> = going.as_ref().map(|r| r.shards.iter().map(|s| s.id.clone()).collect()).unwrap_or_default();
        let (apps, others): (Vec<Shard>, Vec<Shard>) = shards.into_iter().partition(|s| s.id.starts_with("smoke:"));
        let (macs, apps): (Vec<Shard>, Vec<Shard>) = apps.into_iter().partition(|s| arm.full && s.os == Os::Macos);
        match select(&ctx.repo, pair, &tree, &digests, apps, &|os| bs.iter().filter(|b| b.os == os).count(), &was) {
            Ok((picked, lines)) => { shards = picked; why_lines = lines; }
            Err(e) => {
                ctx.store.event("ci", Sev::Err, e, None);
                return (Verdict::Incomplete, Vec::new());
            }
        }
        shards.extend(others);
        shards.extend(macs);
    }
    shards.extend(classified.iter().cloned());
    for sh in &shards {
        if classified.iter().any(|s| s.id == sh.id) {
            why_lines.push((sh.id.clone(), arm.summary.clone()));
            continue;
        }
        // The weekly Mac sweep must execute even when its inputs have a stored proof.
        if sh.os == Os::Macos && arm.full { continue; }
        if why_lines.iter().any(|(id, _)| id == &sh.id) { continue; }
        match kept_shard(&tree, &digests, sh) {
            Ok(Some(why)) => why_lines.push((sh.id.clone(), why)),
            Ok(None) => {}
            Err(e) => {
                ctx.store.event("ci", Sev::Err, e, None);
                return (Verdict::Incomplete, Vec::new());
            }
        }
    }
    if sweep || calling {
        shards = batch_tests(shards, &mut why_lines, &timing_of);
    }
    let state: Vec<CiShard> = shards
        .iter()
        .map(|s| CiShard { id: s.id.clone(), state: "wait".into(), log: dir.join(format!("{}.log", safe(&s.id))).to_string_lossy().into_owned(), sig: s.sig(), ..Default::default() })
        .collect();
    let mut state = state;
    // why each app runs, or where it passed with these inputs (then it does not run)
    for (id, line) in &why_lines {
        if let Some(st) = state.iter_mut().find(|st| st.id == *id) {
            st.summary = line.clone();
            if line.starts_with("kept:") {
                st.state = "ok".into();
                st.kept = line.clone();
            }
            if line.starts_with("arm64: skipped (") {
                st.state = "ok".into();
                st.kept = line.clone(); // classification is not a new platform proof
                let _ = std::fs::write(&st.log, format!("{line}\n"));
            }
        }
    }
    // where the going run was: its results and the steps on their boxes (a plan that changed under
    // a shard starts it over); its shards' leases come back with the master
    let mut back: Vec<usize> = Vec::new();
    let mut stray: Vec<String> = Vec::new();
    if let Some(r) = &going {
        for (i, s) in state.iter_mut().enumerate() {
            if s.kept.starts_with("arm64: skipped (") { continue; }
            match r.shards.iter().find(|p| p.id == s.id && p.sig == s.sig) {
                Some(p) if matches!(p.state.as_str(), "ok" | "fail") => *s = p.clone(),
                // (also with no run noted: the node may have started the step's run as the
                // master died, and it is followed then, not started twice; see `run_steps`)
                // (its held lease is the proof it is on a CI box: a restarted master knows the
                // boxes' lease.conf only once they report in)
                Some(p) if p.state == "run" && super::leg::lease_held(ctx, &p.lease) => {
                    *s = p.clone();
                    back.push(i);
                }
                // not on a box now: it waits again, and keeps what it knows (a first failure, the
                // boxes it was lost on, its time so far)
                Some(p) => {
                    let log = std::mem::take(&mut s.log);
                    *s = p.clone();
                    s.log = log;
                    if s.state == "run" {
                        let node = s.node.clone();
                        close_span(s, &node);
                    }
                    s.state = "wait".into();
                    s.lease.clear();
                    s.step = 0;
                    s.run_n = 0;
                    s.failed.clear();
                }
                None => {}
            }
        }
        stray = r.shards.iter().filter(|p| super::leg::lease_held(ctx, &p.lease) && !back.iter().any(|&i| state[i].lease == p.lease)).map(|p| p.lease.clone()).collect();
    }
    let kept = state.iter().filter(|s| !s.kept.is_empty()).count();
    for st in state.iter_mut().filter(|s| s.state == "wait" && s.summary.is_empty()) {
        st.summary = "runs: no proof".into();
    }
    let n = shards.len();
    let started = going.as_ref().map_or_else(wall_ms, |r| r.started_ms);
    ctx.store.update(|d| *d.ci.slot_mut(sweep) = Some(CiRun { lock_digest, checkout, id: id.clone(), pair: pair.clone(), dry, started_ms: started, verdict: "running".into(), shards: state.clone(), rotation, plan_timings: timings.clone(), sweep, stopping: going.as_ref().is_some_and(|r| r.stopping), ..Default::default() })).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
    for l in stray {
        let _ = crate::cli::ask(&makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-ci"), None, "release", &l);
    }
    let word = if going.is_some() { "goes on" } else { "started" };
    ctx.store.event("ci", Sev::Info, format!("ci {} {word}: {n} shards, {kept} kept, {} on their boxes (run {id})", pair.short(), back.len()), None);
    let max_active = boxes(ctx).len().saturating_sub(if sweep { 2 } else { 0 }).max(1);
    let pool = Arc::new((Mutex::new(Pool { proofs_collected: false, reserved: vec![None; n], shards, state, stopped: false, prio: PRIO, max_active, overdue: vec![false; n], run_id: id.clone(), sweep }), Condvar::new()));
    let master_t0 = Instant::now();
    let mut nothing_left: Option<Instant> = None;
    let mut last_ran = Instant::now();
    // the alert of a run that goes nowhere: no shard ended for this long, whatever still "runs"
    let (mut ended, mut last_end) = (0usize, Instant::now());
    // passes are committed as proofs while the run goes (a failure elsewhere, a stop or a master
    // restart never throws them away); the run's end commits the rest
    let (mut published, mut last_batch): (BTreeSet<(Unit, Os)>, Instant) = (BTreeSet::new(), Instant::now());
    let mut workers = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    // the steps a previous master left on their boxes go on
    for i in back {
        let (ctx, pool) = (ctx.clone(), pool.clone());
        {
            let mut g = pool.0.lock().unwrap();
            g.reserved[i] = Some(g.state[i].node.clone());
        }
        workers.push(std::thread::Builder::new().name("devhub-ci-back".into()).spawn(move || {
            let (lease, node) = {
                let g = pool.0.lock().unwrap();
                (g.state[i].lease.clone(), g.state[i].node.clone())
            };
            let _ = run_shard(&ctx, &pool, i, Held::attach(&lease, &node));
        }).ok());
    }
    // boxes join while the run goes (a box that comes up or gets free takes shards too)
    loop {
        {
            let (p, cv) = &*pool;
            let mut g = p.lock().unwrap();
            if run_stop(sweep).load(std::sync::atomic::Ordering::SeqCst) {
                g.stopped = true;
            }
            if g.all_done() || g.stopped {
                cv.notify_all();
                break;
            }
            // a shard no box can take waits while anything else runs, and then for a box to come
            // back (a master just started has not heard its boxes yet: that is no box gone)
            let bs = boxes(ctx);
            g.max_active = bs.len().saturating_sub(if sweep { 2 } else { 0 }).max(1);
            let now_ended = (0..g.state.len()).filter(|&i| g.finished(i)).count();
            if now_ended != ended {
                (ended, last_end) = (now_ended, Instant::now());
            } else if last_end.elapsed() >= STALLED {
                last_end = Instant::now();
                ctx.store.event("ci", Sev::Err, stall_line(&pair.short(), &g.state, ended), None);
            }
            // a step far past its measured time: the shard's verdict is overdue (a failure) and the
            // run can conclude; nothing is stopped, its result and time count when it ends
            for i in 0..g.shards.len() {
                let s = &g.state[i];
                if s.state != "run" || s.granted_ms == 0 || g.overdue[i] {
                    continue;
                }
                let ran = wall_ms().saturating_sub(s.granted_ms) / 1000;
                if ran as f64 >= (g.shards[i].est * OVERDUE_TIMES).clamp(OVERDUE_MIN_SECS, OVERDUE_MAX_SECS) {
                    let step = g.shards[i].steps.get(s.step as usize).map(|st| step_name(st)).unwrap_or_default();
                    let line = format!("overdue: running {} min (measured {:.0} s), at {step} on {}", ran / 60, g.shards[i].est, s.node);
                    ctx.store.event("ci", Sev::Err, format!("ci {}: {} {line}; the run goes on without it (it is not stopped)", pair.short(), s.id), None);
                    g.overdue[i] = true;
                    g.state[i].state = "fail".into();
                    g.state[i].summary = line;
                }
            }
            let busy = (0..g.shards.len()).any(|i| g.reserved[i].is_some() || g.state[i].state == "run");
            if g.state.iter().any(|s| s.state == "run") {
                last_ran = Instant::now();
            } else if last_ran.elapsed() >= STALLED {
                let asks: Vec<String> = g.state.iter().filter(|s| s.summary.starts_with(QUEUED)).map(|s| format!("{}: {}", s.id, s.summary)).take(4).collect();
                ctx.store.event("ci", Sev::Err, format!("ci {}: stalled, no shard got a box in {} min: {}", pair.short(), STALLED.as_secs() / 60, asks.join("; ")), None);
                g.stopped = true;
                cv.notify_all();
                break;
            }
            let takeable = |g: &Pool, i: usize| bs.iter().any(|b| g.can(b, i));
            let stuck: Vec<usize> = (0..g.shards.len()).filter(|&i| g.state[i].state == "wait" && g.reserved[i].is_none() && !takeable(&g, i)).collect();
            let others = (0..g.shards.len()).any(|i| g.state[i].state == "wait" && takeable(&g, i));
            if busy || others || stuck.is_empty() {
                nothing_left = None;
            } else {
                let waited = nothing_left.get_or_insert_with(Instant::now).elapsed();
                let out: Vec<String> = bs.iter().filter(|b| g.out(&b.node)).map(|b| b.node.clone()).collect();
                let gone = ctx.leases.nodes().iter().any(|n| !n.up || !n.dev);
                for i in stuck {
                    let here: Vec<&BoxInfo> = bs.iter().filter(|b| b.fits(&g.shards[i])).collect();
                    match no_box(&g.shards[i], &g.state[i], &here, &out, waited, master_t0.elapsed() >= HEAR, gone) {
                        Some((state, summary)) => {
                            g.state[i].state = state.into();
                            g.state[i].summary = summary;
                        }
                        None if here.is_empty() => g.state[i].summary = format!("waits for a box of {} ({} s)", g.shards[i].os.as_str(), waited.as_secs()),
                        None => {}
                    }
                }
            }
            drop(g);
            for b in bs {
                if seen.insert(b.node.clone()) {
                    let (ctx, pool, pair, dir) = (ctx.clone(), pool.clone(), pair.clone(), dir.clone());
                    workers.push(std::thread::Builder::new().name(format!("devhub-ci-{}", b.node)).spawn(move || worker(&ctx, &pool, &pair, &dir, b)).ok());
                }
            }
        }
        let (p, cv) = &*pool;
        let g = p.lock().unwrap();
        let _ = cv.wait_timeout(g, Duration::from_secs(5));
        let g = pool.0.lock().unwrap();
        publish_changed(ctx, &g);
        if last_batch.elapsed() >= PROOF_BATCH {
            last_batch = Instant::now();
            let fresh: Updates = passed_units(ctx, pair, &digests, &g.shards, &g.state, wall_ms()).into_iter().filter(|(k, _)| !published.contains(k)).collect();
            drop(g);
            if !fresh.is_empty() {
                match publish_proofs(ctx, pair, &fresh, calling) {
                    Ok(_) => published.extend(fresh.into_keys()),
                    Err(e) => ctx.store.event("ci", Sev::Warn, format!("ci proofs (batch of {}): {e}", fresh.len()), None),
                }
            }
        }
    }
    // (a worker whose shard is overdue is not waited for: it records its result when it ends)
    for w in workers.into_iter().flatten() {
        loop {
            if w.is_finished() {
                let _ = w.join();
                break;
            }
            let g = pool.0.lock().unwrap();
            if (0..g.shards.len()).all(|i| g.state[i].state != "run" || g.overdue[i]) && (0..g.shards.len()).any(|i| g.overdue[i] && g.state[i].state == "fail") && !w.is_finished() {
                // the box's worker runs an overdue step: detached
                break;
            }
            // a stopped run ends now: what still runs records its result when it ends, and the
            // passes so far become proofs at once (a hung shard held a stopped run for hours)
            if g.stopped {
                break;
            }
            drop(g);
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let mut g = pool.0.lock().unwrap();
    // a gate failure goes to the inbox for the lane that fixes it, with its first error lines
    if !sweep && !dry {
        for st in g.state.iter().filter(|s| s.state == "fail") {
            let errors: Vec<&str> = st.errors.iter().map(String::as_str).take(3).collect();
            ctx.store.event("ci", Sev::Err, format!("ci gate {} FAILED at {} on {} ({}): {}{}", st.id, pair.short(), st.node, st.log, st.summary, if errors.is_empty() { String::new() } else { format!(" | {}", errors.join(" | ")) }), None);
        }
    }
    let mut verdict = if g.state.iter().any(|s| s.state == "fail") {
        Verdict::Fail
    } else if g.stopped || g.state.iter().any(|s| s.state != "ok") {
        Verdict::Incomplete
    } else {
        Verdict::Ok
    };
    let ended = wall_ms();
    let (crit, boxes, fleet) = critical(&g.state, started, ended);
    let failed: Vec<String> = g.state.iter().filter(|s| s.state == "fail").map(|s| s.id.clone()).collect();
    let mut state = g.state.clone();
    for (id, why) in &why_lines {
        if let Some(st) = state.iter_mut().find(|st| &st.id == id && st.kept.is_empty()) {
            st.summary = format!("{why}; {}", st.summary);
        }
    }
    let one_box = one_box_failures(&state);
    let legs = state.iter().map(CiShard::leg).collect();
    let updates: Updates = passed_units(ctx, pair, &digests, &g.shards, &state, ended).into_iter().filter(|(k, _)| !published.contains(k)).collect();
    g.proofs_collected = true;
    drop(g);
    let published = publish_proofs(ctx, pair, &updates, calling);
    if let Err(e) = published {
        ctx.store.event("ci", Sev::Err, format!("ci proofs: {e}"), None);
        if verdict == Verdict::Ok { verdict = Verdict::Incomplete; }
    }
    let word = match verdict {
        Verdict::Ok => "ok",
        Verdict::Fail => "fail",
        Verdict::Incomplete => "incomplete",
    };
    ctx.store.update(|d| {
        let slot = d.ci.slot_mut(sweep);
        if let Some(mut r) = slot.take_if(|r| r.id == id) {
            r.shards = state;
            r.lock_digest.clear();
            r.verdict = word.into();
            r.ended_ms = ended;
            r.critical = crit.clone();
            r.fleet = fleet.clone();
            r.boxes = boxes.clone();
            d.ci.runs.push(r);
        }
    }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
    let line = format!(
        "ci {} {word}: {n} shards, {fleet}{}{}; critical path {crit}; {}",
        pair.short(),
        if failed.is_empty() { String::new() } else { format!("; failed {}", failed.join(" ")) },
        if one_box.is_empty() { String::new() } else { format!("; FAILED ON ONE BOX ONLY: {}", one_box.join("; ")) },
        arm.summary
    );
    ctx.store.event("ci", if verdict == Verdict::Fail || !one_box.is_empty() { Sev::Err } else { Sev::Info }, line, None);
    (verdict, legs)
}

/// The shards that failed on one box and passed on another: `<id> on <box>: <why>`.
fn one_box_failures(state: &[CiShard]) -> Vec<String> {
    state.iter().filter(|s| s.state == "ok" && !s.failed_on.is_empty() && s.kept.is_empty()).map(|s| format!("{} on {}: {}", s.id, s.failed_on, s.errors.first().unwrap_or(&s.failed_why))).collect()
}

fn safe(id: &str) -> String {
    id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.' { c } else { '_' }).collect()
}

/// One box: pull the longest shard it can run, take a lease on it, run its steps, again.
fn worker(ctx: &Arc<Ctx>, pool: &Arc<(Mutex<Pool>, Condvar)>, pair: &Pair, dir: &Path, b: BoxInfo) {
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
        let (shard, prio, sweep) = {
            let g = p.lock().unwrap();
            (g.shards[i].clone(), g.prio, g.sweep)
        };
        let spec = Spec {
            since_ms: None,
            // each box asks for itself: a soft pin could fall back to a twin, also one that failed
            // the shard or called itself unfit for it
            only: true,
            lane: LANE.into(),
            agent: if sweep { SWEEP_AGENT } else { AGENT }.into(),
            kind: shard.kind,
            needs: b.needs(&shard),
            prio,
            rev: pair.rev(),
            repo_dir: ctx.repo.to_string_lossy().into_owned(),
            node: Some(b.node.clone()),
        };
        let asked = Instant::now();
        let asked_ms = wall_ms();
        let stop = || asked.elapsed() > QUEUE_WAIT || p.lock().map(|g| g.stopped).unwrap_or(true);
        let mut queued = |line: &str| {
            if let Ok(mut g) = p.lock() {
                g.state[i].summary = format!("{QUEUED}{}: {line}", b.node);
            }
        };
        let (whole, cores) = (shard.whole, shard.cores);
        let held = match Held::acquire_with(&spec, &|r| { r.whole = whole; r.cores = cores; }, &mut queued, &stop) {
            Ok(h) => h,
            Err(e) => {
                // busy or held: the shard goes back, this box rests a while; a request that only
                // waited its turn asks again at once (resting, its box went to the next lane
                // lease in the queue while verify's work was owed it, 11-01)
                let mut g = p.lock().unwrap();
                g.reserved[i] = None;
                g.state[i].spans.push(CiSpan { node: b.node.clone(), asked_ms, granted_ms: 0, ended_ms: wall_ms() });
                idle_until = (!e.ends_with("given up")).then(|| Instant::now() + BACKOFF);
                cv.notify_all();
                continue;
            }
        };
        if let Err(e) = ship_lock(&held, dir, false) {
            held.release(None);
            let mut g = p.lock().unwrap();
            g.reserved[i] = None;
            g.state[i].summary = format!("lock shipment failed: {e}; retries");
            ctx.store.event("ci", Sev::Warn, g.state[i].summary.clone(), None);
            idle_until = Some(Instant::now() + BACKOFF);
            cv.notify_all();
            continue;
        }
        {
            let mut g = p.lock().unwrap();
            let s = &mut g.state[i];
            s.state = "run".into();
            s.node = held.node.clone();
            s.lease = held.id.clone();
            s.asked_ms = asked_ms;
            s.granted_ms = wall_ms();
            s.step = 0;
            s.run_n = 0;
            s.failed.clear();
            publish_changed(ctx, &g);
        }
        if run_shard(ctx, pool, i, held) {
            idle_until = Some(Instant::now() + BACKOFF);
        }
    }
}

/// A shard on its lease, from the step its state names, to its result; whether lane work took the
/// box back (the box rests a while: the shard waits again).
pub(crate) fn ship_lock(held: &Held, dir: &Path, overlay: bool) -> Result<(), String> {
    let mut entries = Vec::new();
    crate::lease::ship::entries(dir, "Cargo.lock", &mut entries)?;
    crate::lease::ship::send(held.client(), &held.node, &held.id, dir, &entries)?;
    if overlay {
        let body = br#"{"shipped":["Cargo.lock"],"deleted":[],"bases":{"Cargo.lock":"new"}}"#;
        let a = crate::cli::ask(held.client(), Some(&held.node), "ship-done", &format!("ship-done {} {}", held.id, makepad_network::digest::base64_encode(body)))?;
        if let Some(e) = a.iter().find(|e| e.starts_with("{\"err\":")) { return Err(e.clone()); }
    }
    Ok(())
}

fn run_shard(ctx: &Ctx, pool: &(Mutex<Pool>, Condvar), i: usize, held: Held) -> bool {
    let (p, cv) = pool;
    let (shard, log_path, node, run_id) = {
        let g = p.lock().unwrap();
        (g.shards[i].clone(), g.state[i].log.clone(), g.state[i].node.clone(), g.run_id.clone())
    };
    let at = p.lock().unwrap().state[i].clone();
    let mut took = Vec::new();
    let (mut state, mut summary) = run_steps(&held, &shard, Path::new(&log_path), &at, &mut |step, run_n, failed, took| {
        let mut g = p.lock().unwrap();
        let s = &mut g.state[i];
        s.step = step as u64;
        s.run_n = run_n;
        s.failed = failed.to_vec();
        // Publish each pass before the next step: a restart or retry keeps it.
        if let Some(&(k, _, true)) = took.last() {
            if !s.steps_ok.contains(&(k as u64)) { s.steps_ok.push(k as u64); }
        }
        publish_changed(ctx, &g);
    }, &mut took);
    // a crate's own time, for the batches of the next plan
    // and whether it failed: a failing crate runs in a batch of its own (its old failure must not
    // hide a new one in its batch)
    for (k, secs, ok) in took {
        if secs == 0.0 { continue; } // followed steps have a verdict but no whole time
        if let [.., t, pkg] = shard.steps[k].as_slice() {
            if t == "test" && shard.steps[k].iter().rev().nth(2).is_some_and(|w| w == "--") {
                ctx.store.update(|d| {
                    d.ci.measured(&format!("test:{pkg}"), secs);
                    d.ci.set_mark(&format!("failing:{pkg}"), !ok);
                }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
            }
        }
    }
    // Copy while the lease still resolves its target. Nothing on the node uses the LAN;
    // the master does all model work after release, on this box's worker without the pool lock.
    let grabs = if state == "ok" && shard.kind == Kind::Ci {
        let dst = ctx.logs("ci").join(&run_id).join(safe(&shard.id));
        match copy_deferred(&held, Path::new(&log_path), &dst) {
            Ok(grabs) => grabs,
            Err(e) => {
                state = "fail";
                summary = format!("vision judge on the master: {e}");
                Vec::new()
            }
        }
    } else {
        Vec::new()
    };
    let lease = held.id.clone();
    held.release(Some(if state == "ok" { 0 } else { 1 }));
    let mut secs = {
        let mut g = p.lock().unwrap();
        let s = &mut g.state[i];
        // a run a previous master left on the box may have been granted before this state knew
        if s.granted_ms == 0 {
            s.granted_ms = wall_ms();
        }
        let secs = wall_ms().saturating_sub(s.granted_ms) as f64 / 1000.0;
        close_span(s, &node);
        secs
    };
    if !grabs.is_empty() {
        {
            let mut g = p.lock().unwrap();
            g.state[i].lease.clear();
            g.state[i].summary = format!("vision judge on the master: judging {} grabs", grabs.len());
            publish_changed(ctx, &g);
            cv.notify_all();
        }
        let started = Instant::now();
        match judge_deferred(&grabs, Path::new(&log_path)) {
            Ok(n) => summary.push_str(&format!("; judged on the master: {n} grabs")),
            Err(e) => {
                state = "fail";
                summary = format!("vision judge on the master: {e}");
            }
        }
        secs += started.elapsed().as_secs_f64();
    }
    if state == "fail" && summary.starts_with("vision judge on the master:") {
        if let Ok(mut f) = std::fs::OpenOptions::new().append(true).open(&log_path) {
            for line in summary.lines().filter(|l| !l.trim().is_empty()) {
                let _ = writeln!(f, "CIERR {line}");
            }
        }
    }
    // lane work took the box back (the master preempts background work): the shard waits again
    if state == "incomplete" && super::leg::was_preempted(&lease) {
        let mut g = p.lock().unwrap();
        g.reserved[i] = None;
        g.state[i].state = "wait".into();
        g.state[i].summary = format!("preempted on {node} by lane work; asks again");
        publish_changed(ctx, &g);
        cv.notify_all();
        return true;
    }
    let errors = if state == "fail" { super::ci_status::error_lines(&std::fs::read_to_string(&log_path).unwrap_or_default()) } else { Vec::new() };
    let bs = boxes(ctx);
    let mut g = p.lock().unwrap();
    g.reserved[i] = None;
    // overdue: its real end, as it came (the run went on without it)
    if g.overdue[i] {
        let was = std::mem::take(&mut g.state[i].summary);
        g.state[i].state = state.into();
        g.state[i].summary = format!("{summary} (after {:.0} s; {was})", secs);
        g.state[i].errors = errors;
        let id = g.state[i].id.clone();
        if state != "incomplete" {
            ctx.store.update(|d| d.ci.measured(&id, secs)).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
        }
        publish_changed(ctx, &g);
        cv.notify_all();
        let late = g.proofs_collected && units_of(&shard).iter().enumerate().any(|(k, _)| unit_passed(&shard, &g.state[i], k));
        let (shards, states, run_id, sweep) = (g.shards.clone(), g.state.clone(), g.run_id.clone(), g.sweep);
        drop(g);
        if late {
            let run = ctx.store.read(|d| d.ci.slot(sweep).iter().chain(d.ci.runs.iter()).find(|r| r.id == run_id && r.sweep == sweep).cloned());
            if let Some(run) = run {
                let wanted = units_of(&shard);
                let result = Tree::at(&ctx.repo, &run.pair).and_then(|tree| Digests::at(&ctx.logs("ci").join(&run_id), &tree, &wanted)).and_then(|dg| {
                    let mut updates = passed_units(ctx, &run.pair, &dg, &shards, &states, wall_ms());
                    updates.retain(|(u, _), _| wanted.contains(u));
                    publish_proofs(ctx, &run.pair, &updates, !run.checkout.is_empty())
                });
                if let Err(e) = result { ctx.store.event("ci", Sev::Err, format!("late CI proof: {e}"), None); }
            }
        }
        return false;
    }
    // a first failure asks a second box of that OS; its log stays beside the next one's
    if state == "fail" && g.second_box(i, &node, &bs, &summary, errors.clone()) {
        let _ = std::fs::rename(&log_path, format!("{log_path}.{node}"));
        let evidence = ctx.logs("ci").join(&run_id).join(safe(&shard.id));
        if evidence.exists() {
            let _ = std::fs::rename(&evidence, evidence.with_file_name(format!("{}.{}", safe(&shard.id), safe(&node))));
        }
        publish_changed(ctx, &g);
        cv.notify_all();
        return false;
    }
    if state == "incomplete" && g.state[i].lost.iter().map(|(_, k)| *k).sum::<u32>() < 2 * LOST_TRIES {
        // a lost lease: the shard is asked again (twice on one box, then elsewhere)
        // an unfit box is not asked again; a lost lease is, once
        let n = if summary.starts_with("unfit:") { LOST_TRIES } else { 1 };
        let s = &mut g.state[i];
        match s.lost.iter_mut().find(|(b, _)| *b == node) {
            Some((_, k)) => *k += n,
            None => s.lost.push((node, n)),
        }
        g.state[i].state = "wait".into();
        g.state[i].summary = summary;
    } else {
        let first = g.state[i].failed_on.clone();
        g.state[i].state = state.into();
        g.state[i].secs = secs;
        if first.is_empty() {
            g.state[i].summary = summary;
            g.state[i].errors = errors;
        } else if state == "ok" {
            // the pass stands; what the first box said stays, loudly
            let line = format!("ci: {} FAILED on {first} and passed on {node}: {}", g.state[i].id, g.state[i].errors.first().unwrap_or(&g.state[i].failed_why));
            ctx.store.event("ci", Sev::Err, line, None);
            g.state[i].summary = format!("passed on {node}; {summary}; FAILED on {first}: {}", g.state[i].failed_why);
            let n = g.failed_only_on(&first);
            if n == BOX_OUT {
                ctx.store.event("ci", Sev::Err, format!("ci: {first} is out of this run: {n} shards failed there and passed elsewhere (its toolchain or cache?)"), None);
            }
        } else {
            g.state[i].summary = format!("failed on {first} and on {node}: {summary}");
            g.state[i].errors = errors;
        }
        if state != "incomplete" {
            let id = g.state[i].id.clone();
            ctx.store.update(|d| d.ci.measured(&id, secs)).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
        }
    }
    publish_changed(ctx, &g);
    cv.notify_all();
    false
}

struct DeferredGrab {
    step: String,
    image: PathBuf,
    prompt: String,
}

/// A gate has one smoke run. Its numbered accepts are the manifest: require every file the
/// log promised to have arrived in this transfer, so an old or partial copy cannot pass it.
fn copy_deferred(held: &Held, log: &Path, dst: &Path) -> Result<Vec<DeferredGrab>, String> {
    let text = std::fs::read_to_string(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let mut dir = None;
    let mut accepts = Vec::new();
    for line in text.lines() {
        if let Some(path) = line.strip_prefix("CI-DEFER-DIR ") {
            if dir.is_some_and(|old| old != path) || !path.starts_with("target/ci-leg/") || !path.ends_with("/deferred") || Path::new(path).components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
                return Err(format!("invalid or conflicting CI-DEFER-DIR: {path}"));
            }
            dir = Some(path);
        } else if let Some(rest) = line.strip_prefix("CI-DEFER ") {
            let (n, step) = rest.split_once(' ').ok_or_else(|| format!("invalid CI-DEFER: {rest}"))?;
            // A restart can follow the same completed lease run again if it died between
            // writing the output and noting the next step. Those log lines are not new accepts.
            if n.parse::<usize>().ok().and_then(|n| n.checked_sub(1)).and_then(|i| accepts.get(i)) == Some(&(n, step)) {
                continue;
            }
            if dir.is_none() || n != format!("{:02}", accepts.len() + 1) || step.trim().is_empty() {
                return Err(format!("invalid or out-of-order CI-DEFER: {rest}"));
            }
            accepts.push((n, step));
        }
    }
    if accepts.is_empty() {
        return Ok(Vec::new());
    }
    let dir = dir.ok_or("CI-DEFER without CI-DEFER-DIR")?;
    let copied = held.get(LANE, dir, dst)?;
    if copied.len() != accepts.len() * 3 {
        return Err("deferred files do not match the logged acceptances".into());
    }
    let mut grabs = Vec::new();
    for (n, logged_step) in accepts {
        let base = dst.join("deferred").join(n);
        let image = base.with_extension("png");
        let prompt_path = base.with_extension("txt");
        let step_path = base.with_extension("step");
        for path in [&image, &prompt_path, &step_path] {
            if !copied.contains(path) {
                return Err(format!("{logged_step}: missing {}", path.display()));
            }
        }
        let step = std::fs::read_to_string(&step_path).map_err(|e| format!("{logged_step}: {}: {e}", step_path.display()))?;
        if step.replace(['\r', '\n'], " ") != logged_step {
            return Err(format!("{logged_step}: deferred step differs from its log"));
        }
        let prompt = std::fs::read_to_string(&prompt_path).map_err(|e| format!("{logged_step}: {}: {e}", prompt_path.display()))?;
        grabs.push(DeferredGrab { step, image, prompt });
    }
    Ok(grabs)
}

/// Runs only on a shard worker, with its lease released and no pool lock held.
fn judge_deferred(grabs: &[DeferredGrab], log: &Path) -> Result<usize, String> {
    let mut f = std::fs::OpenOptions::new().append(true).open(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let judge = makepad_ci_judge::Judge::start();
    if let Some(why) = judge.unreachable() {
        return Err(format!("{}: {why}", grabs[0].step));
    }
    let mut failed = Vec::new();
    for grab in grabs {
        match judge.judge(&grab.image, &grab.prompt) {
            Ok(verdict) => {
                writeln!(f, "CI-JUDGE {} on {} in {:.1} s", grab.step, verdict.hub, verdict.secs).map_err(|e| e.to_string())?;
                for line in verdict.raw.lines() {
                    writeln!(f, "  {line}").map_err(|e| e.to_string())?;
                }
                let evidence = grab.image.with_extension("verdict.json");
                std::fs::write(&evidence, verdict.json().to_json()).map_err(|e| format!("{}: {}: {e}", grab.step, evidence.display()))?;
                if !verdict.yes {
                    let why = if verdict.parse_error { format!("vision parse failed\n{}", verdict.raw) } else { verdict.raw };
                    failed.push(format!("{}: {why}", grab.step));
                }
            }
            Err(e) => failed.push(format!("{}: {e}", grab.step)),
        }
    }
    if failed.is_empty() {
        writeln!(f, "judged on the master: {} grabs", grabs.len()).map_err(|e| e.to_string())?;
        Ok(grabs.len())
    } else {
        Err(failed.join("\n"))
    }
}

/// The steps of a shard in its lease, from the one `at` names (a run in flight there is followed
/// from the start of its output: a step's output reaches the log when it ends), their output
/// appended to its log: (state, summary). `note` hears the step, its run and the results so far.
fn run_steps(held: &Held, shard: &Shard, log: &Path, at: &CiShard, note: &mut dyn FnMut(usize, u64, &[String], &[(usize, f64, bool)]), took: &mut Vec<(usize, f64, bool)>) -> (&'static str, String) {
    let first = at.step as usize;
    let file = if first == 0 && at.run_n == 0 { std::fs::File::create(log) } else { std::fs::OpenOptions::new().append(true).create(true).open(log) };
    let Ok(mut f) = file else { return ("incomplete", "cannot write its log".into()) };
    let mut failed = at.failed.clone();
    for (k, step) in shard.steps.iter().enumerate().skip(first) {
        let mut out: Vec<u8> = Vec::new();
        // a shard's steps are its lease's runs in order (a lease starts it at its first step): a
        // step whose run the node has though no master noted it is followed, not run again
        let started = match (k == first, at.run_n) {
            (true, 0) => match held.has_run(k as u64 + 1) {
                Ok(true) => Some(k as u64 + 1),
                Ok(false) => None,
                Err(e) => {
                    let _ = writeln!(f, "== lost: {e}");
                    return ("incomplete", format!("{}: {e}", step_name(step)));
                }
            },
            (true, n) => Some(n),
            _ => None,
        };
        let t0 = Instant::now();
        let r = if let Some(n) = started {
            note(k, n, &failed, took);
            held.follow(n, 0, &mut out)
        } else {
            let _ = writeln!(f, "$ {}", step.join(" "));
            held.run_noted(step, &mut out, &mut |n| note(k, n, &failed, took))
        };
        let _ = f.write_all(&out);
        let text = String::from_utf8_lossy(&out);
        match r {
            Ok(makepad_remote::hot::JIT_ERROR_EXIT) => {
                let verdict = makepad_remote::hot::Verdict::from_output(makepad_remote::hot::JIT_ERROR_EXIT, &text)
                    .map(|v| v.line()).unwrap_or_else(|| "JIT infrastructure error; its verdict record was not received".into());
                return ("incomplete", verdict);
            }
            // the step says this box cannot do it (ci_xcheck.sh: no std for the target)
            Ok(UNFIT) => {
                let _ = writeln!(f, "== exit {UNFIT}: this box cannot run it");
                let why = text.lines().rev().find(|l| !l.starts_with("makepad-run:")).unwrap_or("");
                return ("incomplete", format!("unfit: {}: {why}", step_name(step)));
            }
            Ok(code) => {
                let (ok, why) = judge(shard.judge, &step_name(step), code, &text);
                let _ = writeln!(f, "== exit {code}{}", if ok { String::new() } else { format!(": {why}") });
                // (a run followed from its middle has no whole time)
                took.push((k, if started.is_none() { t0.elapsed().as_secs_f64() } else { 0.0 }, ok));
                if !ok {
                    failed.push(why);
                }
                note(k + 1, 0, &failed, took);
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

// ---------------------------------------------------------------- proof publication

pub type Updates = BTreeMap<(Unit, Os), Proof>;

pub(crate) fn date(ms: u64) -> String {
    let t = stamp(ms);
    format!("{}-{}-{}", &t[..4], &t[4..6], &t[6..8])
}

fn unit_passed(sh: &Shard, st: &CiShard, k: usize) -> bool {
    if (sh.id.starts_with("test:") || sh.id.starts_with("wtest:")) && sh.dirs.as_ref().is_some_and(|d| d.len() > 1 && d.len() == sh.steps.len()) {
        st.steps_ok.contains(&(k as u64))
    } else {
        st.state == "ok"
    }
}

/// A crate's tests pass only when every planned target of it passed on that OS.
fn passed_units(ctx: &Ctx, pair: &Pair, dg: &Digests, shards: &[Shard], state: &[CiShard], at_ms: u64) -> Updates {
    let oses: BTreeMap<String, Os> = ctx.leases.nodes().into_iter().filter_map(|n| n.caps.map(|c| (n.name, Os::of(&c.os)))).collect();
    let mut groups: BTreeMap<(Unit, Os), Vec<(&CiShard, bool)>> = BTreeMap::new();
    for (sh, st) in shards.iter().zip(state) {
        let os = if sh.os == Os::Any { oses.get(&st.node).copied().unwrap_or(Os::Any) } else { sh.os };
        if os == Os::Any {
            continue;
        }
        for (k, unit) in units_of(sh).into_iter().enumerate() {
            groups.entry((unit, os)).or_default().push((st, unit_passed(sh, st, k)));
        }
    }
    groups.into_iter().filter_map(|((unit, os), states)| {
        if states.iter().any(|(_, ok)| !ok) || states.iter().all(|(s, _)| !s.kept.is_empty()) {
            return None;
        }
        Some(((unit.clone(), os), Proof { digest: dg.of(&unit, os).to_string(), pair: pair.clone(), capture: String::new(), date: date(at_ms) }))
    }).collect()
}

/// Read, merge and write under one proof lock so concurrent runs preserve each other's sections.
fn write_updates(repo: &Path, updates: &Updates) -> Result<Vec<String>, String> {
    static PROOFS: Mutex<()> = Mutex::new(());
    let _proofs = PROOFS.lock().unwrap_or_else(|e| e.into_inner());
    let mut files: BTreeMap<String, (CiConf, Proofs)> = BTreeMap::new();
    for ((unit, os), proof) in updates {
        let file = unit.file();
        if !files.contains_key(&file) {
            let both = match std::fs::read_to_string(repo.join(&file)) {
                Ok(text) => (read_conf(&text)?, read_proofs(&text)?),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (CiConf::default(), Proofs::new()),
                Err(e) => return Err(format!("{file}: {e}")),
            };
            files.insert(file.clone(), both);
        }
        files.get_mut(&file).unwrap().1.insert((unit.section.clone(), *os), proof.clone());
    }
    let mut changed = Vec::new();
    for (file, (conf, proofs)) in files {
        let text = write_proofs(&conf, &proofs);
        let path = repo.join(&file);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(&text) {
            std::fs::write(&path, text).map_err(|e| format!("{file}: {e}"))?;
            changed.push(file);
        }
    }
    Ok(changed)
}

fn publish_proofs(ctx: &Ctx, pair: &Pair, updates: &Updates, calling: bool) -> Result<String, String> {
    if calling && !on_work(&ctx.repo) {
        write_updates(&ctx.repo, updates).map(|files| format!("{} proof files written", files.len()))
    } else {
        commit_proofs(ctx, pair, updates)
    }
}

/// Detached, owned worktrees; even a failed publication removes its checkouts.
struct ProofCheckout {
    trees: Vec<(std::path::PathBuf, std::path::PathBuf)>,
}

impl ProofCheckout {
    fn add(&mut self, repo: &Path, dir: &Path, rev: &str) -> Result<(), String> {
        if let Some(parent) = dir.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        super::git(repo, &["worktree", "add", "--detach", &dir.to_string_lossy(), rev])?;
        self.trees.push((repo.to_path_buf(), dir.to_path_buf()));
        let root = if dir.ends_with(COMMERCIAL) { dir.parent().and_then(Path::parent).unwrap_or(dir) } else { dir };
        crate::ci_merge::install_merge_driver(dir, root)
    }

    fn remove(&mut self) -> Result<(), String> {
        while let Some((repo, dir)) = self.trees.last() {
            super::git(repo, &["worktree", "remove", "--force", &dir.to_string_lossy()])?;
            self.trees.pop();
        }
        Ok(())
    }
}

impl Drop for ProofCheckout {
    fn drop(&mut self) {
        if let Err(e) = self.remove() {
            eprintln!("ci proof cleanup: {e}");
        }
    }
}

static MERGE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// One paired hand-in, with only proof files staged and the master's author identity.
pub fn commit_proofs(ctx: &Ctx, pair: &Pair, updates: &Updates) -> Result<String, String> {
    if updates.is_empty() {
        return Ok("no new proofs".into());
    }
    let name = format!("ci-proof-{}-{}-{}", stamp(wall_ms()), std::process::id(), MERGE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    let root = ctx.home.join("ci-proof").join(&name);
    let mut scratch = ProofCheckout { trees: Vec::new() };
    scratch.add(&ctx.repo, &root, &pair.work)?;
    let commercial = updates.keys().any(|(u, _)| u.dir.starts_with("apps/commercial/"));
    if commercial {
        scratch.add(&ctx.commercial(), &root.join(COMMERCIAL), &pair.main)?;
    }
    let files = write_updates(&root, updates)?;
    // the identity git itself would commit with (a checkout with no user.name gets the system's)
    let ident = super::git(&ctx.repo, &["var", "GIT_AUTHOR_IDENT"])?;
    let (who, email) = ident.split_once(" <").and_then(|(n, r)| r.split_once('>').map(|(e, _)| (n.trim().to_string(), e.to_string()))).ok_or_else(|| format!("git var GIT_AUTHOR_IDENT: {ident}"))?;
    let message = format!("ci proofs: {} units at {}\n\nCo-Authored-By: Claude Opus 5.5 <noreply@anthropic.com>", updates.len(), pair.short());
    let mut branches = ["-".to_string(), String::new()];
    for (i, prefix) in ["", "apps/commercial/"].iter().enumerate() {
        let changed: Vec<&str> = files.iter().filter(|f| f.starts_with("apps/commercial/") == (i == 1)).map(|f| f.strip_prefix(*prefix).unwrap_or(f)).collect();
        if changed.is_empty() {
            continue;
        }
        let repo = if i == 0 { root.clone() } else { root.join(COMMERCIAL) };
        let mut args = vec!["add", "--"];
        args.extend(changed.iter().copied());
        super::git(&repo, &args)?;
        let staged = super::git(&repo, &["diff", "--cached", "--name-only"])?;
        if staged.lines().any(|p| !changed.contains(&p) || !proof_path(p)) {
            return Err("ci proof commit contains other files".into());
        }
        super::git(&repo, &["-c", "core.hooksPath=/dev/null", "-c", &format!("user.name={who}"), "-c", &format!("user.email={email}"), "-c", "commit.gpgsign=false", "commit", "--author", &format!("{who} <{email}>"), "-m", &message])?;
        super::git(&repo, &["branch", &name, "HEAD"])?;
        branches[i] = name.clone();
    }
    scratch.remove()?;
    if branches[0] == "-" && branches[1].is_empty() {
        return Ok("no changed proofs".into());
    }
    let com = ctx.commercial().to_string_lossy().into_owned();
    let line = if branches[1].is_empty() { format!("land add {} proof", branches[0]) } else { format!("land add {} {} {} proof", branches[0], com, branches[1]) };
    let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-ci");
    let answer = crate::cli::ask(&c, None, "land", &line)?;
    let reply = answer.first().ok_or("ci proofs: no landing answer")?;
    let v = makepad_strict_json::parse(reply.as_bytes()).map_err(|_| "ci proofs: bad landing answer")?;
    if let Some(e) = v.get("err").and_then(Value::as_str) {
        return Err(format!("{name}: {e}"));
    }
    Ok(format!("{name}: {reply}"))
}

fn on_work(repo: &Path) -> bool {
    super::git(repo, &["symbolic-ref", "--quiet", "--short", "HEAD"]).is_ok_and(|b| b == "work")
}

fn head_pair(repo: &Path) -> Result<Pair, String> {
    let work = super::git(repo, &["rev-parse", "HEAD"])?;
    let main = if repo.join(COMMERCIAL).join(".git").exists() { super::git(&repo.join(COMMERCIAL), &["rev-parse", "HEAD"])? } else { String::new() };
    Ok(Pair::new(&work, &main))
}

fn calling_ctx(ctx: &Ctx, repo: std::path::PathBuf) -> Arc<Ctx> {
    Arc::new(Ctx { repo, store: ctx.store.clone(), leases: ctx.leases.clone(), machine: ctx.machine.clone(), home: ctx.home.clone(), monitor: ctx.monitor.clone() })
}

fn sweep_tests(tree: &Tree, timing: &dyn Fn(&str) -> Option<f64>) -> Vec<Shard> {
    // All rotations: a tests section covers all the crate's targets on its platform.
    let mut shards = BTreeMap::new();
    for rotation in 0..3 {
        for sh in sweep_plan(tree, timing, rotation).into_iter().filter(|s| s.id.starts_with("test:") || s.id.starts_with("wtest:")) {
            shards.entry(sh.id.clone()).or_insert(sh);
        }
    }
    shards.into_values().collect()
}

fn test_batch(id: &str) -> bool {
    ["test:cpu:", "test:gpu:", "wtest:cpu:", "wtest:gpu:"].iter().any(|p| id.starts_with(*p))
}

fn test_shards(tree: &Tree, timing: &dyn Fn(&str) -> Option<f64>) -> Vec<Shard> {
    let mut out = Vec::new();
    for mut sh in sweep_tests(tree, &|_| None) {
        if !test_batch(&sh.id) { sh.est = timing(&sh.id).unwrap_or(sh.est); out.push(sh); continue; }
        for (step, dir) in sh.steps.iter().zip(sh.dirs.iter().flatten()) {
            let mut one = sh.clone();
            let prefix = if sh.os == Os::Windows { "wtest" } else { "test" };
            let pkg = step.last().unwrap();
            one.id = format!("{prefix}:{pkg}");
            one.steps = vec![step.clone()];
            one.dirs = Some(vec![dir.clone()]);
            one.est = timing(&one.id).unwrap_or(sh.est / sh.steps.len() as f64);
            out.push(one);
        }
    }
    out
}

fn batch_tests(shards: Vec<Shard>, lines: &mut Vec<(String, String)>, timing: &dyn Fn(&str) -> Option<f64>) -> Vec<Shard> {
    let why: BTreeMap<String, String> = lines.iter().cloned().collect();
    let mut pending: BTreeMap<(Os, bool), Vec<Shard>> = BTreeMap::new();
    let mut out = Vec::new();
    for sh in shards {
        let pkg = sh.id.strip_prefix("test:").or_else(|| sh.id.strip_prefix("wtest:")).filter(|p| !p.contains(':'));
        let batch = pkg.is_some_and(|p| timing(&format!("failing:{p}")).is_none())
            && matches!(sh.os, Os::Linux | Os::Windows) && sh.steps.len() == 1 && sh.dirs.as_ref().is_some_and(|d| d.len() == 1)
            && !why.get(&sh.id).is_some_and(|w| w.starts_with("kept:"));
        if batch { pending.entry((sh.os, sh.gpu)).or_default().push(sh); } else { out.push(sh); }
    }
    // Proofs stay per crate; only the dirty crates share a lease.
    for ((os, gpu), list) in pending {
        let by_id: BTreeMap<&str, &Shard> = list.iter().map(|sh| (sh.id.as_str(), sh)).collect();
        lines.retain(|(id, _)| !by_id.contains_key(id.as_str()));
        for (i, ids) in groups(list.iter().map(|sh| (sh.id.clone(), sh.est)).collect(), GROUP_SECS).into_iter().enumerate() {
            let members: Vec<&Shard> = ids.iter().map(|id| by_id[id.as_str()]).collect();
            let mut sh = (*members[0]).clone();
            let prefix = if os == Os::Windows { "wtest" } else { "test" };
            let tag = if gpu { "gpu" } else { "cpu" };
            sh.id = format!("{prefix}:{tag}:{:02}", i + 1);
            sh.steps = members.iter().flat_map(|s| s.steps.iter().cloned()).collect();
            sh.dirs = Some(members.iter().flat_map(|s| s.dirs.iter().flatten().cloned()).collect());
            sh.est = members.iter().map(|s| s.est).sum();
            let reasons: Vec<String> = members.iter().map(|s| format!("{}: {}", s.id.split_once(':').unwrap().1, why.get(&s.id).map_or("no proof", String::as_str))).collect();
            lines.push((sh.id.clone(), format!("runs: {}", reasons.join("; "))));
            out.push(sh);
        }
    }
    out
}

fn kept_shard(tree: &Tree, dg: &Digests, sh: &Shard) -> Result<Option<String>, String> {
    let units = units_of(sh);
    if units.is_empty() {
        return Ok(None);
    }
    let mut kept = None;
    for unit in units {
        let proofs = proofs_at(tree, &unit)?;
        let oses = if sh.os == Os::Any { vec![Os::Linux, Os::Macos] } else { vec![sh.os] };
        let Some((os, p)) = oses.iter().find_map(|&os| proofs.get(&(unit.section.clone(), os)).filter(|p| p.digest == dg.of(&unit, os)).map(|p| (os, p))) else { return Ok(None) };
        kept = Some(format!("kept: proof on {} from {} {}", os.as_str(), p.pair.short(), p.date));
    }
    Ok(kept)
}

/// The calling checkout's committed units that need work. No hub, leases or state store.
pub fn status(repo: &Path) -> Result<Value, String> {
    let pair = head_pair(repo)?;
    let tree = Tree::at(repo, &pair)?;
    let apps = plan(&tree, &|_| None, 0);
    let tests = test_shards(&tree, &|_| None);
    // only a unit with a proof is hashed: one without needs work whatever its digest
    let mut units: Vec<Unit> = Vec::new();
    for unit in apps.iter().chain(&tests).flat_map(units_of) {
        if proofs_at(&tree, &unit)?.keys().any(|(section, _)| *section == unit.section) { units.push(unit); }
    }
    let dg = Digests::at(repo, &tree, &units)?;
    let (picked, lines) = select(repo, &pair, &tree, &dg, apps, &|_| 1, &BTreeSet::new())?;
    let mut dirty = BTreeMap::new();
    let mut changes = Changes::new();
    for sh in picked.iter().chain(&tests) {
        if sh.id.starts_with("smoke:") && lines.iter().any(|(id, why)| id == &sh.id && why.starts_with("kept:")) {
            continue;
        }
        for unit in units_of(sh) {
            let proofs = proofs_at(&tree, &unit)?;
            if proofs.get(&(unit.section.clone(), sh.os)).is_some_and(|p| p.digest == dg.of(&unit, sh.os)) {
                continue;
            }
            let paths = changed_paths(repo, &pair, newest(&proofs, &unit.section), dg.closures.get(&unit), &mut changes);
            let why = paths.first().map_or("no proof".to_string(), |p| format!("{p} changed"));
            dirty.insert((unit, sh.os), why);
        }
    }
    let rows = dirty.into_iter().map(|((u, os), why)| obj(vec![("dir", s(&u.dir)), ("unit", s(&u.section)), ("os", s(os.as_str())), ("why", s(why))])).collect();
    Ok(obj(vec![("pair", s(pair.short())), ("dirty", Value::Arr(rows))]))
}

/// Imports only the newest actual passes whose inputs still match today's work.
pub fn seed(ctx: &Ctx) -> Result<String, String> {
    let pair = super::snapshot(&ctx.repo).ok_or("no stable work tip")?;
    let tree = Tree::at(&ctx.repo, &pair)?;
    let oses: BTreeMap<String, Os> = ctx.leases.nodes().into_iter().filter_map(|n| n.caps.map(|c| (n.name, Os::of(&c.os)))).collect();
    let mut runs = ctx.store.read(|d| d.ci.runs.clone());
    runs.sort_by_key(|r| std::cmp::Reverse(r.ended_ms.max(r.started_ms)));
    let mut seen = BTreeSet::new();
    let mut updates = Updates::new();
    for run in runs {
        let old = Tree::at(&ctx.repo, &run.pair)?;
        let timings = |id: &str| run.plan_timings.iter().find(|(k, _)| k == id).map(|(_, t)| *t);
        let planned = sweep_tests(&old, &timings);
        let mut kept: Vec<(String, String)> = run.shards.iter().filter(|s| !s.kept.is_empty()).map(|s| (s.id.clone(), s.kept.clone())).collect();
        let batched = batch_tests(test_shards(&old, &timings), &mut kept, &timings);
        let packages: BTreeMap<String, String> = workspace_crates(&old).into_iter().map(|k| (k.package, k.dir)).collect();
        let mut rows: BTreeMap<(Unit, Os), Vec<(&CiShard, bool)>> = BTreeMap::new();
        for st in run.shards.iter().filter(|s| s.kept.is_empty()) {
            let (units, os) = if let Some(rest) = st.id.strip_prefix("smoke:") {
                let (os, dir) = rest.split_once(':').filter(|(os, _)| Os::of(os) != Os::Any).map_or((oses.get(&st.node).copied().unwrap_or(Os::Any), rest), |(os, dir)| (Os::of(os), dir));
                (vec![(Unit::app(dir), st.state == "ok")], os)
            } else if st.id.starts_with("test:") || st.id.starts_with("wtest:") {
                let units = if test_batch(&st.id) {
                    // Kept crates changed the batches; the recorded steps must still match.
                    let Some(sh) = batched.iter().chain(&planned).find(|sh| sh.id == st.id && sh.sig() == st.sig) else { continue };
                    sh.steps.iter().enumerate().filter_map(|(k, step)| {
                        let dir = packages.get(step.last()?)?;
                        Some((Unit { dir: dir.clone(), section: "tests".into() }, unit_passed(sh, st, k)))
                    }).collect()
                } else {
                    planned.iter().find(|s| s.id == st.id).map(units_of).unwrap_or_else(|| {
                        st.id.split(':').nth(1).and_then(|p| packages.get(p)).map(|d| vec![Unit { dir: d.clone(), section: "tests".into() }]).unwrap_or_default()
                    }).into_iter().map(|u| (u, st.state == "ok")).collect()
                };
                (units, if st.id.starts_with("wtest:") { Os::Windows } else { oses.get(&st.node).copied().unwrap_or(Os::Linux) })
            } else { continue };
            if os == Os::Any { continue; }
            for (unit, ok) in units {
                rows.entry((unit, os)).or_default().push((st, ok));
            }
        }
        let units: Vec<Unit> = rows.keys().filter(|(u, _)| tree.has(&join(&u.dir, "Cargo.toml"))).map(|(u, _)| u.clone()).collect();
        if units.is_empty() { continue; }
        let before = match std::fs::read_to_string(ctx.logs("ci").join(&run.id).join("Cargo.lock")) {
            Ok(lock) => Digests::with_lock(&old, &units, lock)?,
            Err(_) => Digests::at(&ctx.repo, &old, &units)?,
        };
        let now = Digests::at(&ctx.repo, &tree, &units)?;
        for ((unit, os), states) in rows {
            if !units.contains(&unit) || states.iter().any(|(_, ok)| !ok) { continue; }
            let whole_crate = states.iter().any(|(st, _)| test_batch(&st.id)) || packages.iter().find(|(_, dir)| *dir == &unit.dir).is_some_and(|(pkg, _)| states.iter().any(|(st, _)| st.id == format!("test:{pkg}") || st.id == format!("wtest:{pkg}")));
            if unit.section == "tests" && !whole_crate && planned.iter().filter(|s| s.os == os && units_of(s).contains(&unit)).any(|s| !states.iter().any(|(st, _)| st.id == s.id)) { continue; }
            if !seen.insert((unit.clone(), os)) { continue; }
            if before.of(&unit, os) == now.of(&unit, os) {
                updates.insert((unit.clone(), os), Proof { digest: now.of(&unit, os).into(), pair: run.pair.clone(), capture: String::new(), date: date(run.ended_ms.max(run.started_ms)) });
            }
        }
    }
    commit_proofs(ctx, &pair, &updates)
}

// ---------------------------------------------------------------- service and CLI

pub const HELP: &str = "makepad ci:
  status [--json]                        checkout proofs, gate run, then the active sweep
  run                                   run this checkout's dirty units over the master's pool
  seed                                  seed committed proofs from the master's passing runs
  sweep [<work> <main>]                  start a background sweep alongside the gate
  plan [<work> <main>] [--sweep]         show shards, boxes and estimates
  stop [--sweep]                         stop the gate (or sweep); running shards finish";

static STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static SWEEP_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// Gate runs share a slot; the background sweep owns an independent slot.
static GATE: Mutex<()> = Mutex::new(());
static SWEEP: Mutex<()> = Mutex::new(());

fn run_slot(sweep: bool) -> &'static Mutex<()> {
    if sweep { &SWEEP } else { &GATE }
}

fn run_stop(sweep: bool) -> &'static std::sync::atomic::AtomicBool {
    if sweep { &SWEEP_STOP } else { &STOP }
}

fn run_status(ci: &Ci, work: Option<&str>) -> Value {
    let matches = |r: &&CiRun| work.is_none_or(|w| r.pair.work == w);
    let gate = ci.run.iter().chain(ci.runs.iter().rev().filter(|r| !r.sweep)).find(matches);
    let mut fields = match gate.map(crate::state::ci_run_value) {
        Some(Value::Obj(fields)) => fields,
        _ => vec![("note".into(), s("no CI run yet"))],
    };
    if let Some(run) = ci.sweep_run.as_ref().filter(matches) {
        fields.push(("sweep_run".into(), crate::state::ci_run_value(run)));
    }
    Value::Obj(fields)
}

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
    if let Some(json) = line.strip_prefix("ci run ").filter(|s| s.trim_start().starts_with('{')) {
        let v = makepad_strict_json::parse(json.as_bytes()).map_err(|_| "ci run: bad request")?;
        let get = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("");
        let root = std::path::PathBuf::from(get("root"));
        if !root.is_absolute() || !root.join(".git").exists() { return Err("ci run: no calling checkout".into()); }
        let pair = Pair::new(get("work"), get("main"));
        if head_pair(&root)? != pair { return Err("ci run: the calling checkout moved".into()); }
        // Validate locally before accepting; a missing lock is incomplete evidence.
        status(&root)?;
        let c = calling_ctx(ctx, root);
        // The service owns this request until the gate ends, including proof write-back.
        let (verdict, legs) = gate_for(&c, &pair, true, false, true);
        let word = match verdict { Verdict::Ok => "ok", Verdict::Fail => "fail", Verdict::Incomplete => "incomplete" };
        return Ok(obj(vec![("verdict", s(word)), ("pair", s(pair.short())), ("shards", Value::Int(legs.len() as i64))]).to_json());
    }
    let w: Vec<&str> = line.split_whitespace().skip(1).collect();
    match w.first().copied() {
        None | Some("status") => Ok(ctx.store.read(|d| run_status(&d.ci, w.get(1).copied()).to_json())),
        Some("seed") => seed(ctx).map(|message| obj(vec![("seed", s(message))]).to_json()),
        Some("plan") => {
            let sweep = w.contains(&"--sweep");
            let rest: Vec<&str> = w[1..].iter().copied().filter(|x| *x != "--sweep").collect();
            let pair = pair_of(ctx, &rest)?;
            let tree = Tree::at(&ctx.repo, &pair)?;
            let rotation = ctx.store.read(|d| d.ci.rotation + 1);
            let arm = arm_decision(ctx, &pair);
            let mut shards = ctx.store.read(|d| {
                let mut shards = if sweep { sweep_plan(&tree, &|s| d.ci.timing(s), rotation) } else { plan(&tree, &|s| d.ci.timing(s), rotation) };
                if !sweep && arm.full {
                    shards.extend(sweep_plan(&tree, &|s| d.ci.timing(s), rotation).into_iter().filter(|s| s.id.starts_with("mjit:")));
                }
                shards
            });
            let skipped = arm_plan(&tree, &mut shards, &arm);
            let bs = boxes(ctx);
            let rows: Vec<Value> = shards
                .iter()
                .map(|sh| {
                    let fits: Vec<Value> = bs.iter().filter(|b| b.fits(sh)).map(|b| s(&b.node)).collect();
                    obj(vec![("id", s(&sh.id)), ("os", s(sh.os.as_str())), ("gui", Value::Bool(sh.gui)), ("est", Value::F64(sh.est)), ("steps", Value::Int(sh.steps.len() as i64)), ("boxes", Value::Arr(fits))])
                })
                .collect();
            Ok(obj(vec![("pair", s(pair.short())), ("arm64", s(&arm.summary)), ("classified", Value::Arr(skipped.iter().map(|sh| s(&sh.id)).collect())), ("shards", Value::Arr(rows))]).to_json())
        }
        Some("run") => Err("ci run needs the calling checkout (use makepad ci run)".into()),
        Some("sweep") => {
            let rest: Vec<&str> = w[1..].iter().copied().filter(|x| *x != "--dry").collect();
            let pair = pair_of(ctx, &rest)?;
            let c = ctx.clone();
            let p = pair.clone();
            let (held, slot) = std::sync::mpsc::channel();
            let _ = std::thread::Builder::new().name("devhub-ci-sweep".into()).spawn(move || {
                let one = match SWEEP.try_lock() {
                    Ok(one) => one,
                    Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
                    Err(std::sync::TryLockError::WouldBlock) => { let _ = held.send(false); return; }
                };
                SWEEP_STOP.store(false, std::sync::atomic::Ordering::SeqCst);
                let _ = held.send(true);
                let _ = gate_held(one, &c, &p, true, true, false);
            }).map_err(|e| format!("sweep: {e}"))?;
            if !slot.recv().map_err(|e| format!("sweep: {e}"))? {
                return Err("a sweep is going (makepad ci stop --sweep)".into());
            }
            Ok(obj(vec![("started", s(pair.short())), ("dry", Value::Bool(true)), ("sweep", Value::Bool(true))]).to_json())
        }
        Some("stop") => {
            if w[1..].iter().any(|w| *w != "--sweep") { return Err("ci stop [--sweep]".into()); }
            let sweep = w.contains(&"--sweep");
            run_stop(sweep).store(true, std::sync::atomic::Ordering::SeqCst);
            ctx.store.update(|d| if let Some(r) = d.ci.slot_mut(sweep) { r.stopping = true; })?;
            Ok(obj(vec![("stopping", Value::Bool(run_slot(sweep).try_lock().is_err()))]).to_json())
        }
        Some(x) => Err(format!("ci: unknown word {x} (ci status | run | seed | sweep | plan | stop)")),
    }
}

/// The run as lines: verdict, wall, critical path, boxes, then the shards not ok.
pub fn render(v: &Value) -> String {
    let t = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let sweep = v.get("sweep_run").map(render).unwrap_or_default();
    if let Some(n) = v.get("note").and_then(|x| x.as_str()) {
        return format!("{n}\n{sweep}");
    }
    let num = |x: &Value, k: &str| x.get(k).and_then(|y| y.as_u64()).unwrap_or(0);
    let shards = v.get("shards").and_then(|x| x.as_arr()).unwrap_or(&[]);
    let count = |st: &str| shards.iter().filter(|x| x.get("state").and_then(|y| y.as_str()) == Some(st)).count();
    let (start, end) = (num(v, "started_ms"), num(v, "ended_ms"));
    let wall = if end > 0 { format!("{:.0} s", (end - start) as f64 / 1000.0) } else { format!("{} running", super::age(start)) };
    let pair = v.get("pair").map(|p| crate::state::Pair::new(p.get("work").and_then(|x| x.as_str()).unwrap_or(""), p.get("main").and_then(|x| x.as_str()).unwrap_or("")).short()).unwrap_or_default();
    let mut out = if v.get("sweep").and_then(Value::as_bool) == Some(true) {
        format!("sweep {} {pair}: {} of {} shards ok, {} fail, {} running, {} waiting, {} incomplete, wall {wall}\n", t("id"), count("ok"), shards.len(), count("fail"), count("run"), count("wait"), count("incomplete"))
    } else {
        format!(
            "ci {} {pair}: {} ({} shards: {} ok, {} fail, {} running, {} waiting, {} incomplete), wall {wall}\n",
            t("id"),
            t("verdict"),
            shards.len(),
            count("ok"),
            count("fail"),
            count("run"),
            count("wait"),
            count("incomplete")
        )
    };
    if !t("critical").is_empty() {
        out.push_str(&format!("critical path: {}\n", t("critical")));
    }
    if !t("fleet").is_empty() {
        out.push_str(&format!("fleet: {}\n", t("fleet")));
    }
    for b in v.get("boxes").and_then(|x| x.as_arr()).unwrap_or(&[]) {
        out.push_str(&format!("  box {}\n", b.as_str().unwrap_or("")));
    }
    let waits_for_a_box = |x: &Value| x.get("summary").and_then(|y| y.as_str()).is_some_and(|y| y.starts_with("waits for a box") || y.starts_with(QUEUED));
    let one_box = |x: &Value| x.get("state").and_then(|y| y.as_str()) == Some("ok") && x.get("failed_on").and_then(|y| y.as_str()).is_some_and(|y| !y.is_empty()) && x.get("kept").and_then(|y| y.as_str()).is_some_and(str::is_empty);
    for sh in shards.iter().filter(|x| one_box(x) || waits_for_a_box(x) || !matches!(x.get("state").and_then(|y| y.as_str()), Some("ok") | Some("wait"))) {
        let g = |k: &str| sh.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        out.push_str(&format!("  {:10} {:40} {:8} {}\n", g("state"), g("id"), g("node"), g("summary")));
        for e in sh.get("errors").and_then(|x| x.as_arr()).unwrap_or(&[]).iter().take(6) {
            out.push_str(&format!("      {}\n", e.as_str().unwrap_or("")));
        }
    }
    out.push_str(&sweep);
    out
}

/// Decode the master's reply for run commands and the live part of local status.
fn ask_ci(c: &makepad_remote::link::Client, line: &str) -> Result<Value, String> {
    let lines = crate::cli::ask(c, None, "ci", line)?;
    let first = lines.first().ok_or("ci: no answer")?;
    let v = makepad_strict_json::parse_depth(first.as_bytes(), 16).map_err(|_| "ci: bad answer".to_string())?;
    match v.get("err").and_then(Value::as_str) {
        Some(e) => Err(e.to_string()),
        None => Ok(v),
    }
}

/// `makepad ci ...` (the binary receives `ci` as its first word).
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
    let checkout = || -> Result<std::path::PathBuf, String> {
        let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
        let root = cwd.ancestors().find(|p| p.join("libs/devhub/Cargo.toml").is_file()).ok_or("no Makepad checkout here")?;
        root.canonicalize().map_err(|e| e.to_string())
    };
    if words.is_empty() || words.as_slice() == ["status"] {
        return Some(match checkout().and_then(|root| status(&root)) {
            Ok(mut v) => {
                if let Ok(c) = crate::cli::control() {
                    if crate::cli::has(&c, "ci") {
                        match ask_ci(&c, "ci status") {
                            Ok(runs) => if let Value::Obj(fields) = &mut v { fields.push(("ci".into(), runs)); },
                            Err(e) => { eprintln!("makepad ci: {e}"); return Some(1); }
                        }
                    }
                }
                if json { println!("{}", v.to_json()); } else {
                    let rows = v.get("dirty").and_then(Value::as_arr).unwrap_or(&[]);
                    for row in rows {
                        let get = |k: &str| row.get(k).and_then(Value::as_str).unwrap_or("");
                        println!("{} {}.{}: {}", get("dir"), get("unit"), get("os"), get("why"));
                    }
                    if rows.is_empty() { println!("CI proofs match this checkout"); }
                    if let Some(runs) = v.get("ci") { print!("{}", render(runs)); }
                }
                0
            }
            Err(e) => { eprintln!("makepad ci: {e}"); 1 }
        });
    }
    let c = match crate::cli::control() {
        Ok(c) if crate::cli::has(&c, "ci") => c,
        _ => {
            eprintln!("makepad ci: no reachable devhub master with ci");
            return Some(2);
        }
    };
    let line = if words.first() == Some(&"run") {
        if words.len() != 1 { eprintln!("use makepad ci run in the checkout to test"); return Some(2); }
        match checkout().and_then(|root| head_pair(&root).map(|pair| (root, pair))) {
            Ok((root, pair)) => format!("ci run {}", obj(vec![("root", s(root.to_string_lossy())), ("work", s(pair.work)), ("main", s(pair.main))]).to_json()),
            Err(e) => { eprintln!("makepad ci: {e}"); return Some(1); }
        }
    } else { format!("ci {}", words.join(" ")) };
    Some(match ask_ci(&c, &line) {
        Ok(v) => {
            if json || words.first().is_some_and(|w| *w != "status") {
                println!("{}", v.to_json());
            } else {
                print!("{}", render(&v));
            }
            if v.get("verdict").and_then(Value::as_str).is_some_and(|s| s != "ok") { 1 } else { 0 }
        }
        Err(e) => {
            eprintln!("makepad ci: {e}");
            1
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gate_and_sweep_publish_and_resume_independently() {
        let run = |id: &str, sweep| CiRun {
            id: id.into(), pair: Pair::new(&"a".repeat(40), ""), dry: true, sweep,
            lock_digest: format!("lock-{id}"), rotation: if sweep { 2 } else { 1 },
            plan_timings: vec![(id.into(), if sweep { 90.0 } else { 10.0 })],
            shards: vec![CiShard { id: id.into(), state: "run".into(), lease: format!("lease-{id}"), step: 2, run_n: 3, steps_ok: vec![0], ..Default::default() }],
            ..Default::default()
        };
        let doc = crate::state::Doc { ci: Ci { run: Some(run("gate", false)), sweep_run: Some(run("sweep", true)), ..Default::default() }, ..Default::default() };
        let mut ci = crate::state::Doc::parse(&doc.to_value().to_json()).unwrap().ci;
        assert_eq!(ci, doc.ci);
        for sweep in [false, true] {
            let r = doc.ci.slot(sweep).as_ref().unwrap();
            let going = resume_run(&mut ci, &r.pair, r.dry, sweep, &r.lock_digest, &r.checkout);
            assert_eq!(going.as_ref(), Some(r));
            assert_eq!(ci, doc.ci);
        }
        let mut pool = Pool {
            proofs_collected: false, shards: Vec::new(), state: vec![CiShard { id: "gate".into(), state: "ok".into(), ..Default::default() }],
            reserved: Vec::new(), stopped: false, prio: PRIO, max_active: 1, overdue: Vec::new(), run_id: "gate".into(), sweep: false,
        };
        let sweep = ci.sweep_run.clone();
        publish(&mut ci, &pool);
        assert_eq!(ci.run.as_ref().unwrap().shards, pool.state);
        assert_eq!(ci.sweep_run, sweep);
        let gate = ci.run.clone();
        pool.sweep = true;
        pool.run_id = "sweep".into();
        pool.state = vec![CiShard { id: "tests".into(), state: "fail".into(), summary: "exit 1".into(), errors: vec!["tests failed".into()], ..Default::default() }];
        publish(&mut ci, &pool);
        assert_eq!(ci.sweep_run.as_ref().unwrap().shards, pool.state);
        assert_eq!(ci.run, gate);

        let shown = render(&run_status(&ci, None));
        assert!(shown.starts_with("ci gate "), "{shown}");
        assert!(shown.contains(&format!("sweep sweep {}: 0 of 1 shards ok, 1 fail", ci.sweep_run.as_ref().unwrap().pair.short())), "{shown}");
        assert!(shown.contains("tests failed"), "{shown}");

        let sweep = ci.sweep_run.as_ref().unwrap().clone();
        assert!(resume_run(&mut ci, &sweep.pair, sweep.dry, true, "new lock", "").is_none());
        assert_eq!(ci.run, gate);
        assert!(ci.sweep_run.is_none());
        assert_eq!(ci.runs.last().unwrap().id, "sweep");
        assert!(ci.runs.last().unwrap().sweep);
        ci.sweep_run = Some(run("next-sweep", true));
        let next = ci.sweep_run.clone();
        pool.state[0].state = "ok".into();
        publish(&mut ci, &pool);
        assert_eq!(ci.runs.last().unwrap().shards, pool.state);
        assert_eq!(ci.sweep_run, next);
        assert_eq!(ci.run, gate);
        pool.run_id = "gate".into();
        publish(&mut ci, &pool);
        assert_eq!(ci.run, gate);
        assert_eq!(ci.sweep_run, next);
    }

    #[test]
    fn gate_and_sweep_have_separate_slots_and_stops() {
        let gate = run_slot(false).try_lock().unwrap();
        let sweep = run_slot(true).try_lock().unwrap();
        assert!(run_slot(false).try_lock().is_err());
        assert!(run_slot(true).try_lock().is_err());
        run_stop(false).store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!run_stop(true).load(std::sync::atomic::Ordering::SeqCst));
        run_stop(false).store(false, std::sync::atomic::Ordering::SeqCst);
        run_stop(true).store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!run_stop(false).load(std::sync::atomic::Ordering::SeqCst));
        run_stop(true).store(false, std::sync::atomic::Ordering::SeqCst);
        drop(gate);
        assert!(run_slot(false).try_lock().is_ok());
        assert!(run_slot(true).try_lock().is_err());
        drop(sweep);
    }

    fn tree() -> Tree {
        Tree::mem(vec![
            ("Cargo.toml", "[workspace]\nmembers = [\"platform\", \"widgets\", \"apps/*\", \"tools/ci_leg\"]\nexclude = [\"apps/gone\"]\n"),
            ("platform/Cargo.toml", "[package]\nname = \"makepad-platform\"\n"),
            ("platform/src/lib.rs", "#[cfg(test)]\nmod tests {\n    #[test]\n    fn a() {}\n}\n"),
            ("platform/tests/remote.rs", "#[test]\n#[cfg(windows)]\nfn w() {}\n#[test]\nfn a() {}\n"),
            ("platform/tests/mac.rs", "//! Mac only\n#![cfg(target_os = \"macos\")]\n#[test]\nfn a() {}\n"),
            ("platform/tests/win.rs", "#[cfg(not(any(unix, target_os = \"linux\")))]\n#[test]\nfn a() {}\n#[test]\n#[ignore]\nfn b() {}\n"),
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

    fn jit_tree() -> Tree {
        Tree::mem(vec![
            ("Cargo.toml", "[workspace]\nmembers = [\"tools/run\", \"tools/run/android_host\", \"libs/relax/*\", \"libs/devhub\"]\n"),
            ("tools/run/Cargo.toml", "[package]\nname = \"makepad-run\"\n[dependencies]\nrelax-compiler = { path = \"../../libs/relax/compiler\" }\n"),
            ("tools/run/src/lib.rs", "#[test]\nfn a() {}\n"),
            ("tools/run/android_host/Cargo.toml", "[package]\nname = \"makepad-android-host\"\n[dependencies]\nmakepad-run = { path = \"..\" }\n"),
            ("tools/run/android_host/src/lib.rs", "#[test]\nfn a() {}\n"),
            ("libs/relax/compiler/Cargo.toml", "[package]\nname = \"relax-compiler\"\n[dependencies]\nrelax-base = { path = \"../base\" }\n[features]\nchecks = []\n"),
            ("libs/relax/compiler/ci.toml", "[ci]\ntest_features = \"checks\"\n"),
            ("libs/relax/compiler/src/lib.rs", "#[cfg(feature = \"checks\")]\n#[test]\nfn a() {}\n"),
            ("libs/relax/base/Cargo.toml", "[package]\nname = \"relax-base\"\n"),
            ("libs/relax/base/src/lib.rs", "#[test]\nfn a() {}\n"),
            ("libs/devhub/Cargo.toml", "[package]\nname = \"makepad-devhub\"\n[dependencies]\nrelax-base = { path = \"../relax/base\" }\n"),
            ("libs/devhub/src/lib.rs", "#[test]\nfn a() {}\n"),
        ])
    }

    #[test]
    fn jit_own_dirs_exclude_shared_crates() {
        let own = jit_own_dirs(&jit_tree());
        assert_eq!(own, ["tools/run", "libs/relax/compiler"].into_iter().map(str::to_string).collect());
        assert!(!own.contains("tools/run/android_host"));
    }

    #[test]
    fn jit_own_tests_use_cargo() {
        let t = jit_tree();
        for failing in [false, true] {
            let p = sweep_plan(&t, &|id| (failing && id == "failing:relax-compiler").then_some(1.0), 0);
            let compiler = argv(&["cargo", "test", "--release", "-p", "relax-compiler", "--features", "checks"]);
            for os in [Os::Linux, Os::Windows] {
                let steps: Vec<_> = p.iter().filter(|s| s.kind == Kind::Test && s.os == os).flat_map(|s| &s.steps).collect();
                assert!(steps.contains(&&argv(&["cargo", "test", "--release", "-p", "makepad-run"])), "loader on {os:?}");
                assert!(steps.contains(&&jit(&["test", "relax-base"])), "shared dependency on {os:?}");
                assert!(steps.contains(&&jit(&["test", "makepad-devhub", "--lib"])), "other member on {os:?}");
                assert_eq!(steps.contains(&&compiler), !failing || os == Os::Linux, "compiler on {os:?}, failing={failing}");
            }
            if failing {
                assert_eq!(p.iter().find(|s| s.id == "test:relax-compiler").unwrap().steps, vec![compiler]);
            }
        }
    }

    #[test]
    fn the_sweep_has_every_kind_of_shard_on_the_right_os() {
        let t = tree();
        let p = sweep_plan(&t, &|_| None, 0);
        assert!(p.iter().all(|s| s.kind != Kind::Ci));
        let ids: Vec<&str> = p.iter().map(|s| s.id.as_str()).collect();
        for want in ["jit:01", "mjit:01", "wjit:01", "check:wasm32-unknown-unknown", "check:aarch64-apple-tvos", "test:makepad-platform:lib", "test:makepad-platform:remote", "test:makepad-widgets:ui", "slice:scope", "smoke:apps/calc", "smoke:apps/tool"] {
            assert!(ids.contains(&want), "{want} in {ids:?}");
        }
        assert!(!ids.iter().any(|i| i.starts_with("check:x86_64-unknown-linux-gnu") || i.starts_with("check:aarch64-apple-darwin") || *i == "smoke:."));
        // no shard where a target has no test to run: a lib without tests, a Mac-only file, tests
        // that hold on Windows alone or are ignored
        for none in ["test:makepad-widgets:lib", "test:makepad-platform:mac", "test:makepad-platform:win"] {
            assert!(!ids.contains(&none), "{none} in {ids:?}");
        }
        assert!(ids.contains(&"wtest:makepad-platform:win"), "{ids:?}");
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
        let macs = |r: u64| sweep_plan(&t, &|_| None, r).into_iter().filter(|s| s.id.starts_with("smoke:") && s.os == Os::Macos).map(|s| s.id).collect::<Vec<_>>();
        assert_ne!(macs(0), macs(1));
    }

    #[test]
    fn the_gate_boots_the_apps_only() {
        let t = tree();
        let p = plan(&t, &|_| None, 0);
        assert!(!p.is_empty() && p.iter().all(|s| s.id.starts_with("smoke:") && s.gui && s.kind == Kind::Ci), "{:?}", p.iter().map(|s| &s.id).collect::<Vec<_>>());
        // each app offered on every platform it ships on (select keeps one)
        for os in ["linux", "macos", "windows"] {
            assert!(p.iter().any(|s| s.id == format!("smoke:{os}:apps/calc")), "{os}");
        }
        assert_eq!(unit_of("smoke:windows:apps/calc").as_deref(), Some("app:apps/calc"));
        assert_eq!((os_of_path("platform/src/os/windows/win32_app.rs"), os_of_path("platform/hal/src/backend/apple/metal/x.rs"), os_of_path("widgets/src/button.rs")), (Some(Os::Windows), Some(Os::Macos), None));
    }

    #[test]
    fn proof_files_do_not_change_a_digest() {
        let a = "100644 blob aaa\tapps/calc/src/main.rs\n100644 blob old\tapps/calc/ci.toml\n";
        let b = "100644 blob aaa\tapps/calc/src/main.rs\n100644 blob new\tapps/calc/ci.toml\n100644 blob xyz\tlibs/core/ci.toml\n";
        assert_eq!(digest_lines(a, &[], Os::Linux), digest_lines(b, &[], Os::Linux));
        assert_ne!(digest_lines(a, &[], Os::Linux), digest_lines(&b.replace("blob aaa", "blob bbb"), &[], Os::Linux));
        assert_ne!(digest_lines(a, &[], Os::Linux), digest_lines(a, &[], Os::Macos));
        assert_ne!(digest_lines(a, &[], Os::Linux), digest_lines(a, &["external@1 checksum".into()], Os::Linux));
    }

    #[test]
    fn a_failure_asks_a_second_box_and_a_box_failing_several_is_out() {
        let bx = |n: &str, os: Os| BoxInfo { node: n.into(), os, arch: "x86_64".into(), gui: true, gpu: true, ci: true };
        let boxes = [bx("a", Os::Linux), bx("b", Os::Linux), bx("m", Os::Macos)];
        let shard = |id: &str| Shard { id: id.into(), kind: Kind::Test, os: Os::Any, gui: false, gpu: false, steps: Vec::new(), judge: Judge::Exit, est: 1.0, dirs: None, whole: false, cores: None };
        let n = BOX_OUT + 1;
        let mut p = Pool { proofs_collected: false, shards: (0..n).map(|i| shard(&format!("s{i}"))).collect(), state: (0..n).map(|i| CiShard { id: format!("s{i}"), state: "wait".into(), ..Default::default() }).collect(), reserved: vec![None; n], stopped: false, prio: 0, max_active: 8, overdue: vec![false; n], run_id: String::new(), sweep: false };
        // the same OS only, never the box that failed it (an `Any` shard failing on Linux says
        // nothing about a Mac)
        assert!(p.second_box(0, "a", &boxes, "exit 1", vec!["error: x".into()]));
        assert_eq!(p.state[0].state, "wait");
        assert!(!p.can(&boxes[0], 0) && p.can(&boxes[1], 0) && !p.can(&boxes[2], 0));
        // a second failure stands
        assert!(!p.second_box(0, "b", &boxes, "exit 1", Vec::new()));
        // with no second box of that OS the first failure stands, and nothing was changed
        assert!(!p.second_box(1, "m", &boxes, "exit 1", Vec::new()));
        assert!(p.state[1].failed_on.is_empty() && p.state[1].state == "wait");
        // passes elsewhere are reported, and enough of them take the box out
        for i in 0..BOX_OUT {
            p.state[i].failed_on = "a".into();
            p.state[i].failed_os = "linux".into();
            p.state[i].errors = vec!["error: x".into()];
            assert!(!p.out("a"));
            p.state[i].state = "ok".into();
        }
        assert!(p.out("a") && !p.out("b"));
        assert!(!p.can(&boxes[0], BOX_OUT) && p.can(&boxes[1], BOX_OUT));
        assert_eq!(one_box_failures(&p.state)[0], "s0 on a: error: x");
        // a pass kept from an earlier run counts for that run, not this one
        for s in &mut p.state {
            s.kept = "w,m".into();
        }
        assert!(!p.out("a") && one_box_failures(&p.state).is_empty());
    }

    #[test]
    fn steps_are_judged_by_exit_warnings_and_error_counts() {
        assert_eq!(judge(Judge::Exit, "x", 1, "").0, false);
        assert!(!judge(Judge::Tests, "x", 0, "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 1 ignored; 0 measured\n").0);
        assert!(judge(Judge::Tests, "x", 0, "test result: ok. 0 passed; 0 failed\ntest result: ok. 3 passed; 0 failed\n").0);
        assert!(!judge(Judge::Tests, "cargo test --release -p x", 101, "test result: ok. 3 passed; 0 failed\nerror: could not compile `x`\n").0);
        assert_eq!(judge(Judge::Report, "x", 3, "a\nlive_replay: app pass rate 90.0%\n"), (true, "live_replay: app pass rate 90.0%".to_string()));
        assert_eq!(judge(Judge::Report, "x", 1, "").0, false);
        assert!(judge(Judge::Cargo, "x", 0, "warning: unused manifest key\n    Finished").0);
        assert!(!judge(Judge::Cargo, "x", 0, "src/a.rs:3:9: warning: unused variable: `x`\n").0);
        assert!(!judge(Judge::Check, "x", 0, "makepad check: 1193 body owners of the checked crates, 9 compiled in all, 2 errors\n").0);
        assert!(judge(Judge::Check, "x", 0, "makepad check: 1193 body owners, 9 compiled in all, 0 errors\n").0);
        let warned = "/t/platform/src/remote_core.rs:92:8: warning: function `end_fields` is never used [dead_code]\nmakepad check: 6435 body owners of the checked crates, 204 compiled in all, 0 errors\n";
        assert_eq!(judge(Judge::Check, "check makepad-platform", 0, warned), (false, "check makepad-platform: 1 warning".to_string()));
    }

    #[test]
    fn boxes_take_what_fits_and_the_critical_path_is_the_last_box() {
        let lin = BoxInfo { node: "101".into(), os: Os::Linux, arch: "x86_64".into(), gui: false, gpu: true, ci: true };
        let win = BoxInfo { node: "105".into(), os: Os::Windows, arch: "x86_64".into(), gui: true, gpu: true, ci: true };
        let sh = |os, gui| Shard { id: "s".into(), kind: Kind::Build, os, gui, gpu: false, steps: Vec::new(), judge: Judge::Exit, est: 1.0, dirs: None, whole: false, cores: None };
        assert!(lin.fits(&sh(Os::Linux, false)) && !lin.fits(&sh(Os::Any, true)) && !win.fits(&sh(Os::Any, true)) && win.fits(&sh(Os::Windows, false)));
        let shard = |id: &str, spans: Vec<(&str, u64, u64, u64)>| CiShard { id: id.into(), spans: spans.into_iter().map(|(n, a, g, e)| CiSpan { node: n.into(), asked_ms: a, granted_ms: g, ended_ms: e }).collect(), ..Default::default() };
        let state = vec![
            shard("s0", vec![("101", 1000, 1000, 31_000)]),
            // asked on one box, never granted: it went to another
            shard("s1", vec![("101", 31_000, 0, 41_000), ("101", 41_000, 41_000, 61_000)]),
            shard("s2", vec![("106", 0, 0, 5000), ("106", 5000, 11_000, 71_000)]),
            shard("kept", Vec::new()),
        ];
        let (crit, boxes, fleet) = critical(&state, 1000, 81_000);
        assert_eq!(crit, "106 ends at 70 s: s2 60 s");
        assert_eq!(boxes, vec!["101 50 s busy, 2 shards, 10 s queued", "106 60 s busy, 1 shards, 11 s queued"]);
        assert_eq!(fleet, "wall 80 s: 110 s on 2 boxes (1.6 at once), 21 s queued for leases, 10 s with nothing on any box");
        assert_eq!(groups(vec![("a".into(), 30.0), ("b".into(), 30.0), ("c".into(), 30.0)], 60.0), vec![vec!["a".to_string(), "b".into()], vec!["c".into()]]);
    }

    #[test]
    fn a_shard_no_box_can_take_waits_or_says_why() {
        let sh = Shard { id: "s".into(), kind: Kind::Build, os: Os::Macos, gui: false, gpu: false, steps: Vec::new(), judge: Judge::Exit, est: 1.0, dirs: None, whole: false, cores: None };
        let mac = BoxInfo { node: "mini".into(), os: Os::Macos, arch: "aarch64".into(), gui: true, gpu: true, ci: true };
        let wait = CiShard::default();
        // a master that just started has not heard its boxes: no box is gone
        assert_eq!(no_box(&sh, &wait, &[], &[], Duration::from_secs(3600), false, false), None);
        // heard, none of its kind here: it waits, within a bound
        assert_eq!(no_box(&sh, &wait, &[], &[], Duration::from_secs(60), true, false), None);
        let (state, why) = no_box(&sh, &wait, &[], &[], NO_BOX_WAIT, true, false).unwrap();
        assert_eq!((state, why.as_str()), ("incomplete", "no box of macos came in 15 min"));
        // every box of its kind said it cannot run it: a failure that names them
        let unfit = CiShard { summary: "unfit: xcheck x: no nightly".into(), lost: vec![("mini".into(), LOST_TRIES)], ..Default::default() };
        let (state, why) = no_box(&sh, &unfit, &[&mac], &[], Duration::ZERO, true, false).unwrap();
        assert_eq!((state, why.as_str()), ("fail", "no box can run it (mini): unfit: xcheck x: no nightly"));
        // ... unless a box that may be able to is away: it gets its time
        assert_eq!(no_box(&sh, &unfit, &[&mac], &[], Duration::ZERO, true, true), None);
        assert_eq!(no_box(&sh, &unfit, &[&mac], &[], NO_BOX_WAIT, true, true).unwrap().0, "fail");
        // the box is out of the run, or the shard's first failure has no second box
        assert_eq!(no_box(&sh, &wait, &[&mac], &["mini".to_string()], Duration::ZERO, true, false).unwrap().0, "incomplete");
        let failed = CiShard { failed_on: "mini".into(), failed_why: "exit 1".into(), ..Default::default() };
        assert_eq!(no_box(&sh, &failed, &[], &[], Duration::ZERO, true, false), Some(("fail", "exit 1".to_string())));
    }

    #[test]
    fn a_script_can_pin_its_os() {
        assert_eq!(pinned_os("// a comment\n// ci os: linux\nnil\n"), Some(Os::Linux));
        assert_eq!(pinned_os("nil\n"), None);
    }

    #[test]
    fn a_tree_from_before_ci_leg_plans_no_smokes() {
        let t = Tree::mem(vec![("Cargo.toml", "[workspace]\nmembers = [\"apps/*\"]\n"), ("apps/calc/Cargo.toml", "[package]\nname = \"makepad-app-calc\"\n"), ("apps/calc/src/main.rs", ""), ("apps/calc/ci.splash", "nil\n")]);
        assert!(plan(&t, &|_| None, 0).iter().all(|s| !s.gui));
        assert!(plan(&tree(), &|_| None, 0).iter().any(|s| s.gui));
    }

    #[test]
    fn the_ci_buckets_last_run_and_an_adopted_verdict_come_from_its_rolling_runs() {
        let pair = |c: &str| Pair::new(&c.repeat(40), &"c".repeat(40));
        let run = |id: &str, p: &Pair, started_ms: u64, verdict: &str| CiRun { id: id.into(), pair: p.clone(), started_ms, ended_ms: started_ms + 10, verdict: verdict.into(), ..Default::default() };
        let mut d = crate::state::Doc::default();
        assert_eq!(last_rolling(&d), None);
        d.ci.runs.push(run("128", &pair("a"), 100, "ok"));
        // a calling checkout's run and the sweep are not the bucket's
        d.ci.runs.push(CiRun { dry: true, checkout: "/co".into(), ..run("dry", &pair("b"), 200, "fail") });
        d.ci.runs.push(CiRun { sweep: true, ..run("sweep", &pair("b"), 300, "ok") });
        assert_eq!(last_rolling(&d), Some((100, pair("a"))));
        // one in flight (resumed after a master restart) is the newest
        d.ci.run = Some(run("129", &pair("d"), 400, "running"));
        assert_eq!(last_rolling(&d), Some((400, pair("d"))));
        // adopted: the run of the tip that started since the bucket asked, once it has a verdict
        assert!(judged(&d.ci, &pair("d"), 390).is_none(), "still running");
        let mut done = d.ci.run.take().unwrap();
        done.verdict = "fail".into();
        d.ci.runs.push(done);
        assert_eq!(judged(&d.ci, &pair("d"), 390).map(|r| r.id.as_str()), Some("129"));
        assert!(judged(&d.ci, &pair("d"), 401).is_none(), "a run before the bucket's ask is not its");
        assert!(judged(&d.ci, &pair("a"), 0).is_some() && judged(&d.ci, &pair("b"), 0).is_none());
        d.ci.runs.push(run("130", &pair("e"), 500, "incomplete"));
        assert!(judged(&d.ci, &pair("e"), 0).is_none(), "incomplete is no verdict: the bucket runs it");
    }
}
