use makepad_map_nav::{
    geo::LonLat,
    graph::GraphBuilder,
    search::{Category, SearchIndexBuilder, SearchResult},
};
use makepad_web_server::{
    api::{sample_along, AlongRequest, AlongResult, ApiFailure, NavBackend, RouteRequest, RouteResult, SearchRequest, ServiceRegistry},
    static_files::{ReportRateLimiter, StaticHandler},
};
use makepad_geodata::{
    mvt::AttrVal,
    query::LayerDb,
    sidecar::SidecarBuilder,
    wkb::Geometry,
};
use makepad_mbtile_reader::MbtilesWriter;
use makepad_network::HttpServerHeaders;
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::{IpAddr, Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

struct TempTree(PathBuf);

impl TempTree {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "makepad-web-{label}-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        make_tree_writable(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[cfg(unix)]
fn make_tree_writable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let Ok(metadata) = fs::symlink_metadata(path) else { return };
    if metadata.is_dir() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o755));
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                make_tree_writable(&entry.path());
            }
        }
    } else if !metadata.file_type().is_symlink() {
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o644));
    }
}

#[cfg(not(unix))]
fn make_tree_writable(_path: &Path) {}

#[cfg(target_os = "linux")]
fn freeze_docroot(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if fs::symlink_metadata(path).unwrap().is_dir() {
        for entry in fs::read_dir(path).unwrap() {
            freeze_docroot(&entry.unwrap().path());
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o555)).unwrap();
    } else {
        fs::set_permissions(path, fs::Permissions::from_mode(0o444)).unwrap();
    }
}

#[cfg(not(target_os = "linux"))]
fn freeze_docroot(_path: &Path) {}

struct ServerChild(Child);

impl Drop for ServerChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[derive(Debug)]
struct Response {
    status: u16,
    headers: String,
    body: Vec<u8>,
}

fn free_address() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.local_addr().unwrap()
}

fn wait_until_listening(address: SocketAddr) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if TcpStream::connect_timeout(&address, Duration::from_millis(50)).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("server did not listen on {address}");
}

fn request(address: SocketAddr, request: &[u8]) -> Response {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    stream.write_all(request).unwrap();
    stream.shutdown(Shutdown::Write).unwrap();
    let mut bytes = Vec::new();
    if let Err(error) = stream.read_to_end(&mut bytes) {
        assert_eq!(error.kind(), std::io::ErrorKind::ConnectionReset);
    }
    let split = bytes.windows(4).position(|window| window == b"\r\n\r\n").unwrap() + 4;
    let headers = String::from_utf8(bytes[..split].to_vec()).unwrap();
    let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
    Response { status, headers, body: bytes[split..].to_vec() }
}

fn get(address: SocketAddr, path: &str, headers: &str) -> Response {
    request(address, format!("GET {path} HTTP/1.1\r\nHost: test\r\n{headers}\r\n").as_bytes())
}

fn post(address: SocketAddr, path: &str, body: &[u8]) -> Response {
    let mut request_bytes = format!(
        "POST {path} HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n",
        body.len()
    ).into_bytes();
    request_bytes.extend_from_slice(body);
    request(address, &request_bytes)
}

fn slow_along_upload(address: SocketAddr) -> TcpStream {
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
    stream
        .write_all(
            b"POST /api/along HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: 2097152\r\n\r\n",
        )
        .unwrap();
    stream
}

fn post_with_type(address: SocketAddr, path: &str, content_type: &str, body: &[u8]) -> Response {
    let mut request_bytes = format!(
        "POST {path} HTTP/1.1\r\nHost: test\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n",
        body.len()
    )
    .into_bytes();
    request_bytes.extend_from_slice(body);
    request(address, &request_bytes)
}

fn direct_headers(verb: &str, target: &str) -> HttpServerHeaders {
    let (path, search) = target
        .split_once('?')
        .map(|(path, query)| (path, Some(query.to_string())))
        .unwrap_or((target, None));
    HttpServerHeaders {
        addr: "127.0.0.1:12345".parse().unwrap(),
        addr_text: "127.0.0.1:12345".into(),
        lines: if verb == "POST" {
            vec![
                format!("{verb} {target} HTTP/1.1\r\n"),
                "Content-Type: application/json\r\n".into(),
            ]
        } else {
            vec![format!("{verb} {target} HTTP/1.1\r\n")]
        },
        verb: verb.into(),
        path: path.into(),
        path_no_slash: path.trim_start_matches('/').into(),
        search,
        content_length: None,
        accept_encoding: None,
        sec_websocket_key: None,
    }
}

fn direct_get(registry: &ServiceRegistry, target: &str) -> makepad_network::HttpServerResponse {
    let (sender, receiver) = std::sync::mpsc::channel();
    assert!(registry.handle_get(&direct_headers("GET", target), &sender));
    receiver.recv_timeout(Duration::from_secs(2)).unwrap()
}

fn direct_status(response: &makepad_network::HttpServerResponse) -> u16 {
    response.header.split_whitespace().nth(1).unwrap().parse().unwrap()
}

