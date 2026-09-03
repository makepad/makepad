use makepad_flow::graph::{evaluate, is_canonical, tool_schema, write, FlowVm};
use makepad_flow::{Graph, Literal};
use std::fs;
use std::path::PathBuf;

fn error(source: &str, file: &str) -> makepad_flow::EvalError {
    let error = evaluate(source, file).expect_err("source unexpectedly evaluated");
    assert_eq!(error.file, file);
    assert!(error.line > 0 && error.col > 0, "unlocated error: {error:?}");
    error
}

fn param<'a>(graph: &'a Graph, node: &str, name: &str) -> &'a Literal {
    graph
        .nodes
        .iter()
        .find(|candidate| candidate.id == node)
        .and_then(|node| {
            node.params
                .iter()
                .find_map(|(key, value)| (key == name).then_some(value))
        })
        .unwrap_or_else(|| panic!("missing {node}.{name}"))
}

fn without_revision_or_locations(mut graph: Graph) -> Graph {
    graph.revision = 0;
    for node in &mut graph.nodes {
        node.loc = Default::default();
    }
    graph
}

#[test]
fn every_template_round_trips_to_the_same_graph_and_edge_count() {
    let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("recipes/templates");
    let mut templates: Vec<_> = fs::read_dir(&template_dir)
        .expect("read recipe template directory")
        .map(|entry| entry.expect("read recipe template entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "splash"))
        .collect();
    templates.sort();
    assert!(!templates.is_empty());

    for path in templates {
        let name = path.file_name().unwrap().to_str().unwrap();
        let source = fs::read_to_string(&path).unwrap();
        let original = evaluate(&source, name).unwrap_or_else(|error| panic!("{name}: {error}"));
        let edge_count = original.edges.len();
        let written = write(&original);
        assert!(is_canonical(&written), "{name}: writer output is not canonical");
        let reparsed = evaluate(&written, name)
            .unwrap_or_else(|error| panic!("{name}: written form failed: {error}"));
        assert_eq!(reparsed.edges.len(), edge_count, "{name}: edge count changed");
        let original = without_revision_or_locations(original);
        let mut reparsed = without_revision_or_locations(reparsed);
        for node in &original.nodes {
            if node.at.is_none() {
                reparsed.nodes.iter_mut().find(|other| other.id == node.id).unwrap().at = None;
            }
        }
        assert_eq!(
            reparsed,
            original,
            "{name}: Graph changed"
        );
    }
}

#[test]
fn node_identity_rejects_aliases_and_references_to_omitted_nodes() {
    let duplicate = error(
        "use mod.flow.*\nlet a = Input{}\nFlow{a, b: a}\n",
        "duplicate.splash",
    );
    assert!(duplicate.message.contains("same node"), "{duplicate:?}");

    let omitted = error(
        "use mod.flow.*\nlet hidden = Input{}\nlet image = Image{prompt: hidden.text()}\nFlow{image}\n",
        "omitted.splash",
    );
    assert_eq!(
        omitted.message,
        "node `hidden` is referenced by `image.prompt` but not listed in `Flow{}`"
    );
}

#[test]
fn port_references_reject_self_cycles_long_cycles_and_node_objects() {
    let long_cycle = error(
        r#"use mod.flow.*
let a = Fn{in: {text: "a"} out: [@text] run: |i| {{text: i.text}}}
let b = Fn{in: {text: a.text()} out: [@text] run: |i| {{text: i.text}}}
let c = Fn{in: {text: b.text()} out: [@text] run: |i| {{text: i.text}}}
a.in.text = c.text()
Flow{a, b, c}
"#,
        "long-cycle.splash",
    );
    assert!(long_cycle.message.contains("cycle"), "{long_cycle:?}");

    let object = error(
        "use mod.flow.*\nlet expand = Llm{}\nlet image = Image{\n    prompt: expand\n}\nFlow{expand, image}\n",
        "object-not-port.splash",
    );
    assert_eq!(object.line, 4, "{object:?}");
    assert!(
        object.message.contains("expected a port reference"),
        "{object:?}"
    );

    let self_cycle = error(
        "use mod.flow.*\nlet image = Image{}\nimage.prompt = image.image()\nFlow{image}\n",
        "self-cycle.splash",
    );
    assert!(self_cycle.message.contains("cycle"), "{self_cycle:?}");
}

#[test]
fn numeric_literals_are_typed_but_documented_ranges_are_hints() {
    let graph = evaluate(
        "use mod.flow.*\nlet image = Image{width: 100000}\nFlow{image}\n",
        "width-range.splash",
    )
    .expect("inspector ranges are documentation, not validation");
    assert_eq!(param(&graph, "image", "width"), &Literal::Num(100_000.0));

    let wrong = error(
        "use mod.flow.*\nlet image = Image{\n    width: \"1024\"\n}\nFlow{image}\n",
        "width-string.splash",
    );
    assert_eq!(wrong.line, 3, "{wrong:?}");
    assert!(wrong.message.contains("width"), "{wrong:?}");
}

#[test]
fn fn_requires_a_closure_and_declared_input_and_output_ports() {
    for (name, source) in [
        (
            "missing-run.splash",
            "use mod.flow.*\nlet f = Fn{in: {} out: [@text]}\nFlow{f}\n",
        ),
        (
            "number-run.splash",
            "use mod.flow.*\nlet f = Fn{in: {} out: [@text] run: 42}\nFlow{f}\n",
        ),
    ] {
        let error = error(source, name);
        assert!(error.message.contains("run"), "{error:?}");
        assert!(error.message.contains("closure"), "{error:?}");
    }

    let empty_out =
        "use mod.flow.*\nlet f = Fn{in: {} out: [] run: |i| {{text: \"extra\"}}}\nFlow{f}\n";
    let (mut vm, _) = FlowVm::load(empty_out, "empty-out.splash").unwrap();
    assert!(vm.call_fn("f", &[]).unwrap().is_empty());

    let reserved = error(
        "use mod.flow.*\nlet f = Fn{in: {} out: [@text] run: |in| {{text: \"x\"}}}\nFlow{f}\n",
        "reserved-in.splash",
    );
    assert!(reserved.message.contains("reserved"), "{reserved:?}");

    let undeclared = error(
        "use mod.flow.*\nlet source = Input{}\nlet f = Fn{in: {} out: [@text] run: |i| {{text: \"x\"}} text: source.text()}\nFlow{source, f}\n",
        "undeclared-fn-input.splash",
    );
    assert!(undeclared.message.contains("undeclared"), "{undeclared:?}");
}

#[test]
fn http_validates_method_output_and_url_edge_types() {
    let output = error(
        "use mod.flow.*\nlet request = Http{out: @mesh url: \"https://example.test\"}\nFlow{request}\n",
        "http-output.splash",
    );
    assert!(output.message.contains("out"), "{output:?}");

    evaluate(
        "use mod.flow.*\nlet url = Input{}\nlet request = Http{url: url.text()}\nFlow{url, request}\n",
        "http-text-url.splash",
    )
    .unwrap();
    let mismatch = error(
        "use mod.flow.*\nlet picture = Input{type: @image}\nlet request = Http{url: picture.image()}\nFlow{picture, request}\n",
        "http-image-url.splash",
    );
    assert!(mismatch.message.contains("type mismatch"), "{mismatch:?}");

    let method = error(
        "use mod.flow.*\nlet request = Http{method: @patch url: \"https://example.test\"}\nFlow{request}\n",
        "http-method.splash",
    );
    for allowed in ["get", "post", "put", "delete"] {
        assert!(method.message.contains(allowed), "{method:?}");
    }
}

#[test]
fn ask_validates_options_timeout_and_answer_type() {
    for (name, field) in [
        ("ask-options.splash", "options: \"yes\""),
        ("ask-timeout.splash", "timeout: -1"),
        ("ask-bytes.splash", "type: @bytes"),
    ] {
        let source = format!("use mod.flow.*\nlet ask = Ask{{{field}}}\nFlow{{ask}}\n");
        let error = error(&source, name);
        assert!(error.message.contains(field.split(':').next().unwrap()), "{error:?}");
    }
}

#[test]
fn build_time_loops_flatten_generated_nodes_and_if_values() {
    let source = r#"use mod.flow.*
let flow = Flow{}
for i in 0..20 {
    flow["image_" + i] = Image{prompt: "generated"}
}
flow
"#;
    let graph = evaluate(source, "generated-loop.splash").unwrap();
    assert_eq!(graph.nodes.len(), 20);
    for index in 0..20 {
        assert!(graph.nodes.iter().any(|node| node.id == format!("image_{index}")));
    }
    let flat = write(&graph);
    assert!(!flat.contains("for "));
    assert!(is_canonical(&flat));

    let chosen = evaluate(
        r#"use mod.flow.*
let instruction = if true { "chosen system" } else { "wrong system" }
let llm = Llm{system: instruction}
Flow{llm}
"#,
        "build-if.splash",
    )
    .unwrap();
    assert_eq!(
        param(&chosen, "llm", "system"),
        &Literal::Str("chosen system".to_string())
    );
}

#[test]
fn unicode_and_large_sources_round_trip_or_fail_with_a_budget_error() {
    let text = format!("🧪{}", "é".repeat(50_000));
    assert!(text.len() >= 100_000);
    let source = format!(
        "use mod.flow.*\nlet llm = Llm{{system: {}}}\nFlow{{llm}}\n",
        makepad_strict_json::Value::Str(text.clone()).to_json()
    );
    let graph = evaluate(&source, "unicode-100k.splash").unwrap();
    assert_eq!(param(&graph, "llm", "system"), &Literal::Str(text.clone()));
    let reparsed = evaluate(&write(&graph), "unicode-written.splash").unwrap();
    assert_eq!(param(&reparsed, "llm", "system"), &Literal::Str(text));

    let huge = format!(
        "use mod.flow.*\nlet llm = Llm{{system: \"{}\"}}\nFlow{{llm}}\n",
        "x".repeat(205 * 1024)
    );
    let error = error(&huge, "source-200k.splash");
    assert!(
        error.message.contains("budget") || error.message.contains("limit"),
        "{error:?}"
    );
}

#[test]
fn face_source_spans_include_nested_braces_strings_and_closing_braces() {
    let dropdown = r#"DropDown{ labels: ["a"] bind: styled.style }"#;
    let nested = r#"NestedFace{ panel: {label: "a } brace" inner: {open: true}} }"#;
    let flow_face = r#"View{ body: {label: "whole flow"} }"#;
    let source = format!(
        r#"use mod.flow.*
let View = {{}}
let DropDown = {{}}
let NestedFace = {{}}

let styled = Fn{{in: {{style: "a"}} out: [@text] run: |i| {{{{text: i.style}}}}}}

let first = Image{{prompt: "x" ui: {dropdown}}}

let second = Image{{width: 512 ui: {nested}}}

Flow{{
    styled, first, second
    ui: {flow_face}
}}
"#
    );
    let graph = evaluate(&source, "faces.splash").unwrap();
    assert_eq!(
        graph.nodes.iter().find(|node| node.id == "second").unwrap().face_src.as_deref(),
        Some(nested)
    );
    assert_eq!(graph.flow_ui_src.as_deref(), Some(flow_face));
    assert_eq!(
        graph.nodes.iter().find(|node| node.id == "first").unwrap().face_src.as_deref(),
        Some(dropdown)
    );
}

#[test]
fn docs_on_lets_attach_to_nodes_but_parameter_docs_do_not() {
    let graph = evaluate(
        r#"use mod.flow.*
/** a doc */
let documented = Image{}
let field_only = Image{
    /** width doc */ width: 512
}
Flow{documented, field_only}
"#,
        "docs.splash",
    )
    .unwrap();
    assert_eq!(
        graph.nodes.iter().find(|node| node.id == "documented").unwrap().doc.as_deref(),
        Some("a doc")
    );
    assert_eq!(
        graph.nodes.iter().find(|node| node.id == "field_only").unwrap().doc,
        None
    );
}

#[test]
fn tool_schema_uses_exact_dependencies_and_rejects_bad_declarations() {
    let graph = evaluate(
        r#"use mod.flow.*
let prompt = Input{}
let unused = Input{}
let image = Image{prompt: prompt.text()}
let picture = Output{type: @image value: image.image()}
Flow{tools: {paint: {in: [prompt] out: [picture]}} prompt, unused, image, picture}
"#,
        "paint-tool.splash",
    )
    .unwrap();
    let paint = graph.tools.iter().find(|tool| tool.name == "paint").unwrap();
    assert_eq!(paint.nodes, ["prompt", "image", "picture"]);
    let schema = tool_schema(&graph);
    assert_eq!(schema.tools.iter().find(|tool| tool.name == "paint").unwrap().name, "paint");

    let non_output = error(
        "use mod.flow.*\nlet prompt = Input{}\nFlow{tools: {paint: {in: [prompt] out: [prompt]}} prompt}\n",
        "tool-non-output.splash",
    );
    assert!(non_output.message.contains("Output") || non_output.message.contains("output"));

    let reserved = error(
        "use mod.flow.*\nlet prompt = Input{}\nFlow{tools: {run: {in: [prompt] out: []}} prompt}\n",
        "tool-run.splash",
    );
    assert!(reserved.message.contains("reserved"), "{reserved:?}");
}
