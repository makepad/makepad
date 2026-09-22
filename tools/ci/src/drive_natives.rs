//! Native build/test/fleet API, kept separate from the remote app protocol.
use super::*;
use crate::cargo::{self, CargoResult, Options};
use makepad_strict_json::{self as json, Value};

fn field_text(vm: &mut ScriptVm, v: ScriptValue, key: LiveId) -> Result<Option<String>> {
    let v = object_field(vm, v, key);
    if v.is_nil() {
        Ok(None)
    } else {
        text(vm, v)
            .map(Some)
            .map_err(|e| format!("field {key}: {e} ({v:?})"))
    }
}
fn array(vm: &mut ScriptVm, v: ScriptValue) -> Result<Vec<String>> {
    if v.is_nil() {
        return Ok(Vec::new());
    }
    let a = v.as_array().ok_or("expected array of strings")?;
    (0..vm.bx.heap.array_len(a))
        .map(|i| {
            let v = vm.bx.heap.array_index(a, i, NoTrap);
            text(vm, v)
        })
        .collect()
}
fn fields(vm: &mut ScriptVm, v: ScriptValue, key: LiveId) -> Result<Vec<String>> {
    let v = object_field(vm, v, key);
    array(vm, v)
}
fn options(vm: &mut ScriptVm, v: ScriptValue) -> Result<Options> {
    let timeout = object_field(vm, v, id!(timeout_secs));
    let timeout = if timeout.is_nil() {
        3600.0
    } else {
        timeout.as_number().ok_or("timeout_secs must be numeric")?
    };
    if !timeout.is_finite() || !(1.0..=86400.0).contains(&timeout) {
        return Err("invalid timeout_secs".into());
    }
    let env = env_pairs(vm, object_field(vm, v, id!(env)), "env")?;
    Ok(Options {
        target: field_text(vm, v, id!(target))?,
        timeout: timeout as u64,
        env,
        allow_fail: object_field(vm, v, id!(allow_fail)).as_bool() == Some(true),
        deny: fields(vm, v, id!(deny_warnings))?,
        toolchain: None,
    })
}
fn warning(vm: &mut ScriptVm, detail: &str) {
    let r = rt(vm);
    r.run.log(detail);
    if !r.warning.is_empty() {
        r.warning.push('\n');
    }
    r.warning.push_str(detail);
    if r.step.is_none() {
        let i = r.run.plan(&format!("{} / warning", r.group));
        r.run.end(i, "warning", detail);
    }
}
fn owner(vm: &mut ScriptVm) -> Option<String> {
    rt(vm).manifest.as_ref().map(|(name, _)| name.clone())
}
fn record(vm: &mut ScriptVm, result: &CargoResult, opts: &Options) -> Result<bool> {
    rt(vm).run.evidence.push(result.json());
    let owner = owner(vm);
    let warnings = result.warning_text(owner.as_deref());
    if let Some(i) = rt(vm).step {
        rt(vm).run.annotate(i, &warnings);
    }
    let denied: Vec<_> = result.warnings.iter()
        .filter(|(name, n)| **n > 0 && opts.deny.contains(name))
        .map(|(name, n)| format!("{name}: {n} warnings")).collect();
    if !denied.is_empty() {
        return Err(format!("denied compiler warnings\n{}", denied.join("\n")));
    }
    let orange = result.own_warnings(owner.as_deref());
    if orange {
        let own = result.warnings.iter()
            .filter(|(name, _)| owner.as_deref().is_none_or(|p| p == name.as_str()))
            .map(|(name, n)| format!("{name}: {n} warnings")).collect::<Vec<_>>().join("\n");
        warning(vm, &own);
    }
    if result.output.code != 0 && !opts.allow_fail {
        return Err(format!("{}\nexit {}", result.detail(), result.output.code));
    }
    Ok(orange)
}
fn cargo_call(
    vm: &mut ScriptVm,
    args: Vec<String>,
    opts: Options,
    machine: Option<&crate::machine::Machine>,
) -> Result<CargoResult> {
    if rt(vm).validate {
        let host = rt(vm).host.target.clone();
        cargo::command_args(&args, &opts, &host)?;
        return Ok(CargoResult::parse(crate::process::Output {
            code: 0,
            out: String::new(),
        }));
    }
    let r = rt(vm);
    let host = r.host.clone();
    let command_args = if machine.is_some() {
        args.clone()
    } else {
        cargo::command_args(&args, &opts, &host.target)?
    };
    let command = crate::report::display_command("cargo", &command_args);
    let own = r.step.is_none();
    let i = if let Some(i) = r.step {
        i
    } else {
        let i = r.run.plan(&format!("{} / {}", r.group, args.join(" ")));
        r.run.begin(i, &command);
        i
    };
    r.run.stages[i].command = command;
    r.run.publish();
    let output = if let Some(m) = machine {
        m.cargo(&mut r.run, &args, &opts)
    } else {
        cargo::execute(&mut r.run, &args, &opts, &host)
    };
    match output {
        Ok(out) => {
            let parent = rt(vm).step.replace(i);
            let result = record(vm, &out, &opts);
            rt(vm).step = parent;
            if own {
                let detail = result
                    .as_ref()
                    .err()
                    .cloned()
                    .unwrap_or_default();
                rt(vm).run.end(
                    i,
                    if result.is_err() {
                        "failed"
                    } else if result.as_ref().copied().unwrap_or(false) {
                        "warning"
                    } else {
                        "passed"
                    },
                    &detail,
                );
            }
            result?;
            Ok(out)
        }
        Err(e) => {
            if own {
                rt(vm).run.end(i, "failed", &e);
            }
            Err(e)
        }
    }
}
fn package_manifest(vm: &mut ScriptVm, package: &str) -> Option<String> {
    rt(vm)
        .manifest
        .as_ref()
        .filter(|(name, _)| name == package)
        .map(|(_, path)| path.display().to_string())
}
fn build(vm: &mut ScriptVm, v: ScriptValue) -> Result<PathBuf> {
    let package = field_text(vm, v, id!(package))?.ok_or("build requires package")?;
    let binary = field_text(vm, v, id!(binary))?.unwrap_or_else(|| package.clone());
    let path = cargo::binary_path(&rt(vm).run.root, &binary)?;
    let manifest = field_text(vm, v, id!(manifest))?
        .or_else(|| package_manifest(vm, &package)).map(PathBuf::from);
    let mut opts = options(vm, v)?;
    let host = rt(vm).host.clone();
    if let Some(target) = &opts.target {
        if target != &host.target { return Err("other platforms may only be checked".into()); }
        // The native build API always returns target/release/<bin>.
        opts.target = None;
    }
    if rt(vm).validate { return Ok(path); }
    let own = rt(vm).step.is_none();
    let i = rt(vm).step.unwrap_or_else(|| {
        let r = rt(vm);
        let i = r.run.plan(&format!("{} / build {package} / {binary}", r.group));
        r.run.begin(i, "release build (run cache)");
        i
    });
    let parent = rt(vm).step.replace(i);
    let outcome = crate::cargo_cache::build(&mut rt(vm).run, &host, &package, &binary, manifest.as_deref(), &opts);
    let result = outcome.and_then(|outcome| record_outcome(vm, &outcome, &opts));
    rt(vm).step = parent;
    if own {
        let state = match &result { Err(_) => "failed", Ok(true) => "warning", Ok(false) => "passed" };
        rt(vm).run.end(i, state, &result.as_ref().err().cloned().unwrap_or_default());
    }
    result?;
    Ok(path)
}
/// `{NAME: "value"}` as pairs; nil is no pairs.
fn env_pairs(vm: &mut ScriptVm, e: ScriptValue, what: &str) -> Result<Vec<(String, String)>> {
    let mut env = Vec::new();
    if !e.is_nil() {
        let obj = e.as_object().ok_or_else(|| format!("{what} must be an object"))?;
        let entries: Vec<_> = vm
            .bx
            .heap
            .map_ref(obj)
            .iter()
            .map(|(k, v)| (*k, v.value))
            .collect();
        for (k, v) in entries {
            let mut key = String::new();
            vm.bx.heap.cast_to_string(k, &mut key);
            env.push((key, text(vm, v)?));
        }
    }
    Ok(env)
}
pub(super) fn launch(vm: &mut ScriptVm, v: ScriptValue) -> Result<usize> {
    // `env` is the build's environment; `app_env` is what the app itself runs
    // with (a demo data switch, so a test never reads the machine's real files).
    let (binary, cwd, app_env) = if v.is_object() {
        let p = build(vm, v)?;
        let cwd = field_text(vm, v, id!(cwd))?.unwrap_or_else(|| ".".into());
        let app_env = env_pairs(vm, object_field(vm, v, id!(app_env)), "app_env")?;
        (p, PathBuf::from(cwd), app_env)
    } else {
        (PathBuf::from(text(vm, v)?), PathBuf::from("."), Vec::new())
    };
    if rt(vm).validate {
        return Ok(usize::MAX);
    }
    let r = rt(vm);
    let i = r.run.plan(&format!(
        "{} / launch {}",
        r.group,
        binary.file_name().unwrap_or_default().to_string_lossy()
    ));
    r.run.begin(i, &format!("{} --remote", binary.display()));
    let result = r.launch(binary, cwd, &app_env);
    match &result {
        Ok(_) => r.run.end(i, "passed", ""),
        Err(e) => {
            r.run.end(i, "failed", e);
            r.fatal = true;
        }
    }
    result
}
fn record_outcome(vm: &mut ScriptVm, outcome: &crate::cargo_cache::Outcome, opts: &Options) -> Result<bool> {
    match outcome {
        crate::cargo_cache::Outcome::Cargo(result) => record(vm, result, opts),
        crate::cargo_cache::Outcome::Warning(detail) => {
            if let Some(i) = rt(vm).step { rt(vm).run.annotate(i, detail); }
            warning(vm, detail);
            Ok(true)
        }
        crate::cargo_cache::Outcome::NotApplicable(detail) => {
            if let Some(i) = rt(vm).step { rt(vm).run.annotate(i, detail); }
            Ok(false)
        }
        crate::cargo_cache::Outcome::Failed(detail) => Err(detail.clone()),
    }
}
fn finish_batch(vm: &mut ScriptVm, i: usize, outcomes: Result<Vec<(String, crate::cargo_cache::Outcome)>>, opts: &Options, warm: bool) -> bool {
    let parent = rt(vm).step.replace(i);
    let mut failures = Vec::new();
    let mut orange = false;
    match outcomes {
        Err(e) => failures.push(e),
        // Warming only fills the cache. A package that fails or warns here is
        // told by the script that asks for it, on its OWN tile; the warming
        // script goes red only when cargo itself could not run.
        Ok(outcomes) if warm => {
            let failing: Vec<_> = outcomes.iter().filter(|(_, outcome)| match outcome {
                crate::cargo_cache::Outcome::Failed(_) => true,
                crate::cargo_cache::Outcome::Cargo(r) => r.output.code != 0 || !r.errors.is_empty(),
                _ => false,
            }).map(|(package, _)| package.as_str()).collect();
            if !failing.is_empty() {
                let note = format!("cached; failing here, told on their own tiles: {}", failing.join(", "));
                rt(vm).run.annotate(i, &note);
            }
            // An app's warnings are its own tile's. A LIBRARY's warnings have
            // no tile of their own, so they are this script's: the workspace
            // block goes yellow and names the crates.
            let mut libraries = std::collections::BTreeMap::new();
            for (_, outcome) in &outcomes {
                if let crate::cargo_cache::Outcome::Cargo(r) = outcome {
                    for (name, n) in &r.warnings {
                        if *n > 0 && !outcomes.iter().any(|(package, _)| package == name) {
                            let seen = libraries.entry(name.clone()).or_insert(0u64);
                            *seen = (*seen).max(*n);
                        }
                    }
                }
            }
            if !libraries.is_empty() {
                let text = libraries.iter().map(|(name, n)| format!("{name}: {n} warnings")).collect::<Vec<_>>().join("\n");
                rt(vm).run.annotate(i, &text);
                warning(vm, &text);
                orange = true;
            }
        }
        Ok(outcomes) => for (package, outcome) in outcomes {
            match record_outcome(vm, &outcome, opts) {
                Ok(warning) => orange |= warning,
                Err(e) => failures.push(format!("{package}: {e}")),
            }
        }
    }
    rt(vm).step = parent;
    rt(vm).run.end(i, if !failures.is_empty() { "failed" } else if orange { "warning" } else { "passed" }, &failures.join("\n"));
    !failures.is_empty()
}
fn check_targets(vm: &mut ScriptVm, v: ScriptValue, warm: bool) -> Result<()> {
    let packages = fields(vm, v, id!(packages))?;
    let workspace = object_field(vm, v, id!(workspace)).as_bool() == Some(true);
    if packages.is_empty() && !workspace {
        // An empty discovered app list has nothing to warm.
        if warm { return Ok(()); }
        return Err("check_targets needs packages or workspace: true".into());
    }
    if warm && workspace { return Err("warm takes packages, not workspace".into()); }
    let mut targets = fields(vm, v, id!(targets))?;
    let host = rt(vm).host.clone();
    if targets == host.targets || object_field(vm, v, id!(targets)).is_nil() {
        targets = cargo::matrix_targets(&host.target);
        for target in rt(vm).config.targets.clone() {
            if !targets.contains(&target) { targets.push(target); }
        }
    }
    let opts = options(vm, v)?;
    if opts.target.is_some() { return Err("check_targets/warm takes targets, not target".into()); }
    if warm && !opts.env.is_empty() { return Err("warm uses the matrix environment".into()); }
    let manifest = if !warm && packages.len() == 1 {
        package_manifest(vm, &packages[0]).map(PathBuf::from)
    } else { None };
    // Packages that are desktop tools (they drive cargo and child processes,
    // and most of them is compiled out elsewhere): not checked for web, mobile
    // or embedded rows, where they would only report their own absence.
    let desktop_only = fields(vm, v, id!(desktop_only))?;
    let all_packages = packages;
    let mut failed = false;
    for check in cargo::target_checks(&host, &targets, &all_packages, workspace) {
        let narrowed = check.platform != cargo::Platform::Desktop && !desktop_only.is_empty();
        let packages: Vec<String> = if narrowed {
            all_packages.iter().filter(|p| !desktop_only.contains(p)).cloned().collect()
        } else {
            all_packages.clone()
        };
        let check = if narrowed { check.without_packages(&desktop_only) } else { check };
        if packages.is_empty() && !workspace { continue; }
        let mut check_opts = opts.clone();
        check_opts.toolchain = check.toolchain.clone();
        check_opts.env.retain(|(key, _)| key != "MAKEPAD");
        check_opts.env.extend(check.env.clone());
        let args = cargo::command_args(&check.args, &check_opts, &host.target)?;
        if rt(vm).validate { continue; }
        let r = rt(vm);
        let i = r.run.plan(&format!("{} / {}check {}", r.group, if warm { "warm " } else { "" }, check.label));
        r.run.begin(i, &crate::report::display_command("cargo", &args));
        let outcomes = crate::cargo_cache::checks(&mut r.run, &host, &check, &packages, workspace, manifest.as_deref(), &check_opts);
        failed |= finish_batch(vm, i, outcomes, &opts, warm);
    }
    if warm {
        cargo::command_args(&crate::cargo_cache::release_args(&all_packages), &opts, &host.target)?;
        if !rt(vm).validate {
            let r = rt(vm);
            let i = r.run.plan(&format!("{} / warm host builds", r.group));
            r.run.begin(i, "cargo build --release --bins (all app packages)");
            let outcomes = crate::cargo_cache::warm_builds(&mut r.run, &host, &all_packages, &opts);
            failed |= finish_batch(vm, i, outcomes, &opts, true);
        }
    }
    if failed { Err("one or more cargo batches failed; see target/build steps".into()) } else { Ok(()) }
}
pub(super) fn register(vm: &mut ScriptVm, ci: ScriptObject) {
    let host = rt(vm).host.json();
    let host = json_value(vm, host);
    vm.bx.heap.set_value_def(ci, id!(host).into(), host);
    let mut packages: Vec<_> = rt(vm).run.app_targets.iter().map(|t| t.package.clone()).collect();
    packages.sort();
    packages.dedup();
    let apps = json_value(vm, Value::Arr(packages.iter().map(json::s).collect()));
    vm.bx.heap.set_value_def(ci, id!(apps).into(), apps);
    // The deep run (every crate's tests) is opt-in: `deep_tests = true` in
    // ci.toml, or `ci --deep`.
    let deep_tests = rt(vm).config.deep_tests;
    let deep = json_value(vm, Value::Bool(deep_tests));
    vm.bx.heap.set_value_def(ci, id!(deep).into(), deep);
    vm.add_method(ci, id_lut!(exclusive), script_args!(), |vm, _| {
        if allow(vm) {
            let r = rt(vm);
            if !r.validate {
                if let Some(p) = &mut r.permit {
                    if let Err(e) = p.exclusive(&r.run.control) {
                        r.fail(&e);
                    }
                }
            }
        }
        NIL
    });
    vm.add_method(ci, id_lut!(shared), script_args!(), |vm, _| {
        if allow(vm) {
            let r = rt(vm);
            if !r.validate {
                if let Some(p) = &mut r.permit {
                    p.shared();
                }
            }
        }
        NIL
    });
    vm.add_method(
        ci,
        id_lut!(cargo),
        script_args!(args = NIL, options = NIL),
        |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let result_value = (|| {
                let a = value(vm, args, id!(args));
                let a = array(vm, a)?;
                let o = value(vm, args, id!(options));
                let o = options(vm, o)?;
                cargo_call(vm, a, o, None).map(|r| r.json())
            })();
            result(vm, result_value)
        },
    );
    for (name, warm) in [(id_lut!(check_targets), false), (id_lut!(warm), true)] {
        vm.add_method(ci, name, script_args!(options = NIL), move |vm, args| {
            if !allow(vm) { return NIL; }
            let v = value(vm, args, id!(options));
            let r = check_targets(vm, v, warm).map(|_| Value::Null);
            result(vm, r)
        });
    }
    vm.add_method(
        ci,
        id_lut!(test),
        script_args!(options = NIL),
        |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let r = (|| {
                let v = value(vm, args, id!(options));
                let workspace = object_field(vm, v, id!(workspace)).as_bool() == Some(true);
                let mut packages = fields(vm, v, id!(packages))?;
                if let Some(package) = field_text(vm, v, id!(package))? {
                    packages.push(package);
                }
                let dirs = fields(vm, v, id!(dirs))?;
                let budget = object_field(vm, v, id!(budget_secs)).as_number().unwrap_or(120.0);
                let cap = object_field(vm, v, id!(cap_secs)).as_number().unwrap_or(600.0);
                let at_once_asked = object_field(vm, v, id!(at_once)).as_number();
                let extra = fields(vm, v, id!(args))?;
                let mut opts = options(vm, v)?;
                if !workspace && packages.is_empty() && dirs.is_empty() {
                    return Err("test requires workspace: true, package, packages or dirs".into());
                }
                if !(1.0..=86400.0).contains(&cap) || !(1.0..=86400.0).contains(&budget) {
                    return Err("cap_secs and budget_secs are seconds, 1 to 86400".into());
                }
                let totals_json = |t: &crate::pipeline::TestResults| {
                    json::obj(vec![
                        ("passed", Value::Int(t.passed as i64)),
                        ("failed", Value::Int(t.failed as i64)),
                        ("failed_names", Value::Arr(t.failed_names.iter().map(json::s).collect())),
                    ])
                };
                if rt(vm).validate {
                    let host = rt(vm).host.target.clone();
                    cargo::command_args(&["test".into(), "--release".into(), "--no-run".into()], &opts, &host)?;
                    return Ok(totals_json(&Default::default()));
                }
                // The directories name their packages through cargo metadata.
                if !dirs.is_empty() {
                    let root = rt(vm).run.root.clone();
                    let args = vec!["metadata".to_string(), "--no-deps".into(), "--format-version=1".into()];
                    let meta = rt(vm).run.cargo_command("cargo", &args, &root, &[], 60)?;
                    if meta.output.code != 0 {
                        return Err(meta.detail());
                    }
                    let found = crate::test_run::packages_under(&meta.output.out, &root, &dirs)?;
                    if found.is_empty() {
                        return Err(format!("no packages under {}", dirs.join(", ")));
                    }
                    packages.extend(found);
                }
                packages.sort();
                packages.dedup();
                let selection: Vec<String> = if workspace {
                    vec!["--workspace".into()]
                } else {
                    packages.iter().flat_map(|p| ["-p".to_string(), p.clone()]).collect()
                };
                // 1. Every test binary, built once, in release.
                let mut build = vec!["test".to_string(), "--release".into(), "--no-run".into()];
                build.extend(selection.iter().cloned());
                build.extend(extra.iter().cloned());
                let built = cargo_call(vm, build, opts.clone(), None)?;
                if built.output.code != 0 {
                    return Err(format!("test build failed\n{}", built.detail()));
                }
                let binaries = crate::test_run::binaries_from_json(&built.output.out);
                if binaries.is_empty() {
                    return Err("cargo built no test binaries".into());
                }
                // 2. Run them, several at a time, each one timed and capped.
                let cpus = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
                let (mut at_once, threads) = crate::test_run::plan(cpus);
                if let Some(n) = at_once_asked {
                    at_once = (n as usize).clamp(1, 64);
                }
                let step = rt(vm).step;
                let started = std::time::Instant::now();
                let outcomes = crate::test_run::run_all(
                    &mut rt(vm).run,
                    &binaries,
                    at_once,
                    threads,
                    std::time::Duration::from_secs(cap as u64),
                    step,
                );
                // 3. The doc tests, through cargo, as one command: for the
                // selected packages that have a library (cargo refuses --doc
                // for one without), or the whole workspace.
                let mut with_libs: Vec<String> = binaries
                    .iter()
                    .filter(|b| b.name.ends_with(" lib"))
                    .map(|b| b.package.clone())
                    .collect();
                with_libs.sort();
                with_libs.dedup();
                let doc_selection: Vec<String> = if workspace {
                    vec!["--workspace".into()]
                } else {
                    with_libs.iter().flat_map(|p| ["-p".to_string(), p.clone()]).collect()
                };
                let doc_out = if doc_selection.is_empty() {
                    None
                } else {
                    let mut doc = vec!["test".to_string(), "--release".into(), "--doc".into()];
                    doc.extend(doc_selection);
                    if !extra.iter().any(|a| a == "--no-fail-fast") {
                        doc.push("--no-fail-fast".into());
                    }
                    doc.extend(extra.iter().cloned());
                    opts.allow_fail = true;
                    Some(cargo_call(vm, doc, opts, None)?)
                };
                let doc_failed = doc_out.as_ref().is_some_and(|d| d.output.code != 0);
                let doc_results = doc_out
                    .as_ref()
                    .map(|d| crate::pipeline::test_results(&d.output.out))
                    .unwrap_or_default();
                let wall = started.elapsed().as_secs_f64();
                let binaries_total = crate::test_run::totals(&outcomes);
                let mut totals = binaries_total.clone();
                totals.passed += doc_results.passed;
                totals.failed += doc_results.failed;
                totals.failed_names.extend(doc_results.failed_names.iter().cloned());
                let summary = format!(
                    "{} test binaries, {at_once} at a time, {wall:.0} s: {} passed, {} failed; doc tests {} passed, {} failed. Slowest:\n{}",
                    binaries.len(), binaries_total.passed, binaries_total.failed, doc_results.passed, doc_results.failed,
                    crate::test_run::table(&outcomes, 12)
                );
                {
                    let r = rt(vm);
                    r.run.log(&summary);
                    if let Some(i) = r.step {
                        r.run.annotate(i, &summary);
                    }
                }
                let json_totals = totals_json(&totals);
                rt(vm).run.evidence.push(json_totals.clone());
                let broken: Vec<_> = outcomes.iter().filter(|o| o.failed()).collect();
                if !broken.is_empty() || doc_failed {
                    let mut lines = vec![format!("tests failed: {}", json_totals.to_json())];
                    for o in &broken {
                        let how = if o.hung { format!("hung after {:.0} s", o.seconds) } else { format!("exit {}", o.code) };
                        lines.push(format!(
                            "{} ({how}): {}\n  full output: {}",
                            o.name,
                            o.error.lines().take(6).collect::<Vec<_>>().join(" / "),
                            o.log.display()
                        ));
                    }
                    if let Some(d) = doc_out.as_ref().filter(|d| d.output.code != 0) {
                        lines.push(format!("doc tests: {}", d.detail().lines().take(8).collect::<Vec<_>>().join(" / ")));
                    }
                    return Err(lines.join("\n"));
                }
                if wall > budget {
                    warning(vm, &format!("the tests took {wall:.0} s of a {budget:.0} s budget; the slowest are named in the step"));
                }
                Ok(json_totals)
            })();
            result(vm, r)
        },
    );
    vm.add_method(
        ci,
        id_lut!(build),
        script_args!(target = NIL),
        |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let v = value(vm, args, id!(target));
            let r = build(vm, v).map(|p| json::s(p.display().to_string()));
            result(vm, r)
        },
    );
    vm.add_method(
        ci,
        id_lut!(machine),
        script_args!(name = NIL),
        |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let name = match str_arg(vm, args, id!(name)) {
                Ok(n) => n,
                Err(e) => {
                    rt(vm).fail(&e);
                    return NIL;
                }
            };
            let machine = rt(vm).config.machines.get(&name).cloned();
            if machine.is_none() {
                warning(vm, &format!("machine {name}: machine not configured"));
            }
            let obj = vm.bx.heap.new_object();
            let sync_machine = machine.clone();
            let sync_name = name.clone();
            vm.add_method(obj, id_lut!(sync), script_args!(), move |vm, _| {
                if !allow(vm) {
                    return NIL;
                }
                if rt(vm).validate {
                    return NIL;
                }
                let Some(m) = &sync_machine else {
                    warning(vm, &format!("machine {sync_name}: machine not configured"));
                    return NIL;
                };
                let r = m.sync(&mut rt(vm).run).map(|_| Value::Null);
                result(vm, r)
            });
            let run_machine = machine.clone();
            let run_name = name.clone();
            vm.add_method(
                obj,
                id_lut!(run),
                script_args!(program = NIL, args = NIL, options = NIL),
                move |vm, args| {
                    if !allow(vm) {
                        return NIL;
                    }
                    let r = (|| {
                        let p = str_arg(vm, args, id!(program))?;
                        let av = value(vm, args, id!(args));
                        let av = array(vm, av)?;
                        let ov = value(vm, args, id!(options));
                        let opts = options(vm, ov)?;
                        if !rt(vm).validate {
                            if let Some(m) = &run_machine {
                                let out =
                                    m.run(&mut rt(vm).run, &p, &av, &opts.env, opts.timeout)?;
                                return Ok(json::obj(vec![
                                    ("code", Value::Int(out.code as i64)),
                                    ("out", json::s(out.out)),
                                ]));
                            }
                            warning(vm, &format!("machine {run_name}: machine not configured"));
                        }
                        Ok(json::obj(vec![
                            ("code", Value::Int(0)),
                            ("out", json::s("machine not configured")),
                            ("skipped", Value::Bool(true)),
                        ]))
                    })();
                    result(vm, r)
                },
            );
            vm.add_method(
                obj,
                id_lut!(cargo),
                script_args!(args = NIL, options = NIL),
                move |vm, args| {
                    if !allow(vm) {
                        return NIL;
                    }
                    let r = (|| {
                        let av = value(vm, args, id!(args));
                        let av = array(vm, av)?;
                        let ov = value(vm, args, id!(options));
                        let opts = options(vm, ov)?;
                        if let Some(m) = &machine {
                            return cargo_call(vm, av, opts, Some(m)).map(|r| r.json());
                        }
                        warning(vm, &format!("machine {name}: machine not configured"));
                        let mut v = CargoResult::parse(crate::process::Output {
                            code: 0,
                            out: "machine not configured".into(),
                        })
                        .json();
                        if let Value::Obj(ref mut fields) = v {
                            fields.push(("skipped".into(), Value::Bool(true)));
                        }
                        Ok(v)
                    })();
                    result(vm, r)
                },
            );
            obj.into()
        },
    );
}
