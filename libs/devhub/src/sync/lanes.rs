//! The master keeps every open lane branch merged with the tip it lands on: when work (makepad) or
//! main (commercial) moves, each `rc-*` branch checked out in a worktree under the lanes root
//! (`lanes_root` in autosync.conf, default `~/wt.noindex`) that is not yet in the tip gets the tip
//! merged into it, in the lane's own worktree. Both repos' halves of a lane are handled the same
//! way, each against its own tip (makepad against work, commercial against main).
//!
//! - Only a clean worktree is touched (no uncommitted tracked changes, no merge/rebase/cherry-pick
//!   in progress): a lane's edits are never disturbed; a dirty one is retried every round.
//! - The merge is computed with plumbing first (`merge-tree --write-tree`, nothing checked out):
//!   a conflict touches nothing in the worktree, it is recorded (branch, files, both tips) and
//!   published as a warning event at once; it is not retried until the branch or the tip moves.
//! - A clean merge becomes the commit "Merge work (<sha>) into <branch>" (`commit-tree`, both
//!   parents), which the worktree then fast-forwards to (`merge --ff-only`): it either moves
//!   whole or not at all, so a worktree is never left half merged.
//! - Held branches (`devhub hold <branch>`) are skipped.
//! - Never a push, a force, a reset or a rebase. A merge gives the branch a new sha, which the
//!   landing train treats as a new candidate (landbatch.failed skips only the sha that failed).
//!
//! State (the master's state dir, like leases.live): `lanes.state` (one JSON line per lane half:
//! the last result) and `lanes.held` (`<branch> <why>`). `devhub lanes` lists it; every change of
//! a lane's result is an event (`kind: lanes`) on the master's stream.

use crate::proto::{self, hub_record, Sev};
use makepad_remote::json_str;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

pub use crate::local::Publish;

/// What the bot last did with (or found in) one lane half.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// the branch already holds the tip
    UpToDate,
    /// the branch is in the tip (landed): nothing to merge
    Landed,
    /// the bot merged the tip in: the merge commit
    Merged(String),
    /// the tip does not merge cleanly: the conflicting files (nothing was touched)
    Conflict(Vec<String>),
    /// the worktree has uncommitted tracked changes or an operation in progress
    Dirty(String),
    /// `devhub hold`: why
    Held(String),
    /// git failed (a lock, the branch moved under it): retried next round
    Failed(String),
}

impl Outcome {
    pub fn word(&self) -> &'static str {
        match self {
            Outcome::UpToDate => "up-to-date",
            Outcome::Landed => "landed",
            Outcome::Merged(_) => "merged",
            Outcome::Conflict(_) => "conflict",
            Outcome::Dirty(_) => "dirty",
            Outcome::Held(_) => "held",
            Outcome::Failed(_) => "failed",
        }
    }
}

/// One lane half: a branch of one repo in one worktree.
#[derive(Clone, Debug, PartialEq)]
pub struct Lane {
    /// makepad | commercial
    pub repo: String,
    pub branch: String,
    pub worktree: PathBuf,
    /// the branch's commit when last looked at (after a merge: the merge)
    pub tip: String,
    /// the tip it was compared with (work or main)
    pub base: String,
    pub outcome: Outcome,
    pub at_ms: u64,
}

impl Lane {
    pub fn to_json(&self) -> String {
        let (merge, files, why) = match &self.outcome {
            Outcome::Merged(c) => (json_str(c), String::new(), String::new()),
            Outcome::Conflict(f) => ("null".into(), f.iter().map(|f| json_str(f)).collect::<Vec<_>>().join(","), String::new()),
            Outcome::Dirty(w) | Outcome::Held(w) | Outcome::Failed(w) => ("null".into(), String::new(), w.clone()),
            _ => ("null".into(), String::new(), String::new()),
        };
        format!(
            "{{\"branch\":{},\"repo\":{},\"worktree\":{},\"tip\":{},\"base\":{},\"result\":\"{}\",\"merge\":{merge},\"files\":[{files}],\"why\":{},\"at_ms\":{}}}",
            json_str(&self.branch),
            json_str(&self.repo),
            json_str(&self.worktree.display().to_string()),
            json_str(&self.tip),
            json_str(&self.base),
            self.outcome.word(),
            json_str(&why),
            self.at_ms
        )
    }

