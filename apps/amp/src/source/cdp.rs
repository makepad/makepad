//! Just enough of the Chrome DevTools Protocol to evaluate script in one
//! page of a Chromium app on this machine: the target list over HTTP and
//! `Runtime.evaluate` over the page's WebSocket. Synchronous, on the calling
//! worker; frames are encoded and decoded by the platform's own WebSocket
//! code. Only 127.0.0.1 is ever dialled.

use makepad_micro_serde::*;
use makepad_widgets::makepad_platform::makepad_network::{ParsedWebSocketMessage, WebSocketMessageFormat, WebSocketMessageHeader, WebSocketParser};
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

/// Connecting to a port nobody listens on fails at once; one that hangs is
/// given up after this.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
/// The target list and the WebSocket handshake answer in this time or not
/// at all.
const HTTP_TIMEOUT: Duration = Duration::from_secs(3);
/// An answer larger than this is not one this client asked for.
const MAX_MESSAGE: usize = 128 * 1024 * 1024;

#[derive(DeJson, Debug)]
pub struct Target {
    #[rename(type)]
    pub kind: Option<String>,
    pub url: Option<String>,
    #[rename(webSocketDebuggerUrl)]
    pub web_socket: Option<String>,
}

#[derive(DeJson, Debug)]
struct ReplyJson {
    id: Option<u64>,
    result: Option<EvaluatedJson>,
    error: Option<ProtocolErrorJson>,
}

#[derive(DeJson, Debug)]
struct EvaluatedJson {
    result: Option<RemoteObjectJson>,
    #[rename(exceptionDetails)]
    exception: Option<ExceptionJson>,
}

#[derive(DeJson, Debug)]
struct RemoteObjectJson {
    #[rename(type)]
    kind: Option<String>,
    value: Option<String>,
    description: Option<String>,
}

#[derive(DeJson, Debug)]
struct ExceptionJson {
    text: Option<String>,
    exception: Option<RemoteObjectJson>,
}

#[derive(DeJson, Debug)]
struct ProtocolErrorJson {
    message: Option<String>,
}

fn local(port: u16) -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, port))
}

