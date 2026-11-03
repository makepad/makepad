//! devhub's workflow state: what `devhub land`, `lanes`, `verify`, `jitlegs`, `after`, `allapps` and `push`
//! know, in one master-owned document (`~/.makepad/devhub/state.json`). Large output and completed
//! records live in append-only files referenced by byte slices; unchanged updates write nothing. Every change is also a record on the master's event stream (its kind
//! is the section's name), so an agent blocks on `devhub watch <kinds>` instead of polling.
//!
//! The sections are typed. A command that keeps state adds its struct here, a field on [`Doc`] and
//! its lines in [`Doc::to_value`] / [`Doc::from_value`]; it changes state only through
//! [`Store::update`] and says so with [`Store::event`].

use crate::proto::{hub_record, Record, Sev};
use makepad_strict_json::{obj, s, Value};
use std::path::{Path, PathBuf};
use std::io::{Read, Seek, Write};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// Copy-on-write history: taking a transaction snapshot shares the immutable completed
/// runs. Only a mutation of that history clones it; ordinary job updates never do.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SharedVec<T: Clone>(Arc<Vec<T>>);
impl<T: Clone> std::ops::Deref for SharedVec<T> {
    type Target = Vec<T>;
    fn deref(&self) -> &Vec<T> { &self.0 }
}
impl<T: Clone> std::ops::DerefMut for SharedVec<T> {
    fn deref_mut(&mut self) -> &mut Vec<T> { Arc::make_mut(&mut self.0) }
}
impl<T: Clone> From<Vec<T>> for SharedVec<T> {
    fn from(v: Vec<T>) -> Self { Self(Arc::new(v)) }
}
impl<T: Clone> FromIterator<T> for SharedVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self { Self::from(Vec::from_iter(iter)) }
}
impl<T: Clone> IntoIterator for SharedVec<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter { Arc::unwrap_or_clone(self.0).into_iter() }
}
impl<'a, T: Clone> IntoIterator for &'a SharedVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter { self.iter() }
}

pub type Publish = Arc<dyn Fn(Record) -> u64 + Send + Sync>;

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
        if self.main.is_empty() { format!("work@{}", self.work) } else { format!("work@{},main@{}", self.work, self.main) }
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
    /// when a full sweep was last attempted (also updated on success)
    pub last_full_ms: u64,
    /// runs asked for on the CLI, first first
    pub asked: Vec<Asked>,
    /// a pair that failed while its bisect goes (a master restart goes on from here)
    pub failing: Option<Failing>,
}

/// A failed pair being bisected.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Failing {
    pub pair: Pair,
    /// the legs that failed it
    pub legs: Vec<Leg>,
    /// the pairs its bisect gated, and their verdict word
    pub gated: Vec<(Pair, String)>,
    /// what failed on the newest verified pair too, once that run said (None: not yet)
    pub old: Option<Vec<String>>,
}

impl Default for Verify {
    fn default() -> Verify {
        Verify { on: true, verified: Vec::new(), failures: Vec::new(), run: None, last_full_ms: 0, asked: Vec::new(), failing: None }
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
    /// started for the first run asked on the CLI
    pub asked: bool,
    /// the times it ended incomplete and goes on (what passed stands)
    pub tries: u32,
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
    /// the run on its lease: a restarted master follows it from there
    pub run_n: u64,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Asked {
    /// None: the newest landed pair now
    pub pair: Option<Pair>,
    pub dry: bool,
    /// diff every program
    pub full: bool,
}

// ---- jitlegs ----

#[derive(Clone, Debug, PartialEq, Default)]
pub struct JitLegs {
    /// oldest first, including the run in progress
    pub runs: Vec<JitLegsRun>,
    /// `devhub jitlegs run`: run the newest landed pair even if its inputs are unchanged
    pub asked: bool,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct JitLegsRun {
    pub pair: Pair,
    pub started_ms: u64,
    /// zero while running
    pub ended_ms: u64,
    pub legs: Vec<Leg>,
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
    /// a show of the app on the device runs, started after the waiter was added
    Shown { app: String, device: String },
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
            Cond::Shown { app, device } => format!("shown {app} on {device}"),
            Cond::None => "-".into(),
        }
    }

    fn to_value(&self) -> Value {
        match self {
            Cond::Landed { branch, repo, sha } => obj(vec![("on", s("landed")), ("branch", s(branch)), ("repo", s(repo)), ("sha", s(sha))]),
            Cond::Free { node } => obj(vec![("on", s("free")), ("node", s(node))]),
            Cond::Verified { contains } => obj(vec![("on", s("verified")), ("contains", contains.as_deref().map_or(Value::Null, s))]),
            Cond::Shown { app, device } => obj(vec![("on", s("shown")), ("app", s(app)), ("device", s(device))]),
            Cond::None => Value::Null,
        }
    }

    pub fn from_value(v: Option<&Value>) -> Cond {
        match text(v, "on").as_str() {
            "landed" => Cond::Landed { branch: text(v, "branch"), repo: text(v, "repo"), sha: text(v, "sha") },
            "free" => Cond::Free { node: text(v, "node") },
            "shown" => Cond::Shown { app: text(v, "app"), device: text(v, "device") },
            "verified" => Cond::Verified { contains: v.and_then(|v| v.get("contains")).and_then(|c| c.as_str()).map(str::to_string) },
            _ => Cond::None,
        }
    }
}

// ---- push ----

/// One collapsed push of a selected tip to origin/work (`devhub push`).
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
/// balancing the boxes), the gate and sweep in progress and the finished runs.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Ci {
    /// seconds per shard id (a moving average of its finished runs), for balancing
    pub timings: Vec<(String, f64)>,
    pub run: Option<CiRun>,
    /// the background sweep, independent of the gate
    pub sweep_run: Option<CiRun>,
    /// finished runs, oldest first
    pub runs: SharedVec<CiRun>,
    /// runs started: rotates which OS each app's smoke script runs on
    pub rotation: u64,
}

impl Ci {
    pub fn slot(&self, sweep: bool) -> &Option<CiRun> {
        if sweep { &self.sweep_run } else { &self.run }
    }

    pub fn slot_mut(&mut self, sweep: bool) -> &mut Option<CiRun> {
        if sweep { &mut self.sweep_run } else { &mut self.run }
    }

    pub fn timing(&self, id: &str) -> Option<f64> {
        self.timings.iter().find(|(k, _)| k == id).map(|(_, s)| *s)
    }

    /// A mark kept beside the timings (`failing:<package>`: its tests failed when they last ran).
    pub fn set_mark(&mut self, key: &str, on: bool) {
        self.timings.retain(|(k, _)| k != key);
        if on {
            self.timings.push((key.to_string(), 1.0));
        }
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
    /// The lock of an in-flight run, for restart identity only.
    pub lock_digest: String,
    /// A calling checkout; empty for the rolling verifier.
    pub checkout: String,
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
    /// where the wall went: time on the boxes, queued for a lease, nothing running at all
    pub fleet: String,
    /// per box: `<node> <busy s> s, <n> shards`
    pub boxes: Vec<String>,
    /// the plan's rotation (a run that goes on keeps its plan)
    pub rotation: u64,
    /// the timings and marks its plan was made with (a run that goes on keeps its batches)
    pub plan_timings: Vec<(String, f64)>,
    /// the sweep (every crate's tests, `devhub ci sweep`), not the gate's app checks
    pub sweep: bool,
    /// `ci stop` was asked: a master restart resumes it only to finish its running shards
    pub stopping: bool,
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
    /// what it runs (a hash of its steps): a result stands only for the same steps
    pub sig: String,
    /// the step it is at, and that step's run on its lease
    pub step: u64,
    pub run_n: u64,
    /// steps that passed on any attempt of this shard
    pub steps_ok: Vec<u64>,
    /// why the steps before failed
    pub failed: Vec<String>,
    /// the pair a kept result is from
    pub kept: String,
    /// the box it failed on and that box's OS: a second box of that OS decides. A pass there is a
    /// pass, with this box, its summary and `errors` kept and reported.
    pub failed_on: String,
    pub failed_os: String,
    pub failed_why: String,
    /// the attempt in progress: when its lease was asked for and granted (ms)
    pub asked_ms: u64,
    pub granted_ms: u64,
    /// every attempt of this run on a box: what the run's critical path and fleet time are read from
    pub spans: Vec<CiSpan>,
    /// the boxes it came back incomplete from, and how often (twice there: it goes elsewhere)
    pub lost: Vec<(String, u32)>,
}

/// One attempt of a shard on a box: asked for a lease, granted (0: never, it went back to the pool),
/// ended, all in wall ms.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct CiSpan {
    pub node: String,
    pub asked_ms: u64,
    pub granted_ms: u64,
    pub ended_ms: u64,
}

impl CiShard {
    /// The shard as a verify leg (the legs a failure keeps).
    pub fn leg(&self) -> Leg {
        Leg { name: self.id.clone(), state: self.state.clone(), lease: self.lease.clone(), node: self.node.clone(), summary: self.summary.clone(), errors: self.errors.clone(), log: self.log.clone(), run_n: 0 }
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
        ("sig", s(&c.sig)),
        ("step", int(c.step)),
        ("run_n", int(c.run_n)),
        ("steps_ok", Value::Arr(c.steps_ok.iter().map(|&k| int(k)).collect())),
        ("failed", strs(&c.failed)),
        ("kept", s(&c.kept)),
        ("failed_on", s(&c.failed_on)),
        ("failed_os", s(&c.failed_os)),
        ("failed_why", s(&c.failed_why)),
        ("asked_ms", int(c.asked_ms)),
        ("granted_ms", int(c.granted_ms)),
        ("spans", Value::Arr(c.spans.iter().map(|x| obj(vec![("node", s(&x.node)), ("asked_ms", int(x.asked_ms)), ("granted_ms", int(x.granted_ms)), ("ended_ms", int(x.ended_ms))])).collect())),
        ("lost", Value::Obj(c.lost.iter().map(|(n, k)| (n.clone(), int(*k as u64))).collect())),
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
        sig: text(o, "sig"),
        step: uint(o, "step"),
        run_n: uint(o, "run_n"),
        steps_ok: arr(o, "steps_ok").iter().filter_map(Value::as_u64).collect(),
        failed: strs_from(o, "failed"),
        kept: text(o, "kept"),
        failed_on: text(o, "failed_on"),
        failed_os: text(o, "failed_os"),
        failed_why: text(o, "failed_why"),
        asked_ms: uint(o, "asked_ms"),
        granted_ms: uint(o, "granted_ms"),
        spans: arr(o, "spans")
            .iter()
            .map(|x| {
                let o = Some(x);
                CiSpan { node: text(o, "node"), asked_ms: uint(o, "asked_ms"), granted_ms: uint(o, "granted_ms"), ended_ms: uint(o, "ended_ms") }
            })
            .collect(),
        lost: match v.get("lost") {
            Some(Value::Obj(f)) => f.iter().filter_map(|(k, x)| x.as_u64().map(|n| (k.clone(), n as u32))).collect(),
            _ => Vec::new(),
        },
    }
}

