//! `proc-pack --proc-toolchain=<tc>`: the multi-process WM's APK carries what
//! the phone needs to build its hosted apps itself — dyn-pack's pipeline
//! ([`super::stage`], [`super::pack`], [`super::rehearse`]) with the
//! [`Kind::Proc`] tree.
//!
//! What differs from the super-app: every hosted app is a self-contained
//! cdylib (its `src/main.rs` through a generated wrapper crate,
//! `proc-apps/app_<bin>`) with the engine linked STATICALLY, so there is no
//! engine dylib to bind and no `--extern force:`; the phone's
//! `cargo build -p makepad-hosted-<bin>` must only find the shipped engine
//! rlibs Fresh and compile what changed (the app, then the wrapper's link).
//! The identity rules are the super-app's unchanged: one controlled env
//! (the linker string, RUSTFLAGS, MAKEPAD, RUSTC_BOOTSTRAP), the remapping
//! rustc wrapper on both sides, whole-second mtimes, the proc-macro
//! bootstrap on the phone with the SVHs the Mac's rlibs record.
//!
//! The apps' libraries the APK carries in `lib/<abi>/` are the ones built
//! here, from the stage — the same units the phone starts from.

use super::{
    absolute, check_inherited_env, check_toolchain, claim_stage, claim_target, install_wrapper, pack, publish,
    rehearse, run_logged, rustc_vv, stage, Dyn, Kind, Wrapper,
};
use crate::android::compile;
use crate::android::sdk::AndroidSDKUrls;
use crate::android::{AndroidTarget, HostOs};
use std::{
    fs,
    path::{Path, PathBuf},
    time::{Instant, SystemTime},
};

/// `--proc-*` options of proc-pack that belong to the on-device build.
#[derive(Default)]
pub struct ProcOptions {
    /// The phone's musl-host rustc tree; `None`: no on-device build.
    pub toolchain: Option<PathBuf>,
    /// The apps (bin names) the rehearsal builds (default: calculator).
    pub rehearse: Option<Vec<String>>,
    pub no_rehearsal: bool,
}

/// Split `--proc-toolchain=`, `--proc-rehearse=`, `--proc-no-rehearsal` out
/// of the cargo args.
pub fn split_proc_options(args: &[String]) -> (ProcOptions, Vec<String>) {
    let mut opts = ProcOptions::default();
    let mut rest = Vec::new();
    for a in args {
        if let Some(v) = a.strip_prefix("--proc-toolchain=") {
            opts.toolchain = Some(absolute(Path::new(v)));
        } else if let Some(v) = a.strip_prefix("--proc-rehearse=") {
            opts.rehearse = Some(v.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect());
        } else if a == "--proc-no-rehearsal" {
            opts.no_rehearsal = true;
        } else {
            rest.push(a.clone());
        }
    }
    (opts, rest)
}

/// The staged tree of the multi-process WM, built: [`prepare`] returns it,
/// [`finish`] packs its toolchain/source/target into the APK.
pub struct ProcTree {
    d: Dyn,
    toolchain: PathBuf,
    rehearse: Vec<String>,
    no_rehearsal: bool,
    t0: Instant,
}

impl ProcTree {
    /// Where the hosted libraries the APK ships were built.
    pub fn out_dir(&self) -> PathBuf {
        self.d.target.join(self.d.triple()).join("release")
    }
}

