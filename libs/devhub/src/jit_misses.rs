//! Compile/resolve misses: cargo supplies the verdict, the master remembers which JIT targets
//! to skip until their miss is closed or the JIT's inputs changed since its last failure.
//! The JSONL ledger has one current row per diagnostic.

use makepad_strict_json::{obj, s, Value};
use std::path::Path;
use std::sync::Mutex;

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Diagnostics may come from a coloured cargo launcher, the JIT's short format, or JSON.
pub(crate) fn plain(line: &str) -> String {
    let mut out = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' && chars.clone().next() == Some('[') {
            chars.next();
            for c in chars.by_ref() { if ('@'..='~').contains(&c) { break; } }
        } else { out.push(c); }
    }
    out
}

pub(crate) fn running_tests(line: &str) -> bool {
    let mut words = line.split_whitespace();
    words.next() == Some("running") && words.next().is_some_and(|n| n.parse::<u64>().is_ok())
        && matches!(words.next(), Some("test" | "tests")) && words.next().is_none()
}

fn summary_line(line: &str) -> bool {
    line.starts_with("error: could not compile")
        || line.starts_with("makepad-run:") && (line.contains("unresolved imports") || line.contains("signature errors") || line.contains("load error"))
}

/// The JIT's own word that it could not build a target: `makepad test`'s `error: could not compile ..`
/// summary or a makepad-run load failure. Test output never says this.
pub(crate) fn build_failed(log: &str) -> bool {
    plain(log).lines().map(str::trim).any(summary_line)
}

/// Only output outside a target's harness window (`running N tests` .. `test result:`) can
/// be a compile miss. A package run builds and runs one target after another, so one target's
/// compile error counts even when other targets ran. Diagnostics in a failing test's captured
/// output inside its window never turn that test into a JIT miss; a test's own uncaptured
/// `test result:` text does not end the window.
/// The JIT's live-edit reports (`makepad live:` lines) and restart reasons (also with a prefix a
/// log window cut off) reach devhub through its patch events: never a build's compile errors (a
/// restart reason's edit is applied by the warm restart). Neither are a run's crash reports
/// (`report: {..}`): a failing assertion is the program's own verdict.
pub(crate) fn compile_errors(log: &str) -> Vec<String> {
    let log = plain(log);
    let mut errors: Vec<String> = Vec::new();
    let mut summary = Vec::new();
    let mut harness = false;
    for line in log.lines().map(str::trim) {
        if running_tests(line) { harness = true; continue; }
        if harness {
            if line.starts_with("test result: ok.") || line.starts_with("test result: FAILED.") { harness = false; }
            continue;
        }
        if line.starts_with("makepad live:") || line.contains("restart the app") || app_log(line) { continue; }
        // A run's crash report (a test's panic, a fault): the program's verdict, never a build's.
        if line.starts_with("report: {") || line.contains(" report: {") || line.starts_with("report (caught): {") { continue; }
        if let Ok(v) = makepad_strict_json::parse(line.as_bytes()) {
            if text(&v, "severity") == "error" {
                let file = text(&v, "file");
                let message = text(&v, "message").lines().next().unwrap_or("");
                errors.push(if file.is_empty() { message.to_string() } else {
                    format!("{file}:{}:{}: {message}", v.get("line").and_then(Value::as_u64).unwrap_or(0), v.get("col").and_then(Value::as_u64).unwrap_or(0))
                });
                continue;
            }
        }
        if let Some(location) = line.strip_prefix("--> ") {
            if let Some(error) = errors.last_mut().filter(|e| e.starts_with("error")) { *error = format!("{location}: {error}"); }
            continue;
        }
        if summary_line(line) || line.starts_with("error: aborting") {
            summary.push(line.to_string());
        // a filter that matched no test: the run's verdict, not a build's (makepad test)
        } else if line.starts_with("error: test failed") || line.starts_with("error: no test matched") || line.contains("panicked at") || line.contains("internal compiler error") || line.contains("makepad run bug") || line.contains("jit panic") || line.contains("jit ice")
            || line.ends_with("restart the app") // A live restart notice, not a build error.
        {
            continue;
        } else if located_note(line) {
            continue;
        } else if line.starts_with("error:") || line.starts_with("error[")
            || line.contains(": error:") || line.contains(": error[")
            || line.split_once(".rs:").is_some_and(|(_, rest)| {
                let Some((line, message)) = rest.split_once(':') else { return false };
                line.parse::<u64>().is_ok() && !message.trim().is_empty()
                    && message.trim().trim_end_matches(':').parse::<u64>().is_err()
            }) {
            errors.push(line.to_string());
        }
    }
    if errors.is_empty() { summary } else { errors }
}

/// A line of the program's own log (`log!`, `error!`: `[I] <file>:<line>:<col> - <message>`, and
/// `[E]`, `[W]`, `[!]`, `[.]`): what the app said while it ran, never a build's error. An app's
/// `[I]` line at a source line read as one, and its runs went to cargo as JIT misses (M75-M77, 11-07).
fn app_log(line: &str) -> bool {
    ["[I] ", "[E] ", "[W] ", "[!] ", "[.] "].iter().any(|p| line.starts_with(p))
}

