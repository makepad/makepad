//! Single-generation Flow submission, validation and asset publication.
//!
//! Production fleet calls submit a Gen graph to the configured flow-server,
//! observe its result, then dress and publish through asset-client. Flow owns
//! hub routing, admission and retries. Custom provider transports retain the
//! compatibility path below. These blocking operations belong on workers.

use crate::pipeline::StageSpec;
use makepad_ai_hub::client::{ArtifactBytes, ContentProvider, LocalService};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::protocol::GenerateRequestJson;
use makepad_ai_hub::registry::Domain;
use makepad_ai_hub::{discovery, fleet};
use makepad_asset_client::json::Value;
use makepad_asset_client::{ApiEndpoints, AssetClient, ClientConfig, PublishRights};
use makepad_asset_data::AssetAlias;
use makepad_asset_importer::gen_publish::{
    dress_generated_publish, wire_request, GenArtifact, GenRequest,
};
use makepad_asset_importer::gen_kinds::{kind_of, GenKind};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
#[path = "runner_submission_tests.rs"]
mod submission_tests;

/// Rough job cost for compatibility callers that use the original fleet ETA
/// API instead of supplying a translated request.
pub fn stage_cost(domain: &str) -> f64 {
    match domain {
        "text" => 0.2,
        "image" => 1.0,
        "video" => 6.0,
        _ => 1.0,
    }
}

/// The live LAN fleet, one health + models probe per node.
pub fn fleet_snapshots() -> Vec<fleet::BoxSnapshot> {
    let mut snapshots = Vec::new();
    for node in discovery::start_listener().nodes() {
        let service = LocalService::new(&node.base_url);
        let mut snapshot = fleet::BoxSnapshot::new(&node.base_url);
        snapshot.health = service.health().ok();
        snapshot.models = service.list_models().unwrap_or_default();
        snapshots.push(snapshot);
    }
    snapshots
}

/// ETA-ranked node and model pick for one translated request.
pub fn pick_node_for_request(
    domain: &str,
    request: &GenerateRequestJson,
) -> Result<(LocalService, String), AssetAiError> {
    let snapshots = fleet_snapshots();
    if snapshots.is_empty() {
        return Err(AssetAiError::Unavailable(
            "no GPU nodes on the LAN".to_string(),
        ));
    }
    let (index, model) = if request.model.is_empty() {
        fleet::pick_for_domain_eta_request(&snapshots, domain, request)
            .map(|(index, model, _)| (index, model))
    } else {
        fleet::pick_for_model_eta(&snapshots, &request.model, request)
            .map(|(index, _)| (index, request.model.clone()))
    }
    .ok_or_else(|| {
        AssetAiError::Unavailable(fleet::unroutable_request_error(
            &snapshots, domain, request,
        ))
    })?;
    Ok((LocalService::new(&snapshots[index].base_url), model))
}

/// Compatibility entry point for provider traits that only expose a stage's
/// domain and cannot yet pass its translated generation request.
pub fn pick_node(domain: &str) -> Result<LocalService, AssetAiError> {
    pick_node_for_request(domain, &GenerateRequestJson::default()).map(|(service, _)| service)
}

/// [`crate::engine::ProviderPick`] over the live fleet.
pub struct FleetPick;

impl crate::engine::ProviderPick for FleetPick {
    fn pick(&self, stage: &StageSpec) -> Result<Box<dyn ContentProvider>, AssetAiError> {
        Ok(Box::new(pick_node(&stage.domain)?))
    }
}

/// Where a finished generation landed.
#[derive(Clone, Debug)]
pub struct Generated {
    /// `Some` when the kind publishes a catalog row.
    pub asset_id: Option<String>,
    pub revision: Option<String>,
    pub alias: Option<String>,
    /// Text answer, for text kinds (`text.expand`, vision).
    pub text: Option<String>,
}

