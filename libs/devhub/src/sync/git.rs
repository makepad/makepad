//! Git stores shared by the deployment and work trees: one bare store per repo
//! (DESIGN.md §10.12). Sync renames verified, synced copies over changed files,
//! preserving unchanged mtimes and cargo and JIT caches.
//! Local edits or untracked files in the way make the move refuse. Leases use detached
//! worktree HEADs ([`crate::lease::tree`]), independently of the deployment refs.
//!
//! Moves use makepad-git. Remaining git commands have fixed arguments and hooks disabled.

use makepad_git::{Index, IndexEntry, ObjectId, Repository, TreeChangeRecord};
use makepad_git::text_clean::{self, Conversion};
use std::collections::{BTreeSet, HashMap};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// The deployment checkout's commit; lease worktree HEADs never move it.
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

/// A store's name: makepad, or a nested repository's ([`crate::nested::valid`]).
pub fn valid_repo(s: &str) -> bool {
    s == "makepad" || crate::nested::valid(s)
}

/// The stores this node holds, by name: makepad's, and each nested repository's a sync or a
/// lease pushed.
pub fn store_names() -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(stores_dir())
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.strip_suffix(".git").filter(|n| valid_repo(n)).map(str::to_string))
        .collect();
    out.sort();
    out
}

/// `~/devhub/git` (DESIGN.md §10.12), next to the lease worktrees and targets.
pub fn stores_dir() -> PathBuf {
    std::env::var_os("MAKEPAD_DEVHUB_GIT").map(PathBuf::from).unwrap_or_else(|| crate::os::home_dir().join("devhub").join("git"))
}

/// Runs `git <args>` in `dir` (or with the store as GIT_DIR); stdout on success.
#[track_caller]
pub fn run(dir: Option<&Path>, git_dir: Option<&Path>, env: &[(&str, &Path)], args: &[&str]) -> Result<String, String> {
    run_with(dir, git_dir, env, args, None)
}

/// [`run`] with `input` on git's stdin.
#[track_caller]
fn run_with(dir: Option<&Path>, git_dir: Option<&Path>, env: &[(&str, &Path)], args: &[&str], input: Option<&[u8]>) -> Result<String, String> {
    let mut cmd = crate::git_exec::command();
    cmd.arg("-c").arg("core.hooksPath=/dev/null").arg("-c").arg("advice.detachedHead=false");
    if let Some(g) = git_dir {
        cmd.arg("--git-dir").arg(g);
    }
    cmd.args(args).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
    // git reads its working directory at start: never the hub's (proto::settle_cwd)
    if let Some(d) = dir.or(git_dir) {
        cmd.current_dir(d);
    }
    // the hub's own environment never steers git; `env` is the call's
    cmd.env("GIT_TERMINAL_PROMPT", "0").env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = match input {
        None => cmd.output(),
        Some(input) => cmd.spawn().and_then(|mut child| {
            use std::io::Write;
            // small inputs (ref edits): written whole before git's output is read
            let wrote = child.stdin.take().map_or(Ok(()), |mut i| i.write_all(input));
            let out = child.wait_with_output();
            wrote.and(out)
        }),
    }
    .map_err(|e| {
        let err = format!("git: {e}");
        crate::lease::caps::git_failed(&err);
        err
    })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = format!("git {}: {}", args.first().unwrap_or(&""), failure(out.status, &out.stderr));
        crate::lease::caps::git_failed(&err);
        Err(err)
    }
}

/// What a failed git said: its stderr's lines, and how it ended (one a signal ended says nothing:
/// an overlay feed's failure read `bare rev-list: ` and no more, 11-04).
pub fn failure(status: std::process::ExitStatus, stderr: &[u8]) -> String {
    let err = String::from_utf8_lossy(stderr);
    let lines: Vec<&str> = err.lines().map(str::trim).filter(|l| !l.is_empty()).take(12).collect();
    if lines.is_empty() { status.to_string() } else { format!("{} ({status})", lines.join(" | ")) }
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
    /// the crash debris restored before this move
    pub repaired: Vec<String>,
}

#[derive(Clone)]
struct Entry {
    path: String,
    value: Option<(ObjectId, u32)>,
}

struct Revision {
    files: HashMap<String, (ObjectId, u32)>,
    attributes: HashMap<String, Option<String>>,
}

impl Revision {
    fn read(repo: &mut Repository, rev: &str) -> Result<Self, String> {
        let oid = ObjectId::from_hex(rev).map_err(|e| e.to_string())?;
        let tree = repo.read_commit(&oid).map_err(|e| e.to_string())?.tree;
        Ok(Self { files: repo.flatten_tree_with_mode(&tree).map_err(|e| e.to_string())?, attributes: HashMap::new() })
    }

    fn entry(&self, path: &str) -> Entry {
        Entry { path: path.into(), value: self.files.get(path).copied() }
    }

    fn has_directory(&self, path: &str) -> bool {
        let prefix = format!("{path}/");
        self.files.keys().any(|p| p.starts_with(&prefix))
    }

    fn conversion(&mut self, repo: &mut Repository, common: &Path, path: &str) -> Result<Conversion, String> {
        text_clean::conversion(common, path, |dir| {
            if let Some(rules) = self.attributes.get(dir) { return Ok(rules.clone()); }
            let name = if dir.is_empty() { ".gitattributes".into() } else { format!("{dir}/.gitattributes") };
            let rules = match self.files.get(&name) {
                Some(&(oid, 0o100644 | 0o100755)) => {
                    let raw = repo.read_blob(&oid).map_err(|e| e.to_string())?;
                    verify_blob(&name, oid, &raw)?;
                    Some(String::from_utf8(raw).map_err(|e| format!("{name}: {e}"))?)
                }
                _ => None,
            };
            self.attributes.insert(dir.into(), rules.clone());
            Ok(rules)
        }).map_err(|e| format!("{path}: {e}"))
    }
}

fn verify_blob(path: &str, oid: ObjectId, raw: &[u8]) -> Result<(), String> {
    if makepad_git::oid::hash_object("blob", raw) != oid {
        return Err(format!("{path}: blob {oid} read back as {} bytes that are not it: nothing moved", raw.len()));
    }
    Ok(())
}

fn checked(work: &Path, path: &str) -> Result<PathBuf, String> {
    if path.split('/').next().is_some_and(|p| p.eq_ignore_ascii_case(".devhub-move")) {
        return Err(format!("{path}: reserved move staging path"));
    }
    makepad_git::worktree::checked_worktree_path(work, path).map_err(|e| e.to_string())
}

