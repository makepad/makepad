use std::{
    fs,
    io,
    process::{Command, Output},
    time::Duration,
};
use std::path::Path;

use crate::extract;
use crate::http;

pub const DEFAULT_VERSION: &str = "1.98.0";
/// Prefix used internally to keep a transient Windows executable lock
/// distinct from a compiler that is permanently invalid.
pub const RETRY_COMPILER_CHECK: &str = "retry-compiler-check:";

pub fn needs_compiler_retry(error: &str) -> bool {
    error.starts_with(RETRY_COMPILER_CHECK)
}

pub fn install(cache: &Path, dest: &Path) -> Result<(), String> {
    install_version(cache, dest, DEFAULT_VERSION)
}

pub fn install_version(cache: &Path, dest: &Path, version: &str) -> Result<(), String> {
    if version.split('.').count() != 3
        || !version
            .split('.')
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("A pinned Rust version is required".into());
    }
    crate::progress::package("Rust", "Read toolchain manifest", 0, 0);
    let triple = crate::catalog::platform();
    let stamp = format!("{version} {triple}");
    if fs::read_to_string(dest.join(".toolchain-version")).is_ok_and(|s| s == stamp)
        && dest
            .join("bin")
            .join(crate::runtime::exe("cargo"))
            .is_file()
        && dest
            .join("bin")
            .join(crate::runtime::exe("rustc"))
            .is_file()
    {
        prepare_host_tools(dest)?;
        crate::progress::stage("Ready", &format!("Rust {stamp} already installed"), 1.0);
        return Ok(());
    }
    if dest.exists() {
        return Err(format!(
            "Incomplete or different Rust toolchain at {}",
            dest.display()
        ));
    }
    let channel = format!("https://static.rust-lang.org/dist/channel-rust-{version}.toml");
    crate::setup_note!("rust: fetching {stamp}");
    let toml = String::from_utf8(http::fetch_bytes(&channel)?)
        .map_err(|_| "Rust manifest is not UTF-8")?;
    let rustc = pkg_target(&toml, "rustc", triple)?;
    let std = pkg_target(&toml, "rust-std", triple)?;
    let cargo = pkg_target(&toml, "cargo", triple)?;
    crate::progress::package("Rust downloads", "rustc", 1, 3);
    let rustc_path = http::cached_file(cache, &rustc.1, &file_name(&rustc.1), Some(&rustc.2))?;
    crate::progress::package("Rust downloads", "rust-std", 2, 3);
    let std_path = http::cached_file(cache, &std.1, &file_name(&std.1), Some(&std.2))?;
    crate::progress::package("Rust downloads", "cargo", 3, 3);
    let cargo_path = http::cached_file(cache, &cargo.1, &file_name(&cargo.1), Some(&cargo.2))?;

    let tmp = dest.with_extension("unpack");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;

    crate::progress::package("Install Rust", "rustc", 1, 3);
    extract::extract_tar_gz(&rustc_path, &tmp.join("rustc"))?;
    crate::progress::package("Install Rust", "rust-std", 2, 3);
    extract::extract_tar_gz(&std_path, &tmp.join("std"))?;
    crate::progress::package("Install Rust", "cargo", 3, 3);
    extract::extract_tar_gz(&cargo_path, &tmp.join("cargo"))?;

    let staged = tmp.join("ready");
    fs::create_dir_all(&staged).map_err(|e| e.to_string())?;
    merge_component(&tmp.join("rustc"), &staged, "rustc")?;
    merge_component(&tmp.join("cargo"), &staged, "cargo")?;
    merge_std(&tmp.join("std"), &staged)?;
    prepare_host_tools(&staged)?;
    check_rustc(&staged, version)?;
    fs::write(staged.join(".toolchain-version"), stamp).map_err(|e| e.to_string())?;
    move_staged(&staged, dest)?;
    let _ = fs::remove_dir_all(&tmp);
    if !dest
        .join("bin")
        .join(crate::runtime::exe("rustc"))
        .is_file()
    {
        return Err("rustc.exe missing after extract".into());
    }
    crate::progress::stage("Ready", "Rust installed and verified", 1.0);
    Ok(())
}

