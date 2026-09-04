//! Small, blocking HTTP/1.1 transport for the two flow-server planes.
//!
//! Responses are accepted only when an exact `Content-Length` frames them,
//! except for an inherently bodyless 204. Transfer encodings, informational
//! responses, and redirects are protocol errors. Each instance owns at most
//! one idle keep-alive connection.

use super::client::{ClientError, ClientResult};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const MAX_HEAD_BYTES: usize = 32 * 1024;
const MAX_HEADER_LINE_BYTES: usize = 8 * 1024;
const MAX_HEADERS: usize = 64;

#[derive(Clone, Copy, Debug)]
pub(crate) struct HttpLimits {
    pub connect: Duration,
    pub io: Duration,
    pub head: Duration,
    pub body: Duration,
}

impl HttpLimits {
    pub(crate) fn default_v1() -> Self {
        Self {
            connect: Duration::from_secs(5),
            io: Duration::from_secs(10),
            head: Duration::from_secs(10),
            body: Duration::from_secs(60),
        }
    }

    pub(crate) fn probe() -> Self {
        let short = Duration::from_millis(400);
        Self {
            connect: short,
            io: short,
            head: short,
            body: short,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Method {
    Get,
    Put,
    Post,
    Delete,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Post => "POST",
            Self::Delete => "DELETE",
        }
    }
}

#[derive(Debug)]
pub(crate) struct Response {
    pub status: u16,
    pub body: Vec<u8>,
    /// The `Content-Type` header, when the server sent one (the data plane
    /// types a value's bytes with it).
    pub content_type: Option<String>,
}

#[derive(Debug)]
pub(crate) struct HttpClient {
    addr: SocketAddr,
    body_cap: usize,
    limits: HttpLimits,
    idle: Mutex<Option<TcpStream>>,
}

impl HttpClient {
    pub(crate) fn new(addr: SocketAddr, body_cap: usize) -> Self {
        Self::with_limits(addr, body_cap, HttpLimits::default_v1())
    }

    pub(crate) fn with_limits(
        addr: SocketAddr,
        body_cap: usize,
        limits: HttpLimits,
    ) -> Self {
        Self {
            addr,
            body_cap,
            limits,
            idle: Mutex::new(None),
        }
    }

    pub(crate) fn call(
        &self,
        method: Method,
        target: &str,
        bearer: Option<&str>,
        body: Option<&[u8]>,
        head_deadline: Option<Duration>,
    ) -> ClientResult<Response> {
        validate_target(target)?;
        if body.is_some_and(|bytes| bytes.len() > self.body_cap) {
            return Err(ClientError::Protocol(format!(
                "request body exceeds {} bytes",
                self.body_cap
            )));
        }
        if bearer.is_some_and(|value| contains_line_break(value)) {
            return Err(ClientError::Protocol("bearer token contains a line break".into()));
        }

        let mut guard = self
            .idle
            .lock()
            .map_err(|_| ClientError::Protocol("HTTP connection lock poisoned".into()))?;
        let (mut stream, reused) = match guard.take() {
            Some(stream) => (stream, true),
            None => (self.connect()?, false),
        };
        let head_budget = head_deadline.unwrap_or(self.limits.head);

        let mut stale = false;
        let mut result =
            self.call_on(&mut stream, method, target, bearer, body, head_budget, &mut stale);
        if reused && stale {
            // The server had already closed its idle keep-alive side, so the
            // request never reached it (nothing of a response came back):
            // replay it once on a fresh connection. A fresh connection that
            // dies the same way is a real failure and is reported as one.
            stream = self.connect()?;
            result =
                self.call_on(&mut stream, method, target, bearer, body, head_budget, &mut stale);
        }
        match result {
            Ok((response, reusable)) => {
                if reusable {
                    *guard = Some(stream);
                }
                Ok(response)
            }
            Err(error) => Err(error),
        }
    }

