use crate::http::{
    json_string, percent_decode, percent_decode_path, reason, response, send_response, APP_ISOLATION_HEADERS,
    PUBLIC_ASSET_HEADERS,
};
use makepad_network::http_server::{
    HttpServerResponse, HttpServerResponseSender,
};
use makepad_network::HttpServerHeaders;
use std::{
    collections::HashMap,
    ffi::{CString, OsString},
    fs::{File, Metadata},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    sync::{mpsc::{self, SyncSender}, Mutex},
    time::{Duration, Instant, UNIX_EPOCH},
};

#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};

#[cfg(target_os = "linux")]
use std::os::unix::fs::MetadataExt;

const REPORT_LIMIT: usize = 8_192;
pub const REPORT_BODY_LIMIT: usize = REPORT_LIMIT;
const REPORTS_PER_MINUTE: u32 = 10;
const REPORT_WINDOW: Duration = Duration::from_secs(60);
const MAX_REPORT_CLIENTS: usize = 4_096;
const LOG_QUEUE_CAPACITY: usize = 256;

#[derive(Clone, Copy)]
struct ReportWindow {
    started: Instant,
    count: u32,
}

/// Fixed-capacity, expiring per-client limiter. Public only so the integration
/// suite can assert its memory bound and deterministic eviction behavior.
pub struct ReportRateLimiter {
    entries: HashMap<IpAddr, ReportWindow>,
    capacity: usize,
}

impl ReportRateLimiter {
    pub fn new(capacity: usize) -> Self {
        Self { entries: HashMap::new(), capacity: capacity.max(1) }
    }

