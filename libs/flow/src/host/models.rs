use crate::{FleetNodeDto, ModelInfoDto, ModelsResponse, NodeTypeCatalog};
use makepad_ai_hub::client::{ContentProvider, LocalService};
use makepad_ai_hub::protocol::{HealthJson, ModelInfoJson};
use makepad_ai_hub::{discovery, machine};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const SNAPSHOT_TTL: Duration = Duration::from_secs(10);
const NODE_BUDGET: Duration = Duration::from_millis(400);
const JOIN_CAP: Duration = Duration::from_millis(1_500);

fn matches_requested_domain(model_domain: &str, requested: Option<&str>) -> bool {
    requested.is_none_or(|domain| {
        model_domain == domain || (domain == "text" && model_domain == "chat")
    })
}

#[derive(Clone)]
struct Candidate {
    base_url: String,
    fleet: String,
}

struct ProbeResult {
    base_url: String,
    fallback_fleet: String,
    health: Option<HealthJson>,
    models: Option<Vec<ModelInfoJson>>,
}

pub(crate) struct FleetSnapshot {
    discovery: discovery::Discovery,
    nodes: Vec<FleetNodeDto>,
    models: Vec<ModelInfoDto>,
    snapshot_ms: u64,
    refreshed_at: Option<Instant>,
}

impl Default for FleetSnapshot {
    fn default() -> Self {
        Self {
            discovery: discovery::start_listener(),
            nodes: Vec::new(),
            models: Vec::new(),
            snapshot_ms: 0,
            refreshed_at: None,
        }
    }
}

impl FleetSnapshot {
    pub(crate) fn response(
        &mut self,
        fleet_hint: &[String],
        domain: Option<&str>,
    ) -> ModelsResponse {
        self.refresh_if_due(fleet_hint);
        let models = self
            .models
            .iter()
            .filter(|model| matches_requested_domain(&model.domain, domain))
            .cloned()
            .collect();
        ModelsResponse {
            nodes: self.nodes.clone(),
            models,
            snapshot_ms: self.snapshot_ms,
        }
    }

