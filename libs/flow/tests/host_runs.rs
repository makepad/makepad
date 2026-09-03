#![cfg(not(target_arch = "wasm32"))]
//! Socket tests for the F2 lane: instance/run/value routes over real HTTP,
//! against `FlowServer::start` with fake seams injected via
//! `FlowServerConfig::with_seams` (§12 headless acceptance, adapted to the
//! Gates list in `local/agent_state/flow/briefs/F2-routes.md`).

mod support;

use makepad_ai_hub::discovery::DEFAULT_FLEET;
use makepad_ai_hub::download::Downloader;
use makepad_ai_hub::peer_serve::PeerOptions;
use makepad_ai_hub::registry::{Domain, ModelSpec, Registry};
use makepad_ai_hub::server::{start_service, ServiceConfig};
use makepad_ai_hub::sha256::sha256_hex;
use makepad_flow::engine::FixedGen;
use makepad_flow::host::{FlowServer, FlowServerConfig};
use makepad_flow::{
    CreateInstanceRequest, CreateInstanceResponse, CreateRunRequest, CreateRunResponse,
    EventsResponse, InputValueDto, InstanceRow, PortType, PutSourceRequest, PutValueResponse,
    RunRowDto, Seams, SetInputsResponse,
};
use makepad_micro_serde::{DeJson, JsonValue, SerJson};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use support::{seams, FakeChat, FakeGen, FakeHttp};

struct TempRoot(PathBuf);

impl TempRoot {
    fn new(label: &str) -> Self {
        let nonce = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let path = std::env::temp_dir().join(format!(
            "makepad-flow-host-runs-{}-{}-{nonce}",
            std::process::id(),
            label
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct HttpResponse {
    status: u16,
    /// Lossily decoded for the common case (JSON, error messages).
    body: String,
    /// The exact response bytes, for the one route (`GET /v1/values/…`)
    /// whose body is not text.
    bytes: Vec<u8>,
}

fn request(
    address: SocketAddr,
    method: &str,
    target: &str,
    token: Option<&str>,
    body: &str,
) -> HttpResponse {
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(35))).unwrap();
    let authorization = token
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default();
    write!(
        stream,
        "{method} {target} HTTP/1.1\r\nHost: localhost\r\n{authorization}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .unwrap();
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).unwrap();
    let boundary = raw.windows(4).position(|window| window == b"\r\n\r\n").expect("head/body boundary");
    let head = std::str::from_utf8(&raw[..boundary]).expect("response head is not utf-8");
    let bytes = raw[boundary + 4..].to_vec();
    let status = head.lines().next().unwrap().split_whitespace().nth(1).unwrap().parse().unwrap();
    let body = String::from_utf8_lossy(&bytes).into_owned();
    HttpResponse { status, body, bytes }
}

fn start(root: &Path, seams: Seams) -> FlowServer {
    let mut config = FlowServerConfig::new(root.to_path_buf()).with_seams(seams);
    config.watch_interval_ms = 25;
    config.janitor_sweep_secs = 1;
    config.log = Box::new(|_| {});
    FlowServer::start(config).unwrap()
}

fn put_flow(address: SocketAddr, token: &str, name: &str, source: &str) {
    let response = request(
        address,
        "PUT",
        &format!("/v1/flows/{name}"),
        Some(token),
        &PutSourceRequest { source: source.to_string() }.serialize_json(),
    );
    assert_eq!(response.status, 200, "put_flow {name}: {}", response.body);
}

fn cursor(address: SocketAddr, token: &str, topic: &str) -> String {
    let response = request(address, "GET", &format!("/v1/events?topic={topic}"), Some(token), "");
    assert_eq!(response.status, 200, "{}", response.body);
    EventsResponse::deserialize_json(&response.body).unwrap().cursor
}

/// Poll `/v1/events` on `topic` until `stop` returns true on one of the
/// events seen so far, or a 5 s deadline elapses. Returns every event kind
/// string observed, in order.
fn poll_events_until(
    address: SocketAddr,
    token: &str,
    topic: &str,
    mut cursor: String,
    mut stop: impl FnMut(&JsonValue) -> bool,
) -> Vec<JsonValue> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut seen = Vec::new();
    loop {
        let response = request(
            address,
            "GET",
            &format!("/v1/events?cursor={cursor}&wait=500&topic={topic}"),
            Some(token),
            "",
        );
        assert_eq!(response.status, 200, "{}", response.body);
        let page = EventsResponse::deserialize_json(&response.body).unwrap();
        cursor = page.cursor;
        for event in page.events {
            let done = stop(&event);
            seen.push(event);
            if done {
                return seen;
            }
        }
        assert!(Instant::now() < deadline, "event deadline elapsed; saw {seen:#?}");
    }
}

fn event_kind(event: &JsonValue) -> &str {
    event.key("kind").and_then(JsonValue::string).map(String::as_str).unwrap_or("")
}

fn event_str<'a>(event: &'a JsonValue, key: &str) -> Option<&'a str> {
    event.key(key).and_then(JsonValue::string).map(String::as_str)
}

