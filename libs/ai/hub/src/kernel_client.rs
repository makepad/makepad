//! The client side of the `kernel` domain (wire: [`crate::kernel_wire`]):
//! blocking calls for a worker thread, and the fleet's kernel nodes.
//!
//! A fleet URL (`https://host:port`, from the LAN beacon or
//! [`makepad_network::fleet::id::mark_fleet_endpoint`]) goes through the fleet TLS
//! front with this process's credential; a loopback URL (`http://`) is a
//! machine-local node.

use crate::error::AssetAiError;
use crate::http_client::{http_fetch, HttpClientRequest};
use crate::kernel_wire::*;
use makepad_micro_serde::{DeJson, SerJson};
use std::sync::atomic::{AtomicBool, Ordering};
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

    /// `GET /kernel`: Err when the node does not serve kernels.
    pub fn info(&self) -> Result<KernelInfoJson, AssetAiError> {
        let url = format!("{}/kernel", self.base_url);
        let response = http_fetch(&HttpClientRequest::get(&url))?;
        let status = response.status;
        let body = response.read_body_to_vec(1 << 20)?;
        if status != 200 {
            return Err(AssetAiError::Http(format!("{url}: {status} {}", String::from_utf8_lossy(&body))));
        }
        let text = String::from_utf8_lossy(&body);
        KernelInfoJson::deserialize_json(&text).map_err(|e| AssetAiError::Http(format!("{url}: bad kernel info: {e:?}")))
    }

    pub fn submit(&self, job: &KernelJobJson) -> Result<KernelSubmittedJson, AssetAiError> {
        let url = format!("{}/kernel/jobs", self.base_url);
        let body = job.serialize_json();
        let response = http_fetch(&HttpClientRequest::post(&url, "application/json", body.as_bytes()))?;
        let status = response.status;
        let body = response.read_body_to_vec(1 << 20)?;
        let text = String::from_utf8_lossy(&body);
        if status != 200 {
            return Err(AssetAiError::Http(format!("kernel submit: {status} {text}")));
        }
        KernelSubmittedJson::deserialize_json(&text).map_err(|e| AssetAiError::Http(format!("kernel submit: bad reply: {e:?}")))
    }

    /// The tiles finished since the last poll, waiting up to `timeout` for
    /// one; no output may be larger than `max_words`.
    pub fn poll(&self, job_id: u64, timeout: Duration, max_words: usize) -> Result<TileBatch, AssetAiError> {
        let url = format!("{}/kernel/job/{job_id}/tiles?timeout_ms={}", self.base_url, timeout.as_millis());
        let response = http_fetch(&HttpClientRequest::get(&url))?;
        let status = response.status;
        let body = response.read_body_to_vec(MAX_JOB_OUTPUT_WORDS * 4 + (1 << 20))?;
        if status != 200 {
            return Err(AssetAiError::Http(format!("kernel poll: {status} {}", String::from_utf8_lossy(&body))));
        }
        decode_batch(&body, max_words).map_err(AssetAiError::Http)
    }

    pub fn cancel(&self, job_id: u64) -> Result<(), AssetAiError> {
        let url = format!("{}/kernel/job/{job_id}/cancel", self.base_url);
        let response = http_fetch(&HttpClientRequest::post(&url, "application/json", b"{}"))?;
        let status = response.status;
        let _ = response.read_body_to_vec(1 << 16);
        if status != 200 && status != 404 {
            return Err(AssetAiError::Http(format!("kernel cancel: {status}")));
        }
        Ok(())
    }

    /// Submits and polls until every tile is back (a tool or test; apps
    /// poll from their own loop to cancel on the way).
    pub fn run(&self, job: &KernelJobJson, max_words: usize) -> Result<Vec<TileResult>, AssetAiError> {
        let submitted = self.submit(job)?;
        let mut out = Vec::with_capacity(job.tiles.len());
        loop {
            let batch = self.poll(submitted.job_id, Duration::from_millis(2000), max_words)?;
            out.extend(batch.tiles);
            if batch.done {
                return Ok(out);
            }
        }
    }
}

