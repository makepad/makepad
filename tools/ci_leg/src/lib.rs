//! `ci-leg`: the CI shards devhub runs (libs/devhub/src/orch/ci.rs) that are more than one command,
//! under the JIT (`makepad run makepad-ci-leg -- ...`) in the lease's tree:
//! - `smoke <ci.splash> --dir <dir> [--target <package>:<binary>:<cwd>]`: one app's smoke script
//!   (the CI box's ci.splash runner, ported from tools/ci): its apps launch hidden and muted under
//!   the JIT, are driven over makepad-remote, grabbed, and every grab is judged by the vision model on
//!   the fastest AI fleet hub that has or can load it ([`judge`]);
//! - `slice <app> <root,...>`: a licensed slice of the commercial repository, cut into a clean
//!   checkout of the tree's commit, linted and checked ([`slice`]).
//!
//! Each prints its steps (`STEP <state> <name> (<s> s)`), the lines that say why one failed
//! (`CIERR ...`, what devhub's status shows) and one verdict line, `CI-OK ...` or `CI-FAIL ...`;
//! the exit code is the verdict.

pub mod judge;
pub mod report;
pub mod slice;
pub mod smoke;

/// The JIT loader for `makepad run` (an app launch): `MAKEPAD_CI_LOADER`, else this executable when
/// it is the loader (`makepad run` hosts this program in-process; a spawn of it with `run` first
/// launches another program), else the loader through cargo (a ~0.2 s no-op when warm).
pub fn loader() -> Vec<String> {
    if let Ok(l) = std::env::var("MAKEPAD_CI_LOADER") {
        if !l.is_empty() {
            return vec![l];
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if exe.file_stem().is_some_and(|s| s == "makepad") {
            return vec![exe.to_string_lossy().into_owned()];
        }
    }
    ["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--"].iter().map(|s| s.to_string()).collect()
}

/// The JIT loader for every other command (`inspect`, `check`): through cargo, since a spawn of
/// this program's own executable with anything but `run` first runs this program again.
pub fn loader_cmd() -> Vec<String> {
    if let Ok(l) = std::env::var("MAKEPAD_CI_LOADER") {
        if !l.is_empty() {
            return vec![l];
        }
    }
    ["cargo", "run", "--profile", "host", "-q", "-p", "makepad-run", "--bin", "makepad", "--"].iter().map(|s| s.to_string()).collect()
}