    pub fn allow_at(&mut self, ip: IpAddr, now: Instant) -> bool {
        self.entries.retain(|_, entry| {
            now.checked_duration_since(entry.started)
                .is_some_and(|age| age < REPORT_WINDOW)
        });
        if !self.entries.contains_key(&ip) && self.entries.len() >= self.capacity {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, entry)| entry.started)
                .map(|(ip, _)| *ip)
            {
                self.entries.remove(&oldest);
            }
        }
        let entry = self.entries.entry(ip).or_insert(ReportWindow { started: now, count: 0 });
        if now.checked_duration_since(entry.started).unwrap_or_default() >= REPORT_WINDOW {
            *entry = ReportWindow { started: now, count: 0 };
        }
        if entry.count >= REPORTS_PER_MINUTE {
            false
        } else {
            entry.count += 1;
            true
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

pub struct StaticHandler {
    root: PathBuf,
    root_fd: File,
    reports: Mutex<ReportRateLimiter>,
    log_tx: SyncSender<String>,
}

impl StaticHandler {
    pub fn new(root: &Path) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|error| format!("canonicalize static root {}: {error}", root.display()))?;
        let root_fd = File::open(&root)
            .map_err(|error| format!("open static root {}: {error}", root.display()))?;
        if !root_fd.metadata().is_ok_and(|metadata| metadata.is_dir()) {
            return Err(format!("static root {} is not a directory", root.display()));
        }
        validate_docroot_permissions(&root)?;
        validate_root_identity(&root, &root_fd)?;
        let (log_tx, log_rx) = mpsc::sync_channel::<String>(LOG_QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("web-report-log".into())
            .spawn(move || {
                while let Ok(line) = log_rx.recv() {
                    eprintln!("{line}");
                }
            })
            .map_err(|error| format!("start report logger: {error}"))?;
        Ok(Self {
            root,
            root_fd,
            reports: Mutex::new(ReportRateLimiter::new(MAX_REPORT_CLIENTS)),
            log_tx,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn handle_get(
        &self,
        headers: &HttpServerHeaders,
        sender: &HttpServerResponseSender,
    ) {
        if headers.path == "/$report_error" {
            if matches!(headers.verb.as_str(), "GET" | "HEAD") {
                let raw = headers
                    .search
                    .as_deref()
                    .unwrap_or("")
                    .strip_prefix("data=")
                    .unwrap_or(headers.search.as_deref().unwrap_or(""));
                self.report(headers, raw.as_bytes(), true);
                send_response(sender, no_content("no-store", ""));
            } else if headers.verb == "OPTIONS" {
                send_response(sender, options_response(true));
            } else {
                send_response(sender, method_not_allowed("GET, HEAD, POST, OPTIONS"));
            }
            return;
        }
        if headers.verb == "OPTIONS" {
            send_response(sender, options_response(false));
            return;
        }
        if !matches!(headers.verb.as_str(), "GET" | "HEAD") {
            send_response(sender, method_not_allowed("GET, HEAD, OPTIONS"));
            return;
        }
        self.serve_file(headers, sender);
    }

    pub fn handle_post(
        &self,
        headers: &HttpServerHeaders,
        body: &[u8],
        sender: &HttpServerResponseSender,
    ) -> bool {
        if headers.path != "/$report_error" {
            return false;
        }
        self.report(headers, body, false);
        send_response(sender, no_content("no-store", ""));
        true
    }

    fn serve_file(&self, headers: &HttpServerHeaders, sender: &HttpServerResponseSender) {
        let request_path = match percent_decode_path(&headers.path, 8_192) {
            Ok(path) if path.starts_with('/') && !path.contains('\0') && !path.contains('\\') => path,
            _ => {
                send_response(sender, static_error(400, "bad request"));
                return;
            }
        };
        if request_path.split('/').any(|part| part == "..") {
            send_response(sender, static_error(400, "bad request"));
            return;
        }
        let relative = request_path.trim_start_matches('/');
        let original = match self.open_beneath(relative) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                send_response(sender, static_error(404, "not found"));
                return;
            }
            Err(error) if is_path_refusal(&error) => {
                send_response(sender, static_error(400, "bad request"));
                return;
            }
            Err(_) => {
                send_response(sender, static_error(500, "internal error"));
                return;
            }
        };
        if !original.metadata().is_ok_and(|metadata| metadata.is_file()) {
            send_response(sender, static_error(404, "not found"));
            return;
        }
        let Some(mime) = mime_for_path(Path::new(relative)) else {
            send_response(sender, static_error(404, "not found"));
            return;
        };

        let range_header = headers.header("Range");
        let range = range_header.map(parse_range);
        let range_requested = range_header.is_some();
        let never_brotli = matches!(extension(relative), Some("mkidx" | "mkshard"));
        let brotli_file = if !range_requested && !never_brotli {
            self.open_beneath(&sibling_brotli(relative))
                .ok()
                .filter(|file| file.metadata().is_ok_and(|metadata| metadata.is_file()))
        } else {
            None
        };
        let vary = brotli_file.is_some();
        let use_brotli = vary && accepts_brotli(headers.header("Accept-Encoding"));
        let selected = if use_brotli {
            brotli_file.unwrap()
        } else {
            original
        };
        let metadata = match selected.metadata() {
            Ok(metadata) => metadata,
            Err(_) => {
                send_response(sender, static_error(500, "internal error"));
                return;
            }
        };
        let len = metadata.len();
        let etag = etag(&metadata);
        let cache = cache_policy(relative);
        let is_public = is_public_asset(relative);
        let common = common_file_headers(&etag, vary, use_brotli, is_public);

        if if_none_match(headers.header("If-None-Match"), &etag) {
            let header = format!(
                "HTTP/1.1 304 Not Modified\r\nContent-Type: {mime}\r\n{APP_ISOLATION_HEADERS}\
                 Cache-Control: {cache}\r\n{common}Content-Length: 0\r\nConnection: close\r\n\r\n"
            );
            send_response(sender, HttpServerResponse { header, body: Vec::new() });
            return;
        }

        let (status, offset, body_len, content_range) = match range {
            None => (200, 0, len, None),
            Some(Ok(spec)) => match spec.resolve(len) {
                Some((start, range_len)) => (
                    206,
                    start,
                    range_len,
                    Some(format!("Content-Range: bytes {start}-{}/{len}\r\n", start + range_len - 1)),
                ),
                None => {
                    send_response(sender, range_error(len, cache, &etag, is_public));
                    return;
                }
            },
            Some(Err(())) => {
                send_response(sender, range_error(len, cache, &etag, is_public));
                return;
            }
        };
        let content_range = content_range.unwrap_or_default();
        let header = format!(
            "HTTP/1.1 {status} {}\r\nContent-Type: {mime}\r\n{APP_ISOLATION_HEADERS}\
             Cache-Control: {cache}\r\n{common}{content_range}Content-Length: {body_len}\r\n\
             Connection: close\r\n\r\n",
            reason(status)
        );
        if headers.verb == "HEAD" {
            send_response(sender, HttpServerResponse { header, body: Vec::new() });
            return;
        }
        let _ = sender.send(HttpServerResponse::from_file(header, selected, offset, body_len));
    }

    fn report(&self, headers: &HttpServerHeaders, bytes: &[u8], encoded: bool) {
        let raw = String::from_utf8_lossy(&bytes[..bytes.len().min(REPORT_LIMIT * 3)]);
        let decoded = if encoded {
            percent_decode(&raw, REPORT_LIMIT).unwrap_or_else(|_| "invalid percent encoding".into())
        } else {
            percent_decode(&raw, REPORT_LIMIT).unwrap_or_else(|_| raw.chars().take(REPORT_LIMIT).collect())
        };
        let clean: String = decoded
            .chars()
            .take(REPORT_LIMIT)
            .map(|ch| if ch.is_control() { ' ' } else { ch })
            .collect();
        let ip = client_ip(headers);
        let now = Instant::now();
        let allowed = {
            let mut reports = self.reports.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            reports.allow_at(ip, now)
        };
        if allowed {
            let _ = self.log_tx.try_send(format!(
                "{{\"event\":\"browser_error\",\"peer\":{},\"message\":{}}}",
                json_string(&ip.to_string()),
                json_string(clean.trim())
            ));
        }
    }

    fn open_beneath(&self, relative: &str) -> std::io::Result<File> {
        open_beneath(&self.root_fd, &self.root, relative)
    }
}

#[cfg(target_os = "linux")]
fn open_beneath(root_fd: &File, _root: &Path, relative: &str) -> std::io::Result<File> {
    #[repr(C)]
    struct OpenHow {
        flags: u64,
        mode: u64,
        resolve: u64,
    }
    const RESOLVE_NO_MAGICLINKS: u64 = 0x02;
    const RESOLVE_BENEATH: u64 = 0x08;
    let path = CString::new(relative)
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    let how = OpenHow {
        flags: (libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK) as u64,
        mode: 0,
        resolve: RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS,
    };
    // SAFETY: all pointers remain valid for the syscall and a successful
    // return is a newly owned descriptor.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            root_fd.as_raw_fd(),
            path.as_ptr(),
            &how as *const OpenHow,
            std::mem::size_of::<OpenHow>(),
        )
    } as libc::c_int;
    if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        // SAFETY: `openat2` returned a fresh descriptor owned by this call.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn open_beneath(root_fd: &File, _root: &Path, relative: &str) -> std::io::Result<File> {
    let components: Vec<&str> = relative.split('/').collect();
    if components.is_empty()
        || components
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
    {
        return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
    }
    let mut current = root_fd.try_clone()?;
    for (index, component) in components.iter().enumerate() {
        let component = CString::new(*component)
            .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
        let mut flags = libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK;
        if index + 1 < components.len() {
            flags |= libc::O_DIRECTORY;
        }
        // SAFETY: `current` is open, the C string is valid, and a successful
        // descriptor is immediately wrapped in an owning `File`.
        let fd = unsafe { libc::openat(current.as_raw_fd(), component.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        current = unsafe { File::from_raw_fd(fd) };
    }
    Ok(current)
}

#[cfg(not(unix))]
fn open_beneath(_root_fd: &File, root: &Path, relative: &str) -> std::io::Result<File> {
    let path = root.join(relative).canonicalize()?;
    if !path.starts_with(root) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "path escapes static root",
        ));
    }
    File::open(path)
}

