use super::socket_stream::SocketStream;

use crate::types::{HttpError, HttpProgress, HttpRequest, HttpResponse, NetworkResponse};
use makepad_live_id::LiveId;
use std::{
    collections::HashMap,
    io,
    io::{Read, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};

pub struct LinuxHttpSocket;

impl LinuxHttpSocket {
    pub fn open(
        request_id: LiveId,
        request: HttpRequest,
        response_sender: Sender<NetworkResponse>,
    ) {
        let cancel_flag = Arc::new(AtomicBool::new(false));
        cancellation_map()
            .lock()
            .unwrap()
            .insert(request_id, cancel_flag.clone());

        std::thread::spawn(move || {
            let metadata_id = request.metadata_id;
            let result = run_http_request(request_id, &request, &response_sender, &cancel_flag);

            cancellation_map().lock().unwrap().remove(&request_id);

            if let Err(err) = result {
                let _ = response_sender.send(NetworkResponse::HttpError {
                    request_id,
                    error: HttpError {
                        message: err,
                        metadata_id,
                    },
                });
            }
        });
    }

    pub fn cancel(request_id: LiveId) {
        if let Some(flag) = cancellation_map().lock().unwrap().get(&request_id) {
            flag.store(true, Ordering::SeqCst);
        }
    }
}

fn cancellation_map() -> &'static Mutex<HashMap<LiveId, Arc<AtomicBool>>> {
    static MAP: OnceLock<Mutex<HashMap<LiveId, Arc<AtomicBool>>>> = OnceLock::new();
    MAP.get_or_init(|| Mutex::new(HashMap::new()))
}

