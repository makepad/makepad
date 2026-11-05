//! `devhub ci`: independent regression and sweep runs, each planned from its pair's tree. One worker
//! per allowed CI box (a lease.conf with `slots.ci`: a Linux and a Windows box) pulls shards
//! longest first, behind lane work.
//! Smoke boots use the separate CI tree;
//! sweep shards use their own lease kinds at background priority, with the background cap and
//! preemption. A sweep never holds the gate's run slot or takes priority over lane work.
//! After each regression run the sweep's slot takes the tests of the crates whose own files changed
//! ([`own_tests_after`]): a sweep's terms, on the Linux CI box, never gating a push.
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
use std::io::Write;
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
pub const DEFAULT_SCRIPT: &str = "tools/ci_leg/default.ci.splash";
pub const COMMERCIAL: &str = "apps/commercial";

/// How a target is checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Build {
    Bin,
    /// the lib only (`--lib`): web and Android build an app as a library
    Lib,
    /// tier 3: nightly with `-Z build-std=std`
    Std,
}

/// The target matrix (mirrored from cargo_makepad): target, how it is checked, its platform (the
/// word a package's `[package.metadata.makepad] platforms` uses).
pub const TARGETS: [(&str, Build, &str); 15] = [
    ("aarch64-apple-darwin", Build::Bin, "desktop"),
    ("x86_64-pc-windows-msvc", Build::Bin, "desktop"),
    ("x86_64-unknown-linux-gnu", Build::Bin, "desktop"),
    ("wasm32-unknown-unknown", Build::Lib, "web"),
    ("aarch64-linux-android", Build::Lib, "mobile"),
    ("aarch64-apple-ios", Build::Bin, "mobile"),
    ("x86_64-linux-android", Build::Lib, "mobile"),
    ("aarch64-apple-tvos", Build::Std, "mobile"),
    ("aarch64-apple-tvos-sim", Build::Std, "mobile"),
    ("i686-linux-android", Build::Lib, "mobile"),
    ("aarch64-apple-ios-sim", Build::Bin, "mobile"),
    ("x86_64-apple-ios", Build::Bin, "mobile"),
    ("x86_64-apple-tvos", Build::Std, "mobile"),
    ("x86_64-apple-darwin", Build::Bin, "desktop"),
    ("x86_64-pc-windows-gnu", Build::Bin, "desktop"),
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

    /// The tree of `pair` in `repo` (and `repo/apps/commercial`), read in process (gitread).
    pub fn at(repo: &Path, pair: &Pair) -> Result<Tree, String> {
        let com = repo.join(COMMERCIAL);
        let mut blobs = crate::gitread::files_at(repo, &pair.work)?;
        let has_com = !pair.main.is_empty() && com.join(".git").exists();
        if has_com {
            blobs.extend(crate::gitread::files_at(&com, &pair.main)?.into_iter().map(|(p, h)| (format!("{COMMERCIAL}/{p}"), h)));
        }
        let files = blobs.keys().cloned().collect();
        let (oids, repo) = (blobs.clone(), repo.to_path_buf());
        Ok(Tree {
            id: format!("{},{}", pair.work, pair.main),
            cache: Mutex::new(BTreeMap::new()),
            files,
            blobs,
            read: Box::new(move |p| {
                let dir = if p.starts_with(&format!("{COMMERCIAL}/")) { &com } else { &repo };
                crate::gitread::blob(dir, oids.get(p)?).ok().map(|b| String::from_utf8_lossy(&b).into_owned())
            }),
        })
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
/// app without one.
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
        // (a tree from before the script moved, a bisect's baseline, has no default to ask)
        if covered.contains(&a.dir) || !tree.has(DEFAULT_SCRIPT) {
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

/// What packages say they are built for: `[package.metadata.makepad] platforms`. A package not
/// here is built for all of them, as is one whose list has a word that names no platform (a typo
/// must not drop its checks).
fn declared_platforms(tree: &Tree) -> BTreeMap<String, Vec<String>> {
    let declared = |manifest: &String| {
        let doc = parse_toml(&tree.read(manifest)?).ok()?;
        let list: Vec<String> = doc.get_path(&["package", "metadata", "makepad", "platforms"])?.as_array()?.iter().filter_map(|v| v.as_str().map(str::to_string)).collect();
        let known = list.iter().all(|word| TARGETS.iter().any(|(_, _, platform)| platform == word));
        known.then_some((doc.get_path(&["package", "name"])?.as_str()?.to_string(), list))
    };
    tree.files.iter().filter(|f| f.ends_with("/Cargo.toml")).filter_map(declared).collect()
}

/// target -> the packages checked there (from every script's `check_targets`), but for a package
/// not built for that target's platform.
pub fn matrix(tree: &Tree, scripts: &[Script]) -> BTreeMap<String, BTreeSet<String>> {
    let declared = declared_platforms(tree);
    let mut m: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for sc in scripts {
        let Some(text) = tree.read(&sc.path) else { continue };
        let default = sc.target.as_deref().and_then(|t| t.split(':').next());
        for (packages, targets) in check_specs(&text, default) {
            for (t, _, platform) in TARGETS {
                if targets.as_ref().is_none_or(|ts| ts.iter().any(|x| x == t)) {
                    let built = |p: &&String| declared.get(*p).is_none_or(|list| list.iter().any(|word| word == platform));
                    m.entry(t.to_string()).or_default().extend(packages.iter().filter(built).cloned());
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
/// A named check's first guess (an export matrix runs for minutes).
const GUESS_CHECK_SECS: f64 = 600.0;

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
    /// `checks = "a::b,c::d"`: ignored lib tests the sweep runs on every pass, each its own shard
    /// (too heavy for the crate's tests, such as a whole export matrix)
    pub checks: Vec<String>,
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
        if !self.checks.is_empty() {
            out.push(("checks", self.checks.join(",")));
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
            "checks" => c.checks = v.split(',').map(str::trim).filter(|t| !t.is_empty()).map(str::to_string).collect(),
            "os" => {
                c.os = v.split(',').map(str::trim).filter(|os| !os.is_empty()).map(Os::of).collect();
                if c.os.contains(&Os::Any) {
                    return Err(format!("ci.toml: [ci] os = {v:?}: linux, macos or windows"));
                }
            }
            _ => return Err(format!("ci.toml: [ci] {k} = {v:?}: unknown (box = \"whole\", cores, test_features, os, checks)")),
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

/// A unit's digests (Linux, macOS, Windows) and its closure, by (pair, lock, dir, [`digest_kind`]).
type DigestKey = (String, String, String, &'static str);
static DIGESTS: LazyLock<Mutex<HashMap<DigestKey, ([String; 3], BTreeSet<String>)>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
/// Entries kept before the cache starts over (about a hundred pairs of every unit).
const DIGESTS_KEPT: usize = 100_000;
/// Whether a Rust source (by object id) names a file (by name), for [`Digests`]' left-out
/// documents ([`makepad_ci_merge::inert`]); an object's text never changes.
static NAMES: LazyLock<Mutex<HashMap<(String, String), bool>>> = LazyLock::new(|| Mutex::new(HashMap::new()));
const NAMES_KEPT: usize = 500_000;

/// The sections whose units hash the same files: an app's (with the JIT), its tests' (no
/// `ci.splash`), and its jobs'.
fn digest_kind(section: &str) -> &'static str {
    match section { "app" => "app", "tests" => "tests", _ => "job" }
}

/// Whether a Rust source of `tree` under `krate` names `file` ([`makepad_ci_merge::names`]).
fn named_in(tree: &Tree, krate: &str, file: &str) -> bool {
    let name = file.rsplit('/').next().unwrap_or(file).to_string();
    let prefix = if krate.is_empty() { String::new() } else { format!("{krate}/") };
    for (path, oid) in tree.blobs.range(prefix.clone()..).take_while(|(p, _)| p.starts_with(&prefix)).filter(|(p, _)| p.ends_with(".rs")) {
        let key = (oid.clone(), name.clone());
        let known = NAMES.lock().unwrap().get(&key).copied();
        let hit = match known {
            Some(hit) => hit,
            None => {
                let hit = (tree.read)(path).is_some_and(|text| makepad_ci_merge::names(&text, file));
                let mut kept = NAMES.lock().unwrap();
                if kept.len() >= NAMES_KEPT { kept.clear(); }
                kept.insert(key, hit);
                hit
            }
        };
        if hit {
            return true;
        }
    }
    false
}

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
        let mut computed: BTreeMap<(String, &'static str), Unit> = BTreeMap::new();
        for unit in units.iter().collect::<BTreeSet<_>>() {
            let key = (unit.dir.clone(), digest_kind(&unit.section));
            let kept_key = (tree.id.clone(), lock_id.clone(), unit.dir.clone(), digest_kind(&unit.section));
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
            // a document (and a test's ci.splash) is no input unless a source of its crate names it
            let crates: BTreeSet<String> = dirs.iter().map(|d| d.trim_end_matches('/').to_string()).collect();
            for (krate, inert) in makepad_ci_merge::inert_by_crate(files.keys().map(|p| p.as_str()), &crates, unit.section == "tests") {
                for file in inert.into_iter().map(str::to_string).collect::<Vec<_>>() {
                    if !named_in(tree, &krate, &file) {
                        files.remove(&file);
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
        // the JIT, and the crates the leg names (it runs when either changes)
        let named = shard.dirs.iter().flatten().map(|d| Unit { dir: d.clone(), section: section.clone() }).collect::<Vec<_>>();
        return std::iter::once(Unit { dir: JIT_DIR.into(), section }).chain(named).collect();
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

/// How soon a shard no proof keeps runs: its own crates' files changed since their proof first,
/// then units with no proof, then those whose dependencies alone changed; the longest first
/// within each ([`Pool::next_for`]). Nearly every unit depends on something that changed since its
/// proof (a shared lib), so "changed since the proof" alone orders nothing: the 11-03 compute
/// change waited behind the whole sweep, its tests untried 21 h later.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Urgency {
    #[default]
    Deps,
    New,
    Own,
}

/// What changed in a unit since `proof`: the first of its own crate's paths that changed (a
/// package nested in its directory is not its own), else the first changed path it reads, else
/// `Cargo.lock`. A document is the cause only when no code changed: most are no input
/// ([`makepad_ci_merge::inert`]), and one beside code that did made its crate's tests look like its
/// own change (11-05: a CREDITS.md edit named as the cause of 9 units stale by a dependency).
fn stale_path(repo: &Path, pair: &Pair, unit: &Unit, proof: &Proof, closure: Option<&BTreeSet<String>>, cache: &mut Changes) -> (bool, String) {
    let paths = changed_paths(repo, pair, Some(proof), closure, cache);
    let nested: Vec<&String> = closure.into_iter().flatten().filter(|d| **d != unit.dir && under(d, &unit.dir)).collect();
    let own = |p: &String| under(p, &unit.dir) && !nested.iter().any(|n| under(p, n));
    for docs in [false, true] {
        if let Some(p) = paths.iter().find(|p| makepad_ci_merge::doc_file(p) == docs && own(p)) {
            return (true, p.clone());
        }
        if let Some(p) = paths.iter().find(|p| makepad_ci_merge::doc_file(p) == docs) {
            return (false, p.clone());
        }
    }
    (false, "Cargo.lock".into())
}

/// Why a shard no proof keeps runs, and its [`Urgency`].
fn runs_why(repo: &Path, pair: &Pair, tree: &Tree, dg: &Digests, sh: &Shard, cache: &mut Changes) -> Result<(String, Urgency), String> {
    let oses = if sh.os == Os::Any { vec![Os::Linux, Os::Macos] } else { vec![sh.os] };
    let units = units_of(sh);
    if units.is_empty() {
        return Ok(("runs: on every pass".into(), Urgency::Deps));
    }
    let (mut own, mut deps, mut new) = (None, None, false);
    for unit in units {
        let proofs = proofs_at(tree, &unit)?;
        let mine: Vec<(Os, &Proof)> = oses.iter().filter_map(|&os| proofs.get(&(unit.section.clone(), os)).map(|p| (os, p))).collect();
        if mine.iter().any(|(os, p)| p.digest == dg.of(&unit, *os)) {
            continue;
        }
        let Some((_, proof)) = mine.into_iter().max_by(|a, b| a.1.date.cmp(&b.1.date)) else {
            new = true;
            continue;
        };
        let (is_own, path) = stale_path(repo, pair, &unit, proof, dg.closures.get(&unit), cache);
        let line = format!("runs: proof {} stale: {path} changed", proof.date);
        if is_own && own.is_none() {
            own = Some(line);
        } else if deps.is_none() {
            deps = Some(line);
        }
    }
    Ok(match (own, new, deps) {
        (Some(line), _, _) => (line, Urgency::Own),
        (None, true, _) => ("runs: no proof".into(), Urgency::New),
        (None, false, Some(line)) => (line, Urgency::Deps),
        (None, false, None) => ("runs: no proof".into(), Urgency::New),
    })
}

/// An app runs on one OS only; a matching proof on any shipped OS keeps it. All available
/// platforms, including Macs, rotate by oldest proof date, then load per box. The picks, why each
/// runs or is kept, and how soon each that runs goes.
pub fn select(repo: &Path, pair: &Pair, tree: &Tree, digests: &Digests, candidates: Vec<Shard>, boxes_of: &dyn Fn(Os) -> usize, going: &BTreeSet<String>) -> Result<(Vec<Shard>, Vec<(String, String)>, BTreeMap<String, Urgency>), String> {
    let mut changes = Changes::new();
    let mut urgency = BTreeMap::new();
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
        let (why, soon) = match newest(&proofs, &unit.section) {
            Some(proof) => {
                let (own, path) = stale_path(repo, pair, &unit, proof, digests.closures.get(&unit), &mut changes);
                (format!("runs on {}: proof {} stale: {path} changed", pick.os.as_str(), proof.date), if own { Urgency::Own } else { Urgency::Deps })
            }
            None => (format!("runs on {}: no proof", pick.os.as_str()), Urgency::New),
        };
        urgency.insert(pick.id.clone(), soon);
        lines.push((pick.id.clone(), why));
        out.push(pick);
    }
    Ok((out, lines, urgency))
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
    for (target, build, _) in TARGETS {
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
    // the checks a crate's ci.toml names: ignored lib tests, too heavy for its tests above, each
    // its own shard on Linux on every pass (`--ignored --exact`: a check that is gone fails)
    for k in all.iter().filter(|k| crate_runs_on(tree, &k.dir, Os::Linux)) {
        for name in conf_at(tree, &k.dir).checks {
            let id = format!("checks:{}:{}", k.package, name.rsplit("::").next().unwrap_or(&name));
            let with = test_features(tree, &k.dir, default_features(tree, k)).1;
            let mut args: Vec<&str> = vec!["test", &k.package];
            args.extend(with.iter().map(String::as_str));
            args.extend(["--lib", "--", "--ignored", "--exact", &name]);
            let est = timing(&id).unwrap_or(GUESS_CHECK_SECS);
            extra.push(Shard { id, kind: Kind::Test, os: Os::Linux, gui: false, gpu: gpu_dirs.contains(&k.dir), steps: vec![test_step(&k.dir, &args)], judge: Judge::Tests, est, dirs: Some(vec![k.dir.clone()]), whole: false, cores: None });
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
    // a restart keeps the process and the window: a restart-rung edit, a panic, a repeated relaunch; the state kept, no leftovers
    if tree.has("tools/run/fixture/restart_in_place.py") {
        push("live:restart".to_string(), Kind::Screen, Os::Linux, true, vec![argv(&["python3", "tools/run/fixture/restart_in_place.py"])], Judge::Exit, 240.0, None);
    }
    // every static of the JIT's crates is reset at a run's end or stays with a reason: a new one is a
    // decision, or the next run sees the last one's
    if tree.has("tools/run/fixture/restart_statics.py") {
        push("live:restart-statics".to_string(), Kind::Build, Os::Linux, false, vec![argv(&["python3", "tools/run/fixture/restart_statics.py"])], Judge::Exit, 5.0, None);
    }
    // a failed hot load is a warm restart and a ledger report, never a parked panic
    if tree.has("tools/run/fixture/hot_fail.py") {
        push("live:hot-fail".to_string(), Kind::Screen, Os::Linux, false, vec![argv(&["python3", "tools/run/fixture/hot_fail.py"])], Judge::Exit, 60.0, None);
    }
    // the same on Windows, where a restart is another process under one parent
    if tree.has("tools/run/fixture/hot_fail.ps1") {
        push("live:hot-fail-windows".to_string(), Kind::Screen, Os::Windows, false, vec![argv(&["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "tools/run/fixture/hot_fail.ps1"])], Judge::Exit, 180.0, None);
    }
    // Runner on Windows: its list, a live run and Stop, in real windows
    if tree.has("tools/run/fixture/runner_windows.ps1") && tree.has("apps/commercial/runner/Cargo.toml") {
        push("live:runner-windows".to_string(), Kind::Screen, Os::Windows, true, vec![argv(&["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", "tools/run/fixture/runner_windows.ps1"])], Judge::Exit, 240.0, Some(vec!["apps/commercial/runner".to_string()]));
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
        let names = if judge == Judge::Tests { crate::lease::master::failed_names(log) } else { Vec::new() };
        return (false, if names.is_empty() { format!("{step}: exit {exit}") } else { format!("{step}: exit {exit}: {}", names.join(", ")) });
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
    /// a bench box (its quiet windows go to timed work): it lends the run only the cross-target
    /// checks, the shards of its OS no pool box can take (a Mac's Apple targets)
    pub checks_only: bool,
}

impl BoxInfo {
    fn needs_gui(s: &Shard) -> bool {
        s.gui || s.id.starts_with("smoke:")
    }

    pub fn fits(&self, s: &Shard) -> bool {
        (!self.checks_only || s.id.starts_with("check:")) && (s.kind != Kind::Ci || self.ci) && (s.os == Os::Any || s.os == self.os) && (!Self::needs_gui(s) || self.gui) && (s.os != Os::Any || self.os != Os::Windows) && (!s.gpu || self.gpu)
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
    ci_boxes(ctx, true)
}

/// The CI boxes that are down (their caps as last heard): a shard only they could take does not
/// run this round ([`no_box`]).
fn down_boxes(ctx: &Ctx) -> Vec<BoxInfo> {
    ci_boxes(ctx, false)
}

/// Whether `node` is up for CI work now: a worker asks nothing of a box that is down (its asks
/// sat "node down" in the queue and kept the run from ending, 11-04).
fn box_up(ctx: &Ctx, node: &str) -> bool {
    ctx.leases.nodes().iter().any(|n| n.name == node && n.up && n.dev)
}

fn ci_boxes(ctx: &Ctx, up: bool) -> Vec<BoxInfo> {
    ctx.leases
        .nodes()
        .into_iter()
        .filter(|n| n.up == up && n.dev && takes_ci(n))
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
            (os != Os::Any).then(|| BoxInfo { node: n.name.clone(), os, arch: c.arch.clone(), gui, gpu: !c.gpus.is_empty(), ci: crate::lease::sched::slots(&c, Kind::Ci) > 0, checks_only: c.role() == crate::lease::Role::Bench })
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
    /// how soon each shard goes ([`Urgency`])
    urgency: Vec<Urgency>,
    /// shards given the overdue verdict while their step still runs ([`OVERDUE_TIMES`])
    overdue: Vec<bool>,
    /// the run's id: a step that ends after its run did updates that run's record only
    run_id: String,
    sweep: bool,
    /// a sweep's newest leg tree ([`sweep_leg`])
    leg: Option<Arc<LegTree>>,
    /// per box, what its JIT build before its shards did ([`tool_first`])
    tools: Vec<(String, String)>,
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

    /// The pending shard this box runs next: the most urgent ([`Urgency`]), then the longest. None
    /// while the box holds one of the run's shards (running there, or asked for there): one shard
    /// per box, and a shard queued on a busy box holds no other box. (A count of the run's shards
    /// across its boxes, boxes less two for a sweep, ran a sweep one shard at a time on the two CI
    /// boxes: the Linux box idled while a Windows shard waited in its queue, 11-03.)
    fn next_for(&self, b: &BoxInfo) -> Option<usize> {
        if (0..self.shards.len()).any(|i| self.reserved[i].as_deref() == Some(b.node.as_str()) || (self.state[i].state == "run" && self.state[i].node == b.node)) {
            return None;
        }
        // an ask that gave up here goes behind the rest: a whole-box shard on a busy box kept the
        // 31 shards that fit beside lane work waiting for 97 min (11-04)
        let gave_up = |i: usize| self.state[i].spans.iter().filter(|x| x.node == b.node && x.granted_ms == 0).count();
        (0..self.shards.len())
            .filter(|&i| self.state[i].state == "wait" && self.reserved[i].is_none() && self.can(b, i))
            .max_by(|&a, &c| gave_up(c).cmp(&gave_up(a)).then_with(|| (self.urgency[a], self.shards[a].est).partial_cmp(&(self.urgency[c], self.shards[c].est)).unwrap_or(std::cmp::Ordering::Equal)))
    }
}

fn lost_on(s: &CiShard, node: &str) -> u32 {
    s.lost.iter().find(|(n, _)| n == node).map_or(0, |(_, k)| *k)
}

/// Lane work is queued for `node`: a lease that is no CI's names the box (`--node`), or holds it
/// reserved (`hold`). A sweep starts no shard there until that went: a shard holds its box through
/// all its steps (one of 12 steps kept a whole-box drain and an interactive screen lease waiting on
/// the Windows box, 11-04).
fn lane_queued_for(leases: &[crate::lease::sched::Lease], hold: Option<&str>, node: &str) -> bool {
    leases.iter().any(|l| l.state == crate::lease::sched::State::Queued && !l.req.ci() && (l.req.node.as_deref() == Some(node) || hold == Some(l.id.as_str())))
}

/// A shard the run ended without (it waited, or no box could finish it) says so at the end of its
/// log: every shard of a run has its output or a line saying why there is none. A shard still
/// running (a stopped run, an overdue step) writes its own end.
fn did_not_run(st: &CiShard, stopped: bool) -> Result<(), String> {
    let line = match st.state.as_str() {
        "ok" | "fail" | "run" => return Ok(()),
        "wait" if stopped => format!("did not run: the run stopped while it waited ({})", st.summary),
        _ if !st.spans.iter().any(|s| s.granted_ms > 0) => format!("did not run: {}", st.summary),
        _ => format!("did not finish: {}", st.summary),
    };
    let mut f = std::fs::OpenOptions::new().append(true).create(true).open(&st.log).map_err(|e| format!("{}: {e}", st.log))?;
    writeln!(f, "{line}").map_err(|e| format!("{}: {e}", st.log))
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
fn no_box(shard: &Shard, s: &CiShard, here: &[&BoxInfo], down: &[&str], out: &[String], waited: Duration, heard: bool, gone: bool) -> Option<(&'static str, String)> {
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
    // the boxes that could take it are down: it does not run this round (its log says so)
    if !down.is_empty() {
        return (waited >= NO_BOX_WAIT).then(|| ("incomplete", format!("box down: {} (not back in {} min)", down.join(", "), NO_BOX_WAIT.as_secs() / 60)));
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
const SHARED_PATHS: [&str; 4] = ["Cargo.lock", "Cargo.toml", "tools/ci_leg", "tools/run"];
/// The crate whose closure the JIT is.
const JIT_DIR: &str = "tools/run";
/// Not read by any shard through the JIT.
const NOT_JIT: [&str; 2] = ["libs/devhub", "apps/devhub"];

/// The crates the JIT binary is built from: a change to one changes the JIT itself.
pub fn jit_binary_dirs(tree: &Tree) -> BTreeSet<String> {
    let mut dirs = super::roll::crate_dirs_from(JIT_DIR, &|p| tree.read(p));
    dirs.retain(|d| !NOT_JIT.iter().any(|n| under(d, n)));
    dirs
}

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
        // a document is no JIT input unless a source of its crate names it, as for proofs
        // ([`makepad_ci_merge::inert`]); the tree is read only when one changed
        let doc = |p: &String| makepad_ci_merge::doc_file(p);
        let code = changed.iter().any(|p| (read_by_jit(p) || SHARED_PATHS.iter().any(|d| under(p, d))) && !doc(p));
        let jit_changed = code || {
            let docs: Vec<&String> = changed.iter().filter(|p| read_by_jit(p) && doc(p)).collect();
            !docs.is_empty() && Tree::at(repo, to).map_or(true, |tree| {
                let crates: BTreeSet<String> = jit.iter().map(|d| d.trim_end_matches('/').to_string()).collect();
                makepad_ci_merge::inert_by_crate(docs.iter().map(|p| p.as_str()), &crates, false).iter().any(|(krate, files)| files.iter().any(|f| named_in(&tree, krate, f)))
            })
        };
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
/// smoke scripts still run on x64; a script pinned to macOS cannot be moved to Linux. The Apple
/// cross-target checks go by their proofs: a bench Mac lends its quiet time to them only (the
/// user's call, 11-05).
fn arm_plan(tree: &Tree, shards: &mut Vec<Shard>, arm: &super::arm::Decision) -> Vec<Shard> {
    if arm.required { return Vec::new(); }
    let (_, scripts) = discover(tree);
    let mut skipped = Vec::new();
    shards.retain_mut(|s| {
        if s.os == Os::Any { s.os = Os::Linux; }
        if s.os != Os::Macos || s.id.starts_with("check:") { return true; }
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
            let _ = gate_held(one, &c, &run.pair, run.dry, run.sweep, !run.checkout.is_empty(), run.own_tests);
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
    newest_rolling(d).map(|r| (r.started_ms, r.pair.clone()))
}

fn newest_rolling(d: &crate::state::Doc) -> Option<&CiRun> {
    d.ci.run.iter().chain(&d.ci.runs).filter(|r| rolling(r)).max_by_key(|r| r.started_ms)
}

/// A sweep leg's tree when it is not the sweep's own ([`sweep_leg`]).
struct LegTree {
    pair: Pair,
    /// holds the Cargo.lock the leg ships: the one its rolling run shipped
    dir: PathBuf,
    tree: Tree,
}

/// Where a sweep leg runs: the newest rolling run's pair when the sweep's own is older (on both
/// repos an ancestor of it) and that tree still has every crate the leg tests, with the
/// Cargo.lock that run shipped. A box that ran that run has its tree built, so the leg builds
/// nothing new there, and it tests code as new as the rolling runs do. Sweep 140's legs started
/// a median 41 landings (4.6 h) behind work: a Windows leg re-found a hang fixed 73 min before,
/// and each switch of a box's CI tree between the two pairs rebuilt for ~10 min. None: its own.
fn sweep_leg(ctx: &Ctx, own: &Pair, dir: &Path, shard: &Shard, pool: &Mutex<Pool>) -> Option<Arc<LegTree>> {
    let (pair, run_id) = ctx.store.read(|d| newest_rolling(d).map(|r| (r.pair.clone(), r.id.clone())))?;
    if pair == *own {
        return None;
    }
    let known = pool.lock().unwrap().leg.clone().filter(|l| l.pair == pair);
    let leg = match known {
        Some(leg) => leg,
        None => {
            let leg = Arc::new(leg_tree(ctx, own, dir, &pair, &ctx.logs("ci").join(run_id).join("Cargo.lock"))?);
            pool.lock().unwrap().leg = Some(leg.clone());
            leg
        }
    };
    units_of(shard).iter().all(|u| leg.tree.has(&format!("{}/Cargo.toml", u.dir))).then_some(leg)
}

/// The leg tree of `pair` for a sweep at `own` whose logs are `dir`, with `lock` (its rolling
/// run's) kept beside them; None when `pair` does not descend from `own` or the lock is gone.
fn leg_tree(ctx: &Ctx, own: &Pair, dir: &Path, pair: &Pair, lock: &Path) -> Option<LegTree> {
    let descends = crate::gitread::is_ancestor(&ctx.repo, &own.work, &pair.work)
        && (own.main == pair.main || crate::gitread::is_ancestor(&ctx.commercial(), &own.main, &pair.main));
    if !descends {
        return None;
    }
    let at = leg_dir(dir, pair);
    if !at.join("Cargo.lock").exists() {
        std::fs::create_dir_all(&at).ok()?;
        std::fs::copy(lock, at.join("Cargo.lock")).ok()?;
    }
    Some(LegTree { pair: pair.clone(), dir: at, tree: Tree::at(&ctx.repo, pair).ok()? })
}

/// Where a sweep keeps the Cargo.lock of a leg pair.
fn leg_dir(dir: &Path, pair: &Pair) -> PathBuf {
    dir.join(format!("at-{}", pair.short().replace(',', "-")))
}

/// Input digests by leg pair ([`Pair::rev`]); None: they could not be read (its legs prove nothing).
type LegDigests = BTreeMap<String, Option<Digests>>;

/// The input digests of `units` at each pair a leg of `state` ran other than the run's: what a
/// leg's proof names.
fn leg_digests(ctx: &Ctx, run: &Pair, dir: &Path, units: &[Unit], state: &[CiShard], have: &mut LegDigests) {
    for at in state.iter().map(|s| &s.at).filter(|at| !at.work.is_empty() && *at != run) {
        if have.contains_key(&at.rev()) {
            continue;
        }
        let read = Tree::at(&ctx.repo, at).and_then(|tree| {
            let lock = std::fs::read_to_string(leg_dir(dir, at).join("Cargo.lock")).map_err(|e| format!("its Cargo.lock: {e}"))?;
            let here: Vec<Unit> = units.iter().filter(|u| tree.has(&format!("{}/Cargo.toml", u.dir))).cloned().collect();
            Digests::with_lock(&tree, &here, lock)
        });
        if let Err(e) = &read {
            ctx.store.event("ci", Sev::Warn, format!("ci {}: the inputs at {}: {e}; its legs there prove nothing", run.short(), at.short()), None);
        }
        have.insert(at.rev(), read.ok());
    }
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
            return crate::bucket::Outcome { ok: false, failing: Vec::new(), log: String::new(), summary: "CI incomplete: master not started".into(), no_verdict: Some("the master has not started".into()) };
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
                    gate_held(one, &ctx, tip, false, false, false, false)
                }
            }
        };
        let mut outcome = bucket_outcome(verdict, &legs, log);
        let summary = &mut outcome.summary;
        let mut text = format!("CI {}: {summary}\n", tip.short());
        for leg in &legs {
            text.push_str(&format!("{} {}: {} ({})\n", leg.state, leg.name, leg.summary, leg.log));
            for error in &leg.errors { text.push_str(&format!("{error}\n")); }
        }
        if let Err(e) = std::fs::write(log, text) {
            summary.push_str(&format!("; log: {e}"));
        }
        outcome
    })
}

/// The CI bucket's run: [`ci`], then the tests of the crates that changed ([`own_tests_after`]),
/// unless someone stopped it.
pub fn regression(repo: PathBuf) -> crate::bucket::Run {
    let run = ci(repo.clone());
    Arc::new(move |base, tip, log| {
        let outcome = run(base, tip, log);
        if let (Some(ctx), false) = (super::ctx(), run_stop(false).load(std::sync::atomic::Ordering::SeqCst)) {
            own_tests_after(&calling_ctx(ctx, repo.clone()), tip);
        }
        outcome
    })
}

/// A run's verdict and legs as a bucket's: only a leg that failed is a failure. Legs that did not
/// run say so with their reason, and a run with nothing else against it has no verdict (stopped
/// CI runs read FAILED with their unrun legs as the failures: a regression hunt for nothing, 11-04).
fn bucket_outcome(verdict: Verdict, legs: &[crate::state::Leg], log: &Path) -> crate::bucket::Outcome {
    let count = |st: &str| legs.iter().filter(|l| l.state == st).count();
    let (ok, fail, not_run) = (count("ok"), count("fail"), count(crate::state::NOT_RUN));
    let word = match verdict { Verdict::Ok => "ok", Verdict::Fail => "fail", Verdict::Incomplete => "incomplete" };
    let summary = format!("{} shards: {ok} ok, {fail} fail, {} incomplete, {not_run} not run; {word}", legs.len(), legs.len() - ok - fail - not_run);
    let failing: Vec<String> = legs.iter().filter(|l| l.state == "fail").map(|l| l.name.clone()).collect();
    let no_verdict = (verdict != Verdict::Ok && failing.is_empty()).then(|| match legs.iter().find(|l| l.state != "ok") {
        Some(l) => format!("{} of {} shards have no verdict, {}: {}", legs.len() - ok, legs.len(), l.name, l.summary),
        None => "the run ended before its shards: see ci events".to_string(),
    });
    crate::bucket::Outcome { ok: verdict == Verdict::Ok, failing, log: log.display().to_string(), summary, no_verdict }
}

fn gate_for(ctx: &Arc<Ctx>, pair: &Pair, dry: bool, sweep: bool, calling: bool) -> (Verdict, Vec<crate::state::Leg>) {
    let one = run_slot(sweep).lock().unwrap_or_else(|e| e.into_inner());
    run_stop(sweep).store(false, std::sync::atomic::Ordering::SeqCst);
    gate_held(one, ctx, pair, dry, sweep, calling, false)
}

/// What the own-units run goes for next, and whether its thread is up.
struct OwnTests {
    tip: Option<Pair>,
    going: bool,
}
static OWN_TESTS: Mutex<OwnTests> = Mutex::new(OwnTests { tip: None, going: false });

/// A regression run ended at `tip`: the crates whose own files changed get their tests, in the
/// sweep's slot. Crate tests ran only in sweeps, so a landing that broke one went unseen until the
/// next sweep (none of regression runs 141 to 147 tested a crate). One at a time: the newest tip
/// wins, and the tips a run did not get to are skipped, so runs never pile up; a sweep goes first
/// (it runs these first anyway).
fn own_tests_after(ctx: &Arc<Ctx>, tip: &Pair) {
    let mut own = OWN_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    own.tip = Some(tip.clone());
    if std::mem::replace(&mut own.going, true) {
        return;
    }
    drop(own);
    let ctx = ctx.clone();
    let spawned = std::thread::Builder::new().name("devhub-ci-own".into()).spawn(move || loop {
        let one = run_slot(true).lock().unwrap_or_else(|e| e.into_inner());
        let tip = {
            let mut own = OWN_TESTS.lock().unwrap_or_else(|e| e.into_inner());
            let tip = own.tip.take();
            own.going = tip.is_some();
            tip
        };
        let Some(tip) = tip else { return };
        run_stop(true).store(false, std::sync::atomic::Ordering::SeqCst);
        let _ = gate_held(one, &ctx, &tip, true, true, false, true);
    });
    if spawned.is_err() {
        OWN_TESTS.lock().unwrap_or_else(|e| e.into_inner()).going = false;
    }
}

/// The units a run of [`own_tests_after`] takes, of what the plan's digests left to run: its own
/// crate's files changed since its proof ([`Urgency::Own`]) or it has no proof at all
/// ([`Urgency::New`]: a new crate, or one whose proof a merge lost, which no later change names), a
/// box of the run can take it, and the same inputs did not fail already (they would fail again
/// every run, until their crate changes).
fn own_tests_picked(shards: Vec<Shard>, urgency: &BTreeMap<String, Urgency>, fits: &dyn Fn(&Shard) -> bool, failed_as_is: &dyn Fn(&Shard) -> bool) -> Vec<Shard> {
    shards.into_iter().filter(|s| matches!(urgency.get(&s.id), Some(Urgency::Own | Urgency::New)) && fits(s) && !failed_as_is(s)).collect()
}

/// The inputs a unit of [`own_tests_after`] last failed with. A master that starts over reports
/// such a failure once more.
static OWN_FAILED: LazyLock<Mutex<BTreeMap<Unit, String>>> = LazyLock::new(Default::default);

/// Keep a matching run in its slot while planning; archive only that slot's superseded run.
fn resume_run(ci: &mut Ci, pair: &Pair, dry: bool, sweep: bool, own: bool, lock_digest: &str, checkout: &str) -> Option<CiRun> {
    if let Some(r) = ci.slot(sweep).as_ref().filter(|r| r.lock_digest == lock_digest && r.pair == *pair && r.dry == dry && r.sweep == sweep && r.own_tests == own && Path::new(&r.checkout) == Path::new(checkout)) {
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
fn gate_held(_one: std::sync::MutexGuard<'static, ()>, ctx: &Arc<Ctx>, pair: &Pair, dry: bool, sweep: bool, calling: bool, own: bool) -> (Verdict, Vec<crate::state::Leg>) {
    let checkout = if calling { ctx.repo.to_string_lossy().into_owned() } else { String::new() };
    let prior_lock = ctx.store.read(|d| d.ci.slot(sweep).as_ref().filter(|r| r.pair == *pair && r.dry == dry && r.sweep == sweep && r.own_tests == own && Path::new(&r.checkout) == Path::new(&checkout)).map(|r| ctx.logs("ci").join(&r.id).join("Cargo.lock")));
    let lock = prior_lock.and_then(|p| std::fs::read_to_string(p).ok()).or_else(|| std::fs::read_to_string(ctx.repo.join("Cargo.lock")).ok());
    let Some(lock) = lock else {
        ctx.store.event("ci", Sev::Err, "Cargo.lock is missing; no CI proof can be computed".into(), None);
        return (Verdict::Incomplete, Vec::new());
    };
    let lock_digest = hex_digest(lock.as_bytes());
    let going = ctx.store.read(|d| d.ci.slot(sweep).as_ref().filter(|r| r.lock_digest == lock_digest && r.pair == *pair && r.dry == dry && r.sweep == sweep && r.own_tests == own && Path::new(&r.checkout) == Path::new(&checkout)).cloned());
    if going.is_none() {
        if let Err(e) = ctx.store.update(|d| resume_run(&mut d.ci, pair, dry, sweep, own, &lock_digest, &checkout)) {
            ctx.store.event("ci", Sev::Err, e, None);
            return (Verdict::Incomplete, Vec::new());
        }
    }
    let rotation = match &going {
        Some(r) => r.rotation,
        // (the rotation is the smoke scripts': an own-units run has none, and leaves it where it is)
        None if own => ctx.store.read(|d| d.ci.rotation),
        None => match ctx.store.update(|d| { d.ci.rotation += 1; d.ci.rotation }) {
            Ok(rotation) => rotation,
            Err(e) => { ctx.store.event("ci", Sev::Err, e, None); return (Verdict::Incomplete, Vec::new()); }
        },
    };
    let id = going.as_ref().map_or_else(|| format!("{}-{rotation}{}", stamp(wall_ms()), if own { "-own" } else { "" }), |r| r.id.clone());
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
    let mut shards = if own { Vec::new() } else if sweep { sweep_plan(&tree, &timing_of, rotation) } else { plan(&tree, &timing_of, rotation) };
    if !sweep && arm.full {
        shards.extend(sweep_plan(&tree, &timing_of, rotation).into_iter().filter(|s| s.id.starts_with("mjit:")));
    }
    if sweep || calling {
        // Whole crate proofs need every test target, independently of sweep rotation.
        shards.retain(|s| !s.id.starts_with("test:") && !s.id.starts_with("wtest:"));
        shards.extend(test_shards(&tree, &timing_of));
    }
    if own {
        shards.retain(|s| s.id.starts_with("test:") && matches!(s.os, Os::Linux | Os::Any));
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
    let mut urgency: BTreeMap<String, Urgency> = BTreeMap::new();
    let classified = arm_plan(&tree, &mut shards, &arm);
    ctx.store.event("ci", Sev::Info, format!("ci {}: {}", pair.short(), arm.summary), None);
    let _ = std::fs::write(dir.join("arm64.log"), format!("{}\n", arm.summary));
    if !sweep {
        let bs = boxes(ctx);
        let was: BTreeSet<String> = going.as_ref().map(|r| r.shards.iter().map(|s| s.id.clone()).collect()).unwrap_or_default();
        let (apps, others): (Vec<Shard>, Vec<Shard>) = shards.into_iter().partition(|s| s.id.starts_with("smoke:"));
        let (macs, apps): (Vec<Shard>, Vec<Shard>) = apps.into_iter().partition(|s| arm.full && s.os == Os::Macos);
        match select(&ctx.repo, pair, &tree, &digests, apps, &|os| bs.iter().filter(|b| b.os == os).count(), &was) {
            Ok((picked, lines, soon)) => { shards = picked; why_lines = lines; urgency = soon; }
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
    // why the rest run, and how soon ([`Urgency`])
    let mut changes = Changes::new();
    for sh in &shards {
        if why_lines.iter().any(|(id, _)| id == &sh.id) { continue; }
        match runs_why(&ctx.repo, pair, &tree, &digests, sh, &mut changes) {
            Ok((why, soon)) => {
                why_lines.push((sh.id.clone(), why));
                urgency.insert(sh.id.clone(), soon);
            }
            Err(e) => {
                ctx.store.event("ci", Sev::Err, e, None);
                return (Verdict::Incomplete, Vec::new());
            }
        }
    }
    // what an own-units run picked stays in its plan's marks: a run that goes on keeps its batches
    let mut own_marks = Vec::new();
    if own {
        if going.is_some() {
            shards.retain(|s| timing_of(&format!("own:{}", s.id)).is_some());
        } else {
            let bs = boxes(ctx);
            let failed = OWN_FAILED.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let failed_as_is = |s: &Shard| { let units = units_of(s); !units.is_empty() && units.iter().all(|u| failed.get(u).is_some_and(|d| d == digests.of(u, Os::Linux))) };
            shards = own_tests_picked(shards, &urgency, &|s| bs.iter().any(|b| b.fits(s)), &failed_as_is);
            own_marks = shards.iter().map(|s| (format!("own:{}", s.id), 0.0)).collect();
            let names: Vec<&str> = shards.iter().take(8).filter_map(|s| s.id.strip_prefix("test:")).collect();
            ctx.store.event("ci", Sev::Info, format!("own tests {}: {} of {} units{}{}", pair.short(), shards.len(), units.len(), if names.is_empty() { "" } else { ": " }, names.join(" ")), None);
            if shards.is_empty() {
                let _ = std::fs::remove_dir_all(&dir);
                return (Verdict::Ok, Vec::new());
            }
        }
    }
    if sweep || calling {
        shards = batch_tests(shards, &mut why_lines, &mut urgency, &timing_of);
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
    // every shard's log says from the plan on why it runs or is kept (its run's output replaces
    // the line); a run that ends without running it adds why ([`did_not_run`])
    let unwritten: Vec<String> = state.iter().filter(|st| !Path::new(&st.log).exists()).filter_map(|st| {
        let line = if st.kept.is_empty() { format!("waits: {}", st.summary) } else { st.kept.clone() };
        std::fs::write(&st.log, format!("{line}\n")).err().map(|e| format!("{}: {e}", st.log))
    }).collect();
    if let Some(e) = unwritten.first() {
        ctx.store.event("ci", Sev::Warn, format!("ci {}: {} shard logs could not be written ({e})", pair.short(), unwritten.len()), None);
    }
    let n = shards.len();
    let started = going.as_ref().map_or_else(wall_ms, |r| r.started_ms);
    ctx.store.update(|d| *d.ci.slot_mut(sweep) = Some(CiRun { lock_digest, checkout, id: id.clone(), pair: pair.clone(), dry, started_ms: started, verdict: "running".into(), shards: state.clone(), rotation, plan_timings: timings.iter().cloned().chain(own_marks).collect(), sweep, own_tests: own, stopping: going.as_ref().is_some_and(|r| r.stopping), ..Default::default() })).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
    for l in stray {
        let _ = crate::cli::ask(&makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-ci"), None, "release", &l);
    }
    let word = if going.is_some() { "goes on" } else { "started" };
    ctx.store.event("ci", Sev::Info, format!("ci {} {word}: {n} shards, {kept} kept, {} on their boxes (run {id})", pair.short(), back.len()), None);
    let urgency = shards.iter().map(|s| urgency.get(&s.id).copied().unwrap_or_default()).collect();
    let pool = Arc::new((Mutex::new(Pool { tools: Vec::new(), proofs_collected: false, reserved: vec![None; n], shards, state, stopped: false, prio: PRIO, urgency, overdue: vec![false; n], run_id: id.clone(), sweep, leg: None }), Condvar::new()));
    let master_t0 = Instant::now();
    let mut nothing_left: Option<Instant> = None;
    let mut last_ran = Instant::now();
    // why the run ended with shards left (none asked for or none here: it ended by itself)
    let mut stop_why = String::new();
    // the alert of a run that goes nowhere: no shard ended for this long, whatever still "runs"
    let (mut ended, mut last_end) = (0usize, Instant::now());
    // passes are committed as proofs while the run goes (a failure elsewhere, a stop or a master
    // restart never throws them away); the run's end commits the rest
    let (mut published, mut last_batch): (BTreeSet<(Unit, Os)>, Instant) = (BTreeSet::new(), Instant::now());
    let mut leg_dg = LegDigests::new();
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
                stop_why = "the run was stopped".into();
                g.stopped = true;
            }
            if g.all_done() || g.stopped {
                cv.notify_all();
                break;
            }
            // a shard no box can take waits while anything else runs, and then for a box to come
            // back (a master just started has not heard its boxes yet: that is no box gone)
            let bs = boxes(ctx);
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
                let stall = format!("stalled, no shard got a box in {} min", STALLED.as_secs() / 60);
                ctx.store.event("ci", Sev::Err, format!("ci {}: {stall}: {}", pair.short(), asks.join("; ")), None);
                stop_why = format!("the run {stall}");
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
                let downs = down_boxes(ctx);
                for i in stuck {
                    let here: Vec<&BoxInfo> = bs.iter().filter(|b| b.fits(&g.shards[i])).collect();
                    let down: Vec<&str> = downs.iter().filter(|b| b.fits(&g.shards[i])).map(|b| b.node.as_str()).collect();
                    match no_box(&g.shards[i], &g.state[i], &here, &down, &out, waited, master_t0.elapsed() >= HEAR, gone) {
                        Some((state, summary)) => {
                            g.state[i].state = state.into();
                            g.state[i].summary = summary;
                        }
                        None if here.is_empty() && !down.is_empty() => g.state[i].summary = format!("waits for {} to come back (box down, {} s)", down.join(", "), waited.as_secs()),
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
            let (shards, state) = (g.shards.clone(), g.state.clone());
            drop(g);
            leg_digests(ctx, pair, &dir, &units, &state, &mut leg_dg);
            let fresh: Updates = passed_units(ctx, pair, &digests, &leg_dg, &shards, &state, wall_ms()).into_iter().filter(|(k, _)| !published.contains(k)).collect();
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
    // an own-units failure goes to the inbox with its crate and its tests (the step's name and
    // [`judge`]'s line); its inputs do not run again until they change
    if own {
        for st in g.state.iter().filter(|s| s.state == "fail") {
            ctx.store.event("ci", Sev::Err, format!("own tests FAILED at {} on {} ({}): {}", if st.at.work.is_empty() { pair } else { &st.at }.short(), st.node, st.log, st.summary), None);
        }
        let mut failed = OWN_FAILED.lock().unwrap_or_else(|e| e.into_inner());
        for (sh, st) in g.shards.iter().zip(&g.state) {
            for (k, unit) in units_of(sh).into_iter().enumerate() {
                if unit_passed(sh, st, k) {
                    failed.remove(&unit);
                } else if st.state == "fail" {
                    let digest = digests.of(&unit, Os::Linux).to_string();
                    failed.insert(unit, digest);
                }
            }
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
    let (crit, mut boxes, fleet) = critical(&g.state, started, ended);
    for (node, line) in &g.tools {
        match boxes.iter_mut().find(|b| b.split(' ').next() == Some(node.as_str())) {
            Some(b) => *b = format!("{b}; {line}"),
            None => boxes.push(format!("{node} 0 shards; {line}")),
        }
    }
    let slow = slowest(&g.state, 5);
    let failed: Vec<String> = g.state.iter().filter(|s| s.state == "fail").map(|s| s.id.clone()).collect();
    let mut state = g.state.clone();
    let unwritten: Vec<String> = state.iter().filter_map(|st| did_not_run(st, g.stopped).err()).collect();
    if let Some(e) = unwritten.first() {
        ctx.store.event("ci", Sev::Warn, format!("ci {}: {} shard logs could not say why they did not run ({e})", pair.short(), unwritten.len()), None);
    }
    for (id, why) in &why_lines {
        // (one that waited has it as its summary since the start)
        if let Some(st) = state.iter_mut().find(|st| &st.id == id && st.kept.is_empty() && !st.summary.starts_with(why.as_str())) {
            st.summary = format!("{why}; {}", st.summary);
        }
    }
    // what the run ended without: neither passed nor failed, with why
    for st in state.iter_mut().filter(|s| s.state == "wait") {
        st.state = crate::state::NOT_RUN.into();
        if !stop_why.is_empty() { st.summary = format!("{stop_why}; {}", st.summary); }
    }
    let not_run = state.iter().filter(|s| s.state == crate::state::NOT_RUN).count();
    let one_box = one_box_failures(&state);
    let legs = state.iter().map(CiShard::leg).collect();
    // a sweep's legs follow the rolling runs ([`sweep_leg`]): the newest pair one ran
    let upto = state.iter().filter(|s| !s.at.work.is_empty() && s.at != *pair).max_by_key(|s| s.granted_ms).map(|s| format!(" (legs up to {})", s.at.short())).unwrap_or_default();
    leg_digests(ctx, pair, &dir, &units, &state, &mut leg_dg);
    let updates: Updates = passed_units(ctx, pair, &digests, &leg_dg, &g.shards, &state, ended).into_iter().filter(|(k, _)| !published.contains(k)).collect();
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
            r.slow = slow.clone();
            d.ci.runs.push(r);
        }
    }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
    let line = format!(
        "ci {}{upto} {word}: {n} shards, {fleet}{}{}{}; critical path {crit}; slowest {}; {}",
        pair.short(),
        if failed.is_empty() { String::new() } else { format!("; failed {}", failed.join(" ")) },
        if not_run == 0 { String::new() } else { format!("; {not_run} did not run") },
        if one_box.is_empty() { String::new() } else { format!("; FAILED ON ONE BOX ONLY: {}", one_box.join("; ")) },
        slow.iter().map(|l| l.split(':').next().unwrap_or(l)).collect::<Vec<_>>().join(", "),
        arm.summary
    );
    // (an own-units run's failures have their lines above)
    ctx.store.event("ci", if (verdict == Verdict::Fail && !own) || !one_box.is_empty() { Sev::Err } else { Sev::Info }, line, None);
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
    let sweep = p.lock().unwrap().sweep;
    tool_first(ctx, pool, pair, dir, &b);
    loop {
        let i = loop {
            // lane work goes first between a sweep's shards
            let lanes_first = sweep && lane_queued_for(&ctx.leases.view(), ctx.leases.nodes().iter().find(|n| n.name == b.node).and_then(|n| n.hold.as_deref()), &b.node);
            let mut g = p.lock().unwrap();
            if g.all_done() || g.stopped {
                return;
            }
            if !lanes_first && idle_until.is_none_or(|t| Instant::now() >= t) && box_up(ctx, &b.node) {
                if let Some(i) = g.next_for(&b) {
                    g.reserved[i] = Some(b.node.clone());
                    break i;
                }
            }
            drop(cv.wait_timeout(g, Duration::from_secs(5)).unwrap());
        };
        let (shard, prio, sweep) = {
            let g = p.lock().unwrap();
            (g.shards[i].clone(), g.prio, g.sweep)
        };
        let leg = if sweep { sweep_leg(ctx, pair, dir, &shard, p) } else { None };
        let (at, lock_dir) = leg.as_ref().map_or_else(|| (pair.clone(), dir.to_path_buf()), |l| (l.pair.clone(), l.dir.clone()));
        let spec = Spec {
            since_ms: None,
            // each box asks for itself
            lane: LANE.into(),
            agent: if sweep { SWEEP_AGENT } else { AGENT }.into(),
            kind: shard.kind,
            needs: b.needs(&shard),
            prio,
            rev: at.rev(),
            repo_dir: ctx.repo.to_string_lossy().into_owned(),
            node: Some(b.node.clone()),
        };
        let asked = Instant::now();
        let asked_ms = wall_ms();
        let down = std::cell::Cell::new(false);
        let stop = || {
            down.set(!box_up(ctx, &b.node));
            asked.elapsed() > QUEUE_WAIT || down.get() || p.lock().map(|g| g.stopped).unwrap_or(true)
        };
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
                // its box went down: the ask goes, and the box is asked nothing until it is back
                if down.get() {
                    g.state[i].summary = format!("box down: {}", b.node);
                }
                idle_until = (!e.ends_with("given up")).then(|| Instant::now() + BACKOFF);
                cv.notify_all();
                continue;
            }
        };
        if let Err(e) = ship_lock(&held, &lock_dir, false) {
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
            s.at = at;
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

/// Builds the JIT the box's shards run through (`cargo run` of makepad-run at the run's pair, kept
/// by content on the box) before its first shard, a stage of its own: a box's first shard paid
/// for that build and was blamed for it (run 153's route smoke on Windows: 800 s, 31 s of it the
/// smoke). A box that keeps that binary already answers in a second or two.
fn tool_first(ctx: &Arc<Ctx>, pool: &Arc<(Mutex<Pool>, Condvar)>, pair: &Pair, dir: &Path, b: &BoxInfo) {
    let (p, _) = &**pool;
    let (prio, sweep) = {
        let g = p.lock().unwrap();
        if !(0..g.shards.len()).any(|i| !g.finished(i) && g.can(b, i) && g.shards[i].steps.iter().flatten().any(|w| w == "makepad-run")) {
            return;
        }
        (g.prio, g.sweep)
    };
    let spec = Spec {
        since_ms: None,
        lane: LANE.into(),
        agent: if sweep { SWEEP_AGENT } else { AGENT }.into(),
        kind: Kind::Ci,
        needs: format!("os={},arch={}", b.os.as_str(), b.arch),
        prio,
        rev: pair.rev(),
        repo_dir: ctx.repo.to_string_lossy().into_owned(),
        node: Some(b.node.clone()),
    };
    let asked = Instant::now();
    let stop = || asked.elapsed() > QUEUE_WAIT || !box_up(ctx, &b.node) || p.lock().map(|g| g.stopped).unwrap_or(true);
    let line = match Held::acquire_with(&spec, &|_| {}, &mut |_| {}, &stop) {
        Err(e) => format!("its JIT before its shards: no lease ({e}); its first shard builds it"),
        Ok(held) => {
            let started = Instant::now();
            let mut out = Vec::new();
            let r = ship_lock(&held, dir, false).and_then(|_| held.run_asked("jit", ".", &jit(&["--version"]), &mut out, &mut |_| {}));
            held.release(Some(if matches!(r, Ok(0)) { 0 } else { 1 }));
            let _ = std::fs::write(dir.join(format!("jit_{}.log", safe(&b.node))), &out);
            let how = match r {
                Ok(0) => tool_how(&String::from_utf8_lossy(&out)),
                Ok(code) => format!("failed (exit {code}): its first shard builds it"),
                Err(e) => format!("failed ({e}): its first shard builds it"),
            };
            format!("its JIT before its shards: {how} in {:.0} s", started.elapsed().as_secs_f64())
        }
    };
    p.lock().unwrap().tools.push((b.node.clone(), line));
}

/// What a JIT run's log says the lease did for its binary ([`tool_first`]).
fn tool_how(log: &str) -> String {
    if log.lines().any(|l| l.starts_with("devhub: kept the JIT binary")) {
        return "built".into();
    }
    match log.lines().find_map(|l| l.strip_prefix("devhub: this JIT binary is not kept: ")) {
        Some(why) => format!("built, not kept ({why})"),
        None => "the box had it".into(),
    }
}

/// The run's `n` slowest shards on their boxes and what their time went to ([`time_went`]).
fn slowest(state: &[CiShard], n: usize) -> Vec<String> {
    let held = |s: &CiShard| s.spans.iter().filter(|x| x.granted_ms > 0).map(|x| x.ended_ms.saturating_sub(x.granted_ms)).sum::<u64>() as f64 / 1000.0;
    let mut by: Vec<(f64, &CiShard)> = state.iter().map(|s| (held(s), s)).filter(|(t, _)| *t > 0.0).collect();
    by.sort_by(|a, b| b.0.total_cmp(&a.0));
    by.into_iter().take(n).map(|(t, s)| format!("{} {t:.0} s on {}: {}", s.id, s.node, time_went(&std::fs::read_to_string(&s.log).unwrap_or_default(), t))).collect()
}

/// What a shard's log says its time went to: its steps (ci-leg's `STEP` lines, each test binary's
/// `finished in`), the longest of them, and the rest, outside them (the lease, compiling, a JIT
/// build, which it names).
fn time_went(log: &str, held: f64) -> String {
    let mut steps: Vec<(String, f64)> = Vec::new();
    let (mut package, mut binary) = ("", "");
    for line in log.lines() {
        if let Some(cmd) = line.strip_prefix("$ ") {
            let mut words = cmd.split_whitespace().skip_while(|w| *w != "test");
            package = words.nth(1).unwrap_or("");
        } else if let Some(b) = line.trim_start().strip_prefix("Running ") {
            binary = b.split(" (").next().unwrap_or(b);
        } else if let Some(rest) = line.strip_prefix("STEP ").and_then(|r| r.split_once(' ')).filter(|(state, _)| *state != "running").map(|(_, r)| r) {
            if let Some((name, secs)) = rest.strip_suffix(" s)").and_then(|r| r.rsplit_once(" (")) {
                if let Ok(secs) = secs.parse::<f64>() {
                    steps.push((name.to_string(), secs));
                }
            }
        } else if let Some(secs) = line.strip_prefix("test result: ").and_then(|r| r.rsplit_once("finished in ")).and_then(|(_, t)| t.trim().trim_end_matches('s').parse::<f64>().ok()) {
            steps.push((format!("{package} {binary}"), secs));
        }
    }
    let inside: f64 = steps.iter().map(|(_, s)| s).sum();
    let built = log.lines().any(|l| l.starts_with("devhub: kept the JIT binary") || l.starts_with("devhub: this JIT binary is not kept"));
    let built = if built { " (it built the JIT)" } else { "" };
    match steps.iter().max_by(|a, b| a.1.total_cmp(&b.1)) {
        Some((name, secs)) => format!("{inside:.0} s in {} steps (longest {name} {secs:.1} s), {:.0} s outside them{built}", steps.len(), (held - inside).max(0.0)),
        None => format!("no step times in its log{built}"),
    }
}

/// A shard on its lease, from the step its state names, to its result; whether lane work took the
/// box back (the box rests a while: the shard waits again).
pub(crate) fn ship_lock(held: &Held, dir: &Path, overlay: bool) -> Result<(), String> {
    let mut entries = Vec::new();
    crate::lease::ship::entries(dir, "Cargo.lock", &mut entries)?;
    crate::lease::ship::send(held.client(), &held.node, &held.id, dir, &entries, false)?;
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
    let grabs = if state == "ok" && saves_grabs(&shard) {
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
                let dir = ctx.logs("ci").join(&run_id);
                let mut legs = LegDigests::new();
                leg_digests(ctx, &run.pair, &dir, &wanted, &states, &mut legs);
                let result = Tree::at(&ctx.repo, &run.pair).and_then(|tree| Digests::at(&dir, &tree, &wanted)).and_then(|dg| {
                    let mut updates = passed_units(ctx, &run.pair, &dg, &legs, &shards, &states, wall_ms());
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

/// A smoke's zero exit covers its script's flow; the acceptances it saved still need the judge, in
/// the gate's shards and the sweep's alike.
fn saves_grabs(shard: &Shard) -> bool {
    shard.steps.iter().any(|step| step.iter().any(|word| word == "smoke"))
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

/// Why a judged grab is no pass: the questions answered otherwise than expected; a reply that
/// cannot be read, with the reply.
fn failure(v: &makepad_ci_judge::Verdict) -> String {
    match (v.parse_error, v.why.is_empty()) {
        (true, _) => format!("vision parse failed: {}; the reply:\n{}", v.why, v.raw),
        (false, true) => v.raw.clone(),
        (false, false) => v.why.clone(),
    }
}

/// `ci judge <dir>...`: each saved grab `NN.png` of the dirs judged again against its `NN.txt`
/// (a CI run's `deferred` dir, or a copy with the acceptance texts rewritten); nothing is written.
fn judge_saved(dirs: &[&str]) -> Result<String, String> {
    if dirs.is_empty() {
        return Err("ci judge <dir>...: judges each NN.png there against its NN.txt".into());
    }
    let judge = makepad_ci_judge::Judge::start();
    if let Some(why) = judge.unreachable() {
        return Err(why);
    }
    let mut rows = Vec::new();
    for dir in dirs {
        let mut grabs: Vec<PathBuf> = std::fs::read_dir(dir).map_err(|e| format!("{dir}: {e}"))?.filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|x| x == "png")).collect();
        grabs.sort();
        for image in grabs {
            let text = std::fs::read_to_string(image.with_extension("txt")).map_err(|e| format!("{}: {e}", image.with_extension("txt").display()))?;
            let grab = s(image.display().to_string());
            rows.push(match judge.judge(&image, &text) {
                Ok(v) => obj(vec![("grab", grab), ("yes", Value::Bool(v.yes)), ("why", s(if v.yes { String::new() } else { failure(&v) })), ("raw", s(&v.raw)), ("hub", s(&v.hub))]),
                Err(e) => obj(vec![("grab", grab), ("err", s(e))]),
            });
        }
    }
    Ok(obj(vec![("judged", Value::Arr(rows))]).to_json())
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
                    failed.push(format!("{}: {}", grab.step, failure(&verdict)));
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
            held.run_asked(&format!("step-{k}"), ".", step, &mut out, &mut |n| note(k, n, &failed, took))
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
            // a result, not a lost box: run again, it hangs again
            Err(e) if e.ends_with(" ended (hung)") => {
                let why = format!("{}: hung, every process idle (the hub's inbox has its stacks: `{} stalled`)", step_name(step), e.split(' ').next().unwrap_or(""));
                let _ = writeln!(f, "== hung: {e}");
                took.push((k, 0.0, false));
                failed.push(why);
                note(k + 1, 0, &failed, took);
                return ("fail", failed.join("; "));
            }
            // a result too: it took its box's disk, and would take the next one's
            Err(e) if e.contains(" ended (disk): ") => {
                let _ = writeln!(f, "== ended: {e}");
                took.push((k, 0.0, false));
                failed.push(format!("{}: {e}", step_name(step)));
                note(k + 1, 0, &failed, took);
                return ("fail", failed.join("; "));
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

/// A crate's tests pass only when every planned target of it passed on that OS, all with the same
/// inputs: a proof names the pair its legs ran (a sweep leg's own, [`sweep_leg`]); targets that
/// ran at different inputs prove none of them.
fn passed_units(ctx: &Ctx, pair: &Pair, dg: &Digests, legs: &LegDigests, shards: &[Shard], state: &[CiShard], at_ms: u64) -> Updates {
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
        // each target's inputs: at its run's pair (a kept result too) or its leg's
        let mut ran: Vec<(u64, &Pair, &str)> = Vec::new();
        for (s, _) in &states {
            let at = if s.at.work.is_empty() { pair } else { &s.at };
            let digests = if at == pair { Some(dg) } else { legs.get(&at.rev()).and_then(Option::as_ref) };
            ran.push((s.granted_ms, at, digests.map(|d| d.of(&unit, os)).filter(|d| !d.is_empty())?));
        }
        if ran.iter().any(|(_, _, d)| *d != ran[0].2) {
            return None;
        }
        let (_, at, digest) = ran.into_iter().max_by_key(|(granted, _, _)| *granted)?;
        Some(((unit.clone(), os), Proof { digest: digest.to_string(), pair: at.clone(), capture: String::new(), date: date(at_ms) }))
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
    // (a sweep's legs ran on the pairs of the rolling runs: each one its proofs name)
    let pairs: BTreeSet<String> = updates.values().map(|p| p.pair.short()).collect();
    let message = format!("ci proofs: {} units at {}", updates.len(), pairs.into_iter().collect::<Vec<_>>().join(" "));
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
        super::git(&repo, &["-c", &format!("user.name={who}"), "-c", &format!("user.email={email}"), "-c", "commit.gpgsign=false", "commit", "--author", &format!("{who} <{email}>"), "-m", &message])?;
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

/// The crates' test shards that run, batched by OS and GPU into shards of about [`GROUP_SECS`]; a
/// batch is as urgent as its most urgent crate.
fn batch_tests(shards: Vec<Shard>, lines: &mut Vec<(String, String)>, urgency: &mut BTreeMap<String, Urgency>, timing: &dyn Fn(&str) -> Option<f64>) -> Vec<Shard> {
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
            let reasons: Vec<String> = members.iter().map(|s| format!("{}: {}", s.id.split_once(':').unwrap().1, why.get(&s.id).map_or("no proof", |w| w.strip_prefix("runs: ").unwrap_or(w)))).collect();
            lines.push((sh.id.clone(), format!("runs: {}", reasons.join("; "))));
            let soon = members.iter().filter_map(|s| urgency.get(&s.id).copied()).max().unwrap_or_default();
            urgency.insert(sh.id.clone(), soon);
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
    let (picked, lines, _) = select(repo, &pair, &tree, &dg, apps, &|_| 1, &BTreeSet::new())?;
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
            // its own OS's proof says what changed since: another's is newer (a Windows unit read
            // "Cargo.lock changed" since the Linux proof of its crate, 11-05)
            let why = match proofs.get(&(unit.section.clone(), sh.os)).or_else(|| newest(&proofs, &unit.section)) {
                Some(proof) => format!("proof {} stale: {} changed", proof.date, stale_path(repo, &pair, &unit, proof, dg.closures.get(&unit), &mut changes).1),
                None => "no proof".to_string(),
            };
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
        let batched = batch_tests(test_shards(&old, &timings), &mut kept, &mut BTreeMap::new(), &timings);
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
  stop [--sweep]                         stop the gate (or sweep); running shards finish
  judge <dir>...                         judge each saved grab NN.png there again against its NN.txt";

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
        Some("judge") => judge_saved(&w[1..]),
        Some("sweep") => {
            let rest: Vec<&str> = w[1..].iter().copied().filter(|x| *x != "--dry").collect();
            let pair = pair_of(ctx, &rest)?;
            let c = ctx.clone();
            let p = pair.clone();
            let (held, slot) = std::sync::mpsc::channel();
            let _ = std::thread::Builder::new().name("devhub-ci-sweep".into()).spawn(move || {
                // a sweep goes before own-units tests: they stop (what runs finishes) and it starts
                let own_going = c.store.read(|d| d.ci.sweep_run.as_ref().is_some_and(|r| r.own_tests));
                let one = if own_going {
                    OWN_TESTS.lock().unwrap_or_else(|e| e.into_inner()).tip = None;
                    SWEEP_STOP.store(true, std::sync::atomic::Ordering::SeqCst);
                    SWEEP.lock().unwrap_or_else(|e| e.into_inner())
                } else {
                    match SWEEP.try_lock() {
                        Ok(one) => one,
                        Err(std::sync::TryLockError::Poisoned(e)) => e.into_inner(),
                        Err(std::sync::TryLockError::WouldBlock) => { let _ = held.send(false); return; }
                    }
                };
                SWEEP_STOP.store(false, std::sync::atomic::Ordering::SeqCst);
                let _ = held.send(true);
                let _ = gate_held(one, &c, &p, true, true, false, false);
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
        Some(x) => Err(format!("ci: unknown word {x} (ci status | run | seed | sweep | plan | stop | judge)")),
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
    let pair_of = |p: &Value| crate::state::Pair::new(p.get("work").and_then(|x| x.as_str()).unwrap_or(""), p.get("main").and_then(|x| x.as_str()).unwrap_or(""));
    let run_pair = v.get("pair").map(pair_of).unwrap_or_default();
    let mut pair = run_pair.short();
    // a sweep's legs follow the rolling runs ([`sweep_leg`]): the newest pair one ran
    let legs = shards.iter().filter_map(|x| Some((num(x, "granted_ms"), pair_of(x.get("at")?)))).filter(|(_, at)| !at.work.is_empty() && *at != run_pair).max_by_key(|(granted, _)| *granted);
    if let Some((_, at)) = legs {
        pair = format!("{pair} (legs up to {})", at.short());
    }
    // (only a run that ended has any)
    let not_run = match count(crate::state::NOT_RUN) { 0 => String::new(), n => format!(", {n} not run") };
    let mut out = if v.get("sweep").and_then(Value::as_bool) == Some(true) {
        let word = if v.get("own_tests").and_then(Value::as_bool) == Some(true) { "own tests" } else { "sweep" };
        format!("{word} {} {pair}: {} of {} shards ok, {} fail, {} running, {} waiting, {} incomplete{not_run}, wall {wall}\n", t("id"), count("ok"), shards.len(), count("fail"), count("run"), count("wait"), count("incomplete"))
    } else {
        format!(
            "ci {} {pair}: {} ({} shards: {} ok, {} fail, {} running, {} waiting, {} incomplete{not_run}), wall {wall}\n",
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
    // what waits, by kind (`test`, `wtest`, `smoke`, ...): the list below leaves those out
    let mut waits: BTreeMap<&str, usize> = BTreeMap::new();
    for x in shards.iter().filter(|x| x.get("state").and_then(|y| y.as_str()) == Some("wait")) {
        *waits.entry(x.get("id").and_then(|y| y.as_str()).unwrap_or("").split(':').next().unwrap_or("")).or_default() += 1;
    }
    if !waits.is_empty() {
        let mut by: Vec<(&str, usize)> = waits.into_iter().collect();
        by.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
        out.push_str(&format!("  waiting: {}\n", by.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(", ")));
    }
    if !t("critical").is_empty() {
        out.push_str(&format!("critical path: {}\n", t("critical")));
    }
    if !t("fleet").is_empty() {
        out.push_str(&format!("fleet: {}\n", t("fleet")));
    }
    for b in v.get("boxes").and_then(|x| x.as_arr()).unwrap_or(&[]) {
        out.push_str(&format!("  box {}\n", b.as_str().unwrap_or("")));
    }
    for (i, l) in v.get("slow").and_then(|x| x.as_arr()).unwrap_or(&[]).iter().enumerate() {
        out.push_str(&format!("{} {}\n", if i == 0 { "slowest:" } else { "        " }, l.as_str().unwrap_or("")));
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
            let going = resume_run(&mut ci, &r.pair, r.dry, sweep, false, &r.lock_digest, &r.checkout);
            assert_eq!(going.as_ref(), Some(r));
            assert_eq!(ci, doc.ci);
        }
        let mut pool = Pool {
            tools: Vec::new(), proofs_collected: false, shards: Vec::new(), state: vec![CiShard { id: "gate".into(), state: "ok".into(), ..Default::default() }],
            reserved: Vec::new(), stopped: false, prio: PRIO, urgency: Vec::new(), overdue: Vec::new(), run_id: "gate".into(), sweep: false, leg: None,
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
        assert!(resume_run(&mut ci, &sweep.pair, sweep.dry, true, false, "new lock", "").is_none());
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
            ("apps/commercial/slices.json", "{\"foo\": [\"makepad-foo\"]}"),
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
    fn a_named_check_is_its_own_linux_shard_and_its_setting_round_trips() {
        let t = Tree::mem(vec![
            ("Cargo.toml", "[workspace]\nmembers = [\"apps/story\"]\n"),
            ("apps/story/Cargo.toml", "[package]\nname = \"story\"\n"),
            ("apps/story/ci.toml", "[ci]\nchecks = \"export::tests::every_profile\"\n"),
            ("apps/story/src/lib.rs", "#[test]\nfn one_profile() {}\n#[test]\n#[ignore]\nfn every_profile() {}\n"),
        ]);
        let p = sweep_plan(&t, &|_| None, 0);
        let check = p.iter().find(|s| s.id == "checks:story:every_profile").unwrap_or_else(|| panic!("{:?}", p.iter().map(|s| &s.id).collect::<Vec<_>>()));
        assert_eq!((check.os, check.kind, check.est), (Os::Linux, Kind::Test, GUESS_CHECK_SECS));
        assert_eq!(check.steps, vec![jit(&["test", "story", "--lib", "--", "--ignored", "--exact", "export::tests::every_profile"])]);
        // its own proof: the crate's tests are another unit, so a pass runs both
        assert_eq!(units_of(check)[0].section, "job.checks_story_every_profile");
        assert!(p.iter().any(|s| s.id.starts_with("test:") && s.steps.contains(&jit(&["test", "story"]))));
        let conf = read_conf("[ci]\nchecks = \"a::b, c\"\n").unwrap();
        assert_eq!(conf.checks, ["a::b", "c"]);
        assert_eq!(read_conf(&write_proofs(&conf, &Proofs::new())).unwrap(), conf);
    }

    #[test]
    fn the_sweep_has_every_kind_of_shard_on_the_right_os() {
        let t = tree();
        let p = sweep_plan(&t, &|_| None, 0);
        assert!(p.iter().all(|s| s.kind != Kind::Ci));
        let ids: Vec<&str> = p.iter().map(|s| s.id.as_str()).collect();
        for want in ["jit:01", "mjit:01", "wjit:01", "check:wasm32-unknown-unknown", "check:aarch64-apple-tvos", "test:makepad-platform:lib", "test:makepad-platform:remote", "test:makepad-widgets:ui", "slice:foo", "smoke:apps/calc", "smoke:apps/tool"] {
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

    /// A sweep on the two CI boxes (one Linux, one Windows): a Windows shard waiting in its box's
    /// queue holds no slot of the Linux box, a box takes one shard at a time (an ask counts), and a
    /// crate whose own files changed goes before longer ones whose dependencies alone changed.
    #[test]
    fn each_box_pulls_its_own_shard_and_changed_crates_go_first() {
        let bx = |n: &str, os: Os| BoxInfo { node: n.into(), os, arch: "x86_64".into(), gui: false, gpu: true, ci: true, checks_only: false };
        let (lin, win) = (bx("l", Os::Linux), bx("w", Os::Windows));
        let shard = |id: &str, os: Os, est: f64| Shard { id: id.into(), kind: Kind::Test, os, gui: false, gpu: false, steps: Vec::new(), judge: Judge::Tests, est, dirs: None, whole: false, cores: None };
        let shards = vec![shard("wtest:a", Os::Windows, 60.0), shard("test:deps", Os::Linux, 300.0), shard("test:own", Os::Linux, 20.0), shard("test:new", Os::Linux, 10.0)];
        let n = shards.len();
        let state = shards.iter().map(|s| CiShard { id: s.id.clone(), state: "wait".into(), ..Default::default() }).collect();
        let mut p = Pool { tools: Vec::new(), proofs_collected: false, shards, state, reserved: vec![None; n], stopped: false, prio: 0, urgency: vec![Urgency::Deps, Urgency::Deps, Urgency::Own, Urgency::New], overdue: vec![false; n], run_id: "S".into(), sweep: true, leg: None };
        // the Windows shard is stuck in its box's queue
        assert_eq!(p.next_for(&win), Some(0));
        p.reserved[0] = Some("w".into());
        p.state[0].state = "run".into();
        p.state[0].node = "w".into();
        p.state[0].summary = format!("{QUEUED}w: L1 queued");
        assert_eq!(p.next_for(&win), None, "one shard per box");
        assert_eq!(p.next_for(&lin), Some(2), "the Linux box works on, the crate whose own files changed first");
        p.reserved[2] = Some("l".into());
        assert_eq!(p.next_for(&lin), None, "its ask holds the box: no second shard races past it");
        p.reserved[2] = None;
        p.state[2].state = "ok".into();
        assert_eq!(p.next_for(&lin), Some(3), "then a crate with no proof");
        p.state[3].state = "ok".into();
        assert_eq!(p.next_for(&lin), Some(1));
    }

    /// Run 146: one leg failed and 18 never ran, all 19 read as failed; only the one is.
    #[test]
    fn a_leg_that_did_not_run_is_no_failure() {
        let leg = |name: &str, state: &str, summary: &str| crate::state::Leg { name: name.into(), state: state.into(), summary: summary.into(), ..Default::default() };
        let log = Path::new("x.log");
        let unrun = leg("smoke:a", crate::state::NOT_RUN, "the run stalled, no shard got a box in 30 min; runs: proof stale");
        let mixed = bucket_outcome(Verdict::Fail, &[leg("t", "ok", ""), leg("smoke:b", "fail", "exit 1"), unrun.clone()], log);
        assert_eq!(mixed.failing, vec!["smoke:b".to_string()]);
        assert!(!mixed.ok && mixed.no_verdict.is_none(), "a real failure stands");
        assert_eq!(mixed.summary, "3 shards: 1 ok, 1 fail, 0 incomplete, 1 not run; fail");
        let stopped = bucket_outcome(Verdict::Incomplete, &[leg("t", "ok", ""), unrun], log);
        assert!(stopped.failing.is_empty() && !stopped.ok);
        let why = stopped.no_verdict.expect("nothing failed: no verdict");
        assert!(why.starts_with("1 of 2 shards have no verdict, smoke:a: the run stalled"), "{why}");
        assert!(bucket_outcome(Verdict::Ok, &[leg("t", "ok", "")], log).no_verdict.is_none());
        assert!(bucket_outcome(Verdict::Incomplete, &[], log).no_verdict.is_some(), "no legs at all");
    }

    #[test]
    fn a_shard_that_could_not_get_its_box_goes_behind_the_rest() {
        let lin = BoxInfo { node: "l".into(), os: Os::Linux, arch: "x86_64".into(), gui: false, gpu: true, ci: true, checks_only: false };
        let shard = |id: &str, est: f64| Shard { id: id.into(), kind: Kind::Test, os: Os::Linux, gui: false, gpu: false, steps: Vec::new(), judge: Judge::Tests, est, dirs: None, whole: false, cores: None };
        let shards = vec![shard("whole", 300.0), shard("small", 20.0)];
        let state = shards.iter().map(|s| CiShard { id: s.id.clone(), state: "wait".into(), ..Default::default() }).collect();
        let mut p = Pool { tools: Vec::new(), proofs_collected: false, shards, state, reserved: vec![None; 2], stopped: false, prio: 0, urgency: vec![Urgency::Deps; 2], overdue: vec![false; 2], run_id: "r".into(), sweep: false, leg: None };
        assert_eq!(p.next_for(&lin), Some(0), "the longest first");
        p.state[0].spans.push(CiSpan { node: "l".into(), asked_ms: 1, granted_ms: 0, ended_ms: 2 });
        assert_eq!(p.next_for(&lin), Some(1), "its ask gave up: the other goes");
        p.state[1].state = "ok".into();
        assert_eq!(p.next_for(&lin), Some(0), "and it asks again when nothing else can run");
    }

    #[test]
    fn own_tests_take_the_changed_and_unproven_crates_a_box_can_run_that_did_not_fail_as_they_are() {
        let shard = |id: &str| Shard { id: id.into(), kind: Kind::Test, os: Os::Linux, gui: false, gpu: false, steps: Vec::new(), judge: Judge::Tests, est: 1.0, dirs: None, whole: false, cores: None };
        let urgency: BTreeMap<String, Urgency> = [("test:own", Urgency::Own), ("test:gpu", Urgency::Own), ("test:broken", Urgency::Own), ("test:deps", Urgency::Deps), ("test:new", Urgency::New)].map(|(k, v)| (k.to_string(), v)).into();
        let all = ["test:own", "test:gpu", "test:broken", "test:deps", "test:new", "test:kept"].map(shard).to_vec();
        let picked = own_tests_picked(all, &urgency, &|s| s.id != "test:gpu", &|s| s.id == "test:broken");
        assert_eq!(picked.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["test:own", "test:new"]);
    }

    #[test]
    fn a_failed_test_step_names_its_tests() {
        let log = "test a::b ... FAILED\n\nfailures:\n\n---- a::b stdout ----\npanicked\n\nfailures:\n    a::b\n    c::d\n\ntest result: FAILED. 1 passed; 2 failed\n";
        assert_eq!(judge(Judge::Tests, "test makepad-x --lib", 101, log), (false, "test makepad-x --lib: exit 101: a::b, c::d".to_string()));
        assert_eq!(judge(Judge::Tests, "test makepad-x --lib", 1, "error: could not compile `x`\n"), (false, "test makepad-x --lib: exit 1".to_string()));
    }

    #[test]
    fn a_stale_proof_names_the_crates_own_change_first() {
        let proof = Proof { digest: "d".into(), pair: Pair::new("a", "b"), capture: String::new(), date: "2026-11-02".into() };
        let unit = Unit { dir: "platform/script/compute".into(), section: "tests".into() };
        let closure: BTreeSet<String> = ["platform/script/compute", "platform/script/compute/shim", "libs/error_log"].map(String::from).into();
        let stale = |changed: &[&str]| {
            let mut cache = Changes::new();
            cache.insert(("a".into(), "b".into()), Some(Inputs { changed: changed.iter().map(|p| p.to_string()).collect(), jit_changed: false }));
            stale_path(Path::new("."), &Pair::new("c", "d"), &unit, &proof, Some(&closure), &mut cache)
        };
        let (dep, nested, own) = ("libs/error_log/src/lib.rs", "platform/script/compute/shim/src/lib.rs", "platform/script/compute/src/kernel.rs");
        assert_eq!(stale(&[dep, nested, own]), (true, own.to_string()));
        assert_eq!(stale(&[dep, nested]), (false, dep.to_string()), "a package nested in its directory is not its own");
        assert_eq!(stale(&["apps/x/src/main.rs"]), (false, "Cargo.lock".to_string()), "nothing it reads changed: its locked dependencies did");
        let doc = "platform/script/compute/CREDITS.md";
        assert_eq!(stale(&[dep, doc]), (false, dep.to_string()), "its own document beside a dependency's code is no own change");
        assert_eq!(stale(&[doc]), (true, doc.to_string()), "only a document changed: one its sources name");
    }

    #[test]
    fn a_shard_the_run_ended_without_says_why_in_its_log() {
        let dir = std::env::temp_dir().join(format!("devhub-ci-did-not-run-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let at = |id: &str, state: &str, summary: &str| CiShard { id: id.into(), state: state.into(), summary: summary.into(), log: dir.join(format!("{id}.log")).to_string_lossy().into_owned(), ..Default::default() };
        let (waits, lost, ok) = (at("w", "wait", "runs: no proof"), at("i", "incomplete", "lost twice"), at("o", "ok", "1 step ok"));
        std::fs::write(&waits.log, "waits: runs: no proof\n").unwrap();
        let mut lost = lost;
        lost.spans.push(CiSpan { node: "l".into(), asked_ms: 1, granted_ms: 2, ended_ms: 3 });
        for st in [&waits, &lost, &ok] {
            did_not_run(st, true).unwrap();
        }
        assert_eq!(std::fs::read_to_string(&waits.log).unwrap(), "waits: runs: no proof\ndid not run: the run stopped while it waited (runs: no proof)\n");
        assert_eq!(std::fs::read_to_string(&lost.log).unwrap(), "did not finish: lost twice\n");
        assert!(!Path::new(&ok.log).exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_sweep_starts_no_shard_where_lane_work_is_queued() {
        use crate::lease::sched::{Req, Sched};
        let mut s = Sched::new("m", 1);
        s.add_node("w", "w", Some(true));
        let mut lane = Req::new("foo", Kind::Screen);
        lane.node = Some("w".into());
        let id = s.request(lane, 0);
        let mut ci = Req::new(LANE, Kind::Test);
        ci.node = Some("l".into());
        s.request(ci, 0);
        let v = s.view();
        assert!(lane_queued_for(&v, None, "w"), "a lane lease names the box");
        assert!(!lane_queued_for(&v, None, "l"), "CI's own asks do not hold a sweep back");
        assert!(!lane_queued_for(&v, None, "x"));
        assert!(lane_queued_for(&v, Some(&id), "x"), "a lane lease holding the box reserved");
    }

    #[test]
    fn a_failure_asks_a_second_box_and_a_box_failing_several_is_out() {
        let bx = |n: &str, os: Os| BoxInfo { node: n.into(), os, arch: "x86_64".into(), gui: true, gpu: true, ci: true, checks_only: false };
        let boxes = [bx("a", Os::Linux), bx("b", Os::Linux), bx("m", Os::Macos)];
        let shard = |id: &str| Shard { id: id.into(), kind: Kind::Test, os: Os::Any, gui: false, gpu: false, steps: Vec::new(), judge: Judge::Exit, est: 1.0, dirs: None, whole: false, cores: None };
        let n = BOX_OUT + 1;
        let mut p = Pool { tools: Vec::new(), proofs_collected: false, shards: (0..n).map(|i| shard(&format!("s{i}"))).collect(), state: (0..n).map(|i| CiShard { id: format!("s{i}"), state: "wait".into(), ..Default::default() }).collect(), reserved: vec![None; n], stopped: false, prio: 0, urgency: vec![Urgency::Deps; n], overdue: vec![false; n], run_id: String::new(), sweep: false, leg: None };
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

    /// A licence, a readme or a test's `ci.splash` re-proved every crate holding one (11-05: 26
    /// stale units from two CREDITS.md and 12 tests units from a comment in a ci.splash).
    #[test]
    fn a_document_no_source_names_and_a_tests_ci_splash_are_no_proof_input() {
        let base = [
            ("tools/run/Cargo.toml", "[package]\nname = \"makepad-run\"\n"),
            ("tools/run/src/main.rs", "fn main() {}\n"),
            ("apps/x/Cargo.toml", "[package]\nname = \"x\"\n"),
            ("apps/x/src/main.rs", "const CREDITS: &str = include_str!(\"../CREDITS.md\");\n"),
            ("apps/x/README.md", "old\n"),
            ("apps/x/LICENSE-MIT", "old\n"),
            ("apps/x/CREDITS.md", "old\n"),
            ("apps/x/resources/help.md", "old\n"),
            ("apps/x/ci.splash", "old\n"),
        ];
        let units: Vec<Unit> = ["app", "tests", "job.check"].iter().map(|s| Unit { dir: "apps/x".into(), section: s.to_string() }).collect();
        let digests = |changed: &str| {
            let tree = Tree::mem(base.iter().map(|(p, t)| (*p, if *p == changed { "new\n" } else { *t })).collect());
            let lock = "[[package]]\nname = \"x\"\nversion = \"0.1.0\"\n\n[[package]]\nname = \"makepad-run\"\nversion = \"0.1.0\"\n";
            let dg = Digests::with_lock(&tree, &units, lock.to_string()).unwrap();
            units.iter().map(|u| dg.of(u, Os::Linux).to_string()).collect::<Vec<_>>()
        };
        let before = digests("");
        assert!(before.iter().all(|d| !d.is_empty()));
        for doc in ["apps/x/README.md", "apps/x/LICENSE-MIT"] {
            assert_eq!(digests(doc), before, "{doc}: no unit's input");
        }
        // a document a source includes, an app's resources, a source: every unit's
        for input in ["apps/x/CREDITS.md", "apps/x/resources/help.md", "apps/x/src/main.rs"] {
            assert!(digests(input).iter().zip(&before).all(|(a, b)| a != b), "{input}");
        }
        // the smoke script: the app's and its jobs', not its tests'
        let script = digests("apps/x/ci.splash");
        assert!(script[0] != before[0] && script[1] == before[1] && script[2] != before[2]);
    }

    /// Run 153's route smoke on Windows: 800 s, 770 s of it building the JIT before its first
    /// step; the summary names where a slow shard's time went.
    #[test]
    fn a_shards_log_says_where_its_time_went_and_what_its_jit_did() {
        let smoke = "STEP running apps/x / launch\n  owned x\nSTEP passed apps/x / launch (12.5 s)\nSTEP passed apps/x / comes up (3.0 s)\nSTEP failed apps/x / judge (1.5 s)\n";
        assert_eq!(time_went(smoke, 31.0), "17 s in 3 steps (longest apps/x / launch 12.5 s), 14 s outside them");
        let tests = "$ cargo run --profile host -q -p makepad-run --bin makepad -- test demo-model --features x\ndevhub: kept the JIT binary k\n     Running unittests src/lib.rs\ntest result: ok. 7 passed; 0 failed; finished in 4.29s\n     Running tests/plates.rs\ntest result: ok. 8 passed; 0 failed; finished in 10.30s\n== exit 0\n";
        assert_eq!(time_went(tests, 800.0), "15 s in 2 steps (longest demo-model tests/plates.rs 10.3 s), 785 s outside them (it built the JIT)");
        assert_eq!(time_went("== exit 0\n", 5.0), "no step times in its log");
        assert_eq!(tool_how("devhub: the JIT is this box's binary k, no cargo\nmakepad 0.1\n"), "the box had it");
        assert_eq!(tool_how("devhub: kept the JIT binary k\nmakepad 0.1\n"), "built");
        assert_eq!(tool_how("devhub: this JIT binary is not kept: makepad.d names only 2 tracked files\n"), "built, not kept (makepad.d names only 2 tracked files)");
        let shard = |id: &str, granted: u64, ended: u64| CiShard { id: id.into(), node: "n".into(), log: "/nonexistent/x.log".into(), spans: vec![CiSpan { granted_ms: granted, ended_ms: ended, ..Default::default() }], ..Default::default() };
        let slow = slowest(&[shard("a", 1000, 2000), shard("b", 1000, 61_000), shard("never", 0, 5000)], 5);
        assert_eq!(slow, vec!["b 60 s on n: no step times in its log".to_string(), "a 1 s on n: no step times in its log".to_string()]);
    }

    /// A bench box takes only cross-target checks, its OS's shards no pool box can;
    /// those stay in a run with no arm64-relevant change (their proofs say whether they run).
    #[test]
    fn a_bench_box_takes_only_the_cross_target_checks() {
        let mac = |id: &str| Shard { id: id.into(), kind: Kind::Build, os: Os::Macos, gui: false, gpu: false, steps: Vec::new(), judge: Judge::Cargo, est: 1.0, dirs: None, whole: false, cores: None };
        let mut shards = vec![mac("check:aarch64-apple-ios"), mac("mjit"), Shard { os: Os::Linux, ..mac("test:a:lib") }];
        let skipped = arm_plan(&Tree::mem(Vec::new()), &mut shards, &super::super::arm::Decision { required: false, full: false, summary: String::new() });
        assert_eq!(skipped.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["mjit"]);
        assert_eq!(shards.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["check:aarch64-apple-ios", "test:a:lib"]);
        let bench = BoxInfo { node: "m".into(), os: Os::Macos, arch: "aarch64".into(), gui: true, gpu: true, ci: true, checks_only: true };
        let sh = |id: &str, kind: Kind| Shard { id: id.into(), kind, os: Os::Macos, gui: false, gpu: false, steps: Vec::new(), judge: Judge::Cargo, est: 1.0, dirs: None, whole: false, cores: None };
        assert!(bench.fits(&sh("check:aarch64-apple-ios", Kind::Build)));
        for other in ["smoke:macos:apps/x", "test:a:lib", "mjit"] {
            assert!(!bench.fits(&sh(other, Kind::Test)), "{other}");
        }
        assert!(BoxInfo { checks_only: false, ..bench.clone() }.fits(&sh("test:a:lib", Kind::Test)));
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
        let lin = BoxInfo { node: "node-a".into(), os: Os::Linux, arch: "x86_64".into(), gui: false, gpu: true, ci: true, checks_only: false };
        let win = BoxInfo { node: "node-b".into(), os: Os::Windows, arch: "x86_64".into(), gui: true, gpu: true, ci: true, checks_only: false };
        let sh = |os, gui| Shard { id: "s".into(), kind: Kind::Build, os, gui, gpu: false, steps: Vec::new(), judge: Judge::Exit, est: 1.0, dirs: None, whole: false, cores: None };
        assert!(lin.fits(&sh(Os::Linux, false)) && !lin.fits(&sh(Os::Any, true)) && !win.fits(&sh(Os::Any, true)) && win.fits(&sh(Os::Windows, false)));
        let shard = |id: &str, spans: Vec<(&str, u64, u64, u64)>| CiShard { id: id.into(), spans: spans.into_iter().map(|(n, a, g, e)| CiSpan { node: n.into(), asked_ms: a, granted_ms: g, ended_ms: e }).collect(), ..Default::default() };
        let state = vec![
            shard("s0", vec![("node-a", 1000, 1000, 31_000)]),
            // asked on one box, never granted: it went to another
            shard("s1", vec![("node-a", 31_000, 0, 41_000), ("node-a", 41_000, 41_000, 61_000)]),
            shard("s2", vec![("node-c", 0, 0, 5000), ("node-c", 5000, 11_000, 71_000)]),
            shard("kept", Vec::new()),
        ];
        let (crit, boxes, fleet) = critical(&state, 1000, 81_000);
        assert_eq!(crit, "node-c ends at 70 s: s2 60 s");
        assert_eq!(boxes, vec!["node-a 50 s busy, 2 shards, 10 s queued", "node-c 60 s busy, 1 shards, 11 s queued"]);
        assert_eq!(fleet, "wall 80 s: 110 s on 2 boxes (1.6 at once), 21 s queued for leases, 10 s with nothing on any box");
        assert_eq!(groups(vec![("a".into(), 30.0), ("b".into(), 30.0), ("c".into(), 30.0)], 60.0), vec![vec!["a".to_string(), "b".into()], vec!["c".into()]]);
    }

    #[test]
    fn a_shard_no_box_can_take_waits_or_says_why() {
        let sh = Shard { id: "s".into(), kind: Kind::Build, os: Os::Macos, gui: false, gpu: false, steps: Vec::new(), judge: Judge::Exit, est: 1.0, dirs: None, whole: false, cores: None };
        let mac = BoxInfo { node: "node-d".into(), os: Os::Macos, arch: "aarch64".into(), gui: true, gpu: true, ci: true, checks_only: false };
        let wait = CiShard::default();
        // a master that just started has not heard its boxes: no box is gone
        assert_eq!(no_box(&sh, &wait, &[], &[], &[], Duration::from_secs(3600), false, false), None);
        // heard, none of its kind here: it waits, within a bound
        assert_eq!(no_box(&sh, &wait, &[], &[], &[], Duration::from_secs(60), true, false), None);
        let (state, why) = no_box(&sh, &wait, &[], &[], &[], NO_BOX_WAIT, true, false).unwrap();
        assert_eq!((state, why.as_str()), ("incomplete", "no box of macos came in 15 min"));
        // every box of its kind said it cannot run it: a failure that names them
        let unfit = CiShard { summary: "unfit: xcheck x: no nightly".into(), lost: vec![("node-d".into(), LOST_TRIES)], ..Default::default() };
        let (state, why) = no_box(&sh, &unfit, &[&mac], &[], &[], Duration::ZERO, true, false).unwrap();
        assert_eq!((state, why.as_str()), ("fail", "no box can run it (node-d): unfit: xcheck x: no nightly"));
        // ... unless a box that may be able to is away: it gets its time
        assert_eq!(no_box(&sh, &unfit, &[&mac], &[], &[], Duration::ZERO, true, true), None);
        assert_eq!(no_box(&sh, &unfit, &[&mac], &[], &[], NO_BOX_WAIT, true, true).unwrap().0, "fail");
        // the box is out of the run, or the shard's first failure has no second box
        assert_eq!(no_box(&sh, &wait, &[&mac], &[], &["node-d".to_string()], Duration::ZERO, true, false).unwrap().0, "incomplete");
        let failed = CiShard { failed_on: "node-d".into(), failed_why: "exit 1".into(), ..Default::default() };
        assert_eq!(no_box(&sh, &failed, &[], &[], &[], Duration::ZERO, true, false), Some(("fail", "exit 1".to_string())));
        // the box of its kind is down: it waits for it a while, then does not run this round
        assert_eq!(no_box(&sh, &wait, &[], &["node-d"], &[], Duration::from_secs(60), true, true), None);
        let (state, why) = no_box(&sh, &wait, &[], &["node-d"], &[], NO_BOX_WAIT, true, true).unwrap();
        assert_eq!((state, why.as_str()), ("incomplete", "box down: node-d (not back in 15 min)"));
        let dir = std::env::temp_dir().join(format!("devhub-ci-box-down-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // only ever asked for there (its asks withdrawn as the box went down): it did not run
        let asked = CiShard { state: state.into(), summary: why, log: dir.join("s.log").to_string_lossy().into_owned(), spans: vec![CiSpan { node: "node-d".into(), asked_ms: 1, granted_ms: 0, ended_ms: 2 }], ..Default::default() };
        did_not_run(&asked, false).unwrap();
        assert_eq!(std::fs::read_to_string(&asked.log).unwrap(), "did not run: box down: node-d (not back in 15 min)\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_script_can_pin_its_os() {
        assert_eq!(pinned_os("// a comment\n// ci os: linux\nnil\n"), Some(Os::Linux));
        assert_eq!(pinned_os("nil\n"), None);
    }

    #[test]
    fn the_target_matrix_matches_cargo_makepad() {
        let source = include_str!("../../../../tools/cargo_makepad/src/check/mod.rs");
        let table = source.split_once("const TOOLCHAINS:").unwrap().1.split_once("= [").unwrap().1.split_once("];").unwrap().0;
        let table = table.lines().map(|line| line.split("//").next().unwrap()).collect::<Vec<_>>().join("\n");
        let rows: Vec<(String, String, String)> = table.split('(').skip(1).map(|tuple| {
            let fields: Vec<&str> = tuple.split_once(')').unwrap().0.split(',').map(str::trim).filter(|s| !s.is_empty()).collect();
            assert_eq!(fields.len(), 3);
            (fields[0].trim_matches('"').to_string(), fields[1].strip_prefix("BuildTy::").unwrap().to_string(), fields[2].strip_prefix("Platform::").unwrap().to_lowercase())
        }).collect();
        let mirror: Vec<(String, String, String)> = TARGETS.iter().map(|(target, build, platform)| {
            let build = match build {
                Build::Bin => "Binary",
                Build::Lib => "Lib",
                Build::Std => "BinaryBuildStd",
            };
            (target.to_string(), build.to_string(), platform.to_string())
        }).collect();
        assert_eq!(rows, mirror, "update the target matrix with cargo_makepad");
    }

    #[test]
    fn a_package_is_checked_only_on_the_platforms_it_declares() {
        let script = |package: &str| format!("ci.check_targets({{packages: [\"{package}\"] targets: ci.host.targets}})\n");
        let (wm, flow, calc, typo) = (script("wm"), script("flow"), script("calc"), script("typo"));
        let t = Tree::mem(vec![
            ("apps/wm/Cargo.toml", "[package]\nname = \"wm\"\n[package.metadata.makepad]\nplatforms = [\"desktop\"]\n"),
            ("apps/wm/ci.splash", wm.as_str()),
            ("apps/flow/Cargo.toml", "[package]\nname = \"flow\"\n[package.metadata.makepad]\nplatforms = [\"desktop\", \"mobile\"]\n"),
            ("apps/flow/ci.splash", flow.as_str()),
            ("apps/calc/Cargo.toml", "[package]\nname = \"calc\"\n"),
            ("apps/calc/ci.splash", calc.as_str()),
            ("apps/typo/Cargo.toml", "[package]\nname = \"typo\"\n[package.metadata.makepad]\nplatforms = [\"browser\"]\n"),
            ("apps/typo/ci.splash", typo.as_str()),
        ]);
        let (_, scripts) = discover(&t);
        let m = matrix(&t, &scripts);
        let at = |target: &str| m[target].iter().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(at("x86_64-pc-windows-gnu"), ["calc", "flow", "typo", "wm"]);
        assert_eq!(at("aarch64-linux-android"), ["calc", "flow", "typo"]);
        assert_eq!(at("wasm32-unknown-unknown"), ["calc", "typo"]);
    }

    #[test]
    fn every_smoke_gets_judged_in_the_gate_and_the_sweep() {
        let t = tree();
        for shards in [plan(&t, &|_| None, 0), sweep_plan(&t, &|_| None, 0)] {
            let (smokes, others): (Vec<&Shard>, Vec<&Shard>) = shards.iter().partition(|s| s.id.starts_with("smoke:"));
            assert!(!smokes.is_empty());
            assert!(smokes.iter().all(|s| saves_grabs(s)));
            assert!(!others.iter().any(|s| saves_grabs(s)));
        }
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

    /// The JIT legs and a lane's JIT pin skip a pair whose JIT inputs did not change: a document no
    /// source of its crate names is none, as for proofs.
    #[test]
    fn a_document_no_source_names_leaves_the_jit_unchanged() {
        let repo = std::env::temp_dir().join(format!("devhub-ci-jit-docs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(repo.join("tools/run/src")).unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git").current_dir(&repo).args(args).output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let commit = |what: &str| {
            git(&["add", "-A"]);
            git(&["commit", "-q", "-m", what]);
            Pair::new(&git(&["rev-parse", "HEAD"]), "")
        };
        let write = |p: &str, t: &str| std::fs::write(repo.join(p), t).unwrap();
        write("tools/run/Cargo.toml", "[package]\nname = \"makepad-run\"\n");
        write("tools/run/src/main.rs", "const CREDITS: &str = include_str!(\"../CREDITS.md\");\n");
        write("tools/run/README.md", "old\n");
        write("tools/run/CREDITS.md", "old\n");
        git(&["init", "-q", "-b", "work"]);
        git(&["config", "user.name", "CI Test"]);
        git(&["config", "user.email", "ci@example.invalid"]);
        let mut at = commit("old");
        let mut changed = |p: &str| {
            write(p, &format!("{}\n", std::fs::read_to_string(repo.join(p)).unwrap()));
            let next = commit(p);
            let jit = Inputs::between(&repo, &at, &next).unwrap().jit_changed;
            at = next;
            jit
        };
        assert!(!changed("tools/run/README.md"), "a readme nothing includes");
        assert!(changed("tools/run/CREDITS.md"), "a document its source includes");
        assert!(changed("tools/run/src/main.rs"));
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// A sweep started behind a newer rolling run runs its legs on that run's pair with that
    /// run's Cargo.lock: a box that ran that run has that tree built and builds nothing new. A leg
    /// whose crate is gone there keeps the sweep's pair, as does a sweep the rolling run is not
    /// newer than. A proof names the pair its legs ran; targets that ran at different inputs
    /// prove nothing.
    #[test]
    fn a_sweep_leg_takes_the_newest_rolling_runs_pair_and_lock() {
        let home = std::env::temp_dir().join(format!("devhub-ci-sweep-leg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let repo = home.join("tree");
        std::fs::create_dir_all(repo.join("libs/a/src")).unwrap();
        std::fs::create_dir_all(repo.join("libs/gone")).unwrap();
        let git = |args: &[&str]| {
            let o = std::process::Command::new("git").current_dir(&repo).args(args).output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            String::from_utf8_lossy(&o.stdout).trim().to_string()
        };
        let commit = |what: &str| {
            git(&["add", "-A"]);
            git(&["commit", "-q", "-m", what]);
            Pair::new(&git(&["rev-parse", "HEAD"]), "")
        };
        std::fs::write(repo.join("libs/a/Cargo.toml"), "[package]\nname = \"a\"\n").unwrap();
        std::fs::write(repo.join("libs/a/src/lib.rs"), "// old\n").unwrap();
        std::fs::write(repo.join("libs/gone/Cargo.toml"), "[package]\nname = \"gone\"\n").unwrap();
        git(&["init", "-q", "-b", "work"]);
        git(&["config", "user.name", "CI Test"]);
        git(&["config", "user.email", "ci@example.invalid"]);
        let old = commit("old");
        std::fs::write(repo.join("libs/a/src/lib.rs"), "// new\n").unwrap();
        std::fs::remove_dir_all(repo.join("libs/gone")).unwrap();
        let new = commit("new");
        std::fs::write(repo.join("libs/a/src/lib.rs"), "// newer\n").unwrap();
        let newer = commit("newer");
        let leases = Arc::new(crate::lease::master::LeaseMaster::new("mac", home.clone(), Arc::new(|_: &str, _: &str, _: &str| Err("no nodes".to_string())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        let ctx = Ctx { store: crate::state::Store::open(&home, "mac", Arc::new(|_| 0)).unwrap(), leases, machine: "mac".into(), monitor: crate::monitor::Monitor::start(&[]), repo: repo.clone(), home: home.clone() };
        // the rolling run of `new`, with the Cargo.lock it shipped
        let lock = "[[package]]\nname = \"a\"\nversion = \"0.1.0\"\n";
        std::fs::create_dir_all(ctx.logs("ci").join("R")).unwrap();
        std::fs::write(ctx.logs("ci").join("R/Cargo.lock"), lock).unwrap();
        ctx.store.update(|d| d.ci.runs.push(CiRun { id: "R".into(), pair: new.clone(), started_ms: 10, ended_ms: 20, verdict: "ok".into(), ..Default::default() })).unwrap();
        let dir = ctx.logs("ci").join("S");
        let shard = |id: &str, dir: &str| Shard { id: id.into(), kind: Kind::Test, os: Os::Linux, gui: false, gpu: false, steps: Vec::new(), judge: Judge::Tests, est: 1.0, dirs: Some(vec![dir.into()]), whole: false, cores: None };
        let (a, gone) = (shard("wtest:a", "libs/a"), shard("wtest:gone", "libs/gone"));
        // (a sweep's pool: its legs share the leg tree it found)
        let sweep = || Mutex::new(Pool { tools: Vec::new(), proofs_collected: false, shards: Vec::new(), state: Vec::new(), reserved: Vec::new(), stopped: false, prio: 0, urgency: Vec::new(), overdue: Vec::new(), run_id: "S".into(), sweep: true, leg: None });
        let pool = sweep();
        let leg = sweep_leg(&ctx, &old, &dir, &a, &pool).expect("a leg of a sweep behind the rolling run takes its pair");
        assert_eq!(leg.pair, new);
        assert_eq!(std::fs::read_to_string(leg.dir.join("Cargo.lock")).unwrap(), lock, "and ships the Cargo.lock that run shipped");
        assert!(Arc::ptr_eq(&leg, &sweep_leg(&ctx, &old, &dir, &a, &pool).unwrap()), "the next leg reads no tree again");
        assert!(sweep_leg(&ctx, &old, &dir, &gone, &pool).is_none(), "a crate gone at the newer tree keeps the sweep's pair");
        assert!(sweep_leg(&ctx, &new, &dir, &a, &sweep()).is_none(), "a sweep at the rolling run's pair runs its own");
        assert!(sweep_leg(&ctx, &newer, &dir, &a, &sweep()).is_none(), "a sweep newer than the rolling run runs its own");

        // the proof of a leg at `new`: its inputs there, not the sweep's
        let units = units_of(&a);
        let dg = Digests::with_lock(&Tree::at(&repo, &old).unwrap(), &units, lock.to_string()).unwrap();
        let at_new = Digests::with_lock(&Tree::at(&repo, &new).unwrap(), &units, lock.to_string()).unwrap();
        let ran = |at: &Pair, granted_ms: u64| CiShard { id: "wtest:a".into(), state: "ok".into(), at: at.clone(), granted_ms, ..Default::default() };
        let state = vec![ran(&new, 5)];
        let mut legs = LegDigests::new();
        leg_digests(&ctx, &old, &dir, &units, &state, &mut legs);
        let proofs = passed_units(&ctx, &old, &dg, &legs, std::slice::from_ref(&a), &state, 7);
        let proof = &proofs[&(units[0].clone(), Os::Linux)];
        assert_eq!((&proof.pair, proof.digest.as_str()), (&new, at_new.of(&units[0], Os::Linux)));
        assert_ne!(proof.digest, dg.of(&units[0], Os::Linux));
        // two targets of the crate, one at each pair: neither inputs passed whole
        let two = [a.clone(), Shard { id: "wtest:a:2".into(), ..a.clone() }];
        let state = vec![ran(&new, 5), ran(&old, 6)];
        leg_digests(&ctx, &old, &dir, &units, &state, &mut legs);
        assert!(passed_units(&ctx, &old, &dg, &legs, &two, &state, 7).is_empty());
        let _ = std::fs::remove_dir_all(&home);
    }
}
