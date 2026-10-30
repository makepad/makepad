//! The landing train (`makepad devhub land`), run by the master. Lanes hand in a branch pair (a makepad branch or
//! `-`, plus an optional commercial worktree and branch); the master keeps the queue in `land.jsonl` (its state dir)
//! and runs rounds: every pending entry gets its own candidate on the CURRENT work and commercial main tips (git
//! merge-tree + commit-tree: plumbing only, nothing checked out) and its own quick gate lease (land_gate.sh quick on a
//! linux x64 node's warm tree, one lane per entry, so the scheduler spreads them over the nodes), all at once. The
//! passing entries then fast-forward work and main one after another in queue order (makepad half first: the pair
//! rule), each re-merged onto tips the round already moved (a clean merge, not re-gated: land + fix forward); one that
//! no longer merges waits for the next round. A failed or conflicting entry keeps its error lines (compiler errors, a
//! present's log tail, the conflicting files) in its status and is not gated again until one of its branches moves.
//! Every transition is a `land` record on the master's feed (`devhub stream`), so lanes subscribe instead of polling.
//!
//! Control service `land`: `add <branch|-> [<commercial worktree> <commercial branch>] [agent=<a>]`, `status [all|<branch>]`,
//! `hold <branch> [reason]`, `unhold <branch>`, `retry <branch>`, `drop <branch>`, `import <landq> <failed> <held>`.

use crate::proto::{hub_record, Record, Sev};
use makepad_strict_json::{obj, Value};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub type Publish = Arc<dyn Fn(Record) + Send + Sync>;

/// The user's checkout: work, and its nested commercial main.
pub const REPO: &str = crate::lease::wt::DEFAULT_REPO;
/// Where the quick gates run: linux x64 nodes with their dev switch on (the gate's warm tree is Linux-only).
pub const GATE_NEEDS: &str = "arch=x64,os=linux";
/// How often the train looks for work between rounds.
const IDLE: Duration = Duration::from_secs(15);
/// Error lines kept per failed entry.
const MAX_DETAIL: usize = 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum St {
    Queued,
    Gating,
    Passed,
    Failed,
    Conflict,
    Waiting,
    Landed,
}

