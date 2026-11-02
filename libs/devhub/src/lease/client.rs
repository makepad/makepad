//! The lease words of `makepad devhub`: the holder side of a lease (it beats the master every
//! 2 s while it holds one; a holder that dies lets its lease expire), `run` (ship the worktree,
//! run under a lease, stream the output, exit with its code), and the views and switches.
//!
//! Exit codes: 0 ok, 1 the command failed, 2 usage or denied.

use super::master::b64_decode;
use super::sched::{ordinal, parse_needs, Req};
use super::{CostClass, Kind, Priority, Src};
use crate::cli::{ask, control, has, local_name, retry_master};
use makepad_remote::link::Client;
use makepad_strict_json::Value;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// An exclusive request's expected wait, as the asker reads it (minutes; 5-minute steps past 10).
fn eta(ms: u64) -> String {
    if ms == 0 { return String::new(); }
    let min = ms.div_ceil(60_000);
    let min = if min > 10 { min.div_ceil(5) * 5 } else { min };
    format!("; expected start in ~{min} min")
}

pub const HELP: &str = "  lease <kind> [--node N|--only N|--needs k=v,..] [--lane L] [--prio P] [--rev <branch>[@sha][,<commercial>[@sha]]]
        [--repo-dir D] [--watch-pid P] [--cost small|medium|large] [--est-min N] [--cores N] [--ram-gb N] [--whole]
        [--class name] [--priority interactive|background] [--jit key] [--native <reason>] [-- cmd...]
                                          hold a lease (kinds: build test screen long timed gpu inspect gate ci;
                                          ci: its own tree and target, beside ordinary work, never timed or user boxes;
                                          --needs gui=1: a test that opens windows, only nodes that take screen leases);
                                          with a cmd: run it there and exit with its code. --node N: N when it can
                                          take it, else (after 2 min, or at once when N is held/busy/off) a node of
                                          the same arch and os (the grant line names it); --only N: N or wait
                                          (timed, screen, ci and inspect leases always wait for the named node)
  run <lane> <kind> [--node N|--needs ..] [--with-uncommitted] [--tree]
        [--cost small|medium|large] [--est-min N] [--cores N] [--ram-gb N] [--whole]
        [--class name] [--priority interactive|background] [--jit key] [--native <reason>] -- cmd...
                                          JIT commands of kinds test, screen, gpu, long and build use overlays by default;
                                          --tree uses the work tree. CI, gates, timed and non-JIT commands always use trees.
                                          Main checkout edits need --with-uncommitted.
                                          JIT first: a lane command (or job, or test --native) that cargo-builds a package
                                          is refused unless it is the JIT's own crate, has an open JIT miss, targets a
                                          platform no JIT runs on, or says --native <reason> (native-vs-JIT baseline, timed
                                          perf series); the refusal names the JIT command
  overlay status <lane> [--node N]        pinned commits, overlay hashes, whiteouts and diff vs work
  overlay why <lane> <path> [--node N]     serving layer and content hash
  overlay selftest [--node N] [--case NAME]
                                          compare real checkouts with overlay runs on a test lease
  overlay shadow on|off|status           background tree/overlay comparisons of JIT tests; last 24 h of evidence
  test <lane> <pkg> [--lib|--bin B|--test T|--tests] [--features F] [--needs k=v,..] [--node N] [--cost small|medium|large] [--est-min N] [--class name] [--priority interactive|background] [--jit key] [--native [<reason>]|--live] [--tree] [-- args...]
                                          --live: THE lane check; one warm process per lane, package, target and args,
                                          small cost on a hot box; overlay by default, --tree opts out; only affected tests rerun.
                                          Without --live: one JIT run, the lane's final check; devhub's measured routing may
                                          pick cargo (saying both walls). --native <reason>: cargo test --release (a tree),
                                          only for native-vs-JIT baselines and timed perf series (or an open JIT miss).
                                          compile/resolve misses fall back to cargo on a pool box; open misses skip the JIT;
                                          an open miss is retried once the JIT's inputs changed.
                                          Other JIT errors: a separate event, exit 3; carry on
  get <lane> <rel> <local-dir> [--node N] [--lease L]
                                          copy results back from the lane's dir on its node (`src/..`: the tree
                                          `run` ships, which holds only tracked and ignored files once
                                          the lease ended, `target/..`: its target; with --lease: that live lease's
                                          work, `target/..`, `tmp/..`)
  release <id>                            end a lease (says how its node cleaned up)
  lease <id> cancel | cancel <id>         the user's cancel of ONE lease: its processes stop, its slot frees, the
                                          lane is kept off that node for 30 min (unless it names it with --node)
  override <node> [--to <lane>]           take the node now: its leases end; to the user (dev off) or a lane
  evict <lane|agent>                      end every lease of it, clean up, block new ones; unblock <lane|agent>
  blocked                                 the lanes and agents the user blocked
  jit stats [--day]                      JIT builds per id/arch and box/day (UTC), store hits/misses
  jit misses [--all]                     open compile/resolve misses (--all includes closed entries)
  jit misses close <id>                  close M<n>; the next test tries the JIT again
  prof [--hour|--day] [--agent] [--lane L] [--kind K]
                                          workflow minutes: agent wait first (how long agents sat blocked on
                                          devhub: run/test/lease from a lane, job wait, live save -> verdict;
                                          not CI, post-land buckets, prebuilds, timed series), then phases,
                                          blockers, run classes, box idle, and last `jit first`: per lane, native
                                          builds (by gate verdict, --native reasons) vs saves patched live, OVER
                                          on 2+ refused or --native builds above the live saves; --agent: per lane
  stats queue [--hour]                   the last hour's agent wait, then live counts and average/max queue
                                          wait per class
  queue | leases [--all]                  live and waiting leases, positions and reasons
  why-queued <lease>                     each candidate box's start blocker and reservation expiry
  nodes                                   capabilities, dev switch and leases per node
  node <n> dev on|off                     the node's dev switch (the user's decision)
  node <n> [role pool|hot|suite|bench|user-wm|device] [set slots.screen=0 disk_floor_gb=40 ...]
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
/// How long a holder waits for its node hub to reattach after a restart.
const REATTACH_FOR: Duration = Duration::from_secs(900);
/// After the master is back, how long its errors may still be its node links coming up.
const SETTLE_FOR: Duration = Duration::from_secs(60);

/// A node-side error that is its hub restarting for the self-update (the hub refused a run in its
/// last moment, or is not up again yet): the lease stays, the call is tried again.
fn node_restarting(e: &str) -> bool {
    e.contains("restarting: ") || super::master::unreachable(e)
}

