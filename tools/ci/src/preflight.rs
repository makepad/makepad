//! `ci --preflight`: the cheap part of CI on a developer's checkout, before a
//! push. The packages the checkout changed against a base (origin/work) pick
//! the same `ci.splash` scripts the CI box runs; each runs in preflight mode:
//! its other-platform checks (`ci.check_targets`, installed targets only) and
//! host `cargo check`s run for real, and so do the release builds of what it
//! launches. Nothing is launched, grabbed or judged, and no tests run.
//!
//! An app's change runs that app's script. A library's change runs the
//! workspace script (host check of the whole workspace, other-platform
//! checks of the core crates it reaches); `--dependents` adds the script of
//! every app that depends on the library.
use crate::{
    process::{Control, Result},
    report::{self, Run, Stage},
    smoke::{self, Script},
    watch::{self, Config},
};
use makepad_strict_json::{self as json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

pub struct Options {
    pub base_ref: String,
    pub dependents: bool,
    pub report: Option<PathBuf>,
    /// Test these packages as if they had changed, instead of the diff.
    pub packages: Vec<String>,
}

/// A workspace member: its name and its directory relative to the root.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub name: String,
    pub dir: PathBuf,
}

#[derive(Clone, Debug, Default)]
pub struct Selection {
    /// Packages that own a changed file.
    pub changed: BTreeSet<String>,
    /// The changed packages and every workspace member that depends on them.
    pub affected: BTreeSet<String>,
    /// Scripts to run, each with the package it tests (None: the workspace script).
    pub scripts: Vec<(Script, Option<String>)>,
    /// Changed files no workspace member owns (docs, the root manifest).
    pub unowned: Vec<PathBuf>,
}

/// The member owning `file`: the one whose directory is its longest prefix.
pub fn owner<'a>(members: &'a [Member], file: &Path) -> Option<&'a Member> {
    members
        .iter()
        .filter(|m| !m.dir.as_os_str().is_empty() && file.starts_with(&m.dir))
        .max_by_key(|m| m.dir.components().count())
}

/// `changed` plus everything that reaches it through `reverse` (package ->
/// the members depending on it directly).
pub fn affected(changed: &BTreeSet<String>, reverse: &BTreeMap<String, BTreeSet<String>>) -> BTreeSet<String> {
    let mut all = changed.clone();
    let mut stack: Vec<String> = changed.iter().cloned().collect();
    while let Some(p) = stack.pop() {
        for user in reverse.get(&p).into_iter().flatten() {
            if all.insert(user.clone()) {
                stack.push(user.clone());
            }
        }
    }
    all
}

/// What to run for these changed files. `scripts` is the CI's own discovery
/// (`smoke::discover`), so preflight runs exactly the scripts the box runs.
pub fn select(
    files: &[PathBuf],
    members: &[Member],
    reverse: &BTreeMap<String, BTreeSet<String>>,
    scripts: &[Script],
    dependents: bool,
) -> Selection {
    let mut selection = Selection::default();
    let mut workspace_wide = false;
    for file in files {
        match owner(members, file) {
            Some(m) => {
                selection.changed.insert(m.name.clone());
            }
            None => {
                // The root manifest and lockfile reach every crate.
                if matches!(file.to_str(), Some("Cargo.toml" | "Cargo.lock" | "rust-toolchain" | "rust-toolchain.toml")) {
                    workspace_wide = true;
                }
                selection.unowned.push(file.clone());
            }
        }
    }
    selection.affected = if workspace_wide {
        members.iter().map(|m| m.name.clone()).collect()
    } else {
        affected(&selection.changed, reverse)
    };
    let package_of = |script: &Script| -> Option<String> {
        script.target.as_ref().map(|t| t.package.clone()).or_else(|| {
            let dir = script.path.parent().unwrap_or(Path::new(""));
            members.iter().find(|m| !dir.as_os_str().is_empty() && m.dir == dir).map(|m| m.name.clone())
        })
    };
    let tested: BTreeSet<String> = scripts.iter().filter_map(package_of).collect();
    let library_changed = workspace_wide || selection.changed.iter().any(|p| !tested.contains(p));
    for script in scripts {
        let package = package_of(script);
        let run = match &package {
            Some(p) => selection.changed.contains(p) || (dependents && selection.affected.contains(p)),
            None => script.path == Path::new("ci.splash") && library_changed,
        };
        if run {
            selection.scripts.push((script.clone(), package));
        }
    }
    selection
}