pub(crate) fn ci_run_value(r: &CiRun) -> Value {
    obj(vec![
        ("checkout", s(&r.checkout)),
        ("lock_digest", s(&r.lock_digest)),
        ("id", s(&r.id)),
        ("pair", r.pair.to_value()),
        ("dry", Value::Bool(r.dry)),
        ("started_ms", int(r.started_ms)),
        ("ended_ms", int(r.ended_ms)),
        ("verdict", s(&r.verdict)),
        ("shards", Value::Arr(r.shards.iter().map(ci_shard_value).collect())),
        ("critical", s(&r.critical)),
        ("fleet", s(&r.fleet)),
        ("boxes", strs(&r.boxes)),
        ("rotation", int(r.rotation)),
        ("plan_timings", Value::Obj(r.plan_timings.iter().map(|(k, v)| (k.clone(), Value::F64(*v))).collect())),
        ("sweep", Value::Bool(r.sweep)),
        ("stopping", Value::Bool(r.stopping)),
    ])
}

fn ci_run_from(v: &Value) -> CiRun {
    let o = Some(v);
    CiRun {
        checkout: text(o, "checkout"),
        lock_digest: text(o, "lock_digest"),
        id: text(o, "id"),
        pair: Pair::from_value(v.get("pair")),
        dry: flag(o, "dry", false),
        started_ms: uint(o, "started_ms"),
        ended_ms: uint(o, "ended_ms"),
        verdict: text(o, "verdict"),
        shards: arr(o, "shards").iter().map(ci_shard_from).collect(),
        critical: text(o, "critical"),
        fleet: text(o, "fleet"),
        boxes: strs_from(o, "boxes"),
        rotation: uint(o, "rotation"),
        sweep: flag(o, "sweep", false),
        stopping: flag(o, "stopping", false),
        plan_timings: match v.get("plan_timings") {
            Some(Value::Obj(f)) => f.iter().filter_map(|(k, x)| f64_of(x).map(|s| (k.clone(), s))).collect(),
            _ => Vec::new(),
        },
    }
}

fn ci_value(c: &Ci) -> Value {
    obj(vec![
        ("timings", Value::Obj(c.timings.iter().map(|(k, v)| (k.clone(), Value::F64(*v))).collect())),
        ("run", c.run.as_ref().map_or(Value::Null, ci_run_value)),
        ("sweep_run", c.sweep_run.as_ref().map_or(Value::Null, ci_run_value)),
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
        sweep_run: v.and_then(|x| x.get("sweep_run")).filter(|r| !r.is_null()).map(ci_run_from),
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
    /// `devhub lane live <name> on`: the user waits for this lane (the manager's mark): its
    /// landings take the fast path (merge + check; the boot at `devhub show`, the tests after)
    pub waited: bool,
    pub ended_ms: u64,
    /// what the cleanup did, a drop's bundles, or why a landed lane is still kept
    pub note: String,
}

// ---- shows ----

/// An app shown to the user on a display device (`devhub show`): one per app per device.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Show {
    /// `V<n>`
    pub id: String,
    /// the package (`makepad-fab`)
    pub app: String,
    /// `mac`, a WM node, or `quest`
    pub device: String,
    /// `waiting` (its --after), `starting`, `running`, `paused` (a parked panic), then `closed`
    /// (the user closed it), `crashed`, `stopped`, `failed`, `replaced`, `cancelled`
    pub state: String,
    /// waiting for this
    pub cond: Cond,
    pub pid: u32,
    /// the landed commit it runs (work at launch; a landing while it runs patches it live)
    pub sha: String,
    pub added_ms: u64,
    pub started_ms: u64,
    pub ended_ms: u64,
    pub exit: Option<i32>,
    pub panic: Option<JobPanic>,
    pub by: String,
    pub log: String,
    pub note: String,
    /// `devhub show pin`: an operation runs on it: no landing or edit reaches it, it does not restart
    pub pinned: bool,
    /// when its launch began (its --after met)
    pub launch_ms: u64,
    /// what the launch took, stage by stage (`box 3 s, jit reused, first frame 6 s, settle 8 s`)
    pub stages: String,
}

impl Show {
    pub fn active(&self) -> bool {
        matches!(self.state.as_str(), "waiting" | "starting" | "running" | "paused")
    }
}

// ---- live jobs ----

/// A long-running app on a box that edits patch into (`devhub live`).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Live {
    /// `W<n>`
    pub id: String,
    pub app: String,
    pub test: bool,
    /// target selection and libtest arguments, including `--`
    pub test_args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// paths the run names in `DEVHUB_ALSO`: they ship with every patch, `local/` included
    pub also: Vec<String>,
    pub lane: String,
    pub src: String,
    pub cost: String,
    pub est_min: Option<f64>,
    pub class: String,
    pub priority: String,
    pub loader: String,
    pub agent: String,
    /// the worktree whose edits stream in
    pub tree: String,
    pub rev: String,
    pub needs: String,
    pub visible: bool,
    pub lease: String,
    pub node: String,
    /// the app's JIT on the box
    pub pid: u32,
    /// the run in its lease (a new master follows it from there)
    pub run_n: u64,
    pub run_log_start: u64,
    /// Content last shipped, including whiteouts (`-`).
    pub sources: std::collections::BTreeMap<String, String>,
    /// `starting`, `running`, `paused` (a parked panic), then `ended`, `failed`
    pub state: String,
    pub started_ms: u64,
    /// the last edit or action (the idle timeout counts from it)
    pub touched_ms: u64,
    pub ended_ms: u64,
    /// the last edit: files, when saved, shipped, patched (or refused), the first frame after
    pub patch: Option<LivePatch>,
    pub panic: Option<JobPanic>,
    pub log: String,
    pub note: String,
}

impl Live {
    pub fn test_code(&self) -> i32 {
        let x = self;
        match &x.patch {
            Some(p) if p.outcome == "jit_error" => 3,
            Some(p) if matches!(p.outcome.as_str(), "none" | "failed") => 2,
            Some(p) if x.state != "paused" && p.tests.as_ref().is_some_and(|t| t.failed == 0) => 0,
            _ => 1,
        }
    }

    pub fn test_result(&self, seconds: f64) -> String {
        let mut loaded = self.clone();
        if let Err(e) = loaded.load_text() { return format!("live log: {e}\n"); }
        let x = &loaded;
        let Some(p) = &x.patch else { return format!("devhub test: {} {}: {}\n", x.app, x.state, x.note) };
        if p.outcome == "jit_error" { return jit_error_verdict(p); }
        let Some(t) = &p.tests else { return format!("{}\n{}", if p.reason.is_empty() { &x.note } else { &p.reason }, p.output) };
        let mut out = format!("devhub test: {} {}/{} ran ({} skipped, selected {}) in {seconds:.3} s (ship {} ms, patch {:.0} ms, tests {:.0} ms) on {} [live {}]\n", x.app, t.passed, t.ran, t.skipped, if t.selected == "affected" { "affected" } else { "all" }, p.shipped_ms.saturating_sub(p.edit_ms.max(p.called_ms)), t.compile_ms, t.ms, x.node, x.id);
        for name in &t.failures { out.push_str(&format!("  {name}\n")); }
        out.push_str(&p.output);
        out
    }

    pub fn active(&self) -> bool {
        matches!(self.state.as_str(), "starting" | "running" | "paused")
    }
}