fn run_http_request(
    request_id: LiveId,
    request: &HttpRequest,
    response_sender: &Sender<NetworkResponse>,
    cancel_flag: &AtomicBool,
) -> Result<(), String> {
    let split = request.split_url();
    let use_tls = match split.proto {
        "http" => false,
        "https" => true,
        other => {
            return Err(format!(
                "unsupported URL scheme for http_request: {other} (expected http or https)"
            ));
        }
    };

    let mut stream =
        SocketStream::connect(split.host, split.port, use_tls, request.ignore_ssl_cert)
            .map_err(|e| format!("connect failed: {e}"))?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(30)));

    write_request(&mut stream, request, &split, use_tls)
        .map_err(|e| format!("write failed: {e}"))?;

    let max_body = request.max_response_body_bytes;
    let (status_code, headers_string, mut body_prefix, chunked) =
        match read_response_head(&mut stream, cancel_flag, max_body) {
            Ok(response) => response,
            Err(ReadHeadError::BodyLimit) => {
                stream.shutdown();
                return Err(crate::HTTP_BODY_LIMIT_ERROR.to_string());
            }
            Err(ReadHeadError::Io(error)) => return Err(format!("read failed: {error}")),
        };
    let is_head = matches!(request.method, crate::types::HttpMethod::HEAD);
    let content_length = content_length_of(&headers_string);
    let declared = content_length.unwrap_or(0);
    let mut body_end = BodyEnd::new(is_head, status_code, content_length, chunked);
    body_end.feed(&body_prefix);
    if (!is_head && declared > max_body)
        || body_prefix.len() as u64 > max_body
    {
        stream.shutdown();
        return Err(crate::HTTP_BODY_LIMIT_ERROR.to_string());
    }

    if request.is_streaming {
        let mut streamed = body_prefix.len() as u64;
        if !body_prefix.is_empty() {
            let _ = response_sender.send(NetworkResponse::HttpStreamChunk {
                request_id,
                response: HttpResponse {
                    metadata_id: request.metadata_id,
                    status_code,
                    headers: Default::default(),
                    body: Some(std::mem::take(&mut body_prefix).into()),
                },
            });
        }

        let mut buf = [0u8; 16384];
        while !body_end.is_complete() {
            if cancel_flag.load(Ordering::SeqCst) {
                return Err("request cancelled".to_string());
            }
            let read_len = capped_read_len(max_body, streamed, buf.len());
            match stream.read(&mut buf[..read_len]) {
                Ok(0) => break,
                Ok(n) => {
                    body_end.feed(&buf[..n]);
                    streamed = streamed.saturating_add(n as u64);
                    if streamed > max_body {
                        stream.shutdown();
                        return Err(crate::HTTP_BODY_LIMIT_ERROR.to_string());
                    }
                    let _ = response_sender.send(NetworkResponse::HttpStreamChunk {
                        request_id,
                        response: HttpResponse {
                            metadata_id: request.metadata_id,
                            status_code,
                            headers: Default::default(),
                            body: Some(buf[..n].to_vec().into()),
                        },
                    });
                }
                Err(err)
                    if matches!(
                        err.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    continue;
                }
                Err(err) if body_end.ends_at_close(&err) => break,
                Err(err) => return Err(format!("stream read failed: {err}")),
            }
        }

        let _ = response_sender.send(NetworkResponse::HttpStreamComplete {
            request_id,
            response: HttpResponse::from_header_string(
                request.metadata_id,
                status_code,
                headers_string,
                None,
            ),
        });
        stream.shutdown();
        return Ok(());
    }

    let mut body = std::mem::take(&mut body_prefix);
    if max_body != u64::MAX && declared > 0 {
        if let Ok(declared) = usize::try_from(declared) {
            body.reserve_exact(declared.saturating_sub(body.len()));
        }
    }
    let total = declared;
    let mut last_emit = 0usize;
    let emit_progress = |loaded: u64| {
        let _ = response_sender.send(NetworkResponse::HttpProgress {
            request_id,
            progress: HttpProgress { loaded, total },
        });
    };
    emit_progress(body.len() as u64);
    let mut buf = [0u8; 16384];
    while !body_end.is_complete() {
        if cancel_flag.load(Ordering::SeqCst) {
            return Err("request cancelled".to_string());
        }
        let read_len = capped_read_len(max_body, body.len() as u64, buf.len());
        match stream.read(&mut buf[..read_len]) {
            Ok(0) => break,
            Ok(n) => {
                body_end.feed(&buf[..n]);
                if body.len().saturating_add(n) as u64 > max_body {
                    stream.shutdown();
                    return Err(crate::HTTP_BODY_LIMIT_ERROR.to_string());
                }
                if max_body != u64::MAX && declared == 0 {
                    body.reserve_exact(n);
                }
                body.extend_from_slice(&buf[..n]);
                if body.len().saturating_sub(last_emit) >= 256 * 1024 {
                    last_emit = body.len();
                    emit_progress(body.len() as u64);
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(err) if body_end.ends_at_close(&err) => break,
            Err(err) => return Err(format!("read failed: {err}")),
        }
    }
    stream.shutdown();

    if chunked {
        if let Ok(decoded) = decode_chunked_body(&body) {
            body = decoded;
        }
    }

    let _ = response_sender.send(NetworkResponse::HttpResponse {
        request_id,
        response: HttpResponse::from_header_string(
            request.metadata_id,
            status_code,
            headers_string,
            Some(body),
        ),
    });
    Ok(())
}

fn write_request(
    stream: &mut SocketStream,
    request: &HttpRequest,
    split: &crate::types::SplitUrl<'_>,
    use_tls: bool,
) -> io::Result<()> {
    let path = if split.file.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", split.file)
    };
    let method = request.method.as_str();

    let default_port = if use_tls { "443" } else { "80" };
    let host_header = if split.port == default_port {
        split.host.to_string()
    } else {
        format!("{}:{}", split.host, split.port)
    };

    let mut req = String::new();
    req.push_str(&format!("{method} {path} HTTP/1.1\r\n"));
    req.push_str(&format!("Host: {host_header}\r\n"));
    req.push_str("Connection: close\r\n");

    let mut has_content_length = false;
    for (name, values) in &request.headers {
        if name.eq_ignore_ascii_case("content-length") {
            has_content_length = true;
        }
        for value in values {
            req.push_str(name);
            req.push_str(": ");
            req.push_str(value);
            req.push_str("\r\n");
        }
    }

    if let Some(body) = &request.body {
        if !has_content_length {
            req.push_str(&format!("Content-Length: {}\r\n", body.len()));
        }
    }
    req.push_str("\r\n");

    write_all(stream, req.as_bytes())?;
    if let Some(body) = &request.body {
        write_all(stream, body)?;
    }
    stream.flush()
}

enum ReadHeadError {
    Io(io::Error),
    BodyLimit,
}

fn read_response_head(
    stream: &mut SocketStream,
    cancel_flag: &AtomicBool,
    max_body: u64,
) -> Result<(u16, String, Vec<u8>, bool), ReadHeadError> {
    let mut data = Vec::with_capacity(8192);
    let mut buf = [0u8; 4096];
    let mut header_end = None;

    while header_end.is_none() {
        if cancel_flag.load(Ordering::SeqCst) {
            return Err(ReadHeadError::Io(io::Error::new(
                io::ErrorKind::Interrupted,
                "request cancelled",
            )));
        }
        match stream.read(&mut buf) {
            Ok(0) => {
                return Err(ReadHeadError::Io(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "connection closed before HTTP headers",
                )));
            }
            Ok(n) => {
                data.extend_from_slice(&buf[..n]);
                header_end = find_header_end(&data);
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(err) => return Err(ReadHeadError::Io(err)),
        }
    }

    let header_end = header_end.unwrap();
    let head = &data[..header_end];
    let body_prefix = &data[header_end..];
    if body_prefix.len() as u64 > max_body {
        return Err(ReadHeadError::BodyLimit);
    }
    let body_prefix = body_prefix.to_vec();
    let head_str = String::from_utf8_lossy(head);

    let mut lines = head_str.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status_code = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<u16>().ok())
        .unwrap_or_default();

    let mut headers_string = String::new();
    let mut chunked = false;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            headers_string.push_str(name.trim());
            headers_string.push_str(": ");
            headers_string.push_str(value);
            headers_string.push('\n');

            if name.eq_ignore_ascii_case("transfer-encoding")
                && value.to_ascii_lowercase().contains("chunked")
            {
                chunked = true;
            }
        }
    }

    Ok((status_code, headers_string, body_prefix, chunked))
}

