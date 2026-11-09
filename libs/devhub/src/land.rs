//! The landing train (`makepad devhub land`), run by the master. Hand-ins merge into work and
//! commercial main in queue order, then fast-forward the user's checkout. A hand-in's half in
//! another nested repository ([`crate::nested`]) merges into the branch the user's checkout of it
//! is on, and that checkout moves with the rest. A landing merges exactly the commits pinned at
//! its hand-in, every repository's, never a branch's later head. A conflict goes back to the lane,
//! nothing landed, until it hands in again; dirty overlaps wait. Holds and pause are explicit
//! controls.
//! Checks run after landing in timer buckets (see `bucket.rs`); they never block the train.
//! Every transition is a `land` record on the master's feed. The queue and pause live in state.json.

use crate::proto::{hub_record, Record, Sev};
use makepad_strict_json::{obj, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Publish = Arc<dyn Fn(Record) + Send + Sync>;

/// How often the train looks for moved branches without a hand-in.
const IDLE: Duration = Duration::from_secs(15);
/// An entry waiting for the user's checkout retries this often.
const WAITING_RETRY_MS: u64 = 60_000;

/// How a hand-in is asked for.
const ADD_USAGE: &str = "add <branch|-> [<commercial worktree> <commercial branch>] [<nested repository>=<branch>]...";

/// Error lines kept per failed entry.
const MAX_DETAIL: usize = 60;

/// The files the hosted wire's fingerprint hashes ([`makepad_studio_protocol::WIRE`]): an edit of
/// any of them (a comment too) changes the wire, and a WM built before it refuses every app built
/// after it, and the reverse, until both are rebuilt (#2561 closed Source Library on a user's
/// box). A landing that changes one needs the `wire-change` mark.
pub const WIRE_SOURCES: [&str; 10] = [
    "platform/key_code/src/lib.rs",
    "platform/studio/src/lib.rs",
    "platform/studio/src/cursor.rs",
    "platform/studio/src/hub_protocol.rs",
    "platform/studio/src/keyboard.rs",
    "platform/studio/src/mouse.rs",
    "platform/studio/src/relay.rs",
    "platform/studio/src/shared_framebuf.rs",
    "platform/studio/src/studio.rs",
    "platform/studio/src/native_app.rs",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    Queued,
    Failed,
    Conflict,
    Waiting,
    Landed,
}

impl St {
    pub fn as_str(self) -> &'static str {
        match self {
            St::Queued => "queued",
            St::Failed => "failed",
            St::Conflict => "conflict",
            St::Waiting => "waiting",
            St::Landed => "landed",
        }
    }
    fn parse(s: &str) -> St {
        [St::Queued, St::Failed, St::Conflict, St::Waiting, St::Landed].into_iter().find(|x| x.as_str() == s).unwrap_or(St::Queued)
    }
}

/// A hand-in's half in a nested repository other than commercial's ([`Entry::cbranch`]): its
/// branch (in the user's checkout of it, `apps/<name>`, whose worktrees share its branches),
/// merged into the branch that checkout is on.
#[derive(Clone, Debug, PartialEq)]
pub struct Side {
    pub name: String,
    pub branch: String,
    /// the branch's commit pinned at the hand-in: what the landing merges
    pub sha: String,
    /// the tip it landed at
    pub landed: String,
}

impl Side {
    fn to_value(&self) -> Value {
        let s = |x: &str| Value::Str(x.to_string());
        obj(vec![("name", s(&self.name)), ("branch", s(&self.branch)), ("sha", s(&self.sha)), ("landed", s(&self.landed))])
    }
    fn of(v: &Value) -> Option<Side> {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
        Some(Side { name: s("name")?, branch: s("branch")?, sha: s("sha").unwrap_or_default(), landed: s("landed").unwrap_or_default() })
    }
}

/// One hand-in.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: u64,
    /// the makepad branch, or `-` (a commercial-only hand-in)
    pub branch: String,
    /// the commercial worktree and branch ("" when none)
    pub cwt: String,
    pub cbranch: String,
    /// its halves in the other nested repositories
    pub nested: Vec<Side>,
    pub state: St,
    pub held: String,
    /// the branches' commits pinned at the hand-in, what the landing merges (empty: no such
    /// half). A branch's later head never lands with them: #2759's conflict was retried when its
    /// commercial branch moved, and it landed that branch's newer head beside the makepad half it
    /// was handed in with, a pair never built together, and an app broke at startup (11-08). A failure
    /// or conflict is final: the next landing of these branches is a new hand-in, pinned anew.
    pub ms: String,
    pub cs: String,
    /// `work <sha> main <sha>` once landed
    pub landed: String,
    /// `work <sha> main <sha>` just before the fast-forward; empty when unknown
    pub pre: String,
    pub detail: Vec<String>,
    pub agent: String,
    /// Only ci.toml changes.
    pub proof: bool,
    /// It may change the hosted wire ([`WIRE_SOURCES`]): `land --wire-change`.
    pub wire: bool,
    pub asked_ms: u64,
    pub changed_ms: u64,
}

impl Entry {
    pub fn label(&self) -> String {
        let label = match (self.branch.as_str(), self.cbranch.is_empty()) {
            (b, true) => b.to_string(),
            ("-", false) => format!("-+{}", self.cbranch),
            (b, false) => format!("{b}+{}", self.cbranch),
        };
        self.nested.iter().fold(label, |l, n| format!("{l}+{}={}", n.name, n.branch))
    }
    fn names(&self, b: &str) -> bool {
        self.branch == b || self.cbranch == b || self.nested.iter().any(|n| n.branch == b) || self.label() == b
    }
    pub fn to_json(&self) -> String {
        let s = |x: &str| Value::Str(x.to_string());
        let mut out = String::new();
        obj(vec![
            ("id", Value::Int(self.id as i64)),
            ("branch", s(&self.branch)),
            ("cwt", s(&self.cwt)),
            ("cbranch", s(&self.cbranch)),
            ("nested", Value::Arr(self.nested.iter().map(Side::to_value).collect())),
            ("state", s(self.state.as_str())),
            ("held", s(&self.held)),
            ("ms", s(&self.ms)),
            ("cs", s(&self.cs)),
            ("landed", s(&self.landed)),
            ("pre", s(&self.pre)),
            ("detail", Value::Arr(self.detail.iter().map(|d| s(d)).collect())),
            ("agent", s(&self.agent)),
            ("proof", Value::Bool(self.proof)),
            ("wire", Value::Bool(self.wire)),
            ("asked_ms", Value::Int(self.asked_ms as i64)),
            ("changed_ms", Value::Int(self.changed_ms as i64)),
        ])
        .write_into(&mut out);
        out
    }
    pub fn parse(line: &str) -> Option<Entry> {
        let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        Some(Entry {
            id: n("id"),
            branch: s("branch"),
            cwt: s("cwt"),
            cbranch: s("cbranch"),
            nested: v.get("nested").and_then(|d| d.as_arr()).map(|a| a.iter().filter_map(Side::of).collect()).unwrap_or_default(),
            state: St::parse(&s("state")),
            held: s("held"),
            ms: s("ms"),
            cs: s("cs"),
            landed: s("landed"),
            pre: s("pre"),
            detail: v.get("detail").and_then(|d| d.as_arr()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            agent: s("agent"),
            proof: v.get("proof").and_then(Value::as_bool).unwrap_or(false),
            wire: v.get("wire").and_then(Value::as_bool).unwrap_or(false),
            asked_ms: n("asked_ms"),
            changed_ms: n("changed_ms"),
        })
    }
}

/// `git -C <dir> <args>`: (success, stdout), hooks off, never a prompt.
#[track_caller]
fn git(dir: &Path, args: &[&str]) -> (bool, String) {
    let o = crate::git_exec::command()
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match o {
        Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stdout).trim().to_string()),
        Err(_) => (false, String::new()),
    }
}

fn rev(dir: &Path, what: &str) -> Option<String> {
    crate::gitread::rev(dir, what)
}

fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    crate::gitread::is_ancestor(dir, a, b)
}

/// Where a branch left a retired work chain (`refs/heads/work-old-*`, kept when work moves onto
/// main's squashed tip): its merge base with that chain, when `tip` doesn't hold it. Such a branch
/// carries its own changes over as one commit on the tip, so the old history stays out of work.
pub fn retired_base(dir: &Path, branch: &str, tip: &str) -> Option<String> {
    crate::gitread::Pass::default().retired_base(dir, branch, tip)
}

struct Merge {
    base: String,
    sha: String,
    tree: String,
    retired: bool,
    unchanged: bool,
}

fn proof_file(path: &str) -> bool {
    path.rsplit('/').next() == Some("ci.toml")
}

fn prepare_merge(dir: &Path, base: &str, sha: &str) -> Result<Merge, Vec<String>> {
    let unchanged = sha.is_empty() || is_ancestor(dir, sha, base);
    if unchanged {
        return Ok(Merge { base: base.into(), sha: sha.into(), tree: base.into(), retired: false, unchanged });
    }
    let retired = retired_base(dir, sha, base);
    let mut args = vec!["merge-tree", "--write-tree", "--name-only", "--no-messages"];
    if let Some(mb) = &retired {
        args.extend(["--merge-base", mb]);
    }
    args.extend([base, sha]);
    let (ok, out) = git(dir, &args);
    let mut lines = out.lines();
    let tree = lines.next().unwrap_or("").to_string();
    let conflicts: Vec<String> = lines.filter(|l| !l.is_empty()).map(str::to_string).collect();
    if tree.len() != 40 || !ok {
        return Err(if conflicts.is_empty() { vec![format!("merge-tree failed ({base:.10} + {sha:.10})")] } else { conflicts });
    }
    Ok(Merge { base: base.into(), sha: sha.into(), tree, retired: retired.is_some(), unchanged })
}

fn finish_merge(dir: &Path, m: &Merge, msg: &str) -> Result<String, Vec<String>> {
    if m.unchanged { return Ok(m.base.clone()); }
    let mut args = vec!["commit-tree", &m.tree, "-p", &m.base];
    if !m.retired { args.extend(["-p", &m.sha]); }
    args.extend(["-m", msg]);
    let (ok, c) = git(dir, &args);
    if ok && c.len() == 40 { Ok(c) } else { Err(vec!["commit-tree failed".into()]) }
}

