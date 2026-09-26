use std::{
    fs,
    io,
    process::{Command, Output},
    time::Duration,
};
use std::path::Path;

use crate::extract;

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

/// `makepad-builder rust --root DIR [--version X.Y.Z]`: the private Rust
/// exactly as setup installs it, into DIR/toolchain/rust/<version>-<triple>,
/// with its progress and timing on stdout.
pub fn cli_install() -> Result<(), String> {
    let mut root = None;
    let mut version = DEFAULT_VERSION.to_string();
    let mut args = std::env::args().skip(2);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = Some(std::path::PathBuf::from(args.next().ok_or("--root needs a directory")?)),
            "--version" => version = args.next().ok_or("--version needs X.Y.Z")?,
            _ => return Err(format!("unknown rust option {arg}")),
        }
    }
    let root = crate::validate_install_root(&root.ok_or("rust requires --root DIR")?)?;
    let (cache, dest) = (root.join("cache"), crate::runtime::rust_dir(&root, &version));
    crate::timing::reset();
    let result = crate::progress::scope(crate::progress::printer(), || {
        crate::jobs::run(vec![crate::jobs::Job::new("Rust", || install_version(&cache, &dest, &version))])
    });
    for line in crate::timing::report() {
        println!("{line}");
    }
    result
}

pub fn install_version(cache: &Path, dest: &Path, version: &str) -> Result<(), String> {
    install_version_for(cache, dest, version, crate::catalog::platform())
}

/// [`install_version`] for `triple`: on Windows also Rust's GNU toolchain,
/// which adds rust-mingw (MinGW's runtime and import libraries).
pub fn install_version_for(cache: &Path, dest: &Path, version: &str, triple: &str) -> Result<(), String> {
    if version.split('.').count() != 3
        || !version
            .split('.')
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
    {
        return Err("A pinned Rust version is required".into());
    }
    let _timing = crate::timing::component("Rust");
    crate::progress::package("Rust", "Read toolchain manifest", 0, 0);
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
    let toml = String::from_utf8(crate::fetch::bytes(&channel)?)
        .map_err(|_| "Rust manifest is not UTF-8")?;
    let rustc = pkg_target(&toml, "rustc", triple)?;
    let std = pkg_target(&toml, "rust-std", triple)?;
    let cargo = pkg_target(&toml, "cargo", triple)?;
    // One bar for Rust: the three archives' sizes (cached, or the server's
    // Content-Length), each downloaded then unpacked.
    let mut parts = vec![rustc, cargo, std];
    if triple.ends_with("-windows-gnu") {
        parts.push(pkg_target(&toml, "rust-mingw", triple)?);
    }
    let sizes = crate::fetch::sizes(cache, &parts.iter().map(|p| (p.1.clone(), file_name(&p.1))).collect::<Vec<_>>());
    let _total = crate::progress::total("Rust", sizes.iter().sum());
    unpack_rust(cache, dest, version, &stamp, triple, parts, &sizes)
}

