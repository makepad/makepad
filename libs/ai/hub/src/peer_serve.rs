//! The authenticated model-blob endpoint, served on the ONE existing service
//! port (hard invariant: one service process per box, no sidecar, no second
//! listener).
//!
//! `GET /v1/model_blob/<sha256>` with headers:
//! - `X-Peer-Ticket: <ticket>` — a [`crate::peer::PeerTicket`] scoped
//!   to (this source node, the claimed receiver node, this exact digest).
//!   (`Authorization` carries the receiver's fleet credential, role `node`,
//!   checked by the TLS front first.)
//! - `X-Peer-Receiver: <receiver node_key>` — must equal the ticket scope.
//! - `Range: bytes=<from>-[<to>]` — optional resume offset.
//!
//! The in-repo `makepad-network` HttpServer buffers whole responses in
//! memory, so this endpoint never streams an entire model file per request:
//! every response body is AT MOST `chunk_max` bytes (default 32 MiB). Small
//! files answer `200` with the full body; everything else answers `206` with
//! an exact `Content-Range`, and the receiver loops ranged requests until the
//! file is complete. `Content-Length` is always exact, `ETag` is the strong
//! content address (`"sha256:<digest>"`), `Accept-Ranges: bytes`.
//!
//! Bounds: up to `max_serves` workers read chunks; excess requests queue,
//! the digest path segment must be exactly 64 hex chars (no other addressing
//! exists — path traversal has no surface), query strings are refused,
//! per-response memory is `chunk_max`, and an optional fleet-wide MB/s
//! throttle paces workers. Every chunk read holds a [`ServeLeases`] lease and
//! re-verifies the artifact's receipt first, so eviction/replacement can
//! neither clobber an in-flight source nor let an invalid file leak out.

use crate::peer::{find_verified_blob, ServeLeases, TransferSecret};
use crate::protocol::ErrorJson;
use crate::server::{RouteWorker, ServiceShared};
use makepad_micro_serde::SerJson;
use makepad_network::{HttpServerHeaders, HttpServerResponse};
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub const BLOB_PATH_PREFIX: &str = "/v1/model_blob/";

pub const DEFAULT_CHUNK_MAX: u64 = 32 * 1024 * 1024;
pub const DEFAULT_MAX_SERVES: usize = 3;

// ---------------------------------------------------------------------------
// Options + runtime state
// ---------------------------------------------------------------------------

/// Peer-lane options on [`crate::server::ServiceConfig`]. Every `None` falls
/// back to env, then to the built-in default — tests inject explicit values
/// so they never race on process-global env.
#[derive(Clone, Default)]
pub struct PeerOptions {
    /// Transfer secret override (else `MAKEPAD_AI_PEER_SECRET` env, else the
    /// cache-dir `peer-secret` file). Never logged.
    pub secret: Option<String>,
    /// Serve blobs at all (else `MAKEPAD_AI_PEER_SERVE` env, "off" disables;
    /// default on). Serving additionally requires a secret — fail closed.
    pub serve: Option<bool>,
    /// Max bytes per blob response (else `MAKEPAD_AI_PEER_CHUNK_MB`).
    pub chunk_max_bytes: Option<u64>,
    /// Blob worker count (else `MAKEPAD_AI_PEER_MAX_SERVES`); excess serves queue.
    pub max_serves: Option<usize>,
    /// Serve-side bandwidth pacing in MB/s (else `MAKEPAD_AI_PEER_MBPS`;
    /// absent/0 = unpaced).
    pub mbps: Option<f64>,
    /// Peer source base URLs for THIS box's own downloads (else
    /// `MAKEPAD_AI_PEER_SOURCES` env). Request fields still take precedence
    /// per job.
    pub sources: Option<Vec<String>>,
}

impl std::fmt::Debug for PeerOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PeerOptions{{secret:{}, serve:{:?}, chunk_max_bytes:{:?}, max_serves:{:?}, mbps:{:?}, sources:{:?}}}",
            if self.secret.is_some() { "present" } else { "absent" },
            self.serve,
            self.chunk_max_bytes,
            self.max_serves,
            self.mbps,
            self.sources
        )
    }
}

