mod support;

use makepad_ai_hub::client::{ArtifactBytes, ContentProvider};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::protocol::{
    ArtifactRefJson, GenerateRequestJson, HealthJson, JobStatusJson, ModelInfoJson, JOB_STATE_DONE,
};
use makepad_ai_hub::registry::Domain;
use makepad_flow::engine::executors::gen::GenSeam;
use makepad_flow::engine::executors::http::HttpResp;
use makepad_flow::engine::{
    spawn_run, spawn_run_with_policy, NetPolicy, RunEvent, RunId, RunInput, RunState, Seams,
};
use makepad_flow::graph::evaluate;
use makepad_flow::{InputEffect, Instance, Owner, RunDecision, Value, ValueStore};
use std::collections::{BTreeMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use support::{output, seams, FakeChat, FakeGen, FakeHttp, GenMode};

fn run_input(source: &str, name: &str) -> RunInput {
    let mut graph = evaluate(source, name).unwrap();
    graph.revision = 1;
    RunInput {
        run_id: RunId(format!("run_{name}")),
        instance: "inst_battery".to_string(),
        source: source.to_string(),
        file_name: name.to_string(),
        graph_revision: 1,
        graph,
        inputs: BTreeMap::new(),
        outputs: None,
        origin: ("battery".to_string(), 1),
    }
}

fn run_with_policy(source: &str, http: FakeHttp, policy: NetPolicy) -> Vec<RunEvent> {
    let (sender, receiver) = mpsc::channel();
    let handle = spawn_run_with_policy(
        run_input(source, "battery-http.splash"),
        seams(FakeChat::done("unused"), FakeGen::done(), http),
        sender,
        policy,
    );
    handle.join.join().unwrap();
    receiver.try_iter().collect()
}

fn temp_dir(name: &str) -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/battery")
        .join(format!("{name}-{}-{unique}", std::process::id()));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn fan_out_starts_four_generation_nodes_in_one_scheduler_burst() {
    let source = r#"use mod.flow.*
let prompt = Input{default: "x"}
let expand = Llm{prompt: prompt.text()}
let a = Image{prompt: expand.text()}
let b = Image{prompt: expand.text()}
let c = Image{prompt: expand.text()}
let d = Image{prompt: expand.text()}
let ao = Output{type: @image value: a.image()}
let bo = Output{type: @image value: b.image()}
let co = Output{type: @image value: c.image()}
let do_ = Output{type: @image value: d.image()}
Flow{prompt, expand, a, b, c, d, ao, bo, co, do_}
"#;
    let events = support::run(
        source,
        seams(FakeChat::done("expanded"), FakeGen::done(), FakeHttp::json(200, "{}")),
        None,
    );
    let feeder_done = events
        .iter()
        .position(|event| matches!(event, RunEvent::NodeDone { node, .. } if node == "expand"))
        .unwrap();
    let first_generated_done = events
        .iter()
        .position(|event| {
            matches!(event, RunEvent::NodeDone { node, .. } if ["a", "b", "c", "d"].contains(&node.as_str()))
        })
        .unwrap();
    let starts: Vec<_> = events[feeder_done + 1..first_generated_done]
        .iter()
        .filter_map(|event| match event {
            RunEvent::NodeStarted { node } if ["a", "b", "c", "d"].contains(&node.as_str()) => {
                Some(node.as_str())
            }
            _ => None,
        })
        .collect();
    assert_eq!(starts.len(), 4, "{events:#?}");
}

#[test]
fn fan_in_runs_once_after_all_three_inputs_and_receives_every_value() {
    let source = r#"use mod.flow.*
let a = Input{default: "a"}
let b = Input{default: "b"}
let c = Input{default: "c"}
let join = Fn{in: {a: a.text() b: b.text() c: c.text()} out: [@text] run: |i| {{text: i.a + i.b + i.c}}}
let result = Output{value: join.text()}
Flow{a, b, c, join, result}
"#;
    let events = support::run(
        source,
        seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")),
        None,
    );
    let input_done: Vec<_> = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| match event {
            RunEvent::NodeDone { node, .. } if ["a", "b", "c"].contains(&node.as_str()) => {
                Some(index)
            }
            _ => None,
        })
        .collect();
    let joins: Vec<_> = events
        .iter()
        .enumerate()
        .filter(|(_, event)| matches!(event, RunEvent::NodeDone { node, .. } if node == "join"))
        .collect();
    assert_eq!(input_done.len(), 3, "{events:#?}");
    assert_eq!(joins.len(), 1, "{events:#?}");
    assert!(joins[0].0 > *input_done.iter().max().unwrap());
    assert_eq!(output(&events, "result").as_text().unwrap(), "abc");
}