/// A live patch that ended in a JIT error: the typed verdict line, else the raw error.
pub fn jit_error_verdict(p: &LivePatch) -> String {
    if let Some(verdict) = makepad_remote::hot::Verdict::from_output(makepad_remote::hot::JIT_ERROR_EXIT, &p.reason) { return format!("{}\n", verdict.line()); }
    format!("JIT error, not your change: {}\n", p.jit_error)
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct LivePatch {
    pub files: Vec<String>,
    /// the JIT's display text for the patch awaiting tests
    pub edit: String,
    /// the shared identity of patchverified and its testverdict
    pub group: Option<u64>,
    /// last event; an exec can reuse its sequence number with different contents
    pub seq: u64,
    pub cursor: String,
    pub edit_ms: u64,
    /// when the call that shipped it came (a test session ships on its call: a save the agent
    /// made minutes before it is no ship time)
    pub called_ms: u64,
    pub shipped_ms: u64,
    /// Patch state, including `recovering` and terminal `jit_error`.
    pub outcome: String,
    pub outcome_ms: u64,
    pub frame_ms: u64,
    pub tests: Option<LiveTests>,
    pub verdict_ms: u64,
    pub log_start: u64,
    pub log_end: u64,
    pub output: String,
    pub output_ref: TextRef,
    pub reason_ref: TextRef,
    pub retried: bool,
    pub jit_error: u64,
    /// why a restart or a refusal
    pub reason: String,
}

/// A master-owned append-only log slice, never embedded in the state document.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TextRef {
    pub path: String,
    pub offset: u64,
    pub len: u64,
}
impl TextRef {
    fn to_value(&self) -> Value { obj(vec![("path", s(&self.path)), ("offset", int(self.offset)), ("len", int(self.len))]) }
    fn from_value(v: Option<&Value>) -> Self { Self { path: text(v, "path"), offset: uint(v, "offset"), len: uint(v, "len") } }
    pub fn read(&self) -> Result<String, String> {
        if self.len == 0 { return Ok(String::new()); }
        let mut file = std::fs::File::open(&self.path).map_err(|e| e.to_string())?;
        file.seek(std::io::SeekFrom::Start(self.offset)).map_err(|e| e.to_string())?;
        let mut text = String::new();
        file.take(self.len).read_to_string(&mut text).map_err(|e| e.to_string())?;
        if text.len() as u64 != self.len { return Err("truncated live log slice".into()); }
        Ok(text)
    }
    fn spill(&mut self, path: &Path, text: &mut String) -> Result<(), String> {
        if text.is_empty() { return Ok(()); }
        if self.len == text.len() as u64 && self.read()? == *text { text.clear(); return Ok(()); }
        std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
        let mut file = std::fs::OpenOptions::new().create(true).append(true).open(path).map_err(|e| e.to_string())?;
        let offset = file.metadata().map_err(|e| e.to_string())?.len();
        file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        #[cfg(unix)]
        std::fs::File::open(path.parent().unwrap()).and_then(|dir| dir.sync_all()).map_err(|e| e.to_string())?;
        *self = Self { path: path.to_string_lossy().into_owned(), offset, len: text.len() as u64 };
        text.clear();
        Ok(())
    }
}
impl Live {
    pub fn load_text(&mut self) -> Result<(), String> {
        if let Some(p) = &mut self.patch {
            if p.output.is_empty() { p.output = p.output_ref.read()?; }
            if p.reason.is_empty() { p.reason = p.reason_ref.read()?; }
        }
        Ok(())
    }
}
impl Doc {
    fn spill(&mut self, home: &Path) -> Result<(), String> {
        for live in &mut self.lives {
            if let Some(patch) = &mut live.patch {
                // IDs are generated by the master; encode before using one as a file name.
                let id: String = live.id.bytes().map(|b| format!("{b:02x}")).collect();
                let path = home.join("live-logs").join(format!("{id}.log"));
                patch.output_ref.spill(&path, &mut patch.output)?;
                patch.reason_ref.spill(&path, &mut patch.reason)?;
            }
        }
        Ok(())
    }
}
// Finished records retain their existing in-memory representation for callers. Their durable
// representation is an immutable file slice, so a job event never serializes CI history or
// another job's output. This is the document's storage layout, not a second mutable cache.
impl Doc {
    fn disk_value(&self, old: &Doc, previous: &std::collections::BTreeMap<String, TextRef>, home: &Path) -> Result<(Value, std::collections::BTreeMap<String, TextRef>), String> {
        let mut compact = self.clone();
        let mut records = std::collections::BTreeMap::new();
        let mut order = Vec::new();
        let mut save = |key: String, unchanged: bool, value: &dyn Fn() -> Value| -> Result<(), String> {
            let reference = if unchanged { previous.get(&key).cloned() } else { None };
            let reference = match reference {
                Some(r) => r,
                None => {
                    let mut r = TextRef::default();
                    r.spill(&home.join("records.jsonl"), &mut format!("{}\n", value().to_json()))?;
                    r
                }
            };
            order.push(key.clone());
            records.insert(key, reference);
            Ok(())
        };
        for run in &self.ci.runs {
            save(format!("ci/{}", run.id), old.ci.runs.iter().find(|r| r.id == run.id).is_some_and(|r| std::ptr::eq(r, run) || r == run), &|| ci_run_value(run))?;
        }
        compact.ci.runs = self.ci.runs.iter().map(|r| CiRun {
            id: r.id.clone(), pair: r.pair.clone(), dry: r.dry, checkout: r.checkout.clone(), lock_digest: r.lock_digest.clone(),
            started_ms: r.started_ms, ended_ms: r.ended_ms, verdict: r.verdict.clone(), critical: r.critical.clone(), fleet: r.fleet.clone(),
            rotation: r.rotation, sweep: r.sweep, stopping: r.stopping, ..Default::default()
        }).collect();
        for job in self.jobs.iter().filter(|j| j.done()) {
            save(format!("jobs/{}", job.id), old.jobs.iter().find(|j| j.id == job.id) == Some(job), &|| job_value(job))?;
        }
        for job in compact.jobs.iter_mut().filter(|j| j.done()) { job.errors.clear(); job.tail.clear(); }
        for live in self.lives.iter().filter(|x| !x.active()) {
            save(format!("lives/{}", live.id), old.lives.iter().find(|x| x.id == live.id) == Some(live), &|| live_state_value(live))?;
        }
        for live in compact.lives.iter_mut().filter(|x| !x.active()) { live.sources.clear(); }
        for (i, failure) in self.verify.failures.iter().enumerate() {
            save(format!("failures/{i}"), old.verify.failures.get(i) == Some(failure), &|| failure_value(failure))?;
        }
        for failure in &mut compact.verify.failures { failure.legs.clear(); }
        for (i, run) in self.jitlegs.runs.iter().enumerate().filter(|(_, r)| r.ended_ms > 0) {
            save(format!("jitlegs/{i}"), old.jitlegs.runs.get(i) == Some(run), &|| jitlegs_run_value(run))?;
        }
        for run in compact.jitlegs.runs.iter_mut().filter(|r| r.ended_ms > 0) { run.legs.clear(); }
        let mut value = compact.to_value();
        if let Value::Obj(fields) = &mut value {
            for (key, value) in fields.iter_mut() {
                if key == "jobs" {
                    *value = Value::Arr(self.jobs.iter().map(|j| if j.done() { obj(vec![("id", s(&j.id)), ("state", s(&j.state)), ("lane", s(&j.lane)), ("submitted_ms", int(j.submitted_ms)), ("ended_ms", int(j.ended_ms))]) } else { job_value(j) }).collect());
                }
                if key == "lives" {
                    *value = Value::Arr(self.lives.iter().map(|x| if !x.active() { obj(vec![("id", s(&x.id)), ("state", s(&x.state)), ("app", s(&x.app)), ("lane", s(&x.lane)), ("started_ms", int(x.started_ms))]) } else { live_state_value(x) }).collect());
                }
            }
            fields.push(("records".into(), Value::Arr(order.iter().map(|key| obj(vec![("key", s(key)), ("slice", records[key].to_value())])).collect())));
        }
        Ok((value, records))
    }

    fn from_disk(mut value: Value, records: &mut std::collections::BTreeMap<String, TextRef>) -> Result<Doc, String> {
        // Metadata keeps the same schema. Embedded diagnostic/history payload fields
        // are not read; file slices supply them. No old payload is migrated or imported.
        fn remove(v: &mut Value, key: &str) {
            if let Value::Obj(fields) = v { fields.retain(|(k, _)| k != key); }
        }
        fn list_mut(v: &mut Value, key: &str, f: impl Fn(&mut Value)) {
            if let Value::Obj(fields) = v {
                if let Some((_, Value::Arr(items))) = fields.iter_mut().find(|(k, _)| k == key) {
                    for item in items { f(item); }
                }
            }
        }
        if let Value::Obj(fields) = &mut value {
            for (key, v) in fields {
                match key.as_str() {
                    "ci" => list_mut(v, "runs", |run| { remove(run, "shards"); remove(run, "boxes"); remove(run, "plan_timings"); }),
                    "jitlegs" => list_mut(v, "runs", |run| if uint(Some(run), "ended_ms") > 0 { remove(run, "legs"); }),
                    "verify" => list_mut(v, "failures", |failure| remove(failure, "legs")),
                    "jobs" => if let Value::Arr(jobs) = v { for job in jobs { remove(job, "errors"); remove(job, "tail"); } },
                    _ => {}
                }
            }
        }
        let mut doc = Doc::from_value(&value);
        for record in arr(Some(&value), "records") {
            let key = text(Some(record), "key");
            let reference = TextRef::from_value(record.get("slice"));
            let text = reference.read().map_err(|e| format!("read workflow record {key}: {e}; state kept"))?;
            let v = makepad_strict_json::parse_depth(text.as_bytes(), 16).map_err(|e| format!("invalid workflow record {key}: {e:?}; state kept"))?;
            match key.split('/').next().unwrap_or("") {
                "ci" => { let run = ci_run_from(&v); if let Some(old) = doc.ci.runs.iter_mut().find(|r| r.id == run.id) { *old = run; } else { doc.ci.runs.push(run); } },
                "jobs" => { let job = job_from(&v); if let Some(old) = doc.jobs.iter_mut().find(|j| j.id == job.id) { *old = job; } else { doc.jobs.push(job); } },
                "lives" => { let live = live_state_from(&v); if let Some(old) = doc.lives.iter_mut().find(|x| x.id == live.id) { *old = live; } else { doc.lives.push(live); } },
                "failures" => { let failure = failure_from(&v); if let Some(old) = doc.verify.failures.iter_mut().find(|x| x.pair == failure.pair && x.at_ms == failure.at_ms) { *old = failure; } else { doc.verify.failures.push(failure); } },
                "jitlegs" => { let run = jitlegs_run_from(&v); if let Some(old) = doc.jitlegs.runs.iter_mut().find(|x| x.pair == run.pair && x.started_ms == run.started_ms) { *old = run; } else { doc.jitlegs.runs.push(run); } },
                _ => return Err(format!("unknown workflow record {key}; state kept")),
            }
            records.insert(key, reference);
        }
        doc.jobs.sort_by_key(|j| j.id.strip_prefix('J').and_then(|id| id.parse::<u64>().ok()).unwrap_or(0));
        doc.lives.sort_by_key(|x| x.id.strip_prefix('W').and_then(|id| id.parse::<u64>().ok()).unwrap_or(0));
        Ok(doc)
    }
}

fn archive_history(doc: &mut Doc) -> Vec<(&'static str, Value)> {
    let mut archive = Vec::new();
    for (section, entries, field, terminal) in [("land", &mut doc.land, "state", "landed"), ("merges", &mut doc.merges, "result", "landed")] {
        let ended = entries.iter().filter(|v| v.get(field).and_then(Value::as_str) == Some(terminal)).count();
        let mut excess = ended.saturating_sub(KEEP);
        entries.retain(|v| {
            if excess > 0 && v.get(field).and_then(Value::as_str) == Some(terminal) {
                archive.push((section, v.clone())); excess -= 1; false
            } else { true }
        });
    }
    archive
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct LiveTests {
    pub edit: String,
    pub group: u64,
    pub ran: u64,
    pub passed: u64,
    pub failed: u64,
    pub ignored: u64,
    pub skipped: u64,
    pub selected: String,
    pub failures: Vec<String>,
    pub ms: f64,
    pub compile_ms: f64,
    pub restarted: bool,
}

// ---- job ----

/// One self-contained command run (`devhub job submit`): the master places it on a free box the
/// moment one fits and releases the box when the command ends; no box waits for an agent.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Job {
    /// `J<n>`
    pub id: String,
    /// its run on its lease: a restarted master follows it from there
    pub run_n: u64,
    /// its log's bytes before that run's output (the node's waits it noted: `L1 104: selecting`),
    /// so a restarted master follows the run from where the log stands
    pub log_base: u64,
    /// the batch it came in (`B<n>`), or empty
    pub batch: String,
    pub lane: String,
    pub src: String,
    pub cost: String,
    pub est_min: Option<f64>,
    pub cores: Option<f64>,
    pub ram_gb: Option<f64>,
    pub whole: bool,
    pub class: String,
    pub priority: String,
    pub loader: String,
    pub agent: String,
    /// the lease kind (`test`, `build`, `gpu`, ...)
    pub kind: String,
    /// `arch=x64,os=linux`, or empty
    pub needs: String,
    /// a preferred node, or empty
    pub node_pref: String,
    /// `--only`: that node or wait, no twin
    pub only: bool,
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
    pub cwd: String,
    pub holder: Option<u32>,
    /// A JIT test's opposite-source shadow pair; retained until its evidence reaches shadow.jsonl.
    pub shadow: bool,
    pub shadow_job: String,
    pub shadow_of: String,
    pub shadow_recorded: bool,
    /// a sampled live verdict this native run checks (JSON: live id, group, counts, failures)
    pub live_sample: String,
    pub jit_error: String,
    /// jobs that must end ok first
    pub after: Vec<String>,
    /// `queued` (deps), `leasing` (waiting for a box), `running`, `paused` (a parked panic; the box
    /// stays held), then `ok`, `failed`, `panicked`, `cancelled`, `skipped` (a dep did not end ok),
    /// `lost` (the master restarted under it)
    pub state: String,
    pub submitted_ms: u64,
    /// when it first asked for a box (its place in the queue)
    pub asked_ms: u64,
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
    /// when a `job wait` first asked for it while it ran (an agent is blocked on it); 0 once its
    /// wait went to the agent-wait ledger
    pub waited_ms: u64,
    pub note: String,
}

