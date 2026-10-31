//! `devhub job`: async jobs, so no box sits idle while an agent thinks. A job is one
//! self-contained command run (build, check, test, present) at pinned commits. `submit` returns
//! its id at once (a batch queues a whole plan, ordered by `--after`); the master asks a lease for
//! it the moment its dependencies ended ok, the scheduler places it on the first free box that fits
//! (a box whose tree already holds the commits first), the command runs, and the lease ends with
//! it. Its result (exit, duration, error lines, log tail) is state; every change is a `job` event
//! that `job wait` and `devhub watch job` return on.
//!
//! A program that parks on a panic under the JIT turns its job interactive: the job becomes
//! `paused` with the panic (id, message, thread, location), the box stays held with the process
//! alive, and the agent acts through the job (`job inspect`, `job resume`, `job release`). Ten
//! minutes with no agent action release the box; the result is the panic (`panicked`).

use super::leg::{Held, Spec};
use super::{age, Ctx};
use crate::cli::ask;
use crate::lease::Kind;
use crate::proto::{wall_ms, Record, Sev};
use crate::state::{Job, JobPanic};
use makepad_remote::json_str;
use makepad_remote::link::{Client, Target};
use std::sync::Arc;
use std::time::Duration;

/// A paused job with no agent action for this long releases its box.
pub const PAUSE_IDLE_MS: u64 = 10 * 60 * 1000;
const TICK: Duration = Duration::from_secs(2);
const ERRORS: usize = 12;
const TAIL: usize = 15;

fn ctl() -> Client {
    Client::new(Target::Unix(crate::proto::control_socket()), "devhub-job")
}

fn event(ctx: &Ctx, sev: Sev, line: String, j: Option<&Job>) {
    ctx.store.event("job", sev, line, j.map(|j| crate::state::job_value(j).to_json()));
}

pub fn start(ctx: Arc<Ctx>) {
    // what a previous master ran goes on: the run lives on its node, and the lease came back with
    // the master. A job with no run yet (its lease going or its snapshot shipping) or whose lease
    // ended meanwhile is placed again; one still waiting for a box asks again.
    let held = |j: &Job| super::leg::lease_held(&ctx, &j.lease);
    let (back, moved, stray): (Vec<Job>, Vec<Job>, Vec<String>) = ctx.store.update(|d| {
        let (mut back, mut moved, mut stray) = (Vec::new(), Vec::new(), Vec::new());
        for j in d.jobs.iter_mut() {
            match j.state.as_str() {
                "leasing" => {
                    j.state = "queued".into();
                    j.attempts = j.attempts.saturating_sub(1);
                }
                "running" | "paused" if j.run_n > 0 && held(j) => back.push(j.clone()),
                "running" | "paused" => {
                    if j.run_n == 0 {
                        j.attempts = j.attempts.saturating_sub(1);
                    }
                    if held(j) {
                        stray.push(j.lease.clone());
                    }
                    if j.attempts < ATTEMPTS {
                        j.state = "queued".into();
                        j.note = "its lease did not outlive the master's restart; placed again".into();
                    } else {
                        j.state = "lost".into();
                        j.ended_ms = wall_ms();
                        j.note = "its lease did not outlive the master's restart (no placements left)".into();
                    }
                    j.lease.clear();
                    j.node.clear();
                    j.run_n = 0;
                    moved.push(j.clone());
                }
                _ => {}
            }
        }
        (back, moved, stray)
    });
    for l in stray {
        end_lease(&l);
    }
    for j in moved {
        event(&ctx, Sev::Warn, format!("job {}: {}", j.id, j.note), Some(&j));
    }
    for j in back {
        event(&ctx, Sev::Info, format!("job {}: the master restarted; its run on {} goes on", j.id, j.node), Some(&j));
        let c = ctx.clone();
        let _ = std::thread::Builder::new().name("devhub-job".into()).spawn(move || reattach(&c, &j));
    }
    prune_logs(&ctx);
    let _ = std::thread::Builder::new().name("devhub-jobs".into()).spawn(move || loop {
        dispatch(&ctx);
        idle(&ctx);
        ctx.store.wait(TICK);
    });
}

/// Removes the logs and snapshots of jobs the state no longer keeps.
fn prune_logs(ctx: &Ctx) {
    let snaps: Vec<String> = ctx.store.read(|d| d.jobs.iter().map(|j| j.snap.clone()).filter(|s| !s.is_empty()).collect());
    if let Ok(rd) = std::fs::read_dir(ctx.logs("jobs").join("snap")) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with('S') && name[1..].bytes().all(|b| b.is_ascii_digit()) && !snaps.contains(&name) {
                let _ = std::fs::remove_dir_all(snap_dir(ctx, &name));
            }
        }
    }
    let dir = ctx.logs("jobs");
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let keep: Vec<String> = ctx.store.read(|d| d.jobs.iter().map(|j| format!("{}.log", j.id)).collect());
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('J') && name.ends_with(".log") && !keep.contains(&name) {
            let _ = std::fs::remove_file(dir.join(&name));
        }
    }
}

/// Starts every queued job whose dependencies ended ok; skips those whose did not.
fn dispatch(ctx: &Arc<Ctx>) {
    let (go, skip): (Vec<String>, Vec<(String, String)>) = ctx.store.read(|d| {
        let state = |id: &str| d.jobs.iter().find(|j| j.id == id).map(|j| j.state.clone());
        let (mut go, mut skip) = (Vec::new(), Vec::new());
        for j in d.jobs.iter().filter(|j| j.state == "queued") {
            let deps: Vec<(String, Option<String>)> = j.after.iter().map(|a| (a.clone(), state(a))).collect();
            if let Some((a, s)) = deps.iter().find(|(_, s)| s.as_deref().is_none_or(|s| matches!(s, "failed" | "panicked" | "cancelled" | "skipped" | "lost"))) {
                skip.push((j.id.clone(), format!("{a} {}", s.as_deref().unwrap_or("is unknown"))));
            } else if deps.iter().all(|(_, s)| s.as_deref() == Some("ok")) {
                go.push(j.id.clone());
            }
        }
        (go, skip)
    });
    for (id, why) in skip {
        let j = ctx.store.update(|d| {
            let j = d.jobs.iter_mut().find(|j| j.id == id)?;
            j.state = "skipped".into();
            j.ended_ms = wall_ms();
            j.note = why.clone();
            Some(j.clone())
        });
        if let Some(j) = j {
            event(ctx, Sev::Warn, format!("job {id} skipped: {why}"), Some(&j));
        }
    }
    for id in go {
        let ok = ctx.store.update(|d| match d.jobs.iter_mut().find(|j| j.id == id && j.state == "queued") {
            Some(j) => {
                j.state = "leasing".into();
                true
            }
            None => false,
        });
        if ok {
            let c = ctx.clone();
            let _ = std::thread::Builder::new().name("devhub-job".into()).spawn(move || run(&c, &id));
        }
    }
}

