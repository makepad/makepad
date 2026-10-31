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
//! - `wtest:<pkg>:<target>`: a rotating third of them natively on Windows (105);
//! - `check:<target>`: cargo's `check --release --target` of every app the scripts check there:
//!   the cross targets the JIT does not build (Apple-SDK targets on a Mac, the rest on Linux);
//! - `slice:<app>`: a licensed commercial slice, cut, linted and checked (`ci-leg slice`);
//! - `smoke:<dir>`: one app's `ci.splash` (launch hidden, drive it, grab, the vision judge on the AI
//!   fleet: `ci-leg smoke`) on any box that shows windows; a rotating third of them on a Mac, a third on Windows.
//!
//! What is checked where comes from the tree: the apps (`apps/<x>/Cargo.toml`), their
//! `ci.splash` (`ci.check_targets({packages, targets})`), the workspace members, the commercial
//! `slices.json`. The run's state is [`crate::state::Ci`]; [`gate`] is what `devhub verify` runs.
//!
//! A master restart ends no run: a shard's steps live on its box, and the gate that goes on follows
//! the step it was at, on the lease that came back, from the log of that run. A run of a later pair
//! keeps the passes of the runs before it whose inputs did not change ([`Inputs`]).
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
use crate::state::{CiRun, CiShard, CiSpan, Pair};
use makepad_strict_json::{obj, s, Value};
use makepad_toml_parser::{parse_toml, Toml};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Verify's priority: the lowest; landings and lanes go first (and background work holds back
/// while lane work waits).
pub const PRIO: i32 = super::verify::PRIO;
/// A dry run (a lane's preflight) is lane work: a lane's priority, on at most N-2 boxes at once.
pub const DRY_PRIO: i32 = crate::lease::sched::DEFAULT_PRIO;
/// The lane and agent the shards' leases are under.
const LANE: &str = crate::lease::sched::CI_LANE;
const AGENT: &str = "devhub-ci";
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
/// A run in which no shard got a box this long gives up, saying what its asks wait for (verify goes
/// on with it later, what passed standing).
const STALLED: Duration = Duration::from_secs(30 * 60);

/// A test binary that takes longer than this on Linux stays off the one Windows box.
const WINDOWS_TEST_MAX: f64 = 300.0;

