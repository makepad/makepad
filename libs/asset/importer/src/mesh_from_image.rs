//! The `op.mesh.from_image.v1` operation executor: claim the armed job,
//! fetch the EXACT pinned input image, run the TRELLIS image→GLB model on
//! the fleet, measure the result, upload the typed output blobs, and report
//! facts to the Asset Server's atomic finalizer.
//!
//! This worker never publishes: `operation.finalize` is the ONLY success
//! path (the server refuses raw `worker_succeed` for operation jobs), and
//! everything it reports — blob identities, sizes, metrics, model facts —
//! is re-validated server-side before anything becomes visible.

use makepad_asset_importer::thumbs;
use makepad_render::StaticModel;
use makepad_asset_client::{
    AssetClient, ClientError, JobStateDto, OperationFinalizeRequest, OperationId,
    OperationOutputFile,
};
use makepad_asset_data::{BlobId, DeviceTier, FileRole, MediaType};
use makepad_asset_client::json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// The exact executor job kind this coordinator claims. Nothing else.
pub const MESH_OP_KIND: &str = "op.mesh.from_image.v1";

const LEASE_MS: u64 = 60_000;
const HEARTBEAT_EVERY: Duration = Duration::from_secs(15);
const FLEET_POLL_EVERY: Duration = Duration::from_millis(1200);
const CANCEL_CHECK_EVERY: Duration = Duration::from_secs(3);
const IDLE_SLEEP: Duration = Duration::from_secs(2);
/// Hard wall for one mesh generation, dispatch to artifact.
const JOB_DEADLINE: Duration = Duration::from_secs(30 * 60);

// ---------------------------------------------------------------------------
// fleet seam (scripted in tests, ai-content service in production)
// ---------------------------------------------------------------------------

pub struct MeshRequest {
    pub image: Vec<u8>,
    /// "image/png" or "image/jpeg", from the PINNED input media.
    pub content_type: &'static str,
    pub seed: u64,
}

pub enum MeshDispatch {
    Started { fleet_job: String, model: String, version: String },
    /// No admitted mesh model right now (VRAM wait, downloads): keep the
    /// job leased and retry shortly.
    Waiting { stage: String },
}

pub enum MeshPoll {
    Running { stage: String, permille: u16 },
    Done { glb: Vec<u8> },
    Failed { error: String },
}

pub trait MeshFleet {
    fn dispatch(&mut self, request: &MeshRequest) -> Result<MeshDispatch, String>;
    fn poll(&mut self, fleet_job: &str) -> Result<MeshPoll, String>;
    fn cancel(&mut self, fleet_job: &str);
}

// ---------------------------------------------------------------------------
// the coordinator
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum MeshJobOutcome {
    Finalized {
        #[allow(dead_code)]
        operation: String,
        #[allow(dead_code)]
        asset: String,
        #[allow(dead_code)]
        revision: String,
    },
    Failed { error: String },
    CancelledUpstream,
}

pub struct MeshOpCoordinator<'f> {
    pub client: AssetClient,
    pub fleet: &'f mut dyn MeshFleet,
    pub suffix: String,
    pub log: bool,
}

impl<'f> MeshOpCoordinator<'f> {
    fn log(&self, message: &str) {
        if self.log {
            eprintln!("[asset-worker mesh] {message}");
        }
    }

    pub fn run(&mut self, stop: &AtomicBool) {
        while !stop.load(Ordering::SeqCst) {
            match self.run_one(stop) {
                Ok(Some(outcome)) => self.log(&format!("outcome: {outcome:?}")),
                Ok(None) => sleep_sliced(IDLE_SLEEP, stop),
                Err(error) => {
                    self.log(&format!("asset-server call failed: {error}; retrying"));
                    sleep_sliced(Duration::from_secs(5), stop);
                }
            }
        }
    }

