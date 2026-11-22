//! The landing train (`makepad devhub land`), run by the master. Hand-ins merge into work and
//! commercial main in queue order, then fast-forward the user's checkout. A hand-in's half in
//! another nested repository ([`crate::nested`]) merges into the branch the user's checkout of it
//! is on, and that checkout moves with the rest. A landing merges exactly the commits pinned at
//! its hand-in, every repository's, never a branch's later head. A conflict goes back to the lane,
//! nothing landed, until it hands in again; dirty overlaps wait. Holds and pause are explicit
//! controls.
//! Nothing is checked at the merge: a hand-in lands as soon as it merges, and the post-land
//! buckets check the landed tips, the crates the landings reach among them (see `bucket.rs`).
//! Every transition is a `land` record on the master's feed. The queue and pause live in state.json.

use crate::proto::{hub_record, Record, Sev};
use makepad_strict_json::{obj, Value};
use std::collections::{BTreeSet, HashSet};
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
const ADD_USAGE: &str = "add <branch|-> [<commercial worktree> <commercial branch>] [<nested repository>=<branch>]... [revert=<landing number|commit prefix>]... [supersede]";

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

/// The file that defines the frames of the AI bus between the WM and its hosted apps: what they
/// send each other has no fingerprint, so a WM and an app of different builds drop each other's
/// tool calls without a word (#7751 made `ServiceCall.origin` required and landed unmarked,
/// 11-21). A landing that changes it needs the `wire-change` mark as one of [`WIRE_SOURCES`] does.
/// The transports that carry the frames ([`AI_BUS_TRANSPORTS`]) define none of their own, and
/// `wire.rs` names no type of another module, so an edit of a transport changes where a frame goes,
/// never what a running peer has to read (a test holds both).
pub const AI_BUS_SOURCES: [&str; 1] = ["libs/ai/services/src/wire.rs"];

/// The files that carry the AI bus's frames without defining them: the app's port and service
/// sink, the WM's bus and the AI pane's bus. They land unmarked.
pub const AI_BUS_TRANSPORTS: [&str; 4] = [
    "libs/ai/services/src/port.rs",
    "libs/ai/services/src/app_service.rs",
    "apps/wm/src/ai_bus.rs",
    "apps/aichat/src/bus.rs",
];

/// The files that decide how micro_serde writes a frame: its JSON and binary encoders and their
/// derives, which name and order the fields and variants. The AI bus's frames are its JSON and the
/// hosted wire's frames are its binary, and neither fingerprint hashes these files, so an edit of
/// one changes what a running peer of another build reads, without a word. A landing that changes
/// one needs the `wire-change` mark as one of [`WIRE_SOURCES`] does. RON writes no frame of either
/// and lands unmarked.
pub const ENCODING_SOURCES: [&str; 4] = [
    "libs/micro_serde/src/serde_bin.rs",
    "libs/micro_serde/src/serde_json.rs",
    "libs/micro_serde/derive_impl/src/derive_bin.rs",
    "libs/micro_serde/derive_impl/src/derive_json.rs",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    Queued,
    Checking,
    Failed,
    Conflict,
    Waiting,
    Landed,
}

impl St {
    pub fn as_str(self) -> &'static str {
        match self {
            St::Queued => "queued",
            St::Checking => "checking",
            St::Failed => "failed",
            St::Conflict => "conflict",
            St::Waiting => "waiting",
            St::Landed => "landed",
        }
    }
    fn parse(s: &str) -> St {
        [St::Queued, St::Checking, St::Failed, St::Conflict, St::Waiting, St::Landed].into_iter().find(|x| x.as_str() == s).unwrap_or(St::Queued)
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
    /// The tips checked at hand-in (`<work>,<main>`), empty when unchecked.
    pub checked: String,
    pub touched: Vec<String>,
    pub wide: Vec<String>,
    pub set: Vec<String>,
    pub node: String,
    /// what runs for it now, empty when nothing does
    pub job: String,
    pub unchecked: String,
    /// How long its hand-in's gates took, in ms (`gate_ms=`; 0: not said)
    pub gate_ms: u64,
    /// `work <sha> main <sha>` once landed
    pub landed: String,
    /// `work <sha> main <sha>` just before the fast-forward; empty when unknown
    pub pre: String,
    pub detail: Vec<String>,
    pub agent: String,
    /// Only ci.toml changes.
    pub proof: bool,
    /// It may change the hosted wire ([`WIRE_SOURCES`], [`AI_BUS_SOURCES`], [`ENCODING_SOURCES`]):
    /// `land --wire-change`.
    pub wire: bool,
    /// the land rule's ledger witness as this landing left it (empty: none)
    pub witness: String,
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
            ("checked", s(&self.checked)),
            ("touched", Value::Arr(self.touched.iter().map(|p| s(p)).collect())),
            ("wide", Value::Arr(self.wide.iter().map(|p| s(p)).collect())),
            ("set", Value::Arr(self.set.iter().map(|p| s(p)).collect())),
            ("node", s(&self.node)),
            ("job", s(&self.job)),
            ("unchecked", s(&self.unchecked)),
            ("gate_ms", Value::Int(self.gate_ms as i64)),
            ("landed", s(&self.landed)),
            ("pre", s(&self.pre)),
            ("detail", Value::Arr(self.detail.iter().map(|d| s(d)).collect())),
            ("agent", s(&self.agent)),
            ("proof", Value::Bool(self.proof)),
            ("wire", Value::Bool(self.wire)),
            ("witness", s(&self.witness)),
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
        let list = |k: &str| v.get(k).and_then(Value::as_arr).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default();
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
            checked: s("checked"),
            touched: list("touched"),
            wide: list("wide"),
            set: list("set"),
            node: s("node"),
            job: s("job"),
            unchecked: s("unchecked"),
            gate_ms: n("gate_ms"),
            landed: s("landed"),
            pre: s("pre"),
            detail: v.get("detail").and_then(|d| d.as_arr()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            agent: s("agent"),
            proof: v.get("proof").and_then(Value::as_bool).unwrap_or(false),
            wire: v.get("wire").and_then(Value::as_bool).unwrap_or(false),
            witness: s("witness"),
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

/// Of `landed` (landings, any order), the newest two-repo landing a lane's pair holds one half of
/// (`work` the makepad half's commit, `main` the commercial half's): it, and whether the makepad
/// half is the one that holds it. A landing both halves hold ends the look: what landed before
/// it is in both. A lane merged work into its makepad half by hand, a live run started before
/// main came into its commercial half, and its store build asked for a crate the makepad half had
/// renamed: `no matching package named makepad-relax` (#4046, 11-12).
/// Newest is the order they landed in, not their numbers: a held hand-in lands after higher
/// numbers. A CI proof landing (only proof files, [`only_proofs`]) splits no pair:
/// every lane whose commercial half had not taken one was refused under its number (#5455, 11-16).
pub fn half_landed<'a>(landed: &'a [Entry], repo: &Path, work: &str, main: &str) -> Option<(&'a Entry, bool)> {
    if work.is_empty() || main.is_empty() { return None; }
    let commercial = repo.join("apps/commercial");
    let mut both: Vec<(&Entry, (String, String))> = landed.iter().filter(|e| e.state == St::Landed && !e.proof).filter_map(|e| {
        let (pre, done) = (pair_sides(&e.pre)?, pair_sides(&e.landed)?);
        (pre.0 != done.0 && pre.1 != done.1).then_some((e, done))
    }).collect();
    both.sort_by_key(|(e, _)| std::cmp::Reverse((e.changed_ms, e.id)));
    for (e, (w, m)) in both {
        match (is_ancestor(repo, &w, work), is_ancestor(&commercial, &m, main)) {
            (true, true) => return None,
            (false, false) => {}
            (makepad, _) => return Some((e, makepad)),
        }
    }
    None
}

/// A landing's recorded pair (`work <sha> main <sha>`): its work and main commits.
fn pair_sides(text: &str) -> Option<(String, String)> {
    match text.split_whitespace().collect::<Vec<_>>()[..] {
        ["work", w, "main", m] => Some((w.to_string(), m.to_string())),
        _ => None,
    }
}

/// The commercial main a makepad commit on work landed beside: the main of the landing that made
/// it, else of the newest landing it holds. None: it holds no landing of `landed`.
pub fn landed_main(landed: &[Entry], repo: &Path, work: &str) -> Option<String> {
    let mut sides: Vec<(u64, (String, String))> = landed.iter().filter(|e| e.state == St::Landed).filter_map(|e| Some((e.id, pair_sides(&e.landed)?))).collect();
    sides.sort_by_key(|(id, _)| std::cmp::Reverse(*id));
    sides.iter().find(|(_, (w, _))| w == work).or_else(|| sides.iter().find(|(_, (w, _))| is_ancestor(repo, w, work))).map(|(_, (_, m))| m.clone())
}

/// Whether a run pins makepad commit `work` beside the checkout's commercial HEAD `head` (a `--rev`
/// naming no commercial side): Ok(None) when they never split a two-repo landing; Ok(Some(why)) when
/// they split one and the head has no commits of its own (all in main): the run takes the main
/// `work` landed beside instead ([`landed_main`]), whichever half holds the landing (a rig branch on
/// an older base beside the lane's newer main died at cargo's "no matching package named
/// `makepad-relax`", 11-12; a lane's makepad commit on the newest work beside its untouched
/// commercial half, behind main by a two-repo landing, was refused every `--rev` run, 11-15); Err
/// when the head would lose its own commits, naming the merge that joins the halves.
pub fn beside_head(live: &[Entry], repo: &Path, work: &str, head: &str) -> Result<Option<String>, String> {
    let commercial = repo.join("apps/commercial");
    let Some((e, makepad)) = half_landed(live, repo, work, head) else { return Ok(None) };
    let main = crate::gitread::rev(&commercial, "refs/heads/main^{commit}").unwrap_or_default();
    if !is_ancestor(&commercial, head, &main) {
        return Err(format!("{}; or name the commercial side in the rev (`<rev>,commercial=<branch>@<sha>`)", half_landed_said(e, makepad, "")));
    }
    let split = if makepad {
        format!("makepad {work:.10} holds #{} {} and the commercial HEAD does not", e.id, e.label())
    } else {
        format!("the commercial HEAD holds #{} {} and makepad {work:.10} does not", e.id, e.label())
    };
    Ok(Some(format!("{split}: commercial runs at the main makepad {work:.10} landed beside")))
}

/// Why a run does not pin a pair [`half_landed`] found: which half lacks the landing, and how
/// that half takes it.
pub fn half_landed_said(e: &Entry, makepad: bool, branch: &str) -> String {
    let (has, lacks, how) = if makepad { ("makepad", "commercial", "`git -C apps/commercial merge main`") } else { ("commercial", "makepad", "`git merge work`") };
    let branch = if branch.is_empty() { String::new() } else { format!(" {branch}") };
    format!("the lane's {has} half holds #{} {} ({}) and its {lacks} half does not: a run would build a pair that never landed together; merge both halves together: `makepad devhub lanes merge{branch}` (or the {lacks} half alone: {how}) and run again", e.id, e.label(), e.agent)
}

/// A run's pinned pair [`half_landed`] found split (`e`, `makepad`: whether the makepad half holds
/// it), said so its lane can act at once: each half's branch and commit (`mk`, `c`: branch, sha),
/// and the fix for the half that lacks the landing: its branch holds it now (the rev pinned an older
/// commit of it), the lane's pair takes its tips in one step (`devhub lanes merge`), or a side
/// branch takes its own tip. The lanes refused today had merged or reset one half by hand, run a
/// commercial side branch that never took main, or pinned their own older commit, and read a
/// refusal that named neither their branch nor that cause (#5683, 11-16).
pub fn torn_pair_said(e: &Entry, makepad: bool, repo: &Path, mk: (&str, &str), c: (&str, &str)) -> String {
    let (has, lacks, has_repo, lacks_repo) = if makepad { (mk, c, "makepad", "commercial") } else { (c, mk, "commercial", "makepad") };
    let (dir, tip, how) = if makepad { (repo.join("apps/commercial"), "main", "`git -C apps/commercial merge main`") } else { (repo.to_path_buf(), "work", "`git merge work`") };
    let landed = pair_sides(&e.landed).map(|(w, m)| if makepad { m } else { w }).unwrap_or_default();
    let holds_now = !landed.is_empty() && crate::gitread::rev(&dir, &format!("refs/heads/{}", lacks.0)).is_some_and(|t| is_ancestor(&dir, &landed, &t));
    let fix = if holds_now {
        format!("branch {} holds it now: your rev pins its older {:.10}; run at the branch's tip", lacks.0, lacks.1)
    } else if lacks.0 == has.0 {
        format!("run `makepad devhub lanes merge {}` (both halves take their tips in one step), then run again", has.0)
    } else {
        format!("{} is a side branch, not the lane's {lacks_repo} half {}: merge {tip} into it ({how} with it checked out), or run with the lane's own half, then run again", lacks.0, has.0)
    };
    format!("{lacks_repo} rev {:.10} (branch {}) lacks #{} {} ({}), which your {has_repo} half {:.10} (branch {}) holds: a run would build a pair that never landed together; {fix}", lacks.1, lacks.0, e.id, e.label(), e.agent, has.1, has.0)
}

fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    crate::gitread::is_ancestor(dir, a, b)
}

/// Whether `tree`, a lane's worktree of one repository, sits on a detached HEAD holding commits
/// its lane branch `branch` lacks, said as the rest of a refusal: a hand-in of the branch would
/// land without them. A lane's script switched its commercial worktree to the branch's own
/// commit, the lane committed there, and its hand-in landed nothing without a word (#6597, 11-18).
fn detached_off(tree: &Path, branch: &str) -> Option<String> {
    if branch.is_empty() || branch == "-" || !tree.join(".git").exists() || git(tree, &["symbolic-ref", "-q", "HEAD"]).0 {
        return None;
    }
    let (ok, head) = git(tree, &["rev-parse", "--verify", "-q", "HEAD"]);
    if !ok { return None }
    let (ok, n) = git(tree, &["rev-list", "--count", &format!("refs/heads/{branch}..{head}")]);
    let n = n.parse::<u64>().ok().filter(|n| ok && *n > 0)?;
    let (held, them) = if n == 1 { ("1 commit".to_string(), "it") } else { (format!("{n} commits"), "them") };
    let on = tree.display();
    let fix = if rev(tree, &format!("refs/heads/{branch}")).is_some_and(|b| is_ancestor(tree, &b, &head)) {
        format!("`git -C {on} checkout -B {branch}` puts {them} on the branch")
    } else {
        format!("merge {them} into {branch} (`git -C {on} checkout {branch} && git merge {head:.10}`)")
    };
    Some(format!("{on} is on a detached HEAD at {head:.10} holding {held} its branch {branch} lacks, and a hand-in of {branch} would land without {them}; {fix}, then hand in again"))
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
    // a textual merge keeps both sides' additions of one key: valid git, and a manifest cargo
    // then refuses in every build of the workspace (a dependency declared twice in a commercial
    // crate, 11-15). Checked before any ref moves, as the merge bot checks its merges.
    let (Some(b), Some(s)) = (crate::gitread::rev(dir, base), crate::gitread::rev(dir, sha)) else { return Err(vec![format!("merge-tree: cannot read {base:.10} or {sha:.10}")]) };
    let broken = crate::gitread::broken_manifests(dir, &b, &s, &tree).map_err(|e| vec![format!("the merged manifests: {e}")])?;
    if !broken.is_empty() { return Err(broken); }
    Ok(Merge { base: base.into(), sha: sha.into(), tree, retired: retired.is_some(), unchanged })
}