fn sync_dir(dir: &Path) -> Result<(), String> {
    #[cfg(unix)]
    File::open(dir).and_then(|f| f.sync_all()).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

fn remove_staging(work: &Path) -> Result<(), String> {
    let staging = work.join(".devhub-move");
    match fs::symlink_metadata(&staging) {
        Ok(m) => {
            if m.is_dir() { fs::remove_dir_all(&staging) } else { fs::remove_file(&staging) }
                .map_err(|e| format!("{}: {e}", staging.display()))?;
            sync_dir(work)
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn zero_stat(path: String, oid: ObjectId, mode: u32) -> IndexEntry {
    IndexEntry { ctime_sec: 0, ctime_nsec: 0, mtime_sec: 0, mtime_nsec: 0, dev: 0, ino: 0,
        mode, uid: 0, gid: 0, file_size: 0, oid, flags: path.len().min(0xfff) as u16, path }
}

fn link_bytes(path: &Path) -> Result<Vec<u8>, String> {
    let target = fs::read_link(path).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    { use std::os::unix::ffi::OsStrExt; Ok(target.as_os_str().as_bytes().to_vec()) }
    #[cfg(not(unix))]
    { Ok(target.to_string_lossy().as_bytes().to_vec()) }
}

fn verify_staged(staged: &Path, path: &str, oid: ObjectId, mode: u32, raw: &[u8], smudged: &[u8], conversion: &Conversion, config: &str) -> Result<(), String> {
    let meta = fs::symlink_metadata(staged).ok();
    let size = meta.as_ref().map_or(0, |m| m.len());
    let good = if mode == 0o120000 && cfg!(unix) {
        size == raw.len() as u64 && meta.as_ref().is_some_and(|m| m.file_type().is_symlink()) && link_bytes(staged).is_ok_and(|b| b == raw)
    } else if meta.as_ref().is_some_and(|m| m.is_file()) && size == smudged.len() as u64 {
        if *conversion == Conversion::default() && raw == smudged {
            makepad_git::hash_file_blob(staged).is_ok_and(|id| id == oid)
        } else {
            fs::read(staged).ok().map(|b| text_clean::bytes_clean_to(&b, oid, conversion, config)).transpose()?.unwrap_or(false)
        }
    } else { false };
    if good { Ok(()) } else {
        Err(format!("{path}: the staged copy ({size} bytes{}) is not blob {oid} ({} bytes): nothing moved",
            if meta.is_none() { ", missing" } else { "" }, raw.len()))
    }
}

// No traversal through a parent symlink: a descendant blocked by a file is absent.
enum Disk { Missing, Directory, File(Vec<u8>, u32), Symlink(Vec<u8>), Other }

impl Disk {
    fn read(work: &Path, path: &str) -> Result<Self, String> {
        let mut parent = work.to_path_buf();
        let parts: Vec<_> = path.split('/').collect();
        for part in &parts[..parts.len().saturating_sub(1)] {
            makepad_git::tree::validate_entry_name(part).map_err(|e| e.to_string())?;
            parent.push(part);
            match fs::symlink_metadata(&parent) {
                Ok(m) if !m.is_dir() => return Ok(Self::Missing),
                Ok(_) => {},
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::Missing),
                Err(e) => return Err(e.to_string()),
            }
        }
        let file = checked(work, path)?;
        match fs::symlink_metadata(&file) {
            Ok(m) if m.file_type().is_symlink() => link_bytes(&file).map(Self::Symlink),
            Ok(m) if m.is_file() => {
                #[cfg(unix)]
                let mode = { use std::os::unix::fs::PermissionsExt; if m.permissions().mode() & 0o100 != 0 { 0o100755 } else { 0o100644 } };
                #[cfg(not(unix))]
                let mode = 0o100644;
                fs::read(&file).map(|bytes| Self::File(bytes, mode)).map_err(|e| format!("{path}: {e}"))
            }
            Ok(m) if m.is_dir() => Ok(Self::Directory),
            Ok(_) => Ok(Self::Other),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => Ok(Self::Missing),
            Err(e) => Err(format!("{path}: {e}")),
        }
    }

    fn matches(&self, entry: &Entry, rev: &mut Revision, repo: &mut Repository, common: &Path, config: &str) -> Result<bool, String> {
        let Some((oid, mode)) = entry.value else {
            return Ok(matches!(self, Self::Missing) || (matches!(self, Self::Directory) && rev.has_directory(&entry.path)));
        };
        match (self, mode) {
            (Self::Directory, 0o160000) => Ok(true),
            (Self::Symlink(bytes), 0o120000) => Ok(makepad_git::oid::hash_object("blob", bytes) == oid),
            (Self::File(bytes, _), 0o120000) if !cfg!(unix) => Ok(makepad_git::oid::hash_object("blob", bytes) == oid),
            (Self::File(bytes, actual_mode), 0o100644 | 0o100755) => {
                let filemode = makepad_git::config::get(config, "core.filemode").map_err(|e| e.to_string())?
                    .as_deref().and_then(makepad_git::config::parse_bool).unwrap_or(cfg!(unix));
                if filemode && *actual_mode != mode { return Ok(false); }
                if makepad_git::oid::hash_object("blob", bytes) == oid { return Ok(true); }
                let c = rev.conversion(repo, common, &entry.path)?;
                text_clean::bytes_clean_to(bytes, oid, &c, config)
            }
            _ => Ok(false),
        }
    }
}

// A directory replacing a file is debris only when all its leaves belong to
// one of the marked trees. Unknown contents must survive the repair too.
fn directory_copy(work: &Path, path: &str, old: &mut Revision, new: &mut Revision, repo: &mut Repository, common: &Path, config: &str) -> Result<bool, String> {
    if !old.has_directory(path) && !new.has_directory(path) { return Ok(false); }
    let mut dirs = vec![path.to_string()];
    while let Some(dir) = dirs.pop() {
        for item in fs::read_dir(checked(work, &dir)?).map_err(|e| e.to_string())? {
            let item = item.map_err(|e| e.to_string())?;
            let name = item.file_name().into_string().map_err(|_| "non-UTF-8 path in move directory")?;
            let path = format!("{dir}/{name}");
            let disk = Disk::read(work, &path)?;
            if matches!(disk, Disk::Directory) && (old.has_directory(&path) || new.has_directory(&path)) {
                dirs.push(path);
            } else {
                let a = old.entry(&path);
                let b = new.entry(&path);
                if !(a.value.is_some() && disk.matches(&a, old, repo, common, config)?)
                    && !(b.value.is_some() && disk.matches(&b, new, repo, common, config)?) { return Ok(false); }
            }
        }
    }
    Ok(true)
}

fn changes(repo: &mut Repository, from: &str, to: &str) -> Result<Vec<TreeChangeRecord>, String> {
    let id = |s: &str| ObjectId::from_hex(s).map_err(|e| e.to_string());
    // Address-space bounds, not repository-size limits. Incomplete is never a move.
    let limits = makepad_git::TreeDiffLimits { max_records: usize::MAX, max_result_bytes: usize::MAX,
        max_scratch_bytes: usize::MAX, max_path_bytes: usize::MAX, max_depth: usize::MAX };
    let diff = repo.diff_commits_bounded(Some(id(from)?), Some(id(to)?), &limits,
        &makepad_git::memory::MemoryAccount::new(usize::MAX), &|| false).map_err(|e| e.to_string())?;
    if !diff.complete { return Err(format!("incomplete checkout tree diff: {:?}", diff.reason)); }
    Ok(diff.records)
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
    /// The commit `r` names here (makepad-git: a node's caps read three of these every 10 s).
    pub fn rev(&self, r: &str) -> Option<String> {
        crate::gitread::store_rev(&self.git_dir, r)
    }

    pub fn has(&self, sha: &str) -> bool {
        valid_sha(sha) && self.git(&["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_ok()
    }

    pub fn is_ancestor(&self, a: &str, b: &str) -> bool {
        self.git(&["merge-base", "--is-ancestor", a, b]).is_ok()
    }

    /// The subjects of the commits `from..to` brought (merges left out: a landing's merge says
    /// nothing a person reads), newest first, at most `max`.
    pub fn subjects(&self, from: &str, to: &str, max: usize) -> Vec<String> {
        if !valid_sha(from) || !valid_sha(to) {
            return Vec::new();
        }
        self.git(&["log", "--no-merges", "--format=%s", &format!("-n{max}"), &format!("{from}..{to}")])
            .unwrap_or_default()
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect()
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
        let fetch = || self.git(&["fetch", "-q", "--no-tags", "--no-write-fetch-head", &b, &format!("+refs/heads/{branch}:{dst}")]);
        if let Err(e) = fetch() {
            // A ref of the store's own it cannot read (this branch's, or another the fetch names:
            // git reads them all) is only its copy of a master's branch: dropped, and the fetch
            // made again. A box's store that could not resolve one refused a lane's lease start
            // ("cannot lock ref ... unable to resolve reference", 11-13).
            let named = e.split(|c: char| c.is_whitespace() || matches!(c, '\'' | '"' | ':' | '!' | ',')).filter(|w| w.starts_with("refs/devhub/")).map(str::to_string);
            let broken: std::collections::BTreeSet<String> = std::iter::once(dst.clone()).chain(named).filter(|r| self.broken_ref(r)).collect();
            if broken.is_empty() {
                return Err(e);
            }
            for r in &broken {
                self.drop_ref(r);
            }
            let dropped = broken.into_iter().collect::<Vec<_>>().join(", ");
            fetch().map_err(|again| format!("{again} (after dropping {dropped}, which it could not read: {e})"))?;
            eprintln!("sync: {}: dropped {dropped}, which it could not read, and fetched again", self.git_dir.display());
        }
        self.rev(&dst).ok_or_else(|| format!("the bundle holds no branch {branch}"))
    }

    /// Whether `r` is there but names no commit the store holds: a loose ref file that is empty,
    /// not a sha, or a sha of a missing object (a crash mid-write), or a packed one of a missing
    /// object.
    fn broken_ref(&self, r: &str) -> bool {
        let loose = self.git_dir.join(r);
        let packed = std::fs::read_to_string(self.git_dir.join("packed-refs")).is_ok_and(|t| t.lines().any(|l| l.split_once(' ').is_some_and(|(_, n)| n == r)));
        (loose.is_file() || packed) && self.rev(r).is_none()
    }

    /// Drops `r` whatever it holds: its loose file, then its packed line (git refuses to delete a
    /// ref it cannot read).
    fn drop_ref(&self, r: &str) {
        let _ = std::fs::remove_file(self.git_dir.join(r));
        let _ = self.git(&["update-ref", "--no-deref", "-d", r]);
    }

    /// Drops `refs/devhub/pushed/<branch>` of each branch, in one git process: the master's word
    /// for branches that landed or went away (a node holds no state of its own).
    pub fn prune_pushed(&self, branches: &[&str]) -> Result<(), String> {
        let input: String = branches.iter().map(|b| format!("delete {PUSHED}{b}\n")).collect();
        run_with(None, Some(&self.git_dir), &[], &["update-ref", "--stdin"], Some(input.as_bytes())).map(|_| ())
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
        let root = if self.name() == "commercial" { work.parent().and_then(Path::parent).unwrap_or(work) } else { work };
        let exclude = self.git_dir.join("info/exclude");
        let mut ex = fs::read_to_string(&exclude).unwrap_or_default();
        let mut changed = false;
        for line in ["/FLEET-REVISION.txt", "/.devhub-move/"] {
            if !ex.lines().any(|l| l == line) {
                if !ex.is_empty() && !ex.ends_with('\n') { ex.push('\n'); }
                ex.push_str(line);
                ex.push('\n');
                changed = true;
            }
        }
        if changed {
            fs::create_dir_all(self.git_dir.join("info")).map_err(|e| e.to_string())?;
            fs::write(&exclude, ex).map_err(|e| e.to_string())?;
        }
        let dotgit = work.join(".git");
        if let Some(admin) = self.admin_of(work) {
            if admin.join("HEAD").exists() {
                return crate::ci_merge::install_merge_driver(work, root);
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
        self.read_index(work, at)?;
        crate::ci_merge::install_merge_driver(work, root)
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
    pub(crate) fn admin_of(&self, work: &Path) -> Option<PathBuf> {
        let text = std::fs::read_to_string(work.join(".git")).ok()?;
        let dir = PathBuf::from(text.trim().strip_prefix("gitdir:")?.trim());
        let common = std::fs::canonicalize(&self.git_dir).ok()?;
        std::fs::canonicalize(&dir).ok().filter(|d| d.starts_with(common.join("worktrees")))
    }

    /// Move only changed files, using verified, fsynced copies renamed over the old
    /// ones. A durable marker lets the next move repair an interrupted checkout. Its error says
    /// which step met it: a node's sync once read only "IO error: No such file or directory"
    /// (11-16), naming neither step nor file.
    pub fn move_checkout(&self, work: &Path, from: Option<&str>, to: &str) -> Result<Moved, String> {
        let short = |s: &str| s.get(..10).unwrap_or(s).to_string();
        self.move_steps(work, from, to).map_err(|(step, e)| format!("{step} ({} -> {}): {e}", from.map_or("(new)".into(), short), short(to)))
    }

    fn move_steps(&self, work: &Path, from: Option<&str>, to: &str) -> Result<Moved, (&'static str, String)> {
        let at = |step: &'static str| move |e: String| (step, e);
        let Some(from) = from else {
            self.link(work, to).map_err(at("linking the worktree"))?;
            self.set_ref(CHECKOUT_REF, to).map_err(at("its checkout ref"))?;
            return Ok(Moved::default());
        };
        let repaired = self.repair(work, from).map_err(at("repairing a cut-short move"))?;
        if from == to { return Ok(Moved { files: Vec::new(), repaired }); }
        let p = crate::gitread::paths(work).ok_or(("reading the worktree", "not a worktree".to_string()))?;
        let config = text_clean::conversion_config(&p).map_err(at("reading its config"))?;
        let delta = crate::gitread::with_common(&self.git_dir, |repo| {
            let delta = changes(repo, from, to)?;
            let mut before = Revision::read(repo, from)?;
            let index = makepad_git::index::read_index_for_update(&p.git_dir).map_err(|e| e.to_string())?;
            let indexed: HashMap<_, _> = index.entries.iter().map(|e| (e.path.as_str(), e)).collect();
            // Direct callers have the same edit protection as apply's check_move.
            for d in &delta {
                let entry = before.entry(&d.path);
                let staged = indexed.get(d.path.as_str());
                let disk = Disk::read(work, &d.path)?;
                let empty_dir = d.old.is_none() && matches!(disk, Disk::Directory)
                    && fs::read_dir(checked(work, &d.path)?).map_err(|e| e.to_string())?.next().is_none();
                let mut clean = empty_dir || disk.matches(&entry, &mut before, repo, &p.common_dir, &config)?;
                if !empty_dir && matches!(disk, Disk::Directory) && matches!(d.new, Some((_, 0o100644 | 0o100755 | 0o120000))) {
                    let mut after = Revision::read(repo, to)?;
                    clean &= directory_copy(work, &d.path, &mut before, &mut after, repo, &p.common_dir, &config)?;
                }
                if staged.map(|e| (e.oid, e.mode)) != d.old || staged.is_some_and(|e| e.stage() != 0) || !clean {
                    return Err(format!("{}: holds edits its move would overwrite", d.path));
                }
            }
            Ok(delta)
        }).ok_or("cannot read shared object store".to_string()).and_then(|r| r).map_err(at("checking the disk against the move"))?;
        let files = delta.iter().map(|d| d.path.clone()).collect();
        let entries: Vec<_> = delta.into_iter().map(|d| Entry { path: d.path, value: d.new }).collect();
        self.mark_move(work, from, to).map_err(at("marking the move"))?;
        self.write_paths(work, to, &entries).map_err(at("writing its files and index"))?;
        let oid = ObjectId::from_hex(to).map_err(|e| ("its commit", e.to_string()))?;
        makepad_git::refs::update_head_durable(&p.git_dir, &makepad_git::RefTarget::Direct(oid)).map_err(|e| ("its HEAD", e.to_string()))?;
        makepad_git::refs::write_ref_durable(&p.common_dir, CHECKOUT_REF, &oid).map_err(|e| ("its checkout ref", e.to_string()))?;
        self.unmark_move(work).map_err(at("unmarking the move"))?;
        Ok(Moved { files, repaired })
    }

    fn mark_move(&self, work: &Path, from: &str, to: &str) -> Result<(), String> {
        let admin = self.admin_of(work).ok_or("not a devhub worktree")?;
        let tmp = admin.join("devhub-move.tmp");
        match fs::remove_file(&tmp) {
            Ok(()) => {},
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.to_string()),
        }
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&tmp).map_err(|e| e.to_string())?;
        file.write_all(format!("{from} {to}\n").as_bytes()).and_then(|_| file.sync_all()).map_err(|e| e.to_string())?;
        drop(file);
        fs::rename(tmp, admin.join("devhub-move")).map_err(|e| e.to_string())?;
        sync_dir(&admin)
    }

    fn unmark_move(&self, work: &Path) -> Result<(), String> {
        let admin = self.admin_of(work).ok_or("not a devhub worktree")?;
        fs::remove_file(admin.join("devhub-move")).map_err(|e| e.to_string())?;
        sync_dir(&admin)
    }

    fn write_paths(&self, work: &Path, rev: &str, entries: &[Entry]) -> Result<(), String> {
        let admin = self.admin_of(work).ok_or("not a devhub worktree")?;
        let mut index = makepad_git::index::read_index_for_update(&admin).map_err(|e| e.to_string())?;
        let p = crate::gitread::paths(work).ok_or("not a worktree")?;
        let config = text_clean::conversion_config(&p)?;
        remove_staging(work)?;
        let staging = work.join(".devhub-move");
        // All reads, writes and verification finish before any destination changes.
        let stage = crate::gitread::with_common(&self.git_dir, |repo| {
            let mut revision = Revision::read(repo, rev)?;
            fs::create_dir(&staging).map_err(|e| e.to_string())?;
            for entry in entries {
                let copy = checked(&staging, &entry.path)?;
                let Some((oid, mode)) = entry.value else { continue };
                if mode == 0o160000 { continue; }
                if !matches!(mode, 0o100644 | 0o100755 | 0o120000) { return Err(format!("{}: unsupported mode {mode:o}", entry.path)); }
                let raw = repo.read_blob(&oid).map_err(|e| format!("{}: {e}", entry.path))?;
                verify_blob(&entry.path, oid, &raw)?;
                let conversion = if mode == 0o120000 { Conversion::default() } else { revision.conversion(repo, &p.common_dir, &entry.path)? };
                let bytes = if mode == 0o120000 { raw.clone() } else { text_clean::smudge(&raw, oid, &conversion, &config).map_err(|e| format!("{}: {e}", entry.path))? };
                fs::create_dir_all(copy.parent().unwrap()).map_err(|e| e.to_string())?;
                if mode == 0o120000 && cfg!(unix) {
                    #[cfg(unix)] {
                        use std::os::unix::ffi::OsStrExt;
                        std::os::unix::fs::symlink(std::ffi::OsStr::from_bytes(&raw), &copy).map_err(|e| format!("{}: {e}", entry.path))?;
                    }
                } else {
                    let mut options = fs::OpenOptions::new();
                    options.write(true).create_new(true);
                    #[cfg(unix)] {
                        use std::os::unix::fs::OpenOptionsExt;
                        options.mode(if mode == 0o100755 { 0o755 } else { 0o644 });
                    }
                    let mut file = options.open(&copy).map_err(|e| format!("{}: {e}", entry.path))?;
                    file.write_all(&bytes).and_then(|_| file.sync_all()).map_err(|e| format!("{}: {e}", entry.path))?;
                }
                verify_staged(&copy, &entry.path, oid, mode, &raw, &bytes, &conversion, &config)?;
            }
            Ok(())
        }).ok_or_else(|| "cannot read shared object store".to_string()).and_then(|r| r);
        if let Err(e) = stage {
            let cleanup = remove_staging(work);
            return Err(match cleanup { Ok(()) => e, Err(c) => format!("{e}; staging cleanup: {c}") });
        }
        let mut dirs = BTreeSet::new();
        for entry in entries.iter().filter(|e| e.value.is_none()) {
            let path = checked(work, &entry.path)?;
            match fs::symlink_metadata(&path) {
                Ok(m) if m.is_dir() => {
                    // A populated gitlink (or a directory replacing a deleted file) stays.
                    let _ = fs::remove_dir(&path);
                }
                Ok(_) => fs::remove_file(&path).map_err(|e| format!("{}: {e}", entry.path))?,
                Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {},
                Err(e) => return Err(format!("{}: {e}", entry.path)),
            }
            let mut parent = path.parent().unwrap();
            dirs.insert(parent.to_path_buf());
            while parent != work && parent.starts_with(work) && fs::remove_dir(parent).is_ok() {
                parent = parent.parent().unwrap();
                dirs.insert(parent.to_path_buf());
            }
        }
        for entry in entries {
            let Some((_, mode)) = entry.value else { continue };
            let path = checked(work, &entry.path)?;
            let parent = path.parent().unwrap();
            fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", entry.path))?;
            for dir in parent.ancestors().take_while(|d| d.starts_with(work)) { dirs.insert(dir.to_path_buf()); }
            if mode == 0o160000 {
                if fs::symlink_metadata(&path).is_ok_and(|m| !m.is_dir()) {
                    fs::remove_file(&path).map_err(|e| format!("{}: {e}", entry.path))?;
                }
                fs::create_dir_all(&path).map_err(|e| format!("{}: {e}", entry.path))?;
                dirs.insert(path);
                continue;
            }
            if fs::symlink_metadata(&path).is_ok_and(|m| m.is_dir()) {
                fs::remove_dir(&path).map_err(|_| format!("{}: a directory with other files is where {rev} has a file", entry.path))?;
            }
            let copy = checked(&staging, &entry.path)?;
            dirs.insert(copy.parent().unwrap().to_path_buf());
            fs::rename(copy, &path).map_err(|e| format!("{}: {e}", entry.path))?;
        }
        // Children first; removed directories have no entries left to sync.
        let mut dirs: Vec<_> = dirs.into_iter().collect();
        dirs.sort_by_key(|p| std::cmp::Reverse(p.components().count()));
        for dir in dirs {
            match fs::symlink_metadata(&dir) {
                Ok(m) if m.is_dir() => sync_dir(&dir)?,
                Ok(_) => {},
                Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {},
                Err(e) => return Err(format!("{}: {e}", dir.display())),
            }
        }
        let touched: BTreeSet<_> = entries.iter().map(|e| e.path.as_str()).collect();
        index.entries.retain(|e| !touched.contains(e.path.as_str()));
        for entry in entries {
            let Some((oid, mode)) = entry.value else { continue };
            let e = if mode == 0o160000 { zero_stat(entry.path.clone(), oid, mode) } else {
                let meta = fs::symlink_metadata(checked(work, &entry.path)?).map_err(|e| e.to_string())?;
                makepad_git::index_entry_from_metadata(entry.path.clone(), oid, mode, &meta)
            };
            index.entries.push(e);
        }
        index.entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()).then(a.stage().cmp(&b.stage())));
        makepad_git::index::write_index_durable(&admin, &index).map_err(|e| e.to_string())?;
        remove_staging(work)
    }

    /// Restore missing, empty or old/new-blob copies left by a marked move, and
    /// tracked nonempty blobs truncated to zero. Anything else is someone's content and stays.
    pub fn repair(&self, work: &Path, from: &str) -> Result<Vec<String>, String> {
        if !valid_sha(from) { return Err("invalid checkout commit".into()); }
        let marker = match self.admin_of(work) {
            Some(admin) => match fs::read_to_string(admin.join("devhub-move")) {
                Ok(s) => Some(s),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                Err(e) => return Err(e.to_string()),
            },
            None => None,
        };
        let mut repaired = Vec::new();
        if let Some(marker) = marker {
            let words: Vec<_> = marker.split_whitespace().collect();
            if words.len() != 2 || !words.iter().all(|w| valid_sha(w)) { return Err("invalid devhub move marker".into()); }
            let (f, t) = (words[0], words[1]);
            let admin = self.admin_of(work).ok_or("not a devhub worktree")?;
            match fs::read(admin.join("index")) {
                Ok(data) if !data.is_empty() => {
                    if makepad_git::index::is_split_or_sparse(&data).map_err(|e| e.to_string())? {
                        return Err("cannot repair a split or sparse index".into());
                    }
                }
                Ok(_) => {},
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
                Err(e) => return Err(e.to_string()),
            }
            let mut wanted = crate::gitread::with_common(&self.git_dir, |repo| Revision::read(repo, from)).ok_or("cannot read shared object store")??;
            let mut entries: Vec<_> = wanted.files.iter().map(|(path, &(oid, mode))| zero_stat(path.clone(), oid, mode)).collect();
            entries.sort_by(|a, b| a.path.as_bytes().cmp(b.path.as_bytes()));
            makepad_git::index::write_index_durable(&admin, &Index { version: 2, entries }).map_err(|e| e.to_string())?;
            // Prevent at_from from checking missing files out before they are classified.
            let oid = ObjectId::from_hex(from).map_err(|e| e.to_string())?;
            makepad_git::refs::update_head_durable(&admin, &makepad_git::RefTarget::Direct(oid)).map_err(|e| e.to_string())?;
            self.at_from(work, from)?;
            remove_staging(work)?;
            let p = crate::gitread::paths(work).ok_or("not a worktree")?;
            let config = text_clean::conversion_config(&p)?;
            let fixes = crate::gitread::with_common(&self.git_dir, |repo| {
                let mut old = Revision::read(repo, f)?;
                let mut new = Revision::read(repo, t)?;
                let mut fixes = Vec::new();
                for d in changes(repo, f, t)? {
                    let entry = wanted.entry(&d.path);
                    let disk = Disk::read(work, &d.path)?;
                    if disk.matches(&entry, &mut wanted, repo, &p.common_dir, &config)? { continue; }
                    let old_copy = d.old.is_some() && disk.matches(&old.entry(&d.path), &mut old, repo, &p.common_dir, &config)?;
                    let new_copy = d.new.is_some() && disk.matches(&new.entry(&d.path), &mut new, repo, &p.common_dir, &config)?;
                    let directory = matches!(disk, Disk::Directory) && entry.value.is_some()
                        && directory_copy(work, &d.path, &mut old, &mut new, repo, &p.common_dir, &config)?;
                    let short = &from[..8];
                    let line = match (&disk, entry.value) {
                        (Disk::Missing, Some(_)) => Some(format!("{}: missing -> {short}", d.path)),
                        (Disk::File(bytes, _), Some((oid, 0o100644 | 0o100755 | 0o120000))) if bytes.is_empty() && oid != makepad_git::oid::hash_object("blob", b"") => Some(format!("{}: 0 bytes on disk -> {short}", d.path)),
                        (_, None) if new_copy || old_copy => Some(format!("{}: an extra file of {} -> removed", d.path, if new_copy { &t[..8] } else { &f[..8] })),
                        (_, Some(_)) if directory => Some(format!("{}: the directory copy -> {short}", d.path)),
                        (_, Some(_)) if new_copy || old_copy => Some(format!("{}: the {} copy -> {short}", d.path, if new_copy { &t[..8] } else { &f[..8] })),
                        _ => None,
                    };
                    if let Some(line) = line { fixes.push(entry); repaired.push(line); }
                }
                Ok::<_, String>(fixes)
            }).ok_or("cannot read shared object store")??;
            if !fixes.is_empty() { self.write_paths(work, from, &fixes)?; }
            // at_from may have replaced the fresh index while refreshing its stats.
            File::open(admin.join("index")).and_then(|f| f.sync_all()).map_err(|e| e.to_string())?;
            sync_dir(&admin)?;
            self.unmark_move(work)?;
        } else {
            self.at_from(work, from)?;
        }
        let admin = self.admin_of(work).ok_or("not a devhub worktree")?;
        let index = makepad_git::index::read_index_for_update(&admin).map_err(|e| e.to_string())?;
        let mut empty = Vec::new();
        for entry in index.entries.iter().filter(|e| e.stage() == 0 && matches!(e.mode, 0o100644 | 0o100755) && e.file_size != 0) {
            // One lstat per eligible path. Nonempty files need no content read.
            match fs::symlink_metadata(work.join(&entry.path)) {
                Ok(m) if m.is_file() && m.len() == 0 => empty.push(entry.path.clone()),
                Ok(_) => {},
                Err(e) if matches!(e.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory) => {},
                Err(e) => return Err(format!("{}: {e}", entry.path)),
            }
        }
        if !empty.is_empty() {
            let fixes = crate::gitread::with_common(&self.git_dir, |repo| {
                let wanted = Revision::read(repo, from)?;
                let mut fixes = Vec::new();
                for path in empty {
                    let entry = wanted.entry(&path);
                    if let Some((oid, 0o100644 | 0o100755)) = entry.value {
                        let raw = repo.read_blob(&oid).map_err(|e| e.to_string())?;
                        verify_blob(&path, oid, &raw)?;
                        if !raw.is_empty() {
                            repaired.push(format!("{path}: 0 bytes on disk, {} bytes in {} -> restored", raw.len(), &from[..8]));
                            fixes.push(entry);
                        }
                    }
                }
                Ok::<_, String>(fixes)
            }).ok_or("cannot read shared object store")??;
            if !fixes.is_empty() { self.write_paths(work, from, &fixes)?; }
        }
        Ok(repaired)
    }

    /// Whether [`Self::move_checkout`] from `from` to `to` would go through: Err names the edits
    /// or untracked files the move would overwrite, which stay. Nothing in the tree changes.
    pub fn check_move(&self, work: &Path, from: &str, to: &str) -> Result<(), String> {
        self.at_from(work, from)?;
        run(Some(work), None, &[], &["read-tree", "-m", "-u", "-n", from, to]).map(|_| ())
    }

    /// The checkout `work` linked to this store with its HEAD and index at `from`, what its files
    /// hold, their stat data current: a file another process rewrote with the same content is no
    /// edit, and a move refuses only real edits it would change.
    fn at_from(&self, work: &Path, from: &str) -> Result<(), String> {
        let g = |args: &[&str]| run(Some(work), None, &[], args);
        self.link(work, from)?;
        let head = crate::gitread::rev(work, "HEAD").unwrap_or_default();
        if head != from {
            // the index describes HEAD: read it from `from` (what the tree holds) first
            let p = crate::gitread::paths(work).ok_or("not a worktree")?;
            let oid = ObjectId::from_hex(from).map_err(|e| e.to_string())?;
            makepad_git::refs::update_head_durable(&p.git_dir, &makepad_git::RefTarget::Direct(oid)).map_err(|e| e.to_string())?;
            self.read_index(work, from)?;
        }
        // the move's one git process: the stats of the whole index refreshed in one batch (exit 1
        // names the edits: the move refuses those it changes)
        let _ = g(&["update-index", "-q", "--refresh"]);
        Ok(())
    }

    /// Changes to tracked files in the tree. Untracked files do not count here: [`Store::clean_tree`]
    /// takes those.
    pub fn tree_dirty(&self, work: &Path) -> Result<bool, String> {
        run(Some(work), None, &[], &["status", "--porcelain", "--untracked-files=no", "--ignore-submodules=all"]).map(|o| !o.trim().is_empty())
    }

    /// Removes what this store's tree holds besides tracked and ignored files (whole stray dirs
    /// too): the paths removed. Ignored files (`target/`, the JIT cache, `local/`) stay, any dir named
    /// `target` stays whether its repo ignores it or not (the commercial repo's `.gitignore` is not
    /// the root's: a nested workspace keeps its warm target there), and so does a nested repository, which is someone's: Err names it, or whatever else could not go. A dir
    /// that is not this store's worktree is left alone (`git clean` there would walk up into
    /// whatever repository sits above it).
    pub fn clean_tree(&self, work: &Path) -> Result<Vec<String>, String> {
        if self.admin_of(work).is_none() {
            return Ok(Vec::new());
        }
        let out = run(Some(work), None, &[("LC_ALL", Path::new("C"))], &["-c", "core.quotePath=false", "clean", "-fd", "-e", "target/"])?;
        // a nested worktree of one of devhub's own stores (the lane tree's apps/commercial) is
        // part of the tree, not someone's repository: it is cleaned as its own store's tree
        let ours = |rel: &str| {
            std::fs::read_to_string(work.join(rel.trim_end_matches('/')).join(".git")).ok().and_then(|t| t.strip_prefix("gitdir: ").map(|g| PathBuf::from(g.trim()))).is_some_and(|g| g.starts_with(stores_dir()))
        };
        let kept: Vec<&str> = out.lines().filter_map(|l| l.strip_prefix("Skipping repository ")).filter(|r| !ours(r)).collect();
        if !kept.is_empty() {
            return Err(format!("nested repositories stay: {}", kept.join(" ")));
        }
        Ok(out.lines().filter_map(|l| l.strip_prefix("Removing ")).map(str::to_string).collect())
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
        g(&["bundle", "create", "-q", &b, "HEAD", "--not", "--glob=refs/*"])?;
        // Recovery moves this worktree's HEAD only, never the main checkout ref.
        g(&["reset", "-q", "--hard", at])?;
        Ok(bundle)
    }
}

/// The commits the deploy scripts recorded in `FLEET-REVISION.txt` (`makepad <sha>`,
/// `commercial <sha>`): where a first sync of a deployed tree starts from.
pub fn fleet_revision(checkout: &Path, repo: &str) -> Option<String> {
    let text = std::fs::read_to_string(checkout.join("FLEET-REVISION.txt")).ok()?;
    text.lines().find_map(|l| l.strip_prefix(&format!("{repo} ")).map(|s| s.trim().to_string())).filter(|s| valid_sha(s))
}

/// Every repository FLEET-REVISION.txt names (makepad's, and each nested one's): None without
/// makepad's.
pub fn fleet_pin(checkout: &Path) -> Option<crate::lease::Pin> {
    let text = std::fs::read_to_string(checkout.join("FLEET-REVISION.txt")).ok()?;
    let mut pin = crate::lease::Pin::at(&fleet_revision(checkout, "makepad")?);
    for (name, sha) in text.lines().filter_map(|l| l.split_once(' ')) {
        if crate::nested::valid(name) && valid_sha(sha.trim()) {
            pin.nested.insert(name.to_string(), sha.trim().to_string());
        }
    }
    Some(pin)
}

/// The deployment's commits, a `<repo> <sha>` line each, makepad first.
pub fn write_fleet_revision(checkout: &Path, pin: &crate::lease::Pin) {
    let mut text = String::new();
    for (name, sha, _) in pin.repos() {
        text.push_str(&format!("{name} {sha}\n"));
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

    #[cfg(unix)]
    #[test]
    fn a_failed_git_says_how_it_ended() {
        use std::os::unix::process::ExitStatusExt;
        let (killed, exited) = (std::process::ExitStatus::from_raw(9), std::process::ExitStatus::from_raw(128 << 8));
        assert_eq!(failure(killed, b""), "signal: 9 (SIGKILL)", "a signal leaves no stderr: its end is the reason");
        assert_eq!(failure(exited, b"fatal: bad object x\n\n  hint: y\n"), "fatal: bad object x | hint: y (exit status: 128)");
    }

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

    fn source() -> (makepad_git::test_support::TempDir, PathBuf) {
        let base = makepad_git::test_support::tempdir().unwrap();
        let src = base.path().join("src");
        fs::create_dir(&src).unwrap();
        sh(&src, &["init", "-q", "-b", "work"]);
        sh(&src, &["config", "core.autocrlf", "false"]);
        (base, src)
    }

    fn put(root: &Path, files: &[(&str, &str)]) {
        for (path, bytes) in files {
            let path = root.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
    }

    fn revision(src: &Path, message: &str) -> String {
        sh(src, &["add", "-A"]);
        sh(src, &["commit", "-q", "-m", message]);
        sh(src, &["rev-parse", "HEAD"]).trim().into()
    }

    fn linked(src: &Path, work: &Path, at: &str) -> Store {
        sh(src, &["worktree", "add", "-q", "--detach", &work.to_string_lossy(), at]);
        let store = Store { git_dir: src.join(".git") };
        store.link(work, at).unwrap();
        makepad_git::refs::write_ref_durable(&store.git_dir, CHECKOUT_REF, &ObjectId::from_hex(at).unwrap()).unwrap();
        store
    }

    #[test]
    fn a_durable_move_writes_modes_links_directories_and_new_attributes() {
        let (base, src) = source();
        put(&src, &[("modified", "before\n"), ("gone/file", "delete\n"), ("run", "script\n"), ("replace/child", "child\n"), ("unchanged", "keep\n"), ("run.bat", "old\n")]);
        let from = revision(&src, "before");
        put(&src, &[("modified", "after\n"), ("new/file", "added\n"), ("run.bat", "echo done\n"), (".gitattributes", "*.bat text eol=crlf\n")]);
        fs::remove_dir_all(src.join("gone")).unwrap();
        fs::remove_dir_all(src.join("replace")).unwrap();
        fs::write(src.join("replace"), b"now a file\n").unwrap();
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(src.join("run"), fs::Permissions::from_mode(0o755)).unwrap();
            std::os::unix::fs::symlink("modified", src.join("link")).unwrap();
        }
        let to = revision(&src, "after");
        let work = base.path().join("work");
        let store = linked(&src, &work, &from);
        let kept = mtime(&work.join("unchanged"));
        let spawned = crate::git_exec::on_this_thread();
        let m = store.move_checkout(&work, Some(&from), &to).unwrap();
        assert_eq!(crate::git_exec::on_this_thread() - spawned, 1, "one git process per move: the index refresh");
        let expected = sh(&src, &["diff", "--name-only", "--no-renames", &from, &to]);
        assert_eq!(m.files, expected.lines().collect::<Vec<_>>(), "the manifest is every changed leaf");
        assert!(m.repaired.is_empty(), "a clean move has no repairs");
        assert_eq!(fs::read(work.join("modified")).unwrap(), b"after\n", "modified file replaced");
        assert_eq!(fs::read(work.join("new/file")).unwrap(), b"added\n", "new parent created");
        assert!(!work.join("gone").exists(), "the deleted file's empty parent is gone");
        assert_eq!(fs::read(work.join("replace")).unwrap(), b"now a file\n", "directory replaced by file");
        assert_eq!(fs::read(work.join("run.bat")).unwrap(), b"echo done\r\n", "new tree attributes govern checkout");
        assert_eq!(mtime(&work.join("unchanged")), kept, "unchanged mtimes stay warm");
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(work.join("run")).unwrap().permissions().mode() & 0o777, 0o755, "executable mode written");
            assert_eq!(fs::read_link(work.join("link")).unwrap(), Path::new("modified"), "symlink target written");
        }
        assert!(sh(&work, &["status", "--porcelain"]).is_empty(), "the final index and worktree are clean");
        let admin = store.admin_of(&work).unwrap();
        assert!(!work.join(".devhub-move").exists(), "staging removed");
        assert!(!admin.join("devhub-move").exists(), "marker removed");
        assert_eq!(makepad_git::refs::resolve_head_in(&admin, &store.git_dir).unwrap().to_hex(), to, "HEAD published");
        assert_eq!(store.rev(CHECKOUT_REF).as_deref(), Some(to.as_str()), "checkout ref published");
        store.move_checkout(&work, Some(&to), &from).unwrap();
        assert_eq!(fs::read(work.join("replace/child")).unwrap(), b"child\n", "reverse move replaces file by directory");
        assert!(sh(&work, &["status", "--porcelain"]).is_empty(), "the reverse move is clean too");
        let entries = crate::gitread::with_common(&store.git_dir, |repo| changes(repo, &from, &to)).unwrap().unwrap()
            .into_iter().map(|d| Entry { path: d.path, value: d.new }).collect::<Vec<_>>();
        store.mark_move(&work, &from, &to).unwrap();
        let spawned = crate::git_exec::on_this_thread();
        store.write_paths(&work, &to, &entries).unwrap();
        assert_eq!(crate::git_exec::on_this_thread(), spawned, "staging, verification and the durable index are in-process");
        let repairs = store.repair(&work, &from).unwrap();
        assert!(repairs.iter().any(|l| l.starts_with("run.bat:")), "repair recognizes the target's converted bytes");
        assert!(sh(&work, &["status", "--porcelain"]).is_empty(), "repair reverses file/directory and mode changes too");
    }

    #[test]
    fn bad_staged_copies_and_blob_reads_do_not_touch_the_destination() {
        let dir = makepad_git::test_support::tempdir().unwrap();
        let raw = b"complete contents\n";
        let oid = makepad_git::oid::hash_object("blob", raw);
        let destination = dir.path().join("file");
        fs::write(&destination, b"old bytes").unwrap();
        let staging = dir.path().join(".devhub-move");
        fs::create_dir(&staging).unwrap();
        let copy = staging.join("file");
        let corrupt = vec![b'x'; raw.len()];
        for bytes in [Some(&raw[..3]), Some(corrupt.as_slice()), Some(&b""[..]), None] {
            match bytes {
                Some(bytes) => fs::write(&copy, bytes).unwrap(),
                None => fs::remove_file(&copy).unwrap(),
            }
            let err = verify_staged(&copy, "file", oid, 0o100644, raw, raw, &Conversion::default(), "").unwrap_err();
            assert!(err.contains("file:") && err.contains("staged copy") && err.contains("nothing moved"), "verification names the failed path: {err}");
            assert_eq!(fs::read(&destination).unwrap(), b"old bytes", "verification never changes the destination");
        }
        let err = verify_blob("file", oid, b"wrong").unwrap_err();
        assert!(err.contains("file:") && err.contains("5 bytes") && err.contains("nothing moved"), "a short blob read is named: {err}");
        assert_eq!(fs::read(&destination).unwrap(), b"old bytes", "bad object bytes never change the destination");
    }

    #[test]
    fn a_cut_short_move_restores_only_recognizable_debris() {
        let (base, src) = source();
        let original = [("changed", "old changed\n"), ("empty", "old empty\n"), ("missing", "old missing\n"), ("edited", "old edited\n")];
        put(&src, &original);
        let from = revision(&src, "before");
        put(&src, &[("changed", "new changed\n"), ("empty", "new empty\n"), ("missing", "new missing\n"), ("edited", "new edited\n"), ("added", "new added\n")]);
        let to = revision(&src, "interrupted");
        put(&src, &[("changed", "next changed\n"), ("edited", "next edited\n")]);
        let next = revision(&src, "next");
        let work = base.path().join("work");
        let store = linked(&src, &work, &from);
        store.mark_move(&work, &from, &to).unwrap();
        put(&work, &[("changed", "new changed\n"), ("empty", ""), ("added", "new added\n"), ("edited", "someone's content\n")]);
        fs::remove_file(work.join("missing")).unwrap();
        fs::create_dir(work.join(".devhub-move")).unwrap();
        fs::write(work.join(".devhub-move/leftover"), b"partial").unwrap();
        let lines = store.repair(&work, &from).unwrap();
        assert_eq!(lines.len(), 4, "only the four debris paths are reported: {lines:?}");
        for name in ["changed", "empty", "missing", "added"] {
            assert!(lines.iter().any(|l| l.starts_with(&format!("{name}:"))), "repair reports {name}");
        }
        for (name, bytes) in &original[..3] {
            assert_eq!(fs::read(work.join(name)).unwrap(), bytes.as_bytes(), "{name} restored to the from tree");
        }
        assert!(!work.join("added").exists(), "the interrupted addition is removed");
        assert!(!work.join(".devhub-move").exists(), "leftover staging removed");
        assert_eq!(fs::read(work.join("edited")).unwrap(), b"someone's content\n", "unrecognized content stays");
        assert!(store.check_move(&work, &from, &next).unwrap_err().contains("edited"), "preflight still names the real edit");
        let e = store.move_checkout(&work, Some(&from), &next).unwrap_err();
        assert!(e.contains("edited") && e.starts_with("checking the disk against the move ("), "a direct move also protects the edit, and says which step: {e}");
        fs::write(work.join("edited"), b"old edited\n").unwrap();
        store.move_checkout(&work, Some(&from), &next).unwrap();
        assert!(sh(&work, &["status", "--porcelain"]).is_empty(), "the later move reaches its own target cleanly");
        assert_eq!(fs::read(work.join("changed")).unwrap(), b"next changed\n", "the later target, not the interrupted one");
        assert!(!store.admin_of(&work).unwrap().join("devhub-move").exists(), "no marker remains");
    }

    #[test]
    fn an_unmarked_empty_file_and_a_lost_index_are_repaired() {
        let (base, src) = source();
        put(&src, &[("file", "before\n"), ("untouched", "stay\n")]);
        let from = revision(&src, "before");
        put(&src, &[("file", "after\n")]);
        let to = revision(&src, "after");
        let work = base.path().join("work");
        let store = linked(&src, &work, &from);
        fs::write(work.join("file"), b"").unwrap();
        let lines = store.repair(&work, &from).unwrap();
        assert_eq!(lines, [format!("file: 0 bytes on disk, 7 bytes in {} -> restored", &from[..8])], "unmarked truncation is reported");
        assert_eq!(fs::read(work.join("file")).unwrap(), b"before\n", "unmarked truncation is restored");
        assert!(sh(&work, &["status", "--porcelain"]).is_empty(), "unmarked repair refreshes the index");
        assert!(store.repair(&work, &from).unwrap().is_empty(), "repair is idempotent");
        store.mark_move(&work, &from, &to).unwrap();
        let admin = store.admin_of(&work).unwrap();
        fs::write(admin.join("index"), b"").unwrap();
        fs::write(work.join("file"), b"").unwrap();
        let lines = store.repair(&work, &from).unwrap();
        assert_eq!(lines.len(), 1, "a lost index does not hide the damaged file");
        assert!(lines[0].starts_with("file:"), "lost-index recovery names the file");
        assert_eq!(makepad_git::index::read_index(&admin).unwrap().entries.len(), 2, "the complete from index is rebuilt");
        store.move_checkout(&work, Some(&from), &to).unwrap();
        assert_eq!(fs::read(work.join("file")).unwrap(), b"after\n", "the move after index recovery succeeds");
        assert!(sh(&work, &["status", "--porcelain"]).is_empty(), "recovered index matches the final tree");
        assert!(!admin.join("devhub-move").exists(), "recovery removes the marker");
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
        // a landed lane's ref goes (a name the store never had is no error); work's stays
        store.prune_pushed(&["rc-x", "rc-never"]).unwrap();
        assert_eq!(store.refs().iter().map(|(r, _)| r.as_str()).collect::<Vec<_>>(), vec!["refs/devhub/pushed/work"]);
        assert_eq!(store.fetch_bundle(&bundle, "rc-x").unwrap(), lane);
        // a ref the store cannot read (empty, not a sha, a missing object) is dropped and fetched again
        for broken in ["", "garbage\n", "0123456789012345678901234567890123456789\n"] {
            std::fs::write(stores.join("makepad.git/refs/devhub/pushed/rc-x"), broken).unwrap();
            assert_eq!(store.fetch_bundle(&bundle, "rc-x").unwrap(), lane, "{broken:?}");
        }
        // as is another one of its refs the fetch trips on
        std::fs::write(stores.join("makepad.git/refs/devhub/pushed/work"), "garbage\n").unwrap();
        assert_eq!(store.fetch_bundle(&bundle, "rc-x").unwrap(), lane, "another broken ref");
        assert_eq!(store.fetch_bundle(&bundle, "work").unwrap(), two);

        // a tree shipped at `one` by a deploy script: no .git, no dotfiles, a target and FLEET-REVISION
        std::fs::create_dir_all(work.join("target")).unwrap();
        std::fs::write(work.join("a.txt"), "a1").unwrap();
        std::fs::write(work.join("b.txt"), "b1").unwrap();
        std::fs::write(work.join("target/x.rlib"), "bin").unwrap();
        write_fleet_revision(&work, &crate::lease::Pin::at(&one));
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
        // what a node's caps read every 10 s starts no git
        let spawned = crate::git_exec::on_this_thread();
        assert_eq!(store.rev(CHECKOUT_REF).as_deref(), Some(two.as_str()));
        assert_eq!(store.rev("refs/devhub/never"), None, "a ref the store lacks");
        assert_eq!(crate::lease::tree::current(&work).map(|p| p.makepad), Some(two.clone()), "the tree's HEAD");
        assert_eq!(crate::git_exec::on_this_thread(), spawned, "the caps reads are makepad-git's");

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

        // a local edit in a file the move changes: refused, nothing moves (said before any move:
        // a deployment's other repositories wait with it); an edit the move leaves alone stays
        std::fs::write(work.join("b.txt"), "local edit").unwrap();
        assert!(store.check_move(&work, &lane, &two).unwrap_err().contains("b.txt"));
        assert_eq!(std::fs::read_to_string(work.join("b.txt")).unwrap(), "local edit");
        assert!(store.move_checkout(&work, Some(&lane), &two).is_err());
        assert_eq!(std::fs::read_to_string(work.join("b.txt")).unwrap(), "local edit");
        assert_eq!(store.rev(CHECKOUT_REF).as_deref(), Some(lane.as_str()));
        std::fs::write(work.join("b.txt"), "b-lane").unwrap();
        store.check_move(&work, &lane, &two).unwrap();
        store.move_checkout(&work, Some(&lane), &two).unwrap();
        assert_eq!(std::fs::read_to_string(work.join("b.txt")).unwrap(), "b1");
        std::fs::write(work.join("b.txt"), "kept").unwrap();
        store.check_move(&work, &two, &one).unwrap();
        store.move_checkout(&work, Some(&two), &one).unwrap();
        assert_eq!((std::fs::read_to_string(work.join("a.txt")).unwrap(), std::fs::read_to_string(work.join("b.txt")).unwrap()), ("a1".to_string(), "kept".to_string()));
        std::fs::write(work.join("b.txt"), "b1").unwrap();

        // a fresh tree in an empty dir; a clone of its own is never taken
        let fresh = base.join("fresh");
        store.move_checkout(&fresh, None, &lane).unwrap();
        assert_eq!(std::fs::read_to_string(fresh.join("b.txt")).unwrap(), "b-lane");
        assert!(store.link(&src, &one).unwrap_err().contains("of its own"));
        let _ = std::fs::remove_dir_all(&base);
    }
}
