//! The blocking /generate admission wire. A refusal is a typed result of a
//! COMPLETE, strictly framed response, never inferred from a transport error.
//! No redirects, connection retries, or retries after returning a job id.
use crate::error::AssetAiError;
use crate::http_client::{http_fetch_no_redirect, parse_url, HttpClientRequest};
use makepad_strict_json::{self as json, Value};
use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

const ATTEMPTS: usize = 8;
const BUDGET: Duration = Duration::from_secs(90);
const HEAD_LIMIT: usize = 16 * 1024;

pub(super) enum Reply {
    Accepted(String),
    Refused { status: u16, reason: String },
}

pub(super) trait Transport {
    fn post(&mut self, url: &str, headers: &[(String, String)], body: &[u8],
        remaining: Duration) -> Result<Reply, AssetAiError>;
    fn now(&self) -> Instant { Instant::now() }
    fn sleep(&mut self, duration: Duration) { std::thread::sleep(duration); }
}

pub(super) fn submit(
    url: &str, headers: &[(String, String)], body: &[u8],
    cancelled: &dyn Fn() -> bool, pending: &mut dyn FnMut(&str), transport: &mut impl Transport,
) -> Result<String, AssetAiError> {
    let deadline = transport.now() + BUDGET;
    let mut last = "no admission response".to_string();
    for attempt in 1..=ATTEMPTS {
        if cancelled() { return Err(AssetAiError::Cancelled); }
        let remaining = deadline.saturating_duration_since(transport.now());
        if remaining.is_zero() { break; }
        match transport.post(url, headers, body, remaining).map_err(|error| {
            let reason = match error {
                AssetAiError::Http(reason) | AssetAiError::Io(reason) => reason,
                other => other.to_string(),
            };
            AssetAiError::Http(format!("{url}: {reason}"))
        })? {
            // Cancellation during the POST does not discard ownership.
            Reply::Accepted(id) => return Ok(id),
            Reply::Refused { status, reason } => {
                // A full, strictly framed no-job refusal proves no work was
                // accepted. A full disk won't heal on the queue backoff;
                // return its typed reason so fleet callers can choose a peer.
                if let Some(disk) = reason.strip_prefix("model unavailable: disk-space:") {
                    return Err(AssetAiError::Unavailable(format!("disk-space:{disk}")));
                }
                last = format!("http {status}: {reason}");
            }
        }
        if cancelled() { return Err(AssetAiError::Cancelled); }
        if attempt == ATTEMPTS || transport.now() >= deadline { break; }
        let delay = Duration::from_millis((500u64 << (attempt - 1)).min(8_000));
        pending(&format!("waiting for {url} admission (attempt {attempt}/{ATTEMPTS}): {last}"));
        let wake = (transport.now() + delay).min(deadline);
        while transport.now() < wake {
            if cancelled() { return Err(AssetAiError::Cancelled); }
            transport.sleep(wake.saturating_duration_since(transport.now()).min(Duration::from_millis(50)));
        }
    }
    Err(AssetAiError::Http(format!("{url}: admission budget exhausted: {last}")))
}

pub(super) struct HttpTransport;
impl Transport for HttpTransport {
    fn post(&mut self, url: &str, headers: &[(String, String)], body: &[u8],
        remaining: Duration) -> Result<Reply, AssetAiError> {
        let deadline = Instant::now() + remaining;
        let parsed = parse_url(url)?;
        if parsed.https {
            // Keep the platform TLS implementation. Its normalized headers
            // cannot prove the exact transport gate, so HTTPS stays one-shot.
            let mut request = HttpClientRequest::post(url, "application/json", body);
            request.extra_headers = headers;
            let response = http_fetch_no_redirect(&request)?;
            let status = response.status;
            return classify(status, &response.read_body_to_vec(super::MAX_JSON_BODY)?, false);
        }
        let addr = (parsed.host.as_str(), parsed.port).to_socket_addrs()?
            .next().ok_or_else(|| failure("no address for submission endpoint"))?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() { return Err(failure("submission deadline elapsed resolving endpoint")); }
        let stream = TcpStream::connect_timeout(&addr, remaining.min(Duration::from_secs(3)))?;
        let mut stream = DeadlineStream { stream, deadline };
        let mut head = format!("POST {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\n",
            parsed.target, parsed.host, parsed.port, body.len());
        for (key, value) in headers {
            if key.contains(['\r', '\n']) || value.contains(['\r', '\n']) {
                return Err(failure("invalid submission header"));
            }
            head.push_str(&format!("{key}: {value}\r\n"));
        }
        head.push_str("\r\n");
        stream.write_all(head.as_bytes())?;
        stream.write_all(body)?;
        read_reply(&mut stream)
    }
}

// Bound the whole exchange as well as each blocking operation: a peer
// trickling fragments must not renew the admission deadline on every read.
struct DeadlineStream { stream: TcpStream, deadline: Instant }
impl DeadlineStream {
    fn timeout(&self) -> std::io::Result<Duration> {
        self.deadline.checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero()).map(|d| d.min(Duration::from_secs(10)))
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::TimedOut, "submission deadline elapsed"))
    }
}
impl Read for DeadlineStream {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.stream.set_read_timeout(Some(self.timeout()?))?;
        self.stream.read(bytes)
    }
}
impl Write for DeadlineStream {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.stream.set_write_timeout(Some(self.timeout()?))?;
        self.stream.write(bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.stream.flush() }
}

fn failure(message: &str) -> AssetAiError { AssetAiError::Http(message.into()) }