fn get(ctx: &Ctx, id: &str) -> Option<Job> {
    ctx.store.read(|d| d.jobs.iter().find(|j| j.id == id).cloned())
}

fn set(ctx: &Ctx, id: &str, f: impl FnOnce(&mut Job)) -> Option<Job> {
    ctx.store.update(|d| {
        let j = d.jobs.iter_mut().find(|j| j.id == id)?;
        f(j);
        Some(j.clone())
    })
}

/// The first error lines and the last lines of a job's output.
pub fn digest(text: &str) -> (Vec<String>, Vec<String>) {
    let is_err = |l: &str| {
        let t = l.trim_start();
        t.starts_with("error") || t.contains("panicked at") || t.starts_with("FAILED") || t.contains(" FAILED") || t.contains("-FAIL") || t.starts_with("test result: FAILED") || t.contains(": error:")
    };
    let errors = text.lines().filter(|l| is_err(l)).take(ERRORS).map(|l| crate::proto::clean(l, 240)).collect();
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(TAIL)..].iter().map(|l| crate::proto::clean(l, 240)).collect();
    (errors, tail)
}

/// The output file of a job: the first bytes stamp `first_out_ms`.
struct Out<'a> {
    file: std::fs::File,
    first: Option<Box<dyn FnMut() + 'a>>,
}

impl std::io::Write for Out<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if !buf.is_empty() {
            if let Some(mut f) = self.first.take() {
                f();
            }
        }
        self.file.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

/// A snapshot's dir: `files/` (its files at their paths) and `manifest.json`.
pub fn snap_dir(ctx: &Ctx, snap: &str) -> std::path::PathBuf {
    ctx.logs("jobs").join("snap").join(snap)
}

/// The paths a snapshot carries on top of its base commits: (changed, deleted).
fn snap_paths(ctx: &Ctx, snap: &str) -> Result<(Vec<String>, Vec<String>), String> {
    let text = std::fs::read_to_string(snap_dir(ctx, snap).join("manifest.json")).map_err(|e| format!("snapshot {snap}: {e}"))?;
    let v = makepad_strict_json::parse(text.as_bytes()).map_err(|_| format!("snapshot {snap}: bad manifest"))?;
    // (an older snapshot may name what never ships)
    let ok = |p: &String| !crate::lease::ship::never_ships(p);
    let changed = v.get("changed").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.get("path").and_then(|p| p.as_str()).map(str::to_string)).filter(ok).collect();
    let deleted = v.get("deleted").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str().map(str::to_string)).filter(ok).collect();
    Ok((changed, deleted))
}

/// Ships the snapshot into the lease's tree (already at the snapshot's base commits): only the
/// files the box lacks as they are go; then `ship-done` names them and the deletions.
pub(super) fn ship_snap(ctx: &Ctx, held: &Held, snap: &str) -> Result<usize, String> {
    let (changed, deleted) = snap_paths(ctx, snap)?;
    let files = snap_dir(ctx, snap).join("files");
    let mut entries = Vec::new();
    for p in &changed {
        crate::lease::ship::entries(&files, p, &mut entries)?;
    }
    let sent = if entries.is_empty() { 0 } else { crate::lease::ship::send(held.client(), &held.node, &held.id, &files, &entries)?.sent };
    let list = |v: &[String]| v.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(",");
    let body = format!("{{\"shipped\":[{}],\"deleted\":[{}]}}", list(&changed), list(&deleted));
    let r = ask(held.client(), Some(&held.node), "ship-done", &format!("ship-done {} {}", held.id, makepad_network::digest::base64_encode(body.as_bytes())))?;
    if let Some(e) = r.first().filter(|l| l.starts_with("{\"err\":")) {
        return Err(format!("{}: {e}", held.node));
    }
    Ok(sent)
}

/// How often a job is placed again when its box is lost under it.
pub const ATTEMPTS: u32 = 3;

