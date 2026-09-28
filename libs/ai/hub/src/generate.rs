//! One generation against the fleet: route, submit, own, poll, fetch.
//!
//! The caller already holds a wire request. Nothing here knows a job
//! vocabulary, a catalog or a graph: the assistant's `gen` service uses this
//! directly, and the asset creator runs its direct-provider path through the
//! same code. These blocking operations belong on workers.

use crate::client::{verify_artifact_bytes, ArtifactBytes, ContentProvider, LocalService};
use crate::error::AssetAiError;
use crate::protocol::{
    GenerateRequestJson, JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_ERROR,
};
use crate::registry::Domain;
use crate::{discovery, fleet};
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GenerateError {
    Unavailable(String),
    Failed(String),
    Cancelled,
}
impl std::fmt::Display for GenerateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unavailable(s) => write!(f, "unavailable: {s}"),
            Self::Failed(s) => f.write_str(s),
            Self::Cancelled => f.write_str("cancelled"),
        }
    }
}
impl From<AssetAiError> for GenerateError {
    fn from(error: AssetAiError) -> Self {
        match error {
            AssetAiError::Cancelled => Self::Cancelled,
            AssetAiError::Unavailable(s) => Self::Unavailable(s),
            other => Self::Failed(other.to_string()),
        }
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

pub struct RoutedProvider {
    pub provider: Box<dyn ContentProvider>,
    pub model: String,
    pub node: String,
}

/// The ordinary live capability/ETA gate over the LAN fleet.
pub fn route_fleet(
    domain: &str,
    request: &GenerateRequestJson,
) -> Result<RoutedProvider, GenerateError> {
    #[cfg(target_arch = "wasm32")]
    { let _ = (domain, request); Err(GenerateError::Unavailable("LAN fleet unavailable on wasm".into())) }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (provider, model) = pick_node_for_request(domain, request)?;
        let node = provider.base_url().to_string();
        Ok(RoutedProvider { provider: Box::new(provider), model, node })
    }
}

/// What a finished job has to hand back.
pub struct JobExpect<'a> {
    /// Names the job in errors (`image.generate`).
    pub label: &'a str,
    /// Accepted artifact content types; `None` for a job that answers in text.
    pub artifact: Option<&'a [&'a str]>,
}

#[derive(Clone, Debug)]
pub struct JobOutput {
    pub artifact: Option<ArtifactBytes>,
    pub text: Option<String>,
    /// The node's base url, for a card that says where it ran.
    pub node: String,
    /// The remote job this invocation owned.
    pub job_id: String,
}

/// Submit, own, poll and fetch ONE real job on an already routed provider.
/// Every early exit after submit relinquishes that job. No UI thread,
/// per-job thread, or progress-driven cancellation bridge is involved.
pub fn run_job(
    domain: Domain,
    wire: &GenerateRequestJson,
    routed: RoutedProvider,
    expect: &JobExpect,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&str, u16),
    poll_interval: Duration,
) -> Result<JobOutput, GenerateError> {
    let check = || if cancelled() { Err(GenerateError::Cancelled) } else { Ok(()) };
    check()?;
    let service = routed.provider;
    let remote = service.request_pending(domain, wire, cancelled, &mut |note| progress(note, 0))?;
    progress(&format!("job {} on {}", remote, routed.node), 0);
    let result = (|| {
        let status = loop {
            check()?;
            let status = service.poll(&remote)?;
            match status.state.as_str() {
                JOB_STATE_DONE => break status,
                JOB_STATE_ERROR => return Err(GenerateError::Failed(status.error.unwrap_or_else(|| "job error".into()))),
                JOB_STATE_CANCELLED => return Err(GenerateError::Cancelled),
                _ => progress(status.stage.as_deref().unwrap_or("running"),
                    (status.progress.unwrap_or(0.0).clamp(0.0, 1.0) * 1000.0) as u16),
            }
            service.keepalive(&remote)?;
            std::thread::sleep(poll_interval);
        };
        check()?;
        let artifact = if let Some(content_types) = expect.artifact {
            let reference = status.artifacts.first()
                .ok_or_else(|| GenerateError::Failed("job finished without an artifact".into()))?;
            let bytes = service.fetch_artifact(&reference.id)?;
            check()?;
            verify_artifact_bytes(&bytes.bytes, reference)?;
            if !content_types.contains(&bytes.content_type.as_str()) {
                return Err(GenerateError::Failed(format!("{} returned {}", expect.label, bytes.content_type)));
            }
            Some(bytes)
        } else { None };
        Ok(JobOutput { artifact, text: status.text, node: routed.node, job_id: remote.clone() })
    })();
    if result.is_err() { let _ = service.cancel(&remote); }
    result
}

/// Route over the live fleet and run the job there. A request without a
/// model takes the routed one.
pub fn generate(
    domain: Domain,
    mut wire: GenerateRequestJson,
    expect: &JobExpect,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(&str, u16),
    poll_interval: Duration,
) -> Result<JobOutput, GenerateError> {
    if cancelled() { return Err(GenerateError::Cancelled); }
    let routed = route_fleet(domain.as_str(), &wire)?;
    if wire.model.is_empty() { wire.model = routed.model.clone(); }
    run_job(domain, &wire, routed, expect, cancelled, progress, poll_interval)
}
