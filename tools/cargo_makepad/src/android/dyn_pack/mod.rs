//! `cargo makepad android dyn-pack` / `dyn-rehearse`: the APK of the Android
//! super-app (`apps/wm-dyn`), whose tile apps are compiled ON THE PHONE
//! against the engine dylib the APK ships.
//!
//! The binary is the host (`makepad-wm-dyn`): the WM desk with no statically
//! linked apps. Opening a tile runs `cargo rustc --lib --crate-type dylib`
//! against the checkout the APK carries, then `dlopen`s `makepad_app_module`.
//! The engine (`apps/wm-dyn/engine`, one Rust dylib) is in the APK; the app
//! `.so` binds it.
//!
//! **The tile-build law.** Nothing below the app is ever compiled on the
//! phone: widgets/platform/engine are the loaded engine `.so`, and Rust's ABI
//! is per build, so a rebuilt widgets could not bind it (and its build script
//! cannot run: W^X). No app manifest names the engine. Two host-side
//! mechanisms make `cargo rustc -p <app>` reuse the engine's units and link
//! the engine:
//! - cargo: the packaged tree's `.cargo/config.toml` ([`stage`]) sets
//!   `[resolver] feature-unification = "workspace"` (+ `[unstable]`), so
//!   features resolve across ALL members whichever package is selected; the
//!   cross-build and every tile build pass `--no-default-features` (it applies
//!   to every member there: the apps' `standalone` graphs stay out). Same
//!   units, same hashes -> widgets Fresh.
//! - rustc: the app's own invocation gets `-- -Zunstable-options --extern
//!   force:makepad_wm_engine=<target>/<triple>/release/deps/libmakepad_wm_engine.so`
//!   (apps/wm/src/dylib_host.rs), which loads the engine crate with no
//!   `extern crate` in the app; rustc links widgets and friends from it
//!   (`IncludedFromDylib`).
//! Both are nightly-gated on stable rustc/cargo, hence `RUSTC_BOOTSTRAP=1` in
//! every cargo env — the Mac cross-build too, because the flag is in every
//! crate's SVH.
//!
//! **Identity is the environment.** cargo hashes the linker string and
//! RUSTFLAGS into every fingerprint, and every SVH hashes the compiler and
//! its flags, so every cargo this command runs — the stage's metadata /
//! vendor / lockfile, the cross-build, the proofs, the rehearsal — gets ONE
//! controlled environment ([`Dyn::cargo_env`]): inherited settings that would
//! change the outcome behind the recorded values (`CARGO_ENCODED_RUSTFLAGS`,
//! `RUSTC`, foreign wrappers, `CARGO_BUILD_RUSTFLAGS`, `CARGO_BUILD_TARGET_DIR`,
//! `CARGO_TARGET_*_RUSTFLAGS`) are refused up front, then exactly RUSTFLAGS,
//! MAKEPAD, RUSTC_BOOTSTRAP, the linker, CARGO_TARGET_DIR and our wrapper are
//! set. The stage's target/ carries `.makepad-dyn-env`: that set plus
//! `rustc -vV`; a target built under anything else is wiped first.
//!
//! **Ownership.** The stage directory (`--dyn-stage`, default under the
//! cargo target dir) is owned by this command: it carries `.makepad-dyn-stage`,
//! is created only into a missing or empty directory, and is refused when it
//! overlaps the checkout's source tree, the toolchain or the output APK. Only
//! paths inside it are ever wiped. The APK is published last, by renaming a
//! finished `<out>.tmp.apk` over the final path: a failed run never touches a
//! previous good APK.
//!
//! Phases of `dyn-pack`, each a module: [`stage`] the relocatable checkout
//! (mtimes normalized: identity), the cross-build of host + engine through
//! cargo-makepad's own Android build (RUSTFLAGS keeps `prefer-dynamic` in
//! release), [`prove`] every tile against the fresh cross-build, [`pack`] the
//! APK with the toolchain and tree as assets, then [`rehearse`] the phone's
//! first tile open FROM THE PACKED APK with the phone's mtime semantics — the
//! gate the Mac proof cannot be: exact mtimes there.
//!
//! Every rustc of those cargos runs through `<stage>/bin/makepad-dyn-rustc`, a
//! link to this binary, as `RUSTC_WRAPPER` ([`rustc_wrapper`]): it remaps the
//! build directories out of every crate's metadata so a crate rebuilt on the
//! phone (its `librustc-wrap.so` remaps the same way) has the SVH the engine
//! recorded. The wrapper mode is entered only under that name.
//!
//! Layout on device (`getFilesDir()`, provisioned on first tile open):
//! `files/src` (MAKEPAD_WM_ROOT, the staged checkout), `files/target` (the
//! Mac cross-built engine tree, host proc-macros rebuilt there), `files/tc`
//! (rustc, cargo, rust-lld: a musl host toolchain run through the shipped
//! musl loader; rust-std; the NDK API sysroot), `cargo/ home/ tmp/`.