fn fixture_graph() -> Vec<u8> {
    let mut builder = GraphBuilder::new();
    builder.add_node(1, 4.8952, 52.3702);
    builder.add_node(2, 4.9050, 52.3600);
    builder.add_node(3, 4.9150, 52.3500);
    let mut tags = HashMap::new();
    tags.insert("highway".into(), "residential".into());
    tags.insert("name".into(), "Fixture Street".into());
    builder.add_way(1, vec![1, 2, 3], tags);
    builder.build().serialize()
}

fn fixture_search() -> Vec<u8> {
    let mut builder = SearchIndexBuilder::new();
    builder.add("Fixture Museum", "Fixture City", LonLat::new(4.9050, 52.3600), Category::Museum, 220);
    builder.add("Fixture City", "", LonLat::new(4.9000, 52.3650), Category::City, 255);
    builder.build().serialize()
}

fn start_fixture_server(base: &Path) -> (ServerChild, SocketAddr) {
    let root = base.join("site");
    let data = base.join("private-data");
    fs::create_dir_all(root.join("maps")).unwrap();
    fs::create_dir_all(root.join("dir")).unwrap();
    fs::create_dir_all(data.join("nav")).unwrap();
    fs::write(root.join("index.html"), b"<h1>fixture</h1>").unwrap();
    fs::write(root.join("app.0123456789abcdef.wasm"), b"0123456789").unwrap();
    fs::write(root.join("app.0123456789abcdef.wasm.br"), b"BR").unwrap();
    fs::write(root.join("plus+file.js"), b"plus").unwrap();
    fs::write(root.join("dir/file.js"), b"nested").unwrap();
    fs::write(root.join("maps/root.mkidx"), b"map-index").unwrap();
    fs::write(root.join("maps/root.mkidx.br"), b"wrong-index-representation").unwrap();
    fs::write(root.join("maps/tiles-001.mkshard"), b"map-shard").unwrap();
    fs::write(data.join("nav/test.search"), fixture_search()).unwrap();
    fs::write(data.join("nav/test.graph"), fixture_graph()).unwrap();
    freeze_docroot(&root);
    let address = free_address();
    let child = Command::new(env!("CARGO_BIN_EXE_makepad-web-server"))
        .args([
            "--listen", &address.to_string(),
            "--root", root.to_str().unwrap(),
            "--data-dir", data.to_str().unwrap(),
            "--nav-basename", "nav/test",
            "--searchdb", "off",
            "--places", "off",
            "--major-graph", "off",
            "--chargers", "off",
            "--route-workers", "1",
            "--route-queue", "1",
            "--query-workers", "2",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    wait_until_listening(address);
    (ServerChild(child), address)
}

fn check_static_and_navigation_contracts_work_end_to_end() {
    let tree = TempTree::new("integration");
    let (_server, address) = start_fixture_server(&tree.0);

    let full = get(address, "/", "");
    assert_eq!(full.status, 200);
    assert_eq!(full.body, b"<h1>fixture</h1>");
    assert!(full.headers.contains("Cross-Origin-Opener-Policy: same-origin"));
    assert!(full.headers.contains("Cross-Origin-Embedder-Policy: require-corp"));
    assert!(full.headers.contains("Cache-Control: private, no-cache"));
    assert!(full.headers.contains("Vary: Accept-Encoding"));
    assert!(full.headers.contains("Last-Modified: "));

    let compressed = get(address, "/app.0123456789abcdef.wasm", "Accept-Encoding: gzip, br\r\n");
    assert_eq!(compressed.status, 200);
    assert_eq!(compressed.body, b"BR");
    assert!(compressed.headers.contains("Content-Encoding: br"));
    assert!(compressed.headers.contains("Vary: Accept-Encoding"));
    assert!(compressed.headers.contains("Cache-Control: public, max-age=31536000, immutable"));
    let compressed_etag = compressed
        .headers
        .lines()
        .find_map(|line| line.strip_prefix("ETag: "))
        .unwrap();
    assert!(compressed_etag.ends_with("-br\""));
    assert_eq!(
        get(address, "/app.0123456789abcdef.wasm", "Accept-Encoding: *;q=1\r\n").body,
        b"BR"
    );
    let invalid_quality = get(address, "/app.0123456789abcdef.wasm", "Accept-Encoding: br;q=garbage\r\n");
    assert_eq!(invalid_quality.body, b"0123456789");
    assert!(!invalid_quality.headers.contains("Content-Encoding: br"));
    assert_eq!(get(address, "/plus+file.js", "").body, b"plus");
    assert_eq!(get(address, "/dir//file.js", "").status, 400);
    assert_eq!(get(address, "/dir/./file.js", "").status, 400);

    let partial = get(address, "/app.0123456789abcdef.wasm", "Range: bytes=2-5\r\nAccept-Encoding: br\r\n");
    assert_eq!(partial.status, 206);
    assert_eq!(partial.body, b"2345");
    assert!(partial.headers.contains("Content-Range: bytes 2-5/10"));
    assert!(!partial.headers.contains("Content-Encoding: br"));

    let unsatisfied = get(address, "/maps/root.mkidx", "Range: bytes=99-\r\n");
    assert_eq!(unsatisfied.status, 416);
    assert!(unsatisfied.headers.contains("Content-Range: bytes */9"));
    let index = get(address, "/maps/root.mkidx", "Accept-Encoding: br\r\n");
    assert_eq!(index.body, b"map-index");
    assert!(!index.headers.contains("Content-Encoding: br"));
    assert!(index.headers.contains("Cache-Control: public, max-age=31536000, immutable"));

    let head = request(address, b"HEAD /index.html HTTP/1.1\r\nHost: test\r\n\r\n");
    assert_eq!(head.status, 200);
    assert!(head.body.is_empty());
    assert!(head.headers.contains("Content-Length: 16"));

    let etag_response = get(address, "/index.html", "");
    let etag = etag_response
        .headers
        .lines()
        .find_map(|line| line.strip_prefix("ETag: "))
        .unwrap();
    assert!(!etag.ends_with("-br\""));
    let last_modified = etag_response
        .headers
        .lines()
        .find_map(|line| line.strip_prefix("Last-Modified: "))
        .unwrap();
    let not_modified = get(address, "/index.html", &format!("If-None-Match: {etag}\r\n"));
    assert_eq!(not_modified.status, 304);
    assert!(not_modified.body.is_empty());
    assert_eq!(
        get(address, "/index.html", &format!("If-Modified-Since: {last_modified}\r\n")).status,
        304
    );
    let if_range_match = get(
        address,
        "/index.html",
        &format!("Range: bytes=0-2\r\nIf-Range: {etag}\r\n"),
    );
    assert_eq!(if_range_match.status, 206);
    assert_eq!(if_range_match.body, b"<h1");
    let if_range_date = get(
        address,
        "/index.html",
        &format!("Range: bytes=0-2\r\nIf-Range: {last_modified}\r\n"),
    );
    assert_eq!(if_range_date.status, 206);
    let if_range_miss = get(
        address,
        "/index.html",
        "Range: bytes=0-2\r\nIf-Range: \"different\"\r\n",
    );
    assert_eq!(if_range_miss.status, 200);
    assert_eq!(if_range_miss.body, b"<h1>fixture</h1>");

    let options = request(address, b"OPTIONS /maps/root.mkidx HTTP/1.1\r\nHost: test\r\n\r\n");
    assert_eq!(options.status, 204);
    assert!(options.headers.contains("Access-Control-Allow-Origin: *"));
    assert_eq!(get(address, "/missing.wasm", "").status, 404);
    assert_eq!(
        get(
            address,
            "/ws",
            "Connection: Upgrade\r\nUpgrade: websocket\r\nSec-WebSocket-Key: fixture\r\n",
        )
        .status,
        404
    );
    assert_eq!(post(address, "/index.html", b"ignored").status, 405);
    for (raw, allow) in [
        (
            &b"PUT /api/along HTTP/1.1\r\nHost: test\r\n\r\n"[..],
            "POST, OPTIONS",
        ),
        (
            &b"POST /api/search HTTP/1.1\r\nHost: test\r\n\r\n"[..],
            "GET, HEAD, OPTIONS",
        ),
        (
            &b"POST /index.html HTTP/1.1\r\nHost: test\r\n\r\n"[..],
            "GET, HEAD, OPTIONS",
        ),
        (
            &b"BREW /api/search HTTP/1.1\r\nHost: test\r\n\r\n"[..],
            "GET, HEAD, OPTIONS",
        ),
        (
            &b"POST /ws HTTP/1.1\r\nHost: test\r\nSec-WebSocket-Key: fixture\r\n\r\n"[..],
            "GET, HEAD, OPTIONS",
        ),
    ] {
        let response = request(address, raw);
        assert_eq!(response.status, 405);
        assert!(response.headers.contains(&format!("Allow: {allow}\r\n")));
    }
    let missing_api = request(address, b"DELETE /api/not-real HTTP/1.1\r\nHost: test\r\n\r\n");
    assert_eq!(missing_api.status, 404);
    assert!(String::from_utf8_lossy(&missing_api.body).contains("API endpoint not found"));
    assert_eq!(get(address, "/../index.html", "").status, 400);
    assert_eq!(get(address, "/$report_error?data=boom%0Aline", "").status, 204);
    assert_eq!(post(address, "/$report_error", b"post error").status, 204);
    assert_eq!(post(address, "/$report_error", &vec![b'x'; 8_193]).status, 413);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let health = get(address, "/api/healthz", "");
        if String::from_utf8_lossy(&health.body).contains("\"ok\":true") {
            assert_eq!(health.status, 200);
            assert!(health.headers.contains("Cache-Control: no-store"));
            break;
        }
        assert!(Instant::now() < deadline, "navigation fixture did not become ready");
        thread::sleep(Duration::from_millis(20));
    }

    let search = get(address, "/api/search?q=Fixture%20Museum&near=4.9,52.36&limit=8", "");
    assert_eq!(search.status, 200);
    assert!(String::from_utf8_lossy(&search.body).contains("Fixture Museum"));
    assert!(String::from_utf8_lossy(&search.body).contains("\"query\":\"Fixture Museum\""));
    assert!(search.headers.contains("Cache-Control: private, no-store"));
    assert!(!search.headers.contains("Access-Control-Allow-Origin"));

    let api_options = request(address, b"OPTIONS /api/along HTTP/1.1\r\nHost: test\r\n\r\n");
    assert_eq!(api_options.status, 204);
    assert!(api_options.headers.contains("Allow: POST, OPTIONS"));
    assert!(!api_options.headers.contains("Access-Control-Allow-Origin"));

    let route = get(address, "/api/route?from=4.8952,52.3702&to=4.915,52.35&mode=car", "");
    assert_eq!(route.status, 200, "{}", String::from_utf8_lossy(&route.body));
    assert!(String::from_utf8_lossy(&route.body).contains("\"graph\":\"test\""));

    let along_body = br#"{"polyline":[[4.8952,52.3702],[4.915,52.35]],"cum_dist_m":[0,3000],"kinds":["museum"],"max_detour_min":10,"min_kw":0,"limit":12}"#;
    let along = post(address, "/api/along", along_body);
    assert_eq!(along.status, 200, "{}", String::from_utf8_lossy(&along.body));
    assert!(String::from_utf8_lossy(&along.body).contains("Fixture Museum"));
    assert_eq!(post(address, "/api/along", br#"{"polyline":[]}"#).status, 400);
    assert_eq!(
        request(
            address,
            b"POST /api/along HTTP/1.1\r\nHost: test\r\nContent-Length: 2\r\n\r\n{}",
        )
        .status,
        415
    );
    assert_eq!(
        request(
            address,
            b"POST /api/along HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Encoding: gzip\r\nContent-Length: 2\r\n\r\n{}",
        )
        .status,
        415
    );
    assert_eq!(get(address, "/api/search?q=x&near=999,52", "").status, 400);
}

fn check_unavailable_is_returned_before_navigation_is_ready() {
    let tree = TempTree::new("unavailable");
    let root = tree.0.join("site");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("index.html"), b"ok").unwrap();
    freeze_docroot(&root);
    let address = free_address();
    let child = Command::new(env!("CARGO_BIN_EXE_makepad-web-server"))
        .args(["--listen", &address.to_string(), "--root", root.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _server = ServerChild(child);
    wait_until_listening(address);
    let health = get(address, "/api/healthz", "");
    assert_eq!(health.status, 503);
    assert!(String::from_utf8_lossy(&health.body).contains("\"along\":\"unavailable\""));
    let response = get(address, "/api/search?q=Amsterdam", "");
    assert_eq!(response.status, 503);
    assert!(String::from_utf8_lossy(&response.body).contains("\"code\":\"unavailable\""));
    assert_eq!(
        get(address, "/api/route?from=4.9,52.3&to=5.0,52.2&mode=car", "").status,
        503
    );
    let along = br#"{"polyline":[[4.9,52.3],[5.0,52.2]],"cum_dist_m":[0,10000],"kinds":["museum"]}"#;
    assert_eq!(post(address, "/api/along", along).status, 503);
}

fn check_along_json_rejects_deep_trailing_and_huge_typed_inputs() {
    let tree = TempTree::new("hostile-json");
    let (_server, address) = start_fixture_server(&tree.0);
    let deep = format!(
        "{{\"polyline\":{},\"cum_dist_m\":[0,3000],\"kinds\":[\"museum\"]}}",
        "[".repeat(2_000)
    );
    assert_eq!(post(address, "/api/along", deep.as_bytes()).status, 400);
    let trailing = br#"{"polyline":[[4.9,52.3],[5.0,52.2]],"cum_dist_m":[0,13000],"kinds":["museum"]} []"#;
    assert_eq!(post(address, "/api/along", trailing).status, 400);

    let mut huge = String::from("{\"polyline\":[");
    for index in 0..20_001 {
        if index != 0 { huge.push(','); }
        huge.push_str("[4.9,52.3]");
    }
    huge.push_str("],\"cum_dist_m\":[0,1],\"kinds\":[\"museum\"]}");
    assert!(huge.len() < 2 * 1024 * 1024);
    assert_eq!(post(address, "/api/along", huge.as_bytes()).status, 400);
}

fn check_slow_along_upload_never_occupies_compute_worker() {
    let tree = TempTree::new("slow-along");
    let (_server, address) = start_fixture_server(&tree.0);
    let first = slow_along_upload(address);
    thread::sleep(Duration::from_millis(50));

    let valid = br#"{"polyline":[[4.8952,52.3702],[4.915,52.35]],"cum_dist_m":[0,3000],"kinds":["museum"]}"#;
    let response = post(address, "/api/along", valid);
    assert_eq!(response.status, 200, "{}", String::from_utf8_lossy(&response.body));

    let second = slow_along_upload(address);
    thread::sleep(Duration::from_millis(50));
    let capped = post(address, "/api/along", valid);
    assert_eq!(capped.status, 503, "per-client upload cap must reject promptly");
    let _ = first.shutdown(Shutdown::Both);
    let _ = second.shutdown(Shutdown::Both);
}

fn check_malformed_request_line_is_a_hardened_bad_request() {
    let tree = TempTree::new("bad-line");
    let (_server, address) = start_fixture_server(&tree.0);
    for raw in [
        &b"GET /x HTTP/1.1?q\r\nHost: test\r\n\r\n"[..],
        &b"GET  HTTP/1.1\r\nHost: test\r\n\r\n"[..],
        &b"GET https://example.test/x HTTP/1.1\r\nHost: test\r\n\r\n"[..],
    ] {
        let response = request(address, raw);
        assert_eq!(response.status, 400);
        assert!(response.headers.contains("Cross-Origin-Opener-Policy: same-origin"));
    }
    for raw in [
        &b"POST /api/along HTTP/1.1\r\nContent-Length: 1\r\nContent-Length: 1\r\n\r\nx"[..],
        &b"POST /api/along HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n0\r\n\r\n"[..],
    ] {
        assert_eq!(request(address, raw).status, 400);
    }
}

#[cfg(unix)]
fn check_opened_docroot_refuses_symlink_replacement_escape() {
    use std::os::unix::fs::symlink;
    let tree = TempTree::new("symlink-race");
    let root = tree.0.join("site");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("index.html"), b"public").unwrap();
    freeze_docroot(&root);
    let handler = StaticHandler::new(&root).unwrap();
    let outside = tree.0.join("outside.html");
    fs::write(&outside, b"secret").unwrap();
    assert_eq!(
        root.join("index.html").canonicalize().unwrap(),
        root.canonicalize().unwrap().join("index.html")
    );
    make_tree_writable(&root);
    fs::remove_file(root.join("index.html")).unwrap();
    symlink(&outside, root.join("index.html")).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    handler.handle_get(&direct_headers("GET", "/index.html"), &sender);
    let response = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(matches!(direct_status(&response), 400 | 404));
    assert_ne!(response.body, b"secret");
}

fn check_api_options_has_allow_without_cors() {
    let registry = ServiceRegistry::new(false);
    let (sender, receiver) = std::sync::mpsc::channel();
    assert!(registry.handle_get(&direct_headers("OPTIONS", "/api/along"), &sender));
    let response = receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(direct_status(&response), 204);
    assert!(response.header.contains("Allow: POST, OPTIONS"));
    assert!(!response.header.contains("Access-Control-Allow-Origin"));
}

struct PanicOnceBackend {
    panic_next: AtomicBool,
}

impl NavBackend for PanicOnceBackend {
    fn search(&self, _request: SearchRequest) -> Result<Vec<SearchResult>, ApiFailure> {
        if self.panic_next.swap(false, Ordering::SeqCst) {
            panic!("adversarial worker panic");
        }
        Ok(Vec::new())
    }

    fn route(&self, _request: RouteRequest) -> Result<RouteResult, ApiFailure> {
        Err(ApiFailure { status: 422, code: "not_found", message: "fixture".into() })
    }

    fn along(&self, _request: AlongRequest) -> Result<Vec<AlongResult>, ApiFailure> {
        Ok(Vec::new())
    }
}

fn check_worker_panic_returns_error_recovers_and_degrades_health() {
    let registry = ServiceRegistry::new(false);
    registry.install(
        Arc::new(PanicOnceBackend { panic_next: AtomicBool::new(true) }),
        1,
        1,
        2,
        false,
    );
    assert_eq!(direct_status(&direct_get(&registry, "/api/search?q=first")), 500);
    assert_eq!(direct_status(&direct_get(&registry, "/api/search?q=second")), 200);
    let health = direct_get(&registry, "/api/healthz");
    assert!(String::from_utf8_lossy(&health.body).contains("\"ok\":false"));
    assert!(String::from_utf8_lossy(&health.body).contains("\"search\":\"degraded\""));
    assert_eq!(direct_status(&health), 503);
}

struct PanicAlongBackend;

impl NavBackend for PanicAlongBackend {
    fn search(&self, _request: SearchRequest) -> Result<Vec<SearchResult>, ApiFailure> {
        Ok(Vec::new())
    }

    fn route(&self, _request: RouteRequest) -> Result<RouteResult, ApiFailure> {
        Err(ApiFailure { status: 422, code: "not_found", message: "fixture".into() })
    }

    fn along(&self, _request: AlongRequest) -> Result<Vec<AlongResult>, ApiFailure> {
        panic!("adversarial along panic")
    }
}

fn check_along_panic_degrades_along_not_search() {
    let registry = ServiceRegistry::new(false);
    registry.install(Arc::new(PanicAlongBackend), 2, 1, 2, false);
    let (sender, receiver) = std::sync::mpsc::channel();
    assert!(registry.handle_post(
        &direct_headers("POST", "/api/along"),
        br#"{"polyline":[[4.9,52.3],[5.0,52.2]],"cum_dist_m":[0,1],"kinds":["museum"]}"#.to_vec(),
        &sender,
    ));
    assert_eq!(direct_status(&receiver.recv_timeout(Duration::from_secs(2)).unwrap()), 500);
    let health = direct_get(&registry, "/api/healthz");
    let body = String::from_utf8_lossy(&health.body);
    assert_eq!(direct_status(&health), 503);
    assert!(body.contains("\"search\":\"ready\""), "{body}");
    assert!(body.contains("\"along\":\"degraded\""), "{body}");
    assert!(body.contains("\"chargers\":\"disabled\""), "{body}");
}

struct BlockingAlongBackend {
    state: Arc<(Mutex<(bool, bool)>, Condvar)>,
}

impl NavBackend for BlockingAlongBackend {
    fn search(&self, _request: SearchRequest) -> Result<Vec<SearchResult>, ApiFailure> {
        Ok(Vec::new())
    }

    fn route(&self, _request: RouteRequest) -> Result<RouteResult, ApiFailure> {
        Err(ApiFailure { status: 422, code: "not_found", message: "fixture".into() })
    }

    fn along(&self, _request: AlongRequest) -> Result<Vec<AlongResult>, ApiFailure> {
        let (lock, changed) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.0 = true;
        changed.notify_all();
        while !state.1 {
            state = changed.wait(state).unwrap();
        }
        Ok(Vec::new())
    }
}

fn check_along_admission_cannot_starve_search_capacity() {
    let state = Arc::new((Mutex::new((false, false)), Condvar::new()));
    let registry = ServiceRegistry::new(false);
    registry.install(
        Arc::new(BlockingAlongBackend { state: state.clone() }),
        1,
        1,
        1,
        false,
    );
    let along_registry = registry.clone();
    let along = thread::spawn(move || {
        let (sender, receiver) = std::sync::mpsc::channel();
        assert!(along_registry.handle_post(
            &direct_headers("POST", "/api/along"),
            br#"{"polyline":[[4.9,52.3],[5.0,52.2]],"cum_dist_m":[0,13000],"kinds":["museum"]}"#.to_vec(),
            &sender,
        ));
        receiver.recv_timeout(Duration::from_secs(2)).unwrap()
    });
    {
        let (lock, changed) = &*state;
        let mut current = lock.lock().unwrap();
        while !current.0 { current = changed.wait(current).unwrap(); }
    }
    let started = Instant::now();
    assert_eq!(direct_status(&direct_get(&registry, "/api/search?q=ready")), 200);
    assert!(started.elapsed() < Duration::from_secs(1));
    {
        let (lock, changed) = &*state;
        let mut current = lock.lock().unwrap();
        current.1 = true;
        changed.notify_all();
    }
    assert_eq!(direct_status(&along.join().unwrap()), 200);
}

fn disconnected_graph() -> Vec<u8> {
    let mut builder = GraphBuilder::new();
    for (id, lon, lat) in [(1, 4.0, 52.0), (2, 4.01, 52.0), (3, 5.0, 52.0), (4, 5.01, 52.0)] {
        builder.add_node(id, lon, lat);
    }
    let mut tags = HashMap::new();
    tags.insert("highway".into(), "residential".into());
    builder.add_way(1, vec![1, 2], tags.clone());
    builder.add_way(2, vec![3, 4], tags);
    builder.build().serialize()
}

fn major_graph() -> Vec<u8> {
    let mut builder = GraphBuilder::new();
    builder.add_node(1, 4.0, 52.0);
    builder.add_node(2, 4.5, 52.0);
    builder.add_node(3, 5.0, 52.0);
    let mut tags = HashMap::new();
    tags.insert("highway".into(), "primary".into());
    builder.add_way(1, vec![1, 2, 3], tags);
    builder.build().serialize()
}

fn check_route_falls_back_as_a_whole_to_major_graph() {
    let tree = TempTree::new("major-fallback");
    let root = tree.0.join("site");
    let data = tree.0.join("private-data/nav");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&data).unwrap();
    fs::write(root.join("index.html"), b"ok").unwrap();
    fs::write(data.join("test.search"), fixture_search()).unwrap();
    fs::write(data.join("test.graph"), disconnected_graph()).unwrap();
    fs::write(data.join("major.graph"), major_graph()).unwrap();
    freeze_docroot(&root);
    let address = free_address();
    let child = Command::new(env!("CARGO_BIN_EXE_makepad-web-server"))
        .args([
            "--listen", &address.to_string(), "--root", root.to_str().unwrap(),
            "--data-dir", tree.0.join("private-data").to_str().unwrap(),
            "--nav-basename", "nav/test", "--searchdb", "off", "--places", "off",
            "--major-graph", "nav/major.graph", "--chargers", "off",
        ])
        .stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
    let _server = ServerChild(child);
    wait_until_listening(address);
    let deadline = Instant::now() + Duration::from_secs(5);
    while !String::from_utf8_lossy(&get(address, "/api/healthz", "").body).contains("\"ok\":true") {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(20));
    }
    let route = get(address, "/api/route?from=4.0,52.0&to=5.0,52.0&mode=car", "");
    assert_eq!(route.status, 200, "{}", String::from_utf8_lossy(&route.body));
    assert!(String::from_utf8_lossy(&route.body).contains("\"graph\":\"europe-major\""));
}

#[test]
fn live_server_adversarial_contracts() {
    check_along_sampling_matches_trip_reference_policy();
    check_report_rate_limiter_is_bounded_and_expires_entries();
    check_dense_charger_query_obeys_shared_scan_budget_and_top_k_bound();
    check_worker_panic_returns_error_recovers_and_degrades_health();
    check_along_panic_degrades_along_not_search();
    check_along_admission_cannot_starve_search_capacity();
    check_full_route_queue_returns_busy();
    check_api_options_has_allow_without_cors();
    #[cfg(unix)]
    check_opened_docroot_refuses_symlink_replacement_escape();

    // Match the network crate's integration-test convention: managed runners
    // may deny listener creation. The same checks run whenever loopback bind
    // is available (and are also covered at the shared parser boundary).
    let Ok(probe) = TcpListener::bind("127.0.0.1:0") else {
        eprintln!("live site-server checks skipped: loopback bind unavailable");
        return;
    };
    drop(probe);
    check_crash_endpoint_contracts();
    check_slow_crash_body_does_not_hold_dispatcher();
    check_static_and_navigation_contracts_work_end_to_end();
    check_unavailable_is_returned_before_navigation_is_ready();
    check_along_json_rejects_deep_trailing_and_huge_typed_inputs();
    check_slow_along_upload_never_occupies_compute_worker();
    check_malformed_request_line_is_a_hardened_bad_request();
    check_route_falls_back_as_a_whole_to_major_graph();
}

fn check_crash_endpoint_contracts() {
    let tree = TempTree::new("crash-endpoint");
    let (_server, address) = start_fixture_server(&tree.0);
    let data = tree.0.join("private-data");
    let body = b"{\n\"kind\":\"panic\",\"data\":{\"message\":\"boom\"}\n}";

    let stored = post(address, "/api/crash", body);
    assert_eq!(stored.status, 204);
    assert!(stored.headers.contains("Cache-Control: private, no-cache"));
    assert_eq!(
        post_with_type(address, "/api/crash", "text/plain;charset=UTF-8", b"plain").status,
        204
    );

    let log = fs::read_to_string(data.join("crash.log")).unwrap();
    let mut fields = log.lines().next().unwrap().splitn(3, ' ');
    assert!(fields.next().unwrap().parse::<u128>().is_ok());
    assert_eq!(fields.next(), Some("127.0.0.1"));
    let escaped_body = String::from_utf8_lossy(body).replace('\n', "\\n");
    assert_eq!(fields.next(), Some(escaped_body.as_str()));

    let wrong_method = get(address, "/api/crash", "");
    assert_eq!(wrong_method.status, 405);
    assert!(wrong_method.headers.contains("Allow: POST, OPTIONS"));
    let options = request(address, b"OPTIONS /api/crash HTTP/1.1\r\nHost: test\r\n\r\n");
    assert_eq!(options.status, 204);
    assert!(options.headers.contains("Allow: POST, OPTIONS"));
    assert_eq!(post(address, "/api/crash", &[0xff]).status, 400);
    assert_eq!(post(address, "/api/crash", &vec![b'x'; 64 * 1024 + 1]).status, 413);

    assert_eq!(get(address, "/$report_error?data=old%0Aline", "").status, 204);
    let log = fs::read_to_string(data.join("crash.log")).unwrap();
    assert!(log.contains("{\"kind\":\"legacy-get\",\"data\":\"old\\nline\"}"));

    for _ in 0..26 {
        assert_eq!(post(address, "/api/crash", b"{}").status, 204);
    }
    assert_eq!(post(address, "/api/crash", b"{}").status, 429);
}

fn check_slow_crash_body_does_not_hold_dispatcher() {
    let tree = TempTree::new("crash-slow-body");
    let (_server, address) = start_fixture_server(&tree.0);
    let mut slow = TcpStream::connect_timeout(&address, Duration::from_secs(2)).unwrap();
    slow.write_all(
        b"POST /api/crash HTTP/1.1\r\nHost: test\r\nContent-Type: application/json\r\nContent-Length: 64\r\n\r\n",
    )
    .unwrap();
    thread::sleep(Duration::from_millis(50));
    assert_eq!(get(address, "/", "").status, 200);
    drop(slow);
}

fn reference_trip_samples(line: &[LonLat], spacing_m: f64) -> Vec<(LonLat, f64)> {
    let mut samples = vec![(line[0], 0.0)];
    let mut cumulative = 0.0;
    let mut next = spacing_m;
    for segment in line.windows(2) {
        let length = makepad_map_nav::geo::haversine_m(segment[0], segment[1]);
        while next <= cumulative + length {
            let fraction = (next - cumulative) / length;
            samples.push((LonLat::new(
                segment[0].lon + (segment[1].lon - segment[0].lon) * fraction,
                segment[0].lat + (segment[1].lat - segment[0].lat) * fraction,
            ), next));
            next += spacing_m;
        }
        cumulative += length;
    }
    if cumulative - samples.last().unwrap().1 > spacing_m * 0.5 {
        samples.push((*line.last().unwrap(), cumulative));
    }
    samples
}

fn check_along_sampling_matches_trip_reference_policy() {
    let line = [LonLat::new(0.0, 0.0), LonLat::new(0.04, 0.0), LonLat::new(0.09, 0.0)];
    let total = makepad_map_nav::geo::haversine_m(line[0], line[1])
        + makepad_map_nav::geo::haversine_m(line[1], line[2]);
    let spacing = (total / 48.0).max(3_000.0);
    let actual = sample_along(&line);
    let expected = reference_trip_samples(&line, spacing);
    assert_eq!(actual.len(), expected.len());
    for (actual, expected) in actual.iter().zip(expected.iter()) {
        assert!((actual.0.lon - expected.0.lon).abs() < 1e-10);
        assert!((actual.0.lat - expected.0.lat).abs() < 1e-10);
        assert!((actual.1 - expected.1).abs() < 1e-6);
    }
}

fn check_report_rate_limiter_is_bounded_and_expires_entries() {
    let now = Instant::now();
    let mut limiter = ReportRateLimiter::new(3);
    for octet in 1..=3 {
        assert!(limiter.allow_at(IpAddr::V4(Ipv4Addr::new(192, 0, 2, octet)), now));
    }
    assert_eq!(limiter.len(), 3);
    assert!(!limiter.allow_at(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 4)), now));
    assert!(limiter.allow_at(IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)), now));
    assert!(limiter.allow_at(
        IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)),
        now + Duration::from_secs(61),
    ));
    assert_eq!(limiter.len(), 1);
}