/// Stage the tree for `apps` (`(package, bin)`) and cross-build every
/// wrapper from it, under the identity env. Must run after the host's own
/// Android build: it takes over this process's environment.
#[allow(clippy::too_many_arguments)]
pub fn prepare(
    sdk_dir: &Path,
    host_os: HostOs,
    urls: &AndroidSDKUrls,
    android_target: AndroidTarget,
    host: &str,
    min_sdk_version: Option<usize>,
    apps: &[(String, String)],
    out_apk: &Path,
    opts: &ProcOptions,
) -> Result<ProcTree, String> {
    let toolchain = opts.toolchain.clone().ok_or("prepare: no --proc-toolchain")?;
    if android_target != AndroidTarget::aarch64 {
        return Err("--proc-toolchain supports --abi=aarch64 only: the phone toolchain and the link drivers are aarch64".into());
    }
    check_inherited_env()?;
    check_toolchain(&toolchain, android_target.toolchain())?;
    let t0 = Instant::now();
    let checkout = std::env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    let urls = compile::effective_sdk_urls(host, min_sdk_version, urls)?;
    let stage_dir = proc_base(&checkout, host).join("stage");
    claim_stage(&stage_dir, &checkout, Some(&toolchain), out_apk)?;
    let wrapper = install_wrapper(&stage_dir)?;
    let makepad = std::env::var("MAKEPAD").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "vulkan".to_string());
    let wrappers: Vec<Wrapper> = apps.iter().map(|(app, bin)| Wrapper { app: app.clone(), bin: bin.clone() }).collect();
    let rehearse = opts.rehearse.clone().unwrap_or_else(|| vec!["calculator".to_string()]);
    for bin in &rehearse {
        if !wrappers.iter().any(|w| &w.bin == bin) {
            return Err(format!("--proc-rehearse: {bin} is not one of the packed apps"));
        }
    }
    let d = Dyn {
        kind: Kind::Proc { wrappers },
        rustc_vv: rustc_vv(&checkout)?,
        checkout,
        host: host.to_string(),
        engine: String::new(),
        apps: apps.iter().map(|(app, _)| app.clone()).collect(),
        proof_apps: Vec::new(),
        src: stage_dir.join("src"),
        target: stage_dir.join("target"),
        logs: stage_dir.join("logs"),
        stage: stage_dir,
        out_apk: out_apk.to_path_buf(),
        wrapper,
        android_target,
        sdk_dir: sdk_dir.to_path_buf(),
        host_os,
        urls,
        makepad,
    };
    d.apply_process_env()?;
    fs::create_dir_all(&d.logs).map_err(|e| format!("{}: {e}", d.logs.display()))?;

    println!("== stage (hosted apps + their wrapper crates)");
    stage::stage(&d)?;

    println!("== android cross-build of the hosted libraries from the stage");
    claim_target(&d)?;
    let Kind::Proc { wrappers } = &d.kind else { unreachable!() };
    let log = d.logs.join("cross-build.log");
    let mut cmd = d.command("rustup", &d.target)?;
    cmd.args(["run", "stable", "cargo", "build", "--release", "--offline", "--frozen", "--target"])
        .arg(d.triple())
        .arg("--manifest-path")
        .arg(d.src.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&d.target)
        .current_dir(&d.src);
    for w in wrappers {
        cmd.arg("-p").arg(w.package());
    }
    let started = Instant::now();
    if !run_logged(&mut cmd, &log)? {
        tail(&log, 30);
        return Err(format!("cross-build of the hosted libraries failed ({})", log.display()));
    }
    println!("built {} hosted libraries in {:.0} s", wrappers.len(), started.elapsed().as_secs_f64());
    Ok(ProcTree { d, toolchain, rehearse, no_rehearsal: opts.no_rehearsal, t0 })
}

/// Pack the toolchain, the staged tree and its target/ into `apk` (the
/// finished proc APK), rehearse the phone's builds from the packed file,
/// and publish it over `apk`.
pub fn finish(tree: ProcTree, apk: &Path) -> Result<(), String> {
    let d = &tree.d;
    println!("== pack (toolchain, source, target/ as assets)");
    let tmp = pack::pack(d, apk, &tree.toolchain)?;
    if !tree.no_rehearsal {
        println!("== rehearsal from the packed APK (the phone's mtime semantics)");
        if !rehearse::rehearse(d, &tmp, &tree.rehearse, "proc-rehearsal")? {
            return Err(format!(
                "APK NOT PROVEN: {} would rebuild the engine or run build scripts on the phone; {} left untouched",
                tmp.display(),
                apk.display()
            ));
        }
        // The unpacked copy is several GB: it proved what it had to.
        let x = d.stage.join("rehearsal");
        if x.is_dir() {
            fs::remove_dir_all(&x).map_err(|e| format!("{}: {e}", x.display()))?;
        }
    }
    publish(&tmp, apk)?;
    println!(
        "== {}: {} ({:.0} s)",
        if tree.no_rehearsal { "packed (rehearsal skipped)" } else { "proven" },
        apk.display(),
        tree.t0.elapsed().as_secs_f64()
    );
    Ok(())
}

