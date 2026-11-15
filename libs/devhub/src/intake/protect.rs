//! Protected paths and the land rule (docs/agents/intake.md, the list in `protected.txt`).
//!
//! Every landing is classified by what it changes:
//! - **t1-hard** paths always need an approval the user gave at a terminal (`intake approve`);
//! - **t1-origin** paths need it only from a lane made from an intake item (`intake route`);
//!   from any other lane the landing is recorded for the daily push review;
//! - **t2** paths need an adversarial-review record (`intake reviewed`) from an intake lane, and
//!   are recorded otherwise.
//!
//! An approval or a review covers the exact blobs it saw at the lane's tip: a later change to one
//! of those files needs a new one. The records live in devhub's home on the master
//! (`intake/records.jsonl`, `intake/lanes.jsonl`, `intake/protected.jsonl`), append-only: no
//! command removes an intake lane's mark or an approval.

use makepad_strict_json::{self as json, Value};
use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

pub const LIST: &str = include_str!("protected.txt");

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tier {
    T2,
    Origin,
    Hard,
}

impl Tier {
    pub fn word(self) -> &'static str {
        match self {
            Tier::Hard => "t1-hard",
            Tier::Origin => "t1-origin",
            Tier::T2 => "t2",
        }
    }
    fn parse(word: &str) -> Option<Tier> {
        [Tier::Hard, Tier::Origin, Tier::T2].into_iter().find(|t| t.word() == word)
    }
}

/// The list's rules; a line with an unknown tier counts as t1-hard (the list fails closed).
fn rules() -> Vec<(Tier, &'static str)> {
    LIST.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once(' ').map(|(t, p)| (Tier::parse(t).unwrap_or(Tier::Hard), p.trim())))
        .collect()
}

fn matches(pattern: &str, path: &str) -> bool {
    if let Some(name) = pattern.strip_prefix("*/") {
        return match name.strip_suffix('/') {
            Some(dir) => path.starts_with(&format!("{dir}/")) || path.contains(&format!("/{dir}/")),
            None => path == name || path.ends_with(&format!("/{name}")),
        };
    }
    if pattern.ends_with('/') {
        return path.starts_with(pattern);
    }
    path == pattern
}

/// A test's or a fixture's path: t2 does not record it.
fn test_path(path: &str) -> bool {
    path.split('/').any(|c| matches!(c, "tests" | "fixture" | "fixtures" | "testdata"))
}

/// The most severe tier a path falls under (the root Cargo.toml is ruled per hunk, not here).
pub fn tier_of(path: &str) -> Option<Tier> {
    rules().into_iter().filter(|(t, p)| matches(p, path) && !(*t == Tier::T2 && test_path(path))).map(|(t, _)| t).max()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Touches {
    pub hard: Vec<String>,
    pub origin: Vec<String>,
    pub t2: Vec<String>,
}

impl Touches {
    pub fn is_empty(&self) -> bool {
        self.hard.is_empty() && self.origin.is_empty() && self.t2.is_empty()
    }
    fn add(&mut self, tier: Tier, path: &str) {
        let list = match tier {
            Tier::Hard => &mut self.hard,
            Tier::Origin => &mut self.origin,
            Tier::T2 => &mut self.t2,
        };
        if !list.iter().any(|p| p == path) {
            list.push(path.to_string());
        }
    }
}

/// The section a 1-based line of a TOML file sits in (`patch.crates-io`, `workspace`), the line
/// itself when it is a header; "" before the first header.
fn section_at(text: &str, line: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    for l in lines[..line.min(lines.len())].iter().rev() {
        let t = l.trim();
        if t.starts_with('[') {
            let inner = t.trim_start_matches('[');
            let inner = inner.split(']').next().unwrap_or("");
            return inner.split('.').map(|p| p.trim().trim_matches('"')).collect::<Vec<_>>().join(".");
        }
    }
    String::new()
}

/// The body of the section a 1-based line sits in.
fn section_body(text: &str, line: usize) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let line = line.clamp(1, lines.len().max(1));
    let start = (0..line).rev().find(|&i| lines.get(i).is_some_and(|l| l.trim().starts_with('['))).unwrap_or(0);
    let end = (start + 1..lines.len()).find(|&i| lines[i].trim().starts_with('[')).unwrap_or(lines.len());
    lines.get(start..end).map(|ls| ls.join("\n")).unwrap_or_default()
}

fn is_dependency_section(section: &str) -> bool {
    section.split('.').any(|p| matches!(p, "dependencies" | "dev-dependencies" | "build-dependencies"))
}

