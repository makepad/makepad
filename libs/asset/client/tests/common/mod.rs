//! Shared test infrastructure: unique on-disk roots, REAL canonical content
//! (manifests built with `makepad-asset-data`, hashed with real SHA-256),
//! and REAL TCP fixture servers speaking the wire protocol over sockets.
//! There are no mocks: the client under test dials actual endpoints and every
//! digest it verifies is a genuine hash of genuine bytes.
#![allow(dead_code)]

use makepad_asset_client::json::{self, obj, s, Value};
use makepad_asset_client::wire;
use makepad_asset_data::*;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A fresh unique root per call: pid + counter + name, under the OS temp dir.
pub fn test_root(name: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "mp_asset_client_test_{}_{}_{}",
        std::process::id(),
        n,
        name
    ))
}

pub fn hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    for b in bytes {
        use std::fmt::Write;
        let _ = write!(out, "{b:02x}");
    }
    out
}

// ---------------------------------------------------------------------------
// real canonical content
// ---------------------------------------------------------------------------

/// Deterministic pseudo-random payload bytes.
pub fn payload(seed: u64, len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut x = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    while out.len() < len {
        x = x.wrapping_mul(0x2545_f491_4f6c_dd1d).wrapping_add(0x1234_5678);
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
}

/// Minimal valid mesh-bearing manifest: one render GLB plus the mandatory
/// thumbnail, both referencing the digests of the supplied bytes.
pub fn prop_manifest(
    asset_id: AssetId,
    glb: &[u8],
    thumb: &[u8],
    dependencies: Vec<AssetRevisionRef>,
) -> AssetManifest {
    AssetManifest {
        asset_id,
        kind: AssetKind::Prop,
        files: vec![AssetFile {
            role: FileRole::RenderGlb,
            tier: DeviceTier::Any,
            lod: 0,
            media: MediaType::Glb,
            blob: BlobId::hash_of(glb),
            byte_len: glb.len() as u64,
            dims: None,
        }],
        dependencies,
        thumbnail: Some(ThumbnailMeta {
            blob: BlobId::hash_of(thumb),
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            byte_len: thumb.len() as u64,
        }),
        metrics: Metrics {
            total_bytes: glb.len() as u64 + thumb.len() as u64,
            triangles: 12,
            vertices: 8,
            joints: 0,
            clips: 0,
            max_texture_dim: 0,
            media_millis: 0,
        },
        coordinate_system: CoordinateSystem {
            units_per_meter: 1.0,
            up: Axis::YPos,
            forward: Axis::ZNeg,
            pivot: Pivot::Origin,
        },
        bounds: Bounds { min: Vec3::new(-1.0, -1.0, -1.0), max: Vec3::new(1.0, 1.0, 1.0) },
        anchors: vec![],
        capabilities: Capabilities {
            rigged: false,
            animated: false,
            collidable: false,
            loopable: false,
            spawnable: false,
        },
        spawn_recipe: None,
        provenance: None,
        rights: Rights {
            license: "CC0-1.0".into(),
            license_revision: String::new(),
            terms_digest: None,
            terms_url: String::new(),
            credits: "fixture".into(),
            source: String::new(),
            source_archive: None,
            redistribution: Redistribution::Allowed,
            derivatives: DerivativePolicy::Allowed,
        },
    }
}

/// One published asset inside the fixture store.
pub struct FixtureAsset {
    pub asset_id: AssetId,
    pub namespace: String,
    pub alias: Option<String>,
    pub title: String,
    pub revision: AssetRevisionId,
    pub manifest: AssetManifest,
    pub created_ms: u64,
}

/// In-memory content store served by [`FixtureServer`]. All identities are
/// real digests of real canonical bytes.
#[derive(Default)]
pub struct FixtureStore {
    pub blobs: HashMap<[u8; 32], Vec<u8>>,
    /// arev digest → canonical manifest bytes.
    pub manifests: HashMap<[u8; 32], Vec<u8>>,
    /// grev digest → canonical game manifest bytes.
    pub game_manifests: HashMap<[u8; 32], Vec<u8>>,
    pub assets: Vec<FixtureAsset>,
    /// alias string → (asset, head revision).
    pub aliases: HashMap<String, (AssetId, AssetRevisionId)>,
}

impl FixtureStore {
    pub fn add_blob(&mut self, bytes: Vec<u8>) -> BlobId {
        let id = BlobId::hash_of(&bytes);
        self.blobs.insert(*id.as_bytes(), bytes);
        id
    }

    /// Publish a prop built from real bytes; returns the exact ref.
    pub fn add_prop(
        &mut self,
        id_byte: u8,
        namespace: &str,
        alias: Option<&str>,
        title: &str,
        glb: Vec<u8>,
        deps: Vec<AssetRevisionRef>,
    ) -> AssetRevisionRef {
        let thumb = payload(9000 + id_byte as u64, 900);
        self.add_blob(glb.clone());
        self.add_blob(thumb.clone());
        let asset_id = AssetId::from_bytes([id_byte; 16]);
        let manifest = prop_manifest(asset_id, &glb, &thumb, deps);
        manifest.validate().expect("fixture manifest valid");
        let bytes = manifest.to_canonical_bytes().expect("fixture canonical bytes");
        let revision = manifest.revision().expect("fixture revision");
        self.manifests.insert(*revision.as_bytes(), bytes);
        if let Some(a) = alias {
            self.aliases.insert(a.to_string(), (asset_id, revision));
        }
        self.assets.push(FixtureAsset {
            asset_id,
            namespace: namespace.to_string(),
            alias: alias.map(str::to_string),
            title: title.to_string(),
            revision,
            manifest,
            created_ms: 1_700_000_000_000 + id_byte as u64,
        });
        // Keyset pages compare display strings, so keep the canonical order
        // in the same key (base32 byte order differs from ASCII order).
        self.assets.sort_by_key(|a| a.asset_id.to_string());
        AssetRevisionRef { asset_id, revision }
    }

    /// Publish a game revision whose splash/manifest/lock blobs are real.
    pub fn add_game(&mut self, id_byte: u8, name: &str) -> GameRevisionId {
        let splash = payload(50_000 + id_byte as u64, 2_000);
        let card = payload(60_000 + id_byte as u64, 300);
        let lock = payload(70_000 + id_byte as u64, 400);
        let thumb = payload(80_000 + id_byte as u64, 800);
        let manifest = GameRevisionManifest {
            game_id: GameId::from_bytes([id_byte; 16]),
            name: name.into(),
            description: "fixture game".into(),
            author: "fixture".into(),
            splash_blob: self.add_blob(splash.clone()),
            manifest_blob: self.add_blob(card),
            lock_blob: self.add_blob(lock),
            thumbnail: ThumbnailMeta {
                blob: self.add_blob(thumb.clone()),
                media: ThumbnailMedia::Png,
                width: 512,
                height: 512,
                byte_len: thumb.len() as u64,
            },
            catalog_snapshot: None,
            search_algorithm_version: 1,
            engine_version: 1,
            protocol_version: 1,
            splash_byte_len: splash.len() as u64,
        };
        manifest.validate().expect("fixture game valid");
        let bytes = manifest.to_canonical_bytes().expect("fixture game bytes");
        let revision = manifest.revision().expect("fixture game revision");
        self.game_manifests.insert(*revision.as_bytes(), bytes);
        revision
    }
}

// ---------------------------------------------------------------------------
// raw TCP server (hostile tests hand-craft the bytes)
// ---------------------------------------------------------------------------

/// A parsed incoming request, enough for routing and assertions.
#[derive(Clone, Debug)]
pub struct ParsedRequest {
    pub method: String,
    pub target: String,
    /// Lowercased names.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl ParsedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
    }

    /// Path segments of the target (no query).
    pub fn segs(&self) -> Vec<String> {
        let path = self.target.split('?').next().unwrap_or("");
        path.trim_start_matches('/').split('/').map(str::to_string).collect()
    }

    pub fn query_get(&self, key: &str) -> Option<String> {
        let q = self.target.split_once('?')?.1;
        for pair in q.split('&') {
            let (k, v) = pair.split_once('=')?;
            if k == key {
                return Some(v.to_string());
            }
        }
        None
    }
}

