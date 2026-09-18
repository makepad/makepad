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
        let display_url = current.split('?').next().unwrap_or(&current).to_string();
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
                    detail: name.clone(),
                    loaded,
                    total: total.unwrap_or(0),
                    frac,
                    unit: progress::Unit::Bytes,
                    package: progress::Package::default(),
                });
            });
        }
        let resp = blocking_http::request_no_redirect(req)
            .map_err(|e| {
                if current.contains('?') { format!("HTTP request failed for {display_url}") }
                else { format!("http {e} for {display_url}") }
            })?;
        if (300..400).contains(&resp.status) {
            let loc = resp
                .header("location")
                .ok_or_else(|| format!("redirect without location from {display_url}"))?;
            if current.contains('?') || headers.iter().any(|(name, _)| {
                name.eq_ignore_ascii_case("authorization")
                    || name.eq_ignore_ascii_case("x-makepad-email")
            }) {
                return Err("Authenticated source downloads must not redirect".into());
            }
            let next = resolve_url(&current, loc)?;
            if current.starts_with("https://") && !next.starts_with("https://") {
                return Err("Refusing HTTPS download downgrade".into());
            }
            current = next;
            continue;
        }
        if resp.status != 200 {
            let detail = if resp.body.len() <= 4096 {
                makepad_strict_json::parse(&resp.body).ok().and_then(|v| {
                    v.get("error")
                        .and_then(|e| e.as_str())
                        .filter(|s| s.len() < 256)
                        .map(str::to_owned)
                })
            } else {
                None
            };
            return Err(match detail {
                Some(detail) => format!("{detail} (HTTP {})", resp.status),
                None => format!("http {} for {display_url}", resp.status),
            });
        }
        return Ok(resp);
    }
    Err(format!("too many redirects from {}", url.split('?').next().unwrap_or(url)))
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

    progress::stage("Verify cache", file_name, 0.0);
    if dest.is_file() && file_is_complete(&dest, &ok, sha256_hex)? {
        crate::setup_note!(
            "  cache hit {}",
            dest.file_name().unwrap().to_string_lossy()
        );
        return Ok(dest);
    }
    if dest.is_file() {
        crate::setup_note!("  incomplete or corrupt {file_name}, redownloading");
        let _ = fs::remove_file(&dest);
        let _ = fs::remove_file(&ok);
    }

    crate::setup_note!("  download {file_name}");
    progress::stage("Download", file_name, 0.0);
    let resp = fetch_method_progress("GET", url, &[], &[], Some(file_name))?;
    if resp.body.is_empty() {
        return Err(format!("empty download {file_name}"));
    }
    if let Some(expect) = sha256_hex {
        progress::stage("Verify SHA-256", file_name, 0.0);
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

    progress::stage("Save download", file_name, 0.0);
    write_atomic(&dest, &part, &ok, &resp.body, sha256_hex)?;
    crate::setup_note!(
        "  wrote {} ({:.1} MB)",
        dest.file_name().unwrap().to_string_lossy(),
        dest.metadata().map(|m| m.len()).unwrap_or(0) as f64 / 1_048_576.0
    );
    Ok(dest)
}

pub fn fetch_bytes(url: &str) -> Result<Vec<u8>, String> {
    Ok(fetch(url)?.body)
}

/// Bounded, credential-free transfer diagnostics using the installer's HTTP
/// backend. Collect the body just as setup does, but never write or install it.
pub fn download_probe() -> Result<(), String> {
    let mut args = std::env::args().skip(2);
    let url = args.next().ok_or("Usage: download-probe HTTPS_URL [--tui] [--range START-END]")?;
    if !url.starts_with("https://") || url.contains(['?', '#', '@']) {
        return Err("Use a public HTTPS URL without credentials or query parameters".into());
    }
    let mut tui = false;
    let mut range = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--tui" => tui = true,
            "--range" => range = Some(args.next().ok_or("Missing byte range")?),
            _ => return Err(format!("Unknown download-probe option: {arg}")),
        }
    }
    let received = Arc::new(AtomicU64::new(0));
    let callback_ns = Arc::new(AtomicU64::new(0));
    let callbacks = Arc::new(AtomicU64::new(0));
    let bytes = received.clone();
    let timing = callback_ns.clone();
    let count = callbacks.clone();
    let last = AtomicU64::new(0);
    let mut limits = big_limits();
    limits.total_timeout = Duration::from_secs(20);
    let mut request = Request::get(&url).limits(limits).on_body_progress(move |loaded, total| {
        let start = std::time::Instant::now();
        bytes.store(loaded, Ordering::Relaxed);
        count.fetch_add(1, Ordering::Relaxed);
        if loaded >= last.load(Ordering::Relaxed).saturating_add(256 * 1024) || total == Some(loaded) {
            last.store(loaded, Ordering::Relaxed);
            progress::measured("Download", "Transfer probe", loaded, total.unwrap_or(0), progress::Unit::Bytes);
        }
        timing.fetch_add(start.elapsed().as_nanos().min(u64::MAX as u128) as u64, Ordering::Relaxed);
    });
    if let Some(range) = range { request = request.header("Range", &format!("bytes={range}")).map_err(|e| e.to_string())?; }
    let start = std::time::Instant::now();
    let result = if tui { crate::tui::with_progress(|| blocking_http::request_no_redirect(request)) }
        else { blocking_http::request_no_redirect(request) };
    let elapsed = start.elapsed();
    let bytes = received.load(Ordering::Relaxed);
    let report = format!("download-probe bytes={bytes} seconds={:.3} MiB/s={:.3} callback_ms={:.3} callbacks={} status={}",
        elapsed.as_secs_f64(), bytes as f64 / 1048576.0 / elapsed.as_secs_f64().max(0.000001),
        callback_ns.load(Ordering::Relaxed) as f64 / 1_000_000., callbacks.load(Ordering::Relaxed),
        result.as_ref().map(|r| r.status.to_string()).unwrap_or_else(|e| e.to_string()));
    println!("{report}");
    if let Some(path) = std::env::var_os("MAKEPAD_DOWNLOAD_PROBE_REPORT") {
        fs::write(path, &report).map_err(|e| e.to_string())?;
    }
    // A timed-out sample is still useful; other failures remain failures.
    match result { Ok(_) | Err(blocking_http::Error::Timeout) => Ok(()), Err(e) => Err(e.to_string()) }
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

fn file_is_complete(dest: &Path, ok: &Path, sha256_hex: Option<&str>) -> Result<bool, String> {
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
        let bytes = fs::read(dest).map_err(|e| e.to_string())?;
        return Ok(sha256::sha256_hex(&bytes).eq_ignore_ascii_case(expect));
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
