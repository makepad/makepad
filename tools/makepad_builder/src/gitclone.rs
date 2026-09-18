use std::path::{Path, PathBuf};
use std::process::Command;

use makepad_git::http_sync::{
    apply_pack_and_checkout, build_info_refs_request, build_upload_pack_request,
    extract_pack_from_response, parse_info_refs_response, GitHttpMethod, GitHttpRequest,
    GitHttpResponse, NoopHttpSyncHooks,
};

use crate::http;

pub fn clone_depth1(url: &str, dest: &Path, branch: Option<&str>) -> Result<(), String> {
    if dest.join(".git").is_dir() && dest.join("Cargo.toml").is_file() {
        println!("  git: already cloned at {}", dest.display());
        return Ok(());
    }
    let parent = dest.parent().unwrap_or(dest);
    let tmp = parent.join(format!(
        ".{}.clone-tmp",
        dest.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "src".into())
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = clone_into(url, &tmp, branch);
    match result {
        Ok(()) => {
            if dest.exists() {
                let _ = std::fs::remove_dir_all(dest);
            }
            std::fs::rename(&tmp, dest).map_err(|e| {
                let _ = std::fs::remove_dir_all(&tmp);
                e.to_string()
            })?;
            Ok(())
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Err(e)
        }
    }
}

fn clone_into(url: &str, dest: &Path, branch: Option<&str>) -> Result<(), String> {
    println!("  git ls-refs {url}");
    let info = build_info_refs_request(url, None);
    let info_resp = do_git(&info)?;
    let head = parse_info_refs_response(&info_resp, branch).map_err(|e| e.to_string())?;
    println!(
        "  HEAD {} {}",
        head.oid.to_hex(),
        head.ref_name.as_deref().unwrap_or("HEAD")
    );
    let pack_req = build_upload_pack_request(
        url,
        head.oid,
        &head.capabilities,
        &[],
        Some(1),
    )
    .map_err(|e| e.to_string())?;
    println!("  git upload-pack depth=1");
    let pack_resp = do_git(&pack_req)?;
    let pack = extract_pack_from_response(&pack_resp)
        .map_err(|e| e.to_string())?
        .ok_or("empty pack from github")?;
    println!("  pack {:.1} MB", pack.len() as f64 / 1_048_576.0);
    crate::progress::stage(
        "Git",
        &format!("pack {:.1} MB", pack.len() as f64 / 1_048_576.0),
        0.9,
    );
    std::fs::create_dir_all(dest).map_err(|e| e.to_string())?;
    let report = apply_pack_and_checkout(
        dest,
        url,
        head.oid,
        head.ref_name.as_deref(),
        &pack,
        &mut NoopHttpSyncHooks,
    )
    .map_err(|e| e.to_string())?;
    println!(
        "  checkout {} files, {:.1} MB",
        report.checked_out_files,
        report.checked_out_bytes as f64 / 1_048_576.0
    );
    Ok(())
}

pub fn pull_branch(url: &str, dest: &Path, branch: &str) -> Result<String, String> {
    if !dest.is_dir() {
        return Err(format!("no checkout at {}", dest.display()));
    }
    if let Some(git) = find_git_exe() {
        return pull_git_exe(&git, dest, branch);
    }
    crate::progress::stage("Git", &format!("fetch {branch} (depth 1)"), 0.2);
    let parent = dest.parent().unwrap_or(dest);
    let tmp = parent.join(format!(
        ".{}.pull-tmp",
        dest.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "src".into())
    ));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    match clone_into(url, &tmp, Some(branch)) {
        Ok(()) => {
            let backup = parent.join(format!(
                ".{}.pull-old",
                dest.file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "src".into())
            ));
            let _ = std::fs::remove_dir_all(&backup);
            std::fs::rename(dest, &backup).map_err(|e| {
                let _ = std::fs::remove_dir_all(&tmp);
                e.to_string()
            })?;
            if let Err(e) = std::fs::rename(&tmp, dest) {
                let _ = std::fs::rename(&backup, dest);
                let _ = std::fs::remove_dir_all(&tmp);
                return Err(e.to_string());
            }
            let _ = std::fs::remove_dir_all(&backup);
            Ok(format!("checked out {branch}"))
        }
        Err(e) => {
            let _ = std::fs::remove_dir_all(&tmp);
            Err(e)
        }
    }
}

fn find_git_exe() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = if cfg!(windows) {
                dir.join("git.exe")
            } else {
                dir.join("git")
            };
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    if cfg!(windows) {
        let pf = std::env::var_os("ProgramFiles").map(PathBuf::from)?;
        let cand = pf.join("Git").join("cmd").join("git.exe");
        if cand.is_file() {
            return Some(cand);
        }
    } else {
        for p in ["/usr/bin/git", "/opt/homebrew/bin/git", "/usr/local/bin/git"] {
            let cand = PathBuf::from(p);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

fn pull_git_exe(git: &Path, dest: &Path, branch: &str) -> Result<String, String> {
    let run = |args: &[&str]| -> Result<String, String> {
        crate::progress::stage("Git", &args.join(" "), 0.5);
        let out = Command::new(git)
            .current_dir(dest)
            .args(args)
            .output()
            .map_err(|e| format!("git {}: {e}", args.join(" ")))?;
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stdout = String::from_utf8_lossy(&out.stdout);
        if !out.status.success() {
            return Err(format!(
                "git {} failed: {}",
                args.join(" "),
                stderr.trim().if_empty(stdout.trim())
            ));
        }
        Ok(stdout.trim().to_string())
    };
    run(&["fetch", "origin", branch])?;
    run(&["checkout", branch])?;
    run(&["pull", "--ff-only", "origin", branch])?;
    let rev = run(&["rev-parse", "--short", "HEAD"])?;
    Ok(format!("{branch} at {rev}"))
}

trait IfEmpty {
    fn if_empty(self, other: Self) -> Self;
}

impl IfEmpty for &str {
    fn if_empty(self, other: Self) -> Self {
        if self.is_empty() { other } else { self }
    }
}

fn do_git(req: &GitHttpRequest) -> Result<GitHttpResponse, String> {
    let method = match req.method {
        GitHttpMethod::Get => "GET",
        GitHttpMethod::Post => "POST",
    };
    let resp = http::fetch_method(method, &req.url, &req.headers, &req.body)?;
    Ok(GitHttpResponse {
        status_code: resp.status,
        headers: resp.headers.clone(),
        body: resp.body,
    })
}
