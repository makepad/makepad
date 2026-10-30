//! The lease words of `makepad devhub`: the holder side of a lease (it beats the master every
//! 2 s while it holds one; a holder that dies lets its lease expire), `run` (ship the worktree,
//! run under a lease, stream the output, exit with its code), and the views and switches.
//!
//! Exit codes: 0 ok, 1 the command failed, 2 usage or denied.

use super::master::b64_decode;
use super::sched::{ordinal, parse_needs, Req};
use super::Kind;
use crate::cli::{ask, control, has, local_name};
use makepad_remote::link::Client;
use makepad_strict_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const HELP: &str = "  lease <kind> [--node N|--needs k=v,..] [--lane L] [--prio P] [--rev <branch>[@sha][,<commercial>[@sha]]]
        [--repo-dir D] [--watch-pid P] [-- cmd...]
                                          hold a lease (kinds: build test screen long timed gpu inspect gate;
                                          --needs gui=1: a test that opens windows, only nodes with screen slots);
                                          with a cmd: run it there and exit with its code
  run <lane> <kind> [--node N|--needs ..] -- cmd...   ship this worktree to the node, run the cmd under a lease
  get <lane> <rel> <local-dir> [--node N] [--lease L]
                                          copy results back from the lane's dir on its node (`src/..`: the tree
                                          `run` ships, `target/..`: its target; with --lease: that live lease's
                                          work, `target/..`, `tmp/..`)
  release <id>                            end a lease (says how its node cleaned up)
  lease <id> cancel | cancel <id>         the user's cancel of ONE lease: its processes stop, its slot frees, the
                                          lane is kept off that node for 30 min (unless it names it with --node)
  override <node> [--to <lane>]           take the node now: its leases end; to the user (dev off) or a lane
  evict <lane|agent>                      end every lease of it, clean up, block new ones; unblock <lane|agent>
  blocked                                 the lanes and agents the user blocked
  queue | leases [--all]                  live and waiting leases, positions and reasons
  nodes                                   capabilities, dev switch and leases per node
  node <n> dev on|off                     the node's dev switch (the user's decision)
  node <n> [role pool|suite|bench|user-wm|device] [set slots.build=3 disk_floor_gb=40 ...]
                                          the node's role and budgets (its lease.conf, kept on the node)
  disk <node> [--dry-run]                 the node's disk categories and reclaim
  desk <node> [on|off]                    the Desk Link endpoint of a JIT host (its own render, app input)
  landed <lane> | clean <lane>            delete the lane's dirs on every node and its merged Mac worktree";

/// Exit code of a `run`/`lease` whose lease the user cancelled (0 ok, 1 failed, 2 usage or denied).
/// Stderr then says `LEASE CANCELLED by user on <node>: pick another node ...`; a lane's agent
/// reads that as: choose another node and retry, and not retry on this one unless asked.
pub const CANCELLED_EXIT: i32 = 3;

fn cancelled_notice(id: &str, node: &str) -> String {
    format!("LEASE CANCELLED by user on {node} ({id}): pick another node (devhub nodes) and retry; don't retry on {node} unless asked. Exit code {CANCELLED_EXIT}.")
}

const BEAT: Duration = Duration::from_secs(2);
/// How long a holder waits out an unreachable master (a restart: it takes its granted leases back
/// from `leases.live`, and the nodes keep running them) before it gives up.
const REATTACH_FOR: Duration = Duration::from_secs(900);
/// After the master is back, how long its errors may still be its node links coming up.
const SETTLE_FOR: Duration = Duration::from_secs(60);

/// Calls `f` with the master's control client, through master restarts: while the master is not
/// reachable (`up` fails) the call is tried again with backoff (0.5 s to 2 s; the master expires a
/// silent holder after 10 s), and once it is back with a fresh client, re-attaching by the lease
/// id. A live master's answer stands, except while its node links come back (`SETTLE_FOR`); "no
/// lease" always stands. Said once each way on stderr.
fn through_restarts<T>(id: &str, c: &mut Client, mut f: impl FnMut(&Client) -> Result<T, String>, up: &dyn Fn() -> Result<Client, String>) -> Result<T, String> {
    let mut down: Option<Instant> = None;
    let mut back: Option<Instant> = None;
    let mut wait = Duration::from_millis(500);
    loop {
        let e = match f(c) {
            Ok(v) => {
                if down.take().is_some() {
                    eprintln!("{id}: the master is back; re-attached");
                }
                return Ok(v);
            }
            Err(e) => e,
        };
        if e.starts_with("no lease") {
            return Err(e);
        }
        match up() {
            Ok(fresh) => {
                *c = fresh;
                match (down, back) {
                    (None, _) => return Err(e),
                    (Some(_), None) => back = Some(Instant::now()),
                    (Some(_), Some(t)) if t.elapsed() > SETTLE_FOR => return Err(e),
                    _ => {}
                }
            }
            Err(why) => {
                back = None;
                match down {
                    None => {
                        eprintln!("{id}: the master is not reachable ({why}); re-attaching when it is back");
                        down = Some(Instant::now());
                    }
                    Some(t) if t.elapsed() > REATTACH_FOR => return Err(format!("{e} (the master stayed unreachable for {} s)", REATTACH_FOR.as_secs())),
                    _ => {}
                }
            }
        }
        std::thread::sleep(wait);
        wait = (wait * 2).min(Duration::from_secs(2));
    }
}