fn check_dense_charger_query_obeys_shared_scan_budget_and_top_k_bound() {
    let tree = TempTree::new("dense-chargers");
    let path = tree.0.join("chargers.mbtiles");
    let mut writer = MbtilesWriter::create(&path).unwrap();
    let mut sidecar = SidecarBuilder::new();
    for index in 0..1_000 {
        sidecar.add(
            "chargers",
            &Geometry::Point(4.9 + f64::from(index) * 1e-8, 52.3),
            &[("max_kw".into(), AttrVal::Int(150))],
            false,
        );
    }
    assert_eq!(sidecar.write(&mut writer).unwrap(), 1_000);
    writer.finish().unwrap();
    let mut database = LayerDb::open(&path).unwrap();
    let mut budget = 17;
    let hits = database
        .query_radius_with_budget(4.9, 52.3, 1_000.0, 8, &mut budget)
        .unwrap();
    assert_eq!(budget, 0);
    assert_eq!(hits.len(), 8);
}

struct BlockingBackend {
    state: Arc<(Mutex<(usize, bool)>, Condvar)>,
}

impl NavBackend for BlockingBackend {
    fn search(&self, _request: SearchRequest) -> Result<Vec<SearchResult>, ApiFailure> {
        Ok(Vec::new())
    }

    fn route(&self, _request: RouteRequest) -> Result<RouteResult, ApiFailure> {
        let (lock, changed) = &*self.state;
        let mut state = lock.lock().unwrap();
        state.0 += 1;
        changed.notify_all();
        while !state.1 {
            state = changed.wait(state).unwrap();
        }
        Err(ApiFailure { status: 422, code: "not_found", message: "fixture".into() })
    }