pub type RawHandler = dyn Fn(ParsedRequest, &mut TcpStream) + Send + Sync;

/// Real listening TCP server; each connection serves one request through the
/// handler and closes (matching the client's `Connection: close` model).
pub struct RawServer {
    pub addr: SocketAddr,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl RawServer {
    pub fn start(handler: Arc<RawHandler>) -> RawServer {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
        let addr = listener.local_addr().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_t = stop.clone();
        let join = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if stop_t.load(Ordering::Relaxed) {
                    break;
                }
                let Ok(mut stream) = conn else { continue };
                let handler = handler.clone();
                std::thread::spawn(move || {
                    if let Some(req) = read_request(&mut stream) {
                        handler(req, &mut stream);
                    }
                });
            }
        });
        RawServer { addr, stop, join: Some(join) }
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Unblock accept.
        let _ = TcpStream::connect(self.addr);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for RawServer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Minimal exact request reader for the fixture side.
pub fn read_request(stream: &mut TcpStream) -> Option<ParsedRequest> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        if buf.len() > 128 * 1024 {
            return None;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head = String::from_utf8(buf[..head_end].to_vec()).ok()?;
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split(' ');
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = Vec::new();
    for line in lines {
        let (k, v) = line.split_once(':')?;
        headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
    }
    let mut body = buf.split_off(head_end + 4);
    let content_length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);
    while body.len() < content_length {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);
    Some(ParsedRequest { method, target, headers, body })
}

