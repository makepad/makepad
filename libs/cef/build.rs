use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

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

fn run_download_script(workspace_root: &Path, platform: &str) {
    let script = workspace_root.join("download_cef.sh");
    let status = Command::new(&script)
        .arg("--platform")
        .arg(platform)
        .status()
        .unwrap_or_else(|err| panic!("failed to execute {}: {err}", script.display()));
    if !status.success() {
        panic!("{} failed with status {status}", script.display());
    }
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
        .unwrap_or_else(|err| panic!("failed to execute clang for {}: {err}", helper_source.display()));
    if !status.success() {
        panic!("clang failed to build {}", helper_binary.display());
    }

    helper_binary
}

fn main() {
    println!("cargo:rerun-if-env-changed=MAKEPAD_CEF_DIST_DIR");
    println!("cargo:rerun-if-changed=../../download_cef.sh");
    println!("cargo:rerun-if-changed=helper_main_macos.c");
    println!("cargo:rustc-check-cfg=cfg(makepad_cef_api_ge_13800)");
    println!("cargo:rustc-check-cfg=cfg(makepad_cef_api_ge_14600)");

    let Some(platform) = target_platform() else {
        return;
    };

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

    let dist_dir = env::var_os("MAKEPAD_CEF_DIST_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            workspace_root
                .join("local/cef-prebuilt")
                .join(format!("current-{platform}"))
        });

    if !dist_dir.exists() {
        run_download_script(&workspace_root, platform);
    }

    if !dist_dir.exists() {
        panic!("CEF distribution not found at {}", dist_dir.display());
    }

    let dist_dir = dist_dir
        .canonicalize()
        .unwrap_or_else(|err| panic!("failed to canonicalize {}: {err}", dist_dir.display()));
    let include_dir = dist_dir.join("include");

    println!(
        "cargo:rustc-env=MAKEPAD_CEF_DIST_DIR={}",
        dist_dir.display()
    );
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