/// One long-lived stream to a node (`GET /kernel/stream`, a websocket over
/// the fleet TLS front): submits and cancels go up it, tiles are pushed down
/// it as they finish. No TLS handshake or poll per request.
///
/// One thread owns the connection (a TLS session cannot be split into a
/// reader and a writer): it writes what is queued, then reads for at most
/// 1 ms while jobs are open or frames moved in the last half second (10 ms
/// when idle), so a submit leaves within a millisecond while work flows.
pub struct KernelStream {
    out: Mutex<std::sync::mpsc::Sender<Vec<u8>>>,
    shared: Arc<StreamShared>,
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

/// Connects and upgrades; returns the link and any bytes read past the
/// upgrade response.
fn open_link(base_url: &str) -> Result<(Link, Vec<u8>), String> {
    use std::io::{Read, Write};
    let url = crate::http_client::parse_url(&format!("{base_url}/kernel/stream")).map_err(|e| e.to_string())?;
    let host_port = format!("{}:{}", url.host, url.port);
    let (mut link, authorization): (Link, Option<String>) = if url.https {
        #[cfg(target_arch = "wasm32")]
        return Err(format!("{host_port}: fleet TLS links are native-only"));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (s, _server, auth) = makepad_network::fleet::id::connect(&url.host, &url.port.to_string()).map_err(|e| format!("{host_port}: {e}"))?;
            (Link::Tls(s), Some(auth))
        }
    } else {
        let addr = std::net::ToSocketAddrs::to_socket_addrs(&host_port).map_err(|e| e.to_string())?.next().ok_or("no address")?;
        let s = makepad_network::lan::connect_timeout("ai-hub kernel stream", &addr, Duration::from_secs(5)).map_err(|e| format!("{host_port}: {e}"))?;
        let _ = s.set_nodelay(true);
        (Link::Plain(s), None)
    };
    let mut head = format!(
        "GET /kernel/stream HTTP/1.1\r\nHost: {host_port}\r\nConnection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: SxJdXBRtW7Q4awLDhflO0Q==\r\n"
    );
    if let Some(a) = authorization {
        head.push_str(&format!("Authorization: {a}\r\n"));
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
            return Ok((link, data[end + 4..].to_vec()));
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
    submitted: std::collections::HashMap<u64, Result<KernelSubmittedJson, String>>,
    batches: std::collections::HashMap<u64, std::collections::VecDeque<Result<TileBatch, String>>>,
    closed: Option<String>,
    /// Submits waiting for their answer plus jobs not done yet (the link
    /// reads with a short timeout while there are any).
    open: usize,
}

#[derive(Default)]
struct StreamShared {
    state: Mutex<StreamState>,
    cv: Condvar,
}

impl KernelStream {
    /// Opens the stream (Err when the node has none: an older node).
    pub fn open(node: &KernelNode) -> Result<KernelStream, AssetAiError> {
        let (link, leftover) = open_link(&node.base_url).map_err(AssetAiError::Http)?;
        let shared = Arc::new(StreamShared::default());
        let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
        let reader = shared.clone();
        std::thread::Builder::new()
            .name("kernel-stream".into())
            .spawn(move || run_link(link, leftover, rx, reader))
            .map_err(|e| AssetAiError::Http(format!("kernel stream thread: {e}")))?;
        Ok(KernelStream { out: Mutex::new(tx), shared })
    }

    fn send(&self, kind: u8, word: u64, payload: &[u8]) -> Result<(), AssetAiError> {
        let mut frame = Vec::with_capacity(9 + payload.len());
        frame.push(kind);
        frame.extend_from_slice(&word.to_le_bytes());
        frame.extend_from_slice(payload);
        self.out.lock().unwrap().send(frame).map_err(|_| AssetAiError::Http("kernel stream: closed".into()))
    }

    pub fn is_closed(&self) -> bool {
        self.shared.state.lock().unwrap().closed.is_some()
    }