mod metadata;
mod pack;
pub(crate) mod proc_build;
mod prove;
mod rehearse;
mod stage;

use proc_build::proc_ondevice;

use super::compile;
use super::sdk::AndroidSDKUrls;
use super::{AndroidConfig, AndroidTarget, AndroidVariant, HostOs};
use crate::utils::VersionCodeStrategy;
use makepad_toml_parser::{parse_toml, Toml};
use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::Instant,
};

/// The host side's RUSTFLAGS for the cross-build; the `--cfg android_target`
/// the platform reads is appended by the Android build itself, and the
/// device's env.txt carries the composed string (cargo hashes it).
const HOST_RUSTFLAGS: &str = "-C prefer-dynamic";
const ENV_MARKER: &str = ".makepad-dyn-env";
const STAGE_MARKER: &str = ".makepad-dyn-stage";
const STAGE_MARKER_TEXT: &str =
    "Owned by `cargo makepad android dyn-pack`: everything in this directory is regenerated or deleted by it.\n";
/// The name the rustc wrapper link has; main.rs enters wrapper mode only under it.
pub const WRAPPER_NAME: &str = "makepad-dyn-rustc";
pub const WRAPPER_VAR: &str = "MAKEPAD_DYN_RUSTC_WRAPPER";
#[cfg(unix)]
const NICED_VAR: &str = "MAKEPAD_DYN_NICED";
/// The phone toolchain's host triple. The device side (apps/wm, the link
/// driver templates) is aarch64 only, so this command is too.
pub const RUST_HOST: &str = "aarch64-unknown-linux-musl";

/// Inherited settings that would change every fingerprint or SVH behind the
/// recorded environment. `CARGO_TARGET_*_RUSTFLAGS` belongs here as well.
const REJECTED_ENV: &[&str] = &[
    "CARGO_ENCODED_RUSTFLAGS",
    "RUSTC",
    "RUSTC_WRAPPER",
    "RUSTC_WORKSPACE_WRAPPER",
    "CARGO_BUILD_RUSTFLAGS",
    "CARGO_BUILD_TARGET_DIR",
];

fn is_rejected_env(key: &str) -> bool {
    REJECTED_ENV.contains(&key) || (key.starts_with("CARGO_TARGET_") && key.ends_with("_RUSTFLAGS"))
}

/// Refuse to start under any rejected setting. Our own wrapper (the
/// `makepad-dyn-rustc` link and its marker variable) is the one exception.
pub fn check_inherited_env() -> Result<(), String> {
    let mut bad = Vec::new();
    for (key, value) in std::env::vars_os() {
        let Ok(key) = key.into_string() else { continue };
        if !is_rejected_env(&key) {
            continue;
        }
        if key == "RUSTC_WRAPPER" {
            let ours = Path::new(&value).file_stem().map(|s| s == WRAPPER_NAME).unwrap_or(false);
            if ours {
                continue;
            }
        }
        bad.push(key);
    }
    if bad.is_empty() {
        return Ok(());
    }
    bad.sort();
    Err(format!(
        "refusing to run with {} set: the phone's cargo would not see it, so the tree built here could not stay Fresh there (unset it; dyn-pack sets RUSTFLAGS, MAKEPAD, RUSTC_BOOTSTRAP, the linker, CARGO_TARGET_DIR and its own RUSTC_WRAPPER itself)",
        bad.join(", ")
    ))
}

/// A command whose environment is exactly the inherited one minus every
/// rejected setting, plus `env`.
pub fn controlled_command(program: impl AsRef<std::ffi::OsStr>, env: &[(String, String)]) -> Command {
    let mut cmd = Command::new(program);
    for (key, _) in std::env::vars_os() {
        if let Ok(key) = key.into_string() {
            if is_rejected_env(&key) || key == WRAPPER_VAR {
                cmd.env_remove(key);
            }
        }
    }
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd
}

/// `--dyn-*` options, parsed out of the cargo args.
#[derive(Default)]
pub struct DynOptions {
    /// The phone's musl-host rustc tree.
    pub toolchain: Option<PathBuf>,
    /// The tiles proven / rehearsed (default: the host's `metadata.makepad.dyn.apps`).
    pub apps: Option<Vec<String>>,
    pub out: Option<PathBuf>,
    pub stage: Option<PathBuf>,
    pub no_rehearsal: bool,
}

