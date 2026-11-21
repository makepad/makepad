//! The lane worktree lifecycle, owned by the master (lanes never run `git worktree` themselves):
//! - `devhub lane new <name> [--from work] [--no-commercial]`: `<lanes root>/<name>` on `rc-<name>`
//!   from work, with the nested `apps/commercial` on `rc-<name>` from main; registered and
//!   watched ([`crate::monitor`]: its dirty files without scans).
//! - after it lands, the master removes it: both halves in work and main (the branch tips are in
//!   them and are not work's own first-parent commits, so a fresh lane never counts), nothing
//!   uncommitted, no process inside, no lease, job or land entry of the lane open, and no file
//!   activity and no job or lease ending for [`GRACE_MS`]. Then the lane's dirs on every node go;
//!   the worktree pair and the branches (local, and origin's when pushed) only once its agents
//!   also made no devhub call for [`AGENT_IDLE_MS`]: an agent that reads or thinks between steps
//!   writes no file and runs nothing. `devhub lane keep <name>` exempts a lane.
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
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// A landed lane is removed only after this long with no file activity, no new commit and no job
/// or lease of it ending.
pub const GRACE_MS: u64 = 30 * 60 * 1000;
/// A landed lane's worktree and branches go only after this long with no devhub call of its agents
/// either (its node dirs went at [`GRACE_MS`]): a worktree is cheap to keep, an agent that comes
/// back to its removed lane is not (11-16).
pub const AGENT_IDLE_MS: u64 = 60 * 60 * 1000;
const LANDED_NOTE: &str = "landed: its node dirs go after 30 min with no activity, its worktree and branches once its agents are idle for 1 h";
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

    /// [`View::landed`], or its tip in the target and a hand-in of the branch landed since
    /// `since_ms` (`land`: [`land_entries`]), its pinned commit in the target. The merge bot
    /// fast-forwards a landed lane to the target: its tip is one of the target's own first-parent
    /// commits then, as a fresh lane's is, and only the train's record tells the two apart.
    fn landed_since(&mut self, branch: &str, tree: &Path, since_ms: u64, land: &[String]) -> Result<(), String> {
        self.landed(branch, tree, true).or_else(|e| {
            let Ok(Some(tip)) = self.tip_of(branch) else { return Err(e) };
            let handed = self.reached(&tip) && land.iter().filter_map(|x| crate::land::Entry::parse(x)).any(|h| {
                let (b, pinned) = if self.target == "main" { (&h.cbranch, &h.cs) } else { (&h.branch, &h.ms) };
                h.state == crate::land::St::Landed && b == branch && h.asked_ms >= since_ms && ObjectId::from_hex(pinned).is_ok_and(|o| self.reached(&o))
            });
            if handed { Ok(()) } else { Err(e) }
        })
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
    views.makepad.landed_since(&l.branch, path, l.created_ms, land).or_else(|e| views.makepad.nothing_unlanded(&l.branch, path, asked, e))?;
    let nested = path.join("apps/commercial");
    if nested.join(".git").exists() {
        if l.cbranch.is_empty() {
            // (one git let go of has no HEAD to read: it is refused below, on no branch)
            if !let_go(&ctx.commercial(), &nested) && wt::unique_commits(&nested) {
                return Err("apps/commercial has commits on no commercial ref".into());
            }
        } else if let Some(c) = views.commercial.as_mut() {
            // its own commits (if any) in main; the bot's merges of main into it do not count
            if c.tip_of(&l.cbranch).map_err(|e| format!("commercial: {e}"))?.is_some() {
                c.landed(&l.cbranch, &nested, false).or_else(|e| c.nothing_unlanded(&l.cbranch, &nested, asked, e)).map_err(|e| format!("commercial: {e}"))?;
            }
        }
    }
    // a half git let go of mid-removal has no HEAD: what is left of it is checked against its branch
    for (tree, repo, branch) in [(path.to_path_buf(), ctx.repo.clone(), &l.branch), (nested.clone(), ctx.commercial(), &l.cbranch)] {
        if let_go(&repo, &tree) {
            left_is_branchs(&repo, &tree, branch)?;
        }
    }
    for (tree, repo, branch) in [(path.to_path_buf(), ctx.repo.clone(), &l.branch), (nested.clone(), ctx.commercial(), &l.cbranch)] {
        if tree.join(".git").exists() && !let_go(&repo, &tree) {
            let (head, actual) = crate::monitor::read_head(&tree).ok_or("worktree has no HEAD")?;
            if actual != *branch || (!branch.is_empty() && branch_tip(&tree, branch).as_deref() != Some(head.as_str())) {
                return Err(format!("{}: HEAD or branch identity changed; reconcile before cleanup", tree.display()));
            }
        }
    }
    for (tree, repo) in [(path.to_path_buf(), ctx.repo.clone()), (nested.clone(), ctx.commercial())] {
        if !tree.join(".git").exists() || let_go(&repo, &tree) {
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
    if let Some((_, why)) = open_run(ctx, l) {
        return Err(why);
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
        return Some(why);
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
    super::slot::check_clean(ctx, &l)?;
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

/// A lane's runs include live sessions and synthetic leases rooted in either half (11-15).
struct LaneRuns<'a> {
    lane: &'a LaneRec,
    lives: Vec<&'a crate::state::Live>,
}

impl<'a> LaneRuns<'a> {
    fn new(lane: &'a LaneRec, lives: &'a [crate::state::Live]) -> Self {
        let mut runs = Self { lane, lives: Vec::new() };
        runs.lives = lives.iter().filter(|x| x.lane == lane.name || runs.tree(&x.tree)).collect();
        runs
    }

    fn named(&self, name: &str) -> bool { !name.is_empty() && (name == self.lane.name || name == self.lane.branch) }
    fn tree(&self, tree: &str) -> bool { !self.lane.path.is_empty() && !tree.is_empty() && Path::new(tree).starts_with(&self.lane.path) }
    fn lease(&self, id: &str, name: &str, tree: &str) -> bool {
        self.named(name) || self.tree(tree) || (!id.is_empty() && self.lives.iter().any(|x| x.lease == id))
    }
    fn live(&self) -> Option<String> { self.lives.iter().find(|x| x.active()).map(|x| format!("live job {} runs from this worktree", x.id)) }
}

/// A live session, lease or job of the lane in flight, as (class, refusal).
fn open_run(ctx: &Ctx, l: &LaneRec) -> Option<(&'static str, String)> {
    let leases = ctx.leases.view();
    ctx.store.read(|d| {
        let runs = LaneRuns::new(l, &d.lives);
        if let Some(why) = runs.live() { return Some(("a live job runs from this worktree", why)); }
        if let Some(x) = leases.iter().find(|x| !matches!(x.state, crate::lease::sched::State::Ended(_)) && runs.lease(&x.id, &x.req.lane, x.req.repo_dir.as_deref().unwrap_or(""))) {
            return Some(("a lease of the lane is open", format!("a lease of the lane is open ({})", x.id)));
        }
        d.jobs.iter().any(|j| runs.named(&j.lane) && !j.done()).then(|| ("a job of the lane is open", "a job of the lane is open".to_string()))
    })
}

/// Prune reads the master's durable runs even without a running master (11-15).
pub(crate) fn saved_open_run(home: &Path, l: &LaneRec) -> Result<Option<String>, String> {
    let doc = crate::state::committed(home)?.map(|v| crate::state::Doc::from_value(&v)).unwrap_or_default();
    let runs = LaneRuns::new(l, &doc.lives);
    if let Some(why) = runs.live() { return Ok(Some(why)); }
    let text = match std::fs::read_to_string(home.join("leases.live")) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("leases.live: {e}")),
    };
    for line in text.lines() {
        let v = makepad_strict_json::parse(line.as_bytes()).map_err(|e| format!("leases.live: {e:?}"))?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
        let Some(req) = v.get("req") else { continue };
        let r = |k: &str| req.get(k).and_then(|x| x.as_str()).unwrap_or("");
        if s("state") != "ended" && runs.lease(s("id"), r("lane"), r("repo_dir")) {
            return Ok(Some(format!("a lease of the lane is open ({})", s("id"))));
        }
    }
    Ok(doc.jobs.iter().any(|j| runs.named(&j.lane) && !j.done()).then(|| "a job of the lane is open".to_string()))
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

/// The train moves changed_ms at landing; later landings restart the grace (11-15).
fn last_landing(l: &LaneRec, land: &[String]) -> u64 {
    land.iter().filter_map(|x| makepad_strict_json::parse(x.as_bytes()).ok()).filter(|v| {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
        s("state") == "landed" && (s("branch") == l.branch || (!l.cbranch.is_empty() && s("cbranch") == l.cbranch))
    }).filter_map(|v| v.get("changed_ms").and_then(|x| x.as_u64())).max().unwrap_or(0).max(l.landed_ms)
}

/// The automatic cleanup pass: removes the lanes that landed, after the grace period.
pub fn cleanup(ctx: &Ctx) {
    cleanup_pass(ctx, wt::live_cwds);
}

fn cleanup_pass(ctx: &Ctx, cwds: impl FnOnce() -> Vec<(u32, PathBuf)>) {
    let mut lanes: Vec<LaneRec> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live").cloned().collect());
    // One write for the new activity in either half, including kept lanes (11-15).
    let mut activity = HashMap::new();
    for l in &mut lanes {
        let active = ctx.monitor.last_event_ms(Path::new(&l.path)).max(ctx.monitor.last_event_ms(&Path::new(&l.path).join("apps/commercial")));
        if active > l.active_ms { l.active_ms = active; activity.insert(l.name.clone(), active); }
    }
    if !activity.is_empty() {
        if let Err(e) = ctx.store.update(|d| for l in d.lanes.iter_mut().filter(|l| l.state == "live") {
            if let Some(at) = activity.get(&l.name) { l.active_ms = l.active_ms.max(*at); }
        }) {
            event(ctx, Sev::Warn, format!("lane activity: {e}"), None);
            return;
        }
    }
    lanes.retain(|l| !l.keep);
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
    let cwds = cwds();
    let land = land_entries(ctx);
    let now = wall_ms();
    let mut agents = HashMap::new();
    for mut l in lanes {
        if only_the_record(ctx, &l, &views) {
            finish(ctx, &l.name, "removed", remove(ctx, &l, false, true));
            continue;
        }
        match removable(ctx, &l, &mut views, &cwds, &land, false) {
            Err(why) => {
                // a lane that landed and is still referenced says why once
                let landed = views.makepad.landed_since(&l.branch, Path::new(&l.path), l.created_ms, &land).is_ok();
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
                        x.note = LANDED_NOTE.into();
                    });
                    event(ctx, Sev::Info, format!("lane {}: landed; its node dirs go after 30 min with no activity, its worktree and branches once its agents are idle for 1 h (devhub lane keep {} to keep it)", l.name, l.name), Some(&l));
                    continue;
                }
                let landed = last_landing(&l, &land);
                if landed > l.landed_ms {
                    set(ctx, &l.name, |x| x.landed_ms = landed);
                    l.landed_ms = landed;
                }
                // its agents' calls, saved for the worktree's wait: a restarted master heard no one
                let heard = lane_agents(ctx, &l, &land).iter().map(|a| ctx.leases.agent_seen_ms(a)).max().unwrap_or(0);
                if heard > l.agent_ms {
                    l.agent_ms = heard;
                    agents.insert(l.name.clone(), heard);
                }
                let last = l.landed_ms.max(l.active_ms);
                if now.saturating_sub(last) < GRACE_MS {
                    continue;
                }
                // the node dirs, once per quiet spell: the grace since it landed, since its last
                // file event and since its last run: an agent between runs on overlays touches no
                // file (a lane's worktree and 23 GB of its node dirs went the moment its last A/B
                // ended, 40 s before its next step, 11-04)
                if l.dirs_ms < last {
                    let ran = newest_run(ctx, &l);
                    if now.saturating_sub(ran) < GRACE_MS {
                        continue;
                    }
                    let what = clean_node_dirs(ctx, &l);
                    let rec = set(ctx, &l.name, |x| {
                        x.dirs_ms = now;
                        x.agent_ms = x.agent_ms.max(ran);
                        x.note = format!("landed: {what} cleaned; its worktree and branches go once its agents are idle for 1 h");
                    });
                    event(ctx, Sev::Info, format!("lane {}: {what} cleaned; its worktree and branches go once its agents are idle for 1 h", l.name), rec.as_ref());
                    continue;
                }
                // the worktree and branches: no landing, file event, devhub call of its agents or
                // run of it for the hour. The ledger is read once per idle hour, not every pass.
                if now.saturating_sub(last.max(l.agent_ms)) < AGENT_IDLE_MS {
                    continue;
                }
                let ran = newest_run(ctx, &l);
                if now.saturating_sub(ran) < AGENT_IDLE_MS {
                    agents.insert(l.name.clone(), l.agent_ms.max(ran));
                    continue;
                }
                let what = remove(ctx, &l, false, true);
                finish(ctx, &l.name, "removed", what);
            }
        }
    }
    // one write for the agents' new calls (11-16)
    if !agents.is_empty() {
        if let Err(e) = ctx.store.update(|d| for l in d.lanes.iter_mut().filter(|l| l.state == "live") {
            if let Some(at) = agents.get(&l.name) { l.agent_ms = l.agent_ms.max(*at); }
        }) {
            event(ctx, Sev::Warn, format!("lane agents: {e}"), None);
        }
    }
    views.makepad.forget_unused();
    if let Some(c) = views.commercial.as_mut() {
        c.forget_unused();
    }
}