    pub fn parse(line: &str) -> Option<Lane> {
        let v = makepad_strict_json::parse(line.as_bytes()).ok()?;
        let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
        let why = s("why");
        let outcome = match s("result").as_str() {
            "up-to-date" => Outcome::UpToDate,
            "landed" => Outcome::Landed,
            "merged" => Outcome::Merged(s("merge")),
            "conflict" => Outcome::Conflict(v.get("files").and_then(|f| f.as_arr()).map(|f| f.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()),
            "dirty" => Outcome::Dirty(why),
            "held" => Outcome::Held(why),
            "failed" => Outcome::Failed(why),
            _ => return None,
        };
        Some(Lane { repo: s("repo"), branch: s("branch"), worktree: PathBuf::from(s("worktree")), tip: s("tip"), base: s("base"), outcome, at_ms: v.get("at_ms").and_then(|x| x.as_u64()).unwrap_or(0) })
    }

    /// The one-line event text for this result.
    fn event(&self) -> (Sev, String) {
        let tip_name = if self.repo == "commercial" { "main" } else { "work" };
        let b = &self.base[..self.base.len().min(10)];
        match &self.outcome {
            Outcome::Merged(c) => (Sev::Info, format!("lanes: merged {tip_name} ({b}) into {} ({}): now {}", self.branch, self.repo, &c[..c.len().min(10)])),
            Outcome::Conflict(f) => (
                Sev::Warn,
                format!("lanes: {} ({}) conflicts with {tip_name} {b} at {}: {} (nothing was merged; merge {tip_name} into it by hand)", self.branch, self.repo, &self.tip[..self.tip.len().min(10)], f.join(", ")),
            ),
            Outcome::Dirty(w) => (Sev::Info, format!("lanes: {} ({}) not merged with {tip_name} {b}: {w}; retried when clean", self.branch, self.repo)),
            Outcome::Held(w) => (Sev::Info, format!("lanes: {} held ({w}): not merged", self.branch)),
            Outcome::Failed(w) => (Sev::Warn, format!("lanes: {} ({}) merge of {tip_name} {b} failed: {w}; retried next round", self.branch, self.repo)),
            Outcome::Landed => (Sev::Info, format!("lanes: {} ({}) is in {tip_name}", self.branch, self.repo)),
            Outcome::UpToDate => (Sev::Info, format!("lanes: {} ({}) holds {tip_name} {b}", self.branch, self.repo)),
        }
    }
}

/// Where the lanes are: the repo (its `apps/commercial` is the commercial repo) and the root
/// lane worktrees live under.
#[derive(Clone, Debug)]
pub struct Where {
    pub repo: PathBuf,
    pub root: PathBuf,
}

/// git in `dir`: (success, stdout, stderr). Hooks off, never a prompt, the hub's env never steers it.
fn git(dir: &Path, env: &[(String, String)], args: &[&str]) -> (bool, String, String) {
    let mut cmd = Command::new("git");
    cmd.arg("-c").arg("core.hooksPath=/dev/null").args(args).current_dir(dir).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.env("GIT_TERMINAL_PROMPT", "0").env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE");
    for (k, v) in env {
        cmd.env(k, v);
    }
    match cmd.output() {
        Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned()),
        Err(e) => (false, String::new(), format!("git: {e}")),
    }
}

fn first_err(err: &str) -> String {
    let l: Vec<&str> = err.lines().map(str::trim).filter(|l| !l.is_empty()).take(3).collect();
    proto::clean(&l.join(" | "), 300)
}

/// `worktree list --porcelain`: (path, branch, head) of every worktree on a branch.
pub fn parse_worktrees(text: &str) -> Vec<(PathBuf, String, String)> {
    let mut out = Vec::new();
    for block in text.split("\n\n") {
        let (mut path, mut head, mut branch) = (None, None, None);
        for l in block.lines() {
            if let Some(p) = l.strip_prefix("worktree ") {
                path = Some(PathBuf::from(p));
            } else if let Some(h) = l.strip_prefix("HEAD ") {
                head = Some(h.to_string());
            } else if let Some(b) = l.strip_prefix("branch refs/heads/") {
                branch = Some(b.to_string());
            }
        }
        if let (Some(p), Some(h), Some(b)) = (path, head, branch) {
            out.push((p, b, h));
        }
    }
    out
}

pub struct Lanes {
    machine: String,
    home: PathBuf,
    publish: Publish,
    /// extra git env (tests: an identity)
    env: Vec<(String, String)>,
    state: Mutex<BTreeMap<(String, String), Lane>>,
    held: Mutex<BTreeMap<String, String>>,
    wake: (Mutex<bool>, Condvar),
}

impl Lanes {
    pub fn new(machine: &str, home: PathBuf, publish: Publish) -> Arc<Lanes> {
        let mut state = BTreeMap::new();
        for l in std::fs::read_to_string(home.join("lanes.state")).unwrap_or_default().lines().filter_map(Lane::parse) {
            state.insert((l.repo.clone(), l.branch.clone()), l);
        }
        let mut held = BTreeMap::new();
        for line in std::fs::read_to_string(home.join("lanes.held")).unwrap_or_default().lines() {
            let mut w = line.splitn(2, ' ');
            if let Some(b) = w.next().filter(|b| !b.is_empty()) {
                held.insert(b.to_string(), w.next().unwrap_or("").trim().to_string());
            }
        }
        Arc::new(Lanes { machine: machine.into(), home, publish, env: Vec::new(), state: Mutex::new(state), held: Mutex::new(held), wake: (Mutex::new(false), Condvar::new()) })
    }

