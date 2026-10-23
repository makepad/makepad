use super::{Executor, Poll, one_inputs, one_outputs, param, string_param};
use crate::{Literal, Node, PortType, PortValue, Value};
use makepad_ai_hub::job::{GenJob, GenOutput, GenRouter, JobPoll, UsedProviders, Want};
use makepad_ai_hub::makepad_base64;
use makepad_ai_hub::protocol::{decode_stems_artifact, ChatMessageJson, GenerateRequestJson, LoraRefJson, NamedInputJson, StemsArtifact, STEMS_ARTIFACT_CONTENT_TYPE};
use makepad_ai_hub::registry::{Domain, Wire};
use std::sync::Arc;
use std::time::Instant;
use makepad_micro_serde::{DeJson, JsonValue, SerJson};

/// The Flow `Gen` node: its parameters and inputs become one hub request,
/// run by the hub's [`GenJob`]; the answer becomes its output ports.
pub struct GenExecutor {
    job: GenJob,
    node: Option<Node>,
}

impl GenExecutor {
    pub fn new(router: Arc<dyn GenRouter>, origin: (String, u64)) -> Self {
        Self::with_used_providers(router, origin, UsedProviders::default())
    }

    pub(crate) fn with_used_providers(router: Arc<dyn GenRouter>, origin: (String, u64), used_providers: UsedProviders) -> Self {
        Self { job: GenJob::with_used_providers(router, origin, used_providers), node: None }
    }

    pub fn poll_at(&mut self, now: Instant) -> Poll {
        match self.job.poll_at(now) {
            JobPoll::Pending => Poll::Pending,
            JobPoll::Progress { permille, stage } => Poll::Progress { permille, stage },
            JobPoll::Delta(text) => Poll::Delta { port: "text".to_string(), text },
            // a streaming job's published chunk: the flow node takes the finished job's artifacts
            JobPoll::Chunk(_) => Poll::Pending,
            JobPoll::Failed(error) => Poll::Failed(error),
            JobPoll::Done(output) => match outputs(self.node.as_ref().expect("generation node set before polling"), output) {
                Ok(outputs) => Poll::Done(one_outputs(outputs)),
                Err(error) => Poll::Failed(error),
            },
        }
    }
}

/// The node's ports from a finished job: a text or JSON port reads the
/// job's text when it answered with text, every other port takes the next
/// artifact; then `seed_used`, `model_used`, `provider_url` and `job_id`.
fn outputs(node: &Node, output: GenOutput) -> Result<Vec<(String, Value)>, String> {
    let mut outputs = Vec::new();
    // A stems answer is one packed bundle; the node's per-stem audio ports
    // are cut from it, every other port takes artifacts as usual.
    let stems = match node.outputs.iter().any(|port| stem_index(node, port).is_some()) {
        true => Some(
            output.artifacts.iter().find(|artifact| artifact.content_type == STEMS_ARTIFACT_CONTENT_TYPE)
                .ok_or_else(|| "the stems job returned no stem bundle".to_string())
                .and_then(|artifact| decode_stems_artifact(&artifact.bytes))?,
        ),
        false => None,
    };
    let mut artifacts = output.artifacts.into_iter();
    for port in &node.outputs {
        if let (Some(index), Some(stems)) = (stem_index(node, port), &stems) {
            outputs.push((port.name.clone(), Value::media(PortType::Audio, "audio/wav".to_string(), stem_wav(stems, index))));
            continue;
        }
        if matches!(&port.ty, PortType::Text | PortType::Json) {
            if let Some(text) = output.text.as_deref() {
                outputs.push((port.name.clone(), if port.ty == PortType::Json {
                    makepad_strict_json::parse(text.as_bytes()).map_err(|error| format!("invalid JSON result: {error}"))?;
                    Value::json(text)
                } else {
                    Value::text(text)
                }));
                continue;
            }
        }
        let artifact = artifacts.next()
            .ok_or_else(|| format!("generation returned no artifact for output `{}`", port.name))?;
        let value = match &port.ty {
            PortType::Text => Value::text(String::from_utf8(artifact.bytes).map_err(|error| error.to_string())?),
            PortType::Json => {
                let text = String::from_utf8(artifact.bytes).map_err(|error| error.to_string())?;
                makepad_strict_json::parse(text.as_bytes()).map_err(|error| format!("invalid JSON result: {error}"))?;
                Value::json(text)
            }
            PortType::List => Value::list(String::from_utf8(artifact.bytes).map_err(|error| error.to_string())?),
            PortType::Registered(key) => return Err(format!("unsupported registered type `{}`", key.as_str())),
            ty => Value::media(ty.clone(), artifact.content_type, artifact.bytes),
        };
        outputs.push((port.name.clone(), value));
    }
    append_seed_used(&mut outputs, output.seed);
    if !output.model.is_empty() {
        outputs.push(("model_used".into(), Value::text(output.model)));
    }
    if !output.node.is_empty() {
        outputs.push(("provider_url".into(), Value::text(output.node)));
    }
    outputs.push(("job_id".into(), Value::text(output.job_id)));
    Ok(outputs)
}