    pub fn submit(&self, job: &KernelJobJson) -> Result<KernelSubmittedJson, AssetAiError> {
        let request = {
            let mut s = self.shared.state.lock().unwrap();
            s.next_request += 1;
            s.open += 1;
            s.next_request
        };
        self.send(STREAM_SUBMIT, request, job.serialize_json().as_bytes())?;
        let deadline = Instant::now() + Duration::from_secs(60);
        let mut s = self.shared.state.lock().unwrap();
        loop {
            if let Some(r) = s.submitted.remove(&request) {
                if r.is_err() {
                    s.open = s.open.saturating_sub(1);
                }
                return r.map_err(AssetAiError::Http);
            }
            if let Some(why) = &s.closed {
                return Err(AssetAiError::Http(format!("kernel stream: {why}")));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(AssetAiError::Http("kernel stream: no answer to a submit".into()));
            }
            s = self.shared.cv.wait_timeout(s, deadline - now).unwrap().0;
        }
    }

    /// The next batch the node pushed for `job_id`, waiting up to `timeout`
    /// (an empty, not-done batch when nothing came).
    pub fn next_batch(&self, job_id: u64, timeout: Duration) -> Result<TileBatch, AssetAiError> {
        let deadline = Instant::now() + timeout;
        let mut s = self.shared.state.lock().unwrap();
        loop {
            if let Some(b) = s.batches.get_mut(&job_id).and_then(|q| q.pop_front()) {
                if b.as_ref().is_ok_and(|b| b.done) {
                    s.batches.remove(&job_id);
                    s.open = s.open.saturating_sub(1);
                }
                return b.map_err(AssetAiError::Http);
            }
            if let Some(why) = &s.closed {
                return Err(AssetAiError::Http(format!("kernel stream: {why}")));
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(TileBatch::default());
            }
            s = self.shared.cv.wait_timeout(s, deadline - now).unwrap().0;
        }
    }