/// Resolved peer-lane state held by [`ServiceShared`].
pub struct PeerRuntime {
    pub secret: Option<TransferSecret>,
    pub serve: bool,
    pub leases: ServeLeases,
    pub chunk_max: u64,
    /// Bounds concurrent chunk buffers; additional requests wait for a worker.
    pub max_serves: usize,
    serves_in_flight: AtomicUsize,
    #[cfg(test)]
    read_gate: Option<mpsc::Sender<(std::thread::ThreadId, mpsc::Sender<()>)>>,
    pub throttle: Option<Throttle>,
    /// Operator-injected peer sources this box may download from.
    pub env_sources: Vec<String>,
}

impl PeerRuntime {
    pub fn resolve(options: &PeerOptions, cache_dir: &Path) -> Self {
        let secret = TransferSecret::resolve(options.secret.as_deref(), cache_dir);
        let serve = options.serve.unwrap_or_else(|| {
            !matches!(
                std::env::var("MAKEPAD_AI_PEER_SERVE").as_deref(),
                Ok("off") | Ok("0") | Ok("false")
            )
        });
        // A setting outside the supported range is clamped, and said so once at startup.
        let wanted_chunk = options
            .chunk_max_bytes
            .or_else(|| env_parse::<u64>("MAKEPAD_AI_PEER_CHUNK_MB").map(|mb| mb.saturating_mul(1024 * 1024)))
            .unwrap_or(DEFAULT_CHUNK_MAX);
        let chunk_max = wanted_chunk.clamp(64 * 1024, 256 * 1024 * 1024);
        if chunk_max != wanted_chunk {
            eprintln!("peer-cache: chunk size {wanted_chunk} B clamped to {chunk_max} B (supported: 64 KiB to 256 MiB)");
        }
        let wanted_serves = options
            .max_serves
            .or_else(|| env_parse::<usize>("MAKEPAD_AI_PEER_MAX_SERVES"))
            .unwrap_or(DEFAULT_MAX_SERVES);
        let max_serves = wanted_serves.clamp(1, 64);
        if max_serves != wanted_serves {
            eprintln!("peer-cache: {wanted_serves} blob workers clamped to {max_serves} (supported: 1 to 64)");
        }
        let mbps = options
            .mbps
            .or_else(|| env_parse::<f64>("MAKEPAD_AI_PEER_MBPS"))
            .filter(|v| v.is_finite() && *v > 0.0);
        let env_sources = options
            .sources
            .clone()
            .unwrap_or_else(crate::peer::PeerPlan::env_sources);
        Self {
            secret,
            serve,
            leases: ServeLeases::new(),
            chunk_max,
            max_serves,
            serves_in_flight: AtomicUsize::new(0),
            #[cfg(test)]
            read_gate: None,
            throttle: mbps.map(Throttle::mb_per_sec),
            env_sources,
        }
    }

    pub(crate) fn start_pool(&self) -> Option<RouteWorker> {
        // A paired phone may fetch without the node-to-node transfer secret.
        self.serve.then(|| RouteWorker::new("peer-blob", "blob worker unavailable", self.max_serves))
    }

    /// Serving is on only with networked hub permission, configured on AND a secret (fail
    /// closed: no secret means no authenticated caller can exist).
    pub fn serving_enabled(&self) -> bool {
        self.serve && self.secret.is_some() && makepad_network::lan::hub_service("ai-hub peer serve")
    }
}

fn env_parse<T: std::str::FromStr>(name: &str) -> Option<T> {
    std::env::var(name).ok()?.trim().parse().ok()
}

// ---------------------------------------------------------------------------
// Bandwidth throttle (global leaky bucket across serve workers)
// ---------------------------------------------------------------------------

pub struct Throttle {
    bytes_per_sec: f64,
    next_free: Mutex<Instant>,
}