/// Whether TOML text sets the key (`key = `, `{ key = `, `, key=`).
fn has_key(text: &str, key: &str) -> bool {
    text.match_indices(key).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let ident = |c: char| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.';
        before.is_none_or(|c| !ident(c)) && text[at + key.len()..].trim_start().starts_with('=')
    })
}

/// One changed line of the root Cargo.toml: t1-hard for [patch] and any non-path dependency
/// (a new external dependency needs the user), t1-origin for the rest ([profile], members, path
/// dependencies).
fn cargo_line_tier(text: &str, line: usize) -> Tier {
    let section = section_at(text, line);
    if section.split('.').next() == Some("patch") {
        return Tier::Hard;
    }
    if is_dependency_section(&section) {
        let changed = text.lines().nth(line.saturating_sub(1)).unwrap_or("").trim();
        if changed.is_empty() || changed.starts_with('#') {
            return Tier::Origin;
        }
        // `foo = { path = .. }`, or a `[dependencies.foo]` table that names a path; a git or
        // registry source makes it foreign whatever else it says
        let scope = if section.ends_with("dependencies") { changed.to_string() } else { section_body(text, line) };
        let local = has_key(&scope, "path") && !has_key(&scope, "git") && !has_key(&scope, "registry");
        return if local { Tier::Origin } else { Tier::Hard };
    }
    Tier::Origin
}

/// The root Cargo.toml's change, per hunk of a `git diff -U0`: the most severe tier of its changed
/// lines (None: no changed line).
pub fn cargo_tier(old: &str, new: &str, diff_u0: &str) -> Option<Tier> {
    let mut tier = None;
    for line in diff_u0.lines() {
        let Some(rest) = line.strip_prefix("@@ -") else { continue };
        let mut parts = rest.split(' ');
        let span = |s: Option<&str>| -> Option<(usize, usize)> {
            let s = s?.trim_start_matches(['-', '+']);
            let (a, n) = s.split_once(',').unwrap_or((s, "1"));
            Some((a.parse().ok()?, n.parse().ok()?))
        };
        let (Some((oa, on)), Some((na, nn))) = (span(parts.next()), span(parts.next())) else {
            return Some(Tier::Hard); // a hunk head we cannot read: fail closed
        };
        for l in oa..oa + on {
            tier = tier.max(Some(cargo_line_tier(old, l)));
        }
        for l in na..na + nn {
            tier = tier.max(Some(cargo_line_tier(new, l)));
        }
    }
    tier
}

/// What a set of changed paths touches; `cargo` is the root Cargo.toml's (old, new, `-U0` diff)
/// when it changed.
pub fn classify(paths: &[String], cargo: Option<(&str, &str, &str)>) -> Touches {
    let mut t = Touches::default();
    for p in paths {
        if p == "Cargo.toml" {
            let tier = cargo.map_or(Some(Tier::Hard), |(o, n, d)| cargo_tier(o, n, d));
            if let Some(tier) = tier {
                t.add(tier, p);
            }
        } else if let Some(tier) = tier_of(p) {
            t.add(tier, p);
        }
    }
    t
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output().map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// What the change from `from` to `to` touches (a landing: work's tip to the merge).
pub fn touches(dir: &Path, from: &str, to: &str) -> Result<Touches, String> {
    let names: Vec<String> = git(dir, &["diff", "--name-only", "--no-renames", from, to])?.lines().map(str::to_string).collect();
    let cargo = if names.iter().any(|n| n == "Cargo.toml") {
        let old = git(dir, &["show", &format!("{from}:Cargo.toml")]).unwrap_or_default();
        let new = git(dir, &["show", &format!("{to}:Cargo.toml")]).unwrap_or_default();
        Some((old, new, git(dir, &["diff", "-U0", "--no-renames", from, to, "--", "Cargo.toml"])?))
    } else {
        None
    };
    Ok(classify(&names, cargo.as_ref().map(|(o, n, d)| (o.as_str(), n.as_str(), d.as_str()))))
}

/// The blob each path has at `rev` (`-`: absent there).
pub fn blobs(dir: &Path, rev: &str, paths: &[String]) -> Result<Vec<(String, String)>, String> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    let mut args = vec!["ls-tree", "-r", rev, "--"];
    args.extend(paths.iter().map(String::as_str));
    let listed = git(dir, &args)?;
    let found: BTreeMap<&str, &str> = listed.lines().filter_map(|l| l.split_once('\t')).filter_map(|(meta, path)| Some((path, meta.split(' ').nth(2)?))).collect();
    Ok(paths.iter().map(|p| (p.clone(), found.get(p.as_str()).map_or("-".to_string(), |b| b.to_string()))).collect())
}