/// The end of the lane's newest job, live session or lease (reads the lease ledger).
fn newest_run(ctx: &Ctx, l: &LaneRec) -> u64 {
    ended_runs(ctx, std::slice::from_ref(l)).into_values().map(|e| e.at).max().unwrap_or(0)
}

/// The agents of the lane since it was made: who handed it in, and who asked for its jobs, live
/// sessions and the leases the master still holds.
fn lane_agents(ctx: &Ctx, l: &LaneRec, land: &[String]) -> BTreeSet<String> {
    let mut agents: BTreeSet<String> = land.iter().filter_map(|x| crate::land::Entry::parse(x)).filter(|e| e.changed_ms >= l.created_ms && ((!l.branch.is_empty() && e.branch == l.branch) || (!l.cbranch.is_empty() && e.cbranch == l.cbranch))).map(|e| e.agent).collect();
    let leases = ctx.leases.view();
    ctx.store.read(|d| {
        let runs = LaneRuns::new(l, &d.lives);
        agents.extend(runs.lives.iter().map(|x| x.agent.clone()));
        agents.extend(d.jobs.iter().filter(|j| runs.named(&j.lane)).map(|j| j.agent.clone()));
        agents.extend(leases.iter().filter(|x| runs.lease(&x.id, &x.req.lane, x.req.repo_dir.as_deref().unwrap_or(""))).filter_map(|x| x.req.agent.clone()));
    });
    agents.retain(|a| !a.is_empty());
    agents
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

/// Whether git let go of the worktree at `tree` (of `repo`) partway through removing it: its `.git`
/// file names an admin dir that is gone, or the tree has no `.git` and `repo` lists no worktree
/// there. `git worktree remove` deletes the admin dir even when the tree's own delete failed, so
/// what is left of the tree has no HEAD and no index (wmcaption, 11-19).
fn let_go(repo: &Path, tree: &Path) -> bool {
    let dot = tree.join(".git");
    if dot.is_file() {
        return std::fs::read_to_string(&dot).ok().and_then(|t| t.trim().strip_prefix("gitdir:").map(|d| !tree.join(d.trim()).exists())).unwrap_or(false);
    }
    // (a nested half's dir with no `.git` is its makepad half's, not a worktree)
    if dot.exists() || !tree.is_dir() || tree.ends_with("apps/commercial") {
        return false;
    }
    let listed = wt::git(Some(repo), None, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    let canon = std::fs::canonicalize(tree).unwrap_or_else(|_| tree.to_path_buf());
    !listed.lines().filter_map(|x| x.strip_prefix("worktree ")).any(|w| Path::new(w) == tree || Path::new(w) == canon)
}

/// What is left of a half git let go of ([`let_go`]) that `tip` of `repo` does not hold: each file
/// changed or added (one git ignores aside, and the nested half's dir). A file gone is the
/// removal's. Read with a private index against the repository's objects; no file of git's changes.
fn left_beyond(repo: &Path, tree: &Path, tip: &str) -> Result<Vec<String>, String> {
    let common = wt::git(Some(repo), None, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let index = std::env::temp_dir().join(format!("devhub-let-go-{}-{}.index", std::process::id(), wall_ms()));
    let git = |args: &[&str]| -> Result<String, String> {
        let out = crate::git_exec::command().arg("-c").arg("core.hooksPath=/dev/null").arg("--git-dir").arg(common.trim()).arg("--work-tree").arg(tree).args(args)
            .current_dir(tree).stdin(std::process::Stdio::null()).env("GIT_TERMINAL_PROMPT", "0").env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env("GIT_INDEX_FILE", &index)
            .output().map_err(|e| format!("git: {e}"))?;
        if out.status.success() { Ok(String::from_utf8_lossy(&out.stdout).into_owned()) } else { Err(format!("git {}: {}", args[0], crate::proto::clean(String::from_utf8_lossy(&out.stderr).trim(), 300))) }
    };
    let read = (|| {
        git(&["read-tree", tip])?;
        // (the files' stat data in the index, so only a content change shows; -q: one is no error)
        let _ = git(&["update-index", "-q", "--refresh"]);
        Ok::<_, String>((git(&["diff-files", "--name-status", "-z"])?, git(&["ls-files", "--others", "--exclude-standard", "-z"])?))
    })();
    let _ = std::fs::remove_file(&index);
    let (changed, others) = read?;
    let fields: Vec<&str> = changed.split('\0').collect();
    let mut left: Vec<String> = fields.chunks(2).filter(|c| c.len() == 2 && !c[0].is_empty() && c[0] != "D").map(|c| c[1].to_string()).collect();
    left.extend(others.split('\0').filter(|p| !p.is_empty() && !(*p == "apps/commercial/" || p.starts_with("apps/commercial/"))).map(str::to_string));
    Ok(left)
}

/// A half git let go of ([`let_go`]) may go when what is left of it is its branch's.
fn left_is_branchs(repo: &Path, tree: &Path, branch: &str) -> Result<(), String> {
    let tip = (!branch.is_empty()).then(|| branch_tip(repo, branch)).flatten()
        .ok_or_else(|| format!("{}: git let go of it mid-removal, and it is on no branch to check what is left of it against", tree.display()))?;
    let left = left_beyond(repo, tree, &tip)?;
    if left.is_empty() {
        return Ok(());
    }
    let more = if left.len() > 8 { format!(" and {} more", left.len() - 8) } else { String::new() };
    Err(format!("{}: git let go of it mid-removal, and {} file(s) left in it are not {branch}'s: {}{more}", tree.display(), left.len(), left[..left.len().min(8)].join(", ")))
}

/// Deletes what is left of a half git let go of ([`let_go`]), checked against its branch first.
fn drop_let_go(repo: &Path, tree: &Path, branch: &str) -> Result<(), String> {
    left_is_branchs(repo, tree, branch)?;
    std::fs::remove_dir_all(tree).map_err(|e| format!("{}: {e}", tree.display()))
}

/// Removes the worktree pair after preserving its actual heads, conditionally deletes the
/// checked local/origin branch tips (`branches`; a retired lane keeps them), and cleans the
/// lane's dirs on every node.
fn remove(ctx: &Ctx, l: &LaneRec, drop: bool, branches: bool) -> Result<String, String> {
    let path = PathBuf::from(&l.path);
    let nested = path.join("apps/commercial");
    let mut done = super::slot::remove(ctx, l, drop)?;
    let force: &[&str] = if drop { &["--force"] } else { &[] };
    let mut checked = Vec::new();
    for (dir, branch, target) in [(ctx.repo.clone(), &l.branch, "work"), (ctx.commercial(), &l.cbranch, "main")] {
        if let Some(oid) = branch_tip(&dir, branch) {
            preserve_commit(&dir, &oid, target)?;
            let remote = git(&dir, &["rev-parse", "-q", "--verify", &format!("refs/remotes/origin/{branch}")]).ok();
            checked.push((dir, branch.clone(), oid, remote));
        }
    }

    // The merge bot moves none of its files while it goes (the lane's write turn): a fast-forward
    // the bot made while `git worktree remove` deleted the tree wrote a file back into it, the
    // delete stopped at that file's dir, and git dropped the worktree with half its tree left
    // (wmcaption, 11-19). A half git let go of so goes once what is left of it is its branch's.
    crate::sync::lanes::lane_write_as(&path, "the lane's removal", || -> Result<(), String> {
        if let_go(&ctx.commercial(), &nested) {
            drop_let_go(&ctx.commercial(), &nested, &l.cbranch)?;
        } else if nested.join(".git").exists() {
            // removed from its own repository (the one its .git names), so neither keeps a dangling entry
            let common = wt::git(Some(&nested), None, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
            preserve_head(&nested, "main")?;
            let mut a = vec!["worktree", "remove"];
            a.extend_from_slice(force);
            let n = nested.to_string_lossy().into_owned();
            a.push(&n);
            wt::git(None, Some(Path::new(common.trim())), &a)?;
        }
        ctx.monitor.forget(&nested);
        if let_go(&ctx.repo, &path) {
            drop_let_go(&ctx.repo, &path, &l.branch)?;
        } else if path.exists() {
            preserve_head(&path, "work")?;
            let mut a = vec!["worktree", "remove"];
            a.extend_from_slice(force);
            let p = path.to_string_lossy().into_owned();
            a.push(&p);
            wt::git(Some(&ctx.repo), None, &a)?;
        }
        Ok(())
    })?;
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
    done.push(clean_node_dirs(ctx, l));
    Ok(done.join(", "))
}

/// The lane's dirs on every node that is up, as "node dirs (N MB: per node)".
fn clean_node_dirs(ctx: &Ctx, l: &LaneRec) -> String {
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
    format!("node dirs ({total:.0} MB{each})")
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
    // Remove slots under SLOTS before committing or bundling: a take cannot empty one after its lane was saved (11-15).
    let mut removed = super::slot::remove(ctx, &l, true)?;
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
    removed.push(remove(ctx, &l, true, true)?);
    let what = removed.join(", ");
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
    /// `since`: a registered lane's making and the land queue's entries ([`View::landed_since`]).
    fn audit_commits(&mut self, path: &Path, branch: &str, head: ObjectId, check_own: bool, nested: bool, since: Option<(u64, &[String])>) -> HalfAudit {
        let landed = match since {
            Some((ms, land)) if check_own => self.landed_since(branch, path, ms, land),
            _ => self.landed(branch, path, check_own),
        };
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
    let mut made: HashMap<PathBuf, u64> = HashMap::new();
    for l in state.as_ref().and_then(|v| v.get("lanes")).and_then(|v| v.as_arr()).unwrap_or(&[]) {
        let s = |k: &str| l.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        if s("state") == "live" {
            registered.insert(PathBuf::from(s("path")), (s("name"), s("note"), l.get("keep").and_then(|x| x.as_bool()).unwrap_or(false)));
            made.insert(PathBuf::from(s("path")), l.get("created_ms").and_then(|x| x.as_u64()).unwrap_or(0));
        }
    }
    let land: Vec<String> = state.as_ref().and_then(|v| v.get("land")).and_then(|v| v.as_arr()).unwrap_or(&[]).iter().map(|v| v.to_json()).collect();
    let since = |path: &Path| made.get(path).map(|ms| (*ms, land.as_slice()));
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
        let mut halves = vec![("makepad", mk.audit_commits(&path, &branch, head, true, nested.join(".git").exists(), since(&path)))];
        if let Some(i) = com_list.iter().position(|c| c.0 == nested) {
            let (cp, cb, ch) = com_list.swap_remove(i);
            if let Some(c) = com.as_mut() {
                halves.push(("commercial", c.audit_commits(&cp, &cb, ch, false, false, None)));
            }
        }
        let reg = registered.get(&path).cloned();
        lanes.push((name, path, reg, halves));
    }
    for (path, branch, head) in com_list {
        let name = path.strip_prefix(&root).map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
        if let Some(c) = com.as_mut() {
            let h = c.audit_commits(&path, &branch, head, true, false, since(&path));
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
    // A registered lane's sibling slots are never sweep archives (11-15).
    if name.strip_suffix(".slots").is_some_and(|name| ctx.store.read(|d| d.lanes.iter().any(|l| l.name == name))) {
        return Item { name, path: path.to_path_buf(), kind: "lane slots".into(), git: None, clone: false };
    }
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
    if item.kind == "lane slots" {
        p.keep = "a recorded lane's slots".into();
        return p;
    }
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
        let o = std::process::Command::new(crate::os::tool("df")).arg("-k").arg(p).output();
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
        let mut cmd = std::process::Command::new(crate::os::tool("nice"));
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
        let t = std::process::Command::new(crate::os::tool("tar")).arg("-t").arg("-f").arg(&archive).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::piped()).output().map_err(|e| e.to_string())?;
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
/// The bucket shards' units far past their measure, each with its box as the lease scheduler sees
/// it now ([`crate::bucket::slow_units`]), and CI's shards still running past their overdue verdict
/// ([`super::ci::overdue_running`]): the check-in's rows, and `devhub bucket`'s beside the run.
pub(crate) fn slow_now(ctx: &Ctx, now: u64) -> Vec<String> {
    let (leases, nodes) = (ctx.leases.view(), ctx.leases.nodes());
    let shard_box = |id: &str| {
        let node = leases.iter().find(|l| l.id == id && !matches!(l.state, crate::lease::sched::State::Ended(_)))?.node.clone()?;
        let caps = nodes.iter().find(|n| n.name == node).map(|n| (n.up, n.caps.as_ref()));
        Some(crate::bucket::ShardBox { up: caps.is_some_and(|(up, _)| up), load: caps.and_then(|(_, c)| c).map_or(0.0, |c| c.load1), cores: caps.and_then(|(_, c)| c).map_or(0, |c| c.cores), node })
    };
    let mut out = crate::bucket::slow_units(&ctx.home, now, &shard_box);
    // and CI's shards past their overdue verdict whose lease still runs
    let live = |id: &str| leases.iter().any(|l| l.id == id && !matches!(l.state, crate::lease::sched::State::Ended(_)));
    out.extend(ctx.store.read(|d| super::ci::overdue_running(&d.ci, now, &live)));
    out
}

/// How long a node goes unheard before the check-in lists it ([`down_nodes`]).
const NODE_DOWN_LISTED_MS: u64 = 3_600_000;

/// For the manager's check-in (`lane quiet`): the nodes unheard past [`NODE_DOWN_LISTED_MS`], how
/// long and why. A Windows box's hub ended at a logoff and its box stayed out of the fleet two
/// days; `devhub nodes` said "not heard" to whoever looked, and no check-in listed it (11-14).
pub(crate) fn down_nodes(nodes: &[crate::lease::sched::NodeView]) -> Vec<String> {
    let down: Vec<String> = nodes.iter().filter_map(|n| n.unheard.as_ref().filter(|(ms, _)| *ms >= NODE_DOWN_LISTED_MS).map(|(ms, why)| {
        format!("  {}: {:.1} h: {}", n.name, *ms as f64 / 3_600_000.0, crate::proto::clean(why, 200))
    })).collect();
    if down.is_empty() { return down; }
    let mut rows = vec![format!("nodes down over {} h (their hub not heard; `devhub nodes` says why):", NODE_DOWN_LISTED_MS / 3_600_000)];
    rows.extend(down);
    rows
}

/// How many asks one lane has queued at once before the check-in lists it ([`deep_queues`]).
const DEEP_QUEUE: usize = 10;

/// For the manager's check-in (`lane quiet`): the lanes with [`DEEP_QUEUE`] or more asks queued at
/// once, the most first, with how long the oldest has waited. Lane caps run them a few at a time,
/// but the queue view and its starvation lines fill with them: a lane and its edit lane had 22
/// test asks queued at once (11-16). `queued`: each queued ask's lane and when it asked.
pub(crate) fn deep_queues<'a>(queued: impl Iterator<Item = (&'a str, u64)>, now: u64) -> Vec<String> {
    let mut by: BTreeMap<&str, (usize, u64)> = BTreeMap::new();
    for (lane, asked) in queued {
        let e = by.entry(lane).or_insert((0, asked));
        e.0 += 1;
        e.1 = e.1.min(asked);
    }
    let mut deep: Vec<(&str, (usize, u64))> = by.into_iter().filter(|(_, (n, _))| *n >= DEEP_QUEUE).collect();
    if deep.is_empty() { return Vec::new(); }
    deep.sort_by(|a, b| b.1 .0.cmp(&a.1 .0).then(a.0.cmp(b.0)));
    let mut rows = vec![format!("lanes with {DEEP_QUEUE} or more asks queued at once (ask them to batch: one `devhub test` with several filters, or one `job submit --batch`):")];
    rows.extend(deep.into_iter().map(|(lane, (n, oldest))| format!("  {lane}: {n} queued, the oldest {} min", now.saturating_sub(oldest) / 60_000)));
    rows
}

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
    let silent = ctx.leases.output_silent(now);
    if !silent.is_empty() {
        rows.push(format!("leases whose output grew none for {} min or more (busy or not):", crate::lease::sched::OUTPUT_SILENT_MS / 60_000));
        rows.extend(silent);
    }
    rows.extend(super::live::restarts_pending(ctx, now));
    rows.extend(slow_now(ctx, now));
    rows.extend(down_nodes(&ctx.leases.nodes()));
    let stale = ctx.leases.stale_boxes(now);
    if !stale.is_empty() {
        rows.push("boxes that run older code than their checkout (their WM or JIT apps):".to_string());
        rows.extend(stale);
    }
    let view = ctx.leases.view();
    rows.extend(deep_queues(view.iter().filter(|l| l.state == crate::lease::sched::State::Queued).map(|l| (l.req.lane.as_str(), l.asked_ms)), now));
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
        *e = (*e).max(wrote).max(l.active_ms);
    }
    retire_offers(lanes, ended, busy, &active, &|a| ctx.leases.agent_seen_ms(a), now)
}

/// `lane offers`: each lane offered for retirement, its branches' hold and what of its runs would
/// refuse it (a process inside, a lease, job or hand-in open). Status, size and merged content are
/// read by the asking CLI, never on the master's control thread.
fn offers(ctx: &Ctx) -> String {
    let lanes: Vec<LaneRec> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.ended_ms == 0).cloned().collect());
    let held = ctx.store.read(|d| d.held.clone());
    let offers = offered(ctx, &lanes, &ended_runs(ctx, &lanes), &in_flight(ctx), wall_ms());
    let (cwds, land) = (wt::live_cwds(), land_entries(ctx));
    let rows: Vec<String> = offers.iter().filter_map(|o| {
        let l = lanes.iter().find(|l| l.name == o.name)?;
        let (class, why) = run_refusal(ctx, l, &cwds, &land).map_or(("", String::new()), |(c, w)| (c, w));
        let b = branches(l);
        let held = held.iter().find(|(branch, _)| b.contains(branch)).map_or("", |(_, why)| why.as_str());
        Some(format!("{{\"name\":{},\"path\":{},\"branch\":{},\"cbranch\":{},\"idle_ms\":{},\"row\":{},\"refused_class\":{},\"refused\":{},\"held\":{}}}", json_str(&l.name), json_str(&l.path), json_str(&l.branch), json_str(&l.cbranch), o.idle_ms, json_str(&o.row), json_str(class), json_str(&why), json_str(held)))
    }).collect();
    format!("{{\"lanes\":[{}]}}", rows.join(","))
}