impl Job {
    pub fn done(&self) -> bool {
        matches!(self.state.as_str(), "ok" | "failed" | "panicked" | "cancelled" | "skipped" | "lost" | "jit-error")
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
    pub devhub_shadow: bool,
    pub verify: Verify,
    pub jitlegs: JitLegs,
    pub allapps: Allapps,
    pub after: Vec<Waiter>,
    /// the id number of the next waiter (`A<n>`)
    pub after_next: u64,
    pub pushes: Vec<Pushed>,
    pub ci: Ci,
    pub lanes: Vec<LaneRec>,
    pub shows: Vec<Show>,
    pub shows_next: u64,
    pub lives: Vec<Live>,
    pub lives_next: u64,
    /// the next landing queue number: never reused, even after landed entries are trimmed
    pub land_next: u64,
    pub jobs: Vec<Job>,
    /// Durable submission receipts, independent of trimmed job display history.
    pub submissions: std::collections::BTreeMap<String, String>,
    /// the id numbers of the next job (`J<n>`) and batch (`B<n>`)
    pub jobs_next: u64,
    pub batches_next: u64,
    /// the id number of the next job snapshot (`S<n>`)
    pub snaps_next: u64,
    /// the landing train's hand-ins (`land.rs`'s entries, as their JSON), oldest first
    pub land: Vec<Value>,
    /// why the landing train is paused (empty: it runs)
    pub land_paused: String,
    /// the merge bot's last result per lane half (`sync/lanes.rs`'s lanes, as their JSON)
    pub merges: Vec<Value>,
    /// the branches the merge bot keeps off (`devhub hold`): (branch, why)
    pub held: Vec<(String, String)>,
    /// the last pre-warm record per node (`sync/prewarm.rs`)
    pub prewarm: Vec<Value>,
}

impl Doc {
    pub fn to_value(&self) -> Value {
        let v = &self.verify;
        let a = &self.allapps;
        obj(vec![
            ("DEVHUB_SHADOW", Value::Bool(self.devhub_shadow)),
            (
                "verify",
                obj(vec![
                    ("on", Value::Bool(v.on)),
                    ("verified", Value::Arr(v.verified.iter().map(tip_value).collect())),
                    ("failures", Value::Arr(v.failures.iter().map(failure_value).collect())),
                    ("run", v.run.as_ref().map_or(Value::Null, run_value)),
                    ("last_full_ms", int(v.last_full_ms)),
                    ("asked", Value::Arr(v.asked.iter().map(|q| obj(vec![("pair", q.pair.as_ref().map_or(Value::Null, Pair::to_value)), ("dry", Value::Bool(q.dry)), ("full", Value::Bool(q.full))])).collect())),
                    ("failing", v.failing.as_ref().map_or(Value::Null, failing_value)),
                ]),
            ),
            ("jitlegs", jitlegs_value(&self.jitlegs)),
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
            ("shows", Value::Arr(self.shows.iter().map(show_value).collect())),
            ("shows_next", int(self.shows_next)),
            ("lives", Value::Arr(self.lives.iter().map(live_state_value).collect())),
            ("lives_next", int(self.lives_next)),
            ("land_next", int(self.land_next)),
            ("jobs", Value::Arr(self.jobs.iter().map(job_value).collect())),
            ("submissions", Value::Arr(self.submissions.iter().map(|(id, result)| obj(vec![("id", s(id)), ("result", s(result))])).collect())),
            ("jobs_next", int(self.jobs_next)),
            ("batches_next", int(self.batches_next)),
            ("snaps_next", int(self.snaps_next)),
            ("land", Value::Arr(self.land.clone())),
            ("land_paused", s(&self.land_paused)),
            ("merges", Value::Arr(self.merges.clone())),
            ("held", Value::Arr(self.held.iter().map(|(b, w)| obj(vec![("branch", s(b)), ("why", s(w))])).collect())),
            ("prewarm", Value::Arr(self.prewarm.clone())),
        ])
    }

    pub fn from_value(v: &Value) -> Doc {
        let ve = v.get("verify");
        let al = v.get("allapps");
        Doc {
            devhub_shadow: flag(Some(v), "DEVHUB_SHADOW", false),
            verify: Verify {
                on: flag(ve, "on", true),
                verified: arr(ve, "verified").iter().map(|t| Tip { pair: Pair::from_value(t.get("pair")), at_ms: uint(Some(t), "at_ms"), diff: text(Some(t), "diff") }).collect(),
                failures: arr(ve, "failures").iter().map(failure_from).collect(),
                run: ve.and_then(|x| x.get("run")).filter(|r| !r.is_null()).map(run_from),
                last_full_ms: uint(ve, "last_full_ms"),
                asked: arr(ve, "asked").iter().map(|q| Asked { pair: q.get("pair").filter(|p| !p.is_null()).map(|p| Pair::from_value(Some(p))), dry: flag(Some(q), "dry", false), full: flag(Some(q), "full", false) }).collect(),
                failing: ve.and_then(|x| x.get("failing")).filter(|f| !f.is_null()).map(failing_from),
            },
            jitlegs: jitlegs_from(v.get("jitlegs")),
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
                LaneRec { name: text(o, "name"), path: text(o, "path"), branch: text(o, "branch"), cbranch: text(o, "cbranch"), created_ms: uint(o, "created_ms"), adopted: flag(o, "adopted", false), state: text(o, "state"), landed_ms: uint(o, "landed_ms"), keep: flag(o, "keep", false), waited: flag(o, "waited", false), ended_ms: uint(o, "ended_ms"), note: text(o, "note") }
            }).collect(),
            shows: arr(Some(v), "shows").iter().map(show_from).collect(),
            shows_next: uint(Some(v), "shows_next"),
            lives: arr(Some(v), "lives").iter().map(live_state_from).collect(),
            lives_next: uint(Some(v), "lives_next"),
            land_next: uint(Some(v), "land_next"),
            jobs: arr(Some(v), "jobs").iter().map(job_from).collect(),
            submissions: arr(Some(v), "submissions").iter().map(|r| (text(Some(r), "id"), text(Some(r), "result"))).collect(),
            jobs_next: uint(Some(v), "jobs_next"),
            batches_next: uint(Some(v), "batches_next"),
            snaps_next: uint(Some(v), "snaps_next"),
            land: arr(Some(v), "land").to_vec(),
            land_paused: text(Some(v), "land_paused"),
            merges: arr(Some(v), "merges").to_vec(),
            held: arr(Some(v), "held").iter().map(|h| (text(Some(h), "branch"), text(Some(h), "why"))).filter(|(b, _)| !b.is_empty()).collect(),
            prewarm: arr(Some(v), "prewarm").to_vec(),
        }
    }

    pub fn parse(text: &str) -> Option<Doc> {
        makepad_strict_json::parse_depth(text.as_bytes(), 16).ok().filter(|v| matches!(v, Value::Obj(_))).map(|v| Doc::from_value(&v))
    }

    /// One section's JSON (`verify`, `jitlegs`, `allapps`, `after`, `pushes`, `lanes`, `shows`, `lives`, `jobs`), or the whole document.
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
        // a retry comes within minutes: the newest receipts answer it (ids start with their time)
        while self.submissions.len() > JOBS_KEEP {
            self.submissions.pop_first();
        }
        cut(&mut self.pushes);
        if self.ci.runs.len() > KEEP {
            let excess = self.ci.runs.len() - KEEP;
            self.ci.runs.drain(..excess);
        }
        if self.jitlegs.runs.len() > 20 {
            self.jitlegs.runs.drain(..self.jitlegs.runs.len() - 20);
        }
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
        let ended_lives = self.lives.iter().filter(|x| !x.active()).count();
        if ended_lives > KEEP {
            let mut drop = ended_lives - KEEP;
            self.lives.retain(|x| {
                let old = drop > 0 && !x.active();
                if old {
                    drop -= 1;
                }
                !old
            });
        }
        let ended = self.shows.iter().filter(|x| !x.active()).count();
        if ended > KEEP {
            let mut drop = ended - KEEP;
            self.shows.retain(|x| {
                let old = drop > 0 && !x.active();
                if old {
                    drop -= 1;
                }
                !old
            });
        }
        let jobs_done = self.jobs.iter().filter(|j| j.done()).count();
        if jobs_done > JOBS_KEEP {
            let mut drop = jobs_done - JOBS_KEEP;
            let pending: std::collections::HashSet<String> = self.jobs.iter().filter(|j| (j.shadow || !j.live_sample.is_empty()) && !j.shadow_recorded).flat_map(|j| [j.id.clone(), j.shadow_job.clone()]).collect();
            self.jobs.retain(|j| {
                let old = drop > 0 && j.done() && !pending.contains(&j.id);
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
    obj(vec![("name", s(&l.name)), ("state", s(&l.state)), ("lease", s(&l.lease)), ("node", s(&l.node)), ("summary", s(&l.summary)), ("errors", strs(&l.errors)), ("log", s(&l.log)), ("run_n", int(l.run_n))])
}

fn leg_from(v: &Value) -> Leg {
    let v = Some(v);
    Leg { name: text(v, "name"), state: text(v, "state"), lease: text(v, "lease"), node: text(v, "node"), summary: text(v, "summary"), errors: strs_from(v, "errors"), log: text(v, "log"), run_n: uint(v, "run_n") }
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

fn failing_value(f: &Failing) -> Value {
    obj(vec![
        ("pair", f.pair.to_value()),
        ("legs", Value::Arr(f.legs.iter().map(leg_value).collect())),
        ("gated", Value::Arr(f.gated.iter().map(|(p, v)| obj(vec![("pair", p.to_value()), ("verdict", s(v))])).collect())),
        ("old", f.old.as_ref().map_or(Value::Null, |o| strs(o))),
    ])
}

fn failing_from(v: &Value) -> Failing {
    let o = Some(v);
    Failing { pair: Pair::from_value(v.get("pair")), legs: arr(o, "legs").iter().map(leg_from).collect(), gated: arr(o, "gated").iter().map(|g| (Pair::from_value(g.get("pair")), text(Some(g), "verdict"))).collect(), old: v.get("old").filter(|x| !x.is_null()).map(|_| strs_from(o, "old")) }
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
        ("asked", Value::Bool(r.asked)),
        ("tries", int(r.tries as u64)),
    ])
}

fn run_from(v: &Value) -> Run {
    let o = Some(v);
    Run { id: text(o, "id"), pair: Pair::from_value(v.get("pair")), started_ms: uint(o, "started_ms"), diff: text(o, "diff"), dry: flag(o, "dry", false), purpose: text(o, "purpose"), legs: arr(o, "legs").iter().map(leg_from).collect(), asked: flag(o, "asked", false), tries: uint(o, "tries") as u32 }
}

fn jitlegs_run_value(r: &JitLegsRun) -> Value {
    obj(vec![("pair", r.pair.to_value()), ("started_ms", int(r.started_ms)), ("ended_ms", int(r.ended_ms)), ("legs", Value::Arr(r.legs.iter().map(leg_value).collect()))])
}

fn jitlegs_run_from(v: &Value) -> JitLegsRun {
    let o = Some(v);
    JitLegsRun { pair: Pair::from_value(v.get("pair")), started_ms: uint(o, "started_ms"), ended_ms: uint(o, "ended_ms"), legs: arr(o, "legs").iter().map(leg_from).collect() }
}

fn jitlegs_value(j: &JitLegs) -> Value {
    obj(vec![("runs", Value::Arr(j.runs.iter().map(jitlegs_run_value).collect())), ("asked", Value::Bool(j.asked))])
}

fn jitlegs_from(v: Option<&Value>) -> JitLegs {
    JitLegs { runs: arr(v, "runs").iter().map(jitlegs_run_from).collect(), asked: flag(v, "asked", false) }
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
        ("waited", Value::Bool(l.waited)),
        ("ended_ms", int(l.ended_ms)),
        ("note", s(&l.note)),
    ])
}