    /// Claim and process at most one operation job. `Ok(None)` = queue empty
    /// (the poll itself keeps the operation's availability truthful).
    pub fn run_one(&mut self, stop: &AtomicBool) -> Result<Option<MeshJobOutcome>, ClientError> {
        let Some(claimed) =
            self.client
                .worker_claim_kinds(LEASE_MS, Some(&self.suffix), &[MESH_OP_KIND])?
        else {
            return Ok(None);
        };
        if claimed.kind != MESH_OP_KIND {
            let error = format!("worker does not handle kind {}", claimed.kind);
            self.fail_job(&claimed.job, &error)?;
            return Ok(Some(MeshJobOutcome::Failed { error }));
        }
        let outcome = self.process(&claimed, stop);
        if let Ok(MeshJobOutcome::Failed { error }) = &outcome {
            self.fail_job(&claimed.job, error)?;
        }
        outcome.map(Some)
    }

    fn fail_job(
        &self,
        job: &makepad_asset_client::JobId,
        error: &str,
    ) -> Result<(), ClientError> {
        let doc = makepad_asset_client::json::obj(vec![(
            "error",
            makepad_asset_client::json::s(bounded(error, 2_000)),
        )]);
        // Terminal for this round (operation retries arm fresh rounds); a
        // lost lease here just means the server already moved on.
        let _ = self.client.worker_fail(job, Some(&self.suffix), 0, Some(&doc));
        Ok(())
    }

    fn process(
        &mut self,
        claimed: &makepad_asset_client::ClaimedJobDto,
        stop: &AtomicBool,
    ) -> Result<MeshJobOutcome, ClientError> {
        // ---- decode the armed payload: operation + exact pinned input ----
        let body = &claimed.body;
        let Some(operation) = body
            .get("operation")
            .and_then(Value::as_str)
            .and_then(OperationId::parse)
        else {
            return Ok(MeshJobOutcome::Failed {
                error: "job body missing operation id".to_string(),
            });
        };
        let Some(input) = body
            .get("inputs")
            .and_then(Value::as_arr)
            .and_then(|arr| arr.first())
        else {
            return Ok(MeshJobOutcome::Failed { error: "job body missing inputs".to_string() });
        };
        let Some(blob) = input
            .get("blob")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<BlobId>().ok())
        else {
            return Ok(MeshJobOutcome::Failed { error: "job input missing blob".to_string() });
        };
        let byte_len = input.get("byte_len").and_then(Value::as_u64);
        let content_type = match input.get("media").and_then(Value::as_str) {
            Some("png") => "image/png",
            Some("jpeg") => "image/jpeg",
            other => {
                return Ok(MeshJobOutcome::Failed {
                    error: format!("unsupported input media {other:?}"),
                })
            }
        };
        let seed = body
            .get("params")
            .and_then(|p| p.get("seed"))
            .and_then(Value::as_u64)
            .unwrap_or(0);

        // ---- fetch EXACTLY the pinned bytes (digest-verified) ----
        self.heartbeat(&claimed.job, 20, "fetching input")?;
        let image = self.client.fetch_blob_bytes(&blob, byte_len)?;

        // ---- dispatch to the fleet, waiting through VRAM admission ----
        let started = Instant::now();
        let request = MeshRequest { image, content_type, seed };
        let (fleet_job, model, version) = loop {
            if stop.load(Ordering::SeqCst) {
                return Ok(MeshJobOutcome::Failed { error: "worker stopping".to_string() });
            }
            if started.elapsed() > JOB_DEADLINE {
                return Ok(MeshJobOutcome::Failed { error: "mesh job deadline".to_string() });
            }
            if self.upstream_cancelled(&claimed.job)? {
                return Ok(MeshJobOutcome::CancelledUpstream);
            }
            match self.fleet.dispatch(&request) {
                Ok(MeshDispatch::Started { fleet_job, model, version }) => {
                    break (fleet_job, model, version)
                }
                Ok(MeshDispatch::Waiting { stage }) => {
                    self.heartbeat(&claimed.job, 40, &bounded(&stage, 180))?;
                    sleep_sliced(FLEET_POLL_EVERY, stop);
                }
                Err(error) => return Ok(MeshJobOutcome::Failed { error }),
            }
        };
        self.log(&format!("dispatched {model} for {operation}"));

