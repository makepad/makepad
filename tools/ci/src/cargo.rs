//! Cargo mechanics shared by the script natives and remote machines.
use crate::{
    pipeline::warning_counts,
    process::{Output, Result},
    report::{strings, Run},
    watch,
};
use makepad_strict_json::{self as json, Value};
use std::{collections::BTreeMap, path::{Path, PathBuf}};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BuildTy {
    Binary,
    BinaryBuildStd,
    Lib,
    LinuxDirect,
}
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Platform {
    Web,
    Mobile,
    Desktop,
    Embedded,
}
impl Platform {
    fn name(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::Mobile => "mobile",
            Self::Desktop => "desktop",
            Self::Embedded => "embedded",
        }
    }
}
// Mirrored from cargo_makepad; the drift test below compares every row.
pub const TOOLCHAINS: [(&str, BuildTy, Platform); 16] = [
    ("aarch64-apple-darwin", BuildTy::Binary, Platform::Desktop),
    ("x86_64-pc-windows-msvc", BuildTy::Binary, Platform::Desktop),
    ("x86_64-unknown-linux-gnu", BuildTy::Binary, Platform::Desktop),
    ("x86_64-unknown-linux-gnu", BuildTy::LinuxDirect, Platform::Embedded),
    ("wasm32-unknown-unknown", BuildTy::Lib, Platform::Web),
    ("aarch64-linux-android", BuildTy::Lib, Platform::Mobile),
    ("aarch64-apple-ios", BuildTy::Binary, Platform::Mobile),
    ("x86_64-linux-android", BuildTy::Lib, Platform::Mobile),
    ("aarch64-apple-tvos", BuildTy::BinaryBuildStd, Platform::Mobile),
    ("aarch64-apple-tvos-sim", BuildTy::BinaryBuildStd, Platform::Mobile),
    ("i686-linux-android", BuildTy::Lib, Platform::Mobile),
    ("aarch64-apple-ios-sim", BuildTy::Binary, Platform::Mobile),
    ("x86_64-apple-ios", BuildTy::Binary, Platform::Mobile),
    ("x86_64-apple-tvos", BuildTy::BinaryBuildStd, Platform::Mobile),
    ("x86_64-apple-darwin", BuildTy::Binary, Platform::Desktop),
    ("x86_64-pc-windows-gnu", BuildTy::Binary, Platform::Desktop),
];
pub fn matrix_targets(host: &str) -> Vec<String> {
    let mut targets = Vec::new();
    for (target, _, _) in TOOLCHAINS {
        if target != host && !targets.iter().any(|t| t == target) {
            targets.push(target.into());
        }
    }
    targets
}

