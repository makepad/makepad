//! `cargo makepad android proc-pack`: the APK of the window manager whose
//! apps are real child PROCESSES (`apps/wm-android`), the phone's version of
//! the desktop WM hosting its apps over the hub.
//!
//! Three builds, one APK:
//! - the host: cargo-makepad's own Android build of the WM crate (`-p`);
//! - the launcher (`[package.metadata.makepad.proc] launcher`), an executable
//!   packed as `lib/<abi>/libmakepad_launch.so` — the native-library
//!   directory is the one place an app may exec from;
//! - one library per hosted app (`[package.metadata.makepad.proc] apps`, each
//!   `"<package> <bin>"`), packed as `lib/<abi>/libapp_<bin>.so`: the app's
//!   own `src/main.rs` (its `app_main!`, whose `makepad_hosted_main` the
//!   launcher calls) built as a cdylib by a generated wrapper crate, the
//!   engine linked statically. Every app is self-contained: no engine dylib,
//!   no shared Rust ABI between the WM and its apps. What they share is the
//!   hosting protocol, the shared frames (AHardwareBuffers) and the touches
//!   (platform/src/os/linux/android/android_hosted.rs).
//!
//! The apps' resources (and whatever of their dependencies' resources the
//! host's APK lacks) go into `assets/makepad/<crate>/…`, where a hosted child
//! reads them straight from the APK file.
//!
//! `--proc-toolchain=<musl rustc tree>`: the phone builds its apps itself.
//! The app libraries are then built from a staged, relocatable tree
//! (dyn_pack's stage, [`super::dyn_pack::proc_build`]) and the APK also
//! carries that tree, its target/ and the toolchain; the launcher's
//! `--build` mode compiles an app with them before running it.

