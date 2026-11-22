//! The repositories nested in a makepad checkout: each `apps/<name>` holding a git checkout of
//! its own (a clone, or a lane's worktree of one). makepad's `.gitignore` keeps them out of its
//! tree and its workspace takes their crates in. A pin, a lease's tree and its overlay name each
//! by its `name`, which is also the name of its store on a node.

use std::path::{Path, PathBuf};

/// A nested repository's directory in the tree: `apps/<name>`.
pub fn dir(name: &str) -> String {
    format!("apps/{name}")
}

/// What a tree path of nested repository `name` starts with: `apps/<name>/`.
pub fn prefix(name: &str) -> String {
    format!("apps/{name}/")
}

/// The public nested repositories (Fab's makepad/apps, 11-22): the private check runs on what their
/// commits, pushes and landings bring, as on makepad's. Commercial and Source Library are private:
/// their lanes keep the message check alone (main, 11-22: no refusals of their data at land).
pub const PUBLIC: &[&str] = &["apps"];

/// Whether nested repository `name` is public ([`PUBLIC`]).
pub fn public(name: &str) -> bool {
    PUBLIC.contains(&name)
}

/// Whether `name` can name a nested repository: lowercase letters, digits and dashes, and not
/// makepad's own name.
pub fn valid(name: &str) -> bool {
    !name.is_empty() && name != "makepad" && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The nested repositories of the checkout at `root`, by name.
pub fn names(root: &Path) -> Vec<String> {
    let mut out: Vec<String> = std::fs::read_dir(root.join("apps"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().join(".git").exists())
        .filter_map(|e| e.file_name().to_str().map(str::to_string))
        .filter(|n| valid(n))
        .collect();
    out.sort();
    out
}

/// The dirs of the nested repositories in the tree at `root` (`apps/<name>`): the paired one's
/// always, and each other one checked out there. Their files are their own halves', never the
/// outer tree's.
pub fn dirs(root: &Path) -> Vec<String> {
    std::iter::once(dir(crate::state::PAIRED)).chain(names(root).into_iter().filter(|n| n != crate::state::PAIRED).map(|n| dir(&n))).collect()
}

/// The nested repositories the checkout at `root` follows, each with its branch: the paired one's
/// `main` ([`crate::state::PAIRED`]), each other one's the branch its checkout is on, where the land
/// queue lands a hand-in's halves (one on no branch is followed by nothing). What the buckets
/// prove and the deployments follow. One on a branch with no commit yet (a fresh `git init`) is
/// followed by nothing either, said once: its tip held up every bucket and deployment unsaid.
pub fn branches(root: &Path) -> Vec<(String, String)> {
    let paired = crate::state::PAIRED;
    let others = names(root).into_iter().filter(|n| n != paired).filter_map(|n| {
        let at = root.join(dir(&n));
        let branch = crate::gitread::branch(&at)?;
        if crate::gitread::rev(&at, &format!("refs/heads/{branch}")).is_none() {
            static SAID: std::sync::Mutex<std::collections::BTreeSet<std::path::PathBuf>> = std::sync::Mutex::new(std::collections::BTreeSet::new());
            if SAID.lock().unwrap_or_else(|e| e.into_inner()).insert(at.clone()) {
                eprintln!("devhub: {} is on branch {branch} with no commit yet: nothing follows it until it has one", at.display());
            }
            return None;
        }
        Some((n, branch))
    });
    std::iter::once((paired.to_string(), "main".to_string())).chain(others).collect()
}

/// The user's checkout of makepad that the checkout at `root` is a worktree of (`root` itself when
/// it is that checkout): it holds each nested repository the buckets follow ([`branches`]), which a
/// lane's worktree may lack. None for a tree that is no worktree of a checkout (a node's lease tree
/// is one of a bare store).
pub fn hub(root: &Path) -> Option<PathBuf> {
    let paths = makepad_git::repository_paths(root).ok().flatten()?;
    let hub = paths.common_dir.parent()?;
    (paths.common_dir.file_name()? == ".git" && hub.join(".git").is_dir()).then(|| hub.to_path_buf())
}

/// Where nested repository `name` of the tree at `root` is checked out: its own `apps/<name>`, else
/// the user's checkout's ([`hub`]). None: neither holds it.
pub fn checkout(root: &Path, name: &str) -> Option<PathBuf> {
    std::iter::once(root.to_path_buf()).chain(hub(root)).map(|r| r.join(dir(name))).find(|d| d.join(".git").exists())
}

/// The followed nested repositories besides the paired one ([`branches`] of the user's checkout,
/// [`hub`]), each at the tip of its branch there: (name, branch, sha). What a hand-in lands beside.
pub fn tips(root: &Path) -> Vec<(String, String, String)> {
    let Some(hub) = hub(root) else { return Vec::new() };
    branches(&hub).into_iter().filter(|(n, _)| n != crate::state::PAIRED).filter_map(|(n, branch)| {
        let sha = crate::gitread::rev(&hub.join(dir(&n)), &format!("refs/heads/{branch}"))?;
        Some((n, branch, sha))
    }).collect()
}

/// Those of [`tips`] the checkout at `root` does not hold: a lane's worktree has makepad and
/// commercial alone. A lane's run pins each at its landed tip, so its packages are in the lease's
/// tree as they are in the buckets': #7425 broke Source Library's build, and its lane could not
/// test it, nor its hand-in's check see it (11-19).
pub fn absent(root: &Path) -> Vec<(String, String, String)> {
    let here = names(root);
    tips(root).into_iter().filter(|(n, _, _)| !here.contains(n)).collect()
}

/// [`absent`] as sides of a pinned `--rev`: `,<name>=<branch>@<sha>` each.
pub fn absent_sides(root: &Path) -> String {
    absent(root).iter().map(|(n, branch, sha)| format!(",{n}={branch}@{sha}")).collect()
}

/// The nested repository among `names` that tree path `path` is in, with the path inside it.
pub fn split<'a, 'n>(path: &'a str, names: impl IntoIterator<Item = &'n str>) -> Option<(&'n str, &'a str)> {
    names.into_iter().find_map(|n| path.strip_prefix("apps/")?.strip_prefix(n)?.strip_prefix('/').map(|rest| (n, rest)))
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_nested_repository_is_an_apps_dir_with_its_own_git() {
        let d = std::env::temp_dir().join(format!("devhub-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for p in ["apps/alpha/.git", "apps/beta-2/.git", "apps/plain/src", "apps/Upper/.git"] {
            std::fs::create_dir_all(d.join(p)).unwrap();
        }
        // a worktree's `.git` is a file
        std::fs::create_dir_all(d.join("apps/gamma")).unwrap();
        std::fs::write(d.join("apps/gamma/.git"), "gitdir: /elsewhere\n").unwrap();
        assert_eq!(super::names(&d), ["alpha", "beta-2", "gamma"]);
        assert!(super::names(&d.join("none")).is_empty());
        assert_eq!(super::split("apps/beta-2/src/lib.rs", ["alpha", "beta-2"]), Some(("beta-2", "src/lib.rs")));
        assert_eq!(super::split("apps/beta-20/x", ["beta-2"]), None);
        assert_eq!(super::split("apps/plain/x", ["alpha"]), None);
        assert!(!super::valid("makepad") && !super::valid("") && !super::valid("a/b") && super::valid("source-library"));
        assert_eq!((super::dir("alpha"), super::prefix("alpha")), ("apps/alpha".to_string(), "apps/alpha/".to_string()));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The buckets prove, and the deployments follow, the paired repository's main and each other
    /// nested checkout at the branch it is on; one on no branch, or on one with no commit yet, is
    /// followed by nothing.
    #[test]
    fn each_nested_checkout_is_followed_at_the_branch_it_is_on() {
        let d = std::env::temp_dir().join(format!("devhub-nested-branches-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let git = |dir: &std::path::Path, args: &[&str]| assert!(std::process::Command::new("git").arg("-C").arg(dir).args(args).env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap().status.success(), "git {args:?}");
        for (name, branch) in [("repo-b", "side"), ("repo-c", "main")] {
            let dir = d.join(super::dir(name));
            std::fs::create_dir_all(&dir).unwrap();
            git(&dir, &["init", "-q", "-b", branch]);
            git(&dir, &["-c", "user.name=t", "-c", "user.email=t", "commit", "-q", "--allow-empty", "-m", "one"]);
        }
        git(&d.join(super::dir("repo-c")), &["checkout", "-q", "--detach"]);
        // a fresh clone's branch with no commit yet: no tip to follow
        std::fs::create_dir_all(d.join(super::dir("repo-d"))).unwrap();
        git(&d.join(super::dir("repo-d")), &["init", "-q", "-b", "fresh"]);
        let followed = |pairs: &[(&str, &str)]| pairs.iter().map(|(n, b)| (n.to_string(), b.to_string())).collect::<Vec<_>>();
        assert_eq!(super::branches(&d), followed(&[(crate::state::PAIRED, "main"), ("repo-b", "side")]));
        // a lane's worktree of the checkout holds none of them: each followed one is the checkout's,
        // at its branch's tip, for the lane's runs to pin (11-19)
        let lane = std::env::temp_dir().join(format!("devhub-nested-lane-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&lane);
        assert!(super::tips(&d).is_empty() && super::hub(&d).is_none(), "no checkout: nothing to follow");
        git(&d, &["init", "-q", "-b", "work"]);
        git(&d, &["-c", "user.name=t", "-c", "user.email=t", "commit", "-q", "--allow-empty", "-m", "base"]);
        git(&d, &["worktree", "add", "-q", "-b", "rc-lane", &lane.to_string_lossy()]);
        let side = std::process::Command::new("git").arg("-C").arg(d.join(super::dir("repo-b"))).args(["rev-parse", "side"]).output().unwrap();
        let side = String::from_utf8_lossy(&side.stdout).trim().to_string();
        let tips = vec![("repo-b".to_string(), "side".to_string(), side.clone())];
        let canon = |p: Option<std::path::PathBuf>| p.map(|p| p.canonicalize().unwrap());
        assert_eq!(canon(super::hub(&lane)), canon(Some(d.clone())), "a lane's worktree reads the user's checkout");
        assert_eq!((super::tips(&lane), super::absent(&lane)), (tips.clone(), tips), "the lane lacks the followed repository");
        assert_eq!(super::absent_sides(&lane), format!(",repo-b=side@{side}"));
        assert_eq!(canon(super::checkout(&lane, "repo-b")), canon(Some(d.join(super::dir("repo-b")))), "read in the user's checkout");
        assert!(super::absent(&d).is_empty() && super::checkout(&lane, "repo-x").is_none(), "the checkout holds them all");
        // a job's rev naming it is pinned again in the lane, where it resolves in the user's
        // checkout (a carry gate's job found "no commit" in the lane, #7434), and the lane's
        // uncommitted files leave it alone: nothing of it is the lane's, nothing deleted
        let head = std::process::Command::new("git").arg("-C").arg(&lane).args(["rev-parse", "HEAD"]).output().unwrap();
        let head = String::from_utf8_lossy(&head.stdout).trim().to_string();
        assert_eq!(crate::lease::pin_revs(&lane, "rc-lane,repo-b=side"), Ok(format!("rc-lane@{head},repo-b=side@{side}")));
        let pin = crate::lease::Pin::at(&head).with("repo-b", &side);
        assert_eq!(crate::lease::ship::uncommitted(&lane, &pin), Ok((Vec::new(), Vec::new())));
        let _ = std::fs::remove_dir_all(&lane);
        let _ = std::fs::remove_dir_all(&d);
    }
}
