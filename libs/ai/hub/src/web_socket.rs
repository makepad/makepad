//! A WebSocket to a selected hub, under the same network policy and identity as a job request.

use makepad_network::web_socket_parser::{WebSocketMessage as Frame, WebSocketMessageFormat, WebSocketMessageHeader, WebSocketParser};
use makepad_network::WsSend;
use std::io::{self, Read, Write};
use std::time::{Duration, Instant};

#[cfg(not(target_arch = "wasm32"))]
use makepad_network::tls::os_random;
#[cfg(target_arch = "wasm32")]
fn os_random(_: &mut [u8]) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Unsupported, "blocking WebSockets require native sockets"))
}

pub(crate) enum Link {
    Plain(std::net::TcpStream),
    /// a fleet TLS link (native only: never made on the web)
    #[cfg(not(target_arch = "wasm32"))]
    Tls(makepad_network::SocketStream),
}

impl Link {
    pub(crate) fn set_read_timeout(&self, t: Duration) {
        let _ = match self {
            Link::Plain(s) => s.set_read_timeout(Some(t)),
            #[cfg(not(target_arch = "wasm32"))]
            Link::Tls(s) => s.set_read_timeout(Some(t)),
        };
    }
    pub(crate) fn shutdown(&mut self) {
        match self {
            Link::Plain(s) => {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            #[cfg(not(target_arch = "wasm32"))]
            Link::Tls(s) => s.shutdown(),
        }
    }
}

impl std::io::Read for Link {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Link::Plain(s) => s.read(buf),
            #[cfg(not(target_arch = "wasm32"))]
            Link::Tls(s) => s.read(buf),
        }
    }
}

impl std::io::Write for Link {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Link::Plain(s) => s.write(buf),
            #[cfg(not(target_arch = "wasm32"))]
            Link::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Link::Plain(s) => s.flush(),
            #[cfg(not(target_arch = "wasm32"))]
            Link::Tls(s) => s.flush(),
        }
    }
}

/// A hub on this machine's loopback surface: plain http to a loopback address.
fn on_loopback(url: &crate::http_client::ParsedUrl) -> bool {
    let host = url.host.trim_start_matches('[').trim_end_matches(']');
    !url.https && host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.to_canonical().is_loopback())
}

/// The machine token header for a hub on this machine's loopback surface, which takes this
/// machine's apps by it; none for any other node.
pub(crate) fn machine_token_for(url: &crate::http_client::ParsedUrl) -> Result<Option<(String, String)>, String> {
    #[cfg(not(target_arch = "wasm32"))]
    if on_loopback(url) {
        let token = crate::machine::machine_token().map_err(|e| format!("{}:{}: the machine token: {e}", url.host, url.port))?;
        return Ok(Some((crate::machine::TOKEN_HEADER.to_string(), token)));
    }
    let _ = url;
    Ok(None)
}

/// Connects and upgrades; returns the link, any bytes read past the upgrade response, and
/// whether the hub is on this machine's loopback surface.
pub(crate) fn open_link(base_url: &str, path: &str) -> Result<(Link, Vec<u8>, bool), String> {
    if !path.starts_with('/') || path.bytes().any(|b| !b.is_ascii_graphic()) {
        return Err("invalid WebSocket request path".into());
    }
    let url = crate::http_client::parse_url(base_url).map_err(|e| e.to_string())?;
    if url.host.bytes().any(|b| !b.is_ascii_graphic()) { return Err("invalid WebSocket host".into()); }
    let host_port = format!("{}:{}", url.host, url.port);
    makepad_network::lan::hub_check_host("ai-hub WebSocket", &url.host).map_err(|e| e.to_string())?;
    let (mut link, authorization): (Link, Option<String>) = if url.https {
        #[cfg(target_arch = "wasm32")]
        return Err(format!("{host_port}: fleet TLS links are native-only"));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (s, _server, auth) = crate::http_client::with_hub_connect(|| makepad_network::fleet::id::connect(&url.host, &url.port.to_string())).map_err(|e| format!("{host_port}: {e}"))?;
            (Link::Tls(s), Some(auth))
        }
    } else {
        let s = makepad_network::lan::hub_connect_host("ai-hub WebSocket", &url.host, url.port, Some(Duration::from_secs(5))).map_err(|e| format!("{host_port}: {e}"))?;
        let _ = s.set_nodelay(true);
        (Link::Plain(s), None)
    };
    let mut nonce = [0u8; 16];
    os_random(&mut nonce).map_err(|e| e.to_string())?;
    let key = makepad_network::digest::base64_encode(&nonce);
    let mut digest = makepad_network::digest::Sha1::new();
    digest.update(format!("{key}258EAFA5-E914-47DA-95CA-C5AB0DC85B11").as_bytes());
    let accept = makepad_network::digest::base64_encode(&digest.finalise());
    let mut head = format!(
        "GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: {key}\r\n"
    );
    if let Some(a) = authorization {
        head.push_str(&format!("Authorization: {a}\r\n"));
    }
    if let Some((name, token)) = machine_token_for(&url)? {
        head.push_str(&format!("{name}: {token}\r\n"));
    }
    head.push_str("\r\n");
    link.write_all(head.as_bytes()).map_err(|e| format!("{host_port}: {e}"))?;
    link.set_read_timeout(Duration::from_millis(200));
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        if let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") {
            let status = String::from_utf8_lossy(&data[..end]).lines().next().unwrap_or_default().to_string();
            if status.split_ascii_whitespace().nth(1) != Some("101") {
                return Err(format!("{host_port}: upgrade refused ({})", status.trim()));
            }
            let headers = String::from_utf8_lossy(&data[..end]);
            if !headers.lines().filter_map(|line| line.split_once(':')).any(|(name, value)| {
                name.eq_ignore_ascii_case("Sec-WebSocket-Accept") && value.trim() == accept
            }) { return Err("invalid WebSocket upgrade response".into()); }
            return Ok((link, data[end + 4..].to_vec(), on_loopback(&url)));
        }
        if Instant::now() >= deadline {
            return Err(format!("{host_port}: no answer to the stream upgrade"));
        }
        match link.read(&mut buf) {
            Ok(0) => return Err(format!("{host_port}: closed during the stream upgrade")),
            Ok(n) => data.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(e) => return Err(format!("{host_port}: {e}")),
        }
    }
}

