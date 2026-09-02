// Native static-store integration test uses blocking socket deadlines.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use makepad_asset_client::json::{obj, s, Value};
use makepad_asset_client::{
    BaseUrl, BlobContent, ClientConfig, ClientError, ClientEvent, ClientOutput, ClientRequest,
    ClientRuntime, MemoryCacheStore, OwnedRequest, OwnedResponse, StaticStore, StaticStoreEvent,
    ClientLocation, SessionConfig, SessionConnector, SessionMsg, Transport, TransportCompletion,
    TransportError, TransportId, MAX_STATIC_MANIFEST_BYTES,
};
use makepad_asset_data::*;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

struct ExportFixture {
    routes: BTreeMap<String, Vec<u8>>,
    asset: AssetId,
    revision: AssetRevisionId,
    game_revision: GameRevisionId,
    alias: AssetAlias,
    blob: BlobId,
    blob_bytes: Vec<u8>,
    thumbnail: BlobId,
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::new();
    for byte in bytes { use std::fmt::Write; let _ = write!(out, "{byte:02x}"); }
    out
}

fn fixture() -> ExportFixture {
    let asset = AssetId::from_bytes([7; 16]);
    let alias = AssetAlias::from_str("stock/fixture/box").unwrap();
    let blob_bytes = b"fixture glb payload".to_vec();
    let thumbnail_bytes = b"fixture png payload".to_vec();
    let blob = BlobId::hash_of(&blob_bytes);
    let thumbnail = BlobId::hash_of(&thumbnail_bytes);
    let manifest = AssetManifest {
        asset_id: asset,
        kind: AssetKind::Prop,
        files: vec![AssetFile {
            role: FileRole::RenderGlb, tier: DeviceTier::Any, lod: 0, media: MediaType::Glb,
            blob, byte_len: blob_bytes.len() as u64, dims: None,
        }],
        dependencies: Vec::new(),
        thumbnail: Some(ThumbnailMeta {
            blob: thumbnail, media: ThumbnailMedia::Png, width: 512, height: 512,
            byte_len: thumbnail_bytes.len() as u64, views: Vec::new(),
        }),
        metrics: Metrics {
            total_bytes: (blob_bytes.len() + thumbnail_bytes.len()) as u64,
            triangles: 12, vertices: 8, joints: 0, clips: 0, max_texture_dim: 0,
            media_millis: 0,
        },
        coordinate_system: CoordinateSystem {
            units_per_meter: 1.0, up: Axis::YPos, forward: Axis::ZNeg, pivot: Pivot::Origin,
        },
        bounds: Bounds { min: Vec3::new(-1.0, -1.0, -1.0), max: Vec3::new(1.0, 1.0, 1.0) },
        anchors: Vec::new(),
        capabilities: Capabilities {
            rigged: false, animated: false, collidable: false, loopable: false, spawnable: false,
        },
        spawn_recipe: None,
        provenance: None,
        rights: Rights {
            license: "CC0-1.0".into(), license_revision: String::new(), terms_digest: None,
            terms_url: String::new(), credits: "fixture".into(), source: String::new(),
            source_archive: None, redistribution: Redistribution::Allowed,
            derivatives: DerivativePolicy::Allowed,
        },
    };
    manifest.validate().unwrap();
    let manifest_bytes = manifest.to_canonical_bytes().unwrap();
    let revision = AssetRevisionId::hash_of(&manifest_bytes);
    let game_manifest = GameRevisionManifest {
        game_id: GameId::from_bytes([8; 16]),
        name: "Fixture Game".into(),
        description: "Static game manifest fixture".into(),
        author: "Fixture Author".into(),
        splash_blob: BlobId::hash_of(b"game splash"),
        manifest_blob: BlobId::hash_of(b"game manifest"),
        lock_blob: BlobId::hash_of(b"game lock"),
        thumbnail: ThumbnailMeta {
            blob: thumbnail,
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            byte_len: thumbnail_bytes.len() as u64,
            views: Vec::new(),
        },
        catalog_snapshot: None,
        search_algorithm_version: 1,
        engine_version: 1,
        protocol_version: 1,
        splash_byte_len: 11,
    };
    let game_bytes = game_manifest.to_canonical_bytes().unwrap();
    let game_revision = GameRevisionId::hash_of(&game_bytes);
    let health = obj(vec![
        ("server_id", s("11111111111111111111111111111111")),
        ("protocol_version", Value::Int(makepad_asset_client::wire::PROTOCOL_VERSION as i64)),
    ]).to_json().into_bytes();
    let detail = obj(vec![
        ("asset_id", s(asset.to_string())), ("namespace", s("stock")),
        ("retired", Value::Bool(false)), ("retired_ms", Value::Null),
        ("candidates", Value::Arr(vec![])),
    ]).to_json().into_bytes();
    let alias_bytes = obj(vec![
        ("alias", s(alias.to_string())), ("asset_id", s(asset.to_string())),
        ("head_revision", s(revision.to_string())),
    ]).to_json().into_bytes();
    let mut routes = BTreeMap::from([
        ("/v1/health".into(), health),
        (format!("/v1/assets/{asset}"), detail),
        (format!("/v1/aliases/{alias}"), alias_bytes),
        (format!("/v1/revisions/{revision}"), manifest_bytes),
        (format!("/v1/game-revisions/{game_revision}"), game_bytes),
        (format!("/v1/blobs/{blob}"), blob_bytes.clone()),
        (format!("/v1/blobs/{thumbnail}"), thumbnail_bytes.clone()),
        (format!("/v1/thumbnails/revision/{revision}"), thumbnail_bytes),
        (format!("/v1/thumbnails/alias/{alias}"), routes_placeholder()),
    ]);
    // Alias and revision thumbnail routes are byte-identical in a real export.
    *routes.get_mut(&format!("/v1/thumbnails/alias/{alias}")).unwrap() =
        routes[&format!("/v1/blobs/{thumbnail}")].clone();
    let files = Value::Arr(routes.iter().map(|(path, bytes)| obj(vec![
        ("path", s(path.clone())), ("byte_len", Value::Int(bytes.len() as i64)),
        ("sha256", s(hex(&sha256(bytes)))), ("content_type", s("application/octet-stream")),
        ("content_encoding", Value::Null),
    ])).collect());
    let document = obj(vec![
        ("asset_id", s(asset.to_string())), ("kind", s("prop")),
        ("files", Value::Arr(vec![obj(vec![
            ("role", s("render_glb")), ("tier", s("any")), ("lod", Value::Int(0)),
            ("media", s("glb")), ("blob", s(blob.to_string())),
            ("byte_len", Value::Int(blob_bytes.len() as i64)),
        ])])),
        ("dependencies", Value::Arr(vec![])),
        ("thumbnail", obj(vec![
            ("blob", s(thumbnail.to_string())), ("media", s("png")),
            ("width", Value::Int(512)), ("height", Value::Int(512)),
            ("byte_len", Value::Int(routes[&format!("/v1/blobs/{thumbnail}")].len() as i64)),
        ])),
    ]);
    let static_manifest = obj(vec![
        ("static_version", Value::Int(1)),
        ("protocol_version", Value::Int(makepad_asset_client::wire::PROTOCOL_VERSION as i64)),
        ("snapshot_id", s("22222222222222222222222222222222")),
        ("server_id", s("11111111111111111111111111111111")),
        ("generated_ms", Value::Int(1_700_000_000_000)),
        ("assets", Value::Arr(vec![obj(vec![
            ("asset_id", s(asset.to_string())), ("namespace", s("stock")),
            ("created_ms", Value::Int(1_700_000_000_000)),
            ("revisions", Value::Arr(vec![s(revision.to_string())])),
        ])])),
        ("aliases", Value::Arr(vec![obj(vec![
            ("alias", s(alias.to_string())), ("asset_id", s(asset.to_string())),
            ("head_revision", s(revision.to_string())),
            ("updated_ms", Value::Int(1_700_000_000_001)),
        ])])),
        ("revisions", Value::Arr(vec![obj(vec![
            ("revision", s(revision.to_string())), ("document", document),
        ])])),
        ("search", obj(vec![
            ("normalization", s("ascii-alnum-lower-v1")),
            ("ranking", s("public-weight-sum-v1")),
            ("documents", Value::Arr(vec![obj(vec![
                ("asset_id", s(asset.to_string())), ("namespace", s("stock")),
                ("kind", s("prop")), ("title", s("Fixture Box")),
                ("description", s("A public fixture box")),
                ("categories", Value::Arr(vec![s("fixture")])),
                ("tags", Value::Arr(vec![s("public")])),
                ("creator", s("Fixture Author")), ("generator", s("fixture")),
                ("backend", s("fixture")), ("model", s("fixture")),
                ("live", Value::Bool(true)), ("updated_ms", Value::Int(1_700_000_000_001)),
                ("aliases", Value::Arr(vec![s(alias.to_string())])),
                ("terms", Value::Arr(vec![
                    obj(vec![("term", s("box")), ("weight", Value::Int(5))]),
                    obj(vec![("term", s("fixture")), ("weight", Value::Int(10))]),
                ])),
            ])])),
        ])),
        ("variants", Value::Arr(vec![])),
        ("blobs", {
            let mut blobs = vec![
                blob_value(blob, blob_bytes.len() as u64),
                blob_value(thumbnail, routes[&format!("/v1/blobs/{thumbnail}")].len() as u64),
            ];
            blobs.sort_by_key(Value::to_json);
            Value::Arr(blobs)
        }),
        ("files", files),
        ("policy", obj(vec![
            ("namespace", Value::Null),
            ("kind", Value::Null),
            ("limit", Value::Null),
            ("max_bytes_per_asset", Value::Null),
            ("max_total_bytes", Value::Null),
            ("include_video_up_to", Value::Int(32 * 1024 * 1024)),
        ])),
        ("totals", obj(vec![
            ("assets", Value::Int(1)),
            ("aliases", Value::Int(1)),
            ("revisions", Value::Int(1)),
            ("blobs_present", Value::Int(2)),
            ("blobs_omitted", Value::Int(0)),
            ("unique_blob_bytes", Value::Int((blob_bytes.len()
                + routes[&format!("/v1/blobs/{thumbnail}")].len()) as i64)),
        ])),
        ("exclusions", obj(vec![
            ("rights", Value::Int(0)),
            ("budget", Value::Int(0)),
            ("kind_mismatch", Value::Int(0)),
        ])),
    ]).to_json().into_bytes();
    routes.insert("/v1/static/manifest.json".into(), static_manifest);
    ExportFixture {
        routes,
        asset,
        revision,
        game_revision,
        alias,
        blob,
        blob_bytes,
        thumbnail,
    }
}