/// The stems domain's bundle order (drums, bass, other, vocals): a node of
/// that domain names its audio ports after them to get each stem.
const STEM_NAMES: [&str; 4] = ["drums", "bass", "other", "vocals"];

fn stem_index(node: &Node, port: &crate::Port) -> Option<usize> {
    if node.domain.as_deref() != Some("stems") || port.ty != PortType::Audio {
        return None;
    }
    STEM_NAMES.iter().position(|name| *name == port.name)
}

/// One stem of a bundle as a 16-bit stereo WAV at the bundle's rate.
fn stem_wav(stems: &StemsArtifact, index: usize) -> Vec<u8> {
    let (left, right) = (&stems.channels[index * 2], &stems.channels[index * 2 + 1]);
    let data_len = (stems.frames * 4) as u32;
    let mut wav = Vec::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&stems.sample_rate.to_le_bytes());
    wav.extend_from_slice(&(stems.sample_rate * 4).to_le_bytes());
    wav.extend_from_slice(&4u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for (l, r) in left.iter().zip(right) {
        for sample in [l, r] {
            wav.extend_from_slice(&((sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16).to_le_bytes());
        }
    }
    wav
}

impl Executor for GenExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, PortValue)]) -> Result<(), String> {
        let domain_text = node.domain.as_deref().unwrap_or("");
        let domain = Domain::parse(domain_text)
            .ok_or_else(|| format!("unknown generation domain `{domain_text}`"))?;
        let inputs = one_inputs(node, inputs)?;
        let request = build_request(node, domain, &inputs)?;
        let text = node.outputs.iter().filter(|port| matches!(port.ty, PortType::Text | PortType::Json)).count();
        // Per-stem ports come out of the one bundle artifact, not artifacts of their own.
        let stem_ports = node.outputs.iter().filter(|port| stem_index(node, port).is_some()).count();
        let media = (node.outputs.len() - text - stem_ports).max(usize::from(stem_ports > 0));
        self.node = Some(node.clone());
        self.job.start(domain, request, Want::Outputs { media, text })
    }

    fn poll(&mut self) -> Poll { self.poll_at(Instant::now()) }

    fn cancel(&mut self) { self.job.cancel(); }
}

