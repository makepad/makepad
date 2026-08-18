//! Minimal blocking HTTP/1.1 client.
//!
//! HTTPS story: this repo avoids external deps, and it turns out no external
//! TLS dep is needed — `makepad-network::SocketStream` already wraps the
//! platform-native TLS stack behind blocking `Read`/`Write` on every OS we
//! care about (SecureTransport on macOS/iOS, WinRT `StreamSocket` on Windows,
//! a native impl on Linux). Plain `http://` uses `std::net::TcpStream`
//! directly, which is also what the unit-test fixture servers speak.
//!
//! Supports exactly what the HuggingFace download path and the LAN provider
//! client need: GET/POST, redirect following (resolve/main URLs 302 to the HF
//! CDN), `Range` for resume, `Authorization: Bearer` restricted to a host
//! suffix (the token must not leak to the CDN host on redirect),
//! content-length and chunked bodies, streaming body reads.

use crate::error::AssetAiError;
#[cfg(not(target_os = "windows"))]
use makepad_network::SocketStream;
use std::io::{Read, Write};
#[cfg(not(target_os = "windows"))]
use std::net::TcpStream;
use std::time::Duration;

const MAX_REDIRECTS: usize = 8;
#[cfg(any(not(target_os = "windows"), test))]
const MAX_HEAD_BYTES: usize = 256 * 1024;
const IO_TIMEOUT: Duration = Duration::from_secs(60);

pub struct HttpClientRequest<'a> {
    pub method: &'a str,
    pub url: &'a str,
    /// Resume offset: sends `Range: bytes=<n>-`.
    pub range_from: Option<u64>,
    /// Optional inclusive range end. Only meaningful with `range_from`.
    /// Peer downloads set this so a hostile source cannot choose an
    /// arbitrarily large response allocation through `Content-Range`.
    pub range_to: Option<u64>,
    /// Bearer token, only attached when the current hop's host equals
    /// `host_suffix` or ends with `.<host_suffix>`.
    pub bearer: Option<BearerAuth<'a>>,
    /// (content_type, bytes) for POST.
    pub body: Option<(&'a str, &'a [u8])>,
    /// Extra request headers, sent verbatim on every hop (peer transfers
    /// carry their ticket auth here — headers, never URLs). Values must not
    /// contain CR/LF; callers are in-crate and construct them from validated
    /// material.
    pub extra_headers: &'a [(String, String)],
}

impl<'a> HttpClientRequest<'a> {
    pub fn get(url: &'a str) -> Self {
        Self {
            method: "GET",
            url,
            range_from: None,
            range_to: None,
            bearer: None,
            body: None,
            extra_headers: &[],
        }
    }

    pub fn post(url: &'a str, content_type: &'a str, body: &'a [u8]) -> Self {
        Self {
            method: "POST",
            url,
            range_from: None,
            range_to: None,
            bearer: None,
            body: Some((content_type, body)),
            extra_headers: &[],
        }
    }
}

#[derive(Clone, Copy)]
pub struct BearerAuth<'a> {
    pub token: &'a str,
    pub host_suffix: &'a str,
}

pub struct HttpClientResponse {
    pub status: u16,
    /// Header names lowercased.
    pub headers: Vec<(String, String)>,
    pub body: BodyReader,
}

impl HttpClientResponse {
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn content_length(&self) -> Option<u64> {
        self.header("content-length")?.trim().parse().ok()
    }

    /// Total size out of `Content-Range: bytes <from>-<to>/<total>`.
    pub fn content_range_total(&self) -> Option<u64> {
        let value = self.header("content-range")?;
        let total = value.rsplit('/').next()?.trim();
        total.parse().ok()
    }

    pub fn read_body_to_vec(mut self, max_len: usize) -> Result<Vec<u8>, AssetAiError> {
        let mut out = Vec::new();
        let mut buf = [0u8; 16384];
        loop {
            let n = self
                .body
                .read(&mut buf)
                .map_err(|e| AssetAiError::Http(format!("body read: {e}")))?;
            if n == 0 {
                return Ok(out);
            }
            if out.len() + n > max_len {
                return Err(AssetAiError::Http("response body too large".into()));
            }
            out.extend_from_slice(&buf[..n]);
        }
    }
}

// ---------------------------------------------------------------------------
// URL parsing
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct ParsedUrl {
    pub https: bool,
    pub host: String,
    pub port: u16,
    /// Path plus query, always starting with '/'.
    pub target: String,
}

