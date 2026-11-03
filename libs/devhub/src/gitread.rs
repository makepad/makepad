//! Every git repository devhub reads, in-process with makepad-git: no git process starts, so a
//! loop may ask every tick (on macOS every exec is a code assessment). What changes a repo still
//! runs git, through [`crate::git_exec`].
//!
//! Reads are bounded: an object is read as its compressed range of the pack, never by loading the
//! whole pack (the makepad pack is several GB: unbounded readers took the master to 20 GB). Object
//! and ref questions go to one repository per shared git dir, opened once per process and kept,
//! so its pack indexes are read once ([`with`]); a worktree's own state (index, status) opens
//! [`worktree`] per use. No other devhub code opens a repository (a test holds it to that).
mod view;
pub use view::Pass;

use makepad_git::memory::MemoryAccount;
use makepad_git::{refs, GitError, ObjectId, Repository, RepositoryPaths};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// What one repository may hold: pack index tables, decoded objects it keeps, the object in hand.
const ACCOUNT_BYTES: usize = 512 << 20;
/// Optional decoded-object retention; larger objects remain readable.
const OBJECT_BYTES: usize = 32 << 20;

fn bounded(mut repo: Repository) -> Result<Repository, GitError> {
    repo.set_read_cache_budget(MemoryAccount::new(ACCOUNT_BYTES), OBJECT_BYTES)?;
    Ok(repo)
}

/// The repository `dir` is in, walking up as git does.
pub fn paths(dir: &Path) -> Option<RepositoryPaths> {
    let mut at = dir.canonicalize().ok()?;
    if !at.is_dir() { return None; }
    loop {
        match makepad_git::repository_paths(&at) {
            Ok(Some(mut p)) => { p.common_dir = p.common_dir.canonicalize().ok()?; return Some(p); }
            Ok(None) => {}
            Err(_) => return None,
        }
        if !at.pop() {
            return None;
        }
    }
}

/// One worktree's repository (its index, HEAD, status), bounded, opened for this use only.
pub fn worktree(p: RepositoryPaths) -> Result<Repository, GitError> {
    bounded(Repository::open_git_dir(p.git_dir, p.workdir)?)
}

/// The repository of the shared git dir `common`, opened once per process; opened again when its
/// packs change (a repack or a fetch writes the pack dir).
fn shared(common: &Path) -> Option<Arc<Mutex<Repository>>> {
    let canonical = common.canonicalize().ok()?;
    let common = canonical.as_path();
    type Open = HashMap<PathBuf, (Vec<(PathBuf, Option<std::time::SystemTime>, u64)>, Arc<Mutex<Repository>>)>;
    static OPEN: Mutex<Option<Open>> = Mutex::new(None);
    let objects = common.join("objects");
    let mut dirs = vec![objects.clone()];
    let alternate_file = objects.join("info/alternates");
    match std::fs::read_to_string(&alternate_file) {
        Ok(text) => for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let path = PathBuf::from(line);
            dirs.push(if path.is_absolute() { path } else { objects.join(path) });
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
        Err(_) => return None,
    }
    let mut sources = vec![alternate_file];
    sources.extend(dirs.into_iter().map(|p| p.join("pack")));
    let packs: Vec<_> = sources.into_iter().map(|p| {
        let meta = std::fs::metadata(&p).ok();
        let modified = meta.as_ref().and_then(|m| m.modified().ok());
        (p, modified, meta.map_or(0, |m| m.len()))
    }).collect();
    let mut open = OPEN.lock().unwrap_or_else(|e| e.into_inner());
    let open = open.get_or_insert_with(HashMap::new);
    if let Some((at, repo)) = open.get(common) {
        if *at == packs {
            return Some(repo.clone());
        }
    }
    // (objects and shared refs only: a worktree's HEAD is read from its own dir, [`oid_of`])
    let workdir = if common.file_name().is_some_and(|n| n == ".git") { common.parent().unwrap_or(common) } else { common };
    let repo = Arc::new(Mutex::new(bounded(Repository::open_git_dir(common.to_path_buf(), workdir.to_path_buf()).ok()?).ok()?));
    open.retain(|path, _| path.is_dir());
    open.insert(common.to_path_buf(), (packs, repo.clone()));
    Some(repo)
}

