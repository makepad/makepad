//! The lease words of `makepad devhub`: the holder side of a lease (it beats the master every
//! 2 s while it holds one; a holder that dies lets its lease expire), `run` (ship the worktree,
//! run under a lease, stream the output, exit with its code), and the views and switches.
//!
//! Exit codes: 0 ok, 1 the command failed, 2 usage or denied, 3 the user cancelled the lease
//! ([`CANCELLED_EXIT`]), 4 the lease was lost under the command: no verdict ([`LOST_EXIT`]).

use super::master::b64_decode;
use super::sched::{ordinal, Req};
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
pub(crate) fn eta_of(v: &Value) -> String {
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

pub const HELP: &str = "  lease <kind> [--node N [--series S|--pin-why R]|--needs k=v,..] [--lane L] [--prio P] [--rev <branch>[@sha][,<name>=<branch>[@sha]]...]
        [--repo-dir D] [--watch-pid P] [--cost small|medium|large] [--est-min N] [--cores N] [--ram-gb N] [--ram-floor-gb N] [--whole]
        [--class name] [--priority interactive|background] [--jit key] [--native <reason>] [--keep <dir>] [--yields]
        [--untimed] [--input NAME=<archive> ..] [--for <node>] [--exit-to <file>] [--run-since MS] [-- cmd...]
                                          hold a lease (kinds: build test screen long timed gpu inspect gate ci;
                                          ci: its own tree and target, beside ordinary work, never timed or user boxes;
                                          --needs gui=1: a run that opens windows (a test, a timed GUI measurement),
                                          on its box's WM display, only nodes that take screen leases;
                                          gpu=1: a box with a GPU, gpu=<name>: one with that in its name;
                                          gpu on a Mac is a GPU timing, which drains its box: pictures and counters
                                          that read no timing take --untimed, a test sharing its box);
                                          with a cmd: run it there and exit with its code. --node N: N or wait for
                                          it, never another box (refused, saying why, when N can never take it);
                                          --series S says why: a series S compared on N (never asked to unpin);
                                          --keep <dir>: what the cmd put in $DEVHUB_KEEP (a failed step's logs) is
                                          copied into <dir> before the lease ends (never into a checkout: the main
                                          one's local/agent_state/<lane>/runs/ is the place). --ram-floor-gb N: the least RAM its
                                          unasked need is cut to on a box (a tests shard's largest unit). --run-since MS: a
                                          tests bucket shard's run took its tip then (wall-clock ms): by day a full run's shards
                                          hold a box only once that is 2 h ago, at night always. --input NAME=<archive> (lease and
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
                                          it exists, stops after the current step with its partial results and releases.
                                          --yields: the cmd reads $DEVHUB_YIELD between its steps and, once it exists, ends
                                          at its step's end, its holder asking again for the rest; a bench box is lent it
                                          between timed series whatever its length (other runs: 10 min at most by estimate),
                                          and its file appears as timed work comes for the box
  run <lane> <kind> [--node N [--series S|--pin-why R]|--needs ..] [--rev R] [--with-uncommitted] [--tree|--look] [--patch <diff> ..]
        [--cost small|medium|large] [--est-min N] [--cores N] [--ram-gb N] [--whole]
        [--class name] [--priority interactive|background] [--jit key] [--native <reason>] [--keep <dir>]
        [--for <node>] [--rounds N] -- cmd...
                                          --rounds N (timed): the command runs once per round, DEVHUB_ROUND=1..N and
                                          DEVHUB_ROUNDS=N in its environment; when a short timed ask waits on the box
                                          (whatever this run's length) it hands the box over after the round and asks
                                          again for the rest, DEVHUB_RESUMED=1 in the first round after (another run had
                                          the box: redo a warm-up). Name each round's output by its round. A timed run
                                          without it hands its box over only when it ends, and hears so as it asks.
                                          --cores N: the run sees N cores, as each unit of a land gate's tests bucket does:
                                          MAKEPAD_CPU_CORES (its pools and JIT workers), RELAX_JOBS, CARGO_BUILD_JOBS and
                                          RUST_TEST_THREADS are N, unpinned (`devhub test ... --cores 4` runs as a unit of 4).
                                          Its box is charged what its class's runs used, its N until one is measured.
                                          A run's outputs (sheets, grabs, logs): the cmd writes them to $DEVHUB_KEEP and
                                          --keep <dir> copies them into <dir> before the lease ends; a `get` after the
                                          run can find the lane's tree already handed to another lane.
                                          A lease's end stops an app its run `open`s too: no run (re)starts a user's app.
                                          JIT commands of kinds test, screen, gpu, long and build use overlays by default;
                                          --tree uses the work tree. CI, gates, timed and non-JIT commands use trees, but --look runs any command on an overlay, holding no tree: one that touches neither the tree nor build output (`ps`, a log's tail) never waits behind a lane's tree lease. A look asks no cores or RAM until its class measures some, is outside its lane's cap, and passes every hold but exclusive work's.
                                          Main checkout edits need --with-uncommitted. --rev R: those commits
                                          alone (`work@<sha>[,commercial=main@<sha>]`), none of the worktree's files.
                                          --patch <diff>: each diff (`git diff`) applies to the file as the run
                                          ships it (with --rev R: R's), for this lease only (a rig's instrumentation):
                                          never in the worktree or a node's tree, which gets every shipped file back
                                          at release. Its paths are those of the repository the run starts in
                                          (inside a nested apps/<name> checkout, that one's). A run that edits tracked files itself names them
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
  test <lane> <pkg> [--lib|--bin B|--test T|--tests] [--features F] [--no-default-features] [--needs k=v,..] [--node N [--series S|--pin-why R]] [--avoid N[,N..]] [--cost small|medium|large] [--est-min N] [--cores N] [--class name] [--priority interactive|background] [--jit key] [--native [<reason>]|--live [--offline] [--env K=V ..]] [--rev R] [--tree] [-- args...]
                                          --features F, --no-default-features: the package built with those features,
                                          as cargo builds it (the land gate's `--lib --no-default-features` run of
                                          makepad-devhub, which Scope's agents and the phone host build so); with
                                          --live, the feature set is part of the session's key: each set its own session.
                                          --offline needs --live: Linux user/network namespaces, only loopback up;
                                          a namespace failure or JIT miss fails the run, never a networked fallback.
                                          --env K=V (with --live, repeatable): the session's environment, as `live start
                                          --env`; a session asked with other env or another --jit is a session of its own.
                                          --pin-why R (with --node): the reason it names its box when no series
                                          does; devhub then never asks it to unpin.
                                          --avoid N[,N..]: boxes it never takes (a tests run's rerun of a lost
                                          shard's units goes elsewhere).
                                          --rev R: one run at the commits R alone (`<branch>[@sha]` for makepad, then
                                          `,<name>=<branch>[@sha]` per nested apps/<name> repo, as in `work,commercial=main`;
                                          each nested checkout's HEAD where R names none), none of the worktree's uncommitted files.
                                          --live: THE lane check; no selector or --tests on a package of several harness targets
                                          is one JIT run of the whole package on one lease (no session per target, no pre-land checks);
                                          --lib, --bin B or --test T selects one. One warm process per lane, package, target and args,
                                          small cost on a hot box unless --cost/--est-min hint otherwise; overlay by default,
                                          --tree opts out; only affected tests rerun,
                                          a changed file in a package of its graph that is none of its sources starts
                                          them again, so does one in no package the tests opened, probed or listed the
                                          folder of (a data file); any other in no package keeps the last verdict and says so.
                                          With nothing changed since its last round, a call answers that round's verdict
                                          and says it is cached (nothing ran); --fresh runs the tests again, in a warm
                                          restart of the session's run on its box (a flake hunt's rerun).
                                          Once cargo confirms a compile error is the lane's, the session stays held under its lease
                                          and yields its RAM to queued work. `live list` and `leases` show `held: <first error>`,
                                          and the next `test --live` after the fix reattaches and answers with that run's verdict.
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
                                          <local-dir> and --keep's <dir> are refused in a checkout (a lane's worktree or a
                                          repo's): the main checkout's local/agent_state/<lane>/runs/ takes kept files.
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
  lease <id> show | lease show <id>       that lease's line of `queue --all`: its state, box, cost, and why it
                                          waits or how it was granted (per box: `why-queued <id>`)
  lease <id> why                          what a running lease does now, on that lease (a second lease on its tree
                                          waits for it): processes, threads and kernel waits, gdb's stacks where the
                                          box lets it attach, tests running over a minute, the last lines
  lease <id> jits                         its JITs, wherever they run (a JIT-run test leads a process group of its
                                          own), as <node>/<pid> <app>: `devhub inspect <node>/<pid> why-hung`
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
  queue --long                            the leases queued 10 min or more (a check-in's starvation list),
                                          counted as the panel counts them: a lane's leases behind its
                                          own lease in its tree (its serial plan) one line per lane, the
                                          boxes' prebuilds and lanes' warm legs on a line of their own,
                                          never counted
  why-queued <lease>                     each candidate box's start blocker and reservation expiry
  replay take <from> <to> <out-dir>        export replay inputs on the master (UTC YYYY-MM-DDTHH:MM:SSZ)
  replay run <dir> <from> <to>             simulate scheduler waits offline from exported inputs
  nodes                                   capabilities, dev switch and leases per node
  node <n> dev on|off                     the node's dev switch (the user's decision)
  node <n> probe                          its toolchain probe now (a compile and a link): a box a toolchain move left
                                          out of rotation comes back at once (it probes again as a lease there ends too);
                                          and its audio output where it measures it (a silent playback): audio=1 work
                                          goes there again at once once its output runs
  node <n> [role pool|hot|suite|bench|user-wm|device] [set slots.screen=0 disk_floor_gb=40 ...]
                                          the node's role and budgets (its lease.conf, kept on the node);
                                          `key=` unsets a key, so the box's default applies
  disk <node> [--need GB] [--dry-run] [--ended <lane>,..]
                                          the node's disk categories and reclaim, to its floor plus
                                          GB (default: the largest need of a queued ask its disk
                                          keeps off, as that ask's own reclaim aims for). An ended
                                          lane's folder in its local/screencap is reclaimed with the
                                          rotated logs: a lane ends there by its cleanup, or by --ended
                                          (lanes that ended before; their folders stay until a reclaim)
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

/// Exit code of a hand-in's gate run that had not started by its start bound
/// ([`Opts::start_within`]): it went after the landing, to the post-land buckets, unrun. Its
/// lease or job was let go; no one runs it again before the landing.
pub const DEFERRED_EXIT: i32 = 6;

/// How the master ends a lease that infrastructure lost, never a person or the command.
fn lost_how(how: &str) -> bool {
    matches!(how, "expired" | "lost")
}

/// How the master ends a lease for what its command did: its end is the command's verdict, said
/// with its reason, never a lost lease. A lease ended for a panic its run held parked with nobody
/// to decide it read as `LEASE LOST ... the code is not to blame: run it again`, and its agent
/// hunted a hub and master restart that had not touched it (L1382556, 11-18).
fn verdict_how(how: &str) -> bool {
    matches!(how, "hung" | "disk" | "memory" | "panicked" | "stopped")
}

/// The master's answer to a release of a lease that has ended: what a release whose answer a
/// master restart cut hears when [`through_restarts`] asks it again, the restarted master having
/// the lease ended by that release. It stands at once, as the lease's end does: asked again for
/// [`SETTLE_FOR`], a check whose run had ended on its box said nothing for a minute after the
/// reconnect, and its lane started it again by hand (L1717780, 11-24).
fn ended_already(id: &str, e: &str) -> bool {
    e.starts_with(&format!("{id} has already ended"))
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

/// The grant line of lease `id` on `node` as its ask hears it granted, unless the ask said it as
/// the lease started (`last`, its last line, names the lease at a node then) and has no class note
/// to add. A lease set up between two polls went from queued to granted and said no grant: a full
/// run's shard read its running gate as an ask not granted yet and ended it to give its slot up,
/// 32 units in (L1561926, 11-22).
pub(crate) fn grant_line(id: &str, kind: Kind, node: &str, last: &str, note: &str) -> Option<String> {
    let said = last.starts_with(&format!("{id} {}@", kind.as_str()));
    (!said || !note.is_empty()).then(|| format!("{id} {}@{node}: granted{note}", kind.as_str()))
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
/// always stand. A node off the network ([`super::master::host_gone`]) counts no `REATTACH_FOR`:
/// the master's outage rule ([`super::sched::Sched::caps_missed`]) keeps the lease or ends it, and
/// each node call's `f` checks its end first. Said once each way on stderr.
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
        if e.starts_with("no lease") || e.starts_with(&format!("{id} ended (")) || ended_already(id, &e) {
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
                            eprintln!("{id}: its node is off the network ({e}); the master keeps the lease through the outage (up to twice the box's longest recorded outage, or {} s with none on record; a timed lease ends at once) and its run goes on there; re-attaching when it is back", super::sched::HOST_GONE_MS / 1000);
                        } else {
                            eprintln!("{id}: its node does not answer ({e}); its hub restarts? re-attaching when it is back");
                        }
                        gone = "its node's hub";
                        down = Some(Instant::now());
                        back = Some(Instant::now());
                    }
                    (Some(t), _) if node_restarting(&e) => {
                        if t.elapsed() > REATTACH_FOR && !super::master::host_gone(&e) {
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

/// One line of a `live test` call's stream: its end (`Some`: the call's answer), an error, or a
/// line taken in (`None`). What the session says while its call waits for room on its box (a
/// `{"say":..}` line, [`crate::cli::said_line`]) is said here and is no answer: taken for one, a
/// test call that waited for room ended on "the master answered with no session" and exit 1,
/// its tests run on the box with nobody to read them (W6628, 11-13).
fn live_line(l: &str, answer: &mut Option<Value>, out: &mut dyn Write) -> Result<Option<Value>, String> {
    if let Some((said, _)) = crate::cli::said_line(l) {
        if !said.is_empty() {
            let prefix = if said.starts_with("the held session ") { "" } else { "devhub: " };
            let _ = writeln!(out, "{prefix}{said}");
        }
        return Ok(None);
    }
    let v = json(l).ok_or("live: bad answer")?;
    if v.get("end").is_some() { return answer.take().map(Some).ok_or_else(|| "live: no answer".to_string()); }
    if let Some(e) = v.get("err").and_then(Value::as_str) { return Err(e.to_string()); }
    if answer.is_none() { *answer = Some(v); }
    Ok(None)
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
        Some(e) => {
            let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
            Err(builds_said(v.get("master_rev").and_then(Value::as_str), super::caps::cli_build_rev(), &|a, b| crate::gitread::is_ancestor(&repo, a, b), e))
        }
        None => Ok(v),
    }
}

/// A master's refusal, said with both builds when the master runs another one than this CLI was
/// built at: a request one of them lacks then reads as that, not as a usage line. Nothing when
/// either build is unknown (a CLI built by hand) or both are the same.
fn builds_said(master: Option<&str>, cli: Option<&str>, ancestor: &dyn Fn(&str, &str) -> bool, e: &str) -> String {
    let (Some(master), Some(cli)) = (master.filter(|m| !m.is_empty()), cli) else { return e.to_string() };
    if master == cli { return e.to_string(); }
    let short = |rev: &str| rev.get(..10).unwrap_or(rev).to_string();
    let how = if ancestor(master, cli) { "older than" } else { "another build than" };
    format!("the master runs {}, {how} this CLI's {} (the CLI and the master move together at the master's restart: `devhub master who`): {e}", short(master), short(cli))
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

/// This shell's makepad worktree ([`lane_root`]), also from inside a nested repository's
/// checkout, where `git rev-parse --show-toplevel` names the nested repository.
pub(crate) fn makepad_top() -> Option<String> {
    lane_root().map(|(root, _)| root.to_string_lossy().into_owned())
}

/// The commit `branch` names in the makepad worktree that holds `dir` ([`lane_root_of`]): a
/// hand-in's makepad half. Read in the shell's own toplevel, an explicit hand-in (`land rc-x
/// <commercial worktree> rc-x`) from inside the lane's commercial checkout pinned the commercial
/// branch's commit as its makepad one, and the hand-in's gate failed on it ("git merge-base:
/// fatal: Not a valid commit name", 11-15).
#[cfg(feature = "checkout")]
pub(crate) fn makepad_branch_sha(dir: &Path, branch: &str) -> Option<String> {
    let (root, _) = lane_root_of(dir)?;
    git(&root, &["rev-parse", "--verify", &format!("refs/heads/{branch}^{{commit}}")])
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
    /// the cargo route devhub's measured routing granted this `devhub test` run (`stats route`'s
    /// why), which its gate check keeps through a master restart (lease/native.rs `Ask::granted`)
    routed: String,
    /// `run --patch <diff>`: diffs to apply once [`snap`] has selected the tree that ships.
    patches: Vec<Patch>,
    /// `test --rev`: the run's sources are its pinned commits alone; nothing of the worktree ships
    commits_only: bool,
    /// `--input NAME=<archive>`: shipped to the box by hash before the run, unpacked at `$NAME`
    inputs: Vec<super::ship::Input>,
    /// `--for <node>`: the box the build is for; [`for_cpu`] turns it into the lease's
    /// [`Req::cpu_target`]
    for_node: Option<String>,
    /// `--exit-to <file>`: this client's exit, written there as it ends ([`write_exit`])
    exit_to: Option<PathBuf>,
    /// A hand-in's gate run that has not started this long after it asked goes after the landing
    /// unrun ([`DEFERRED_EXIT`], [`gate_start_bound`]); None: it waits for its start.
    start_within: Option<Duration>,
    /// `test --live --env K=V`: the live session's environment (`live start --env`)
    env: Vec<(String, String)>,
    /// `test --live --fresh`: the session runs its tests again with nothing changed (a flake
    /// hunt's rerun), where its last round's verdict would answer
    fresh: bool,
}

/// Flags handled by the client instead of fields in the lease request.
#[cfg(test)]
const LOCAL_FLAGS: &[(&str, &str)] = &[
    ("--watch-pid", "The holder watches the process and ends its lease when it exits."),
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
    (&["--rounds", "3"], "rounds"),
    (&["--jit", "x"], "loader"),
    #[cfg(windows)]
    (&["--keep", r"C:\devhub-flag-keep"], "keep_dir"),
    #[cfg(not(windows))]
    (&["--keep", "/devhub-flag-keep"], "keep_dir"),
    (&["--lane", "lane-b"], "lane"),
    (&["--needs", "os=linux"], "needs"),
    (&["--node", "box-a"], "node"),
    (&["--prio", "7"], "prio"),
    (&["--priority", "interactive"], "priority"),
    (&["--ram-floor-gb", "1"], "ram_floor_gb"),
    (&["--ram-gb", "2"], "ram_gb"),
    (&["--repo-dir", "/x"], "repo_dir"),
    (&["--series", "s1", "--node", "box-a"], "series"),
    (&["--pin-why", "its files are in that home", "--node", "box-a"], "pin_why"),
    (&["--avoid", "box-a,box-b"], "avoid"),
    (&["--run-since", "5"], "run_ms"),
    (&["--whole"], "whole"),
    (&["--yields"], "yields"),
    (&["--untimed"], "untimed"),
    (&["--look"], "look"),
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
            "--look" => { req.look = true; i += 1; continue; }
            "--whole" => { req.whole = true; i += 1; continue; }
            "--yields" => { req.yields = true; i += 1; continue; }
            "--untimed" => { req.untimed = true; i += 1; continue; }
            "--cost" => { req.cost = CostClass::parse(val(i)?).ok_or("--cost is small, medium or large")?; req.cost_asked = true; }
            "--est-min" => req.est_min = Some(val(i)?.parse::<f64>().ok().filter(|n| n.is_finite() && *n >= 0.0).ok_or("--est-min is a nonnegative number")?),
            "--rounds" => req.rounds = Some(val(i)?.parse::<u32>().ok().filter(|n| *n >= 1).ok_or("--rounds is a positive whole number")?),
            "--cores" => req.cores = Some(val(i)?.parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0).ok_or("--cores is a positive number")?),
            "--ram-gb" => req.ram_gb = Some(val(i)?.parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0).ok_or("--ram-gb is a positive number")?),
            "--ram-floor-gb" => req.ram_floor_gb = Some(val(i)?.parse::<f64>().ok().filter(|n| n.is_finite() && *n > 0.0).ok_or("--ram-floor-gb is a positive number")?),
            "--class" => req.class = val(i)?.to_string(),
            "--node" => super::sched::one_node(&mut req.node, val(i)?)?,
            "--series" => req.series = Some(val(i)?.to_string()),
            "--pin-why" => req.pin_why = Some(val(i)?.to_string()),
            "--run-since" => req.run_ms = Some(val(i)?.parse().map_err(|_| "--run-since takes a wall-clock time in ms")?),
            "--avoid" => super::sched::add_avoid(&mut req.avoid, val(i)?),
            "--needs" => super::sched::add_needs(&mut req.needs, val(i)?)?,
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
    if let Some(why) = reads_stdin(&cmd) { return Err(why); }
    let root = lane_root().map(|(root, _)| root);
    if req.lane.is_empty() {
        req.lane = default_lane();
        let named = std::env::var("DEVHUB_LANE").is_ok_and(|l| !l.is_empty());
        let in_lane = root.as_deref().is_some_and(|root| !super::ship::is_main_checkout(root));
        if let Some(why) = laneless(&req.lane, named || in_lane, std::env::var("DEVHUB_AGENT").ok().as_deref()) {
            return Err(why);
        }
    }
    if let Some(k) = &keep {
        let cwd = cwd();
        if k.as_os_str().is_empty() { return Err("--keep needs a directory".into()); }
        refuse_symlink(&cwd.join(k))?;
        if let Some(why) = kept_into_checkout(k, &cwd, &req.lane) { return Err(format!("--keep: {why}")); }
        let abs = abs_keep_path(k, &cwd);
        let path = abs.to_str().filter(|p| !p.chars().any(char::is_control)).ok_or("--keep needs a UTF-8 path without control characters")?;
        req.keep_dir = Some(path.to_string());
        keep = Some(abs);
    }
    // the lane's repo: revisions are looked up and recovered work lands there
    if req.repo_dir.is_none() {
        req.repo_dir = root.map(|root| root.to_string_lossy().into_owned());
    }
    // branch names become commits here, once: what ships is what was ordered, however long the
    // lease queues and whatever other landings do to the branches meanwhile. A rev named runs those
    // commits alone, as `devhub test --rev` and a job's `--rev` do: none of the worktree's files
    // on top. Shipped the worktree's differences from the pin, three runs a lane pinned at the tip,
    // before and after a rename ran its own tree, and all three the same JIT (11-13).
    let commits_only = req.rev.is_some();
    if let Some(spec) = &req.rev {
        let dir = req.repo_dir.as_deref().ok_or("--rev needs a git repo (--repo-dir)")?;
        req.rev = Some(pin_lane_rev(Path::new(dir), spec, &req.lane, Some(&master_paired), Some(&master_beside))?);
    }
    if req.class.is_empty() && !cmd.is_empty() { req.class = super::run_class(&req.lane, kind, &cmd); }
    req.exec_profile = super::argv::ExecProfile::of(&cmd);
    // its cargo's `--target`s: only a box holding their std takes it ([`super::argv::rust_targets`])
    req.targets = super::argv::rust_targets(&cmd);
    // a run that changes the box's toolchain has the box to itself ([`super::argv::changes_toolchain`])
    if let Some(what) = super::argv::changes_toolchain(&cmd).filter(|_| !req.whole) {
        eprintln!("makepad devhub: `{what}` changes its box's toolchain: the lease takes the box whole");
        req.whole = true;
    }
    if req.look && tree {
        return Err("--look holds no tree: drop --tree".into());
    }
    if req.untimed && kind == Kind::Timed {
        return Err("--untimed with a timed lease: a timed lease times; GPU work that reads no timing is `gpu --untimed`".into());
    }
    Ok(Opts { req, watch, keep, cmd, tree, native, routed: String::new(), patches: Vec::new(), commits_only, inputs, for_node, exit_to, start_within: None, env: Vec::new(), fresh: false })
}

/// Why a run's command would read its script from stdin, which a lease's run never gets (it runs
/// on its box with an empty stdin): `/dev/stdin` named, or a shell given no script (`-s`, or
/// options alone). `devhub run ... -- bash /dev/stdin < script` ran nothing and said nothing
/// (a tier rig, 11-21).
fn reads_stdin(cmd: &[String]) -> Option<String> {
    let fix = "ship the script as a file (in the lane's tree, or `--input NAME=<archive>`) and run it by its path";
    if let Some(a) = cmd.iter().find(|a| matches!(a.as_str(), "/dev/stdin" | "/dev/fd/0" | "/proc/self/fd/0")) {
        return Some(format!("the command reads `{a}`, and a lease's run gets no stdin from here: {fix}"));
    }
    let shell = cmd.first().map(|p| p.rsplit('/').next().unwrap_or(p))?;
    if !matches!(shell, "sh" | "bash" | "zsh" | "dash" | "ksh") { return None; }
    let rest = &cmd[1..];
    let opts = |a: &&String| a.starts_with('-') && !a.starts_with("--") && a.len() > 1;
    let from_stdin = rest.iter().filter(opts).any(|a| a.contains('s') && !a.contains('c')) || rest.iter().all(|a| a.starts_with('-'));
    from_stdin.then(|| format!("`{}` with no script reads it from stdin, and a lease's run gets no stdin from here: {fix}", cmd.join(" ")))
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
    /// why its run has no verdict ([`LOST_EXIT`]): the lease was lost under it, or the toolchain
    /// changed under it; [`finish`] says it, and a test's verdict line carries it
    lost: std::sync::Mutex<Option<String>>,
    /// its run's own exit, once [`finish`] has it (not a cancel, a lost lease or a hand-over)
    exit: std::sync::Mutex<Option<i32>>,
    /// why its run's command never started, as its wrapper said ([`crate::os::never_started_said`])
    never_started: std::sync::Mutex<Option<String>>,
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

fn beat_word(v: &Value, told: &mut String) -> Option<(String, bool)> {
    let word = v.get("idle").and_then(Value::as_str).unwrap_or("");
    if word == told { return None; }
    told.clear();
    told.push_str(word);
    (!word.is_empty()).then(|| (word.to_string(), v.get("hung").and_then(Value::as_bool) == Some(true)))
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
            let mut told = String::new();
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
                        if let Some((word, hung)) = beat_word(&v, &mut told) {
                            eprintln!("{word}");
                            if hung { *verdict.lock().unwrap() = Some(word); }
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
            Some((how, why)) if !why.is_empty() && verdict_how(&how) => Err(format!("{id} ended ({how}): {why}")),
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

/// Whether a job still waiting for its lease says something new of that wait (its note, the
/// lease's queue line), kept in `told`: its waiter says each one as it comes, as a lease's own
/// client does ([`acquire_said`]).
pub(crate) fn waits_said(j: &crate::state::Job, told: &mut String) -> bool {
    let waits = matches!(j.state.as_str(), "queued" | "leasing") && !j.note.is_empty() && j.note != *told;
    if waits { told.clone_from(&j.note); }
    waits
}

/// Asks for the lease and waits for it (one stderr line per change); Err(exit code).
fn acquire(c: &Client, req: &Req) -> Result<Held, i32> {
    acquire_said(c, req).map_err(|(code, _)| code)
}

/// [`acquire`], and when the lease ends before its run, what it said of that on stderr: a check
/// whose lease never started is "not run", naming why, never read as the check's red
/// ("check of relax-compiler .. FAILED: no error line": its lease was refused, 11-13).
fn acquire_said(c: &Client, req: &Req) -> Result<Held, (i32, String)> {
    acquire_within(c, req, None)
}

/// [`acquire_said`], letting the ask go once it has queued `within` without a grant: its lease is
/// released and the error is [`DEFERRED_EXIT`] with its not-run reason.
fn acquire_within(c: &Client, req: &Req, within: Option<Duration>) -> Result<Held, (i32, String)> {
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
        (2, format!("the lease was not taken: {e}"))
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
            // not ranked yet: no "0th in queue ()"
            "queued" if n(&v, "pos") < 1.0 => format!("{id} {}: {}", req.kind.as_str(), unranked(&v)),
            "queued" => format!("{id} {}: {} in queue ({}){}", req.kind.as_str(), ordinal(n(&v, "pos") as usize), s(&v, "reason"), eta_of(&v)),
            "starting" => format!("{id} {}@{}: granted, setting up", req.kind.as_str(), s(&v, "node")),
            "granted" => {
                if let Some(said) = grant_line(&id, req.kind, &s(&v, "node"), &last, &note) { eprintln!("{said}"); }
                // a healthy log and `inspect` both read the same with no window on the screen
                if req.kind == Kind::Screen { eprintln!("{id} screen@{}: its windows are hidden from the person, as every lease's are (MAKEPAD_HIDE_WINDOWS=1): not on the screen unless the command takes that off. A person's launch is `devhub show <app> --on <node>` (`--native`: compiled)", s(&v, "node")); }
                let deployment = s(&v, "deployment");
                if req.kind == Kind::Timed && !deployment.is_empty() {
                    let short = deployment.split(',').map(|sha| &sha[..sha.len().min(10)]).collect::<Vec<_>>().join(",");
                    eprintln!("{id} timed@{}: runs on deployment {short}", s(&v, "node"));
                }
                if req.kind == Kind::Timed && req.rounds.is_some_and(|n| n > 1) {
                    eprintln!("{id}: a run in rounds: when a short timed ask waits on its box, it hands the box over after the current round and asks again for the rest");
                } else if req.kind == Kind::Timed {
                    eprintln!("{id}: between steps, check $DEVHUB_YIELD: it appears when this run is past twice its estimate while a timed ask waits on its box, or runs over 10 min while one of 10 min or less waits there; then stop after the current step with what you have and release");
                }
                let start = v.get("start");
                let env: Vec<(String, String)> = match start.and_then(|st| st.get("env")) {
                    Some(Value::Obj(pairs)) => pairs.iter().map(|(k, x)| (k.clone(), x.as_str().unwrap_or("").to_string())).collect(),
                    _ => Vec::new(),
                };
                eprintln!("{}", budget_said(&id, req.kind, &env));
                if !s(&v, "reason").is_empty() { eprintln!("{id}: {}", s(&v, "reason")); }
                let beat = Beat::start(&id);
                return Ok(Held { id, node: s(&v, "node"), work: start.map(|st| s(st, "work")).unwrap_or_default(), env, beat, adopt: None, handed: Default::default(), lost: Default::default(), exit: Default::default(), never_started: Default::default() });
            }
            _ => {
                let how = s(&v, "how");
                let why = s(&v, "reason");
                if how == "cancelled" {
                    let node = s(&v, "node");
                    let at = if node.is_empty() { CancelledAt::Queued(req.node.as_deref()) } else { CancelledAt::Ran(&node) };
                    let said = cancelled_notice(&id, at);
                    eprintln!("{said}");
                    return Err((CANCELLED_EXIT, said));
                }
                let why = if why.is_empty() { String::new() } else { format!(": {why}") };
                eprintln!("{id} {}: {how}{why}", req.kind.as_str());
                return Err((if how == "denied" { 2 } else { 1 }, format!("lease {id} {how}{why}")));
            }
        };
        if line != last || !note.is_empty() {
            eprintln!("{line}{note}");
            last = line;
            class_notified |= !note.is_empty();
        }
        if let Some(within) = within.filter(|w| state == "queued" && t0.elapsed() >= *w) {
            let said = deferred_said(&id, t0.elapsed(), within);
            let _ = release(&client, &id, None);
            eprintln!("{said}");
            return Err((DEFERRED_EXIT, said));
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
                    return Err((1, format!("lease {id}: asked again after the master's restart: {e}")));
                }
            },
            Err(e) => {
                eprintln!("makepad devhub: beat {id}: {e} (after {} s)", t0.elapsed().as_secs());
                return Err((1, format!("lease {id}: beat: {e}")));
            }
        }
    }
}

/// The CPU budget a lease hands its job, at its grant (exec.rs job_env, as the lease's environment
/// holds it): MAKEPAD_CPU_CORES, which the job's JIT workers and pools size from, and RELAX_JOBS,
/// or the box's budget when it hands neither (a live session). A budget of one or two cores admits
/// one job at once (cpu_budget: its limit is the cores less one), so a timing a timed or screen run
/// takes on it is a single-core figure: Fab's cold start measured on such screen leases took
/// 49.5 s where a live session's took 13.7 s (11-20).
fn budget_said(id: &str, kind: Kind, env: &[(String, String)]) -> String {
    let get = |key: &str| env.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str());
    match (get("MAKEPAD_CPU_CORES"), get("RELAX_JOBS")) {
        (None, None) => format!("{id}: CPU budget: box budget (no MAKEPAD_CPU_CORES or RELAX_JOBS handed to the job)"),
        (cores, jobs) => {
            let one = matches!(kind, Kind::Timed | Kind::Screen) && cores.and_then(|c| c.parse::<u32>().ok()).is_some_and(|c| c <= 2);
            format!("{id}: CPU budget handed to the job: MAKEPAD_CPU_CORES={} RELAX_JOBS={}{}", cores.unwrap_or("unset"), jobs.unwrap_or("unset"),
                if one { " (one core: timings are single-core)" } else { "" })
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
    let ask = || {
        through_restarts(&held.id, c, |c| {
            held.beat.check(&held.id)?;
            node_one(c, &held.node, service, line)
        }, &master)
    };
    poll_start(&held.id, &held.node, ask, std::thread::sleep)
}

fn poll_start(id: &str, node: &str, mut ask: impl FnMut() -> Result<Value, String>, mut sleep: impl FnMut(Duration)) -> Result<Value, String> {
    let mut last = String::new();
    let mut wait = Duration::ZERO;
    loop {
        let v = ask()?;
        let Some(pending) = v.get("pending").and_then(Value::as_str) else { return Ok(v) };
        if pending != last {
            eprintln!("{id} {node}: {pending}");
            last = pending.to_string();
            wait = Duration::ZERO;
        }
        wait = super::pending_poll_delay(wait);
        sleep(wait);
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
        // its first output says whether the command started at all
        if off < 4096 && !data.is_empty() {
            if let Some(why) = crate::os::never_started_said(&String::from_utf8_lossy(&data)) {
                held.never_started.lock().unwrap().get_or_insert(why);
            }
        }
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
/// Err(2) when the master refuses the native build. Its answer also says whether a Windows box
/// takes the command ([`super::sched::Req::windows`]).
fn native_gate(c: &Client, o: &mut Opts) -> Result<(), i32> {
    if o.cmd.is_empty() { return Ok(()); }
    let (top, rel) = lane_root().unwrap_or_else(|| (PathBuf::new(), ".".into()));
    let root = o.req.repo_dir.clone().unwrap_or_else(|| top.to_string_lossy().into_owned());
    let j = makepad_remote::json_str;
    let needs = o.req.needs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",");
    let body = format!("{{\"lane\":{},\"agent\":{},\"kind\":{},\"cmd\":[{}],\"repo_dir\":{},\"cwd\":{},\"rev\":{},\"reason\":{},\"class\":{},\"needs\":{},\"node\":{},\"route\":{}}}",
        j(&o.req.lane), j(&std::env::var("DEVHUB_AGENT").unwrap_or_default()), j(o.req.kind.as_str()), o.cmd.iter().map(|a| j(a)).collect::<Vec<_>>().join(","),
        j(&root), j(rel.trim_end_matches('/')), j(o.req.rev.as_deref().unwrap_or("")), j(&o.native), j(&o.req.class), j(&needs), j(o.req.node.as_deref().unwrap_or("")), j(&o.routed));
    match retry_master(|| one(c, "native", &format!("native check {body}"))) {
        Ok(v) => {
            let note = s(&v, "note");
            if !note.is_empty() { eprintln!("{note}"); }
            o.req.windows = v.get("windows").and_then(|w| w.as_bool()).unwrap_or(false);
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
        Ok(o) if o.req.rounds.is_some() => {
            eprintln!("makepad devhub lease: --rounds is `devhub run`'s: its client runs the command once per round");
            return 2;
        }
        Ok(o) => o,
        Err(e) => {
            eprintln!("makepad devhub lease: {e}");
            return 2;
        }
    };
    let (code, lease) = lease_with(&mut o);
    write_exit(o.exit_to.as_deref(), code, lease.as_deref())
}

/// What a watcher that is not this client's parent knows of its end: `--exit-to`'s file, `<pid>
/// <exit>`, written whole as the client ends. A bucket run's watcher is the master that started it;
/// a master that started again (a crash, a switch-over: not an exec) adopts the run by its pid and
/// cannot reap it, and a run that had ended before it adopted it is no process at all. Both read
/// as "no exit status" and the run had no verdict (the tests bucket at 16:51Z, 11-08). One that
/// ends by a signal writes nothing: its end is not known.
fn write_exit(to: Option<&Path>, code: i32, lease: Option<&str>) -> i32 {
    if let Some(path) = to {
        let temp = path.with_extension("exit.tmp");
        let suffix = lease.filter(|id| !id.is_empty()).map_or(String::new(), |id| format!(" {id}"));
        if let Err(e) = std::fs::write(&temp, format!("{} {code}{suffix}\n", std::process::id())).and_then(|()| std::fs::rename(&temp, path)) {
            eprintln!("makepad devhub: --exit-to {}: {e}", path.display());
        }
    }
    code
}

/// The exit `--exit-to` wrote for the client with `pid` ([`write_exit`]); None: none, or another
/// client's. Read by the bucket's adopted runs (the checkout's).
#[cfg(feature = "checkout")]
pub(crate) fn read_exit(path: &Path, pid: u32) -> Option<i32> {
    read_exit_lease(path, pid).map(|(code, _)| code)
}

#[cfg(feature = "checkout")]
pub(crate) fn read_exit_lease(path: &Path, pid: u32) -> Option<(i32, Option<String>)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut fields = text.split_whitespace();
    if fields.next()?.parse::<u32>().ok()? != pid { return None; }
    let code = fields.next()?.parse().ok()?;
    let lease = fields.next().filter(|id| id.strip_prefix('L').is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())));
    Some((code, lease.map(str::to_owned)))
}

fn lease_with(o: &mut Opts) -> (i32, Option<String>) {
    let c = match master() {
        Ok(c) => c,
        Err(e) => return (fail(e), None),
    };
    if let Err(e) = for_cpu(&c, o) {
        eprintln!("makepad devhub lease: {e}");
        return (2, None);
    }
    if let Err(code) = native_gate(&c, o) { return (code, None); }
    let mut held = match acquire(&c, &o.req) {
        Ok(h) => h,
        Err(code) => return (code, None),
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
        let holder_kept = r.is_ok() && o.keep.as_deref().is_some_and(|dst| {
            let got = kept(&c, &held.node, &o.req.lane, &held.id, dst);
            eprintln!("{}", keep_said(&held.id, dst, &got));
            got.is_ok()
        });
        let code = finish(&c, &held, r, o.keep.as_deref(), holder_kept);
        if let Some(line) = run_failed_said(&held.id, &held.node, *held.exit.lock().unwrap(), held.never_started.lock().unwrap().as_deref()) { eprintln!("{line}"); }
        // the verdict goes last: a `| tail -1` reader sees it
        if let Some(said) = held.handed.lock().unwrap().as_deref() { println!("{said}"); }
        return (code, Some(held.id));
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
            if o.keep.is_some() { wait_keep_said(&c, &held.id); }
            eprintln!("{}: watched pid ended; released", held.id);
            return (0, Some(held.id));
        }
        let ended = held.beat.ended();
        if ended.is_some() && o.keep.is_some() { wait_keep_said(&c, &held.id); }
        match ended {
            Some((how, _)) if how == "cancelled" => {
                eprintln!("{}", cancelled_notice(&held.id, CancelledAt::Ran(&held.node)));
                return (CANCELLED_EXIT, Some(held.id));
            }
            Some((how, why)) if how == "lost" && why.starts_with("no lease") => return (fail(format!("beat {}: {why}", held.id)), Some(held.id)),
            Some((how, why)) => {
                eprintln!("{}: ended ({how}){}", held.id, if why.is_empty() { String::new() } else { format!(": {why}") });
                return (0, Some(held.id));
            }
            None => {}
        }
    }
}

fn wait_keep_said(c: &Client, id: &str) {
    if let Ok(v) = one(c, "beat", &format!("{id} wait-keep")) {
        if let Some(line) = v.get("keep").and_then(Value::as_str).filter(|s| !s.is_empty()) { eprintln!("{line}"); }
    }
}

fn finish_ended(id: &str, node: &str, end: Option<&(String, String)>) -> Option<(i32, String)> {
    let (how, why) = end?;
    if how == "cancelled" {
        return Some((CANCELLED_EXIT, cancelled_notice(id, CancelledAt::Ran(node))));
    }
    if verdict_how(how) {
        let tail = if why.is_empty() { String::new() } else { format!(": {why}") };
        return Some((1, format!("makepad devhub: {id} on {node} ended ({how}){tail}")));
    }
    None
}

fn finish(c: &Client, held: &Held, r: Result<i32, String>, keep: Option<&Path>, holder_kept: bool) -> i32 {
    // its run went on as a job: the lease is the job's, not this client's to release
    if held.handed.lock().unwrap().is_some() { return HUNG_EXIT; }
    let end = one(c, "beat", &held.id).ok().filter(|v| s(v, "state") == "ended")
        .map(|v| (s(&v, "how"), s(&v, "reason")))
        .or_else(|| held.beat.ended());
    if let Some((code, line)) = finish_ended(&held.id, &held.node, end.as_ref()) {
        if keep.is_some() && !holder_kept { wait_keep_said(c, &held.id); }
        eprintln!("{line}");
        return code;
    }
    let ended = end.as_ref().map(|(how, _)| how.as_str());
    let (exit, code) = match r {
        Ok(code) => (Some(code), code),
        // the lease went under the command (not a person, not the command): no verdict at all
        Err(e) if ended.is_some_and(lost_how) || lost_error(&e) => {
            eprintln!("LEASE LOST on {} ({}): {e}; the command has no verdict, the code is not to blame: run it again. Exit code {LOST_EXIT}.", held.node, held.id);
            *held.lost.lock().unwrap() = Some(format!("its lease {} was lost on {}: {e}; no verdict, run it again", held.id, held.node));
            (None, LOST_EXIT)
        }
        Err(e) => (None, fail(e)),
    };
    *held.exit.lock().unwrap() = exit;
    let mut c = c.clone();
    let line = match exit {
        Some(e) => format!("{} released{} exit={e}", held.id, if holder_kept { " kept=1" } else { "" }),
        None => held.id.clone(),
    };
    match through_restarts(&held.id, &mut c, |c| one(c, "release", &line), &master) {
        Ok(v) => {
            eprintln!("{}", released_line(&v));
            if let Some(dst) = keep.filter(|_| !holder_kept) {
                if let Some(cleanup) = v.get("cleanup") {
                    if let Some(line) = cleanup.get("kept").and_then(Value::as_str).filter(|s| !s.is_empty()) {
                        eprintln!("{line}");
                    } else if cleanup.get("cleanup").and_then(Value::as_str) == Some("still running") {
                        eprintln!("{}: $DEVHUB_KEEP still landing in {} (cleanup still running)", held.id, dst.display());
                    }
                }
            }
            // the toolchain changed under the run (a rustup update beside it): a failure is no verdict
            if let Some(t) = v.get("cleanup").and_then(|c| c.get("toolchain")).and_then(Value::as_str).filter(|_| code != 0) {
                eprintln!("TOOLCHAIN CHANGED under {} on {} ({t}): toolchain changed under you: rerun. The command has no verdict, the code is not to blame. Exit code {LOST_EXIT}.", held.id, held.node);
                *held.lost.lock().unwrap() = Some(format!("the toolchain changed under {} on {} ({t}); no verdict, run it again", held.id, held.node));
                return LOST_EXIT;
            }
        }
        // the run's end is in hand: the release only frees the box, and how it went is said
        Err(e) if ended_already(&held.id, &e) => {
            if keep.is_some() && !holder_kept { wait_keep_said(&c, &held.id); }
            eprintln!("{} released: the master restarted while it answered the release, and the restarted master has it ended ({e})", held.id);
        }
        Err(e) => eprintln!("{}: its release was not answered ({e}); the master ends the lease once its beats stop", held.id),
    }
    code
}

/// The last line of a `devhub run` whose command failed (`exit`, its own exit): that exit, and why
/// it never started when its wrapper said so. A run's output that ended with its release line read
/// as clean: four Windows checks whose `sh` was not on the box were read as 0 warnings (11-27).
/// None: it passed, or it has no exit of its own (cancelled, lost, handed over: each says so).
pub(super) fn run_failed_said(id: &str, node: &str, exit: Option<i32>, never_started: Option<&str>) -> Option<String> {
    let code = exit.filter(|c| *c != 0)?;
    Some(match never_started {
        Some(why) => format!("{id} on {node}: FAILED, exit {code}: the command never started: {why}"),
        None => format!("{id} on {node}: FAILED, exit {code}"),
    })
}

/// `L3 screen@node-a (lane x) released; node-a cleaned up: 2 processes stopped, 120 MB freed`.
fn released_line(v: &Value) -> String {
    let node = s(v, "node");
    let at = if node.is_empty() { String::new() } else { format!("@{node}") };
    let head = if v.get("kind").is_none() {
        format!("{} ended (outside recent lease history)", s(v, "id"))
    } else {
        format!("{} {}{at} (lane {}) {}", s(v, "id"), s(v, "kind"), s(v, "lane"), match s(v, "how").as_str() {
            "released" | "" => "released".to_string(),
            "cancelled" => { let why = s(v, "reason"); if why.is_empty() { "cancelled".to_string() } else { why } }
            how => format!("ended ({how})"),
        })
    };
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
    pin_lane_rev(root, &head_side(root, lane)?, lane, None, None)
}

/// A `--rev` given for a lane worktree, pinned whole: each nested repository checked out in it
/// ([`crate::nested`]) that the rev does not name is pinned at its HEAD, so its packages are in
/// the lease's tree (a makepad-only pin left them out of the overlay: `package .. is not a member
/// of the workspace`). With `paired` (a rev someone named: the master's `job landed-main`), the
/// commercial side it does not name pins where the makepad commit landed beside, never at the
/// worktree's HEAD alone: on work, the main that commit landed beside; else the HEAD, unless the
/// master says another main (the HEAD holds a two-repo landing the commit lacks). An older makepad
/// commit beside a lane's newer main is a pair that never landed together: a rig branch on an older
/// base died at cargo's "no matching package named `makepad-relax`" (11-12). Each followed nested
/// repository the worktree lacks is pinned at its landed tip ([`crate::nested::absent`]); the
/// master moves it to where it stood beside the pin's own halves ([`crate::land::beside_pin`]).
pub fn pin_lane_rev(root: &Path, spec: &str, lane: &str, paired: Option<&dyn Fn(&str, Option<&str>) -> Result<String, String>>, beside: Option<&dyn Fn(&str, &str, &str) -> Result<String, String>>) -> Result<String, String> {
    let named: Vec<&str> = spec.split(',').skip(1).filter_map(|side| side.split_once('=').map(|(name, _)| name)).collect();
    let first = spec.split(',').next().unwrap_or("");
    let mut whole = spec.to_string();
    // the rev as given pins first: a malformed one is refused before the master is asked
    let makepad = || super::pin_revs(root, spec).map(|pinned| pinned.split(',').next().and_then(|side| side.rsplit('@').next()).unwrap_or("").to_string());
    for name in crate::nested::names(root).into_iter().filter(|n| !named.contains(&n.as_str())) {
        let head = || head_side(&root.join(crate::nested::dir(&name)), lane);
        let side = match (paired, beside) {
            (Some(paired), _) if name == crate::state::PAIRED => {
                let makepad = makepad()?;
                let makepad = makepad.as_str();
                if first.split('@').next() == Some("work") {
                    format!("main@{}", paired(makepad, None)?)
                } else {
                    let head = head()?;
                    let main = paired(makepad, head.rsplit('@').next())?;
                    if head.ends_with(&format!("@{main}")) { head } else { format!("main@{main}") }
                }
            }
            // each other nested half stands where it stood beside the makepad commit when the two
            // split a landing in both: a run on a torn pair is made whole or refused, never built
            // (a lane's old makepad/apps half beside a newer makepad: two E0277 in Synth, 11-27)
            (_, Some(beside)) if name != crate::state::PAIRED => {
                let head = head()?;
                let (branch, sha) = head.rsplit_once('@').unwrap_or(("", head.as_str()));
                let at = beside(&name, &makepad()?, sha)?;
                if at == sha { head } else {
                    let tip = crate::nested::tips(root).into_iter().find(|(n, _, _)| *n == name).map_or_else(|| branch.to_string(), |(_, b, _)| b);
                    format!("{tip}@{at}")
                }
            }
            _ => head()?,
        };
        whole.push_str(&format!(",{name}={side}"));
    }
    // and each followed one the worktree lacks, at its landed tip, as the buckets build it: a lane
    // had no Source Library, so it could not test what its change broke there (#7425, 11-19)
    let absent: String = crate::nested::absent(root).into_iter().filter(|(n, _, _)| !named.contains(&n.as_str())).map(|(n, branch, sha)| format!(",{n}={branch}@{sha}")).collect();
    Ok(super::pin_revs(root, &whole)? + &absent)
}

/// The master's [`pin_lane_rev`] pairing (`job landed-main`), its line said when it moved the
/// commercial side off the HEAD.
fn master_paired(makepad: &str, head: Option<&str>) -> Result<String, String> {
    let line = format!("job landed-main {makepad} {}", head.unwrap_or(""));
    let v = crate::cli::retry_master(|| master().and_then(|c| one(&c, "job", line.trim_end())))?;
    let note = s(&v, "note");
    if !note.is_empty() {
        eprintln!("devhub: {note}");
    }
    Ok(s(&v, "main"))
}

/// The master's [`pin_lane_rev`] pairing of a nested half besides the paired one (`job landed-side`),
/// its line said when it moved the half off the HEAD.
fn master_beside(name: &str, makepad: &str, head: &str) -> Result<String, String> {
    let line = format!("job landed-side {name} {makepad} {head}");
    let v = crate::cli::retry_master(|| master().and_then(|c| one(&c, "job", &line)))?;
    let note = s(&v, "note");
    if !note.is_empty() {
        eprintln!("devhub: {note}");
    }
    Ok(s(&v, "side"))
}

/// What a run ships on top of its pinned commits, read when it is asked and staged: a queued ask
/// runs what was ordered, never the worktree as its grant finds it. The merge bot merged work into
/// a lane while two `--patch` runs waited, and each shipped the patched files of the HEAD it was
/// asked at beside the merged ones: a call with 3 arguments to a function that took 4 there, a
/// compile error on both arms (L924557/8, 11-13). Hand commits while it waits are the same.
/// The stage goes with it.
struct Snap {
    stage: PathBuf,
    entries: Vec<super::ship::Entry>,
    deleted: Vec<String>,
    patched: usize,
}

impl Drop for Snap {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.stage);
    }
}

/// `devhub-snap-<pid>-<n>` in the temp dir: one per run a client asks (a client's checks ask
/// several at once).
const SNAP_PREFIX: &str = "devhub-snap-";

/// The paths a run names in `DEVHUB_ALSO`, which ship with it.
fn also_named() -> Vec<String> {
    std::env::var("DEVHUB_ALSO").unwrap_or_default().split_whitespace().map(str::to_string).collect()
}

/// The complete uncommitted set (`--with-uncommitted` in the main checkout, none with `--rev`),
/// the `also` paths (DEVHUB_ALSO's; with `--rev` only files the revision does not track:
/// [`super::ship::also_with_rev`]) and the patched files, staged now with their modes and mtimes.
/// A stage a client that is gone left behind is removed first.
fn snap(root: &Path, rev: &str, uncommitted: bool, commits_only: bool, also: &[String], patches: &[Patch]) -> Result<Snap, String> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let pin = &super::Pin::of_rev(rev).ok_or_else(|| format!("rev `{rev}` is not pinned"))?;
    let (files, deleted) = if commits_only { (Vec::new(), Vec::new()) } else { super::ship::uncommitted(root, pin)? };
    let (files, deleted) = if commits_only || uncommitted || !super::ship::is_main_checkout(root) {
        (files, deleted)
    } else {
        if files.len() + deleted.len() > 0 {
            eprintln!("devhub: main checkout: its {} uncommitted file(s) stay out (--with-uncommitted ships them)", files.len() + deleted.len());
        }
        (Vec::new(), Vec::new())
    };
    let patched = apply_patches(root, pin, &files, &deleted, patches)?;
    let mut found = Vec::new();
    for f in &files {
        super::ship::entries(root, f, &mut found)?;
    }
    found.retain(|e| !super::ship::never_ships(&e.path));
    // named paths ship as they are (untracked dirs and `local/` included); a file the
    // uncommitted set has already ships once
    for p in also.iter().map(String::as_str) {
        if !super::ship::inside(p) || super::ship::never_ships_named(p) {
            return Err(format!("DEVHUB_ALSO path never ships: {p}"));
        }
        if !root.join(p).exists() {
            return Err(format!("DEVHUB_ALSO path missing: {p}"));
        }
        let mut named = Vec::new();
        super::ship::entries(root, p, &mut named)?;
        named.retain(|e| !super::ship::never_ships_named(&e.path));
        if commits_only {
            let tracked = super::ship::tracked_at_pin(root, pin, &named.iter().map(|e| e.path.clone()).collect::<Vec<_>>())?;
            if !tracked.is_empty() { return Err(super::ship::also_with_rev(&tracked, rev)); }
        }
        found.extend(named);
    }
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);
    // a patched file ships patched, whatever else would ship it
    found.retain(|e| !patched.iter().any(|(p, _, _)| *p == e.path));
    let deleted: Vec<String> = deleted.into_iter().filter(|d| !patched.iter().any(|(p, _, _)| p == d)).collect();
    let tmp = std::env::temp_dir();
    for e in std::fs::read_dir(&tmp).into_iter().flatten().flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let pid = name.strip_prefix(SNAP_PREFIX).and_then(|r| r.split('-').next()).and_then(|p| p.parse::<u32>().ok());
        // (a client this one may not signal is no gone one: [`crate::procinfo::exists`])
        if pid.is_some_and(|p| p != std::process::id() && !crate::procinfo::exists(p)) {
            let _ = std::fs::remove_dir_all(e.path());
        }
    }
    let stage = tmp.join(format!("{SNAP_PREFIX}{}-{}", std::process::id(), SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&stage);
    let mut snap = Snap { stage, entries: Vec::new(), deleted, patched: patched.len() };
    for e in found {
        if e.link.is_none() {
            let to = snap.stage.join(&e.path);
            let err = |err: std::io::Error| format!("staging {}: {err}", e.path);
            let (bytes, mtime_us, _) = super::ship::read_stable(&root.join(&e.path)).map_err(err)?;
            std::fs::create_dir_all(to.parent().unwrap_or(&snap.stage)).map_err(err)?;
            std::fs::write(&to, &bytes).map_err(err)?;
            std::fs::set_permissions(&to, std::fs::metadata(root.join(&e.path)).map_err(err)?.permissions()).map_err(err)?;
            std::fs::File::options().write(true).open(&to).and_then(|f| f.set_modified(std::time::UNIX_EPOCH + Duration::from_micros(mtime_us))).map_err(err)?;
        }
        snap.entries.push(e);
    }
    snap.entries.extend(stage_patched(&snap.stage, &patched)?);
    snap.entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(snap)
}

/// Ships a run's [`snap`] and its paths' base blobs into the lease's source layer; its line names
/// each repository's branch and commit of the pinned `rev`.
fn ship(c: &Client, root: &Path, held: &Held, rev: &str, commits_only: bool, snap: &Snap) -> Result<(), String> {
    let pin = &super::Pin::of_rev(rev).ok_or_else(|| format!("{}: rev `{rev}` is not pinned", held.id))?;
    if held.work.is_empty() {
        return Err(format!("{}: no work dir to ship to", held.id));
    }
    let t = Instant::now();
    let sent = if snap.entries.is_empty() { super::ship::Shipped::default() } else { super::ship::send(c, &held.node, &held.id, &snap.stage, &snap.entries, false)? };
    let paths: Vec<String> = snap.entries.iter().map(|e| e.path.clone()).collect();
    let list = |v: &[String]| v.iter().map(|p| makepad_remote::json_str(p)).collect::<Vec<_>>().join(",");
    let bases = super::ship::bases_json(root, pin, &paths.iter().chain(&snap.deleted).cloned().collect::<Vec<_>>())?;
    let body = format!("{{\"shipped\":[{}],\"deleted\":[{}],\"bases\":{bases}}}", list(&paths), list(&snap.deleted));
    let done = node_one(c, &held.node, "ship-done", &format!("ship-done {} {}", held.id, makepad_network::digest::base64_encode(body.as_bytes())))?;
    let source = done.get("source").and_then(|s| s.as_str()).unwrap_or("").to_string();
    let patched_note = if snap.patched == 0 { String::new() } else { format!(", {} of them patched by --patch", snap.patched) };
    let at = if commits_only { format!("{} alone (--rev)", super::rev_said(rev)) } else { format!("the worktree's HEAD {} as it was asked", super::rev_said(rev)) };
    eprintln!("{} {}: sources at {at}, source {source}; shipped {} changed file(s){patched_note} ({} KB, {} sent) and {} deletion(s) on top in {:.1} s", held.id, held.node, paths.len(), sent.bytes >> 10, sent.sent, snap.deleted.len(), t.elapsed().as_secs_f64());
    Ok(())
}

/// A diff read from a flag, still unapplied: its name, repository prefix and text (11-15).
#[derive(Debug, PartialEq)]
struct Patch {
    file: String,
    repo: String,
    diff: String,
}

/// Apply to precisely what ships: deletions, shipped worktree files, then the pin. A `--rev`
/// proof used a patched file from a later HEAD against the pin's other files (11-15).
fn apply_patches(root: &Path, pin: &super::Pin, files: &[String], deleted: &[String], patches: &[Patch]) -> Result<Vec<(String, Vec<u8>, u32)>, String> {
    let mut patched: Vec<(String, Vec<u8>, u32)> = Vec::new();
    for Patch { file, repo, diff } in patches {
        let source = std::cell::RefCell::new(None::<(String, String)>);
        let modes = std::cell::RefCell::new(Vec::new());
        let rows = super::patch::apply(diff, &|path| {
            let full = format!("{repo}{path}");
            let gone = deleted.contains(&full);
            let worktree = files.contains(&full);
            let rev = pin.repo_of(&full).1;
            let at = if gone || worktree { "in the worktree".to_string() } else { format!("at {}", &rev[..10.min(rev.len())]) };
            *source.borrow_mut() = Some((path.to_string(), at));
            let (bytes, mode) = if gone {
                (None, 0o644)
            } else if worktree {
                let bytes = super::patch::read_file(root, &full)?;
                #[cfg(unix)]
                let mode = {
                    use std::os::unix::fs::PermissionsExt;
                    if bytes.is_some() { std::fs::metadata(root.join(&full)).map_err(|e| format!("{full}: {e}"))?.permissions().mode() } else { 0o644 }
                };
                #[cfg(not(unix))]
                let mode = 0o644;
                (bytes, mode)
            } else {
                let bytes = super::ship::pin_file(root, pin, &full)?;
                let mode = super::ship::pin_mode(root, pin, &full)?.unwrap_or(0o644);
                (bytes, mode)
            };
            modes.borrow_mut().push(mode);
            Ok(bytes)
        }).map_err(|e| {
            if let Some((path, at)) = source.borrow().as_ref() {
                let full = format!("{repo}{path}");
                if let Some(why) = e.strip_prefix(&format!("{path}: ")).or_else(|| e.strip_prefix(&format!("{full}: "))) {
                    return format!("--patch {file}: {full} {at}: {why}");
                }
                if e.starts_with(&format!("adds {path}, ")) { return format!("--patch {file}: {full} {at}: {e}"); }
            }
            format!("--patch {file}: {e}")
        })?;
        for ((path, text), mode) in rows.into_iter().zip(modes.into_inner()) {
            let path = format!("{repo}{path}");
            if patched.iter().any(|(p, _, _)| *p == path) { return Err(format!("--patch {file}: {path} is in another patch too")); }
            patched.push((path, text, mode));
        }
    }
    Ok(patched)
}

/// The patched files under `stage` at their paths, with the mode of the copy they patched.
fn stage_patched(stage: &Path, patched: &[(String, Vec<u8>, u32)]) -> Result<Vec<super::ship::Entry>, String> {
    let mut entries = Vec::new();
    for (path, text, mode) in patched {
        if !super::ship::inside(path) || super::ship::never_ships(path) {
            return Err(format!("--patch: {path} never ships"));
        }
        let to = stage.join(path);
        let e = |e: std::io::Error| format!("--patch: staging {path}: {e}");
        std::fs::create_dir_all(to.parent().unwrap()).map_err(e)?;
        std::fs::write(&to, text).map_err(e)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&to, std::fs::Permissions::from_mode(*mode)).map_err(e)?;
        }
        super::ship::entries(stage, path, &mut entries)?;
        if let Some(e) = entries.last_mut() { e.exec = mode & 0o111 != 0; }
    }
    Ok(entries)
}