/// One job: lease, ship its snapshot, run, release, result.
fn run(ctx: &Arc<Ctx>, id: &str) {
    let Some(j) = set(ctx, id, |j| j.attempts += 1) else { return };
    let dir = ctx.logs("jobs");
    let _ = std::fs::create_dir_all(&dir);
    let log = dir.join(format!("{id}.log"));
    let kind = Kind::parse(&j.kind).unwrap_or(Kind::Test);
    let spec = Spec { only: j.only, lane: j.lane.clone(), agent: j.agent.clone(), kind, needs: j.needs.clone(), prio: j.prio, rev: j.rev.clone(), repo_dir: j.repo_dir.clone(), node: (!j.node_pref.is_empty()).then(|| j.node_pref.clone()) };
    let cancelled = || get(ctx, id).is_none_or(|j| j.state == "cancelled");
    let held = match Held::acquire(&spec, &mut |_| {}, &cancelled) {
        Ok(h) => h,
        // the master's socket or a box's link went away: no verdict on the job
        Err(e) if crate::lease::master::unreachable(&e) && again(ctx, id, &format!("no lease ({})", crate::proto::clean(&e, 160))) => return,
        Err(e) => return finish(ctx, id, Err(e), &log),
    };
    let j = set(ctx, id, |j| {
        if j.state == "leasing" {
            j.state = "running".into();
            j.lease = held.id.clone();
            j.node = held.node.clone();
            j.granted_ms = wall_ms();
            j.started_ms = j.granted_ms;
        }
    });
    // a soft pin that fell to a twin says so: the job's artifacts and timing are that box's
    if !spec.only && spec.node.as_deref().is_some_and(|n| n != held.node) {
        event(ctx, Sev::Warn, format!("job {id}: --node {} was held, so it runs on {} (`--only {}` waits for it)", spec.node.as_deref().unwrap_or(""), held.node, spec.node.as_deref().unwrap_or("")), None);
    }
    let Some(j) = j.filter(|j| j.state == "running") else {
        // cancelled while the grant came
        held.release(None);
        return finish(ctx, id, Err("cancelled".into()), &log);
    };
    if !j.snap.is_empty() {
        if let Err(e) = ship_snap(ctx, &held, &j.snap) {
            held.release(None);
            if crate::lease::master::unreachable(&e) && again(ctx, id, &format!("shipping to {} failed ({})", j.node, crate::proto::clean(&e, 160))) {
                return;
            }
            return finish(ctx, id, Err(format!("shipping {}: {e}", j.snap)), &log);
        }
    }
    let j = set(ctx, id, |j| j.shipped_ms = wall_ms()).unwrap_or(j);
    event(ctx, Sev::Info, format!("job {id} running on {} ({}{}): {}", held.node, held.id, if j.snap.is_empty() { String::new() } else { format!(", {} {} file(s)", j.snap, j.snap_files) }, crate::proto::clean(&spec_cmd(&j), 160)), Some(&j));
    // (only now: an earlier placement's output stays until this one has its own)
    let file = match std::fs::File::create(&log) {
        Ok(f) => f,
        Err(e) => {
            held.release(None);
            return finish(ctx, id, Err(format!("{}: {e}", log.display())), &log);
        }
    };
    let mut out = Out { file, first: Some(Box::new(|| {
        set(ctx, id, |j| j.first_out_ms = wall_ms());
    })) };
    let r = held.run_noted(&j.cmd, &mut out, &mut |n| {
        set(ctx, id, |j| j.run_n = n);
    });
    drop(out);
    ended(ctx, id, held, r, &log);
}

/// A previous master's job on the lease this master took back: its run followed from where the
/// log stands, to the end.
fn reattach(ctx: &Arc<Ctx>, j: &Job) {
    let log = ctx.logs("jobs").join(format!("{}.log", j.id));
    let off = std::fs::metadata(&log).map_or(0, |m| m.len());
    let held = Held::attach(&j.lease, &j.node);
    let file = match std::fs::OpenOptions::new().append(true).create(true).open(&log) {
        Ok(f) => f,
        Err(e) => {
            held.release(None);
            return finish(ctx, &j.id, Err(format!("{}: {e}", log.display())), &log);
        }
    };
    let id = j.id.as_str();
    let first: Option<Box<dyn FnMut() + '_>> = (j.first_out_ms == 0).then(|| {
        Box::new(move || {
            set(ctx, id, |j| j.first_out_ms = wall_ms());
        }) as Box<dyn FnMut() + '_>
    });
    let mut out = Out { file, first };
    let r = held.follow(j.run_n, off, &mut out);
    drop(out);
    ended(ctx, id, held, r, &log);
}

/// A job's run is over: its lease goes, and it has its result, or (the box lost under it, no exit)
/// it is placed again from its snapshot.
fn ended(ctx: &Ctx, id: &str, held: Held, r: Result<i32, String>, log: &std::path::Path) {
    held.release(r.as_ref().ok().copied());
    if let Err(e) = &r {
        let node = get(ctx, id).map_or(String::new(), |j| j.node);
        if again(ctx, id, &format!("box {node} lost ({})", crate::proto::clean(e, 160))) {
            return;
        }
    }
    finish(ctx, id, r, log);
}

/// Back to the queue, with what happened as its note, if it has placements left; whether it went.
fn again(ctx: &Ctx, id: &str, why: &str) -> bool {
    let j = set(ctx, id, |j| {
        if matches!(j.state.as_str(), "leasing" | "running") && j.attempts < ATTEMPTS {
            j.state = "queued".into();
            j.note = format!("{why}; placed again");
            j.lease.clear();
            j.node.clear();
            j.run_n = 0;
        }
    });
    match j.filter(|j| j.state == "queued") {
        Some(j) => {
            event(ctx, Sev::Warn, format!("job {id}: {}", j.note), Some(&j));
            true
        }
        None => false,
    }
}

fn spec_cmd(j: &Job) -> String {
    j.cmd.join(" ")
}

/// The job's result from its run (cancelled and released-while-paused jobs keep that outcome).
fn finish(ctx: &Ctx, id: &str, r: Result<i32, String>, log: &std::path::Path) {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let (errors, tail) = digest(&text);
    let j = set(ctx, id, |j| {
        j.ended_ms = wall_ms();
        j.errors = errors;
        j.tail = tail;
        j.log = log.to_string_lossy().into_owned();
        j.state = match (j.state.as_str(), &r) {
            ("cancelled", _) => "cancelled".into(),
            ("paused", _) => "panicked".into(),
            (_, Ok(0)) => "ok".into(),
            (_, Ok(_)) => "failed".into(),
            (_, Err(_)) => "failed".into(),
        };
        if let Ok(code) = &r {
            j.exit = Some(*code);
        }
        if let Err(e) = &r {
            // what ended it, also over what an earlier placement noted
            j.note = crate::proto::clean(e, 240);
        }
    });
    let Some(j) = j else { return };
    let secs = j.ended_ms.saturating_sub(if j.started_ms > 0 { j.started_ms } else { j.submitted_ms }) / 1000;
    let what = match j.state.as_str() {
        "ok" => format!("job {id} ok in {secs} s on {}", j.node),
        "panicked" => format!("job {id} panicked: {}", j.panic.as_ref().map_or(String::new(), |p| format!("{} at {}", p.message, p.location))),
        "cancelled" => format!("job {id} cancelled"),
        _ => format!("job {id} failed{} in {secs} s on {}: {}", j.exit.map_or(String::new(), |e| format!(" exit={e}")), j.node, j.errors.first().cloned().unwrap_or_else(|| j.note.clone())),
    };
    let sev = match j.state.as_str() {
        "ok" | "cancelled" => Sev::Info,
        _ => Sev::Warn,
    };
    event(ctx, sev, what, Some(&j));
}

