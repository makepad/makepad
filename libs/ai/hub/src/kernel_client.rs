//! The client side of the `kernel` domain (wire: [`crate::kernel_wire`]): a node's info, the
//! stream compute program sessions run over (`crate::program_client`), and the fleet's nodes.
//!
//! A fleet URL (`https://host:port`, from the LAN beacon or
//! [`makepad_network::fleet::id::mark_fleet_endpoint`]) goes through the fleet TLS
//! front with this process's credential; a loopback URL (`http://`) is a
//! machine-local node.

use crate::error::AssetAiError;
use crate::http_client::{http_fetch, HttpClientRequest};
use crate::kernel_wire::*;
use makepad_compute_program::codec::Codec;
use makepad_compute_program::{wire, Value};
use makepad_micro_serde::DeJson;
use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// One kernel node.
#[derive(Clone, Debug)]
pub struct KernelNode {
    pub base_url: String,
}

impl KernelNode {
    pub fn new(base_url: &str) -> Self {
        KernelNode { base_url: base_url.trim_end_matches('/').to_string() }
    }

    /// `GET /kernel`: Err when the node does not serve programs, or speaks another version.
    pub fn info(&self) -> Result<KernelInfoJson, AssetAiError> {
        let url = format!("{}/kernel", self.base_url);
        let headers: Vec<(String, String)> = machine_token_for(&crate::http_client::parse_url(&url)?).map_err(AssetAiError::Http)?.into_iter().collect();
        let response = http_fetch(&HttpClientRequest { extra_headers: &headers, ..HttpClientRequest::get(&url) })?;
        let status = response.status;
        let body = response.read_body_to_vec(1 << 20)?;
        if status != 200 {
            return Err(AssetAiError::Http(format!("{url}: {status} {}", String::from_utf8_lossy(&body))));
        }
        let text = String::from_utf8_lossy(&body);
        let info = KernelInfoJson::deserialize_json(&text).map_err(|e| AssetAiError::Http(format!("{url}: not a program node ({e:?})")))?;
        if info.api != KERNEL_API_VERSION {
            return Err(AssetAiError::Http(format!("{url}: kernel api {} (this client speaks {KERNEL_API_VERSION})", info.api)));
        }
        Ok(info)
    }
}

/// One long-lived stream to a node (`GET /kernel/stream`, a websocket over the fleet TLS
/// front): program frames go up it, the sessions' updates come down it as the node sends them.
///
/// One thread owns the connection (a TLS session cannot be split into a reader and a writer):
/// it writes what is queued, then reads for at most 1 ms while sessions are open or frames moved
/// in the last half second (10 ms when idle), so a message leaves within a millisecond.
pub struct KernelStream {
    out: Mutex<std::sync::mpsc::Sender<Vec<u8>>>,
    shared: Arc<StreamShared>,
    /// To a hub on this machine's loopback surface: what crosses it is a copy, not sent.
    loopback: bool,
    /// What the node keeps of this stream's inputs, as it keeps it: each slot's hashes, from the
    /// opens it took (`wire`'s `keep`).
    kept: Mutex<HashMap<String, Vec<u128>>>,
}

/// How long a look at a program session's updates waits for one or a wake.
#[derive(Clone, Copy)]
pub(crate) enum Look {
    Now,
    Until,
    Within(Duration),
}

enum Link {
    Plain(std::net::TcpStream),
    /// a fleet TLS link (native only: never made on the web)
    #[cfg_attr(target_arch = "wasm32", allow(dead_code))]
    Tls(makepad_network::SocketStream),
}

impl Link {
    fn set_read_timeout(&self, t: Duration) {
        let _ = match self {
            Link::Plain(s) => s.set_read_timeout(Some(t)),
            Link::Tls(s) => s.set_read_timeout(Some(t)),
        };
    }
    fn shutdown(&mut self) {
        match self {
            Link::Plain(s) => {
                let _ = s.shutdown(std::net::Shutdown::Both);
            }
            Link::Tls(s) => s.shutdown(),
        }
    }
}

