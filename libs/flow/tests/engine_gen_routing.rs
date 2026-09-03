mod support;

use makepad_ai_hub::registry::Domain;
use makepad_flow::engine::executors::gen::GenExecutor;
use makepad_flow::engine::executors::Executor;
use makepad_flow::graph::evaluate;
use makepad_flow::{PortType, Value};
use std::sync::Arc;
use support::FakeGen;

#[test]
fn inpaint_and_control_media_follow_their_backend_wire_contracts() {
    let source = r#"use mod.flow.*
let inpaint = Inpaint{}
let control = Control{}
Flow{inpaint, control}
"#;
    let graph = evaluate(source, "gen-routing.splash").unwrap();
    let fake = FakeGen::done();
    let requests = fake.requests.clone();

    let inpaint = graph.nodes.iter().find(|node| node.id == "inpaint").unwrap();
    let mut executor = GenExecutor::new(Arc::new(fake.clone()), ("routing".to_string(), 1));
    executor
        .start(
            inpaint,
            &[
                ("image".to_string(), Value::media(PortType::Image, "image/png", vec![1])),
                ("mask".to_string(), Value::media(PortType::Image, "image/png", vec![2])),
            ],
        )
        .unwrap();

    let control = graph.nodes.iter().find(|node| node.id == "control").unwrap();
    let mut executor = GenExecutor::new(Arc::new(fake), ("routing".to_string(), 1));
    executor
        .start(
            control,
            &[(
                "control".to_string(),
                Value::media(PortType::Image, "image/png", vec![3]),
            )],
        )
        .unwrap();

    let requests = requests.lock().unwrap();
    let inpaint = &requests
        .iter()
        .find(|(domain, _)| *domain == Domain::Inpaint)
        .unwrap()
        .1;
    assert!(inpaint.input_b64.is_none());
    assert_eq!(
        inpaint
            .inputs
            .as_ref()
            .unwrap()
            .iter()
            .map(|input| input.name.as_str())
            .collect::<Vec<_>>(),
        ["image", "mask"]
    );

    let control = &requests
        .iter()
        .find(|(domain, _)| *domain == Domain::Control)
        .unwrap()
        .1;
    assert!(control.input_b64.is_some());
    assert_eq!(control.input_content_type.as_deref(), Some("image/png"));
    assert!(control.inputs.as_deref().unwrap_or_default().is_empty());
}
