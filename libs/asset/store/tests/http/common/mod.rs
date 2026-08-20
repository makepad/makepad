//! Shared integration-test harness: unique on-disk roots, a real server
//! instance on ephemeral ports, a minimal keep-alive HTTP client speaking
//! the same bytes any foreign client would, and content-contract fixtures
//! whose blob references point at real uploaded bytes.
#![allow(dead_code)]

use makepad_asset_store::json::{self, Value};
use makepad_asset_store::{AssetServer, ServerConfig};
use makepad_asset_data::*;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

pub fn test_root(name: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "mp_asset_server_http_{}_{}_{}",
        std::process::id(),
        n,
        name
    ))
}

pub struct TestServer {
    pub server: AssetServer,
    pub root: PathBuf,
}

pub fn base_config(root: PathBuf) -> ServerConfig {
    let mut cfg = ServerConfig::new(root);
    cfg.control_addr = "127.0.0.1:0".parse().unwrap();
    cfg.data_addr = "127.0.0.1:0".parse().unwrap();
    cfg.bootstrap_admin = true;
    cfg.log = false;
    // Fast janitor so lease-expiry paths are observable in test time.
    cfg.janitor_interval_ms = 50;
    cfg
}

pub fn start_server(name: &str) -> TestServer {
    start_server_with(name, |_| {})
}

pub fn start_server_with(name: &str, tune: impl FnOnce(&mut ServerConfig)) -> TestServer {
    let root = test_root(name);
    let mut cfg = base_config(root.clone());
    tune(&mut cfg);
    let server = AssetServer::start(cfg).expect("server start");
    TestServer { server, root }
}

impl TestServer {
    pub fn admin_token(&self) -> String {
        std::fs::read_to_string(self.root.join("admin-token"))
            .expect("admin token file")
            .trim()
            .to_string()
    }

    pub fn control(&self, token: Option<&str>) -> Client {
        Client::new(self.server.control_addr(), token)
    }

    pub fn data(&self, token: Option<&str>) -> Client {
        Client::new(self.server.data_addr(), token)
    }
}

// ---------------------------------------------------------------------------
// minimal HTTP/1.1 client
// ---------------------------------------------------------------------------

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub closed: bool,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        json::parse(&self.body).expect("response json")
    }

    pub fn str_field(&self, key: &str) -> String {
        self.json()
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("missing string field {key} in {:?}", String::from_utf8_lossy(&self.body)))
            .to_string()
    }
}

pub struct Client {
    addr: SocketAddr,
    token: Option<String>,
    stream: Option<TcpStream>,
}

impl Client {
    pub fn new(addr: SocketAddr, token: Option<&str>) -> Client {
        Client { addr, token: token.map(str::to_string), stream: None }
    }

    pub fn set_token(&mut self, token: Option<&str>) {
        self.token = token.map(str::to_string);
    }

    fn connect(&mut self) -> &mut TcpStream {
        if self.stream.is_none() {
            let s = TcpStream::connect(self.addr).expect("connect");
            s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
            s.set_write_timeout(Some(Duration::from_secs(20))).unwrap();
            self.stream = Some(s);
        }
        self.stream.as_mut().unwrap()
    }