// ---- records ----

pub fn dir() -> PathBuf {
    crate::proto::home().join("intake")
}

fn append(name: &str, line: &Value) -> Result<(), String> {
    let d = dir();
    std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
    let p = d.join(name);
    let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&p).map_err(|e| format!("{}: {e}", p.display()))?;
    writeln!(f, "{}", line.to_json()).map_err(|e| format!("{}: {e}", p.display()))
}

fn read(name: &str) -> Vec<Value> {
    let Ok(f) = std::fs::File::open(dir().join(name)) else { return Vec::new() };
    std::io::BufReader::new(f).lines().map_while(Result::ok).filter_map(|l| json::parse(l.as_bytes()).ok()).collect()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

/// A lane made from an intake item (`intake route`): marked for good.
pub fn intake_lane(lane: &str) -> bool {
    read("lanes.jsonl").iter().any(|v| v.get("lane").and_then(Value::as_str) == Some(lane))
}

/// Whether an `approve` (or `review`) record for the lane saw exactly these blobs.
fn covered(records: &[Value], kind: &str, lane: &str, current: &[(String, String)]) -> bool {
    current.is_empty()
        || records.iter().filter(|r| r.get("kind").and_then(Value::as_str) == Some(kind) && r.get("lane").and_then(Value::as_str) == Some(lane)).any(|r| {
            let Some(Value::Arr(seen)) = r.get("blobs") else { return false };
            let seen: Vec<(&str, &str)> = seen
                .iter()
                .filter_map(|b| match b {
                    Value::Arr(pair) if pair.len() == 2 => Some((pair[0].as_str()?, pair[1].as_str()?)),
                    _ => None,
                })
                .collect();
            current.iter().all(|(p, b)| seen.contains(&(p.as_str(), b.as_str())))
        })
}

/// The land rule for one landing: Err with why it may not land. `lane_tip` is the lane's commit
/// the landing merges.
pub fn rule(dir: &Path, lane: &str, lane_tip: &str, touches: &Touches, intake: bool, records: &[Value]) -> Result<(), Vec<String>> {
    let mut need_approval = touches.hard.clone();
    if intake {
        need_approval.extend(touches.origin.iter().cloned());
    }
    let mut why = Vec::new();
    let approved = blobs(dir, lane_tip, &need_approval).map_err(|e| vec![e])?;
    if !covered(records, "approve", lane, &approved) {
        why.push(format!("it changes protected paths ({}) without the user's approval for these exact files: the user runs `makepad devhub intake approve {lane}` at a terminal", need_approval.join(" ")));
    }
    if intake && !touches.t2.is_empty() {
        let reviewed = blobs(dir, lane_tip, &touches.t2).map_err(|e| vec![e])?;
        if !covered(records, "review", lane, &reviewed) {
            why.push(format!("a lane made from an intake item changes t2 paths ({}) without an adversarial-review record: `makepad devhub intake reviewed {lane} --by <reviewer> --report <file>`", touches.t2.join(" ")));
        }
    }
    if why.is_empty() { Ok(()) } else { Err(why) }
}

/// The rule at a landing (devhub land), reading the records: Ok with the ledger line of what it
/// lets through (None: no protected path), or Err with why the landing may not land. `from` is
/// work's tip, `merge` the landing's merge, `lane_tip` the lane's commit.
pub fn check(dir: &Path, branch: &str, from: &str, merge: &str, lane_tip: &str) -> Result<Option<Value>, Vec<String>> {
    let lane = branch.strip_prefix("rc-").unwrap_or(branch);
    let t = touches(dir, from, merge).map_err(|e| vec![format!("the protected-path rule could not read the landing: {e}")])?;
    if t.is_empty() {
        return Ok(None);
    }
    let intake = intake_lane(lane);
    rule(dir, lane, lane_tip, &t, intake, &read("records.jsonl"))?;
    let tiers: Vec<Value> = [(Tier::Hard, &t.hard), (Tier::Origin, &t.origin), (Tier::T2, &t.t2)]
        .into_iter()
        .filter(|(_, ps)| !ps.is_empty())
        .map(|(tier, ps)| json::obj(vec![("tier", json::s(tier.word())), ("paths", Value::Arr(ps.iter().map(|p| json::s(p.as_str())).collect()))]))
        .collect();
    Ok(Some(json::obj(vec![("ms", Value::Int(now_ms() as i64)), ("lane", json::s(lane)), ("intake", Value::Bool(intake)), ("merge", json::s(merge)), ("tiers", Value::Arr(tiers))])))
}

/// [`check`] at the landing itself: the line goes into the ledger (`intake/protected.jsonl`) that the
/// daily push review lists.
pub fn land_check(dir: &Path, branch: &str, from: &str, merge: &str, lane_tip: &str) -> Result<(), Vec<String>> {
    if let Some(line) = check(dir, branch, from, merge, lane_tip)? {
        if let Err(e) = append("protected.jsonl", &line) {
            eprintln!("intake: the protected-path ledger did not take {branch}'s landing: {e}");
        }
    }
    Ok(())
}

/// The ledger's landings since `since_ms`, one line each: `<lane> <tier>: <paths>` (the push review).
pub fn ledger_since(since_ms: u64) -> Vec<String> {
    let mut out = Vec::new();
    for v in read("protected.jsonl") {
        if v.get("ms").and_then(Value::as_i64).unwrap_or(0) < since_ms as i64 {
            continue;
        }
        let lane = v.get("lane").and_then(Value::as_str).unwrap_or("?");
        let Some(Value::Arr(tiers)) = v.get("tiers") else { continue };
        for t in tiers {
            let tier = t.get("tier").and_then(Value::as_str).unwrap_or("?");
            let paths: Vec<&str> = match t.get("paths") {
                Some(Value::Arr(ps)) => ps.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            out.push(format!("{lane} {tier}: {}", paths.join(" ")));
        }
    }
    out
}

// ---- the tripwire ----

/// The tracked agent-instruction files the tripwire hashes.
pub const TRIPWIRE: [&str; 3] = ["AGENTS.md", "docs/agents", ".claude"];

/// The agents' memory folder for a checkout: `~/.claude/projects/<the checkout's path, / as ->/memory`.
pub fn memory_dir(repo: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let slug = repo.to_str()?.replace(['/', '.'], "-");
    Some(PathBuf::from(home).join(".claude/projects").join(slug).join("memory"))
}

/// `<path> <digest>` for every tripwire file: the tracked ones at `rev` by blob, the memory folder's
/// by sha256.
pub fn tripwire_state(dir: &Path, rev: &str, memory: Option<&Path>) -> Result<BTreeMap<String, String>, String> {
    let mut args = vec!["ls-tree", "-r", rev, "--"];
    args.extend(TRIPWIRE);
    let mut out = BTreeMap::new();
    for l in git(dir, &args)?.lines() {
        if let Some((meta, path)) = l.split_once('\t') {
            out.insert(path.to_string(), meta.split(' ').nth(2).unwrap_or("").to_string());
        }
    }
    if let Some(mem) = memory {
        let mut stack = vec![mem.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&d) else { continue };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if let Ok(bytes) = std::fs::read(&p) {
                    let rel = p.strip_prefix(mem).unwrap_or(&p).to_string_lossy().into_owned();
                    out.insert(format!("memory/{rel}"), crate::intake::sha256_hex(&bytes));
                }
            }
        }
    }
    Ok(out)
}