/// Releases the box of a job paused too long with no agent action.
fn idle(ctx: &Ctx) {
    let now = wall_ms();
    let stale: Vec<Job> = ctx.store.read(|d| d.jobs.iter().filter(|j| j.state == "paused" && now.saturating_sub(j.touched_ms) >= PAUSE_IDLE_MS).cloned().collect());
    for j in stale {
        set(ctx, &j.id, |j| j.note = "released after 10 min paused with no agent action".into());
        event(ctx, Sev::Warn, format!("job {} paused 10 min with no agent action: its box is released", j.id), Some(&j));
        end_lease(&j.lease);
    }
}

fn end_lease(lease: &str) {
    if !lease.is_empty() {
        let _ = ask(&ctl(), None, "release", lease);
    }
}

/// A record on the master's stream: a panic parked on a running job's box pauses that job.
pub fn on_record(ctx: &Arc<Ctx>, r: &Record) {
    if r.kind != "panic" || r.pid == 0 {
        return;
    }
    let Some(j) = ctx.store.read(|d| d.jobs.iter().find(|j| j.state == "running" && j.node == r.machine).cloned()) else { return };
    let (ctx, r) = (ctx.clone(), r.clone());
    // the node says whether the panicking process is one of the job's (never on the record path)
    let _ = std::thread::Builder::new().name("devhub-job-panic".into()).spawn(move || {
        let owns = ask(&ctl(), Some(&j.node), "lease-owns", &format!("lease-owns {} {}", j.lease, r.pid)).ok().and_then(|l| l.first().cloned()).is_some_and(|l| l.contains("\"owns\":true"));
        if !owns {
            return;
        }
        let d = makepad_strict_json::parse(r.detail.as_bytes()).ok();
        let f = |k: &str| d.as_ref().and_then(|d| d.get(k)).and_then(|x| x.as_str()).map(|s| crate::proto::clean(s, 300)).unwrap_or_default();
        let p = JobPanic { id: f("id"), pid: r.pid, message: f("message"), thread: f("thread"), location: f("location"), at_ms: wall_ms() };
        let Some(j) = set(&ctx, &j.id, |j| {
            if j.state == "running" {
                j.state = "paused".into();
                j.panic = Some(p.clone());
                j.touched_ms = wall_ms();
            }
        }) else { return };
        if j.state != "paused" {
            return;
        }
        let pid = if p.id.is_empty() { "P?".to_string() } else { p.id.clone() };
        event(
            &ctx,
            Sev::Err,
            format!("job {} PAUSED on a panic: {} at {} [{}] ({pid}, pid {}): job inspect {} stacks | job resume {} {pid} retry|restart|unwind | job release {}", j.id, p.message, p.location, p.thread, p.pid, j.id, j.id, j.id),
            Some(&j),
        );
    });
}

/// A job as the CLI sees it: a running one with its output so far (errors, tail) from its log.
fn live(ctx: &Ctx, j: &Job) -> String {
    let mut j = j.clone();
    if !j.done() {
        let log = ctx.logs("jobs").join(format!("{}.log", j.id));
        if let Ok(text) = std::fs::read_to_string(&log) {
            (j.errors, j.tail) = digest(&text);
            j.log = log.to_string_lossy().into_owned();
        }
    }
    crate::state::job_value(&j).to_json()
}

/// `job …` requests from the CLI.
pub fn serve(ctx: &Ctx, line: &str) -> Result<String, String> {
    let mut w = line.splitn(3, ' ');
    let _ = w.next();
    let verb = w.next().unwrap_or("list");
    let rest = w.next().unwrap_or("").trim();
    let ids = || -> Vec<String> { rest.split([' ', ',']).filter(|s| !s.is_empty()).map(str::to_string).collect() };
    match verb {
        "submit" => submit(ctx, rest),
        "list" => {
            let all = rest == "all";
            let list: Vec<String> = ctx.store.read(|d| d.jobs.iter().filter(|j| all || !j.done()).cloned().collect::<Vec<_>>()).iter().map(|j| live(ctx, j)).collect();
            Ok(format!("[{}]", list.join(",")))
        }
        "get" => {
            let want = ids();
            let found: Vec<(String, Option<Job>)> = ctx.store.read(|d| want.iter().map(|id| (id.clone(), d.jobs.iter().find(|j| &j.id == id).cloned())).collect());
            let list: Vec<String> = found.iter().map(|(id, j)| j.as_ref().map_or_else(|| format!("{{\"id\":{},\"state\":\"unknown\"}}", json_str(id)), |j| live(ctx, j))).collect();
            Ok(format!("[{}]", list.join(",")))
        }
        "cancel" | "release" => {
            let mut out = Vec::new();
            for id in ids() {
                let j = get(ctx, &id).ok_or_else(|| format!("no job {id}"))?;
                if j.done() {
                    out.push(format!("{{\"id\":{},\"state\":{}}}", json_str(&id), json_str(&j.state)));
                    continue;
                }
                // release: the paused result stands (`panicked`); cancel: the job is cancelled
                let paused = j.state == "paused";
                let j = set(ctx, &id, |j| {
                    if verb == "cancel" || !paused {
                        if j.state == "queued" {
                            j.ended_ms = wall_ms();
                        }
                        j.state = "cancelled".into();
                    } else {
                        j.note = "released by the agent".into();
                    }
                })
                .unwrap_or(j);
                end_lease(&j.lease);
                if j.state == "cancelled" {
                    event(ctx, Sev::Info, format!("job {id} cancelled"), Some(&j));
                }
                out.push(format!("{{\"id\":{},\"state\":{}}}", json_str(&id), json_str(&j.state)));
            }
            Ok(format!("[{}]", out.join(",")))
        }
        "rerun" => rerun(ctx, rest),
        "inspect" => {
            let (id, cmd) = rest.split_once(' ').ok_or("job inspect <id> <inspect command>")?;
            let j = get(ctx, id).ok_or_else(|| format!("no job {id}"))?;
            let p = j.panic.as_ref().filter(|_| j.state == "paused").ok_or_else(|| format!("job {id} is {}, not paused on a panic", j.state))?;
            set(ctx, id, |j| j.touched_ms = wall_ms());
            let mut lines = Vec::new();
            makepad_remote::hub::ask(&ctl(), &format!("{}:{}", j.node, p.pid), "cmd", "", cmd, &mut |l| {
                lines.push(json_str(l));
                true
            })?;
            Ok(format!("{{\"lines\":[{}]}}", lines.join(",")))
        }
        "resume" => {
            let mut w = rest.split_whitespace();
            let (Some(id), Some(pid), Some(how)) = (w.next(), w.next(), w.next()) else { return Err("job resume <id> <P1> retry|restart|unwind".into()) };
            let word = match how {
                "retry" | "restart" => how,
                "unwind" => "panic",
                _ => return Err("job resume <id> <P1> retry|restart|unwind".into()),
            };
            let j = get(ctx, id).ok_or_else(|| format!("no job {id}"))?;
            let p = j.panic.clone().filter(|_| j.state == "paused").ok_or_else(|| format!("job {id} is {}, not paused on a panic", j.state))?;
            let mut lines = Vec::new();
            makepad_remote::hub::ask(&ctl(), &format!("{}:{}", j.node, p.pid), "cmd", "", &format!("resume {pid} {word}"), &mut |l| {
                lines.push(l.to_string());
                true
            })?;
            if let Some(e) = lines.iter().find(|l| l.starts_with("{\"err\":")) {
                set(ctx, id, |j| j.touched_ms = wall_ms());
                return Err(format!("resume: {e}"));
            }
            let j = set(ctx, id, |j| {
                j.state = "running".into();
                j.note = format!("resumed {pid} {how}");
            });
            event(ctx, Sev::Info, format!("job {id} resumed ({pid} {how})"), j.as_ref());
            Ok(format!("{{\"id\":{},\"state\":\"running\",\"resumed\":{}}}", json_str(id), json_str(how)))
        }
        _ => Err(format!("job: unknown word `{verb}` (submit list get cancel release rerun inspect resume)")),
    }
}