/// Re-check a compiler whose first post-extraction launch was blocked by
/// Windows security software. The staged archive is retained by
/// `install_version`, so this path performs no download or extraction.
pub fn retry_staged(dest: &Path, version: &str) -> Result<(), String> {
    if !cfg!(windows) {
        return Err("Staged compiler retries are only available on Windows".into());
    }
    let staged = dest.with_extension("unpack").join("ready");
    if !staged
        .join("bin")
        .join(crate::runtime::exe("rustc"))
        .is_file()
    {
        return Err("The staged Rust compiler is no longer available; choose Download compiler to start again".into());
    }
    crate::progress::stage("Checking compiler", "Retrying rustc --version", 0.0);
    check_rustc(&staged, version)?;
    fs::write(
        staged.join(".toolchain-version"),
        format!("{version} {}", crate::catalog::platform()),
    )
    .map_err(|e| e.to_string())?;
    move_staged(&staged, dest)?;
    let _ = fs::remove_dir_all(dest.with_extension("unpack"));
    crate::progress::stage("Ready", "Rust installed and verified", 1.0);
    Ok(())
}

pub(crate) fn prepare_host_tools(_dest: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let host = _dest.join("lib/rustlib").join(crate::catalog::platform());
        let library = host.join("lib/libLLVM.dylib");
        if _dest.join("lib/libLLVM.dylib").is_file() && !library.try_exists().map_err(|e| e.to_string())?
            && !library.is_symlink()
        {
            fs::create_dir_all(host.join("lib")).map_err(|e| e.to_string())?;
            std::os::unix::fs::symlink("../../../libLLVM.dylib", &library)
                .map_err(|e| format!("Link private Rust LLVM library: {e}"))?;
        }
        let mut command = Command::new(host.join("bin/rust-objcopy"));
        let output = tool_output(command.arg("--version"), "rust-objcopy")
            .map_err(|e| format!("Check private rust-objcopy: {e}"))?;
        if !output.status.success() {
            return Err(format!("Private rust-objcopy cannot load its LLVM library: {}", String::from_utf8_lossy(&output.stderr)));
        }
    }
    Ok(())
}