pub fn parse_url(url: &str) -> Result<ParsedUrl, AssetAiError> {
    let (https, rest) = if let Some(rest) = url.strip_prefix("https://") {
        (true, rest)
    } else if let Some(rest) = url.strip_prefix("http://") {
        (false, rest)
    } else {
        return Err(AssetAiError::Http(format!(
            "url must start with http:// or https://: {url}"
        )));
    };
    let (authority, target) = match rest.find('/') {
        Some(pos) => (&rest[..pos], rest[pos..].to_string()),
        None => (rest, "/".to_string()),
    };
    let (host, port) = match authority.rfind(':') {
        Some(pos) => {
            let port = authority[pos + 1..]
                .parse::<u16>()
                .map_err(|_| AssetAiError::Http(format!("bad port in url: {url}")))?;
            (authority[..pos].to_string(), port)
        }
        None => (
            authority.to_string(),
            if https { 443 } else { 80 },
        ),
    };
    if host.is_empty() {
        return Err(AssetAiError::Http(format!("empty host in url: {url}")));
    }
    Ok(ParsedUrl {
        https,
        host,
        port,
        target,
    })
}

fn host_matches_suffix(host: &str, suffix: &str) -> bool {
    host == suffix || host.ends_with(&format!(".{suffix}"))
}

// ---------------------------------------------------------------------------
// Transport: plain TCP or platform-native TLS
// ---------------------------------------------------------------------------

enum Transport {
    #[cfg(not(target_os = "windows"))]
    Plain(TcpStream),
    #[cfg(not(target_os = "windows"))]
    Tls(SocketStream),
    #[cfg(target_os = "windows")]
    WinHttp(WinHttpBody),
    #[cfg(test)]
    Mem(std::io::Cursor<Vec<u8>>),
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            #[cfg(not(target_os = "windows"))]
            Transport::Plain(s) => s.read(buf),
            #[cfg(not(target_os = "windows"))]
            Transport::Tls(s) => s.read(buf),
            #[cfg(target_os = "windows")]
            Transport::WinHttp(s) => s.read(buf),
            #[cfg(test)]
            Transport::Mem(s) => s.read(buf),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
        match self {
            #[cfg(not(target_os = "windows"))]
            Transport::Plain(s) => s.write(_buf),
            #[cfg(not(target_os = "windows"))]
            Transport::Tls(s) => s.write(_buf),
            #[cfg(target_os = "windows")]
            Transport::WinHttp(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "WinHTTP response streams are read-only",
            )),
            #[cfg(test)]
            Transport::Mem(_) => Ok(_buf.len()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            #[cfg(not(target_os = "windows"))]
            Transport::Plain(s) => s.flush(),
            #[cfg(not(target_os = "windows"))]
            Transport::Tls(s) => s.flush(),
            #[cfg(target_os = "windows")]
            Transport::WinHttp(_) => Ok(()),
            #[cfg(test)]
            Transport::Mem(_) => Ok(()),
        }
    }
}

#[cfg(not(target_os = "windows"))]
fn connect(url: &ParsedUrl) -> Result<Transport, AssetAiError> {
    if url.https {
        let stream = SocketStream::connect(&url.host, &url.port.to_string(), true, false)
            .map_err(|e| AssetAiError::Http(format!("tls connect {}:{}: {e}", url.host, url.port)))?;
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        Ok(Transport::Tls(stream))
    } else {
        let stream = TcpStream::connect((url.host.as_str(), url.port))
            .map_err(|e| AssetAiError::Http(format!("connect {}:{}: {e}", url.host, url.port)))?;
        let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
        let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
        Ok(Transport::Plain(stream))
    }
}