/// A branch name the node's store takes as a ref label (else `job-<lane>`).
fn label(b: &str, lane: &str) -> String {
    let ok = !b.is_empty() && b.len() <= 128 && !b.starts_with(['-', '/', '.']) && !b.ends_with(['/', '.']) && !b.contains("..") && b.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b'/'));
    if ok {
        b.to_string()
    } else {
        format!("job-{lane}")
    }
}

/// The worktree as a job snapshot: its HEAD (and the nested commercial HEAD) as the pinned base,
/// and a copy of its dirty files (the monitor's set: no scan) with their content hashes.
/// Returns (rev, snapshot id or "", files).
pub fn snapshot(ctx: &Ctx, tree: &std::path::Path, lane: &str) -> Result<(String, String, u32), String> {
    let (rev, changed, deleted) = dirty_set(ctx, tree, lane)?;
    if changed.is_empty() && deleted.is_empty() {
        return Ok((rev, String::new(), 0));
    }
    copy_snapshot(ctx, tree, rev, changed, deleted)
}

/// The worktree's pinned base and its uncommitted paths, without what never ships.
fn dirty_set(ctx: &Ctx, tree: &std::path::Path, lane: &str) -> Result<(String, Vec<String>, Vec<String>), String> {
    let d = ctx.monitor.dirty(tree)?;
    let mut rev = format!("{}@{}", label(&d.branch, lane), d.head);
    let (mut changed, mut deleted) = (d.changed, d.deleted);
    let nested = tree.join("apps/commercial");
    if nested.join(".git").exists() {
        let c = ctx.monitor.dirty(&nested)?;
        rev = format!("{rev},{}@{}", label(&c.branch, lane), c.head);
        changed.extend(c.changed.into_iter().map(|p| format!("apps/commercial/{p}")));
        deleted.extend(c.deleted.into_iter().map(|p| format!("apps/commercial/{p}")));
    }
    changed.retain(|p| !crate::lease::ship::never_ships(p));
    deleted.retain(|p| !crate::lease::ship::never_ships(p));
    Ok((rev, changed, deleted))
}

fn copy_snapshot(ctx: &Ctx, tree: &std::path::Path, rev: String, changed: Vec<String>, deleted: Vec<String>) -> Result<(String, String, u32), String> {
    let snap = ctx.store.update(|d| {
        d.snaps_next += 1;
        format!("S{}", d.snaps_next)
    });
    let dir = snap_dir(ctx, &snap);
    let files = dir.join("files");
    let mut man = Vec::new();
    for rel in &changed {
        let (src, dst) = (tree.join(rel), files.join(rel));
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("{}: {e}", p.display()))?;
        }
        let m = std::fs::symlink_metadata(&src).map_err(|e| format!("{}: {e}", src.display()))?;
        let hash = if m.file_type().is_symlink() {
            let target = std::fs::read_link(&src).map_err(|e| e.to_string())?;
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &dst).map_err(|e| format!("{}: {e}", dst.display()))?;
            hex(&makepad_network::digest::sha256_hash(target.to_string_lossy().as_bytes()))
        } else {
            // the bytes and the mtime they were written at (a file saved meanwhile goes as it was read)
            let (bytes, mtime_us, _) = crate::lease::ship::read_stable(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            std::fs::write(&dst, &bytes).map_err(|e| format!("{}: {e}", dst.display()))?;
            let f = std::fs::OpenOptions::new().write(true).open(&dst).map_err(|e| e.to_string())?;
            let _ = f.set_modified(std::time::UNIX_EPOCH + Duration::from_micros(mtime_us));
            let _ = f.set_permissions(m.permissions());
            hex(&makepad_network::digest::sha256_hash(&bytes))
        };
        man.push(format!("{{\"path\":{},\"hash\":\"{hash}\",\"size\":{}}}", json_str(rel), m.len()));
    }
    let manifest = format!("{{\"rev\":{},\"tree\":{},\"changed\":[{}],\"deleted\":[{}]}}", json_str(&rev), json_str(&tree.to_string_lossy()), man.join(","), deleted.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","));
    std::fs::write(dir.join("manifest.json"), manifest).map_err(|e| e.to_string())?;
    Ok((rev, snap, (changed.len() + deleted.len()) as u32))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `submit {"tree","rev"?,"batch","jobs":[{kind,needs,node,prio,lane,agent,cmd,after}]}`: ids at