fn panic_value(p: &Option<JobPanic>) -> Value {
    p.as_ref().map_or(Value::Null, |p| obj(vec![("id", s(&p.id)), ("pid", int(p.pid as u64)), ("message", s(&p.message)), ("thread", s(&p.thread)), ("location", s(&p.location)), ("at_ms", int(p.at_ms))]))
}

fn panic_from(v: Option<&Value>) -> Option<JobPanic> {
    let p = v.filter(|p| !p.is_null())?;
    Some(JobPanic { id: text(Some(p), "id"), pid: uint(Some(p), "pid") as u32, message: text(Some(p), "message"), thread: text(Some(p), "thread"), location: text(Some(p), "location"), at_ms: uint(Some(p), "at_ms") })
}

fn live_tests_value(t: &LiveTests) -> Value {
    obj(vec![("edit", s(&t.edit)), ("group", int(t.group)), ("ran", int(t.ran)), ("passed", int(t.passed)), ("failed", int(t.failed)), ("ignored", int(t.ignored)), ("skipped", int(t.skipped)), ("selected", s(&t.selected)), ("failures", strs(&t.failures)), ("ms", Value::F64(t.ms)), ("compile_ms", Value::F64(t.compile_ms)), ("restarted", Value::Bool(t.restarted))])
}

pub fn live_tests_from(v: &Value) -> Option<LiveTests> {
    let edit = v.get("edit").map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_json())).unwrap_or_default();
    // Initial runs and some restarts have no patch compile time.
    let compile_ms = match v.get("compile_ms") { None | Some(Value::Null) => 0.0, Some(v) => f64_of(v)? };
    Some(LiveTests { edit, group: v.get("group")?.as_u64()?, ran: v.get("ran")?.as_u64()?, passed: v.get("passed")?.as_u64()?, failed: v.get("failed")?.as_u64()?, ignored: uint(Some(v), "ignored"), skipped: uint(Some(v), "skipped"), selected: text(Some(v), "selected"), failures: strs_from(Some(v), "failures"), ms: v.get("ms").and_then(f64_of)?, compile_ms, restarted: flag(Some(v), "restarted", false) })
}

pub fn live_value(x: &Live) -> Value {
    let mut value = live_state_value(x);
    if let Some(patch) = &x.patch {
        if let Value::Obj(fields) = &mut value {
            if let Some((_, Value::Obj(fields))) = fields.iter_mut().find(|(k, _)| k == "patch") {
                fields.push(("output".into(), s(if patch.output.is_empty() { patch.output_ref.read().unwrap_or_else(|e| format!("live log: {e}")) } else { patch.output.clone() })));
                fields.push(("reason".into(), s(if patch.reason.is_empty() { patch.reason_ref.read().unwrap_or_else(|e| format!("live log: {e}")) } else { patch.reason.clone() })));
            }
        }
    }
    value
}

fn live_state_value(x: &Live) -> Value {
    let patch = x.patch.as_ref().map_or(Value::Null, |p| {
        obj(vec![("files", strs(&p.files)), ("edit", s(&p.edit)), ("group", p.group.map_or(Value::Null, int)), ("seq", int(p.seq)), ("cursor", s(&p.cursor)), ("edit_ms", int(p.edit_ms)), ("called_ms", int(p.called_ms)), ("shipped_ms", int(p.shipped_ms)), ("outcome", s(&p.outcome)), ("outcome_ms", int(p.outcome_ms)), ("frame_ms", int(p.frame_ms)), ("tests", p.tests.as_ref().map_or(Value::Null, live_tests_value)), ("verdict_ms", int(p.verdict_ms)),  ("log_start", int(p.log_start)), ("log_end", int(p.log_end)), ("output_ref", p.output_ref.to_value()), ("reason_ref", p.reason_ref.to_value()), ("retried", Value::Bool(p.retried)), ("jit_error", int(p.jit_error))])
    });
    obj(vec![
        ("id", s(&x.id)),
        ("run_n", int(x.run_n)),
        ("run_log_start", int(x.run_log_start)),
        ("sources", Value::Obj(x.sources.iter().map(|(k, v)| (k.clone(), s(v))).collect())),
        ("app", s(&x.app)),
        ("test", Value::Bool(x.test)),
        ("test_args", strs(&x.test_args)),
        ("env", Value::Arr(x.env.iter().map(|(k, v)| Value::Arr(vec![s(k), s(v)])).collect())),
        ("also", strs(&x.also)),
        ("lane", s(&x.lane)),
        ("src", s(&x.src)),
        ("cost", s(&x.cost)),
        ("est_min", x.est_min.map_or(Value::Null, Value::F64)),
        ("class", s(&x.class)),
        ("priority", s(&x.priority)),
        ("loader", s(&x.loader)),
        ("agent", s(&x.agent)),
        ("tree", s(&x.tree)),
        ("rev", s(&x.rev)),
        ("needs", s(&x.needs)),
        ("visible", Value::Bool(x.visible)),
        ("lease", s(&x.lease)),
        ("node", s(&x.node)),
        ("pid", int(x.pid as u64)),
        ("state", s(&x.state)),
        ("started_ms", int(x.started_ms)),
        ("touched_ms", int(x.touched_ms)),
        ("ended_ms", int(x.ended_ms)),
        ("patch", patch),
        ("panic", panic_value(&x.panic)),
        ("log", s(&x.log)),
        ("note", s(&x.note)),
    ])
}

pub fn live_from(v: &Value) -> Live {
    let mut live = live_state_from(v);
    if let Some(p) = &mut live.patch {
        p.output = text(v.get("patch"), "output");
        p.reason = text(v.get("patch"), "reason");
    }
    live
}

fn live_state_from(v: &Value) -> Live {
    let o = Some(v);
    let p = v.get("patch").filter(|p| !p.is_null());
    Live {
        id: text(o, "id"),
        run_n: uint(o, "run_n"),
        run_log_start: uint(o, "run_log_start"),
        sources: match v.get("sources") { Some(Value::Obj(fields)) => fields.iter().filter_map(|(k, v)| v.as_str().map(|v| (k.clone(), v.to_string()))).collect(), _ => Default::default() },
        app: text(o, "app"),
        test: flag(o, "test", false),
        test_args: strs_from(o, "test_args"),
        env: arr(o, "env").iter().filter_map(|pair| {
            let [k, v] = pair.as_arr()? else { return None };
            Some((k.as_str()?.to_string(), v.as_str()?.to_string()))
        }).collect(),
        also: strs_from(o, "also"),
        lane: text(o, "lane"),
        src: text(o, "src"),
        cost: text(o, "cost"),
        est_min: v.get("est_min").and_then(crate::lease::num),
        class: text(o, "class"),
        priority: text(o, "priority"),
        loader: text(o, "loader"),
        agent: text(o, "agent"),
        tree: text(o, "tree"),
        rev: text(o, "rev"),
        needs: text(o, "needs"),
        visible: flag(o, "visible", false),
        lease: text(o, "lease"),
        node: text(o, "node"),
        pid: uint(o, "pid") as u32,
        state: text(o, "state"),
        started_ms: uint(o, "started_ms"),
        touched_ms: uint(o, "touched_ms"),
        ended_ms: uint(o, "ended_ms"),
        patch: p.map(|p| LivePatch { files: strs_from(Some(p), "files"), edit: text(Some(p), "edit"), group: p.get("group").and_then(Value::as_u64), seq: uint(Some(p), "seq"), cursor: text(Some(p), "cursor"), edit_ms: uint(Some(p), "edit_ms"), called_ms: uint(Some(p), "called_ms"), shipped_ms: uint(Some(p), "shipped_ms"), outcome: text(Some(p), "outcome"), outcome_ms: uint(Some(p), "outcome_ms"), frame_ms: uint(Some(p), "frame_ms"), tests: p.get("tests").and_then(live_tests_from), verdict_ms: uint(Some(p), "verdict_ms"), log_start: uint(Some(p), "log_start"), log_end: uint(Some(p), "log_end"), output: String::new(), output_ref: TextRef::from_value(p.get("output_ref")), reason_ref: TextRef::from_value(p.get("reason_ref")), retried: flag(Some(p), "retried", false), jit_error: uint(Some(p), "jit_error"), reason: String::new() }),
        panic: panic_from(v.get("panic")),
        log: text(o, "log"),
        note: text(o, "note"),
    }
}

pub fn show_value(x: &Show) -> Value {
    obj(vec![
        ("id", s(&x.id)),
        ("pinned", Value::Bool(x.pinned)),
        ("launch_ms", int(x.launch_ms)),
        ("stages", s(&x.stages)),
        ("app", s(&x.app)),
        ("device", s(&x.device)),
        ("state", s(&x.state)),
        ("cond", x.cond.to_value()),
        ("pid", int(x.pid as u64)),
        ("sha", s(&x.sha)),
        ("added_ms", int(x.added_ms)),
        ("started_ms", int(x.started_ms)),
        ("ended_ms", int(x.ended_ms)),
        ("exit", x.exit.map_or(Value::Null, |e| Value::Int(e as i64))),
        ("panic", panic_value(&x.panic)),
        ("by", s(&x.by)),
        ("log", s(&x.log)),
        ("note", s(&x.note)),
    ])
}

pub fn show_from(v: &Value) -> Show {
    let o = Some(v);
    Show {
        id: text(o, "id"),
        pinned: flag(o, "pinned", false),
        launch_ms: uint(o, "launch_ms"),
        stages: text(o, "stages"),
        app: text(o, "app"),
        device: text(o, "device"),
        state: text(o, "state"),
        cond: Cond::from_value(v.get("cond")),
        pid: uint(o, "pid") as u32,
        sha: text(o, "sha"),
        added_ms: uint(o, "added_ms"),
        started_ms: uint(o, "started_ms"),
        ended_ms: uint(o, "ended_ms"),
        exit: v.get("exit").and_then(|e| e.as_i64()).map(|e| e as i32),
        panic: panic_from(v.get("panic")),
        by: text(o, "by"),
        log: text(o, "log"),
        note: text(o, "note"),
    }
}