        // ---- poll to completion with heartbeats + cancel checks ----
        let mut last_heartbeat = Instant::now();
        let mut last_cancel_check = Instant::now();
        let glb = loop {
            if started.elapsed() > JOB_DEADLINE {
                self.fleet.cancel(&fleet_job);
                return Ok(MeshJobOutcome::Failed { error: "mesh job deadline".to_string() });
            }
            if stop.load(Ordering::SeqCst) {
                self.fleet.cancel(&fleet_job);
                return Ok(MeshJobOutcome::Failed { error: "worker stopping".to_string() });
            }
            if last_cancel_check.elapsed() >= CANCEL_CHECK_EVERY {
                last_cancel_check = Instant::now();
                if self.upstream_cancelled(&claimed.job)? {
                    self.fleet.cancel(&fleet_job);
                    return Ok(MeshJobOutcome::CancelledUpstream);
                }
            }
            match self.fleet.poll(&fleet_job) {
                Ok(MeshPoll::Done { glb }) => break glb,
                Ok(MeshPoll::Failed { error }) => {
                    return Ok(MeshJobOutcome::Failed { error })
                }
                Ok(MeshPoll::Running { stage, permille }) => {
                    if last_heartbeat.elapsed() >= HEARTBEAT_EVERY {
                        last_heartbeat = Instant::now();
                        // The fleet owns 0..=850; upload/finalize own the rest.
                        let clamped = 50 + (permille.min(1000) as u32 * 800 / 1000) as u16;
                        if self
                            .client
                            .worker_heartbeat(
                                &claimed.job,
                                LEASE_MS,
                                Some(&self.suffix),
                                Some((clamped, &bounded(&stage, 180))),
                            )
                            .is_err()
                        {
                            // Lease lost (cancel/expiry): the finalizer would
                            // refuse anyway; stop burning GPU time.
                            self.fleet.cancel(&fleet_job);
                            return Ok(MeshJobOutcome::CancelledUpstream);
                        }
                    }
                }
                Err(error) => {
                    // Transient poll error: keep the lease alive and retry.
                    self.log(&format!("fleet poll failed: {error}; retrying"));
                    self.heartbeat(&claimed.job, 500, "fleet poll retry")?;
                }
            }
            sleep_sliced(FLEET_POLL_EVERY, stop);
        };

        // ---- measure, thumbnail, upload, finalize ----
        self.heartbeat(&claimed.job, 900, "measuring output")?;
        let measured = match measure_glb(&glb) {
            Ok(measured) => measured,
            Err(error) => {
                return Ok(MeshJobOutcome::Failed {
                    error: format!("model produced unreadable GLB: {error}"),
                })
            }
        };
        let (thumb_bytes, thumb_media, thumb_dims) = thumbnail_for(&measured.base_color);

        self.heartbeat(&claimed.job, 940, "uploading output")?;
        let glb_blob = self.client.upload_blob(&claimed.namespace, &glb)?;
        let thumb_blob = self.client.upload_blob(&claimed.namespace, &thumb_bytes)?;

        self.heartbeat(&claimed.job, 980, "finalizing")?;
        let finalize = OperationFinalizeRequest {
            job: claimed.job.clone(),
            suffix: Some(self.suffix.clone()),
            output_name: "mesh".into(),
            files: vec![OperationOutputFile {
                role: FileRole::RenderGlb,
                tier: DeviceTier::Any,
                lod: 0,
                media: MediaType::Glb,
                blob: glb_blob,
                byte_len: glb.len() as u64,
                dims: None,
            }],
            thumbnail: Some((
                thumb_blob,
                thumb_media,
                thumb_dims.0,
                thumb_dims.1,
                thumb_bytes.len() as u64,
            )),
            metrics: (
                glb.len() as u64 + thumb_bytes.len() as u64,
                measured.triangles,
                measured.vertices,
                0,
                0,
                thumb_dims.0.max(thumb_dims.1),
                0,
            ),
            bounds: Some((measured.min, measured.max)),
            generator: "asset-worker".into(),
            model,
            version: if version.is_empty() { "unversioned".into() } else { version },
            seed,
        };
        match self.client.operation_finalize(&operation, &finalize) {
            Ok((asset, revision)) => Ok(MeshJobOutcome::Finalized {
                operation: operation.to_string(),
                asset: asset.to_string(),
                revision: revision.to_string(),
            }),
            Err(error) => Ok(MeshJobOutcome::Failed {
                error: format!("finalize refused: {error}"),
            }),
        }
    }

    fn heartbeat(
        &self,
        job: &makepad_asset_client::JobId,
        permille: u16,
        note: &str,
    ) -> Result<(), ClientError> {
        // A failed heartbeat is not fatal here; the terminal paths handle
        // lease loss where it matters (the poll loop and the finalizer).
        let _ = self
            .client
            .worker_heartbeat(job, LEASE_MS, Some(&self.suffix), Some((permille, note)));
        Ok(())
    }

    fn upstream_cancelled(
        &self,
        job: &makepad_asset_client::JobId,
    ) -> Result<bool, ClientError> {
        match self.client.job_status(job) {
            Ok(status) => Ok(status.state == JobStateDto::Cancelled),
            // Transient status failure: keep going; the lease is authority.
            Err(_) => Ok(false),
        }
    }
}

