//! The lease words of `makepad devhub`: the holder side of a lease (it beats the master every
//! 2 s while it holds one; a holder that dies lets its lease expire), `run` (ship the worktree,
//! run under a lease, stream the output, exit with its code), and the views and switches.
//!
//! Exit codes: 0 ok, 1 the command failed, 2 usage or denied, 3 the user cancelled the lease
//! ([`CANCELLED_EXIT`]), 4 the lease was lost under the command: no verdict ([`LOST_EXIT`]).

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
    format!("; expected start in ~{} min", eta_min(ms))
}

/// [`eta`] of a queued lease's row.
fn eta_of(v: &Value) -> String {
    if v.get("eta_late").and_then(Value::as_bool).unwrap_or(false) { return format!("; no expected start: {}", s(v, "eta_late_by")); }
    eta(n(v, "eta_ms") as u64)
}

fn eta_min(ms: u64) -> u64 {
    let min = ms.div_ceil(60_000);
    if min > 10 { min.div_ceil(5) * 5 } else { min }
}

/// The timed queue per box: what runs timed there, then who waits for it, in order, with their
/// expected starts (a box's prebuild waited 19 min behind timed leases nobody could see lined up
/// for its box, 11-03).
fn timed_boxes(leases: &[Value]) -> Vec<String> {
    let mut boxes: std::collections::BTreeMap<String, (Vec<String>, Vec<(u64, String)>)> = Default::default();
    for v in leases.iter().filter(|v| s(v, "kind") == "timed") {
        let who = format!("{} {}", s(v, "id"), s(v, "lane"));
        match s(v, "state").as_str() {
            "queued" => {
                let at = s(v, "eta_box");
                let ms = n(v, "eta_ms") as u64;
                let when = if v.get("eta_late").and_then(Value::as_bool).unwrap_or(false) { s(v, "eta_late_by") } else if ms == 0 { "next".to_string() } else { format!("in ~{} min", eta_min(ms)) };
                boxes.entry(if at.is_empty() { "(no box yet)".into() } else { at }).or_default().1.push((ms, format!("{who} {when}")));
            }
            "ended" => {}
            st => { boxes.entry(s(v, "node")).or_default().0.push(format!("{who} {st} {} s", n(v, "granted_s"))); }
        }
    }
    boxes.into_iter().map(|(name, (running, mut waiting))| {
        waiting.sort();
        let waiting: Vec<String> = waiting.into_iter().map(|(_, w)| w).collect();
        let now = if running.is_empty() { String::new() } else { format!("{}; ", running.join(", ")) };
        format!("  {name}: {now}{}", if waiting.is_empty() { "none waiting".to_string() } else { format!("then {}", waiting.join(", ")) })
    }).collect()
}

pub const HELP: &str = "  lease <kind> [--node N [--series S]|--needs k=v,..] [--lane L] [--prio P] [--rev <branch>[@sha][,<name>=<branch>[@sha]]...]
        [--repo-dir D] [--watch-pid P] [--cost small|medium|large] [--est-min N] [--cores N] [--ram-gb N] [--ram-floor-gb N] [--whole]
        [--class name] [--priority interactive|background] [--jit key] [--native <reason>] [--keep <dir>]
        [--input NAME=<archive> ..] [--for <node>] [--exit-to <file>] [-- cmd...]
                                          hold a lease (kinds: build test screen long timed gpu inspect gate ci;
                                          ci: its own tree and target, beside ordinary work, never timed or user boxes;
                                          --needs gui=1: a test that opens windows, only nodes that take screen leases;
                                          gpu=1: a box with a GPU, gpu=<name>: one with that in its name);
                                          with a cmd: run it there and exit with its code. --node N: N or wait for
                                          it, never another box (refused, saying why, when N can never take it);
                                          --series S says why: a series S compared on N (never asked to unpin);
                                          --keep <dir>: what the cmd put in $DEVHUB_KEEP (a failed step's logs) is
                                          copied into <dir> before the lease ends. --ram-floor-gb N: the least RAM its
                                          unasked need is cut to on a box (a tests shard's largest unit). --input NAME=<archive> (lease and
                                          run): a tar archive here (a test corpus) the cmd finds unpacked at $NAME; a
                                          box gets it once by its sha256 and keeps it for the next lease that names it
                                          (any box can take the job). --for <node> (lease and run): cargo builds
                                          for that node's CPU (its hub's report, `nodes`), not `native`, in
                                          <target>/for-<cpu>/, on a box of its OS and arch. --exit-to <file> (lease and
                                          run): `<pid> <exit>` written there as the client ends, for a watcher that is
                                          not its parent (a restarted master); nothing after a signal. $DEVHUB_YIELD: a file that appears
                                          when a timed run is past twice its estimate while a timed ask waits on its
                                          box, or runs over 10 min while one of 10 min or less waits there (it says
                                          who); a timed series or bench checks it between steps and, once
                                          it exists, stops after the current step with its partial results and releases
  run <lane> <kind> [--node N [--series S]|--needs ..] [--with-uncommitted] [--tree] [--patch <diff> ..]
        [--cost small|medium|large] [--est-min N] [--cores N] [--ram-gb N] [--whole]
        [--class name] [--priority interactive|background] [--jit key] [--native <reason>] [--keep <dir>]
        [--for <node>] -- cmd...
                                          --cores N: the run sees N cores, as each unit of a land gate's tests bucket does:
                                          MAKEPAD_CPU_CORES (its pools and JIT workers), MAKEPAD_RUN_JOBS, CARGO_BUILD_JOBS and
                                          RUST_TEST_THREADS are N, unpinned (`devhub test ... --cores 4` runs as a unit of 4).
                                          Its box is charged what its class's runs used, its N until one is measured.
                                          A run's outputs (sheets, grabs, logs): the cmd writes them to $DEVHUB_KEEP and
                                          --keep <dir> copies them into <dir> before the lease ends; a `get` after the
                                          run can find the lane's tree already handed to another lane.
                                          JIT commands of kinds test, screen, gpu, long and build use overlays by default;
                                          --tree uses the work tree. CI, gates, timed and non-JIT commands always use trees.
                                          Main checkout edits need --with-uncommitted.
                                          --patch <diff>: the files a diff (`git diff`) changes ship patched, for
                                          this lease only (a rig's instrumentation): never in the worktree or a
                                          node's tree, which gets every shipped file back at release. Its paths are
                                          those of the repository the run starts in (inside a nested apps/<name>
                                          checkout, that one's). A run that edits tracked files itself names them
                                          in DEVHUB_ALSO: undone the same way.
                                          JIT first: a lane command (or job, or test --native) that cargo-builds a package
                                          is refused unless it is the JIT's own crate, has an open JIT miss, targets a
                                          platform no JIT runs on, or says --native <reason> (native-vs-JIT baseline, timed
                                          perf series); the refusal names the JIT command
                                          A run (or test) whose every process sat idle 10 min goes on as a job and the
                                          command returns, exit 5, the verdict last: `job wait J..` waits on, `job cancel
                                          J..` ends it, `job result L..` has its stacks. devhub ends nothing.
  overlay status <lane> [--node N]        pinned commits, overlay hashes, whiteouts and diff vs work
  overlay why <lane> <path> [--node N]     serving layer and content hash
  overlay selftest [--node N] [--case NAME]
                                          compare real checkouts with overlay runs on a test lease
  overlay shadow on|off|status           background tree/overlay comparisons of JIT tests; last 24 h of evidence
  test <lane> <pkg> [--lib|--bin B|--test T|--tests] [--features F] [--needs k=v,..] [--node N [--series S]] [--cost small|medium|large] [--est-min N] [--cores N] [--class name] [--priority interactive|background] [--jit key] [--native [<reason>]|--live] [--rev R] [--tree] [-- args...]
                                          --rev R: one run at the commits R alone (`<branch>[@sha]` for makepad, then
                                          `,<name>=<branch>[@sha]` per nested apps/<name> repo, as in `work,commercial=main`;
                                          each nested checkout's HEAD where R names none), none of the worktree's uncommitted files.
                                          --live: THE lane check; one warm process per lane, package, target and args,
                                          small cost on a hot box; overlay by default, --tree opts out; only affected tests rerun,
                                          a changed file in a package of its graph that is none of its sources starts
                                          them again, so does one in no package the tests opened, probed or listed the
                                          folder of (a data file); any other in no package keeps the last verdict and says so.
                                          Without --live: one JIT run, the lane's final check; devhub's measured routing may
                                          pick cargo (saying both walls). --native <reason>: cargo test --release (a tree),
                                          only for native-vs-JIT baselines and timed perf series (or an open JIT miss).
                                          compile/resolve misses fall back to cargo on a pool box; open misses skip the JIT;
                                          an open miss is retried once the JIT's inputs changed.
                                          Other JIT errors: a separate event, exit 3; carry on.
                                          A JIT run reads the pinned commit alone, at a root only its own process sees:
                                          a package whose tests read the checkout itself (its history, cargo on it) says
                                          `[package.metadata.makepad] test-tree = \"<why>\"` and its runs take the lane's tree
  get <lane> <rel> <local-dir> [--node N] [--lease L]
                                          copy results back from the lane's dir on its node (`src/..`: the tree
                                          `run` ships, which holds only tracked and ignored files once
                                          the lease ended, `target/..`: its target; with --lease: that live lease's
                                          work, `target/..`, `tmp/..`). Once its lease ended, the lane's tree may go to
                                          another lane's lease at any time: a run's outputs come back with `run ...
                                          --keep <dir>`, which copies what the cmd put in $DEVHUB_KEEP before the lease ends.
                                          A native build is for its box's CPU: a get from a target no `--for` built
                                          says so (`target/for-<cpu>/..`: one that did).
                                          `captures/<file>` (with --node, any lane, no lease): the box's JIT
                                          captures, a report or flight recording, in seconds; then
                                          `makepad inspect file=<dir>/<x.mkpf> pf <query>` here
  release <id>                            end a lease (says how its node cleaned up)
  lease <id> cancel | cancel <id>         the user's cancel of ONE lease: its processes stop, its slot frees, the
                                          lane is kept off that node for 30 min (unless it names it with --node;
                                          a land queue's gate or tests shard keeps none: the next landing's is
                                          other work);
                                          how far its last run got stays: `job result L<id> --tail`
  lease <id> why                          what a running lease does now, on that lease (a second lease on its tree
                                          waits for it): processes, threads and kernel waits, gdb's stacks where the
                                          box lets it attach, tests running over a minute, the last lines
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
  stats cores                             each box's cores as charged past its cores in the last hour
                                          (share of samples over, peak)
  stats calls                             the last day's live calls as their boxes metered them, per
                                          class: launches and calls, peak cores p50/p90/max, mean, wall
  queue | leases [--all]                  live and waiting leases, positions and reasons
  why-queued <lease>                     each candidate box's start blocker and reservation expiry
  replay take <from> <to> <out-dir>        export replay inputs on the master (UTC YYYY-MM-DDTHH:MM:SSZ)
  replay run <dir> <from> <to>             simulate scheduler waits offline from exported inputs
  nodes                                   capabilities, dev switch and leases per node
  node <n> dev on|off                     the node's dev switch (the user's decision)
  node <n> back                           the person hands the node back (on their say-so): the input it saw so
                                          far holds it no more; new input does again
  node <n> [role pool|hot|suite|bench|user-wm|device] [set slots.screen=0 disk_floor_gb=40 ...]
                                          the node's role and budgets (its lease.conf, kept on the node)
  disk <node> [--dry-run]                 the node's disk categories and reclaim
  heap <node>|master [top]                its hub's (or the master's) heap: live, peak and resident
                                          MB, then the call sites holding the most (sampled), top 12
  desk <node> [on|off]                    the Desk Link endpoint of a JIT host (its own render, app input)
  landed <lane> | clean <lane>            remove a landed lane as `lane remove` does: both halves' worktrees and
                                          branches, its dirs on every node; refused while anything is unlanded,
                                          uncommitted, untracked or running";

/// Exit code of a `run`/`lease` whose lease the user cancelled (0 ok, 1 failed, 2 usage or denied).
/// Stderr then says `LEASE CANCELLED by user on <node>: pick another node ...` (or `while queued
/// [for <node>]` when it never got one); a lane's agent reads that as: choose another node and
/// retry, and not retry on this one unless asked (with no node: not ask again unless asked).
pub const CANCELLED_EXIT: i32 = 3;

/// Exit code of a `run`/`lease` whose lease was lost under its command (it expired, the master
/// lost it, or its node lost it or stayed unreachable): the command has no verdict, the code is
/// not to blame; run it again (a landing bucket does, once).
pub const LOST_EXIT: i32 = 4;

/// Exit code of a `run`/`test`/`lease -- cmd` whose run sat idle, every process of its lease, for
/// the idle verdict's 10 min: the run goes on as a job the master follows (`job adopt`), and this
/// client returns to its owner, who decides: `devhub job wait J..` waits on, `devhub job cancel
/// J..` ends it, its stacks are in `devhub job result L..`. devhub ends nothing.
pub const HUNG_EXIT: i32 = 5;

/// How the master ends a lease that infrastructure lost, never a person or the command.
fn lost_how(how: &str) -> bool {
    matches!(how, "expired" | "lost")
}

/// A node call's error that means the lease is gone there: its hub no longer has it, or it stayed
/// unreachable past [`REATTACH_FOR`].
fn lost_error(e: &str) -> bool {
    e.contains("no lease ") || e.contains("stayed unreachable")
}

