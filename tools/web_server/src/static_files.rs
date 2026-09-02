use crate::http::{
    json_string, percent_decode, reason, response, send_response, APP_ISOLATION_HEADERS,
    PUBLIC_ASSET_HEADERS,
};
use makepad_network::http_server::{
    HttpServerResponse, HttpServerResponseSender,
};
use makepad_network::HttpServerHeaders;
use std::{
    collections::HashMap,
    ffi::OsString,
    fs::{File, Metadata},
    net::IpAddr,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant, UNIX_EPOCH},
};

const REPORT_LIMIT: usize = 8_192;
const REPORTS_PER_MINUTE: u32 = 10;

pub struct StaticHandler {
    root: PathBuf,
    reports: Mutex<HashMap<IpAddr, (Instant, u32)>>,
}

impl StaticHandler {
    pub fn new(root: &Path) -> Result<Self, String> {
        let root = root
            .canonicalize()
            .map_err(|error| format!("canonicalize static root {}: {error}", root.display()))?;
        if !root.is_dir() {
            return Err(format!("static root {} is not a directory", root.display()));
        }
        Ok(Self {
            root,
            reports: Mutex::new(HashMap::new()),
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
        let request_path = match percent_decode(&headers.path, 8_192) {
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
        let unresolved = self.root.join(relative);
        let original = match unresolved.canonicalize() {
            Ok(path) if path.starts_with(&self.root) => path,
            Ok(_) => {
                send_response(sender, static_error(400, "bad request"));
                return;
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                send_response(sender, static_error(404, "not found"));
                return;
            }
            Err(_) => {
                send_response(sender, static_error(500, "internal error"));
                return;
            }
        };
        if !original.is_file() {
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
        let brotli_path = (!range_requested && !never_brotli)
            .then(|| sibling_brotli(&original))
            .filter(|path| path.is_file());
        let vary = brotli_path.is_some();
        let use_brotli = vary && accepts_brotli(headers.header("Accept-Encoding"));
        let selected = if use_brotli {
            match brotli_path.as_ref().unwrap().canonicalize() {
                Ok(path) if path.starts_with(&self.root) => path,
                _ => {
                    send_response(sender, static_error(500, "internal error"));
                    return;
                }
            }
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
        match File::open(&selected) {
            Ok(file) => {
                let _ = sender.send(HttpServerResponse::from_file(header, file, offset, body_len));
            }
            Err(_) => send_response(sender, static_error(500, "internal error")),
        }
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
        let ip = headers.addr.ip();
        let now = Instant::now();
        let allowed = {
            let mut reports = self.reports.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let entry = reports.entry(ip).or_insert((now, 0));
            if now.duration_since(entry.0) >= Duration::from_secs(60) {
                *entry = (now, 0);
            }
            if entry.1 >= REPORTS_PER_MINUTE {
                false
            } else {
                entry.1 += 1;
                true
            }
        };
        if allowed {
            eprintln!(
                "{{\"event\":\"browser_error\",\"peer\":{},\"message\":{}}}",
                json_string(&ip.to_string()),
                json_string(clean.trim())
            );
        }
    }
}

fn sibling_brotli(path: &Path) -> PathBuf {
    let mut name: OsString = path.as_os_str().to_owned();
    name.push(".br");
    PathBuf::from(name)
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
            !parts.any(|parameter| {
                parameter
                    .trim()
                    .strip_prefix("q=")
                    .and_then(|quality| quality.parse::<f32>().ok())
                    == Some(0.0)
            })
        })
    })
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