/// A lane may have landed by content without its tip being an ancestor (11-14). Lane retire's
/// (checkout-only) question, so a featureless build compiles it out.
#[cfg(feature = "checkout")]
pub(crate) fn changes_nothing(dir: &Path, base: &str, sha: &str) -> Result<bool, String> {
    let base = crate::gitread::rev(dir, base).ok_or_else(|| format!("cannot read commit {base}"))?;
    let sha = crate::gitread::rev(dir, sha).ok_or_else(|| format!("cannot read commit {sha}"))?;
    let m = prepare_merge(dir, &base, &sha).map_err(|why| why.into_iter().next().unwrap_or_else(|| "merge-tree failed".into()))?;
    if m.unchanged { return Ok(true); }
    let (tree, _) = crate::gitread::commit_parts(dir, &base).ok_or_else(|| format!("cannot read tree of {base}"))?;
    Ok(m.tree == tree)
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
        // commit-tree runs no hook: the private check the hooks run, on what a public half adds
        // (Fab's makepad/apps, 11-22), as on makepad's
        if !m.unchanged && crate::nested::public(&h.name) {
            crate::private_check::merge_gate(&h.dir, &h.tip, &h.sha, &m.tree).map_err(|hits| hits.into_iter().map(|l| format!("{}: {l}", h.name)).collect::<Vec<_>>())?;
        }
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

/// The land rule's view of a landing ([`crate::gate`]): Makepad's work from its tip before to the
/// merge, and the commercial main the same way when this checkout holds the commercial repository.
fn rule_sides<'a>(r: &'a Path, c: &'a Path, w: &'a str, wm: &'a str, m: &'a str, mm: &'a str) -> Vec<crate::gate::Side<'a>> {
    let mut sides = vec![crate::gate::Side { name: "makepad", dir: r, from: w, merge: wm }];
    if c.join(".git").exists() {
        sides.push(crate::gate::Side { name: "commercial", dir: c, from: m, merge: mm });
    }
    sides
}

/// A standalone merge uses the repository's configured merge drivers.
pub fn merge_onto(dir: &Path, base: &str, sha: &str, msg: &str) -> Result<String, Vec<String>> {
    let merge = prepare_merge(dir, base, sha)?;
    finish_merge(dir, &merge, msg)
}

/// A merge's files that differ from the clean merge of its two parents (`merge-tree --write-tree`)
/// outside the files that conflict there: what its resolution changed that no conflict asked for.
/// A merge made by hand from a tree merged against an older tip, committed with the newer tip as
/// its parent and in the merge bot's form, reverted another lane's landing in three files this way
/// (11-13). Where each path's clean merge is the one side that changed it, it is read in process
/// ([`crate::gitread::one_sided_strays`]); git's merge decides the rest (a path both sides
/// changed, a criss-cross). Err: git failed.
fn strays(dir: &Path, merge: &str, p1: &str, p2: &str) -> Result<Vec<String>, String> {
    if let Some(files) = crate::gitread::one_sided_strays(dir, merge, p1, p2) {
        return Ok(files);
    }
    let (_, out) = git(dir, &["merge-tree", "--write-tree", "--name-only", "--no-messages", p1, p2]);
    let mut lines = out.lines();
    let clean = lines.next().unwrap_or("");
    // (a tree id: sync's valid_sha is a checkout build's alone)
    if clean.len() != 40 || !clean.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        return Err(format!("`git merge-tree --write-tree {p1:.10} {p2:.10}` failed"));
    }
    let conflicted: std::collections::HashSet<&str> = lines.filter(|l| !l.is_empty()).collect();
    let (ok, diff) = git(dir, &["diff-tree", "-r", "--name-only", "--no-renames", clean, &format!("{merge}^{{tree}}")]);
    if !ok {
        return Err(format!("the tree of {merge:.10} did not compare with its parents' merge"));
    }
    Ok(diff.lines().filter(|f| !f.is_empty() && !conflicted.contains(f)).map(str::to_string).collect())
}

/// Whether a hand-in (`pins`: its makepad commit, its commercial one and that checkout) holds at
/// `path` the bytes a live session shipped (`shipped`, its `sources` entry: link, exec bit, sha256).
fn carries(repo: &Path, (ms, cs, cwt): (&str, &str, &str), path: &str, shipped: &str) -> bool {
    let Some(sha) = shipped.rsplit(':').next().filter(|s| s.len() == 64) else { return false };
    let (dir, at, rel) = match path.strip_prefix("apps/commercial/") {
        Some(rel) if !cs.is_empty() => (Path::new(cwt), cs, rel),
        _ if !ms.is_empty() => (repo, ms, path),
        _ => return false,
    };
    let Some(oid) = crate::gitread::entries_at(dir, at, &[rel]).and_then(|v| v.into_iter().next()).filter(|o| !o.is_empty()) else { return false };
    crate::gitread::blob(dir, &oid).is_ok_and(|bytes| makepad_network::digest::sha256_hash(&bytes).iter().map(|b| format!("{b:02x}")).collect::<String>() == sha)
}

/// The merges a hand-in brings (`target..sha`) against their parents' clean merge
/// ([`strays`]). Err: a merge with files outside its conflicts that
/// differ from that clean merge, which the branch has not changed again since by a commit of its
/// own: a merge resolves its conflicts and nothing else, and a resolution past them is a commit
/// of its own. Such a file the tip changed since the merge base is a landing the merge reverts;
/// one it did not holds the branch's own work inside the merge (an amend onto the bot's merge).
/// Ok: what to say of the files the branch changed again since (its own to have checked).
pub fn merges_checked(dir: &Path, target: &str, sha: &str, branch: &str) -> Result<Vec<String>, String> {
    // (a retired chain's merges, `work-old-<date>`, landed long ago)
    let (ok, log) = git(dir, &["log", "--merges", "--format=%H %P%x09%s", &format!("refs/heads/{target}..{sha}"), "--not", &format!("--glob=refs/heads/{target}-old-*")]);
    if !ok {
        return Ok(vec![format!("the merges {branch} brings did not read: not checked against their parents")]);
    }
    let named = |files: &[String]| if files.len() > 6 { format!("{} and {} more", files[..6].join(" "), files.len() - 6) } else { files.join(" ") };
    // of `files`, those the git command lists (a diff's or a log's names)
    let changed = |args: &[&str], files: &[String]| -> std::collections::HashSet<String> {
        let mut args = args.to_vec();
        args.push("--");
        args.extend(files.iter().map(String::as_str));
        git(dir, &args).1.lines().filter(|l| !l.is_empty()).map(str::to_string).collect()
    };
    let (mut refused, mut said) = (Vec::new(), Vec::new());
    for l in log.lines() {
        let Some((ids, subject)) = l.split_once('\t') else { continue };
        let (m, p1, p2) = match ids.split_whitespace().collect::<Vec<_>>().as_slice() {
            [m, p1, p2] => (*m, *p1, *p2),
            _ => continue,
        };
        let files = match strays(dir, m, p1, p2) {
            Ok(f) if f.is_empty() => continue,
            Ok(f) => f,
            Err(e) => {
                said.push(format!("merge {m:.10} \"{subject}\": {e}: not checked"));
                continue;
            }
        };
        let against = format!("`git merge-tree --write-tree {p1:.10} {p2:.10}`");
        let since = changed(&["log", "--no-merges", "--name-only", "--format=", &format!("{m}..{sha}")], &files);
        let (fixed, kept): (Vec<String>, Vec<String>) = files.into_iter().partition(|f| since.contains(f));
        if !fixed.is_empty() {
            said.push(format!("merge {m:.10} \"{subject}\" differs from its parents' clean merge outside its conflicts in {}, which the branch changed again since: check them against {against}", named(&fixed)));
        }
        if kept.is_empty() { continue; }
        let mut tips = std::collections::HashSet::new();
        for base in git(dir, &["merge-base", "--all", p1, p2]).1.lines().filter(|b| !b.is_empty()) {
            tips.extend(changed(&["diff-tree", "-r", "--name-only", "--no-renames", base, p2], &kept));
        }
        let (tip, own): (Vec<String>, Vec<String>) = kept.into_iter().partition(|f| tips.contains(f));
        let mut why = Vec::new();
        if !tip.is_empty() { why.push(format!("it lacks what {p2:.10} changed in {} since the merge base: a tree merged against another tip reverts that tip's landings", named(&tip))); }
        if !own.is_empty() { why.push(format!("it holds the branch's own changes to {}, which neither side brought (an amend onto a merge?)", named(&own))); }
        refused.push(format!("merge {m:.10} \"{subject}\" is not the clean merge of its parents outside its conflicts ({against}): {}. Nothing the branch did since changed those files. Remake it (reset to {p1:.10}, `git merge {p2:.10}`, resolve only the conflicts and commit any other change on its own), or commit those files as they should be and hand in again", why.join("; ")));
    }
    if refused.is_empty() { Ok(said) } else { Err(refused.join("; ")) }
}

/// The hosted wire's files ([`WIRE_SOURCES`], [`AI_BUS_SOURCES`], [`ENCODING_SOURCES`]) that differ
/// between makepad commits `from` and `to`; Err: the diff did not read.
fn wire_changed(dir: &Path, from: &str, to: &str) -> Result<Vec<String>, String> {
    let mut args = vec!["diff", "--name-only", "--no-renames", from, to, "--"];
    args.extend(WIRE_SOURCES);
    args.extend(AI_BUS_SOURCES);
    args.extend(ENCODING_SOURCES);
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
        Ok(files) => {
            let mut said = vec![format!("it changes the hosted wire: {}", files.join(" "))];
            if files.iter().any(|f| WIRE_SOURCES.contains(&f.as_str())) {
                said.push("an edit of the studio and key_code files (a comment too) changes the wire fingerprint: every WM built before it refuses every app built after it, and the reverse, until both are rebuilt".into());
            }
            if files.iter().any(|f| AI_BUS_SOURCES.contains(&f.as_str())) {
                said.push("the AI bus's frames (wire.rs) are what the WM and its apps send each other, with no fingerprint: a WM and an app of different builds drop each other's tool calls without a word until both are rebuilt".into());
            }
            if files.iter().any(|f| ENCODING_SOURCES.contains(&f.as_str())) {
                said.push("micro_serde's JSON and binary encoding writes the AI bus's and the hosted wire's frames, and no fingerprint hashes it: a WM and an app of different builds misread each other's frames without a word until both are rebuilt".into());
            }
            said.push("if that is meant: makepad devhub land --wire-change (each node's WM then restarts on the new wire as its deployment moves); if not, leave these files as work has them".into());
            Some(said)
        }
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
            || l.starts_with("GATESTEP ")
            || l.starts_with("GATEOUT ")
            || l.starts_with("GATEKEPT ")
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
    /// the land rule ([`crate::gate::land_rule`]; a test: its stand-in)
    rule: RuleFn,
    /// a round: one at a time
    round: Mutex<()>,
    /// the members packages at the tips it was read at ([`Self::members`]; read from the tips'
    /// manifests, so a featureless build knows none)
    #[cfg(feature = "checkout")]
    members: Mutex<Option<((String, String), BTreeSet<String>)>>,
}

/// [`Train::rule`]: the land rule's answer for one landing over its repositories.
pub type RuleFn = fn(&Path, crate::gate::Verb, &str, &[crate::gate::Side], &str, &str) -> Result<Vec<String>, crate::gate::No>;

