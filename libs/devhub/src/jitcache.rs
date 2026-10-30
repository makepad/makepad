//! Clearing this machine's JIT cache: the run dirs `makepad run` keeps its compiled code in
//! (relax `dirs.rs`: `<target dir>/makepad-run` per workspace, `<run home>/target/<8 hex>` per git
//! run). The next start of an app on a cleared cache is cold.
//!
//! Which run dirs: the checkout devhub was built from, the git-run dirs under the run home, the
//! run dir every running JIT announces (`run_dir` in its discovery file) and the ones announced
//! before (remembered in `~/.makepad/devhub/jit-caches`). A dir is cleared only when it is what a
//! run dir is: a real folder (not a link) named `makepad-run` holding a `cache/` or `startup/`, or
//! an 8-hex folder directly under the run home's `target/`.
//!
//! What goes, by explicit name inside a run dir, and nothing else: `cache/` (the compiled stores:
//! code, front images, graph certificates, collected programs; the live locks of running processes
//! stay) and `startup/` (startup manifests). What stays: `registry/` (downloaded packages),
//! `git.lock` (dependency pins), `windows/` (the apps' window geometry), `build/` and
//! `build-scripts/` (build script outputs) and anything else; never `~/.cargo`, cargo's own
//! `target/` output, the run home's git source stores (`~/.makepad-run/cache`) or any source. One app (`app=<package>`): its
//! startup manifest and its own crate's stores (`<crate>-<16 hex>.rcs` and their generations);
//! the stores shared with other apps stay, so only its own crate compiles again.
//!
//! In use: a running process of the cache holds its live lock (`cache/live/<build>`), so the
//! check is the lock itself (every `makepad run`, `check`, `test`, announced or not); the
//! announced JITs name who. A clear of a cache in use is refused unless forced; forced, the files
//! go and the folders stay. A running JIT keeps what it mapped (unlinked files live on until
//! unmapped; JIT code is in memory and only grows) and writes new stores into the kept folders.
//!
//! Removal walks the tree with explicit paths: no shell, no glob, links removed, never followed.

use makepad_remote::discovery;
use makepad_remote::json_str;
use std::path::{Path, PathBuf};

/// What a run dir is, for the line that names it.
#[derive(Clone, Debug, PartialEq)]
pub enum Kind {
    /// `<target>/makepad-run`
    Workspace,
    /// `<run home>/target/<8 hex>` (a git repository run's virtual workspace)
    GitRun,
}

/// A running process on a cache: (pid, app).
pub type User = (u32, String);

#[derive(Clone, Debug)]
pub struct CachePlan {
    pub dir: PathBuf,
    pub kind: Kind,
    /// files and bytes a clear removes (the scope's)
    pub files: u64,
    pub bytes: u64,
    /// bytes that stay (downloads, window geometry, ...)
    pub kept_bytes: u64,
    /// the announced JITs on it
    pub users: Vec<User>,
    /// live locks held (running processes of this cache, announced or not)
    pub held: usize,
}

impl CachePlan {
    pub fn in_use(&self) -> bool {
        self.held > 0 || !self.users.is_empty()
    }

    fn json(&self) -> String {
        let users: Vec<String> = self.users.iter().map(|(p, a)| format!("{{\"pid\":{p},\"app\":{}}}", json_str(a))).collect();
        format!(
            "{{\"dir\":{},\"kind\":\"{}\",\"files\":{},\"bytes\":{},\"kept_bytes\":{},\"held\":{},\"users\":[{}]}}",
            json_str(&self.dir.display().to_string()),
            if self.kind == Kind::Workspace { "workspace" } else { "git" },
            self.files,
            self.bytes,
            self.kept_bytes,
            self.held,
            users.join(",")
        )
    }
}

/// The entries of a run dir a clear removes; everything else stays.
pub const CLEARED: &[&str] = &["cache", "startup"];

fn run_home() -> PathBuf {
    std::env::var_os("MAKEPAD_RUN_HOME").filter(|h| !h.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" }).map(PathBuf::from).unwrap_or_default();
        home.join(".makepad-run")
    })
}

fn remembered_file() -> PathBuf {
    crate::proto::home().join("jit-caches")
}