fn check_rustc(staged: &Path, version: &str) -> Result<(), String> {
    crate::progress::stage("Checking compiler", "Running rustc --version", 0.0);
    let mut check = Command::new(staged.join("bin").join(crate::runtime::exe("rustc")));
    crate::runtime::hide_console(&mut check);
    let output = tool_output(check.arg("--version"), "rustc").map_err(|error| {
        if cfg!(windows) && error.kind() == io::ErrorKind::PermissionDenied {
            format!(
                "{RETRY_COMPILER_CHECK}Rust executable access is still blocked: {error}"
            )
        } else {
            format!("Run private rustc: {error}")
        }
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if cfg!(windows) && stderr.to_ascii_lowercase().contains("access is denied") {
            return Err(format!(
                "{RETRY_COMPILER_CHECK}rustc reported access denied"
            ));
        }
        return Err("Extracted Rust compiler failed its version check".into());
    }
    if !String::from_utf8_lossy(&output.stdout).starts_with(&format!("rustc {version} ")) {
        return Err("Extracted Rust compiler failed its version check".into());
    }
    Ok(())
}

fn move_staged(staged: &Path, dest: &Path) -> Result<(), String> {
    fs::rename(staged, dest).map_err(|error| {
        if cfg!(windows) && error.kind() == io::ErrorKind::PermissionDenied {
            format!("{RETRY_COMPILER_CHECK}Rust files are still locked by Windows security software: {error}")
        } else {
            error.to_string()
        }
    })
}

/// Windows security scanners can briefly deny access to a freshly unpacked
/// executable. Retry only that transient class of failure, with a short
/// bounded delay; all other errors are returned immediately and a persistent
/// denial is returned after the final attempt.
fn tool_output(command: &mut Command, tool: &str) -> io::Result<Output> {
    #[cfg(windows)] {
        // Defender and other endpoint scanners can hold a newly unpacked
        // executable for several seconds. Eight bounded waits total about
        // 7.85 seconds, with the delay capped so the UI remains responsive.
        const RETRY_DELAYS_MS: [u64; 8] = [100, 250, 500, 1_000, 1_500, 1_500, 1_500, 1_500];
        const ATTEMPTS: usize = RETRY_DELAYS_MS.len() + 1;
        for attempt in 0..ATTEMPTS {
            match command.output() {
                Ok(output) => {
                    let denied = !output.status.success()
                        && String::from_utf8_lossy(&output.stderr)
                            .to_ascii_lowercase()
                            .contains("access is denied");
                    if !denied {
                        return Ok(output);
                    }
                    if attempt + 1 == ATTEMPTS {
                        return Err(io::Error::new(
                            io::ErrorKind::PermissionDenied,
                            format!(
                                "{tool} remained blocked by Windows security software after {} seconds",
                                RETRY_DELAYS_MS.iter().sum::<u64>() as f64 / 1_000.0
                            ),
                        ));
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::PermissionDenied
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) && attempt + 1 < ATTEMPTS => {}
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::PermissionDenied
                            | io::ErrorKind::WouldBlock
                            | io::ErrorKind::Interrupted
                    ) => {
                        return Err(io::Error::new(
                            error.kind(),
                            format!(
                                "{tool} remained inaccessible after {} seconds: {error}",
                                RETRY_DELAYS_MS.iter().sum::<u64>() as f64 / 1_000.0
                            ),
                        ));
                    }
                Err(error) => return Err(error),
            }
            crate::progress::stage(
                "Checking compiler",
                &format!(
                    "Waiting for Windows security scan ({tool}); retry {}/{}",
                    attempt + 1,
                    RETRY_DELAYS_MS.len()
                ),
                0.0,
            );
            std::thread::sleep(Duration::from_millis(RETRY_DELAYS_MS[attempt]));
        }
        unreachable!("bounded compiler probe loop always returns")
    }
    #[cfg(not(windows))]
    {
        let _ = (tool, Duration::from_millis(0));
        command.output()
    }
}

fn merge_component(unpacked: &Path, dest: &Path, inner: &str) -> Result<(), String> {
    let root = extract::single_child_dir(unpacked).unwrap_or_else(|| unpacked.to_path_buf());
    let from = if root.join(inner).is_dir() {
        root.join(inner)
    } else {
        root
    };
    extract::merge_dir(&from, dest)
}

fn merge_std(unpacked: &Path, dest: &Path) -> Result<(), String> {
    let root = extract::single_child_dir(unpacked).unwrap_or_else(|| unpacked.to_path_buf());
    let std_dir = root
        .read_dir()
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .map(|n| n.to_string_lossy().starts_with("rust-std-"))
                .unwrap_or(false)
        })
        .unwrap_or(root);
    extract::merge_dir(&std_dir, dest)
}

fn pkg_target(toml: &str, pkg: &str, triple: &str) -> Result<(String, String, String), String> {
    let header = format!("[pkg.{pkg}.target.{triple}]");
    let rest = toml
        .split(&header)
        .nth(1)
        .ok_or_else(|| format!("missing {header} in rust channel"))?;
    let section = rest.split("\n[").next().unwrap_or(rest);
    let url = toml_quoted(section, "url").ok_or_else(|| format!("no url in {header}"))?;
    let hash = toml_quoted(section, "hash").ok_or_else(|| format!("no hash in {header}"))?;
    Ok((pkg.to_string(), url, hash))
}

fn toml_quoted(section: &str, key: &str) -> Option<String> {
    for line in section.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(key) {
            let rest = rest.trim().strip_prefix('=')?.trim();
            if let Some(s) = rest.strip_prefix('"') {
                return Some(s.trim_end_matches('"').to_string());
            }
        }
    }
    None
}

fn file_name(url: &str) -> String {
    url.rsplit('/').next().unwrap_or("download").to_string()
}