use super::compile;
use super::dyn_pack::proc_build;
use super::sdk::AndroidSDKUrls;
use super::{AndroidConfig, AndroidTarget, AndroidVariant, HostOs};
use crate::utils::{get_build_crate_from_args, get_crate_dep_dirs, get_crate_dir, VersionCodeStrategy};
use makepad_zip_file::{zip_read_central_directory, ZipMethod, ZipWriter, COMPRESS_METHOD_UNCOMPRESSED};
use std::{
    collections::BTreeSet,
    fs,
    io::BufWriter,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

/// One hosted app: its cargo package and the WM registry's binary name.
struct HostedApp {
    package: String,
    bin: String,
}

#[allow(clippy::too_many_arguments)]
pub fn proc_pack(
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
    let t0 = Instant::now();
    let (proc_opts, args) = proc_build::split_proc_options(args);
    let args = &args[..];
    // `--proc-apps=<bin,bin>`: pack only these of the listed apps.
    let only: Option<Vec<String>> = args
        .iter()
        .find_map(|a| a.strip_prefix("--proc-apps="))
        .map(|list| list.split(',').map(str::to_string).collect());
    let args: Vec<String> = args.iter().filter(|a| !a.starts_with("--proc-apps=")).cloned().collect();
    let args = &args[..];
    let [android_target] = android_targets else {
        return Err("proc-pack builds one ABI at a time: pass --abi=aarch64".into());
    };
    // Every process renders with Vulkan: the shared frames are Vulkan imports.
    if std::env::var("MAKEPAD").map(|v| !v.contains("vulkan")).unwrap_or(true) {
        let makepad = match std::env::var("MAKEPAD") {
            Ok(v) if !v.is_empty() => format!("{v}+vulkan"),
            _ => "vulkan".to_string(),
        };
        std::env::set_var("MAKEPAD", makepad);
    }
    let host = get_build_crate_from_args(args)?.to_string();
    let host_dir = get_crate_dir(&host)?;
    let host_manifest = fs::read_to_string(host_dir.join("Cargo.toml"))
        .map_err(|e| format!("{}: {e}", host_dir.display()))?;
    let mut apps = hosted_apps(&host_manifest)?;
    if let Some(only) = &only {
        apps.retain(|app| only.contains(&app.bin));
    }
    let launcher = metadata_string(&host_manifest, "launcher")
        .unwrap_or_else(|| "makepad-wm-android-launch".to_string());
    let release = args.iter().any(|a| a == "--release");
    let profile = if release { "release" } else { "debug" };

    if proc_opts.toolchain.is_some() {
        // The phone builds from a tree in the app's files dir: debuggable,
        // so `adb shell run-as <package>` can reach and edit it.
        std::env::set_var("MAKEPAD_ANDROID_DEBUGGABLE", "1");
    }
    println!("== host APK ({host})");
    let result = compile::build(
        sdk_dir,
        host_os,
        package_name,
        app_label,
        version_code,
        version_name,
        min_sdk_version,
        args,
        android_targets,
        variant,
        config,
        urls,
    )?;
    let mut host_apk = result.apk().to_path_buf();
    if proc_opts.toolchain.is_some() {
        // The on-device pack takes a while; plain proc-pack builds of the
        // same crate share cargo-makepad's APK dir. Keep this run's host APK.
        let base = proc_build::proc_base(&std::env::current_dir().map_err(|e| format!("cwd: {e}"))?, &host);
        fs::create_dir_all(&base).map_err(|e| format!("{}: {e}", base.display()))?;
        let own = base.join(host_apk.file_name().ok_or("host apk has no name")?);
        fs::copy(&host_apk, &own).map_err(|e| format!("{} -> {}: {e}", host_apk.display(), own.display()))?;
        // That one is debuggable (MAKEPAD_ANDROID_DEBUGGABLE above): it must
        // not stay behind as cargo-makepad's ordinary release APK.
        let _ = fs::remove_file(&host_apk);
        let _ = fs::remove_file(PathBuf::from(format!("{}.idsig", host_apk.display())));
        std::env::remove_var("MAKEPAD_ANDROID_DEBUGGABLE");
        host_apk = own;
    }

    let cwd = std::env::current_dir().map_err(|e| format!("cwd: {e}"))?;
    let target_dir = compile::cargo_target_dir(&cwd);
    let toolchain = android_target.toolchain();
    let env = cargo_env(sdk_dir, host_os, urls, android_target)?;
    let out_dir = target_dir.join(toolchain).join(profile);

    println!("== launcher ({launcher})");
    let mut launcher_args = vec!["build".to_string(), "-p".into(), launcher.clone()];
    launcher_args.extend(common_cargo_args(&target_dir, toolchain, release));
    run_cargo(&cwd, &env, &launcher_args)?;
    let launcher_bin = out_dir.join(launcher_bin_name(&launcher)?);

    let out = host_apk.with_file_name(format!(
        "{}-proc{}.apk",
        host_apk.file_stem().and_then(|s| s.to_str()).unwrap_or("app"),
        if proc_opts.toolchain.is_some() { "-ondevice" } else { "" }
    ));
    // The app assets are listed before an on-device stage takes over this
    // process's environment (cargo tree runs under the caller's).
    let assets = app_assets(&apps, &target_dir, toolchain)?;

    println!("== hosted apps ({} libraries)", apps.len());
    let (lib_dir, tree) = if proc_opts.toolchain.is_some() {
        if !release {
            return Err("--proc-toolchain packs a release tree: pass --release".into());
        }
        let pairs: Vec<(String, String)> = apps.iter().map(|a| (a.package.clone(), a.bin.clone())).collect();
        let tree = proc_build::prepare(sdk_dir, host_os, urls, *android_target, &host, min_sdk_version, &pairs, &out, &proc_opts)?;
        (tree.out_dir(), Some(tree))
    } else {
        let wrappers = write_wrapper_workspace(&cwd, &target_dir, &apps)?;
        let mut app_args = vec![
            "build".to_string(),
            format!("--manifest-path={}", wrappers.join("Cargo.toml").display()),
            "--workspace".into(),
        ];
        app_args.extend(common_cargo_args(&target_dir, toolchain, release));
        run_cargo(&wrappers, &env, &app_args)?;
        (out_dir.clone(), None)
    };

    println!("== pack");
    let abi = android_target.abi_identifier();
    let mut libs: Vec<(String, PathBuf)> = vec![("libmakepad_launch.so".into(), launcher_bin)];
    for app in &apps {
        let name = format!("libapp_{}.so", app.bin.replace('-', "_"));
        libs.push((name.clone(), lib_dir.join(&name)));
    }
    pack(sdk_dir, urls, &host_apk, &out, abi, &libs, &assets)?;
    println!("{} in {:.0} s", out.display(), t0.elapsed().as_secs_f64());
    if let Some(tree) = tree {
        proc_build::finish(tree, &out)?;
    }
    Ok(())
}

/// `apps = ["<package> <bin>", …]` of `[package.metadata.makepad.proc]`.
fn hosted_apps(manifest: &str) -> Result<Vec<HostedApp>, String> {
    let mut in_section = false;
    let mut in_apps = false;
    let mut apps = Vec::new();
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') && !in_apps {
            in_section = line == "[package.metadata.makepad.proc]";
            continue;
        }
        if !in_section {
            continue;
        }
        if line.starts_with("apps") && line.contains('[') {
            in_apps = true;
            continue;
        }
        if in_apps {
            if line.starts_with(']') {
                in_apps = false;
                continue;
            }
            let entry = line.trim_end_matches(',').trim_matches('"');
            let mut parts = entry.split_whitespace();
            if let (Some(package), Some(bin)) = (parts.next(), parts.next()) {
                apps.push(HostedApp { package: package.to_string(), bin: bin.to_string() });
            }
        }
    }
    if apps.is_empty() {
        return Err("the host crate lists no [package.metadata.makepad.proc] apps".into());
    }
    Ok(apps)
}