pub fn split_dyn_options(args: &[String]) -> Result<(DynOptions, Vec<String>), String> {
    let mut opts = DynOptions::default();
    let mut rest = Vec::new();
    for a in args {
        if let Some(v) = a.strip_prefix("--dyn-toolchain=") {
            opts.toolchain = Some(absolute(Path::new(v)));
        } else if let Some(v) = a.strip_prefix("--dyn-apps=") {
            opts.apps = Some(v.split(',').filter(|s| !s.is_empty()).map(str::to_string).collect());
        } else if let Some(v) = a.strip_prefix("--dyn-out=") {
            opts.out = Some(absolute(Path::new(v)));
        } else if let Some(v) = a.strip_prefix("--dyn-stage=") {
            opts.stage = Some(absolute(Path::new(v)));
        } else if a == "--dyn-no-rehearsal" {
            opts.no_rehearsal = true;
        } else if a.starts_with("--dyn-") {
            return Err(format!("unknown option {a}; see `cargo makepad android help`"));
        } else {
            rest.push(a.clone());
        }
    }
    Ok((opts, rest))
}

pub(super) fn absolute(p: &Path) -> PathBuf {
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap().join(p)
    }
}

/// The `-p <host>` of a dyn command.
fn host_from_args(args: &[String]) -> Result<&str, String> {
    crate::utils::get_build_crate_from_args(args)
}

/// Both commands take one ABI, aarch64, and the default variant: the device
/// consumer (apps/wm), the phone toolchain and the link-driver templates are
/// aarch64-specific, and the Quest variant changes `MAKEPAD` behind the
/// recorded environment.
fn check_target_and_variant(android_targets: &[AndroidTarget], variant: &AndroidVariant) -> Result<AndroidTarget, String> {
    let [target] = android_targets else {
        return Err("dyn-pack / dyn-rehearse take one ABI: --abi=aarch64".to_string());
    };
    if *target != AndroidTarget::aarch64 {
        return Err(format!("dyn-pack / dyn-rehearse support --abi=aarch64 only, not {}", target.to_str()));
    }
    if *variant != AndroidVariant::Default {
        return Err("dyn-pack / dyn-rehearse do not support --variant=quest".to_string());
    }
    Ok(*target)
}

/// The phone toolchain tree: rustc + cargo, the musl loader, the musl host
/// std and the phone target std.
pub(super) fn check_toolchain(tc: &Path, triple: &str) -> Result<(), String> {
    for rel in ["bin/rustc", "bin/cargo", "bin/busybox", "ld.so"] {
        if !tc.join(rel).is_file() {
            return Err(format!("--dyn-toolchain: {} has no {rel}", tc.display()));
        }
    }
    for rel in [format!("lib/rustlib/{RUST_HOST}"), format!("lib/rustlib/{triple}")] {
        if !tc.join(&rel).is_dir() {
            return Err(format!("--dyn-toolchain: {} has no {rel} (the phone's host std / target std)", tc.display()));
        }
    }
    Ok(())
}

/// What the packed tree is built for.
///
/// - `Dyn`: the super-app (`apps/wm-dyn`): one engine dylib in the APK, every
///   tile a dylib that binds it (`--extern force:`, `-C prefer-dynamic`).
/// - `Proc`: the multi-process WM (`apps/wm-android`, `proc-pack
///   --proc-toolchain`): every hosted app is a self-contained cdylib, the
///   engine linked statically, run in a process of its own by the APK's
///   launcher. No engine dylib and no SVH binding across the WM<->app
///   boundary; the phone's `cargo build -p makepad-hosted-<bin>` only has to
///   find the shipped engine rlibs Fresh. The WM host is not staged: nothing
///   on the phone rebuilds it.
pub enum Kind {
    Dyn,
    Proc { wrappers: Vec<Wrapper> },
}

/// A hosted app's generated wrapper crate in the staged tree
/// (`proc-apps/app_<bin>`): its cdylib is the app's own `src/main.rs`, the
/// library the launcher `dlopen`s (`libapp_<bin>.so`).
#[derive(Clone)]
pub struct Wrapper {
    /// The app's cargo package.
    pub app: String,
    /// The WM registry's binary name.
    pub bin: String,
}

impl Wrapper {
    /// The wrapper's package name: what the phone builds with `-p`.
    pub fn package(&self) -> String {
        format!("makepad-hosted-{}", self.bin)
    }

    /// The cdylib's crate name; the library is `lib<this>.so`.
    pub fn lib(&self) -> String {
        format!("app_{}", self.bin.replace('-', "_"))
    }

    /// Its directory in the staged tree.
    pub fn dir(&self) -> String {
        format!("{PROC_APPS}/{}", self.lib())
    }
}

/// The staged tree's directory of generated wrapper crates.
pub const PROC_APPS: &str = "proc-apps";

