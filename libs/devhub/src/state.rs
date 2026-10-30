//! devhub's workflow state: what `devhub land`, `lanes`, `verify`, `after`, `allapps` and `push`
//! know, in ONE document the master owns (`~/.makepad/devhub/state.json`, rewritten whole on every
//! change; no .txt ledgers). Every change is also a record on the master's event stream (its kind
//! is the section's name), so an agent blocks on `devhub watch <kinds>` instead of polling.
//!
//! The sections are typed. A command that keeps state adds its struct here, a field on [`Doc`] and
//! its lines in [`Doc::to_value`] / [`Doc::from_value`]; it changes state only through
//! [`Store::update`] and says so with [`Store::event`].

use crate::proto::{hub_record, Record, Sev};
use makepad_strict_json::{obj, s, Value};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub type Publish = Arc<dyn Fn(Record) + Send + Sync>;

/// How many finished entries a list keeps (verified tips, failures, waiters, pushes).
pub const KEEP: usize = 200;
/// How many finished jobs it keeps.
pub const JOBS_KEEP: usize = 500;

/// A makepad `work` commit and the commercial `main` commit landed with it: what verify, allapps
/// and push name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Default)]
pub struct Pair {
    pub work: String,
    pub main: String,
}

impl Pair {
    pub fn new(work: &str, main: &str) -> Pair {
        Pair { work: work.to_string(), main: main.to_string() }
    }

    /// `<work 10>,<main 10>`.
    pub fn short(&self) -> String {
        format!("{},{}", short(&self.work), short(&self.main))
    }

    /// The lease `--rev` of the pair: `work@<sha>,main@<sha>`.
    pub fn rev(&self) -> String {
        format!("work@{},main@{}", self.work, self.main)
    }

    pub fn valid(&self) -> bool {
        hex40(&self.work) && hex40(&self.main)
    }

    fn to_value(&self) -> Value {
        obj(vec![("work", s(&self.work)), ("main", s(&self.main))])
    }

    fn from_value(v: Option<&Value>) -> Pair {
        Pair { work: text(v, "work"), main: text(v, "main") }
    }
}

pub fn short(sha: &str) -> &str {
    &sha[..sha.len().min(10)]
}