fn metadata_string(manifest: &str, key: &str) -> Option<String> {
    let mut in_section = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_section = line == "[package.metadata.makepad.proc]";
            continue;
        }
        if in_section {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    None
}

fn launcher_bin_name(package: &str) -> Result<String, String> {
    let dir = get_crate_dir(package)?;
    let manifest = fs::read_to_string(dir.join("Cargo.toml")).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut in_bin = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_bin = line == "[[bin]]";
            continue;
        }
        if in_bin {
            if let Some((k, v)) = line.split_once('=') {
                if k.trim() == "name" {
                    return Ok(v.trim().trim_matches('"').to_string());
                }
            }
        }
    }
    Ok(package.to_string())
}

/// The environment cargo-makepad's own Android build compiles with.
fn cargo_env(
    sdk_dir: &Path,
    host_os: HostOs,
    urls: &AndroidSDKUrls,
    android_target: &AndroidTarget,
) -> Result<Vec<(String, String)>, String> {
    let linker = compile::android_linker_path(sdk_dir, host_os, urls, android_target)?;
    let bin = linker.parent().ok_or("the NDK clang has no directory")?;
    let toolchain = android_target.toolchain();
    let clangpp = format!("{}++", linker.to_string_lossy());
    let mut env = vec![
        (android_target.linker_env_var().to_string(), linker.to_string_lossy().to_string()),
        (format!("CC_{toolchain}"), linker.to_string_lossy().to_string()),
        (format!("CXX_{toolchain}"), clangpp),
        (format!("AR_{toolchain}"), bin.join("llvm-ar").to_string_lossy().to_string()),
        (format!("RANLIB_{toolchain}"), bin.join("llvm-ranlib").to_string_lossy().to_string()),
        (
            "RUSTFLAGS".to_string(),
            compile::android_rustflags(std::env::var("RUSTFLAGS").ok().as_deref(), android_target, false),
        ),
    ];
    if let Ok(makepad) = std::env::var("MAKEPAD") {
        env.push(("MAKEPAD".to_string(), makepad));
    }
    Ok(env)
}

fn common_cargo_args(target_dir: &Path, toolchain: &str, release: bool) -> Vec<String> {
    let mut args = vec![format!("--target={toolchain}"), format!("--target-dir={}", target_dir.display())];
    if release {
        args.push("--release".to_string());
    }
    args
}

