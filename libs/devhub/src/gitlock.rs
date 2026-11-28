//! Git's lock files (`<file>.lock`, created exclusively, renamed into place by their writer) that
//! a writer left when it died. A node hub the WM replaced in the middle of a lease start left
//! `config.worktree.lock` in a lane tree's git dir, and every later lease start in that tree was
//! refused on it ("ci merge config: ... is locked", 11-13). A lease start clears such a lock and
//! says so in one line; a lock a live process holds open is waited out, never taken from it.
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

/// Between two looks at a held lock.
const LOOK: Duration = Duration::from_millis(100);
/// A lock no process holds goes once it is this old: a younger one's writer may be between
/// closing it and renaming it into place.
const SETTLE: Duration = Duration::from_millis(200);

/// The processes that hold `path` open (Linux: `/proc/*/fd`, macOS: `lsof`); None where this
/// platform cannot tell (Windows: there a removal fails while a writer holds the file).
pub fn holders(path: &Path) -> Option<Vec<u32>> {
    #[cfg(target_os = "linux")]
    {
        let want = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        let mut pids = Vec::new();
        for e in std::fs::read_dir("/proc").ok()?.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let Ok(fds) = std::fs::read_dir(e.path().join("fd")) else { continue };
            if fds.flatten().any(|fd| std::fs::read_link(fd.path()).is_ok_and(|t| t == want)) {
                pids.push(pid);
            }
        }
        Some(pids)
    }
    #[cfg(target_os = "macos")]
    {
        let o = makepad_remote::group::command(crate::os::tool("lsof")).arg("-t").arg("--").arg(path).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
        // (exit 1 and nothing said: no process has it open)
        Some(String::from_utf8_lossy(&o.stdout).lines().filter_map(|l| l.trim().parse().ok()).collect())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = path;
        None
    }
}

fn stamp(p: &Path) -> Option<(u64, SystemTime)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.len(), m.modified().ok()?))
}

fn secs(d: Duration) -> String {
    format!("{:.1} s", d.as_secs_f64())
}

/// `lock` once no live process holds it: a lock left by a writer that is gone (no process has it
/// open, and it is [`SETTLE`] old) is removed; one a process holds is waited for until its writer
/// renames it into place or goes, said on stderr when the wait starts. The line says what was
/// found and done; None when there was no lock. One that cannot be removed is said, and left to
/// the git that meets it.
pub fn settle(lock: &Path) -> Option<String> {
    let t = Instant::now();
    let mut waited: Vec<u32> = Vec::new();
    let waited_said = |waited: &[u32]| if waited.is_empty() { String::new() } else {
        format!(" after waiting {} for pid {}", secs(t.elapsed()), waited.iter().map(u32::to_string).collect::<Vec<_>>().join(" "))
    };
    loop {
        let Some(before) = stamp(lock) else {
            return (!waited.is_empty()).then(|| format!("{}: waited {} for pid {} holding it; its writer finished", lock.display(), secs(t.elapsed()), waited.iter().map(u32::to_string).collect::<Vec<_>>().join(" ")));
        };
        let known = holders(lock);
        if let Some(pids) = known.as_ref().filter(|p| !p.is_empty()) {
            if waited.is_empty() {
                eprintln!("{}: held by pid {}: waiting for it", lock.display(), pids.iter().map(u32::to_string).collect::<Vec<_>>().join(" "));
                #[cfg(test)]
                tests::SAW_HOLDER.with(|tx| {
                    if let Some(tx) = tx.borrow().as_ref() { let _ = tx.send(()); }
                });
            }
            for p in pids { if !waited.contains(p) { waited.push(*p); } }
            std::thread::sleep(LOOK);
            continue;
        }
        // (a clock that went back: old enough)
        let age = before.1.elapsed().unwrap_or(SETTLE);
        if age < SETTLE {
            std::thread::sleep(SETTLE - age);
            continue;
        }
        match std::fs::remove_file(lock) {
            Ok(()) => return Some(format!("{}: cleared a stale lock ({} bytes, {} old): no process held it, its writer gone{}", lock.display(), before.0, secs(age), waited_said(&waited))),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            // (where no holder can be named, a writer that holds it makes its removal fail)
            Err(_) if known.is_none() => std::thread::sleep(LOOK),
            Err(e) => return Some(format!("{}: a stale lock (no process holds it) that could not be removed: {e}", lock.display())),
        }
    }
}