/// Calls `f` with the master's control client, through master restarts: while the master is not
/// reachable (`up` fails), retry every 2 s without a time cap. Once it is back, a fresh client
/// reattaches by the lease
/// id. A live master's answer stands, except while its node links come back (`SETTLE_FOR`) or the
/// lease's node hub restarts for its self-update ([`node_restarting`], up to `REATTACH_FOR`: the
/// new hub takes the lease back from its ledger); "no lease" always stands. Said once each way on
/// stderr.
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
                if down.is_some() && back.is_none() {
                    down = Some(Instant::now());
                    back = Some(Instant::now());
                    continue;
                }
                match (down, back) {
                    (None, _) if node_restarting(&e) => {
                        eprintln!("{id}: its node does not answer ({e}); its hub restarts? re-attaching when it is back");
                        down = Some(Instant::now());
                        back = Some(Instant::now());
                    }
                    (Some(t), _) if node_restarting(&e) => {
                        if t.elapsed() > REATTACH_FOR {
                            return Err(format!("{e} (the node stayed unreachable for {} s)", REATTACH_FOR.as_secs()));
                        }
                        back = Some(Instant::now());
                    }
                    (None, _) => return Err(e),
                    (Some(_), None) => back = Some(Instant::now()),
                    (Some(_), Some(t)) if t.elapsed() > SETTLE_FOR => return Err(e),
                    _ => {}
                }
            }
            Err(_) => {
                back = None;
                wait = BEAT;
                if down.is_none() {
                    eprintln!("master not reachable: waiting");
                    down = Some(Instant::now());
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
    tree: bool,
    /// `--native "<reason>"`: why this command builds natively (lease/native.rs)
    native: String,
}

/// The flags shared by `lease` and `run`.
fn parse(kind: &str, lane: Option<&str>, args: &[&str]) -> Result<Opts, String> {
    let kind = Kind::parse(kind).ok_or_else(|| format!("unknown kind `{kind}`"))?;
    let mut req = Req::new(lane.unwrap_or(""), kind);
    let mut watch = None;
    let mut tree = false;
    let mut native = String::new();
    let mut i = 0;
    let mut cmd = Vec::new();
    while i < args.len() {
        let val = |i: usize| args.get(i + 1).copied().ok_or_else(|| format!("{} needs a value", args[i]));
        match args[i] {
            "--" => {
                cmd = args[i + 1..].iter().map(|a| a.to_string()).collect();
                break;
            }
            "--tree" => { tree = true; i += 1; continue; }
            "--whole" => { req.whole = true; i += 1; continue; }
            "--cost" => { req.cost = CostClass::parse(val(i)?).ok_or("--cost is small, medium or large")?; req.cost_asked = true; }
            "--est-min" => req.est_min = Some(val(i)?.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0).ok_or("--est-min is a nonnegative number")?),
            "--cores" => req.cores = Some(val(i)?.parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0).ok_or("--cores is a positive number")?),
            "--ram-gb" => req.ram_gb = Some(val(i)?.parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0).ok_or("--ram-gb is a positive number")?),
            "--class" => req.class = val(i)?.to_string(),
            "--node" => req.node = Some(val(i)?.to_string()),
            "--only" => {
                req.node = Some(val(i)?.to_string());
                req.only = true;
            }
            "--needs" => req.needs = parse_needs(val(i)?),
            "--lane" => req.lane = val(i)?.to_string(),
            "--priority" => req.priority = Priority::parse(val(i)?).ok_or("--priority is interactive or background")?,
            "--prio" => req.prio = val(i)?.parse().map_err(|_| "--prio takes a number")?,
            "--rev" => req.rev = Some(val(i)?.to_string()),
            "--repo-dir" => req.repo_dir = Some(val(i)?.to_string()),
            "--watch-pid" => watch = Some(val(i)?.parse().map_err(|_| "--watch-pid takes a pid")?),
            "--jit" => req.loader = Some(val(i)?.to_string()),
            "--native" => native = val(i)?.to_string(),
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
    if req.class.is_empty() && !cmd.is_empty() { req.class = super::run_class(kind, &cmd); }
    Ok(Opts { req, watch, cmd, tree, native })
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
    req.since_ms.get_or_insert_with(crate::proto::wall_ms);
    if req.agent.is_none() {
        req.agent = std::env::var("DEVHUB_AGENT").ok().filter(|a| makepad_devhub_conf::valid_name(a));
    }
    let req = &req;
    let wire = req.to_json();
    let mut v = retry_master(|| one(c, "lease", &wire)).map_err(|e| {
        eprintln!("makepad devhub lease: {e}");
        2
    })?;
    let mut id = s(&v, "id");
    let mut client = c.clone();
    let mut last = String::new();
    let mut class_notified = false;
    let t0 = Instant::now();
    loop {
        let state = s(&v, "state");
        let note = s(&v, "class_note");
        let note = if class_notified || note.is_empty() { String::new() } else { format!("; {note}") };
        let line = match state.as_str() {
            "queued" => format!("{id} {}: {} in queue ({}){}", req.kind.as_str(), ordinal(n(&v, "pos") as usize), s(&v, "reason"), eta(n(&v, "eta_ms") as u64)),
            "starting" => format!("{id} {}@{}: granted, setting up", req.kind.as_str(), s(&v, "node")),
            "granted" => {
                if !note.is_empty() { eprintln!("{id} {}@{}: granted{note}", req.kind.as_str(), s(&v, "node")); }
                let start = v.get("start");
                let env = match start.and_then(|st| st.get("env")) {
                    Some(Value::Obj(pairs)) => pairs.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or("").to_string())).collect(),
                    _ => Vec::new(),
                };
                if !s(&v, "reason").is_empty() { eprintln!("{id}: {}", s(&v, "reason")); }
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
        if line != last || !note.is_empty() {
            eprintln!("{line}{note}");
            last = line;
            class_notified |= !note.is_empty();
        }
        std::thread::sleep(BEAT);
        match through_restarts(&id, &mut client, |c| one(c, "beat", &id), &master) {
            Ok(x) => v = x,
            // a restarted master keeps only granted leases: a queued one asks again, with when it first asked
            Err(e) if e.starts_with("no lease") && state == "queued" => match retry_master(|| one(&client, "lease", &wire)) {
                Ok(x) => {
                    let new_id = s(&x, "id");
                    eprintln!("{id}: the master restarted; asked again as {new_id}, in the same place");
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
    run_there_into(c, held, rel, cmd, &mut std::io::stdout())
}

fn run_there_into(c: &Client, held: &Held, rel: &str, cmd: &[String], out: &mut dyn Write) -> Result<i32, String> {
    let argv = makepad_network::digest::base64_encode(cmd.join("\0").as_bytes());
    let mut c = c.clone();
    let v = start_there(&mut c, held, "lease-run", &format!("lease-run {} {rel} {argv}", held.id))?;
    let run = v.get("n").and_then(super::num).map_or(1, |x| x as u64);
    follow_there(&c, held, run, out)
}

/// Keep the lease alive while admission, resolution or a hub restart delays its run.
fn start_there(c: &mut Client, held: &Held, service: &str, line: &str) -> Result<Value, String> {
    let (mut last, mut beat) = (String::new(), Instant::now());
    loop {
        let v = through_restarts(&held.id, c, |c| {
            if beat.elapsed() >= BEAT {
                let st = one(c, "beat", &held.id)?;
                if s(&st, "state") == "ended" { return Err(format!("{} ended ({})", held.id, s(&st, "how"))); }
                beat = Instant::now();
            }
            node_one(c, &held.node, service, line)
        }, &master)?;
        let Some(pending) = v.get("pending").and_then(Value::as_str) else { return Ok(v) };
        if pending != last {
            eprintln!("{} {}: {pending}", held.id, held.node);
            last = pending.to_string();
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

fn follow_there(c: &Client, held: &Held, run: u64, out: &mut dyn Write) -> Result<i32, String> {
    let mut c = c.clone();
    let mut off = 0u64;
    let mut beat = Instant::now();
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

/// The JIT-first gate (lease/native.rs) for a command the lane runs here: prints its hint;
/// Err(2) when the master refuses the native build.
fn native_gate(c: &Client, o: &Opts) -> Result<(), i32> {
    if o.cmd.is_empty() { return Ok(()); }
    let root = o.req.repo_dir.clone().or_else(|| git(&cwd(), &["rev-parse", "--show-toplevel"])).unwrap_or_default();
    let rel = git(&cwd(), &["rev-parse", "--show-prefix"]).filter(|p| !p.is_empty()).unwrap_or_else(|| ".".into());
    let j = makepad_remote::json_str;
    let needs = o.req.needs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",");
    let body = format!("{{\"lane\":{},\"agent\":{},\"kind\":{},\"cmd\":[{}],\"repo_dir\":{},\"cwd\":{},\"rev\":{},\"reason\":{},\"class\":{},\"needs\":{},\"node\":{}}}",
        j(&o.req.lane), j(&std::env::var("DEVHUB_AGENT").unwrap_or_default()), j(o.req.kind.as_str()), o.cmd.iter().map(|a| j(a)).collect::<Vec<_>>().join(","),
        j(&root), j(rel.trim_end_matches('/')), j(o.req.rev.as_deref().unwrap_or("")), j(&o.native), j(&o.req.class), j(&needs), j(o.req.node.as_deref().unwrap_or("")));
    match retry_master(|| one(c, "native", &format!("native check {body}"))) {
        Ok(v) => {
            let note = s(&v, "note");
            if !note.is_empty() { eprintln!("{note}"); }
            Ok(())
        }
        Err(e) => {
            eprintln!("makepad devhub: {e}");
            Err(2)
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
    if let Err(code) = native_gate(&c, &o) { return code; }
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
        "cancelled" => { let why = s(v, "reason"); if why.is_empty() { "cancelled".to_string() } else { why } }
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

/// A branch name the node's store takes as a ref label.
fn valid_label(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && !s.starts_with(['-', '/', '.']) && !s.ends_with(['/', '.']) && !s.contains("..") && !s.contains("//") && !s.ends_with(".lock") && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/'))
}

/// The worktree's revision for `run`: its HEAD (+ the nested commercial's), as a pinned `--rev`
/// (`<branch>@<sha>[,<branch>@<sha>]`; a detached HEAD is labelled `run-<lane>`).
fn head_rev(root: &Path, lane: &str) -> Result<String, String> {
    let side = |dir: &Path| -> Result<String, String> {
        let sha = git(dir, &["rev-parse", "HEAD"]).ok_or_else(|| format!("{}: no HEAD", dir.display()))?;
        let branch = git(dir, &["symbolic-ref", "-q", "--short", "HEAD"]).filter(|b| valid_label(b)).unwrap_or_else(|| format!("run-{lane}"));
        Ok(format!("{branch}@{sha}"))
    };
    let mut spec = side(root)?;
    if root.join("apps/commercial/.git").exists() {
        spec = format!("{spec},{}", side(&root.join("apps/commercial"))?);
    }
    super::pin_revs(root, &spec)
}

/// Ships the complete uncommitted set and base blobs into the lease's source layer.
fn ship(c: &Client, root: &Path, held: &Held, pin: &super::Pin, uncommitted: bool) -> Result<(), String> {
    if held.work.is_empty() {
        return Err(format!("{}: no work dir to ship to", held.id));
    }
    let (files, deleted) = super::ship::uncommitted(root, pin)?;
    let (files, deleted) = if uncommitted || !super::ship::is_main_checkout(root) {
        (files, deleted)
    } else {
        if files.len() + deleted.len() > 0 {
            eprintln!("{} {}: main checkout: its {} uncommitted file(s) stay out (--with-uncommitted ships them)", held.id, held.node, files.len() + deleted.len());
        }
        (Vec::new(), Vec::new())
    };
    let mut entries = Vec::new();
    for f in &files {
        super::ship::entries(root, f, &mut entries)?;
    }
    entries.retain(|e| !super::ship::never_ships(&e.path));
    // named paths ship as they are (untracked dirs and `local/` included); a file the
    // uncommitted set has already ships once
    for p in std::env::var("DEVHUB_ALSO").unwrap_or_default().split_whitespace() {
        if !super::ship::inside(p) || super::ship::never_ships_named(p) {
            return Err(format!("DEVHUB_ALSO path never ships: {p}"));
        }
        if !root.join(p).exists() {
            return Err(format!("DEVHUB_ALSO path missing: {p}"));
        }
        let mut named = Vec::new();
        super::ship::entries(root, p, &mut named)?;
        entries.extend(named.into_iter().filter(|e| !super::ship::never_ships_named(&e.path)));
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    entries.dedup_by(|a, b| a.path == b.path);
    let t = Instant::now();
    // the lease is beaten while the files go
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
    let sent = if entries.is_empty() { Ok(super::ship::Shipped::default()) } else { super::ship::send(c, &held.node, &held.id, root, &entries) };
    done.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = beater.join();
    let sent = sent?;
    let paths: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
    let list = |v: &[String]| v.iter().map(|p| makepad_remote::json_str(p)).collect::<Vec<_>>().join(",");
    let bases = super::ship::bases_json(root, pin, &paths.iter().chain(&deleted).cloned().collect::<Vec<_>>())?;
    let body = format!("{{\"shipped\":[{}],\"deleted\":[{}],\"bases\":{bases}}}", list(&paths), list(&deleted));
    node_one(c, &held.node, "ship-done", &format!("ship-done {} {}", held.id, makepad_network::digest::base64_encode(body.as_bytes())))?;
    eprintln!("{} {}: sources at the worktree's HEAD; shipped {} changed file(s) ({} KB, {} sent) and {} deletion(s) on top in {:.1} s", held.id, held.node, paths.len(), sent.bytes >> 10, sent.sent, deleted.len(), t.elapsed().as_secs_f64());
    Ok(())
}

/// `run <lane> <kind> … -- cmd`: pin HEAD, select the source layer, ship, run and release.
fn run(lane: &str, kind: &str, rest: &[&str]) -> i32 {
    let flags = rest.iter().position(|a| *a == "--").unwrap_or(rest.len());
    let uncommitted = rest[..flags].contains(&"--with-uncommitted");
    let rest: Vec<&str> = rest.iter().enumerate().filter(|(i, a)| *i >= flags || **a != "--with-uncommitted").map(|(_, a)| *a).collect();
    let o = match parse(kind, Some(lane), &rest) {
        Ok(o) if !o.cmd.is_empty() => o,
        Ok(_) => {
            eprintln!("makepad devhub run <lane> <kind> [--node N|--needs ..] [--tree] -- cmd...");
            return 2;
        }
        Err(e) => {
            eprintln!("makepad devhub run: {e}");
            return 2;
        }
    };
    if let Err(code) = retry_master(master).map_err(fail).and_then(|c| native_gate(&c, &o)) { return code; }
    run_worktree(o, uncommitted, None)
}

/// Runs at HEAD with the same overlay as `run`.
fn run_worktree(mut o: Opts, uncommitted: bool, test: Option<&str>) -> i32 {
    run_worktree_into(&mut o, uncommitted, test, &mut std::io::stdout()).0
}

/// The second result says whether the command ended, as opposed to a setup/link failure.
fn run_worktree_into(o: &mut Opts, uncommitted: bool, test: Option<&str>, out: &mut dyn Write) -> (i32, bool) {
    let t = Instant::now();
    let word = if test.is_some() { "test" } else { "run" };
    let Some(root) = git(&cwd(), &["rev-parse", "--show-toplevel"]).map(PathBuf::from) else {
        eprintln!("makepad devhub {word}: run it from inside the lane's worktree");
        return (2, false);
    };
    let rel = git(&cwd(), &["rev-parse", "--show-prefix"]).filter(|p| !p.is_empty()).unwrap_or_else(|| ".".into());
    match o.req.rev.clone().map_or_else(|| head_rev(&root, &o.req.lane), Ok) {
        Ok(rev) => {
            o.req.rev = Some(rev);
            o.req.repo_dir = Some(root.to_string_lossy().into_owned());
        }
        Err(e) => {
            eprintln!("makepad devhub {word}: {e}");
            return (2, false);
        }
    }
    o.req.src = Src::for_run(o.req.kind, &o.cmd, o.tree);
    let c = match retry_master(master) {
        Ok(c) => c,
        Err(e) => return (fail(e), false),
    };
    let queued = Instant::now();
    let held = match acquire(&c, &o.req) {
        Ok(h) => h,
        Err(code) => return (code, false),
    };
    let queue = queued.elapsed();
    let shipping = Instant::now();
    let pin = o.req.rev.as_deref().and_then(super::Pin::of_rev).unwrap();
    // a node hub restarting mid-ship keeps the lease (its new hub takes it back from the
    // ledger): the ship is made again once it answers
    let shipped = through_restarts(&held.id, &mut c.clone(), |c| ship(c, &root, &held, &pin, uncommitted), &master);
    let ship = shipping.elapsed();
    let running = Instant::now();
    let r = shipped.and_then(|_| run_there_into(&c, &held, rel.trim_end_matches('/'), &o.cmd, out));
    let run = running.elapsed();
    let completed = r.is_ok();
    let code = finish(&c, &held, r);
    if let Some(pkg) = test {
        let verdict = match code { 0 => "passed", CANCELLED_EXIT => "cancelled", _ => "failed" };
        let _ = writeln!(out, "devhub test: {pkg} {verdict} in {:.3}s (queue {:.3}s, ship {:.3}s, run {:.3}s) on {}", t.elapsed().as_secs_f64(), queue.as_secs_f64(), ship.as_secs_f64(), run.as_secs_f64(), held.node);
    }
    (code, completed)
}

fn test_miss(o: &Opts, pkg: &str, args: &[String], verb: &str, errors: &[String]) -> Result<Value, String> {
    let repo = o.req.repo_dir.clone().or_else(|| git(&cwd(), &["rev-parse", "--show-toplevel"])).unwrap_or_default();
    let req = format!("{{\"pkg\":{},\"target\":{},\"lane\":{},\"rev\":{},\"repo\":{},\"error\":[{}]}}", makepad_remote::json_str(pkg), makepad_remote::json_str(&crate::jit_misses::target(args)), makepad_remote::json_str(&o.req.lane), makepad_remote::json_str(o.req.rev.as_deref().unwrap_or("")), makepad_remote::json_str(&repo), errors.iter().take(5).map(|e| makepad_remote::json_str(e)).collect::<Vec<_>>().join(","));
    retry_master(|| master().and_then(|c| one(&c, "jit", &format!("jit misses {verb} {req}"))))
}

/// Cargo's build-finished event distinguishes a compile error from a test binary's failure,
/// including harness=false targets and filters selecting zero tests. Keep human diagnostics.
fn cargo_output(log: &str) -> (Option<bool>, String) {
    let (mut built, mut out) = (None, String::new());
    for line in log.split_inclusive('\n') {
        // Once compilation ended, these bytes belong to the test program, even if it prints
        // JSON that happens to look like a cargo message.
        if built.is_some() { out.push_str(line); continue; }
        if let Ok(v) = makepad_strict_json::parse_depth(line.trim().as_bytes(), 16) {
            match s(&v, "reason").as_str() {
                "build-finished" => { built = v.get("success").and_then(Value::as_bool); continue; }
                "compiler-artifact" | "build-script-executed" => continue,
                "compiler-message" => {
                    if let Some(message) = v.get("message").and_then(|m| m.get("rendered")).and_then(Value::as_str) {
                        out.push_str(message);
                    }
                    continue;
                }
                _ => {}
            }
        }
        out.push_str(line);
    }
    (built, out)
}

fn cargo_test(mut o: Opts, pkg: &str, args: &[String], errors: &[String], known: Option<Value>) -> i32 {
    if let Some(miss) = &known { println!("{}", crate::jit_misses::notice(miss)); }
    else { eprintln!("devhub test: JIT compile/resolve failure on {pkg} {}; checking with cargo on a pool box", crate::jit_misses::target(args)); }
    let _ = std::io::stdout().flush();
    o.cmd = ["cargo", "test", "--release", "--message-format=json-render-diagnostics", "-p", pkg].iter().map(|s| s.to_string()).chain(args.iter().cloned()).collect();
    o.tree = true;
    o.req.needs.retain(|(k, _)| k != "role");
    o.req.needs.push(("role".into(), "pool".into()));
    o.req.node = None;
    o.req.only = false;
    o.req.loader = None;
    o.req.class = format!("native:{}", o.req.class);
    o.req.cost = CostClass::Medium;
    o.req.cost_asked = true;
    o.req.est_min = None;
    let mut output = Vec::new();
    let (code, completed) = run_worktree_into(&mut o, false, None, &mut output);
    let (built, output) = cargo_output(&String::from_utf8_lossy(&output));
    // Cancellation and transport/setup failures never become evidence of a JIT miss.
    if code == CANCELLED_EXIT { print!("{output}"); return code; }
    if !completed {
        print!("{output}");
        eprintln!("devhub test: cargo fallback did not finish; no JIT-miss verdict recorded");
        return code;
    }
    if built == Some(false) {
        eprintln!("devhub test: {pkg} also fails to compile with cargo; this is the lane's error. JIT diagnostics:");
        for error in errors { eprintln!("{error}"); }
        print!("{output}");
        return 1;
    }
    if built != Some(true) {
        print!("{output}");
        eprintln!("devhub test: cargo did not report a completed build; no JIT-miss verdict recorded");
        return if code == 0 { 2 } else { code };
    }
    if known.is_none() {
        match test_miss(&o, pkg, args, "record", errors) {
            Ok(miss) => println!("{}", crate::jit_misses::notice(&miss)),
            Err(e) => eprintln!("devhub test: JIT miss on {pkg} {} (recording failed: {e}): {}; this verdict is cargo's", crate::jit_misses::target(args), errors.first().map_or("", String::as_str)),
        }
    }
    print!("{output}");
    code
}

/// One-shot tests change their default route only with at least this measured run-phase gain.
const ROUTE_MIN_GAIN: f64 = 0.25;

/// One-shot tests default to the JIT, or cargo for JIT-own crates; true selects the JIT.
/// Compares run-phase medians (the command's own time, its build included; not the dev JIT build).
/// Switch only when both medians exist and the other side is at least ROUTE_MIN_GAIN faster.
pub fn test_route(own: bool, jit: Option<u64>, cargo: Option<u64>) -> (bool, String) {
    let default = if own { "cargo" } else { "the JIT" };
    let why = if own { "compiled only into the JIT binary; " } else { "" };
    match (jit, cargo) {
        (None, _) => (!own, format!("{default} ({why}no JIT measurement yet)")),
        (_, None) => (!own, format!("{default} ({why}no cargo measurement yet)")),
        (Some(jit), Some(cargo)) => {
            let use_jit = if own { jit as f64 <= cargo as f64 * (1.0 - ROUTE_MIN_GAIN) }
                else { cargo as f64 > jit as f64 * (1.0 - ROUTE_MIN_GAIN) };
            let jit = super::sched::dur(jit).replace(' ', "");
            let cargo = super::sched::dur(cargo).replace(' ', "");
            let reason = match (own, use_jit) {
                (true, true) => format!("the JIT (JIT {jit}, cargo {cargo}: at least {:.0}% faster than cargo)", ROUTE_MIN_GAIN * 100.0),
                (true, false) => format!("cargo ({why}cargo {cargo}, JIT {jit}: the JIT must be {:.0}% faster)", ROUTE_MIN_GAIN * 100.0),
                (false, true) => format!("the JIT (JIT {jit}, cargo {cargo})"),
                (false, false) => format!("cargo (cargo {cargo}, JIT {jit}: cargo is at least {:.0}% faster)", ROUTE_MIN_GAIN * 100.0),
            };
            (use_jit, reason)
        }
    }
}

pub(crate) const ALL_TESTS: &str = "tests:*";

fn test(lane: &str, pkg: &str, rest: &[&str]) -> i32 {
    let parsed = (|| -> Result<_, String> {
        if pkg.is_empty() || pkg.starts_with('-') || !pkg.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
            return Err(format!("`{pkg}` is not a package name"));
        }
        let (mut flags, mut select, mut args) = (Vec::new(), Vec::new(), Vec::new());
        let (mut native, mut live, mut target) = (false, false, None::<String>);
        let mut i = 0;
        while i < rest.len() {
            let val = || rest.get(i + 1).copied().filter(|v| !v.is_empty() && !v.starts_with('-')).ok_or_else(|| format!("{} needs a value", rest[i]));
            match rest[i] {
                "--" => { args.extend_from_slice(&rest[i..]); break; }
                // `--native "<reason>"`: the reason goes to the JIT-first gate (lease/native.rs)
                "--native" => {
                    native = true;
                    if let Ok(reason) = val() { flags.extend(["--native", reason]); i += 1; }
                }
                "--live" => live = true,
                "--tree" => flags.push("--tree"),
                "--needs" | "--node" | "--priority" | "--jit" | "--cost" | "--est-min" | "--class" => { flags.extend([rest[i], val()?]); i += 1; }
                "--features" => { select.extend([rest[i], val()?]); i += 1; }
                "--lib" | "--bin" | "--tests" | "--test" => {
                    if target.is_some() { return Err("select only one of --lib, --bin B, --test T or --tests".into()); }
                    select.push(rest[i]);
                    target = Some(match rest[i] {
                        "--bin" => { let t = val()?; select.push(t); i += 1; format!("bin:{t}") }
                        "--test" => { let t = val()?; select.push(t); i += 1; t.to_string() }
                        "--lib" => "lib".to_string(),
                        _ => ALL_TESTS.to_string(),
                    });
                }
                f => return Err(format!("unknown flag `{f}`")),
            }
            i += 1;
        }
        let mut o = parse("test", Some(lane), &flags)?;
        let root = o.req.repo_dir.as_deref().map(PathBuf::from).unwrap_or_else(cwd);
        let rev = head_rev(&root, &o.req.lane)?;
        let pin = super::Pin::of_rev(&rev).ok_or("test: missing commit pair")?;
        o.req.rev = Some(rev);
        // Leave the live default to makepad test: the lib, else the package's only harness target.
        if live && (native || target.as_deref() == Some(ALL_TESTS) || select.contains(&"--features")) {
            return Err("--live selects one target (--lib, --bin B or --test T; default: the lib, else the package's only harness target); --native, --tests and --features cannot be combined with it".into());
        }
        if live && (o.req.node.is_some() || o.req.cost != CostClass::Small || o.req.est_min.is_some()) {
            return Err("--live uses small cost and hot-box routing; omit --node, --cost and --est-min".into());
        }
        if o.req.class.is_empty() {
            o.req.class = target.map_or_else(|| format!("test:{pkg}"), |t| format!("test:{pkg}:{t}"));
            if live { o.req.class = format!("live:{}", o.req.class); }
        }
        // Warm iteration always uses the JIT; measured routing is only for one-shot runs.
        if !native && !live {
            let own = crate::jit_misses::jit_own(&root, &crate::state::Pair::new(&pin.makepad, pin.commercial.as_deref().unwrap_or("")), pkg)?;
            let cargo_class = format!("native:{}", o.req.class);
            let v = retry_master(|| master().and_then(|c| one(&c, "stats", &format!("stats class {} {} {cargo_class}", o.req.kind.as_str(), o.req.class))))?;
            let runs = v.get("runs");
            let (use_jit, reason) = test_route(own, runs.and_then(|r| r.get(&o.req.class)).and_then(Value::as_u64), runs.and_then(|r| r.get(&cargo_class)).and_then(Value::as_u64));
            eprintln!("devhub test: {}: {reason}", o.req.class);
            if !use_jit { native = true; }
        }
        if live && !flags.contains(&"--priority") { o.req.priority = Priority::Interactive; }
        let mut test_args = select.clone();
        test_args.extend(args.iter().copied());
        let mut cmd = if native { vec!["cargo", "test", "--release", "-p", pkg] } else { vec!["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--", "test", pkg] };
        cmd.extend(select);
        cmd.extend(args);
        o.cmd = cmd.into_iter().map(str::to_string).collect();
        if native { o.req.class = format!("native:{}", o.req.class); }
        o.tree |= native;
        Ok((o, native, live, test_args.into_iter().map(str::to_string).collect::<Vec<_>>()))
    })();
    let (mut o, native, live, test_args) = match parsed {
        Ok(v) => v,
        Err(e) => { eprintln!("test: {e}"); return 2; }
    };
    if native {
        if let Err(code) = retry_master(master).map_err(fail).and_then(|c| native_gate(&c, &o)) { return code; }
        return run_worktree(o, false, Some(pkg));
    }
    match test_miss(&o, pkg, &test_args, "lookup", &[]) {
        Ok(miss) if !miss.is_null() => {
            let errors = miss.get("error").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
            return cargo_test(o, pkg, &test_args, &errors, Some(miss));
        }
        Ok(_) => {}
        Err(e) => return fail(e),
    }
    if live { return live_test(o, pkg, &test_args); }
    let c = match retry_master(master) { Ok(c) => c, Err(e) => return fail(e) };
    let shadow = match one(&c, "overlay-shadow", "overlay-shadow enabled") {
        Ok(v) => v.get("on").and_then(Value::as_bool) == Some(true),
        Err(e) => return fail(e),
    };
    let mut output = Vec::new();
    let (code, completed) = if shadow {
        shadow_test(&c, &mut o, pkg, &mut output)
    } else {
        run_worktree_into(&mut o, false, Some(pkg), &mut output)
    };
    let log = String::from_utf8_lossy(&output);
    let errors = crate::jit_misses::compile_errors(&log);
    if completed && code != CANCELLED_EXIT && !errors.is_empty() { return cargo_test(o, pkg, &test_args, &errors, None); }
    print!("{log}");
    code
}

/// Submit the requested source mode; the master snapshots once and owns the opposite-mode shadow.
/// Capture only the requested run; the second result distinguishes completion from setup/link failure.
fn shadow_test(c: &Client, o: &mut Opts, pkg: &str, output: &mut Vec<u8>) -> (i32, bool) {
    let mut run = || -> Result<(i32, bool), String> {
        use makepad_remote::json_str;
        let root = git(&cwd(), &["rev-parse", "--show-toplevel"]).ok_or("test: run inside the lane's worktree")?;
        let rel = git(&cwd(), &["rev-parse", "--show-prefix"]).filter(|p| !p.is_empty()).unwrap_or_else(|| ".".into());
        let optional = |n: Option<f64>| n.map_or("null".into(), |n| n.to_string());
        let body = format!("{{\"tree\":{},\"also\":[{}],\"jobs\":[{{\"kind\":\"test\",\"holder\":{},\"lane\":{},\"agent\":{},\"cmd\":[{}],\"cwd\":{},\"shadow\":true,\"tree\":{},\"cost\":{},\"est_min\":{},\"cores\":{},\"ram_gb\":{},\"whole\":{},\"class\":{},\"priority\":{},\"loader\":{},\"needs\":{},\"node\":{},\"prio\":{}}}]}}",
            json_str(&root), std::env::var("DEVHUB_ALSO").unwrap_or_default().split_whitespace().map(json_str).collect::<Vec<_>>().join(","), std::process::id(), json_str(&o.req.lane), json_str(o.req.agent.as_deref().unwrap_or("devhub-test")), o.cmd.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","), json_str(rel.trim_end_matches('/')),
            o.tree, json_str(if o.req.cost_asked { o.req.cost.as_str() } else { "" }), optional(o.req.est_min), optional(o.req.cores), optional(o.req.ram_gb), o.req.whole, json_str(&o.req.class), json_str(o.req.priority.as_str()), json_str(o.req.loader.as_deref().unwrap_or("")),
            json_str(&o.req.needs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")), json_str(o.req.node.as_deref().unwrap_or("")), o.req.prio);
        let v = one(c, "job", &format!("job submit {body}"))?;
        let id = v.get("jobs").and_then(Value::as_arr).and_then(|a| a.first()).and_then(Value::as_str).ok_or("test: no job")?;
        let (mut off, mut generation) = (0, (String::new(), 0));
        loop {
            let v = retry_master(|| one(c, "job", &format!("job output {id} {off}")))?;
            let j = crate::state::job_from(v.get("job").ok_or("test: missing job")?);
            let current = (j.lease.clone(), j.run_n);
            if generation != current && j.run_n > 0 {
                generation = current;
                output.clear();
                if off != 0 { off = 0; continue; }
            }
            let bytes = b64_decode(&s(&v, "data")).ok_or("test: bad job output")?;
            output.extend_from_slice(&bytes);
            off = v.get("off").and_then(Value::as_u64).ok_or("test: missing output offset")?;
            if bytes.is_empty() && j.done() {
                let code = if j.state == "cancelled" { CANCELLED_EXIT } else { j.exit.unwrap_or(1) };
                if j.exit.is_none() && !j.note.is_empty() { eprintln!("test: {}", j.note); }
                let seconds = |a: u64, b: u64| b.saturating_sub(a) as f64 / 1000.0;
                let _ = writeln!(output, "devhub test: {pkg} {} in {:.3}s (queue {:.3}s, ship {:.3}s, run {:.3}s) on {}", if code == 0 { "passed" } else if code == CANCELLED_EXIT { "cancelled" } else { "failed" }, seconds(j.submitted_ms, j.ended_ms), seconds(j.submitted_ms, j.granted_ms), seconds(j.granted_ms, j.shipped_ms), seconds(j.shipped_ms, j.ended_ms), j.node);
                o.req.rev = Some(j.rev.clone());
                return Ok((code, j.exit.is_some()));
            }
            if bytes.is_empty() { std::thread::sleep(Duration::from_millis(300)); }
        }
    };
    match run() { Ok(result) => result, Err(e) => (fail(e), false) }
}

fn overlay_selftest(args: &[&str]) -> i32 {
    let mut req = Req::new("overlay-selftest", Kind::Test);
    req.cost = CostClass::Medium;
    req.cost_asked = true;
    req.class = "overlay-selftest".into();
    let mut case = "";
    let mut words = args.iter();
    while let Some(word) = words.next() {
        match (*word, words.next()) {
            ("--node", Some(n)) => req.node = Some(n.to_string()),
            ("--case", Some(n)) => case = n,
            _ => { eprintln!("devhub overlay selftest [--node N] [--case NAME]"); return 2; }
        }
    }
    let mut c = match master() { Ok(c) => c, Err(e) => return fail(e) };
    let held = match acquire(&c, &req) { Ok(h) => h, Err(code) => return code };
    let r = start_there(&mut c, &held, "overlay-selftest", &format!("overlay-selftest {} {case}", held.id))
        .and_then(|v| v.get("n").and_then(Value::as_u64).ok_or("selftest: missing run".into()))
        .and_then(|n| follow_there(&c, &held, n, &mut std::io::stdout()));
    finish(&c, &held, r)
}

fn live_test(mut o: Opts, pkg: &str, args: &[String]) -> i32 {
    let started = Instant::now();
    let mut said = false;
    let result = (|| -> Result<crate::state::Live, String> {
        let root = git(&cwd(), &["rev-parse", "--show-toplevel"]).map(PathBuf::from).ok_or("test --live: run it inside the lane's worktree")?;
        let c = retry_master(master)?;
        let loader = o.req.loader.clone().unwrap_or_default();
        let needs = o.req.needs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",");
        let req = format!("{{\"app\":{},\"repo_dir\":{},\"tree\":{},\"lane\":{},\"agent\":{},\"needs\":{},\"priority\":{},\"loader\":{},\"cost\":\"small\",\"class\":{},\"also\":[{}],\"test_args\":[{}]}}", makepad_remote::json_str(pkg), makepad_remote::json_str(&root.to_string_lossy()), o.tree, makepad_remote::json_str(&o.req.lane), makepad_remote::json_str(o.req.agent.as_deref().unwrap_or("")), makepad_remote::json_str(&needs), makepad_remote::json_str(o.req.priority.as_str()), makepad_remote::json_str(&loader), makepad_remote::json_str(&o.req.class), std::env::var("DEVHUB_ALSO").unwrap_or_default().split_whitespace().map(makepad_remote::json_str).collect::<Vec<_>>().join(","), args.iter().map(|a| makepad_remote::json_str(a)).collect::<Vec<_>>().join(","));
        let selected = if args.last().is_some_and(|a| a == "--") { &args[..args.len() - 1] } else { args };
        let status = || -> Option<crate::state::Live> {
            let v = one(&c, "live", "live list").ok()?;
            v.as_arr()?.iter().map(crate::state::live_from).find(|x| x.test && x.lane == o.req.lane && x.app == pkg && x.test_args == selected)
        };
        let measured_ms = status().and_then(|x| x.patch).and_then(|p| p.tests).map_or(0, |t| t.ms.max(0.0) as u64);
        let notice_after = Duration::from_millis(measured_ms.saturating_add(120_000));
        let v = retry_master(|| {
            // Keep the request open without a deadline. A separate channel reads its phase
            // once it exceeds the previous test time plus two minutes.
            let ch = c.open("live", "").map_err(|e| e.to_string())?;
            ch.send(format!("live test {req}").into_bytes()).map_err(|e| format!("link lost: {e}"))?;
            let mut answer = None;
            loop {
                match ch.recv_timeout(Duration::from_secs(1)) {
                    Ok(makepad_remote::Event::Data(d)) => {
                        for l in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                            let v = json(l).ok_or("live: bad answer")?;
                            if v.get("end").is_some() { return answer.ok_or_else(|| "live: no answer".to_string()); }
                            if let Some(e) = v.get("err").and_then(Value::as_str) { return Err(e.to_string()); }
                            if answer.is_none() { answer = Some(v); }
                        }
                    }
                    Ok(makepad_remote::Event::Opened(_)) | Err(true) => {}
                    Ok(makepad_remote::Event::Closed(makepad_remote::conn::CLOSE_LINK_LOST, why)) => return Err(format!("link lost: {why}")),
                    Ok(makepad_remote::Event::Closed(_, why)) => return Err(why),
                    Err(false) => return Err("the hub connection ended".into()),
                }
                if !said && started.elapsed() > notice_after {
                    let x = status();
                    let phase = match x.as_ref() {
                        None => "waiting for a live job",
                        Some(x) if x.state != "running" => x.state.as_str(),
                        Some(x) if x.pid == 0 => "starting JIT",
                        Some(x) => match x.patch.as_ref() {
                            Some(p) if p.tests.is_some() => "finishing the current call",
                            Some(p) if matches!(p.outcome.as_str(), "recovering" | "restart") => "restarting JIT",
                            Some(p) if matches!(p.outcome.as_str(), "initial" | "patched") => "tests",
                            Some(p) if p.shipped_ms == 0 => "shipping",
                            _ => "patching",
                        },
                    };
                    eprintln!("still running: {phase}");
                    said = true;
                }
            }
        })?;
        Ok(crate::state::live_from(&v))
    })();
    match result {
        Ok(x) => {
            if let Some(p) = &x.patch {
                let errors = crate::jit_misses::compile_errors(&format!("{}\n{}", p.reason, p.output));
                if makepad_remote::hot::Verdict::from_output(makepad_remote::hot::JIT_ERROR_EXIT, &p.reason).is_none() && x.state != "paused" && p.tests.is_none() && matches!(p.outcome.as_str(), "compile_error" | "refused" | "jit_error" | "failed") && !errors.is_empty() {
                    // A refused warm job must release its hot box before the cargo check, and
                    // closing the ledger later must create a fresh JIT rather than reuse it.
                    if let Err(e) = retry_master(|| master().and_then(|c| one(&c, "live", &format!("live release {}", x.id)))) { return fail(e); }
                    o.req.rev = Some(x.rev.clone());
                    return cargo_test(o, pkg, args, &errors, None);
                }
            }
            print!("{}", x.test_result(started.elapsed().as_secs_f64()));
            x.test_code()
        }
        Err(e) => fail(e),
    }
}

fn overlay_inspect(word: &str, lane: &str, args: &[&str]) -> i32 {
    let mut node = None;
    let mut path = None;
    let mut i = 0;
    while i < args.len() {
        match args[i] {
            "--node" if i + 1 < args.len() => { node = Some(args[i + 1].to_string()); i += 2; }
            p if word == "why" && path.is_none() && !p.starts_with("--") => { path = Some(p); i += 1; }
            _ => return fail("overlay: status <lane> | why <lane> <path> [--node N]"),
        }
    }
    if word == "why" && path.is_none() { return fail("overlay why needs a relative path"); }
    let result = (|| -> Result<(), String> {
        let c = master()?;
        let node = match node { Some(n) => n, None => lane_node(&c, lane, None)? };
        let service = format!("overlay-{word}");
        let line = format!("{service} {lane}{}", path.map_or(String::new(), |p| format!(" {p}")));
        for line in ask(&c, Some(&node), &service, &line)? {
            if let Some(e) = json(&line).and_then(|v| v.get("err").and_then(|e| e.as_str()).map(str::to_string)) { return Err(e); }
            println!("{line}");
        }
        Ok(())
    })();
    match result { Ok(()) => 0, Err(e) => fail(e) }
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
            None => s(&v, "lane") == lane && (s(&v, "kind") != "inspect" || s(&v, "src") == "overlay"),
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

/// Copies `rel` (a file or folder) from the lane's dir on `node`, or from live lease `lease`'s
/// (`-`: none), into `dst`: the files written and their sizes.
pub(crate) fn fetch(c: &Client, node: &str, lane: &str, lease: &str, rel: &str, dst: &Path) -> Result<Vec<(PathBuf, u64)>, String> {
    let list = ask(c, Some(node), "get-ls", &format!("get-ls {lane} {lease} {rel}")).map_err(|e| format!("{node}: {e}"))?;
    let mut files = Vec::new();
    for l in list {
        let Some(v) = json(&l) else { continue };
        if let Some(e) = v.get("err").and_then(|e| e.as_str()) {
            return Err(format!("{node}: {e}"));
        }
        let (p, r) = (s(&v, "p"), s(&v, "r"));
        let out = dst.join(&p);
        if Path::new(&p).components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return Err(format!("{node}: bad path {p}"));
        }
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut data = Vec::new();
        loop {
            let v = node_one(c, node, "get-read", &format!("get-read {lane} {lease} {} {r}", data.len()))?;
            let chunk = b64_decode(&s(&v, "data")).ok_or("get-read: bad data")?;
            data.extend_from_slice(&chunk);
            if v.get("eof").and_then(|e| e.as_bool()) != Some(false) || chunk.is_empty() {
                break;
            }
        }
        std::fs::write(&out, &data).map_err(|e| format!("{}: {e}", out.display()))?;
        files.push((out, data.len() as u64));
    }
    if files.is_empty() {
        return Err(format!("{node}: nothing under {rel} for lane {lane}"));
    }
    Ok(files)
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
    if !makepad_devhub_conf::valid_name(lane) || rel.is_empty() || rel.contains(' ') {
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
    let dst = PathBuf::from(dst);
    let (count, bytes) = match fetch(&c, &node, lane, lease.as_deref().unwrap_or("-"), rel, &dst) {
        Ok(files) => (files.len(), files.iter().map(|(_, n)| n).sum::<u64>()),
        Err(e) => return fail(e),
    };
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
            "queued" => format!("{} in queue ({}) waiting {} s{}", ordinal(n(&v, "pos") as usize), s(&v, "reason"), n(&v, "wait_s"), eta(n(&v, "eta_ms") as u64)),
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
        let loader = s(&v, "loader");
        let target = if loader.is_empty() { jit } else { loader };
        println!("{:<6} {:<8} {:<10} {} {} ({}) {} {}{}: {tail}", s(&v, "id"), s(&v, "lane"), format!("{}{at}", s(&v, "kind")), s(&v, "priority"), s(&v, "cost"), s(&v, "cost_source"), s(&v, "src"), if target.is_empty() { "" } else { "jit " }, target);
    }
    0
}

fn nodes(c: &Client) -> i32 {
    match ask(c, None, "nodes", "nodes") {
        Ok(lines) => {
            for l in lines {
                let Some(v) = json(&l) else { continue };
                println!("{:<8} {:<8} {}  [{}; {} leases, {} waiting]", s(&v, "node"), s(&v, "role"), s(&v, "caps_line"), s(&v, "state"), n(&v, "leases"), n(&v, "waiting"));
                // the box's lease latencies (release -> grant of a waiting lease, setup, job, teardown)
                let lat = s(&v, "latency");
                if !lat.is_empty() {
                    println!("{:<8} {:<8} {lat}", "", "");
                }
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
    if !makepad_devhub_conf::valid_name(lane) {
        eprintln!("makepad devhub landed <lane>");
        return 2;
    }
    let mut code = 0;
    let repo = cwd();
    let branch = format!("rc-{lane}");
    let merged = crate::lease::wt::in_work(&repo, &branch);
    let exists = git(&repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_some();
    if exists && !merged {
        eprintln!("{branch} is not merged into work: nothing deleted");
        return 1;
    }
    // the lane is done: so are its live jobs (their apps and leases)
    match one(c, "live", &format!("live end-lane {lane}")) {
        Ok(v) => {
            let ids: Vec<&str> = v.get("released").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).collect();
            if !ids.is_empty() { println!("live jobs released: {}", ids.join(" ")); }
        }
        Err(e) => { code = 1; println!("live jobs: {e}"); }
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
    // `-D`: the content check above (`in_work`) already proved the branch is in work; `-d` would
    // refuse a tip that is only a merge-train "Merge work into rc-x" commit
    if makepad_remote::group::command("git").arg("-C").arg(&repo).args(["branch", "-D", &branch]).status().is_ok_and(|s| s.success()) {
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
    // who asks: an agent or a lane says so (no name: the user)
    let cwd = std::env::current_dir().unwrap_or_default();
    let lane = cwd.to_string_lossy().split("/wt.noindex/").nth(1).and_then(|r| r.split('/').next()).map(str::to_string);
    let by = std::env::var("DEVHUB_AGENT").ok().or_else(|| std::env::var("DEVHUB_LANE").ok()).filter(|x| makepad_devhub_conf::valid_name(x)).or(lane);
    let line = by.map_or_else(|| id.to_string(), |b| format!("{id} by={b}"));
    match one(&c, "cancel", &line) {
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
    if !matches!(word, "prof" | "stats" | "jit" | "overlay" | "lease" | "run" | "test" | "get" | "release" | "cancel" | "queue" | "leases" | "why-queued" | "nodes" | "node" | "disk" | "desk" | "landed" | "clean" | "evict" | "unblock" | "override" | "blocked") {
        return None;
    }
    match args {
        ["prof", rest @ ..] => if let Err(e) = super::prof::Query::parse(rest) { eprintln!("makepad devhub {e}"); return Some(2); },
        ["lease", id, "cancel"] | ["cancel", id] => return Some(cancel(id)),
        ["lease", rest @ ..] => return Some(lease(rest)),
        ["overlay", "selftest", rest @ ..] => return Some(overlay_selftest(rest)),
        ["overlay", word @ ("status" | "why"), lane, rest @ ..] => return Some(overlay_inspect(word, lane, rest)),
        ["run", lane, kind, rest @ ..] => return Some(run(lane, kind, rest)),
        ["test", lane, pkg, rest @ ..] => return Some(test(lane, pkg, rest)),
        ["test", ..] => { eprintln!("makepad devhub test <lane> <pkg> [--lib|--bin B|--test T|--tests] [--features F] [--needs k=v,..] [--node N] [--cost small|medium|large] [--est-min N] [--class name] [--priority interactive|background] [--jit key] [--native [<reason>]|--live] [--tree] [-- args...]"); return Some(2); }
        ["get", lane, rel, dst, rest @ ..] => return Some(get(lane, rel, dst, rest)),
        _ => {}
    }
    let c = match master() {
        Ok(c) => c,
        Err(e) => return Some(fail(e)),
    };
    Some(match args {
        ["prof", rest @ ..] => match one(&c, "prof", &format!("prof {}", rest.join(" "))) {
            Ok(v) => { print!("{}", s(&v, "text")); 0 }
            Err(e) => fail(e),
        },
        ["overlay", "shadow", word @ ("on" | "off" | "status")] => match one(&c, "overlay-shadow", &format!("overlay-shadow {word}")) {
            Ok(v) => { print!("{}", s(&v, "text")); 0 }
            Err(e) => fail(e),
        },
        ["stats", "queue", rest @ ..] if rest.is_empty() || rest == ["--hour"] => match one(&c, "stats", "stats queue") {
            Ok(v) => { print!("{}", s(&v, "text")); 0 }
            Err(e) => fail(e),
        },
        ["stats", ..] => { eprintln!("makepad devhub stats queue [--hour]"); 2 },
        ["jit", "stats", rest @ ..] if rest.is_empty() || rest == ["--day"] => match one(&c, "jit", &format!("jit stats {}", rest.join(" "))) {
            Ok(v) => { print!("{}", s(&v, "text")); 0 }
            Err(e) => fail(e),
        },
        ["jit", "misses"] | ["jit", "misses", "--all"] => match one(&c, "jit", &args.join(" ")) {
            Ok(v) => { print!("{}", s(&v, "text")); 0 }
            Err(e) => fail(e),
        },
        ["jit", "misses", "close", id] => match one(&c, "jit", &format!("jit misses close {id}")) {
            Ok(_) => { println!("JIT miss {id} closed; the next devhub test tries the JIT again"); 0 }
            Err(e) => fail(e),
        },
        ["jit", "miss", "add", pkg, error, rest @ ..] if rest.is_empty() || matches!(rest, ["--repro", _]) => {
            let repro = rest.get(1).copied().unwrap_or("");
            // the worktree it is filed from names the lane
            let lane = std::env::current_dir().ok().and_then(|d| d.ancestors().find(|a| a.join(".git").exists()).and_then(|a| a.file_name()).map(|n| n.to_string_lossy().into_owned())).unwrap_or_default();
            match one(&c, "jit", &format!("jit misses add {{\"pkg\":{},\"error\":{},\"repro\":{},\"lane\":{}}}", makepad_remote::json_str(pkg), makepad_remote::json_str(error), makepad_remote::json_str(repro), makepad_remote::json_str(&lane))) {
                Ok(v) => { println!("JIT miss {} recorded for {pkg}: `devhub jit misses` lists it", s(&v, "id")); 0 }
                Err(e) => fail(e),
            }
        }
        ["jit", ..] => { eprintln!("makepad devhub jit stats [--day] | misses [--all] | misses close <id> | miss add <pkg> \"<error line>\" [--repro \"<cmd>\"]"); 2 },
        ["release", id] => match one(&c, "release", id) {
            Ok(v) => {
                println!("{}", released_line(&v));
                0
            }
            Err(e) => fail(e),
        },
        ["queue" | "leases", rest @ ..] => queue(&c, rest.contains(&"--all")),
        ["why-queued", id] => match ask(&c, None, "why-queued", id) {
            Ok(lines) => {
                for line in lines {
                    let Some(v) = json(&line) else { continue };
                    if let Some(e) = v.get("err").and_then(|v| v.as_str()) { return Some(fail(e)); }
                    let blockers: Vec<_> = v.get("blockers").and_then(|v| v.as_arr()).into_iter().flatten().filter_map(|v| v.as_str()).collect();
                    println!("{:<8} {}{}{}", s(&v, "node"), s(&v, "reason"), if blockers.is_empty() { String::new() } else { format!("; blockers {}", blockers.join(", ")) }, v.get("allocation").and_then(|v| v.as_str()).map_or(String::new(), |s| format!("; {s}")));
                }
                0
            }
            Err(e) => fail(e),
        },
        ["why-queued", ..] => { eprintln!("makepad devhub why-queued <lease>"); 2 },
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
#[test]
fn test_route_takes_the_measured_faster_side() {
    for own in [false, true] {
        for (jit, cargo) in [(None, None), (None, Some(100_000)), (Some(70_000), None)] {
            assert_eq!(test_route(own, jit, cargo).0, !own, "missing measurement uses the default: own={own}, JIT={jit:?}, cargo={cargo:?}");
        }
    }
    for (own, jit, cargo, use_jit, jit_dur, cargo_dur) in [
        (true, 70_000, 100_000, true, "1m10s", "1m40s"),
        (true, 80_000, 100_000, false, "1m20s", "1m40s"),
        (false, 100_000, 70_000, false, "1m40s", "1m10s"),
        (false, 100_000, 80_000, true, "1m40s", "1m20s"),
    ] {
        let (chosen, reason) = test_route(own, Some(jit), Some(cargo));
        assert_eq!(chosen, use_jit, "30% gain switches, 20% keeps the default: own={own}, {reason}");
        assert!(reason.contains(&format!("JIT {jit_dur}")) && reason.contains(&format!("cargo {cargo_dur}")), "measured reason includes both durations: {reason}");
    }
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