/// Everything the phases share.
pub struct Dyn {
    pub kind: Kind,
    /// The checkout (the working directory of the command).
    pub checkout: PathBuf,
    /// The host package (`makepad-wm-dyn`).
    pub host: String,
    /// The engine package (`metadata.makepad.dyn.engine`).
    pub engine: String,
    /// The tile packages the APK carries (`metadata.makepad.dyn.apps`).
    pub apps: Vec<String>,
    /// The tiles proven / rehearsed this run.
    pub proof_apps: Vec<String>,
    pub stage: PathBuf,
    pub src: PathBuf,
    pub target: PathBuf,
    pub logs: PathBuf,
    pub out_apk: PathBuf,
    /// `<stage>/bin/makepad-dyn-rustc`: this binary, as RUSTC_WRAPPER.
    pub wrapper: PathBuf,
    pub android_target: AndroidTarget,
    pub sdk_dir: PathBuf,
    pub host_os: HostOs,
    /// The EFFECTIVE SDK urls (the host's min SDK applied): the linker
    /// string, the sysroot API dir, env.txt and the marker all come from it.
    pub urls: AndroidSDKUrls,
    /// `MAKEPAD` of the cross-build and of every on-device cargo (env.txt).
    pub makepad: String,
    /// `rustup run stable rustc -vV`: what the phone's rustc wrapper answers
    /// with, and part of the target's env marker.
    pub rustc_vv: String,
}