impl St {
    pub fn as_str(self) -> &'static str {
        match self {
            St::Queued => "queued",
            St::Gating => "gating",
            St::Passed => "passed",
            St::Failed => "failed",
            St::Conflict => "conflict",
            St::Waiting => "waiting",
            St::Landed => "landed",
        }
    }
    fn parse(s: &str) -> St {
        [St::Queued, St::Gating, St::Passed, St::Failed, St::Conflict, St::Waiting, St::Landed].into_iter().find(|x| x.as_str() == s).unwrap_or(St::Queued)
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
    pub node: String,
    /// the pinned shas of the last gate (a failure or conflict waits until one of them moves)
    pub ms: String,
    pub cs: String,
    /// `work <sha> main <sha>` once landed
    pub landed: String,
    pub detail: Vec<String>,
    pub log: String,
    pub agent: String,
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
            ("node", s(&self.node)),
            ("ms", s(&self.ms)),
            ("cs", s(&self.cs)),
            ("landed", s(&self.landed)),
            ("detail", Value::Arr(self.detail.iter().map(|d| s(d)).collect())),
            ("log", s(&self.log)),
            ("agent", s(&self.agent)),
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
            node: s("node"),
            ms: s("ms"),
            cs: s("cs"),
            landed: s("landed"),
            detail: v.get("detail").and_then(|d| d.as_arr()).map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default(),
            log: s("log"),
            agent: s("agent"),
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

/// `base` + `sha` as a merge commit (no worktree): Ok(commit), or Err(the conflicting paths).
pub fn merge_onto(dir: &Path, base: &str, sha: &str, msg: &str) -> Result<String, Vec<String>> {
    let (ok, out) = git(dir, &["merge-tree", "--write-tree", "--name-only", "--no-messages", base, sha]);
    let mut lines = out.lines();
    let tree = lines.next().unwrap_or("").to_string();
    if !ok || tree.len() != 40 {
        let files: Vec<String> = lines.map(str::to_string).filter(|l| !l.is_empty()).collect();
        return Err(if files.is_empty() { vec![format!("merge-tree failed ({base:.10} + {sha:.10})")] } else { files });
    }
    let (ok, c) = git(dir, &["commit-tree", &tree, "-p", base, "-p", sha, "-m", msg]);
    if ok && c.len() == 40 {
        Ok(c)
    } else {
        Err(vec![format!("commit-tree failed ({base:.10} + {sha:.10})")])
    }
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
    exe: PathBuf,
    publish: Publish,
    st: Mutex<Vec<Entry>>,
    clock: fn() -> u64,
}

impl Train {
    fn path(&self) -> PathBuf {
        self.home.join("land.jsonl")
    }
    fn load(&self) {
        let text = std::fs::read_to_string(self.path()).unwrap_or_default();
        *self.st.lock().unwrap() = text.lines().filter_map(Entry::parse).collect();
    }
    fn save(&self, list: &[Entry]) {
        let text: String = list.iter().map(|e| e.to_json() + "\n").collect();
        let tmp = self.home.join("land.jsonl.new");
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, self.path());
        }
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
                let Some(i) = list.iter().rposition(|e| e.names(b) && e.state != St::Landed) else {
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
            ["import", landq, failed, held] => match self.import(Path::new(landq), Path::new(failed), Path::new(held)) {
                Ok(n) => out.push(format!("{{\"ok\":1,\"imported\":{n}}}")),
                Err(e) => err(out, &e),
            },
            _ => err(out, "land add <branch|-> [<commercial worktree> <commercial branch>] | status [all|<branch>] | hold <b> [reason] | unhold <b> | retry <b> | drop <b> | import <landq> <failed> <held>"),
        }
    }

    fn add(&self, w: &[&str]) -> Result<String, String> {
        let agent = w.iter().find_map(|x| x.strip_prefix("agent=")).unwrap_or("").to_string();
        let w: Vec<&str> = w.iter().copied().filter(|x| !x.starts_with("agent=")).collect();
        let (branch, cwt, cb) = match w.as_slice() {
            [b] => (*b, "", ""),
            [b, wt, cb] => (*b, *wt, *cb),
            _ => return Err("add <branch|-> [<commercial worktree> <commercial branch>]".into()),
        };
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
                e.state = St::Queued;
                e.detail.clear();
                e.changed_ms = now;
                e.id
            }
            None => {
                let id = list.iter().map(|e| e.id).max().unwrap_or(0) + 1;
                list.push(Entry { id, branch: branch.into(), cwt: cwt.into(), cbranch: cb.into(), state: St::Queued, held: String::new(), node: String::new(), ms: String::new(), cs: String::new(), landed: String::new(), detail: Vec::new(), log: String::new(), agent, asked_ms: now, changed_ms: now });
                id
            }
        };
        let pos = list.iter().filter(|e| e.state != St::Landed && e.id <= id).count();
        self.save(&list);
        let label = list.iter().find(|e| e.id == id).map(Entry::label).unwrap_or_default();
        drop(list);
        self.event(Sev::Info, format!("land queued {label} (#{id}, {pos} open)"), &[]);
        Ok(format!("{{\"ok\":1,\"id\":{id},\"pos\":{pos}}}"))
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

    /// One round; false when nothing was pending.
    fn round(self: &Arc<Self>) -> bool {
        let r = &self.repo;
        let c = r.join("apps/commercial");
        let (Some(w0), Some(m0)) = (rev(r, "refs/heads/work"), rev(&c, "refs/heads/main")) else { return false };
        // pin and pick
        let list = self.st.lock().unwrap().clone();
        let mut picked: Vec<(u64, String, String, String, String)> = Vec::new(); // id, ms, cs, cm, cc
        for e in list.iter().filter(|e| e.state != St::Landed && e.held.is_empty()) {
            let ms = if e.branch == "-" { Some(String::new()) } else { rev(r, &format!("refs/heads/{}", e.branch)) };
            let cs = if e.cbranch.is_empty() { Some(String::new()) } else { rev(Path::new(&e.cwt), &format!("refs/heads/{}", e.cbranch)) };
            let (Some(ms), Some(cs)) = (ms, cs) else {
                self.set(e.id, |e| {
                    e.state = St::Failed;
                    e.detail = vec!["a branch of this entry is gone".into()];
                }, Some((Sev::Warn, format!("land failed {}: a branch is gone", e.label()))));
                continue;
            };
            let mk_in = ms.is_empty() || is_ancestor(r, &ms, &w0);
            let c_in = cs.is_empty() || is_ancestor(&c, &cs, &m0);
            if mk_in && c_in {
                self.set(e.id, |e| {
                    e.state = St::Landed;
                    e.landed = format!("work {:.10} main {:.10} (already in)", w0, m0);
                }, Some((Sev::Info, format!("land landed {} (already in work/main)", e.label()))));
                continue;
            }
            // a failure or conflict waits for one of its branches to move
            if matches!(e.state, St::Failed | St::Conflict) && e.ms == ms && e.cs == cs {
                continue;
            }
            let label = e.label();
            let cm = if mk_in { Ok(w0.clone()) } else { merge_onto(r, &w0, &ms, &format!("Merge branch '{}' into work (devhub land)", e.branch)) };
            let cc = if c_in { Ok(m0.clone()) } else { merge_onto(&c, &m0, &cs, &format!("Merge branch '{}' into main (devhub land)", e.cbranch)) };
            match (cm, cc) {
                (Ok(cm), Ok(cc)) => picked.push((e.id, ms, cs, cm, cc)),
                (Err(f), _) | (_, Err(f)) => {
                    let (ms2, cs2) = (ms.clone(), cs.clone());
                    self.set(e.id, move |e| {
                        e.state = St::Conflict;
                        e.ms = ms2;
                        e.cs = cs2;
                        e.detail = f;
                    }, Some((Sev::Warn, format!("land conflict {label} with work {:.10} / main {:.10}", w0, m0))));
                }
            }
        }
        if picked.is_empty() {
            return false;
        }
        // all gates at once
        let logs = self.home.join("land-logs");
        let _ = std::fs::create_dir_all(&logs);
        let stamp = (self.clock)();
        let mut handles = Vec::new();
        for (id, ms, cs, cm, cc) in picked.iter().cloned() {
            let log = logs.join(format!("{id}-{stamp}.log"));
            let label = self.st.lock().unwrap().iter().find(|e| e.id == id).map(Entry::label).unwrap_or_default();
            self.set(id, |e| {
                e.state = St::Gating;
                e.ms = ms;
                e.cs = cs;
                e.node.clear();
                e.detail.clear();
                e.log = log.to_string_lossy().into_owned();
            }, Some((Sev::Info, format!("land gating {label} ({:.10}/{:.10})", cm, cc))));
            let t = self.clone();
            let (w0, m0) = (w0.clone(), m0.clone());
            handles.push(std::thread::spawn(move || t.gate(id, &label, &cm, &cc, &w0, &m0, &log)));
        }
        let passed: Vec<u64> = handles.into_iter().filter_map(|h| h.join().ok().flatten()).collect();
        // land the passing ones in queue order
        let mut landed = 0;
        for (id, ms, cs, cm, cc) in picked.iter().filter(|p| passed.contains(&p.0)) {
            if self.land_one(*id, ms, cs, cm, cc, &w0, &m0) {
                landed += 1;
            }
        }
        if landed > 0 {
            // LIVEDEPLOY: the WM nodes get the new tips now
            let _ = Command::new(&self.exe).args(["devhub", "sync", "now"]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
        }
        true
    }

    /// One entry's quick gate (a lease on its own lane); Some(id) when it passed.
    fn gate(&self, id: u64, label: &str, cm: &str, cc: &str, w0: &str, m0: &str, log: &Path) -> Option<u64> {
        let file = std::fs::File::create(log).ok()?;
        let err = file.try_clone().ok()?;
        let mut child = Command::new(&self.exe)
            .args(["devhub", "lease", "gate", "--needs", GATE_NEEDS, "--lane", &format!("landq{id}"), "--prio", "0", "--rev", &format!("land@{cm},land@{cc}"), "--"])
            .args(["bash", "tools/run/fixture/land_gate.sh", "quick", w0, m0])
            .current_dir(&self.repo)
            .env("DEVHUB_AGENT", "devhub-land")
            .stdin(Stdio::null())
            .stdout(file)
            .stderr(err)
            .spawn()
            .ok()?;
        let mut node = String::new();
        // the node, once the lease is granted (its first log line names it)
        let see_node = |node: &mut String| {
            if node.is_empty() {
                let text = std::fs::read_to_string(log).unwrap_or_default();
                if let Some(n) = text.lines().find_map(|l| l.split_once(" gate@").and_then(|x| x.1.split_once(':')).map(|x| x.0.to_string())) {
                    *node = n.clone();
                    self.set(id, |e| e.node = n.clone(), Some((Sev::Info, format!("land gating {label} on {n}"))));
                }
            }
        };
        let status = loop {
            if let Ok(Some(s)) = child.try_wait() {
                break s;
            }
            see_node(&mut node);
            std::thread::sleep(Duration::from_secs(2));
        };
        see_node(&mut node);
        let text = std::fs::read_to_string(log).unwrap_or_default();
        let verdict = text.lines().rev().find(|l| l.starts_with("NODEGATE-")).unwrap_or("").to_string();
        let steps: Vec<String> = text.lines().filter_map(|l| l.strip_prefix("STEP ")).map(|s| s.split(" [").next().unwrap_or(s).to_string()).collect();
        if status.success() && verdict.starts_with("NODEGATE-OK") {
            self.set(id, |e| e.state = St::Passed, Some((Sev::Info, format!("land passed {label} on {node} ({})", steps.join(", ")))));
            Some(id)
        } else {
            let mut d = vec![if verdict.is_empty() { format!("no verdict (exit {:?})", status.code()) } else { crate::proto::clean(&verdict, 300) }];
            d.extend(failure_lines(&text));
            self.set(id, |e| {
                e.state = St::Failed;
                e.detail = d;
            }, Some((Sev::Warn, format!("land failed {label} on {node}"))));
            None
        }
    }

    /// Fast-forwards work and main to one passed entry (re-merged onto tips this round moved); true when it landed.
    fn land_one(&self, id: u64, ms: &str, cs: &str, cm: &str, cc: &str, w0: &str, m0: &str) -> bool {
        let r = &self.repo;
        let c = r.join("apps/commercial");
        let lock = r.join(".git/land-ff.lock");
        while std::fs::create_dir(&lock).is_err() {
            std::thread::sleep(Duration::from_secs(5));
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
        let on = |dir: &Path, want: &str| git(dir, &["branch", "--show-current"]).1 == want;
        if !on(r, "work") || !on(&c, "main") {
            return wait("the user checkout is not on work / main".into());
        }
        let (Some(w), Some(m)) = (rev(r, "refs/heads/work"), rev(&c, "refs/heads/main")) else { return wait("no work/main".into()) };
        // the gated candidate when the tip did not move, else a clean re-merge onto the moved tip (not re-gated)
        let tm = if ms.is_empty() || is_ancestor(r, ms, &w) {
            w.clone()
        } else if w == w0 {
            cm.to_string()
        } else {
            match merge_onto(r, &w, ms, &format!("Merge branch '{branch}' into work (devhub land)")) {
                Ok(x) => x,
                Err(f) => return wait(format!("passed, but no longer merges onto work as this round moved it ({}); next round", f.join(" "))),
            }
        };
        let tc = if cs.is_empty() || is_ancestor(&c, cs, &m) {
            m.clone()
        } else if m == m0 {
            cc.to_string()
        } else {
            match merge_onto(&c, &m, cs, &format!("Merge branch '{cbranch}' into main (devhub land)")) {
                Ok(x) => x,
                Err(f) => return wait(format!("passed, but no longer merges onto main as this round moved it ({}); next round", f.join(" "))),
            }
        };
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
        unlock();
        self.set(id, |e| {
            e.state = St::Landed;
            e.landed = format!("work {tm:.10} main {tc:.10}");
            e.detail.clear();
        }, Some((Sev::Info, format!("land landed {label}: work {tm:.10} main {tc:.10}"))));
        true
    }

    fn run(self: Arc<Self>) {
        // entries a restarted master left mid-round go back to the queue
        {
            let mut list = self.st.lock().unwrap();
            for e in list.iter_mut().filter(|e| matches!(e.state, St::Gating | St::Passed | St::Waiting)) {
                e.state = St::Queued;
            }
            self.save(&list);
        }
        loop {
            if !self.round() {
                std::thread::sleep(IDLE);
            }
        }
    }
}

/// The master's train: loads the queue, starts the round thread, registers the `land` control service.
pub fn register(ep: &makepad_remote::Endpoint, machine: &str, publish: Publish) -> Arc<Train> {
    let t = Arc::new(Train {
        machine: machine.to_string(),
        home: crate::proto::home(),
        repo: PathBuf::from(REPO),
        exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from(REPO).join("target/host/makepad")),
        publish,
        st: Mutex::new(Vec::new()),
        clock: crate::proto::wall_ms,
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
    fn entries_round_trip_and_label() {
        let e = Entry { id: 3, branch: "rc-x".into(), cwt: "/w/x/apps/commercial".into(), cbranch: "rc-x".into(), state: St::Failed, held: String::new(), node: "104".into(), ms: "a".repeat(40), cs: String::new(), landed: String::new(), detail: vec!["stage: 1 errors".into(), "src/a.rs:3:4: \"quoted\"".into()], log: "/l".into(), agent: "x".into(), asked_ms: 1, changed_ms: 2 };
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
            exe: exe.to_path_buf(),
            publish: Arc::new(move |r: Record| events.lock().unwrap().push(r.line.clone())),
            st: Mutex::new(Vec::new()),
            clock: crate::proto::wall_ms,
        })
    }

    /// A whole round without a master: hand-ins, a conflict, a hold, gates (a stand-in gate command that
    /// passes), the fast-forward of work and main in queue order, the status lines and events.
    #[test]
    fn a_round_gates_each_entry_and_lands_the_passing_ones_in_order() {
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
        g(&r, &["checkout", "-q", "work"]);
        g(&c, &["checkout", "-q", "main"]);
        std::fs::write(r.join("a"), "w\n").unwrap();
        g(&r, &["commit", "-qam", "work moves"]);
        // the gate: a stand-in for `makepad devhub lease gate ... land_gate.sh quick`, which passes
        let exe = d.join("gate.sh");
        std::fs::write(&exe, "#!/bin/sh\necho \"L1 gate@n1: granted, setting up\"\necho \"STEP qcheck 1 s [x-y]\"\necho \"NODEGATE-OK quick\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        let events = Arc::new(Mutex::new(Vec::new()));
        let t = train(&home, &r, &exe, events.clone());
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
        assert!(t.round());
        let st = |b: &str| Entry::parse(&ask(&format!("status {b}"))[0]).unwrap();
        assert_eq!(st("rc-x").state, St::Landed, "{:?}", st("rc-x"));
        assert_eq!(st("rc-y").state, St::Landed, "{:?}", st("rc-y"));
        assert_eq!((st("rc-z").state, st("rc-z").detail.clone()), (St::Conflict, vec!["a".to_string()]));
        assert_eq!((st("rc-h").state, st("rc-h").held.as_str()), (St::Queued, "waits for the user"));
        assert_eq!(st("rc-x").node, "n1");
        // work has both makepad halves, main the commercial half; the user's checkouts moved with them
        let work = rev(&r, "refs/heads/work").unwrap();
        assert!(is_ancestor(&r, &rev(&r, "rc-x").unwrap(), &work) && is_ancestor(&r, &rev(&r, "rc-y").unwrap(), &work));
        assert!(is_ancestor(&c, &rev(&c, "rc-y").unwrap(), &rev(&c, "refs/heads/main").unwrap()));
        assert!(r.join("x").exists() && r.join("y").exists() && c.join("sy").exists());
        // nothing changed: the conflict waits for its branch to move, the held entry for its unhold
        assert!(!t.round());
        // the queue survives a restart
        let t2 = train(&home, &r, &exe, events.clone());
        t2.load();
        assert_eq!(t2.st.lock().unwrap().len(), 4);
        let ev = events.lock().unwrap().join("\n");
        for want in ["land queued rc-x", "land gating rc-x on n1", "land passed rc-x on n1", "land landed rc-y+rc-y", "land conflict rc-z"] {
            assert!(ev.contains(want), "no `{want}` in\n{ev}");
        }
        assert!(ask("drop rc-z")[0].contains("ok"));
        assert_eq!(ask("status all").len(), 3);
        let _ = std::fs::remove_dir_all(&d);
    }
}
