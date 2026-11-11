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
//! alive, and the agent acts through the job (`job inspect`, `job resume`, `job release`). The
//! pause lasts while the process holds that panic parked: one that went on (an edit fixed the call,
//! the park's own limit unwound it, a script killed it) leaves the job running, and the verdict is
//! the job's own exit. A box released while the panic is parked ends the job `panicked`.

use super::leg::{Held, Spec};
use super::{age, Ctx};
use crate::cli::ask;
use crate::lease::{CostClass, Kind, Priority, Src};
use crate::proto::{wall_ms, Record, Sev};
use crate::state::{Job, JobPanic};
use makepad_remote::json_str;
use makepad_remote::link::{Client, Target};
use makepad_strict_json::Value;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// A paused job with no agent action for this long releases its box: past the JIT's own 600 s
/// limit on a park, so a park ending at its limit reaches [`runs_on`] first.
pub const PAUSE_IDLE_MS: u64 = 630 * 1000;
const TICK: Duration = Duration::from_secs(2);
const ERRORS: usize = 12;
const TAIL: usize = 15;
/// What a running job's view reads of its log: its end (the whole log is read once, as it ends).
const LIVE_READ: u64 = 256 << 10;

fn ctl() -> Client {
    Client::new(Target::Unix(crate::proto::control_socket()), "devhub-job")
}

fn event(ctx: &Ctx, sev: Sev, line: String, j: Option<&Job>) {
    ctx.store.event("job", sev, line, j.map(|j| crate::state::job_value(j).to_json()));
}

pub fn start(ctx: Arc<Ctx>) {
    // what a previous master ran goes on: the run lives on its node, and the lease came back with
    // the master. So does a run it had asked for (shipped): the node may be building its JIT for
    // it (one job lost two placements' JIT builds to two master restarts, 11-02). A job whose
    // lease is still going or whose snapshot is still shipping, or whose lease ended meanwhile,
    // is placed again; one still waiting for a box asks again.
    let held = |j: &Job| super::leg::lease_held(&ctx, &j.lease) && (j.run_n > 0 || j.granted_ms > 0 && j.shipped_ms >= j.granted_ms);
    let (back, moved, stray): (Vec<Job>, Vec<Job>, Vec<String>) = ctx.store.update(|d| {
        let (mut back, mut moved, mut stray) = (Vec::new(), Vec::new(), Vec::new());
        let pending: std::collections::HashSet<String> = d.jobs.iter().filter(|j| matches!(j.state.as_str(), "queued" | "leasing"))
            .flat_map(|j| j.after.iter().cloned()).collect();
        for j in d.jobs.iter_mut() {
            match j.state.as_str() {
                "leasing" => {
                    j.state = "queued".into();
                    j.attempts = j.attempts.saturating_sub(1);
                }
                // A restart between finish and release resumes the handoff, not the run.
                "ok" if j.kind == "timed" && (held(j) || pending.contains(&j.id)) => back.push(j.clone()),
                "running" | "paused" if held(j) => back.push(j.clone()),
                "running" | "paused" => {
                    if j.run_n == 0 {
                        j.attempts = j.attempts.saturating_sub(1);
                    }
                    // (a lease that outlived the restart but had nothing running yet ends here)
                    let what = if super::leg::lease_held(&ctx, &j.lease) {
                        stray.push(j.lease.clone());
                        "the master restarted while it was setting up its lease, so that lease ended"
                    } else {
                        "its lease did not outlive the master's restart"
                    };
                    if j.attempts < ATTEMPTS {
                        j.state = "queued".into();
                        j.note = format!("{what}; placed again");
                    } else {
                        j.state = "lost".into();
                        j.ended_ms = wall_ms();
                        j.note = format!("{what} (no placements left)");
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
    }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
    for l in stray {
        end_lease(&l);
    }
    for j in moved {
        event(&ctx, Sev::Warn, format!("job {}: {}", j.id, j.note), Some(&j));
    }
    prune_logs(&ctx);
    crate::lease::shadow::start(ctx.clone());
    // (its own thread: it asks the boxes, and a box slow to answer holds no dispatch)
    let c = ctx.clone();
    let _ = std::thread::Builder::new().name("devhub-job-parks".into()).spawn(move || loop {
        runs_on(&c, &holds_parked);
        std::thread::sleep(TICK);
    });
    let _ = std::thread::Builder::new().name("devhub-jobs".into()).spawn(move || {
        // Recover every completed handoff before any other dispatcher can take its successors.
        // No new recovery state: the successful job, its lease and asked_ms are already durable.
        let asked = {
            let _dispatch = DISPATCH.lock().unwrap();
            let following: Vec<String> = back.iter().filter(|j| j.state == "ok").flat_map(|j| chain_successors(&ctx, &j.id)).collect();
            dispatch_following(&ctx, &following)
        };
        for next in asked { let _ = next.recv(); }
        for j in back {
            if j.state == "ok" {
                if super::leg::lease_held(&ctx, &j.lease) { Held::attach(&j.lease, &j.node).release(Some(0)); }
                continue;
            }
            event(&ctx, Sev::Info, format!("job {}: the master restarted; its run on {} goes on", j.id, j.node), Some(&j));
            let c = ctx.clone();
            let _ = std::thread::Builder::new().name("devhub-job".into()).spawn(move || reattach(&c, &j));
        }
        loop {
            dispatch(&ctx);
            idle(&ctx);
            ctx.store.wait(TICK);
        }
    });
}

/// What lease ends keep (lease/master.rs), and how many of them stay.
const LEASE_LOGS: &str = "lease-logs";
const LEASE_LOGS_KEPT: usize = 500;

/// Removes the logs and snapshots of jobs the state no longer keeps, and all but the newest lease logs.
fn prune_logs(ctx: &Ctx) {
    if let Ok(rd) = std::fs::read_dir(ctx.logs(LEASE_LOGS)) {
        let mut logs: Vec<_> = rd.flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
        logs.sort_by(|a, b| b.0.cmp(&a.0));
        for (_, old) in logs.iter().skip(LEASE_LOGS_KEPT) {
            let _ = std::fs::remove_file(old);
        }
    }
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

// Serialize dispatch with a timed completion so the periodic dispatcher cannot take its
// successors before their place and first-ask notifications have been assigned.
static DISPATCH: Mutex<()> = Mutex::new(());

/// Starts every queued job whose dependencies ended ok; skips those whose did not.
fn dispatch(ctx: &Arc<Ctx>) {
    let _dispatch = DISPATCH.lock().unwrap();
    dispatch_following(ctx, &[]);
}

fn dispatch_following(ctx: &Arc<Ctx>, following: &[String]) -> Vec<mpsc::Receiver<()>> {
    let mut asked = Vec::new();
    let (go, skip): (Vec<String>, Vec<(String, String)>) = ctx.store.read(|d| {
        let state = |id: &str| d.jobs.iter().find(|j| j.id == id).map(|j| j.state.clone());
        let (mut go, mut skip) = (Vec::new(), Vec::new());
        for j in d.jobs.iter().filter(|j| j.state == "queued") {
            let deps: Vec<(String, Option<String>)> = j.after.iter().map(|a| (a.clone(), state(a))).collect();
            if let Some((a, s)) = deps.iter().find(|(_, s)| s.as_deref().is_none_or(|s| matches!(s, "failed" | "panicked" | "cancelled" | "skipped" | "lost" | "jit-error"))) {
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
        }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
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
        }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
        if ok {
            let notify = following.contains(&id).then(|| {
                let (send, recv) = mpsc::channel();
                asked.push(recv);
                send
            });
            let c = ctx.clone();
            let _ = std::thread::Builder::new().name("devhub-job".into()).spawn(move || run(&c, &id, notify));
        }
    }
    asked
}

/// Ready timed successors in the same lane inherit the chain's first ask, once.
fn chain_successors(ctx: &Ctx, id: &str) -> Vec<String> {
    ctx.store.update(|d| {
        let Some(j) = d.jobs.iter().find(|j| j.id == id && j.kind == "timed" && j.state == "ok") else { return Vec::new() };
        let (lane, asked) = (j.lane.clone(), j.asked_ms);
        let ready: Vec<String> = d.jobs.iter().filter(|j| j.state == "queued" && j.kind == "timed" && j.lane == lane && j.after.iter().any(|a| a == id)
            && j.after.iter().all(|a| d.jobs.iter().any(|dep| dep.id == *a && dep.state == "ok"))).map(|j| j.id.clone()).collect();
        for j in d.jobs.iter_mut().filter(|j| ready.contains(&j.id) && j.asked_ms == 0) { j.asked_ms = asked; }
        ready
    }).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Vec::new() })
}

fn get(ctx: &Ctx, id: &str) -> Option<Job> {
    ctx.store.read(|d| d.jobs.iter().find(|j| j.id == id).cloned())
}

fn set(ctx: &Ctx, id: &str, f: impl FnOnce(&mut Job)) -> Option<Job> {
    try_set(ctx, id, f).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); None })
}

fn try_set(ctx: &Ctx, id: &str, f: impl FnOnce(&mut Job)) -> Result<Option<Job>, String> {
    ctx.store.update(|d| {
        let j = d.jobs.iter_mut().find(|j| j.id == id)?;
        f(j);
        Some(j.clone())
    })
}

/// The first error lines and the last lines of a job's output. A test program that passed comes
/// last: what its tests panicked with, they asked for (a result led with `injected part job panic`
/// from one, the failing program's assertion further down, 11-04).
pub fn digest(text: &str) -> (Vec<String>, Vec<String>) {
    let is_err = |l: &str| {
        let t = l.trim_start();
        t.starts_with("error") || t.contains("panicked at") || t.starts_with("FAILED") || t.contains(" FAILED") || t.contains("-FAIL") || t.starts_with("test result: FAILED") || t.contains(": error:")
    };
    let (mut errors, mut block, mut passed) = (Vec::new(), Vec::new(), Vec::new());
    for l in text.lines() {
        let t = l.trim_start();
        if t.starts_with("running ") && (t.ends_with(" test") || t.ends_with(" tests")) {
            errors.append(&mut block);
        } else if t.starts_with("test result: ok") {
            passed.append(&mut block);
        }
        block.extend(report_line(l).or_else(|| is_err(l).then(|| crate::proto::clean(l, 240))));
    }
    errors.append(&mut block);
    errors.append(&mut passed);
    errors.truncate(ERRORS);
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    let tail = lines[lines.len().saturating_sub(TAIL)..].iter().map(|l| crate::proto::clean(l, 240)).collect();
    (errors, tail)
}

/// A JIT's report of a panic or fault (`  report: {"kind":"panic","message":..,"location":..}`):
/// what ended the thread, and where. It says no "panicked at", so a failed test's result had only
/// its FAILED lines, and the tail of a `--verbose` run is the JIT's end-of-run summary: the panic
/// was in the log and in no result (11-04).
fn report_line(line: &str) -> Option<String> {
    let v = makepad_strict_json::parse(line.trim_start().strip_prefix("report: ")?.as_bytes()).ok()?;
    let text = |k: &str| v.get(k).and_then(|x| x.as_str()).map(str::to_string);
    Some(crate::proto::clean(&format!("{}: {} at {}", text("kind")?, text("message")?, text("location").unwrap_or_default()), 800))
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
    let ok = |p: &String| !crate::lease::ship::never_ships_named(p);
    let changed = v.get("changed").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.get("path").and_then(|p| p.as_str()).map(str::to_string)).filter(ok).collect();
    let deleted = v.get("deleted").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| x.as_str().map(str::to_string)).filter(ok).collect();
    Ok((changed, deleted))
}