fn fail(e: impl std::fmt::Display) -> i32 {
    eprintln!("makepad devhub: {e}");
    1
}

fn json(l: &str) -> Option<Value> {
    makepad_strict_json::parse(l.as_bytes()).ok()
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn n(v: &Value, k: &str) -> f64 {
    v.get(k).and_then(super::num).unwrap_or(0.0)
}

/// One request to a master service; its first line as JSON (an `err` line is the error).
fn one(c: &Client, service: &str, line: &str) -> Result<Value, String> {
    let lines = ask(c, None, service, line)?;
    let first = lines.first().ok_or_else(|| format!("{service}: no answer"))?;
    let v = json(first).ok_or_else(|| format!("{service}: bad answer"))?;
    match v.get("err").and_then(|e| e.as_str()) {
        Some(e) => Err(e.to_string()),
        None => Ok(v),
    }
}

/// One request to a node service through the master's router.
fn node_one(c: &Client, node: &str, service: &str, line: &str) -> Result<Value, String> {
    let lines = ask(c, Some(node), service, line)?;
    let first = lines.first().ok_or_else(|| format!("{node} {service}: no answer"))?;
    let v = json(first).ok_or_else(|| format!("{node} {service}: bad answer"))?;
    match v.get("err").and_then(|e| e.as_str()) {
        Some(e) => Err(format!("{node}: {e}")),
        None => Ok(v),
    }
}

/// The master's control socket, or why there is none.
fn master() -> Result<Client, String> {
    let c = control()?;
    if !has(&c, "lease") {
        return Err("the devhub here is not the master (leases live on the master)".into());
    }
    Ok(c)
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let o = makepad_remote::group::command("git").arg("-C").arg(dir).args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// `--lane`, else `DEVHUB_LANE`, else the worktree's directory name.
fn default_lane() -> String {
    std::env::var("DEVHUB_LANE").ok().filter(|l| !l.is_empty()).unwrap_or_else(|| {
        let top = git(&cwd(), &["rev-parse", "--show-toplevel"]).map(PathBuf::from).unwrap_or_else(cwd);
        top.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default()
    })
}

fn pid_alive(pid: i32) -> bool {
    crate::os::pid_alive(pid as u32)
}

struct Opts {
    req: Req,
    watch: Option<i32>,
    cmd: Vec<String>,
}

/// The flags shared by `lease` and `run`.
fn parse(kind: &str, lane: Option<&str>, args: &[&str]) -> Result<Opts, String> {
    let kind = Kind::parse(kind).ok_or_else(|| format!("unknown kind `{kind}`"))?;
    let mut req = Req::new(lane.unwrap_or(""), kind);
    let mut watch = None;
    let mut i = 0;
    let mut cmd = Vec::new();
    while i < args.len() {
        let val = |i: usize| args.get(i + 1).copied().ok_or_else(|| format!("{} needs a value", args[i]));
        match args[i] {
            "--" => {
                cmd = args[i + 1..].iter().map(|a| a.to_string()).collect();
                break;
            }
            "--node" => req.node = Some(val(i)?.to_string()),
            "--needs" => req.needs = parse_needs(val(i)?),
            "--lane" => req.lane = val(i)?.to_string(),
            "--prio" => req.prio = val(i)?.parse().map_err(|_| "--prio takes a number")?,
            "--rev" => req.rev = Some(val(i)?.to_string()),
            "--repo-dir" => req.repo_dir = Some(val(i)?.to_string()),
            "--watch-pid" => watch = Some(val(i)?.parse().map_err(|_| "--watch-pid takes a pid")?),
            "--jit" => req.jit = Some(val(i)?.to_string()),
            a => return Err(format!("unknown flag `{a}`")),
        }
        i += 2;
    }
    if req.lane.is_empty() {
        req.lane = default_lane();
    }
    // the lane's repo: revisions are looked up and recovered work lands there
    if req.repo_dir.is_none() {
        req.repo_dir = git(&cwd(), &["rev-parse", "--show-toplevel"]);
    }
    // branch names become commits here, once: what ships is what was ordered, however long the
    // lease queues and whatever other landings do to the branches meanwhile
    if let Some(spec) = &req.rev {
        let dir = req.repo_dir.as_deref().ok_or("--rev needs a git repo (--repo-dir)")?;
        req.rev = Some(super::pin_revs(Path::new(dir), spec)?);
    }
    Ok(Opts { req, watch, cmd })
}

/// A granted lease as the holder knows it.
struct Held {
    id: String,
    node: String,
    work: String,
    env: Vec<(String, String)>,
}

/// Asks for the lease and waits for it (one stderr line per change); Err(exit code).
fn acquire(c: &Client, req: &Req) -> Result<Held, i32> {
    // this process holds it: the master ends the lease as soon as this pid is gone
    let mut req = req.clone();
    req.holder = Some(std::process::id());
    if req.agent.is_none() {
        req.agent = std::env::var("DEVHUB_AGENT").ok().filter(|a| crate::trust::valid_name(a));
    }
    let req = &req;
    let mut v = one(c, "lease", &req.to_json()).map_err(|e| {
        eprintln!("makepad devhub lease: {e}");
        2
    })?;
    let mut id = s(&v, "id");
    let mut client = c.clone();
    let mut last = String::new();
    let t0 = Instant::now();
    loop {
        let state = s(&v, "state");
        let line = match state.as_str() {
            "queued" => format!("{id} {}: {} in queue ({})", req.kind.as_str(), ordinal(n(&v, "pos") as usize), s(&v, "reason")),
            "starting" => format!("{id} {}@{}: granted, setting up", req.kind.as_str(), s(&v, "node")),
            "granted" => {
                let start = v.get("start");
                let env = match start.and_then(|st| st.get("env")) {
                    Some(Value::Obj(pairs)) => pairs.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or("").to_string())).collect(),
                    _ => Vec::new(),
                };
                return Ok(Held { id, node: s(&v, "node"), work: start.map(|st| s(st, "work")).unwrap_or_default(), env });
            }
            _ => {
                let how = s(&v, "how");
                let why = s(&v, "reason");
                if how == "cancelled" {
                    eprintln!("{}", cancelled_notice(&id, &s(&v, "node")));
                    return Err(CANCELLED_EXIT);
                }
                eprintln!("{id} {}: {how}{}", req.kind.as_str(), if why.is_empty() { String::new() } else { format!(": {why}") });
                return Err(if how == "denied" { 2 } else { 1 });
            }
        };
        if line != last {
            eprintln!("{line}");
            last = line;
        }
        std::thread::sleep(BEAT);
        match through_restarts(&id, &mut client, |c| one(c, "beat", &id), &master) {
            Ok(x) => v = x,
            // a restarted master keeps only granted leases: a queued one asks again
            Err(e) if e.starts_with("no lease") && state == "queued" => match one(&client, "lease", &req.to_json()) {
                Ok(x) => {
                    let new_id = s(&x, "id");
                    eprintln!("{id}: the master restarted and lost the queue; asked again as {new_id}");
                    id = new_id;
                    v = x;
                }
                Err(e) => {
                    eprintln!("makepad devhub: lease: {e}");
                    return Err(1);
                }
            },
            Err(e) => {
                eprintln!("makepad devhub: beat {id}: {e} (after {} s)", t0.elapsed().as_secs());
                return Err(1);
            }
        }
    }
}