/// `RunState`/`NodeState`/`PortType` are fieldless enums; this crate's
/// `SerJson` derive still wraps them as a single-key object (`{"Done":[]}`)
/// rather than a bare string, so raw-event assertions match on the key.
fn variant_name(value: Option<&JsonValue>) -> Option<&str> {
    match value {
        Some(JsonValue::Object(fields)) if fields.len() == 1 => {
            fields.keys().next().map(String::as_str)
        }
        _ => None,
    }
}

fn text_input(text: &str) -> InputValueDto {
    InputValueDto { ty: PortType::Text, text: Some(text.to_string()), json: None, digest: None }
}

fn image_digest_input(digest: &str) -> InputValueDto {
    InputValueDto {
        ty: PortType::Image,
        text: None,
        json: None,
        digest: Some(digest.to_string()),
    }
}

fn one_input(node: &str, port: &str, value: InputValueDto) -> HashMap<String, HashMap<String, InputValueDto>> {
    let mut ports = HashMap::new();
    ports.insert(port.to_string(), value);
    let mut inputs = HashMap::new();
    inputs.insert(node.to_string(), ports);
    inputs
}

const ECHO_FLOW: &str = r#"use mod.flow.*
let prompt = Input{type: @text default: "x"}
let echo = Fn{in: {text: prompt.text()} out: [@text] run: |i| {{text: i.text}}}
let result = Output{type: @text value: echo.text()}
Flow{prompt, echo, result}
"#;

/// `expand` never finishes while the injected `FakeChat` is `pending`, so a
/// run on this flow stays `running` until cancelled — exactly what the
/// concurrency-queue and cancel tests need to observe deterministically.
const STALLED_FLOW: &str = r#"use mod.flow.*
let prompt = Input{type: @text default: "x"}
let expand = Llm{prompt: prompt.text()}
let result = Output{type: @text value: expand.text()}
Flow{concurrency: 1 prompt, expand, result}
"#;

const ASK_FLOW: &str = r#"use mod.flow.*
let which = Ask{question: "Which?" type: @text options: ["first", "second"]}
let echo = Fn{in: {text: which.text()} out: [@text] run: |i| {{text: i.text}}}
let result = Output{type: @text value: echo.text()}
Flow{which, echo, result}
"#;

const IMAGE_ECHO_FLOW: &str = r#"use mod.flow.*
let picture = Input{type: @image default: nil}
let result = Output{type: @image value: picture.image()}
Flow{picture, result}
"#;

// ---------------------------------------------------------------------------
// 1. §12 headless acceptance: the full instance/run/value pipeline
// ---------------------------------------------------------------------------

