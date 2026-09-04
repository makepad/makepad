//! Fleet-backed capacity estimate for a flow's pinned generation models.

use super::state::FlowState;
use crate::{Graph, Literal, ModelsResponse, ParallelismNodeDto, ParallelismResponse};
use std::collections::BTreeSet;

impl FlowState {
    pub(crate) fn parallelism(&mut self, flow: &str) -> Option<ParallelismResponse> {
        let graph = self.definitions.get(flow)?.graph.clone()?;
        let snapshot = self.models_response(None);
        Some(estimate_parallelism(&graph, &snapshot))
    }
}

/// Estimate independent whole-flow runs. This is deliberately a snapshot,
/// not a reservation: jobs beyond the estimate still go to the hub queue.
pub fn estimate_parallelism(graph: &Graph, snapshot: &ModelsResponse) -> ParallelismResponse {
    let mut per_node = Vec::new();
    let mut gen_capacity = Vec::new();
    let mut chat_capacity = Vec::new();
    for node in graph
        .nodes
        .iter()
        .filter(|node| matches!(node.kind.as_str(), "gen" | "chat"))
    {
        let domain = node.domain.as_deref().unwrap_or(if node.kind == "chat" {
            "text"
        } else {
            ""
        });
        let requested = node.params.iter().find_map(|(name, value)| {
            (name == "model").then_some(value).and_then(|value| match value {
                Literal::Str(model) | Literal::Id(model) if !model.is_empty() => {
                    Some(model.clone())
                }
                _ => None,
            })
        });
        let model = requested.unwrap_or_else(|| resolve_domain_model(snapshot, domain));
        let admitted = admitted_nodes(snapshot, &model, domain);
        let nodes = admitted.len() as u64;
        let lanes = if node.kind == "chat" {
            admitted
                .iter()
                .map(|base_url| {
                    snapshot
                        .nodes
                        .iter()
                        .find(|node| node.base_url == *base_url)
                        .and_then(|node| {
                            (node.lanes_model.as_deref() == Some(model.as_str()))
                                .then_some(node.lanes)
                                .flatten()
                        })
                        .unwrap_or(1)
                        .max(1)
                })
                .sum()
        } else {
            nodes
        };
        if node.kind == "gen" {
            gen_capacity.push(nodes);
        } else {
            chat_capacity.push(lanes);
        }
        per_node.push(ParallelismNodeDto {
            node: node.id.clone(),
            model,
            nodes,
            lanes,
        });
    }
    // The heavy generation stages bound a batch: with the chat model pinned
    // to one box and the image model on every other box, four boxes mean
    // four runs, and the short chat stage simply serialises on its box.
    // Chat lanes bound the estimate only when the flow generates nothing.
    let max = gen_capacity
        .into_iter()
        .min()
        .or_else(|| chat_capacity.into_iter().min())
        .unwrap_or(1)
        .max(1);
    ParallelismResponse { max, per_node }
}

fn resolve_domain_model(snapshot: &ModelsResponse, domain: &str) -> String {
    let mut ids = BTreeSet::new();
    for model in snapshot.models.iter().filter(|model| {
        model.available && domain_matches(&model.domain, domain)
    }) {
        ids.insert(model.id.clone());
    }
    let mut chosen = String::new();
    let mut chosen_score = (0u8, 0u64);
    for model in ids {
        let rows: Vec<_> = snapshot
            .models
            .iter()
            .filter(|row| row.id == model && row.available)
            .collect();
        let readiness = rows
            .iter()
            .map(|row| readiness_rank(&row.state))
            .max()
            .unwrap_or(0);
        let capacity = admitted_nodes(snapshot, &model, domain).len() as u64;
        let score = (readiness, capacity);
        if chosen.is_empty() || score > chosen_score {
            chosen = model;
            chosen_score = score;
        }
    }
    chosen
}

