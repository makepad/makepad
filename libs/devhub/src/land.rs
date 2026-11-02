//! The landing train (`makepad devhub land`), run by the master. Hand-ins merge into work and
//! commercial main in queue order, then fast-forward the user's checkout. Conflicts go back to
//! the lane; dirty overlaps wait. Holds and pause are explicit controls.
//! Checks run after landing in timer buckets (see `bucket.rs`); they never block the train.
//! Every transition is a `land` record on the master's feed. The queue and pause live in state.json.

use crate::proto::{hub_record, Record, Sev};
use makepad_strict_json::{obj, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Publish = Arc<dyn Fn(Record) + Send + Sync>;

/// How often the train looks for moved branches without a hand-in.
const IDLE: Duration = Duration::from_secs(15);
/// An entry waiting for the user's checkout retries this often.
const WAITING_RETRY_MS: u64 = 60_000;

/// Error lines kept per failed entry.
const MAX_DETAIL: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    Queued,
    Failed,
    Conflict,
    Waiting,
    Landed,
}

impl St {
    pub fn as_str(self) -> &'static str {
        match self {
            St::Queued => "queued",
            St::Failed => "failed",
            St::Conflict => "conflict",
            St::Waiting => "waiting",
            St::Landed => "landed",
        }
    }
    fn parse(s: &str) -> St {
        [St::Queued, St::Failed, St::Conflict, St::Waiting, St::Landed].into_iter().find(|x| x.as_str() == s).unwrap_or(St::Queued)
    }
}

/// One hand-in.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: u64,
    /// the makepad branch, or `-` (a commercial-only hand-in)
    pub branch: String,
    /// the commercial worktree and branch ("" when none)
    pub cwt: String,
    pub cbranch: String,
    pub state: St,
    pub held: String,
    /// the branch shas of the last attempt (a failure or conflict waits until one moves)
    pub ms: String,
    pub cs: String,
    /// `work <sha> main <sha>` once landed
    pub landed: String,
    /// `work <sha> main <sha>` just before the fast-forward; empty when unknown
    pub pre: String,
    pub detail: Vec<String>,
    pub agent: String,
    /// Only ci.toml changes.
    pub proof: bool,
    pub asked_ms: u64,
    pub changed_ms: u64,
}

impl Entry {
    pub fn label(&self) -> String {
        match (self.branch.as_str(), self.cbranch.is_empty()) {
            (b, true) => b.to_string(),
            ("-", false) => format!("-+{}", self.cbranch),
            (b, false) => format!("{b}+{}", self.cbranch),
        }
    }
    fn names(&self, b: &str) -> bool {
        self.branch == b || self.cbranch == b || self.label() == b
    }
    pub fn to_json(&self) -> String {
        let s = |x: &str| Value::Str(x.to_string());
        let mut out = String::new();
        obj(vec![
            ("id", Value::Int(self.id as i64)),
            ("branch", s(&self.branch)),
            ("cwt", s(&self.cwt)),
            ("cbranch", s(&self.cbranch)),
            ("state", s(self.state.as_str())),
            ("held", s(&self.held)),
            ("ms", s(&self.ms)),
            ("cs", s(&self.cs)),
            ("landed", s(&self.landed)),
            ("pre", s(&self.pre)),
            ("detail", Value::Arr(self.detail.iter().map(|d| s(d)).collect())),
            ("agent", s(&self.agent)),
            ("proof", Value::Bool(self.proof)),
            ("asked_ms", Value::Int(self.asked_ms as i64)),
            ("changed_ms", Value::Int(self.changed_ms as i64)),
        ])
        .write_into(&mut out);
        out
    }
    pub fn parse(line: &str) -> Option<Entry> {
        let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
        Some(Entry {
            id: n("id"),
            branch: s("branch"),
            cwt: s("cwt"),
            cbranch: s("cbranch"),
            state: St::parse(&s("state")),
            held: s("held"),
            ms: s("ms"),
            cs: s("cs"),
            landed: s("landed"),
            pre: s("pre"),
            detail: v.get("detail").and_then(|d| d.as_arr()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            agent: s("agent"),
            proof: v.get("proof").and_then(Value::as_bool).unwrap_or(false),
            asked_ms: n("asked_ms"),
            changed_ms: n("changed_ms"),
        })
    }
}

/// `git -C <dir> <args>`: (success, stdout), hooks off, never a prompt.
fn git(dir: &Path, args: &[&str]) -> (bool, String) {
    let o = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["-c", "core.hooksPath=/dev/null"])
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null())
        .stderr(Stdio::piped())
        .output();
    match o {
        Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stdout).trim().to_string()),
        Err(_) => (false, String::new()),
    }
}

fn rev(dir: &Path, what: &str) -> Option<String> {
    let (ok, out) = git(dir, &["rev-parse", "-q", "--verify", &format!("{what}^{{commit}}")]);
    (ok && out.len() == 40).then_some(out)
}

fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    git(dir, &["merge-base", "--is-ancestor", a, b]).0
}

/// Where a branch left a retired work chain (`refs/heads/work-old-*`, kept when work moves onto
/// main's squashed tip): its merge base with that chain, when `tip` doesn't hold it. Such a branch
/// carries its own changes over as one commit on the tip, so the old history stays out of work.
pub fn retired_base(dir: &Path, branch: &str, tip: &str) -> Option<String> {
    let (_, refs) = git(dir, &["for-each-ref", "--format=%(refname)", "refs/heads/work-old-*"]);
    refs.lines().find_map(|retired| {
        let (ok, mb) = git(dir, &["merge-base", branch, retired]);
        (ok && mb.len() == 40 && !is_ancestor(dir, &mb, tip)).then_some(mb)
    })
}