#[test]
fn full_run_streams_the_design_pipeline_and_serves_the_picture() {
    let root = TempRoot::new("pipeline");
    let chat = FakeChat::done("a moody paragraph");
    let chat_requests = chat.requests.clone();
    let server = start(
        &root.0,
        seams(chat, FakeGen::done(), FakeHttp::json(200, "{}")),
    );
    let endpoints = server.endpoints();
    put_flow(
        endpoints.control,
        &endpoints.token,
        "demo",
        include_str!("fixtures/prompt_image.splash"),
    );

    let create = request(
        endpoints.control,
        "POST",
        "/v1/flows/demo/instances",
        Some(&endpoints.token),
        &CreateInstanceRequest::default().serialize_json(),
    );
    assert_eq!(create.status, 201, "{}", create.body);
    let instance = CreateInstanceResponse::deserialize_json(&create.body).unwrap().instance;

    let set_inputs = request(
        endpoints.control,
        "PUT",
        &format!("/v1/instances/{instance}/inputs"),
        Some(&endpoints.token),
        &one_input("prompt", "text", text_input("a lighthouse at dusk")).serialize_json(),
    );
    assert_eq!(set_inputs.status, 200, "{}", set_inputs.body);

    let run_cursor = cursor(endpoints.control, &endpoints.token, "run");
    let start_run = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{instance}/runs"),
        Some(&endpoints.token),
        &CreateRunRequest::default().serialize_json(),
    );
    assert_eq!(start_run.status, 202, "{}", start_run.body);
    let run_id = CreateRunResponse::deserialize_json(&start_run.body).unwrap().run_id;
    assert_eq!(CreateRunResponse::deserialize_json(&start_run.body).unwrap().queued, 0);

    let events = poll_events_until(endpoints.control, &endpoints.token, "run", run_cursor, |event| {
        event_kind(event) == "run.finished" && event_str(event, "run_id") == Some(run_id.as_str())
    });
    assert_eq!(
        chat_requests.lock().unwrap().as_slice(),
        &[(
            "Rewrite the prompt as one vivid paragraph for an image model.\n             \
             Keep the subject. Add light, lens, material, mood. No lists."
                .to_string(),
            "a lighthouse at dusk".to_string(),
            String::new(),
            None,
            Some(false),
        )]
    );
    let kinds: Vec<&str> = events.iter().map(event_kind).collect();

    // DESIGN.md §12's exact sequence, with the granularity the brief lists.
    assert_eq!(kinds.first(), Some(&"run.started"));
    assert!(kinds.contains(&"node.done"), "{kinds:#?}");
    assert!(kinds.contains(&"node.started"), "{kinds:#?}");
    assert!(kinds.contains(&"node.delta"), "{kinds:#?}");
    assert_eq!(kinds.last(), Some(&"run.finished"));

    let node_done_events: Vec<&JsonValue> =
        events.iter().filter(|event| event_kind(event) == "node.done").collect();
    assert!(
        node_done_events.iter().any(|event| event_str(event, "node") == Some("prompt")),
        "{kinds:#?}"
    );
    assert!(
        node_done_events.iter().any(|event| event_str(event, "node") == Some("expand")),
        "{kinds:#?}"
    );
    assert!(
        node_done_events.iter().any(|event| event_str(event, "node") == Some("styled")),
        "{kinds:#?}"
    );
    let image_done = node_done_events
        .iter()
        .find(|event| event_str(event, "node") == Some("image"))
        .unwrap_or_else(|| panic!("no node.done(image): {kinds:#?}"));
    let outputs = match image_done.key("outputs") {
        Some(JsonValue::Array(items)) => items,
        other => panic!("expected outputs array, got {other:?}"),
    };
    let picture_port = outputs
        .iter()
        .find(|entry| entry.key("port").and_then(JsonValue::string).map(String::as_str) == Some("image"))
        .expect("an `image` output port");
    let picture_value = picture_port.key("value").expect("output port value");
    assert_eq!(
        picture_value.key("content_type").and_then(JsonValue::string).map(String::as_str),
        Some("image/png")
    );
    let digest =
        picture_value.key("digest").and_then(JsonValue::string).cloned().expect("digest field");

    let finished = events.last().unwrap();
    assert_eq!(variant_name(finished.key("state")), Some("Done"));

    let fetched = request(endpoints.data, "GET", &format!("/v1/values/{digest}"), Some(&endpoints.token), "");
    assert_eq!(fetched.status, 200, "{}", fetched.body);
    assert_eq!(sha256_hex(&fetched.bytes), digest.trim_start_matches("sha256:"));

    let get_run = request(endpoints.control, "GET", &format!("/v1/runs/{run_id}"), Some(&endpoints.token), "");
    assert_eq!(get_run.status, 200, "{}", get_run.body);
    let run_row = RunRowDto::deserialize_json(&get_run.body).unwrap();
    assert!(run_row.outputs.contains_key("picture"), "{run_row:#?}");
    assert_eq!(run_row.outputs["picture"].digest.trim_start_matches("sha256:"), digest);

    // Same instance, same inputs -> the fake gen's fixed bytes dedupe to the
    // same digest, exercising the value-store path a second time.
    let run_cursor2 = cursor(endpoints.control, &endpoints.token, "run");
    let second = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{instance}/runs"),
        Some(&endpoints.token),
        &CreateRunRequest::default().serialize_json(),
    );
    assert_eq!(second.status, 202, "{}", second.body);
    let second_run_id = CreateRunResponse::deserialize_json(&second.body).unwrap().run_id;
    poll_events_until(endpoints.control, &endpoints.token, "run", run_cursor2, |event| {
        event_kind(event) == "run.finished" && event_str(event, "run_id") == Some(second_run_id.as_str())
    });
    let second_row = request(endpoints.control, "GET", &format!("/v1/runs/{second_run_id}"), Some(&endpoints.token), "");
    let second_row = RunRowDto::deserialize_json(&second_row.body).unwrap();
    assert_eq!(second_row.outputs["picture"].digest, digest);

    server.shutdown();
}

// ---------------------------------------------------------------------------
// 2. concurrent instances with different outputs; concurrency queues a run
// ---------------------------------------------------------------------------