// WinRT StreamSocket's synchronous-looking adapter cannot safely issue a
// second `block_on` operation on the same thread: the TLS connect completes,
// but the following DataWriter::StoreAsync can wait forever. Downloads use
// synchronous WinHTTP on Windows instead. Besides avoiding that deadlock,
// WinHTTP gives us real per-request timeouts and a streaming response handle.
#[cfg(target_os = "windows")]
const WINHTTP_ACCESS_TYPE_NO_PROXY: u32 = 1;
#[cfg(target_os = "windows")]
const WINHTTP_FLAG_SECURE: u32 = 0x0080_0000;
#[cfg(target_os = "windows")]
const WINHTTP_OPTION_DISABLE_FEATURE: u32 = 63;
#[cfg(target_os = "windows")]
const WINHTTP_DISABLE_REDIRECTS: u32 = 0x0000_0002;
#[cfg(target_os = "windows")]
const WINHTTP_QUERY_STATUS_CODE: u32 = 19;
#[cfg(target_os = "windows")]
const WINHTTP_QUERY_RAW_HEADERS_CRLF: u32 = 22;
#[cfg(target_os = "windows")]
const WINHTTP_QUERY_FLAG_NUMBER: u32 = 0x2000_0000;
#[cfg(target_os = "windows")]
const ERROR_INSUFFICIENT_BUFFER: u32 = 122;

#[cfg(target_os = "windows")]
#[link(name = "winhttp")]
unsafe extern "system" {
    fn WinHttpOpen(
        user_agent: *const u16,
        access_type: u32,
        proxy_name: *const u16,
        proxy_bypass: *const u16,
        flags: u32,
    ) -> *mut std::ffi::c_void;
    fn WinHttpConnect(
        session: *mut std::ffi::c_void,
        server_name: *const u16,
        server_port: u16,
        reserved: u32,
    ) -> *mut std::ffi::c_void;
    fn WinHttpOpenRequest(
        connect: *mut std::ffi::c_void,
        verb: *const u16,
        object_name: *const u16,
        version: *const u16,
        referrer: *const u16,
        accept_types: *const *const u16,
        flags: u32,
    ) -> *mut std::ffi::c_void;
    fn WinHttpSetTimeouts(
        handle: *mut std::ffi::c_void,
        resolve_timeout: i32,
        connect_timeout: i32,
        send_timeout: i32,
        receive_timeout: i32,
    ) -> i32;
    fn WinHttpSetOption(
        handle: *mut std::ffi::c_void,
        option: u32,
        buffer: *mut std::ffi::c_void,
        buffer_len: u32,
    ) -> i32;
    fn WinHttpSendRequest(
        request: *mut std::ffi::c_void,
        headers: *const u16,
        headers_len: u32,
        optional: *mut std::ffi::c_void,
        optional_len: u32,
        total_len: u32,
        context: usize,
    ) -> i32;
    fn WinHttpReceiveResponse(
        request: *mut std::ffi::c_void,
        reserved: *mut std::ffi::c_void,
    ) -> i32;
    fn WinHttpQueryHeaders(
        request: *mut std::ffi::c_void,
        info_level: u32,
        name: *const u16,
        buffer: *mut std::ffi::c_void,
        buffer_len: *mut u32,
        index: *mut u32,
    ) -> i32;
    fn WinHttpReadData(
        request: *mut std::ffi::c_void,
        buffer: *mut std::ffi::c_void,
        bytes_to_read: u32,
        bytes_read: *mut u32,
    ) -> i32;
    fn WinHttpCloseHandle(handle: *mut std::ffi::c_void) -> i32;
}

#[cfg(target_os = "windows")]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetLastError() -> u32;
}

#[cfg(target_os = "windows")]
fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(target_os = "windows")]
fn winhttp_last_error(operation: &str) -> AssetAiError {
    let code = unsafe { GetLastError() };
    AssetAiError::Http(format!(
        "{operation}: {} (code {code})",
        std::io::Error::from_raw_os_error(code as i32)
    ))
}

#[cfg(target_os = "windows")]
struct WinHttpBody {
    session: *mut std::ffi::c_void,
    connect: *mut std::ffi::c_void,
    request: *mut std::ffi::c_void,
}

#[cfg(target_os = "windows")]
impl Read for WinHttpBody {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut read = 0u32;
        let ok = unsafe {
            WinHttpReadData(
                self.request,
                buf.as_mut_ptr().cast::<std::ffi::c_void>(),
                buf.len().min(u32::MAX as usize) as u32,
                &mut read,
            )
        };
        if ok == 0 {
            let code = unsafe { GetLastError() };
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                format!(
                    "WinHttpReadData failed: {} (code {code})",
                    std::io::Error::from_raw_os_error(code as i32)
                ),
            ));
        }
        Ok(read as usize)
    }
}