#[test]
fn generation_failure_skip_uses_defaults_but_fail_blocks_dependents() {
    let source = |skip: bool| {
        format!(
            r#"use mod.flow.*
let generated = Gen{{domain: "image" ports: {{in: {{prompt: @text}} out: {{text: @text}}}} prompt: "x" text: "fallback" {}}}
let append = Fn{{in: {{text: generated.out(@text)}} out: [@text] run: |i| {{{{text: i.text + "!"}}}}}}
let result = Output{{value: append.text()}}
Flow{{generated, append, result}}
"#,
            if skip { "on_fail: @skip" } else { "" }
        )
    };
    let failed_gen = || FakeGen {
        mode: GenMode::Fail,
        ..FakeGen::done()
    };
    let skipped = support::run(
        &source(true),
        seams(FakeChat::done("unused"), failed_gen(), FakeHttp::json(200, "{}")),
        None,
    );
    assert!(skipped.iter().any(
        |event| matches!(event, RunEvent::NodeSkipped { node, .. } if node == "generated")
    ));
    assert_eq!(output(&skipped, "result").as_text().unwrap(), "fallback!");

    let failed = support::run(
        &source(false),
        seams(FakeChat::done("unused"), failed_gen(), FakeHttp::json(200, "{}")),
        None,
    );
    assert!(failed.iter().any(|event| matches!(
        event,
        RunEvent::NodeSkipped { node, reason } if node == "append" && reason == "upstream"
    )));
    assert!(matches!(
        failed.last(),
        Some(RunEvent::RunFinished {
            state: RunState::Failed,
            ..
        })
    ));
}

#[test]
fn cancellation_reaches_inflight_generation_and_parked_ask_promptly() {
    let generation = r#"use mod.flow.*
let image = Image{prompt: "wait"}
let result = Output{type: @image value: image.image()}
Flow{image, result}
"#;
    let gen = FakeGen {
        mode: GenMode::Pending,
        ..FakeGen::done()
    };
    let cancelled = gen.cancelled.clone();
    let (sender, receiver) = mpsc::channel();
    let handle = spawn_run(
        run_input(generation, "cancel-generation.splash"),
        seams(FakeChat::done("unused"), gen, FakeHttp::json(200, "{}")),
        sender,
    );
    let mut events = support::receive_until(
        &receiver,
        |event| matches!(event, RunEvent::NodeStarted { node } if node == "image"),
    );
    let cancelled_at = Instant::now();
    handle.cancel.store(true, Ordering::Relaxed);
    handle.join.join().unwrap();
    events.extend(receiver.try_iter());
    assert!(cancelled_at.elapsed() < Duration::from_millis(300));
    assert!(cancelled.load(Ordering::Relaxed));
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            state: RunState::Cancelled,
            ..
        })
    ));

    let ask = "use mod.flow.*\nlet ask = Ask{question: \"wait\"}\nlet result = Output{value: ask.text()}\nFlow{ask, result}\n";
    let (sender, receiver) = mpsc::channel();
    let handle = spawn_run(
        run_input(ask, "cancel-ask.splash"),
        seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")),
        sender,
    );
    let mut events = support::receive_until(
        &receiver,
        |event| matches!(event, RunEvent::NodeWaiting { node, .. } if node == "ask"),
    );
    let cancelled_at = Instant::now();
    handle.cancel.store(true, Ordering::Relaxed);
    handle.join.join().unwrap();
    events.extend(receiver.try_iter());
    assert!(cancelled_at.elapsed() < Duration::from_millis(300));
    assert!(!events.iter().any(|event| matches!(event, RunEvent::NodeAnswered { .. })));
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            state: RunState::Cancelled,
            ..
        })
    ));
}