fn build_request(node: &Node, domain: Domain, inputs: &[(String, Value)]) -> Result<GenerateRequestJson, String> {
    // A typed request input preserves fields and 64-bit integers when an
    // existing creator submits a reusable graph. Explicit controls and data
    // edges below override it; ownership always belongs to this flow run.
    let mut request = match inputs.iter().find(|(name, _)| name == "request") {
        Some((_, value)) if value.ty == PortType::Json => {
            // The JSON value plane has already parsed this input. Keep the
            // hub's u64 decoder here: the read-only strict-json value type
            // represents integers as i64 and would reject full-width seeds.
            let mut base = JsonValue::deserialize_json(value.as_text()?)
                .map_err(|e| format!("Gen request input: {e:?}"))?;
            let JsonValue::Object(fields) = &mut base else {
                return Err("Gen request input must be a JSON object".into());
            };
            // The graph may supply the model separately, so settings objects
            // need not repeat it. JsonValue retains the full u64 seed range.
            fields.entry("model".into()).or_insert(JsonValue::String(String::new()));
            GenerateRequestJson::deserialize_json(&base.serialize_json())
                .map_err(|e| format!("Gen request input: {e:?}"))?
        }
        Some(_) => return Err("Gen request input must be JSON".into()),
        None => GenerateRequestJson::default(),
    };
    let model = string_param(node, "model");
    if !model.is_empty() { request.model = model; }
    for (name, value) in inputs {
        if name.starts_with("dependency_") { continue; }
        if name == "prompt" && value.ty == PortType::Text {
            let text = value.as_text()?.trim();
            if !text.is_empty() { request.prompt = Some(text.to_string()); }
        } else if name == "text" && value.ty == PortType::Text {
            request.text = Some(value.as_text()?.to_string());
        } else if name == "lyrics" && value.ty == PortType::Text {
            request.lyrics = Some(value.as_text()?.to_string());
        } else if name == "identity_anchor" && value.ty == PortType::Text {
            // The words an expansion must keep verbatim, wired from the
            // recipe's own input.
            let text = value.as_text()?.trim();
            if !text.is_empty() { request.identity_anchor = Some(text.to_string()); }
        } else if value.ty.is_media() {
            let data_b64 = String::from_utf8(makepad_base64::base64_encode(
                &value.bytes,
                &makepad_base64::BASE64_STANDARD,
            ))
            .map_err(|error| error.to_string())?;
            // The domain's input ports say where each medium travels; a
            // port it does not declare rides as a same-named input.
            let wire = if name == "primary" { Some(Wire::Primary) } else { domain.input(name).map(|port| port.wire) };
            match wire {
                Some(Wire::Primary) => {
                    request.input_b64 = Some(data_b64);
                    request.input_content_type = Some(value.content_type.clone());
                }
                Some(Wire::Named(wire_name)) => {
                    request.inputs.get_or_insert_with(Vec::new).push(NamedInputJson {
                        name: wire_name.to_string(),
                        content_type: value.content_type.clone(),
                        data_b64,
                    });
                }
                None => {
                    request.inputs.get_or_insert_with(Vec::new).push(NamedInputJson {
                        name: name.clone(),
                        content_type: value.content_type.clone(),
                        data_b64,
                    });
                }
            }
        }
    }
    request.negative_prompt = (string_opt(node, "negative").or_else(|| string_opt(node, "negative_prompt"))).or(request.negative_prompt);
    request.width = (u32_param(node, "width")).or(request.width);
    request.height = (u32_param(node, "height")).or(request.height);
    request.seed = (seed_param(node)?).or(request.seed);
    request.steps = (u32_param(node, "steps")).or(request.steps);
    request.guidance = (f64_param(node, "guidance")).or(request.guidance);
    request.queue_policy = (string_opt(node, "queue_policy")).or(request.queue_policy);
    request.strength = (f64_param(node, "strength").map(|value| value as f32)).or(request.strength);
    request.frames = (u32_param(node, "frames")).or(request.frames);
    request.codec = (string_opt(node, "codec")).or(request.codec);
    request.audio = (bool_param(node, "audio")).or(request.audio);
    request.interpolate = (u32_param(node, "interpolate")).or(request.interpolate);
    request.upscale = (u32_param(node, "upscale").or_else(|| u32_param(node, "factor"))).or(request.upscale);
    request.flow_map = (bool_param(node, "flow_map")).or(request.flow_map);
    request.delay_ms = (u64_param(node, "delay_ms")).or(request.delay_ms);
    request.pull_only = (bool_param(node, "pull_only")).or(request.pull_only);
    request.target_domain = (string_opt(node, "target_domain")).or(request.target_domain);
    request.identity_anchor = (string_opt(node, "identity_anchor")).or(request.identity_anchor);
    request.style = (string_opt(node, "style")).or(request.style);
    request.max_tokens = (u32_param(node, "max_tokens")).or(request.max_tokens);
    request.temperature = (f64_param(node, "temperature")).or(request.temperature);
    request.variants = (u32_param(node, "variants")).or(request.variants);
    request.chat_session = (string_opt(node, "chat_session")).or(request.chat_session);
    request.domain = (string_opt(node, "request_domain")).or(request.domain);
    request.chat_system = (string_opt(node, "chat_system")).or(request.chat_system);
    request.chat_messages = (chat_messages_param(node)?).or(request.chat_messages);
    request.voice = (string_opt(node, "voice")).or(request.voice);
    request.speed = (f64_param(node, "speed")).or(request.speed);
    request.language = (string_opt(node, "language")).or(request.language);
    request.emotion = (number_array_param(node, "emotion")).or(request.emotion);
    request.seconds = (f64_param(node, "seconds")).or(request.seconds);
    request.lyrics = request.lyrics.or_else(|| string_opt(node, "lyrics"));
    request.score = (string_opt(node, "score")).or(request.score);
    request.score_mode = (string_opt(node, "score_mode")).or(request.score_mode);
    request.score_only = (bool_param(node, "score_only")).or(request.score_only);
    request.remesh_resolution = (u32_param(node, "remesh_resolution")).or(request.remesh_resolution);
    request.texture = (bool_param(node, "texture")).or(request.texture);
    request.decimation_target = (u32_param(node, "decimation_target")).or(request.decimation_target);
    request.texture_size = (u32_param(node, "texture_size")).or(request.texture_size);
    request.gaussians = (u32_param(node, "gaussians")).or(request.gaussians);
    request.motion_mode = (string_opt(node, "motion_mode")).or(request.motion_mode);
    request.canny_low = (f64_param(node, "canny_low")).or(request.canny_low);
    request.canny_high = (f64_param(node, "canny_high")).or(request.canny_high);
    request.peer_sources = (string_array_param(node, "peer_sources")).or(request.peer_sources);
    request.peer_tickets = (string_array_param(node, "peer_tickets")).or(request.peer_tickets);
    request.loras = (lora_param(node)?).or(request.loras);
    Ok(request)
}

