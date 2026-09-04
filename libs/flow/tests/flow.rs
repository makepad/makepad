use makepad_flow::graph::{evaluate, is_canonical, prelude_catalog, tool_schema, write};
use makepad_flow::{Literal, NodeInputValue, PortType};
use makepad_micro_serde::{DeJson, SerJson};
use std::collections::HashSet;

#[test]
fn design_fixture_evaluates() {
    let source = include_str!("fixtures/prompt_image.splash");
    let graph = evaluate(source, "prompt_image.splash").unwrap();
    assert_eq!(graph.nodes.len(), 5);
    assert_eq!(graph.edges.len(), 4);
    assert_eq!(graph.label, "Prompt to image");
    assert_eq!(
        graph.nodes.iter().map(|node| node.kind.as_str()).collect::<Vec<_>>(),
        ["input", "chat", "fn", "gen", "output"]
    );
    assert_eq!(graph.nodes[0].at, Some((40.0, 120.0)));
    assert_eq!(graph.tools[0].inputs, ["prompt"]);
    assert_eq!(graph.tools[0].outputs, ["picture"]);
    assert!(graph.nodes[2].fn_src.as_deref().unwrap().starts_with("|i|"));
    assert!(graph.nodes[3]
        .face_src
        .as_deref()
        .unwrap()
        .starts_with("ImageFace{"));
    let schema = tool_schema(&graph);
    assert_eq!(schema.tools[0].name, "run");
    let canonical = write(&graph);
    let graph2 = evaluate(&canonical, "written.splash").unwrap();
    assert_eq!(write(&graph2), canonical);
    assert!(is_canonical(&canonical));
}

#[test]
fn node_facing_round_trips_without_emitting_the_default() {
    let source = r#"use mod.flow.*
let left_to_right = Input{at: vec2(10, 20)}
let right_to_left = Output{at: vec2(30, 40) size: vec2(300, 180) flip: true}
Flow{left_to_right, right_to_left}
"#;
    let graph = evaluate(source, "facing.splash").unwrap();
    assert!(!graph.nodes[0].flip);
    assert!(graph.nodes[1].flip);
    let written = write(&graph);
    assert_eq!(written.matches("    flip: true\n").count(), 1);
    assert!(written.find("    size: vec2(300, 180)\n").unwrap()
        < written.find("    flip: true\n").unwrap());
    let reparsed = evaluate(&written, "facing-written.splash").unwrap();
    assert_eq!(reparsed.nodes[0].flip, graph.nodes[0].flip);
    assert_eq!(reparsed.nodes[1].flip, graph.nodes[1].flip);
    assert_eq!(write(&reparsed), written);
}

#[test]
fn errors_have_locations() {
    let cases = [
        (
            "unknown.splash",
            "use mod.flow.*\nlet x = Unknown{}\nFlow{x}",
            "variable Unknown not found",
        ),
        (
            "not_flow.splash",
            "use mod.flow.*\nlet x = Text{}\n{x}",
            "not a Flow",
        ),
        (
            "missing.splash",
            "use mod.flow.*\nlet hidden = Text{}\nlet result = Output{value: hidden.text()}\nFlow{result}",
            "node `hidden` is referenced by `result.value` but not listed in `Flow{}`",
        ),
        (
            "mismatch.splash",
            "use mod.flow.*\nlet pixels = Input{type: @image}\nlet image = Image{prompt: pixels.image()}\nFlow{pixels, image}",
            "type mismatch",
        ),
        (
            "closure.splash",
            "use mod.flow.*\nlet image = Image{prompt: || {\"no\"}}\nFlow{image}",
            "closure where",
        ),
        (
            "wrong_param.splash",
            "use mod.flow.*\nlet image = Image{width: \"wide\"}\nFlow{image}",
            "wrong type for parameter `width`",
        ),
        (
            "syntax.splash",
            "use mod.flow.*\nlet image = Image{ width: }\nFlow{image}",
            "Expected expression",
        ),
    ];
    for (file, source, expected) in cases {
        let error = evaluate(source, file).unwrap_err();
        assert_eq!(error.file, file);
        assert!(error.line > 0, "{file}: {error:?}");
        assert!(error.col > 0, "{file}: {error:?}");
        assert!(
            error.message.contains(expected),
            "{file}: expected {expected:?}, got {error:?}"
        );
    }
}