fn release(c: &Client, id: &str, exit: Option<i32>) -> Option<Value> {
    let line = match exit {
        Some(code) => format!("{id} released exit={code}"),
        None => id.to_string(),
    };
    one(c, "release", &line).ok()
}

/// Runs `cmd` on the node under the lease (relative to its work dir), streams its output.
fn run_there(c: &Client, held: &Held, rel: &str, cmd: &[String]) -> Result<i32, String> {
    let argv = makepad_network::digest::base64_encode(cmd.join("\0").as_bytes());
    let v = node_one(c, &held.node, "lease-run", &format!("lease-run {} {rel} {argv}", held.id))?;
    let run = v.get("n").and_then(super::num).map_or(1, |x| x as u64);
    let mut off = 0u64;
    let mut beat = Instant::now();
    let mut out = std::io::stdout();
    let mut c = c.clone();
    loop {
        let v = through_restarts(&held.id, &mut c, |c| node_one(c, &held.node, "lease-log", &format!("lease-log {} {run} {off}", held.id)), &master)?;
        let data = b64_decode(&s(&v, "data")).ok_or("lease-log: bad data")?;
        let _ = out.write_all(&data);
        let _ = out.flush();
        off = v.get("off").and_then(super::num).map_or(off + data.len() as u64, |x| x as u64);
        if data.is_empty() {
            if let Some(code) = v.get("exit").filter(|e| !e.is_null()).and_then(super::num) {
                return Ok(code as i32);
            }
            std::thread::sleep(Duration::from_millis(300));
        }
        if beat.elapsed() >= BEAT {
            beat = Instant::now();
            let st = through_restarts(&held.id, &mut c, |c| one(c, "beat", &held.id), &master)?;
            if s(&st, "state") == "ended" {
                return Err(format!("{} ended ({})", held.id, s(&st, "how")));
            }
        }
    }
}

/// `lease …`: hold (print the env, beat until released) or run a cmd under it.
fn lease(args: &[&str]) -> i32 {
    let Some((kind, rest)) = args.split_first() else {
        eprintln!("makepad devhub lease <kind> [--node N|--needs ..] [-- cmd...]");
        return 2;
    };
    let o = match parse(kind, None, rest) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("makepad devhub lease: {e}");
            return 2;
        }
    };
    let c = match master() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let held = match acquire(&c, &o.req) {
        Ok(h) => h,
        Err(code) => return code,
    };
    if !o.cmd.is_empty() {
        let r = run_there(&c, &held, ".", &o.cmd);
        return finish(&c, &held, r);
    }
    println!("{} granted {}@{} work={}", held.id, o.req.kind.as_str(), held.node, held.work);
    println!("export DEVHUB_LEASE={}", held.id);
    for (k, v) in &held.env {
        println!("export {k}={}", shell_quote(v));
    }
    let _ = std::io::stdout().flush();
    // the holder: beat until the lease ends, its watch pid dies, or this process is killed
    let mut lost: Option<()> = None;
    loop {
        std::thread::sleep(BEAT);
        if o.watch.is_some_and(|p| !pid_alive(p)) {
            release(&c, &held.id, None);
            eprintln!("{}: watched pid ended; released", held.id);
            return 0;
        }
        match master().and_then(|c| one(&c, "beat", &held.id)) {
            Ok(v) if s(&v, "state") == "ended" => {
                if s(&v, "how") == "cancelled" {
                    eprintln!("{}", cancelled_notice(&held.id, &held.node));
                    return CANCELLED_EXIT;
                }
                let why = s(&v, "reason");
                eprintln!("{}: ended ({}){}", held.id, s(&v, "how"), if why.is_empty() { String::new() } else { format!(": {why}") });
                return 0;
            }
            Ok(_) => {
                if lost.take().is_some() {
                    eprintln!("{}: the master is back; still held", held.id);
                }
            }
            // a restarting master keeps its granted leases (leases.live): keep beating
            Err(e) if e.starts_with("no lease") => return fail(format!("beat {}: {e}", held.id)),
            Err(e) => {
                if lost.is_none() {
                    eprintln!("{}: the master does not answer ({e}); still holding, retrying", held.id);
                    lost = Some(());
                }
            }
        }
    }
}

