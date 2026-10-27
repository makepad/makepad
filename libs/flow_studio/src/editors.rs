//! The chosen preset's nodes, editable in place on the canvas: what each
//! node's editor offers ([`EditorSpec`]), the edits the person made (the
//! [`Draft`]), and runs fired from a node's own Run button.
//!
//! The draft is the page's own state for the chosen preset: an Input's text,
//! a language model's instruction, a generator's model and settings, as
//! typed. A run takes the draft through the same `RunRequest` the API and
//! the chat use (`run_preset`), so a node edit and a chat card edit mean the
//! same thing. A node's Run runs that node and everything it needs upstream,
//! plus the Publish nodes it alone feeds (so a picture still lands in the
//! library); the header's Run runs the whole graph.

use super::api::RunRequest;
use super::canvas::{EditorSpec, ModelChoice, MODEL_ROWS, PARAM_ROWS};
use super::presets::{self, Preset};
use super::{FlowStudio, PARAMS};
use makepad_ai_hub::fleet::{self, BoxSnapshot, VramAdmission};
use makepad_ai_hub::protocol::GenerateRequestJson;
use makepad_flow::{Graph, Literal, Node};
use std::collections::{HashMap, HashSet};

/// What the person typed and picked on the chosen preset's nodes.
#[derive(Clone, Debug, Default)]
pub(super) struct Draft {
    /// An Input's text, or a language model's instruction, by node.
    pub text: HashMap<String, String>,
    /// `(node, key)` → the setting as typed ("" = the node's default).
    pub params: HashMap<(String, String), String>,
    /// A generator's model by node ("" = the hub picks).
    pub models: HashMap<String, String>,
}

/// A setting as the editor shows it: a fresh seed reads as empty ("auto").
fn shown_value(node: &Node, key: &str) -> String {
    match node.params.iter().find(|(name, _)| name == key).map(|(_, value)| value) {
        Some(Literal::Id(id)) if id == "random" => String::new(),
        Some(Literal::Num(value)) if key == "seed" && *value < 0.0 => String::new(),
        _ => presets::text_param(node, key).unwrap_or_default(),
    }
}

/// A typed setting as the graph takes it; `None` keeps the node's default.
pub(super) fn typed_literal(key: &str, value: &str) -> Option<Literal> {
    let value = value.trim();
    if value.is_empty() {
        return (key == "seed").then(|| Literal::Id("random".into()));
    }
    Some(match value.parse::<f64>() {
        Ok(number) if number.is_finite() => Literal::Num(number),
        _ => Literal::Str(value.to_string()),
    })
}

/// The editable settings of `node`, in the order they are offered (the
/// model has its own list).
fn param_keys(node: &Node) -> Vec<&'static str> {
    if !matches!(node.kind.as_str(), "gen" | "chat") {
        return Vec::new();
    }
    PARAMS.iter().copied()
        .filter(|key| *key != "model")
        .filter(|key| node.params.iter().any(|(name, value)| name == key && matches!(value, Literal::Num(_) | Literal::Str(_) | Literal::Id(_))))
        .take(PARAM_ROWS)
        .collect()
}

/// `graph` cut to `node`, everything it needs upstream, and the Publish /
/// Output nodes fed only from those.
pub(super) fn upstream_of(graph: &Graph, node: &str) -> Graph {
    let mut keep: HashSet<String> = HashSet::from([node.to_string()]);
    loop {
        let before = keep.len();
        for edge in &graph.edges {
            if keep.contains(&edge.to_node) {
                keep.insert(edge.from_node.clone());
            }
        }
        if keep.len() == before {
            break;
        }
    }
    for candidate in graph.nodes.iter().filter(|candidate| matches!(candidate.kind.as_str(), "publish" | "output")) {
        let mut feeds = graph.edges.iter().filter(|edge| edge.to_node == candidate.id).peekable();
        if feeds.peek().is_some() && feeds.all(|edge| keep.contains(&edge.from_node)) {
            keep.insert(candidate.id.clone());
        }
    }
    let mut cut = graph.clone();
    for id in graph.nodes.iter().map(|node| node.id.clone()).filter(|id| !keep.contains(id)) {
        cut = makepad_flow_ui::graph_edit::delete_node(&cut, &id);
    }
    cut.tools.clear();
    cut
}