    /// Never blocks on the network (the frame is queued for the writer).
    pub fn cancel(&self, job_id: u64) {
        let _ = self.send(STREAM_CANCEL, job_id, &[]);
    }
}

fn stream_frame(shared: &StreamShared, frame: &[u8]) {
    if frame.len() < 9 {
        return;
    }
    let word = u64::from_le_bytes(frame[1..9].try_into().unwrap());
    let mut s = shared.state.lock().unwrap();
    match frame[0] {
        STREAM_SUBMITTED if frame.len() >= 11 => {
            let status = u16::from_le_bytes([frame[9], frame[10]]);
            let text = String::from_utf8_lossy(&frame[11..]);
            let r = if status == 200 {
                KernelSubmittedJson::deserialize_json(&text).map_err(|e| format!("kernel submit: bad reply: {e:?}"))
            } else {
                Err(format!("kernel submit: {status} {text}"))
            };
            s.submitted.insert(word, r);
        }
        STREAM_BATCH => {
            let b = decode_batch(&frame[9..], MAX_TILE_OUTPUT_WORDS);
            s.batches.entry(word).or_default().push_back(b);
        }
        _ => return,
    }
    shared.cv.notify_all();
}

/// The fleet nodes heard on the LAN beacon plus `MAKEPAD_AI_KERNEL_NODES`
/// (comma-separated `host:port` fleet endpoints, for nodes no beacon
/// reaches), each with its kernel info; nodes that do not serve kernels
/// are left out. Blocking (one `GET /kernel` per node, in parallel).
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

/// Tiles for a node: one job (its tiles filled) and the caller's tag of
/// each tile, `tags[i]` for `job.tiles[i]`.
pub struct Batch<T> {
    pub job: KernelJobJson,
    pub tags: Vec<T>,
}

/// Where a [`KernelFleet`] gets its tiles: up to `max` of them, or None
/// when there is nothing to do now.
pub type TileSource<T> = Arc<dyn Fn(usize) -> Option<Batch<T>> + Send + Sync>;
/// Where every tile comes back, exactly once: (node index, tag, result).
/// A tile that never ran (its job was cancelled, or the node failed) comes
/// back with status [`STATUS_CANCELLED`] or [`STATUS_FAILED`] and no outputs.
pub type TileSink<T> = Arc<dyn Fn(usize, T, TileResult) + Send + Sync>;

/// One node's numbers, for a status display.
#[derive(Clone, Debug, Default)]
pub struct NodeStats {
    pub url: String,
    pub cores: u32,
    pub arch: String,
    pub tiles_done: u64,
    /// Over the last few seconds.
    pub tiles_per_sec: f64,
    pub in_flight: usize,
    pub failures: u64,
    pub last_error: Option<String>,
    /// Frame-tail copies this node took of tiles out elsewhere.
    pub copies: u64,
    /// Tiles this node answered while another copy was still out.
    pub won: u64,
}

/// A tile the fleet holds from the source until it is answered: sent to
/// one node, and near a frame's end maybe to a second (speculatively); the
/// first good result is the answer, the other copy is cancelled.
struct Entry<T> {
    tag: T,
    /// Its tile, `id` = the entry's sequence number.
    tile: KernelTileJson,
    /// The job it came in, without tiles (a copy is sent with the same).
    template: Arc<KernelJobJson>,
    /// When its first copy went out.
    sent: Instant,
    /// The copies out now: (node, job id).
    copies: Vec<(usize, u64)>,
    /// A cancel found it stale (a job still being submitted with it is
    /// cancelled as soon as it has an id).
    stale: bool,
}

/// A job out on a node: its tiles and how it went up.
struct Out {
    seqs: Vec<u64>,
    via: Option<Arc<KernelStream>>,
}

struct Book<T> {
    next: u64,
    entries: std::collections::HashMap<u64, Entry<T>>,
    jobs: std::collections::HashMap<(usize, u64), Out>,
}

struct NodeState {
    node: KernelNode,
    stats: Mutex<NodeStats>,
    /// (when, tiles) of recent finishes, for tiles/s.
    recent: Mutex<std::collections::VecDeque<(Instant, u32)>>,
    /// Tiles handed to the node and not back yet (copies included).
    in_flight: std::sync::atomic::AtomicUsize,
    /// False while the node backs off after a failure (it then takes no
    /// share of new tiles, so the others are not held to it).
    ready: AtomicBool,
    /// A tile's time on the node's worker, smoothed (seconds; 0: none yet).
    tile_secs: Mutex<f64>,
    /// The node's stream, opened on first use; None after a failure until
    /// the next job tries again (an older node has none: requests then).
    stream: Mutex<Option<Arc<KernelStream>>>,
    /// When opening the stream last failed.
    stream_retry: Mutex<Option<Instant>>,
}

struct FleetShared<T> {
    nodes: Vec<Arc<NodeState>>,
    source: TileSource<T>,
    sink: TileSink<T>,
    book: Mutex<Book<T>>,
    wake: (Mutex<u64>, Condvar),
    quit: AtomicBool,
}

/// Remote tiles across kernel nodes, taken from the caller's source in its
/// order. Each node holds at most its cores plus a quarter of tiles (so a
/// core that finishes finds the next tile already there), in
/// [`SLOTS_PER_NODE`] jobs. When the source is empty (a frame's tail), an
/// idle node takes a copy of a tile that has been out on another node for
/// well over that node's tile time; the first good result answers it and
/// the other copy is cancelled (kernels are bit-identical everywhere, so
/// either copy is the tile). Every tag is answered exactly once.
/// A node's `cores` is what it reports: cap a busy node on the node itself
/// (`MAKEPAD_AI_KERNEL_THREADS`).
/// The caller's own threads stay its business: a fleet only adds CPUs.
pub struct KernelFleet<T: Clone + Send + 'static> {
    shared: Arc<FleetShared<T>>,
    threads: Vec<std::thread::JoinHandle<()>>,
}

/// Jobs a node has in flight at once, each of a quarter of the node's tile
/// allowance: while one waits for its slowest tile, the others keep the
/// node's cores fed.
const SLOTS_PER_NODE: usize = 4;
const RATE_WINDOW: Duration = Duration::from_secs(3);
/// A tile is copied to an idle node once it has been out this many of its
/// node's tile times (plus [`SPECULATE_SLACK`]).
const SPECULATE_AFTER: f64 = 2.0;
const SPECULATE_SLACK: Duration = Duration::from_millis(50);

