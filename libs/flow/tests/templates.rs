use makepad_flow::graph::{evaluate, is_canonical, write};
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;

#[test]
fn every_recipe_template_evaluates_and_round_trips() {
    let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("recipes/templates");
    let mut templates: Vec<_> = fs::read_dir(&template_dir)
        .unwrap_or_else(|error| panic!("cannot read {}: {error}", template_dir.display()))
        .map(|entry| entry.expect("cannot read template directory entry").path())
        .filter(|path| path.extension().is_some_and(|extension| extension == "splash"))
        .collect();
    templates.sort();
    assert!(!templates.is_empty(), "no recipe templates found");

    for path in templates {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("template file name is not UTF-8");
        println!("checking recipe template: {name}");
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("{name}: cannot read template: {error}"));
        let graph = evaluate(&source, name)
            .unwrap_or_else(|error| panic!("{name}: evaluation failed: {error}"));

        if source.contains("let prompt = Input") {
            assert!(
                graph
                    .nodes
                    .iter()
                    .any(|node| node.id == "prompt" && node.kind == "input"),
                "{name}: expected an Input node named `prompt`"
            );
        }

        let mut reachable: HashSet<&str> = graph
            .nodes
            .iter()
            .filter(|node| node.kind == "input")
            .map(|node| node.id.as_str())
            .collect();
        loop {
            let before = reachable.len();
            for edge in &graph.edges {
                if reachable.contains(edge.from_node.as_str()) {
                    reachable.insert(edge.to_node.as_str());
                }
            }
            if reachable.len() == before {
                break;
            }
        }
        let outputs: Vec<_> = graph
            .nodes
            .iter()
            .filter(|node| node.kind == "output")
            .collect();
        assert!(!outputs.is_empty(), "{name}: template has no Output node");
        for output in outputs {
            assert!(
                reachable.contains(output.id.as_str()),
                "{name}: Output `{}` is not reachable from an Input",
                output.id
            );
        }

        let written = write(&graph);
        let rewritten = write(
            &evaluate(&written, name)
                .unwrap_or_else(|error| panic!("{name}: written form failed: {error}")),
        );
        assert_eq!(rewritten, written, "{name}: writer did not round-trip");
        assert!(is_canonical(&written), "{name}: written form is not canonical");
    }
}

#[test]
fn shipped_templates_are_warning_free_and_keep_typed_multi_input_edges() {
    let template_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("recipes/templates");
    for entry in fs::read_dir(&template_dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|extension| extension != "splash") {
            continue;
        }
        let name = path.file_name().unwrap().to_str().unwrap();
        let source = fs::read_to_string(&path).unwrap();
        let graph = evaluate(&source, name).unwrap();
        assert!(graph.warnings.is_empty(), "{name}: {:?}", graph.warnings);
    }

    let dream = evaluate(include_str!("../recipes/templates/dream.splash"), "dream.splash").unwrap();
    let keyframes: HashSet<_> = dream
        .edges
        .iter()
        .filter(|edge| edge.from_node == "image" && edge.to_node == "video")
        .map(|edge| (edge.from_port.as_str(), edge.to_port.as_str()))
        .collect();
    assert_eq!(keyframes, HashSet::from([("image", "image"), ("image", "last_frame")]));

    let music = evaluate(include_str!("../recipes/templates/music.splash"), "music.splash").unwrap();
    assert!(music.edges.iter().any(|edge| {
        edge.from_node == "lyrics"
            && edge.from_port == "text"
            && edge.to_node == "music"
            && edge.to_port == "lyrics"
    }));

    let inpaint = evaluate(include_str!("../recipes/templates/inpaint.splash"), "inpaint.splash")
        .unwrap();
    let image_edges: HashSet<_> = inpaint
        .edges
        .iter()
        .filter(|edge| edge.to_node == "inpaint" && matches!(edge.to_port.as_str(), "image" | "mask"))
        .map(|edge| (edge.from_node.as_str(), edge.to_port.as_str()))
        .collect();
    assert_eq!(image_edges, HashSet::from([("image", "image"), ("mask", "mask")]));
    let node = inpaint.nodes.iter().find(|node| node.id == "inpaint").unwrap();
    assert!(node
        .inputs
        .iter()
        .filter(|input| matches!(input.port.as_str(), "image" | "mask"))
        .all(|input| input.ty == makepad_flow::PortType::Image));
}
