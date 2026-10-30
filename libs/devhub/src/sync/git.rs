//! git, the only tool sync drives on a node: one bare store per repo (`~/devhub/git/<repo>.git`,
//! DESIGN.md §10.12) that bundles are fetched into, and the synced checkout, moved between commits
//! with `read-tree -m -u` on the store's own index of it. Only files that differ between the two
//! commits are written (cargo fingerprints and the JITs' watchers see exactly the change); a file
//! with local edits, or an untracked file in the way, makes git refuse, and nothing moves.
//!
//! Every call is `git` with fixed arguments (never a shell); hooks are off (`core.hooksPath` to
//! /dev/null), so nothing that arrives in a bundle ever runs.

use std::path::{Path, PathBuf};
use std::process::Stdio;

/// The ref that names the commit the checkout holds (the store's index of it matches it).
pub const CHECKOUT_REF: &str = "refs/devhub/checkout";
/// Pushed branches land under here (`refs/devhub/pushed/<branch>`).
pub const PUSHED: &str = "refs/devhub/pushed/";

pub fn valid_sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}

/// A branch name sync carries: what a ref and a line can hold, nothing a git option or range
/// could be read from.
pub fn valid_branch(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 128
        && !s.starts_with(['-', '/', '.'])
        && !s.ends_with(['/', '.'])
        && !s.contains("..")
        && !s.contains("//")
        && !s.ends_with(".lock")
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'/'))
}

pub fn valid_repo(s: &str) -> bool {
    matches!(s, "makepad" | "commercial")
}

/// `~/devhub/git` (DESIGN.md §10.12), next to the lease worktrees and targets.
pub fn stores_dir() -> PathBuf {
    std::env::var_os("MAKEPAD_DEVHUB_GIT").map(PathBuf::from).unwrap_or_else(|| crate::os::home_dir().join("devhub").join("git"))
}

/// Runs `git <args>` in `dir` (or with the store as GIT_DIR); stdout on success.
pub fn run(dir: Option<&Path>, git_dir: Option<&Path>, env: &[(&str, &Path)], args: &[&str]) -> Result<String, String> {
    let mut cmd = makepad_remote::group::command("git");
    cmd.arg("-c").arg("core.hooksPath=/dev/null").arg("-c").arg("advice.detachedHead=false");
    if let Some(g) = git_dir {
        cmd.arg("--git-dir").arg(g);
    }
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(d) = dir {
        cmd.current_dir(d);
    }
    // the hub's own environment never steers git; `env` is the call's
    cmd.env("GIT_TERMINAL_PROMPT", "0").env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).take(12).collect();
        Err(format!("git {}: {}", args.first().unwrap_or(&""), err.join(" | ")))
    }
}

/// One repo's store on a node.
pub struct Store {
    pub git_dir: PathBuf,
}

/// What a checkout move did.
#[derive(Debug, Default)]
pub struct Moved {
    /// the files that changed between the two commits
    pub files: Vec<String>,
}

impl Store {
    pub fn open(name: &str) -> Result<Store, String> {
        let git_dir = stores_dir().join(format!("{name}.git"));
        if !git_dir.join("HEAD").exists() {
            std::fs::create_dir_all(&git_dir).map_err(|e| format!("{}: {e}", git_dir.display()))?;
            run(None, None, &[], &["init", "--bare", "-q", &git_dir.to_string_lossy()])?;
        }
        Ok(Store { git_dir })
    }

    fn git(&self, args: &[&str]) -> Result<String, String> {
        run(None, Some(&self.git_dir), &[], args)
    }

    /// The commit a ref names, if any.
    pub fn rev(&self, r: &str) -> Option<String> {
        self.git(&["rev-parse", "-q", "--verify", &format!("{r}^{{commit}}")]).ok().map(|s| s.trim().to_string()).filter(|s| valid_sha(s))
    }

