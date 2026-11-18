//! `ci-leg`: the CI shards devhub runs (libs/devhub/src/orch/ci.rs) that are more than one command,
//! under the JIT (`makepad run makepad-ci-leg -- ...`) in the lease's tree:
//! - `smoke <ci.splash> --dir <dir> [--target <package>:<binary>:<cwd>]`: one app's smoke script
//!   (the ci.splash runner): its apps launch hidden and muted under
//!   the JIT, are driven over makepad-remote and grabbed; each acceptance (grab + text) is saved in
//!   the lease target, and devhub judges them on the master with makepad-ci-judge;
//! - `slice <app> <root,...>`: a licensed slice of the commercial repository, cut into a clean
//!   checkout of the tree's commit, linted and checked ([`slice`]).
//!
//! Each prints its steps (`STEP <state> <name> (<s> s)`), the lines that say why one failed
//! (`CIERR ...`, what devhub's status shows) and one verdict line, `CI-OK ...` or `CI-FAIL ...`;
//! the exit code is the verdict. A smoke's zero exit covers the script's flow; devhub judges the
//! saved acceptances before it passes the shard.

pub mod report;
pub mod slice;
pub mod smoke;

use std::ffi::OsString;
use std::path::PathBuf;

/// The JIT loader a leg launches its apps with (`run`) and checks a slice with (`check`): exactly
/// the executable it is given, `MAKEPAD_CI_LOADER`, else the loader this leg runs under
/// (`RELAX_LOADER`: the JIT's runner sets it at its start to the loader it was started with,
/// whatever that file's name), with the variable that named it. Neither: an error, never a
/// stand-in. It took this executable only when named `makepad` and otherwise rebuilt the loader
/// through cargo from the tree's sources, so an A/B of two renamed loaders ran the same new code
/// in both arms and nothing said so (11-17). An app's inspect commands go to its own socket.
pub fn loader() -> Result<(PathBuf, &'static str), String> {
    pick_loader(std::env::var_os("MAKEPAD_CI_LOADER"), std::env::var_os("RELAX_LOADER"))
}

fn pick_loader(ci: Option<OsString>, relax: Option<OsString>) -> Result<(PathBuf, &'static str), String> {
    for (var, value) in [("MAKEPAD_CI_LOADER", ci), ("RELAX_LOADER", relax)] {
        if let Some(p) = value.filter(|p| !p.is_empty()) {
            let p = PathBuf::from(p);
            if !p.is_file() {
                return Err(format!("{var} names {}, which is no file: give the loader's path", p.display()));
            }
            return Ok((p, var));
        }
    }
    Err("no loader to run the apps with: set MAKEPAD_CI_LOADER to the loader's path, or run ci-leg under the JIT (`makepad run makepad-ci-leg`), which sets RELAX_LOADER".to_string())
}

/// The line that says which loader a leg runs (`CI-LOADER <path> (<variable>; <bytes>, modified
/// <unix s>)`), at its start: what tells two arms' loaders apart.
pub fn loader_line(path: &std::path::Path, var: &str) -> String {
    let meta = std::fs::metadata(path).ok();
    let modified = meta.as_ref().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
    format!("CI-LOADER {} ({var}; {} bytes, modified {modified})", path.display(), meta.map_or(0, |m| m.len()))
}

#[cfg(test)]
mod tests {
    use super::pick_loader;

    /// The loader is exactly the one named, MAKEPAD_CI_LOADER first; a name that is no file, or no
    /// name at all, is an error, never a rebuild through cargo (11-17).
    #[test]
    fn the_loader_is_the_one_named_or_an_error() {
        let file = std::env::current_exe().unwrap();
        let other = file.with_file_name("no-such-loader");
        let os = |p: &std::path::Path| Some(p.as_os_str().to_owned());
        assert_eq!(pick_loader(os(&file), os(&other)), Ok((file.clone(), "MAKEPAD_CI_LOADER")), "MAKEPAD_CI_LOADER first, whatever its name");
        assert_eq!(pick_loader(Some("".into()), os(&file)), Ok((file.clone(), "RELAX_LOADER")), "else the loader this leg runs under");
        assert!(pick_loader(os(&other), os(&file)).unwrap_err().starts_with("MAKEPAD_CI_LOADER names"), "a name that is no file is refused, not passed over");
        assert!(pick_loader(None, None).unwrap_err().starts_with("no loader"), "no loader: refused");
    }
}
