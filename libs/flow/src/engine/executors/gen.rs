use super::{param, string_param, Executor, Poll};
use crate::{Literal, Node, PortType, Value};
use makepad_ai_hub::client::{verify_artifact_bytes, ContentProvider, LocalService};
use makepad_ai_hub::protocol::{
    ChatMessageJson, GenerateRequestJson, LoraRefJson, NamedInputJson,
};
use makepad_ai_hub::registry::Domain;
use makepad_ai_hub::{discovery, fleet, makepad_base64};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

#[derive(Clone, Copy)]
enum MediaDestination {
    Primary,
    Named(&'static str),
}

// This is the hub's binary-input wire contract. Domains not listed here use
// the compatibility default: `image` is primary and every other port is a
// same-named entry in `GenerateRequestJson::inputs`.
const MEDIA_INPUT_ROUTES: &[(&str, &[(&str, MediaDestination)])] = &[
    (
        "edit",
        &[
            ("image", MediaDestination::Primary),
            ("reference_1", MediaDestination::Named("reference_1")),
            ("reference_2", MediaDestination::Named("reference_2")),
            ("reference_3", MediaDestination::Named("reference_3")),
        ],
    ),
    (
        "inpaint",
        &[
            ("image", MediaDestination::Named("image")),
            ("mask", MediaDestination::Named("mask")),
        ],
    ),
    ("control", &[("control", MediaDestination::Primary)]),
    ("upscale", &[("image", MediaDestination::Primary)]),
    ("matte", &[("image", MediaDestination::Primary)]),
    ("depth", &[("image", MediaDestination::Primary)]),
    (
        "video",
        &[
            ("image", MediaDestination::Primary),
            ("last_frame", MediaDestination::Named("last_frame")),
        ],
    ),
    ("enhance", &[("video", MediaDestination::Primary)]),
    ("mesh", &[("image", MediaDestination::Primary)]),
    (
        "paint",
        &[
            ("mesh", MediaDestination::Named("mesh")),
            (
                "reference_image",
                MediaDestination::Named("reference_image"),
            ),
        ],
    ),
    ("rig", &[("mesh", MediaDestination::Primary)]),
    ("motion", &[("mesh", MediaDestination::Primary)]),
    ("splat", &[("image", MediaDestination::Primary)]),
    ("world", &[("image", MediaDestination::Primary)]),
    ("vision", &[("image", MediaDestination::Primary)]),
    ("body", &[("image", MediaDestination::Primary)]),
    ("segment", &[("image", MediaDestination::Primary)]),
    ("stt", &[("audio", MediaDestination::Primary)]),
    ("beats", &[("audio", MediaDestination::Primary)]),
    ("stems", &[("audio", MediaDestination::Primary)]),
    ("notes", &[("audio", MediaDestination::Primary)]),
    ("music", &[("audio", MediaDestination::Primary)]),
    ("speech", &[("audio", MediaDestination::Primary)]),
];

pub trait GenSeam: Send + Sync {
    fn pick(&self, domain: &str) -> Result<Box<dyn ContentProvider>, String>;

    /// Model-aware, retry-capable routing retained for existing seam
    /// implementations.
    fn pick_for(
        &self,
        domain: &str,
        model: &str,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        if !excluded.is_empty() {
            return Err("generation seam has no alternate provider".to_string());
        }
        Ok(GenPick {
            provider: self.pick(domain)?,
            base_url: "provider".to_string(),
            model: model.to_string(),
            model_state: None,
        })
    }

    /// Request-aware, retry-capable routing. Existing model-aware seams
    /// receive the request's model through [`GenSeam::pick_for`], while seams
    /// that need dimensions or steps can override this additive entry point.
    fn pick_for_request(
        &self,
        domain: &str,
        request: &GenerateRequestJson,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        self.pick_for(domain, &request.model, excluded)
    }
}

/// One routed provider plus the facts the executor needs to make retries
/// observable and to submit the exact model the fleet scorer admitted.
pub struct GenPick {
    pub provider: Box<dyn ContentProvider>,
    pub base_url: String,
    pub model: String,
    pub model_state: Option<String>,
}

pub struct FleetGen;

impl GenSeam for FleetGen {
    fn pick(&self, domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        Ok(self
            .pick_for_request(domain, &GenerateRequestJson::default(), &[])?.provider)
    }

    fn pick_for(
        &self,
        domain: &str,
        model: &str,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        let request = GenerateRequestJson {
            model: model.to_string(),
            ..Default::default()
        };
        self.pick_for_request(domain, &request, excluded)
    }

