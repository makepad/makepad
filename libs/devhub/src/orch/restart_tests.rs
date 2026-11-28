//! A master restart ends nothing, end to end: a real master process (leases, jobs, verify, CI,
//! allapps, warm) over real node executors in another process, killed under a job, a job whose
//! lease is still building its JIT, a verify leg, a CI shard, its own allapps run and a box's JIT
//! prebuild mid-build, and started again on its state after the nodes went longer than their
//! no-beat limit without it; and a job whose run had ended and whose lease had gone before its
//! verdict was recorded. All finish with their real exit codes, each command and each build run
//! once. The tools wait for a release file, created once the restarted master has taken its state
//! back.

use super::*;
use crate::lease::exec::{BoxLook, Exec, Opts, WORDS};
use crate::lease::master::SERVICES as LEASE_SERVICES;
use makepad_remote::hosts::{Host, Hosts, Via};
use makepad_remote::hub::Hub;
use makepad_remote::{lines, Endpoint};
use makepad_strict_json::Value;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// The first two are the CI boxes (their lease.conf opts in).
const NODES: [&str; 6] = ["c1", "c2", "n3", "n4", "n5", "n6"];
// Match makepad_remote::carrier::unix's sun_path limit, excluding its terminating zero.
#[cfg(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd"))]
const SOCKET_PATH_MAX: usize = 103;
#[cfg(not(any(target_os = "macos", target_os = "ios", target_os = "freebsd", target_os = "openbsd", target_os = "netbsd")))]
const SOCKET_PATH_MAX: usize = 107;
/// The nodes' no-beat limit here (60 s on a box), five of the master's beats ([`PACE`]); it is
/// gone until each box holding its leases has said it is away.
const NO_BEAT: Duration = Duration::from_secs(1);
const PACE: crate::lease::master::Pace = crate::lease::master::Pace { beat: Duration::from_millis(200), caps: Duration::from_millis(500) };

fn role_env(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_default())
}

/// A role lives while the test that started it holds its stdin: until that test ends, or dies.
/// A life of 600 s of its own ran out on a loaded box before the event the test waited for, and
/// the wait failed on "the nodes process ended (exit status: 0)" (11-26).
fn live_while_held() {
    let _ = std::io::Read::read_to_end(&mut std::io::stdin(), &mut Vec::new());
}

/// The hub side: the executors of [`NODES`], each on its socket. Its PATH has the fake tools.
#[test]
fn restart_role_nodes() {
    if std::env::var_os("DEVHUB_RESTART_ROLE").is_none_or(|r| r != "nodes") {
        return;
    }
    let base = role_env("DEVHUB_RESTART_BASE");
    // The boots need a gui box, independent of the host's WM (a node's may speak another wire).
    let wm = std::os::unix::net::UnixListener::bind(role_env("XDG_RUNTIME_DIR").join("makepad-wm-0")).unwrap();
    let _ = std::thread::spawn(move || {
        use std::io::{BufRead, Write};
        for mut s in wm.incoming().flatten() {
            let _ = std::io::BufReader::new(&s).read_line(&mut String::new());
            let _ = s.write_all(b"makepad-wm ok 1\n");
        }
    });
    let mut keep = Vec::new();
    for name in NODES {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        // (beside the test binary: a build lease asks for more disk than /tmp may hold)
        let work = work_dir(&base, name);
        let ci = if NODES[..2].contains(&name) { "" } else { "slots.ci = 0\n" };
        let conf = crate::lease::caps::LeaseConf::parse(&format!("work = {}\nrole = pool\ndisk_floor_gb = 0\n{ci}", work.display()), &dir);
        // BoxLook::quiet() reports wm:makepad-wm-0, matching the fake WM socket above.
        let x = Exec::new(name, conf, dir.join("leases"), Opts { watch_memory: false, scopes: Some(false), no_beat: NO_BEAT, tick: Duration::from_millis(100), box_look: Some(BoxLook::quiet()), ..Opts::default() }, Arc::new(|_| {}));
        let ep = Endpoint::new(name);
        for &word in WORDS {
            let e = x.clone();
            ep.register(word, 1, lines::service(word, Arc::new(move |line, out| {
                // no checkout here: a lease at a revision, an overlay too, runs in its lane's dir
                if word == "ship-done" { out.push("{\"overlay\":0}".into()); return; }
                let line = if word == "lease-start" { unpinned(line).replace("\"src\":\"overlay\"", "\"src\":\"tree\"") } else { line.to_string() };
                out.extend(e.handle(word, &line));
            })));
        }
        makepad_remote::carrier::unix::listen_at(&ep, &base.join(format!("{name}.sock"))).unwrap();
        keep.push((x, ep));
    }
    live_while_held();
}

fn work_dir(base: &std::path::Path, node: &str) -> PathBuf {
    std::env::current_exe().unwrap().with_file_name(format!("{}-{node}", base.file_name().unwrap().to_string_lossy()))
}

fn socket_paths_under(base: &Path) -> Vec<PathBuf> {
    // <base>/registry holds only JSON discovery files. makepad_remote::carrier::unix::paths()
    // puts endpoint sockets in /tmp/makepad-<uid>.
    let mut paths = vec![base.join("home/devhub/devhub.sock"), base.join("rt/makepad-wm-0")];
    paths.extend(NODES.iter().map(|name| base.join(format!("{name}.sock"))));
    paths
}

fn longest_socket_under(base: &Path) -> (PathBuf, usize) {
    socket_paths_under(base).into_iter().map(|p| {
        let len = p.as_os_str().as_encoded_bytes().len();
        (p, len)
    }).max_by_key(|(_, len)| *len).unwrap()
}

struct RestartBase {
    dir: PathBuf,
    fallback_warning: Option<String>,
    marker: Option<PathBuf>,
}