#[test]
fn two_instances_finish_with_different_digests() {
    let root = TempRoot::new("concurrent");
    let server = start(&root.0, seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")));
    let endpoints = server.endpoints();
    put_flow(endpoints.control, &endpoints.token, "echo", ECHO_FLOW);

    let mut digests = Vec::new();
    for prompt in ["alpha", "beta"] {
        let create = request(
            endpoints.control,
            "POST",
            "/v1/flows/echo/instances",
            Some(&endpoints.token),
            &CreateInstanceRequest::default().serialize_json(),
        );
        let instance = CreateInstanceResponse::deserialize_json(&create.body).unwrap().instance;
        let set_inputs = request(
            endpoints.control,
            "PUT",
            &format!("/v1/instances/{instance}/inputs"),
            Some(&endpoints.token),
            &one_input("prompt", "text", text_input(prompt)).serialize_json(),
        );
        assert_eq!(set_inputs.status, 200, "{}", set_inputs.body);
        let run_cursor = cursor(endpoints.control, &endpoints.token, "run");
        let start_run = request(
            endpoints.control,
            "POST",
            &format!("/v1/instances/{instance}/runs"),
            Some(&endpoints.token),
            &CreateRunRequest::default().serialize_json(),
        );
        assert_eq!(start_run.status, 202, "{}", start_run.body);
        let run_id = CreateRunResponse::deserialize_json(&start_run.body).unwrap().run_id;
        poll_events_until(endpoints.control, &endpoints.token, "run", run_cursor, |event| {
            event_kind(event) == "run.finished" && event_str(event, "run_id") == Some(run_id.as_str())
        });
        let row = request(endpoints.control, "GET", &format!("/v1/runs/{run_id}"), Some(&endpoints.token), "");
        let row = RunRowDto::deserialize_json(&row.body).unwrap();
        digests.push(row.outputs["result"].digest.clone());
    }
    assert_ne!(digests[0], digests[1]);

    server.shutdown();
}

#[test]
fn concurrency_one_queues_the_second_run() {
    let root = TempRoot::new("queue");
    let server = start(
        &root.0,
        seams(
            FakeChat { pending: true, ..FakeChat::done("unused") },
            FakeGen::done(),
            FakeHttp::json(200, "{}"),
        ),
    );
    let endpoints = server.endpoints();
    put_flow(endpoints.control, &endpoints.token, "stalled", STALLED_FLOW);
    let create = request(
        endpoints.control,
        "POST",
        "/v1/flows/stalled/instances",
        Some(&endpoints.token),
        &CreateInstanceRequest::default().serialize_json(),
    );
    let instance = CreateInstanceResponse::deserialize_json(&create.body).unwrap().instance;

    let first = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{instance}/runs"),
        Some(&endpoints.token),
        &CreateRunRequest::default().serialize_json(),
    );
    assert_eq!(first.status, 202, "{}", first.body);
    assert_eq!(CreateRunResponse::deserialize_json(&first.body).unwrap().queued, 0);

    let second = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{instance}/runs"),
        Some(&endpoints.token),
        &CreateRunRequest::default().serialize_json(),
    );
    assert_eq!(second.status, 202, "{}", second.body);
    assert_eq!(CreateRunResponse::deserialize_json(&second.body).unwrap().queued, 1);

    let get = request(endpoints.control, "GET", &format!("/v1/instances/{instance}"), Some(&endpoints.token), "");
    let row = InstanceRow::deserialize_json(&get.body).unwrap();
    assert_eq!(row.state, "running");

    server.shutdown();
}

// ---------------------------------------------------------------------------
// 3. the Ask flow: parks, is listed waiting, is answered, wrong node is 409
// ---------------------------------------------------------------------------

#[test]
fn ask_flow_parks_lists_waiting_and_is_answered() {
    let root = TempRoot::new("ask");
    let server = start(&root.0, seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")));
    let endpoints = server.endpoints();
    put_flow(endpoints.control, &endpoints.token, "ask", ASK_FLOW);
    let create = request(
        endpoints.control,
        "POST",
        "/v1/flows/ask/instances",
        Some(&endpoints.token),
        &CreateInstanceRequest::default().serialize_json(),
    );
    let instance = CreateInstanceResponse::deserialize_json(&create.body).unwrap().instance;

    // The Ask node exists but nothing is waiting there yet: 409.
    let too_early = request(
        endpoints.control,
        "PUT",
        &format!("/v1/instances/{instance}/inputs"),
        Some(&endpoints.token),
        &one_input("which", "text", text_input("first")).serialize_json(),
    );
    assert_eq!(too_early.status, 409, "{}", too_early.body);

    let instance_cursor = cursor(endpoints.control, &endpoints.token, "instance");
    let start_run = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{instance}/runs"),
        Some(&endpoints.token),
        &CreateRunRequest::default().serialize_json(),
    );
    assert_eq!(start_run.status, 202, "{}", start_run.body);
    let run_id = CreateRunResponse::deserialize_json(&start_run.body).unwrap().run_id;

    let waiting_events = poll_events_until(endpoints.control, &endpoints.token, "instance", instance_cursor, |event| {
        event_kind(event) == "node.waiting"
    });
    assert!(waiting_events.iter().any(|event| event_kind(event) == "node.waiting"), "{waiting_events:#?}");

    let waiting = request(endpoints.control, "GET", "/v1/instances?waiting=1", Some(&endpoints.token), "");
    let rows = Vec::<InstanceRow>::deserialize_json(&waiting.body).unwrap();
    assert!(rows.iter().any(|row| row.instance == instance), "{rows:#?}");
    let row = rows.iter().find(|row| row.instance == instance).unwrap();
    assert_eq!(row.waiting.as_ref().unwrap().node, "which");

    let run_cursor = cursor(endpoints.control, &endpoints.token, "run");
    let answer = request(
        endpoints.control,
        "PUT",
        &format!("/v1/instances/{instance}/inputs?actor=tab"),
        Some(&endpoints.token),
        &one_input("which", "text", text_input("first")).serialize_json(),
    );
    assert_eq!(answer.status, 200, "{}", answer.body);

    let events = poll_events_until(endpoints.control, &endpoints.token, "run", run_cursor, |event| {
        event_kind(event) == "run.finished" && event_str(event, "run_id") == Some(run_id.as_str())
    });
    let answered = events
        .iter()
        .find(|event| event_kind(event) == "node.answered")
        .unwrap_or_else(|| panic!("no node.answered: {events:#?}"));
    assert_eq!(event_str(answered, "by"), Some("tab"));
    assert_eq!(events.last().map(event_kind), Some("run.finished"));

    server.shutdown();
}