impl<T: Clone + Send + 'static> KernelFleet<T> {
    pub fn start(nodes: Vec<(KernelNode, KernelInfoJson)>, source: TileSource<T>, sink: TileSink<T>) -> Self {
        let nodes: Vec<Arc<NodeState>> = nodes
            .into_iter()
            .map(|(node, info)| {
                Arc::new(NodeState {
                    stats: Mutex::new(NodeStats { url: node.base_url.clone(), cores: info.cores, arch: info.arch.clone(), ..NodeStats::default() }),
                    node,
                    recent: Mutex::new(std::collections::VecDeque::new()),
                    in_flight: std::sync::atomic::AtomicUsize::new(0),
                    ready: AtomicBool::new(true),
                    tile_secs: Mutex::new(0.0),
                    stream: Mutex::new(None),
                    stream_retry: Mutex::new(None),
                })
            })
            .collect();
        let book = Book { next: 1, entries: std::collections::HashMap::new(), jobs: std::collections::HashMap::new() };
        let shared = Arc::new(FleetShared { nodes, source, sink, book: Mutex::new(book), wake: (Mutex::new(0), Condvar::new()), quit: AtomicBool::new(false) });
        let mut threads = Vec::new();
        for n in 0..shared.nodes.len() {
            for s in 0..SLOTS_PER_NODE {
                let shared = shared.clone();
                if let Ok(t) = std::thread::Builder::new().name(format!("kernel-fleet-{n}-{s}")).spawn(move || drive(&shared, n)) {
                    threads.push(t);
                }
            }
        }
        KernelFleet { shared, threads }
    }

    /// New tiles are in the source.
    pub fn notify(&self) {
        let (m, c) = &self.shared.wake;
        *m.lock().unwrap() += 1;
        c.notify_all();
    }

    /// Cancels every job out whose tiles are all `stale` or answered (their
    /// tiles come back cancelled through the sink).
    pub fn cancel(&self, stale: impl Fn(&T) -> bool) {
        let mut book = self.shared.book.lock().unwrap();
        for e in book.entries.values_mut() {
            if stale(&e.tag) {
                e.stale = true;
            }
        }
        let mut cancels = Vec::new();
        for ((node, job_id), out) in &book.jobs {
            if out.seqs.iter().all(|q| book.entries.get(q).is_none_or(|e| e.stale)) {
                cancels.push((*node, *job_id, out.via.clone()));
            }
        }
        drop(book);
        for (node, job_id, via) in cancels {
            cancel_job(&self.shared.nodes[node], job_id, via);
        }
    }

    pub fn stats(&self) -> Vec<NodeStats> {
        self.shared
            .nodes
            .iter()
            .map(|n| {
                let mut s = n.stats.lock().unwrap().clone();
                let mut recent = n.recent.lock().unwrap();
                while recent.front().is_some_and(|(t, _)| t.elapsed() > RATE_WINDOW) {
                    recent.pop_front();
                }
                s.tiles_per_sec = recent.iter().map(|(_, k)| *k as f64).sum::<f64>() / RATE_WINDOW.as_secs_f64();
                s.in_flight = n.in_flight.load(Ordering::SeqCst);
                s
            })
            .collect()
    }
}

impl<T: Clone + Send + 'static> Drop for KernelFleet<T> {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::SeqCst);
        self.notify();
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

fn not_run(id: u64, status: u32, message: &str) -> TileResult {
    TileResult { id, status, elements: 0, work: 0, nanos: 0, reduced: Vec::new(), outputs: Vec::new(), message: message.to_string() }
}

/// Off the caller's thread when it has to go by request.
fn cancel_job(node: &NodeState, job_id: u64, via: Option<Arc<KernelStream>>) {
    match via {
        Some(stream) => stream.cancel(job_id),
        None => {
            let node = node.node.clone();
            let _ = std::thread::Builder::new().name("kernel-fleet-cancel".into()).spawn(move || {
                let _ = node.cancel(job_id);
            });
        }
    }
}