#[cfg(target_os = "windows")]
impl Drop for WinHttpBody {
    fn drop(&mut self) {
        unsafe {
            if !self.request.is_null() {
                let _ = WinHttpCloseHandle(self.request);
            }
            if !self.connect.is_null() {
                let _ = WinHttpCloseHandle(self.connect);
            }
            if !self.session.is_null() {
                let _ = WinHttpCloseHandle(self.session);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn winhttp_raw_headers(request: *mut std::ffi::c_void) -> Result<String, AssetAiError> {
    let mut byte_len = 0u32;
    let first = unsafe {
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            std::ptr::null(),
            std::ptr::null_mut(),
            &mut byte_len,
            std::ptr::null_mut(),
        )
    };
    if first == 0 {
        let code = unsafe { GetLastError() };
        if code != ERROR_INSUFFICIENT_BUFFER {
            return Err(winhttp_last_error("WinHttpQueryHeaders(size) failed"));
        }
    }
    if byte_len < 2 {
        return Err(AssetAiError::Http(
            "WinHTTP returned an empty response header block".into(),
        ));
    }
    let mut wide = vec![0u16; (byte_len as usize + 1) / 2];
    let ok = unsafe {
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_RAW_HEADERS_CRLF,
            std::ptr::null(),
            wide.as_mut_ptr().cast::<std::ffi::c_void>(),
            &mut byte_len,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(winhttp_last_error("WinHttpQueryHeaders(raw) failed"));
    }
    let mut units = byte_len as usize / 2;
    while units > 0 && wide[units - 1] == 0 {
        units -= 1;
    }
    Ok(String::from_utf16_lossy(&wide[..units]))
}

#[cfg(target_os = "windows")]
fn winhttp_fetch_once(
    url: &ParsedUrl,
    method: &str,
    req: &HttpClientRequest,
    body: Option<(&str, &[u8])>,
    send_extra_headers: bool,
) -> Result<HttpClientResponse, AssetAiError> {
    let user_agent = wide_null(&format!(
        "{}/{}",
        crate::SERVICE_NAME,
        crate::SERVICE_VERSION
    ));
    let session = unsafe {
        WinHttpOpen(
            user_agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_NO_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        )
    };
    if session.is_null() {
        return Err(winhttp_last_error("WinHttpOpen failed"));
    }
    let mut handles = WinHttpBody {
        session,
        connect: std::ptr::null_mut(),
        request: std::ptr::null_mut(),
    };
    let timeout_ms = IO_TIMEOUT.as_millis().min(i32::MAX as u128) as i32;
    if unsafe {
        WinHttpSetTimeouts(
            handles.session,
            timeout_ms,
            timeout_ms,
            timeout_ms,
            timeout_ms,
        )
    } == 0
    {
        return Err(winhttp_last_error("WinHttpSetTimeouts failed"));
    }

    let host = wide_null(&url.host);
    handles.connect = unsafe { WinHttpConnect(handles.session, host.as_ptr(), url.port, 0) };
    if handles.connect.is_null() {
        return Err(winhttp_last_error("WinHttpConnect failed"));
    }

    let verb = wide_null(method);
    let target = wide_null(&url.target);
    handles.request = unsafe {
        WinHttpOpenRequest(
            handles.connect,
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            if url.https { WINHTTP_FLAG_SECURE } else { 0 },
        )
    };
    if handles.request.is_null() {
        return Err(winhttp_last_error("WinHttpOpenRequest failed"));
    }
    let mut disabled = WINHTTP_DISABLE_REDIRECTS;
    if unsafe {
        WinHttpSetOption(
            handles.request,
            WINHTTP_OPTION_DISABLE_FEATURE,
            (&mut disabled as *mut u32).cast::<std::ffi::c_void>(),
            std::mem::size_of::<u32>() as u32,
        )
    } == 0
    {
        return Err(winhttp_last_error(
            "WinHttpSetOption(DISABLE_REDIRECTS) failed",
        ));
    }

    let mut headers = String::from("Accept: */*\r\nAccept-Encoding: identity\r\n");
    if let Some(from) = req.range_from {
        match req.range_to {
            Some(to) if to >= from => headers.push_str(&format!("Range: bytes={from}-{to}\r\n")),
            Some(to) => {
                return Err(AssetAiError::Http(format!(
                    "invalid byte range {from}-{to}"
                )))
            }
            None => headers.push_str(&format!("Range: bytes={from}-\r\n")),
        }
    }
    if let Some(bearer) = &req.bearer {
        if host_matches_suffix(&url.host, bearer.host_suffix) {
            headers.push_str(&format!("Authorization: Bearer {}\r\n", bearer.token));
        }
    }
    if send_extra_headers {
        for (name, value) in req.extra_headers {
            if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
                return Err(AssetAiError::Http(
                    "request header contains a newline".to_string(),
                ));
            }
            headers.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    if let Some((content_type, _)) = body {
        headers.push_str(&format!("Content-Type: {content_type}\r\n"));
    }
    let headers = wide_null(&headers);
    let body_bytes = body.map(|(_, bytes)| bytes).unwrap_or_default();
    let body_len = u32::try_from(body_bytes.len())
        .map_err(|_| AssetAiError::Http("WinHTTP request body exceeds 4 GiB".into()))?;
    let body_ptr = if body_bytes.is_empty() {
        std::ptr::null_mut()
    } else {
        body_bytes.as_ptr() as *mut std::ffi::c_void
    };
    let ok = unsafe {
        WinHttpSendRequest(
            handles.request,
            headers.as_ptr(),
            u32::MAX,
            body_ptr,
            body_len,
            body_len,
            0,
        )
    };
    if ok == 0 {
        return Err(winhttp_last_error("WinHttpSendRequest failed"));
    }
    if unsafe { WinHttpReceiveResponse(handles.request, std::ptr::null_mut()) } == 0 {
        return Err(winhttp_last_error("WinHttpReceiveResponse failed"));
    }

    let mut status = 0u32;
    let mut status_len = std::mem::size_of::<u32>() as u32;
    if unsafe {
        WinHttpQueryHeaders(
            handles.request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            (&mut status as *mut u32).cast::<std::ffi::c_void>(),
            &mut status_len,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(winhttp_last_error("WinHttpQueryHeaders(status) failed"));
    }
    let status = u16::try_from(status)
        .map_err(|_| AssetAiError::Http(format!("invalid HTTP status {status}")))?;
    let raw_headers = winhttp_raw_headers(handles.request)?;
    let mut parsed_headers = Vec::new();
    for line in raw_headers.split("\r\n").skip(1) {
        if let Some(pos) = line.find(':') {
            parsed_headers.push((
                line[..pos].trim().to_ascii_lowercase(),
                line[pos + 1..].trim().to_string(),
            ));
        }
    }
    // WinHTTP de-chunks response bodies before WinHttpReadData returns them.
    parsed_headers.retain(|(name, _)| name != "transfer-encoding");
    let mode = body_mode(status, &parsed_headers)?;
    Ok(HttpClientResponse {
        status,
        headers: parsed_headers,
        body: BodyReader {
            transport: Transport::WinHttp(handles),
            prefix: Vec::new(),
            prefix_pos: 0,
            mode,
        },
    })
}

// ---------------------------------------------------------------------------
// Request execution with redirect following
// ---------------------------------------------------------------------------

pub fn http_fetch(req: &HttpClientRequest) -> Result<HttpClientResponse, AssetAiError> {
    let mut url = parse_url(req.url)?;
    let mut method = req.method.to_string();
    let mut body = req.body;
    let mut send_extra_headers = true;
    for _ in 0..=MAX_REDIRECTS {
        let response = fetch_once(&url, &method, req, body, send_extra_headers)?;
        match response.status {
            301 | 302 | 303 | 307 | 308 => {
                let location = response
                    .header("location")
                    .ok_or_else(|| AssetAiError::Http("redirect without location".into()))?
                    .to_string();
                let next = resolve_redirect(&url, &location)?;
                if !same_origin(&url, &next) {
                    // Extra headers can contain scoped credentials (peer
                    // tickets). A redirect must never turn the HTTP client
                    // into a credential forwarder for another origin.
                    send_extra_headers = false;
                }
                url = next;
                // Per convention a 303 (and historically 301/302) turns a
                // POST into a GET; 307/308 keep the method and body.
                if response.status == 303
                    || ((response.status == 301 || response.status == 302) && method == "POST")
                {
                    method = "GET".to_string();
                    body = None;
                }
                // Connection: close on every hop; just drop the old transport.
            }
            _ => return Ok(response),
        }
    }
    Err(AssetAiError::Http(format!(
        "too many redirects fetching {}",
        req.url
    )))
}

/// Executes exactly one HTTP request. Peer identity/blob requests use this:
/// their authorization is scoped to the selected source origin, and a peer
/// redirect is a failure rather than permission to forward the ticket.
pub fn http_fetch_no_redirect(
    req: &HttpClientRequest,
) -> Result<HttpClientResponse, AssetAiError> {
    let url = parse_url(req.url)?;
    fetch_once(&url, req.method, req, req.body, true)
}

fn same_origin(a: &ParsedUrl, b: &ParsedUrl) -> bool {
    a.https == b.https && a.port == b.port && a.host.eq_ignore_ascii_case(&b.host)
}

#[cfg(not(target_os = "windows"))]
fn fetch_once(
    url: &ParsedUrl,
    method: &str,
    req: &HttpClientRequest,
    body: Option<(&str, &[u8])>,
    send_extra_headers: bool,
) -> Result<HttpClientResponse, AssetAiError> {
    let mut transport = connect(url)?;
    write_request(
        &mut transport,
        url,
        method,
        req,
        body,
        send_extra_headers,
    )?;
    read_response_head(transport)
}

#[cfg(target_os = "windows")]
fn fetch_once(
    url: &ParsedUrl,
    method: &str,
    req: &HttpClientRequest,
    body: Option<(&str, &[u8])>,
    send_extra_headers: bool,
) -> Result<HttpClientResponse, AssetAiError> {
    winhttp_fetch_once(url, method, req, body, send_extra_headers)
}

#[cfg(not(target_os = "windows"))]
fn write_request(
    transport: &mut Transport,
    url: &ParsedUrl,
    method: &str,
    req: &HttpClientRequest,
    body: Option<(&str, &[u8])>,
    send_extra_headers: bool,
) -> Result<(), AssetAiError> {
    let mut head = String::new();
    head.push_str(&format!("{} {} HTTP/1.1\r\n", method, url.target));
    // Only include :port when non-default, matching what browsers/curl send.
    let default_port = if url.https { 443 } else { 80 };
    if url.port == default_port {
        head.push_str(&format!("Host: {}\r\n", url.host));
    } else {
        head.push_str(&format!("Host: {}:{}\r\n", url.host, url.port));
    }
    head.push_str(&format!(
        "User-Agent: {}/{}\r\n",
        crate::SERVICE_NAME,
        crate::SERVICE_VERSION
    ));
    head.push_str("Accept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n");
    if let Some(from) = req.range_from {
        match req.range_to {
            Some(to) if to >= from => head.push_str(&format!("Range: bytes={from}-{to}\r\n")),
            Some(to) => {
                return Err(AssetAiError::Http(format!(
                    "invalid byte range {from}-{to}"
                )))
            }
            None => head.push_str(&format!("Range: bytes={from}-\r\n")),
        }
    }
    if let Some(bearer) = &req.bearer {
        if host_matches_suffix(&url.host, bearer.host_suffix) {
            head.push_str(&format!("Authorization: Bearer {}\r\n", bearer.token));
        }
    }
    if send_extra_headers {
        for (name, value) in req.extra_headers {
            if name.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
                return Err(AssetAiError::Http(
                    "request header contains a newline".to_string(),
                ));
            }
            head.push_str(&format!("{name}: {value}\r\n"));
        }
    }
    if let Some((content_type, bytes)) = body {
        head.push_str(&format!(
            "Content-Type: {}\r\nContent-Length: {}\r\n",
            content_type,
            bytes.len()
        ));
    }
    head.push_str("\r\n");
    transport
        .write_all(head.as_bytes())
        .map_err(|e| AssetAiError::Http(format!("request write: {e}")))?;
    if let Some((_, bytes)) = body {
        transport
            .write_all(bytes)
            .map_err(|e| AssetAiError::Http(format!("request body write: {e}")))?;
    }
    transport
        .flush()
        .map_err(|e| AssetAiError::Http(format!("request flush: {e}")))?;
    Ok(())
}

fn resolve_redirect(current: &ParsedUrl, location: &str) -> Result<ParsedUrl, AssetAiError> {
    if location.starts_with("http://") || location.starts_with("https://") {
        parse_url(location)
    } else if location.starts_with('/') {
        Ok(ParsedUrl {
            target: location.to_string(),
            ..current.clone()
        })
    } else {
        Err(AssetAiError::Http(format!(
            "unsupported relative redirect: {location}"
        )))
    }
}

#[cfg(any(not(target_os = "windows"), test))]
fn read_response_head(mut transport: Transport) -> Result<HttpClientResponse, AssetAiError> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(pos) = find_head_end(&buf) {
            break pos;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(AssetAiError::Http("response head too large".into()));
        }
        let n = transport
            .read(&mut chunk)
            .map_err(|e| AssetAiError::Http(format!("response read: {e}")))?;
        if n == 0 {
            return Err(AssetAiError::Http("connection closed in headers".into()));
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let body_prefix = buf[head_end..].to_vec();

    let mut lines = head.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| AssetAiError::Http("no status line".into()))?;
    // "HTTP/1.1 200 OK"
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| AssetAiError::Http(format!("bad status line: {status_line}")))?;
    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some(pos) = line.find(':') {
            headers.push((
                line[..pos].trim().to_ascii_lowercase(),
                line[pos + 1..].trim().to_string(),
            ));
        }
    }

    let mode = body_mode(status, &headers)?;
    Ok(HttpClientResponse {
        status,
        headers,
        body: BodyReader {
            transport,
            prefix: body_prefix,
            prefix_pos: 0,
            mode,
        },
    })
}

#[cfg(any(not(target_os = "windows"), test))]
fn find_head_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
}