// ---- response writers ------------------------------------------------------

pub fn write_raw(stream: &mut TcpStream, bytes: &[u8]) {
    let _ = stream.write_all(bytes);
}

pub fn response_head(status: u16, content_type: &str, len: u64, extra: &[(&str, &str)]) -> String {
    let mut out = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {len}\r\n"
    );
    for (k, v) in extra {
        out.push_str(&format!("{k}: {v}\r\n"));
    }
    out.push_str("Connection: close\r\n\r\n");
    out
}

pub fn write_json_resp(stream: &mut TcpStream, status: u16, v: &Value) {
    let body = v.to_json().into_bytes();
    let head = response_head(status, "application/json", body.len() as u64, &[]);
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
}

pub fn write_error(stream: &mut TcpStream, status: u16, msg: &str) {
    write_json_resp(stream, status, &obj(vec![("error", s(msg))]));
}

pub fn write_bytes_resp(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra: &[(&str, &str)],
) {
    let head = response_head(status, content_type, body.len() as u64, extra);
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
}

// ---------------------------------------------------------------------------
// full protocol fixture server
// ---------------------------------------------------------------------------

pub struct FixtureOptions {
    pub server_id: [u8; 16],
    /// Value reported by /v1/health (tests can lie here).
    pub health_protocol_version: u64,
    /// Require `Authorization: Bearer <token>` on every route.
    pub auth_token: Option<String>,
}

impl Default for FixtureOptions {
    fn default() -> Self {
        Self {
            server_id: [0x5a; 16],
            health_protocol_version: wire::PROTOCOL_VERSION as u64,
            auth_token: None,
        }
    }
}

#[derive(Default)]
pub struct FixtureKnobs {
    /// Close the connection after this many blob body bytes — consumed once.
    pub kill_blob_after: Mutex<Option<u64>>,
    /// Serve blob bytes with the first byte flipped (digest mismatch).
    pub corrupt_blobs: Mutex<bool>,
    /// Answer range requests with a full 200 (ignore Range).
    pub ignore_range: Mutex<bool>,
    /// Drip blob bodies as `(chunk_bytes, delay_ms)` — deterministic slow
    /// transfers for cancellation tests.
    pub drip_blob: Mutex<Option<(usize, u64)>>,
}