    fn connect(&self) -> ClientResult<TcpStream> {
        let stream = TcpStream::connect_timeout(&self.addr, self.limits.connect)
            .map_err(|error| io_error("HTTP connect", error))?;
        stream
            .set_nodelay(true)
            .map_err(|error| io_error("HTTP set nodelay", error))?;
        stream
            .set_write_timeout(Some(self.limits.io))
            .map_err(|error| io_error("HTTP set write timeout", error))?;
        Ok(stream)
    }

    fn call_on(
        &self,
        stream: &mut TcpStream,
        method: Method,
        target: &str,
        bearer: Option<&str>,
        body: Option<&[u8]>,
        head_budget: Duration,
        stale: &mut bool,
    ) -> ClientResult<(Response, bool)> {
        let mut request = Vec::with_capacity(256 + body.map_or(0, <[u8]>::len));
        write!(&mut request, "{} {} HTTP/1.1\r\n", method.as_str(), target)
            .map_err(|_| ClientError::Protocol("could not construct HTTP request".into()))?;
        write!(&mut request, "Host: {}\r\n", self.addr)
            .map_err(|_| ClientError::Protocol("could not construct HTTP request".into()))?;
        request.extend_from_slice(b"Accept: application/json\r\nAccept-Encoding: identity\r\n");
        if let Some(token) = bearer {
            request.extend_from_slice(b"Authorization: Bearer ");
            request.extend_from_slice(token.as_bytes());
            request.extend_from_slice(b"\r\n");
        }
        if let Some(bytes) = body {
            write!(
                &mut request,
                "Content-Type: application/json\r\nContent-Length: {}\r\n",
                bytes.len()
            )
            .map_err(|_| ClientError::Protocol("could not construct HTTP request".into()))?;
        }
        request.extend_from_slice(b"Connection: keep-alive\r\n\r\n");
        if let Some(bytes) = body {
            request.extend_from_slice(bytes);
        }
        if let Err(error) = stream.write_all(&request).and_then(|()| stream.flush()) {
            // A peer that closed first refuses the bytes outright: none of
            // the request was read, so the caller may replay it.
            *stale = peer_closed(&error);
            return Err(io_error("HTTP request write", error));
        }

        let head_started = Instant::now();
        let mut bytes = Vec::with_capacity(4096);
        let head_end = loop {
            if let Some(index) = find_head_end(&bytes) {
                break index;
            }
            if bytes.len() >= MAX_HEAD_BYTES {
                return Err(ClientError::Protocol("response head too large".into()));
            }
            let left = head_budget
                .checked_sub(head_started.elapsed())
                .ok_or_else(|| ClientError::Timeout("HTTP response head".into()))?;
            stream
                .set_read_timeout(Some(left.min(self.limits.io)))
                .map_err(|error| io_error("HTTP set head timeout", error))?;
            let mut chunk = [0u8; 4096];
            match stream.read(&mut chunk) {
                Ok(0) => {
                    // Closed before a single response byte: the request was
                    // never answered, so a reused connection was stale.
                    *stale = bytes.is_empty();
                    return Err(ClientError::Io {
                        op: "HTTP response head",
                        kind: std::io::ErrorKind::UnexpectedEof,
                    });
                }
                Ok(count) => bytes.extend_from_slice(&chunk[..count]),
                Err(error) if retryable_timeout(&error) => {
                    if head_started.elapsed() >= head_budget {
                        return Err(ClientError::Timeout("HTTP response head".into()));
                    }
                }
                Err(error) => {
                    *stale = bytes.is_empty() && peer_closed(&error);
                    return Err(io_error("HTTP response head", error));
                }
            }
        };

        let parsed = parse_head(&bytes[..head_end])?;
        if parsed.content_length > self.body_cap as u64 {
            return Err(ClientError::Protocol(format!(
                "response body exceeds {} bytes",
                self.body_cap
            )));
        }
        let mut body_bytes = bytes[(head_end + 4)..].to_vec();
        if body_bytes.len() as u64 > parsed.content_length {
            return Err(ClientError::Protocol("bytes past declared response body".into()));
        }
        let body_started = Instant::now();
        while body_bytes.len() as u64 != parsed.content_length {
            let left = self
                .limits
                .body
                .checked_sub(body_started.elapsed())
                .ok_or_else(|| ClientError::Timeout("HTTP response body".into()))?;
            stream
                .set_read_timeout(Some(left.min(self.limits.io)))
                .map_err(|error| io_error("HTTP set body timeout", error))?;
            let remaining = parsed.content_length as usize - body_bytes.len();
            let mut chunk = [0u8; 16 * 1024];
            let count = remaining.min(chunk.len());
            match stream.read(&mut chunk[..count]) {
                Ok(0) => {
                    return Err(ClientError::Io {
                        op: "HTTP response body",
                        kind: std::io::ErrorKind::UnexpectedEof,
                    })
                }
                Ok(count) => body_bytes.extend_from_slice(&chunk[..count]),
                Err(error) if retryable_timeout(&error) => {
                    if body_started.elapsed() >= self.limits.body {
                        return Err(ClientError::Timeout("HTTP response body".into()));
                    }
                }
                Err(error) => return Err(io_error("HTTP response body", error)),
            }
        }
        Ok((
            Response {
                status: parsed.status,
                body: body_bytes,
                content_type: parsed.content_type,
            },
            !parsed.close,
        ))
    }
}

struct ParsedHead {
    status: u16,
    content_length: u64,
    close: bool,
    content_type: Option<String>,
}

fn parse_head(block: &[u8]) -> ClientResult<ParsedHead> {
    let text = std::str::from_utf8(block)
        .map_err(|_| ClientError::Protocol("response head is not ASCII".into()))?;
    let mut lines = text.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| ClientError::Protocol("empty HTTP response".into()))?;
    let mut status_parts = status_line.splitn(3, ' ');
    if status_parts.next() != Some("HTTP/1.1") {
        return Err(ClientError::Protocol("HTTP/1.1 response required".into()));
    }
    let status_text = status_parts
        .next()
        .ok_or_else(|| ClientError::Protocol("malformed HTTP status".into()))?;
    if status_text.len() != 3 || !status_text.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ClientError::Protocol("malformed HTTP status".into()));
    }
    let status = status_text
        .parse::<u16>()
        .map_err(|_| ClientError::Protocol("malformed HTTP status".into()))?;
    if (100..200).contains(&status) {
        return Err(ClientError::Protocol("informational HTTP response refused".into()));
    }
    if (300..400).contains(&status) {
        return Err(ClientError::Protocol("HTTP redirect refused".into()));
    }

    let mut content_length = None;
    let mut content_type = None;
    let mut close = false;
    let mut count = 0usize;
    for line in lines {
        count += 1;
        if count > MAX_HEADERS || line.len() > MAX_HEADER_LINE_BYTES {
            return Err(ClientError::Protocol("response headers exceed limits".into()));
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| ClientError::Protocol("malformed response header".into()))?;
        if name.is_empty() || !name.bytes().all(header_name_byte) {
            return Err(ClientError::Protocol("malformed response header name".into()));
        }
        let value = value.trim();
        if contains_line_break(value) {
            return Err(ClientError::Protocol("malformed response header value".into()));
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(ClientError::Protocol("Transfer-Encoding refused".into()));
        }
        if name.eq_ignore_ascii_case("content-length") {
            if content_length.is_some() {
                return Err(ClientError::Protocol("duplicate Content-Length".into()));
            }
            content_length = Some(
                value
                    .parse::<u64>()
                    .map_err(|_| ClientError::Protocol("malformed Content-Length".into()))?,
            );
        }
        if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value.to_string());
        }
        if name.eq_ignore_ascii_case("connection") {
            if value.eq_ignore_ascii_case("close") {
                close = true;
            } else if !value.eq_ignore_ascii_case("keep-alive") {
                return Err(ClientError::Protocol("unsupported Connection header".into()));
            }
        }
    }
    let content_length = match content_length {
        Some(content_length) => content_length,
        None if status == 204 => 0,
        None => {
            return Err(ClientError::Protocol(
                "Content-Length response required".into(),
            ))
        }
    };
    if status == 204 && content_length != 0 {
        return Err(ClientError::Protocol("204 response declared a body".into()));
    }
    Ok(ParsedHead {
        status,
        content_length,
        close,
        content_type,
    })
}

