//! Which models a preset needs and whether the AI Hub has them. The hub's
//! `GET /models` is the truth when a hub answers; otherwise the embedded
//! registry and this machine's weights folder say what is installed here.
//! Probing happens on the studio worker; this module only interprets.

use super::presets::ModelNeed;
use makepad_ai_hub::protocol::ModelInfoJson;
use makepad_ai_hub::registry::{LicenseRestriction, Registry};
use std::path::PathBuf;

/// How generation reaches a hub.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenMode {
    /// The discovered fleet, as the workbench and Asset UI route.
    Fleet,
    /// One pinned hub (`STAGE_FLOW_GEN=<url>`).
    Fixed(String),
    /// The in-process preview generator (`STAGE_FLOW_GEN=preview`): test
    /// pictures and tones, no hub, no network.
    Preview,
}

impl GenMode {
    pub fn from_env() -> GenMode {
        match std::env::var("STAGE_FLOW_GEN").ok().map(|value| value.trim().to_string()).filter(|value| !value.is_empty()) {
            None => GenMode::Fleet,
            Some(value) if value == "preview" => GenMode::Preview,
            Some(value) => GenMode::Fixed(value.trim_end_matches('/').to_string()),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            GenMode::Fleet => "AI Hub fleet".into(),
            GenMode::Fixed(url) => format!("AI Hub {url}"),
            GenMode::Preview => "preview generator (no hub)".into(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelState {
    Loaded,
    Ready,
    Downloading,
    Absent,
    Unavailable,
    TooSmall,
    Error,
}

impl ModelState {
    pub fn parse(state: &str, available: bool) -> ModelState {
        if !available {
            return ModelState::Unavailable;
        }
        match state {
            "loaded" => ModelState::Loaded,
            "ready" => ModelState::Ready,
            "downloading" => ModelState::Downloading,
            "too_small" => ModelState::TooSmall,
            "error" => ModelState::Error,
            _ => ModelState::Absent,
        }
    }

    pub fn installed(self) -> bool {
        matches!(self, ModelState::Loaded | ModelState::Ready)
    }
}

#[derive(Clone, Debug)]
pub struct HubModel {
    /// The node that reported it (`fleet::node_label`).
    pub node: String,
    pub id: String,
    pub domain: String,
    pub state: ModelState,
    pub detail: String,
}

impl HubModel {
    pub fn from_info(node: &str, info: &ModelInfoJson) -> HubModel {
        let state = ModelState::parse(&info.state, info.available);
        let detail = match state {
            ModelState::Downloading => match (info.progress_done, info.progress_total) {
                (Some(done), Some(total)) if total > 0 => format!("downloading {}%", done * 100 / total),
                _ => "downloading".into(),
            },
            ModelState::Unavailable => info.unavailable_reason.clone().unwrap_or_else(|| "unavailable on this hub".into()),
            ModelState::Error => info.error.clone().unwrap_or_else(|| "error".into()),
            ModelState::TooSmall => "this hub's GPU is too small".into(),
            _ => String::new(),
        };
        HubModel { node: node.to_string(), id: info.id.clone(), domain: info.domain.clone(), state, detail }
    }
}

/// What the studio knows about the hub(s), refreshed while FLOW shows.
#[derive(Clone, Debug, Default)]
pub struct HubSnapshot {
    pub mode: Option<GenMode>,
    /// Hubs that answered, as `host:port`.
    pub hubs: Vec<String>,
    pub models: Vec<HubModel>,
    /// Registry models whose files are on this machine.
    pub local: Vec<String>,
    /// Each node that answered, as the fleet scorer reads it (ETAs).
    pub boxes: Vec<makepad_ai_hub::fleet::BoxSnapshot>,
    pub error: Option<String>,
}

impl HubSnapshot {
    pub fn preview(&self) -> bool {
        self.mode == Some(GenMode::Preview)
    }

    pub fn line(&self) -> String {
        match &self.mode {
            None => "checking the AI Hub…".into(),
            Some(GenMode::Preview) => "preview generator: test pictures and tones, nothing leaves this machine".into(),
            Some(mode) if self.hubs.is_empty() => format!("{}: no node answers{}{}", mode.describe(),
                self.error.as_deref().map(|error| format!(" ({error})")).unwrap_or_default(),
                if *mode == GenMode::Fleet { "; generators run as previews" } else { "" }),
            Some(mode) => {
                let mut ids: Vec<&str> = self.models.iter().filter(|model| model.state.installed()).map(|model| model.id.as_str()).collect();
                ids.sort();
                ids.dedup();
                let nodes = if self.hubs.len() == 1 { "1 node".to_string() } else { format!("{} nodes", self.hubs.len()) };
                format!("{}: {nodes} ({}) · {} models installed", mode.describe(), self.hubs.join(", "), ids.len())
            }
        }
    }
}

/// One need, resolved against the snapshot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NeedState {
    /// Installed on nodes that answered: the model that would run, and
    /// every node that has it.
    Ready { model: String, nodes: Vec<String> },
    /// The model is not installed on any hub that answered (or not known).
    Missing(String),
    /// No hub answered; `true` when the weights are on this machine.
    NoHub(bool),
    Preview,
}

impl NeedState {
    pub fn ok(&self) -> bool {
        matches!(self, NeedState::Ready { .. } | NeedState::Preview)
    }
}

pub fn need_state(need: &ModelNeed, snapshot: &HubSnapshot) -> NeedState {
    if snapshot.preview() {
        return NeedState::Preview;
    }
    let matches = |model: &&HubModel| if need.model.is_empty() { model.domain == need.domain } else { model.id == need.model };
    if snapshot.hubs.is_empty() {
        let local = if need.model.is_empty() {
            registry().models.iter().any(|spec| spec.domain.as_str() == need.domain && snapshot.local.contains(&spec.id))
        } else {
            snapshot.local.contains(&need.model)
        };
        return NeedState::NoHub(local);
    }
    let mut candidates: Vec<&HubModel> = snapshot.models.iter().filter(matches).collect();
    candidates.sort_by_key(|model| match model.state {
        ModelState::Loaded => 0,
        ModelState::Ready => 1,
        ModelState::Downloading => 2,
        _ => 3,
    });
    match candidates.first() {
        Some(model) if model.state.installed() => {
            let mut nodes: Vec<String> = snapshot.models.iter()
                .filter(|other| other.id == model.id && other.state.installed()).map(|other| other.node.clone()).collect();
            nodes.sort();
            nodes.dedup();
            NeedState::Ready { model: model.id.clone(), nodes }
        }
        Some(model) => NeedState::Missing(format!("{} {}", model.id, if model.detail.is_empty() { "not installed".to_string() } else { model.detail.clone() })),
        None if need.model.is_empty() => NeedState::Missing(format!("no {} model on the hub", need.domain)),
        None => NeedState::Missing(format!("{} is not on the hub", need.model)),
    }
}

/// A preset's readiness, for its card badge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    Ready,
    /// Needs that are not installed, by model or domain.
    Missing(Vec<String>),
    NoHub,
    Preview,
    /// Nothing to generate (utility flows).
    Local,
}

impl Readiness {
    pub fn badge(&self) -> &'static str {
        match self {
            Readiness::Ready => "READY",
            Readiness::Missing(_) => "NOT INSTALLED",
            Readiness::NoHub => "NO HUB",
            Readiness::Preview => "PREVIEW",
            Readiness::Local => "LOCAL",
        }
    }
}

/// The nodes that can run every generator of a preset (empty when none can,
/// or when nothing is generated).
pub fn nodes_for(needs: &[ModelNeed], snapshot: &HubSnapshot) -> Vec<String> {
    let mut common: Option<Vec<String>> = None;
    for need in needs {
        let NeedState::Ready { nodes, .. } = need_state(need, snapshot) else { return Vec::new() };
        common = Some(match common {
            None => nodes,
            Some(before) => before.into_iter().filter(|node| nodes.contains(node)).collect(),
        });
    }
    common.unwrap_or_default()
}

pub fn readiness(needs: &[ModelNeed], snapshot: &HubSnapshot) -> Readiness {
    if needs.is_empty() {
        return Readiness::Local;
    }
    if snapshot.preview() {
        return Readiness::Preview;
    }
    if snapshot.hubs.is_empty() {
        return Readiness::NoHub;
    }
    let missing: Vec<String> = needs.iter().filter(|need| !need_state(need, snapshot).ok())
        .map(|need| if need.model.is_empty() { need.domain.clone() } else { need.model.clone() }).collect();
    if missing.is_empty() { Readiness::Ready } else { Readiness::Missing(missing) }
}

/// The domain a model serves: the hubs' own answer when a node reported
/// it, else the registry's.
pub fn model_domain(model: &str, snapshot: &HubSnapshot) -> Option<makepad_ai_hub::registry::Domain> {
    snapshot.models.iter().find(|candidate| candidate.id == model).and_then(|candidate| makepad_ai_hub::registry::Domain::parse(&candidate.domain))
        .or_else(|| registry().find(model).map(|spec| spec.domain))
}

/// Why a preset's generators cannot run as the hubs stand (see
/// `makepad_flow`'s `graph_problems`).
pub fn graph_problems(graph: &makepad_flow::Graph, snapshot: &HubSnapshot) -> Vec<String> {
    makepad_flow::engine::executors::gen::graph_problems(graph, &|model| model_domain(model, snapshot))
}

/// The embedded registry, parsed once.
pub fn registry() -> &'static Registry {
    static REGISTRY: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| Registry::embedded().unwrap_or_default())
}