impl Throttle {
    pub fn mb_per_sec(mbps: f64) -> Self {
        Self {
            bytes_per_sec: mbps * 1024.0 * 1024.0,
            next_free: Mutex::new(Instant::now()),
        }
    }

    pub fn mb_per_sec_value(&self) -> f64 {
        self.bytes_per_sec / (1024.0 * 1024.0)
    }

    /// Reserves transmission time for `len` bytes and sleeps until this
    /// worker's reservation starts. First chunk goes immediately; sustained
    /// load converges to the configured rate.
    pub fn pace(&self, len: u64) {
        let wait = {
            let mut next_free = self.next_free.lock().unwrap();
            let now = Instant::now();
            let start = (*next_free).max(now);
            let cost = Duration::from_secs_f64(len as f64 / self.bytes_per_sec);
            *next_free = start + cost;
            start.saturating_duration_since(now)
        };
        if !wait.is_zero() {
            std::thread::sleep(wait);
        }
    }
}

// ---------------------------------------------------------------------------
// Request handling
// ---------------------------------------------------------------------------

fn header_value<'a>(headers: &'a HttpServerHeaders, name: &str) -> Option<&'a str> {
    for line in &headers.lines {
        let line = line.trim_end();
        if line.len() > name.len()
            && line.as_bytes()[name.len()] == b':'
            && line[..name.len()].eq_ignore_ascii_case(name)
        {
            return Some(line[name.len() + 1..].trim());
        }
    }
    None
}

/// `Range: bytes=<from>-[<to>]` (single range only). Malformed or
/// unsupported forms are treated as absent, per RFC 7233's "may ignore".
fn parse_range(value: &str) -> Option<(u64, Option<u64>)> {
    let spec = value.trim().strip_prefix("bytes=")?.trim();
    if spec.contains(',') {
        return None;
    }
    let (from, to) = spec.split_once('-')?;
    let from: u64 = from.trim().parse().ok()?;
    let to = to.trim();
    if to.is_empty() {
        return Some((from, None));
    }
    let to: u64 = to.parse().ok()?;
    if to < from {
        return None;
    }
    Some((from, Some(to)))
}

fn error_response(status: u16, reason: &'static str, message: String) -> HttpServerResponse {
    let body = ErrorJson { error: message }.serialize_json().into_bytes();
    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    HttpServerResponse::new(header, body)
}

fn etag_for(digest: &str) -> String {
    format!("\"sha256:{digest}\"")
}

/// Count queued and running serves until their read finishes or the queued job is dropped.
struct BlobServe(Arc<ServiceShared>);

impl BlobServe {
    fn new(shared: &Arc<ServiceShared>) -> Self {
        let peer = &shared.peer;
        let previous = peer.serves_in_flight.fetch_add(1, Ordering::Relaxed);
        if previous == peer.max_serves {
            eprintln!("peer-cache: blob pool at capacity ({}), 1 request waiting", peer.max_serves);
        }
        Self(Arc::clone(shared))
    }
}

