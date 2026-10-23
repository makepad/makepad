//! The preset library: every prefab workflow the studio can fire up, as
//! data. The bundled templates (`makepad_flow::templates`: the recipe set
//! and the "Studio" group of `libs/flow/recipes/studio`), the host's own
//! presets ([`crate::host::StudioHost::presets`]) and presets saved to the
//! result store are all ordinary flow sources; describing one evaluates it (on
//! the studio worker, never the UI thread) and reads what a card shows: its
//! inputs, the models its generators need, and where its results land.

use makepad_flow::{Graph, Literal, Node, NodeInputValue, PortType};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Group {
    Live,
    Image,
    Video,
    Audio,
    Music,
    Text,
    Mesh,
    Utility,
    Saved,
}

impl Group {
    pub const ALL: [Group; 9] = [
        Group::Live, Group::Image, Group::Video, Group::Audio, Group::Music,
        Group::Text, Group::Mesh, Group::Utility, Group::Saved,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Group::Live => "LIVE",
            Group::Image => "IMAGE",
            Group::Video => "VIDEO",
            Group::Audio => "AUDIO · SPEECH",
            Group::Music => "MUSIC",
            Group::Text => "TEXT · VISION",
            Group::Mesh => "3D",
            Group::Utility => "UTILITY",
            Group::Saved => "SAVED",
        }
    }

    /// The short glyph on a card's tile.
    pub fn glyph(self) -> &'static str {
        match self {
            Group::Live => "LIV",
            Group::Image => "IMG",
            Group::Video => "VID",
            Group::Audio => "SPK",
            Group::Music => "MUS",
            Group::Text => "TXT",
            Group::Mesh => "3D",
            Group::Utility => "UTL",
            Group::Saved => "USR",
        }
    }

    /// The bundled template groups, split where the studio is finer.
    fn of_template(group: &str, domains: &[String]) -> Group {
        match group {
            // Studio presets go where their results land.
            "Studio" if domains.iter().any(|domain| domain == "music") => Group::Music,
            "Studio" if domains.iter().any(|domain| domain == "video") => Group::Video,
            "Studio" if domains.iter().any(|domain| matches!(domain.as_str(), "speech" | "audio")) => Group::Audio,
            "Studio" => Group::Image,
            "Image" => Group::Image,
            "Video" => Group::Video,
            "Audio" if domains.iter().any(|domain| domain == "music") => Group::Music,
            "Audio" => Group::Audio,
            "3D" => Group::Mesh,
            "Vision & text" => Group::Text,
            _ => Group::Utility,
        }
    }
}

/// Where a published value lands (the host names the place, see `host::Place`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destination {
    /// Pictures.
    Image,
    /// Video.
    Video,
    /// Music (Audio tagged `music`).
    Music,
    /// A voiceover take (Audio tagged `voiceover`), for edits.
    Voice,
    /// Sound effects and other audio.
    Sound,
    /// One separated stem of a song (Audio tagged `stem`).
    Stem,
    Mesh,
    World,
    Data,
}

impl Destination {
    /// From the published value's type and the domain that made it.
    pub fn of(ty: &PortType, domain: Option<&str>) -> Destination {
        match (ty, domain) {
            (PortType::Image, _) => Destination::Image,
            (PortType::Video, _) => Destination::Video,
            (PortType::Audio, Some("music")) => Destination::Music,
            (PortType::Audio, Some("speech")) => Destination::Voice,
            (PortType::Audio, Some("stems")) => Destination::Stem,
            (PortType::Audio, _) => Destination::Sound,
            (PortType::Mesh, Some("world" | "splat")) => Destination::World,
            (PortType::Mesh, _) => Destination::Mesh,
            _ => Destination::Data,
        }
    }

    /// The tags that put a published result where the host looks for it.
    pub fn tags(self) -> &'static [&'static str] {
        (crate::host::host().place)(self).tags
    }

    /// Where the result shows, for the card and the results list.
    pub fn place(self) -> &'static str {
        (crate::host::host().place)(self).label
    }
}

/// One settable input of a preset: an `Input` node.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetInput {
    pub node: String,
    pub ty: PortType,
    /// The node's doc comment, or its id.
    pub label: String,
    /// The design value, for text inputs.
    pub default: String,
}

/// One generator's model: its hub domain and pinned model (empty: the hub
/// picks).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ModelNeed {
    pub node: String,
    pub domain: String,
    pub model: String,
}

