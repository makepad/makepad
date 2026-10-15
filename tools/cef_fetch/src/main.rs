//! Explicit distribution setup; building this tool never fetches CEF.
mod bz2;
mod cef_dist;
mod tar;

use std::path::{Path, PathBuf};

fn run() -> Result<(), String> {
    let mut fetch = false;
    let mut dry_run = false;
    let mut platform = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "macosarm64",
        ("macos", "x86_64") => "macosx64",
        ("linux", "x86_64") => "linux64",
        ("linux", "aarch64") => "linuxarm64",
        ("windows", "x86_64") => "windows64",
        ("windows", "aarch64") => "windowsarm64",
        _ => "unsupported",
    }
    .to_owned();
    let mut pin = std::env::var("MAKEPAD_CEF_VERSION")
        .ok()
        .filter(|v| !v.trim().is_empty());
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--fetch" => fetch = true,
            "--dry-run" => dry_run = true,
            "--platform" => platform = args.next().ok_or("--platform needs a value")?,
            "--version" => pin = Some(args.next().ok_or("--version needs a CEF version")?),
            "--help" | "-h" => {
                println!("makepad-cef-fetch [--fetch | --dry-run] [--platform PLATFORM] [--version CEF_VERSION]\n\
                    Without --fetch: select an installed distribution and write its pointer, with no network.\n\
                    --fetch: download/verify/extract if none is installed (~300 MB from cef-builds.spotifycdn.com).\n\
                    --dry-run: read the online index and print the download plan without downloading an archive.");
                return Ok(());
            }
            _ => return Err(format!("unknown argument {arg}; use --help")),
        }
    }
    if !cef_dist::PLATFORMS.contains(&platform.as_str()) {
        return Err(format!("unsupported platform {platform}"));
    }
    if fetch && dry_run {
        return Err("use either --fetch or --dry-run".into());
    }
    if (fetch || dry_run)
        && std::env::var("CARGO_NET_OFFLINE").is_ok_and(|v| v == "true" || v == "1")
    {
        return Err("network access requested while CARGO_NET_OFFLINE is set".into());
    }
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or("cannot find workspace")?;
    let prebuilt = workspace.join("local/cef-prebuilt");
    let shared = std::fs::read_to_string(workspace.join(".git"))
        .ok()
        .and_then(|text| {
            text.trim()
                .strip_prefix("gitdir:")
                .map(|dir| workspace.join(dir.trim()))
        })
        .and_then(|dir| dir.canonicalize().ok())
        .and_then(|dir| {
            dir.ancestors()
                .find(|p| p.file_name().is_some_and(|n| n == ".git"))
                .and_then(Path::parent)
                .map(|root| root.join("local/cef-prebuilt"))
        });
    if dry_run {
        println!(
            "{}",
            cef_dist::plan(&prebuilt, &platform, pin.as_deref())?.describe()
        );
        return Ok(());
    }
    let dist = match std::env::var_os("MAKEPAD_CEF_DIST_DIR").map(PathBuf::from) {
        Some(dir) => {
            if !dir.join("include").is_dir() {
                return Err(format!("{} has no include/ directory", dir.display()));
            }
            dir
        }
        None => cef_dist::ensure_dist(
            &prebuilt,
            shared.as_deref(),
            &platform,
            pin.as_deref(),
            if fetch {
                cef_dist::Fetch::Allowed
            } else {
                cef_dist::Fetch::Never
            },
        )?,
    };
    println!("{}", dist.display());
    Ok(())
}

fn main() -> std::process::ExitCode {
    match run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            std::process::ExitCode::FAILURE
        }
    }
}