#[cfg(unix)]
fn is_path_refusal(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code) if matches!(code, libc::ELOOP | libc::EXDEV | libc::EINVAL | libc::ENOTDIR)
    )
}

#[cfg(not(unix))]
fn is_path_refusal(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::PermissionDenied
}

#[cfg(target_os = "linux")]
fn validate_docroot_permissions(root: &Path) -> Result<(), String> {
    let euid = unsafe { libc::geteuid() };
    if euid == 0 {
        return Err("refusing to serve a public docroot as root".into());
    }
    let egid = unsafe { libc::getegid() };
    let count = unsafe { libc::getgroups(0, std::ptr::null_mut()) };
    let mut groups = if count > 0 { vec![0; count as usize] } else { Vec::new() };
    if count > 0 && unsafe { libc::getgroups(count, groups.as_mut_ptr()) } < 0 {
        return Err("cannot inspect service-account groups".into());
    }
    groups.push(egid);

    let mut pending = vec![root.to_path_buf()];
    while let Some(path) = pending.pop() {
        let metadata = path
            .symlink_metadata()
            .map_err(|error| format!("inspect docroot {}: {error}", path.display()))?;
        let mode = metadata.mode();
        let writable = if metadata.uid() == euid {
            mode & 0o200 != 0
        } else if groups.contains(&metadata.gid()) {
            mode & 0o020 != 0
        } else {
            mode & 0o002 != 0
        };
        if writable {
            return Err(format!(
                "service account can mutate static docroot entry {}",
                path.display()
            ));
        }
        if metadata.is_dir() {
            for entry in std::fs::read_dir(&path)
                .map_err(|error| format!("inspect docroot {}: {error}", path.display()))?
            {
                pending.push(entry.map_err(|error| error.to_string())?.path());
            }
        }
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn validate_docroot_permissions(_root: &Path) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "linux")]
fn validate_root_identity(root: &Path, root_fd: &File) -> Result<(), String> {
    let path_metadata = root
        .metadata()
        .map_err(|error| format!("recheck static root {}: {error}", root.display()))?;
    let fd_metadata = root_fd
        .metadata()
        .map_err(|error| format!("inspect static root descriptor: {error}"))?;
    if path_metadata.dev() != fd_metadata.dev() || path_metadata.ino() != fd_metadata.ino() {
        return Err("static root changed during startup validation".into());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn validate_root_identity(_root: &Path, _root_fd: &File) -> Result<(), String> {
    Ok(())
}

// Published by Cloudflare at https://www.cloudflare.com/ips-v4/ and
// https://www.cloudflare.com/ips-v6/ (verified 2026-09-02).
const CLOUDFLARE_V4: &[(&str, u8)] = &[
    ("173.245.48.0", 20), ("103.21.244.0", 22), ("103.22.200.0", 22),
    ("103.31.4.0", 22), ("141.101.64.0", 18), ("108.162.192.0", 18),
    ("190.93.240.0", 20), ("188.114.96.0", 20), ("197.234.240.0", 22),
    ("198.41.128.0", 17), ("162.158.0.0", 15), ("104.16.0.0", 13),
    ("104.24.0.0", 14), ("172.64.0.0", 13), ("131.0.72.0", 22),
];
const CLOUDFLARE_V6: &[(&str, u8)] = &[
    ("2400:cb00::", 32), ("2606:4700::", 32), ("2803:f800::", 32),
    ("2405:b500::", 32), ("2405:8100::", 32), ("2a06:98c0::", 29),
    ("2c0f:f248::", 32),
];

fn cloudflare_peer(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let value = u32::from(ip);
            CLOUDFLARE_V4.iter().any(|(base, prefix)| {
                let base = u32::from(base.parse::<Ipv4Addr>().expect("valid Cloudflare IPv4"));
                let mask = u32::MAX.checked_shl(32 - u32::from(*prefix)).unwrap_or(0);
                value & mask == base & mask
            })
        }
        IpAddr::V6(ip) => {
            let value = u128::from(ip);
            CLOUDFLARE_V6.iter().any(|(base, prefix)| {
                let base = u128::from(base.parse::<Ipv6Addr>().expect("valid Cloudflare IPv6"));
                let mask = u128::MAX.checked_shl(128 - u32::from(*prefix)).unwrap_or(0);
                value & mask == base & mask
            })
        }
    }
}