/// The names that differ between two tripwire states (added, changed or gone).
pub fn tripwire_diff(old: &BTreeMap<String, String>, new: &BTreeMap<String, String>) -> Vec<String> {
    let mut names: Vec<String> = new.iter().filter(|(k, v)| old.get(*k) != Some(v)).map(|(k, _)| k.clone()).collect();
    names.extend(old.keys().filter(|k| !new.contains_key(*k)).cloned());
    names.sort();
    names
}

/// At a landing: what changed in the tripwire set since the last landing (None: nothing, or the
/// first run, which only records). The new state is kept in `intake/tripwire.txt`.
pub fn tripwire_check(dir: &Path, rev: &str) -> Option<String> {
    let new = match tripwire_state(dir, rev, memory_dir(dir).as_deref()) {
        Ok(s) => s,
        Err(e) => return Some(format!("the tripwire could not read the agent files: {e}")),
    };
    let file = self::dir().join("tripwire.txt");
    let old: Option<BTreeMap<String, String>> = std::fs::read_to_string(&file).ok().map(|t| t.lines().filter_map(|l| l.rsplit_once(' ')).map(|(k, v)| (k.to_string(), v.to_string())).collect());
    let text: String = new.iter().map(|(k, v)| format!("{k} {v}\n")).collect();
    if std::fs::create_dir_all(self::dir()).and_then(|_| std::fs::write(&file, text)).is_err() {
        return Some(format!("the tripwire could not write {}", file.display()));
    }
    let changed = tripwire_diff(&old?, &new);
    (!changed.is_empty()).then(|| format!("agent-instruction files changed (AGENTS.md, docs/agents, .claude, memory): {}", changed.join(" ")))
}