/// Records a run dir a JIT announced (once), so a later clear finds it after the JIT ended.
pub fn remember(dir: &str) {
    let dir = dir.trim();
    if dir.is_empty() || dir.contains('\n') {
        return;
    }
    let f = remembered_file();
    let text = std::fs::read_to_string(&f).unwrap_or_default();
    if text.lines().any(|l| l == dir) {
        return;
    }
    let _ = std::fs::create_dir_all(crate::proto::home());
    use std::io::Write;
    if let Ok(mut out) = std::fs::OpenOptions::new().create(true).append(true).open(&f) {
        let _ = writeln!(out, "{dir}");
    }
}

/// The announced JITs of this machine: (pid, app, run dir). The registry is read without
/// `discovery::apps` (which removes the files of ended processes: the hub's crash detection
/// needs them).
fn announced() -> Vec<(u32, String, Option<PathBuf>)> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(discovery::registry_dir()) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let Some(pid) = name.strip_prefix("makepad-remote-").and_then(|r| r.strip_suffix(".json")).and_then(|p| p.parse::<u32>().ok()) else { continue };
        if !crate::procinfo::alive(pid) {
            continue;
        }
        let Some(app) = std::fs::read_to_string(e.path()).ok().and_then(|t| discovery::parse(&t)) else { continue };
        if app.text("run").is_none() {
            continue;
        }
        // a run that predates `run_dir`: the workspace it was started in (Linux), found upwards
        let dir = app.text("run_dir").filter(|d| !d.is_empty()).map(PathBuf::from).or_else(|| below_cwd(pid));
        out.push((pid, app.app, dir));
    }
    out.sort_by_key(|j| j.0);
    out
}

/// `<folder>/target/makepad-run` of the nearest folder at or above the process's working
/// directory that has one (Linux: `/proc/<pid>/cwd`).
fn below_cwd(pid: u32) -> Option<PathBuf> {
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd")).ok()?;
    cwd.ancestors().map(|a| a.join("target").join("makepad-run")).find(|d| d.is_dir())
}

/// `dir` as a run dir (its real path), or why it is not one.
pub fn checked(dir: &Path) -> Result<(PathBuf, Kind), String> {
    let meta = std::fs::symlink_metadata(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if !meta.file_type().is_dir() {
        return Err(format!("{} is not a folder (a link or a file)", dir.display()));
    }
    let real = std::fs::canonicalize(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let name = real.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name == "makepad-run" {
        if !(real.join("cache").is_dir() || real.join("startup").is_dir()) {
            return Err(format!("{} holds no cache/ or startup/: not a run dir", real.display()));
        }
        return Ok((real, Kind::Workspace));
    }
    let git_runs = std::fs::canonicalize(run_home().join("target")).ok();
    if name.len() == 8 && name.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()) && real.parent() == git_runs.as_deref() {
        return Ok((real, Kind::GitRun));
    }
    Err(format!("{} is neither a `makepad-run` folder nor a git run's dir under {}", real.display(), run_home().join("target").display()))
}

/// Every run dir of this machine (checked, each once).
pub fn caches() -> Vec<(PathBuf, Kind)> {
    let mut cand: Vec<PathBuf> = Vec::new();
    // the checkout devhub was built from (libs/devhub is two folders below its root), and the
    // one it runs in
    cand.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/makepad-run"));
    if let Ok(cwd) = std::env::current_dir() {
        cand.push(cwd.join("target").join("makepad-run"));
    }
    if let Ok(rd) = std::fs::read_dir(run_home().join("target")) {
        cand.extend(rd.flatten().map(|e| e.path()));
    }
    for (_, _, dir) in announced() {
        cand.extend(dir);
    }
    cand.extend(std::fs::read_to_string(remembered_file()).unwrap_or_default().lines().filter(|l| !l.trim().is_empty()).map(PathBuf::from));
    let mut out: Vec<(PathBuf, Kind)> = Vec::new();
    for c in cand {
        if let Ok((real, kind)) = checked(&c) {
            if !out.iter().any(|(d, _)| *d == real) {
                out.push((real, kind));
            }
        }
    }
    out
}

/// The live locks of `dir` that a running process holds.
pub(crate) fn held_locks(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir.join("cache").join("live")).into_iter().flatten().flatten() {
        let p = e.path();
        let Ok(f) = std::fs::OpenOptions::new().read(true).open(&p) else { continue };
        // a process of this cache holds it shared while it runs (relax store.rs hold_build)
        if f.try_lock().is_err() {
            out.push(p);
        }
    }
    out
}

