mod support;

use makepad_flow::engine::{
    spawn_run, spawn_run_with_policy, NetPolicy, RunEvent, RunId, RunInput, RunState,
};
use makepad_flow::graph::evaluate;
use makepad_flow::Value;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use support::{event_name, output, seams, FakeChat, FakeGen, FakeHttp};

fn http_source(accept: &str, url: &str) -> String {
    format!(
        r#"use mod.flow.*
let feed = Http{{method: @get url: "{url}" headers: {{}} out: @json accept: [{accept}]}}
let result = Output{{type: @json value: feed.value()}}
Flow{{feed, result}}
"#
    )
}

fn run_http(source: &str, http: FakeHttp, policy: NetPolicy) -> Vec<RunEvent> {
    let mut graph = evaluate(source, "http.splash").unwrap();
    graph.revision = 2;
    let input = RunInput {
        run_id: RunId("run_http".to_string()),
        instance: "inst_http".to_string(),
        source: source.to_string(),
        file_name: "http.splash".to_string(),
        graph_revision: 2,
        graph,
        inputs: BTreeMap::new(),
        outputs: None,
        origin: ("http-test".to_string(), 1),
    };
    let (sender, receiver) = mpsc::channel();
    let handle = spawn_run_with_policy(
        input,
        seams(FakeChat::done("unused"), FakeGen::done(), http),
        sender,
        policy,
    );
    handle.join.join().unwrap();
    receiver.try_iter().collect()
}

#[test]
fn http_json_and_meta_are_typed() {
    let source = http_source("", "http://127.0.0.1/data");
    let events = run_http(
        &source,
        FakeHttp::json(200, "{\"answer\":42}"),
        NetPolicy {
            allow: vec!["127.0.0.1".to_string()],
            deny_private: false,
        },
    );
    let json = makepad_strict_json::parse(&output(&events, "result").bytes).unwrap();
    assert_eq!(json.get("answer").and_then(|value| value.as_i64()), Some(42));
    let meta = events
        .iter()
        .find_map(|event| match event {
            RunEvent::NodeDone { node, outputs } if node == "feed" => outputs
                .iter()
                .find_map(|(port, value)| (port == "meta").then_some(value)),
            _ => None,
        })
        .unwrap();
    let meta = makepad_strict_json::parse(&meta.bytes).unwrap();
    assert_eq!(meta.get("status").and_then(|value| value.as_i64()), Some(200));
}

#[test]
fn http_404_requires_explicit_acceptance() {
    let rejected = run_http(
        &http_source("", "http://127.0.0.1/missing"),
        FakeHttp::json(404, "{}"),
        NetPolicy::default(),
    );
    assert!(rejected.iter().any(|event| matches!(
        event,
        RunEvent::NodeFailed { node, error }
            if node == "feed" && error.contains("404")
    )));
    assert!(matches!(
        rejected.last(),
        Some(RunEvent::RunFinished {
            state: RunState::Failed,
            ..
        })
    ));

    let accepted = run_http(
        &http_source("404", "http://127.0.0.1/missing"),
        FakeHttp::json(404, "{\"missing\":true}"),
        NetPolicy::default(),
    );
    assert!(matches!(
        accepted.last(),
        Some(RunEvent::RunFinished {
            state: RunState::Done,
            ..
        })
    ));
}

#[test]
fn denied_http_never_reaches_seam_and_is_logged() {
    let http = FakeHttp::json(200, "{}");
    let calls = http.calls.clone();
    let events = run_http(
        &http_source("", "https://example.org/private"),
        http,
        NetPolicy {
            allow: vec!["127.0.0.1".to_string()],
            deny_private: false,
        },
    );
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::NodeFailed { error, .. } if error.contains("refused by policy")
    )));
    let log = events
        .iter()
        .find_map(|event| match event {
            RunEvent::RunFinished { http_log, .. } => Some(http_log),
            _ => None,
        })
        .unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].url, "https://example.org/private");
    assert_eq!(log[0].status, None);
}

#[test]
fn ask_parks_while_other_branch_runs_then_answer_resumes_dependents() {
    let source = r#"use mod.flow.*
let which = Ask{question: "Which?" type: @text options: ["first", "second"]}
let add = Fn{in: {text: which.text()} out: [@text] run: |i| {{text: i.text + "!"}}}
let choice = Output{type: @text value: add.text()}
let prompt = Input{default: "parallel"}
let expand = Llm{prompt: prompt.text()}
let other = Output{type: @text value: expand.text()}
Flow{which, add, choice, prompt, expand, other}
"#;
    let mut graph = evaluate(source, "ask.splash").unwrap();
    graph.revision = 1;
    let input = RunInput {
        run_id: RunId("run_ask".to_string()),
        instance: "inst_ask".to_string(),
        source: source.to_string(),
        file_name: "ask.splash".to_string(),
        graph_revision: 1,
        graph,
        inputs: BTreeMap::new(),
        outputs: None,
        origin: ("ask-test".to_string(), 1),
    };
    let (sender, receiver) = mpsc::channel();
    let handle = spawn_run(
        input,
        seams(
            FakeChat::done("parallel done"),
            FakeGen::done(),
            FakeHttp::json(200, "{}"),
        ),
        sender,
    );
    let seen = support::receive_until(&receiver, |event| {
        matches!(event, RunEvent::NodeDone { node, .. } if node == "expand")
    });
    assert!(seen.iter().any(|event| matches!(
        event,
        RunEvent::NodeWaiting { node, question, .. }
            if node == "which" && question == "Which?"
    )));
    handle
        .answer
        .send(("which".to_string(), Value::text("first")))
        .unwrap();
    handle.join.join().unwrap();
    let mut events = seen;
    events.extend(receiver.try_iter());
    let names: Vec<_> = events.iter().map(event_name).collect();
    assert!(names.contains(&"node.answered:which".to_string()), "{names:#?}");
    assert_eq!(output(&events, "choice").as_text().unwrap(), "first!");
}

#[test]
fn ask_timeout_with_skip_flows_default() {
    let source = r#"use mod.flow.*
let which = Ask{question: "Which?" type: @text default: "fallback" timeout: 1 on_fail: @skip}
let add = Fn{in: {text: which.text()} out: [@text] run: |i| {{text: i.text + "!"}}}
let choice = Output{type: @text value: add.text()}
Flow{which, add, choice}
"#;
    let events = support::run(
        source,
        seams(
            FakeChat::done("unused"),
            FakeGen::done(),
            FakeHttp::json(200, "{}"),
        ),
        None,
    );
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::NodeSkipped { node, reason }
            if node == "which" && reason == "timeout"
    )));
    assert_eq!(output(&events, "choice").as_text().unwrap(), "fallback!");
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            state: RunState::Done,
            ..
        })
    ));
}