/// How much memory a model wants, as a row reads it.
fn size_text(vram_gb: Option<f64>) -> String {
    match vram_gb {
        Some(gb) if gb.is_finite() && gb > 0.0 => format!("{gb:.0} GB"),
        _ => String::new(),
    }
}

/// A generator's MODEL list from what the fleet reports: "the hub picks"
/// (with what it would pick now), then each model of the node's domain
/// with its size and the nodes it fits on (or is too big for).
pub(super) fn model_choices(boxes: &[BoxSnapshot], domain: &str, chosen: &str) -> Vec<ModelChoice> {
    struct Row {
        size: String,
        fits: Vec<String>,
        too_big: Vec<String>,
    }
    let mut rows: Vec<(String, Row)> = Vec::new();
    for snapshot in boxes.iter().filter(|snapshot| snapshot.is_up()) {
        let node = fleet::node_label(&snapshot.base_url);
        for model in snapshot.models.iter().filter(|model| model.domain == domain && model.available) {
            let installed = matches!(model.state.as_str(), "loaded" | "ready");
            let too_small = model.state == "too_small"
                || matches!(fleet::model_admission(snapshot, &model.id), Some(VramAdmission::Incompatible { .. }));
            if !installed && !too_small {
                continue;
            }
            let index = match rows.iter().position(|(id, _)| *id == model.id) {
                Some(index) => index,
                None => {
                    rows.push((model.id.clone(), Row { size: size_text(model.vram_gb), fits: Vec::new(), too_big: Vec::new() }));
                    rows.len() - 1
                }
            };
            let row = &mut rows[index].1;
            if too_small { row.too_big.push(node.clone()) } else { row.fits.push(node.clone()) }
        }
    }
    // What fits somewhere first, then by name.
    rows.sort_by(|a, b| a.1.fits.is_empty().cmp(&b.1.fits.is_empty()).then_with(|| a.0.cmp(&b.0)));
    let request = GenerateRequestJson::default();
    let picks = makepad_ai_hub::job::route_generation(boxes.to_vec(), domain, &request).ok()
        .map(|(snapshot, model)| format!("now {model} on {}", fleet::node_label(&snapshot.base_url)));
    let mut out = vec![ModelChoice {
        id: String::new(),
        label: match picks {
            Some(picks) => format!("auto · the hub picks · {picks}"),
            None => "auto · the hub picks".into(),
        },
    }];
    for (id, row) in rows.into_iter().take(MODEL_ROWS - 1) {
        let mut parts = vec![id.clone()];
        if !row.size.is_empty() {
            parts.push(row.size);
        }
        if !row.fits.is_empty() {
            parts.push(format!("fits {}", row.fits.join(", ")));
        }
        if !row.too_big.is_empty() {
            parts.push(format!("too big for {}", row.too_big.join(", ")));
        }
        out.push(ModelChoice { id, label: parts.join(" · ") });
    }
    if !chosen.is_empty() && !out.iter().any(|choice| choice.id == chosen) {
        let last = out.len().min(MODEL_ROWS - 1);
        out.truncate(last);
        out.push(ModelChoice { id: chosen.to_string(), label: format!("{chosen} · not on any node that answers") });
    }
    out
}

