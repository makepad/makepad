//! Native build/test/fleet API, kept separate from the remote app protocol.
use super::*;
use crate::cargo::{self, CargoResult, Options};

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
    let mut env = Vec::new();
    let e = object_field(vm, v, id!(env));
    if !e.is_nil() {
        let obj = e.as_object().ok_or("env must be an object")?;
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
pub(super) fn launch(vm: &mut ScriptVm, v: ScriptValue) -> Result<usize> {
    let (binary, cwd) = if v.is_object() {
        let p = build(vm, v)?;
        let cwd = field_text(vm, v, id!(cwd))?.unwrap_or_else(|| ".".into());
        (p, PathBuf::from(cwd))
    } else {
        (PathBuf::from(text(vm, v)?), PathBuf::from("."))
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
    let result = r.launch(binary, cwd);
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
        crate::cargo_cache::Outcome::Failed(detail) => Err(detail.clone()),
    }
}
fn finish_batch(vm: &mut ScriptVm, i: usize, outcomes: Result<Vec<(String, crate::cargo_cache::Outcome)>>, opts: &Options) -> bool {
    let parent = rt(vm).step.replace(i);
    let mut failures = Vec::new();
    let mut orange = false;
    match outcomes {
        Err(e) => failures.push(e),
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
    let mut failed = false;
    for check in cargo::target_checks(&host, &targets, &packages, workspace) {
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
        failed |= finish_batch(vm, i, outcomes, &opts);
    }
    if warm {
        cargo::command_args(&crate::cargo_cache::release_args(&packages), &opts, &host.target)?;
        if !rt(vm).validate {
            let r = rt(vm);
            let i = r.run.plan(&format!("{} / warm host builds", r.group));
            r.run.begin(i, "cargo build --release --bins (all app packages)");
            let outcomes = crate::cargo_cache::warm_builds(&mut r.run, &host, &packages, &opts);
            failed |= finish_batch(vm, i, outcomes, &opts);
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
                let mut argv = vec!["test".into()];
                if object_field(vm, v, id!(workspace)).as_bool() == Some(true) {
                    argv.push("--workspace".into());
                } else {
                    let package = field_text(vm, v, id!(package))?
                        .ok_or("test requires package or workspace")?;
                    argv.extend(["-p".into(), package.clone()]);
                    if let Some(m) = package_manifest(vm, &package) {
                        argv.extend(["--manifest-path".into(), m]);
                    }
                }
                argv.extend(fields(vm, v, id!(args))?);
                let mut opts = options(vm, v)?;
                opts.allow_fail = true;
                let out = cargo_call(vm, argv, opts, None)?;
                let totals = cargo::test_json(&out.output.out);
                rt(vm).run.evidence.push(totals.clone());
                if out.output.code != 0 {
                    return Err(format!(
                        "tests failed: {}\n{}",
                        totals.to_json(),
                        out.detail()
                    ));
                }
                Ok(totals)
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
