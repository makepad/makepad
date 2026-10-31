//! What a fired-up preset runs: the working graph with every result routed
//! into the result store. Each `Publish` keeps its own settings and gains the
//! tags that put its result where the host looks for it ([`crate::host::Place`])
//! plus the preset it came from; a media `Output` that no
//! `Publish` reads gets one beside it. Pure graph edits: the worker writes
//! the result to Splash (`write_with_registry`) and the engine evaluates it.

use super::presets::{publish_points, tag, text_param, Destination};
use makepad_flow::{Edge, EdgeRef, Graph, Literal, Node, NodeInput, NodeInputValue, Port, PortType};

/// Who made a run, for the published assets' titles and tags.
#[derive(Clone, Debug, Default)]
pub struct Provenance {
    /// The preset key (`bundled:music`, `studio:dj-track`, `saved:<id>`).
    pub preset: String,
    pub label: String,
    /// The first text input, for titles.
    pub prompt: String,
    /// More tags for every published asset (valid store labels).
    pub tags: Vec<String>,
}

/// The artist music and voice lists show for generated audio.
pub const ARTIST: &str = "Generated · FLOW";

/// The tag every studio result carries, so the studio can list its own.
pub const STUDIO_TAG: &str = "flow-studio";

pub fn prepare(graph: &Graph, provenance: &Provenance) -> Graph {
    let mut next = graph.clone();
    let title = title_for(provenance);
    for point in publish_points(graph) {
        let destination = Destination::of(&point.ty, point.domain.as_deref());
        // The stems of one song share its title; each says which stem it is.
        let title = match destination {
            Destination::Stem => format!("{title} · {}", point.from_port),
            _ => title.clone(),
        };
        let mut tags: Vec<String> = vec!["flow".into(), STUDIO_TAG.into(), tag("preset", &provenance.preset)];
        tags.extend(destination.tags().iter().map(|tag| tag.to_string()));
        tags.extend(provenance.tags.iter().cloned());
        if point.existing {
            let Some(node) = next.nodes.iter_mut().find(|node| node.id == point.node) else { continue };
            let mut merged: Vec<String> = match node.params.iter().find(|(name, _)| name == "tags") {
                Some((_, Literal::Arr(values))) => values.iter().filter_map(|value| match value {
                    Literal::Str(text) | Literal::Id(text) => Some(text.clone()),
                    _ => None,
                }).collect(),
                _ => Vec::new(),
            };
            for tag in tags {
                if !merged.contains(&tag) {
                    merged.push(tag);
                }
            }
            set(node, "tags", Literal::Arr(merged.into_iter().map(Literal::Str).collect()));
            if text_param(node, "title").is_none_or(|text| text.trim().is_empty()) {
                set(node, "title", Literal::Str(title.clone()));
            }
            if matches!(destination, Destination::Music | Destination::Voice)
                && text_param(node, "artist").is_none_or(|text| text.trim().is_empty()) {
                set(node, "artist", Literal::Str(ARTIST.into()));
            }
        } else {
            let id = fresh_id(&next, "published");
            let at = next.nodes.iter().find(|node| node.id == point.node).and_then(|node| node.at).map(|(x, y)| (x, y + 150.0));
            let artist = matches!(destination, Destination::Music | Destination::Voice).then_some(ARTIST);
            next.nodes.push(publish_node(&id, &point.from_node, &point.from_port, point.ty.clone(), &title, tags, artist, at));
            next.edges.push(Edge { from_node: point.from_node.clone(), from_port: point.from_port.clone(), to_node: id, to_port: "value".into(), conn: None });
        }
    }
    next
}

/// `<preset> · <the prompt's first words>`.
pub fn title_for(provenance: &Provenance) -> String {
    let prompt = provenance.prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if prompt.is_empty() {
        return provenance.label.clone();
    }
    let mut excerpt: String = prompt.chars().take(56).collect();
    if prompt.chars().count() > 56 {
        excerpt = excerpt.trim_end().to_string() + "…";
    }
    format!("{} · {excerpt}", provenance.label)
}

fn set(node: &mut Node, key: &str, value: Literal) {
    match node.params.iter_mut().find(|(name, _)| name == key) {
        Some((_, slot)) => *slot = value,
        None => node.params.push((key.to_string(), value)),
    }
}

fn fresh_id(graph: &Graph, base: &str) -> String {
    if !graph.nodes.iter().any(|node| node.id == base) {
        return base.to_string();
    }
    (2..).map(|n| format!("{base}_{n}")).find(|id| !graph.nodes.iter().any(|node| &node.id == id)).unwrap()
}