/// A located diagnostic that is no error: `<file>.rs:<line>:<col>: warning|note|help: ..`. A JIT
/// warning (`unused imports`) read as a compile error sent a failing test run to cargo as a JIT
/// miss (M71, 11-07).
fn located_note(line: &str) -> bool {
    let Some((_, rest)) = line.split_once(".rs:") else { return false };
    let mut text = rest;
    for _ in 0..2 {
        match text.split_once(':') {
            Some((n, after)) if n.trim().parse::<u64>().is_ok() => text = after,
            _ => break,
        }
    }
    let text = text.trim_start();
    ["warning", "note", "help"].iter().any(|k| text.strip_prefix(k).is_some_and(|after| after.starts_with(':') || after.starts_with('[')))
}

/// The spelling distinguishes `--lib` from a test target actually named `lib`.
pub(crate) fn target(args: &[String]) -> String {
    let select = &args[..args.iter().position(|a| a == "--").unwrap_or(args.len())];
    for (i, arg) in select.iter().enumerate() {
        match arg.as_str() {
            "--lib" | "--tests" => return arg.clone(),
            "--bin" => return format!("--bin {}", select.get(i + 1).map_or("", String::as_str)),
            "--test" => return format!("--test {}", select.get(i + 1).map_or("", String::as_str)),
            _ => {}
        }
    }
    "default".into()
}

struct Miss {
    id: String,
    pkg: String,
    target: String,
    error: Vec<String>,
    lane: String,
    pair: crate::state::Pair,
    first_ms: u64,
    last_ms: u64,
    count: u64,
    closed_ms: u64,
    /// how to see it again, for one filed by hand (`jit miss add --repro`)
    repro: String,
    /// what its package's build read when the JIT last failed on it ([`crate_inputs`]): other
    /// inputs, another JIT try
    inputs: String,
}

impl Miss {
    fn value(&self) -> Value {
        obj(vec![
            ("id", s(&self.id)), ("pkg", s(&self.pkg)), ("target", s(&self.target)),
            ("error", Value::Arr(self.error.iter().map(|e| s(e)).collect())),
            ("lane", s(&self.lane)), ("pair", obj(vec![("work", s(&self.pair.work)), ("main", s(&self.pair.main))])),
            ("first_ms", Value::Int(self.first_ms as i64)), ("last_ms", Value::Int(self.last_ms as i64)),
            ("count", Value::Int(self.count as i64)), ("closed_ms", Value::Int(self.closed_ms as i64)), ("repro", s(&self.repro)),
            ("inputs", s(&self.inputs)),
        ])
    }

    fn parse(v: &Value) -> Result<Self, String> {
        let id = text(v, "id");
        let errors: Vec<String> = v.get("error").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect();
        if id.strip_prefix('M').and_then(|s| s.parse::<u64>().ok()).is_none() || errors.is_empty() {
            return Err("jit misses: invalid ledger row".into());
        }
        let n = |k| v.get(k).and_then(Value::as_u64).unwrap_or(0);
        let pair = v.get("pair").map(|p| crate::state::Pair::new(text(p, "work"), text(p, "main"))).unwrap_or_default();
        Ok(Self { id: id.into(), pkg: text(v, "pkg").into(), target: text(v, "target").into(), error: errors, lane: text(v, "lane").into(), pair, first_ms: n("first_ms"), last_ms: n("last_ms"), count: n("count"), closed_ms: n("closed_ms"), repro: text(v, "repro").into(), inputs: text(v, "inputs").into() })
    }

    fn touch(&mut self, req: &Value, now: u64, set_pair: bool) -> Result<(), String> {
        let pin = crate::lease::Pin::of_rev(text(req, "rev")).ok_or("jit misses: missing commit pair")?;
        if set_pair { self.pair = pin.pair(); }
        self.lane = text(req, "lane").into();
        self.last_ms = now;
        self.count += 1;
        self.closed_ms = 0;
        Ok(())
    }
}

pub(crate) fn notice(v: &Value) -> String {
    let error = v.get("error").and_then(Value::as_arr).and_then(|a| a.first()).and_then(Value::as_str).unwrap_or("");
    // (the line is the JIT's; cargo's part is that the package builds: "this verdict is cargo's"
    // after the JIT's line read as cargo printing it, 11-07)
    format!("devhub test: JIT miss on {} {} (recorded {}): the JIT said {error}; cargo builds it", text(v, "pkg"), text(v, "target"), text(v, "id"))
}

/// Whether a package is JIT-own; an empty repo uses the local checkout.
pub(crate) fn jit_own(repo: &Path, pair: &crate::state::Pair, pkg: &str) -> Result<bool, String> {
    test_package(repo, pair, pkg).map(|(own, _)| own)
}

/// Whether a package is JIT-own, and why its tests run in the lane's tree, never a JIT overlay:
/// its `[package.metadata.makepad] test-tree` reason. An overlay holds the pinned commit alone,
/// at a root only the JIT's own process reads: a test that walks the checkout's history or runs
/// cargo on it failed there for want of them, never of its code (devhub's, 11-07). An empty
/// repo uses the local checkout.
pub(crate) fn test_package(repo: &Path, pair: &crate::state::Pair, pkg: &str) -> Result<(bool, Option<String>), String> {
    #[cfg(feature = "checkout")]
    {
        let repo = if repo.as_os_str().is_empty() { crate::orch::repo() } else { repo.to_path_buf() };
        Ok(package_of(&crate::orch::ci::Tree::at(&repo, pair)?, pkg))
    }
    #[cfg(not(feature = "checkout"))]
    {
        let _ = (repo, pair, pkg);
        Ok((false, None))
    }
}