impl Dyn {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        sdk_dir: &Path,
        host_os: HostOs,
        urls: &AndroidSDKUrls,
        android_target: AndroidTarget,
        host: &str,
        label: Option<&str>,
        min_sdk_version: Option<usize>,
        opts: &DynOptions,
    ) -> Result<Dyn, String> {
        let checkout = std::env::current_dir().map_err(|e| format!("cwd: {e}"))?;
        let urls = compile::effective_sdk_urls(host, min_sdk_version, urls)?;
        let m = metadata::cargo_metadata(&checkout, &[], &[])?;
        let host_id = m.id_of(host)?;
        let manifest = &m.package(host_id)?.manifest_path;
        let text = fs::read_to_string(manifest).map_err(|e| format!("{}: {e}", manifest.display()))?;
        let toml = parse_toml(&text).map_err(|e| format!("{}: {}", manifest.display(), e.msg))?;
        let engine = toml
            .get_path(&["package", "metadata", "makepad", "dyn", "engine"])
            .and_then(Toml::as_str)
            .ok_or_else(|| format!("{}: [package.metadata.makepad.dyn] engine = \"<package>\" is required", manifest.display()))?
            .to_string();
        let apps: Vec<String> = toml
            .get_path(&["package", "metadata", "makepad", "dyn", "apps"])
            .and_then(Toml::as_array)
            .map(|a| a.iter().filter_map(Toml::as_str).map(str::to_string).collect())
            .ok_or_else(|| format!("{}: [package.metadata.makepad.dyn] apps = [..] is required", manifest.display()))?;
        for pkg in [engine.as_str()].into_iter().chain(apps.iter().map(String::as_str)) {
            m.id_of(pkg)?;
        }
        let proof_apps = match &opts.apps {
            Some(list) => {
                for a in list {
                    if !apps.contains(a) {
                        return Err(format!("--dyn-apps: {a} is not one of the host's apps {apps:?}"));
                    }
                }
                list.clone()
            }
            None => apps.clone(),
        };
        let underscore = host.replace('-', "_");
        let base = compile::cargo_target_dir(&checkout).join("makepad-android-dyn").join(&underscore);
        let stage = opts.stage.clone().unwrap_or_else(|| base.join("stage"));
        let apk_name = format!("{}-dyn.apk", label.map(compile::to_snakecase).unwrap_or(underscore));
        let out_apk = opts.out.clone().unwrap_or_else(|| base.join(apk_name));
        claim_stage(&stage, &checkout, opts.toolchain.as_deref(), &out_apk)?;
        let wrapper = install_wrapper(&stage)?;
        let makepad = std::env::var("MAKEPAD").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "vulkan".to_string());
        let rustc_vv = rustc_vv(&checkout)?;
        Ok(Dyn {
            kind: Kind::Dyn,
            checkout,
            host: host.to_string(),
            engine,
            apps,
            proof_apps,
            src: stage.join("src"),
            target: stage.join("target"),
            logs: stage.join("logs"),
            stage,
            out_apk,
            wrapper,
            android_target,
            sdk_dir: sdk_dir.to_path_buf(),
            host_os,
            urls,
            makepad,
            rustc_vv,
        })
    }

    /// The stage roots: host, engine, apps (Proc: the apps; their wrappers
    /// are generated into the stage, not in the checkout).
    pub fn roots(&self) -> Vec<&str> {
        let mut v = match self.kind {
            Kind::Dyn => vec![self.host.as_str(), self.engine.as_str()],
            Kind::Proc { .. } => vec![],
        };
        v.extend(self.apps.iter().map(String::as_str));
        v
    }

    pub fn is_proc(&self) -> bool {
        matches!(self.kind, Kind::Proc { .. })
    }

    /// The RUSTFLAGS the Mac build starts from (the Android build appends
    /// the `--cfg android_target`): the super-app links its tiles against the
    /// engine dylib, the multi-process WM's apps link the engine statically.
    pub fn host_rustflags(&self) -> Option<&'static str> {
        match self.kind {
            Kind::Dyn => Some(HOST_RUSTFLAGS),
            Kind::Proc { .. } => None,
        }
    }

    pub fn triple(&self) -> &'static str {
        self.android_target.toolchain()
    }

    /// The engine crate as rustc names it (`--extern force:<this>=…`).
    pub fn engine_lib(&self) -> String {
        self.engine.replace('-', "_")
    }

    /// The engine dylib in a target dir: a path package's dylib, no
    /// `-<hash>` in the name (apps/wm/src/dylib_host.rs `engine_dylib`).
    pub fn engine_dylib(&self, target: &Path) -> PathBuf {
        target.join(self.triple()).join("release/deps").join(format!("lib{}.so", self.engine_lib()))
    }

    /// `lib<package with _>` (apps/wm/src/dylib_host.rs `lib_stem`). The
    /// stage checks every app's `[lib].name` against this rule.
    pub fn app_stem(package: &str) -> String {
        format!("lib{}", package.replace('-', "_"))
    }

    /// The env of every cargo the device runs, verbatim from the Mac build:
    /// the two strings cargo hashes (linker, RUSTFLAGS), MAKEPAD, and
    /// RUSTC_BOOTSTRAP=1 — the packaged tree's workspace feature unification
    /// and the app build's `--extern force:` are nightly-gated, and the flag
    /// is in every crate's SVH (a side without it gets E0463 against the
    /// engine's metadata).
    pub fn device_env(&self) -> Result<Vec<(String, String)>, String> {
        let linker = compile::android_linker_path(&self.sdk_dir, self.host_os, &self.urls, &self.android_target)?;
        Ok(vec![
            (self.android_target.linker_env_var().to_string(), linker.to_string_lossy().to_string()),
            ("RUSTFLAGS".to_string(), compile::android_rustflags(self.host_rustflags(), &self.android_target, false)),
            ("MAKEPAD".to_string(), self.makepad.clone()),
            ("RUSTC_BOOTSTRAP".to_string(), "1".to_string()),
        ])
    }

    /// `RUSTC_WRAPPER` = the `makepad-dyn-rustc` link, and its marker.
    pub fn wrapper_env(&self) -> Vec<(String, String)> {
        vec![
            ("RUSTC_WRAPPER".to_string(), self.wrapper.to_string_lossy().to_string()),
            (WRAPPER_VAR.to_string(), "1".to_string()),
        ]
    }

    /// The controlled cargo environment for a target dir: the device env,
    /// the target dir, our wrapper — exactly that on top of a cleaned
    /// inheritance ([`controlled_command`]).
    pub fn cargo_env(&self, target_dir: &Path) -> Result<Vec<(String, String)>, String> {
        let mut env = self.device_env()?;
        env.push(("CARGO_TARGET_DIR".to_string(), target_dir.to_string_lossy().to_string()));
        env.extend(self.wrapper_env());
        Ok(env)
    }

    /// A command under the controlled cargo environment.
    pub fn command(&self, program: &str, target_dir: &Path) -> Result<Command, String> {
        Ok(controlled_command(program, &self.cargo_env(target_dir)?))
    }

    /// The controlled environment on THIS process: what cargo-makepad's own
    /// Android build (in-process, `compile::build`) and everything it spawns
    /// see. RUSTFLAGS is the pre-composition value here: `rust_build`
    /// appends the `--cfg android_target` itself.
    pub fn apply_process_env(&self) -> Result<(), String> {
        for (key, _) in std::env::vars_os() {
            if let Ok(key) = key.into_string() {
                if is_rejected_env(&key) || key == WRAPPER_VAR {
                    std::env::remove_var(key);
                }
            }
        }
        let linker = compile::android_linker_path(&self.sdk_dir, self.host_os, &self.urls, &self.android_target)?;
        std::env::set_var(self.android_target.linker_env_var(), &linker);
        match self.host_rustflags() {
            Some(flags) => std::env::set_var("RUSTFLAGS", flags),
            None => std::env::remove_var("RUSTFLAGS"),
        }
        std::env::set_var("MAKEPAD", &self.makepad);
        std::env::set_var("RUSTC_BOOTSTRAP", "1");
        std::env::set_var("CARGO_TARGET_DIR", &self.target);
        for (k, v) in self.wrapper_env() {
            std::env::set_var(k, v);
        }
        Ok(())
    }

    /// `env.txt`: the device env, one `KEY=VALUE` per line.
    pub fn env_text(&self) -> Result<String, String> {
        Ok(self.device_env()?.iter().map(|(k, v)| format!("{k}={v}\n")).collect())
    }

    /// The target's identity marker: the effective env plus the compiler.
    pub fn marker_text(&self) -> Result<String, String> {
        Ok(format!("{}# rustc -vV\n{}\n", self.env_text()?, self.rustc_vv))
    }
}

