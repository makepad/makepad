use std::fs;
use std::path::Path;

use crate::extract;
use crate::http;

const CHANNEL: &str = "https://static.rust-lang.org/dist/channel-rust-stable.toml";
const TRIPLE: &str = "x86_64-pc-windows-msvc";

pub fn install(cache: &Path, dest: &Path) -> Result<(), String> {
    if dest.join("bin").join("cargo.exe").is_file() && dest.join("bin").join("rustc.exe").is_file()
    {
        println!("rust: already extracted at {}", dest.display());
        return Ok(());
    }
    println!("rust: fetching stable channel");
    let toml = String::from_utf8(http::fetch_bytes(CHANNEL)?)
        .map_err(|_| "rust channel is not utf-8")?;
    let rustc = pkg_target(&toml, "rustc", TRIPLE)?;
    let std = pkg_target(&toml, "rust-std", TRIPLE)?;
    let cargo = pkg_target(&toml, "cargo", TRIPLE)?;
    println!("rust: {} / {} / {}", rustc.0, std.0, cargo.0);

    let rustc_path = http::cached_file(cache, &rustc.1, &file_name(&rustc.1), Some(&rustc.2))?;
    let std_path = http::cached_file(cache, &std.1, &file_name(&std.1), Some(&std.2))?;
    let cargo_path = http::cached_file(cache, &cargo.1, &file_name(&cargo.1), Some(&cargo.2))?;

    let tmp = dest.parent().unwrap_or(dest).join("rust-unpack");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;

    println!("rust: unpack rustc");
    extract::extract_tar_gz(&rustc_path, &tmp.join("rustc"))?;
    println!("rust: unpack rust-std");
    extract::extract_tar_gz(&std_path, &tmp.join("std"))?;
    println!("rust: unpack cargo");
    extract::extract_tar_gz(&cargo_path, &tmp.join("cargo"))?;

    fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    merge_component(&tmp.join("rustc"), dest, "rustc")?;
    merge_component(&tmp.join("cargo"), dest, "cargo")?;
    merge_std(&tmp.join("std"), dest)?;
    let _ = fs::remove_dir_all(&tmp);
    if !dest.join("bin").join("rustc.exe").is_file() {
        return Err("rustc.exe missing after extract".into());
    }
    println!("rust: ready at {}", dest.display());
    Ok(())
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