#[derive(Clone, Debug)]
pub struct Preset {
    /// `bundled:<name>`, `studio:<name>` or `saved:<asset id>`.
    pub key: String,
    pub label: String,
    pub brief: String,
    pub group: Group,
    pub inputs: Vec<PresetInput>,
    pub needs: Vec<ModelNeed>,
    /// Language-model turns (`Llm`, `Chat`): they run through the hub's chat.
    pub llm_nodes: usize,
    /// Every value it publishes (or would, through an added `Publish`).
    pub destinations: Vec<Destination>,
    pub graph: Arc<Graph>,
}

impl Preset {
    /// Evaluate `source` and read the card from its graph.
    pub fn describe(key: String, group: Option<Group>, template_group: &str, source: Arc<str>, file_name: String) -> Result<Preset, String> {
        // With the recipe checks the shared presets use.
        let graph = makepad_gen::catalog::graph(&source, &file_name)?;
        Ok(Self::from_graph(key, group, template_group, &file_name, graph))
    }

    pub fn from_graph(key: String, group: Option<Group>, template_group: &str, file_name: &str, graph: Graph) -> Preset {
        let inputs = graph.nodes.iter()
            .filter(|node| node.kind == "input")
            .filter_map(|node| {
                let port = node.outputs.first()?;
                Some(PresetInput {
                    node: node.id.clone(),
                    ty: port.ty.clone(),
                    label: node.doc.as_deref().map(first_sentence).filter(|doc| !doc.is_empty()).unwrap_or_else(|| node.id.clone()),
                    default: design_text(node).unwrap_or_default(),
                })
            })
            .collect();
        let needs: Vec<ModelNeed> = graph.nodes.iter()
            .filter(|node| node.kind == "gen")
            .filter_map(|node| Some(ModelNeed {
                node: node.id.clone(),
                domain: node.domain.clone().filter(|domain| !domain.is_empty())?,
                model: text_param(node, "model").unwrap_or_default(),
            }))
            .collect();
        let llm_nodes = graph.nodes.iter().filter(|node| node.kind == "chat").count();
        let domains: Vec<String> = needs.iter().map(|need| need.domain.clone()).collect();
        let destinations = destinations(&graph);
        Preset {
            key,
            label: if graph.label.is_empty() { file_name.trim_end_matches(".splash").to_string() } else { graph.label.clone() },
            brief: graph.brief.clone(),
            group: group.unwrap_or_else(|| Group::of_template(template_group, &domains)),
            inputs,
            needs,
            llm_nodes,
            destinations,
            graph: Arc::new(graph),
        }
    }

    /// The line under the title: what goes in and where it lands.
    pub fn summary(&self) -> String {
        let inputs = self.inputs.iter().map(|input| input.ty.as_str().to_string()).collect::<Vec<_>>();
        let mut places: Vec<&str> = Vec::new();
        for place in self.destinations.iter().map(|destination| destination.place()) {
            if !places.contains(&place) {
                places.push(place);
            }
        }
        let inputs = if inputs.is_empty() { "no inputs".to_string() } else { inputs.join(" + ") };
        if places.is_empty() { inputs } else { format!("{inputs} → {}", places.join(", ")) }
    }
}

/// Every bundled and studio preset source, in the library's order.
pub fn sources() -> Vec<(String, Option<Group>, &'static str, Arc<str>, String)> {
    makepad_flow::templates::TEMPLATES.iter().chain(crate::host::host().presets.iter()).map(|template| {
        // Studio presets keep the `studio:` key their published assets'
        // `preset-studio-…` tags were made from.
        let prefix = if template.group == "Studio" { "studio" } else { "bundled" };
        (format!("{prefix}:{}", template.name), None, template.group, Arc::from(template.source), format!("{}.splash", template.name))
    }).chain(makepad_gen::catalog::PRESETS.iter().map(|preset| {
        // The shared generation presets, under `gen:<id>`; the gen.* tools
        // show them by their own ids.
        let group = match (preset.kind, preset.id) {
            ("image", _) => Group::Image,
            ("video", _) => Group::Video,
            ("audio", "music" | "song") => Group::Music,
            ("audio", _) => Group::Audio,
            _ => Group::Mesh,
        };
        (format!("{}{}", super::api::GEN_KEY, preset.id), Some(group), "Generation", Arc::from(preset.source), format!("{}.splash", preset.id))
    })).chain(super::api::sources()).collect()
}

/// The type and the generating domain of every value the graph publishes,
/// or would publish: each `Publish`, and each media `Output` no `Publish`
/// reads the same port of.
pub fn destinations(graph: &Graph) -> Vec<Destination> {
    publish_points(graph).into_iter().map(|point| Destination::of(&point.ty, point.domain.as_deref())).collect()
}

