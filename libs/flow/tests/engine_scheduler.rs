mod support;

use makepad_flow::engine::{spawn_run, RunEvent, RunId, RunInput, RunState};
use makepad_flow::graph::evaluate;
use makepad_flow::Value;
use std::collections::BTreeMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::Duration;
use support::{event_name, output, seams, FakeChat, FakeGen, FakeHttp, GenMode};

const DESIGN_FLOW: &str = r#"use mod.flow.*
let prompt = Input{type: @text default: "lighthouse"}
let expand = Llm{system: "expand" prompt: prompt.text()}
let styled = Fn{in: {text: expand.text() style: "photo"} out: [@text] run: |i| {{text: i.text + ", " + i.style + " style"}}}
let image = Image{prompt: styled.text() width: 64 height: 64 seed: 7 model: "fake"}
let picture = Output{type: @image value: image.image()}
Flow{label: "design" prompt, expand, styled, image, picture}
"#;

#[test]
fn design_pipeline_runs_in_order_and_returns_picture() {
    let chat = FakeChat::done("expanded");
    let gen = FakeGen::done();
    let origins = gen.origins.clone();
    let events = support::run(
        DESIGN_FLOW,
        seams(chat, gen, FakeHttp::json(200, "{}")),
        None,
    );
    let names: Vec<_> = events.iter().map(event_name).collect();
    assert_eq!(
        names,
        [
            "run.started",
            "node.done:prompt",
            "node.started:expand",
            "node.delta:expand",
            "node.done:expand",
            "node.done:styled",
            "node.started:image",
            "node.progress:image",
            "node.done:image",
            "run.finished:Done",
        ]
    );
    assert_eq!(output(&events, "picture").content_type, "image/png");
    assert_eq!(
        origins.lock().unwrap().as_slice(),
        &[(Some("test-origin".to_string()), Some(9))]
    );
}

#[test]
fn fanout_generation_nodes_start_in_one_scheduler_tick() {
    let source = r#"use mod.flow.*
let prompt = Input{default: "x"}
let expand = Llm{prompt: prompt.text()}
let left = Image{prompt: expand.text() model: "fake"}
let right = Image{prompt: expand.text() model: "fake"}
let left_out = Output{type: @image value: left.image()}
let right_out = Output{type: @image value: right.image()}
Flow{prompt, expand, left, right, left_out, right_out}
"#;
    let gen = FakeGen::done();
    let starts = gen.starts.clone();
    let events = support::run(
        source,
        seams(FakeChat::done("expanded"), gen, FakeHttp::json(200, "{}")),
        None,
    );
    assert!(matches!(events.last(), Some(RunEvent::RunFinished { state: RunState::Done, .. })));
    let starts = starts.lock().unwrap();
    assert_eq!(starts.len(), 2);
    assert!(starts[1].0.duration_since(starts[0].0) < Duration::from_millis(50));
}

#[test]
fn failed_generation_skips_dependents_and_fails_run() {
    let gen = FakeGen {
        mode: GenMode::Fail,
        ..FakeGen::done()
    };
    let events = support::run(
        DESIGN_FLOW,
        seams(FakeChat::done("expanded"), gen, FakeHttp::json(200, "{}")),
        None,
    );
    let names: Vec<_> = events.iter().map(event_name).collect();
    assert!(names.contains(&"node.failed:image".to_string()), "{names:#?}");
    assert!(
        names.contains(&"node.skipped:picture:upstream".to_string()),
        "{names:#?}"
    );
    assert_eq!(names.last().unwrap(), "run.finished:Failed");
}

#[test]
fn skipped_input_failure_flows_its_declared_default() {
    let source = r#"use mod.flow.*
let prompt = Input{type: @text default: "fallback" on_fail: @skip}
let add = Fn{in: {text: prompt.text()} out: [@text] run: |i| {{text: i.text + "!"}}}
let result = Output{type: @text value: add.text()}
Flow{prompt, add, result}
"#;
    let graph = evaluate(source, "input-skip.splash").unwrap();
    let inputs = BTreeMap::from([(
        "prompt".to_string(),
        BTreeMap::from([("text".to_string(), Value::json("{}"))]),
    )]);
    let events = support::run_graph(
        source,
        graph,
        seams(
            FakeChat::done("unused"),
            FakeGen::done(),
            FakeHttp::json(200, "{}"),
        ),
        None,
        inputs,
    );
    assert!(events.iter().any(|event| matches!(
        event,
        RunEvent::NodeSkipped { node, reason }
            if node == "prompt" && reason.contains("expected text")
    )));
    assert_eq!(output(&events, "result").as_text().unwrap(), "fallback!");
}

