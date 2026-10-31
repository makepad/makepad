//! A master restart ends nothing, end to end: a real master process (leases, jobs, verify, CI) over
//! real node executors in another process, killed under a job, a verify leg and a CI shard and
//! started again on its state. All three finish with their real exit codes, each command run once.

use super::*;
use crate::lease::exec::{Exec, Opts, WORDS};
use crate::lease::master::SERVICES as LEASE_SERVICES;
use makepad_remote::hosts::{Host, Hosts, Via};
use makepad_remote::hub::Hub;
use makepad_remote::{lines, Endpoint};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const NODES: [&str; 6] = ["n1", "n2", "n3", "n4", "n5", "n6"];

fn role_env(name: &str) -> PathBuf {
    PathBuf::from(std::env::var_os(name).unwrap_or_default())
}

/// The hub side: the executors of [`NODES`], each on its socket. Its PATH has the fake tools.
#[test]
fn restart_role_nodes() {
    if std::env::var_os("DEVHUB_RESTART_ROLE").is_none_or(|r| r != "nodes") {
        return;
    }
    let base = role_env("DEVHUB_RESTART_BASE");
    let mut keep = Vec::new();
    for name in NODES {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        // (beside the test binary: a build lease asks for more disk than /tmp may hold)
        let work = work_dir(&base, name);
        let conf = crate::lease::caps::LeaseConf::parse(&format!("work = {}\nrole = pool\ndisk_floor_gb = 0\ndisk_warn_gb = 0\n", work.display()), &dir);
        let x = Exec::new(name, conf, dir.join("leases"), Opts { scopes: Some(false), ..Opts::default() }, Arc::new(|_| {}));
        let ep = Endpoint::new(name);
        for &word in WORDS {
            let e = x.clone();
            ep.register(word, 1, lines::service(word, Arc::new(move |line, out| {
                // no checkout here: a lease at a revision runs in its lane's dir
                let line = if word == "lease-start" { unpinned(line) } else { line.to_string() };
                out.extend(e.handle(word, &line));
            })));
        }
        makepad_remote::carrier::unix::listen_at(&ep, &base.join(format!("{name}.sock"))).unwrap();
        keep.push((x, ep));
    }
    std::thread::sleep(Duration::from_secs(600));
}

fn work_dir(base: &std::path::Path, node: &str) -> PathBuf {
    std::env::current_exe().unwrap().with_file_name(format!("{}-{node}", base.file_name().unwrap().to_string_lossy()))
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
    let rev: crate::lease::master::EnsureRev = Arc::new(|_: &str, _: &str, _: &std::path::Path, pin: &crate::lease::RevPin| Ok(pin.sha.clone()));
    let leases = Arc::new(LeaseMaster::new("mac", crate::proto::home(), ask, Arc::new(|_| {}), Arc::new(crate::proto::wall_ms), Some(rev), false));
    for svc in LEASE_SERVICES {
        let l = leases.clone();
        ep.register(svc, 1, lines::service(svc, Arc::new(move |line, out| l.serve(svc, line, out))));
    }
    start(&ep, "mac", leases.clone(), Arc::new(|_| {}));
    crate::node::listen_control(&ep).unwrap();
    let names: Vec<(String, String)> = hosts.hosts.iter().map(|h| (h.name.clone(), match &h.via { Via::Fleet { addr } => addr.clone(), _ => String::new() })).collect();
    leases.start(Arc::new(move || names.clone()));
    let ctx = ctx().unwrap().clone();
    if let Some(submit) = std::env::var_os("DEVHUB_RESTART_SUBMIT") {
        job::serve(&ctx, &submit.to_string_lossy()).unwrap();
    }
    job::start(ctx.clone());
    ci::start(&ctx);
    verify::start(ctx);
    std::thread::sleep(Duration::from_secs(600));
}

fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let o = Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