/// Takes tiles from the source into the book; the job to send (tile ids =
/// sequence numbers).
/// A node's speed for sharing tiles: its cores over its smoothed tile time
/// (a node starved by its own work is slow per tile), its cores until known.
fn speed(o: &NodeState) -> f64 {
    let cores = o.stats.lock().unwrap().cores.max(1) as f64;
    let t = *o.tile_secs.lock().unwrap();
    if t > 0.0 {
        cores / t
    } else {
        cores
    }
}

/// The most tiles node `n` may hold now: as many per unit of speed as the
/// least loaded other ready node holds, plus one job's worth.
fn fair_share<T>(shared: &FleetShared<T>, n: usize, slot: usize) -> usize {
    let mut least: Option<f64> = None;
    for (k, o) in shared.nodes.iter().enumerate() {
        if k == n || !o.ready.load(Ordering::SeqCst) {
            continue;
        }
        let load = o.in_flight.load(Ordering::SeqCst) as f64 / speed(o);
        least = Some(least.map_or(load, |l: f64| l.min(load)));
    }
    match least {
        Some(l) => (l * speed(&shared.nodes[n])) as usize + slot,
        None => usize::MAX,
    }
}

fn take_new<T: Clone + Send + 'static>(shared: &FleetShared<T>, want: usize) -> Option<(KernelJobJson, Vec<u64>)> {
    let Batch { mut job, tags } = (shared.source)(want)?;
    let tiles = std::mem::take(&mut job.tiles);
    let template = Arc::new(job);
    let mut book = shared.book.lock().unwrap();
    let mut seqs = Vec::with_capacity(tiles.len());
    let mut sent = Vec::with_capacity(tiles.len());
    for (mut tile, tag) in tiles.into_iter().zip(tags) {
        let seq = book.next;
        book.next += 1;
        tile.id = seq;
        sent.push(tile.clone());
        book.entries.insert(seq, Entry { tag, tile, template: template.clone(), sent: Instant::now(), copies: Vec::new(), stale: false });
        seqs.push(seq);
    }
    let mut job = (*template).clone();
    job.tiles = sent;
    Some((job, seqs))
}

/// The frame's tail: copies of tiles out on other nodes for well over their
/// tile time, for node `n` (same job template, at most `want`).
/// `MAKEPAD_AI_KERNEL_SPECULATE=0` turns it off (A/B).
fn take_copies<T: Clone + Send + 'static>(shared: &FleetShared<T>, n: usize, want: usize) -> Option<(KernelJobJson, Vec<u64>)> {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *OFF.get_or_init(|| std::env::var("MAKEPAD_AI_KERNEL_SPECULATE").is_ok_and(|v| v == "0")) {
        return None;
    }
    let book = shared.book.lock().unwrap();
    let mut late: Vec<(&u64, &Entry<T>)> = book
        .entries
        .iter()
        .filter(|(_, e)| {
            if e.stale {
                return false;
            }
            let [(holder, _)] = e.copies[..] else { return false };
            if holder == n {
                return false;
            }
            let t = *shared.nodes[holder].tile_secs.lock().unwrap();
            t > 0.0 && e.sent.elapsed() > Duration::from_secs_f64(t * SPECULATE_AFTER) + SPECULATE_SLACK
        })
        .collect();
    if late.is_empty() {
        return None;
    }
    late.sort_by_key(|(_, e)| e.sent);
    let template = late[0].1.template.clone();
    let picked: Vec<(u64, KernelTileJson)> = late.iter().filter(|(_, e)| Arc::ptr_eq(&e.template, &template)).take(want).map(|(q, e)| (**q, e.tile.clone())).collect();
    drop(book);
    let mut job = (*template).clone();
    job.tiles = picked.iter().map(|(_, t)| t.clone()).collect();
    Some((job, picked.into_iter().map(|(q, _)| q).collect()))
}