/// Ships the snapshot into the lease's tree (already at the snapshot's base commits): only the
/// files the box lacks as they are go; then `ship-done` names them and the deletions.
pub(super) fn ship_snap(ctx: &Ctx, held: &Held, snap: &str) -> Result<String, String> {
    let (changed, deleted) = if snap.is_empty() { (Vec::new(), Vec::new()) } else { snap_paths(ctx, snap)? };
    let files = snap_dir(ctx, snap).join("files");
    let mut entries = Vec::new();
    for p in &changed {
        crate::lease::ship::entries(&files, p, &mut entries)?;
    }
    if !entries.is_empty() { crate::lease::ship::send(held.client(), &held.node, &held.id, &files, &entries, false)?; }
    let list = |v: &[String]| v.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(",");
    let manifest = if snap.is_empty() { None } else {
        let text = std::fs::read_to_string(snap_dir(ctx, snap).join("manifest.json")).map_err(|e| e.to_string())?;
        Some(makepad_strict_json::parse(text.as_bytes()).map_err(|e| format!("snapshot {snap}: {e:?}"))?)
    };
    let bases = match &manifest {
        Some(v) => v.get("bases").ok_or("snapshot lacks pinned base blobs")?.to_json(),
        None => "{}".to_string(),
    };
    let body = format!("{{\"shipped\":[{}],\"deleted\":[{}],\"bases\":{bases}}}", list(&changed), list(&deleted));
    let r = ask(held.client(), Some(&held.node), "ship-done", &format!("ship-done {} {}", held.id, makepad_network::digest::base64_encode(body.as_bytes())))?;
    if let Some(e) = r.first().filter(|l| l.starts_with("{\"err\":")) {
        return Err(format!("{}: {e}", held.node));
    }
    let source = r.first().and_then(|l| makepad_strict_json::parse(l.as_bytes()).ok()).and_then(|v| v.get("source").and_then(|s| s.as_str()).map(str::to_string)).unwrap_or_default();
    // it runs what was submitted, or not at all
    if let Some(v) = &manifest {
        let hashes: std::collections::BTreeMap<&str, &str> = v.get("changed").and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|x| Some((x.get("path")?.as_str()?, x.get("hash")?.as_str()?))).collect();
        let files: Vec<(String, String)> = changed.iter().map(|p| (p.clone(), hashes.get(p.as_str()).unwrap_or(&"").to_string())).collect();
        let pin = v.get("rev").and_then(|r| r.as_str()).and_then(crate::lease::Pin::of_rev).map_or(String::new(), |p| p.key());
        let want = crate::lease::ship::source_id(&pin, &files, &deleted);
        if !source.is_empty() && source != want {
            return Err(format!("{}: took source {source}, snapshot {snap} is {want}: its files differ on the box", held.node));
        }
    }
    Ok(source)
}

/// How often a job is placed again when its box is lost under it.
pub const ATTEMPTS: u32 = 3;

/// One job: lease, ship its snapshot, run, release, result.
fn run(ctx: &Arc<Ctx>, id: &str, notify: Option<mpsc::Sender<()>>) {
    let Some(j) = set(ctx, id, |j| j.attempts += 1) else { return };
    let dir = ctx.logs("jobs");
    let _ = std::fs::create_dir_all(&dir);
    let log = dir.join(format!("{id}.log"));
    let kind = Kind::parse(&j.kind).unwrap_or(Kind::Test);
    // its place in the queue from its first ask on: a master's restart or a lost box keeps it
    let asked = set(ctx, id, |j| {
        if j.asked_ms == 0 {
            j.asked_ms = wall_ms();
        }
    }).map_or(0, |j| j.asked_ms);
    let spec = Spec { since_ms: (asked > 0).then_some(asked), lane: if j.shadow_of.is_empty() { j.lane.clone() } else { format!("shadow-{}", j.id) }, agent: j.agent.clone(), kind, needs: j.needs.clone(), prio: j.prio, rev: j.rev.clone(), repo_dir: j.repo_dir.clone(), node: (!j.node_pref.is_empty()).then(|| j.node_pref.clone()) };
    let cancelled = || get(ctx, id).is_none_or(|j| j.state == "cancelled");
    let configure = |r: &mut crate::lease::sched::Req| configure_request(r, &j);
    // what it waits for shows in the job (a job waited 23 min on its lane's own lease, its queue
    // reason in no output its lane read, 11-05)
    let said = std::cell::RefCell::new(String::new());
    let notify = std::cell::RefCell::new(notify);
    let asked = || { if let Some(send) = notify.borrow_mut().take() { let _ = send.send(()); } };
    let mut waiting = |line: &str| {
        asked();
        let line = crate::proto::clean(line, 400);
        set(ctx, id, |j| j.note = line.clone());
        *said.borrow_mut() = line;
    };
    let acquired = Held::acquire_with(&spec, &configure, &mut waiting, &cancelled);
    asked();
    let held = match acquired {
        Ok(h) => h,
        // the master's socket or a box's link went away, or the master answered its ask with
        // nothing: no verdict on the job, which never ran
        Err(e) if super::leg::exit_lost(&e) && again(ctx, id, &format!("no lease ({})", crate::proto::clean(&e, 160))) => return,
        Err(e) => {
            ended_by_hand(ctx, id, &e);
            return finish(ctx, id, Err(e), &log);
        }
    };
    let class = j.shadow.then(|| ctx.leases.nodes().into_iter().find(|n| n.name == held.node)).flatten().and_then(|n| {
        n.caps.as_ref().map(|c| [("arch".to_string(), c.arch.clone()), ("os".into(), c.os.clone()), ("role".into(), n.role().as_str().into())])
    });
    let j = set(ctx, id, |j| {
        if j.state == "leasing" {
            if j.note == *said.borrow() { j.note.clear(); }
            j.state = "running".into();
            j.lease = held.id.clone();
            j.node = held.node.clone();
            j.granted_ms = wall_ms();
            j.started_ms = j.granted_ms;
            if let Some(class) = &class {
                let mut needs = crate::lease::sched::parse_needs(&j.needs);
                needs.retain(|(k, _)| !matches!(k.as_str(), "arch" | "os" | "role"));
                needs.extend(class.iter().cloned());
                j.needs = needs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",");
            }
        }
    });
    let Some(j) = j.filter(|j| j.state == "running") else {
        // cancelled while the grant came
        held.release(None);
        return finish(ctx, id, Err("cancelled".into()), &log);
    };
    let mut source = String::new();
    if !j.snap.is_empty() || j.src == "overlay" {
        match ship_snap(ctx, &held, &j.snap) {
            Ok(s) => source = s,
            Err(e) => {
                held.release(None);
                if super::leg::exit_lost(&e) && again(ctx, id, &format!("shipping to {} failed ({})", j.node, crate::proto::clean(&e, 160))) {
                    return;
                }
                return finish(ctx, id, Err(format!("shipping {}: {e}", j.snap)), &log);
            }
        }
    }
    let j = set(ctx, id, |j| { j.shipped_ms = wall_ms(); j.source = source.clone(); }).unwrap_or(j);
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
    let r = run_on(ctx, &held, &j, &log, &mut out);
    drop(out);
    ended(ctx, id, held, r, &log);
}

/// Asks the node for the job's run and follows it; the run's number and where its output starts
/// in the log are noted for a restarted master.
fn run_on(ctx: &Ctx, held: &Held, j: &Job, log: &std::path::Path, out: &mut Out) -> Result<i32, String> {
    let mut persistence_error = None;
    // one run per lease, asked under the job's name: a master that took the lease back asks
    // again under it ([`Held::run_asked`])
    let result = held.run_asked(&format!("job-{}", j.id), if j.cwd.is_empty() { "." } else { &j.cwd }, &j.cmd, out, &mut |n| {
        let base = std::fs::metadata(log).map_or(0, |m| m.len());
        if let Err(e) = try_set(ctx, &j.id, |j| { j.run_n = n; j.log_base = base; }) {
            eprintln!("devhub job run record: {e}");
            persistence_error = Some(e);
        }
    });
    match persistence_error { Some(e) => Err(e), None => result }

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
    // a run asked for and not noted: the node has it (followed), or is still getting it ready (the
    // same ask answers from that: an overlay's JIT resolution goes on), or never got the ask
    let r = match j.run_n {
        0 => match held.has_run(1) {
            Ok(true) => {
                set(ctx, id, |j| { j.run_n = 1; j.log_base = off; });
                held.follow(1, 0, &mut out)
            }
            Ok(false) => run_on(ctx, &held, j, &log, &mut out),
            Err(e) => Err(e),
        },
        n => held.follow(n, off.saturating_sub(j.log_base), &mut out),
    };
    drop(out);
    ended(ctx, id, held, r, &log);
}

/// `adopt {lease,pid,run,lane,agent,kind,cmd,cwd,rev,repo_dir,class,priority,src}`: a `devhub
/// run`/`test`/`lease -- cmd` client's run, told every process of its lease sat idle, becomes a
/// job, and the client returns to its owner with the verdict: an owner waits on that process's
/// end, and a lane's background `devhub test` hung 16 min with its verdict on a stderr nobody read
/// (11-10). The run goes on, followed from its first byte into the job's log; `take` hands the
/// lease over from its holder `pid` (its box, its grant). devhub ends nothing: `job wait`, `job
/// result` and `job cancel` are the owner's. Placed again it would run without the client's tree:
/// it never is.
fn adopt(ctx: &Ctx, json: &str, take: &dyn Fn(&str, u32) -> Result<(String, u64), String>) -> Result<Job, String> {
    let v = makepad_strict_json::parse_depth(json.as_bytes(), 8).map_err(|_| "job adopt: bad request")?;
    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let lease = text("lease");
    let pid = v.get("pid").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok()).ok_or("job adopt: no pid")?;
    let run_n = v.get("run").and_then(Value::as_u64).filter(|n| *n > 0).ok_or("job adopt: no run")?;
    let cmd: Vec<String> = v.get("cmd").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect();
    if lease.is_empty() || cmd.is_empty() { return Err("job adopt: no lease or command".into()); }
    let (node, granted) = take(&lease, pid)?;
    let j = ctx.store.update(|d| {
        d.jobs_next += 1;
        let id = format!("J{}", d.jobs_next);
        let log = ctx.logs("jobs").join(format!("{id}.log")).to_string_lossy().into_owned();
        // its wait was told this lease's verdict: the client said it
        let j = Job { id, lane: text("lane"), agent: text("agent"), kind: text("kind"), cmd: cmd.clone(), cwd: text("cwd"), rev: text("rev"), repo_dir: text("repo_dir"), class: text("class"), priority: text("priority"), src: text("src"), state: "running".into(), submitted_ms: granted, asked_ms: granted, granted_ms: granted, shipped_ms: granted, started_ms: granted, lease: lease.clone(), node: node.clone(), run_n, log, attempts: ATTEMPTS, idle_told: lease.clone(), note: format!("{lease}'s client handed its run over at the idle verdict"), ..Default::default() };
        d.jobs.push(j.clone());
        j
    })?;
    event(ctx, Sev::Warn, format!("job {}: {lease} on {node} of lane {}, every process idle, goes on as this job (its client returned to its owner); `devhub job cancel {}` ends it", j.id, j.lane, j.id), Some(&j));
    Ok(j)
}

/// A job's run is over: its lease goes, and it has its result, or (the box lost under it, no exit)
/// it is placed again from its snapshot.
fn ended(ctx: &Arc<Ctx>, id: &str, held: Held, r: Result<i32, String>, log: &std::path::Path) {
    if get(ctx, id).is_some_and(|j| j.kind == "timed") { note_quiet(ctx, id, &held); }
    if let Some(j) = get(ctx, id).filter(|j| j.waited_ms > 0) {
        crate::lease::prof::log_wait(&ctx.home, &format!("{{\"kind\":\"job\",\"lane\":{},\"id\":{},\"lease\":{},\"start_ms\":{},\"end_ms\":{}}}", json_str(&j.lane), json_str(id), json_str(&j.lease), j.waited_ms, wall_ms()));
    }
    let finished = matches!(r, Ok(0)) && get(ctx, id).is_some_and(|j| j.kind == "timed");
    if finished {
        let asked = {
            let _dispatch = DISPATCH.lock().unwrap();
            finish(ctx, id, Ok(0), log);
            let following = chain_successors(ctx, id);
            dispatch_following(ctx, &following)
        };
        // The first on_wait (including "held back") or acquire_with's return ends the
        // handoff. No grant wait: that grant needs this lease to end. No timeout.
        for next in asked { let _ = next.recv(); }
    }
    let node = held.node.clone();
    let fault = held.release(r.as_ref().ok().copied());
    if get(ctx, id).is_some_and(|j| !j.shadow_of.is_empty()) {
        let lane = format!("shadow-{id}");
        let cleaned = ask(&ctl(), Some(&node), "disk", &format!("disk clean {lane}"));
        if let Err(e) = cleaned.and_then(|lines| match lines.iter().find(|l| l.starts_with("{\"err\":")) { Some(e) => Err(e.clone()), None => Ok(()) }) {
            ctx.store.event("shadow", Sev::Warn, format!("shadow {id} cleanup on {node}: {e}"), None);
        }
    }
    // what it did to its box is its result, whatever its run said (it wrote under the overlay's
    // virtual root, and the box refused every overlay lease after it: a Linux node, 11-06)
    if let Some(why) = fault.filter(|_| !finished) {
        return finish(ctx, id, Err(why), log);
    }
    // (a run the node refused is the job's result: another box refuses it too)
    if let Some(e) = r.as_ref().err().filter(|e| !e.starts_with(super::leg::REFUSED)) {
        if !ended_by_hand(ctx, id, e) && !e.contains(" ended (disk): ") && !e.contains(" ended (memory): ") {
            let node = get(ctx, id).map_or(String::new(), |j| j.node);
            if again(ctx, id, &format!("box {node} lost ({})", crate::proto::clean(e, 160))) {
                return;
            }
        }
    }
    if !finished { finish(ctx, id, r, log); }
}

