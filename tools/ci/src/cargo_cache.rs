//! Run-owned Cargo verdicts. The mutex is used only by script workers, never UI.
use crate::{cargo::{self, CargoResult, Host, Options, TargetCheck}, process::{Output, Result}, report::{strings, Run}};
use makepad_strict_json::{self as json, Value};
use std::{collections::{BTreeMap, BTreeSet}, path::{Path, PathBuf}, sync::Arc};

#[derive(Clone)]
pub enum Outcome {
    Cargo(Arc<CargoResult>),
    Warning(String),
    /// The check does not apply to this package (a desktop-only app has no
    /// library to check for web or mobile). Said on the step, never orange.
    NotApplicable(String),
    Failed(String),
}
#[derive(Clone, Default)]
struct Package {
    libraries: BTreeSet<String>,
    bins: BTreeSet<String>,
}
#[derive(Default)]
pub struct Cache {
    pub host: Option<Host>,
    checks: BTreeMap<(String, String), Outcome>,
    builds: BTreeMap<(String, String), Outcome>,
    catalogs: BTreeMap<PathBuf, BTreeMap<String, Package>>,
}

fn catalog(run: &mut Run, cache: &mut Cache, manifest: &Path) -> Result<BTreeMap<String, Package>> {
    if let Some(packages) = cache.catalogs.get(manifest) { return Ok(packages.clone()); }
    let args = vec!["metadata".into(), "--no-deps".into(), "--format-version=1".into(),
        "--manifest-path".into(), run.root.join(manifest).display().to_string()];
    let root = run.root.clone();
    let result = run.cargo_command("cargo", &args, &root, &[], 60)?;
    if result.output.code != 0 { return Err(result.detail()); }
    let doc = result.output.out.lines().find_map(|line| json::parse_depth(line.as_bytes(), 128).ok()
        .filter(|v| v.get("packages").is_some())).ok_or("cargo metadata missing packages")?;
    let members = doc.get("workspace_members").and_then(Value::as_arr).ok_or("missing workspace members")?;
    let mut packages = BTreeMap::new();
    for p in doc.get("packages").and_then(Value::as_arr).ok_or("missing packages")? {
        if !members.iter().any(|id| id.as_str() == p.get("id").and_then(Value::as_str)) { continue; }
        let name = p.get("name").and_then(Value::as_str).ok_or("missing package name")?;
        let mut package = Package::default();
        for target in p.get("targets").and_then(Value::as_arr).ok_or("missing targets")? {
            let name = target.get("name").and_then(Value::as_str).ok_or("missing target name")?;
            let kinds = target.get("kind").and_then(Value::as_arr).ok_or("missing target kind")?;
            // A gated binary need not produce an artifact with default features.
            // Keeping it in the expected set is conservative: on batch failure,
            // it gets an individual Cargo verdict instead of a false pass.
            if kinds.iter().any(|k| k.as_str() == Some("bin")) { package.bins.insert(name.into()); }
            if kinds.iter().any(|k| matches!(k.as_str(), Some("lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"))) {
                package.libraries.insert(name.into());
            }
        }
        packages.insert(name.into(), package);
    }
    cache.catalogs.insert(manifest.into(), packages.clone());
    Ok(packages)
}

/// Preserve every row's flags while replacing just its package selection.
pub fn select_packages(args: &[String], packages: &[String]) -> Vec<String> {
    let mut result = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-p" || arg == "--package" { iter.next(); }
        else if arg != "--workspace" { result.push(arg.clone()); }
    }
    for p in packages { result.extend(["-p".into(), p.clone()]); }
    result
}
pub fn release_args(packages: &[String]) -> Vec<String> {
    let mut args = strings(&["build", "--release"]);
    for p in packages { args.extend(["-p".into(), p.clone()]); }
    args.push("--bins".into());
    args
}