/// `--patch <diff>` flags before the command, out of `rest`: each diff's name, prefix and text,
/// applied only once [`snap`] knows what ships (11-15). `at`: the lane's root and where in it the run
/// was started ([`lane_root`]). A diff's paths are relative to the repository the run starts in,
/// as `git diff` writes them and `git apply` reads them there: from inside a nested checkout
/// (`apps/<name>`, [`crate::nested`]) they are that repository's.
fn take_patches(at: Option<(&Path, &str)>, rest: &mut Vec<&str>) -> Result<Vec<Patch>, String> {
    let mut patches = Vec::new();
    while let Some(i) = rest.iter().take_while(|a| **a != "--").position(|a| *a == "--patch") {
        let file = rest.get(i + 1).copied().filter(|f| *f != "--").ok_or("--patch needs a diff file")?;
        let (root, rel) = at.ok_or("--patch: run it from inside the lane's worktree")?;
        let names = crate::nested::names(root);
        let repo = crate::nested::split(&format!("{rel}/"), names.iter().map(String::as_str)).map_or(String::new(), |(name, _)| crate::nested::prefix(name));
        let diff = std::fs::read_to_string(file).map_err(|e| format!("--patch {file}: {e}"))?;
        patches.push(Patch { file: file.to_string(), repo, diff });
        rest.drain(i..i + 2);
    }
    Ok(patches)
}

/// `run <lane> <kind> … -- cmd`: pin HEAD, select the source layer, ship, run and release.
fn run(lane: &str, kind: &str, rest: &[&str]) -> i32 {
    let flags = rest.iter().position(|a| *a == "--").unwrap_or(rest.len());
    let uncommitted = rest[..flags].contains(&"--with-uncommitted");
    let mut rest: Vec<&str> = rest.iter().enumerate().filter(|(i, a)| *i >= flags || **a != "--with-uncommitted").map(|(_, a)| *a).collect();
    let patches = match take_patches(lane_root().as_ref().map(|(root, rel)| (root.as_path(), rel.as_str())), &mut rest) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("makepad devhub run: {e}");
            return 2;
        }
    };
    let mut o = match parse(kind, Some(lane), &rest) {
        Ok(o) if o.req.rounds.is_some() && o.req.kind != Kind::Timed => {
            eprintln!("makepad devhub run: --rounds is for a timed run: only timed work hands its box over between rounds");
            return 2;
        }
        Ok(o) if !o.cmd.is_empty() => Opts { req: Req { patched: !patches.is_empty(), ..o.req }, patches, ..o },
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
    if let Some(note) = rounds_note(&o.req) { eprintln!("makepad devhub run: {note}"); }
    let exit_to = o.exit_to.clone();
    let code = match native_gate(&c, &mut o) {
        Ok(()) => run_worktree(o, uncommitted, None),
        Err(code) => code,
    };
    write_exit(exit_to.as_deref(), code, None)
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
    if test.is_none() && !o.tree && !o.req.look && Src::for_run(o.req.kind, &o.cmd, false, false) == Src::Overlay {
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
    o.req.src = Src::for_run(o.req.kind, &o.cmd, o.tree, o.req.look);
    // a command that names a file of the lane's `local/` it does not ship would find nothing on
    // its box: refused before it asks ([`super::ship::named_unshipped`])
    if let Some(p) = super::ship::named_unshipped(&root, &rel, &o.cmd, &also_named()) {
        eprintln!("makepad devhub {word}: {}", super::ship::unshipped_refusal(&p));
        return (2, false);
    }
    // what it ships, read now: the worktree as its grant finds it is not what was asked ([`Snap`])
    let snapped = match snap(&root, o.req.rev.as_deref().unwrap_or(""), uncommitted, o.commits_only, &also_named(), &o.patches) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("makepad devhub {word}: {e}");
            return (2, false);
        }
    };
    let c = match retry_master(master) {
        Ok(c) => c,
        Err(e) => return (fail(e), false),
    };
    shipped_hint(&c, &root, &mut o.req, &snapped);
    // a run in rounds, told to yield, hands its box over after a round and asks again for the rest
    // ([`run_rounds`]): a new ask, behind the short ones it yielded to
    let rounds = o.req.rounds.filter(|n| *n > 1 && test.is_none());
    let mut from = 1;
    // the test's runs in a row whose lease was lost under them ([`lost_pause`])
    let mut lost_runs = 0;
    loop {
        let queued = Instant::now();
        let mut held = match acquire_within(&c, &o.req, o.start_within) {
            Ok(h) => h,
            Err((code, said)) => {
                // (a test's verdict line: one whose lease never started has none)
                if let Some(pkg) = test { let _ = writeln!(out, "{}", not_run_line(pkg, &said)); }
                return (code, false);
            }
        };
        held.adopt = adopt_body(o, rel.trim_end_matches('/'));
        let queue = queued.elapsed();
        let shipping = Instant::now();
        let rev = o.req.rev.clone().unwrap_or_default();
        // a node hub restarting mid-ship keeps the lease (its new hub takes it back from the
        // ledger): the ship is made again once it answers
        let shipped = through_restarts(&held.id, &mut c.clone(), |c| {
            held.beat.check(&held.id)?;
            ship(c, &root, &held, &rev, o.commits_only, &snapped)
        }, &master);
        let ship = shipping.elapsed();
        let running = Instant::now();
        let mut tally = crate::testlog::Tally::default();
        let mut yielded = None;
        let r = shipped.and_then(|_| {
            let mut tee = Tee { out: &mut *out, tally: &mut tally };
            match rounds {
                Some(n) => run_rounds(&c, &held, rel.trim_end_matches('/'), &o.cmd, &o.inputs, &mut tee, (from, n)).map(|(code, at)| { yielded = at; code }),
                None => run_there_into(&c, &held, rel.trim_end_matches('/'), &o.cmd, &o.inputs, &mut tee),
            }
        });
        let run = running.elapsed();
        // what the run put in $DEVHUB_KEEP, before the lease's end takes its tmp: `devhub run` and
        // `devhub test` took `--keep` and copied nothing back, three Source Library rig runs' grabs
        // in a row (11-07)
        let holder_kept = r.is_ok() && o.keep.as_deref().is_some_and(|dst| {
            let got = kept(&c, &held.node, &o.req.lane, &held.id, dst);
            eprintln!("{}", keep_said(&held.id, dst, &got));
            got.is_ok()
        });
        let mut code = finish(&c, &held, r, o.keep.as_deref(), holder_kept);
        let completed = held.exit.lock().unwrap().is_some();
        // its run went on as a job: no verdict yet; what its owner hears goes last
        if let Some(said) = held.handed.lock().unwrap().as_deref() {
            let _ = writeln!(out, "{said}");
            return (code, false);
        }
        if let (0, Some(k), Some(n)) = (code, yielded, rounds) {
            eprintln!("{}: handed its box over after round {k} of {n}; asks again for rounds {}..{n}", held.id, k + 1);
            from = k + 1;
            continue;
        }
        // a test or check whose lease was lost under it has no verdict: it says so and asks for a
        // box again by itself, and its verdict is the rerun's. Ended with no verdict, a lane read
        // it as a hang and started it again by hand (a pre-land check, a master restart, 11-24)
        if let (LOST_EXIT, Some(pkg)) = (code, test) {
            lost_runs += 1;
            let why = held.lost.lock().unwrap().clone().unwrap_or_else(|| format!("its lease {} was lost on {}", held.id, held.node));
            let pause = lost_pause(lost_runs);
            let when = if pause.is_zero() { "now".to_string() } else { format!("in {} s ({} lost in a row)", pause.as_secs(), lost_runs) };
            let line = format!("devhub test: {pkg} lost: rerun ({why}): it asks for a box again {when}; the output above is the lost run's");
            eprintln!("{line}");
            let _ = writeln!(out, "{line}");
            std::thread::sleep(pause);
            continue;
        }
        if let Some(pkg) = test {
            // a test run that executed no test is no pass
            let mut none = if code == 0 { tally.no_pass(&o.cmd) } else { None };
            if let Some(why) = &mut none { crate::testlog::no_pass_hint(why, &root, &rev, &o.cmd); }
            if none.is_some() { code = 1; }
            // a lost run's line is a not-run line: what reads the output back finds its why there
            let lost = || held.lost.lock().unwrap().clone().unwrap_or_else(|| format!("its lease {} was lost on {}; no verdict, run it again", held.id, held.node));
            let verdict = match (code, &none) { (0, _) => "passed".into(), (CANCELLED_EXIT, _) => "cancelled".into(), (LOST_EXIT, _) => format!("{NOT_RUN}{}", lost()), (_, Some(why)) => format!("failed: {why}"), _ => "failed".to_string() };
            let _ = writeln!(out, "devhub test: {pkg} {verdict} in {:.3}s (queue {:.3}s, ship {:.3}s, run {:.3}s) on {}", t.elapsed().as_secs_f64(), queue.as_secs_f64(), ship.as_secs_f64(), run.as_secs_f64(), held.node);
            // each failed target with its first panics, and where its whole output is: its end
            // kept it whole (a failed run) under the lease's id
            if none.is_none() && !matches!(code, 0 | CANCELLED_EXIT | LOST_EXIT) {
                let _ = write!(out, "{}its whole output: `devhub job result {}`\n", tally.failed_said(), held.id);
            }
        } else if let Some(line) = run_failed_said(&held.id, &held.node, *held.exit.lock().unwrap(), held.never_started.lock().unwrap().as_deref()) {
            eprintln!("{line}");
        }
        break (code, completed);
    }
}

/// How long a test whose lease was lost `n` times in a row waits before it asks for a box again:
/// not at all the first time (a master's or hub's restart, a box gone), then doubling from 2 s to
/// a minute: a run that loses its box each time asks more slowly, and never stops asking.
fn lost_pause(n: u32) -> Duration {
    if n <= 1 { Duration::ZERO } else { Duration::from_secs((1u64 << (n - 1).min(6)).min(60)) }
}

/// What a timed run without `--rounds` hears as it asks: it hands its box over only when it ends.
/// A 60-min script that never read $DEVHUB_YIELD held a bench box 70 min while three short timed
/// asks waited, told to yield five times (11-14). None for other runs, and for one in rounds.
fn rounds_note(req: &Req) -> Option<String> {
    (req.kind == Kind::Timed && req.rounds.is_none()).then(|| "a timed run without --rounds hands its box over only when it ends: a short timed ask that comes meanwhile waits for all of it. `--rounds N` runs the command once per round (DEVHUB_ROUND=1..N, DEVHUB_ROUNDS=N) and, when a short timed ask waits, hands the box over after the round and asks again for the rest (DEVHUB_RESUMED=1 in the first round after); each round's output goes in files named by its round".to_string())
}