// ---------------------------------------------------------------------------
// 4. flow.changed re-evaluates a live instance, a pinned one ignores it;
//    flow.removed drops instances
// ---------------------------------------------------------------------------

#[test]
fn flow_changed_updates_live_instances_and_flow_removed_drops_them() {
    let root = TempRoot::new("changed");
    let server = start(&root.0, seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")));
    let endpoints = server.endpoints();
    put_flow(endpoints.control, &endpoints.token, "echo", ECHO_FLOW);

    let live = request(
        endpoints.control,
        "POST",
        "/v1/flows/echo/instances",
        Some(&endpoints.token),
        &CreateInstanceRequest::default().serialize_json(),
    );
    let live_id = CreateInstanceResponse::deserialize_json(&live.body).unwrap().instance;

    let pinned = request(
        endpoints.control,
        "POST",
        "/v1/flows/echo/instances",
        Some(&endpoints.token),
        &CreateInstanceRequest { pin: Some(true), ..CreateInstanceRequest::default() }.serialize_json(),
    );
    let pinned_id = CreateInstanceResponse::deserialize_json(&pinned.body).unwrap().instance;

    let changed_source = r#"use mod.flow.*
let prompt = Input{type: @text default: "x"}
let extra = Input{type: @text default: "new"}
let echo = Fn{in: {text: prompt.text()} out: [@text] run: |i| {{text: i.text}}}
let result = Output{type: @text value: echo.text()}
Flow{prompt, extra, echo, result}
"#;
    put_flow(endpoints.control, &endpoints.token, "echo", changed_source);

    let live_row = request(endpoints.control, "GET", &format!("/v1/instances/{live_id}"), Some(&endpoints.token), "");
    let live_row = InstanceRow::deserialize_json(&live_row.body).unwrap();
    assert_eq!(live_row.revision, 2);
    assert!(live_row.inputs.contains_key("extra"), "{live_row:#?}");
    assert!(live_row.inputs.contains_key("prompt"), "{live_row:#?}");

    let pinned_row =
        request(endpoints.control, "GET", &format!("/v1/instances/{pinned_id}"), Some(&endpoints.token), "");
    let pinned_row = InstanceRow::deserialize_json(&pinned_row.body).unwrap();
    assert_eq!(pinned_row.revision, 1);
    assert!(!pinned_row.inputs.contains_key("extra"), "{pinned_row:#?}");

    let delete = request(endpoints.control, "DELETE", "/v1/flows/echo", Some(&endpoints.token), "");
    assert_eq!(delete.status, 204, "{}", delete.body);

    let gone_live = request(endpoints.control, "GET", &format!("/v1/instances/{live_id}"), Some(&endpoints.token), "");
    assert_eq!(gone_live.status, 404);
    let gone_pinned =
        request(endpoints.control, "GET", &format!("/v1/instances/{pinned_id}"), Some(&endpoints.token), "");
    assert_eq!(gone_pinned.status, 404);

    server.shutdown();
}

// ---------------------------------------------------------------------------
// 5. DELETE mid-run and POST cancel both finish the run cancelled
// ---------------------------------------------------------------------------

#[test]
fn delete_instance_and_cancel_route_both_cancel_the_run() {
    let root = TempRoot::new("cancel");
    let server = start(
        &root.0,
        seams(
            FakeChat { pending: true, ..FakeChat::done("unused") },
            FakeGen::done(),
            FakeHttp::json(200, "{}"),
        ),
    );
    let endpoints = server.endpoints();
    put_flow(endpoints.control, &endpoints.token, "stalled", STALLED_FLOW);

    // DELETE mid-run.
    let create = request(
        endpoints.control,
        "POST",
        "/v1/flows/stalled/instances",
        Some(&endpoints.token),
        &CreateInstanceRequest::default().serialize_json(),
    );
    let instance = CreateInstanceResponse::deserialize_json(&create.body).unwrap().instance;
    let run_cursor = cursor(endpoints.control, &endpoints.token, "run");
    let start_run = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{instance}/runs"),
        Some(&endpoints.token),
        &CreateRunRequest::default().serialize_json(),
    );
    let run_id = CreateRunResponse::deserialize_json(&start_run.body).unwrap().run_id;
    let delete = request(endpoints.control, "DELETE", &format!("/v1/instances/{instance}"), Some(&endpoints.token), "");
    assert_eq!(delete.status, 204, "{}", delete.body);
    let events = poll_events_until(endpoints.control, &endpoints.token, "run", run_cursor, |event| {
        event_kind(event) == "run.finished" && event_str(event, "run_id") == Some(run_id.as_str())
    });
    let finished = events.last().unwrap();
    assert_eq!(variant_name(finished.key("state")), Some("Cancelled"));

    // POST .../cancel on a second, independent run.
    let create2 = request(
        endpoints.control,
        "POST",
        "/v1/flows/stalled/instances",
        Some(&endpoints.token),
        &CreateInstanceRequest::default().serialize_json(),
    );
    let instance2 = CreateInstanceResponse::deserialize_json(&create2.body).unwrap().instance;
    let run_cursor2 = cursor(endpoints.control, &endpoints.token, "run");
    let start_run2 = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{instance2}/runs"),
        Some(&endpoints.token),
        &CreateRunRequest::default().serialize_json(),
    );
    let run_id2 = CreateRunResponse::deserialize_json(&start_run2.body).unwrap().run_id;
    let cancel = request(endpoints.control, "POST", &format!("/v1/runs/{run_id2}/cancel"), Some(&endpoints.token), "");
    assert_eq!(cancel.status, 200, "{}", cancel.body);
    let events2 = poll_events_until(endpoints.control, &endpoints.token, "run", run_cursor2, |event| {
        event_kind(event) == "run.finished" && event_str(event, "run_id") == Some(run_id2.as_str())
    });
    let finished2 = events2.last().unwrap();
    assert_eq!(variant_name(finished2.key("state")), Some("Cancelled"));

    server.shutdown();
}