fn publish_node(id: &str, from_node: &str, from_port: &str, ty: PortType, title: &str, tags: Vec<String>, artist: Option<&str>, at: Option<(f64, f64)>) -> Node {
    Node {
        id: id.to_string(),
        kind: "publish".into(),
        type_name: "Publish".into(),
        params: vec![
            ("title".into(), Literal::Str(title.to_string())),
            ("namespace".into(), Literal::Str("flows".into())),
            ("tags".into(), Literal::Arr(tags.into_iter().map(Literal::Str).collect())),
            ("description".into(), Literal::Str(String::new())),
            ("alias".into(), Literal::Str(String::new())),
        ].into_iter().chain(artist.map(|artist| ("artist".to_string(), Literal::Str(artist.to_string())))).collect(),
        inputs: vec![NodeInput {
            port: "value".into(),
            ty,
            many: None,
            value: NodeInputValue::Edge(EdgeRef { from_node: from_node.to_string(), from_port: from_port.to_string() }),
        }],
        outputs: vec![Port { name: "asset".into(), ty: PortType::Json, many: None }],
        at,
        size: None,
        flip: false,
        loc: Default::default(),
        fn_src: None,
        face_src: None,
        on_fail: "fail".into(),
        label: None,
        domain: None,
        doc: None,
    }
}

/// Bake the run's settings into a flow source's design values, for saving
/// a customised preset: the same graph, its inputs' values set.
pub fn with_input_values(graph: &Graph, values: &[(String, String)]) -> Graph {
    let mut next = graph.clone();
    for (node_id, value) in values {
        if let Some(node) = next.nodes.iter_mut().find(|node| &node.id == node_id && node.kind == "input") {
            let text = node.outputs.first().is_none_or(|port| port.ty == PortType::Text);
            let literal = if text { Literal::Str(value.clone()) } else {
                super::presets::json_literal(value).unwrap_or_else(|_| Literal::Str(value.clone()))
            };
            set(node, "value", literal);
            node.params.retain(|(name, _)| name != "default");
        }
    }
    next
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::presets::{sources, Preset};

    fn preset(key: &str) -> Preset {
        let (key, group, template_group, source, file) = sources().into_iter().find(|entry| entry.0 == key).unwrap();
        Preset::describe(key, group, template_group, source, file).unwrap()
    }

    fn provenance(key: &str) -> Provenance {
        Provenance { preset: key.into(), label: "Music".into(), prompt: "slow dub techno".into(), tags: Vec::new() }
    }

    fn tags_of(node: &Node) -> Vec<String> {
        match node.params.iter().find(|(name, _)| name == "tags") {
            Some((_, Literal::Arr(values))) => values.iter().filter_map(|value| match value {
                Literal::Str(text) => Some(text.clone()),
                _ => None,
            }).collect(),
            _ => Vec::new(),
        }
    }

    #[test]
    fn an_output_gets_a_publish_that_survives_the_writer() {
        let music = preset("bundled:music");
        let prepared = prepare(&music.graph, &provenance("bundled:music"));
        let source = makepad_flow::graph::write(&prepared).expect("prepared graph writes");
        let back = makepad_flow::graph::evaluate(&source, "prepared.splash").expect("written graph evaluates");
        let publish = back.nodes.iter().find(|node| node.kind == "publish").expect("a Publish was added");
        let tags = tags_of(publish);
        assert!(tags.contains(&"music".to_string()), "{tags:?}");
        assert!(tags.contains(&STUDIO_TAG.to_string()));
        assert!(tags.contains(&"preset-bundled-music".to_string()));
        assert!(tags.iter().all(|tag| crate::presets::tests::valid_tag(tag)), "{tags:?}");
        assert_eq!(text_param(publish, "title").as_deref(), Some("Music · slow dub techno"));
        assert!(back.edges.iter().any(|edge| edge.to_node == publish.id && edge.from_node == "music"));
        // The Output stays: the run still returns the song.
        assert!(back.nodes.iter().any(|node| node.kind == "output"));
    }

    #[test]
    fn an_existing_publish_keeps_its_tags_and_gains_the_destination() {
        let sfx = preset("bundled:expanded-prompt-to-sfx");
        let prepared = prepare(&sfx.graph, &provenance("bundled:expanded-prompt-to-sfx"));
        let publishes: Vec<&Node> = prepared.nodes.iter().filter(|node| node.kind == "publish").collect();
        assert_eq!(publishes.len(), 1, "no second Publish for an already published value");
        let tags = tags_of(publishes[0]);
        for tag in ["flow", "sfx", STUDIO_TAG] {
            assert_eq!(tags.iter().filter(|have| *have == tag).count(), 1, "{tag} in {tags:?}");
        }
    }

    #[test]
    fn saved_values_replace_the_design_defaults() {
        let music = preset("bundled:music");
        let saved = with_input_values(&music.graph, &[("prompt".into(), "ambient drone".into())]);
        let source = makepad_flow::graph::write(&saved).unwrap();
        let back = Preset::from_graph("saved:x".into(), None, "Audio", "x.splash",
            makepad_flow::graph::evaluate(&source, "x.splash").unwrap());
        assert_eq!(back.inputs.iter().find(|input| input.node == "prompt").unwrap().default, "ambient drone");
    }

    #[test]
    fn long_prompts_are_cut_for_titles() {
        let title = title_for(&Provenance { preset: String::new(), label: "Image".into(), prompt: "word ".repeat(40), tags: Vec::new() });
        assert!(title.ends_with('…'));
        assert!(title.chars().count() < 80);
    }
}