    fn along(&self, _request: AlongRequest) -> Result<Vec<AlongResult>, ApiFailure> {
        Ok(Vec::new())
    }
}

fn check_full_route_queue_returns_busy() {
    let state = Arc::new((Mutex::new((0usize, false)), Condvar::new()));
    let registry = ServiceRegistry::new(false);
    registry.install(
        Arc::new(BlockingBackend { state: state.clone() }),
        1,
        1,
        1,
        false,
    );

    let path = "/api/route?from=4.9,52.3&to=5.0,52.2&mode=car";
    let first_registry = registry.clone();
    let first = thread::spawn(move || direct_get(&first_registry, path));
    {
        let (lock, changed) = &*state;
        let mut current = lock.lock().unwrap();
        while current.0 == 0 {
            current = changed.wait(current).unwrap();
        }
    }
    let (result_sender, result_receiver) = std::sync::mpsc::channel();
    let mut contenders = Vec::new();
    for _ in 0..3 {
        let result_sender = result_sender.clone();
        let registry = registry.clone();
        contenders.push(thread::spawn(move || {
            let response = direct_get(&registry, path);
            let _ = result_sender.send(direct_status(&response));
        }));
    }
    drop(result_sender);
    let status = result_receiver.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(status, 429);
    {
        let (lock, changed) = &*state;
        let mut current = lock.lock().unwrap();
        current.1 = true;
        changed.notify_all();
    }
    let _ = first.join();
    for contender in contenders {
        let _ = contender.join();
    }
}