/// `retire --clean-offers` checks content here, in the CLI, with land's retired-chain merge (11-14).
pub(crate) fn clean_offer_refusal(offer: &makepad_strict_json::Value) -> Option<String> {
    let s = |k: &str| offer.get(k).and_then(|v| v.as_str()).unwrap_or("");
    if !s("refused").is_empty() { return Some(s("refused").to_string()); }
    if !s("held").is_empty() { return Some(format!("its branch is held: {}", s("held"))); }
    let path = Path::new(s("path"));
    if let Some((_, why)) = dirty_refusal(path) { return Some(why); }
    // An older master's offer has no branch names: never guess what may be removed.
    if s("branch").is_empty() || offer.get("cbranch").and_then(|v| v.as_str()).is_none() {
        return Some("its branches are not known (lane offers needs a newer master)".into());
    }
    for (dir, target, branch) in [(path.to_path_buf(), "work", s("branch")), (path.join("apps/commercial"), "main", s("cbranch"))] {
        if branch.is_empty() { continue; }
        match crate::land::changes_nothing(&dir, &format!("refs/heads/{target}"), &format!("refs/heads/{branch}")) {
            Ok(true) => {}
            Ok(false) => return Some(format!("{branch} has changes {target} lacks")),
            Err(why) => return Some(format!("its branch {branch} does not merge cleanly into {target}: {why}")),
        }
    }
    None
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

/// The newest job, live session or lease of these lanes that ended (11-15).
fn ended_runs(ctx: &Ctx, lanes: &[LaneRec]) -> HashMap<String, Ended> {
    use std::io::BufRead;
    // Only identity and activity cross the lock; live source maps and output stay in the store.
    let lives: Vec<_> = ctx.store.read(|d| d.lives.iter().map(|x| crate::state::Live { id: x.id.clone(), lane: x.lane.clone(), tree: x.tree.clone(), lease: x.lease.clone(), state: x.state.clone(), ended_ms: x.ended_ms, touched_ms: x.touched_ms, agent: x.agent.clone(), ..Default::default() }).collect());
    let runs: Vec<_> = lanes.iter().map(|l| LaneRuns::new(l, &lives)).collect();
    // JSON-escaped prefixes also match a nested repo_dir (and paths with backslashes).
    let hints: Vec<String> = runs.iter().flat_map(|r| {
        let mut hints = vec![format!("\"lane\":{}", json_str(&r.lane.name)), format!("\"lane\":{}", json_str(&r.lane.branch))];
        if !r.lane.path.is_empty() { hints.push(format!("\"repo_dir\":{}", json_str(&r.lane.path).trim_end_matches('"'))); }
        hints.extend(r.lives.iter().filter(|x| !x.lease.is_empty()).map(|x| format!("\"id\":{}", json_str(&x.lease))));
        hints
    }).collect();
    let mut ended: HashMap<String, Ended> = HashMap::new();
    let mut note = |lane: &str, at: u64, what: String, agent: &str| {
        let last = ended.entry(lane.to_string()).or_default();
        if at >= last.at { *last = Ended { at, what, agent: agent.to_string() }; }
    };
    if let Ok(file) = std::fs::File::open(ctx.home.join("leases.jsonl")) {
        for line in std::io::BufReader::new(file).lines().map_while(Result::ok) {
            // the cheap look first: the ledger holds every lease there was
            if !hints.iter().any(|hint| line.contains(hint)) { continue; }
            let Ok(v) = makepad_strict_json::parse(line.as_bytes()) else { continue };
            let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("");
            let req = |k: &str| v.get("req").and_then(|r| r.get(k)).and_then(|x| x.as_str()).unwrap_or("");
            for r in &runs {
                if r.lease(text("id"), if req("lane").is_empty() { text("lane") } else { req("lane") }, req("repo_dir")) {
                    note(&r.lane.name, v.get("ended_ms").and_then(|x| x.as_u64()).unwrap_or(0), format!("{} {}@{} {}", text("id"), text("kind"), text("node"), text("how")), req("agent"));
                }
            }
        }
    }
    for r in &runs {
        for x in r.lives.iter().filter(|x| !x.active()) {
            note(&r.lane.name, x.ended_ms.max(x.touched_ms), format!("{} {}", x.id, x.state), &x.agent);
        }
    }
    ctx.store.read(|d| for r in &runs {
        for j in d.jobs.iter().filter(|j| j.done() && r.named(&j.lane)) {
            note(&r.lane.name, j.ended_ms, format!("{} {}", j.id, j.state), &j.agent);
        }
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
        "slot" => super::slot::serve(ctx, rest),
        "slots" => super::slot::serve(ctx, &super::slot::request(&["slots", rest])?),
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

    /// A lane with ten or more asks queued is listed, the most first, with its oldest's wait; one
    /// with fewer is not.
    #[test]
    fn the_check_in_lists_a_lane_with_many_asks_queued() {
        let mut queued: Vec<(&str, u64)> = (0..12).map(|i| ("elang", i * 60_000)).collect();
        queued.extend((0..10).map(|i| ("elangedit", 5 * 60_000 + i)));
        queued.extend((0..9).map(|i| ("eroute", i)));
        let rows = deep_queues(queued.into_iter(), 20 * 60_000);
        assert_eq!(rows[1..], ["  elang: 12 queued, the oldest 20 min".to_string(), "  elangedit: 10 queued, the oldest 15 min".to_string()]);
        assert!(rows[0].starts_with("lanes with 10 or more asks queued at once"), "{}", rows[0]);
        assert!(deep_queues([("eroute", 0u64); 9].into_iter(), 0).is_empty(), "nine is no deep queue");
    }

    /// A node unheard past an hour is listed with how long and why; one heard, or unheard for
    /// less, is not, and with none down there is no row at all.
    #[test]
    fn the_check_in_lists_a_node_down_over_an_hour() {
        use crate::lease::sched::NodeView;
        let node = |name: &str, unheard: Option<(u64, &str)>| NodeView { name: name.into(), up: unheard.is_none(), unheard: unheard.map(|(ms, why)| (ms, why.to_string())), ..Default::default() };
        let why = "link lost: cannot reach fleet hub `w` at 192.0.2.5:8797: connection timed out";
        let rows = down_nodes(&[node("w", Some((49 * 3_600_000, why))), node("a", None), node("b", Some((20 * 60_000, "its caps call has not answered")))]);
        assert_eq!(rows, vec!["nodes down over 1 h (their hub not heard; `devhub nodes` says why):".to_string(), format!("  w: 49.0 h: {why}")]);
        assert!(down_nodes(&[node("a", None), node("b", Some((20 * 60_000, "x")))]).is_empty());
    }

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

    /// Synthetic live leases, later landings and file activity survive cleanup/restart (11-15).
    #[test]
    fn landed_lane_waits_for_live_runs_landings_and_durable_activity() {
        use crate::lease::{sched::Req, Kind};
        use crate::state::{Doc, Live};
        let base = std::env::temp_dir().join(format!("devhub-lane-live-cleanup-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let mk = base.join("makepad");
        repo(&mk, "work");
        let home = base.join("state");
        std::fs::create_dir_all(&home).unwrap();
        let leases = Arc::new(LeaseMaster::new("", home.clone(), Arc::new(|_: &str, _: &str, _: &str| Err("no nodes".into())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        let mut ctx = Ctx { store: Store::open(&home, "", Arc::new(|_| 0)).unwrap(), leases, machine: String::new(), monitor: crate::monitor::Monitor::start(&[]), repo: mk.clone(), home: home.clone() };
        serve(&ctx, "lane new {\"name\":\"x\",\"commercial\":false}").unwrap();
        let lane = base.join("wt.noindex/x");
        let nested = lane.join("apps/commercial");
        std::fs::create_dir_all(&nested).unwrap();
        g(&lane, &["commit", "-q", "--allow-empty", "-m", "lane work"]);
        g(&mk, &["merge", "-q", "--no-ff", "-m", "land", "rc-x"]);
        let (now, old) = (wall_ms(), wall_ms() - 2 * GRACE_MS);
        set(&ctx, "x", |l| l.landed_ms = old);
        let kept = |ctx: &Ctx, why: &str| {
            // No process scan or watcher timing: this fixture owns every file and run.
            cleanup_pass(ctx, Vec::new);
            let l = ctx.store.read(|d| d.lanes[0].clone());
            assert!(l.state == "live" && lane.exists(), "{why}: {} ({})", l.state, l.note);
            l
        };
        ctx.store.update(|d| d.lives.push(Live { id: "W1".into(), lane: "other".into(), tree: lane.to_string_lossy().into_owned(), state: "running".into(), ..Default::default() })).unwrap();
        let l = kept(&ctx, "an active session keeps its worktree despite a different lane name");
        assert!(l.note.contains("live job W1 runs from this worktree"), "the refusal names the live job: {}", l.note);
        assert_eq!(saved_open_run(&home, &l).unwrap().as_deref(), Some("live job W1 runs from this worktree"), "prune sees the durable active session");
        let mut views = Views::open(&ctx).unwrap();
        assert!(removable(&ctx, &l, &mut views, &[], &[], true).unwrap_err().contains("live job W1"), "explicit removal keeps the active session too");
        ctx.store.update(|d| d.lives[0].tree = nested.to_string_lossy().into_owned()).unwrap();
        assert!(kept(&ctx, "a live session inside commercial keeps the outer worktree").note.contains("W1"), "nested session is named");
        ctx.store.update(|d| d.lives.clear()).unwrap();

        // Each lease is matched once by its root, its nested repo_dir, or the live session's id.
        for tree in [lane.to_string_lossy().into_owned(), nested.to_string_lossy().into_owned(), String::new()] {
            let mut req = Req::new("live-w1", Kind::Test);
            req.repo_dir = (!tree.is_empty()).then(|| tree.clone());
            let mut id = String::new();
            ctx.leases.test_sched(|s| {
                id = s.request(req.clone(), now);
                std::fs::write(home.join("leases.live"), s.live_lines().join("\n")).unwrap();
            });
            if tree.is_empty() {
                ctx.store.update(|d| d.lives.push(Live { id: "W1".into(), lane: "x".into(), lease: id.clone(), state: "ended".into(), ended_ms: old, touched_ms: old, ..Default::default() })).unwrap();
            }
            let l = kept(&ctx, "a synthetic open lease keeps the lane by path or session linkage");
            let why = format!("a lease of the lane is open ({id})");
            assert!(l.note.contains(&why), "the refusal names the lease: {}", l.note);
            assert!(removable(&ctx, &l, &mut views, &[], &[], true).unwrap_err().contains(&why), "explicit removal keeps the synthetic lease");
            if !tree.is_empty() { assert_eq!(saved_open_run(&home, &l).unwrap(), Some(why), "prune sees the durable lease's repo_dir"); }
            ctx.leases.test_sched(|s| { assert!(s.release(&id, "released", old), "the fixture lease ends"); });
            let ledger = |at| std::fs::write(home.join("leases.jsonl"), format!("{{\"id\":{},\"lane\":\"live-w1\",\"ended_ms\":{at},\"req\":{}}}\n", json_str(&id), req.to_json())).unwrap();
            ledger(now);
            kept(&ctx, "a recently ended synthetic lease keeps the lane by path or session linkage");
            ledger(old);
            ctx.store.update(|d| d.lives.clear()).unwrap();
        }
        std::fs::write(home.join("leases.live"), "").unwrap();
        ctx.store.update(|d| d.lives.push(Live { id: "W2".into(), lane: "x".into(), state: "ended".into(), ended_ms: now, touched_ms: old, ..Default::default() })).unwrap();
        kept(&ctx, "a recently ended live session keeps the lane by name");
        ctx.store.update(|d| { d.lives[0].ended_ms = old; d.lives[0].touched_ms = now; }).unwrap();
        kept(&ctx, "a live session's last touch also starts the grace");
        ctx.store.update(|d| d.lives[0].touched_ms = old).unwrap();

        for (branch, cbranch) in [("rc-x", ""), ("other", "rc-commercial")] {
            set(&ctx, "x", |l| { l.landed_ms = old; l.cbranch = "rc-commercial".into(); });
            let entry = |at| makepad_strict_json::parse(format!(r#"{{"branch":"{branch}","cbranch":"{cbranch}","state":"landed","changed_ms":{at}}}"#).as_bytes()).unwrap();
            ctx.store.update(|d| d.land = vec![entry(now), entry(old)]).unwrap();
            assert_eq!(kept(&ctx, "the newest landing of either branch restarts the grace").landed_ms, now, "the later landing advances the saved clock regardless of queue order");
            ctx.store.update(|d| d.land.clear()).unwrap();
            set(&ctx, "x", |l| { l.landed_ms = old; l.cbranch.clear(); });
        }

        for tree in [&lane, &nested] {
            ctx.monitor.set_last_event_ms(tree, now);
            assert_eq!(kept(&ctx, "activity in either half keeps the lane").active_ms, now, "file activity is saved");
            ctx.monitor = crate::monitor::Monitor::start(&[]);
            let saved = crate::state::committed(&home).unwrap().unwrap();
            assert_eq!(Doc::from_value(&saved).lanes[0].active_ms, now, "the activity reached durable state");
            assert_eq!(ctx.monitor.last_event_ms(tree), 0, "the new monitor has heard nothing");
            kept(&ctx, "persisted activity protects the lane after a master restart");
            set(&ctx, "x", |l| l.active_ms = old);
        }
        let legacy = Doc::parse(r#"{"lanes":[{"name":"x","state":"live"}]}"#).unwrap();
        assert_eq!(legacy.lanes[0].active_ms, 0, "old lane records load without activity");
        let other = format!("{}-other", lane.to_string_lossy());
        ctx.store.update(|d| d.lives.push(Live { id: "W3".into(), lane: "other".into(), tree: other.clone(), state: "running".into(), ..Default::default() })).unwrap();
        std::fs::write(home.join("leases.jsonl"), format!("{{\"id\":\"L9\",\"lane\":\"live-w3\",\"ended_ms\":{now},\"req\":{{\"repo_dir\":{}}}}}\n", json_str(&other))).unwrap();
        drop(views);
        // The node dirs go first; the worktree and branches once the lane's agents are idle too (11-16).
        set(&ctx, "x", |l| l.created_ms = old - GRACE_MS);
        let entry = makepad_strict_json::parse(format!(r#"{{"branch":"rc-x","agent":"AGENT-X","state":"landed","changed_ms":{old}}}"#).as_bytes()).unwrap();
        ctx.store.update(|d| d.land = vec![entry]).unwrap();
        let restart = |ctx: &mut Ctx| ctx.leases = Arc::new(LeaseMaster::new("", home.clone(), Arc::new(|_: &str, _: &str, _: &str| Err("no nodes".into())), Arc::new(|_| {}), Arc::new(wall_ms), None, true));
        restart(&mut ctx);
        ctx.leases.agent_heard("AGENT-X");
        let heard = ctx.leases.agent_seen_ms("AGENT-X");
        let l = kept(&ctx, "all lane activity is old and a sibling with the same path prefix is unrelated, but its agent called");
        assert!(l.dirs_ms >= now && l.note.contains("node dirs (0 MB) cleaned"), "its node dirs went: {l:?}");
        assert_eq!(l.agent_ms, heard, "the hand-in's agent's call is saved");
        kept(&ctx, "a devhub call of the lane's agent within the hour keeps its worktree");
        restart(&mut ctx);
        assert_eq!(ctx.leases.agent_seen_ms("AGENT-X"), 0, "the restarted master heard no one");
        assert_eq!(kept(&ctx, "the saved call keeps it after a master restart").dirs_ms, l.dirs_ms, "the node dirs are cleaned once per quiet spell");
        set(&ctx, "x", |l| l.agent_ms = old);
        cleanup_pass(&ctx, Vec::new);
        let l = ctx.store.read(|d| d.lanes[0].clone());
        assert_eq!(l.state, "removed", "its agent idle for the hour too: {}", l.note);
        assert!(!lane.exists() && g(&mk, &["branch", "--list", "rc-x"]).is_empty(), "the idle landed worktree and branch are removed");
        drop(ctx);
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
        // the merge bot fast-forwards it to work: its tip is one of work's own commits then, as a
        // fresh lane's is. The train's record of its hand-in says it landed; one of an earlier
        // lane of the name does not.
        let own = g(&ran, &["rev-parse", "HEAD"]);
        g(&ran, &["merge", "-q", "--ff-only", "work"]);
        let landed_ms = || ctx.store.read(|d| d.lanes.iter().rev().find(|l| l.name == "r").map(|l| l.landed_ms).unwrap());
        cleanup(&ctx);
        assert_eq!(landed_ms(), 0, "{:?}", ctx.store.read(|d| d.lanes.last().map(|l| l.note.clone())));
        let made = ctx.store.read(|d| d.lanes.iter().rev().find(|l| l.name == "r").unwrap().created_ms);
        let hand_in = |asked_ms: u64| makepad_strict_json::parse(format!(r#"{{"id":10,"branch":"rc-r","state":"landed","ms":"{own}","asked_ms":{asked_ms}}}"#).as_bytes()).unwrap();
        ctx.store.update(|d| d.land = vec![hand_in(made - 1)]).unwrap();
        cleanup(&ctx);
        assert_eq!(landed_ms(), 0);
        ctx.store.update(|d| d.land = vec![hand_in(made)]).unwrap();
        cleanup(&ctx);
        assert!(landed_ms() > 0, "{:?}", ctx.store.read(|d| d.lanes.last().map(|l| l.note.clone())));
        set(&ctx, "r", |x| x.landed_ms = 1);
        let ledger = |ended_ms: u64| std::fs::write(home.join("leases.jsonl"), format!("{{\"id\":\"L7\",\"lane\":\"rc-r\",\"kind\":\"screen\",\"node\":\"n1\",\"how\":\"released\",\"ended_ms\":{ended_ms}}}\n")).unwrap();
        ledger(wall_ms());
        cleanup(&ctx);
        assert_eq!(state("r"), "live");
        ledger(wall_ms() - GRACE_MS - 1);
        cleanup(&ctx);
        // its node dirs go at the grace; its worktree waits an hour since that run too (11-16)
        assert_eq!(state("r"), "live");
        assert!(ctx.store.read(|d| d.lanes.iter().rev().find(|l| l.name == "r").unwrap().dirs_ms) > 0, "its node dirs went");
        set(&ctx, "r", |x| x.agent_ms = 1);
        cleanup(&ctx);
        assert_eq!(state("r"), "live", "a run that ended within the hour keeps the worktree with no agent call saved");
        ledger(wall_ms() - AGENT_IDLE_MS - 1);
        set(&ctx, "r", |x| x.agent_ms = 1);
        cleanup(&ctx);
        assert_eq!(state("r"), "removed");
        assert!(!ran.exists());
        // git let go of a lane partway through removing it (the merge bot wrote a file into it as
        // `git worktree remove` deleted it: the admin dir went, part of the tree stayed, with no
        // HEAD; wmcaption, 11-19): asked again, it goes once what is left of it is its branch's
        serve(&ctx, "lane new {\"name\":\"half\"}").unwrap();
        let half = base.join("wt.noindex/half");
        std::fs::remove_dir_all(mk.join(".git/worktrees/half")).unwrap();
        std::fs::write(half.join("a.txt"), "not the branch's").unwrap();
        std::fs::write(half.join("new.txt"), "n").unwrap();
        let e = serve(&ctx, "lane remove half").unwrap_err();
        assert!(e.contains("git let go of it mid-removal, and 2 file(s) left in it are not rc-half's: a.txt, new.txt"), "{e}");
        assert!(half.exists() && state("half") == "live");
        std::fs::remove_file(half.join("a.txt")).unwrap();
        std::fs::remove_file(half.join("new.txt")).unwrap();
        serve(&ctx, "lane remove half").unwrap();
        assert_eq!(state("half"), "removed", "{:?}", ctx.store.read(|d| d.lanes.iter().rev().find(|l| l.name == "half").map(|l| l.note.clone())));
        assert!(!half.exists() && g(&mk, &["branch", "--list", "rc-half"]).is_empty(), "the rest of its tree and its branch went");
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
    /// reason; a node that did not answer is named. `retire --clean-offers` also keeps held lanes
    /// and changes absent from either target, using land's merge through retired chains (11-14).
    #[test]
    fn offers_are_checked_by_what_would_refuse_them() {
        let base = std::env::temp_dir().join(format!("devhub-lane-offers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for name in ["clean", "untracked", "leased"] {
            repo(&base.join(name), "rc-x");
            g(&base.join(name), &["branch", "work"]);
        }
        std::fs::write(base.join("untracked/notes.txt"), "n").unwrap();
        let lane = |name: &str, class: &str| format!("{{\"name\":\"{name}\",\"path\":{},\"branch\":\"rc-x\",\"cbranch\":\"\",\"held\":\"\",\"refused_class\":\"{class}\",\"refused\":\"{class}\"}}", json_str(&base.join(name).to_string_lossy()));
        let offers = makepad_strict_json::parse(format!("{{\"lanes\":[{},{},{}]}}", lane("clean", ""), lane("untracked", ""), lane("leased", "a lease of the lane is open")).as_bytes()).unwrap();
        let nodes = vec![("box-a".to_string(), Ok(makepad_strict_json::parse(br#"{"lanes":{"clean":1073741824,"other":5}}"#).unwrap())), ("box-b".to_string(), Err("link lost".to_string()))];
        let out = check_offers(&offers, &nodes);
        assert!(out.starts_with("3 lane(s) offered for retirement (nothing retired)\n"), "{out}");
        let clean = out.lines().find(|l| l.starts_with("retire cleanly")).unwrap_or_default();
        assert!(clean.split_whitespace().nth(2) == Some("1") && clean.ends_with("1.0 GB"), "{out}");
        assert!(out.contains("uncommitted or untracked files: untracked\n") && out.contains("a lease of the lane is open: leased\n"), "{out}");
        assert!(out.contains("node dirs not read: box-b (link lost)"), "{out}");
        let list = offers.get("lanes").unwrap().as_arr().unwrap();
        assert_eq!(clean_offer_refusal(&list[0]), None, "a fresh clean branch changes nothing");
        assert!(clean_offer_refusal(&list[1]).unwrap().contains("notes.txt"), "untracked work keeps the lane");
        assert_eq!(clean_offer_refusal(&list[2]).as_deref(), Some("a lease of the lane is open"));
        let held = makepad_strict_json::parse(br#"{"held":"timed work"}"#).unwrap();
        assert_eq!(clean_offer_refusal(&held).as_deref(), Some("its branch is held: timed work"));

        // Land by content, then merge work back into the lane: its tip is not in work (11-14).
        let clean = base.join("clean");
        std::fs::write(clean.join("landed.txt"), "landed").unwrap();
        g(&clean, &["add", "."]);
        g(&clean, &["commit", "-q", "-m", "lane change"]);
        g(&clean, &["checkout", "-q", "work"]);
        g(&clean, &["merge", "--squash", "rc-x"]);
        g(&clean, &["commit", "-q", "-m", "land lane change"]);
        std::fs::write(clean.join("other.txt"), "another landing").unwrap();
        g(&clean, &["add", "."]);
        g(&clean, &["commit", "-q", "-m", "another landing"]);
        g(&clean, &["checkout", "-q", "rc-x"]);
        g(&clean, &["merge", "-q", "--no-ff", "-m", "merge work", "work"]);
        assert!(!crate::gitread::is_ancestor(&clean, "rc-x", "work"), "the merge bot's tip need not be an ancestor");
        assert_eq!(clean_offer_refusal(&list[0]), None, "landed content retires after merging work");

        std::fs::write(clean.join("a.txt"), "lane change").unwrap();
        g(&clean, &["add", "."]);
        g(&clean, &["commit", "-q", "-m", "unlanded change"]);
        assert_eq!(clean_offer_refusal(&list[0]).as_deref(), Some("rc-x has changes work lacks"));
        let before = g(&clean, &["rev-parse", "work"]);
        g(&clean, &["checkout", "-q", "work"]);
        std::fs::write(clean.join("a.txt"), "conflicting change").unwrap();
        g(&clean, &["add", "."]);
        g(&clean, &["commit", "-q", "-m", "conflicting change"]);
        assert_eq!(clean_offer_refusal(&list[0]).as_deref(), Some("its branch rc-x does not merge cleanly into work: a.txt"));
        g(&clean, &["reset", "--hard", &before]);

        // Its changes landed on the retired chain; newer work has changed the same file again.
        g(&clean, &["merge", "-q", "--no-ff", "-m", "land remaining change", "rc-x"]);
        g(&clean, &["branch", "work-old-2026-11-14", "work"]);
        let tree = g(&clean, &["rev-parse", "work^{tree}"]);
        let squashed = g(&clean, &["commit-tree", &tree, "-m", "squashed work"]);
        g(&clean, &["reset", "--hard", &squashed]);
        std::fs::write(clean.join("a.txt"), "after promotion").unwrap();
        g(&clean, &["add", "."]);
        g(&clean, &["commit", "-q", "-m", "after promotion"]);
        g(&clean, &["checkout", "-q", "rc-x"]);
        assert_eq!(clean_offer_refusal(&list[0]), None, "retired-chain content must not be counted again");

        repo(&clean.join("apps/commercial"), "rc-commercial");
        g(&clean.join("apps/commercial"), &["branch", "main"]);
        let pair = makepad_strict_json::parse(lane("clean", "").replace("\"cbranch\":\"\"", "\"cbranch\":\"rc-commercial\"").as_bytes()).unwrap();
        assert_eq!(clean_offer_refusal(&pair), None, "both halves have landed");
        std::fs::write(clean.join("apps/commercial/a.txt"), "commercial change").unwrap();
        g(&clean.join("apps/commercial"), &["add", "."]);
        g(&clean.join("apps/commercial"), &["commit", "-q", "-m", "commercial change"]);
        assert_eq!(clean_offer_refusal(&pair).as_deref(), Some("rc-commercial has changes main lacks"));
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
        ctx.store.update(|d| d.lanes.push(LaneRec { name: "offered".into(), path: base.join("offered").to_string_lossy().into_owned(), branch: "rc-offered".into(), cbranch: "rc-offered-commercial".into(), state: "live".into(), created_ms: 1, ..LaneRec::default() })).unwrap();
        let offered = || {
            let v = makepad_strict_json::parse(offers(&ctx).as_bytes()).unwrap();
            v.get("lanes").unwrap().as_arr().unwrap()[0].clone()
        };
        assert_eq!(offered().get("held").and_then(|v| v.as_str()), Some(""));
        for branch in ["rc-offered", "rc-offered-commercial"] {
            crate::sync::lanes::hold_in(&ctx.store, branch, "timed work").unwrap();
            let offer = offered();
            assert_eq!(offer.get("held").and_then(|v| v.as_str()), Some("timed work"), "a hold on either half is reported");
            assert_eq!(clean_offer_refusal(&offer).as_deref(), Some("its branch is held: timed work"));
            crate::sync::lanes::unhold_in(&ctx.store, branch).unwrap();
        }
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
