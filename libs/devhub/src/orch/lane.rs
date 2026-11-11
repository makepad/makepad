//! The lane worktree lifecycle, owned by the master (lanes never run `git worktree` themselves):
//! - `devhub lane new <name> [--from work] [--no-commercial]`: `<lanes root>/<name>` on `rc-<name>`
//!   from work, with the nested `apps/commercial` on `rc-<name>` from main; registered and
//!   watched ([`crate::monitor`]: its dirty files without scans).
//! - after it lands, the master removes it: both halves in work and main (the branch tips are in
//!   them and are not work's own first-parent commits, so a fresh lane never counts), nothing
//!   uncommitted, no process inside, no lease, job or land entry of the lane open, and no file
//!   activity and no job or lease ending for [`GRACE_MS`]. Then the worktree pair, the branches
//!   (local, and origin's when pushed) and the lane's dirs on every node go. An agent that only
//!   reads or thinks that long is not seen: `devhub lane keep <name>` exempts a lane.
//!   A lane whose branch and worktree went some other way has its record cleared.
//! - `devhub lane drop <name>`: abandons a lane: uncommitted work is committed on its branch,
//!   every commit not in work / main is bundled into `~/.makepad/devhub/dropped/`, then
//!   everything goes.
//! - `devhub lane adopt`: registers the existing live worktrees under the lanes root and lists the
//!   landed ones (it removes nothing); `devhub lane remove <name>` removes one of those once the
//!   user agreed (the same checks as the automatic cleanup, without the grace).
//! - `devhub lane park <name> [why]` / `unpark <name>`: a lane that waits (finished, or for
//!   another lane): the merge bot leaves its branches alone (held: said once, never a warning),
//!   `lane quiet` lists it in one row and never offers it for retirement.
//! - `devhub lane retire <name>`: a lane that is over without landing leaves its worktree and keeps
//!   its branches (nothing uncommitted or untracked in it, no process, run or hand-in of it open).
//!   `lane quiet` offers it when nothing of it came for [`RETIRE_OFFER_MS`] and its agent is gone.
//! - `devhub lanes`: each lane's worktree, tip, dirty files, merge state (the merge bot's), land
//!   state (the train's).

use super::{git, Ctx};
use crate::lease::wt;
use crate::proto::{wall_ms, Sev};
use crate::state::LaneRec;
use makepad_git::{ObjectId, Reach, Repository};
use makepad_remote::json_str;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A landed lane is removed only after this long with no file activity, no new commit and no job
/// or lease of it ending.
pub const GRACE_MS: u64 = 30 * 60 * 1000;
const TICK: Duration = Duration::from_secs(60);

pub fn valid_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && !s.starts_with('-') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

pub fn root(ctx: &Ctx) -> PathBuf {
    wt::root_of(&ctx.repo)
}

fn event(ctx: &Ctx, sev: Sev, line: String, l: Option<&LaneRec>) {
    ctx.store.event("lane", sev, line, l.map(|l| crate::state::lane_value(l).to_json()));
}

fn rec(ctx: &Ctx, name: &str) -> Option<LaneRec> {
    ctx.store.read(|d| d.lanes.iter().rev().find(|l| l.name == name && l.state == "live").cloned())
}

fn set(ctx: &Ctx, name: &str, f: impl FnOnce(&mut LaneRec)) -> Option<LaneRec> {
    try_set(ctx, name, f).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); None })
}

fn try_set(ctx: &Ctx, name: &str, f: impl FnOnce(&mut LaneRec)) -> Result<Option<LaneRec>, String> {
    ctx.store.update(|d| {
        let l = d.lanes.iter_mut().rev().find(|l| l.name == name && l.state == "live")?;
        f(l);
        Some(l.clone())
    })
}

pub fn start(ctx: Arc<Ctx>) {
    // a lane's dirty set is baselined when it is first asked about (a job, a live session, the
    // cleanup of a landed lane), not for every registered lane at start: most are idle
    let c2 = ctx.clone();
    let _ = std::thread::Builder::new().name("devhub-lanes-clean".into()).spawn(move || {
        loop {
            std::thread::sleep(TICK);
            cleanup(&c2);
        }
    });
}

fn branch_tip(dir: &Path, branch: &str) -> Option<String> {
    let p = makepad_git::repository_paths(dir).ok()??;
    makepad_git::refs::resolve_ref_in(&p.git_dir, &p.common_dir, &format!("refs/heads/{branch}")).ok().map(|o| o.to_hex())
}

/// One repo's history as the cleanup reads it, kept across its ticks (in process, never git):
/// what the target (work or main) reaches, the target's own first-parent commits, and each lane
/// tip's newest own commit.
#[derive(Default)]
struct Hist {
    reach: Reach,
    /// the target's first-parent line (`git rev-list --first-parent <target>`)
    firsts: HashSet<ObjectId>,
    /// (lane tip, branch) -> its newest commit past the merge bot's merges (None: none in 200)
    newest: HashMap<(ObjectId, String), Option<ObjectId>>,
    /// the retired chains (`<target>-old-*`, kept when the target moved onto a squashed tip):
    /// (ref, what it reaches, its first-parent line). What they reach landed.
    retired: Vec<(String, Reach, HashSet<ObjectId>)>,
}

static HISTS: Mutex<BTreeMap<PathBuf, Hist>> = Mutex::new(BTreeMap::new());

/// A repo for one cleanup pass: its history moved to the target's tip (only new commits read).
struct View {
    dir: PathBuf,
    target: &'static str,
    git_dir: PathBuf,
    common: PathBuf,
    /// the repo's shared reader ([`crate::gitread`]), locked per question
    repo: std::sync::Arc<Mutex<Repository>>,
    hist: Hist,
    /// the lane tips this pass asked about (the cleanup forgets the others)
    used: HashSet<(ObjectId, String)>,
}

impl View {
    fn open(dir: &Path, target: &'static str) -> Result<View, String> {
        let e = |e: makepad_git::GitError| format!("{}: {e}", dir.display());
        let p = makepad_git::repository_paths(dir).map_err(e)?.ok_or_else(|| format!("{}: not a repository", dir.display()))?;
        let tip = makepad_git::refs::resolve_ref_in(&p.git_dir, &p.common_dir, &format!("refs/heads/{target}")).map_err(e)?;
        let shared = crate::gitread::handle(&p.common_dir).ok_or_else(|| format!("{}: cannot read {}", dir.display(), p.common_dir.display()))?;
        let mut guard = shared.lock().unwrap_or_else(|e| e.into_inner());
        let repo = &mut *guard;
        let mut hist = HISTS.lock().unwrap().remove(dir).unwrap_or_default();
        let old = hist.reach.tip();
        if old != Some(tip) {
            hist.reach.advance(repo, tip).map_err(e)?;
            // the first-parent line grows at its top while the tip moves forward along it
            let mut fresh = Vec::new();
            let mut joined = false;
            for c in repo.first_parents(tip) {
                let (oid, _) = c.map_err(e)?;
                if hist.firsts.contains(&oid) {
                    joined = Some(oid) == old;
                    break;
                }
                fresh.push(oid);
            }
            if !joined && !hist.firsts.is_empty() {
                hist.firsts.clear();
                for c in repo.first_parents(tip) {
                    hist.firsts.insert(c.map_err(e)?.0);
                }
            } else {
                hist.firsts.extend(fresh);
            }
        }
        // retired chains never move: each is read once
        let old_prefix = format!("refs/heads/{target}-old-");
        let refs = makepad_git::refs::list_refs_in(&p.git_dir, &p.common_dir, "refs/heads/").map_err(e)?;
        let mut retired = Vec::new();
        for r in refs.into_iter().filter(|r| r.name.starts_with(&old_prefix)) {
            let makepad_git::RefTarget::Direct(tip) = r.target else { continue };
            if let Some(i) = hist.retired.iter().position(|x| x.0 == r.name && x.1.tip() == Some(tip)) {
                retired.push(hist.retired.swap_remove(i));
                continue;
            }
            let mut reach = Reach::new();
            reach.advance(repo, tip).map_err(e)?;
            let mut firsts = HashSet::new();
            for c in repo.first_parents(tip) {
                firsts.insert(c.map_err(e)?.0);
            }
            retired.push((r.name, reach, firsts));
        }
        hist.retired = retired;
        drop(guard);
        Ok(View { dir: dir.to_path_buf(), target, git_dir: p.git_dir, common: p.common_dir, repo: shared, hist, used: HashSet::new() })
    }

    /// Whether `oid` landed: the target or a retired chain of it reaches it.
    fn reached(&self, oid: &ObjectId) -> bool {
        self.hist.reach.contains(oid) || self.hist.retired.iter().any(|r| r.1.contains(oid))
    }

    /// One of the target's (or a retired chain's) own first-parent commits.
    fn first_parent(&self, oid: &ObjectId) -> bool {
        self.hist.firsts.contains(oid) || self.hist.retired.iter().any(|r| r.2.contains(oid))
    }

    /// The branch's commit; None: no such branch. A ref that does not read is an error.
    fn tip_of(&self, branch: &str) -> Result<Option<ObjectId>, String> {
        match makepad_git::refs::resolve_ref_in(&self.git_dir, &self.common, &format!("refs/heads/{branch}")) {
            Ok(oid) => Ok(Some(oid)),
            Err(makepad_git::GitError::RefNotFound(_)) => Ok(None),
            Err(e) => Err(format!("branch {branch} does not read: {e}")),
        }
    }

    /// One half landed: its own commits are in the target (they reach it, and the newest is not
    /// one of the target's own first-parent commits, so a fresh lane never counts). The merge
    /// bot's merges of the tip into the lane ("Merge work (<sha>) into rc-x") are not the lane's
    /// own: a lane that landed and then got the tip merged in again is landed. `tree`: a detached
    /// worktree's HEAD.
    fn landed(&mut self, branch: &str, tree: &Path, check_own: bool) -> Result<(), String> {
        let tip = if branch.is_empty() {
            crate::monitor::read_head(tree).and_then(|h| ObjectId::from_hex(&h.0).ok()).ok_or_else(|| format!("{}: no HEAD", tree.display()))?
        } else {
            self.tip_of(branch)?.ok_or_else(|| format!("no branch {branch}"))?
        };
        let name = if branch.is_empty() { "its detached HEAD" } else { branch };
        let key = (tip, branch.to_string());
        self.used.insert(key.clone());
        let newest = match self.hist.newest.get(&key) {
            Some(n) => *n,
            None => {
                let (pre, post) = (format!("Merge {} (", self.target), format!(") into {branch}"));
                let mut newest = None;
                // the lane's newest own commit: down its first-parent line past the bot's merges
                let mut repo = self.repo.lock().unwrap_or_else(|e| e.into_inner());
                for c in repo.first_parents(tip).take(200) {
                    let (oid, c) = c.map_err(|e| format!("{}: {e}", self.dir.display()))?;
                    let subject = c.message.lines().next().unwrap_or("");
                    if branch.is_empty() || !(subject.starts_with(&pre) && subject.ends_with(&post)) {
                        newest = Some(oid);
                        break;
                    }
                }
                self.hist.newest.insert(key, newest);
                newest
            }
        };
        let newest = newest.ok_or_else(|| format!("{name}: no commit of its own in 200"))?;
        // a lane that landed before its target moved onto a squashed tip landed on the retired chain
        if !self.reached(&newest) {
            return Err(format!("{name} is not in {}", self.target));
        }
        if check_own && self.first_parent(&newest) {
            return Err(format!("{name} has no commits of its own (never landed)"));
        }
        Ok(())
    }
}

impl View {
    /// Its commits that landed nowhere: beyond the target and its retired chains, merges and the
    /// bot's merges of the target left out (usize::MAX: unreadable).
    fn unlanded(&mut self, branch: &str, head: ObjectId) -> usize {
        let (pre, post) = (format!("Merge {} (", self.target), format!(") into {branch}"));
        let reaches: Vec<&Reach> = std::iter::once(&self.hist.reach).chain(self.hist.retired.iter().map(|r| &r.1)).collect();
        match Reach::range_beyond(&reaches, &mut self.repo.lock().unwrap_or_else(|e| e.into_inner()), head) {
            Ok(r) => r.commits.iter().filter(|(_, c)| {
                let subject = c.message.lines().next().unwrap_or("");
                c.parents.len() < 2 && !(subject.starts_with(&pre) && subject.ends_with(&post))
            }).count(),
            Err(_) => usize::MAX,
        }
    }

    /// A half that did not land (`why`) may still go when asked: when nothing of it is unlanded.
    fn nothing_unlanded(&mut self, branch: &str, tree: &Path, asked: bool, why: String) -> Result<(), String> {
        if !asked {
            return Err(why);
        }
        let head = if branch.is_empty() { crate::monitor::read_head(tree).and_then(|h| ObjectId::from_hex(&h.0).ok()) } else { self.tip_of(branch)? };
        let Some(head) = head else { return Err(why) };
        match self.unlanded(branch, head) {
            0 => Ok(()),
            usize::MAX => Err(format!("{why}; its commits could not be read")),
            n => Err(format!("{why}: {n} commit(s) of its own landed nowhere")),
        }
    }

    /// Forgets the lane tips no one asked about this pass (lanes gone or moved on).
    fn forget_unused(&mut self) {
        let used = std::mem::take(&mut self.used);
        self.hist.newest.retain(|k, _| used.contains(k));
    }
}

impl Drop for View {
    fn drop(&mut self) {
        HISTS.lock().unwrap().insert(self.dir.clone(), std::mem::take(&mut self.hist));
    }
}

/// Both repos for one cleanup pass (commercial: when the checkout has it).
struct Views {
    makepad: View,
    commercial: Option<View>,
}

impl Views {
    fn open(ctx: &Ctx) -> Result<Views, String> {
        let commercial = if ctx.commercial().join(".git").exists() { Some(View::open(&ctx.commercial(), "main")?) } else { None };
        Ok(Views { makepad: View::open(&ctx.repo, "work")?, commercial })
    }
}

/// Whether the lane landed (both halves), and what still references it; Ok = removable.
/// `asked`: `lane remove` named it. A lane with nothing of its own (every commit but merges in
/// its target or a retired chain of it: never landed, or a fresh lane left unused) goes too then;
/// the automatic cleanup takes only landed lanes, never one an agent just made.
fn removable(ctx: &Ctx, l: &LaneRec, views: &mut Views, cwds: &[(u32, PathBuf)], land: &[String], asked: bool) -> Result<(), String> {
    let path = Path::new(&l.path);
    views.makepad.landed(&l.branch, path, true).or_else(|e| views.makepad.nothing_unlanded(&l.branch, path, asked, e))?;
    let nested = path.join("apps/commercial");
    if nested.join(".git").exists() {
        if l.cbranch.is_empty() {
            if wt::unique_commits(&nested) {
                return Err("apps/commercial has commits on no commercial ref".into());
            }
        } else if let Some(c) = views.commercial.as_mut() {
            // its own commits (if any) in main; the bot's merges of main into it do not count
            if c.tip_of(&l.cbranch).map_err(|e| format!("commercial: {e}"))?.is_some() {
                c.landed(&l.cbranch, &nested, false).or_else(|e| c.nothing_unlanded(&l.cbranch, &nested, asked, e)).map_err(|e| format!("commercial: {e}"))?;
            }
        }
    }
    for (tree, branch) in [(path.to_path_buf(), &l.branch), (nested.clone(), &l.cbranch)] {
        if tree.join(".git").exists() {
            let (head, actual) = crate::monitor::read_head(&tree).ok_or("worktree has no HEAD")?;
            if actual != *branch || (!branch.is_empty() && branch_tip(&tree, branch).as_deref() != Some(head.as_str())) {
                return Err(format!("{}: HEAD or branch identity changed; reconcile before cleanup", tree.display()));
            }
        }
    }
    for tree in [path.to_path_buf(), nested.clone()] {
        if !tree.join(".git").exists() {
            continue;
        }
        let d = ctx.monitor.dirty(&tree)?;
        if !d.changed.is_empty() || !d.deleted.is_empty() {
            return Err(format!("{} uncommitted file(s) in {}", d.changed.len() + d.deleted.len(), tree.display()));
        }
        // and git itself before anything is destroyed (the monitor may not have heard the last write yet)
        if wt::dirty(&tree, (tree == path).then_some("apps/commercial")) {
            return Err(format!("uncommitted changes in {}", tree.display()));
        }
    }
    let live = wt::live_in(cwds, path);
    if !live.is_empty() {
        return Err(format!("process(es) {live:?} run inside"));
    }
    if let Some(why) = open_run(ctx, l) {
        return Err(why.into());
    }
    if handed_in(l, land) {
        return Err("its hand-in is still in the land queue".into());
    }
    Ok(())
}