/// The apps the CI box skipped (its ci.toml `skip_apps`): not apps, or not launchable alone.
pub const SKIP_APPS: &[&str] = &["source-library", "mixer", "mail", "wm-all", "wm-dyn", "asset-server", "flow-server", "ai-hub"];
/// The platform's own crates: their tests run in every CI run.
pub const TEST_DIRS: &[&str] = &["platform", "draw", "widgets", "tools/ci_leg", "libs/remote"];
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
    /// the crates it reads, by their dirs (and through them their path dependencies); None: it
    /// reads more than crates, so its result holds for its own pair only
    pub dirs: Option<Vec<String>>,
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
fn pinned_os(text: &str) -> Option<Os> {
    text.lines().find_map(|l| l.strip_prefix("// ci os:")).map(|os| Os::of(os.trim())).filter(|os| *os != Os::Any)
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
    // a package's dir (a package not found keeps its shard to its own pair)
    let dirs_of: BTreeMap<String, String> = tree.files.iter().filter(|f| f.ends_with("/Cargo.toml")).filter_map(|f| krate(tree, f)).map(|k| (k.package, k.dir)).collect();
    let dirs = |pkgs: &mut dyn Iterator<Item = &String>| -> Option<Vec<String>> { pkgs.map(|p| dirs_of.get(p).cloned()).collect() };
    let mut push = |id: String, kind: Kind, os: Os, gui: bool, steps: Vec<Vec<String>>, judge: Judge, guess: f64, dirs: Option<Vec<String>>| {
        let e = est(&id, guess);
        shards.push(Shard { id, kind, os, gui, steps, judge, est: e, dirs });
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
        for (target, select) in lib.into_iter().chain(k.tests.iter().map(|t| (t.clone(), vec!["--test".to_string(), t.clone()]))) {
            let mut args = vec!["test", k.package.as_str()];
            args.extend(select.iter().map(String::as_str));
            let id = format!("test:{}:{target}", k.package);
            let long = timing(&id).is_some_and(|secs| secs > WINDOWS_TEST_MAX);
            push(id, Kind::Test, Os::Linux, false, vec![jit(&args)], Judge::Exit, 20.0, Some(vec![k.dir.clone()]));
            if (tests + rotation) % 3 == 0 && !long {
                push(format!("wtest:{}:{target}", k.package), Kind::Test, Os::Windows, false, vec![jit(&args)], Judge::Exit, 60.0, Some(vec![k.dir.clone()]));
            }
            tests += 1;
        }
    }
    // what runs through ci-leg: a tree from before it (a bisect's baseline) has none to ask
    if !tree.has("tools/ci_leg/Cargo.toml") {
        return shards;
    }
    // the licensed slices
    for (app, roots) in slices(tree) {
        let roots = roots.join(",");
        push(format!("slice:{app}"), Kind::Build, Os::Linux, false, vec![ci_leg(&["slice", &app, &roots])], Judge::Exit, 40.0, None);
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
    stopped: bool,
    /// the lease priority of the run's shards
    prio: i32,
    /// boxes the run may hold at once
    max_active: usize,
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
        let active = (0..self.shards.len()).filter(|&i| self.reserved[i].is_some() || self.state[i].state == "run").count();
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
fn publish(ctx: &Ctx, pool: &Pool) {
    let shards = pool.state.clone();
    ctx.store.update(|d| {
        if let Some(r) = d.ci.run.as_mut() {
            r.shards = shards;
        }
    });
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

/// The earlier runs a new run looks through for results that still hold.
const KEEP_FROM: usize = 6;

/// The shards that passed in an earlier run keep their result if they run the same steps and
/// `holds` says their inputs did not change since; how many.
pub fn resume(state: &mut [CiShard], prior: &CiRun, holds: &dyn Fn(&str) -> bool) -> usize {
    let mut kept = 0;
    for s in state.iter_mut().filter(|s| s.state == "wait") {
        if let Some(p) = prior.shards.iter().find(|p| p.id == s.id && p.state == "ok" && p.sig == s.sig && holds(&p.id)) {
            let log = std::mem::take(&mut s.log);
            *s = p.clone();
            if p.log.is_empty() {
                s.log = log;
            }
            // its time was another run's
            s.spans.clear();
            s.lost.clear();
            if s.kept.is_empty() {
                s.kept = prior.pair.short();
            }
            kept += 1;
        }
    }
    kept
}

/// What a shard reads besides its own crates: the JIT that runs it (not devhub, which only comes
/// along in its binary), the scripts around it, the workspace's manifest and lock.
const SHARED_PATHS: [&str; 5] = ["Cargo.lock", "Cargo.toml", "tools/ci", "tools/ci_leg", "tools/run"];
/// The crate whose closure the JIT is.
const JIT_DIR: &str = "tools/run";
/// Not read by any shard through the JIT.
const NOT_JIT: [&str; 2] = ["libs/devhub", "apps/devhub"];

/// Which shards' inputs a range of landings left alone.
struct Inputs {
    repo: std::path::PathBuf,
    changed: Vec<String>,
    jit_changed: bool,
    closures: Mutex<BTreeMap<String, BTreeSet<String>>>,
}

fn under(path: &str, dir: &str) -> bool {
    path == dir || (path.starts_with(dir) && path.as_bytes().get(dir.len()) == Some(&b'/'))
}

impl Inputs {
    /// None when the range cannot be read.
    fn between(ctx: &Ctx, from: &Pair, to: &Pair) -> Option<Inputs> {
        let mut changed: Vec<String> = super::git(&ctx.repo, &["diff", "--name-only", &from.work, &to.work]).ok()?.lines().map(str::to_string).collect();
        changed.extend(super::git(&ctx.commercial(), &["diff", "--name-only", &from.main, &to.main]).ok()?.lines().map(|p| format!("{COMMERCIAL}/{p}")));
        let jit = super::roll::crate_dirs(&ctx.repo, JIT_DIR);
        let read_by_jit = |p: &String| jit.iter().any(|d| under(p, d)) && !NOT_JIT.iter().any(|d| under(p, d));
        let jit_changed = changed.iter().any(|p| read_by_jit(p) || SHARED_PATHS.iter().any(|d| under(p, d)));
        Some(Inputs { repo: ctx.repo.clone(), changed, jit_changed, closures: Mutex::new(BTreeMap::new()) })
    }

    /// Whether what `shard` reads is the same at both ends of the range.
    fn holds(&self, shard: &Shard) -> bool {
        let Some(dirs) = shard.dirs.as_ref().filter(|_| !self.jit_changed) else { return false };
        let mut closures = self.closures.lock().unwrap();
        dirs.iter().all(|d| {
            let c = closures.entry(d.clone()).or_insert_with(|| super::roll::crate_dirs(&self.repo, d));
            !self.changed.iter().any(|p| c.iter().any(|d| under(p, d)))
        })
    }
}

/// A preflight a previous master left goes on (a lane waits for its verdict); this holds the one
/// run's slot before returning, so verify's run comes after it. What verify runs goes on when verify
/// asks for its pair again ([`gate`]).
pub fn start(ctx: &Arc<Ctx>) {
    let Some(pair) = ctx.store.read(|d| d.ci.run.as_ref().filter(|r| r.dry).map(|r| r.pair.clone())) else { return };
    let (held, slot) = std::sync::mpsc::channel();
    let c = ctx.clone();
    let _ = std::thread::Builder::new().name("devhub-ci".into()).spawn(move || {
        let one = GATE.lock().unwrap_or_else(|e| e.into_inner());
        let _ = held.send(());
        let _ = gate_held(one, &c, &pair, true, None, Arc::new(|| STOP.load(std::sync::atomic::Ordering::SeqCst)));
    });
    let _ = slot.recv();
}

/// Runs the shards of `pair` over the free boxes (`only`: those of these ids, behind everything
/// else); the verdict and the shards as verify legs. `stop` gives the run up (shards running finish
/// their lease; nothing new starts).
pub fn gate(ctx: &Arc<Ctx>, pair: &Pair, dry: bool, only: Option<&[String]>, stop: Arc<dyn Fn() -> bool + Send + Sync>) -> (Verdict, Vec<crate::state::Leg>) {
    gate_held(GATE.lock().unwrap_or_else(|e| e.into_inner()), ctx, pair, dry, only, stop)
}

/// [`gate`] with the one run's slot in hand.
fn gate_held(_one: std::sync::MutexGuard<'static, ()>, ctx: &Arc<Ctx>, pair: &Pair, dry: bool, only: Option<&[String]>, stop: Arc<dyn Fn() -> bool + Send + Sync>) -> (Verdict, Vec<crate::state::Leg>) {
    if dry {
        YIELD.store(false, std::sync::atomic::Ordering::SeqCst);
    }
    // the run of this pair a previous master was in goes on; any other left in the state is over
    let going = ctx.store.update(|d| match d.ci.run.take() {
        Some(r) if r.pair == *pair && r.dry == dry => Some(r),
        Some(mut r) => {
            r.verdict = "incomplete".into();
            r.ended_ms = wall_ms();
            r.critical = ENDED_WITH_MASTER.into();
            d.ci.runs.push(r);
            None
        }
        None => None,
    });
    let id = going.as_ref().map_or_else(|| stamp(wall_ms()), |r| r.id.clone());
    let dir = ctx.logs("ci").join(&id);
    let _ = std::fs::create_dir_all(&dir);
    super::prune(&ctx.logs("ci"), 40);
    let rotation = match &going {
        Some(r) => r.rotation,
        None => ctx.store.update(|d| {
            d.ci.rotation += 1;
            d.ci.rotation
        }),
    };
    let tree = match Tree::at(&ctx.repo, pair) {
        Ok(t) => t,
        Err(e) => {
            ctx.store.event("ci", Sev::Warn, format!("ci {}: the tree: {e}", pair.short()), None);
            return (Verdict::Incomplete, Vec::new());
        }
    };
    let mut shards = ctx.store.read(|d| plan(&tree, &|s| d.ci.timing(s), rotation));
    if let Some(only) = only {
        shards.retain(|s| only.contains(&s.id));
    }
    let state: Vec<CiShard> = shards
        .iter()
        .map(|s| CiShard { id: s.id.clone(), state: "wait".into(), log: dir.join(format!("{}.log", safe(&s.id))).to_string_lossy().into_owned(), sig: s.sig(), ..Default::default() })
        .collect();
    let mut state = state;
    // where the going run was: its results and the steps on their boxes (a plan that changed under
    // a shard starts it over); its shards' leases come back with the master
    let mut back: Vec<usize> = Vec::new();
    let mut stray: Vec<String> = Vec::new();
    if let Some(r) = &going {
        for (i, s) in state.iter_mut().enumerate() {
            match r.shards.iter().find(|p| p.id == s.id && p.sig == s.sig) {
                Some(p) if matches!(p.state.as_str(), "ok" | "fail") => *s = p.clone(),
                Some(p) if p.state == "run" && p.run_n > 0 && super::leg::lease_held(ctx, &p.lease) => {
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
    // what passed in the runs before and still holds (the same pair: a run that ended incomplete)
    let priors: Vec<CiRun> = ctx.store.read(|d| d.ci.runs.iter().rev().take(KEEP_FROM).filter(|r| !r.shards.is_empty()).cloned().collect());
    let mut kept = 0;
    for r in &priors {
        let inputs = if r.pair == *pair { None } else { Inputs::between(ctx, &r.pair, pair) };
        let by_id = |id: &str| shards.iter().find(|s| s.id == id);
        kept += resume(&mut state, r, &|id| r.pair == *pair || inputs.as_ref().is_some_and(|x| by_id(id).is_some_and(|s| x.holds(s))));
    }
    let n = shards.len();
    let started = going.as_ref().map_or_else(wall_ms, |r| r.started_ms);
    ctx.store.update(|d| d.ci.run = Some(CiRun { id: id.clone(), pair: pair.clone(), dry, started_ms: started, verdict: "running".into(), shards: state.clone(), rotation, ..Default::default() }));
    for l in stray {
        let _ = crate::cli::ask(&makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-ci"), None, "release", &l);
    }
    let word = if going.is_some() { "goes on" } else { "started" };
    ctx.store.event("ci", Sev::Info, format!("ci {} {word}: {n} shards, {kept} kept, {} on their boxes (run {id})", pair.short(), back.len()), None);
    let max_active = boxes(ctx).len().saturating_sub(2).max(1);
    let prio = match (dry, only) {
        (true, _) => DRY_PRIO,
        (false, Some(_)) => super::verify::OLD_PAIR_PRIO,
        (false, None) => PRIO,
    };
    let pool = Arc::new((Mutex::new(Pool { reserved: vec![None; n], shards, state, stopped: false, prio, max_active }), Condvar::new()));
    let master_t0 = Instant::now();
    let mut nothing_left: Option<Instant> = None;
    let mut last_ran = Instant::now();
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
            if stop() || (!dry && YIELD.load(std::sync::atomic::Ordering::SeqCst)) {
                g.stopped = true;
            }
            if g.all_done() || g.stopped {
                cv.notify_all();
                break;
            }
            // a shard no box can take waits while anything else runs, and then for a box to come
            // back (a master just started has not heard its boxes yet: that is no box gone)
            let bs = boxes(ctx);
            g.max_active = bs.len().saturating_sub(2).max(1);
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
    let ended = wall_ms();
    let (crit, boxes, fleet) = critical(&g.state, started, ended);
    let failed: Vec<String> = g.state.iter().filter(|s| s.state == "fail").map(|s| s.id.clone()).collect();
    let word = match verdict {
        Verdict::Ok => "ok",
        Verdict::Fail => "fail",
        Verdict::Incomplete => "incomplete",
    };
    let state = g.state.clone();
    let one_box = one_box_failures(&state);
    let legs = state.iter().map(CiShard::leg).collect();
    ctx.store.update(|d| {
        if let Some(mut r) = d.ci.run.take() {
            r.shards = state;
            r.verdict = word.into();
            r.ended_ms = ended;
            r.critical = crit.clone();
            r.fleet = fleet.clone();
            r.boxes = boxes.clone();
            d.ci.runs.push(r);
        }
    });
    let line = format!(
        "ci {} {word}: {n} shards, {fleet}{}{}; critical path {crit}",
        pair.short(),
        if failed.is_empty() { String::new() } else { format!("; failed {}", failed.join(" ")) },
        if one_box.is_empty() { String::new() } else { format!("; FAILED ON ONE BOX ONLY: {}", one_box.join("; ")) }
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
fn worker(ctx: &Arc<Ctx>, pool: &Arc<(Mutex<Pool>, Condvar)>, pair: &Pair, _dir: &Path, b: BoxInfo) {
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
        let (shard, prio) = {
            let g = p.lock().unwrap();
            (g.shards[i].clone(), g.prio)
        };
        let spec = Spec {
            // each box asks for itself: a soft pin could fall back to a twin, also one that failed
            // the shard or called itself unfit for it
            only: true,
            lane: LANE.into(),
            agent: AGENT.into(),
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
        let held = match Held::acquire(&spec, &mut queued, &stop) {
            Ok(h) => h,
            Err(_) => {
                // busy or held: the shard goes back, this box rests a while
                let mut g = p.lock().unwrap();
                g.reserved[i] = None;
                g.state[i].spans.push(CiSpan { node: b.node.clone(), asked_ms, granted_ms: 0, ended_ms: wall_ms() });
                idle_until = Some(Instant::now() + BACKOFF);
                cv.notify_all();
                continue;
            }
        };
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
            publish(ctx, &g);
        }
        if run_shard(ctx, pool, i, held) {
            idle_until = Some(Instant::now() + BACKOFF);
        }
    }
}

/// A shard on its lease, from the step its state names, to its result; whether lane work took the
/// box back (the box rests a while: the shard waits again).
fn run_shard(ctx: &Ctx, pool: &(Mutex<Pool>, Condvar), i: usize, held: Held) -> bool {
    let (p, cv) = pool;
    let (shard, log_path, node) = {
        let g = p.lock().unwrap();
        (g.shards[i].clone(), g.state[i].log.clone(), g.state[i].node.clone())
    };
    let at = p.lock().unwrap().state[i].clone();
    let (state, summary) = run_steps(&held, &shard, Path::new(&log_path), &at, &mut |step, run_n, failed| {
        let mut g = p.lock().unwrap();
        let s = &mut g.state[i];
        s.step = step as u64;
        s.run_n = run_n;
        s.failed = failed.to_vec();
        publish(ctx, &g);
    });
    let lease = held.id.clone();
    held.release(Some(if state == "ok" { 0 } else { 1 }));
    let secs = {
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
    // lane work took the box back (the master preempts background work): the shard waits again
    if state == "incomplete" && super::leg::was_preempted(&lease) {
        let mut g = p.lock().unwrap();
        g.reserved[i] = None;
        g.state[i].state = "wait".into();
        g.state[i].summary = format!("preempted on {node} by lane work; asks again");
        publish(ctx, &g);
        cv.notify_all();
        return true;
    }
    let errors = if state == "fail" { super::ci_status::error_lines(&std::fs::read_to_string(&log_path).unwrap_or_default()) } else { Vec::new() };
    let bs = boxes(ctx);
    let mut g = p.lock().unwrap();
    g.reserved[i] = None;
    // a first failure asks a second box of that OS; its log stays beside the next one's
    if state == "fail" && g.second_box(i, &node, &bs, &summary, errors.clone()) {
        let _ = std::fs::rename(&log_path, format!("{log_path}.{node}"));
        publish(ctx, &g);
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
            g.state[i].summary = format!("passed on {node}; FAILED on {first}: {}", g.state[i].failed_why);
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
            ctx.store.update(|d| d.ci.measured(&id, secs));
        }
    }
    publish(ctx, &g);
    cv.notify_all();
    false
}

/// The steps of a shard in its lease, from the one `at` names (a run in flight there is followed
/// from the start of its output: a step's output reaches the log when it ends), their output
/// appended to its log: (state, summary). `note` hears the step, its run and what failed so far.
fn run_steps(held: &Held, shard: &Shard, log: &Path, at: &CiShard, note: &mut dyn FnMut(usize, u64, &[String])) -> (&'static str, String) {
    let first = at.step as usize;
    let file = if first == 0 && at.run_n == 0 { std::fs::File::create(log) } else { std::fs::OpenOptions::new().append(true).create(true).open(log) };
    let Ok(mut f) = file else { return ("incomplete", "cannot write its log".into()) };
    let mut failed = at.failed.clone();
    for (k, step) in shard.steps.iter().enumerate().skip(first) {
        let mut out: Vec<u8> = Vec::new();
        let r = if k == first && at.run_n > 0 {
            held.follow(at.run_n, 0, &mut out)
        } else {
            let _ = writeln!(f, "$ {}", step.join(" "));
            held.run_noted(step, &mut out, &mut |n| note(k, n, &failed))
        };
        let _ = f.write_all(&out);
        let text = String::from_utf8_lossy(&out);
        match r {
            // the step says this box cannot do it (ci_xcheck.sh: no std for the target)
            Ok(UNFIT) => {
                let _ = writeln!(f, "== exit {UNFIT}: this box cannot run it");
                let why = text.lines().rev().find(|l| !l.starts_with("makepad-run:")).unwrap_or("");
                return ("incomplete", format!("unfit: {}: {why}", step_name(step)));
            }
            Ok(code) => {
                let (ok, why) = judge(shard.judge, &step_name(step), code, &text);
                let _ = writeln!(f, "== exit {code}{}", if ok { String::new() } else { format!(": {why}") });
                if !ok {
                    failed.push(why);
                }
                note(k + 1, 0, &failed);
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
/// One CI run at a time (a CLI run, a preflight, verify's): the run's state is one slot.
static GATE: Mutex<()> = Mutex::new(());
/// A dry run (lane work) waits for the lock: a background run in progress gives up (running shards
/// finish); verify tries its pair again after.
static YIELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

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
            if GATE.try_lock().is_err() {
                // a background run yields to a dry run; two dry runs go one after the other
                if !dry || ctx.store.read(|d| d.ci.run.as_ref().is_none_or(|r| r.dry)) {
                    return Err("a CI run is going (devhub ci; devhub ci stop)".into());
                }
                YIELD.store(true, std::sync::atomic::Ordering::SeqCst);
            }
            STOP.store(false, std::sync::atomic::Ordering::SeqCst);
            let c = ctx.clone();
            let p = pair.clone();
            let _ = std::thread::Builder::new().name("devhub-ci".into()).spawn(move || {
                let _ = gate(&c, &p, dry, None, Arc::new(|| STOP.load(std::sync::atomic::Ordering::SeqCst)));
            });
            Ok(obj(vec![("started", s(pair.short())), ("dry", Value::Bool(dry))]).to_json())
        }
        Some("stop") => {
            STOP.store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(obj(vec![("stopping", Value::Bool(GATE.try_lock().is_err()))]).to_json())
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
    fn a_resumed_run_keeps_only_what_passed() {
        let sh = |id: &str, st: &str| CiShard { id: id.into(), state: st.into(), ..Default::default() };
        let prior = CiRun { shards: vec![sh("a", "ok"), sh("b", "fail"), sh("c", "run")], ..Default::default() };
        let mut now = vec![sh("a", "wait"), sh("b", "wait"), sh("c", "wait"), sh("d", "wait")];
        // a result holds only while its inputs do, and for the same steps
        let mut none = now.clone();
        assert_eq!(resume(&mut none, &prior, &|_| false), 0);
        assert_eq!(resume(&mut now, &prior, &|_| true), 1);
        assert_eq!(now.iter().map(|s| s.state.as_str()).collect::<Vec<_>>(), ["ok", "wait", "wait", "wait"]);
        let mut other = vec![CiShard { sig: "other".into(), ..sh("a", "wait") }];
        assert_eq!(resume(&mut other, &prior, &|_| true), 0);
    }

    #[test]
    fn a_failure_asks_a_second_box_and_a_box_failing_several_is_out() {
        let bx = |n: &str, os: Os| BoxInfo { node: n.into(), os, arch: "x86_64".into(), gui: true };
        let boxes = [bx("a", Os::Linux), bx("b", Os::Linux), bx("m", Os::Macos)];
        let shard = |id: &str| Shard { id: id.into(), kind: Kind::Test, os: Os::Any, gui: false, steps: Vec::new(), judge: Judge::Exit, est: 1.0, dirs: None };
        let n = BOX_OUT + 1;
        let mut p = Pool { shards: (0..n).map(|i| shard(&format!("s{i}"))).collect(), state: (0..n).map(|i| CiShard { id: format!("s{i}"), state: "wait".into(), ..Default::default() }).collect(), reserved: vec![None; n], stopped: false, prio: 0, max_active: 8 };
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
    fn a_later_pair_keeps_what_its_inputs_left_alone() {
        let shard = |dirs: Option<Vec<&str>>| Shard { id: "s".into(), kind: Kind::Test, os: Os::Linux, gui: false, steps: Vec::new(), judge: Judge::Exit, est: 1.0, dirs: dirs.map(|d| d.iter().map(|x| x.to_string()).collect()) };
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let inputs = |changed: &[&str], jit_changed: bool| Inputs { repo: repo.clone(), changed: changed.iter().map(|x| x.to_string()).collect(), jit_changed, closures: Mutex::new(BTreeMap::new()) };
        // its own crate, or a crate it depends on, changed
        assert!(!inputs(&["libs/devhub/src/x.rs"], false).holds(&shard(Some(vec!["libs/devhub"]))));
        assert!(!inputs(&["libs/remote/src/x.rs"], false).holds(&shard(Some(vec!["libs/devhub"]))));
        // something else did (a directory whose name starts the same is another directory)
        assert!(inputs(&["libs/devhubx/src/x.rs", "apps/wm/src/x.rs"], false).holds(&shard(Some(vec!["libs/devhub"]))));
        // the JIT changed, or the shard reads more than crates
        assert!(!inputs(&[], true).holds(&shard(Some(vec!["libs/devhub"]))));
        assert!(!inputs(&[], false).holds(&shard(None)));
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
        let sh = |os, gui| Shard { id: "s".into(), kind: Kind::Build, os, gui, steps: Vec::new(), judge: Judge::Exit, est: 1.0, dirs: None };
        assert!(lin.fits(&sh(Os::Linux, false)) && !lin.fits(&sh(Os::Any, true)) && !win.fits(&sh(Os::Any, true)) && win.fits(&sh(Os::Windows, false)));
        let shard = |id: &str, spans: Vec<(&str, u64, u64, u64)>| CiShard { id: id.into(), spans: spans.into_iter().map(|(n, a, g, e)| CiSpan { node: n.into(), asked_ms: a, granted_ms: g, ended_ms: e }).collect(), ..Default::default() };
        let state = vec![
            shard("s0", vec![("101", 1000, 1000, 31_000)]),
            // asked on 101, never granted: it went to 106
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
        let sh = Shard { id: "s".into(), kind: Kind::Build, os: Os::Macos, gui: false, steps: Vec::new(), judge: Judge::Exit, est: 1.0, dirs: None };
        let mac = BoxInfo { node: "mini".into(), os: Os::Macos, arch: "aarch64".into(), gui: true };
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
}