/// The shared repository of the git dir `common`, for a caller that asks many questions in a row
/// (lock it per question; never across a call into this module for the same repository).
pub fn handle(common: &Path) -> Option<Arc<Mutex<Repository>>> {
    shared(common)
}

/// Runs `f` with the paths of the repository `dir` is in and its shared repository, locked while
/// `f` runs (`f` asks nothing else of this module about the same repository).
pub fn with<T>(dir: &Path, f: impl FnOnce(&RepositoryPaths, &mut Repository) -> T) -> Option<T> {
    let p = paths(dir)?;
    let repo = shared(&p.common_dir)?;
    let mut repo = repo.lock().unwrap_or_else(|e| e.into_inner());
    Some(f(&p, &mut repo))
}

/// [`with`] for a shared git dir itself (a repository's `.git`, a bare store).
pub fn with_common<T>(common: &Path, f: impl FnOnce(&mut Repository) -> T) -> Option<T> {
    let repo = shared(common)?;
    let mut repo = repo.lock().unwrap_or_else(|e| e.into_inner());
    Some(f(&mut repo))
}

/// What `name` names, unchecked: a full commit id, `HEAD` (the worktree's own), `refs/…`, or a
/// branch, tag or remote branch name; a `^{commit}` suffix is allowed.
fn oid_of(p: &RepositoryPaths, name: &str) -> Option<ObjectId> {
    let name = name.strip_suffix("^{commit}").unwrap_or(name);
    if name.len() == 40 && name.bytes().all(|b| b.is_ascii_hexdigit()) {
        ObjectId::from_hex(name).ok()
    } else if name == "HEAD" {
        refs::resolve_head_in(&p.git_dir, &p.common_dir).ok()
    } else if name.starts_with("refs/") {
        refs::resolve_ref_in(&p.git_dir, &p.common_dir, name).ok()
    } else {
        ["refs/heads/", "refs/tags/", "refs/remotes/"].iter().find_map(|pre| refs::resolve_ref_in(&p.git_dir, &p.common_dir, &format!("{pre}{name}")).ok())
    }
}

/// The commit `name` names ([`oid_of`]), when the repository has it.
pub fn resolve(p: &RepositoryPaths, repo: &mut Repository, name: &str) -> Option<ObjectId> {
    let oid = oid_of(p, name)?;
    repo.read_commit(&oid).ok().map(|_| oid)
}

/// [`resolve`] in the repository of `dir`, as hex.
pub fn rev(dir: &Path, name: &str) -> Option<String> {
    with(dir, |p, repo| resolve(p, repo, name)).flatten().map(|o| o.to_hex())
}

/// Whether the repository of `dir` definitely has no commit `sha` (a read that failed for another
/// reason, or a repository that cannot be opened, says nothing: false).
pub fn lacks_commit(dir: &Path, sha: &str) -> bool {
    let Ok(oid) = ObjectId::from_hex(sha) else { return false };
    with(dir, |_, repo| matches!(repo.read_commit(&oid), Err(GitError::ObjectNotFound(_) | GitError::InvalidObject(_)))).unwrap_or(false)
}

/// `git merge-base --is-ancestor a b`: `a` is `b` or one of its ancestors. A name that resolves
/// to no commit is no ancestor.
pub fn is_ancestor(dir: &Path, a: &str, b: &str) -> bool {
    with(dir, |p, repo| {
        let (Some(a), Some(b)) = (resolve(p, repo, a), resolve(p, repo, b)) else { return false };
        makepad_git::is_ancestor(repo, a, b).unwrap_or(false)
    })
    .unwrap_or(false)
}

/// The object id at each of `paths` in the tree of commit `at` (`git rev-parse <at>:<path>`; ""
/// where nothing is), for the repository of `dir`. None: no such commit.
pub fn entries_at(dir: &Path, at: &str, paths: &[&str]) -> Option<Vec<String>> {
    with(dir, |p, repo| {
        let c = resolve(p, repo, at)?;
        let tree = repo.read_commit(&c).ok()?.tree;
        Some(paths.iter().map(|path| repo.entry_at(&tree, path.trim_end_matches('/')).ok().flatten().map_or(String::new(), |o| o.to_hex())).collect())
    })
    .flatten()
}

