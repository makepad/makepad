//! A small static server for a packed wasm app's folder: a build tool
//! (Stage's web export, `cargo makepad wasm --pack`) serves its output so
//! it opens in a browser on this machine (localhost) or, when the user
//! picks it, on a phone on the same network.
//!
//! It serves the files of one folder and nothing else: no listing, no
//! path outside it (`..`, absolute paths and hidden files are refused),
//! GET only. MIME types for web files (`application/wasm`), `*.wasm.br`
//! with `Content-Encoding: br` (the browser then decodes it), byte ranges
//! (a browser's media loader asks for them), and no caching (a new build
//! is seen at once).
//!
//! Built on the platform's HTTP server (`makepad-network`); one long-lived
//! thread answers requests while the server runs.

use makepad_network::{start_http_server, HttpServer, HttpServerRequest, HttpServerResponse};
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

pub struct PackServer {
    pub addr: SocketAddr,
    /// The URL to open on this machine.
    pub local_url: String,
    /// The URL a device on the same network opens (when bound to the LAN).
    pub lan_url: Option<String>,
}

/// The MIME type of an export file.
pub fn mime(name: &str) -> &'static str {
    let name = name.strip_suffix(".br").unwrap_or(name);
    match name.rsplit('.').next().unwrap_or("").to_ascii_lowercase().as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        "css" => "text/css",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "mp3" => "audio/mpeg",
        "m4a" | "aac" => "audio/mp4",
        "ogg" | "opus" => "audio/ogg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

/// The file a request path names inside `root`, or None for anything
/// that could leave it.
fn resolve(root: &Path, path: &str) -> Option<PathBuf> {
    let path = path.split(['?', '#']).next().unwrap_or("");
    let rel = path.trim_start_matches('/');
    let rel = if rel.is_empty() { "index.html" } else { rel };
    if rel.split('/').any(|seg| seg.is_empty() || seg == ".." || seg.starts_with('.')) || rel.contains('\\') || rel.contains(':') {
        return None;
    }
    let p = root.join(rel);
    p.is_file().then_some(p)
}

/// `Range: bytes=a-b` as (start, end inclusive) within `len`.
fn range(header: Option<&str>, len: u64) -> Option<(u64, u64)> {
    let spec = header?.trim().strip_prefix("bytes=")?;
    let (a, b) = spec.split(',').next()?.split_once('-')?;
    let (start, end) = match (a.trim(), b.trim()) {
        ("", suffix) => {
            let n: u64 = suffix.parse().ok()?;
            (len.saturating_sub(n), len.checked_sub(1)?)
        }
        (a, "") => (a.parse().ok()?, len.checked_sub(1)?),
        (a, b) => (a.parse().ok()?, b.parse::<u64>().ok()?.min(len.checked_sub(1)?)),
    };
    (start <= end && end < len).then_some((start, end))
}

fn status_response(code: u16, text: &str) -> HttpServerResponse {
    let body = text.as_bytes().to_vec();
    HttpServerResponse::new(
        format!("HTTP/1.1 {code} {text}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()),
        body,
    )
}

fn respond(root: &Path, headers: &makepad_network::HttpServerHeaders) -> HttpServerResponse {
    let Some(file) = resolve(root, &headers.path) else { return status_response(404, "Not Found") };
    let Ok(f) = std::fs::File::open(&file) else { return status_response(404, "Not Found") };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let name = file.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let encoding = if name.ends_with(".wasm.br") { "Content-Encoding: br\r\n" } else { "" };
    let common = format!("Content-Type: {}\r\n{encoding}Cache-Control: no-store\r\nAccept-Ranges: bytes\r\nConnection: close\r\n", mime(&name));
    if encoding.is_empty() {
        if let Some((start, end)) = range(headers.header("Range"), len) {
            let n = end - start + 1;
            return HttpServerResponse::from_file(format!("HTTP/1.1 206 Partial Content\r\n{common}Content-Range: bytes {start}-{end}/{len}\r\nContent-Length: {n}\r\n\r\n"), f, start, n);
        }
    }
    HttpServerResponse::from_file(format!("HTTP/1.1 200 OK\r\n{common}Content-Length: {len}\r\n\r\n"), f, 0, len)
}

/// This machine's address on the local network, when it has one.
fn lan_ip() -> Option<IpAddr> {
    // No packet is sent: connecting a UDP socket only picks the route.
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.168.0.1:9").ok()?;
    let ip = s.local_addr().ok()?.ip();
    (!ip.is_loopback() && !ip.is_unspecified()).then_some(ip)
}

/// Serves `root` on `port` (0: any free port), on localhost only unless
/// `lan`. The server runs for the life of the process.
pub fn serve(root: &Path, lan: bool, port: u16) -> Result<PackServer, String> {
    let root = root.canonicalize().map_err(|e| format!("{}: {e}", root.display()))?;
    // Find a free port first (the platform server binds by address).
    let bind_ip = if lan { IpAddr::V4(Ipv4Addr::UNSPECIFIED) } else { IpAddr::V4(Ipv4Addr::LOCALHOST) };
    let probe = std::net::TcpListener::bind(SocketAddr::new(bind_ip, port)).map_err(|e| format!("cannot listen on port {port}: {e}"))?;
    let addr = probe.local_addr().map_err(|e| e.to_string())?;
    drop(probe);
    let (tx, rx) = std::sync::mpsc::channel::<HttpServerRequest>();
    let server = HttpServer {
        listen_address: addr,
        request: tx,
        post_max_size: 0,
        post_max_size_overrides: Vec::new(),
        pre_admit_posts: false,
        client_ip_resolver: None,
        trusted_proxy: None,
        allowed_methods: Some(|_| Some("GET, HEAD")),
    };
    start_http_server(server).ok_or_else(|| format!("cannot listen on {addr}"))?;
    std::thread::Builder::new()
        .name("pack-server".into())
        .spawn(move || {
            while let Ok(request) = rx.recv() {
                if let HttpServerRequest::Get { headers, response_sender } = request {
                    let _ = response_sender.send(respond(&root, &headers));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    let local_url = format!("http://localhost:{}/", addr.port());
    let lan_url = if lan { lan_ip().map(|ip| format!("http://{ip}:{}/", addr.port())) } else { None };
    Ok(PackServer { addr, local_url, lan_url })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_folder() {
        let dir = std::env::temp_dir().join(format!("pack-serve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.html"), "x").unwrap();
        std::fs::write(dir.join(".hidden"), "x").unwrap();
        assert!(resolve(&dir, "/").is_some());
        assert!(resolve(&dir, "/index.html?x=1").is_some());
        assert!(resolve(&dir, "/../etc/passwd").is_none());
        assert!(resolve(&dir, "/.hidden").is_none());
        assert!(resolve(&dir, "//etc/passwd").is_none());
        assert!(resolve(&dir, "/missing.js").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ranges() {
        assert_eq!(range(Some("bytes=0-99"), 1000), Some((0, 99)));
        assert_eq!(range(Some("bytes=900-"), 1000), Some((900, 999)));
        assert_eq!(range(Some("bytes=-100"), 1000), Some((900, 999)));
        assert_eq!(range(Some("bytes=0-5000"), 1000), Some((0, 999)));
        assert_eq!(range(Some("bytes=1000-"), 1000), None);
        assert_eq!(range(None, 1000), None);
    }

    #[test]
    fn mimes() {
        assert_eq!(mime("film.wasm"), "application/wasm");
        assert_eq!(mime("film.wasm.br"), "application/wasm");
        assert_eq!(mime("song.0123456789abcdef.mp3"), "audio/mpeg");
    }
}
