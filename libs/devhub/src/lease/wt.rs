//! Mac lane worktrees: `makepad devhub wt create|list|prune` (local git, no master needed).
//!
//! Lane worktrees live in `wt.noindex` next to the repo (`/Users/admin/wt.noindex/<lane>` for
//! `/Users/admin/makepad`), on branch `rc-<lane>`, with an optional nested commercial worktree at
//! `apps/commercial`. `prune` removes only worktrees that are merged into `work`, clean (nested
//! commercial included, and its commits on a commercial ref), unlocked and idle (no live process
//! with its cwd inside), with `git worktree remove` + `git branch -d`; it reports what it kept and
//! why. Every git call has fixed arguments (never a shell), hooks off.

use std::path::{Path, PathBuf};
use std::process::Stdio;

/// The user's checkout: where `wt` works unless `--repo` names another.
pub const DEFAULT_REPO: &str = "/Users/admin/makepad";

/// Runs `git <args>` in `cwd` (or with `git_dir` as the repository); stdout on success.
pub fn git(cwd: Option<&Path>, git_dir: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let mut cmd = makepad_remote::group::command("git");
    cmd.arg("-c").arg("core.hooksPath=/dev/null").arg("-c").arg("advice.detachedHead=false");
    if let Some(g) = git_dir {
        cmd.arg("--git-dir").arg(g);
    }
    cmd.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if let Some(d) = cwd {
        cmd.current_dir(d);
    }
    cmd.env("GIT_TERMINAL_PROMPT", "0").env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE");
    let out = cmd.output().map_err(|e| format!("git: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let err: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).take(6).collect();
        Err(format!("git {}: {}", args.first().unwrap_or(&""), err.join(" | ")))
    }
}

/// Whether HEAD of the worktree at `dir` has commits no ref of its repository holds (a worktree's
/// HEAD is not a ref, so this is "work only this checkout has").
pub fn unique_commits(dir: &Path) -> bool {
    let refs = git(Some(dir), None, &["for-each-ref", "--format=%(objectname)"]).unwrap_or_default();
    let mut args = vec!["rev-list", "-n", "1", "HEAD", "--not"];
    args.extend(refs.lines().map(str::trim).filter(|s| !s.is_empty()));
    git(Some(dir), None, &args).map_or(true, |o| !o.trim().is_empty())
}

/// Tracked changes or untracked files (ignored ones do not count), apart from `except`.
pub fn dirty(dir: &Path, except: Option<&str>) -> bool {
    let ex = except.map(|e| format!(":(exclude){e}"));
    let mut args = vec!["status", "--porcelain", "--", "."];
    if let Some(e) = &ex {
        args.push(e);
    }
    git(Some(dir), None, &args).map_or(true, |o| !o.trim().is_empty())
}

/// Every live process's cwd: Linux `/proc/*/cwd`, macOS one `lsof -a -d cwd -Fn`.
pub fn live_cwds() -> Vec<(u32, PathBuf)> {
    #[cfg(target_os = "linux")]
    {
        let mut out = Vec::new();
        for e in std::fs::read_dir("/proc").into_iter().flatten().flatten() {
            if let Ok(pid) = e.file_name().to_string_lossy().parse::<u32>() {
                if let Ok(cwd) = std::fs::read_link(e.path().join("cwd")) {
                    out.push((pid, cwd));
                }
            }
        }
        out
    }
    #[cfg(not(target_os = "linux"))]
    {
        let o = makepad_remote::group::command("lsof").args(["-a", "-d", "cwd", "-Fn"]).stdin(Stdio::null()).stderr(Stdio::null()).output();
        o.map_or_else(|_| Vec::new(), |o| parse_lsof(&String::from_utf8_lossy(&o.stdout)))
    }
}

/// `lsof -Fn` output: `p<pid>` then `fcwd` / `n<path>` per process.
pub fn parse_lsof(text: &str) -> Vec<(u32, PathBuf)> {
    let mut out = Vec::new();
    let mut pid = None;
    for l in text.lines() {
        if let Some(p) = l.strip_prefix('p') {
            pid = p.parse().ok();
        } else if let (Some(n), Some(p)) = (l.strip_prefix('n'), pid) {
            out.push((p, PathBuf::from(n)));
        }
    }
    out
}

/// The pids whose cwd is `dir` or inside it.
pub fn live_in(cwds: &[(u32, PathBuf)], dir: &Path) -> Vec<u32> {
    cwds.iter().filter(|(_, c)| c.starts_with(dir)).map(|(p, _)| *p).collect()
}

/// One entry of `git worktree list --porcelain`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    pub head: String,
    /// `rc-gapx` (None: detached)
    pub branch: Option<String>,
    pub locked: bool,
}