/// The package a JIT `makepad test` command tests: the first word after `test` that is no option,
/// as makepad test reads it (relax test_pkg.rs: `-p` / `--package` only mark it; an option before
/// it may take a value).
pub(crate) fn tested_package(argv: &[String]) -> Option<&str> {
    let (first, rest) = crate::lease::loaders::cargo_run_args(argv)?.split_first()?;
    if first != "test" { return None; }
    let mut words = rest.iter();
    while let Some(w) = words.next() {
        match w.as_str() {
            "--" => return None,
            "--features" | "-F" | "--cfg" | "--report-to" | "--run-dir" | "--workspace-dir" => { words.next(); }
            w if w.starts_with('-') => {}
            pkg => return Some(pkg),
        }
    }
    None
}

/// What a lane's run of the JIT `makepad test` command `argv` says as it takes the lane's tree
/// rather than an overlay: its package's `test-tree` reason at `pair` ([`test_package`]). None:
/// the command tests no package that says it.
pub(crate) fn test_tree_of(repo: &Path, pair: &crate::state::Pair, argv: &[String]) -> Result<Option<String>, String> {
    let Some(pkg) = tested_package(argv) else { return Ok(None) };
    Ok(test_package(repo, pair, pkg)?.1.map(|why| tree_line(pkg, &why)))
}

/// A run that takes the lane's tree for its package's `test-tree` says so in these words.
pub(crate) fn tree_line(pkg: &str, why: &str) -> String {
    format!("{pkg} runs in the lane's tree: {why} (its Cargo.toml's test-tree)")
}

#[cfg(feature = "checkout")]
fn package_of(tree: &crate::orch::ci::Tree, pkg: &str) -> (bool, Option<String>) {
    use crate::orch::ci::{jit_own_dirs, package_dirs};
    use makepad_toml_parser::{parse_toml, Toml};
    let Some(dir) = package_dirs(tree).remove(pkg) else { return (false, None) };
    let manifest = if dir.is_empty() { "Cargo.toml".to_string() } else { format!("{dir}/Cargo.toml") };
    let why = tree.read(&manifest).and_then(|t| parse_toml(&t).ok()).and_then(|d| d.get_path(&["package", "metadata", "makepad", "test-tree"]).and_then(Toml::as_str).map(str::to_string));
    (jit_own_dirs(tree).contains(&dir), why)
}

/// The packages the JIT binary is built from.
#[cfg(feature = "checkout")]
pub(crate) fn jit_binary_packages(repo: &Path, pair: &crate::state::Pair) -> Result<std::collections::BTreeSet<String>, String> {
    use crate::orch::ci::{jit_binary_dirs, package_dirs, Tree};
    let repo = if repo.as_os_str().is_empty() { crate::orch::repo() } else { repo.to_path_buf() };
    let tree = Tree::at(&repo, pair)?;
    let dirs = jit_binary_dirs(&tree);
    Ok(package_dirs(&tree).into_iter().filter(|(_, dir)| dirs.contains(dir)).map(|(pkg, _)| pkg).collect())
}

/// The old CI refusal seeds a miss once. A closed row overrides the stale mark, allowing the
/// next attempt to reach the JIT; a real miss on that attempt reopens its diagnostic's row.
fn ci_error(home: &Path, pkg: &str) -> Option<Vec<String>> {
    let (pair, checkout, errors) = crate::state::Store::open_quiet(home, "").map_err(|e| eprintln!("devhub state: {e}")).ok()?.read(|d| {
        d.ci.timing(&format!("failing:{pkg}"))?;
        let id = format!("test:{pkg}");
        d.ci.run.iter().chain(d.ci.sweep_run.iter()).chain(d.ci.runs.iter().rev()).find_map(|r| {
            r.shards.iter().find(|s| s.state == "fail" && (s.id == id || s.id.starts_with(&format!("{id}:"))))
                .map(|s| (r.pair.clone(), r.checkout.clone(), s.errors.clone()))
        })
    })?;
    // a crate whose tests merely fail is no JIT miss: only compile or resolve errors seed one
    // (any failing mark did, and `devhub test` then answered with cargo's verdict). The
    // filtered sample lacks harness windows: require the JIT's build failure summary so a
    // test's own error lines cannot seed a miss
    let log = errors.join("\n");
    if !build_failed(&log) { return None; }
    let compile = compile_errors(&log);
    if compile.is_empty() { return None; }
    // JIT-own tests run under cargo; their compile errors cannot diagnose the JIT.
    if jit_own(Path::new(&checkout), &pair, pkg).ok()? { return None; }
    let mut errors = vec![format!("CI marks failing:{pkg}: the JIT could not build its tests")];
    errors.extend(compile.into_iter().take(4));
    Some(errors)
}

/// The id of an open miss of `pkg` (any target), if the ledger has one.
#[cfg(feature = "checkout")]
pub(crate) fn open(home: &Path, pkg: &str) -> Option<String> {
    std::fs::read_to_string(home.join("jit_misses.jsonl")).ok()?.lines()
        .filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok().and_then(|v| Miss::parse(&v).ok()))
        .find(|m| m.pkg == pkg && m.closed_ms == 0).map(|m| m.id)
}