/// A hand-in of the lane is still in the land queue (`land`: [`land_entries`]).
fn handed_in(l: &LaneRec, land: &[String]) -> bool {
    land.iter().filter_map(|x| makepad_strict_json::parse(x.as_bytes()).ok()).any(|v| {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        (s("branch") == l.branch || (!l.cbranch.is_empty() && s("cbranch") == l.cbranch)) && s("state") != "landed"
    })
}

/// A worktree's status read in process (no git process): its uncommitted tracked paths and its
/// untracked (not ignored) ones. `outer`: the makepad half, whose nested `apps/commercial` is the
/// other half's.
fn status_of(tree: &Path, outer: bool) -> (Vec<String>, Vec<String>) {
    let (mut tracked, mut untracked) = (Vec::new(), Vec::new());
    match makepad_git::repository_paths(tree).ok().flatten().map(crate::gitread::worktree) {
        Some(Ok(mut r)) => match r.status() {
            Ok(st) => {
                for e in st.entries {
                    if outer && e.path.starts_with("apps/commercial/") {
                        continue;
                    }
                    if e.status == makepad_git::FileStatus::Untracked { untracked.push(e.path) } else { tracked.push(e.path) }
                }
                tracked.dedup();
            }
            Err(e) => tracked.push(format!("(status failed: {e})")),
        },
        _ => tracked.push("(no git dir)".into()),
    }
    (tracked, untracked)
}

/// Why `lane retire` refuses a lane for what is in its worktree: anything uncommitted or untracked
/// in either half, as (class, the refusal naming the files).
fn dirty_refusal(path: &Path) -> Option<(&'static str, String)> {
    for (tree, outer) in [(path.to_path_buf(), true), (path.join("apps/commercial"), false)] {
        if !tree.join(".git").exists() {
            continue;
        }
        let (tracked, untracked) = status_of(&tree, outer);
        let files: Vec<String> = tracked.into_iter().chain(untracked).collect();
        if !files.is_empty() {
            let more = if files.len() > 8 { format!(" and {} more", files.len() - 8) } else { String::new() };
            return Some(("uncommitted or untracked files", format!("{} uncommitted or untracked file(s) in {}: {}{more} (commit them on its branch or remove them first)", files.len(), tree.display(), files[..files.len().min(8)].join(", "))));
        }
    }
    None
}

/// Why `lane retire` refuses a lane for what runs of it: a process inside (`cwds`), a lease or
/// job of it open, its hand-in in the land queue (`land`), as (class, the refusal).
fn run_refusal(ctx: &Ctx, l: &LaneRec, cwds: &[(u32, PathBuf)], land: &[String]) -> Option<(&'static str, String)> {
    let path = Path::new(&l.path);
    let live = wt::live_in(cwds, path);
    if !live.is_empty() {
        return Some(("a process runs inside", format!("process(es) {live:?} run inside {}: stop them first", path.display())));
    }
    if let Some(why) = open_run(ctx, l) {
        return Some((why, why.into()));
    }
    handed_in(l, land).then(|| ("its hand-in is in the land queue", "its hand-in is still in the land queue".to_string()))
}

/// The lane's branches, each once (both halves are usually `rc-<name>`).
fn branches(l: &LaneRec) -> Vec<String> {
    let mut b: Vec<String> = [&l.branch, &l.cbranch].into_iter().filter(|b| !b.is_empty()).cloned().collect();
    b.dedup();
    b
}

/// `lane park <name> [why]`: its branches go on the held list (the merge bot merges nothing into
/// them and says so once, as info), and `lane quiet` lists it in one row. Two finished lanes the
/// bot found in conflict with work warned about it every time the files moved (11-06).
fn park(ctx: &Ctx, name: &str, why: &str) -> Result<String, String> {
    let l = rec(ctx, name).ok_or_else(|| format!("no live lane {name}"))?;
    let why = if why.trim().is_empty() { "parked".to_string() } else { crate::proto::clean(why.trim(), 200) };
    let held = branches(&l);
    let held_why = if why == "parked" { why.clone() } else { format!("parked: {why}") };
    for b in &held {
        crate::sync::lanes::hold_in(&ctx.store, b, &held_why)?;
    }
    let l = try_set(ctx, name, |x| x.parked = why.clone())?;
    event(ctx, Sev::Info, format!("lane {name} parked ({why}): the merge bot leaves {} alone (`devhub lane unpark {name}`)", held.join(", ")), l.as_ref());
    Ok(format!("{{\"parked\":{},\"why\":{},\"branches\":[{}]}}", json_str(name), json_str(&why), held.iter().map(|b| json_str(b)).collect::<Vec<_>>().join(",")))
}

/// `lane unpark <name>`: its branches leave the held list; the bot merges into them again.
fn unpark(ctx: &Ctx, name: &str) -> Result<String, String> {
    let l = rec(ctx, name).ok_or_else(|| format!("no live lane {name}"))?;
    if l.parked.is_empty() {
        return Err(format!("lane {name} is not parked"));
    }
    for b in branches(&l) {
        crate::sync::lanes::unhold_in(&ctx.store, &b)?;
    }
    let l = try_set(ctx, name, |x| x.parked.clear())?;
    event(ctx, Sev::Info, format!("lane {name} unparked: the merge bot merges into it again"), l.as_ref());
    Ok(format!("{{\"unparked\":{}}}", json_str(name)))
}

/// `lane retire <name>`: a lane that is over without landing (finished elsewhere, or given up):
/// its worktree pair and node dirs go, its branches stay. Nothing uncommitted or untracked may be
/// in it (named), no process may run inside, and no lease, job or hand-in of it may be open.
fn retire(ctx: &Ctx, name: &str) -> Result<String, String> {
    let l = rec(ctx, name).ok_or_else(|| format!("no live lane {name}"))?;
    if let Some((_, why)) = dirty_refusal(Path::new(&l.path)).or_else(|| run_refusal(ctx, &l, &wt::live_cwds(), &land_entries(ctx))) {
        return Err(why);
    }
    let path = Path::new(&l.path);
    let mac = tree_bytes(path);
    let (files, bytes) = keep_local(&path.join("local"), &ctx.repo.join("local"), name).map_err(|e| format!("its local/ was not kept, nothing removed: {e}"))?;
    let what = remove(ctx, &l, false, false)?;
    let kept = if files == 0 { String::new() } else { format!(", local/: {files} file(s) kept in {} ({} MB)", ctx.repo.join("local").display(), bytes / 1_000_000) };
    let what = format!("{what}, Mac {} MB{kept}", mac / 1_000_000);
    // nothing of it is left for the bot to merge into
    if !l.parked.is_empty() {
        for b in branches(&l) {
            crate::sync::lanes::unhold_in(&ctx.store, &b)?;
        }
    }
    finish(ctx, name, "retired", Ok(what.clone()));
    Ok(what)
}

/// Keeps what a lane's worktree holds under `local/` before a retire removes it: reports,
/// evidence and screencaps, which git ignores, so neither the retire's check nor `git worktree
/// remove` sees them. Each file goes to the same path under `to` (the main checkout's `local/`).
/// One that is there with the same content is skipped. One that differs is never overwritten:
/// the lane's copy goes beside it as `<name>.<lane>` (`.<lane>.2`, … when that one differs too).
/// Symlinks and cargo targets (a CACHEDIR.TAG) are not copied. Returns the files and bytes copied.
fn keep_local(from: &Path, to: &Path, lane: &str) -> Result<(usize, u64), String> {
    let mut stack = vec![PathBuf::new()];
    let (mut files, mut bytes) = (0, 0);
    while let Some(rel) = stack.pop() {
        let dir = from.join(&rel);
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for e in entries.flatten() {
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            let rel = rel.join(e.file_name());
            if m.is_dir() {
                if !e.path().join("CACHEDIR.TAG").exists() {
                    stack.push(rel);
                }
                continue;
            }
            if !m.is_file() {
                continue;
            }
            let src = e.path();
            let mut dest = to.join(&rel);
            let mut n = 1;
            while dest.symlink_metadata().is_ok() && !same_file(&src, &dest) {
                let name = rel.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
                dest = to.join(&rel).with_file_name(if n == 1 { format!("{name}.{lane}") } else { format!("{name}.{lane}.{n}") });
                n += 1;
            }
            if dest.symlink_metadata().is_ok() {
                continue;
            }
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            bytes += std::fs::copy(&src, &dest).map_err(|e| format!("{} -> {}: {e}", src.display(), dest.display()))?;
            files += 1;
        }
    }
    Ok((files, bytes))
}

