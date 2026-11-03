//! The private-data check over every tracked file, for CI and the land gates: it runs
//! `tools/private_check/scan.sh tree`, the one matcher the git hooks and devhub share. The rules
//! are `tools/private_check/rules` plus the untracked `~/.makepad/private_patterns`; scan.sh says
//! how they read and AGENTS.md says what to write instead.
use std::path::PathBuf;
use std::process::{Command, ExitCode};

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// The POSIX shell scan.sh runs under: `sh`, or on Windows the one Git for Windows ships
/// (`<git>/mingw64/libexec/git-core` is git's exec path, `<git>/bin/sh.exe` its shell).
fn shell() -> Result<PathBuf, String> {
    if !cfg!(windows) {
        return Ok(PathBuf::from("sh"));
    }
    let exec = git(&["--exec-path"]).ok_or("git --exec-path failed")?;
    let mut root = PathBuf::from(exec);
    for _ in 0..3 {
        root.pop();
    }
    let sh = root.join("bin").join("sh.exe");
    if sh.is_file() {
        Ok(sh)
    } else {
        Err(format!("no Git for Windows shell at {}", sh.display()))
    }
}

fn main() -> ExitCode {
    let Some(top) = git(&["rev-parse", "--show-toplevel"]) else {
        eprintln!("private check: not inside a git checkout");
        return ExitCode::from(2);
    };
    let sh = match shell() {
        Ok(sh) => sh,
        Err(e) => {
            eprintln!("private check: {e}");
            return ExitCode::from(2);
        }
    };
    let scan = PathBuf::from(&top).join("tools").join("private_check").join("scan.sh");
    match Command::new(&sh).arg(&scan).arg("tree").current_dir(&top).status() {
        Ok(status) if status.success() => ExitCode::SUCCESS,
        Ok(status) => ExitCode::from(status.code().unwrap_or(1).clamp(1, 255) as u8),
        Err(e) => {
            eprintln!("private check: cannot run {} {}: {e}", sh.display(), scan.display());
            ExitCode::from(2)
        }
    }
}