fn validate_target(target: &str) -> ClientResult<()> {
    if target.is_empty()
        || !target.starts_with('/')
        || target.len() > 4096
        || !target
            .bytes()
            .all(|byte| (0x21..=0x7e).contains(&byte) && byte != b'\\')
    {
        return Err(ClientError::Protocol("invalid HTTP request target".into()));
    }
    Ok(())
}

fn find_head_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

/// The peer had closed the connection before this side used it.
fn peer_closed(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::ConnectionAborted
    )
}

fn retryable_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::Interrupted
    )
}

fn io_error(op: &'static str, error: std::io::Error) -> ClientError {
    if matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
    ) {
        ClientError::Timeout(op.into())
    } else {
        ClientError::Io {
            op,
            kind: error.kind(),
        }
    }
}

fn contains_line_break(value: &str) -> bool {
    value.contains(['\r', '\n'])
}

fn header_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&byte)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_head_refuses_non_length_framing_and_redirects() {
        assert!(parse_head(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked").is_err());
        assert!(parse_head(b"HTTP/1.1 302 Found\r\nContent-Length: 0").is_err());
        assert!(parse_head(b"HTTP/1.1 100 Continue\r\nContent-Length: 0").is_err());
        assert!(parse_head(b"HTTP/1.1 200 OK\r\nConnection: close").is_err());
    }

    #[test]
    fn response_head_accepts_exact_content_length() {
        let head = parse_head(b"HTTP/1.1 200 OK\r\nContent-Length: 12").unwrap();
        assert_eq!(head.status, 200);
        assert_eq!(head.content_length, 12);
        assert!(!head.close);
    }

    /// Serve `connections` connections; each answers one request with
    /// `ok` and then closes, the way a server drops an idle keep-alive.
    /// Every close is reported on the returned channel.
    fn one_request_per_connection_server(
        connections: usize,
        answer: bool,
    ) -> (SocketAddr, std::sync::mpsc::Receiver<()>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let (closed_tx, closed_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for stream in listener.incoming().take(connections) {
                let mut stream = stream.unwrap();
                if answer {
                    let mut head = Vec::new();
                    let mut byte = [0u8; 1];
                    while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap() == 1 {
                        head.push(byte[0]);
                    }
                    stream
                        .write_all(
                            b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: keep-alive\r\n\r\nok",
                        )
                        .unwrap();
                }
                drop(stream);
                closed_tx.send(()).unwrap();
            }
        });
        (addr, closed_rx)
    }

    #[test]
    fn a_kept_alive_connection_the_server_closed_is_replayed_on_a_fresh_one() {
        let (addr, closed) = one_request_per_connection_server(2, true);
        let client = HttpClient::new(addr, 1024);
        let first = client.call(Method::Get, "/a", None, None, None).unwrap();
        assert_eq!(first.body, b"ok");
        closed.recv().unwrap();
        // The idle socket is half-closed by now: the call must land on a
        // second connection instead of surfacing UnexpectedEof.
        let second = client.call(Method::Get, "/b", None, None, None).unwrap();
        assert_eq!(second.body, b"ok");
        closed.recv().unwrap();
    }

    #[test]
    fn a_fresh_connection_closed_without_a_response_is_an_error_not_a_retry() {
        let (addr, closed) = one_request_per_connection_server(2, false);
        let client = HttpClient::new(addr, 1024);
        let error = client.call(Method::Get, "/a", None, None, None).unwrap_err();
        assert!(matches!(error, ClientError::Io { .. }), "{error:?}");
        closed.recv().unwrap();
        // No second connection was opened for the replay.
        assert!(closed.recv_timeout(Duration::from_millis(200)).is_err());
    }
}
