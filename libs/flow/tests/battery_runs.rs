#![cfg(not(target_arch = "wasm32"))]
//! T3 destructive socket battery for the instance, run, value and event
//! routes. Every operation under test goes through one of FlowServer's two
//! HTTP planes; only fixture setup and assertions use library types.

mod support;

use makepad_ai_hub::discovery::DEFAULT_FLEET;
use makepad_ai_hub::download::Downloader;
use makepad_ai_hub::peer_serve::PeerOptions;
use makepad_ai_hub::registry::{Domain, ModelSpec, Registry};
use makepad_ai_hub::server::{start_service, ServiceConfig};
use makepad_flow::engine::{FixedGen, HubHttp, NetPolicy};
use makepad_flow::host::{Endpoints, FlowServer, FlowServerConfig};
use makepad_flow::{
    CreateInstanceRequest, CreateInstanceResponse, CreateRunRequest, CreateRunResponse,
    EventsResponse, FlowResponse, InputValueDto, InstanceRow, Literal, PortType, PutSourceRequest,
    PutValueResponse, RunRowDto, RunState, Seams,
};
use makepad_micro_serde::{DeJson, JsonValue, SerJson};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use support::{seams, FakeChat, FakeGen, FakeHttp};

// This battery opens two FlowServer listeners per case (and two more for
// real HTTP/gen cases). Some CI sandboxes reject concurrent binds instead
// of merely assigning separate ephemeral ports, so keep this test binary's
// socket ownership deterministic.
static SOCKET_TEST: Mutex<()> = Mutex::new(());

fn socket_test() -> std::sync::MutexGuard<'static, ()> {
    SOCKET_TEST.lock().unwrap_or_else(|poison| poison.into_inner())
}

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "makepad-flow-battery-runs-{}-{label}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Debug)]
struct HttpResponse {
    status: u16,
    headers: HashMap<String, String>,
    bytes: Vec<u8>,
}

impl HttpResponse {
    fn body(&self) -> String {
        String::from_utf8_lossy(&self.bytes).into_owned()
    }
}

fn request(
    address: SocketAddr,
    method: &str,
    target: &str,
    token: Option<&str>,
    headers: &[(&str, &str)],
    body: &[u8],
) -> HttpResponse {
    let mut stream = TcpStream::connect(address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(35)))
        .unwrap();
    stream
        .set_write_timeout(Some(Duration::from_secs(35)))
        .unwrap();
    write!(stream, "{method} {target} HTTP/1.1\r\nHost: localhost\r\n").unwrap();
    if let Some(token) = token {
        write!(stream, "Authorization: Bearer {token}\r\n").unwrap();
    }
    for (name, value) in headers {
        write!(stream, "{name}: {value}\r\n").unwrap();
    }
    write!(
        stream,
        "Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .unwrap();
    stream.write_all(body).unwrap();
    stream.flush().unwrap();

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let boundary = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .expect("HTTP response head terminator");
    let head = std::str::from_utf8(&raw[..boundary]).expect("HTTP response head utf-8");
    let status = head
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|status| status.parse().ok())
        .expect("HTTP response status");
    let headers = head
        .lines()
        .skip(1)
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    HttpResponse {
        status,
        headers,
        bytes: raw[boundary + 4..].to_vec(),
    }
}

fn json_request(
    address: SocketAddr,
    method: &str,
    target: &str,
    token: Option<&str>,
    body: &str,
) -> HttpResponse {
    request(
        address,
        method,
        target,
        token,
        &[("Content-Type", "application/json")],
        body.as_bytes(),
    )
}

fn base_config(root: &Path, seams: Seams) -> FlowServerConfig {
    let mut config = FlowServerConfig::new(root.to_path_buf()).with_seams(seams);
    config.watch_interval_ms = 25;
    config.janitor_sweep_secs = 1;
    config.log = Box::new(|_| {});
    config
}

fn start(root: &Path, seams: Seams) -> FlowServer {
    FlowServer::start(base_config(root, seams)).unwrap()
}

fn put_flow(endpoints: &Endpoints, name: &str, source: &str) {
    let response = json_request(
        endpoints.control,
        "PUT",
        &format!("/v1/flows/{name}"),
        Some(&endpoints.token),
        &PutSourceRequest {
            source: source.to_string(),
        }
        .serialize_json(),
    );
    assert_eq!(response.status, 200, "PUT flow {name}: {}", response.body());
}

fn create_instance(endpoints: &Endpoints, flow: &str, pin: bool) -> String {
    let response = json_request(
        endpoints.control,
        "POST",
        &format!("/v1/flows/{flow}/instances"),
        Some(&endpoints.token),
        &CreateInstanceRequest {
            pin: pin.then_some(true),
            ..CreateInstanceRequest::default()
        }
        .serialize_json(),
    );
    assert_eq!(response.status, 201, "create {flow}: {}", response.body());
    CreateInstanceResponse::deserialize_json(&response.body())
        .unwrap()
        .instance
}

fn start_run(endpoints: &Endpoints, instance: &str, outputs: Option<Vec<String>>) -> CreateRunResponse {
    let response = json_request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{instance}/runs"),
        Some(&endpoints.token),
        &CreateRunRequest { outputs }.serialize_json(),
    );
    assert_eq!(response.status, 202, "start run: {}", response.body());
    CreateRunResponse::deserialize_json(&response.body()).unwrap()
}

fn event_cursor(endpoints: &Endpoints, topic: &str) -> String {
    let response = json_request(
        endpoints.control,
        "GET",
        &format!("/v1/events?topic={topic}"),
        Some(&endpoints.token),
        "",
    );
    assert_eq!(response.status, 200, "events cursor: {}", response.body());
    EventsResponse::deserialize_json(&response.body())
        .unwrap()
        .cursor
}

fn event_kind(event: &JsonValue) -> &str {
    event
        .key("kind")
        .and_then(JsonValue::string)
        .map(String::as_str)
        .unwrap_or("")
}

fn event_str<'a>(event: &'a JsonValue, key: &str) -> Option<&'a str> {
    event
        .key(key)
        .and_then(JsonValue::string)
        .map(String::as_str)
}

fn event_seq(event: &JsonValue) -> u64 {
    match event.key("seq") {
        Some(JsonValue::U64(value)) => *value,
        Some(JsonValue::I64(value)) => (*value).try_into().unwrap(),
        other => panic!("event has no numeric seq: {other:?}"),
    }
}