/// Rounds `from..=n` of a run in rounds (`--rounds N`), its command once each in `held`'s lease
/// ([`rounds_until`]).
fn run_rounds(c: &Client, held: &Held, rel: &str, cmd: &[String], inputs: &[super::ship::Input], out: &mut dyn Write, (from, n): (u32, u32)) -> Result<(i32, Option<u32>), String> {
    rounds_until((from, n), cmd, |k, argv| {
        eprintln!("{}: round {k} of {n}{}", held.id, if from > 1 && k == from { " (resumed: DEVHUB_RESUMED=1)" } else { "" });
        run_there_into(c, held, rel, argv, inputs, &mut *out)
    }, || told_to_yield(c, &held.id))
}

/// Runs rounds `from..=n` of `cmd` ([`round_argv`]) with `run`. It stops at a failed round (its
/// exit), and after a round before the last once `told` says the lease was told to yield
/// ([`super::sched::Sched::say_overruns`]): `(0, Some(k))`, and the next lease goes on at k+1.
fn rounds_until(
    (from, n): (u32, u32),
    cmd: &[String],
    mut run: impl FnMut(u32, &[String]) -> Result<i32, String>,
    mut told: impl FnMut() -> bool,
) -> Result<(i32, Option<u32>), String> {
    for k in from..=n {
        let code = run(k, &round_argv(cmd, k, n, from > 1 && k == from))?;
        if code != 0 { return Ok((code, None)); }
        if k < n && told() { return Ok((0, Some(k))); }
    }
    Ok((0, None))
}

/// Round `k` of `n`'s command: `DEVHUB_ROUND`, `DEVHUB_ROUNDS` and, in the first round after its
/// run handed the box over, `DEVHUB_RESUMED=1` (another run had the box meanwhile: its caches may
/// be cold, so a warm-up runs again), joined with the command's own `env` words: the node reads
/// one `env` prefix as the run's environment.
fn round_argv(cmd: &[String], k: u32, n: u32, resumed: bool) -> Vec<String> {
    let mut argv = vec!["env".to_string(), format!("DEVHUB_ROUND={k}"), format!("DEVHUB_ROUNDS={n}")];
    if resumed { argv.push("DEVHUB_RESUMED=1".to_string()); }
    let rest = if cmd.first().is_some_and(|c| c == "env") { &cmd[1..] } else { cmd };
    argv.extend(rest.iter().cloned());
    argv
}

/// Whether the master told lease `id` to yield (its beat's `yield`); not when it does not answer.
fn told_to_yield(c: &Client, id: &str) -> bool {
    one(c, "beat", id).ok().and_then(|v| v.get("yield").and_then(Value::as_bool)).unwrap_or(false)
}

/// The JIT ids an overlay run is placed by, read off what it ships ([`super::loaders::shipped_ids`]):
/// its pin's, and the lane's own when its edits change the JIT's inputs, in the order of the
/// master's compilers (`jit compilers`). Without them (a master that predates the query, a tree it
/// cannot read) the master sets the pin's at intake, as it did.
#[cfg(feature = "checkout")]
fn shipped_hint(c: &Client, root: &Path, req: &mut Req, snap: &Snap) {
    if req.src != Src::Overlay || !req.on_node() || !req.hint.is_empty() || req.loader.as_deref().is_some_and(|id| !id.is_empty()) { return; }
    let Some(pin) = req.rev.as_deref().and_then(super::Pin::of_rev) else { return };
    let Ok(v) = one(c, "jit", "compilers") else { return };
    let compilers: Vec<String> = v.get("compilers").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect();
    if compilers.is_empty() { return; }
    let edits: Vec<(String, Option<PathBuf>)> = snap.entries.iter()
        .map(|e| (e.path.clone(), Some(if e.link.is_some() { root.join(&e.path) } else { snap.stage.join(&e.path) })))
        .chain(snap.deleted.iter().map(|p| (p.clone(), None)))
        .collect();
    match super::loaders::shipped_ids(root, &pin.makepad, &compilers, &edits) {
        Ok((hint, own)) => (req.hint, req.own_jit) = (hint, own),
        Err(e) => eprintln!("devhub: JIT placement hint: {e} (the master's is the pin's)"),
    }
}

#[cfg(not(feature = "checkout"))]
fn shipped_hint(_: &Client, _: &Path, _: &mut Req, _: &Snap) {}

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

/// Where a JIT miss's cargo check runs: a named box stays the box (its cargo run is the one asked
/// for); else a pool box. A macOS ask keeps every Mac, the hot one too (the sched takes macOS work
/// on any Mac): pinned to the pool, it had one Mac, held by a person's screen session, while the
/// other stood free (L929083 queued 13 min, 11-13).
fn on_cargo_boxes(req: &mut Req) {
    if req.node.is_none() && !req.requires_macos() {
        req.needs.retain(|(k, _)| k != "role");
        req.needs.push(("role".into(), "pool".into()));
    }
}

/// `jit_log`: what the JIT run that failed printed, kept first whatever cargo says: a run whose
/// cargo check passed kept only cargo's output, and the JIT's own failing message was gone (11-07).
fn cargo_test(mut o: Opts, pkg: &str, args: &[String], errors: &[String], known: Option<Value>, inputs: Option<MissInputs>, jit_log: Option<&str>) -> (i32, bool) {
    if let Some(log) = jit_log.filter(|l| !l.trim().is_empty()) {
        println!("devhub test: the JIT run's output:");
        print!("{log}");
        if !log.ends_with('\n') { println!(); }
    }
    if let Some(miss) = &known { println!("{}", crate::jit_misses::notice(miss)); }
    else { eprintln!("devhub test: JIT compile/resolve failure on {pkg} {}; checking with cargo on a pool box", crate::jit_misses::target(args)); }
    let _ = std::io::stdout().flush();
    // what cargo builds, read as its run ships ([`crate::jit_misses::other_sources`]): a miss is
    // the JIT's only when cargo built what the JIT refused
    let (shipped, unheld) = if known.is_none() { (Some(miss_inputs(&o, pkg)), crate::jit_misses::unheld_cited(Path::new(&miss_repo(&o)), errors)) } else { (None, None) };
    o.cmd = ["cargo", "test", "--release", "--message-format=json-render-diagnostics", "-p", pkg].iter().map(|s| s.to_string()).chain(args.iter().cloned()).collect();
    o.tree = true;
    on_cargo_boxes(&mut o.req);
    o.req.loader = None;
    o.req.class = format!("native:{}", o.req.class);
    o.req.cost = CostClass::Medium;
    o.req.cost_asked = true;
    o.req.est_min = None;
    let mut output = Vec::new();
    let (code, completed) = run_worktree_into(&mut o, false, None, &mut output);
    let (built, output) = cargo_output(&String::from_utf8_lossy(&output));
    // Cancellation and transport/setup failures never become evidence of a JIT miss.
    if code == CANCELLED_EXIT { print!("{output}"); return (code, false); }
    if !completed {
        print!("{output}");
        eprintln!("devhub test: cargo fallback did not finish; no JIT-miss verdict recorded");
        return (code, false);
    }
    if built == Some(false) {
        eprintln!("devhub test: {pkg} also fails to compile with cargo; this is the lane's error. JIT diagnostics:");
        for error in errors { eprintln!("{error}"); }
        print!("{output}");
        return (1, true);
    }
    if built != Some(true) {
        print!("{output}");
        eprintln!("devhub test: cargo did not report a completed build; no JIT-miss verdict recorded");
        return (if code == 0 { 2 } else { code }, false);
    }
    // a lane whose JIT is older than work's: its own code, not the JIT, until it runs on work's
    let repo = o.req.repo_dir.clone().unwrap_or_else(|| ".".into());
    let behind = o.req.rev.as_deref().and_then(super::Pin::of_rev).and_then(|pin| crate::jit_misses::behind_work_jit(Path::new(&repo), &pin.pair()));
    if let (None, Some(base)) = (&known, &behind) {
        eprintln!("devhub test: not a JIT miss: this lane's JIT is older than work's (work changed the JIT since the lane's base {}), and its code may use what landed since: merge work and run it again (cargo builds it; the JIT said {})", &base[..base.len().min(10)], errors.first().map_or("", String::as_str));
    } else if known.is_none() {
        let inputs = inputs.and_then(|h| h.join().ok().flatten());
        let shipped = shipped.and_then(|h| h.join().ok().flatten());
        if let Some(why) = crate::jit_misses::other_sources(inputs.as_deref(), shipped.as_deref(), unheld.as_ref()) {
            eprintln!("devhub test: not a JIT miss, none recorded: {why}; run it again (the JIT said {})", errors.first().map_or("", String::as_str));
        } else { match test_miss(&o, pkg, args, "record", errors, inputs.as_deref()) {
            Ok(miss) => println!("{}", crate::jit_misses::notice(&miss)),
            Err(e) => eprintln!("devhub test: JIT miss on {pkg} {} (recording failed: {e}): the JIT said {}; cargo builds it", crate::jit_misses::target(args), errors.first().map_or("", String::as_str)),
        } }
    }
    print!("{output}");
    (code, false)
}

/// One-shot tests change their default route only with at least this measured run-phase gain.
const ROUTE_MIN_GAIN: f64 = 0.25;