// ---- commands ----

/// An agent's environment: an approval is the user's, typed at a terminal.
fn agent_env() -> Option<String> {
    std::env::vars().map(|(k, _)| k).find(|k| k == "DEVHUB_AGENT" || k == "CLAUDECODE" || k.starts_with("CLAUDE_CODE_") || k.starts_with("CODEX_") || k.starts_with("GROK_"))
}

fn repo_dir() -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    Ok(PathBuf::from(git(&cwd, &["rev-parse", "--show-toplevel"])?.trim()))
}

/// The lane's change against work: (tip, touches).
fn lane_change(dir: &Path, lane: &str) -> Result<(String, Touches), String> {
    let branch = format!("rc-{lane}");
    let tip = git(dir, &["rev-parse", "--verify", &format!("{branch}^{{commit}}")])?.trim().to_string();
    let base = git(dir, &["merge-base", "work", &tip])?.trim().to_string();
    Ok((tip.clone(), touches(dir, &base, &tip)?))
}

/// `intake approve <lane>`: the user's approval, at a terminal, of the protected files the lane
/// changes, as they are at its tip now.
pub fn approve(args: &[&str]) -> i32 {
    let Some(lane) = args.first().copied() else {
        eprintln!("usage: makepad devhub intake approve <lane>   (the user, at a terminal)");
        return 2;
    };
    if let Some(var) = agent_env() {
        eprintln!("intake approve: refused in an agent's environment ({var} is set): an approval is the user's, typed at a terminal");
        return 1;
    }
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        eprintln!("intake approve: refused without a terminal");
        return 1;
    }
    let result = repo_dir().and_then(|d| lane_change(&d, lane).map(|c| (d, c)));
    let (dir, (tip, t)) = match result {
        Ok(x) => x,
        Err(e) => {
            eprintln!("intake approve: {e}");
            return 1;
        }
    };
    let paths: Vec<String> = t.hard.iter().chain(t.origin.iter()).cloned().collect();
    if paths.is_empty() {
        println!("lane {lane} changes no t1 path: nothing to approve");
        return 0;
    }
    println!("lane {lane} at {tip:.12} changes these protected files:");
    for p in &t.hard {
        println!("  t1-hard    {p}");
    }
    for p in &t.origin {
        println!("  t1-origin  {p}");
    }
    let range = format!("work...{tip}");
    let mut stat_args = vec!["diff", "--stat", range.as_str(), "--"];
    stat_args.extend(paths.iter().map(String::as_str));
    if let Ok(stat) = git(&dir, &stat_args) {
        print!("{stat}");
    }
    print!("Read the diff (git diff {range} -- <paths>). Type `approve {lane}` to approve these exact files: ");
    let _ = std::io::stdout().flush();
    let mut answer = String::new();
    if std::io::stdin().read_line(&mut answer).is_err() || answer.trim() != format!("approve {lane}") {
        println!("not approved");
        return 1;
    }
    record("approve", lane, &dir, &tip, &paths, vec![("by", json::s("tty"))])
}

/// `intake reviewed <lane> --by <reviewer> --report <file>`: the adversarial review of the t2
/// files a lane made from an intake item changes. The lane's own agent may not record it.
pub fn reviewed(args: &[&str]) -> i32 {
    let flag = |name: &str| args.iter().position(|a| *a == name).and_then(|i| args.get(i + 1)).copied();
    let (Some(lane), Some(by), Some(report)) = (args.first().copied().filter(|a| !a.starts_with("--")), flag("--by"), flag("--report")) else {
        eprintln!("usage: makepad devhub intake reviewed <lane> --by <reviewer> --report <file>");
        return 2;
    };
    let me = std::env::var("DEVHUB_AGENT").unwrap_or_default();
    if me.eq_ignore_ascii_case(lane) || by.eq_ignore_ascii_case(lane) {
        eprintln!("intake reviewed: a lane does not review itself");
        return 1;
    }
    let digest = match std::fs::read(report) {
        Ok(bytes) if !bytes.is_empty() => crate::intake::sha256_hex(&bytes),
        _ => {
            eprintln!("intake reviewed: {report}: no review report");
            return 1;
        }
    };
    let result = repo_dir().and_then(|d| lane_change(&d, lane).map(|c| (d, c)));
    let (dir, (tip, t)) = match result {
        Ok(x) => x,
        Err(e) => {
            eprintln!("intake reviewed: {e}");
            return 1;
        }
    };
    record("review", lane, &dir, &tip, &t.t2, vec![("by", json::s(by)), ("agent", json::s(me.as_str())), ("report_sha256", json::s(digest.as_str()))])
}