/// A copy's result: the answer when it is the first good one (or the last
/// copy out); the other copies of an answered tile are cancelled once their
/// jobs have nothing left to answer.
fn deliver<T: Clone + Send + 'static>(shared: &FleetShared<T>, n: usize, job_id: u64, seq: u64, r: TileResult) {
    let mut book = shared.book.lock().unwrap();
    let Some(entry) = book.entries.get_mut(&seq) else { return };
    entry.copies.retain(|c| *c != (n, job_id));
    if r.status != STATUS_OK && !entry.copies.is_empty() {
        return;
    }
    let entry = book.entries.remove(&seq).unwrap();
    if !entry.copies.is_empty() {
        shared.nodes[n].stats.lock().unwrap().won += 1;
    }
    let mut cancels = Vec::new();
    for other in &entry.copies {
        if let Some(out) = book.jobs.get(other) {
            if out.seqs.iter().all(|q| !book.entries.contains_key(q)) {
                cancels.push((other.0, other.1, out.via.clone()));
            }
        }
    }
    drop(book);
    for (node, job, via) in cancels {
        cancel_job(&shared.nodes[node], job, via);
    }
    (shared.sink)(n, entry.tag, r);
}

fn drive<T: Clone + Send + 'static>(shared: &FleetShared<T>, n: usize) {
    let state = &shared.nodes[n];
    let cores = state.stats.lock().unwrap().cores.max(1) as usize;
    let mut backoff = Duration::ZERO;
    while !shared.quit.load(Ordering::SeqCst) {
        if !backoff.is_zero() {
            state.ready.store(false, Ordering::SeqCst);
            std::thread::sleep(backoff);
        }
        state.ready.store(true, Ordering::SeqCst);
        let seen = *shared.wake.0.lock().unwrap();
        let allowance = cores + cores / 4;
        let mine = state.in_flight.load(Ordering::SeqCst);
        let room = allowance.saturating_sub(mine);
        let slot = allowance.div_ceil(SLOTS_PER_NODE);
        let want = room.min(slot);
        // new tiles go round the nodes by speed: none holds more than one
        // job's worth over the least loaded ready node, so a small frame is
        // spread over every node instead of the first one to ask taking it all
        let fair = fair_share(shared, n, slot).saturating_sub(mine);
        let picked = if want == 0 || fair == 0 {
            None
        } else {
            take_new(shared, want.min(fair)).or_else(|| {
                let copies = take_copies(shared, n, want)?;
                state.stats.lock().unwrap().copies += copies.1.len() as u64;
                Some(copies)
            })
        };
        let Some((job, seqs)) = picked else {
            let (m, c) = &shared.wake;
            let g = m.lock().unwrap();
            if *g == seen && !shared.quit.load(Ordering::SeqCst) {
                let _ = c.wait_timeout(g, Duration::from_millis(10));
            }
            continue;
        };
        state.in_flight.fetch_add(seqs.len(), Ordering::SeqCst);
        let via = stream_for(state);
        let submitted = match &via {
            Some(stream) => stream.submit(&job),
            None => state.node.submit(&job),
        };
        let job_id = match submitted {
            Ok(sub) => sub.job_id,
            Err(e) => {
                if via.is_some() {
                    *state.stream.lock().unwrap() = None;
                }
                note_failure(state, &e.to_string());
                for q in &seqs {
                    state.in_flight.fetch_sub(1, Ordering::SeqCst);
                    deliver_unsent(shared, n, *q, &e.to_string());
                }
                backoff = (backoff * 2).clamp(Duration::from_millis(250), Duration::from_secs(5));
                continue;
            }
        };
        backoff = Duration::ZERO;
        {
            let mut book = shared.book.lock().unwrap();
            for q in &seqs {
                if let Some(e) = book.entries.get_mut(q) {
                    e.copies.push((n, job_id));
                }
            }
            book.jobs.insert((n, job_id), Out { seqs: seqs.clone(), via: via.clone() });
            // a cancel that came while it was being submitted
            if seqs.iter().all(|q| book.entries.get(q).is_none_or(|e| e.stale)) {
                drop(book);
                cancel_job(state, job_id, via.clone());
            }
        }
        let mut open: Vec<u64> = seqs.clone();
        let result = poll_job(shared, state, n, job_id, &mut open, via.as_deref());
        if let Err(e) = &result {
            match &via {
                Some(stream) => {
                    stream.cancel(job_id);
                    *state.stream.lock().unwrap() = None;
                }
                None => {
                    let _ = state.node.cancel(job_id);
                }
            }
            note_failure(state, e);
            backoff = Duration::from_millis(250);
        }
        // tiles the job never answered: cancelled (job over) or failed
        let (status, why) = match &result {
            Ok(()) => (STATUS_CANCELLED, "cancelled before it ran".to_string()),
            Err(e) => (STATUS_FAILED, e.clone()),
        };
        for q in open {
            state.in_flight.fetch_sub(1, Ordering::SeqCst);
            deliver(shared, n, job_id, q, not_run(q, status, &why));
        }
        shared.book.lock().unwrap().jobs.remove(&(n, job_id));
    }
}