#[test]
fn reserved_in_closure_parameter_is_a_vm_error() {
    let source = "use mod.flow.*\nlet f = Fn{in: {text: \"x\"} out: [@text] run: |in| {{text: in.text}}}\nFlow{f}";
    let error = evaluate(source, "reserved_in.splash").unwrap_err();
    assert_eq!(error.file, "reserved_in.splash");
    assert!(error.message.contains("reserved"), "{error:?}");
    assert!(error.line > 0 && error.col > 0);
}

#[test]
fn cycle_is_rejected() {
    let source = r#"use mod.flow.*
let a = Fn{in: {text: "a"} out: [@text] run: |input| {{text: input.text}}}
let b = Fn{in: {text: a.text()} out: [@text] run: |input| {{text: input.text}}}
a.in.text = b.text()
Flow{a, b}
"#;
    let error = evaluate(source, "cycle.splash").unwrap_err();
    assert!(error.message.contains("cycle"), "{error:?}");
    assert!(error.line > 0 && error.col > 0);
}

#[test]
fn derived_generators_keep_base_type_and_overrides() {
    let graph = evaluate(include_str!("fixtures/derived.splash"), "derived.splash").unwrap();
    let portrait = &graph.nodes[1];
    assert_eq!(portrait.type_name, "Image");
    assert_eq!(portrait.domain.as_deref(), Some("image"));
    assert_eq!(param(portrait, "width"), &Literal::Num(768.0));
    assert_eq!(param(portrait, "height"), &Literal::Num(1024.0));
    let upscale = &graph.nodes[2];
    assert_eq!(upscale.type_name, "Upscale");
    assert_eq!(upscale.domain.as_deref(), Some("upscale"));
    assert_eq!(upscale.outputs[0].ty, PortType::Image);
    round_trip(&graph);
}

#[test]
fn http_ask_and_text_have_declared_ports_and_params() {
    let graph = evaluate(
        include_str!("fixtures/http_ask_text.splash"),
        "http_ask_text.splash",
    )
    .unwrap();
    assert_eq!(graph.trigger, "input");
    assert_eq!(graph.concurrency, 2);
    assert!(graph.autostart);
    assert_eq!(graph.nodes[0].type_name, "Text");
    let http = &graph.nodes[1];
    assert_eq!(http.outputs[0].name, "value");
    assert_eq!(http.outputs[0].ty, PortType::Json);
    assert_eq!(http.outputs[1].ty, PortType::Json);
    assert_eq!(param(http, "method"), &Literal::Id("get".to_string()));
    let ask = &graph.nodes[2];
    assert_eq!(ask.outputs[0].name, "text");
    assert_eq!(param(ask, "timeout"), &Literal::Num(30.0));
    round_trip(&graph);
}

#[test]
fn generic_gen_carries_custom_params_and_ports() {
    let graph = evaluate(include_str!("fixtures/gen.splash"), "gen.splash").unwrap();
    let node = &graph.nodes[0];
    assert_eq!(node.type_name, "Gen");
    assert_eq!(node.domain.as_deref(), Some("video"));
    assert_eq!(param(node, "quality"), &Literal::Num(4.0));
    assert_eq!(node.inputs[0].port, "prompt");
    assert_eq!(node.inputs[0].ty, PortType::Text);
    assert_eq!(node.outputs[0].name, "video");
    assert_eq!(node.outputs[0].ty, PortType::Video);
    round_trip(&graph);
}