fn restart_base_in(temp_dir: &Path, fallback_root: &Path, pid: u32, limit: usize) -> Result<RestartBase, String> {
    let name = format!("dhr{pid}");
    let primary = temp_dir.join(&name);
    let (socket, len) = longest_socket_under(&primary);
    if len <= limit {
        return Ok(RestartBase { dir: primary, fallback_warning: None, marker: None });
    }
    let fallback = fallback_root.join(&name);
    let (fallback_socket, fallback_len) = longest_socket_under(&fallback);
    let why = format!("restart test temp dir {} gives socket {} ({len} bytes), over the Unix socket limit of {limit} bytes", temp_dir.display(), socket.display());
    if fallback_len > limit {
        return Err(format!("{why}; fallback socket {} ({fallback_len} bytes) also exceeds the limit", fallback_socket.display()));
    }
    Ok(RestartBase {
        fallback_warning: Some(format!("{why}; using {}. A killed run can leave this directory outside the lease tmp.", fallback.display())),
        marker: Some(temp_dir.join(format!("{name}.fallback"))),
        dir: fallback,
    })
}

struct BaseGuard {
    dir: PathBuf,
    marker: Option<PathBuf>,
    work_dirs: Vec<PathBuf>,
}

impl Drop for BaseGuard {
    fn drop(&mut self) {
        for work in &self.work_dirs {
            let _ = std::fs::remove_dir_all(work);
        }
        let _ = std::fs::remove_dir_all(&self.dir);
        if let Some(marker) = &self.marker {
            let _ = std::fs::remove_file(marker);
        }
    }
}

#[test]
fn restart_base_uses_temp_dir_and_falls_back_loudly_when_socket_paths_exceed_limit() {
    let fallback_root = Path::new("/tmp");
    for (tmp_len, suffix, pid, limit, socket_len) in [
        (37, "/L1998250", 4194304, 107, 72),
        (44, "/L1998250", 99999, 103, 77),
        (48, "", 99999, 103, 81),
    ] {
        let temp = PathBuf::from(format!("/srv/{}{suffix}", "t".repeat(tmp_len - "/srv/".len() - suffix.len())));
        assert_eq!(temp.as_os_str().as_encoded_bytes().len(), tmp_len);
        let chosen = restart_base_in(&temp, fallback_root, pid, limit).unwrap();
        assert_eq!(chosen.dir, temp.join(format!("dhr{pid}")));
        assert_eq!(longest_socket_under(&chosen.dir), (chosen.dir.join("home/devhub/devhub.sock"), socket_len));
        assert_eq!(chosen.fallback_warning, None);
        assert_eq!(chosen.marker, None);
    }

    // Count bytes even when a path component contains multibyte characters.
    let component = "é".repeat(34);
    let temp = PathBuf::from(format!("/s{component}"));
    let primary = temp.join("dhr99999");
    assert_eq!(longest_socket_under(&primary).1, 103);
    let chosen = restart_base_in(&temp, fallback_root, 99999, 103).unwrap();
    assert_eq!(chosen.dir, primary);
    assert_eq!(chosen.fallback_warning, None);
    assert_eq!(chosen.marker, None);

    let temp = PathBuf::from(format!("/st{component}"));
    let primary_socket = temp.join("dhr99999/home/devhub/devhub.sock");
    assert_eq!(primary_socket.as_os_str().as_encoded_bytes().len(), 104);
    let chosen = restart_base_in(&temp, fallback_root, 99999, 103).unwrap();
    assert_eq!(chosen.dir, fallback_root.join("dhr99999"));
    assert_eq!(chosen.marker, Some(temp.join("dhr99999.fallback")));
    let warning = chosen.fallback_warning.unwrap();
    let primary_detail = format!("{} (104 bytes)", primary_socket.display());
    assert!(warning.contains(&primary_detail) && warning.contains("limit of 103 bytes"), "{warning}");
    assert!(warning.contains(chosen.dir.to_str().unwrap()) && warning.contains("outside the lease tmp"), "{warning}");

    let long_root = temp.join("long");
    let err = restart_base_in(&temp, &long_root, 99999, 103).err().expect("both sockets exceed the limit");
    let fallback_detail = format!("{} (109 bytes)", long_root.join("dhr99999/home/devhub/devhub.sock").display());
    assert!(err.contains(&primary_detail) && err.contains(&fallback_detail) && err.contains("limit of 103 bytes"), "{err}");

    let temp = std::env::temp_dir();
    let name = format!("dhr-guard-{}", std::process::id());
    let dir = temp.join(&name);
    let marker = temp.join(format!("{name}.fallback"));
    let work_dirs = [temp.join(format!("{name}-a")), temp.join(format!("{name}-b"))];
    for unwind in [false, true] {
        for path in work_dirs.iter().chain([&dir]) {
            std::fs::create_dir_all(path.join("nested")).unwrap();
        }
        std::fs::write(&marker, "fallback").unwrap();
        let guard = BaseGuard { dir: dir.clone(), marker: Some(marker.clone()), work_dirs: work_dirs.to_vec() };
        let result = std::panic::catch_unwind(move || {
            let _guard = guard;
            if unwind {
                // Unwind through the guard without logging an expected panic as a test failure.
                std::panic::resume_unwind(Box::new(()));
            }
        });
        assert_eq!(result.is_err(), unwind);
        for path in work_dirs.iter().chain([&dir, &marker]) {
            assert!(!path.try_exists().unwrap(), "{} survived cleanup (unwind={unwind})", path.display());
        }
    }
}

fn unpinned(line: &str) -> String {
    let Some(at) = line.find("\"rev\":{") else { return line.to_string() };
    let end = at + line[at..].find('}').unwrap();
    format!("{}\"rev\":null{}", &line[..at], &line[end + 1..])
}

