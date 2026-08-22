//! Fleet discovery over `cx.http_request`: polls `GET /health` +
//! `GET /models` on every fleet endpoint and feeds the parsed JSON into
//! [`makepad_asset_ai::fleet::BoxSnapshot`]s — the scheduler
//! (`fleet::pick_box` / `pick_for_domain`) is pure over those snapshots.
//!
//! Endpoint lifecycle (the part that keeps the fleet free of duplicates):
//!
//! - Every row is LEASED from the LAN beacon set. Reconcile adds live
//!   beacons that alias no existing row and drops rows whose beacon lease
//!   expired.
//! - After health responses land, rows are COALESCED by durable identity:
//!   `node_key` (persisted per service instance, stable across restarts)
//!   first, live `node_id` (random per process start) as the fallback for
//!   services predating `node_key`. One service reachable under several
//!   addresses (127.0.0.1 + LAN ip) collapses to one row; a restart keeps
//!   the row because the beacon url and `node_key` both survive it.
//! - Rows are NEVER merged merely for sharing a host: two service
//!   instances on one box (e.g. 10.0.0.169:8123 and :8080) have distinct
//!   node_keys, distinct model sets and both stay. [`host_groups`] offers a
//!   display-only grouping so the UI can render one physical-node block
//!   with its service endpoints; scheduling still routes per endpoint.
//!
//! One request pair in flight per endpoint, so a down box (OS connect
//! timeout can be a minute) never stacks requests. In-flight requests are
//! keyed by url, not row index — rows may be coalesced away mid-flight and
//! the late response is then dropped instead of updating the wrong box.

use makepad_asset_ai::discovery::DiscoveredNode;
use makepad_asset_ai::fleet::BoxSnapshot;
use makepad_asset_ai::protocol::{HealthJson, JobStatusJson, JobsJson, LorasJson, ModelsJson};
use makepad_micro_serde::DeJson;
use makepad_widgets::*;
use std::collections::HashMap;

enum Pending {
    Health,
    Models,
    Jobs,
    Loras,
}

struct InFlight {
    pending: Pending,
    base_url: String,
    sent_at: std::time::Instant,
}

pub struct FleetPoll {
    pub snapshots: Vec<BoxSnapshot>,
    /// /health round-trip per endpoint, from the last completed poll.
    pub latency_ms: Vec<Option<u64>>,
    /// `GET /jobs` per endpoint (running first, then queued) — other
    /// clients' work included, so the UI can show and cancel it. Empty for
    /// services without the endpoint.
    pub jobs: Vec<Vec<JobStatusJson>>,
    /// `GET /loras` per endpoint: adapter names the box can apply to FLUX.1.
    /// Empty for services without the endpoint.
    pub loras: Vec<Vec<String>>,
    /// Endpoints with any request in flight (index-parallel to snapshots).
    busy: Vec<bool>,
    in_flight: HashMap<LiveId, InFlight>,
}

impl FleetPoll {
    pub fn new() -> Self {
        Self {
            snapshots: Vec::new(),
            latency_ms: Vec::new(),
            jobs: Vec::new(),
            loras: Vec::new(),
            busy: Vec::new(),
            in_flight: HashMap::new(),
        }
    }

    fn remove_row(&mut self, index: usize) {
        self.snapshots.remove(index);
        self.latency_ms.remove(index);
        self.jobs.remove(index);
        self.loras.remove(index);
        self.busy.remove(index);
    }

    fn row_by_url(&self, url: &str) -> Option<usize> {
        self.snapshots
            .iter()
            .position(|snapshot| snapshot.base_url == url)
    }

    fn live_node_id(&self, index: usize) -> Option<u64> {
        self.snapshots[index].health.as_ref().and_then(|h| h.node_id)
    }

    fn get(&mut self, cx: &mut Cx, base_url: String, url: String, pending: Pending) {
        let request_id = LiveId::unique();
        let request = crate::http::get(url);
        cx.http_request(request_id, request);
        self.in_flight.insert(
            request_id,
            InFlight {
                pending,
                base_url,
                sent_at: std::time::Instant::now(),
            },
        );
    }