fn record(kind: &str, lane: &str, dir: &Path, tip: &str, paths: &[String], extra: Vec<(&str, Value)>) -> i32 {
    let seen = match blobs(dir, tip, paths) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("intake {kind}: {e}");
            return 1;
        }
    };
    let n = read("records.jsonl").len() + 1;
    let id = format!("{}{n}", if kind == "approve" { "A" } else { "R" });
    let mut fields = vec![
        ("id", json::s(id.as_str())),
        ("kind", json::s(kind)),
        ("lane", json::s(lane)),
        ("tip", json::s(tip)),
        ("ms", Value::Int(now_ms() as i64)),
        ("blobs", Value::Arr(seen.iter().map(|(p, b)| Value::Arr(vec![json::s(p.as_str()), json::s(b.as_str())])).collect())),
    ];
    fields.extend(extra);
    match append("records.jsonl", &json::obj(fields)) {
        Ok(()) => {
            println!("{id}: {kind} of lane {lane} at {tip:.12} recorded ({} file(s))", seen.len());
            0
        }
        Err(e) => {
            eprintln!("intake {kind}: {e}");
            1
        }
    }
}

/// `intake route <item> <lane>`: the lane works on an intake item; marked for good.
pub fn route(args: &[&str]) -> i32 {
    let (Some(&item), Some(&lane)) = (args.first(), args.get(1)) else {
        eprintln!("usage: makepad devhub intake route <relax#N|makepad#N> <lane>");
        return 2;
    };
    let valid_item = item.split_once('#').is_some_and(|(r, n)| matches!(r, "relax" | "makepad") && !n.is_empty() && n.len() <= 9 && n.bytes().all(|b| b.is_ascii_digit()));
    if !valid_item || lane.is_empty() || !lane.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') {
        eprintln!("intake route: an item is relax#N or makepad#N, a lane a plain name");
        return 2;
    }
    match append("lanes.jsonl", &json::obj(vec![("item", json::s(item)), ("lane", json::s(lane)), ("ms", Value::Int(now_ms() as i64))])) {
        Ok(()) => {
            println!("lane {lane} works on {item}: its landings follow the intake rule from now on");
            0
        }
        Err(e) => {
            eprintln!("intake route: {e}");
            1
        }
    }
}