pub fn job_value(j: &Job) -> Value {
    let panic = j.panic.as_ref().map_or(Value::Null, |p| obj(vec![("id", s(&p.id)), ("pid", int(p.pid as u64)), ("message", s(&p.message)), ("thread", s(&p.thread)), ("location", s(&p.location)), ("at_ms", int(p.at_ms))]));
    obj(vec![
        ("id", s(&j.id)),
        ("batch", s(&j.batch)),
        ("lane", s(&j.lane)),
        ("src", s(&j.src)),
        ("cost", s(&j.cost)),
        ("est_min", j.est_min.map_or(Value::Null, Value::F64)),
        ("cores", j.cores.map_or(Value::Null, Value::F64)),
        ("ram_gb", j.ram_gb.map_or(Value::Null, Value::F64)),
        ("whole", Value::Bool(j.whole)),
        ("class", s(&j.class)),
        ("priority", s(&j.priority)),
        ("loader", s(&j.loader)),
        ("agent", s(&j.agent)),
        ("kind", s(&j.kind)),
        ("needs", s(&j.needs)),
        ("node_pref", s(&j.node_pref)),
        ("only", Value::Bool(j.only)),
        ("prio", Value::Int(j.prio as i64)),
        ("rev", s(&j.rev)),
        ("repo_dir", s(&j.repo_dir)),
        ("snap", s(&j.snap)),
        ("snap_files", int(j.snap_files as u64)),
        ("run_n", int(j.run_n)),
        ("log_base", int(j.log_base)),
        ("attempts", int(j.attempts as u64)),
        ("granted_ms", int(j.granted_ms)),
        ("shipped_ms", int(j.shipped_ms)),
        ("first_out_ms", int(j.first_out_ms)),
        ("cmd", strs(&j.cmd)),
        ("cwd", s(&j.cwd)),
        ("holder", j.holder.map_or(Value::Null, |p| int(p as u64))),
        ("shadow", Value::Bool(j.shadow)),
        ("shadow_job", s(&j.shadow_job)),
        ("shadow_of", s(&j.shadow_of)),
        ("shadow_recorded", Value::Bool(j.shadow_recorded)),
        ("live_sample", s(&j.live_sample)),
        ("jit_error", s(&j.jit_error)),
        ("after", strs(&j.after)),
        ("state", s(&j.state)),
        ("submitted_ms", int(j.submitted_ms)),
        ("asked_ms", int(j.asked_ms)),
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
        ("waited_ms", int(j.waited_ms)),
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
        src: text(o, "src"),
        cost: text(o, "cost"),
        est_min: v.get("est_min").and_then(crate::lease::num),
        cores: v.get("cores").and_then(crate::lease::num),
        ram_gb: v.get("ram_gb").and_then(crate::lease::num),
        whole: flag(o, "whole", false),
        class: text(o, "class"),
        priority: text(o, "priority"),
        loader: text(o, "loader"),
        agent: text(o, "agent"),
        kind: text(o, "kind"),
        needs: text(o, "needs"),
        node_pref: text(o, "node_pref"),
        only: flag(o, "only", false),
        prio: v.get("prio").and_then(|x| x.as_i64()).unwrap_or(5) as i32,
        rev: text(o, "rev"),
        repo_dir: text(o, "repo_dir"),
        snap: text(o, "snap"),
        snap_files: uint(o, "snap_files") as u32,
        run_n: uint(o, "run_n"),
        log_base: uint(o, "log_base"),
        attempts: uint(o, "attempts") as u32,
        granted_ms: uint(o, "granted_ms"),
        shipped_ms: uint(o, "shipped_ms"),
        first_out_ms: uint(o, "first_out_ms"),
        cmd: strs_from(o, "cmd"),
        cwd: text(o, "cwd"),
        holder: v.get("holder").and_then(Value::as_u64).and_then(|p| u32::try_from(p).ok()),
        shadow: flag(o, "shadow", false),
        shadow_job: text(o, "shadow_job"),
        shadow_of: text(o, "shadow_of"),
        shadow_recorded: flag(o, "shadow_recorded", false),
        live_sample: text(o, "live_sample"),
        jit_error: text(o, "jit_error"),
        after: strs_from(o, "after"),
        state: text(o, "state"),
        submitted_ms: uint(o, "submitted_ms"),
        asked_ms: uint(o, "asked_ms"),
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
        waited_ms: uint(o, "waited_ms"),
        note: text(o, "note"),
    }
}

fn pushed_value(p: &Pushed) -> Value {
    obj(vec![("pair", p.pair.to_value()), ("commit", s(&p.commit)), ("origin_was", s(&p.origin_was)), ("at_ms", int(p.at_ms)), ("files", int(p.files as u64))])
}

/// The master's state: one document behind one lock, saved on every change.
pub struct Store {
    path: PathBuf,
    /// the machine its events name (the first open that knows it: the pre-warm board opens first
    /// and does not)
    machine: Mutex<String>,
    doc: Mutex<Versioned>,
    signals: Mutex<std::collections::HashMap<String, Vec<std::sync::Weak<Signal>>>>,
    /// where [`Store::event`] goes: the master's stream (set by the first [`Store::open`]; a
    /// [`Store::open_quiet`] module that writes its section publishes its own records)
    publish: Mutex<Option<Publish>>,
    /// woken on every change and every [`Store::kick`]
    changed: Condvar,
    generation: Mutex<u64>,
}

struct Versioned {
    doc: Doc,
    revision: u64,
    records: std::collections::BTreeMap<String, TextRef>,
    history_len: u64,
}

/// A subscription belongs only to the requested job/session ids. Subscribe before reading.
#[derive(Default)]
pub struct Signal {
    generation: Mutex<u64>,
    changed: Condvar,
}
impl Signal {
    pub fn generation(&self) -> u64 { *self.generation.lock().unwrap() }
    pub fn wait(&self, seen: u64, duration: Duration) {
        let g = self.generation.lock().unwrap();
        let _ = self.changed.wait_timeout_while(g, duration, |g| *g == seen);
    }
    fn notify(&self) {
        *self.generation.lock().unwrap() += 1;
        self.changed.notify_all();
    }
}

/// Replace a durable file. Callers serialize snapshot creation and this write with one lock.
pub(crate) fn durable_write(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let write = || -> std::io::Result<()> {
        let parent = path.parent().ok_or_else(|| std::io::Error::other("state path has no parent"))?;
        std::fs::create_dir_all(parent)?;
        let tmp = path.with_extension("new");
        let mut file = std::fs::File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(tmp, path)?;
        #[cfg(unix)]
        std::fs::File::open(parent)?.sync_all()?;
        Ok(())
    };
    write().map_err(|e| format!("persist {}: {e}", path.display()))
}

/// The state files of before the one document; [`Store::open`] moves them in once, then deletes them.
pub const OLD_FILES: [&str; 5] = ["land.jsonl", "land.paused", "lanes.state", "lanes.held", "prewarm.state"];

impl Store {
    /// `<home>/state.json`: only a missing file starts empty; corrupt or unreadable state fails. ONE store per
    /// home in the process: the jobs, the landing train, the merge bot and the pre-warm board all
    /// write their sections into the same document (two stores on one file would undo each
    /// other's writes). The first open moves the old per-module files in ([`OLD_FILES`]) and
    /// deletes them.
    pub fn open(home: &Path, machine: &str, publish: Publish) -> Result<Arc<Store>, String> {
        Self::open_with(home, machine, Some(publish))
    }

    /// The same store for a module that only keeps its section (the landing train, the merge
    /// bot, the pre-warm board): the stream stays the one the master's open gave.
    pub fn open_quiet(home: &Path, machine: &str) -> Result<Arc<Store>, String> {
        Self::open_with(home, machine, None)
    }

    fn open_with(home: &Path, machine: &str, publish: Option<Publish>) -> Result<Arc<Store>, String> {
        static OPEN: Mutex<Vec<(PathBuf, std::sync::Weak<Store>)>> = Mutex::new(Vec::new());
        let path = home.join("state.json");
        let mut open = OPEN.lock().unwrap();
        open.retain(|(_, w)| w.strong_count() > 0);
        if let Some(s) = open.iter().find(|(p, _)| *p == path).and_then(|(_, w)| w.upgrade()) {
            if !machine.is_empty() {
                let mut m = s.machine.lock().unwrap();
                if m.is_empty() {
                    *m = machine.to_string();
                }
            }
            if let Some(p) = publish {
                let mut mine = s.publish.lock().unwrap();
                if mine.is_none() {
                    *mine = Some(p);
                }
            }
            return Ok(s);
        }
        let mut revision = 0;
        let mut history_len = 0;
        let mut records = std::collections::BTreeMap::new();
        let doc = match std::fs::read_to_string(&path) {
            Ok(text) => {
                let value = makepad_strict_json::parse_depth(text.as_bytes(), 16).map_err(|e| format!("invalid workflow state {}; file kept: {e:?}", path.display()))?;
                if !matches!(value, Value::Obj(_)) { return Err(format!("invalid workflow state {}; expected object; file kept", path.display())); }
                revision = uint(Some(&value), "revision");
                history_len = uint(Some(&value), "history_len");
                Doc::from_disk(value, &mut records)?
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Doc::default(),
            Err(e) => return Err(format!("read workflow state {}: {e}; file kept", path.display())),
        };
        if history_len > 0 {
            let len = std::fs::metadata(home.join("history.jsonl")).map_err(|e| format!("workflow history: {e}; state kept"))?.len();
            if len < history_len { return Err("truncated workflow history; state kept".into()); }
        }
        let store = Arc::new(Store { path: path.clone(), machine: Mutex::new(machine.to_string()), doc: Mutex::new(Versioned { doc, revision, records, history_len }), signals: Mutex::new(Default::default()), publish: Mutex::new(publish), changed: Condvar::new(), generation: Mutex::new(0) });
        store.migrate(home)?;
        open.push((path, Arc::downgrade(&store)));
        Ok(store)
    }

    /// The one-time move of the old state files into the document; each goes once it is in.
    fn migrate(&self, home: &Path) -> Result<(), String> {
        let read = |f: &str| std::fs::read_to_string(home.join(f)).ok();
        let lines = |t: &str| -> Vec<Value> { t.lines().filter_map(|l| makepad_strict_json::parse_depth(l.as_bytes(), 16).ok()).collect() };
        let (land, paused, merges, held, prewarm) = (read(OLD_FILES[0]), read(OLD_FILES[1]), read(OLD_FILES[2]), read(OLD_FILES[3]), read(OLD_FILES[4]));
        if land.is_none() && paused.is_none() && merges.is_none() && held.is_none() && prewarm.is_none() {
            return Ok(());
        }
        self.update(|d| {
            if let Some(t) = &land {
                if d.land.is_empty() {
                    d.land = lines(t);
                }
            }
            if let Some(t) = &paused {
                if d.land_paused.is_empty() {
                    d.land_paused = t.trim().to_string();
                }
            }
            if let Some(t) = &merges {
                if d.merges.is_empty() {
                    d.merges = lines(t);
                }
            }
            if let Some(t) = &held {
                if d.held.is_empty() {
                    d.held = t.lines().filter_map(|l| {
                        let mut w = l.splitn(2, ' ');
                        let b = w.next().filter(|b| !b.is_empty())?;
                        Some((b.to_string(), w.next().unwrap_or("").trim().to_string()))
                    }).collect();
                }
            }
            if let Some(t) = &prewarm {
                if d.prewarm.is_empty() {
                    d.prewarm = lines(t);
                }
            }
        })?;
        // in the document (saved by the update): the old files go
        if self.path.exists() {
            for f in OLD_FILES {
                let _ = std::fs::remove_file(home.join(f));
                let _ = std::fs::remove_file(home.join(format!("{f}.new")));
            }
        }
        Ok(())
    }

    pub fn read<T>(&self, f: impl FnOnce(&Doc) -> T) -> T {
        f(&self.doc.lock().unwrap().doc)
    }

    /// The candidate is published only after persistence succeeds. Equality detects actual
    /// mutations without JSON serialization; only committed mutations advance the revision.
    pub fn update<T>(&self, f: impl FnOnce(&mut Doc) -> T) -> Result<T, String> {
        self.try_update(|d| Ok(f(d)))
    }

    pub fn try_update<T>(&self, f: impl FnOnce(&mut Doc) -> Result<T, String>) -> Result<T, String> {
        let mut state = self.doc.lock().unwrap();
        let mut next = state.doc.clone();
        let out = f(&mut next)?;
        if next == state.doc { return Ok(out); }
        next.spill(self.path.parent().unwrap())?;
        if next == state.doc { return Ok(out); }
        next.trim();
        let revision = state.revision + 1;
        let archived = archive_history(&mut next);
        let mut history_len = state.history_len;
        if !archived.is_empty() {
            std::fs::create_dir_all(self.path.parent().unwrap()).map_err(|e| e.to_string())?;
            let mut file = std::fs::OpenOptions::new().create(true).append(true)
                .open(self.path.with_file_name("history.jsonl")).map_err(|e| e.to_string())?;
            // Discard only an uncommitted tail from an interrupted attempt. Committed
            // history is append-only; state.json records its durable byte boundary.
            file.set_len(history_len).map_err(|e| e.to_string())?;
            for (section, value) in archived {
                writeln!(file, "{}", obj(vec![("revision", int(revision)), ("section", s(section)), ("record", value)]).to_json()).map_err(|e| e.to_string())?;
            }
            file.sync_all().map_err(|e| e.to_string())?;
            history_len = file.metadata().map_err(|e| e.to_string())?.len();
        }
        let (mut value, records) = next.disk_value(&state.doc, &state.records, self.path.parent().unwrap())?;
        if let Value::Obj(fields) = &mut value { fields.push(("revision".into(), int(revision))); fields.push(("history_len".into(), int(history_len))); }
        durable_write(&self.path, value.to_json().as_bytes())?;
        let mut signals = self.signals.lock().unwrap();
        signals.retain(|_, subscribers| { subscribers.retain(|w| w.strong_count() > 0); !subscribers.is_empty() });
        for (key, subscribers) in signals.iter() {
            let changed = if let Some(id) = key.strip_prefix("job:") {
                state.doc.jobs.iter().find(|j| j.id == id) != next.jobs.iter().find(|j| j.id == id)
            } else if let Some(id) = key.strip_prefix("live:") {
                state.doc.lives.iter().find(|j| j.id == id) != next.lives.iter().find(|j| j.id == id)
            } else { false };
            if changed { for signal in subscribers.iter().filter_map(std::sync::Weak::upgrade) { signal.notify(); } }
        }
        state.doc = next;
        state.revision = revision;
        state.records = records;
        state.history_len = history_len;
        drop(signals);
        drop(state);
        self.kick();
        Ok(out)
    }

    /// Historical lookups stream archived records only when requested, outside the document lock.
    pub fn history(&self, section: &str) -> Result<Vec<Value>, String> {
        use std::io::BufRead;
        let (history_len, current) = {
            let state = self.doc.lock().unwrap();
            (state.history_len, if section == "land" { state.doc.land.clone() } else { state.doc.merges.clone() })
        };
        let mut entries = std::collections::BTreeMap::new();
        let key = |v: &Value| if section == "land" { uint(Some(v), "id").to_string() } else { format!("{}/{}", text(Some(v), "repo"), text(Some(v), "branch")) };
        match std::fs::File::open(self.path.with_file_name("history.jsonl")) {
            Ok(file) => for line in std::io::BufReader::new(file.take(history_len)).lines() {
                let line = line.map_err(|e| e.to_string())?;
                let v = makepad_strict_json::parse_depth(line.as_bytes(), 16).map_err(|e| format!("invalid workflow history: {e:?}"))?;
                if v.get("section").and_then(Value::as_str) == Some(section) {
                    if let Some(record) = v.get("record") { entries.insert(key(record), record.clone()); }
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound && history_len == 0 => {},
            Err(e) => return Err(e.to_string()),
        }
        for v in &current { entries.insert(key(v), v.clone()); }
        let mut values: Vec<Value> = entries.into_values().collect();
        values.sort_by_key(|v| uint(Some(v), if section == "land" { "id" } else { "at_ms" }));
        Ok(values)
    }

    /// A session's process-local admission/ship state changed without a document mutation.
    pub fn notify(&self, kind: &str, id: &str) {
        if let Some(subscribers) = self.signals.lock().unwrap().get(&format!("{kind}:{id}")) {
            for signal in subscribers.iter().filter_map(std::sync::Weak::upgrade) { signal.notify(); }
        }
    }

    pub fn subscribe(&self, kind: &str, ids: &[&str]) -> Arc<Signal> {
        let signal = Arc::new(Signal::default());
        let mut signals = self.signals.lock().unwrap();
        signals.retain(|_, subscribers| { subscribers.retain(|w| w.strong_count() > 0); !subscribers.is_empty() });
        for id in ids { signals.entry(format!("{kind}:{id}")).or_default().push(Arc::downgrade(&signal)); }
        signal
    }

    /// A state change on the event stream: `kind` is the section (`verify`, `after`, ...), `line`
    /// the answer-first text, `detail` the changed entry.
    pub fn event(&self, kind: &str, sev: Sev, line: String, detail: Option<String>) {
        self.event_seq(kind, sev, line, detail);
    }

    /// Publishes an event and returns its stream sequence.
    pub fn event_seq(&self, kind: &str, sev: Sev, line: String, detail: Option<String>) -> u64 {
        let p = self.publish.lock().unwrap().clone();
        p.map_or(0, |p| {
            let machine = self.machine.lock().unwrap().clone();
            p(hub_record(&machine, "", 0, kind, sev, line, detail.unwrap_or_default()))
        })
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

    struct Scratch(PathBuf);
    impl Scratch {
        fn new(label: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!("devhub-store-{label}-{}-{n}", std::process::id()));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn store(&self) -> Arc<Store> { Store::open_quiet(&self.0, "").unwrap() }
    }
    impl Drop for Scratch {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[test]
    fn no_op_update_writes_nothing() {
        let dir = Scratch::new("noop");
        let store = dir.store();
        store.update(|_| ()).unwrap();
        assert!(!dir.0.join("state.json").exists(), "fresh read/no-op creates no state file");
        store.update(|d| d.jobs_next = 1).unwrap();
        let old = std::fs::metadata(dir.0.join("state.json")).unwrap().modified().unwrap();
        store.update(|d| d.jobs_next = 1).unwrap();
        assert_eq!(store.doc.lock().unwrap().revision, 1, "equal assignment is not a revision");
        assert_eq!(old, std::fs::metadata(dir.0.join("state.json")).unwrap().modified().unwrap(), "no rewrite");
        assert!(!dir.0.join("state.new").exists(), "no temporary write");
    }

    #[test]
    fn failed_write_is_reported_without_publishing_the_mutation() {
        let dir = Scratch::new("failed");
        let store = dir.store();
        std::fs::create_dir(dir.0.join("state.new")).unwrap();
        let signal = store.subscribe("job", &["J1"]);
        let error = store.update(|d| d.jobs.push(Job { id: "J1".into(), ..Default::default() })).unwrap_err();
        assert!(error.contains("persist"));
        assert!(store.read(|d| d.jobs.is_empty()), "failed work is not accepted in memory");
        assert_eq!(signal.generation(), 0, "failed writes never wake waiters");
        assert_eq!(store.doc.lock().unwrap().revision, 0);
    }

    #[test]
    fn corrupt_or_unreadable_state_refuses_open_and_is_preserved() {
        let dir = Scratch::new("corrupt");
        let path = dir.0.join("state.json");
        for text in ["{broken", "[]", "null"] {
            std::fs::write(&path, text).unwrap();
            assert!(Store::open_quiet(&dir.0, "").is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text, "corrupt file kept");
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(Store::open_quiet(&dir.0, "").is_err(), "unreadable existing file is not a fresh install");
        assert!(path.is_dir());
    }

    #[test]
    fn large_live_output_and_finished_records_are_file_slices() {
        let dir = Scratch::new("output");
        let store = dir.store();
        let output = "failure output\n".repeat(100_000);
        store.update(|d| {
            d.lives.push(Live { id: "W1".into(), state: "running".into(), patch: Some(LivePatch { output: output.clone(), reason: "compile failed".into(), ..Default::default() }), ..Default::default() });
            d.jobs.push(Job { id: "J1".into(), state: "failed".into(), tail: vec![output.clone()], ..Default::default() });
            d.ci.runs.push(CiRun { id: "C1".into(), shards: vec![CiShard { errors: vec![output.clone()], ..Default::default() }], ..Default::default() });
        }).unwrap();
        let text = std::fs::read_to_string(dir.0.join("state.json")).unwrap();
        assert!(text.len() < 10_000, "large diagnostics never enter state.json: {}", text.len());
        assert!(!text.contains("failure output"));
        let live = store.read(|d| d.lives[0].clone());
        let patch = live.patch.as_ref().unwrap();
        assert!(patch.output.is_empty() && patch.reason.is_empty());
        assert_eq!(patch.output_ref.read().unwrap(), output);
        assert_eq!(patch.reason_ref.read().unwrap(), "compile failed");
        assert!(Path::new(&patch.output_ref.path).starts_with(dir.0.join("live-logs")));
        assert!(live_value(&live).to_json().contains("failure output"), "wire response retains output");
        drop(store);
        let reopened = dir.store();
        assert_eq!(reopened.read(|d| d.jobs[0].tail[0].clone()), output);
        assert_eq!(reopened.read(|d| d.ci.runs[0].shards[0].errors[0].clone()), output);
        assert_eq!(reopened.read(|d| d.lives[0].patch.as_ref().unwrap().output_ref.clone()).read().unwrap(), output);
    }

    #[test]
    fn concurrent_persists_cannot_regress_the_file() {
        let dir = Scratch::new("ordered");
        let store = dir.store();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let handles: Vec<_> = (0..2).map(|_| {
            let (store, barrier) = (store.clone(), barrier.clone());
            std::thread::spawn(move || { barrier.wait(); store.update(|d| d.jobs_next += 1).unwrap(); })
        }).collect();
        barrier.wait();
        for h in handles { h.join().unwrap(); }
        let value = makepad_strict_json::parse(std::fs::read(dir.0.join("state.json")).unwrap().as_slice()).unwrap();
        assert_eq!(value.get("jobs_next").and_then(Value::as_u64), Some(2));
        assert_eq!(value.get("revision").and_then(Value::as_u64), Some(2));
    }

    #[test]
    fn waiter_only_wakes_for_its_jobs_or_session() {
        let dir = Scratch::new("wait");
        let store = dir.store();
        let jobs = store.subscribe("job", &["J1", "J2"]);
        let session = store.subscribe("live", &["W1"]);
        let unrelated = store.subscribe("job", &["J3"]);
        store.update(|d| d.jobs.push(Job { id: "J3".into(), ..Default::default() })).unwrap();
        assert_eq!(jobs.generation(), 0, "unrelated job does not signal waiter");
        assert_eq!(session.generation(), 0, "job never signals live waiter");
        assert_eq!(unrelated.generation(), 1);
        store.update(|d| d.jobs.push(Job { id: "J2".into(), ..Default::default() })).unwrap();
        assert_eq!(jobs.generation(), 1, "any subscribed job signals waiter");
        jobs.wait(0, Duration::from_secs(10)); // already signalled: no lost wake before wait
        store.update(|d| d.lives.push(Live { id: "W1".into(), ..Default::default() })).unwrap();
        assert_eq!(session.generation(), 1);
        assert_eq!(jobs.generation(), 1);
    }

    #[test]
    fn archived_history_retains_attribution() {
        let dir = Scratch::new("history");
        let store = dir.store();
        store.update(|d| {
            for id in 1..=KEEP + 2 {
                d.land.push(obj(vec![("id", int(id as u64)), ("state", s("landed")), ("pre", s("parent")), ("landed", s("child"))]));
                d.merges.push(obj(vec![("branch", s(format!("lane-{id}"))), ("result", s("landed"))]));
            }
        }).unwrap();
        assert_eq!(store.read(|d| (d.land.len(), d.merges.len())), (KEEP, KEEP));
        assert_eq!(store.history("land").unwrap().len(), KEEP + 2);
        assert_eq!(store.history("merges").unwrap().len(), KEEP + 2);
        assert_eq!(store.history("land").unwrap()[0].get("pre").and_then(Value::as_str), Some("parent"));
        std::fs::remove_file(dir.0.join("history.jsonl")).unwrap();
        assert!(store.history("land").is_err(), "missing committed history is reported");
    }

    fn pair(c: char) -> Pair {
        Pair { work: c.to_string().repeat(40), main: "c".repeat(40) }
    }

    #[test]
    fn doc_round_trip() {
        let leg = Leg { name: "full-a".into(), state: "fail".into(), lease: "L9".into(), node: "107".into(), summary: "NODEGATE-FAIL: fuzz".into(), errors: vec!["fuzz: seed 7 disagrees".into()], log: "/x/full-a.log".into(), run_n: 4 };
        let doc = Doc {
            devhub_shadow: false,
            ci: Default::default(),
            jitlegs: JitLegs { runs: vec![JitLegsRun { pair: pair('b'), started_ms: 3, ended_ms: 6, legs: vec![leg.clone()] }], asked: true },
            verify: Verify {
                on: false,
                verified: vec![Tip { pair: pair('a'), at_ms: 5, diff: "all".into() }],
                failures: vec![
                    Failure { pair: pair('b'), at_ms: 6, blame: Some(Blame { repo: "work".into(), sha: "d".repeat(40), subject: "x \"y\"".into(), good: pair('a'), bad: pair('b') }), unresolved: String::new(), legs: vec![leg.clone()] },
                    Failure { pair: pair('e'), at_ms: 7, blame: None, unresolved: "no-verified-first-parent-baseline".into(), legs: vec![] },
                ],
                run: Some(Run { id: "V3".into(), pair: pair('f'), started_ms: 8, diff: "affected".into(), dry: true, purpose: "bisect 1/3".into(), legs: vec![leg], asked: true, tries: 1 }),
                last_full_ms: 9,
                asked: vec![Asked { pair: None, dry: false, full: true }, Asked { pair: Some(pair('a')), dry: true, full: false }],
                failing: Some(Failing { pair: pair('b'), legs: Vec::new(), gated: vec![(pair('a'), "ok".into())], old: Some(vec!["jit:01".into()]) }),
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
            lanes: vec![LaneRec { name: "x".into(), path: "/w/x".into(), branch: "rc-x".into(), cbranch: "rc-x".into(), created_ms: 1, adopted: true, state: "live".into(), landed_ms: 4, keep: true, waited: true, ended_ms: 0, note: "n".into() }],
            shows: vec![Show { id: "V1".into(), app: "makepad-fab".into(), device: "mac".into(), state: "paused".into(), cond: Cond::Landed { branch: "rc-x".into(), repo: "makepad".into(), sha: String::new() }, pid: 4, sha: "s".into(), exit: Some(0), panic: Some(JobPanic { id: "P2".into(), ..Default::default() }), ..Default::default() }],
            shows_next: 1,
            lives: vec![Live { id: "W1".into(), app: "makepad-app-calculator".into(), state: "running".into(), visible: true, pid: 7, patch: Some(LivePatch { files: vec!["a.rs".into()], edit_ms: 1, shipped_ms: 2, outcome: "patched".into(), outcome_ms: 3, frame_ms: 4, reason: String::new(), ..Default::default() }), ..Default::default() }],
            lives_next: 1,
            land_next: 1,
            submissions: std::collections::BTreeMap::from([("request-one".into(), "{\"jobs\":[\"J1\"]}".into())]),
            jobs_next: 2,
            batches_next: 1,
            snaps_next: 3,
            land: vec![makepad_strict_json::parse(br#"{"id":4,"branch":"rc-x","state":"queued"}"#).unwrap()],
            land_paused: "user review".into(),
            merges: vec![makepad_strict_json::parse(br#"{"branch":"rc-x","repo":"makepad","result":"merged"}"#).unwrap()],
            held: vec![("rc-h".into(), "a viewer rebuild".into())],
            prewarm: vec![makepad_strict_json::parse(br#"{"node":"101","sev":"info","line":"prewarm: done","at_ms":5}"#).unwrap()],
        };
        let text = doc.to_value().to_json();
        assert_eq!(Doc::parse(&text), Some(doc.clone()));
        assert!(doc.section(Some("verify")).unwrap().starts_with("{\"on\":false"));
        assert_eq!(makepad_strict_json::parse(doc.section(Some("jitlegs")).unwrap().as_bytes()).unwrap(), jitlegs_value(&doc.jitlegs));
        assert_eq!(Doc::parse("{}"), Some(Doc::default()));
        assert!(Doc::default().verify.on && Doc::default().allapps.on);
    }

    #[test]
    fn ci_round_trip_and_timings_average() {
        let shard = CiShard { id: "smoke:apps/calc".into(), state: "fail".into(), node: "106".into(), lease: "L2".into(), secs: 12.5, summary: "exit 1".into(), errors: vec!["error: x".into()], log: "/l".into(), sig: "ab".into(), step: 1, run_n: 2, steps_ok: vec![0], failed: vec!["x".into()], kept: "k".into(), failed_on: "101".into(), failed_os: "linux".into(), failed_why: "exit 1".into(), asked_ms: 5, granted_ms: 6, spans: vec![CiSpan { node: "106".into(), asked_ms: 1, granted_ms: 2, ended_ms: 3 }], lost: vec![("104".into(), 1)] };
        let run = CiRun { lock_digest: "lock".into(), checkout: String::new(), id: "R1".into(), pair: pair('a'), dry: true, started_ms: 1, ended_ms: 2, verdict: "fail".into(), shards: vec![shard], critical: "106 ends at 12 s".into(), fleet: "wall 12 s".into(), boxes: vec!["106 12 s busy, 1 shards".into()], rotation: 2, plan_timings: vec![("test:x".into(), 7.0)], sweep: true, stopping: true };
        let mut ci = Ci { timings: vec![("jit:01".into(), 40.0)], run: None, sweep_run: Some(run.clone()), runs: vec![run].into(), rotation: 3 };
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
        let store = Store::open(&dir, "mac", Arc::new(move |r: Record| { s2.lock().unwrap().push(r.kind.clone()); 0 })).expect("workflow state must open before services start");
        store.update(|d| {
            for i in 0..KEEP + 5 {
                d.verify.verified.push(Tip { pair: pair('a'), at_ms: i as u64, diff: "affected".into() });
            }
        }).unwrap();
        store.event("verify", Sev::Info, "verify OK".into(), None);
        let again = Store::open(&dir, "mac", Arc::new(|_| 0)).expect("workflow state must open before services start");
        assert_eq!(again.read(|d| (d.verify.verified.len(), d.verify.verified[0].at_ms)), (KEEP, 5));
        assert_eq!(*seen.lock().unwrap(), vec!["verify".to_string()]);
        let _ = std::fs::remove_file(dir.join("state.json"));
        let _ = std::fs::remove_dir(&dir);
    }

    /// The old per-module files move into the one document once, then go.
    #[test]
    fn old_state_files_move_in_once() {
        let home = std::env::temp_dir().join(format!("devhub-state-migrate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("land.jsonl"), "{\"id\":1,\"branch\":\"rc-a\",\"state\":\"landed\"}\n{\"id\":2,\"branch\":\"rc-b\",\"state\":\"queued\"}\n").unwrap();
        std::fs::write(home.join("land.paused"), "user review\n").unwrap();
        std::fs::write(home.join("lanes.state"), "{\"branch\":\"rc-b\",\"repo\":\"makepad\",\"result\":\"landed\"}\n").unwrap();
        std::fs::write(home.join("lanes.held"), "rc-h needs a rebuild\n").unwrap();
        std::fs::write(home.join("prewarm.state"), "{\"node\":\"101\",\"sev\":\"info\",\"line\":\"prewarm: done\",\"at_ms\":5}\n").unwrap();
        {
            let st = Store::open_quiet(&home, "mac").expect("workflow state must open before services start");
            let (land, paused, merges, held, prewarm) = st.read(|d| (d.land.len(), d.land_paused.clone(), d.merges.len(), d.held.clone(), d.prewarm.len()));
            assert_eq!((land, paused.as_str(), merges, prewarm), (2, "user review", 1, 1));
            assert_eq!(held, vec![("rc-h".to_string(), "needs a rebuild".to_string())]);
            // one store per home: a second open is the same document
            let again = Store::open(&home, "mac", Arc::new(|_| 0)).expect("workflow state must open before services start");
            assert!(Arc::ptr_eq(&st, &again));
        }
        for f in OLD_FILES {
            assert!(!home.join(f).exists(), "{f} is still there");
        }
        // a later start reads it from the document alone
        let st = Store::open_quiet(&home, "mac").expect("workflow state must open before services start");
        assert_eq!(st.read(|d| d.land.len()), 2);
        drop(st);
        let _ = std::fs::remove_dir_all(&home);
    }
}
