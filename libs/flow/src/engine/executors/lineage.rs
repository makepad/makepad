//! What a run knows about where a value came from: the generator jobs
//! upstream of a node (nearest first) with the model and seed they report,
//! the prompt the nearest one was given, and the digests of every media
//! value upstream. `Publish` writes this into the library as provenance.

use crate::{Graph, Literal, NodeInputValue, PortValue};
use std::collections::{HashMap, HashSet, VecDeque};

/// One generator job upstream of a value.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratorRecord {
    pub node: String,
    pub type_name: String,
    pub domain: String,
    /// The model the job reported (`model_used`), else the one it asked for;
    /// empty when the hub chose and said nothing.
    pub model: String,
    /// The seed the job reported (`seed_used`), else a fixed seed it asked for.
    pub seed: Option<u64>,
    pub params: Vec<(String, Literal)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Lineage {
    /// Generator jobs upstream, nearest first.
    pub generators: Vec<GeneratorRecord>,
    /// The prompt the nearest generator with one was given.
    pub prompt: String,
    /// Digests of the media values upstream (images, audio, video, meshes,
    /// bytes), in the order the walk met them.
    pub sources: Vec<[u8; 32]>,
}

impl Lineage {
    /// The lineage of `node`'s inputs in a run with these values.
    pub fn of(graph: &Graph, node: &str, values: &HashMap<(String, String), PortValue>) -> Self {
        let value_text = |node: &str, port: &str| {
            values
                .get(&(node.to_string(), port.to_string()))
                .and_then(PortValue::one)
                .and_then(|value| value.as_text().ok())
                .map(str::to_string)
        };
        // Which Map item each Choose downstream picked, by Map.
        let mut picked: HashMap<String, usize> = HashMap::new();
        let mut lineage = Lineage::default();
        let mut visited = HashSet::from([node.to_string()]);
        let mut queue = VecDeque::from([node.to_string()]);
        let mut order = Vec::new();
        while let Some(current) = queue.pop_front() {
            let Some(current_node) = graph.nodes.iter().find(|candidate| candidate.id == current) else {
                continue;
            };
            if current != node {
                order.push(current.clone());
            }
            if current_node.kind == "choose" {
                let from_map = current_node.inputs.iter().find(|input| input.port == "from").and_then(|input| match &input.value {
                    NodeInputValue::Edge(edge) => Some(edge.from_node.clone()),
                    NodeInputValue::Edges(entries) if entries.len() == 1 => Some(entries[0].from.from_node.clone()),
                    _ => None,
                });
                let index = value_text(&current, "index").and_then(|text| text.trim().parse::<usize>().ok());
                if let (Some(map), Some(index)) = (from_map, index) {
                    picked.insert(map, index);
                }
            }
            let map_of_body = graph.nodes.iter().find(|map| {
                map.kind == "map"
                    && map.inputs.iter().any(|input| {
                        input.port == "body" && matches!(&input.value, NodeInputValue::Edge(edge) if edge.from_node == current)
                    })
            });
            if matches!(current_node.kind.as_str(), "gen" | "chat") && current != node {
                let reported = |port: &str| value_text(&current, port);
                let mut model = reported("model_used").unwrap_or_default();
                let mut seed = reported("seed_used").and_then(|text| text.trim().parse::<u64>().ok());
                if let Some(map) = map_of_body {
                    // A Map body ran once per element; the item a Choose
                    // took says which run made the value.
                    let used = value_text(&map.id, "used")
                        .and_then(|text| makepad_strict_json::parse(text.as_bytes()).ok());
                    let item = picked.get(&map.id).and_then(|index| {
                        used.as_ref()?.get("items")?.as_arr()?.get(*index).cloned()
                    });
                    if let Some(item) = item {
                        model = item.get("model_used").and_then(|value| value.as_str()).unwrap_or_default().to_string();
                        seed = item.get("seed_used").and_then(|value| value.as_u64());
                    }
                }
                if model.is_empty() {
                    model = param_text(&current_node.params, "model");
                }
                if seed.is_none() {
                    seed = current_node.params.iter().find_map(|(name, value)| match value {
                        Literal::Num(seed) if name == "seed" && *seed >= 0.0 && seed.fract() == 0.0 => Some(*seed as u64),
                        _ => None,
                    });
                }
                if lineage.prompt.is_empty() {
                    lineage.prompt = current_node
                        .inputs
                        .iter()
                        .find(|input| input.port == "prompt")
                        .and_then(|input| match &input.value {
                            NodeInputValue::Literal(Literal::Str(text)) => Some(text.clone()),
                            NodeInputValue::Edge(edge) => value_text(&edge.from_node, &edge.from_port),
                            _ => None,
                        })
                        .unwrap_or_default();
                }
                lineage.generators.push(GeneratorRecord {
                    node: current.clone(),
                    type_name: current_node.type_name.clone(),
                    domain: current_node.domain.clone().unwrap_or_default(),
                    model,
                    seed,
                    params: current_node.params.clone(),
                });
            }
            for edge in graph.edges.iter().filter(|edge| edge.to_node == current) {
                if visited.insert(edge.from_node.clone()) {
                    queue.push_back(edge.from_node.clone());
                }
            }
        }
        let mut seen = HashSet::new();
        for upstream in &order {
            let mut ports: Vec<_> = values.iter().filter(|((owner, _), _)| owner == upstream).collect();
            ports.sort_by(|a, b| a.0 .1.cmp(&b.0 .1));
            for (_, value) in ports {
                for element in value.values() {
                    if element.ty.is_media() && seen.insert(element.digest) {
                        lineage.sources.push(element.digest);
                    }
                }
            }
        }
        lineage
    }
}