/// Take the stage directory: it must be ours (the marker), or missing, or
/// empty; and it must not overlap the checkout's source tree, the toolchain
/// or the output APK, because everything below it is regenerated or wiped.
pub(super) fn claim_stage(stage: &Path, checkout: &Path, toolchain: Option<&Path>, out_apk: &Path) -> Result<(), String> {
    if stage == checkout || checkout.starts_with(stage) {
        return Err(format!("--dyn-stage: {} contains the checkout", stage.display()));
    }
    if stage.starts_with(checkout) {
        // Inside the checkout only where git ignores it (target/, local/):
        // never in the source tree.
        let ignored = Command::new("git")
            .args(["check-ignore", "-q", "--"])
            .arg(stage)
            .current_dir(checkout)
            .status()
            .map_err(|e| format!("git check-ignore: {e}"))?
            .success();
        if !ignored {
            return Err(format!(
                "--dyn-stage: {} is inside the checkout's source tree (git does not ignore it); use a path under target/ or outside the checkout",
                stage.display()
            ));
        }
    }
    if let Some(tc) = toolchain {
        if stage == tc || stage.starts_with(tc) || tc.starts_with(stage) {
            return Err(format!("--dyn-stage: {} overlaps the toolchain {}", stage.display(), tc.display()));
        }
    }
    let out_parent = out_apk.parent().ok_or("--dyn-out: the apk has no directory")?;
    if out_parent == stage || out_parent.starts_with(stage) {
        return Err(format!("--dyn-out: {} is inside the stage {}", out_apk.display(), stage.display()));
    }
    let marker = stage.join(STAGE_MARKER);
    if stage.exists() {
        if !stage.is_dir() {
            return Err(format!("--dyn-stage: {} is not a directory", stage.display()));
        }
        if marker.is_file() {
            return Ok(());
        }
        let mut entries = fs::read_dir(stage).map_err(|e| format!("{}: {e}", stage.display()))?;
        if entries.next().is_some() {
            return Err(format!(
                "--dyn-stage: {} exists and is not a dyn-pack stage (no {STAGE_MARKER}); refusing to touch it",
                stage.display()
            ));
        }
    } else {
        fs::create_dir_all(stage).map_err(|e| format!("{}: {e}", stage.display()))?;
    }
    fs::write(&marker, STAGE_MARKER_TEXT).map_err(|e| format!("{}: {e}", marker.display()))
}

/// `<stage>/bin/makepad-dyn-rustc`: a link to this binary (a copy where
/// links are not available), refreshed every run.
pub(super) fn install_wrapper(stage: &Path) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let bin = stage.join("bin");
    fs::create_dir_all(&bin).map_err(|e| format!("{}: {e}", bin.display()))?;
    let link = bin.join(WRAPPER_NAME);
    if fs::symlink_metadata(&link).is_ok() {
        fs::remove_file(&link).map_err(|e| format!("{}: {e}", link.display()))?;
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(&exe, &link).map_err(|e| format!("{}: {e}", link.display()))?;
    #[cfg(not(unix))]
    fs::copy(&exe, &link).map_err(|e| format!("{}: {e}", link.display()))?;
    Ok(link)
}

/// `rustup run stable rustc -vV`: the text the device's rustc wrapper answers
/// with, so cargo's fingerprints (a hash of it) match the shipped target/.
pub(super) fn rustc_vv(cwd: &Path) -> Result<String, String> {
    let out = controlled_command("rustup", &[])
        .args(["run", "stable", "rustc", "-vV"])
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("rustc -vV: {e}"))?;
    if !out.status.success() {
        return Err(format!("rustc -vV failed: {}", String::from_utf8_lossy(&out.stderr)));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim_end_matches('\n').to_string())
}

/// Run `cmd` with stdout and stderr to `log`; Ok(success).
pub fn run_logged(cmd: &mut Command, log: &Path) -> Result<bool, String> {
    if let Some(parent) = log.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let out = fs::File::create(log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| format!("{}: {e}", log.display()))?;
    cmd.stdout(Stdio::from(out)).stderr(Stdio::from(err)).stdin(Stdio::null());
    let status = cmd.status().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
    Ok(status.success())
}

/// On unix, re-run this very command under `nice -n 15` once (marked by
/// `MAKEPAD_DYN_NICED`), so every child — metadata, vendor, the cross-build,
/// the proofs — inherits the niceness: an agent's builds must not starve the
/// user's window. Returns only when already niced or when `nice` could not
/// be started (then the run goes on un-niced, with a warning).
pub(super) fn renice_self() {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        if std::env::var_os(NICED_VAR).is_some() {
            return;
        }
        let Ok(exe) = std::env::current_exe() else { return };
        let args: Vec<String> = std::env::args().skip(1).collect();
        let err = Command::new("nice").args(["-n", "15"]).arg(exe).args(args).env(NICED_VAR, "1").exec();
        eprintln!("warning: could not re-run under nice ({err}); continuing at normal priority");
    }
}