impl Drop for BlobServe {
    fn drop(&mut self) {
        self.0.peer.serves_in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

/// Routes one `GET /v1/model_blob/<digest>`. Cheap checks (path, auth,
/// allow-list, range math) run inline on the route thread; the bounded file
/// read + response run on the blob pool so a blob chunk never stalls
/// `/health`, `/generate` or job polling. The response sender is consumed
/// either way.
pub(crate) fn route_blob(
    shared: &Arc<ServiceShared>,
    pool: Option<&RouteWorker>,
    headers: &HttpServerHeaders,
    caller: &makepad_network::fleet::front::Caller,
    response_sender: mpsc::Sender<HttpServerResponse>,
) {
    let respond = |response: HttpServerResponse| {
        let _ = response_sender.send(response);
    };

    // -- path: exactly one 64-hex segment, nothing else addressable --
    let digest = &headers.path[BLOB_PATH_PREFIX.len()..];
    let digest = digest.to_ascii_lowercase();
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
    {
        return respond(error_response(
            400,
            "Bad Request",
            "blob path must be a single 64-hex sha256 digest".to_string(),
        ));
    }
    if headers.search.is_some() {
        return respond(error_response(
            400,
            "Bad Request",
            "query parameters are not accepted on the blob endpoint".to_string(),
        ));
    }

    // -- a paired phone (fleet credential checked by the front) fetches
    // the on-device model files: no node-to-node ticket, the same
    // allow-list, range and concurrency rules below --
    let peer = &shared.peer;
    let Some(pool) = pool else {
        return respond(error_response(
            403,
            "Forbidden",
            "peer-cache serving is disabled on this node".to_string(),
        ));
    };
    if caller.role != makepad_network::fleet::id::Role::Paired {
        // -- fail closed without a secret --
        let Some(secret) = peer.secret.as_ref().filter(|_| peer.serve) else {
            return respond(error_response(
                403,
                "Forbidden",
                "peer-cache serving is disabled on this node".to_string(),
            ));
        };

        // -- authentication before any existence disclosure --
        // The ticket rides its own header: Authorization carries the node's
        // fleet credential, checked by the TLS front before this runs.
        let Some(bearer) = header_value(headers, "x-peer-ticket").map(str::trim) else {
            return respond(error_response(
                401,
                "Unauthorized",
                "missing X-Peer-Ticket: <transfer ticket>".to_string(),
            ));
        };
        let Some(receiver) = header_value(headers, "x-peer-receiver") else {
            return respond(error_response(
                401,
                "Unauthorized",
                "missing X-Peer-Receiver header".to_string(),
            ));
        };
        let Some(ticket) = crate::peer::PeerTicket::parse(bearer) else {
            return respond(error_response(
                401,
                "Unauthorized",
                "malformed transfer ticket".to_string(),
            ));
        };
        if let Err(denied) = ticket.verify(
            secret,
            &shared.node_key,
            receiver,
            &digest,
            crate::peer::now_unix(),
        ) {
            return respond(error_response(403, "Forbidden", denied.to_string()));
        }
    }

    // -- allow-list: known digest with a valid receipt right now --
    let Some(blob) = find_verified_blob(&shared.registry, &shared.cache_dir, &digest) else {
        return respond(error_response(
            404,
            "Not Found",
            "no verified artifact with that digest on this node".to_string(),
        ));
    };

    // -- range math (content-addressed, so If-Range mismatch = from zero) --
    let mut range = header_value(headers, "range").and_then(parse_range);
    if let Some(if_range) = header_value(headers, "if-range") {
        if if_range != etag_for(&digest) {
            range = None;
        }
    }
    if let Some((from, _)) = range {
        if from >= blob.size {
            let body = Vec::new();
            let header = format!(
                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{}\r\nETag: {}\r\nCache-Control: no-cache\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                blob.size,
                etag_for(&digest)
            );
            return respond(HttpServerResponse::new(header, body));
        }
    }
    let start = range.map(|(from, _)| from).unwrap_or(0);
    let requested_end = range.and_then(|(_, to)| to).unwrap_or(blob.size - 1);
    let end = requested_end
        .min(blob.size - 1)
        .min(start + peer.chunk_max - 1);
    let len = end - start + 1;
    // Whole small file without a Range -> plain 200; everything else 206.
    let whole = range.is_none() && len == blob.size;

    let serve = BlobServe::new(shared);
    pool.dispatch(Box::new(move || read_chunk_response(serve, &blob, start, len, whole)), response_sender);
}

/// Reads one bounded chunk under a serve lease, re-verifying the receipt
/// first, and builds the 200/206 response.
fn read_chunk_response(
    serve: BlobServe,
    blob: &crate::peer::BlobEntry,
    start: u64,
    len: u64,
    whole: bool,
) -> HttpServerResponse {
    let shared = &serve.0;
    let peer = &shared.peer;
    #[cfg(test)]
    if let Some(gate) = &peer.read_gate {
        let (release, wait) = mpsc::channel();
        gate.send((std::thread::current().id(), release)).expect("the test must see the worker enter the blob read");
        wait.recv().expect("the test must release the blob read");
    }
    let _lease = peer.leases.lease(&blob.path);
    // Fail closed against replacement between routing and this read.
    if find_verified_blob(&shared.registry, &shared.cache_dir, &blob.digest).is_none() {
        return error_response(
            404,
            "Not Found",
            "artifact stopped verifying on this node".to_string(),
        );
    }
    let mut file = match std::fs::File::open(&blob.path) {
        Ok(file) => file,
        Err(e) => {
            return error_response(500, "Internal Server Error", format!("blob open: {e}"));
        }
    };
    match file.metadata() {
        Ok(meta) if meta.len() == blob.size => {}
        Ok(meta) => {
            return error_response(
                404,
                "Not Found",
                format!(
                    "artifact size changed on disk ({} != {}) — refusing to serve",
                    meta.len(),
                    blob.size
                ),
            );
        }
        Err(e) => {
            return error_response(500, "Internal Server Error", format!("blob stat: {e}"));
        }
    }
    if let Err(e) = file.seek(SeekFrom::Start(start)) {
        return error_response(500, "Internal Server Error", format!("blob seek: {e}"));
    }
    let mut body = vec![0u8; len as usize];
    let mut read = 0usize;
    while read < body.len() {
        match file.read(&mut body[read..]) {
            Ok(0) => {
                return error_response(
                    500,
                    "Internal Server Error",
                    "blob ended early on disk".to_string(),
                );
            }
            Ok(n) => read += n,
            Err(e) => {
                return error_response(500, "Internal Server Error", format!("blob read: {e}"));
            }
        }
    }
    if let Some(throttle) = &peer.throttle {
        throttle.pace(len);
    }
    let etag = etag_for(&blob.digest);
    let header = if whole {
        format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nETag: {etag}\r\nAccept-Ranges: bytes\r\nCache-Control: no-cache\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n"
        )
    } else {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\nContent-Range: bytes {start}-{}/{}\r\nETag: {etag}\r\nAccept-Ranges: bytes\r\nCache-Control: no-cache\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n",
            start + len - 1,
            blob.size
        )
    };
    HttpServerResponse::new(header, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_parsing() {
        assert_eq!(parse_range("bytes=0-"), Some((0, None)));
        assert_eq!(parse_range("bytes=100-"), Some((100, None)));
        assert_eq!(parse_range("bytes=5-9"), Some((5, Some(9))));
        assert_eq!(parse_range(" bytes=7-7 "), Some((7, Some(7))));
        for malformed in [
            "bytes=-500",
            "bytes=9-5",
            "bytes=a-",
            "items=0-",
            "bytes=0-1,5-9",
            "",
        ] {
            assert_eq!(parse_range(malformed), None, "{malformed:?}");
        }
    }

    #[test]
    fn throttle_reserves_time() {
        let throttle = Throttle::mb_per_sec(1.0);
        let started = Instant::now();
        // Three reservations of ~100 KiB at 1 MiB/s: the first is free, the
        // rest owe ~200ms of pacing between them.
        throttle.pace(100 * 1024);
        throttle.pace(100 * 1024);
        throttle.pace(100 * 1024);
        let elapsed = started.elapsed();
        assert!(elapsed >= Duration::from_millis(150), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(2), "{elapsed:?}");
    }

    #[test]
    fn blob_serves_queue_at_capacity_and_use_every_free_worker() {
        use crate::backend::CancelToken;
        use crate::registry::Registry;
        use crate::server::lifecycle_tests::{fixture_shared_serving, request_headers};
        use makepad_network::fleet::{front::Caller, id::Role};

        let bytes = b"abcd";
        let digest = crate::sha256::sha256_hex(bytes);
        let cache = std::env::temp_dir().join(format!("hub-blob-pool-{}", std::process::id()));
        std::fs::create_dir(&cache).unwrap();
        let mut shared = fixture_shared_serving(&[], None, true);
        let source = Arc::get_mut(&mut shared).unwrap();
        source.cache_dir = cache.clone();
        source.registry = Registry::parse(&format!(r#"{{"models":[{{
            "id":"blob","domain":"text","backend":"testpattern","available":true,"gated":false,
            "files":[{{"repo":"","path":"","local":true,"cache_as":"chunk","size":4,"sha256":"{digest}"}}]
        }}]}}"#)).unwrap();
        let file = &source.registry.models[0].files[0];
        std::fs::write(file.dest_path(&cache), bytes).unwrap();
        source.downloader.ensure_file(file, &cache, &mut |_| {}, &CancelToken::new())
            .expect("the local file must have a verified receipt before serving");
        let (entered, at_gate) = mpsc::channel();
        source.peer.read_gate = Some(entered);
        source.peer.secret = None;
        source.peer.throttle = None;
        let phone = Caller { role: Role::Paired, ..Caller::local() };

        for threads in [1, 2] {
            Arc::get_mut(&mut shared).unwrap().peer.max_serves = threads;
            let pool = shared.peer.start_pool().expect("paired phones must have a blob pool");
            let request = |range: Option<&str>| {
                let mut headers = request_headers("GET", &format!("{BLOB_PATH_PREFIX}{digest}"), None);
                if let Some(range) = range {
                    headers.lines.push(format!("Range: bytes={range}\r\n"));
                }
                let (send, receive) = mpsc::channel();
                route_blob(&shared, Some(&pool), &headers, &phone, send);
                assert!(matches!(receive.try_recv(), Err(mpsc::TryRecvError::Empty)),
                    "a valid GET must wait for its chunk instead of returning an immediate refusal");
                receive
            };
            let whole = request(None);
            let (first_worker, release_first) = at_gate.recv().expect("the first blob must enter its read");
            let range = request(Some("1-2"));
            assert_eq!(shared.peer.serves_in_flight.load(Ordering::Relaxed), 2,
                "both accepted serves must count while the first read is held");
            if threads == 1 {
                assert!(matches!(at_gate.try_recv(), Err(mpsc::TryRecvError::Empty)),
                    "a single worker must leave the second read queued");
                release_first.send(()).unwrap();
                let (next_worker, release_second) = at_gate.recv().expect("the queued read must start when the first finishes");
                assert_eq!(next_worker, first_worker, "the queued read must reuse the single worker");
                release_second.send(()).unwrap();
            } else {
                let (other_worker, release_second) = at_gate.recv().expect("a free second worker must start before the first finishes");
                assert_ne!(other_worker, first_worker, "two workers must hold independent chunk reads");
                release_second.send(()).unwrap();
                release_first.send(()).unwrap();
            }

            let tail = |len| format!("ETag: \"sha256:{digest}\"\r\nAccept-Ranges: bytes\r\nCache-Control: no-cache\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n");
            let whole = whole.recv().expect("the whole-file GET must receive its reply");
            assert_eq!(whole.header, format!("HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\n{}", tail(4)),
                "a whole-file serve must retain its exact 200 headers");
            assert_eq!(whole.body, bytes, "the whole-file GET must receive every byte");
            let range = range.recv().expect("the ranged GET must receive its reply");
            assert_eq!(range.header, format!("HTTP/1.1 206 Partial Content\r\nContent-Type: application/octet-stream\r\nContent-Range: bytes 1-2/4\r\n{}", tail(2)),
                "a ranged serve must retain its exact 206 headers");
            assert_eq!(range.body, b"bc", "the ranged GET must receive exactly the requested bytes");
            pool.close();
            assert_eq!(shared.peer.serves_in_flight.load(Ordering::Relaxed), 0,
                "a drained pool must release both serves from the in-flight count");
        }
        std::fs::remove_dir_all(cache).unwrap();
    }
}