/// The lane's base on work, when work changed the JIT since: the JIT of the lane's pin (the one
/// its tree needs) is older than work's, so its compile errors can be the lane's code using what
/// landed since (M54: an std API of #806 on a pin before it), not a JIT miss. None: the lane's JIT
/// is work's, or git cannot say.
#[cfg(feature = "checkout")]
pub(crate) fn behind_work_jit(repo: &Path, pin: &crate::state::Pair) -> Option<String> {
    let tip = crate::orch::snapshot(repo)?;
    let base = crate::orch::git(repo, &["merge-base", &pin.work, &tip.work]).ok()?.trim().to_string();
    if base == tip.work { return None; }
    crate::orch::ci::Inputs::between(repo, &crate::state::Pair::new(&base, &tip.main), &tip).filter(|i| i.jit_changed).map(|_| base)
}
#[cfg(not(feature = "checkout"))]
pub(crate) fn behind_work_jit(_: &Path, _: &crate::state::Pair) -> Option<String> { None }

/// What a package's build reads of the lane's trees, as one key (its miss's, [`Miss::inputs`]):
/// each file under its own and its path dependencies' dirs ([`crate::orch::roll::crate_dirs_from`])
/// and the workspace's root manifests, by git blob id, as the commits of `pin` hold them with the
/// lane's edits on top ([`crate::lease::ship::uncommitted`]: what ships with its runs). An edit
/// and its commit key the same. Err: the trees or the package cannot be read.
#[cfg(feature = "checkout")]
pub(crate) fn crate_inputs(repo: &Path, pin: &crate::lease::Pin, pkg: &str) -> Result<String, String> {
    use crate::orch::ci::{package_dirs, Tree};
    let pair = pin.pair();
    let tree = Tree::at(repo, &pair)?;
    let dir = package_dirs(&tree).remove(pkg).ok_or_else(|| format!("no package {pkg} at {}", pair.short()))?;
    let dirs = crate::orch::roll::crate_dirs_from(&dir, &|p| tree.read(p));
    let under = |p: &str| ["Cargo.toml", ".cargo/config.toml"].contains(&p) || dirs.iter().any(|d| d.is_empty() || p.strip_prefix(d.as_str()).is_some_and(|rest| rest.starts_with('/')));
    let (edited, deleted) = crate::lease::ship::uncommitted(repo, pin)?;
    let mut files: std::collections::BTreeMap<String, String> = tree.files.iter().filter(|f| under(f)).map(|f| (f.clone(), tree.blob(f).unwrap_or("").to_string())).collect();
    for f in deleted.iter().filter(|f| under(f)) { files.remove(f); }
    for f in edited.into_iter().filter(|f| under(f)) {
        let path = repo.join(&f);
        let bytes = match std::fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => std::fs::read_link(&path).map(|t| t.to_string_lossy().into_owned().into_bytes()),
            _ => std::fs::read(&path),
        }.map_err(|e| format!("{}: {e}", path.display()))?;
        files.insert(f, makepad_git::oid::hash_object("blob", &bytes).to_hex());
    }
    let text: String = files.iter().map(|(f, blob)| format!("{f}\t{blob}\n")).collect();
    Ok(makepad_network::digest::sha256_hash(text.as_bytes())[..8].iter().map(|b| format!("{b:02x}")).collect())
}
#[cfg(not(feature = "checkout"))]
pub(crate) fn crate_inputs(_: &Path, _: &crate::lease::Pin, _: &str) -> Result<String, String> { Ok(String::new()) }

/// [`crate_inputs`] of `pkg` in the lane's worktree `repo` at its pin `rev`; None: no worktree or
/// pin, or they cannot be read.
pub(crate) fn inputs_at(repo: &str, rev: &str, pkg: &str) -> Option<String> {
    let pin = crate::lease::Pin::of_rev(rev).filter(|_| !repo.is_empty())?;
    crate_inputs(Path::new(repo), &pin, pkg).ok()
}

/// Whether the JIT's inputs differ between the pair of a miss's last JIT failure and the
/// requester's (the JIT id changed): CI's own JIT-input rule. Commits git cannot compare
/// count as changed (one JIT attempt, and a real miss reopens its row); no repo: unchanged.
#[cfg(feature = "checkout")]
fn jit_changed(repo: &str, from: &crate::state::Pair, to: &crate::state::Pair) -> bool {
    if repo.is_empty() || from == to || from.work.is_empty() { return false; }
    crate::orch::ci::Inputs::between(std::path::Path::new(repo), from, to).map_or(true, |i| i.jit_changed)
}
#[cfg(not(feature = "checkout"))]
fn jit_changed(_: &str, _: &crate::state::Pair, _: &crate::state::Pair) -> bool { false }