/// rustc, cargo and rust-std download side by side (each over several
/// connections) and each unpacks on the pool as soon as it has arrived,
/// straight into the staged toolchain: the part of each archive under its
/// component directory, exactly what merging the unpacked components gave.
fn unpack_rust(cache: &Path, dest: &Path, version: &str, stamp: &str, triple: &str, parts: Vec<(String, String, String)>, sizes: &[u64]) -> Result<(), String> {
    // Unpacked and verified beside the destination, then moved into place.
    // An unpack left by an interrupted run starts over from the cached archives.
    let tmp = dest.with_extension("unpack");
    let toolchains = dest.parent().ok_or("Rust destination has no parent")?;
    crate::remove_inside(toolchains, &tmp)?;
    let staged = tmp.join("ready");
    fs::create_dir_all(&staged).map_err(|e| e.to_string())?;
    let pending: Vec<_> = parts
        .into_iter()
        .zip(sizes)
        .enumerate()
        .map(|(order, ((name, url, hash), &size))| {
            let staged = staged.clone();
            let inner = if name == "rust-std" { format!("rust-std-{triple}") } else { name.clone() };
            crate::fetch::file_then(cache, &url, &file_name(&url), Some(&hash), size, move |gz| {
                crate::jobs::check_cancelled()?;
                crate::progress::package("Rust", &name, 0, 0);
                let tar = {
                    let _unpacking = crate::progress::activity(crate::progress::Activity::Unpack);
                    crate::progress::stage("Decompressing", &name, 0.0);
                    std::sync::Arc::new(extract::gunzip(&gz).map_err(|e| format!("gzip {name}: {e}"))?)
                };
                drop(gz);
                let entries = extract::tar_entries(&tar)?;
                let prefix = component_prefix(&entries, &inner);
                extract::write_tar_parallel(tar, entries, &staged, &prefix, order as u64, size)?;
                Ok(())
            })
        })
        .collect();
    crate::jobs::wait_all(pending)?;
    prepare_host_tools(&staged)?;
    check_rustc(&staged, version)?;
    fs::write(staged.join(".toolchain-version"), stamp).map_err(|e| e.to_string())?;
    move_staged(&staged, dest)?;
    let _ = crate::remove_inside(toolchains, &tmp);
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

/// A dist archive holds `<top>/<component>/…` plus installer files; only
/// the component directory is installed (`<top>` alone when it has none).
fn component_prefix(entries: &[extract::TarEntry], inner: &str) -> String {
    let top = entries.iter().find_map(|e| e.name.split('/').next().filter(|t| !t.is_empty())).unwrap_or_default();
    let nested = format!("{top}/{inner}/");
    if entries.iter().any(|e| e.name.starts_with(&nested)) {
        format!("{top}/{inner}")
    } else {
        top.to_string()
    }
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
        format!("{version} {}", crate::runtime::rust_triple(dest.ancestors().nth(3).unwrap_or(dest))),
    )
    .map_err(|e| e.to_string())?;
    move_staged(&staged, dest)?;
    if let Some(toolchains) = dest.parent() {
        let _ = crate::remove_inside(toolchains, &dest.with_extension("unpack"));
    }
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
    let _span = crate::timing::span(crate::timing::Phase::Check);
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

/// Waits for Windows security software to let go of fresh Rust files:
/// about 30 seconds in all.
const SCAN_WAITS_MS: [u64; 14] = [100, 250, 500, 1_000, 1_500, 2_000, 2_500, 3_000, 3_000, 3_000, 3_000, 3_000, 3_000, 3_000];

/// A scanner that just looked at rustc.exe still holds files in the folder,
/// and Windows refuses to rename a folder with open files. Try again with
/// the same bounded waits before asking the person to retry.
fn move_staged(staged: &Path, dest: &Path) -> Result<(), String> {
    let _span = crate::timing::span(crate::timing::Phase::Check);
    let mut waits = SCAN_WAITS_MS.iter();
    loop {
        match fs::rename(staged, dest) {
            Ok(()) => return Ok(()),
            Err(error) if cfg!(windows) && error.kind() == io::ErrorKind::PermissionDenied => match waits.next() {
                Some(ms) => {
                    crate::progress::stage("Checking compiler", "Waiting for Windows security to finish scanning Rust", 0.0);
                    std::thread::sleep(std::time::Duration::from_millis(*ms));
                }
                None => return Err(format!("{RETRY_COMPILER_CHECK}Rust files are still locked by Windows security software: {error}")),
            },
            Err(error) => return Err(error.to_string()),
        }
    }
}

/// Windows security scanners can briefly deny access to a freshly unpacked
/// executable. Retry only that transient class of failure, with a short
/// bounded delay; all other errors are returned immediately and a persistent
/// denial is returned after the final attempt.
fn tool_output(command: &mut Command, tool: &str) -> io::Result<Output> {
    #[cfg(windows)] {
        // Defender and other endpoint scanners can hold a newly unpacked
        // executable for many seconds. The bounded waits total about 30
        // seconds, with the delay capped so the UI remains responsive.
        const RETRY_DELAYS_MS: [u64; 14] = SCAN_WAITS_MS;
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
