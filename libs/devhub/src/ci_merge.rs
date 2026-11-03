//! Local Git configuration for proof files, including ordinary Git outside devhub.
//!
//! The configuration is read and written as files (makepad-git's config module): checking a
//! worktree that has the driver already costs a few small reads and never runs git.
use makepad_git::config;
use std::path::{Path, PathBuf};

/// The private and the shared git directory of the worktree whose root is `repo`.
fn git_dirs(repo: &Path) -> Result<(PathBuf, PathBuf), String> {
    let paths = makepad_git::repository_paths(repo).map_err(|e| format!("ci merge: {}: {e}", repo.display()))?;
    let paths = paths.ok_or_else(|| format!("ci merge: {} is not a worktree root", repo.display()))?;
    Ok((paths.git_dir, paths.common_dir))
}

/// The driver command for the tree at `root` (canonical).
fn driver(root: &Path) -> String {
    // Git invokes drivers through its POSIX shell, including Git for Windows. Remove
    // canonical Windows extended prefixes and quote paths with spaces/apostrophes.
    let quote = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        let text = if let Some(unc) = text.strip_prefix("//?/UNC/") { format!("//{unc}") }
            else { text.strip_prefix("//?/").unwrap_or(&text).to_string() };
        format!("'{}'", text.replace('\'', "'\\''"))
    };
    format!("cargo run -q --release --manifest-path {} --target-dir {} -- %O %A %B %P",
        quote(&root.join("tools/ci_merge/Cargo.toml")), quote(&root.join("target/ci_merge")))
}

/// Install per worktree: a lane must not replace the master's absolute driver path.
/// This only writes Git configuration/attributes; Cargo runs when Git invokes the driver.
pub fn install_merge_driver(repo: &Path, makepad_root: &Path) -> Result<(), String> {
    static INSTALL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _install = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let root = makepad_root.canonicalize().map_err(|e| format!("ci merge root: {e}"))?;
    let (git_dir, common) = git_dirs(repo)?;
    let e = |e: makepad_git::GitError| format!("ci merge config: {e}");
    let shared = common.join("config");
    let mut text = config::read_file(&shared).map_err(e)?;
    if config::get(&text, "extensions.worktreeConfig").map_err(e)?.as_deref().and_then(config::parse_bool) != Some(true) {
        // Git requires these primary-worktree-only settings to leave the shared config
        // when worktreeConfig is enabled (particularly core.bare in node stores).
        for key in ["core.bare", "core.worktree"] {
            if let Some(value) = config::get(&text, key).map_err(e)? {
                if key == "core.bare" && value.trim() != "true" { continue; }
                config::set_in_file(&common.join("config.worktree"), key, &value).map_err(e)?;
                text = config::unset_all(&text, key).map_err(e)?;
            }
        }
        text = config::set(&text, "extensions.worktreeConfig", "true").map_err(e)?;
        config::write_file(&shared, &text).map_err(e)?;
    }
    // `git config --worktree`: the main worktree's is `<common>/config.worktree`, a linked one's
    // `<common>/worktrees/<name>/config.worktree` (its private dir)
    let own = git_dir.join("config.worktree");
    for (key, value) in [("merge.ci-proof.name", "ci.toml proofs".to_string()), ("merge.ci-proof.driver", driver(&root))] {
        config::set_in_file(&own, key, &value).map_err(e)?;
    }
    // Cover older revisions and the separate commercial repo too, without dirtying them.
    let attributes = common.join("info/attributes");
    let mut text = match std::fs::read_to_string(&attributes) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(format!("ci merge attributes: {e}")),
    };
    if !text.lines().any(|l| l.trim() == "ci.toml merge=ci-proof") {
        if !text.is_empty() && !text.ends_with('\n') { text.push('\n'); }
        text.push_str("ci.toml merge=ci-proof\n");
        std::fs::create_dir_all(common.join("info")).map_err(|e| e.to_string())?;
        std::fs::write(attributes, text).map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git(dir: &Path, args: &[&str]) -> String {
        let o = std::process::Command::new("git").args(args).current_dir(dir).env("GIT_CONFIG_NOSYSTEM", "1").env("GIT_CONFIG_GLOBAL", "/dev/null").output().unwrap();
        assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).trim().to_string()
    }

    /// What git itself reads back after the install, in the main worktree and a linked one; a
    /// second install changes no file.
    #[test]
    fn git_reads_the_driver_it_installs_without_git() {
        let dir = std::env::temp_dir().join(format!("devhub-ci-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (main, linked) = (dir.join("m"), dir.join("l k"));
        std::fs::create_dir_all(&main).unwrap();
        git(&main, &["init", "-q", "-b", "work"]);
        git(&main, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "a"]);
        git(&main, &["worktree", "add", "-q", "-b", "rc-x", linked.to_str().unwrap()]);
        for wt in [&main, &linked] {
            install_merge_driver(wt, wt).unwrap();
            assert_eq!(git(wt, &["config", "--bool", "extensions.worktreeConfig"]), "true");
            assert_eq!(git(wt, &["config", "--worktree", "--get", "merge.ci-proof.name"]), "ci.toml proofs");
            assert_eq!(git(wt, &["config", "--worktree", "--get", "merge.ci-proof.driver"]), driver(&wt.canonicalize().unwrap()));
        }
        let (git_dir, _) = git_dirs(&linked).unwrap();
        let stamp = std::fs::metadata(git_dir.join("config.worktree")).unwrap().modified().unwrap();
        let before = std::fs::read_to_string(dir.join("m/.git/config")).unwrap();
        install_merge_driver(&linked, &linked).unwrap();
        assert_eq!(std::fs::metadata(git_dir.join("config.worktree")).unwrap().modified().unwrap(), stamp);
        assert_eq!(std::fs::read_to_string(dir.join("m/.git/config")).unwrap(), before);
        assert_eq!(git(&main, &["check-attr", "merge", "ci.toml"]), "ci.toml: merge: ci-proof");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