pub fn parse_worktrees(text: &str) -> Vec<Entry> {
    let mut out: Vec<Entry> = Vec::new();
    for l in text.lines() {
        if let Some(p) = l.strip_prefix("worktree ") {
            out.push(Entry { path: PathBuf::from(p), ..Default::default() });
        } else if let Some(e) = out.last_mut() {
            if let Some(h) = l.strip_prefix("HEAD ") {
                e.head = h.to_string();
            } else if let Some(b) = l.strip_prefix("branch ") {
                e.branch = Some(b.strip_prefix("refs/heads/").unwrap_or(b).to_string());
            } else if l == "locked" || l.starts_with("locked ") {
                e.locked = true;
            }
        }
    }
    out
}

/// What prune looks at for one lane worktree.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Info {
    pub lane: String,
    pub entry: Entry,
    pub merged: bool,
    pub ahead: u32,
    pub behind: u32,
    pub dirty: bool,
    pub live: Vec<u32>,
    /// the nested commercial worktree: (dirty, has commits no commercial ref holds)
    pub commercial: Option<(bool, bool)>,
}

/// Prune's rule: Ok = remove; Err = keep, and why.
pub fn decide(i: &Info) -> Result<(), String> {
    if i.entry.locked {
        return Err("locked".into());
    }
    if !i.merged {
        return Err(format!("not merged into work ({} ahead)", i.ahead));
    }
    if i.dirty {
        return Err("uncommitted changes".into());
    }
    match i.commercial {
        Some((true, _)) => return Err("apps/commercial has uncommitted changes".into()),
        Some((_, true)) => return Err("apps/commercial has commits on no commercial ref".into()),
        _ => {}
    }
    if !i.live.is_empty() {
        return Err(format!("live process(es) inside: {:?}", i.live));
    }
    Ok(())
}

/// `<repo parent>/wt.noindex`.
pub fn root_of(repo: &Path) -> PathBuf {
    repo.parent().unwrap_or(Path::new("/")).join("wt.noindex")
}

/// The lane worktrees of `repo` (under its `wt.noindex`), measured.
pub fn infos(repo: &Path) -> Result<Vec<Info>, String> {
    // git prints resolved paths (macOS: /var -> /private/var), so compare resolved ones
    let root = root_of(repo);
    let root = std::fs::canonicalize(&root).unwrap_or(root);
    let cwds = live_cwds();
    let list = parse_worktrees(&git(Some(repo), None, &["worktree", "list", "--porcelain"])?);
    let mut out = Vec::new();
    // (Windows: git prints `C:/...`, canonical paths are `\\?\C:\...`: compare those of both)
    let under = |p: &Path| p.starts_with(&root) || std::fs::canonicalize(p).is_ok_and(|c| c.starts_with(&root));
    for e in list.into_iter().filter(|e| under(&e.path)) {
        let lane = e.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let merged = !e.head.is_empty() && git(Some(repo), None, &["merge-base", "--is-ancestor", &e.head, "work"]).is_ok();
        let (mut ahead, mut behind) = (0, 0);
        if let Ok(c) = git(Some(repo), None, &["rev-list", "--left-right", "--count", &format!("{}...work", e.head)]) {
            let mut w = c.split_whitespace().map(|n| n.parse().unwrap_or(0));
            ahead = w.next().unwrap_or(0);
            behind = w.next().unwrap_or(0);
        }
        let exists = e.path.is_dir();
        let nested = e.path.join("apps/commercial");
        let commercial = (exists && nested.join(".git").exists()).then(|| (dirty(&nested, None), unique_commits(&nested)));
        out.push(Info {
            lane,
            merged,
            ahead,
            behind,
            dirty: !exists || dirty(&e.path, Some("apps/commercial")),
            live: live_in(&cwds, &e.path),
            commercial,
            entry: e,
        });
    }
    Ok(out)
}