#[test]
fn declared_gen_port_signatures_survive_writer_round_trip() {
    let source = r#"use mod.flow.*
let Misleading = Image{
    ports: {
        in: {picture: @text, caption: @image}
        out: {picture: @audio, caption: @video}
    }
}
let generated = Misleading{}
let result = Output{type: @audio value: generated.out(@picture)}
Flow{generated, result}
"#;
    let graph = evaluate(source, "misleading-ports.splash").unwrap();
    let before = graph.nodes.iter().find(|node| node.id == "generated").unwrap();
    assert_eq!(before.type_name, "Image");
    assert_eq!(
        before
            .inputs
            .iter()
            .map(|port| (port.port.as_str(), port.ty))
            .collect::<Vec<_>>(),
        [("picture", PortType::Text), ("caption", PortType::Image)]
    );
    assert_eq!(
        before
            .outputs
            .iter()
            .map(|port| (port.name.as_str(), port.ty))
            .collect::<Vec<_>>(),
        [("picture", PortType::Audio), ("caption", PortType::Video)]
    );
    let written = write(&graph);
    let rewritten = evaluate(&written, "misleading-ports-written.splash").unwrap();
    let after = rewritten
        .nodes
        .iter()
        .find(|node| node.id == "generated")
        .unwrap();
    assert_eq!(
        before
            .inputs
            .iter()
            .map(|port| (port.port.as_str(), port.ty))
            .collect::<Vec<_>>(),
        after
            .inputs
            .iter()
            .map(|port| (port.port.as_str(), port.ty))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        before
            .outputs
            .iter()
            .map(|port| (port.name.as_str(), port.ty))
            .collect::<Vec<_>>(),
        after
            .outputs
            .iter()
            .map(|port| (port.name.as_str(), port.ty))
            .collect::<Vec<_>>()
    );
}

#[test]
fn named_tool_prunes_the_unrelated_branch() {
    let graph = evaluate(include_str!("fixtures/tools.splash"), "tools.splash").unwrap();
    assert_eq!(graph.tools[0].name, "run");
    assert_eq!(graph.tools[0].nodes.len(), 5);
    let paint = &graph.tools[1];
    assert_eq!(paint.name, "paint");
    assert_eq!(paint.inputs, ["prompt"]);
    assert_eq!(paint.outputs, ["picture"]);
    assert_eq!(paint.nodes, ["prompt", "image", "picture"]);
    let schema = tool_schema(&graph);
    assert_eq!(schema.tools.len(), 2);
    assert_eq!(schema.tools[1].result_fields, [("picture".to_string(), PortType::Image)]);
    let parsed = makepad_strict_json::parse(schema.tools[1].parameters.as_bytes()).unwrap();
    assert_eq!(parsed.get("type").and_then(|value| value.as_str()), Some("object"));
    round_trip(&graph);
}

#[test]
fn build_time_loop_is_flattened_by_writer() {
    let source = include_str!("fixtures/loop.splash");
    let graph = evaluate(source, "loop.splash").unwrap();
    assert_eq!(graph.nodes.len(), 3);
    assert!(!is_canonical(source));
    let flat = write(&graph);
    assert!(!flat.contains("for "));
    assert!(is_canonical(&flat));
    round_trip(&graph);
}

#[test]
fn comments_make_an_otherwise_flat_file_custom() {
    let source = include_str!("fixtures/custom_comment.splash");
    assert!(!is_canonical(source));
    let written = write(&evaluate(source, "custom_comment.splash").unwrap());
    assert!(is_canonical(&written));
}

#[test]
fn every_fixture_writer_round_trips() {
    for (name, source) in [
        ("prompt_image.splash", include_str!("fixtures/prompt_image.splash")),
        ("derived.splash", include_str!("fixtures/derived.splash")),
        ("http_ask_text.splash", include_str!("fixtures/http_ask_text.splash")),
        ("tools.splash", include_str!("fixtures/tools.splash")),
        ("gen.splash", include_str!("fixtures/gen.splash")),
        ("loop.splash", include_str!("fixtures/loop.splash")),
        ("custom_comment.splash", include_str!("fixtures/custom_comment.splash")),
    ] {
        let graph = evaluate(source, name).unwrap();
        round_trip(&graph);
    }
}