fn event_u64(event: &JsonValue, key: &str) -> Option<u64> {
    match event.key(key) {
        Some(JsonValue::U64(value)) => Some(*value),
        Some(JsonValue::I64(value)) => (*value).try_into().ok(),
        _ => None,
    }
}

fn variant_name(value: Option<&JsonValue>) -> Option<&str> {
    match value {
        Some(JsonValue::Object(fields)) if fields.len() == 1 => {
            fields.keys().next().map(String::as_str)
        }
        _ => None,
    }
}

fn poll_events_until(
    endpoints: &Endpoints,
    topic: &str,
    mut cursor: String,
    timeout: Duration,
    mut stop: impl FnMut(&JsonValue) -> bool,
) -> Vec<JsonValue> {
    let deadline = Instant::now() + timeout;
    let mut events = Vec::new();
    loop {
        let response = json_request(
            endpoints.control,
            "GET",
            &format!("/v1/events?cursor={cursor}&wait=500&topic={topic}"),
            Some(&endpoints.token),
            "",
        );
        assert_eq!(response.status, 200, "poll events: {}", response.body());
        let page = EventsResponse::deserialize_json(&response.body()).unwrap();
        assert!(!page.gap, "unexpected event gap while polling {topic}");
        cursor = page.cursor;
        for event in page.events {
            let done = stop(&event);
            events.push(event);
            if done {
                return events;
            }
        }
        assert!(Instant::now() < deadline, "event deadline elapsed for {topic}: {events:#?}");
    }
}

fn get_run(endpoints: &Endpoints, run_id: &str) -> RunRowDto {
    let response = json_request(
        endpoints.control,
        "GET",
        &format!("/v1/runs/{run_id}"),
        Some(&endpoints.token),
        "",
    );
    assert_eq!(response.status, 200, "get run: {}", response.body());
    RunRowDto::deserialize_json(&response.body()).unwrap()
}

fn wait_run_terminal(endpoints: &Endpoints, run_id: &str, timeout: Duration) -> RunRowDto {
    let deadline = Instant::now() + timeout;
    loop {
        let row = get_run(endpoints, run_id);
        if matches!(row.state, RunState::Done | RunState::Failed | RunState::Cancelled) {
            return row;
        }
        assert!(Instant::now() < deadline, "run {run_id} did not finish: {row:#?}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn text_input(text: &str) -> InputValueDto {
    InputValueDto {
        ty: PortType::Text,
        text: Some(text.to_string()),
        json: None,
        digest: None,
    }
}

fn json_input(value: JsonValue) -> InputValueDto {
    InputValueDto {
        ty: PortType::Json,
        text: None,
        json: Some(value),
        digest: None,
    }
}

fn image_input(digest: &str) -> InputValueDto {
    InputValueDto {
        ty: PortType::Image,
        text: None,
        json: None,
        digest: Some(digest.to_string()),
    }
}

fn one_input(
    node: &str,
    port: &str,
    value: InputValueDto,
) -> HashMap<String, HashMap<String, InputValueDto>> {
    HashMap::from([(node.to_string(), HashMap::from([(port.to_string(), value)]))])
}

fn put_inputs(
    endpoints: &Endpoints,
    instance: &str,
    actor: Option<&str>,
    inputs: &HashMap<String, HashMap<String, InputValueDto>>,
) -> HttpResponse {
    let suffix = actor.map(|actor| format!("?actor={actor}")).unwrap_or_default();
    json_request(
        endpoints.control,
        "PUT",
        &format!("/v1/instances/{instance}/inputs{suffix}"),
        Some(&endpoints.token),
        &inputs.serialize_json(),
    )
}

fn put_value(endpoints: &Endpoints, ty: &str, content_type: &str, bytes: &[u8]) -> (HttpResponse, String) {
    let response = request(
        endpoints.data,
        "PUT",
        &format!("/v1/values?type={ty}&content_type={content_type}"),
        Some(&endpoints.token),
        &[("Content-Type", content_type)],
        bytes,
    );
    let digest = if response.status == 201 {
        PutValueResponse::deserialize_json(&response.body())
            .unwrap()
            .digest
    } else {
        String::new()
    };
    (response, digest)
}

fn start_testpattern(label: &str) -> Option<String> {
    let cache = std::env::temp_dir().join(format!(
        "makepad-flow-battery-runs-hub-{}-{label}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir_all(&cache).unwrap();
    let service = start_service(ServiceConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        cache_dir: cache,
        registry: Registry {
            models: vec![ModelSpec {
                id: "testpattern".to_string(),
                domain: Domain::Image,
                backend: "testpattern".to_string(),
                available: true,
                gated: false,
                vram_gb: Some(0.0),
                min_vram_gb: None,
                min_compute_cap: None,
                note: None,
                license: None,
                files: Vec::new(),
            }],
        },
        downloader: Downloader::new("http://127.0.0.1:1", None).unwrap(),
        peer: PeerOptions {
            serve: Some(false),
            sources: Some(Vec::new()),
            ..Default::default()
        },
        fleet: DEFAULT_FLEET.to_string(),
    });
    let handle = match service {
        Ok(handle) => handle,
        Err(error) if error.to_string().contains("Operation not permitted") => {
            eprintln!("skipping real testpattern service: loopback bind is forbidden");
            return None;
        }
        Err(error) => panic!("start testpattern service: {error}"),
    };
    let url = format!("http://{}", handle.addr);
    // The hub service has no stop message. This integration-test process
    // owns the service threads and tears them down when the binary exits.
    std::mem::forget(handle);
    Some(url)
}

#[derive(Clone, Debug)]
struct FixtureRequest {
    method: String,
    path: String,
    content_type: Option<String>,
    body: Vec<u8>,
}

struct FixtureServer {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<FixtureRequest>>>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl FixtureServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = requests.clone();
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = stop.clone();
        let join = std::thread::spawn(move || {
            while !thread_stop.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_read_timeout(Some(Duration::from_secs(5)))
                            .unwrap();
                        let request = read_fixture_request(&mut stream).unwrap();
                        let (status, content_type, body) = match (request.method.as_str(), request.path.as_str()) {
                            ("GET", "/today.json") => {
                                (200, "application/json", br#"["a red fox", "a blue heron"]"#.to_vec())
                            }
                            ("POST", "/pictures") => {
                                (200, "application/json", br#"{"stored":true}"#.to_vec())
                            }
                            _ => (404, "application/json", br#"{"error":"not found"}"#.to_vec()),
                        };
                        thread_requests.lock().unwrap().push(request);
                        write!(
                            stream,
                            "HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .unwrap();
                        stream.write_all(&body).unwrap();
                        stream.flush().unwrap();
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("fixture accept: {error}"),
                }
            }
        });
        Self {
            address,
            requests,
            stop,
            join: Some(join),
        }
    }

    fn base_url(&self) -> String {
        format!("http://{}", self.address)
    }

    fn posted(&self) -> Option<FixtureRequest> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .find(|request| request.method == "POST" && request.path == "/pictures")
            .cloned()
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn read_fixture_request(stream: &mut TcpStream) -> io::Result<FixtureRequest> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    let boundary = loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "fixture request head"));
        }
        raw.extend_from_slice(&buffer[..read]);
        if let Some(boundary) = raw.windows(4).position(|window| window == b"\r\n\r\n") {
            break boundary;
        }
        if raw.len() > 64 * 1024 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "fixture request head too large"));
        }
    };
    let head = std::str::from_utf8(&raw[..boundary])
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    let mut lines = head.lines();
    let mut request_line = lines.next().unwrap_or("").split_whitespace();
    let method = request_line.next().unwrap_or("").to_string();
    let path = request_line.next().unwrap_or("").to_string();
    let mut content_length = 0usize;
    let mut content_type = None;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        if name.eq_ignore_ascii_case("content-length") {
            content_length = value.trim().parse().unwrap_or(0);
        }
        if name.eq_ignore_ascii_case("content-type") {
            content_type = Some(value.trim().to_string());
        }
    }
    let mut body = raw[boundary + 4..].to_vec();
    while body.len() < content_length {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        body.extend_from_slice(&buffer[..read]);
    }
    body.truncate(content_length);
    Ok(FixtureRequest {
        method,
        path,
        content_type,
        body,
    })
}

