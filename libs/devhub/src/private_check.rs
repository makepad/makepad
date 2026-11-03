//! The private-data check at devhub's own commit points. A land merge is written with
//! commit-tree, so no git hook sees it; this runs the same matcher the hooks and CI run,
//! `tools/private_check/scan.sh` (rules: that directory's `rules` and `allow`, plus the
//! untracked `~/.makepad/private_patterns`). A hit names `<file>:<line>: <rule>`, never the text.
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// The scanner of the checkout devhub is built from, as the hooks use the common checkout's: the
/// repository a check reads may be any (a lane's, a test's), the rules are this checkout's.
fn scanner_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tools/private_check")
}

/// The scanner's files (scan.sh, its rules and allow list) copied to a fresh folder `sh` can run
/// them from, for one check: devhub reads them through its own std, and under the JIT's overlay
/// the checkout is a virtual tree that no other process sees. Fresh per check, so the rules are
/// the checkout's as they stand. Removed when dropped.
struct Staged(PathBuf);

impl Staged {
    fn new() -> Result<Self, Vec<String>> {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let staged = Staged(std::env::temp_dir().join(format!("devhub-private-check-{}-{n}", std::process::id())));
        let fail = |what: &Path, e: std::io::Error| vec![format!("private check: staging {}: {e}", what.display())];
        std::fs::create_dir_all(&staged.0).map_err(|e| fail(&staged.0, e))?;
        let src = scanner_dir();
        for entry in std::fs::read_dir(&src).map_err(|e| fail(&src, e))? {
            let entry = entry.map_err(|e| fail(&src, e))?;
            if entry.file_type().map_err(|e| fail(&entry.path(), e))?.is_file() {
                let body = std::fs::read(entry.path()).map_err(|e| fail(&entry.path(), e))?;
                std::fs::write(staged.0.join(entry.file_name()), body).map_err(|e| fail(&entry.path(), e))?;
            }
        }
        Ok(staged)
    }
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Checks the added lines of `diff` (unified, `git diff -U0` shape) and the `messages`, given as
/// (label, text). Err holds every hit and a last `private: <N> hit(s): ...` line, or why the
/// check could not run.
pub fn private_check(root: &Path, diff: &str, messages: &[(String, String)]) -> Result<(), Vec<String>> {
    if diff.is_empty() && messages.is_empty() {
        return Ok(());
    }
    let staged = Staged::new()?;
    let mut hits = Vec::new();
    if !diff.is_empty() {
        hits.extend(scan(&staged, root, "diff", diff)?);
    }
    if !messages.is_empty() {
        let mut text = String::new();
        for (label, body) in messages {
            text.push('\u{1}');
            text.push_str(label);
            text.push('\n');
            text.push_str(body);
            if !body.ends_with('\n') {
                text.push('\n');
            }
        }
        hits.extend(scan(&staged, root, "messages", &text)?);
    }
    if hits.is_empty() {
        return Ok(());
    }
    hits.push(format!("private: {} hit(s): write \"a Linux node\", \"the Mac\", \"a 24 GB GPU\" or 192.0.2.x and repo-relative paths (AGENTS.md)", hits.len()));
    Err(hits)
}

/// The land gate: what merging `sha` onto `base` as `tree` adds, and the messages it brings.
pub fn merge_gate(root: &Path, base: &str, sha: &str, tree: &str) -> Result<(), Vec<String>> {
    let diff = git(root, &["diff", "-U0", "--no-color", "--no-ext-diff", "--no-renames", base, tree])?;
    let log = git(root, &["log", "--format=%x01%h%n%B", &format!("{base}..{sha}")])?;
    let messages: Vec<(String, String)> = log
        .split('\u{1}')
        .filter(|m| !m.is_empty())
        .map(|m| {
            let (sha, body) = m.split_once('\n').unwrap_or((m, ""));
            (format!("message of {sha}"), body.to_string())
        })
        .collect();
    private_check(root, &diff, &messages)
}

fn git(root: &Path, args: &[&str]) -> Result<String, Vec<String>> {
    let out = Command::new("git").arg("-C").arg(root).args(args).output().map_err(|e| vec![format!("private check: git: {e}")])?;
    if !out.status.success() {
        return Err(vec![format!("private check: git {} failed: {}", args[0], String::from_utf8_lossy(&out.stderr).trim())]);
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The commit-msg hook of `repo`'s shared git dir (every worktree of it: a lane's makepad and
/// commercial halves alike) runs the private check on each message as it is written: one
/// naming a machine, an address or a user path is refused at the commit, never first at the
/// land (11-03: three such commits spread into 10 lane branches). `scan` is the makepad
/// checkout's `tools/private_check/scan.sh`. The tracked hook (tools/private_check/hooks) is kept,
/// devhub's own is brought up to `scan`; any other hook is an error, never overwritten.
pub fn install_message_check(repo: &Path, scan: &Path) -> Result<(), String> {
    const CHECK: &str = "# The private-data check on the commit message (AGENTS.md)";
    const OURS: &str = "installed by devhub";
    let p = crate::gitread::paths(repo).ok_or_else(|| format!("message check: no repository at {}", repo.display()))?;
    let hook = p.common_dir.join("hooks/commit-msg");
    let scan = scan.to_string_lossy();
    if scan.contains('\'') {
        return Err(format!("message check: {scan}: a quote in the path"));
    }
    let body = format!("#!/bin/sh\n{CHECK}, {OURS} for every\n# worktree of this repository. Never commit with --no-verify.\nexec sh '{scan}' msg \"$1\"\n");
    match std::fs::read_to_string(&hook) {
        Ok(text) if text == body || (text.contains(CHECK) && !text.contains(OURS)) => return Ok(()),
        Ok(text) if !text.contains(CHECK) => return Err(format!("message check: {} is a commit-msg hook of its own; the private check needs that hook", hook.display())),
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(format!("message check: {}: {e}", hook.display())),
    }
    let fail = |e: std::io::Error| format!("message check: {}: {e}", hook.display());
    std::fs::create_dir_all(p.common_dir.join("hooks")).map_err(fail)?;
    let next = hook.with_extension("next");
    std::fs::write(&next, body).map_err(fail)?;
    #[cfg(unix)]
    std::fs::set_permissions(&next, std::os::unix::fs::PermissionsExt::from_mode(0o755)).map_err(fail)?;
    std::fs::rename(&next, &hook).map_err(fail)
}

/// The makepad checkout's scanner, for [`install_message_check`].
pub fn scan_in(makepad_root: &Path) -> PathBuf {
    makepad_root.join("tools/private_check/scan.sh")
}

/// One scan.sh run over `input`; Ok holds its hit lines (none when clean).
fn scan(staged: &Staged, root: &Path, mode: &str, input: &str) -> Result<Vec<String>, Vec<String>> {
    let script = staged.0.join("scan.sh");
    let mut child = Command::new("sh")
        .arg(&script)
        .arg(mode)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| vec![format!("private check: cannot run {}: {e}", script.display())])?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let input = input.to_string();
    // the scan writes hits while it reads: feed it from a thread so neither pipe fills up
    let feed = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let out = child.wait_with_output().map_err(|e| vec![format!("private check: {mode}: {e}")])?;
    let _ = feed.join();
    match out.status.code() {
        Some(0) => Ok(Vec::new()),
        Some(1) => Ok(String::from_utf8_lossy(&out.stdout).lines().filter(|l| !l.starts_with("private: ")).map(str::to_string).collect()),
        _ => Err(vec![format!("private check: {} {mode} failed: {}", script.display(), String::from_utf8_lossy(&out.stderr).trim())]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        std::env::temp_dir()
    }

    #[test]
    fn a_clean_diff_and_message_pass_and_hits_name_file_line_and_rule() {
        let clean = "+++ b/a.rs\n@@ -0,0 +1,2 @@\n+fn main() {}\n+// on 192.0.2.10 and 10.77.0.3\n";
        assert_eq!(private_check(&root(), clean, &[("message of x".into(), "a clean subject\n".into())]), Ok(()));
        // built at run time, so this file never holds the shapes it tests
        let dirty = format!("+++ b/a.rs\n@@ -0,0 +1,2 @@\n+ok\n+let host = \"10.{}.2.3\";\n", 1);
        let message = format!("subject\n\nfrom /{}/somebody/x\n", "home");
        let hits = private_check(&root(), &dirty, &[("message of y".into(), message)]).unwrap_err();
        assert_eq!(hits[..2], ["a.rs:2: lan-address".to_string(), "message of y:3: home-path".to_string()]);
        assert!(hits[2].starts_with("private: 2 hit(s)"), "{hits:?}");
    }

    /// A lane's commit with a private shape in its message is refused as it is written, in both
    /// repositories; a clean one goes in, and the hook stays as it is when installed again.
    #[test]
    fn a_commit_message_is_checked_when_it_is_written() {
        let staged = Staged::new().unwrap();
        let d = std::env::temp_dir().join(format!("devhub-msg-check-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let git = |dir: &Path, args: &[&str]| crate::git_exec::command().arg("-C").arg(dir).args(args).output().unwrap();
        for repo in ["makepad", "commercial"] {
            let r = d.join(repo);
            std::fs::create_dir_all(&r).unwrap();
            // (no template: a machine's own template hooks are not this test's)
            assert!(git(&r, &["init", "-q", "--template=", "-b", "work"]).status.success());
            install_message_check(&r, &d.join("moved/scan.sh")).unwrap();
            install_message_check(&r, &staged.0.join("scan.sh")).unwrap();
            install_message_check(&r, &staged.0.join("scan.sh")).unwrap();
            let lane = d.join(format!("{repo}-lane"));
            assert!(git(&r, &["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "--allow-empty", "-m", "base"]).status.success());
            assert!(git(&r, &["worktree", "add", "-q", "-b", "rc-lane", lane.to_str().unwrap()]).status.success());
            let dirty = format!("subject\n\nfrom /{}/somebody/x\n", "home");
            let refused = git(&lane, &["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "--allow-empty", "-m", &dirty]);
            assert!(!refused.status.success(), "{repo}: {}", String::from_utf8_lossy(&refused.stderr));
            let said = format!("{}{}", String::from_utf8_lossy(&refused.stdout), String::from_utf8_lossy(&refused.stderr));
            assert!(said.contains("home-path"), "{repo}: {said}");
            assert!(git(&lane, &["-c", "user.name=t", "-c", "user.email=t@example.com", "commit", "-q", "--allow-empty", "-m", "a clean subject"]).status.success(), "{repo}");
        }
        std::fs::write(d.join("makepad/.git/hooks/commit-msg"), "#!/bin/sh\nexit 0\n").unwrap();
        assert!(install_message_check(&d.join("makepad"), &staged.0.join("scan.sh")).is_err(), "a hook of its own is never overwritten");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The scanner runs from a staged copy that is gone after the check.
    #[test]
    fn the_staged_scanner_is_the_checkouts_and_is_removed_after() {
        let staged = Staged::new().unwrap();
        let dir = staged.0.clone();
        for f in ["scan.sh", "rules", "allow"] {
            assert_eq!(std::fs::read(dir.join(f)).unwrap(), std::fs::read(scanner_dir().join(f)).unwrap(), "{f}");
        }
        drop(staged);
        assert!(!dir.exists());
    }
}
