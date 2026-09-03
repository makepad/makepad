use super::{param, string_param, Executor, Poll};
use crate::{Literal, Node, PortType, Value};
use makepad_ai_hub::client::{verify_artifact_bytes, ContentProvider, LocalService};
use makepad_ai_hub::protocol::{
    ChatMessageJson, GenerateRequestJson, LoraRefJson, NamedInputJson,
};
use makepad_ai_hub::registry::Domain;
use makepad_ai_hub::{discovery, fleet, makepad_base64};
use std::sync::Arc;

pub trait GenSeam: Send + Sync {
    fn pick(&self, domain: &str) -> Result<Box<dyn ContentProvider>, String>;
}

pub struct FleetGen;

impl GenSeam for FleetGen {
    fn pick(&self, domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        let mut snapshots = Vec::new();
        for node in discovery::start_listener().nodes() {
            let service = LocalService::new(&node.base_url);
            let mut snapshot = fleet::BoxSnapshot::new(&node.base_url);
            snapshot.health = service.health().ok();
            snapshot.models = service.list_models().unwrap_or_default();
            snapshots.push(snapshot);
        }
        let cost = match domain {
            "text" => 0.2,
            "video" => 6.0,
            _ => 1.0,
        };
        let (index, _, _) = fleet::pick_for_domain_eta(&snapshots, domain, cost)
            .ok_or_else(|| format!("no node serves the {domain} domain right now"))?;
        Ok(Box::new(LocalService::new(&snapshots[index].base_url)))
    }
}

pub struct FixedGen(pub String);

impl GenSeam for FixedGen {
    fn pick(&self, domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        Ok(Box::new(LocalService::new(&self.0)))
    }
}

pub struct GenExecutor {
    seam: Arc<dyn GenSeam>,
    origin: (String, u64),
    provider: Option<Box<dyn ContentProvider>>,
    domain: Option<Domain>,
    job_id: Option<String>,
    node: Option<Node>,
    partial_text: String,
}

impl GenExecutor {
    pub fn new(seam: Arc<dyn GenSeam>, origin: (String, u64)) -> Self {
        Self {
            seam,
            origin,
            provider: None,
            domain: None,
            job_id: None,
            node: None,
            partial_text: String::new(),
        }
    }
}

