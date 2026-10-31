//! The lane worktree lifecycle, owned by the master (lanes never run `git worktree` themselves):
//! - `devhub lane new <name> [--from work] [--no-commercial]`: `<lanes root>/<name>` on `rc-<name>`
//!   from work, with the nested `apps/commercial` on `rc-<name>` from main; registered and
//!   watched ([`crate::monitor`]: its dirty files without scans).
//! - after it lands, the master removes it: both halves in work and main (the branch tips are in
//!   them and are not work's own first-parent commits, so a fresh lane never counts), nothing
//!   uncommitted, no process inside, no lease, job or land entry of the lane open, and no file
//!   activity for [`GRACE_MS`]. Then the worktree pair, the branches (local, and origin's when
//!   pushed) and the lane's dirs on every node go. `devhub lane keep <name>` exempts a lane.
//! - `devhub lane drop <name>`: abandons a lane: uncommitted work is committed on its branch,
//!   every commit not in work / main is bundled into `~/.makepad/devhub/dropped/`, then
//!   everything goes.
//! - `devhub lane adopt`: registers the existing live worktrees under the lanes root and lists the
//!   landed ones (it removes nothing); `devhub lane remove <name>` removes one of those once the
//!   user agreed (the same checks as the automatic cleanup, without the grace).
//! - `devhub lanes`: each lane's worktree, tip, dirty files, merge state (the merge bot's), land
//!   state (the train's).

use super::{git, is_ancestor, Ctx};
use crate::lease::wt;
use crate::proto::{wall_ms, Sev};
use crate::state::LaneRec;
use makepad_remote::json_str;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// A landed lane is removed only after this long with no file activity and no new commit.
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
    ctx.store.update(|d| {
        let l = d.lanes.iter_mut().rev().find(|l| l.name == name && l.state == "live")?;
        f(l);
        Some(l.clone())
    })
}

pub fn start(ctx: Arc<Ctx>) {
    // follow the registered lanes from the start (their dirty sets are ready for the first job)
    let live: Vec<String> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live").map(|l| l.path.clone()).collect());
    let c2 = ctx.clone();
    let _ = std::thread::Builder::new().name("devhub-lanes-clean".into()).spawn(move || {
        for p in live {
            let _ = c2.monitor.dirty(Path::new(&p));
            let nested = Path::new(&p).join("apps/commercial");
            if nested.join(".git").exists() {
                let _ = c2.monitor.dirty(&nested);
            }
        }
        loop {
            std::thread::sleep(TICK);
            cleanup(&c2);
        }
    });
}

/// `git rev-list --first-parent <tip>`: work's (main's) own commits.
fn first_parents(dir: &Path, tip: &str) -> HashSet<String> {
    git(dir, &["rev-list", "--first-parent", tip]).map(|o| o.lines().map(str::to_string).collect()).unwrap_or_default()
}

fn branch_tip(dir: &Path, branch: &str) -> Option<String> {
    git(dir, &["rev-parse", "-q", "--verify", &format!("refs/heads/{branch}^{{commit}}")]).ok()
}

/// One half landed: its own commits are in `target` (they reach it, and the newest is not one of
/// `target`'s own first-parent commits, so a fresh lane never counts). The merge bot's merges of
/// the tip into the lane ("Merge work (<sha>) into rc-x") are not the lane's own: a lane that
/// landed and then got the tip merged in again is landed. `tree`: a detached worktree's HEAD.
fn half_landed(dir: &Path, branch: &str, tree: &Path, target: &str, own: Option<&HashSet<String>>) -> Result<(), String> {
    let tip = if branch.is_empty() { crate::monitor::read_head(tree).map(|h| h.0).ok_or_else(|| format!("{}: no HEAD", tree.display()))? } else { branch_tip(dir, branch).ok_or_else(|| format!("no branch {branch}"))? };
    let name = if branch.is_empty() { "its detached HEAD" } else { branch };
    let bot = |s: &str| s.starts_with(&format!("Merge {target} (")) && s.ends_with(&format!(") into {branch}"));
    // the lane's newest own commit: down its first-parent line past the bot's merges
    let line = git(dir, &["log", "--first-parent", "-n", "200", "--format=%H%x09%s", &tip])?;
    let newest = line.lines().filter_map(|l| l.split_once('\t')).find(|(_, s)| branch.is_empty() || !bot(s)).map(|(h, _)| h.to_string()).ok_or_else(|| format!("{name}: no commit of its own in 200"))?;
    if !is_ancestor(dir, &newest, target) {
        return Err(format!("{name} is not in {target}"));
    }
    if own.is_some_and(|o| o.contains(&newest)) {
        return Err(format!("{name} has no commits of its own (never landed)"));
    }
    Ok(())
}