pub(crate) fn unsupported_params(node: &Node) -> Vec<String> {
    const SUPPORTED: &[&str] = &[
        "model", "negative", "negative_prompt", "width", "height", "seed", "steps",
        "guidance", "queue_policy", "strength", "frames", "codec", "audio", "interpolate",
        "upscale", "factor", "flow_map", "delay_ms", "pull_only", "target_domain",
        "identity_anchor", "style", "max_tokens", "temperature", "variants", "chat_session",
        "request_domain", "chat_system", "chat_messages", "voice", "speed", "language",
        "emotion", "seconds", "lyrics", "score", "score_mode", "score_only", "remesh_resolution", "texture", "decimation_target", "texture_size",
        "gaussians", "motion_mode", "canny_low", "canny_high", "loras", "peer_sources",
        "peer_tickets",
    ];
    node.params
        .iter()
        .filter_map(|(name, _)| {
            (!SUPPORTED.contains(&name.as_str()) && !node.outputs.iter().any(|port| port.name == *name))
                .then(|| format!("Gen node `{}` parameter `{name}` is not represented by GenerateRequestJson", node.id))
        })
        .collect()
}

/// Why a graph's generators cannot run as written: one line per problem,
/// naming the node. A pinned model of another domain must get everything
/// its own domain needs, and a required input must be fed (by an edge, a
/// literal, or a `Map` binding its element to that port). `model_domain`
/// answers which domain a model serves (the hubs' reports, else the
/// registry). Empty when every generator is well formed.
pub fn graph_problems(graph: &crate::Graph, model_domain: &dyn Fn(&str) -> Option<Domain>) -> Vec<String> {
    use crate::NodeInputValue;
    let fed = |node: &Node, port: &str| node.inputs.iter().find(|input| input.port == port).is_some_and(|input| match &input.value {
        NodeInputValue::Edge(_) => true,
        NodeInputValue::Edges(entries) => !entries.is_empty(),
        NodeInputValue::Literal(Literal::Null) => false,
        NodeInputValue::Literal(Literal::Str(text)) => !text.trim().is_empty(),
        NodeInputValue::Literal(_) => true,
    });
    // The ports a domain's job must be given: its required media, and its text.
    let required = |domain: Domain| -> Vec<&'static str> {
        let io = domain.io();
        io.inputs.iter().filter(|port| port.required).map(|port| port.name)
            .chain(io.needs_text.then_some("text")).collect()
    };
    let mut problems = Vec::new();
    // A Map's body runs with the Map's element bound to one of its ports.
    let bound: Vec<(String, String)> = graph.nodes.iter().filter(|node| node.kind == "map").filter_map(|map| {
        let body = map.inputs.iter().find(|input| input.port == "body").and_then(|input| match &input.value {
            NodeInputValue::Edge(edge) => Some(edge.from_node.clone()),
            _ => None,
        })?;
        let bind = map.params.iter().find(|(name, _)| name == "bind").and_then(|(_, value)| match value {
            Literal::Id(port) | Literal::Str(port) => Some(port.clone()),
            _ => None,
        })?;
        Some((body, bind))
    }).collect();
    for node in graph.nodes.iter().filter(|node| node.kind == "gen") {
        let Some(domain_text) = node.domain.as_deref().filter(|domain| !domain.is_empty()) else { continue };
        let domain = Domain::parse(domain_text);
        let model = node.params.iter().find(|(name, _)| name == "model").and_then(|(_, value)| match value {
            Literal::Str(text) | Literal::Id(text) if !text.trim().is_empty() => Some(text.trim().to_string()),
            _ => None,
        });
        if let Some(model) = &model {
            let serves = model_domain(model).filter(|serves| Some(*serves) != domain
                && !domain.is_some_and(|domain| makepad_ai_hub::registry::also_serves(model, domain)));
            if let Some(serves) = serves {
                // Another kind of model still fits when this node gives it
                // everything its own kind needs (an edit model on an Image
                // node with a reference image wired).
                let unmet: Vec<&str> = required(serves).into_iter().filter(|port| !fed(node, port)).collect();
                if !unmet.is_empty() {
                    let ours = domain.map_or(domain_text, |domain| domain.io().kind);
                    let needs = unmet.iter().map(|port| format!("`{port}`")).collect::<Vec<_>>().join(", ");
                    problems.push(format!("`{}`: {model} is a {} model and needs {needs} connected; this {} node makes {ours}",
                        node.id, serves.io().kind, node.type_name));
                }
            }
        }
        for port in domain.map_or_else(Vec::new, required) {
            let by_map = bound.iter().any(|(body, bind)| body == &node.id && bind == port);
            if !fed(node, port) && !by_map {
                problems.push(format!("`{}` ({}) needs its `{port}` input connected", node.id, node.type_name));
            }
        }
    }
    problems
}