fn finish(c: &Client, held: &Held, r: Result<i32, String>) -> i32 {
    // the user cancelled it: its processes were stopped, so the run's own result is just the fallout
    if one(c, "beat", &held.id).is_ok_and(|v| s(&v, "state") == "ended" && s(&v, "how") == "cancelled") {
        eprintln!("{}", cancelled_notice(&held.id, &held.node));
        return CANCELLED_EXIT;
    }
    let (exit, code) = match r {
        Ok(code) => (Some(code), code),
        Err(e) => (None, fail(e)),
    };
    let mut c = c.clone();
    let line = match exit {
        Some(e) => format!("{} released exit={e}", held.id),
        None => held.id.clone(),
    };
    if let Ok(v) = through_restarts(&held.id, &mut c, |c| one(c, "release", &line), &master) {
        eprintln!("{}", released_line(&v));
    }
    code
}

/// `L3 screen@100 (lane x) released; 100 cleaned up: 2 processes stopped, 120 MB freed`.
fn released_line(v: &Value) -> String {
    let node = s(v, "node");
    let at = if node.is_empty() { String::new() } else { format!("@{node}") };
    let head = format!("{} {}{at} (lane {}) {}", s(v, "id"), s(v, "kind"), s(v, "lane"), match s(v, "how").as_str() {
        "released" | "" => "released".to_string(),
        "cancelled" => "cancelled by user".to_string(),
        how => format!("ended ({how})"),
    });
    let Some(c) = v.get("cleanup") else { return head };
    match s(c, "cleanup").as_str() {
        "ok" => format!("{head}; {} cleaned up: {} stopped, {} MB freed{}", s(c, "node"), super::master::plural(n(c, "stopped") as usize, "process", "processes"), n(c, "freed_mb"), if n(c, "leak") > 0.0 { format!(", {} LEFT (see the inbox)", super::master::plural(n(c, "leak") as usize, "process", "processes")) } else { String::new() }),
        other => format!("{head}; {other}"),
    }
}

fn shell_quote(v: &str) -> String {
    if v.chars().all(|c| c.is_ascii_alphanumeric() || "/._-:=,+".contains(c)) {
        v.to_string()
    } else {
        format!("'{}'", v.replace('\'', "'\\''"))
    }
}

/// Ships the worktree (tracked + untracked-not-ignored files that exist, + apps/commercial, +
/// DEVHUB_ALSO) to the lease's work dir over the devhub link ([`super::ship`]: a file goes when
/// its size, mtime or executable bit differ there), with its file list; the node then prunes the
/// files the previous ship had and this one has not (deleted on the lane's side).
fn ship(c: &Client, root: &Path, held: &Held) -> Result<(), String> {
    if held.work.is_empty() {
        return Err(format!("{}: no work dir to ship to", held.id));
    }
    let mut files: Vec<String> = git(root, &["ls-files", "-co", "--exclude-standard"]).ok_or("git ls-files failed")?.lines().map(str::to_string).collect();
    if root.join("apps/commercial/.git").exists() {
        if let Some(com) = git(&root.join("apps/commercial"), &["ls-files", "-co", "--exclude-standard"]) {
            files.extend(com.lines().map(|f| format!("apps/commercial/{f}")));
        }
    }
    // only what is there: deleted-but-unstaged files, dangling links and gitlinks are not shipped
    files.retain(|f| {
        let p = root.join(f);
        p.exists() && std::fs::symlink_metadata(&p).is_ok_and(|m| !m.is_dir())
    });
    files.sort();
    files.dedup();
    let mut all = files.clone();
    for p in std::env::var("DEVHUB_ALSO").unwrap_or_default().split_whitespace() {
        if !root.join(p).exists() {
            return Err(format!("DEVHUB_ALSO path missing: {p}"));
        }
        // shipped, never in the prune list: two runs with different DEVHUB_ALSO keep each other's files
        all.push(p.to_string());
    }
    let mut entries = Vec::new();
    for f in &all {
        super::ship::entries(root, f, &mut entries)?;
    }
    let t = Instant::now();
    // the lease is beaten while the tree goes (a first ship of a whole tree takes a while)
    let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (bc, bd, bid) = (c.clone(), done.clone(), held.id.clone());
    let beater = std::thread::spawn(move || {
        while !bd.load(std::sync::atomic::Ordering::Relaxed) {
            let t = Instant::now();
            while t.elapsed() < BEAT && !bd.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(100));
            }
            if !bd.load(std::sync::atomic::Ordering::Relaxed) {
                let _ = one(&bc, "beat", &bid);
            }
        }
    });
    let sent = super::ship::send(c, &held.node, &held.id, root, &entries, &format!("{}\n", files.join("\n")));
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = beater.join();
    let sent = sent?;
    if sent.sent > 0 {
        eprintln!("{} {}: shipped {} of {} files ({} KB) in {:.1} s", held.id, held.node, sent.sent, sent.files, sent.bytes >> 10, t.elapsed().as_secs_f64());
    }
    let v = node_one(c, &held.node, "lease-prune", &format!("lease-prune {}", held.id))?;
    let n = n(&v, "pruned");
    if n > 0.0 {
        eprintln!("{} {}: {n} file(s) deleted on the lane's side removed", held.id, held.node);
    }
    Ok(())
}

