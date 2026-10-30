//! The master's orchestration jobs, in the master itself (no shell waiters, no .txt ledgers):
//! - [`lane`]: the lane worktree lifecycle (new, adopt, cleanup after landing, drop);
//! - [`verify`]: the rolling full verifier of the newest landed work + main pair;
//! - [`after`]: `devhub after <condition> -- <cmd>`, run once the condition holds;
//!
//! Their state is [`crate::state`] (shared with `devhub land` and `lanes`); every change is an
//! event on the master's stream (`devhub watch <kinds>`). On the user's Mac the master runs only
//! git and leases: every build and test goes to a node.

pub mod after;
pub mod cli;
pub mod lane;
pub mod leg;
pub mod verify;

use crate::lease::master::LeaseMaster;
use crate::state::{Pair, Store};
pub use crate::state::short;
use makepad_remote::json_str;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The control services of the jobs (`cli` talks to them).
pub const SERVICES: [&str; 4] = ["lane", "verify", "after", "state"];

/// What every job has: the state, the leases, the checkout the master serves.
pub struct Ctx {
    pub store: Arc<Store>,
    pub leases: Arc<LeaseMaster>,
    /// the master's machine name (`after … -- launch <it>` launches here)
    pub machine: String,
    /// the user checkout (makepad `work`; commercial `main` in its `apps/commercial`)
    pub repo: PathBuf,
    /// the lane worktrees' dirty sets (the OS file monitor on the lanes root)
    pub monitor: Arc<crate::monitor::Monitor>,
}

impl Ctx {
    pub fn commercial(&self) -> PathBuf {
        self.repo.join("apps/commercial")
    }

    /// Where the jobs keep their logs (`~/.makepad/devhub/<job>`).
    pub fn logs(&self, job: &str) -> PathBuf {
        crate::proto::home().join(job)
    }
}

/// The checkout this master was built from: the one it serves.
pub fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..").canonicalize().unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../.."))
}

/// Registers the services and starts the jobs (the master only).
pub fn start(ep: &makepad_remote::Endpoint, machine: &str, leases: Arc<LeaseMaster>, publish: crate::state::Publish) {
    let store = crate::state::Store::open(&crate::proto::home(), machine, publish);
    let repo = repo();
    let monitor = crate::monitor::Monitor::start(&[crate::lease::wt::root_of(&repo)]);
    if let Some(e) = &monitor.error {
        store.event("lane", crate::proto::Sev::Warn, format!("lane: the file monitor did not start ({e}); dirty sets come from git status"), None);
    }
    let ctx = Arc::new(Ctx { store, leases, machine: machine.to_string(), repo, monitor });
    for svc in SERVICES {
        let c = ctx.clone();
        ep.register(svc, 1, makepad_remote::lines::service(svc, Arc::new(move |line, out| serve(&c, svc, line, out))));
    }
    lane::start(ctx.clone());
    verify::start(ctx.clone());
    after::start(ctx);
}

fn serve(ctx: &Arc<Ctx>, svc: &str, line: &str, out: &mut Vec<String>) {
    let r = match svc {
        "lane" => lane::serve(ctx, line),
        "verify" => verify::serve(ctx, line),
        "after" => after::serve(ctx, line),
        "state" => {
            let name = line.split_whitespace().nth(1);
            ctx.store.read(|d| d.section(name)).ok_or_else(|| format!("no state section `{}` (verify allapps after pushes lanes jobs)", name.unwrap_or("")))
        }
        _ => Err(format!("no service {svc}")),
    };
    out.push(r.unwrap_or_else(|e| err(&e)));
}

pub fn err(e: &str) -> String {
    format!("{{\"err\":{}}}", json_str(e))
}

/// `git -C <dir> <args>`: its stdout (trimmed), or why it failed.
pub fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let o = makepad_remote::group::command("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !o.status.success() {
        let e = String::from_utf8_lossy(&o.stderr);
        return Err(format!("git {}: {}", args.first().unwrap_or(&""), crate::proto::clean(e.trim(), 300)));
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// `git merge-base --is-ancestor a b`.
pub fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    makepad_remote::group::command("git")
        .arg("-C")
        .arg(dir)
        .args(["merge-base", "--is-ancestor", a, b])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// The newest landed pair: work and commercial main, read twice, never while a landing moves them
/// (`land-ff.lock` in the makepad repo's common dir). No ref is written.
pub fn snapshot(repo: &Path) -> Option<Pair> {
    let common = PathBuf::from(git(repo, &["rev-parse", "--path-format=absolute", "--git-common-dir"]).ok()?);
    let lock = common.join("land-ff.lock");
    if lock.exists() {
        return None;
    }
    let com = repo.join("apps/commercial");
    let read = || Some(Pair { work: git(repo, &["rev-parse", "refs/heads/work"]).ok()?, main: git(&com, &["rev-parse", "refs/heads/main"]).ok()? });
    let (a, b) = (read()?, read()?);
    (a == b && a.valid() && !lock.exists()).then_some(a)
}

/// `5s`, `3m`, `2h`, `4d` ago.
pub fn age(ms: u64) -> String {
    if ms == 0 {
        return "-".into();
    }
    let s = crate::proto::wall_ms().saturating_sub(ms) / 1000;
    match s {
        0..=119 => format!("{s}s"),
        120..=7199 => format!("{}m", s / 60),
        7200..=172_799 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

/// A run id from the wall clock: `20261030T110641Z` (UTC).
pub fn stamp(ms: u64) -> String {
    let secs = ms / 1000;
    let (days, rest) = (secs / 86_400, secs % 86_400);
    // civil from days (Howard Hinnant's algorithm)
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}T{:02}{:02}{:02}Z", rest / 3600, (rest / 60) % 60, rest % 60)
}

/// Removes the oldest run dirs/logs of a job beyond `keep` (names are stamps, so they sort by age).
pub fn prune(dir: &Path, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut names: Vec<String> = rd.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).filter(|n| n.len() >= 16 && n.as_bytes()[8] == b'T').collect();
    names.sort();
    for n in names.iter().take(names.len().saturating_sub(keep)) {
        let p = dir.join(n);
        if p.is_dir() {
            let _ = std::fs::remove_dir_all(&p);
        } else {
            let _ = std::fs::remove_file(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn stamps_are_utc_and_sort() {
        assert_eq!(super::stamp(0), "19700101T000000Z");
        assert_eq!(super::stamp(1_793_358_401_000), "20261030T110641Z");
        assert!(super::stamp(1_793_358_401_000) < super::stamp(1_793_358_402_000));
    }
}