fn artifact_key(target: &Value) -> Option<String> {
    let name = target.get("name")?.as_str()?;
    let kinds = target.get("kind")?.as_arr()?;
    let kind = if kinds.iter().any(|k| k.as_str() == Some("bin")) { "bin" }
    else if kinds.iter().any(|k| matches!(k.as_str(), Some("lib" | "rlib" | "dylib" | "cdylib" | "staticlib" | "proc-macro"))) { "lib" }
    else { return None; };
    Some(format!("{kind}:{name}"))
}
fn built_binary(result: &CargoResult, binary: &str) -> bool {
    result.output.out.lines().any(|line| {
        let Ok(v) = json::parse_depth(line.as_bytes(), 128) else { return false; };
        v.get("reason").and_then(Value::as_str) == Some("compiler-artifact")
            && v.get("target").and_then(artifact_key).as_deref() == Some(&format!("bin:{binary}"))
            && v.get("executable").and_then(Value::as_str).is_some()
    })
}

/// Only a complete artifact set or an owned error is conclusive on batch failure.
/// package_id, never target.name, determines diagnostic ownership.
pub fn attribute(result: &CargoResult, expected: &BTreeMap<String, BTreeSet<String>>) -> BTreeMap<String, Outcome> {
    let mut streams: BTreeMap<String, String> = BTreeMap::new();
    let mut artifacts: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut failed = BTreeSet::new();
    for line in result.output.out.lines() {
        let Ok(v) = json::parse_depth(line.as_bytes(), 128) else { continue; };
        let Some(id) = v.get("package_id").and_then(Value::as_str) else { continue; };
        let package = cargo::package_name(id);
        if !expected.contains_key(package) { continue; }
        let stream = streams.entry(package.into()).or_default();
        stream.push_str(line);
        stream.push('\n');
        match v.get("reason").and_then(Value::as_str) {
            Some("compiler-message") => {
                if v.get("message").and_then(|m| m.get("level")).and_then(Value::as_str) == Some("error") {
                    failed.insert(package.to_string());
                }
            }
            Some("compiler-artifact") => {
                if let Some(name) = v.get("target").and_then(artifact_key) {
                    artifacts.entry(package.into()).or_default().insert(name.into());
                }
            }
            _ => {}
        }
    }
    let mut verdicts = BTreeMap::new();
    for (package, targets) in expected {
        let error = failed.contains(package);
        let complete = !targets.is_empty() && artifacts.get(package).is_some_and(|a| targets.is_subset(a));
        if error || result.output.code == 0 || complete {
            let mut r = CargoResult::parse(Output { code: if error { 1 } else { 0 }, out: streams.remove(package).unwrap_or_default() });
            r.raw_json = result.raw_json.clone();
            verdicts.insert(package.clone(), Outcome::Cargo(Arc::new(r)));
        }
    }
    verdicts
}

/// Failed batches are retried with keep-going. Old toolchains and unresolved
/// dependency failures are isolated by per-package invocations.
fn batch(
    args: &[String], expected: &BTreeMap<String, BTreeSet<String>>,
    mut execute: impl FnMut(&[String]) -> Result<CargoResult>,
) -> BTreeMap<String, Outcome> {
    let first = match execute(args) {
        Ok(r) => r,
        Err(e) => return expected.keys().map(|p| (p.clone(), Outcome::Failed(e.clone()))).collect(),
    };
    let mut verdicts = attribute(&first, expected);
    if first.output.code != 0 && expected.len() > 1 {
        let mut retry = args.to_vec();
        retry.push("--keep-going".into());
        if let Ok(result) = execute(&retry) {
            // A toolchain rejecting --keep-going has no package verdicts.
            verdicts.extend(attribute(&result, expected));
        }
    }
    for package in expected.keys() {
        if verdicts.contains_key(package) { continue; }
        if expected.len() == 1 {
            verdicts.insert(package.clone(), Outcome::Cargo(Arc::new(first.clone())));
        } else {
            let args = select_packages(args, &[package.clone()]);
            let outcome = match execute(&args) {
                Ok(r) => Outcome::Cargo(Arc::new(r)),
                Err(e) => Outcome::Failed(e),
            };
            verdicts.insert(package.clone(), outcome);
        }
    }
    verdicts
}