/// `run <lane> <kind> … -- cmd`: lease, ship, run, release.
fn run(lane: &str, kind: &str, rest: &[&str]) -> i32 {
    let o = match parse(kind, Some(lane), rest) {
        Ok(o) if !o.cmd.is_empty() => o,
        Ok(_) => {
            eprintln!("makepad devhub run <lane> <kind> [--node N|--needs ..] -- cmd...");
            return 2;
        }
        Err(e) => {
            eprintln!("makepad devhub run: {e}");
            return 2;
        }
    };
    let Some(root) = git(&cwd(), &["rev-parse", "--show-toplevel"]).map(PathBuf::from) else {
        eprintln!("makepad devhub run: run it from inside the lane's worktree");
        return 2;
    };
    let rel = git(&cwd(), &["rev-parse", "--show-prefix"]).filter(|p| !p.is_empty()).unwrap_or_else(|| ".".into());
    let c = match master() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let held = match acquire(&c, &o.req) {
        Ok(h) => h,
        Err(code) => return code,
    };
    eprintln!("{} {}@{}: shipping {}", held.id, o.req.kind.as_str(), held.node, root.display());
    if let Err(e) = ship(&c, &root, &held) {
        release(&c, &held.id, None);
        return fail(e);
    }
    let r = run_there(&c, &held, rel.trim_end_matches('/'), &o.cmd);
    finish(&c, &held, r)
}

/// The node a lane's dir lives on: the node of its newest lease (the scheduler keeps a lane on
/// its node while the target there is warm), or of lease `lease`; from the master's live and
/// recent leases and its ledger (this runs on the master's machine).
fn lane_node(c: &Client, lane: &str, lease: Option<&str>) -> Result<String, String> {
    let mut lines = ask(c, None, "leases", "leases all")?;
    lines.extend(std::fs::read_to_string(crate::proto::home().join("leases.jsonl")).unwrap_or_default().lines().map(str::to_string));
    let mut best: Option<(u64, String)> = None;
    for l in lines {
        let Some(v) = json(&l) else { continue };
        let (id, node) = (s(&v, "id"), s(&v, "node"));
        let hit = match lease {
            Some(want) => id == want,
            None => s(&v, "lane") == lane && s(&v, "kind") != "inspect",
        };
        let num = id.trim_start_matches('L').parse::<u64>().unwrap_or(0);
        if hit && !node.is_empty() && best.as_ref().is_none_or(|(b, _)| num > *b) {
            best = Some((num, node));
        }
    }
    best.map(|(_, n)| n).ok_or_else(|| match lease {
        Some(id) => format!("no lease {id}"),
        None => format!("lane {lane} has had no lease on any node: give --node"),
    })
}

/// `get <lane> <rel> <local-dir> [--node N] [--lease L]`: copies a file or folder from the lane's
/// dir on its node (or a live lease's) into <local-dir>, over the devhub link (no ssh).
fn get(lane: &str, rel: &str, dst: &str, rest: &[&str]) -> i32 {
    let (mut node, mut lease) = (None, None);
    let mut i = 0;
    while i < rest.len() {
        match (rest[i], rest.get(i + 1)) {
            ("--node", Some(n)) => node = Some(n.to_string()),
            ("--lease", Some(l)) => lease = Some(l.to_string()),
            _ => {
                eprintln!("makepad devhub get <lane> <rel> <local-dir> [--node N] [--lease L]");
                return 2;
            }
        }
        i += 2;
    }
    if !crate::trust::valid_name(lane) || rel.is_empty() || rel.contains(' ') {
        eprintln!("makepad devhub get <lane> <rel> <local-dir> [--node N] [--lease L] (rel: a path relative to the lane's dir, no spaces)");
        return 2;
    }
    let c = match master() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let node = match node.map_or_else(|| lane_node(&c, lane, lease.as_deref()), Ok) {
        Ok(n) => n,
        Err(e) => return fail(e),
    };
    let lid = lease.as_deref().unwrap_or("-");
    let list = match ask(&c, Some(&node), "get-ls", &format!("get-ls {lane} {lid} {rel}")) {
        Ok(l) => l,
        Err(e) => return fail(format!("{node}: {e}")),
    };
    let dst = PathBuf::from(dst);
    let (mut count, mut bytes) = (0usize, 0u64);
    for l in list {
        let Some(v) = json(&l) else { continue };
        if let Some(e) = v.get("err").and_then(|e| e.as_str()) {
            return fail(format!("{node}: {e}"));
        }
        let (p, r) = (s(&v, "p"), s(&v, "r"));
        let out = dst.join(&p);
        if Path::new(&p).components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return fail(format!("{node}: bad path {p}"));
        }
        if let Some(dir) = out.parent() {
            if let Err(e) = std::fs::create_dir_all(dir) {
                return fail(format!("{}: {e}", dir.display()));
            }
        }
        let mut data = Vec::new();
        loop {
            let v = match node_one(&c, &node, "get-read", &format!("get-read {lane} {lid} {} {r}", data.len())) {
                Ok(v) => v,
                Err(e) => return fail(e),
            };
            let Some(chunk) = b64_decode(&s(&v, "data")) else { return fail("get-read: bad data") };
            data.extend_from_slice(&chunk);
            if v.get("eof").and_then(|e| e.as_bool()) != Some(false) || chunk.is_empty() {
                break;
            }
        }
        if let Err(e) = std::fs::write(&out, &data) {
            return fail(format!("{}: {e}", out.display()));
        }
        count += 1;
        bytes += data.len() as u64;
    }
    if count == 0 {
        return fail(format!("{node}: nothing under {rel} for lane {lane}"));
    }
    eprintln!("got {} ({:.1} MB) from {node} into {}", super::master::plural(count, "file", "files"), bytes as f64 / 1048576.0, dst.display());
    0
}