/// The phone's build of one hosted app, on the Mac (the rehearsal):
/// `cargo build -p makepad-hosted-<bin>` against the unpacked tree with the
/// device env. Pass = cargo ok, widgets Fresh, no build script run, nothing
/// Compiling but the wrapper (and the app itself when `edited`), the
/// library produced by this run. `edited`: the app's source was changed
/// first, as a person editing it on the phone would.
pub fn proc_ondevice(
    d: &Dyn,
    tree: &Path,
    target: &Path,
    bin: &str,
    log: &Path,
    edited: bool,
    extra_env: &[(String, String)],
) -> Result<bool, String> {
    let Kind::Proc { wrappers } = &d.kind else {
        return Err("proc_ondevice on a dyn tree".into());
    };
    let w = wrappers.iter().find(|w| w.bin == bin).ok_or_else(|| format!("{bin}: not a packed app"))?;
    let so = target.join(d.triple()).join("release").join(format!("lib{}.so", w.lib()));
    let started = SystemTime::now();
    let mut cmd = super::controlled_command("rustup", extra_env);
    cmd.args(["run", "stable", "cargo", "build", "--release", "--offline", "--frozen", "--target"])
        .arg(d.triple())
        .arg("--manifest-path")
        .arg(tree.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target)
        .args(["-p", &w.package(), "-v"])
        .current_dir(tree);
    let t = Instant::now();
    let cargo_ok = run_logged(&mut cmd, log)?;
    let secs = t.elapsed().as_secs_f64();
    let text = fs::read_to_string(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let word_after = |line: &str, key: &str| -> Option<String> {
        let mut words = line.split_whitespace();
        (words.next() == Some(key)).then(|| words.next().map(str::to_string)).flatten()
    };
    let allowed = |pkg: &str| pkg == w.package() || (edited && pkg == w.app);
    let widgets_fresh = text.lines().filter(|l| word_after(l, "Fresh").as_deref() == Some("makepad-widgets")).count();
    let compiling: Vec<String> = text.lines().filter_map(|l| word_after(l, "Compiling")).collect();
    let others: Vec<&String> = compiling.iter().filter(|p| !allowed(p)).collect();
    let dirty: Vec<String> = text
        .lines()
        .filter_map(|l| word_after(l, "Dirty"))
        .filter(|p| !allowed(p))
        .collect();
    let scripts = text.lines().filter(|l| l.contains("Running `") && l.contains("build-script-build`")).count();
    let fresh_so = fs::metadata(&so).and_then(|m| m.modified()).map(|m| m >= started).unwrap_or(false);
    let app_compiled = compiling.iter().any(|p| p == &w.app);
    let pass = cargo_ok && widgets_fresh >= 1 && others.is_empty() && dirty.is_empty() && scripts == 0 && fresh_so && (!edited || app_compiled);
    println!(
        "   cargo: {}  widgets Fresh: {widgets_fresh}  Compiling: {compiling:?}  other Dirty: {dirty:?}  build scripts run: {scripts}  {}: {}  ({secs:.0} s)",
        if cargo_ok { "ok" } else { "FAILED" },
        so.display(),
        if fresh_so { "built" } else { "NOT BUILT" }
    );
    for l in text.lines().filter(|l| l.starts_with("error")).take(8) {
        println!("   {l}");
    }
    println!("   {}", if pass { "PASS" } else { "FAIL" });
    if !pass {
        println!("   log: {}", log.display());
    }
    Ok(pass)
}

/// `<target>/makepad-android-proc-stage/<host>`: the stage (`stage/`), the
/// host APK this run packs from, and the finished APK — apart from plain
/// proc-pack's output, which another build may rewrite meanwhile.
pub fn proc_base(checkout: &Path, host: &str) -> PathBuf {
    compile::cargo_target_dir(checkout).join("makepad-android-proc-stage").join(host.replace('-', "_"))
}

fn tail(log: &Path, n: usize) {
    let text = fs::read_to_string(log).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    for l in lines.iter().skip(lines.len().saturating_sub(n)) {
        println!("{l}");
    }
}