/// Whether a timed job's box was quiet under its run (`lease-box`, [`crate::lease::master::unquiet`]):
/// one that was not says so in its result, before its waiter reads it (a series' next job takes
/// the box before this one's release says what shared it): its timings are not clean.
fn note_quiet(ctx: &Ctx, id: &str, held: &Held) {
    let Ok(lines) = ask(&ctl(), Some(&held.node), "lease-box", &format!("lease-box {}", held.id)) else { return };
    let Some(v) = lines.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok()) else { return };
    let Some(b) = v.get("box").filter(|b| !b.is_null()) else { return };
    if let Some(why) = crate::lease::master::unquiet(b) {
        set(ctx, id, |j| j.unquiet = format!("{}: {why}; {}", held.node, crate::lease::master::box_line(b)));
    }
}

/// Someone ended the job's lease by hand (`devhub cancel L..` or `devhub release L..`, what the
/// hung warning says), granted or still asked: the job ends with it, cancelled. Placed again, a
/// hung job came straight back and hung again (J6015, 11-04), and a released one asked again on a
/// box with dev off and waited there unseen (J8832, 11-06). Whether it was so.
fn ended_by_hand(ctx: &Ctx, id: &str, e: &str) -> bool {
    if !super::leg::ended_by_hand(e) { return false; }
    set(ctx, id, |j| if !j.done() { j.state = "cancelled".into(); });
    true
}

