//! JIT first: lanes iterate and check under the JIT (`devhub live start <app> --visible` and
//! `live look`, `devhub test <lane> <pkg> [--live]`). A lane's lease command that builds a package
//! with cargo (build, run, test, bench) is refused, except:
//! - the JIT's own crates ([`crate::jit_misses::jit_own`]);
//! - a package with an open JIT miss (the run names it);
//! - a platform without a JIT: no JIT leg ([`crate::orch::verify::legs`]) runs on its os and arch;
//! - a test devhub's measured routing sends to cargo (`native:<class>`, the JIT measured slower end
//!   to end: [`super::master::LeaseMaster::test_route`]);
//! - a test run that selects a target its package's Cargo.toml runs under cargo
//!   ([`crate::jit_misses::native_selected`]: `[package.metadata.makepad.native-tests]`);
//! - `--native "<reason>"`: native-vs-JIT baselines and timed perf series. Its reason's words are
//!   not judged; what devhub knows is ([`review`]): a reason passes for a timed or bench series, a
//!   baseline (the same command runs the JIT), a crate of the JIT binary, or with an open JIT miss
//!   it names. Any other is refused once [`REVIEW_REFUSES`] is on; until then it runs, its lane
//!   hears the review at once and main a count a day (main, 11-23).
//!
//! Every verdict but the JIT's own crates is a line of `native.jsonl` (what `devhub prof` counts
//! per lane; `not_over` says why a lane's choice is no sign it passes the JIT by: it builds crates
//! of the JIT binary, which only a rebuild of that binary tries, or it is a baseline, a timed
//! series or a command that runs the JIT beside its native build). CI, gates, inspect leases and devhub's own lanes (landings, buckets, prebuilds, legs,
//! shows, live sessions) keep their own rules.

use super::sched::NodeView;
use super::Kind;
use makepad_remote::json_str;
use makepad_strict_json::Value;
use std::collections::BTreeSet;
use std::path::Path;

pub const LEDGER: &str = "native.jsonl";

/// A command's words, with `;` for each command separator (`;`, `|`, `&`, a newline, a paren).
/// Quotes, brackets and commas only separate words: `bash -c '…'` texts and Python argv lists
/// read like shell lines.
fn words(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut word = String::new();
    let mut chars = text.chars().peekable();
    let flush = |word: &mut String, out: &mut Vec<String>| if !word.is_empty() { out.push(std::mem::take(word)); };
    while let Some(c) = chars.next() {
        match c {
            '#' if word.is_empty() => {
                while chars.next_if(|c| *c != '\n').is_some() {}
            }
            '\\' if chars.peek() == Some(&'\n') => { chars.next(); flush(&mut word, &mut out); }
            ';' | '|' | '&' | '\n' | '(' | ')' => {
                flush(&mut word, &mut out);
                if out.last().is_some_and(|w| w != ";") { out.push(";".into()); }
            }
            c if c.is_whitespace() || "'\"`[]{},\\".contains(c) => flush(&mut word, &mut out),
            c => word.push(c),
        }
    }
    flush(&mut word, &mut out);
    out
}

/// One cargo build in a command.
#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    /// `-p`'s package; `*`: none named (the package of the command's dir, else the workspace's)
    pub pkg: String,
    /// build, run, test or bench
    pub verb: &'static str,
    /// `--target`'s triple; empty: the host's
    pub target: String,
    /// the targets a test selects, as a Cargo.toml's native-tests names them (`lib`,
    /// `bin:<name>`, `test:<name>`), or `bins`, `tests`, `all-targets`; empty: cargo's default (or
    /// no test)
    pub select: Vec<String>,
}

/// What `cmd` builds with cargo: each `cargo build|run|test|bench` (never check, clippy or doc)
/// in its words, a `-c` text, or a `.sh`/`.py` script it names, read with `script` (one level).
pub fn builds(cmd: &[String], script: &dyn Fn(&str) -> Option<String>) -> Vec<Built> {
    built_in(&words(&command_text(cmd, script)))
}

/// A command's text with the `.sh`/`.py` scripts it names (one level).
fn command_text(cmd: &[String], script: &dyn Fn(&str) -> Option<String>) -> String {
    let mut text = cmd.join(" ");
    for w in cmd.iter().filter(|w| w.ends_with(".sh") || w.ends_with(".py")) {
        if let Some(t) = script(w) { text.push('\n'); text.push_str(&t); }
    }
    text
}

/// The command also runs something under the JIT: `makepad test|run`, by any path, or its cargo
/// form.
fn runs_jit(w: &[String]) -> bool {
    w.windows(2).any(|p| {
        let binary = p[0].rsplit(['/', '\\']).next().is_some_and(|b| b == "makepad" || b == "makepad.exe");
        (binary && matches!(p[1].as_str(), "test" | "run")) || (p[0] == "-p" && p[1] == "makepad-run")
    })
}

/// The command calls the bare `makepad` a run gets on its PATH (`exec.rs` job_makepad) with a word
/// after it: in its words, or a shell's or Python's `-c` text (a script file it names is not
/// read). A path to a binary of its own (`target/host/makepad`), or the word alone (`command -v
/// makepad`), is no such call. The master reads a run's class the same way
/// ([`super::sched`]'s `jit_class`): such a run builds its tree's JIT where its box has none.
#[cfg(feature = "checkout")]
pub(super) fn calls_makepad(cmd: &[String]) -> bool {
    words(&cmd.join(" ")).windows(2).any(|p| p[0] == "makepad" && p[1] != ";")
}

/// The command builds a tree it made: cargo after a `cd` into a `mktemp` dir of its own, an export
/// under its own workspace manifest (another repository's tree, built as a fresh clone of it
/// builds), which no JIT run of the lane's workspace stands in for. Such builds before each push
/// of the relax repository marked its lane OVER (11-13).
fn builds_made_tree(w: &[String]) -> bool {
    let mut made: Vec<&str> = Vec::new();
    let mut in_made = false;
    for (i, x) in w.iter().enumerate() {
        if let Some(var) = x.strip_suffix("=$").filter(|_| w.get(i + 1).is_some_and(|s| s == ";") && w.get(i + 2).is_some_and(|m| m == "mktemp")) { made.push(var); }
        if x == "cd" { in_made = w.get(i + 1).and_then(|d| d.strip_prefix('$')).is_some_and(|d| made.contains(&d.split('/').next().unwrap_or(d))); }
        if in_made && (x == "cargo" || x.ends_with("/cargo")) { return true; }
    }
    false
}

