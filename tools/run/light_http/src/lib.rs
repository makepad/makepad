//! One blocking HTTP request, client only, on the operating system's own TLS stack: NSURLSession
//! on macOS, the system OpenSSL (libssl.so.3) on Linux, WinHTTP on Windows. No dependencies: the
//! few C functions it calls are declared here and linked with `#[link]`.
//!
//! What it does: GET/POST/HEAD with headers and a body, the response body streamed into a
//! writer with progress, a total timeout, a body size cap. Redirects are never followed (a 3xx
//! comes back as it is). Certificates are checked against the system's trust store. `http://`
//! is refused except to loopback hosts (tests). No proxies on Linux (macOS and Windows use the
//! system proxy settings), no HTTP/2, no connection reuse.
use std::io::Write;
use std::time::Duration;

#[cfg(target_os = "macos")]
mod apple;
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod h1;
#[cfg(target_os = "linux")]
mod openssl;
#[cfg(windows)]
mod winhttp;

/// The User-Agent sent when the request names none.
pub const USER_AGENT: &str = "makepad-run/0.1";

pub struct Request<'a> {
    pub method: &'a str,
    pub url: &'a str,
    pub headers: &'a [(&'a str, &'a str)],
    pub body: &'a [u8],
    /// The whole request, from connecting to the last body byte.
    pub timeout: Duration,
    /// A larger response body fails the request.
    pub max_body: u64,
}

impl<'a> Request<'a> {
    pub fn get(url: &'a str) -> Self {
        Request { method: "GET", url, headers: &[], body: &[], timeout: Duration::from_secs(300), max_body: 4 << 30 }
    }
    pub fn post(url: &'a str, body: &'a [u8]) -> Self {
        Request { method: "POST", body, ..Request::get(url) }
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
}

/// The parts of an http(s) URL.
#[cfg(not(target_os = "android"))]
pub(crate) struct Url<'a> {
    pub https: bool,
    pub host: &'a str,
    pub port: u16,
    /// path and query, at least "/"
    pub target: &'a str,
}

#[cfg(not(target_os = "android"))]
pub(crate) fn parse_url(url: &str) -> Result<Url<'_>, String> {
    let (https, rest) = if let Some(r) = url.strip_prefix("https://") {
        (true, r)
    } else if let Some(r) = url.strip_prefix("http://") {
        (false, r)
    } else {
        return Err(format!("not an http(s) URL: {url}"));
    };
    if url.bytes().any(|b| b <= b' ' || b == 0x7f) {
        return Err("URL with spaces or control characters".into());
    }
    let (authority, target) = match rest.find(['/', '?']) {
        Some(i) if rest.as_bytes()[i] == b'/' => (&rest[..i], &rest[i..]),
        Some(_) => return Err("URL query without a path".into()),
        None => (rest, "/"),
    };
    if authority.contains('@') {
        return Err("URL with credentials".into());
    }
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) if !h.ends_with(']') || h.starts_with('[') => {
            (h, p.parse::<u16>().map_err(|_| format!("bad port in {url}"))?)
        }
        _ => (authority, if https { 443 } else { 80 }),
    };
    if host.is_empty() {
        return Err(format!("no host in {url}"));
    }
    let target = target.split('#').next().unwrap_or("/");
    if !https && !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
        return Err("plain http only to loopback; use https".into());
    }
    Ok(Url { https, host, port, target })
}

/// Sends `req`; the response body goes to `out`, `progress(received, expected)` follows it.
#[cfg(not(target_os = "android"))]
pub fn send(req: &Request, out: &mut dyn Write, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Response, String> {
    let url = parse_url(req.url)?;
    for (n, v) in req.headers {
        if n.is_empty() || n.bytes().any(|b| b <= b' ' || b == b':' || b >= 0x7f) || v.bytes().any(|b| b == b'\r' || b == b'\n' || b == 0) {
            return Err(format!("invalid header {n}"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if url.https {
            return apple::send(req, out, progress);
        }
        h1::send(req, &url, out, progress)
    }
    #[cfg(target_os = "linux")]
    {
        h1::send(req, &url, out, progress)
    }
    #[cfg(windows)]
    {
        winhttp::send(req, &url, out, progress)
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    {
        let _ = (url, out, progress);
        Err("no HTTP client on this platform".into())
    }
}

/// [`send`] with the body collected in memory.
pub fn fetch(req: &Request) -> Result<(Response, Vec<u8>), String> {
    let mut body = Vec::new();
    let resp = send(req, &mut body, &mut |_, _| {})?;
    Ok((resp, body))
}

/// Writes into `out`, counting against the cap and reporting progress.
#[cfg(not(target_os = "android"))]
pub(crate) struct Sink<'a> {
    pub out: &'a mut dyn Write,
    pub progress: &'a mut dyn FnMut(u64, Option<u64>),
    pub received: u64,
    pub expected: Option<u64>,
    pub max: u64,
}

#[cfg(not(target_os = "android"))]
impl Sink<'_> {
    pub fn put(&mut self, data: &[u8]) -> Result<(), String> {
        self.received += data.len() as u64;
        if self.received > self.max {
            return Err(format!("response body larger than {} bytes", self.max));
        }
        self.out.write_all(data).map_err(|e| e.to_string())?;
        (self.progress)(self.received, self.expected);
        Ok(())
    }
}

#[cfg(test)]
mod tests;

/// Android networking is supplied by the Activity's JNI bridge (not system OpenSSL).
#[cfg(target_os = "android")]
pub fn send(_req: &Request, _out: &mut dyn Write, _progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<Response, String> {
    Err("https on android goes through the JNI bridge".into())
}