    #[cfg(test)]
    fn with_env(machine: &str, home: PathBuf, publish: Publish, env: Vec<(String, String)>) -> Arc<Lanes> {
        let l = Lanes::new(machine, home, publish);
        let mut l = Arc::try_unwrap(l).ok().unwrap();
        l.env = env;
        Arc::new(l)
    }

    fn write_atomic(&self, name: &str, text: String) {
        let _ = std::fs::create_dir_all(&self.home);
        let (path, tmp) = (self.home.join(name), self.home.join(format!("{name}.new")));
        if std::fs::write(&tmp, text).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }

    fn save(&self) {
        let text: String = self.state.lock().unwrap().values().map(|l| format!("{}\n", l.to_json())).collect();
        self.write_atomic("lanes.state", text);
    }

    pub fn list(&self) -> Vec<Lane> {
        self.state.lock().unwrap().values().cloned().collect()
    }

    pub fn held(&self) -> Vec<(String, String)> {
        self.held.lock().unwrap().iter().map(|(b, w)| (b.clone(), w.clone())).collect()
    }

    pub fn hold(&self, branch: &str, why: &str) -> Result<(), String> {
        if !crate::sync::git::valid_branch(branch) {
            return Err(format!("`{branch}` is not a branch name"));
        }
        let why = if why.trim().is_empty() { "held".to_string() } else { proto::clean(why.trim(), 200) };
        self.held.lock().unwrap().insert(branch.to_string(), why.clone());
        self.save_held();
        (self.publish)(hub_record(&self.machine, "", 0, "lanes", Sev::Info, format!("lanes: {branch} held ({why}): the master merges nothing into it"), String::new()));
        Ok(())
    }

    pub fn unhold(&self, branch: &str) -> Result<(), String> {
        if self.held.lock().unwrap().remove(branch).is_none() {
            return Err(format!("{branch} is not held"));
        }
        self.save_held();
        (self.publish)(hub_record(&self.machine, "", 0, "lanes", Sev::Info, format!("lanes: {branch} no longer held"), String::new()));
        self.poke();
        Ok(())
    }

    fn save_held(&self) {
        let text: String = self.held.lock().unwrap().iter().map(|(b, w)| format!("{b} {w}\n")).collect();
        self.write_atomic("lanes.held", text);
    }

    /// Wakes the loop for a round now.
    pub fn poke(&self) {
        *self.wake.0.lock().unwrap() = true;
        self.wake.1.notify_all();
    }

    fn sleep(&self, d: Duration) {
        let g = self.wake.0.lock().unwrap();
        let (mut g, _) = self.wake.1.wait_timeout_while(g, d, |w| !*w).unwrap();
        *g = false;
    }

    /// One round over both repos; returns the lanes whose result changed.
    pub fn pass(&self, w: &Where) -> Result<Vec<Lane>, String> {
        let mut changed = Vec::new();
        let mut seen = Vec::new();
        for (repo, dir, tip_branch) in [("makepad", w.repo.clone(), "work"), ("commercial", w.repo.join("apps").join("commercial"), "main")] {
            if !dir.join(".git").exists() {
                continue;
            }
            let (ok, base, err) = git(&dir, &self.env, &["rev-parse", "-q", "--verify", &format!("refs/heads/{tip_branch}^{{commit}}")]);
            if !ok {
                return Err(format!("{repo}: no {tip_branch}: {}", first_err(&err)));
            }
            let base = base.trim().to_string();
            let (ok, list, err) = git(&dir, &self.env, &["worktree", "list", "--porcelain"]);
            if !ok {
                return Err(format!("{repo}: worktree list: {}", first_err(&err)));
            }
            for (path, branch, head) in parse_worktrees(&list) {
                if !branch.starts_with("rc-") || !path.starts_with(&w.root) || !path.is_dir() {
                    continue;
                }
                let key = (repo.to_string(), branch.clone());
                seen.push(key.clone());
                let prev = self.state.lock().unwrap().get(&key).cloned();
                let outcome = self.one(repo, &dir, &path, &branch, &head, &base, tip_branch, prev.as_ref());
                let tip = match &outcome {
                    Outcome::Merged(c) => c.clone(),
                    _ => head.clone(),
                };
                let lane = Lane { repo: repo.into(), branch, worktree: path, tip, base: base.clone(), outcome, at_ms: proto::wall_ms() };
                let same = prev.as_ref().is_some_and(|p| p.outcome == lane.outcome && (p.tip == lane.tip || matches!(lane.outcome, Outcome::Merged(_))) && (p.base == lane.base || matches!(lane.outcome, Outcome::Landed | Outcome::Held(_))));
                if !same {
                    let (sev, line) = lane.event();
                    // a lane that just caught up after the bot's own merge says nothing new
                    let quiet = matches!(lane.outcome, Outcome::UpToDate) && prev.as_ref().is_some_and(|p| matches!(p.outcome, Outcome::Merged(_)));
                    if !quiet {
                        (self.publish)(hub_record(&self.machine, "", 0, "lanes", sev, line, lane.to_json()));
                    }
                    changed.push(lane.clone());
                    self.state.lock().unwrap().insert(key, lane);
                } else if let Some(p) = self.state.lock().unwrap().get_mut(&key) {
                    p.tip = lane.tip;
                    p.base = lane.base;
                }
            }
        }
        // lanes whose worktree went away
        let before = self.state.lock().unwrap().len();
        self.state.lock().unwrap().retain(|k, _| seen.contains(k));
        if !changed.is_empty() || before != self.state.lock().unwrap().len() {
            self.save();
        }
        Ok(changed)
    }

