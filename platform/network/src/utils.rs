use std::io::prelude::*;
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

#[cfg(feature = "script")]
use makepad_script::*;
#[cfg(feature = "script")]
use std::net::{IpAddr, Ipv4Addr};

pub const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(30);

const LOW_LEVEL_SECURITY_HEADERS: &str = "Cross-Origin-Opener-Policy: same-origin\r\n\
Cross-Origin-Embedder-Policy: require-corp\r\n";

pub fn write_bytes_to_tcp_stream_no_error(tcp_stream: &mut TcpStream, bytes: &[u8]) -> bool {
    let bytes_total = bytes.len();
    let mut bytes_left = bytes_total;
    while bytes_left > 0 {
        let buf = &bytes[(bytes_total - bytes_left)..bytes_total];
        if let Ok(bytes_written) = tcp_stream.write(buf) {
            if bytes_written == 0 {
                return true;
            }
            bytes_left -= bytes_written;
        } else {
            return true;
        }
    }
    false
}

fn status_reason(code: u16) -> &'static str {
    match code {
        400 => "Bad Request",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        411 => "Length Required",
        413 => "Content Too Large",
        431 => "Request Header Fields Too Large",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Error",
    }
}

pub fn http_error_out(mut tcp_stream: TcpStream, code: u16) {
    let allow = if code == 405 {
        "Allow: GET, HEAD, POST, OPTIONS\r\n"
    } else {
        ""
    };
    write_bytes_to_tcp_stream_no_error(
        &mut tcp_stream,
        format!(
            "HTTP/1.1 {code} {}\r\n{LOW_LEVEL_SECURITY_HEADERS}{allow}Content-Length: 0\r\nConnection: close\r\n\r\n",
            status_reason(code)
        )
        .as_bytes(),
    );
    let _ = tcp_stream.shutdown(Shutdown::Both);
}

pub fn split_header_line<'a>(inp: &'a str, what: &str) -> Option<&'a str> {
    let line = inp.strip_suffix("\r\n").unwrap_or(inp);
    let (name, value) = line.split_once(':')?;
    name.eq_ignore_ascii_case(what.trim_end_matches([':', ' ']))
        .then(|| value.trim())
}

/// Parses a request-line remainder (`target HTTP/version`) without searching
/// past the target for `?`. Kept public for compatibility with older users.
pub fn parse_url_path(url: &str, append_index_html: bool) -> Option<(String, Option<String>)> {
    let mut fields = url.split_ascii_whitespace();
    let target = fields.next()?;
    let version = fields.next()?;
    if fields.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return None;
    }
    parse_origin_target(target, append_index_html)
}

fn parse_origin_target(target: &str, append_index_html: bool) -> Option<(String, Option<String>)> {
    if !target.starts_with('/')
        || target.starts_with("//")
        || target.contains('#')
        || target.bytes().any(|byte| byte.is_ascii_control() || byte == b' ')
    {
        return None;
    }
    let (path, search) = match target.split_once('?') {
        Some((path, search)) => (path, Some(search.to_string())),
        None => (target, None),
    };
    if path.is_empty() {
        return None;
    }
    let mut path = path.to_string();
    if append_index_html && path.ends_with('/') {
        path.push_str("index.html");
    }
    Some((path, search))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HttpHeadError {
    BadRequest,
    Timeout,
    TooLarge,
}

impl HttpHeadError {
    pub fn status(self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::Timeout => 408,
            Self::TooLarge => 431,
        }
    }
}

#[cfg_attr(feature = "script", derive(Script, ScriptHook))]
#[derive(Clone, Debug)]
pub struct HttpServerHeaders {
    #[cfg_attr(
        feature = "script",
        rust(SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 8080))
    )]
    pub addr: SocketAddr,
    #[cfg_attr(feature = "script", live)]
    pub addr_text: String,
    #[cfg_attr(feature = "script", live)]
    pub lines: Vec<String>,
    #[cfg_attr(feature = "script", rust(Vec::new()))]
    pub parsed_headers: Vec<(String, String)>,
    #[cfg_attr(feature = "script", live)]
    pub verb: String,
    #[cfg_attr(feature = "script", live)]
    pub path: String,
    #[cfg_attr(feature = "script", live)]
    pub path_no_slash: String,
    #[cfg_attr(feature = "script", live)]
    pub search: Option<String>,
    #[cfg_attr(feature = "script", live)]
    pub content_length: Option<u64>,
    #[cfg_attr(feature = "script", live)]
    pub accept_encoding: Option<String>,
    #[cfg_attr(feature = "script", live)]
    pub sec_websocket_key: Option<String>,
}

