//! Provider abstraction for the sandbox side.
//!
//! The sandbox (and any other consumer) talks to content generation through
//! `ContentProvider`, never to a concrete service. Two families of impls are
//! planned:
//!
//! - [`LocalService`] (here): a `makepad-asset-ai` process on a LAN GPU box
//!   (or localhost), plain HTTP.
//! - Cloud providers (NOT implemented yet, by design): fal.ai, Replicate,
//!   hosted image APIs, etc. implement the same trait later — not everyone
//!   has local generator boxes, and local and cloud backends must stay
//!   interchangeable behind this one interface. A cloud impl maps
//!   `request`/`poll`/`fetch_artifact` onto the vendor's job API and
//!   translates its model catalog into `ModelInfoJson` entries.
//!
//! Everything here is blocking; callers on a UI thread should run providers
//! from a worker (the sandbox toolcall broker already does).

use crate::error::AssetAiError;
use crate::http_client::{http_fetch, HttpClientRequest};
use crate::protocol::{
    GenerateRequestJson, GenerateResponseJson, HealthJson, JobStatusJson, ModelInfoJson,
    ModelsJson,
};
use crate::registry::Domain;
use makepad_micro_serde::{DeJson, SerJson};

/// One artifact fetched from a provider.
#[derive(Clone, Debug)]
pub struct ArtifactBytes {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

pub trait ContentProvider {
    /// Service liveness + identity; cheap enough to poll for discovery.
    fn health(&self) -> Result<HealthJson, AssetAiError>;

    /// Model catalog with per-model state (absent/downloading/ready/loaded).
    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError>;

    /// Submits a generation and returns the job id. When `request.model` is
    /// empty the provider picks its first available model for `domain`.
    fn request(
        &self,
        domain: Domain,
        request: &GenerateRequestJson,
    ) -> Result<String, AssetAiError>;

    /// Job progress: queued / running{stage, progress} / done{artifacts} /
    /// error.
    fn poll(&self, job_id: &str) -> Result<JobStatusJson, AssetAiError>;

    /// Downloads one artifact by id (from `JobStatusJson::artifacts`).
    fn fetch_artifact(&self, artifact_id: &str) -> Result<ArtifactBytes, AssetAiError>;

    /// Cancels a QUEUED job (`POST /job/<id>/cancel`). Running jobs are not
    /// interruptible in v1; providers without cancel report Unavailable.
    fn cancel(&self, job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        let _ = job_id;
        Err(AssetAiError::Unavailable(
            "cancel not supported by this provider".to_string(),
        ))
    }
}

/// `ContentProvider` over a `makepad-asset-ai` service at `base_url`,
/// e.g. `http://10.0.0.217:8765`.
pub struct LocalService {
    base_url: String,
    auth_headers: Vec<(String, String)>,
}

const MAX_JSON_BODY: usize = 4 * 1024 * 1024;
/// Generated artifacts (video!) can be large.
const MAX_ARTIFACT_BODY: usize = 1024 * 1024 * 1024;

impl LocalService {
    pub fn new(base_url: &str) -> Self {
        let base_url = base_url.trim_end_matches('/').to_string();
        let secret = std::env::var("MAKEPAD_AI_HUB_SECRET").ok();
        Self {
            base_url,
            auth_headers: Self::auth_headers(secret.as_deref()),
        }
    }

    /// Override the environment-provided fabric secret for this service.
    pub fn with_secret(mut self, secret: impl Into<String>) -> Self {
        let secret = secret.into();
        self.auth_headers = Self::auth_headers(Some(&secret));
        self
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn auth_headers(secret: Option<&str>) -> Vec<(String, String)> {
        let Some(secret) = secret.map(str::trim) else {
            return Vec::new();
        };
        if secret.contains(['\r', '\n']) {
            return Vec::new();
        }
        vec![(
            "Authorization".to_string(),
            format!("Bearer {secret}"),
        )]
    }

    fn get_request<'a>(&'a self, url: &'a str) -> HttpClientRequest<'a> {
        let mut request = HttpClientRequest::get(url);
        request.extra_headers = &self.auth_headers;
        request
    }

    fn post_request<'a>(
        &'a self,
        url: &'a str,
        content_type: &'a str,
        body: &'a [u8],
    ) -> HttpClientRequest<'a> {
        let mut request = HttpClientRequest::post(url, content_type, body);
        request.extra_headers = &self.auth_headers;
        request
    }

    fn get_json<T: DeJson>(&self, path: &str) -> Result<T, AssetAiError> {
        let url = format!("{}{}", self.base_url, path);
        let response = http_fetch(&self.get_request(&url))?;
        let status = response.status;
        let body = response.read_body_to_vec(MAX_JSON_BODY)?;
        parse_json_body::<T>(status, &body, &url)
    }
}