/// A value the studio publishes: the port it comes from and its type.
#[derive(Clone, Debug, PartialEq)]
pub struct PublishPoint {
    pub from_node: String,
    pub from_port: String,
    pub ty: PortType,
    pub domain: Option<String>,
    /// The existing `Publish` node, or the `Output` a new one goes beside.
    pub node: String,
    pub existing: bool,
}

pub fn publish_points(graph: &Graph) -> Vec<PublishPoint> {
    let source_of = |node: &Node| -> Option<(String, String, PortType)> {
        let input = node.inputs.iter().find(|input| input.port == "value")?;
        match &input.value {
            NodeInputValue::Edge(edge) => Some((edge.from_node.clone(), edge.from_port.clone(), input.ty.clone())),
            _ => None,
        }
    };
    let domain_of = |id: &str| graph.nodes.iter().find(|node| node.id == id).and_then(|node| node.domain.clone());
    let port_type = |node_id: &str, port: &str| graph.nodes.iter().find(|node| node.id == node_id)
        .and_then(|node| node.outputs.iter().find(|output| output.name == port)).map(|output| output.ty.clone());
    let mut points: Vec<PublishPoint> = Vec::new();
    for node in graph.nodes.iter().filter(|node| node.kind == "publish") {
        if let Some((from_node, from_port, ty)) = source_of(node) {
            let ty = port_type(&from_node, &from_port).unwrap_or(ty);
            points.push(PublishPoint { domain: domain_of(&from_node), from_node, from_port, ty, node: node.id.clone(), existing: true });
        }
    }
    for node in graph.nodes.iter().filter(|node| node.kind == "output") {
        let Some((from_node, from_port, ty)) = source_of(node) else { continue };
        let ty = port_type(&from_node, &from_port).unwrap_or(ty);
        if !matches!(ty, PortType::Image | PortType::Audio | PortType::Video | PortType::Mesh) {
            continue;
        }
        if points.iter().any(|point| point.from_node == from_node && point.from_port == from_port) {
            continue;
        }
        points.push(PublishPoint { domain: domain_of(&from_node), from_node, from_port, ty, node: node.id.clone(), existing: false });
    }
    points
}

/// An Input's design value as its field shows it: text as is, a list or
/// object as JSON.
fn design_text(node: &Node) -> Option<String> {
    let value = node.params.iter().find(|(name, _)| name == "value").or_else(|| node.params.iter().find(|(name, _)| name == "default"))
        .map(|(_, value)| value)?;
    match value {
        Literal::Arr(_) | Literal::Obj(_) => Some(literal_json(value)),
        _ => text_param(node, "value").or_else(|| text_param(node, "default")),
    }
}

/// A literal written as JSON.
pub fn literal_json(value: &Literal) -> String {
    match value {
        Literal::Null => "null".into(),
        Literal::Bool(value) => value.to_string(),
        Literal::Num(value) => format_number(*value),
        Literal::Str(text) | Literal::Id(text) => makepad_strict_json::Value::Str(text.clone()).to_json(),
        Literal::Arr(items) => format!("[{}]", items.iter().map(literal_json).collect::<Vec<_>>().join(", ")),
        Literal::Obj(fields) => format!("{{{}}}", fields.iter()
            .map(|(key, value)| format!("{}: {}", makepad_strict_json::Value::Str(key.clone()).to_json(), literal_json(value))).collect::<Vec<_>>().join(", ")),
    }
}

/// A field's JSON text as a literal, for list and JSON inputs.
pub fn json_literal(text: &str) -> Result<Literal, String> {
    fn convert(value: &makepad_strict_json::Value) -> Literal {
        use makepad_strict_json::Value as J;
        match value {
            J::Null => Literal::Null,
            J::Bool(value) => Literal::Bool(*value),
            J::Int(value) => Literal::Num(*value as f64),
            J::F64(value) => Literal::Num(*value),
            J::Str(text) => Literal::Str(text.clone()),
            J::Arr(items) => Literal::Arr(items.iter().map(convert).collect()),
            J::Obj(fields) => Literal::Obj(fields.iter().map(|(key, value)| (key.clone(), convert(value))).collect()),
        }
    }
    makepad_strict_json::parse(text.trim().as_bytes()).map(|value| convert(&value)).map_err(|error| format!("not JSON: {error}"))
}