impl FlowStudio {
    /// The chosen preset's node editors, from its graph, the draft, the
    /// hub snapshot and the run shown.
    pub(super) fn editor_specs(&self) -> Vec<EditorSpec> {
        let Some(preset) = self.chosen.as_ref().filter(|_| !matches!(self.selected, Some(super::Selected::Live(_)))) else { return Vec::new() };
        let run = self.shown_run.and_then(|id| self.runs.iter().find(|run| run.id == id));
        let node_run = self.node_run.as_ref().and_then(|(node, id)| self.runs.iter().find(|run| run.id == *id && run.active()).map(|_| node.clone()));
        let mut specs = Vec::new();
        for node in &preset.graph.nodes {
            let mut spec = EditorSpec { node: node.id.clone(), ..Default::default() };
            match node.kind.as_str() {
                "input" => {
                    if let Some(input) = preset.inputs.iter().find(|input| input.node == node.id && !input.ty.is_media()) {
                        let text = self.draft.text.get(&node.id).cloned().unwrap_or_else(|| input.default.clone());
                        spec.text = Some((input.label.to_uppercase(), text));
                    }
                }
                "chat" => {
                    let text = self.draft.text.get(&node.id).cloned().or_else(|| presets::text_param(node, "system")).unwrap_or_default();
                    spec.text = Some(("INSTRUCTION".into(), text));
                }
                _ => {}
            }
            if node.kind == "gen" {
                let pinned = presets::text_param(node, "model").unwrap_or_default();
                spec.model = self.draft.models.get(&node.id).cloned().unwrap_or(pinned);
                if let Some(domain) = node.domain.as_deref().filter(|domain| !domain.is_empty()) {
                    spec.models = model_choices(&self.snapshot.boxes, domain, &spec.model);
                }
            }
            spec.params = param_keys(node).into_iter().map(|key| {
                let value = self.draft.params.get(&(node.id.clone(), key.to_string())).cloned().unwrap_or_else(|| shown_value(node, key));
                (key.to_string(), value)
            }).collect();
            spec.run = matches!(node.kind.as_str(), "gen" | "chat" | "input" | "fn" | "map" | "http" | "publish");
            spec.running = node_run.as_deref() == Some(node.id.as_str());
            if let Some(run) = run {
                if let Some((seed, model, hub)) = run.used.get(&node.id) {
                    let on = if hub.is_empty() { String::new() } else { format!(" on {hub}") };
                    let seed = if seed.is_empty() { String::new() } else { format!(" · seed {seed}") };
                    spec.note = format!("ran {model}{on}{seed}");
                }
                if let Some(status) = run.statuses.get(&node.id) {
                    match (&status.error, status.state.as_str()) {
                        (Some(error), _) => spec.note = format!("failed: {error}"),
                        (None, "running") if !status.stage.is_empty() => spec.note = status.stage.clone(),
                        (None, "running") => spec.note = "running…".into(),
                        _ => {}
                    }
                }
            }
            specs.push(spec);
        }
        specs
    }

    /// The draft as a run request (the header's Run and a node's Run).
    pub(super) fn draft_request(&self, preset: &Preset) -> RunRequest {
        let mut request = RunRequest::new("FLOW");
        for (node, text) in &self.draft.text {
            if preset.inputs.iter().any(|input| &input.node == node && !input.ty.is_media()) {
                request = request.text(node, text);
            } else if preset.graph.nodes.iter().any(|candidate| &candidate.id == node && candidate.kind == "chat") {
                request = request.param(Some(node), "system", Literal::Str(text.clone()));
            }
        }
        for ((node, key), value) in &self.draft.params {
            if let Some(literal) = typed_literal(key, value) {
                request = request.param(Some(node), key, literal);
            }
        }
        for (node, model) in &self.draft.models {
            request = request.param(Some(node), "model", Literal::Str(model.clone()));
        }
        request
    }

    /// A node's Run: that node and what it needs, with the draft; Cancel
    /// while its run is under way.
    pub(super) fn run_node(&mut self, cx: &mut makepad_widgets::Cx, ui: &makepad_widgets::WidgetRef, node: &str) {
        if let Some((running, id)) = self.node_run.clone() {
            if running == node && self.runs.iter().any(|run| run.id == id && run.active()) {
                self.cancel_job(id);
                return;
            }
        }
        let Some(preset) = self.chosen.clone() else { return };
        let mut request = self.draft_request(&preset);
        request.only = Some(node.to_string());
        match self.run_preset(cx, &preset.key, request) {
            Ok(id) => {
                self.node_run = Some((node.to_string(), id));
                self.notice.clear();
                self.show_run(cx, ui, Some(id));
            }
            Err(error) => self.notice = format!("Run {node}: {error}"),
        }
        self.inspector_dirty = true;
    }