/// Whether the trees of `from` and `to` differ under any of `paths` (`git diff --quiet from to --
/// <paths>` failing; no paths: anywhere). A commit that cannot be read counts as a difference.
pub fn differ_under<'a>(dir: &Path, from: &str, to: &str, paths: impl IntoIterator<Item = &'a str>) -> bool {
    let paths: Vec<&str> = paths.into_iter().collect();
    with(dir, |p, repo| {
        let trees = (|| {
            let f = resolve(p, repo, from)?;
            let t = resolve(p, repo, to)?;
            Some((repo.read_commit(&f).ok()?.tree, repo.read_commit(&t).ok()?.tree))
        })();
        let Some((f, t)) = trees else { return true };
        if paths.is_empty() {
            return f != t;
        }
        paths.iter().any(|path| {
            let path = path.trim_end_matches('/');
            match (repo.entry_at(&f, path), repo.entry_at(&t, path)) {
                (Ok(a), Ok(b)) => a != b,
                _ => true,
            }
        })
    })
    .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    fn g(dir: &Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git").arg("-C").arg(dir).args(["-c", "user.name=t", "-c", "user.email=t@t"]).args(args).env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    /// The questions loops ask, answered as git answers them, without starting git, from a
    /// packed repository whose pack is never read whole.
    #[test]
    fn answers_as_git_without_git_and_reads_packs_bounded() {
        let d = std::env::temp_dir().join(format!("devhub-gitread-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("libs/a")).unwrap();
        std::fs::create_dir_all(d.join("libs/b")).unwrap();
        g(&d, &["init", "-q", "-b", "work"]);
        // a large incompressible blob makes the pack big: a whole-pack read would show
        let mut noise = vec![0u8; 4 << 20];
        let mut x: u64 = 0x9e3779b97f4a7c15;
        for b in noise.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        std::fs::write(d.join("big.bin"), &noise).unwrap();
        std::fs::write(d.join("libs/a/x.rs"), "1").unwrap();
        std::fs::write(d.join("libs/b/y.rs"), "1").unwrap();
        g(&d, &["add", "."]);
        g(&d, &["commit", "-q", "-m", "1"]);
        let one = g(&d, &["rev-parse", "HEAD"]);
        std::fs::write(d.join("libs/b/y.rs"), "2").unwrap();
        g(&d, &["commit", "-q", "-am", "2"]);
        let two = g(&d, &["rev-parse", "HEAD"]);
        g(&d, &["checkout", "-q", "-b", "rc-x", &one]);
        g(&d, &["commit", "-q", "--allow-empty", "-m", "x"]);
        g(&d, &["gc", "-q"]);
        let pack: u64 = std::fs::read_dir(d.join(".git/objects/pack")).unwrap().flatten().filter(|e| e.path().extension().is_some_and(|x| x == "pack")).map(|e| e.metadata().unwrap().len()).sum();
        assert!(pack > 4 << 20, "the pack holds the blob ({pack} bytes)");
        let before = crate::git_exec::on_this_thread();
        assert_eq!(super::rev(&d, "refs/heads/work").as_deref(), Some(two.as_str()));
        assert_eq!(super::rev(&d, "work").as_deref(), Some(two.as_str()));
        assert_eq!(super::rev(&d, "HEAD").as_deref(), super::rev(&d, "rc-x").as_deref());
        assert_eq!(super::rev(&d, &format!("{one}^{{commit}}")).as_deref(), Some(one.as_str()));
        assert_eq!(super::rev(&d, &"ab".repeat(20)), None);
        assert_eq!(super::rev(&d, "refs/heads/nope"), None);
        assert!(super::lacks_commit(&d, &"ab".repeat(20)) && !super::lacks_commit(&d, &one) && !super::lacks_commit(Path::new("/nonexistent-devhub-repo"), &one));
        assert!(super::is_ancestor(&d, &one, "refs/heads/work") && super::is_ancestor(&d, &one, "rc-x"));
        assert!(!super::is_ancestor(&d, "rc-x", "work") && !super::is_ancestor(&d, &two, &one));
        assert!(!super::differ_under(&d, &one, &two, ["libs/a"]) && super::differ_under(&d, &one, &two, ["libs/a", "libs/b/"]));
        assert!(super::differ_under(&d, &one, &two, []) && !super::differ_under(&d, &one, "rc-x", []));
        assert!(super::differ_under(&d, &one, &"ab".repeat(20), ["libs/a"]));
        let e = super::entries_at(&d, &two, &["libs/b/y.rs", "nope", "big.bin"]).unwrap();
        assert_eq!(e[0], g(&d, &["rev-parse", &format!("{two}:libs/b/y.rs")]));
        assert_eq!((e[1].as_str(), e[2].len()), ("", 40));
        assert_eq!(crate::git_exec::on_this_thread(), before, "a read started git");
        // one repository for all of it, and none of its pack reads took the pack whole
        let read = super::with(&d, |_, repo| repo.read_stats().pack_bytes_read).unwrap();
        assert!(read < pack / 4, "{read} of the pack's {pack} bytes read");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The process's resident and peak memory (Linux), in MB.
    fn rss_mb() -> (u64, u64) {
        let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        let kb = |k: &str| s.lines().find(|l| l.starts_with(k)).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0) / 1024;
        (kb("VmRSS:"), kb("VmHWM:"))
    }

    /// The read paths loops use, on a clone of the node's own store of the repository at
    /// `~/devhub/soak/makepad`: the newest lane branch pushed to it (the real history, merges
    /// included), its one pack padded with an incompressible side branch to the size of a busy
    /// master's (3.8 GB); run by hand with `--ignored`. Every answer equals git's, and time and
    /// memory per call stay bounded: a pack read whole would show in the peak.
    #[test]
    #[ignore]
    fn soak_read_paths_on_a_real_repo() {
        const PAD_FILES: u64 = 8;
        const PAD_FILE_BYTES: u64 = 448 << 20;
        let soak = crate::os::home_dir().join("devhub/soak");
        let d = soak.join("makepad");
        if d.join(".git").exists() && g(&d, &["rev-list", "--count", "--min-parents=2", "HEAD"]) == "0" {
            std::fs::remove_dir_all(&soak).unwrap();
        }
        if !d.join(".git").exists() {
            std::fs::create_dir_all(&soak).unwrap();
            // the branches as the master pushed them (`refs/devhub/pushed/<branch>`)
            let store = crate::os::home_dir().join("devhub/git/makepad.git");
            g(&soak, &["init", "-q", "-b", "soak-base", "makepad"]);
            g(&d, &["fetch", "-q", store.to_str().unwrap(), "refs/devhub/pushed/*:refs/heads/*"]);
            let lane = g(&d, &["for-each-ref", "--sort=-committerdate", "--count=1", "--format=%(refname:short)", "refs/heads/rc-*"]);
            g(&d, &["checkout", "-q", &lane]);
        }
        let pack_size = || -> u64 { std::fs::read_dir(d.join(".git/objects/pack")).unwrap().flatten().map(|e| e.metadata().unwrap().len()).sum() };
        if pack_size() < PAD_FILES * PAD_FILE_BYTES {
            // written in small chunks: the test's own peak must not hold a pad file
            use std::io::Write;
            std::fs::create_dir_all(d.join("pad")).unwrap();
            let mut x: u64 = 0x9e3779b97f4a7c15;
            let mut chunk = vec![0u8; 8 << 20];
            for i in 0..PAD_FILES {
                let mut f = std::io::BufWriter::new(std::fs::File::create(d.join(format!("pad/{i}.bin"))).unwrap());
                for _ in 0..PAD_FILE_BYTES / chunk.len() as u64 {
                    for b in chunk.iter_mut() {
                        x ^= x << 13;
                        x ^= x >> 7;
                        x ^= x << 17;
                        *b = x as u8;
                    }
                    f.write_all(&chunk).unwrap();
                }
            }
            drop(chunk);
            g(&d, &["checkout", "-q", "-b", "soak-pad"]);
            g(&d, &["-c", "core.bigFileThreshold=1m", "add", "pad"]);
            g(&d, &["commit", "-q", "-m", "pad"]);
            g(&d, &["checkout", "-q", "-"]);
            // one pack, the history's commits and trees among the pad blobs
            g(&d, &["-c", "core.bigFileThreshold=1m", "repack", "-a", "-d", "-q"]);
        }
        let pack = pack_size();
        let say = |what: &str, t: std::time::Instant| {
            let (rss, peak) = rss_mb();
            println!("SOAK {what:44} {:>9.1} ms  rss {rss:>5} MB  peak {peak:>5} MB", t.elapsed().as_secs_f64() * 1000.0);
        };
        println!("SOAK pack {} MB, start rss {:?}", pack >> 20, rss_mb());
        let tip = g(&d, &["rev-parse", "HEAD"]);
        let firsts: Vec<String> = g(&d, &["rev-list", "--first-parent", "-n", "2001", &tip]).lines().map(str::to_string).collect();
        let (near, far) = (firsts[50].clone(), firsts[firsts.len() - 1].clone());
        let merge = g(&d, &["rev-list", "--min-parents=2", "-n", "1", &near]);
        assert!(!merge.is_empty(), "the soak needs a history with merges");
        let side = g(&d, &["rev-parse", &format!("{merge}^2")]);
        let dirs = ["libs/devhub", "platform", "Cargo.toml"];
        for round in 0..3 {
            let t = std::time::Instant::now();
            assert_eq!(super::rev(&d, "HEAD").as_deref(), Some(tip.as_str()));
            say(&format!("r{round} rev HEAD (snapshot, show, roll)"), t);
            for (a, b) in [(&near, &tip), (&far, &tip), (&tip, &near), (&side, &tip), (&side, &far)] {
                let want = std::process::Command::new("git").arg("-C").arg(&d).args(["merge-base", "--is-ancestor", a, b]).status().unwrap().success();
                let t = std::time::Instant::now();
                assert_eq!(super::is_ancestor(&d, a, b), want, "is_ancestor {a} {b}");
                say(&format!("r{round} is_ancestor {}..{} = {want}", &a[..8], &b[..8]), t);
            }
            for from in [&near, &far] {
                let want = !std::process::Command::new("git").arg("-C").arg(&d).args(["diff", "--quiet", from, &tip, "--"]).args(dirs).status().unwrap().success();
                let t = std::time::Instant::now();
                assert_eq!(super::differ_under(&d, from, &tip, dirs), want);
                say(&format!("r{round} differ_under {} = {want} (roll)", &from[..8]), t);
            }
            let t = std::time::Instant::now();
            let e = super::entries_at(&d, &tip, &dirs).unwrap();
            say(&format!("r{round} entries_at x3 (show inputs)"), t);
            for (i, p) in dirs.iter().enumerate() {
                assert_eq!(e[i], g(&d, &["rev-parse", &format!("{tip}:{p}")]));
            }
        }
        // the lanes pass and cleanup: everything the tip reaches, once
        let t = std::time::Instant::now();
        let n = super::with(&d, |_, repo| {
            let mut reach = makepad_git::Reach::new();
            reach.advance(repo, makepad_git::ObjectId::from_hex(&tip).unwrap()).unwrap();
            reach.len()
        }).unwrap();
        say(&format!("Reach of HEAD: {n} commits (lanes pass, cleanup)"), t);
        assert_eq!(n.to_string(), g(&d, &["rev-list", "--count", &tip]));
        // a worktree's status (monitor baseline, lanes dirty check)
        for skip_untracked in [true, false] {
            let t = std::time::Instant::now();
            let st = super::worktree(super::paths(&d).unwrap()).unwrap().status_with_options(makepad_git::StatusOptions { skip_untracked, ..Default::default() }).unwrap();
            say(&format!("worktree status skip_untracked={skip_untracked}: {}", st.entries.len()), t);
            let want = g(&d, &["status", "--porcelain", if skip_untracked { "--untracked-files=no" } else { "--untracked-files=all" }]).lines().count();
            assert_eq!(st.entries.len(), want);
        }
        let (_, peak) = rss_mb();
        let read = super::with(&d, |_, repo| repo.read_stats().pack_bytes_read).unwrap();
        println!("SOAK peak {peak} MB, {} MB of the {} MB pack read", read >> 20, pack >> 20);
        assert!(peak < 512 && read < pack / 4, "peak {peak} MB, {read} bytes read: a pack was read whole");
    }

    /// Devhub opens repositories only here: anything else would read packs whole again.
    #[test]
    fn no_repository_is_opened_elsewhere() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut todo = vec![src];
        let mut found = Vec::new();
        while let Some(dir) = todo.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let path = e.path();
                if path.is_dir() {
                    todo.push(path);
                } else if path.extension().is_some_and(|x| x == "rs") && !path.ends_with("gitread.rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    for (i, l) in text.lines().enumerate() {
                        if l.contains("Repository::open") || l.contains("open_git_dir(") {
                            found.push(format!("{}:{}", path.display(), i + 1));
                        }
                    }
                }
            }
        }
        assert!(found.is_empty(), "repositories opened outside gitread: {found:?}");
    }
}
