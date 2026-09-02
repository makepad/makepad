use makepad_map_nav::{
    geo::LonLat,
    graph::GraphBuilder,
    search::{Category, SearchIndexBuilder, SearchResult},
};
use makepad_web_server::{
    api::{AlongRequest, AlongResult, ApiFailure, NavBackend, RouteRequest, RouteResult, SearchRequest, ServiceRegistry},
    run_with_registry, Config,
};
use std::{
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::{Shutdown, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{Arc, Condvar, Mutex},
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
        let _ = fs::remove_dir_all(&self.0);
    }
}

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
    stream.read_to_end(&mut bytes).unwrap();
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
    fs::create_dir_all(data.join("nav")).unwrap();
    fs::write(root.join("index.html"), b"<h1>fixture</h1>").unwrap();
    fs::write(root.join("app-a1b2c3d4.wasm"), b"0123456789").unwrap();
    fs::write(root.join("app-a1b2c3d4.wasm.br"), b"BR").unwrap();
    fs::write(root.join("maps/root.mkidx"), b"map-index").unwrap();
    fs::write(root.join("maps/root.mkidx.br"), b"wrong-index-representation").unwrap();
    fs::write(root.join("maps/tiles-001.mkshard"), b"map-shard").unwrap();
    fs::write(data.join("nav/test.search"), fixture_search()).unwrap();
    fs::write(data.join("nav/test.graph"), fixture_graph()).unwrap();
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

#[test]
fn static_and_navigation_contracts_work_end_to_end() {
    let tree = TempTree::new("integration");
    let (_server, address) = start_fixture_server(&tree.0);

    let full = get(address, "/", "");
    assert_eq!(full.status, 200);
    assert_eq!(full.body, b"<h1>fixture</h1>");
    assert!(full.headers.contains("Cross-Origin-Opener-Policy: same-origin"));
    assert!(full.headers.contains("Cross-Origin-Embedder-Policy: require-corp"));

    let compressed = get(address, "/app-a1b2c3d4.wasm", "Accept-Encoding: gzip, br\r\n");
    assert_eq!(compressed.status, 200);
    assert_eq!(compressed.body, b"BR");
    assert!(compressed.headers.contains("Content-Encoding: br"));
    assert!(compressed.headers.contains("Vary: Accept-Encoding"));

    let partial = get(address, "/app-a1b2c3d4.wasm", "Range: bytes=2-5\r\nAccept-Encoding: br\r\n");
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
    let not_modified = get(address, "/index.html", &format!("If-None-Match: {etag}\r\n"));
    assert_eq!(not_modified.status, 304);
    assert!(not_modified.body.is_empty());

    let options = request(address, b"OPTIONS /maps/root.mkidx HTTP/1.1\r\nHost: test\r\n\r\n");
    assert_eq!(options.status, 204);
    assert!(options.headers.contains("Access-Control-Allow-Origin: *"));
    assert_eq!(get(address, "/missing.wasm", "").status, 404);
    assert_eq!(post(address, "/index.html", b"ignored").status, 405);
    assert_eq!(get(address, "/../index.html", "").status, 400);
    assert_eq!(get(address, "/$report_error?data=boom%0Aline", "").status, 204);
    assert_eq!(post(address, "/$report_error", b"post error").status, 204);

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

    let route = get(address, "/api/route?from=4.8952,52.3702&to=4.915,52.35&mode=car", "");
    assert_eq!(route.status, 200, "{}", String::from_utf8_lossy(&route.body));
    assert!(String::from_utf8_lossy(&route.body).contains("\"graph\":\"test\""));

    let along_body = br#"{"polyline":[[4.8952,52.3702],[4.915,52.35]],"cum_dist_m":[0,3000],"kinds":["museum"],"max_detour_min":10,"min_kw":0,"limit":12}"#;
    let along = post(address, "/api/along", along_body);
    assert_eq!(along.status, 200, "{}", String::from_utf8_lossy(&along.body));
    assert!(String::from_utf8_lossy(&along.body).contains("Fixture Museum"));
    assert_eq!(post(address, "/api/along", br#"{"polyline":[]}"#).status, 400);
    assert_eq!(get(address, "/api/search?q=x&near=999,52", "").status, 400);
}

#[test]
fn unavailable_is_returned_before_navigation_is_ready() {
    let tree = TempTree::new("unavailable");
    let root = tree.0.join("site");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("index.html"), b"ok").unwrap();
    let address = free_address();
    let child = Command::new(env!("CARGO_BIN_EXE_makepad-web-server"))
        .args(["--listen", &address.to_string(), "--root", root.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let _server = ServerChild(child);
    wait_until_listening(address);
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

#[test]
fn full_route_queue_returns_busy() {
    let tree = TempTree::new("busy");
    let root = tree.0.join("site");
    fs::create_dir(&root).unwrap();
    fs::write(root.join("index.html"), b"ok").unwrap();
    let address = free_address();
    let mut config = Config::parse([root.to_string_lossy().as_ref()]).unwrap();
    config.listen = address;
    config.route_queue = 1;
    config.route_workers = 1;
    let state = Arc::new((Mutex::new((0usize, false)), Condvar::new()));
    let registry = ServiceRegistry::new(false);
    registry.install(
        Arc::new(BlockingBackend { state: state.clone() }),
        1,
        1,
        1,
        false,
    );
    thread::spawn(move || {
        let _ = run_with_registry(config, registry, false);
    });
    wait_until_listening(address);

    let path = "/api/route?from=4.9,52.3&to=5.0,52.2&mode=car";
    let first = thread::spawn(move || get(address, path, ""));
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
        contenders.push(thread::spawn(move || {
            let response = get(address, path, "");
            let _ = result_sender.send(response.status);
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