/// The master side, as `Master::start` wires it; a second start of it is the restart.
#[test]
fn restart_role_master() {
    if std::env::var_os("DEVHUB_RESTART_ROLE").is_none_or(|r| r != "master") {
        return;
    }
    let base = role_env("DEVHUB_RESTART_BASE");
    let hosts = Hosts { hosts: NODES.iter().map(|n| Host { name: n.to_string(), via: Via::Fleet { addr: base.join(format!("{n}.sock")).to_string_lossy().into_owned() } }).collect() };
    let hub = Hub::new(&Hosts::default());
    hub.set_fleet(Arc::new(|addr: &str| makepad_remote::carrier::unix::dial(std::path::Path::new(addr)).map(|(c, s)| {
        let _ = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(500));
            makepad_remote::carrier::unix::settle(&s);
        });
        c
    })));
    hub.sync(&hosts);
    let ep = hub.client_endpoint();
    let ctl = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(crate::proto::control_socket()), "devhub-lease");
    let ask: crate::lease::master::Ask = Arc::new(move |node: &str, service: &str, line: &str| {
        let mut out = Vec::new();
        makepad_remote::hub::ask(&ctl, node, service, "", line, &mut |l| {
            out.push(l.to_string());
            true
        })?;
        Ok(out)
    });
    let rev: crate::lease::master::EnsureRev = Arc::new(|_: &str, _: &str, _: &std::path::Path, pin: &crate::lease::RevPin, _: bool| Ok(pin.sha.clone()));
    let leases = Arc::new(LeaseMaster::new("mac", crate::proto::home(), ask, Arc::new(|_| {}), Arc::new(crate::proto::wall_ms), Some(rev), false));
    let names: Vec<(String, String)> = hosts.hosts.iter().map(|h| (h.name.clone(), match &h.via { Via::Fleet { addr } => addr.clone(), _ => String::new() })).collect();
    leases.test_sched(|s| {
        for (name, addr) in &names {
            s.add_node(name, addr, None);
        }
    });
    for svc in LEASE_SERVICES {
        let l = leases.clone();
        ep.register(svc, 1, lines::service(svc, Arc::new(move |line, out| l.serve(svc, line, out))));
    }
    start(&ep, "mac", leases.clone(), Arc::new(|_| 0));
    ep.register("land", 1, lines::service("land", Arc::new(|line, out| {
        assert!(line.starts_with("land add ci-proof-") && line.ends_with(" proof"));
        out.push("{\"ok\":1,\"id\":1}".into());
    })));
    crate::node::listen_control(&ep).unwrap();
    let ctx = ctx().unwrap().clone();
    if let Some(submit) = std::env::var_os("DEVHUB_RESTART_SUBMIT") {
        job::serve(&ctx, &submit.to_string_lossy()).unwrap();
        // the JIT's legs run in the background job (jitlegs), asked here
        jitlegs::serve(&ctx, "jitlegs run").unwrap();
        // and the master's own run, as `allapps kick` asks it
        allapps::serve(&ctx, "allapps kick").unwrap();
    }
    job::start(ctx.clone());
    ci::start(&ctx);
    jitlegs::start(ctx.clone());
    allapps::start(ctx.clone());
    verify::start(ctx.clone());
    // the warm legs the previous master ran come back (`warm::start` without its planner: these
    // boxes report no JIT store); the first master plans one box's prebuild
    warm::take_back(&ctx);
    if std::env::var_os("DEVHUB_RESTART_SUBMIT").is_some() {
        warm::prebuild_on(&ctx, "n6", &snapshot(&ctx.repo).unwrap().rev());
    }
    // as `go` does: the master's own leases no record names end
    let named = ctx.store.read(|d| {
        let mut ids = std::collections::HashSet::new();
        strings_of(&d.to_value(), &mut ids);
        ids
    });
    ctx.leases.end_unfollowed(&|id| named.contains(id));
    leases.start(Arc::new(move || names.clone()), PACE);
    if std::env::var_os("DEVHUB_RESTART_SUBMIT").is_some() {
        loop {
            let seen = leases.generation();
            if leases.nodes().iter().filter(|n| n.up && n.caps.is_some()).count() == NODES.len() {
                break;
            }
            leases.wait_change(seen, Duration::MAX);
        }
        let run = ci::ci(ctx.repo.clone());
        let pin = super::snapshot_pin(&ctx.repo).unwrap();
        let log = base.join("ci.log");
        let _ = std::thread::spawn(move || run(None, &pin, &log));
    }
    eprintln!("restart master ready");
    live_while_held();
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let o = Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn script(path: &std::path::Path, text: &str) {
    crate::testenv::exec_script(path, text);
}