/// Whether the lane landed (both halves), and what still references it; Ok = removable.
fn removable(ctx: &Ctx, l: &LaneRec, own: &(HashSet<String>, HashSet<String>), cwds: &[(u32, PathBuf)], land: &[String]) -> Result<(), String> {
    let path = Path::new(&l.path);
    half_landed(&ctx.repo, &l.branch, path, "work", Some(&own.0))?;
    let nested = path.join("apps/commercial");
    if nested.join(".git").exists() {
        if l.cbranch.is_empty() {
            if wt::unique_commits(&nested) {
                return Err("apps/commercial has commits on no commercial ref".into());
            }
        } else if branch_tip(&ctx.commercial(), &l.cbranch).is_some() {
            // its own commits (if any) in main; the bot's merges of main into it do not count
            half_landed(&ctx.commercial(), &l.cbranch, &nested, "main", None).map_err(|e| format!("commercial: {e}"))?;
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
    if ctx.leases.view().iter().any(|x| x.req.lane == l.name && !matches!(x.state, crate::lease::sched::State::Ended(_))) {
        return Err("a lease of the lane is open".into());
    }
    if ctx.store.read(|d| d.jobs.iter().any(|j| j.lane == l.name && !j.done())) {
        return Err("a job of the lane is open".into());
    }
    let open = land.iter().filter_map(|x| makepad_strict_json::parse(x.as_bytes()).ok()).any(|v| {
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        (s("branch") == l.branch || (!l.cbranch.is_empty() && s("cbranch") == l.cbranch)) && s("state") != "landed"
    });
    if open {
        return Err("its hand-in is still in the land queue".into());
    }
    Ok(())
}

fn land_entries() -> Vec<String> {
    let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-lanes");
    crate::cli::ask(&c, None, "land", "status all").unwrap_or_default()
}

/// The automatic cleanup pass: removes the lanes that landed, after the grace period.
pub fn cleanup(ctx: &Ctx) {
    let lanes: Vec<LaneRec> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live" && !l.keep).cloned().collect());
    if lanes.is_empty() {
        return;
    }
    let own = (first_parents(&ctx.repo, "work"), first_parents(&ctx.commercial(), "main"));
    let cwds = wt::live_cwds();
    let land = land_entries();
    let now = wall_ms();
    for l in lanes {
        match removable(ctx, &l, &own, &cwds, &land) {
            Err(why) => {
                // a lane that landed and is still referenced says why once
                let landed = half_landed(&ctx.repo, &l.branch, Path::new(&l.path), "work", Some(&own.0)).is_ok();
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
                // quiet for the grace: since it landed and since its last file event
                let active = ctx.monitor.last_event_ms(Path::new(&l.path)).max(ctx.monitor.last_event_ms(&Path::new(&l.path).join("apps/commercial")));
                if now.saturating_sub(l.landed_ms.max(active)) < GRACE_MS {
                    continue;
                }
                let what = remove(ctx, &l, false);
                finish(ctx, &l.name, "removed", what);
            }
        }
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

/// Removes the worktree pair, the branches (`-d`, or `-D` for a drop that bundled them), origin's
/// lane branches, and the lane's dirs on every node.
fn remove(ctx: &Ctx, l: &LaneRec, drop: bool) -> Result<String, String> {
    let path = PathBuf::from(&l.path);
    let nested = path.join("apps/commercial");
    let mut done = Vec::new();
    let force: &[&str] = if drop { &["--force"] } else { &[] };
    if nested.join(".git").exists() {
        // removed from its own repository (the one its .git names), so neither keeps a dangling entry
        let common = wt::git(Some(&nested), None, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
        let mut a = vec!["worktree", "remove"];
        a.extend_from_slice(force);
        let n = nested.to_string_lossy().into_owned();
        a.push(&n);
        wt::git(None, Some(Path::new(common.trim())), &a)?;
        ctx.monitor.forget(&nested);
    }
    if path.exists() {
        let mut a = vec!["worktree", "remove"];
        a.extend_from_slice(force);
        let p = path.to_string_lossy().into_owned();
        a.push(&p);
        wt::git(Some(&ctx.repo), None, &a)?;
    }
    ctx.monitor.forget(&path);
    done.push(format!("removed {}", path.display()));
    // landed (checked) or bundled (a drop): the branch goes even with the merge bot's merges on it
    let del = "-D";
    for (dir, b) in [(ctx.repo.clone(), l.branch.clone()), (ctx.commercial(), l.cbranch.clone())] {
        if b.is_empty() || branch_tip(&dir, &b).is_none() {
            continue;
        }
        match wt::git(Some(&dir), None, &["branch", del, &b]) {
            Ok(_) => done.push(format!("branch {b}")),
            Err(e) => done.push(format!("branch {b} kept ({e})")),
        }
        if git(&dir, &["rev-parse", "-q", "--verify", &format!("refs/remotes/origin/{b}")]).is_ok() {
            match git(&dir, &["push", "-q", "origin", "--delete", &b]) {
                Ok(_) => done.push(format!("origin/{b}")),
                Err(e) => done.push(format!("origin/{b} kept ({e})")),
            }
        }
    }
    let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-lanes");
    let mut freed = 0.0;
    for n in ctx.leases.nodes().into_iter().filter(|n| n.up) {
        if let Ok(lines) = crate::cli::ask(&c, Some(&n.name), "disk", &format!("disk clean {}", l.name)) {
            freed += lines.first().and_then(|x| makepad_strict_json::parse(x.as_bytes()).ok()).and_then(|v| v.get("freed_mb").and_then(crate::lease::num)).unwrap_or(0.0);
        }
    }
    done.push(format!("node dirs ({freed:.0} MB)"));
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
    ctx.store.update(|d| d.lanes.push(l.clone()));
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
        let n: u32 = git(&repo, &["rev-list", "--count", &what, "--not", target])?.trim().parse().unwrap_or(0);
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
    let what = remove(ctx, &l, true)?;
    let note = if kept.is_empty() { format!("{what}; nothing unlanded to bundle") } else { format!("{what}; bundled {}", kept.join(", ")) };
    finish(ctx, name, "dropped", Ok(note.clone()));
    Ok(note)
}

/// `lane adopt`: registers the live worktrees under the lanes root; lists the landed ones.
fn adopt(ctx: &Ctx) -> Result<String, String> {
    let infos = wt::infos(&ctx.repo)?;
    let own = first_parents(&ctx.repo, "work");
    let known: HashSet<String> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live").map(|l| l.path.clone()).collect());
    let (mut registered, mut landed) = (Vec::new(), Vec::new());
    for i in infos {
        let p = i.entry.path.to_string_lossy().into_owned();
        if known.contains(&p) || !i.entry.path.is_dir() {
            continue;
        }
        let tip_landed = i.merged && !own.contains(&i.entry.head);
        let com_ok = !matches!(i.commercial, Some((true, _)) | Some((_, true)));
        if tip_landed && !i.dirty && com_ok {
            landed.push(format!("{{\"name\":{},\"path\":{},\"branch\":{},\"live\":{}}}", json_str(&i.lane), json_str(&p), json_str(i.entry.branch.as_deref().unwrap_or("")), i.live.len()));
            continue;
        }
        let nested = i.entry.path.join("apps/commercial");
        let cbranch = if nested.join(".git").exists() { crate::monitor::read_head(&nested).map(|h| h.1).unwrap_or_default() } else { String::new() };
        let l = LaneRec { name: i.lane.clone(), path: p.clone(), branch: i.entry.branch.clone().unwrap_or_default(), cbranch, created_ms: wall_ms(), adopted: true, state: "live".into(), ..Default::default() };
        registered.push(json_str(&l.name));
        ctx.store.update(|d| d.lanes.push(l));
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
            ctx.store.update(|d| d.lanes.push(l.clone()));
            l
        }
    };
    let own = (first_parents(&ctx.repo, "work"), first_parents(&ctx.commercial(), "main"));
    removable(ctx, &l, &own, &wt::live_cwds(), &land_entries())?;
    let what = remove(ctx, &l, false);
    let out = what.clone();
    finish(ctx, name, "removed", what);
    out
}

/// One row of `devhub lanes`.
fn row(ctx: &Ctx, l: &LaneRec, merge: &[String], land: &[String]) -> String {
    let path = Path::new(&l.path);
    let (tip, dirty) = match ctx.monitor.dirty(path) {
        Ok(d) => {
            let mut n = d.changed.len() + d.deleted.len();
            if path.join("apps/commercial/.git").exists() {
                n += ctx.monitor.dirty(&path.join("apps/commercial")).map_or(0, |c| c.changed.len() + c.deleted.len());
            }
            (d.head, n.to_string())
        }
        Err(e) => (String::new(), format!("? ({e})")),
    };
    let find = |lines: &[String], k: &str, b: &str| -> Option<makepad_strict_json::Value> { lines.iter().rev().filter_map(|x| makepad_strict_json::parse(x.as_bytes()).ok()).find(|v| v.get(k).and_then(|x| x.as_str()) == Some(b)) };
    let word = |v: &Option<makepad_strict_json::Value>, k: &str| v.as_ref().and_then(|v| v.get(k)).and_then(|x| x.as_str()).unwrap_or("-").to_string();
    let m = find(merge, "branch", &l.branch);
    let ld = find(land, "branch", &l.branch);
    let job = ctx.store.read(|d| d.jobs.iter().rev().find(|j| j.lane == l.name).map(|j| format!("{} {}", j.id, j.state))).unwrap_or_else(|| "-".into());
    format!(
        "{{\"name\":{},\"path\":{},\"branch\":{},\"cbranch\":{},\"tip\":{},\"dirty\":{},\"merge\":{},\"land\":{},\"job\":{},\"keep\":{},\"waited\":{},\"note\":{}}}",
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
        Ok(n) => p.unique = n.trim().parse().unwrap_or(0),
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

/// `timeline <lane>`: where the lane's time since its start went. A moment counts once, for what
/// ranks highest: gating (its landings' gates), booting (its shows' launches), running (its leases
/// and jobs), queueing (waiting for any of them), else thinking (nothing of devhub's in flight).
fn timeline(ctx: &Ctx, name: &str) -> Result<String, String> {
    const KINDS: [&str; 5] = ["gating", "booting", "running", "queueing", "thinking"];
    let l = ctx.store.read(|d| d.lanes.iter().rev().find(|l| l.name == name).cloned()).ok_or_else(|| format!("no lane {name}"))?;
    let now = wall_ms();
    let (t0, t1) = (l.created_ms, if l.ended_ms > 0 { l.ended_ms } else { now });
    // the lane's landings
    let entries: Vec<crate::land::Entry> = ctx.store.read(|d| d.land.iter().filter_map(|v| crate::land::Entry::parse(&v.to_json())).filter(|e| (!l.branch.is_empty() && e.branch == l.branch) || (!l.cbranch.is_empty() && e.cbranch == l.cbranch)).collect());
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

/// `lane new {"name","from","commercial"}`, `lane drop|remove|keep|unkeep <name>`, `lane adopt`,
/// `lane list`.
pub fn serve(ctx: &Ctx, line: &str) -> Result<String, String> {
    let mut w = line.splitn(3, ' ');
    let _ = w.next();
    let verb = w.next().unwrap_or("list");
    let rest = w.next().unwrap_or("").trim();
    match verb {
        "new" => {
            let v = makepad_strict_json::parse(rest.as_bytes()).map_err(|_| "lane new: bad request")?;
            let name = v.get("name").and_then(|x| x.as_str()).unwrap_or("");
            let from = v.get("from").and_then(|x| x.as_str()).filter(|f| !f.is_empty()).unwrap_or("work");
            let commercial = v.get("commercial").and_then(|x| x.as_bool()).unwrap_or(true);
            let l = create(ctx, name, from, commercial)?;
            event(ctx, Sev::Info, format!("lane new {name}: {} on {} from {from}{}", l.path, l.branch, if commercial { " (+ apps/commercial on the same branch from main)" } else { "" }), Some(&l));
            Ok(format!("{{\"name\":{},\"path\":{},\"branch\":{},\"cbranch\":{}}}", json_str(name), json_str(&l.path), json_str(&l.branch), json_str(&l.cbranch)))
        }
        "drop" => drop_lane(ctx, rest).map(|n| format!("{{\"dropped\":{},\"note\":{}}}", json_str(rest), json_str(&n))),
        "remove" => remove_now(ctx, rest).map(|n| format!("{{\"removed\":{},\"note\":{}}}", json_str(rest), json_str(&n))),
        "keep" | "unkeep" => {
            set(ctx, rest, |l| l.keep = verb == "keep").ok_or_else(|| format!("no live lane {rest}"))?;
            Ok(format!("{{\"name\":{},\"keep\":{}}}", json_str(rest), verb == "keep"))
        }
        "timeline" => timeline(ctx, rest).map(|t| format!("{{\"text\":{}}}", json_str(&t))),
        "live" => {
            let (name, on) = match rest.split_once(' ') {
                Some((n, "on")) => (n, true),
                Some((n, "off")) => (n, false),
                _ => return Err("lane live <name> on|off".into()),
            };
            let l = set(ctx, name, |l| l.waited = on).ok_or_else(|| format!("no live lane {name}"))?;
            event(ctx, Sev::Info, format!("lane {name}: {}", if on { "live: the user waits for it; its landings take the fast path (merge + check)" } else { "not live: its landings gate before landing" }), Some(&l));
            Ok(format!("{{\"name\":{},\"live\":{on}}}", json_str(name)))
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
        "list" => {
            let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-lanes");
            let merge = crate::cli::ask(&c, None, "lanes", "lanes list").unwrap_or_default();
            let land = land_entries();
            let lanes: Vec<LaneRec> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live").cloned().collect());
            let rows: Vec<String> = lanes.iter().map(|l| row(ctx, l, &merge, &land)).collect();
            Ok(format!("[{}]", rows.join(",")))
        }
        _ => Err(format!("lane: unknown word `{verb}` (new drop remove keep unkeep live timeline adopt sweep list)")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::master::LeaseMaster;
    use crate::state::Store;

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
        let ctx = Ctx { store: Store::open(&home, "mac", Arc::new(|_| {})), leases, machine: "mac".into(), monitor: crate::monitor::Monitor::start(&[base.join("wt.noindex")]), repo: mk.clone(), home: home.clone() };
        let r = serve(&ctx, "lane new {\"name\":\"x\",\"from\":\"work\",\"commercial\":true}").unwrap();
        let lane = base.join("wt.noindex/x");
        assert!(r.contains("rc-x") && lane.join("a.txt").exists() && lane.join("apps/commercial/a.txt").exists(), "{r}");
        assert_eq!(g(&lane.join("apps/commercial"), &["branch", "--show-current"]), "rc-x");
        assert!(serve(&ctx, "lane new {\"name\":\"x\"}").unwrap_err().contains("exists"));
        // a fresh lane is in work but never landed: the cleanup keeps it
        cleanup(&ctx);
        assert_eq!(ctx.store.read(|d| (d.lanes[0].state.clone(), d.lanes[0].landed_ms)), ("live".to_string(), 0));
        // a commit, landed by a merge into work (and commercial into main)
        std::fs::write(lane.join("b.txt"), "b").unwrap();
        g(&lane, &["add", "."]);
        g(&lane, &["commit", "-q", "-m", "lane work"]);
        std::fs::write(lane.join("apps/commercial/c.txt"), "c").unwrap();
        g(&lane.join("apps/commercial"), &["add", "."]);
        g(&lane.join("apps/commercial"), &["commit", "-q", "-m", "lane work"]);
        g(&mk, &["merge", "-q", "--no-ff", "-m", "land x", "rc-x"]);
        g(&mk.join("apps/commercial"), &["merge", "-q", "--no-ff", "-m", "land x", "rc-x"]);
        // the merge bot merged work in again after the landing: still landed
        g(&lane, &["commit", "-q", "--allow-empty", "-m", "Merge work (0123456789) into rc-x"]);
        // an uncommitted file keeps it
        std::fs::write(lane.join("wip.txt"), "w").unwrap();
        cleanup(&ctx);
        assert!(ctx.store.read(|d| d.lanes[0].note.clone()).contains("uncommitted"));
        std::fs::remove_file(lane.join("wip.txt")).unwrap();
        // clean: first seen landed, then removed once the grace passed with no activity
        cleanup(&ctx);
        assert!(ctx.store.read(|d| d.lanes[0].landed_ms) > 0);
        // within the grace it stays; `lane remove` (adopt's list, once agreed) skips the grace
        cleanup(&ctx);
        assert_eq!(ctx.store.read(|d| d.lanes[0].state.clone()), "live");
        assert!(serve(&ctx, "lane remove x").is_ok());
        assert_eq!(ctx.store.read(|d| d.lanes[0].state.clone()), "removed", "{:?}", ctx.store.read(|d| d.lanes[0].note.clone()));
        assert!(!lane.exists());
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

    #[test]
    fn lane_names() {
        assert!(super::valid_name("devhub-orch_2"));
        assert!(!super::valid_name("-x") && !super::valid_name("a/b") && !super::valid_name("") && !super::valid_name("a b"));
    }
}