struct Merge {
    base: String,
    sha: String,
    tree: String,
    retired: bool,
    unchanged: bool,
}

fn proof_file(path: &str) -> bool {
    path.rsplit('/').next() == Some("ci.toml")
}

fn prepare_merge(dir: &Path, base: &str, sha: &str) -> Result<Merge, Vec<String>> {
    let unchanged = sha.is_empty() || is_ancestor(dir, sha, base);
    if unchanged {
        return Ok(Merge { base: base.into(), sha: sha.into(), tree: base.into(), retired: false, unchanged });
    }
    let retired = retired_base(dir, sha, base);
    let mut args = vec!["merge-tree", "--write-tree", "--name-only", "--no-messages"];
    if let Some(mb) = &retired {
        args.extend(["--merge-base", mb]);
    }
    args.extend([base, sha]);
    let (ok, out) = git(dir, &args);
    let mut lines = out.lines();
    let tree = lines.next().unwrap_or("").to_string();
    let conflicts: Vec<String> = lines.filter(|l| !l.is_empty()).map(str::to_string).collect();
    if tree.len() != 40 || !ok {
        return Err(if conflicts.is_empty() { vec![format!("merge-tree failed ({base:.10} + {sha:.10})")] } else { conflicts });
    }
    Ok(Merge { base: base.into(), sha: sha.into(), tree, retired: retired.is_some(), unchanged })
}

fn finish_merge(dir: &Path, m: &Merge, msg: &str) -> Result<String, Vec<String>> {
    if m.unchanged { return Ok(m.base.clone()); }
    let mut args = vec!["commit-tree", &m.tree, "-p", &m.base];
    if !m.retired { args.extend(["-p", &m.sha]); }
    args.extend(["-m", msg]);
    let (ok, c) = git(dir, &args);
    if ok && c.len() == 40 { Ok(c) } else { Err(vec!["commit-tree failed".into()]) }
}

/// Merge drivers resolve proof content before merge-tree returns.
fn merge_pair(root: &Path, w: &str, ms: &str, m: &str, cs: &str, branch: &str, cbranch: &str) -> Result<(String, String), Vec<String>> {
    let com = root.join("apps/commercial");
    let work = prepare_merge(root, w, ms)?;
    let main = prepare_merge(&com, m, cs)?;
    Ok((finish_merge(root, &work, &format!("Merge branch '{branch}' into work (devhub land)"))?, finish_merge(&com, &main, &format!("Merge branch '{cbranch}' into main (devhub land)"))?))
}

/// A standalone merge uses the repository's configured merge drivers.
pub fn merge_onto(dir: &Path, base: &str, sha: &str, msg: &str) -> Result<String, Vec<String>> {
    let merge = prepare_merge(dir, base, sha)?;
    finish_merge(dir, &merge, msg)
}

fn only_proofs(dir: &Path, from: &str, to: &str) -> bool {
    let (ok, files) = git(dir, &["diff", "--name-only", from, to]);
    ok && files.lines().all(proof_file)
}

/// The user's uncommitted paths a fast-forward of `dir` to `target` would touch.
fn dirty_overlap(dir: &Path, target: &str) -> Vec<String> {
    let (_, changed) = git(dir, &["diff", "--name-only", "HEAD", target]);
    let (_, status) = git(dir, &["status", "--porcelain", "--untracked-files=all"]);
    let dirty: Vec<&str> = status.lines().filter_map(|l| l.get(3..)).map(|p| p.rsplit(" -> ").next().unwrap_or(p)).collect();
    changed.lines().filter(|c| dirty.contains(c)).take(10).map(str::to_string).collect()
}

/// The failure lines of a gate log: everything but progress and OK lines (compiler errors, present log tails).
pub fn failure_lines(log: &str) -> Vec<String> {
    let keep = |l: &&str| {
        let t = l.trim();
        !(t.is_empty()
            || l.starts_with("STEP ")
            || l.starts_with("jit: ")
            || l.starts_with("why ")
            || l.starts_with("GATE ")
            || l.starts_with("GATEUNIT ")
            || l.starts_with("GATEPROOF ")
            || l.starts_with("NODEGATE-")
            || (l.starts_with('L') && l.split_whitespace().next().is_some_and(|w| w[1..].bytes().all(|b| b.is_ascii_digit())))
            || t.ends_with(": 0 errors")
            || t.ends_with(" present: 1"))
    };
    let v: Vec<String> = log.lines().filter(keep).map(|l| crate::proto::clean(l, 300)).collect();
    v[v.len().saturating_sub(MAX_DETAIL)..].to_vec()
}

pub struct Train {
    machine: String,
    home: PathBuf,
    repo: PathBuf,
    /// devhub's own words as a child ([`crate::cli::own_argv`]; a test: its stand-in)
    own: Vec<String>,
    publish: Publish,
    st: Mutex<Vec<Entry>>,
    clock: fn() -> u64,
    /// why the train is paused (empty: it runs)
    paused: Mutex<String>,
    /// a hand-in came: look now
    kicked: AtomicBool,
}

/// The open entries by state, for a glance (the menu bar).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// queued or waiting for the next round
    pub queued: usize,
    /// failed or conflicting, not landed since
    pub failed: usize,
    pub held: usize,
}

impl Train {
    /// The master's one state document (its `land` and `land_paused` sections).
    fn store(&self) -> Arc<crate::state::Store> {
        crate::state::Store::open_quiet(&self.home, &self.machine)
    }
    fn load(&self) {
        let (list, paused) = self.store().read(|d| (d.land.iter().filter_map(|v| Entry::parse(&v.to_json())).collect::<Vec<_>>(), d.land_paused.clone()));
        *self.st.lock().unwrap() = list;
        *self.paused.lock().unwrap() = paused;
    }