fn valid_lane(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && !s.starts_with('-') && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// `wt create`: branch `rc-<lane>` from `from` at `<root>/<lane>`, plus the nested commercial
/// worktree (detached at commercial `main`) when asked. Returns the worktree's path.
pub fn create(repo: &Path, lane: &str, from: &str, commercial: bool) -> Result<PathBuf, String> {
    if !valid_lane(lane) {
        return Err(format!("{lane}: a lane name is a-z, 0-9, - and _"));
    }
    let path = root_of(repo).join(lane);
    if path.exists() {
        return Err(format!("{} exists", path.display()));
    }
    std::fs::create_dir_all(root_of(repo)).map_err(|e| e.to_string())?;
    let p = path.to_string_lossy().into_owned();
    git(Some(repo), None, &["worktree", "add", "-q", "-b", &format!("rc-{lane}"), &p, from])?;
    if commercial {
        let src = repo.join("apps/commercial");
        git(Some(&src), None, &["worktree", "add", "-q", "--detach", &path.join("apps/commercial").to_string_lossy(), "main"])?;
    }
    Ok(path)
}

/// One prune outcome: (lane, Ok(what was removed) | Err(why kept)).
pub type Pruned = (String, Result<String, String>);

/// `wt prune`: removes what [`decide`] allows, reports the rest.
pub fn prune(repo: &Path) -> Result<Vec<Pruned>, String> {
    let mut out = Vec::new();
    for i in infos(repo)? {
        if let Err(why) = decide(&i) {
            out.push((i.lane, Err(why)));
            continue;
        }
        out.push((i.lane.clone(), remove(repo, &i)));
    }
    let _ = git(Some(repo), None, &["worktree", "prune"]);
    Ok(out)
}

fn remove(repo: &Path, i: &Info) -> Result<String, String> {
    let path = &i.entry.path;
    let nested = path.join("apps/commercial");
    if i.commercial.is_some() {
        // removed from its own repository first, so neither repo keeps a dangling entry
        let common = git(Some(&nested), None, &["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
        git(None, Some(Path::new(common.trim())), &["worktree", "remove", &nested.to_string_lossy()])?;
    }
    // ignored files (the target dir) go with it; tracked or untracked changes make git refuse
    git(Some(repo), None, &["worktree", "remove", &path.to_string_lossy()])?;
    let mut what = format!("removed {}", path.display());
    if let Some(b) = &i.entry.branch {
        match git(Some(repo), None, &["branch", "-d", b]) {
            Ok(_) => what.push_str(&format!(", branch {b}")),
            Err(e) => what.push_str(&format!(", branch {b} kept ({e})")),
        }
    }
    Ok(what)
}

/// `makepad devhub wt create|list|prune` (None: not a `wt` word).
pub fn cli(args: &[&str]) -> Option<i32> {
    if args.first() != Some(&"wt") {
        return None;
    }
    let mut repo = PathBuf::from(DEFAULT_REPO);
    let (mut from, mut commercial, mut size) = ("work".to_string(), false, false);
    let mut pos = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i] {
            "--repo" if i + 1 < args.len() => {
                repo = PathBuf::from(args[i + 1]);
                i += 1;
            }
            "--from" if i + 1 < args.len() => {
                from = args[i + 1].to_string();
                i += 1;
            }
            "--commercial" => commercial = true,
            "--size" => size = true,
            a => pos.push(a),
        }
        i += 1;
    }
    let usage = "usage: makepad devhub wt create <lane> [--from work] [--commercial] | list [--size] | prune   [--repo <dir>]";
    Some(match pos.as_slice() {
        ["create", lane] => match create(&repo, lane, &from, commercial) {
            Ok(p) => {
                println!("{lane}: {} on rc-{lane} from {from}{}", p.display(), if commercial { " (+ apps/commercial at main)" } else { "" });
                0
            }
            Err(e) => {
                eprintln!("wt create {lane}: {e}");
                1
            }
        },
        ["list"] => match infos(&repo) {
            Ok(list) => {
                println!("{} lane worktree(s) under {}", list.len(), root_of(&repo).display());
                for i in list {
                    let sz = if size { format!(" {:.1} GB", super::disk::dir_size(&i.entry.path).0 as f64 / super::caps::GB) } else { String::new() };
                    println!(
                        "{} {} +{}/-{} {} {} live:{}{}{}",
                        i.lane,
                        i.entry.branch.as_deref().unwrap_or("(detached)"),
                        i.ahead,
                        i.behind,
                        if i.merged { "merged" } else { "unmerged" },
                        if i.dirty { "dirty" } else { "clean" },
                        if i.live.is_empty() { "-".to_string() } else { format!("{:?}", i.live) },
                        if i.commercial.is_some() { " +commercial" } else { "" },
                        sz
                    );
                }
                0
            }
            Err(e) => {
                eprintln!("wt list: {e}");
                1
            }
        },
        ["prune"] => match prune(&repo) {
            Ok(list) => {
                let n = list.iter().filter(|(_, r)| r.is_ok()).count();
                println!("wt prune: {n} removed, {} kept", list.len() - n);
                for (lane, r) in list {
                    match r {
                        Ok(what) => println!("removed {lane}: {what}"),
                        Err(why) => println!("kept {lane}: {why}"),
                    }
                }
                0
            }
            Err(e) => {
                eprintln!("wt prune: {e}");
                1
            }
        },
        _ => {
            eprintln!("{usage}");
            2
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_worktree_list_and_lsof() {
        let w = parse_worktrees("worktree /r/makepad\nHEAD aaa\nbranch refs/heads/work\n\nworktree /r/wt.noindex/gapx\nHEAD bbb\nbranch refs/heads/rc-gapx\nlocked busy\n\nworktree /r/wt.noindex/d\nHEAD ccc\ndetached\n");
        assert_eq!(w.len(), 3);
        assert_eq!((w[1].branch.as_deref(), w[1].locked), (Some("rc-gapx"), true));
        assert_eq!((w[2].branch.as_deref(), w[2].head.as_str()), (None, "ccc"));
        let l = parse_lsof("p12\nfcwd\nn/Users/admin/wt.noindex/gapx/libs\np13\nfcwd\nn/\n");
        assert_eq!(live_in(&l, Path::new("/Users/admin/wt.noindex/gapx")), vec![12]);
    }

    #[test]
    fn prune_rules() {
        let ok = Info { lane: "a".into(), merged: true, ..Default::default() };
        assert!(decide(&ok).is_ok());
        assert!(decide(&Info { merged: false, ahead: 3, ..ok.clone() }).unwrap_err().contains("3 ahead"));
        assert!(decide(&Info { dirty: true, ..ok.clone() }).is_err());
        assert!(decide(&Info { live: vec![5], ..ok.clone() }).is_err());
        assert!(decide(&Info { commercial: Some((false, true)), ..ok.clone() }).is_err());
        assert!(decide(&Info { commercial: Some((false, false)), ..ok.clone() }).is_ok());
        let mut locked = ok.clone();
        locked.entry.locked = true;
        assert!(decide(&locked).is_err());
    }

    /// A throwaway repo in a temp dir: never the user's checkout.
    #[test]
    fn create_list_prune_in_a_temp_repo() {
        let base = std::env::temp_dir().join(format!("devhub-wt-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let g = |dir: &Path, args: &[&str]| git(Some(dir), None, args).unwrap();
        g(&repo, &["init", "-q", "-b", "work"]);
        std::fs::write(repo.join("a.txt"), "a").unwrap();
        std::fs::write(repo.join(".gitignore"), "target/\n").unwrap();
        g(&repo, &["add", "-A"]);
        g(&repo, &["-c", "user.name=t", "-c", "user.email=t@localhost", "commit", "-q", "-m", "one"]);
        for lane in ["merged", "ahead", "dirty", "busy"] {
            create(&repo, lane, "work", false).unwrap();
        }
        assert!(create(&repo, "merged", "work", false).is_err());
        assert!(create(&repo, "../x", "work", false).is_err());
        let root = root_of(&repo);
        // a target dir (ignored) goes with a merged worktree
        std::fs::create_dir_all(root.join("merged/target/release")).unwrap();
        std::fs::write(root.join("merged/target/release/x"), "bin").unwrap();
        std::fs::write(root.join("ahead/b.txt"), "b").unwrap();
        g(&root.join("ahead"), &["add", "-A"]);
        g(&root.join("ahead"), &["-c", "user.name=t", "-c", "user.email=t@localhost", "commit", "-q", "-m", "two"]);
        std::fs::write(root.join("dirty/new.txt"), "x").unwrap();
        // a process working in a worktree keeps it (Unix: its cwd is seen; Windows: an open cwd
        // cannot be removed)
        let mut child = if cfg!(windows) { makepad_remote::group::command("ping.exe").args(["-n", "30", "127.0.0.1"]).stdout(Stdio::null()).current_dir(root.join("busy")).spawn().unwrap() } else { makepad_remote::group::command("sleep").arg("30").current_dir(root.join("busy")).spawn().unwrap() };
        let list = infos(&repo).unwrap();
        assert_eq!(list.len(), 4);
        let r = prune(&repo).unwrap();
        let get = |l: &str| r.iter().find(|(x, _)| x == l).unwrap().1.clone();
        assert!(get("merged").is_ok(), "{r:?}");
        assert!(get("ahead").unwrap_err().contains("not merged"));
        assert!(get("dirty").unwrap_err().contains("uncommitted"));
        assert!(get("busy").is_err() && (cfg!(windows) || get("busy").unwrap_err().contains(&child.id().to_string())), "{r:?}");
        assert!(!root.join("merged").exists());
        assert!(git(Some(&repo), None, &["rev-parse", "--verify", "-q", "rc-merged"]).is_err());
        assert!(root.join("ahead/b.txt").exists() && root.join("dirty/new.txt").exists());
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&base);
    }
}