fn queue(c: &Client, all: bool) -> i32 {
    let lines = match ask(c, None, "leases", if all { "leases all" } else { "leases" }) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    if lines.is_empty() {
        println!("no leases");
    }
    for l in lines {
        let Some(v) = json(&l) else { continue };
        let node = s(&v, "node");
        let at = if node.is_empty() { String::new() } else { format!("@{node}") };
        let tail = match s(&v, "state").as_str() {
            "queued" => format!("{} in queue ({}) waiting {} s", ordinal(n(&v, "pos") as usize), s(&v, "reason"), n(&v, "wait_s")),
            "ended" => format!("ended ({}){}", s(&v, "how"), if s(&v, "reason").is_empty() { String::new() } else { format!(": {}", s(&v, "reason")) }),
            st => {
                let info = v.get("info").filter(|i| !i.is_null());
                let run = info.map(|i| s(i, "running")).filter(|r| !r.is_empty()).map_or(String::new(), |r| format!(" running `{r}`"));
                let disk = info.and_then(|i| i.get("disk_mb")).and_then(super::num).map_or(String::new(), |m| format!(" disk {:.1} GB", m / 1024.0));
                let agent = s(&v, "agent");
                format!(
                    "{st} {}s ago, beat {}s ago{}{run}{disk}{}",
                    n(&v, "granted_s"),
                    n(&v, "beat_s"),
                    if agent.is_empty() { String::new() } else { format!(" agent {agent}") },
                    v.get("start").map(|x| format!(" work={}", s(x, "work"))).unwrap_or_default()
                )
            }
        };
        let jit = s(&v, "jit");
        println!("{:<6} {:<8} {:<10} {}{}: {tail}", s(&v, "id"), s(&v, "lane"), format!("{}{at}", s(&v, "kind")), if jit.is_empty() { "" } else { "jit " }, jit);
    }
    0
}

fn nodes(c: &Client) -> i32 {
    match ask(c, None, "nodes", "nodes") {
        Ok(lines) => {
            for l in lines {
                let Some(v) = json(&l) else { continue };
                println!("{:<8} {:<8} {}  [{}; {} leases, {} waiting]", s(&v, "node"), s(&v, "role"), s(&v, "caps_line"), s(&v, "state"), n(&v, "leases"), n(&v, "waiting"));
            }
            0
        }
        Err(e) => fail(e),
    }
}

/// `inspect <node>/<pid|app> [--lease L] <cmd...>` on a node: under an inspect lease, through
/// the master's `linspect` (None: this machine's JIT or no master; plain `inspect` handles it).
fn inspect(target: &str, rest: &[&str]) -> Option<i32> {
    let c = control().ok()?;
    if !has(&c, "linspect") {
        return None;
    }
    let (node, jit) = target.split_once('/')?;
    if node == local_name(&c) {
        return None;
    }
    let (lease, cmd) = match rest {
        ["--lease", l, cmd @ ..] => (Some(l.to_string()), cmd),
        cmd => (None, cmd),
    };
    if cmd.is_empty() {
        eprintln!("makepad devhub inspect <node>/<pid|app> [--lease L] <cmd...>");
        return Some(2);
    }
    let (id, own) = match lease {
        Some(l) => (l, false),
        None => {
            let mut req = Req::new(&default_lane(), Kind::Inspect);
            req.node = Some(node.to_string());
            req.jit = Some(jit.to_string());
            match acquire(&c, &req) {
                Ok(h) => (h.id, true),
                Err(code) => return Some(code),
            }
        }
    };
    let r = ask(&c, None, "linspect", &format!("{id} {node} {jit} {}", cmd.join(" ")));
    if own {
        release(&c, &id, None);
    }
    Some(match r {
        Ok(lines) => {
            let mut code = 0;
            for l in lines {
                code |= l.starts_with("{\"err\":") as i32;
                println!("{l}");
            }
            code
        }
        Err(e) => fail(e),
    })
}