pub fn hex40(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

// ---- verify ----

/// The rolling verifier (`devhub verify`).
#[derive(Clone, Debug, PartialEq)]
pub struct Verify {
    /// the master verifies new tips (`devhub verify on|off`)
    pub on: bool,
    /// verified pairs, oldest first
    pub verified: Vec<Tip>,
    /// pairs that failed, oldest first (a failed pair is not tried again; newer tips are)
    pub failures: Vec<Failure>,
    /// the run in progress
    pub run: Option<Run>,
    /// when a run last diffed every program (the nightly full diff)
    pub last_full_ms: u64,
    /// runs asked for on the CLI, first first
    pub asked: Vec<Asked>,
}

impl Default for Verify {
    fn default() -> Verify {
        Verify { on: true, verified: Vec::new(), failures: Vec::new(), run: None, last_full_ms: 0, asked: Vec::new() }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Tip {
    pub pair: Pair,
    pub at_ms: u64,
    /// `affected` or `all`: what the diff leg covered
    pub diff: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Failure {
    pub pair: Pair,
    pub at_ms: u64,
    /// the first bad landed commit, when the bisect found one
    pub blame: Option<Blame>,
    /// why there is no blame (empty with one)
    pub unresolved: String,
    /// the failing run's legs
    pub legs: Vec<Leg>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Blame {
    /// `work` or `commercial`
    pub repo: String,
    pub sha: String,
    pub subject: String,
    pub good: Pair,
    pub bad: Pair,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Run {
    pub id: String,
    pub pair: Pair,
    pub started_ms: u64,
    pub diff: String,
    /// a CLI pair run: no ledger is written
    pub dry: bool,
    /// `tip`, `baseline` or `bisect k/n`
    pub purpose: String,
    pub legs: Vec<Leg>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Leg {
    pub name: String,
    /// `wait` (queued), `run`, `ok`, `fail`, `incomplete`
    pub state: String,
    pub lease: String,
    pub node: String,
    /// the leg's verdict line (or why there is none)
    pub summary: String,
    /// the first error lines of its log
    pub errors: Vec<String>,
    /// its log on the master
    pub log: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Asked {
    /// None: the newest landed pair now
    pub pair: Option<Pair>,
    pub dry: bool,
    /// diff every program
    pub full: bool,
}

// ---- allapps ----

/// `makepad check` of every app, example and WM package (`devhub allapps`).
#[derive(Clone, Debug, PartialEq)]
pub struct Allapps {
    pub on: bool,
    pub last: Option<AllappsRun>,
    pub running: Option<AllappsRun>,
    /// `devhub allapps kick`: run now, idle or not
    pub asked: bool,
}

impl Default for Allapps {
    fn default() -> Allapps {
        Allapps { on: true, last: None, running: None, asked: false }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct AllappsRun {
    pub pair: Pair,
    pub at_ms: u64,
    pub secs: u64,
    /// `running`, `ok`, `errors`, `build-failed`, `incomplete`
    pub verdict: String,
    pub errors: Vec<PkgErrors>,
    pub lease: String,
    pub node: String,
    pub log: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct PkgErrors {
    pub pkg: String,
    /// 0: the check printed no summary line
    pub errors: u32,
    pub first: Vec<String>,
}

// ---- after ----

/// `devhub after <cond> -- <cmd>`: a command the master runs once a condition holds.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Waiter {
    pub id: String,
    pub cond: Cond,
    /// devhub words (`launch mac makepad-fab`, `verify kick`, ...)
    pub cmd: Vec<String>,
    /// the agent or lane that asked
    pub by: String,
    pub added_ms: u64,
    /// `waiting`, `running`, `done`, `failed`, `cancelled`
    pub state: String,
    pub fired_ms: u64,
    pub exit: Option<i32>,
    /// what met the condition, or why it failed
    pub note: String,
    pub log: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum Cond {
    /// the branch (of `repo`: makepad or commercial) is in work / main; `sha`: its last seen tip
    Landed { branch: String, repo: String, sha: String },
    /// the node is up, on for dev and holds and waits for no lease
    Free { node: String },
    /// a verify passed after the waiter was added (`contains`: one whose pair has this commit)
    Verified { contains: Option<String> },
    #[default]
    None,
}

impl Cond {
    pub fn describe(&self) -> String {
        match self {
            Cond::Landed { branch, sha, .. } if sha.is_empty() => format!("landed {branch}"),
            Cond::Landed { branch, sha, .. } => format!("landed {branch}@{}", short(sha)),
            Cond::Free { node } => format!("free {node}"),
            Cond::Verified { contains: Some(c) } => format!("verified {}", short(c)),
            Cond::Verified { contains: None } => "verified".into(),
            Cond::None => "-".into(),
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Cond::Landed { branch, repo, sha } => obj(vec![("on", s("landed")), ("branch", s(branch)), ("repo", s(repo)), ("sha", s(sha))]),
            Cond::Free { node } => obj(vec![("on", s("free")), ("node", s(node))]),
            Cond::Verified { contains } => obj(vec![("on", s("verified")), ("contains", contains.as_deref().map_or(Value::Null, s))]),
            Cond::None => Value::Null,
        }
    }

    pub fn from_value(v: Option<&Value>) -> Cond {
        match text(v, "on").as_str() {
            "landed" => Cond::Landed { branch: text(v, "branch"), repo: text(v, "repo"), sha: text(v, "sha") },
            "free" => Cond::Free { node: text(v, "node") },
            "verified" => Cond::Verified { contains: v.and_then(|v| v.get("contains")).and_then(|c| c.as_str()).map(str::to_string) },
            _ => Cond::None,
        }
    }
}

// ---- push ----

/// One collapsed push of a verified tip to origin/work (`devhub push`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Pushed {
    pub pair: Pair,
    /// the pushed commit (parent: `origin_was`)
    pub commit: String,
    pub origin_was: String,
    pub at_ms: u64,
    pub files: u32,
}

// ---- ci ----

/// The CI shards of verify (`devhub ci`, [`crate::orch::ci`]): what each shard took last (for
/// balancing the boxes), the run in progress and the finished runs.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Ci {
    /// seconds per shard id (a moving average of its finished runs), for balancing
    pub timings: Vec<(String, f64)>,
    pub run: Option<CiRun>,
    /// finished runs, oldest first
    pub runs: Vec<CiRun>,
    /// runs started: rotates which OS each app's smoke script runs on
    pub rotation: u64,
}

impl Ci {
    pub fn timing(&self, id: &str) -> Option<f64> {
        self.timings.iter().find(|(k, _)| k == id).map(|(_, s)| *s)
    }

    /// Folds a measured time into the shard's average (new runs weigh half).
    pub fn measured(&mut self, id: &str, secs: f64) {
        match self.timings.iter_mut().find(|(k, _)| k == id) {
            Some((_, s)) => *s = (*s + secs) / 2.0,
            None => self.timings.push((id.to_string(), secs)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CiRun {
    pub id: String,
    pub pair: Pair,
    /// a CLI run: no verify ledger is written
    pub dry: bool,
    pub started_ms: u64,
    pub ended_ms: u64,
    /// `running`, `ok`, `fail`, `incomplete`
    pub verdict: String,
    pub shards: Vec<CiShard>,
    /// the box that finished last and its shards in order: the run's critical path
    pub critical: String,
    /// per box: `<node> <busy s> s, <n> shards`
    pub boxes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct CiShard {
    /// `check:<target>`, `jit:<group>`, `test:<pkg>:<target>`, `slice:<app>`, `smoke:<script dir>`
    pub id: String,
    /// `wait`, `run`, `ok`, `fail`, `incomplete`
    pub state: String,
    pub node: String,
    pub lease: String,
    pub secs: f64,
    /// what decided it (its failing step and why, or the steps' count)
    pub summary: String,
    /// its first error lines
    pub errors: Vec<String>,
    /// its log on the master
    pub log: String,
}

impl CiShard {
    /// The shard as a verify leg (the legs a failure keeps).
    pub fn leg(&self) -> Leg {
        Leg { name: self.id.clone(), state: self.state.clone(), lease: self.lease.clone(), node: self.node.clone(), summary: self.summary.clone(), errors: self.errors.clone(), log: self.log.clone() }
    }
}

fn ci_shard_value(c: &CiShard) -> Value {
    obj(vec![
        ("id", s(&c.id)),
        ("state", s(&c.state)),
        ("node", s(&c.node)),
        ("lease", s(&c.lease)),
        ("secs", Value::F64(c.secs)),
        ("summary", s(&c.summary)),
        ("errors", strs(&c.errors)),
        ("log", s(&c.log)),
    ])
}

fn ci_shard_from(v: &Value) -> CiShard {
    let o = Some(v);
    CiShard {
        id: text(o, "id"),
        state: text(o, "state"),
        node: text(o, "node"),
        lease: text(o, "lease"),
        secs: v.get("secs").and_then(f64_of).unwrap_or(0.0),
        summary: text(o, "summary"),
        errors: strs_from(o, "errors"),
        log: text(o, "log"),
    }
}

fn ci_run_value(r: &CiRun) -> Value {
    obj(vec![
        ("id", s(&r.id)),
        ("pair", r.pair.to_value()),
        ("dry", Value::Bool(r.dry)),
        ("started_ms", int(r.started_ms)),
        ("ended_ms", int(r.ended_ms)),
        ("verdict", s(&r.verdict)),
        ("shards", Value::Arr(r.shards.iter().map(ci_shard_value).collect())),
        ("critical", s(&r.critical)),
        ("boxes", strs(&r.boxes)),
    ])
}

fn ci_run_from(v: &Value) -> CiRun {
    let o = Some(v);
    CiRun {
        id: text(o, "id"),
        pair: Pair::from_value(v.get("pair")),
        dry: flag(o, "dry", false),
        started_ms: uint(o, "started_ms"),
        ended_ms: uint(o, "ended_ms"),
        verdict: text(o, "verdict"),
        shards: arr(o, "shards").iter().map(ci_shard_from).collect(),
        critical: text(o, "critical"),
        boxes: strs_from(o, "boxes"),
    }
}

fn ci_value(c: &Ci) -> Value {
    obj(vec![
        ("timings", Value::Obj(c.timings.iter().map(|(k, v)| (k.clone(), Value::F64(*v))).collect())),
        ("run", c.run.as_ref().map_or(Value::Null, ci_run_value)),
        ("runs", Value::Arr(c.runs.iter().map(ci_run_value).collect())),
        ("rotation", int(c.rotation)),
    ])
}

fn ci_from(v: Option<&Value>) -> Ci {
    let timings = match v.and_then(|v| v.get("timings")) {
        Some(Value::Obj(f)) => f.iter().filter_map(|(k, x)| f64_of(x).map(|s| (k.clone(), s))).collect(),
        _ => Vec::new(),
    };
    Ci {
        timings,
        run: v.and_then(|x| x.get("run")).filter(|r| !r.is_null()).map(ci_run_from),
        runs: arr(v, "runs").iter().map(ci_run_from).collect(),
        rotation: uint(v, "rotation"),
    }
}

fn f64_of(v: &Value) -> Option<f64> {
    match v {
        Value::F64(f) => Some(*f),
        Value::Int(i) => Some(*i as f64),
        _ => None,
    }
}

// ---- lanes ----

/// A lane worktree the master controls (`devhub lane new`, or adopted).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct LaneRec {
    pub name: String,
    pub path: String,
    /// `rc-<name>`
    pub branch: String,
    /// the nested commercial worktree's branch (empty: none, or detached)
    pub cbranch: String,
    pub created_ms: u64,
    /// registered from an existing worktree
    pub adopted: bool,
    /// `live`, `removed` (landed and cleaned up), `dropped`
    pub state: String,
    /// when the cleanup first saw both halves landed (it waits out a grace period of no activity)
    pub landed_ms: u64,
    /// `devhub lane keep`: never cleaned up automatically
    pub keep: bool,
    pub ended_ms: u64,
    /// what the cleanup did, a drop's bundles, or why a landed lane is still kept
    pub note: String,
}

// ---- job ----

/// One self-contained command run (`devhub job submit`): the master places it on a free box the
/// moment one fits and releases the box when the command ends; no box waits for an agent.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Job {
    /// `J<n>`
    pub id: String,
    /// the batch it came in (`B<n>`), or empty
    pub batch: String,
    pub lane: String,
    pub agent: String,
    /// the lease kind (`test`, `build`, `gpu`, ...)
    pub kind: String,
    /// `arch=x64,os=linux`, or empty
    pub needs: String,
    /// a preferred node, or empty
    pub node_pref: String,
    pub prio: i32,
    /// the pinned `--rev` (`branch@sha[,branch@sha]`): the snapshot's base commits
    pub rev: String,
    pub repo_dir: String,
    /// the snapshot of the worktree's uncommitted files (`S<n>`, its files and manifest in
    /// `~/.makepad/devhub/jobs/snap/S<n>`), shipped on top of `rev`; empty: the commits alone
    pub snap: String,
    /// files shipped and deleted on top of `rev`
    pub snap_files: u32,
    /// placements so far (a box lost under it places it again)
    pub attempts: u32,
    pub cmd: Vec<String>,
    /// jobs that must end ok first
    pub after: Vec<String>,
    /// `queued` (deps), `leasing` (waiting for a box), `running`, `paused` (a parked panic; the box
    /// stays held), then `ok`, `failed`, `panicked`, `cancelled`, `skipped` (a dep did not end ok),
    /// `lost` (the master restarted under it)
    pub state: String,
    pub submitted_ms: u64,
    /// the box granted, the snapshot shipped, the first output byte seen (latency of a job)
    pub granted_ms: u64,
    pub shipped_ms: u64,
    pub first_out_ms: u64,
    pub started_ms: u64,
    pub ended_ms: u64,
    pub lease: String,
    pub node: String,
    pub exit: Option<i32>,
    pub log: String,
    /// the first error lines of its output
    pub errors: Vec<String>,
    /// its output's last lines
    pub tail: Vec<String>,
    pub panic: Option<JobPanic>,
    /// when an agent last acted on it while paused (inspect, resume): the idle timeout counts from it
    pub touched_ms: u64,
    pub note: String,
}

impl Job {
    pub fn done(&self) -> bool {
        matches!(self.state.as_str(), "ok" | "failed" | "panicked" | "cancelled" | "skipped" | "lost")
    }
}

/// The panic a job's program parked on.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct JobPanic {
    /// the JIT's panic id (`P1`), for `resume`
    pub id: String,
    pub pid: u32,
    pub message: String,
    pub thread: String,
    pub location: String,
    pub at_ms: u64,
}

// ---- the document ----

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Doc {
    pub verify: Verify,
    pub allapps: Allapps,
    pub after: Vec<Waiter>,
    /// the id number of the next waiter (`A<n>`)
    pub after_next: u64,
    pub pushes: Vec<Pushed>,
    pub ci: Ci,
    pub lanes: Vec<LaneRec>,
    pub jobs: Vec<Job>,
    /// the id numbers of the next job (`J<n>`) and batch (`B<n>`)
    pub jobs_next: u64,
    pub batches_next: u64,
    /// the id number of the next job snapshot (`S<n>`)
    pub snaps_next: u64,
}

impl Doc {
    pub fn to_value(&self) -> Value {
        let v = &self.verify;
        let a = &self.allapps;
        obj(vec![
            (
                "verify",
                obj(vec![
                    ("on", Value::Bool(v.on)),
                    ("verified", Value::Arr(v.verified.iter().map(tip_value).collect())),
                    ("failures", Value::Arr(v.failures.iter().map(failure_value).collect())),
                    ("run", v.run.as_ref().map_or(Value::Null, run_value)),
                    ("last_full_ms", int(v.last_full_ms)),
                    ("asked", Value::Arr(v.asked.iter().map(|q| obj(vec![("pair", q.pair.as_ref().map_or(Value::Null, Pair::to_value)), ("dry", Value::Bool(q.dry)), ("full", Value::Bool(q.full))])).collect())),
                ]),
            ),
            (
                "allapps",
                obj(vec![
                    ("on", Value::Bool(a.on)),
                    ("last", a.last.as_ref().map_or(Value::Null, allapps_value)),
                    ("running", a.running.as_ref().map_or(Value::Null, allapps_value)),
                    ("asked", Value::Bool(a.asked)),
                ]),
            ),
            ("after", Value::Arr(self.after.iter().map(waiter_value).collect())),
            ("after_next", int(self.after_next)),
            ("pushes", Value::Arr(self.pushes.iter().map(pushed_value).collect())),
            ("ci", ci_value(&self.ci)),
            ("lanes", Value::Arr(self.lanes.iter().map(lane_value).collect())),
            ("jobs", Value::Arr(self.jobs.iter().map(job_value).collect())),
            ("jobs_next", int(self.jobs_next)),
            ("batches_next", int(self.batches_next)),
            ("snaps_next", int(self.snaps_next)),
        ])
    }

    pub fn from_value(v: &Value) -> Doc {
        let ve = v.get("verify");
        let al = v.get("allapps");
        Doc {
            verify: Verify {
                on: flag(ve, "on", true),
                verified: arr(ve, "verified").iter().map(|t| Tip { pair: Pair::from_value(t.get("pair")), at_ms: uint(Some(t), "at_ms"), diff: text(Some(t), "diff") }).collect(),
                failures: arr(ve, "failures").iter().map(failure_from).collect(),
                run: ve.and_then(|x| x.get("run")).filter(|r| !r.is_null()).map(run_from),
                last_full_ms: uint(ve, "last_full_ms"),
                asked: arr(ve, "asked").iter().map(|q| Asked { pair: q.get("pair").filter(|p| !p.is_null()).map(|p| Pair::from_value(Some(p))), dry: flag(Some(q), "dry", false), full: flag(Some(q), "full", false) }).collect(),
            },
            allapps: Allapps {
                on: flag(al, "on", true),
                last: al.and_then(|x| x.get("last")).filter(|r| !r.is_null()).map(allapps_from),
                running: al.and_then(|x| x.get("running")).filter(|r| !r.is_null()).map(allapps_from),
                asked: flag(al, "asked", false),
            },
            after: arr(Some(v), "after").iter().map(waiter_from).collect(),
            after_next: uint(Some(v), "after_next"),
            pushes: arr(Some(v), "pushes").iter().map(|p| Pushed { pair: Pair::from_value(p.get("pair")), commit: text(Some(p), "commit"), origin_was: text(Some(p), "origin_was"), at_ms: uint(Some(p), "at_ms"), files: uint(Some(p), "files") as u32 }).collect(),
            ci: ci_from(v.get("ci")),
            lanes: arr(Some(v), "lanes").iter().map(|l| {
                let o = Some(l);
                LaneRec { name: text(o, "name"), path: text(o, "path"), branch: text(o, "branch"), cbranch: text(o, "cbranch"), created_ms: uint(o, "created_ms"), adopted: flag(o, "adopted", false), state: text(o, "state"), landed_ms: uint(o, "landed_ms"), keep: flag(o, "keep", false), ended_ms: uint(o, "ended_ms"), note: text(o, "note") }
            }).collect(),
            jobs: arr(Some(v), "jobs").iter().map(job_from).collect(),
            jobs_next: uint(Some(v), "jobs_next"),
            batches_next: uint(Some(v), "batches_next"),
            snaps_next: uint(Some(v), "snaps_next"),
        }
    }

    pub fn parse(text: &str) -> Option<Doc> {
        makepad_strict_json::parse_depth(text.as_bytes(), 16).ok().map(|v| Doc::from_value(&v))
    }

    /// One section's JSON (`verify`, `allapps`, `after`, `pushes`, `lanes`, `jobs`), or the whole document.
    pub fn section(&self, name: Option<&str>) -> Option<String> {
        let all = self.to_value();
        match name {
            None => Some(all.to_json()),
            Some(n) => all.get(n).map(|v| v.to_json()),
        }
    }

    /// Keeps the lists bounded (finished entries go first).
    fn trim(&mut self) {
        fn cut<T>(v: &mut Vec<T>) {
            if v.len() > KEEP {
                v.drain(..v.len() - KEEP);
            }
        }
        cut(&mut self.verify.verified);
        cut(&mut self.verify.failures);
        cut(&mut self.pushes);
        cut(&mut self.ci.runs);
        let ended = self.lanes.iter().filter(|l| l.state != "live").count();
        if ended > KEEP {
            let mut drop = ended - KEEP;
            self.lanes.retain(|l| {
                let old = drop > 0 && l.state != "live";
                if old {
                    drop -= 1;
                }
                !old
            });
        }
        let jobs_done = self.jobs.iter().filter(|j| j.done()).count();
        if jobs_done > JOBS_KEEP {
            let mut drop = jobs_done - JOBS_KEEP;
            self.jobs.retain(|j| {
                let old = drop > 0 && j.done();
                if old {
                    drop -= 1;
                }
                !old
            });
        }
        let done = self.after.iter().filter(|w| !matches!(w.state.as_str(), "waiting" | "running")).count();
        if done > KEEP {
            let mut drop = done - KEEP;
            self.after.retain(|w| {
                let old = drop > 0 && !matches!(w.state.as_str(), "waiting" | "running");
                if old {
                    drop -= 1;
                }
                !old
            });
        }
    }
}

fn int(n: u64) -> Value {
    Value::Int(n.min(i64::MAX as u64) as i64)
}

fn text(v: Option<&Value>, k: &str) -> String {
    v.and_then(|v| v.get(k)).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn uint(v: Option<&Value>, k: &str) -> u64 {
    v.and_then(|v| v.get(k)).and_then(|x| x.as_u64()).unwrap_or(0)
}

fn flag(v: Option<&Value>, k: &str, default: bool) -> bool {
    v.and_then(|v| v.get(k)).and_then(|x| x.as_bool()).unwrap_or(default)
}

fn arr<'a>(v: Option<&'a Value>, k: &str) -> &'a [Value] {
    v.and_then(|v| v.get(k)).and_then(|x| x.as_arr()).unwrap_or(&[])
}

fn strs(v: &[String]) -> Value {
    Value::Arr(v.iter().map(s).collect())
}

fn strs_from(v: Option<&Value>, k: &str) -> Vec<String> {
    arr(v, k).iter().filter_map(|x| x.as_str().map(str::to_string)).collect()
}

fn tip_value(t: &Tip) -> Value {
    obj(vec![("pair", t.pair.to_value()), ("at_ms", int(t.at_ms)), ("diff", s(&t.diff))])
}

fn leg_value(l: &Leg) -> Value {
    obj(vec![("name", s(&l.name)), ("state", s(&l.state)), ("lease", s(&l.lease)), ("node", s(&l.node)), ("summary", s(&l.summary)), ("errors", strs(&l.errors)), ("log", s(&l.log))])
}

fn leg_from(v: &Value) -> Leg {
    let v = Some(v);
    Leg { name: text(v, "name"), state: text(v, "state"), lease: text(v, "lease"), node: text(v, "node"), summary: text(v, "summary"), errors: strs_from(v, "errors"), log: text(v, "log") }
}

fn failure_value(f: &Failure) -> Value {
    let blame = f.blame.as_ref().map_or(Value::Null, |b| obj(vec![("repo", s(&b.repo)), ("sha", s(&b.sha)), ("subject", s(&b.subject)), ("good", b.good.to_value()), ("bad", b.bad.to_value())]));
    obj(vec![("pair", f.pair.to_value()), ("at_ms", int(f.at_ms)), ("blame", blame), ("unresolved", s(&f.unresolved)), ("legs", Value::Arr(f.legs.iter().map(leg_value).collect()))])
}

fn failure_from(v: &Value) -> Failure {
    let b = v.get("blame").filter(|b| !b.is_null());
    Failure {
        pair: Pair::from_value(v.get("pair")),
        at_ms: uint(Some(v), "at_ms"),
        blame: b.map(|b| Blame { repo: text(Some(b), "repo"), sha: text(Some(b), "sha"), subject: text(Some(b), "subject"), good: Pair::from_value(b.get("good")), bad: Pair::from_value(b.get("bad")) }),
        unresolved: text(Some(v), "unresolved"),
        legs: arr(Some(v), "legs").iter().map(leg_from).collect(),
    }
}

fn run_value(r: &Run) -> Value {
    obj(vec![
        ("id", s(&r.id)),
        ("pair", r.pair.to_value()),
        ("started_ms", int(r.started_ms)),
        ("diff", s(&r.diff)),
        ("dry", Value::Bool(r.dry)),
        ("purpose", s(&r.purpose)),
        ("legs", Value::Arr(r.legs.iter().map(leg_value).collect())),
    ])
}

fn run_from(v: &Value) -> Run {
    let o = Some(v);
    Run { id: text(o, "id"), pair: Pair::from_value(v.get("pair")), started_ms: uint(o, "started_ms"), diff: text(o, "diff"), dry: flag(o, "dry", false), purpose: text(o, "purpose"), legs: arr(o, "legs").iter().map(leg_from).collect() }
}

fn allapps_value(r: &AllappsRun) -> Value {
    obj(vec![
        ("pair", r.pair.to_value()),
        ("at_ms", int(r.at_ms)),
        ("secs", int(r.secs)),
        ("verdict", s(&r.verdict)),
        ("errors", Value::Arr(r.errors.iter().map(|e| obj(vec![("pkg", s(&e.pkg)), ("errors", int(e.errors as u64)), ("first", strs(&e.first))])).collect())),
        ("lease", s(&r.lease)),
        ("node", s(&r.node)),
        ("log", s(&r.log)),
    ])
}

fn allapps_from(v: &Value) -> AllappsRun {
    let o = Some(v);
    AllappsRun {
        pair: Pair::from_value(v.get("pair")),
        at_ms: uint(o, "at_ms"),
        secs: uint(o, "secs"),
        verdict: text(o, "verdict"),
        errors: arr(o, "errors").iter().map(|e| PkgErrors { pkg: text(Some(e), "pkg"), errors: uint(Some(e), "errors") as u32, first: strs_from(Some(e), "first") }).collect(),
        lease: text(o, "lease"),
        node: text(o, "node"),
        log: text(o, "log"),
    }
}

fn waiter_value(w: &Waiter) -> Value {
    obj(vec![
        ("id", s(&w.id)),
        ("cond", w.cond.to_value()),
        ("cmd", strs(&w.cmd)),
        ("by", s(&w.by)),
        ("added_ms", int(w.added_ms)),
        ("state", s(&w.state)),
        ("fired_ms", int(w.fired_ms)),
        ("exit", w.exit.map_or(Value::Null, |e| Value::Int(e as i64))),
        ("note", s(&w.note)),
        ("log", s(&w.log)),
    ])
}

fn waiter_from(v: &Value) -> Waiter {
    let o = Some(v);
    Waiter {
        id: text(o, "id"),
        cond: Cond::from_value(v.get("cond")),
        cmd: strs_from(o, "cmd"),
        by: text(o, "by"),
        added_ms: uint(o, "added_ms"),
        state: text(o, "state"),
        fired_ms: uint(o, "fired_ms"),
        exit: v.get("exit").and_then(|e| e.as_i64()).map(|e| e as i32),
        note: text(o, "note"),
        log: text(o, "log"),
    }
}

pub fn lane_value(l: &LaneRec) -> Value {
    obj(vec![
        ("name", s(&l.name)),
        ("path", s(&l.path)),
        ("branch", s(&l.branch)),
        ("cbranch", s(&l.cbranch)),
        ("created_ms", int(l.created_ms)),
        ("adopted", Value::Bool(l.adopted)),
        ("state", s(&l.state)),
        ("landed_ms", int(l.landed_ms)),
        ("keep", Value::Bool(l.keep)),
        ("ended_ms", int(l.ended_ms)),
        ("note", s(&l.note)),
    ])
}

pub fn job_value(j: &Job) -> Value {
    let panic = j.panic.as_ref().map_or(Value::Null, |p| obj(vec![("id", s(&p.id)), ("pid", int(p.pid as u64)), ("message", s(&p.message)), ("thread", s(&p.thread)), ("location", s(&p.location)), ("at_ms", int(p.at_ms))]));
    obj(vec![
        ("id", s(&j.id)),
        ("batch", s(&j.batch)),
        ("lane", s(&j.lane)),
        ("agent", s(&j.agent)),
        ("kind", s(&j.kind)),
        ("needs", s(&j.needs)),
        ("node_pref", s(&j.node_pref)),
        ("prio", Value::Int(j.prio as i64)),
        ("rev", s(&j.rev)),
        ("repo_dir", s(&j.repo_dir)),
        ("snap", s(&j.snap)),
        ("snap_files", int(j.snap_files as u64)),
        ("attempts", int(j.attempts as u64)),
        ("granted_ms", int(j.granted_ms)),
        ("shipped_ms", int(j.shipped_ms)),
        ("first_out_ms", int(j.first_out_ms)),
        ("cmd", strs(&j.cmd)),
        ("after", strs(&j.after)),
        ("state", s(&j.state)),
        ("submitted_ms", int(j.submitted_ms)),
        ("started_ms", int(j.started_ms)),
        ("ended_ms", int(j.ended_ms)),
        ("lease", s(&j.lease)),
        ("node", s(&j.node)),
        ("exit", j.exit.map_or(Value::Null, |e| Value::Int(e as i64))),
        ("log", s(&j.log)),
        ("errors", strs(&j.errors)),
        ("tail", strs(&j.tail)),
        ("panic", panic),
        ("touched_ms", int(j.touched_ms)),
        ("note", s(&j.note)),
    ])
}

pub fn job_from(v: &Value) -> Job {
    let o = Some(v);
    let p = v.get("panic").filter(|p| !p.is_null());
    Job {
        id: text(o, "id"),
        batch: text(o, "batch"),
        lane: text(o, "lane"),
        agent: text(o, "agent"),
        kind: text(o, "kind"),
        needs: text(o, "needs"),
        node_pref: text(o, "node_pref"),
        prio: v.get("prio").and_then(|x| x.as_i64()).unwrap_or(5) as i32,
        rev: text(o, "rev"),
        repo_dir: text(o, "repo_dir"),
        snap: text(o, "snap"),
        snap_files: uint(o, "snap_files") as u32,
        attempts: uint(o, "attempts") as u32,
        granted_ms: uint(o, "granted_ms"),
        shipped_ms: uint(o, "shipped_ms"),
        first_out_ms: uint(o, "first_out_ms"),
        cmd: strs_from(o, "cmd"),
        after: strs_from(o, "after"),
        state: text(o, "state"),
        submitted_ms: uint(o, "submitted_ms"),
        started_ms: uint(o, "started_ms"),
        ended_ms: uint(o, "ended_ms"),
        lease: text(o, "lease"),
        node: text(o, "node"),
        exit: v.get("exit").and_then(|e| e.as_i64()).map(|e| e as i32),
        log: text(o, "log"),
        errors: strs_from(o, "errors"),
        tail: strs_from(o, "tail"),
        panic: p.map(|p| JobPanic { id: text(Some(p), "id"), pid: uint(Some(p), "pid") as u32, message: text(Some(p), "message"), thread: text(Some(p), "thread"), location: text(Some(p), "location"), at_ms: uint(Some(p), "at_ms") }),
        touched_ms: uint(o, "touched_ms"),
        note: text(o, "note"),
    }
}

fn pushed_value(p: &Pushed) -> Value {
    obj(vec![("pair", p.pair.to_value()), ("commit", s(&p.commit)), ("origin_was", s(&p.origin_was)), ("at_ms", int(p.at_ms)), ("files", int(p.files as u64))])
}

/// The master's state: one document behind one lock, saved on every change.
pub struct Store {
    path: PathBuf,
    machine: String,
    doc: Mutex<Doc>,
    publish: Publish,
    /// woken on every change and every [`Store::kick`]
    changed: Condvar,
    generation: Mutex<u64>,
}

impl Store {
    /// `<home>/state.json` (a missing or unreadable file is an empty document).
    pub fn open(home: &Path, machine: &str, publish: Publish) -> Arc<Store> {
        let path = home.join("state.json");
        let doc = std::fs::read_to_string(&path).ok().and_then(|t| Doc::parse(&t)).unwrap_or_default();
        Arc::new(Store { path, machine: machine.to_string(), doc: Mutex::new(doc), publish, changed: Condvar::new(), generation: Mutex::new(0) })
    }

    pub fn read<T>(&self, f: impl FnOnce(&Doc) -> T) -> T {
        f(&self.doc.lock().unwrap())
    }

    /// Changes the document and saves it (whole, through a rename).
    pub fn update<T>(&self, f: impl FnOnce(&mut Doc) -> T) -> T {
        let mut doc = self.doc.lock().unwrap();
        let out = f(&mut doc);
        doc.trim();
        // written under the lock: the file never goes back to an older document
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = self.path.with_extension("json.new");
        if std::fs::write(&tmp, doc.to_value().to_json()).is_ok() {
            let _ = std::fs::rename(&tmp, &self.path);
        }
        drop(doc);
        self.kick();
        out
    }

    /// A state change on the event stream: `kind` is the section (`verify`, `after`, ...), `line`
    /// the answer-first text, `detail` the changed entry.
    pub fn event(&self, kind: &str, sev: Sev, line: String, detail: Option<String>) {
        (self.publish)(hub_record(&self.machine, "", 0, kind, sev, line, detail.unwrap_or_default()));
    }

    /// Wakes every waiter (a CLI asked for something).
    pub fn kick(&self) {
        *self.generation.lock().unwrap() += 1;
        self.changed.notify_all();
    }

    /// Sleeps up to `d`, or until the next change or kick.
    pub fn wait(&self, d: Duration) {
        let g = self.generation.lock().unwrap();
        let start = *g;
        let _ = self.changed.wait_timeout_while(g, d, |n| *n == start);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair(c: char) -> Pair {
        Pair { work: c.to_string().repeat(40), main: "c".repeat(40) }
    }

    #[test]
    fn doc_round_trip() {
        let leg = Leg { name: "full-a".into(), state: "fail".into(), lease: "L9".into(), node: "107".into(), summary: "NODEGATE-FAIL: fuzz".into(), errors: vec!["fuzz: seed 7 disagrees".into()], log: "/x/full-a.log".into() };
        let doc = Doc {
            ci: Default::default(),
            verify: Verify {
                on: false,
                verified: vec![Tip { pair: pair('a'), at_ms: 5, diff: "all".into() }],
                failures: vec![
                    Failure { pair: pair('b'), at_ms: 6, blame: Some(Blame { repo: "work".into(), sha: "d".repeat(40), subject: "x \"y\"".into(), good: pair('a'), bad: pair('b') }), unresolved: String::new(), legs: vec![leg.clone()] },
                    Failure { pair: pair('e'), at_ms: 7, blame: None, unresolved: "no-verified-first-parent-baseline".into(), legs: vec![] },
                ],
                run: Some(Run { id: "V3".into(), pair: pair('f'), started_ms: 8, diff: "affected".into(), dry: true, purpose: "bisect 1/3".into(), legs: vec![leg] }),
                last_full_ms: 9,
                asked: vec![Asked { pair: None, dry: false, full: true }, Asked { pair: Some(pair('a')), dry: true, full: false }],
            },
            allapps: Allapps {
                on: true,
                last: Some(AllappsRun { pair: pair('a'), at_ms: 1, secs: 600, verdict: "errors".into(), errors: vec![PkgErrors { pkg: "makepad-app-x".into(), errors: 2, first: vec!["E0425 a".into()] }], lease: "L1".into(), node: "101".into(), log: "l".into() }),
                running: None,
                asked: true,
            },
            after: vec![
                Waiter { id: "A1".into(), cond: Cond::Landed { branch: "rc-foo".into(), repo: "makepad".into(), sha: "a".repeat(40) }, cmd: vec!["launch".into(), "mac".into(), "makepad-fab".into()], by: "lane".into(), added_ms: 3, state: "waiting".into(), fired_ms: 0, exit: None, note: String::new(), log: String::new() },
                Waiter { id: "A2".into(), cond: Cond::Verified { contains: Some("abc".into()) }, cmd: vec!["verify".into()], by: String::new(), added_ms: 3, state: "done".into(), fired_ms: 4, exit: Some(0), note: "n".into(), log: "l".into() },
                Waiter { id: "A3".into(), cond: Cond::Free { node: "107".into() }, cmd: vec![], by: String::new(), added_ms: 0, state: "cancelled".into(), fired_ms: 0, exit: Some(-1), note: String::new(), log: String::new() },
                Waiter { id: "A4".into(), cond: Cond::Verified { contains: None }, ..Default::default() },
            ],
            after_next: 5,
            pushes: vec![Pushed { pair: pair('a'), commit: "1".repeat(40), origin_was: "2".repeat(40), at_ms: 4, files: 12 }],
            jobs: vec![
                Job { id: "J1".into(), batch: "B1".into(), lane: "l".into(), kind: "test".into(), prio: 5, cmd: vec!["cargo".into(), "test".into()], after: vec!["J0".into()], state: "paused".into(), snap: "S2".into(), snap_files: 3, attempts: 1, granted_ms: 5, shipped_ms: 6, first_out_ms: 7, exit: None, panic: Some(JobPanic { id: "P1".into(), pid: 9, message: "index".into(), thread: "main".into(), location: "a.rs:1".into(), at_ms: 3 }), ..Default::default() },
                Job { id: "J2".into(), state: "failed".into(), exit: Some(101), errors: vec!["error[E1]".into()], tail: vec!["x".into()], prio: 9, ..Default::default() },
            ],
            lanes: vec![LaneRec { name: "x".into(), path: "/w/x".into(), branch: "rc-x".into(), cbranch: "rc-x".into(), created_ms: 1, adopted: true, state: "live".into(), landed_ms: 4, keep: true, ended_ms: 0, note: "n".into() }],
            jobs_next: 2,
            batches_next: 1,
            snaps_next: 3,
        };
        let text = doc.to_value().to_json();
        assert_eq!(Doc::parse(&text), Some(doc.clone()));
        assert!(doc.section(Some("verify")).unwrap().starts_with("{\"on\":false"));
        assert_eq!(Doc::parse("{}"), Some(Doc::default()));
        assert!(Doc::default().verify.on && Doc::default().allapps.on);
    }

    #[test]
    fn ci_round_trip_and_timings_average() {
        let shard = CiShard { id: "smoke:apps/calc".into(), state: "fail".into(), node: "106".into(), lease: "L2".into(), secs: 12.5, summary: "exit 1".into(), errors: vec!["error: x".into()], log: "/l".into() };
        let run = CiRun { id: "R1".into(), pair: pair('a'), dry: true, started_ms: 1, ended_ms: 2, verdict: "fail".into(), shards: vec![shard], critical: "106 ends at 12 s".into(), boxes: vec!["106 12 s busy, 1 shards".into()] };
        let mut ci = Ci { timings: vec![("jit:01".into(), 40.0)], run: Some(run.clone()), runs: vec![run], rotation: 3 };
        ci.measured("jit:01", 20.0);
        ci.measured("test:x:lib", 5.0);
        assert_eq!((ci.timing("jit:01"), ci.timing("test:x:lib"), ci.timing("nope")), (Some(30.0), Some(5.0), None));
        let doc = Doc { ci, ..Default::default() };
        assert_eq!(Doc::parse(&doc.to_value().to_json()), Some(doc));
    }

    #[test]
    fn store_saves_and_lists_stay_bounded() {
        let dir = std::env::temp_dir().join(format!("devhub-state-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let s2 = seen.clone();
        let store = Store::open(&dir, "mac", Arc::new(move |r: Record| s2.lock().unwrap().push(r.kind.clone())));
        store.update(|d| {
            for i in 0..KEEP + 5 {
                d.verify.verified.push(Tip { pair: pair('a'), at_ms: i as u64, diff: "affected".into() });
            }
        });
        store.event("verify", Sev::Info, "verify OK".into(), None);
        let again = Store::open(&dir, "mac", Arc::new(|_| {}));
        assert_eq!(again.read(|d| (d.verify.verified.len(), d.verify.verified[0].at_ms)), (KEEP, 5));
        assert_eq!(*seen.lock().unwrap(), vec!["verify".to_string()]);
        let _ = std::fs::remove_file(dir.join("state.json"));
        let _ = std::fs::remove_dir(&dir);
    }
}