fn string_opt(node: &Node, name: &str) -> Option<String> {
    match param(node, name) {
        Some(Literal::Str(value) | Literal::Id(value)) if !value.is_empty() => Some(value.clone()),
        _ => None,
    }
}

fn f64_param(node: &Node, name: &str) -> Option<f64> {
    match param(node, name) {
        Some(Literal::Num(value)) => Some(*value),
        _ => None,
    }
}

fn u64_param(node: &Node, name: &str) -> Option<u64> {
    f64_param(node, name).filter(|value| *value >= 0.0).map(|value| value as u64)
}

fn seed_param(node: &Node) -> Result<Option<u64>, String> {
    match param(node, "seed") {
        None | Some(Literal::Null) => Ok(None),
        Some(Literal::Num(value)) if *value == -1.0 => Ok(Some(draw_seed())),
        Some(Literal::Id(value) | Literal::Str(value)) if value == "random" => {
            Ok(Some(draw_seed()))
        }
        Some(Literal::Str(value)) => value.parse::<u64>().map(Some)
            .map_err(|_| "seed string must be a non-negative 64-bit integer".to_string()),
        Some(Literal::Num(value))
            if value.is_finite()
                && value.fract() == 0.0
                && *value >= 0.0
                && *value <= u64::MAX as f64 =>
        {
            Ok(Some(*value as u64))
        }
        Some(_) => Err("seed must be a non-negative integer, -1, or @random".to_string()),
    }
}

/// A fresh seed in the UI's six-digit space from the OS entropy behind
/// `RandomState` (every call gets new keys; no process-wide state).
fn draw_seed() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish() % 1_000_000
}

fn append_seed_used(outputs: &mut Vec<(String, Value)>, seed: Option<u64>) {
    if let Some(seed) = seed {
        outputs.push(("seed_used".to_string(), Value::json(seed.to_string())));
    }
}

fn u32_param(node: &Node, name: &str) -> Option<u32> {
    u64_param(node, name).and_then(|value| u32::try_from(value).ok())
}

fn bool_param(node: &Node, name: &str) -> Option<bool> {
    match param(node, name) {
        Some(Literal::Bool(value)) => Some(*value),
        _ => None,
    }
}