    /// Reconcile against the FULL live beacon set: add live nodes that
    /// alias no existing row, and drop rows whose lease expired. A row
    /// survives by beacon url match or by live `node_id` match (its beacon
    /// source address may change while the identity holds). Returns true
    /// when a row was added or removed.
    pub fn reconcile_discovered(&mut self, nodes: &[DiscoveredNode]) -> bool {
        let mut changed = false;
        for node in nodes {
            let known = (0..self.snapshots.len()).any(|index| {
                self.snapshots[index].base_url == node.base_url
                    || self.live_node_id(index) == Some(node.node_id)
            });
            if !known {
                self.snapshots.push(BoxSnapshot::new(&node.base_url));
                self.latency_ms.push(None);
                self.jobs.push(Vec::new());
                self.loras.push(Vec::new());
                self.busy.push(false);
                changed = true;
            }
        }
        let mut index = 0;
        while index < self.snapshots.len() {
            let keep = nodes.iter().any(|node| {
                node.base_url == self.snapshots[index].base_url
                    || self.live_node_id(index) == Some(node.node_id)
            });
            if keep {
                index += 1;
            } else {
                self.remove_row(index);
                changed = true;
            }
        }
        if self.coalesce_aliases() {
            changed = true;
        }
        changed
    }

    /// Collapse rows that PROVE to be the same service instance: identical
    /// durable `node_key`, or identical live `node_id` (pre-node_key
    /// services). Sharing a host is deliberately NOT identity — same-box
    /// sibling instances (own cache dirs, own node_keys, own model sets)
    /// must both stay; see [`host_groups`] for the display-side grouping.
    /// The surviving row is the earlier index.
    fn coalesce_aliases(&mut self) -> bool {
        let mut changed = false;
        let mut index = 0;
        while index < self.snapshots.len() {
            let key = self.snapshots[index]
                .health
                .as_ref()
                .and_then(|h| h.node_key.clone());
            let id = self.live_node_id(index);
            let mut other = index + 1;
            while other < self.snapshots.len() {
                let other_key = self.snapshots[other]
                    .health
                    .as_ref()
                    .and_then(|h| h.node_key.clone());
                let other_id = self.live_node_id(other);
                let same_instance = (key.is_some() && key == other_key)
                    || (id.is_some() && id == other_id);
                if !same_instance {
                    other += 1;
                    continue;
                }
                log!(
                    "fleet: {} is the same service as {} — coalesced",
                    self.snapshots[other].base_url,
                    self.snapshots[index].base_url
                );
                self.remove_row(other);
                changed = true;
            }
            if other >= self.snapshots.len() {
                index += 1;
            }
        }
        changed
    }

    /// Kick a health+models round for every endpoint without one in flight.
    /// Call from an interval timer.
    pub fn poll(&mut self, cx: &mut Cx) {
        for index in 0..self.snapshots.len() {
            if self.busy[index] {
                continue;
            }
            self.busy[index] = true;
            let base_url = self.snapshots[index].base_url.clone();
            let url = format!("{base_url}/health");
            self.get(cx, base_url, url, Pending::Health);
        }
    }