#[derive(Clone, Debug)]
pub struct Host {
    pub target: String,
    pub targets: Vec<String>,
    pub nightly: Option<String>,
}
impl Host {
    pub fn detect(run: &Run) -> Result<Self> {
        let target = watch::host_target(&run.root, &run.control)?;
        let installed = watch::installed_targets(&run.root, &run.control)?;
        let targets = matrix_targets(&target).into_iter()
            .filter(|t| installed.contains(t)).collect();
        // Query only already-installed toolchains/components. Never bootstrap nightly.
        let toolchains = watch::probe("rustup", &strings(&["toolchain", "list"]),
            &run.root, &run.control)?;
        let mut nightly = None;
        for name in toolchains.lines().filter_map(|line| line.split_whitespace().next()) {
            if name != "nightly" && !name.starts_with("nightly-") {
                continue;
            }
            let components = watch::probe("rustup",
                &strings(&["component", "list", "--installed", "--toolchain", name]),
                &run.root, &run.control)?;
            if components.lines().any(|line| line.split_whitespace().next() == Some("rust-src")) {
                nightly = Some(name.into());
                break;
            }
        }
        Ok(Self { target, targets, nightly })
    }
    pub fn json(&self) -> Value {
        json::obj(vec![
            ("os", json::s(std::env::consts::OS)),
            ("arch", json::s(std::env::consts::ARCH)),
            ("target", json::s(&self.target)),
            (
                "targets",
                Value::Arr(self.targets.iter().map(json::s).collect()),
            ),
        ])
    }
}
#[derive(Clone, Default)]
pub struct Options {
    pub target: Option<String>,
    pub timeout: u64,
    pub env: Vec<(String, String)>,
    pub allow_fail: bool,
    pub deny: Vec<String>,
    pub toolchain: Option<String>,
}
#[derive(Clone, Debug)]
pub struct CargoResult {
    pub output: Output,
    pub warnings: BTreeMap<String, u64>,
    pub errors: Vec<String>,
    pub raw_json: Option<PathBuf>,
}
impl CargoResult {
    pub fn parse(output: Output) -> Self {
        let warnings = warning_counts(&output.out);
        let mut errors = Vec::new();
        for line in output.out.lines() {
            if let Ok(v) = json::parse_depth(line.as_bytes(), 128) {
                if let Some(m) = v.get("message") {
                    if m.get("level").and_then(Value::as_str) == Some("error") {
                        errors.extend(
                            m.get("rendered")
                                .and_then(Value::as_str)
                                .or_else(|| m.get("message").and_then(Value::as_str))
                                .unwrap_or(line)
                                .lines()
                                .map(str::to_string),
                        );
                    }
                }
            } else if line.to_lowercase().contains("error") {
                errors.push(line.into());
            }
        }
        if errors.is_empty() && output.code != 0 {
            let plain = output.out.lines().filter(|line| json::parse_depth(line.as_bytes(), 128).is_err())
                .collect::<Vec<_>>().join("\n");
            errors = crate::process::error_lines(&plain).lines().map(str::to_string).collect();
            if errors.is_empty() {
                errors.push(format!("cargo exited with code {}", output.code));
            }
        }
        errors.truncate(40);
        Self {
            output,
            warnings,
            errors,
            raw_json: None,
        }
    }
    pub fn json(&self) -> Value {
        json::obj(vec![
            ("raw_json", self.raw_json.as_ref().map(|p| json::s(p.display().to_string())).unwrap_or(Value::Null)),
            ("code", Value::Int(self.output.code as i64)),
            ("out", json::s(&self.output.out)),
            (
                "warnings",
                Value::Obj(
                    self.warnings
                        .iter()
                        .map(|(k, n)| (k.clone(), Value::Int(*n as i64)))
                        .collect(),
                ),
            ),
            (
                "errors",
                Value::Arr(self.errors.iter().map(json::s).collect()),
            ),
        ])
    }
    pub fn detail(&self) -> String {
        self.errors.join("\n")
    }
    pub fn own_warnings(&self, owner: Option<&str>) -> bool {
        self.warnings.iter().any(|(name, n)| *n > 0 && owner.is_none_or(|p| p == name))
    }
    pub fn warning_text(&self, owner: Option<&str>) -> String {
        let mut lines = Vec::new();
        let mut upstream = Vec::new();
        for (name, n) in &self.warnings {
            if owner.is_none_or(|p| p == name) {
                lines.push(format!("{name}: {n} warnings"));
            } else {
                upstream.push(format!("{name}: {n}"));
            }
        }
        if !upstream.is_empty() {
            lines.push(format!("upstream warnings: {}", upstream.join(", ")));
        }
        lines.join("\n")
    }

}
pub fn command_args(args: &[String], options: &Options, host: &str) -> Result<Vec<String>> {
    if args.is_empty() {
        return Err("cargo requires a subcommand".into());
    }
    let mut targets = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if arg == "--target" {
            targets.push(args.get(i + 1).ok_or("--target needs a triple")?.as_str());
        }
        if let Some(target) = arg.strip_prefix("--target=") {
            targets.push(target);
        }
    }
    if let Some(t) = &options.target {
        targets.push(t);
    }
    if targets.iter().any(|t| *t != host) && args[0] != "check" {
        return Err("other platforms may only be checked with cargo check --target".into());
    }
    let mut result = args.to_vec();
    if let Some(target) = &options.target {
        result.extend(["--target".into(), target.clone()]);
    }
    if matches!(args[0].as_str(), "check" | "build" | "test")
        && !args.iter().any(|a| a.starts_with("--message-format"))
    {
        let at = result
            .iter()
            .position(|a| a == "--")
            .unwrap_or(result.len());
        result.insert(at, "--message-format=json".into());
    }
    if let Some(toolchain) = &options.toolchain {
        result.insert(0, format!("+{toolchain}"));
    }
    Ok(result)
}
pub fn execute(
    run: &mut Run,
    args: &[String],
    options: &Options,
    host: &Host,
) -> Result<CargoResult> {
    let args = command_args(args, options, &host.target)?;
    let mut env = options.env.clone();
    if env.iter().any(|(k, _)| k == "CARGO_BUILD_TARGET") {
        return Err("use the target option instead of CARGO_BUILD_TARGET".into());
    }
    env.retain(|(k, _)| k != "CARGO_TARGET_DIR");
    env.push((
        "CARGO_TARGET_DIR".into(),
        run.root.join("target").display().to_string(),
    ));
    env.push(("CARGO_TERM_COLOR".into(), "never".into()));
    let root = run.root.clone();
    run.cargo_command(
        "cargo",
        &args,
        &root,
        &env,
        if options.timeout == 0 {
            3600
        } else {
            options.timeout
        },
    )
}
/// Every local command (including metadata and scripts which spawn Cargo)
/// inherits the same absolute checkout target directory.
pub fn target_env(root: &Path, env: &[(String, String)]) -> Vec<(String, String)> {
    let mut env = env.to_vec();
    env.retain(|(key, _)| key != "CARGO_TARGET_DIR");
    env.push(("CARGO_TARGET_DIR".into(), root.join("target").display().to_string()));
    env
}