fn routes_placeholder() -> Vec<u8> { Vec::new() }

fn blob_value(blob: BlobId, byte_len: u64) -> Value {
    obj(vec![
        ("blob", s(blob.to_string())), ("path", s(format!("/v1/blobs/{blob}"))),
        ("byte_len", Value::Int(byte_len as i64)),
        ("sha256", s(hex(blob.as_bytes()))), ("present", Value::Bool(true)),
        ("reason", Value::Null), ("media", Value::Arr(vec![])), ("roles", Value::Arr(vec![])),
    ])
}

struct StaticServer {
    addr: SocketAddr,
    stopping: Arc<AtomicBool>,
    requests: Arc<Mutex<Vec<String>>>,
    join: Option<std::thread::JoinHandle<()>>,
}

fn socket_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

impl StaticServer {
    fn start(routes: BTreeMap<String, Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let stopping = Arc::new(AtomicBool::new(false));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = stopping.clone();
        let log = requests.clone();
        let join = std::thread::spawn(move || while !stop.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((mut stream, _)) => serve(&mut stream, &routes, &log),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock =>
                    std::thread::sleep(Duration::from_millis(1)),
                Err(_) => break,
            }
        });
        Self { addr, stopping, requests, join: Some(join) }
    }
}

impl Drop for StaticServer {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = TcpStream::connect(self.addr);
        if let Some(join) = self.join.take() { let _ = join.join(); }
    }
}