    #[allow(clippy::too_many_arguments)]
    fn one(&self, repo: &str, dir: &Path, wt: &Path, branch: &str, head: &str, base: &str, tip_branch: &str, prev: Option<&Lane>) -> Outcome {
        if let Some(why) = self.held.lock().unwrap().get(branch) {
            return Outcome::Held(why.clone());
        }
        let anc = |a: &str, b: &str| git(dir, &self.env, &["merge-base", "--is-ancestor", a, b]).0;
        // results that only a move of the branch or the tip changes: no git at all
        if let Some(p) = prev.filter(|p| p.tip == head && p.base == base) {
            if matches!(p.outcome, Outcome::UpToDate | Outcome::Landed | Outcome::Conflict(_) | Outcome::Merged(_)) {
                return p.outcome.clone();
            }
        }
        if anc(head, base) {
            return Outcome::Landed;
        }
        if anc(base, head) {
            // the bot's own merge, seen again: keep saying what it did
            if let Some(Outcome::Merged(c)) = prev.map(|p| &p.outcome) {
                if c == head {
                    return Outcome::Merged(c.clone());
                }
            }
            return Outcome::UpToDate;
        }
        if let Some(p) = prev.filter(|p| p.tip == head && p.base == base && matches!(p.outcome, Outcome::Conflict(_))) {
            return p.outcome.clone();
        }
        // the lane's own work in progress is never touched
        for op in ["MERGE_HEAD", "rebase-merge", "rebase-apply", "CHERRY_PICK_HEAD", "REVERT_HEAD"] {
            let (ok, p, _) = git(wt, &self.env, &["rev-parse", "--git-path", op]);
            if ok && wt.join(p.trim()).exists() {
                return Outcome::Dirty(format!("a {} is in progress", op.to_lowercase().replace('_', " ")));
            }
        }
        let (ok, st, err) = git(wt, &self.env, &["status", "--porcelain", "--untracked-files=no"]);
        if !ok {
            return Outcome::Failed(format!("status: {}", first_err(&err)));
        }
        let dirty: Vec<&str> = st.lines().filter(|l| l.len() > 3).map(|l| &l[3..]).collect();
        if !dirty.is_empty() {
            return Outcome::Dirty(format!("{} uncommitted file{} ({})", dirty.len(), if dirty.len() == 1 { "" } else { "s" }, dirty.iter().take(4).copied().collect::<Vec<_>>().join(", ")));
        }
        // the merge, computed without touching any worktree
        let (ok, out, err) = git(dir, &self.env, &["merge-tree", "--write-tree", "--name-only", "--no-messages", head, base]);
        let mut lines = out.lines();
        let tree = lines.next().unwrap_or("").trim().to_string();
        if !ok {
            let files: Vec<String> = lines.map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
            if files.is_empty() || !crate::sync::git::valid_sha(&tree) {
                return Outcome::Failed(format!("merge-tree: {}", first_err(&err)));
            }
            return Outcome::Conflict(files);
        }
        let msg = format!("Merge {tip_branch} ({}) into {branch}", &base[..base.len().min(10)]);
        let (ok, commit, err) = git(dir, &self.env, &["commit-tree", &tree, "-p", head, "-p", base, "-m", &msg]);
        let commit = commit.trim().to_string();
        if !ok || !crate::sync::git::valid_sha(&commit) {
            return Outcome::Failed(format!("commit-tree: {}", first_err(&err)));
        }
        // whole or not at all: the worktree fast-forwards to the merge (it refuses if the lane
        // committed or touched a file meanwhile; the next round looks again)
        let (ok, _, err) = git(wt, &self.env, &["merge", "--ff-only", "-q", &commit]);
        if !ok {
            return Outcome::Failed(format!("fast-forward to the merge: {}", first_err(&err)));
        }
        let _ = repo;
        Outcome::Merged(commit)
    }