fn param_text(params: &[(String, Literal)], name: &str) -> String {
    params
        .iter()
        .find_map(|(key, value)| match value {
            Literal::Str(text) | Literal::Id(text) if key == name => Some(text.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::evaluate;
    use crate::{PortType, Sequence, Value};

    #[test]
    fn nearest_generator_first_with_reported_model_seed_and_picked_map_item() {
        let source = r#"use mod.flow.*
let prompt = Input{ type: @text default: "koi" }
let seeds = Input{ type: @list default: [5, 9] }
let image = Image{ prompt: prompt.text() width: 512 height: 512 model: "flux1-schnell" }
let fan = Map{ type: @image many: 4 body: image.image() over: seeds.list() bind: @seed }
let pick = Choose{ type: @image many: 4 from: fan.items() }
let video = Video{ prompt: prompt.text() image: pick.image() }
let published = Publish{ value: video.video() }
Flow{ prompt, seeds, image, fan, pick, video, published }
"#;
        let graph = evaluate(source, "fan.splash").unwrap();
        let png = |byte: u8| Value::media(PortType::Image, "image/png", vec![byte; 8]);
        let mut values = HashMap::new();
        let mut put = |node: &str, port: &str, value: PortValue| {
            values.insert((node.to_string(), port.to_string()), value);
        };
        put("prompt", "text", PortValue::One(Value::text("koi")));
        put("fan", "items", PortValue::Seq(Sequence { ty: PortType::Image, values: vec![png(1), png(2)] }));
        put("fan", "used", PortValue::One(Value::json(
            r#"{"items":[{"seed_used":5,"model_used":"flux1-schnell"},{"seed_used":9,"model_used":"flux1-dev"}],"dropped":[]}"#,
        )));
        put("pick", "image", PortValue::One(png(2)));
        put("pick", "index", PortValue::One(Value::json("1")));
        put("video", "video", PortValue::One(Value::media(PortType::Video, "video/mp4", b"mp4".to_vec())));
        put("video", "model_used", PortValue::One(Value::text("minimax-h3")));
        put("video", "seed_used", PortValue::One(Value::json("77")));

        let lineage = Lineage::of(&graph, "published", &values);
        let records: Vec<_> = lineage.generators.iter().map(|record| (record.node.as_str(), record.model.as_str(), record.seed)).collect();
        assert_eq!(records, [("video", "minimax-h3", Some(77)), ("image", "flux1-dev", Some(9))]);
        assert_eq!(lineage.prompt, "koi");
        // The video, the picked image and both candidates, each once.
        assert_eq!(lineage.sources.len(), 3);
        assert_eq!(lineage.sources[0], values[&("video".to_string(), "video".to_string())].one().unwrap().digest);
    }
}
