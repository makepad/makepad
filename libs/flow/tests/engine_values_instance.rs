use makepad_flow::graph::evaluate;
use makepad_flow::{
    InputEffect, Instance, Literal, Owner, PortType, RunDecision, RunId, Value, ValueStore,
    Waiting,
};
use std::collections::HashSet;
use std::time::{Duration, SystemTime};

fn temp_dir(name: &str) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!(
        "makepad-flow-{name}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn value_store_deduplicates_spills_and_expires_around_live_runs() {
    let dir = temp_dir("values");
    let mut store = ValueStore::new(dir.clone());
    store.ram_budget = 8;
    store.ttl = Duration::from_secs(1);

    let first = Value::text("1111");
    let first_digest = store.put(first.clone());
    std::thread::sleep(Duration::from_millis(2));
    let second = Value::text("2222");
    let second_digest = store.put(second);
    std::thread::sleep(Duration::from_millis(2));
    assert!(store.touch(&first_digest));
    std::thread::sleep(Duration::from_millis(2));
    let third_digest = store.put(Value::text("3333"));

    assert_eq!(store.ram_bytes(), 8);
    assert_eq!(store.spilled_bytes(), 4);
    assert!(!dir.join(makepad_ai_hub::sha256::to_hex(&first_digest)).exists());
    assert!(dir.join(makepad_ai_hub::sha256::to_hex(&second_digest)).exists());
    assert!(!dir.join(makepad_ai_hub::sha256::to_hex(&third_digest)).exists());
    assert_eq!(store.put(first), first_digest);
    assert_eq!(store.get(&second_digest).unwrap().as_text().unwrap(), "2222");

    let later = SystemTime::now() + Duration::from_secs(2);
    store.expire(later, &HashSet::from([second_digest]));
    assert!(store.get(&second_digest).is_some());
    store.expire(later + Duration::from_secs(2), &HashSet::new());
    assert!(store.get(&second_digest).is_none());
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn instance_defaults_changes_types_and_concurrency_are_pure_transitions() {
    let source = r#"use mod.flow.*
let prompt = Input{type: @text default: "hello"}
let gone = Input{type: @text default: "gone"}
let picture = Output{type: @text value: prompt.text()}
Flow{trigger: @input concurrency: 1 prompt, gone, picture}
"#;
    let mut graph = evaluate(source, "instance.splash").unwrap();
    graph.revision = 7;
    let mut instance = Instance::new(
        "demo",
        &graph,
        Some("A".to_string()),
        false,
        Owner::Tab,
        10,
    )
    .unwrap();
    assert_eq!(
        instance.inputs["prompt"]["text"].as_text().unwrap(),
        "hello"
    );
    assert_eq!(instance.inputs["gone"]["text"].as_text().unwrap(), "gone");
    assert!(instance
        .set_input("prompt", "text", Value::media(PortType::Image, "image/png", vec![1]), &graph)
        .unwrap_err()
        .contains("type mismatch"));
    assert_eq!(
        instance
            .set_input("prompt", "text", Value::text("changed"), &graph)
            .unwrap(),
        InputEffect::TriggerRun
    );

    let mut changed = evaluate(
        r#"use mod.flow.*
let prompt = Input{type: @text default: "new default"}
let extra = Input{type: @json default: {valid: true}}
let result = Output{type: @text value: prompt.text()}
Flow{concurrency: 1 prompt, extra, result}
"#,
        "changed.splash",
    )
    .unwrap();
    changed.revision = 8;
    instance.on_graph_changed(&changed).unwrap();
    assert_eq!(instance.revision, 8);
    assert_eq!(
        instance.inputs["prompt"]["text"].as_text().unwrap(),
        "changed"
    );
    let extra = makepad_strict_json::parse(&instance.inputs["extra"]["json"].bytes).unwrap();
    assert_eq!(extra.get("valid").and_then(|value| value.as_bool()), Some(true));
    assert!(!instance.inputs.contains_key("picture"));
    assert!(!instance.inputs.contains_key("gone"));

    let first = instance.request_run(None);
    assert!(matches!(first, RunDecision::Start(_)));
    assert_eq!(instance.request_run(None), RunDecision::Queued(1));
}

#[test]
fn pinned_instance_ignores_graph_changes() {
    let graph = evaluate(
        "use mod.flow.*\nlet a = Input{default: \"a\"}\nFlow{a}",
        "a.splash",
    )
    .unwrap();
    let changed = evaluate(
        "use mod.flow.*\nlet b = Input{default: \"b\"}\nFlow{b}",
        "b.splash",
    )
    .unwrap();
    let mut instance = Instance::new("a", &graph, None, true, Owner::Service, 1).unwrap();
    instance.on_graph_changed(&changed).unwrap();
    assert!(instance.inputs.contains_key("a"));
    assert!(!instance.inputs.contains_key("b"));
}

#[test]
fn waiting_ask_is_answered_through_the_instance_input_transition() {
    let graph = evaluate(
        "use mod.flow.*\nlet choice = Ask{type: @text question: \"Which?\"}\nFlow{choice}",
        "ask-instance.splash",
    )
    .unwrap();
    let run = RunId("run_waiting".to_string());
    let mut instance = Instance::new("ask", &graph, None, false, Owner::Tab, 1).unwrap();
    instance.waiting = Some(Waiting {
        run: run.clone(),
        node: "choice".to_string(),
        question: "Which?".to_string(),
        ty: PortType::Text,
        options: Vec::new(),
    });
    assert_eq!(
        instance
            .set_input("choice", "text", Value::text("first"), &graph)
            .unwrap(),
        InputEffect::Answered(run)
    );
    assert!(instance.waiting.is_none());
    assert_eq!(
        instance.inputs["choice"]["text"].as_text().unwrap(),
        "first"
    );
}

#[test]
fn value_literal_shapes_cover_numbers_bools_and_lists() {
    let json = Value::from_literal(PortType::Json, &Literal::Num(2.0)).unwrap();
    assert_eq!(json.as_text().unwrap(), "2");
    let boolean = Value::from_literal(PortType::Json, &Literal::Bool(true)).unwrap();
    assert_eq!(boolean.as_text().unwrap(), "true");
    let list = Value::from_literal(
        PortType::List,
        &Literal::Arr(vec![Literal::Num(1.0), Literal::Str("x".to_string())]),
    )
    .unwrap();
    assert_eq!(list.as_text().unwrap(), "[1,\"x\"]");
}
