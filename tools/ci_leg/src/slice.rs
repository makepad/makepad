//! A licensed app's slice of the commercial repository ships complete (tools/ci slice.ci.splash,
//! ported): the slice of the tree's commercial commit, cut for the app's roots as a release builds
//! them (no default features), goes into a clean checkout of the tree's makepad commit at
//! `apps/commercial`, as a customer's Builder has it; its crates' outside references must be in
//! their declared includes (lint), and the roots must check there (under the JIT).

use crate::report::Run;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const COMMERCIAL: &str = "apps/commercial";
/// The features a slice is cut and checked with: none (no release names any).
const FEATURES: &[String] = &[];

fn git(args: &[&str], cwd: &Path) -> Result<String, String> {
    let o = Command::new("git").args(args).current_dir(cwd).stdin(Stdio::null()).output().map_err(|e| format!("git: {e}"))?;
    if !o.status.success() {
        return Err(format!("git {}: {}", args.join(" "), String::from_utf8_lossy(&o.stderr).trim()));
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

/// The slice checkout: a git worktree of the tree in `local/ci-slices/<app>` (outside every
/// workspace glob, never walked for scripts).
fn slice_dir(root: &Path, app: &str) -> PathBuf {
    root.join("local").join("ci-slices").join(app)
}

fn checkout(root: &Path, app: &str, roots: &[String]) -> Result<(PathBuf, String), String> {
    let commercial = root.join(COMMERCIAL);
    let slice = makepad_source_slice::slice(&commercial, "HEAD", app, roots, FEATURES)?;
    let dir = slice_dir(root, app);
    let tip = git(&["rev-parse", "HEAD"], root)?;
    if dir.join(".git").exists() {
        git(&["checkout", "-q", "-f", "--detach", &tip], &dir)?;
        git(&["clean", "-q", "-ffdx", "-e", "/Cargo.lock", "-e", "/apps/commercial/"], &dir)?;
    } else {
        git(&["worktree", "prune"], root)?;
        std::fs::create_dir_all(dir.parent().unwrap()).map_err(|e| e.to_string())?;
        git(&["worktree", "add", "-q", "--detach", &dir.display().to_string(), &tip], root)?;
    }
    // the slice is its own repository, whose objects are the commercial checkout's
    let sliced = dir.join(COMMERCIAL);
    if !sliced.join(".git").exists() {
        std::fs::create_dir_all(&sliced).map_err(|e| e.to_string())?;
        git(&["init", "-q"], &sliced)?;
    }
    // the commercial checkout's objects (its `.git` may be a gitlink file: ask git), every run: a
    // run that stopped after the init left none
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"], &commercial)?;
    let objects = Path::new(&common).join("objects").canonicalize().map_err(|e| format!("{common}/objects: {e}"))?;
    let info = sliced.join(".git/objects/info");
    std::fs::create_dir_all(&info).map_err(|e| e.to_string())?;
    std::fs::write(info.join("alternates"), format!("{}\n", objects.display())).map_err(|e| e.to_string())?;
    git(&["checkout", "-q", "-f", "--detach", &slice.commit], &sliced)?;
    git(&["clean", "-q", "-ffdx"], &sliced)?;
    let crates: Vec<String> = slice.crates.iter().map(|(name, dir)| format!("{name} ({dir})")).collect();
    Ok((dir, format!("slice {} of commercial {} for {app}: {} crates, {} files, {} bytes: {}", &slice.commit[..12], &slice.source[..12], slice.crates.len(), slice.files, slice.bytes, crates.join(", "))))
}

fn lint(root: &Path, roots: &[String]) -> Result<String, String> {
    let findings = makepad_source_slice::lint(&root.join(COMMERCIAL), "HEAD", roots, FEATURES)?;
    if findings.is_empty() {
        return Ok(String::new());
    }
    let shown: Vec<String> = findings.iter().take(40).map(|f| f.to_string()).collect();
    Err(format!("{} references outside their crate are not in its [package.metadata.commercial] include:\n{}", findings.len(), shown.join("\n")))
}

/// `makepad check --no-default-features <root>` in the slice checkout, per root.
fn check(dir: &Path, roots: &[String]) -> Result<String, String> {
    let loader = crate::loader_cmd();
    let mut said = Vec::new();
    for r in roots {
        let o = Command::new(&loader[0]).args(&loader[1..]).args(["check", r, "--no-default-features"]).current_dir(dir).stdin(Stdio::null()).output().map_err(|e| format!("makepad check: {e}"))?;
        let text = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
        let summary = text.lines().rev().find(|l| l.trim_end().ends_with(" errors")).unwrap_or("").to_string();
        let errors = summary.trim_end().strip_suffix(" errors").and_then(|h| h.rsplit([' ', ',']).next()).and_then(|n| n.parse::<u64>().ok());
        if !o.status.success() || errors.is_some_and(|n| n > 0) {
            let lines: Vec<&str> = text.lines().filter(|l| l.contains("error")).take(30).collect();
            return Err(format!("makepad check {r} in the slice: exit {}; {summary}\n{}", o.status.code().unwrap_or(-1), lines.join("\n")));
        }
        said.push(format!("{r}: {summary}"));
    }
    Ok(said.join("; "))
}

/// The slice shard of `app`: checkout, lint, check.
pub fn run(root: PathBuf, app: &str, roots: &[String]) -> i32 {
    let mut run = Run::new(root.clone(), root.join("local/ci-leg").join(format!("slice-{app}")));
    if !root.join(COMMERCIAL).join(".git").exists() {
        run.fail("slice", "no commercial checkout in the tree");
        return run.finish(&format!("slice {app}"));
    }
    let i = run.begin(&format!("slice:{app} / slice"));
    let dir = match checkout(&root, app, roots) {
        Ok((dir, summary)) => {
            run.end(i, "passed", &summary);
            Some(dir)
        }
        Err(e) => {
            run.end(i, "failed", &e);
            None
        }
    };
    let i = run.begin(&format!("slice:{app} / lint"));
    match lint(&root, roots) {
        Ok(_) => run.end(i, "passed", ""),
        Err(e) => run.end(i, "failed", &e),
    }
    if let Some(dir) = dir {
        let i = run.begin(&format!("slice:{app} / check"));
        match check(&dir, roots) {
            Ok(s) => run.end(i, "passed", &s),
            Err(e) => run.end(i, "failed", &e),
        }
    }
    run.finish(&format!("slice {app}"))
}