    pub fn has(&self, sha: &str) -> bool {
        valid_sha(sha) && self.git(&["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_ok()
    }

    pub fn is_ancestor(&self, a: &str, b: &str) -> bool {
        self.git(&["merge-base", "--is-ancestor", a, b]).is_ok()
    }

    /// `refs/devhub/*` with their commits: what the master can use as bundle bases.
    pub fn refs(&self) -> Vec<(String, String)> {
        self.git(&["for-each-ref", "--format=%(objectname) %(refname)", "refs/devhub/"])
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once(' '))
            .map(|(sha, r)| (r.to_string(), sha.to_string()))
            .collect()
    }

    /// Checks a bundle and fetches its branch into `refs/devhub/pushed/<branch>`; returns the
    /// commit it holds.
    pub fn fetch_bundle(&self, bundle: &Path, branch: &str) -> Result<String, String> {
        // one fetch at a time: two leases shipping different pins of one branch at once must each
        // read back their own commit (and not fail on the ref's lock)
        static FETCH: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _one = FETCH.lock().unwrap_or_else(|e| e.into_inner());
        let b = bundle.to_string_lossy();
        self.git(&["bundle", "verify", "-q", &b])?;
        let dst = format!("{PUSHED}{branch}");
        self.git(&["fetch", "-q", "--no-tags", "--no-write-fetch-head", &b, &format!("+refs/heads/{branch}:{dst}")])?;
        self.rev(&dst).ok_or_else(|| format!("the bundle holds no branch {branch}"))
    }

    pub fn set_ref(&self, r: &str, sha: &str) -> Result<(), String> {
        self.git(&["update-ref", r, sha]).map(|_| ())
    }

    fn index(&self) -> PathBuf {
        self.git_dir.join("checkout.index")
    }

    /// Moves the checkout at `work` from `from` (None: an empty or missing dir) to `to`.
    /// Refuses anything but a fast-forward, local edits in files that change, and untracked files
    /// in the way.
    pub fn move_checkout(&self, work: &Path, from: Option<&str>, to: &str) -> Result<Moved, String> {
        let index = self.index();
        let env: [(&str, &Path); 2] = [("GIT_INDEX_FILE", &index), ("GIT_WORK_TREE", work)];
        let g = |args: &[&str]| run(Some(work), Some(&self.git_dir), &env, args);
        match from {
            Some(from) if from == to => return Ok(Moved { files: Vec::new() }),
            Some(from) => {
                if !self.is_ancestor(from, to) {
                    return Err(format!("not a fast-forward: {} is not an ancestor of {}", &from[..10.min(from.len())], &to[..10.min(to.len())]));
                }
                // the index describes `from` only when it was written for it (a first sync of a
                // tree a deploy script shipped has none: it is read from `from` and refreshed)
                if self.rev(CHECKOUT_REF).as_deref() != Some(from) || !index.exists() {
                    g(&["read-tree", from])?;
                    // exit 1 lists files that differ from `from`: read-tree -m -u refuses those
                    // that change; the rest stay as they are
                    let _ = g(&["update-index", "-q", "--refresh"]);
                }
                g(&["read-tree", "-m", "-u", from, to])?;
            }
            None => {
                std::fs::create_dir_all(work).map_err(|e| format!("{}: {e}", work.display()))?;
                let _ = std::fs::remove_file(&index);
                g(&["read-tree", "-m", "-u", to])?;
            }
        }
        let files = match from {
            Some(from) => self.git(&["diff", "--name-only", "--no-renames", from, to])?.lines().map(str::to_string).collect(),
            None => Vec::new(),
        };
        self.set_ref(CHECKOUT_REF, to)?;
        Ok(Moved { files })
    }
}

/// The commits the deploy scripts recorded in `FLEET-REVISION.txt` (`makepad <sha>`,
/// `commercial <sha>`): where a first sync of a deployed tree starts from.
pub fn fleet_revision(checkout: &Path, repo: &str) -> Option<String> {
    let text = std::fs::read_to_string(checkout.join("FLEET-REVISION.txt")).ok()?;
    text.lines().find_map(|l| l.strip_prefix(&format!("{repo} ")).map(|s| s.trim().to_string())).filter(|s| valid_sha(s))
}

pub fn write_fleet_revision(checkout: &Path, makepad: &str, commercial: Option<&str>) {
    let mut text = format!("makepad {makepad}\n");
    if let Some(c) = commercial {
        text.push_str(&format!("commercial {c}\n"));
    }
    text.push_str(&format!("synced {} (devhub sync)\n", crate::proto::wall_ms() / 1000));
    let tmp = checkout.join("FLEET-REVISION.txt.new");
    if std::fs::write(&tmp, text).is_ok() {
        let _ = std::fs::rename(&tmp, checkout.join("FLEET-REVISION.txt"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_that_cannot_turn_into_options() {
        assert!(valid_branch("work") && valid_branch("rc-devhub2") && valid_branch("lane/x.y"));
        for bad in ["", "-x", "--upload-pack=x", "a..b", "a b", "a/", "/a", "x.lock", "a//b", "a;b", "^x"] {
            assert!(!valid_branch(bad), "{bad}");
        }
        assert!(valid_sha("d8cb46d6c1a69041cd12c5822fc2f89a46fedc81"));
        assert!(!valid_sha("D8CB46D6C1A69041CD12C5822FC2F89A46FEDC81"));
        assert!(!valid_sha("d8cb46d6c1"));
    }

    fn sh(dir: &Path, args: &[&str]) -> String {
        run(Some(dir), None, &[("GIT_AUTHOR_DATE", Path::new("1700000000 +0000")), ("GIT_COMMITTER_DATE", Path::new("1700000000 +0000"))], &[&["-c", "user.name=t", "-c", "user.email=t@t"], args].concat()).unwrap()
    }

    /// A first sync of a shipped tree, a fast-forward that writes only the changed files, a
    /// local edit that makes it refuse, and a non-fast-forward that is refused.
    #[test]
    fn checkout_moves_only_by_fast_forward() {
        let base = std::env::temp_dir().join(format!("devhub-sync-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, work, stores) = (base.join("src"), base.join("work"), base.join("stores"));
        std::fs::create_dir_all(&src).unwrap();
        sh(&src, &["init", "-q", "-b", "work"]);
        std::fs::write(src.join("a.txt"), "a1").unwrap();
        std::fs::write(src.join("b.txt"), "b1").unwrap();
        sh(&src, &["add", "."]);
        sh(&src, &["commit", "-q", "-m", "one"]);
        let one = sh(&src, &["rev-parse", "HEAD"]).trim().to_string();
        std::fs::write(src.join("a.txt"), "a2").unwrap();
        sh(&src, &["commit", "-q", "-am", "two"]);
        let two = sh(&src, &["rev-parse", "HEAD"]).trim().to_string();

        std::env::set_var("MAKEPAD_DEVHUB_GIT", &stores);
        let store = Store::open("makepad").unwrap();
        let bundle = base.join("x.bundle");
        sh(&src, &["bundle", "create", "-q", &bundle.to_string_lossy(), "refs/heads/work"]);
        assert_eq!(store.fetch_bundle(&bundle, "work").unwrap(), two);

        // a tree shipped at `one` by a deploy script (no index yet)
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("a.txt"), "a1").unwrap();
        std::fs::write(work.join("b.txt"), "b1").unwrap();
        std::fs::write(work.join("untracked.log"), "kept").unwrap();
        let m = store.move_checkout(&work, Some(&one), &two).unwrap();
        assert_eq!(m.files, vec!["a.txt"]);
        assert_eq!(std::fs::read_to_string(work.join("a.txt")).unwrap(), "a2");
        assert_eq!(std::fs::read_to_string(work.join("untracked.log")).unwrap(), "kept");
        assert_eq!(store.rev(CHECKOUT_REF).as_deref(), Some(two.as_str()));

        // a local edit in a file the next commit changes: refused, nothing moves
        std::fs::write(src.join("a.txt"), "a3").unwrap();
        sh(&src, &["commit", "-q", "-am", "three"]);
        let three = sh(&src, &["rev-parse", "HEAD"]).trim().to_string();
        sh(&src, &["bundle", "create", "-q", &bundle.to_string_lossy(), "refs/heads/work", &format!("^{two}")]);
        store.fetch_bundle(&bundle, "work").unwrap();
        std::fs::write(work.join("a.txt"), "local edit").unwrap();
        assert!(store.move_checkout(&work, Some(&two), &three).is_err());
        assert_eq!(std::fs::read_to_string(work.join("a.txt")).unwrap(), "local edit");
        assert_eq!(store.rev(CHECKOUT_REF).as_deref(), Some(two.as_str()));

        // going back is not a fast-forward
        std::fs::write(work.join("a.txt"), "a2").unwrap();
        let e = store.move_checkout(&work, Some(&two), &one).unwrap_err();
        assert!(e.contains("not a fast-forward"), "{e}");

        // a fresh checkout into an empty dir
        let fresh = base.join("fresh");
        store.move_checkout(&fresh, None, &three).unwrap();
        assert_eq!(std::fs::read_to_string(fresh.join("a.txt")).unwrap(), "a3");
        let _ = std::fs::remove_dir_all(&base);
    }
}