fn client_ip(headers: &HttpServerHeaders) -> IpAddr {
    let peer = headers.addr.ip();
    if !cloudflare_peer(peer) {
        return peer;
    }
    headers
        .header("CF-Connecting-IP")
        .filter(|value| !value.contains(','))
        .and_then(|value| value.parse().ok())
        .unwrap_or(peer)
}

fn sibling_brotli(path: &str) -> String {
    let mut name: OsString = Path::new(path).as_os_str().to_owned();
    name.push(".br");
    name.to_string_lossy().into_owned()
}

fn extension(path: &str) -> Option<&str> {
    Path::new(path).extension()?.to_str()
}

pub fn mime_for_path(path: &Path) -> Option<&'static str> {
    Some(match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "html" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "json" => "application/json; charset=utf-8",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "glb" => "model/gltf-binary",
        "bin" | "data" | "blob" | "mkidx" | "mkshard" | "search" | "searchdb"
        | "graph" | "mbtiles" => "application/octet-stream",
        _ => return None,
    })
}

pub fn cache_policy(path: &str) -> &'static str {
    let lower = path.to_ascii_lowercase();
    if lower.ends_with(".html") || lower.ends_with(".mkidx") {
        "no-cache"
    } else if lower.starts_with("maps/") && lower.ends_with(".mkshard") {
        "public, max-age=31536000, immutable"
    } else if is_hashed_asset(&lower) {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    }
}

