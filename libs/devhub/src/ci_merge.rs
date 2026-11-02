//! Local Git configuration for proof files, including ordinary Git outside devhub.
use crate::lease::wt::git;
use std::path::Path;

/// Install per worktree: a lane must not replace the master's absolute driver path.
/// This only writes Git configuration/attributes; Cargo runs when Git invokes the driver.
pub fn install_merge_driver(repo: &Path, makepad_root: &Path) -> Result<(), String> {
    static INSTALL: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _install = INSTALL.lock().unwrap_or_else(|e| e.into_inner());
    let root = makepad_root.canonicalize().map_err(|e| format!("ci merge root: {e}"))?;
    let g = |args: &[&str]| git(Some(repo), None, args);
    let common = g(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let common = Path::new(common.trim());
    if g(&["config", "--local", "--bool", "extensions.worktreeConfig"]).unwrap_or_default().trim() != "true" {
        // Git requires these primary-worktree-only settings to leave the shared config
        // when worktreeConfig is enabled (particularly core.bare in node stores).
        let mut moved = Vec::new();
        for key in ["core.bare", "core.worktree"] {
            if let Ok(value) = g(&["config", "--local", "--get", key]) {
                if key == "core.bare" && value.trim() != "true" { continue; }
                g(&["config", "--file", &common.join("config.worktree").to_string_lossy(), key, value.trim_end()])?;
                moved.push(key);
            }
        }
        g(&["config", "--local", "extensions.worktreeConfig", "true"])?;
        for key in moved { g(&["config", "--local", "--unset-all", key])?; }
    }
    // Git invokes drivers through its POSIX shell, including Git for Windows. Remove
    // canonical Windows extended prefixes and quote paths with spaces/apostrophes.
    let quote = |path: &Path| {
        let text = path.to_string_lossy().replace('\\', "/");
        let text = if let Some(unc) = text.strip_prefix("//?/UNC/") { format!("//{unc}") }
            else { text.strip_prefix("//?/").unwrap_or(&text).to_string() };
        format!("'{}'", text.replace('\'', "'\\''"))
    };
    let driver = format!("cargo run -q --release --manifest-path {} --target-dir {} -- %O %A %B %P",
        quote(&root.join("tools/ci_merge/Cargo.toml")), quote(&root.join("target/ci_merge")));
    for (key, value) in [("merge.ci-proof.name", "ci.toml proofs"), ("merge.ci-proof.driver", driver.as_str())] {
        if g(&["config", "--worktree", "--get", key]).unwrap_or_default().trim_end() != value {
            g(&["config", "--worktree", key, value])?;
        }
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