pub fn text_param(node: &Node, key: &str) -> Option<String> {
    node.params.iter().find(|(name, _)| name == key).and_then(|(_, value)| match value {
        Literal::Str(text) | Literal::Id(text) => Some(text.clone()),
        Literal::Num(value) => Some(format_number(*value)),
        Literal::Bool(value) => Some(value.to_string()),
        _ => None,
    })
}

pub fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 { format!("{}", value as i64) } else { format!("{value}") }
}

fn first_sentence(doc: &str) -> String {
    let doc = doc.trim();
    let end = doc.find(". ").map(|at| at + 1).unwrap_or(doc.len());
    doc[..end].trim_end_matches('.').trim().to_string()
}

/// Most bytes a library tag may have (the store's label budget).
pub const MAX_TAG_BYTES: usize = 48;

/// `<prefix>-<slug of text>` as a library tag: the store takes lower-case
/// ASCII letters, digits, `-` and `_`, starting with a letter or digit,
/// at most [`MAX_TAG_BYTES`].
pub fn tag(prefix: &str, text: &str) -> String {
    let mut tag = format!("{prefix}-{}", slug(text));
    tag.truncate(MAX_TAG_BYTES);
    tag.trim_end_matches('-').to_string()
}

/// A preset's library key, as a tag: lower-case, `-` for anything else.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') && !out.is_empty() {
            out.push('-');
        }
    }
    out.trim_end_matches('-').to_string()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn every_shipped_preset_is_well_formed() {
        let snapshot = super::super::models::HubSnapshot::default();
        for (key, group, template_group, source, file) in sources() {
            let preset = Preset::describe(key.clone(), group, template_group, source, file).unwrap();
            let problems = super::super::models::graph_problems(&preset.graph, &snapshot);
            assert!(problems.is_empty(), "{key}: {problems:?}");
        }
    }

    #[test]
    fn every_preset_evaluates_and_describes() {
        for (key, group, template_group, source, file) in sources() {
            let preset = Preset::describe(key.clone(), group, template_group, source, file)
                .unwrap_or_else(|error| panic!("{key}: {error}"));
            assert!(!preset.label.is_empty(), "{key} has no label");
        }
    }

    #[test]
    fn studio_presets_land_where_their_results_go() {
        let find = |name: &str| {
            let (key, group, template_group, source, file) = sources().into_iter().find(|entry| entry.0 == name).unwrap();
            Preset::describe(key, group, template_group, source, file).unwrap()
        };
        let flux = find("studio:flux-schnell-fast");
        assert_eq!(flux.destinations, vec![Destination::Image]);
        assert_eq!(flux.needs, vec![ModelNeed { node: "image".into(), domain: "image".into(), model: "flux1-schnell-q4".into() }]);
        assert_eq!(flux.inputs.len(), 1);
        assert_eq!(flux.inputs[0].ty, PortType::Text);
        assert!(flux.inputs[0].default.contains("koi"));
    }

    #[test]
    fn music_templates_group_as_music_and_outputs_count_as_publish_points() {
        let (key, group, template_group, source, file) = sources().into_iter().find(|entry| entry.0 == "bundled:music").unwrap();
        let preset = Preset::describe(key, group, template_group, source, file).unwrap();
        assert_eq!(preset.group, Group::Music);
        // `music.splash` ends in an Output: the studio publishes it anyway.
        assert_eq!(preset.destinations, vec![Destination::Music]);
        let points = publish_points(&preset.graph);
        assert_eq!(points.len(), 1);
        assert!(!points[0].existing);
    }

    /// The store's `check_label`: what every published tag must pass.
    pub fn valid_tag(tag: &str) -> bool {
        let bytes = tag.as_bytes();
        !bytes.is_empty() && bytes.len() <= MAX_TAG_BYTES
            && (bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit())
            && bytes.iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-' || c == b'_')
    }

    #[test]
    fn tags_pass_the_stores_label_rules() {
        assert!(valid_tag(&tag("preset", "studio:flux-dev-portrait")));
        assert!(valid_tag(&tag("preset", &"a very long preset name ".repeat(6))));
        assert!(valid_tag(&tag("group", Group::Audio.label())));
        for destination in [Destination::Image, Destination::Video, Destination::Music, Destination::Voice, Destination::Sound, Destination::Stem, Destination::Mesh, Destination::World, Destination::Data] {
            assert!(destination.tags().iter().all(|tag| valid_tag(tag)));
        }
    }

    #[test]
    fn slugs_are_tag_safe() {
        assert_eq!(slug("Flux dev · 9:16 portrait"), "flux-dev-9-16-portrait");
        assert_eq!(slug("studio:dj-track"), "studio-dj-track");
    }
}