/// Wrapper mode (`makepad-dyn-rustc`, `MAKEPAD_DYN_RUSTC_WRAPPER` set):
/// `args` = `[rustc, args…]` as cargo hands them to `RUSTC_WRAPPER`. Remap
/// the build directories out of every crate's metadata so a crate rebuilt in
/// another directory has the same SVH; `-vV` and `--print` queries pass
/// through untouched. On unix rustc replaces this process, so its signals
/// and exit status are cargo's to see.
pub fn rustc_wrapper(args: &[String]) -> ! {
    let Some((rustc, rest)) = args.split_first() else {
        eprintln!("{WRAPPER_NAME}: no rustc given");
        std::process::exit(2);
    };
    let query = rest.iter().any(|a| a == "-vV" || a.starts_with("--print"));
    let mut cmd = Command::new(rustc);
    cmd.args(rest);
    if !query {
        let cwd = std::env::current_dir().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        cmd.arg(format!("--remap-path-prefix={cwd}=/wmdyn"));
        if let Ok(target) = std::env::var("CARGO_TARGET_DIR") {
            if !target.is_empty() {
                cmd.arg(format!("--remap-path-prefix={target}=/wmdyn-target"));
            }
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let err = cmd.exec();
        eprintln!("{WRAPPER_NAME}: {rustc}: {err}");
        std::process::exit(1);
    }
    #[cfg(not(unix))]
    match cmd.status() {
        Ok(status) => std::process::exit(status.code().unwrap_or(1)),
        Err(e) => {
            eprintln!("{WRAPPER_NAME}: {rustc}: {e}");
            std::process::exit(1);
        }
    }
}

/// The whole pack: stage -> cross-build -> proofs -> pack -> rehearsal ->
/// publish.
#[allow(clippy::too_many_arguments)]
pub fn dyn_pack(
    sdk_dir: &Path,
    host_os: HostOs,
    package_name: Option<String>,
    app_label: Option<String>,
    version_code: Option<VersionCodeStrategy>,
    version_name: Option<String>,
    min_sdk_version: Option<usize>,
    args: &[String],
    android_targets: &[AndroidTarget],
    variant: &AndroidVariant,
    config: &AndroidConfig,
    urls: &AndroidSDKUrls,
) -> Result<(), String> {
    renice_self();
    check_inherited_env()?;
    let (opts, cargo_args) = split_dyn_options(args)?;
    let android_target = check_target_and_variant(android_targets, variant)?;
    let toolchain = opts
        .toolchain
        .clone()
        .ok_or("dyn-pack needs --dyn-toolchain=<dir>: the phone's musl-host rustc tree (bin/rustc, bin/cargo, ld.so, lib/rustlib/<musl host>)")?;
    check_toolchain(&toolchain, android_target.toolchain())?;
    let host = host_from_args(&cargo_args)?;
    let d = Dyn::new(sdk_dir, host_os, urls, android_target, host, app_label.as_deref(), min_sdk_version, &opts)?;
    d.apply_process_env()?;
    fs::create_dir_all(&d.logs).map_err(|e| format!("{}: {e}", d.logs.display()))?;
    let t0 = Instant::now();

    println!("== stage");
    stage::stage(&d)?;

    println!("== android cross-build (host cdylib + engine dylib; --no-default-features = every member's, as on the phone)");
    let apk_in = cross_build(&d, package_name, app_label, version_code, version_name, min_sdk_version, &cargo_args, variant, config)?;

    println!("== the tile builds against the fresh cross-build (the on-device command, on the Mac)");
    for app in &d.proof_apps {
        println!("== {app}");
        let log = d.logs.join(format!("pack-{app}.log"));
        // An app's own build script (apps/files) compiles and runs HERE, once:
        // the pack ships that run, the phone finds it Fresh (the rehearsal
        // below insists on 0 runs).
        if !prove::prove(&d, &d.src, &d.target, app, &log, true, &[])? {
            return Err(format!("{app} would rebuild the engine / embed widgets — see {}", log.display()));
        }
    }

    println!("== pack");
    let tmp = pack::pack(&d, &apk_in, &toolchain)?;

    if !opts.no_rehearsal {
        println!("== rehearsal from the packed APK (the phone's mtime semantics)");
        if !rehearse::rehearse(&d, &tmp, &d.proof_apps, "pack-rehearsal")? {
            return Err(format!(
                "APK NOT PROVEN: {} would run build scripts / rebuild the engine on the phone; {} left untouched",
                tmp.display(),
                d.out_apk.display()
            ));
        }
    }
    publish(&tmp, &d.out_apk)?;
    println!(
        "== {}: {} ({:.0} s)",
        if opts.no_rehearsal { "packed (rehearsal skipped)" } else { "proven" },
        d.out_apk.display(),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

/// The rehearsal alone: `dyn-rehearse -p <host> <apk> [package …]`.
#[allow(clippy::too_many_arguments)]
pub fn dyn_rehearse(
    sdk_dir: &Path,
    host_os: HostOs,
    app_label: Option<String>,
    min_sdk_version: Option<usize>,
    args: &[String],
    android_targets: &[AndroidTarget],
    variant: &AndroidVariant,
    urls: &AndroidSDKUrls,
) -> Result<(), String> {
    renice_self();
    check_inherited_env()?;
    let (opts, rest) = split_dyn_options(args)?;
    let android_target = check_target_and_variant(android_targets, variant)?;
    let mut host = None;
    let mut positional = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let a = &rest[i];
        if a == "-p" || a == "--package" {
            host = rest.get(i + 1).cloned();
            i += 2;
            continue;
        }
        if let Some(p) = a.strip_prefix("--package=") {
            host = Some(p.to_string());
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }
    let host = host.ok_or("dyn-rehearse needs -p <host package> (the super-app crate)")?;
    let (apk, apps) = positional.split_first().ok_or("dyn-rehearse needs the apk to rehearse")?;
    let apk = absolute(Path::new(apk));
    if !apk.is_file() {
        return Err(format!("{}: no such apk", apk.display()));
    }
    let mut opts = opts;
    if !apps.is_empty() {
        opts.apps = Some(apps.to_vec());
    }
    let d = Dyn::new(sdk_dir, host_os, urls, android_target, &host, app_label.as_deref(), min_sdk_version, &opts)?;
    d.apply_process_env()?;
    if rehearse::rehearse(&d, &apk, &d.proof_apps, "rehearsal")? {
        println!("== rehearsed: {}", apk.display());
        Ok(())
    } else {
        Err(format!("{}: rehearsal failed for at least one app", apk.display()))
    }
}

/// cargo-makepad's own Android build of the host, run FROM the stage with
/// the stage's target dir and the identity env; returns the APK it made.
#[allow(clippy::too_many_arguments)]
fn cross_build(
    d: &Dyn,
    package_name: Option<String>,
    app_label: Option<String>,
    version_code: Option<VersionCodeStrategy>,
    version_name: Option<String>,
    min_sdk_version: Option<usize>,
    cargo_args: &[String],
    variant: &AndroidVariant,
    config: &AndroidConfig,
) -> Result<PathBuf, String> {
    claim_target(d)?;
    let previous = std::env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    std::env::set_current_dir(&d.src).map_err(|e| format!("{}: {e}", d.src.display()))?;
    let result = compile::build(
        &d.sdk_dir,
        d.host_os,
        package_name,
        app_label,
        version_code,
        version_name,
        min_sdk_version,
        cargo_args,
        &[d.android_target],
        variant,
        config,
        &d.urls,
    );
    std::env::set_current_dir(&previous).map_err(|e| format!("{}: {e}", previous.display()))?;
    let apk = result?.apk().to_path_buf();
    println!("Bundled {}", apk.display());
    Ok(apk)
}

/// The env and the compiler are identity: a target built under another one
/// never matches. The stage is ours (claim_stage), so its target may go.
pub fn claim_target(d: &Dyn) -> Result<(), String> {
    let marker_text = d.marker_text()?;
    let marker = d.target.join(ENV_MARKER);
    if d.target.is_dir() {
        let have = fs::read_to_string(&marker).ok();
        let populated = fs::read_dir(&d.target).map(|mut r| r.next().is_some()).unwrap_or(false);
        if populated && have.as_deref() != Some(marker_text.as_str()) {
            println!(
                "{} was built under another environment or compiler (the linker, RUSTFLAGS, MAKEPAD, RUSTC_BOOTSTRAP and rustc are in every SVH): wiping it",
                d.target.display()
            );
            fs::remove_dir_all(&d.target).map_err(|e| format!("{}: {e}", d.target.display()))?;
        }
    }
    fs::create_dir_all(&d.target).map_err(|e| format!("{}: {e}", d.target.display()))?;
    fs::write(&marker, &marker_text).map_err(|e| format!("{}: {e}", marker.display()))
}

/// The finished `<out>.tmp.apk` (and apksigner's `.idsig` beside it) become
/// the final APK by rename: the last step, so a failed run leaves the
/// previous APK as it was.
pub(super) fn publish(tmp: &Path, out: &Path) -> Result<(), String> {
    let tmp_sig = PathBuf::from(format!("{}.idsig", tmp.display()));
    let out_sig = PathBuf::from(format!("{}.idsig", out.display()));
    fs::rename(tmp, out).map_err(|e| format!("publish {} -> {}: {e}", tmp.display(), out.display()))?;
    if tmp_sig.is_file() {
        fs::rename(&tmp_sig, &out_sig).map_err(|e| format!("publish {} -> {}: {e}", tmp_sig.display(), out_sig.display()))?;
    }
    Ok(())
}