/// A blocking connection. The caller owns the thread which reads and writes it.
pub struct HubWebSocket {
    link: Link,
    parser: WebSocketParser,
    leftover: Vec<u8>,
    buffer: Vec<u8>,
    pub loopback: bool,
}

/// Shuts down the transport from another thread while the connection owner is in a read or write.
#[derive(Debug)]
pub struct HubWsCloser(std::net::TcpStream);

impl HubWsCloser {
    pub fn close(&self) -> io::Result<()> {
        self.0.shutdown(std::net::Shutdown::Both)
    }
}

impl HubWebSocket {
    pub fn open(base_url: &str, path: &str) -> Result<Self, String> {
        let (link, leftover, loopback) = open_link(base_url, path)?;
        Ok(Self { link, leftover, loopback, parser: WebSocketParser::new(), buffer: vec![0; 64 * 1024] })
    }

    pub fn set_read_timeout(&self, timeout: Duration) {
        self.link.set_read_timeout(timeout);
    }

    /// Clone the underlying TCP socket. The TLS session stays with the connection owner.
    /// Android's Java TLS backend returns `io::ErrorKind::Unsupported`.
    pub fn closer(&self) -> io::Result<HubWsCloser> {
        let socket = match &self.link {
            Link::Plain(stream) => makepad_network::clone_tcp(stream)?,
            #[cfg(not(target_arch = "wasm32"))]
            Link::Tls(stream) => stream.try_clone_tcp()?,
        };
        Ok(HubWsCloser(socket))
    }

    pub fn send(&mut self, message: WsSend) -> io::Result<()> {
        let (format, payload) = match &message {
            WsSend::Text(s) => (WebSocketMessageFormat::Text, s.as_bytes()),
            WsSend::Binary(b) => (WebSocketMessageFormat::Binary, b.as_slice()),
        };
        self.write_frame(format, None, payload)
    }

    fn write_frame(&mut self, format: WebSocketMessageFormat, opcode: Option<u8>, payload: &[u8]) -> io::Result<()> {
        let header = WebSocketMessageHeader::from_len(payload.len(), format, true);
        let mut frame = Vec::with_capacity(header.as_slice().len() + payload.len());
        frame.extend_from_slice(header.as_slice());
        if let Some(opcode) = opcode { frame[0] = 0x80 | opcode; }
        let mut mask = [0; 4];
        os_random(&mut mask)?;
        let at = frame.len() - 4;
        frame[at..].copy_from_slice(&mask);
        frame.extend(payload.iter().enumerate().map(|(i, byte)| byte ^ mask[i % 4]));
        self.link.write_all(&frame)
    }

    /// Deliver complete text and binary messages from one read. A timeout is an ordinary I/O timeout.
    pub fn read(&mut self, mut receive: impl FnMut(Frame<'_>)) -> io::Result<()> {
        let data = if self.leftover.is_empty() {
            let n = self.link.read(&mut self.buffer)?;
            if n == 0 { return Err(io::Error::from(io::ErrorKind::UnexpectedEof)); }
            &self.buffer[..n]
        } else { self.leftover.as_slice() };
        let mut pings = Vec::new();
        let mut result = Ok(());
        self.parser.parse(data, |frame| match frame {
            Ok(message @ (Frame::Text(_) | Frame::Binary(_))) => receive(message),
            Ok(Frame::Ping(payload)) => pings.push(payload.to_vec()),
            Ok(Frame::Pong(_)) => {}
            Ok(Frame::Close) => result = Err(io::Error::from(io::ErrorKind::UnexpectedEof)),
            Err(e) => result = Err(io::Error::new(io::ErrorKind::InvalidData, format!("WebSocket frame: {e:?}"))),
        });
        self.leftover.clear();
        result?;
        for payload in pings { self.write_frame(WebSocketMessageFormat::Binary, Some(10), &payload)?; }
        Ok(())
    }
}

impl Drop for HubWebSocket {
    fn drop(&mut self) { self.link.shutdown(); }
}