/// The packages two entries' sets share. A members package ([`Train::members`]) counts only for
/// an entry that touched it (its manifest): it is in every commercial hand-in's set and builds none
/// of the crates it brings in.
fn meeting(a: &Entry, b: &Entry, members: &BTreeSet<String>) -> Vec<String> {
    a.set.iter().filter(|p| b.set.contains(p) && (!members.contains(*p) || a.touched.contains(p) || b.touched.contains(p))).cloned().collect()
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

    /// Why the land rule is switched off ([`crate::state::Doc::land_rule_off`]); None while it runs.
    pub fn rule_off(&self) -> Option<String> {
        Some(self.store().read(|d| d.land_rule_off.clone())).filter(|w| !w.is_empty())
    }

    /// Switches the land rule off (`Some(why)`) or on (`None`); kept across master restarts, said
    /// on the feed when it changes.
    pub fn set_rule_off(&self, why: Option<&str>) -> Result<(), String> {
        let why = why.map(|w| if w.trim().is_empty() { "switched off" } else { w.trim() }).unwrap_or("").to_string();
        let was = self.store().read(|d| d.land_rule_off.clone());
        // off drops the ledger witness: the first landing after `on` starts a fresh one
        self.store.update(|d| {
            d.land_rule_off = why.clone();
            if !why.is_empty() {
                d.land_witness.clear();
            }
        })?;
        if was.is_empty() != why.is_empty() {
            self.event(Sev::Warn, if why.is_empty() { "land rule on: every landing runs it again, the first one starting a fresh ledger witness".into() } else { format!("land rule off ({why}): no landing runs it, and the master neither builds nor ticks its binary") }, &[]);
        }
        Ok(())
    }

    /// The land rule's answer ([`Self::rule`]), or while it is switched off ([`Self::rule_off`])
    /// its one note: it runs nothing and needs no binary then. It gets the last landing's ledger
    /// witness; a `land`'s new one is kept for the next run (and returned, for its entry), never
    /// said as a note.
    fn ruled(&self, repo: &Path, verb: crate::gate::Verb, branch: &str, sides: &[crate::gate::Side]) -> Result<(Vec<String>, Option<String>), crate::gate::No> {
        if let Some(why) = self.rule_off() {
            return Ok((vec![format!("land rule off ({why}): nothing checked")], None));
        }
        let last = self.store().read(|d| d.land_witness.clone());
        // none since an off and on: the last landed entry's, so the rule says what was lost since
        let before = if last.is_empty() { self.st.lock().unwrap().iter().filter(|e| e.state == St::Landed && !e.witness.is_empty()).max_by_key(|e| e.id).map(|e| e.witness.clone()).unwrap_or_default() } else { String::new() };
        let said = (self.rule)(repo, verb, branch, sides, &last, &before)?;
        if verb != crate::gate::Verb::Land {
            return Ok((said, None));
        }
        let (notes, witness) = crate::gate::witness_of(said);
        if let Some(w) = &witness {
            if let Err(e) = self.store.update(|d| d.land_witness = w.clone()) {
                self.event(Sev::Err, format!("land rule: the ledger witness is not kept ({e}): the next landing is checked against the one before"), &[]);
            }
        }
        Ok((notes, witness))
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
        if e.state != St::Checking { e.job.clear(); }
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
                    "retry" => {
                        list[i].state = St::Queued;
                        list[i].job.clear();
                    }
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
            ["rule", rest @ ..] => {
                let set = match rest {
                    [] => Ok(()),
                    ["on"] => self.set_rule_off(None),
                    ["off", why @ ..] => self.set_rule_off(Some(&why.join(" "))),
                    _ => return err(out, "land rule [on | off [why]]"),
                };
                if let Err(e) = set { return err(out, &e); }
                let why = self.rule_off();
                out.push(format!("{{\"rule\":\"{}\",\"why\":{}}}", if why.is_some() { "off" } else { "on" }, makepad_remote::json_str(why.as_deref().unwrap_or(""))));
            }
            ["import", landq, failed, held] => match self.import(Path::new(landq), Path::new(failed), Path::new(held)) {
                Ok(n) => out.push(format!("{{\"ok\":1,\"imported\":{n}}}")),
                Err(e) => err(out, &e),
            },
            _ => err(out, &format!("land {ADD_USAGE} | status [all|<branch>] | hold <b> [reason] | unhold <b> | retry <b> | drop <b> | import <landq> <failed> <held> | pause [reason] | resume | paused | rule [on | off [why]]")),
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

    /// What the lane's own live sessions hold open for the files its hand-in brings: their last save
    /// did not compile, or the JIT reverted it, and the hand-in carries those very bytes (`pins`:
    /// its makepad commit, its commercial one and that checkout). Said, never refused: landing
    /// merges, the checks run after it. The JIT knew it at no build; a lane landed an app that did
    /// not compile (E0597), its live session had said so, and the fast bucket named it 17 min
    /// later (#4251, 11-13).
    fn live_open(&self, lane: &crate::state::LaneRec, pins: (&str, &str, &str)) -> Vec<String> {
        let lives: Vec<crate::state::Live> = self.store().read(|d| d.lives.iter().filter(|x| x.active() && (x.lane == lane.name || x.tree == lane.path)).cloned().collect());
        let mut out = Vec::new();
        for x in lives {
            let Some(p) = x.patch.as_ref() else { continue };
            let reason = if p.reason.is_empty() { p.reason_ref.read().unwrap_or_default() } else { p.reason.clone() };
            let errors = crate::jit_misses::compile_errors(&reason);
            let what = match p.outcome.as_str() {
                "compile_error" => "did not compile",
                "refused" if !errors.is_empty() => "did not compile",
                "reverted" | "jit_fault" => "was reverted by the JIT",
                _ => continue,
            };
            let carried: Vec<&str> = p.files.iter().map(String::as_str).filter(|f| x.sources.get(*f).is_some_and(|shipped| carries(&self.repo, pins, f, shipped))).collect();
            if carried.is_empty() { continue; }
            let why = errors.first().cloned().or_else(|| reason.lines().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string())).unwrap_or_default();
            let why = if why.chars().count() > 200 { format!("{}…", why.chars().take(200).collect::<String>()) } else { why };
            out.push(format!("live session {} of lane {}: its last save of {} {what} ({why}), and this hand-in carries those bytes: it lands, and the fast bucket will say so", x.id, lane.name, carried.join(", ")));
        }
        out
    }

    /// A hand-in lands only the halves it names; unlanded work on an omitted half is refused.
    fn add(&self, w: &[&str]) -> Result<String, String> {
        let proof = w.contains(&"proof");
        let wire = w.contains(&"wire-change");
        let supersede = w.contains(&"supersede");
        let agent = w.iter().find_map(|x| x.strip_prefix("agent=")).unwrap_or("").to_string();
        let gate_words = ["checked", "touched", "wide", "set", "node", "unchecked", "gate_ms"];
        let (mut checked, mut node, mut unchecked) = (String::new(), String::new(), String::new());
        let mut gate_ms = 0;
        let (mut touched, mut wide, mut set) = (Vec::new(), Vec::new(), Vec::new());
        let mut seen = std::collections::BTreeSet::new();
        for word in w {
            let Some((key, value)) = word.split_once('=').filter(|(k, _)| gate_words.contains(k)) else { continue };
            if !seen.insert(key) { return Err(format!("{word}: repeated hand-in word")); }
            match key {
                "checked" => {
                    let sha = |s: &str| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit());
                    if !value.split_once(',').is_some_and(|(w, m)| sha(w) && (m.is_empty() || sha(m))) {
                        return Err(format!("{word}: expected <40 hex>,<40 hex or empty>"));
                    }
                    checked = value.to_string();
                }
                "touched" | "wide" | "set" => {
                    let names: Vec<String> = if value.is_empty() { Vec::new() } else { value.split(',').map(str::to_string).collect() };
                    if names.iter().any(|n| n.is_empty() || !n.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))) {
                        return Err(format!("{word}: expected comma-separated package names ([A-Za-z0-9_-])"));
                    }
                    match key { "touched" => touched = names, "wide" => wide = names, _ => set = names }
                }
                "node" => node = value.to_string(),
                "gate_ms" => gate_ms = value.parse().map_err(|_| format!("{word}: expected milliseconds"))?,
                "unchecked" => {
                    if value.is_empty() || value.chars().any(|c| c.is_whitespace() || c == '=') { return Err(format!("{word}: expected an unchecked reason without spaces or =")); }
                    unchecked = value.to_string();
                }
                _ => unreachable!(),
            }
        }
        if !checked.is_empty() && !unchecked.is_empty() { return Err("checked= and unchecked= cannot both describe a hand-in".into()); }
        if checked.is_empty() && unchecked.is_empty() && !proof {
            return Err("the hand-in names no checked pair (checked=): hand in with a current devhub".into());
        }
        let reverts: Vec<&str> = w.iter().filter_map(|x| x.strip_prefix("revert=")).collect();
        for r in &reverts {
            if r.parse::<u64>().is_err() && !(r.len() >= 7 && r.bytes().all(|b| b.is_ascii_hexdigit())) {
                return Err(format!("revert={r}: expected a landing number or a commit sha prefix of 7+ hex digits"));
            }
        }
        let w: Vec<&str> = w.iter().copied().filter(|x| !x.starts_with("agent=") && !x.starts_with("revert=") && !x.split_once('=').is_some_and(|(k, _)| gate_words.contains(&k)) && *x != "proof" && *x != "wire-change" && *x != "supersede").collect();
        // `<name>=<branch>`: a half in another nested repository (a path has a `/` before any `=`)
        let (sides, w): (Vec<&str>, Vec<&str>) = w.into_iter().partition(|x| x.split_once('=').is_some_and(|(n, b)| crate::nested::valid(n) && !b.is_empty()));
        let (mut branch, cwt, cb) = match w.as_slice() {
            [b] => (*b, "", ""),
            [b, wt, cb] => (*b, *wt, *cb),
            _ => return Err(ADD_USAGE.into()),
        };
        let nested = self.sides(&sides)?;
        // check the lane's omitted half before checking branches or looking up the queue entry
        let lanes = self.store().read(|d| d.lanes.clone());
        let lane = match w.as_slice() {
            [b] if *b != "-" => lanes.iter().rev().find(|l| l.branch == *b || l.name == *b),
            ["-", _, cb] => lanes.iter().rev().find(|l| l.cbranch == *cb),
            _ => None,
        };
        // either worktree of the lane on a detached HEAD holding commits its branch lacks: a
        // hand-in of the branch lands without them ([`detached_off`])
        let owner = lane.or_else(|| lanes.iter().rev().find(|l| (branch != "-" && l.branch == branch) || (!cb.is_empty() && l.cbranch == cb)));
        if let Some(l) = owner {
            let tree = Path::new(&l.path);
            for (dir, b, repo_name) in [(tree.to_path_buf(), &l.branch, "makepad"), (tree.join("apps/commercial"), &l.cbranch, "commercial")] {
                if let Some(why) = detached_off(&dir, b) {
                    return Err(format!("lane {}'s {repo_name} worktree {why}", l.name));
                }
            }
        }
        let lane_cwt;
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
                    return Err(refuse());
                }
            }
        }
        // the commits this hand-in lands, pinned now ([`Entry::ms`])
        let ms = if branch == "-" { String::new() } else { rev(&self.repo, &format!("refs/heads/{branch}")).ok_or_else(|| format!("no makepad branch {branch}"))? };
        let cs = if cb.is_empty() { String::new() } else { rev(Path::new(cwt), &format!("refs/heads/{cb}")).ok_or_else(|| format!("no commercial branch {cb} in {cwt}"))? };
        if branch == "-" && cb.is_empty() && nested.is_empty() {
            return Err("nothing to land".into());
        }
        // an old tree with a newer parent silently undid other lanes' landings (11-15)
        let mut refused = Vec::new();
        for (dir, target, sha) in [(self.repo.as_path(), "work", &ms), (Path::new(cwt), "main", &cs)] {
            if sha.is_empty() { continue; }
            let mut by_commit = std::collections::BTreeMap::<String, Vec<String>>::new();
            // (a version older than the lane is one it never saw: the walk ends at its start)
            let since = lane.map_or(0, |l| (l.created_ms / 1000) as i64);
            // judged by what merging it onto the tip does, as its landing merges ([`prepare_merge`]),
            // not by its own history; one that does not merge cleanly is refused at its landing
            let tip = rev(dir, &format!("refs/heads/{target}")).ok_or_else(|| format!("no {target} in {}", dir.display()))?;
            let Ok(merge) = prepare_merge(dir, &tip, sha) else { continue };
            if merge.unchanged { continue; }
            for (path, x) in crate::gitread::undone(dir, &tip, &merge.tree, since)? {
                by_commit.entry(x).or_default().push(path);
            }
            for (x, paths) in by_commit {
                let landing = self.st.lock().unwrap().iter().filter(|e| e.state == St::Landed).find(|e| {
                    pair_sides(&e.landed).is_some_and(|(w, m)| if target == "work" { w == x } else { m == x })
                }).map(|e| (e.id, if target == "work" { e.branch.clone() } else { e.cbranch.clone() }));
                if reverts.iter().any(|r| landing.as_ref().is_some_and(|(id, _)| r.parse::<u64>().ok() == Some(*id)) || (r.len() >= 7 && r.bytes().all(|b| b.is_ascii_hexdigit()) && x.get(..r.len()).is_some_and(|p| p.eq_ignore_ascii_case(r)))) { continue; }
                let (name, token) = match landing {
                    Some((id, branch)) => (format!("#{id} ({branch})"), id.to_string()),
                    None => {
                        let subject = crate::gitread::with(dir, |p, repo| {
                            let oid = crate::gitread::resolve(p, repo, &x)?;
                            Some(repo.read_commit(&oid).ok()?.message.lines().next().unwrap_or("").trim().to_string())
                        }).flatten().unwrap_or_default();
                        (format!("{x:.10} {subject}"), format!("{x:.10}"))
                    }
                };
                let named = if paths.len() > 20 { format!("{} and {} more", paths[..20].join(", "), paths.len() - 20) } else { paths.join(", ") };
                refused.push(format!("the hand-in returns {} path(s) to the versions {name} replaced (its tree was read from an older {target} than its parent?): {named}; if the undo is meant, hand in again with --revert {token}", paths.len()));
            }
        }
        if !refused.is_empty() { return Err(refused.join("; ")); }
        // each merge it brings is its parents' merge outside its conflicts ([`merges_checked`])
        let mut warn = Vec::new();
        if !ms.is_empty() { warn.extend(merges_checked(&self.repo, "work", &ms, branch)?); }
        if !cs.is_empty() { warn.extend(merges_checked(Path::new(cwt), "main", &cs, cb)?); }
        if let Some(lane) = lane { warn.extend(self.live_open(lane, (&ms, &cs, cwt))); }
        let now = (self.clock)();
        let mut list = self.st.lock().unwrap();
        // (a hand-in of nested halves alone is the same entry for the same branches)
        let same = |e: &Entry| e.branch == branch && e.cbranch == cb && (branch != "-" || !cb.is_empty() || e.nested.iter().map(|n| (&n.name, &n.branch)).eq(nested.iter().map(|n| (&n.name, &n.branch))));
        let open = list.iter().position(|e| e.state != St::Landed && same(e));
        let id = match open {
            // a hand-in again: the same entry, queued again (its place in the queue kept). One still
            // queued at other commits is re-pinned only when asked: two lanes batched two reviewed
            // changes into one landing by handing in again before the first landed (11-16).
            Some(i) => {
                let e = &mut list[i];
                let pins = |ms: &str, cs: &str, nested: &[Side]| (ms.to_string(), cs.to_string(), nested.iter().map(|n| n.sha.clone()).collect::<Vec<_>>());
                if !matches!(e.state, St::Failed | St::Conflict) && !supersede && pins(&e.ms, &e.cs, &e.nested) != pins(&ms, &cs, &nested) {
                    let tip = |ms: &str, cs: &str, nested: &[Side]| [ms, cs].into_iter().chain(nested.iter().map(|n| n.sha.as_str())).find(|s| !s.is_empty()).unwrap_or("").to_string();
                    return Err(format!("#{} is still queued for this branch at {:.10}; pass --supersede to re-pin it to {:.10}, or wait for it to land", e.id, tip(&e.ms, &e.cs, &e.nested), tip(&ms, &cs, &nested)));
                }
                e.cwt = cwt.to_string();
                (e.ms, e.cs) = (ms, cs);
                e.nested = nested;
                (e.checked, e.touched, e.wide, e.set, e.node, e.unchecked, e.gate_ms) = (checked, touched, wide, set, node, unchecked, gate_ms);
                e.proof = proof;
                e.wire = wire;
                e.state = St::Queued;
                e.job.clear();
                e.detail.clear();
                e.changed_ms = now;
                e.id
            }
            None => {
                // (a counter in the state document: numbers of trimmed or lost entries are not reused)
                let id = self.store().read(|d| d.land_next).max(list.iter().map(|e| e.id).max().unwrap_or(0) + 1);
                list.push(Entry { id, branch: branch.into(), cwt: cwt.into(), cbranch: cb.into(), nested, state: St::Queued, held: String::new(), ms, cs, checked, touched, wide, set, node, job: String::new(), unchecked, gate_ms, landed: String::new(), pre: String::new(), detail: Vec::new(), agent, proof, wire, witness: String::new(), asked_ms: now, changed_ms: now });
                id
            }
        };
        let pos = list.iter().filter(|e| e.state != St::Landed && e.id <= id).count();
        self.save(&mut list)?;
        let label = list.iter().find(|e| e.id == id).map(Entry::label).unwrap_or_default();
        drop(list);
        self.event(Sev::Info, format!("land queued {label} (#{id}, {pos} open)"), &[]);
        self.kicked.store(true, Ordering::Release);
        let warn = if warn.is_empty() { String::new() } else { format!(",\"warn\":[{}]", warn.iter().map(|w| makepad_remote::json_str(w)).collect::<Vec<_>>().join(",")) };
        Ok(format!("{{\"ok\":1,\"id\":{id},\"pos\":{pos},\"label\":{}{warn}}}", makepad_remote::json_str(&label)))
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

    /// Walk the actual fast-forwards, not entry numbers: a held entry can land much later.
    fn landed_since(&self, checked: (&str, &str), tips: (&str, &str)) -> Option<Vec<Entry>> {
        let list = self.st.lock().unwrap();
        let mut at = (tips.0.to_string(), tips.1.to_string());
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        while (at.0.as_str(), at.1.as_str()) != checked {
            let (e, pre) = list.iter().filter(|e| e.state == St::Landed && !seen.contains(&e.id)).find_map(|e| {
                let pre = pair_sides(&e.pre)?;
                // An already-landed hand-in explains no movement of either tip.
                (pair_sides(&e.landed)? == at && pre != at).then_some((e, pre))
            })?;
            seen.insert(e.id);
            found.push(e.clone());
            at = pre;
        }
        Some(found)
    }

    /// The members packages at the tips ([`crate::orch::ci::members_packages`]), read again once
    /// they move; none when the tips or their tree do not read.
    fn members(&self) -> BTreeSet<String> {
        #[cfg(feature = "checkout")]
        {
            let r = &self.repo;
            let (Some(w), Some(m)) = (rev(r, "refs/heads/work"), rev(&r.join("apps/commercial"), "refs/heads/main")) else { return BTreeSet::new() };
            let mut kept = self.members.lock().unwrap();
            if let Some((_, set)) = kept.as_ref().filter(|(at, _)| at.0 == w && at.1 == m) { return set.clone(); }
            let set = crate::orch::ci::Tree::at(r, &crate::state::Pair::new(&w, &m)).map(|t| crate::orch::ci::members_packages(&t)).unwrap_or_default();
            *kept = Some(((w, m), set.clone()));
            set
        }
        #[cfg(not(feature = "checkout"))]
        BTreeSet::new()
    }

    /// Lands ready entries one at a time, oldest first.
    fn start_ready(self: &Arc<Self>) -> bool {
        let _round = self.round.lock().unwrap();
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
            let members = self.members();
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
                    e.job.clear();
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
            // Its check is carried to these tips, whatever landed since: nothing re-checks it at
            // the merge, and nothing waits for a check. What the landings since reach of its set
            // is the post-land buckets' to check on the tip, failures fixed forward (user, 11-16:
            // "just land them and keep them checking async"; a hand-in's re-check of 305 crates
            // held 19 hand-ins, the tip's own fix among them, 16 min and more, L1181372).
            if !e.proof {
                let checked = e.checked.split_once(',');
                if e.state == St::Checking || checked != Some((w0.as_str(), m0.as_str())) {
                    let since = checked.and_then(|checked| self.landed_since(checked, (&w0, &m0)));
                    let met: Vec<String> = {
                        let met: HashSet<String> = since.iter().flatten().flat_map(|p| meeting(p, &e, &members)).collect();
                        e.set.iter().filter(|c| since.is_none() || met.contains(*c)).cloned().collect()
                    };
                    if !met.is_empty() {
                        self.event(Sev::Info, format!("land {}: lands with its check carried to work {w0:.10}; the post-land buckets check {} on the tip", e.label(), met.join(", ")), &[]);
                    }
                    self.set(e.id, |e| {
                        e.checked = format!("{w0},{m0}");
                        if e.state == St::Checking { e.state = St::Queued; }
                        e.job.clear();
                    }, None);
                }
            }
            // the land rule ([`crate::gate`]), on every landing, over both repositories
            match self.ruled(r, crate::gate::Verb::Check, &e.branch, &rule_sides(r, &c, &w0, &cm, &m0, &cc)) {
                Ok(_) => {}
                Err(crate::gate::No::Wait(why)) => {
                    self.set(e.id, |e| {
                        e.state = St::Waiting;
                        e.detail = vec![why.clone()];
                    }, Some((Sev::Info, format!("land waiting {label}: {why}"))));
                    continue;
                }
                Err(crate::gate::No::Refused(why)) => {
                    self.set(e.id, |e| {
                        e.state = St::Failed;
                        e.detail = why.clone();
                    }, Some((Sev::Err, format!("land refused {label}: {}", why[0]))));
                    continue;
                }
            }
            if self.land_one(e.id, Attempt { ms, cs, cm, cc, w0, m0, halves }) {
                landed = true;
            }
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
        // tips that moved since the round's merge: its check is decided again at them, by the round
        if (w != w0 || m != m0) && !self.st.lock().unwrap().iter().any(|e| e.id == id && e.proof) {
            unlock();
            self.kicked.store(true, Ordering::Release);
            return false;
        }
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
        // the land rule at the fast-forward itself: it keeps its record, and what it notes is said
        match self.ruled(r, crate::gate::Verb::Land, &branch, &rule_sides(r, &c, &w, &tm, &m, &tc)) {
            Err(crate::gate::No::Wait(why)) => return wait(why),
            Err(crate::gate::No::Refused(why)) => {
                unlock();
                self.set(id, |e| {
                    e.state = St::Failed;
                    e.detail = why.clone();
                }, Some((Sev::Err, format!("land refused {label}: {}", why[0]))));
                return false;
            }
            Ok((notes, witness)) => {
                if let Some(w) = witness {
                    self.set(id, |e| e.witness = w.clone(), None);
                }
                for note in notes {
                    self.set(id, |_| {}, Some((Sev::Warn, format!("land rule at {label}: {note}"))));
                }
            }
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
            e.job.clear();
            e.pre = format!("work {w} main {m}");
            e.landed = format!("work {tm} main {tc}");
            for (n, h) in e.nested.iter_mut().zip(&halves) {
                n.landed = h.merged.clone();
            }
            e.detail.clear();
        }, Some((if wire_note.is_empty() { Sev::Info } else { Sev::Warn }, format!("land landed {label}: work {tm:.10} main {tc:.10}{nested}{wire_note}"))));
        unlock();
        // the maintenance its writes call for, kept from the lanes' commits (gitlock::own_maintenance)
        for repo in [r, &c].into_iter().chain(halves.iter().map(|h| &h.dir)) { crate::gitlock::maintain(repo); }
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
    let own = crate::cli::own_argv(&crate::lease::wt::default_repo());
    let t = Arc::new(Train {
        machine: machine.to_string(),
        store: crate::state::Store::open_quiet(&home, machine).expect("workflow state must open before services start"),
        repo: crate::lease::wt::default_repo(),
        own,
        publish,
        st: Mutex::new(Vec::new()),
        clock: crate::proto::wall_ms,
        paused: Mutex::new(String::new()),
        kicked: AtomicBool::new(false),
        before_ff: Mutex::new(None),
        rule: crate::gate::land_rule,
        #[cfg(feature = "checkout")]
        members: Mutex::new(None),
        round: Mutex::new(()),
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

    // the pair a hand-in names (only the unix tests, which run git, hand one in)
    #[cfg(unix)]
    const CHECKED: &str = "checked=aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa,bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    #[cfg(unix)]
    fn checked(repo: &Path) -> String {
        format!("checked={},{}", rev(repo, "work").unwrap(), rev(&repo.join("apps/commercial"), "main").unwrap_or_default())
    }

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
        let e = Entry { id: 3, branch: "rc-x".into(), cwt: "/w/x/apps/commercial".into(), cbranch: "rc-x".into(), nested: vec![Side { name: "extra".into(), branch: "rc-x".into(), sha: "c".repeat(40), landed: String::new() }], state: St::Failed, held: String::new(), ms: "a".repeat(40), cs: String::new(), checked: String::new(), touched: Vec::new(), wide: Vec::new(), set: Vec::new(), node: String::new(), job: String::new(), unchecked: String::new(), gate_ms: 0, landed: String::new(), pre: format!("work {} main {}", "a".repeat(40), "b".repeat(40)), detail: vec!["stage: 1 errors".into(), "src/a.rs:3:4: \"quoted\"".into()], agent: "x".into(), proof: true, wire: true, witness: "ports.jsonl=1:aa".into(), asked_ms: 1, changed_ms: 2 };
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

    /// A merge is its parents' clean merge outside its conflicts. One whose tree was merged
    /// against an older tip and committed on the newer one, or that holds the branch's own work
    /// (an amend onto the bot's merge), is refused at hand-in, whatever its message, until the
    /// branch changes those files again; the clean merge has nothing to say (a hand-made merge
    /// reverted another lane's landing, 11-13).
    #[test]
    fn a_merge_that_is_not_its_parents_clean_merge_is_caught() {
        let d = std::env::temp_dir().join(format!("devhub-land-strays-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let g = |args: &[&str]| {
            let (ok, out) = git(&d, args);
            assert!(ok, "git {args:?}: {out}");
            out
        };
        let commit = |file: &str, text: &str, msg: &str| {
            std::fs::write(d.join(file), text).unwrap();
            g(&["add", file]);
            g(&["commit", "-qm", msg]);
            g(&["rev-parse", "HEAD"])
        };
        g(&["init", "-q", "-b", "work"]);
        g(&["config", "user.email", "t@example.com"]);
        g(&["config", "user.name", "t"]);
        commit("a", "1\n", "a");
        commit("b", "1\n", "b");
        g(&["checkout", "-qb", "rc-x"]);
        let x = commit("c", "x\n", "x");
        g(&["checkout", "-q", "work"]);
        let w1 = commit("a", "w1\n", "w1");
        let stale = g(&["merge-tree", "--write-tree", &x, &w1]);
        let w2 = commit("b", "w2\n", "w2: another lane's landing");
        let fresh = g(&["merge-tree", "--write-tree", &x, &w2]);
        let merge = |tree: &str, msg: &str| g(&["commit-tree", tree, "-p", &x, "-p", &w2, "-m", msg]);
        let bot_form = format!("Merge work ({w2:.10}) into rc-x");
        let bad = merge(&stale, &bot_form);
        // each path here is as the one side that changed it has it: read in process, no git
        let spawned = crate::git_exec::on_this_thread();
        assert_eq!(strays(&d, &bad, &x, &w2).unwrap(), vec!["b".to_string()], "the old tip's b reverts the newer tip's");
        assert_eq!(crate::git_exec::on_this_thread(), spawned, "a one-sided merge's check started git");
        let refused = merges_checked(&d, "work", &bad, "rc-x").unwrap_err();
        assert!(refused.contains(&format!("merge {bad:.10} \"{bot_form}\" is not the clean merge")) && refused.contains("changed in b since the merge base"), "{refused}");
        let by_hand = merge(&stale, "merge work by hand");
        assert!(merges_checked(&d, "work", &by_hand, "rc-x").unwrap_err().contains("changed in b since the merge base"), "a lane's own message refuses the same");
        // the clean merge: nothing to say
        let good = merge(&fresh, &bot_form);
        assert_eq!(strays(&d, &good, &x, &w2).unwrap(), Vec::<String>::new());
        assert_eq!(merges_checked(&d, "work", &good, "rc-x").unwrap(), Vec::<String>::new());
        // the branch put b right since: said, not refused
        g(&["checkout", "-q", &bad]);
        let fixed = commit("b", "w2\n", "b as work has it");
        let said = merges_checked(&d, "work", &fixed, "rc-x").unwrap();
        assert!(said.len() == 1 && said[0].contains("in b, which the branch changed again since"), "{said:?}");
        // the branch's own work amended onto the bot's merge (a file neither side changed)
        g(&["checkout", "-q", &x]);
        std::fs::write(d.join("c"), "x amended\n").unwrap();
        g(&["add", "c"]);
        let amended = merge(&g(&["write-tree"]), &bot_form);
        g(&["reset", "-q", "--hard", &x]);
        assert!(merges_checked(&d, "work", &amended, "rc-x").unwrap_err().contains("the branch's own changes to c"));
        // a file both sides changed (git's content merge), a file both deleted, a criss-cross:
        // git's merge decides, and the answer is the same
        g(&["checkout", "-q", "work"]);
        commit("f", "1\n2\n3\n4\n5\n", "f");
        g(&["checkout", "-qb", "rc-y"]);
        let y1 = commit("f", "1y\n2\n3\n4\n5\n", "y");
        g(&["checkout", "-q", "work"]);
        let w4 = commit("f", "1\n2\n3\n4\n5w\n", "w4");
        let on = |tree: &str, p: &str, q: &str| g(&["commit-tree", tree, "-p", p, "-p", q, "-m", "m"]);
        let tree_of = |c: &str| g(&["rev-parse", &format!("{c}^{{tree}}")]);
        let content = on(&g(&["merge-tree", "--write-tree", &y1, &w4]), &y1, &w4);
        let reverted = on(&tree_of(&y1), &y1, &w4);
        let (c1, c2) = (on(&tree_of(&y1), &y1, &w4), on(&tree_of(&w4), &w4, &y1));
        let crossed = on(&tree_of(&c1), &c1, &c2);
        g(&["checkout", "-qb", "rc-z", &w4]);
        g(&["rm", "-q", "a"]);
        g(&["commit", "-qm", "z: a goes"]);
        let z1 = g(&["rev-parse", "HEAD"]);
        g(&["checkout", "-q", "work"]);
        g(&["rm", "-q", "a"]);
        g(&["commit", "-qm", "w5: a goes"]);
        let w5 = g(&["rev-parse", "HEAD"]);
        let gone = on(&tree_of(&z1), &z1, &w5);
        let lane = on(&tree_of(&z1), &z1, &w4);
        for (m, p, q) in [(&content, &y1, &w4), (&reverted, &y1, &w4), (&crossed, &c1, &c2), (&gone, &z1, &w5)] {
            assert!(crate::gitread::one_sided_strays(&d, m, p, q).is_none(), "{m:.10} of {p:.10} and {q:.10} read as one-sided");
        }
        let spawned = crate::git_exec::on_this_thread();
        assert_eq!(strays(&d, &content, &y1, &w4).unwrap(), Vec::<String>::new(), "git's content merge of f");
        assert_eq!(strays(&d, &reverted, &y1, &w4).unwrap(), vec!["f".to_string()], "the lane's f alone drops work's line 5");
        assert_eq!(strays(&d, &gone, &z1, &w5).unwrap(), Vec::<String>::new(), "a deleted on both sides");
        assert!(crate::git_exec::on_this_thread() > spawned, "git's merge did not decide");
        // one side deleted it: in process
        let spawned = crate::git_exec::on_this_thread();
        assert_eq!(strays(&d, &lane, &z1, &w4).unwrap(), Vec::<String>::new());
        assert_eq!(crate::git_exec::on_this_thread(), spawned, "a one-sided deletion started git");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(unix)]
    fn train(home: &Path, repo: &Path, exe: &Path, events: Arc<Mutex<Vec<String>>>) -> Arc<Train> {
        train_ruled(home, repo, exe, events, |_, _, _, _, _, _| Ok(Vec::new()))
    }

    #[cfg(unix)]
    fn train_ruled(home: &Path, repo: &Path, exe: &Path, events: Arc<Mutex<Vec<String>>>, rule: RuleFn) -> Arc<Train> {
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
            rule,
            #[cfg(feature = "checkout")]
            members: Mutex::new(None),
            round: Mutex::new(()),
        })
    }

    /// Commits are objects; only the train's fast-forwards need a checkout.
    #[cfg(unix)]
    struct TrainRepos {
        dir: PathBuf,
        repo: PathBuf,
        home: PathBuf,
        base: String,
        events: Arc<Mutex<Vec<String>>>,
    }

    #[cfg(unix)]
    impl TrainRepos {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("devhub-train-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            let (repo, home) = (dir.join("repo"), dir.join("home"));
            std::fs::create_dir_all(&repo).unwrap();
            std::fs::create_dir_all(&home).unwrap();
            for (r, target) in [(repo.clone(), "work"), (repo.join("apps/commercial"), "main")] {
                std::fs::create_dir_all(&r).unwrap();
                for args in [vec!["init", "-q", "-b", target], vec!["config", "user.email", "t@t"], vec!["config", "user.name", "t"], vec!["config", "commit.gpgsign", "false"]] {
                    assert!(git(&r, &args).0, "fixture repository must initialize: {args:?}");
                }
                let base = Self::commit(&r, &[], "base", "a.rs", "pub struct A {}\n");
                assert!(git(&r, &["update-ref", &format!("refs/heads/{target}"), &base]).0, "fixture tip must be written");
                assert!(git(&r, &["reset", "--hard", "-q", target]).0, "fixture checkout must match its tip");
            }
            let base = rev(&repo, "work").unwrap();
            Self { dir, repo, home, base, events: Arc::new(Mutex::new(Vec::new())) }
        }

        fn commit(repo: &Path, parents: &[&str], name: &str, file: &str, text: &str) -> String {
            use makepad_git::{Commit, ObjectId, Signature, Tree, TreeEntry};
            crate::gitread::with(repo, |_, repo| {
                let mut files = vec![(".gitignore", "apps/commercial\n"), ("a.rs", "pub struct A {}\n"), ("b.rs", "")];
                files.retain(|(p, _)| *p != file);
                files.push((file, text));
                let entries = files.into_iter().map(|(name, text)| TreeEntry { mode: 0o100644, name: name.into(), oid: repo.write_blob(text.as_bytes()).unwrap() }).collect();
                let tree = repo.write_tree(&Tree { entries }).unwrap();
                let author = Signature { name: "t".into(), email: "t@t".into(), timestamp: 1, tz_offset: "+0000".into() };
                repo.write_commit(&Commit { tree, parents: parents.iter().map(|p| ObjectId::from_hex(p).unwrap()).collect(), author: author.clone(), committer: author, message: name.into() }).unwrap().to_hex()
            }).unwrap()
        }

        fn branch(&self, name: &str, file: &str, text: &str) -> String {
            let sha = Self::commit(&self.repo, &[&self.base], name, file, text);
            assert!(git(&self.repo, &["update-ref", &format!("refs/heads/{name}"), &sha]).0, "fixture branch must be written");
            sha
        }

        fn train(&self) -> Arc<Train> {
            train(&self.home, &self.repo, Path::new("/usr/bin/true"), self.events.clone())
        }

        fn add(&self, t: &Train, branch: &str, at: &str, set: &str) -> u64 {
            t.add(&[branch, at, &format!("touched={}", set.split(',').next().unwrap_or("")), &format!("set={set}")]).expect("fixture hand-in must queue");
            t.st.lock().unwrap().iter().find(|e| e.branch == branch && e.state != St::Landed).unwrap().id
        }

        fn entry(t: &Train, id: u64) -> Entry {
            t.st.lock().unwrap().iter().find(|e| e.id == id).unwrap().clone()
        }
    }

    #[cfg(unix)]
    impl Drop for TrainRepos {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.dir); }
    }

    /// A second hand-in of a branch whose first is still queued is refused unless it supersedes
    /// it: two lanes batched two reviewed changes into one landing that way, re-pinned in silence
    /// (11-16). The same commits again are the same hand-in; superseded, it is the same entry at
    /// the new commits; a failed one is handed in again as before.
    #[cfg(unix)]
    #[test]
    fn a_queued_hand_in_is_re_pinned_only_when_superseded() {
        let r = TrainRepos::new("supersede");
        let first = r.branch("rc-s", "s.rs", "pub fn s() {}\n");
        let t = r.train();
        let base = checked(&r.repo);
        let id = r.add(&t, "rc-s", &base, "s");
        let words = |extra: &[&str]| { let mut w = vec!["rc-s", base.as_str(), "touched=s", "set=s"]; w.extend_from_slice(extra); t.add(&w) };
        assert!(words(&[]).is_ok(), "the same commits again are the same hand-in");
        let second = TrainRepos::commit(&r.repo, &[&first], "rc-s second", "s.rs", "pub fn s() { second() }\n");
        assert!(git(&r.repo, &["update-ref", "refs/heads/rc-s", &second]).0);
        let refused = words(&[]).expect_err("a queued hand-in is not re-pinned in silence");
        assert_eq!(refused, format!("#{id} is still queued for this branch at {:.10}; pass --supersede to re-pin it to {:.10}, or wait for it to land", first, second));
        assert_eq!(TrainRepos::entry(&t, id).ms, first, "the queued entry keeps its commits");
        assert!(words(&["supersede"]).is_ok());
        let e = TrainRepos::entry(&t, id);
        assert_eq!((e.ms, e.state), (second.clone(), St::Queued), "superseded: the same entry at the new commits");
        t.set(id, |e| e.state = St::Failed, None);
        let third = TrainRepos::commit(&r.repo, &[&second], "rc-s third", "s.rs", "pub fn s() { third() }\n");
        assert!(git(&r.repo, &["update-ref", "refs/heads/rc-s", &third]).0);
        assert!(words(&[]).is_ok(), "a failed hand-in is handed in again as before");
        assert_eq!(TrainRepos::entry(&t, id).ms, third);
    }

    /// Nothing re-checks a hand-in at its merge, and nothing waits for a check: each lands as
    /// soon as it merges, its check carried to the tips it lands on, and the crates of its set the
    /// landings since its check reach as well are named for the post-land buckets, which check
    /// them on the tip (user, 11-16: "just land them and keep them checking async"; one re-check
    /// of 305 crates held 19 hand-ins). A tip no landing explains, an unchecked hand-in and one a
    /// restarted master left `Checking` name their whole set.
    #[cfg(unix)]
    #[test]
    fn hand_ins_land_at_their_merge_with_their_check_carried() {
        let r = TrainRepos::new("carried");
        let a = r.branch("rc-a", "a.rs", "pub struct A { pub value: u32 }\n");
        let b = r.branch("rc-b", "b.rs", "pub fn b() -> A { A {} }\n");
        r.branch("rc-c", "c.rs", "pub fn c() {}\n");
        r.branch("rc-unchecked", "d.rs", "pub fn d() {}\n");
        r.branch("rc-restart", "e.rs", "pub fn e() {}\n");
        let t = r.train();
        let base = checked(&r.repo);
        let aid = r.add(&t, "rc-a", &base, "a,b");
        let bid = r.add(&t, "rc-b", &base, "b,x");
        let cid = r.add(&t, "rc-c", &base, "c");
        let unchecked = r.add(&t, "rc-unchecked", "unchecked=no-graph", "d,y");
        let restart = r.add(&t, "rc-restart", &base, "e,b");
        t.set(restart, |e| { e.state = St::Checking; e.job = "re-check at work before restart".into(); }, None);
        assert!(t.start_ready(), "the round lands");
        let entries: Vec<Entry> = [aid, bid, cid, unchecked, restart].into_iter().map(|id| TrainRepos::entry(&t, id)).collect();
        assert!(entries.iter().all(|e| e.state == St::Landed && e.job.is_empty()), "every hand-in that merges lands in the one round, none held or re-checked: {entries:?}");
        for e in &entries {
            let pre = pair_sides(&e.pre).unwrap();
            assert_eq!(e.checked, format!("{},{}", pre.0, pre.1), "#{}'s check is carried to the tips it landed on", e.id);
        }
        let work = rev(&r.repo, "work").unwrap();
        assert!(is_ancestor(&r.repo, &a, &work) && is_ancestor(&r.repo, &b, &work), "work holds A and B: B's literal is the post-land buckets' to catch, fixed forward");
        let events = r.events.lock().unwrap().clone();
        let named = |label: &str| events.iter().find(|l| l.starts_with(&format!("land {label}: lands with its check carried"))).cloned();
        assert!(named("rc-b").is_some_and(|l| l.ends_with("the post-land buckets check b on the tip")), "B names only the crate A's landing reaches as well: {events:?}");
        assert!(named("rc-a").is_none() && named("rc-c").is_none(), "a hand-in checked at the tips it lands on, or met by no landing since, names nothing: {events:?}");
        assert!(named("rc-unchecked").is_some_and(|l| l.ends_with("check d, y on the tip")), "an unchecked hand-in names its whole set: {events:?}");
        assert!(named("rc-restart").is_some_and(|l| l.ends_with("check b on the tip")), "one a restarted master left Checking lands, naming what the landings since reach: {events:?}");
        assert!(!events.iter().any(|l| l.contains("re-check")), "nothing re-checks: {events:?}");
    }

    /// The land rule decides each landing over both repositories: a refusal fails it, a rule not
    /// built yet holds it, and what the rule notes at the landing is said.
    #[cfg(unix)]
    #[test]
    fn the_land_rule_refuses_holds_or_notes_a_landing_over_both_repositories() {
        let d = std::env::temp_dir().join(format!("devhub-land-rule-{}", std::process::id()));
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
        let branch = |dir: &Path, b: &str, file: &str| {
            g(dir, &["checkout", "-qb", b, if dir == r.as_path() { "work" } else { "main" }]);
            std::fs::write(dir.join(file), "x\n").unwrap();
            g(dir, &["add", "."]);
            g(dir, &["commit", "-qm", b]);
        };
        branch(&r, "rc-ok", "ok");
        branch(&c, "rc-ok", "cok");
        branch(&r, "rc-no", "no");
        branch(&r, "rc-later", "later");
        branch(&r, "rc-hard", "AGENTS.md");
        g(&r, &["checkout", "-q", "work"]);
        g(&c, &["checkout", "-q", "main"]);
        // the witnesses the rule was handed, in order
        static HANDED: Mutex<Vec<String>> = Mutex::new(Vec::new());
        fn rule(_: &Path, verb: crate::gate::Verb, branch: &str, sides: &[crate::gate::Side], witness: &str, before: &str) -> Result<Vec<String>, crate::gate::No> {
            HANDED.lock().unwrap().push(format!("{branch} {witness}{}", if before.is_empty() { String::new() } else { format!(" before {before}") }));
            let names: Vec<&str> = sides.iter().map(|s| s.name).collect();
            assert_eq!(names, ["makepad", "commercial"], "the rule sees both repositories");
            assert!(sides.iter().all(|s| s.from != s.merge || s.name == "commercial"), "each from its branch's tip to the merge");
            match branch {
                "rc-no" => Err(crate::gate::No::Refused(vec!["the rule says no".into()])),
                "rc-hard" => Err(crate::gate::No::Refused(vec!["AGENTS.md is T1-hard".into()])),
                "rc-later" => Err(crate::gate::No::Wait("not built yet".into())),
                _ if verb == crate::gate::Verb::Land => Ok(vec!["noted".into(), format!("witness\tafter {branch}")]),
                _ => Ok(Vec::new()),
            }
        }
        let events = Arc::new(Mutex::new(Vec::new()));
        let t = train_ruled(&home, &r, Path::new("/usr/bin/true"), events.clone(), rule);
        let ask = |line: &str| {
            let mut out = Vec::new();
            let line = if line.starts_with("add ") { format!("{line} {}", checked(&r)) } else { line.to_string() };
            t.serve(&line, &mut out);
            out
        };
        assert!(ask(&format!("add rc-ok {} rc-ok", c.display()))[0].contains("\"id\":1"));
        assert!(ask("add rc-no")[0].contains("\"id\":2"));
        assert!(ask("add rc-later")[0].contains("\"id\":3"));
        t.start_ready();
        let st = |b: &str| Entry::parse(&ask(&format!("status {b}"))[0]).unwrap();
        assert_eq!(st("rc-ok").state, St::Landed, "{:?}", st("rc-ok"));
        assert!(c.join("cok").exists(), "with its commercial half");
        assert!(events.lock().unwrap().iter().any(|l| l.contains("land rule at") && l.contains("noted")), "the rule's note is said");
        assert!(!events.lock().unwrap().iter().any(|l| l.contains("witness")), "its witness is no note");
        assert_eq!((st("rc-ok").witness.as_str(), t.store().read(|d| d.land_witness.clone())), ("after rc-ok", "after rc-ok".to_string()), "the landing's entry and the master keep its witness");
        assert_eq!(HANDED.lock().unwrap().iter().filter(|h| h.starts_with("rc-ok ")).cloned().collect::<Vec<_>>(), ["rc-ok ", "rc-ok "], "the first landing had none to hand on");
        assert!(HANDED.lock().unwrap().iter().any(|h| h == "rc-no after rc-ok" || h == "rc-later after rc-ok"), "a later run gets it: {:?}", HANDED.lock().unwrap());
        assert_eq!((st("rc-no").state, st("rc-no").detail.clone()), (St::Failed, vec!["the rule says no".to_string()]), "a refusal fails the landing");
        assert_eq!((st("rc-later").state, st("rc-later").detail.clone()), (St::Waiting, vec!["not built yet".to_string()]), "a rule not built yet holds it");
        assert!(!r.join("no").exists() && !r.join("later").exists(), "neither landed");
        // switched off, no landing runs the rule: one the rule refuses lands, saying so
        assert_eq!(ask("rule"), vec!["{\"rule\":\"on\",\"why\":\"\"}".to_string()], "the rule is on until switched off");
        // switching it off is the user's, at a terminal: an agent's identity, or no terminal, is
        // refused before the master is asked, and the rule stays on; on is anyone's
        let line = |rest: &[&str], names: &[&str], terminal: bool| crate::cli::rule_line(rest, || crate::cli::user_at(names.iter().map(|n| n.to_string()), terminal));
        for (names, terminal) in [(&["HOME", "DEVHUB_AGENT"][..], true), (&["CLAUDECODE"][..], true), (&["CODEX_HOME"][..], true), (&["HOME"][..], false)] {
            let refused = line(&["off", "a", "reason"], names, terminal).expect_err("an agent, or no terminal, cannot switch the rule off");
            assert!(refused.starts_with("refused"), "{names:?} terminal={terminal}: {refused}");
        }
        assert_eq!(ask("rule"), vec!["{\"rule\":\"on\",\"why\":\"\"}".to_string()], "a refused off leaves the rule on");
        assert_eq!(line(&["on"], &["DEVHUB_AGENT"], false), Ok("rule on".to_string()), "switching it on adds protection: anyone may");
        let off = line(&["off", "user", "11-15"], &["HOME"], true).expect("the user at a terminal switches it off");
        assert_eq!(ask(&off), vec!["{\"rule\":\"off\",\"why\":\"user 11-15\"}".to_string()]);
        assert_eq!(t.store().read(|d| d.land_witness.clone()), "", "the user's off drops the witness: the next landing after on starts a fresh one");
        assert_eq!(train_ruled(&home, &r, Path::new("/usr/bin/true"), events.clone(), rule).rule_off().as_deref(), Some("user 11-15"), "the switch outlives a master restart");
        assert!(ask("add rc-hard")[0].contains("\"id\":4"));
        t.start_ready();
        assert_eq!(st("rc-hard").state, St::Landed, "off, a landing the rule refuses lands: {:?}", st("rc-hard"));
        assert!(r.join("AGENTS.md").exists(), "its T1-hard path landed");
        assert!(events.lock().unwrap().iter().any(|l| l == "land rule at rc-hard: land rule off (user 11-15): nothing checked"), "the landing says the rule checked nothing: {:?}", events.lock().unwrap());
        // switched on, it runs again: its refusal fails the landing as before
        assert_eq!(ask("rule on"), vec!["{\"rule\":\"on\",\"why\":\"\"}".to_string()]);
        assert!(ask("retry rc-no")[0].contains("ok"));
        t.start_ready();
        assert_eq!((st("rc-no").state, st("rc-no").detail.clone()), (St::Failed, vec!["the rule says no".to_string()]), "on, the rule refuses again");
        assert!(!r.join("no").exists(), "nothing of it landed");
        assert_eq!(HANDED.lock().unwrap().last().map(String::as_str), Some("rc-no  before after rc-ok"), "with no witness since the off, the last landed entry's goes with it, for the rule to say what was lost");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A hand-in records its checked scope, including an explicit lack of a verdict.
    #[cfg(unix)]
    #[test]
    fn a_hand_in_names_the_pair_it_was_checked_at() {
        let d = std::env::temp_dir().join(format!("devhub-land-checked-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (r, home) = (d.join("repo"), d.join("home"));
        std::fs::create_dir_all(&r).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let g = |args: &[&str]| assert!(git(&r, args).0, "fixture git {args:?} succeeds");
        g(&["init", "-q", "-b", "work"]);
        g(&["config", "user.name", "t"]);
        g(&["config", "user.email", "t@t"]);
        g(&["config", "commit.gpgsign", "false"]);
        std::fs::write(r.join("ci.toml"), "# base\n").unwrap();
        g(&["add", "."]);
        g(&["commit", "-qm", "base"]);
        g(&["checkout", "-qb", "rc-checked"]);
        std::fs::write(r.join("ci.toml"), "# lane\n").unwrap();
        g(&["commit", "-qam", "lane"]);
        g(&["branch", "rc-proof"]);
        g(&["branch", "rc-unchecked"]);
        let events = Arc::new(Mutex::new(Vec::new()));
        let t = train(&home, &r, Path::new("/usr/bin/true"), events.clone());
        let why = t.add(&["rc-checked"]).expect_err("an old client must be refused");
        assert_eq!(why, "the hand-in names no checked pair (checked=): hand in with a current devhub", "the refusal explains the required client");
        assert!(t.st.lock().unwrap().is_empty(), "a missing pair queues nothing");
        t.add(&["rc-proof", "proof"]).expect("a proof hand-in needs no checked pair");
        assert!(t.st.lock().unwrap()[0].proof, "the proof entry is queued");
        t.add(&["rc-unchecked", "unchecked=no-lane"]).expect("an explicit unchecked reason is accepted");
        let unchecked = t.st.lock().unwrap()[1].clone();
        assert_eq!((unchecked.checked.as_str(), unchecked.unchecked.as_str()), ("", "no-lane"), "unchecked entries have no checked pair");
        let pair = CHECKED.strip_prefix("checked=").unwrap();
        for word in ["checked=bad", &format!("checked={pair},extra"), "touched=a,", "wide=a/b", "set=a,,b", "set=a.b", "gate_ms=soon"] {
            let words = if word.starts_with("checked=") { vec!["rc-checked", word] } else { vec!["rc-checked", CHECKED, word] };
            let why = t.add(&words).expect_err("malformed gate metadata is refused");
            assert!(why.contains(word), "the refusal names the malformed word {word}: {why}");
        }
        assert_eq!(t.st.lock().unwrap().len(), 2, "malformed words queue nothing");
        t.add(&["rc-checked", CHECKED, "touched=a", "wide=a", "set=a,b", "node=n1", "gate_ms=41250"]).expect("a checked hand-in queues");
        let e = t.st.lock().unwrap()[2].clone();
        assert_eq!(e.checked, pair, "the checked pair is kept intact");
        assert_eq!((&e.touched, &e.wide, &e.set), (&vec!["a".to_string()], &vec!["a".to_string()], &vec!["a".to_string(), "b".to_string()]), "the touched, wide and reached packages are recorded separately");
        assert_eq!((e.node.as_str(), e.unchecked.as_str(), e.gate_ms), ("n1", "", 41250), "a checked entry records its node, no unchecked reason, and how long its gates took (`land status --json`)");
        assert_eq!(Entry::parse(&e.to_json()), Some(e.clone()), "all gate fields survive JSON");
        let reloaded = train(&home, &r, Path::new("/usr/bin/true"), events);
        reloaded.load();
        assert_eq!(reloaded.st.lock().unwrap()[2], e, "all gate fields survive a store reload");
        let mut old = e.to_json();
        for (key, value) in [("checked", format!("\"{pair}\"")), ("touched", "[\"a\"]".into()), ("wide", "[\"a\"]".into()), ("set", "[\"a\",\"b\"]".into()), ("node", "\"n1\"".into()), ("unchecked", "\"\"".into()), ("gate_ms", "41250".into())] {
            old = old.replace(&format!("\"{key}\":{value},"), "");
        }
        let old = Entry::parse(&old).expect("entries saved before the gate still parse");
        assert!(old.checked.is_empty() && old.touched.is_empty() && old.wide.is_empty() && old.set.is_empty() && old.node.is_empty() && old.unchecked.is_empty() && old.gate_ms == 0, "missing gate fields default to empty");
        let (_, main) = pair.split_once(',').unwrap();
        let next = format!("checked={main},");
        t.add(&["rc-checked", &next, "touched=b", "wide=", "set=b", "node=n2", "gate_ms=900"]).expect("a repeated hand-in replaces its gate");
        let again = t.st.lock().unwrap()[2].clone();
        assert_eq!((again.id, again.checked.as_str(), again.node.as_str()), (e.id, &next[8..], "n2"), "the open entry keeps its identity and replaces its pair and node");
        assert_eq!(again.gate_ms, 900, "a repeated hand-in says its own gates' time, not the first one's");
        assert_eq!((&again.touched, &again.wide, &again.set), (&vec!["b".to_string()], &Vec::<String>::new(), &vec!["b".to_string()]), "a repeated hand-in replaces each list, including an empty list");
        t.add(&["rc-checked", "unchecked=no-lane"]).expect("a later unchecked hand-in replaces the checked one");
        let again = t.st.lock().unwrap()[2].clone();
        assert!(again.checked.is_empty() && again.touched.is_empty() && again.wide.is_empty() && again.set.is_empty() && again.node.is_empty() && again.gate_ms == 0, "omitted gate fields never retain an earlier hand-in's proof");
        assert_eq!(again.unchecked, "no-lane", "a repeated hand-in replaces the unchecked reason too");
        let _ = std::fs::remove_dir_all(&d);
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
        let (c, x, a) = (r.join("apps/commercial"), r.join("apps/extra"), r.join("apps/apps"));
        for dir in [&c, &x, &a] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::create_dir_all(&home).unwrap();
        let g = |dir: &Path, args: &[&str]| assert!(git(dir, args).0, "git {args:?} in {}", dir.display());
        for (dir, main) in [(&r, "work"), (&c, "main"), (&x, "trunk"), (&a, "main")] {
            g(dir, &["init", "-q", "-b", main]);
            g(dir, &["config", "user.email", "t@t"]);
            g(dir, &["config", "user.name", "t"]);
        }
        std::fs::write(r.join(".gitignore"), "apps/commercial\napps/extra\napps/apps\n").unwrap();
        for dir in [&r, &c, &x, &a] {
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
        // rc-n: makepad + its extra half; rc-o: extra alone; rc-k: extra conflicts with trunk; rc-p:
        // a home path in the public apps repository, refused as in makepad. A private repository's
        // history is its own (rc-o's home path lands).
        let home_path = format!("/{}/somebody/x\n", "home");
        branch(&r, "rc-n", "n", "n\n", "work");
        branch(&x, "rc-n", "xn", "n\n", "trunk");
        branch(&x, "rc-o", "xo", &home_path, "trunk");
        branch(&a, "rc-p", "pa", &home_path, "main");
        g(&a, &["checkout", "-q", "main"]);
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
            let line = if line.starts_with("add ") { format!("{line} {}", checked(&r)) } else { line.to_string() };
            t.serve(&line, &mut out);
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
        assert!(ask("add - apps=rc-p")[0].contains("\"id\":4"));
        assert!(t.start_ready());
        let st = |b: &str| Entry::parse(&ask(&format!("status {b}"))[0]).unwrap();
        let (n, o, k) = (st("rc-n+extra=rc-n"), st("-+extra=rc-o"), st("-+extra=rc-k"));
        // (the train's events say why an entry stayed: its round stopped, its change did not save;
        // one red here, 11-08, left its second entry queued untouched with nothing said)
        assert_eq!((n.state, o.state), (St::Landed, St::Landed), "{n:?} {o:?}; the train said: {:?}", events.lock().unwrap());
        assert_eq!((k.state, k.detail.clone()), (St::Conflict, vec!["apps/extra/base".to_string()]));
        let p = st("-+apps=rc-p");
        assert!(p.state == St::Conflict && p.detail.iter().any(|l| l.starts_with("apps: ") && l.contains("home-path")), "the public half's private data is refused: {p:?}");
        assert!(!a.join("pa").exists() && rev(&a, "refs/heads/main") != rev(&a, "rc-p"), "apps' main stays as it was");
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
    /// the studio crate's `WIRE` and of key_code's `SOURCE_FINGERPRINT`. [`AI_BUS_SOURCES`] is every
    /// file that defines an AI bus frame: `wire.rs` names no type of another module, and the
    /// [`AI_BUS_TRANSPORTS`] derive no codec and write no envelope of their own.
    /// [`ENCODING_SOURCES`] is every module of micro_serde and its derive but RON's.
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

        // a file's code before its test modules
        let code = |text: &'static str| &text[..text.find("\n#[cfg(test)]").unwrap_or(text.len())];
        let wire = include_str!("../../ai/services/src/wire.rs");
        assert_eq!(AI_BUS_SOURCES, ["libs/ai/services/src/wire.rs"]);
        for line in code(wire).lines().filter(|l| !l.trim_start().starts_with("//")) {
            let imports = line.starts_with("use ") || line.starts_with("pub use ");
            assert!(
                !(imports && !line.starts_with("use makepad_micro_serde::") && !line.starts_with("use std::"))
                    && !line.contains("crate::")
                    && !line.contains("super::"),
                "wire.rs names a type of another module, so a frame may be defined outside the file the land gate watches: {line}"
            );
        }
        let transports = [
            include_str!("../../ai/services/src/port.rs"),
            include_str!("../../ai/services/src/app_service.rs"),
            include_str!("../../../apps/wm/src/ai_bus.rs"),
            include_str!("../../../apps/aichat/src/bus.rs"),
        ];
        for (file, text) in AI_BUS_TRANSPORTS.iter().zip(transports) {
            for codec in ["SerJson", "DeJson", "SerBin", "DeBin", "HOSTED_KEY", "\"wm_ai\"", "wm_ai:"] {
                assert!(
                    !code(text).contains(codec),
                    "{file} carries the AI bus's frames and lands without the wire-change mark, so it may define none: `{codec}` belongs in wire.rs"
                );
            }
        }

        // the modules a crate root declares, but RON's, which writes no frame
        let modules = |text: &str, dir: &str| -> Vec<String> {
            text.lines()
                .filter_map(|l| l.trim().strip_prefix("mod ")?.strip_suffix(';'))
                .filter(|m| !m.ends_with("ron"))
                .map(|m| format!("{dir}/{m}.rs"))
                .collect()
        };
        let mut encoding = modules(include_str!("../../micro_serde/src/lib.rs"), "libs/micro_serde/src");
        encoding.extend(modules(include_str!("../../micro_serde/derive_impl/src/lib.rs"), "libs/micro_serde/derive_impl/src"));
        assert_eq!(encoding, ENCODING_SOURCES, "micro_serde's encoding modules and ENCODING_SOURCES, in that order: a module that writes a frame lands only marked");
    }

    /// A hand-in that changes a file of the hosted wire fails, naming the files and the mark; the
    /// same hand-in marked `wire-change` lands and says it changes the wire. Other files of the
    /// same crate land as ever, and so does an AI bus transport.
    #[cfg(unix)]
    #[test]
    fn a_change_of_the_hosted_wire_lands_only_marked() {
        let d = std::env::temp_dir().join(format!("devhub-land-wire-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (r, home) = (d.join("repo"), d.join("home"));
        let c = r.join("apps/commercial");
        std::fs::create_dir_all(r.join("platform/studio/src")).unwrap();
        std::fs::create_dir_all(r.join("libs/ai/services/src")).unwrap();
        std::fs::create_dir_all(r.join("libs/micro_serde/src")).unwrap();
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
        std::fs::write(r.join("libs/ai/services/src/wire.rs"), "pub struct ServiceCall;\n").unwrap();
        std::fs::write(r.join("libs/ai/services/src/port.rs"), "pub struct AiServicePort;\n").unwrap();
        std::fs::write(r.join("libs/micro_serde/src/serde_bin.rs"), "pub trait SerBin {}\n").unwrap();
        g(&r, &["add", "."]);
        g(&r, &["commit", "-qm", "base"]);
        std::fs::write(c.join("s"), "1\n").unwrap();
        g(&c, &["add", "."]);
        g(&c, &["commit", "-qm", "cbase"]);
        for (b, file) in [
            ("rc-wire", "platform/studio/src/studio.rs"),
            ("rc-plain", "platform/studio/src/other.rs"),
            ("rc-bus", "libs/ai/services/src/wire.rs"),
            ("rc-port", "libs/ai/services/src/port.rs"),
            ("rc-codec", "libs/micro_serde/src/serde_bin.rs"),
        ] {
            g(&r, &["checkout", "-qb", b, "work"]);
            std::fs::write(r.join(file), "// a comment\n").unwrap();
            g(&r, &["commit", "-qam", b]);
        }
        g(&r, &["checkout", "-q", "work"]);
        let events = Arc::new(Mutex::new(Vec::new()));
        let t = train(&home, &r, Path::new("/usr/bin/true"), events.clone());
        let ask = |line: &str| {
            let mut out = Vec::new();
            let line = if line.starts_with("add ") { format!("{line} {}", checked(&r)) } else { line.to_string() };
            t.serve(&line, &mut out);
            out
        };
        let st = |b: &str| Entry::parse(&ask(&format!("status {b}"))[0]).unwrap();
        assert!(ask("add rc-wire")[0].contains("\"id\":1"));
        assert!(ask("add rc-plain")[0].contains("\"id\":2"));
        assert!(ask("add rc-bus")[0].contains("\"id\":3"));
        assert!(ask("add rc-port")[0].contains("\"id\":4"));
        assert!(ask("add rc-codec")[0].contains("\"id\":5"));
        t.start_ready();
        let (wire, plain, bus, port, codec) = (st("rc-wire"), st("rc-plain"), st("rc-bus"), st("rc-port"), st("rc-codec"));
        assert_eq!((wire.state, wire.detail[0].as_str()), (St::Failed, "it changes the hosted wire: platform/studio/src/studio.rs"), "{wire:?}");
        assert!(wire.detail.iter().any(|l| l.contains("makepad devhub land --wire-change")), "{wire:?}");
        // the AI bus's wire has no fingerprint: an unmarked change of it fails the same way, saying so
        assert_eq!((bus.state, bus.detail[0].as_str()), (St::Failed, "it changes the hosted wire: libs/ai/services/src/wire.rs"), "{bus:?}");
        assert!(bus.detail.iter().any(|l| l.contains("drop each other's tool calls")) && !bus.detail.iter().any(|l| l.contains("wire fingerprint")), "{bus:?}");
        // micro_serde's encoding writes both wires' frames and no fingerprint hashes it: the same
        assert_eq!((codec.state, codec.detail[0].as_str()), (St::Failed, "it changes the hosted wire: libs/micro_serde/src/serde_bin.rs"), "{codec:?}");
        assert!(codec.detail.iter().any(|l| l.contains("misread each other's frames")) && !codec.detail.iter().any(|l| l.contains("wire fingerprint")), "{codec:?}");
        assert_eq!(plain.state, St::Landed, "{plain:?}");
        // a transport of the AI bus defines no frame: its edit lands unmarked
        assert_eq!(port.state, St::Landed, "{port:?}");
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

    /// A hand-in that carries the very bytes its lane's live session saw fail to compile is told so
    /// in its reply, and is queued all the same; one whose commit carries other bytes there (the
    /// fix) hears nothing.
    #[cfg(unix)]
    #[test]
    fn a_hand_in_hears_of_its_lanes_open_live_compile_error() {
        let d = std::env::temp_dir().join(format!("devhub-land-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (r, home) = (d.join("repo"), d.join("home"));
        std::fs::create_dir_all(r.join("app/src")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let g = |args: &[&str]| assert!(git(&r, args).0, "git {args:?}");
        g(&["init", "-q", "-b", "work"]);
        g(&["config", "user.email", "t@t"]);
        g(&["config", "user.name", "t"]);
        std::fs::write(r.join("app/src/a.rs"), "fn a() {}\n").unwrap();
        g(&["add", "."]);
        g(&["commit", "-qm", "base"]);
        let broken = "fn a() { let r; { let x = 1; r = &x; } }\n";
        g(&["checkout", "-qb", "rc-x"]);
        std::fs::write(r.join("app/src/a.rs"), broken).unwrap();
        g(&["commit", "-qam", "x"]);
        g(&["checkout", "-q", "work"]);
        let t = train(&home, &r, Path::new("/usr/bin/true"), Arc::new(Mutex::new(Vec::new())));
        let sha: String = makepad_network::digest::sha256_hash(broken.as_bytes()).iter().map(|b| format!("{b:02x}")).collect();
        t.store().update(|d| {
            d.lanes.push(crate::state::LaneRec { name: "x".into(), path: "/nowhere/x".into(), branch: "rc-x".into(), state: "live".into(), ..Default::default() });
            d.lives.push(crate::state::Live {
                id: "W1".into(), lane: "x".into(), state: "running".into(),
                sources: [("app/src/a.rs".to_string(), format!("false:false:{sha}"))].into(),
                patch: Some(crate::state::LivePatch { files: vec!["app/src/a.rs".into()], outcome: "compile_error".into(), reason: "error[E0597]: `x` does not live long enough\n --> app/src/a.rs:1:30".into(), ..Default::default() }),
                ..Default::default()
            });
        }).unwrap();
        let ask = |line: &str| {
            let mut out = Vec::new();
            let line = if line.starts_with("add ") { format!("{line} {}", checked(&r)) } else { line.to_string() };
            t.serve(&line, &mut out);
            out.join("\n")
        };
        let told = ask("add rc-x");
        assert!(told.contains("\"id\":1"), "queued all the same: {told}");
        assert!(told.contains("live session W1 of lane x: its last save of app/src/a.rs did not compile (app/src/a.rs:1:30: error[E0597]: `x` does not live long enough), and this hand-in carries those bytes"), "{told}");
        // the fix committed: other bytes than the ones that failed
        g(&["checkout", "-q", "rc-x"]);
        std::fs::write(r.join("app/src/a.rs"), "fn a() { let x = 1; let _r = &x; }\n").unwrap();
        g(&["commit", "-qam", "fix"]);
        g(&["checkout", "-q", "work"]);
        let told = ask("add rc-x");
        assert!(!told.contains("live session"), "{told}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A lane with unlanded work on both halves must name both, even if its worktree is gone.
    /// A half already in its target does not force a paired hand-in. A lane worktree on a
    /// detached HEAD holding commits its branch lacks refuses the lane's hand-ins, either half.
    #[cfg(unix)]
    #[test]
    fn a_hand_in_that_would_leave_lane_commits_behind_is_refused() {
        let d = std::env::temp_dir().join(format!("devhub-land-one-half-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (r, home) = (d.join("repo"), d.join("home"));
        let c = r.join("apps/commercial");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let g = |dir: &Path, args: &[&str]| assert!(git(dir, args).0, "fixture git {args:?} in {} succeeds", dir.display());
        for (dir, target) in [(&r, "work"), (&c, "main")] {
            g(dir, &["init", "-q", "-b", target]);
            g(dir, &["config", "user.email", "t@t"]);
            g(dir, &["config", "user.name", "t"]);
            g(dir, &["config", "commit.gpgsign", "false"]);
        }
        std::fs::write(r.join(".gitignore"), "apps/commercial\n").unwrap();
        let commit = |dir: &Path, file: &str, text: &str, msg: &str| {
            std::fs::write(dir.join(file), text).unwrap();
            g(dir, &["add", "."]);
            g(dir, &["commit", "-qm", msg]);
            rev(dir, "HEAD").unwrap()
        };
        let w0 = commit(&r, "base", "base\n", "base");
        let m0 = commit(&c, "base", "cbase\n", "cbase");
        g(&r, &["checkout", "-qb", "rc-half"]);
        let ms = commit(&r, "makepad", "makepad\n", "makepad half");
        g(&c, &["checkout", "-qb", "rc-half-c"]);
        let cs = commit(&c, "commercial", "commercial\n", "commercial half");
        g(&r, &["checkout", "-q", "work"]);
        g(&c, &["checkout", "-q", "main"]);
        let t = train(&home, &r, Path::new("/usr/bin/true"), Arc::new(Mutex::new(Vec::new())));
        t.store().update(|d| {
            d.lanes.push(crate::state::LaneRec { name: "half".into(), path: r.display().to_string(), branch: "rc-half".into(), cbranch: "rc-half-c".into(), state: "live".into(), ..Default::default() });
        }).unwrap();
        let ask = |line: &str| {
            let mut out = Vec::new();
            let line = if line.starts_with("add ") { format!("{line} {}", checked(&r)) } else { line.to_string() };
            t.serve(&line, &mut out);
            out
        };
        let both = format!("makepad devhub land rc-half {} rc-half-c", c.display());
        let missing = d.join("missing");
        for path in [&r, &missing] {
            t.store().update(|d| d.lanes[0].path = path.display().to_string()).unwrap();
            let refused = ask("add rc-half");
            assert!(refused[0].contains("\"err\"") && refused[0].contains("has commits on rc-half-c in commercial too"), "makepad alone must be refused and name its commercial half, with lane path {}: {refused:?}", path.display());
            assert!(refused[0].contains(&both), "the makepad refusal must give the two-half command: {refused:?}");
            let refused = ask(&format!("add - {} rc-half-c", c.display()));
            assert!(refused[0].contains("\"err\"") && refused[0].contains("has commits on rc-half in makepad too"), "commercial alone must be refused and name its makepad half, with lane path {}: {refused:?}", path.display());
            assert!(refused[0].contains(&both), "the commercial refusal must give the two-half command: {refused:?}");
            assert!(t.st.lock().unwrap().is_empty(), "both refusals must leave the queue empty");
            assert!(!t.start_ready(), "a train round after the refusals must land nothing");
            assert_eq!((rev(&r, "refs/heads/work").unwrap(), rev(&c, "refs/heads/main").unwrap()), (w0.clone(), m0.clone()), "neither target may move after the refused hand-ins");
        }
        let added = ask(&format!("add rc-half {} rc-half-c", c.display()));
        assert!(added[0].contains("\"id\":1"), "naming both halves must queue the first entry: {added:?}");
        let e = t.st.lock().unwrap()[0].clone();
        assert_eq!((e.state, e.ms, e.cs), (St::Queued, ms.clone(), cs.clone()), "the paired hand-in must pin both halves");
        assert!(t.start_ready(), "the explicitly paired hand-in must land");
        assert_eq!(t.st.lock().unwrap()[0].state, St::Landed, "the paired entry must record its landing");
        assert!(is_ancestor(&r, &ms, &rev(&r, "refs/heads/work").unwrap()), "work must hold the named makepad half");
        assert!(is_ancestor(&c, &cs, &rev(&c, "refs/heads/main").unwrap()), "main must hold the named commercial half");
        // the same lane has new makepad work, but its commercial half is already in main
        g(&r, &["checkout", "-q", "rc-half"]);
        let next = commit(&r, "next", "next\n", "next makepad half");
        g(&r, &["checkout", "-q", "work"]);
        let added = ask("add rc-half");
        assert!(added[0].contains("\"id\":2"), "an already-landed commercial half must allow makepad alone: {added:?}");
        let e = t.st.lock().unwrap()[1].clone();
        assert_eq!((e.state, e.ms), (St::Queued, next), "the new makepad half must be queued");
        assert!(e.cwt.is_empty() && e.cbranch.is_empty() && e.cs.is_empty(), "the omitted commercial half must stay out of the entry");
        // the commercial worktree switched to its branch's commit and committed on there: a
        // hand-in of the branch landed nothing without a word (#6597, 11-18)
        t.store().update(|d| d.lanes[0].path = r.display().to_string()).unwrap();
        g(&c, &["checkout", "-q", "--detach", "rc-half-c"]);
        let off = commit(&c, "off", "off\n", "made on a detached HEAD");
        for line in ["add rc-half".to_string(), format!("add rc-half {} rc-half-c", c.display())] {
            let refused = ask(&line);
            assert!(refused[0].contains("\"err\"") && refused[0].contains(&format!("lane half's commercial worktree {} is on a detached HEAD at {off:.10} holding 1 commit its branch rc-half-c lacks", c.display())), "`{line}` must be refused, naming the detached half: {refused:?}");
            assert!(refused[0].contains(&format!("`git -C {} checkout -B rc-half-c` puts it on the branch", c.display())), "the refusal must say how to put the commit on the branch: {refused:?}");
        }
        g(&c, &["checkout", "-q", "-B", "rc-half-c"]);
        // the makepad half the same, one that also lost a commit its branch has since taken
        g(&r, &["checkout", "-q", "--detach", "rc-half"]);
        let moff = commit(&r, "moff", "moff\n", "made on a detached HEAD");
        g(&r, &["branch", "-q", "-f", "rc-half", "work"]);
        let both_cmd = format!("add rc-half {} rc-half-c", c.display());
        let refused = ask(&both_cmd);
        assert!(refused[0].contains("\"err\"") && refused[0].contains(&format!("lane half's makepad worktree {} is on a detached HEAD at {moff:.10} holding 2 commits its branch rc-half lacks", r.display())), "the detached makepad half must refuse the hand-in: {refused:?}");
        assert!(refused[0].contains(&format!("merge them into rc-half (`git -C {} checkout rc-half && git merge {moff:.10}`)", r.display())), "a branch that is not behind the detached commits must be told to merge them: {refused:?}");
        assert_eq!(t.st.lock().unwrap().len(), 2, "the refused hand-ins must queue nothing");
        // a detached HEAD at a commit its branch holds leaves nothing behind
        g(&r, &["checkout", "-q", "-B", "rc-half", &moff]);
        g(&r, &["checkout", "-q", "--detach", "rc-half"]);
        let added = ask(&both_cmd);
        assert!(added[0].contains("\"id\":3"), "a detached HEAD its branch holds must not refuse the hand-in: {added:?}");
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
            let line = if line.starts_with("add ") { format!("{line} {}", checked(&r)) } else { line.to_string() };
            t.serve(&line, &mut out);
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
        // a lane pair that holds one half of this two-repo landing is named, with the half to merge;
        // both halves, or neither, is a pair that landed together
        let landed = [e.clone()];
        let (said, makepad) = half_landed(&landed, &r, &work, &m0).expect("its makepad half alone");
        assert!(makepad && half_landed_said(said, makepad, "rc-s").contains("its commercial half does not") && half_landed_said(said, makepad, "rc-s").contains("makepad devhub lanes merge rc-s"), "{}", half_landed_said(said, makepad, "rc-s"));
        // a run's refusal names its halves and the fix for the half that lacks it: the lane's pair
        // merges, a side branch takes main, a pinned older commit of a branch that holds it now
        let torn = |c: (&str, &str)| torn_pair_said(said, makepad, &r, ("rc-s", &work), c);
        let lane = torn(("rc-s", &m0));
        assert!(lane.starts_with(&format!("commercial rev {:.10} (branch rc-s) lacks #1 ", m0)) && lane.contains(&format!("which your makepad half {:.10} (branch rc-s) holds", work)) && lane.contains("run `makepad devhub lanes merge rc-s`"), "{lane}");
        let side = torn(("rc-s-vip", &m0));
        assert!(side.contains("rc-s-vip is a side branch, not the lane's commercial half rc-s: merge main into it"), "{side}");
        let pinned = torn(("main", &m0));
        assert!(pinned.contains(&format!("branch main holds it now: your rev pins its older {:.10}; run at the branch's tip", m0)), "{pinned}");
        assert_eq!(half_landed(&landed, &r, &w0, &main).map(|(e, m)| (e.id, m)), Some((1, false)), "its commercial half alone");
        assert!(half_landed(&landed, &r, &work, &main).is_none() && half_landed(&landed, &r, &w0, &m0).is_none());
        // a makepad commit on work pairs with the main its landing left: the landing that made it,
        // else the newest one it holds; one older than every landing has none
        assert_eq!(landed_main(&landed, &r, &work).as_deref(), Some(main.as_str()), "the landing's own work commit");
        let after = commit(&r, "after", "after\n", "a makepad-only commit on work");
        assert_eq!(landed_main(&landed, &r, &after).as_deref(), Some(main.as_str()), "a later work commit");
        assert_eq!(landed_main(&landed, &r, &w0), None, "older than the landing");
        // a rev naming an older makepad commit beside a commercial HEAD that holds the landing: a head
        // with nothing of its own runs at the main that commit landed beside; one with commits of its
        // own, or a makepad commit holding a landing the head lacks, is refused
        assert_eq!(beside_head(&landed, &r, &work, &main), Ok(None));
        assert!(beside_head(&landed, &r, &w0, &main).unwrap().is_some_and(|why| why.starts_with("the commercial HEAD holds #1 ")), "a head that is main's");
        g(&c, &["checkout", "-qb", "rc-own", "main"]);
        let own = commit(&c, "model", "tubes(curves, blend, own)\n", "a commit of its own on main");
        g(&c, &["checkout", "-q", "main"]);
        assert!(beside_head(&landed, &r, &w0, &own).is_err_and(|e| e.contains("or name the commercial side")), "a head with commits of its own");
        // newest is the order of landing: a later landing with a lower number (held behind a
        // re-check) whose makepad half the pair holds is named, not passed for the older one both
        // hold; a CI proof landing in both repos splits no pair, whatever its number (#5455, 11-16)
        let later = Entry { id: 0, pre: e.landed.clone(), landed: format!("work {after} main {own}"), changed_ms: e.changed_ms + 1, ..e.clone() };
        assert_eq!(half_landed(&[e.clone(), later.clone()], &r, &after, &main).map(|(x, m)| (x.id, m)), Some((0, true)), "the later landing, though its number is lower");
        let proof = Entry { id: 2, proof: true, ..later };
        assert!(half_landed(&[e.clone(), proof.clone()], &r, &after, &main).is_none(), "a proof landing splits no pair");
        assert_eq!(beside_head(&[e.clone(), proof], &r, &after, &main), Ok(None), "nor a --rev beside the head");
        assert!(beside_head(&landed, &r, &work, &m0).unwrap().is_some_and(|why| why.starts_with(&format!("makepad {work:.10} holds #1 "))), "a head behind the landing with nothing of its own runs at the main the makepad commit landed beside");
        g(&c, &["checkout", "-qb", "rc-behind", &m0]);
        let behind = commit(&c, "other", "behind\n", "a commit of its own before the landing");
        g(&c, &["checkout", "-q", "main"]);
        assert!(beside_head(&landed, &r, &work, &behind).is_err_and(|e| e.contains("makepad devhub lanes merge")), "a head of its own that lacks the landing is refused, naming the merge");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Reading the tree before a landing and the parent after it is an undo, not a lane's edit
    /// (11-15). Both halves refuse it before queueing, unless that landing is named explicitly.
    #[cfg(unix)]
    #[test]
    fn a_hand_in_that_undoes_a_landing_must_name_it() {
        use makepad_git::{Commit, ObjectId, Signature, Tree, TreeEntry};
        let d = std::env::temp_dir().join(format!("devhub-land-undone-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (r, home) = (d.join("repo"), d.join("home"));
        let c = r.join("apps/commercial");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let g = |dir: &Path, args: &[&str]| assert!(git(dir, args).0, "git {args:?} in {}", dir.display());
        for (dir, target) in [(&r, "work"), (&c, "main")] {
            g(dir, &["init", "-q", "-b", target]);
            g(dir, &["config", "user.email", "t@t"]);
            g(dir, &["config", "user.name", "t"]);
        }
        // Write the tiny fixtures in process; only the train's landing needs real checkouts.
        let tree = |dir: &Path, files: &[(&str, &str)]| crate::gitread::with(dir, |_, repo| {
            let mut entries: Vec<_> = files.iter().map(|(name, text)| TreeEntry { mode: 0o100644, name: name.to_string(), oid: repo.write_blob(text.as_bytes()).unwrap() }).collect();
            entries.push(TreeEntry { mode: 0o100644, name: ".gitignore".into(), oid: repo.write_blob(b"apps/commercial\n").unwrap() });
            repo.write_tree(&Tree { entries }).unwrap().to_hex()
        }).unwrap();
        let commit = |dir: &Path, tree: &str, parents: &[&str], message: &str| crate::gitread::with(dir, |_, repo| {
            let author = Signature { name: "t".into(), email: "t@t".into(), timestamp: 1, tz_offset: "+0000".into() };
            repo.write_commit(&Commit { tree: ObjectId::from_hex(tree).unwrap(), parents: parents.iter().map(|s| ObjectId::from_hex(s).unwrap()).collect(), author: author.clone(), committer: author, message: message.into() }).unwrap().to_hex()
        }).unwrap();
        let mut old_trees = Vec::new();
        for (dir, target) in [(&r, "work"), (&c, "main")] {
            let old = tree(dir, &[("a", "old"), ("b", "old")]);
            let base = commit(dir, &old, &[], "base");
            g(dir, &["update-ref", &format!("refs/heads/{target}"), &base]);
            let changed = tree(dir, &[("a", "landed"), ("added", "new"), ("b", "landed")]);
            let lane = commit(dir, &changed, &[&base], &format!("first {target} lane"));
            g(dir, &["update-ref", "refs/heads/rc-first", &lane]);
            g(dir, &["reset", "--hard", "-q", target]);
            old_trees.push(old);
        }
        let cwt = c.to_str().unwrap();
        let t = train(&home, &r, Path::new("/usr/bin/true"), Arc::new(Mutex::new(Vec::new())));
        t.add(&[&checked(&r), "rc-first", cwt, "rc-first"]).expect("the first lane is queued");
        assert!(t.start_ready(), "the first lane must land");
        let first = t.st.lock().unwrap()[0].clone();
        assert_eq!(first.state, St::Landed, "the first lane's state records the landing");
        let tips = pair_sides(&first.landed).expect("the landing records both target tips");
        for ((dir, tip), old) in [(&r, &tips.0), (&c, &tips.1)].into_iter().zip(&old_trees) {
            let stale = commit(dir, old, &[tip], "old tree with newer parent");
            g(dir, &["update-ref", "refs/heads/rc-stale", &stale]);
        }
        for (words, target) in [(vec!["rc-stale"], "work"), (vec!["-", cwt, "rc-stale"], "main")] {
            let mut words = words;
            words.push(CHECKED);
            let why = t.add(&words).expect_err("an old tree with a newer parent must be refused");
            assert!(why.contains("returns 3 path(s)") && why.contains("a, added, b"), "the refusal names every undone path in {target}: {why}");
            assert!(why.contains(&format!("#{} (rc-first)", first.id)) && why.contains(&format!("older {target}")) && why.contains(&format!("--revert {}", first.id)), "the refusal names the landing, target and remedy: {why}");
        }
        assert_eq!(t.st.lock().unwrap().len(), 1, "refused hand-ins must leave nothing queued");
        assert!(t.add(&[CHECKED, "rc-stale", "revert=999"]).is_err(), "naming an unrelated landing must not exempt any paths");
        assert!(t.add(&[CHECKED, "rc-stale", "revert=abcdef"]).unwrap_err().contains("7+ hex"), "a short nonnumeric prefix must be refused");
        let allow = format!("revert={}", first.id);
        t.add(&[CHECKED, "rc-stale", cwt, "rc-stale", &allow, "agent=lane-stale"]).expect("naming the landing permits its undo in both halves");
        assert_eq!(t.st.lock().unwrap()[1].state, St::Queued, "the explicit revert is queued");
        let (work, main) = (format!("revert={}", tips.0[..7].to_ascii_uppercase()), format!("revert={:.10}", tips.1));
        let why = t.add(&[CHECKED, "rc-stale", cwt, "rc-stale", &work]).expect_err("a makepad commit prefix must not exempt the commercial undo");
        assert!(why.contains("older main") && !why.contains("older work"), "an exemption applies only to its commit: {why}");
        t.add(&[CHECKED, "rc-stale", cwt, "rc-stale", &work, &main]).expect("repeatable commit prefixes also permit the undo");
        for (dir, tip) in [(&r, &tips.0), (&c, &tips.1)] {
            let edited = tree(dir, &[("a", "own edit"), ("added", "new"), ("b", "landed")]);
            let own = commit(dir, &edited, &[tip], "own edit");
            g(dir, &["update-ref", "refs/heads/rc-own", &own]);
            let deleted = tree(dir, &[("a", "landed"), ("b", "landed")]);
            let deletion = commit(dir, &deleted, &[tip], "delete added file");
            g(dir, &["update-ref", "refs/heads/rc-delete", &deletion]);
        }
        t.add(&[CHECKED, "rc-own", cwt, "rc-own"]).expect("a new version of a previously landed path must pass");
        assert_eq!(t.st.lock().unwrap()[2].state, St::Queued, "the lane's own edit is queued");
        for words in [vec!["rc-delete"], vec!["-", cwt, "rc-delete"]] {
            let mut words = words;
            words.push(CHECKED);
            let why = t.add(&words).expect_err("deleting a file the landing added must be refused");
            assert!(why.contains("returns 1 path(s)") && why.contains(": added;") && why.contains(&format!("#{} (rc-first)", first.id)), "the added file and landing must be named: {why}");
        }
        assert_eq!(t.st.lock().unwrap().len(), 3, "refused deletions must leave the queue untouched");
        // A target change without a landing record is still named and needs an explicit undo.
        let changed = tree(&r, &[("a", "direct edit"), ("added", "new"), ("b", "landed")]);
        let direct = commit(&r, &changed, &[&tips.0], "unrecorded target change");
        g(&r, &["reset", "--hard", "-q", &direct]);
        let prior = crate::gitread::commit_parts(&r, &tips.0).unwrap().0;
        let undo = commit(&r, &prior, &[&direct], "undo unrecorded change");
        g(&r, &["update-ref", "refs/heads/rc-unrecorded", &undo]);
        let why = t.add(&[CHECKED, "rc-unrecorded"]).expect_err("an unrecorded target change must also be protected");
        assert!(why.contains(&format!("{direct:.10} unrecorded target change")) && why.contains(&format!("--revert {direct:.10}")), "an unrecorded change is named by sha and subject: {why}");
        t.add(&[CHECKED, "rc-unrecorded", &format!("revert={direct:.7}")]).expect("an unrecorded change can be reverted by commit prefix");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Two lanes that each add one dependency, in different places of a manifest: git merges the
    /// second landing cleanly with the line twice, and cargo then refuses every build of the
    /// workspace. The gate refuses that landing before a ref moves and names the file and key, in
    /// either repository (a commercial crate's dependency declared twice, 11-15).
    #[cfg(unix)]
    #[test]
    fn a_landing_whose_merged_manifest_does_not_parse_is_refused() {
        let d = std::env::temp_dir().join(format!("devhub-land-manifest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (r, home) = (d.join("repo"), d.join("home"));
        let c = r.join("apps/commercial");
        std::fs::create_dir_all(c.join("view")).unwrap();
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
        let manifest = |extra_after_a: &str, extra_after_d: &str| format!("[package]\nname = \"view\"\n\n[dependencies]\na = \"1\"\n{extra_after_a}b = \"1\"\nc = \"1\"\nd = \"1\"\n{extra_after_d}");
        commit(&r, "lib", "base\n", "base");
        commit(&c, "view/Cargo.toml", &manifest("", ""), "cbase");
        // two lanes off the same tips, each adding `parts` in its own place
        for (lane, after_a, after_d) in [("rc-a", "parts = \"1\"\n", ""), ("rc-b", "", "parts = \"1\"\n")] {
            for dir in [&r, &c] { g(dir, &["checkout", "-qb", lane, if dir == &r { "work" } else { "main" }]); }
            commit(&r, &format!("{lane}.rs"), "x\n", lane);
            commit(&c, "view/Cargo.toml", &manifest(after_a, after_d), lane);
        }
        g(&r, &["checkout", "-q", "work"]);
        g(&c, &["checkout", "-q", "main"]);
        let t = train(&home, &r, Path::new("/usr/bin/true"), Arc::new(Mutex::new(Vec::new())));
        let ask = |line: &str| {
            let mut out = Vec::new();
            let line = if line.starts_with("add ") { format!("{line} {}", checked(&r)) } else { line.to_string() };
            t.serve(&line, &mut out);
            out
        };
        let st = |lane: &str| Entry::parse(&ask(&format!("status {lane}"))[0]).unwrap();
        assert!(ask(&format!("add rc-a {} rc-a agent=lane-a", c.display()))[0].contains("\"id\":1"));
        assert!(t.start_ready());
        assert_eq!(st("rc-a").state, St::Landed, "the first lane's dependency lands");
        let tips = (rev(&r, "refs/heads/work").unwrap(), rev(&c, "refs/heads/main").unwrap());
        assert!(ask(&format!("add rc-b {} rc-b agent=lane-b", c.display()))[0].contains("\"id\":2"));
        assert!(!t.start_ready());
        let e = st("rc-b");
        assert_eq!(e.state, St::Conflict, "the second landing's merged manifest has `parts` twice: {e:?}");
        assert!(e.detail.len() == 1 && e.detail[0].starts_with("view/Cargo.toml:") && e.detail[0].contains("duplicate key") && e.detail[0].contains("parts"), "the refusal names the file and the key: {:?}", e.detail);
        assert_eq!((rev(&r, "refs/heads/work").unwrap(), rev(&c, "refs/heads/main").unwrap()), tips, "no ref moved, in either repository");
        // the makepad side's merges are checked the same way
        let w = commit(&r, "Cargo.toml", &manifest("", ""), "a makepad manifest");
        for (lane, after_a, after_d) in [("rc-c", "parts = \"1\"\n", ""), ("rc-d", "", "parts = \"1\"\n")] {
            g(&r, &["checkout", "-qb", lane, &w]);
            commit(&r, "Cargo.toml", &manifest(after_a, after_d), lane);
        }
        g(&r, &["checkout", "-q", "work"]);
        let first = merge_onto(&r, &w, &rev(&r, "rc-c").unwrap(), "m").expect("one addition merges");
        let refused = merge_onto(&r, &first, &rev(&r, "rc-d").unwrap(), "m").expect_err("the second addition is refused");
        assert!(refused.len() == 1 && refused[0].starts_with("Cargo.toml:") && refused[0].contains("duplicate key"), "the makepad manifest is named: {refused:?}");
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
            let line = if line.starts_with("add ") { format!("{line} {}", checked(&r)) } else { line.to_string() };
            t.serve(&line, &mut out);
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
            let line = if line.starts_with("add ") { format!("{line} {}", checked(&r)) } else { line.to_string() };
            t.serve(&line, &mut out);
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