fn body_mode(status: u16, headers: &[(String, String)]) -> Result<BodyMode, AssetAiError> {
    if status == 204 || status == 304 || (100..200).contains(&status) {
        return Ok(BodyMode::Sized { remaining: 0 });
    }
    let transfer_encoding = headers
        .iter()
        .find(|(k, _)| k == "transfer-encoding")
        .map(|(_, v)| v.to_ascii_lowercase());
    if let Some(te) = transfer_encoding {
        if te.contains("chunked") {
            return Ok(BodyMode::Chunked {
                remaining_in_chunk: 0,
                finished: false,
            });
        }
        return Err(AssetAiError::Http(format!(
            "unsupported transfer-encoding: {te}"
        )));
    }
    let content_length = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.trim().parse::<u64>().ok());
    match content_length {
        Some(len) => Ok(BodyMode::Sized { remaining: len }),
        // Connection: close delimits the body.
        None => Ok(BodyMode::UntilClose),
    }
}

// ---------------------------------------------------------------------------
// Body reader: content-length, chunked, or read-until-close
// ---------------------------------------------------------------------------

enum BodyMode {
    Sized { remaining: u64 },
    Chunked { remaining_in_chunk: u64, finished: bool },
    UntilClose,
}

pub struct BodyReader {
    transport: Transport,
    prefix: Vec<u8>,
    prefix_pos: usize,
    mode: BodyMode,
}