/// `intake replay [--days N] [--branch work]`: the rule over past landings, as if it had been in
/// force: how many approvals it would have asked for, per path and per lane.
pub fn replay(args: &[&str]) -> i32 {
    let flag = |name: &str| args.iter().position(|a| *a == name).and_then(|i| args.get(i + 1)).copied();
    let days: u32 = flag("--days").and_then(|d| d.parse().ok()).unwrap_or(7);
    let branch = flag("--branch").unwrap_or("work");
    let run = || -> Result<String, String> {
        let dir = repo_dir()?;
        let log = git(&dir, &["log", "--first-parent", "--merges", &format!("--since={days} days ago"), "--format=%H%x09%s", branch])?;
        let mut landings = 0;
        let mut hard_lane: BTreeMap<String, u32> = BTreeMap::new();
        let mut per_path: BTreeMap<(Tier, String), u32> = BTreeMap::new();
        let mut hard_landings = 0;
        for line in log.lines() {
            let Some((sha, subject)) = line.split_once('\t') else { continue };
            let Some(lane) = subject.strip_prefix("Merge branch '").and_then(|s| s.split_once('\'')).filter(|(_, rest)| rest.contains("(devhub land)")).map(|(b, _)| b.strip_prefix("rc-").unwrap_or(b).to_string()) else { continue };
            landings += 1;
            let t = touches(&dir, &format!("{sha}^1"), sha)?;
            if !t.hard.is_empty() {
                hard_landings += 1;
                *hard_lane.entry(lane).or_default() += 1;
            }
            for (tier, ps) in [(Tier::Hard, &t.hard), (Tier::Origin, &t.origin), (Tier::T2, &t.t2)] {
                for p in ps {
                    let key = match tier {
                        Tier::T2 => p.split('/').take(3).collect::<Vec<_>>().join("/"),
                        _ => p.clone(),
                    };
                    *per_path.entry((tier, key)).or_default() += 1;
                }
            }
        }
        let mut out = format!("{landings} landings on {branch} in {days} days; {hard_landings} would have asked for a t1-hard approval\n\nt1-hard approvals per lane:\n");
        for (lane, n) in &hard_lane {
            out.push_str(&format!("  {n:4}  {lane}\n"));
        }
        out.push_str("\nlandings per protected path (t2 by directory):\n");
        for ((tier, p), n) in per_path.iter().rev() {
            out.push_str(&format!("  {n:4}  {:9}  {p}\n", tier.word()));
        }
        Ok(out)
    };
    match run() {
        Ok(out) => {
            print!("{out}");
            0
        }
        Err(e) => {
            eprintln!("intake replay: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_and_the_root_manifest_rule() {
        assert!(LIST.lines().filter(|l| !l.trim().is_empty() && !l.starts_with('#')).all(|l| l.split_once(' ').is_some_and(|(t, _)| Tier::parse(t).is_some())), "every list line names a known tier");
        assert_eq!(tier_of("libs/devhub/src/intake/protected.txt"), Some(Tier::Hard), "the list protects itself");
        assert_eq!(tier_of(".github/workflows/ci.yml"), Some(Tier::Hard), "CI");
        assert_eq!(tier_of("libs/devhub/src/land.rs"), Some(Tier::Origin), "the land gate is recorded, not approved");
        assert_eq!(tier_of("libs/relax/compiler/tests/pm_x/build.rs"), None, "a test fixture's build script is not t2");
        assert_eq!(tier_of("tools/run/fixture/build.rs"), None, "nor a fixture's");
        assert_eq!(tier_of("apps/foo/AGENTS.md"), Some(Tier::Origin), "agent files anywhere");
        assert_eq!(tier_of("apps/foo/.claude/settings.json"), Some(Tier::Origin), "nested .claude");
        assert_eq!(tier_of("libs/relax/compiler/src/sandbox/broker.rs"), Some(Tier::Origin), "sandbox dir");
        assert_eq!(tier_of("libs/relax/backend/src/x64.rs"), Some(Tier::T2), "backend");
        assert_eq!(tier_of("libs/foo/build.rs"), Some(Tier::T2), "build scripts anywhere");
        assert_eq!(tier_of("libs/relax/front/src/parser.rs"), None, "the F side is unprotected");
        assert_eq!(tier_of("docs/agentsX.md"), None, "a directory prefix is a whole directory");

        let old = "[workspace]\nmembers = [\n  \"a\",\n]\n\n[workspace.dependencies]\nfoo = \"1\"\n\n[patch.crates-io]\nbar = { path = \"libs/bar\" }\n\n[profile.release]\ndebug = 1\n\n[dependencies]\nlocal = { path = \"libs/local\" }\n";
        let member = "[workspace]\nmembers = [\n  \"a\",\n  \"b\",\n]\n\n[workspace.dependencies]\nfoo = \"1\"\n\n[patch.crates-io]\nbar = { path = \"libs/bar\" }\n\n[profile.release]\ndebug = 1\n\n[dependencies]\nlocal = { path = \"libs/local\" }\n";
        assert_eq!(cargo_tier(old, member, "@@ -3,0 +4 @@\n+  \"b\",\n"), Some(Tier::Origin), "a member is t1-origin");
        let profile = old.replace("debug = 1", "debug = 2");
        assert_eq!(cargo_tier(old, &profile, "@@ -13 +13 @@\n-debug = 1\n+debug = 2\n"), Some(Tier::Origin), "[profile] is t1-origin");
        let wdep = old.replace("foo = \"1\"", "foo = \"2\"");
        assert_eq!(cargo_tier(old, &wdep, "@@ -7 +7 @@\n-foo = \"1\"\n+foo = \"2\"\n"), Some(Tier::Hard), "a crates.io entry in [workspace.dependencies] is t1-hard");
        let wpath = old.replace("foo = \"1\"", "foo = { path = \"libs/foo\" }");
        assert_eq!(cargo_tier(&wpath, &wpath.replace("libs/foo", "libs/foo2"), "@@ -7 +7 @@\n-x\n+x\n"), Some(Tier::Origin), "a path-only entry in [workspace.dependencies] is t1-origin");
        let patch = old.replace("libs/bar", "libs/baz");
        assert_eq!(cargo_tier(old, &patch, "@@ -10 +10 @@\n-x\n+x\n"), Some(Tier::Hard), "[patch] is t1-hard even with a path");
        let pathdep = format!("{old}other = {{ path = \"libs/other\" }}\n");
        assert_eq!(cargo_tier(old, &pathdep, "@@ -16,0 +17 @@\n+x\n"), Some(Tier::Origin), "a path dependency is t1-origin");
        let crate_dep = format!("{old}serde = \"1\"\n");
        assert_eq!(cargo_tier(old, &crate_dep, "@@ -16,0 +17 @@\n+x\n"), Some(Tier::Hard), "a crates.io dependency is t1-hard");
        let git_dep = format!("{old}g = {{ git = \"https://example.invalid/g\", path = \"x\" }}\n");
        assert_eq!(cargo_tier(old, &git_dep, "@@ -16,0 +17 @@\n+x\n"), Some(Tier::Hard), "a git dependency is t1-hard");
        assert_eq!(cargo_tier(old, old, "@@ -x @@\n"), Some(Tier::Hard), "an unreadable hunk fails closed");
        assert_eq!(cargo_tier(old, old, ""), None, "no hunk, no tier");

        let names = vec!["Cargo.toml".to_string(), "AGENTS.md".to_string(), "libs/relax/codegen/src/opt.rs".to_string(), "README.md".to_string()];
        let t = classify(&names, Some((old, member, "@@ -3,0 +4 @@\n+  \"b\",\n")));
        assert_eq!((t.hard.len(), t.origin, t.t2), (0, vec!["Cargo.toml".to_string(), "AGENTS.md".to_string()], vec!["libs/relax/codegen/src/opt.rs".to_string()]), "classify sorts paths into tiers");
        assert_eq!(classify(&["Cargo.toml".to_string()], None).hard, vec!["Cargo.toml".to_string()], "an unread root manifest fails closed");
    }

    #[test]
    fn approvals_cover_exact_blobs_and_the_tripwire_names_changes() {
        let state = |v: &[(&str, &str)]| v.iter().map(|(k, d)| (k.to_string(), d.to_string())).collect::<BTreeMap<_, _>>();
        let before = state(&[("AGENTS.md", "a1"), ("memory/MEMORY.md", "m1"), ("docs/agents/x.md", "d1")]);
        let after = state(&[("AGENTS.md", "a1"), ("memory/MEMORY.md", "m2"), ("memory/new.md", "n1")]);
        assert_eq!(tripwire_diff(&before, &after), ["docs/agents/x.md", "memory/MEMORY.md", "memory/new.md"], "changed, added and gone files are named");
        assert!(tripwire_diff(&after, &after).is_empty(), "an unchanged set says nothing");
        assert_eq!(memory_dir(Path::new("/r/makepad")).map(|p| p.ends_with(".claude/projects/-r-makepad/memory")), Some(true), "the memory folder follows the checkout's path");

        let rec = |kind: &str, lane: &str, blobs: &[(&str, &str)]| {
            json::obj(vec![("kind", json::s(kind)), ("lane", json::s(lane)), ("blobs", Value::Arr(blobs.iter().map(|(p, b)| Value::Arr(vec![json::s(*p), json::s(*b)])).collect()))])
        };
        let records = vec![rec("approve", "x", &[(".github/a", "b1"), ("AGENTS.md", "b2")]), rec("review", "y", &[("libs/relax/backend/a", "b3")])];
        let cur = |v: &[(&str, &str)]| v.iter().map(|(p, b)| (p.to_string(), b.to_string())).collect::<Vec<_>>();
        assert!(covered(&records, "approve", "x", &cur(&[(".github/a", "b1")])), "a subset of the approved blobs is covered");
        assert!(!covered(&records, "approve", "x", &cur(&[(".github/a", "b9")])), "a changed blob needs a new approval");
        assert!(!covered(&records, "approve", "z", &cur(&[(".github/a", "b1")])), "an approval is per lane");
        assert!(!covered(&records, "approve", "y", &cur(&[("libs/relax/backend/a", "b3")])), "a review is not an approval");
        assert!(covered(&records, "review", "y", &cur(&[("libs/relax/backend/a", "b3")])), "the review covers its lane");
        assert!(covered(&records, "approve", "q", &[]), "nothing to approve is covered");
    }
}