    /// Why the train is paused; None while it runs.
    pub fn paused(&self) -> Option<String> {
        Some(self.paused.lock().unwrap().clone()).filter(|p| !p.is_empty())
    }

    /// Pauses the train (`Some(reason)`) or resumes it (`None`); kept across master restarts.
    pub fn set_paused(&self, reason: Option<&str>) {
        let reason = reason.map(|r| if r.trim().is_empty() { "paused" } else { r.trim() });
        let was = std::mem::replace(&mut *self.paused.lock().unwrap(), reason.unwrap_or("").to_string());
        self.store().update(|d| d.land_paused = reason.unwrap_or("").to_string());
        if was.is_empty() != reason.is_none() {
            self.event(Sev::Info, reason.map_or("land resumed".into(), |r| format!("land paused ({r})")), &[]);
        }
    }

    /// The open entries by state.
    pub fn counts(&self) -> Counts {
        let mut c = Counts::default();
        for e in self.st.lock().unwrap().iter().filter(|e| e.state != St::Landed) {
            if !e.held.is_empty() {
                c.held += 1;
            }
            match e.state {
                St::Failed | St::Conflict => c.failed += 1,
                _ if e.held.is_empty() => c.queued += 1,
                _ => {}
            }
        }
        c
    }
    fn save(&self, list: &[Entry]) {
        let vals: Vec<makepad_strict_json::Value> = list.iter().filter_map(|e| makepad_strict_json::parse_depth(e.to_json().as_bytes(), 16).ok()).collect();
        let next = list.iter().map(|e| e.id).max().unwrap_or(0) + 1;
        self.store().update(|d| { d.land = vals; d.land_next = d.land_next.max(next); });
    }
    fn event(&self, sev: Sev, line: String, detail: &[String]) {
        (self.publish)(hub_record(&self.machine, "", 0, "land", sev, line, detail.join("\n")));
    }
    /// Changes one entry (by id), saves, and publishes `line` when given.
    fn set(&self, id: u64, f: impl FnOnce(&mut Entry), ev: Option<(Sev, String)>) {
        let now = (self.clock)();
        let mut list = self.st.lock().unwrap();
        let Some(e) = list.iter_mut().find(|e| e.id == id) else { return };
        f(e);
        e.changed_ms = now;
        let detail = e.detail.clone();
        self.save(&list);
        drop(list);
        if let Some((sev, line)) = ev {
            self.event(sev, line, if sev == Sev::Info { &[] } else { &detail });
        }
    }

    /// One control request.
    pub fn serve(&self, line: &str, out: &mut Vec<String>) {
        let line = line.strip_prefix("land").map_or(line, str::trim_start);
        let w: Vec<&str> = line.split_whitespace().collect();
        let err = |out: &mut Vec<String>, e: &str| out.push(format!("{{\"err\":{}}}", makepad_remote::json_str(e)));
        match w.as_slice() {
            ["add", rest @ ..] => match self.add(rest) {
                Ok(l) => out.push(l),
                Err(e) => err(out, &e),
            },
            ["status", rest @ ..] => {
                let list = self.st.lock().unwrap().clone();
                let now = (self.clock)();
                for e in list.iter().filter(|e| match rest.first() {
                    Some(&"all") => true,
                    Some(b) => e.names(b),
                    // what is open, and what landed in the last hour
                    None => e.state != St::Landed || now.saturating_sub(e.changed_ms) < 3_600_000,
                }) {
                    out.push(e.to_json());
                }
            }
            [verb @ ("hold" | "unhold" | "retry" | "drop"), b, reason @ ..] => {
                let mut list = self.st.lock().unwrap();
                // `#<id>` names one entry; a branch or label names its newest open entry
                let by_id = b.strip_prefix('#').and_then(|n| n.parse::<u64>().ok());
                let Some(i) = list.iter().rposition(|e| e.state != St::Landed && by_id.map_or_else(|| e.names(b), |id| e.id == id)) else {
                    drop(list);
                    return err(out, &format!("no open entry {b}"));
                };
                match *verb {
                    "hold" => list[i].held = if reason.is_empty() { "held".into() } else { reason.join(" ") },
                    "unhold" => list[i].held.clear(),
                    "retry" => {
                        list[i].state = St::Queued;
                        list[i].ms.clear();
                        list[i].cs.clear();
                    }
                    _ => {
                        list.remove(i);
                    }
                }
                self.save(&list);
                drop(list);
                self.event(Sev::Info, format!("land {verb} {b}"), &[]);
                out.push(format!("{{\"ok\":1,\"{verb}\":{}}}", makepad_remote::json_str(b)));
            }
            ["pause", reason @ ..] => {
                self.set_paused(Some(&reason.join(" ")));
                out.push(format!("{{\"paused\":{}}}", makepad_remote::json_str(&self.paused().unwrap_or_default())));
            }
            ["resume"] => {
                self.set_paused(None);
                out.push("{\"paused\":\"\"}".into());
            }
            ["paused"] => out.push(format!("{{\"paused\":{}}}", makepad_remote::json_str(&self.paused().unwrap_or_default()))),
            ["import", landq, failed, held] => match self.import(Path::new(landq), Path::new(failed), Path::new(held)) {
                Ok(n) => out.push(format!("{{\"ok\":1,\"imported\":{n}}}")),
                Err(e) => err(out, &e),
            },
            _ => err(out, "land add <branch|-> [<commercial worktree> <commercial branch>] | status [all|<branch>] | hold <b> [reason] | unhold <b> | retry <b> | drop <b> | import <landq> <failed> <held> | pause [reason] | resume | paused"),
        }
    }