/// A hand-in's nested half ([`Side`]) in one attempt: its repository in the user's checkout, the
/// branch that checkout is on and its tip, the half's commit and their merge (empty until made).
struct Half {
    name: String,
    branch: String,
    dir: PathBuf,
    target: String,
    tip: String,
    sha: String,
    merged: String,
}

/// One attempt at an entry: its makepad and commercial shas, their merges onto work's and main's
/// tips, and its nested halves.
struct Attempt {
    ms: String,
    cs: String,
    cm: String,
    cc: String,
    w0: String,
    m0: String,
    halves: Vec<Half>,
}

/// Merges each half onto its target's tip ([`Half::merged`]); Err: the conflicting paths, as
/// tree paths.
fn merge_halves<'a>(halves: impl Iterator<Item = &'a mut Half>) -> Result<(), Vec<String>> {
    for h in halves {
        let tree = |f: Vec<String>| f.into_iter().map(|p| if p.starts_with("merge-tree failed") { format!("{}: {p}", h.name) } else { format!("{}{p}", crate::nested::prefix(&h.name)) }).collect::<Vec<_>>();
        let m = prepare_merge(&h.dir, &h.tip, &h.sha).map_err(tree)?;
        h.merged = finish_merge(&h.dir, &m, &format!("Merge branch '{}' into {} (devhub land)", h.branch, h.target))?;
    }
    Ok(())
}

/// The halves' tips, as a conflict line says them (` / source-library main 0123456789`).
fn tips_said(halves: &[Half]) -> String {
    halves.iter().map(|h| format!(" / {} {} {:.10}", h.name, h.target, h.tip)).collect()
}

/// Merge drivers resolve proof content before merge-tree returns.
fn merge_pair(root: &Path, w: &str, ms: &str, m: &str, cs: &str, branch: &str, cbranch: &str) -> Result<(String, String), Vec<String>> {
    let com = root.join("apps/commercial");
    let work = prepare_merge(root, w, ms)?;
    // commit-tree runs no hook: the private check the hooks run, on what this merge adds
    if !work.unchanged { crate::private_check::merge_gate(root, w, ms, &work.tree)?; }
    let main = prepare_merge(&com, m, cs)?;
    Ok((finish_merge(root, &work, &format!("Merge branch '{branch}' into work (devhub land)"))?, finish_merge(&com, &main, &format!("Merge branch '{cbranch}' into main (devhub land)"))?))
}

/// A standalone merge uses the repository's configured merge drivers.
pub fn merge_onto(dir: &Path, base: &str, sha: &str, msg: &str) -> Result<String, Vec<String>> {
    let merge = prepare_merge(dir, base, sha)?;
    finish_merge(dir, &merge, msg)
}

/// The hosted wire's files ([`WIRE_SOURCES`]) that differ between makepad commits `from` and `to`;
/// Err: the diff did not read.
fn wire_changed(dir: &Path, from: &str, to: &str) -> Result<Vec<String>, String> {
    let mut args = vec!["diff", "--name-only", "--no-renames", from, to, "--"];
    args.extend(WIRE_SOURCES);
    let (ok, files) = git(dir, &args);
    if !ok {
        return Err(format!("the hosted wire's files from {from:.10} to {to:.10} did not read: {files}"));
    }
    Ok(files.lines().map(str::to_string).collect())
}

/// Why a merge of an entry without the `wire-change` mark may not land: the hosted wire's files it
/// changes. None: it changes none, or it is marked.
fn wire_refusal(dir: &Path, from: &str, to: &str, marked: bool) -> Option<Vec<String>> {
    if marked {
        return None;
    }
    match wire_changed(dir, from, to) {
        Ok(files) if files.is_empty() => None,
        Ok(files) => Some(vec![
            format!("it changes the hosted wire: {}", files.join(" ")),
            "an edit of these files (a comment too) changes the wire fingerprint: every WM built before it refuses every app built after it, and the reverse, until both are rebuilt".into(),
            "if that is meant: makepad devhub land --wire-change (each node's WM then restarts on the new wire as its deployment moves); if not, leave these files as work has them".into(),
        ]),
        Err(e) => Some(vec![e]),
    }
}

fn only_proofs(dir: &Path, from: &str, to: &str) -> bool {
    let (ok, files) = git(dir, &["diff", "--name-only", from, to]);
    ok && files.lines().all(proof_file)
}

/// The user's uncommitted paths a fast-forward of `dir` to `target` would touch.
fn dirty_overlap(dir: &Path, target: &str) -> Vec<String> {
    let (_, changed) = git(dir, &["diff", "--name-only", "HEAD", target]);
    let (_, status) = git(dir, &["status", "--porcelain", "--untracked-files=all"]);
    let dirty: Vec<&str> = status.lines().filter_map(|l| l.get(3..)).map(|p| p.rsplit(" -> ").next().unwrap_or(p)).collect();
    changed.lines().filter(|c| dirty.contains(c)).take(10).map(str::to_string).collect()
}

/// The failure lines of a gate log: everything but progress and OK lines (compiler errors, present log tails).
pub fn failure_lines(log: &str) -> Vec<String> {
    let keep = |l: &&str| {
        let t = l.trim();
        !(t.is_empty()
            || l.starts_with("STEP ")
            || l.starts_with("jit: ")
            || l.starts_with("why ")
            || l.starts_with("GATE ")
            || l.starts_with("GATEUNIT ")
            || l.starts_with("GATEPROOF ")
            || l.starts_with("NODEGATE-")
            || (l.starts_with('L') && l.split_whitespace().next().is_some_and(|w| w[1..].bytes().all(|b| b.is_ascii_digit())))
            || t.ends_with(": 0 errors")
            || t.ends_with(" present: 1"))
    };
    let v: Vec<String> = log.lines().filter(keep).map(|l| crate::proto::clean(l, 300)).collect();
    v[v.len().saturating_sub(MAX_DETAIL)..].to_vec()
}

pub struct Train {
    machine: String,
    store: Arc<crate::state::Store>,
    repo: PathBuf,
    /// devhub's own words as a child ([`crate::cli::own_argv`]; a test: its stand-in)
    own: Vec<String>,
    publish: Publish,
    st: Mutex<Vec<Entry>>,
    clock: fn() -> u64,
    /// why the train is paused (empty: it runs)
    paused: Mutex<String>,
    /// a hand-in came: look now
    kicked: AtomicBool,
    /// told each landing's pairs (the checkout's, the landing's) under the fast-forward lock, just
    /// before the checkout moves: the Mac's shows hold what reaches them until the tip is proven
    before_ff: Mutex<Option<BeforeFf>>,
}

/// [`Train::before_ff`]'s hook: (the checkout's pair now, the pair it moves to).
pub type BeforeFf = Arc<dyn Fn(&crate::state::Pair, &crate::state::Pair) + Send + Sync>;

/// The open entries by state, for a glance (the menu bar).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// queued or waiting for the next round
    pub queued: usize,
    /// failed or conflicting, not landed since
    pub failed: usize,
    pub held: usize,
}

impl Train {
    /// The master's one state document (its `land` and `land_paused` sections).
    fn store(&self) -> Arc<crate::state::Store> {
        self.store.clone()
    }
    fn load(&self) {
        let (list, paused) = self.store().read(|d| (d.land.iter().filter_map(|v| Entry::parse(&v.to_json())).collect::<Vec<_>>(), d.land_paused.clone()));
        *self.st.lock().unwrap() = list;
        *self.paused.lock().unwrap() = paused;
    }

    /// Runs `hook` before each fast-forward of the checkout ([`BeforeFf`]).
    pub fn before_ff(&self, hook: BeforeFf) {
        *self.before_ff.lock().unwrap() = Some(hook);
    }

    /// Why the train is paused; None while it runs.
    pub fn paused(&self) -> Option<String> {
        Some(self.paused.lock().unwrap().clone()).filter(|p| !p.is_empty())
    }

    /// Pauses the train (`Some(reason)`) or resumes it (`None`); kept across master restarts.
    pub fn set_paused(&self, reason: Option<&str>) -> Result<(), String> {
        let reason = reason.map(|r| if r.trim().is_empty() { "paused" } else { r.trim() });
        let mut paused = self.paused.lock().unwrap();
        self.store.update(|d| d.land_paused = reason.unwrap_or("").to_string())?;
        let was = std::mem::replace(&mut *paused, reason.unwrap_or("").to_string());
        drop(paused);
        if was.is_empty() != reason.is_none() {
            self.event(Sev::Info, reason.map_or("land resumed".into(), |r| format!("land paused ({r})")), &[]);
        }
        Ok(())
    }