fn admitted_nodes(snapshot: &ModelsResponse, model: &str, domain: &str) -> Vec<String> {
    if model.is_empty() {
        return Vec::new();
    }
    let healthy: BTreeSet<&str> = snapshot
        .nodes
        .iter()
        .filter(|node| node.healthy)
        .map(|node| node.base_url.as_str())
        .collect();
    let collect = |fallback: bool| {
        snapshot
            .models
            .iter()
            .filter(|row| {
                row.id == model
                    && row.available
                    && domain_matches(&row.domain, domain)
                    && healthy.contains(row.node.as_str())
                    && if fallback {
                        row.state == makepad_ai_hub::protocol::MODEL_STATE_ABSENT
                    } else {
                        matches!(
                            row.state.as_str(),
                            makepad_ai_hub::protocol::MODEL_STATE_READY
                                | makepad_ai_hub::protocol::MODEL_STATE_LOADED
                        )
                    }
            })
            .map(|row| row.node.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>()
    };
    let ready = collect(false);
    if ready.is_empty() {
        collect(true)
    } else {
        ready
    }
}

fn readiness_rank(state: &str) -> u8 {
    match state {
        makepad_ai_hub::protocol::MODEL_STATE_LOADED => 3,
        makepad_ai_hub::protocol::MODEL_STATE_READY => 2,
        makepad_ai_hub::protocol::MODEL_STATE_ABSENT => 1,
        _ => 0,
    }
}

fn domain_matches(actual: &str, requested: &str) -> bool {
    actual == requested || (requested == "text" && actual == "chat")
}

#[cfg(test)]
mod tests {
    use super::estimate_parallelism;
    use crate::{FleetNodeDto, ModelInfoDto, ModelsResponse};

    fn node(url: &str, lanes_model: Option<&str>, lanes: Option<u64>) -> FleetNodeDto {
        FleetNodeDto {
            base_url: url.into(),
            fleet: "test".into(),
            healthy: true,
            gpu: None,
            vram_total_mb: None,
            vram_usable_mb: None,
            vram_free_mb: None,
            lanes_model: lanes_model.map(str::to_string),
            lanes,
        }
    }

    fn model(id: &str, domain: &str, node: &str, state: &str) -> ModelInfoDto {
        ModelInfoDto {
            id: id.into(),
            domain: domain.into(),
            backend: "fake".into(),
            node: node.into(),
            available: true,
            gated: false,
            state: state.into(),
            vram_gb: None,
            note: None,
        }
    }

    #[test]
    fn two_models_take_the_min_and_absent_is_only_a_fallback() {
        let source = r#"use mod.flow.*
let a = Image{model: "a"}
let b = Gen{domain: "video" model: "b" ports: {in: {} out: {video: @video}}}
Flow{a, b}
"#;
        let graph = crate::graph::evaluate(source, "parallel.splash").unwrap();
        let nodes = vec![node("n1", None, None), node("n2", None, None), node("n3", None, None)];
        let models = vec![
            model("a", "image", "n1", "ready"),
            model("a", "image", "n2", "ready"),
            model("a", "image", "n3", "absent"),
            model("b", "video", "n1", "ready"),
        ];
        let estimate = estimate_parallelism(&graph, &ModelsResponse { nodes, models, snapshot_ms: 1 });
        assert_eq!(estimate.max, 1);
        assert_eq!(estimate.per_node[0].nodes, 2);
        assert_eq!(estimate.per_node[1].nodes, 1);
    }

    #[test]
    fn one_chat_box_does_not_cap_the_image_fleet() {
        let source = "use mod.flow.*\nlet talk = Llm{model: \"chat-a\"}\nlet picture = Image{model: \"a\"}\nFlow{talk, picture}\n";
        let graph = crate::graph::evaluate(source, "pinned.splash").unwrap();
        let snapshot = ModelsResponse {
            nodes: vec![
                node("n1", Some("chat-a"), Some(1)),
                node("n2", None, None),
                node("n3", None, None),
                node("n4", None, None),
            ],
            models: vec![
                model("chat-a", "text", "n1", "loaded"),
                model("a", "image", "n2", "ready"),
                model("a", "image", "n3", "ready"),
                model("a", "image", "n4", "ready"),
            ],
            snapshot_ms: 1,
        };
        let estimate = estimate_parallelism(&graph, &snapshot);
        assert_eq!(estimate.max, 3);
        assert_eq!(estimate.per_node[0].lanes, 1);
        assert_eq!(estimate.per_node[1].nodes, 3);
    }

    #[test]
    fn chat_sums_lanes_and_an_empty_fleet_floors_at_one() {
        let graph = crate::graph::evaluate(
            "use mod.flow.*\nlet talk = Llm{model: \"chat-a\"}\nFlow{talk}\n",
            "chat.splash",
        )
        .unwrap();
        let snapshot = ModelsResponse {
            nodes: vec![node("n1", Some("chat-a"), Some(3)), node("n2", Some("chat-a"), Some(2))],
            models: vec![
                model("chat-a", "text", "n1", "loaded"),
                model("chat-a", "text", "n2", "ready"),
            ],
            snapshot_ms: 1,
        };
        assert_eq!(estimate_parallelism(&graph, &snapshot).max, 5);
        assert_eq!(
            estimate_parallelism(
                &graph,
                &ModelsResponse { nodes: Vec::new(), models: Vec::new(), snapshot_ms: 2 }
            )
            .max,
            1
        );
    }
}