impl std::io::Read for Link {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Link::Plain(s) => s.read(buf),
            Link::Tls(s) => s.read(buf),
        }
    }
}

impl std::io::Write for Link {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Link::Plain(s) => s.write(buf),
            Link::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Link::Plain(s) => s.flush(),
            Link::Tls(s) => s.flush(),
        }
    }
}

/// A hub on this machine's loopback surface: plain http to a loopback address.
fn on_loopback(url: &crate::http_client::ParsedUrl) -> bool {
    !url.https && url.host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The machine token header for a hub on this machine's loopback surface, which takes this
/// machine's apps by it; none for any other node.
fn machine_token_for(url: &crate::http_client::ParsedUrl) -> Result<Option<(String, String)>, String> {
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
fn open_link(base_url: &str) -> Result<(Link, Vec<u8>, bool), String> {
    use std::io::{Read, Write};
    let url = crate::http_client::parse_url(&format!("{base_url}/kernel/stream")).map_err(|e| e.to_string())?;
    let host_port = format!("{}:{}", url.host, url.port);
    makepad_network::lan::hub_check_host("ai-hub kernel stream", &url.host).map_err(|e| e.to_string())?;
    let (mut link, authorization): (Link, Option<String>) = if url.https {
        #[cfg(target_arch = "wasm32")]
        return Err(format!("{host_port}: fleet TLS links are native-only"));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (s, _server, auth) = crate::http_client::with_hub_connect(|| makepad_network::fleet::id::connect(&url.host, &url.port.to_string())).map_err(|e| format!("{host_port}: {e}"))?;
            (Link::Tls(s), Some(auth))
        }
    } else {
        let s = makepad_network::lan::hub_connect_host("ai-hub kernel stream", &url.host, url.port, Some(Duration::from_secs(5))).map_err(|e| format!("{host_port}: {e}"))?;
        let _ = s.set_nodelay(true);
        (Link::Plain(s), None)
    };
    let mut head = format!(
        "GET /kernel/stream HTTP/1.1\r\nHost: {host_port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: SxJdXBRtW7Q4awLDhflO0Q==\r\n"
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
            if !status.contains(" 101") {
                return Err(format!("{host_port}: no kernel stream ({})", status.trim()));
            }
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

/// How long after the last frame the link still reads with 1 ms timeouts.
const LINK_BUSY: Duration = Duration::from_millis(500);

/// How often an open the node has not answered asks whether it still matters.
const ASK_EVERY: Duration = Duration::from_millis(100);

/// The connection's thread: queued frames out, pushed frames in.
fn run_link(mut link: Link, leftover: Vec<u8>, out: std::sync::mpsc::Receiver<Vec<u8>>, shared: Arc<StreamShared>) {
    use makepad_network::web_socket_parser::{WebSocketMessage as Frame, WebSocketMessageFormat, WebSocketMessageHeader, WebSocketParser, SERVER_WEB_SOCKET_PONG_MESSAGE};
    use std::io::{Read, Write};
    let close = |why: String| {
        let mut s = shared.state.lock().unwrap();
        s.closed.get_or_insert(why);
        shared.cv.notify_all();
    };
    let mut parser = WebSocketParser::new();
    let mut pong = false;
    let mut ended = None;
    let feed = |parser: &mut WebSocketParser, bytes: &[u8], pong: &mut bool, ended: &mut Option<String>| {
        parser.parse(bytes, |r| match r {
            Ok(Frame::Binary(frame)) => stream_frame(&shared, frame),
            Ok(Frame::Ping(_)) => *pong = true,
            Ok(Frame::Close) => *ended = Some("the node closed the stream".into()),
            Ok(_) => {}
            Err(e) => *ended = Some(format!("stream frame: {e:?}")),
        });
    };
    feed(&mut parser, &leftover, &mut pong, &mut ended);
    let mut buf = vec![0u8; 64 * 1024];
    let mut timeout = Duration::ZERO;
    let mut last = Instant::now();
    loop {
        if let Some(why) = ended.take() {
            link.shutdown();
            return close(why);
        }
        loop {
            match out.try_recv() {
                Ok(frame) => {
                    last = Instant::now();
                    // one write per frame (one TLS record, no Nagle wait inside a frame)
                    let header = WebSocketMessageHeader::from_len(frame.len(), WebSocketMessageFormat::Binary, false);
                    let mut bytes = Vec::with_capacity(header.as_slice().len() + frame.len());
                    bytes.extend_from_slice(header.as_slice());
                    bytes.extend_from_slice(&frame);
                    if link.write_all(&bytes).is_err() {
                        link.shutdown();
                        return close("the stream broke while sending".into());
                    }
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    link.shutdown();
                    return close("the stream was dropped".into());
                }
            }
        }
        if std::mem::take(&mut pong) && link.write_all(&SERVER_WEB_SOCKET_PONG_MESSAGE).is_err() {
            link.shutdown();
            return close("the stream broke while sending".into());
        }
        let busy = shared.state.lock().unwrap().open > 0 || last.elapsed() < LINK_BUSY;
        let want = if busy { Duration::from_millis(1) } else { Duration::from_millis(10) };
        if want != timeout {
            timeout = want;
            link.set_read_timeout(timeout);
        }
        match link.read(&mut buf) {
            Ok(0) => {
                link.shutdown();
                return close("the node closed the stream".into());
            }
            Ok(n) => {
                last = Instant::now();
                feed(&mut parser, &buf[..n], &mut pong, &mut ended)
            }
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut | std::io::ErrorKind::Interrupted) => {}
            Err(e) => {
                link.shutdown();
                return close(format!("stream read: {e}"));
            }
        }
    }
}

#[derive(Default)]
struct StreamState {
    next_request: u64,
    closed: Option<String>,
    /// Opens waiting for their answer plus sessions open (the link reads with a short timeout
    /// while there are any).
    open: usize,
    /// Program opens answered, by request.
    opened: HashMap<u64, Result<(u64, Value), (u16, String)>>,
    /// Opens their opener stopped waiting for, by request, and the way out: a session one of
    /// them still gets is cancelled as its answer comes.
    given_up: HashMap<u64, std::sync::mpsc::Sender<Vec<u8>>>,
    /// Per session: the updates not yet taken (decoded by the taker), and a wake not yet seen.
    programs: HashMap<u64, (Vec<Vec<u8>>, bool)>,
}

#[derive(Default)]
struct StreamShared {
    state: Mutex<StreamState>,
    cv: Condvar,
}

impl KernelStream {
    /// Opens the stream (Err when the node has none).
    pub fn open(node: &KernelNode) -> Result<KernelStream, AssetAiError> {
        let (link, leftover, loopback) = open_link(&node.base_url).map_err(AssetAiError::Http)?;
        let shared = Arc::new(StreamShared::default());
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let reader = shared.clone();
        std::thread::Builder::new()
            .name("kernel-stream".into())
            .spawn(move || run_link(link, leftover, rx, reader))
            .map_err(|e| AssetAiError::Http(format!("kernel stream thread: {e}")))?;
        Ok(KernelStream { out: Mutex::new(tx), shared, loopback, kept: Mutex::new(HashMap::new()) })
    }

    fn send(&self, kind: u8, word: u64, payload: &[u8]) -> Result<(), AssetAiError> {
        self.out.lock().unwrap().send(stream_out(kind, word, payload)).map_err(|_| AssetAiError::Http("kernel stream: closed".into()))
    }

    pub fn is_closed(&self) -> bool {
        self.shared.state.lock().unwrap().closed.is_some()
    }

    /// How a program's inputs and outputs cross this stream unless its open says
    /// (`RemoteConfig::codec`): as they are on loopback (a copy: coding them only costs time),
    /// else delta-coded and compressed.
    pub fn codec(&self) -> Codec {
        match self.loopback {
            true => Codec::Raw,
            false => Codec::DeltaLz4,
        }
    }

    /// Opens `open` on the node: its session id and device, once the node answers or the stream
    /// breaks; None once `stale` says yes first (asked every [`ASK_EVERY`] while the node says
    /// nothing). The inputs the node keeps go by their hash; when it no longer keeps one it says
    /// so, and they all go again.
    pub(crate) fn program_open(&self, open: &wire::Open, threads: usize, stale: &mut dyn FnMut() -> bool) -> Result<Option<(u64, Value)>, AssetAiError> {
        let kept: std::collections::HashSet<u128> = self.kept.lock().unwrap().values().flatten().copied().collect();
        let (payload, hashes) = wire::encode_open(open, threads, &|h| kept.contains(&h));
        let opened = match self.open_request(&payload, stale)? {
            Some(Err((wire::NOT_KEPT, _))) => {
                self.kept.lock().unwrap().clear();
                self.open_request(&wire::encode_open(open, threads, &|_| false).0, stale)?
            }
            other => other,
        };
        let Some(opened) = opened else {
            return Ok(None);
        };
        let (id, device) = opened.map_err(|(status, why)| AssetAiError::Http(format!("program open: {status} {why}")))?;
        if !open.keep.is_empty() {
            self.kept.lock().unwrap().insert(open.keep.clone(), hashes);
        }
        Ok(Some((id, device)))
    }

    /// One `wire::OPEN` and the node's answer to it; None once `stale` says yes first.
    fn open_request(&self, payload: &[u8], stale: &mut dyn FnMut() -> bool) -> Result<Option<Result<(u64, Value), (u16, String)>>, AssetAiError> {
        let request = {
            let mut s = self.shared.state.lock().unwrap();
            s.next_request += 1;
            s.open += 1;
            s.next_request
        };
        self.send(wire::OPEN, request, payload)?;
        let mut ask = Instant::now() + ASK_EVERY;
        let mut s = self.shared.state.lock().unwrap();
        loop {
            if let Some(r) = s.opened.remove(&request) {
                return Ok(Some(r));
            }
            if let Some(why) = &s.closed {
                return Err(AssetAiError::Http(format!("kernel stream: {why}")));
            }
            // other sessions' updates wake this too: the time to ask is its own
            let now = Instant::now();
            if now < ask {
                s = self.shared.cv.wait_timeout(s, ask - now).unwrap().0;
                continue;
            }
            drop(s);
            let gone = stale();
            ask = Instant::now() + ASK_EVERY;
            s = self.shared.state.lock().unwrap();
            if gone && !s.opened.contains_key(&request) && s.closed.is_none() {
                s.given_up.insert(request, self.out.lock().unwrap().clone());
                s.open = s.open.saturating_sub(1);
                return Ok(None);
            }
        }
    }

    /// Queues a program frame for the writer (never blocks on the network).
    pub(crate) fn program_send(&self, kind: u8, session: u64, payload: Vec<u8>) {
        let _ = self.send(kind, session, &payload);
    }

    /// The session's updates not yet taken, waiting as `look` says for one or a wake; whether
    /// a wake came; and why the stream closed, if it did.
    pub(crate) fn program_take(&self, session: u64, look: Look) -> (Vec<Vec<u8>>, bool, Option<String>) {
        let until = match look {
            Look::Within(wait) => Some(Instant::now() + wait),
            Look::Now | Look::Until => None,
        };
        let mut s = self.shared.state.lock().unwrap();
        loop {
            let closed = s.closed.clone();
            let (frames, woken) = match s.programs.get_mut(&session) {
                Some((frames, woken)) => (std::mem::take(frames), std::mem::take(woken)),
                None => (Vec::new(), false),
            };
            if !frames.is_empty() || woken || matches!(look, Look::Now) || closed.is_some() {
                return (frames, woken, closed);
            }
            s = match until {
                None => self.shared.cv.wait(s).unwrap(),
                Some(at) => {
                    let left = at.saturating_duration_since(Instant::now());
                    if left.is_zero() {
                        return (frames, woken, closed);
                    }
                    self.shared.cv.wait_timeout(s, left).unwrap().0
                }
            };
        }
    }

    /// Makes the session's `program_take` (now or the next) return.
    pub(crate) fn program_wake(&self, session: u64) {
        if let Some((_, woken)) = self.shared.state.lock().unwrap().programs.get_mut(&session) {
            *woken = true;
        }
        self.shared.cv.notify_all();
    }

    /// A session the app is done with.
    pub(crate) fn program_forget(&self, session: u64) {
        let mut s = self.shared.state.lock().unwrap();
        if s.programs.remove(&session).is_some() {
            s.open = s.open.saturating_sub(1);
        }
    }
}

/// A frame for the writer: its kind, the word it is about, its payload.
fn stream_out(kind: u8, word: u64, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(9 + payload.len());
    frame.push(kind);
    frame.extend_from_slice(&word.to_le_bytes());
    frame.extend_from_slice(payload);
    frame
}

fn stream_frame(shared: &StreamShared, frame: &[u8]) {
    if frame.len() < 9 {
        return;
    }
    let word = u64::from_le_bytes(frame[1..9].try_into().unwrap());
    let mut s = shared.state.lock().unwrap();
    match frame[0] {
        wire::OPENED => {
            let r = wire::decode_opened(&frame[9..]).unwrap_or_else(|e| Err((502, e)));
            if let Some(out) = s.given_up.remove(&word) {
                if let Ok((id, _)) = r {
                    let _ = out.send(stream_out(wire::CANCEL, id, &[]));
                }
                return;
            }
            // the session's updates follow at once, maybe before its opener wakes: they queue
            match &r {
                Ok((id, _)) => {
                    s.programs.insert(*id, (Vec::new(), false));
                }
                Err(_) => s.open = s.open.saturating_sub(1),
            }
            s.opened.insert(word, r);
        }
        wire::UPDATE => {
            // Decoded by the session's taker, on its thread; a session the app dropped is gone.
            if let Some((q, _)) = s.programs.get_mut(&word) {
                q.push(frame[9..].to_vec());
            }
        }
        _ => return,
    }
    shared.cv.notify_all();
}

/// This machine's hubs that serve compute programs (their `~/.makepad/run` entries, on loopback,
/// where the machine token lets this machine's apps in), each with its info; an entry whose hub
/// is gone answers nothing and is left out. Blocking (one `GET /kernel` each).
#[cfg(not(target_arch = "wasm32"))]
pub fn machine_kernel_nodes() -> Vec<(KernelNode, KernelInfoJson)> {
    crate::machine::read_node_entries()
        .into_iter()
        .filter(|(_, entry)| entry.port > 0)
        .filter_map(|(_, entry)| {
            let node = KernelNode::new(&format!("http://127.0.0.1:{}", entry.port));
            node.info().ok().map(|info| (node, info))
        })
        .collect()
}

/// The fleet nodes heard on the LAN beacon plus `MAKEPAD_AI_KERNEL_NODES` (comma-separated
/// `host:port` fleet endpoints, for nodes no beacon reaches), each with its info; nodes that do
/// not serve programs are left out. Blocking (one `GET /kernel` per node, in parallel).
pub fn kernel_nodes() -> Vec<(KernelNode, KernelInfoJson)> {
    let mut urls: Vec<String> = Vec::new();
    if let Ok(list) = std::env::var("MAKEPAD_AI_KERNEL_NODES") {
        for host_port in list.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if host_port.starts_with("http://") {
                urls.push(host_port.to_string());
            } else {
                makepad_network::fleet::id::mark_fleet_endpoint(host_port);
                urls.push(format!("https://{host_port}"));
            }
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    for node in crate::discovery::start_listener().nodes() {
        if !urls.contains(&node.base_url) {
            urls.push(node.base_url);
        }
    }
    let probes: Vec<_> = urls
        .into_iter()
        .map(|url| {
            std::thread::spawn(move || {
                let node = KernelNode::new(&url);
                node.info().ok().map(|info| (node, info))
            })
        })
        .collect();
    probes.into_iter().filter_map(|p| p.join().ok().flatten()).collect()
}
