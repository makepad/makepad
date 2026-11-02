//! One arm64/macOS decision for the verifier, landing train and CI planner.

use super::ci::{self, Os, Tree};
use crate::state::Pair;
use std::path::Path;

pub struct Decision {
    pub required: bool,
    pub full: bool,
    pub summary: String,
}

impl Decision {
    fn required(why: &str) -> Decision {
        Decision { required: true, full: why == "full sweep", summary: format!("arm64: required ({why})") }
    }

    /// A resumed run keeps the decision made for its original baseline.
    pub fn read(path: &Path) -> Option<Decision> {
        let summary = std::fs::read_to_string(path).ok()?.trim().to_string();
        if !summary.starts_with("arm64: required (") && !summary.starts_with("arm64: skipped (") { return None; }
        Some(Decision { required: summary.starts_with("arm64: required ("), full: summary == "arm64: required (full sweep)", summary })
    }
}

fn under(path: &str, dir: &str) -> bool {
    path == dir || path.strip_prefix(dir).is_some_and(|s| s.starts_with('/'))
}

fn relevant_path(path: &str) -> bool {
    let file = path.rsplit('/').next().unwrap_or(path);
    file == "Cargo.lock" || file == "rust-toolchain" || file == "rust-toolchain.toml"
        || path.split('/').any(|p| p.split(['_', '-', '.']).any(|p| p == "toolchain" || p == "toolchains"))
        || path == ".cargo/config" || path == ".cargo/config.toml" || path.ends_with("/.cargo/config") || path.ends_with("/.cargo/config.toml")
        || path.split('/').any(|p| p.split(['_', '-', '.']).any(|p| matches!(p, "apple" | "macos" | "metal" | "objc" | "cocoa" | "ios")))
        || path.starts_with("platform/src/os/apple")
        || path.starts_with("libs/relax/backend/src/arm64")
        || (under(path, "libs/relax/backend/src/asm") && path.contains("arm64"))
        || (under(path, "libs/relax/codegen") && (path.contains("aarch64") || path.contains("arm64")))
        || under(path, "libs/relax/compiler/src/os")
        || (under(path, "libs/relax") && file.split(['_', '.']).any(|s| s == "cabi"))
}

/// Includes removed lines and hunk context: changing a body just below a cfg matters too.
/// Join lines before looking inside cfg(...) so multiline and nested cfgs work alike.
fn relevant_hunks(diff: &str) -> bool {
    let mut hunk = false;
    let mut text = String::new();
    for line in diff.lines() {
        if line.starts_with("diff --git ") { hunk = false; }
        if line.starts_with("@@") { hunk = true; text.push('\n'); continue; }
        if hunk && line.starts_with(['+', '-', ' ']) {
            text.extend(line[1..].chars().filter(|c| !c.is_whitespace()));
        }
    }
    for (at, _) in text.match_indices("cfg(").chain(text.match_indices("cfg!(")).chain(text.match_indices("cfg_attr(")) {
        let rest = &text[at..];
        let open = rest.find('(').unwrap();
        let mut depth = 1;
        let end = rest[open + 1..].char_indices().find_map(|(i, c)| {
            match c { '(' => depth += 1, ')' => depth -= 1, _ => {} }
            (depth == 0).then_some(open + 1 + i)
        }).unwrap_or(rest.len());
        let cfg = &rest[open + 1..end];
        if ["target_arch=\"aarch64\"", "target_os=\"macos\"", "target_os=\"ios\"", "target_vendor=\"apple\""].iter().any(|s| cfg.contains(s)) {
            return true;
        }
    }
    false
}

/// The apps booted on Macs, not their dependency closure. Read both trees so deleting an app
/// or removing its smoke script still counts. ci.toml proofs are not boot inputs. The no-codegen
/// leg also boots these two packages.
fn boot_changed(tree: &Tree, paths: &[String]) -> bool {
    let (_, scripts) = ci::discover(tree);
    let owns = |dir: &str| paths.iter().any(|p| under(p, dir) && p.rsplit('/').next() != Some("ci.toml"));
    scripts.iter().any(|sc| sc.dir != "." && !matches!(tree.read(&sc.path).as_deref().and_then(ci::pinned_os), Some(Os::Linux | Os::Windows)) && owns(&sc.dir))
        || owns("examples/counter") || owns("apps/calculator")
}

/// Compare the exact pair to its base in both repositories. Unknown history runs the Mac legs;
/// it must never turn a missing diff into a classification pass. Full sweeps bypass the filter.
pub fn classify(repo: &Path, base: Option<&Pair>, pair: &Pair, full: bool) -> Decision {
    if full { return Decision::required("full sweep"); }
    let Some(base) = base else { return Decision::required("no baseline") };
    let read = || -> Result<Decision, String> {
        let mut paths = Vec::new();
        let mut hunks = Vec::new();
        for (dir, prefix, from, to) in [(repo.to_path_buf(), "", &base.work, &pair.work), (repo.join(ci::COMMERCIAL), "apps/commercial/", &base.main, &pair.main)] {
            if from == to { continue; }
            if from.is_empty() || to.is_empty() { return Ok(Decision::required("repository added or removed")); }
            let names = super::git(&dir, &["diff", "--no-color", "--no-ext-diff", "--no-renames", "--name-only", "-z", from, to, "--"])?;
            paths.extend(names.split('\0').filter(|p| !p.is_empty()).map(|p| format!("{prefix}{p}")));
            hunks.push((dir, from, to));
        }
        paths.sort();
        paths.dedup();
        if let Some(path) = paths.iter().find(|p| relevant_path(p)) {
            return Ok(Decision::required(path));
        }
        for (dir, from, to) in hunks {
            let diff = super::git(&dir, &["diff", "--no-color", "--no-ext-diff", "--no-textconv", "--no-renames", "--unified=3", from, to, "--"])?;
            if relevant_hunks(&diff) { return Ok(Decision::required("arm64/Apple cfg in changed hunks")); }
        }
        if paths.iter().any(|p| p.starts_with("apps/") || p.starts_with("examples/")) {
            for p in [pair, base] {
                if boot_changed(&Tree::at(repo, p)?, &paths) {
                    return Ok(Decision::required("macOS smoke app files"));
                }
            }
        }
        Ok(Decision { required: false, full: false, summary: format!("arm64: skipped (no arm64/macOS-relevant change: {} files)", paths.len()) })
    };
    read().unwrap_or_else(|e| Decision::required(&format!("classification unavailable: {e}")))
}

/// Sunday UTC, once that Sunday's sweep has been attempted. The verifier's existing wake loop
/// owns the cadence; no extra timer or thread.
pub fn weekly_due(now: u64, last: u64) -> bool {
    const DAY: u64 = 86_400_000;
    (now / DAY + 4) % 7 == 0 && last / DAY < now / DAY
}