pub(super) fn safe_note(note: &str, request: &crate::protocol::GenerateRequestJson,
    headers: &[(String, String)]) -> String {
    let mut out = note.to_string();
    let texts = [&request.prompt, &request.negative_prompt, &request.input_b64,
        &request.chat_system, &request.text, &request.lyrics, &request.identity_anchor];
    let messages = request.chat_messages.iter().flatten().map(|m| m.text.as_str());
    let inputs = request.inputs.iter().flatten().map(|i| i.data_b64.as_str());
    for text in texts.into_iter().filter_map(|s| s.as_deref()).chain(messages).chain(inputs)
        .chain(headers.iter().map(|(_, value)| value.strip_prefix("Bearer ").unwrap_or(value))) {
        if !text.is_empty() { out = out.replace(text, "[redacted]"); }
    }
    let mut bounded: String = out.chars().take(768).map(|c| if c.is_control() { ' ' } else { c }).collect();
    if out.chars().count() > 768 { bounded.push('…'); }
    bounded
}

pub(super) fn read_reply(reader: &mut impl Read) -> Result<Reply, AssetAiError> {
    let mut bytes = Vec::new();
    let mut chunk = [0; 4096];
    let start = loop {
        if let Some(end) = bytes.windows(4).position(|s| s == b"\r\n\r\n") {
            if end > HEAD_LIMIT { return Err(failure("response head too large")); }
            break end + 4;
        }
        if bytes.len() > HEAD_LIMIT { return Err(failure("response head too large")); }
        let n = reader.read(&mut chunk)?;
        if n == 0 { return Err(failure("incomplete response head")); }
        bytes.extend_from_slice(&chunk[..n]);
    };
    let head = std::str::from_utf8(&bytes[..start - 4]).map_err(|_| failure("non-UTF8 response head"))?;
    let mut lines = head.split("\r\n");
    let status_line = lines.next().ok_or_else(|| failure("missing status"))?;
    let mut status_parts = status_line.splitn(3, ' ');
    if !matches!(status_parts.next(), Some("HTTP/1.1" | "HTTP/1.0")) {
        return Err(failure("invalid HTTP version"));
    }
    let status: u16 = status_parts.next().filter(|code| code.len() == 3)
        .and_then(|code| code.parse().ok()).ok_or_else(|| failure("invalid response status"))?;
    let mut length = None;
    let mut connection = None;
    for line in lines {
        let (name, value) = line.split_once(':').ok_or_else(|| failure("malformed response header"))?;
        if name.eq_ignore_ascii_case("content-length") {
            if length.is_some() { return Err(failure("duplicate content-length")); }
            let value = value.trim();
            if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
                return Err(failure("invalid content-length"));
            }
            length = Some(value.parse::<usize>().map_err(|_| failure("invalid content-length"))?);
        }
        if name.eq_ignore_ascii_case("connection") {
            if connection.is_some() { return Err(failure("duplicate connection header")); }
            connection = Some(value.trim());
        }
        if name.eq_ignore_ascii_case("transfer-encoding") {
            return Err(failure("unsupported submission transfer-encoding"));
        }
    }
    // The hub always sends Content-Length. Close-delimited/normalized bodies
    // do not constitute proof of complete pre-admission rejection here.
    let length = length.ok_or_else(|| failure("missing submission content-length"))?;
    if length > super::MAX_JSON_BODY { return Err(failure("response body too large")); }
    let empty_gate = status_line == "HTTP/1.1 503 Service Unavailable"
        && length == 0 && connection.is_some_and(|s| s.eq_ignore_ascii_case("close"));
    if bytes.len() > start + length { return Err(failure("unexpected bytes after response body")); }
    while bytes.len() < start + length {
        let take = chunk.len().min(start + length - bytes.len());
        let n = reader.read(&mut chunk[..take])?;
        if n == 0 { return Err(failure("incomplete response body")); }
        bytes.extend_from_slice(&chunk[..n]);
    }
    if empty_gate {
        return Ok(Reply::Refused { status, reason: "HTTP server overloaded before job admission".into() });
    }
    classify(status, &bytes[start..], true)
}

fn classify(status: u16, bytes: &[u8], allow_refusal: bool) -> Result<Reply, AssetAiError> {
    // Never echo response documents, which may contain prompts or payloads.
    let value = json::parse(bytes).map_err(|_| failure(&format!("http {status}: malformed submission JSON")))?;
    if let Some(id) = value.get("job_id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
        return Ok(Reply::Accepted(id.into()));
    }
    let error = value.get("error").and_then(Value::as_str).filter(|s| !s.trim().is_empty());
    let no_job = matches!(value.get("job_id"), None | Some(Value::Null));
    let valid_metadata = matches!(value.get("think_open"), None | Some(Value::Null | Value::Bool(_)));
    let known_shape = matches!(&value, Value::Obj(fields) if fields.iter().all(|(key, _)|
        matches!(key.as_str(), "job_id" | "error" | "think_open")));
    // Explicit null is the GenerateResponseJson no-job contract. Legacy
    // error-only responses are safe only for the hub's Busy/QueueFull reasons.
    let known_reason = error.is_some_and(|reason| reason == "busy: a job is already queued or running"
        || reason.strip_prefix("queue full: ").and_then(|s| s.strip_suffix(" jobs already queued on this node"))
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())));
    if allow_refusal && matches!(status, 409 | 429 | 503) && no_job && known_shape && valid_metadata
        && error.is_some() && (value.get("job_id") == Some(&Value::Null) || known_reason) {
        return Ok(Reply::Refused { status, reason: error.unwrap().into() });
    }
    Err(failure(&format!("http {status}: {}", error.unwrap_or("submission returned no job id"))))
}