/// Measured facts of one static TRELLIS output. Everything comes from the
/// parsed geometry — triangles from the index buffer, addressed vertex
/// count from the indices, model-space bounds from the loader — never from
/// an assumed vertex layout.
struct MeasuredGlb {
    triangles: u32,
    vertices: u32,
    min: [f32; 3],
    max: [f32; 3],
    base_color: Option<Vec<u8>>,
}

fn measure_glb(glb: &[u8]) -> Result<MeasuredGlb, String> {
    let model = StaticModel::parse_glb(glb)?;
    let triangles = (model.indices.len() / 3).min(u32::MAX as usize) as u32;
    let vertices = model
        .indices
        .iter()
        .max()
        .map(|m| m.saturating_add(1))
        .unwrap_or(0);
    if triangles == 0 || vertices < 3 {
        return Err("model produced empty geometry".to_string());
    }
    Ok(MeasuredGlb {
        triangles,
        vertices,
        min: [model.min.x, model.min.y, model.min.z],
        max: [model.max.x, model.max.y, model.max.z],
        base_color: model.texture_png.clone(),
    })
}

/// Thumbnail policy: the GLB's embedded base-color when it is a PNG/JPEG
/// with contract-legal dimensions; otherwise the deterministic 512×512
/// placeholder. Never fabricated dimensions — they are read from the bytes.
fn thumbnail_for(base_color: &Option<Vec<u8>>) -> (Vec<u8>, &'static str, (u32, u32)) {
    if let Some(bytes) = base_color {
        if let Some((w, h)) = thumbs::png_dims(bytes) {
            if (256..=4096).contains(&w) && (256..=4096).contains(&h) {
                return (bytes.clone(), "png", (w, h));
            }
        }
        if let Some((w, h)) = thumbs::jpeg_dims(bytes) {
            if (256..=4096).contains(&w) && (256..=4096).contains(&h) {
                return (bytes.clone(), "jpeg", (w, h));
            }
        }
    }
    let bgra = thumbs::placeholder_bgra_512();
    let jpeg = thumbs::encode_jpeg_bgra(&bgra, 512, 512).expect("placeholder encode");
    (jpeg, "jpeg", (512, 512))
}

// ---------------------------------------------------------------------------
// production fleet adapter (ai-content service over the public client)
// ---------------------------------------------------------------------------

/// TRELLIS on the fleet through the PUBLIC `makepad-asset-ai` client:
/// probe boxes, pick an admitted mesh-domain model, relay the pinned image
/// as `input_b64`, and fetch/verify the GLB artifact.
pub struct AssetAiMeshFleet {
    boxes: Vec<String>,
    discovered: Option<makepad_asset_ai::discovery::Discovered>,
    /// fleet_job -> base_url routing for poll/cancel/fetch.
    routes: std::collections::HashMap<String, String>,
    log: bool,
}

impl AssetAiMeshFleet {
    pub fn from_fleet_file(path: &std::path::Path, log: bool) -> Result<Self, String> {
        let config = makepad_asset_ai::fleet::FleetConfig::load_file(path)
            .map_err(|e| format!("fleet config {}: {e}", path.display()))?;
        let boxes = config.boxes.clone();
        if boxes.is_empty() {
            return Err(format!("fleet config {} lists no boxes", path.display()));
        }
        Ok(Self {
            boxes,
            discovered: None,
            routes: std::collections::HashMap::new(),
            log,
        })
    }

