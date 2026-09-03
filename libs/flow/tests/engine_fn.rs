use makepad_flow::graph::FlowVm;
use makepad_flow::Value;

#[test]
fn flow_vm_calls_real_closure_and_keeps_it_alive() {
    let source = r#"use mod.flow.*
let add = Fn{in: {text: "" suffix: "!"} out: [@text] run: |i| {{text: i.text + i.suffix}}}
Flow{add}
"#;
    let (mut vm, _) = FlowVm::load(source, "fn.splash").unwrap();
    let outputs = vm
        .call_fn(
            "add",
            &[
                ("text".to_string(), Value::text("hello")),
                ("suffix".to_string(), Value::text("!")),
            ],
        )
        .unwrap();
    assert_eq!(outputs[0].1.as_text().unwrap(), "hello!");
}

#[test]
fn flow_vm_names_missing_declared_output() {
    let source = r#"use mod.flow.*
let bad = Fn{in: {} out: [@text] run: |i| {{json: i}}}
Flow{bad}
"#;
    let (mut vm, _) = FlowVm::load(source, "bad.splash").unwrap();
    let error = vm.call_fn("bad", &[]).unwrap_err();
    assert!(error.contains("text"), "{error}");
}

#[test]
fn flow_vm_stops_infinite_closure_at_instruction_budget() {
    let source = r#"use mod.flow.*
let forever = Fn{in: {} out: [@text] run: |i| {loop {} {text: "never"}}}
Flow{forever}
"#;
    let (mut vm, _) = FlowVm::load(source, "forever.splash").unwrap();
    let error = vm.call_fn("forever", &[]).unwrap_err();
    assert!(error.contains("limit"), "{error}");
}

#[test]
fn media_handles_pass_through_without_exposing_bytes() {
    let source = r#"use mod.flow.*
let source = Input{type: @image default: nil}
let pass = Fn{in: {image: source.image()} out: [@image, @json] run: |i| {{image: i.image, json: {digest: i.image.digest, content_type: i.image.content_type, bytes: i.image.bytes}}}}
Flow{source, pass}
"#;
    let (mut vm, _) = FlowVm::load(source, "media.splash").unwrap();
    let image = Value::media(makepad_flow::PortType::Image, "image/png", vec![1, 2, 3]);
    let outputs = vm
        .call_fn("pass", &[("image".to_string(), image.clone())])
        .unwrap();
    assert_eq!(outputs[0].1, image);
    let handle = makepad_strict_json::parse(&outputs[1].1.bytes).unwrap();
    assert_eq!(
        handle.get("digest").and_then(|value| value.as_str()),
        Some(image.digest_hex().as_str())
    );
    assert_eq!(
        handle.get("content_type").and_then(|value| value.as_str()),
        Some("image/png")
    );
    assert_eq!(handle.get("bytes").and_then(|value| value.as_i64()), Some(3));
}

#[test]
fn json_scalars_objects_and_lists_enter_the_vm_as_script_values() {
    let source = r#"use mod.flow.*
let pass = Fn{in: {json: {}, list: []} out: [@json, @list] run: |i| {{json: i.json, list: i.list}}}
Flow{pass}
"#;
    let (mut vm, _) = FlowVm::load(source, "json-values.splash").unwrap();
    let outputs = vm
        .call_fn(
            "pass",
            &[
                ("json".to_string(), Value::json(r#"{"n":2,"ok":true}"#)),
                ("list".to_string(), Value::list(r#"[1,false,"x"]"#)),
            ],
        )
        .unwrap();
    let json = makepad_strict_json::parse(&outputs[0].1.bytes).unwrap();
    assert_eq!(json.get("n").and_then(|value| value.as_i64()), Some(2));
    assert_eq!(json.get("ok").and_then(|value| value.as_bool()), Some(true));
    let list = makepad_strict_json::parse(&outputs[1].1.bytes).unwrap();
    assert!(matches!(list, makepad_strict_json::Value::Arr(values) if values.len() == 3));
}
