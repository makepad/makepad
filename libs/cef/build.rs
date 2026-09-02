use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "build_support/cef_dist.rs"]
mod cef_dist;

fn target_platform() -> Option<&'static str> {
    let os = env::var("CARGO_CFG_TARGET_OS").ok()?;
    let arch = env::var("CARGO_CFG_TARGET_ARCH").ok()?;
    match (os.as_str(), arch.as_str()) {
        ("macos", "aarch64") => Some("macosarm64"),
        ("macos", "x86_64") => Some("macosx64"),
        ("linux", "x86_64") => Some("linux64"),
        ("linux", "aarch64") => Some("linuxarm64"),
        ("windows", "x86_64") => Some("windows64"),
        ("windows", "aarch64") => Some("windowsarm64"),
        _ => None,
    }
}

fn parse_api_version(include_dir: &Path) -> Option<String> {
    let header = fs::read_to_string(include_dir.join("cef_api_versions.h")).ok()?;
    for line in header.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("#define CEF_API_VERSION_LAST CEF_API_VERSION_") {
            return Some(value.trim().to_string());
        }
    }
    None
}

/// `#define CEF_VERSION "138.0.59+g21d63d5+chromium-138.0.7204.306"` from the
/// prebuilt's own header (authoritative for the binary we link).
fn parse_cef_version(include_dir: &Path) -> Option<String> {
    let header = fs::read_to_string(include_dir.join("cef_version.h")).ok()?;
    for line in header.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("#define CEF_VERSION ") {
            return Some(value.trim().trim_matches('"').to_string());
        }
    }
    None
}

fn build_macos_helper(manifest_dir: &Path, dist_dir: &Path, include_dir: &Path) -> PathBuf {
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let helper_source = manifest_dir.join("helper_main_macos.c");
    let helper_binary = out_dir.join("makepad-cef-helper");

    let status = Command::new("clang")
        .arg("-std=c11")
        .arg("-mmacosx-version-min=12.0")
        .arg("-I")
        .arg(dist_dir)
        .arg("-I")
        .arg(include_dir)
        .arg(&helper_source)
        .arg("-o")
        .arg(&helper_binary)
        .status()
        .unwrap_or_else(|err| {
            panic!(
                "failed to execute clang for {}: {err}",
                helper_source.display()
            )
        });
    if !status.success() {
        panic!("clang failed to build {}", helper_binary.display());
    }

    helper_binary
}