#[test]
fn independent_asks_park_answer_independently_ignore_strays_and_time_out() {
    let source = r#"use mod.flow.*
let left = Ask{question: "left?"}
let right = Ask{question: "right?"}
let left_fn = Fn{in: {text: left.text()} out: [@text] run: |i| {{text: i.text + "!"}}}
let right_fn = Fn{in: {text: right.text()} out: [@text] run: |i| {{text: i.text + "!"}}}
let left_out = Output{value: left_fn.text()}
let right_out = Output{value: right_fn.text()}
Flow{left, right, left_fn, right_fn, left_out, right_out}
"#;
    let (sender, receiver) = mpsc::channel();
    let handle = spawn_run(
        run_input(source, "two-asks.splash"),
        seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")),
        sender,
    );
    let mut events = support::receive_until(&receiver, |event| {
        matches!(event, RunEvent::NodeWaiting { node, .. } if node == "right")
    });
    assert!(events.iter().any(
        |event| matches!(event, RunEvent::NodeWaiting { node, .. } if node == "left")
    ));
    handle
        .answer
        .send(("not_waiting".to_string(), Value::text("ignored")))
        .unwrap();
    handle.answer.send(("left".to_string(), Value::text("yes"))).unwrap();
    events.extend(support::receive_until(&receiver, |event| {
        matches!(event, RunEvent::NodeDone { node, .. } if node == "left_fn")
    }));
    assert!(events.iter().any(
        |event| matches!(event, RunEvent::NodeAnswered { node, .. } if node == "left")
    ));
    assert!(!events.iter().any(
        |event| matches!(event, RunEvent::NodeAnswered { node, .. } if node == "right" || node == "not_waiting")
    ));
    assert!(!events.iter().any(|event| matches!(event, RunEvent::RunFinished { .. })));
    handle.cancel.store(true, Ordering::Relaxed);
    handle.join.join().unwrap();

    let timeout = "use mod.flow.*\nlet ask = Ask{question: \"wait\" timeout: 1}\nlet result = Output{value: ask.text()}\nFlow{ask, result}\n";
    let started = Instant::now();
    let events = support::run(
        timeout,
        seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")),
        None,
    );
    assert!(started.elapsed() >= Duration::from_millis(900));
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::NodeFailed { node, error } if node == "ask" && error.contains("timeout")
    )));
}

#[test]
fn real_fn_trims_budgets_types_outputs_and_isolates_input_mutation() {
    let source = r#"use mod.flow.*
let original = Input{default: "  Mixed  "}
let trim = Fn{in: {text: original.text()} out: [@text] run: |i| {{text: i.text.trim()}}}
let mutate = Fn{in: {text: original.text()} out: [@text] run: |i| {i.text = "changed" return {text: i.text}}}
let observe = Fn{in: {text: original.text()} out: [@text] run: |i| {{text: i.text}}}
let wrong = Fn{in: {} out: [@text] run: |i| {{text: 7}}}
let forever = Fn{in: {} out: [@text] run: |i| {loop {} {text: "never"}}}
let trimmed = Output{value: trim.text()}
let mutated = Output{value: mutate.text()}
let observed = Output{value: observe.text()}
let wrong_out = Output{value: wrong.text()}
let forever_out = Output{value: forever.text()}
Flow{original, trim, mutate, observe, wrong, forever, trimmed, mutated, observed, wrong_out, forever_out}
"#;
    let started = Instant::now();
    let events = support::run(
        source,
        seams(FakeChat::done("unused"), FakeGen::done(), FakeHttp::json(200, "{}")),
        None,
    );
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(output(&events, "trimmed").as_text().unwrap(), "Mixed");
    assert_eq!(output(&events, "mutated").as_text().unwrap(), "changed");
    assert_eq!(output(&events, "observed").as_text().unwrap(), "  Mixed  ");
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::NodeFailed { node, error }
            if node == "wrong" && error.contains("text")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::NodeFailed { node, error }
            if node == "forever" && error.contains("limit")
    )));
}