const ASK_FLOW: &str = r#"use mod.flow.*
let first = Ask{question: "Which?" type: @text options: ["first", "second"]}
let other = Ask{question: "Other?" type: @text}
let result = Output{type: @text value: first.text()}
let other_result = Output{type: @text value: other.text()}
Flow{first, other, result, other_result}
"#;

const IMAGE_INPUT_FLOW: &str = r#"use mod.flow.*
let picture = Input{type: @image default: nil}
let result = Output{type: @image value: picture.image()}
Flow{picture, result}
"#;

const CHAT_FLOW_ONE: &str = r#"use mod.flow.*
let prompt = Input{default: "one"}
let expand = Llm{prompt: prompt.text()}
let result = Output{value: expand.text()}
Flow{concurrency: 1 prompt, expand, result}
"#;

const CHAT_FLOW_TWO: &str = r#"use mod.flow.*
let prompt = Input{default: "two"}
let expand = Llm{prompt: prompt.text()}
let result = Output{value: expand.text()}
Flow{concurrency: 2 prompt, expand, result}
"#;

// -------------------------------------------------------------------------
// One test per T3 bullet.
// -------------------------------------------------------------------------

#[test]
fn contact_sheet_end_to_end_through_http_only() {
    let _socket_test = socket_test();
    let Some(gen_url) = start_testpattern("contact-sheet") else {
        return;
    };
    let fixture = FixtureServer::start();
    let root = TempRoot::new("contact-sheet");
    let mut config = base_config(
        &root.0,
        Seams {
            chat: Arc::new(FakeChat::done("unused")),
            gen: Arc::new(FixedGen(gen_url)),
            http: Arc::new(HubHttp),
        },
    );
    config.net = NetPolicy {
        allow: vec!["127.0.0.1".to_string()],
        deny_private: false,
    };
    let server = FlowServer::start(config).unwrap();
    let endpoints = server.endpoints();
    let source = include_str!("fixtures/battery/contact_sheet.splash")
        .replace("{{BASE_URL}}", &fixture.base_url());
    put_flow(&endpoints, "contact-sheet", &source);
    let instance = create_instance(&endpoints, "contact-sheet", false);
    let run_cursor = event_cursor(&endpoints, "run");
    let run = start_run(&endpoints, &instance, None);
    assert_eq!(run.queued, 0);

    let mut saw_first = false;
    let mut saw_second = false;
    let mut saw_waiting = false;
    let before_answer = poll_events_until(
        &endpoints,
        "run",
        run_cursor,
        Duration::from_secs(15),
        |event| {
            if event_str(event, "run_id") == Some(run.run_id.as_str()) {
                saw_first |= event_kind(event) == "node.started"
                    && event_str(event, "node") == Some("first");
                saw_second |= event_kind(event) == "node.started"
                    && event_str(event, "node") == Some("second");
                saw_waiting |= event_kind(event) == "node.waiting"
                    && event_str(event, "node") == Some("choice");
            }
            saw_first && saw_second && saw_waiting
        },
    );
    let first_start = before_answer
        .iter()
        .position(|event| event_kind(event) == "node.started" && event_str(event, "node") == Some("first"))
        .expect("first candidate started");
    let second_start = before_answer
        .iter()
        .position(|event| event_kind(event) == "node.started" && event_str(event, "node") == Some("second"))
        .expect("second candidate started");
    let first_candidate_result = before_answer.iter().position(|event| {
        matches!(event_kind(event), "node.progress" | "node.done")
            && matches!(event_str(event, "node"), Some("first" | "second"))
    });
    assert!(
        first_candidate_result.is_none_or(|index| first_start < index && second_start < index),
        "candidate images were not dispatched together: {before_answer:#?}"
    );
    let waiting = before_answer
        .iter()
        .find(|event| {
            event_kind(event) == "node.waiting" && event_str(event, "node") == Some("choice")
        })
        .expect("choice node.waiting");
    let options = match waiting.key("options") {
        Some(JsonValue::Array(options)) => options,
        other => panic!("waiting options are not an array: {other:?}"),
    };
    assert_eq!(options.len(), 2, "{waiting:#?}");

    let waiting_rows = json_request(
        endpoints.control,
        "GET",
        "/v1/instances?waiting=1",
        Some(&endpoints.token),
        "",
    );
    assert_eq!(waiting_rows.status, 200, "{}", waiting_rows.body());
    let waiting_rows = Vec::<InstanceRow>::deserialize_json(&waiting_rows.body()).unwrap();
    let waiting_row = waiting_rows
        .iter()
        .find(|row| row.instance == instance)
        .expect("parked contact-sheet instance");
    assert_eq!(
        waiting_row.waiting.as_ref().unwrap().options,
        vec![Literal::Str("first".to_string()), Literal::Str("second".to_string())]
    );

    let resume_cursor = event_cursor(&endpoints, "run");
    let answer = put_inputs(
        &endpoints,
        &instance,
        Some("chat"),
        &one_input("choice", "text", text_input("first")),
    );
    assert_eq!(answer.status, 200, "{}", answer.body());
    let after_answer = poll_events_until(
        &endpoints,
        "run",
        resume_cursor,
        Duration::from_secs(15),
        |event| {
            event_kind(event) == "run.finished"
                && event_str(event, "run_id") == Some(run.run_id.as_str())
        },
    );
    let answered = after_answer
        .iter()
        .find(|event| event_kind(event) == "node.answered")
        .expect("node.answered");
    assert_eq!(event_str(answered, "node"), Some("choice"));
    assert_eq!(event_str(answered, "by"), Some("chat"));
    let image_done = after_answer
        .iter()
        .find(|event| event_kind(event) == "node.done" && event_str(event, "node") == Some("image"))
        .expect("final image node.done");
    let digest = image_done
        .key("outputs")
        .and_then(|value| match value {
            JsonValue::Array(values) => Some(values.as_slice()),
            _ => None,
        })
        .and_then(|outputs| outputs.first())
        .and_then(|output| output.key("value"))
        .and_then(|value| value.key("digest"))
        .and_then(JsonValue::string)
        .cloned()
        .expect("final image digest");
    assert_eq!(
        variant_name(after_answer.last().and_then(|event| event.key("state"))),
        Some("Done")
    );

    let stored = fixture.posted().expect("fixture received POST /pictures");
    assert_eq!(stored.content_type.as_deref(), Some("image/png"));
    assert_eq!(&stored.body[..8], b"\x89PNG\r\n\x1a\n");
    let fetched = request(
        endpoints.data,
        "GET",
        &format!("/v1/values/{digest}"),
        Some(&endpoints.token),
        &[],
        &[],
    );
    assert_eq!(fetched.status, 200, "{}", fetched.body());
    assert_eq!(stored.body, fetched.bytes);
    let row = get_run(&endpoints, &run.run_id);
    assert_eq!(row.state, RunState::Done);
    assert_eq!(row.http_log.len(), 2, "{row:#?}");
    assert_eq!(row.http_log[0].method, "GET");
    assert_eq!(row.http_log[1].method, "POST");
    server.shutdown();
}