    /// Routes one NetworkResponse; returns true when a snapshot changed
    /// (caller redraws the fleet panel). A response for a row that was
    /// coalesced or lease-expired while the request flew is dropped.
    pub fn handle_response(&mut self, cx: &mut Cx, item: &NetworkResponse) -> bool {
        let (request_id, response) = match item {
            NetworkResponse::HttpResponse {
                request_id,
                response,
            } => (*request_id, Some(response)),
            NetworkResponse::HttpError { request_id, .. } => (*request_id, None),
            _ => return false,
        };
        let Some(in_flight) = self.in_flight.remove(&request_id) else {
            return false;
        };
        let Some(index) = self.row_by_url(&in_flight.base_url) else {
            return false;
        };
        match in_flight.pending {
            Pending::Health => {
                let health = response
                    .filter(|r| r.status_code == 200)
                    .and_then(|r| r.get_string_body())
                    .and_then(|body| HealthJson::deserialize_json_lenient(&body).ok());
                let up = health.is_some();
                self.latency_ms[index] =
                    up.then(|| in_flight.sent_at.elapsed().as_millis() as u64);
                self.snapshots[index].health = health;
                if up {
                    // A fresh health may prove this row aliases another
                    // (same node_key/node_id) — collapse before fetching
                    // models so the duplicate never renders.
                    self.coalesce_aliases();
                    if let Some(index) = self.row_by_url(&in_flight.base_url) {
                        let base_url = self.snapshots[index].base_url.clone();
                        let url = format!("{base_url}/models");
                        self.get(cx, base_url, url, Pending::Models);
                    }
                } else {
                    self.snapshots[index].models.clear();
                    self.busy[index] = false;
                }
                true
            }
            Pending::Models => {
                self.busy[index] = false;
                let models = match response {
                    Some(response) if response.status_code == 200 => {
                        match response.get_string_body() {
                            Some(body) => match ModelsJson::deserialize_json_lenient(&body) {
                                Ok(models) => Some(models),
                                Err(error) => {
                                    log!(
                                        "fleet: {} /models JSON rejected ({} bytes): {:?}",
                                        in_flight.base_url,
                                        body.len(),
                                        error
                                    );
                                    None
                                }
                            },
                            None => {
                                log!("fleet: {} /models returned no text body", in_flight.base_url);
                                None
                            }
                        }
                    }
                    Some(response) => {
                        log!(
                            "fleet: {} /models returned HTTP {}",
                            in_flight.base_url,
                            response.status_code
                        );
                        None
                    }
                    None => {
                        log!("fleet: {} /models request failed", in_flight.base_url);
                        None
                    }
                };
                if let Some(models) = models {
                    self.snapshots[index].models = models.models;
                }
                // Live job list last (running + queued, other clients too).
                let base_url = self.snapshots[index].base_url.clone();
                let url = format!("{base_url}/jobs");
                self.busy[index] = true;
                self.get(cx, base_url, url, Pending::Jobs);
                true
            }
            Pending::Jobs => {
                self.busy[index] = false;
                let jobs = response
                    .filter(|r| r.status_code == 200)
                    .and_then(|r| r.get_string_body())
                    .and_then(|body| JobsJson::deserialize_json_lenient(&body).ok())
                    .map(|j| j.jobs)
                    // Older services (no /jobs) → nothing to show, not an error.
                    .unwrap_or_default();
                let changed = self.jobs[index].len() != jobs.len()
                    || self.jobs[index]
                        .iter()
                        .zip(&jobs)
                        .any(|(a, b)| a.job_id != b.job_id || a.state != b.state || a.stage != b.stage);
                self.jobs[index] = jobs;
                // LoRA inventory after the job list (cheap directory listing).
                let base_url = self.snapshots[index].base_url.clone();
                let url = format!("{base_url}/loras");
                self.busy[index] = true;
                self.get(cx, base_url, url, Pending::Loras);
                changed
            }
            Pending::Loras => {
                self.busy[index] = false;
                let loras: Vec<String> = response
                    .filter(|r| r.status_code == 200)
                    .and_then(|r| r.get_string_body())
                    .and_then(|body| LorasJson::deserialize_json_lenient(&body).ok())
                    .map(|l| l.loras.into_iter().map(|l| l.name).collect())
                    .unwrap_or_default();
                let changed = self.loras[index] != loras;
                self.loras[index] = loras;
                changed
            }
        }
    }

    /// Every LoRA name any up endpoint offers (sorted, deduped).
    pub fn all_loras(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .snapshots
            .iter()
            .zip(&self.loras)
            .filter(|(snap, _)| snap.is_up())
            .flat_map(|(_, loras)| loras.iter().cloned())
            .collect();
        names.sort();
        names.dedup();
        names
    }