    /// An edit on a node's editor, into the draft.
    pub(super) fn edit(&mut self, action: super::canvas::StudioCanvasAction) {
        use super::canvas::StudioCanvasAction as A;
        match action {
            A::Text { node, text } => { self.draft.text.insert(node, text); }
            A::Param { node, key, value } => { self.draft.params.insert((node, key), value); }
            A::Model { node, model } => {
                self.draft.models.insert(node, model);
                self.inspector_dirty = true;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_ai_hub::protocol::{HealthJson, ModelInfoJson};
    use makepad_widgets::makepad_micro_serde::DeJson;

    fn graph() -> Graph {
        let source = r#"use mod.flow.*
let prompt = Input{ type: @text default: "a robot" }
let expand = Llm{ system: "Rewrite." prompt: prompt.text() }
let image = Image{ prompt: expand.text() width: 512 height: 512 }
let published = Publish{ value: image.image() namespace: "flows" tags: ["flow"] }
let other = Input{ type: @text default: "unused" }
Flow{ label: "t" brief: "t" prompt, expand, image, published, other }
"#;
        makepad_gen::catalog::graph(source, "t.splash").unwrap()
    }

    #[test]
    fn a_node_run_takes_what_it_needs_and_what_it_alone_publishes() {
        let graph = graph();
        let mut ids: Vec<String> = upstream_of(&graph, "image").nodes.iter().map(|node| node.id.clone()).collect();
        ids.sort();
        assert_eq!(ids, ["expand", "image", "prompt", "published"]);
        let mut ids: Vec<String> = upstream_of(&graph, "expand").nodes.iter().map(|node| node.id.clone()).collect();
        ids.sort();
        assert_eq!(ids, ["expand", "prompt"]);
    }

    #[test]
    fn typed_settings_become_literals_and_an_empty_seed_is_fresh() {
        assert_eq!(typed_literal("width", "768"), Some(Literal::Num(768.0)));
        assert_eq!(typed_literal("width", " "), None);
        assert_eq!(typed_literal("seed", ""), Some(Literal::Id("random".into())));
        assert_eq!(typed_literal("voice", "af_heart"), Some(Literal::Str("af_heart".into())));
    }

    fn model(id: &str, gb: f64, state: &str) -> ModelInfoJson {
        let json = format!(r#"{{"id":"{id}","domain":"image","backend":"flux","available":true,"gated":false,"vram_gb":{gb},"state":"{state}"}}"#);
        ModelInfoJson::deserialize_json(&json).unwrap()
    }

    #[test]
    fn the_model_list_says_size_and_where_each_model_fits() {
        // 149: a 24 GB card with 22.3 GB usable; the full schnell is too big there.
        let health = HealthJson::deserialize_json(r#"{"service":"makepad-asset-ai","version":"0.3.0","gpu":"RTX 4090","vram_free_mb":22300,"vram_total_mb":24463,"vram_usable_mb":22300,"models_loaded":[],"vram_reserve_mb":2048}"#).unwrap();
        let snapshot = BoxSnapshot { base_url: "https://10.0.0.149:8123".into(), health: Some(health), models: vec![model("flux1-schnell", 21.0, "ready"), model("flux1-schnell-q4", 18.0, "ready")] };
        let choices = model_choices(&[snapshot], "image", "");
        assert_eq!(choices[0].id, "");
        assert!(choices[0].label.starts_with("auto"));
        assert_eq!(choices[1].id, "flux1-schnell-q4");
        assert!(choices[1].label.contains("18 GB") && choices[1].label.contains("fits"), "{}", choices[1].label);
        assert_eq!(choices[2].id, "flux1-schnell");
        assert!(choices[2].label.contains("too big for"), "{}", choices[2].label);
    }
}