#[test]
fn ask_edge_cases_reject_bad_answers_cancel_and_timeout() {
    let _socket_test = socket_test();
    let root = TempRoot::new("ask-edges");
    let server = start(
        &root.0,
        seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")),
    );
    let endpoints = server.endpoints();
    put_flow(&endpoints, "ask", ASK_FLOW);

    let instance = create_instance(&endpoints, "ask", false);
    let cursor = event_cursor(&endpoints, "run");
    let run = start_run(&endpoints, &instance, Some(vec!["result".to_string()]));
    poll_events_until(
        &endpoints,
        "run",
        cursor,
        Duration::from_secs(5),
        |event| event_kind(event) == "node.waiting" && event_str(event, "node") == Some("first"),
    );

    let wrong_node = put_inputs(
        &endpoints,
        &instance,
        None,
        &one_input("other", "text", text_input("first")),
    );
    assert_eq!(wrong_node.status, 409, "{}", wrong_node.body());
    let wrong_type = put_inputs(
        &endpoints,
        &instance,
        None,
        &one_input("first", "text", json_input(JsonValue::String("first".to_string()))),
    );
    assert_eq!(wrong_type.status, 422, "{}", wrong_type.body());

    let answer_cursor = event_cursor(&endpoints, "run");
    let answer = put_inputs(
        &endpoints,
        &instance,
        Some("chat"),
        &one_input("first", "text", text_input("first")),
    );
    assert_eq!(answer.status, 200, "{}", answer.body());
    poll_events_until(
        &endpoints,
        "run",
        answer_cursor,
        Duration::from_secs(5),
        |event| event_kind(event) == "run.finished" && event_str(event, "run_id") == Some(&run.run_id),
    );
    let twice = put_inputs(
        &endpoints,
        &instance,
        None,
        &one_input("first", "text", text_input("second")),
    );
    assert_eq!(twice.status, 409, "{}", twice.body());

    let delete_instance = create_instance(&endpoints, "ask", false);
    let delete_cursor = event_cursor(&endpoints, "run");
    let delete_run = start_run(
        &endpoints,
        &delete_instance,
        Some(vec!["result".to_string()]),
    );
    poll_events_until(
        &endpoints,
        "run",
        delete_cursor.clone(),
        Duration::from_secs(5),
        |event| {
            event_kind(event) == "node.waiting"
                && event_str(event, "run_id") == Some(&delete_run.run_id)
        },
    );
    let after_wait_cursor = event_cursor(&endpoints, "run");
    let deleted = json_request(
        endpoints.control,
        "DELETE",
        &format!("/v1/instances/{delete_instance}"),
        Some(&endpoints.token),
        "",
    );
    assert_eq!(deleted.status, 204, "{}", deleted.body());
    let cancelled = poll_events_until(
        &endpoints,
        "run",
        after_wait_cursor,
        Duration::from_secs(5),
        |event| {
            event_kind(event) == "run.finished"
                && event_str(event, "run_id") == Some(&delete_run.run_id)
        },
    );
    assert_eq!(
        variant_name(cancelled.last().and_then(|event| event.key("state"))),
        Some("Cancelled")
    );
    let gone = json_request(
        endpoints.control,
        "GET",
        &format!("/v1/instances/{delete_instance}"),
        Some(&endpoints.token),
        "",
    );
    assert_eq!(gone.status, 404);

    let timeout_source = r#"use mod.flow.*
let ask = Ask{question: "Wait?" timeout: 1}
let result = Output{value: ask.text()}
Flow{ask, result}
"#;
    put_flow(&endpoints, "timeout", timeout_source);
    let timeout_instance = create_instance(&endpoints, "timeout", false);
    let timeout_cursor = event_cursor(&endpoints, "run");
    let started = Instant::now();
    let timeout_run = start_run(&endpoints, &timeout_instance, None);
    let timeout_events = poll_events_until(
        &endpoints,
        "run",
        timeout_cursor,
        Duration::from_secs(3),
        |event| {
            event_kind(event) == "run.finished"
                && event_str(event, "run_id") == Some(&timeout_run.run_id)
        },
    );
    assert!(started.elapsed() < Duration::from_secs(2), "Ask timeout took {:?}", started.elapsed());
    assert!(timeout_events.iter().any(|event| {
        event_kind(event) == "node.failed"
            && event_str(event, "node") == Some("ask")
            && event_str(event, "error").is_some_and(|error| error.contains("timeout"))
    }));
    server.shutdown();
}