    /// One request over the kept-alive connection, transparently reconnecting
    /// once if the pooled connection died at the boundary.
    pub fn request(
        &mut self,
        method: &str,
        target: &str,
        extra: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Response {
        match self.try_request(method, target, extra, body) {
            Some(r) => r,
            None => {
                self.stream = None;
                self.try_request(method, target, extra, body)
                    .expect("request after reconnect")
            }
        }
    }

    fn try_request(
        &mut self,
        method: &str,
        target: &str,
        extra: &[(&str, &str)],
        body: Option<&[u8]>,
    ) -> Option<Response> {
        let mut head = format!("{method} {target} HTTP/1.1\r\nHost: test\r\n");
        if let Some(token) = &self.token {
            head.push_str(&format!("Authorization: Bearer {token}\r\n"));
        }
        for (k, v) in extra {
            head.push_str(&format!("{k}: {v}\r\n"));
        }
        if let Some(b) = body {
            head.push_str(&format!("Content-Length: {}\r\n", b.len()));
        }
        head.push_str("\r\n");
        let stream = self.connect();
        if stream.write_all(head.as_bytes()).is_err() {
            return None;
        }
        if let Some(b) = body {
            if stream.write_all(b).is_err() {
                return None;
            }
        }
        let head_only = method == "HEAD";
        let resp = read_response(stream, head_only)?;
        if resp.closed {
            self.stream = None;
        }
        Some(resp)
    }

    pub fn get(&mut self, target: &str) -> Response {
        self.request("GET", target, &[], None)
    }

    pub fn head(&mut self, target: &str) -> Response {
        self.request("HEAD", target, &[], None)
    }

    pub fn delete(&mut self, target: &str) -> Response {
        self.request("DELETE", target, &[], None)
    }

    pub fn post_json(&mut self, target: &str, v: &Value) -> Response {
        self.request(
            "POST",
            target,
            &[("Content-Type", "application/json")],
            Some(v.to_json().as_bytes()),
        )
    }

    pub fn put_json(&mut self, target: &str, v: &Value) -> Response {
        self.request(
            "PUT",
            target,
            &[("Content-Type", "application/json")],
            Some(v.to_json().as_bytes()),
        )
    }

    pub fn post_bytes(&mut self, target: &str, bytes: &[u8]) -> Response {
        self.request(
            "POST",
            target,
            &[("Content-Type", "application/octet-stream")],
            Some(bytes),
        )
    }
}

fn read_response(stream: &mut TcpStream, head_only: bool) -> Option<Response> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };
    let head_text = String::from_utf8(buf[..header_end].to_vec()).ok()?;
    let mut lines = head_text.split("\r\n");
    let status_line = lines.next()?;
    let status: u16 = status_line.split(' ').nth(1)?.parse().ok()?;
    let mut headers = Vec::new();
    for line in lines {
        let (k, v) = line.split_once(':')?;
        headers.push((k.trim().to_string(), v.trim().to_string()));
    }
    let content_length: usize = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .map(|(_, v)| v.parse().unwrap())
        .unwrap_or(0);
    let closed = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("connection"))
        .map(|(_, v)| v.eq_ignore_ascii_case("close"))
        .unwrap_or(false);
    let mut body = buf[header_end + 4..].to_vec();
    let want = if head_only || status == 204 || status == 304 { 0 } else { content_length };
    while body.len() < want {
        let n = stream.read(&mut chunk).ok()?;
        if n == 0 {
            return None;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(want);
    Some(Response { status, headers, body, closed })
}

/// Open a raw connection, write exactly `bytes`, and read until EOF or the
/// first full response. For hostile-input tests.
pub fn raw_roundtrip(addr: SocketAddr, bytes: &[u8]) -> Vec<u8> {
    let mut s = TcpStream::connect(addr).expect("connect");
    s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let _ = s.write_all(bytes);
    let mut out = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => out.extend_from_slice(&chunk[..n]),
        }
    }
    out
}

pub fn status_of(raw: &[u8]) -> u16 {
    let text = String::from_utf8_lossy(raw);
    text.split(' ').nth(1).and_then(|t| t.parse().ok()).unwrap_or(0)
}

// ---------------------------------------------------------------------------
// json construction helpers
// ---------------------------------------------------------------------------

pub fn jobj(pairs: Vec<(&str, Value)>) -> Value {
    json::obj(pairs)
}

pub fn jstr(v: impl Into<String>) -> Value {
    json::s(v)
}

// ---------------------------------------------------------------------------
// auth flows
// ---------------------------------------------------------------------------

/// Create a fresh principal with the given `(capability, scope)` grants and
/// return a bearer token for it.
pub fn principal_with(admin: &mut Client, grants: &[(&str, &str)]) -> String {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    let r = admin.post_json(
        "/v1/auth/principals",
        &jobj(vec![("name", jstr(format!("test-principal-{n}")))]),
    );
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let principal = r.str_field("principal");
    for (cap, scope) in grants {
        let r = admin.post_json(
            "/v1/auth/grants",
            &jobj(vec![
                ("principal", jstr(principal.clone())),
                ("capability", jstr(*cap)),
                ("scope", jstr(*scope)),
            ]),
        );
        assert_eq!(r.status, 204, "{}", String::from_utf8_lossy(&r.body));
    }
    let r = admin.post_json(
        "/v1/auth/tokens",
        &jobj(vec![("principal", jstr(principal))]),
    );
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    r.str_field("token")
}

// ---------------------------------------------------------------------------
// content fixtures (real canonical documents over real uploaded bytes)
// ---------------------------------------------------------------------------

