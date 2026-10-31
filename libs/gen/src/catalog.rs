//! The generation presets: one recipe document per preset, the same list in
//! every app. A preset is an ordinary Flow document (`recipes/presets/`):
//! a `prompt` text input, media inputs in the order a caller hands asset
//! ids, generators whose literal parameters are the preset's defaults, and
//! `Publish` nodes for what lands in the library (`published` is the
//! primary product).

use makepad_flow::document::closure::Closure;
use makepad_flow::document::Registry;
use makepad_flow::{Graph, Literal, PortType};

/// One preset of the catalog.
#[derive(Clone, Copy, Debug)]
pub struct Preset {
    pub id: &'static str,
    /// What it makes: image, video, audio, mesh, character or world.
    pub kind: &'static str,
    pub source: &'static str,
}

macro_rules! preset {
    ($id:literal, $kind:literal) => {
        Preset { id: $id, kind: $kind, source: include_str!(concat!("../recipes/presets/", $id, ".splash")) }
    };
}

pub const PRESETS: &[Preset] = &[
    preset!("image", "image"),
    preset!("image.edit", "image"),
    preset!("image.upscale", "image"),
    preset!("image.matte", "image"),
    preset!("image.depth", "image"),
    preset!("video", "video"),
    preset!("video.from_image", "video"),
    preset!("video.enhance", "video"),
    preset!("sound", "audio"),
    preset!("music", "audio"),
    preset!("song", "audio"),
    preset!("song.score", "audio"),
    preset!("speech", "audio"),
    preset!("mesh", "mesh"),
    preset!("mesh.paint", "mesh"),
    preset!("character", "character"),
    preset!("prop", "mesh"),
    preset!("world", "world"),
];

pub const KINDS: &[&str] = &["image", "video", "audio", "mesh", "character", "world"];

pub fn preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|preset| preset.id == id)
}

/// The registry recipes are read with: the builtins and the recipe checks.
pub fn registry() -> &'static Registry {
    static REGISTRY: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = Registry::with_builtins();
        let mut executors = makepad_flow::engine::providers::ExecutorRegistry::default();
        crate::nodes::register(&mut registry, &mut executors).expect("recipe check nodes register");
        registry
    })
}

/// Evaluate a recipe document.
pub fn graph(source: &str, file_name: &str) -> Result<Graph, String> {
    makepad_flow::graph::evaluate_with_registry(source, file_name, &Closure::default(), registry()).map_err(|error| error.to_string())
}

/// A media input a caller fills with an asset.
#[derive(Clone, Debug)]
pub struct MediaInput {
    pub node: String,
    pub ty: PortType,
    pub doc: String,
}

/// A text input other than the prompt (a song's lyrics).
#[derive(Clone, Debug)]
pub struct TextInput {
    pub node: String,
    pub default: String,
    pub doc: String,
}

/// One generator job of a preset.
#[derive(Clone, Debug)]
pub struct Generator {
    pub node: String,
    pub domain: String,
    /// The pinned model; empty lets the fleet choose.
    pub model: String,
}

/// What a caller needs to run a preset.
#[derive(Clone, Debug)]
pub struct PresetInfo {
    pub id: String,
    pub kind: String,
    pub label: String,
    pub brief: String,
    /// The prompt's meaning and default, when the preset takes one.
    pub prompt: Option<(String, String)>,
    pub texts: Vec<TextInput>,
    pub media: Vec<MediaInput>,
    /// Tunable generator parameters and their defaults (the first
    /// generator that has one), `seed` and `model` aside.
    pub params: Vec<(String, Literal)>,
    pub generators: Vec<Generator>,
}

fn doc_of(node: &makepad_flow::Node) -> String {
    node.doc.clone().unwrap_or_default().trim().to_string()
}

fn text_of(literal: Option<&Literal>) -> String {
    match literal {
        Some(Literal::Str(text) | Literal::Id(text)) => text.clone(),
        _ => String::new(),
    }
}

/// The parameters a caller may not set through `params`: identity and
/// routing belong to the recipe and the run.
const FIXED_PARAMS: &[&str] = &["seed", "label", "ports", "domain", "ui", "kind", "type_name", "at", "size", "flip"];

/// Describe a recipe document as a preset.
pub fn describe(id: &str, kind: &str, graph: &Graph) -> PresetInfo {
    let mut info = PresetInfo {
        id: id.to_string(), kind: kind.to_string(), label: graph.label.clone(), brief: graph.brief.clone(),
        prompt: None, texts: Vec::new(), media: Vec::new(), params: Vec::new(), generators: Vec::new(),
    };
    for node in &graph.nodes {
        match node.kind.as_str() {
            "input" => {
                let Some(ty) = node.outputs.first().map(|port| port.ty.clone()) else { continue };
                let default = text_of(node.params.iter().find(|(name, _)| name == "value" || name == "default").map(|(_, value)| value));
                if ty == PortType::Text && node.id == "prompt" {
                    info.prompt = Some((doc_of(node), default));
                } else if ty == PortType::Text {
                    info.texts.push(TextInput { node: node.id.clone(), default, doc: doc_of(node) });
                } else if ty.is_media() {
                    info.media.push(MediaInput { node: node.id.clone(), ty, doc: doc_of(node) });
                }
            }
            "gen" => {
                info.generators.push(Generator {
                    node: node.id.clone(),
                    domain: node.domain.clone().unwrap_or_default(),
                    model: text_of(node.params.iter().find(|(name, _)| name == "model").map(|(_, value)| value)),
                });
                for (name, value) in &node.params {
                    if FIXED_PARAMS.contains(&name.as_str()) || name == "model" || node.inputs.iter().any(|input| &input.port == name)
                        || info.params.iter().any(|(known, _)| known == name) {
                        continue;
                    }
                    info.params.push((name.clone(), value.clone()));
                }
            }
            _ => {}
        }
    }
    info
}

/// Every preset, described.
pub fn presets() -> Vec<PresetInfo> {
    PRESETS.iter().map(|preset| {
        let graph = graph(preset.source, &format!("{}.splash", preset.id))
            .unwrap_or_else(|error| panic!("bundled preset `{}` is invalid: {error}", preset.id));
        describe(preset.id, preset.kind, &graph)
    }).collect()
}