#[test]
fn cancel_mid_run_calls_inflight_cancel_and_finishes_cancelled() {
    let gen = FakeGen {
        mode: GenMode::Pending,
        ..FakeGen::done()
    };
    let cancelled = gen.cancelled.clone();
    let mut graph = evaluate(DESIGN_FLOW, "cancel.splash").unwrap();
    graph.revision = 4;
    let input = RunInput {
        run_id: RunId("run_cancel".to_string()),
        instance: "inst_cancel".to_string(),
        source: DESIGN_FLOW.to_string(),
        file_name: "cancel.splash".to_string(),
        graph_revision: 4,
        graph,
        inputs: BTreeMap::new(),
        outputs: None,
        origin: ("cancel".to_string(), 1),
    };
    let (sender, receiver) = mpsc::channel();
    let handle = spawn_run(
        input,
        seams(FakeChat::done("expanded"), gen, FakeHttp::json(200, "{}")),
        sender,
    );
    let mut seen = support::receive_until(&receiver, |event| {
        matches!(event, RunEvent::NodeStarted { node } if node == "image")
    });
    handle.cancel.store(true, Ordering::Relaxed);
    handle.join.join().unwrap();
    seen.extend(receiver.try_iter());
    assert!(cancelled.load(Ordering::Relaxed));
    assert!(matches!(
        seen.last(),
        Some(RunEvent::RunFinished {
            state: RunState::Cancelled,
            ..
        })
    ));
}

#[test]
fn requested_outputs_prune_unrelated_branch() {
    let source = r#"use mod.flow.*
let prompt = Input{default: "paint"}
let image = Image{prompt: prompt.text() model: "fake"}
let picture = Output{type: @image value: image.image()}
let other_prompt = Input{default: "other"}
let other_image = Image{prompt: other_prompt.text() model: "fake"}
let other = Output{type: @image value: other_image.image()}
Flow{tools: {paint: {in: [prompt] out: [picture]}} prompt, image, picture, other_prompt, other_image, other}
"#;
    let gen = FakeGen::done();
    let starts = gen.starts.clone();
    let events = support::run(
        source,
        seams(FakeChat::done("unused"), gen, FakeHttp::json(200, "{}")),
        Some(vec!["picture".to_string()]),
    );
    assert_eq!(starts.lock().unwrap().len(), 1);
    assert_eq!(output(&events, "picture").content_type, "image/png");
    assert!(!events.iter().any(
        |event| matches!(event, RunEvent::NodeStarted { node } if node == "other_image")
    ));
}

#[test]
fn fn_contract_and_budget_failures_are_node_events() {
    for (body, expected) in [
        ("{json: i}", "text"),
        ("loop {} {text: \"never\"}", "limit"),
    ] {
        let source = format!(
            "use mod.flow.*\nlet bad = Fn{{in: {{}} out: [@text] run: |i| {{{body}}}}}\nlet result = Output{{type: @text value: bad.text()}}\nFlow{{bad, result}}\n"
        );
        let events = support::run(
            &source,
            seams(
                FakeChat::done("unused"),
                FakeGen::done(),
                FakeHttp::json(200, "{}"),
            ),
            None,
        );
        assert!(events.iter().any(|event| matches!(
            event,
            RunEvent::NodeFailed { node, error }
                if node == "bad" && error.contains(expected)
        )), "{events:#?}");
    }
}

#[test]
fn unsupported_generation_params_are_visible_run_warnings() {
    let source = r#"use mod.flow.*
let custom = Gen{domain: "image" quality: 4 ports: {in: [@prompt] out: [@image]} prompt: "x"}
let result = Output{type: @image value: custom.out(@image)}
Flow{custom, result}
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
        RunEvent::RunFinished { warnings, .. }
            if warnings.iter().any(|warning| warning.contains("quality"))
    )));
}