/// once. Without `rev` the jobs run at the worktree's snapshot (HEAD + its dirty files), with one
/// at those commits alone; a main checkout's dirty files only with `with_uncommitted`. `after` names jobs (`J3`) or earlier jobs of this request (`@1`).
fn submit(ctx: &Ctx, json: &str) -> Result<String, String> {
    let v = makepad_strict_json::parse_depth(json.as_bytes(), 8).map_err(|_| "job submit: bad request")?;
    let reqs = v.get("jobs").and_then(|a| a.as_arr()).filter(|a| !a.is_empty()).ok_or("job submit: no jobs")?;
    let batch = v.get("batch").and_then(|b| b.as_bool()).unwrap_or(false);
    let s = |x: &makepad_strict_json::Value, k: &str| x.get(k).and_then(|y| y.as_str()).unwrap_or("").to_string();
    let list = |x: &makepad_strict_json::Value, k: &str| -> Vec<String> { x.get(k).and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|y| y.as_str().map(str::to_string)).collect() };
    let tree = std::path::PathBuf::from(s(&v, "tree"));
    if !tree.is_absolute() || !tree.join(".git").exists() {
        return Err(format!("job submit: `{}` is not a worktree", tree.display()));
    }
    let mut jobs = Vec::new();
    for (i, r) in reqs.iter().enumerate() {
        let kind = s(r, "kind");
        match Kind::parse(&kind) {
            Some(Kind::Inspect) | None => return Err(format!("job {}: kind `{kind}` (build test screen long timed gpu gate)", i + 1)),
            _ => {}
        }
        let cmd = list(r, "cmd");
        if cmd.is_empty() || cmd[0].is_empty() {
            return Err(format!("job {}: no command", i + 1));
        }
        let lane = s(r, "lane");
        if !crate::trust::valid_name(&lane) {
            return Err(format!("job {}: lane `{lane}` is not a name", i + 1));
        }
        let prio = r.get("prio").and_then(|p| p.as_i64()).unwrap_or(crate::lease::sched::DEFAULT_PRIO as i64) as i32;
        jobs.push(Job { kind, needs: s(r, "needs"), node_pref: s(r, "node"), only: r.get("only").and_then(|o| o.as_bool()).unwrap_or(false), prio, repo_dir: tree.to_string_lossy().into_owned(), lane, agent: s(r, "agent"), cmd, after: list(r, "after"), state: "queued".into(), ..Default::default() });
    }
    // the base and the snapshot: one for the whole request
    let lane = jobs[0].lane.clone();
    let main_checkout = crate::lease::ship::is_main_checkout(&tree);
    let mut left_out = 0;
    let (rev, snap, files) = match s(&v, "rev") {
        r if r.is_empty() && main_checkout && !v.get("with_uncommitted").and_then(|b| b.as_bool()).unwrap_or(false) => {
            let (rev, changed, deleted) = dirty_set(ctx, &tree, &lane)?;
            left_out = changed.len() + deleted.len();
            (rev, String::new(), 0)
        }
        r if r.is_empty() => snapshot(ctx, &tree, &lane)?,
        r => (crate::lease::pin_revs(&tree, &r)?, String::new(), 0),
    };
    let made = ctx.store.update(|d| -> Result<(String, Vec<Job>), String> {
        let first = d.jobs_next + 1;
        let batch_id = if batch {
            d.batches_next += 1;
            format!("B{}", d.batches_next)
        } else {
            String::new()
        };
        let mut out = jobs.clone();
        for (i, j) in out.iter_mut().enumerate() {
            j.id = format!("J{}", first + i as u64);
            j.batch = batch_id.clone();
            j.rev = rev.clone();
            j.snap = snap.clone();
            j.snap_files = files;
            j.submitted_ms = wall_ms();
            for a in j.after.iter_mut() {
                if let Some(n) = a.strip_prefix('@') {
                    let n: usize = n.parse().map_err(|_| format!("job {}: --after {a}", i + 1))?;
                    if n == 0 || n > i {
                        return Err(format!("job {}: --after {a} names no earlier job of this batch", i + 1));
                    }
                    *a = format!("J{}", first + n as u64 - 1);
                } else if !d.jobs.iter().any(|x| &x.id == a) {
                    return Err(format!("job {}: no job {a}", i + 1));
                }
            }
        }
        d.jobs_next += out.len() as u64;
        d.jobs.extend(out.iter().cloned());
        Ok((batch_id, out))
    })?;
    let (batch_id, out) = made;
    // what ships on top of the commits, named: the worktree's uncommitted files go to a box
    let (changed, deleted) = if snap.is_empty() { Default::default() } else { snap_paths(ctx, &snap)? };
    if main_checkout && !snap.is_empty() {
        event(ctx, Sev::Warn, format!("job {}: {snap} ships {files} uncommitted file(s) of the main checkout {} (--with-uncommitted)", out[0].id, tree.display()), Some(&out[0]));
    }
    for j in &out {
        let after = if j.after.is_empty() { String::new() } else { format!(" after {}", j.after.join(",")) };
        event(ctx, Sev::Info, format!("job {} queued{}: {} {}{after} ({}, {})", j.id, if j.batch.is_empty() { String::new() } else { format!(" in {}", j.batch) }, j.kind, crate::proto::clean(&spec_cmd(j), 160), j.lane, if j.snap.is_empty() { "commits only".to_string() } else { format!("{} with {} file(s)", j.snap, j.snap_files) }), Some(j));
    }
    let ids: Vec<String> = out.iter().map(|j| json_str(&j.id)).collect();
    let list = |v: &[String]| v.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(",");
    Ok(format!("{{\"batch\":{},\"jobs\":[{}],\"rev\":{},\"snap\":{},\"files\":{files},\"tree\":{},\"main_checkout\":{main_checkout},\"left_out\":{left_out},\"changed\":[{}],\"deleted\":[{}]}}", json_str(&batch_id), ids.join(","), json_str(&rev), json_str(&snap), json_str(&tree.to_string_lossy()), list(&changed), list(&deleted)))
}

