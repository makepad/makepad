use std::path::Path;

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
    // Existing files are never replaced: they may hold someone's work.
    if dest.exists() {
        return Err(format!("{} exists but is not a complete checkout; move it away to clone again", dest.display()));
    }
    let parent = dest.parent().ok_or("Clone destination has no parent folder")?;
    let name = dest.file_name().ok_or("Clone destination has no name")?.to_string_lossy();
    // Cloned beside the destination, then renamed into place; a leftover
    // from an interrupted clone starts over.
    let tmp = parent.join(format!(".{name}.clone-tmp"));
    crate::remove_inside(parent, &tmp)?;
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = clone_into(url, &tmp, branch).and_then(|()| std::fs::rename(&tmp, dest).map_err(|e| e.to_string()));
    if result.is_err() {
        let _ = crate::remove_inside(parent, &tmp);
    }
    result
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