fn param<'a>(node: &'a makepad_flow::Node, name: &str) -> &'a Literal {
    node.params
        .iter()
        .find_map(|(key, value)| (key == name).then_some(value))
        .unwrap()
}

fn round_trip(graph: &makepad_flow::Graph) {
    let first = write(graph);
    let second = write(&evaluate(&first, "round_trip.splash").unwrap());
    assert_eq!(second, first);
    assert!(is_canonical(&first));
}

#[test]
fn edge_values_are_exposed_in_inputs() {
    let graph = evaluate(include_str!("fixtures/prompt_image.splash"), "ports.splash").unwrap();
    let prompt = graph.nodes[1]
        .inputs
        .iter()
        .find(|input| input.port == "prompt")
        .unwrap();
    assert!(matches!(prompt.value, NodeInputValue::Edge(_)));
}

#[test]
fn graph_wire_json_round_trips() {
    let graph = evaluate(include_str!("fixtures/prompt_image.splash"), "wire.splash").unwrap();
    let json = graph.serialize_json();
    assert_eq!(makepad_flow::Graph::deserialize_json(&json).unwrap(), graph);
}

#[test]
fn inpaint_mask_port_type_checks_against_declared_image_type() {
    let source = r#"use mod.flow.*
let image = Input{type: @image}
let mask = Input{type: @image}
let inpaint = Inpaint{prompt: "fill" image: image.image() mask: mask.image()}
let picture = Output{type: @image value: inpaint.out(@image)}
Flow{image, mask, inpaint, picture}
"#;
    let graph = evaluate(source, "inpaint_ok.splash").unwrap();
    let node = graph.nodes.iter().find(|node| node.id == "inpaint").unwrap();
    let mask_input = node.inputs.iter().find(|input| input.port == "mask").unwrap();
    assert_eq!(mask_input.ty, PortType::Image);
    let image_input = node.inputs.iter().find(|input| input.port == "image").unwrap();
    assert_eq!(image_input.ty, PortType::Image);
    let prompt_input = node.inputs.iter().find(|input| input.port == "prompt").unwrap();
    assert_eq!(prompt_input.ty, PortType::Text);
    round_trip(&graph);
}

#[test]
fn wiring_text_into_inpaint_mask_is_a_located_error() {
    let source = r#"use mod.flow.*
let image = Input{type: @image}
let words = Input{type: @text}
let inpaint = Inpaint{prompt: "fill" image: image.image() mask: words.text()}
Flow{image, words, inpaint}
"#;
    let error = evaluate(source, "inpaint_mismatch.splash").unwrap_err();
    assert_eq!(error.file, "inpaint_mismatch.splash");
    assert!(error.line > 0 && error.col > 0, "{error:?}");
    assert!(error.message.contains("type mismatch"), "{error:?}");
    assert!(error.message.contains("mask"), "{error:?}");
}

#[test]
fn old_array_ports_form_still_evaluates_with_a_deprecation_warning() {
    let source = r#"use mod.flow.*
let LegacyGen = Gen{domain: "video" ports: {in: [@prompt] out: [@video]}}
let clip = LegacyGen{prompt: "waves"}
let result = Output{type: @video value: clip.out(@video)}
Flow{clip, result}
"#;
    let graph = evaluate(source, "legacy_ports.splash").unwrap();
    let node = graph.nodes.iter().find(|node| node.id == "clip").unwrap();
    // The array form still infers types by name, exactly as before.
    assert_eq!(node.inputs[0].ty, PortType::Text);
    assert_eq!(node.outputs[0].ty, PortType::Video);
    assert!(
        graph
            .warnings
            .iter()
            .any(|warning| warning.contains("deprecated") && warning.contains("clip")),
        "{:?}",
        graph.warnings
    );
    round_trip(&graph);
}

