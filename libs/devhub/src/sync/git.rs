//! git, the only tool sync drives on a node: one bare store per repo (`~/devhub/git/<repo>.git`,
//! DESIGN.md §10.12) that bundles are fetched into, and the node's ONE tree (the synced checkout,
//! a linked worktree of the store), moved between commits with `read-tree -m -u` by syncs and
//! leases alike. Only files that differ between the two commits are written (cargo fingerprints
//! and the JITs' watchers see exactly the change); a file with local edits, or an untracked file
//! in the way, makes git refuse, and nothing moves.
//!
//! Every call is `git` with fixed arguments (never a shell); hooks are off (`core.hooksPath` to
//! /dev/null), so nothing that arrives in a bundle ever runs.

use std::path::{Path, PathBuf};
use std::process::Stdio;

/// The ref that names the commit the node's tree holds now (a sync's or a lease's).
pub const CHECKOUT_REF: &str = "refs/devhub/checkout";
/// The ref that names the last commit a sync moved the tree to: syncs move forward from it only.
pub const SYNCED_REF: &str = "refs/devhub/synced";
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

    /// The repo's name (`makepad.git` -> `makepad`).
    fn name(&self) -> String {
        self.git_dir.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
    }

    /// Makes `work` a linked worktree of this store (its `.git` file points at
    /// `<store>/worktrees/<name>`), so git in the node's one tree answers what it holds (`git
    /// rev-parse HEAD`, `git diff` in a lease's scripts). A tree that is one already stays as it
    /// is; else its index is read from `at` (the commit it holds) and refreshed (no file is
    /// rewritten), and only tracked files it lacks are written (a tree a deploy script shipped
    /// had no dotfiles; an empty dir gets every file). A `.git` that is not ours: refused.
    pub fn link(&self, work: &Path, at: &str) -> Result<(), String> {
        let dotgit = work.join(".git");
        if let Some(admin) = self.admin_of(work) {
            if admin.join("HEAD").exists() {
                return Ok(());
            }
        }
        if dotgit.exists() {
            return Err(format!("{} has a .git of its own: not a devhub tree", work.display()));
        }
        std::fs::create_dir_all(work).map_err(|e| format!("{}: {e}", work.display()))?;
        // the admin dir comes from `worktree add` into an empty dir next to the store; its `.git`
        // file then moves into the tree and `worktree repair` points the admin dir at it
        let adopt = stores_dir().join(format!("{}.adopt", self.name()));
        let tmp = adopt.join("tree");
        if tmp.exists() {
            let _ = std::fs::remove_file(tmp.join(".git"));
            let _ = std::fs::remove_dir(&tmp);
        }
        std::fs::create_dir_all(&adopt).map_err(|e| format!("{}: {e}", adopt.display()))?;
        self.git(&["worktree", "add", "-q", "--no-checkout", "--detach", &tmp.to_string_lossy(), at])?;
        let text = std::fs::read_to_string(tmp.join(".git")).map_err(|e| format!("{}: {e}", tmp.display()))?;
        std::fs::write(&dotgit, &text).map_err(|e| format!("{}: {e}", dotgit.display()))?;
        let _ = std::fs::remove_file(tmp.join(".git"));
        let _ = std::fs::remove_dir(&tmp);
        let _ = std::fs::remove_dir(&adopt);
        self.git(&["worktree", "repair", &work.to_string_lossy()])?;
        // FLEET-REVISION.txt is the tree's own record, never a change of the tree
        let exclude = self.git_dir.join("info").join("exclude");
        let ex = std::fs::read_to_string(&exclude).unwrap_or_default();
        if !ex.lines().any(|l| l == "/FLEET-REVISION.txt") {
            let _ = std::fs::create_dir_all(self.git_dir.join("info"));
            let _ = std::fs::write(&exclude, format!("{ex}{}/FLEET-REVISION.txt\n", if ex.is_empty() || ex.ends_with('\n') { "" } else { "\n" }));
        }
        self.read_index(work, at)
    }

    /// The tree's index read from `at` and refreshed against the files (none is rewritten), then
    /// the tracked files it lacks written.
    fn read_index(&self, work: &Path, at: &str) -> Result<(), String> {
        let g = |args: &[&str]| run(Some(work), None, &[], args);
        g(&["read-tree", at])?;
        // exit 1 lists files that differ from `at`: they stay as they are
        let _ = g(&["update-index", "-q", "--refresh"]);
        let missing = g(&["ls-files", "-d", "-z"])?;
        let missing: Vec<&str> = missing.split('\0').filter(|f| !f.is_empty()).collect();
        for chunk in missing.chunks(500) {
            // -u: the index records the written files' stat data (else the next move finds them
            // "not uptodate")
            let mut args = vec!["checkout-index", "-f", "-u", "--"];
            args.extend(chunk.iter().copied());
            g(&args)?;
        }
        Ok(())
    }

    /// The admin dir `work`'s `.git` file names when it is one of this store's worktrees.
    fn admin_of(&self, work: &Path) -> Option<PathBuf> {
        let text = std::fs::read_to_string(work.join(".git")).ok()?;
        let dir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        let common = std::fs::canonicalize(&self.git_dir).ok()?;
        std::fs::canonicalize(&dir).ok().filter(|d| d.starts_with(common.join("worktrees")))
    }

    /// Moves the tree at `work` from `from` (None: an empty or missing dir) to `to`, in place:
    /// only files that differ between the two commits are written (cargo fingerprints and the
    /// JITs' watchers see exactly the change). Any two commits (a lease's pin, then landed work
    /// again): whether a sync may move forward only is the caller's rule. A file with local edits
    /// that the move changes, or an untracked file in the way, makes git refuse, and nothing moves.
    pub fn move_checkout(&self, work: &Path, from: Option<&str>, to: &str) -> Result<Moved, String> {
        let g = |args: &[&str]| run(Some(work), None, &[], args);
        let Some(from) = from else {
            self.link(work, to)?;
            self.set_ref(CHECKOUT_REF, to)?;
            return Ok(Moved { files: Vec::new() });
        };
        self.link(work, from)?;
        let head = g(&["rev-parse", "-q", "--verify", "HEAD"]).map(|s| s.trim().to_string()).unwrap_or_default();
        if head != from {
            // the index describes HEAD: read it from `from` (what the tree holds) first
            g(&["update-ref", "--no-deref", "HEAD", from])?;
            self.read_index(work, from)?;
        }
        if from != to {
            // stat data current (a file another process rewrote with the same content is not an
            // edit); exit 1 names real edits, which read-tree then refuses if the move changes them
            let _ = g(&["update-index", "-q", "--refresh"]);
            g(&["read-tree", "-m", "-u", from, to])?;
            g(&["update-ref", "--no-deref", "HEAD", to])?;
        }
        let files = if from == to { Vec::new() } else { self.git(&["diff", "--name-only", "--no-renames", from, to])?.lines().map(str::to_string).collect() };
        self.set_ref(CHECKOUT_REF, to)?;
        Ok(Moved { files })
    }

    /// Changes to tracked files in the tree. Untracked files never count: the tree is also where
    /// the node's WM and apps keep what they write, and devhub never removes it.
    pub fn tree_dirty(&self, work: &Path) -> Result<bool, String> {
        run(Some(work), None, &[], &["status", "--porcelain", "--untracked-files=no", "--ignore-submodules=all"]).map(|o| !o.trim().is_empty())
    }

    /// A lease left changes in tracked files of the tree: they are committed (`WIP devhub
    /// recovery <id>`), bundled to `out` (minus what the store's refs hold), and those files go
    /// back to `at` (only they are written; untracked and ignored files such as `target/` stay).
    /// Returns the bundle. Never `git stash`.
    pub fn recover_tree(&self, work: &Path, at: &str, id: &str, out: &Path, tag: &str) -> Result<PathBuf, String> {
        let g = |args: &[&str]| run(Some(work), None, &[], args);
        g(&["add", "-u", "--", "."])?;
        g(&["-c", "user.name=devhub", "-c", "user.email=devhub@localhost", "commit", "-q", "--no-verify", "-m", &format!("WIP devhub recovery {id}")])?;
        std::fs::create_dir_all(out).map_err(|e| format!("{}: {e}", out.display()))?;
        let bundle = out.join(format!("{tag}.bundle"));
        let b = bundle.to_string_lossy().into_owned();
        let refs = g(&["for-each-ref", "--format=%(objectname)"]).unwrap_or_default();
        let mut args = vec!["bundle", "create", "-q", &b, "HEAD"];
        let shas: Vec<&str> = refs.lines().map(str::trim).filter(|s| !s.is_empty()).collect();
        if !shas.is_empty() {
            args.push("--not");
            args.extend(shas);
        }
        g(&args)?;
        g(&["reset", "-q", "--hard", at])?;
        self.set_ref(CHECKOUT_REF, at)?;
        Ok(bundle)
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

/// Tests that point MAKEPAD_DEVHUB_GIT at their own stores run one at a time.
#[cfg(test)]
pub(crate) static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

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

    fn mtime(p: &Path) -> std::time::SystemTime {
        std::fs::metadata(p).unwrap().modified().unwrap()
    }

    /// A first move of a shipped tree (no .git, dotfiles missing) links it as a worktree of the
    /// store; moves to any commit write only the changed files; a local edit in a changing file
    /// makes it refuse; a lease's leftovers are bundled and the tree goes back to its commit.
    #[test]
    fn the_tree_moves_in_place_to_any_commit() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let base = std::env::temp_dir().join(format!("devhub-sync-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (src, work, stores) = (base.join("src"), base.join("work"), base.join("stores"));
        std::fs::create_dir_all(&src).unwrap();
        sh(&src, &["init", "-q", "-b", "work"]);
        std::fs::write(src.join("a.txt"), "a1").unwrap();
        std::fs::write(src.join("b.txt"), "b1").unwrap();
        std::fs::write(src.join(".gitignore"), "/target/\n").unwrap();
        sh(&src, &["add", "."]);
        sh(&src, &["commit", "-q", "-m", "one"]);
        let one = sh(&src, &["rev-parse", "HEAD"]).trim().to_string();
        std::fs::write(src.join("a.txt"), "a2").unwrap();
        sh(&src, &["commit", "-q", "-am", "two"]);
        let two = sh(&src, &["rev-parse", "HEAD"]).trim().to_string();
        // a lane branch off `one`
        sh(&src, &["checkout", "-q", "-b", "rc-x", &one]);
        std::fs::write(src.join("b.txt"), "b-lane").unwrap();
        sh(&src, &["commit", "-q", "-am", "lane"]);
        let lane = sh(&src, &["rev-parse", "HEAD"]).trim().to_string();

        std::env::set_var("MAKEPAD_DEVHUB_GIT", &stores);
        let store = Store::open("makepad").unwrap();
        let bundle = base.join("x.bundle");
        sh(&src, &["bundle", "create", "-q", &bundle.to_string_lossy(), "refs/heads/work", "refs/heads/rc-x"]);
        assert_eq!(store.fetch_bundle(&bundle, "work").unwrap(), two);
        assert_eq!(store.fetch_bundle(&bundle, "rc-x").unwrap(), lane);

        // a tree shipped at `one` by a deploy script: no .git, no dotfiles, a target and FLEET-REVISION
        std::fs::create_dir_all(work.join("target")).unwrap();
        std::fs::write(work.join("a.txt"), "a1").unwrap();
        std::fs::write(work.join("b.txt"), "b1").unwrap();
        std::fs::write(work.join("target/x.rlib"), "bin").unwrap();
        write_fleet_revision(&work, &one, None);
        let b_time = mtime(&work.join("b.txt"));
        std::thread::sleep(std::time::Duration::from_millis(20));
        let m = store.move_checkout(&work, Some(&one), &two).unwrap();
        assert_eq!(m.files, vec!["a.txt"]);
        assert_eq!(std::fs::read_to_string(work.join("a.txt")).unwrap(), "a2");
        assert_eq!(mtime(&work.join("b.txt")), b_time, "an unchanged file keeps its mtime");
        assert!(work.join(".gitignore").exists() && work.join(".git").is_file(), "linked: git answers in the tree");
        assert_eq!(run(Some(&work), None, &[], &["rev-parse", "HEAD"]).unwrap().trim(), two);
        assert!(!store.tree_dirty(&work).unwrap(), "target/ is ignored, FLEET-REVISION.txt excluded");
        assert_eq!(store.rev(CHECKOUT_REF).as_deref(), Some(two.as_str()));

        // a lease moves it to an unlanded lane commit (not a fast-forward) and back: only changed files
        let a_time = mtime(&work.join("a.txt"));
        let m = store.move_checkout(&work, Some(&two), &lane).unwrap();
        assert_eq!(m.files, vec!["a.txt", "b.txt"]);
        assert_eq!(std::fs::read_to_string(work.join("b.txt")).unwrap(), "b-lane");
        assert_eq!(std::fs::read_to_string(work.join("target/x.rlib")).unwrap(), "bin", "the target stays");
        assert_ne!(mtime(&work.join("a.txt")), a_time);

        // the lease leaves an edit and a new file: the edit is bundled and undone, the new
        // (untracked) file stays: the tree is also where the WM's apps write
        std::fs::write(work.join("new.txt"), "new").unwrap();
        assert!(!store.tree_dirty(&work).unwrap(), "untracked files do not count");
        std::fs::write(work.join("b.txt"), "lane edit").unwrap();
        assert!(store.tree_dirty(&work).unwrap());
        let out = base.join("recovered");
        let b = store.recover_tree(&work, &lane, "L9", &out, "x-L9").unwrap();
        assert!(!store.tree_dirty(&work).unwrap() && work.join("new.txt").exists());
        assert_eq!(std::fs::read_to_string(work.join("b.txt")).unwrap(), "b-lane");
        sh(&src, &["fetch", "-q", &b.to_string_lossy(), "HEAD:refs/heads/recovered"]);
        assert_eq!(sh(&src, &["show", "recovered:b.txt"]), "lane edit");

        // a local edit in a file the move changes: refused, nothing moves
        std::fs::write(work.join("b.txt"), "local edit").unwrap();
        assert!(store.move_checkout(&work, Some(&lane), &two).is_err());
        assert_eq!(std::fs::read_to_string(work.join("b.txt")).unwrap(), "local edit");
        assert_eq!(store.rev(CHECKOUT_REF).as_deref(), Some(lane.as_str()));
        std::fs::write(work.join("b.txt"), "b-lane").unwrap();
        store.move_checkout(&work, Some(&lane), &two).unwrap();
        assert_eq!(std::fs::read_to_string(work.join("b.txt")).unwrap(), "b1");

        // a fresh tree in an empty dir; a clone of its own is never taken
        let fresh = base.join("fresh");
        store.move_checkout(&fresh, None, &lane).unwrap();
        assert_eq!(std::fs::read_to_string(fresh.join("b.txt")).unwrap(), "b-lane");
        assert!(store.link(&src, &one).unwrap_err().contains("of its own"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