    /// The `lanes` control service: `list`, `held`, `hold <branch> [why]`, `unhold <branch>`, `now`.
    pub fn serve(&self, line: &str, out: &mut Vec<String>) {
        let line = line.strip_prefix("lanes").map_or(line, str::trim_start);
        let mut w = line.splitn(3, ' ');
        let err = |e: &str| format!("{{\"err\":{}}}", json_str(e));
        match (w.next().unwrap_or(""), w.next(), w.next()) {
            ("" | "list", None, None) => {
                let held = self.held.lock().unwrap().clone();
                out.extend(self.list().iter().map(|l| l.to_json()));
                // held branches with no worktree (yet) are listed too
                for (b, why) in held {
                    if !self.state.lock().unwrap().keys().any(|(_, x)| *x == b) {
                        out.push(format!("{{\"branch\":{},\"result\":\"held\",\"why\":{}}}", json_str(&b), json_str(&why)));
                    }
                }
            }
            ("held", None, None) => out.extend(self.held().iter().map(|(b, w)| format!("{{\"held\":{},\"why\":{}}}", json_str(b), json_str(w)))),
            ("hold", Some(b), why) => match self.hold(b, why.unwrap_or("")) {
                Ok(()) => out.push(format!("{{\"held\":{}}}", json_str(b))),
                Err(e) => out.push(err(&e)),
            },
            ("unhold", Some(b), None) => match self.unhold(b) {
                Ok(()) => out.push(format!("{{\"unheld\":{}}}", json_str(b))),
                Err(e) => out.push(err(&e)),
            },
            ("now", None, None) => {
                self.poke();
                out.push("{\"poked\":true}".into());
            }
            _ => out.push(err("lanes [list] | held | hold <branch> [why] | unhold <branch> | now")),
        }
    }

    /// The master's loop: one round per interval (autosync.conf's), or at once when poked.
    pub fn start(self: &Arc<Self>) {
        let me = self.clone();
        let spawned = std::thread::Builder::new().name("devhub-lanes".into()).spawn(move || {
            let mut last_err = String::new();
            loop {
                let text = std::fs::read_to_string(me.home.join("autosync.conf")).unwrap_or_default();
                let conf = super::autosync::AutoConf::parse(&text, &makepad_app_registry::repo_root().unwrap_or_default());
                if conf.lanes && !conf.repo.as_os_str().is_empty() {
                    let w = Where { repo: conf.repo.clone(), root: conf.lanes_root.clone() };
                    match me.pass(&w) {
                        Ok(_) => last_err.clear(),
                        Err(e) if e != last_err => {
                            (me.publish)(hub_record(&me.machine, "", 0, "lanes", Sev::Warn, format!("lanes: {e}"), String::new()));
                            last_err = e;
                        }
                        Err(_) => {}
                    }
                }
                me.sleep(Duration::from_secs(conf.interval_s));
            }
        });
        if let Err(e) = spawned {
            eprintln!("devhub lanes: cannot start: {e}");
        }
    }
}

/// One lane half as (sev word, its line) for the panel and the CLI.
pub fn row(lane: &Lane) -> (&'static str, String) {
    let tip_name = if lane.repo == "commercial" { "main" } else { "work" };
    let base = &lane.base[..lane.base.len().min(10)];
    let (sev, detail) = match &lane.outcome {
        Outcome::Merged(_) => ("info", format!("merged {tip_name} {base}")),
        Outcome::Conflict(f) => ("err", format!("CONFLICT with {tip_name} {base}: {}", f.join(" "))),
        Outcome::Dirty(w) => ("warn", format!("not merged: {w}")),
        Outcome::Held(w) => ("warn", format!("held: {w}")),
        Outcome::Failed(w) => ("err", format!("failed: {w}")),
        Outcome::Landed => ("info", format!("in {tip_name}")),
        Outcome::UpToDate => ("info", format!("holds {tip_name} {base}")),
    };
    let ago = proto::wall_ms().saturating_sub(lane.at_ms) / 60_000;
    (sev, format!("{:<28} {:<10} {}  {:<10} {detail}  ({ago} min ago)", lane.branch, lane.repo, &lane.tip[..lane.tip.len().min(10)], lane.outcome.word()))
}

/// The `lanes list` answer as rows (held branches with no worktree included).
pub fn rows(lines: &[String]) -> Vec<(&'static str, String, String)> {
    let home = crate::os::home_dir();
    let mut out = Vec::new();
    for l in lines {
        match Lane::parse(l) {
            Some(lane) => {
                let wt = lane.worktree.strip_prefix(&home).map_or(lane.worktree.display().to_string(), |p| format!("~/{}", p.display()));
                let (sev, line) = row(&lane);
                out.push((sev, line, wt));
            }
            None => {
                let Ok(v) = makepad_strict_json::parse(l.as_bytes()) else { continue };
                if let (Some(b), Some(w)) = (v.get("branch").and_then(|x| x.as_str()), v.get("why").and_then(|x| x.as_str())) {
                    out.push(("warn", format!("{b:<28} held ({w}), no worktree"), String::new()));
                }
            }
        }
    }
    out
}

/// `devhub lanes` as text: one line per lane half, its worktree under it.
pub fn render(lines: &[String]) -> String {
    let mut out = String::new();
    for (_, line, wt) in rows(lines) {
        out.push_str(&line);
        out.push('\n');
        if !wt.is_empty() {
            out.push_str(&format!("      {wt}\n"));
        }
    }
    if out.is_empty() {
        out.push_str("no open lane branches\n");
    }
    out
}