fn parse_json_body<T: DeJson>(status: u16, body: &[u8], url: &str) -> Result<T, AssetAiError> {
    let text = std::str::from_utf8(body)
        .map_err(|_| AssetAiError::Http(format!("{url}: response is not utf-8")))?;
    if status == 409 {
        return Err(AssetAiError::Busy);
    }
    if status != 200 {
        // Error bodies are {"error": "..."} — surface the message.
        let message = crate::protocol::ErrorJson::deserialize_json_lenient(text)
            .map(|e| e.error)
            .unwrap_or_else(|_| text.to_string());
        return Err(AssetAiError::Http(format!("{url}: http {status}: {message}")));
    }
    T::deserialize_json_lenient(text)
        .map_err(|e| AssetAiError::Http(format!("{url}: bad response json: {e:?}")))
}

impl ContentProvider for LocalService {
    fn health(&self) -> Result<HealthJson, AssetAiError> {
        self.get_json("/health")
    }

    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> {
        let models: ModelsJson = self.get_json("/models")?;
        Ok(models.models)
    }

    fn request(
        &self,
        domain: Domain,
        request: &GenerateRequestJson,
    ) -> Result<String, AssetAiError> {
        let mut request = request.clone();
        if request.model.is_empty() {
            let models = self.list_models()?;
            let chosen = models
                .iter()
                .find(|model| model.available && model.domain == domain.as_str())
                .ok_or_else(|| {
                    AssetAiError::Unavailable(format!(
                        "no available {} model on {}",
                        domain.as_str(),
                        self.base_url
                    ))
                })?;
            request.model = chosen.id.clone();
        }
        let url = format!("{}/generate", self.base_url);
        let body = request.serialize_json();
        let response = http_fetch(&self.post_request(&url, "application/json", body.as_bytes()))?;
        let status = response.status;
        let bytes = response.read_body_to_vec(MAX_JSON_BODY)?;
        let parsed: GenerateResponseJson = parse_json_body(status, &bytes, &url)?;
        match parsed.job_id {
            Some(job_id) => Ok(job_id),
            None => Err(AssetAiError::Http(format!(
                "{url}: {}",
                parsed.error.unwrap_or_else(|| "no job id".to_string())
            ))),
        }
    }

    fn poll(&self, job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        self.get_json(&format!("/job/{job_id}"))
    }

    fn cancel(&self, job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        let url = format!("{}/job/{job_id}/cancel", self.base_url);
        // The in-repo HttpServer 500s on body-less POSTs; send an empty object.
        let response = http_fetch(&self.post_request(&url, "application/json", b"{}"))?;
        let status = response.status;
        let body = response.read_body_to_vec(MAX_JSON_BODY)?;
        parse_json_body(status, &body, &url)
    }

    fn fetch_artifact(&self, artifact_id: &str) -> Result<ArtifactBytes, AssetAiError> {
        let url = format!("{}/artifact/{artifact_id}", self.base_url);
        let response = http_fetch(&self.get_request(&url))?;
        let status = response.status;
        let content_type = response
            .header("content-type")
            .unwrap_or("application/octet-stream")
            .to_string();
        let expected_sha256 = response.header("x-artifact-sha256").map(|s| s.to_string());
        let bytes = response.read_body_to_vec(MAX_ARTIFACT_BODY)?;
        if status != 200 {
            let _: () = parse_json_body::<crate::protocol::ErrorJson>(status, &bytes, &url)
                .map(|_| ())?;
            return Err(AssetAiError::Http(format!("{url}: http {status}")));
        }
        // Hash-verified handoff: when the service declared a digest, the
        // received bytes must match it — a truncated/corrupted relay fails
        // here explicitly instead of poisoning the next pipeline stage.
        if let Some(expected) = expected_sha256 {
            let actual = crate::sha256::sha256_hex(&bytes);
            if !actual.eq_ignore_ascii_case(expected.trim()) {
                return Err(AssetAiError::Http(format!(
                    "{url}: artifact sha256 mismatch: got {actual}, service declared {expected} — transfer corrupted"
                )));
            }
        }
        Ok(ArtifactBytes {
            content_type,
            bytes,
        })
    }
}

/// Verifies fetched artifact bytes against a job status artifact ref
/// (JSON-level counterpart of the transport header check — use it when the
/// bytes traveled through some other channel).
pub fn verify_artifact_bytes(
    bytes: &[u8],
    artifact: &crate::protocol::ArtifactRefJson,
) -> Result<(), AssetAiError> {
    if let Some(expected_len) = artifact.byte_len {
        if bytes.len() as u64 != expected_len {
            return Err(AssetAiError::Http(format!(
                "artifact {}: byte length mismatch: got {}, declared {expected_len}",
                artifact.id,
                bytes.len()
            )));
        }
    }
    if let Some(expected) = artifact.sha256.as_deref() {
        let actual = crate::sha256::sha256_hex(bytes);
        if !actual.eq_ignore_ascii_case(expected.trim()) {
            return Err(AssetAiError::Http(format!(
                "artifact {}: sha256 mismatch: got {actual}, declared {expected}",
                artifact.id
            )));
        }
    }
    Ok(())
}