    /// Fleet panel text: one block per physical host, its service
    /// endpoints indented under it with queue depth, latency and per-model
    /// loaded/ready badges.
    pub fn panel_text(&self) -> String {
        let mut out = String::new();
        for (host, rows) in host_groups(&self.snapshots) {
            let any_up = rows.iter().any(|&row| self.snapshots[row].is_up());
            out.push_str(&format!("{} {}\n", if any_up { "●" } else { "○" }, host));
            for row in rows {
                let snap = &self.snapshots[row];
                let port = port_of(&snap.base_url);
                match &snap.health {
                    Some(health) => {
                        let latency = self.latency_ms[row]
                            .map(|ms| format!("{ms}ms"))
                            .unwrap_or_default();
                        out.push_str(&format!(
                            "  :{port}   {}   queue {}   {latency}\n",
                            health.gpu.as_deref().unwrap_or("no gpu info"),
                            snap.jobs_pending(),
                        ));
                        if let (Some(free), Some(total)) =
                            (health.vram_free_mb, health.vram_total_mb)
                        {
                            out.push_str(&format!("      vram {free}/{total} MB\n"));
                        }
                        if snap.models.is_empty() {
                            out.push_str("      (no models reported)\n");
                        }
                        for model in &snap.models {
                            if !model.available {
                                continue;
                            }
                            let badge = match model.state.as_str() {
                                "loaded" => "◆ loaded",
                                "ready" => "◇ ready",
                                other => other,
                            };
                            out.push_str(&format!(
                                "      {:<7} {}  {}\n",
                                model.domain, model.id, badge
                            ));
                        }
                    }
                    None => {
                        out.push_str(&format!("  :{port}   offline\n"));
                    }
                }
            }
            out.push('\n');
        }
        out
    }
}

/// Host part of a fleet base url ("http://10.0.0.169:8123" → "10.0.0.169").
pub fn host_of(base_url: &str) -> &str {
    let stripped = base_url
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    stripped
        .rsplit_once(':')
        .map_or(stripped, |(host, _port)| host)
}

pub fn port_of(base_url: &str) -> &str {
    let stripped = base_url
        .trim_start_matches("http://")
        .trim_start_matches("https://");
    stripped.rsplit_once(':').map_or("80", |(_host, port)| port)
}