/// `devhub prewarm`: the master's last pre-warm record per node.
fn prewarm_cli() -> i32 {
    let lines = match crate::cli::control().and_then(|c| super::cli::ask_local(&c, "prewarm", "prewarm")) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("makepad devhub prewarm: {e}");
            return 2;
        }
    };
    let rows = super::prewarm::rows(&lines);
    if rows.is_empty() {
        println!("no pre-warm yet (it runs on idle dev-on nodes after a landing)");
    }
    for (_, line) in rows {
        println!("{line}");
    }
    0
}

/// `lanes [--json]`, `hold <branch> [why...]`, `unhold <branch>`: None for other words.
pub fn cli(args: &[&str]) -> Option<i32> {
    let line = match args {
        ["lanes", rest @ ..] if rest.iter().all(|a| *a == "--json") => "list".to_string(),
        ["lanes", "now"] => "now".to_string(),
        ["hold", b, why @ ..] => format!("hold {b} {}", why.join(" ")),
        ["unhold", b] => format!("unhold {b}"),
        ["held"] => "held".to_string(),
        ["prewarm"] => return Some(prewarm_cli()),
        ["prewarm", "run", rest @ ..] => {
            let cancel_after = match rest {
                [] => None,
                ["--cancel-after", s] => match s.parse() {
                    Ok(s) => Some(s),
                    Err(_) => return Some(2),
                },
                _ => {
                    eprintln!("makepad devhub prewarm run [--cancel-after <s>]");
                    return Some(2);
                }
            };
            let root = std::env::current_dir().unwrap_or_default();
            if !root.join("Cargo.toml").is_file() {
                eprintln!("makepad devhub prewarm run: run it at a tree's root (its Cargo.toml)");
                return Some(2);
            }
            return Some(super::prewarm::foreground(root, cancel_after));
        }
        ["lanes" | "hold" | "unhold" | "held", ..] => {
            eprintln!("makepad devhub lanes [--json] | lanes now | hold <branch> [why...] | unhold <branch> | held");
            return Some(2);
        }
        _ => return None,
    };
    let c = match crate::cli::control() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("makepad devhub: {e}");
            return Some(2);
        }
    };
    let lines = match super::cli::ask_local(&c, "lanes", line.trim()) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("makepad devhub lanes: {e} (the master runs the merge bot)");
            return Some(2);
        }
    };
    if let Some(e) = lines.iter().find_map(|l| makepad_strict_json::parse(l.as_bytes()).ok().and_then(|v| v.get("err").and_then(|e| e.as_str()).map(str::to_string))) {
        eprintln!("makepad devhub: {e}");
        return Some(1);
    }
    match args {
        ["lanes", "--json", ..] => lines.iter().for_each(|l| println!("{l}")),
        ["lanes", ..] => print!("{}", render(&lines)),
        ["held"] if lines.is_empty() => println!("no held branches"),
        ["held"] => {
            for l in &lines {
                if let Ok(v) = makepad_strict_json::parse(l.as_bytes()) {
                    println!("{}  {}", v.get("held").and_then(|x| x.as_str()).unwrap_or(""), v.get("why").and_then(|x| x.as_str()).unwrap_or(""));
                }
            }
        }
        ["hold", b, ..] => println!("{b} held: the master merges nothing into it until `makepad devhub unhold {b}`"),
        ["unhold", b] => println!("{b} unheld: merged with the tip on the next round"),
        _ => {}
    }
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    struct Fix {
        dir: PathBuf,
        repo: PathBuf,
        root: PathBuf,
        env: Vec<(String, String)>,
        events: Arc<Mutex<Vec<(Sev, String)>>>,
    }

    fn env() -> Vec<(String, String)> {
        [("GIT_AUTHOR_NAME", "t"), ("GIT_AUTHOR_EMAIL", "t@t"), ("GIT_COMMITTER_NAME", "t"), ("GIT_COMMITTER_EMAIL", "t@t"), ("GIT_CONFIG_NOSYSTEM", "1"), ("GIT_CONFIG_GLOBAL", "/dev/null")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    impl Fix {
        fn new(name: &str) -> Fix {
            static N: AtomicU64 = AtomicU64::new(0);
            let dir = std::env::temp_dir().join(format!("devhub-lanes-{name}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
            let _ = std::fs::remove_dir_all(&dir);
            let repo = dir.join("makepad");
            let root = dir.join("wt");
            std::fs::create_dir_all(&repo).unwrap();
            std::fs::create_dir_all(&root).unwrap();
            let f = Fix { dir, repo, root, env: env(), events: Arc::new(Mutex::new(Vec::new())) };
            f.g(&f.repo, &["init", "-q", "-b", "work"]);
            f.write(&f.repo, "a.txt", "one\ntwo\nthree\n");
            f.commit(&f.repo, "base");
            f
        }
        fn g(&self, dir: &Path, args: &[&str]) -> String {
            let (ok, out, err) = git(dir, &self.env, args);
            assert!(ok, "git {args:?}: {err}");
            out.trim().to_string()
        }
        fn write(&self, dir: &Path, f: &str, text: &str) {
            std::fs::write(dir.join(f), text).unwrap();
        }
        fn commit(&self, dir: &Path, msg: &str) -> String {
            self.g(dir, &["add", "-A"]);
            self.g(dir, &["commit", "-q", "-m", msg]);
            self.g(dir, &["rev-parse", "HEAD"])
        }
        fn lane(&self, name: &str) -> PathBuf {
            let wt = self.root.join(name);
            self.g(&self.repo, &["worktree", "add", "-q", "-b", &format!("rc-{name}"), wt.to_str().unwrap(), "work"]);
            wt
        }
        fn bot(&self) -> Arc<Lanes> {
            let ev = self.events.clone();
            Lanes::with_env("mac", self.dir.join("home"), Arc::new(move |r: crate::proto::Record| ev.lock().unwrap().push((r.sev, r.line))), self.env.clone())
        }
        fn w(&self) -> Where {
            Where { repo: self.repo.clone(), root: self.root.clone() }
        }
        fn result(&self, bot: &Lanes, branch: &str) -> Outcome {
            bot.list().into_iter().find(|l| l.branch == branch).map(|l| l.outcome).unwrap()
        }
    }

    impl Drop for Fix {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn worktree_porcelain() {
        let t = "worktree /r\nHEAD 1111\nbranch refs/heads/work\n\nworktree /w/a\nHEAD 2222\nbranch refs/heads/rc-a\n\nworktree /w/d\nHEAD 3333\ndetached\n";
        assert_eq!(parse_worktrees(t), vec![(PathBuf::from("/r"), "work".into(), "1111".into()), (PathBuf::from("/w/a"), "rc-a".into(), "2222".into())]);
    }

    #[test]
    fn a_clean_lane_gets_the_tip_merged_in() {
        let f = Fix::new("clean");
        let wt = f.lane("a");
        f.write(&wt, "lane.txt", "lane\n");
        let lane_tip = f.commit(&wt, "lane work");
        // work moves (an unrelated file)
        f.write(&f.repo, "b.txt", "landed\n");
        let work = f.commit(&f.repo, "landed");
        let bot = f.bot();
        let changed = bot.pass(&f.w()).unwrap();
        assert_eq!(changed.len(), 1);
        let Outcome::Merged(m) = f.result(&bot, "rc-a") else { panic!("{:?}", bot.list()) };
        // the worktree is at the merge, with both parents and the message
        assert_eq!(f.g(&wt, &["rev-parse", "HEAD"]), m);
        assert_eq!(f.g(&wt, &["rev-parse", "HEAD^1"]), lane_tip);
        assert_eq!(f.g(&wt, &["rev-parse", "HEAD^2"]), work);
        assert_eq!(f.g(&wt, &["log", "-1", "--format=%s"]), format!("Merge work ({}) into rc-a", &work[..10]));
        assert!(wt.join("b.txt").is_file());
        assert_eq!(f.g(&wt, &["status", "--porcelain"]), "");
        // the next round has nothing to do and says nothing
        let n = f.events.lock().unwrap().len();
        assert!(bot.pass(&f.w()).unwrap().is_empty());
        assert_eq!(f.events.lock().unwrap().len(), n);
        // state survives a master restart
        let again = f.bot();
        assert_eq!(f.result(&again, "rc-a"), Outcome::Merged(m));
    }

    #[test]
    fn a_conflict_touches_nothing_and_notifies() {
        let f = Fix::new("conflict");
        let wt = f.lane("c");
        f.write(&wt, "a.txt", "one\nLANE\nthree\n");
        let lane_tip = f.commit(&wt, "lane edit");
        f.write(&f.repo, "a.txt", "one\nWORK\nthree\n");
        f.commit(&f.repo, "work edit");
        let bot = f.bot();
        bot.pass(&f.w()).unwrap();
        assert_eq!(f.result(&bot, "rc-c"), Outcome::Conflict(vec!["a.txt".into()]));
        // the worktree is exactly as it was: same commit, clean, no merge in progress
        assert_eq!(f.g(&wt, &["rev-parse", "HEAD"]), lane_tip);
        assert_eq!(f.g(&wt, &["status", "--porcelain"]), "");
        let mh = f.g(&wt, &["rev-parse", "--git-path", "MERGE_HEAD"]);
        assert!(!wt.join(mh).exists());
        let ev = f.events.lock().unwrap().clone();
        assert!(ev.iter().any(|(s, l)| *s == Sev::Warn && l.contains("rc-c") && l.contains("a.txt") && l.contains(&lane_tip[..10])), "{ev:?}");
        // not retried (nor re-announced) until the branch or the tip moves
        assert!(bot.pass(&f.w()).unwrap().is_empty());
        assert_eq!(f.events.lock().unwrap().len(), ev.len());
        // the lane resolves by hand: its merge holds work, the bot sees it up to date
        f.g(&wt, &["merge", "-q", "work", "-X", "ours", "-m", "resolve"]);
        bot.pass(&f.w()).unwrap();
        assert_eq!(f.result(&bot, "rc-c"), Outcome::UpToDate);
    }

    #[test]
    fn a_dirty_worktree_is_skipped_until_clean() {
        let f = Fix::new("dirty");
        let wt = f.lane("d");
        f.write(&wt, "lane.txt", "lane\n");
        f.commit(&wt, "lane work");
        f.write(&f.repo, "b.txt", "landed\n");
        f.commit(&f.repo, "landed");
        // the lane is editing a tracked file
        f.write(&wt, "a.txt", "one\ntwo\nthree\nediting\n");
        let head = f.g(&wt, &["rev-parse", "HEAD"]);
        let bot = f.bot();
        bot.pass(&f.w()).unwrap();
        assert!(matches!(f.result(&bot, "rc-d"), Outcome::Dirty(w) if w.contains("a.txt")));
        assert_eq!(f.g(&wt, &["rev-parse", "HEAD"]), head);
        assert_eq!(std::fs::read_to_string(wt.join("a.txt")).unwrap(), "one\ntwo\nthree\nediting\n");
        // untracked files alone do not count as dirty: after the lane commits, the next round merges
        f.commit(&wt, "lane commit");
        f.write(&wt, "scratch.log", "x\n");
        bot.pass(&f.w()).unwrap();
        assert!(matches!(f.result(&bot, "rc-d"), Outcome::Merged(_)), "{:?}", bot.list());
        assert!(wt.join("scratch.log").is_file());
    }

    #[test]
    fn held_branches_and_landed_ones_are_skipped() {
        let f = Fix::new("held");
        let wt = f.lane("h");
        f.write(&wt, "lane.txt", "lane\n");
        let lane_tip = f.commit(&wt, "lane work");
        let landed = f.lane("l");
        f.write(&f.repo, "b.txt", "landed\n");
        f.commit(&f.repo, "landed");
        // a worktree outside the lanes root and a non-rc branch are not lanes
        let other = f.dir.join("elsewhere");
        f.g(&f.repo, &["worktree", "add", "-q", "-b", "rc-out", other.to_str().unwrap(), "work~1"]);
        f.g(&f.repo, &["worktree", "add", "-q", "-b", "feature", f.root.join("feature").to_str().unwrap(), "work~1"]);
        let bot = f.bot();
        bot.hold("rc-h", "needs a viewer rebuild").unwrap();
        bot.pass(&f.w()).unwrap();
        assert_eq!(f.result(&bot, "rc-h"), Outcome::Held("needs a viewer rebuild".into()));
        assert_eq!(f.g(&wt, &["rev-parse", "HEAD"]), lane_tip);
        assert_eq!(f.result(&bot, "rc-l"), Outcome::Landed);
        let _ = landed;
        let names: Vec<String> = bot.list().into_iter().map(|l| l.branch).collect();
        assert_eq!(names, vec!["rc-h".to_string(), "rc-l".to_string()]);
        // held survives a restart; unhold merges at once
        let bot = f.bot();
        assert_eq!(bot.held(), vec![("rc-h".to_string(), "needs a viewer rebuild".to_string())]);
        bot.unhold("rc-h").unwrap();
        bot.pass(&f.w()).unwrap();
        assert!(matches!(f.result(&bot, "rc-h"), Outcome::Merged(_)));
        let mut out = Vec::new();
        bot.serve("lanes list", &mut out);
        assert!(out.iter().any(|l| l.contains("\"branch\":\"rc-h\"") && l.contains("\"result\":\"merged\"")), "{out:?}");
        assert!(Lane::parse(&out[0]).is_some());
    }

    #[test]
    fn the_commercial_half_merges_main() {
        let f = Fix::new("comm");
        let c = f.repo.join("apps").join("commercial");
        std::fs::create_dir_all(&c).unwrap();
        f.g(&c, &["init", "-q", "-b", "main"]);
        f.write(&c, "s.txt", "stage\n");
        f.commit(&c, "c base");
        let cw = f.root.join("x").join("apps").join("commercial");
        f.g(&c, &["worktree", "add", "-q", "-b", "rc-x", cw.to_str().unwrap(), "main"]);
        f.write(&cw, "lane.txt", "x\n");
        f.commit(&cw, "lane");
        f.write(&c, "t.txt", "main moved\n");
        let main = f.commit(&c, "main moved");
        let bot = f.bot();
        bot.pass(&f.w()).unwrap();
        let l = bot.list().into_iter().find(|l| l.repo == "commercial").unwrap();
        assert!(matches!(l.outcome, Outcome::Merged(_)));
        assert_eq!(f.g(&cw, &["log", "-1", "--format=%s"]), format!("Merge main ({}) into rc-x", &main[..10]));
    }
}