fn built_in(w: &[String]) -> Vec<Built> {
    let mut out: Vec<Built> = Vec::new();
    let mut i = 0;
    while i < w.len() {
        if w[i] != "cargo" && !w[i].ends_with("/cargo") { i += 1; continue; }
        let mut j = i + 1;
        while w.get(j).is_some_and(|x| x.starts_with('+') || matches!(x.as_str(), "-q" | "--quiet" | "-v" | "-vv" | "--locked" | "--frozen" | "--offline")) { j += 1; }
        let verb = match w.get(j).map(String::as_str) {
            Some("build" | "b") => "build",
            Some("run" | "r") => "run",
            Some("test" | "t") => "test",
            Some("bench") => "bench",
            _ => { i = j; continue; }
        };
        let (mut pkgs, mut target, mut select) = (Vec::new(), String::new(), Vec::new());
        let mut k = j + 1;
        while k < w.len() && w[k] != ";" && w[k] != "--" {
            let x = w[k].as_str();
            if (x == "-p" || x == "--package" || x == "--target") && k + 1 < w.len() {
                if x == "--target" { target = w[k + 1].clone(); } else { pkgs.push(w[k + 1].clone()); }
                k += 1;
            } else if matches!(x, "--lib" | "--bins" | "--tests" | "--all-targets") {
                select.push(x[2..].to_string());
            } else if (x == "--bin" || x == "--test") && k + 1 < w.len() {
                select.push(format!("{}:{}", &x[2..], w[k + 1]));
                k += 1;
            } else if let Some((kind, name)) = x.strip_prefix("--bin=").map(|n| ("bin", n)).or_else(|| x.strip_prefix("--test=").map(|n| ("test", n))) {
                select.push(format!("{kind}:{name}"));
            } else if let Some(t) = x.strip_prefix("--target=") {
                target = t.to_string();
            } else if let Some(p) = x.strip_prefix("--package=").or_else(|| x.strip_prefix("-p").filter(|p| !p.is_empty() && !p.starts_with('-'))) {
                pkgs.push(p.to_string());
            }
            k += 1;
        }
        if pkgs.is_empty() { pkgs.push("*".into()); }
        for pkg in pkgs {
            let b = Built { pkg, verb, target: target.clone(), select: if verb == "test" { select.clone() } else { Vec::new() } };
            if !out.contains(&b) { out.push(b); }
        }
        i = k;
    }
    out
}

/// The package whose manifest is in `dir` (`[package] name`).
fn package_at(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join("Cargo.toml")).ok()?;
    let mut in_package = false;
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') { in_package = line == "[package]"; continue; }
        let Some(v) = line.strip_prefix("name").map(str::trim_start).and_then(|v| v.strip_prefix('=')).filter(|_| in_package) else { continue };
        return Some(v.trim().trim_matches('"').to_string());
    }
    None
}

/// devhub's own lanes and agents run what they must (CI, landing, buckets, prebuilds, legs,
/// shows, live tests).
fn exempt_lane(lane: &str, agent: &str) -> bool {
    matches!(lane, super::sched::CI_LANE | super::sched::LANDING_LANE)
        || ["landq", "jitlegs", super::WARM_LANE_PREFIX, "show-", "live-"].iter().any(|p| lane.starts_with(p))
        || agent.starts_with("devhub-")
}

/// `x86_64` and `x64` alike: the names `--needs arch=` takes.
fn arch_word(arch: &str) -> String {
    match arch.to_ascii_lowercase().as_str() {
        "x86_64" | "amd64" => "x64".into(),
        "aarch64" => "arm64".into(),
        a => a.into(),
    }
}

/// The platform a lease runs on, (os, arch): its named node's, else its needs' (the pool's
/// default: x64 Linux; an arm64 Mac for `os=macos`).
pub fn platform(needs: &[(String, String)], node: &str, nodes: &[NodeView]) -> (String, String) {
    if let Some(c) = nodes.iter().find(|n| n.name == node).and_then(|n| n.caps.as_ref()) {
        return (c.os.to_ascii_lowercase(), arch_word(&c.arch));
    }
    let need = |k: &str| needs.iter().find(|(n, _)| n == k).map(|(_, v)| v.to_ascii_lowercase());
    let os = need("os").unwrap_or_else(|| "linux".into());
    let arch = need("arch").map(|a| arch_word(&a)).unwrap_or_else(|| if os == "macos" { "arm64" } else { "x64" }.into());
    (os, arch)
}

/// A cargo `--target`'s platform: wasm, iOS and the like name their own os, which no JIT runs on.
fn target_platform(triple: &str) -> (String, String) {
    let os = ["android", "windows", "darwin", "linux"].into_iter().find(|o| triple.contains(o)).map_or(triple, |o| if o == "darwin" { "macos" } else { o });
    (os.to_string(), arch_word(triple.split('-').next().unwrap_or("")))
}

/// Whether the JIT runs on this platform: a JIT leg verifies it.
fn jit_runs(os: &str, arch: &str) -> bool {
    // (a gating leg: a report-only one is on its way to verifying)
    crate::orch::verify::legs(true).iter().filter(|leg| !leg.report_only).any(|leg| {
        let needs = super::sched::parse_needs(&leg.needs);
        needs.iter().any(|(k, v)| k == "os" && v.eq_ignore_ascii_case(os)) && needs.iter().any(|(k, v)| k == "arch" && arch_word(v) == arch)
    })
}

/// The JIT command for this package instead of its native build.
fn hint(lane: &str, rev: &str, b: &Built) -> String {
    let pkg = if b.pkg == "*" { "<pkg>" } else { &b.pkg };
    if matches!(b.verb, "test" | "bench") && !rev.is_empty() {
        format!("`devhub test {lane} {pkg} --rev {rev}` runs it under the JIT at those commits alone")
    } else if matches!(b.verb, "test" | "bench") {
        format!("`devhub test {lane} {pkg} --live` while iterating, `devhub test {lane} {pkg}` for the final check")
    } else {
        format!("`devhub live start {pkg} --visible`, then `devhub live look <id>` after each save")
    }
}