/// A git that started this long after a lock was written is still counted as its possible
/// writer: Linux gives a process's start in clock ticks after a boot time of whole seconds.
const START_SLACK: Duration = Duration::from_secs(2);

/// The live gits of this box (`git` and its `git-*` helpers), each with when it started; None
/// where this platform cannot tell.
fn gits() -> Option<Vec<(u32, SystemTime)>> {
    let is_git = |name: &str| name == "git" || name.starts_with("git-");
    #[cfg(target_os = "linux")]
    {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        let boot = stat.lines().find_map(|l| l.strip_prefix("btime ")).and_then(|v| v.trim().parse::<u64>().ok())?;
        let tick = crate::top::ticks_per_s();
        let mut out = Vec::new();
        for e in std::fs::read_dir("/proc").ok()?.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
            let Ok(comm) = std::fs::read_to_string(e.path().join("comm")) else { continue };
            if !is_git(comm.trim()) { continue; }
            // (gone, or a zombie: no writer)
            let Some(ticks) = makepad_remote::relay::proc_start(pid) else { continue };
            out.push((pid, SystemTime::UNIX_EPOCH + Duration::from_secs(boot) + Duration::from_secs_f64(ticks as f64 / tick)));
        }
        Some(out)
    }
    #[cfg(target_os = "macos")]
    {
        // (macOS: microseconds since the epoch)
        Some(crate::top::named().into_iter().filter(|p| is_git(&p.name))
            .filter_map(|p| makepad_remote::relay::proc_start(p.pid).map(|us| (p.pid, SystemTime::UNIX_EPOCH + Duration::from_micros(us))))
            .collect())
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = is_git;
        None
    }
}

/// A checkout's `index.lock` (in its git dir) that its writer left when it died, cleared: no
/// process holds it open, and no git that runs on the box started before it was written. Not
/// [`settle`]'s rule alone: `git commit` holds it closed while its hooks run, but that git started
/// before it. Ok(None): no lock. Err: one that a writer may still hold, kept, and why (the next
/// look judges it again). A node hub restarted in the middle of its apply's index write left
/// `index.lock` in its deployment tree's git dir, and every later apply waited on it, the box's
/// self-update too, for 9 h (11-19).
pub fn clear_left_index(git_dir: &Path) -> Result<Option<String>, String> {
    clear_left(&git_dir.join("index.lock"), &gits)
}

/// A lease tree's `index.lock` that its writer left when it died, cleared as its lease starts
/// ([`clear_left`]): no process holds it open, and no git that runs in the tree, its git dir or
/// its store ([`in_tree`]) started before it was written. A git elsewhere on the box writes no
/// index of this tree's. A git killed in its `add` left one in a tree's nested checkout, and every
/// lease start there was refused on it (`git add: ... index.lock: File exists`, 11-26). Ok(None):
/// no lock, or no checkout. Err: one a writer may still hold, kept, and why.
pub fn clear_left_index_in(tree: &Path) -> Result<Option<String>, String> {
    let Ok(Some(paths)) = makepad_git::repository_paths(tree) else { return Ok(None) };
    let roots: Vec<PathBuf> = [tree, &paths.git_dir, &paths.common_dir].iter().map(|r| r.canonicalize().unwrap_or_else(|_| r.to_path_buf())).collect();
    clear_left(&paths.git_dir.join("index.lock"), &|| gits().map(|g| in_tree(g, &roots, &cwd)))
}

/// Of `gits`, those that may run in one of `roots`: their working directory is in one, or this
/// platform cannot tell it.
fn in_tree(gits: Vec<(u32, SystemTime)>, roots: &[PathBuf], cwd: &dyn Fn(u32) -> Option<PathBuf>) -> Vec<(u32, SystemTime)> {
    gits.into_iter().filter(|(pid, _)| cwd(*pid).is_none_or(|c| roots.iter().any(|r| c.starts_with(r)))).collect()
}

/// A process's working directory (Linux: `/proc/<pid>/cwd`); None where this platform cannot tell.
fn cwd(pid: u32) -> Option<PathBuf> {
    #[cfg(target_os = "linux")]
    { std::fs::read_link(format!("/proc/{pid}/cwd")).ok() }
    #[cfg(not(target_os = "linux"))]
    { let _ = pid; None }
}