/// Mutable write-route state (uploads/registrations made by the client
/// under test). Kept apart from the immutable seeded [`FixtureStore`].
#[derive(Default)]
pub struct PublishedStore {
    pub blobs: HashMap<[u8; 32], Vec<u8>>,
    /// arev digest → canonical manifest bytes.
    pub manifests: HashMap<[u8; 32], Vec<u8>>,
    /// asset display id → (namespace, revisions[(rev display, published)]).
    pub assets: HashMap<String, (String, Vec<(String, bool)>)>,
    /// alias → (asset display id, revision display id).
    pub aliases: HashMap<String, (String, String)>,
    /// asset display id → annotation title (proves the annotation landed).
    pub annotations: HashMap<String, String>,
    pub minted: u64,
}

#[derive(Default)]
pub struct FixtureLog {
    pub requests: Mutex<Vec<ParsedRequest>>,
}

impl FixtureLog {
    pub fn count(&self, method: &str, target_prefix: &str) -> usize {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.method == method && r.target.starts_with(target_prefix))
            .count()
    }

    pub fn last_matching(&self, method: &str, target_prefix: &str) -> Option<ParsedRequest> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|r| r.method == method && r.target.starts_with(target_prefix))
            .cloned()
    }
}

/// The full read-API fixture: control plane and data plane on separate real
/// TCP ports, sharing one store. JSON routes live ONLY on control, blobs
/// ONLY on data — requests to the wrong plane 404, which proves the client
/// routes correctly.
pub struct FixtureServer {
    pub control: RawServer,
    pub data: RawServer,
    pub store: Arc<FixtureStore>,
    pub published: Arc<Mutex<PublishedStore>>,
    pub knobs: Arc<FixtureKnobs>,
    pub log: Arc<FixtureLog>,
    pub options: Arc<FixtureOptions>,
}

impl FixtureServer {
    pub fn start(store: FixtureStore, options: FixtureOptions) -> FixtureServer {
        let store = Arc::new(store);
        let published = Arc::new(Mutex::new(PublishedStore::default()));
        let knobs = Arc::new(FixtureKnobs::default());
        let log = Arc::new(FixtureLog::default());
        let options = Arc::new(options);

        let control = {
            let (store, published, log, options) =
                (store.clone(), published.clone(), log.clone(), options.clone());
            RawServer::start(Arc::new(move |req, stream| {
                log.requests.lock().unwrap().push(req.clone());
                if !auth_ok(&options, &req) {
                    write_error(stream, 401, "unauthenticated");
                    return;
                }
                control_route(&store, &published, &options, &req, stream);
            }))
        };
        let data = {
            let (store, published, knobs, log, options) = (
                store.clone(),
                published.clone(),
                knobs.clone(),
                log.clone(),
                options.clone(),
            );
            RawServer::start(Arc::new(move |req, stream| {
                log.requests.lock().unwrap().push(req.clone());
                if !auth_ok(&options, &req) {
                    write_error(stream, 401, "unauthenticated");
                    return;
                }
                data_route(&store, &published, &knobs, &req, stream);
            }))
        };
        FixtureServer { control, data, store, published, knobs, log, options }
    }

    pub fn endpoints(&self) -> makepad_asset_client::ApiEndpoints {
        makepad_asset_client::ApiEndpoints { control: self.control.addr, data: self.data.addr }
    }

    /// Broadcast one discovery beacon for this fixture to a loopback port.
    pub fn send_beacon(&self, port: u16, auth_required: bool) {
        let beacon = makepad_asset_client::Beacon {
            protocol_version: wire::PROTOCOL_VERSION,
            server_id: self.options.server_id,
            control_port: self.control.addr.port(),
            data_port: self.data.addr.port(),
            auth_required,
            tls: false,
            capability_bits: wire::caps::ALL_V1,
        };
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket.send_to(&beacon.encode(), ("127.0.0.1", port)).unwrap();
    }
}

fn auth_ok(options: &FixtureOptions, req: &ParsedRequest) -> bool {
    match &options.auth_token {
        None => true,
        Some(token) => req.header("authorization") == Some(&format!("Bearer {token}")),
    }
}

// ---- control plane routes --------------------------------------------------

