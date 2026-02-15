//! Check command for cargo_makepad.
//!
//! Provides script validation and cross-platform build checking.

mod cargo;
mod error;
mod parser;
mod runtime;
mod script;

use crate::makepad_shell::*;
use crate::utils::*;
use makepad_toml_parser::*;

// Note: CheckError is defined in error module but currently only used internally

// Re-export for compatibility
use script::{check_scripts, CheckConfig};

/// Handle the `check script` subcommand.
pub fn handle_check_script(args: &[String]) -> Result<(), String> {
    if !args.is_empty() {
        return Err("Usage: cargo makepad check script".to_string());
    }

    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let config = CheckConfig::new(cwd);

    match check_scripts(&config) {
        Ok(result) => {
            result.print_issues();

            if result.has_errors() {
                Err(format!("{} script check error(s)", result.error_count))
            } else {
                if result.warning_count > 0 {
                    println!("Script checks completed with {} warning(s)", result.warning_count);
                } else {
                    println!("All script checks completed successfully");
                }
                Ok(())
            }
        }
        Err(e) => Err(e.to_string()),
    }
}

// ============================================================================
// Cross-platform build checking (existing functionality)
// ============================================================================

#[derive(Copy, Clone, Debug)]
enum BuildTy {
    Binary,
    BinaryBuildStd,
    Lib,
    LinuxDirect,
}