fn string_array_param(node: &Node, name: &str) -> Option<Vec<String>> {
    match param(node, name) {
        Some(Literal::Arr(values)) => values
            .iter()
            .map(|value| match value {
                Literal::Str(value) | Literal::Id(value) => Some(value.clone()),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn number_array_param(node: &Node, name: &str) -> Option<Vec<f64>> {
    match param(node, name) {
        Some(Literal::Arr(values)) => values
            .iter()
            .map(|value| match value {
                Literal::Num(value) => Some(*value),
                _ => None,
            })
            .collect(),
        _ => None,
    }
}

fn lora_param(node: &Node) -> Result<Option<Vec<LoraRefJson>>, String> {
    let Some(Literal::Arr(values)) = param(node, "loras") else {
        return Ok(None);
    };
    let mut loras = Vec::new();
    for value in values {
        let Literal::Obj(fields) = value else {
            return Err("loras entries must be objects".to_string());
        };
        let name = fields
            .iter()
            .find_map(|(key, value)| match (key.as_str(), value) {
                ("name", Literal::Str(value) | Literal::Id(value)) => Some(value.clone()),
                _ => None,
            })
            .ok_or_else(|| "lora entry has no name".to_string())?;
        let strength = fields.iter().find_map(|(key, value)| match (key.as_str(), value) {
            ("strength", Literal::Num(value)) => Some(*value),
            _ => None,
        });
        loras.push(LoraRefJson { name, strength });
    }
    Ok((!loras.is_empty()).then_some(loras))
}

fn chat_messages_param(node: &Node) -> Result<Option<Vec<ChatMessageJson>>, String> {
    let Some(Literal::Arr(values)) = param(node, "chat_messages") else {
        return Ok(None);
    };
    let mut messages = Vec::new();
    for value in values {
        let Literal::Obj(fields) = value else {
            return Err("chat_messages entries must be objects".to_string());
        };
        let field = |name: &str| {
            fields.iter().find_map(|(key, value)| match (key.as_str(), value) {
                (key, Literal::Str(value) | Literal::Id(value)) if key == name => {
                    Some(value.clone())
                }
                _ => None,
            })
        };
        messages.push(ChatMessageJson {
            role: field("role")
                .ok_or_else(|| "chat_messages entry has no role".to_string())?,
            text: field("text")
                .ok_or_else(|| "chat_messages entry has no text".to_string())?,
            images: None,
        });
    }
    Ok((!messages.is_empty()).then_some(messages))
}

#[cfg(test)]
mod seed_tests {
    use super::{append_seed_used, seed_param};
    use crate::{Literal, Loc, Node};

    fn random_node() -> Node {
        Node {
            id: "image".into(),
            kind: "gen".into(),
            type_name: "Image".into(),
            params: vec![("seed".into(), Literal::Id("random".into()))],
            inputs: Vec::new(),
            outputs: Vec::new(),
            at: None,
            size: None,
            flip: false,
            loc: Loc { line: 1, col: 1 },
            fn_src: None,
            face_src: None,
            on_fail: "stop".into(),
            label: None,
            domain: Some("image".into()),
            doc: None,
        }
    }

    #[test]
    fn random_seed_is_fresh_and_recorded_as_seed_used() {
        let mut node = random_node();
        let first = seed_param(&node).unwrap().unwrap();
        let second = seed_param(&node).unwrap().unwrap();
        assert_ne!(first, second);
        node.params[0].1 = Literal::Num(-1.0);
        assert_ne!(seed_param(&node).unwrap().unwrap(), second);
        let mut outputs = Vec::new();
        append_seed_used(&mut outputs, Some(first));
        assert_eq!(outputs[0].0, "seed_used");
        assert_eq!(outputs[0].1.as_text().unwrap(), first.to_string());
    }
}


#[cfg(test)]
mod graph_tests {
    use super::graph_problems;
    use makepad_ai_hub::registry::{embedded_registry, Domain};

    fn problems(source: &str) -> Vec<String> {
        let graph = crate::graph::evaluate(source, "test.splash").unwrap();
        graph_problems(&graph, &|model| embedded_registry().find(model).map(|spec| spec.domain))
    }

    #[test]
    fn an_edit_model_on_a_text_to_image_node_is_refused_until_its_reference_is_wired() {
        let found = problems("use mod.flow.*\nlet prompt = Input{ type: @text default: \"a fox\" }\nlet image = Image{ prompt: prompt.text() model: \"flux2-klein-4b\" }\nlet out = Output{ type: @image value: image.image() }\nFlow{ prompt, image, out }\n");
        assert_eq!(found.len(), 1, "{found:?}");
        assert!(found[0].contains("flux2-klein-4b is a image→image edit model"), "{}", found[0]);
        assert!(problems("use mod.flow.*\nlet prompt = Input{ type: @text default: \"a fox\" }\nlet photo = Input{ type: @image }\nlet paint = Image{ prompt: prompt.text() image: photo.image() model: \"flux2-klein-4b\" }\nlet out = Output{ type: @image value: paint.out(@image) }\nFlow{ prompt, photo, paint, out }\n").is_empty());
        assert_eq!(Domain::Edit.io().kind, "image→image edit");
    }

    #[test]
    fn an_edit_without_its_image_is_refused() {
        let found = problems("use mod.flow.*\nlet prompt = Input{ type: @text default: \"make it red\" }\nlet edit = ImageEdit{ prompt: prompt.text() }\nlet out = Output{ type: @image value: edit.image() }\nFlow{ prompt, edit, out }\n");
        assert!(found.iter().any(|problem| problem.contains("`edit`") && problem.contains("`image`")), "{found:?}");
    }
}