/// `node <n> [role R | set k=v..]`: the node's lease.conf, set by the master (persisted on the
/// node, in force at once); prints its capabilities after.
fn node_conf(c: &Client, node: &str, what: &str) -> i32 {
    match node_one(c, node, "lease-conf", &format!("lease-conf {what}")) {
        Ok(v) => {
            let caps = super::Caps::parse(&v.to_json()).unwrap_or_default();
            let slots: Vec<String> = caps.slots.iter().map(|(k, n)| format!("{}={n}", k.as_str())).collect();
            println!(
                "{node}: role {}  slots {}  disk floor {:.0} GB (warn {:.0})  timed {}{}",
                caps.role().as_str(),
                slots.join(" "),
                caps.disk_floor_gb,
                caps.disk_warn_gb,
                if caps.timed { "on" } else { "off" },
                if caps.spec.is_empty() { String::new() } else { format!("  spec {}", caps.spec) }
            );
            0
        }
        Err(e) => fail(e),
    }
}

/// `desk <node> [on|off]`: the node's Desk Link endpoint switched (through the master's router),
/// and what it does now. The viewers list it from the master's next caps round (10 s).
fn desk(c: &Client, node: &str, what: &str) -> i32 {
    match node_one(c, node, "desk", &format!("desk {what}")) {
        Ok(v) => {
            let d = v.get("desk_link").and_then(crate::embed::DeskLinkState::parse).unwrap_or_default();
            let said = s(&v, "said");
            let state = if !d.on { "off".to_string() } else if d.port > 0 { format!("on, port {}", d.port) } else { "on, not listening yet".to_string() };
            println!("{node}: desk link {state}{}{}", if d.note.is_empty() { String::new() } else { format!(" ({})", d.note) }, if said.is_empty() || said == d.note { String::new() } else { format!("; {said}") });
            0
        }
        Err(e) => fail(e),
    }
}

fn disk(c: &Client, node: &str, dry: bool) -> i32 {
    match ask(c, Some(node), "disk", if dry { "disk dry" } else { "disk" }) {
        Ok(lines) => {
            let mut code = 0;
            for l in lines {
                code |= l.starts_with("{\"err\":") as i32;
                println!("{l}");
            }
            code
        }
        Err(e) => fail(e),
    }
}