#[derive(Default)]
pub struct DiagnosticLimit { count: usize }
impl DiagnosticLimit {
    pub fn render(&mut self, line: &str) -> Option<String> {
        let text = rendered_diagnostic(line)?;
        self.count += 1;
        (self.count <= 20).then_some(text)
    }
    pub fn summary(&self) -> Option<String> {
        (self.count > 20).then(|| format!("{} more cargo diagnostics omitted; see raw cargo JSON/full output", self.count - 20))
    }
}

pub fn package_name(package: &str) -> &str {
    if let Some((source, fragment)) = package.rsplit_once('#') {
        if fragment.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            source.rsplit('/').next().unwrap_or("unknown")
        } else {
            fragment.split('@').next().unwrap_or("unknown")
        }
    } else {
        package.split_whitespace().next().unwrap_or("unknown")
    }
}
#[derive(Debug)]
pub struct TargetCheck {
    pub label: String,
    pub ty: BuildTy,
    pub platform: Platform,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub toolchain: Option<String>,
    pub skip: Option<String>,
}
impl TargetCheck {
    /// The same check without some packages: a desktop tool is not asked to
    /// compile for the web or a phone.
    pub fn without_packages(mut self, skip: &[String]) -> Self {
        let mut args = Vec::with_capacity(self.args.len());
        let mut it = std::mem::take(&mut self.args).into_iter();
        while let Some(arg) = it.next() {
            if arg == "-p" {
                if let Some(package) = it.next() {
                    if !skip.contains(&package) {
                        args.push(arg);
                        args.push(package);
                    }
                }
            } else {
                args.push(arg);
            }
        }
        self.args = args;
        self
    }
}
pub fn target_checks(
    host: &Host,
    targets: &[String],
    packages: &[String],
    workspace: bool,
) -> Vec<TargetCheck> {
    let mut result = Vec::new();
    for (t, ty, platform) in TOOLCHAINS {
        if t == host.target || !targets.iter().any(|s| s == t) {
            continue;
        }
        let mut args = strings(&["check", "--target", t]);
        if workspace {
            args.push("--workspace".into());
        }
        for p in packages {
            args.extend(["-p".into(), p.clone()]);
        }
        let mut skip = None;
        let mut toolchain = None;
        let makepad = match ty {
            BuildTy::Binary => " ",
            BuildTy::Lib => {
                args.push("--lib".into());
                " "
            }
            BuildTy::BinaryBuildStd => {
                args.extend(strings(&["-Z", "build-std=std"]));
                toolchain = host.nightly.clone();
                if toolchain.is_none() {
                    skip = Some("needs nightly build-std".into());
                }
                "lines"
            }
            BuildTy::LinuxDirect => "linux_direct",
        };
        // build-std supplies the target's std itself; tier-3 targets need not
        // appear in rustup target list when nightly + rust-src are available.
        if ty != BuildTy::BinaryBuildStd && !host.targets.iter().any(|s| s == t) {
            skip = Some("not installed".into());
        }
        result.push(TargetCheck {
            label: if ty == BuildTy::LinuxDirect { format!("{t} (linux_direct)") } else { t.into() },
            ty, platform, args,
            env: vec![("MAKEPAD".into(), makepad.into())],
            toolchain, skip,
        });
    }
    result
}
pub fn library_selection(
    packages: &[String], workspace: bool, libraries: &BTreeMap<String, bool>,
) -> Result<(Vec<String>, Vec<String>)> {
    let selected: Vec<_> = if workspace { libraries.keys().cloned().collect() } else { packages.to_vec() };
    let mut present = Vec::new();
    let mut missing = Vec::new();
    for name in selected {
        match libraries.get(&name) {
            Some(true) => present.push(name),
            Some(false) => missing.push(name),
            None => return Err(format!("package {name} not found in cargo metadata")),
        }
    }
    Ok((present, missing))
}
impl TargetCheck {
    pub fn no_lib_detail(&self, packages: &[String]) -> String {
        format!("no lib target for {}: {}", self.platform.name(), packages.join(", "))
    }
}
/// Human log output from Cargo. Artifact/fingerprint JSON stays in the raw file.
pub fn rendered_diagnostic(line: &str) -> Option<String> {
    match json::parse_depth(line.as_bytes(), 128) {
        Ok(v) => {
            if v.get("reason").and_then(Value::as_str) != Some("compiler-message") {
                return None;
            }
            let m = v.get("message")?;
            if !matches!(m.get("level").and_then(Value::as_str), Some("warning" | "error")) {
                return None;
            }
            m.get("rendered").and_then(Value::as_str)
                .or_else(|| m.get("message").and_then(Value::as_str))
                .map(|s| s.trim_end().to_string())
        }
        Err(_) => {
            let text = line.trim_start();
            (text.starts_with("error:") || text.starts_with("warning:") || text.starts_with("error["))
                .then(|| line.into())
        }
    }
}
pub fn binary_path(root: &std::path::Path, binary: &str) -> Result<PathBuf> {
    if binary.is_empty() || binary.contains(['/', '\\']) || binary == ".." {
        return Err("binary must be a basename".into());
    }
    Ok(root
        .join("target/release")
        .join(format!("{binary}{}", std::env::consts::EXE_SUFFIX)))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_desktop_tool_is_left_out_of_a_check_without_disturbing_the_rest() {
        let check = TargetCheck {
            label: "wasm32-unknown-unknown".into(), ty: BuildTy::Lib, platform: Platform::Web,
            args: ["check", "--target", "wasm32-unknown-unknown", "--lib", "-p", "makepad-wm", "-p", "makepad-director", "-p", "makepad-notes", "--message-format=json"]
                .iter().map(|s| s.to_string()).collect(),
            env: Vec::new(), toolchain: None, skip: None,
        };
        let narrowed = check.without_packages(&["makepad-director".to_string()]);
        assert_eq!(narrowed.args.join(" "),
            "check --target wasm32-unknown-unknown --lib -p makepad-wm -p makepad-notes --message-format=json");
    }
    #[test]
    fn other_targets_are_check_only_and_host_is_omitted() {
        let host = Host {
            target: "aarch64-apple-darwin".into(),
            targets: strings(&["wasm32-unknown-unknown", "x86_64-pc-windows-msvc"]),
            nightly: None,
        };
        let plan = target_checks(
            &host,
            &strings(&["aarch64-apple-darwin", "wasm32-unknown-unknown", "x86_64-pc-windows-msvc", "wasm32-unknown-unknown", "riscv32imac-unknown-none-elf"]),
            &strings(&["app"]), false,
        );
        assert_eq!(plan.len(), 2);
        assert_eq!(plan[0].args, strings(&["check", "--target", "x86_64-pc-windows-msvc", "-p", "app"]));
        assert_eq!(plan[1].args, strings(&["check", "--target", "wasm32-unknown-unknown", "-p", "app", "--lib"]));
        for sub in ["build", "test", "run"] {
            assert!(command_args(
                &strings(&[sub, "--target=wasm"]),
                &Options::default(),
                "host"
            )
            .is_err());
        }
        assert!(command_args(
            &strings(&["build"]),
            &Options {
                target: Some("wasm".into()),
                ..Default::default()
            },
            "host"
        )
        .is_err());
    }
    #[test]
    fn result_keeps_rendered_error_and_warning_counts() {
        let r=CargoResult::parse(Output{code:1,out:"{\"reason\":\"compiler-message\",\"package_id\":\"path+file:///x#app@1\",\"message\":{\"level\":\"warning\"}}\n{\"message\":{\"level\":\"error\",\"rendered\":\"error: broken\\n at source.rs:2\"}}".into()});
        assert_eq!(r.warnings["app"], 1);
        assert_eq!(r.errors[0], "error: broken");
        assert_eq!(r.errors.len(), 2);
    }
    #[test]
    fn matrix_matches_cargo_makepad_exactly() {
        let source = include_str!("../../cargo_makepad/src/check/mod.rs");
        let table = source.split_once("const TOOLCHAINS:").unwrap().1
            .split_once("= [").unwrap().1.split_once("];" ).unwrap().0;
        // Remove line comments, then parse each tuple independent of wrapping.
        let table = table.lines().map(|line| line.split("//").next().unwrap())
            .collect::<Vec<_>>().join("\n");
        let rows: Vec<_> = table.split('(').skip(1).map(|tuple| {
            let fields: Vec<_> = tuple.split_once(')').unwrap().0.split(',')
                .map(str::trim).filter(|s| !s.is_empty()).collect();
            assert_eq!(fields.len(), 3);
            (fields[0].trim_matches('"').to_string(),
             fields[1].strip_prefix("BuildTy::").unwrap().to_string(),
             fields[2].strip_prefix("Platform::").unwrap().to_string())
        }).collect();
        let mirror: Vec<_> = TOOLCHAINS.iter().map(|(target, ty, platform)|
            (target.to_string(), format!("{ty:?}"), format!("{platform:?}"))).collect();
        assert_eq!(rows, mirror, "update CI's target matrix with cargo_makepad");
    }
    #[test]
    fn matrix_modes_missing_coverage_and_library_selection() {
        let mut host = Host { target: "aarch64-apple-darwin".into(),
            targets: strings(&["x86_64-unknown-linux-gnu", "wasm32-unknown-unknown"]), nightly: None };
        let targets = matrix_targets(&host.target);
        let plan = target_checks(&host, &targets, &strings(&["app"]), false);
        assert_eq!(plan.len(), TOOLCHAINS.len() - 1);
        assert!(plan.iter().any(|p| p.ty == BuildTy::LinuxDirect && p.env == vec![("MAKEPAD".into(), "linux_direct".into())]));
        assert!(plan.iter().filter(|p| p.ty == BuildTy::BinaryBuildStd).all(|p| p.skip.as_deref() == Some("needs nightly build-std")));
        assert_eq!(plan.iter().find(|p| p.label == "x86_64-pc-windows-msvc").unwrap().skip.as_deref(), Some("not installed"));
        host.nightly = Some("nightly".into());
        let plan = target_checks(&host, &targets, &strings(&["app"]), false);
        let p = plan.iter().find(|p| p.ty == BuildTy::BinaryBuildStd).unwrap();
        assert!(p.skip.is_none());
        let args = command_args(&p.args, &Options { toolchain: p.toolchain.clone(), ..Default::default() }, &host.target).unwrap();
        assert_eq!(args[0], "+nightly");
        assert!(args.windows(2).any(|a| a == ["-Z", "build-std=std"]));
        let libraries = BTreeMap::from([("app".into(), false), ("core".into(), true)]);
        let (present, missing) = library_selection(&[], true, &libraries).unwrap();
        assert_eq!(present, strings(&["core"]));
        assert_eq!(missing, strings(&["app"]));
        assert_eq!(plan.iter().find(|p| p.ty == BuildTy::Lib).unwrap().no_lib_detail(&missing), "no lib target for web: app");
    }
    #[test]
    fn upstream_diagnostics_do_not_colour_the_app() {
        let output = r#"{"reason":"compiler-message","package_id":"path+file:///repo/libs/vulkan/libloading#0.8.9","target":{"name":"wm"},"message":{"level":"warning","rendered":"warning: unused function\n  --> lib.rs:3\n"}}
{"reason":"compiler-message","package_id":"path+file:///repo/platform#makepad-platform@2.0.0","target":{"name":"wm"},"message":{"level":"warning","rendered":"warning: unexpected cfg\n"}}
{"reason":"compiler-artifact","package_id":"path+file:///repo/apps/wm#makepad-wm@0.1.0"}"#;
        let mut result = CargoResult::parse(Output { code: 0, out: output.into() });
        assert_eq!(result.warnings, BTreeMap::from([("libloading".into(), 1), ("makepad-platform".into(), 1)]));
        assert!(!result.own_warnings(Some("makepad-wm")));
        assert!(result.own_warnings(None));
        assert_eq!(result.warning_text(Some("makepad-wm")), "upstream warnings: libloading: 1, makepad-platform: 1");
        let rendered = output.lines().filter_map(rendered_diagnostic).collect::<Vec<_>>();
        assert_eq!(rendered, vec!["warning: unused function\n  --> lib.rs:3", "warning: unexpected cfg"]);
        result.warnings.insert("makepad-wm".into(), 2);
        assert!(result.own_warnings(Some("makepad-wm")));
        assert!(result.warning_text(Some("makepad-wm")).starts_with("makepad-wm: 2 warnings\nupstream warnings:"));
    }

}
