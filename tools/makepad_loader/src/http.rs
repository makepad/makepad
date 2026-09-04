use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use makepad_network::blocking_http::{self, Limits, Request};

use crate::progress::{self, Progress};
use crate::sha256;

pub fn big_limits() -> Limits {
    Limits {
        max_head_bytes: 256 * 1024,
        max_header_count: 128,
        max_header_line_bytes: 16 * 1024,
        max_trailer_count: 32,
        max_trailer_bytes: 8 * 1024,
        max_body_bytes: 2 * 1024 * 1024 * 1024,
        max_chunk_line_bytes: 4096,
        total_timeout: Duration::from_secs(3 * 3600),
    }
}

pub fn fetch(url: &str) -> Result<blocking_http::Response, String> {
    fetch_method("GET", url, &[], &[])
}

pub fn fetch_method(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
) -> Result<blocking_http::Response, String> {
    fetch_method_progress(method, url, headers, body, None)
}

pub fn fetch_method_progress(
    method: &str,
    url: &str,
    headers: &[(String, String)],
    body: &[u8],
    file_name: Option<&str>,
) -> Result<blocking_http::Response, String> {
    let mut current = url.to_string();
    for _ in 0..8 {
        let mut req = match method {
            "POST" => Request::post(&current),
            _ => Request::get(&current),
        }
        .limits(big_limits())
        .body(body.to_vec());
        for (name, value) in headers {
            req = req
                .header(name, value)
                .map_err(|e| format!("{e} ({name})"))?;
        }
        if let Some(name) = file_name {
            let name = name.to_string();
            let last = Arc::new(AtomicU64::new(0));
            req = req.on_body_progress(move |loaded, total| {
                let prev = last.load(Ordering::Relaxed);
                let step = 256 * 1024;
                let done = total.is_some_and(|t| t > 0 && loaded >= t);
                if !done && loaded < prev.saturating_add(step) {
                    return;
                }
                last.store(loaded, Ordering::Relaxed);
                let frac = match total {
                    Some(t) if t > 0 => (loaded as f64 / t as f64) as f32,
                    _ => 0.0,
                };
                progress::emit(Progress {
                    stage: "Download".into(),
                    detail: format!(
                        "{name}  {:.1} / {:.1} MB",
                        loaded as f64 / 1_048_576.0,
                        total.unwrap_or(0) as f64 / 1_048_576.0
                    ),
                    loaded,
                    total: total.unwrap_or(0),
                    frac,
                });
            });
        }
        let resp = blocking_http::request_no_redirect(req)
            .map_err(|e| format!("http {e} for {current}"))?;
        if (300..400).contains(&resp.status) {
            let loc = resp
                .header("location")
                .ok_or_else(|| format!("redirect without location from {current}"))?;
            current = resolve_url(&current, loc)?;
            continue;
        }
        if resp.status != 200 {
            return Err(format!("http {} for {current}", resp.status));
        }
        return Ok(resp);
    }
    Err(format!("too many redirects from {url}"))
}

pub fn cached_file(
    cache: &Path,
    url: &str,
    file_name: &str,
    sha256_hex: Option<&str>,
) -> Result<PathBuf, String> {
    fs::create_dir_all(cache).map_err(|e| e.to_string())?;
    let dest = cache.join(safe_name(file_name));
    let part = sidecar(&dest, ".part");
    let ok = sidecar(&dest, ".ok");
    let _ = fs::remove_file(&part);

    if dest.is_file() && file_is_complete(&dest, &ok, sha256_hex)? {
        println!(
            "  cache hit {}",
            dest.file_name().unwrap().to_string_lossy()
        );
        return Ok(dest);
    }
    if dest.is_file() {
        println!("  incomplete or corrupt {file_name}, redownloading");
        let _ = fs::remove_file(&dest);
        let _ = fs::remove_file(&ok);
    }

    println!("  download {file_name}");
    progress::stage("Download", file_name, 0.0);
    let resp = fetch_method_progress("GET", url, &[], &[], Some(file_name))?;
    if resp.body.is_empty() {
        return Err(format!("empty download {file_name}"));
    }
    if let Some(expect) = sha256_hex {
        let got = sha256::sha256_hex(&resp.body);
        if !got.eq_ignore_ascii_case(expect) {
            return Err(format!(
                "sha256 mismatch for {file_name}: got {got} want {expect}"
            ));
        }
    }
    if let Some(len) = content_length(&resp) {
        if resp.body.len() as u64 != len {
            return Err(format!(
                "truncated download {file_name}: {} != {len}",
                resp.body.len()
            ));
        }
    }

    write_atomic(&dest, &part, &ok, &resp.body, sha256_hex)?;
    println!(
        "  wrote {} ({:.1} MB)",
        dest.file_name().unwrap().to_string_lossy(),
        dest.metadata().map(|m| m.len()).unwrap_or(0) as f64 / 1_048_576.0
    );
    Ok(dest)
}