/// `landed <lane>`: the lane's dirs on every node (`disk clean <lane>`), then its Mac worktree
/// and branch when the branch is merged into `work`.
fn landed(c: &Client, lane: &str) -> i32 {
    if !crate::trust::valid_name(lane) {
        eprintln!("makepad devhub landed <lane>");
        return 2;
    }
    let mut code = 0;
    let repo = cwd();
    let branch = format!("rc-{lane}");
    let merged = git(&repo, &["merge-base", "--is-ancestor", &branch, "work"]).is_some();
    let exists = git(&repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_some();
    if exists && !merged {
        eprintln!("{branch} is not merged into work: nothing deleted");
        return 1;
    }
    for l in ask(c, None, "nodes", "nodes").unwrap_or_default() {
        let Some(v) = json(&l) else { continue };
        let node = s(&v, "node");
        if v.get("up").and_then(|u| u.as_bool()) != Some(true) {
            println!("{node}: down, skipped");
            continue;
        }
        match node_one(c, &node, "disk", &format!("disk clean {lane}")) {
            Ok(v) => println!("{node}: freed {} MB", n(&v, "freed_mb")),
            Err(e) => {
                code = 1;
                println!("{node}: {e}");
            }
        }
    }
    if !exists {
        return code;
    }
    // the worktree that has the branch checked out (git refuses a dirty one)
    let list = git(&repo, &["worktree", "list", "--porcelain"]).unwrap_or_default();
    let mut path = None;
    for block in list.split("\n\n") {
        if block.lines().any(|l| l == format!("branch refs/heads/{branch}")) {
            path = block.lines().find_map(|l| l.strip_prefix("worktree ")).map(str::to_string);
        }
    }
    if let Some(p) = path {
        match makepad_remote::group::command("git").arg("-C").arg(&repo).args(["worktree", "remove", &p]).status() {
            Ok(st) if st.success() => println!("worktree {p}: removed"),
            _ => {
                println!("worktree {p}: kept (git refused: local changes or nested checkouts)");
                return 1;
            }
        }
    }
    if makepad_remote::group::command("git").arg("-C").arg(&repo).args(["branch", "-d", &branch]).status().is_ok_and(|s| s.success()) {
        println!("branch {branch}: deleted");
    } else {
        code = 1;
    }
    code
}

/// `lease <id> cancel`: the user's end of one lease.
fn cancel(id: &str) -> i32 {
    let c = match master() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    match one(&c, "cancel", id) {
        Ok(v) => {
            println!("{}", released_line(&v));
            0
        }
        Err(e) => fail(e),
    }
}

/// The lease words; None: not one of them.
pub fn cli(args: &[&str]) -> Option<i32> {
    let word = *args.first()?;
    if word == "inspect" {
        // `inspect --lease L <target> <cmd...>` reads as `inspect <target> --lease L <cmd...>`
        if let ["inspect", "--lease", l, target, cmd @ ..] = args {
            let mut rest = vec!["--lease", *l];
            rest.extend_from_slice(cmd);
            return inspect(target, &rest);
        }
        return inspect(args.get(1)?, &args[2..]);
    }
    if !matches!(word, "lease" | "run" | "get" | "release" | "cancel" | "queue" | "leases" | "nodes" | "node" | "disk" | "desk" | "landed" | "clean" | "evict" | "unblock" | "override" | "blocked") {
        return None;
    }
    match args {
        ["lease", id, "cancel"] | ["cancel", id] => return Some(cancel(id)),
        ["lease", rest @ ..] => return Some(lease(rest)),
        ["run", lane, kind, rest @ ..] => return Some(run(lane, kind, rest)),
        ["get", lane, rel, dst, rest @ ..] => return Some(get(lane, rel, dst, rest)),
        _ => {}
    }
    let c = match master() {
        Ok(c) => c,
        Err(e) => return Some(fail(e)),
    };
    Some(match args {
        ["release", id] => match one(&c, "release", id) {
            Ok(v) => {
                println!("{}", released_line(&v));
                0
            }
            Err(e) => fail(e),
        },
        ["queue" | "leases", rest @ ..] => queue(&c, rest.contains(&"--all")),
        ["evict", who] => match one(&c, "evict", who) {
            Ok(v) => {
                let n = match v.get("ended") {
                    Some(Value::Obj(o)) => o.len(),
                    _ => 0,
                };
                println!("{who} evicted: {} ended (processes stopped, dirs cleaned); blocked until `makepad devhub unblock {who}`", super::master::plural(n, "lease", "leases"));
                0
            }
            Err(e) => fail(e),
        },
        ["unblock", who] => match one(&c, "unblock", who) {
            Ok(_) => {
                println!("{who} unblocked: it may take leases again");
                0
            }
            Err(e) => fail(e),
        },
        ["blocked"] => match ask(&c, None, "blocked", "blocked") {
            Ok(l) if l.is_empty() => {
                println!("nobody is blocked");
                0
            }
            Ok(l) => {
                l.iter().filter_map(|x| json(x)).for_each(|v| println!("{}", s(&v, "blocked")));
                0
            }
            Err(e) => fail(e),
        },
        ["override", node, rest @ ..] => {
            let to = match rest {
                [] => None,
                ["--to", lane] => Some(*lane),
                _ => {
                    eprintln!("makepad devhub override <node> [--to <lane>]");
                    return Some(2);
                }
            };
            match one(&c, "override", &format!("{node}{}", to.map_or(String::new(), |t| format!(" --to {t}")))) {
                Ok(v) => {
                    let n = match v.get("ended") {
                        Some(Value::Obj(o)) => o.len(),
                        _ => 0,
                    };
                    match to {
                        Some(t) => println!("{node} given to lane {t}: {} ended; its waiting leases go first", super::master::plural(n, "lease", "leases")),
                        None => println!("{node} given to the user: {} ended; dev switch off (no new leases)", super::master::plural(n, "lease", "leases")),
                    }
                    0
                }
                Err(e) => fail(e),
            }
        }
        ["nodes"] => nodes(&c),
        ["node", node, "dev", v @ ("on" | "off")] => match one(&c, "node-dev", &format!("{node} {v}")) {
            Ok(v) => {
                println!("{}: {}", s(&v, "node"), s(&v, "state"));
                0
            }
            Err(e) => fail(e),
        },
        ["node", node] => node_conf(&c, node, ""),
        ["node", node, "role", role] => node_conf(&c, node, &format!("set role={role}")),
        ["node", node, "set", kvs @ ..] if !kvs.is_empty() => node_conf(&c, node, &format!("set {}", kvs.join(" "))),
        ["disk", node, rest @ ..] => disk(&c, node, rest.contains(&"--dry-run")),
        ["desk", node, rest @ ..] => match rest {
            [] | ["state"] => desk(&c, node, "state"),
            [v @ ("on" | "off")] => desk(&c, node, v),
            _ => {
                eprintln!("makepad devhub desk <node> [on|off]");
                2
            }
        },
        ["landed" | "clean", lane] => landed(&c, lane),
        _ => {
            eprintln!("{HELP}");
            2
        }
    })
}

#[cfg(test)]
mod restart_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    /// A gate's holder outlives a master restart: while the master is down every call fails and
    /// the holder waits; once it is back the same lease answers, and the gate's verdict arrives.
    #[test]
    fn a_holder_waits_out_a_master_restart_and_gets_the_verdict() {
        let down = Arc::new(AtomicBool::new(true));
        let d = down.clone();
        // the master comes back after 1.5 s
        let flip = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(1500));
            d.store(false, Ordering::SeqCst);
        });
        let up = {
            let down = down.clone();
            move || if down.load(Ordering::SeqCst) { Err("no devhub runs here".to_string()) } else { Ok(Client::new(makepad_remote::link::Target::Unix("/nonexistent".into()), "t")) }
        };
        let mut c = Client::new(makepad_remote::link::Target::Unix("/nonexistent".into()), "t");
        let mut calls = 0;
        let r = through_restarts("L1", &mut c, |_| {
            calls += 1;
            if down.load(Ordering::SeqCst) { Err("link lost: Connection refused".to_string()) } else { Ok("NODEGATE-OK") }
        }, &up);
        flip.join().unwrap();
        assert_eq!(r, Ok("NODEGATE-OK"));
        assert!(calls > 1, "the call was tried again");
        // a live master's "no lease" stands at once
        let r: Result<(), String> = through_restarts("L1", &mut c, |_| Err("no lease L1".into()), &up);
        assert_eq!(r, Err("no lease L1".to_string()));
        // a live master's other error stands at once when there was no outage
        let r: Result<(), String> = through_restarts("L1", &mut c, |_| Err("103: lease-log: bad".into()), &up);
        assert!(r.is_err());
    }
}