/// One gate question: a lane's command and where it runs.
pub struct Ask<'a> {
    pub lane: &'a str,
    pub agent: &'a str,
    pub kind: Kind,
    pub cmd: &'a [String],
    /// the lane's worktree and the command's dir in it
    pub repo_dir: &'a Path,
    pub cwd: &'a str,
    /// `<branch>@<sha>[,…]`: the commits it runs (empty: the worktree's HEAD)
    pub rev: &'a str,
    /// `--native "<reason>"`
    pub reason: &'a str,
    /// the lease's class (`native:test:<pkg>[:<target>]` for a native test)
    pub class: &'a str,
    /// devhub's measured routing for a JIT test class: Some(why, both walls) when it picks cargo
    pub route: &'a dyn Fn(&str) -> Option<String>,
    /// the cargo route `devhub test` was granted at its run's start (`stats route`'s why); empty:
    /// none. Kept when the measure has moved since its ask: a run that ended between, or a master
    /// restart (11-25). Its CLI sends it only as the master answered it, as it runs this check.
    pub granted: &'a str,
    /// [`platform`]
    pub os: &'a str,
    pub arch: &'a str,
}

/// The commits a lane's command runs: its pin, else its worktree's HEAD.
fn pair_of(repo_dir: &Path, rev: &str) -> Result<crate::state::Pair, String> {
    if let Some(pin) = super::Pin::of_rev(rev) {
        return Ok(pin.pair());
    }
    let o = crate::git_exec::command().args(["rev-parse", "HEAD"]).current_dir(repo_dir).output().map_err(|e| format!("git: {e}"))?;
    Ok(crate::state::Pair::new(String::from_utf8_lossy(&o.stdout).trim(), ""))
}

/// Whether a Windows box takes a lease's command ([`super::sched::Req::windows`]).
#[derive(Clone, Debug, PartialEq)]
pub enum OnWindows {
    /// a plain cargo build, check, clippy or test of packages checked there, none of their gaps
    Runs,
    /// not such a command, or a package not checked there: no Windows box takes it unless named
    Not,
    /// a package's gap there, with why ([`crate::orch::ci::Krate::platform_gaps`])
    Gap(String),
}

/// Whether a Windows box runs `cmd`: plain argv (a Windows run is never a shell's), `cargo build`,
/// `check`, `clippy` or `test` of packages it names with `-p`, each checked on Windows (`gaps`:
/// its platform-gaps' `windows` table, None when it has none) and selecting none of its gaps.
/// A build or check selects its lib and bins unless it names targets; a test, every target. A
/// `lib` gap is its tests' alone (a lib that does not build is its package's `build` gap), so it
/// keeps only a test off. The workspace, another manifest or a `--target` is not checked anywhere:
/// it stays off Windows.
pub fn on_windows(cmd: &[String], gaps: &dyn Fn(&str) -> Option<std::collections::BTreeMap<String, String>>) -> OnWindows {
    let Some(first) = cmd.first() else { return OnWindows::Not };
    let program = first.rsplit(['/', '\\']).next().unwrap_or(first);
    if program != "cargo" && program != "cargo.exe" { return OnWindows::Not; }
    let mut words = cmd[1..].iter().map(String::as_str).skip_while(|w| w.starts_with('+') || matches!(*w, "-q" | "--quiet" | "-v" | "-vv" | "--locked" | "--frozen" | "--offline"));
    let verb = match words.next() {
        Some("build" | "b") => "build",
        Some("check" | "c") => "check",
        Some("clippy") => "clippy",
        Some("test" | "t") => "test",
        _ => return OnWindows::Not,
    };
    let (mut pkgs, mut select) = (Vec::new(), Vec::new());
    while let Some(w) = words.next() {
        match w {
            "--" => break,
            "--workspace" | "--all" | "--manifest-path" | "--target" | "--examples" | "--benches" | "--example" | "--bench" | "-Z" => return OnWindows::Not,
            w if w.starts_with("--manifest-path=") || w.starts_with("--target=") || w.starts_with("-Z") => return OnWindows::Not,
            "-p" | "--package" => match words.next() { Some(p) => pkgs.push(p.to_string()), None => return OnWindows::Not },
            "--lib" | "--bins" | "--tests" | "--all-targets" => select.push(w[2..].to_string()),
            "--bin" | "--test" => match words.next() { Some(name) => select.push(format!("{}:{name}", &w[2..])), None => return OnWindows::Not },
            w => {
                if let Some(p) = w.strip_prefix("--package=").or_else(|| w.strip_prefix("-p").filter(|p| !p.is_empty())) { pkgs.push(p.to_string()); }
                else if let Some((kind, name)) = w.strip_prefix("--bin=").map(|n| ("bin", n)).or_else(|| w.strip_prefix("--test=").map(|n| ("test", n))) { select.push(format!("{kind}:{name}")); }
            }
        }
    }
    if pkgs.is_empty() { return OnWindows::Not; }
    if verb != "test" && select.is_empty() { select = vec!["lib".into(), "bins".into()]; }
    for pkg in &pkgs {
        let Some(mut gaps) = gaps(pkg) else { return OnWindows::Not };
        if let Some(why) = gaps.get("build") { return OnWindows::Gap(format!("{pkg} does not build on Windows: {why}")); }
        // (devhub's lib, whose tests fail there, kept every Windows check of its hub off the box, 11-19)
        if verb != "test" { gaps.remove("lib"); }
        if let Some((target, why)) = crate::jit_misses::native_selected(&gaps, &select).first() {
            return OnWindows::Gap(format!("{pkg} {target} on Windows: {why}"));
        }
    }
    OnWindows::Runs
}

/// [`on_windows`] for `cmd` in the lane's tree at `rev` (its pin, else the worktree's HEAD),
/// with the line to print when a gap keeps it off Windows.
pub fn windows_verdict(repo_dir: &Path, rev: &str, cmd: &[String]) -> (bool, String) {
    // (the tree is read only for a cargo command that could run there)
    let pair = std::cell::OnceCell::new();
    let verdict = on_windows(cmd, &|pkg| {
        let pair = pair.get_or_init(|| pair_of(repo_dir, rev).ok()).as_ref()?;
        crate::jit_misses::test_package(repo_dir, pair, pkg).ok().and_then(|mut p| p.platform_gaps.remove("windows"))
    });
    match verdict {
        OnWindows::Runs => (true, String::new()),
        OnWindows::Not => (false, String::new()),
        OnWindows::Gap(why) => (false, format!("devhub: no Windows box takes it: {why} (its Cargo.toml's platform-gaps)")),
    }
}