/// A tile whose copy never went out (the submit failed).
fn deliver_unsent<T: Clone + Send + 'static>(shared: &FleetShared<T>, n: usize, seq: u64, why: &str) {
    let mut book = shared.book.lock().unwrap();
    let Some(entry) = book.entries.get(&seq) else { return };
    if !entry.copies.is_empty() {
        return;
    }
    let entry = book.entries.remove(&seq).unwrap();
    drop(book);
    (shared.sink)(n, entry.tag, not_run(seq, STATUS_FAILED, why));
}

fn note_failure(state: &NodeState, message: &str) {
    let mut st = state.stats.lock().unwrap();
    st.failures += 1;
    st.last_error = Some(message.to_string());
}

fn poll_job<T: Clone + Send + 'static>(shared: &FleetShared<T>, state: &NodeState, n: usize, job_id: u64, open: &mut Vec<u64>, via: Option<&KernelStream>) -> Result<(), String> {
    loop {
        let batch = match via {
            Some(stream) => stream.next_batch(job_id, Duration::from_millis(1000)),
            None => state.node.poll(job_id, Duration::from_millis(1000), MAX_TILE_OUTPUT_WORDS),
        }
        .map_err(|e| e.to_string())?;
        let mut ok = 0u32;
        for tile in batch.tiles {
            let Some(k) = open.iter().position(|q| *q == tile.id) else { continue };
            open.swap_remove(k);
            state.in_flight.fetch_sub(1, Ordering::SeqCst);
            if tile.status == STATUS_OK {
                ok += 1;
                let secs = tile.nanos as f64 * 1e-9;
                let mut t = state.tile_secs.lock().unwrap();
                *t = if *t > 0.0 { 0.8 * *t + 0.2 * secs } else { secs };
            }
            deliver(shared, n, job_id, tile.id, tile);
        }
        if ok > 0 {
            state.stats.lock().unwrap().tiles_done += ok as u64;
            state.recent.lock().unwrap().push_back((Instant::now(), ok));
        }
        if batch.done {
            return Ok(());
        }
    }
}

/// The node's stream: the open one, or a new one (None when it cannot be
/// opened: the job goes by request, and the stream is tried again later).
/// `MAKEPAD_AI_KERNEL_STREAM=0` sends every job by request (A/B, old nodes).
fn stream_for(state: &NodeState) -> Option<Arc<KernelStream>> {
    static OFF: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if *OFF.get_or_init(|| std::env::var("MAKEPAD_AI_KERNEL_STREAM").is_ok_and(|v| v == "0")) {
        return None;
    }
    let mut slot = state.stream.lock().unwrap();
    if let Some(stream) = slot.as_ref().filter(|s| !s.is_closed()) {
        return Some(stream.clone());
    }
    let mut retry = state.stream_retry.lock().unwrap();
    if retry.is_some_and(|t| t.elapsed() < STREAM_RETRY) {
        return None;
    }
    match KernelStream::open(&state.node) {
        Ok(stream) => {
            *retry = None;
            let stream = Arc::new(stream);
            *slot = Some(stream.clone());
            Some(stream)
        }
        Err(_) => {
            *retry = Some(Instant::now());
            *slot = None;
            None
        }
    }
}

const STREAM_RETRY: Duration = Duration::from_secs(10);
