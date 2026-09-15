//! The authenticated model-blob endpoint, served on the ONE existing service
//! port (hard invariant: one service process per box, no sidecar, no second
//! listener).
//!
//! `GET /v1/model_blob/<sha256>` with headers:
//! - `Authorization: Bearer <ticket>` — a [`crate::peer::PeerTicket`] scoped
//!   to (this source node, the claimed receiver node, this exact digest).
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
//! Bounds: concurrent serves are capped (excess answers `503 Retry-After`),
//! the digest path segment must be exactly 64 hex chars (no other addressing
//! exists — path traversal has no surface), query strings are refused,
//! per-response memory is `chunk_max`, and an optional fleet-wide MB/s
//! throttle paces workers. Every chunk read holds a [`ServeLeases`] lease and
//! re-verifies the artifact's receipt first, so eviction/replacement can
//! neither clobber an in-flight source nor let an invalid file leak out.

use crate::peer::{find_verified_blob, ServeLeases, TransferSecret};
use crate::protocol::ErrorJson;
use crate::server::ServiceShared;
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
    /// Max concurrent blob serves (else `MAKEPAD_AI_PEER_MAX_SERVES`).
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
    pub max_serves: usize,
    pub active_serves: Arc<AtomicUsize>,
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
        let chunk_max = options
            .chunk_max_bytes
            .or_else(|| env_parse::<u64>("MAKEPAD_AI_PEER_CHUNK_MB").map(|mb| mb * 1024 * 1024))
            .unwrap_or(DEFAULT_CHUNK_MAX)
            .clamp(64 * 1024, 256 * 1024 * 1024);
        let max_serves = options
            .max_serves
            .or_else(|| env_parse::<usize>("MAKEPAD_AI_PEER_MAX_SERVES"))
            .unwrap_or(DEFAULT_MAX_SERVES)
            .clamp(1, 64);
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
            active_serves: Arc::new(AtomicUsize::new(0)),
            throttle: mbps.map(Throttle::mb_per_sec),
            env_sources,
        }
    }

    /// Serving is on only when configured on AND a secret exists (fail
    /// closed: no secret means no authenticated caller can exist).
    pub fn serving_enabled(&self) -> bool {
        self.serve && self.secret.is_some()
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

fn busy_response() -> HttpServerResponse {
    let body = ErrorJson {
        error: "peer serve capacity reached on this node".to_string(),
    }
    .serialize_json()
    .into_bytes();
    let header = format!(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Type: application/json\r\nRetry-After: 1\r\nCache-Control: no-cache\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    HttpServerResponse::new(header, body)
}

fn etag_for(digest: &str) -> String {
    format!("\"sha256:{digest}\"")
}

struct ServeSlot {
    active: Arc<AtomicUsize>,
}

impl ServeSlot {
    fn try_acquire(active: &Arc<AtomicUsize>, cap: usize) -> Option<Self> {
        let mut current = active.load(Ordering::Relaxed);
        loop {
            if current >= cap {
                return None;
            }
            match active.compare_exchange_weak(
                current,
                current + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => {
                    return Some(Self {
                        active: active.clone(),
                    })
                }
                Err(seen) => current = seen,
            }
        }
    }
}

impl Drop for ServeSlot {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Routes one `GET /v1/model_blob/<digest>`. Cheap checks (path, auth,
/// allow-list, range math) run inline on the route thread; the bounded file
/// read + response happen on a spawned worker so a blob chunk never stalls
/// `/health`, `/generate` or job polling. The response sender is consumed
/// either way.
pub fn route_blob(
    shared: &Arc<ServiceShared>,
    headers: &HttpServerHeaders,
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

    // -- fail closed without a secret / with serving off --
    let peer = &shared.peer;
    let Some(secret) = peer.secret.as_ref().filter(|_| peer.serve) else {
        return respond(error_response(
            403,
            "Forbidden",
            "peer-cache serving is disabled on this node".to_string(),
        ));
    };

    // -- authentication before any existence disclosure --
    let Some(bearer) = header_value(headers, "authorization")
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(str::trim)
    else {
        return respond(error_response(
            401,
            "Unauthorized",
            "missing Authorization: Bearer <transfer ticket>".to_string(),
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

    // -- bounded concurrency; excess is an immediate, honest 503 --
    let Some(slot) = ServeSlot::try_acquire(&peer.active_serves, peer.max_serves) else {
        return respond(busy_response());
    };

    let shared = shared.clone();
    std::thread::spawn(move || {
        let _slot = slot;
        let response = read_chunk_response(&shared, &blob, start, len, whole);
        let _ = response_sender.send(response);
    });
}

/// Reads one bounded chunk under a serve lease, re-verifying the receipt
/// first, and builds the 200/206 response.
fn read_chunk_response(
    shared: &Arc<ServiceShared>,
    blob: &crate::peer::BlobEntry,
    start: u64,
    len: u64,
    whole: bool,
) -> HttpServerResponse {
    let peer = &shared.peer;
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
    fn serve_slots_bound_and_release() {
        let active = Arc::new(AtomicUsize::new(0));
        let a = ServeSlot::try_acquire(&active, 2).expect("slot 1");
        let _b = ServeSlot::try_acquire(&active, 2).expect("slot 2");
        assert!(ServeSlot::try_acquire(&active, 2).is_none(), "at cap");
        drop(a);
        assert!(ServeSlot::try_acquire(&active, 2).is_some(), "released");
    }
}
