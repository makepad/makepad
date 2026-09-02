use crate::http::{
    json_string, percent_decode, percent_decode_path, reason, response, send_response, APP_ISOLATION_HEADERS,
    PUBLIC_ASSET_HEADERS,
};
use makepad_network::http_server::{
    normalize_client_ip, HttpServerPendingBody, HttpServerResponse, HttpServerResponseSender,
};
use makepad_network::HttpServerHeaders;
use std::{
    collections::{HashMap, VecDeque},
    ffi::{CString, OsString},
    fs::{File, Metadata},
    fmt::Write as _,
    io::{Read, Seek},
    net::{IpAddr, Ipv4Addr, Ipv6Addr},
    path::{Path, PathBuf},
    sync::{mpsc::{self, SyncSender, TrySendError}, Mutex},
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
const GLOBAL_REPORT_BURST: f64 = 256.0;
const GLOBAL_REPORTS_PER_SECOND: f64 = 2.0;

#[derive(Clone, Copy)]
struct ReportWindow {
    started: Instant,
    count: u32,
}

/// Fixed-capacity, expiring per-client limiter. Public only so the integration
/// suite can assert its memory bound and refusal to evict active clients.
pub struct ReportRateLimiter {
    entries: HashMap<IpAddr, ReportWindow>,
    expirations: VecDeque<(Instant, IpAddr)>,
    capacity: usize,
    global_tokens: f64,
    global_updated: Instant,
}

impl ReportRateLimiter {
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: HashMap::new(),
            expirations: VecDeque::new(),
            capacity: capacity.max(1),
            global_tokens: GLOBAL_REPORT_BURST,
            global_updated: Instant::now(),
        }
    }

    pub fn allow_at(&mut self, ip: IpAddr, now: Instant) -> bool {
        let ip = normalize_client_ip(ip);
        while self
            .expirations
            .front()
            .is_some_and(|(expires, _)| *expires <= now)
        {
            let (expires, expired_ip) = self.expirations.pop_front().expect("front existed");
            if self
                .entries
                .get(&expired_ip)
                .is_some_and(|entry| entry.started + REPORT_WINDOW <= expires)
            {
                self.entries.remove(&expired_ip);
            }
        }
        if self.entries.get(&ip).is_some_and(|entry| entry.count >= REPORTS_PER_MINUTE) {
            return false;
        }
        if !self.entries.contains_key(&ip) && self.entries.len() >= self.capacity {
            return false;
        }
        if let Some(elapsed) = now.checked_duration_since(self.global_updated) {
            self.global_tokens = (self.global_tokens
                + elapsed.as_secs_f64() * GLOBAL_REPORTS_PER_SECOND)
                .min(GLOBAL_REPORT_BURST);
            self.global_updated = now;
        }
        if self.global_tokens < 1.0 {
            return false;
        }
        self.global_tokens -= 1.0;
        let entry = self.entries.entry(ip).or_insert_with(|| {
            self.expirations.push_back((now + REPORT_WINDOW, ip));
            ReportWindow { started: now, count: 0 }
        });
        entry.count += 1;
        true
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

enum ReportSource {
    Ready(Vec<u8>),
    Pending(HttpServerPendingBody),
}

struct ReportLogJob {
    ip: IpAddr,
    encoded: bool,
    source: ReportSource,
    response: Option<HttpServerResponseSender>,
}

pub struct StaticHandler {
    root: PathBuf,
    root_fd: File,
    reports: Mutex<ReportRateLimiter>,
    log_tx: SyncSender<ReportLogJob>,
}

impl StaticHandler {
    pub fn new(root: &Path) -> Result<Self, String> {
        ensure_static_platform()?;
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
        let (log_tx, log_rx) = mpsc::sync_channel::<ReportLogJob>(LOG_QUEUE_CAPACITY);
        std::thread::Builder::new()
            .name("web-report-log".into())
            .spawn(move || {
                while let Ok(job) = log_rx.recv() {
                    let bytes = match job.source {
                        ReportSource::Ready(bytes) => bytes,
                        ReportSource::Pending(body) => match body.receive() {
                            Ok(body) => body,
                            Err(()) => continue,
                        },
                    };
                    log_report(job.ip, &bytes, job.encoded);
                    if let Some(response) = job.response {
                        send_response(&response, no_content("no-store", ""));
                    }
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

    pub fn handle_post_pending(
        &self,
        headers: &HttpServerHeaders,
        body: HttpServerPendingBody,
        sender: &HttpServerResponseSender,
    ) -> Result<(), HttpServerPendingBody> {
        if headers.path != "/$report_error" {
            return Err(body);
        }
        let Some(ip) = self.admit_report(headers) else {
            body.reject(no_content("no-store", ""));
            return Ok(());
        };
        let job = ReportLogJob {
            ip,
            encoded: false,
            source: ReportSource::Pending(body),
            response: Some(sender.clone()),
        };
        match self.log_tx.try_send(job) {
            Ok(()) => {}
            Err(TrySendError::Full(job) | TrySendError::Disconnected(job)) => {
                if let ReportSource::Pending(body) = job.source {
                    body.reject(no_content("no-store", ""));
                }
            }
        }
        Ok(())
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
        let mut original = match self.open_beneath(relative) {
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
        let original_metadata = match original.metadata() {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => {
                send_response(sender, static_error(404, "not found"));
                return;
            }
            Err(_) => {
                send_response(sender, static_error(500, "internal error"));
                return;
            }
        };
        if !original_metadata.is_file() {
            send_response(sender, static_error(404, "not found"));
            return;
        }
        let Some(mime) = mime_for_path(Path::new(relative)) else {
            send_response(sender, static_error(404, "not found"));
            return;
        };

        let range_header = headers.header("Range");
        let identity_etag = match etag(&mut original, false) {
            Ok(etag) => etag,
            Err(_) => {
                send_response(sender, static_error(500, "internal error"));
                return;
            }
        };
        let identity_modified = modified_seconds(&original_metadata);
        let range_honored = range_header.is_some()
            && headers
                .header("If-Range")
                .is_none_or(|value| if_range_matches(value, &identity_etag, identity_modified));
        let range = range_honored.then(|| parse_range(range_header.expect("range was present")));
        let never_brotli = matches!(extension(relative), Some("mkidx" | "mkshard"));
        let brotli_file = if !range_honored && !never_brotli {
            self.open_beneath(&sibling_brotli(relative))
                .ok()
                .filter(|file| file.metadata().is_ok_and(|metadata| metadata.is_file()))
        } else {
            None
        };
        let use_brotli = brotli_file.is_some() && accepts_brotli(headers.header("Accept-Encoding"));
        let mut selected = if use_brotli {
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
        let etag = if use_brotli {
            match etag(&mut selected, true) {
                Ok(etag) => etag,
                Err(_) => {
                    send_response(sender, static_error(500, "internal error"));
                    return;
                }
            }
        } else {
            identity_etag.clone()
        };
        let modified = modified_seconds(&metadata);
        let last_modified = http_date(modified);
        let cache = cache_policy(relative);
        let is_public = is_public_asset(relative);
        let common = common_file_headers(&etag, &last_modified, use_brotli, is_public);

        let not_modified = match headers.header("If-None-Match") {
            Some(value) => if_none_match(Some(value), &etag),
            None => headers
                .header("If-Modified-Since")
                .and_then(parse_http_date)
                .is_some_and(|since| modified <= since),
        };
        if not_modified {
            let header = format!(
                "HTTP/1.1 304 Not Modified\r\nContent-Type: {mime}\r\n{APP_ISOLATION_HEADERS}\
                 Cache-Control: {cache}\r\n{common}Content-Length: 0\r\nConnection: close\r\n\r\n"
            );
            send_response(sender, HttpServerResponse::new(header, Vec::new()));
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
                    send_response(sender, range_error(len, cache, &etag, &last_modified, is_public));
                    return;
                }
            },
            Some(Err(())) => {
                send_response(sender, range_error(len, cache, &etag, &last_modified, is_public));
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
            send_response(sender, HttpServerResponse::new(header, Vec::new()));
            return;
        }
        send_response(sender, HttpServerResponse::from_file(header, selected, offset, body_len));
    }

    fn report(&self, headers: &HttpServerHeaders, bytes: &[u8], encoded: bool) {
        let Some(ip) = self.admit_report(headers) else { return };
        let source = ReportSource::Ready(bytes[..bytes.len().min(REPORT_LIMIT * 3)].to_vec());
        let _ = self.log_tx.try_send(ReportLogJob { ip, encoded, source, response: None });
    }

    fn admit_report(&self, headers: &HttpServerHeaders) -> Option<IpAddr> {
        let ip = client_ip(headers);
        let now = Instant::now();
        let allowed = {
            let mut reports = self.reports.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            reports.allow_at(ip, now)
        };
        allowed.then_some(ip)
    }

    fn open_beneath(&self, relative: &str) -> std::io::Result<File> {
        validate_relative_components(relative)?;
        open_beneath(&self.root_fd, &self.root, relative)
    }
}

fn validate_relative_components(relative: &str) -> std::io::Result<()> {
    if relative.is_empty()
        || relative
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(std::io::Error::from_raw_os_error(libc::EINVAL));
    }
    Ok(())
}

fn log_report(ip: IpAddr, bytes: &[u8], encoded: bool) {
    let raw = String::from_utf8_lossy(&bytes[..bytes.len().min(REPORT_LIMIT * 3)]);
    let decoded = if encoded {
        percent_decode(&raw, REPORT_LIMIT).unwrap_or_else(|_| "invalid percent encoding".into())
    } else {
        percent_decode(&raw, REPORT_LIMIT)
            .unwrap_or_else(|_| raw.chars().take(REPORT_LIMIT).collect())
    };
    let clean: String = decoded
        .chars()
        .take(REPORT_LIMIT)
        .map(|ch| if ch.is_control() { ' ' } else { ch })
        .collect();
    eprintln!(
        "{{\"event\":\"browser_error\",\"peer\":{},\"message\":{}}}",
        json_string(&ip.to_string()),
        json_string(clean.trim())
    );
}

#[cfg(unix)]
fn ensure_static_platform() -> Result<(), String> {
    Ok(())
}

#[cfg(not(unix))]
fn ensure_static_platform() -> Result<(), String> {
    Err("refusing to serve static files: descriptor-relative serving requires Unix".into())
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
    const RESOLVE_NO_SYMLINKS: u64 = 0x04;
    const RESOLVE_BENEATH: u64 = 0x08;
    let path = CString::new(relative)
        .map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    let how = OpenHow {
        flags: (libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NONBLOCK) as u64,
        mode: 0,
        resolve: RESOLVE_BENEATH | RESOLVE_NO_MAGICLINKS | RESOLVE_NO_SYMLINKS,
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
    let result = if fd < 0 {
        Err(std::io::Error::last_os_error())
    } else {
        // SAFETY: `openat2` returned a fresh descriptor owned by this call.
        Ok(unsafe { File::from_raw_fd(fd) })
    };
    openat2_or_component_walk(root_fd, relative, result)
}

#[cfg(target_os = "linux")]
fn openat2_or_component_walk(
    root_fd: &File,
    relative: &str,
    result: std::io::Result<File>,
) -> std::io::Result<File> {
    match result {
        Err(error) if matches!(error.raw_os_error(), Some(libc::ENOSYS | libc::EINVAL)) => {
            open_beneath_components(root_fd, relative)
        }
        result => result,
    }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn open_beneath(root_fd: &File, _root: &Path, relative: &str) -> std::io::Result<File> {
    open_beneath_components(root_fd, relative)
}

#[cfg(unix)]
fn open_beneath_components(root_fd: &File, relative: &str) -> std::io::Result<File> {
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

pub fn cloudflare_peer(ip: IpAddr) -> bool {
    match normalize_client_ip(ip) {
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

pub fn client_ip(headers: &HttpServerHeaders) -> IpAddr {
    let peer = normalize_client_ip(headers.addr.ip());
    let client = if cloudflare_peer(peer) {
        let mut forwarded = headers
            .lines
            .iter()
            .skip(1)
            .filter_map(|line| makepad_network::utils::split_header_line(line, "CF-Connecting-IP"));
        forwarded
            .next()
            .filter(|_| forwarded.next().is_none())
            .filter(|value| !value.contains(','))
            .and_then(|value| value.parse().ok())
            .unwrap_or(peer)
    } else {
        peer
    };
    normalize_client_ip(client)
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
    if lower.starts_with("maps/") || is_hashed_asset(&lower) {
        "public, max-age=31536000, immutable"
    } else {
        "private, no-cache"
    }
}

fn is_hashed_asset(path: &str) -> bool {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    let name = file_name.strip_suffix(".br").unwrap_or(file_name);
    let (stem, kind) = if let Some(stem) = name.strip_suffix(".wasm") {
        (stem, "wasm")
    } else if let Some(stem) = name.strip_suffix(".bin") {
        (stem, "bin")
    } else {
        return false;
    };
    let Some((prefix, hash)) = stem.rsplit_once('.') else { return false };
    if hash.len() != 16 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return false;
    }
    match kind {
        "wasm" => !prefix.is_empty()
            && !prefix.ends_with(".data")
            && !prefix.ends_with(".names"),
        "bin" => prefix
            .strip_suffix(".data")
            .is_some_and(|package| !package.is_empty()),
        _ => false,
    }
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

fn common_file_headers(etag: &str, last_modified: &str, encoded: bool, public: bool) -> String {
    format!(
        "Accept-Ranges: bytes\r\nETag: {etag}\r\nLast-Modified: {last_modified}\r\nVary: Accept-Encoding\r\n{}{}",
        if encoded { "Content-Encoding: br\r\n" } else { "" },
        if public { PUBLIC_ASSET_HEADERS } else { "" }
    )
}

pub fn etag(file: &mut File, encoded: bool) -> std::io::Result<String> {
    let position = file.stream_position()?;
    file.seek(std::io::SeekFrom::Start(0))?;
    let digest = (|| {
        let mut sha256 = makepad_network::digest::Sha256::new();
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            sha256.update(&buffer[..read]);
        }
        Ok::<_, std::io::Error>(sha256.finalise())
    })();
    let restored = file.seek(std::io::SeekFrom::Start(position));
    let digest = digest?;
    restored?;
    let mut value = String::with_capacity(69);
    value.push('"');
    for byte in digest {
        write!(&mut value, "{byte:02x}").expect("write to string");
    }
    if encoded {
        value.push_str("-br");
    }
    value.push('"');
    Ok(value)
}

fn if_none_match(header: Option<&str>, etag: &str) -> bool {
    header.is_some_and(|value| {
        value.split(',').any(|candidate| {
            let candidate = candidate.trim();
            candidate == "*" || candidate == etag || candidate.strip_prefix("W/") == Some(etag)
        })
    })
}

fn modified_seconds(metadata: &Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .unwrap_or_default()
        .as_secs()
}

fn if_range_matches(value: &str, etag: &str, modified: u64) -> bool {
    let value = value.trim();
    if value.starts_with('"') {
        value == etag
    } else {
        parse_http_date(value).is_some_and(|since| modified <= since)
    }
}

fn http_date(seconds: u64) -> String {
    let days = (seconds / 86_400) as i64;
    let day_seconds = seconds % 86_400;
    let hour = day_seconds / 3_600;
    let minute = day_seconds % 3_600 / 60;
    let second = day_seconds % 60;
    let (year, month, day) = civil_from_days(days);
    let weekdays = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    let months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let weekday = weekdays[(days + 4).rem_euclid(7) as usize];
    format!(
        "{weekday}, {day:02} {} {year:04} {hour:02}:{minute:02}:{second:02} GMT",
        months[month as usize - 1]
    )
}

fn parse_http_date(value: &str) -> Option<u64> {
    let fields = value.split_ascii_whitespace().collect::<Vec<_>>();
    if fields.len() != 6 || !fields[0].ends_with(',') || fields[5] != "GMT" {
        return None;
    }
    let day = fields[1].parse::<u32>().ok()?;
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        .iter()
        .position(|month| *month == fields[2])? as u32
        + 1;
    let year = fields[3].parse::<i64>().ok()?;
    let clock = fields[4].split(':').collect::<Vec<_>>();
    if clock.len() != 3 {
        return None;
    }
    let hour = clock[0].parse::<u64>().ok()?;
    let minute = clock[1].parse::<u64>().ok()?;
    let second = clock[2].parse::<u64>().ok()?;
    if !(1970..=9999).contains(&year)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    u64::try_from(days).ok()?.checked_mul(86_400)?
        .checked_add(hour * 3_600 + minute * 60 + second)
}

fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z - era * 146_097;
    let year_of_era = (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month as u32, day as u32)
}

fn days_from_civil(mut year: i64, month: u32, day: u32) -> Option<i64> {
    if !(1..=12).contains(&month) {
        return None;
    }
    year -= i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_prime = i64::from(month) + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month_prime + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
}

fn accepts_brotli(header: Option<&str>) -> bool {
    header.is_some_and(|value| {
        let mut explicit = None;
        let mut wildcard = None;
        for item in value.split(',') {
            let mut parts = item.trim().split(';');
            let encoding = parts.next().unwrap_or("").trim();
            if !encoding.eq_ignore_ascii_case("br") && encoding != "*" { continue }
            let mut quality = 1.0f32;
            let mut saw_quality = false;
            let mut valid = true;
            for parameter in parts {
                let parameter = parameter.trim();
                let Some((name, value)) = parameter.split_once('=') else { continue };
                if name.trim().eq_ignore_ascii_case("q") {
                    if saw_quality {
                        valid = false;
                        break;
                    }
                    let Some(parsed) = parse_quality(value.trim()) else {
                        valid = false;
                        break;
                    };
                    saw_quality = true;
                    quality = parsed;
                }
            }
            if !valid { continue }
            if encoding.eq_ignore_ascii_case("br") { explicit = Some(quality) } else { wildcard = Some(quality) }
        }
        explicit.or(wildcard).is_some_and(|quality| quality > 0.0)
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

fn range_error(
    len: u64,
    cache: &str,
    etag: &str,
    last_modified: &str,
    public: bool,
) -> HttpServerResponse {
    response(
        416,
        Some("text/plain; charset=utf-8"),
        cache,
        &format!(
            "Accept-Ranges: bytes\r\nContent-Range: bytes */{len}\r\nETag: {etag}\r\nLast-Modified: {last_modified}\r\nVary: Accept-Encoding\r\n{}",
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
    #[cfg(target_os = "linux")]
    use std::io::Read;

    fn headers(peer: &str, forwarded: Option<&str>) -> HttpServerHeaders {
        let peer_ip = peer.parse::<IpAddr>().unwrap();
        let mut lines = vec!["GET / HTTP/1.1\r\n".into()];
        if let Some(forwarded) = forwarded {
            lines.push(format!("CF-Connecting-IP: {forwarded}\r\n"));
        }
        HttpServerHeaders {
            addr: std::net::SocketAddr::new(peer_ip, 443),
            addr_text: std::net::SocketAddr::new(peer_ip, 443).to_string(),
            lines,
            verb: "GET".into(),
            path: "/".into(),
            path_no_slash: String::new(),
            search: None,
            content_length: None,
            accept_encoding: None,
            sec_websocket_key: None,
        }
    }

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
    fn redundant_path_components_are_rejected_before_platform_open() {
        for path in ["dir//file.js", "dir/./file.js", "./file.js", "dir/../file.js", ""] {
            assert!(validate_relative_components(path).is_err(), "accepted {path:?}");
        }
        assert!(validate_relative_components("dir/file.js").is_ok());
    }

    #[test]
    fn brotli_requires_a_valid_positive_quality() {
        assert!(accepts_brotli(Some("gzip, br")));
        assert!(accepts_brotli(Some("br;q=0.5")));
        assert!(accepts_brotli(Some("*;q=0.5")));
        assert!(!accepts_brotli(Some("*;q=1, br;q=0")));
        for invalid in ["br;q=0", "br;q=garbage", "br;q=0.0001", "br;q=2", "br;q=1;q=0.5"] {
            assert!(!accepts_brotli(Some(invalid)), "accepted {invalid}");
        }
    }

    #[test]
    fn cache_policy_distinguishes_mutable_and_content_addressed_files() {
        assert_eq!(cache_policy("index.html"), "private, no-cache");
        assert!(cache_policy("maps/root.mkidx").contains("immutable"));
        assert!(cache_policy("maps/tiles-003.mkshard").contains("immutable"));
        for path in [
            "app.0123456789abcdef.wasm",
            "app.secondary.0123456789abcdef.wasm.br",
            "app.data.0123456789abcdef.bin",
        ] {
            assert!(cache_policy(path).contains("immutable"), "{path}");
        }
        for path in [
            "app-a1b2c3d4.wasm",
            "app.wasm",
            "app.names.wasm",
            "app.0123.wasm",
            "app.0123456789abcdef.js",
        ] {
            assert_eq!(cache_policy(path), "private, no-cache", "{path}");
        }
    }

    #[test]
    fn http_dates_round_trip() {
        for seconds in [0, 1_445_412_480, 1_800_000_000] {
            assert_eq!(parse_http_date(&http_date(seconds)), Some(seconds));
        }
    }

    #[test]
    fn forwarded_client_ip_requires_a_cloudflare_peer() {
        let spoofed = headers("192.0.2.10", Some("198.51.100.7"));
        assert_eq!(client_ip(&spoofed), "192.0.2.10".parse::<IpAddr>().unwrap());

        let trusted = headers("173.245.48.1", Some("2001:db8:1234:5678:abcd::1"));
        assert_eq!(
            client_ip(&trusted),
            "2001:db8:1234:5678::".parse::<IpAddr>().unwrap()
        );

        let mapped_peer = headers("::ffff:173.245.48.1", Some("::ffff:198.51.100.7"));
        assert!(cloudflare_peer(mapped_peer.addr.ip()));
        assert_eq!(client_ip(&mapped_peer), "198.51.100.7".parse::<IpAddr>().unwrap());

        let mut duplicate = headers("173.245.48.1", Some("198.51.100.7"));
        duplicate.lines.push("CF-Connecting-IP: 203.0.113.8\r\n".into());
        assert_eq!(client_ip(&duplicate), "173.245.48.1".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn report_limiter_groups_ipv6_64_and_does_not_evict_live_clients() {
        let now = Instant::now();
        let mut limiter = ReportRateLimiter::new(1);
        assert!(limiter.allow_at("2001:db8::1".parse().unwrap(), now));
        assert!(limiter.allow_at("2001:db8::2".parse().unwrap(), now));
        assert!(!limiter.allow_at("2001:db8:0:1::1".parse().unwrap(), now));
        assert_eq!(limiter.len(), 1);
    }

    #[test]
    fn report_limiter_has_a_global_token_bucket() {
        let mut limiter = ReportRateLimiter::new(300);
        let now = Instant::now();
        for subnet in 0..GLOBAL_REPORT_BURST as u128 {
            let ip = IpAddr::V6(Ipv6Addr::from((0x2001_0db8u128 << 96) | (subnet << 64)));
            assert!(limiter.allow_at(ip, now));
        }
        let next = IpAddr::V6("3001:db8::1".parse().unwrap());
        assert!(!limiter.allow_at(next, now));
        assert!(limiter.allow_at(next, now + Duration::from_millis(500)));
    }

    #[cfg(unix)]
    #[test]
    fn component_walk_refuses_replaced_directory_symlink() {
        use std::os::unix::fs::symlink;

        let base = std::env::temp_dir().join(format!(
            "makepad-web-component-walk-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        let root = base.join("root");
        let outside = base.join("outside");
        fs::create_dir_all(root.join("dir")).unwrap();
        fs::create_dir(&outside).unwrap();
        fs::write(root.join("dir/file.txt"), b"public").unwrap();
        fs::write(outside.join("file.txt"), b"secret").unwrap();
        let root_fd = File::open(&root).unwrap();
        fs::rename(root.join("dir"), root.join("old-dir")).unwrap();
        symlink(&outside, root.join("dir")).unwrap();
        assert!(open_beneath_components(&root_fd, "dir/file.txt").is_err());
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn openat2_capability_failure_uses_component_walk() {
        let base = std::env::temp_dir().join(format!(
            "makepad-web-openat2-fallback-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir(&base).unwrap();
        fs::write(base.join("file.txt"), b"fallback").unwrap();
        let root_fd = File::open(&base).unwrap();
        let mut file = openat2_or_component_walk(
            &root_fd,
            "file.txt",
            Err(std::io::Error::from_raw_os_error(libc::ENOSYS)),
        )
        .unwrap();
        let mut contents = String::new();
        file.read_to_string(&mut contents).unwrap();
        assert_eq!(contents, "fallback");
        fs::remove_dir_all(base).unwrap();
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
    fn etag_changes_with_bytes_when_length_and_mtime_are_preserved() {
        let dir = std::env::temp_dir().join(format!(
            "makepad-web-etag-{}-{:?}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir(&dir).unwrap();
        let path = dir.join("asset.bin");
        fs::write(&path, b"AAA").unwrap();
        let modified = path.metadata().unwrap().modified().unwrap();
        let mut first_file = File::open(&path).unwrap();
        let first = etag(&mut first_file, false).unwrap();
        let first_encoded = etag(&mut first_file, true).unwrap();
        assert!(!first.ends_with("-br\""));
        assert!(first_encoded.ends_with("-br\""));
        fs::write(&path, b"BBB").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(modified))
            .unwrap();
        let mut second_file = File::open(&path).unwrap();
        let second = etag(&mut second_file, false).unwrap();
        assert_eq!(path.metadata().unwrap().len(), 3);
        assert_eq!(path.metadata().unwrap().modified().unwrap(), modified);
        assert_ne!(first, second);
        fs::remove_file(path).unwrap();
        fs::remove_dir(dir).unwrap();
    }
}
