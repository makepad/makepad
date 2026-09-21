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
    let mut args = vec![
        "build".into(),
        "--release".into(),
        "-p".into(),
        package.clone(),
        "--bin".into(),
        binary,
    ];
    // Standalone app workspaces are built through their owning manifest.
    if let Some(manifest) =
        field_text(vm, v, id!(manifest))?.or_else(|| package_manifest(vm, &package))
    {
        args.extend(["--manifest-path".into(), manifest]);
    }
    let opts = options(vm, v)?;
    cargo_call(vm, args, opts, None)?;
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
pub(super) fn register(vm: &mut ScriptVm, ci: ScriptObject) {
    let host = rt(vm).host.json();
    let host = json_value(vm, host);
    vm.bx.heap.set_value_def(ci, id!(host).into(), host);
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
    vm.add_method(
        ci,
        id_lut!(check_targets),
        script_args!(options = NIL),
        |vm, args| {
            if !allow(vm) {
                return NIL;
            }
            let result_value = (|| {
                let v = value(vm, args, id!(options));
                let packages = fields(vm, v, id!(packages))?;
                let workspace = object_field(vm, v, id!(workspace)).as_bool() == Some(true);
                if packages.is_empty() && !workspace {
                    return Err("check_targets needs packages or workspace: true".into());
                }
                let mut targets = fields(vm, v, id!(targets))?;
                let host = rt(vm).host.clone();
                // The usual host list also reports missing matrix coverage.
                // Explicit subsets stay subsets. Arbitrary configured/rustup
                // triples are discarded by target_checks' matrix iteration.
                if targets == host.targets || object_field(vm, v, id!(targets)).is_nil() {
                    targets = cargo::matrix_targets(&host.target);
                    for t in rt(vm).config.targets.clone() {
                        if !targets.contains(&t) {
                            targets.push(t);
                        }
                    }
                }
                let opts = options(vm, v)?;
                if opts.target.is_some() {
                    return Err("check_targets takes targets, not a single target option".into());
                }
                let manifest = if packages.len() == 1 {
                    package_manifest(vm, &packages[0]).map(PathBuf::from)
                } else { None };
                let mut libraries = None;
                let mut failed = false;
                for mut check in cargo::target_checks(&host, &targets, &packages, workspace) {
                    let mut check_opts = opts.clone();
                    check_opts.toolchain = check.toolchain.clone();
                    check_opts.env.retain(|(name, _)| name != "MAKEPAD");
                    check_opts.env.extend(check.env.clone());
                    if check.skip.is_none() && check.ty == cargo::BuildTy::Lib && !rt(vm).validate {
                        if libraries.is_none() {
                            libraries = Some(cargo::library_packages(&mut rt(vm).run, manifest.as_deref())?);
                        }
                        let (present, missing) = cargo::library_selection(&packages, workspace, libraries.as_ref().unwrap())?;
                        if !missing.is_empty() {
                            let detail = check.no_lib_detail(&missing);
                            let r = rt(vm);
                            let i = r.run.plan(&format!("{} / check {} / libraries", r.group, check.label));
                            r.run.end(i, "warning", &detail);
                            if !r.warning.is_empty() { r.warning.push('\n'); }
                            r.warning.push_str(&format!("{}: {detail}", check.label));
                            if present.is_empty() {
                                continue;
                            }
                            // Check the remaining libraries even in a mixed
                            // workspace with binary-only tools.
                            let mut args = Vec::new();
                            let mut iter = check.args.into_iter();
                            while let Some(arg) = iter.next() {
                                if arg == "-p" { iter.next(); }
                                else if arg != "--workspace" { args.push(arg); }
                            }
                            for name in present { args.extend(["-p".into(), name]); }
                            check.args = args;
                        }
                    }
                    if let Some(m) = &manifest {
                        check.args.extend(["--manifest-path".into(), m.display().to_string()]);
                    }
                    let args = cargo::command_args(&check.args, &check_opts, &host.target)?;
                    let r = rt(vm);
                    let i = r.run.plan(&format!("{} / check {}", r.group, check.label));
                    r.run.begin(i, &crate::report::display_command("cargo", &args));
                    if let Some(detail) = &check.skip {
                        r.run.end(i, "warning", detail);
                        if !r.warning.is_empty() { r.warning.push('\n'); }
                        r.warning.push_str(&format!("{}: {detail}", check.label));
                        continue;
                    }
                    let parent = rt(vm).step.replace(i);
                    match cargo_call(vm, check.args, check_opts, None) {
                        Ok(out) => {
                            let owner = owner(vm);
                            rt(vm).run.end(i,
                                if out.own_warnings(owner.as_deref()) { "warning" } else { "passed" }, "");
                        }
                        Err(e) => {
                            failed = true;
                            rt(vm).run.end(i, "failed", &e);
                        }
                    }
                    rt(vm).step = parent;
                }
                if failed {
                    Err("one or more target checks failed; see target steps".into())
                } else {
                    Ok(Value::Null)
                }
            })();
            result(vm, result_value)
        },
    );
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