    fn add(&self, w: &[&str]) -> Result<String, String> {
        let proof = w.contains(&"proof");
        let agent = w.iter().find_map(|x| x.strip_prefix("agent=")).unwrap_or("").to_string();
        let w: Vec<&str> = w.iter().copied().filter(|x| !x.starts_with("agent=") && *x != "proof").collect();
        let (mut branch, mut cwt, mut cb) = match w.as_slice() {
            [b] => (*b, "", ""),
            [b, wt, cb] => (*b, *wt, *cb),
            _ => return Err("add <branch|-> [<commercial worktree> <commercial branch>]".into()),
        };
        // resolve the lane pair before checking branches or looking up the queue entry
        let lanes = self.store().read(|d| d.lanes.clone());
        let lane = match w.as_slice() {
            [b] if *b != "-" => lanes.iter().rev().find(|l| l.branch == *b || l.name == *b),
            ["-", _, cb] => lanes.iter().rev().find(|l| l.cbranch == *cb),
            _ => None,
        };
        let lane_cwt;
        let mut included = "";
        if let Some(lane) = lane {
            let commercial = self.repo.join("apps/commercial");
            let wt = Path::new(&lane.path).join("apps/commercial");
            lane_cwt = if wt.is_dir() { wt } else { commercial.clone() }.to_string_lossy().into_owned();
            let makepad_only = branch != "-";
            if makepad_only { branch = &lane.branch; }
            let (repo, target, other, repo_name) = if makepad_only {
                (&commercial, "main", lane.cbranch.as_str(), "commercial")
            } else {
                (&self.repo, "work", lane.branch.as_str(), "makepad")
            };
            // (a half whose branch is gone has nothing left to land)
            if !other.is_empty() && other != "-" && repo.join(".git").exists() && rev(repo, &format!("refs/heads/{other}")).is_some() {
                let pair_cwt = if makepad_only { lane_cwt.as_str() } else { cwt };
                let pair_cb = if makepad_only { lane.cbranch.as_str() } else { cb };
                let refuse = || format!("lane {} has commits on {other} in {repo_name} too; land both: makepad devhub land {} {pair_cwt} {pair_cb}", lane.name, lane.branch);
                let (ok, count) = git(repo, &["rev-list", "--count", &format!("refs/heads/{target}..refs/heads/{other}")]);
                let count = count.parse::<u64>().ok().filter(|_| ok).ok_or_else(&refuse)?;
                if count > 0 {
                    if !Path::new(pair_cwt).join(".git").exists() || rev(Path::new(pair_cwt), &format!("refs/heads/{pair_cb}")).is_none() {
                        return Err(refuse());
                    }
                    if makepad_only {
                        cwt = &lane_cwt;
                        cb = &lane.cbranch;
                    } else {
                        branch = &lane.branch;
                    }
                    included = other;
                }
            }
        }
        if branch != "-" && rev(&self.repo, &format!("refs/heads/{branch}")).is_none() {
            return Err(format!("no makepad branch {branch}"));
        }
        if !cb.is_empty() && rev(Path::new(cwt), &format!("refs/heads/{cb}")).is_none() {
            return Err(format!("no commercial branch {cb} in {cwt}"));
        }
        if branch == "-" && cb.is_empty() {
            return Err("nothing to land".into());
        }
        let now = (self.clock)();
        let mut list = self.st.lock().unwrap();
        let open = list.iter().position(|e| e.state != St::Landed && e.branch == branch && e.cbranch == cb);
        let id = match open {
            // a hand-in again: the same entry, queued again (its place in the queue kept)
            Some(i) => {
                let e = &mut list[i];
                e.cwt = cwt.to_string();
                e.proof = proof;
                e.state = St::Queued;
                e.detail.clear();
                e.changed_ms = now;
                e.id
            }
            None => {
                // (a counter in the state document: numbers of trimmed or lost entries are not reused)
                let id = self.store().read(|d| d.land_next).max(list.iter().map(|e| e.id).max().unwrap_or(0) + 1);
                list.push(Entry { id, branch: branch.into(), cwt: cwt.into(), cbranch: cb.into(), state: St::Queued, held: String::new(), ms: String::new(), cs: String::new(), landed: String::new(), pre: String::new(), detail: Vec::new(), agent, proof, asked_ms: now, changed_ms: now });
                id
            }
        };
        let pos = list.iter().filter(|e| e.state != St::Landed && e.id <= id).count();
        self.save(&list);
        let label = list.iter().find(|e| e.id == id).map(Entry::label).unwrap_or_default();
        drop(list);
        self.event(Sev::Info, format!("land queued {label} (#{id}, {pos} open)"), &[]);
        self.kicked.store(true, Ordering::Release);
        let included = if included.is_empty() { String::new() } else { format!(",\"included\":{}", makepad_remote::json_str(included)) };
        Ok(format!("{{\"ok\":1,\"id\":{id},\"pos\":{pos},\"label\":{}{included}}}", makepad_remote::json_str(&label)))
    }