/// The debugging targets of the app listening on `port`.
pub fn targets(port: u16) -> Result<Vec<Target>, String> {
    let mut stream = TcpStream::connect_timeout(&local(port), CONNECT_TIMEOUT).map_err(|error| format!("nothing answers on 127.0.0.1:{port} ({error})"))?;
    let _ = stream.set_read_timeout(Some(HTTP_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HTTP_TIMEOUT));
    stream
        .write_all(format!("GET /json/list HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes())
        .map_err(|error| format!("target list: {error}"))?;
    // DevTools keeps the connection open whatever the request says: the
    // answer ends where its Content-Length says.
    let mut answer = Vec::new();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        if let Some(end) = answer.windows(4).position(|w| w == b"\r\n\r\n").map(|at| at + 4) {
            let head = String::from_utf8_lossy(&answer[..end]).to_string();
            if !head.starts_with("HTTP/1.1 200") && !head.starts_with("HTTP/1.0 200") {
                return Err(format!("target list: {}", head.lines().next().unwrap_or_default()));
            }
            let length = head.lines().find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.trim().eq_ignore_ascii_case("content-length").then(|| value.trim().parse::<usize>().ok()).flatten()
            });
            if length.is_some_and(|length| answer.len() - end >= length) {
                let body = String::from_utf8_lossy(&answer[end..end + length.unwrap_or(0)]).to_string();
                return Vec::<Target>::deserialize_json_lenient(&body).map_err(|error| format!("target list: {error:?}"));
            }
        }
        if answer.len() > MAX_MESSAGE {
            return Err("target list: answer too large".into());
        }
        match stream.read(&mut buffer) {
            Ok(0) => return Err("target list: connection closed before the answer ended".into()),
            Ok(read) => answer.extend_from_slice(&buffer[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => return Err(format!("target list: {error}")),
        }
    }
}

/// One page's DevTools connection.
pub struct Cdp {
    stream: TcpStream,
    parser: WebSocketParser,
    next_id: u64,
}

impl Cdp {
    /// Open `ws://127.0.0.1:<port>/...`, the page's `webSocketDebuggerUrl`.
    pub fn connect(port: u16, web_socket: &str) -> Result<Cdp, String> {
        let path = web_socket.strip_prefix(&format!("ws://127.0.0.1:{port}")).filter(|path| path.starts_with('/')).ok_or_else(|| format!("not a local page: {web_socket}"))?;
        let mut stream = TcpStream::connect_timeout(&local(port), CONNECT_TIMEOUT).map_err(|error| format!("cannot reach 127.0.0.1:{port} ({error})"))?;
        let _ = stream.set_nodelay(true);
        let _ = stream.set_read_timeout(Some(HTTP_TIMEOUT));
        let _ = stream.set_write_timeout(Some(HTTP_TIMEOUT));
        // No Origin header: DevTools refuses WebSocket clients from web origins
        // it was not told to allow, and this is none.
        let request = format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: {}\r\nSec-WebSocket-Version: 13\r\n\r\n", handshake_key());
        stream.write_all(request.as_bytes()).map_err(|error| format!("handshake: {error}"))?;
        let deadline = Instant::now() + HTTP_TIMEOUT;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        // Byte by byte up to the blank line, so no frame after it is read here.
        while !head.ends_with(b"\r\n\r\n") {
            if Instant::now() >= deadline || head.len() > 16 * 1024 {
                return Err("handshake: no answer".into());
            }
            match stream.read(&mut byte) {
                Ok(0) => return Err("handshake: connection closed".into()),
                Ok(_) => head.push(byte[0]),
                Err(error) if matches!(error.kind(), std::io::ErrorKind::Interrupted) => {}
                Err(error) => return Err(format!("handshake: {error}")),
            }
        }
        let status = String::from_utf8_lossy(&head);
        if !status.starts_with("HTTP/1.1 101") {
            return Err(format!("handshake refused: {}", status.lines().next().unwrap_or_default()));
        }
        Ok(Cdp { stream, parser: WebSocketParser::new(), next_id: 1 })
    }

    /// A client's frames are masked (RFC 6455); DevTools drops the
    /// connection on one that is not.
    fn send(&mut self, format: WebSocketMessageFormat, data: &[u8]) -> Result<(), String> {
        let header = WebSocketMessageHeader::from_len(data.len(), format, true);
        let frame = WebSocketParser::build_message(header, data);
        self.stream.write_all(&frame).map_err(|error| format!("send: {error}"))
    }

    /// Evaluate `expression` in the page, awaiting a promise, and return the
    /// string it comes to. An exception or a value that is not a string is
    /// an error.
    pub fn evaluate(&mut self, expression: &str, timeout: Duration) -> Result<String, String> {
        let id = self.next_id;
        self.next_id += 1;
        let message = format!(
            "{{\"id\":{id},\"method\":\"Runtime.evaluate\",\"params\":{{\"expression\":{},\"awaitPromise\":true,\"returnByValue\":true}}}}",
            expression.to_string().serialize_json()
        );
        self.send(WebSocketMessageFormat::Text, message.as_bytes())?;
        let reply = self.reply(id, timeout)?;
        let reply = ReplyJson::deserialize_json_lenient(&reply).map_err(|error| format!("evaluate: unexpected answer ({error:?})"))?;
        if let Some(error) = reply.error {
            return Err(format!("evaluate: {}", error.message.unwrap_or_default()));
        }
        let evaluated = reply.result.ok_or("evaluate: no result")?;
        if let Some(exception) = evaluated.exception {
            let what = exception.exception.and_then(|e| e.description).or(exception.text).unwrap_or_default();
            return Err(format!("script error: {}", what.lines().next().unwrap_or_default()));
        }
        let value = evaluated.result.ok_or("evaluate: no value")?;
        match (value.kind.as_deref(), value.value) {
            (Some("string"), Some(text)) => Ok(text),
            (kind, _) => Err(format!("evaluate: a {} came back, not text", kind.unwrap_or("nothing"))),
        }
    }

    /// Read frames until the answer to `id`; events and older answers are
    /// dropped.
    fn reply(&mut self, id: u64, timeout: Duration) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        let mut buffer = vec![0u8; 256 * 1024];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(format!("no answer in {} s", timeout.as_secs()));
            }
            let _ = self.stream.set_read_timeout(Some(left));
            let read = match self.stream.read(&mut buffer) {
                Ok(0) => return Err("the page closed the connection".into()),
                Ok(read) => read,
                Err(error) if matches!(error.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => continue,
                Err(error) => return Err(format!("receive: {error}")),
            };
            let mut texts = Vec::new();
            let mut pings = 0;
            let mut closed = false;
            let mut failed = None;
            self.parser.parse(&buffer[..read], |message| match message {
                Ok(ParsedWebSocketMessage::Text(text)) => texts.push(text.to_string()),
                Ok(ParsedWebSocketMessage::Ping(_)) => pings += 1,
                Ok(ParsedWebSocketMessage::Close) => closed = true,
                Ok(_) => {}
                Err(error) => failed = Some(format!("{error:?}")),
            });
            if let Some(error) = failed {
                return Err(format!("bad frame: {error}"));
            }
            for _ in 0..pings {
                // A masked, empty pong.
                let mask = handshake_bytes();
                self.stream.write_all(&[0x8A, 0x80, mask[0], mask[1], mask[2], mask[3]]).map_err(|error| format!("send: {error}"))?;
            }
            for text in texts {
                if text.len() > MAX_MESSAGE {
                    return Err("answer too large".into());
                }
                // Cheap check before a full parse: only our answer matters.
                let head: String = text.chars().take(32).collect();
                if head.starts_with(&format!("{{\"id\":{id},")) || head.starts_with(&format!("{{\"id\":{id}}}")) {
                    return Ok(text);
                }
                if !head.starts_with("{\"method\"") {
                    if let Ok(reply) = ReplyJson::deserialize_json_lenient(&text) {
                        if reply.id == Some(id) {
                            return Ok(text);
                        }
                    }
                }
            }
            if closed {
                return Err("the page closed the connection".into());
            }
        }
    }
}

/// 16 bytes that differ per connection.
fn handshake_bytes() -> [u8; 16] {
    let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0) ^ (std::process::id() as u128) << 64;
    let mut state = seed as u64 ^ (seed >> 64) as u64 ^ 0x9e37_79b9_7f4a_7c15;
    let mut out = [0u8; 16];
    for chunk in out.chunks_mut(8) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        chunk.copy_from_slice(&state.to_le_bytes()[..chunk.len()]);
    }
    out
}

fn handshake_key() -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = handshake_bytes();
    let mut key = String::new();
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16 | (*chunk.get(1).unwrap_or(&0) as u32) << 8 | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                key.push(ALPHABET[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                key.push('=');
            }
        }
    }
    key
}