/// One-shot tests default to the JIT, or cargo for JIT-own crates once the JIT is measured; true
/// selects the JIT. A class with no JIT measure runs under the JIT, which measures it.
/// Compares run-phase medians (the command's own time, its build included; not the dev JIT build).
/// Switch only when both medians exist and the other side is at least ROUTE_MIN_GAIN faster.
pub fn test_route(own: bool, jit: Option<u64>, cargo: Option<u64>) -> (bool, String) {
    let default = if own { "cargo" } else { "the JIT" };
    let why = if own { "compiled only into the JIT binary; " } else { "" };
    match (jit, cargo) {
        // a class the JIT never ran has no JIT measure: it runs there once and is measured (the JIT's
        // own crates stayed on cargo for good, and differed from the tests bucket, which runs
        // every package under the JIT: a lane could not reproduce its red, 11-13)
        (None, _) => (true, format!("the JIT ({why}no JIT measurement yet: this run measures it)")),
        (_, None) => (!own, format!("{default} ({why}no cargo measurement yet{})", if own { ": this run measures it" } else { "" })),
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

/// The class `devhub test` measures a package's run by, and its measured routing's key:
/// `test:<pkg>`, or `test:<pkg>:<target>` for one target (`lib`, `bin:<name>`, a test's name,
/// [`ALL_TESTS`]). The native gate names a cargo test's class with it too ([`super::native`]).
pub(crate) fn test_class(pkg: &str, target: Option<&str>) -> String {
    target.map_or_else(|| format!("test:{pkg}"), |t| format!("test:{pkg}:{t}"))
}

/// A package's harness targets, which decide a live run without a selector: one is a warm live
/// session, several run the package whole; other test targets are named so none vanish silently.
struct LiveTargets {
    runs: Vec<Vec<String>>,
    not_live: Vec<String>,
}

/// Discover from the worktree on every call, including new files not yet in git.
#[cfg(feature = "checkout")]
fn live_targets(root: &Path, pkg: &str, flags: &[String]) -> Result<LiveTargets, String> {
    let mut features = Vec::new();
    let mut words = flags.iter();
    while let Some(word) = words.next() {
        match word.as_str() {
            "--" => break,
            "--features" | "-F" => {
                features.push(word.clone());
                features.push(words.next().ok_or_else(|| format!("{word} needs a value"))?.clone());
            }
            "--all-features" | "--no-default-features" => features.push(word.clone()),
            f if f.starts_with("--features=") || f.starts_with("-F") => features.push(word.clone()),
            _ => {}
        }
    }
    let targets_in = |dir: &Path| relax_base::resolve::test_targets(&std::sync::Arc::new(relax_base::Session::default()), &dir.to_string_lossy(), pkg, &features, &[], &[], &[]);
    // a package of a followed nested repository the worktree lacks: its targets as the user's
    // checkout holds it, which the lane's runs pin at its tip ([`crate::nested::absent`])
    let targets = targets_in(root).or_else(|e| match crate::nested::hub(root).filter(|_| !crate::nested::absent(root).is_empty()) {
        Some(hub) => targets_in(&hub).map_err(|_| e),
        None => Err(e),
    })?;
    let mut out = LiveTargets { runs: Vec::new(), not_live: Vec::new() };
    if targets.lib.as_ref().is_some_and(|l| l.test) { out.runs.push(vec!["--lib".into()]); }
    for (flag, targets) in [("--bin", targets.bins), ("--test", targets.tests)] {
        for (name, _, harness) in targets {
            if harness { out.runs.push(vec![flag.into(), name]); }
            else { out.not_live.push(format!("{flag} {name} (harness = false)")); }
        }
    }
    out.not_live.extend(targets.examples.into_iter().map(|(name, _, _)| format!("--example {name} (examples run without --live)")));
    if out.runs.is_empty() {
        return Err(format!("{pkg} has no harness target to run live{}", if out.not_live.is_empty() { String::new() } else { format!("; not run live: {}", out.not_live.join(", ")) }));
    }
    Ok(out)
}

/// What the tests bucket did with `devhub test <pkg> <select>`'s unit (`test <pkg> --test T`, else
/// `test <pkg>`), from this machine's bucket ledger (the master's): its runner and last look, and
/// whether that look was red. None: no bucket run here looked at it.
#[cfg(feature = "checkout")]
fn bucket_said(pkg: &str, select: &[&str]) -> Option<(String, bool)> {
    let home = crate::proto::home();
    let target = select.iter().position(|w| *w == "--test").and_then(|i| select.get(i + 1)).map(|t| format!("test {pkg} --test {t}"));
    let (unit, (failed, started, secs)) = target.into_iter().chain([format!("test {pkg}")]).find_map(|u| crate::bucket::tests_last(&home, &u).map(|l| (u, l)))?;
    let ago = super::sched::dur(crate::proto::wall_ms().saturating_sub(started));
    let took = secs.map_or(String::new(), |s| format!(" in {}", super::sched::dur(s * 1000)));
    Some((format!("the tests bucket runs `{unit}` under the JIT (`makepad test {}`): {} {ago} ago{took}", &unit[5..], if failed { "red" } else { "passed" }), failed))
}

/// devhub's own `test` flags that the test binary never takes (libtest's `--test` is its own): one
/// after `--` reached the binary, which refused it, and that read as a JIT miss (M98, 11-16).
const TEST_FLAGS: [&str; 23] = ["--env", "--native", "--live", "--offline", "--fresh", "--rev", "--tree", "--needs", "--node", "--avoid", "--series", "--pin-why", "--priority", "--jit", "--cost", "--est-min", "--cores", "--class", "--features", "--no-default-features", "--lib", "--bin", "--tests"];

/// The refusal of a devhub flag among the test binary's arguments (the words after `--`).
fn flag_after_separator(args: &[&str]) -> Option<String> {
    let flag = args.iter().skip_while(|a| **a != "--").skip(1).map(|a| a.split('=').next().unwrap_or(a)).find(|a| TEST_FLAGS.contains(a))?;
    Some(format!("`{flag}` after `--` goes to the test binary; put it before `--`"))
}

/// Whether the run compiled every test target under the package's default features.
fn tested_in_full(target: Option<&str>, select: &[&str]) -> bool {
    matches!((target, select), (None, []) | (Some(ALL_TESTS), ["--tests"]))
}

fn test(lane: &str, pkg: &str, rest: &[&str]) -> i32 {
    let parsed = (|| -> Result<_, String> {
        if pkg.is_empty() || pkg.starts_with('-') || !pkg.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')) {
            return Err(format!("`{pkg}` is not a package name"));
        }
        let (mut flags, mut select, mut args) = (Vec::new(), Vec::new(), Vec::new());
        let (mut native, mut live, mut offline, mut target, mut rev_spec) = (false, false, false, None::<String>, None::<&str>);
        let mut fresh = false;
        let mut asked_native = false;
        let mut env: Vec<(String, String)> = Vec::new();
        let mut i = 0;
        while i < rest.len() {
            let val = || rest.get(i + 1).copied().filter(|v| !v.is_empty() && !v.starts_with('-')).ok_or_else(|| format!("{} needs a value", rest[i]));
            match rest[i] {
                "--" => { args.extend_from_slice(&rest[i..]); break; }
                // the live session's environment, as `live start --env` (a saved JIT cache state
                // replayed, 11-18); the master checks each name
                "--env" => {
                    let pair = val()?;
                    let (k, v) = pair.split_once('=').filter(|(k, _)| !k.is_empty()).ok_or("--env needs K=V")?;
                    env.push((k.to_string(), v.to_string()));
                    i += 1;
                }
                // `--native "<reason>"`: the reason goes to the JIT-first gate (lease/native.rs)
                "--native" => {
                    (native, asked_native) = (true, true);
                    if let Ok(reason) = val() { flags.extend(["--native", reason]); i += 1; }
                }
                "--live" => live = true,
                "--offline" => offline = true,
                "--fresh" => fresh = true,
                "--rev" => { rev_spec = Some(val()?); i += 1; }
                "--tree" => flags.push("--tree"),
                "--needs" | "--node" | "--avoid" | "--series" | "--pin-why" | "--priority" | "--jit" | "--cost" | "--est-min" | "--cores" | "--class" => { flags.extend([rest[i], val()?]); i += 1; }
                "--features" => { select.extend([rest[i], val()?]); i += 1; }
                "--no-default-features" => select.push(rest[i]),
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
        if offline && !live { return Err("--offline needs --live: it is a live session's sandbox".into()); }
        if fresh && !live { return Err("--fresh needs --live: a run without it runs its tests anyway".into()); }
        if let Some(why) = flag_after_separator(&args) { return Err(why); }
        let words: Vec<String> = std::iter::once("test").chain(args.iter().skip(1).copied()).map(str::to_string).collect();
        if let Some(why) = crate::proto::joined_flags(&words) { return Err(why); }
        let mut o = parse("test", Some(lane), &flags)?;
        let root = o.req.repo_dir.as_deref().map(PathBuf::from).unwrap_or_else(cwd);
        // `--rev`: the commits alone (a landed sha, say), under the same JIT, none of the lane's
        // uncommitted files with them (a lane reverted a file by hand to leave it out, 11-06)
        let rev = match rev_spec { Some(spec) => pin_lane_rev(&root, spec, &o.req.lane, Some(&master_paired), Some(&master_beside))?, None => head_rev(&root, &o.req.lane)? };
        let pin = super::Pin::of_rev(&rev).ok_or("test: missing commit pair")?;
        o.req.rev = Some(rev);
        o.commits_only = rev_spec.is_some();
        if live && o.commits_only {
            return Err("--live follows the lane's saves; --rev runs its commits once (drop one of them)".into());
        }
        if live && native { return Err("--native cannot be combined with --live".into()); }
        if !env.is_empty() && !live { return Err("--env sets a live session's environment: add --live".into()); }
        o.env = env;
        o.fresh = fresh;
        // (--cost and --est-min are the live job's hints, as everywhere: refused, two lanes lost a
        // run each to a flag AGENTS.md asks of every run, 11-16)
        let mut suite = if live && matches!(target.as_deref(), None | Some(ALL_TESTS)) {
            #[cfg(feature = "checkout")]
            { Some(live_targets(&root, pkg, &select.iter().map(|s| s.to_string()).collect::<Vec<_>>())?) }
            #[cfg(not(feature = "checkout"))]
            { return Err("a live run of a whole package needs the checkout feature to discover its targets; select --lib, --bin B or --test T".into()); }
        } else { None::<LiveTargets> };
        // A package of several harness targets runs whole: one JIT run of `makepad test <pkg>` on
        // one lease, the lane's saves in it, as the pre-land check runs it. A JIT live session
        // keeps one target warm, and a session per target was a lease per target: one run asked
        // 9 for one package, 35 sat idle on one box (11-15, 11-16). --lib, --bin B or --test T
        // keeps a warm session.
        let whole = suite.take_if(|s| s.runs.len() > 1);
        if let Some(whole) = &whole {
            let targets = whole.runs.iter().map(|s| s.join(" ")).collect::<Vec<_>>().join(", ");
            if offline { return Err(format!("--offline is one live session's sandbox, and {pkg} has {} harness targets: select one ({targets})", whole.runs.len())); }
            if !o.env.is_empty() { return Err(format!("--env is one live session's environment, and {pkg} has {} harness targets: select one ({targets})", whole.runs.len())); }
            eprintln!("devhub test: {pkg}: {} harness targets ({targets}): one JIT run of the whole package on one lease, no live session per target (--lib, --bin B or --test T keeps one warm)", whole.runs.len());
            live = false;
        }
        let whole = whole.is_some();
        let default_class = o.req.class.is_empty();
        // the package's own class: measured routing reads it whatever --class names the lease's
        // series, as the native gate does (a timed series' `--class timed` run was routed by that
        // class's medians, which the gate could not tie to the package: refused, 11-24)
        let pkg_class = test_class(pkg, target.as_deref());
        if default_class {
            o.req.class = pkg_class.clone();
            if live { o.req.class = format!("live:{}", o.req.class); }
        }
        let own = if native { false } else {
            let p = crate::jit_misses::test_package_at(&root, &pin, pkg)?;
            // tests that read the checkout itself (its history, cargo on it) say so in their
            // Cargo.toml, and run in the lane's tree: an overlay has neither
            if let Some(why) = p.tree.as_ref().filter(|_| !o.tree) {
                eprintln!("devhub test: {}", crate::jit_misses::tree_line(pkg, &why));
                o.tree = true;
            }
            // a target its Cargo.toml runs under cargo (native-tests): a run that selects it goes
            // there whole, as the tests bucket runs it. A live run checks the target it found.
            let select = if let Some(suite) = &suite {
                suite.runs.iter().map(|s| match s[0].as_str() {
                    "--lib" => "lib".to_string(),
                    "--bin" => format!("bin:{}", s[1]),
                    _ => format!("test:{}", s[1]),
                }).collect()
            } else { match target.as_deref() {
                None => Vec::new(),
                Some(ALL_TESTS) => vec!["tests".to_string()],
                Some(t) if t == "lib" || t.starts_with("bin:") => vec![t.to_string()],
                Some(t) => vec![format!("test:{t}")],
            } };
            let declared: Vec<String> = crate::jit_misses::native_selected(&p.native, &select).into_iter().map(|(t, why)| format!("{t}: {why}")).collect();
            if !declared.is_empty() {
                if live { return Err(format!("{pkg} {}: its Cargo.toml runs it under cargo (native-tests), and --live is the JIT's warm run: drop --live", declared.join("; "))); }
                let rest = if target.is_none() { " (the whole package with it: --lib, --bin B or --test T runs another target under the JIT)" } else { "" };
                eprintln!("devhub test: {}: cargo, as its Cargo.toml's native-tests says: {}{rest}", o.req.class, declared.join("; "));
                native = true;
            }
            // its tests run apps side by side, as many as the cores they are told hold: told
            // APP_TESTS_CORES unless it asks, and charged its class's measure (Req::cores_default).
            // A live session's calls are charged their own peaks.
            if p.app_tests && o.req.cores.is_none() && !live {
                (o.req.cores, o.req.cores_default) = (Some(super::APP_TESTS_CORES), true);
            }
            // its tests drive the app's windows: only a box that shows them runs it, with the WM
            // session given at its start (`gui=1`), waiting while none can (`opens_windows`);
            // `--needs gui=0` or a display need of its own says otherwise
            if p.opens_windows(&select) && !o.req.needs.iter().any(|(k, _)| k == "gui" || k == "display") {
                o.req.needs.push(("gui".to_string(), "1".to_string()));
                eprintln!("devhub test: {pkg}: its tests open the app's windows (it depends on makepad-test): a box that shows them runs it (gui=1)");
            }
            // its tests play to the output device, whose callback clocks them: a box whose audio
            // output runs takes it (`audio=1`, measured by each Mac's and Linux box's hub); `--needs audio=0`
            // says otherwise. On one Mac the output never ran under any app, and the synth
            // app's transport tests failed there at any revision (11-26).
            let audio: Vec<String> = crate::jit_misses::native_selected(&p.audio, &select).into_iter().map(|(t, why)| format!("{t}: {why}")).collect();
            if !audio.is_empty() && !o.req.needs.iter().any(|(k, _)| k == "audio") {
                o.req.needs.push(("audio".to_string(), "1".to_string()));
                eprintln!("devhub test: {pkg}: its Cargo.toml's audio-tests play to the output device ({}): a box whose audio output runs takes it (audio=1)", audio.join("; "));
            }
            p.own
        };
        // Iteration always uses the JIT (a whole package's live run too); measured routing is only
        // for one-shot runs.
        if !native && !live && !whole {
            let v = retry_master(|| master().and_then(|c| one(&c, "stats", &format!("stats route {} {pkg_class} {own}", o.req.kind.as_str()))))?;
            let use_jit = v.get("jit").and_then(Value::as_bool).ok_or("stats route: no jit choice")?;
            let reason = v.get("why").and_then(Value::as_str).ok_or("stats route: no reason")?.to_string();
            // the tests bucket's runner for it, and its last look: a red there is reproduced as
            // it ran (a lane's cargo run of a bucket red under the JIT passed, 11-13)
            #[cfg(feature = "checkout")]
            let (use_jit, reason) = match bucket_said(pkg, &select) {
                Some((said, red)) => {
                    eprintln!("devhub test: {said}");
                    if red && !use_jit { (true, format!("the JIT, as the tests bucket that found it red ran it (else {reason})")) } else { (use_jit, reason) }
                }
                None => (use_jit, reason),
            };
            eprintln!("devhub test: {}: {reason}", o.req.class);
            if !use_jit { native = true; o.routed = reason; }
        }
        // a filtered run is measured apart from its whole suite (routing above reads the suite's)
        let filter = super::filter_key(&args);
        if default_class && !filter.is_empty() { o.req.class = format!("{}?{filter}", o.req.class); }
        if (live || whole) && !flags.contains(&"--priority") { o.req.priority = Priority::Interactive; }
        let mut test_args: Vec<&str> = select.iter().copied().filter(|s| suite.is_none() || *s != "--tests").collect();
        test_args.extend(args.iter().copied());
        // the JIT: release with the checks ([`super::JIT_TEST_PROFILE`]); cargo: release, as every
        // other cargo build of the tree (a checked one would be a second copy of target/)
        let mut cmd = if native { vec!["cargo", "test", "--release", "-p", pkg] } else { ["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--", "test", pkg].into_iter().chain(super::JIT_TEST_PROFILE).collect() };
        let full = tested_in_full(target.as_deref(), &select);
        cmd.extend(select);
        cmd.extend(args);
        o.cmd = cmd.into_iter().map(str::to_string).collect();
        if native { o.req.class = format!("native:{}", o.req.class); }
        o.tree |= native;
        let test_args: Vec<String> = test_args.into_iter().map(str::to_string).collect();
        Ok((o, native, asked_native, live, whole, offline, if live { live_select(test_args) } else { test_args }, suite, full))
    })();
    let (mut o, native, asked_native, live, whole, offline, mut test_args, suite, full) = match parsed {
        Ok(v) => v,
        Err(e) => { eprintln!("test: {e}"); return 2; }
    };
    if let Some(suite) = suite {
        test_args = suite.runs[0].iter().chain(&test_args).cloned().collect();
        if !suite.not_live.is_empty() { println!("devhub test: {pkg}: not run live: {}", suite.not_live.join(", ")); }
    }
    // a run that passed is the lane's pre-land check when devhub chose its route (a `--native` one
    // is a baseline, a whole package's live run an iteration): the packages its diff touches are
    // checked in their shipping and test configurations, placed as the run was
    let (lane, commits_only, placed) = (o.req.lane.clone(), o.commits_only, o.req.clone());
    if native {
        if let Err(code) = retry_master(master).map_err(fail).and_then(|c| native_gate(&c, &mut o)) { return code; }
        let rev = o.req.rev.clone().unwrap_or_default();
        let code = run_worktree(o, false, Some(pkg));
        return if code == 0 && !asked_native && !whole { pre_land_checks(&lane, &rev, commits_only, &placed, full.then_some(pkg)) } else { code };
    }
    match test_miss(&o, pkg, &test_args, "lookup", &[], None) {
        Ok(miss) if !miss.is_null() => {
            let errors = miss.get("error").and_then(Value::as_arr).unwrap_or(&[]).iter().filter_map(Value::as_str).map(str::to_string).collect::<Vec<_>>();
            // The spool's sandbox is the live run, never a cargo fallback (11-15).
            if offline { return fail(format!("an offline test runs only as a JIT live session; {pkg}'s recorded JIT miss would use cargo: {}", errors.join("; "))); }
            return cargo_test(o, pkg, &test_args, &errors, Some(miss), None, None).0;
        }
        Ok(_) => {}
        Err(e) => return fail(e),
    }
    let inputs = miss_inputs(&o, pkg);
    if live { return single_live_test(o, pkg, &test_args, inputs, offline); }
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
    if completed && code != CANCELLED_EXIT && !errors.is_empty() { return cargo_test(o, pkg, &test_args, &errors, None, Some(inputs), Some(&*log)).0; }
    print!("{log}");
    if let Some(line) = log.lines().map(str::trim).find(|l| crate::jit_misses::usage_error(l)).filter(|_| code != 0) {
        eprintln!("devhub test: the test binary refused its arguments (`{line}`): a usage error, not a JIT miss");
    }
    let (lines, code) = warnings_verdict(&run_warnings(&log), code);
    for l in lines { println!("{l}"); }
    if code != 0 || whole { return code; }
    pre_land_checks(&lane, o.req.rev.as_deref().unwrap_or(""), commits_only, &placed, full.then_some(pkg))
}

/// One check of the hand-in's feature-set gate: the package, a feature set a dependent builds it
/// with, and that dependent ([`crate::orch::ci::dependent_feature_sets`]).
#[cfg(feature = "checkout")]
pub(crate) type FeatureCheck = (String, crate::orch::ci::FeatureSet, String);

/// The checks a hand-in owes: each package it touches, once per feature set other than its
/// defaults that a dependent of the tree builds it with.
#[cfg(feature = "checkout")]
pub(crate) fn feature_checks(touched: &[String], sets: &std::collections::BTreeMap<String, std::collections::BTreeMap<crate::orch::ci::FeatureSet, String>>) -> Vec<FeatureCheck> {
    touched.iter().flat_map(|pkg| sets.get(pkg).into_iter().flatten().map(move |(set, by)| (pkg.clone(), set.clone(), by.clone()))).collect()
}

/// The checks' verdict from each run's exit and log: a compile error or a warning at a source
/// line refuses the hand-in, naming the package and the feature set; a run that never started or
/// said no error line is noted, and the buckets check the landing after (no verdict blocks it).
#[cfg(feature = "checkout")]
pub(crate) fn feature_verdict(checks: &[FeatureCheck], ran: &[(i32, String)], red: &[Red]) -> Result<Vec<String>, Vec<String>> {
    let (mut notes, mut refused, none) = (Vec::new(), Vec::new(), Red::new());
    for (i, ((pkg, set, by), (code, log))) in checks.iter().zip(ran).enumerate() {
        let named = format!("{pkg} {} (as {by} builds it)", set.said());
        let (failures, old) = brought(crate::jit_misses::check_failures(log), red.get(i).unwrap_or(&none));
        if old > 0 { notes.push(red_note("feature sets", &named, old)); }
        if !failures.is_empty() {
            refused.push(format!("{named}: {} error and warning line(s); `makepad check {pkg} {}` shows it alone", failures.len(), set.said()));
            refused.extend(failures.into_iter().map(|l| format!("  {l}")));
        } else if *code == 0 {
            notes.push(format!("{named}: ok"));
        } else if old == 0 {
            notes.push(format!("{named}: not checked ({}); the buckets check it after the landing", no_verdict(*code, log)));
        }
    }
    if refused.is_empty() { Ok(notes) } else { Err(refused) }
}

/// What a run says at the tips without the hand-in: its error and warning lines' keys.
#[cfg(feature = "checkout")]
type Red = std::collections::HashSet<String>;

/// The keys ([`crate::bucket::error_key`]: file and message) of the error and warning lines `run`
/// says at the tips without the hand-in ([`Merged::tips`]), asked only of a run that failed at
/// the merge.
#[cfg(feature = "checkout")]
fn red_at_tips(run: &crate::jit_misses::CheckRun, tips: &str, check: &(dyn Fn(&crate::jit_misses::CheckRun, &str) -> (i32, String) + Sync)) -> Red {
    let (_, log) = check(run, tips);
    crate::jit_misses::check_failures(&log).iter().filter_map(|l| crate::bucket::error_key(l)).collect()
}

/// `failures` split by what the tips without the hand-in already say (`red`): the lines the
/// hand-in brings, and how many were red before it. An error already red at the tips is the
/// buckets' to fix and never refuses a hand-in (user, 11-16): a Fab hand-in was refused for an
/// error in a file it did not touch, red at work since an earlier landing.
#[cfg(feature = "checkout")]
fn brought(failures: Vec<String>, red: &Red) -> (Vec<String>, usize) {
    let (old, new): (Vec<String>, Vec<String>) = failures.into_iter().partition(|l| crate::bucket::error_key(l).is_some_and(|k| red.contains(&k)));
    (new, old.len())
}

/// The note for `n` lines of `named` already red at the tips.
#[cfg(feature = "checkout")]
fn red_note(what: &str, named: &str, n: usize) -> String {
    format!("{what}: {named}: {n} error and warning line(s) already red at the tips without the hand-in: not its own, the buckets say them")
}

/// Maximum diagnostic lines printed in a [`tests_gate`] refusal; every line is still checked
/// against the tips and counted before capping the printed output.
#[cfg(feature = "checkout")]
const TESTS_GATE_MAX_SHOWN: usize = 40;

/// Deduplicates identical diagnostic lines while preserving first-seen order: `cargo check
/// --tests` builds both `lib` and `lib --test`, emitting the same `src/lib.rs` warning twice
/// (compare `land_gate.sh` `cargo_units` `awk '!seen[$0]++'`).
#[cfg(feature = "checkout")]
fn dedup_lines(lines: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::with_capacity(lines.len());
    lines.into_iter().filter(|l| seen.insert(l.clone())).collect()
}

/// Formats the refusal lines for [`tests_gate`] after [`brought`] removes lines already red at
/// the tips. Caller guarantees `!errors.is_empty() || !warnings.is_empty()`.
#[cfg(feature = "checkout")]
fn tests_gate_refusal(named: &str, alone: &str, took: &str, errors: &[String], warnings: &[String]) -> Vec<String> {
    let header = match (errors.len(), warnings.len()) {
        (e, 0) => format!("test targets: rustc's check of {named} fails: {e} error line(s) in {took}; `{alone}` shows it alone"),
        (0, w) => format!("test targets: rustc's check of {named} fails: {w} new warning line(s) in {took} (native rustc check; catches lints the JIT check doesn't run); `{alone}` shows it alone"),
        (e, w) => format!("test targets: rustc's check of {named} fails: {e} error line(s) and {w} new warning line(s) in {took}; `{alone}` shows it alone"),
    };
    let mut refused = vec![header];
    let total = errors.len() + warnings.len();
    for line in errors.iter().chain(warnings).take(TESTS_GATE_MAX_SHOWN) {
        refused.push(format!("  {line}"));
    }
    if total > TESTS_GATE_MAX_SHOWN {
        refused.push(format!("  ... and {} more line(s); `{alone}` shows all", total - TESTS_GATE_MAX_SHOWN));
    }
    refused
}

/// The hand-in's check of the test targets of the packages it touches by rustc
/// ([`crate::jit_misses::tests_run`]), matching the fast bucket's `step_testcheck` after the
/// landing: any new error or warning line refuses the hand-in (zero warnings), errors and
/// warnings already red at the tips without the hand-in are excused per line via
/// [`red_at_tips`] / [`brought`], and a run with no verdict (skipped past [`CHEAP_GATE_MS`],
/// deferred past [`gate_start_bound`], or lost) leaves them to the fast bucket. The JIT runs
/// a lane's tests with its own type checker and does not run every rustc late lint (#5087,
/// 11-15; #10571, 11-27).
#[cfg(feature = "checkout")]
fn tests_gate(tested: &[String], check: &(dyn Fn(&crate::jit_misses::CheckRun) -> (i32, String) + Sync), red: &(dyn Fn(&crate::jit_misses::CheckRun) -> Red + Sync)) -> Result<Vec<String>, Vec<String>> {
    let t = Instant::now();
    let Some(run) = crate::jit_misses::tests_run(tested) else { return Ok(Vec::new()) };
    let named = run.packages.join(", ");
    let (code, log) = check(&run);
    let mut errors = dedup_lines(crate::jit_misses::compile_errors(&log));
    let mut warnings = dedup_lines(crate::jit_misses::located_warnings(&log));
    let mut notes = Vec::new();
    if !errors.is_empty() || !warnings.is_empty() {
        let tip_red = red(&run);
        let (new_errors, old_errors) = brought(errors, &tip_red);
        let (new_warnings, old_warnings) = brought(warnings, &tip_red);
        let old = old_errors + old_warnings;
        if old > 0 { notes.push(red_note("test targets", &named, old)); }
        errors = new_errors;
        warnings = new_warnings;
        if errors.is_empty() && warnings.is_empty() { return Ok(notes); }
    }
    let took = format!("{:.1} s", t.elapsed().as_secs_f64());
    if !errors.is_empty() || !warnings.is_empty() {
        return Err(tests_gate_refusal(&named, &run.alone, &took, &errors, &warnings));
    }
    if code == 0 { return Ok(vec![format!("test targets: rustc checked {named} in {took}")]); }
    Ok(vec![format!("test targets: {named} not checked ({}); the fast bucket checks them after the landing", no_verdict(code, &log))])
}

/// The checked pair and scope sent to the train, and the hand-in's lines to print.
#[cfg(feature = "checkout")]
#[derive(Debug)]
pub(crate) struct Gate {
    pub words: Vec<String>,
    pub notes: Vec<String>,
}

/// What a hand-in lands, as its gate checks it: its pinned commits merged onto work and
/// commercial main read as one landed pair ([`landed_tips`]), and the rev that pins that merge.
/// The gate's runs take this rev: at the lane's own pair, a makepad half that had merged work past
/// a two-repo landing beside a commercial half that had not merged main was a pair that never
/// landed together, the master refused its runs, and rustc's check of its test targets was
/// skipped (#5188 beside #5185, 11-15).
#[cfg(feature = "checkout")]
pub(crate) struct Merged {
    /// the tips it merged onto
    pub w: String,
    pub m: String,
    /// the merge: work's side, commercial's (main's tip when the hand-in has no commercial half),
    /// and each other followed nested repository's tip (a half of the hand-in there merged onto it)
    pub tree: super::Pin,
    pub rev: String,
    /// the tips without the hand-in, pinned as `rev` is: a run that fails at the merge is asked
    /// here too, and what it says here is not the hand-in's ([`red_at_tips`])
    pub tips: String,
}

/// Work and commercial main as one landed pair, never read while a landing moves them
/// ([`crate::orch::snapshot`]: the train holds `land-ff.lock` from its first ref to its last):
/// work read just after a two-repo landing moved it and main just before is a pair the master
/// refuses to run. Commercial is empty without a commercial checkout.
#[cfg(feature = "checkout")]
fn landed_tips(root: &Path) -> Result<(String, String), Vec<String>> {
    let cannot = || vec!["cannot read work and commercial main".to_string()];
    if !root.join(crate::orch::ci::COMMERCIAL).join(".git").exists() {
        return crate::gitread::rev(root, "refs/heads/work").map(|w| (w, String::new())).ok_or_else(cannot);
    }
    let lock = makepad_git::repository_paths(root).ok().flatten().ok_or_else(cannot)?.common_dir.join("land-ff.lock");
    let (t, mut said) = (Instant::now(), false);
    loop {
        if let Some(p) = crate::orch::snapshot(root) { return Ok((p.work, p.main)); }
        // no landing moves them: a ref that does not read, or one moved between the reads
        if !lock.exists() { return crate::orch::snapshot(root).map(|p| (p.work, p.main)).ok_or_else(cannot); }
        if !said && t.elapsed() >= Duration::from_secs(5) {
            eprintln!("devhub land: waiting for the landing that is moving work and main");
            said = true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Merge the pinned commits onto the tips ([`Merged`]) as git objects, moving no ref.
#[cfg(feature = "checkout")]
fn merged_onto_tips(root: &Path, lane: &str, rev: &str) -> Result<Merged, Vec<String>> {
    let pin = super::Pin::of_rev(rev).ok_or_else(|| vec![format!("rev `{rev}` is not pinned")])?;
    let (w, m) = landed_tips(root)?;
    let com = root.join(crate::orch::ci::COMMERCIAL);
    let merge = |dir: &Path, base: &str, sha: &str| crate::land::merge_onto(dir, base, sha, &format!("Merge {lane} for the hand-in check"))
        .map_err(|lines| vec![format!("the hand-in does not merge onto work {w:.10} / main {m:.10}: {}; merge the tips into the lane first", lines.join("; "))]);
    let cm = if pin.makepad.is_empty() { w.clone() } else { merge(root, &w, &pin.makepad)? };
    let cc = match pin.nested.get(crate::state::PAIRED) {
        Some(cs) => merge(&com, &m, cs)?,
        None => m.clone(),
    };
    let (mk, nested) = super::parse_pinned(rev).map_err(|e| vec![e])?;
    // Each other followed nested repository at its landed tip ([`crate::nested::tips`]): Source
    // Library's build broke at #7425's landing, its 0 crates checked by a gate that saw makepad and
    // commercial alone (11-19). A hand-in's half in one merges onto that tip, as commercial's does.
    let others = crate::nested::tips(root);
    let tip_of = |name: &str| others.iter().find(|(n, _, _)| n == name).map(|(_, _, sha)| sha.clone());
    let mut merged_rev = format!("{}@{cm}", mk.branch);
    let mut tips = format!("{}@{w}", mk.branch);
    for (name, side) in &nested {
        let (sha, tip) = if name == crate::state::PAIRED { (cc.clone(), m.clone()) } else {
            match (tip_of(name), crate::nested::checkout(root, name)) {
                (Some(tip), Some(dir)) => (merge(&dir, &tip, &side.sha)?, tip),
                _ => (side.sha.clone(), side.sha.clone()),
            }
        };
        merged_rev.push_str(&format!(",{name}={}@{sha}", side.branch));
        tips.push_str(&format!(",{name}={}@{tip}", side.branch));
    }
    // a makepad-only hand-in is checked beside main's tip
    if !cc.is_empty() && !pin.nested.contains_key(crate::state::PAIRED) {
        merged_rev.push_str(&format!(",{}=main@{cc}", crate::state::PAIRED));
        tips.push_str(&format!(",{}=main@{m}", crate::state::PAIRED));
    }
    for (name, branch, sha) in others.iter().filter(|(n, _, _)| !pin.nested.contains_key(n)) {
        merged_rev.push_str(&format!(",{name}={branch}@{sha}"));
        tips.push_str(&format!(",{name}={branch}@{sha}"));
    }
    let tree = super::Pin::of_rev(&merged_rev).ok_or_else(|| vec![format!("the merge `{merged_rev}` is not pinned")])?;
    Ok(Merged { tree, rev: merged_rev, tips, w, m })
}

/// The hand-in's gates, before queueing it: the feature sets and test targets of what it touches
/// ([`feature_gate`]) and the crates it reaches ([`dependents_gate`]), all at the one merge onto
/// the tips it lands as ([`Merged`]).
#[cfg(feature = "checkout")]
pub(crate) fn hand_in_gates(lane: &str, rev: &str) -> (Result<Vec<String>, Vec<String>>, Result<Gate, Vec<String>>) {
    let Some((root, _)) = lane_root() else {
        return (Ok(vec!["feature sets: not checked (not inside a lane worktree)".into()]),
            Ok(Gate { words: vec!["unchecked=no-lane".into()], notes: vec!["dependents: not checked (not inside a lane worktree)".into()] }));
    };
    let shadow = std::sync::OnceLock::new();
    gates(&root, lane, rev, &|run, at| {
        let estimate = estimate_ms(run);
        if let Err(line) = pre_land(run, estimate) { return (2, format!("{line}\n")); }
        let on = shadow.get_or_init(|| retry_master(master).and_then(|c| one(&c, "overlay-shadow", "overlay-shadow enabled")).map(|v| v.get("on").and_then(Value::as_bool) == Some(true)));
        match on {
            Ok(on) => check_run(lane, at, true, run, *on, true, Some(gate_start_bound(estimate)), None),
            Err(e) => (2, format!("check {}: {e}\n", run.packages.join(" "))),
        }
    })
}

/// The longest a hand-in's gate run may take by the master's estimate to run before the landing
/// ([`pre_land`]). A longer one runs after it, in the post-land buckets on the tip: a re-check of
/// 305 crates held 19 hand-ins (11-16), and the user's rule is to land and let the buckets catch
/// what a wide check would ("figure out if its worth running a HUGE check, or if you can just
/// wait for CI"). A cheap one still runs at the merge, beside other work, holding no one.
#[cfg(feature = "checkout")]
const CHEAP_GATE_MS: u64 = 2 * 60_000;

/// How long a hand-in's gate run may wait for its start before it goes after the landing unrun
/// ([`DEFERRED_EXIT`]): its estimate (at most [`CHEAP_GATE_MS`], [`pre_land`]), and never under
/// [`GATE_START_MIN_MS`]; [`CHEAP_GATE_MS`] when no measure says. A gate's estimate counts only
/// its run, so a 1m18s check queued 26 min on a box drained for another ask and held its
/// landing 28 min (L1205988, 11-16): the user's rule is to land and let the buckets check.
#[cfg(feature = "checkout")]
fn gate_start_bound(estimate: Option<u64>) -> Duration {
    Duration::from_millis(estimate.unwrap_or(CHEAP_GATE_MS).max(GATE_START_MIN_MS))
}

/// The least start bound of a hand-in's gate run ([`gate_start_bound`]): a short run's own start
/// bound on its box's short line (30 s, `sched::SHORT_GATE_MS`), which a few seconds' check would
/// otherwise miss on every busy box.
#[cfg(feature = "checkout")]
const GATE_START_MIN_MS: u64 = 30_000;

/// What a gate run let go at its start bound says ([`gate_start_bound`]).
fn deferred_said(id: &str, waited: Duration, within: Duration) -> String {
    format!("{id} had not started {} after it asked (its start bound {}): it goes after the landing, to the post-land buckets", super::sched::dur(waited.as_millis() as u64), super::sched::dur(within.as_millis() as u64))
}

/// Whether `run` runs before the landing, by its estimate in ms (None: no measure says; it runs,
/// and its run is measured): Err, its not-run line, which leaves it to the buckets.
#[cfg(feature = "checkout")]
fn pre_land(run: &crate::jit_misses::CheckRun, estimate: Option<u64>) -> Result<(), String> {
    match estimate.filter(|ms| *ms > CHEAP_GATE_MS) {
        Some(ms) => Err(not_run_line(&format!("check of {}", run.packages.join(" ")), &format!("estimated {}, past the {} {PAST_GATE_BOUND}", super::sched::dur(ms), super::sched::dur(CHEAP_GATE_MS)))),
        None => Ok(()),
    }
}

/// How a gate run past [`CHEAP_GATE_MS`] by its estimate ends its not-run line ([`pre_land`]).
#[cfg(feature = "checkout")]
const PAST_GATE_BOUND: &str = "a hand-in's check may take before it lands";

/// The master's estimate of a gate run (`estimate`), in ms: None when no measure says or the
/// master does not answer.
#[cfg(feature = "checkout")]
fn estimate_ms(run: &crate::jit_misses::CheckRun) -> Option<u64> {
    let c = retry_master(master).ok()?;
    one(&c, "estimate", &format!("test {}", run.class)).ok()?.get("ms")?.as_u64()
}

/// [`hand_in_gates`] with its checker, which receives each run and the merged rev it runs at.
#[cfg(feature = "checkout")]
fn gates(root: &Path, lane: &str, rev: &str, check: &(dyn Fn(&crate::jit_misses::CheckRun, &str) -> (i32, String) + Sync)) -> (Result<Vec<String>, Vec<String>>, Result<Gate, Vec<String>>) {
    let merged = match merged_onto_tips(root, lane, rev) {
        Ok(merged) => merged,
        Err(refused) => return (Ok(Vec::new()), Err(refused)),
    };
    std::thread::scope(|scope| {
        let features = scope.spawn(|| feature_gate(root, rev, &merged, check));
        let dependents = scope.spawn(|| dependents_gate(root, rev, &merged, check));
        (features.join().unwrap_or_else(|_| Err(vec!["feature sets: its gate's thread panicked".into()])),
         dependents.join().unwrap_or_else(|_| Err(vec!["dependents: its gate's thread panicked".into()])))
    })
}

/// The hand-in's feature-set gate (11-15): a module gated behind a default feature broke every
/// dependent that builds the crate without it (#4787, makepad-devhub without `checkout`), and only
/// the buckets said so, after the landing. Each package the pinned commits touch
/// ([`crate::jit_misses::packages_touched`]) is checked with each feature set its dependents use,
/// all at once, at the hand-in's merge onto the tips ([`Merged`]); read from git objects
/// ([`crate::orch::ci::Tree`]), no checkout. The lane that touched the package waits; the landing
/// train never does. Ok: the lines to print (none when nothing is owed); Err: the refusal.
#[cfg(feature = "checkout")]
fn feature_gate(root: &Path, rev: &str, merged: &Merged, check: &(dyn Fn(&crate::jit_misses::CheckRun, &str) -> (i32, String) + Sync)) -> Result<Vec<String>, Vec<String>> {
    let t = Instant::now();
    let pin = super::Pin::of_rev(rev).ok_or_else(|| vec![format!("feature sets: rev `{rev}` is not pinned")])?;
    let paths = crate::jit_misses::touched_paths(root, &pin, true).map_err(|e| vec![format!("feature sets: the paths the hand-in touches: {e}")])?;
    if paths.is_empty() { return Ok(Vec::new()); }
    // (its commercial side is main's tip without a commercial half: scope's agents build
    // makepad-devhub without defaults; each other followed nested repository's is its tip)
    let tree = crate::orch::ci::Tree::at_pin(root, &merged.tree).map_err(|e| vec![format!("feature sets: the merged tree: {e}")])?;
    let touched = crate::jit_misses::packages_touched(&tree, &paths);
    let checks = feature_checks(&touched, &crate::orch::ci::dependent_feature_sets(&tree));
    // and rustc's check of the touched packages' test targets, beside them ([`tests_gate`])
    let tested = crate::jit_misses::test_targets_touched(&tree, &paths);
    let read = t.elapsed();
    if checks.is_empty() && tested.is_empty() { return Ok(Vec::new()); }
    let runs: Vec<crate::jit_misses::CheckRun> = checks.iter().map(|(pkg, set, _)| {
        let own = crate::jit_misses::test_package_at(root, &merged.tree, pkg).map(|p| p.own).unwrap_or(false);
        let flags = set.flags();
        // `makepad check <package> <flags>`: the package first (a flag before it reads as `run`'s)
        let (cmd, alone): (Vec<String>, String) = if own {
            (["cargo", "check", "--release", "--message-format", "short", "-p", pkg].iter().map(|w| w.to_string()).chain(flags.iter().cloned()).collect(), format!("cargo check --release -p {pkg} {}", set.said()))
        } else {
            (["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--", "check", pkg].iter().map(|w| w.to_string()).chain(flags.iter().cloned()).collect(), format!("makepad check {pkg} {}", set.said()))
        };
        let class = format!("check:{pkg}");
        crate::jit_misses::CheckRun { own, packages: vec![pkg.clone()], class: if own { format!("native:{class}") } else { class }, cmd, alone, tests: false }
    }).collect();
    if !checks.is_empty() {
        eprintln!("devhub land: {} feature set(s) of {} to check before the hand-in", checks.len(), touched.iter().filter(|p| checks.iter().any(|(c, _, _)| c == *p)).cloned().collect::<Vec<_>>().join(", "));
    }
    // a run that fails at the merge is asked at the tips too: what is red there is not the hand-in's
    let at_tips = |run: &crate::jit_misses::CheckRun| red_at_tips(run, &merged.tips, check);
    let (ran, red, tests): (Vec<(i32, String)>, Vec<Red>, _) = std::thread::scope(|scope| {
        let tests = scope.spawn(|| tests_gate(&tested, &|run| check(run, &merged.rev), &at_tips));
        let handles: Vec<_> = runs.iter().map(|run| scope.spawn(move || {
            let (code, log) = check(run, &merged.rev);
            let red = if crate::jit_misses::check_failures(&log).is_empty() { Red::new() } else { at_tips(run) };
            ((code, log), red)
        })).collect();
        let (ran, red): (Vec<(i32, String)>, Vec<Red>) = handles.into_iter().map(|h| h.join().unwrap_or_else(|_| ((1, "its check's thread panicked".into()), Red::new()))).unzip();
        (ran, red, tests.join().unwrap_or_else(|_| Err(vec!["test targets: its check's thread panicked".into()])))
    });
    let took = format!("feature sets: {} check(s) in {:.1} s (the manifests read in {:.2} s)", checks.len(), t.elapsed().as_secs_f64(), read.as_secs_f64());
    let features = if checks.is_empty() { Ok(Vec::new()) } else {
        match feature_verdict(&checks, &ran, &red) {
            Ok(mut notes) => { notes.push(took); Ok(notes) }
            Err(mut refused) => { refused.push(took); Err(refused) }
        }
    };
    match (features, tests) {
        (Ok(mut notes), Ok(more)) => { notes.extend(more); Ok(notes) }
        (Err(mut refused), Err(more)) => { refused.extend(more); Err(refused) }
        (Err(refused), Ok(_)) | (Ok(_), Err(refused)) => Err(refused),
    }
}

/// Check the crates a hand-in reaches at its merge onto the tips ([`Merged`]); the checker
/// receives exactly that pair. Reads git objects, moving no ref.
#[cfg(feature = "checkout")]
fn dependents_gate(root: &Path, rev: &str, merged: &Merged, check: &(dyn Fn(&crate::jit_misses::CheckRun, &str) -> (i32, String) + Sync)) -> Result<Gate, Vec<String>> {
    use crate::orch::ci::{in_set, touched_crates, Tree};
    let t = Instant::now();
    let pin = super::Pin::of_rev(rev).ok_or_else(|| vec![format!("dependents: rev `{rev}` is not pinned")])?;
    let paths = crate::jit_misses::touched_paths(root, &pin, true).map_err(|e| vec![format!("dependents: the paths the hand-in touches: {e}")])?;
    let (w, m) = (&merged.w, &merged.m);
    let tree = Tree::at_pin(root, &merged.tree).map_err(|e| vec![format!("dependents: the merged tree: {e}")])?;
    let touched = touched_crates(&tree, &paths);
    let wide = touched.clone(); // Step 5 narrows wide to crates whose interfaces changed.
    let set = in_set(&tree, &touched, &wide);
    let mut words = vec![format!("checked={w},{m}")];
    for (name, packages) in [("touched", &touched), ("wide", &wide), ("set", &set)] {
        words.push(format!("{name}={}", packages.iter().cloned().collect::<Vec<_>>().join(",")));
    }
    let dirs = crate::orch::ci::package_dirs(&tree);
    // the boundary audit of the app, when the hand-in reaches it, beside the set's check
    // ([`boundary_gate`]): its lane waits for both at once
    let audited = dirs.get(BOUNDARY_APP).filter(|_| set.contains(BOUNDARY_APP));
    std::thread::scope(|scope| {
        let audit = audited.map(|dir| scope.spawn(move || boundary_gate(BOUNDARY_APP, dir, &merged.rev, &merged.tips, w, check)));
        let mut said = check_set(&tree, &merged.rev, &merged.tips, &set.iter().cloned().collect::<Vec<_>>(), check);
        // Past the bound by its estimate, the set goes to the buckets, and the touched crates are
        // checked alone, beside it: the hand-in's own errors and warnings refuse it, as when its whole
        // set is checked. Two hand-ins' sets of 28 and 38 crates went unchecked whole, each with its
        // own warning in a touched crate, and the fast bucket after them failed on it for every lane
        // (#6704, #6712, 11-18). The set's crates in the other nested repositories go with them: a fix
        // there is a landing of its own in another repository, and #7425's set of 338 broke Source
        // Library's build unchecked, its fix-forward written without a compile (11-19).
        let elsewhere = |p: &String| dirs.get(p).is_some_and(|d| merged.tree.nested.keys().any(|n| n != crate::state::PAIRED && format!("{d}/").starts_with(&crate::nested::prefix(n))));
        let own: Vec<String> = said.past_bound.iter().filter(|p| touched.contains(*p) || elsewhere(p)).cloned().collect();
        let mut alone_note = None;
        if !own.is_empty() && own.len() < said.past_bound.len() {
            let alone = check_set(&tree, &merged.rev, &merged.tips, &own, check);
            let which = if own.iter().any(elsewhere) { "the crates the hand-in touches and its dependents in other repositories" } else { "the crates the hand-in touches" };
            alone_note = Some(format!("dependents: {} of them checked alone, {which}{}", own.join(", "), if alone.skipped.is_empty() { String::new() } else { format!(" (not run: {})", alone.unchecked) }));
            said.checked += alone.checked;
            said.failed.extend(alone.failed);
            said.red.extend(alone.red);
            if said.node.is_empty() { said.node = alone.node; }
        }
        let mut refused = Vec::new();
        for (named, alone, failures) in said.failed {
            refused.push(format!("dependents: {named} fail at the merge with work {w:.10} / main {m:.10}: {} error and warning line(s); `{alone}` shows it alone", failures.len()));
            refused.extend(failures.into_iter().map(|l| format!("  {l}")));
        }
        let mut notes: Vec<String> = said.skipped.into_iter().map(|(named, why)| format!("dependents: {named} not checked ({why}): the buckets check them after the landing")).collect();
        notes.extend(alone_note);
        notes.extend(said.red.into_iter().map(|(named, n)| red_note("dependents", &named, n)));
        match audit.map(|h| h.join().unwrap_or_else(|_| Err(vec!["boundary: its audit's thread panicked".into()]))) {
            Some(Ok(more)) => notes.extend(more),
            Some(Err(more)) => refused.extend(more),
            None => {}
        }
        let (unchecked, node, checked) = (said.unchecked, said.node, said.checked);
        let took = format!("dependents: {checked} crate(s) checked at the merge with work {w:.10} in {:.1} s", t.elapsed().as_secs_f64());
        if !refused.is_empty() { refused.push(took); return Err(refused); }
        if !unchecked.is_empty() {
            words[0] = format!("unchecked={}", unchecked.chars().map(|c| if c.is_whitespace() || c == '=' { '_' } else { c }).collect::<String>());
        }
        if !node.is_empty() { words.push(format!("node={node}")); }
        notes.push(took);
        Ok(Gate { words, notes })
    })
}

/// The app whose boundary audit a hand-in that reaches it runs at its merge ([`boundary_gate`]):
/// the fast bucket's audit of it after a landing takes about 10 s, and it said #10089 first.
#[cfg(feature = "checkout")]
const BOUNDARY_APP: &str = "makepad-app-calculator";

/// The hand-in's boundary audit of `app` (in `dir`) at its merge (`rev`) onto the tips (`tips`,
/// work's `work`), beside its dependents' check (11-27): `makepad check --boundary all <app>`
/// lowers every body a run of the app compiles and fails on each provided crate's static or
/// rustc-laid field that JIT-compiled code reaches, which the app meets at its first call there.
/// #10089's derive reached a `ScriptHeap` field from JIT code: its crate's tests passed under the
/// JIT, whose test harness builds the provided crates from source, and every app refused to start
/// until a fix landed; the fast bucket's audit said so after the landing. Replayed at #10089's
/// merge, the audit runs on that merge's own JIT, which devhub builds when a pin changes the
/// JIT's inputs (a provided crate's source among them: 2m34s there), and its 18 rows in
/// `ScriptHeap::field_type_from_type_check` refuse it: the lane waits for that build, the queue
/// never does. A failing row the tips
/// already show without the hand-in is not its own; a run without a verdict, or one let go at its
/// start bound, leaves the audit to the buckets after the landing. Ok: its notes; Err: the
/// refusal, with its rows and the line that runs it alone.
#[cfg(feature = "checkout")]
fn boundary_gate(app: &str, dir: &str, rev: &str, tips: &str, work: &str, check: &(dyn Fn(&crate::jit_misses::CheckRun, &str) -> (i32, String) + Sync)) -> Result<Vec<String>, Vec<String>> {
    let t = Instant::now();
    let alone = format!("cd {dir} && makepad check --boundary all {app}");
    let run = crate::jit_misses::CheckRun {
        own: false,
        packages: vec![app.to_string()],
        class: format!("boundary:{app}"),
        cmd: ["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--", "check", "--boundary", "all", app].iter().map(|w| w.to_string()).collect(),
        alone: alone.clone(),
        tests: false,
    };
    let (code, log) = check(&run, rev);
    let rows = boundary_rows(&log);
    if rows.is_empty() {
        return Ok(vec![if code == 0 {
            format!("boundary: {app} audited at the merge with work {work:.10} in {:.1} s: no JIT-compiled body reaches a provided crate's rustc-laid field or static", t.elapsed().as_secs_f64())
        } else {
            format!("boundary: {app} not audited ({}): the buckets audit it after the landing", no_verdict(code, &log))
        }]);
    }
    let red: std::collections::HashSet<String> = boundary_rows(&check(&run, tips).1).into_iter().map(|(key, _)| key).collect();
    let (old, new): (Vec<_>, Vec<_>) = rows.into_iter().partition(|(key, _)| red.contains(key));
    if new.is_empty() {
        return Ok(vec![format!("boundary: {app}: {} failing row(s) already at the tips without the hand-in: not its own, the buckets say them", old.len())]);
    }
    let mut refused = vec![format!("boundary: {app} fails its audit at the merge with work {work:.10}: {} row(s) where JIT-compiled code reaches a provided crate's rustc-laid field or static, which every start of the app meets", new.len())];
    refused.extend(new.into_iter().flat_map(|(_, shown)| shown));
    refused.push(format!("GATEREPRO boundary {app} :: {alone}"));
    Err(refused)
}

/// The failing rows of a boundary audit's output, as land_gate.sh's `BOUNDARY_ROWS` reads them
/// (a `violation` or `option-entry` row marked FAIL): each keyed by what it says without its
/// location (another box's tree, a line that moved), and shown with the line after it, a
/// violation's way in (its `reached` line).
#[cfg(feature = "checkout")]
fn boundary_rows(log: &str) -> Vec<(String, Vec<String>)> {
    // `path:line:col: what` says `what`
    let unlocated = |col: &str| -> String {
        let Some((at, what)) = col.split_once(": ") else { return col.to_string() };
        let mut tail = at.rsplitn(3, ':');
        let located = tail.next().is_some_and(|n| n.parse::<u32>().is_ok()) && tail.next().is_some_and(|n| n.parse::<u32>().is_ok());
        if located { what.to_string() } else { col.to_string() }
    };
    let cut = |l: &str| l.chars().take(600).collect::<String>();
    let lines: Vec<&str> = log.lines().map(|l| l.trim_end_matches('\r')).collect();
    lines.iter().enumerate().filter_map(|(i, line)| {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 3 || !matches!(cols[0], "violation" | "option-entry") || cols[1] != "FAIL" { return None; }
        let key = std::iter::once(cols[0].to_string()).chain(cols[2..].iter().map(|c| unlocated(c))).collect::<Vec<_>>().join("\t");
        let mut shown = vec![format!("  {}", cut(line))];
        if let Some(next) = lines.get(i + 1).filter(|n| n.starts_with([' ', '\t']) && !n.trim().is_empty()) {
            shown.push(format!("    {}", cut(next.trim())));
        }
        Some((key, shown))
    }).collect()
}

/// What a check of a set of crates said ([`check_set`]), run by run.
#[cfg(feature = "checkout")]
struct SetVerdict {
    /// (the run's crates, the command that shows it alone, its error and warning lines)
    failed: Vec<(String, String, Vec<String>)>,
    /// (the run's crates, why it did not run)
    skipped: Vec<(String, String)>,
    /// (the run's crates, how many of its lines were already red at the tips: not the hand-in's)
    red: Vec<(String, usize)>,
    /// the first reason a run did not run (empty: all ran)
    unchecked: String,
    /// the crates of the runs past the hand-in's bound by their estimate ([`PAST_GATE_BOUND`])
    past_bound: Vec<String>,
    /// the box the JIT's run ran on
    node: String,
    /// the crates whose run ran
    checked: usize,
}

/// The crates of `set` checked at `rev`, the merge they land as (`tree`, that merge's tree): one
/// run per checker ([`crate::jit_misses::check_runs`]), each once more when it ended without an
/// error line (it did not run), and at `tips` too when it failed ([`red_at_tips`]). The hand-in's
/// dependents gate ([`dependents_gate`]) takes it.
#[cfg(feature = "checkout")]
fn check_set(tree: &crate::orch::ci::Tree, rev: &str, tips: &str, set: &[String], check: &(dyn Fn(&crate::jit_misses::CheckRun, &str) -> (i32, String) + Sync)) -> SetVerdict {
    // which checker each takes, from the merged tree read once (a read per package, as
    // `test_package` does, took 0.5 s each: minutes for a platform's closure)
    let (dirs, own) = (crate::orch::ci::package_dirs(tree), crate::orch::ci::jit_own_dirs(tree));
    let sides: Vec<(String, bool)> = set.iter().map(|pkg| (pkg.clone(), dirs.get(pkg).is_some_and(|d| own.contains(d)))).collect();
    // Dependents stay plain-only here (CHEAP_GATE_MS). The fast bucket's step_testcheck
    // checks the reached dependents' test targets after landing.
    let runs = crate::jit_misses::check_runs(&sides);
    let ran: Vec<(i32, String, Red)> = std::thread::scope(|scope| {
        let handles: Vec<_> = runs.iter().map(|run| scope.spawn(move || {
            let (code, log) = check(run, rev);
            // (a run let go at its start bound goes after the landing: never asked again before it)
            let (code, log) = if code == 0 || code == DEFERRED_EXIT || !crate::jit_misses::check_failures(&log).is_empty() { (code, log) } else { check(run, rev) };
            let red = if crate::jit_misses::check_failures(&log).is_empty() { Red::new() } else { red_at_tips(run, tips, check) };
            (code, log, red)
        })).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| (1, "its check's thread panicked".into(), Red::new()))).collect()
    });
    let mut said = SetVerdict { failed: Vec::new(), skipped: Vec::new(), red: Vec::new(), unchecked: String::new(), past_bound: Vec::new(), node: String::new(), checked: 0 };
    for (run, (code, log, red)) in runs.into_iter().zip(ran) {
        let named = run.packages.join(", ");
        let (failures, old) = brought(crate::jit_misses::check_failures(&log), &red);
        if old > 0 { said.red.push((named.clone(), old)); }
        if let Some(on) = log.lines().find_map(|l| l.strip_prefix("devhub test: ")?.split_once(" passed in ")?.1.rsplit_once(" on ")?.1.split_whitespace().next()) {
            if said.node.is_empty() || !run.own { said.node = on.to_string(); }
        }
        if !failures.is_empty() {
            said.checked += run.packages.len();
            said.failed.push((named, run.alone, failures));
        } else if code == 0 || old > 0 {
            said.checked += run.packages.len();
        } else {
            let why = no_verdict(code, &log);
            if said.unchecked.is_empty() { said.unchecked = why.clone(); }
            if why.ends_with(PAST_GATE_BOUND) { said.past_bound.extend(run.packages.iter().cloned()); }
            said.skipped.push((named, why));
        }
    }
    said
}

/// A passing test also checks the plain builds touched by its diff (#4005, #4024: a function
/// called only under cfg(test) is dead in the shipping build), and touched test targets
/// ([`crate::jit_misses::touched_pre_land`]). Skip the latter only for the package whose full
/// default-feature test set just ran. Both checks fail on a source warning or error.
fn pre_land_checks(lane: &str, rev: &str, commits_only: bool, placed: &Req, tested_in_full: Option<&str>) -> i32 {
    let Some((root, _)) = lane_root() else { return 0 };
    let Some(pin) = super::Pin::of_rev(rev) else {
        eprintln!("devhub test: rev `{rev}` is not pinned: the packages it touches are not checked");
        return 2;
    };
    pre_land_checks_at(&root, &pin, commits_only, tested_in_full, || {
        let reply = retry_master(master).and_then(|c| one(&c, "overlay-shadow", "overlay-shadow enabled"))?;
        let shadow = reply.get("on").and_then(Value::as_bool) == Some(true);
        Ok(move |run: &crate::jit_misses::CheckRun| check_run(lane, rev, commits_only, run, shadow, false, None, Some(placed)))
    }, |line| println!("{line}"))
}

/// Prepare the checker once, after discovering runnable targets. A package with only test
/// edits may already have run in full and still needs the harness scan.
fn pre_land_checks_at<C>(
    root: &Path, pin: &super::Pin, commits_only: bool, tested_in_full: Option<&str>,
    prepare: impl FnOnce() -> Result<C, String>, mut report: impl FnMut(&str),
) -> i32 where C: Fn(&crate::jit_misses::CheckRun) -> (i32, String) + Sync {
    let (plain, tests) = match crate::jit_misses::touched_pre_land(root, pin, commits_only) {
        Ok(t) => t,
        Err(e) => return fail(format!("devhub test: the packages the lane's diff touches: {e}")),
    };
    if plain.is_empty() && tests.is_empty() { return 0; }
    let touched: Vec<_> = plain.iter().chain(&tests).collect::<std::collections::BTreeSet<_>>().into_iter().cloned().collect();
    // a #[test] in a `harness = false` target compiles out: it never runs and nothing says so
    let silent = match crate::testlog::silent_tests_at(root, pin, &touched) {
        Ok(lines) => lines,
        Err(e) => return fail(format!("devhub test: the touched packages' harness = false targets: {e}")),
    };
    for line in &silent { report(&format!("devhub test: {line}")); }
    let (mut plain_sides, mut test_sides) = (Vec::new(), Vec::new());
    for pkg in touched {
        let check_plain = plain.binary_search(&pkg).is_ok();
        let check_tests = tested_in_full != Some(pkg.as_str()) && tests.binary_search(&pkg).is_ok();
        if !check_plain && !check_tests { continue; }
        let own = match crate::jit_misses::test_package_at(root, pin, &pkg) {
            Ok(p) => p.own,
            Err(e) => return fail(format!("devhub test: check {pkg}: {e}")),
        };
        if check_plain { plain_sides.push((pkg.clone(), own)); }
        if check_tests { test_sides.push((pkg, own)); }
    }
    let mut runs = crate::jit_misses::check_runs(&plain_sides);
    runs.extend(crate::jit_misses::check_tests_runs(&test_sides));
    let mut worst = i32::from(!silent.is_empty());
    if runs.is_empty() { return worst; }
    let check = match prepare() {
        Ok(check) => check,
        Err(e) => return fail(e),
    };
    eprintln!("devhub test: checking the lane's touched packages in {} run{}; a warning fails the check", runs.len(), if runs.len() == 1 { "" } else { "s" });
    let ran: Vec<(i32, String)> = std::thread::scope(|scope| {
        let check = &check;
        let handles: Vec<_> = runs.iter().map(|run| scope.spawn(move || check(run))).collect();
        handles.into_iter().map(|h| h.join().unwrap_or_else(|_| (1, "its check's thread panicked".into()))).collect()
    });
    for (run, (code, log)) in runs.iter().zip(ran) {
        let failures = crate::jit_misses::check_failures(&log);
        let label = format!("check of {} {}", run.packages.join(", "), if run.tests { "(--tests)" } else { "without their tests" });
        if code == 0 && failures.is_empty() {
            report(&format!("devhub test: {label}: ok, 0 warnings"));
            continue;
        }
        // its lease never started: no verdict on the crates, said as that
        if let Some(said) = not_run(&log).filter(|_| code != 0) {
            report(&format!("devhub test: {label} {NOT_RUN}{said} (exit {code}): no verdict on them; run it again"));
            worst = 1;
            continue;
        }
        if failures.is_empty() {
            for l in log.lines().rev().take(20).collect::<Vec<_>>().into_iter().rev() { report(l); }
        }
        for l in &failures { report(l); }
        report(&format!("devhub test: {label} FAILED: {} (exit {code}); `{}` shows it alone", if failures.is_empty() { "no error line".to_string() } else { format!("{} error and warning line(s)", failures.len()) }, run.alone));
        worst = 1;
    }
    worst
}

/// The warnings of a JIT test run's output, each with whether the lane changed its file
/// ([`crate::orch::live::round_warnings`], in this shell's lane).
#[cfg(feature = "checkout")]
fn run_warnings(log: &str) -> Vec<(String, bool)> {
    lane_root().map_or_else(Vec::new, |(root, _)| crate::orch::live::round_warnings(log, &root))
}

/// (a build without the lane's orchestration reads no lane: no warning is the lane's)
#[cfg(not(feature = "checkout"))]
fn run_warnings(_: &str) -> Vec<(String, bool)> {
    Vec::new()
}

/// A JIT test run's rustc warnings (each with whether the lane changed its file,
/// [`crate::orch::live::round_warnings`]): the lines that end its output, and its exit. Each crate's
/// build says its own among the run's output (`relax test: 1 warning` among ten targets' `0
/// warnings`), and a run that passed with one in a file the lane changed fails: a validated
/// revision has none (AGENTS.md), and the fast bucket after the landing fails on it for every
/// lane. Two lanes' whole-package runs passed so, each with its own warning in its output, and
/// both landed it (#6704, #6712, 11-18).
fn warnings_verdict(warned: &[(String, bool)], code: i32) -> (Vec<String>, i32) {
    if warned.is_empty() { return (Vec::new(), code); }
    let mine = warned.iter().filter(|(_, m)| *m).count();
    let mut lines = vec![format!("devhub test: {} warning(s), {mine} in files this lane changed", warned.len())];
    lines.extend(warned.iter().map(|(l, m)| format!("  {}{l}", if *m { "[this lane] " } else { "" })));
    if code != 0 || mine == 0 { return (lines, code); }
    lines.push(format!("devhub test: FAILED: {mine} warning(s) in files this lane changed: a validated revision has none, and the fast bucket fails on them after the landing"));
    (lines, 1)
}

/// What a test run whose lease never started says in place of its verdict, before why.
const NOT_RUN: &str = "not run: ";

/// A test run's line in place of its verdict when its lease never started.
fn not_run_line(pkg: &str, said: &str) -> String {
    format!("devhub test: {pkg} {NOT_RUN}{said}")
}

/// Why a test run's lease never started, from its output ([`not_run_line`]): None when it did.
fn not_run(log: &str) -> Option<&str> {
    log.lines().find_map(|l| l.strip_prefix("devhub test: ")?.split_once(&format!(" {NOT_RUN}")).map(|(_, why)| why))
}

/// Why a check run that neither refused nor passed has no verdict: the why of its not-run line,
/// else its exit by name. A lease lost under the run said `exit 4, no error line`, its reason on
/// the hand-in's stderr only (11-15).
#[cfg(feature = "checkout")]
fn no_verdict(code: i32, log: &str) -> String {
    match not_run(log).filter(|why| !why.is_empty()) {
        Some(why) => why.to_string(),
        None if code == LOST_EXIT => format!("exit {LOST_EXIT}: its lease was lost under it, no verdict"),
        None if code == CANCELLED_EXIT => format!("exit {CANCELLED_EXIT}: cancelled"),
        None => format!("exit {code}, no error line"),
    }
}

/// One run of the pre-land check at `rev` ([`pre_land_checks`]), or of a hand-in's gates
/// (`hand_in`: [`hand_in_gates`]): its exit and output.
fn check_run(lane: &str, rev: &str, commits_only: bool, run: &crate::jit_misses::CheckRun, shadow: bool, hand_in: bool, start_within: Option<Duration>, placed: Option<&Req>) -> (i32, String) {
    let named = run.packages.join(" ");
    let mut o = match parse("test", Some(lane), &[]) {
        Ok(o) => o,
        Err(e) => return (2, format!("check {named}: {e}\n")),
    };
    // a test's check goes where the test went: on the box it names (a timed series, a home's
    // files), on its platform, off the boxes it avoids. Placed anywhere, a timed arm's check read
    // "test@<another box>" in its log, as if the arm had left its box (L1351943, 11-18)
    if let Some(r) = placed {
        (o.req.node, o.req.series, o.req.pin_why, o.req.avoid, o.req.needs) = (r.node.clone(), r.series.clone(), r.pin_why.clone(), r.avoid.clone(), r.needs.clone());
    }
    (o.req.rev, o.commits_only, o.req.class, o.cmd) = (Some(rev.to_string()), commits_only, run.class.clone(), run.cmd.clone());
    o.req.hand_in = hand_in;
    o.start_within = start_within;
    let c = match retry_master(master) {
        Ok(c) => c,
        Err(e) => return (2, format!("check {named}: {e}\n")),
    };
    let mut out = Vec::new();
    let name = format!("check of {named}");
    let code = if run.own {
        o.tree = true;
        if let Err(code) = native_gate(&c, &mut o) { return (code, format!("check {named}: the master refused its cargo check\n")); }
        run_worktree_into(&mut o, false, Some(&name), &mut out).0
    } else if shadow {
        shadow_test(&c, &mut o, &name, &mut out).0
    } else {
        run_worktree_into(&mut o, false, Some(&name), &mut out).0
    };
    (code, String::from_utf8_lossy(&out).into_owned())
}

/// Submit the requested source mode; the master snapshots once and owns the opposite-mode shadow.
/// Capture only the requested run; the second result distinguishes completion from setup/link failure.
fn shadow_test(c: &Client, o: &mut Opts, pkg: &str, output: &mut Vec<u8>) -> (i32, bool) {
    let mut run = || -> Result<(i32, bool), String> {
        use makepad_remote::json_str;
        let (root, rel) = lane_root().ok_or("test: run inside the lane's worktree")?;
        let root = root.to_string_lossy().into_owned();
        let optional = |n: Option<f64>| n.map_or("null".into(), |n| n.to_string());
        let body = format!("{{\"tree\":{},\"also\":[{}],\"jobs\":[{{\"kind\":\"test\",\"holder\":{},\"lane\":{},\"agent\":{},\"cmd\":[{}],\"cwd\":{},\"shadow\":true,\"tree\":{},\"cost\":{},\"est_min\":{},\"cores\":{},\"ram_gb\":{},\"whole\":{},\"class\":{},\"priority\":{},\"loader\":{},\"needs\":{},\"node\":{},\"prio\":{}{}}}]}}",
            json_str(&root), also_named().iter().map(|p| json_str(p)).collect::<Vec<_>>().join(","), std::process::id(), json_str(&o.req.lane), json_str(o.req.agent.as_deref().unwrap_or("devhub-test")), o.cmd.iter().map(|s| json_str(s)).collect::<Vec<_>>().join(","), json_str(rel.trim_end_matches('/')),
            o.tree, json_str(if o.req.cost_asked { o.req.cost.as_str() } else { "" }), optional(o.req.est_min), optional(o.req.cores), optional(o.req.ram_gb), o.req.whole, json_str(&o.req.class), json_str(o.req.priority.as_str()), json_str(o.req.loader.as_deref().unwrap_or("")),
            json_str(&o.req.needs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")), json_str(o.req.node.as_deref().unwrap_or("")), o.req.prio, if o.req.hand_in { ",\"hand_in\":true" } else { "" });
        let request = crate::proto::request_id();
        // `--rev`: the master runs the job at those commits alone
        let rev = if o.commits_only { format!("\"rev\":{},", json_str(o.req.rev.as_deref().unwrap_or(""))) } else { String::new() };
        let line = format!("job submit {{\"request_id\":\"{request}\",{rev}{}", &body[1..]);
        let v = retry_master(|| one(c, "job", &line))?;
        let id = v.get("jobs").and_then(Value::as_arr).and_then(|a| a.first()).and_then(Value::as_str).ok_or("test: no job")?;
        let (mut off, mut generation) = (0, (String::new(), 0));
        let mut told = String::new();
        let asked = Instant::now();
        loop {
            let v = retry_master(|| one(c, "job", &format!("job output {id} {off}")))?;
            let j = crate::state::job_from(v.get("job").ok_or("test: missing job")?);
            if let Some(within) = o.start_within.filter(|w| j.state == "queued" && asked.elapsed() >= *w) {
                let said = deferred_said(id, asked.elapsed(), within);
                let _ = one(c, "job", &format!("job cancel {id}"));
                eprintln!("test: {said}");
                let _ = writeln!(output, "{}", not_run_line(pkg, &said));
                return Ok((DEFERRED_EXIT, false));
            }
            // what its lease waits for, said as it changes: a lane's tests waited behind its own
            // screen lease ("one lease at a time in its tree"), said only in the queue (11-15)
            if waits_said(&j, &mut told) { eprintln!("test: {told}"); }
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
                let failed = (!matches!(code, 0 | CANCELLED_EXIT)).then(|| crate::testlog::Tally::of(&String::from_utf8_lossy(&output)).failed_said());
                let _ = writeln!(output, "devhub test: {pkg} {} in {:.3}s (queue {:.3}s, ship {:.3}s, run {:.3}s) on {}", if code == 0 { "passed" } else if code == CANCELLED_EXIT { "cancelled" } else { "failed" }, seconds(j.submitted_ms, j.ended_ms), seconds(j.submitted_ms, j.granted_ms), seconds(j.granted_ms, j.shipped_ms), seconds(j.shipped_ms, j.ended_ms), j.node);
                if let Some(said) = failed { let _ = write!(output, "{said}its whole output: `devhub job result {id}`\n"); }
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
    finish(&c, &held, r, None, false)
}

/// A live test's arguments with its feature flags said one way (`--no-default-features`, then
/// `--features` sorted and comma-joined, after its target): they are its session's key
/// ([`live_test`]), so a feature set keeps one session and two sets never share one. A test that
/// needs its package's features could not be iterated live, and ran on the pre-land path or
/// `--native` instead (a selection test needing `packs scene3d`, 11-13).
fn live_select(args: Vec<String>) -> Vec<String> {
    let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
    let (select, rest) = args.split_at(end);
    let (mut out, mut features, mut no_default) = (Vec::new(), std::collections::BTreeSet::new(), false);
    let mut i = 0;
    while i < select.len() {
        match select[i].as_str() {
            "--features" => {
                if let Some(v) = select.get(i + 1) { features.extend(v.split([',', ' ']).filter(|f| !f.is_empty()).map(str::to_string)); }
                i += 1;
            }
            "--no-default-features" => no_default = true,
            a => out.push(a.to_string()),
        }
        i += 1;
    }
    if no_default { out.push("--no-default-features".into()); }
    if !features.is_empty() { out.extend(["--features".to_string(), features.into_iter().collect::<Vec<_>>().join(",")]); }
    out.extend(rest.iter().cloned());
    out
}

/// Keep reconnect notices in the target's block as well as its live stream. Each attempt asks on
/// a client `dial` gives: after a cut, a new link to the master, never the cut one, whose every
/// ask was cut again at once and went round with nothing said. Two of a lane's test reruns said
/// "the link was cut" at a master's restart, then nothing for 15 min until their lane killed
/// them (11-27). `beat` waits a while and says whether the master answers.
fn live_retry<C, T>(out: &mut dyn Write, mut dial: impl FnMut() -> Result<C, String>, mut call: impl FnMut(&C, &mut dyn Write) -> Result<T, String>, mut beat: impl FnMut() -> bool) -> Result<(T, C), String> {
    let mut told = None;
    loop {
        match dial().and_then(|c| call(&c, out).map(|v| (v, c))) {
            Ok(v) => return Ok(v),
            Err(e) if e.starts_with("no devhub master at ") || crate::cli::link_cut(&e) => {
                let up = beat();
                if told != Some(up) {
                    let _ = writeln!(out, "{}", match (told, up) {
                        (_, false) => "master not reachable: waiting",
                        (None, true) => "the link was cut: asking again",
                        (Some(_), true) => "the master is back: asking again",
                    });
                    told = Some(up);
                }
            }
            Err(e) => return Err(e),
        }
    }
}

/// [`live_retry`]'s wait between attempts: whether the master answers after it.
fn master_beat() -> bool {
    std::thread::sleep(Duration::from_secs(2));
    control().is_ok()
}

/// One ordinary live request, buffering its notices; the caller renders the returned verdict.
fn live_test(o: &Opts, pkg: &str, args: &[String], offline: bool, check: &str, out: &mut dyn Write) -> Result<(crate::state::Live, Client, Option<u64>), String> {
    let started = Instant::now();

    let (root, _) = lane_root().ok_or("test --live: run it inside the lane's worktree")?;
    let mut dialed = Some(live_retry(out, master, |_, _| Ok(()), master_beat)?.1);
    let loader = o.req.loader.clone().unwrap_or_default();
    let needs = o.req.needs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",");
    let sandbox = if offline { ",\"offline\":true" } else { "" };
    // (`live start --env`'s pairs: a session asked with other env, or another JIT, is no
    // session of this call's, orch/live.rs `answers`)
    let env = o.env.iter().map(|(k, v)| format!("[{},{}]", makepad_remote::json_str(k), makepad_remote::json_str(v))).collect::<Vec<_>>().join(",");
    let sandbox = format!("{sandbox},\"env\":[{env}],\"check\":{}{}", makepad_remote::json_str(check), if o.fresh { ",\"fresh\":true" } else { "" });
    // placed as a lease is: --node (with --series/--pin-why), --needs, --avoid
    let placement = format!(",\"node\":{},\"avoid\":{},\"series\":{},\"pin_why\":{}", makepad_remote::json_str(o.req.node.as_deref().unwrap_or("")), makepad_remote::json_str(&o.req.avoid.join(",")), makepad_remote::json_str(o.req.series.as_deref().unwrap_or("")), makepad_remote::json_str(o.req.pin_why.as_deref().unwrap_or("")));
    let req = format!("{{\"app\":{},\"repo_dir\":{},\"tree\":{},\"lane\":{},\"agent\":{},\"needs\":{}{placement},\"priority\":{},\"loader\":{},\"cost\":{},\"est_min\":{},\"class\":{},\"also\":[{}],\"test_args\":[{}]{sandbox}}}", makepad_remote::json_str(pkg), makepad_remote::json_str(&root.to_string_lossy()), o.tree, makepad_remote::json_str(&o.req.lane), makepad_remote::json_str(o.req.agent.as_deref().unwrap_or("")), makepad_remote::json_str(&needs), makepad_remote::json_str(o.req.priority.as_str()), makepad_remote::json_str(&loader), makepad_remote::json_str(o.req.cost.as_str()), o.req.est_min.map_or("null".to_string(), |m| m.to_string()), makepad_remote::json_str(&o.req.class), std::env::var("DEVHUB_ALSO").unwrap_or_default().split_whitespace().map(makepad_remote::json_str).collect::<Vec<_>>().join(","), args.iter().map(|a| makepad_remote::json_str(a)).collect::<Vec<_>>().join(","));
    let selected = if args.last().is_some_and(|a| a == "--") { &args[..args.len() - 1] } else { args };
    // the session, and what devhub tells its owner of it (its call far past its measure)
    // (and how long its run has been silent)
    let status = |c: &Client| -> Option<(crate::state::Live, Option<String>, Option<u64>, Option<String>)> {
        let v = one(c, "live", "live list").ok()?;
        v.as_arr()?.iter().map(|v| (crate::state::live_from(v), v.get("word").and_then(Value::as_str).map(str::to_string), v.get("quiet_ms").and_then(Value::as_u64), v.get("no_progress").and_then(Value::as_str).map(str::to_string))).find(|(x, _, _, _)| x.test && x.offline == offline && x.lane == o.req.lane && x.app == pkg && x.test_args == selected)
    };
    let measured_ms = dialed.as_ref().and_then(|c| status(c)).and_then(|(x, _, _, _)| x.patch).and_then(|p| p.tests).map_or(0, |t| t.ms.max(0.0) as u64);
    let mut notice_after = Duration::from_millis(measured_ms.saturating_add(crate::state::STARTUP_MARGIN_MS));
    let mut told = String::new();
    // (the first attempt asks on the client dialed above; one after a cut on a new one)
    let (v, c) = live_retry(out, || dialed.take().map_or_else(master, Ok), |c, out| {
        // Keep the request open without a deadline. A separate channel reads its phase
        // once it exceeds the previous test time plus two minutes.
        let ch = c.open("live", "").map_err(|e| e.to_string())?;
        ch.send(format!("live test {req}").into_bytes()).map_err(|e| format!("link lost: {e}"))?;
        let mut answer = None;
        loop {
            match ch.recv_timeout(Duration::from_secs(1)) {
                Ok(makepad_remote::Event::Data(d)) => {
                    for l in String::from_utf8_lossy(&d).lines().filter(|l| !l.is_empty()) {
                        if let Some(v) = live_line(l, &mut answer, out)? { return Ok(v); }
                    }
                }
                Ok(makepad_remote::Event::Opened(_)) | Err(true) => {}
                Ok(makepad_remote::Event::Closed(makepad_remote::conn::CLOSE_LINK_LOST, why)) => return Err(format!("link lost: {why}")),
                Ok(makepad_remote::Event::Closed(_, why)) => return Err(why),
                Err(false) => return Err("the hub connection ended".into()),
            }
            if started.elapsed() > notice_after {
                let (x, word, quiet, stuck) = status(c).map_or((None, None, None, None), |(x, w, q, n)| (Some(x), w, q, n));
                // (a session whose run has not started for this call waits for its JIT: selected,
                // built, or another lease's build of it, however long that takes, live.rs startup_due)
                let jit_wait = x.as_ref().filter(|x| x.state == "running" && x.pid == 0 && x.run_n <= x.ask_after)
                    .map(|x| format!("waiting for {}'s JIT build on {} ({} s)", x.lane, x.node, started.elapsed().as_secs()));
                let phase = match x.as_ref() {
                    None => "waiting for a live job",
                    Some(x) if x.state != "running" => x.state.as_str(),
                    Some(x) if x.pid == 0 && x.note.starts_with("no startup line read") => x.note.as_str(),
                    Some(_) if jit_wait.is_some() => jit_wait.as_deref().unwrap_or_default(),
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
                // a run silent past a minute says since when: a hung one is seen as one, and a call
                // whose edit makes no progress says that with its evidence (orch/live.rs no_progress)
                let silent = match stuck {
                    Some(why) => format!("; {why}"),
                    None => quiet.filter(|ms| *ms >= crate::state::QUIET_SAID_MS).map_or(String::new(), |ms| format!("; last output {} ago", crate::state::ago(ms))),
                };
                let _ = writeln!(out, "still running: {phase}{note} ({} min{silent})", started.elapsed().as_secs() / 60);
                if let Some(word) = word.filter(|w| *w != told) {
                    let _ = writeln!(out, "devhub: {word}");
                    told = word;
                }
                notice_after = started.elapsed() + STILL_RUNNING_EVERY;
            }
        }
    }, master_beat)?;
    let x = crate::state::live_from(&v);
    // an answer that names no session printed `devhub test:  : ` and exit 1, unexplained
    // (11-13): what came instead is said
    if x.id.is_empty() { return Err(format!("test --live: the master answered with no session: {}", crate::proto::clean(&v.to_json(), 400))); }
    Ok((x, c, v.get("cached_ms").and_then(Value::as_u64)))
}

/// A live test's verdict as said: one its session answered from its last round, nothing having
/// changed since (`cached`, that round's verdict time), says so on its first line, and how to run
/// again. Four of a lane's five reruns in a flake hunt read their session's last pass as theirs
/// (11-26).
fn cached_said(result: &str, cached: Option<u64>) -> String {
    let Some(ms) = cached else { return result.to_string() };
    let at = ms / 1000 % 86_400;
    let said = format!("devhub test (cached: the verdict of its round at {:02}:{:02}:{:02}Z, nothing changed since and nothing ran; --fresh runs the tests again): ", at / 3600, at / 60 % 60, at % 60);
    match result.find("devhub test: ") {
        Some(i) => format!("{}{said}{}", &result[..i], &result[i + "devhub test: ".len()..]),
        None => format!("{}\n{result}", said.trim_end_matches(": ")),
    }
}

/// Keep the test connection until cargo gives its verdict. Its loss tells the master that
/// this refusal has no caller left to confirm it.
fn single_live_test(mut o: Opts, pkg: &str, args: &[String], inputs: MissInputs, offline: bool) -> i32 {
    let started = Instant::now();
    let check = crate::proto::request_id();
    let (x, client, cached) = match live_test(&o, pkg, args, offline, &check, &mut std::io::stderr()) {
        Ok(result) => result,
        Err(e) => return fail(e),
    };
    let held = |diagnostic: &str| println!("held: {diagnostic}: session {} keeps its lease; your next `devhub test {} {pkg} --live` after the fix reattaches to it", x.id, x.lane);
    if x.active() && !x.held.is_empty() {
        print!("{}", x.test_result(started.elapsed().as_secs_f64()));
        held(&x.held);
        return 1;
    }
    let errors = x.compile_errors();
    if let Some(p) = x.patch.as_ref().filter(|_| !errors.is_empty()) {
        let release = || retry_master(|| master().and_then(|c| one(&c, "live", &format!("live release {}", x.id))));
        if offline {
            if let Err(e) = release() { return fail(e); }
            return fail(format!("an offline test runs only as a JIT live session; cargo would follow the live session's {}: {}", p.outcome, errors.join("; ")));
        }
        o.req.rev = Some(x.rev.clone());
        let said = format!("{}\n{}", p.reason, p.output);
        let (code, confirmed) = cargo_test(o, pkg, args, &errors, None, Some(inputs), Some(&said));
        if let Some(refusal) = x.refusal_key() {
            let verdict = if confirmed { "confirmed" } else { "release" };
            match retry_master(|| one(&client, "live", &format!("live checked {} {check} {refusal} {verdict}", x.id))) {
                Ok(v) => {
                    if let Some(diagnostic) = v.get("held").and_then(Value::as_str).filter(|s| !s.is_empty()) { held(diagnostic); }
                }
                // The old live service lists its verbs in parentheses. one() may prepend the build details.
                Err(e) if e.contains("live: unknown word `checked` (") => if let Err(e) = release() { return fail(e); },
                Err(e) => return fail(e),
            }
        }
        return code;
    }
    print!("{}", cached_said(&x.test_result(started.elapsed().as_secs_f64()), cached));
    let warned = x.patch.as_ref().and_then(|p| p.tests.as_ref()).map_or(&[][..], |t| &t.warnings[..]);
    let (lines, code) = warnings_verdict(warned, x.test_code());
    for l in lines.into_iter().filter(|l| l.contains("FAILED")) { println!("{l}"); }
    code
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
    fetch_keep_from(&|service, line| ask(c, Some(node), service, line), node, lane, lease, dst)
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
pub(super) fn keep_said(id: &str, dst: &Path, r: &Result<Vec<(PathBuf, u64)>, String>) -> String {
    match r {
        Ok(files) if files.is_empty() => format!("{id}: kept nothing in {}: the run put no file in $DEVHUB_KEEP", dst.display()),
        Ok(files) => format!("{id}: kept {} ({:.1} MB) in {}", super::master::plural(files.len(), "file", "files"), files.iter().map(|(_, n)| n).sum::<u64>() as f64 / 1048576.0, dst.display()),
        Err(e) => format!("{id}: --keep {}: nothing kept: {e}", dst.display()),
    }
}

/// Resolve the destination once, including symlinks in its longest existing ancestor.
pub(crate) fn abs_keep_path(dst: &Path, cwd: &Path) -> PathBuf {
    // `..` and `.` lexically, then symlinks in the part that exists (the rest is made by the copy)
    let mut abs = PathBuf::new();
    for c in cwd.join(dst).components() {
        match c {
            std::path::Component::ParentDir => { abs.pop(); }
            std::path::Component::CurDir => {}
            c => abs.push(c),
        }
    }
    for ancestor in abs.ancestors() {
        if let Ok(real) = ancestor.canonicalize() {
            let rest = abs.strip_prefix(ancestor).unwrap();
            return if rest.as_os_str().is_empty() { real } else { real.join(rest) };
        }
    }
    abs
}

/// Why files kept for lane `lane` (`--keep <dir>`, `get .. <local-dir>`) may not go to `dst`
/// (relative to `cwd`): it lies in a checkout, a lane's worktree or a repo's own, where a kept
/// file can be committed or read as a build's input. Only the main checkout's `local/` takes them,
/// and the refusal names the lane's `local/agent_state/<lane>/runs/` there. A lane's `run --keep
/// apps/apps/fab/app/target/makepad_test/..`, relative in its worktree, left a `target/` in the
/// app's folder (11-26).
pub(super) fn kept_into_checkout(dst: &Path, cwd: &Path, lane: &str) -> Option<String> {
    let real = abs_keep_path(dst, cwd);
    let root = real.ancestors().find(|a| a.join(".git").exists())?;
    let dot = root.join(".git");
    if dot.is_dir() && real.strip_prefix(root).ok()?.starts_with("local") { return None; }
    // a linked worktree's main checkout: its gitdir's common dir's parent
    let gitdir = if dot.is_dir() { dot } else { root.join(std::fs::read_to_string(&dot).ok()?.trim().strip_prefix("gitdir:")?.trim()) };
    let common = std::fs::read_to_string(gitdir.join("commondir")).ok().map_or(gitdir.clone(), |c| gitdir.join(c.trim()));
    let main = common.canonicalize().ok().and_then(|c| c.parent().map(Path::to_path_buf)).unwrap_or_else(|| root.to_path_buf());
    // a nested repository's checkout (`apps/<name>`): the makepad checkout around it ([`lane_root_of`])
    let main = main.parent().filter(|apps| apps.file_name().is_some_and(|a| a == "apps")).and_then(Path::parent).filter(|outer| outer.join(".git").is_dir()).map_or(main.clone(), Path::to_path_buf);
    Some(format!("{} is in the checkout {}: a file kept there can be committed or read as a build's input; keep it under {}/local/agent_state/{lane}/runs/", dst.display(), root.display(), main.display()))
}

/// [`fetch`]'s copy, none an empty list; `inside`: `rel`'s own folder is left out of the paths
/// written under `dst`.
fn fetch_under(c: &Client, node: &str, lane: &str, lease: &str, rel: &str, dst: &Path, inside: bool) -> Result<Vec<(PathBuf, u64)>, String> {
    fetch_from(&|service, line| ask(c, Some(node), service, line), node, lane, lease, rel, dst, inside)
}

/// The same lease-file protocol through either a client router or the master's node link.
pub(super) fn fetch_from(ask: &dyn Fn(&str, &str) -> Result<Vec<String>, String>, node: &str, lane: &str, lease: &str, rel: &str, dst: &Path, inside: bool) -> Result<Vec<(PathBuf, u64)>, String> {
    fetch_from_opts(ask, node, lane, lease, rel, dst, inside, false)
}

pub(super) fn fetch_keep_from(ask: &dyn Fn(&str, &str) -> Result<Vec<String>, String>, node: &str, lane: &str, lease: &str, dst: &Path) -> Result<Vec<(PathBuf, u64)>, String> {
    fetch_from_opts(ask, node, lane, lease, &format!("tmp/{}", super::KEEP_DIR), dst, true, true)
}

fn refuse_symlink(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path.components().as_path()) {
        Ok(m) if m.is_symlink() => Err(format!("refusing symlink destination {}", path.display())),
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("{}: {e}", path.display())),
        _ => Ok(()),
    }
}

/// A sibling created exclusively by this copy. Its handle closes before either rename or unlink.
struct GetTemp {
    path: PathBuf,
    renamed: bool,
}

impl Drop for GetTemp {
    fn drop(&mut self) {
        if self.renamed { return; }
        if let Err(e) = std::fs::remove_file(&self.path) {
            if e.kind() != std::io::ErrorKind::NotFound { eprintln!("devhub get: removing {}: {e}", self.path.display()); }
        }
    }
}

impl GetTemp {
    fn create(out: &Path) -> Result<(Self, std::fs::File), String> {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = out.with_file_name(format!(".devhub-get-{}-{}.tmp", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed)));
            if path == out { continue; }
            match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Ok((Self { path, renamed: false }, file)),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(format!("{}: {e}", path.display())),
            }
        }
    }
}

fn fetch_from_opts(ask: &dyn Fn(&str, &str) -> Result<Vec<String>, String>, node: &str, lane: &str, lease: &str, rel: &str, dst: &Path, inside: bool, skip_missing: bool) -> Result<Vec<(PathBuf, u64)>, String> {
    refuse_symlink(dst)?;
    let list = ask("get-ls", &format!("get-ls {lane} {lease} {rel}")).map_err(|e| format!("{node}: {e}"))?;
    let mut entries = Vec::with_capacity(list.len());
    for line in list {
        let v = json(&line).ok_or_else(|| format!("{node} get-ls: bad answer"))?;
        if let Some(e) = v.get("err").and_then(Value::as_str) { return Err(format!("{node}: {e}")); }
        let p = v.get("p").and_then(Value::as_str).ok_or("get-ls: missing path")?;
        let r = v.get("r").and_then(Value::as_str).ok_or("get-ls: missing read path")?;
        let normal = |p: &str| !p.is_empty() && Path::new(p).components().all(|c| matches!(c, std::path::Component::Normal(_)));
        if !normal(p) { return Err(format!("{node}: bad path {p}")); }
        let p = if inside { p.split_once('/').map_or(p, |(_, under)| under) } else { p };
        if !normal(p) { return Err(format!("{node}: bad path {p}")); }
        let priority = p.starts_with("gate-run.") || p == "gate-manifest.jsonl" || p.ends_with("/gate-manifest.jsonl");
        let size = v.get("n").and_then(Value::as_u64).unwrap_or(u64::MAX);
        entries.push((!priority, size, p.to_string(), r.to_string()));
    }
    entries.sort_unstable();
    let mut files = Vec::new();
    let vanished = |e: &str, marked: bool| {
        skip_missing && !e.contains("no live lease") && !super::master::unreachable(e) && !e.contains("restarting: ")
            && (marked || ["No such file or directory", "(os error 2)", "(os error 3)"].iter().any(|s| e.contains(s)))
    };
    'files: for (_, _, p, r) in entries {
        let out = dst.join(&p);
        let check_path = || {
            for path in out.ancestors().take_while(|path| path.starts_with(dst)) { refuse_symlink(path)?; }
            Ok::<_, String>(())
        };
        check_path()?;
        let dir = out.parent().ok_or("get: missing destination directory")?;
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let (mut temp, mut file) = GetTemp::create(&out)?;
        let mut written = 0u64;
        loop {
            let lines = match ask("get-read", &format!("get-read {lane} {lease} {written} {r}")) {
                Ok(lines) => lines,
                Err(e) if vanished(&e, false) => continue 'files,
                Err(e) => return Err(e),
            };
            let first = lines.first().ok_or_else(|| format!("{node} get-read: no answer"))?;
            let v = json(first).ok_or_else(|| format!("{node} get-read: bad answer"))?;
            if let Some(e) = v.get("err").and_then(Value::as_str) {
                if vanished(e, v.get("missing").and_then(Value::as_bool) == Some(true)) { continue 'files; }
                return Err(format!("{node}: {e}"));
            }
            let chunk = v.get("data").and_then(Value::as_str).and_then(b64_decode).ok_or("get-read: bad data")?;
            file.write_all(&chunk).map_err(|e| format!("{}: {e}", temp.path.display()))?;
            written += chunk.len() as u64;
            if v.get("eof").and_then(Value::as_bool).ok_or("get-read: missing eof")? { break; }
            if chunk.is_empty() { return Err(format!("{node} get-read: no progress reading {r} at {written}")); }
        }
        drop(file);
        check_path()?;
        std::fs::rename(&temp.path, &out).map_err(|e| format!("{}: {e}", out.display()))?;
        temp.renamed = true;
        files.push((out, written));
    }
    Ok(files)
}

/// `get <lane> <rel> <local-dir> [--node N] [--lease L]`: copies a file or folder from the lane's
/// dir on its node (or a live lease's) into <local-dir>, over the devhub link (no ssh).
pub(crate) fn get(lane: &str, rel: &str, dst: &str, rest: &[&str]) -> i32 {
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
    if let Some(why) = kept_into_checkout(&dst, &cwd(), lane) {
        eprintln!("makepad devhub get: {why}");
        return 2;
    }
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

/// The master's lease list (`queue`, `leases [--all]`), or one lease's line of it (`only`: `lease
/// <id> show`, a check-in's one-lease view; its queue reason in full, `why-queued` per box).
fn queue(c: &Client, all: bool, only: Option<&str>) -> i32 {
    let lines = match ask(c, None, "leases", if all || only.is_some() { "leases all" } else { "leases" }) {
        Ok(l) => l,
        Err(e) => return fail(e),
    };
    if lines.is_empty() {
        println!("no leases");
    }
    let mut leases: Vec<Value> = lines.iter().filter_map(|l| json(l)).collect();
    if let Some(id) = only {
        leases.retain(|v| s(v, "id") == id);
        if leases.is_empty() {
            return fail(format!("no lease {id} in the master's list (one that ended a while ago: `devhub job result {id} --tail`)"));
        }
    }
    let held = if leases.iter().any(|v| s(v, "state") == "granted") { one(c, "live", "live list held-leases").ok() } else { None };
    for v in &leases {
        println!("{}", queue_row(v, held.as_ref()));
    }
    let timed = if only.is_some() { Vec::new() } else { timed_boxes(&leases) };
    if !timed.is_empty() {
        println!("timed queue per box:");
        for line in timed { println!("{line}"); }
    }
    0
}

/// A queued lease no look ranked yet: a new ask before the master's next look, or one a restarted
/// master took back ([`super::sched::RESTART_REASON`]), whose place and held box are kept until
/// its first look ranks it. 5 s into a master's restart the queue read "not ranked yet" for an ask
/// that held its box and was granted a second later, and it read as parked where no look reached
/// it (L1821072, 11-26).
fn unranked(v: &Value) -> &'static str {
    if s(v, "reason") == super::sched::RESTART_REASON {
        "queued, not ranked yet: the master restarted, and its first look ranks it (its place and any box it held are kept)"
    } else {
        "queued, not ranked yet: the master's next look ranks it"
    }
}

/// One lease's line of `queue`.
fn queue_row(v: &Value, held: Option<&Value>) -> String {
    let node = s(&v, "node");
    let at = if node.is_empty() { String::new() } else { format!("@{node}") };
    let tail = match s(&v, "state").as_str() {
        "queued" if n(&v, "pos") < 1.0 => format!("{}, waiting {} s", unranked(v), n(&v, "wait_s")),
        "queued" => format!("{} in queue ({}) waiting {} s{}", ordinal(n(&v, "pos") as usize), s(&v, "reason"), n(&v, "wait_s"), eta_of(&v)),
        "ended" => format!("ended ({}){}", s(&v, "how"), if s(&v, "reason").is_empty() { String::new() } else { format!(": {}", s(&v, "reason")) }),
        st => {
            let info = v.get("info").filter(|i| !i.is_null());
            let progress = s(&v, "progress");
            let run = info.map(|i| s(i, "running")).filter(|r| !r.is_empty()).map_or(String::new(), |r| format!(" running `{r}`"));
            let run = if progress.is_empty() { run } else { format!("{run} ({progress})") };
            let ram = info.and_then(|i| i.get("held_mb")).and_then(super::num).map_or(String::new(), |m| format!(" ram {:.1} GB", m / 1024.0));
            let ram = format!("{ram}{}", s(&v, "ram_now"));
            let cores = s(&v, "cores_now");
            let disk = info.and_then(|i| i.get("disk_mb")).and_then(super::num).map_or(String::new(), |m| format!(" disk {:.1} GB", m / 1024.0));
            let agent = s(&v, "agent");
            let past = n(&v, "past_budget");
            let why = s(&v, "grant_why");
            let why = if why.is_empty() { why } else { format!(" ({why})") };
            format!(
                "{st} {}s ago{why}, beat {}s ago{}{run}{ram}{cores}{disk}{}{}",
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
    let diagnostic = held.and_then(|rows| rows.get(&s(v, "id"))).and_then(Value::as_str).filter(|d| !d.is_empty() && s(v, "state") == "granted")
        .map_or(String::new(), |d| format!(" held: {d}"));
    let wm_wait = v.get("wm_wait").and_then(Value::as_str).filter(|w| !w.is_empty()).map_or(String::new(), |w| format!(" [{w}]"));
    format!("{:<6} {:<8} {:<10} {} {} ({}) {} {}{}: {tail}{diagnostic}{wm_wait}", s(&v, "id"), s(&v, "lane"), format!("{}{at}", s(&v, "kind")), s(&v, "priority"), s(&v, "cost"), s(&v, "cost_source"), s(&v, "src"), if target.is_empty() { "" } else { "jit " }, target)
}

/// `queue --long`: the leases queued 10 min or more, a check-in's starvation list. A lane's
/// leases that wait only on their own lane's lease (its tree's one lease at a time: its serial
/// plan, [`super::sched::Sched::own_wait`]) fold into one line per lane.
fn long_waits(c: &Client) -> i32 {
    match ask(c, None, "leases", "leases") {
        Ok(lines) => {
            for line in long_wait_lines(&lines.iter().filter_map(|l| json(l)).collect::<Vec<_>>()) { println!("{line}"); }
            0
        }
        Err(e) => fail(e),
    }
}

/// [`long_waits`]' lines, folded as every count a person reads ([`super::long_waits`]): the
/// count, each long wait on another lane, cores, RAM or a hold as `queue` lists it, per lane its
/// leases behind its own lease with the soonest of their expected starts, and the boxes'
/// prebuilds on a line of their own. A sweep's 14 jobs queued one at a time behind their own
/// lease read as 14 of 15 starved leases while three boxes idled (11-17).
fn long_wait_lines(leases: &[Value]) -> Vec<String> {
    let w = super::long_waits(leases.iter().filter(|v| s(v, "state") == "queued"), |v| n(v, "wait_s") as u64 * 1000, |v| s(v, "lane"), |v| !s(v, "own_wait").is_empty());
    if w.count() == 0 && w.prebuilds.is_empty() { return Vec::new(); }
    let plans = match w.serial.len() { 0 => String::new(), 1 => ", a lane's serial plan counted once".into(), n => format!(", {n} lanes' serial plans counted once each") };
    let mut out = vec![format!("{} waiting over 10 min ({} starved{plans})", w.count(), w.starved.len())];
    out.extend(w.starved.iter().map(|v| queue_row(v, None)));
    for (lane, ls) in &w.serial {
        let holders: std::collections::BTreeSet<String> = ls.iter().map(|v| s(v, "own_wait")).collect();
        let next = ls.iter().filter(|v| !v.get("eta_late").and_then(Value::as_bool).unwrap_or(false)).map(|v| n(v, "eta_ms") as u64).filter(|ms| *ms > 0).min();
        let next = next.map_or("next start not known".to_string(), |ms| format!("next in ~{} min", eta_min(ms)));
        out.push(format!("{lane}: {} queued behind its own {} (serial in its tree), {next}", ls.len(), holders.into_iter().collect::<Vec<_>>().join(", ")));
    }
    if !w.prebuilds.is_empty() {
        let each: Vec<String> = w.prebuilds.iter().map(|v| format!("{} {} {} min ({})", s(v, "id"), s(v, "lane"), n(v, "wait_s") as u64 / 60, s(v, "reason"))).collect();
        out.push(format!("prebuilds and warm legs waiting (warm-ups, no starvation): {}", each.join("; ")));
    }
    out
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
                    let unsupervised = s(&v, "unsupervised");
                    if !unsupervised.is_empty() {
                        println!("{:<8} {:<8} warning: {unsupervised}", "", "");
                    }
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
                // its drops off the network and their pattern, once it dropped in the last day
                let link = s(&v, "link");
                if !link.is_empty() {
                    println!("{:<8} {:<8} {link}", "", "");
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

fn disk(c: &Client, node: &str, rest: &[&str]) -> i32 {
    let dry = rest.contains(&"--dry-run");
    // lanes that ended before their cleanup said so on the node: their grabs there reclaimable
    if let Some(i) = rest.iter().position(|a| *a == "--ended") {
        let Some(lanes) = rest.get(i + 1).filter(|l| !l.starts_with("--")) else { eprintln!("makepad devhub disk <node> --ended <lane>,.."); return 2 };
        match ask(c, Some(node), "disk", &format!("disk ended {lanes}")) {
            Ok(lines) if lines.first().is_some_and(|l| l.starts_with("{\"ended\":")) => {
                eprintln!("devhub disk: {node} notes {} lane(s) as ended", json(&lines[0]).map_or(0.0, |v| n(&v, "ended")));
            }
            Ok(lines) => { eprintln!("devhub disk: {node}'s hub predates `disk ended`: {}", lines.first().map_or("", |l| l.as_str())); return 1; }
            Err(e) => return fail(e),
        }
    }
    // the queue's: the need of a head its disk keeps off, and the lanes whose own JIT trees stay
    // for their queued asks, as the head's own reclaim has them
    let queue = ask(c, None, "disk-need", &format!("disk-need {node}")).ok().and_then(|l| l.first().and_then(|l| json(l)));
    let need = match rest.iter().position(|a| *a == "--need") {
        Some(i) => match rest.get(i + 1).and_then(|n| n.parse::<f64>().ok()).filter(|n| *n >= 0.0) {
            Some(n) => n,
            None => { eprintln!("makepad devhub disk <node> [--need GB] [--dry-run]"); return 2; }
        },
        None => match &queue {
            Some(v) if n(v, "need_gb") > 0.0 => {
                eprintln!("devhub disk: {} waits for {:.0} GB over {node}'s floor: the reclaim aims there (--need 0: the floor only)", s(v, "for"), n(v, "need_gb"));
                n(v, "need_gb")
            }
            _ => 0.0,
        },
    };
    let keep = queue.as_ref().map(|v| s(v, "keep")).unwrap_or_default();
    let line = format!("disk{}{}{}", if dry { " dry" } else { "" }, if need > 0.0 { format!(" need {need:.0}") } else { String::new() },
        if keep.is_empty() { String::new() } else { format!(" keep {keep}") });
    match ask(c, Some(node), "disk", &line) {
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

/// `lease <id> jits`: the JITs among a running lease's processes, wherever they run (a JIT-run
/// test leads a process group of its own), as `<node>/<pid> <app>`: what `devhub inspect` and a
/// sampler take.
fn lease_jits(id: &str) -> i32 {
    let c = match master() {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    match lane_node(&c, "", Some(id)).and_then(|node| node_one(&c, &node, "lease-jits", &format!("lease-jits {id}")).map(|v| (node, v))) {
        Ok((node, v)) => {
            let jits = v.get("jits").and_then(Value::as_arr).unwrap_or(&[]);
            if jits.is_empty() { println!("no JIT runs under {id} on {node}"); }
            for j in jits {
                println!("{node}/{} {}", j.get("pid").and_then(super::num).unwrap_or(0.0) as u64, j.get("app").and_then(Value::as_str).unwrap_or(""));
            }
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
        ["lease", id, "jits"] => return Some(lease_jits(id)),
        ["lease", _, "show"] | ["lease", "show", _] => {}
        ["lease", rest @ ..] => return Some(lease(rest)),
        ["overlay", "selftest", rest @ ..] => return Some(overlay_selftest(rest)),
        ["overlay", word @ ("status" | "why"), lane, rest @ ..] => return Some(overlay_inspect(word, lane, rest)),
        ["run", lane, kind, rest @ ..] => return Some(run(lane, kind, rest)),
        ["test", lane, pkg, rest @ ..] => return Some(test(lane, pkg, rest)),
        ["test", ..] => { eprintln!("makepad devhub test <lane> <pkg> [--lib|--bin B|--test T|--tests] [--features F] [--no-default-features] [--needs k=v,..] [--node N [--series S|--pin-why R]] [--avoid N[,N..]] [--cost small|medium|large] [--est-min N] [--cores N] [--class name] [--priority interactive|background] [--jit key] [--native [<reason>]|--live [--offline]] [--tree] [-- args...]"); return Some(2); }
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
        ["queue" | "leases", rest @ ..] if rest.contains(&"--long") => long_waits(&c),
        ["queue" | "leases", rest @ ..] => queue(&c, rest.contains(&"--all"), None),
        ["lease", id, "show"] | ["lease", "show", id] => queue(&c, true, Some(id)),
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
        // the toolchain probe now (a box out of rotation after a toolchain move comes back at once)
        ["node", node, "probe"] => match node_one(&c, node, "toolchain-probe", "toolchain-probe") {
            Ok(v) => {
                match v.get("audio").and_then(|a| a.as_bool()) {
                    Some(true) => println!("{node}: its audio output runs: audio=1 work may go there"),
                    Some(false) => println!("{node}: its audio output does not run (a silent playback did not end in {} s): audio=1 work goes elsewhere", super::caps::AUDIO_PROBE_WAIT.as_secs()),
                    None => {}
                }
                match v.get("why").and_then(|w| w.as_str()) {
                    None => { println!("{node}: its toolchain links: in rotation"); 0 }
                    Some(why) => { println!("{node}: its toolchain cannot link, out of rotation: {why}"); 1 }
                }
            }
            Err(e) if e.contains("unknown") || e.contains("unsupported") => fail(format!("{e} ({node}'s hub is older than `node probe`: it takes it from its next self-update)")),
            Err(e) => fail(e),
        },
        ["node", node] => node_conf(&c, node, ""),
        ["node", node, "role", role] => node_conf(&c, node, &format!("set role={role}{}", conf_by())),
        ["node", node, "set", kvs @ ..] if !kvs.is_empty() => node_conf(&c, node, &format!("set {}{}", kvs.join(" "), conf_by())),
        ["disk", node, rest @ ..] => disk(&c, node, rest),
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
        for cargo in [None, Some(100_000)] {
            let (chosen, reason) = test_route(own, None, cargo);
            assert!(chosen && reason.contains("this run measures it"), "no JIT measure: the JIT measures it: own={own}, cargo={cargo:?}, {reason}");
        }
        assert_eq!(test_route(own, Some(70_000), None).0, !own, "no cargo measure: the default: own={own}");
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
        // a release whose answer the master's restart cut, asked again: the restarted master has the
        // lease ended by it, and that stands at once (L1717780 waited a minute on it, 11-24)
        let (mut n, ended) = (0, "L1 has already ended (`devhub leases --all`)");
        let r: Result<(), String> = through_restarts("L1", &mut c, |_| { n += 1; Err(if n == 1 { "link lost: connection closed" } else { ended }.into()) }, &up);
        assert_eq!((r, n), (Err(ended.to_string()), 2));
        assert!(ended_already("L1", ended) && !ended_already("L1", "L12 has already ended (`devhub leases --all`)"));
        assert!(lost_how("expired") && lost_how("lost") && !lost_how("cancelled") && lost_error("node-a: no lease L1 on a node") && !lost_error("node-a: lease-log: bad"));
        // the command's own end is said with its reason, never as lost
        assert!(["hung", "disk", "memory", "panicked", "stopped"].iter().all(|h| verdict_how(h) && !lost_how(h)) && !verdict_how("lost") && !verdict_how("cancelled"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_run_backs_off_and_restarts_on_a_new_phase() {
        let resolving = r#"{"pending":"resolving"}"#;
        let building = r#"{"pending":"building"}"#;
        let mut replies = [resolving, resolving, building, building, building, building, building, building, r#"{"n":7}"#].into_iter();
        let mut waits = Vec::new();
        let run = poll_start("L1", "box-a", || Ok(json(replies.next().unwrap()).unwrap()), |d| waits.push(d)).unwrap();
        assert_eq!(n(&run, "n"), 7.0, "return the admitted run");
        assert_eq!(waits, [100, 200, 100, 200, 400, 800, 1000, 1000].map(Duration::from_millis));
    }

    #[test]
    fn finish_ended_treats_stopped_as_verdict_not_lost() {
        let finish = |node: &str, how: &str, why: &str| finish_ended("L7", node, Some(&(how.into(), why.into())));
        assert_eq!(finish("win-a", "stopped", "stopped by user: wedged"), Some((1, "makepad devhub: L7 on win-a ended (stopped): stopped by user: wedged".into())));
        assert_eq!(finish("box-a", "stopped", ""), Some((1, "makepad devhub: L7 on box-a ended (stopped)".into())));
        assert_eq!(finish("box-a", "cancelled", "cancelled by user").unwrap().0, CANCELLED_EXIT);
        for how in ["lost", "expired"] { assert_eq!(finish("box-a", how, "no lease L7"), None); }
        let beat = Beat { stop: Default::default(), ended: Default::default(), hung: Default::default(), thread: None };
        for (how, why, message) in [
            ("stopped", "stopped by main: hung", "L7 ended (stopped): stopped by main: hung"),
            ("stopped", "", "L7 ended (stopped)"),
            ("lost", "no lease L7", "L7 ended (lost)"),
        ] {
            *beat.ended.lock().unwrap() = Some((how.into(), why.into()));
            assert_eq!(beat.check("L7"), Err(message.into()));
        }
    }

    #[test]
    fn beat_word_shows_distinct_words_and_never_masks_later_hung_verdict() {
        let mut told = String::new();
        for (word, hung) in [("a's WM update waits on you", false), ("every process idle", true)] {
            let beat = json(&format!(r#"{{"idle":"{word}","hung":{hung}}}"#)).unwrap();
            assert_eq!(beat_word(&beat, &mut told), Some((word.to_string(), hung)));
            assert_eq!(beat_word(&beat, &mut told), None);
        }
    }

    #[test]
    fn queue_row_displays_wm_wait_tag_when_present_and_omits_when_absent() {
        let row = queue_row(&json(r#"{"state":"granted"}"#).unwrap(), None);
        assert!(!row.contains('['));
        let tag = "a's WM update waits on this lease (6m00s)";
        let held = json(&format!(r#"{{"state":"granted","wm_wait":"{tag}"}}"#)).unwrap();
        assert_eq!(queue_row(&held, None), format!("{row} [{tag}]"));
    }
    #[cfg(feature = "checkout")]
    use crate::jit_misses::{pre_land_fixture::Repo, CheckRun};
    #[cfg(feature = "checkout")]
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[cfg(feature = "checkout")]
    fn check_fixture(repo: &Repo, full: Option<&str>, check: impl Fn(&CheckRun) -> (i32, String) + Sync) -> (i32, String) {
        let mut output = String::new();
        let code = pre_land_checks_at(&repo.0, &repo.pin(), false, full, || Ok(check), |line| {
            output.push_str(line);
            output.push('\n');
        });
        (code, output)
    }

    #[cfg(feature = "checkout")]
    #[test]
    fn exit_to_round_trips_lease_id_and_stays_compatible_with_two_field_files() {
        let pid = std::process::id();
        let path = std::env::temp_dir().join(format!("devhub-exit-lease-{pid}.exit"));
        super::write_exit(Some(&path), 101, Some("L42"));
        assert_eq!(super::read_exit_lease(&path, pid), Some((101, Some("L42".into()))));
        assert_eq!(super::read_exit(&path, pid), Some(101));
        assert_eq!(super::read_exit_lease(&path, pid + 1), None);
        super::write_exit(Some(&path), 0, None);
        assert_eq!(super::read_exit_lease(&path, pid), Some((0, None)));
        for suffix in ["", " bogus"] {
            std::fs::write(&path, format!("{pid} 7{suffix}\n")).unwrap();
            assert_eq!(super::read_exit_lease(&path, pid), Some((7, None)));
        }
        std::fs::remove_file(path).unwrap();
    }

    #[cfg(feature = "checkout")]
    #[test]
    fn pre_land_skips_tests_check_only_for_pkg_tested_in_full() {
        let repo = Repo::new("full", &[
            ("Cargo.toml", "[workspace]\nmembers = ['town', 'core']\n"),
            ("town/Cargo.toml", "[package]\nname = 'makepad-town'\n"), ("town/src/lib.rs", ""),
            ("core/Cargo.toml", "[package]\nname = 'makepad-town-core'\n"), ("core/src/lib.rs", ""),
        ]);
        repo.write("town/src/lib.rs", "pub fn town() {}\n");
        repo.write("core/src/lib.rs", "pub fn core() {}\n");
        let check = |full, plain: &[&str], tests: &[&str]| {
            let seen = std::sync::Mutex::new(Vec::new());
            let (code, log) = check_fixture(&repo, full, |run| {
                seen.lock().unwrap().push((run.tests, run.packages.clone()));
                (0, String::new())
            });
            assert_eq!(code, 0, "{log}");
            let mut runs = seen.into_inner().unwrap();
            runs.sort();
            assert_eq!(runs.len(), 1 + usize::from(!tests.is_empty()));
            assert!(!runs[0].0);
            assert_eq!(runs[0].1, plain, "the plain check always covers the touched shipping packages");
            if !tests.is_empty() {
                assert!(runs[1].0);
                assert_eq!(runs[1].1, tests, "the companion covers every touched package still needing tests");
            }
        };
        let both = ["makepad-town", "makepad-town-core"];
        check(tested_in_full(Some("lib"), &["--lib"]).then_some("makepad-town"), &both, &both);
        check(Some("makepad-town"), &both, &["makepad-town-core"]);
        repo.write("core/src/lib.rs", "");
        check(Some("makepad-town"), &["makepad-town"], &[]);
    }

    #[cfg(feature = "checkout")]
    #[test]
    fn pre_land_catches_bin_test_error_under_lib_selector() {
        let repo = Repo::new("bin-error", &[
            ("Cargo.toml", "[workspace]\nmembers = ['app']\n"),
            ("app/Cargo.toml", "[package]\nname = 'app'\n"), ("app/src/lib.rs", ""),
            ("app/src/main.rs", "fn main() {}\n"),
        ]);
        repo.write("app/src/main.rs", "fn call(_: u8) {}\nfn main() { call(1); }\n#[test] fn broken() { call(); }\n");
        let error = "app/src/main.rs:3:23: error[E0061]: this function takes 1 argument but 0 arguments were supplied";
        let full = tested_in_full(Some("lib"), &["--lib"]).then_some("app");
        let (code, log) = check_fixture(&repo, full, |run| if run.tests { (1, error.into()) } else { (0, String::new()) });
        assert_eq!(code, 1, "{log}");
        assert!(log.contains(error) && log.contains("check of app (--tests) FAILED") && log.contains("`makepad check -p app --tests` shows it alone"), "{log}");
    }

    #[cfg(feature = "checkout")]
    #[test]
    fn pre_land_preserves_dead_code_warning_in_plain_build() {
        let repo = Repo::new("plain-warning", &[
            ("Cargo.toml", "[workspace]\nmembers = ['a']\n"),
            ("a/Cargo.toml", "[package]\nname = 'a'\n"), ("a/src/lib.rs", ""),
        ]);
        repo.write("a/src/lib.rs", "fn helper() {}\n#[test] fn uses_helper() { helper(); }\n");
        let warning = "a/src/lib.rs:1:4: warning: function `helper` is never used";
        let (code, log) = check_fixture(&repo, None, |run| (0, if run.tests { String::new() } else { warning.into() }));
        assert_eq!(code, 1, "{log}");
        assert!(log.contains(warning) && log.contains("check of a without their tests FAILED") && log.contains("`makepad check -p a` shows it alone"), "{log}");
        assert!(log.contains("check of a (--tests): ok, 0 warnings"), "{log}");
    }

    #[cfg(feature = "checkout")]
    #[test]
    fn pre_land_retains_tests_check_when_feature_flags_are_passed() {
        let cases: &[(Option<&str>, &[&str], bool)] = &[
            (None, &[], true), (Some(ALL_TESTS), &["--tests"], true),
            (None, &["--no-default-features"], false), (Some(ALL_TESTS), &["--tests", "--no-default-features"], false),
            (None, &["--features", "kernel"], false), (Some(ALL_TESTS), &["--tests", "--features", "kernel"], false),
            (None, &["--all-features"], false), (Some("lib"), &["--lib"], false),
            (Some("bin:b"), &["--bin", "b"], false), (Some("t"), &["--test", "t"], false),
        ];
        for &(target, select, expected) in cases { assert_eq!(tested_in_full(target, select), expected, "{select:?}"); }
        let repo = Repo::new("feature-flags", &[
            ("Cargo.toml", "[workspace]\nmembers = ['hub']\n"),
            ("hub/Cargo.toml", "[package]\nname = 'hub'\n[features]\ndefault = ['kernel']\nkernel = []\n[[test]]\nname = 'program_remote'\nrequired-features = ['kernel']\n"),
            ("hub/src/lib.rs", ""), ("hub/tests/program_remote.rs", ""),
        ]);
        repo.write("hub/tests/program_remote.rs", "#[test] fn remote() { let _ = x; }\n");
        repo.git(&["commit", "-qam", "edit integration test"]);
        let runs = std::sync::atomic::AtomicUsize::new(0);
        let error = "hub/tests/program_remote.rs:1:1: error[E0425]: cannot find value `x`";
        let (code, log) = check_fixture(&repo, tested_in_full(None, &["--no-default-features"]).then_some("hub"), |run| {
            runs.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            assert!(run.tests && !run.own);
            assert_eq!(run.packages, ["hub"]);
            assert_eq!(run.alone, "makepad check -p hub --tests");
            (1, error.into())
        });
        assert_eq!(runs.into_inner(), 1);
        assert_eq!(code, 1, "{log}");
        assert!(log.contains(error), "{log}");
        let (code, log) = check_fixture(&repo, tested_in_full(None, &[]).then_some("hub"), |_| panic!("the default-feature suite already ran"));
        assert_eq!(code, 0, "{log}");
    }

    /// The long waits list starvation lease by lease; a lane's leases behind its own lease fold
    /// into one line with their soonest expected start, counted once; the prebuilds go on a line
    /// of their own, uncounted; waits under 10 min are not listed.
    #[test]
    fn long_waits_fold_a_lanes_own_serial_plan() {
        let lease = |id: &str, lane: &str, wait: u64, own: &str, eta: u64| json(&format!("{{\"id\":\"{id}\",\"lane\":\"{lane}\",\"kind\":\"test\",\"state\":\"queued\",\"pos\":1,\"reason\":\"r\",\"wait_s\":{wait},\"eta_ms\":{eta},\"own_wait\":{}}}", if own.is_empty() { "null".to_string() } else { format!("\"{own}\"") })).unwrap();
        let leases = [lease("L2", "sweep", 900, "L1", 240_000), lease("L3", "sweep", 900, "L1", 0), lease("L4", "sweep", 700, "L1", 600_000), lease("L5", "sweep", 60, "L1", 0), lease("L6", "other", 1200, "", 0), lease("L7", "late", 30, "", 0), lease("L8", "prebuild-node-b", 1560, "", 0), lease("L9", "warm-rc-x", 660, "", 0)];
        let lines = long_wait_lines(&leases);
        assert_eq!(lines.len(), 4, "{lines:?}");
        assert_eq!(lines[0], "2 waiting over 10 min (1 starved, a lane's serial plan counted once)");
        assert!(lines[1].starts_with("L6") && lines[1].contains("in queue (r) waiting 1200 s"), "{lines:?}");
        assert_eq!(lines[2], "sweep: 3 queued behind its own L1 (serial in its tree), next in ~4 min");
        assert_eq!(lines[3], "prebuilds and warm legs waiting (warm-ups, no starvation): L8 prebuild-node-b 26 min (r); L9 warm-rc-x 11 min (r)", "a lane's warm leg is no starved lease");
        assert!(long_wait_lines(&[lease("L3", "sweep", 900, "L1", 0)])[1].ends_with("next start not known"));
        assert!(long_wait_lines(&[lease("L5", "sweep", 60, "L1", 0)]).is_empty(), "nothing long: nothing said");
        let grant = json(r#"{"id":"L1","state":"granted"}"#).unwrap();
        let plain = queue_row(&grant, None);
        let held = json(r#"{"L1":"error: missing item","L2":"another session"}"#).unwrap();
        assert_eq!(queue_row(&grant, Some(&held)), format!("{plain} held: error: missing item"), "only the held lease's diagnostic is appended");
        assert_eq!(queue_row(&grant, Some(&Value::Null)), plain, "an unavailable lookup leaves the row intact");
        // a lease no look ranked yet says why: a restarted master took it back (its place and its
        // box's hold kept), or a new ask before the next look (a restart's read as parked, 11-26)
        let taken_back = json(r#"{"id":"L2","state":"queued","pos":0,"reason":"master restart","wait_s":1419}"#).unwrap();
        assert!(queue_row(&taken_back, None).ends_with(": queued, not ranked yet: the master restarted, and its first look ranks it (its place and any box it held are kept), waiting 1419 s"), "{}", queue_row(&taken_back, None));
        assert!(queue_row(&json(r#"{"id":"L3","state":"queued","pos":0,"reason":"","wait_s":0}"#).unwrap(), None).ends_with(": queued, not ranked yet: the master's next look ranks it, waiting 0 s"));
    }

    /// A devhub flag among the test binary's words is refused before any run: `--rev` after `--`
    /// reached the binary and was recorded as a JIT miss (M98, 11-16). The binary's own flags pass.
    #[test]
    fn a_devhub_flag_after_the_separator_is_refused() {
        assert_eq!(flag_after_separator(&["--", "lease::", "--rev", "rc-x@abc"]).as_deref(), Some("`--rev` after `--` goes to the test binary; put it before `--`"));
        assert_eq!(flag_after_separator(&["--", "--cost=small"]).as_deref(), Some("`--cost` after `--` goes to the test binary; put it before `--`"));
        assert_eq!(flag_after_separator(&["--", "x", "--exact", "--nocapture", "--test-threads", "1", "--test"]), None, "the binary's own flags");
        assert_eq!(flag_after_separator(&[]), None);
        assert_eq!(flag_after_separator(&["--", "--fresh"]).as_deref(), Some("`--fresh` after `--` goes to the test binary; put it before `--`"));
    }

    /// A master's refusal names both builds when the master runs another one than the CLI's, which
    /// side lags as git says, and is only the refusal when they match or one is unknown: the CLI of
    /// a landing printed the old master's usage line on every `devhub test` for 8 min (11-15).
    #[test]
    fn a_refusal_from_a_master_of_another_build_says_both_builds() {
        let (old, new) = ("a".repeat(40), "b".repeat(40));
        let older = |a: &str, b: &str| a == "a".repeat(40) && b == "b".repeat(40);
        let said = builds_said(Some(&old), Some(&new), &older, "stats queue | route");
        assert_eq!(said, "the master runs aaaaaaaaaa, older than this CLI's bbbbbbbbbb (the CLI and the master move together at the master's restart: `devhub master who`): stats queue | route", "an older master is said as older");
        assert!(builds_said(Some(&new), Some(&old), &older, "x").contains("another build than this CLI's aaaaaaaaaa"), "a newer master is another build");
        assert_eq!(builds_said(Some(&new), Some(&new), &older, "x"), "x", "the same build: the refusal alone");
        assert_eq!(builds_said(None, Some(&new), &older, "x"), "x", "a master that names no build");
        assert_eq!(builds_said(Some(&old), None, &older, "x"), "x", "a CLI built by hand");
    }

    /// A job's waiter says each new line of its lease's wait at once, and only while it waits:
    /// a lane's tests waited behind its own screen lease, "one lease at a time in its tree" said
    /// only in the queue (11-15).
    #[test]
    fn a_waiting_job_says_each_new_line_of_its_wait_once() {
        let job = |state: &str, note: &str| crate::state::Job { id: "J1".into(), state: state.into(), note: note.into(), ..Default::default() };
        let mut told = String::new();
        let held = "L1 queued 101: held by L0 (screen of lane-a): one lease at a time in its tree";
        assert!(waits_said(&job("leasing", held), &mut told) && told == held, "the first line is said: {told}");
        assert!(!waits_said(&job("leasing", held), &mut told), "the same line is not said again");
        assert!(waits_said(&job("leasing", "L1 queued 101: budget cores 16, in use 16, asked 4"), &mut told), "a new line is");
        assert!(!waits_said(&job("running", "its run's note"), &mut told), "a running job's note is no wait");
        assert!(!waits_said(&job("queued", ""), &mut told), "nothing said says nothing");
    }

    /// A live run without a selector follows disk discovery on each call: one harness target is a
    /// warm session, several run the package whole.
    #[cfg(feature = "checkout")]
    #[test]
    fn a_live_run_finds_every_harness_target_on_disk() {
        let dir = std::env::temp_dir().join(format!("devhub-live-suite-{}-{}", std::process::id(), crate::proto::wall_ms()));
        for path in ["p/src", "p/tests", "p/checks", "p/examples"] { std::fs::create_dir_all(dir.join(path)).unwrap(); }
        let write = |path: &str, text: &str| std::fs::write(dir.join(path), text).unwrap();
        let words = |s: &[&str]| s.iter().map(|w| w.to_string()).collect::<Vec<_>>();
        write("Cargo.toml", "[workspace]\nmembers = [\"p\"]\n");
        let manifest = "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[[test]]\nname = \"c\"\npath = \"checks/c.rs\"\n\n[[test]]\nname = \"d\"\npath = \"checks/d.rs\"\nharness = false\n";
        write("p/Cargo.toml", manifest);
        write("p/src/lib.rs", "");
        for path in ["p/tests/a.rs", "p/tests/b.rs", "p/checks/c.rs", "p/checks/d.rs", "p/src/main.rs"] { write(path, "fn main() {}\n"); }
        let targets = live_targets(&dir, "p", &[]).unwrap();
        let expected = vec![words(&["--lib"]), words(&["--bin", "p"]), words(&["--test", "a"]), words(&["--test", "b"]), words(&["--test", "c"])];
        assert_eq!(targets.runs, expected, "lib, bins and integration harnesses all run in resolver order");
        assert_eq!(targets.not_live, words(&["--test d (harness = false)"]), "a plain-main target is named instead of silently skipped");
        assert!(!dir.join(".git").exists(), "discovery needs no git repository or tracked files");
        write("p/tests/e.rs", "");
        let targets = live_targets(&dir, "p", &[]).unwrap();
        let mut expected = expected;
        expected.push(words(&["--test", "e"]));
        assert_eq!(targets.runs, expected, "a new file is a new target on the next call");
        let manifest = format!("{manifest}\n[[test]]\nname = \"a\"\nharness = false\n");
        write("p/Cargo.toml", &manifest);
        let targets = live_targets(&dir, "p", &[]).unwrap();
        expected.retain(|s| s != &words(&["--test", "a"]));
        assert_eq!(targets.runs, expected, "an explicit harness=false overrides the discovered target");
        assert_eq!(targets.not_live, words(&["--test a (harness = false)", "--test d (harness = false)"]), "both non-harness targets stay visible");

        write("p/checks/featured.rs", "");
        write("p/Cargo.toml", &format!("{manifest}\n[[test]]\nname = \"featured\"\npath = \"checks/featured.rs\"\nrequired-features = [\"extra\"]\n\n[features]\nextra = []\n"));
        assert_eq!(live_targets(&dir, "p", &words(&["--no-default-features"])).unwrap().runs, expected, "a target's required features still gate discovery");
        for flags in [words(&["--features", "extra"]), words(&["-F", "extra"]), words(&["--features=extra"]), words(&["-Fextra"]), words(&["--all-features"])] {
            assert!(live_targets(&dir, "p", &flags).unwrap().runs.contains(&words(&["--test", "featured"])), "feature words enable the target: {flags:?}");
        }
        assert_eq!(live_targets(&dir, "p", &words(&["--lib", "--", "--features", "missing"])).unwrap().runs, expected, "selectors and libtest arguments are not resolver feature flags");

        write("p/examples/plain.rs", "fn main() {}\n");
        write("p/Cargo.toml", "[package]\nname = \"p\"\nversion = \"0.1.0\"\nedition = \"2021\"\nautobins = false\nautotests = false\n[lib]\ntest = false\n[[test]]\nname = \"d\"\npath = \"checks/d.rs\"\nharness = false\n[[example]]\nname = \"plain\"\ntest = true\n");
        let error = live_targets(&dir, "p", &[]).err().expect("a package with no harness must fail discovery");
        assert_eq!(error, "p has no harness target to run live; not run live: --test d (harness = false), --example plain (examples run without --live)", "an empty suite names every target it cannot run live");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A test run whose lease never started says so in place of its verdict, and the pre-land
    /// check reads that back as no verdict, never as the crates' red ("FAILED: no error line" for
    /// a check whose lease a stale lock refused, 11-13).
    #[test]
    fn a_test_whose_lease_never_started_reads_as_not_run() {
        let said = "lease L1 denied: refused on box-a: lease-start L1: ci merge config: config.worktree is locked";
        let log = format!("{}\n", super::not_run_line("check of relax-compiler relax-types", said));
        assert_eq!(super::not_run(&log), Some(said));
        assert_eq!(super::not_run("devhub test: makepad-devhub failed in 1.000s (queue 0.100s, ship 0.100s, run 0.800s) on box-a\n"), None, "a run that started has its verdict");
    }

    /// A hand-in's gate run that has not started by its estimate goes after the landing unrun, its
    /// gate saying why: a 1m18s check queued 26 min and held its landing (L1205988, 11-16).
    #[cfg(feature = "checkout")]
    #[test]
    fn a_gate_run_not_started_by_its_estimate_goes_after_the_landing() {
        use std::time::Duration;
        assert_eq!(super::gate_start_bound(Some(78_000)), Duration::from_millis(78_000), "its estimate");
        assert_eq!(super::gate_start_bound(Some(5_000)), Duration::from_millis(super::GATE_START_MIN_MS), "never under a short run's own start bound");
        assert_eq!(super::gate_start_bound(None), Duration::from_millis(super::CHEAP_GATE_MS), "no measure: the cheap gate's bound");
        let said = super::deferred_said("L1", Duration::from_secs(95), Duration::from_secs(78));
        let log = format!("{}\n", super::not_run_line("check of makepad-devhub", &said));
        assert_eq!(super::no_verdict(super::DEFERRED_EXIT, &log), said, "its gate's note says why it went after the landing");
        assert!(said.starts_with("L1 had not started") && said.ends_with("to the post-land buckets"), "{said}");
    }

    /// A live test's feature set is said one way and is part of its session's key: the same set
    /// in any order shares a session, another set gets its own (`--live` refused `--features`, 11-13).
    #[test]
    fn a_live_tests_feature_set_is_part_of_its_session_key() {
        let s = |a: &[&str]| a.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let one = super::live_select(s(&["--features", "scene3d packs", "--lib", "--", "selection_bounds"]));
        assert_eq!(one, s(&["--lib", "--features", "packs,scene3d", "--", "selection_bounds"]), "one order, after its target");
        assert_eq!(super::live_select(s(&["--lib", "--features", "packs,scene3d", "--", "selection_bounds"])), one, "the same set: the same session");
        assert_ne!(super::live_select(s(&["--lib", "--features", "packs", "--", "selection_bounds"])), one, "another set: another session");
        assert_eq!(super::live_select(s(&["--no-default-features", "--lib"])), s(&["--lib", "--no-default-features"]));
        assert_eq!(super::live_select(s(&["--lib", "--", "--features", "x"])), s(&["--lib", "--", "--features", "x"]), "libtest's own arguments stay");
        #[cfg(feature = "checkout")]
        for args in [one.clone(), super::live_select(s(&["--no-default-features", "--features", "a", "--test", "t"])), super::live_select(s(&["--features", "a"]))] {
            let select = &args[..args.iter().position(|a| a == "--").unwrap_or(args.len())];
            assert!(crate::orch::live::test_select_ok(select), "the master takes the key the client says: {args:?}");
        }
    }

    /// A `live test` call that waits for room on its box answers with its session after the wait's
    /// lines: they are said, never the answer (W6628, 11-13).
    #[test]
    fn a_live_test_call_that_waits_for_room_answers_with_its_session() {
        let mut answer = None;
        let mut output = Vec::new();
        let lines = [
            r#"{"say":"the held session W0 was parked for L1 at 01:01Z; this run started cold","waiting":true}"#,
            r#"{"say":"live call W1 waits for room on box-a: box-a: budget ram 38.0 GB, in use 36.1 GB, asked 4.1 GB","waiting":true}"#,
            r#"{"say":"","waiting":false}"#,
            r#"{"id":"W1","state":"running","room":"live call W1 waited 41000 ms for room on box-a"}"#,
            r#"{"end":true}"#,
        ];
        let mut got = None;
        for l in lines { if let Some(v) = live_line(l, &mut answer, &mut output).unwrap() { got = Some(v); } }
        let got = got.expect("the call's answer at its end");
        assert!(output.starts_with(b"the held session W0 was parked for L1 at 01:01Z; this run started cold\ndevhub: live call W1 waits"), "the cold notice precedes the wait and the fresh session's answer");
        assert_eq!(crate::state::live_from(&got).id, "W1", "{}", got.to_json());
        // an answer from the session's last round, nothing changed since, says so on its verdict's
        // line, and how to run the tests again (a flake hunt read four cached passes as its own, 11-26)
        let result = "relax: notice\ndevhub test: a 1/1 ran (0 skipped, selected all) in 0.100 s\n";
        assert_eq!(cached_said(result, Some(45_296_000)), "relax: notice\ndevhub test (cached: the verdict of its round at 12:34:56Z, nothing changed since and nothing ran; --fresh runs the tests again): a 1/1 ran (0 skipped, selected all) in 0.100 s\n");
        assert_eq!(cached_said(result, None), result, "a round that ran reads as it did");
        assert_eq!(live_line(r#"{"err":"W1: the session ended"}"#, &mut None, &mut output), Err("W1: the session ended".to_string()));
        assert_eq!(live_line(r#"{"end":true}"#, &mut None, &mut output), Err("live: no answer".to_string()), "a wait alone is no answer");
        // a cut link is asked again on a new one, once the master answers: the cut one's asks
        // were cut again at once, round after round, and the call said nothing (11-27)
        let mut out = Vec::new();
        let (mut dials, mut beats) = (0, 0);
        let got = live_retry(&mut out, || { dials += 1; Ok(dials) }, |c: &u32, _| if *c == 1 { Err("link lost: connection closed by the peer".to_string()) } else { Ok(*c * 10) }, || { beats += 1; true });
        assert_eq!((got, dials, beats), (Ok((20, 2)), 2, 1), "asked again on the second link, after one wait");
        assert_eq!(String::from_utf8_lossy(&out), "the link was cut: asking again\n", "said once");
        let got: Result<(u32, u32), String> = live_retry(&mut Vec::new(), || Ok(1), |_: &u32, _| Err("W1: the session ended".to_string()), || true);
        assert_eq!(got, Err("W1: the session ended".to_string()), "any other error ends the call");
    }

    /// A JIT miss's cargo check goes to a pool box, a macOS one to any Mac, a named box to itself.
    #[test]
    fn a_cargo_check_goes_to_the_pool_and_a_macos_one_to_any_mac() {
        let on = |node: Option<&str>, needs: &[(&str, &str)]| {
            let mut req = Req::new("lane", Kind::Test);
            (req.node, req.needs) = (node.map(str::to_string), needs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect());
            on_cargo_boxes(&mut req);
            req.needs.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(",")
        };
        assert_eq!(on(None, &[("os", "linux"), ("role", "hot")]), "os=linux,role=pool");
        assert_eq!(on(None, &[("os", "macos")]), "os=macos", "the hot Mac too");
        assert_eq!(on(Some("boxa"), &[("os", "linux")]), "os=linux");
    }

    /// A run ships what it was asked with: the worktree's files and its patch as they were when
    /// it queued, never an edit or a commit (the lane's, the merge bot's) made while it waited;
    /// with `--rev`, nothing of the worktree. Its stage goes with it (11-13).
    #[cfg(feature = "checkout")]
    #[test]
    fn a_queued_run_ships_its_tree_as_it_was_asked() {
        let repo = std::env::temp_dir().join(format!("devhub-snap-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&repo);
        std::fs::create_dir_all(&repo).unwrap();
        let git = |args: &[&str]| crate::orch::git(&repo, &[&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"][..], args].concat()).unwrap().trim().to_string();
        let write = |p: &str, text: &str| std::fs::write(repo.join(p), text).unwrap();
        write("a.rs", "fn a() {}\n");
        write("b.rs", "fn b(x: u8) {}\n");
        git(&["init", "-q", "-b", "work"]);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "base"]);
        let rev = format!("work@{}", git(&["rev-parse", "HEAD"]));
        write("a.rs", "fn a() { 1 }\n");
        let us = |p: &Path| std::fs::metadata(p).unwrap().modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_micros();
        let mtime = us(&repo.join("a.rs"));
        let patches = [Patch { file: "probe.diff".into(), repo: String::new(), diff: "--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-fn b(x: u8) {}\n+fn b(x: u8) { patched }\n".into() }];
        // another client's stages: one whose client is gone is swept, and one whose client this
        // one may not signal stays (pid 1 stands in: a sandboxed agent's run gets EPERM for every
        // process outside its sandbox, took it for gone, and swept the stages of runs waiting in
        // the queue, whose ships then failed with "No such file or directory", 11-23)
        let stage_of = |pid: u32| std::env::temp_dir().join(format!("{SNAP_PREFIX}{pid}-test-{}", std::process::id()));
        let mut ended = std::process::Command::new("true").spawn().unwrap();
        ended.wait().unwrap();
        for pid in [1, ended.id()] {
            std::fs::create_dir_all(stage_of(pid)).unwrap();
            std::fs::write(stage_of(pid).join("a.rs"), "fn a() {}\n").unwrap();
        }
        // started in a subdirectory, a run stages from its lane's top, as from the top itself
        std::fs::create_dir_all(repo.join("sub")).unwrap();
        let (top, rel) = lane_root_of(&repo.join("sub")).unwrap();
        assert_eq!((top.canonicalize().unwrap(), rel.as_str()), (repo.canonicalize().unwrap(), "sub"), "a subdirectory's run finds its lane's top");
        let from_sub = snap(&top, &rev, true, false, &[], &[]).unwrap();
        assert_eq!(std::fs::read_to_string(from_sub.stage.join("a.rs")).unwrap(), "fn a() { 1 }\n", "and stages its changed file from there");
        drop(from_sub);
        let asked = snap(&repo, &rev, true, false, &[], &patches).unwrap();
        assert!(stage_of(1).join("a.rs").is_file() && !stage_of(ended.id()).exists(), "a stage another client may still ship stays; a gone client's is swept");
        let _ = std::fs::remove_dir_all(stage_of(1));
        // while it waits: an edit, and a merge that changes b.rs and adds c.rs
        write("a.rs", "fn a() { 2 }\n");
        write("b.rs", "fn b(x: u8, y: u8) {}\n");
        write("c.rs", "fn c() {}\n");
        git(&["add", "-A"]);
        git(&["commit", "-qm", "merged while it waited"]);
        assert_eq!(asked.entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["a.rs", "b.rs"], "only the files asked for ship");
        let staged = |p: &str| std::fs::read_to_string(asked.stage.join(p)).unwrap();
        assert_eq!((staged("a.rs"), staged("b.rs")), ("fn a() { 1 }\n".to_string(), "fn b(x: u8) { patched }\n".to_string()), "queued copies survive edits and commits while waiting");
        assert_eq!(us(&asked.stage.join("a.rs")), mtime, "the file's own mtime: the node's cargo sees no new edit");
        assert_eq!(asked.patched, 1, "the patched count is files, not diff flags");
        let stage = asked.stage.clone();
        drop(asked);
        assert!(!stage.exists(), "the stage goes with the run");
        let alone = snap(&repo, &rev, true, true, &[], &[]).unwrap();
        assert!(alone.entries.is_empty() && alone.deleted.is_empty(), "--rev: the commits alone");
        drop(alone);
        // DEVHUB_ALSO with --rev: a file R tracks is refused, saying why, never the worktree's copy
        // on top of R (11-24); an untracked bench script in local/ changes none of R's and ships
        write("b.rs", "fn b() { in the worktree only }\n");
        std::fs::create_dir_all(repo.join("local")).unwrap();
        write("local/bench.sh", "echo bench\n");
        let named = ["b.rs".to_string(), "local/bench.sh".to_string()];
        let refused = snap(&repo, &rev, true, true, &named, &[]).err().expect("--rev with a DEVHUB_ALSO file R tracks must refuse: the run is R alone");
        assert!(refused.contains("DEVHUB_ALSO names b.rs, which") && refused.contains(&format!("--rev {rev}")), "the refusal names the tracked file and the revision: {refused}");
        let script = snap(&repo, &rev, true, true, &named[1..], &[]).unwrap();
        assert_eq!(script.entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["local/bench.sh"], "--rev ships an untracked local/ script, and nothing else of the worktree");
        drop(script);
        let also = snap(&repo, &rev, true, false, &named, &[]).unwrap();
        assert!(also.entries.iter().any(|e| e.path == "b.rs"), "without --rev a named tracked file ships");
        drop(also);
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// A patch uses the shipped copy, including its mode: `--rev` never borrows a later HEAD's
    /// file, and a lane's dirty file is still its base without `--rev` (11-15).
    #[cfg(feature = "checkout")]
    #[test]
    fn a_runs_patches_use_the_tree_it_ships() {
        let dir = std::env::temp_dir().join(format!("a_runs_patches_use_the_tree_it_ships-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (root, lane) = (dir.join("repo"), dir.join("lane"));
        std::fs::create_dir_all(&root).unwrap();
        let git = |args: &[&str]| crate::orch::git(&root, &[&["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"][..], args].concat()).unwrap().trim().to_string();
        let write = |p: &str, text: &str| std::fs::write(root.join(p), text).unwrap();
        write("b.rs", "fn b(x: u8) {}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.join("b.rs"), std::fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink("b.rs", root.join("link.rs")).unwrap();
            std::os::unix::fs::symlink(".", root.join("linked")).unwrap();
        }
        git(&["init", "-q", "-b", "work"]);
        git(&["add", "-A"]);
        git(&["commit", "-qm", "base"]);
        let sha = git(&["rev-parse", "HEAD"]);
        let rev = format!("work@{sha}");
        write("b.rs", "fn b(x: u8, y: u8) {}\n");
        write("new.rs", "fn later() {}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.join("b.rs"), std::fs::Permissions::from_mode(0o644)).unwrap();
            std::fs::set_permissions(root.join("new.rs"), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        git(&["add", "-A"]);
        git(&["commit", "-qm", "after base"]);
        let patch = |diff: &str| Patch { file: "probe.diff".into(), repo: String::new(), diff: diff.to_string() };
        let base = "--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-fn b(x: u8) {}\n+fn b(x: u8) { probe(); }\n";
        let alone = snap(&root, &rev, false, true, &[], &[patch(base)]).unwrap();
        assert_eq!(alone.entries.iter().map(|e| e.path.as_str()).collect::<Vec<_>>(), ["b.rs"], "--rev stages only the patched path");
        assert_eq!(std::fs::read(alone.stage.join("b.rs")).unwrap(), b"fn b(x: u8) { probe(); }\n", "--rev patches R's file, not the later HEAD's");
        #[cfg(unix)]
        assert!(alone.entries[0].exec, "--rev keeps R's executable mode despite the worktree's mode");
        drop(alone);
        let head = "--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-fn b(x: u8, y: u8) {}\n+fn b(x: u8, y: u8) { probe(); }\n";
        let refused = snap(&root, &rev, false, true, &[], &[patch(head)]).err().expect("--rev must refuse a hunk that fits only the worktree");
        assert!(refused.starts_with(&format!("--patch probe.diff: b.rs at {}: hunk 1", &sha[..10])), "a pin refusal names the diff, path and revision: {refused}");
        let main = snap(&root, &rev, false, false, &[], &[patch(base)]).unwrap();
        assert_eq!(std::fs::read(main.stage.join("b.rs")).unwrap(), b"fn b(x: u8) { probe(); }\n", "main without --with-uncommitted patches the pin too");
        drop(main);
        let added = snap(&root, &rev, false, true, &[], &[patch("--- /dev/null\n+++ b/new.rs\n@@ -0,0 +1 @@\n+fn new() {}\n")]).unwrap();
        assert_eq!(std::fs::read(added.stage.join("new.rs")).unwrap(), b"fn new() {}\n", "--rev can add a file absent at R but present in the worktree");
        assert!(!added.entries[0].exec, "a new file cannot inherit the later worktree file's executable mode");
        drop(added);
        git(&["worktree", "add", "-q", "--detach", lane.to_str().unwrap(), "HEAD"]);
        let lane_rev = format!("work@{}", git(&["rev-parse", "HEAD"]));
        std::fs::write(lane.join("b.rs"), "fn b(x: u16) {}\n").unwrap();
        let dirty = "--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-fn b(x: u16) {}\n+fn b(x: u16) { probe(); }\n";
        let shipped = snap(&lane, &lane_rev, false, false, &[], &[patch(dirty)]).unwrap();
        assert_eq!(std::fs::read(shipped.stage.join("b.rs")).unwrap(), b"fn b(x: u16) { probe(); }\n", "a lane without --rev patches its shipped dirty file");
        assert!(!shipped.entries[0].exec, "the dirty copy keeps its worktree mode");
        drop(shipped);
        let refused = snap(&lane, &lane_rev, false, false, &[], &[patch(head)]).err().expect("a shipped dirty file must not fall back to the pin");
        assert!(refused.starts_with("--patch probe.diff: b.rs in the worktree: hunk 1"), "a worktree refusal names the diff, path and source: {refused}");
        #[cfg(unix)]
        for path in ["link.rs", "linked/b.rs"] {
            let refused = super::super::ship::pin_file(&root, &super::super::Pin::at(&sha), path).unwrap_err();
            assert!(refused.starts_with(&format!("{path}: through the symlink ")) && refused.contains(&format!("in {sha}, which a ship sends as a link")), "a pin refuses a symlink at any path component: {refused}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A run in rounds: each round's command names its round, the first after a hand-over says
    /// it resumed, and the command's own `env` words join the one prefix the node reads as its
    /// environment. A timed run without rounds hears as it asks that it hands its box over only
    /// when it ends; other runs and runs in rounds hear nothing.
    #[test]
    fn a_run_in_rounds_names_its_round_and_a_timed_run_without_hears_why() {
        let cmd: Vec<String> = ["env", "A=1", "sh", "bench.sh"].iter().map(|s| s.to_string()).collect();
        assert_eq!(round_argv(&cmd, 2, 4, false), ["env", "DEVHUB_ROUND=2", "DEVHUB_ROUNDS=4", "A=1", "sh", "bench.sh"]);
        assert_eq!(round_argv(&cmd[2..], 3, 4, true), ["env", "DEVHUB_ROUND=3", "DEVHUB_ROUNDS=4", "DEVHUB_RESUMED=1", "sh", "bench.sh"]);
        assert_eq!(super::super::loaders::command_of(&round_argv(&cmd, 1, 4, true)), &cmd[2..], "the node runs the command itself");
        let timed = parse("timed", Some("lane-a"), &["--", "sh", "bench.sh"]).unwrap().req;
        assert!(rounds_note(&timed).is_some_and(|n| n.contains("only when it ends") && n.contains("--rounds N")), "{:?}", rounds_note(&timed));
        let rounds = parse("timed", Some("lane-a"), &["--rounds", "4", "--", "sh", "bench.sh"]).unwrap().req;
        assert_eq!((rounds.rounds, rounds_note(&rounds)), (Some(4), None));
        assert_eq!(rounds_note(&parse("build", Some("lane-a"), &["--", "cargo", "build"]).unwrap().req), None);
        assert!(parse("timed", Some("lane-a"), &["--rounds", "0", "--", "sh"]).is_err());
        // told to yield during round 2 of 4: it hands the box over after it, and the next lease
        // runs rounds 3 and 4, the first of them resumed
        let cmd = &cmd[2..];
        let ran = std::cell::RefCell::new(Vec::new());
        let round = |k: u32, argv: &[String]| { ran.borrow_mut().push((k, argv.iter().any(|a| a == "DEVHUB_RESUMED=1"))); Ok(0) };
        let r = rounds_until((1, 4), cmd, round, || ran.borrow().len() == 2);
        assert_eq!((r, ran.take()), (Ok((0, Some(2))), vec![(1, false), (2, false)]));
        let r = rounds_until((3, 4), cmd, round, || false);
        assert_eq!((r, ran.take()), (Ok((0, None)), vec![(3, true), (4, false)]));
        let r = rounds_until((4, 4), cmd, round, || true);
        assert_eq!((r, ran.take()), (Ok((0, None)), vec![(4, true)]), "told at its last round: done, no hand-over");
        // a failed round ends the run with its exit, told or not
        let r = rounds_until((1, 4), cmd, |k, _| Ok(if k == 2 { 3 } else { 0 }), || false);
        assert_eq!(r, Ok((3, None)));
    }

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
        // a repeated list flag adds to the ones before it, or is refused saying both: never the
        // last alone (`--avoid <a> --avoid <b>` kept only `<b>`, and the lease ran on `<a>`, 11-21)
        let req = parse("build", Some("lane-a"), &["--avoid", "box-a", "--avoid", "box-b,box-a", "--needs", "os=linux", "--needs", "gpu=1,os=linux", "--node", "box-c", "--node", "box-c"]).unwrap().req;
        let pairs = |p: &[(&str, &str)]| p.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>();
        assert_eq!((req.avoid, req.needs, req.node.as_deref()), (vec!["box-a".to_string(), "box-b".to_string()], pairs(&[("os", "linux"), ("gpu", "1")]), Some("box-c")));
        assert_eq!(parse("build", Some("lane-a"), &["--node", "box-a", "--node", "box-b"]).err(), Some("--node box-b after --node box-a: a lease names one box (--avoid keeps it off others)".to_string()));
        assert!(parse("build", Some("lane-a"), &["--needs", "os=linux", "--needs", "os=macos"]).is_err_and(|e| e.contains("--needs os=macos after os=linux")));
        // a command that reads its script from stdin is refused: its run gets none (11-21)
        for cmd in [&["bash", "/dev/stdin", "a"][..], &["/bin/sh", "-s", "x"], &["bash"], &["sh", "-e"], &["python3", "/dev/fd/0"]] {
            let args: Vec<&str> = ["--"].into_iter().chain(cmd.iter().copied()).collect();
            assert!(parse("build", Some("lane-a"), &args).is_err_and(|e| e.contains("gets no stdin") && e.contains("ship the script as a file")), "{cmd:?}");
        }
        for cmd in [&["bash", "-c", "true"][..], &["sh", "-ec", "true"], &["bash", "tools/x.sh"], &["sh", "-e", "x.sh"], &["cat", "-s"]] {
            let args: Vec<&str> = ["--"].into_iter().chain(cmd.iter().copied()).collect();
            assert!(parse("build", Some("lane-a"), &args).is_ok(), "{cmd:?}");
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

    /// `run --patch` flags give unapplied rows; the command's own `--patch` stays. Applying them
    /// to shipped worktree files keeps their mode, even if it changes before staging (11-15).
    #[cfg(feature = "checkout")]
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
        let text = "--- a/tools/rig.sh\n+++ b/tools/rig.sh\n@@ -1,2 +1,3 @@\n echo one\n+echo probe\n echo two\n";
        std::fs::write(&diff, text).unwrap();
        let diff = diff.to_string_lossy().into_owned();
        let mut rest = vec!["--patch", diff.as_str(), "--cores", "2", "--", "bash", "--patch", "x"];
        let patches = take_patches(Some((&root, ".")), &mut rest).unwrap();
        assert_eq!(rest, ["--cores", "2", "--", "bash", "--patch", "x"], "only run's patch flags come out");
        assert_eq!(patches, [Patch { file: diff.clone(), repo: String::new(), diff: text.to_string() }], "flags retain the diff without applying it");
        let pin = super::super::Pin::default();
        let files = ["tools/rig.sh".to_string()];
        let patched = apply_patches(&root, &pin, &files, &[], &patches).unwrap();
        assert_eq!(patched[0].1, b"echo one\necho probe\necho two\n", "the diff applies to the shipped file");
        let mut twice = vec!["--patch", diff.as_str(), "--patch", diff.as_str(), "--", "x"];
        let twice = take_patches(Some((&root, "tools")), &mut twice).unwrap();
        assert!(apply_patches(&root, &pin, &files, &[], &twice).unwrap_err().ends_with("tools/rig.sh is in another patch too"), "overlapping patches are refused when applied");
        // from inside a nested checkout, its diff's paths are that repository's (as `git diff`
        // wrote them there); a run started beside it, in makepad, reads them as makepad's
        std::fs::create_dir_all(root.join("apps/side-app/.git")).unwrap();
        std::fs::create_dir_all(root.join("apps/side-app/tools")).unwrap();
        std::fs::write(root.join("apps/side-app/tools/rig.sh"), "echo one\necho two\n").unwrap();
        for rel in ["apps/side-app", "apps/side-app/tools"] {
            let mut rest = vec!["--patch", diff.as_str(), "--", "x"];
            let patches = take_patches(Some((&root, rel)), &mut rest).unwrap();
            assert_eq!(patches, [Patch { file: diff.clone(), repo: "apps/side-app/".to_string(), diff: text.to_string() }], "{rel}: flags keep the nested repository prefix");
            let files = ["apps/side-app/tools/rig.sh".to_string()];
            let patched = apply_patches(&root, &pin, &files, &[], &patches).unwrap();
            assert_eq!((&patched[0].0, &patched[0].1), (&files[0], &b"echo one\necho probe\necho two\n".to_vec()), "{rel}: a nested diff applies inside its repository");
        }
        let mut rest = vec!["--patch", diff.as_str(), "--", "x"];
        assert_eq!(take_patches(Some((&root, "apps")), &mut rest).unwrap()[0].repo, "", "beside a nested repository the diff belongs to makepad");
        let stage = dir.join("stage");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(root.join("tools/rig.sh"), std::fs::Permissions::from_mode(0o644)).unwrap();
        }
        let staged = stage_patched(&stage, &patched).unwrap();
        assert_eq!(staged.iter().map(|e| (e.path.as_str(), e.size)).collect::<Vec<_>>(), [("tools/rig.sh", 29)], "the patched script is staged once");
        assert_eq!(std::fs::read(stage.join("tools/rig.sh")).unwrap(), patched[0].1, "the staged bytes are the patched bytes");
        #[cfg(unix)]
        assert!(staged[0].exec, "the staged script keeps its mode");
        assert!(stage_patched(&stage, &[("local/x.sh".into(), Vec::new(), 0o644)]).unwrap_err().contains("never ships"), "excluded paths still cannot ship patched");
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
        // an exact look's: a program's waited-for children, the hub and what no process showed
        // are figures beside it, and only the programs outside it are its verdict
        let exact = makepad_strict_json::parse(br#"{"samples":4,"secs":40,"load1_max":2.3,"outside_cpu_pct":30,"outside_cpu_pct_max":45,"busiest":[["makepad-app-aihub (under makepad-app-wm)",17,13],["makepad-app-wm",5,0]],"hub_cpu_pct":0.7,"unseen_cpu_pct":6.2,"irq_cpu_pct":4.2}"#).unwrap();
        assert_eq!(super::super::master::box_line(&exact), "load1 max 2.3; outside it 30% of a core (max 45%): makepad-app-aihub (under makepad-app-wm) 17% (13% in its child processes), makepad-app-wm 5%; its hub 1%; no process showed 6% (interrupts 4%)");
        assert_eq!(unquiet(&exact), None, "30% of a core outside it: quiet, its hub and the unseen apart");
    }

    /// `--keep` says what came back, an empty copy too (an empty one said nothing, 11-07).
    #[test]
    fn a_keep_says_what_it_brought_back_or_that_it_was_nothing() {
        let dst = Path::new("grabs");
        assert_eq!(keep_said("L7", dst, &Ok(Vec::new())), "L7: kept nothing in grabs: the run put no file in $DEVHUB_KEEP");
        assert_eq!(keep_said("L7", dst, &Ok(vec![(dst.join("a.png"), 1 << 20), (dst.join("b.png"), 1 << 19)])), "L7: kept 2 files (1.5 MB) in grabs");
        assert_eq!(keep_said("L7", dst, &Err("node-a: no lease L7".into())), "L7: --keep grabs: nothing kept: node-a: no lease L7");
    }

    /// Kept files go nowhere in a checkout, a lane's worktree or the main one, but the main one's
    /// `local/`, which the refusal names: `--keep` relative in a worktree left a `target/` in an
    /// app's folder (11-26).
    #[test]
    fn kept_files_go_to_the_main_checkouts_local_never_into_a_checkout() {
        let dir = std::env::temp_dir().join(format!("devhub-keep-dest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (main, wt) = (dir.join("main"), dir.join("wt"));
        for d in [main.join(".git/worktrees/wt"), main.join("local"), main.join("apps/app/src"), main.join("apps/nested/.git"), wt.join("apps/app")] {
            std::fs::create_dir_all(d).unwrap();
        }
        let main = main.canonicalize().unwrap();
        std::fs::write(wt.join(".git"), format!("gitdir: {}\n", main.join(".git/worktrees/wt").display())).unwrap();
        std::fs::write(main.join(".git/worktrees/wt/commondir"), "../..\n").unwrap();
        let wt = wt.canonicalize().unwrap();
        let place = format!("keep it under {}/local/agent_state/lane-a/runs/", main.display());
        let refused = |dst: &str, cwd: &Path, root: &Path| {
            let why = kept_into_checkout(Path::new(dst), cwd, "lane-a").unwrap_or_else(|| panic!("{dst} from {} is kept into a checkout", cwd.display()));
            assert_eq!(why, format!("{dst} is in the checkout {}: a file kept there can be committed or read as a build's input; {place}", root.display()));
        };
        refused("apps/app/target/makepad_test/L1", &wt, &wt);
        refused(&wt.join("apps/app").display().to_string(), &dir, &wt);
        refused("local/agent_state/lane-a/runs", &wt, &wt);
        refused("apps/app/src", &main, &main);
        refused("local/../apps/app", &main, &main);
        refused("apps/nested/out", &main, &main.join("apps/nested"));
        for (dst, cwd) in [("local/agent_state/lane-a/runs/L1", &main), ("../main/local/x", &wt), ("out", &dir)] {
            assert_eq!(kept_into_checkout(Path::new(dst), cwd, "lane-a"), None, "{dst} from {}: the main checkout's local/ and outside every checkout take kept files", cwd.display());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn keep_flag_sets_abs_req_keep_dir_and_streaming_fetch_is_atomic() {
        let base = std::env::temp_dir().join(format!("devhub-get-stream-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        let base = base.canonicalize().unwrap();
        let dst = base.join("result");
        let arg = base.join("unused/../result");
        let opts = parse("build", Some("keep"), &["--keep", arg.to_str().unwrap()]).unwrap();
        assert_eq!(opts.keep.as_deref(), Some(dst.as_path()));
        assert_eq!(Req::parse(&opts.req.to_json()).unwrap().keep_dir.as_deref(), dst.to_str());

        let fetch = |ask: &dyn Fn(&str, &str) -> Result<Vec<String>, String>| fetch_keep_from(ask, "box-a", "keep", "L1", &dst);
        assert!(fetch(&|_, _| Ok(vec![])).unwrap().is_empty() && !dst.exists());
        std::fs::create_dir(&dst).unwrap();
        let out = dst.join("report");
        std::fs::write(&out, "old").unwrap();
        let chunk = vec![b'x'; 256 << 10];
        let listing = r#"{"p":"keep/report","r":"tmp/keep/report","n":262145}"#;
        let copy = |last: &str| fetch(&|service, line| {
            if service == "get-ls" { return Ok(vec![listing.into()]); }
            assert_eq!(std::fs::read(&out).unwrap(), b"old", "the previous complete file stays readable during the copy");
            if line == "get-read keep L1 0 tmp/keep/report" {
                return Ok(vec![format!(r#"{{"data":"{}","eof":false}}"#, makepad_network::digest::base64_encode(&chunk))]);
            }
            assert_eq!(line, "get-read keep L1 262144 tmp/keep/report");
            let temp = std::fs::read_dir(&dst).unwrap().flatten().find(|e| e.file_name().to_string_lossy().starts_with(".devhub-get-")).unwrap();
            assert_eq!(temp.metadata().unwrap().len(), chunk.len() as u64, "the first chunk is already on disk");
            Ok(vec![last.into()])
        });
        assert!(copy(r#"{"err":"read failed"}"#).unwrap_err().contains("read failed"));
        for missing in [r#"{"err":"gone","missing":true}"#, r#"{"err":"No such file or directory"}"#, r#"{"err":"path (os error 3)"}"#] {
            assert!(copy(missing).unwrap().is_empty());
        }
        assert!(copy(r#"{"err":"no live lease (os error 2)","missing":true}"#).is_err());
        assert_eq!(std::fs::read_dir(&dst).unwrap().count(), 1, "failed and vanished reads remove their temp");
        assert_eq!(copy(r#"{"data":"eg==","eof":true}"#).unwrap(), [(out.clone(), 262145)]);
        assert_eq!(std::fs::read(&out).unwrap(), [chunk.as_slice(), b"z"].concat());
        assert!(fetch(&|service, _| {
            assert_eq!(service, "get-ls", "a listing error aborts before the first read");
            Ok(vec![listing.into(), r#"{"err":"listing failed"}"#.into()])
        }).is_err());
        #[cfg(unix)]
        {
            let link = dst.join("link");
            std::os::unix::fs::symlink(&base, &link).unwrap();
            for path in ["link", "link/out", "../escape"] {
                assert!(fetch(&|service, _| {
                    assert_eq!(service, "get-ls");
                    Ok(vec![format!(r#"{{"p":"keep/{path}","r":"tmp/keep/{path}"}}"#)])
                }).is_err());
            }
            assert!(fetch_keep_from(&|_, _| panic!("symlink root"), "box-a", "keep", "L1", &link).is_err());
        }
        std::fs::remove_dir_all(base).unwrap();
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

    /// An explicit hand-in from inside a lane's commercial checkout (11-15): its makepad half is
    /// the makepad worktree's branch, never the nested repository's of the same name.
    #[test]
    #[cfg(feature = "checkout")]
    fn a_hand_ins_makepad_half_is_the_makepad_worktrees_branch_from_a_nested_checkout() {
        let d = std::env::temp_dir().join(format!("devhub-handin-half-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let inner = d.join("lane/apps/commercial/fab");
        std::fs::create_dir_all(&inner).unwrap();
        let mut shas = Vec::new();
        for repo in [d.join("lane"), d.join("lane/apps/commercial")] {
            let g = |args: &[&str]| git(&repo, &[&["-c", "core.hooksPath=/dev/null", "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"][..], args].concat());
            assert!(g(&["init", "-q", "-b", "rc-x"]).is_some(), "git init {}", repo.display());
            // a message of its own: two empty commits of one second and author are one sha
            assert!(g(&["commit", "-q", "--allow-empty", "-m", &format!("c{}", shas.len())]).is_some(), "git commit {}", repo.display());
            shas.push(g(&["rev-parse", "HEAD"]).unwrap());
        }
        assert_ne!(shas[0], shas[1], "the two repositories' rc-x are two commits");
        assert_eq!(git(&inner, &["rev-parse", "--verify", "refs/heads/rc-x^{commit}"]).as_ref(), Some(&shas[1]), "the shell's own toplevel there is the commercial repository");
        assert_eq!(makepad_branch_sha(&inner, "rc-x").as_ref(), Some(&shas[0]), "from inside the commercial checkout, the hand-in's makepad half is makepad's rc-x");
        assert_eq!(makepad_branch_sha(&d.join("lane"), "rc-x").as_ref(), Some(&shas[0]), "and from the makepad worktree");
        assert_eq!(makepad_branch_sha(&inner, "rc-none"), None, "a branch makepad lacks resolves to nothing (the master refuses it)");
        let _ = std::fs::remove_dir_all(&d);
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

    /// Individually valid lanes can break together; check the merged dependent, not the old tree.
    #[cfg(feature = "checkout")]
    #[test]
    fn a_hand_in_is_checked_with_its_dependents_at_its_merge_onto_the_tips() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dependents = |root: &Path, lane: &str, rev: &str, check: &(dyn Fn(&crate::jit_misses::CheckRun, &str) -> (i32, String) + Sync)| {
            dependents_gate(root, rev, &merged_onto_tips(root, lane, rev)?, check)
        };
        let root = std::env::temp_dir().join(format!("devhub-dependents-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("crates/a/src")).unwrap();
        std::fs::create_dir_all(root.join("crates/b/src")).unwrap();
        let g = |args: &[&str]| git(&root, &[&["-c", "core.hooksPath=/dev/null"][..], args].concat()).expect("fixture git succeeds");
        let write = |path: &str, text: &str| std::fs::write(root.join(path), text).unwrap();
        g(&["init", "-q", "-b", "work"]);
        g(&["config", "user.name", "t"]);
        g(&["config", "user.email", "t@t"]);
        g(&["config", "commit.gpgsign", "false"]);
        write("Cargo.toml", "[workspace]\nmembers = [\"crates/*\", \"apps/*/src/..\"]\n");
        write("crates/a/Cargo.toml", "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        write("crates/b/Cargo.toml", "[package]\nname = \"b\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\na = { path = \"../a\" }\n");
        write("crates/a/src/lib.rs", "pub fn f() {}\n");
        // Separate the calls so git merges the edits cleanly, leaving the compiler to catch it.
        let b = "pub fn first() {\n    a::f();\n}\n\npub fn middle() {\n    let value = 1;\n    let _ = value;\n}\n\npub fn second() {\n}\n";
        write("crates/b/src/lib.rs", b);
        g(&["add", "."]);
        g(&["commit", "-qm", "base"]);
        g(&["branch", "rc-t"]);
        write("crates/a/src/lib.rs", "pub fn f(x: u32) {}\n");
        write("crates/b/src/lib.rs", &b.replace("a::f()", "a::f(1)"));
        g(&["commit", "-qam", "interface and caller"]);
        let work = g(&["rev-parse", "work"]);
        g(&["checkout", "-q", "rc-t"]);
        write("crates/b/src/lib.rs", &b.replace("pub fn second() {\n", "pub fn second() {\n    a::f();\n"));
        g(&["commit", "-qam", "another caller"]);
        let lane = g(&["rev-parse", "rc-t"]);
        let seen = std::sync::Mutex::new(Vec::new());
        let error = "crates/b/src/lib.rs:11:5: error[E0061]: this function takes 1 argument but 0 arguments were supplied";
        let refused = dependents(&root, "rc-t", &format!("rc-t@{lane}"), &|run, rev| {
            seen.lock().unwrap().push((run.packages.clone(), rev.to_string()));
            let pin = super::super::Pin::of_rev(rev).expect("the checker receives a pinned rev");
            let a = g(&["show", &format!("{}:crates/a/src/lib.rs", pin.makepad)]);
            let b = g(&["show", &format!("{}:crates/b/src/lib.rs", pin.makepad)]);
            if a.contains("f(x: u32)") && b.contains("a::f()") { (1, error.into()) } else { (0, String::new()) }
        }).expect_err("the caller must fail against the new interface");
        assert!(refused.iter().any(|l| l.starts_with("dependents: b fail at the merge")), "the refusal names the dependent: {refused:?}");
        assert!(refused.iter().any(|l| l.trim() == error), "the refusal includes the compiler's error line: {refused:?}");
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.len(), 2, "a compiler failure is not retried, only asked at the tips without the hand-in: {seen:?}");
        assert_eq!(seen[0].0, ["b"], "only the touched dependent is checked");
        let tips = super::super::Pin::of_rev(&seen[1].1).expect("the tips are pinned as the merge is");
        assert_eq!((seen[1].0.clone(), tips.makepad.clone()), (vec!["b".to_string()], work.clone()), "the failing run is asked again at work's tip, without the hand-in, where it passes: refused");
        let (mk, nested) = super::super::parse_pinned(&seen[0].1).unwrap();
        assert_eq!(mk.branch, "rc-t", "the merged rev preserves the lane's branch name");
        assert_eq!(g(&["rev-parse", &format!("{}^1", mk.sha)]), work, "the merge's first parent is the current work tip");
        assert_eq!(g(&["rev-parse", &format!("{}^2", mk.sha)]), lane, "the merge's second parent is the hand-in");
        assert!(nested.is_empty() && super::super::Pin::of_rev(&seen[0].1).unwrap().pair().main.is_empty(), "no commercial checkout leaves the second side empty");
        assert_eq!(g(&["rev-parse", "work"]), work, "the gate never moves work");
        assert_eq!(g(&["rev-parse", "rc-t"]), lane, "the gate never moves the lane");
        // the same error red at the tips without the hand-in, at another line: the buckets' to fix
        let moved = "crates/b/src/lib.rs:4:5: error[E0061]: this function takes 1 argument but 0 arguments were supplied";
        let gate = dependents(&root, "rc-t", &format!("rc-t@{lane}"), &|_, rev| {
            let at_tips = super::super::Pin::of_rev(rev).unwrap().makepad == work;
            (1, if at_tips { moved.into() } else { error.into() })
        }).expect("an error already red at the tips refuses nothing");
        assert!(gate.notes.contains(&red_note("dependents", "b", 1)), "it is said, not refused: {:?}", gate.notes);

        g(&["checkout", "-qb", "rc-body", "work"]);
        write("crates/a/src/lib.rs", "pub fn f(x: u32) { let _ = x; }\n");
        g(&["commit", "-qam", "body only"]);
        let rev = format!("rc-body@{}", g(&["rev-parse", "HEAD"]));
        let seen = std::sync::Mutex::new(Vec::new());
        let gate = dependents(&root, "rc-body", &rev, &|run, _| {
            seen.lock().unwrap().push(run.packages.clone());
            (0, "devhub test: check of a b passed in 0.1s on n1\n".into())
        }).expect("a body change with passing dependents is checked");
        assert_eq!(seen.into_inner().unwrap(), [vec!["a".to_string(), "b".to_string()]], "wide includes every touched crate until step 5 narrows it");
        assert_eq!(gate.words, [format!("checked={work},"), "touched=a".into(), "wide=a".into(), "set=a,b".into(), "node=n1".into()], "the train receives the tips, scope and run's node");
        assert!(gate.notes.last().unwrap().starts_with(&format!("dependents: 2 crate(s) checked at the merge with work {work:.10} in ")), "notes end with the checked count, tip and duration");
        let calls = AtomicUsize::new(0);
        let gate = dependents(&root, "rc-body", &rev, &|_, _| {
            calls.fetch_add(1, Ordering::Relaxed);
            (2, not_run_line("check of a b", "no box=available"))
        }).expect("a check without a verdict is left to the buckets");
        assert_eq!(calls.load(Ordering::Relaxed), 2, "a run without a verdict is tried exactly twice");
        assert!(gate.words.contains(&"unchecked=no_box_available".to_string()) && !gate.words.iter().any(|w| w.starts_with("checked=")), "unchecked reasons replace checked pairs and escape spaces and equals");
        assert!(gate.notes.iter().any(|n| n == "dependents: a, b not checked (no box=available): the buckets check them after the landing"), "unchecked notes name the packages and reason: {:?}", gate.notes);
        // past the bound by its estimate, the set goes to the buckets, and the crate the hand-in
        // touches is checked alone: its own warning refuses it (#6704, #6712, 11-18)
        let warning = "crates/a/src/lib.rs:1:10: warning: unused variable: `y`";
        let seen = std::sync::Mutex::new(Vec::new());
        let past = |run: &crate::jit_misses::CheckRun, rev: &str, own: &str| {
            seen.lock().unwrap().push(run.packages.clone());
            let at_tips = super::super::Pin::of_rev(rev).unwrap().makepad == work;
            match run.packages.len() {
                2 => (2, not_run_line("check of a b", &format!("estimated 9m14s, past the 2m00s {PAST_GATE_BOUND}"))),
                _ if at_tips => (0, String::new()),
                _ => (0, own.to_string()),
            }
        };
        let refused = dependents(&root, "rc-body", &rev, &|run, rev| past(run, rev, warning)).expect_err("the touched crate's own warning refuses the hand-in");
        assert!(refused.iter().any(|l| l.starts_with("dependents: a fail at the merge")) && refused.iter().any(|l| l.trim() == warning), "{refused:?}");
        assert!(seen.lock().unwrap().contains(&vec!["a".to_string()]), "the touched crate is checked alone: {:?}", seen.lock().unwrap());
        let gate = dependents(&root, "rc-body", &rev, &|run, rev| past(run, rev, "")).expect("clean alone, it lands, its set left to the buckets");
        assert!(gate.notes.iter().any(|n| n.starts_with("dependents: a, b not checked (estimated 9m14s")) && gate.notes.iter().any(|n| n == "dependents: a of them checked alone, the crates the hand-in touches"), "{:?}", gate.notes);

        g(&["checkout", "-qb", "rc-docs", "work"]);
        write("README.md", "documentation\n");
        g(&["add", "README.md"]);
        g(&["commit", "-qm", "docs"]);
        let gate = dependents(&root, "rc-docs", &format!("rc-docs@{}", g(&["rev-parse", "HEAD"])), &|_, _| panic!("docs-only changes must not run a check"))
            .expect("docs-only changes have an empty checked scope");
        assert_eq!(gate.words, [format!("checked={work},"), "touched=".into(), "wide=".into(), "set=".into()], "docs-only changes still name their checked pair and empty lists");
        assert!(gate.notes.last().unwrap().starts_with("dependents: 0 crate(s) checked"), "an empty scope still has a final timing note");

        // A followed nested repository the lane's tree lacks (Source Library: #7425 broke its build
        // with 0 crates of it checked, 11-19): its crates the hand-in reaches are in the set, checked
        // at its landed tip, and past the bound checked alone beside the touched ones.
        let extra = root.join("apps/extra");
        std::fs::create_dir_all(extra.join("src")).unwrap();
        let x = |args: &[&str]| git(&extra, &[&["-c", "core.hooksPath=/dev/null", "-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false"][..], args].concat()).expect("fixture git succeeds");
        x(&["init", "-q", "-b", "main"]);
        std::fs::write(extra.join("Cargo.toml"), "[package]\nname = \"c\"\nversion = \"0.1.0\"\nedition = \"2021\"\n[dependencies]\na = { path = \"../../crates/a\" }\n").unwrap();
        std::fs::write(extra.join("src/lib.rs"), "pub fn g() {\n    a::f(1);\n}\n").unwrap();
        x(&["add", "."]);
        x(&["commit", "-qm", "extra"]);
        let extra_tip = x(&["rev-parse", "main"]);
        let seen = std::sync::Mutex::new(Vec::new());
        let gate = dependents(&root, "rc-body", &rev, &|run, rev| {
            seen.lock().unwrap().push((run.packages.clone(), rev.to_string()));
            (0, String::new())
        }).expect("a passing set lands");
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.iter().map(|(p, _)| p.join(",")).collect::<Vec<_>>(), ["a,b,c"], "the other repository's dependent is in the one run of the set");
        assert_eq!(super::super::Pin::of_rev(&seen[0].1).unwrap().nested.get("extra"), Some(&extra_tip), "checked at its landed tip: {}", seen[0].1);
        assert!(gate.words.contains(&"set=a,b,c".to_string()), "{:?}", gate.words);
        let seen = std::sync::Mutex::new(Vec::new());
        let gate = dependents(&root, "rc-body", &rev, &|run, _| {
            seen.lock().unwrap().push(run.packages.join(","));
            if run.packages.len() == 3 { (2, not_run_line("check of a b c", &format!("estimated 2h14m, past the 2m00s {PAST_GATE_BOUND}"))) } else { (0, String::new()) }
        }).expect("clean alone, it lands");
        assert_eq!(seen.into_inner().unwrap().last().map(String::as_str), Some("a,c"), "past the bound, the touched crate and the other repository's are checked alone");
        assert!(gate.notes.iter().any(|n| n == "dependents: a, c of them checked alone, the crates the hand-in touches and its dependents in other repositories"), "{:?}", gate.notes);
        // the boundary audit of the app it reaches, at the merge: a row it brings refuses it, with
        // the row, its way in and the line that runs it alone; a row the tips already show, a
        // clean audit and one let go at its start bound refuse nothing (#10089, 11-27)
        let (merge, tips) = ("rc-t@m", "work@w");
        let row = |at: &str| format!("violation\tFAIL\tmakepad_s\tmakepad_s::Heap::field_type\t{at}/platform/s/src/heap.rs:108:1: a field of `Heap` (`objects: Vec<Obj>`) is reached from JIT-compiled code\n  reached from x::Widget::script_apply (a derive's expansion)\n");
        let asked = std::sync::Mutex::new(Vec::new());
        let audit = |on_merge: String, on_tips: String| {
            asked.lock().unwrap().clear();
            boundary_gate("makepad-app-x", "apps/x", merge, tips, "0123456789abc", &|run, rev| {
                asked.lock().unwrap().push((run.class.clone(), run.cmd[run.cmd.len() - 4..].join(" "), rev.to_string()));
                let log = if rev == merge { on_merge.clone() } else { on_tips.clone() };
                (if log.contains("FAIL") { 1 } else { 0 }, format!("# lowered 912 bodies\n{log}"))
            })
        };
        let refused = audit(row("/n/lane"), String::new()).expect_err("a violation the hand-in brings refuses it");
        assert!(refused[0].starts_with("boundary: makepad-app-x fails its audit at the merge with work 0123456789: 1 row(s)"), "the refusal says what failed: {refused:?}");
        assert!(refused[1].starts_with("  violation\tFAIL\tmakepad_s\tmakepad_s::Heap::field_type\t") && refused[2] == "    reached from x::Widget::script_apply (a derive's expansion)", "the row and its way in: {refused:?}");
        assert_eq!(refused.last().unwrap(), "GATEREPRO boundary makepad-app-x :: cd apps/x && makepad check --boundary all makepad-app-x", "the line that runs it alone");
        assert_eq!(asked.lock().unwrap().clone(), [("boundary:makepad-app-x".to_string(), "check --boundary all makepad-app-x".to_string(), merge.to_string()), ("boundary:makepad-app-x".into(), "check --boundary all makepad-app-x".into(), tips.into())], "audited at the merge, then at the tips for what was red before it");
        let gate = audit(row("/n/lane"), row("/m/other").replace(":108:1:", ":97:5:")).expect("a row already red at the tips, on another tree and line, is not its own");
        assert_eq!(gate, ["boundary: makepad-app-x: 1 failing row(s) already at the tips without the hand-in: not its own, the buckets say them"]);
        let gate = audit(String::new(), String::new()).expect("a clean audit");
        assert!(gate[0].starts_with("boundary: makepad-app-x audited at the merge with work 0123456789 in ") && asked.lock().unwrap().len() == 1, "said, and not asked at the tips: {gate:?}");
        let deferred = boundary_gate("makepad-app-x", "apps/x", merge, tips, "0123456789abc", &|_, _| (DEFERRED_EXIT, not_run_line("check of makepad-app-x", &deferred_said("L7", Duration::from_secs(46), Duration::from_secs(44)))))
            .expect("an audit that did not start in its start bound goes after the landing");
        assert!(deferred[0].starts_with("boundary: makepad-app-x not audited (L7 had not started 46 s after it asked") && deferred[0].ends_with("): the buckets audit it after the landing"), "{deferred:?}");
        assert_eq!(boundary_rows("option-entry\tFAIL\tmakepad_s\tOption<Obj>\tadd it to option_layouts!\nviolation\tok\tmakepad_s\ta::b\tfine\n").len(), 1, "a failed option entry is a failing row; a row that passed is none");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// #4787: a module behind a default feature broke the crate's featureless dependents (11-15).
    #[cfg(feature = "checkout")]
    #[test]
    fn a_hand_in_whose_featureless_dependent_breaks_is_refused_naming_the_set() {
        let tree = crate::orch::ci::Tree::mem(vec![
            ("Cargo.toml", "[workspace]\nmembers = [\"core\", \"panel\", \"app\"]\n"),
            ("core/Cargo.toml", "[package]\nname = \"core\"\n[features]\ndefault = [\"checkout\"]\ncheckout = []\n"),
            ("core/src/lib.rs", "#[cfg(feature = \"checkout\")]\npub mod lanes;\npub mod land;\n"),
            ("panel/Cargo.toml", "[package]\nname = \"panel\"\n[dependencies]\ncore = { path = \"../core\", default-features = false }\n"),
            ("app/Cargo.toml", "[package]\nname = \"app\"\n[dependencies]\ncore = { path = \"../core\" }\n"),
        ]);
        let checks = feature_checks(&["core".into(), "app".into()], &crate::orch::ci::dependent_feature_sets(&tree));
        let said: Vec<(&str, String, &str)> = checks.iter().map(|(p, s, by)| (p.as_str(), s.said(), by.as_str())).collect();
        assert_eq!(said, [("core", "--no-default-features".to_string(), "panel")], "only the touched package a dependent builds another way owes a check, once per set");
        let broken = "core/src/land.rs:3:31: error[E0433]: cannot find `lanes` in `crate`: could not find `lanes` in the crate root\n";
        let refused = feature_verdict(&checks, &[(101, broken.into())], &[]).unwrap_err();
        assert!(refused[0].starts_with("core --no-default-features (as panel builds it): 1 error"), "the refusal names the package, the set and the dependent: {refused:?}");
        assert!(refused.iter().any(|l| l.contains("core/src/land.rs:3:31") && l.contains("E0433")), "and says the error where it is: {refused:?}");
        assert_eq!(feature_verdict(&checks, &[(0, "relax check: 0 errors\n".into())], &[]), Ok(vec!["core --no-default-features (as panel builds it): ok".to_string()]), "a clean check passes the hand-in");
        let lost = feature_verdict(&checks, &[(2, "devhub test: check of core not run: no box took it\n".into())], &[]).expect("a check that never ran does not refuse");
        assert!(lost[0].contains("not checked (no box took it)"), "it is said as unchecked, with why: {lost:?}");
        let gone = feature_verdict(&checks, &[(LOST_EXIT, String::new())], &[]).expect("a check whose lease was lost does not refuse");
        assert!(gone[0].contains("not checked (exit 4: its lease was lost under it, no verdict)"), "a lost lease is said by name, never as `no error line`: {gone:?}");
        // the same error already red at the tips without the hand-in, at another line: not its own
        let red: Red = [crate::bucket::error_key("/elsewhere/core/src/land.rs:9:1: error[E0433]: cannot find `lanes` in `crate`: could not find `lanes` in the crate root").unwrap()].into();
        let excused = feature_verdict(&checks, &[(101, broken.into())], &[red.clone()]).expect("an error already red at the tips refuses nothing");
        assert_eq!(excused, [red_note("feature sets", "core --no-default-features (as panel builds it)", 1)], "{excused:?}");
        let mine = "core/src/land.rs:4:1: error[E0425]: cannot find function `x` in this scope\n";
        let refused = feature_verdict(&checks, &[(101, format!("{broken}{mine}"))], &[red]).unwrap_err();
        assert!(refused[0].starts_with("core --no-default-features (as panel builds it): 1 error") && refused.iter().any(|l| l.contains("E0425")) && !refused.iter().any(|l| l.contains("E0433")), "only the hand-in's own line refuses: {refused:?}");
    }

    /// A JIT test run that passed with a warning in a file the lane changed fails, each warning
    /// listed at its end; others' warnings are listed and fail nothing (#6704, #6712, 11-18).
    #[test]
    fn a_passing_run_with_the_lanes_own_warning_fails() {
        let warned = vec![("src/a.rs:1:1: warning: unused variable: `z`".to_string(), true), ("src/b.rs:2:3: warning: unused import: `y`".to_string(), false)];
        let (lines, code) = warnings_verdict(&warned, 0);
        assert_eq!(code, 1, "the lane's own warning fails a run that passed");
        assert_eq!(lines[0], "devhub test: 2 warning(s), 1 in files this lane changed");
        assert!(lines[1] == "  [this lane] src/a.rs:1:1: warning: unused variable: `z`" && lines[2] == "  src/b.rs:2:3: warning: unused import: `y`", "{lines:?}");
        assert!(lines[3].starts_with("devhub test: FAILED: 1 warning(s) in files this lane changed"), "{lines:?}");
        assert_eq!(warnings_verdict(&warned[1..], 0).1, 0, "another's warning is said, and fails nothing");
        assert_eq!(warnings_verdict(&warned, 101).1, 101, "a failed run keeps its own exit");
        assert_eq!(warnings_verdict(&[], 0), (Vec::new(), 0));
    }

    /// A hand-in's gate run runs before the landing only when cheap by the master's estimate; a
    /// wide one (a re-check of 305 crates held 19 hand-ins, 11-16) runs after it, in the buckets.
    /// One no measure says runs, and is measured for the next.
    #[cfg(feature = "checkout")]
    #[test]
    fn a_gate_run_runs_before_the_landing_only_when_cheap() {
        let run = crate::jit_misses::check_runs(&[("a".into(), false), ("b".into(), false)]).remove(0);
        assert_eq!(pre_land(&run, Some(CHEAP_GATE_MS)), Ok(()), "a run within the bound runs");
        assert_eq!(pre_land(&run, None), Ok(()), "a run no measure says runs");
        let line = pre_land(&run, Some(1_830_000)).expect_err("a 30 min check runs after the landing");
        let why = no_verdict(2, &line);
        assert!(why.starts_with("estimated 30m") && why.ends_with("a hand-in's check may take before it lands"), "the gate's note says why, as a run that did not run: {why}");
    }

    /// #5087: the JIT took a test rustc rejects (E0277 under rustc's integer fallback); it landed
    /// green, and the fast bucket's testcheck was the first to say so (11-15). The hand-in checks
    /// the touched packages' test targets by rustc: a new error or warning refuses it.
    #[cfg(feature = "checkout")]
    #[test]
    fn a_hand_in_whose_one_rustc_error_is_in_a_test_target_is_refused() {
        let tree = crate::orch::ci::Tree::mem(vec![
            ("Cargo.toml", "[workspace]\nmembers = [\"a\", \"b\"]\n"),
            ("a/Cargo.toml", "[package]\nname = \"a\"\n"),
            ("a/src/lib.rs", ""),
            ("a/tests/t.rs", "#[test]\nfn bits() {\n    let bit = |n| (7u8 >> n) & 1 != 0;\n    assert!(bit(0u32));\n}\n"),
            ("b/Cargo.toml", "[package]\nname = \"b\"\n"),
            ("b/src/lib.rs", ""),
        ]);
        let paths = vec!["a/tests/t.rs".to_string()];
        let tested = crate::jit_misses::test_targets_touched(&tree, &paths);
        assert_eq!(tested, ["a"], "a package whose one change is its test is checked with its test targets");
        assert!(crate::jit_misses::packages_touched(&tree, &paths).is_empty(), "and only there: no build that ships holds it");
        let error = "a/tests/t.rs:3:24: error[E0277]: no implementation for `u8 & i32`";
        let none = |_: &crate::jit_misses::CheckRun| Red::new();
        let refused = tests_gate(&tested, &|run| {
            assert!(run.tests && run.own && run.cmd.iter().any(|w| w == "--tests") && run.alone == "cargo check --release --tests -p a", "rustc checks the test targets: {run:?}");
            (101, format!("{error}\n"))
        }, &none).expect_err("rustc's error in a test target refuses the hand-in");
        assert!(refused[0].starts_with("test targets: rustc's check of a fails: 1 error line(s)") && refused[0].ends_with("`cargo check --release --tests -p a` shows it alone"), "the refusal names the package and the command: {refused:?}");
        assert!(refused.iter().any(|l| l.trim() == error), "and says rustc's error where it is: {refused:?}");
        let clean = tests_gate(&tested, &|_| (0, "warning: unused manifest key\n".into()), &|_| panic!("a clean check never checks the tips"))
            .expect("an unlocated manifest warning passes");
        assert!(clean[0].starts_with("test targets: rustc checked a in "), "{clean:?}");
        let warning = "a/tests/t.rs:1:5: warning: unused import: `std::fmt`";
        let warned = tests_gate(&tested, &|_| (0, format!("{warning}\n")), &none).expect_err("a located warning refuses the hand-in");
        assert!(warned[0].ends_with("`cargo check --release --tests -p a` shows it alone"), "{warned:?}");
        assert_eq!(warned[1].trim(), warning);
        let lost = tests_gate(&tested, &|_| (2, not_run_line("check of a", "no box took it")), &none).expect("a check that never ran does not refuse");
        assert!(lost[0].contains("a not checked (no box took it); the fast bucket checks them after the landing"), "{lost:?}");
        let gone = tests_gate(&tested, &|_| (LOST_EXIT, not_run_line("check of a", "its lease L1 was lost on node-a: no lease L1; no verdict, run it again")), &none).expect("a check whose lease was lost does not refuse");
        assert!(gone[0].contains("a not checked (its lease L1 was lost on node-a: no lease L1; no verdict, run it again)"), "the lost run's own line says why: {gone:?}");
        assert!(tests_gate(&[], &|_| panic!("no touched package, no run"), &none).unwrap().is_empty(), "a docs-only hand-in runs nothing");
        // rustc's same error already at the tips without the hand-in: the buckets', not the hand-in's
        let excused = tests_gate(&tested, &|_| (101, format!("{error}\n")), &|_| crate::jit_misses::check_failures(&format!("{error}\n")).iter().filter_map(|l| crate::bucket::error_key(l)).collect()).expect("an error already red at the tips refuses nothing");
        assert_eq!(excused, [red_note("test targets", "a", 1)], "{excused:?}");
    }

    #[cfg(feature = "checkout")]
    #[test]
    fn tests_gate_refuses_lanes_new_warning_with_repro_and_native_lint_note() {
        let warning = "libs/relax/compiler/src/domain_tests.rs:2403:22: warning: `malloc` redeclared with a different signature";
        let none = |_: &CheckRun| Red::new();
        let refused = tests_gate(&["relax-compiler".into()], &|_| {
            (0, format!("{warning}\n{warning}\nwarning: `relax-compiler` (lib test) generated 1 warning\n"))
        }, &none).expect_err("a new warning refuses even when cargo exits successfully");
        assert_eq!(refused.len(), 2, "the duplicate and the cargo summary add no lines");
        assert!(refused[0].starts_with("test targets: rustc's check of relax-compiler fails: 1 new warning line(s) in "), "{refused:?}");
        assert!(refused[0].contains("(native rustc check; catches lints the JIT check doesn't run)"), "{refused:?}");
        assert!(refused[0].ends_with("; `cargo check --release --tests -p relax-compiler` shows it alone"), "{refused:?}");
        assert_eq!(refused[1], format!("  {warning}"));

        let refused = tests_gate(&["a".into(), "b".into()], &|_| (0, "b/tests/t.rs:1:5: warning: unused import\n".into()), &none)
            .expect_err("a warning in the second package refuses the hand-in");
        assert!(refused[0].starts_with("test targets: rustc's check of a, b fails: "), "{refused:?}");
        assert!(refused[0].ends_with("`cargo check --release --tests -p a -p b` shows it alone"), "{refused:?}");
    }

    #[cfg(feature = "checkout")]
    #[test]
    fn tests_gate_excuses_tip_warning_but_refuses_new_warning_in_same_package() {
        let tested = ["a".into()];
        let tip = "/other-box/tree/a/tests/t.rs:1:5: warning: unused import: `std::fmt`";
        let shifted = "a/tests/t.rs:11:5: warning: unused import: `std::fmt`";
        let warning = "a/tests/t.rs:24:22: warning: `malloc` redeclared with a different signature";
        let tip_red = Red::from([crate::bucket::error_key(tip).unwrap()]);
        let calls = AtomicUsize::new(0);
        let red = |_: &CheckRun| {
            calls.fetch_add(1, Ordering::Relaxed);
            tip_red.clone()
        };
        let excused = tests_gate(&tested, &|_| (0, format!("{shifted}\n{shifted}\n")), &red)
            .expect("the same tip warning on a shifted line is excused");
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(excused, [red_note("test targets", "a", 1)]);

        let refused = tests_gate(&tested, &|_| (0, format!("{shifted}\n{warning}\n")), &red)
            .expect_err("a different warning in the same file still refuses");
        assert_eq!(calls.load(Ordering::Relaxed), 2, "one tip check per gate invocation");
        assert_eq!(refused.len(), 2);
        assert!(refused[0].contains("1 new warning line(s)"), "{refused:?}");
        assert!(refused[0].ends_with("`cargo check --release --tests -p a` shows it alone"), "{refused:?}");
        assert_eq!(refused[1].trim(), warning);
        assert!(!refused.iter().any(|line| line.contains("unused import")), "{refused:?}");
    }

    #[cfg(feature = "checkout")]
    #[test]
    fn tests_gate_keeps_error_behavior_mixed_diagnostics_and_print_cap() {
        let tested = ["a".into()];
        let none = |_: &CheckRun| Red::new();
        let error = "a/tests/t.rs:3:24: error[E0277]: no implementation for `u8 & i32`";
        let warning = "a/tests/t.rs:1:5: warning: unused import: `std::fmt`";
        let refused = tests_gate(&tested, &|_| (101, format!("{error}\n")), &none).expect_err("a new error refuses");
        assert!(refused[0].contains("1 error line(s) in "), "{refused:?}");
        assert!(refused[0].ends_with("`cargo check --release --tests -p a` shows it alone"), "{refused:?}");
        assert!(!refused[0].contains("native rustc check"), "{refused:?}");

        let refused = tests_gate(&tested, &|_| (101, format!("{warning}\n{error}\n")), &none).expect_err("mixed diagnostics refuse");
        assert!(refused[0].contains("1 error line(s) and 1 new warning line(s) in "), "{refused:?}");
        assert!(!refused[0].contains("native rustc check"), "{refused:?}");
        assert_eq!(refused[1].trim(), error, "errors are printed first");
        assert_eq!(refused[2].trim(), warning);

        let mut lines: Vec<String> = (1..=45).map(|i| format!("a/tests/t.rs:{i}:1: warning: old {i}")).collect();
        let tip_red: Red = lines.iter().map(|line| crate::bucket::error_key(line).unwrap()).collect();
        lines.extend((0..42).map(|i| format!("a/tests/t.rs:{}:1: warning: new {i}", 100 + i)));
        let refused = tests_gate(&tested, &|_| (0, lines.join("\n")), &|_| tip_red.clone()).expect_err("new warnings past the print cap refuse");
        assert!(refused[0].contains("42 new warning line(s)"), "{refused:?}");
        assert_eq!(refused.len(), 1 + TESTS_GATE_MAX_SHOWN + 1);
        assert!(!refused.iter().any(|line| line.contains("warning: old ")), "{refused:?}");
        assert!(refused[1].ends_with("warning: new 0"), "{refused:?}");
        assert!(refused[40].ends_with("warning: new 39"), "{refused:?}");
        assert_eq!(refused[41], "  ... and 2 more line(s); `cargo check --release --tests -p a` shows all");
    }

    #[cfg(feature = "checkout")]
    #[test]
    fn tests_gate_passes_through_when_skipped_past_cheap_bound_or_deferred_at_start_bound() {
        let tested = ["a".into()];
        let calls = AtomicUsize::new(0);
        let red = |_: &CheckRun| {
            calls.fetch_add(1, Ordering::Relaxed);
            Red::new()
        };
        let skipped = tests_gate(&tested, &|run| (2, format!("{}\n", pre_land(run, Some(CHEAP_GATE_MS + 60_000)).unwrap_err())), &red)
            .expect("a slow check is left to the fast bucket");
        assert!(skipped[0].starts_with("test targets: a not checked (estimated 3m00s, past the 2m00s "), "{skipped:?}");
        assert!(skipped[0].ends_with("); the fast bucket checks them after the landing"), "{skipped:?}");
        assert_eq!(calls.load(Ordering::Relaxed), 0);

        assert_eq!(gate_start_bound(Some(5_000)), Duration::from_millis(GATE_START_MIN_MS));
        assert_eq!(gate_start_bound(Some(90_000)), Duration::from_secs(90));
        assert_eq!(gate_start_bound(None), Duration::from_millis(CHEAP_GATE_MS));
        let deferred = tests_gate(&tested, &|_| {
            (DEFERRED_EXIT, not_run_line("check of a", &deferred_said("L9", Duration::from_secs(46), gate_start_bound(Some(45_000)))))
        }, &red).expect("a deferred check is left to the fast bucket");
        assert!(deferred[0].starts_with("test targets: a not checked (L9 had not started 46 s after it asked"), "{deferred:?}");
        assert!(deferred[0].ends_with("); the fast bucket checks them after the landing"), "{deferred:?}");
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    /// #5188 (11-15): the lane's makepad half had merged work past a two-repo landing (#5185) and
    /// its commercial half had not merged main; at that pinned pair the master refused the runs
    /// and rustc's check of the test targets was skipped. Every gate checks the merge onto the
    /// tips, read as one landed pair. #5197 (11-15): a landing removed an item, the next added a
    /// use of it in a test target, checked where the item still was, and main's test target went
    /// red: at the merge rustc refuses it.
    #[cfg(feature = "checkout")]
    #[test]
    fn a_hand_in_whose_halves_hold_different_landings_is_checked_at_its_merge_onto_the_tips() {
        let root = std::env::temp_dir().join(format!("devhub-split-pair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let com = root.join("apps/commercial");
        for dir in ["crates/a/src", "apps/commercial/c/src", "apps/commercial/c/tests"] { std::fs::create_dir_all(root.join(dir)).unwrap(); }
        let run = |dir: &Path, args: &[&str]| git(dir, &[&["-c", "core.hooksPath=/dev/null"][..], args].concat()).expect("fixture git succeeds");
        let write = |path: &str, text: &str| std::fs::write(root.join(path), text).unwrap();
        for (dir, branch) in [(&root, "work"), (&com, "main")] {
            run(dir, &["init", "-q", "-b", branch]);
            for (k, v) in [("user.name", "t"), ("user.email", "t@t"), ("commit.gpgsign", "false")] { run(dir, &["config", k, v]); }
        }
        write(".gitignore", "/apps/commercial/\n");
        write("Cargo.toml", "[workspace]\nmembers = [\"crates/a\", \"apps/commercial/c\"]\n");
        write("crates/a/Cargo.toml", "[package]\nname = \"a\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        write("crates/a/src/lib.rs", "pub fn f() {}\n");
        write("apps/commercial/c/Cargo.toml", "[package]\nname = \"c\"\nversion = \"0.1.0\"\nedition = \"2021\"\n");
        write("apps/commercial/c/src/lib.rs", "pub fn g() {}\npub fn ledctl() {}\n");
        write("apps/commercial/c/tests/t.rs", "#[test]\nfn t() {}\n");
        for dir in [&root, &com] {
            run(dir, &["add", "."]);
            run(dir, &["commit", "-qm", "base"]);
            run(dir, &["branch", "rc-s"]);
        }
        // a two-repo landing that removes `ledctl`; the lane's makepad half merges work, its
        // commercial half not main
        write("crates/a/src/lib.rs", "pub fn f() {}\npub fn h() {}\n");
        run(&root, &["commit", "-qam", "landing, makepad half"]);
        write("apps/commercial/c/src/lib.rs", "pub fn g() {}\n");
        run(&com, &["commit", "-qam", "landing, commercial half: ledctl goes"]);
        let (work, main) = (run(&root, &["rev-parse", "work"]), run(&com, &["rev-parse", "main"]));
        run(&root, &["branch", "-f", "rc-s", "work"]);
        run(&com, &["checkout", "-q", "rc-s"]);
        write("apps/commercial/c/tests/t.rs", "#[test]\nfn t() {\n    c::ledctl();\n}\n");
        run(&com, &["commit", "-qam", "the lane's test uses ledctl"]);
        let cs = run(&com, &["rev-parse", "rc-s"]);
        assert!(git(&com, &["merge-base", "--is-ancestor", &main, &cs]).is_none(), "the fixture's commercial half lacks the landing its makepad half holds");
        let rev = format!("rc-s@{work},{}=rc-s@{cs}", crate::state::PAIRED);
        let seen = std::sync::Mutex::new(Vec::new());
        let error = "apps/commercial/c/tests/t.rs:3:8: error[E0425]: cannot find function `ledctl` in crate `c`";
        let rustc = |at: &str| {
            let main = super::super::Pin::of_rev(at).expect("the checker receives a pinned rev").pair().main;
            let lib = run(&com, &["show", &format!("{main}:c/src/lib.rs")]);
            let uses = run(&com, &["show", &format!("{main}:c/tests/t.rs")]).contains("c::ledctl()");
            if uses && !lib.contains("pub fn ledctl") { (101, format!("{error}\n")) } else { (0, "devhub test: check of c passed in 0.1s on n1\n".to_string()) }
        };
        assert_eq!(rustc(&rev).0, 0, "at the lane's own pair the use compiles: checked there, it passed");
        let (features, _) = gates(&root, "rc-s", &rev, &|run, at| {
            seen.lock().unwrap().push((run.tests, run.packages.clone(), at.to_string()));
            if run.tests { rustc(at) } else { (0, String::new()) }
        });
        let refused = features.expect_err("the test target's use of the removed item fails at the merge");
        assert!(refused[0].starts_with("test targets: rustc's check of c fails: 1 error line(s)") && refused.iter().any(|l| l.trim() == error), "rustc's check of the test target ran at the merge, not skipped: {refused:?}");
        let seen = seen.into_inner().unwrap();
        assert!(seen.iter().any(|(tests, packages, _)| *tests && packages == &["c"]), "rustc's check of the touched test target ran: {seen:?}");
        // every gate's runs pin the one merge; the failing one is asked at the tips without the
        // hand-in too, where the use of the removed item is not red: it is the hand-in's own
        let tips: Vec<&(bool, Vec<String>, String)> = seen.iter().filter(|(_, _, at)| *at != seen[0].2).collect();
        assert!(tips.len() == 1 && tips[0].0 && tips[0].1 == ["c"], "only the failing run is asked again, at the tips: {seen:?}");
        let at_tips = super::super::Pin::of_rev(&tips[0].2).expect("the tips are pinned as the merge is").pair();
        assert_eq!((at_tips.work, at_tips.main), (work.clone(), main.clone()), "the tips without the hand-in: {seen:?}");
        let pair = super::super::Pin::of_rev(&seen[0].2).expect("the checker receives a pinned rev").pair();
        assert_eq!(pair.work, work, "the makepad side is work's tip, which the lane's half already was");
        assert_eq!((run(&com, &["rev-parse", &format!("{}^1", pair.main)]), run(&com, &["rev-parse", &format!("{}^2", pair.main)])), (main.clone(), cs.clone()),
            "the commercial side is the lane's half merged onto main's tip: both halves of the landing, never the lane's own pair");
        // a landing moving the tips: the gate reads them after it, as one pair
        let next = run(&com, &["commit-tree", &format!("{main}^{{tree}}"), "-p", &main, "-m", "next landing"]);
        let lock = root.join(".git/land-ff.lock");
        std::fs::create_dir(&lock).unwrap();
        let mover = std::thread::spawn({
            let (com, lock, next) = (com.clone(), lock.clone(), next.clone());
            move || {
                std::thread::sleep(Duration::from_millis(150));
                git(&com, &["update-ref", "refs/heads/main", &next]).expect("main moves");
                std::fs::remove_dir(&lock).unwrap();
            }
        });
        assert_eq!(landed_tips(&root), Ok((work.clone(), next)), "the tips are read once the landing that holds land-ff.lock has moved both");
        mover.join().unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }
}