impl BodyReader {
    /// Reads from the leftover head bytes first, then the socket.
    fn read_raw(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.prefix_pos < self.prefix.len() {
            let take = (self.prefix.len() - self.prefix_pos).min(buf.len());
            buf[..take].copy_from_slice(&self.prefix[self.prefix_pos..self.prefix_pos + take]);
            self.prefix_pos += take;
            return Ok(take);
        }
        self.transport.read(buf)
    }

    fn read_raw_exact(&mut self, buf: &mut [u8]) -> std::io::Result<()> {
        let mut pos = 0;
        while pos < buf.len() {
            let n = self.read_raw(&mut buf[pos..])?;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "connection closed mid body",
                ));
            }
            pos += n;
        }
        Ok(())
    }

    /// Reads a CRLF-terminated line byte by byte (chunk size lines only, so
    /// the byte-at-a-time cost is irrelevant).
    fn read_raw_line(&mut self) -> std::io::Result<String> {
        let mut line = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            self.read_raw_exact(&mut byte)?;
            if byte[0] == b'\n' {
                if line.last() == Some(&b'\r') {
                    line.pop();
                }
                return Ok(String::from_utf8_lossy(&line).to_string());
            }
            line.push(byte[0]);
            if line.len() > 1024 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "chunk header line too long",
                ));
            }
        }
    }
}