fn capped_read_len(max_body: u64, received: u64, buffer_len: usize) -> usize {
    let through_first_excess = max_body.saturating_sub(received).saturating_add(1);
    usize::try_from(through_first_excess)
        .unwrap_or(buffer_len)
        .min(buffer_len)
        .max(1)
}

fn find_header_end(data: &[u8]) -> Option<usize> {
    data.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
}

fn write_all(stream: &mut SocketStream, data: &[u8]) -> io::Result<()> {
    let mut offset = 0;
    while offset < data.len() {
        match stream.write(&data[offset..]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::WriteZero,
                    "socket closed while writing",
                ));
            }
            Ok(n) => offset += n,
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock
                        | io::ErrorKind::TimedOut
                        | io::ErrorKind::Interrupted
                ) =>
            {
                continue;
            }
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

fn content_length_of(headers: &str) -> Option<u64> {
    headers.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if name.trim().eq_ignore_ascii_case("content-length") {
            value.trim().parse().ok()
        } else {
            None
        }
    })
}

/// Where a response body ends (RFC 9112 section 6.3), followed as its bytes
/// arrive. The reader stops once the body is complete instead of waiting for
/// the server to close, so a TLS peer that closes without close_notify after
/// a complete response is never read into an error.
enum BodyEnd {
    /// HEAD, 1xx, 204 and 304 responses carry no body.
    Empty,
    /// Content-Length: the bytes still to come.
    Length(u64),
    Chunked(ChunkedEnd),
    /// Neither a length nor chunking: the body ends when the connection does.
    Close,
}

impl BodyEnd {
    fn new(is_head: bool, status: u16, content_length: Option<u64>, chunked: bool) -> Self {
        if is_head || (100..200).contains(&status) || status == 204 || status == 304 {
            BodyEnd::Empty
        } else if chunked {
            BodyEnd::Chunked(ChunkedEnd::default())
        } else if let Some(length) = content_length {
            BodyEnd::Length(length)
        } else {
            BodyEnd::Close
        }
    }

    fn feed(&mut self, bytes: &[u8]) {
        match self {
            BodyEnd::Length(remaining) => {
                *remaining = remaining.saturating_sub(bytes.len() as u64)
            }
            BodyEnd::Chunked(chunked) => chunked.feed(bytes),
            BodyEnd::Empty | BodyEnd::Close => {}
        }
    }

    fn is_complete(&self) -> bool {
        match self {
            BodyEnd::Empty | BodyEnd::Length(0) => true,
            BodyEnd::Length(_) | BodyEnd::Close => false,
            BodyEnd::Chunked(chunked) => chunked.is_done(),
        }
    }

    /// A TLS close without close_notify ends a close-delimited body, as it
    /// did before OpenSSL 3. A body with its own framing that is still
    /// incomplete was cut short, so that stays an error.
    fn ends_at_close(&self, err: &io::Error) -> bool {
        err.kind() == io::ErrorKind::UnexpectedEof
            && (self.is_complete() || matches!(self, BodyEnd::Close))
    }
}

/// Follows a chunked body (RFC 9112 section 7.1) byte by byte to see when its
/// last chunk and trailer section are in. Malformed framing never completes,
/// so such a body is still read until the connection closes.
#[derive(Default)]
struct ChunkedEnd {
    state: ChunkState,
    size: u64,
    trailer_line_len: usize,
}

#[derive(Default, PartialEq)]
enum ChunkState {
    #[default]
    Size,
    Extension,
    Data,
    DataCr,
    DataLf,
    Trailer,
    Done,
    Invalid,
}

impl ChunkedEnd {
    fn is_done(&self) -> bool {
        self.state == ChunkState::Done
    }