fn git(root: &Path, args: &[&str], control: &Control) -> Result<String> {
    watch::probe("git", &report::strings(args), root, control)
}

/// Files changed in the checkout against the merge base with `base_ref`:
/// commits, uncommitted edits and untracked files, relative to the root.
pub fn changed_files(root: &Path, base_ref: &str, control: &Control) -> Result<(String, Vec<PathBuf>)> {
    if base_ref.starts_with('-') {
        return Err("invalid base".into());
    }
    let base = git(root, &["merge-base", base_ref, "HEAD"], control)?.trim().to_string();
    let mut files = BTreeSet::new();
    for line in git(root, &["diff", "--name-only", "--no-renames", &base], control)?
        .lines()
        .chain(git(root, &["ls-files", "--others", "--exclude-standard"], control)?.lines())
    {
        if !line.trim().is_empty() {
            files.insert(PathBuf::from(line.trim()));
        }
    }
    Ok((base, files.into_iter().collect()))
}

/// Workspace members and who depends on whom, from `cargo metadata`.
pub fn workspace(root: &Path, control: &Control) -> Result<Workspace> {
    let text = watch::probe_with_timeout(
        "cargo",
        &report::strings(&["metadata", "--format-version=1"]),
        root,
        control,
        300,
    )?;
    parse_metadata(root, &text)
}

/// Members, reverse dependencies and the target directory cargo builds
/// into here (its config or environment may point it at a shared one).
pub type Workspace = (Vec<Member>, BTreeMap<String, BTreeSet<String>>, Option<PathBuf>);
pub fn parse_metadata(root: &Path, text: &str) -> Result<Workspace> {
    let doc = text
        .lines()
        .find_map(|l| json::parse_depth(l.as_bytes(), 128).ok().filter(|v| v.get("packages").is_some()))
        .ok_or("cargo metadata printed no package list")?;
    let members: BTreeSet<&str> = doc
        .get("workspace_members")
        .and_then(Value::as_arr)
        .unwrap_or(&[])
        .iter()
        .filter_map(Value::as_str)
        .collect();
    let mut names = BTreeMap::new();
    let mut result = Vec::new();
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    for p in doc.get("packages").and_then(Value::as_arr).unwrap_or(&[]) {
        let (Some(id), Some(name), Some(manifest)) = (
            p.get("id").and_then(Value::as_str),
            p.get("name").and_then(Value::as_str),
            p.get("manifest_path").and_then(Value::as_str),
        ) else {
            continue;
        };
        if !members.contains(id) {
            continue;
        }
        names.insert(id.to_string(), name.to_string());
        let dir = Path::new(manifest).parent().unwrap_or(Path::new(""));
        let dir = dir
            .strip_prefix(&canonical)
            .or_else(|_| dir.strip_prefix(root))
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| dir.to_path_buf());
        result.push(Member { name: name.into(), dir });
    }
    let mut reverse: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for node in doc.get("resolve").and_then(|r| r.get("nodes")).and_then(Value::as_arr).unwrap_or(&[]) {
        let Some(user) = node.get("id").and_then(Value::as_str).and_then(|id| names.get(id)) else { continue };
        for dep in node.get("deps").and_then(Value::as_arr).unwrap_or(&[]) {
            if let Some(used) = dep.get("pkg").and_then(Value::as_str).and_then(|id| names.get(id)) {
                if used != user {
                    reverse.entry(used.clone()).or_default().insert(user.clone());
                }
            }
        }
    }
    let target = doc.get("target_directory").and_then(Value::as_str).map(PathBuf::from);
    Ok((result, reverse, target))
}