#[test]
fn clear_instance_forgets_generated_state_but_keeps_inputs_and_refuses_live_runs() {
    let root = TempRoot::new("clear-instance");
    let server = start(
        &root.0,
        seams(
            FakeChat { pending: true, ..FakeChat::done("unused") },
            FakeGen::done(),
            FakeHttp::json(200, "{}"),
        ),
    );
    let endpoints = server.endpoints();
    put_flow(endpoints.control, &endpoints.token, "echo", ECHO_FLOW);
    put_flow(endpoints.control, &endpoints.token, "stalled-clear", STALLED_FLOW);

    let create = request(
        endpoints.control,
        "POST",
        "/v1/flows/echo/instances",
        Some(&endpoints.token),
        &CreateInstanceRequest {
            inputs: Some(one_input("prompt", "text", text_input("keep me"))),
            ..CreateInstanceRequest::default()
        }
        .serialize_json(),
    );
    let instance = CreateInstanceResponse::deserialize_json(&create.body).unwrap().instance;
    let run_cursor = cursor(endpoints.control, &endpoints.token, "run");
    let started = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{instance}/runs"),
        Some(&endpoints.token),
        &CreateRunRequest::default().serialize_json(),
    );
    let run_id = CreateRunResponse::deserialize_json(&started.body).unwrap().run_id;
    poll_events_until(endpoints.control, &endpoints.token, "run", run_cursor, |event| {
        event_kind(event) == "run.finished" && event_str(event, "run_id") == Some(run_id.as_str())
    });
    let before = request(
        endpoints.control,
        "GET",
        &format!("/v1/instances/{instance}"),
        Some(&endpoints.token),
        "",
    );
    assert!(!InstanceRow::deserialize_json(&before.body).unwrap().outputs.is_empty());

    let instance_cursor = cursor(endpoints.control, &endpoints.token, "instance");
    let clear = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{instance}/clear"),
        Some(&endpoints.token),
        "",
    );
    assert_eq!(clear.status, 200, "{}", clear.body);
    let after = request(
        endpoints.control,
        "GET",
        &format!("/v1/instances/{instance}"),
        Some(&endpoints.token),
        "",
    );
    let after = InstanceRow::deserialize_json(&after.body).unwrap();
    assert_eq!(after.input_text("prompt", "text").as_deref(), Some("keep me"));
    assert!(after.outputs.is_empty());
    assert_eq!(after.state, "idle");
    let forgotten = request(
        endpoints.control,
        "GET",
        &format!("/v1/runs/{run_id}"),
        Some(&endpoints.token),
        "",
    );
    assert_eq!(forgotten.status, 404, "{}", forgotten.body);
    let events = poll_events_until(
        endpoints.control,
        &endpoints.token,
        "instance",
        instance_cursor,
        |event| event_kind(event) == "instance.cleared",
    );
    assert_eq!(event_str(events.last().unwrap(), "instance"), Some(instance.as_str()));

    let create_live = request(
        endpoints.control,
        "POST",
        "/v1/flows/stalled-clear/instances",
        Some(&endpoints.token),
        &CreateInstanceRequest::default().serialize_json(),
    );
    let live = CreateInstanceResponse::deserialize_json(&create_live.body).unwrap().instance;
    let started_live = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{live}/runs"),
        Some(&endpoints.token),
        &CreateRunRequest::default().serialize_json(),
    );
    assert_eq!(started_live.status, 202, "{}", started_live.body);
    let busy = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{live}/clear"),
        Some(&endpoints.token),
        "",
    );
    assert_eq!(busy.status, 409, "{}", busy.body);

    server.shutdown();
}