fn groups(
    run: &mut Run, cache: &mut Cache, packages: &[String], manifest: Option<&Path>, workspace: bool,
) -> Result<Vec<(PathBuf, BTreeMap<String, Package>)>> {
    let manifest = manifest.unwrap_or(Path::new("Cargo.toml"));
    let primary = catalog(run, cache, manifest)?;
    if workspace { return Ok(vec![(manifest.into(), primary)]); }
    let mut groups: BTreeMap<PathBuf, BTreeMap<String, Package>> = BTreeMap::new();
    for p in packages {
        let (path, info) = if let Some(info) = primary.get(p) { (manifest.to_path_buf(), info.clone()) }
        else {
            let path = run.app_targets.iter().find(|t| &t.package == p).map(|t| t.manifest.clone())
                .ok_or_else(|| format!("package {p} not found in cargo metadata"))?;
            let info = catalog(run, cache, &path)?.get(p).cloned().ok_or_else(|| format!("package {p} not in {}", path.display()))?;
            (path, info)
        };
        groups.entry(path).or_default().insert(p.clone(), info);
    }
    Ok(groups.into_iter().collect())
}

pub fn checks(
    run: &mut Run, host: &Host, check: &TargetCheck, packages: &[String], workspace: bool,
    manifest: Option<&Path>, opts: &Options,
) -> Result<Vec<(String, Outcome)>> {
    let shared = run.cargo_cache.clone();
    let mut cache = shared.lock().map_err(|e| e.to_string())?;
    // User-supplied environment changes must not reuse a canonical warm verdict.
    let memoize = opts.env.iter().all(|(key, _)| key == "MAKEPAD" || key == "CARGO_TARGET_DIR");
    if !workspace && memoize && packages.iter().all(|p| cache.checks.contains_key(&(p.clone(), check.label.clone()))) {
        return Ok(packages.iter().map(|p| (p.clone(), cache.checks[&(p.clone(), check.label.clone())].clone())).collect());
    }
    let groups = groups(run, &mut cache, packages, manifest, workspace)?;
    let mut answer = Vec::new();
    for (manifest, members) in groups {
        let libraries = members.iter().map(|(name, p)| (name.clone(), !p.libraries.is_empty())).collect();
        let (_, missing_libs) = cargo::library_selection(&[], true, &libraries)?;
        let mut expected = BTreeMap::new();
        let mut outcomes = BTreeMap::new();
        for (package, info) in &members {
            let cached = memoize.then(|| cache.checks.get(&(package.clone(), check.label.clone()))).flatten();
            let outcome = if let Some(outcome) = cached { Some(outcome.clone()) }
            else if check.ty == cargo::BuildTy::Lib && missing_libs.contains(package) {
                Some(Outcome::NotApplicable(check.no_lib_detail(&[package.clone()])))
            } else { check.skip.clone().map(Outcome::Warning) };
            if let Some(outcome) = outcome { outcomes.insert(package.clone(), outcome); }
            else {
                let mut targets: BTreeSet<_> = info.libraries.iter().map(|name| format!("lib:{name}")).collect();
                if check.ty != cargo::BuildTy::Lib { targets.extend(info.bins.iter().map(|name| format!("bin:{name}"))); }
                expected.insert(package.clone(), targets);
            }
        }
        if !expected.is_empty() {
            let mut args = select_packages(&check.args, &expected.keys().cloned().collect::<Vec<_>>());
            if manifest != Path::new("Cargo.toml") {
                args.extend(["--manifest-path".into(), run.root.join(&manifest).display().to_string()]);
            }
            outcomes.extend(batch(&args, &expected, |args| cargo::execute(run, args, opts, host)));
        }
        for (package, outcome) in outcomes {
            if memoize { cache.checks.insert((package.clone(), check.label.clone()), outcome.clone()); }
            answer.push((package, outcome));
        }
    }
    Ok(answer)
}