fn write_ledger(home: &Path, path: &Path, entries: &[Miss]) -> Result<(), String> {
    let tmp = path.with_extension("jsonl.new");
    let contents: String = entries.iter().map(|m| format!("{}\n", m.value().to_json())).collect();
    std::fs::create_dir_all(home).and_then(|_| std::fs::write(&tmp, contents)).and_then(|_| std::fs::rename(&tmp, path)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Called only on the master. Serialize read/modify/rename, including across simultaneous
/// clients; a failed write leaves the old ledger intact and is reported to the caller.
pub(crate) fn serve(home: &Path, line: &str, now: u64, event: &dyn Fn(&str, &Value)) -> Result<String, String> {
    static LEDGER: Mutex<()> = Mutex::new(());
    let _guard = LEDGER.lock().unwrap();
    let path = home.join("jit_misses.jsonl");
    let contents = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let mut entries: Vec<Miss> = contents.lines().filter(|l| !l.trim().is_empty()).map(|l| {
        let v = makepad_strict_json::parse(l.as_bytes()).map_err(|e| format!("{}: {e:?}", path.display()))?;
        Miss::parse(&v)
    }).collect::<Result<_, String>>()?;
    let rest = line.strip_prefix("misses").unwrap_or(line).trim();
    if rest.is_empty() || rest == "--all" {
        let list: Vec<_> = entries.iter().filter(|m| rest == "--all" || m.closed_ms == 0).collect();
        let mut out = String::new();
        for m in &list {
            out.push_str(&format!("{} {} {} {} ({} times, lane {}, pair {}, first {}, last {}): {}{}\n", m.id, if m.closed_ms == 0 { "open" } else { "closed" }, m.pkg, m.target, m.count, m.lane, m.pair.short(), m.first_ms, m.last_ms, m.error[0], if m.repro.is_empty() { String::new() } else { format!(" (repro: {})", m.repro) }));
        }
        if list.is_empty() { out.push_str("no JIT misses\n"); }
        return Ok(obj(vec![("text", s(&out)), ("misses", Value::Arr(list.iter().map(|m| m.value()).collect()))]).to_json());
    }
    let (verb, arg) = rest.split_once(' ').ok_or("jit misses [--all] | close <id>")?;
    let (index, action) = match verb {
        // filed by hand (`jit miss add`): target `manual`, so it never routes a test to cargo
        "add" => {
            let req = makepad_strict_json::parse(arg.as_bytes()).map_err(|_| "jit misses: bad request")?;
            let (pkg, error) = (text(&req, "pkg"), text(&req, "error").lines().next().unwrap_or("").trim());
            if pkg.is_empty() || error.is_empty() { return Err("jit miss add <pkg> \"<error line>\" [--repro \"<cmd>\"]".into()); }
            let i = entries.iter().position(|m| m.pkg == pkg && m.target == "manual" && m.closed_ms == 0 && m.error.first().map(String::as_str) == Some(error)).unwrap_or_else(|| {
                let id = entries.iter().filter_map(|m| m.id[1..].parse::<u64>().ok()).max().unwrap_or(0) + 1;
                entries.push(Miss { id: format!("M{id}"), pkg: pkg.into(), target: "manual".into(), error: vec![error.into()], lane: String::new(), pair: Default::default(), first_ms: now, last_ms: now, count: 0, closed_ms: 0, repro: String::new(), inputs: String::new() });
                entries.len() - 1
            });
            let m = &mut entries[i];
            (m.lane, m.last_ms, m.count) = (text(&req, "lane").into(), now, m.count + 1);
            if !text(&req, "repro").is_empty() { m.repro = text(&req, "repro").into(); }
            (i, "recorded")
        }
        "close" => {
            let i = entries.iter().position(|m| m.id == arg.trim()).ok_or_else(|| format!("no JIT miss {}", arg.trim()))?;
            entries[i].closed_ms = now;
            (i, "closed")
        }
        "lookup" | "record" => {
            let req = makepad_strict_json::parse(arg.as_bytes()).map_err(|_| "jit misses: bad request")?;
            let (pkg, target) = (text(&req, "pkg"), text(&req, "target"));
            if pkg.is_empty() || target.is_empty() { return Err("jit misses: pkg and target required".into()); }
            let existing = entries.iter().position(|m| m.pkg == pkg && m.target == target && m.closed_ms == 0);
            let errors = if verb == "lookup" {
                if let Some(i) = existing {
                    if let Some(pin) = crate::lease::Pin::of_rev(text(&req, "rev")) {
                        let to = pin.pair();
                        // the JIT's inputs changed, or the package's: the miss was the JIT's
                        // failure on others (M64 kept a crate's E0700 on cargo after its line
                        // changed, 11-06)
                        let repo = text(&req, "repo");
                        if jit_changed(repo, &entries[i].pair, &to) || (!repo.is_empty() && crate_inputs(Path::new(repo), &pin, pkg).ok().as_deref() != Some(entries[i].inputs.as_str())) {
                            entries[i].closed_ms = now;
                            write_ledger(home, &path, &entries)?;
                            let value = entries[i].value();
                            drop(_guard);
                            event("retried", &value);
                            return Ok("null".into());
                        }
                    }
                    entries[i].error.clone()
                }
                else if entries.iter().any(|m| m.pkg == pkg && m.target == target) { return Ok("null".into()); }
                else if let Some(errors) = ci_error(home, pkg) { errors }
                else { return Ok("null".into()); }
            } else {
                req.get("error").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).flat_map(str::lines).take(5).map(str::to_string).collect()
            };
            if errors.is_empty() { return Err("jit misses: error lines required".into()); }
            let i = entries.iter().position(|m| m.pkg == pkg && m.target == target && m.error.first() == errors.first()).unwrap_or_else(|| {
                let id = entries.iter().filter_map(|m| m.id[1..].parse::<u64>().ok()).max().unwrap_or(0) + 1;
                entries.push(Miss { id: format!("M{id}"), pkg: pkg.into(), target: target.into(), error: errors.clone(), lane: String::new(), pair: Default::default(), first_ms: now, last_ms: now, count: 0, closed_ms: 0, repro: String::new(), inputs: String::new() });
                entries.len() - 1
            });
            let action = if verb == "lookup" && existing.is_some() { "used" } else { "recorded" };
            entries[i].touch(&req, now, action == "recorded")?;
            entries[i].error = errors;
            if action == "recorded" {
                // a record's: those its failing JIT run read, as its client read them when the run
                // shipped, never the lane's once cargo has answered (M64's lane edited its crate
                // in between); a miss a lookup seeds from CI's error, the lane's now. None
                // (unreadable) equals no lookup's: the next one tries the JIT.
                entries[i].inputs = if verb == "record" { text(&req, "inputs").to_string() } else { inputs_at(text(&req, "repo"), text(&req, "rev"), pkg).unwrap_or_default() };
            }
            (i, action)
        }
        _ => return Err("jit misses [--all] | close <id>".into()),
    };
    write_ledger(home, &path, &entries)?;
    let value = entries[index].value();
    drop(_guard);
    event(action, &value);
    Ok(value.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "checkout")]
    #[test]
    fn a_lane_whose_jit_is_older_than_works_has_no_jit_miss() {
        let repo = std::env::temp_dir().join(format!("devhub-miss-behind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        let git = |dir: &Path, args: &[&str]| crate::orch::git(dir, &[&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"][..], args].concat()).unwrap().trim().to_string();
        let write = |p: &str, text: &str| { let f = repo.join(p); std::fs::create_dir_all(f.parent().unwrap()).unwrap(); std::fs::write(f, text).unwrap(); };
        write("Cargo.toml", "[workspace]\nmembers = [\"tools/run\", \"libs/relax\", \"apps/x\"]\n");
        write("tools/run/Cargo.toml", "[package]\nname = \"makepad-run\"\n[dependencies]\nrelax = { path = \"../../libs/relax\" }\n");
        write("libs/relax/Cargo.toml", "[package]\nname = \"relax\"\n");
        write("libs/relax/std.rs", "pub struct Realtime;\n");
        write("apps/x/x.rs", "fn main() {}\n");
        git(&repo, &["init", "-q", "-b", "work"]);
        git(&repo, &["add", "-A"]);
        git(&repo, &["commit", "-qm", "base"]);
        let com = repo.join("apps/commercial");
        std::fs::create_dir_all(&com).unwrap();
        git(&com, &["init", "-q", "-b", "main"]);
        git(&com, &["commit", "-q", "--allow-empty", "-m", "c"]);
        let pin = crate::state::Pair::new(&git(&repo, &["rev-parse", "HEAD"]), &git(&com, &["rev-parse", "HEAD"]));
        assert_eq!(behind_work_jit(&repo, &pin), None, "the lane is at work");
        // work moved, but not the JIT: the lane's JIT is work's
        write("apps/x/x.rs", "fn main() { let _ = 1; }\n");
        git(&repo, &["commit", "-qam", "app"]);
        assert_eq!(behind_work_jit(&repo, &pin), None);
        // work changed the JIT's std (#806): the lane's JIT is older, its errors no miss
        write("libs/relax/std.rs", "pub struct Realtime;\npub struct RealtimeArmed;\n");
        git(&repo, &["commit", "-qam", "std"]);
        assert_eq!(behind_work_jit(&repo, &pin).as_deref(), Some(pin.work.as_str()));
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// A miss is the JIT's failure on its package's inputs: an edit to the package or a crate it
    /// depends on by path (committed or not) tries the JIT again; an edit elsewhere does not, and
    /// a commit of the same edit is the same inputs.
    #[cfg(feature = "checkout")]
    #[test]
    fn a_miss_is_retried_once_its_crates_inputs_change() {
        let repo = std::env::temp_dir().join(format!("devhub-miss-inputs-{}", std::process::id()));
        let home = repo.with_extension("home");
        let _ = (std::fs::remove_dir_all(&repo), std::fs::remove_dir_all(&home));
        let git = |args: &[&str]| crate::orch::git(&repo, &[&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"][..], args].concat()).unwrap().trim().to_string();
        let write = |p: &str, text: &str| { let f = repo.join(p); std::fs::create_dir_all(f.parent().unwrap()).unwrap(); std::fs::write(f, text).unwrap(); };
        write("Cargo.toml", "[workspace]\nmembers = [\"tools/run\", \"libs/relax\", \"apps/a\", \"libs/b\", \"apps/c\"]\n");
        write("tools/run/Cargo.toml", "[package]\nname = \"makepad-run\"\n[dependencies]\nrelax = { path = \"../../libs/relax\" }\n");
        write("libs/relax/Cargo.toml", "[package]\nname = \"relax\"\n");
        write("apps/a/Cargo.toml", "[package]\nname = \"a\"\n[dependencies]\nb = { path = \"../../libs/b\" }\n");
        write("apps/a/src/lib.rs", "pub fn f() -> impl Sized { b::g() }\n");
        write("libs/b/Cargo.toml", "[package]\nname = \"b\"\n");
        write("libs/b/src/lib.rs", "pub fn g() {}\n");
        write("apps/c/Cargo.toml", "[package]\nname = \"c\"\n");
        write("apps/c/src/lib.rs", "\n");
        git(&["init", "-q", "-b", "work"]);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "base"]);
        let events = Mutex::new(Vec::new());
        let dir = repo.to_string_lossy().into_owned();
        // a record carries the inputs its JIT run shipped, as the client reads them then
        let ask = |verb: &str, sha: &str, now: u64, inputs: Option<&str>| {
            let req = format!(r#"{{"pkg":"a","target":"--lib","lane":"l","rev":"work@{sha}","repo":{},"error":["apps/a/src/lib.rs:1:15: error[E0700]: hidden type captures lifetime"],"inputs":{}}}"#, makepad_remote::json_str(&dir), inputs.map_or("null".to_string(), makepad_remote::json_str));
            serve(&home, &format!("misses {verb} {req}"), now, &|action, _| events.lock().unwrap().push(action.to_string())).unwrap()
        };
        let shipped = |sha: &str| inputs_at(&dir, &format!("work@{sha}"), "a");
        let base = git(&["rev-parse", "HEAD"]);
        assert!(ask("record", &base, 1, shipped(&base).as_deref()).contains("\"id\":\"M1\""));
        assert!(ask("lookup", &base, 2, None).contains("\"id\":\"M1\""), "the same inputs: cargo's verdict stands");
        write("apps/c/src/lib.rs", "pub fn h() {}\n");
        assert!(ask("lookup", &base, 3, None).contains("\"id\":\"M1\""), "a crate it does not build");
        write("libs/b/src/lib.rs", "pub fn g() -> u8 { 1 }\n");
        assert_eq!(ask("lookup", &base, 4, None), "null", "a crate it builds changed: the JIT tries again");
        assert_eq!(events.lock().unwrap().last().map(String::as_str), Some("retried"));
        // the JIT failed again on these: the miss reopens, at them
        assert!(ask("record", &base, 5, shipped(&base).as_deref()).contains("\"closed_ms\":0"));
        assert!(ask("lookup", &base, 6, None).contains("\"id\":\"M1\""));
        git(&["commit", "-qam", "b"]);
        let head = git(&["rev-parse", "HEAD"]);
        assert!(ask("lookup", &head, 7, None).contains("\"id\":\"M1\""), "its edit committed: the same inputs");
        // M64: the lane edits its crate between the JIT's failure and cargo's answer; the miss is
        // keyed by what the JIT read, so the next lookup tries the edit on it
        let before_edit = shipped(&head);
        write("libs/b/src/lib.rs", "pub fn g() -> u16 { 2 }\n");
        assert!(ask("record", &head, 8, before_edit.as_deref()).contains("\"closed_ms\":0"));
        assert_eq!(ask("lookup", &head, 9, None), "null", "the edit the JIT never read is tried");
        // a client that could not read them keys it by none, which no lookup's are
        assert!(ask("record", &head, 10, None).contains("\"closed_ms\":0"));
        assert_eq!(ask("lookup", &head, 11, None), "null");
        let _ = (std::fs::remove_dir_all(&repo), std::fs::remove_dir_all(&home));
    }

    #[test]
    fn a_miss_filed_by_hand_is_listed_and_counted() {
        let home = std::env::temp_dir().join(format!("devhub-miss-add-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let add = r#"misses add {"pkg":"makepad-foo","error":"src/a.rs:3:1: unsupported intrinsic\nmore","repro":"devhub test x makepad-foo --lib","lane":"wp"}"#;
        let first = serve(&home, add, 1, &|_, _| {}).unwrap();
        assert!(first.contains("\"id\":\"M1\"") && first.contains("\"target\":\"manual\""), "{first}");
        assert!(serve(&home, add, 2, &|_, _| {}).unwrap().contains("\"count\":2"));
        let list = serve(&home, "misses", 3, &|_, _| {}).unwrap();
        assert!(list.contains("M1 open makepad-foo manual (2 times, lane wp") && list.contains("unsupported intrinsic (repro: devhub test x makepad-foo --lib)"), "{list}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_tests_own_result_line_does_not_end_its_window() {
        let mut log = String::from(r#"running 3 tests
test a ... ok
test result: 10 passed; 0 failed. compiled 185 fns (35708 bytes, 6827 RIR insts): typeck 139.68 ms
  report: {"kind":"panic","message":"Box<dyn Any>","location":"/tree/std/std/panic.rs:248:5"}
src/a.rs:12:5: some note
test b ... ok
test c ... ok
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.10s
"#);
        assert!(compile_errors(&log).is_empty());
        let error = "/tree/src/b.rs:1:2: E0308: mismatched types";
        log.push_str(error);
        assert_eq!(compile_errors(&log), vec![error.to_string()]);
    }

    /// A line of the app's own log at a source line is no compile error: an `[I]` line of a guard
    /// run was its JIT miss (M75-M77, 11-07).
    #[test]
    fn an_apps_log_line_is_no_compile_error() {
        let log = "[I] apps/x/src/fx.rs:628:17 - effects: bloom read (144.5 ms)\n[E] apps/x/src/fx.rs:9:1 - no such picture\n[W] src/a.rs:3:2 - slow\n";
        assert!(compile_errors(log).is_empty(), "{:?}", compile_errors(log));
        let error = "/tree/src/b.rs:1:2: E0308: mismatched types";
        assert_eq!(compile_errors(&format!("{log}{error}\n")), vec![error.to_string()]);
    }

    /// A package says why its tests need the lane's tree; one that says nothing runs in an
    /// overlay. devhub's own tests walk this checkout's history and run cargo tree on it, which
    /// an overlay lacks (11-07).
    #[cfg(feature = "checkout")]
    #[test]
    fn a_package_whose_tests_read_the_checkout_runs_in_the_tree() {
        let t = crate::orch::ci::Tree::mem(vec![
            ("libs/a/Cargo.toml", "[package]\nname = \"a\"\n[package.metadata.makepad]\ntest-tree = \"its tests read the history\"\n"),
            ("libs/b/Cargo.toml", "[package]\nname = \"b\"\n[package.metadata.makepad]\nplatforms = [\"desktop\"]\n"),
        ]);
        assert_eq!(super::package_of(&t, "a"), (false, Some("its tests read the history".to_string())));
        assert_eq!(super::package_of(&t, "b"), (false, None));
        assert_eq!(super::package_of(&t, "c"), (false, None));
        let own = makepad_toml_parser::parse_toml(&std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml")).unwrap()).unwrap();
        assert!(own.get_path(&["package", "metadata", "makepad", "test-tree"]).and_then(makepad_toml_parser::Toml::as_str).is_some(), "devhub's tests read the checkout: its Cargo.toml says so");
    }

    /// The package a JIT `makepad test` command tests, as makepad test reads it: past its
    /// options (with their values) and the `-p` marker; nothing for other commands.
    #[test]
    fn a_jit_test_command_names_its_package() {
        let w = |c: &str| c.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        for (cmd, pkg) in [
            ("makepad test -p pkg-a --release -- one", Some("pkg-a")),
            ("env CARGO_INCREMENTAL=0 makepad test --package pkg-a", Some("pkg-a")),
            ("makepad test --features x,y --live pkg-b --lib", Some("pkg-b")),
            ("cargo run --profile host -q -p makepad-run --bin makepad -- test pkg-c", Some("pkg-c")),
            ("makepad test -- pkg-d", None),
            ("makepad run pkg-e", None),
            ("cargo test -p pkg-f", None),
        ] {
            assert_eq!(super::tested_package(&w(cmd)), pkg, "{cmd}");
        }
    }

    /// A JIT warning or note at a source line is no compile error: M71 recorded a test run's
    /// `unused imports` warning as its JIT miss (11-07).
    #[test]
    fn a_located_warning_or_note_is_no_compile_error() {
        let warn = "/tree/libs/video/tests/color.rs:20:94: warning: unused imports: `VideoMatrix` and `VideoRange` [unused_imports]";
        let log = format!("{warn}\n/tree/src/a.rs:3:1: warning[W1]: x\n/tree/src/a.rs:4:1: note: y\n/tree/src/a.rs:5: help: z\nmakepad test: 1 warning\n");
        assert!(compile_errors(&log).is_empty(), "{:?}", compile_errors(&log));
        let error = "/tree/src/b.rs:1:2: E0308: mismatched types";
        assert_eq!(compile_errors(&format!("{log}{error}\n")), vec![error.to_string()]);
        assert_eq!(compile_errors("/tree/src/b.rs:9:3: error: expected `;`\n"), vec!["/tree/src/b.rs:9:3: error: expected `;`".to_string()]);
    }

    #[test]
    fn a_tests_crash_report_is_not_a_miss() {
        let log = r#"running 1 test
test an_unbaked_protected_request_is_traced ... FAILED
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.10s
  report: {"kind":"panic","message":"at rest the tracer still holds 507392 bytes of GPU buffers","location":"/tree/apps/x/tests/trace_unbaked.rs:112:13"}
makepad-run: main panic report: {"kind":"panic","location":"/tree/src/main.rs:3:5"}
"#;
        assert!(compile_errors(log).is_empty(), "{:?}", compile_errors(log));
        let none = "running 0 tests\n\ntest result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 9 filtered out; finished in 0.01s\n\nerror: no test matched no_such_test\n";
        assert!(compile_errors(none).is_empty(), "{:?}", compile_errors(none));
    }

    #[test]
    fn a_warm_restarts_reasons_are_not_misses() {
        let log = "testverdict group=1 ran=101 passed=101 failed=0
makepad live:   /tree/src/lease/exec.rs:2435:1: mod `tests` changed: a module's item list changed -- restart the app
makepad live:   /tree/src/proto.rs:1:1: const `BUFFER_SIZE` is used in a type: its users' types change -- restart the app
makepad live:   /tree/src/jit_misses.rs:147:9: use use crate::x; added here (inside a fn body) -- adding this kind of item is not applied live yet: restart the app
makepad live: restarting (/tree/src/a.rs, /tree/src/b.rs)
running 103 tests
test a ... ok
test b ... ok
";
        assert!(compile_errors(log).is_empty(), "warm restart reasons are not compile errors");
        let cut = "rch/src/jit_misses.rs:147:9: use use crate::x; added here (inside a fn body) -- adding this kind of item is not applied live yet: restart the app";
        assert!(compile_errors(cut).is_empty(), "a cut-off restart reason is not a compile error");
        let error = "/tree/src/b.rs:1:2: E0308: mismatched types";
        let log = log.replace("running 103 tests", &format!("{error}\nrunning 103 tests"));
        assert_eq!(compile_errors(&log), vec![error.to_string()], "a real compile error before the restarted harness still counts");
    }

    #[test]
    fn a_target_that_fails_to_compile_between_passing_targets_is_a_miss() {
        let error = "/tree/apps/x/deps/atlas/tests/relief.rs:160:62: E0277: the trait bound `HashMap<Arc<str>, Arc<str>, RandomState>: FromIterator<<Map<Iter<String, FrozenFile>, closure#825> as Iterator>::Item>` is not satisfied";
        let log = format!("running 2 tests
test a ... ok
test b ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.68s

{error}
error: could not compile `relief` (test) due to 1 previous error: no test ran (`makepad test` fails as `cargo test` does on code rustc rejects; if `cargo test` builds it, this is a makepad run bug)

running 10 tests
test result: ok. 10 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.83s
");
        assert_eq!(compile_errors(&log), vec![error.to_string()]);
        let log = "running 1 test
test t ... FAILED

failures:

---- t stdout ----
src/a.rs:1:2: E0308: mismatched types
error: could not compile `a` (lib) due to 1 previous error

failures:
    t

test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
";
        assert!(compile_errors(log).is_empty());
    }
}