    /// The open entries by state.
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for e in self.st.lock().unwrap().iter().filter(|e| e.state != St::Landed) {
            if !e.held.is_empty() {
                c.held += 1;
            }
            match e.state {
                St::Failed | St::Conflict => c.failed += 1,
                _ if e.held.is_empty() => c.queued += 1,
                _ => {}
            }
        }
        c
    }
    fn save(&self, list: &mut Vec<Entry>) -> Result<(), String> {
        let vals: Vec<makepad_strict_json::Value> = list.iter().filter_map(|e| makepad_strict_json::parse_depth(e.to_json().as_bytes(), 16).ok()).collect();
        let next = list.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        let result = self.store().update(|d| { d.land = vals; d.land_next = d.land_next.max(next); });
        // Retain exactly the durable live view; older completed entries are in history.jsonl.
        *list = self.store.read(|d| d.land.iter().filter_map(|v| Entry::parse(&v.to_json())).collect());
        result
    }
    fn event(&self, sev: Sev, line: String, detail: &[String]) {
        (self.publish)(hub_record(&self.machine, "", 0, "land", sev, line, detail.join("\n")));
    }
    /// Changes one entry (by id), saves, and publishes `line` when given.
    fn set(&self, id: u64, f: impl FnOnce(&mut Entry), ev: Option<(Sev, String)>) {
        let now = (self.clock)();
        let mut list = self.st.lock().unwrap();
        let Some(e) = list.iter_mut().find(|e| e.id == id) else { return };
        f(e);
        e.changed_ms = now;
        let (detail, label) = (e.detail.clone(), e.label());
        // a change that did not save is undone (the list is the store's again): said where the
        // train's events go, never only in the master's log
        if let Err(e) = self.save(&mut list) {
            drop(list);
            eprintln!("devhub land state: {e}");
            self.event(Sev::Err, format!("land {label}: its change did not save, so it stays as it was: {e}"), &[]);
            return;
        }
        drop(list);
        if let Some((sev, line)) = ev {
            self.event(sev, line, if sev == Sev::Info { &[] } else { &detail });
        }
    }

    /// One control request.
    pub fn serve(&self, line: &str, out: &mut Vec<String>) {
        let line = line.strip_prefix("land").map_or(line, str::trim_start);
        let w: Vec<&str> = line.split_whitespace().collect();
        let err = |out: &mut Vec<String>, e: &str| out.push(format!("{{\"err\":{}}}", makepad_remote::json_str(e)));
        match w.as_slice() {
            ["add", rest @ ..] => match self.add(rest) {
                Ok(l) => out.push(l),
                Err(e) => err(out, &e),
            },
            ["status", rest @ ..] => {
                let list = self.st.lock().unwrap().clone();
                let now = (self.clock)();
                for e in list.iter().filter(|e| match rest.first() {
                    Some(&"all") => true,
                    Some(b) => e.names(b),
                    // what is open, and what landed in the last hour
                    None => e.state != St::Landed || now.saturating_sub(e.changed_ms) < 3_600_000,
                }) {
                    out.push(e.to_json());
                }
            }
            [verb @ ("hold" | "unhold" | "retry" | "drop"), b, reason @ ..] => {
                let mut list = self.st.lock().unwrap();
                // `#<id>` names one entry; a branch or label names its newest open entry
                let by_id = b.strip_prefix('#').and_then(|n| n.parse::<u64>().ok());
                let Some(i) = list.iter().rposition(|e| e.state != St::Landed && by_id.map_or_else(|| e.names(b), |id| e.id == id)) else {
                    drop(list);
                    return err(out, &format!("no open entry {b}"));
                };
                match *verb {
                    "hold" => list[i].held = if reason.is_empty() { "held".into() } else { reason.join(" ") },
                    "unhold" => list[i].held.clear(),
                    // the hand-in again as it was handed in, its pinned commits; newer commits
                    // of its branches are a new hand-in
                    "retry" => list[i].state = St::Queued,
                    _ => {
                        list.remove(i);
                    }
                }
                if let Err(e) = self.save(&mut list) { out.push(format!("{{\"err\":{}}}", makepad_remote::json_str(&e))); return; }
                drop(list);
                self.event(Sev::Info, format!("land {verb} {b}"), &[]);
                out.push(format!("{{\"ok\":1,\"{verb}\":{}}}", makepad_remote::json_str(b)));
            }
            ["pause", reason @ ..] => {
                if let Err(e) = self.set_paused(Some(&reason.join(" "))) { err(out, &e); return; }
                out.push(format!("{{\"paused\":{}}}", makepad_remote::json_str(&self.paused().unwrap_or_default())));
            }
            ["resume"] => {
                if let Err(e) = self.set_paused(None) { err(out, &e); return; }
                out.push("{\"paused\":\"\"}".into());
            }
            ["paused"] => out.push(format!("{{\"paused\":{}}}", makepad_remote::json_str(&self.paused().unwrap_or_default()))),
            ["import", landq, failed, held] => match self.import(Path::new(landq), Path::new(failed), Path::new(held)) {
                Ok(n) => out.push(format!("{{\"ok\":1,\"imported\":{n}}}")),
                Err(e) => err(out, &e),
            },
            _ => err(out, &format!("land {ADD_USAGE} | status [all|<branch>] | hold <b> [reason] | unhold <b> | retry <b> | drop <b> | import <landq> <failed> <held> | pause [reason] | resume | paused")),
        }
    }

    /// A hand-in's halves in the other nested repositories (`<name>=<branch>`): each branch in
    /// the user's checkout of it, which is on a branch; one with nothing that branch lacks lands
    /// nothing and is left out.
    fn sides(&self, words: &[&str]) -> Result<Vec<Side>, String> {
        let mut out: Vec<Side> = Vec::new();
        for (name, branch) in words.iter().filter_map(|x| x.split_once('=')) {
            if name == crate::state::PAIRED {
                return Err(format!("{name}'s half is `<commercial worktree> <commercial branch>`: {ADD_USAGE}"));
            }
            if out.iter().any(|s| s.name == name) {
                return Err(format!("{name} twice"));
            }
            let dir = self.repo.join(crate::nested::dir(name));
            let sha = rev(&dir, &format!("refs/heads/{branch}")).ok_or_else(|| format!("no {name} branch {branch} in {} (a lane's {name} is a worktree of it)", dir.display()))?;
            let target = git(&dir, &["branch", "--show-current"]).1;
            if target.is_empty() {
                return Err(format!("{} is on no branch: nothing to land {name} into", dir.display()));
            }
            let tip = rev(&dir, &format!("refs/heads/{target}")).unwrap_or_default();
            if branch != target && !is_ancestor(&dir, &sha, &tip) {
                out.push(Side { name: name.to_string(), branch: branch.to_string(), sha, landed: String::new() });
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    fn add(&self, w: &[&str]) -> Result<String, String> {
        let proof = w.contains(&"proof");
        let wire = w.contains(&"wire-change");
        let agent = w.iter().find_map(|x| x.strip_prefix("agent=")).unwrap_or("").to_string();
        let w: Vec<&str> = w.iter().copied().filter(|x| !x.starts_with("agent=") && *x != "proof" && *x != "wire-change").collect();
        // `<name>=<branch>`: a half in another nested repository (a path has a `/` before any `=`)
        let (sides, w): (Vec<&str>, Vec<&str>) = w.into_iter().partition(|x| x.split_once('=').is_some_and(|(n, b)| crate::nested::valid(n) && !b.is_empty()));
        let (mut branch, mut cwt, mut cb) = match w.as_slice() {
            [b] => (*b, "", ""),
            [b, wt, cb] => (*b, *wt, *cb),
            _ => return Err(ADD_USAGE.into()),
        };
        let nested = self.sides(&sides)?;
        // resolve the lane pair before checking branches or looking up the queue entry
        let lanes = self.store().read(|d| d.lanes.clone());
        let lane = match w.as_slice() {
            [b] if *b != "-" => lanes.iter().rev().find(|l| l.branch == *b || l.name == *b),
            ["-", _, cb] => lanes.iter().rev().find(|l| l.cbranch == *cb),
            _ => None,
        };
        let lane_cwt;
        let mut included = "";
        if let Some(lane) = lane {
            let commercial = self.repo.join("apps/commercial");
            let wt = Path::new(&lane.path).join("apps/commercial");
            lane_cwt = if wt.is_dir() { wt } else { commercial.clone() }.to_string_lossy().into_owned();
            let makepad_only = branch != "-";
            if makepad_only { branch = &lane.branch; }
            let (repo, target, other, repo_name) = if makepad_only {
                (&commercial, "main", lane.cbranch.as_str(), "commercial")
            } else {
                (&self.repo, "work", lane.branch.as_str(), "makepad")
            };
            // (a half whose branch is gone has nothing left to land)
            if !other.is_empty() && other != "-" && repo.join(".git").exists() && rev(repo, &format!("refs/heads/{other}")).is_some() {
                let pair_cwt = if makepad_only { lane_cwt.as_str() } else { cwt };
                let pair_cb = if makepad_only { lane.cbranch.as_str() } else { cb };
                let refuse = || format!("lane {} has commits on {other} in {repo_name} too; land both: makepad devhub land {} {pair_cwt} {pair_cb}", lane.name, lane.branch);
                let (ok, count) = git(repo, &["rev-list", "--count", &format!("refs/heads/{target}..refs/heads/{other}")]);
                let count = count.parse::<u64>().ok().filter(|_| ok).ok_or_else(&refuse)?;
                if count > 0 {
                    if !Path::new(pair_cwt).join(".git").exists() || rev(Path::new(pair_cwt), &format!("refs/heads/{pair_cb}")).is_none() {
                        return Err(refuse());
                    }
                    if makepad_only {
                        cwt = &lane_cwt;
                        cb = &lane.cbranch;
                    } else {
                        branch = &lane.branch;
                    }
                    included = other;
                }
            }
        }
        // the commits this hand-in lands, pinned now ([`Entry::ms`])
        let ms = if branch == "-" { String::new() } else { rev(&self.repo, &format!("refs/heads/{branch}")).ok_or_else(|| format!("no makepad branch {branch}"))? };
        let cs = if cb.is_empty() { String::new() } else { rev(Path::new(cwt), &format!("refs/heads/{cb}")).ok_or_else(|| format!("no commercial branch {cb} in {cwt}"))? };
        if branch == "-" && cb.is_empty() && nested.is_empty() {
            return Err("nothing to land".into());
        }
        let now = (self.clock)();
        let mut list = self.st.lock().unwrap();
        // (a hand-in of nested halves alone is the same entry for the same branches)
        let same = |e: &Entry| e.branch == branch && e.cbranch == cb && (branch != "-" || !cb.is_empty() || e.nested.iter().map(|n| (&n.name, &n.branch)).eq(nested.iter().map(|n| (&n.name, &n.branch))));
        let open = list.iter().position(|e| e.state != St::Landed && same(e));
        let id = match open {
            // a hand-in again: the same entry, queued again (its place in the queue kept)
            Some(i) => {
                let e = &mut list[i];
                e.cwt = cwt.to_string();
                (e.ms, e.cs) = (ms, cs);
                e.nested = nested;
                e.proof = proof;
                e.wire = wire;
                e.state = St::Queued;
                e.detail.clear();
                e.changed_ms = now;
                e.id
            }
            None => {
                // (a counter in the state document: numbers of trimmed or lost entries are not reused)
                let id = self.store().read(|d| d.land_next).max(list.iter().map(|e| e.id).max().unwrap_or(0) + 1);
                list.push(Entry { id, branch: branch.into(), cwt: cwt.into(), cbranch: cb.into(), nested, state: St::Queued, held: String::new(), ms, cs, landed: String::new(), pre: String::new(), detail: Vec::new(), agent, proof, wire, asked_ms: now, changed_ms: now });
                id
            }
        };
        let pos = list.iter().filter(|e| e.state != St::Landed && e.id <= id).count();
        self.save(&mut list)?;
        let label = list.iter().find(|e| e.id == id).map(Entry::label).unwrap_or_default();
        drop(list);
        self.event(Sev::Info, format!("land queued {label} (#{id}, {pos} open)"), &[]);
        self.kicked.store(true, Ordering::Release);
        let included = if included.is_empty() { String::new() } else { format!(",\"included\":{}", makepad_remote::json_str(included)) };
        Ok(format!("{{\"ok\":1,\"id\":{id},\"pos\":{pos},\"label\":{}{included}}}", makepad_remote::json_str(&label)))
    }

    /// One-time import of the old train's files: landq lines, failed shas (they stay failed until their branch moves),
    /// HELD lines (`<branch> HELD: <reason>`).
    fn import(&self, landq: &Path, failed: &Path, held: &Path) -> Result<usize, String> {
        let q = std::fs::read_to_string(landq).map_err(|e| format!("{}: {e}", landq.display()))?;
        let failed = std::fs::read_to_string(failed).unwrap_or_default();
        let held = std::fs::read_to_string(held).unwrap_or_default();
        let mut n = 0;
        for l in q.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let w: Vec<&str> = l.split_whitespace().collect();
            if self.add(&w).is_err() {
                continue;
            }
            n += 1;
            let (b, cb) = (w[0], w.get(2).copied().unwrap_or(""));
            let ms = if b == "-" { None } else { rev(&self.repo, b) };
            let cs = w.get(1).and_then(|wt| rev(Path::new(wt), cb));
            let bad = |sha: &Option<String>| sha.as_deref().and_then(|s| failed.lines().find(|f| f.starts_with(s)).map(str::to_string));
            let mut list = self.st.lock().unwrap();
            if let Some(e) = list.iter_mut().rev().find(|e| e.branch == b && e.cbranch == cb && e.state != St::Landed) {
                if let Some(f) = bad(&ms).or_else(|| bad(&cs)) {
                    e.state = if f.contains(" conflict ") { St::Conflict } else { St::Failed };
                    e.ms = ms.clone().unwrap_or_default();
                    e.cs = cs.clone().unwrap_or_default();
                    e.detail = vec![format!("imported from landbatch.failed: {f}")];
                }
                if let Some(h) = held.lines().find(|h| h.split_whitespace().next() == Some(b)) {
                    e.held = h.split_once("HELD:").map_or("held", |x| x.1.trim()).to_string();
                }
            }
            self.save(&mut list)?;
        }
        Ok(n)
    }

    /// Lands ready entries one at a time, oldest first.
    fn start_ready(&self) -> bool {
        let r = &self.repo;
        let c = r.join("apps/commercial");
        let mut list: Vec<Entry> = self.st.lock().unwrap().iter().filter(|e| e.state != St::Landed).cloned().collect();
        list.sort_by_key(|e| (e.asked_ms, e.id));
        let mut landed = false;
        for queued in &list {
            if self.paused().is_some() { break; }
            let Some(e) = self.st.lock().unwrap().iter().find(|e| e.id == queued.id).cloned() else { continue };
            // a failure or a conflict is final: nothing landed, and only a new hand-in lands these
            // branches ([`Entry::ms`])
            if matches!(e.state, St::Landed | St::Failed | St::Conflict) || !e.held.is_empty() { continue; }
            if e.state == St::Waiting && (self.clock)().saturating_sub(e.changed_ms) < WAITING_RETRY_MS { continue; }
            // a tip that does not read stops the round, said: a round that stopped unsaid left a
            // hand-in queued with nothing to tell why (a test's second entry, 11-08)
            let (Some(w0), Some(m0)) = (rev(r, "refs/heads/work"), rev(&c, "refs/heads/main")) else {
                let unread: Vec<String> = [(r.as_path(), "work"), (c.as_path(), "main")].into_iter().filter(|(dir, b)| rev(dir, &format!("refs/heads/{b}")).is_none()).map(|(dir, b)| format!("{b} in {}", dir.display())).collect();
                self.event(Sev::Warn, format!("land: {} did not read: the round stops before {}", if unread.is_empty() { "work / main".to_string() } else { unread.join(", ") }, e.label()), &[]);
                break;
            };
            // the pinned commits, whatever the branches hold now; one no longer in its repository
            // (its branch deleted and pruned) lands nothing
            let pinned = |dir: &Path, sha: &str, none: bool| if none { Some(String::new()) } else { (!sha.is_empty() && rev(dir, sha).as_deref() == Some(sha)).then(|| sha.to_string()) };
            let ms = pinned(r, &e.ms, e.branch == "-");
            let cs = pinned(&c, &e.cs, e.cbranch.is_empty());
            // the nested halves, each onto the branch the user's checkout of it is on
            let mut halves: Vec<Half> = Vec::new();
            let mut unready = None;
            for n in &e.nested {
                let dir = r.join(crate::nested::dir(&n.name));
                let target = git(&dir, &["branch", "--show-current"]).1;
                let Some(tip) = rev(&dir, &format!("refs/heads/{target}")).filter(|_| !target.is_empty()) else {
                    unready = Some(format!("the user's checkout {} is on no branch", dir.display()));
                    break;
                };
                let sha = pinned(&dir, &n.sha, false).unwrap_or_default();
                halves.push(Half { name: n.name.clone(), branch: n.branch.clone(), dir, target, tip, sha, merged: String::new() });
            }
            if let Some(why) = unready {
                self.set(e.id, |e| {
                    e.state = St::Waiting;
                    e.detail = vec![why.clone()];
                }, Some((Sev::Info, format!("land waiting {}: {why}", e.label()))));
                continue;
            }
            let (Some(ms), Some(cs), true) = (ms, cs, halves.iter().all(|h| !h.sha.is_empty())) else {
                self.set(e.id, |e| {
                    e.state = St::Failed;
                    e.detail = vec!["a commit pinned at its hand-in is not in its repository (or it was handed in unpinned): nothing landed; hand in again".into()];
                }, Some((Sev::Warn, format!("land failed {}: a pinned commit is gone", e.label()))));
                continue;
            };
            if (ms.is_empty() || is_ancestor(r, &ms, &w0)) && (cs.is_empty() || is_ancestor(&c, &cs, &m0)) && halves.iter().all(|h| is_ancestor(&h.dir, &h.sha, &h.tip)) {
                let why = "already in work/main";
                self.set(e.id, |e| {
                    e.state = St::Landed;
                    e.landed = format!("work {w0} main {m0}");
                    for (n, h) in e.nested.iter_mut().zip(&halves) {
                        n.landed = h.tip.clone();
                    }
                    // it moved nothing: what was just before is what it landed, so no bucket names
                    // it for what a range broke (a re-land of a landed branch was named for a
                    // warning in a file it never touched, 11-06)
                    e.pre = e.landed.clone();
                    e.detail.clear();
                }, Some((Sev::Info, format!("land landed {} ({why})", e.label()))));
                continue;
            }
            let label = e.label();
            let (cm, cc) = match merge_pair(r, &w0, &ms, &m0, &cs, &e.branch, &e.cbranch).and_then(|pair| merge_halves(halves.iter_mut()).map(|_| pair)) {
                Ok(pair) => pair,
                Err(f) => {
                    self.set(e.id, |e| {
                        e.state = St::Conflict;
                        e.detail = f;
                    }, Some((Sev::Warn, format!("land conflict {label} with work {w0:.10} / main {m0:.10}{}", tips_said(&halves)))));
                    continue;
                }
            };
            if e.proof && (!only_proofs(r, &w0, &cm) || !only_proofs(&c, &m0, &cc) || halves.iter().any(|h| !only_proofs(&h.dir, &h.tip, &h.merged))) {
                self.set(e.id, |e| {
                    e.state = St::Failed;
                    e.detail = vec!["proof entry touches files other than ci.toml".into()];
                }, Some((Sev::Err, format!("land refused proof entry {label}: non-proof files"))));
                continue;
            }
            if let Some(why) = wire_refusal(r, &w0, &cm, e.wire) {
                self.set(e.id, |e| {
                    e.state = St::Failed;
                    e.detail = why.clone();
                }, Some((Sev::Err, format!("land refused {label}: {}", why[0]))));
                continue;
            }
            landed |= self.land_one(e.id, Attempt { ms, cs, cm, cc, w0, m0, halves });
        }
        if landed { self.sync_now(); }
        landed
    }

    /// LIVEDEPLOY: the WM nodes get the new tips now.
    fn sync_now(&self) {
        // reaped on a thread of its own: a dropped Child stays a zombie of the master
        if let Ok(mut child) = Command::new(&self.own[0]).args(&self.own[1..]).args(["sync", "now"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            let _ = std::thread::Builder::new().name("devhub-sync-now".into()).spawn(move || { let _ = child.wait(); });
        }
    }

    /// Fast-forwards work and main and the nested halves' branches, re-merging if the tips moved;
    /// true when it landed.
    fn land_one(&self, id: u64, a: Attempt) -> bool {
        let Attempt { ms, cs, cm, cc, w0, m0, mut halves } = a;
        let (ms, cs) = (ms.as_str(), cs.as_str());
        let r = &self.repo;
        let c = r.join("apps/commercial");
        let lock = r.join(".git/land-ff.lock");
        while std::fs::create_dir(&lock).is_err() {
            std::thread::sleep(Duration::from_millis(100));
        }
        let unlock = || {
            let _ = std::fs::remove_dir(&lock);
        };
        let label = self.st.lock().unwrap().iter().find(|e| e.id == id).map(Entry::label).unwrap_or_default();
        let (branch, cbranch) = self.st.lock().unwrap().iter().find(|e| e.id == id).map(|e| (e.branch.clone(), e.cbranch.clone())).unwrap_or_default();
        let wait = |why: String| {
            unlock();
            self.set(id, |e| {
                e.state = St::Waiting;
                e.detail = vec![why.clone()];
            }, Some((Sev::Info, format!("land waiting {label}: {why}"))));
            false
        };
        // Recheck controls after acquiring the fast-forward lock.
        let held = self.st.lock().unwrap().iter().find(|e| e.id == id).map(|e| e.held.clone()).unwrap_or_default();
        if !held.is_empty() {
            unlock();
            self.set(id, |e| e.state = St::Queued, Some((Sev::Info, format!("land {label} is held ({held})"))));
            return false;
        }
        if self.paused().is_some() || !self.st.lock().unwrap().iter().any(|e| e.id == id) {
            unlock();
            return false;
        }
        let on = |dir: &Path, want: &str| git(dir, &["branch", "--show-current"]).1 == want;
        if !on(r, "work") || !on(&c, "main") {
            return wait("the user checkout is not on work / main".into());
        }
        if let Some(h) = halves.iter().find(|h| !on(&h.dir, &h.target)) {
            return wait(format!("the user's checkout {} is not on {}", h.dir.display(), h.target));
        }
        let (Some(w), Some(m)) = (rev(r, "refs/heads/work"), rev(&c, "refs/heads/main")) else { return wait("no work/main".into()) };
        // a half whose branch moved since its merge merges again
        for h in halves.iter_mut() {
            let Some(tip) = rev(&h.dir, &format!("refs/heads/{}", h.target)) else { return wait(format!("no {} in {}", h.target, h.dir.display())) };
            if tip != h.tip {
                (h.tip, h.merged) = (tip, String::new());
            }
        }
        let merged = if w == w0 && m == m0 { Ok((cm.to_string(), cc.to_string())) } else { merge_pair(r, &w, ms, &m, cs, &branch, &cbranch) };
        let (tm, tc) = match merged.and_then(|pair| merge_halves(halves.iter_mut().filter(|h| h.merged.is_empty())).map(|_| pair)) {
            Ok(pair) => pair,
            Err(f) => {
                unlock();
                self.set(id, |e| {
                    e.state = St::Conflict;
                    e.detail = f;
                }, Some((Sev::Warn, format!("land conflict {label} with work {w:.10} / main {m:.10}{}", tips_said(&halves)))));
                return false;
            }
        };
        let proof = self.st.lock().unwrap().iter().find(|e| e.id == id).is_some_and(|e| e.proof);
        if proof && (!only_proofs(r, &w, &tm) || !only_proofs(&c, &m, &tc) || halves.iter().any(|h| !only_proofs(&h.dir, &h.tip, &h.merged))) {
            return wait("proof entry's new merge touches files other than ci.toml".into());
        }
        let wire = self.st.lock().unwrap().iter().find(|e| e.id == id).is_some_and(|e| e.wire);
        if let Some(why) = wire_refusal(r, &w, &tm, wire) {
            unlock();
            self.set(id, |e| {
                e.state = St::Failed;
                e.detail = why.clone();
            }, Some((Sev::Err, format!("land refused {label}: {}", why[0]))));
            return false;
        }
        // a marked change of the wire is said where everyone reads: the rolls restart the WMs on it
        let wire_note = if wire { wire_changed(r, &w, &tm).ok().filter(|f| !f.is_empty()).map_or(String::new(), |f| format!("; it changes the hosted wire ({})", f.join(" "))) } else { String::new() };
        let mut over = dirty_overlap(r, &tm);
        over.extend(dirty_overlap(&c, &tc));
        for h in &halves {
            over.extend(dirty_overlap(&h.dir, &h.merged).into_iter().map(|p| format!("{}{p}", crate::nested::prefix(&h.name))));
        }
        if !over.is_empty() {
            return wait(format!("the user's uncommitted files the ff would touch: {}", over.join(" ")));
        }
        if tm != w || tc != m {
            let hook = self.before_ff.lock().unwrap().clone();
            if let Some(hook) = hook {
                hook(&crate::state::Pair::new(&w, &m), &crate::state::Pair::new(&tm, &tc));
            }
        }
        if tm != w && !git(r, &["merge", "-q", "--ff-only", &tm]).0 {
            return wait("ff of work failed (nothing landed)".into());
        }
        if tc != m && !git(&c, &["merge", "-q", "--ff-only", &tc]).0 {
            std::thread::sleep(Duration::from_secs(5));
            if !git(&c, &["merge", "-q", "--ff-only", &tc]).0 {
                unlock();
                let why = format!("HALF-LANDED: work is at {tm:.10} but commercial main did not ff to {tc:.10} (git -C {} merge --ff-only {tc})", c.display());
                self.set(id, |e| {
                    e.state = St::Failed;
                    e.detail = vec![why.clone()];
                }, Some((Sev::Err, format!("land {label}: {why}"))));
                return false;
            }
        }
        for h in halves.iter().filter(|h| h.merged != h.tip) {
            if !git(&h.dir, &["merge", "-q", "--ff-only", &h.merged]).0 {
                std::thread::sleep(Duration::from_secs(5));
                if !git(&h.dir, &["merge", "-q", "--ff-only", &h.merged]).0 {
                    unlock();
                    let why = format!("HALF-LANDED: work is at {tm:.10}, main at {tc:.10}, but {} {} did not ff to {:.10} (git -C {} merge --ff-only {})", h.name, h.target, h.merged, h.dir.display(), h.merged);
                    self.set(id, |e| {
                        e.state = St::Failed;
                        e.detail = vec![why.clone()];
                    }, Some((Sev::Err, format!("land {label}: {why}"))));
                    return false;
                }
            }
        }
        let nested: String = halves.iter().map(|h| format!(" {} {:.10}", h.name, h.merged)).collect();
        self.set(id, |e| {
            e.state = St::Landed;
            e.pre = format!("work {w} main {m}");
            e.landed = format!("work {tm} main {tc}");
            for (n, h) in e.nested.iter_mut().zip(&halves) {
                n.landed = h.merged.clone();
            }
            e.detail.clear();
        }, Some((if wire_note.is_empty() { Sev::Info } else { Sev::Warn }, format!("land landed {label}: work {tm:.10} main {tc:.10}{nested}{wire_note}"))));
        unlock();
        if proof {
            for (repo, branch, pin, tip) in [(r, &branch, ms, &tm), (&c, &cbranch, cs, &tc)] {
                if branch.starts_with("ci-proof-") && rev(repo, branch).as_deref() == Some(pin) && is_ancestor(repo, pin, tip) {
                    if !git(repo, &["branch", "-d", branch]).0 {
                        self.event(Sev::Warn, format!("land: could not remove merged proof branch {branch}"), &[]);
                    }
                }
            }
        }
        true
    }

    fn run(self: Arc<Self>) {
        // A restarted master retries waiting entries and releases its old fast-forward lock.
        let _ = std::fs::remove_dir(self.repo.join(".git/land-ff.lock"));
        {
            let mut list = self.st.lock().unwrap();
            for e in list.iter_mut().filter(|e| e.state == St::Waiting) {
                e.state = St::Queued;
            }
            if let Err(e) = self.save(&mut list) { eprintln!("devhub land state: {e}"); }
        }
        let mut looked = std::time::Instant::now() - IDLE;
        loop {
            if self.kicked.swap(false, Ordering::AcqRel) || looked.elapsed() >= IDLE {
                looked = std::time::Instant::now();
                if self.paused().is_none() {
                    self.start_ready();
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// The master's train: loads the queue, starts the round thread, registers the `land` control service.
pub fn register(ep: &makepad_remote::Endpoint, machine: &str, publish: Publish) -> Arc<Train> {
    let home = crate::proto::home();
    let t = Arc::new(Train {
        machine: machine.to_string(),
        store: crate::state::Store::open_quiet(&home, machine).expect("workflow state must open before services start"),
        repo: crate::lease::wt::default_repo(),
        own: crate::cli::own_argv(&crate::lease::wt::default_repo()),
        publish,
        st: Mutex::new(Vec::new()),
        clock: crate::proto::wall_ms,
        paused: Mutex::new(String::new()),
        kicked: AtomicBool::new(false),
        before_ff: Mutex::new(None),
    });
    t.load();
    let t2 = t.clone();
    ep.register("land", 1, makepad_remote::lines::service("land", Arc::new(move |line, out| t2.serve(line, out))));
    let t3 = t.clone();
    let _ = std::thread::Builder::new().name("devhub-land".into()).spawn(move || t3.run());
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_off_a_retired_chain_lands_without_its_history() {
        let d = std::env::temp_dir().join(format!("devhub-retired-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let g = |args: &[&str]| {
            let (ok, out) = git(&d, args);
            assert!(ok, "{args:?}: {out}");
            out
        };
        g(&["init", "-q", "-b", "work"]);
        g(&["config", "user.email", "t@example.com"]);
        g(&["config", "user.name", "t"]);
        std::fs::write(d.join("a"), "a\n").unwrap();
        g(&["add", "a"]);
        g(&["commit", "-qm", "old chain"]);
        let old = g(&["rev-parse", "HEAD"]);
        g(&["checkout", "-qb", "lane"]);
        std::fs::write(d.join("b"), "b\n").unwrap();
        g(&["add", "b"]);
        g(&["commit", "-qm", "lane"]);
        let lane = g(&["rev-parse", "HEAD"]);
        let tree = g(&["rev-parse", &format!("{old}^{{tree}}")]);
        let squashed = g(&["commit-tree", &tree, "-m", "squashed"]);
        g(&["update-ref", "refs/heads/work-old-1", &old]);
        let landed = merge_onto(&d, &squashed, &lane, "land").unwrap();
        assert_eq!(g(&["rev-parse", &format!("{landed}^")]), squashed);
        assert!(!is_ancestor(&d, &old, &landed));
        assert!(g(&["ls-tree", "--name-only", &landed]).contains('b'));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn entries_round_trip_and_label() {
        let e = Entry { id: 3, branch: "rc-x".into(), cwt: "/w/x/apps/commercial".into(), cbranch: "rc-x".into(), nested: vec![Side { name: "extra".into(), branch: "rc-x".into(), sha: "c".repeat(40), landed: String::new() }], state: St::Failed, held: String::new(), ms: "a".repeat(40), cs: String::new(), landed: String::new(), pre: format!("work {} main {}", "a".repeat(40), "b".repeat(40)), detail: vec!["stage: 1 errors".into(), "src/a.rs:3:4: \"quoted\"".into()], agent: "x".into(), proof: true, wire: true, asked_ms: 1, changed_ms: 2 };
        assert_eq!(Entry::parse(&e.to_json()).unwrap(), e);
        assert_eq!(e.label(), "rc-x+rc-x+extra=rc-x");
        assert!(e.names("rc-x") && !e.names("rc-y") && e.names("rc-x+rc-x+extra=rc-x"));
        // an entry from before nested halves reads as one with none
        let old = e.to_json().replace(r#""nested":[{"name":"extra","branch":"rc-x","sha":"cccccccccccccccccccccccccccccccccccccccc","landed":""}],"#, "");
        assert_eq!(Entry::parse(&old).unwrap().nested, Vec::new());
    }

    #[test]
    fn failure_lines_keep_errors_and_drop_progress() {
        let log = "L7095 gate@node-a: granted, setting up\nSTEP tree 0 s [1-2]\nmakepad-bar: 0 errors\nmakepad-foo: 2 errors\napps/commercial/foo/src/a.rs:3:4: cannot find value `x`\nmakepad-app-calculator present: 1\nmakepad-baz present: 0\nmakepad-baz: no present within 30 s; its last log lines:\n  panicked at x\nNODEGATE-FAIL: makepad-foo\nL7095 gate@node-a (lane landq1) released; node-a cleaned up";
        assert_eq!(failure_lines(log), vec!["makepad-foo: 2 errors", "apps/commercial/foo/src/a.rs:3:4: cannot find value `x`", "makepad-baz present: 0", "makepad-baz: no present within 30 s; its last log lines:", "  panicked at x"]);
    }

    /// merge_onto: a clean merge is a two-parent commit on the base; a conflict names its files.
    #[test]
    fn merge_onto_merges_or_names_conflicts() {
        let d = std::env::temp_dir().join(format!("devhub-land-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let g = |args: &[&str]| assert!(git(&d, args).0, "git {args:?}");
        g(&["init", "-q", "-b", "work"]);
        g(&["config", "user.email", "t@t"]);
        g(&["config", "user.name", "t"]);
        std::fs::write(d.join("a"), "1\n").unwrap();
        g(&["add", "a"]);
        g(&["commit", "-qm", "base"]);
        g(&["checkout", "-qb", "x"]);
        std::fs::write(d.join("b"), "x\n").unwrap();
        g(&["add", "b"]);
        g(&["commit", "-qm", "x"]);
        g(&["checkout", "-qb", "y", "work"]);
        std::fs::write(d.join("a"), "y\n").unwrap();
        g(&["commit", "-qam", "y"]);
        g(&["checkout", "-q", "work"]);
        std::fs::write(d.join("a"), "w\n").unwrap();
        g(&["commit", "-qam", "w"]);
        let w = rev(&d, "work").unwrap();
        let m = merge_onto(&d, &w, &rev(&d, "x").unwrap(), "m").unwrap();
        assert_eq!(git(&d, &["rev-parse", &format!("{m}^1")]).1, w);
        assert!(is_ancestor(&d, &rev(&d, "x").unwrap(), &m));
        assert_eq!(merge_onto(&d, &w, &rev(&d, "y").unwrap(), "m"), Err(vec!["a".to_string()]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    fn train(home: &Path, repo: &Path, exe: &Path, events: Arc<Mutex<Vec<String>>>) -> Arc<Train> {
        Arc::new(Train {
            machine: "mac".into(),
            store: crate::state::Store::open_quiet(home, "").unwrap(),
            repo: repo.to_path_buf(),
            own: vec![exe.display().to_string()],
            publish: Arc::new(move |r: Record| events.lock().unwrap().push(r.line.clone())),
            st: Mutex::new(Vec::new()),
            clock: crate::proto::wall_ms,
            paused: Mutex::new(String::new()),
            kicked: AtomicBool::new(false),
            before_ff: Mutex::new(None),
        })
    }

    /// A hand-in's half in another nested repository merges into the branch the user's checkout
    /// of it is on, and that checkout moves with work; one alone lands too, a conflict there is
    /// the entry's, and a half with nothing new is left out.
    #[cfg(unix)]
    #[test]
    fn a_nested_repositorys_half_lands_into_the_branch_its_checkout_is_on() {
        let d = std::env::temp_dir().join(format!("devhub-land-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (r, home) = (d.join("repo"), d.join("home"));
        let (c, x) = (r.join("apps/commercial"), r.join("apps/extra"));
        std::fs::create_dir_all(&c).unwrap();
        std::fs::create_dir_all(&x).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let g = |dir: &Path, args: &[&str]| assert!(git(dir, args).0, "git {args:?} in {}", dir.display());
        for (dir, main) in [(&r, "work"), (&c, "main"), (&x, "trunk")] {
            g(dir, &["init", "-q", "-b", main]);
            g(dir, &["config", "user.email", "t@t"]);
            g(dir, &["config", "user.name", "t"]);
        }
        std::fs::write(r.join(".gitignore"), "apps/commercial\napps/extra\n").unwrap();
        for dir in [&r, &c, &x] {
            std::fs::write(dir.join("base"), "1\n").unwrap();
            g(dir, &["add", "."]);
            g(dir, &["commit", "-qm", "base"]);
        }
        let branch = |dir: &Path, b: &str, file: &str, text: &str, from: &str| {
            g(dir, &["checkout", "-qb", b, from]);
            std::fs::write(dir.join(file), text).unwrap();
            g(dir, &["add", "."]);
            g(dir, &["commit", "-qm", b]);
        };
        // rc-n: makepad + its extra half; rc-o: extra alone; rc-k: extra conflicts with trunk
        branch(&r, "rc-n", "n", "n\n", "work");
        branch(&x, "rc-n", "xn", "n\n", "trunk");
        branch(&x, "rc-o", "xo", "o\n", "trunk");
        branch(&x, "rc-k", "base", "k\n", "trunk");
        g(&r, &["checkout", "-q", "work"]);
        g(&x, &["checkout", "-q", "trunk"]);
        std::fs::write(x.join("base"), "trunk\n").unwrap();
        g(&x, &["commit", "-qam", "trunk moves"]);
        g(&x, &["branch", "rc-in", "trunk"]);
        let events = Arc::new(Mutex::new(Vec::new()));
        let t = train(&home, &r, Path::new("/usr/bin/true"), events.clone());
        let ask = |line: &str| {
            let mut out = Vec::new();
            t.serve(line, &mut out);
            out
        };
        assert!(ask("add rc-n extra=rc-none")[0].contains("no extra branch rc-none"));
        assert!(ask("add rc-n commercial=rc-n")[0].contains("err"), "commercial's half is the worktree form");
        assert!(ask("add rc-n extra=rc-n extra=rc-o")[0].contains("extra twice"));
        assert!(ask("add - extra=rc-in")[0].contains("nothing to land"), "a half with nothing new is left out");
        let added = ask("add rc-n extra=rc-n agent=lane-n");
        assert!(added[0].contains("\"label\":\"rc-n+extra=rc-n\""), "{added:?}");
        assert!(ask("add - extra=rc-o")[0].contains("\"label\":\"-+extra=rc-o\""));
        assert!(ask("add - extra=rc-k")[0].contains("\"id\":3"));
        // the same halves again: the same entry
        assert!(ask("add - extra=rc-o")[0].contains("\"id\":2"));
        assert!(t.start_ready());
        let st = |b: &str| Entry::parse(&ask(&format!("status {b}"))[0]).unwrap();
        let (n, o, k) = (st("rc-n+extra=rc-n"), st("-+extra=rc-o"), st("-+extra=rc-k"));
        // (the train's events say why an entry stayed: its round stopped, its change did not save;
        // one red here, 11-08, left its second entry queued untouched with nothing said)
        assert_eq!((n.state, o.state), (St::Landed, St::Landed), "{n:?} {o:?}; the train said: {:?}", events.lock().unwrap());
        assert_eq!((k.state, k.detail.clone()), (St::Conflict, vec!["apps/extra/base".to_string()]));
        // trunk has both extra halves, work the makepad one; the user's checkouts moved with them
        let trunk = rev(&x, "refs/heads/trunk").unwrap();
        assert!(is_ancestor(&x, &rev(&x, "rc-n").unwrap(), &trunk) && is_ancestor(&x, &rev(&x, "rc-o").unwrap(), &trunk));
        assert!(is_ancestor(&r, &rev(&r, "rc-n").unwrap(), &rev(&r, "refs/heads/work").unwrap()));
        assert!(r.join("n").exists() && x.join("xn").exists() && x.join("xo").exists());
        assert_eq!(o.nested[0].landed, trunk, "the tip it landed at");
        assert!(n.nested[0].landed.len() == 40 && is_ancestor(&x, &n.nested[0].landed, &trunk));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// [`WIRE_SOURCES`] is every file the hosted wire's fingerprint hashes: the `include_bytes!` of
    /// the studio crate's `WIRE` and of key_code's `SOURCE_FINGERPRINT`.
    #[test]
    fn the_wire_sources_are_the_files_the_wire_fingerprint_hashes() {
        // the files a const's `include_bytes!` names, from `from` to its end `to`
        let included = |text: &str, from: &str, to: &str, dir: &str| -> Vec<String> {
            let body = &text[text.find(from).unwrap_or_else(|| panic!("no `{from}`"))..];
            let body = &body[..body.find(to).unwrap()];
            body.split("include_bytes!(\"").skip(1).map(|r| format!("{dir}/{}", &r[..r.find('"').unwrap()])).collect()
        };
        let studio = include_str!("../../../platform/studio/src/lib.rs");
        let key_code = include_str!("../../../platform/key_code/src/lib.rs");
        assert!(studio.contains("makepad_key_code::SOURCE_FINGERPRINT"), "the wire seeds with key_code's fingerprint");
        let mut want = included(key_code, "pub const SOURCE_FINGERPRINT", ";\n", "platform/key_code/src");
        want.extend(included(studio, "pub const WIRE: u64", "\n};", "platform/studio/src"));
        assert_eq!(want, WIRE_SOURCES, "the wire's files and WIRE_SOURCES, in that order");
    }

    /// A hand-in that changes a file of the hosted wire fails, naming the files and the mark; the
    /// same hand-in marked `wire-change` lands and says it changes the wire. Other files of the
    /// same crate land as ever.
    #[cfg(unix)]
    #[test]
    fn a_change_of_the_hosted_wire_lands_only_marked() {
        let d = std::env::temp_dir().join(format!("devhub-land-wire-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (r, home) = (d.join("repo"), d.join("home"));
        let c = r.join("apps/commercial");
        std::fs::create_dir_all(r.join("platform/studio/src")).unwrap();
        std::fs::create_dir_all(&c).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let g = |dir: &Path, args: &[&str]| assert!(git(dir, args).0, "git {args:?} in {}", dir.display());
        for (dir, main) in [(&r, "work"), (&c, "main")] {
            g(dir, &["init", "-q", "-b", main]);
            g(dir, &["config", "user.email", "t@t"]);
            g(dir, &["config", "user.name", "t"]);
        }
        std::fs::write(r.join(".gitignore"), "apps/commercial\n").unwrap();
        std::fs::write(r.join("platform/studio/src/studio.rs"), "pub struct Hosted;\n").unwrap();
        std::fs::write(r.join("platform/studio/src/other.rs"), "pub struct Other;\n").unwrap();
        g(&r, &["add", "."]);
        g(&r, &["commit", "-qm", "base"]);
        std::fs::write(c.join("s"), "1\n").unwrap();
        g(&c, &["add", "."]);
        g(&c, &["commit", "-qm", "cbase"]);
        for (b, file) in [("rc-wire", "studio.rs"), ("rc-plain", "other.rs")] {
            g(&r, &["checkout", "-qb", b, "work"]);
            std::fs::write(r.join("platform/studio/src").join(file), "// a comment\n").unwrap();
            g(&r, &["commit", "-qam", b]);
        }
        g(&r, &["checkout", "-q", "work"]);
        let events = Arc::new(Mutex::new(Vec::new()));
        let t = train(&home, &r, Path::new("/usr/bin/true"), events.clone());
        let ask = |line: &str| {
            let mut out = Vec::new();
            t.serve(line, &mut out);
            out
        };
        let st = |b: &str| Entry::parse(&ask(&format!("status {b}"))[0]).unwrap();
        assert!(ask("add rc-wire")[0].contains("\"id\":1"));
        assert!(ask("add rc-plain")[0].contains("\"id\":2"));
        t.start_ready();
        let (wire, plain) = (st("rc-wire"), st("rc-plain"));
        assert_eq!((wire.state, wire.detail[0].as_str()), (St::Failed, "it changes the hosted wire: platform/studio/src/studio.rs"), "{wire:?}");
        assert!(wire.detail.iter().any(|l| l.contains("makepad devhub land --wire-change")), "{wire:?}");
        assert_eq!(plain.state, St::Landed, "{plain:?}");
        assert!(!is_ancestor(&r, &rev(&r, "rc-wire").unwrap(), &rev(&r, "refs/heads/work").unwrap()), "work has no wire change");
        // marked, it lands, and says so
        assert!(ask("add rc-wire wire-change")[0].contains("\"id\":1"), "the same entry, now marked");
        assert!(st("rc-wire").wire);
        t.start_ready();
        assert_eq!(st("rc-wire").state, St::Landed, "{:?}", st("rc-wire"));
        let ev = events.lock().unwrap().join("\n");
        assert!(ev.contains("land refused rc-wire: it changes the hosted wire: platform/studio/src/studio.rs"), "{ev}");
        assert!(ev.contains("land landed rc-wire") && ev.contains("it changes the hosted wire (platform/studio/src/studio.rs)"), "{ev}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A two-repo lane's hand-in lands exactly the commits pinned when it was handed in. A conflict
    /// lands nothing and stays so whatever the branches do next, a retry too; a new hand-in pins
    /// the branches anew, and commits after it wait for the next one. #2759 conflicted, its
    /// commercial branch moved, the train retried it and landed that newer commercial head beside
    /// the makepad half of the hand-in: a pair never built together (11-08).
    #[cfg(unix)]
    #[test]
    fn a_two_repo_hand_in_lands_its_pinned_pair_or_nothing() {
        let d = std::env::temp_dir().join(format!("devhub-land-pinned-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (r, home) = (d.join("repo"), d.join("home"));
        let c = r.join("apps/commercial");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let g = |dir: &Path, args: &[&str]| assert!(git(dir, args).0, "git {args:?} in {}", dir.display());
        for (dir, main) in [(&r, "work"), (&c, "main")] {
            g(dir, &["init", "-q", "-b", main]);
            g(dir, &["config", "user.email", "t@t"]);
            g(dir, &["config", "user.name", "t"]);
        }
        std::fs::write(r.join(".gitignore"), "apps/commercial\n").unwrap();
        let commit = |dir: &Path, file: &str, text: &str, msg: &str| {
            std::fs::write(dir.join(file), text).unwrap();
            g(dir, &["add", "."]);
            g(dir, &["commit", "-qm", msg]);
            rev(dir, "HEAD").unwrap()
        };
        commit(&r, "lib", "fn tubes(curves) {}\n", "base");
        commit(&c, "model", "tubes(curves)\n", "cbase");
        // the lane's pair: its makepad half and a commercial half that conflicts with main
        g(&r, &["checkout", "-qb", "rc-s"]);
        commit(&r, "cage", "cage\n", "cage");
        g(&c, &["checkout", "-qb", "rc-s"]);
        commit(&c, "model", "tubes(curves) cage\n", "cage");
        g(&r, &["checkout", "-q", "work"]);
        g(&c, &["checkout", "-q", "main"]);
        commit(&c, "model", "tubes(curves) other\n", "main moves");
        let (w0, m0) = (rev(&r, "refs/heads/work").unwrap(), rev(&c, "refs/heads/main").unwrap());
        let t = train(&home, &r, Path::new("/usr/bin/true"), Arc::new(Mutex::new(Vec::new())));
        let ask = |line: &str| {
            let mut out = Vec::new();
            t.serve(line, &mut out);
            out
        };
        let st = || Entry::parse(&ask("status rc-s")[0]).unwrap();
        let hand_in = format!("add rc-s {} rc-s agent=lane-s", c.display());
        assert!(ask(&hand_in)[0].contains("\"id\":1"));
        let pinned = (st().ms.clone(), st().cs.clone());
        assert_eq!(pinned, (rev(&r, "rc-s").unwrap(), rev(&c, "rc-s").unwrap()), "pinned at the hand-in");
        assert!(!t.start_ready());
        assert_eq!((st().state, st().detail.clone()), (St::Conflict, vec!["model".to_string()]));
        // the lane resolves the conflict on its commercial branch and goes on with the next change's
        // commercial half; its makepad half comes later: nothing lands, a retry neither
        g(&c, &["checkout", "-q", "rc-s"]);
        g(&c, &["merge", "-q", "-X", "theirs", "main", "-m", "main into rc-s"]);
        let blend = commit(&c, "model", "tubes(curves, blend)\n", "blend, commercial half");
        g(&c, &["checkout", "-q", "main"]);
        for _ in 0..2 { assert!(!t.start_ready()); }
        assert!(ask("retry rc-s")[0].contains("ok"));
        assert!(!t.start_ready());
        assert_eq!((st().state, (st().ms.clone(), st().cs.clone())), (St::Conflict, pinned.clone()), "a retry is the same hand-in");
        assert_eq!((rev(&r, "refs/heads/work").unwrap(), rev(&c, "refs/heads/main").unwrap()), (w0.clone(), m0.clone()), "nothing landed");
        // the makepad half too, then a hand-in: it pins both; a commit after it waits for the next
        g(&r, &["checkout", "-q", "rc-s"]);
        let blend_oss = commit(&r, "lib", "fn tubes(curves, blend) {}\n", "blend, makepad half");
        g(&r, &["checkout", "-q", "work"]);
        assert!(ask(&hand_in)[0].contains("\"id\":1"), "the same entry, handed in again");
        assert_eq!((st().state, st().ms.clone(), st().cs.clone()), (St::Queued, blend_oss.clone(), blend.clone()));
        g(&c, &["checkout", "-q", "rc-s"]);
        let later = commit(&c, "model", "tubes(curves, blend, more)\n", "after the hand-in");
        g(&c, &["checkout", "-q", "main"]);
        assert!(t.start_ready());
        let e = st();
        assert_eq!(e.state, St::Landed, "{e:?}");
        let (work, main) = (rev(&r, "refs/heads/work").unwrap(), rev(&c, "refs/heads/main").unwrap());
        assert_eq!(e.landed, format!("work {work} main {main}"));
        assert!(is_ancestor(&r, &blend_oss, &work) && is_ancestor(&c, &blend, &main), "the pinned pair landed");
        assert!(!is_ancestor(&c, &later, &main), "a commit after the hand-in did not");
        assert_eq!(std::fs::read_to_string(c.join("model")).unwrap(), "tubes(curves, blend)\n");
        assert_eq!(std::fs::read_to_string(r.join("lib")).unwrap(), "fn tubes(curves, blend) {}\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A round that stops on a tip that does not read says so and which, and a change of an entry
    /// that does not save says so too: either left a hand-in queued untouched with nothing said.
    #[cfg(unix)]
    #[test]
    fn a_stopped_round_and_an_unsaved_change_are_said() {
        use std::os::unix::fs::PermissionsExt;
        let d = std::env::temp_dir().join(format!("devhub-land-said-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (r, home) = (d.join("repo"), d.join("home"));
        let c = r.join("apps/commercial");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let g = |dir: &Path, args: &[&str]| assert!(git(dir, args).0, "git {args:?} in {}", dir.display());
        for (dir, main) in [(&r, "work"), (&c, "main")] {
            g(dir, &["init", "-q", "-b", main]);
            g(dir, &["config", "user.email", "t@t"]);
            g(dir, &["config", "user.name", "t"]);
            std::fs::write(dir.join("base"), "1\n").unwrap();
            g(dir, &["add", "."]);
            g(dir, &["commit", "-qm", "base"]);
        }
        std::fs::write(r.join(".gitignore"), "apps/commercial\n").unwrap();
        g(&r, &["add", "."]);
        g(&r, &["commit", "-qm", "ignore"]);
        g(&r, &["checkout", "-qb", "rc-a"]);
        std::fs::write(r.join("a"), "a\n").unwrap();
        g(&r, &["add", "."]);
        g(&r, &["commit", "-qm", "rc-a"]);
        g(&r, &["checkout", "-q", "work"]);
        let events = Arc::new(Mutex::new(Vec::new()));
        let t = train(&home, &r, Path::new("/usr/bin/true"), events.clone());
        let ask = |line: &str| {
            let mut out = Vec::new();
            t.serve(line, &mut out);
            out
        };
        let st = || Entry::parse(&ask("status rc-a")[0]).unwrap();
        assert!(ask("add rc-a")[0].contains("\"id\":1"));
        // commercial's main does not read: the round stops before rc-a, and says so
        let (main, away) = (c.join(".git/refs/heads/main"), c.join(".git/main.away"));
        std::fs::rename(&main, &away).unwrap();
        assert!(!t.start_ready());
        let said = events.lock().unwrap().join("\n");
        assert!(said.contains(&format!("land: main in {} did not read: the round stops before rc-a", c.display())), "{said}");
        assert_eq!(st().state, St::Queued);
        std::fs::rename(&away, &main).unwrap();
        // the state's folder takes no write: the landing's change does not save, and says so
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o555)).unwrap();
        // (a user no permission stops, root, writes there anyway: nothing to see)
        if std::fs::write(home.join("probe"), "").is_err() {
            assert!(t.start_ready(), "work moved");
            let said = events.lock().unwrap().join("\n");
            assert!(said.contains("land rc-a: its change did not save, so it stays as it was: "), "{said}");
            assert_eq!(st().state, St::Queued, "the store's state is the train's");
        }
        std::fs::set_permissions(&home, std::fs::Permissions::from_mode(0o755)).unwrap();
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Hand-ins, a conflict and a hold; both checkouts move in queue order.
    #[cfg(unix)]
    #[test]
    fn a_round_lands_entries_in_order() {
        let d = std::env::temp_dir().join(format!("devhub-land-round-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (r, home) = (d.join("repo"), d.join("home"));
        let c = r.join("apps/commercial");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let g = |dir: &Path, args: &[&str]| assert!(git(dir, args).0, "git {args:?} in {}", dir.display());
        for (dir, main) in [(&r, "work"), (&c, "main")] {
            g(dir, &["init", "-q", "-b", main]);
            g(dir, &["config", "user.email", "t@t"]);
            g(dir, &["config", "user.name", "t"]);
        }
        std::fs::write(r.join(".gitignore"), "apps/commercial\n").unwrap();
        std::fs::write(r.join("a"), "1\n").unwrap();
        g(&r, &["add", "."]);
        g(&r, &["commit", "-qm", "base"]);
        std::fs::write(c.join("s"), "1\n").unwrap();
        g(&c, &["add", "."]);
        g(&c, &["commit", "-qm", "cbase"]);
        // rc-x: a makepad file; rc-y: makepad + its commercial half; rc-z: conflicts with work
        let branch = |dir: &Path, b: &str, file: &str, text: &str, from: &str| {
            g(dir, &["checkout", "-qb", b, from]);
            std::fs::write(dir.join(file), text).unwrap();
            g(dir, &["add", "."]);
            g(dir, &["commit", "-qm", b]);
        };
        branch(&r, "rc-x", "x", "x\n", "work");
        branch(&r, "rc-y", "y", "y\n", "work");
        branch(&c, "rc-y", "sy", "y\n", "main");
        branch(&r, "rc-z", "a", "z\n", "work");
        branch(&r, "rc-h", "h", "h\n", "work");
        branch(&r, "rc-bad", "bad", "bad\n", "work");
        g(&r, &["checkout", "-q", "work"]);
        g(&c, &["checkout", "-q", "main"]);
        std::fs::write(r.join("a"), "w\n").unwrap();
        g(&r, &["commit", "-qam", "work moves"]);
        let exe = Path::new("/usr/bin/true");
        let events = Arc::new(Mutex::new(Vec::new()));
        let t = train(&home, &r, exe, events.clone());
        // each landing tells its pairs before the checkout moves (the Mac's shows hold first)
        let told = Arc::new(Mutex::new(Vec::new()));
        let (told2, r2) = (told.clone(), r.clone());
        t.before_ff(Arc::new(move |from: &crate::state::Pair, to: &crate::state::Pair| {
            assert_eq!(rev(&r2, "refs/heads/work").as_deref(), Some(from.work.as_str()), "told before the fast-forward");
            told2.lock().unwrap().push((from.clone(), to.clone()));
        }));
        let ask = |line: &str| {
            let mut out = Vec::new();
            t.serve(line, &mut out);
            out
        };
        assert!(ask("add rc-nope")[0].contains("err"));
        assert!(ask("add rc-x agent=lane-x")[0].contains("\"id\":1"));
        assert!(ask(&format!("add rc-y {} rc-y", c.display()))[0].contains("\"id\":2"));
        assert!(ask("add rc-z")[0].contains("\"id\":3"));
        assert!(ask("add rc-h")[0].contains("\"id\":4"));
        assert!(ask("hold rc-h waits for the user")[0].contains("ok"));
        assert!(ask("add rc-bad")[0].contains("\"id\":5"));
        assert!(t.start_ready());
        let st = |b: &str| Entry::parse(&ask(&format!("status {b}"))[0]).unwrap();
        assert_eq!(st("rc-x").state, St::Landed, "{:?}", st("rc-x"));
        assert_eq!(st("rc-y").state, St::Landed, "{:?}", st("rc-y"));
        assert_eq!((st("rc-z").state, st("rc-z").detail.clone()), (St::Conflict, vec!["a".to_string()]));
        assert_eq!((st("rc-h").state, st("rc-h").held.as_str()), (St::Queued, "waits for the user"));
        assert_eq!(st("rc-bad").state, St::Landed, "{:?}", st("rc-bad"));
        // work has both makepad halves, main the commercial half; the user's checkouts moved with them
        let work = rev(&r, "refs/heads/work").unwrap();
        assert!(is_ancestor(&r, &rev(&r, "rc-x").unwrap(), &work) && is_ancestor(&r, &rev(&r, "rc-y").unwrap(), &work));
        assert!(is_ancestor(&c, &rev(&c, "rc-y").unwrap(), &rev(&c, "refs/heads/main").unwrap()));
        assert!(r.join("x").exists() && r.join("y").exists() && c.join("sy").exists());
        let told = told.lock().unwrap().clone();
        assert_eq!(told.len(), 3, "rc-x, rc-y and rc-bad: {told:?}");
        assert!(told.windows(2).all(|w| w[0].1 == w[1].0), "each from the last one's tip: {told:?}");
        assert_eq!(told.last().map(|t| t.1.work.clone()), Some(work.clone()));
        // the conflict is final, the held entry waits for its unhold
        assert!(!t.start_ready());
        // the queue survives a restart
        let t2 = train(&home, &r, exe, events.clone());
        t2.load();
        assert_eq!(t2.st.lock().unwrap().len(), 5);
        // numbers are never reused: the newest entry gone (dropped or trimmed), the next is still new
        t2.st.lock().unwrap().retain(|e| e.id != 5);
        t2.save(&mut t2.st.lock().unwrap()).unwrap();
        let t3 = train(&home, &r, exe, events.clone());
        t3.load();
        assert_eq!(t3.st.lock().unwrap().iter().map(|e| e.id).max(), Some(4));
        assert!(t3.store().read(|d| d.land_next) >= 6, "the counter outlives the entry: the next number is 6, not 5");
        assert_eq!(t.counts(), Counts { queued: 0, failed: 1, held: 1 });
        // a pause holds the train across a restart; resume lets it run again
        assert_eq!(ask("pause user review"), vec!["{\"paused\":\"user review\"}".to_string()]);
        let t3 = train(&home, &r, exe, events.clone());
        t3.load();
        assert_eq!(t3.paused().as_deref(), Some("user review"));
        assert_eq!(ask("resume"), vec!["{\"paused\":\"\"}".to_string()]);
        assert_eq!(t.paused(), None);
        assert!(!home.join("land.paused").exists() && !home.join("land.jsonl").exists());
        let ev = events.lock().unwrap().join("\n");
        for want in ["land queued rc-x", "land landed rc-x", "land landed rc-y+rc-y", "land conflict rc-z", "land paused (user review)", "land resumed"] {
            assert!(ev.contains(want), "no `{want}` in\n{ev}");
        }
        assert!(ask("drop rc-z")[0].contains("ok"));
        assert_eq!(ask("status all").len(), 4);
        // a branch already in work lands nothing: it is just before what it landed
        g(&r, &["branch", "rc-in", "work"]);
        assert!(ask("add rc-in")[0].contains("\"id\":6"));
        t.start_ready();
        let e = st("rc-in");
        assert_eq!((e.state, e.pre.clone()), (St::Landed, e.landed.clone()), "{e:?}");
        assert!(!e.landed.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