    pub fn from_lan(log: bool) -> Self {
        Self {
            boxes: Vec::new(),
            discovered: Some(makepad_asset_ai::discovery::start_listener()),
            routes: std::collections::HashMap::new(),
            log,
        }
    }

    fn boxes(&self) -> Vec<String> {
        let mut boxes = self.boxes.clone();
        if let Some(discovered) = &self.discovered {
            for node in discovered.nodes() {
                if !boxes.contains(&node.base_url) {
                    boxes.push(node.base_url);
                }
            }
        }
        boxes
    }

    fn snapshots(&self) -> Vec<makepad_asset_ai::fleet::BoxSnapshot> {
        use makepad_asset_ai::client::{ContentProvider, LocalService};
        use makepad_asset_ai::fleet::BoxSnapshot;
        let boxes = self.boxes();
        let mut snapshots = Vec::with_capacity(boxes.len());
        for url in &boxes {
            let provider = LocalService::new(url);
            let mut snapshot = BoxSnapshot::new(url);
            if let Ok(health) = provider.health() {
                snapshot.health = Some(health);
                if let Ok(models) = provider.list_models() {
                    snapshot.models = models;
                } else if self.log {
                    eprintln!("[asset-worker mesh] model probe {url} failed");
                }
            } else if self.log {
                eprintln!("[asset-worker mesh] health probe {url} failed");
            }
            snapshots.push(snapshot);
        }
        snapshots
    }
}

impl MeshFleet for AssetAiMeshFleet {
    fn dispatch(&mut self, request: &MeshRequest) -> Result<MeshDispatch, String> {
        use makepad_asset_ai::client::{ContentProvider, LocalService};
        use makepad_asset_ai::fleet::pick_for_domain_admitted_scored;
        use makepad_asset_ai::protocol::GenerateRequestJson;
        use makepad_asset_ai::registry::Domain;
        let snapshots = self.snapshots();
        let Some((index, model, _score)) =
            pick_for_domain_admitted_scored(&snapshots, "mesh")
        else {
            return Ok(MeshDispatch::Waiting {
                stage: "waiting-for-fleet: no admitted mesh model".to_string(),
            });
        };
        let base_url = self.boxes[index].clone();
        let provider = LocalService::new(&base_url);
        let wire = GenerateRequestJson {
            model: model.clone(),
            seed: Some(request.seed),
            input_b64: Some(base64_encode(&request.image)),
            input_content_type: Some(request.content_type.to_string()),
            ..Default::default()
        };
        match provider.request(Domain::Mesh, &wire) {
            Ok(fleet_job) => {
                self.routes.insert(fleet_job.clone(), base_url);
                Ok(MeshDispatch::Started {
                    fleet_job,
                    model,
                    // The service does not report a model version on the job
                    // wire; the registry id IS the pinned identity.
                    version: "registry".to_string(),
                })
            }
            Err(makepad_asset_ai::AssetAiError::Busy)
            | Err(makepad_asset_ai::AssetAiError::QueueFull(_)) => Ok(MeshDispatch::Waiting {
                stage: "waiting-for-fleet: box busy".to_string(),
            }),
            Err(error) => Err(scrub(&error.to_string(), &base_url)),
        }
    }

    fn poll(&mut self, fleet_job: &str) -> Result<MeshPoll, String> {
        use makepad_asset_ai::client::{verify_artifact_bytes, ContentProvider, LocalService};
        use makepad_asset_ai::protocol::{JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_ERROR};
        let Some(base_url) = self.routes.get(fleet_job).cloned() else {
            return Err("unknown fleet job".to_string());
        };
        let provider = LocalService::new(&base_url);
        let status = provider
            .poll(fleet_job)
            .map_err(|e| scrub(&e.to_string(), &base_url))?;
        match status.state.as_str() {
            s if s == JOB_STATE_DONE => {
                let Some(artifact) = status.artifacts.first() else {
                    return Ok(MeshPoll::Failed { error: "done with no artifact".to_string() });
                };
                if artifact.content_type != "model/gltf-binary" {
                    return Ok(MeshPoll::Failed {
                        error: format!("unexpected artifact type {}", artifact.content_type),
                    });
                }
                let bytes = provider
                    .fetch_artifact(&artifact.id)
                    .map_err(|e| scrub(&e.to_string(), &base_url))?;
                verify_artifact_bytes(&bytes.bytes, artifact)
                    .map_err(|e| scrub(&e.to_string(), &base_url))?;
                Ok(MeshPoll::Done { glb: bytes.bytes })
            }
            s if s == JOB_STATE_ERROR || s == JOB_STATE_CANCELLED => Ok(MeshPoll::Failed {
                error: scrub(
                    status.error.as_deref().unwrap_or("fleet job failed"),
                    &base_url,
                ),
            }),
            _ => {
                let permille = (status.progress.unwrap_or(0.0).clamp(0.0, 1.0) * 1000.0) as u16;
                Ok(MeshPoll::Running {
                    stage: status.stage.unwrap_or_else(|| "generating".to_string()),
                    permille,
                })
            }
        }
    }