    pub(crate) fn catalog(
        &mut self,
        fleet_hint: &[String],
        catalog: &[NodeTypeCatalog],
    ) -> Vec<NodeTypeCatalog> {
        self.refresh_if_due(fleet_hint);
        let mut catalog = catalog.to_vec();
        for ty in &mut catalog {
            let Some(domain) = ty.domain.as_deref() else {
                continue;
            };
            ty.models = self
                .models
                .iter()
                .filter(|model| {
                    model.available && matches_requested_domain(&model.domain, Some(domain))
                })
                .map(|model| model.id.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
        }
        catalog
    }

    fn refresh_if_due(&mut self, fleet_hint: &[String]) {
        if self.refreshed_at.is_some_and(|at| at.elapsed() < SNAPSHOT_TTL) {
            return;
        }
        self.refresh(fleet_hint);
    }

    fn refresh(&mut self, fleet_hint: &[String]) {
        let candidates = candidates(fleet_hint, &self.discovery);
        let previous_nodes: HashMap<_, _> = self
            .nodes
            .iter()
            .map(|node| (node.base_url.clone(), node.clone()))
            .collect();
        let previous_models: HashMap<_, Vec<_>> = self.models.iter().cloned().fold(
            HashMap::new(),
            |mut by_node, model| {
                by_node
                    .entry(model.node.clone())
                    .or_insert_with(Vec::new)
                    .push(model);
                by_node
            },
        );

        let started = Instant::now();
        let (tx, rx) = mpsc::channel();
        let mut handles = Vec::new();
        for (index, candidate) in candidates.iter().cloned().enumerate() {
            let tx = tx.clone();
            let name = format!("flow-model-probe-{index}");
            if let Ok(handle) = std::thread::Builder::new().name(name).spawn(move || {
                let service = LocalService::new(&candidate.base_url);
                let health = service.health().ok();
                let models = service.list_models().ok();
                let _ = tx.send(ProbeResult {
                    base_url: candidate.base_url,
                    fallback_fleet: candidate.fleet,
                    health,
                    models,
                });
            }) {
                handles.push(handle);
            }
        }
        drop(tx);

        let result_deadline = started + NODE_BUDGET;
        let mut results = HashMap::new();
        while results.len() < handles.len() {
            let now = Instant::now();
            if now >= result_deadline {
                break;
            }
            match rx.recv_timeout(result_deadline - now) {
                Ok(result) => {
                    results.insert(result.base_url.clone(), result);
                }
                Err(_) => break,
            }
        }

        join_finished_until(&mut handles, started + JOIN_CAP);

        let mut nodes = Vec::with_capacity(candidates.len());
        let mut models = Vec::new();
        for candidate in candidates {
            if let Some(result) = results.remove(&candidate.base_url) {
                let fleet = result
                    .health
                    .as_ref()
                    .and_then(|health| health.fleet.as_deref())
                    .map(discovery::normalize_fleet)
                    .unwrap_or(result.fallback_fleet);
                let healthy = result.health.is_some();
                let gpu = result.health.as_ref().and_then(|health| health.gpu.clone());
                let vram_total_mb = result.health.as_ref().and_then(|health| health.vram_total_mb);
                let vram_usable_mb = result
                    .health
                    .as_ref()
                    .and_then(|health| health.vram_usable_mb);
                let vram_free_mb = result.health.as_ref().and_then(|health| health.vram_free_mb);
                let lanes_model = result
                    .health
                    .as_ref()
                    .and_then(|health| health.lanes.as_ref())
                    .map(|lanes| lanes.model.clone());
                let lanes = result
                    .health
                    .as_ref()
                    .and_then(|health| health.lanes.as_ref())
                    .map(|lanes| lanes.slots_total);
                nodes.push(FleetNodeDto {
                    base_url: candidate.base_url.clone(),
                    fleet,
                    healthy,
                    gpu,
                    vram_total_mb,
                    vram_usable_mb,
                    vram_free_mb,
                    lanes_model,
                    lanes,
                });
                if let Some(rows) = result.models {
                    models.extend(
                        rows.into_iter()
                            .filter(|model| listed_by_role(&candidate.base_url, &model.domain))
                            .map(|model| model_dto(&candidate.base_url, model)),
                    );
                }
            } else {
                let fleet = previous_nodes
                    .get(&candidate.base_url)
                    .map(|node| node.fleet.clone())
                    .unwrap_or(candidate.fleet);
                nodes.push(FleetNodeDto {
                    base_url: candidate.base_url.clone(),
                    fleet,
                    healthy: false,
                    gpu: previous_nodes
                        .get(&candidate.base_url)
                        .and_then(|node| node.gpu.clone()),
                    vram_total_mb: previous_nodes
                        .get(&candidate.base_url)
                        .and_then(|node| node.vram_total_mb),
                    vram_usable_mb: previous_nodes
                        .get(&candidate.base_url)
                        .and_then(|node| node.vram_usable_mb),
                    vram_free_mb: previous_nodes
                        .get(&candidate.base_url)
                        .and_then(|node| node.vram_free_mb),
                    lanes_model: previous_nodes
                        .get(&candidate.base_url)
                        .and_then(|node| node.lanes_model.clone()),
                    lanes: previous_nodes
                        .get(&candidate.base_url)
                        .and_then(|node| node.lanes),
                });
                if let Some(stale) = previous_models.get(&candidate.base_url) {
                    models.extend(stale.iter().cloned());
                }
            }
        }
        nodes.sort_by(|left, right| left.base_url.cmp(&right.base_url));
        models.sort_by(|left, right| {
            (&left.domain, &left.id, &left.node).cmp(&(&right.domain, &right.id, &right.node))
        });
        models.dedup_by(|left, right| {
            left.domain == right.domain && left.id == right.id && left.node == right.node
        });
        self.nodes = nodes;
        self.models = models;
        self.snapshot_ms = unix_ms();
        self.refreshed_at = Some(Instant::now());
    }
}

fn candidates(fleet_hint: &[String], discovered: &discovery::Discovery) -> Vec<Candidate> {
    let wanted_fleet = discovery::wanted_fleet();
    let mut by_url = BTreeMap::new();
    for base_url in fleet_hint {
        insert_candidate(&mut by_url, base_url, &wanted_fleet);
    }
    for (_, entry) in machine::read_node_entries() {
        if entry.port != 0 {
            insert_candidate(
                &mut by_url,
                &format!("http://127.0.0.1:{}", entry.port),
                &wanted_fleet,
            );
        }
    }
    for node in discovered.nodes() {
        insert_candidate(&mut by_url, &node.base_url, &node.fleet);
    }
    by_url.into_values().collect()
}

fn insert_candidate(candidates: &mut BTreeMap<String, Candidate>, base_url: &str, fleet: &str) {
    let base_url = base_url.trim().trim_end_matches('/');
    if base_url.is_empty() {
        return;
    }
    candidates.entry(base_url.to_string()).or_insert_with(|| Candidate {
        base_url: base_url.to_string(),
        fleet: discovery::normalize_fleet(fleet),
    });
}

/// A model a node's fleet role bars from serving is not listed for that
/// node: the hub's pickers never route there, so a picker counting it as
/// `ready` would promise a node the job cannot use (the `.165` incident:
/// the PRO 6000 holds flux2-dev ready but its role is chat/text only).
fn listed_by_role(base_url: &str, domain: &str) -> bool {
    makepad_ai_hub::fleet::role_allows(base_url, domain)
}

fn model_dto(node: &str, model: ModelInfoJson) -> ModelInfoDto {
    ModelInfoDto {
        id: model.id,
        // The hub advertises the conversational face of an LLM as `chat`.
        // Flow's public generation domain and Llm picker call that `text`.
        domain: if model.domain == "chat" { "text".to_string() } else { model.domain },
        backend: model.backend,
        node: node.to_string(),
        available: model.available,
        gated: model.gated,
        state: model.state,
        vram_gb: model.vram_gb,
        note: model.note,
    }
}

fn join_finished_until(handles: &mut Vec<JoinHandle<()>>, deadline: Instant) {
    while !handles.is_empty() && Instant::now() < deadline {
        let mut index = 0;
        let mut joined = false;
        while index < handles.len() {
            if handles[index].is_finished() {
                let handle = handles.swap_remove(index);
                let _ = handle.join();
                joined = true;
            } else {
                index += 1;
            }
        }
        if !joined && !handles.is_empty() {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{listed_by_role, matches_requested_domain};

    #[test]
    fn role_barred_models_are_not_listed_for_that_node() {
        // The built-in role table: 10.0.0.165 serves chat, text and image only.
        assert!(!listed_by_role("http://10.0.0.165:8123", "video"));
        assert!(listed_by_role("http://10.0.0.165:8123", "chat"));
        assert!(listed_by_role("http://10.0.0.165:8123", "image"));
        assert!(listed_by_role("http://10.0.0.217:8123", "image"));
    }

    #[test]
    fn text_picker_includes_chat_residency() {
        assert!(matches_requested_domain("text", Some("text")));
        assert!(matches_requested_domain("chat", Some("text")));
        assert!(!matches_requested_domain("image", Some("text")));
        assert!(!matches_requested_domain("chat", Some("image")));
    }
}

fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis() as u64
}