pub fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    Ok(fetch(url)?.body)
}

fn content_length(resp: &blocking_http::Response) -> Option<u64> {
    resp.header("content-length")
        .and_then(|s| s.trim().parse().ok())
}

fn sidecar(dest: &Path, extra: &str) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(extra);
    PathBuf::from(s)
}

fn file_is_complete(
    dest: &Path,
    ok: &Path,
    sha256_hex: Option<&str>,
) -> Result<bool, String> {
    let meta = dest.metadata().map_err(|e| e.to_string())?;
    if meta.len() == 0 {
        return Ok(false);
    }
    let stamp = match fs::read_to_string(ok) {
        Ok(s) => s,
        Err(_) => return Ok(false),
    };
    let mut size = None;
    let mut sha = None;
    for line in stamp.lines() {
        if let Some(v) = line.strip_prefix("size=") {
            size = v.trim().parse::<u64>().ok();
        }
        if let Some(v) = line.strip_prefix("sha256=") {
            sha = Some(v.trim().to_string());
        }
    }
    if size != Some(meta.len()) {
        return Ok(false);
    }
    if let Some(expect) = sha256_hex {
        if sha
            .as_deref()
            .map(|s| !s.eq_ignore_ascii_case(expect))
            .unwrap_or(true)
        {
            return Ok(false);
        }
        return Ok(true);
    }
    if let Some(recorded) = sha {
        let bytes = fs::read(dest).map_err(|e| e.to_string())?;
        let got = sha256::sha256_hex(&bytes);
        return Ok(got.eq_ignore_ascii_case(&recorded));
    }
    Ok(false)
}

fn write_atomic(
    dest: &Path,
    part: &Path,
    ok: &Path,
    body: &[u8],
    sha256_hex: Option<&str>,
) -> Result<(), String> {
    let _ = fs::remove_file(part);
    {
        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(part)
            .map_err(|e| e.to_string())?;
        f.write_all(body).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
    }
    let sha = sha256_hex
        .map(|s| s.to_string())
        .unwrap_or_else(|| sha256::sha256_hex(body));
    let stamp = format!("size={}\nsha256={sha}\n", body.len());
    fs::write(ok, stamp).map_err(|e| e.to_string())?;
    if let Err(e) = fs::rename(part, dest) {
        let _ = fs::remove_file(part);
        return Err(e.to_string());
    }
    Ok(())
}

fn safe_name(name: &str) -> String {
    name.replace(['/', '\\', ':'], "_")
}

fn resolve_url(current: &str, location: &str) -> Result<String, String> {
    let loc = location.trim();
    if loc.starts_with("https://") || loc.starts_with("http://") {
        return Ok(loc.to_string());
    }
    if loc.starts_with("//") {
        let scheme = if current.starts_with("https://") {
            "https:"
        } else {
            "http:"
        };
        return Ok(format!("{scheme}{loc}"));
    }
    let slash = current
        .find("://")
        .and_then(|i| current[i + 3..].find('/').map(|j| i + 3 + j))
        .unwrap_or(current.len());
    let origin = &current[..slash.min(current.len())];
    if loc.starts_with('/') {
        Ok(format!("{origin}{loc}"))
    } else {
        let base = current.rsplit_once('/').map(|(a, _)| a).unwrap_or(origin);
        Ok(format!("{base}/{loc}"))
    }
}

#[allow(dead_code)]
fn _io(e: io::Error) -> String {
    e.to_string()
}