/// Whether `a` and `b` are files with the same content.
fn same_file(a: &Path, b: &Path) -> bool {
    use std::io::Read;
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    if !mb.is_file() || ma.len() != mb.len() {
        return false;
    }
    let (Ok(mut fa), Ok(mut fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    let (mut ba, mut bb) = (vec![0u8; 1 << 16], vec![0u8; 1 << 16]);
    loop {
        let (Ok(na), Ok(nb)) = (fa.read(&mut ba), fb.read(&mut bb)) else { return false };
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
}

/// A lease or job of the lane in flight, under its name or its branch (a lane's asks carry either).
fn open_run(ctx: &Ctx, l: &LaneRec) -> Option<&'static str> {
    let ours = |lane: &str| lane == l.name || lane == l.branch;
    if ctx.leases.view().iter().any(|x| ours(&x.req.lane) && !matches!(x.state, crate::lease::sched::State::Ended(_))) {
        return Some("a lease of the lane is open");
    }
    if ctx.store.read(|d| d.jobs.iter().any(|j| ours(&j.lane) && !j.done())) {
        return Some("a job of the lane is open");
    }
    None
}

/// Its branch and worktree went some other way: only the record is left, and `lane remove` could
/// only answer "no branch" while `lane quiet` kept listing it.
fn only_the_record(ctx: &Ctx, l: &LaneRec, views: &Views) -> bool {
    !Path::new(&l.path).exists() && (l.branch.is_empty() || matches!(views.makepad.tip_of(&l.branch), Ok(None))) && open_run(ctx, l).is_none()
}

/// The land queue's entries, from the master's state (the train keeps its queue there): an open
/// hand-in keeps its lane, so they are read in process, never over a link that may not answer.
fn land_entries(ctx: &Ctx) -> Vec<String> {
    ctx.store.read(|d| d.land.iter().map(|v| v.to_json()).collect())
}

/// The automatic cleanup pass: removes the lanes that landed, after the grace period.
pub fn cleanup(ctx: &Ctx) {
    let lanes: Vec<LaneRec> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live" && !l.keep).cloned().collect());
    if lanes.is_empty() {
        return;
    }
    let mut views = match Views::open(ctx) {
        Ok(v) => v,
        Err(e) => {
            event(ctx, Sev::Warn, format!("lane cleanup: {e}"), None);
            return;
        }
    };
    let cwds = wt::live_cwds();
    let land = land_entries(ctx);
    let now = wall_ms();
    for l in lanes {
        if only_the_record(ctx, &l, &views) {
            finish(ctx, &l.name, "removed", remove(ctx, &l, false, true));
            continue;
        }
        match removable(ctx, &l, &mut views, &cwds, &land, false) {
            Err(why) => {
                // a lane that landed and is still referenced says why once
                let landed = views.makepad.landed(&l.branch, Path::new(&l.path), true).is_ok();
                if l.landed_ms != 0 || (landed && l.note != why) {
                    set(ctx, &l.name, |x| {
                        if !landed {
                            x.landed_ms = 0;
                        }
                        x.note = if landed { format!("landed, kept: {why}") } else { String::new() };
                    });
                }
            }
            Ok(()) => {
                if l.landed_ms == 0 {
                    set(ctx, &l.name, |x| {
                        x.landed_ms = now;
                        x.note = "landed: removed after 30 min with no activity".into();
                    });
                    event(ctx, Sev::Info, format!("lane {}: landed; its worktree goes after 30 min with no activity (devhub lane keep {} to keep it)", l.name, l.name), Some(&l));
                    continue;
                }
                // quiet for the grace: since it landed, since its last file event and since its
                // last run: an agent between runs on overlays touches no file (a lane's worktree
                // and 23 GB of its node dirs went the moment its last A/B ended, 40 s before its
                // next step, 11-04)
                let active = ctx.monitor.last_event_ms(Path::new(&l.path)).max(ctx.monitor.last_event_ms(&Path::new(&l.path).join("apps/commercial")));
                if now.saturating_sub(l.landed_ms.max(active)) < GRACE_MS {
                    continue;
                }
                let ran = ended_runs(ctx, std::slice::from_ref(&l)).into_values().map(|e| e.at).max().unwrap_or(0);
                if now.saturating_sub(ran) < GRACE_MS {
                    continue;
                }
                let what = remove(ctx, &l, false, true);
                finish(ctx, &l.name, "removed", what);
            }
        }
    }
    views.makepad.forget_unused();
    if let Some(c) = views.commercial.as_mut() {
        c.forget_unused();
    }
}

fn finish(ctx: &Ctx, name: &str, state: &str, what: Result<String, String>) {
    match what {
        Ok(w) => {
            let l = set(ctx, name, |x| {
                x.state = state.into();
                x.ended_ms = wall_ms();
                x.note = w.clone();
            });
            event(ctx, Sev::Info, format!("lane {name} {state}: {w}"), l.as_ref());
        }
        Err(e) => {
            let l = set(ctx, name, |x| x.note = format!("{state} failed: {e}"));
            event(ctx, Sev::Warn, format!("lane {name}: {state} failed: {e}"), l.as_ref());
        }
    }
}

/// Keep the exact commit before removing its worktree or branch, including detached heads
/// and bot merges absent from the destination. Recovery refs are private and never pushed.
fn preserve_commit(tree: &Path, oid: &str, target: &str) -> Result<(), String> {
    if git(tree, &["merge-base", "--is-ancestor", oid, &format!("refs/heads/{target}")]).is_err() {
        git(tree, &["update-ref", &format!("refs/devhub/recovery/{oid}"), oid])?;
    }
    Ok(())
}

fn preserve_head(tree: &Path, target: &str) -> Result<(), String> {
    let head = git(tree, &["rev-parse", "--verify", "HEAD"])?;
    preserve_commit(tree, &head, target)?;
    if git(tree, &["rev-parse", "--verify", "HEAD"])? != head {
        return Err("HEAD changed during cleanup; worktree kept for reconciliation".into());
    }
    Ok(())
}

/// Removes the worktree pair after preserving its actual heads, conditionally deletes the
/// checked local/origin branch tips (`branches`; a retired lane keeps them), and cleans the
/// lane's dirs on every node.
fn remove(ctx: &Ctx, l: &LaneRec, drop: bool, branches: bool) -> Result<String, String> {
    let path = PathBuf::from(&l.path);
    let nested = path.join("apps/commercial");
    let mut done = Vec::new();
    let force: &[&str] = if drop { &["--force"] } else { &[] };
    let mut checked = Vec::new();
    for (dir, branch, target) in [(ctx.repo.clone(), &l.branch, "work"), (ctx.commercial(), &l.cbranch, "main")] {
        if let Some(oid) = branch_tip(&dir, branch) {
            preserve_commit(&dir, &oid, target)?;
            let remote = git(&dir, &["rev-parse", "-q", "--verify", &format!("refs/remotes/origin/{branch}")]).ok();
            checked.push((dir, branch.clone(), oid, remote));
        }
    }

    if nested.join(".git").exists() {
        // removed from its own repository (the one its .git names), so neither keeps a dangling entry
        let common = wt::git(Some(&nested), None, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
        preserve_head(&nested, "main")?;
        let mut a = vec!["worktree", "remove"];
        a.extend_from_slice(force);
        let n = nested.to_string_lossy().into_owned();
        a.push(&n);
        wt::git(None, Some(Path::new(common.trim())), &a)?;
        ctx.monitor.forget(&nested);
    }
    if path.exists() {
        preserve_head(&path, "work")?;
        let mut a = vec!["worktree", "remove"];
        a.extend_from_slice(force);
        let p = path.to_string_lossy().into_owned();
        a.push(&p);
        wt::git(Some(&ctx.repo), None, &a)?;
    }
    ctx.monitor.forget(&path);
    done.push(format!("removed {}", path.display()));
    // landed (checked) or bundled (a drop): the branch goes even with the merge bot's merges on it
    if !branches {
        let mut kept: Vec<&str> = checked.iter().map(|c| c.1.as_str()).collect();
        kept.dedup();
        if !kept.is_empty() {
            done.push(format!("branch {} kept", kept.join(", ")));
        }
        checked.clear();
    }
    for (dir, b, oid, remote) in checked {
        match wt::git(Some(&dir), None, &["update-ref", "-d", &format!("refs/heads/{b}"), &oid]) {
            Ok(_) => done.push(format!("branch {b}")),
            Err(e) => { done.push(format!("branch {b} kept ({e})")); continue; }
        }
        if let Some(remote) = remote {
            match git(&dir, &["push", "-q", &format!("--force-with-lease=refs/heads/{b}:{remote}"), "origin", &format!(":refs/heads/{b}")]) {
                Ok(_) => done.push(format!("origin/{b}")),
                Err(e) => done.push(format!("origin/{b} kept ({e})")),
            }
        }
    }
    let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-lanes");
    let mut freed: Vec<(String, f64)> = Vec::new();
    for n in ctx.leases.nodes().into_iter().filter(|n| n.up) {
        if let Ok(lines) = crate::cli::ask(&c, Some(&n.name), "disk", &format!("disk clean {}", l.name)) {
            let mb = lines.first().and_then(|x| makepad_strict_json::parse(x.as_bytes()).ok()).and_then(|v| v.get("freed_mb").and_then(crate::lease::num)).unwrap_or(0.0);
            if mb >= 1.0 {
                freed.push((n.name.clone(), mb));
            }
        }
    }
    // (from +0: an empty f64 sum is -0, which printed as "-0 MB")
    let total = freed.iter().fold(0.0, |a, f| a + f.1);
    let each = if freed.is_empty() { String::new() } else { format!(": {}", freed.iter().map(|(n, mb)| format!("{n} {mb:.0}")).collect::<Vec<_>>().join(", ")) };
    done.push(format!("node dirs ({total:.0} MB{each})"));
    Ok(done.join(", "))
}

/// `lane new`: the worktree pair on `rc-<name>`.
fn create(ctx: &Ctx, name: &str, from: &str, commercial: bool) -> Result<LaneRec, String> {
    if !valid_name(name) {
        return Err(format!("{name}: a lane name is a-z, 0-9, - and _"));
    }
    let path = root(ctx).join(name);
    if path.exists() {
        return Err(format!("{} exists", path.display()));
    }
    let branch = format!("rc-{name}");
    if branch_tip(&ctx.repo, &branch).is_some() {
        return Err(format!("branch {branch} exists"));
    }
    if commercial && branch_tip(&ctx.commercial(), &branch).is_some() {
        return Err(format!("commercial branch {branch} exists"));
    }
    std::fs::create_dir_all(root(ctx)).map_err(|e| e.to_string())?;
    let p = path.to_string_lossy().into_owned();
    wt::git(Some(&ctx.repo), None, &["worktree", "add", "-q", "-b", &branch, &p, from])?;
    if commercial {
        let n = path.join("apps/commercial").to_string_lossy().into_owned();
        if let Err(e) = wt::git(Some(&ctx.commercial()), None, &["worktree", "add", "-q", "-b", &branch, &n, "main"]) {
            // undo the half made: a fresh worktree and a branch with no commits of its own
            let _ = wt::git(Some(&ctx.repo), None, &["worktree", "remove", "--force", &p]);
            let _ = wt::git(Some(&ctx.repo), None, &["branch", "-D", &branch]);
            return Err(e);
        }
    }
    let l = LaneRec { name: name.into(), path: p, branch: branch.clone(), cbranch: if commercial { branch } else { String::new() }, created_ms: wall_ms(), state: "live".into(), ..Default::default() };
    ctx.store.update(|d| d.lanes.push(l.clone()))?;
    crate::ci_merge::install_merge_driver(&path, &path)?;
    if commercial {
        crate::ci_merge::install_merge_driver(&path.join("apps/commercial"), &path)?;
    }
    let _ = ctx.monitor.dirty(&path);
    if commercial {
        let _ = ctx.monitor.dirty(&path.join("apps/commercial"));
    }
    Ok(l)
}

/// `lane drop`: bundle what is not landed, then remove everything.
fn drop_lane(ctx: &Ctx, name: &str) -> Result<String, String> {
    let l = rec(ctx, name).ok_or_else(|| format!("no live lane {name} (devhub lanes)"))?;
    let path = PathBuf::from(&l.path);
    let live = wt::live_in(&wt::live_cwds(), &path);
    if !live.is_empty() {
        return Err(format!("process(es) {live:?} run inside {}: stop them first", path.display()));
    }
    let dir = ctx.home.join("dropped");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = super::stamp(wall_ms());
    let mut kept = Vec::new();
    for (tree, repo, branch, target, half) in [(path.clone(), ctx.repo.clone(), l.branch.clone(), "work", "makepad"), (path.join("apps/commercial"), ctx.commercial(), l.cbranch.clone(), "main", "commercial")] {
        if !tree.join(".git").exists() {
            continue;
        }
        // git itself before anything is destroyed (the monitor may not have heard the last write yet)
        if wt::dirty(&tree, if half == "makepad" { Some("apps/commercial") } else { None }) {
            wt::git(Some(&tree), None, &["add", "-A"])?;
            wt::git(Some(&tree), None, &["commit", "-q", "-m", &format!("devhub lane drop {name}: uncommitted work")])?;
        }
        // a detached commercial half: its HEAD
        let what = if branch.is_empty() { git(&tree, &["rev-parse", "HEAD"])? } else { branch.clone() };
        // what it holds beyond its target decides the bundle: a count that does not read stops the drop
        let count = git(&repo, &["rev-list", "--count", &what, "--not", target])?;
        let n: u32 = count.trim().parse().map_err(|_| format!("{half} half of {name}: `git rev-list --count` said {:?}, not a count: nothing dropped", count.trim()))?;
        if n > 0 {
            let file = dir.join(format!("{name}-{half}-{stamp}.bundle"));
            let f = file.to_string_lossy().into_owned();
            let refname = if branch.is_empty() { format!("refs/heads/dropped-{name}") } else { String::new() };
            if !refname.is_empty() {
                git(&repo, &["update-ref", &refname, &what])?;
            }
            let r = git(&repo, &["bundle", "create", &f, if refname.is_empty() { &what } else { &refname }, "--not", target]);
            if !refname.is_empty() {
                let _ = git(&repo, &["update-ref", "-d", &refname]);
            }
            r?;
            kept.push(format!("{n} {half} commit(s) in {}", file.display()));
        }
    }
    let what = remove(ctx, &l, true, true)?;
    let note = if kept.is_empty() { format!("{what}; nothing unlanded to bundle") } else { format!("{what}; bundled {}", kept.join(", ")) };
    finish(ctx, name, "dropped", Ok(note.clone()));
    Ok(note)
}

/// One worktree half as `lane audit` finds it.
struct HalfAudit {
    path: PathBuf,
    nested: bool,
    branch: String,
    /// Ok: landed; Err: why not
    landed: Result<(), String>,
    /// its own commits that landed nowhere (merges and the bot's merges left out)
    unlanded: usize,
    /// uncommitted tracked paths, untracked (not ignored) paths
    tracked: Vec<String>,
    untracked: Vec<String>,
}

impl View {
    fn audit_commits(&mut self, path: &Path, branch: &str, head: ObjectId, check_own: bool, nested: bool) -> HalfAudit {
        let landed = self.landed(branch, path, check_own);
        let unlanded = self.unlanded(branch, head);
        HalfAudit { path: path.to_path_buf(), nested, branch: branch.to_string(), landed, unlanded, tracked: Vec::new(), untracked: Vec::new() }
    }
}

impl HalfAudit {
    /// The filesystem walk is independent of the shared history view.
    fn read_status(&mut self) {
        (self.tracked, self.untracked) = status_of(&self.path, self.nested);
    }
}

type AuditLane = (String, PathBuf, Option<(String, String, bool)>, Vec<(&'static str, HalfAudit)>);

/// `lane audit [--json]`: every worktree under the lanes root (both repos, registered or not):
/// whether each half landed (work / main, or a retired chain of them), its commits that landed
/// nowhere, its uncommitted tracked and untracked files, and what to do. Read in process (no git),
/// here (no master needed). Removes nothing.
pub fn audit(repo: &Path, json: bool, out: &mut dyn Write) -> Result<(), String> {
    let write_line = |out: &mut dyn Write, line: &str| -> Result<(), String> {
        writeln!(out, "{line}").and_then(|_| out.flush()).map_err(|e| format!("audit output: {e}"))
    };
    let root = wt::root_of(repo).canonicalize().map_err(|e| format!("lanes root: {e}"))?;
    // (git's paths have no Windows `\\?\` in front)
    let root = root.to_str().and_then(|r| r.strip_prefix(r"\\?\")).map_or_else(|| root.clone(), PathBuf::from);
    let mut mk = View::open(repo, "work")?;
    let commercial = repo.join("apps/commercial");
    let mut com = if commercial.join(".git").exists() { Some(View::open(&commercial, "main")?) } else { None };
    // registered lanes, from the master's state (read only)
    let state = crate::state::committed(&crate::proto::home())?;
    let mut registered: HashMap<PathBuf, (String, String, bool)> = HashMap::new();
    for l in state.as_ref().and_then(|v| v.get("lanes")).and_then(|v| v.as_arr()).unwrap_or(&[]) {
        let s = |k: &str| l.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        if s("state") == "live" {
            registered.insert(PathBuf::from(s("path")), (s("name"), s("note"), l.get("keep").and_then(|x| x.as_bool()).unwrap_or(false)));
        }
    }
    let list = |v: &View| -> Result<Vec<(PathBuf, String, ObjectId)>, String> {
        Ok(v.repo.lock().unwrap_or_else(|e| e.into_inner()).worktree_list().map_err(|e| e.to_string())?.into_iter().filter(|w| !w.main && w.path.starts_with(&root) && w.path.is_dir()).filter_map(|w| Some((w.path, w.branch.unwrap_or_default(), w.head?))).collect())
    };
    let mk_list = list(&mk)?;
    let mut com_list = match &com { Some(c) => list(c)?, None => Vec::new() };
    let total = mk_list.len() + com_list.iter().filter(|c| !mk_list.iter().any(|m| m.0.join("apps/commercial") == c.0)).count();
    if !json {
        write_line(out, &format!("{total} lane worktree(s) under {}:", root.display()))?;
    }
    // (name, path, registered note, kept, halves)
    let mut lanes: Vec<AuditLane> = Vec::new();
    for (path, branch, head) in mk_list {
        let name = path.strip_prefix(&root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        let nested = path.join("apps/commercial");
        let mut halves = vec![("makepad", mk.audit_commits(&path, &branch, head, true, nested.join(".git").exists()))];
        if let Some(i) = com_list.iter().position(|c| c.0 == nested) {
            let (cp, cb, ch) = com_list.swap_remove(i);
            if let Some(c) = com.as_mut() {
                halves.push(("commercial", c.audit_commits(&cp, &cb, ch, false, false)));
            }
        }
        let reg = registered.get(&path).cloned();
        lanes.push((name, path, reg, halves));
    }
    for (path, branch, head) in com_list {
        let name = path.strip_prefix(&root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(c) = com.as_mut() {
            let h = c.audit_commits(&path, &branch, head, true, false);
            lanes.push((name, path.clone(), registered.get(&path).cloned(), vec![("commercial", h)]));
        }
    }
    lanes.sort_by(|a, b| a.0.cmp(&b.0));
    // (the header's count came before the halves were read; the streaming waits for these)
    let count = lanes.len();
    // the class of each lane: what keeps it, most serious first
    let class = |halves: &[(&str, HalfAudit)]| -> &'static str {
        if halves.iter().any(|(_, h)| h.unlanded > 0) {
            "unlanded commits"
        } else if halves.iter().any(|(_, h)| !h.tracked.is_empty()) {
            "uncommitted changes"
        } else if halves.iter().any(|(_, h)| h.landed.is_err()) {
            "nothing of its own"
        } else if halves.iter().any(|(_, h)| !h.untracked.is_empty()) {
            "landed, untracked files only"
        } else {
            "landed and clean"
        }
    };
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    let mut rows = Vec::new();
    let mut removable = Vec::new();
    let workers = std::thread::available_parallelism().map_err(|e| format!("audit workers: {e}"))?.get().min(4).min(count);
    let (tasks, jobs) = std::sync::mpsc::sync_channel::<(usize, AuditLane)>(workers);
    let jobs = Mutex::new(jobs);
    let (results, finished) = std::sync::mpsc::channel();
    std::thread::scope(|scope| -> Result<(), String> {
        for _ in 0..workers {
            let jobs = &jobs;
            let results = results.clone();
            scope.spawn(move || loop {
                let job = jobs.lock().unwrap().recv();
                let Ok((index, mut lane)) = job else { break };
                for (_, half) in &mut lane.3 {
                    half.read_status();
                }
                if results.send((index, lane)).is_err() { break; }
            });
        }
        drop(results);
        // At most one window of work is outstanding, including completed results waiting for
        // an earlier name. Slow worktrees apply backpressure instead of growing the buffer.
        let tasks = tasks;
        let mut pending = lanes.into_iter().enumerate();
        for job in pending.by_ref().take(workers) {
            tasks.send(job).map_err(|e| format!("audit worker: {e}"))?;
        }
        let mut ready = BTreeMap::new();
        let mut next = 0;
        while next < count {
            let (index, lane) = finished.recv().map_err(|e| format!("audit worker: {e}"))?;
            ready.insert(index, lane);
            while let Some((name, path, reg, halves)) = ready.remove(&next) {
                let (name, path, reg, halves) = (&name, &path, &reg, &halves);
                let c = class(halves);
                *counts.entry(c).or_default() += 1;
                if json {
                    let hs: Vec<String> = halves.iter().map(|(repo, h)| format!(
                        "{{\"repo\":\"{repo}\",\"branch\":{},\"landed\":{},\"why\":{},\"unlanded\":{},\"tracked\":{},\"untracked\":{},\"untracked_files\":[{}]}}",
                        json_str(&h.branch), h.landed.is_ok(), json_str(h.landed.as_ref().err().map_or("", |s| s.as_str())), h.unlanded, h.tracked.len(), h.untracked.len(),
                        h.untracked.iter().take(20).map(|f| json_str(f)).collect::<Vec<_>>().join(","))).collect();
                    rows.push(format!("{{\"name\":{},\"path\":{},\"class\":\"{c}\",\"registered\":{},\"keep\":{},\"note\":{},\"halves\":[{}]}}",
                        json_str(name), json_str(&path.to_string_lossy()), reg.is_some(), reg.as_ref().is_some_and(|r| r.2), json_str(reg.as_ref().map_or("", |r| r.1.as_str())), hs.join(",")));
                } else {
                    let detail: Vec<String> = halves.iter().map(|(repo, h)| {
                        let mut d = format!("{repo} {}", if h.branch.is_empty() { "(detached)" } else { &h.branch });
                        if h.unlanded > 0 { d.push_str(&format!(" {} unlanded commit(s)", h.unlanded)); }
                        if !h.tracked.is_empty() { d.push_str(&format!(" {} uncommitted tracked", h.tracked.len())); }
                        for error in h.tracked.iter().filter(|p| p.starts_with("(status failed:") || p.as_str() == "(no git dir)") { d.push_str(&format!(" {error}")); }
                        if !h.untracked.is_empty() { d.push_str(&format!(" {} untracked ({})", h.untracked.len(), h.untracked.iter().take(3).cloned().collect::<Vec<_>>().join(", "))); }
                        if let Err(w) = &h.landed { if h.unlanded == 0 { d.push_str(&format!(" [{w}]")); } }
                        d
                    }).collect();
                    write_line(out, &format!("{name}  {c}: {}{}", detail.join("; "), if reg.is_none() { " (unregistered)" } else if reg.as_ref().is_some_and(|r| r.2) { " (keep)" } else { "" }))?;
                }
                if c == "landed and clean" { removable.push(name.clone()); }
                next += 1;
                if let Some(job) = pending.next() {
                    tasks.send(job).map_err(|e| format!("audit worker: {e}"))?;
                }
            }
        }
        Ok(())
    })?;
    if json {
        return write_line(out, &format!("{{\"lanes\":[{}]}}", rows.join(",")));
    }
    write_line(out, &format!("Summary: {}", counts.iter().map(|(k, n)| format!("{n} {k}")).collect::<Vec<_>>().join(", ")))?;
    write_line(out, &format!("landed and clean ({}): {}", removable.len(), removable.join(", ")))?;
    write_line(out, "landed and clean: `makepad devhub lane remove <name>` removes each (worktrees, branches, node dirs). Nothing here was removed.")
}

/// `lane adopt`: registers the live worktrees under the lanes root; lists the landed ones.
fn adopt(ctx: &Ctx) -> Result<String, String> {
    let infos = wt::infos(&ctx.repo)?;
    let work = View::open(&ctx.repo, "work")?;
    let known: HashSet<String> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live").map(|l| l.path.clone()).collect());
    let (mut registered, mut landed) = (Vec::new(), Vec::new());
    for i in infos {
        let p = i.entry.path.to_string_lossy().into_owned();
        if i.entry.path.join(".git").exists() {
            crate::ci_merge::install_merge_driver(&i.entry.path, &i.entry.path)?;
            let nested = i.entry.path.join("apps/commercial");
            if nested.join(".git").exists() {
                crate::ci_merge::install_merge_driver(&nested, &i.entry.path)?;
            }
        }
        if known.contains(&p) || !i.entry.path.is_dir() {
            continue;
        }
        let tip_landed = i.merged && !ObjectId::from_hex(&i.entry.head).is_ok_and(|h| work.first_parent(&h));
        let com_ok = !matches!(i.commercial, Some((true, _)) | Some((_, true)));
        if tip_landed && !i.dirty && com_ok {
            landed.push(format!("{{\"name\":{},\"path\":{},\"branch\":{},\"live\":{}}}", json_str(&i.lane), json_str(&p), json_str(i.entry.branch.as_deref().unwrap_or("")), i.live.len()));
            continue;
        }
        let nested = i.entry.path.join("apps/commercial");
        let cbranch = if nested.join(".git").exists() { crate::monitor::read_head(&nested).map(|h| h.1).unwrap_or_default() } else { String::new() };
        let l = LaneRec { name: i.lane.clone(), path: p.clone(), branch: i.entry.branch.clone().unwrap_or_default(), cbranch, created_ms: wall_ms(), adopted: true, state: "live".into(), ..Default::default() };
        registered.push(json_str(&l.name));
        ctx.store.update(|d| d.lanes.push(l))?;
    }
    event(ctx, Sev::Info, format!("lane adopt: {} registered, {} landed listed for removal (nothing removed)", registered.len(), landed.len()), None);
    Ok(format!("{{\"registered\":[{}],\"landed\":[{}]}}", registered.join(","), landed.join(",")))
}

/// `lane remove <name>`: a landed lane now (the user agreed), with every check but the grace.
fn remove_now(ctx: &Ctx, name: &str) -> Result<String, String> {
    let l = match rec(ctx, name) {
        Some(l) => l,
        None => {
            // a landed worktree adoption listed (never registered)
            let path = root(ctx).join(name);
            let head = crate::monitor::read_head(&path).ok_or_else(|| format!("no lane {name}"))?;
            let nested = path.join("apps/commercial");
            let cbranch = crate::monitor::read_head(&nested).map(|h| h.1).unwrap_or_default();
            let l = LaneRec { name: name.into(), path: path.to_string_lossy().into_owned(), branch: head.1, cbranch, created_ms: wall_ms(), adopted: true, state: "live".into(), ..Default::default() };
            ctx.store.update(|d| d.lanes.push(l.clone()))?;
            l
        }
    };
    let mut views = Views::open(ctx)?;
    if !only_the_record(ctx, &l, &views) {
        removable(ctx, &l, &mut views, &wt::live_cwds(), &land_entries(ctx), true)?;
    }
    let what = remove(ctx, &l, false, true);
    let out = what.clone();
    finish(ctx, name, "removed", what);
    out
}

/// One row of `devhub lanes`.
fn row(ctx: &Ctx, l: &LaneRec, merge: &[String], land: &[String]) -> String {
    let path = Path::new(&l.path);
    // what is known: a tree whose baseline is not read yet says so and is read in the background
    let reading = || (crate::monitor::read_head(path).map_or(String::new(), |(h, _)| h), "reading".to_string());
    let (tip, dirty) = match ctx.monitor.dirty_known(path) {
        Some(Ok(d)) => {
            let mut n = Some(d.changed.len() + d.deleted.len());
            if path.join("apps/commercial/.git").exists() {
                n = match ctx.monitor.dirty_known(&path.join("apps/commercial")) {
                    Some(Ok(c)) => n.map(|n| n + c.changed.len() + c.deleted.len()),
                    Some(Err(_)) => n,
                    None => None,
                };
            }
            match n {
                Some(n) => (d.head, n.to_string()),
                None => reading(),
            }
        }
        Some(Err(e)) => (String::new(), format!("? ({e})")),
        None => reading(),
    };
    let find = |lines: &[String], k: &str, b: &str| -> Option<makepad_strict_json::Value> { lines.iter().rev().filter_map(|x| makepad_strict_json::parse(x.as_bytes()).ok()).find(|v| v.get(k).and_then(|x| x.as_str()) == Some(b)) };
    let word = |v: &Option<makepad_strict_json::Value>, k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(|x| x.as_str()).unwrap_or("-").to_string();
    let m = find(merge, "branch", &l.branch);
    let ld = find(land, "branch", &l.branch);
    let job = ctx.store.read(|d| d.jobs.iter().rev().find(|j| j.lane == l.name).map(|j| format!("{} {}", j.id, j.state))).unwrap_or_else(|| "-".into());
    format!(
        "{{\"name\":{},\"path\":{},\"branch\":{},\"cbranch\":{},\"tip\":{},\"dirty\":{},\"merge\":{},\"land\":{},\"job\":{},\"keep\":{},\"waited\":{},\"parked\":{},\"note\":{}}}",
        json_str(&l.name),
        json_str(&l.path),
        json_str(&l.branch),
        json_str(&l.cbranch),
        json_str(&tip),
        json_str(&dirty),
        json_str(&word(&m, "result")),
        json_str(&word(&ld, "state")),
        json_str(&job),
        l.keep,
        l.waited,
        json_str(&l.parked),
        json_str(&l.note)
    )
}

// ---- the sweep: what else lives under the lanes root ----

/// One thing under the lanes root that is no makepad lane.
#[derive(Debug)]
struct Item {
    name: String,
    path: PathBuf,
    /// `worktree of <repo>` | `clone` | `no git`
    kind: String,
    /// the repo whose objects hold its commits (a worktree's common dir; a clone itself)
    git: Option<PathBuf>,
    clone: bool,
}

/// What the sweep does with one item.
#[derive(Debug, Default)]
struct Plan {
    /// commits on no kept ref: bundled first
    unique: u32,
    /// dirty and untracked files (paths from the item), archived first
    files: Vec<String>,
    /// why it stays (a process inside, a live lane, git failed)
    keep: String,
}

/// The master's own context as an Arc (a sweep runs on its own thread).
fn ctx_arc(_: &Ctx) -> Result<Arc<Ctx>, String> {
    super::ctx().cloned().ok_or_else(|| "no master context".to_string())
}

/// A folder the sweep deletes rather than archives (the user's policy, 2026-10-30): build output
/// (`target/`, a cargo target dir: `.fingerprint` or `CACHEDIR.TAG` inside), `*.noindex`, the
/// asset and JIT caches, and test-run homes (`homes/`, `userhome/`; `c1`..`c9` run homes in
/// folders with no git). Sources, scripts, logs, repro inputs and rendered media (`*.renders`)
/// are archived.
fn cache_dir(path: &Path, name: &str, no_git: bool) -> bool {
    matches!(name, "target" | "cache" | ".cache" | "__pycache__" | "asset-cache" | "sandbox-prepared-assets" | "diffcache" | "diff_fast_cache" | "jit-cache" | "jit-caches" | "homes" | "userhome")
        || name.ends_with(".noindex")
        || name.starts_with("stdcache-")
        || (no_git && name.len() == 2 && name.as_bytes()[0] == b'c' && (b'1'..=b'9').contains(&name.as_bytes()[1]))
        || path.join(".fingerprint").is_dir()
        || path.join("CACHEDIR.TAG").is_file()
}

/// Bytes on disk of a file's metadata (its blocks; its length where there are none).
fn disk_bytes(m: &std::fs::Metadata) -> u64 {
    #[cfg(unix)]
    {
        std::os::unix::fs::MetadataExt::blocks(m) * 512
    }
    #[cfg(not(unix))]
    {
        m.len()
    }
}

/// Bytes on disk under `path` (no symlink followed).
fn tree_bytes(path: &Path) -> u64 {
    let Ok(m) = std::fs::symlink_metadata(path) else { return 0 };
    let mut n = disk_bytes(&m);
    if m.is_dir() {
        for e in std::fs::read_dir(path).into_iter().flatten().flatten() {
            n += tree_bytes(&e.path());
        }
    }
    n
}

/// The files under `root.join(rel)` the archive takes (paths relative to `root`), skipping the
/// cache folders; their bytes.
fn archive_files(root: &Path, rel: &str, no_git: bool, out: &mut Vec<String>) -> u64 {
    let path = root.join(rel);
    let Ok(m) = std::fs::symlink_metadata(&path) else { return 0 };
    if !m.is_dir() {
        out.push(rel.to_string());
        return disk_bytes(&m);
    }
    let mut n = 0;
    let mut kids: Vec<String> = std::fs::read_dir(&path).into_iter().flatten().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    kids.sort();
    for k in kids {
        let p = path.join(&k);
        let is_dir = std::fs::symlink_metadata(&p).is_ok_and(|m| m.is_dir());
        if is_dir && cache_dir(&p, &k, no_git) {
            continue;
        }
        n += archive_files(root, &format!("{rel}/{k}"), no_git, out);
    }
    n
}

/// Whether a path from an item (`a/b/c`) lies in a cache folder.
fn in_cache(item: &Path, rel: &str) -> bool {
    let mut p = item.to_path_buf();
    let parts: Vec<&str> = rel.split('/').collect();
    for c in &parts[..parts.len().saturating_sub(1)] {
        p.push(c);
        if cache_dir(&p, c, false) {
            return true;
        }
    }
    false
}

fn gb(n: u64) -> String {
    format!("{:.1} GB", n as f64 / 1e9)
}

fn sweep_dir(ctx: &Ctx) -> PathBuf {
    ctx.home.join("sweep")
}

/// The kept refs a commit must reach to count as in git: work, main and every remote branch of
/// the repo (a worktree's branch ref stays in its repo, but a detached HEAD's commits would not).
fn kept_refs(git_dir: &Path) -> Vec<String> {
    let mut v = vec!["--remotes".to_string()];
    for b in ["work", "main"] {
        if wt::git(None, Some(git_dir), &["rev-parse", "-q", "--verify", &format!("refs/heads/{b}")]).is_ok() {
            v.push(format!("refs/heads/{b}"));
        }
    }
    v
}

fn classify(ctx: &Ctx, path: &Path) -> Item {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let dotgit = path.join(".git");
    if dotgit.is_dir() {
        return Item { name, path: path.to_path_buf(), kind: "clone".into(), git: Some(dotgit), clone: true };
    }
    if dotgit.is_file() {
        if let Ok(c) = wt::git(Some(path), None, &["rev-parse", "--path-format=absolute", "--git-common-dir"]) {
            let common = PathBuf::from(c.trim());
            let owner = common.parent().map(|p| p.to_path_buf()).unwrap_or_default();
            let mk = std::fs::canonicalize(ctx.repo.join(".git")).ok();
            if std::fs::canonicalize(&common).ok() == mk {
                return Item { name, path: path.to_path_buf(), kind: "makepad worktree".into(), git: Some(common), clone: false };
            }
            return Item { name, path: path.to_path_buf(), kind: format!("worktree of {}", owner.display()), git: Some(common), clone: false };
        }
        return Item { name, path: path.to_path_buf(), kind: "broken worktree (its repo is gone)".into(), git: None, clone: false };
    }
    Item { name, path: path.to_path_buf(), kind: "no git".into(), git: None, clone: false }
}

fn plan_of(item: &Item, cwds: &[(u32, PathBuf)], live: &HashSet<PathBuf>) -> Plan {
    let mut p = Plan::default();
    let canon = std::fs::canonicalize(&item.path).unwrap_or_else(|_| item.path.clone());
    if live.iter().any(|l| l.starts_with(&canon) || canon.starts_with(l)) {
        p.keep = "a live lane".into();
        return p;
    }
    let procs = wt::live_in(cwds, &canon);
    if !procs.is_empty() {
        p.keep = format!("process(es) {procs:?} run inside");
        return p;
    }
    let Some(gd) = &item.git else { return p };
    let what: Vec<String> = if item.clone { vec!["--branches".into(), "HEAD".into()] } else { vec!["HEAD".into()] };
    let mut args: Vec<String> = vec!["rev-list".into(), "--count".into()];
    args.extend(what);
    args.push("--not".into());
    args.extend(kept_refs(gd));
    let a: Vec<&str> = args.iter().map(String::as_str).collect();
    match wt::git(Some(&item.path), None, &a) {
        Ok(n) => match n.trim().parse() {
            Ok(n) => p.unique = n,
            // what it holds outside git decides the bundle: a count that does not read keeps it
            Err(_) => {
                p.keep = format!("git rev-list --count said {:?}, not a count", crate::proto::clean(n.trim(), 80));
                return p;
            }
        },
        // no commit at all (an empty repo): nothing to bundle
        Err(e) if e.contains("ambiguous argument 'HEAD'") || e.contains("unknown revision") => {}
        Err(e) => {
            p.keep = format!("git rev-list: {}", crate::proto::clean(&e, 160));
            return p;
        }
    }
    match wt::git(Some(&item.path), None, &["status", "--porcelain", "-z", "--untracked-files=all", "--ignore-submodules=all"]) {
        Ok(st) => {
            let mut it = st.split('\0').filter(|s| !s.is_empty());
            while let Some(e) = it.next() {
                let (code, rel) = (e.get(..2).unwrap_or(""), e.get(3..).unwrap_or(""));
                // a rename names its source next: the new path is what exists
                if code.starts_with('R') || code.starts_with('C') {
                    let _ = it.next();
                }
                if !rel.is_empty() && std::fs::symlink_metadata(item.path.join(rel)).is_ok() {
                    p.files.push(rel.to_string());
                }
            }
        }
        Err(e) => p.keep = format!("git status: {}", crate::proto::clean(&e, 160)),
    }
    p
}

/// A sweep runs: the master does not restart under it (its archive and its deletions are one step).
static RUNNING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn sweeping() -> bool {
    RUNNING.load(std::sync::atomic::Ordering::Acquire)
}

/// `lane sweep [--dry]`: everything under the lanes root that is no makepad lane — standalone
/// worktrees of other repos (apps/commercial, webserver), separate clones, folders with no git —
/// goes, after ONE archive (`~/makepad-archive/wt-noindex-<date>.tar.xz`) took what is not in git:
/// a bundle of each repo's commits on no kept ref (work, main, remote branches), its dirty and
/// untracked files, and every no-git folder whole, except the folders [`cache_dir`] names (build
/// output, caches, run homes: just deleted). The report says the bytes: on disk, archive input,
/// deleted without archive. Makepad worktrees are `lane adopt`/`remove`'s; a live lane or a
/// folder a process runs in stays. Runs in the background: `lane sweep status` reads its report.
fn sweep(ctx: &Arc<Ctx>, dry: bool) -> Result<String, String> {
    if RUNNING.swap(true, std::sync::atomic::Ordering::AcqRel) {
        return Err("a sweep runs already (devhub lane sweep status)".into());
    }
    let dir = sweep_dir(ctx).join(super::stamp(wall_ms()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let report = dir.join("report.txt");
    let (c, d2) = (ctx.clone(), dir.clone());
    let r = std::thread::Builder::new().name("devhub-sweep".into()).spawn(move || {
        let r = sweep_run(&c, &d2, dry);
        let text = match &r {
            Ok(t) => t.clone(),
            Err(e) => format!("sweep FAILED: {e}\n"),
        };
        let _ = std::fs::write(d2.join("report.txt"), &text);
        let _ = std::fs::write(sweep_dir(&c).join("last"), d2.to_string_lossy().as_bytes());
        event(&c, if r.is_ok() { Sev::Info } else { Sev::Err }, format!("lane sweep{}: {}", if dry { " (dry)" } else { "" }, text.lines().next().unwrap_or("")), None);
        RUNNING.store(false, std::sync::atomic::Ordering::Release);
    });
    if let Err(e) = r {
        RUNNING.store(false, std::sync::atomic::Ordering::Release);
        return Err(e.to_string());
    }
    let _ = std::fs::write(sweep_dir(ctx).join("last"), dir.to_string_lossy().as_bytes());
    Ok(format!("{{\"started\":true,\"dry\":{dry},\"report\":{}}}", json_str(&report.to_string_lossy())))
}

/// Free bytes of the volume holding `p`.
fn free_bytes(p: &Path) -> u64 {
    #[cfg(unix)]
    {
        let o = std::process::Command::new("df").arg("-k").arg(p).output();
        if let Ok(o) = o {
            let t = String::from_utf8_lossy(&o.stdout);
            if let Some(l) = t.lines().nth(1) {
                return l.split_whitespace().nth(3).and_then(|n| n.parse::<u64>().ok()).unwrap_or(0) * 1024;
            }
        }
    }
    let _ = p;
    0
}

fn sweep_run(ctx: &Ctx, dir: &Path, dry: bool) -> Result<String, String> {
    let root = std::fs::canonicalize(root(ctx)).map_err(|e| e.to_string())?;
    // the makepad lanes (registered or not) are adopt/remove's; the live ones are never touched
    let mut live: HashSet<PathBuf> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live").map(|l| PathBuf::from(&l.path)).collect());
    live.extend(ctx.store.read(|d| d.jobs.iter().filter(|j| !j.done()).map(|j| PathBuf::from(&j.repo_dir)).chain(d.lives.iter().filter(|x| x.active()).map(|x| PathBuf::from(&x.tree))).collect::<Vec<_>>()));
    let live: HashSet<PathBuf> = live.into_iter().map(|p| std::fs::canonicalize(&p).unwrap_or(p)).collect();
    let cwds = wt::live_cwds();
    let mut entries: Vec<PathBuf> = std::fs::read_dir(&root).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    entries.sort();
    let (mut kept, mut gone, mut lines) = (Vec::new(), Vec::new(), Vec::new());
    let (mut bundles, mut archived): (Vec<String>, Vec<String>) = (Vec::new(), Vec::new());
    // bytes: of the items that go, of what the archive takes from them
    let (mut total, mut input) = (0u64, 0u64);
    let bdir = dir.join("bundles");
    let mut counts: std::collections::BTreeMap<String, (u32, u32)> = Default::default();
    for path in entries {
        let item = classify(ctx, &path);
        let cat = if item.kind.starts_with("worktree of") { item.kind.clone() } else { item.kind.clone() };
        if item.kind == "makepad worktree" {
            continue;
        }
        let plan = plan_of(&item, &cwds, &live);
        let c = counts.entry(cat.clone()).or_default();
        if !plan.keep.is_empty() {
            c.1 += 1;
            kept.push(format!("KEPT  {:<36} {:<40} {}", item.name, item.kind, plan.keep));
            continue;
        }
        c.0 += 1;
        let mut note = String::new();
        if plan.unique > 0 {
            note.push_str(&format!("{} unique commit(s) bundled; ", plan.unique));
            if !dry {
                std::fs::create_dir_all(&bdir).map_err(|e| e.to_string())?;
                let f = bdir.join(format!("{}.bundle", item.name));
                let fs = f.to_string_lossy().into_owned();
                let mut args: Vec<String> = vec!["bundle".into(), "create".into(), "-q".into(), fs];
                if item.clone {
                    args.push("--branches".into());
                }
                args.push("HEAD".into());
                args.push("--not".into());
                args.extend(kept_refs(item.git.as_ref().unwrap()));
                let a: Vec<&str> = args.iter().map(String::as_str).collect();
                if let Err(e) = wt::git(Some(&item.path), None, &a) {
                    c.0 -= 1;
                    c.1 += 1;
                    kept.push(format!("KEPT  {:<36} {:<40} bundle failed: {}", item.name, item.kind, crate::proto::clean(&e, 160)));
                    continue;
                }
                bundles.push(format!("bundles/{}.bundle", item.name));
            }
        }
        let size = tree_bytes(&item.path);
        let mut took = 0u64;
        if item.git.is_none() {
            took = archive_files(&root, &item.name, true, &mut archived);
            note.push_str(&format!("archived {} of {} (caches, run homes, build output deleted)", gb(took), gb(size)));
        } else if !plan.files.is_empty() {
            let files: Vec<&String> = plan.files.iter().filter(|f| !in_cache(&item.path, f)).collect();
            for f in &files {
                took += archive_files(&root, &format!("{}/{f}", item.name), false, &mut archived);
            }
            note.push_str(&format!("{} dirty/untracked file(s) archived ({}; {} in caches deleted)", files.len(), gb(took), plan.files.len() - files.len()));
        } else if plan.unique == 0 {
            note.push_str("all in git, clean");
        }
        total += size;
        input += took;
        lines.push(format!("{}  {:<36} {:<40} {note}", if dry { "WOULD" } else { "GONE " }, item.name, item.kind));
        gone.push(item);
    }
    let mut head = Vec::new();
    for (k, (g, s)) in &counts {
        head.push(format!("{k}: {g} {}, {s} kept", if dry { "would go" } else { "gone" }));
    }
    let sizes = format!("{} on disk: archive input {}, deleted without archive {}", gb(total), gb(input), gb(total.saturating_sub(input)));
    if dry {
        let mut out = format!("dry run: {} item(s) would go, {} stay ({})\n{sizes}\n", gone.len(), kept.len(), head.join("; "));
        out.push_str(&lines.join("\n"));
        out.push('\n');
        out.push_str(&kept.join("\n"));
        out.push('\n');
        return Ok(out);
    }
    // ONE archive of everything that is not in git, before anything is deleted
    let adir = crate::os::home_dir().join("makepad-archive");
    std::fs::create_dir_all(&adir).map_err(|e| e.to_string())?;
    // a second sweep of the day never overwrites the first one's archive
    let day = &super::stamp(wall_ms())[..8];
    let archive = (0..).map(|i| adir.join(if i == 0 { format!("wt-noindex-{day}.tar.xz") } else { format!("wt-noindex-{day}-{i}.tar.xz") })).find(|p| !p.exists()).unwrap();
    if !bundles.is_empty() || !archived.is_empty() {
        let list = dir.join("files.list");
        let mut text = Vec::new();
        for f in &archived {
            text.extend_from_slice(f.as_bytes());
            text.push(0);
        }
        std::fs::write(&list, &text).map_err(|e| e.to_string())?;
        let mut cmd = std::process::Command::new("nice");
        cmd.args(["tar", "-c", "-J", "--options", "xz:threads=0,xz:compression-level=3", "-f"]).arg(&archive).arg("--no-recursion");
        // one `-C` only (a second one would move the list's directory too): the bundles by their
        // full paths
        cmd.arg("-C").arg(&root).arg("--null").arg("-T").arg(&list);
        cmd.args(bundles.iter().map(|b| dir.join(b)));
        let o = cmd.stdin(std::process::Stdio::null()).output().map_err(|e| format!("tar: {e}"))?;
        if !o.status.success() {
            let _ = std::fs::remove_file(&archive);
            return Err(format!("tar failed (nothing deleted): {}", crate::proto::clean(&String::from_utf8_lossy(&o.stderr), 400)));
        }
        // the archive reads back before anything goes
        let t = std::process::Command::new("tar").arg("-t").arg("-f").arg(&archive).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).output().map_err(|e| e.to_string())?;
        if !t.status.success() {
            return Err(format!("the archive does not read back (nothing deleted): {}", crate::proto::clean(&String::from_utf8_lossy(&t.stderr), 300)));
        }
    }
    let before = free_bytes(&root);
    let mut failed = Vec::new();
    for item in &gone {
        // a worktree leaves its repo's admin entry with it
        let r = match (&item.git, item.clone) {
            (Some(gd), false) => wt::git(None, Some(gd), &["worktree", "remove", "--force", "--force", &item.path.to_string_lossy()]).map(|_| ()).or_else(|_| std::fs::remove_dir_all(&item.path).map_err(|e| e.to_string())),
            _ => {
                if item.path.is_dir() {
                    std::fs::remove_dir_all(&item.path).map_err(|e| e.to_string())
                } else {
                    std::fs::remove_file(&item.path).map_err(|e| e.to_string())
                }
            }
        };
        if let Err(e) = r {
            failed.push(format!("{}: {e}", item.name));
        }
    }
    // the repos whose worktrees went forget them
    let repos: HashSet<PathBuf> = gone.iter().filter(|i| !i.clone).filter_map(|i| i.git.clone()).collect();
    for gd in repos {
        let _ = wt::git(None, Some(&gd), &["worktree", "prune"]);
    }
    let freed = free_bytes(&root).saturating_sub(before);
    let size = std::fs::metadata(&archive).map(|m| m.len()).unwrap_or(0);
    let mut out = format!(
        "swept {} item(s), {} kept, {:.1} GB freed; archive {} ({:.2} GB){}\n",
        gone.len() - failed.len(),
        kept.len(),
        freed as f64 / 1e9,
        archive.display(),
        size as f64 / 1e9,
        if failed.is_empty() { String::new() } else { format!("; {} failed", failed.len()) }
    );
    out.push_str(&format!("{}\n{sizes}\n", head.join("; ")));
    out.push_str(&lines.join("\n"));
    out.push('\n');
    out.push_str(&kept.join("\n"));
    out.push('\n');
    for f in failed {
        out.push_str(&format!("FAILED {f}\n"));
    }
    Ok(out)
}

/// How long after its last job or lease ended a lane is quiet, with nothing newer from it.
const QUIET_MS: u64 = 15 * 60_000;
/// How long it stays listed: past this it is parked, not a wait that never woke it.
const QUIET_LISTED_MS: u64 = 12 * 3_600_000;
/// How long a lane had nothing (no job or lease, no commit, no file written) before `lane quiet`
/// offers its retirement, when the agent of its last run is silent too.
const RETIRE_OFFER_MS: u64 = 6 * 3_600_000;

/// `lane quiet`: for the manager's check-in, the live lanes that may be waiting on a run that is
/// over (a watcher that stayed silent left two lanes idle, 11-04), the leases far over their
/// measure whose agent went silent ([`crate::lease::master::LeaseMaster::silent_overdue`]), the
/// live sessions whose restart waited past 5 min ([`super::live::restarts_pending`]) and the tests
/// run's units far over their measure ([`crate::bucket::slow_units`]). Some just work on code. Then the lanes offered for retirement ([`retire_offers`]) and the parked
/// ones, in one row.
fn quiet(ctx: &Ctx) -> Result<String, String> {
    let lanes: Vec<LaneRec> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.ended_ms == 0).cloned().collect());
    let ended = ended_runs(ctx, &lanes);
    let busy = in_flight(ctx);
    let landed: Vec<(String, String, u64)> = ctx.store.history("land")?.iter().filter_map(|v| crate::land::Entry::parse(&v.to_json())).map(|e| (e.branch, e.cbranch, e.asked_ms)).collect();
    let now = wall_ms();
    let mut rows = Vec::new();
    let detached = ctx.leases.detached(now);
    if !detached.is_empty() {
        rows.push("agents with running leases and no call of their own waiting on them:".to_string());
        rows.extend(detached);
    }
    let overdue = ctx.leases.silent_overdue(now);
    if !overdue.is_empty() {
        rows.push("leases far over their measure whose agent went silent:".to_string());
        rows.extend(overdue);
    }
    rows.extend(super::live::restarts_pending(ctx, now));
    let (leases, nodes) = (ctx.leases.view(), ctx.leases.nodes());
    let shard_box = |id: &str| {
        let node = leases.iter().find(|l| l.id == id && !matches!(l.state, crate::lease::sched::State::Ended(_)))?.node.clone()?;
        let caps = nodes.iter().find(|n| n.name == node).map(|n| (n.up, n.caps.as_ref()));
        Some(crate::bucket::ShardBox { up: caps.is_some_and(|(up, _)| up), load: caps.and_then(|(_, c)| c).map_or(0.0, |c| c.load1), cores: caps.and_then(|(_, c)| c).map_or(0, |c| c.cores), node })
    };
    rows.extend(crate::bucket::slow_units(&ctx.home, now, &shard_box));
    let offers = offered(ctx, &lanes, &ended, &busy, now);
    let waiting: Vec<LaneRec> = lanes.iter().filter(|l| l.parked.is_empty() && !offers.iter().any(|o| o.name == l.name)).cloned().collect();
    rows.extend(quiet_lanes(&waiting, &ended, &busy, &landed, now));
    // one row: 344 lanes came up at once, one row each, the first time (11-06)
    if !offers.is_empty() {
        let newest: Vec<String> = offers.iter().take(5).map(|o| format!("{} ({} h)", o.name, o.idle_ms / 3_600_000)).collect();
        rows.push(format!("offered for retirement (nothing of them for {} h, their agent gone): {} lane(s), the newest {}{}: `devhub lane offers` lists each (`--check`: what retiring them frees and what refuses), `devhub lane retire <name>` retires one", RETIRE_OFFER_MS / 3_600_000, offers.len(), newest.join(", "), if offers.len() > 5 { ", ..." } else { "" }));
    }
    let parked: Vec<String> = lanes.iter().filter(|l| !l.parked.is_empty()).map(|l| format!("{} ({})", l.name, l.parked)).collect();
    if !parked.is_empty() {
        rows.push(format!("parked: {} (the merge bot leaves them alone; `devhub lane unpark <name>`)", parked.join(", ")));
    }
    Ok(if rows.is_empty() { "no lane is quiet\n".to_string() } else { rows.join("\n") + "\n" })
}

/// The lanes with a lease or job in flight, by the lane name or branch they were asked under.
fn in_flight(ctx: &Ctx) -> HashSet<String> {
    let mut busy: HashSet<String> = ctx.leases.view().iter().filter(|x| !matches!(x.state, crate::lease::sched::State::Ended(_))).map(|x| x.req.lane.clone()).collect();
    ctx.store.read(|d| busy.extend(d.jobs.iter().filter(|j| !j.done()).map(|j| j.lane.clone())));
    busy
}

/// [`retire_offers`] with what the master knows of the lanes' activity: each branch's newest
/// commit, lease or job as the merge bot saw it, its newest file write, and its agents' calls.
fn offered(ctx: &Ctx, lanes: &[LaneRec], ended: &HashMap<String, Ended>, busy: &HashSet<String>, now: u64) -> Vec<Offer> {
    let mut active: HashMap<String, u64> = HashMap::new();
    ctx.store.read(|d| for m in &d.merges {
        if let (Some(b), Some(at)) = (m.get("branch").and_then(|x| x.as_str()), m.get("active_ms").and_then(|x| x.as_u64())) {
            let e = active.entry(b.to_string()).or_default();
            *e = (*e).max(at);
        }
    });
    for l in lanes {
        let wrote = ctx.monitor.last_event_ms(Path::new(&l.path)).max(ctx.monitor.last_event_ms(&Path::new(&l.path).join("apps/commercial")));
        let e = active.entry(l.branch.clone()).or_default();
        *e = (*e).max(wrote);
    }
    retire_offers(lanes, ended, busy, &active, &|a| ctx.leases.agent_seen_ms(a), now)
}

/// `lane offers`: each lane offered for retirement, with what of its runs would refuse it (a
/// process inside, a lease, job or hand-in open). Its worktree's status and size, and its node
/// dirs, are read by the asking CLI (`--check`), never on the master's control thread.
fn offers(ctx: &Ctx) -> String {
    let lanes: Vec<LaneRec> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.ended_ms == 0).cloned().collect());
    let offers = offered(ctx, &lanes, &ended_runs(ctx, &lanes), &in_flight(ctx), wall_ms());
    let (cwds, land) = (wt::live_cwds(), land_entries(ctx));
    let rows: Vec<String> = offers.iter().filter_map(|o| {
        let l = lanes.iter().find(|l| l.name == o.name)?;
        let (class, why) = run_refusal(ctx, l, &cwds, &land).map_or(("", String::new()), |(c, w)| (c, w));
        Some(format!("{{\"name\":{},\"path\":{},\"idle_ms\":{},\"row\":{},\"refused_class\":{},\"refused\":{}}}", json_str(&l.name), json_str(&l.path), o.idle_ms, json_str(&o.row), json_str(class), json_str(&why)))
    }).collect();
    format!("{{\"lanes\":[{}]}}", rows.join(","))
}

/// One lane [`retire_offers`] offers: its name, how long nothing came of it, and its row.
#[derive(Clone, Debug, PartialEq)]
struct Offer {
    name: String,
    idle_ms: u64,
    row: String,
}

/// What retiring the offered lanes would do (`offers`: the master's `lane offers` answer; `nodes`:
/// each node's `lane-dirs` answer, its lanes' dir bytes): read here, in the CLI's process, 4 lanes
/// at a time: which retire and the disk they free (the Mac's worktrees, the nodes' dirs), and which
/// refuse, by reason. Retires nothing.
pub fn check_offers(offers: &makepad_strict_json::Value, nodes: &[(String, Result<makepad_strict_json::Value, String>)]) -> String {
    let list = offers.get("lanes").and_then(|l| l.as_arr()).unwrap_or(&[]);
    let s = |v: &makepad_strict_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let todo: Vec<(String, String, String, String)> = list.iter().map(|v| (s(v, "name"), s(v, "path"), s(v, "refused_class"), s(v, "refused"))).collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    // (index, the refusal's class: empty when it retires, the worktree's bytes)
    let done: Mutex<Vec<(usize, String, u64)>> = Mutex::new(Vec::new());
    std::thread::scope(|sc| {
        for _ in 0..4 {
            sc.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let Some((_, path, class, _)) = todo.get(i) else { break };
                let path = Path::new(path);
                let class = dirty_refusal(path).map_or_else(|| class.clone(), |(c, _)| c.to_string());
                done.lock().unwrap().push((i, class, tree_bytes(path)));
            });
        }
    });
    let mut done = done.into_inner().unwrap();
    done.sort_by_key(|d| d.0);
    let mut node_bytes: HashMap<String, u64> = HashMap::new();
    let mut unread = Vec::new();
    for (node, r) in nodes {
        match r.as_ref().map(|v| v.get("lanes")) {
            Ok(Some(makepad_strict_json::Value::Obj(pairs))) => for (lane, b) in pairs { *node_bytes.entry(lane.clone()).or_default() += b.as_u64().unwrap_or(0); },
            Ok(_) => unread.push(format!("{node} (no lane sizes: an older hub)")),
            Err(e) => unread.push(format!("{node} ({e})")),
        }
    }
    let gb = |b: u64| format!("{:.1} GB", b as f64 / (1u64 << 30) as f64);
    let mut by: BTreeMap<&str, (Vec<&str>, u64, u64)> = BTreeMap::new();
    for (i, class, mac) in &done {
        let e = by.entry(class.as_str()).or_default();
        e.0.push(&todo[*i].0);
        e.1 += mac;
        e.2 += node_bytes.get(&todo[*i].0).copied().unwrap_or(0);
    }
    let mut out = format!("{} lane(s) offered for retirement (nothing retired)\n", todo.len());
    out.push_str(&format!("{:<34} {:>6} {:>10} {:>10}\n", "", "lanes", "Mac", "nodes"));
    for (class, (names, mac, node)) in &by {
        let what = if class.is_empty() { "retire cleanly" } else { class };
        out.push_str(&format!("{:<34} {:>6} {:>10} {:>10}\n", what, names.len(), gb(*mac), gb(*node)));
    }
    for (class, (names, ..)) in by.iter().filter(|(c, _)| !c.is_empty()) {
        out.push_str(&format!("{class}: {}{}\n", names.iter().take(12).copied().collect::<Vec<_>>().join(", "), if names.len() > 12 { ", ..." } else { "" }));
    }
    if !unread.is_empty() {
        out.push_str(&format!("node dirs not read: {}\n", unread.join(", ")));
    }
    out
}

#[derive(Clone, Debug, Default)]
struct Ended {
    at: u64,
    what: String,
    agent: String,
}

/// The newest job or lease of these lanes that ended, by lane name or branch.
fn ended_runs(ctx: &Ctx, lanes: &[LaneRec]) -> HashMap<String, Ended> {
    use std::io::BufRead;
    let names: HashSet<&str> = lanes.iter().flat_map(|l| [l.name.as_str(), l.branch.as_str()]).collect();
    let mut ended: HashMap<String, Ended> = HashMap::new();
    let mut note = |lane: &str, at: u64, what: String, agent: &str| {
        let last = ended.entry(lane.to_string()).or_default();
        if at >= last.at { *last = Ended { at, what, agent: agent.to_string() }; }
    };
    if let Ok(file) = std::fs::File::open(ctx.home.join("leases.jsonl")) {
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            // the cheap look first: the ledger holds every lease there was
            if !line.split("\"lane\":\"").nth(1).and_then(|r| r.split('"').next()).is_some_and(|l| names.contains(l)) { continue; }
            let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
            let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
            let agent = v.get("req").and_then(|r| r.get("agent")).and_then(|x| x.as_str()).unwrap_or("");
            note(text("lane"), v.get("ended_ms").and_then(|x| x.as_u64()).unwrap_or(0), format!("{} {}@{} {}", text("id"), text("kind"), text("node"), text("how")), agent);
        }
    }
    ctx.store.read(|d| for j in d.jobs.iter().filter(|j| j.done() && names.contains(j.lane.as_str())) {
        note(&j.lane, j.ended_ms, format!("{} {}", j.id, j.state), &j.agent);
    });
    ended
}

/// The lines of [`quiet`], the newest first. `ended`: the newest job or lease that ended, by lane
/// name or branch; `busy`: the lanes with one in flight; `landed`: each hand-in's branches and ask.
fn quiet_lanes(lanes: &[LaneRec], ended: &HashMap<String, Ended>, busy: &HashSet<String>, landed: &[(String, String, u64)], now: u64) -> Vec<String> {
    let mut rows: Vec<(u64, String)> = lanes.iter().filter(|l| !busy.contains(&l.name) && !busy.contains(&l.branch)).filter_map(|l| {
        let Ended { at, what, .. } = [&l.name, &l.branch].into_iter().filter_map(|n| ended.get(n)).max_by_key(|e| e.at)?;
        let age = now.saturating_sub(*at);
        let acted = landed.iter().any(|(b, c, asked)| *asked > *at && (*b == l.branch || (!c.is_empty() && *c == l.cbranch)));
        (age > QUIET_MS && age < QUIET_LISTED_MS && !acted).then(|| (*at, format!("{}: {what} ended {} min ago", l.name, age / 60_000)))
    }).collect();
    rows.sort_by(|a, b| b.0.cmp(&a.0));
    rows.into_iter().map(|(_, line)| line).collect()
}

/// The lanes [`quiet`] offers for retirement, as (name, row): live, not kept, parked or landed,
/// nothing in flight (`busy`), and nothing of it for [`RETIRE_OFFER_MS`]: no job or lease ended
/// (`ended`), no commit or file written (`active`, by branch), not made since; the agent of its last
/// run made no devhub call for [`crate::lease::master::AGENT_SILENT_MS`] (`seen`). The newest
/// first.
fn retire_offers(lanes: &[LaneRec], ended: &HashMap<String, Ended>, busy: &HashSet<String>, active: &HashMap<String, u64>, seen: &dyn Fn(&str) -> u64, now: u64) -> Vec<Offer> {
    let mut rows: Vec<Offer> = lanes.iter().filter(|l| l.state == "live" && !l.keep && l.parked.is_empty() && l.landed_ms == 0 && !busy.contains(&l.name) && !busy.contains(&l.branch)).filter_map(|l| {
        let run = [&l.name, &l.branch].into_iter().filter_map(|n| ended.get(n)).max_by_key(|e| e.at);
        let last = [l.created_ms, run.map_or(0, |r| r.at), [&l.branch, &l.cbranch].into_iter().filter_map(|b| active.get(b)).copied().max().unwrap_or(0)].into_iter().max().unwrap_or(0);
        if now.saturating_sub(last) < RETIRE_OFFER_MS {
            return None;
        }
        let agent = run.map_or("", |r| r.agent.as_str());
        if !agent.is_empty() && now.saturating_sub(seen(agent)) < crate::lease::master::AGENT_SILENT_MS {
            return None;
        }
        let ran = run.map_or("no job or lease".to_string(), |r| format!("last {}", r.what));
        let who = if agent.is_empty() { "no agent known".to_string() } else { format!("its agent {agent} is silent") };
        let idle_ms = now.saturating_sub(last);
        Some(Offer { name: l.name.clone(), idle_ms, row: format!("{}: nothing of it for {} h ({ran}), {who}: `devhub lane retire {}` removes its worktree, its branches stay", l.name, idle_ms / 3_600_000, l.name) })
    }).collect();
    rows.sort_by_key(|o| o.idle_ms);
    rows
}

/// `timeline <lane>`: where the lane's time since its start went. A moment counts once, for what
/// ranks highest: gating (its landings' gates), booting (its shows' launches), running (its leases
/// and jobs), queueing (waiting for any of them), else thinking (nothing of devhub's in flight).
fn timeline(ctx: &Ctx, name: &str) -> Result<String, String> {
    const KINDS: [&str; 5] = ["gating", "booting", "running", "queueing", "thinking"];
    let l = ctx.store.read(|d| d.lanes.iter().rev().find(|l| l.name == name).cloned()).ok_or_else(|| format!("no lane {name}"))?;
    let now = wall_ms();
    let (t0, t1) = (l.created_ms, if l.ended_ms > 0 { l.ended_ms } else { now });
    // the lane's landings
    let entries: Vec<crate::land::Entry> = ctx.store.history("land")?.iter().filter_map(|v| crate::land::Entry::parse(&v.to_json())).filter(|e| (!l.branch.is_empty() && e.branch == l.branch) || (!l.cbranch.is_empty() && e.cbranch == l.cbranch)).collect();
    let gates: Vec<String> = entries.iter().map(|e| format!("landq{}", e.id)).collect();
    // (kind rank, from, to)
    let mut spans: Vec<(usize, u64, u64)> = Vec::new();
    let mut first_gate: HashMap<String, u64> = HashMap::new();
    let mut lease = |lane: &str, asked: u64, granted: u64, ended: u64| {
        let gate = gates.iter().any(|g| g == lane);
        if lane != name && !gate {
            return;
        }
        if gate {
            let f = first_gate.entry(lane.to_string()).or_insert(asked);
            *f = (*f).min(asked);
        }
        if granted > 0 {
            spans.push((3, asked, granted));
            spans.push((if gate { 0 } else { 2 }, granted, if ended > 0 { ended } else { now }));
        } else {
            spans.push((3, asked, if ended > 0 { ended } else { now }));
        }
    };
    for line in std::fs::read_to_string(ctx.home.join("leases.jsonl")).unwrap_or_default().lines() {
        // the cheap look first: the ledger holds every lane's leases
        if !line.contains(name) && !gates.iter().any(|g| line.contains(g.as_str())) {
            continue;
        }
        let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
        let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        lease(v.get("lane").and_then(|x| x.as_str()).unwrap_or(""), n("asked_ms"), n("granted_ms"), n("ended_ms"));
    }
    for x in ctx.leases.view().iter().filter(|x| !matches!(x.state, crate::lease::sched::State::Ended(_))) {
        lease(&x.req.lane, x.asked_ms, x.granted_ms.unwrap_or(0), 0);
    }
    // a landing's wait before its first gate lease asked
    for e in &entries {
        if let Some(f) = first_gate.get(&format!("landq{}", e.id)) {
            spans.push((3, e.asked_ms, *f));
        }
    }
    // its shows' launches (by the lane, or waiting for its branch)
    let shows: Vec<crate::state::Show> = ctx.store.read(|d| d.shows.iter().filter(|x| x.by == name || x.by == l.branch).cloned().collect());
    for x in &shows {
        if x.launch_ms > 0 {
            spans.push((1, x.launch_ms, if x.started_ms > 0 { x.started_ms } else if x.ended_ms > 0 { x.ended_ms } else { now }));
        }
    }
    let mut at: Vec<u64> = vec![t0, t1];
    for s in &spans {
        at.extend([s.1.clamp(t0, t1), s.2.clamp(t0, t1)]);
    }
    at.sort_unstable();
    at.dedup();
    let mut total = [0u64; 5];
    for w in at.windows(2) {
        let kind = spans.iter().filter(|s| s.1 <= w[0] && s.2 >= w[1]).map(|s| s.0).min().unwrap_or(4);
        total[kind] += w[1] - w[0];
    }
    let all = t1.saturating_sub(t0).max(1);
    let time = |ms: u64| if ms >= 3_600_000 { format!("{}h{:02}m", ms / 3_600_000, ms / 60_000 % 60) } else if ms >= 60_000 { format!("{}m{:02}s", ms / 60_000, ms / 1000 % 60) } else { format!("{} s", ms / 1000) };
    let mut out = format!("{name}: {} since its start\n", time(all));
    for (k, t) in KINDS.iter().zip(total) {
        out.push_str(&format!("  {k:<9} {:>7} {:>3}%\n", time(t), t * 100 / all));
    }
    out.push_str(&format!("  ({} landing(s), {} show(s))\n", entries.len(), shows.len()));
    Ok(out)
}

/// `lane new {"name","from","commercial"}`, `lane drop|remove|retire|keep|unkeep|unpark <name>`,
/// `lane park <name> [why]`, `lane adopt`, `lane list`.
pub fn serve(ctx: &Ctx, line: &str) -> Result<String, String> {
    let mut w = line.splitn(3, ' ');
    let _ = w.next();
    let verb = w.next().unwrap_or("list");
    let rest = w.next().unwrap_or("").trim();
    match verb {
        "new" => {
            let v = makepad_strict_json::parse(rest.as_bytes()).map_err(|_| "lane new: bad request")?;
            let name = &makepad_devhub_conf::lane_name(v.get("name").and_then(|x| x.as_str()).unwrap_or(""));
            let from = v.get("from").and_then(|x| x.as_str()).filter(|f| !f.is_empty()).unwrap_or("work");
            let commercial = v.get("commercial").and_then(|x| x.as_bool()).unwrap_or(true);
            let l = create(ctx, name, from, commercial)?;
            event(ctx, Sev::Info, format!("lane new {name}: {} on {} from {from}{}", l.path, l.branch, if commercial { " (+ apps/commercial on the same branch from main)" } else { "" }), Some(&l));
            Ok(format!("{{\"name\":{},\"path\":{},\"branch\":{},\"cbranch\":{}}}", json_str(name), json_str(&l.path), json_str(&l.branch), json_str(&l.cbranch)))
        }
        "drop" => drop_lane(ctx, rest).map(|n| format!("{{\"dropped\":{},\"note\":{}}}", json_str(rest), json_str(&n))),
        "remove" => remove_now(ctx, rest).map(|n| format!("{{\"removed\":{},\"note\":{}}}", json_str(rest), json_str(&n))),
        "park" => {
            let (name, why) = rest.split_once(' ').unwrap_or((rest, ""));
            park(ctx, name, why)
        }
        "unpark" => unpark(ctx, rest),
        "offers" => Ok(offers(ctx)),
        "retire" => retire(ctx, rest).map(|n| format!("{{\"retired\":{},\"note\":{}}}", json_str(rest), json_str(&n))),
        "keep" | "unkeep" => {
            try_set(ctx, rest, |l| l.keep = verb == "keep")?.ok_or_else(|| format!("no live lane {rest}"))?;
            Ok(format!("{{\"name\":{},\"keep\":{}}}", json_str(rest), verb == "keep"))
        }
        "timeline" => timeline(ctx, rest).map(|t| format!("{{\"text\":{}}}", json_str(&t))),
        "quiet" => quiet(ctx).map(|t| format!("{{\"text\":{}}}", json_str(&t))),
        "live" => {
            let (name, on) = match rest.split_once(' ') {
                Some((n, "on")) => (n, true),
                Some((n, "off")) => (n, false),
                _ => return Err("lane live <name> on|off".into()),
            };
            let l = try_set(ctx, name, |l| l.waited = on)?.ok_or_else(|| format!("no live lane {name}"))?;
            event(ctx, Sev::Info, format!("lane {name}: {}", if on { "live: the user waits for it; its landings take the fast path (merge + check)" } else { "not live: its landings gate before landing" }), Some(&l));
            Ok(format!("{{\"name\":{},\"live\":{on}}}", json_str(name)))
        }
        // The user's priority now ("prioritise right now all the work on the source library
        // app", 11-07): these lanes' leases go first. `lane first off` clears it for every lane.
        "first" => {
            let words: Vec<&str> = rest.split_whitespace().collect();
            let (names, on) = match words.split_last() {
                Some((&"on", names)) if !names.is_empty() => (names, true),
                Some((&"off", names)) => (names, false),
                _ => return Err("lane first <name>.. on | lane first [<name>..] off".into()),
            };
            if let Some(n) = names.iter().find(|n| ctx.store.read(|d| !d.lanes.iter().any(|l| l.name == **n && l.state == "live"))) {
                return Err(format!("no live lane {n}"));
            }
            let changed = ctx.store.update(|d| {
                let mut changed = Vec::new();
                for l in d.lanes.iter_mut().filter(|l| l.state == "live" && l.first != on && (names.is_empty() || names.contains(&l.name.as_str()))) {
                    l.first = on;
                    changed.push(l.name.clone());
                }
                changed
            })?;
            let first: Vec<String> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live" && l.first).map(|l| l.name.clone()).collect());
            if !changed.is_empty() {
                let now = if first.is_empty() { "no lane goes first now".to_string() } else { format!("first now: {}", first.join(" ")) };
                event(ctx, Sev::Warn, format!("lane first {}: {} ({now})", if on { "on" } else { "off" }, changed.join(" ")), None);
            }
            Ok(format!("{{\"first\":[{}]}}", first.iter().map(|n| json_str(n)).collect::<Vec<_>>().join(",")))
        }
        "adopt" => adopt(ctx),
        "sweep" => match rest {
            "status" => {
                let last = std::fs::read_to_string(sweep_dir(ctx).join("last")).map_err(|_| "no sweep yet".to_string())?;
                let text = std::fs::read_to_string(Path::new(last.trim()).join("report.txt")).unwrap_or_else(|_| "running\n".into());
                Ok(format!("{{\"report\":{}}}", json_str(&text)))
            }
            "" | "--dry" => sweep(&ctx_arc(ctx)?, rest == "--dry"),
            _ => Err("lane sweep [--dry] | lane sweep status".into()),
        },
        "dormant" => {
            let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-lanes");
            let rows = crate::cli::ask(&c, None, "lanes", "lanes dormant")?;
            Ok(format!("[{}]", rows.join(",")))
        }
        "list" => {
            let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-lanes");
            let merge = crate::cli::ask(&c, None, "lanes", "lanes list").unwrap_or_default();
            let land = land_entries(ctx);
            let lanes: Vec<LaneRec> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live").cloned().collect());
            let rows: Vec<String> = lanes.iter().map(|l| row(ctx, l, &merge, &land)).collect();
            Ok(format!("[{}]", rows.join(",")))
        }
        _ => Err(format!("lane: unknown word `{verb}` (new drop remove keep unkeep live timeline adopt sweep list dormant)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::master::LeaseMaster;
    use crate::state::Store;

    /// A lane is listed when its last run is over and nothing newer came from it: not while
    /// something runs, not after it acted on the result by landing, and not for ever.
    #[test]
    fn a_lane_is_quiet_when_its_last_run_ended_and_nothing_came_after() {
        let (min, now) = (60_000u64, 1_800_000_000_000u64);
        let lane = |name: &str| LaneRec { name: name.into(), branch: format!("rc-{name}"), cbranch: format!("rc-{name}"), ..LaneRec::default() };
        let lanes = [lane("waits"), lane("recent"), lane("running"), lane("landed"), lane("parked"), lane("branch"), lane("never")];
        let ended: HashMap<String, Ended> = [
            ("waits", now - 22 * min, "J7 ok"), ("recent", now - 10 * min, "J8 ok"), ("running", now - 40 * min, "J9 ok"),
            ("landed", now - 40 * min, "J10 ok"), ("parked", now - 13 * 60 * min, "J11 ok"), ("rc-branch", now - 16 * min, "L1 test@1 released"),
        ].into_iter().map(|(l, at, what)| (l.to_string(), Ended { at, what: what.to_string(), agent: String::new() })).collect();
        let busy = HashSet::from(["running".to_string()]);
        let landed = [("rc-landed".to_string(), "-".to_string(), now - 30 * min), ("rc-waits".to_string(), "".to_string(), now - 60 * min)];
        assert_eq!(quiet_lanes(&lanes, &ended, &busy, &landed, now), ["branch: L1 test@1 released ended 16 min ago", "waits: J7 ok ended 22 min ago"]);
    }

    /// A lane is offered for retirement when nothing of it came for 6 h (no run, commit or file
    /// write) and the agent of its last run is silent; never a parked, kept or busy one.
    #[test]
    fn a_lane_long_idle_with_its_agent_gone_is_offered_for_retirement() {
        let (h, now) = (3_600_000u64, 1_800_000_000_000u64);
        let lane = |name: &str| LaneRec { name: name.into(), branch: format!("rc-{name}"), cbranch: format!("rc-{name}"), state: "live".into(), created_ms: now - 48 * h, ..LaneRec::default() };
        let mut lanes = vec![lane("over"), lane("committed"), lane("agent-on"), lane("parked"), lane("kept"), lane("busy"), lane("unused"), lane("recent")];
        lanes[3].parked = "waits for the user".into();
        lanes[4].keep = true;
        let run = |at: u64, agent: &str| Ended { at, what: "J1 ok".into(), agent: agent.into() };
        let ended: HashMap<String, Ended> = [
            ("over", run(now - 7 * h, "AGENT1")), ("rc-committed", run(now - 8 * h, "AGENT2")), ("agent-on", run(now - 9 * h, "AGENT3")),
            ("parked", run(now - 30 * h, "AGENT4")), ("kept", run(now - 30 * h, "AGENT5")), ("busy", run(now - 30 * h, "AGENT6")), ("recent", run(now - 2 * h, "AGENT7")),
        ].into_iter().map(|(l, e)| (l.to_string(), e)).collect();
        let busy = HashSet::from(["busy".to_string()]);
        let active = HashMap::from([("rc-committed".to_string(), now - h)]);
        let seen = |a: &str| if a == "AGENT3" { now - 60_000 } else { 0 };
        let offers = retire_offers(&lanes, &ended, &busy, &active, &seen, now);
        assert_eq!(offers.iter().map(|o| o.name.as_str()).collect::<Vec<_>>(), ["over", "unused"]);
        assert_eq!(offers[0].row, "over: nothing of it for 7 h (last J1 ok), its agent AGENT1 is silent: `devhub lane retire over` removes its worktree, its branches stay");
        assert!(offers[1].row.contains("for 48 h (no job or lease), no agent known"), "{}", offers[1].row);
    }

    fn g(dir: &Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn repo(dir: &Path, branch: &str) {
        std::fs::create_dir_all(dir).unwrap();
        g(dir, &["init", "-q", "-b", branch]);
        g(dir, &["config", "user.email", "t@t"]);
        g(dir, &["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "a").unwrap();
        std::fs::write(dir.join(".gitignore"), "apps/commercial/\ntarget/\n").unwrap();
        g(dir, &["add", "."]);
        g(dir, &["commit", "-q", "-m", "base"]);
    }

    /// A lane whose commits landed before work moved onto a squashed tip (its old chain kept as
    /// `work-old-*`) is landed; a lane with commits of its own on neither is not.
    #[test]
    fn a_lane_landed_on_a_retired_chain_is_landed() {
        let base = std::env::temp_dir().join(format!("devhub-lane-retired-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let mk = base.join("makepad");
        repo(&mk, "work");
        for (name, land) in [("a", true), ("b", false)] {
            let wt = base.join(name);
            g(&mk, &["worktree", "add", "-q", "-b", &format!("rc-{name}"), &wt.to_string_lossy(), "work"]);
            std::fs::write(wt.join(format!("lane-{name}.txt")), name).unwrap();
            g(&wt, &["-c", "user.name=t", "-c", "user.email=t@t", "add", "."]);
            g(&wt, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "-m", "lane work"]);
            if land {
                g(&mk, &["merge", "-q", "--no-ff", "-m", "land", &format!("rc-{name}")]);
            }
        }
        let old = g(&mk, &["rev-parse", "work"]);
        let tree = g(&mk, &["rev-parse", "work^{tree}"]);
        let squashed = g(&mk, &["commit-tree", &tree, "-m", "squashed"]);
        g(&mk, &["update-ref", "refs/heads/work-old-1", &old]);
        g(&mk, &["update-ref", "refs/heads/work", &squashed]);
        let mut v = View::open(&mk, "work").unwrap();
        assert_eq!(v.landed("rc-a", &base.join("a"), true), Ok(()));
        assert_eq!(v.landed("rc-b", &base.join("b"), true), Err("rc-b is not in work".to_string()));
        drop(v);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn audit_streams_header_sorted_worktrees_then_summary() {
        struct Lines {
            pending: Vec<u8>,
            flushed: Vec<String>,
        }
        impl Write for Lines {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                self.pending.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                let line = String::from_utf8(std::mem::take(&mut self.pending)).unwrap();
                assert!(line.ends_with('\n'), "flush ends a line: {line:?}");
                assert_eq!(line.lines().count(), 1, "each line is flushed separately");
                self.flushed.push(line);
                Ok(())
            }
        }
        let base = std::env::temp_dir().join(format!("devhub-lane-audit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let mk = base.join("makepad");
        repo(&mk, "work");
        let root = base.join("wt.noindex");
        std::fs::create_dir_all(&root).unwrap();
        for name in ["gamma", "alpha", "beta"] {
            let path = root.join(name);
            g(&mk, &["worktree", "add", "-q", "-b", &format!("rc-{name}"), &path.to_string_lossy(), "work"]);
            if name == "gamma" {
                std::fs::write(path.join("a.txt"), "changed").unwrap();
            } else {
                g(&path, &["commit", "-q", "--allow-empty", "-m", "lane work"]);
                if name == "alpha" {
                    g(&mk, &["merge", "-q", "--no-ff", "-m", "land", "rc-alpha"]);
                }
            }
        }
        let mut out = Lines { pending: Vec::new(), flushed: Vec::new() };
        audit(&mk, false, &mut out).unwrap();
        assert!(out.pending.is_empty(), "no buffered output remains");
        let lines = &out.flushed;
        assert_eq!(lines.len(), 7, "header, three worktrees, summary, removable names and hint");
        assert!(lines[0].starts_with("3 lane worktree(s) under "), "header first: {lines:?}");
        for (line, expected) in lines[1..4].iter().zip([
            "alpha  landed and clean:", "beta  unlanded commits:", "gamma  uncommitted changes:",
        ]) {
            assert!(line.starts_with(expected) && line.contains("(unregistered)"), "sorted worktree and class: {line}");
        }
        assert!(lines[4].starts_with("Summary: ") && lines[4].contains("1 landed and clean") && lines[4].contains("1 unlanded commits") && lines[4].contains("1 uncommitted changes"), "counts last: {lines:?}");
        assert_eq!(lines[5], "landed and clean (1): alpha\n");
        assert!(lines[6].contains("makepad devhub lane remove <name>") && lines[6].contains("Nothing here was removed."));
        let mut json = Lines { pending: Vec::new(), flushed: Vec::new() };
        audit(&mk, true, &mut json).unwrap();
        assert_eq!(json.flushed.len(), 1, "JSON remains one document");
        let doc = makepad_strict_json::parse(json.flushed[0].as_bytes()).unwrap();
        let lanes = doc.get("lanes").unwrap().as_arr().unwrap();
        for (lane, name) in lanes.iter().zip(["alpha", "beta", "gamma"]) {
            assert_eq!(lane.get("name").unwrap().as_str(), Some(name));
            assert!(root.join(name).join(".git").exists(), "audit removes nothing");
        }
        assert_eq!(lanes.len(), 3);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// new, a landing, the cleanup (after its grace) and a drop, on throwaway repos.
    #[test]
    fn lanes_are_made_cleaned_after_landing_and_dropped() {
        let base = std::env::temp_dir().join(format!("devhub-lane-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let mk = base.join("makepad");
        repo(&mk, "work");
        repo(&mk.join("apps/commercial"), "main");
        let home = base.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let leases = Arc::new(LeaseMaster::new("mac", home.clone(), Arc::new(|_: &str, _: &str, _: &str| Err("no nodes".to_string())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        let ctx = Ctx { store: Store::open(&home, "mac", Arc::new(|_| 0)).unwrap(), leases, machine: "mac".into(), monitor: crate::monitor::Monitor::start(&[base.join("wt.noindex")]), repo: mk.clone(), home: home.clone() };
        let r = serve(&ctx, "lane new {\"name\":\"x\",\"from\":\"work\",\"commercial\":true}").unwrap();
        let lane = base.join("wt.noindex/x");
        assert!(r.contains("rc-x") && lane.join("a.txt").exists() && lane.join("apps/commercial/a.txt").exists(), "{r}");
        assert_eq!(g(&lane.join("apps/commercial"), &["branch", "--show-current"]), "rc-x");
        assert!(serve(&ctx, "lane new {\"name\":\"x\"}").unwrap_err().contains("exists"));
        // a fresh lane is in work but never landed: the cleanup keeps it, without a git process
        let spawned = crate::git_exec::on_this_thread();
        cleanup(&ctx);
        cleanup(&ctx);
        assert_eq!(crate::git_exec::on_this_thread(), spawned);
        assert_eq!(ctx.store.read(|d| (d.lanes[0].state.clone(), d.lanes[0].landed_ms)), ("live".to_string(), 0));
        // asked by name, a lane with nothing of its own goes (the cleanup never takes one)
        serve(&ctx, "lane new {\"name\":\"z\",\"commercial\":true}").unwrap();
        assert!(serve(&ctx, "lane remove z").is_ok());
        assert!(!base.join("wt.noindex/z").exists() && g(&mk, &["branch", "--list", "rc-z"]).is_empty());
        // a commit, landed by a merge into work (and commercial into main)
        std::fs::write(lane.join("b.txt"), "b").unwrap();
        g(&lane, &["add", "."]);
        g(&lane, &["commit", "-q", "-m", "lane work"]);
        std::fs::write(lane.join("apps/commercial/c.txt"), "c").unwrap();
        g(&lane.join("apps/commercial"), &["add", "."]);
        g(&lane.join("apps/commercial"), &["commit", "-q", "-m", "lane work"]);
        // a commit of its own that landed nowhere keeps it, asked or not
        let e = serve(&ctx, "lane remove x").unwrap_err();
        assert!(e.contains("1 commit(s) of its own landed nowhere"), "{e}");
        g(&mk, &["merge", "-q", "--no-ff", "-m", "land x", "rc-x"]);
        g(&mk.join("apps/commercial"), &["merge", "-q", "--no-ff", "-m", "land x", "rc-x"]);
        // the merge bot merged work in again after the landing: still landed
        g(&lane, &["commit", "-q", "--allow-empty", "-m", "Merge work (0123456789) into rc-x"]);
        // an uncommitted file keeps it
        std::fs::write(lane.join("wip.txt"), "w").unwrap();
        cleanup(&ctx);
        assert!(ctx.store.read(|d| d.lanes[0].note.clone()).contains("uncommitted"));
        let mut out = Vec::new();
        audit(&mk, false, &mut out).unwrap();
        let a = String::from_utf8(out).unwrap();
        assert!(a.contains("x  landed, untracked files only:") && a.contains("1 untracked (wip.txt)"), "{a}");
        std::fs::remove_file(lane.join("wip.txt")).unwrap();
        // clean: first seen landed, then removed once the grace passed with no activity
        cleanup(&ctx);
        assert!(ctx.store.read(|d| d.lanes[0].landed_ms) > 0);
        // within the grace it stays; `lane remove` (adopt's list, once agreed) skips the grace
        cleanup(&ctx);
        assert_eq!(ctx.store.read(|d| d.lanes[0].state.clone()), "live");
        // an open hand-in of it keeps it: the queue is read from the master's state, never a link
        ctx.store.update(|d| d.land = vec![makepad_strict_json::parse(br#"{"id":9,"branch":"rc-x","state":"queued"}"#).unwrap()]).unwrap();
        let kept = serve(&ctx, "lane remove x").unwrap_err();
        assert!(kept.contains("still in the land queue"), "{kept}");
        ctx.store.update(|d| d.land.clear()).unwrap();
        assert!(serve(&ctx, "lane remove x").is_ok());
        assert_eq!(ctx.store.read(|d| d.lanes[0].state.clone()), "removed", "{:?}", ctx.store.read(|d| d.lanes[0].note.clone()));
        assert!(!lane.exists());
        // landed long ago, nothing touched, but a run of it (by its branch) ended just now: it
        // stays until the grace has passed since that run
        let state = |name: &str| ctx.store.read(|d| d.lanes.iter().rev().find(|l| l.name == name).map(|l| l.state.clone()).unwrap());
        serve(&ctx, "lane new {\"name\":\"r\"}").unwrap();
        let ran = base.join("wt.noindex/r");
        std::fs::write(ran.join("r.txt"), "r").unwrap();
        g(&ran, &["add", "."]);
        g(&ran, &["commit", "-q", "-m", "lane work"]);
        g(&mk, &["merge", "-q", "--no-ff", "-m", "land r", "rc-r"]);
        cleanup(&ctx);
        set(&ctx, "r", |x| x.landed_ms = 1);
        let ledger = |ended_ms: u64| std::fs::write(home.join("leases.jsonl"), format!("{{\"id\":\"L7\",\"lane\":\"rc-r\",\"kind\":\"screen\",\"node\":\"n1\",\"how\":\"released\",\"ended_ms\":{ended_ms}}}\n")).unwrap();
        ledger(wall_ms());
        cleanup(&ctx);
        assert_eq!(state("r"), "live");
        ledger(wall_ms() - GRACE_MS - 1);
        cleanup(&ctx);
        assert_eq!(state("r"), "removed");
        assert!(!ran.exists());
        // branch and worktree gone some other way: the pass clears the record, and so does asking
        for name in ["gone", "gone2"] {
            serve(&ctx, &format!("lane new {{\"name\":\"{name}\"}}")).unwrap();
            g(&mk, &["worktree", "remove", "--force", &base.join("wt.noindex").join(name).to_string_lossy()]);
            g(&mk, &["branch", "-D", &format!("rc-{name}")]);
        }
        serve(&ctx, "lane remove gone2").unwrap();
        assert_eq!(state("gone2"), "removed");
        cleanup(&ctx);
        assert_eq!(state("gone"), "removed");
        assert!(g(&mk, &["branch", "--list", "rc-x"]).is_empty());
        assert!(g(&mk.join("apps/commercial"), &["branch", "--list", "rc-x"]).is_empty());
        // a dropped lane keeps its unlanded and uncommitted work in a bundle
        serve(&ctx, "lane new {\"name\":\"y\",\"commercial\":true}").unwrap();
        let y = base.join("wt.noindex/y");
        std::fs::write(y.join("draft.txt"), "d").unwrap();
        let r = serve(&ctx, "lane drop y").unwrap();
        assert!(r.contains("1 makepad commit(s)") && !r.contains("commercial commit"), "{r}");
        assert!(!y.exists() && g(&mk, &["branch", "--list", "rc-y"]).is_empty());
        let bundle = std::fs::read_dir(ctx.home.join("dropped")).unwrap().flatten().map(|e| e.path()).find(|p| p.to_string_lossy().contains("y-makepad-")).unwrap();
        assert!(g(&mk, &["bundle", "list-heads", &bundle.to_string_lossy()]).contains("refs/heads/rc-y"));
        let _ = std::fs::remove_file(&bundle);
        drop(ctx);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// `lane offers --check` reads each offered worktree here: a clean one retires (its bytes and
    /// its nodes' dirs counted), one with an untracked file or a run the master named refuses, by
    /// reason; a node that did not answer is named.
    #[test]
    fn offers_are_checked_by_what_would_refuse_them() {
        let base = std::env::temp_dir().join(format!("devhub-lane-offers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for name in ["clean", "untracked", "leased"] {
            repo(&base.join(name), "rc-x");
        }
        std::fs::write(base.join("untracked/notes.txt"), "n").unwrap();
        let lane = |name: &str, class: &str| format!("{{\"name\":\"{name}\",\"path\":{},\"refused_class\":\"{class}\",\"refused\":\"{class}\"}}", json_str(&base.join(name).to_string_lossy()));
        let offers = makepad_strict_json::parse(format!("{{\"lanes\":[{},{},{}]}}", lane("clean", ""), lane("untracked", ""), lane("leased", "a lease of the lane is open")).as_bytes()).unwrap();
        let nodes = vec![("box-a".to_string(), Ok(makepad_strict_json::parse(br#"{"lanes":{"clean":1073741824,"other":5}}"#).unwrap())), ("box-b".to_string(), Err("link lost".to_string()))];
        let out = check_offers(&offers, &nodes);
        assert!(out.starts_with("3 lane(s) offered for retirement (nothing retired)\n"), "{out}");
        let clean = out.lines().find(|l| l.starts_with("retire cleanly")).unwrap_or_default();
        assert!(clean.split_whitespace().nth(2) == Some("1") && clean.ends_with("1.0 GB"), "{out}");
        assert!(out.contains("uncommitted or untracked files: untracked\n") && out.contains("a lease of the lane is open: leased\n"), "{out}");
        assert!(out.contains("node dirs not read: box-b (link lost)"), "{out}");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A parked lane's branches are held (the merge bot leaves them alone) and `lane quiet` lists it
    /// in one row; a retired one leaves its worktree and keeps its branches, never with an
    /// untracked file in it.
    /// A tracked `eol=crlf` script with its index stat out of date is clean to git, and so to the
    /// retire check: four of them refused a finished lane's retire as uncommitted until a later
    /// `git status` had refreshed the index. A real edit still refuses.
    #[test]
    fn a_checkout_crlf_file_does_not_refuse_a_retire() {
        let base = std::env::temp_dir().join(format!("devhub-lane-crlf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        repo(&base, "work");
        std::fs::write(base.join(".gitattributes"), "*.ps1 text eol=crlf\n").unwrap();
        std::fs::write(base.join("tool.ps1"), "one\r\ntwo\r\n").unwrap();
        g(&base, &["add", "."]);
        g(&base, &["commit", "-q", "-m", "a script"]);
        let rewritten = |bytes: &str| {
            std::fs::remove_file(base.join("tool.ps1")).unwrap();
            std::fs::write(base.join("tool.ps1"), bytes).unwrap();
            let later = std::time::SystemTime::now() + std::time::Duration::from_secs(5);
            std::fs::File::options().write(true).open(base.join("tool.ps1")).unwrap().set_modified(later).unwrap();
        };
        rewritten("one\r\ntwo\r\n");
        assert_eq!(dirty_refusal(&base), None);
        rewritten("one\r\nthree\r\n");
        let (class, why) = dirty_refusal(&base).expect("an edit");
        assert_eq!(class, "uncommitted or untracked files");
        assert!(why.contains("tool.ps1"), "{why}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_lane_parks_and_retires_keeping_its_branches() {
        let base = std::env::temp_dir().join(format!("devhub-lane-park-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let mk = base.join("makepad");
        repo(&mk, "work");
        repo(&mk.join("apps/commercial"), "main");
        let home = base.join("home");
        std::fs::create_dir_all(&home).unwrap();
        let leases = Arc::new(LeaseMaster::new("mac", home.clone(), Arc::new(|_: &str, _: &str, _: &str| Err("no nodes".to_string())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        let ctx = Ctx { store: Store::open(&home, "mac", Arc::new(|_| 0)).unwrap(), leases, machine: "mac".into(), monitor: crate::monitor::Monitor::start(&[base.join("wt.noindex")]), repo: mk.clone(), home: home.clone() };
        serve(&ctx, "lane new {\"name\":\"p\",\"commercial\":true}").unwrap();
        let lane = base.join("wt.noindex/p");
        std::fs::write(lane.join("b.txt"), "b").unwrap();
        std::fs::write(lane.join(".gitignore"), "apps/commercial/\ntarget/\nlocal/\n").unwrap();
        g(&lane, &["add", "."]);
        g(&lane, &["commit", "-q", "-m", "lane work that lands elsewhere"]);
        // its report, ignored by git, goes to the main checkout's local/ with the retire
        std::fs::create_dir_all(lane.join("local/agent_state/p")).unwrap();
        std::fs::write(lane.join("local/agent_state/p/report.md"), "found it").unwrap();
        let r = serve(&ctx, "lane park p finished, rc-q lands it").unwrap();
        assert!(r.contains("\"branches\":[\"rc-p\"]"), "{r}");
        assert_eq!(ctx.store.read(|d| d.held.clone()), [("rc-p".to_string(), "parked: finished, rc-q lands it".to_string())]);
        let quiet = serve(&ctx, "lane quiet").unwrap();
        assert!(quiet.contains("parked: p (finished, rc-q lands it)"), "{quiet}");
        serve(&ctx, "lane unpark p").unwrap();
        assert!(ctx.store.read(|d| d.held.is_empty()));
        assert!(serve(&ctx, "lane unpark p").unwrap_err().contains("not parked"));
        serve(&ctx, "lane park p").unwrap();
        // an untracked file keeps it, named
        std::fs::write(lane.join("apps/commercial/notes.txt"), "n").unwrap();
        let e = serve(&ctx, "lane retire p").unwrap_err();
        assert!(e.contains("notes.txt"), "{e}");
        std::fs::remove_file(lane.join("apps/commercial/notes.txt")).unwrap();
        let r = serve(&ctx, "lane retire p").unwrap();
        assert!(r.contains("branch rc-p kept"), "{r}");
        assert!(r.contains("local/: 1 file(s) kept"), "{r}");
        assert_eq!(std::fs::read_to_string(mk.join("local/agent_state/p/report.md")).unwrap(), "found it");
        assert!(!lane.exists());
        assert_eq!(g(&mk, &["branch", "--list", "rc-p"]), "rc-p");
        assert_eq!(g(&mk.join("apps/commercial"), &["branch", "--list", "rc-p"]), "rc-p");
        assert!(ctx.store.read(|d| d.held.is_empty()));
        assert_eq!(ctx.store.read(|d| d.lanes[0].state.clone()), "retired");
        drop(ctx);
        let _ = std::fs::remove_dir_all(&base);
    }

    /// A lane's local/ is copied beside what the main checkout has, never over it, and a second
    /// pass copies nothing.
    #[test]
    fn a_retired_lanes_local_files_are_kept_never_over_others() {
        let base = std::env::temp_dir().join(format!("devhub-keep-local-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (from, to) = (base.join("lane/local"), base.join("main/local"));
        let put = |p: &Path, s: &str| {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, s).unwrap();
        };
        put(&from.join("agent_state/l/report.md"), "new");
        put(&from.join("same.txt"), "same");
        put(&to.join("same.txt"), "same");
        put(&from.join("notes.txt"), "lane's");
        put(&to.join("notes.txt"), "main's");
        put(&to.join("notes.txt.l"), "an older lane l's");
        put(&from.join("target/CACHEDIR.TAG"), "Signature: 8a477f597d28d172789f06886806bc55");
        put(&from.join("target/big.rlib"), "x");
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc/hosts", from.join("hosts")).unwrap();
        assert_eq!(keep_local(&from, &to, "l").unwrap(), (2, 9));
        let read = |p: &str| std::fs::read_to_string(to.join(p)).unwrap();
        assert_eq!(read("agent_state/l/report.md"), "new");
        assert_eq!(read("notes.txt"), "main's");
        assert_eq!(read("notes.txt.l"), "an older lane l's");
        assert_eq!(read("notes.txt.l.2"), "lane's");
        assert!(!to.join("target").exists() && !to.join("hosts").exists() && !to.join("same.txt.l").exists());
        assert_eq!(keep_local(&from, &to, "l").unwrap(), (0, 0), "a second pass finds everything kept");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn safety_detached_cleanup_head_has_a_recovery_ref() {
        let base = std::env::temp_dir().join(format!("devhub-detached-{}", std::process::id()));
        let mk = base.join("makepad");
        repo(&mk, "work");
        repo(&mk.join("apps/commercial"), "main");
        let home = base.join("state");
        let leases = Arc::new(LeaseMaster::new("test", home.clone(), Arc::new(|_, _, _| Err("no nodes".into())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        let ctx = Ctx { store: Store::open(&home, "test", Arc::new(|_| 0)).unwrap(), leases, machine: "test".into(), monitor: crate::monitor::Monitor::start(&[]), repo: mk.clone(), home };
        let lane = create(&ctx, "detached", "work", true).unwrap();
        let path = PathBuf::from(&lane.path);
        let mut heads = Vec::new();
        // Both actual heads diverge from the registered branches before the final remove.
        for tree in [&path, &path.join("apps/commercial")] {
            g(tree, &["checkout", "--detach", "-q"]);
            g(tree, &["commit", "--allow-empty", "-q", "-m", "detached work"]);
            heads.push(g(tree, &["rev-parse", "HEAD"]));
        }
        remove(&ctx, &lane, false, true).unwrap();
        assert!(!path.exists());
        for (repo, head) in [mk.clone(), mk.join("apps/commercial")].iter().zip(heads) {
            assert_eq!(g(repo, &["rev-parse", &format!("refs/devhub/recovery/{head}")]), head, "detached work must survive removal of either half");
        }
        drop(ctx);
        std::fs::remove_dir_all(&base).unwrap();
    }

    #[test]
    fn lane_names() {
        assert!(super::valid_name("devhub-orch_2"));
        assert!(!super::valid_name("-x") && !super::valid_name("a/b") && !super::valid_name("") && !super::valid_name("a b"));
    }
}