// ---------------------------------------------------------------------------
// 6. values: PUT bytes -> digest -> usable as an image input; TTL expiry
// ---------------------------------------------------------------------------

#[test]
fn value_upload_is_usable_as_an_image_input() {
    let root = TempRoot::new("value-upload");
    let server = start(&root.0, seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")));
    let endpoints = server.endpoints();
    put_flow(endpoints.control, &endpoints.token, "image-echo", IMAGE_ECHO_FLOW);

    let put_value = request(
        endpoints.data,
        "PUT",
        "/v1/values?type=image&content_type=image/png",
        Some(&endpoints.token),
        "not-really-a-png-but-real-bytes",
    );
    assert_eq!(put_value.status, 201, "{}", put_value.body);
    let digest = PutValueResponse::deserialize_json(&put_value.body).unwrap().digest;

    let create = request(
        endpoints.control,
        "POST",
        "/v1/flows/image-echo/instances",
        Some(&endpoints.token),
        &CreateInstanceRequest::default().serialize_json(),
    );
    assert_eq!(create.status, 201, "{}", create.body);
    let instance = CreateInstanceResponse::deserialize_json(&create.body).unwrap().instance;
    let set_inputs = request(
        endpoints.control,
        "PUT",
        &format!("/v1/instances/{instance}/inputs"),
        Some(&endpoints.token),
        &one_input("picture", "image", image_digest_input(&digest)).serialize_json(),
    );
    assert_eq!(set_inputs.status, 200, "{}", set_inputs.body);
    let inputs = SetInputsResponse::deserialize_json(&set_inputs.body).unwrap().inputs;
    assert_eq!(inputs["picture"]["image"].digest, digest);

    let run_cursor = cursor(endpoints.control, &endpoints.token, "run");
    let start_run = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{instance}/runs"),
        Some(&endpoints.token),
        &CreateRunRequest::default().serialize_json(),
    );
    let run_id = CreateRunResponse::deserialize_json(&start_run.body).unwrap().run_id;
    poll_events_until(endpoints.control, &endpoints.token, "run", run_cursor, |event| {
        event_kind(event) == "run.finished" && event_str(event, "run_id") == Some(run_id.as_str())
    });
    let row = request(endpoints.control, "GET", &format!("/v1/runs/{run_id}"), Some(&endpoints.token), "");
    let row = RunRowDto::deserialize_json(&row.body).unwrap();
    assert_eq!(row.outputs["result"].digest, digest);

    server.shutdown();
}