fn run_cargo(cwd: &Path, env: &[(String, String)], args: &[String]) -> Result<(), String> {
    let mut cmd = Command::new("rustup");
    cmd.args(["run", "stable", "cargo"]).args(args).current_dir(cwd);
    for (key, value) in env {
        cmd.env(key, value);
    }
    println!("cargo {}", args.join(" "));
    let status = cmd.status().map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err(format!("cargo {} failed", args.join(" ")));
    }
    Ok(())
}

/// A workspace of one wrapper crate per app, under the target dir: the
/// wrapper's cdylib is the app's `src/main.rs`, depending on the app's
/// library and on the app's own dependencies (paths made absolute).
fn write_wrapper_workspace(cwd: &Path, target_dir: &Path, apps: &[HostedApp]) -> Result<PathBuf, String> {
    let root = target_dir.join("makepad-android-proc");
    fs::create_dir_all(&root).map_err(|e| format!("{}: {e}", root.display()))?;
    let mut members = Vec::new();
    for app in apps {
        let crate_dir = get_crate_dir(&app.package)?;
        let manifest = fs::read_to_string(crate_dir.join("Cargo.toml"))
            .map_err(|e| format!("{}: {e}", crate_dir.display()))?;
        let main_rs = crate_dir.join("src/main.rs");
        if !main_rs.is_file() {
            return Err(format!("{} has no src/main.rs to host", app.package));
        }
        let name = format!("app_{}", app.bin.replace('-', "_"));
        let dir = root.join(&name);
        fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut out = format!(
            "[package]\nname = \"makepad-hosted-{bin}\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n\
             [lib]\nname = \"{name}\"\npath = \"{main}\"\ncrate-type = [\"cdylib\"]\n\n",
            bin = app.bin,
            main = main_rs.display()
        );
        let sections = copied_sections(&manifest);
        let rewritten = compile::rewrite_wrapper_manifest_paths(&sections, &crate_dir);
        let dep_line = format!("{} = {{ path = \"{}\" }}\n", app.package, crate_dir.display());
        if rewritten.contains("[dependencies]\n") {
            out.push_str(&rewritten.replacen("[dependencies]\n", &format!("[dependencies]\n{dep_line}"), 1));
        } else {
            out.push_str(&format!("[dependencies]\n{dep_line}\n{rewritten}"));
        }
        fs::write(dir.join("Cargo.toml"), out).map_err(|e| format!("{}: {e}", dir.display()))?;
        members.push(name);
    }
    let mut workspace = String::from("[workspace]\nresolver = \"2\"\nmembers = [\n");
    for member in &members {
        workspace.push_str(&format!("    \"{member}\",\n"));
    }
    workspace.push_str("]\n");
    if let Ok(root_manifest) = fs::read_to_string(cwd.join("Cargo.toml")) {
        let patches = compile::extract_workspace_patch_sections(&root_manifest);
        if !patches.trim().is_empty() {
            workspace.push('\n');
            workspace.push_str(&compile::rewrite_wrapper_manifest_paths(&patches, cwd));
        }
    }
    fs::write(root.join("Cargo.toml"), workspace).map_err(|e| format!("{}: {e}", root.display()))?;
    if let Ok(lock) = fs::read(cwd.join("Cargo.lock")) {
        let _ = fs::write(root.join("Cargo.lock"), lock);
    }
    Ok(root)
}