/// Back to the queue, with what happened as its note, if it has placements left; whether it went.
fn again(ctx: &Ctx, id: &str, why: &str) -> bool {
    let j = set(ctx, id, |j| {
        if matches!(j.state.as_str(), "leasing" | "running") && j.attempts < ATTEMPTS && j.holder.is_none_or(crate::os::pid_alive) {
            j.state = "queued".into();
            j.note = format!("{why}; placed again");
            j.unquiet.clear();
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

/// What its lease kept of a stall (its processes, every JIT thread's stack: the idle verdict's
/// capture) goes at the end of the job's log, after the run's output its result reads: a hung
/// job's evidence is its own log's, also once it is cancelled (11-10).
fn keep_stall(ctx: &Ctx, id: &str, log: &std::path::Path) {
    use std::io::Write;
    let Some(lease) = get(ctx, id).map(|j| j.lease).filter(|l| !l.is_empty()) else { return };
    let Some(kept) = std::fs::read_to_string(ctx.logs(LEASE_LOGS).join(format!("{lease}.log"))).ok().filter(|k| k.contains(" when it stalled (")) else { return };
    let added = std::fs::OpenOptions::new().append(true).create(true).open(log).and_then(|mut f| f.write_all(format!("\n== what {lease} kept when it stalled (`devhub job result {lease}`)\n{kept}").as_bytes()));
    if let Err(e) = added { eprintln!("devhub job {id}: its stall's capture is not in {}: {e}", log.display()); }
}

/// Why a failed job failed, for its event line: its first error line; else, when its run exited
/// by itself, the last line of its output (a script's `bash: x.sh: No such file or directory` has
/// no error word: the line read "failed exit=127 in 0 s on <box>:", 11-04); else what ended it.
fn fail_reason(j: &Job, ended_by_exit: bool) -> String {
    let said = j.errors.first().or(if ended_by_exit { j.tail.last() } else { None });
    said.cloned().filter(|l| !l.is_empty()).unwrap_or_else(|| j.note.clone())
}

fn spec_cmd(j: &Job) -> String {
    j.cmd.join(" ")
}

/// The job's result from its run (cancelled and released-while-paused jobs keep that outcome).
fn finish(ctx: &Ctx, id: &str, r: Result<i32, String>, log: &std::path::Path) {
    let text = std::fs::read_to_string(log).unwrap_or_default();
    let (errors, tail) = digest(&text);
    keep_stall(ctx, id, log);
    // Source discovery can read many files; do it before taking the state store's lock.
    let none = get(ctx, id).and_then(|j| {
        let mut why = matches!(r, Ok(0)).then(|| crate::testlog::Tally::of(&text).no_pass(&j.cmd)).flatten()?;
        crate::testlog::no_pass_hint(&mut why, std::path::Path::new(&j.repo_dir), &j.rev, &j.cmd);
        Some(why)
    });
    let j = set(ctx, id, |j| {
        j.ended_ms = wall_ms();
        j.errors = errors;
        // a test run that executed no test is no pass, whatever its exit
        if matches!(r, Ok(makepad_remote::hot::JIT_ERROR_EXIT)) {
            j.errors.clear();
            j.note = makepad_remote::hot::Verdict::from_output(makepad_remote::hot::JIT_ERROR_EXIT, &text)
                .map(|v| v.line()).unwrap_or_else(|| "JIT infrastructure error; its verdict record was not received".into());
        }
        j.tail = tail;
        j.log = log.to_string_lossy().into_owned();
        j.state = match (j.state.as_str(), &r) {
            ("cancelled", _) => "cancelled".into(),
            (_, Ok(makepad_remote::hot::JIT_ERROR_EXIT)) => "jit-error".into(),
            ("paused", Err(_)) => "panicked".into(),
            (_, Ok(0)) if none.is_some() => "failed".into(),
            (_, Ok(0)) => "ok".into(),
            (_, Ok(_)) => "failed".into(),
            (_, Err(_)) => "failed".into(),
        };
        if let Ok(code) = &r {
            j.exit = Some(*code);
        }
        if let Some(why) = none {
            j.errors.insert(0, why);
        }
        if let Err(e) = &r {
            // what ended it, also over what an earlier placement noted
            j.note = crate::proto::clean(e, 240);
        }
    });
    let Some(j) = j else { return };
    let ended_by_exit = r.is_ok();
    let secs = j.ended_ms.saturating_sub(if j.started_ms > 0 { j.started_ms } else { j.submitted_ms }) / 1000;
    let what = match j.state.as_str() {
        "ok" => format!("job {id} ok in {secs} s on {}", j.node),
        "panicked" => format!("job {id} panicked: {}", j.panic.as_ref().map_or(String::new(), |p| format!("{} at {}", p.message, p.location))),
        "cancelled" => format!("job {id} cancelled{}", r.as_ref().err().map_or(String::new(), |e| format!(": {}", crate::proto::clean(e, 160)))),
        "jit-error" => j.note.clone(),
        _ => format!("job {id} failed{} in {secs} s on {}: {}", j.exit.map_or(String::new(), |e| format!(" exit={e}")), j.node, fail_reason(&j, ended_by_exit)),
    };
    let sev = match j.state.as_str() {
        "ok" | "cancelled" | "jit-error" => Sev::Info,
        _ => Sev::Warn,
    };
    event(ctx, sev, what, Some(&j));
}

/// Whether the panic's process still holds it parked; None: no answer, for now. A panic with no id
/// is not the JIT's, and nothing parks on it.
fn holds_parked(j: &Job, p: &JobPanic) -> Option<bool> {
    if p.id.is_empty() || !owned(j, p.pid)? {
        return Some(false);
    }
    let mut parked = false;
    let id = format!("\"id\":{}", json_str(&p.id));
    makepad_remote::hub::ask(&ctl(), &format!("{}:{}", j.node, p.pid), "cmd", "", "panics", &mut |l| {
        parked |= l.contains(&id);
        true
    }).ok()?;
    Some(parked)
}

/// Whether the process is one of the job's, by the node's answer.
fn owned(j: &Job, pid: u32) -> Option<bool> {
    let answer = ask(&ctl(), Some(&j.node), "lease-owns", &format!("lease-owns {} {pid}", j.lease)).ok()?;
    let answer = answer.first()?;
    // (a lease that ended is no answer about its process)
    answer.contains("\"owns\":").then(|| answer.contains("\"owns\":true"))
}

/// A job paused on a panic its process no longer holds parked runs on: a child's expected failure
/// inside a script that carries on is the script's business (J6931's fixture fixed its parked call,
/// and the pause's timer then ended the script two steps later).
fn runs_on(ctx: &Ctx, holds: &dyn Fn(&Job, &JobPanic) -> Option<bool>) {
    // (a job whose box is being released stays paused: running, its ended lease would place it again)
    let paused: Vec<Job> = ctx.store.read(|d| d.jobs.iter().filter(|j| j.state == "paused" && !j.released).cloned().collect());
    for j in paused {
        let Some(p) = j.panic.as_ref().filter(|p| holds(&j, p) == Some(false)) else { continue };
        let Some(now) = set(ctx, &j.id, |j| {
            if j.state == "paused" && !j.released && j.panic.as_ref() == Some(p) {
                j.state = "running".into();
                j.jit_error.clear();
                j.note = format!("{} of pid {} parked, then went on: the job's exit decides", p.id, p.pid);
            }
        }) else { continue };
        if now.state == "running" {
            event(ctx, Sev::Info, format!("job {} runs on: {} of pid {} is no longer parked", j.id, p.id, p.pid), Some(&now));
        }
    }
}

/// Releases the box of a job paused too long with no agent action.
fn idle(ctx: &Ctx) {
    let now = wall_ms();
    let stale: Vec<Job> = ctx.store.read(|d| d.jobs.iter().filter(|j| j.state == "paused" && now.saturating_sub(j.touched_ms) >= PAUSE_IDLE_MS).cloned().collect());
    for j in stale {
        set(ctx, &j.id, |j| {
            j.released = true;
            j.note = format!("released after {} min paused with no agent action", PAUSE_IDLE_MS / 60_000);
        });
        event(ctx, Sev::Warn, format!("job {} paused {} min with no agent action: its box is released", j.id, PAUSE_IDLE_MS / 60_000), Some(&j));
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
    if !matches!(r.kind.as_str(), "panic" | "patchrefused" | "patchreverted") || r.pid == 0 {
        return;
    }
    let candidates: Vec<Job> = ctx.store.read(|d| d.jobs.iter().filter(|j| j.state == "running" && j.node == r.machine).cloned().collect());
    if candidates.is_empty() { return; }
    let (ctx, r) = (ctx.clone(), r.clone());
    // the node says whether the panicking process is one of the job's (never on the record path)
    let _ = std::thread::Builder::new().name("devhub-job-panic".into()).spawn(move || {
        let Some(j) = candidates.into_iter().find(|j| owned(j, r.pid) == Some(true)) else { return };
        if r.kind != "panic" {
            set(&ctx, &j.id, |j| j.jit_error = format!("{}: {}", r.kind, crate::proto::clean(&r.detail, 300)));
            return;
        }
        let d = makepad_strict_json::parse(r.detail.as_bytes()).ok();
        let f = |k: &str| d.as_ref().and_then(|d| d.get(k)).and_then(|x| x.as_str()).map(|s| crate::proto::clean(s, 300)).unwrap_or_default();
        let p = JobPanic { id: f("id"), pid: r.pid, message: f("message"), thread: f("thread"), location: f("location"), at_ms: wall_ms() };
        let Some(j) = set(&ctx, &j.id, |j| {
            if j.state == "running" {
                j.state = "paused".into();
                j.panic = Some(p.clone());
                j.jit_error = format!("JIT panic: {}", p.message);
                j.touched_ms = wall_ms();
            }
        }) else { return };
        if j.state != "paused" {
            return;
        }
        if !j.shadow_of.is_empty() { end_lease(&j.lease); return; }
        let pid = if p.id.is_empty() { "P?".to_string() } else { p.id.clone() };
        event(
            &ctx,
            Sev::Err,
            format!("job {} PAUSED on a panic: {} at {} [{}] ({pid}, pid {}): job inspect {} stacks | job resume {} {pid} retry|restart|unwind | job release {}", j.id, p.message, p.location, p.thread, p.pid, j.id, j.id, j.id),
            Some(&j),
        );
    });
}

/// A job as the CLI sees it: a running one with its latest output (errors, tail) from its log's
/// end, so a view that asks every few seconds never reads a whole log.
fn live(ctx: &Ctx, j: &Job) -> String {
    let mut j = j.clone();
    if !j.done() {
        let log = ctx.logs("jobs").join(format!("{}.log", j.id));
        if let Some(text) = log_end(&log, LIVE_READ) {
            (j.errors, j.tail) = digest(&text);
            j.log = log.to_string_lossy().into_owned();
        }
    }
    crate::state::job_value(&j).to_json()
}

/// A file's last `max` bytes, from the first whole line in them.
fn log_end(path: &std::path::Path, max: u64) -> Option<String> {
    use std::io::{Read, Seek};
    let mut f = std::fs::File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    f.seek(std::io::SeekFrom::Start(len.saturating_sub(max))).ok()?;
    let mut bytes = Vec::new();
    f.take(max).read_to_end(&mut bytes).ok()?;
    let start = if len > max { bytes.iter().position(|b| *b == b'\n').map_or(bytes.len(), |i| i + 1) } else { 0 };
    Some(String::from_utf8_lossy(&bytes[start..]).into_owned())
}

/// A `job wait`'s wake-up: the idle verdict on a waited job's lease, or its yield (a timed run
/// past twice its estimate while a timed ask waits on its box), once a lease. A background wait
/// heard nothing but the job's end: a hung script held a timed box and the queue behind it for 28
/// min while its lane sat in `job wait` and only the inbox was told (11-04). The job runs on: only
/// its owner can tell a wait from a hang.
fn told_idle(ctx: &Ctx, ids: &[String], word: &dyn Fn(&str) -> Option<String>, behind: &dyn Fn(&str) -> Option<(String, String)>) -> Result<String, String> {
    let mut told = Vec::new();
    for id in ids {
        let Some(j) = get(ctx, id).filter(|j| !j.done()) else { continue };
        // waiting for a box: what it waits behind, once per hung lease
        if j.lease.is_empty() {
            let Some((hung, word)) = behind(&j.lane).filter(|(hung, _)| j.idle_told != *hung) else { continue };
            try_set(ctx, id, |j| j.idle_told = hung)?;
            told.push(format!("{{\"id\":{},\"queued\":true,\"word\":{}}}", json_str(id), json_str(&word)));
            continue;
        }
        if j.idle_told == j.lease { continue }
        let Some(word) = word(&j.lease) else { continue };
        try_set(ctx, id, |j| j.idle_told = j.lease.clone())?;
        told.push(format!("{{\"id\":{},\"word\":{}}}", json_str(id), json_str(&word)));
    }
    Ok(format!("[{}]", told.join(",")))
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
        // what a lease's end kept (`job result L<id>`): it outlives the lease, not the prune below
        "lease-log" => {
            if rest.len() < 2 || !rest.starts_with('L') || !rest[1..].bytes().all(|b| b.is_ascii_digit()) {
                return Err("job lease-log L<id>".into());
            }
            let path = ctx.logs(LEASE_LOGS).join(format!("{rest}.log"));
            let bytes = std::fs::read(&path).map_err(|_| format!("{rest} kept no output: a lease released normally keeps none, unless it stalled, a test run or a bucket's test unit failed, or a run ended abnormally"))?;
            Ok(format!("{{\"text\":{},\"path\":{}}}", json_str(&String::from_utf8_lossy(&bytes)), json_str(&path.display().to_string())))
        }
        "output" => {
            use std::io::{Read, Seek};
            let (id, offset) = rest.split_once(' ').ok_or("job output <id> <offset>")?;
            // a negative offset counts from the end: `-16384` is the log's last 16 KB (a panel's
            // tail), and the answer's `off` is where to ask from next
            let off: i64 = offset.parse().map_err(|_| "job output: invalid offset")?;
            let j = get(ctx, id).ok_or_else(|| format!("no job {id}"))?;
            let path = ctx.logs("jobs").join(format!("{id}.log"));
            let mut bytes = Vec::new();
            let mut at = off.max(0) as u64;
            match std::fs::File::open(&path) {
                Ok(mut f) => {
                    if off < 0 {
                        let len = f.metadata().map_err(|e| e.to_string())?.len();
                        at = len.saturating_sub(off.unsigned_abs());
                    }
                    f.seek(std::io::SeekFrom::Start(at)).map_err(|e| e.to_string())?;
                    f.take(64 << 10).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
            Ok(format!("{{\"job\":{},\"data\":{},\"off\":{}}}", crate::state::job_value(&j).to_json(), json_str(&makepad_network::digest::base64_encode(&bytes)), at + bytes.len() as u64))
        }
        // `brief`: the open jobs as the store has them, no log read (a panel asks every few seconds)
        "list" if rest == "brief" => {
            let list: Vec<String> = ctx.store.read(|d| d.jobs.iter().filter(|j| !j.done()).map(|j| crate::state::job_value(j).to_json()).collect());
            Ok(format!("[{}]", list.join(",")))
        }
        "list" => {
            let all = rest == "all";
            let list: Vec<String> = ctx.store.read(|d| d.jobs.iter().filter(|j| all || !j.done()).cloned().collect::<Vec<_>>()).iter().map(|j| live(ctx, j)).collect();
            Ok(format!("[{}]", list.join(",")))
        }
        "waiting" => {
            let want = ids();
            let now = wall_ms();
            ctx.store.update(|d| {
                for j in d.jobs.iter_mut().filter(|j| want.contains(&j.id) && !j.done() && j.waited_ms == 0) { j.waited_ms = now; }
            })?;
            Ok("{}".into())
        }
        "idle" => told_idle(ctx, &ids(), &|lease| ctx.leases.owner_word(lease), &|lane| ctx.leases.behind_hung(lane).or_else(|| ctx.leases.behind_own(lane))),
        "adopt" => {
            let own = super::CTX.get().ok_or("job adopt: this master runs no jobs")?.clone();
            let j = adopt(ctx, rest, &|lease, pid| ctx.leases.adopt(lease, pid))?;
            let (c, followed) = (own.clone(), j.clone());
            if let Err(e) = std::thread::Builder::new().name("devhub-job".into()).spawn(move || reattach(&c, &followed)) {
                let why = format!("job adopt: no thread to follow it: {e}");
                finish(&own, &j.id, Err(why.clone()), std::path::Path::new(&j.log));
                return Err(why);
            }
            Ok(format!("{{\"job\":{}}}", json_str(&j.id)))
        }
        "get" | "wait" => {
            let rest = rest.strip_prefix("--wait").unwrap_or(rest).trim();
            let (any, rest) = rest.strip_prefix("--any").map_or((false, rest), |r| (true, r.trim()));
            let want: Vec<String> = rest.split([' ', ',']).filter(|s| !s.is_empty()).map(str::to_string).collect();
            if verb == "wait" {
                let ids: Vec<&str> = want.iter().map(String::as_str).collect();
                let signal = ctx.store.subscribe("job", &ids);
                let seen = signal.generation();
                let ready = ctx.store.read(|d| {
                    if want.iter().any(|id| !d.jobs.iter().any(|j| &j.id == id)) { return true; }
                    let settled = |id: &String| d.jobs.iter().find(|j| &j.id == id).is_none_or(|j| j.done() || j.state == "paused");
                    if any { want.iter().any(settled) } else { want.iter().all(settled) }
                });
                if !ready { signal.wait(seen, Duration::from_secs(30)); }
            }
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
                let j = try_set(ctx, &id, |j| {
                    if verb == "cancel" || !paused {
                        if j.state == "queued" {
                            j.ended_ms = wall_ms();
                        }
                        j.state = "cancelled".into();
                    } else {
                        j.released = true;
                        j.note = "released by the agent".into();
                    }
                })?
                .unwrap_or(j);
                end_lease(&j.lease);
                if j.state == "cancelled" {
                    event(ctx, Sev::Info, format!("job {id} cancelled"), Some(&j));
                }
                out.push(format!("{{\"id\":{},\"state\":{}}}", json_str(&id), json_str(&j.state)));
            }
            Ok(format!("[{}]", out.join(",")))
        }
        "bump" => {
            // the manager unsticks a waiting job (or a lease by its id): the front of its queue
            // class, and its later asks keep that place
            let mut out = Vec::new();
            for id in ids() {
                if id.starts_with('L') {
                    let lease = ctx.leases.bump(Some(&id), &|_| false)?;
                    out.push(format!("{{\"lease\":{}}}", json_str(&lease)));
                    continue;
                }
                let j = get(ctx, &id).ok_or_else(|| format!("no job {id}"))?;
                if !matches!(j.state.as_str(), "queued" | "leasing") { return Err(format!("job {id} is {}, not waiting", j.state)); }
                let j = try_set(ctx, &id, |j| j.prio = crate::lease::sched::BUMP_PRIO)?.unwrap_or(j);
                let lane = if j.shadow_of.is_empty() { j.lane.clone() } else { format!("shadow-{}", j.id) };
                let lease = ctx.leases.bump(None, &|r| r.lane == lane && r.since_ms == Some(j.asked_ms)).unwrap_or_default();
                event(ctx, Sev::Warn, format!("job {id} bumped to the front of the queue{}", if lease.is_empty() { String::new() } else { format!(" ({lease})") }), Some(&j));
                out.push(format!("{{\"id\":{},\"state\":{},\"lease\":{}}}", json_str(&id), json_str(&j.state), json_str(&lease)));
            }
            Ok(format!("[{}]", out.join(",")))
        }
        "rerun" => rerun(ctx, rest),
        "inspect" => {
            let (id, cmd) = rest.split_once(' ').ok_or("job inspect <id> <inspect command>")?;
            let j = get(ctx, id).ok_or_else(|| format!("no job {id}"))?;
            let p = j.panic.as_ref().filter(|_| j.state == "paused").ok_or_else(|| format!("job {id} is {}, not paused on a panic", j.state))?;
            try_set(ctx, id, |j| j.touched_ms = wall_ms())?;
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
                try_set(ctx, id, |j| j.touched_ms = wall_ms())?;
                return Err(format!("resume: {e}"));
            }
            let j = try_set(ctx, id, |j| {
                j.state = "running".into();
                j.note = format!("resumed {pid} {how}");
            })?;
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
    ctx.monitor.settle(tree)?;
    let d = ctx.monitor.dirty(tree)?;
    let mut rev = format!("{}@{}", label(&d.branch, lane), d.head);
    let (mut changed, mut deleted) = (d.changed, d.deleted);
    for name in crate::nested::names(tree) {
        let (c, prefix) = (ctx.monitor.dirty(&tree.join(crate::nested::dir(&name)))?, crate::nested::prefix(&name));
        rev = format!("{rev},{name}={}@{}", label(&c.branch, lane), c.head);
        changed.extend(c.changed.into_iter().map(|p| format!("{prefix}{p}")));
        deleted.extend(c.deleted.into_iter().map(|p| format!("{prefix}{p}")));
    }
    changed.retain(|p| !crate::lease::ship::never_ships(p));
    deleted.retain(|p| !crate::lease::ship::never_ships(p));
    Ok((rev, changed, deleted))
}

pub(super) fn copy_snapshot(ctx: &Ctx, tree: &std::path::Path, rev: String, changed: Vec<String>, deleted: Vec<String>) -> Result<(String, String, u32), String> {
    let snap = ctx.store.update(|d| {
        d.snaps_next += 1;
        format!("S{}", d.snaps_next)
    })?;
    let dir = snap_dir(ctx, &snap);
    let files = dir.join("files");
    std::fs::create_dir_all(&files).map_err(|e| e.to_string())?;
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
    let pin = crate::lease::Pin::of_rev(&rev).ok_or("snapshot: no pin")?;
    let bases = crate::lease::ship::bases_json(tree, &pin, &changed.iter().chain(&deleted).cloned().collect::<Vec<_>>())?;
    let manifest = format!("{{\"bases\":{bases},\"rev\":{},\"tree\":{},\"changed\":[{}],\"deleted\":[{}]}}", json_str(&rev), json_str(&tree.to_string_lossy()), man.join(","), deleted.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","));
    std::fs::write(dir.join("manifest.json"), manifest).map_err(|e| e.to_string())?;
    Ok((rev, snap, (changed.len() + deleted.len()) as u32))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// A job command that is a devhub command (`makepad devhub test <lane> ..`, past an `env` prefix,
/// through `cargo run .. --bin makepad --`, or the devhub binary itself): its direct form. It
/// would ask the master for a box of its own from inside the job's, on a box with no makepad
/// checkout above it (J8486 failed so in 1 s, 11-06).
fn nested_devhub(cmd: &[String]) -> Option<String> {
    let command = crate::lease::loaders::command_of(cmd);
    let program = std::path::Path::new(command.first()?).file_name()?.to_str()?;
    let args = match program {
        "makepad-devhub" => return Some(crate::proto::clean(&format!("makepad devhub {}", command[1..].join(" ")), 300)),
        "makepad" => &command[1..],
        _ => crate::lease::loaders::cargo_run_args(cmd)?,
    };
    (args.first()? == "devhub").then(|| crate::proto::clean(&format!("makepad {}", args.join(" ")), 300))
}

/// The fields of a submitted job this master reads ([`submit`]): one it does not know is refused,
/// never dropped (a job's `--jit` was dropped on its way and the job ran without its JIT, 11-07).
const JOB_FIELDS: &[&str] = &["after", "agent", "class", "cmd", "cores", "cost", "cwd", "est_min", "holder", "kind", "lane", "lane_from", "loader", "native", "needs", "node", "prio", "priority", "ram_gb", "series", "shadow", "tree", "whole"];

/// Why a job that asks for no JIT (`--jit`) is refused: its command, or a file it names (the
/// script it runs, in its tree), names a binary of a box's JIT store
/// ([`crate::lease::jitstore::BINARY_PREFIX`]). A store keeps a few JITs and evicts the rest, so
/// such a run found its binary gone and failed in 0 s ("no JIT binary": three of a lane's timed
/// jobs, 11-07). `--jit <id>` places it on a box that holds that JIT and holds it for the lease,
/// at `$MAKEPAD_LOADER`.
fn reads_jit_store(dir: &std::path::Path, cmd: &[String]) -> Option<String> {
    let prefix = crate::lease::jitstore::BINARY_PREFIX;
    let named = |text: &str| text.contains(prefix);
    let file = |w: &str| {
        let rel = std::path::Path::new(w);
        if w.starts_with('-') || rel.is_absolute() || rel.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
            return None;
        }
        let path = dir.join(rel);
        std::fs::metadata(&path).ok().filter(|m| m.is_file() && m.len() <= 1 << 20)?;
        std::fs::read(&path).ok().filter(|b| named(&String::from_utf8_lossy(b))).map(|_| w.to_string())
    };
    let by = cmd.iter().find(|w| named(w)).map(|_| "its command".to_string())
        .or_else(|| cmd.iter().find_map(|w| file(w)).map(|f| format!("its script {f}")))?;
    let key = cmd.iter().find(|w| w.len() == 24 && w.bytes().all(|b| b.is_ascii_hexdigit())).map_or("<id>", String::as_str);
    Some(format!("{by} reads a box's JIT store by hand ({prefix}<id>), and a store evicts all but its newest JITs: ask with `--jit {key}`, which places the job where that JIT is and holds it for the run, and run it as $MAKEPAD_LOADER"))
}

/// Keep submission's capability check and the later lease ask on the same command and cost.
fn configure_request(r: &mut crate::lease::sched::Req, j: &Job) {
    r.run_options(Src::parse(&j.src).unwrap_or(Src::Tree), CostClass::parse(&j.cost), j.est_min, &j.class, &j.cmd);
    r.priority = Priority::parse(&j.priority).unwrap_or_default();
    r.loader = (!j.loader.is_empty()).then(|| j.loader.clone());
    r.cores = j.cores;
    r.ram_gb = j.ram_gb;
    r.whole = j.whole;
    r.holder = j.holder;
    r.series = (!j.series.is_empty()).then(|| j.series.clone());
}

/// `submit {"tree","rev"?,"batch","jobs":[{kind,needs,node,prio,lane,agent,cmd,after}]}`: ids at
/// once. Without `rev` the jobs run at the worktree's snapshot (HEAD + its dirty files), with one
/// at those commits alone; a main checkout's dirty files only with `with_uncommitted`. `after` names jobs (`J3`) or earlier jobs of this request (`@1`).
fn submit(ctx: &Ctx, json: &str) -> Result<String, String> {
    let v = makepad_strict_json::parse_depth(json.as_bytes(), 8).map_err(|_| "job submit: bad request")?;
    let request = v.get("request_id").and_then(Value::as_str).unwrap_or("");
    if let Some(reply) = ctx.store.read(|d| d.submissions.get(request).cloned()) {
        return Ok(reply);
    }
    let reqs = v.get("jobs").and_then(|a| a.as_arr()).filter(|a| !a.is_empty()).ok_or("job submit: no jobs")?;
    let batch = v.get("batch").and_then(|b| b.as_bool()).unwrap_or(false);
    let s = |x: &makepad_strict_json::Value, k: &str| x.get(k).and_then(|y| y.as_str()).unwrap_or("").to_string();
    let list = |x: &makepad_strict_json::Value, k: &str| -> Vec<String> { x.get(k).and_then(|a| a.as_arr()).unwrap_or(&[]).iter().filter_map(|y| y.as_str().map(str::to_string)).collect() };
    let tree = std::path::PathBuf::from(s(&v, "tree"));
    if !tree.is_absolute() || !tree.join(".git").exists() {
        return Err(format!("job submit: `{}` is not a worktree", tree.display()));
    }
    let mut jobs = Vec::new();
    let mut notes = Vec::new();
    // where each job's lane came from (`--lane`, DEVHUB_LANE or the worktree), said as it queues
    let mut lane_from = Vec::new();
    // the commits read for a JIT test's `test-tree`, once per submission: the pinned ones, else
    // the worktree's HEADs
    let mut pair = None;
    for (i, r) in reqs.iter().enumerate() {
        let fields: &[(String, Value)] = match r { Value::Obj(pairs) => pairs, _ => &[] };
        if let Some(field) = fields.iter().map(|(k, _)| k.as_str()).find(|k| !JOB_FIELDS.contains(k)) {
            return Err(format!("job {}: this master predates its field `{field}` and would run the job without it: submit again once the master runs the newest landed devhub", i + 1));
        }
        let kind = s(r, "kind");
        match Kind::parse(&kind) {
            Some(Kind::Inspect) | None => return Err(format!("job {}: kind `{kind}` (build test screen long timed gpu gate)", i + 1)),
            _ => {}
        }
        let cmd = list(r, "cmd");
        if cmd.is_empty() || cmd[0].is_empty() {
            return Err(format!("job {}: no command", i + 1));
        }
        if let Some(direct) = nested_devhub(&cmd) {
            return Err(format!("job {}: its command is itself a devhub command, which asks the master for its own box and has no makepad checkout to run from on the job's box: run it directly instead: `{direct}`", i + 1));
        }
        if s(r, "loader").is_empty() {
            if let Some(why) = reads_jit_store(&tree.join(s(r, "cwd")), &cmd) {
                return Err(format!("job {}: {why}", i + 1));
            }
        }
        let lane = makepad_devhub_conf::lane_name(&s(r, "lane"));
        if !makepad_devhub_conf::valid_name(&lane) {
            return Err(format!("job {}: lane `{lane}` is not a name", i + 1));
        }
        lane_from.push(s(r, "lane_from"));
        let prio = r.get("prio").and_then(|p| p.as_i64()).unwrap_or(crate::lease::sched::DEFAULT_PRIO as i64) as i32;
        let mut src = Src::for_run(Kind::parse(&kind).unwrap(), &cmd, r.get("tree").and_then(|v| v.as_bool()).unwrap_or(false));
        // a JIT test of a package whose tests read the checkout itself (cargo on it, its history)
        // takes the lane's tree, as `devhub test` gives it: an overlay's tree is one only the JIT
        // process reads (a batch's app-run test failed there, 11-08)
        if src == Src::Overlay && crate::jit_misses::tested_package(&cmd).is_some() {
            if pair.is_none() {
                let rev = s(&v, "rev");
                pair = Some(match crate::lease::Pin::of_rev(&rev) { Some(pin) => pin.pair(), None => super::ci::head_pair(&tree).map_err(|e| format!("job submit: {e}"))? });
            }
            if let Some(line) = crate::jit_misses::test_tree_of(&tree, pair.as_ref().unwrap(), &cmd).map_err(|e| format!("job {}: {e}", i + 1))? {
                notes.push(format!("job {}: {line}", i + 1));
                src = Src::Tree;
            }
        }
        let cwd = s(r, "cwd");
        if !cwd.is_empty() && (std::path::Path::new(&cwd).is_absolute() || cwd.contains(char::is_whitespace) || std::path::Path::new(&cwd).components().any(|c| matches!(c, std::path::Component::ParentDir))) { return Err("job cwd must be a relative directory without spaces".into()); }
        let shadow = r.get("shadow").and_then(|v| v.as_bool()).unwrap_or(false) && kind == "test" && crate::lease::loaders::cargo_run_args(&cmd).is_some_and(|a| a.first().is_some_and(|a| a == "test")) && ctx.store.read(|d| d.devhub_shadow);
        let holder = r.get("holder").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok()).filter(|p| *p > 1);
        let cost = s(r, "cost");
        if !cost.is_empty() && CostClass::parse(&cost).is_none() { return Err("cost is small, medium or large".into()); }
        let est_min = r.get("est_min").filter(|v| !v.is_null()).map(|v| crate::lease::num(v).filter(|m| m.is_finite() && *m >= 0.0).ok_or("est_min is a nonnegative number")).transpose()?;
        let cores = r.get("cores").filter(|v| !v.is_null()).map(|v| crate::lease::num(v).filter(|n| n.is_finite() && *n > 0.0).ok_or("cores is a positive number")).transpose()?;
        let ram_gb = r.get("ram_gb").filter(|v| !v.is_null()).map(|v| crate::lease::num(v).filter(|n| n.is_finite() && *n > 0.0).ok_or("ram_gb is a positive number")).transpose()?;
        let whole = r.get("whole").and_then(|v| v.as_bool()).unwrap_or(false);
        let priority = match s(r, "priority").as_str() { "" => Priority::Background, p => Priority::parse(p).ok_or("priority is interactive or background")? };
        let (agent, rev, reason) = (s(r, "agent"), s(&v, "rev"), s(r, "native"));
        let (os, arch) = crate::lease::native::platform(&crate::lease::sched::parse_needs(&s(r, "needs")), &s(r, "node"), &ctx.leases.nodes());
        let gate = crate::lease::native::Ask { lane: &lane, agent: &agent, kind: Kind::parse(&kind).unwrap(), cmd: &cmd, repo_dir: &tree, cwd: if cwd.is_empty() { "." } else { &cwd }, rev: &rev, reason: &reason, class: &s(r, "class"), route: &|class| { let (jit, why) = ctx.leases.test_route("test", class, false); (!jit).then_some(why) }, os: &os, arch: &arch };
        let note = crate::lease::native::check(&ctx.home, &gate, wall_ms()).map_err(|e| format!("job {}: {e}", i + 1))?;
        if !note.is_empty() { notes.push(note); }
        jobs.push(Job { cwd, holder, shadow, priority: priority.as_str().into(), loader: s(r, "loader"), src: src.as_str().into(), cost: cost.as_str().into(), est_min, cores, ram_gb, whole, class: s(r, "class"), kind, needs: s(r, "needs"), node_pref: s(r, "node"), series: s(r, "series"), prio, repo_dir: tree.to_string_lossy().into_owned(), lane, agent: s(r, "agent"), cmd, after: list(r, "after"), state: "queued".into(), ..Default::default() });
    }
    for (i, j) in jobs.iter().enumerate() {
        let mut req = crate::lease::sched::Req::new(&j.lane, Kind::parse(&j.kind).unwrap());
        req.needs = crate::lease::sched::parse_needs(&j.needs);
        req.node = (!j.node_pref.is_empty()).then(|| j.node_pref.clone());
        req.master = true;
        configure_request(&mut req, j);
        req.check_series().map_err(|e| format!("job {}: {e}", i + 1))?;
        req.normalize_kind();
        if let Some(never) = ctx.leases.never_places(&req) { return Err(format!("job {}: no box could ever take it: {never}", i + 1)); }
    }
    // the base and the snapshot: one for the whole request
    let lane = jobs[0].lane.clone();
    let main_checkout = crate::lease::ship::is_main_checkout(&tree);
    let mut left_out = 0;
    let (rev, snap, files) = match s(&v, "rev") {
        r if r.is_empty() && !list(&v, "also").is_empty() => {
            let (rev, mut changed, mut deleted) = dirty_set(ctx, &tree, &lane)?;
            if main_checkout && !v.get("with_uncommitted").and_then(Value::as_bool).unwrap_or(false) { left_out = changed.len() + deleted.len(); changed.clear(); deleted.clear(); }
            for p in list(&v, "also") {
                if !crate::lease::ship::inside(&p) { return Err(format!("DEVHUB_ALSO needs a repository-relative path: {p}")); }
                if crate::lease::ship::never_ships_named(&p) { return Err(format!("DEVHUB_ALSO path never ships: {p}")); }
                let mut entries = Vec::new();
                crate::lease::ship::entries(&tree, &p, &mut entries)?;
                changed.extend(entries.into_iter().map(|e| e.path).filter(|p| !crate::lease::ship::never_ships_named(p)));
            }
            changed.sort(); changed.dedup();
            copy_snapshot(ctx, &tree, rev, changed, deleted)?
        }
        r if r.is_empty() && main_checkout && !v.get("with_uncommitted").and_then(|b| b.as_bool()).unwrap_or(false) => {
            let (rev, changed, deleted) = dirty_set(ctx, &tree, &lane)?;
            left_out = changed.len() + deleted.len();
            (rev, String::new(), 0)
        }
        r if r.is_empty() => snapshot(ctx, &tree, &lane)?,
        r => (crate::lease::client::pin_lane_rev(&tree, &r, &lane)?, String::new(), 0),
    };
    let (changed, deleted) = if snap.is_empty() { Default::default() } else { snap_paths(ctx, &snap)? };
    let list = |v: &[String]| v.iter().map(|p| json_str(p)).collect::<Vec<_>>().join(",");
    let made = ctx.store.try_update(|d| -> Result<(String, Vec<Job>), String> {
        if let Some(reply) = d.submissions.get(request) {
            return Ok((reply.clone(), Vec::new()));
        }
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
            j.asked_ms = 0;
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
        let ids: Vec<String> = out.iter().map(|j| json_str(&j.id)).collect();
        let reply = format!("{{\"batch\":{},\"jobs\":[{}],\"rev\":{},\"snap\":{},\"files\":{files},\"tree\":{},\"main_checkout\":{main_checkout},\"left_out\":{left_out},\"changed\":[{}],\"deleted\":[{}],\"notes\":[{}]}}", json_str(&batch_id), ids.join(","), json_str(&rev), json_str(&snap), json_str(&tree.to_string_lossy()), list(&changed), list(&deleted), list(&notes));
        if !request.is_empty() { d.submissions.insert(request.to_string(), reply.clone()); }
        Ok((reply, out))
    })?;
    let (reply, out) = made;
    if out.is_empty() { return Ok(reply); }
    // what ships on top of the commits, named: the worktree's uncommitted files go to a box
    if main_checkout && !snap.is_empty() {
        event(ctx, Sev::Warn, format!("job {}: {snap} ships {files} uncommitted file(s) of the main checkout {} (--with-uncommitted)", out[0].id, tree.display()), Some(&out[0]));
    }
    for (j, from) in out.iter().zip(&lane_from) {
        let after = if j.after.is_empty() { String::new() } else { format!(" after {}", j.after.join(",")) };
        event(ctx, Sev::Info, format!("job {} queued{}: {} {}{after} ({}, {})", j.id, if j.batch.is_empty() { String::new() } else { format!(" in {}", j.batch) }, j.kind, crate::proto::clean(&spec_cmd(j), 160), lane_said(&j.lane, from, &j.agent), if j.snap.is_empty() { "commits only".to_string() } else { format!("{} with {} file(s)", j.snap, j.snap_files) }), Some(j));
    }
    Ok(reply)
}

/// A queued job's lane as its line says it: where the lane came from and whose job it is, so a
/// job filed under another lane's cap shows why.
fn lane_said(lane: &str, from: &str, agent: &str) -> String {
    let from = if from.is_empty() { String::new() } else { format!(" from {from}") };
    let by = if agent.is_empty() { String::new() } else { format!(", by {agent}") };
    format!("lane {lane}{from}{by}")
}

/// `rerun <id>`: the job again, exactly: its base, snapshot, command and placement wishes.
fn rerun(ctx: &Ctx, id: &str) -> Result<String, String> {
    let j = get(ctx, id).ok_or_else(|| format!("no job {id}"))?;
    if !j.snap.is_empty() && !snap_dir(ctx, &j.snap).join("manifest.json").exists() {
        return Err(format!("job {id}: its snapshot {} is gone", j.snap));
    }
    let n = ctx.store.update(|d| {
        d.jobs_next += 1;
        let n = Job { id: format!("J{}", d.jobs_next), cwd: j.cwd.clone(), priority: j.priority.clone(), loader: j.loader.clone(), src: j.src.clone(), cost: j.cost.clone(), est_min: j.est_min, cores: j.cores, ram_gb: j.ram_gb, whole: j.whole, class: j.class.clone(), kind: j.kind.clone(), needs: j.needs.clone(), node_pref: j.node_pref.clone(), series: j.series.clone(), prio: j.prio, rev: j.rev.clone(), repo_dir: j.repo_dir.clone(), snap: j.snap.clone(), snap_files: j.snap_files, lane: j.lane.clone(), agent: j.agent.clone(), cmd: j.cmd.clone(), state: "queued".into(), submitted_ms: wall_ms(), note: format!("rerun of {id}"), ..Default::default() };
        d.jobs.push(n.clone());
        n
    })?;
    event(ctx, Sev::Info, format!("job {} queued: rerun of {id}", n.id), Some(&n));
    Ok(format!("{{\"jobs\":[{}],\"rerun_of\":{}}}", json_str(&n.id), json_str(id)))
}

/// One job, answer first: `J12 ok exit=0 34s test@node-a L7300: cargo test ...`, then its error
/// lines (and the tail with `tail`), or the panic and what to do about it.
pub fn render(j: &Job, tail: bool) -> String {
    let secs = match (j.started_ms, j.ended_ms) {
        (s, e) if s > 0 && e >= s => format!(" {}s", (e - s) / 1000),
        (s, 0) if s > 0 => format!(" {}s so far", wall_ms().saturating_sub(s) / 1000),
        _ => String::new(),
    };
    let at = if j.node.is_empty() { String::new() } else { format!(" {}@{} {}", j.kind, j.node, j.lease) };
    let exit = j.exit.map_or(String::new(), |e| format!(" exit={e}"));
    // its lane: `job list` has every lane's open jobs (another lane's job was read as one's own
    // and as what held one's lane cap, 11-06)
    let lane = if j.lane.is_empty() { String::new() } else { format!(" @{}", j.lane) };
    let mut out = format!("{} {}{exit}{secs}{at}{lane}: {}\n", j.id, j.state, crate::proto::clean(&spec_cmd(j), 160));
    if !j.unquiet.is_empty() {
        out.push_str(&format!("  NOT QUIET box {}: its timings are not clean, run it again on a quiet box\n", j.unquiet));
    }
    if let Some(p) = &j.panic {
        out.push_str(&format!("  panic {}: {} at {} [{}] pid {} ({} ago)\n", p.id, p.message, p.location, p.thread, p.pid, age(p.at_ms)));
        if j.state == "paused" {
            out.push_str(&format!("  -> devhub job inspect {0} stacks | job resume {0} {1} retry|restart|unwind | job release {0}\n", j.id, if p.id.is_empty() { "P?" } else { &p.id }));
        }
    }
    if !j.note.is_empty() {
        out.push_str(&format!("  note: {}\n", j.note));
    }
    if !j.source.is_empty() {
        out.push_str(&format!("  source {} ({})\n", j.source, if j.snap.is_empty() { "the commits alone".to_string() } else { format!("snapshot {}, {} file(s) on top", j.snap, j.snap_files) }));
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
    if !j.log.is_empty() && !matches!(j.state.as_str(), "ok" | "queued" | "leasing" | "running") {
        out.push_str(&format!("  whole log: {}\n", j.log));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lease::master::LeaseMaster;

    /// A job that reads a box's JIT store by hand and asks for no JIT is refused at the ask, said
    /// with what to ask instead: the store had evicted the binary, and it failed in 0 s (11-07).
    /// A job field the master does not read is refused at submit, never dropped (the job would
    /// run without it), and so is a script reading the JIT store by hand without `--jit`.
    #[test]
    fn a_job_field_the_master_does_not_know_is_refused() {
        let c = ctx("unknown-field");
        let with = |field: &str, job: String| job.replacen('{', &format!("{{\"{field}\":\"0123456789abcdef01234567\","), 1);
        let e = submit_line(&c, "", false, &[with("jit", req("check", &[]))]).unwrap_err();
        assert!(e.starts_with("job 1: this master predates its field `jit`"), "{e}");
        std::fs::write(c.repo.join("by_hand.sh"), "\"$HOME/devhub/dev/jit/makepad.jit-$1\" run app\n").unwrap();
        let script = req("bash", &[]).replace("\"cmd\":[\"bash\"]", "\"cmd\":[\"bash\",\"by_hand.sh\",\"0123456789abcdef01234567\"]");
        let e = submit_line(&c, "", false, std::slice::from_ref(&script)).unwrap_err();
        assert!(e.starts_with("job 1: its script by_hand.sh reads a box's JIT store by hand"), "{e}");
        assert!(submit_line(&c, "", false, &[with("loader", script)]).is_ok(), "with --jit it runs that JIT");
        std::fs::remove_dir_all(&c.home).unwrap();
    }

    #[test]
    fn a_job_reading_the_jit_store_by_hand_is_refused_with_its_jit_to_ask_for() {
        let dir = std::env::temp_dir().join(format!("devhub-jit-by-hand-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zz")).unwrap();
        std::fs::write(dir.join("zz/by_hand.sh"), "KEY=$1\nJ=\"$HOME/devhub/dev/jit/makepad.jit-$KEY\"\n\"$J\" run app\n").unwrap();
        std::fs::write(dir.join("zz/loader.sh"), "\"$MAKEPAD_LOADER\" run app\n").unwrap();
        let cmd = |words: &[&str]| words.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        let key = "0123456789abcdef01234567";
        let why = reads_jit_store(&dir, &cmd(&["bash", "zz/by_hand.sh", key, "2"])).expect("refused");
        assert!(why.starts_with("its script zz/by_hand.sh reads a box's JIT store by hand") && why.contains(&format!("`--jit {key}`")) && why.contains("$MAKEPAD_LOADER"), "{why}");
        let why = reads_jit_store(&dir, &cmd(&["env", "A=1", "/home/x/devhub/dev/jit/makepad.jit-abc", "run"])).expect("refused");
        assert!(why.starts_with("its command reads") && why.contains("`--jit <id>`"), "{why}");
        assert_eq!(reads_jit_store(&dir, &cmd(&["bash", "zz/loader.sh", key])), None, "$MAKEPAD_LOADER is the JIT it asked for");
        assert_eq!(reads_jit_store(&dir, &cmd(&["bash", "../zz/by_hand.sh"])), None, "only files in its tree");
        assert_eq!(reads_jit_store(&dir, &cmd(&["cargo", "test", "-p", "x"])), None);
        let _ = std::fs::remove_dir_all(&dir);
    }
    use crate::state::Store;

    #[test]
    fn a_queued_jobs_line_says_where_its_lane_came_from_and_whose_it_is() {
        assert_eq!(lane_said("lanea", "the worktree /w/lanea", "LANEA"), "lane lanea from the worktree /w/lanea, by LANEA");
        assert_eq!(lane_said("laneb", "DEVHUB_LANE", ""), "lane laneb from DEVHUB_LANE");
        // (a request from a CLI that does not say)
        assert_eq!(lane_said("lanec", "", ""), "lane lanec");
    }

    #[test]
    fn submit_refuses_a_nested_devhub_command_with_its_direct_form() {
        let c = ctx("nested-devhub");
        let words = |w: &str| w.split_whitespace().map(str::to_string).collect::<Vec<_>>();
        for (cmd, direct) in [
            ("makepad devhub test lane-a pkg --lib", Some("makepad devhub test lane-a pkg --lib")),
            ("/some/where/target/host/makepad devhub run lane-a build -- true", Some("makepad devhub run lane-a build -- true")),
            ("env A=1 makepad devhub test lane-a pkg", Some("makepad devhub test lane-a pkg")),
            ("cargo run --profile host -q -p makepad-run --bin makepad -- devhub test lane-a pkg", Some("makepad devhub test lane-a pkg")),
            ("makepad-devhub lane audit", Some("makepad devhub lane audit")),
            ("makepad test pkg --lib", None),
            ("cargo test -p makepad-devhub", None),
        ] {
            assert_eq!(nested_devhub(&words(cmd)).as_deref(), direct, "{cmd}");
        }
        let nested = r#"{"kind":"test","lane":"lane1","agent":"a","cmd":["makepad","devhub","test","lane1","pkg"],"after":[]}"#.to_string();
        let e = submit_line(&c, "", true, &[nested]).unwrap_err();
        assert!(e.starts_with("job 1: its command is itself a devhub command") && e.ends_with("`makepad devhub test lane1 pkg`"), "{e}");
        assert_eq!(c.store.read(|d| d.jobs.len()), 0, "nothing queued");
        std::fs::remove_dir_all(&c.home).unwrap();
    }

    #[test]
    fn submit_refuses_impossible_argv_before_writing_any_job() {
        let mut c = ctx("exec-size");
        c.leases = Arc::new(LeaseMaster::new("node-a", c.home.clone(), Arc::new(|_: &str, service: &str, _: &str| {
            if service != "caps" { return Ok(vec!["{}".into()]); }
            Ok(vec![crate::lease::Caps {
                arch: "x86_64".into(), os: "linux".into(), cores: 16, ram_gb: 64.0,
                exec_limits: Some(crate::os::ExecLimits { total: 2 * 1024 * 1024, per_string: Some(128 * 1024) }),
                ..Default::default()
            }.to_json()])
        }), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        c.leases.refresh(&[("node-a".into(), "192.0.2.1".into()), ("node-b".into(), "192.0.2.2".into())]);
        let word = format!("X={}", "x".repeat(200 * 1024));
        let bad = req("env", &["@1"]).replace("\"cmd\":[\"env\"]", &format!("\"cmd\":[\"env\",{},\"true\"]", json_str(&word)));
        let error = submit_line(&c, "", true, &[req("true", &[]), bad]).unwrap_err();
        assert!(error.contains("Linux execs at most 128 KiB"), "{error}");
        assert!(error.contains("pass it in a file the run reads"), "{error}");
        assert_eq!(c.store.read(|d| (d.jobs.len(), d.jobs_next, d.batches_next)), (0, 0, 0), "the whole batch is refused before any row or id is written");
        assert!(c.leases.view().is_empty(), "no lease queued, even for an --after job");
        assert!(!c.home.join("jobs").exists(), "no job output created");
        std::fs::remove_dir_all(&c.home).unwrap();
    }

    /// A job no box could ever take is refused at submission with its lease's denial: one pinned
    /// to a Mac without os=macos waited its lane's turn, then was denied (11-06).
    #[test]
    fn submit_refuses_a_job_no_box_could_ever_take() {
        let mut c = ctx("never-places");
        c.leases = Arc::new(LeaseMaster::new("node-a", c.home.clone(), Arc::new(|node: &str, service: &str, _: &str| {
            if service != "caps" { return Ok(vec!["{}".into()]); }
            let os = if node == "node-m" { "macos" } else { "linux" };
            Ok(vec![crate::lease::Caps { arch: "x86_64".into(), os: os.into(), cores: 16, ram_gb: 64.0, timed: true, ..Default::default() }.to_json()])
        }), Arc::new(|_| {}), Arc::new(|| 1), None, true));
        c.leases.refresh(&[("node-a".into(), "192.0.2.1".into()), ("node-m".into(), "192.0.2.2".into())]);
        let pinned = req("true", &[]).replace("\"cmd\":", "\"node\":\"node-m\",\"cmd\":");
        let error = submit_line(&c, "", true, &[pinned]).unwrap_err();
        assert!(error.contains("no box could ever take it: a Mac: the Macs take only work that requires macOS"), "{error}");
        assert_eq!(c.store.read(|d| (d.jobs.len(), d.jobs_next)), (0, 0), "nothing is written");
        assert!(submit_line(&c, "", true, &[req("true", &[])]).is_ok(), "unpinned, a Linux box takes it");
        std::fs::remove_dir_all(&c.home).unwrap();
    }

    #[test]
    fn an_idle_lease_wakes_its_jobs_wait_once() {
        let c = ctx("idle");
        c.store.update(|d| d.jobs.push(Job { id: "J1".into(), state: "running".into(), lease: "L1".into(), ..Default::default() })).unwrap();
        let ids = ["J1".to_string()];
        let no = |_: &str| None;
        assert_eq!(told_idle(&c, &ids, &|_| None, &no).unwrap(), "[]", "it works");
        let word = |lease: &str| Some(format!("{lease} idle"));
        assert_eq!(told_idle(&c, &ids, &word, &no).unwrap(), r#"[{"id":"J1","word":"L1 idle"}]"#);
        assert_eq!(told_idle(&c, &ids, &word, &no).unwrap(), "[]", "the next wait sits it out");
        set(&c, "J1", |j| j.lease = "L2".into());
        assert_eq!(told_idle(&c, &ids, &word, &no).unwrap(), r#"[{"id":"J1","word":"L2 idle"}]"#, "a new box is a new verdict");
        set(&c, "J1", |j| { j.lease = "L3".into(); j.state = "ok".into(); });
        assert_eq!(told_idle(&c, &ids, &word, &no).unwrap(), "[]", "an ended job's wait returns its result");
        // queued behind a hung lease of its lane: its wait wakes too, once per hung lease
        set(&c, "J1", |j| { j.lease.clear(); j.state = "leasing".into(); });
        let behind = |_: &str| Some(("L9".to_string(), "L10 waits behind L9".to_string()));
        assert_eq!(told_idle(&c, &ids, &word, &behind).unwrap(), r#"[{"id":"J1","queued":true,"word":"L10 waits behind L9"}]"#);
        assert_eq!(told_idle(&c, &ids, &word, &behind).unwrap(), "[]");
        let _ = std::fs::remove_dir_all(&c.home);
    }

    /// A run's client told its lease's idle verdict hands the run to a job: from its holder only,
    /// told once, never placed again (it would run without the client's tree), and what its lease
    /// kept of the stall goes at the end of the job's log after a cancel, its result read from the
    /// run's own output.
    #[test]
    fn a_hung_runs_client_hands_it_to_a_job_that_keeps_its_stall() {
        let c = ctx("adopt");
        let body = r#"{"lease":"L7","pid":42,"run":2,"lane":"lane1","agent":"a","kind":"test","cmd":["cargo","test"],"cwd":".","rev":"rc-t@abc","class":"native:test:x"}"#;
        let refused = |_: &str, _: u32| Err::<(String, u64), String>("L7 is not held by pid 42".into());
        assert_eq!(adopt(&c, body, &refused).unwrap_err(), "L7 is not held by pid 42");
        assert!(adopt(&c, &body.replace("\"pid\":42,", ""), &|_, _| Ok(("node-a".into(), 5_000))).is_err(), "no holder named");
        assert_eq!(c.store.read(|d| d.jobs.len()), 0, "a refused hand-over writes no job");
        let taken = |lease: &str, pid: u32| if (lease, pid) == ("L7", 42) { Ok(("node-a".to_string(), 5_000)) } else { Err("wrong lease".to_string()) };
        let j = adopt(&c, body, &taken).unwrap();
        assert_eq!((j.state.as_str(), j.lease.as_str(), j.node.as_str(), j.run_n, j.started_ms, j.cmd.join(" ")), ("running", "L7", "node-a", 2, 5_000, "cargo test".to_string()));
        assert_eq!(told_idle(&c, std::slice::from_ref(&j.id), &|lease| Some(format!("{lease} idle")), &|_| None).unwrap(), "[]", "its client said the verdict");
        assert!(!again(&c, &j.id, "box node-a lost"), "never placed again");
        std::fs::create_dir_all(c.logs("jobs")).unwrap();
        std::fs::create_dir_all(c.logs(LEASE_LOGS)).unwrap();
        let log = std::path::PathBuf::from(&j.log);
        std::fs::write(&log, "running 1 test\ntest a ...\n").unwrap();
        std::fs::write(c.logs(LEASE_LOGS).join("L7.log"), "== node-a: its processes and last lines when it stalled (L7 test@node-a of lane1: every process idle for 10m00s)\npid 7: sleep 3600\n").unwrap();
        finish(&c, &j.id, Err("L7 ended (cancelled): cancelled by user".into()), &log);
        let done = get(&c, &j.id).unwrap();
        assert_eq!(done.tail.last().map(String::as_str), Some("test a ..."), "its result reads the run's output");
        assert!(std::fs::read_to_string(&log).unwrap().ends_with("== what L7 kept when it stalled (`devhub job result L7`)\n== node-a: its processes and last lines when it stalled (L7 test@node-a of lane1: every process idle for 10m00s)\npid 7: sleep 3600\n"));
        let _ = std::fs::remove_dir_all(&c.home);
    }

    /// A lease someone ended ends its job, granted or still asked: a `devhub release L..` of a
    /// running job's lease placed it again, on a box with dev off, where it waited unseen (J8832,
    /// 11-06). A box lost under it still places it again.
    #[test]
    fn a_lease_ended_by_hand_ends_its_job() {
        let c = ctx("byhand");
        c.store.update(|d| for id in ["J1", "J2", "J3"] { d.jobs.push(Job { id: id.into(), state: "running".into(), lease: "L1".into(), node: "box-a".into(), attempts: 1, ..Default::default() }) }).unwrap();
        let log = c.home.join("J.log");
        for (id, e) in [("J1", "L1 ended (released)"), ("J2", "L2 ended (cancelled): cancelled by user")] {
            assert!(ended_by_hand(&c, id, e));
            finish(&c, id, Err(e.into()), &log);
            let j = get(&c, id).unwrap();
            assert_eq!((j.state.as_str(), j.note.as_str()), ("cancelled", e));
            assert!(!again(&c, id, "box box-a lost"), "an ended job stays ended");
        }
        assert!(!ended_by_hand(&c, "J3", "L1 ended (expired)"));
        assert!(again(&c, "J3", "box box-a lost (L1 ended (expired))"));
        assert_eq!(get(&c, "J3").unwrap().state, "queued");
        let _ = std::fs::remove_dir_all(&c.home);
    }

    #[test]
    fn a_failed_job_says_why_from_its_output() {
        let (errors, tail) = digest("bash: zz_d/cpuinfo.sh: No such file or directory\n");
        let j = Job { errors, tail, note: "placed on box-a".into(), ..Default::default() };
        assert_eq!(fail_reason(&j, true), "bash: zz_d/cpuinfo.sh: No such file or directory");
        assert_eq!(fail_reason(&j, false), "placed on box-a", "a run its box lost: what ended it");
        let (errors, tail) = digest("compiling\nerror[E0425]: cannot find value `x`\nwarning: 1 warning\n");
        assert_eq!(fail_reason(&Job { errors, tail, ..Default::default() }, true), "error[E0425]: cannot find value `x`");
    }

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
        Ctx { store: Store::open(&home, "mac", Arc::new(|_| 0)).expect("workflow state must open before services start"), leases, machine: "mac".into(), monitor: crate::monitor::Monitor::start(&[]), repo: home.join("tree"), home }
    }

    fn req(cmd: &str, after: &[&str]) -> String {
        let after: Vec<String> = after.iter().map(|a| json_str(a)).collect();
        format!("{{\"kind\":\"test\",\"needs\":\"arch=x64\",\"lane\":\"lane1\",\"agent\":\"a\",\"cmd\":[{}],\"after\":[{}]}}", json_str(cmd), after.join(","))
    }

    fn submit_line(c: &Ctx, rev: &str, batch: bool, jobs: &[String]) -> Result<String, String> {
        serve(c, &format!("job submit {{\"tree\":{},\"rev\":{},\"batch\":{batch},\"with_uncommitted\":true,\"jobs\":[{}]}}", json_str(&c.repo.to_string_lossy()), json_str(rev), jobs.join(",")))
    }

    #[test]
    fn safety_retried_submission_keeps_its_original_receipt_after_reload() {
        let mut c = ctx("retry");
        let line = format!("job submit {{\"request_id\":\"submission-one\",\"tree\":{},\"jobs\":[{}]}}", json_str(&c.repo.to_string_lossy()), req("check", &[]));
        let mut lost = false;
        let first = crate::cli::retry_master_wait(|| {
            let reply = serve(&c, &line)?;
            if !lost {
                lost = true;
                c.store = Store::open(&c.home, "test", Arc::new(|_| 0)).unwrap();
                return Err("link lost after accepting submission".into());
            }
            Ok(reply)
        }, || true).unwrap();
        assert_eq!(serve(&c, &line).unwrap(), first);
        assert_eq!(c.store.read(|d| d.jobs.len()), 1, "lost reply must not create another job");
        assert_ne!(serve(&c, &line.replace("submission-one", "submission-two")).unwrap(), first);
        assert_eq!(c.store.read(|d| d.jobs.len()), 2, "intentional new submission still runs");
        // A failed durable write must not accept a job that can be duplicated after restart
        // (both writes refused: the checkpoint's temporary and the journal are directories).
        std::fs::create_dir(c.home.join("state.new")).unwrap();
        let _ = std::fs::remove_file(c.home.join("state.journal"));
        std::fs::create_dir(c.home.join("state.journal")).unwrap();
        assert!(serve(&c, &line.replace("submission-one", "submission-three")).is_err());
        assert_eq!(c.store.read(|d| d.jobs.len()), 2);
        std::fs::remove_dir_all(&c.home).unwrap();
    }

    /// `job list brief` lists the open jobs as stored; only the full list reads their logs.
    #[test]
    fn a_brief_list_reads_no_log() {
        let c = ctx("brief");
        serve(&c, &format!("job submit {{\"tree\":{},\"rev\":\"\",\"batch\":false,\"jobs\":[{}]}}", json_str(&c.repo.to_string_lossy()), req("check", &[]))).unwrap();
        let id = c.store.read(|d| d.jobs[0].id.clone());
        std::fs::create_dir_all(c.logs("jobs")).unwrap();
        std::fs::write(c.logs("jobs").join(format!("{id}.log")), "error: one\nthe last line\n").unwrap();
        let brief = serve(&c, "job list brief").unwrap();
        assert!(brief.contains(&format!("\"id\":\"{id}\"")) && !brief.contains("the last line"), "{brief}");
        assert!(serve(&c, "job list").unwrap().contains("the last line"), "the full list reads the log");
        std::fs::remove_dir_all(&c.home).unwrap();
    }

    /// `job output <id> -n` reads the log's last n bytes; the answer's `off` asks on from there.
    #[test]
    fn output_from_the_end_reads_only_the_tail() {
        let c = ctx("tail");
        serve(&c, &format!("job submit {{\"tree\":{},\"rev\":\"\",\"batch\":false,\"jobs\":[{}]}}", json_str(&c.repo.to_string_lossy()), req("check", &[]))).unwrap();
        let id = c.store.read(|d| d.jobs[0].id.clone());
        std::fs::create_dir_all(c.logs("jobs")).unwrap();
        let log = c.logs("jobs").join(format!("{id}.log"));
        std::fs::write(&log, "first line\nthe last line\n").unwrap();
        let ask = |line: &str| makepad_strict_json::parse(serve(&c, line).unwrap().as_bytes()).unwrap();
        let data = |v: &makepad_strict_json::Value| String::from_utf8(crate::lease::master::b64_decode(v.get("data").unwrap().as_str().unwrap()).unwrap()).unwrap();
        let tail = ask(&format!("job output {id} -14"));
        assert_eq!((data(&tail).as_str(), tail.get("off").and_then(|v| v.as_u64())), ("the last line\n", Some(25)));
        let all = ask(&format!("job output {id} -1000"));
        assert_eq!(data(&all), "first line\nthe last line\n", "a tail longer than the log is the log");
        std::fs::write(&log, "first line\nthe last line\nmore\n").unwrap();
        let next = ask(&format!("job output {id} 25"));
        assert_eq!((data(&next).as_str(), next.get("off").and_then(|v| v.as_u64())), ("more\n", Some(30)));
        std::fs::remove_dir_all(&c.home).unwrap();
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
        assert!(r.contains(&format!("\"rev\":\"rc-t@{head},commercial=rc-t@{chead}\"")) && r.contains("\"snap\":\"S1\",\"files\":4"), "{r}");
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
        // the commits alone with a rev; one naming only the makepad side gets the nested
        // commercial HEAD too (a commercial package was missing from the overlay without it)
        let r = submit_line(&c, "rc-t", false, &[req("x", &[])]).unwrap();
        assert!(r.contains(&format!("\"rev\":\"rc-t@{head},commercial=rc-t@{chead}\",\"snap\":\"\"")), "{r}");
        assert_eq!(crate::lease::client::pin_lane_rev(&tree, &format!("rc-t,commercial=rc-t@{}", &chead[..10]), "lane1").unwrap(), format!("rc-t@{head},commercial=rc-t@{chead}"));
        let old = crate::lease::client::pin_lane_rev(&tree, "rc-t,rc-t", "lane1").unwrap_err();
        assert!(old.contains("write `<name>=<branch>`, as in `work,commercial=main`"), "an old-form rev is refused in one line: {old}");
        // bad requests change nothing
        assert!(submit_line(&c, "", false, &[req("x", &["@1"])]).unwrap_err().contains("no earlier job"));
        assert!(submit_line(&c, "", false, &[req("x", &["J9"])]).unwrap_err().contains("no job J9"));
        assert!(submit_line(&c, "", false, &[req("x", &[]).replace("\"test\"", "\"inspect\"")]).is_err());
        assert_eq!(c.store.read(|d| d.jobs.len()), 4);
        // J1 failed: J2 and J3 are skipped, never leased
        c.store.update(|d| d.jobs[0].state = "failed".into()).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
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
        c.store.update(|d| d.jobs[5].state = "cancelled".into()).unwrap_or_else(|e| { eprintln!("devhub state: {e}"); Default::default() });
        assert!(serve(&c, "job get J5 J9").unwrap().contains("\"state\":\"unknown\""));
        assert!(serve(&c, "job resume J5 P1 retry").unwrap_err().contains("not paused"));
        // a lane named in upper case is the same lane: folded at submit
        submit_line(&c, "rc-t", false, &[req("x", &[]).replace("\"lane1\"", "\"LANE1\"")]).unwrap();
        assert_eq!(c.store.read(|d| d.jobs.last().map(|j| j.lane.clone())), Some("lane1".to_string()));
        let home = c.repo.parent().unwrap().to_path_buf();
        drop(c);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// A JIT test of a package whose Cargo.toml says `test-tree` runs in the lane's tree, as
    /// `devhub test` runs it; any other JIT test keeps its overlay. A batch's app-run test built
    /// its apps with cargo at the overlay's virtual root, which the disk has not (11-08).
    #[test]
    fn a_jit_test_whose_package_reads_the_checkout_runs_in_the_tree() {
        let c = ctx("test-tree");
        let tree = c.repo.clone();
        for (dir, manifest) in [("libs/a", "[package]\nname = \"pkg-a\"\n[package.metadata.makepad]\ntest-tree = \"its tests run cargo on the checkout\"\n"), ("libs/b", "[package]\nname = \"pkg-b\"\n")] {
            std::fs::create_dir_all(tree.join(dir)).unwrap();
            std::fs::write(tree.join(dir).join("Cargo.toml"), manifest).unwrap();
        }
        g(&tree, &["add", "."]);
        g(&tree, &["commit", "-q", "-m", "packages"]);
        let job = |cmd: &[&str]| format!("{{\"kind\":\"test\",\"needs\":\"arch=x64\",\"lane\":\"lane1\",\"agent\":\"a\",\"cmd\":[{}],\"after\":[]}}", cmd.iter().map(|w| json_str(w)).collect::<Vec<_>>().join(","));
        let r = submit_line(&c, "", false, &[
            job(&["env", "CARGO_INCREMENTAL=0", "makepad", "test", "-p", "pkg-a", "--release", "--", "one"]),
            job(&["makepad", "test", "pkg-b", "--lib"]),
        ]).unwrap();
        assert!(r.contains("job 1: pkg-a runs in the lane's tree: its tests run cargo on the checkout (its Cargo.toml's test-tree)"), "{r}");
        assert!(!r.contains("job 2:"), "{r}");
        assert_eq!(c.store.read(|d| d.jobs.iter().map(|j| j.src.clone()).collect::<Vec<_>>()), ["tree", "overlay"]);
        let home = c.repo.parent().unwrap().to_path_buf();
        drop(c);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_timed_successor_inherits_the_chains_first_ask_once_ready() {
        let c = ctx("timed-chain");
        let timed = |cmd: &str, after: &[&str]| req(cmd, after).replace("\"kind\":\"test\"", "\"kind\":\"timed\"");
        submit_line(&c, "", true, &[
            timed("first", &[]), timed("next", &["@1"]), timed("last", &["@2"]),
            req("plain", &["@1"]), timed("other", &["@1"]).replace("lane1", "lane2"),
            timed("blocked", &["@1", "@3"]),
        ]).unwrap();
        set(&c, "J1", |j| { j.asked_ms = 10; j.state = "running".into(); });
        assert!(chain_successors(&c, "J1").is_empty(), "not before a successful finish");
        finish(&c, "J1", Ok(0), &c.logs("missing.log"));
        assert_eq!(chain_successors(&c, "J1"), vec!["J2"]);
        assert_eq!(get(&c, "J2").unwrap().asked_ms, 10);
        assert!(c.store.read(|d| d.jobs[2..].iter().all(|j| j.asked_ms == 0)), "only ready timed successors in this lane inherit");
        set(&c, "J1", |j| j.asked_ms = 20);
        chain_successors(&c, "J1");
        assert_eq!(get(&c, "J2").unwrap().asked_ms, 10, "an assigned place never changes");
        finish(&c, "J2", Ok(0), &c.logs("missing.log"));
        assert_eq!(chain_successors(&c, "J2"), vec!["J3"]);
        assert_eq!(get(&c, "J3").unwrap().asked_ms, 10, "the whole chain keeps the first ask");
        std::fs::remove_dir_all(&c.home).unwrap();
    }

    #[test]
    fn a_running_jobs_log_is_read_from_its_end() {
        let p = std::env::temp_dir().join(format!("devhub-log-end-{}", std::process::id()));
        std::fs::write(&p, "first line\nsecond\nthird\n").unwrap();
        assert_eq!(log_end(&p, 12).as_deref(), Some("third\n"), "from the first whole line");
        assert_eq!(log_end(&p, 100).as_deref(), Some("first line\nsecond\nthird\n"));
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn a_failing_test_programs_panic_leads_over_a_passing_ones() {
        let report = |m: &str| format!("  report: {{\"kind\":\"panic\",\"message\":\"{m}\",\"location\":\"a.rs:1:1\"}}");
        let text = format!("running 57 tests\n{}\ntest result: ok. 57 passed; 0 failed\n\nrunning 3 tests\n{}\ntest a ... FAILED\ntest result: FAILED. 0 passed; 3 failed\n", report("injected"), report("left != right"));
        let (errors, _) = digest(&text);
        assert!(errors[0].contains("left != right"), "{errors:?}");
        assert!(errors.last().is_some_and(|e| e.contains("injected")), "{errors:?}");
        // a program that never reached its result crashed: nothing it said was expected
        let (errors, _) = digest(&format!("running 2 tests\n{}\n", report("crash")));
        assert!(errors[0].contains("crash"), "{errors:?}");
    }

    #[test]
    fn digest_keeps_errors_and_tail() {
        let text = "Compiling a\nerror[E0425]: cannot find `x`\n  --> a.rs:1\nthread 'main' panicked at a.rs:3\n\nok\n";
        let (e, t) = digest(text);
        assert_eq!(e, vec!["error[E0425]: cannot find `x`".to_string(), "thread 'main' panicked at a.rs:3".to_string()]);
        assert_eq!(t.last().map(String::as_str), Some("ok"));
        // the JIT's panic is its report line; a later `report:` that is no report stays out
        let text = "  report: {\"kind\":\"panic\",\"message\":\"not in 5 s\",\"build\":\"b\",\"location\":\"a.rs:89:13\",\"backtrace\":[]}\ntest x ... FAILED\nmakepad: report: /a/capture.txt\n";
        assert_eq!(digest(text).0, vec!["panic: not in 5 s at a.rs:89:13".to_string(), "test x ... FAILED".to_string()]);
        let j = Job { id: "J1".into(), state: "paused".into(), cmd: vec!["cargo".into()], panic: Some(JobPanic { id: "P1".into(), pid: 7, message: "boom".into(), thread: "main".into(), location: "a.rs:3".into(), at_ms: 0 }), ..Default::default() };
        assert!(render(&j, false).contains("job resume J1 P1 retry|restart|unwind"));
        let j = Job { id: "J2".into(), state: "failed".into(), log: "/h/jobs/J2.log".into(), ..Default::default() };
        assert!(render(&j, false).contains("whole log: /h/jobs/J2.log"));
        // its first line names its lane (`job list` has every lane's)
        let j = Job { id: "J3".into(), state: "running".into(), kind: "test".into(), node: "boxa".into(), lease: "L9".into(), lane: "lanea".into(), cmd: vec!["bash".into(), "x.sh".into()], ..Default::default() };
        assert_eq!(render(&j, false).lines().next(), Some("J3 running test@boxa L9 @lanea: bash x.sh"));
    }

    /// A pause lasts while the panic stays parked, and the job's verdict is its own exit: a released
    /// box is the only end that reads `panicked`.
    #[test]
    fn a_pause_lasts_while_the_panic_is_parked_and_the_exit_decides() {
        let c = Arc::new(ctx("parks"));
        let p = JobPanic { id: "P1".into(), pid: 7, message: "expected".into(), ..Default::default() };
        let paused = |id: &str| Job { id: id.into(), state: "paused".into(), panic: Some(p.clone()), jit_error: "JIT panic: expected".into(), ..Default::default() };
        c.store.update(|d| {
            d.jobs.extend(["J1", "J2", "J3", "J4"].map(paused));
            d.jobs.push(Job { id: "J5".into(), state: "running".into(), ..Default::default() });
        }).unwrap();
        let state = |id: &str| c.store.read(|d| d.jobs.iter().find(|j| j.id == id).map(|j| j.state.clone())).unwrap();
        // still parked, or no answer from the box: paused
        super::runs_on(&c, &|_, _| Some(true));
        super::runs_on(&c, &|_, _| None);
        assert_eq!(state("J1"), "paused");
        // the park ended: the job runs on, the panic stays on its record
        super::runs_on(&c, &|j, _| (j.id == "J1").then_some(false).or(Some(true)));
        assert_eq!((state("J1"), state("J2")), ("running".to_string(), "paused".to_string()));
        let j1 = super::get(&c, "J1").unwrap();
        assert!(j1.jit_error.is_empty() && j1.panic == Some(p.clone()) && j1.note.contains("P1 of pid 7"), "{j1:?}");
        assert!(render(&j1, false).contains("panic P1: expected") && !render(&j1, false).contains("job resume"));
        let log = c.repo.join("job.log");
        std::fs::write(&log, "ran\n").unwrap();
        // its script went on to pass, or to fail on its own
        super::finish(&c, "J1", Ok(0), &log);
        assert_eq!(state("J1"), "ok");
        // a script that ended in the moment before the park's end is seen: its exit, too
        super::finish(&c, "J2", Ok(0), &log);
        super::finish(&c, "J3", Ok(101), &log);
        assert_eq!((state("J2"), state("J3")), ("ok".to_string(), "failed".to_string()));
        // the agent or the limit released the box with the panic parked: the process it kills is
        // no park that ended, and a job back to running would be placed again by its ended lease
        serve(&c, "job release J4").unwrap();
        super::runs_on(&c, &|_, _| Some(false));
        assert_eq!(state("J4"), "paused");
        super::finish(&c, "J4", Err("L1 ended (released)".into()), &log);
        assert_eq!(state("J4"), "panicked");
        // a job not paused is not asked about
        super::runs_on(&c, &|_, _| panic!("asked about a job with nothing parked"));
        assert_eq!(state("J5"), "running");
    }
}