/// DISPLAY-ONLY grouping of fleet rows by host, in first-appearance order.
/// This never merges data: each row keeps its own health, queue and model
/// set — the group exists so the UI can render one physical-node block with
/// several service endpoints instead of look-alike duplicate rows.
pub fn host_groups(snapshots: &[BoxSnapshot]) -> Vec<(String, Vec<usize>)> {
    let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
    for (index, snapshot) in snapshots.iter().enumerate() {
        let host = host_of(&snapshot.base_url).to_string();
        match groups.iter_mut().find(|(name, _)| *name == host) {
            Some((_, rows)) => rows.push(index),
            None => groups.push((host, vec![index])),
        }
    }
    groups
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fleet(urls: &[&str]) -> FleetPoll {
        let mut fleet = FleetPoll::new();
        if !urls.is_empty() {
            let nodes: Vec<DiscoveredNode> = urls
                .iter()
                .enumerate()
                .map(|(index, url)| discovered(1000 + index as u64, url))
                .collect();
            fleet.reconcile_discovered(&nodes);
        }
        fleet
    }

    /// Health via the same lenient JSON path real responses take, so tests
    /// keep passing when the (concurrently evolving) protocol gains fields.
    fn health(node_id: u64, node_key: &str) -> HealthJson {
        HealthJson::deserialize_json_lenient(&format!(
            r#"{{"service":"makepad-asset-ai","version":"test","models_loaded":[],"node_id":{node_id},"node_key":"{node_key}"}}"#
        ))
        .expect("test health json parses")
    }

    fn discovered(node_id: u64, base_url: &str) -> DiscoveredNode {
        DiscoveredNode {
            base_url: base_url.to_string(),
            node_id,
            fleet: makepad_asset_ai::discovery::DEFAULT_FLEET.to_string(),
        }
    }

    #[test]
    fn same_node_id_via_two_addresses_stays_one_row() {
        // First beacon answered health with node_id 11; the same service
        // also beacons from its LAN address. No second row.
        let mut fleet = FleetPoll::new();
        fleet.reconcile_discovered(&[discovered(11, "http://127.0.0.1:8768")]);
        fleet.snapshots[0].health = Some(health(11, "aaaa"));
        let changed =
            fleet.reconcile_discovered(&[discovered(11, "http://10.0.0.4:8768")]);
        assert!(!changed);
        assert_eq!(fleet.snapshots.len(), 1);
        assert_eq!(fleet.snapshots[0].base_url, "http://127.0.0.1:8768");
    }

    #[test]
    fn same_node_key_via_two_addresses_coalesces() {
        // Health had not yet arrived when a second address joined, so a
        // duplicate row briefly exists — the node_key coalesce collapses it.
        let mut fleet = FleetPoll::new();
        fleet.reconcile_discovered(&[
            discovered(1, "http://127.0.0.1:8768"),
            discovered(2, "http://10.0.0.4:8768"),
        ]);
        assert_eq!(fleet.snapshots.len(), 2);
        fleet.snapshots[0].health = Some(health(11, "aaaa"));
        fleet.snapshots[1].health = Some(health(11, "aaaa"));
        assert!(fleet.coalesce_aliases());
        assert_eq!(fleet.snapshots.len(), 1);
        assert_eq!(fleet.snapshots[0].base_url, "http://127.0.0.1:8768");
    }

    #[test]
    fn restart_with_new_node_id_keeps_one_row() {
        // A discovered service restarts: node_id changes, url + node_key
        // stay. The old row must be reused, not duplicated and not dropped.
        let mut fleet = fleet(&[]);
        fleet.reconcile_discovered(&[discovered(11, "http://10.0.0.9:8767")]);
        fleet.snapshots[0].health = Some(health(11, "cafe"));
        let changed =
            fleet.reconcile_discovered(&[discovered(22, "http://10.0.0.9:8767")]);
        assert!(!changed);
        assert_eq!(fleet.snapshots.len(), 1);
        // The next health refresh reports the new node_id; still one row.
        fleet.snapshots[0].health = Some(health(22, "cafe"));
        assert!(!fleet.coalesce_aliases());
        assert_eq!(fleet.snapshots.len(), 1);
    }

    #[test]
    fn lease_expiry_removes_discovered() {
        let mut fleet = FleetPoll::new();
        fleet.reconcile_discovered(&[discovered(33, "http://10.0.0.9:8767")]);
        assert_eq!(fleet.snapshots.len(), 1);
        let changed = fleet.reconcile_discovered(&[]);
        assert!(changed);
        assert!(fleet.snapshots.is_empty());
        assert!(!fleet.reconcile_discovered(&[]));
    }

    #[test]
    fn two_distinct_instances_on_one_host_never_merge() {
        // Same box, two service instances with their own cache dirs: host
        // grouping is display-only; the rows (and their future model sets)
        // must both survive every reconcile + coalesce pass.
        let nodes = [
            discovered(41, "http://10.0.0.169:8123"),
            discovered(42, "http://10.0.0.169:8080"),
        ];
        let mut fleet = FleetPoll::new();
        fleet.reconcile_discovered(&nodes);
        fleet.snapshots[0].health = Some(health(41, "feed01"));
        fleet.snapshots[1].health = Some(health(42, "feed02"));
        assert!(!fleet.coalesce_aliases());
        assert!(!fleet.reconcile_discovered(&nodes));
        assert_eq!(fleet.snapshots.len(), 2);
        let groups = host_groups(&fleet.snapshots);
        assert_eq!(groups.len(), 1, "one physical host block for display");
        assert_eq!(groups[0].0, "10.0.0.169");
        assert_eq!(groups[0].1, vec![0, 1], "both service endpoints listed");
    }

    #[test]
    fn pre_node_key_services_coalesce_by_live_node_id() {
        // Services predating node_key: the live node_id is the only proof
        // two urls are one process — documented fallback identity.
        let mut fleet = fleet(&["http://127.0.0.1:8768", "http://10.0.0.4:8768"]);
        fleet.snapshots[0].health =
            Some(health(55, "")); // node_key "" parses to Some("") — use distinct json
        fleet.snapshots[0].health = HealthJson::deserialize_json_lenient(
            r#"{"service":"makepad-asset-ai","version":"t","models_loaded":[],"node_id":55}"#,
        )
        .ok();
        fleet.snapshots[1].health = HealthJson::deserialize_json_lenient(
            r#"{"service":"makepad-asset-ai","version":"t","models_loaded":[],"node_id":55}"#,
        )
        .ok();
        assert!(fleet.coalesce_aliases());
        assert_eq!(fleet.snapshots.len(), 1);
        assert_eq!(fleet.snapshots[0].base_url, "http://127.0.0.1:8768");
    }
}