pub fn build(
    run: &mut Run, host: &Host, package: &str, binary: &str, manifest: Option<&Path>, opts: &Options,
) -> Result<Outcome> {
    let shared = run.cargo_cache.clone();
    let mut cache = shared.lock().map_err(|e| e.to_string())?;
    let key = (package.to_string(), binary.to_string());
    let memoize = opts.env.iter().all(|(k, _)| k == "CARGO_TARGET_DIR") && opts.target.is_none();
    if memoize {
        if let Some(result) = cache.builds.get(&key) { return Ok(result.clone()); }
    }
    let mut args = strings(&["build", "--release", "-p", package, "--bin", binary]);
    if let Some(path) = manifest { args.extend(["--manifest-path".into(), run.root.join(path).display().to_string()]); }
    let result = match cargo::execute(run, &args, opts, host) {
        Ok(r) => Outcome::Cargo(Arc::new(r)),
        Err(e) => Outcome::Failed(e),
    };
    if memoize { cache.builds.insert(key, result.clone()); }
    Ok(result)
}

pub fn warm_builds(run: &mut Run, host: &Host, packages: &[String], opts: &Options) -> Result<Vec<(String, Outcome)>> {
    let shared = run.cargo_cache.clone();
    let mut cache = shared.lock().map_err(|e| e.to_string())?;
    let groups = groups(run, &mut cache, packages, None, false)?;
    let mut answer = Vec::new();
    for (manifest, members) in groups {
        let expected: BTreeMap<_, _> = members.iter().filter(|(_, p)| !p.bins.is_empty())
            .map(|(name, p)| (name.clone(), p.bins.iter().map(|bin| format!("bin:{bin}")).collect())).collect();
        if expected.is_empty() { continue; }
        let mut args = release_args(&expected.keys().cloned().collect::<Vec<_>>());
        if manifest != Path::new("Cargo.toml") {
            args.extend(["--manifest-path".into(), run.root.join(&manifest).display().to_string()]);
        }
        for (package, outcome) in batch(&args, &expected, |args| cargo::execute(run, args, opts, host)) {
            for bin in &members[&package].bins {
                // --bins can skip feature-gated binaries. Never cache a path
                // Cargo did not actually produce; explicit ci.build diagnoses it.
                if matches!(&outcome, Outcome::Cargo(r) if r.output.code == 0 && !built_binary(r, bin)) { continue; }
                cache.builds.insert((package.clone(), bin.clone()), outcome.clone());
            }
            answer.push((package, outcome));
        }
    }
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn host() -> Host {
        Host { target: "aarch64-apple-darwin".into(), targets: cargo::matrix_targets("aarch64-apple-darwin"), nightly: Some("nightly".into()) }
    }
    fn result(code: i32, out: &str) -> CargoResult { CargoResult::parse(Output { code, out: out.into() }) }
    fn expected() -> BTreeMap<String, BTreeSet<String>> {
        BTreeMap::from([("a".into(), BTreeSet::from(["bin:a_bin".into()])), ("b".into(), BTreeSet::from(["bin:b_bin".into()]))])
    }
    const FAILED: &str = r#"{"reason":"compiler-message","package_id":"path+file:///repo#b@1","target":{"name":"a_bin"},"message":{"level":"error","rendered":"error: broken b\n  at b.rs:2"}}
{"reason":"compiler-message","package_id":"path+file:///repo#a@1","target":{"name":"b_bin"},"message":{"level":"warning","rendered":"warning: a"}}
{"reason":"compiler-message","package_id":"path+file:///repo#upstream@1","message":{"level":"warning","rendered":"warning: upstream"}}
{"reason":"build-finished","success":false}"#;
    const COMPLETE_A: &str = r#"{"reason":"compiler-artifact","package_id":"path+file:///repo#a@1","target":{"name":"a_bin","kind":["bin"]},"fresh":true}"#;
    #[test]
    fn batch_commands_preserve_every_matrix_mode_and_one_target_directory() {
        let host = host();
        let packages = strings(&["a", "b"]);
        let checks = cargo::target_checks(&host, &host.targets, &packages, false);
        assert_eq!(checks.len(), 15);
        for check in checks {
            let args = select_packages(&check.args, &packages);
            let opts = Options { toolchain: check.toolchain.clone(), env: check.env.clone(), ..Default::default() };
            let full = cargo::command_args(&args, &opts, &host.target).unwrap();
            let mut expected = strings(&["check", "--target", check.label.split(' ').next().unwrap()]);
            match check.ty {
                cargo::BuildTy::Lib => expected.push("--lib".into()),
                cargo::BuildTy::BinaryBuildStd => expected.extend(strings(&["-Z", "build-std=std"])),
                _ => {}
            }
            expected.extend(strings(&["-p", "a", "-p", "b", "--message-format=json"]));
            if check.ty == cargo::BuildTy::BinaryBuildStd { expected.insert(0, "+nightly".into()); }
            assert_eq!(full, expected);
            assert_eq!(check.env[0].1, match check.ty { cargo::BuildTy::LinuxDirect => "linux_direct", cargo::BuildTy::BinaryBuildStd => "lines", _ => " " });
            let env = cargo::target_env(Path::new("/checkout"), &[("CARGO_TARGET_DIR".into(), "/wrong".into())]);
            assert_eq!(env, vec![("CARGO_TARGET_DIR".into(), "/checkout/target".into())]);
        }
        assert_eq!(cargo::command_args(&release_args(&packages), &Options::default(), &host.target).unwrap(),
            strings(&["build", "--release", "-p", "a", "-p", "b", "--bins", "--message-format=json"]));
    }
    #[test]
    fn json_diagnostics_belong_to_package_not_binary_and_partial_artifacts_do_not_pass() {
        let raw = format!("{FAILED}\n{COMPLETE_A}");
        let mut r = result(1, &raw);
        r.raw_json = Some("batch.jsonl".into());
        let results = attribute(&r, &expected());
        let Outcome::Cargo(a) = &results["a"] else { panic!() };
        assert_eq!(a.output.code, 0);
        assert_eq!(a.warnings, BTreeMap::from([("a".into(), 1)]));
        assert!(a.errors.is_empty());
        assert_eq!(a.raw_json, r.raw_json);
        let Outcome::Cargo(b) = &results["b"] else { panic!() };
        assert_eq!(b.output.code, 1);
        assert_eq!(b.errors, strings(&["error: broken b", "  at b.rs:2"]));
        assert!(b.warnings.is_empty());
        let mut targets = expected();
        targets.get_mut("a").unwrap().insert("bin:a_second_bin".into());
        assert!(!attribute(&r, &targets).contains_key("a"));
        let wrong_kind = result(1, &raw.replace("\"kind\":[\"bin\"]", "\"kind\":[\"lib\"]"));
        assert!(!attribute(&wrong_kind, &expected()).contains_key("a"));
        assert!(!built_binary(&r, "a_bin"));
        let executable = result(0, &COMPLETE_A.replace("\"fresh\":true", "\"executable\":\"/checkout/target/release/a_bin\""));
        assert!(built_binary(&executable, "a_bin"));
        assert_eq!(cargo::package_name("path+file:///repo/a#1.0"), "a");
        assert_eq!(cargo::package_name("a 1.0 (path+file:///repo)"), "a");
    }
    #[test]
    fn failed_batch_retries_keep_going_and_falls_back_only_for_unresolved_packages() {
        let args = release_args(&strings(&["a", "b"]));
        for supported in [true, false] {
            let mut calls = Vec::new();
            let verdicts = batch(&args, &expected(), |args| {
                calls.push(args.to_vec());
                Ok(match calls.len() {
                    1 => result(1, FAILED),
                    2 if supported => result(1, &format!("{FAILED}\n{COMPLETE_A}")),
                    2 => result(1, "error: unexpected argument '--keep-going' found"),
                    3 => result(0, COMPLETE_A),
                    _ => panic!("unexpected cargo call"),
                })
            });
            assert_eq!(calls[1], [args.clone(), strings(&["--keep-going"])].concat());
            assert_eq!(calls.len(), if supported { 2 } else { 3 });
            if !supported { assert_eq!(calls[2], select_packages(&args, &strings(&["a"]))); }
            let Outcome::Cargo(a) = &verdicts["a"] else { panic!() };
            let Outcome::Cargo(b) = &verdicts["b"] else { panic!() };
            assert_eq!((a.output.code, b.output.code), (0, 1));
        }
        let mut calls = 0;
        let verdicts = batch(&args, &expected(), |_| { calls += 1; Ok(result(0, COMPLETE_A)) });
        assert_eq!(calls, 1);
        assert_eq!(verdicts.len(), 2);
    }
    #[test]
    fn forked_scripts_replay_cached_verdicts_and_missing_libs_without_cargo() {
        let root = std::env::temp_dir().join(format!("ci-cache-{}-{}", std::process::id(), crate::report::stamp()));
        let run = Run::new(&root, root.clone(), "cache", "test", Default::default(), Arc::new(|_| {})).unwrap();
        let check = cargo::target_checks(&host(), &strings(&["wasm32-unknown-unknown"]), &strings(&["a"]), false).remove(0);
        let cached = Arc::new(result(1, FAILED));
        {
            let mut cache = run.cargo_cache.lock().unwrap();
            cache.checks.insert(("a".into(), check.label.clone()), Outcome::Cargo(cached.clone()));
            cache.catalogs.insert("Cargo.toml".into(), BTreeMap::from([
                ("a".into(), Package { libraries: BTreeSet::from(["a_lib".into()]), ..Default::default() }),
                ("bin-only".into(), Package::default()),
            ]));
            cache.builds.insert(("a".into(), "a_bin".into()), Outcome::Cargo(cached.clone()));
        }
        let mut child = run.fork("app", 0).unwrap();
        // The root deliberately has no Cargo.toml: a cache miss would invoke
        // Cargo and fail, so this covers the real early-return path.
        let checks = checks(&mut child, &host(), &check, &strings(&["a", "bin-only"]), false, None, &Options::default()).unwrap();
        let Outcome::Cargo(hit) = &checks[0].1 else { panic!() };
        assert!(Arc::ptr_eq(hit, &cached));
        assert_eq!(hit.warnings, cached.warnings);
        assert_eq!(hit.errors, cached.errors);
        assert!(matches!(&checks[1].1, Outcome::NotApplicable(s) if s.contains("no lib target")));
        let Outcome::Cargo(hit) = build(&mut child, &host(), "a", "a_bin", None, &Options::default()).unwrap() else { panic!() };
        assert!(Arc::ptr_eq(&hit, &cached));
        let linux = cargo::target_checks(&host(), &strings(&["x86_64-unknown-linux-gnu"]), &strings(&["a"]), false);
        child.cargo_cache.lock().unwrap().checks.insert(("a".into(), linux[0].label.clone()), Outcome::Cargo(cached.clone()));
        let mut direct = cargo::target_checks(&host(), &strings(&["x86_64-unknown-linux-gnu"]), &strings(&["a"]), false).remove(1);
        direct.skip = Some("not installed".into());
        let result = super::checks(&mut child, &host(), &direct, &strings(&["a"]), false, None, &Options::default()).unwrap();
        assert!(matches!(&result[0].1, Outcome::Warning(s) if s == "not installed"));
        assert!(child.stages.is_empty());
        assert_eq!(std::fs::read_dir(&child.dir).unwrap().count(), 1); // only log.txt
        drop(child);
        drop(run);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn diagnostic_cap_keeps_first_twenty_and_counts_the_rest() {
        let mut limit = cargo::DiagnosticLimit::default();
        for index in 0..27 {
            let line = format!(r#"{{"reason":"compiler-message","message":{{"level":"warning","rendered":"warning: {index}"}}}}"#);
            assert_eq!(limit.render(&line), (index < 20).then(|| format!("warning: {index}")));
        }
        assert_eq!(limit.render(COMPLETE_A), None);
        assert_eq!(limit.summary().unwrap(), "7 more cargo diagnostics omitted; see raw cargo JSON/full output");
    }
}