/// The `[features]`, `[dependencies]` and `[target.*.dependencies]` sections
/// of an app manifest: what its `src/main.rs` compiles against.
pub(crate) fn copied_sections(manifest: &str) -> String {
    let mut out = String::new();
    let mut keep = false;
    for line in manifest.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            keep = trimmed == "[dependencies]"
                || trimmed == "[features]"
                || (trimmed.starts_with("[target.") && trimmed.ends_with(".dependencies]"));
            if keep {
                out.push('\n');
            }
        }
        if keep {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

/// Every resource file of the hosted apps and of their dependencies, as
/// `(assets/makepad/<crate>/<dir>/<rel>, file)`.
fn app_assets(apps: &[HostedApp], target_dir: &Path, toolchain: &str) -> Result<Vec<(String, PathBuf)>, String> {
    let mut crates: Vec<(String, PathBuf)> = Vec::new();
    for app in apps {
        crates.push((app.package.clone(), get_crate_dir(&app.package)?));
        for (name, dir) in get_crate_dep_dirs(&app.package, target_dir, toolchain) {
            crates.push((name, dir));
        }
    }
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    for (name, dir) in crates {
        let crate_name = name.replace('-', "_");
        for sub in ["resources", "fonts"] {
            let base = dir.join(sub);
            if !base.is_dir() {
                continue;
            }
            let mut files = Vec::new();
            walk(&base, &mut files)?;
            for file in files {
                let rel = file.strip_prefix(&base).unwrap().to_string_lossy().replace('\\', "/");
                if sub == "resources" && (rel.starts_with("android/") || rel.starts_with("ios/")) {
                    continue;
                }
                let key = format!("assets/makepad/{crate_name}/{sub}/{rel}");
                if seen.insert(key.clone()) {
                    out.push((key, file));
                }
            }
        }
    }
    Ok(out)
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let path = entry.map_err(|e| format!("{}: {e}", dir.display()))?.path();
        if path.is_dir() {
            walk(&path, out)?;
        } else if path.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// Re-zip the host APK (minus its signature) with the launcher, the app
/// libraries and the assets it lacks, then align and sign it.
fn pack(
    sdk_dir: &Path,
    urls: &AndroidSDKUrls,
    host_apk: &Path,
    out: &Path,
    abi: &str,
    libs: &[(String, PathBuf)],
    assets: &[(String, PathBuf)],
) -> Result<(), String> {
    let unaligned = out.with_extension("unaligned.apk");
    let mut zin = fs::File::open(host_apk).map_err(|e| format!("{}: {e}", host_apk.display()))?;
    let dir = zip_read_central_directory(&mut zin).map_err(|e| format!("{}: {e:?}", host_apk.display()))?;
    let file = fs::File::create(&unaligned).map_err(|e| format!("{}: {e}", unaligned.display()))?;
    let mut zout = ZipWriter::to(BufWriter::with_capacity(1 << 20, file));
    let mut present = BTreeSet::new();
    for h in &dir.file_headers {
        if h.file_name.starts_with("META-INF/") {
            continue;
        }
        let data = h.extract(&mut zin).map_err(|e| format!("{}: {e:?}", h.file_name))?;
        let method = if h.compression_method == COMPRESS_METHOD_UNCOMPRESSED { ZipMethod::Store } else { ZipMethod::Deflate };
        zout.add(&h.file_name, &data, method).map_err(|e| format!("{}: {e}", h.file_name))?;
        present.insert(h.file_name.clone());
    }
    for (name, path) in libs {
        let data = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        // Deflated: the package manager extracts it to nativeLibraryDir,
        // the only directory the WM may exec the launcher from.
        zout.add(&format!("lib/{abi}/{name}"), &data, ZipMethod::Deflate).map_err(|e| format!("{name}: {e}"))?;
        println!("  lib/{abi}/{name}: {:.1} MB", data.len() as f64 / 1e6);
    }
    let mut added = 0;
    for (name, path) in assets {
        if present.contains(name) {
            continue;
        }
        let data = fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        zout.add(name, &data, ZipMethod::Deflate).map_err(|e| format!("{name}: {e}"))?;
        added += 1;
    }
    println!("  {added} app assets added");
    zout.finish().map_err(|e| format!("{}: {e}", unaligned.display()))?;
    if out.exists() {
        fs::remove_file(out).map_err(|e| format!("{}: {e}", out.display()))?;
    }
    compile::zipalign_apk(sdk_dir, urls, &unaligned, out)?;
    compile::sign_apk_debug(sdk_dir, urls, out)?;
    fs::remove_file(&unaligned).map_err(|e| format!("{}: {e}", unaligned.display()))?;
    Ok(())
}