#[test]
fn inputs_validate_nodes_digests_uploads_dedup_and_body_limit() {
    let _socket_test = socket_test();
    let root = TempRoot::new("inputs");
    let server = start(
        &root.0,
        seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")),
    );
    let endpoints = server.endpoints();
    put_flow(&endpoints, "image-input", IMAGE_INPUT_FLOW);
    let instance = create_instance(&endpoints, "image-input", false);

    let absent = put_inputs(
        &endpoints,
        &instance,
        None,
        &one_input("not_in_graph", "image", image_input(&"0".repeat(64))),
    );
    assert_eq!(absent.status, 422, "{}", absent.body());
    assert!(absent.body().contains("not_in_graph"), "{}", absent.body());

    let missing = put_inputs(
        &endpoints,
        &instance,
        None,
        &one_input("picture", "image", image_input(&"0".repeat(64))),
    );
    assert_eq!(missing.status, 422, "{}", missing.body());

    const PNG: &[u8] = &[
        0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d, b'I', b'H', b'D',
        b'R', 0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0, 0x1f, 0x15, 0xc4, 0x89,
        0, 0, 0, 0x0d, b'I', b'D', b'A', b'T', 8, 0xd7, 0x63, 0xf8, 0xcf, 0xc0, 0xf0,
        0x1f, 0, 5, 0, 1, 0xff, 0x89, 0x99, 0x3d, 0x1d, 0, 0, 0, 0, b'I', b'E', b'N',
        b'D', 0xae, 0x42, 0x60, 0x82,
    ];
    let (first_upload, first_digest) = put_value(&endpoints, "image", "image/png", PNG);
    assert_eq!(first_upload.status, 201, "{}", first_upload.body());
    let (second_upload, second_digest) = put_value(&endpoints, "image", "image/png", PNG);
    assert_eq!(second_upload.status, 201, "{}", second_upload.body());
    assert_eq!(first_digest, second_digest);

    let set = put_inputs(
        &endpoints,
        &instance,
        None,
        &one_input("picture", "image", image_input(&first_digest)),
    );
    assert_eq!(set.status, 200, "{}", set.body());
    let run = start_run(&endpoints, &instance, None);
    let row = wait_run_terminal(&endpoints, &run.run_id, Duration::from_secs(5));
    assert_eq!(row.state, RunState::Done);
    assert_eq!(row.outputs["result"].digest, first_digest);

    let oversized = vec![0x5a; 64 * 1024 * 1024 + 1];
    let started = Instant::now();
    let too_large = request(
        endpoints.data,
        "PUT",
        "/v1/values?type=image&content_type=image/png",
        Some(&endpoints.token),
        &[("Content-Type", "image/png")],
        &oversized,
    );
    assert!(matches!(too_large.status, 400 | 413), "{too_large:?}");
    assert!(started.elapsed() < Duration::from_secs(10), "oversized upload hung: {:?}", started.elapsed());
    server.shutdown();
}

#[test]
fn concurrency_queues_parallelizes_and_finishes_twenty_seeded_instances() {
    let _socket_test = socket_test();
    let root = TempRoot::new("concurrency");
    let server = start(
        &root.0,
        seams(FakeChat::done("done"), FakeGen::done(), FakeHttp::json(200, "{}")),
    );
    let endpoints = server.endpoints();

    put_flow(&endpoints, "serial", CHAT_FLOW_ONE);
    let serial_instance = create_instance(&endpoints, "serial", false);
    let serial_cursor = event_cursor(&endpoints, "run");
    let serial_first = start_run(&endpoints, &serial_instance, None);
    let serial_second = start_run(&endpoints, &serial_instance, None);
    assert_eq!(serial_first.queued, 0);
    assert_eq!(serial_second.queued, 1);
    let serial_events = poll_events_until(
        &endpoints,
        "run",
        serial_cursor,
        Duration::from_secs(10),
        |event| {
            event_kind(event) == "run.finished"
                && event_str(event, "run_id") == Some(&serial_second.run_id)
        },
    );
    let position = |kind: &str, run_id: &str| {
        serial_events
            .iter()
            .position(|event| event_kind(event) == kind && event_str(event, "run_id") == Some(run_id))
            .unwrap_or_else(|| panic!("missing {kind} for {run_id}: {serial_events:#?}"))
    };
    assert!(
        position("run.started", &serial_first.run_id)
            < position("run.finished", &serial_first.run_id)
    );
    assert!(
        position("run.finished", &serial_first.run_id)
            < position("run.started", &serial_second.run_id),
        "queued run started before the first finished: {serial_events:#?}"
    );

    put_flow(&endpoints, "parallel", CHAT_FLOW_TWO);
    let parallel_instance = create_instance(&endpoints, "parallel", false);
    let parallel_cursor = event_cursor(&endpoints, "run");
    let parallel_first = start_run(&endpoints, &parallel_instance, None);
    let parallel_second = start_run(&endpoints, &parallel_instance, None);
    assert_eq!(parallel_first.queued, 0);
    assert_eq!(parallel_second.queued, 0);
    let parallel_events = poll_events_until(
        &endpoints,
        "run",
        parallel_cursor,
        Duration::from_secs(10),
        |event| {
            event_kind(event) == "run.finished"
                && event_str(event, "run_id") == Some(&parallel_second.run_id)
        },
    );
    let first_finished = parallel_events
        .iter()
        .position(|event| event_kind(event) == "run.finished")
        .expect("a parallel run finished");
    for run_id in [&parallel_first.run_id, &parallel_second.run_id] {
        let started = parallel_events
            .iter()
            .position(|event| event_kind(event) == "run.started" && event_str(event, "run_id") == Some(run_id))
            .unwrap_or_else(|| panic!("parallel run {run_id} never started: {parallel_events:#?}"));
        assert!(started < first_finished, "runs did not overlap: {parallel_events:#?}");
    }
    server.shutdown();

    let Some(gen_url) = start_testpattern("twenty-instances") else {
        return;
    };
    let root = TempRoot::new("twenty-instances");
    let server = start(
        &root.0,
        Seams {
            chat: Arc::new(FakeChat::done("unused")),
            gen: Arc::new(FixedGen(gen_url)),
            http: Arc::new(FakeHttp::json(200, "{}")),
        },
    );
    let endpoints = server.endpoints();
    let mut source = String::from("use mod.flow.*\n");
    for seed in 0..20 {
        source.push_str(&format!(
            "let image{seed} = Image{{prompt: \"seeded\" model: \"testpattern\" width: 32 height: 32 steps: 1 seed: {seed}}}\n"
        ));
        source.push_str(&format!(
            "let result{seed} = Output{{type: @image value: image{seed}.image()}}\n"
        ));
    }
    source.push_str("Flow{concurrency: 2 ");
    for seed in 0..20 {
        source.push_str(&format!("image{seed}, result{seed}, "));
    }
    source.push_str("}\n");
    put_flow(&endpoints, "seeded", &source);
    let cursor = event_cursor(&endpoints, "run");
    let mut runs = Vec::new();
    for seed in 0..20 {
        let instance = create_instance(&endpoints, "seeded", false);
        runs.push(start_run(
            &endpoints,
            &instance,
            Some(vec![format!("result{seed}")]),
        ));
    }
    let expected: HashSet<String> = runs.iter().map(|run| run.run_id.clone()).collect();
    let mut finished = HashSet::new();
    poll_events_until(
        &endpoints,
        "run",
        cursor,
        Duration::from_secs(30),
        |event| {
            if event_kind(event) == "run.finished" {
                if let Some(run_id) = event_str(event, "run_id") {
                    if expected.contains(run_id) {
                        finished.insert(run_id.to_string());
                    }
                }
            }
            finished.len() == expected.len()
        },
    );
    let digests: HashSet<String> = runs
        .iter()
        .map(|run| {
            let row = get_run(&endpoints, &run.run_id);
            assert_eq!(row.state, RunState::Done, "{row:#?}");
            row.outputs
                .values()
                .next()
                .expect("seeded output")
                .digest
                .clone()
        })
        .collect();
    assert_eq!(digests.len(), 20, "different seeds did not produce 20 digests");
    server.shutdown();
}