/// The review of a `--native "<reason>"` refuses what it would only warn of (main, 11-23: a day of
/// warnings first, then main sees their count and what they would have refused before this flips).
const REVIEW_REFUSES: bool = false;

/// When main last heard the review's count (wall ms), in the master's home ([`review_count`]).
const REVIEW_DAY: &str = "native-review-day";

/// The review's count goes to main once a day, not once a run (main, 11-23).
const REVIEW_DAY_MS: u64 = 24 * 3600 * 1000;

/// Why a `--native "<reason>"` build of `gated` (packages the JIT runs, no open miss, no target its
/// Cargo.toml declares, no measured route) does not pass on its reason; None: it does. Not its
/// words, which a lane can reword around: a timed or bench series, a native-vs-JIT baseline (the
/// same command runs the JIT, or builds a tree it made: `not_over` "baseline"), a crate of the JIT
/// binary (only a rebuild of that binary tries it), or a reason that names an open JIT miss.
fn review(home: &Path, a: &Ask, gated: &[Built], not_over: &str) -> Option<String> {
    if a.kind == Kind::Timed || !not_over.is_empty() || gated.iter().all(|b| b.verb == "bench") { return None; }
    let open: BTreeSet<String> = std::fs::read_to_string(home.join("jit_misses.jsonl")).unwrap_or_default().lines()
        .filter_map(|l| makepad_strict_json::parse(l.as_bytes()).ok())
        .filter(|v| v.get("closed_ms").and_then(Value::as_u64) == Some(0))
        .filter_map(|v| v.get("id")?.as_str().map(str::to_string)).collect();
    if words(a.reason).iter().any(|w| open.contains(w.trim_end_matches(['.', ':']))) { return None; }
    Some("it is no timed or bench series (a timed series is a `timed` lease, which drains its box, with `--needs gui=1` when it opens windows: its box's WM display), no native-vs-JIT baseline (the same command runs the JIT), no crate of the JIT binary, and its reason names no open JIT miss".into())
}

/// What a review that refuses says: where the run goes instead, the JIT's command and the
/// metadata route for a target that needs native.
fn review_route(a: &Ask, b: &Built) -> String {
    format!("{}; a test target that needs native (its own runner, a display server, input devices, an app bundle) is declared in its Cargo.toml's [package.metadata.makepad.native-tests] with its reason, reviewed with its landing",
        hint(a.lane, a.rev, b))
}

/// Main's count of the review's warnings, once [`REVIEW_DAY_MS`] passed since the last (the
/// first check starts the day): the runs it would have refused since, by lane, and the first few.
/// A read of the native ledger once a day; one small file's read a check otherwise.
fn review_count(home: &Path, now: u64) {
    let mark = home.join(REVIEW_DAY);
    let Some(since) = std::fs::read_to_string(&mark).ok().and_then(|t| t.trim().parse::<u64>().ok()) else {
        let _ = std::fs::write(&mark, now.to_string());
        return;
    };
    if now.saturating_sub(since) < REVIEW_DAY_MS { return; }
    let _ = std::fs::write(&mark, now.to_string());
    let rows: Vec<Value> = std::fs::read_to_string(home.join(LEDGER)).unwrap_or_default().lines()
        .filter_map(|l| makepad_strict_json::parse_depth(l.as_bytes(), 8).ok())
        .filter(|v| v.get("ms").and_then(Value::as_u64).is_some_and(|ms| ms >= since) && v.get("review").and_then(Value::as_str).is_some_and(|r| !r.is_empty())).collect();
    let mut by: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for v in &rows { *by.entry(v.get("lane").and_then(Value::as_str).unwrap_or("?").to_string()).or_default() += 1; }
    let lanes: Vec<String> = by.iter().map(|(lane, n)| format!("{lane} {n}")).collect();
    let first: Vec<String> = rows.iter().take(5).map(|v| format!("{} `{}` (--native \"{}\")", v.get("lane").and_then(Value::as_str).unwrap_or("?"),
        v.get("cmd").and_then(Value::as_str).unwrap_or(""), v.get("reason").and_then(Value::as_str).unwrap_or(""))).collect();
    let line = format!("native-reason review, the last day: {} run(s) it would have refused{}{}{}", rows.len(),
        if lanes.is_empty() { String::new() } else { format!(" (by lane: {})", lanes.join(", ")) },
        if first.is_empty() { String::new() } else { format!("; the first: {}", first.join("; ")) },
        if REVIEW_REFUSES { "" } else { "; it warns only until you say it refuses" });
    if let Err(e) = crate::notes::leave(home, "main", &line) { eprintln!("devhub native review: agent notes: {e}"); }
}

/// The rule (master only). Ok: the command may run, with a line to print (empty: nothing gated);
/// Err: refused, saying what to do instead.
pub fn check(home: &Path, a: &Ask, now: u64) -> Result<String, String> {
    // the JIT's own invocation (`cargo run --profile host -p makepad-run --bin makepad -- ..`)
    if matches!(a.kind, Kind::Inspect | Kind::Gate | Kind::Ci) || exempt_lane(a.lane, a.agent) || super::loaders::cargo_run_args(a.cmd).is_some() { return Ok(String::new()); }
    let dir = a.repo_dir.join(a.cwd);
    let read = |p: &str| -> Option<String> {
        let p = Path::new(p);
        if p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) { return None; }
        std::fs::read_to_string(dir.join(p)).ok()
    };
    let words = words(&command_text(a.cmd, &read));
    let mut built = built_in(&words);
    if built.is_empty() { return Ok(String::new()); }
    if let Some(here) = package_at(&dir) {
        for b in built.iter_mut().filter(|b| b.pkg == "*") { b.pkg = here.clone(); }
    }
    let on_jit = |b: &Built| if b.target.is_empty() { jit_runs(a.os, a.arch) } else { let (os, arch) = target_platform(&b.target); jit_runs(&os, &arch) };
    let mut not_over = if a.kind == Kind::Timed || runs_jit(&words) || builds_made_tree(&words) { "baseline" } else { "" };
    // a test whose every build on a JIT platform selects a target its Cargo.toml runs under cargo
    let mut declared = Vec::new();
    if built.iter().any(on_jit) {
        let pair = pair_of(a.repo_dir, a.rev)?;
        let mut own = BTreeSet::new();
        let mut all_declared = a.kind == Kind::Test;
        for b in built.iter().filter(|b| on_jit(b)) {
            let p = crate::jit_misses::test_package(a.repo_dir, &pair, &b.pkg)?;
            if p.own { own.insert(b.pkg.clone()); continue; }
            let said = if b.verb == "test" { crate::jit_misses::native_selected(&p.native, &b.select) } else { Vec::new() };
            all_declared &= !said.is_empty();
            declared.extend(said.into_iter().map(|(target, why)| format!("{} {target}: {why}", b.pkg)));
        }
        if !all_declared { declared.clear(); }
        built.retain(|b| !(on_jit(b) && own.contains(&b.pkg)));
        if built.is_empty() { return Ok(String::new()); }
        if not_over.is_empty() {
            let binary = crate::jit_misses::jit_binary_packages(a.repo_dir, &pair)?;
            if built.iter().filter(|b| on_jit(b)).all(|b| binary.contains(&b.pkg)) { not_over = "jit crate"; }
        }
    }
    let (gated, elsewhere): (Vec<Built>, Vec<Built>) = built.into_iter().partition(on_jit);
    verdict(home, a, &gated, &elsewhere, not_over, &declared.join("; "), now)
}