/// Where the user cancelled it: `Ran(node)` once granted there, `Queued(named)` before any grant
/// (the node the ask named, if it named one).
enum CancelledAt<'a> {
    Ran(&'a str),
    Queued(Option<&'a str>),
}

fn cancelled_notice(id: &str, at: CancelledAt) -> String {
    let (at, node) = match at {
        CancelledAt::Ran(node) => (format!("on {node}"), Some(node)),
        CancelledAt::Queued(Some(node)) => (format!("while queued for {node}"), Some(node)),
        CancelledAt::Queued(None) => ("while queued".to_string(), None),
    };
    let next = match node {
        Some(node) => format!("pick another node (devhub nodes) and retry; don't retry on {node} unless asked"),
        None => "don't ask again unless asked".to_string(),
    };
    format!("LEASE CANCELLED by user {at} ({id}): {next}. Exit code {CANCELLED_EXIT}.")
}

const BEAT: Duration = Duration::from_secs(2);
/// How often a call waiting for its tests says it still does (silence read as a hang, 11-05).
const STILL_RUNNING_EVERY: Duration = Duration::from_secs(300);
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
/// new hub takes the lease back from its ledger); "no lease" and the lease's end ([`Beat::check`])
/// always stand. Said once each way on stderr.
fn through_restarts<T>(id: &str, c: &mut Client, mut f: impl FnMut(&Client) -> Result<T, String>, up: &dyn Fn() -> Result<Client, String>) -> Result<T, String> {
    let mut down: Option<Instant> = None;
    let mut back: Option<Instant> = None;
    // what went away: the master, or the lease's node hub (its self-update restart); said as
    // that when it is back (a hub's roll read "the master is back", 11-06)
    let mut gone = "the master";
    let mut wait = Duration::from_millis(500);
    loop {
        let e = match f(c) {
            Ok(v) => {
                if down.take().is_some() {
                    eprintln!("{id}: {gone} is back; re-attached");
                }
                return Ok(v);
            }
            Err(e) => e,
        };
        if e.starts_with("no lease") || e.starts_with(&format!("{id} ended (")) {
            return Err(e);
        }
        match up() {
            Ok(fresh) => {
                // the master's own link was cut and it answers at once: its restart in place (the
                // dial waited on the socket it kept open), not the node
                let cut = c.link_ended();
                *c = fresh;
                if down.is_some() && back.is_none() {
                    down = Some(Instant::now());
                    back = Some(Instant::now());
                    continue;
                }
                match (down, back) {
                    (None, _) if cut => {
                        eprintln!("{id}: the master's link was cut; re-attaching");
                        down = Some(Instant::now());
                        back = Some(Instant::now());
                    }
                    (None, _) if node_restarting(&e) => {
                        if super::master::host_gone(&e) {
                            eprintln!("{id}: its node is off the network ({e}); the master ends its leases as lost after {} s unless it hears no box at all (its own network), re-attaching if it is back first", super::sched::HOST_GONE_MS / 1000);
                        } else {
                            eprintln!("{id}: its node does not answer ({e}); its hub restarts? re-attaching when it is back");
                        }
                        gone = "its node's hub";
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
                gone = "the master";
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
    first_of(node, service, ask(c, Some(node), service, line)?)
}

/// Tells a granted lease's run a line (its node's `lease-tell`, the run's `$DEVHUB_TOLD` file), its
/// node asked of the master: a tests run's dealer answers its shards so (bucket.rs `Dealer`, a
/// checkout's only).
#[cfg(feature = "checkout")]
pub(crate) fn tell(c: &Client, lease: &str, line: &str) -> Result<(), String> {
    let node = ask(c, None, "leases", "leases")?.iter().filter_map(|l| json(l)).find(|v| s(v, "id") == lease).map(|v| s(&v, "node")).filter(|n| !n.is_empty());
    let node = node.ok_or_else(|| format!("{lease} runs on no node"))?;
    node_one(c, &node, "lease-tell", &format!("lease-tell {lease} {line}")).map(|_| ())
}

/// A node's answer: its first line, an `err` one as `Err`.
fn first_of(node: &str, service: &str, lines: Vec<String>) -> Result<Value, String> {
    let first = lines.first().ok_or_else(|| format!("{node} {service}: no answer"))?;
    let v = json(first).ok_or_else(|| format!("{node} {service}: bad answer"))?;
    match v.get("err").and_then(|e| e.as_str()) {
        Some(e) => Err(format!("{node}: {e}")),
        None => Ok(v),
    }
}

/// The master's control socket, or why there is none.
pub(super) fn master() -> Result<Client, String> {
    let c = control()?;
    if !has(&c, "lease") {
        return Err("the devhub here is not the master (leases live on the master)".into());
    }
    Ok(c)
}

#[track_caller]
fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let o = crate::git_exec::command().arg("-C").arg(dir).args(args).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The lane's worktree and this shell's place in it (`.` at its top): the makepad worktree, also
/// from inside a nested repository's checkout ([`crate::nested`]), a repo of its own (its top
/// pinned the nested HEAD as the makepad commit, and the lease failed on a commit no makepad
/// store has, 11-03).
fn lane_root() -> Option<(PathBuf, String)> {
    lane_root_of(&cwd())
}

fn lane_root_of(dir: &Path) -> Option<(PathBuf, String)> {
    let top = PathBuf::from(git(dir, &["rev-parse", "--show-toplevel"])?);
    let prefix = git(dir, &["rev-parse", "--show-prefix"]).unwrap_or_default();
    let name = top.file_name().and_then(|n| n.to_str()).filter(|n| crate::nested::valid(n)).map(str::to_string);
    let outer = top.parent().filter(|apps| apps.file_name().is_some_and(|a| a == "apps")).and_then(Path::parent).filter(|outer| outer.join(".git").exists());
    let nested = match (outer, name) {
        (Some(outer), Some(name)) => Some((outer.to_path_buf(), format!("{}{prefix}", crate::nested::prefix(&name)))),
        _ => None,
    };
    let (root, rel) = nested.unwrap_or((top, prefix));
    let rel = rel.trim_end_matches('/');
    Some((root, if rel.is_empty() { ".".into() } else { rel.to_string() }))
}

/// A lease asked with no `--lane` takes its checkout's name for its lane: in a lane's worktree its
/// lane, but from the main checkout (or no checkout) `makepad`, whose tree on a box the person's
/// and main's own leases use. An agent's job there ran in that tree and left its Cargo.toml edited
/// on a box (a fresh-clone job, 11-12). Refused for an agent other than main and devhub's own,
/// saying to name the lane; `lane_named`: `DEVHUB_LANE` names it, or this is a lane's worktree.
fn laneless(lane: &str, lane_named: bool, agent: Option<&str>) -> Option<String> {
    let agent = agent.filter(|a| !a.is_empty() && *a != "main" && !a.starts_with("devhub-"))?;
    (!lane_named).then(|| format!("no --lane: from here this lease of {agent} would take lane `{lane}` and its tree on the box, which the person's and main's leases use; name your lane: --lane <lane> (or DEVHUB_LANE=<lane>)"))
}

/// `--lane`, else `DEVHUB_LANE`, else the worktree's directory name.
fn default_lane() -> String {
    std::env::var("DEVHUB_LANE").ok().filter(|l| !l.is_empty()).unwrap_or_else(|| {
        let top = lane_root().map(|(root, _)| root).unwrap_or_else(cwd);
        top.file_name().map(|f| f.to_string_lossy().to_lowercase()).unwrap_or_default()
    })
}

fn pid_alive(pid: i32) -> bool {
    crate::os::pid_alive(pid as u32)
}

struct Opts {
    req: Req,
    watch: Option<i32>,
    /// `--keep <dir>`: what the command put in the lease's keep dir is copied there before the
    /// lease ends ([`kept`])
    keep: Option<PathBuf>,
    cmd: Vec<String>,
    tree: bool,
    /// `--native "<reason>"`: why this command builds natively (lease/native.rs)
    native: String,
    /// `run --patch <diff>`: the files the diffs change, as they ship in place of the worktree's
    /// (lease/patch.rs)
    patched: Vec<(String, Vec<u8>)>,
    /// `test --rev`: the run's sources are its pinned commits alone; nothing of the worktree ships
    commits_only: bool,
    /// `--input NAME=<archive>`: shipped to the box by hash before the run, unpacked at `$NAME`
    inputs: Vec<super::ship::Input>,
    /// `--for <node>`: the box the build is for; [`for_cpu`] turns it into the lease's
    /// [`Req::cpu_target`]
    for_node: Option<String>,
    /// `--exit-to <file>`: this client's exit, written there as it ends ([`write_exit`])
    exit_to: Option<PathBuf>,
}

/// Flags handled by the client instead of fields in the lease request.
#[cfg(test)]
const LOCAL_FLAGS: &[(&str, &str)] = &[
    ("--watch-pid", "The holder watches the process and ends its lease when it exits."),
    ("--keep", "The client copies the command's kept output after the run."),
    ("--exit-to", "The client writes its exit status for its watcher."),
    ("--tree", "The client selects how to ship the run's sources."),
    ("--native", "The client checks the reason for a native build."),
    ("--input", "The client ships and unpacks the input archives."),
];

/// Flags whose request fields need a master or git lookup, with the function that sets them.
#[cfg(test)]
const RESOLVED_FLAGS: &[(&str, &str, &str)] = &[
    ("--for", "cpu_target", "for_cpu"),
    ("--rev", "rev", "parse"),
];

/// Flag arguments that change the named request JSON key from its build default.
#[cfg(test)]
const FLAG_FIELDS: &[(&[&str], &str)] = &[
    (&["--class", "build-example"], "class"),
    (&["--cores", "2"], "cores"),
    (&["--cost", "small"], "cost"),
    (&["--est-min", "3"], "est_min"),
    (&["--jit", "x"], "loader"),
    (&["--lane", "lane-b"], "lane"),
    (&["--needs", "os=linux"], "needs"),
    (&["--node", "box-a"], "node"),
    (&["--prio", "7"], "prio"),
    (&["--priority", "interactive"], "priority"),
    (&["--ram-floor-gb", "1"], "ram_floor_gb"),
    (&["--ram-gb", "2"], "ram_gb"),
    (&["--repo-dir", "/x"], "repo_dir"),
    (&["--series", "s1", "--node", "box-a"], "series"),
    (&["--whole"], "whole"),
];

/// The flags shared by `lease` and `run`.
fn parse(kind: &str, lane: Option<&str>, args: &[&str]) -> Result<Opts, String> {
    let kind = Kind::parse(kind).ok_or_else(|| format!("unknown kind `{kind}`"))?;
    let mut req = Req::new(lane.unwrap_or(""), kind);
    let mut watch = None;
    let mut keep = None;
    let mut tree = false;
    let mut native = String::new();
    let mut inputs = Vec::new();
    let mut for_node = None;
    let mut exit_to = None;
    let mut i = 0;
    let mut cmd = Vec::new();
    while i < args.len() {
        let val = |i: usize| args.get(i + 1).copied().filter(|v| !v.starts_with("--")).ok_or_else(|| format!("{} needs a value", args[i]));
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
            "--ram-floor-gb" => req.ram_floor_gb = Some(val(i)?.parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0).ok_or("--ram-floor-gb is a positive number")?),
            "--class" => req.class = val(i)?.to_string(),
            "--node" => req.node = Some(val(i)?.to_string()),
            "--series" => req.series = Some(val(i)?.to_string()),
            "--needs" => req.needs = parse_needs(val(i)?),
            "--lane" => req.lane = val(i)?.to_string(),
            "--priority" => req.priority = Priority::parse(val(i)?).ok_or("--priority is interactive or background")?,
            "--prio" => req.prio = val(i)?.parse().map_err(|_| "--prio takes a number")?,
            "--rev" => req.rev = Some(val(i)?.to_string()),
            "--repo-dir" => req.repo_dir = Some(val(i)?.to_string()),
            "--watch-pid" => watch = Some(val(i)?.parse().map_err(|_| "--watch-pid takes a pid")?),
            "--keep" => keep = Some(PathBuf::from(val(i)?)),
            "--jit" => req.loader = Some(val(i)?.to_string()),
            "--native" => native = val(i)?.to_string(),
            "--input" => inputs.push(super::ship::Input::parse(val(i)?)?),
            "--for" => for_node = Some(val(i)?.to_string()),
            "--exit-to" => exit_to = Some(PathBuf::from(val(i)?)),
            a => return Err(format!("unknown flag `{a}`")),
        }
        i += 2;
    }
    req.check_series()?;
    let root = lane_root().map(|(root, _)| root);
    if req.lane.is_empty() {
        req.lane = default_lane();
        let named = std::env::var("DEVHUB_LANE").is_ok_and(|l| !l.is_empty());
        let in_lane = root.as_deref().is_some_and(|root| !super::ship::is_main_checkout(root));
        if let Some(why) = laneless(&req.lane, named || in_lane, std::env::var("DEVHUB_AGENT").ok().as_deref()) {
            return Err(why);
        }
    }
    // the lane's repo: revisions are looked up and recovered work lands there
    if req.repo_dir.is_none() {
        req.repo_dir = root.map(|root| root.to_string_lossy().into_owned());
    }
    // branch names become commits here, once: what ships is what was ordered, however long the
    // lease queues and whatever other landings do to the branches meanwhile
    if let Some(spec) = &req.rev {
        let dir = req.repo_dir.as_deref().ok_or("--rev needs a git repo (--repo-dir)")?;
        req.rev = Some(pin_lane_rev(Path::new(dir), spec, &req.lane)?);
    }
    if req.class.is_empty() && !cmd.is_empty() { req.class = super::run_class(&req.lane, kind, &cmd); }
    req.exec_profile = super::argv::ExecProfile::of(&cmd);
    Ok(Opts { req, watch, keep, cmd, tree, native, patched: Vec::new(), commits_only: false, inputs, for_node, exit_to })
}

/// `--for <node>`: the lease builds for that box's CPU, as its hub reports it
/// ([`super::Caps::cpu_target`]), in place of `native`, the CPU of whichever box builds, on a box
/// of its OS and arch (the flag is the host target's). A native release built on a box with
/// AVX-512 died with "Illegal instruction" on a box without (11-07).
fn for_cpu(c: &Client, o: &mut Opts) -> Result<(), String> {
    let Some(node) = &o.for_node else { return Ok(()) };
    let lines = retry_master(|| ask(c, None, "nodes", "nodes"))?;
    let line = lines.iter().filter_map(|l| json(l)).find(|v| s(v, "node") == *node).ok_or_else(|| format!("--for {node}: no such node"))?;
    let caps = line.get("caps").cloned().unwrap_or(Value::Null);
    let target = s(&caps, "cpu_target");
    if target.is_empty() {
        return Err(format!("--for {node}: the node reports no CPU target (it is down, or its hub predates `--for`)"));
    }
    for k in ["os", "arch"] {
        if !o.req.needs.iter().any(|(n, _)| n == k) { o.req.needs.push((k.to_string(), s(&caps, k))); }
    }
    eprintln!("devhub: building for {node}'s CPU, {target}, into the target dir's for-{target}/");
    o.req.cpu_target = Some(target);
    Ok(())
}

/// A granted lease as the holder knows it.
struct Held {
    id: String,
    node: String,
    work: String,
    env: Vec<(String, String)>,
    beat: Beat,
    /// the job its run becomes at the idle verdict ([`hand_over`]): its fields, as JSON; None:
    /// it stays with this client (a bucket's run, devhub's own)
    adopt: Option<String>,
    /// what its owner hears once its run went on as a job
    handed: std::sync::Mutex<Option<String>>,
}

/// The holder's beat: a thread of its own beats the master every [`BEAT`] while the lease is
/// held, whatever the node calls wait on. Beats in the loop of the node calls stopped for as long
/// as one call waited: a `lease-log` OPEN a busy node left unanswered for 10 s (the expiry) let
/// a landing's lease expire (11-03). Through master restarts it keeps trying; it stops
/// once the master says the lease ended ([`Beat::ended`]) or the holder drops it.
struct Beat {
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ended: std::sync::Arc<std::sync::Mutex<Option<(String, String)>>>,
    /// the idle verdict, once a beat brought it (`hung`), until its run's follower takes it
    hung: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Beat {
    fn start(id: &str) -> Beat {
        use std::sync::atomic::Ordering;
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let ended = std::sync::Arc::new(std::sync::Mutex::new(None));
        let hung = std::sync::Arc::new(std::sync::Mutex::new(None));
        let (halt, end, verdict, id) = (stop.clone(), ended.clone(), hung.clone(), id.to_string());
        let thread = std::thread::Builder::new().name("devhub-beat".into()).spawn(move || {
            let mut client: Option<Client> = None;
            let mut lost = false;
            let mut told = false;
            loop {
                let t = Instant::now();
                while t.elapsed() < BEAT {
                    if halt.load(Ordering::Acquire) { return; }
                    std::thread::sleep(Duration::from_millis(100));
                }
                let answer = match client.as_ref().map_or_else(master, |c| Ok(c.clone())) {
                    Ok(c) => { let a = one(&c, "beat", &id); client = Some(c); a }
                    Err(e) => Err(e),
                };
                match answer {
                    Ok(v) if s(&v, "state") == "ended" => {
                        *end.lock().unwrap() = Some((s(&v, "how"), s(&v, "reason")));
                        return;
                    }
                    Ok(v) => {
                        if std::mem::take(&mut lost) { eprintln!("{id}: the master is back; still held"); }
                        // the lease runs on: only its owner can tell a wait from a hang
                        if !told && v.get("idle").is_some() {
                            told = true;
                            eprintln!("{}", s(&v, "idle"));
                            if v.get("hung").and_then(Value::as_bool) == Some(true) { *verdict.lock().unwrap() = Some(s(&v, "idle")); }
                        }
                    }
                    // a restarting master keeps its granted leases (leases.live): only "no lease" ends it
                    Err(e) if e.starts_with("no lease") => {
                        *end.lock().unwrap() = Some(("lost".into(), e));
                        return;
                    }
                    Err(e) => {
                        client = None;
                        if !lost { eprintln!("{id}: the master does not answer ({e}); still holding, retrying"); }
                        lost = true;
                    }
                }
            }
        }).ok();
        Beat { stop, ended, hung, thread }
    }

    /// The idle verdict a beat brought, taken once.
    fn hung(&self) -> Option<String> {
        self.hung.lock().unwrap().take()
    }

    /// How and why the lease ended, once a beat heard it.
    fn ended(&self) -> Option<(String, String)> {
        self.ended.lock().unwrap().clone()
    }

    /// Err once the lease ended: what a node call loop returns then.
    fn check(&self, id: &str) -> Result<(), String> {
        match self.ended() {
            Some((how, _)) => Err(format!("{id} ended ({how})")),
            None => Ok(()),
        }
    }
}

impl Drop for Beat {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(t) = self.thread.take() { let _ = t.join(); }
    }
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
            // not ranked yet (the master just took it back after a restart): no "0th in queue ()"
            "queued" if n(&v, "pos") < 1.0 => format!("{id} {}: queued, not ranked yet", req.kind.as_str()),
            "queued" => format!("{id} {}: {} in queue ({}){}", req.kind.as_str(), ordinal(n(&v, "pos") as usize), s(&v, "reason"), eta_of(&v)),
            "starting" => format!("{id} {}@{}: granted, setting up", req.kind.as_str(), s(&v, "node")),
            "granted" => {
                if !note.is_empty() { eprintln!("{id} {}@{}: granted{note}", req.kind.as_str(), s(&v, "node")); }
                // a healthy log and `inspect` both read the same with no window on the screen
                if req.kind == Kind::Screen { eprintln!("{id} screen@{}: its windows are hidden from the person, as every lease's are (MAKEPAD_HIDE_WINDOWS=1): not on the screen unless the command takes that off. A person's launch is `devhub show <app> --on <node>` (`--native`: compiled)", s(&v, "node")); }
                let deployment = s(&v, "deployment");
                if req.kind == Kind::Timed && !deployment.is_empty() {
                    let short = deployment.split(',').map(|sha| &sha[..sha.len().min(10)]).collect::<Vec<_>>().join(",");
                    eprintln!("{id} timed@{}: runs on deployment {short}", s(&v, "node"));
                }
                if req.kind == Kind::Timed {
                    eprintln!("{id}: between steps, check $DEVHUB_YIELD: it appears when this run is past twice its estimate while a timed ask waits on its box, or runs over 10 min while one of 10 min or less waits there; then stop after the current step with what you have and release");
                }
                let start = v.get("start");
                let env = match start.and_then(|st| st.get("env")) {
                    Some(Value::Obj(pairs)) => pairs.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or("").to_string())).collect(),
                    _ => Vec::new(),
                };
                if !s(&v, "reason").is_empty() { eprintln!("{id}: {}", s(&v, "reason")); }
                let beat = Beat::start(&id);
                return Ok(Held { id, node: s(&v, "node"), work: start.map(|st| s(st, "work")).unwrap_or_default(), env, beat, adopt: None, handed: Default::default() });
            }
            _ => {
                let how = s(&v, "how");
                let why = s(&v, "reason");
                if how == "cancelled" {
                    let node = s(&v, "node");
                    let at = if node.is_empty() { CancelledAt::Queued(req.node.as_deref()) } else { CancelledAt::Ran(&node) };
                    eprintln!("{}", cancelled_notice(&id, at));
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
fn run_there(c: &Client, held: &Held, rel: &str, cmd: &[String], inputs: &[super::ship::Input]) -> Result<i32, String> {
    run_there_into(c, held, rel, cmd, inputs, &mut std::io::stdout())
}

fn run_there_into(c: &Client, held: &Held, rel: &str, cmd: &[String], inputs: &[super::ship::Input], out: &mut dyn Write) -> Result<i32, String> {
    let mut c = c.clone();
    let cmd = if inputs.is_empty() { cmd.to_vec() } else {
        let t = Instant::now();
        let placed = through_restarts(&held.id, &mut c, |c| {
            held.beat.check(&held.id)?;
            super::ship::send_inputs(c, &held.node, &held.id, inputs)
        }, &master)?;
        for p in &placed {
            let how = if p.sent == 0 { "held there".to_string() } else { format!("shipped ({:.1} MB) and unpacked in {:.1} s", p.sent as f64 / 1048576.0, t.elapsed().as_secs_f64()) };
            eprintln!("{}: input {} ({}) on {}: {how}", held.id, p.name, &p.hash[..12], held.node);
        }
        super::ship::with_inputs(cmd, &placed)
    };
    let argv = makepad_network::digest::base64_encode(cmd.join("\0").as_bytes());
    let request = crate::proto::request_id();
    let v = start_there(&mut c, held, "lease-run", &format!("lease-run {} {rel} {argv} {{\"request_id\":\"{request}\"}}", held.id))?;
    let run = v.get("n").and_then(super::num).map_or(1, |x| x as u64);
    follow_there(&c, held, run, out)
}

/// Asks for the run while admission, resolution or a hub restart delays it (the lease's own
/// thread beats meanwhile: [`Beat`]).
fn start_there(c: &mut Client, held: &Held, service: &str, line: &str) -> Result<Value, String> {
    let mut last = String::new();
    loop {
        let v = through_restarts(&held.id, c, |c| {
            held.beat.check(&held.id)?;
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

/// How long a follower of a run's output waits for more once it has all of it: the run's next
/// lines come with its next look. Asked again at once, a run that writes all the time had its log
/// asked tens of times a second, each a route through the master's hub (11-06).
pub(crate) const FOLLOW_PACE: Duration = Duration::from_millis(100);

fn follow_there(c: &Client, held: &Held, run: u64, out: &mut dyn Write) -> Result<i32, String> {
    let mut c = c.clone();
    let mut off = 0u64;
    let mut logs = crate::cli::NodeAsks::new(&held.node, "lease-log");
    // a verdict from before this run (its ship) is no verdict on it
    let _ = held.beat.hung();
    loop {
        if let Some(word) = held.beat.hung() {
            if hand_over(&c, held, run, &word) { return Err(format!("{} goes on as a job", held.id)); }
        }
        let v = through_restarts(&held.id, &mut c, |c| {
            held.beat.check(&held.id)?;
            first_of(&held.node, "lease-log", logs.ask(c, &format!("lease-log {} {run} {off}", held.id))?)
        }, &master)?;
        let data = b64_decode(&s(&v, "data")).ok_or("lease-log: bad data")?;
        let _ = out.write_all(&data);
        let _ = out.flush();
        if !data.is_empty() && data.len() < super::exec::LOG_CHUNK {
            std::thread::sleep(FOLLOW_PACE);
        }
        off = v.get("off").and_then(super::num).map_or(off + data.len() as u64, |x| x as u64);
        if data.is_empty() {
            if let Some(code) = v.get("exit").filter(|e| !e.is_null()).and_then(super::num) {
                // the node ended it for memory: the run's exit is that verdict, said with its why
                if let Some(why) = v.get("memory").and_then(|m| m.as_str()) {
                    return Err(format!("{} on {} ended (memory): {why}", held.id, held.node));
                }
                return Ok(code as i32);
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }
}

/// The idle verdict on the run: it goes on as a job the master follows (`job adopt`), and this
/// client returns to its owner, waiting on its end, with the verdict ([`HUNG_EXIT`]). Its run's
/// output so far is above; the whole of it goes in the job's log. False: not handed over (a run
/// devhub watches itself, or the master refused): it is followed here, the verdict said.
fn hand_over(c: &Client, held: &Held, run: u64, word: &str) -> bool {
    let Some(body) = held.adopt.as_deref().and_then(|b| b.strip_prefix('{')) else { return false };
    let line = format!("job adopt {{\"lease\":{},\"pid\":{},\"run\":{run},{body}", makepad_remote::json_str(&held.id), std::process::id());
    match retry_master(|| one(c, "job", &line)) {
        Ok(v) => {
            let job = s(&v, "job");
            *held.handed.lock().unwrap() = Some(format!("{0} goes on as job {job}, devhub ends nothing: `devhub job wait {job}` waits on, `devhub job cancel {job}` ends it; its whole output: `devhub job result {job}` once it ends\n{0}: {word}", held.id));
            true
        }
        Err(e) => {
            eprintln!("{}: its run stays with this client ({e})", held.id);
            false
        }
    }
}

/// The job a run's client hands its run to at the idle verdict ([`hand_over`]), as `job adopt`'s
/// fields; None for a run devhub itself waits on (a bucket's `--exit-to`, an agent `devhub-..`).
fn adopt_body(o: &Opts, cwd: &str) -> Option<String> {
    let j = makepad_remote::json_str;
    let agent = o.req.agent.clone().or_else(|| std::env::var("DEVHUB_AGENT").ok()).unwrap_or_default();
    if o.exit_to.is_some() || agent.starts_with("devhub-") || o.cmd.is_empty() { return None; }
    Some(format!("{{\"lane\":{},\"agent\":{},\"kind\":{},\"cmd\":[{}],\"cwd\":{},\"rev\":{},\"repo_dir\":{},\"class\":{},\"priority\":{},\"src\":{}}}",
        j(&o.req.lane), j(&agent), j(o.req.kind.as_str()), o.cmd.iter().map(|a| j(a)).collect::<Vec<_>>().join(","), j(cwd), j(o.req.rev.as_deref().unwrap_or("")), j(o.req.repo_dir.as_deref().unwrap_or("")), j(&o.req.class), j(o.req.priority.as_str()), j(o.req.src.as_str())))
}

/// The JIT-first gate (lease/native.rs) for a command the lane runs here: prints its hint;
/// Err(2) when the master refuses the native build.
fn native_gate(c: &Client, o: &Opts) -> Result<(), i32> {
    if o.cmd.is_empty() { return Ok(()); }
    let (top, rel) = lane_root().unwrap_or_else(|| (PathBuf::new(), ".".into()));
    let root = o.req.repo_dir.clone().unwrap_or_else(|| top.to_string_lossy().into_owned());
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
    let mut o = match parse(kind, None, rest) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("makepad devhub lease: {e}");
            return 2;
        }
    };
    let code = lease_with(&mut o);
    write_exit(o.exit_to.as_deref(), code)
}

/// What a watcher that is not this client's parent knows of its end: `--exit-to`'s file, `<pid>
/// <exit>`, written whole as the client ends. A bucket run's watcher is the master that started it;
/// a master that started again (a crash, a switch-over: not an exec) adopts the run by its pid and
/// cannot reap it, and a run that had ended before it adopted it is no process at all. Both read
/// as "no exit status" and the run had no verdict (the tests bucket at 16:51Z, 11-08). One that
/// ends by a signal writes nothing: its end is not known.
fn write_exit(to: Option<&Path>, code: i32) -> i32 {
    if let Some(path) = to {
        let temp = path.with_extension("exit.tmp");
        if let Err(e) = std::fs::write(&temp, format!("{} {code}\n", std::process::id())).and_then(|()| std::fs::rename(&temp, path)) {
            eprintln!("makepad devhub: --exit-to {}: {e}", path.display());
        }
    }
    code
}

/// The exit `--exit-to` wrote for the client with `pid` ([`write_exit`]); None: none, or another
/// client's. Read by the bucket's adopted runs (the checkout's).
#[cfg(feature = "checkout")]
pub(crate) fn read_exit(path: &Path, pid: u32) -> Option<i32> {
    let text = std::fs::read_to_string(path).ok()?;
    let (who, code) = text.trim().split_once(' ')?;
    (who.parse::<u32>().ok()? == pid).then(|| code.parse().ok()).flatten()
}

fn lease_with(o: &mut Opts) -> i32 {
    let c = match master() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    if let Err(e) = for_cpu(&c, o) {
        eprintln!("makepad devhub lease: {e}");
        return 2;
    }
    if let Err(code) = native_gate(&c, o) { return code; }
    let mut held = match acquire(&c, &o.req) {
        Ok(h) => h,
        Err(code) => return code,
    };
    held.adopt = adopt_body(o, ".");
    // a lease at a rev: the tree it holds, every repository's branch and commit
    if let Some(rev) = &o.req.rev {
        eprintln!("{} {}: sources at {}", held.id, held.node, super::rev_said(rev));
    }
    if !o.cmd.is_empty() {
        // a lease ships nothing: its command runs on what earlier ships left in the lane's dir (a
        // script only the worktree had was "No such file" on the box, and looked like a lost tree)
        if o.req.rev.is_none() && o.req.repo_dir.is_some() {
            eprintln!("{}: runs in lane {}'s dir on {} as earlier ships left it (a lease ships nothing; `devhub run {} {} [--with-uncommitted] -- ..` ships this worktree)", held.id, o.req.lane, held.node, o.req.lane, o.req.kind.as_str());
        }
        let r = run_there(&c, &held, ".", &o.cmd, &o.inputs);
        // what the run put in $DEVHUB_KEEP, before the lease's end takes its tmp
        if let (Ok(_), Some(dst)) = (&r, &o.keep) {
            eprintln!("{}", keep_said(&held.id, dst, &kept(&c, &held.node, &o.req.lane, &held.id, dst)));
        }
        let code = finish(&c, &held, r);
        // the verdict goes last: a `| tail -1` reader sees it
        if let Some(said) = held.handed.lock().unwrap().as_deref() { println!("{said}"); }
        return code;
    }
    println!("{} granted {}@{} work={}", held.id, o.req.kind.as_str(), held.node, held.work);
    println!("export DEVHUB_LEASE={}", held.id);
    for (k, v) in &held.env {
        println!("export {k}={}", shell_quote(v));
    }
    let _ = std::io::stdout().flush();
    // the holder: its beat runs until the lease ends, its watch pid dies, or this process is killed
    loop {
        std::thread::sleep(Duration::from_millis(500));
        if o.watch.is_some_and(|p| !pid_alive(p)) {
            release(&c, &held.id, None);
            eprintln!("{}: watched pid ended; released", held.id);
            return 0;
        }
        match held.beat.ended() {
            Some((how, _)) if how == "cancelled" => {
                eprintln!("{}", cancelled_notice(&held.id, CancelledAt::Ran(&held.node)));
                return CANCELLED_EXIT;
            }
            Some((how, why)) if how == "lost" && why.starts_with("no lease") => return fail(format!("beat {}: {why}", held.id)),
            Some((how, why)) => {
                eprintln!("{}: ended ({how}){}", held.id, if why.is_empty() { String::new() } else { format!(": {why}") });
                return 0;
            }
            None => {}
        }
    }
}

fn finish(c: &Client, held: &Held, r: Result<i32, String>) -> i32 {
    // its run went on as a job: the lease is the job's, not this client's to release
    if held.handed.lock().unwrap().is_some() { return HUNG_EXIT; }
    let end = one(c, "beat", &held.id).ok().filter(|v| s(v, "state") == "ended");
    let ended = end.as_ref().map(|v| s(v, "how"));
    // the user cancelled it: its processes were stopped, so the run's own result is just the fallout
    if ended.as_deref() == Some("cancelled") {
        eprintln!("{}", cancelled_notice(&held.id, CancelledAt::Ran(&held.node)));
        return CANCELLED_EXIT;
    }
    // a verdict on the command, not a lost box: run again, it hangs again
    if let Some(how) = ended.as_deref().filter(|h| matches!(*h, "hung" | "disk" | "memory")) {
        return fail(format!("{} on {} ended ({how}): {}", held.id, held.node, end.as_ref().map_or(String::new(), |v| s(v, "reason"))));
    }
    let (exit, code) = match r {
        Ok(code) => (Some(code), code),
        // the lease went under the command (not a person, not the command): no verdict at all
        Err(e) if ended.as_deref().is_some_and(lost_how) || lost_error(&e) => {
            eprintln!("LEASE LOST on {} ({}): {e}; the command has no verdict, the code is not to blame: run it again. Exit code {LOST_EXIT}.", held.node, held.id);
            (None, LOST_EXIT)
        }
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

/// `L3 screen@node-a (lane x) released; node-a cleaned up: 2 processes stopped, 120 MB freed`.
fn released_line(v: &Value) -> String {
    let node = s(v, "node");
    let at = if node.is_empty() { String::new() } else { format!("@{node}") };
    let head = format!("{} {}{at} (lane {}) {}", s(v, "id"), s(v, "kind"), s(v, "lane"), match s(v, "how").as_str() {
        "released" | "" => "released".to_string(),
        "cancelled" => { let why = s(v, "reason"); if why.is_empty() { "cancelled".to_string() } else { why } }
        how => format!("ended ({how})"),
    });
    let Some(c) = v.get("cleanup") else { return head };
    let ended = s(c, "ended");
    let ended = if ended.is_empty() { ended } else { format!("\n{ended}") };
    let line = match s(c, "cleanup").as_str() {
        "ok" => format!("{head}; {} cleaned up: {} stopped, {} MB freed{}", s(c, "node"), super::master::plural(n(c, "stopped") as usize, "process", "processes"), n(c, "freed_mb"), if n(c, "leak") > 0.0 { format!(", {} LEFT (see the inbox)", super::master::plural(n(c, "leak") as usize, "process", "processes")) } else { String::new() }),
        other => format!("{head}; {other}"),
    };
    // a timed run: what shared its box, and whether its timings are clean
    let quiet = c.get("box").filter(|b| b.get("samples").is_some()).map_or(String::new(), |b| match c.get("unquiet").and_then(Value::as_str) {
        Some(why) => format!("\nbox {}: NOT QUIET ({why}): its timings are not clean, run it again on a quiet box; {}", s(c, "node"), super::master::box_line(b)),
        None => format!("\nbox {}: quiet: {}", s(c, "node"), super::master::box_line(b)),
    });
    let failed = super::master::strs(c.get("failed_tests"));
    let failed = if failed.is_empty() { String::new() } else { format!("\nfailed tests: {}; their output: `devhub job result {} --tail`", failed.join(", "), s(v, "id")) };
    let kept = if failed.is_empty() && c.get("kept_tail").and_then(Value::as_bool) == Some(true) { format!("\nhow far it got: `devhub job result {} --tail`", s(v, "id")) } else { String::new() };
    format!("{line}{ended}{quiet}{failed}{kept}")
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

/// One checkout's HEAD as a side of a pinned `--rev`: `<branch>@<sha>` (a detached HEAD is
/// labelled `run-<lane>`).
fn head_side(dir: &Path, lane: &str) -> Result<String, String> {
    let sha = git(dir, &["rev-parse", "HEAD"]).ok_or_else(|| format!("{}: no HEAD", dir.display()))?;
    let branch = git(dir, &["symbolic-ref", "-q", "--short", "HEAD"]).filter(|b| valid_label(b)).unwrap_or_else(|| format!("run-{lane}"));
    Ok(format!("{branch}@{sha}"))
}

/// The worktree's revision for `run`: its HEAD (+ each nested repository's), as a pinned `--rev`
/// (`<branch>@<sha>[,<name>=<branch>@<sha>]...`).
fn head_rev(root: &Path, lane: &str) -> Result<String, String> {
    pin_lane_rev(root, &head_side(root, lane)?, lane)
}

/// A `--rev` given for a lane worktree, pinned whole: each nested repository checked out in it
/// ([`crate::nested`]) that the rev does not name is pinned at its HEAD, so its packages are in
/// the lease's tree (a makepad-only pin left them out of the overlay: `package .. is not a member
/// of the workspace`).
pub fn pin_lane_rev(root: &Path, spec: &str, lane: &str) -> Result<String, String> {
    let named: Vec<&str> = spec.split(',').skip(1).filter_map(|side| side.split_once('=').map(|(name, _)| name)).collect();
    let mut whole = spec.to_string();
    for name in crate::nested::names(root).into_iter().filter(|n| !named.contains(&n.as_str())) {
        whole.push_str(&format!(",{name}={}", head_side(&root.join(crate::nested::dir(&name)), lane)?));
    }
    super::pin_revs(root, &whole)
}

/// Ships the complete uncommitted set and base blobs into the lease's source layer; its line names
/// each repository's branch and commit of the pinned `rev`.
fn ship(c: &Client, root: &Path, held: &Held, rev: &str, uncommitted: bool, commits_only: bool, patched: &[(String, Vec<u8>)]) -> Result<(), String> {
    let pin = &super::Pin::of_rev(rev).ok_or_else(|| format!("{}: rev `{rev}` is not pinned", held.id))?;
    if held.work.is_empty() {
        return Err(format!("{}: no work dir to ship to", held.id));
    }
    let (files, deleted) = if commits_only { (Vec::new(), Vec::new()) } else { super::ship::uncommitted(root, pin)? };
    let (files, deleted) = if commits_only || uncommitted || !super::ship::is_main_checkout(root) {
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
    // a patched file ships patched, from a staging copy, whatever else would ship it
    entries.retain(|e| !patched.iter().any(|(p, _)| *p == e.path));
    let deleted: Vec<String> = deleted.into_iter().filter(|d| !patched.iter().any(|(p, _)| p == d)).collect();
    let t = Instant::now();
    let sent = if entries.is_empty() { Ok(super::ship::Shipped::default()) } else { super::ship::send(c, &held.node, &held.id, root, &entries, false) };
    let mut sent = sent?;
    let mut paths: Vec<String> = entries.iter().map(|e| e.path.clone()).collect();
    if !patched.is_empty() {
        let stage = std::env::temp_dir().join(format!("devhub-patch-{}-{}", held.id, std::process::id()));
        let staged = stage_patched(root, &stage, patched).and_then(|staged| super::ship::send(c, &held.node, &held.id, &stage, &staged, false));
        let _ = std::fs::remove_dir_all(&stage);
        let staged = staged?;
        (sent.files, sent.sent, sent.bytes) = (sent.files + staged.files, sent.sent + staged.sent, sent.bytes + staged.bytes);
        paths.extend(patched.iter().map(|(p, _)| p.clone()));
    }
    let list = |v: &[String]| v.iter().map(|p| makepad_remote::json_str(p)).collect::<Vec<_>>().join(",");
    let bases = super::ship::bases_json(root, pin, &paths.iter().chain(&deleted).cloned().collect::<Vec<_>>())?;
    let body = format!("{{\"shipped\":[{}],\"deleted\":[{}],\"bases\":{bases}}}", list(&paths), list(&deleted));
    let done = node_one(c, &held.node, "ship-done", &format!("ship-done {} {}", held.id, makepad_network::digest::base64_encode(body.as_bytes())))?;
    let source = done.get("source").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let patched_note = if patched.is_empty() { String::new() } else { format!(", {} of them patched by --patch", patched.len()) };
    let at = if commits_only { format!("{} alone (--rev)", super::rev_said(rev)) } else { format!("the worktree's HEAD {}", super::rev_said(rev)) };
    eprintln!("{} {}: sources at {at}, source {source}; shipped {} changed file(s){patched_note} ({} KB, {} sent) and {} deletion(s) on top in {:.1} s", held.id, held.node, paths.len(), sent.bytes >> 10, sent.sent, deleted.len(), t.elapsed().as_secs_f64());
    Ok(())
}

/// The patched files under `stage` at their paths, with their worktree file's mode: what ships.
fn stage_patched(root: &Path, stage: &Path, patched: &[(String, Vec<u8>)]) -> Result<Vec<super::ship::Entry>, String> {
    let mut entries = Vec::new();
    for (path, text) in patched {
        if !super::ship::inside(path) || super::ship::never_ships(path) {
            return Err(format!("--patch: {path} never ships"));
        }
        let to = stage.join(path);
        let e = |e: std::io::Error| format!("--patch: staging {path}: {e}");
        std::fs::create_dir_all(to.parent().unwrap()).map_err(e)?;
        std::fs::write(&to, text).map_err(e)?;
        if let Ok(m) = std::fs::metadata(root.join(path)) { std::fs::set_permissions(&to, m.permissions()).map_err(e)?; }
        super::ship::entries(stage, path, &mut entries)?;
    }
    Ok(entries)
}

/// `--patch <diff>` flags before the command, out of `rest`: each diff applied to the worktree's
/// files (lease/patch.rs), the files they change. `at`: the lane's root and where in it the run
/// was started ([`lane_root`]). A diff's paths are relative to the repository the run starts in,
/// as `git diff` writes them and `git apply` reads them there: from inside a nested checkout
/// (`apps/<name>`, [`crate::nested`]) they are that repository's.
fn take_patches(at: Option<(&Path, &str)>, rest: &mut Vec<&str>) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut patched: Vec<(String, Vec<u8>)> = Vec::new();
    while let Some(i) = rest.iter().take_while(|a| **a != "--").position(|a| *a == "--patch") {
        let file = rest.get(i + 1).copied().filter(|f| *f != "--").ok_or("--patch needs a diff file")?;
        let (root, rel) = at.ok_or("--patch: run it from inside the lane's worktree")?;
        let names = crate::nested::names(root);
        let repo = crate::nested::split(&format!("{rel}/"), names.iter().map(String::as_str)).map_or(String::new(), |(name, _)| crate::nested::prefix(name));
        let diff = std::fs::read_to_string(file).map_err(|e| format!("--patch {file}: {e}"))?;
        for (path, text) in super::patch::apply_in(&root.join(&repo), &diff).map_err(|e| format!("--patch {file}: {e}"))? {
            let path = format!("{repo}{path}");
            if patched.iter().any(|(p, _)| *p == path) { return Err(format!("--patch {file}: {path} is in another patch too")); }
            patched.push((path, text));
        }
        rest.drain(i..i + 2);
    }
    Ok(patched)
}

/// `run <lane> <kind> … -- cmd`: pin HEAD, select the source layer, ship, run and release.
fn run(lane: &str, kind: &str, rest: &[&str]) -> i32 {
    let flags = rest.iter().position(|a| *a == "--").unwrap_or(rest.len());
    let uncommitted = rest[..flags].contains(&"--with-uncommitted");
    let mut rest: Vec<&str> = rest.iter().enumerate().filter(|(i, a)| *i >= flags || **a != "--with-uncommitted").map(|(_, a)| *a).collect();
    let patched = match take_patches(lane_root().as_ref().map(|(root, rel)| (root.as_path(), rel.as_str())), &mut rest) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("makepad devhub run: {e}");
            return 2;
        }
    };
    let mut o = match parse(kind, Some(lane), &rest) {
        Ok(o) if !o.cmd.is_empty() => Opts { patched, ..o },
        Ok(_) => {
            eprintln!("makepad devhub run <lane> <kind> [--node N|--needs ..] [--tree] -- cmd...");
            return 2;
        }
        Err(e) => {
            eprintln!("makepad devhub run: {e}");
            return 2;
        }
    };
    let c = match retry_master(master) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    if let Err(e) = for_cpu(&c, &mut o) {
        eprintln!("makepad devhub run: {e}");
        return 2;
    }
    let exit_to = o.exit_to.clone();
    let code = match native_gate(&c, &o) {
        Ok(()) => run_worktree(o, uncommitted, None),
        Err(code) => code,
    };
    write_exit(exit_to.as_deref(), code)
}

/// Runs at HEAD with the same overlay as `run`.
fn run_worktree(mut o: Opts, uncommitted: bool, test: Option<&str>) -> i32 {
    run_worktree_into(&mut o, uncommitted, test, &mut std::io::stdout()).0
}

/// The second result says whether the command ended, as opposed to a setup/link failure.
fn run_worktree_into(o: &mut Opts, uncommitted: bool, test: Option<&str>, out: &mut dyn Write) -> (i32, bool) {
    let t = Instant::now();
    let word = if test.is_some() { "test" } else { "run" };
    let Some((root, rel)) = lane_root() else {
        eprintln!("makepad devhub {word}: run it from inside the lane's worktree");
        return (2, false);
    };
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
    o.req.exec_profile = super::argv::ExecProfile::of(&o.cmd);
    // a JIT `makepad test` of a package whose tests read the checkout itself (cargo on it, its
    // history: `test-tree`) takes the lane's tree, as `devhub test` gives it (`test`: it did): an
    // overlay's tree is one only the JIT process reads
    if test.is_none() && !o.tree && Src::for_run(o.req.kind, &o.cmd, false) == Src::Overlay {
        let rev = o.req.rev.clone().unwrap_or_default();
        let Some(pin) = super::Pin::of_rev(&rev) else {
            eprintln!("makepad devhub {word}: rev `{rev}` is not pinned");
            return (2, false);
        };
        match crate::jit_misses::test_tree_of(&root, &pin.pair(), &o.cmd) {
            Ok(Some(line)) => {
                eprintln!("makepad devhub {word}: {line}");
                o.tree = true;
            }
            Ok(None) => {}
            Err(e) => {
                eprintln!("makepad devhub {word}: {e}");
                return (2, false);
            }
        }
    }
    o.req.src = Src::for_run(o.req.kind, &o.cmd, o.tree);
    let c = match retry_master(master) {
        Ok(c) => c,
        Err(e) => return (fail(e), false),
    };
    let queued = Instant::now();
    let mut held = match acquire(&c, &o.req) {
        Ok(h) => h,
        Err(code) => return (code, false),
    };
    held.adopt = adopt_body(o, rel.trim_end_matches('/'));
    let queue = queued.elapsed();
    let shipping = Instant::now();
    let rev = o.req.rev.clone().unwrap_or_default();
    // a node hub restarting mid-ship keeps the lease (its new hub takes it back from the
    // ledger): the ship is made again once it answers
    let shipped = through_restarts(&held.id, &mut c.clone(), |c| ship(c, &root, &held, &rev, uncommitted, o.commits_only, &o.patched), &master);
    let ship = shipping.elapsed();
    let running = Instant::now();
    let mut tally = crate::testlog::Tally::default();
    let r = shipped.and_then(|_| run_there_into(&c, &held, rel.trim_end_matches('/'), &o.cmd, &o.inputs, &mut Tee { out: &mut *out, tally: &mut tally }));
    let run = running.elapsed();
    let completed = r.is_ok();
    // what the run put in $DEVHUB_KEEP, before the lease's end takes its tmp: `devhub run` and
    // `devhub test` took `--keep` and copied nothing back, three Source Library rig runs' grabs
    // in a row (11-07)
    if let (Ok(_), Some(dst)) = (&r, &o.keep) {
        eprintln!("{}", keep_said(&held.id, dst, &kept(&c, &held.node, &o.req.lane, &held.id, dst)));
    }
    let mut code = finish(&c, &held, r);
    // its run went on as a job: no verdict yet; what its owner hears goes last
    if let Some(said) = held.handed.lock().unwrap().as_deref() {
        let _ = writeln!(out, "{said}");
        return (code, false);
    }
    if let Some(pkg) = test {
        // a test run that executed no test is no pass
        let mut none = if code == 0 { tally.no_pass(&o.cmd) } else { None };
        if let Some(why) = &mut none { crate::testlog::no_pass_hint(why, &root, &rev, &o.cmd); }
        if none.is_some() { code = 1; }
        let verdict = match (code, &none) { (0, _) => "passed".into(), (CANCELLED_EXIT, _) => "cancelled".into(), (LOST_EXIT, _) => "had no verdict (its lease was lost)".into(), (_, Some(why)) => format!("failed: {why}"), _ => "failed".to_string() };
        let _ = writeln!(out, "devhub test: {pkg} {verdict} in {:.3}s (queue {:.3}s, ship {:.3}s, run {:.3}s) on {}", t.elapsed().as_secs_f64(), queue.as_secs_f64(), ship.as_secs_f64(), run.as_secs_f64(), held.node);
    }
    (code, completed)
}

/// The run's output on its way out, tallied for its tests.
struct Tee<'a> {
    out: &'a mut dyn Write,
    tally: &'a mut crate::testlog::Tally,
}

impl Write for Tee<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let n = self.out.write(bytes)?;
        self.tally.feed(&bytes[..n]);
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.out.flush()
    }
}

fn test_miss(o: &Opts, pkg: &str, args: &[String], verb: &str, errors: &[String], inputs: Option<&str>) -> Result<Value, String> {
    let req = format!("{{\"pkg\":{},\"target\":{},\"lane\":{},\"rev\":{},\"repo\":{},\"error\":[{}],\"inputs\":{}}}", makepad_remote::json_str(pkg), makepad_remote::json_str(&crate::jit_misses::target(args)), makepad_remote::json_str(&o.req.lane), makepad_remote::json_str(o.req.rev.as_deref().unwrap_or("")), makepad_remote::json_str(&miss_repo(o)), errors.iter().take(5).map(|e| makepad_remote::json_str(e)).collect::<Vec<_>>().join(","), inputs.map_or("null".to_string(), makepad_remote::json_str));
    retry_master(|| master().and_then(|c| one(&c, "jit", &format!("jit misses {verb} {req}"))))
}

/// The lane's worktree a miss of its tests is looked up and keyed in.
fn miss_repo(o: &Opts) -> String {
    o.req.repo_dir.clone().or_else(|| lane_root().map(|(root, _)| root.to_string_lossy().into_owned())).unwrap_or_default()
}

/// What a JIT run of `pkg` reads of the lane's trees ([`crate::jit_misses::inputs_at`]), read off
/// the caller's thread as the run ships: a miss it records is keyed by these, never by the trees
/// once cargo has answered. M64's lane edited its crate in the 7 s between the JIT's failure and
/// the miss's record (11-06): read then, the edit would have been the miss's.
type MissInputs = std::thread::JoinHandle<Option<String>>;

fn miss_inputs(o: &Opts, pkg: &str) -> MissInputs {
    let (repo, rev, pkg) = (miss_repo(o), o.req.rev.clone().unwrap_or_default(), pkg.to_string());
    std::thread::spawn(move || crate::jit_misses::inputs_at(&repo, &rev, &pkg))
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

/// `jit_log`: what the JIT run that failed printed, kept first whatever cargo says: a run whose
/// cargo check passed kept only cargo's output, and the JIT's own failing message was gone (11-07).
fn cargo_test(mut o: Opts, pkg: &str, args: &[String], errors: &[String], known: Option<Value>, inputs: Option<MissInputs>, jit_log: Option<&str>) -> i32 {
    if let Some(log) = jit_log.filter(|l| !l.trim().is_empty()) {
        println!("devhub test: the JIT run's output:");
        print!("{log}");
        if !log.ends_with('\n') { println!(); }
    }
    if let Some(miss) = &known { println!("{}", crate::jit_misses::notice(miss)); }
    else { eprintln!("devhub test: JIT compile/resolve failure on {pkg} {}; checking with cargo on a pool box", crate::jit_misses::target(args)); }
    let _ = std::io::stdout().flush();
    o.cmd = ["cargo", "test", "--release", "--message-format=json-render-diagnostics", "-p", pkg].iter().map(|s| s.to_string()).chain(args.iter().cloned()).collect();
    o.tree = true;
    // a named box stays the box (its cargo run is the one asked for); else a pool box
    if o.req.node.is_none() {
        o.req.needs.retain(|(k, _)| k != "role");
        o.req.needs.push(("role".into(), "pool".into()));
    }
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
    // a lane whose JIT is older than work's: its own code, not the JIT, until it runs on work's
    let repo = o.req.repo_dir.clone().unwrap_or_else(|| ".".into());
    let behind = o.req.rev.as_deref().and_then(super::Pin::of_rev).and_then(|pin| crate::jit_misses::behind_work_jit(Path::new(&repo), &pin.pair()));
    if let (None, Some(base)) = (&known, &behind) {
        eprintln!("devhub test: not a JIT miss: this lane's JIT is older than work's (work changed the JIT since the lane's base {}), and its code may use what landed since: merge work and run it again (cargo builds it; the JIT said {})", &base[..base.len().min(10)], errors.first().map_or("", String::as_str));
    } else if known.is_none() {
        let inputs = inputs.and_then(|h| h.join().ok().flatten());
        match test_miss(&o, pkg, args, "record", errors, inputs.as_deref()) {
            Ok(miss) => println!("{}", crate::jit_misses::notice(&miss)),
            Err(e) => eprintln!("devhub test: JIT miss on {pkg} {} (recording failed: {e}): the JIT said {}; cargo builds it", crate::jit_misses::target(args), errors.first().map_or("", String::as_str)),
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
        let (mut native, mut live, mut target, mut rev_spec) = (false, false, None::<String>, None::<&str>);
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
                "--rev" => { rev_spec = Some(val()?); i += 1; }
                "--tree" => flags.push("--tree"),
                "--needs" | "--node" | "--series" | "--priority" | "--jit" | "--cost" | "--est-min" | "--cores" | "--class" => { flags.extend([rest[i], val()?]); i += 1; }
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
        // `--rev`: the commits alone (a landed sha, say), under the same JIT, none of the lane's
        // uncommitted files with them (a lane reverted a file by hand to leave it out, 11-06)
        let rev = match rev_spec { Some(spec) => pin_lane_rev(&root, spec, &o.req.lane)?, None => head_rev(&root, &o.req.lane)? };
        let pin = super::Pin::of_rev(&rev).ok_or("test: missing commit pair")?;
        o.req.rev = Some(rev);
        o.commits_only = rev_spec.is_some();
        if live && o.commits_only {
            return Err("--live follows the lane's saves; --rev runs its commits once (drop one of them)".into());
        }
        // Leave the live default to makepad test: the lib, else the package's only harness target.
        if live && (native || target.as_deref() == Some(ALL_TESTS) || select.contains(&"--features")) {
            return Err("--live selects one target (--lib, --bin B or --test T; default: the lib, else the package's only harness target); --native, --tests and --features cannot be combined with it".into());
        }
        if live && (o.req.node.is_some() || o.req.cost != CostClass::Small || o.req.est_min.is_some()) {
            return Err("--live uses small cost and hot-box routing; omit --node, --cost and --est-min".into());
        }
        let default_class = o.req.class.is_empty();
        if default_class {
            o.req.class = target.map_or_else(|| format!("test:{pkg}"), |t| format!("test:{pkg}:{t}"));
            if live { o.req.class = format!("live:{}", o.req.class); }
        }
        let own = if native { false } else {
            let (own, tree) = crate::jit_misses::test_package(&root, &pin.pair(), pkg)?;
            // tests that read the checkout itself (its history, cargo on it) say so in their
            // Cargo.toml, and run in the lane's tree: an overlay has neither
            if let Some(why) = tree.filter(|_| !o.tree) {
                eprintln!("devhub test: {}", crate::jit_misses::tree_line(pkg, &why));
                o.tree = true;
            }
            own
        };
        // Warm iteration always uses the JIT; measured routing is only for one-shot runs.
        if !native && !live {
            let cargo_class = format!("native:{}", o.req.class);
            let v = retry_master(|| master().and_then(|c| one(&c, "stats", &format!("stats class {} {}\t{cargo_class}", o.req.kind.as_str(), o.req.class))))?;
            let runs = v.get("runs");
            let (use_jit, reason) = test_route(own, runs.and_then(|r| r.get(&o.req.class)).and_then(Value::as_u64), runs.and_then(|r| r.get(&cargo_class)).and_then(Value::as_u64));
            eprintln!("devhub test: {}: {reason}", o.req.class);
            if !use_jit { native = true; }
        }
        // a filtered run is measured apart from its whole suite (routing above reads the suite's)
        let filter = super::filter_key(&args);
        if default_class && !filter.is_empty() { o.req.class = format!("{}?{filter}", o.req.class); }
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
    match test_miss(&o, pkg, &test_args, "lookup", &[], None) {
        Ok(miss) if !miss.is_null() => {
            let errors = miss.get("error").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
            return cargo_test(o, pkg, &test_args, &errors, Some(miss), None, None);
        }
        Ok(_) => {}
        Err(e) => return fail(e),
    }
    let inputs = miss_inputs(&o, pkg);
    if live { return live_test(o, pkg, &test_args, inputs); }
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
    if completed && code != CANCELLED_EXIT && !errors.is_empty() { return cargo_test(o, pkg, &test_args, &errors, None, Some(inputs), Some(&*log)); }
    print!("{log}");
    code
}

/// Submit the requested source mode; the master snapshots once and owns the opposite-mode shadow.
/// Capture only the requested run; the second result distinguishes completion from setup/link failure.
fn shadow_test(c: &Client, o: &mut Opts, pkg: &str, output: &mut Vec<u8>) -> (i32, bool) {
    let mut run = || -> Result<(i32, bool), String> {
        use makepad_remote::json_str;
        let (root, rel) = lane_root().ok_or("test: run inside the lane's worktree")?;
        let root = root.to_string_lossy().into_owned();
        let optional = |n: Option<f64>| n.map_or("null".into(), |n| n.to_string());
        let body = format!("{{\"tree\":{},\"also\":[{}],\"jobs\":[{{\"kind\":\"test\",\"holder\":{},\"lane\":{},\"agent\":{},\"cmd\":[{}],\"cwd\":{},\"shadow\":true,\"tree\":{},\"cost\":{},\"est_min\":{},\"cores\":{},\"ram_gb\":{},\"whole\":{},\"class\":{},\"priority\":{},\"loader\":{},\"needs\":{},\"node\":{},\"prio\":{}}}]}}",
            json_str(&root), std::env::var("DEVHUB_ALSO").unwrap_or_default().split_whitespace().map(json_str).collect::<Vec<_>>().join(","), std::process::id(), json_str(&o.req.lane), json_str(o.req.agent.as_deref().unwrap_or("devhub-test")), o.cmd.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","), json_str(rel.trim_end_matches('/')),
            o.tree, json_str(if o.req.cost_asked { o.req.cost.as_str() } else { "" }), optional(o.req.est_min), optional(o.req.cores), optional(o.req.ram_gb), o.req.whole, json_str(&o.req.class), json_str(o.req.priority.as_str()), json_str(o.req.loader.as_deref().unwrap_or("")),
            json_str(&o.req.needs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")), json_str(o.req.node.as_deref().unwrap_or("")), o.req.prio);
        let request = crate::proto::request_id();
        // `--rev`: the master runs the job at those commits alone
        let rev = if o.commits_only { format!("\"rev\":{},", json_str(o.req.rev.as_deref().unwrap_or(""))) } else { String::new() };
        let line = format!("job submit {{\"request_id\":\"{request}\",{rev}{}", &body[1..]);
        let v = retry_master(|| one(c, "job", &line))?;
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
                // the job's state is the verdict (one that ran no test failed with exit 0)
                let code = match j.state.as_str() { "cancelled" => CANCELLED_EXIT, "ok" => 0, _ => j.exit.filter(|e| *e != 0).unwrap_or(1) };
                if j.exit.is_none() && !j.note.is_empty() { eprintln!("test: {}", j.note); }
                if code != 0 && j.exit == Some(0) { if let Some(why) = j.errors.first() { eprintln!("test: {why}"); } }
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

fn live_test(mut o: Opts, pkg: &str, args: &[String], inputs: MissInputs) -> i32 {
    let started = Instant::now();
    let result = (|| -> Result<crate::state::Live, String> {
        let (root, _) = lane_root().ok_or("test --live: run it inside the lane's worktree")?;
        let c = retry_master(master)?;
        let loader = o.req.loader.clone().unwrap_or_default();
        let needs = o.req.needs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",");
        let req = format!("{{\"app\":{},\"repo_dir\":{},\"tree\":{},\"lane\":{},\"agent\":{},\"needs\":{},\"priority\":{},\"loader\":{},\"cost\":\"small\",\"class\":{},\"also\":[{}],\"test_args\":[{}]}}", makepad_remote::json_str(pkg), makepad_remote::json_str(&root.to_string_lossy()), o.tree, makepad_remote::json_str(&o.req.lane), makepad_remote::json_str(o.req.agent.as_deref().unwrap_or("")), makepad_remote::json_str(&needs), makepad_remote::json_str(o.req.priority.as_str()), makepad_remote::json_str(&loader), makepad_remote::json_str(&o.req.class), std::env::var("DEVHUB_ALSO").unwrap_or_default().split_whitespace().map(makepad_remote::json_str).collect::<Vec<_>>().join(","), args.iter().map(|a| makepad_remote::json_str(a)).collect::<Vec<_>>().join(","));
        let selected = if args.last().is_some_and(|a| a == "--") { &args[..args.len() - 1] } else { args };
        // the session, and what devhub tells its owner of it (its call far past its measure)
        let status = || -> Option<(crate::state::Live, Option<String>)> {
            let v = one(&c, "live", "live list").ok()?;
            v.as_arr()?.iter().map(|v| (crate::state::live_from(v), v.get("word").and_then(Value::as_str).map(str::to_string))).find(|(x, _)| x.test && x.lane == o.req.lane && x.app == pkg && x.test_args == selected)
        };
        let measured_ms = status().and_then(|(x, _)| x.patch).and_then(|p| p.tests).map_or(0, |t| t.ms.max(0.0) as u64);
        let mut notice_after = Duration::from_millis(measured_ms.saturating_add(120_000));
        let mut told = String::new();
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
                if started.elapsed() > notice_after {
                    let (x, word) = status().map_or((None, None), |(x, w)| (Some(x), w));
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
                    // a session placed again says so while it waits for its box
                    let note = x.as_ref().filter(|x| x.state == "starting" && !x.note.is_empty()).map_or(String::new(), |x| format!(": {}", x.note));
                    eprintln!("still running: {phase}{note} ({} min)", started.elapsed().as_secs() / 60);
                    if let Some(word) = word.filter(|w| *w != told) {
                        eprintln!("devhub: {word}");
                        told = word;
                    }
                    notice_after = started.elapsed() + STILL_RUNNING_EVERY;
                }
            }
        })?;
        Ok(crate::state::live_from(&v))
    })();
    match result {
        Ok(x) => {
            if let Some(p) = &x.patch {
                let said = format!("{}\n{}", p.reason, p.output);
                let errors = crate::jit_misses::compile_errors(&said);
                if makepad_remote::hot::Verdict::from_output(makepad_remote::hot::JIT_ERROR_EXIT, &p.reason).is_none() && x.state != "paused" && p.tests.is_none() && matches!(p.outcome.as_str(), "compile_error" | "refused" | "jit_error" | "failed") && !errors.is_empty() {
                    // A refused warm job must release its hot box before the cargo check, and
                    // closing the ledger later must create a fresh JIT rather than reuse it.
                    if let Err(e) = retry_master(|| master().and_then(|c| one(&c, "live", &format!("live release {}", x.id)))) { return fail(e); }
                    o.req.rev = Some(x.rev.clone());
                    return cargo_test(o, pkg, args, &errors, None, Some(inputs), Some(&said));
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
    let files = fetch_under(c, node, lane, lease, rel, dst, false)?;
    if files.is_empty() {
        return Err(format!("{node}: nothing under {rel} for lane {lane}"));
    }
    Ok(files)
}

/// What a run of live lease `lease` put in its keep dir (`DEVHUB_KEEP`: a failed step's logs,
/// tools/run/fixture/keep_logs.sh), copied into `dst` before the lease ends and its scratch goes:
/// the files and their sizes, none when it kept nothing.
pub(crate) fn kept(c: &Client, node: &str, lane: &str, lease: &str, dst: &Path) -> Result<Vec<(PathBuf, u64)>, String> {
    fetch_under(c, node, lane, lease, &format!("tmp/{}", super::KEEP_DIR), dst, true)
}

/// A line for a holder's log: what [`kept`] copied, or why it could not (nothing when nothing was kept).
#[cfg(feature = "checkout")]
pub(crate) fn kept_line(id: &str, dst: &Path, r: &Result<Vec<(PathBuf, u64)>, String>) -> Option<String> {
    match r {
        Ok(files) if files.is_empty() => None,
        Ok(files) => Some(format!("{id}: kept {} ({:.1} MB) of its failed steps' logs in {}", super::master::plural(files.len(), "file", "files"), files.iter().map(|(_, n)| n).sum::<u64>() as f64 / 1048576.0, dst.display())),
        Err(e) => Some(format!("{id}: its failed steps' logs could not be kept: {e}")),
    }
}

/// What `--keep <dir>` brought back of a run's $DEVHUB_KEEP ([`kept`]), said even when it was
/// nothing: an empty copy said nothing, and a lane read its missing grabs as a lost run.
fn keep_said(id: &str, dst: &Path, r: &Result<Vec<(PathBuf, u64)>, String>) -> String {
    match r {
        Ok(files) if files.is_empty() => format!("{id}: kept nothing in {}: the run put no file in $DEVHUB_KEEP", dst.display()),
        Ok(files) => format!("{id}: kept {} ({:.1} MB) in {}", super::master::plural(files.len(), "file", "files"), files.iter().map(|(_, n)| n).sum::<u64>() as f64 / 1048576.0, dst.display()),
        Err(e) => format!("{id}: --keep {}: nothing kept: {e}", dst.display()),
    }
}

/// [`fetch`]'s copy, none an empty list; `inside`: `rel`'s own folder is left out of the paths
/// written under `dst`.
fn fetch_under(c: &Client, node: &str, lane: &str, lease: &str, rel: &str, dst: &Path, inside: bool) -> Result<Vec<(PathBuf, u64)>, String> {
    fetch_from(&|service, line| ask(c, Some(node), service, line), node, lane, lease, rel, dst, inside)
}

/// The same lease-file protocol through either a client router or the master's node link.
pub(super) fn fetch_from(ask: &dyn Fn(&str, &str) -> Result<Vec<String>, String>, node: &str, lane: &str, lease: &str, rel: &str, dst: &Path, inside: bool) -> Result<Vec<(PathBuf, u64)>, String> {
    let list = ask("get-ls", &format!("get-ls {lane} {lease} {rel}")).map_err(|e| format!("{node}: {e}"))?;
    let mut files = Vec::new();
    for l in list {
        let Some(v) = json(&l) else { continue };
        if let Some(e) = v.get("err").and_then(|e| e.as_str()) {
            return Err(format!("{node}: {e}"));
        }
        let (p, r) = (s(&v, "p"), s(&v, "r"));
        let p = if inside { p.split_once('/').map_or(p.clone(), |(_, under)| under.to_string()) } else { p };
        let out = dst.join(&p);
        if Path::new(&p).components().any(|c| !matches!(c, std::path::Component::Normal(_))) {
            return Err(format!("{node}: bad path {p}"));
        }
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let mut data = Vec::new();
        loop {
            let lines = ask("get-read", &format!("get-read {lane} {lease} {} {r}", data.len()))?;
            let first = lines.first().ok_or_else(|| format!("{node} get-read: no answer"))?;
            let v = json(first).ok_or_else(|| format!("{node} get-read: bad answer"))?;
            if let Some(e) = v.get("err").and_then(Value::as_str) { return Err(format!("{node}: {e}")); }
            let chunk = b64_decode(&s(&v, "data")).ok_or("get-read: bad data")?;
            data.extend_from_slice(&chunk);
            if v.get("eof").and_then(|e| e.as_bool()) != Some(false) || chunk.is_empty() {
                break;
            }
        }
        std::fs::write(&out, &data).map_err(|e| format!("{}: {e}", out.display()))?;
        files.push((out, data.len() as u64));
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
        eprintln!("makepad devhub get <lane> <rel> <local-dir> [--node N] [--lease L] (rel: a path relative to the lane's dir, or captures/<file>: the box's JIT captures; no spaces)");
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
    if let Some(note) = cpu_note(&c, &node, lease.as_deref(), rel) { eprintln!("{note}"); }
    0
}

/// A native build is for the CPU of the box that built it (`-C target-cpu=native`): on a box
/// without its features it dies with "Illegal instruction" (an AVX-512 build, 11-07). Said when
/// a `get` takes files from a build target that no `--for` built.
fn cpu_note(c: &Client, node: &str, lease: Option<&str>, rel: &str) -> Option<String> {
    let under = rel.strip_prefix("target/")?;
    let built_for = match lease {
        // a live lease's `target/` is its own target dir, a `--for` lease's `for-<cpu>/`
        Some(id) => ask(c, None, "leases", "leases").ok()?.iter().filter_map(|l| json(l)).find(|v| s(v, "id") == id)
            .and_then(|v| v.get("start").map(|st| s(st, "target")))
            .is_some_and(|t| Path::new(&t).file_name().is_some_and(|f| f.to_string_lossy().starts_with("for-"))),
        None => under.starts_with("for-"),
    };
    if built_for { return None; }
    let nodes = ask(c, None, "nodes", "nodes").unwrap_or_default();
    let cpu = nodes.iter().filter_map(|l| json(l)).find(|v| s(v, "node") == node).and_then(|v| v.get("caps").map(|c| s(c, "cpu_target"))).filter(|t| !t.is_empty());
    Some(format!(
        "devhub get: a native build in {rel} is for {node}'s own CPU{}: it runs on boxes with that CPU's features; for another box, build with `--for <node>` and get target/for-<cpu>/..",
        cpu.map_or(String::new(), |t| format!(" ({t})"))
    ))
}

fn queue(c: &Client, all: bool) -> i32 {
    let lines = match ask(c, None, "leases", if all { "leases all" } else { "leases" }) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    if lines.is_empty() {
        println!("no leases");
    }
    let leases: Vec<Value> = lines.iter().filter_map(|l| json(l)).collect();
    for v in &leases {
        let v = v.clone();
        let node = s(&v, "node");
        let at = if node.is_empty() { String::new() } else { format!("@{node}") };
        let tail = match s(&v, "state").as_str() {
            "queued" if n(&v, "pos") < 1.0 => format!("queued, not ranked yet, waiting {} s", n(&v, "wait_s")),
            "queued" => format!("{} in queue ({}) waiting {} s{}", ordinal(n(&v, "pos") as usize), s(&v, "reason"), n(&v, "wait_s"), eta_of(&v)),
            "ended" => format!("ended ({}){}", s(&v, "how"), if s(&v, "reason").is_empty() { String::new() } else { format!(": {}", s(&v, "reason")) }),
            st => {
                let info = v.get("info").filter(|i| !i.is_null());
                let run = info.map(|i| s(i, "running")).filter(|r| !r.is_empty()).map_or(String::new(), |r| format!(" running `{r}`"));
                let ram = info.and_then(|i| i.get("held_mb")).and_then(super::num).map_or(String::new(), |m| format!(" ram {:.1} GB", m / 1024.0));
                let cores = s(&v, "cores_now");
                let disk = info.and_then(|i| i.get("disk_mb")).and_then(super::num).map_or(String::new(), |m| format!(" disk {:.1} GB", m / 1024.0));
                let agent = s(&v, "agent");
                let past = n(&v, "past_budget");
                format!(
                    "{st} {}s ago, beat {}s ago{}{run}{ram}{cores}{disk}{}{}",
                    n(&v, "granted_s"),
                    n(&v, "beat_s"),
                    if agent.is_empty() { String::new() } else { format!(" agent {agent}") },
                    if past > 0.0 { format!(" past the box's budget by {past} cores (background work yields)") } else { String::new() },
                    v.get("start").map(|x| format!(" work={}", s(x, "work"))).unwrap_or_default()
                )
            }
        };
        let jit = s(&v, "jit");
        let loader = s(&v, "loader");
        let target = if loader.is_empty() { jit } else { loader };
        println!("{:<6} {:<8} {:<10} {} {} ({}) {} {}{}: {tail}", s(&v, "id"), s(&v, "lane"), format!("{}{at}", s(&v, "kind")), s(&v, "priority"), s(&v, "cost"), s(&v, "cost_source"), s(&v, "src"), if target.is_empty() { "" } else { "jit " }, target);
    }
    let timed = timed_boxes(&leases);
    if !timed.is_empty() {
        println!("timed queue per box:");
        for line in timed { println!("{line}"); }
    }
    0
}

fn nodes(c: &Client) -> i32 {
    match ask(c, None, "nodes", "nodes master") {
        Ok(lines) => {
            for l in lines {
                let Some(v) = json(&l) else { continue };
                // the master's own build: what its scheduler runs
                if v.get("master").is_some() {
                    let (up, hub) = (crate::proto::wall_ms().saturating_sub(n(&v, "up_ms") as u64), s(&v, "hub"));
                    println!("{:<8} {:<8} hub {} (this build up {})", s(&v, "master"), "master", &hub[..hub.len().min(10)], super::sched::dur(up));
                    let network = s(&v, "network");
                    if !network.is_empty() {
                        println!("{:<8} {:<8} {network}", "", "");
                    }
                    continue;
                }
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

/// `inspect <node>/<pid|app> [--lease L] [--read-only] <cmd...>` on a node: under an inspect
/// lease, through the master's `linspect` (None: this machine's JIT or no master; plain `inspect`
/// handles it). A JIT no devhub lease started is refused first ([`crate::cli::leave_alone`]).
fn inspect(target: &str, lease: Option<&str>, read_only: bool, cmd: &[&str]) -> Option<i32> {
    let c = control().ok()?;
    if !has(&c, "linspect") {
        return None;
    }
    let (node, jit) = target.split_once('/')?;
    if node == local_name(&c) {
        return None;
    }
    if cmd.is_empty() {
        eprintln!("makepad devhub inspect <node>/<pid|app> [--lease L] [--read-only] <cmd...>");
        return Some(2);
    }
    match crate::cli::whose(&c, node, false).map(|jits| crate::cli::leave_alone(&jits, node, jit, cmd, read_only)) {
        Ok(None) => {}
        Ok(Some(why)) => {
            eprintln!("makepad devhub inspect: {why}");
            return Some(2);
        }
        Err(e) => return Some(fail(format!("whose {node}/{jit} is: {e}"))),
    }
    let (id, own) = match lease {
        Some(l) => (l.to_string(), false),
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
    // A grab (or another command read from the app's frames) and the app's AI tools are answered
    // on this side, on a client of the app through the hub ([`crate::cli::answered_here`]). The
    // app has no command for either: relayed as one, a grab was refused as "client-side", which
    // left the lanes no grab of a pool node's app, and `call vis.play ..` of a Source Library a
    // lane's screen lease ran was "unknown command `call`" (11-07). A grab's file lands on this
    // machine (`out=`, else the grab folder here).
    let words: Vec<String> = cmd.iter().map(|w| w.to_string()).collect();
    let r = if crate::cli::answered_here(&words) {
        let app = Client::routed(&c, &format!("{node}:{jit}"));
        let mut lines = Vec::new();
        makepad_remote::app::run(&app, &words, &mut |l| {
            lines.push(l.to_string());
            true
        })
        .map(|_| lines)
        .map_err(|e| e.to_string())
    } else {
        ask(&c, None, "linspect", &format!("{id} {node} {jit} {}", cmd.join(" ")))
    };
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

/// Who sets a node's lease.conf, for its event (` by=<DEVHUB_AGENT>`).
fn conf_by() -> String {
    std::env::var("DEVHUB_AGENT").ok().filter(|a| !a.is_empty() && !a.contains(char::is_whitespace)).map_or(String::new(), |a| format!(" by={a}"))
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
                if caps.timed_spare { "spare" } else if caps.timed { "on" } else { "off" },
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

fn heap(c: &Client, node: &str, top: &str) -> i32 {
    // the master's own heap: its control socket, no node
    match ask(c, Some(node).filter(|n| *n != "master"), "heap", &format!("heap {top}")) {
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

/// `landed <lane>` / `clean <lane>`: a lane whose makepad half is in `work` has its live jobs
/// ended, then the master removes it as `lane remove` does: both halves' worktrees and branches
/// and its dirs on every node, refused while anything of it is unlanded, uncommitted, untracked
/// or still running. It removed the makepad half only, and the commercial worktree's record and
/// branch stayed (two lanes' had to be removed by name after, 11-06). A lane with nothing on
/// the Mac has its node dirs cleaned.
fn landed(c: &Client, lane: &str) -> i32 {
    if !makepad_devhub_conf::valid_name(lane) {
        eprintln!("makepad devhub landed <lane>");
        return 2;
    }
    let mut code = 0;
    let branch = format!("rc-{lane}");
    let repo = cwd();
    let exists = git(&repo, &["rev-parse", "--verify", "--quiet", &format!("refs/heads/{branch}")]).is_some();
    if exists && !crate::lease::wt::in_work(&repo, &branch) {
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
    match one(c, "lane", &format!("lane remove {lane}")) {
        Ok(v) => println!("{lane} removed: {}", s(&v, "note")),
        Err(e) if e == format!("no lane {lane}") => {
            println!("{lane}: no worktree of it on the Mac");
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
        }
        Err(e) => {
            println!("{lane} kept: {e}");
            code = 1;
        }
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

/// `lease <id> why`: what a running lease does now, asked on that lease.
fn why(id: &str) -> i32 {
    let c = match master() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    match lane_node(&c, "", Some(id)).and_then(|node| node_one(&c, &node, "lease-why", &format!("lease-why {id}"))) {
        Ok(v) => {
            print!("{}", s(&v, "text"));
            0
        }
        Err(e) => fail(e),
    }
}

/// The lease words; None: not one of them.
pub fn cli(args: &[&str]) -> Option<i32> {
    let word = *args.first()?;
    if word == "replay" { return Some(super::replay::cli(&args[1..])); }
    if word == "inspect" {
        // its options before or after the target (`inspect --lease L <target> <cmd...>`)
        let (target, lease, read_only, at) = crate::cli::inspect_words(&args[1..]);
        return inspect(target?, lease, read_only, &args[1 + at..]);
    }
    if !matches!(word, "prof" | "stats" | "jit" | "overlay" | "lease" | "run" | "test" | "get" | "release" | "cancel" | "queue" | "leases" | "why-queued" | "nodes" | "node" | "disk" | "heap" | "desk" | "landed" | "clean" | "evict" | "unblock" | "override" | "blocked") {
        return None;
    }
    if matches!(args, ["run", _, _, ..] | ["test", _, _, ..]) {
        if let Err(e) = crate::proto::agent() {
            eprintln!("makepad devhub {word}: {e}");
            return Some(2);
        }
    }
    match args {
        ["prof", rest @ ..] => if let Err(e) = super::prof::Query::parse(rest) { eprintln!("makepad devhub {e}"); return Some(2); },
        ["lease", id, "cancel"] | ["cancel", id] => return Some(cancel(id)),
        ["lease", id, "why"] => return Some(why(id)),
        ["lease", rest @ ..] => return Some(lease(rest)),
        ["overlay", "selftest", rest @ ..] => return Some(overlay_selftest(rest)),
        ["overlay", word @ ("status" | "why"), lane, rest @ ..] => return Some(overlay_inspect(word, lane, rest)),
        ["run", lane, kind, rest @ ..] => return Some(run(lane, kind, rest)),
        ["test", lane, pkg, rest @ ..] => return Some(test(lane, pkg, rest)),
        ["test", ..] => { eprintln!("makepad devhub test <lane> <pkg> [--lib|--bin B|--test T|--tests] [--features F] [--needs k=v,..] [--node N [--series S]] [--cost small|medium|large] [--est-min N] [--cores N] [--class name] [--priority interactive|background] [--jit key] [--native [<reason>]|--live] [--tree] [-- args...]"); return Some(2); }
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
        ["stats", word @ ("cores" | "calls")] => match one(&c, "stats", &format!("stats {word}")) {
            Ok(v) => { print!("{}", s(&v, "text")); 0 }
            Err(e) => fail(e),
        },
        ["stats", ..] => { eprintln!("makepad devhub stats queue [--hour] | stats cores | stats calls"); 2 },
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
        // the person hands the node back (main, on their say-so): its input so far holds it no more
        ["node", node, "back"] => match node_one(&c, node, "user-back", "user-back") {
            Ok(v) => {
                println!("{node}: {}", if v.get("was_present").and_then(|b| b.as_bool()) == Some(true) { "handed back: takes leases again (new input holds it again)" } else { "no person held it" });
                0
            }
            Err(e) if e.contains("unknown") || e.contains("unsupported") => fail(format!("{e} ({node}'s hub is older than `node back`: it takes it from its next self-update)")),
            Err(e) => fail(e),
        },
        ["node", node] => node_conf(&c, node, ""),
        ["node", node, "role", role] => node_conf(&c, node, &format!("set role={role}{}", conf_by())),
        ["node", node, "set", kvs @ ..] if !kvs.is_empty() => node_conf(&c, node, &format!("set {}{}", kvs.join(" "), conf_by())),
        ["disk", node, rest @ ..] => disk(&c, node, rest.contains(&"--dry-run")),
        ["heap", node, rest @ ..] => heap(&c, node, rest.first().copied().unwrap_or("12")),
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
        let r: Result<(), String> = through_restarts("L1", &mut c, |_| Err("node-a: lease-log: bad".into()), &up);
        assert!(r.is_err());
        // the lease's end (its beat heard it) stands at once, even while its node does not answer
        let mut n = 0;
        let r: Result<(), String> = through_restarts("L1", &mut c, |_| { n += 1; Err(if n == 1 { "node-a: link lost: no answer to OPEN" } else { "L1 ended (expired)" }.into()) }, &up);
        assert_eq!((r, n), (Err("L1 ended (expired)".to_string()), 2));
        assert!(lost_how("expired") && lost_how("lost") && !lost_how("cancelled") && lost_error("node-a: no lease L1 on a node") && !lost_error("node-a: lease-log: bad"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_lease_run_flag_reaches_the_request_or_is_local() {
        let src = include_str!("client.rs");
        let body = |name: &str| {
            let start = src.find(&format!("\nfn {name}(")).unwrap();
            let function = &src[start..];
            let end = function.find("\n}").unwrap();
            &function[function.find('{').unwrap() + 1..end]
        };
        let flags: Vec<&str> = body("parse").lines().map(str::trim_start)
            .filter(|line| line.starts_with('"'))
            .filter_map(|line| line.split_once("=>"))
            .flat_map(|(arm, _)| arm.split('|').filter_map(|word| word.trim().split('"').nth(1)))
            .filter(|flag| flag.starts_with("--") && *flag != "--").collect();
        assert!(!flags.is_empty(), "parse must contain lease flags");
        let covered: Vec<&str> = LOCAL_FLAGS.iter().map(|(flag, _)| *flag)
            .chain(RESOLVED_FLAGS.iter().map(|(flag, _, _)| *flag))
            .chain(FLAG_FIELDS.iter().map(|(args, _)| args[0])).collect();
        for flag in &flags {
            let count = covered.iter().filter(|entry| *entry == flag).count();
            assert!(count > 0, "`{flag}` is parsed but is in none of LOCAL_FLAGS, RESOLVED_FLAGS, FLAG_FIELDS");
            assert_eq!(count, 1, "`{flag}` must have exactly one table row");
        }
        for flag in covered {
            assert!(flags.contains(&flag), "`{flag}` has a table row but is not parsed");
        }
        for (flag, why) in LOCAL_FLAGS {
            assert!(!why.is_empty(), "`{flag}` needs a reason it stays local");
        }
        for (flag, key, function) in RESOLVED_FLAGS {
            assert!(super::super::sched::REQ_KEYS.contains(key), "`{flag}` names unknown request key `{key}`");
            assert!(body(function).contains(&format!("req.{key} =")), "`{flag}` must set req.{key} in {function}");
        }
        let baseline = parse("build", Some("lane-a"), &[]).unwrap().req.to_json();
        let baseline = makepad_strict_json::parse(baseline.as_bytes()).unwrap();
        for (args, key) in FLAG_FIELDS {
            let flag = args[0];
            assert!(super::super::sched::REQ_KEYS.contains(key), "`{flag}` names unknown request key `{key}`");
            let request = parse("build", Some("lane-a"), args).unwrap().req.to_json();
            let request = makepad_strict_json::parse(request.as_bytes()).unwrap();
            assert_ne!(request.get(key), baseline.get(key), "`{flag}` is parsed but `{key}` in the request does not change");
        }
    }

    /// A lease with no lane from the main checkout would run in tree `makepad` on its box: an
    /// agent's is refused saying to name its lane (a fresh-clone job left Cargo.toml edited there,
    /// 11-12); the person's (no agent), main's and devhub's own go on, as does one whose lane is
    /// named (`DEVHUB_LANE`, a lane's worktree).
    #[test]
    fn an_agents_lease_without_a_lane_from_the_main_checkout_is_refused() {
        let refused = laneless("makepad", false, Some("relaxsite"));
        assert!(refused.as_deref().is_some_and(|w| w.contains("--lane <lane>") && w.contains("`makepad`")), "an agent is told to name its lane: {refused:?}");
        assert_eq!(laneless("makepad", false, None), None, "the person's own lease goes on");
        assert_eq!(laneless("makepad", false, Some("main")), None, "main's goes on");
        assert_eq!(laneless("makepad", false, Some("devhub-bucket")), None, "devhub's own goes on");
        assert_eq!(laneless("relaxsite", true, Some("relaxsite")), None, "one whose lane is named goes on");
    }

    /// `run --patch` flags before the command come out of it and give the patched files; the
    /// command's own `--patch` stays. The staged copy keeps the worktree file's mode.
    #[test]
    fn a_runs_patches_come_out_of_its_flags_and_stage_with_the_files_mode() {
        let dir = std::env::temp_dir().join(format!("devhub-run-patch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let root = dir.join("wt");
        std::fs::create_dir_all(root.join("tools")).unwrap();
        std::fs::write(root.join("tools/rig.sh"), "echo one\necho two\n").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.join("tools/rig.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let diff = dir.join("probe.diff");
        std::fs::write(&diff, "--- a/tools/rig.sh\n+++ b/tools/rig.sh\n@@ -1,2 +1,3 @@\n echo one\n+echo probe\n echo two\n").unwrap();
        let diff = diff.to_string_lossy().into_owned();
        let mut rest = vec!["--patch", diff.as_str(), "--cores", "2", "--", "bash", "--patch", "x"];
        let patched = take_patches(Some((&root, ".")), &mut rest).unwrap();
        assert_eq!(rest, ["--cores", "2", "--", "bash", "--patch", "x"]);
        assert_eq!(patched, [("tools/rig.sh".to_string(), b"echo one\necho probe\necho two\n".to_vec())]);
        let mut twice = vec!["--patch", diff.as_str(), "--patch", diff.as_str(), "--", "x"];
        assert!(take_patches(Some((&root, "tools")), &mut twice).unwrap_err().ends_with("tools/rig.sh is in another patch too"));
        // from inside a nested checkout, its diff's paths are that repository's (as `git diff`
        // wrote them there); a run started beside it, in makepad, reads them as makepad's
        std::fs::create_dir_all(root.join("apps/side-app/.git")).unwrap();
        std::fs::create_dir_all(root.join("apps/side-app/tools")).unwrap();
        std::fs::write(root.join("apps/side-app/tools/rig.sh"), "echo one\necho two\n").unwrap();
        for rel in ["apps/side-app", "apps/side-app/tools"] {
            let mut rest = vec!["--patch", diff.as_str(), "--", "x"];
            assert_eq!(take_patches(Some((&root, rel)), &mut rest).unwrap(), [("apps/side-app/tools/rig.sh".to_string(), b"echo one\necho probe\necho two\n".to_vec())], "{rel}");
        }
        let mut rest = vec!["--patch", diff.as_str(), "--", "x"];
        assert_eq!(take_patches(Some((&root, "apps")), &mut rest).unwrap()[0].0, "tools/rig.sh");
        let stage = dir.join("stage");
        let staged = stage_patched(&root, &stage, &patched).unwrap();
        assert_eq!(staged.iter().map(|e| (e.path.as_str(), e.size)).collect::<Vec<_>>(), [("tools/rig.sh", 29)]);
        assert_eq!(std::fs::read(stage.join("tools/rig.sh")).unwrap(), patched[0].1);
        #[cfg(unix)]
        assert!(staged[0].exec, "the staged script keeps its mode");
        assert!(stage_patched(&root, &stage, &[("local/x.sh".into(), Vec::new())]).unwrap_err().contains("never ships"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `--input NAME=<archive>`: an environment variable's name and a file here; the run's
    /// command takes the placed folders as `env` words, before its own.
    #[test]
    fn an_input_is_a_variable_name_and_a_file_and_its_run_reads_it_from_env() {
        let dir = std::env::temp_dir().join(format!("devhub-input-flag-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ui.tgz");
        std::fs::write(&file, "x").unwrap();
        let spec = format!("UI_CORPUS={}", file.display());
        let o = parse("test", Some("lanea"), &["--input", &spec, "--", "true"]).unwrap();
        assert_eq!(o.inputs, [super::super::ship::Input { name: "UI_CORPUS".into(), path: file.clone() }]);
        for bad in [format!("1UI={}", file.display()), format!("U-I={}", file.display()), "UI".into(), format!("UI={}", dir.join("none.tgz").display())] {
            assert!(parse("test", Some("lanea"), &["--input", &bad, "--", "true"]).is_err(), "{bad}");
        }
        let placed = [super::super::ship::Placed { name: "UI".into(), hash: "h".into(), dir: "/w/inputs/h".into(), sent: 0 }];
        let words = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(super::super::ship::with_inputs(&words(&["python3", "ty.py"]), &placed), words(&["env", "UI=/w/inputs/h", "python3", "ty.py"]));
        assert_eq!(super::super::ship::with_inputs(&words(&["env", "-u", "X", "A=1", "sh"]), &placed), words(&["env", "UI=/w/inputs/h", "-u", "X", "A=1", "sh"]));
        assert_eq!(super::super::ship::with_inputs(&words(&["sh"]), &[]), words(&["sh"]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_cancel_names_its_node_or_says_it_was_still_queued() {
        assert_eq!(cancelled_notice("L7", CancelledAt::Ran("a")), "LEASE CANCELLED by user on a (L7): pick another node (devhub nodes) and retry; don't retry on a unless asked. Exit code 3.");
        assert_eq!(cancelled_notice("L7", CancelledAt::Queued(Some("a"))), "LEASE CANCELLED by user while queued for a (L7): pick another node (devhub nodes) and retry; don't retry on a unless asked. Exit code 3.");
        assert_eq!(cancelled_notice("L7", CancelledAt::Queued(None)), "LEASE CANCELLED by user while queued (L7): don't ask again unless asked. Exit code 3.");
    }

    /// A timed run's release line says whether its box was quiet, by the master's verdict
    /// ([`super::super::master::unquiet`]): beside a desktop left running it was not, and its
    /// timings are not clean; its own load past the box's cores is no verdict; a quiet box's line
    /// says so too.
    #[test]
    fn a_timed_runs_release_line_says_whether_its_box_was_quiet() {
        use super::super::master::unquiet;
        let b = |outside: f64, load: f64| makepad_strict_json::parse(format!(r#"{{"samples":3,"load1_max":{load},"outside_cpu_pct":{outside},"outside_cpu_pct_max":300,"busiest":[["WindowServer",34]]}}"#).as_bytes()).unwrap();
        assert_eq!(unquiet(&b(143.0, 93.5)).as_deref(), Some("143% of a core ran outside it on average"));
        assert_eq!(unquiet(&b(20.0, 40.0)), None, "a timed build's own load past its box's cores");
        assert_eq!(unquiet(&b(20.0, 3.0)), None, "a quiet box");
        let line = |outside: f64, load: f64| {
            let why = unquiet(&b(outside, load)).map_or("null".to_string(), |w| makepad_remote::json_str(&w));
            released_line(&makepad_strict_json::parse(format!(r#"{{"id":"L1","kind":"timed","lane":"x","node":"box-a","how":"released","cleanup":{{"cleanup":"other","node":"box-a","box":{},"unquiet":{why}}}}}"#, b(outside, load).to_json()).as_bytes()).unwrap())
        };
        assert!(line(143.0, 93.5).ends_with("\nbox box-a: NOT QUIET (143% of a core ran outside it on average): its timings are not clean, run it again on a quiet box; load1 max 93.5; outside it 143% of a core (max 300%): WindowServer 34%"), "{}", line(143.0, 93.5));
        assert!(line(20.0, 3.0).ends_with("\nbox box-a: quiet: load1 max 3.0; outside it 20% of a core (max 300%): WindowServer 34%"), "{}", line(20.0, 3.0));
        // a GPU timing is judged by the GPU busy outside it; the CPU outside it is a figure
        let gpu = |cpu: f64, gpu: f64| makepad_strict_json::parse(format!(r#"{{"samples":3,"load1_max":4,"outside_cpu_pct":{cpu},"outside_cpu_pct_max":150,"busiest":[["mds_stores",60]],"gpu":true,"outside_gpu_pct":{gpu},"outside_gpu_pct_max":21,"gpu_busiest":[["helper",{gpu}]]}}"#).as_bytes()).unwrap();
        assert_eq!(unquiet(&gpu(110.0, 3.0)), None, "a Mac's own services at 1.1 cores, its GPU quiet");
        assert_eq!(unquiet(&gpu(20.0, 14.0)).as_deref(), Some("its GPU was 14% busy outside it on average"));
        assert_eq!(super::super::master::box_line(&gpu(110.0, 3.0)), "load1 max 4.0; outside it 110% of a core (max 150%): mds_stores 60%; its GPU busy outside it 3% (max 21%): helper 3%");
        let unmeasured = makepad_strict_json::parse(br#"{"samples":3,"load1_max":4,"outside_cpu_pct":110,"outside_cpu_pct_max":150,"busiest":[],"gpu":true}"#).unwrap();
        assert_eq!(unquiet(&unmeasured), None, "no GPU use told: not judged on its CPU");
    }

    /// `--keep` says what came back, an empty copy too (an empty one said nothing, 11-07).
    #[test]
    fn a_keep_says_what_it_brought_back_or_that_it_was_nothing() {
        let dst = Path::new("grabs");
        assert_eq!(keep_said("L7", dst, &Ok(Vec::new())), "L7: kept nothing in grabs: the run put no file in $DEVHUB_KEEP");
        assert_eq!(keep_said("L7", dst, &Ok(vec![(dst.join("a.png"), 1 << 20), (dst.join("b.png"), 1 << 19)])), "L7: kept 2 files (1.5 MB) in grabs");
        assert_eq!(keep_said("L7", dst, &Err("node-a: no lease L7".into())), "L7: --keep grabs: nothing kept: node-a: no lease L7");
    }

    #[test]
    fn the_timed_queue_reads_per_box_in_expected_order() {
        let leases: Vec<Value> = [
            r#"{"id":"L1","lane":"one","kind":"timed","state":"running","node":"a","granted_s":40}"#,
            r#"{"id":"L3","lane":"three","kind":"timed","state":"queued","eta_box":"a","eta_ms":900000}"#,
            r#"{"id":"L2","lane":"two","kind":"timed","state":"queued","eta_box":"a","eta_ms":120000}"#,
            r#"{"id":"L4","lane":"four","kind":"timed","state":"queued","eta_box":null,"eta_ms":0}"#,
            r#"{"id":"L5","lane":"five","kind":"test","state":"queued","eta_box":null,"eta_ms":0}"#,
        ].iter().filter_map(|l| json(l)).collect();
        assert_eq!(timed_boxes(&leases), [
            "  (no box yet): then L4 four next",
            "  a: L1 one running 40 s; then L2 two in ~2 min, L3 three in ~15 min",
        ]);
    }

    #[test]
    fn a_lane_is_its_makepad_worktree_also_from_inside_its_commercial_checkout() {
        let d = std::env::temp_dir().join(format!("devhub-laneroot-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let inner = d.join("lane/apps/commercial/foo/app");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::create_dir_all(d.join("lane/apps/source-library/src")).unwrap();
        for repo in [d.join("lane"), d.join("lane/apps/commercial"), d.join("lane/apps/source-library")] {
            assert!(git(&repo, &["init", "-q"]).is_some(), "git init {}", repo.display());
        }
        // as git spells it (forward slashes on Windows, where canonicalize says `\\?\`)
        let lane = PathBuf::from(git(&d.join("lane"), &["rev-parse", "--show-toplevel"]).unwrap());
        assert_eq!(lane_root_of(&inner), Some((lane.clone(), "apps/commercial/foo/app".to_string())));
        assert_eq!(lane_root_of(&d.join("lane/apps/commercial")), Some((lane.clone(), "apps/commercial".to_string())));
        assert_eq!(lane_root_of(&d.join("lane/apps/source-library/src")), Some((lane.clone(), "apps/source-library/src".to_string())), "any nested repository");
        assert_eq!(lane_root_of(&d.join("lane/apps")), Some((lane, "apps".to_string())));
        let _ = std::fs::remove_dir_all(&d);
    }
}