fn serve(stream: &mut TcpStream, routes: &BTreeMap<String, Vec<u8>>, log: &Mutex<Vec<String>>) {
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let mut request = Vec::new();
    let mut buf = [0u8; 4096];
    while !request.windows(4).any(|window| window == b"\r\n\r\n") {
        let Ok(read) = stream.read(&mut buf) else { return };
        if read == 0 { return; }
        request.extend_from_slice(&buf[..read]);
    }
    let head = String::from_utf8_lossy(&request);
    assert!(!head.to_ascii_lowercase().contains("authorization:"));
    let path = head.lines().next().and_then(|line| line.split_whitespace().nth(1)).unwrap_or("/");
    log.lock().unwrap().push(path.to_string());
    let (status, body) = routes.get(path).map(|body| ("200 OK", body.as_slice()))
        .unwrap_or(("404 Not Found", b"missing"));
    let response = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
    let _ = stream.write_all(response.as_bytes());
    let _ = stream.write_all(body);
}

fn ready(runtime: &mut ClientRuntime) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !runtime.is_ready() {
        let _ = runtime.poll();
        if let Some(error) = runtime.connect_error() { panic!("static connect failed: {error}"); }
        assert!(Instant::now() < deadline, "static connect timeout");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn complete(runtime: &mut ClientRuntime, request: ClientRequest) -> Result<ClientOutput, ClientError> {
    let id = runtime.submit(request).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        for event in runtime.poll() {
            match event {
                ClientEvent::Done { id: found, output } if found == id => return Ok(output),
                ClientEvent::Failed { id: found, error } if found == id => return Err(error),
                _ => {}
            }
        }
        assert!(Instant::now() < deadline, "static request timeout");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn platform_runtime_reads_static_export_and_deduplicates_digests() {
    let _serial = socket_test_lock();
    let fixture = fixture();
    let server = StaticServer::start(fixture.routes.clone());
    let base = BaseUrl::parse(format!("http://{}", server.addr)).unwrap();
    let mut runtime = ClientRuntime::start_static(ClientConfig::static_site(base.clone())).unwrap();
    ready(&mut runtime);
    for bootstrap in ["/v1/health", "/v1/static/manifest.json"] {
        assert_eq!(
            server.requests.lock().unwrap().iter().filter(|path| path.as_str() == bootstrap).count(),
            1,
            "bootstrap route {bootstrap} must be fetched exactly once",
        );
    }

    let ClientOutput::AssetsPage(page) = complete(&mut runtime, ClientRequest::AssetsPage {
        namespace: None, cursor: None, limit: 10,
    }).unwrap() else { panic!("listing output") };
    assert_eq!(page.assets[0].asset_id, fixture.asset);
    let ClientOutput::Alias(alias) = complete(&mut runtime, ClientRequest::ResolveAlias {
        alias: fixture.alias.clone(),
    }).unwrap() else { panic!("alias output") };
    assert_eq!(alias.head_revision, fixture.revision);
    let ClientOutput::AliasStatus(status) = complete(&mut runtime, ClientRequest::AliasStatus {
        entries: vec![(fixture.alias.clone(), None)],
        tags: vec!["public".into(), "absent".into()],
    }).unwrap() else { panic!("alias-status output") };
    assert_eq!(status[0].tags, ["public"]);
    let mut query = makepad_asset_client::CatalogQuery::text("fixture box", 10);
    query.facets = 10;
    let ClientOutput::CatalogPage(page) = complete(&mut runtime, ClientRequest::CatalogSearch {
        query, cursor: None,
    }).unwrap() else { panic!("search output") };
    assert_eq!(page.hits[0].asset_id, fixture.asset);
    assert_eq!(
        page.facets.iter().map(|facet| (facet.label.as_str(), facet.count)).collect::<Vec<_>>(),
        [("fixture", 1), ("public", 1)],
    );
    let ClientOutput::AssetDetail(detail) = complete(&mut runtime, ClientRequest::AssetDetail {
        id: fixture.asset,
    }).unwrap() else { panic!("detail output") };
    assert_eq!(detail.latest_published().unwrap().revision, fixture.revision);
    let requests_before_head = server.requests.lock().unwrap().len();
    let ClientOutput::BlobHead { head, .. } = complete(&mut runtime, ClientRequest::HeadBlob {
        blob: fixture.blob,
    }).unwrap() else { panic!("blob head output") };
    assert_eq!(head.size, fixture.blob_bytes.len() as u64);
    assert!(head.etag_matches);
    assert_eq!(server.requests.lock().unwrap().len(), requests_before_head);
    let ClientOutput::AssetManifest(manifest) = complete(&mut runtime, ClientRequest::FetchAssetManifest {
        rev: fixture.revision,
    }).unwrap() else { panic!("manifest output") };
    let ClientOutput::GameManifest(game) = complete(
        &mut runtime,
        ClientRequest::FetchGameManifest { rev: fixture.game_revision },
    ).unwrap() else { panic!("game manifest output") };
    assert_eq!(game.game_id, GameId::from_bytes([8; 16]));

    let a = runtime.submit(ClientRequest::FetchBlob {
        blob: fixture.blob, expected_len: Some(fixture.blob_bytes.len() as u64), pin: false,
    }).unwrap();
    let b = runtime.submit(ClientRequest::FetchBlob {
        blob: fixture.blob, expected_len: Some(fixture.blob_bytes.len() as u64), pin: false,
    }).unwrap();
    let mut done = HashMap::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while done.len() < 2 {
        for event in runtime.poll() {
            if let ClientEvent::Done { id, output: ClientOutput::Blob { content, .. } } = event {
                done.insert(id, content);
            }
        }
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(matches!(done[&a], BlobContent::Bytes(ref bytes) if bytes.as_ref() == fixture.blob_bytes));
    assert!(done.contains_key(&b));
    let path = format!("/v1/blobs/{}", fixture.blob);
    assert_eq!(server.requests.lock().unwrap().iter().filter(|item| **item == path).count(), 1);

    let ClientOutput::Thumbnail(Some(thumbnail)) = complete(&mut runtime,
        ClientRequest::ResolveThumbnail { manifest }).unwrap() else { panic!("thumbnail output") };
    assert_eq!(thumbnail.blob, fixture.thumbnail);
    assert!(thumbnail.content.as_bytes().is_some());
    assert!(matches!(complete(&mut runtime, ClientRequest::RetireAsset { id: fixture.asset }),
        Err(ClientError::Unavailable { capability: "retire", .. })));
    assert!(matches!(complete(&mut runtime, ClientRequest::PublishSideChannels {
        asset: fixture.asset,
        files: Arc::new(Vec::new()),
    }), Err(ClientError::Unavailable { capability: "side_channels", .. })));
    assert!(matches!(complete(&mut runtime, ClientRequest::GcStatus),
        Err(ClientError::Unavailable { capability: "blob_gc", .. })));

    let mut token = ClientConfig::static_site(base.clone());
    token.token = Some("must-not-leak".into());
    assert!(matches!(ClientRuntime::start_static(token),
        Err(ClientError::InvalidInput { what: "static site bearer token" })));
    runtime.shutdown();

    let mut connector = SessionConnector::start(SessionConfig::static_site(base.clone())).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    let handles = loop {
        if let Some(handles) = connector.poll().into_iter().find_map(|message| match message {
            SessionMsg::Up(handles) => Some(handles),
            SessionMsg::Status(_) => None,
        }) {
            break handles;
        }
        assert!(Instant::now() < deadline, "static session connect timeout");
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(handles.location, ClientLocation::StaticSite(base));
    assert!(handles.endpoints.is_none());
    handles.shutdown();
    connector.stop();
}

struct MockTransport {
    next: u64,
    routes: BTreeMap<String, Vec<u8>>,
    ready: VecDeque<TransportCompletion>,
    truncated_manifest: bool,
}

impl Transport for MockTransport {
    fn start(&mut self, request: OwnedRequest) -> TransportId {
        assert!(request.headers.iter().all(|(name, _)| !name.eq_ignore_ascii_case("authorization")));
        let id = TransportId(self.next);
        self.next += 1;
        let path = request.url_or_target.split_once("/v1/").map(|(_, tail)| format!("/v1/{tail}")).unwrap();
        let result = if self.truncated_manifest && path == "/v1/static/manifest.json" {
            Err(TransportError::Protocol { what: "content-length mismatch" })
        } else if let Some(body) = self.routes.get(&path).cloned() {
            if body.len() as u64 > request.max_response_body_bytes {
                Err(TransportError::OverBudget { what: "response body", limit: request.max_response_body_bytes,
                    found: body.len() as u64 })
            } else {
                Ok(OwnedResponse { status: 200,
                    headers: vec![("content-length".into(), body.len().to_string())], body })
            }
        } else {
            Ok(OwnedResponse { status: 404, headers: vec![("content-length".into(), "0".into())], body: Vec::new() })
        };
        self.ready.push_back(TransportCompletion { id, result });
        id
    }
    fn cancel(&mut self, id: TransportId) {
        self.ready.retain(|completion| completion.id != id);
    }
    fn poll(&mut self, out: &mut Vec<TransportCompletion>) { out.extend(self.ready.drain(..)); }
}

fn hostile(mut fixture: ExportFixture, edit: impl FnOnce(&mut Value)) -> ClientError {
    let mut value = makepad_asset_client::json::parse(&fixture.routes["/v1/static/manifest.json"]).unwrap();
    edit(&mut value);
    fixture.routes.insert("/v1/static/manifest.json".into(), value.to_json().into_bytes());
    failed_store(fixture.routes, false)
}

fn failed_store(routes: BTreeMap<String, Vec<u8>>, truncated_manifest: bool) -> ClientError {
    let transport = MockTransport { next: 1, routes, ready: VecDeque::new(), truncated_manifest };
    let mut store = StaticStore::start(
        BaseUrl::parse("https://static.example").unwrap(), Box::new(transport),
        Box::new(MemoryCacheStore::new(1024 * 1024)),
    ).unwrap();
    for _ in 0..4 {
        for event in store.poll() {
            if let StaticStoreEvent::Failed(error) = event { return error; }
        }
    }
    panic!("hostile store became ready")
}

#[test]
fn hostile_indexes_fail_before_ready() {
    let truncated = fixture();
    assert!(matches!(failed_store(truncated.routes, true), ClientError::Protocol { .. }));

    let error = hostile(fixture(), |value| {
        let blobs = value.get("blobs").unwrap().as_arr().unwrap().to_vec();
        let Value::Obj(root) = value else { unreachable!() };
        let slot = root.iter_mut().find(|(key, _)| key == "blobs").unwrap();
        slot.1 = Value::Arr(blobs.into_iter().rev().collect());
    });
    assert!(matches!(error, ClientError::Protocol { .. }));

    let error = hostile(fixture(), |value| {
        let Value::Obj(root) = value else { unreachable!() };
        let Value::Arr(blobs) = &mut root.iter_mut().find(|(key, _)| key == "blobs").unwrap().1 else { unreachable!() };
        let Value::Obj(fields) = &mut blobs[0] else { unreachable!() };
        fields.iter_mut().find(|(key, _)| key == "byte_len").unwrap().1 = Value::Int(999);
    });
    assert!(matches!(error, ClientError::Protocol { .. } | ClientError::SizeMismatch { .. }));

    let mut oversized = fixture();
    oversized.routes.insert("/v1/static/manifest.json".into(), vec![b' '; MAX_STATIC_MANIFEST_BYTES as usize + 1]);
    assert!(matches!(failed_store(oversized.routes, false), ClientError::OverBudget { .. }));
}

#[test]
fn tampered_blob_is_rejected_and_never_cached() {
    let _serial = socket_test_lock();
    let mut fixture = fixture();
    fixture.routes.insert(
        format!("/v1/blobs/{}", fixture.blob),
        vec![b'X'; fixture.blob_bytes.len()],
    );
    let server = StaticServer::start(fixture.routes);
    let base = BaseUrl::parse(format!("http://{}", server.addr)).unwrap();
    let mut runtime = ClientRuntime::start_static(ClientConfig::static_site(base)).unwrap();
    ready(&mut runtime);
    assert!(matches!(complete(&mut runtime, ClientRequest::FetchBlob {
        blob: fixture.blob, expected_len: Some(fixture.blob_bytes.len() as u64), pin: false,
    }), Err(ClientError::SizeMismatch { .. } | ClientError::DigestMismatch { .. })));
}