/// The publisher identity: the same store the app is attached to.
#[derive(Clone)]
pub struct PublishTarget {
    pub endpoints: ApiEndpoints,
    pub token: Option<String>,
    pub namespace: String,
}

fn domain_of(name: &str) -> Option<Domain> {
    Domain::parse(name)
}

/// Translate one store-vocabulary job body into the typed fleet request.
/// Exposed so run transports can translate at declare time and refuse early.
pub fn translate(
    kind_name: &str,
    body: &Value,
    seed_fallback: u64,
) -> Result<(&'static GenKind, GenRequest, GenerateRequestJson), String> {
    let kind = kind_of(kind_name).ok_or_else(|| format!("unknown kind {kind_name}"))?;
    let request = GenRequest::from_body(kind, body)?;
    let mut wire = wire_request(&request, request.model.clone());
    if wire.seed.is_none() {
        wire.seed = Some(seed_fallback);
    }
    Ok((kind, request, wire))
}

/// What one generation produced, before anything is published: the
/// artifact bytes (when the kind makes one), the text answer (for text
/// kinds), and the node that did the work.
#[derive(Clone, Debug)]
pub struct GeneratedBytes {
    pub kind: &'static GenKind,
    pub request: GenRequest,
    pub artifact: Option<makepad_ai_hub::client::ArtifactBytes>,
    pub text: Option<String>,
    /// The node's base url, for a card that says where it ran.
    pub node: String,
    /// The remote job this invocation owns, after a successful submission.
    pub job_id: String,
}

/// Generate one thing and hand back what came out — no catalog, no store.
/// The desktop assistant's `gen` service uses this: the picture goes to
/// disk and into the photo wall, never through the asset store. Blocking;
/// run from a worker thread. `progress` sees (note, permille).
#[cfg(target_arch = "wasm32")]
pub fn generate_bytes(
    kind_name: &str,
    body: &Value,
    seed_fallback: u64,
    cancel: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(&str, u16),
) -> Result<GeneratedBytes, String> {
    let _ = (kind_name, body, seed_fallback, cancel, progress);
    Err("asset creator LAN fleet is unavailable on wasm".to_string())
}

/// Cancellation is queried directly, even between progress messages.
pub trait Cancellation {
    fn cancelled(&self) -> bool;
}
impl Cancellation for Arc<AtomicBool> {
    fn cancelled(&self) -> bool { self.load(Ordering::Relaxed) }
}
impl Cancellation for makepad_asset_chat::session::CancelFlag {
    fn cancelled(&self) -> bool { self.is_cancelled() }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreateError {
    Unavailable(String),
    Failed(String),
    Cancelled,
}
impl std::fmt::Display for CreateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(s) => write!(f, "unavailable: {s}"),
            Self::Failed(s) => f.write_str(s),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}
impl From<AssetAiError> for CreateError {
    fn from(error: AssetAiError) -> Self {
        match error {
            AssetAiError::Cancelled => Self::Cancelled,
            AssetAiError::Unavailable(s) => Self::Unavailable(s),
            other => Self::Failed(other.to_string()),
        }
    }
}
pub fn check_cancel(cancel: &dyn Cancellation) -> Result<(), CreateError> {
    if cancel.cancelled() { Err(CreateError::Cancelled) } else { Ok(()) }
}

pub struct RoutedProvider {
    pub provider: Box<dyn ContentProvider>,
    pub model: String,
    pub node: String,
}