    fn feed(&mut self, mut bytes: &[u8]) {
        while let Some((&byte, rest)) = bytes.split_first() {
            match self.state {
                ChunkState::Done | ChunkState::Invalid => return,
                ChunkState::Data => {
                    let take = self.size.min(bytes.len() as u64) as usize;
                    self.size -= take as u64;
                    bytes = &bytes[take..];
                    if self.size == 0 {
                        self.state = ChunkState::DataCr;
                    }
                    continue;
                }
                ChunkState::Size => match byte {
                    b'\r' => {}
                    b'\n' => self.end_size_line(),
                    b';' | b' ' | b'\t' => self.state = ChunkState::Extension,
                    _ => {
                        let digit = (byte as char).to_digit(16);
                        match (digit, self.size.checked_mul(16)) {
                            (Some(digit), Some(size)) => self.size = size + digit as u64,
                            _ => self.state = ChunkState::Invalid,
                        }
                    }
                },
                ChunkState::Extension => {
                    if byte == b'\n' {
                        self.end_size_line();
                    }
                }
                ChunkState::DataCr => match byte {
                    b'\r' => self.state = ChunkState::DataLf,
                    b'\n' => self.state = ChunkState::Size,
                    _ => self.state = ChunkState::Invalid,
                },
                ChunkState::DataLf => match byte {
                    b'\n' => self.state = ChunkState::Size,
                    _ => self.state = ChunkState::Invalid,
                },
                ChunkState::Trailer => match byte {
                    b'\r' => {}
                    b'\n' if self.trailer_line_len == 0 => self.state = ChunkState::Done,
                    b'\n' => self.trailer_line_len = 0,
                    _ => self.trailer_line_len += 1,
                },
            }
            bytes = rest;
        }
    }

    fn end_size_line(&mut self) {
        self.state = if self.size == 0 {
            ChunkState::Trailer
        } else {
            ChunkState::Data
        };
    }
}

fn decode_chunked_body(raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let mut i = 0usize;

    while i < raw.len() {
        let line_end = find_crlf(raw, i).ok_or("invalid chunked body: missing chunk size line")?;
        let size_line = std::str::from_utf8(&raw[i..line_end])
            .map_err(|_| "invalid chunked body: chunk size line is not utf-8")?;
        let size_hex = size_line.split(';').next().unwrap_or_default().trim();
        let size = usize::from_str_radix(size_hex, 16)
            .map_err(|_| "invalid chunked body: bad chunk size")?;
        i = line_end + 2;

        if size == 0 {
            break;
        }
        if i + size > raw.len() {
            return Err("invalid chunked body: chunk exceeds buffer".to_string());
        }
        out.extend_from_slice(&raw[i..i + size]);
        i += size;

        if i + 2 > raw.len() || &raw[i..i + 2] != b"\r\n" {
            return Err("invalid chunked body: missing chunk terminator".to_string());
        }
        i += 2;
    }
    Ok(out)
}

fn find_crlf(data: &[u8], start: usize) -> Option<usize> {
    data[start..]
        .windows(2)
        .position(|w| w == b"\r\n")
        .map(|p| start + p)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eof() -> io::Error {
        io::Error::new(io::ErrorKind::UnexpectedEof, "no close_notify")
    }

    #[test]
    fn content_length_body_ends_at_its_length() {
        let mut end = BodyEnd::new(false, 401, Some(10), false);
        end.feed(b"01234");
        assert!(!end.is_complete());
        assert!(!end.ends_at_close(&eof()), "a short body is truncated");
        end.feed(b"56789");
        assert!(end.is_complete());
        assert!(end.ends_at_close(&eof()));
        assert!(!end.ends_at_close(&io::Error::new(io::ErrorKind::Other, "tls")));
    }

    #[test]
    fn chunked_body_ends_at_its_terminator() {
        let raw: &[u8] = b"5;ext=1\r\nhello\r\n6\r\n world\r\n0\r\nX-Trailer: 1\r\n\r\n";
        // Byte by byte, so every state sees a read boundary.
        let mut end = BodyEnd::new(false, 200, None, true);
        for (i, byte) in raw.iter().enumerate() {
            assert!(!end.is_complete(), "complete early at byte {i}");
            end.feed(std::slice::from_ref(byte));
        }
        assert!(end.is_complete());
        assert_eq!(decode_chunked_body(raw).unwrap(), b"hello world");

        let mut end = BodyEnd::new(false, 200, Some(3), true);
        end.feed(b"3\r\nabc\r\n0\r\n");
        assert!(!end.is_complete(), "chunked wins over content-length");
        end.feed(b"\r\n");
        assert!(end.is_complete());

        let mut end = BodyEnd::new(false, 200, None, true);
        end.feed(b"zz\r\n");
        assert!(!end.is_complete());
        assert!(!end.ends_at_close(&eof()));
    }

    #[test]
    fn bodyless_and_close_delimited_responses() {
        assert!(BodyEnd::new(true, 200, Some(512), false).is_complete());
        assert!(BodyEnd::new(false, 204, None, false).is_complete());
        assert!(BodyEnd::new(false, 304, Some(9), false).is_complete());
        let close = BodyEnd::new(false, 200, None, false);
        assert!(!close.is_complete());
        assert!(close.ends_at_close(&eof()));
    }
}