fn script(path: &std::path::Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Kills the role processes it started, by the pids it holds.
struct Roles(Vec<Child>);

impl Drop for Roles {
    fn drop(&mut self) {
        for c in &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[test]
fn a_master_restart_ends_nothing() {
    let base = PathBuf::from(format!("/tmp/dhr{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let (bin, tmp, home, ran) = (base.join("bin"), base.join("tmp"), base.join("home"), base.join("ran"));
    let repo = base.join("repo");
    for d in [&bin, &tmp, &home.join("devhub"), &ran, &repo.join("platform/b/src"), &repo.join("platform/src"), &repo.join("apps/commercial")] {
        std::fs::create_dir_all(d).unwrap();
    }
    // the tree: two crates the shards are planned from, and a commercial side
    std::fs::write(repo.join("Cargo.toml"), "[workspace]\nmembers = [\"platform\", \"platform/b\"]\n").unwrap();
    std::fs::write(repo.join("platform/Cargo.toml"), "[package]\nname = \"p-a\"\n").unwrap();
    std::fs::write(repo.join("platform/src/lib.rs"), "").unwrap();
    std::fs::write(repo.join("platform/b/Cargo.toml"), "[package]\nname = \"p-b\"\n").unwrap();
    std::fs::write(repo.join("platform/b/src/lib.rs"), "").unwrap();
    let com = repo.join("apps/commercial");
    git(&com, &["init", "-q", "-b", "main"]);
    git(&com, &["commit", "-q", "--allow-empty", "-m", "c"]);
    std::fs::write(repo.join(".gitignore"), "/apps/commercial/\n").unwrap();
    git(&repo, &["init", "-q", "-b", "work"]);
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "w"]);
    let (work, main) = (git(&repo, &["rev-parse", "HEAD"]), git(&com, &["rev-parse", "HEAD"]));
    // the tools on the nodes: each run leaves a line behind, so a run started twice shows
    let once = |name: &str, body: &str| format!("#!/bin/sh\necho \"$*\" >> {}/{name}\n{body}\n", ran.display());
    script(&bin.join("cargo"), &once("cargo", "echo before\nsleep 5\necho 0 errors\nexit 0"));
    script(&bin.join("git"), &format!("#!/bin/sh\nif [ \"$1\" = -C ]; then echo {main}; else echo {work}; fi\n"));
    script(&bin.join("leg"), &once("leg", "echo before\nsleep 12\necho TESTLEG-OK\nexit 0"));
    script(&bin.join("job"), &once("job", "echo before\nsleep 12\necho after\nexit 7"));

    let exe = std::env::current_exe().unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let role = |name: &str, test: &str| {
        let mut c = Command::new(&exe);
        c.args(["--exact", test, "--nocapture", "--test-threads=1"]).env("DEVHUB_RESTART_ROLE", name).env("DEVHUB_RESTART_BASE", &base).env("TMPDIR", &tmp).env("MAKEPAD_HOME", &home).env("DEVHUB_TEST_REPO", &repo).env("DEVHUB_TEST_LEG", bin.join("leg")).stdin(Stdio::null());
        c
    };
    let mut roles = Roles(Vec::new());
    roles.0.push(role("nodes", "orch::restart_tests::restart_role_nodes").env("PATH", &path).spawn().unwrap());
    let sock = base.join("n6.sock");
    let t = Instant::now();
    while !sock.exists() {
        assert!(t.elapsed() < Duration::from_secs(30), "the nodes did not come up");
        std::thread::sleep(Duration::from_millis(50));
    }
    let submit = format!("job submit {{\"tree\":{},\"rev\":{},\"jobs\":[{{\"kind\":\"test\",\"lane\":\"proof\",\"cmd\":[{}]}}]}}", json(&repo), makepad_remote::json_str(&format!("work@{work}")), json(&bin.join("job")));
    let mut first = role("master", "orch::restart_tests::restart_role_master");
    first.env("DEVHUB_RESTART_SUBMIT", submit);
    let mut master = first.spawn().unwrap();

    // everything is on its box, running
    let state = || {
        let text = std::fs::read(home.join("devhub/state.json")).unwrap_or_default();
        makepad_strict_json::parse_depth(&text, 32).ok()
    };
    let num = |v: &makepad_strict_json::Value, k: &str| v.get(k).and_then(crate::lease::num).unwrap_or(0.0) as u64;
    let text = |v: &makepad_strict_json::Value, k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let until = |what: &str, f: &dyn Fn(&makepad_strict_json::Value) -> bool| {
        let t = Instant::now();
        loop {
            if state().is_some_and(|s| f(&s)) {
                return;
            }
            let now = String::from_utf8_lossy(&std::fs::read(home.join("devhub/state.json")).unwrap_or_default()).into_owned();
            assert!(t.elapsed() < Duration::from_secs(180), "{what}: {now}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let job0 = |s: &makepad_strict_json::Value| s.get("jobs").and_then(|j| j.as_arr()).and_then(|j| j.first()).cloned();
    let leg0 = |s: &makepad_strict_json::Value| s.get("verify").and_then(|v| v.get("run")).and_then(|r| r.get("legs")).and_then(|l| l.as_arr()).and_then(|l| l.first()).cloned();
    let shard_running = |s: &makepad_strict_json::Value| s.get("ci").and_then(|c| c.get("run")).and_then(|r| r.get("shards")).and_then(|x| x.as_arr()).and_then(|x| x.iter().find(|x| num(x, "run_n") > 0).cloned());
    until("a job, a leg and a shard running", &|s| job0(s).is_some_and(|j| num(&j, "run_n") > 0) && leg0(s).is_some_and(|l| num(&l, "run_n") > 0) && shard_running(s).is_some());
    let s = state().unwrap();
    let (job_lease, leg_lease, shard) = (text(&job0(&s).unwrap(), "lease"), text(&leg0(&s).unwrap(), "lease"), shard_running(&s).unwrap());
    let (shard_id, shard_lease) = (text(&shard, "id"), text(&shard, "lease"));

    // the master dies under all three, and comes back on its state
    let _ = master.kill();
    let _ = master.wait();
    let down = Duration::from_secs(2);
    std::thread::sleep(down);
    roles.0.push(role("master", "orch::restart_tests::restart_role_master").spawn().unwrap());

    until("the job's result", &|s| job0(s).is_some_and(|j| !matches!(text(&j, "state").as_str(), "queued" | "leasing" | "running")));
    let s = state().unwrap();
    let j = job0(&s).unwrap();
    assert_eq!((text(&j, "state").as_str(), num(&j, "exit"), text(&j, "lease"), num(&j, "attempts")), ("failed", 7, job_lease, 1), "{}", j.to_json());
    let log = std::fs::read_to_string(home.join("devhub/jobs/J1.log")).unwrap();
    assert_eq!(log, "before\nafter\n", "the job's whole output, once");

    let verified = |s: &makepad_strict_json::Value| s.get("verify").and_then(|v| v.get("verified")).and_then(|v| v.as_arr()).is_some_and(|v| !v.is_empty());
    until("the verdict", &|s| verified(s) || s.get("verify").and_then(|v| v.get("failing")).is_some_and(|f| !f.is_null()));
    let s = state().unwrap();
    assert!(verified(&s), "{}", s.get("verify").unwrap().to_json());
    let runs = s.get("ci").and_then(|c| c.get("runs")).and_then(|r| r.as_arr()).unwrap();
    let run = runs.last().unwrap();
    assert_eq!((runs.len(), text(run, "verdict").as_str()), (1, "ok"), "one CI run, to its verdict: {}", run.to_json());
    let shards = run.get("shards").and_then(|x| x.as_arr()).unwrap();
    assert!(shards.iter().any(|x| text(x, "id") == shard_id && text(x, "lease") == shard_lease), "shard {shard_id} kept its lease {shard_lease}: {}", run.to_json());
    // each command ran exactly once: nothing started over
    let count = |name: &str| std::fs::read_to_string(ran.join(name)).unwrap_or_default().lines().count();
    assert_eq!(count("job"), 1, "the job ran once");
    assert_eq!(count("leg"), 1, "the leg ran once");
    let planned: usize = shards.len() + 1; // jit:01 has both crates' checks
    assert_eq!(count("cargo"), planned, "each shard's steps ran once: {}", run.to_json());
    let ended = std::fs::read_to_string(home.join("devhub/leases.jsonl")).unwrap_or_default();
    assert!(ended.lines().any(|l| l.contains(&format!("\"id\":\"{leg_lease}\""))), "the leg kept its lease {leg_lease}: {ended}");
    drop(roles);
    for name in NODES {
        let _ = std::fs::remove_dir_all(work_dir(&base, name));
    }
    let _ = std::fs::remove_dir_all(&base);
}

fn json(p: &std::path::Path) -> String {
    makepad_remote::json_str(&p.to_string_lossy())
}
