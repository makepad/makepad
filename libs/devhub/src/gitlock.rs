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
        let o = makepad_remote::group::command("lsof").arg("-t").arg("--").arg(path).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
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
            if waited.is_empty() { eprintln!("{}: held by pid {}: waiting for it", lock.display(), pids.iter().map(u32::to_string).collect::<Vec<_>>().join(" ")); }
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

/// Every git lock file directly in `git_dir` (`index.lock`, `HEAD.lock`, `config.worktree.lock`,
/// ..) settled as [`settle`] does; a line for each one there was.
pub fn sweep(git_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(git_dir) else { return Vec::new() };
    let mut locks: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_file() && p.extension().is_some_and(|x| x == "lock")).collect();
    locks.sort();
    locks.iter().filter_map(|l| settle(l)).collect()
}

/// [`sweep`] of a worktree's own git dir and of the repository's shared one.
pub fn sweep_worktree(tree: &Path) -> Vec<String> {
    let Ok(Some(paths)) = makepad_git::repository_paths(tree) else { return Vec::new() };
    let mut said = sweep(&paths.git_dir);
    if paths.common_dir != paths.git_dir { said.extend(sweep(&paths.common_dir)); }
    said
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lock no process holds is cleared and said; one a live process holds open is waited for
    /// (and cleared once that process is gone without renaming it); no lock, no line.
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
            left(&lock, "x");
            let mut child = std::process::Command::new("sh").arg("-c").arg("exec 3<\"$1\"; sleep 0.3").arg("sh").arg(&lock).spawn().unwrap();
            let pid = child.id();
            let t = Instant::now();
            while holders(&lock).is_some_and(|p| !p.contains(&pid)) && t.elapsed() < Duration::from_secs(5) { std::thread::sleep(Duration::from_millis(10)); }
            let said = settle(&lock).expect("a held lock is said");
            let _ = child.wait();
            assert!(!lock.exists() && said.contains(&format!("pid {pid}")) && said.contains("cleared a stale lock"), "its holder waited for and named, then the lock cleared once it was gone: {said}");
        }
        left(&dir.join("index.lock"), "");
        std::fs::write(dir.join("index"), "").unwrap();
        let swept = sweep(&dir);
        assert!(swept.len() == 1 && swept[0].contains("index.lock") && dir.join("index").exists() && !dir.join("index.lock").exists(), "{swept:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