/// The class `devhub test` gives a cargo test of one package and at most one target
/// ([`super::client::test_class`]); None for any other command.
fn selected_class(gated: &[Built]) -> Option<String> {
    let [b] = gated else { return None };
    if b.verb != "test" { return None; }
    let target = match b.select.as_slice() {
        [] => None,
        [s] if s == "lib" || s.starts_with("bin:") => Some(s.clone()),
        [s] if s == "tests" => Some(super::client::ALL_TESTS.to_string()),
        [s] => Some(s.strip_prefix("test:")?.to_string()),
        _ => return None,
    };
    Some(super::client::test_class(&b.pkg, target.as_deref()))
}

/// The verdict on what the command builds natively on a JIT platform (`gated`) and elsewhere,
/// appended to the ledger. `declared`: the targets its Cargo.toml runs under cargo that a test
/// selects, each with why (empty: none, or a build that selects none).
fn verdict(home: &Path, a: &Ask, gated: &[Built], elsewhere: &[Built], not_over: &str, declared: &str, now: u64) -> Result<String, String> {
    let names = |bs: &[Built]| bs.iter().map(|b| b.pkg.clone()).fold(Vec::<String>::new(), |mut v, p| { if !v.contains(&p) { v.push(p); } v });
    let reason = a.reason.trim();
    let mut measured_reason = None;
    let mut miss = None;
    let mut reviewed = None;
    let (word, pkgs, answer) = if gated.is_empty() {
        let pkgs = names(elsewhere);
        let b = &elsewhere[0];
        let on = if b.target.is_empty() { format!("{} {}", a.os, a.arch) } else { b.target.clone() };
        let answer = Ok(format!("devhub: native build of {} for {on}: no JIT runs there (no JIT leg verifies it), so cargo is the way", pkgs.join(", ")));
        ("platform", pkgs, answer)
    } else {
        let pkgs = names(gated);
        let list = pkgs.join(", ");
        miss = pkgs.iter().find_map(|p| crate::jit_misses::open(home, p).map(|id| (p.clone(), id)));
        // the class it was routed by names the package it measures, `test:<pkg>[:<target>]`: the
        // lease's own, else (a custom --class names the lease's series) the package's as the
        // command selects it, which `devhub test` routes such a run by. A route granted at the run's
        // start is its own class's alone: the class it asked the route for
        let class = a.class.strip_prefix("native:").filter(|_| a.kind == Kind::Test).map(super::base_class);
        let own = class.filter(|c| c.split(':').nth(1).is_some_and(|p| pkgs.iter().all(|q| q == p))).map(str::to_string);
        let routed = class.and_then(|_| own.clone().or_else(|| selected_class(gated))).and_then(|c| (a.route)(&c))
            .or_else(|| own.filter(|_| !a.granted.is_empty()).map(|_| format!("{}, as granted at its run's start (the measure moved since)", a.granted)));
        let (word, answer) = if let Some((pkg, id)) = &miss {
            ("miss", Ok(format!("devhub: native build of {list}: {pkg} has the open JIT miss {id}, so cargo is the way until it closes")))
        } else if !declared.is_empty() {
            ("declared", Ok(format!("devhub: native test of {list}: its Cargo.toml runs {declared} (native-tests)")))
        } else if let Some(why) = &routed {
            // both medians belong in the ledger: fab-shell's 136 s cargo vs 40 s JIT route
            // could not be explained by its empty measured reason (11-15)
            measured_reason = Some(why.clone());
            ("measured", Ok(format!("devhub: native test of {list}: devhub's measured routing picked {why}")))
        } else if !reason.is_empty() {
            match review(home, a, gated, not_over) {
                None => ("reason", Ok(format!("devhub: native build of {list} (--native \"{reason}\", logged)"))),
                Some(why) if REVIEW_REFUSES => ("refused", Err(format!("native build of {list} refused on review of its reason: {why}. Instead: {}", review_route(a, &gated[0])))),
                Some(why) => {
                    reviewed = Some(why.clone());
                    ("reason", Ok(format!("devhub: native build of {list} (--native \"{reason}\", logged). Review, a warning for now (devhub will refuse this): {why}. Instead: {}", review_route(a, &gated[0]))))
                }
            }
        } else {
            ("refused", Err(format!("native build of {list} refused: lanes iterate and check under the JIT: {}. Native runs only for the JIT's own crates, a package with an open JIT miss, a platform without a JIT, a test devhub's measured routing sends to cargo, a test target its Cargo.toml's native-tests names, or with --native \"<reason>\" (a native-vs-JIT baseline, a timed perf series; logged)", hint(a.lane, a.rev, &gated[0]))))
        };
        (word, pkgs, answer)
    };
    let reason = measured_reason.as_deref().unwrap_or(reason);
    let line = format!("{{\"ms\":{now},\"lane\":{},\"agent\":{},\"kind\":{},\"pkgs\":[{}],\"platform\":{},\"verdict\":{},\"reason\":{},\"not_over\":{},\"class\":{},\"miss\":{},\"cmd\":{}{}}}\n",
        json_str(a.lane), json_str(a.agent), json_str(a.kind.as_str()), pkgs.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), json_str(&format!("{} {}", a.os, a.arch)),
        json_str(word), json_str(reason), json_str(not_over), json_str(a.class), json_str(miss.as_ref().map_or("", |(_, id)| id.as_str())), json_str(&crate::proto::clean(&a.cmd.join(" "), 200)),
        reviewed.as_deref().map_or(String::new(), |why| format!(",\"review\":{}", json_str(why))));
    use std::io::Write;
    let path = home.join(LEDGER);
    std::fs::OpenOptions::new().create(true).append(true).open(&path).and_then(|mut f| f.write_all(line.as_bytes())).map_err(|e| format!("{}: {e}", path.display()))?;
    review_count(home, now);
    answer
}