    fn pick_for_request(
        &self,
        domain: &str,
        request: &GenerateRequestJson,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        let mut snapshots = Vec::new();
        for node in discovery::start_listener().nodes() {
            if excluded.iter().any(|url| url == &node.base_url) {
                continue;
            }
            let service = LocalService::new(&node.base_url);
            let mut snapshot = fleet::BoxSnapshot::new(&node.base_url);
            snapshot.health = service.health().ok();
            snapshot.models = service.list_models().unwrap_or_default();
            snapshots.push(snapshot);
        }
        let (index, model) = if request.model.is_empty() {
            fleet::pick_for_domain_eta_request(&snapshots, domain, request)
                .map(|(index, model, _)| (index, model))
                .ok_or_else(|| fleet::unroutable_request_error(&snapshots, domain, request))?
        } else {
            fleet::pick_for_model_eta(&snapshots, &request.model, request)
                .map(|(index, _)| (index, request.model.clone()))
                .ok_or_else(|| fleet::unroutable_request_error(&snapshots, domain, request))?
        };
        let model_state = snapshots[index]
            .models
            .iter()
            .find(|candidate| candidate.id == model)
            .map(|candidate| candidate.state.clone());
        let base_url = snapshots[index].base_url.clone();
        Ok(GenPick {
            provider: Box::new(LocalService::new(&base_url)),
            base_url,
            model,
            model_state,
        })
    }
}

pub struct FixedGen(pub String);

impl GenSeam for FixedGen {
    fn pick(&self, domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        Ok(Box::new(LocalService::new(&self.0)))
    }

    fn pick_for(
        &self,
        domain: &str,
        model: &str,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        Domain::parse(domain).ok_or_else(|| format!("unknown generation domain `{domain}`"))?;
        if excluded.iter().any(|url| url == &self.0) {
            return Err(format!(
                "fixed generation node {} was excluded",
                fleet::node_label(&self.0)
            ));
        }
        Ok(GenPick {
            provider: Box::new(LocalService::new(&self.0)),
            base_url: self.0.clone(),
            model: model.to_string(),
            model_state: None,
        })
    }
}

struct UsedProvider {
    provider: Arc<dyn ContentProvider>,
    bye_sent: AtomicBool,
}

impl UsedProvider {
    fn bye(&self) {
        if !self.bye_sent.swap(true, Ordering::SeqCst) {
            let _ = self.provider.bye();
        }
    }
}

/// Providers selected by one run, retained until the run exits so server
/// shutdown can release their origin leases even after a node completed.
#[derive(Clone, Default)]
pub(crate) struct UsedProviders(Arc<Mutex<Vec<Arc<UsedProvider>>>>);

impl UsedProviders {
    fn track(&self, provider: Box<dyn ContentProvider>) -> Arc<UsedProvider> {
        let provider = Arc::new(UsedProvider {
            provider: provider.into(),
            bye_sent: AtomicBool::new(false),
        });
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(provider.clone());
        provider
    }

    pub(crate) fn bye_all(&self) {
        for provider in self.0.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            provider.bye();
        }
    }
}

pub struct GenExecutor {
    seam: Arc<dyn GenSeam>,
    origin: (String, u64),
    used_providers: UsedProviders,
    provider: Option<Arc<UsedProvider>>,
    domain: Option<Domain>,
    job_id: Option<String>,
    node: Option<Node>,
    request: Option<GenerateRequestJson>,
    seed_used: Option<u64>,
    provider_url: Option<String>,
    refusals: Vec<(String, String)>,
    attempts: usize,
    pending_stage: Option<String>,
    partial_text: String,
    last_keepalive: Option<Instant>,
}

impl GenExecutor {
    pub fn new(seam: Arc<dyn GenSeam>, origin: (String, u64)) -> Self {
        Self::with_used_providers(seam, origin, UsedProviders::default())
    }

    pub(crate) fn with_used_providers(
        seam: Arc<dyn GenSeam>,
        origin: (String, u64),
        used_providers: UsedProviders,
    ) -> Self {
        Self {
            seam,
            origin,
            used_providers,
            provider: None,
            domain: None,
            job_id: None,
            node: None,
            request: None,
            seed_used: None,
            provider_url: None,
            refusals: Vec::new(),
            attempts: 0,
            pending_stage: None,
            partial_text: String::new(),
            last_keepalive: None,
        }
    }