/// A pinned model's licence: its name, and a warning when it restricts use.
pub fn license(model: &str) -> Option<(String, Option<&'static str>)> {
    let license = registry().find(model)?.license.clone()?;
    let warning = match license.restriction {
        LicenseRestriction::None => None,
        LicenseRestriction::NonCommercial => Some("non-commercial"),
        LicenseRestriction::Community => Some("community licence"),
        LicenseRestriction::Restricted => Some("restricted"),
    };
    Some((license.name, warning))
}

/// Where the hub keeps weights on this machine (read only; never created).
pub fn weights_dir() -> PathBuf {
    std::env::var_os("MAKEPAD_ASSET_AI_CACHE").filter(|dir| !dir.is_empty()).map(PathBuf::from)
        .unwrap_or_else(|| makepad_ai_hub::home::makepad_home().join("weights"))
}

/// Registry models whose required files are all on this machine. File
/// stats only; run it on a worker.
pub fn local_models() -> Vec<String> {
    let dir = weights_dir();
    if !dir.is_dir() {
        return Vec::new();
    }
    registry().models.iter().filter(|spec| !spec.files.is_empty() && spec.files_present(&dir)).map(|spec| spec.id.clone()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn need(domain: &str, model: &str) -> ModelNeed {
        ModelNeed { node: "n".into(), domain: domain.into(), model: model.into() }
    }

    fn hub(models: &[(&str, &str, ModelState)]) -> HubSnapshot {
        HubSnapshot {
            mode: Some(GenMode::Fleet),
            hubs: vec!["127.0.0.1:8765".into()],
            models: models.iter().map(|(id, domain, state)| HubModel { node: "gpu-a".into(), id: id.to_string(), domain: domain.to_string(), state: *state, detail: String::new() }).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn auto_needs_take_the_best_installed_model_of_the_domain() {
        let snapshot = hub(&[("flux1-dev", "image", ModelState::Absent), ("flux2-klein-4b", "image", ModelState::Ready)]);
        assert_eq!(need_state(&need("image", ""), &snapshot), NeedState::Ready { model: "flux2-klein-4b".into(), nodes: vec!["gpu-a".into()] });
        assert_eq!(nodes_for(&[need("image", "")], &snapshot), vec!["gpu-a".to_string()]);
        assert!(nodes_for(&[need("image", "flux1-dev")], &snapshot).is_empty());
        assert!(matches!(need_state(&need("image", "flux1-dev"), &snapshot), NeedState::Missing(_)));
        assert_eq!(readiness(&[need("image", "flux1-dev"), need("image", "")], &snapshot), Readiness::Missing(vec!["flux1-dev".into()]));
    }

    #[test]
    fn no_hub_and_preview_are_their_own_states() {
        let none = HubSnapshot { mode: Some(GenMode::Fleet), ..Default::default() };
        assert_eq!(readiness(&[need("music", "")], &none), Readiness::NoHub);
        let preview = HubSnapshot { mode: Some(GenMode::Preview), ..Default::default() };
        assert_eq!(readiness(&[need("music", "")], &preview), Readiness::Preview);
        assert_eq!(readiness(&[], &none), Readiness::Local);
    }

    #[test]
    fn restricted_licences_are_flagged() {
        // flux1-dev is released under a non-commercial licence.
        let (_, warning) = license("flux1-dev").expect("flux1-dev has a licence record");
        assert!(warning.is_some());
    }

    #[test]
    fn model_states_parse() {
        assert_eq!(ModelState::parse("loaded", true), ModelState::Loaded);
        assert_eq!(ModelState::parse("ready", false), ModelState::Unavailable);
        assert_eq!(ModelState::parse("absent", true), ModelState::Absent);
    }
}