/// `rerun <id>`: the job again, exactly: its base, snapshot, command and placement wishes.
fn rerun(ctx: &Ctx, id: &str) -> Result<String, String> {
    let j = get(ctx, id).ok_or_else(|| format!("no job {id}"))?;
    if !j.snap.is_empty() && !snap_dir(ctx, &j.snap).join("manifest.json").exists() {
        return Err(format!("job {id}: its snapshot {} is gone", j.snap));
    }
    let n = ctx.store.update(|d| {
        d.jobs_next += 1;
        let n = Job { id: format!("J{}", d.jobs_next), kind: j.kind.clone(), needs: j.needs.clone(), node_pref: j.node_pref.clone(), only: j.only, prio: j.prio, rev: j.rev.clone(), repo_dir: j.repo_dir.clone(), snap: j.snap.clone(), snap_files: j.snap_files, lane: j.lane.clone(), agent: j.agent.clone(), cmd: j.cmd.clone(), state: "queued".into(), submitted_ms: wall_ms(), note: format!("rerun of {id}"), ..Default::default() };
        d.jobs.push(n.clone());
        n
    });
    event(ctx, Sev::Info, format!("job {} queued: rerun of {id}", n.id), Some(&n));
    Ok(format!("{{\"jobs\":[{}],\"rerun_of\":{}}}", json_str(&n.id), json_str(id)))
}