#[test]
fn live_definition_changes_preserve_revision_pins_inputs_and_cancel_on_delete() {
    let _socket_test = socket_test();
    let root = TempRoot::new("live-definitions");
    let server = start(
        &root.0,
        seams(
            FakeChat {
                pending: true,
                ..FakeChat::done("unused")
            },
            FakeGen::done(),
            FakeHttp::json(200, "{}"),
        ),
    );
    let endpoints = server.endpoints();

    let idle_v1 = r#"use mod.flow.*
let prompt = Input{default: "old"}
let result = Output{value: prompt.text()}
Flow{prompt, result}
"#;
    let idle_v2 = r#"use mod.flow.*
let prompt = Input{default: "new default"}
let extra = Input{default: "extra"}
let result = Output{value: prompt.text()}
Flow{prompt, extra, result}
"#;
    put_flow(&endpoints, "idle", idle_v1);
    let live = create_instance(&endpoints, "idle", false);
    let pinned = create_instance(&endpoints, "idle", true);
    for instance in [&live, &pinned] {
        let response = put_inputs(
            &endpoints,
            instance,
            None,
            &one_input("prompt", "text", text_input("kept")),
        );
        assert_eq!(response.status, 200, "{}", response.body());
    }
    put_flow(&endpoints, "idle", idle_v2);
    let live_row = json_request(
        endpoints.control,
        "GET",
        &format!("/v1/instances/{live}"),
        Some(&endpoints.token),
        "",
    );
    let live_row = InstanceRow::deserialize_json(&live_row.body()).unwrap();
    assert_eq!(live_row.revision, 2);
    assert_eq!(live_row.input_text("prompt", "text"), Some("kept".to_string()));
    assert!(live_row.inputs.contains_key("extra"));
    let pinned_row = json_request(
        endpoints.control,
        "GET",
        &format!("/v1/instances/{pinned}"),
        Some(&endpoints.token),
        "",
    );
    let pinned_row = InstanceRow::deserialize_json(&pinned_row.body()).unwrap();
    assert_eq!(pinned_row.revision, 1);
    assert_eq!(pinned_row.input_text("prompt", "text"), Some("kept".to_string()));
    assert!(!pinned_row.inputs.contains_key("extra"));

    let running_v1 = r#"use mod.flow.*
let choice = Ask{question: "Old question"}
let result = Output{value: choice.text()}
Flow{choice, result}
"#;
    let running_v2 = r#"use mod.flow.*
let choice = Ask{question: "New question"}
let extra = Input{default: "new"}
let result = Output{value: choice.text()}
Flow{choice, extra, result}
"#;
    put_flow(&endpoints, "running", running_v1);
    let running = create_instance(&endpoints, "running", false);
    let old_cursor = event_cursor(&endpoints, "run");
    let old_run = start_run(&endpoints, &running, None);
    let old_events = poll_events_until(
        &endpoints,
        "run",
        old_cursor,
        Duration::from_secs(5),
        |event| {
            event_kind(event) == "node.waiting"
                && event_str(event, "run_id") == Some(&old_run.run_id)
        },
    );
    let started = old_events
        .iter()
        .find(|event| event_kind(event) == "run.started")
        .expect("old run.started");
    assert_eq!(event_u64(started, "revision"), Some(1));
    put_flow(&endpoints, "running", running_v2);
    let finish_cursor = event_cursor(&endpoints, "run");
    let answer = put_inputs(
        &endpoints,
        &running,
        Some("chat"),
        &one_input("choice", "text", text_input("first")),
    );
    assert_eq!(answer.status, 200, "{}", answer.body());
    poll_events_until(
        &endpoints,
        "run",
        finish_cursor,
        Duration::from_secs(5),
        |event| {
            event_kind(event) == "run.finished"
                && event_str(event, "run_id") == Some(&old_run.run_id)
        },
    );
    let old_row = get_run(&endpoints, &old_run.run_id);
    assert_eq!(old_row.revision, 1);
    assert_eq!(old_row.state, RunState::Done);

    let new_cursor = event_cursor(&endpoints, "run");
    let new_run = start_run(&endpoints, &running, None);
    let new_events = poll_events_until(
        &endpoints,
        "run",
        new_cursor,
        Duration::from_secs(5),
        |event| {
            event_kind(event) == "node.waiting"
                && event_str(event, "run_id") == Some(&new_run.run_id)
        },
    );
    let new_started = new_events
        .iter()
        .find(|event| event_kind(event) == "run.started")
        .expect("new run.started");
    assert_eq!(event_u64(new_started, "revision"), Some(2));

    let delete_cursor = event_cursor(&endpoints, "run");
    let deleted = json_request(
        endpoints.control,
        "DELETE",
        "/v1/flows/running",
        Some(&endpoints.token),
        "",
    );
    assert_eq!(deleted.status, 204, "{}", deleted.body());
    let cancelled = poll_events_until(
        &endpoints,
        "run",
        delete_cursor,
        Duration::from_secs(5),
        |event| {
            event_kind(event) == "run.finished"
                && event_str(event, "run_id") == Some(&new_run.run_id)
        },
    );
    assert_eq!(
        variant_name(cancelled.last().and_then(|event| event.key("state"))),
        Some("Cancelled")
    );
    let gone = json_request(
        endpoints.control,
        "GET",
        &format!("/v1/instances/{running}"),
        Some(&endpoints.token),
        "",
    );
    assert_eq!(gone.status, 404);
    server.shutdown();
}