#[test]
fn value_ttl_expiry_returns_404() {
    let root = TempRoot::new("value-ttl");
    let mut config = FlowServerConfig::new(root.0.clone())
        .with_seams(seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")));
    // Force an immediate spill (so TTL, not the RAM cache, governs this
    // value's lifetime) and sweep fast enough for a test to observe it.
    config.values_ram_budget = 1;
    config.value_ttl_secs = 1;
    config.janitor_sweep_secs = 1;
    config.log = Box::new(|_| {});
    let server = FlowServer::start(config).unwrap();
    let endpoints = server.endpoints();

    let put_value = request(
        endpoints.data,
        "PUT",
        "/v1/values?type=bytes&content_type=application/octet-stream",
        Some(&endpoints.token),
        "scratch bytes nobody keeps",
    );
    assert_eq!(put_value.status, 201, "{}", put_value.body);
    let digest = PutValueResponse::deserialize_json(&put_value.body).unwrap().digest;

    let immediate = request(endpoints.data, "GET", &format!("/v1/values/{digest}"), Some(&endpoints.token), "");
    assert_eq!(immediate.status, 200);

    std::thread::sleep(Duration::from_millis(2_500));

    let expired = request(endpoints.data, "GET", &format!("/v1/values/{digest}"), Some(&endpoints.token), "");
    assert_eq!(expired.status, 404, "{}", expired.body);

    server.shutdown();
}

// ---------------------------------------------------------------------------
// 7. the real-fleet path is selected by default; the testpattern hub works
//    through the HTTP routes with `FixedGen` injected
// ---------------------------------------------------------------------------

#[test]
fn default_seams_are_the_real_fleet_path() {
    let root = TempRoot::new("default-seams");
    // No `with_seams`: `FlowServer::start` must build `FleetGen` + `HubChat`
    // + `HubHttp` on its own and still come up cleanly.
    let mut config = FlowServerConfig::new(root.0.clone());
    config.log = Box::new(|_| {});
    let server = FlowServer::start(config).unwrap();
    let endpoints = server.endpoints();
    let health = request(endpoints.control, "GET", "/v1/health", None, "");
    assert_eq!(health.status, 200, "{}", health.body);
    server.shutdown();
}

#[test]
fn testpattern_hub_service_runs_through_the_http_routes() {
    let cache_dir = std::env::temp_dir().join(format!(
        "makepad-flow-host-runs-testpattern-{}-{}",
        std::process::id(),
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos()
    ));
    std::fs::create_dir_all(&cache_dir).unwrap();
    let service = start_service(ServiceConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        cache_dir: cache_dir.clone(),
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
        peer: PeerOptions { serve: Some(false), sources: Some(Vec::new()), ..Default::default() },
        fleet: DEFAULT_FLEET.to_string(),
    });
    let handle = match service {
        Ok(handle) => handle,
        Err(error) if error.to_string().contains("Operation not permitted") => {
            let _ = std::fs::remove_dir_all(&cache_dir);
            eprintln!("skipping real hub service: loopback bind is forbidden by this sandbox");
            return;
        }
        Err(error) => panic!("start real hub service: {error}"),
    };
    let base_url = format!("http://{}", handle.addr);
    std::mem::forget(handle);

    let root = TempRoot::new("testpattern-route");
    let server = start(
        &root.0,
        Seams {
            chat: std::sync::Arc::new(FakeChat::done("unused")),
            gen: std::sync::Arc::new(FixedGen(base_url)),
            http: std::sync::Arc::new(FakeHttp::json(200, "{}")),
        },
    );
    let endpoints = server.endpoints();
    let source = r#"use mod.flow.*
let prompt = Input{default: "same prompt"}
let image = Image{prompt: prompt.text() width: 64 height: 48 steps: 4 seed: 42 model: "testpattern"}
let picture = Output{type: @image value: image.image()}
Flow{prompt, image, picture}
"#;
    put_flow(endpoints.control, &endpoints.token, "gen-demo", source);
    let create = request(
        endpoints.control,
        "POST",
        "/v1/flows/gen-demo/instances",
        Some(&endpoints.token),
        &CreateInstanceRequest::default().serialize_json(),
    );
    let instance = CreateInstanceResponse::deserialize_json(&create.body).unwrap().instance;
    let run_cursor = cursor(endpoints.control, &endpoints.token, "run");
    let start_run = request(
        endpoints.control,
        "POST",
        &format!("/v1/instances/{instance}/runs"),
        Some(&endpoints.token),
        &CreateRunRequest::default().serialize_json(),
    );
    assert_eq!(start_run.status, 202, "{}", start_run.body);
    let run_id = CreateRunResponse::deserialize_json(&start_run.body).unwrap().run_id;
    let events = poll_events_until(endpoints.control, &endpoints.token, "run", run_cursor, |event| {
        event_kind(event) == "run.finished" && event_str(event, "run_id") == Some(run_id.as_str())
    });
    assert_eq!(
        variant_name(events.last().and_then(|event| event.key("state"))),
        Some("Done"),
        "{events:#?}"
    );
    let row = request(endpoints.control, "GET", &format!("/v1/runs/{run_id}"), Some(&endpoints.token), "");
    let row = RunRowDto::deserialize_json(&row.body).unwrap();
    let digest = row.outputs["picture"].digest.clone();
    let fetched = request(endpoints.data, "GET", &format!("/v1/values/{digest}"), Some(&endpoints.token), "");
    assert_eq!(fetched.status, 200);
    assert_eq!(&fetched.bytes[..8], &[0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n']);
    assert_eq!(sha256_hex(&fetched.bytes), digest.trim_start_matches("sha256:"));

    server.shutdown();
}