pub fn prop_manifest(asset_id: AssetId, glb: &[u8], thumb: &[u8]) -> AssetManifest {
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
        dependencies: vec![],
        thumbnail: Some(ThumbnailMeta {
            blob: BlobId::hash_of(thumb),
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            byte_len: thumb.len() as u64,
            views: Vec::new(),
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
        bounds: Bounds {
            min: Vec3::new(-1.0, -1.0, -1.0),
            max: Vec3::new(1.0, 1.0, 1.0),
        },
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
        rights: test_rights(),
    }
}

/// Fixed pack source bytes for the Kenney-style import fixture — identical
/// to the core crate's fixture so identities agree across test suites and
/// across clean servers.
pub const PACK_GLB: &[u8] = b"KENNEY-WATCHTOWER-GLB-v1";
pub const PACK_COLLIDER: &[u8] = b"KENNEY-WATCHTOWER-COLLIDER-v1";
pub const PACK_PREVIEW: &[u8] = b"KENNEY-WATCHTOWER-PREVIEW-PNG-v1";
pub const PACK_TEXTURE: &[u8] = b"KENNEY-HULL-PANEL-PNG-v1";

pub fn kenney_terms() -> Rights {
    Rights {
        license: "CC0-1.0".into(),
        license_revision: String::new(),
        terms_digest: Some(sha256(b"CC0-1.0 legal text")),
        terms_url: "https://creativecommons.org/publicdomain/zero/1.0/".into(),
        credits: "Kenney (kenney.nl)".into(),
        source: "https://kenney.nl/assets/space-kit".into(),
        source_archive: Some(sha256(b"space-kit-1.0.zip")),
        redistribution: Redistribution::Allowed,
        derivatives: DerivativePolicy::Allowed,
    }
}

pub fn kenney_collection() -> SourceCollection {
    SourceCollection {
        id: "kenney".into(),
        title: "Kenney game assets".into(),
        origin: SourceOrigin::Upload,
        terms: kenney_terms(),
    }
}

/// The tiny pinned two-asset Kenney-style pack over the fixed source bytes.
pub fn kenney_pack(version: &str) -> ImportManifest {
    let mut manifest = ImportManifest {
        source_collection: kenney_collection().digest().unwrap(),
        source_id: "kenney".into(),
        pack_name: "space-kit".into(),
        pack_version: version.into(),
        policy_version: IMPORT_ASSET_ID_POLICY_V1,
        assets: vec![
            ImportAsset {
                key: "models/watchtower".parse().unwrap(),
                kind: AssetKind::Prop,
                files: vec![
                    ImportFile {
                        path: "models/watchtower.glb".into(),
                        file: AssetFile {
                            role: FileRole::RenderGlb,
                            tier: DeviceTier::Any,
                            lod: 0,
                            media: MediaType::Glb,
                            blob: BlobId::hash_of(PACK_GLB),
                            byte_len: PACK_GLB.len() as u64,
                            dims: None,
                        },
                    },
                    ImportFile {
                        path: "colliders/watchtower.bin".into(),
                        file: AssetFile {
                            role: FileRole::Collider,
                            tier: DeviceTier::Any,
                            lod: 0,
                            media: MediaType::Bin,
                            blob: BlobId::hash_of(PACK_COLLIDER),
                            byte_len: PACK_COLLIDER.len() as u64,
                            dims: None,
                        },
                    },
                ],
                thumbnail: Some(ImportThumbnail {
                    path: "previews/watchtower.png".into(),
                    meta: ThumbnailMeta {
                        blob: BlobId::hash_of(PACK_PREVIEW),
                        media: ThumbnailMedia::Png,
                        width: 512,
                        height: 512,
                        byte_len: PACK_PREVIEW.len() as u64,
                        views: Vec::new(),
                    },
                }),
                metrics: Metrics {
                    total_bytes: (PACK_GLB.len() + PACK_COLLIDER.len() + PACK_PREVIEW.len())
                        as u64,
                    triangles: 500,
                    vertices: 300,
                    joints: 0,
                    clips: 0,
                    max_texture_dim: 512,
                    media_millis: 0,
                },
                coordinate_system: CoordinateSystem {
                    units_per_meter: 1.0,
                    up: Axis::YPos,
                    forward: Axis::ZNeg,
                    pivot: Pivot::BoundsBottom,
                },
                bounds: Bounds {
                    min: Vec3::new(-1.0, 0.0, -1.0),
                    max: Vec3::new(1.0, 3.0, 1.0),
                },
                anchors: vec![],
                capabilities: Capabilities {
                    collidable: true,
                    ..Default::default()
                },
                spawn_recipe: None,
            },
            ImportAsset {
                key: "textures/hull-panel".parse().unwrap(),
                kind: AssetKind::Texture,
                files: vec![ImportFile {
                    path: "textures/hull_panel.png".into(),
                    file: AssetFile {
                        role: FileRole::Texture,
                        tier: DeviceTier::Any,
                        lod: 0,
                        media: MediaType::Png,
                        blob: BlobId::hash_of(PACK_TEXTURE),
                        byte_len: PACK_TEXTURE.len() as u64,
                        dims: Some(ImageDims {
                            width: 2048,
                            height: 2048,
                        }),
                    },
                }],
                thumbnail: None,
                metrics: Metrics {
                    total_bytes: PACK_TEXTURE.len() as u64,
                    max_texture_dim: 2048,
                    ..Default::default()
                },
                coordinate_system: CoordinateSystem {
                    units_per_meter: 1.0,
                    up: Axis::YPos,
                    forward: Axis::ZNeg,
                    pivot: Pivot::Origin,
                },
                bounds: Bounds {
                    min: Vec3::ZERO,
                    max: Vec3::ONE,
                },
                anchors: vec![],
                capabilities: Capabilities::default(),
                spawn_recipe: None,
            },
        ],
        rights: kenney_terms(),
    };
    manifest.canonicalize();
    manifest
}

/// Full v3 rights fixture with pinned terms and provenance digests.
pub fn test_rights() -> Rights {
    Rights {
        license: "CC0-1.0".into(),
        license_revision: String::new(),
        terms_digest: Some(sha256(b"CC0-1.0 legal text")),
        terms_url: "https://creativecommons.org/publicdomain/zero/1.0/".into(),
        credits: "test".into(),
        source: "".into(),
        source_archive: None,
        redistribution: Redistribution::Allowed,
        derivatives: DerivativePolicy::Allowed,
    }
}

pub fn game_manifest(
    game_id: GameId,
    splash: &[u8],
    toml: &[u8],
    lock_bytes: &[u8],
    thumb: &[u8],
) -> GameRevisionManifest {
    GameRevisionManifest {
        game_id,
        name: "Test Game".into(),
        description: "integration fixture".into(),
        author: "rik".into(),
        splash_blob: BlobId::hash_of(splash),
        manifest_blob: BlobId::hash_of(toml),
        lock_blob: BlobId::hash_of(lock_bytes),
        thumbnail: ThumbnailMeta {
            blob: BlobId::hash_of(thumb),
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            byte_len: thumb.len() as u64,
            views: Vec::new(),
        },
        catalog_snapshot: None,
        search_algorithm_version: 1,
        engine_version: 1,
        protocol_version: 1,
        splash_byte_len: splash.len() as u64,
    }
}

pub fn lock_for(game_id: GameId, refs: &[(&str, AssetRevisionRef)]) -> Vec<u8> {
    let mut lock = ContentLock {
        game_id,
        entries: refs
            .iter()
            .map(|(alias, r)| LockEntry {
                alias: alias.parse().unwrap(),
                asset_id: r.asset_id,
                revision: r.revision,
            })
            .collect(),
        closure: refs.iter().map(|(_, r)| *r).collect(),
        variant_sets: vec![],
    };
    lock.canonicalize();
    lock.to_canonical_bytes().unwrap()
}

/// Two length-prefixed frames, as `POST /v1/games/{id}/revisions` expects.
pub fn framed(manifest: &[u8], lock: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + manifest.len() + lock.len());
    out.extend_from_slice(&(manifest.len() as u64).to_be_bytes());
    out.extend_from_slice(manifest);
    out.extend_from_slice(&(lock.len() as u64).to_be_bytes());
    out.extend_from_slice(lock);
    out
}