#[test]
fn values_support_range_etag_missing_and_ttl_reference_liveness() {
    let _socket_test = socket_test();
    let root = TempRoot::new("values");
    let mut config = base_config(
        &root.0,
        seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")),
    );
    config.values_ram_budget = 1;
    config.value_ttl_secs = 1;
    let server = FlowServer::start(config).unwrap();
    let endpoints = server.endpoints();
    let bytes = b"0123456789abcdefghijklmnopqrstuvwxyz";
    let (uploaded, digest) = put_value(&endpoints, "bytes", "application/octet-stream", bytes);
    assert_eq!(uploaded.status, 201, "{}", uploaded.body());

    let full = request(
        endpoints.data,
        "GET",
        &format!("/v1/values/{digest}"),
        Some(&endpoints.token),
        &[],
        &[],
    );
    assert_eq!(full.status, 200);
    assert_eq!(full.bytes, bytes);
    let etag = full.headers.get("etag").expect("ETag").clone();
    let range = request(
        endpoints.data,
        "GET",
        &format!("/v1/values/{digest}"),
        Some(&endpoints.token),
        &[("Range", "bytes=0-9")],
        &[],
    );
    assert_eq!(range.status, 206, "{range:?}");
    assert_eq!(range.bytes, &bytes[..10]);
    let not_modified = request(
        endpoints.data,
        "GET",
        &format!("/v1/values/{digest}"),
        Some(&endpoints.token),
        &[("If-None-Match", &etag)],
        &[],
    );
    assert_eq!(not_modified.status, 304, "{not_modified:?}");
    assert!(not_modified.bytes.is_empty());
    let unknown = request(
        endpoints.data,
        "GET",
        &format!("/v1/values/{}", "f".repeat(64)),
        Some(&endpoints.token),
        &[],
        &[],
    );
    assert_eq!(unknown.status, 404, "{unknown:?}");

    let (unused_upload, unused_digest) = put_value(
        &endpoints,
        "image",
        "image/png",
        b"unreferenced-image-bytes",
    );
    assert_eq!(unused_upload.status, 201);
    let (live_upload, live_digest) = put_value(
        &endpoints,
        "image",
        "image/png",
        b"parked-run-image-bytes",
    );
    assert_eq!(live_upload.status, 201);
    let parked_source = r#"use mod.flow.*
let picture = Input{type: @image default: nil}
let ask = Ask{question: "Keep waiting"}
let result = Output{type: @image value: picture.image()}
let answer = Output{value: ask.text()}
Flow{picture, ask, result, answer}
"#;
    put_flow(&endpoints, "parked-value", parked_source);
    let instance = create_instance(&endpoints, "parked-value", false);
    let set = put_inputs(
        &endpoints,
        &instance,
        None,
        &one_input("picture", "image", image_input(&live_digest)),
    );
    assert_eq!(set.status, 200, "{}", set.body());
    let cursor = event_cursor(&endpoints, "run");
    let run = start_run(&endpoints, &instance, None);
    poll_events_until(
        &endpoints,
        "run",
        cursor,
        Duration::from_secs(5),
        |event| {
            event_kind(event) == "node.waiting"
                && event_str(event, "run_id") == Some(&run.run_id)
        },
    );
    std::thread::sleep(Duration::from_millis(2_500));
    let expired = request(
        endpoints.data,
        "GET",
        &format!("/v1/values/{unused_digest}"),
        Some(&endpoints.token),
        &[],
        &[],
    );
    assert_eq!(expired.status, 404, "{expired:?}");
    let retained = request(
        endpoints.data,
        "GET",
        &format!("/v1/values/{live_digest}"),
        Some(&endpoints.token),
        &[],
        &[],
    );
    assert_eq!(retained.status, 200, "{retained:?}");
    server.shutdown();
}