const MAX_HEAD_BYTES: usize = 1024 * 1024;

fn find_head_end(buf: &[u8], search_from: usize) -> Option<usize> {
    let start = search_from.saturating_sub(3);
    buf[start..]
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|p| start + p + 4)
}

fn set_remaining_read_timeout(stream: &TcpStream, deadline: Instant) -> Result<(), HttpHeadError> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .filter(|duration| !duration.is_zero())
        .ok_or(HttpHeadError::Timeout)?;
    stream
        .set_read_timeout(Some(remaining))
        .map_err(|_| HttpHeadError::BadRequest)
}

impl HttpServerHeaders {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.parsed_headers
            .iter()
            .find_map(|(key, value)| key.eq_ignore_ascii_case(name).then_some(value.as_str()))
    }

    pub fn from_tcp_stream(tcp_stream: &mut TcpStream) -> Option<(HttpServerHeaders, Vec<u8>)> {
        Self::from_tcp_stream_until(tcp_stream, Instant::now() + HTTP_READ_TIMEOUT).ok()
    }

    pub fn from_tcp_stream_until(
        tcp_stream: &mut TcpStream,
        deadline: Instant,
    ) -> Result<(HttpServerHeaders, Vec<u8>), HttpHeadError> {
        let addr = tcp_stream.peer_addr().map_err(|_| HttpHeadError::BadRequest)?;
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        let mut searched = 0;
        let head_end = loop {
            if let Some(end) = find_head_end(&buf, searched) {
                break end;
            }
            if buf.len() >= MAX_HEAD_BYTES {
                return Err(HttpHeadError::TooLarge);
            }
            searched = buf.len();
            set_remaining_read_timeout(tcp_stream, deadline)?;
            match tcp_stream.read(&mut chunk) {
                Ok(0) => return Err(HttpHeadError::BadRequest),
                Ok(n) => {
                    buf.extend_from_slice(&chunk[..n]);
                    if buf.len() > MAX_HEAD_BYTES {
                        return Err(HttpHeadError::TooLarge);
                    }
                }
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
                    ) =>
                {
                    return Err(HttpHeadError::Timeout)
                }
                Err(_) => return Err(HttpHeadError::BadRequest),
            }
        };

        let head = std::str::from_utf8(&buf[..head_end - 2])
            .map_err(|_| HttpHeadError::BadRequest)?;
        let body_prefix = buf[head_end..].to_vec();
        let mut raw_lines = head.split_inclusive("\r\n");
        let request_line = raw_lines.next().ok_or(HttpHeadError::BadRequest)?;
        let request_line = request_line
            .strip_suffix("\r\n")
            .ok_or(HttpHeadError::BadRequest)?;
        if request_line.len() > 4096 {
            return Err(HttpHeadError::TooLarge);
        }
        let mut fields = request_line.split_ascii_whitespace();
        let verb = fields.next().ok_or(HttpHeadError::BadRequest)?;
        let target = fields.next().ok_or(HttpHeadError::BadRequest)?;
        let version = fields.next().ok_or(HttpHeadError::BadRequest)?;
        if fields.next().is_some()
            || !matches!(version, "HTTP/1.0" | "HTTP/1.1")
            || !matches!(verb, "GET" | "HEAD" | "OPTIONS" | "POST" | "PUT" | "DELETE")
        {
            return Err(HttpHeadError::BadRequest);
        }

        let mut lines = vec![format!("{request_line}\r\n")];
        let mut content_length = None;
        let mut accept_encoding = None;
        let mut sec_websocket_key = None;
        let mut saw_transfer_encoding = false;
        let mut parsed_headers = Vec::new();
        for raw in raw_lines {
            if raw == "\r\n" {
                continue;
            }
            if raw.len() > 4096 || lines.len() >= 4096 {
                return Err(HttpHeadError::TooLarge);
            }
            let line = raw.strip_suffix("\r\n").ok_or(HttpHeadError::BadRequest)?;
            if line.starts_with([' ', '\t']) {
                return Err(HttpHeadError::BadRequest);
            }
            let (name, value) = line.split_once(':').ok_or(HttpHeadError::BadRequest)?;
            if name.is_empty()
                || !name
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte))
                || value.bytes().any(|byte| byte == 0 || byte == b'\r' || byte == b'\n')
            {
                return Err(HttpHeadError::BadRequest);
            }
            let value = value.trim_matches([' ', '\t']);
            if name.eq_ignore_ascii_case("content-length") {
                if content_length.is_some()
                    || value.is_empty()
                    || !value.bytes().all(|byte| byte.is_ascii_digit())
                {
                    return Err(HttpHeadError::BadRequest);
                }
                content_length = Some(value.parse().map_err(|_| HttpHeadError::BadRequest)?);
            } else if name.eq_ignore_ascii_case("transfer-encoding") {
                saw_transfer_encoding = true;
            } else if name.eq_ignore_ascii_case("accept-encoding") {
                if accept_encoding.is_none() {
                    accept_encoding = Some(value.to_string());
                }
            } else if name.eq_ignore_ascii_case("sec-websocket-key") {
                if sec_websocket_key.is_some() {
                    return Err(HttpHeadError::BadRequest);
                }
                sec_websocket_key = Some(value.to_string());
            }
            parsed_headers.push((name.to_ascii_lowercase(), value.to_string()));
            lines.push(raw.to_string());
        }
        if saw_transfer_encoding {
            return Err(HttpHeadError::BadRequest);
        }

        let path = parse_origin_target(target, sec_websocket_key.is_none())
            .ok_or(HttpHeadError::BadRequest)?;
        let path_no_slash = path
            .0
            .strip_prefix('/')
            .ok_or(HttpHeadError::BadRequest)?
            .to_string();
        Ok((
            HttpServerHeaders {
                addr,
                addr_text: addr.to_string(),
                parsed_headers,
                verb: verb.to_string(),
                path_no_slash,
                path: path.0,
                search: path.1,
                lines,
                content_length,
                accept_encoding,
                sec_websocket_key,
            },
            body_prefix,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_url_path, HttpHeadError, HttpServerHeaders};
    use std::io::Write;
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    fn parse_head(request: &[u8]) -> Result<(HttpServerHeaders, Vec<u8>), HttpHeadError> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let bytes = request.to_vec();
        let client = std::thread::spawn(move || {
            let mut stream = TcpStream::connect(addr).unwrap();
            stream.write_all(&bytes).unwrap();
        });
        let (mut server, _) = listener.accept().unwrap();
        let result = HttpServerHeaders::from_tcp_stream_until(
            &mut server,
            Instant::now() + Duration::from_secs(2),
        );
        client.join().unwrap();
        result
    }

    #[test]
    fn body_sent_with_the_headers_is_returned_not_swallowed() {
        let (headers, body) = parse_head(
            b"POST /pair HTTP/1.1\r\nHost: x\r\nContent-Length: 5\r\n\r\nhello",
        )
        .unwrap();
        assert_eq!(headers.lines.len(), 3);
        assert_eq!(body, b"hello");
    }

    #[test]
    fn strict_request_line_and_origin_form() {
        for bad in [
            &b"GET /x HTTP/1.1?q\r\nHost: x\r\n\r\n"[..],
            &b"GET  HTTP/1.1\r\nHost: x\r\n\r\n"[..],
            &b"GET https://example.test/x HTTP/1.1\r\nHost: x\r\n\r\n"[..],
            &b"GET //example.test/x HTTP/1.1\r\nHost: x\r\n\r\n"[..],
            &b"GET /x HTTP/1.1 extra\r\nHost: x\r\n\r\n"[..],
        ] {
            assert_eq!(parse_head(bad).unwrap_err(), HttpHeadError::BadRequest);
        }
    }

    #[test]
    fn duplicate_length_and_transfer_encoding_are_rejected() {
        assert_eq!(
            parse_head(b"POST /x HTTP/1.1\r\nContent-Length:5\r\nContent-Length: 5\r\n\r\n")
                .unwrap_err(),
            HttpHeadError::BadRequest
        );
        assert_eq!(
            parse_head(b"POST /x HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n")
                .unwrap_err(),
            HttpHeadError::BadRequest
        );
    }

    #[test]
    fn appends_index_and_splits_query_inside_target_only() {
        assert_eq!(
            parse_url_path("/ui/?x=1 HTTP/1.1\r\n", true),
            Some(("/ui/index.html".to_string(), Some("x=1".to_string())))
        );
        assert_eq!(parse_url_path("/x HTTP/1.1?q", true), None);
    }
}