fn clear_left(lock: &Path, gits: &dyn Fn() -> Option<Vec<(u32, SystemTime)>>) -> Result<Option<String>, String> {
    let Some((len, at)) = stamp(lock) else { return Ok(None) };
    let kept = |why: String| Err(format!("{} is kept: {why}", lock.display()));
    // (a clock that went back: old enough)
    let age = at.elapsed().unwrap_or(SETTLE);
    if age < SETTLE {
        return kept(format!("written {} ago, its writer may be renaming it", secs(age)));
    }
    let Some(held) = holders(lock) else { return kept("this platform cannot tell who holds it".into()) };
    if !held.is_empty() {
        return kept(format!("pid {} holds it open", held.iter().map(u32::to_string).collect::<Vec<_>>().join(" ")));
    }
    let Some(gits) = gits() else { return kept("this platform cannot tell which gits run".into()) };
    let older: Vec<String> = gits.iter().filter(|(_, started)| *started <= at + START_SLACK).map(|(pid, _)| pid.to_string()).collect();
    if !older.is_empty() {
        return kept(format!("git pid {} started before it was written and may still write it", older.join(" ")));
    }
    match std::fs::remove_file(lock) {
        Ok(()) => Ok(Some(format!("{}: cleared a stale lock ({len} bytes, {} old): no process held it, and no git that runs here started before it", lock.display(), secs(age)))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: a stale lock that could not be removed: {e}", lock.display())),
    }
}

/// The locks [`sweep`] settles: a config write's, which its writer (`git config`, a hub's own
/// config writes) renames into place with nothing run between. Never `index.lock`, `HEAD.lock` or a
/// ref's: `git commit` closes `index.lock` before it runs its hooks (the pre-commit scanner, seconds)
/// and renames it after, so one no process holds open can be a live commit's, in this tree or in
/// another worktree of its store.
const SWEPT: &[&str] = &["config.lock", "config.worktree.lock"];

/// The config locks ([`SWEPT`]) in `git_dir` settled as [`settle`] does; a line for each one there
/// was.
pub fn sweep(git_dir: &Path) -> Vec<String> {
    SWEPT.iter().map(|name| git_dir.join(name)).filter(|p| p.is_file()).filter_map(|l| settle(&l)).collect()
}

/// [`sweep`] of a worktree's own git dir and of the repository's shared one.
pub fn sweep_worktree(tree: &Path) -> Vec<String> {
    let Ok(Some(paths)) = makepad_git::repository_paths(tree) else { return Vec::new() };
    let mut said = sweep(&paths.git_dir);
    if paths.common_dir != paths.git_dir { said.extend(sweep(&paths.common_dir)); }
    said
}

/// A shared repository's maintenance is devhub's, never the writing command's: git's auto
/// maintenance after a commit packs every branch, then deletes each packed loose one under a lock
/// that expects the value it packed. Many lanes commit in one repository while the merge bot and
/// the landings move their branches, so a lane's plain `git commit` printed another lane's branch,
/// moved meanwhile, as lost ("cannot lock ref 'refs/heads/rc-eroute': is at c9b3c4870e but
/// expected 9172137d5d", 11-18), and held that branch's lock while it packed. The shared config
/// says `maintenance.auto false` (written once, kept when it is there), and each landing runs the
/// maintenance itself ([`maintain`]).
pub fn own_maintenance(repo: &Path) -> Result<(), String> {
    use makepad_git::config;
    static WRITE: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _write = WRITE.lock().unwrap_or_else(|e| e.into_inner());
    let e = |e: makepad_git::GitError| format!("maintenance config: {e}");
    let paths = makepad_git::repository_paths(repo).map_err(e)?.ok_or_else(|| format!("maintenance config: no repository at {}", repo.display()))?;
    let shared = paths.common_dir.join("config");
    let text = config::read_file(&shared).map_err(e)?;
    if config::get(&text, "maintenance.auto").map_err(e)?.as_deref().and_then(config::parse_bool) == Some(false) {
        return Ok(());
    }
    if let Some(said) = settle(&paths.common_dir.join("config.lock")) { eprintln!("maintenance config: {said}"); }
    config::write_file(&shared, &config::set(&text, "maintenance.auto", "false").map_err(e)?).map_err(e)
}

/// The auto maintenance a landing's writes call for in `repo`, as git would run it after them
/// ([`own_maintenance`] keeps it from every other command): the refs packed, then what else is due,
/// detached. What it says is read by no one: a branch a concurrent move took from it stays as that
/// move left it, loose, and the next landing packs it.
pub fn maintain(repo: &Path) {
    use std::process::Stdio;
    let _ = crate::git_exec::command().arg("-C").arg(repo).args(["-c", "core.hooksPath=/dev/null", "maintenance", "run", "--auto", "--quiet", "--detach"])
        .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
        .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status();
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// Told once when `settle`, on this thread, first finds the lock held.
        pub(super) static SAW_HOLDER: std::cell::RefCell<Option<std::sync::mpsc::Sender<()>>> = const { std::cell::RefCell::new(None) };
    }

    /// [`own_maintenance`] writes `maintenance.auto false` into the shared config once and keeps
    /// it; a commit in a linked worktree then leaves the other lanes' branches loose as they are
    /// (git's own auto maintenance packed and pruned them all, a lost race said in the committer's
    /// terminal), and [`maintain`] runs over them without a word.
    #[test]
    fn a_commit_leaves_other_lanes_branches_to_devhubs_maintenance() {
        let dir = std::env::temp_dir().join(format!("devhub-maintain-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let git = |at: &Path, args: &[&str], input: &str| {
            use std::io::Write;
            let mut c = crate::git_exec::command().arg("-C").arg(at).args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "core.hooksPath=/dev/null"]).args(args)
                .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
                .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).spawn().unwrap();
            c.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            let o = c.wait_with_output().unwrap();
            assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
            (String::from_utf8_lossy(&o.stdout).trim().to_string(), String::from_utf8_lossy(&o.stderr).into_owned())
        };
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        git(&repo, &["init", "-q", "-b", "work"], "");
        git(&repo, &["commit", "-q", "--allow-empty", "-m", "base"], "");
        let base = git(&repo, &["rev-parse", "HEAD"], "").0;
        let lanes: String = (0..40).map(|i| format!("create refs/heads/rc-l{i} {base}\n")).collect();
        git(&repo, &["update-ref", "--stdin"], &lanes);
        own_maintenance(&repo).unwrap();
        let config = std::fs::read_to_string(repo.join(".git/config")).unwrap();
        own_maintenance(&repo).unwrap();
        assert_eq!(std::fs::read_to_string(repo.join(".git/config")).unwrap(), config, "written once");
        assert_eq!(git(&repo, &["config", "--get", "maintenance.auto"], "").0, "false");
        let tree = dir.join("lane");
        git(&repo, &["worktree", "add", "-q", "-b", "rc-own", &tree.to_string_lossy(), "work"], "");
        let (_, said) = git(&tree, &["commit", "-q", "--allow-empty", "-m", "own"], "");
        assert_eq!(said, "", "the commit says nothing of other branches");
        assert!((0..40).all(|i| repo.join(format!(".git/refs/heads/rc-l{i}")).is_file()), "the other lanes' branches stay loose, untouched");
        maintain(&repo);
        assert_eq!(git(&repo, &["rev-parse", "rc-l39"], "").0, base, "a branch keeps its commit through the maintenance");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A lock no process holds is cleared and said; one a live process holds open is waited for
    /// (and cleared once that process is gone without renaming it); no lock, no line. A sweep
    /// takes only a config write's locks: a commit's `index.lock` sits closed while its hooks run.
    #[test]
    fn a_stale_lock_is_cleared_and_a_held_one_waited_for() {
        let dir = std::env::temp_dir().join(format!("devhub-gitlock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("config.worktree.lock");
        // (written a minute ago: its writer long gone, nothing to settle)
        let left = |p: &Path, text: &str| {
            std::fs::write(p, text).unwrap();
            std::fs::File::options().write(true).open(p).unwrap().set_modified(SystemTime::now() - Duration::from_secs(60)).unwrap();
        };
        assert_eq!(settle(&lock), None, "no lock: nothing to say");
        left(&lock, "[merge \"ci-proof\"]\n");
        let said = settle(&lock).expect("a stale lock is said");
        assert!(!lock.exists() && said.contains("cleared a stale lock") && said.contains("no process held it"), "{said}");
        #[cfg(unix)]
        {
            use std::io::{BufRead, BufReader};
            use std::process::{Command, Stdio};

            left(&lock, "x");
            let mut child = Command::new("sh")
                .args(["-c", "exec 3<\"$1\"; echo held; read _ || :", "sh"]).arg(&lock)
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
                .spawn().unwrap();
            let pid = child.id();
            let mut ready = String::new();
            BufReader::new(child.stdout.take().unwrap()).read_line(&mut ready).unwrap();
            assert_eq!(ready, "held\n", "pid {pid} never opened the lock");

            let (tx, saw) = std::sync::mpsc::channel();
            let settler = std::thread::spawn({
                let lock = lock.clone();
                move || {
                    SAW_HOLDER.with(|s| *s.borrow_mut() = Some(tx));
                    settle(&lock)
                }
            });
            let saw_holder = saw.recv();
            drop(child.stdin.take());
            assert!(child.wait().unwrap().success(), "holder pid {pid} did not exit successfully");
            if saw_holder.is_err() {
                panic!("settle returned without seeing pid {pid} hold the lock: {:?}", settler.join());
            }
            let said = settler.join().unwrap().expect("a held lock is said");
            assert!(!lock.exists() && said.contains(&format!("pid {pid}")) && said.contains("cleared a stale lock"), "its holder waited for and named, then the lock cleared once it was gone: {said}");
        }
        // a commit running its hooks holds `index.lock` closed: a sweep leaves it, and `HEAD.lock`;
        // a config write's left lock goes
        left(&dir.join("index.lock"), "");
        left(&dir.join("HEAD.lock"), "");
        left(&lock, "x");
        std::fs::write(dir.join("index"), "").unwrap();
        let swept = sweep(&dir);
        assert!(swept.len() == 1 && swept[0].contains("config.worktree.lock") && !lock.exists(), "{swept:?}");
        assert!(dir.join("index.lock").exists() && dir.join("HEAD.lock").exists() && dir.join("index").exists(), "a commit's locks are never swept");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A left `index.lock` stays while a git that started before it runs (a commit running its
    /// hooks holds it closed), or while this platform cannot tell which gits run; it goes once
    /// only younger gits run, and the line says so.
    #[test]
    fn a_left_index_lock_goes_once_no_git_older_than_it_runs() {
        let dir = std::env::temp_dir().join(format!("devhub-indexlock-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("index.lock");
        assert_eq!(clear_left(&lock, &|| Some(Vec::new())), Ok(None), "no lock: nothing to say");
        let at = SystemTime::now() - Duration::from_secs(600);
        std::fs::write(&lock, "DIRC").unwrap();
        std::fs::File::options().write(true).open(&lock).unwrap().set_modified(at).unwrap();
        let kept = clear_left(&lock, &|| Some(vec![(41, at - Duration::from_secs(5)), (42, at + Duration::from_secs(60))])).unwrap_err();
        assert!(lock.exists() && kept.ends_with(": git pid 41 started before it was written and may still write it"), "a git older than the lock may be its writer, and only that one is named (the lock's path holds any digits): {kept}");
        let kept = clear_left(&lock, &|| None).unwrap_err();
        assert!(lock.exists() && kept.contains("cannot tell which gits run"), "{kept}");
        let said = clear_left(&lock, &|| Some(vec![(42, at + Duration::from_secs(60))])).unwrap().expect("a left lock is said");
        assert!(!lock.exists() && said.contains("cleared a stale lock (4 bytes"), "only a younger git runs: the lock goes: {said}");
        // in a lease's tree, only a git that may run there counts: one in the tree, its git dir or
        // its store, or one whose directory this platform cannot tell (11-26)
        let roots = [dir.join("tree"), dir.join("store.git")];
        let cwd = |pid: u32| match pid { 41 => Some(dir.join("tree/apps")), 43 => Some(dir.join("store.git/worktrees/t1")), 44 => Some(PathBuf::from("/elsewhere")), _ => None };
        let gits = vec![(41, at), (42, at), (43, at), (44, at)];
        assert_eq!(in_tree(gits, &roots, &cwd).iter().map(|g| g.0).collect::<Vec<_>>(), vec![41, 42, 43], "a git elsewhere on the box writes no index of the tree's");
        // a real checkout's left lock goes as its lease starts, and the line says so
        let tree = dir.join("checkout");
        std::fs::create_dir_all(&tree).unwrap();
        assert!(crate::git_exec::command().arg("-C").arg(&tree).args(["init", "-q", "--template="]).status().unwrap().success());
        assert_eq!(clear_left_index_in(&dir.join("none")), Ok(None), "no checkout: nothing to say");
        let lock = tree.join(".git").join("index.lock");
        std::fs::write(&lock, "").unwrap();
        std::fs::File::options().write(true).open(&lock).unwrap().set_modified(at).unwrap();
        if cfg!(target_os = "linux") {
            let said = clear_left_index_in(&tree).unwrap().expect("a left lock is said");
            assert!(!lock.exists() && said.contains("cleared a stale lock (0 bytes"), "{said}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