#[test]
fn empty_legacy_port_arrays_still_warn() {
    let source = r#"use mod.flow.*
let Legacy = Gen{domain: "image" ports: {in: [] out: []}}
let empty = Legacy{}
Flow{empty}
"#;
    let graph = evaluate(source, "empty-legacy-ports.splash").unwrap();
    assert_eq!(graph.warnings.len(), 2, "{:?}", graph.warnings);
    assert!(graph.warnings.iter().all(|warning| warning.contains("deprecated")));
}

#[test]
fn prelude_catalog_walks_every_recipe_type_including_mesh() {
    let catalog = prelude_catalog().unwrap();
    let mesh = catalog
        .iter()
        .find(|node| node.type_name == "Mesh")
        .expect("Mesh reaches the catalog");
    assert_eq!(mesh.kind, "gen");
    assert_eq!(mesh.domain.as_deref(), Some("mesh"));
    assert!(mesh
        .ports
        ._in
        .iter()
        .any(|port| port.name == "prompt" && port.ty == PortType::Text));
    assert!(mesh
        .ports
        ._in
        .iter()
        .any(|port| port.name == "image" && port.ty == PortType::Image));
    assert!(mesh
        .ports
        .out
        .iter()
        .any(|port| port.name == "mesh" && port.ty == PortType::Mesh));
    assert!(mesh.params.iter().any(|param| param.name == "remesh_resolution"));
    assert!(!mesh.doc.is_empty());
    // The catalog is no longer the old hard-coded ten-name list.
    for type_name in ["Inpaint", "Video", "Music", "Paint", "Control", "ImageEdit"] {
        assert!(
            catalog.iter().any(|node| node.type_name == type_name),
            "catalog missing `{type_name}`"
        );
    }
    let music = catalog.iter().find(|node| node.type_name == "Music").unwrap();
    assert!(music
        .ports
        ._in
        .iter()
        .any(|port| port.name == "lyrics" && port.ty == PortType::Text));
    let paint = catalog.iter().find(|node| node.type_name == "Paint").unwrap();
    assert!(paint
        .ports
        ._in
        .iter()
        .any(|port| port.name == "reference_image" && port.ty == PortType::Image));
    let publish = catalog
        .iter()
        .find(|node| node.type_name == "Publish")
        .unwrap();
    assert_eq!(publish.kind, "publish");
    assert!(publish
        .ports
        ._in
        .iter()
        .any(|port| port.name == "value"));
    assert!(publish
        .ports
        .out
        .iter()
        .any(|port| port.name == "asset" && port.ty == PortType::Json));
    assert!(publish.params.iter().all(|param| !param.doc.is_empty()));
    assert!(!publish.doc.is_empty());
}

#[test]
fn prelude_catalog_has_the_exact_public_node_type_set() {
    let catalog = prelude_catalog().unwrap();
    let names: Vec<_> = catalog.iter().map(|node| node.type_name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Annotate",
            "Ask",
            "Control",
            "Depth",
            "Fn",
            "Gen",
            "Http",
            "Image",
            "ImageEdit",
            "Inpaint",
            "Input",
            "Llm",
            "Matte",
            "Mesh",
            "Motion",
            "Music",
            "Output",
            "Paint",
            "Publish",
            "Rig",
            "Sfx",
            "Speech",
            "Splat",
            "Text",
            "Upscale",
            "Video",
            "VideoEnhance",
            "Vision",
            "World",
        ]
    );
    assert_eq!(names.iter().copied().collect::<HashSet<_>>().len(), names.len());
    for excluded in [
        "Node",
        "Flow",
        "NodeFace",
        "InputFace",
        "TextFace",
        "OutputFace",
        "PublishFace",
        "LlmFace",
        "FnFace",
        "HttpFace",
        "AskFace",
        "GenFace",
        "ImageFace",
        "UpscaleFace",
    ] {
        assert!(!names.contains(&excluded), "catalog contains `{excluded}`");
    }
}

#[test]
fn prelude_module_is_frozen() {
    let error = evaluate(
        "use mod.flow.*\nmod.flow.Image = {}\nFlow{}",
        "frozen.splash",
    )
    .unwrap_err();
    assert!(error.message.contains("already exists"), "{error:?}");
}