/// The package names `app` may mean (`stage` is `makepad-app-stage`).
fn app_names(app: &str) -> Vec<String> {
    let mut v = vec![app.to_string()];
    if !app.starts_with("makepad-") {
        v.push(format!("makepad-app-{app}"));
    }
    v
}

/// One app's files in run dir `dir`: its startup manifest and its crate's own stores.
fn app_files(dir: &Path, app: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for name in app_names(app) {
        let s = dir.join("startup").join(format!("{name}.startup"));
        if s.is_file() {
            out.push(s);
        }
        let prefix = format!("{}-", name.replace('-', "_"));
        for e in std::fs::read_dir(dir.join("cache")).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            // `<crate>-<16 hex>.rcs` or a generation of it (`.rcs.<n>`)
            let base = match n.rsplit_once('.') {
                Some((b, g)) if b.ends_with(".rcs") && g.bytes().all(|c| c.is_ascii_digit()) && !g.is_empty() => b.to_string(),
                _ => n.clone(),
            };
            let Some(hex) = base.strip_prefix(&prefix).and_then(|r| r.strip_suffix(".rcs")) else { continue };
            if hex.len() == 16 && hex.bytes().all(|b| b.is_ascii_hexdigit()) && e.file_type().is_ok_and(|t| t.is_file()) {
                out.push(e.path());
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

/// Files and bytes below `p` (links counted as files of no size, never followed).
fn tree_size(p: &Path) -> (u64, u64) {
    let Ok(m) = std::fs::symlink_metadata(p) else { return (0, 0) };
    if !m.file_type().is_dir() {
        return (1, if m.file_type().is_file() { m.len() } else { 0 });
    }
    let mut t = (0, 0);
    for e in std::fs::read_dir(p).into_iter().flatten().flatten() {
        let (f, b) = tree_size(&e.path());
        t.0 += f;
        t.1 += b;
    }
    t
}

/// What a clear of `dir` (one app's files, or the whole cache) would remove.
fn plan_one(dir: &Path, kind: Kind, app: Option<&str>, jits: &[(u32, String, Option<PathBuf>)]) -> CachePlan {
    let held = held_locks(dir).len();
    let users: Vec<User> = jits
        .iter()
        .filter(|(_, _, d)| d.as_ref().and_then(|d| std::fs::canonicalize(d).ok()).as_deref() == Some(dir))
        .filter(|(_, a, _)| app.is_none_or(|want| app_names(want).iter().any(|n| n == a)))
        .map(|(p, a, _)| (*p, a.clone()))
        .collect();
    let total = tree_size(dir).1;
    let (files, bytes) = match app {
        Some(a) => app_files(dir, a).iter().fold((0, 0), |t, f| (t.0 + 1, t.1 + std::fs::metadata(f).map_or(0, |m| m.len()))),
        None => CLEARED.iter().fold((0, 0), |t, n| {
            let (f, b) = tree_size(&dir.join(n));
            (t.0 + f, t.1 + b)
        }),
    };
    // for one app, a held lock is another app's run as much as its own: only its announced runs count
    let held = if app.is_some() { 0 } else { held };
    CachePlan { dir: dir.to_path_buf(), kind, files, bytes, kept_bytes: total.saturating_sub(bytes), users, held }
}

/// What a clear would remove on this machine: one plan per run dir (`app`: only its files).
pub fn plan(app: Option<&str>) -> Vec<CachePlan> {
    let jits = announced();
    let mut out: Vec<CachePlan> = caches().into_iter().map(|(d, k)| plan_one(&d, k, app, &jits)).collect();
    if app.is_some() {
        out.retain(|p| p.files > 0 || !p.users.is_empty());
    }
    out
}

#[derive(Default, Debug)]
pub struct Removal {
    pub files: u64,
    pub bytes: u64,
    pub skipped: Vec<(PathBuf, String)>,
}

impl Removal {
    fn file(&mut self, p: &Path) {
        let Ok(m) = std::fs::symlink_metadata(p) else { return };
        match std::fs::remove_file(p) {
            Ok(()) => {
                self.files += 1;
                self.bytes += if m.file_type().is_file() { m.len() } else { 0 };
            }
            Err(e) => self.skipped.push((p.to_path_buf(), e.to_string())),
        }
    }

    /// Removes everything below `d`, depth first, links removed and never followed; `keep_dirs`:
    /// the folders stay (a running process writes into them), as do the files in `keep`.
    fn tree(&mut self, d: &Path, keep_dirs: bool, keep: &[PathBuf]) {
        let entries: Vec<PathBuf> = match std::fs::read_dir(d) {
            Ok(rd) => rd.flatten().map(|e| e.path()).collect(),
            Err(e) => {
                if std::fs::symlink_metadata(d).is_ok() {
                    self.skipped.push((d.to_path_buf(), e.to_string()));
                }
                return;
            }
        };
        for p in entries {
            match std::fs::symlink_metadata(&p) {
                Ok(m) if m.file_type().is_dir() => {
                    self.tree(&p, keep_dirs, keep);
                    if !keep_dirs {
                        if let Err(e) = std::fs::remove_dir(&p) {
                            if !self.skipped.iter().any(|(s, _)| s.starts_with(&p)) {
                                self.skipped.push((p, e.to_string()));
                            }
                        }
                    }
                }
                Ok(_) if keep.contains(&p) => {}
                Ok(_) => self.file(&p),
                Err(_) => {}
            }
        }
    }
}

/// The outcome of clearing one run dir.
#[derive(Debug)]
pub struct Cleared {
    pub dir: PathBuf,
    pub removal: Removal,
    /// it ran a process: only stores and startup manifests went
    pub in_use: bool,
}

/// Clears one checked run dir (`app`: only its files). The caller checked `in_use` / `force`.
fn clear_one(p: &CachePlan, app: Option<&str>) -> Cleared {
    let mut r = Removal::default();
    let in_use = p.in_use() || !held_locks(&p.dir).is_empty();
    match app {
        Some(a) => {
            for f in app_files(&p.dir, a) {
                r.file(&f);
            }
        }
        None => {
            let held = held_locks(&p.dir);
            for n in CLEARED {
                let d = p.dir.join(n);
                if std::fs::symlink_metadata(&d).is_ok_and(|m| m.file_type().is_dir()) {
                    r.tree(&d, in_use, &held);
                    if !in_use {
                        if let Err(e) = std::fs::remove_dir(&d) {
                            if !r.skipped.iter().any(|(s, _)| s.starts_with(&d)) {
                                r.skipped.push((d, e.to_string()));
                            }
                        }
                    }
                }
            }
        }
    }
    Cleared { dir: p.dir.clone(), removal: r, in_use }
}

pub fn human(b: u64) -> String {
    if b >= 1 << 30 {
        format!("{:.1} GB", b as f64 / (1u64 << 30) as f64)
    } else {
        format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
    }
}

fn log_file() -> PathBuf {
    crate::proto::home().join("jitcache.log")
}

fn log(line: &str) {
    eprintln!("{line}");
    use std::io::Write;
    let _ = std::fs::create_dir_all(crate::proto::home());
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(log_file()) {
        let _ = writeln!(f, "{} {line}", crate::proto::wall_ms() / 1000);
    }
}

/// `jitcache plan|clear [app=<package>] [dir=<run dir>] [force]`: the `jitcache` service of a hub (its own
/// machine's caches). Plan lines are [`CachePlan`] JSON; a clear answers one line per cache and a
/// summary `{"freed":…}`, and publishes the summary as a hub record.
pub fn service(machine: &str, line: &str, out: &mut Vec<String>, publish: &dyn Fn(crate::proto::Record)) {
    let mut words = line.split_whitespace().skip_while(|w| *w == "jitcache");
    let verb = words.next().unwrap_or("plan");
    let (mut app, mut force, mut only) = (None, false, None);
    for w in words {
        match w {
            "force" => force = true,
            w if w.starts_with("dir=") && w.len() > 4 => match checked(Path::new(&w[4..])) {
                Ok(d) => only = Some(d),
                Err(e) => return out.push(format!("{{\"err\":{}}}", json_str(&format!("jitcache: {e}")))),
            },
            w if w.starts_with("app=") && w.len() > 4 => app = Some(w[4..].to_string()),
            _ => return out.push(format!("{{\"err\":{}}}", json_str(&format!("jitcache: unknown word `{}`", crate::proto::clean(w, 64))))),
        }
    }
    if let Some(a) = &app {
        if a.len() > 128 || !a.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
            return out.push(format!("{{\"err\":{}}}", json_str("jitcache: app= is a package name")));
        }
    }
    let mut plans = plan(app.as_deref());
    if let Some((d, kind)) = &only {
        plans.retain(|p| p.dir == *d);
        // a run dir named outright counts even when nothing announced it (no JIT runs on it now)
        if plans.is_empty() {
            plans.push(plan_one(d, kind.clone(), app.as_deref(), &announced()));
        }
    }
    match verb {
        "plan" => {
            out.extend(plans.iter().map(CachePlan::json));
            let bytes: u64 = plans.iter().map(|p| p.bytes).sum();
            out.push(format!("{{\"total_bytes\":{bytes},\"caches\":{},\"in_use\":{}}}", plans.len(), plans.iter().any(CachePlan::in_use)));
        }
        "clear" => {
            let busy: Vec<&CachePlan> = plans.iter().filter(|p| p.in_use()).collect();
            if !busy.is_empty() && !force {
                out.extend(plans.iter().map(CachePlan::json));
                return out.push(format!("{{\"err\":{}}}", json_str(&format!("in use by {}: pass force to clear it under them (they keep running)", who(&busy)))));
            }
            let scope = app.as_deref().map_or("all".to_string(), |a| format!("app {a}"));
            let (mut freed, mut files, mut skipped) = (0u64, 0u64, 0usize);
            for p in &plans {
                let c = clear_one(p, app.as_deref());
                freed += c.removal.bytes;
                files += c.removal.files;
                skipped += c.removal.skipped.len();
                log(&format!("devhub jitcache: {}: removed {} files, {} from cache/ and startup/ ({scope}{})", c.dir.display(), c.removal.files, human(c.removal.bytes), if c.in_use { "; in use: folders and live locks kept" } else { "" }));
                for (s, e) in c.removal.skipped.iter().take(20) {
                    log(&format!("devhub jitcache:   skipped {} ({e})", s.display()));
                }
                out.push(format!(
                    "{{\"dir\":{},\"removed_files\":{},\"freed\":{},\"skipped\":{},\"in_use\":{}}}",
                    json_str(&c.dir.display().to_string()),
                    c.removal.files,
                    c.removal.bytes,
                    c.removal.skipped.len(),
                    c.in_use
                ));
            }
            let summary = format!(
                "jitcache: cleared {} cache{} ({scope}), freed {} in {files} files{}; the next start{} cold{}",
                plans.len(),
                if plans.len() == 1 { "" } else { "s" },
                human(freed),
                if skipped > 0 { format!(", {skipped} skipped (in use)") } else { String::new() },
                if app.is_some() { " of that app is" } else { " of every app here is" },
                if busy.is_empty() { String::new() } else { format!("; {} kept running", who(&busy)) }
            );
            log(&format!("devhub {summary}"));
            publish(crate::proto::hub_record(machine, app.as_deref().unwrap_or(""), 0, "jitcache", crate::proto::Sev::Info, summary.clone(), String::new()));
            out.push(format!("{{\"freed\":{freed},\"files\":{files},\"skipped\":{skipped},\"caches\":{},\"line\":{}}}", plans.len(), json_str(&summary)));
        }
        v => out.push(format!("{{\"err\":{}}}", json_str(&format!("jitcache {}: plan or clear", crate::proto::clean(v, 32))))),
    }
}

/// The processes on the busy caches, for a line: `stage#4412, makepad-app-x#981`.
fn who(busy: &[&CachePlan]) -> String {
    let mut names: Vec<String> = busy.iter().flat_map(|p| p.users.iter().map(|(pid, a)| format!("{a}#{pid}"))).collect();
    names.dedup();
    // a held lock with no announced JIT: a `makepad check` / `test`, or a run that does not
    // announce its run dir
    if busy.iter().any(|p| p.held > 0 && p.users.is_empty()) {
        names.push("a makepad run/check/test process".into());
    }
    if names.is_empty() { "a running process".into() } else { names.join(", ") }
}