impl Executor for GenExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, Value)]) -> Result<(), String> {
        let domain_text = node.domain.as_deref().unwrap_or("");
        let domain = Domain::parse(domain_text)
            .ok_or_else(|| format!("unknown generation domain `{domain_text}`"))?;
        let provider = self.seam.pick(domain_text)?;
        let request = build_request(node, inputs, &self.origin)?;
        let job_id = provider
            .request(domain, &request)
            .map_err(|error| error.to_string())?;
        self.domain = Some(domain);
        self.job_id = Some(job_id);
        self.node = Some(node.clone());
        self.provider = Some(provider);
        // keepalive: F8
        Ok(())
    }

    fn poll(&mut self) -> Poll {
        let (Some(provider), Some(job_id), Some(node)) = (
            self.provider.as_ref(),
            self.job_id.as_deref(),
            self.node.as_ref(),
        ) else {
            return Poll::Pending;
        };
        let status = match provider.poll(job_id) {
            Ok(status) => status,
            Err(error) => return Poll::Failed(error.to_string()),
        };
        if let Some(partial) = status.partial_text.as_deref() {
            if let Some(delta) = partial.strip_prefix(&self.partial_text) {
                if !delta.is_empty() {
                    self.partial_text = partial.to_string();
                    return Poll::Delta {
                        port: "text".to_string(),
                        text: delta.to_string(),
                    };
                }
            }
        }
        match status.state.as_str() {
            makepad_ai_hub::protocol::JOB_STATE_QUEUED => Poll::Pending,
            makepad_ai_hub::protocol::JOB_STATE_RUNNING => Poll::Progress {
                permille: (status.progress.unwrap_or(0.0).clamp(0.0, 1.0) * 1000.0) as u16,
                stage: status.stage.unwrap_or_else(|| "running".to_string()),
            },
            makepad_ai_hub::protocol::JOB_STATE_DONE => {
                let mut outputs = Vec::new();
                let mut artifact_index = 0usize;
                for output in &node.outputs {
                    if matches!(output.ty, PortType::Text | PortType::Json)
                        && status.text.is_some()
                    {
                        let text = status.text.as_deref().unwrap();
                        outputs.push((
                            output.name.clone(),
                            if output.ty == PortType::Json {
                                if let Err(error) = makepad_strict_json::parse(text.as_bytes()) {
                                    return Poll::Failed(format!("invalid JSON result: {error}"));
                                }
                                Value::json(text)
                            } else {
                                Value::text(text)
                            },
                        ));
                        continue;
                    }
                    let Some(artifact_ref) = status.artifacts.get(artifact_index) else {
                        return Poll::Failed(format!(
                            "generation returned no artifact for output `{}`",
                            output.name
                        ));
                    };
                    artifact_index += 1;
                    let artifact = match provider.fetch_artifact(&artifact_ref.id) {
                        Ok(artifact) => artifact,
                        Err(error) => return Poll::Failed(error.to_string()),
                    };
                    if let Err(error) = verify_artifact_bytes(&artifact.bytes, artifact_ref) {
                        return Poll::Failed(error.to_string());
                    }
                    let value = match output.ty {
                        PortType::Text => match String::from_utf8(artifact.bytes) {
                            Ok(text) => Value::text(text),
                            Err(error) => return Poll::Failed(error.to_string()),
                        },
                        PortType::Json => match String::from_utf8(artifact.bytes) {
                            Ok(text) => {
                                if let Err(error) = makepad_strict_json::parse(text.as_bytes()) {
                                    return Poll::Failed(format!("invalid JSON result: {error}"));
                                }
                                Value::json(text)
                            }
                            Err(error) => return Poll::Failed(error.to_string()),
                        },
                        PortType::List => match String::from_utf8(artifact.bytes) {
                            Ok(text) => Value::list(text),
                            Err(error) => return Poll::Failed(error.to_string()),
                        },
                        ty => Value::media(ty, artifact.content_type, artifact.bytes),
                    };
                    outputs.push((output.name.clone(), value));
                }
                Poll::Done(outputs)
            }
            makepad_ai_hub::protocol::JOB_STATE_ERROR
            | makepad_ai_hub::protocol::JOB_STATE_CANCELLED => Poll::Failed(
                status
                    .error
                    .unwrap_or_else(|| format!("generation {}", status.state)),
            ),
            other => Poll::Failed(format!("unknown generation state `{other}`")),
        }
    }

    fn cancel(&mut self) {
        if let (Some(provider), Some(job_id)) = (self.provider.as_ref(), self.job_id.as_deref()) {
            let _ = provider.cancel(job_id);
        }
    }
}