/// `native check {"lane","agent","kind","cmd":[..],"repo_dir","cwd","rev","reason","class","needs","node","route"}`
/// → `{"note":".."}`, or the refusal as the error.
pub fn serve(home: &Path, line: &str, now: u64, nodes: &[NodeView], route: &dyn Fn(&str) -> Option<String>) -> Result<String, String> {
    let body = line.strip_prefix("check ").ok_or("native check {json}")?;
    let v = makepad_strict_json::parse_depth(body.as_bytes(), 8).map_err(|_| "native check: bad request")?;
    let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let kind = Kind::parse(&s("kind")).ok_or("native check: kind")?;
    let cmd: Vec<String> = v.get("cmd").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect();
    // a lease asked outside a worktree: its words alone, against the master's checkout
    let repo_dir = Some(std::path::PathBuf::from(s("repo_dir"))).filter(|d| d.join(".git").exists()).unwrap_or_else(crate::orch::repo);
    let (lane, agent, cwd, rev, reason, class, granted) = (s("lane"), s("agent"), s("cwd"), s("rev"), s("reason"), s("class"), s("route"));
    let (os, arch) = platform(&super::sched::parse_needs(&s("needs")), &s("node"), nodes);
    let ask = Ask { lane: &lane, agent: &agent, kind, cmd: &cmd, repo_dir: &repo_dir, cwd: if cwd.is_empty() { "." } else { &cwd }, rev: &rev, reason: &reason, class: &class, route, granted: &granted, os: &os, arch: &arch };
    let note = check(home, &ask, now)?;
    let (windows, gap) = windows_verdict(&repo_dir, &rev, &cmd);
    let note = [note, gap].into_iter().filter(|n| !n.is_empty()).collect::<Vec<_>>().join("\n");
    Ok(format!("{{\"note\":{},\"windows\":{windows}}}", json_str(&note)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(s: &str) -> Vec<String> { s.split(' ').map(str::to_string).collect() }

    fn built(pkg: &str, verb: &'static str, target: &str) -> Built { Built { pkg: pkg.into(), verb, target: target.into(), select: Vec::new() } }

    /// A Windows box runs plain cargo builds, checks and tests of packages checked there, unless
    /// they select a gap: a package never checked, a shell, a JIT run, the workspace or another
    /// target stay off it. A build or check selects the lib and bins; a test every target.
    #[test]
    fn windows_runs_plain_cargo_work_of_checked_packages_without_their_gaps() {
        let gaps = |pkg: &str| match pkg {
            "clean" => Some(std::collections::BTreeMap::new()),
            "gappy" => Some([("test:modules".to_string(), "no sandbox there yet".to_string())].into()),
            "broken" => Some([("build".to_string(), "its Unix-only deps".to_string())].into()),
            "redlib" => Some([("lib".to_string(), "3 of 90 fail there".to_string())].into()),
            _ => None,
        };
        let on = |cmd: &str| on_windows(&argv(cmd), &gaps);
        assert_eq!(on("cargo test --release -p clean --no-fail-fast"), OnWindows::Runs);
        assert_eq!(on("cargo check -p gappy"), OnWindows::Runs, "a check selects the lib and bins");
        assert_eq!(on("cargo test --release -p gappy --lib"), OnWindows::Runs);
        assert_eq!(on("cargo test --release -p gappy"), OnWindows::Gap("gappy test:modules on Windows: no sandbox there yet".into()));
        assert_eq!(on("cargo test -p gappy --test modules"), OnWindows::Gap("gappy test:modules on Windows: no sandbox there yet".into()));
        assert_eq!(on("cargo check -p gappy --all-targets"), OnWindows::Gap("gappy test:modules on Windows: no sandbox there yet".into()));
        assert_eq!(on("cargo build -p broken --lib"), OnWindows::Gap("broken does not build on Windows: its Unix-only deps".into()));
        // a lib gap is its tests': its build, check and clippy run there
        for runs in ["cargo check -p redlib", "cargo build -p redlib --lib", "cargo clippy -p redlib"] {
            assert_eq!(on(runs), OnWindows::Runs, "{runs}");
        }
        assert_eq!(on("cargo test -p redlib --lib"), OnWindows::Gap("redlib lib on Windows: 3 of 90 fail there".into()));
        assert_eq!(on("cargo test -p clean -p unchecked"), OnWindows::Not, "a package never checked there");
        for not in ["cargo test", "cargo test --workspace", "cargo run -p clean", "cargo check -p clean --target wasm32-unknown-unknown", "bash -c cargo", "env A=1 cargo test -p clean", "cargo"] {
            assert_eq!(on(not), OnWindows::Not, "{not}");
        }
    }

    #[test]
    fn refused_unless_a_miss_a_route_a_reason_or_no_jit() {
        let home = std::env::temp_dir().join(format!("devhub-native-{}", std::process::id()));
        // a warm live process keeps its pid: a failed run's home is still there
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let cmd = argv("cargo test --release -p demo-model");
        let route = |class: &str| (class == "test:demo-model:lib").then(|| "cargo (cargo 2m, JIT 4m)".to_string());
        let ask = |reason, class, os| Ask { lane: "l1", agent: "", kind: Kind::Test, cmd: &cmd, repo_dir: &home, cwd: ".", rev: "", reason, class, route: &route, granted: "", os, arch: "x64" };
        let test = [built("demo-model", "test", "")];
        let refused = verdict(&home, &ask("", "", "linux"), &test, &[], "", "", 1).unwrap_err();
        assert!(refused.contains("`devhub test l1 demo-model --live`") && refused.contains("--native"), "{refused}");
        // a cargo test of a landed commit is pointed at the JIT's run of those commits
        let refused = verdict(&home, &Ask { rev: "work@0123456789", ..ask("", "", "linux") }, &test, &[], "", "", 1).unwrap_err();
        assert!(refused.contains("`devhub test l1 demo-model --rev work@0123456789` runs it under the JIT at those commits alone"), "{refused}");
        assert!(verdict(&home, &ask("perf baseline", "", "linux"), &test, &[], "baseline", "", 2).unwrap().contains("perf baseline"));
        assert!(verdict(&home, &ask("", "native:test:other:lib", "linux"), &test, &[], "", "", 3).is_err(), "another package's class routes nothing");
        assert!(verdict(&home, &ask("", "native:test:demo-model:lib", "linux"), &test, &[], "", "", 3).unwrap().contains("cargo 2m, JIT 4m"));
        assert!(verdict(&home, &ask("", "", "windows"), &[], &test, "", "", 4).unwrap().contains("for windows x64: no JIT runs there"));
        assert!(verdict(&home, &ask("", "", "linux"), &[], &[built("app", "build", "wasm32-unknown-unknown")], "", "", 5).unwrap().contains("for wasm32-unknown-unknown: no JIT runs there"));
        // a test of a target its Cargo.toml runs under cargo (native-tests)
        let said = verdict(&home, &ask("", "", "linux"), &test, &[], "", "demo-model test:spawns: its runner is its own binary", 6).unwrap();
        assert!(said.contains("its Cargo.toml runs demo-model test:spawns: its runner is its own binary (native-tests)"), "{said}");
        std::fs::write(home.join("jit_misses.jsonl"), "{\"id\":\"M7\",\"pkg\":\"demo-model\",\"target\":\"--lib\",\"error\":[\"e\"],\"closed_ms\":0}\n").unwrap();
        assert!(verdict(&home, &ask("", "", "linux"), &test, &[], "", "", 6).unwrap().contains("open JIT miss M7"));
        let ledger = std::fs::read_to_string(home.join(LEDGER)).unwrap();
        let verdicts: Vec<&str> = ledger.lines().map(|l| l.split("\"verdict\":\"").nth(1).unwrap().split('"').next().unwrap()).collect();
        assert_eq!(verdicts, ["refused", "refused", "reason", "refused", "measured", "platform", "platform", "declared", "miss"]);
        let measured = makepad_strict_json::parse_depth(ledger.lines().nth(4).unwrap().as_bytes(), 8).unwrap();
        assert_eq!(measured.get("reason").and_then(Value::as_str), Some("cargo (cargo 2m, JIT 4m)"), "a measured native verdict records the route and both medians");
        assert!(ledger.lines().nth(2).unwrap().contains(r#""not_over":"baseline""#), "{ledger}");
        // A reason is reviewed by what devhub knows, not by its words (main, 11-23): a timed or
        // bench series, a baseline, a crate of the JIT binary, or a reason naming an open JIT miss
        // passes; any other runs with a warning for now, naming the JIT's command and the metadata
        // route, and main hears one count a day.
        let other = [built("demo-view", "test", "")];
        let warned = verdict(&home, &ask("verify the regressions natively", "", "linux"), &other, &[], "", "", 7).unwrap();
        assert!(warned.contains("Review, a warning for now (devhub will refuse this): it is no timed or bench series") && warned.contains("`devhub test l1 demo-view --live`")
            && warned.contains("a timed series is a `timed` lease, which drains its box, with `--needs gui=1` when it opens windows")
            && warned.contains("[package.metadata.makepad.native-tests]"), "{warned}");
        std::fs::write(home.join("jit_misses.jsonl"), "{\"id\":\"M7\",\"pkg\":\"demo-model\",\"target\":\"--lib\",\"error\":[\"e\"],\"closed_ms\":0}\n{\"id\":\"M9\",\"pkg\":\"demo-core\",\"target\":\"--lib\",\"error\":[\"e\"],\"closed_ms\":0}\n").unwrap();
        let passed = [
            verdict(&home, &ask("compare with M9.", "", "linux"), &other, &[], "", "", 8).unwrap(),
            verdict(&home, &Ask { kind: Kind::Timed, ..ask("perf series", "", "linux") }, &other, &[], "", "", 9).unwrap(),
            verdict(&home, &ask("bench", "", "linux"), &[built("demo-view", "bench", "")], &[], "", "", 10).unwrap(),
            verdict(&home, &ask("platform crate", "", "linux"), &other, &[], "jit crate", "", 11).unwrap(),
        ];
        assert!(passed.iter().all(|said| !said.contains("Review")), "an open miss it names, a timed run, a bench, a crate of the JIT binary: {passed:?}");
        let reviews: Vec<bool> = std::fs::read_to_string(home.join(LEDGER)).unwrap().lines().skip(9).map(|l| l.contains("\"review\":")).collect();
        assert_eq!(reviews, [true, false, false, false, false], "the ledger marks the run it would refuse");
        // a run of a custom --class (a timed series' `--class timed`) is routed by its package's
        // class, which the gate names from the command's selection: routed to cargo, it builds
        // (one was refused, 11-24); another target's or package's class still routes nothing. An
        // open miss comes first: the file is set aside for them
        let misses = std::fs::read_to_string(home.join("jit_misses.jsonl")).unwrap();
        std::fs::remove_file(home.join("jit_misses.jsonl")).unwrap();
        let lib = [Built { select: vec!["lib".into()], ..built("demo-model", "test", "") }];
        let routed = verdict(&home, &ask("", "native:timed", "linux"), &lib, &[], "", "", 12).unwrap();
        assert!(routed.contains("measured routing picked cargo (cargo 2m, JIT 4m)"), "{routed}");
        let one_test = [Built { select: vec!["test:zz_drag".into()], ..built("demo-model", "test", "") }];
        assert!(verdict(&home, &ask("", "native:timed", "linux"), &one_test, &[], "", "", 13).is_err(), "test:demo-model:zz_drag is measured JIT-first");
        assert!(verdict(&home, &ask("", "timed", "linux"), &lib, &[], "", "", 14).is_err(), "a class devhub did not send to cargo routes nothing");
        assert_eq!(selected_class(&one_test).as_deref(), Some("test:demo-model:zz_drag"));
        assert_eq!(selected_class(&[Built { select: vec!["tests".into()], ..built("demo-model", "test", "") }]).as_deref(), Some("test:demo-model:tests:*"));
        assert_eq!(selected_class(&[Built { select: vec!["lib".into(), "bins".into()], ..built("demo-model", "test", "") }]), None);
        // a run devhub's routing sent to cargo keeps that route when the measure moved after its
        // route ask (a run ended between, or the master restarted, 11-25): its check carries the
        // route it was granted. Another package's class still routes nothing, granted or not
        let granted = |class| Ask { granted: "cargo (cargo 1m, JIT 3m)", ..ask("", class, "linux") };
        let kept = verdict(&home, &granted("native:test:demo-view:lib"), &other, &[], "", "", 15).unwrap();
        assert!(kept.contains("measured routing picked cargo (cargo 1m, JIT 3m), as granted at its run's start"), "{kept}");
        assert!(verdict(&home, &ask("", "native:test:demo-view:lib", "linux"), &other, &[], "", "", 16).is_err(), "no grant, no measure: refused");
        assert!(verdict(&home, &granted("native:test:other:lib"), &other, &[], "", "", 17).is_err(), "a grant routes only its own package's class");
        std::fs::write(home.join("jit_misses.jsonl"), misses).unwrap();
        assert!(crate::notes::take(&home, "main").is_empty(), "the first check starts the day");
        verdict(&home, &ask("again", "", "linux"), &other, &[], "", "", 1 + 24 * 3_600_000).unwrap();
        let heard = crate::notes::take(&home, "main");
        assert!(heard.len() == 1 && heard[0].1.starts_with("native-reason review, the last day: 2 run(s) it would have refused (by lane: l1 2); the first: l1 `cargo test --release -p demo-model` (--native \"verify the regressions natively\")"), "one count a day: {heard:?}");
        verdict(&home, &ask("again", "", "linux"), &other, &[], "", "", 2 + 24 * 3_600_000).unwrap();
        assert!(crate::notes::take(&home, "main").is_empty(), "not once a run");
        std::fs::remove_dir_all(&home).unwrap();
    }

    /// A native build beside the JIT, by any path to it, is a baseline; the word in a path or an
    /// argument is not.
    #[test]
    fn a_command_that_runs_the_jit_too_is_seen() {
        let jit = |t: &str| runs_jit(&words(t));
        assert!(jit("cargo test --release -p a\n/x/target/host/makepad test a --lib"));
        assert!(jit("cargo build -p a; makepad run a"));
        assert!(jit("cargo run --profile host -p makepad-run --bin makepad -- test a"));
        assert!(!jit("cargo test --release -p a; cd makepad; ls test"));
        assert!(!jit("cargo test --release -p makepad-foo"));
        // a build of a tree the command exported to a dir of its own is a baseline too
        let made = |t: &str| builds_made_tree(&words(t));
        assert!(made("set -u\nO=$(mktemp -d /tmp/t.XXXXXX); [ -n \"$O\" ] || exit 1\ngit archive HEAD -- libs | tar -x -C \"$O\"\ncd \"$O\" && cargo build --release --workspace > \"$O/b.log\"\ncd / && rm -rf \"$O\""));
        assert!(made("D=$(mktemp -d); cd $D/sub; cargo test"));
        assert!(!made("O=$(mktemp -d); cargo build -p a; cd $O"), "the workspace's build, before the cd");
        assert!(!made("O=$(mktemp -d); cd $O; cd /x; cargo build"), "a cd out of it");
        assert!(!made("cd $REPO && cargo build -p a"), "a dir it did not make");
    }

    #[test]
    fn platforms_from_needs_targets_and_the_jit_legs() {
        let p = |needs: &str| platform(&super::super::sched::parse_needs(needs), "", &[]);
        assert_eq!(p(""), ("linux".to_string(), "x64".to_string()));
        assert_eq!(p("os=macos"), ("macos".to_string(), "arm64".to_string()));
        assert_eq!(p("arch=x86_64,os=windows"), ("windows".to_string(), "x64".to_string()));
        assert_eq!(target_platform("aarch64-linux-android"), ("android".to_string(), "arm64".to_string()));
        assert_eq!(target_platform("x86_64-pc-windows-msvc"), ("windows".to_string(), "x64".to_string()));
        assert!(jit_runs("linux", "x64") && jit_runs("macos", "arm64"));
        assert!(!jit_runs("windows", "x64") && !jit_runs("wasm32-unknown-unknown", "wasm32"));
    }

    #[test]
    fn builds_reads_words_texts_and_scripts() {
        let none = |_: &str| None;
        let b = |cmd: &str| builds(&argv(cmd), &none);
        assert_eq!(b("cargo run --release -p makepad-example-demo -- --flag -p x"), vec![built("makepad-example-demo", "run", "")]);
        assert_eq!(b("env A=1 nice -n 10 cargo +nightly -q test --release --package=a -pb"), vec![built("a", "test", ""), built("b", "test", "")]);
        assert!(b("cargo check --release -p a").is_empty());
        // the targets a test selects, as a Cargo.toml's native-tests names them
        assert_eq!(b("cargo test --release -p a --test spawns --lib --bin=x --tests -- --test y").into_iter().map(|b| b.select).collect::<Vec<_>>(), vec![vec!["test:spawns", "lib", "bin:x", "tests"]]);
        assert_eq!(b("cargo build --release --target wasm32-unknown-unknown"), vec![built("*", "build", "wasm32-unknown-unknown")]);
        let script = |p: &str| (p == "drive.sh").then(|| "#!/bin/bash\n# cargo build -p commented\ncargo build --release \\\n  -q -p makepad-run --bin makepad || exit 1\nM=x; cargo run -p app; ./x -p y\n".to_string());
        assert_eq!(builds(&argv("bash drive.sh"), &script), vec![built("makepad-run", "build", ""), built("app", "run", "")]);
        let py = |_: &str| Some("subprocess.run([\"cargo\", \"test\", \"--release\", \"-p\", \"demo-model\"], check=True)".to_string());
        assert_eq!(builds(&argv("python3 t.py"), &py), vec![built("demo-model", "test", "")]);
        // the bare `makepad` a run gets, called in a `-c` text: a gate unit's repro as a lane runs it
        #[cfg(feature = "checkout")]
        {
            let c = |text: &str| calls_makepad(&["bash".to_string(), "-c".to_string(), text.to_string()]);
            assert!(c("cd apps/x && MAKEPAD_HIDE_WINDOWS=1 timeout 240 makepad run --report-to r app -- --remote > out.log"));
            assert!(c("echo a;makepad check app"), "after a separator");
            assert!(!c("cat \"$(command -v makepad)\"") && !c("exit 0"), "the word alone, or none");
            assert!(!c("./target/host/makepad run app"), "a binary of its own");
            assert!(!calls_makepad(&argv("bash tools/run/fixture/land_gate.sh fast a b")), "a script file is not read");
        }
    }
}
