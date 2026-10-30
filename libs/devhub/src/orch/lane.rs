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
use std::collections::HashSet;
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

/// One half landed: its branch tip is in `target` and is not one of `target`'s own commits.
fn half_landed(dir: &Path, branch: &str, target: &str, own: &HashSet<String>) -> Result<(), String> {
    let tip = branch_tip(dir, branch).ok_or_else(|| format!("no branch {branch}"))?;
    if !is_ancestor(dir, &tip, target) {
        return Err(format!("{branch} is not in {target}"));
    }
    if own.contains(&tip) {
        return Err(format!("{branch} has no commits of its own (never landed)"));
    }
    Ok(())
}

/// Whether the lane landed (both halves), and what still references it; Ok = removable.
fn removable(ctx: &Ctx, l: &LaneRec, own: &(HashSet<String>, HashSet<String>), cwds: &[(u32, PathBuf)], land: &[String]) -> Result<(), String> {
    let path = Path::new(&l.path);
    half_landed(&ctx.repo, &l.branch, "work", &own.0)?;
    let nested = path.join("apps/commercial");
    if nested.join(".git").exists() {
        if l.cbranch.is_empty() {
            if wt::unique_commits(&nested) {
                return Err("apps/commercial has commits on no commercial ref".into());
            }
        } else if branch_tip(&ctx.commercial(), &l.cbranch).is_some_and(|t| !is_ancestor(&ctx.commercial(), &t, "main")) {
            return Err(format!("commercial {} is not in main", l.cbranch));
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
                let landed = half_landed(&ctx.repo, &l.branch, "work", &own.0).is_ok();
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
        let mut a = vec!["worktree", "remove"];
        a.extend_from_slice(force);
        let n = nested.to_string_lossy().into_owned();
        a.push(&n);
        wt::git(Some(&ctx.commercial()), None, &a)?;
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
    let del = if drop { "-D" } else { "-d" };
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
        "{{\"name\":{},\"path\":{},\"branch\":{},\"cbranch\":{},\"tip\":{},\"dirty\":{},\"merge\":{},\"land\":{},\"job\":{},\"keep\":{},\"note\":{}}}",
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
        json_str(&l.note)
    )
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
        "adopt" => adopt(ctx),
        "list" => {
            let c = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-lanes");
            let merge = crate::cli::ask(&c, None, "lanes", "lanes list").unwrap_or_default();
            let land = land_entries();
            let lanes: Vec<LaneRec> = ctx.store.read(|d| d.lanes.iter().filter(|l| l.state == "live").cloned().collect());
            let rows: Vec<String> = lanes.iter().map(|l| row(ctx, l, &merge, &land)).collect();
            Ok(format!("[{}]", rows.join(",")))
        }
        _ => Err(format!("lane: unknown word `{verb}` (new drop remove keep unkeep adopt list)")),
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