    fn submit_next(&mut self) -> Result<(), String> {
        const MAX_ATTEMPTS: usize = 3;
        loop {
            if self.attempts >= MAX_ATTEMPTS {
                return Err(self.refusals_error());
            }
            let domain = self.domain.expect("generation domain set before routing");
            let request = self
                .request
                .as_ref()
                .expect("generation request set before routing");
            let excluded: Vec<String> = self
                .refusals
                .iter()
                .map(|(url, _)| url.clone())
                .collect();
            let picked = match self
                .seam
                .pick_for_request(domain.as_str(), request, &excluded)
            {
                Ok(picked) => picked,
                Err(error) if !self.refusals.is_empty() => {
                    return Err(format!("{}; {error}", self.refusals_error()))
                }
                Err(error) => return Err(error),
            };
            self.attempts += 1;
            let mut submitted = request.clone();
            if submitted.model.is_empty() {
                submitted.model = picked.model.clone();
            }
            match picked.provider.request(domain, &submitted) {
                Ok(job_id) => {
                    // A retry moves the accepted request to another machine,
                    // preserving the model the user already started with.
                    self.request = Some(submitted);
                    let retry_stage = (!self.refusals.is_empty()).then(|| {
                        let refused = self
                            .refusals
                            .iter()
                            .map(|(url, error)| {
                                format!(
                                    "{} refused: {}",
                                    fleet::node_label(url),
                                    admission_refusal_summary(error)
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("; ");
                        format!(
                            "retrying on {} ({refused})",
                            fleet::node_label(&picked.base_url)
                        )
                    });
                    let absent_stage = picked
                        .model_state
                        .as_deref()
                        .filter(|state| {
                            *state == makepad_ai_hub::protocol::MODEL_STATE_ABSENT
                        })
                        .map(|_| {
                            format!(
                                "acquiring {} on {} (model absent; download required)",
                                picked.model,
                                fleet::node_label(&picked.base_url)
                            )
                        });
                    self.pending_stage = match (retry_stage, absent_stage) {
                        (Some(retry), Some(absent)) => Some(format!("{retry}; {absent}")),
                        (Some(retry), None) => Some(retry),
                        (None, Some(absent)) => Some(absent),
                        (None, None) => None,
                    };
                    self.provider_url = Some(picked.base_url);
                    self.job_id = Some(job_id);
                    self.provider = Some(self.used_providers.track(picked.provider));
                    self.partial_text.clear();
                    self.last_keepalive = Some(Instant::now());
                    return Ok(());
                }
                Err(error) => {
                    let error = error.to_string();
                    if !is_admission_refusal(&error) {
                        return Err(error);
                    }
                    let _ = picked.provider.bye();
                    self.refusals.push((picked.base_url, error));
                }
            }
        }
    }

    fn retry_after_refusal(&mut self, error: String) -> Poll {
        let url = self
            .provider_url
            .take()
            .unwrap_or_else(|| "unknown node".to_string());
        if let Some(provider) = self.provider.take() {
            provider.bye();
        }
        self.job_id = None;
        self.last_keepalive = None;
        self.partial_text.clear();
        self.refusals.push((url, error));
        match self.submit_next() {
            Ok(()) => Poll::Progress {
                permille: 0,
                stage: self
                    .pending_stage
                    .take()
                    .unwrap_or_else(|| "retrying generation".to_string()),
            },
            Err(error) => Poll::Failed(error),
        }
    }

    fn refusals_error(&self) -> String {
        let listed = self
            .refusals
            .iter()
            .map(|(url, error)| {
                format!("{}: {}", fleet::node_label(url), admission_refusal_summary(error))
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!("generation admission refused by all attempted nodes: {listed}")
    }
}

fn is_admission_refusal(error: &str) -> bool {
    error.contains("insufficient VRAM")
        || error.starts_with("model unavailable: disk-space:")
}

fn admission_refusal_summary(error: &str) -> &str {
    if error.contains("insufficient VRAM") {
        "insufficient VRAM"
    } else {
        error
    }
}

impl Executor for GenExecutor {
    fn start(&mut self, node: &Node, inputs: &[(String, Value)]) -> Result<(), String> {
        let domain_text = node.domain.as_deref().unwrap_or("");
        let domain = Domain::parse(domain_text)
            .ok_or_else(|| format!("unknown generation domain `{domain_text}`"))?;
        let request = build_request(node, inputs, &self.origin)?;
        self.seed_used = request.seed;
        self.domain = Some(domain);
        self.node = Some(node.clone());
        self.request = Some(request);
        self.provider = None;
        self.provider_url = None;
        self.job_id = None;
        self.refusals.clear();
        self.attempts = 0;
        self.pending_stage = None;
        self.submit_next()
    }

    fn poll(&mut self) -> Poll {
        if let Some(stage) = self.pending_stage.take() {
            return Poll::Progress {
                permille: 0,
                stage,
            };
        }
        let (Some(provider), Some(job_id), Some(node)) = (
            self.provider.as_ref(),
            self.job_id.as_deref(),
            self.node.as_ref(),
        ) else {
            return Poll::Pending;
        };
        let status = match provider.provider.poll(job_id) {
            Ok(status) => status,
            Err(error) => {
                let error = error.to_string();
                if is_admission_refusal(&error) {
                    return self.retry_after_refusal(error);
                }
                return Poll::Failed(error);
            }
        };
        let active = matches!(
            status.state.as_str(),
            makepad_ai_hub::protocol::JOB_STATE_QUEUED
                | makepad_ai_hub::protocol::JOB_STATE_RUNNING
                | makepad_ai_hub::protocol::JOB_STATE_LIVE
        );
        if active
            && self
                .last_keepalive
                .is_some_and(|last| last.elapsed() >= makepad_ai_hub::lease::KEEPALIVE_INTERVAL)
        {
            self.last_keepalive = Some(Instant::now());
            if let Err(error) = provider.provider.keepalive(job_id) {
                eprintln!("[flow] generation job {job_id} keepalive warning: {error}");
            }
        } else if !active {
            self.last_keepalive = None;
        }
        if status.state == makepad_ai_hub::protocol::JOB_STATE_CANCELLED
            && self.partial_text.is_empty()
            && status.error.as_deref().is_some_and(|error| error.starts_with("local-use:"))
        {
            return self.retry_after_refusal(status.error.unwrap());
        }
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
            makepad_ai_hub::protocol::JOB_STATE_RUNNING
            | makepad_ai_hub::protocol::JOB_STATE_LIVE => Poll::Progress {
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
                    let artifact = match provider.provider.fetch_artifact(&artifact_ref.id) {
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
                append_seed_used(&mut outputs, self.seed_used);
                Poll::Done(outputs)
            }
            makepad_ai_hub::protocol::JOB_STATE_ERROR => {
                let error = status
                    .error
                    .unwrap_or_else(|| "generation error".to_string());
                if self.partial_text.is_empty() && is_admission_refusal(&error) {
                    self.retry_after_refusal(error)
                } else {
                    Poll::Failed(error)
                }
            }
            makepad_ai_hub::protocol::JOB_STATE_CANCELLED => Poll::Failed(
                status
                    .error
                    .unwrap_or_else(|| "generation cancelled".to_string()),
            ),
            other => Poll::Failed(format!("unknown generation state `{other}`")),
        }
    }

    fn cancel(&mut self) {
        if let (Some(provider), Some(job_id)) = (self.provider.as_ref(), self.job_id.as_deref()) {
            let _ = provider.provider.cancel(job_id);
            provider.bye();
        }
        self.provider = None;
        self.job_id = None;
        self.pending_stage = None;
        self.last_keepalive = None;
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
    let domain_routes = node.domain.as_deref().and_then(|domain| {
        MEDIA_INPUT_ROUTES
            .iter()
            .find_map(|(candidate, routes)| (*candidate == domain).then_some(*routes))
    });
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
            let configured = domain_routes.and_then(|routes| {
                routes
                    .iter()
                    .find_map(|(port, destination)| (*port == name).then_some(*destination))
            });
            match configured {
                Some(MediaDestination::Primary) => {
                    request.input_b64 = Some(data_b64);
                    request.input_content_type = Some(value.content_type.clone());
                }
                None if domain_routes.is_none() && name == "image" => {
                    request.input_b64 = Some(data_b64);
                    request.input_content_type = Some(value.content_type.clone());
                }
                Some(MediaDestination::Named(wire_name)) => {
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
    request.negative_prompt = string_opt(node, "negative").or_else(|| string_opt(node, "negative_prompt"));
    request.width = u32_param(node, "width");
    request.height = u32_param(node, "height");
    request.seed = seed_param(node)?;
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

fn seed_param(node: &Node) -> Result<Option<u64>, String> {
    match param(node, "seed") {
        None => Ok(None),
        Some(Literal::Num(value)) if *value == -1.0 => Ok(Some(draw_seed())),
        Some(Literal::Id(value) | Literal::Str(value)) if value == "random" => {
            Ok(Some(draw_seed()))
        }
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