    fn cancel(&mut self, fleet_job: &str) {
        use makepad_asset_ai::client::{ContentProvider, LocalService};
        if let Some(base_url) = self.routes.get(fleet_job) {
            let provider = LocalService::new(base_url);
            let _ = provider.cancel(fleet_job);
        }
    }
}

/// Fleet node URLs never leave this process in error documents.
fn scrub(error: &str, base_url: &str) -> String {
    error.replace(base_url, "<fleet-node>")
}

/// Sleep in short slices so SIGINT/upstream stop stays responsive
/// (the coordinator module keeps its own private twin of this).
fn sleep_sliced(total: Duration, stop: &AtomicBool) {
    let mut remaining = total;
    while remaining > Duration::ZERO && !stop.load(Ordering::SeqCst) {
        let slice = remaining.min(Duration::from_millis(100));
        std::thread::sleep(slice);
        remaining = remaining.saturating_sub(slice);
    }
}

fn bounded(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut out: String = text[..end].to_string();
    out.retain(|c| !c.is_control());
    out
}

/// Standard base64 (RFC 4648 with padding), std-only.
fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | b[2] as u32;
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
}

// ---------------------------------------------------------------------------
// tests: scripted fleet against a REAL asset server
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_store::{AssetServer, ServerConfig};
    use makepad_asset_client::{
        ApiEndpoints, ClientConfig, OperationCreateRequest, OperationInputRef, OperationStateDto,
        PublishFile, PublishRequest, PublishRights, PublishThumbnail,
    };
    use makepad_asset_data::{
        AssetKind, DerivativePolicy, Redistribution, ThumbnailMedia,
    };
    use std::path::PathBuf;
    use std::sync::atomic::AtomicU64;

    static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn test_root(name: &str) -> PathBuf {
        let n = DIR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "mp_worker_meshop_{}_{}_{}",
            std::process::id(),
            n,
            name
        ))
    }

    fn start_server(name: &str) -> (AssetServer, String) {
        let root = test_root(name);
        let mut cfg = ServerConfig::new(root.clone());
        cfg.control_addr = "127.0.0.1:0".parse().unwrap();
        cfg.data_addr = "127.0.0.1:0".parse().unwrap();
        cfg.bootstrap_admin = true;
        cfg.log = false;
        let server = AssetServer::start(cfg).expect("server start");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        (server, token)
    }

    fn connect(server: &AssetServer, token: &str, leaf: &str) -> AssetClient {
        let mut cfg = ClientConfig::new(test_root(leaf));
        cfg.token = Some(token.to_string());
        let endpoints =
            ApiEndpoints { control: server.control_addr(), data: server.data_addr() };
        AssetClient::connect(cfg, endpoints, Some(server.server_id())).expect("connect")
    }

    /// A deterministic single-shot fleet: verifies the relayed input, then
    /// produces a fixed GLB-shaped payload after one Running poll.
    struct ScriptedMesh {
        expect_image: Vec<u8>,
        glb: Vec<u8>,
        polls: u32,
        dispatched: bool,
    }

    impl MeshFleet for ScriptedMesh {
        fn dispatch(&mut self, request: &MeshRequest) -> Result<MeshDispatch, String> {
            assert_eq!(request.image, self.expect_image, "worker must relay the PINNED bytes");
            assert_eq!(request.content_type, "image/png");
            self.dispatched = true;
            Ok(MeshDispatch::Started {
                fleet_job: "fj-1".to_string(),
                model: "trellis-2".to_string(),
                version: "registry".to_string(),
            })
        }
        fn poll(&mut self, fleet_job: &str) -> Result<MeshPoll, String> {
            assert_eq!(fleet_job, "fj-1");
            self.polls += 1;
            if self.polls == 1 {
                Ok(MeshPoll::Running { stage: "denoise".to_string(), permille: 500 })
            } else {
                Ok(MeshPoll::Done { glb: self.glb.clone() })
            }
        }
        fn cancel(&mut self, _fleet_job: &str) {}
    }

    /// A fleet that always fails compute.
    struct FailingMesh;
    impl MeshFleet for FailingMesh {
        fn dispatch(&mut self, _request: &MeshRequest) -> Result<MeshDispatch, String> {
            Ok(MeshDispatch::Started {
                fleet_job: "fj-x".to_string(),
                model: "trellis-2".to_string(),
                version: "registry".to_string(),
            })
        }
        fn poll(&mut self, _fleet_job: &str) -> Result<MeshPoll, String> {
            Ok(MeshPoll::Failed {
                error: "oom at http://203.0.113.123:8765".to_string(),
            })
        }
        fn cancel(&mut self, _fleet_job: &str) {}
    }

    fn seed_and_create(
        server: &AssetServer,
        token: &str,
    ) -> (AssetClient, makepad_asset_data::AssetRevisionId, OperationId, Vec<u8>) {
        let mut admin = connect(server, token, "seed-cache");
        let png = b"png-bytes-op".to_vec();
        let mut request = PublishRequest::new(
            "gen",
            AssetKind::Texture,
            "op seed",
            PublishFile {
                bytes: png.clone(),
                media: MediaType::Png,
                role: FileRole::Texture,
                media_millis: 0,
                dims: Some((64, 64)),
            },
            PublishThumbnail {
                bytes: vec![0xAB; 1_500],
                media: ThumbnailMedia::Png,
                width: 512,
                height: 512,
            },
        );
        request.rights = PublishRights::declared(
            "CC-BY-4.0",
            "Seed Author",
            "https://example.com/seed",
            Redistribution::AttributionRequired,
            DerivativePolicy::AttributionRequired,
        );
        let published = admin.publish_artifact(&request).expect("seed publish");

        // Liveness first (an empty claim poll), then create the operation.
        let worker_probe = connect(server, token, "probe-cache");
        assert!(worker_probe
            .worker_claim_kinds(60_000, Some("w1"), &[MESH_OP_KIND])
            .unwrap()
            .is_none());
        let status = admin
            .operation_create(&OperationCreateRequest::new(
                "gen",
                "mesh.from_image.v1",
                "worker-e2e",
                vec![OperationInputRef {
                    slot: "image".into(),
                    asset: published.asset_id,
                    revision: published.revision,
                    role: FileRole::Texture,
                    tier: None,
                    lod: None,
                    expected_media: Some(MediaType::Png),
                }],
            ))
            .expect("create");
        (admin, published.revision, status.operation, png)
    }

    /// A single-triangle GLB built in code (the same shape the render
    /// crate's own parser tests pin), so the full worker flow runs without
    /// any downloaded model output.
    fn tiny_glb() -> Vec<u8> {
        let positions: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        let mut bin: Vec<u8> = Vec::new();
        for f in positions {
            bin.extend_from_slice(&f.to_le_bytes());
        }
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},
            "nodes":[{{"mesh":0}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0}}}}]}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}}],
            "bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":{}}}],
            "buffers":[{{"byteLength":{}}}]}}"#,
            bin.len(),
            bin.len()
        );
        let mut json_bytes = json.into_bytes();
        while json_bytes.len() % 4 != 0 {
            json_bytes.push(b' ');
        }
        let mut out = Vec::new();
        out.extend_from_slice(b"glTF");
        out.extend_from_slice(&2u32.to_le_bytes());
        let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(b"JSON");
        out.extend_from_slice(&json_bytes);
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(b"BIN\0");
        out.extend_from_slice(&bin);
        out
    }

    #[test]
    fn scripted_worker_finalizes_operation_end_to_end() {
        let (mut server, token) = start_server("meshop_e2e");
        let (mut admin, seed_rev, op, png) = seed_and_create(&server, &token);
        let stop = AtomicBool::new(false);

        let mut fleet =
            ScriptedMesh { expect_image: png, glb: tiny_glb(), polls: 0, dispatched: false };
        let mut coordinator = MeshOpCoordinator {
            client: connect(&server, &token, "worker-cache"),
            fleet: &mut fleet,
            suffix: "w1".to_string(),
            log: false,
        };
        let outcome = coordinator.run_one(&stop).expect("run").expect("claimed");
        let MeshJobOutcome::Finalized { operation, revision, .. } = outcome else {
            panic!("expected finalized, got {outcome:?}");
        };
        assert_eq!(operation, op.to_string());
        assert!(fleet.dispatched);

        // The operation succeeded with the published revision.
        let done = admin.operation_get(&op).expect("get");
        assert_eq!(done.state, OperationStateDto::Succeeded);
        let (_, out_rev) = done.result.expect("result");
        assert_eq!(out_rev.to_string(), revision);

        // The immutable manifest: measured metrics + bounds, exact parent,
        // inherited rights, actual model facts.
        let manifest = admin.fetch_asset_manifest(&out_rev).expect("manifest");
        assert_eq!(manifest.kind, AssetKind::Mesh);
        assert_eq!(manifest.metrics.triangles, 1);
        assert_eq!(manifest.metrics.vertices, 3);
        assert_eq!(manifest.bounds.min.x, 0.0);
        assert_eq!(manifest.bounds.max.x, 1.0);
        assert_eq!(manifest.bounds.max.y, 1.0);
        let prov = manifest.provenance.as_ref().expect("provenance");
        assert_eq!(prov.parents, vec![seed_rev]);
        assert_eq!(prov.model, "trellis-2");
        assert_eq!(prov.generator, "asset-worker");
        let seed_manifest = admin.fetch_asset_manifest(&seed_rev).expect("seed manifest");
        assert_eq!(manifest.rights, seed_manifest.rights, "rights inherit verbatim");

        server.shutdown();
    }

    #[test]
    fn unreadable_glb_fails_the_round_honestly() {
        let (mut server, token) = start_server("meshop_badglb");
        let (admin, _seed_rev, op, png) = seed_and_create(&server, &token);
        let stop = AtomicBool::new(false);
        let mut fleet = ScriptedMesh {
            expect_image: png,
            glb: b"not a glb".to_vec(),
            polls: 0,
            dispatched: false,
        };
        let mut coordinator = MeshOpCoordinator {
            client: connect(&server, &token, "worker-cache"),
            fleet: &mut fleet,
            suffix: "w1".to_string(),
            log: false,
        };
        match coordinator.run_one(&stop).expect("run") {
            Some(MeshJobOutcome::Failed { error }) => {
                assert!(error.contains("unreadable GLB"), "{error}")
            }
            other => panic!("expected honest failure, got {other:?}"),
        }
        // The operation reads failed; retry arms the next round.
        let failed = admin.operation_get(&op).expect("get");
        assert_eq!(failed.state, OperationStateDto::Failed);
        let retried = admin.operation_retry(&op).expect("retry");
        assert_eq!(retried.round, 1);
        server.shutdown();
    }

    #[test]
    fn failing_fleet_reports_scrubbed_error_and_round_fails() {
        let (mut server, token) = start_server("meshop_fail");
        let (admin, _seed_rev, op, _png) = seed_and_create(&server, &token);
        let stop = AtomicBool::new(false);
        let mut fleet = FailingMesh;
        let mut coordinator = MeshOpCoordinator {
            client: connect(&server, &token, "worker-cache"),
            fleet: &mut fleet,
            suffix: "w1".to_string(),
            log: false,
        };
        match coordinator.run_one(&stop).expect("run") {
            Some(MeshJobOutcome::Failed { error }) => assert!(error.contains("oom")),
            other => panic!("expected failure, got {other:?}"),
        }
        let failed = admin.operation_get(&op).expect("get");
        assert_eq!(failed.state, OperationStateDto::Failed);
        server.shutdown();
    }

    #[test]
    fn base64_matches_reference_vectors() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