fn control_route(
    store: &FixtureStore,
    published: &Mutex<PublishedStore>,
    options: &FixtureOptions,
    req: &ParsedRequest,
    stream: &mut TcpStream,
) {
    let segs = req.segs();
    let seg = |i: usize| segs.get(i).map(String::as_str).unwrap_or("");
    // ---- write routes (publish flow) ----
    match (req.method.as_str(), seg(0), seg(1)) {
        ("POST", "v1", "assets") if segs.len() == 2 => {
            let Ok(body) = json::parse(&req.body) else {
                write_error(stream, 400, "malformed json");
                return;
            };
            let ns = body.get("namespace").and_then(Value::as_str).unwrap_or("").to_string();
            let mut p = published.lock().unwrap();
            let asset_id = match body.get("asset_id").and_then(Value::as_str) {
                Some(id) => {
                    if p.assets.contains_key(id) {
                        write_error(stream, 409, "conflict");
                        return;
                    }
                    id.to_string()
                }
                None => {
                    p.minted += 1;
                    let mut bytes = [0xB0u8; 16];
                    bytes[8..].copy_from_slice(&p.minted.to_be_bytes());
                    AssetId::from_bytes(bytes).to_string()
                }
            };
            p.assets.insert(asset_id.clone(), (ns.clone(), Vec::new()));
            write_json_resp(
                stream,
                201,
                &obj(vec![("asset_id", s(asset_id)), ("namespace", s(ns))]),
            );
            return;
        }
        ("POST", "v1", "assets") if segs.len() == 4 && seg(3) == "revisions" => {
            let asset = seg(2).to_string();
            let mut p = published.lock().unwrap();
            if !p.assets.contains_key(&asset) {
                write_error(stream, 404, "not found");
                return;
            }
            // The revision IS the digest of the canonical bytes.
            let rev = AssetRevisionId::from_bytes(*BlobId::hash_of(&req.body).as_bytes());
            p.manifests.insert(*rev.as_bytes(), req.body.clone());
            p.assets.get_mut(&asset).unwrap().1.push((rev.to_string(), false));
            write_json_resp(
                stream,
                201,
                &obj(vec![
                    ("asset_id", s(asset)),
                    ("revision", s(rev.to_string())),
                    ("state", s("staged")),
                ]),
            );
            return;
        }
        ("POST", "v1", "assets") if segs.len() == 6 && seg(3) == "revisions" && seg(5) == "publish" => {
            let (asset, rev) = (seg(2).to_string(), seg(4).to_string());
            let mut p = published.lock().unwrap();
            let Some((_, revs)) = p.assets.get_mut(&asset) else {
                write_error(stream, 404, "not found");
                return;
            };
            let Some(entry) = revs.iter_mut().find(|(r, _)| *r == rev) else {
                write_error(stream, 404, "not found");
                return;
            };
            entry.1 = true;
            write_json_resp(
                stream,
                200,
                &obj(vec![
                    ("asset_id", s(asset)),
                    ("revision", s(rev)),
                    ("state", s("published")),
                ]),
            );
            return;
        }
        ("PUT", "v1", "aliases") if segs.len() >= 4 => {
            let alias = segs[2..].join("/");
            let Ok(body) = json::parse(&req.body) else {
                write_error(stream, 400, "malformed json");
                return;
            };
            let asset = body.get("asset_id").and_then(Value::as_str).unwrap_or("").to_string();
            let rev = body.get("revision").and_then(Value::as_str).unwrap_or("").to_string();
            published.lock().unwrap().aliases.insert(alias.clone(), (asset.clone(), rev.clone()));
            write_json_resp(
                stream,
                200,
                &obj(vec![
                    ("alias", s(alias)),
                    ("asset_id", s(asset)),
                    ("head_revision", s(rev)),
                ]),
            );
            return;
        }
        ("PUT", "v1", "assets") if segs.len() == 4 && seg(3) == "annotation" => {
            let asset = seg(2).to_string();
            let title = json::parse(&req.body)
                .ok()
                .and_then(|b| b.get("title").and_then(Value::as_str).map(str::to_string))
                .unwrap_or_default();
            published.lock().unwrap().annotations.insert(asset, title);
            // The real server answers a bodyless 204 here.
            write_raw(stream, b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n");
            return;
        }
        _ => {}
    }
    match (req.method.as_str(), seg(0), seg(1)) {
        ("GET", "v1", "health") => {
            write_json_resp(
                stream,
                200,
                &obj(vec![
                    ("server_id", s(hex(&options.server_id))),
                    ("protocol_version", Value::Int(options.health_protocol_version as i64)),
                ]),
            );
        }
        ("POST", "v1", "catalog") => catalog_route(store, req, stream),
        ("GET", "v1", "assets") if segs.len() == 2 => assets_route(store, req, stream),
        ("GET", "v1", "assets") if segs.len() == 3 => {
            // Published-store assets first (write-route round-trips), then
            // the seeded store.
            let asset = seg(2).to_string();
            let candidates = published.lock().unwrap().assets.get(&asset).map(|(ns, revs)| {
                let cands: Vec<Value> = revs
                    .iter()
                    .enumerate()
                    .map(|(i, (rev, is_pub))| {
                        obj(vec![
                            ("revision", s(rev.clone())),
                            ("state", s(if *is_pub { "published" } else { "staged" })),
                            ("staged_ms", Value::Int(1_000 + i as i64)),
                            (
                                "published_ms",
                                if *is_pub { Value::Int(2_000 + i as i64) } else { Value::Null },
                            ),
                            ("quarantined_ms", Value::Null),
                        ])
                    })
                    .collect();
                (ns.clone(), cands)
            });
            match candidates {
                Some((ns, cands)) => write_json_resp(
                    stream,
                    200,
                    &obj(vec![
                        ("asset_id", s(asset)),
                        ("namespace", s(ns)),
                        ("candidates", Value::Arr(cands)),
                    ]),
                ),
                None => asset_detail_route(store, seg(2), stream),
            }
        }
        ("GET", "v1", "aliases") if segs.len() >= 4 => {
            let alias = segs[2..].join("/");
            if let Some((asset_id, rev)) = store.aliases.get(&alias) {
                write_json_resp(
                    stream,
                    200,
                    &obj(vec![
                        ("alias", s(alias)),
                        ("asset_id", s(asset_id.to_string())),
                        ("head_revision", s(rev.to_string())),
                    ]),
                );
                return;
            }
            match published.lock().unwrap().aliases.get(&alias) {
                Some((asset_id, rev)) => write_json_resp(
                    stream,
                    200,
                    &obj(vec![
                        ("alias", s(alias)),
                        ("asset_id", s(asset_id.clone())),
                        ("head_revision", s(rev.clone())),
                    ]),
                ),
                None => write_error(stream, 404, "not found"),
            }
        }
        ("GET", "v1", "revisions") if segs.len() == 3 => {
            let seeded = AssetRevisionId::from_str(seg(2))
                .ok()
                .and_then(|rev| store.manifests.get(rev.as_bytes()).cloned())
                .or_else(|| {
                    AssetRevisionId::from_str(seg(2)).ok().and_then(|rev| {
                        published.lock().unwrap().manifests.get(rev.as_bytes()).cloned()
                    })
                });
            match seeded {
                Some(bytes) => {
                    write_bytes_resp(stream, 200, "application/octet-stream", &bytes, &[])
                }
                None => write_error(stream, 404, "not found"),
            }
        }
        ("GET", "v1", "game-revisions") if segs.len() == 3 => {
            match GameRevisionId::from_str(seg(2))
                .ok()
                .and_then(|rev| store.game_manifests.get(rev.as_bytes()))
            {
                Some(bytes) => {
                    write_bytes_resp(stream, 200, "application/octet-stream", bytes, &[])
                }
                None => write_error(stream, 404, "not found"),
            }
        }
        _ => write_error(stream, 404, "not found"),
    }
}

fn catalog_route(store: &FixtureStore, req: &ParsedRequest, stream: &mut TcpStream) {
    let Ok(body) = json::parse(&req.body) else {
        write_error(stream, 400, "malformed json");
        return;
    };
    let q = body.get("q").and_then(Value::as_str).unwrap_or("").to_lowercase();
    let ns = body.get("ns").and_then(Value::as_str);
    let limit = body.get("limit").and_then(Value::as_u64).unwrap_or(25) as usize;
    // Cursor binds to the query text so a cursor from another query refuses.
    let fingerprint = format!("f{}", q.len() + ns.map_or(0, str::len) * 100);
    let start = match body.get("cursor").and_then(Value::as_str) {
        None => 0usize,
        Some(c) => {
            let Some((idx, fp)) = c.split_once('.') else {
                write_error(stream, 400, "stale search cursor");
                return;
            };
            if fp != fingerprint {
                write_error(stream, 400, "stale search cursor");
                return;
            }
            idx.trim_start_matches('i').parse().unwrap_or(0)
        }
    };
    let matches: Vec<&FixtureAsset> = store
        .assets
        .iter()
        .filter(|a| q.is_empty() || a.title.to_lowercase().contains(&q))
        .filter(|a| ns.is_none_or(|n| a.namespace == n))
        .collect();
    let page: Vec<Value> = matches
        .iter()
        .skip(start)
        .take(limit)
        .map(|a| {
            obj(vec![
                ("asset_id", s(a.asset_id.to_string())),
                ("namespace", s(a.namespace.clone())),
                ("kind", s("prop")),
                ("title", s(a.title.clone())),
                ("snippet", s(format!("about {}", a.title))),
                ("score", Value::Int(100)),
                ("live", Value::Bool(a.alias.is_some())),
            ])
        })
        .collect();
    let next = start + page.len();
    let cursor = if next < matches.len() {
        s(format!("i{next}.{fingerprint}"))
    } else {
        Value::Null
    };
    write_json_resp(
        stream,
        200,
        &obj(vec![
            ("hits", Value::Arr(page)),
            ("total", Value::Int(matches.len() as i64)),
            ("cursor", cursor),
        ]),
    );
}

fn assets_route(store: &FixtureStore, req: &ParsedRequest, stream: &mut TcpStream) {
    let ns = req.query_get("ns");
    let limit: usize = req.query_get("limit").and_then(|l| l.parse().ok()).unwrap_or(50);
    let after = req.query_get("cursor");
    let rows: Vec<&FixtureAsset> = store
        .assets
        .iter()
        .filter(|a| ns.as_deref().is_none_or(|n| a.namespace == n))
        .filter(|a| after.as_deref().is_none_or(|c| a.asset_id.to_string().as_str() > c))
        .collect();
    let page: Vec<Value> = rows
        .iter()
        .take(limit)
        .map(|a| {
            obj(vec![
                ("asset_id", s(a.asset_id.to_string())),
                ("namespace", s(a.namespace.clone())),
                ("created_ms", Value::Int(a.created_ms as i64)),
            ])
        })
        .collect();
    let cursor = if rows.len() > limit {
        s(rows[limit - 1].asset_id.to_string())
    } else {
        Value::Null
    };
    write_json_resp(stream, 200, &obj(vec![("assets", Value::Arr(page)), ("cursor", cursor)]));
}

fn asset_detail_route(store: &FixtureStore, id: &str, stream: &mut TcpStream) {
    let Ok(asset_id) = AssetId::from_str(id) else {
        write_error(stream, 400, "invalid input");
        return;
    };
    let Some(a) = store.assets.iter().find(|a| a.asset_id == asset_id) else {
        write_error(stream, 404, "not found");
        return;
    };
    write_json_resp(
        stream,
        200,
        &obj(vec![
            ("asset_id", s(a.asset_id.to_string())),
            ("namespace", s(a.namespace.clone())),
            (
                "candidates",
                Value::Arr(vec![obj(vec![
                    ("revision", s(a.revision.to_string())),
                    ("state", s("published")),
                    ("staged_ms", Value::Int(a.created_ms as i64)),
                    ("published_ms", Value::Int(a.created_ms as i64 + 5)),
                    ("quarantined_ms", Value::Null),
                ])]),
            ),
        ]),
    );
}

// ---- data plane routes -----------------------------------------------------

fn data_route(
    store: &FixtureStore,
    published: &Mutex<PublishedStore>,
    knobs: &FixtureKnobs,
    req: &ParsedRequest,
    stream: &mut TcpStream,
) {
    let segs = req.segs();
    // Content-addressed upload (publish flow).
    if req.method == "POST" && segs.len() == 2 && segs[0] == "v1" && segs[1] == "blobs" {
        let blob = BlobId::hash_of(&req.body);
        let mut p = published.lock().unwrap();
        let deduped = p.blobs.insert(*blob.as_bytes(), req.body.clone()).is_some();
        write_json_resp(
            stream,
            201,
            &obj(vec![
                ("blob_id", s(blob.to_string())),
                ("size", Value::Int(req.body.len() as i64)),
                ("deduped", Value::Bool(deduped)),
            ]),
        );
        return;
    }
    if !(segs.len() == 3 && segs[0] == "v1" && segs[1] == "blobs") {
        write_error(stream, 404, "not found");
        return;
    }
    let Ok(blob) = BlobId::from_str(&segs[2]) else {
        write_error(stream, 400, "invalid input");
        return;
    };
    let seeded = store
        .blobs
        .get(blob.as_bytes())
        .cloned()
        .or_else(|| published.lock().unwrap().blobs.get(blob.as_bytes()).cloned());
    let Some(bytes) = seeded else {
        write_error(stream, 404, "not found");
        return;
    };
    let mut bytes = bytes.clone();
    if *knobs.corrupt_blobs.lock().unwrap() {
        if let Some(b) = bytes.first_mut() {
            *b ^= 0xff;
        }
    }
    let etag = format!("\"{blob}\"");
    let size = bytes.len() as u64;

    if req.method == "HEAD" {
        let head = response_head(
            200,
            "application/octet-stream",
            size,
            &[("ETag", etag.as_str()), ("Accept-Ranges", "bytes")],
        );
        write_raw(stream, head.as_bytes());
        return;
    }

    // Range handling: exactly the `bytes=<start>-` shape the client sends.
    let range_start = if *knobs.ignore_range.lock().unwrap() {
        None
    } else {
        req.header("range")
            .and_then(|r| r.strip_prefix("bytes="))
            .and_then(|r| r.strip_suffix('-'))
            .and_then(|r| r.parse::<u64>().ok())
            // If-Range mismatch → serve the full body.
            .filter(|_| req.header("if-range").is_none_or(|ir| ir == etag))
    };

    let (status, body_slice, extra) = match range_start {
        Some(start) if start >= size => {
            let cr = format!("bytes */{size}");
            let head = response_head(416, "application/json", 0, &[("Content-Range", &cr)]);
            write_raw(stream, head.as_bytes());
            return;
        }
        Some(start) => {
            let cr = format!("bytes {start}-{}/{size}", size - 1);
            (206, &bytes[start as usize..], vec![("Content-Range".to_string(), cr)])
        }
        None => (200, &bytes[..], vec![]),
    };

    let mut extra_refs: Vec<(&str, &str)> = vec![("ETag", etag.as_str())];
    for (k, v) in &extra {
        extra_refs.push((k.as_str(), v.as_str()));
    }
    let head = response_head(
        status,
        "application/octet-stream",
        body_slice.len() as u64,
        &extra_refs,
    );
    write_raw(stream, head.as_bytes());

    let kill_after = knobs.kill_blob_after.lock().unwrap().take();
    match kill_after {
        Some(n) => {
            let n = (n as usize).min(body_slice.len());
            write_raw(stream, &body_slice[..n]);
            // Dropping the stream closes the connection mid-body.
        }
        None => match *knobs.drip_blob.lock().unwrap() {
            Some((chunk, delay_ms)) => {
                for piece in body_slice.chunks(chunk.max(1)) {
                    write_raw(stream, piece);
                    let _ = stream.flush();
                    std::thread::sleep(std::time::Duration::from_millis(delay_ms));
                }
            }
            None => write_raw(stream, body_slice),
        },
    }
}