    /// One-time import of the old train's files: landq lines, failed shas (they stay failed until their branch moves),
    /// HELD lines (`<branch> HELD: <reason>`).
    fn import(&self, landq: &Path, failed: &Path, held: &Path) -> Result<usize, String> {
        let q = std::fs::read_to_string(landq).map_err(|e| format!("{}: {e}", landq.display()))?;
        let failed = std::fs::read_to_string(failed).unwrap_or_default();
        let held = std::fs::read_to_string(held).unwrap_or_default();
        let mut n = 0;
        for l in q.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let w: Vec<&str> = l.split_whitespace().collect();
            if self.add(&w).is_err() {
                continue;
            }
            n += 1;
            let (b, cb) = (w[0], w.get(2).copied().unwrap_or(""));
            let ms = if b == "-" { None } else { rev(&self.repo, b) };
            let cs = w.get(1).and_then(|wt| rev(Path::new(wt), cb));
            let bad = |sha: &Option<String>| sha.as_deref().and_then(|s| failed.lines().find(|f| f.starts_with(s)).map(str::to_string));
            let mut list = self.st.lock().unwrap();
            if let Some(e) = list.iter_mut().rev().find(|e| e.branch == b && e.cbranch == cb && e.state != St::Landed) {
                if let Some(f) = bad(&ms).or_else(|| bad(&cs)) {
                    e.state = if f.contains(" conflict ") { St::Conflict } else { St::Failed };
                    e.ms = ms.clone().unwrap_or_default();
                    e.cs = cs.clone().unwrap_or_default();
                    e.detail = vec![format!("imported from landbatch.failed: {f}")];
                }
                if let Some(h) = held.lines().find(|h| h.split_whitespace().next() == Some(b)) {
                    e.held = h.split_once("HELD:").map_or("held", |x| x.1.trim()).to_string();
                }
            }
            self.save(&list);
        }
        Ok(n)
    }

    /// Lands ready entries one at a time, oldest first.
    fn start_ready(&self) -> bool {
        let r = &self.repo;
        let c = r.join("apps/commercial");
        let mut list: Vec<Entry> = self.st.lock().unwrap().iter().filter(|e| e.state != St::Landed).cloned().collect();
        list.sort_by_key(|e| (e.asked_ms, e.id));
        let mut landed = false;
        for queued in &list {
            if self.paused().is_some() { break; }
            let Some(e) = self.st.lock().unwrap().iter().find(|e| e.id == queued.id).cloned() else { continue };
            if e.state == St::Landed || !e.held.is_empty() { continue; }
            if e.state == St::Waiting && (self.clock)().saturating_sub(e.changed_ms) < WAITING_RETRY_MS { continue; }
            let (Some(w0), Some(m0)) = (rev(r, "refs/heads/work"), rev(&c, "refs/heads/main")) else { break };
            let ms = if e.branch == "-" { Some(String::new()) } else { rev(r, &format!("refs/heads/{}", e.branch)) };
            let cs = if e.cbranch.is_empty() { Some(String::new()) } else { rev(Path::new(&e.cwt), &format!("refs/heads/{}", e.cbranch)) };
            let removed = ms.is_none() || cs.is_none();
            let ms = ms.or_else(|| (!e.ms.is_empty() && is_ancestor(r, &e.ms, &w0)).then(|| e.ms.clone()));
            let cs = cs.or_else(|| (!e.cs.is_empty() && is_ancestor(&c, &e.cs, &m0)).then(|| e.cs.clone()));
            let (Some(ms), Some(cs)) = (ms, cs) else {
                self.set(e.id, |e| {
                    e.state = St::Failed;
                    e.detail = vec!["a branch of this entry is gone".into()];
                }, Some((Sev::Warn, format!("land failed {}: a branch is gone", e.label()))));
                continue;
            };
            if (ms.is_empty() || is_ancestor(r, &ms, &w0)) && (cs.is_empty() || is_ancestor(&c, &cs, &m0)) {
                let why = if removed { "already in work/main; branch removed" } else { "already in work/main" };
                self.set(e.id, |e| {
                    e.state = St::Landed;
                    e.landed = format!("work {w0} main {m0}");
                    e.detail.clear();
                }, Some((Sev::Info, format!("land landed {} ({why})", e.label()))));
                continue;
            }
            if matches!(e.state, St::Failed | St::Conflict) && e.ms == ms && e.cs == cs { continue; }
            self.set(e.id, |e| {
                e.ms = ms.clone();
                e.cs = cs.clone();
                e.detail.clear();
            }, None);
            let label = e.label();
            let (cm, cc) = match merge_pair(r, &w0, &ms, &m0, &cs, &e.branch, &e.cbranch) {
                Ok(pair) => pair,
                Err(f) => {
                    self.set(e.id, |e| {
                        e.state = St::Conflict;
                        e.detail = f;
                    }, Some((Sev::Warn, format!("land conflict {label} with work {w0:.10} / main {m0:.10}"))));
                    continue;
                }
            };
            if e.proof && (!only_proofs(r, &w0, &cm) || !only_proofs(&c, &m0, &cc)) {
                self.set(e.id, |e| {
                    e.state = St::Failed;
                    e.detail = vec!["proof entry touches files other than ci.toml".into()];
                }, Some((Sev::Err, format!("land refused proof entry {label}: non-proof files"))));
                continue;
            }
            landed |= self.land_one(e.id, &ms, &cs, &cm, &cc, &w0, &m0);
        }
        if landed { self.sync_now(); }
        landed
    }

    /// LIVEDEPLOY: the WM nodes get the new tips now.
    fn sync_now(&self) {
        // reaped on a thread of its own: a dropped Child stays a zombie of the master
        if let Ok(mut child) = Command::new(&self.own[0]).args(&self.own[1..]).args(["sync", "now"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            let _ = std::thread::Builder::new().name("devhub-sync-now".into()).spawn(move || { let _ = child.wait(); });
        }
    }

    /// Fast-forwards work and main, re-merging if the tips moved; true when it landed.
    fn land_one(&self, id: u64, ms: &str, cs: &str, cm: &str, cc: &str, w0: &str, m0: &str) -> bool {
        let r = &self.repo;
        let c = r.join("apps/commercial");
        let lock = r.join(".git/land-ff.lock");
        while std::fs::create_dir(&lock).is_err() {
            std::thread::sleep(Duration::from_millis(100));
        }
        let unlock = || {
            let _ = std::fs::remove_dir(&lock);
        };
        let label = self.st.lock().unwrap().iter().find(|e| e.id == id).map(Entry::label).unwrap_or_default();
        let (branch, cbranch) = self.st.lock().unwrap().iter().find(|e| e.id == id).map(|e| (e.branch.clone(), e.cbranch.clone())).unwrap_or_default();
        let wait = |why: String| {
            unlock();
            self.set(id, |e| {
                e.state = St::Waiting;
                e.detail = vec![why.clone()];
            }, Some((Sev::Info, format!("land waiting {label}: {why}"))));
            false
        };
        // Recheck controls after acquiring the fast-forward lock.
        let held = self.st.lock().unwrap().iter().find(|e| e.id == id).map(|e| e.held.clone()).unwrap_or_default();
        if !held.is_empty() {
            unlock();
            self.set(id, |e| e.state = St::Queued, Some((Sev::Info, format!("land {label} is held ({held})"))));
            return false;
        }
        if self.paused().is_some() || !self.st.lock().unwrap().iter().any(|e| e.id == id) {
            unlock();
            return false;
        }
        let on = |dir: &Path, want: &str| git(dir, &["branch", "--show-current"]).1 == want;
        if !on(r, "work") || !on(&c, "main") {
            return wait("the user checkout is not on work / main".into());
        }
        let (Some(w), Some(m)) = (rev(r, "refs/heads/work"), rev(&c, "refs/heads/main")) else { return wait("no work/main".into()) };
        let (tm, tc) = if w == w0 && m == m0 { (cm.to_string(), cc.to_string()) } else {
            match merge_pair(r, &w, ms, &m, cs, &branch, &cbranch) {
                Ok(pair) => pair,
                Err(f) => {
                    unlock();
                    self.set(id, |e| {
                        e.state = St::Conflict;
                        e.detail = f;
                    }, Some((Sev::Warn, format!("land conflict {label} with work {w:.10} / main {m:.10}"))));
                    return false;
                }
            }
        };
        let proof = self.st.lock().unwrap().iter().find(|e| e.id == id).is_some_and(|e| e.proof);
        if proof && (!only_proofs(r, &w, &tm) || !only_proofs(&c, &m, &tc)) {
            return wait("proof entry's new merge touches files other than ci.toml".into());
        }
        let mut over = dirty_overlap(r, &tm);
        over.extend(dirty_overlap(&c, &tc));
        if !over.is_empty() {
            return wait(format!("the user's uncommitted files the ff would touch: {}", over.join(" ")));
        }
        if tm != w && !git(r, &["merge", "-q", "--ff-only", &tm]).0 {
            return wait("ff of work failed (nothing landed)".into());
        }
        if tc != m && !git(&c, &["merge", "-q", "--ff-only", &tc]).0 {
            std::thread::sleep(Duration::from_secs(5));
            if !git(&c, &["merge", "-q", "--ff-only", &tc]).0 {
                unlock();
                let why = format!("HALF-LANDED: work is at {tm:.10} but commercial main did not ff to {tc:.10} (git -C {} merge --ff-only {tc})", c.display());
                self.set(id, |e| {
                    e.state = St::Failed;
                    e.detail = vec![why.clone()];
                }, Some((Sev::Err, format!("land {label}: {why}"))));
                return false;
            }
        }
        self.set(id, |e| {
            e.state = St::Landed;
            e.pre = format!("work {w} main {m}");
            e.landed = format!("work {tm} main {tc}");
            e.detail.clear();
        }, Some((Sev::Info, format!("land landed {label}: work {tm:.10} main {tc:.10}"))));
        unlock();
        if proof {
            for (repo, branch, pin, tip) in [(r, &branch, ms, &tm), (&c, &cbranch, cs, &tc)] {
                if branch.starts_with("ci-proof-") && rev(repo, branch).as_deref() == Some(pin) && is_ancestor(repo, pin, tip) {
                    if !git(repo, &["branch", "-d", branch]).0 {
                        self.event(Sev::Warn, format!("land: could not remove merged proof branch {branch}"), &[]);
                    }
                }
            }
        }
        true
    }

    fn run(self: Arc<Self>) {
        // A restarted master retries waiting entries and releases its old fast-forward lock.
        let _ = std::fs::remove_dir(self.repo.join(".git/land-ff.lock"));
        {
            let mut list = self.st.lock().unwrap();
            for e in list.iter_mut().filter(|e| e.state == St::Waiting) {
                e.state = St::Queued;
            }
            self.save(&list);
        }
        let mut looked = std::time::Instant::now() - IDLE;
        loop {
            if self.kicked.swap(false, Ordering::AcqRel) || looked.elapsed() >= IDLE {
                looked = std::time::Instant::now();
                if self.paused().is_none() {
                    self.start_ready();
                }
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }
}

/// The master's train: loads the queue, starts the round thread, registers the `land` control service.
pub fn register(ep: &makepad_remote::Endpoint, machine: &str, publish: Publish) -> Arc<Train> {
    let home = crate::proto::home();
    let t = Arc::new(Train {
        machine: machine.to_string(),
        home,
        repo: crate::lease::wt::default_repo(),
        own: crate::cli::own_argv(&crate::lease::wt::default_repo()),
        publish,
        st: Mutex::new(Vec::new()),
        clock: crate::proto::wall_ms,
        paused: Mutex::new(String::new()),
        kicked: AtomicBool::new(false),
    });
    t.load();
    let t2 = t.clone();
    ep.register("land", 1, makepad_remote::lines::service("land", Arc::new(move |line, out| t2.serve(line, out))));
    let t3 = t.clone();
    let _ = std::thread::Builder::new().name("devhub-land".into()).spawn(move || t3.run());
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_branch_off_a_retired_chain_lands_without_its_history() {
        let d = std::env::temp_dir().join(format!("devhub-retired-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let g = |args: &[&str]| {
            let (ok, out) = git(&d, args);
            assert!(ok, "{args:?}: {out}");
            out
        };
        g(&["init", "-q", "-b", "work"]);
        g(&["config", "user.email", "t@example.com"]);
        g(&["config", "user.name", "t"]);
        std::fs::write(d.join("a"), "a\n").unwrap();
        g(&["add", "a"]);
        g(&["commit", "-qm", "old chain"]);
        let old = g(&["rev-parse", "HEAD"]);
        g(&["checkout", "-qb", "lane"]);
        std::fs::write(d.join("b"), "b\n").unwrap();
        g(&["add", "b"]);
        g(&["commit", "-qm", "lane"]);
        let lane = g(&["rev-parse", "HEAD"]);
        let tree = g(&["rev-parse", &format!("{old}^{{tree}}")]);
        let squashed = g(&["commit-tree", &tree, "-m", "squashed"]);
        g(&["update-ref", "refs/heads/work-old-1", &old]);
        let landed = merge_onto(&d, &squashed, &lane, "land").unwrap();
        assert_eq!(g(&["rev-parse", &format!("{landed}^")]), squashed);
        assert!(!is_ancestor(&d, &old, &landed));
        assert!(g(&["ls-tree", "--name-only", &landed]).contains('b'));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn entries_round_trip_and_label() {
        let e = Entry { id: 3, branch: "rc-x".into(), cwt: "/w/x/apps/commercial".into(), cbranch: "rc-x".into(), state: St::Failed, held: String::new(), ms: "a".repeat(40), cs: String::new(), landed: String::new(), pre: format!("work {} main {}", "a".repeat(40), "b".repeat(40)), detail: vec!["stage: 1 errors".into(), "src/a.rs:3:4: \"quoted\"".into()], agent: "x".into(), proof: true, asked_ms: 1, changed_ms: 2 };
        assert_eq!(Entry::parse(&e.to_json()).unwrap(), e);
        assert_eq!(e.label(), "rc-x+rc-x");
        assert!(e.names("rc-x") && !e.names("rc-y"));
    }

    #[test]
    fn failure_lines_keep_errors_and_drop_progress() {
        let log = "L7095 gate@106: granted, setting up\nSTEP tree 0 s [1-2]\nmakepad-stage: 0 errors\nmakepad-fab: 2 errors\napps/commercial/fab/src/a.rs:3:4: cannot find value `x`\nmakepad-app-calculator present: 1\nmakepad-scope present: 0\nmakepad-scope: no present within 30 s; its last log lines:\n  panicked at x\nNODEGATE-FAIL: makepad-fab\nL7095 gate@106 (lane landq1) released; 106 cleaned up";
        assert_eq!(failure_lines(log), vec!["makepad-fab: 2 errors", "apps/commercial/fab/src/a.rs:3:4: cannot find value `x`", "makepad-scope present: 0", "makepad-scope: no present within 30 s; its last log lines:", "  panicked at x"]);
    }

    /// merge_onto: a clean merge is a two-parent commit on the base; a conflict names its files.
    #[test]
    fn merge_onto_merges_or_names_conflicts() {
        let d = std::env::temp_dir().join(format!("devhub-land-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let g = |args: &[&str]| assert!(git(&d, args).0, "git {args:?}");
        g(&["init", "-q", "-b", "work"]);
        g(&["config", "user.email", "t@t"]);
        g(&["config", "user.name", "t"]);
        std::fs::write(d.join("a"), "1\n").unwrap();
        g(&["add", "a"]);
        g(&["commit", "-qm", "base"]);
        g(&["checkout", "-qb", "x"]);
        std::fs::write(d.join("b"), "x\n").unwrap();
        g(&["add", "b"]);
        g(&["commit", "-qm", "x"]);
        g(&["checkout", "-qb", "y", "work"]);
        std::fs::write(d.join("a"), "y\n").unwrap();
        g(&["commit", "-qam", "y"]);
        g(&["checkout", "-q", "work"]);
        std::fs::write(d.join("a"), "w\n").unwrap();
        g(&["commit", "-qam", "w"]);
        let w = rev(&d, "work").unwrap();
        let m = merge_onto(&d, &w, &rev(&d, "x").unwrap(), "m").unwrap();
        assert_eq!(git(&d, &["rev-parse", &format!("{m}^1")]).1, w);
        assert!(is_ancestor(&d, &rev(&d, "x").unwrap(), &m));
        assert_eq!(merge_onto(&d, &w, &rev(&d, "y").unwrap(), "m"), Err(vec!["a".to_string()]));
        let _ = std::fs::remove_dir_all(&d);
    }

    fn train(home: &Path, repo: &Path, exe: &Path, events: Arc<Mutex<Vec<String>>>) -> Arc<Train> {
        Arc::new(Train {
            machine: "mac".into(),
            home: home.to_path_buf(),
            repo: repo.to_path_buf(),
            own: vec![exe.display().to_string()],
            publish: Arc::new(move |r: Record| events.lock().unwrap().push(r.line.clone())),
            st: Mutex::new(Vec::new()),
            clock: crate::proto::wall_ms,
            paused: Mutex::new(String::new()),
            kicked: AtomicBool::new(false),
        })
    }

    /// Hand-ins, a conflict and a hold; both checkouts move in queue order.
    #[cfg(unix)]
    #[test]
    fn a_round_lands_entries_in_order() {
        let d = std::env::temp_dir().join(format!("devhub-land-round-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (r, home) = (d.join("repo"), d.join("home"));
        let c = r.join("apps/commercial");
        std::fs::create_dir_all(&c).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let g = |dir: &Path, args: &[&str]| assert!(git(dir, args).0, "git {args:?} in {}", dir.display());
        for (dir, main) in [(&r, "work"), (&c, "main")] {
            g(dir, &["init", "-q", "-b", main]);
            g(dir, &["config", "user.email", "t@t"]);
            g(dir, &["config", "user.name", "t"]);
        }
        std::fs::write(r.join(".gitignore"), "apps/commercial\n").unwrap();
        std::fs::write(r.join("a"), "1\n").unwrap();
        g(&r, &["add", "."]);
        g(&r, &["commit", "-qm", "base"]);
        std::fs::write(c.join("s"), "1\n").unwrap();
        g(&c, &["add", "."]);
        g(&c, &["commit", "-qm", "cbase"]);
        // rc-x: a makepad file; rc-y: makepad + its commercial half; rc-z: conflicts with work
        let branch = |dir: &Path, b: &str, file: &str, text: &str, from: &str| {
            g(dir, &["checkout", "-qb", b, from]);
            std::fs::write(dir.join(file), text).unwrap();
            g(dir, &["add", "."]);
            g(dir, &["commit", "-qm", b]);
        };
        branch(&r, "rc-x", "x", "x\n", "work");
        branch(&r, "rc-y", "y", "y\n", "work");
        branch(&c, "rc-y", "sy", "y\n", "main");
        branch(&r, "rc-z", "a", "z\n", "work");
        branch(&r, "rc-h", "h", "h\n", "work");
        branch(&r, "rc-bad", "bad", "bad\n", "work");
        g(&r, &["checkout", "-q", "work"]);
        g(&c, &["checkout", "-q", "main"]);
        std::fs::write(r.join("a"), "w\n").unwrap();
        g(&r, &["commit", "-qam", "work moves"]);
        let exe = Path::new("/usr/bin/true");
        let events = Arc::new(Mutex::new(Vec::new()));
        let t = train(&home, &r, exe, events.clone());
        let ask = |line: &str| {
            let mut out = Vec::new();
            t.serve(line, &mut out);
            out
        };
        assert!(ask("add rc-nope")[0].contains("err"));
        assert!(ask("add rc-x agent=lane-x")[0].contains("\"id\":1"));
        assert!(ask(&format!("add rc-y {} rc-y", c.display()))[0].contains("\"id\":2"));
        assert!(ask("add rc-z")[0].contains("\"id\":3"));
        assert!(ask("add rc-h")[0].contains("\"id\":4"));
        assert!(ask("hold rc-h waits for the user")[0].contains("ok"));
        assert!(ask("add rc-bad")[0].contains("\"id\":5"));
        assert!(t.start_ready());
        let st = |b: &str| Entry::parse(&ask(&format!("status {b}"))[0]).unwrap();
        assert_eq!(st("rc-x").state, St::Landed, "{:?}", st("rc-x"));
        assert_eq!(st("rc-y").state, St::Landed, "{:?}", st("rc-y"));
        assert_eq!((st("rc-z").state, st("rc-z").detail.clone()), (St::Conflict, vec!["a".to_string()]));
        assert_eq!((st("rc-h").state, st("rc-h").held.as_str()), (St::Queued, "waits for the user"));
        assert_eq!(st("rc-bad").state, St::Landed, "{:?}", st("rc-bad"));
        // work has both makepad halves, main the commercial half; the user's checkouts moved with them
        let work = rev(&r, "refs/heads/work").unwrap();
        assert!(is_ancestor(&r, &rev(&r, "rc-x").unwrap(), &work) && is_ancestor(&r, &rev(&r, "rc-y").unwrap(), &work));
        assert!(is_ancestor(&c, &rev(&c, "rc-y").unwrap(), &rev(&c, "refs/heads/main").unwrap()));
        assert!(r.join("x").exists() && r.join("y").exists() && c.join("sy").exists());
        // nothing changed: the conflict waits for its branch to move, the held entry for its unhold
        assert!(!t.start_ready());
        // the queue survives a restart
        let t2 = train(&home, &r, exe, events.clone());
        t2.load();
        assert_eq!(t2.st.lock().unwrap().len(), 5);
        // numbers are never reused: the newest entry gone (dropped or trimmed), the next is still new
        t2.st.lock().unwrap().retain(|e| e.id != 5);
        t2.save(&t2.st.lock().unwrap().clone());
        let t3 = train(&home, &r, exe, events.clone());
        t3.load();
        assert_eq!(t3.st.lock().unwrap().iter().map(|e| e.id).max(), Some(4));
        assert!(t3.store().read(|d| d.land_next) >= 6, "the counter outlives the entry: the next number is 6, not 5");
        assert_eq!(t.counts(), Counts { queued: 0, failed: 1, held: 1 });
        // a pause holds the train across a restart; resume lets it run again
        assert_eq!(ask("pause user review"), vec!["{\"paused\":\"user review\"}".to_string()]);
        let t3 = train(&home, &r, exe, events.clone());
        t3.load();
        assert_eq!(t3.paused().as_deref(), Some("user review"));
        assert_eq!(ask("resume"), vec!["{\"paused\":\"\"}".to_string()]);
        assert_eq!(t.paused(), None);
        assert!(!home.join("land.paused").exists() && !home.join("land.jsonl").exists());
        let ev = events.lock().unwrap().join("\n");
        for want in ["land queued rc-x", "land landed rc-x", "land landed rc-y+rc-y", "land conflict rc-z", "land paused (user review)", "land resumed"] {
            assert!(ev.contains(want), "no `{want}` in\n{ev}");
        }
        assert!(ask("drop rc-z")[0].contains("ok"));
        assert_eq!(ask("status all").len(), 4);
        let _ = std::fs::remove_dir_all(&d);
    }
}