#[test]
#[ignore = "BUG: fake HTTP 3xx responses are accepted instead of refused as redirects"]
fn http_accept_policy_private_redirect_and_json_contracts() {
    let source = |accept: &str, out: &str, url: &str| {
        format!(
            "use mod.flow.*\nlet http = Http{{url: \"{url}\" out: @{out} accept: [{accept}]}}\nlet result = Output{{type: @{out} value: http.value()}}\nFlow{{http, result}}\n"
        )
    };
    let accepted = run_with_policy(
        &source("404", "json", "http://example.test/missing"),
        FakeHttp::json(404, "{}"),
        NetPolicy::default(),
    );
    assert!(matches!(
        accepted.last(),
        Some(RunEvent::RunFinished {
            state: RunState::Done,
            ..
        })
    ));

    let denied_http = FakeHttp::json(200, "{}");
    let calls = denied_http.calls.clone();
    let denied = run_with_policy(
        &source("", "json", "http://example.test/data"),
        denied_http,
        NetPolicy {
            allow: Vec::new(),
            deny_private: false,
        },
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(denied.iter().any(|event| matches!(
        event,
        RunEvent::RunFinished { http_log, .. } if http_log.len() == 1
    )));

    let private = run_with_policy(
        &source("", "json", "http://10.0.0.1/data"),
        FakeHttp::json(200, "{}"),
        NetPolicy {
            allow: vec!["*".to_string()],
            deny_private: true,
        },
    );
    assert!(private.iter().any(|event| matches!(
        event,
        RunEvent::NodeFailed { error, .. } if error.contains("private")
    )));

    let invalid = run_with_policy(
        &source("", "json", "http://example.test/data"),
        FakeHttp::json(200, "not json"),
        NetPolicy::default(),
    );
    assert!(invalid.iter().any(|event| matches!(
        event,
        RunEvent::NodeFailed { error, .. } if error.contains("invalid JSON")
    )));

    let redirect = run_with_policy(
        &source("", "text", "http://example.test/redirect"),
        FakeHttp {
            response: HttpResp {
                status: 302,
                headers: vec![("location".to_string(), "https://elsewhere.test".to_string())],
                body: Vec::new(),
            },
            calls: Arc::new(Default::default()),
        },
        NetPolicy::default(),
    );
    assert!(redirect.iter().any(|event| matches!(
        event,
        RunEvent::NodeFailed { error, .. } if error.contains("redirect")
    )));
}

#[test]
fn value_store_spills_gets_expires_and_deduplicates_by_digest() {
    let dir = temp_dir("values");
    let mut store = ValueStore::new(dir.clone());
    store.ram_budget = 1024;
    store.ttl = Duration::from_millis(1);
    let values = [
        Value::text("a".repeat(600)),
        Value::text("b".repeat(600)),
        Value::text("c".repeat(600)),
    ];
    let digests: Vec<_> = values.iter().cloned().map(|value| store.put(value)).collect();
    assert_eq!(store.ram_bytes(), 600);
    assert_eq!(store.spilled_bytes(), 1200);
    for (digest, expected) in digests.iter().zip(&values) {
        assert_eq!(&store.get(digest).unwrap(), expected);
    }
    assert_eq!(store.put(values[0].clone()), digests[0]);
    assert_eq!(store.put(values[0].clone()), digests[0]);

    std::thread::sleep(Duration::from_millis(2));
    let live = HashSet::from([digests[0]]);
    store.expire(SystemTime::now() + Duration::from_secs(1), &live);
    assert!(dir.join(makepad_ai_hub::sha256::to_hex(&digests[0])).exists());
    assert!(!dir.join(makepad_ai_hub::sha256::to_hex(&digests[1])).exists());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn instance_pins_unparked_ask_reload_and_concurrency_transitions() {
    let ask_source = "use mod.flow.*\nlet choice = Ask{question: \"which?\"}\nFlow{choice}\n";
    let ask_graph = evaluate(ask_source, "instance-ask.splash").unwrap();
    let mut ask_instance = Instance::new("ask", &ask_graph, None, false, Owner::Tab, 1).unwrap();
    assert_eq!(
        ask_instance
            .set_input("choice", "text", Value::text("early"), &ask_graph)
            .unwrap(),
        InputEffect::None
    );
    assert_eq!(ask_instance.inputs["choice"]["text"].as_text().unwrap(), "early");

    let original = evaluate(
        "use mod.flow.*\nlet old = Input{default: \"old\"}\nFlow{concurrency: 1 old}\n",
        "old.splash",
    )
    .unwrap();
    let mut instance = Instance::new("reload", &original, None, false, Owner::Tab, 1).unwrap();
    let changed = evaluate(
        "use mod.flow.*\nlet new = Input{default: \"new default\"}\nFlow{concurrency: 2 new}\n",
        "new.splash",
    )
    .unwrap();
    instance.on_graph_changed(&changed).unwrap();
    assert!(!instance.inputs.contains_key("old"));
    assert_eq!(instance.inputs["new"]["text"].as_text().unwrap(), "new default");
    assert!(matches!(instance.request_run(None), RunDecision::Start(_)));
    assert!(matches!(instance.request_run(None), RunDecision::Start(_)));
    assert_eq!(instance.request_run(None), RunDecision::Queued(1));
}

#[derive(Clone, Default)]
struct SeededGen;

impl GenSeam for SeededGen {
    fn pick(&self, _domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        Ok(Box::new(SeededProvider {
            bytes: Arc::new(Mutex::new(Vec::new())),
        }))
    }
}

struct SeededProvider {
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl ContentProvider for SeededProvider {
    fn health(&self) -> Result<HealthJson, AssetAiError> {
        Err(AssetAiError::Unavailable("unused".to_string()))
    }

    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> {
        Err(AssetAiError::Unavailable("unused".to_string()))
    }

    fn request(
        &self,
        _domain: Domain,
        request: &GenerateRequestJson,
    ) -> Result<String, AssetAiError> {
        *self.bytes.lock().unwrap() = request.seed.unwrap_or_default().to_le_bytes().to_vec();
        Ok("seeded-job".to_string())
    }

    fn poll(&self, _job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        let bytes = self.bytes.lock().unwrap();
        Ok(JobStatusJson {
            job_id: "seeded-job".to_string(),
            state: JOB_STATE_DONE.to_string(),
            stage: None,
            progress: None,
            artifacts: vec![ArtifactRefJson {
                id: "seeded-artifact".to_string(),
                url: "/seeded-artifact".to_string(),
                content_type: "application/octet-stream".to_string(),
                sha256: Some(makepad_ai_hub::sha256::sha256_hex(&bytes)),
                byte_len: Some(bytes.len() as u64),
            }],
            error: None,
            model: Some("seeded".to_string()),
            queued_ms: None,
            started_ms: None,
            finished_ms: None,
            log: None,
            partial_text: None,
            live: None,
            serving: None,
            text: None,
        })
    }

    fn fetch_artifact(&self, _artifact_id: &str) -> Result<ArtifactBytes, AssetAiError> {
        Ok(ArtifactBytes {
            content_type: "image/png".to_string(),
            bytes: self.bytes.lock().unwrap().clone(),
        })
    }

    fn cancel(&self, _job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        self.poll("seeded-job")
    }
}

fn seeded_run(seed: u64) -> Vec<RunEvent> {
    let source = format!(
        "use mod.flow.*\nlet image = Image{{prompt: \"same\" seed: {seed}}}\nlet result = Output{{type: @image value: image.image()}}\nFlow{{image, result}}\n"
    );
    support::run(
        &source,
        Seams {
            chat: Arc::new(FakeChat::done("unused")),
            gen: Arc::new(SeededGen),
            http: Arc::new(FakeHttp::json(200, "{}")),
        },
        None,
    )
}

#[test]
fn seeded_fake_generation_is_repeatable_and_seed_sensitive() {
    let first = seeded_run(0);
    let second = seeded_run(0);
    let different = seeded_run(1);
    assert_eq!(output(&first, "result").digest, output(&second, "result").digest);
    assert_ne!(output(&first, "result").digest, output(&different, "result").digest);
}