#[test]
fn events_filter_delta_page_in_order_and_signal_a_4096_event_gap() {
    let _socket_test = socket_test();
    let root = TempRoot::new("events");
    let server = start(
        &root.0,
        seams(
            FakeChat::done("streamed text"),
            FakeGen::done(),
            FakeHttp::json(200, "{}"),
        ),
    );
    let endpoints = server.endpoints();
    put_flow(&endpoints, "stream", CHAT_FLOW_ONE);
    let instance = create_instance(&endpoints, "stream", false);
    let run_cursor = event_cursor(&endpoints, "run");
    let instance_cursor = event_cursor(&endpoints, "instance");
    let run = start_run(&endpoints, &instance, None);
    let run_events = poll_events_until(
        &endpoints,
        "run",
        run_cursor,
        Duration::from_secs(5),
        |event| {
            event_kind(event) == "run.finished"
                && event_str(event, "run_id") == Some(&run.run_id)
        },
    );
    assert!(run_events.iter().any(|event| event_kind(event) == "node.delta"));
    let instance_events = poll_events_until(
        &endpoints,
        "instance",
        instance_cursor,
        Duration::from_secs(5),
        |event| {
            event_kind(event) == "run.finished"
                && event_str(event, "run_id") == Some(&run.run_id)
        },
    );
    assert!(instance_events.iter().any(|event| event_kind(event) == "node.done"));
    assert!(!instance_events.iter().any(|event| event_kind(event) == "node.delta"));

    let paging_cursor = event_cursor(&endpoints, "flows");
    put_flow(&endpoints, "page-a", "use mod.flow.*\nFlow{label: \"a\"}\n");
    put_flow(&endpoints, "page-b", "use mod.flow.*\nFlow{label: \"b\"}\n");
    let first = json_request(
        endpoints.control,
        "GET",
        &format!("/v1/events?cursor={paging_cursor}&topic=flows&limit=1"),
        Some(&endpoints.token),
        "",
    );
    assert_eq!(first.status, 200, "{}", first.body());
    let first = EventsResponse::deserialize_json(&first.body()).unwrap();
    assert_eq!(first.events.len(), 1);
    let second = json_request(
        endpoints.control,
        "GET",
        &format!("/v1/events?cursor={}&topic=flows&limit=1", first.cursor),
        Some(&endpoints.token),
        "",
    );
    assert_eq!(second.status, 200, "{}", second.body());
    let second = EventsResponse::deserialize_json(&second.body()).unwrap();
    assert_eq!(second.events.len(), 1);
    assert!(event_seq(&first.events[0]) < event_seq(&second.events[0]));

    let mut burst = String::from("use mod.flow.*\n");
    for index in 0..256 {
        burst.push_str(&format!("let n{index} = Input{{}}\n"));
        burst.push_str(&format!(
            "let o{index} = Output{{value: n{index}.text()}}\n"
        ));
    }
    burst.push_str("Flow{concurrency: 32 ");
    for index in 0..256 {
        burst.push_str(&format!("n{index}, o{index}, "));
    }
    burst.push_str("}\n");
    put_flow(&endpoints, "burst", &burst);
    let burst_definition = json_request(
        endpoints.control,
        "GET",
        "/v1/flows/burst",
        Some(&endpoints.token),
        "",
    );
    let burst_definition = FlowResponse::deserialize_json(&burst_definition.body()).unwrap();
    assert_eq!(burst_definition.graph.unwrap().nodes.len(), 512);
    let burst_instance = create_instance(&endpoints, "burst", false);
    let stale_cursor = event_cursor(&endpoints, "run");
    // Each run contributes 516 global journal entries: run start/finish and
    // 256 node.done events are each projected to both `run` and `instance`.
    // Nine runs therefore put this subscriber 4,644 entries behind.
    for _ in 0..9 {
        let started = start_run(&endpoints, &burst_instance, None);
        assert_eq!(started.queued, 0);
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let response = json_request(
            endpoints.control,
            "GET",
            &format!("/v1/events?cursor={stale_cursor}&topic=run"),
            Some(&endpoints.token),
            "",
        );
        assert_eq!(response.status, 200, "{}", response.body());
        let page = EventsResponse::deserialize_json(&response.body()).unwrap();
        if page.gap {
            break;
        }
        assert!(Instant::now() < deadline, "subscriber never received gap=true");
        std::thread::sleep(Duration::from_millis(25));
    }
    server.shutdown();
}

#[test]
fn autostart_creates_one_auto_instance_and_janitor_keeps_it() {
    let _socket_test = socket_test();
    let root = TempRoot::new("autostart");
    fs::create_dir_all(root.0.join("flows")).unwrap();
    fs::write(
        root.0.join("flows/auto.splash"),
        "use mod.flow.*\nlet prompt = Input{default: \"auto\"}\nFlow{autostart: true prompt}\n",
    )
    .unwrap();
    let mut config = base_config(
        &root.0,
        seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")),
    );
    config.instance_ttl_secs = 1;
    let server = FlowServer::start(config).unwrap();
    let endpoints = server.endpoints();
    let list = || {
        let response = json_request(
            endpoints.control,
            "GET",
            "/v1/instances?flow=auto",
            Some(&endpoints.token),
            "",
        );
        assert_eq!(response.status, 200, "{}", response.body());
        Vec::<InstanceRow>::deserialize_json(&response.body()).unwrap()
    };
    let rows = list();
    assert_eq!(rows.len(), 1, "{rows:#?}");
    assert_eq!(rows[0].owner, "auto");
    let id = rows[0].instance.clone();
    std::thread::sleep(Duration::from_millis(2_500));
    let rows = list();
    assert_eq!(rows.len(), 1, "autostart instance expired: {rows:#?}");
    assert_eq!(rows[0].instance, id);
    assert_eq!(rows[0].owner, "auto");
    server.shutdown();
}

#[test]
fn shutdown_cancels_inflight_within_five_seconds_and_runs_do_not_persist() {
    let _socket_test = socket_test();
    let root = TempRoot::new("shutdown");
    let server = start(
        &root.0,
        seams(
            FakeChat {
                pending: true,
                ..FakeChat::done("unused")
            },
            FakeGen::done(),
            FakeHttp::json(200, "{}"),
        ),
    );
    let endpoints = server.endpoints();
    put_flow(&endpoints, "stalled", CHAT_FLOW_ONE);
    let instance = create_instance(&endpoints, "stalled", false);
    let cursor = event_cursor(&endpoints, "run");
    let run = start_run(&endpoints, &instance, None);
    poll_events_until(
        &endpoints,
        "run",
        cursor,
        Duration::from_secs(5),
        |event| {
            event_kind(event) == "node.started"
                && event_str(event, "run_id") == Some(&run.run_id)
        },
    );
    let started = Instant::now();
    server.shutdown();
    assert!(started.elapsed() < Duration::from_secs(5), "shutdown took {:?}", started.elapsed());

    let restarted = start(
        &root.0,
        seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")),
    );
    let endpoints = restarted.endpoints();
    let rows = json_request(
        endpoints.control,
        "GET",
        "/v1/runs",
        Some(&endpoints.token),
        "",
    );
    assert_eq!(rows.status, 200, "{}", rows.body());
    let rows = Vec::<RunRowDto>::deserialize_json(&rows.body()).unwrap();
    assert!(rows.is_empty(), "run rows survived restart: {rows:#?}");
    restarted.shutdown();
}