/// The first lines that say why a step failed: cargo's own error lines
/// first, the step's text otherwise.
pub fn first_error(stages: &[Stage]) -> Option<String> {
    // The step that carries cargo's error, not the step that only says a batch failed.
    let has_error = |s: &&Stage| s.detail.lines().any(|l| l.trim_start().starts_with("error"));
    let failed = stages
        .iter()
        .filter(|s| s.state == "failed")
        .find(has_error)
        .or_else(|| stages.iter().find(|s| s.state == "failed"))?;
    let lines: Vec<&str> = failed.detail.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.iter().position(|l| l.trim_start().starts_with("error")).unwrap_or(0);
    let mut text: Vec<&str> = Vec::new();
    for line in &lines[start..] {
        if line.starts_with("raw cargo JSON:") || line.starts_with("full cargo output:") || text.len() >= 8 {
            break;
        }
        text.push(line);
    }
    Some(format!("{}: {}", failed.name, text.join("\n")))
}

fn stage_json(s: &Stage) -> Value {
    json::obj(vec![
        ("name", json::s(&s.name)),
        ("state", json::s(&s.state)),
        ("seconds", Value::F64(s.seconds)),
        ("command", json::s(&s.command)),
        ("detail", json::s(&s.detail)),
    ])
}

pub fn run(root: &Path, options: &Options, control: Control, notify: report::Notify) -> Result<i32> {
    let config = Config::load_existing(root, &control)?;
    let (base, mut files) = changed_files(root, &options.base_ref, &control)?;
    let head = git(root, &["rev-parse", "HEAD"], &control)?.trim().to_string();
    let (members, reverse, target) = workspace(root, &control)?;
    if let Some(target) = target {
        let _ = crate::cargo::SHARED_TARGET.set(target);
    }
    if !options.packages.is_empty() {
        // Named packages stand for the diff: one file inside each.
        files = options
            .packages
            .iter()
            .map(|p| {
                members.iter().find(|m| &m.name == p).map(|m| m.dir.join("Cargo.toml"))
                    .ok_or_else(|| format!("{p} is not a workspace member"))
            })
            .collect::<Result<_>>()?;
    }
    let (targets, scripts) = smoke::discover(root, &config)?;
    let selection = select(&files, &members, &reverse, &scripts, options.dependents);
    let mut parent = Run::new(root, root.to_path_buf(), "preflight", &head, control, notify)?;
    parent.app_targets = Arc::new(targets);
    parent.log(&format!(
        "{} changed files since {base} ({}); packages: {}; scripts: {}",
        files.len(),
        options.base_ref,
        selection.changed.iter().cloned().collect::<Vec<_>>().join(", "),
        selection.scripts.iter().map(|(s, _)| s.name.clone()).collect::<Vec<_>>().join(", ")
    ));
    let judge = crate::uihub::JudgeWorker::start(&config.model, true)?;
    let mut rows = Vec::new();
    let mut failed = false;
    for (index, (script, package)) in selection.scripts.iter().enumerate() {
        let child = parent.fork(&script.name, index)?;
        let child = crate::drive::run_preflight(child, config.clone(), judge.judge.clone(), script, selection.affected.clone());
        let red = child.stages.iter().any(|s| s.state == "failed");
        failed |= red;
        parent.failed |= red;
        parent.stages.extend(child.stages.clone());
        let ran = child.stages.iter().any(|s| s.state != "skipped");
        rows.push(json::obj(vec![
            ("script", json::s(&script.name)),
            ("path", json::s(script.path.display().to_string())),
            ("package", package.as_deref().map(json::s).unwrap_or(Value::Null)),
            ("kind", json::s(if package.is_none() { "workspace" } else if script.target.is_some() { "app" } else { "package" })),
            ("verdict", json::s(if red { "failed" } else if ran { "passed" } else { "skipped" })),
            ("first_error", first_error(&child.stages).map(json::s).unwrap_or(Value::Null)),
            ("steps", Value::Arr(child.stages.iter().map(stage_json).collect())),
            ("log", json::s(child.dir.join("log.txt").display().to_string())),
        ]));
        child.finish();
    }
    judge.finish();
    let verdict = if failed { "failed" } else if rows.is_empty() { "nothing" } else { "passed" };
    let untested: Vec<&String> = selection
        .changed
        .iter()
        .filter(|p| !selection.scripts.iter().any(|(_, s)| s.as_ref() == Some(*p)))
        .collect();
    let report = json::obj(vec![
        ("version", Value::Int(1)),
        ("root", json::s(root.display().to_string())),
        ("base_ref", json::s(&options.base_ref)),
        ("merge_base", json::s(&base)),
        ("head", json::s(&head)),
        ("changed_files", Value::Int(files.len() as i64)),
        ("changed_packages", Value::Arr(selection.changed.iter().map(json::s).collect())),
        ("covered_by_workspace_script", Value::Arr(untested.iter().map(|p| json::s(p.as_str())).collect())),
        ("unowned_files", Value::Arr(selection.unowned.iter().map(|p| json::s(p.display().to_string())).collect())),
        ("verdict", json::s(verdict)),
        ("scripts", Value::Arr(rows)),
        ("run_dir", json::s(parent.dir.display().to_string())),
    ]);
    let path = options.report.clone().unwrap_or_else(|| parent.dir.join("preflight.json"));
    fs::write(&path, report.to_json()).map_err(|e| format!("{}: {e}", path.display()))?;
    parent.log(&format!("preflight {verdict}; report {}", path.display()));
    parent.finish();
    Ok(if failed { 1 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn member(name: &str, dir: &str) -> Member {
        Member { name: name.into(), dir: dir.into() }
    }
    fn app_script(dir: &str, package: &str) -> Script {
        Script {
            path: format!("{dir}/ci.splash").into(),
            name: dir.into(),
            target: Some(smoke::Target {
                package: package.into(),
                binary: package.into(),
                cwd: ".".into(),
                manifest: format!("{dir}/Cargo.toml").into(),
                name: dir.into(),
            }),
            slice: None,
        }
    }
    fn fixture() -> (Vec<Member>, BTreeMap<String, BTreeSet<String>>, Vec<Script>) {
        let members = vec![
            member("makepad-platform", "platform"),
            member("makepad-script", "platform/script"),
            member("makepad-widgets", "widgets"),
            member("makepad-app-calculator", "apps/calculator"),
            member("makepad-app-clock", "apps/clock"),
            member("makepad-strict-json", "libs/strict_json"),
        ];
        let mut reverse: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (used, user) in [
            ("makepad-script", "makepad-platform"),
            ("makepad-platform", "makepad-widgets"),
            ("makepad-widgets", "makepad-app-calculator"),
            ("makepad-widgets", "makepad-app-clock"),
        ] {
            reverse.entry(used.into()).or_default().insert(user.into());
        }
        let scripts = vec![
            Script { path: "ci.splash".into(), name: ".".into(), target: None, slice: None },
            app_script("apps/calculator", "makepad-app-calculator"),
            app_script("apps/clock", "makepad-app-clock"),
        ];
        (members, reverse, scripts)
    }
    fn names(s: &Selection) -> Vec<String> {
        s.scripts.iter().map(|(s, _)| s.name.clone()).collect()
    }
    #[test]
    fn an_app_change_runs_only_that_apps_script() {
        let (members, reverse, scripts) = fixture();
        let s = select(&["apps/calculator/src/main.rs".into(), "README.md".into()], &members, &reverse, &scripts, false);
        assert_eq!(names(&s), vec!["apps/calculator"]);
        assert_eq!(s.changed, BTreeSet::from(["makepad-app-calculator".to_string()]));
        assert_eq!(s.unowned, vec![PathBuf::from("README.md")]);
    }
    #[test]
    fn a_nested_crate_is_owned_by_the_deepest_member() {
        let (members, _, _) = fixture();
        assert_eq!(owner(&members, Path::new("platform/script/src/vm.rs")).unwrap().name, "makepad-script");
        assert_eq!(owner(&members, Path::new("platform/src/cx.rs")).unwrap().name, "makepad-platform");
        assert!(owner(&members, Path::new("platformer/x.rs")).is_none());
    }
    #[test]
    fn a_library_change_runs_the_workspace_script_and_dependents_on_request() {
        let (members, reverse, scripts) = fixture();
        let files = vec![PathBuf::from("platform/script/src/vm.rs")];
        let s = select(&files, &members, &reverse, &scripts, false);
        assert_eq!(names(&s), vec!["."]);
        assert!(s.affected.contains("makepad-widgets") && s.affected.contains("makepad-app-clock"));
        assert!(!s.affected.contains("makepad-strict-json"));
        let s = select(&files, &members, &reverse, &scripts, true);
        assert_eq!(names(&s), vec![".", "apps/calculator", "apps/clock"]);
        // A library nobody depends on still gets the workspace script.
        let s = select(&["libs/strict_json/src/lib.rs".into()], &members, &reverse, &scripts, true);
        assert_eq!(names(&s), vec!["."]);
    }
    #[test]
    fn the_root_manifest_reaches_every_member_and_docs_reach_nothing() {
        let (members, reverse, scripts) = fixture();
        let s = select(&["Cargo.toml".into()], &members, &reverse, &scripts, false);
        assert_eq!(names(&s), vec!["."]);
        assert_eq!(s.affected.len(), members.len());
        let s = select(&["AGENTS.md".into()], &members, &reverse, &scripts, true);
        assert!(s.scripts.is_empty());
    }
    #[test]
    fn metadata_gives_members_relative_dirs_and_reverse_dependencies() {
        let text = r#"{"target_directory":"/shared/target","packages":[{"id":"a","name":"makepad-platform","manifest_path":"/r/platform/Cargo.toml"},{"id":"b","name":"makepad-app-clock","manifest_path":"/r/apps/clock/Cargo.toml"},{"id":"c","name":"serde","manifest_path":"/home/.cargo/serde/Cargo.toml"}],"workspace_members":["a","b"],"resolve":{"nodes":[{"id":"b","deps":[{"pkg":"a"},{"pkg":"c"}]},{"id":"a","deps":[]}]}}"#;
        let (members, reverse, target) = parse_metadata(Path::new("/r"), text).unwrap();
        assert_eq!(target, Some(PathBuf::from("/shared/target")));
        assert_eq!(members, vec![member("makepad-platform", "platform"), member("makepad-app-clock", "apps/clock")]);
        assert_eq!(reverse.get("makepad-platform"), Some(&BTreeSet::from(["makepad-app-clock".to_string()])));
        assert!(reverse.get("serde").is_none());
    }
    #[test]
    fn first_error_skips_to_cargos_error_and_drops_file_pointers() {
        let stage = Stage {
            name: "apps/clock / check wasm32-unknown-unknown".into(), state: "failed".into(), seconds: 1.0,
            command: String::new(), progress: String::new(), started: None,
            detail: "makepad-app-clock: upstream warnings: x: 1\nerror[E0425]: cannot find value `y`\n --> apps/clock/src/main.rs:3:5\nraw cargo JSON: /tmp/x.jsonl".into(),
        };
        let batch = Stage { name: "apps/clock / check other platforms".into(), detail: "one or more cargo batches failed".into(), ..stage.clone() };
        assert_eq!(first_error(&[batch, stage]).unwrap(),
            "apps/clock / check wasm32-unknown-unknown: error[E0425]: cannot find value `y`\n --> apps/clock/src/main.rs:3:5");
    }
}