fn build_request(
    node: &Node,
    inputs: &[(String, Value)],
    origin: &(String, u64),
) -> Result<GenerateRequestJson, String> {
    let mut request = GenerateRequestJson {
        model: string_param(node, "model"),
        origin_key: Some(origin.0.clone()),
        origin_epoch: Some(origin.1),
        ..Default::default()
    };
    for (name, value) in inputs {
        if name == "prompt" && value.ty == PortType::Text {
            request.prompt = Some(value.as_text()?.to_string());
        } else if name == "text" && value.ty == PortType::Text {
            request.text = Some(value.as_text()?.to_string());
        } else if name == "lyrics" && value.ty == PortType::Text {
            request.lyrics = Some(value.as_text()?.to_string());
        } else if value.ty.is_media() {
            let data_b64 = String::from_utf8(makepad_base64::base64_encode(
                &value.bytes,
                &makepad_base64::BASE64_STANDARD,
            ))
            .map_err(|error| error.to_string())?;
            if name == "image" {
                request.input_b64 = Some(data_b64);
                request.input_content_type = Some(value.content_type.clone());
            } else {
                request.inputs.get_or_insert_with(Vec::new).push(NamedInputJson {
                    name: name.clone(),
                    content_type: value.content_type.clone(),
                    data_b64,
                });
            }
        }
    }
    request.negative_prompt = string_opt(node, "negative").or_else(|| string_opt(node, "negative_prompt"));
    request.width = u32_param(node, "width");
    request.height = u32_param(node, "height");
    request.seed = u64_param(node, "seed");
    request.steps = u32_param(node, "steps");
    request.guidance = f64_param(node, "guidance");
    request.queue_policy = string_opt(node, "queue_policy");
    request.strength = f64_param(node, "strength").map(|value| value as f32);
    request.frames = u32_param(node, "frames");
    request.codec = string_opt(node, "codec");
    request.audio = bool_param(node, "audio");
    request.interpolate = u32_param(node, "interpolate");
    request.upscale = u32_param(node, "upscale").or_else(|| u32_param(node, "factor"));
    request.flow_map = bool_param(node, "flow_map");
    request.delay_ms = u64_param(node, "delay_ms");
    request.pull_only = bool_param(node, "pull_only");
    request.target_domain = string_opt(node, "target_domain");
    request.identity_anchor = string_opt(node, "identity_anchor");
    request.style = string_opt(node, "style");
    request.max_tokens = u32_param(node, "max_tokens");
    request.temperature = f64_param(node, "temperature");
    request.variants = u32_param(node, "variants");
    request.chat_session = string_opt(node, "chat_session");
    request.domain = string_opt(node, "request_domain");
    request.chat_system = string_opt(node, "chat_system");
    request.chat_messages = chat_messages_param(node)?;
    request.voice = string_opt(node, "voice");
    request.speed = f64_param(node, "speed");
    request.language = string_opt(node, "language");
    request.emotion = number_array_param(node, "emotion");
    request.seconds = f64_param(node, "seconds");
    request.lyrics = request.lyrics.or_else(|| string_opt(node, "lyrics"));
    request.remesh_resolution = u32_param(node, "remesh_resolution");
    request.texture = bool_param(node, "texture");
    request.decimation_target = u32_param(node, "decimation_target");
    request.texture_size = u32_param(node, "texture_size");
    request.gaussians = u32_param(node, "gaussians");
    request.motion_mode = string_opt(node, "motion_mode");
    request.canny_low = f64_param(node, "canny_low");
    request.canny_high = f64_param(node, "canny_high");
    request.peer_sources = string_array_param(node, "peer_sources");
    request.peer_tickets = string_array_param(node, "peer_tickets");
    request.loras = lora_param(node)?;
    Ok(request)
}

pub(crate) fn unsupported_params(node: &Node) -> Vec<String> {
    const SUPPORTED: &[&str] = &[
        "model", "negative", "negative_prompt", "width", "height", "seed", "steps",
        "guidance", "queue_policy", "strength", "frames", "codec", "audio", "interpolate",
        "upscale", "factor", "flow_map", "delay_ms", "pull_only", "target_domain",
        "identity_anchor", "style", "max_tokens", "temperature", "variants", "chat_session",
        "request_domain", "chat_system", "chat_messages", "voice", "speed", "language",
        "emotion", "seconds", "lyrics", "remesh_resolution", "texture", "decimation_target", "texture_size",
        "gaussians", "motion_mode", "canny_low", "canny_high", "loras", "peer_sources",
        "peer_tickets",
    ];
    node.params
        .iter()
        .filter_map(|(name, _)| {
            (!SUPPORTED.contains(&name.as_str()))
                .then(|| format!("Gen node `{}` parameter `{name}` is not represented by GenerateRequestJson", node.id))
        })
        .collect()
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
        });
    }
    Ok((!messages.is_empty()).then_some(messages))
}