/// The shipping executor's routing seam. Tests inject transports here; the
/// native implementation always uses the ordinary live capability/ETA gate.
pub trait GenerationTransport {
    fn route(&self, domain: &str, request: &GenerateRequestJson) -> Result<RoutedProvider, CreateError>;
    /// Production transports execute through Flow. The direct provider seam
    /// remains for existing embedders and their deterministic fixtures.
    #[cfg(not(target_arch="wasm32"))]
    fn flow(&self) -> Option<Result<Arc<crate::flow::CreatorFlow>,CreateError>> { None }
}
pub struct FleetTransport;
impl GenerationTransport for FleetTransport {
    #[cfg(not(target_arch="wasm32"))]
    fn flow(&self) -> Option<Result<Arc<crate::flow::CreatorFlow>,CreateError>> {
        Some(crate::flow::CreatorFlow::shared().map_err(CreateError::Unavailable))
    }
    fn route(&self, domain: &str, request: &GenerateRequestJson) -> Result<RoutedProvider, CreateError> {
        #[cfg(target_arch = "wasm32")]
        { let _ = (domain, request); Err(CreateError::Unavailable("LAN fleet unavailable on wasm".into())) }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (provider, model) = pick_node_for_request(domain, request)?;
            let node = provider.base_url().to_string();
            Ok(RoutedProvider { provider: Box::new(provider), model, node })
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub fn generate_bytes(
    kind_name: &str, body: &Value, seed_fallback: u64,
    cancel: &Arc<AtomicBool>, progress: &mut dyn FnMut(&str, u16),
) -> Result<GeneratedBytes, String> {
    let (_, request, wire) = translate(kind_name, body, seed_fallback)?;
    generate_request(request, wire, &FleetTransport, cancel, progress, Duration::from_millis(500))
        .map_err(|e| e.to_string())
}

/// Submit, own, poll and fetch ONE real job. Every early exit after submit
/// relinquishes that job. No UI thread, per-job thread, or progress-driven
/// cancellation bridge is involved.
pub fn generate_request(
    mut request: GenRequest, mut wire: GenerateRequestJson,
    transport: &dyn GenerationTransport, cancel: &dyn Cancellation,
    progress: &mut dyn FnMut(&str, u16), poll_interval: Duration,
) -> Result<GeneratedBytes, CreateError> {
    check_cancel(cancel)?;
    let kind = request.kind;
    let domain = domain_of(kind.domain)
        .ok_or_else(|| CreateError::Unavailable(format!("unroutable domain {}", kind.domain)))?;
    if kind.input != makepad_asset_importer::gen_kinds::InputNeed::None
        && wire.input_b64.is_none() && wire.inputs.is_none() {
        return Err(CreateError::Failed(format!("{} requires {} input bytes", kind.kind, kind.input.content_type())));
    }
    #[cfg(not(target_arch="wasm32"))]
    if let Some(flow)=transport.flow() {
        let flow=flow?;
        return generate_request_in(&flow,request,wire,cancel,progress,poll_interval);
    }
    let routed = transport.route(kind.domain, &wire)?;
    if wire.model.is_empty() { wire.model = routed.model; }
    request.model = wire.model.clone();
    request.seed = wire.seed;
    check_cancel(cancel)?;
    let service = routed.provider;
    let remote = service.request_pending(domain, &wire, &|| cancel.cancelled(),
        &mut |note| progress(note, 0))?;
    progress(&format!("job {} on {}", remote, routed.node), 0);
    let result = (|| {
        use makepad_ai_hub::protocol::{JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_ERROR};
        let status = loop {
            check_cancel(cancel)?;
            let status = service.poll(&remote)?;
            match status.state.as_str() {
                JOB_STATE_DONE => break status,
                JOB_STATE_ERROR => return Err(CreateError::Failed(status.error.unwrap_or_else(|| "job error".into()))),
                JOB_STATE_CANCELLED => return Err(CreateError::Cancelled),
                _ => progress(status.stage.as_deref().unwrap_or("running"),
                    (status.progress.unwrap_or(0.0).clamp(0.0, 1.0) * 1000.0) as u16),
            }
            service.keepalive(&remote)?;
            std::thread::sleep(poll_interval);
        };
        check_cancel(cancel)?;
        let artifact = if let Some(shape) = kind.catalog() {
            let reference = status.artifacts.first()
                .ok_or_else(|| CreateError::Failed("job finished without an artifact".into()))?;
            let bytes = service.fetch_artifact(&reference.id)?;
            check_cancel(cancel)?;
            makepad_ai_hub::client::verify_artifact_bytes(&bytes.bytes, reference)?;
            if !shape.content_types.contains(&bytes.content_type.as_str()) {
                return Err(CreateError::Failed(format!("{} returned {}", kind.kind, bytes.content_type)));
            }
            Some(bytes)
        } else { None };
        Ok(GeneratedBytes { kind, request, artifact, text: status.text, node: routed.node, job_id: remote.clone() })
    })();
    if result.is_err() { let _ = service.cancel(&remote); }
    result
}

/// Run one primitive as its own Flow instance. Composite validation and
/// publication consume this result through the existing creator contract.
#[cfg(not(target_arch="wasm32"))]
pub fn generate_request_in(
    flow:&crate::flow::CreatorFlow,mut request:GenRequest,wire:GenerateRequestJson,
    cancel:&dyn Cancellation,progress:&mut dyn FnMut(&str,u16),poll_interval:Duration,
)->Result<GeneratedBytes,CreateError> {
    check_cancel(cancel)?;
    let stage=crate::pipeline::StageSpec{key:"generate".into(),domain:request.kind.domain.into(),deps:vec![],weight:10,seed:wire.seed.unwrap_or(0),on_fail_skip:false};
    let spec=crate::pipeline::PipelineSpec{name:request.kind.kind.into(),stages:vec![stage.clone()]};
    let graph=crate::flow_graph::compile(flow.client(),&spec,&[crate::engine::StageOrder{spec:stage,request:wire,splices:vec![]}]).map_err(CreateError::Failed)?;
    flow.client().put_source(&graph.name,&graph.source).map_err(|e|CreateError::Failed(e.to_string()))?;
    progress("submitting generation to Flow", 0);
    let run=flow.submit(&graph.name,graph.inputs).map_err(CreateError::Failed)?;
    let result=(|| {
        loop {
            check_cancel(cancel)?;
            let row=flow.snapshot(&run).map_err(CreateError::Failed)?;
            let stage=row.nodes.get("stage_0");
            if let Some(stage)=stage {
                let fallback = match stage.state {
                    makepad_flow::NodeState::Pending => "queued; waiting for Flow scheduling",
                    makepad_flow::NodeState::Running => "routing generation to a GPU worker",
                    _ => "generation status updating",
                };
                progress(stage.stage.as_deref().unwrap_or(fallback),stage.progress.unwrap_or(0));
            } else { progress("queued; waiting for Flow scheduling", 0); }
            match row.state {
                makepad_flow::RunState::Done=> {
                    let stage=stage.ok_or_else(||CreateError::Failed("Flow completed without its generation stage".into()))?;
                    let text_value=|name:&str|->Result<Option<String>,CreateError>{
                        let Some(value)=stage.outputs.iter().find(|v|v.port==name) else {return Ok(None)};
                        let bytes=flow.client().value(&value.value.digest).map_err(|e|CreateError::Failed(e.to_string()))?;
                        String::from_utf8(bytes.bytes.to_vec()).map(Some).map_err(|e|CreateError::Failed(e.to_string()))
                    };
                    if let Some(model)=text_value("model_used")? {request.model=model;}
                    if let Some(seed)=text_value("seed_used")? {request.seed=seed.parse().ok();}
                    let mut artifact=None;
                    let mut text=None;
                    if let Some(value)=stage.outputs.iter().find(|v|v.port=="result") {
                        let bytes=flow.client().value(&value.value.digest).map_err(|e|CreateError::Failed(e.to_string()))?;
                        if let Some(shape)=request.kind.catalog() {
                            if !shape.content_types.contains(&bytes.content_type.as_str()) {return Err(CreateError::Failed(format!("{} returned {}",request.kind.kind,bytes.content_type)));}
                            artifact=Some(ArtifactBytes{content_type:bytes.content_type,bytes:bytes.bytes.to_vec()});
                        } else {text=Some(String::from_utf8(bytes.bytes.to_vec()).map_err(|e|CreateError::Failed(e.to_string()))?);}
                    }
                    check_cancel(cancel)?;
                    return Ok(GeneratedBytes{kind:request.kind,request,artifact,text,node:text_value("provider_url")?.unwrap_or_else(||"flow-server".into()),job_id:text_value("job_id")?.unwrap_or_else(||run.run_id.clone())});
                }
                makepad_flow::RunState::Failed=>return Err(CreateError::Failed(stage.and_then(|s|s.error.clone()).unwrap_or_else(||"Flow generation failed".into()))),
                makepad_flow::RunState::Cancelled=>return Err(CreateError::Cancelled),
                makepad_flow::RunState::Waiting=>return Err(CreateError::Failed("generation Flow requires an answer".into())),
                _=>std::thread::sleep(poll_interval.max(Duration::from_millis(10))),
            }
        }
    })();
    if result.is_err() {let _=flow.cancel(&run);}
    result
}

/// Generate one thing and, when its kind publishes, put it in the catalog.
/// Blocking; run from a worker thread. `progress` sees (note, permille).
pub fn generate_and_publish(
    kind_name: &str,
    body: &Value,
    seed_fallback: u64,
    target: &PublishTarget,
    cancel: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(&str, u16),
) -> Result<Generated, String> {
    let generated = generate_bytes(kind_name, body, seed_fallback, cancel, progress)?;
    publish_generated(generated, seed_fallback, target, cancel, progress)
}

pub fn publish_generated(
    generated: GeneratedBytes, seed_fallback: u64, target: &PublishTarget,
    cancel: &dyn Cancellation, progress: &mut dyn FnMut(&str, u16),
) -> Result<Generated, String> {
    check_cancel(cancel).map_err(|e| e.to_string())?;
    let GeneratedBytes { kind, request, artifact, text, .. } = generated;
    let Some(bytes) = artifact else {
        return Ok(Generated { asset_id: None, revision: None, alias: None, text });
    };
    progress("publishing", 950);

    let cache = std::env::temp_dir().join("makepad-creator-publish");
    let mut config = ClientConfig::new(cache);
    config.token = target.token.clone();
    let mut client = AssetClient::connect(config, target.endpoints.clone(), None)
        .map_err(|e| format!("publish connect: {e}"))?;
    let alias_text = format!(
        "{}/run-{:016x}",
        target.namespace,
        seed_fallback ^ 0x5EED_CAFE_F00D_D00D
    );
    let character = if matches!(kind.domain, "rig" | "motion") {
        let facts = crate::character::inspect_character(&bytes.bytes)?;
        if !facts.skinned || kind.domain == "motion" && facts.clips.is_empty() {
            return Err("character output lacks a skin or required motion clips".into());
        }
        Some(facts)
    } else { None };
    let mut publish = dress_generated_publish(
        kind,
        &target.namespace,
        &request,
        GenArtifact {
            content_type: bytes.content_type.clone(),
            bytes: bytes.bytes,
        },
        AssetAlias::from_str(&alias_text).ok(),
        String::new(),
        String::new(),
        String::new(),
        PublishRights::generated_cc0(),
    )?;
    if let Some(facts) = character {
        if facts.playable { publish.tags.push("playable".into()); }
        publish.description.push_str(&format!(" Skin: skinned GLB. Clips: {}. Playable locomotion set: {}.",
            facts.clips.join(", "), facts.playable));
    }
    check_cancel(cancel).map_err(|e| e.to_string())?;
    let published = client
        .publish_artifact(&publish)
        .map_err(|e| format!("publish: {e}"))?;
    Ok(Generated {
        asset_id: Some(published.asset_id.to_string()),
        revision: Some(published.revision.to_string()),
        alias: Some(alias_text),
        text,
    })
}