impl BuildTy {
    fn nightly_only(&self) -> bool {
        matches!(self, Self::BinaryBuildStd)
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Platform {
    Web,
    Mobile,
    Desktop,
    Embedded,
}

const TOOLCHAINS: [(&str, BuildTy, Platform); 16] = [
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

/// Check a crate on all supported platforms.
pub fn check_crate(build_crate: &str, args: &[String]) -> Result<(), String> {
    let crate_dir = get_crate_dir(build_crate).expect("Can't find crate dir");

    // Parse Cargo.toml for metadata
    let cargo_str = std::fs::read_to_string(crate_dir.join("Cargo.toml"))
        .expect("Can't find Cargo.toml");
    let toml = makepad_toml_parser::parse_toml(&cargo_str)
        .expect("Can't parse Cargo.toml");

    // Get platform filter from metadata
    let platforms = match toml.get("package.metadata.makepad-check-platform") {
        Some(Toml::Str(ver, _)) => ver.to_string(),
        _ => "desktop,web,mobile".to_string(),
    };

    let nightly_only = match toml.get("package.metadata.makepad-check-nightly-only") {
        Some(Toml::Bool(ver, _)) => *ver,
        _ => false,
    };

    let platform_filter = parse_platform_filter(&platforms)?;
    let build_count = count_builds(&platform_filter, nightly_only);

    println!("Check all for {} on {} builds", build_crate, build_count);

    let results = run_parallel_checks(&platform_filter, nightly_only, args);
    report_results(results)
}

fn parse_platform_filter(platforms: &str) -> Result<Vec<Platform>, String> {
    let mut filter = Vec::new();
    for platform in platforms.split(',') {
        match platform.trim() {
            "desktop" => filter.push(Platform::Desktop),
            "mobile" => filter.push(Platform::Mobile),
            "web" => filter.push(Platform::Web),
            "embedded" => filter.push(Platform::Embedded),
            e => return Err(format!("Unexpected platform in makepad-check-platform: {}", e)),
        }
    }
    Ok(filter)
}

fn count_builds(platform_filter: &[Platform], nightly_only: bool) -> usize {
    let mut count = 0;
    for (_, ty, platform) in TOOLCHAINS.iter() {
        if !platform_filter.contains(platform) {
            continue;
        }
        if nightly_only || ty.nightly_only() {
            count += 1;
        } else {
            count += 2; // stable + nightly
        }
    }
    count
}

struct BuildCheckResult {
    branch: &'static str,
    toolchain: String,
    build_type: BuildTy,
    stdout: String,
    stderr: String,
    success: bool,
}

fn run_parallel_checks(
    platform_filter: &[Platform],
    nightly_only: bool,
    args: &[String],
) -> Vec<BuildCheckResult> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut handles = Vec::new();

    for (index, (toolchain, ty, platform)) in TOOLCHAINS.iter().enumerate() {
        if !platform_filter.contains(platform) {
            continue;
        }

        let toolchain = toolchain.to_string();
        let ty = *ty;
        let args = args.to_vec();
        let sender = sender.clone();

        let handle = std::thread::spawn(move || {
            if !nightly_only && !ty.nightly_only() {
                let (stdout, stderr, success) = run_check(&toolchain, "stable", ty, &args, index);
                let _ = sender.send(BuildCheckResult {
                    branch: "stable",
                    toolchain: toolchain.clone(),
                    build_type: ty,
                    stdout,
                    stderr,
                    success,
                });
            }

            let (stdout, stderr, success) = run_check(&toolchain, "nightly", ty, &args, index);
            let _ = sender.send(BuildCheckResult {
                branch: "nightly",
                toolchain,
                build_type: ty,
                stdout,
                stderr,
                success,
            });
        });

        handles.push(handle);
    }

    // Wait for all threads
    for handle in handles {
        let _ = handle.join();
    }

    // Collect results
    let mut results = Vec::new();
    while let Ok(result) = receiver.try_recv() {
        results.push(result);
    }
    results
}

fn report_results(results: Vec<BuildCheckResult>) -> Result<(), String> {
    let mut has_errors = false;

    for result in results {
        if !result.success {
            has_errors = true;
            eprintln!(
                "Errors found in build {} {} {:?}",
                result.toolchain, result.branch, result.build_type
            );
        }

        if !result.stdout.is_empty() && result.stdout.contains("warning") {
            print!("{}", result.stdout);
        }

        if !result.success && !result.stderr.is_empty() {
            eprint!("{}", result.stderr);
        }
    }

    if has_errors {
        println!("Errors found whilst checking");
        Err("Errors found whilst checking".to_string())
    } else {
        println!("All checks completed successfully");
        Ok(())
    }
}

fn run_check(
    toolchain: &str,
    branch: &str,
    ty: BuildTy,
    args: &[String],
    par: usize,
) -> (String, String, bool) {
    let target_arg = format!("--target={}", toolchain);
    let target_dir = format!("--target-dir=target/check_all/check{}", par);
    let cwd = std::env::current_dir().unwrap();

    let mut args_out: Vec<&str> = vec!["run", branch, "cargo", "check", &target_arg];
    for arg in args {
        args_out.push(arg);
    }
    args_out.push(&target_dir);

    let makepad_env = match (ty, branch) {
        (BuildTy::LinuxDirect, "stable") => "linux_direct",
        (BuildTy::LinuxDirect, _) => "lines,linux_direct",
        (_, "stable") => " ",
        _ => "lines",
    };

    match ty {
        BuildTy::Binary | BuildTy::LinuxDirect => {
            shell_env_cap_split(&[("MAKEPAD", makepad_env)], &cwd, "rustup", &args_out)
        }
        BuildTy::BinaryBuildStd => {
            args_out.push("-Z");
            args_out.push("build-std=std");
            shell_env_cap_split(&[("MAKEPAD", makepad_env)], &cwd, "rustup", &args_out)
        }
        BuildTy::Lib => {
            args_out.push("--lib");
            shell_env_cap_split(&[("MAKEPAD", makepad_env)], &cwd, "rustup", &args_out)
        }
    }
}

/// Install all required toolchains for cross-platform checking.
fn rustup_toolchain_install() -> Result<(), String> {
    println!("Installing Rust toolchains for wasm");
    let cwd = std::env::current_dir().unwrap();

    shell_env(&[], &cwd, "rustup", &["update"])?;
    shell_env(&[], &cwd, "rustup", &["install", "nightly"])?;

    for (toolchain, ty, _) in TOOLCHAINS {
        let branches = if matches!(ty, BuildTy::BinaryBuildStd) {
            // For build-std targets, try both but don't fail
            vec![("nightly", false), ("stable", false)]
        } else {
            vec![("nightly", true), ("stable", true)]
        };

        for (branch, required) in branches {
            let args = ["target", "add", toolchain, "--toolchain", branch];
            if required {
                shell_env(&[], &cwd, "rustup", &args)?;
            } else {
                let _ = shell_env_cap(&[], &cwd, "rustup", &args);
            }
        }
    }

    Ok(())
}

/// Main entry point for the check command.
pub fn handle_check(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("Usage: cargo makepad check <subcommand>\n\nSubcommands:\n  script    Check script sources\n  all       Check build on all platforms\n  toolchain-install  Install required toolchains".to_string());
    }

    match args[0].as_ref() {
        "toolchain-install" | "install-toolchain" => rustup_toolchain_install(),
        "script" => handle_check_script(&args[1..]),
        "all" => {
            let cwd = std::env::current_dir().unwrap();
            if let Ok(build_crate) = get_build_crate_from_args(&args[1..]) {
                check_crate(&build_crate, &args[1..])
            } else if let Err(e) = shell_env_cap(&[], &cwd, "cargo", &["run", "--bin"]) {
                // Parse available binaries from error message
                let mut after_av = false;
                for line in e.split('\n') {
                    if after_av {
                        let binary = line.trim();
                        if !binary.is_empty() {
                            let mut args = args[1..].to_vec();
                            args.insert(0, binary.to_string());
                            args.insert(0, "-p".to_string());
                            check_crate(binary, &args)?;
                        }
                    }
                    if line.contains("Available binaries:") {
                        after_av = true;
                    }
                }
                Ok(())
            } else {
                Err("No crate to check".to_string())
            }
        }
        _ => Err(format!("Unknown check subcommand: {}", args[0])),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn unique_temp_dir(prefix: &str) -> std::path::PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("{}-{}-{}", prefix, std::process::id(), stamp));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn test_parse_platform_filter() {
        let filter = parse_platform_filter("desktop,web").unwrap();
        assert!(filter.contains(&Platform::Desktop));
        assert!(filter.contains(&Platform::Web));
        assert!(!filter.contains(&Platform::Mobile));
    }

    #[test]
    fn test_invalid_platform() {
        let result = parse_platform_filter("invalid");
        assert!(result.is_err());
    }
}