/// Upload blobs, register, stage, publish and alias one prop entirely over
/// HTTP. Returns `(asset_id, revision)` display strings.
pub fn publish_prop_http(
    control: &mut Client,
    data: &mut Client,
    ns: &str,
    alias: &str,
    glb: &[u8],
    thumb: &[u8],
) -> (String, String) {
    for bytes in [glb, thumb] {
        let r = data.post_bytes(&format!("/v1/blobs?ns={ns}"), bytes);
        assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    }
    let r = control.post_json("/v1/assets", &jobj(vec![("namespace", jstr(ns))]));
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let asset_id = r.str_field("asset_id");
    let ast: AssetId = asset_id.parse().unwrap();
    let manifest = prop_manifest(ast, glb, thumb).to_canonical_bytes().unwrap();
    let r = control.post_bytes(&format!("/v1/assets/{asset_id}/revisions"), &manifest);
    assert_eq!(r.status, 201, "{}", String::from_utf8_lossy(&r.body));
    let revision = r.str_field("revision");
    let r = control.post_json(
        &format!("/v1/assets/{asset_id}/revisions/{revision}/publish"),
        &jobj(vec![]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    let r = control.put_json(
        &format!("/v1/aliases/{alias}"),
        &jobj(vec![
            ("asset_id", jstr(asset_id.clone())),
            ("revision", jstr(revision.clone())),
        ]),
    );
    assert_eq!(r.status, 200, "{}", String::from_utf8_lossy(&r.body));
    (asset_id, revision)
}