impl Read for BodyReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        match &mut self.mode {
            BodyMode::Sized { remaining } => {
                if *remaining == 0 {
                    return Ok(0);
                }
                let take = (*remaining).min(buf.len() as u64) as usize;
                let n = self.read_raw(&mut buf[..take])?;
                if n == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "connection closed before content-length was satisfied",
                    ));
                }
                if let BodyMode::Sized { remaining } = &mut self.mode {
                    *remaining -= n as u64;
                }
                Ok(n)
            }
            BodyMode::UntilClose => self.read_raw(buf),
            BodyMode::Chunked {
                remaining_in_chunk,
                finished,
            } => {
                if *finished {
                    return Ok(0);
                }
                if *remaining_in_chunk == 0 {
                    // Next chunk header: "<hex-size>[;ext]\r\n".
                    let line = self.read_raw_line()?;
                    let size_part = line.split(';').next().unwrap_or("").trim();
                    let size = u64::from_str_radix(size_part, 16).map_err(|_| {
                        std::io::Error::new(
                            std::io::ErrorKind::InvalidData,
                            format!("bad chunk size: {line:?}"),
                        )
                    })?;
                    if size == 0 {
                        // Trailer section: lines until an empty one.
                        loop {
                            let trailer = self.read_raw_line()?;
                            if trailer.is_empty() {
                                break;
                            }
                        }
                        if let BodyMode::Chunked { finished, .. } = &mut self.mode {
                            *finished = true;
                        }
                        return Ok(0);
                    }
                    if let BodyMode::Chunked {
                        remaining_in_chunk, ..
                    } = &mut self.mode
                    {
                        *remaining_in_chunk = size;
                    }
                }
                let remaining = match &self.mode {
                    BodyMode::Chunked {
                        remaining_in_chunk, ..
                    } => *remaining_in_chunk,
                    _ => unreachable!(),
                };
                let take = remaining.min(buf.len() as u64) as usize;
                let n = self.read_raw(&mut buf[..take])?;
                if n == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "connection closed mid chunk",
                    ));
                }
                let mut chunk_done = false;
                if let BodyMode::Chunked {
                    remaining_in_chunk, ..
                } = &mut self.mode
                {
                    *remaining_in_chunk -= n as u64;
                    chunk_done = *remaining_in_chunk == 0;
                }
                if chunk_done {
                    // Consume the CRLF that terminates the chunk data.
                    let mut crlf = [0u8; 2];
                    self.read_raw_exact(&mut crlf)?;
                }
                Ok(n)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_urls() {
        assert_eq!(
            parse_url("https://huggingface.co/black-forest-labs/FLUX.1-schnell/resolve/main/ae.safetensors").unwrap(),
            ParsedUrl {
                https: true,
                host: "huggingface.co".into(),
                port: 443,
                target: "/black-forest-labs/FLUX.1-schnell/resolve/main/ae.safetensors".into(),
            }
        );
        assert_eq!(
            parse_url("http://127.0.0.1:8765/health").unwrap(),
            ParsedUrl {
                https: false,
                host: "127.0.0.1".into(),
                port: 8765,
                target: "/health".into(),
            }
        );
        assert_eq!(parse_url("http://example.com").unwrap().target, "/");
        assert!(parse_url("ftp://example.com").is_err());
        assert!(parse_url("http:///nohost").is_err());
    }

    #[test]
    fn bearer_host_suffix() {
        assert!(host_matches_suffix("huggingface.co", "huggingface.co"));
        assert!(host_matches_suffix("cdn-lfs.huggingface.co", "huggingface.co"));
        assert!(!host_matches_suffix("evilhuggingface.co", "huggingface.co"));
        assert!(!host_matches_suffix("cdn-lfs.hf.co", "huggingface.co"));
    }

    fn response_from_bytes(raw: &[u8]) -> HttpClientResponse {
        let transport = Transport::Mem(std::io::Cursor::new(raw.to_vec()));
        read_response_head(transport).unwrap()
    }

    #[test]
    fn sized_body() {
        let response = response_from_bytes(
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Type: text/plain\r\n\r\nhellotrailing-garbage",
        );
        assert_eq!(response.status, 200);
        assert_eq!(response.content_length(), Some(5));
        let body = response.read_body_to_vec(1024).unwrap();
        assert_eq!(body, b"hello");
    }

    #[test]
    fn chunked_body() {
        let response = response_from_bytes(
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4\r\nWiki\r\n5\r\npedia\r\nE\r\n in\r\n\r\nchunks.\r\n0\r\n\r\n",
        );
        let body = response.read_body_to_vec(1024).unwrap();
        assert_eq!(body, b"Wikipedia in\r\n\r\nchunks.");
    }

    #[test]
    fn content_range_total() {
        let response = response_from_bytes(
            b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 100-499/500\r\nContent-Length: 400\r\n\r\n",
        );
        assert_eq!(response.content_range_total(), Some(500));
    }
}
