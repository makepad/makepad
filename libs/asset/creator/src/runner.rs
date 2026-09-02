//! The single-generation runner: pick a node, generate, publish (aicore §9).
//!
//! One blocking function every creator surface shares — vj's plain rows and
//! DREAM stages, the chat tool pack's `content.generate`, the headless
//! runner — so "generate one thing and put it in the catalog" has exactly
//! one implementation: ETA-ranked node pick over the live LAN fleet, the
//! store-body → typed-request translation and publish dressing the worker
//! shipped with (via the importer's exposed seams), cc0 rights, identical
//! thumbnails/annotations/provenance.

use crate::pipeline::StageSpec;
use makepad_ai_hub::client::{ContentProvider, LocalService};
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

/// Rough job cost for the ETA rank, in "units a 4090 does per 30s".
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

/// ETA-ranked node pick for one domain.
pub fn pick_node(domain: &str) -> Result<LocalService, AssetAiError> {
    let snapshots = fleet_snapshots();
    if snapshots.is_empty() {
        return Err(AssetAiError::Unavailable(
            "no GPU nodes on the LAN".to_string(),
        ));
    }
    let cost = stage_cost(domain);
    let (index, _model, _eta) = fleet::pick_for_domain_eta(&snapshots, domain, cost)
        .ok_or_else(|| {
            AssetAiError::Unavailable(format!("no node serves the {domain} domain right now"))
        })?;
    Ok(LocalService::new(&snapshots[index].base_url))
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
    Some(match name {
        "image" => Domain::Image,
        "video" => Domain::Video,
        "audio" => Domain::Audio,
        "mesh" => Domain::Mesh,
        "text" => Domain::Text,
        "speech" => Domain::Speech,
        "world" => Domain::World,
        "matte" => Domain::Matte,
        "depth" => Domain::Depth,
        _ => return None,
    })
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
}

/// Generate one thing and hand back what came out — no catalog, no store.
/// The desktop assistant's `gen` service uses this: the picture goes to
/// disk and into the photo wall, never through the asset store. Blocking;
/// run from a worker thread. `progress` sees (note, permille).
pub fn generate_bytes(
    kind_name: &str,
    body: &Value,
    seed_fallback: u64,
    cancel: &Arc<AtomicBool>,
    progress: &mut dyn FnMut(&str, u16),
) -> Result<GeneratedBytes, String> {
    let (kind, request, wire) = translate(kind_name, body, seed_fallback)?;
    let domain = kind.domain;
    let parsed_domain =
        domain_of(domain).ok_or_else(|| format!("unroutable domain {domain}"))?;
    let service = pick_node(domain).map_err(|e| e.to_string())?;
    let node = service.base_url().to_string();
    let remote = service
        .request(parsed_domain, &wire)
        .map_err(|e| e.to_string())?;
    progress("queued-on-fleet", 0);

    use makepad_ai_hub::protocol::{JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_ERROR};
    let (artifact_ref, text) = loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = service.cancel(&remote);
            return Err("cancelled".to_string());
        }
        let status = service.poll(&remote).map_err(|e| e.to_string())?;
        match status.state.as_str() {
            JOB_STATE_DONE => break (status.artifacts.first().cloned(), status.text),
            JOB_STATE_ERROR => {
                return Err(status.error.unwrap_or_else(|| "job error".to_string()))
            }
            JOB_STATE_CANCELLED => return Err("cancelled".to_string()),
            _ => {
                progress(
                    status.stage.as_deref().unwrap_or(""),
                    (status.progress.unwrap_or(0.0).clamp(0.0, 1.0) * 1000.0) as u16,
                );
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    };

    if kind.catalog().is_none() {
        return Ok(GeneratedBytes { kind, request, artifact: None, text, node });
    }
    let artifact_ref = artifact_ref.ok_or("the job finished without an artifact")?;
    let bytes = service
        .fetch_artifact(&artifact_ref.id)
        .map_err(|e| format!("artifact fetch: {e}"))?;
    Ok(GeneratedBytes { kind, request, artifact: Some(bytes), text, node })
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
    let GeneratedBytes { kind, request, artifact, text, .. } = generated;
    let Some(bytes) = artifact else {
        return Ok(Generated { asset_id: None, revision: None, text });
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
    let publish = dress_generated_publish(
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
    let published = client
        .publish_artifact(&publish)
        .map_err(|e| format!("publish: {e}"))?;
    Ok(Generated {
        asset_id: Some(published.asset_id.to_string()),
        revision: Some(published.revision.to_string()),
        text,
    })
}