fn main() {
    println!("cargo:rerun-if-env-changed=MAKEPAD_CEF_DIST_DIR");
    println!("cargo:rerun-if-env-changed=MAKEPAD_CEF_VERSION");
    println!("cargo:rerun-if-env-changed=MAKEPAD_CEF_OFFLINE");
    println!("cargo:rerun-if-env-changed=MAKEPAD_CEF_DRY_RUN");
    println!("cargo:rerun-if-env-changed=MAKEPAD_CEF_PLATFORM");
    println!("cargo:rerun-if-changed=build_support/cef_dist.rs");
    println!("cargo:rerun-if-changed=helper_main_macos.c");
    println!("cargo:rustc-check-cfg=cfg(makepad_cef_api_ge_13800)");
    println!("cargo:rustc-check-cfg=cfg(makepad_cef_api_ge_14600)");

    // MAKEPAD_CEF_PLATFORM: a dry-run aid — resolve the index for another
    // platform from this host (what WOULD be fetched for linux64, say)
    // without cross-compiling. Only honoured together with
    // MAKEPAD_CEF_DRY_RUN; a real build always links its own target.
    let dry_run = env::var("MAKEPAD_CEF_DRY_RUN").map(|v| v != "0").unwrap_or(false);
    let platform: String = match env::var("MAKEPAD_CEF_PLATFORM").ok().filter(|_| dry_run) {
        Some(forced) => forced,
        None => match target_platform() {
            Some(p) => p.to_string(),
            None => return,
        },
    };
    let platform = platform.as_str();
    let pin = env::var("MAKEPAD_CEF_VERSION").ok().filter(|v| !v.trim().is_empty());
    let offline = env::var("MAKEPAD_CEF_OFFLINE").map(|v| v != "0").unwrap_or(false);

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let workspace_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| {
            panic!(
                "failed to resolve workspace root from {}",
                manifest_dir.display()
            )
        })
        .to_path_buf();

    let prebuilt_dir = workspace_root.join("local/cef-prebuilt");

    if dry_run {
        // Resolve and report, download nothing: the plan for this (or the
        // forced) platform with the pin applied. The build then goes on
        // with whatever dist THIS target already has, so a dry run is a
        // normal, complete build that also printed what it would fetch.
        match cef_dist::plan(&prebuilt_dir, platform, pin.as_deref()) {
            Ok(plan) => println!("cargo:warning=cef dry run — {}", plan.describe().replace('\n', " ")),
            Err(e) => println!("cargo:warning=cef dry run failed — {e}"),
        }
    }
    let platform = match target_platform() {
        Some(p) => p,
        None => return,
    };

    // MAKEPAD_CEF_DIST_DIR names a distribution directly and is used as
    // it is; otherwise the prebuilt dir's pointer for this platform, else a
    // download into it. A platform that already has a dist is never bumped.
    let dist_dir = match env::var_os("MAKEPAD_CEF_DIST_DIR").map(PathBuf::from) {
        Some(dir) => {
            if !dir.join("include").is_dir() {
                panic!(
                    "MAKEPAD_CEF_DIST_DIR={} is not a CEF distribution (no include/ inside it); unset it to let the build fetch one into {}",
                    dir.display(),
                    prebuilt_dir.display()
                );
            }
            dir
        }
        None => cef_dist::ensure_dist(&prebuilt_dir, platform, pin.as_deref(), offline)
            .unwrap_or_else(|e| panic!("{e}")),
    };

    let dist_dir = dist_dir
        .canonicalize()
        .unwrap_or_else(|err| panic!("failed to canonicalize {}: {err}", dist_dir.display()));
    let include_dir = dist_dir.join("include");

    println!(
        "cargo:rustc-env=MAKEPAD_CEF_DIST_DIR={}",
        dist_dir.display()
    );
    if let Some(cef_version) = parse_cef_version(&include_dir) {
        println!("cargo:rustc-env=MAKEPAD_CEF_VERSION={cef_version}");
    }
    if let Some(api_version) = parse_api_version(&include_dir) {
        println!("cargo:rustc-env=MAKEPAD_CEF_API_VERSION={api_version}");
        if let Ok(api_version_number) = api_version.parse::<u32>() {
            if api_version_number >= 13800 {
                println!("cargo:rustc-cfg=makepad_cef_api_ge_13800");
            }
            if api_version_number >= 14600 {
                println!("cargo:rustc-cfg=makepad_cef_api_ge_14600");
            }
        }
    }

    if env::var("CARGO_CFG_TARGET_OS").ok().as_deref() == Some("macos") {
        let framework_dir = dist_dir
            .join("Release")
            .join("Chromium Embedded Framework.framework");
        let framework_bin = framework_dir.join("Chromium Embedded Framework");
        let resources_dir = framework_dir.join("Resources");
        let helper_binary = build_macos_helper(&manifest_dir, &dist_dir, &include_dir);

        println!(
            "cargo:rustc-env=MAKEPAD_CEF_FRAMEWORK_DIR={}",
            framework_dir.display()
        );
        println!(
            "cargo:rustc-env=MAKEPAD_CEF_FRAMEWORK_BIN={}",
            framework_bin.display()
        );
        println!(
            "cargo:rustc-env=MAKEPAD_CEF_RESOURCES_DIR={}",
            resources_dir.display()
        );
        println!(
            "cargo:rustc-env=MAKEPAD_CEF_HELPER_BIN={}",
            helper_binary.display()
        );
    }
}