/// One job, answer first: `J12 ok exit=0 34s test@107 L7300: cargo test ...`, then its error
/// lines (and the tail with `tail`), or the panic and what to do about it.
pub fn render(j: &Job, tail: bool) -> String {
    let secs = match (j.started_ms, j.ended_ms) {
        (s, e) if s > 0 && e >= s => format!(" {}s", (e - s) / 1000),
        (s, 0) if s > 0 => format!(" {}s so far", wall_ms().saturating_sub(s) / 1000),
        _ => String::new(),
    };
    let at = if j.node.is_empty() { String::new() } else { format!(" {}@{} {}", j.kind, j.node, j.lease) };
    let exit = j.exit.map_or(String::new(), |e| format!(" exit={e}"));
    let mut out = format!("{} {}{exit}{secs}{at}: {}\n", j.id, j.state, crate::proto::clean(&spec_cmd(j), 160));
    if let Some(p) = &j.panic {
        out.push_str(&format!("  panic {}: {} at {} [{}] pid {} ({} ago)\n", p.id, p.message, p.location, p.thread, p.pid, age(p.at_ms)));
        if j.state == "paused" {
            out.push_str(&format!("  -> devhub job inspect {0} stacks | job resume {0} {1} retry|restart|unwind | job release {0}\n", j.id, if p.id.is_empty() { "P?" } else { &p.id }));
        }
    }
    if !j.note.is_empty() {
        out.push_str(&format!("  note: {}\n", j.note));
    }
    if !j.after.is_empty() && j.state == "queued" {
        out.push_str(&format!("  after {}\n", j.after.join(",")));
    }
    for e in &j.errors {
        out.push_str(&format!("  {e}\n"));
    }
    if tail || (j.errors.is_empty() && !matches!(j.state.as_str(), "ok" | "queued" | "leasing" | "running")) {
        for l in &j.tail {
            out.push_str(&format!("  | {l}\n"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::master::LeaseMaster;
    use crate::state::Store;

    fn g(dir: &std::path::Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().unwrap();
        assert!(o.status.success(), "git {args:?}");
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    fn repo(dir: &std::path::Path, branch: &str) {
        std::fs::create_dir_all(dir).unwrap();
        g(dir, &["init", "-q", "-b", branch]);
        g(dir, &["config", "user.email", "t@t"]);
        g(dir, &["config", "user.name", "t"]);
        std::fs::write(dir.join("a.rs"), "a").unwrap();
        std::fs::write(dir.join("gone.rs"), "g").unwrap();
        std::fs::write(dir.join(".gitignore"), "apps/commercial/\n").unwrap();
        g(dir, &["add", "."]);
        g(dir, &["commit", "-q", "-m", "base"]);
    }

    /// A context whose `repo` is a lane worktree with a nested commercial one.
    fn ctx(name: &str) -> Ctx {
        let home = std::env::temp_dir().join(format!("devhub-job-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        repo(&home.join("tree"), "rc-t");
        repo(&home.join("tree/apps/commercial"), "rc-t");
        let leases = Arc::new(LeaseMaster::new("mac", home.clone(), Arc::new(|_: &str, _: &str, _: &str| Err("no nodes".to_string())), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        Ctx { store: Store::open(&home, "mac", Arc::new(|_| {})), leases, machine: "mac".into(), monitor: crate::monitor::Monitor::start(&[]), repo: home.join("tree"), home }
    }

    fn req(cmd: &str, after: &[&str]) -> String {
        let after: Vec<String> = after.iter().map(|a| json_str(a)).collect();
        format!("{{\"kind\":\"test\",\"needs\":\"arch=x64\",\"lane\":\"lane1\",\"agent\":\"a\",\"cmd\":[{}],\"after\":[{}]}}", json_str(cmd), after.join(","))
    }

    fn submit_line(c: &Ctx, rev: &str, batch: bool, jobs: &[String]) -> Result<String, String> {
        serve(c, &format!("job submit {{\"tree\":{},\"rev\":{},\"batch\":{batch},\"with_uncommitted\":true,\"jobs\":[{}]}}", json_str(&c.repo.to_string_lossy()), json_str(rev), jobs.join(",")))
    }

    /// A main checkout's uncommitted files stay out of a job unless asked for by name.
    #[test]
    fn a_main_checkout_ships_only_when_asked() {
        let c = Arc::new(ctx("main"));
        let tree = c.repo.clone();
        std::fs::write(tree.join("a.rs"), "a2").unwrap();
        std::fs::write(tree.join("new.rs"), "n").unwrap();
        let plain = serve(&c, &format!("job submit {{\"tree\":{},\"rev\":\"\",\"batch\":false,\"jobs\":[{}]}}", json_str(&tree.to_string_lossy()), req("check", &[]))).unwrap();
        assert!(plain.contains("\"snap\":\"\",\"files\":0") && plain.contains("\"left_out\":2"), "{plain}");
        let note = super::super::cli::shipped_note(&makepad_strict_json::parse(plain.as_bytes()).unwrap());
        assert!(note.contains("2 uncommitted") && note.contains("--with-uncommitted"), "{note}");
    }

    #[test]
    fn a_submit_snapshots_the_worktree_and_batches_order_by_deps() {
        let c = Arc::new(ctx("a"));
        let tree = c.repo.clone();
        std::fs::write(tree.join("a.rs"), "a2").unwrap();
        std::fs::write(tree.join("new.rs"), "n").unwrap();
        std::fs::remove_file(tree.join("gone.rs")).unwrap();
        std::fs::write(tree.join("apps/commercial/a.rs"), "c2").unwrap();
        // agent configuration and local/ never ship
        std::fs::create_dir_all(tree.join(".claude/skills")).unwrap();
        std::fs::write(tree.join(".claude/skills/s.md"), "x").unwrap();
        std::fs::create_dir_all(tree.join("local")).unwrap();
        std::fs::write(tree.join("local/note.txt"), "x").unwrap();
        let r = submit_line(&c, "", true, &[req("check", &[]), req("test", &["@1"]), req("present", &["@1", "@2"])]).unwrap();
        let (head, chead) = (g(&tree, &["rev-parse", "HEAD"]), g(&tree.join("apps/commercial"), &["rev-parse", "HEAD"]));
        assert!(r.starts_with("{\"batch\":\"B1\",\"jobs\":[\"J1\",\"J2\",\"J3\"]"), "{r}");
        assert!(r.contains(&format!("\"rev\":\"rc-t@{head},rc-t@{chead}\"")) && r.contains("\"snap\":\"S1\",\"files\":4"), "{r}");
        // submit says what it ships, and warns when that is the landed checkout's
        let note = super::super::cli::shipped_note(&makepad_strict_json::parse(r.as_bytes()).unwrap());
        assert!(!note.contains(".claude") && !note.contains("local/") && note.starts_with("WARNING: this is a main checkout") && note.contains("  new.rs\n") && note.contains("  apps/commercial/a.rs\n") && note.contains("  deleted gone.rs\n"), "{note}");
        // the snapshot holds the files as they were at submit
        let dir = snap_dir(&c, "S1");
        std::fs::write(tree.join("a.rs"), "a3").unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("files/a.rs")).unwrap(), "a2");
        assert_eq!(std::fs::read_to_string(dir.join("files/apps/commercial/a.rs")).unwrap(), "c2");
        let man = std::fs::read_to_string(dir.join("manifest.json")).unwrap();
        assert!(man.contains("\"deleted\":[\"gone.rs\"]") && man.contains("\"path\":\"new.rs\""), "{man}");
        assert_eq!(c.store.read(|d| d.jobs[2].after.clone()), vec!["J1".to_string(), "J2".to_string()]);
        // the commits alone with a rev
        let r = submit_line(&c, "rc-t", false, &[req("x", &[])]).unwrap();
        assert!(r.contains(&format!("\"rev\":\"rc-t@{head}\",\"snap\":\"\"")), "{r}");
        // bad requests change nothing
        assert!(submit_line(&c, "", false, &[req("x", &["@1"])]).unwrap_err().contains("no earlier job"));
        assert!(submit_line(&c, "", false, &[req("x", &["J9"])]).unwrap_err().contains("no job J9"));
        assert!(submit_line(&c, "", false, &[req("x", &[]).replace("\"test\"", "\"inspect\"")]).is_err());
        assert_eq!(c.store.read(|d| d.jobs.len()), 4);
        // J1 failed: J2 and J3 are skipped, never leased
        c.store.update(|d| d.jobs[0].state = "failed".into());
        dispatch(&c);
        dispatch(&c);
        assert_eq!(c.store.read(|d| d.jobs[..3].iter().map(|j| j.state.clone()).collect::<Vec<_>>()), vec!["failed", "skipped", "skipped"]);
        assert!(c.store.read(|d| d.jobs[1].note.clone()).contains("J1 failed"));
        // a queued job is cancelled at once; a rerun replays a job from its snapshot
        submit_line(&c, "", false, &[req("y", &["J1"])]).unwrap();
        assert!(serve(&c, "job cancel J5").unwrap().contains("cancelled"));
        let r = serve(&c, "job rerun J1").unwrap();
        assert!(r.contains("\"J6\""), "{r}");
        assert_eq!(c.store.read(|d| (d.jobs[5].snap.clone(), d.jobs[5].cmd.clone(), d.jobs[5].after.len())), ("S1".to_string(), vec!["check".to_string()], 0));
        c.store.update(|d| d.jobs[5].state = "cancelled".into());
        assert!(serve(&c, "job get J5 J9").unwrap().contains("\"state\":\"unknown\""));
        assert!(serve(&c, "job resume J5 P1 retry").unwrap_err().contains("not paused"));
        let home = c.repo.parent().unwrap().to_path_buf();
        drop(c);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn digest_keeps_errors_and_tail() {
        let text = "Compiling a\nerror[E0425]: cannot find `x`\n  --> a.rs:1\nthread 'main' panicked at a.rs:3\n\nok\n";
        let (e, t) = digest(text);
        assert_eq!(e, vec!["error[E0425]: cannot find `x`".to_string(), "thread 'main' panicked at a.rs:3".to_string()]);
        assert_eq!(t.last().map(String::as_str), Some("ok"));
        let j = Job { id: "J1".into(), state: "paused".into(), cmd: vec!["cargo".into()], panic: Some(JobPanic { id: "P1".into(), pid: 7, message: "boom".into(), thread: "main".into(), location: "a.rs:3".into(), at_ms: 0 }), ..Default::default() };
        assert!(render(&j, false).contains("job resume J1 P1 retry|restart|unwind"));
    }
}