/// Kills the role processes it started, by the pids it holds; each by its role's name.
struct Roles(Vec<(&'static str, Child)>);

impl Roles {
    /// The first of them that has ended, with how: the end of every wait on what it would have
    /// done. Each lives while this test holds its stdin ([`live_while_held`]), so its end is a
    /// crash. A loaded box ran past the earlier 180 s wait (11-24) and 600 s role lifetime (11-26).
    fn ended(&mut self) -> Option<String> {
        self.0.iter_mut().find_map(|(name, c)| c.try_wait().ok().flatten().map(|status| format!("the {name} process ended ({status})")))
    }
}

impl Drop for Roles {
    fn drop(&mut self) {
        for (_, c) in &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn json_num(v: &Value, k: &str) -> u64 {
    v.get(k).and_then(crate::lease::num).unwrap_or(0.0) as u64
}

fn json_text(v: &Value, k: &str) -> String {
    v.get(k).and_then(Value::as_str).unwrap_or_default().to_string()
}

fn job_terminal(state: &str) -> bool {
    !matches!(state, "" | "queued" | "leasing" | "running")
}

fn shard_terminal(state: &str) -> bool {
    matches!(state, "ok" | "fail" | "incomplete" | crate::state::NOT_RUN)
}

fn job0(s: &Value) -> Option<Value> {
    s.get("jobs")?.as_arr()?.first().cloned()
}

fn job_lane(s: &Value, lane: &str) -> Option<Value> {
    s.get("jobs")?.as_arr()?.iter().find(|j| j.get("lane").and_then(Value::as_str) == Some(lane)).cloned()
}

fn jitlegs_run(s: &Value) -> Option<Value> {
    s.get("jitlegs")?.get("runs")?.as_arr()?.last().cloned()
}

fn leg0(s: &Value) -> Option<Value> {
    jitlegs_run(s)?.get("legs")?.as_arr()?.first().cloned()
}

fn shards_of(s: &Value) -> Vec<Value> {
    s.get("ci").and_then(|c| c.get("run")).and_then(|r| r.get("shards")).and_then(Value::as_arr).unwrap_or_default().to_vec()
}

fn allapps_running(s: &Value) -> Option<Value> {
    s.get("allapps")?.get("running").filter(|r| !r.is_null()).cloned()
}

fn allapps_last(s: &Value) -> Option<Value> {
    s.get("allapps")?.get("last").filter(|r| !r.is_null()).cloned()
}

fn prebuild(s: &Value) -> Option<Value> {
    s.get("warm")?.get("plan")?.get("nodes")?.as_arr()?.first()?.get("progress").cloned()
}

fn pre_kill_ready(s: &Value, build_count: usize, prebuild_count: usize) -> Result<bool, String> {
    let shard_result = |x: &Value| format!("{}={} ({})", json_text(x, "id"), json_text(x, "state"), json_text(x, "summary"));
    if let Some(run) = s.get("ci").and_then(|c| c.get("runs")).and_then(Value::as_arr).and_then(|r| r.last()) {
        let results = run.get("shards").and_then(Value::as_arr).unwrap_or_default().iter().map(shard_result).collect::<Vec<_>>();
        return Err(format!("CI ended with verdict {}: {}", json_text(run, "verdict"), results.join(", ")));
    }
    let shards = shards_of(s);
    if let Some(shard) = shards.iter().find(|x| shard_terminal(&json_text(x, "state"))) {
        return Err(format!("CI shard ended: {}", shard_result(shard)));
    }
    let job = job0(s);
    let built = job_lane(s, "built");
    let exited = job_lane(s, "exited");
    for (lane, job) in [("first", &job), ("built", &built), ("exited", &exited)] {
        if let Some(j) = job {
            let state = json_text(j, "state");
            if job_terminal(&state) && !(lane == "exited" && state == "failed") {
                return Err(format!("{lane} job {} ended in {state}: {}", json_text(j, "id"), json_text(j, "note")));
            }
        }
    }
    if build_count > 1 {
        return Err(format!("built job started {build_count} builds"));
    }
    if let Some(run) = jitlegs_run(s).filter(|r| json_num(r, "ended_ms") > 0) {
        return Err(format!("jitlegs run ended: {}", run.to_json()));
    }
    let leg = leg0(s);
    if let Some(l) = leg.as_ref().filter(|l| shard_terminal(&json_text(l, "state"))) {
        return Err(format!("jitlegs leg {} ended in {}: {}", json_text(l, "name"), json_text(l, "state"), json_text(l, "summary")));
    }
    let allapps = allapps_running(s);
    if allapps.is_none() {
        if let Some(last) = allapps_last(s) {
            return Err(format!("allapps ended with verdict {}", json_text(&last, "verdict")));
        }
    }
    let prebuild = prebuild(s);
    let state = prebuild.as_ref().map(|p| json_text(p, "state")).unwrap_or_default();
    let ended_ms = prebuild.as_ref().map(|p| json_num(p, "ended_ms")).unwrap_or_default();
    if ended_ms > 0 || matches!(state.as_str(), "done" | "failed" | "stored") || prebuild_count > 1 {
        return Err(format!("prebuild state={state:?}, ended_ms={ended_ms}, builds={prebuild_count}"));
    }

    // The kill must land on a settled fleet. CI runs on two nodes, so other shards can be
    // waiting. Each granted shard must already have its run recorded for the restart to follow.
    Ok(job.is_some_and(|j| json_num(&j, "run_n") > 0)
        && leg.is_some_and(|l| json_num(&l, "run_n") > 0)
        && shards.iter().any(|x| json_num(x, "run_n") > 0)
        && shards.iter().all(|x| json_text(x, "lease").is_empty() || json_num(x, "run_n") > 0)
        && allapps.is_some_and(|r| json_num(&r, "run_n") > 0)
        && built.is_some_and(|j| !json_text(&j, "lease").is_empty()) && build_count == 1
        && prebuild.is_some_and(|p| state == "building" && !json_text(&p, "lease").is_empty()) && prebuild_count == 1
        && exited.is_some_and(|j| json_text(&j, "state") == "failed"))
}

fn verdict_ready(s: &Value) -> Result<bool, String> {
    let verify = s.get("verify");
    if let Some(failing) = verify.and_then(|v| v.get("failing")).filter(|f| !f.is_null()) {
        return Err(format!("verify failed (bisecting): {}", failing.to_json()));
    }
    if let Some(failure) = verify.and_then(|v| v.get("failures")).and_then(Value::as_arr).and_then(|a| a.last()) {
        return Err(format!("verify recorded a failure: {}", failure.to_json()));
    }
    let verified = verify.and_then(|v| v.get("verified")).and_then(Value::as_arr).is_some_and(|v| !v.is_empty());
    let ci_runs_non_empty = s.get("ci").and_then(|c| c.get("runs")).and_then(Value::as_arr).is_some_and(|r| !r.is_empty());
    Ok(verified && ci_runs_non_empty)
}

#[test]
fn pre_kill_and_verdict_waits_fail_loudly_when_events_cannot_come() {
    let parse = |s: &str| makepad_strict_json::parse(s.as_bytes()).unwrap();
    let settled = parse(r#"{
        "jobs": [{"id":"J1","state":"running","run_n":1},{"lane":"built","lease":"L2"},{"lane":"exited","state":"failed"}],
        "ci": {"run":{"shards":[{"state":"run","lease":"L3","run_n":1},{"state":"wait"}]}},
        "jitlegs": {"runs":[{"legs":[{"state":"run","run_n":1}]}]},
        "allapps": {"running":{"run_n":1}},
        "warm": {"plan":{"nodes":[{"progress":{"state":"building","lease":"L4"}}]}}
    }"#);
    assert_eq!(pre_kill_ready(&settled, 1, 0), Ok(false), "still waiting for the box's build");
    assert_eq!(pre_kill_ready(&settled, 1, 1), Ok(true), "every granted shard has started");
    assert!(pre_kill_ready(&settled, 2, 1).unwrap_err().contains("2 builds"));
    let why = pre_kill_ready(&settled, 1, 2).unwrap_err();
    assert!(why.contains("building") && why.contains("builds=2"), "{why}");

    let rejects = |snapshot: &str, details: &[&str]| {
        let why = pre_kill_ready(&parse(snapshot), 1, 1).expect_err(snapshot);
        for detail in details {
            assert!(why.contains(*detail), "missing {detail:?} in {why:?} for {snapshot}");
        }
    };
    for state in ["not run", "incomplete", "fail", "ok"] {
        let shard = format!(r#"{{"id":"smoke:a","state":"{state}","summary":"display check"}}"#);
        rejects(&format!(r#"{{"ci":{{"run":{{"shards":[{shard}]}}}}}}"#), &["smoke:a", state, "display check"]);
    }
    rejects(r#"{"ci":{"run":null,"runs":[{"verdict":"incomplete","shards":[
        {"id":"smoke:a","state":"not run","summary":"no display"},{"id":"smoke:b","state":"fail","summary":"boot failed"}
    ]}]}}"#, &["incomplete", "smoke:a=not run (no display)", "smoke:b=fail (boot failed)"]);
    rejects(r#"{"jobs":[{"id":"J1","state":"failed","note":"early exit"}]}"#, &["J1", "failed", "early exit"]);
    rejects(r#"{"jobs":[{"state":"queued"},{"lane":"built","id":"J3","state":"failed","note":"build refused"}]}"#,
        &["built", "J3", "failed", "build refused"]);
    rejects(r#"{"jobs":[{"state":"leasing"},{"lane":"exited","id":"J4","state":"ok"}]}"#, &["exited", "J4", "ok"]);
    rejects(r#"{"jitlegs":{"runs":[{"ended_ms":1}]}}"#, &["jitlegs", "ended_ms"]);
    rejects(r#"{"jitlegs":{"runs":[{"legs":[{"name":"test","state":"incomplete","summary":"lease ended"}]}]}}"#,
        &["test", "incomplete", "lease ended"]);
    rejects(r#"{"allapps":{"running":null,"last":{"verdict":"incomplete"}}}"#, &["allapps", "incomplete"]);
    rejects(r#"{"warm":{"plan":{"nodes":[{"progress":{"state":"building","ended_ms":1}}]}}}"#, &["building", "ended_ms=1"]);
    for state in ["done", "failed", "stored"] {
        let progress = format!(r#"{{"state":"{state}"}}"#);
        rejects(&format!(r#"{{"warm":{{"plan":{{"nodes":[{{"progress":{progress}}}]}}}}}}"#), &["prebuild", state]);
    }

    assert_eq!(verdict_ready(&parse("{}")), Ok(false));
    assert_eq!(verdict_ready(&parse(r#"{"verify":{"verified":[{}]},"ci":{"runs":[]}}"#)), Ok(false));
    assert_eq!(verdict_ready(&parse(r#"{"verify":{"verified":[{}]},"ci":{"runs":[{}]}}"#)), Ok(true));
    let failing = parse(r#"{"verify":{"failing":{"id":"broken"}}}"#);
    assert_eq!(verdict_ready(&failing).unwrap_err(), r#"verify failed (bisecting): {"id":"broken"}"#);
    let failed = parse(r#"{"verify":{"failing":null,"failures":[{"id":"broken"}],"verified":[{}]},"ci":{"runs":[{}]}}"#);
    assert_eq!(verdict_ready(&failed).unwrap_err(), r#"verify recorded a failure: {"id":"broken"}"#);
}

#[test]
fn a_master_restart_ends_nothing() {
    let chosen = restart_base_in(&std::env::temp_dir(), Path::new("/tmp"), std::process::id(), SOCKET_PATH_MAX).unwrap();
    let base = chosen.dir.clone();
    if let Some(warn) = &chosen.fallback_warning {
        eprintln!("{warn}");
        if let Some(marker) = &chosen.marker {
            let _ = std::fs::write(marker, format!("{}\n{warn}\n", base.display()));
        }
    }
    // Roles are declared later so their children are killed before cleanup, including on unwind.
    let base_guard = BaseGuard { dir: base.clone(), marker: chosen.marker, work_dirs: NODES.iter().map(|n| work_dir(&base, n)).collect() };
    let _ = std::fs::remove_dir_all(&base);
    let (bin, tmp, home, ran) = (base.join("bin"), base.join("tmp"), base.join("home"), base.join("ran"));
    let repo = base.join("repo");
    for d in [&bin, &tmp, &home.join("devhub"), &ran, &base.join("rt"), &repo.join("apps/a/src"), &repo.join("apps/b/src"), &repo.join("tools/ci_leg/src"), &repo.join("tools/run"), &repo.join("apps/commercial")] {
        std::fs::create_dir_all(d).unwrap();
    }
    // the tree: two apps the gate boots (each on Linux only), ci-leg, and a commercial side
    std::fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = [\"apps/*\", \"tools/ci_leg\"]\n").unwrap();
    for app in ["a", "b"] {
        std::fs::write(repo.join(format!("apps/{app}/Cargo.toml")), format!("[package]\nname = \"makepad-app-{app}\"\n")).unwrap();
        std::fs::write(repo.join(format!("apps/{app}/src/main.rs")), "fn main() {}\n").unwrap();
        std::fs::write(repo.join(format!("apps/{app}/ci.splash")), "// ci os: linux\nnil\n").unwrap();
    }
    std::fs::write(repo.join("tools/ci_leg/Cargo.toml"), "[package]\nname = \"makepad-ci-leg\"\n").unwrap();
    std::fs::write(repo.join("tools/ci_leg/src/main.rs"), "fn main() {}\n").unwrap();
    std::fs::write(repo.join("tools/run/Cargo.toml"), "[package]\nname = \"makepad-run\"\n").unwrap();
    let lock: String = ["makepad-app-a", "makepad-app-b", "makepad-ci-leg", "makepad-run"].iter().map(|name| format!("[[package]]\nname = {name:?}\nversion = \"0.1.0\"\n")).collect();
    std::fs::write(repo.join("Cargo.lock"), lock).unwrap();
    let com = repo.join("apps/commercial");
    git(&com, &["init", "-q", "-b", "main"]);
    git(&com, &["commit", "-q", "--allow-empty", "-m", "c"]);
    std::fs::write(repo.join(".gitignore"), "/apps/commercial/\n/Cargo.lock\n").unwrap();
    git(&repo, &["init", "-q", "-b", "work"]);
    git(&repo, &["config", "user.name", "CI Test"]);
    git(&repo, &["config", "user.email", "ci@example.invalid"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "w"]);
    let (work, main) = (git(&repo, &["rev-parse", "HEAD"]), git(&com, &["rev-parse", "HEAD"]));
    // the tools on the nodes: each run leaves a line behind, so a run started twice shows (an empty
    // CARGO_HOME keeps the box's rustup toolchain from going ahead of them in the runs' PATH)
    let release = base.join("release");
    let shell_base = format!("'{}'", base.to_string_lossy().replace('\'', "'\\''"));
    let once = |name: &str, body: &str| format!("#!/bin/sh\necho \"$*\" >> {shell_base}/ran/{name}\necho before\nwhile [ ! -e {shell_base}/release ]; do sleep 0.02; done\n{body}\n");
    // (a lease's `cargo run` of the JIT builds it first, `cargo build`, then runs what it built: this
    // script again, as the JIT; the JIT stage before a box's shards is `makepad --version`; a run
    // that asks holds its build until the release)
    let jit = format!("#!/bin/sh\ncase \"$1\" in\nbuild)\n  case \"$PWD\" in */prebuild-*) echo \"$*\" >> {shell_base}/ran/prebuild; echo building the box prebuild; while [ ! -e {shell_base}/release ]; do sleep 0.02; done;; esac\n  if [ -n \"$BUILD_HELD\" ]; then echo \"$*\" >> {shell_base}/ran/build; echo building; while [ ! -e {shell_base}/release ]; do sleep 0.02; done; fi\n  mkdir -p \"$CARGO_TARGET_DIR/host\" && cp \"$0\" \"$CARGO_TARGET_DIR/host/makepad\"; exit;;\n--version) echo makepad; exit 0;;\nbuilt-job) echo \"$*\" >> {shell_base}/ran/built-job; echo the built JIT ran; exit 0;;\nesac\n");
    script(&bin.join("cargo"), &once("cargo", "echo 0 errors\necho \"test result: ok. 1 passed; 0 failed\"\nexit 0").replacen("#!/bin/sh\n", &jit, 1));
    script(&bin.join("git"), &format!("#!/bin/sh\nif [ \"$1\" = -C ]; then echo {main}; else echo {work}; fi\n"));
    script(&bin.join("leg"), &once("leg", "echo TESTLEG-OK\nexit 0"));
    script(&bin.join("job"), &once("job", "echo after\nexit 7"));
    script(&bin.join("allapps"), &once("allapps", "echo allapps done\nexit 0"));
    // (ends at once: its verdict is in before the kill)
    script(&bin.join("quick"), &format!("#!/bin/sh\necho \"$*\" >> {shell_base}/ran/quick\necho quick ran\nexit 3\n"));

    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let role = |name: &str, test: &str| {
        let mut c = crate::testenv::role(test, &base);
        c.args(["--nocapture", "--test-threads=1"]).env("DEVHUB_RESTART_ROLE", name).env("DEVHUB_RESTART_BASE", &base).env("TMPDIR", &tmp).env("MAKEPAD_HOME", &home).env("CARGO_HOME", base.join("no-cargo-home")).env("DEVHUB_TEST_REPO", &repo).env("DEVHUB_TEST_LEG", bin.join("leg")).env("DEVHUB_TEST_ALLAPPS", bin.join("allapps")).stdin(Stdio::piped());
        c
    };
    let roles = std::cell::RefCell::new(Roles(Vec::new()));
    roles.borrow_mut().0.push(("nodes", role("nodes", "orch::restart_tests::restart_role_nodes").env("PATH", &path).env("XDG_RUNTIME_DIR", base.join("rt")).stderr(std::fs::File::create(base.join("nodes.log")).unwrap()).spawn().unwrap()));
    let sock = base.join("n6.sock");
    while !sock.exists() {
        if let Some(why) = roles.borrow_mut().ended() { panic!("the nodes did not come up: {why}"); }
        std::thread::sleep(Duration::from_millis(50));
    }
    let submit = format!("job submit {{\"tree\":{},\"rev\":{},\"jobs\":[{{\"kind\":\"test\",\"lane\":\"proof\",\"cmd\":[{}]}},{{\"kind\":\"test\",\"lane\":\"refused\",\"cmd\":[\"env\"]}},{{\"kind\":\"test\",\"lane\":\"built\",\"cmd\":[\"env\",\"BUILD_HELD=1\",\"cargo\",\"run\",\"--profile\",\"host\",\"-p\",\"makepad-run\",\"--bin\",\"makepad\",\"--\",\"built-job\"]}},{{\"kind\":\"test\",\"lane\":\"exited\",\"cmd\":[{}]}}]}}", json(&repo), makepad_remote::json_str(&format!("work@{work}")), json(&bin.join("job")), json(&bin.join("quick")));
    let mut first = role("master", "orch::restart_tests::restart_role_master");
    first.env("DEVHUB_RESTART_SUBMIT", submit);
    roles.borrow_mut().0.push(("first master", first.spawn().unwrap()));

    // everything is on its box, running
    let state = || {
        // Decode the durable document, including its file-backed completed records.
        // The assertions below still check the same run/lease identities and exact output.
        crate::state::Store::open_quiet(&home.join("devhub"), "").ok().map(|store| store.read(crate::state::Doc::to_value))
    };
    let fail = |what: &str, why: &str, seen: Option<&Value>| -> ! {
        let now = seen.map(Value::to_json).unwrap_or_else(|| {
            crate::state::committed(&home.join("devhub")).map(|v| v.map(|v| v.to_json()).unwrap_or_default()).unwrap_or_else(|e| e)
        });
        // The durable state leaves out the master's leases, including why a lease waits.
        let client = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(home.join("devhub/devhub.sock")), "observe");
        let leases = crate::cli::ask(&client, None, "leases", "leases all").map(|l| l.join("\n")).unwrap_or_else(|e| e);
        panic!("{what}: {why}\nstate: {now}\nthe master's leases:\n{leases}");
    };
    let until = |what: &str, f: &dyn Fn(&Value) -> Result<bool, String>| {
        loop {
            if let Some(s) = state() {
                match f(&s) {
                    Ok(true) => return,
                    Ok(false) => {}
                    Err(why) => fail(what, &why, Some(&s)),
                }
            }
            if let Some(why) = roles.borrow_mut().ended() {
                fail(what, &why, None);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let count = |name: &str| std::fs::read_to_string(ran.join(name)).unwrap_or_default().lines().count();
    until("a job, a leg, every shard and allapps running", &|s| pre_kill_ready(s, count("build"), count("prebuild")));
    let s = state().unwrap();
    let mid_build = job_lane(&s, "built").unwrap();
    assert_eq!(json_num(&mid_build, "run_n"), 0, "killed before its command: {}", mid_build.to_json());
    let (job_lease, leg_lease, shard) = (json_text(&job0(&s).unwrap(), "lease"), json_text(&leg0(&s).unwrap(), "lease"), shards_of(&s).into_iter().find(|x| json_num(x, "run_n") > 0).unwrap());
    let allapps_lease = json_text(&allapps_running(&s).unwrap(), "lease");
    let prebuild_lease = json_text(&prebuild(&s).unwrap(), "lease");
    let (shard_id, shard_lease) = (json_text(&shard, "id"), json_text(&shard, "lease"));
    // the boxes those leases are on
    let mut held: Vec<String> = [job0(&s).unwrap(), mid_build.clone(), leg0(&s).unwrap(), allapps_running(&s).unwrap()].iter().chain(shards_of(&s).iter().filter(|x| !json_text(x, "lease").is_empty())).map(|x| json_text(x, "node")).collect();
    // (and the prebuild's box)
    held.push("n6".into());
    held.sort();
    held.dedup();
    assert!(!held.is_empty() && held.iter().all(|n| NODES.contains(&n.as_str())), "{held:?}: {}", s.to_json());

    // a client waits for the job: the kill cuts its call, and it asks again until it has the result
    let socket = home.join("devhub/devhub.sock");
    let waiter = std::thread::spawn(move || {
        let client = makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(socket.clone()), "observe");
        let (mut cut, mut lines) = (0, Vec::new());
        while !lines.iter().any(|l: &String| l.contains("\"state\":\"failed\"")) {
            lines = crate::cli::retry_master_wait(|| {
                let answer = crate::cli::ask(&client, None, "job", "job wait J1");
                cut += answer.is_err() as u32;
                answer
            }, || {
                std::thread::sleep(Duration::from_millis(50));
                makepad_remote::link::Client::new(makepad_remote::link::Target::Unix(socket.clone()), "observe").conn().is_ok()
            }).unwrap();
        }
        cut
    });

    // the master dies under all three, and comes back on its state
    let (_, mut master) = { let mut r = roles.borrow_mut(); let i = r.0.iter().position(|(name, _)| *name == "first master").unwrap(); r.0.remove(i) };
    let _ = master.kill();
    let _ = master.wait();
    // the quick job as a master leaves it that dies between its lease's release and its verdict
    // ([`super::job`]'s `ended`): its run's exit noted, its lease gone from the live set, the
    // verdict not written (J16837 ran again after a restart there, 11-23)
    let quick = job_lane(&state().unwrap(), "exited").unwrap();
    let quick_lease = json_text(&quick, "lease");
    // (a cleanup its box has not confirmed yet keeps it there as `cleaning`, never held)
    let live = std::fs::read_to_string(home.join("devhub/leases.live")).unwrap_or_default();
    assert!(!quick_lease.is_empty() && !live.lines().any(|l| l.contains(&format!("\"id\":\"{quick_lease}\"")) && !l.contains("\"state\":\"cleaning\"")), "released: {}\n{live}", quick.to_json());
    crate::state::Store::open_quiet(&home.join("devhub"), "").unwrap().update(|d| {
        let j = d.jobs.iter_mut().find(|j| j.lane == "exited").unwrap();
        (j.state, j.ended_ms, j.log) = ("running".into(), 0, String::new());
        (j.errors, j.tail) = (Vec::new(), Vec::new());
    }).unwrap();
    // and its lease's end as a master leaves it that dies before its ledger line: the line kept
    // in the live set, none in the ledger (L1596652 had none after a restart, 11-23)
    let ledger = home.join("devhub/leases.jsonl");
    let named = format!("\"id\":\"{quick_lease}\"");
    let lines = std::fs::read_to_string(&ledger).unwrap();
    let quick_line = lines.lines().find(|l| l.contains(&named)).unwrap_or_else(|| panic!("{quick_lease} in the ledger: {lines}")).to_string();
    std::fs::write(&ledger, lines.lines().filter(|l| !l.contains(&named)).map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    let mut kept = std::fs::OpenOptions::new().append(true).open(home.join("devhub/leases.live")).unwrap();
    std::io::Write::write_all(&mut kept, format!("{}\n", crate::lease::sched::Sched::unledgered_line(&quick_lease, &quick_line)).as_bytes()).unwrap();
    // and its costs line likewise: kept in the live set, none in costs.jsonl (a lease whose end a
    // restart cut had none, and its class lost the run's measure, 11-23)
    let costs = home.join("devhub/costs.jsonl");
    let costed = format!("\"lease\":\"{quick_lease}\"");
    let lines = std::fs::read_to_string(&costs).unwrap();
    let quick_cost = lines.lines().find(|l| l.contains(&costed)).unwrap_or_else(|| panic!("{quick_lease} in costs.jsonl: {lines}")).to_string();
    std::fs::write(&costs, lines.lines().filter(|l| !l.contains(&costed)).map(|l| format!("{l}\n")).collect::<String>()).unwrap();
    std::io::Write::write_all(&mut kept, format!("{}\n", crate::lease::sched::Sched::uncosted_line(&quick_lease, &quick_cost)).as_bytes()).unwrap();
    // gone past the nodes' no-beat limit: each box holding its leases says it keeps them for it.
    // Their word, not a time: when a box's watchdog sees it gone depends on the box's load.
    // Readiness below, not elapsed time either, releases the tools
    let away = |n: &String| format!(": no call from it on {n};");
    until("every box holding its leases saw the master away", &|_| {
        let log = std::fs::read_to_string(base.join("nodes.log")).unwrap_or_default();
        Ok(held.iter().all(|n| log.contains(&away(n))))
    });
    let ready = base.join("master.log");
    roles.borrow_mut().0.push(("restarted master", role("master", "orch::restart_tests::restart_role_master").stderr(std::fs::File::create(&ready).unwrap()).spawn().unwrap()));
    // only the new master writes this log, after restoring leases and starting every workflow
    until("the restarted master ready", &|_| Ok(std::fs::read_to_string(&ready).unwrap_or_default().lines().any(|l| l == "restart master ready")));
    std::fs::write(&release, "").unwrap();

    until("the job's result", &|s| Ok(job0(s).is_some_and(|j| job_terminal(&json_text(&j, "state")))));
    let s = state().unwrap();
    let j = job0(&s).unwrap();
    assert_eq!((json_text(&j, "state").as_str(), json_num(&j, "exit"), json_text(&j, "lease"), json_num(&j, "attempts")), ("failed", 7, job_lease, 1), "{}", j.to_json());
    let log = std::fs::read_to_string(home.join("devhub/jobs/J1.log")).unwrap();
    assert_eq!(log, "before\nafter\n", "the job's whole output, once");
    assert!(waiter.join().unwrap() >= 1, "the kill cut the waiting client's call");
    // a run its node refuses is the job's result on its first box, not a lost box placed again
    until("the refused job's result", &|s| Ok(job_lane(s, "refused").is_some_and(|j| job_terminal(&json_text(&j, "state")))));
    let refused = job_lane(&state().unwrap(), "refused").unwrap();
    assert_eq!((json_text(&refused, "state").as_str(), json_num(&refused, "attempts")), ("failed", 1), "{}", refused.to_json());
    assert!(json_text(&refused, "note").contains("env needs a command"), "{}", refused.to_json());

    // a job the master was killed under while its lease built its JIT: asked again under its name
    // it joined that build, and its command ran on what was built, not that build as its command
    until("the built job's result", &|s| Ok(job_lane(s, "built").is_some_and(|j| job_terminal(&json_text(&j, "state")))));
    let j = job_lane(&state().unwrap(), "built").unwrap();
    assert_eq!((json_text(&j, "state").as_str(), json_num(&j, "exit"), json_text(&j, "lease"), json_num(&j, "attempts"), json_num(&j, "run_n")), ("ok", 0, json_text(&mid_build, "lease"), 1, 1), "{}", j.to_json());
    let log = std::fs::read_to_string(home.join(format!("devhub/jobs/{}.log", json_text(&j, "id")))).unwrap();
    assert!(log.contains("devhub: cargo built this tree's JIT in") && log.contains("the built JIT ran"), "{log}");
    assert_eq!((count("build"), count("built-job")), (1, 1), "one build, one command: {log}");

    // the job whose run had ended: its noted exit is its result, its command not run again
    until("the exited job's result", &|s| Ok(job_lane(s, "exited").is_some_and(|j| job_terminal(&json_text(&j, "state")))));
    let j = job_lane(&state().unwrap(), "exited").unwrap();
    assert_eq!((json_text(&j, "state").as_str(), json_num(&j, "exit"), json_text(&j, "lease"), json_num(&j, "attempts"), count("quick")), ("failed", 3, quick_lease, 1, 1), "{}", j.to_json());
    // and its lease's ledger line, written once by the restarted master, marked cut
    let cut: Vec<String> = std::fs::read_to_string(&ledger).unwrap().lines().filter(|l| l.contains(&named)).map(str::to_string).collect();
    assert!(cut.len() == 1 && cut[0].contains("\"cut\":\"restart\"") && cut[0].strip_suffix(",\"cut\":\"restart\"}") == quick_line.strip_suffix('}'), "{quick_line}\n{cut:?}");
    let cut: Vec<String> = std::fs::read_to_string(&costs).unwrap().lines().filter(|l| l.contains(&costed)).map(str::to_string).collect();
    assert!(cut.len() == 1 && cut[0].strip_suffix(",\"cut\":\"restart\"}") == quick_cost.strip_suffix('}'), "its costs line, written once, marked cut: {quick_cost}\n{cut:?}");

    until("the verdict", &verdict_ready);
    // and the legs' run, which nothing waits for
    until("the legs' end", &|s| Ok(jitlegs_run(s).is_some_and(|r| json_num(&r, "ended_ms") > 0)));
    let s = state().unwrap();
    assert_eq!(verdict_ready(&s), Ok(true), "{}", s.get("verify").unwrap().to_json());
    let runs = s.get("ci").and_then(|c| c.get("runs")).and_then(|r| r.as_arr()).unwrap();
    let run = runs.last().unwrap();
    assert_eq!((runs.len(), json_text(run, "verdict").as_str()), (1, "ok"), "one CI run, to its verdict: {}", run.to_json());
    let shards = run.get("shards").and_then(|x| x.as_arr()).unwrap();
    assert!(shards.iter().any(|x| json_text(x, "id") == shard_id && json_text(x, "lease") == shard_lease), "shard {shard_id} kept its lease {shard_lease}: {}", run.to_json());
    // each command ran exactly once: nothing started over
    assert_eq!(count("job"), 1, "the job ran once");
    assert_eq!(count("leg"), 1, "the leg ran once");
    let planned: usize = shards.len(); // one boot each
    assert_eq!(count("cargo"), planned, "each shard's steps ran once: {}", run.to_json());
    // the master's own run went on on its lease, once
    until("allapps' verdict", &|s| Ok(allapps_running(s).is_none() && allapps_last(s).is_some()));
    let last = allapps_last(&state().unwrap()).unwrap();
    assert_eq!((json_text(&last, "verdict").as_str(), json_text(&last, "lease")), ("ok", allapps_lease.clone()), "{}", last.to_json());
    assert_eq!(count("allapps"), 1, "allapps ran once");
    // the box's prebuild went on on its lease under the new master, its build once, and its JIT
    // ran on what that build made (`--version`): the restart lost none of it
    until("the prebuild's end", &|s| Ok(prebuild(s).is_some_and(|p| json_num(&p, "ended_ms") > 0)));
    let p = prebuild(&state().unwrap()).unwrap();
    assert_eq!((json_text(&p, "state").as_str(), count("prebuild")), ("done", 1), "the prebuild finished, its build run once: {}", p.to_json());
    let warm_log = std::fs::read_to_string(home.join("devhub/warm/n6.log")).unwrap_or_default();
    assert!(warm_log.contains(&format!("{prebuild_lease} on n6: taken back after the master's restart")), "taken back on its lease {prebuild_lease}: {warm_log}");
    assert!(warm_log.contains("devhub: cargo built this tree's JIT in") && warm_log.contains("makepad\n"), "its build made the JIT its step ran: {warm_log}");
    let ended = std::fs::read_to_string(home.join("devhub/leases.jsonl")).unwrap_or_default();
    assert!(ended.lines().any(|l| l.contains(&format!("\"id\":\"{leg_lease}\""))), "the leg kept its lease {leg_lease}: {ended}");
    drop(roles);
    drop(base_guard);
}

fn json(p: &std::path::Path) -> String {
    makepad_remote::json_str(&p.to_string_lossy())
}