fn is_hashed_asset(path: &str) -> bool {
    let Some(ext) = extension(path) else { return false };
    if !matches!(ext, "wasm" | "js" | "mjs" | "ttf" | "otf" | "woff" | "woff2") {
        return false;
    }
    let stem = path.rsplit('/').next().unwrap_or(path).trim_end_matches(ext).trim_end_matches('.');
    stem.split(['-', '_', '.'])
        .any(|part| part.len() >= 8 && part.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

fn is_public_asset(path: &str) -> bool {
    let ext = extension(path).unwrap_or("");
    path.starts_with("maps/")
        || matches!(
            ext,
            "wasm" | "js" | "mjs" | "css" | "ttf" | "otf" | "woff" | "woff2"
                | "png" | "jpg" | "jpeg" | "webp" | "svg" | "ico" | "glb" | "bin"
                | "data" | "blob" | "mkidx" | "mkshard" | "mbtiles"
        )
}

fn common_file_headers(etag: &str, vary: bool, encoded: bool, public: bool) -> String {
    format!(
        "Accept-Ranges: bytes\r\nETag: {etag}\r\n{}{}{}",
        if vary { "Vary: Accept-Encoding\r\n" } else { "" },
        if encoded { "Content-Encoding: br\r\n" } else { "" },
        if public { PUBLIC_ASSET_HEADERS } else { "" }
    )
}

pub fn etag(metadata: &Metadata) -> String {
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .unwrap_or_default();
    format!("\"{:x}-{:x}-{:x}\"", metadata.len(), modified.as_secs(), modified.subsec_nanos())
}

fn if_none_match(header: Option<&str>, etag: &str) -> bool {
    header.is_some_and(|value| {
        value.split(',').any(|candidate| {
            let candidate = candidate.trim();
            candidate == "*" || candidate == etag || candidate.strip_prefix("W/") == Some(etag)
        })
    })
}

fn accepts_brotli(header: Option<&str>) -> bool {
    header.is_some_and(|value| {
        value.split(',').any(|item| {
            let mut parts = item.trim().split(';');
            if !parts.next().is_some_and(|encoding| encoding.trim().eq_ignore_ascii_case("br")) {
                return false;
            }
            let mut quality = 1.0f32;
            let mut saw_quality = false;
            for parameter in parts {
                let parameter = parameter.trim();
                let Some((name, value)) = parameter.split_once('=') else { continue };
                if name.trim().eq_ignore_ascii_case("q") {
                    if saw_quality {
                        return false;
                    }
                    let Some(parsed) = parse_quality(value.trim()) else { return false };
                    saw_quality = true;
                    quality = parsed;
                }
            }
            quality > 0.0
        })
    })
}

fn parse_quality(value: &str) -> Option<f32> {
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    if fraction.len() > 3 || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    match whole {
        "0" => value.parse().ok(),
        "1" if fraction.bytes().all(|byte| byte == b'0') => Some(1.0),
        _ => None,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ByteRange {
    Inclusive(u64, u64),
    From(u64),
    Suffix(u64),
}

impl ByteRange {
    pub fn resolve(self, len: u64) -> Option<(u64, u64)> {
        if len == 0 {
            return None;
        }
        match self {
            ByteRange::Inclusive(start, end) if start <= end && start < len => {
                let end = end.min(len - 1);
                Some((start, end - start + 1))
            }
            ByteRange::From(start) if start < len => Some((start, len - start)),
            ByteRange::Suffix(count) if count > 0 => {
                let count = count.min(len);
                Some((len - count, count))
            }
            _ => None,
        }
    }
}

pub fn parse_range(value: &str) -> Result<ByteRange, ()> {
    let (unit, value) = value.trim().split_once('=').ok_or(())?;
    if !unit.eq_ignore_ascii_case("bytes") || value.contains(',') {
        return Err(());
    }
    let (start, end) = value.trim().split_once('-').ok_or(())?;
    match (start.trim(), end.trim()) {
        ("", "") => Err(()),
        ("", suffix) => suffix.parse().map(ByteRange::Suffix).map_err(|_| ()),
        (start, "") => start.parse().map(ByteRange::From).map_err(|_| ()),
        (start, end) => Ok(ByteRange::Inclusive(
            start.parse().map_err(|_| ())?,
            end.parse().map_err(|_| ())?,
        )),
    }
}

fn range_error(len: u64, cache: &str, etag: &str, public: bool) -> HttpServerResponse {
    response(
        416,
        Some("text/plain; charset=utf-8"),
        cache,
        &format!(
            "Accept-Ranges: bytes\r\nContent-Range: bytes */{len}\r\nETag: {etag}\r\n{}",
            if public { PUBLIC_ASSET_HEADERS } else { "" }
        ),
        b"range not satisfiable".to_vec(),
    )
}

fn static_error(status: u16, message: &str) -> HttpServerResponse {
    response(
        status,
        Some("text/plain; charset=utf-8"),
        "no-store",
        "",
        message.as_bytes().to_vec(),
    )
}

fn method_not_allowed(allow: &str) -> HttpServerResponse {
    response(
        405,
        Some("text/plain; charset=utf-8"),
        "no-store",
        &format!("Allow: {allow}\r\n"),
        b"method not allowed".to_vec(),
    )
}

fn no_content(cache: &str, extra: &str) -> HttpServerResponse {
    response(204, None, cache, extra, Vec::new())
}

fn options_response(report: bool) -> HttpServerResponse {
    let methods = if report { "GET, HEAD, POST, OPTIONS" } else { "GET, HEAD, OPTIONS" };
    no_content(
        "no-store",
        &format!(
            "{PUBLIC_ASSET_HEADERS}Access-Control-Allow-Methods: {methods}\r\n\
             Access-Control-Allow-Headers: Range, If-None-Match, Accept-Encoding, Content-Type\r\n\
             Access-Control-Max-Age: 86400\r\nAllow: {methods}\r\n"
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::SystemTime};

    #[test]
    fn parses_and_resolves_ranges() {
        assert_eq!(parse_range("bytes=2-5"), Ok(ByteRange::Inclusive(2, 5)));
        assert_eq!(parse_range("BYTES=7-"), Ok(ByteRange::From(7)));
        assert_eq!(parse_range("bytes=-4"), Ok(ByteRange::Suffix(4)));
        assert_eq!(ByteRange::Inclusive(2, 99).resolve(10), Some((2, 8)));
        assert_eq!(ByteRange::Suffix(40).resolve(10), Some((0, 10)));
        assert!(parse_range("bytes=1-2,4-5").is_err());
        assert_eq!(ByteRange::From(10).resolve(10), None);
    }

    #[test]
    fn brotli_requires_a_valid_positive_quality() {
        assert!(accepts_brotli(Some("gzip, br")));
        assert!(accepts_brotli(Some("br;q=0.5")));
        for invalid in ["br;q=0", "br;q=garbage", "br;q=0.0001", "br;q=2", "br;q=1;q=0.5"] {
            assert!(!accepts_brotli(Some(invalid)), "accepted {invalid}");
        }
    }

    #[test]
    fn cache_policy_distinguishes_mutable_and_content_addressed_files() {
        assert_eq!(cache_policy("index.html"), "no-cache");
        assert_eq!(cache_policy("maps/root.mkidx"), "no-cache");
        assert!(cache_policy("maps/tiles-003.mkshard").contains("immutable"));
        assert!(cache_policy("app-a1b2c3d4.wasm").contains("immutable"));
        assert_eq!(cache_policy("app.wasm"), "no-cache");
    }

    #[test]
    fn mime_table_covers_navigation_and_map_data() {
        for ext in ["mkidx", "mkshard", "search", "searchdb", "graph", "mbtiles"] {
            let path = PathBuf::from(format!("x.{ext}"));
            assert_eq!(mime_for_path(&path), Some("application/octet-stream"));
        }
        assert_eq!(mime_for_path(Path::new("x.wasm")), Some("application/wasm"));
        assert_eq!(mime_for_path(Path::new("secret.key")), None);
    }

    #[test]
    fn etag_uses_length_and_mtime() {
        let dir = std::env::temp_dir().join(format!(
            "makepad-web-etag-{}-{:?}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("asset.bin");
        fs::write(&path, b"abcd").unwrap();
        let first = etag(&path.metadata().unwrap());
        fs::write(&path, b"abcdefgh").unwrap();
        let second = etag(&path.metadata().unwrap());
        assert_ne!(first, second);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
