//! The claim→dispatch→publish coordinator: one process that turns queued
//! Asset Server `video.generate` jobs into published, cueable catalog
//! assets.
//!
//! Per job: claim under a lease → pick a fleet box advertising the model →
//! dispatch → poll the box (heartbeating the Asset Server lease with real
//! stage/progress, and propagating Asset Server cancellation to the box) →
//! fetch + verify the mp4 → first-frame thumbnail + measured duration →
//! publish through the shared client (annotation before publish, so the
//! catalog event is kind-stamped and the VJ refreshes instantly) →
//! `worker_succeed` with the produced asset/revision. Failures report a
//! bounded error document. Credentials never leave this process.
//!
//! The fleet transport sits behind [`VideoFleet`], so the coordinator loop
//! is tested against a REAL Asset Server with a scripted fleet — the real
//! implementation ([`AssetAiFleet`]) is a thin adapter over
//! `makepad-asset-ai`'s blocking client.

use makepad_asset_importer::thumbs::{encode_jpeg_bgra, placeholder_bgra_512, THUMB_DIM};
use makepad_asset_importer::videothumb::probe_video;
use makepad_asset_client::json::{obj, s, Value};
use makepad_asset_client::{
    AssetClient, ClaimedJobDto, ClientError, JobStateDto, PublishFile, PublishProvenance,
    PublishRequest, PublishRights, PublishThumbnail,
};
use makepad_asset_data::{AssetAlias, AssetKind, FileRole, MediaType, ThumbnailMedia};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Lease + heartbeat cadence against the Asset Server.
const LEASE_MS: u64 = 60_000;
const HEARTBEAT_EVERY: Duration = Duration::from_secs(15);
/// Fleet poll cadence + Asset-Server cancel check cadence.
const FLEET_POLL_EVERY: Duration = Duration::from_millis(1_200);
const CANCEL_CHECK_EVERY: Duration = Duration::from_secs(3);
/// Idle sleep between claim attempts when the queue is empty.
const IDLE_SLEEP: Duration = Duration::from_secs(2);
/// Hard wall-clock ceiling for one generation (queue + render).
const JOB_DEADLINE: Duration = Duration::from_secs(45 * 60);

/// A dispatched fleet job's observed state.
#[derive(Clone, Debug)]
pub enum FleetPoll {
    Running { stage: String, progress: f64 },
    Done { mp4: Vec<u8> },
    Failed { error: String },
}

/// Result of one fleet dispatch attempt. Memory pressure is intentionally
/// not an error: the coordinator keeps the Asset Server lease alive and
/// retries from a fresh fleet snapshot instead of submitting a job that the
/// GPU service's final admission gate is expected to reject.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FleetDispatch {
    Started {
        job: String,
        model: String,
        backend: String,
        version: String,
    },
    Waiting { stage: String },
}

/// The fleet boundary: everything the coordinator needs from the GPU side.
pub trait VideoFleet {
    /// Attempt one generation dispatch. A temporarily VRAM-blocked compatible
    /// node returns [`FleetDispatch::Waiting`], never a doomed submission.
    fn dispatch(&mut self, request: &VideoRequest) -> Result<FleetDispatch, String>;
    fn poll(&mut self, fleet_job: &str) -> Result<FleetPoll, String>;
    fn cancel(&mut self, fleet_job: &str);
}

/// The typed job-body contract for `video.generate` (matches the server's
/// advertised profile defaults + the VJ's prompt merge).
#[derive(Clone, Debug)]
pub struct VideoRequest {
    pub prompt: String,
    pub model: String,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frames: Option<u32>,
    pub steps: Option<u32>,
    pub seed: Option<u64>,
}

impl VideoRequest {
    /// Parse a claimed job body. Only the prompt is mandatory.
    pub fn from_body(body: &Value) -> Result<VideoRequest, String> {
        let prompt = body
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .ok_or("job body has no prompt")?
            .to_string();
        if prompt.len() > 4_000 {
            return Err("prompt too long".to_string());
        }
        let num = |key: &str| body.get(key).and_then(Value::as_u64);
        Ok(VideoRequest {
            prompt,
            model: body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or("minimax-h3")
                .to_string(),
            width: num("width").map(|v| v as u32),
            height: num("height").map(|v| v as u32),
            frames: num("frames").map(|v| v as u32),
            steps: num("steps").map(|v| v as u32),
            seed: num("seed"),
        })
    }
}

/// What one processed job ended as (for logging/tests).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobOutcome {
    Published { asset: String, revision: String },
    Failed { error: String },
    CancelledUpstream,
}

pub struct Coordinator<'f> {
    pub client: AssetClient,
    pub fleet: &'f mut dyn VideoFleet,
    pub suffix: String,
    /// The operator's explicit rights declaration for generated output —
    /// set from the CLI, never invented here.
    pub rights: PublishRights,
    pub log: bool,
}

impl<'f> Coordinator<'f> {
    /// Claim and fully process at most one job. `Ok(None)` = queue empty.
    pub fn run_one(&mut self, stop: &AtomicBool) -> Result<Option<JobOutcome>, ClientError> {
        let Some(claimed) = self.client.worker_claim_kinds(
            LEASE_MS,
            Some(&self.suffix),
            &["video.generate"],
        )? else {
            return Ok(None);
        };
        if claimed.kind != "video.generate" {
            // This worker only renders video; other kinds fail honestly
            // (their attempt burns — claims cannot be returned).
            let error = obj(vec![(
                "error",
                s(format!("worker does not handle kind {}", claimed.kind)),
            )]);
            self.client
                .worker_fail(&claimed.job, Some(&self.suffix), 0, Some(&error))?;
            return Ok(Some(JobOutcome::Failed {
                error: format!("unsupported kind {}", claimed.kind),
            }));
        }
        let outcome = self.process_video_job(&claimed, stop);
        match &outcome {
            Ok(JobOutcome::Published { asset, revision }) => {
                let result = obj(vec![
                    ("asset_id", s(asset.clone())),
                    ("revision", s(revision.clone())),
                ]);
                self.client
                    .worker_succeed(&claimed.job, Some(&self.suffix), Some(&result))?;
            }
            Ok(JobOutcome::Failed { error }) => {
                let doc = obj(vec![("error", s(bounded(error, 2_000)))]);
                self.client
                    .worker_fail(&claimed.job, Some(&self.suffix), 0, Some(&doc))?;
            }
            // Cancelled upstream: the server already terminated the job; a
            // succeed/fail would just conflict.
            Ok(JobOutcome::CancelledUpstream) => {}
            Err(_) => {}
        }
        outcome.map(Some)
    }

    fn process_video_job(
        &mut self,
        claimed: &ClaimedJobDto,
        stop: &AtomicBool,
    ) -> Result<JobOutcome, ClientError> {
        let request = match VideoRequest::from_body(&claimed.body) {
            Ok(request) => request,
            Err(error) => return Ok(JobOutcome::Failed { error }),
        };
        self.log(&format!(
            "job {}: preparing \"{}\" ({})",
            claimed.job, request.prompt, request.model
        ));
        let started = Instant::now();
        let mut last_heartbeat = Instant::now();
        let mut last_cancel_check = Instant::now();
        let mut wait_stage: Option<String> = None;
        let mut retry_not_before: Option<Instant> = None;
        let (mp4, generated_model, generated_backend, generator_version) = 'generation: loop {
            // Selection is refreshed while a compatible GPU is temporarily
            // below its advertised admission target. This is also the retry
            // path when the service's authoritative, later admission check
            // beats our health snapshot.
            let (fleet_job, dispatched_model, dispatched_backend, dispatched_version) = loop {
                if stop.load(Ordering::SeqCst) {
                    return Ok(JobOutcome::Failed { error: "worker shutdown".to_string() });
                }
                if started.elapsed() > JOB_DEADLINE {
                    return Ok(JobOutcome::Failed { error: "generation deadline".to_string() });
                }
                if last_cancel_check.elapsed() >= CANCEL_CHECK_EVERY {
                    last_cancel_check = Instant::now();
                    if let Ok(status) = self.client.job_status(&claimed.job) {
                        if status.state == JobStateDto::Cancelled {
                            self.log(&format!("job {}: cancelled upstream", claimed.job));
                            return Ok(JobOutcome::CancelledUpstream);
                        }
                    }
                }
                if last_heartbeat.elapsed() >= HEARTBEAT_EVERY {
                    last_heartbeat = Instant::now();
                    let stage = wait_stage
                        .as_deref()
                        .unwrap_or("waiting-for-fleet-admission");
                    if self
                        .client
                        .worker_heartbeat(
                            &claimed.job,
                            LEASE_MS,
                            Some(&self.suffix),
                            Some((0, &bounded(stage, 180))),
                        )
                        .is_err()
                    {
                        return Ok(JobOutcome::Failed { error: "lease lost".to_string() });
                    }
                }
                if retry_not_before.is_some_and(|at| Instant::now() < at) {
                    std::thread::sleep(FLEET_POLL_EVERY);
                    continue;
                }
                retry_not_before = None;
                match self.fleet.dispatch(&request) {
                    Ok(FleetDispatch::Started { job, model, backend, version }) => {
                        break (job, model, backend, version)
                    }
                    Ok(FleetDispatch::Waiting { stage }) => {
                        if wait_stage.as_deref() != Some(stage.as_str()) {
                            self.log(&format!("job {}: {stage}", claimed.job));
                        }
                        wait_stage = Some(stage);
                    }
                    Err(error) => {
                        return Ok(JobOutcome::Failed {
                            error: format!("fleet dispatch: {error}"),
                        })
                    }
                }
                std::thread::sleep(FLEET_POLL_EVERY);
            };

            let mut heartbeat_stage = "queued-on-fleet".to_string();
            let mut heartbeat_permille = 0;
            loop {
                if stop.load(Ordering::SeqCst) {
                    self.fleet.cancel(&fleet_job);
                    return Ok(JobOutcome::Failed { error: "worker shutdown".to_string() });
                }
                if started.elapsed() > JOB_DEADLINE {
                    self.fleet.cancel(&fleet_job);
                    return Ok(JobOutcome::Failed { error: "generation deadline".to_string() });
                }
                // Cancel propagation: the enqueuer (VJ) cancelled
                // server-side → stop the GPU box too.
                if last_cancel_check.elapsed() >= CANCEL_CHECK_EVERY {
                    last_cancel_check = Instant::now();
                    if let Ok(status) = self.client.job_status(&claimed.job) {
                        if status.state == JobStateDto::Cancelled {
                            self.fleet.cancel(&fleet_job);
                            self.log(&format!("job {}: cancelled upstream", claimed.job));
                            return Ok(JobOutcome::CancelledUpstream);
                        }
                    }
                }
                // Renew independently of a successful fleet poll. A box may
                // be restarting or its response may be temporarily lost; the
                // Asset Server lease must not expire merely because the last
                // known stage could not be refreshed.
                if last_heartbeat.elapsed() >= HEARTBEAT_EVERY {
                    last_heartbeat = Instant::now();
                    if self
                        .client
                        .worker_heartbeat(
                            &claimed.job,
                            LEASE_MS,
                            Some(&self.suffix),
                            Some((heartbeat_permille, &bounded(&heartbeat_stage, 180))),
                        )
                        .is_err()
                    {
                        self.fleet.cancel(&fleet_job);
                        return Ok(JobOutcome::Failed { error: "lease lost".to_string() });
                    }
                }
                match self.fleet.poll(&fleet_job) {
                    Ok(FleetPoll::Done { mp4 }) => {
                        break 'generation (
                            mp4,
                            dispatched_model,
                            dispatched_backend,
                            dispatched_version,
                        )
                    }
                    Ok(FleetPoll::Failed { error }) if is_vram_admission_error(&error) => {
                        // A fresh service-side check is authoritative over the
                        // snapshot used for routing. Requeue locally with a
                        // bounded backoff; never burn the Asset Server job.
                        let stage = format!(
                            "waiting-for-vram: backend admission rejected ({})",
                            bounded(&error, 120)
                        );
                        self.log(&format!("job {}: {stage}", claimed.job));
                        wait_stage = Some(stage);
                        retry_not_before = Some(Instant::now() + Duration::from_secs(5));
                        continue 'generation;
                    }
                    Ok(FleetPoll::Failed { error }) => {
                        return Ok(JobOutcome::Failed { error: format!("fleet: {error}") })
                    }
                    Ok(FleetPoll::Running { stage, progress }) => {
                        heartbeat_stage = stage;
                        heartbeat_permille =
                            (progress.clamp(0.0, 1.0) * 1000.0) as u16;
                    }
                    Err(error) => {
                        // Transient fleet transport errors: keep polling
                        // within the deadline (the box may be mid-restart).
                        self.log(&format!("job {}: fleet poll error: {error}", claimed.job));
                    }
                }
                std::thread::sleep(FLEET_POLL_EVERY);
            }
        };

        // Publish: write the verified mp4 to a temp file for the frame probe.
        let tmp = std::env::temp_dir().join(format!(
            "asset-worker-{}-{}.mp4",
            std::process::id(),
            claimed.job
        ));
        let probe = std::fs::write(&tmp, &mp4)
            .map_err(|e| e.to_string())
            .and_then(|_| probe_video(&tmp));
        let _ = std::fs::remove_file(&tmp);
        let (thumbnail_jpeg, duration_ms, real_frame) = match probe {
            Ok(p) => (p.thumbnail_jpeg, p.duration_ms, p.real_frame),
            Err(error) => {
                self.log(&format!("job {}: frame probe failed ({error}); placeholder", claimed.job));
                let jpeg = match encode_jpeg_bgra(&placeholder_bgra_512(), THUMB_DIM, THUMB_DIM) {
                    Ok(jpeg) => jpeg,
                    Err(error) => return Ok(JobOutcome::Failed { error }),
                };
                (jpeg, 0, false)
            }
        };

        let mut title = makepad_asset_client::util::sanitize_text(&request.prompt, 120);
        if title.is_empty() {
            title = "Generated video".to_string();
        }
        let mut publish = PublishRequest::new(
            &claimed.namespace,
            AssetKind::Video,
            title,
            PublishFile {
                bytes: mp4,
                media: MediaType::Mp4,
                role: FileRole::Video,
                media_millis: duration_ms,
                dims: None,
            },
            PublishThumbnail {
                bytes: thumbnail_jpeg,
                media: ThumbnailMedia::Jpeg,
                width: THUMB_DIM as u32,
                height: THUMB_DIM as u32,
            },
        );
        // Stable job-derived alias: `gen/job-<16 hex>` (two segments).
        let job_hex = claimed.job.to_string();
        let alias_text = format!(
            "{}/job-{}",
            claimed.namespace,
            job_hex.trim_start_matches("job_").chars().take(16).collect::<String>()
        );
        publish.alias = AssetAlias::from_str(&alias_text).ok();
        publish.categories = vec!["generated".to_string()];
        publish.prompt = request.prompt.clone();
        publish.generator = "asset-worker".to_string();
        publish.backend = generated_backend;
        publish.model = generated_model.clone();
        // Rights come from the operator's explicit declaration, never from
        // a silent in-code default.
        publish.rights = self.rights.clone();
        if !real_frame {
            publish.tags = vec!["no-preview-frame".to_string()];
        }
        // Typed provenance only from REAL knowledge: the seed is recorded
        // only when the job body pinned one (the box invents nondeterministic
        // seeds it never reports back), and the required generator version
        // is the actual selected service's advertised version.
        if let Some(seed) = request.seed.filter(|_| !generator_version.is_empty()) {
            publish.manifest_provenance = Some(PublishProvenance {
                generator: "makepad-asset-ai".to_string(),
                model: generated_model,
                version: generator_version,
                seed,
                parents: vec![],
                params_digest: None,
            });
        }
        match self.client.publish_artifact(&publish) {
            Ok(published) => {
                self.log(&format!(
                    "job {}: published {} rev {}",
                    claimed.job, published.asset_id, published.revision
                ));
                Ok(JobOutcome::Published {
                    asset: published.asset_id.to_string(),
                    revision: published.revision.to_string(),
                })
            }
            Err(error) => Ok(JobOutcome::Failed { error: format!("publish: {error}") }),
        }
    }

    /// Claim/process until stopped. Transport errors back off and retry.
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

    fn log(&self, message: &str) {
        if self.log {
            eprintln!("[asset-worker] {message}");
        }
    }
}

fn bounded(text: &str, max: usize) -> String {
    makepad_asset_client::util::sanitize_text(text, max)
}

/// The service's final VRAM gate runs on its GPU worker after submission and
/// can therefore observe newer pressure than the routing health snapshot.
/// Only this narrow error class is retryable; bad parameters/models still
/// fail the Asset Server job visibly.
fn is_vram_admission_error(error: &str) -> bool {
    let lower = error.to_ascii_lowercase();
    lower.contains("insufficient vram for")
}

fn sleep_sliced(total: Duration, stop: &AtomicBool) {
    let mut left = total;
    while !left.is_zero() && !stop.load(Ordering::SeqCst) {
        let slice = left.min(Duration::from_millis(100));
        std::thread::sleep(slice);
        left = left.saturating_sub(slice);
    }
}

// ---------------------------------------------------------------------------
// the real fleet adapter
// ---------------------------------------------------------------------------

/// Thin adapter over `makepad-asset-ai`'s blocking client: box selection
/// via the shared fleet scheduler, dispatch/poll/fetch/cancel over its
/// protocol, artifact bytes verified against the declared digest.
pub struct AssetAiFleet {
    boxes: Vec<String>,
    discovered: Option<makepad_asset_ai::discovery::Discovered>,
    log: bool,
    /// The box a dispatched job lives on: `fleet_job -> base_url`.
    routes: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum VideoRoute {
    Admitted {
        index: usize,
        model: String,
        backend: String,
        version: String,
    },
    Waiting { stage: String },
}

impl AssetAiFleet {
    pub fn from_fleet_file(path: &std::path::Path, log: bool) -> Result<AssetAiFleet, String> {
        let config = makepad_asset_ai::fleet::FleetConfig::load_file(path)
            .map_err(|e| format!("fleet file {}: {e}", path.display()))?;
        if config.boxes.is_empty() {
            return Err(format!("fleet file {} lists no boxes", path.display()));
        }
        Ok(AssetAiFleet {
            boxes: config.boxes,
            discovered: None,
            log,
            routes: Default::default(),
        })
    }

    pub fn from_lan(log: bool) -> AssetAiFleet {
        AssetAiFleet {
            boxes: Vec::new(),
            discovered: Some(makepad_asset_ai::discovery::start_listener()),
            log,
            routes: Default::default(),
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

    /// Snapshot every box and select only a route whose latest free-VRAM
    /// facts pass admission. A hardware-compatible target under transient
    /// pressure remains a wait candidate instead of becoming an error or a
    /// fallback submission.
    fn pick_video_box(&self, want_model: &str) -> Result<VideoRoute, String> {
        use makepad_asset_ai::client::{ContentProvider, LocalService};
        use makepad_asset_ai::fleet::BoxSnapshot;
        let boxes = self.boxes();
        let mut snapshots = Vec::with_capacity(boxes.len());
        let mut probe_incomplete = false;
        for url in &boxes {
            let provider = LocalService::new(url);
            let mut snapshot = BoxSnapshot::new(url);
            match provider.health() {
                Ok(health) => {
                    snapshot.health = Some(health);
                    match provider.list_models() {
                        Ok(models) => snapshot.models = models,
                        Err(error) => {
                            probe_incomplete = true;
                            if self.log {
                                eprintln!(
                                    "[asset-worker] fleet model probe {url} failed: {error}"
                                );
                            }
                        }
                    }
                }
                Err(error) => {
                    probe_incomplete = true;
                    if self.log {
                        eprintln!("[asset-worker] fleet health probe {url} failed: {error}");
                    }
                }
            }
            snapshots.push(snapshot);
        }
        match select_video_route(&snapshots, want_model) {
            Err(_) if probe_incomplete => Ok(VideoRoute::Waiting {
                stage: "waiting-for-fleet: capability probe incomplete".to_string(),
            }),
            result => result,
        }
    }
}

fn select_video_route(
    snapshots: &[makepad_asset_ai::fleet::BoxSnapshot],
    want_model: &str,
) -> Result<VideoRoute, String> {
    use makepad_asset_ai::fleet::{
        model_admission, pick_box_admitted_scored, pick_box_scored,
        pick_for_domain_admitted_scored, pick_for_domain_scored, VramAdmission,
    };

    // An explicit/requested model remains a pin whenever any compatible GPU
    // advertises it. Prefer any admitted copy; otherwise hold for the best
    // compatible copy instead of silently changing models.
    if let Some((index, _)) = pick_box_admitted_scored(snapshots, want_model) {
        return Ok(admitted_video_route(snapshots, index, want_model.to_string()));
    }
    if let Some((index, _)) = pick_box_scored(snapshots, want_model) {
        if let Some(VramAdmission::Waiting { required_free_mb, free_mb }) =
            model_admission(&snapshots[index], want_model)
        {
            return Ok(VideoRoute::Waiting {
                stage: format!(
                    "waiting-for-vram: model {want_model} has {free_mb} MiB free, {required_free_mb} MiB required"
                ),
            });
        }
        return Ok(VideoRoute::Waiting {
            stage: format!(
                "waiting-for-vram: model {want_model} awaits a fresh admission snapshot"
            ),
        });
    }

    // The requested id is absent/unavailable/incompatible fleet-wide: retain
    // the historical domain fallback, but apply the same admission contract.
    if let Some((index, model, _)) = pick_for_domain_admitted_scored(snapshots, "video") {
        return Ok(admitted_video_route(snapshots, index, model));
    }
    if let Some((index, model, _)) = pick_for_domain_scored(snapshots, "video") {
        if let Some(VramAdmission::Waiting { required_free_mb, free_mb }) =
            model_admission(&snapshots[index], &model)
        {
            return Ok(VideoRoute::Waiting {
                stage: format!(
                    "waiting-for-vram: model {model} has {free_mb} MiB free, {required_free_mb} MiB required"
                ),
            });
        }
        return Ok(VideoRoute::Waiting {
            stage: format!(
                "waiting-for-vram: model {model} awaits a fresh admission snapshot"
            ),
        });
    }
    Err("no fleet box advertises a hardware-compatible video model".to_string())
}

fn admitted_video_route(
    snapshots: &[makepad_asset_ai::fleet::BoxSnapshot],
    index: usize,
    model: String,
) -> VideoRoute {
    let snapshot = &snapshots[index];
    let backend = snapshot
        .models
        .iter()
        .find(|info| info.id == model)
        .map(|info| info.backend.clone())
        .unwrap_or_default();
    let version = snapshot
        .health
        .as_ref()
        .map(|health| health.version.clone())
        .unwrap_or_default();
    VideoRoute::Admitted { index, model, backend, version }
}

impl VideoFleet for AssetAiFleet {
    fn dispatch(&mut self, request: &VideoRequest) -> Result<FleetDispatch, String> {
        use makepad_asset_ai::client::{ContentProvider, LocalService};
        use makepad_asset_ai::protocol::GenerateRequestJson;
        use makepad_asset_ai::registry::Domain;
        let (index, model, backend, version) = match self.pick_video_box(&request.model)? {
            VideoRoute::Admitted { index, model, backend, version } => {
                (index, model, backend, version)
            }
            VideoRoute::Waiting { stage } => return Ok(FleetDispatch::Waiting { stage }),
        };
        let base_url = self.boxes[index].clone();
        if self.log {
            eprintln!("[asset-worker] dispatch to {base_url} model {model}");
        }
        let wire = GenerateRequestJson {
            model,
            prompt: Some(request.prompt.clone()),
            width: request.width,
            height: request.height,
            frames: request.frames,
            steps: request.steps,
            seed: request.seed,
            codec: Some("h264".to_string()),
            ..Default::default()
        };
        let provider = LocalService::new(&base_url);
        let fleet_job = match provider.request(Domain::Video, &wire) {
            Ok(job) => job,
            Err(
                makepad_asset_ai::error::AssetAiError::Busy
                | makepad_asset_ai::error::AssetAiError::QueueFull(_),
            ) => {
                return Ok(FleetDispatch::Waiting {
                    stage: "waiting-for-fleet: selected video node queue is full".to_string(),
                })
            }
            Err(error) => {
                if self.log {
                    eprintln!("[asset-worker] dispatch to {base_url} failed: {error}");
                }
                // Worker failure documents are visible to Asset Server
                // clients; keep the compute-node address server-side.
                return Err(error.to_string().replace(&base_url, "<fleet-node>"));
            }
        };
        self.routes.insert(fleet_job.clone(), base_url);
        Ok(FleetDispatch::Started {
            job: fleet_job,
            model: wire.model,
            backend,
            version,
        })
    }

    fn poll(&mut self, fleet_job: &str) -> Result<FleetPoll, String> {
        use makepad_asset_ai::client::{verify_artifact_bytes, ContentProvider, LocalService};
        use makepad_asset_ai::protocol::{JOB_STATE_DONE, JOB_STATE_ERROR};
        let base_url = self.routes.get(fleet_job).ok_or("unknown fleet job")?.clone();
        let provider = LocalService::new(&base_url);
        let status = provider.poll(fleet_job).map_err(|e| format!("{e:?}"))?;
        if status.state == JOB_STATE_DONE {
            let artifact = status
                .artifacts
                .first()
                .ok_or("done without artifacts")?
                .clone();
            if artifact.content_type != "video/mp4" {
                return Err(format!("unexpected artifact type {}", artifact.content_type));
            }
            let bytes = provider
                .fetch_artifact(&artifact.id)
                .map_err(|e| format!("{e:?}"))?;
            verify_artifact_bytes(&bytes.bytes, &artifact).map_err(|e| format!("{e:?}"))?;
            self.routes.remove(fleet_job);
            return Ok(FleetPoll::Done { mp4: bytes.bytes });
        }
        if status.state == JOB_STATE_ERROR || status.state == "cancelled" {
            self.routes.remove(fleet_job);
            return Ok(FleetPoll::Failed {
                error: status.error.unwrap_or_else(|| status.state.clone()),
            });
        }
        Ok(FleetPoll::Running {
            stage: status.stage.unwrap_or_else(|| status.state.clone()),
            progress: status.progress.unwrap_or(0.0),
        })
    }

    fn cancel(&mut self, fleet_job: &str) {
        use makepad_asset_ai::client::{ContentProvider, LocalService};
        let Some(base_url) = self.routes.get(fleet_job).cloned() else {
            return;
        };
        match LocalService::new(&base_url).cancel(fleet_job) {
            Ok(_) => {
                self.routes.remove(fleet_job);
            }
            Err(error) if self.log => {
                eprintln!("[asset-worker] cancel on {base_url} failed: {error}");
            }
            Err(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use makepad_asset_store::{AssetServer, ServerConfig};
    use makepad_asset_ai::fleet::BoxSnapshot;
    use makepad_asset_ai::protocol::{HealthJson, ModelInfoJson, MODEL_STATE_READY};
    use makepad_asset_client::{ApiEndpoints, ClientConfig};
    use makepad_asset_data::{AssetId, AssetRevisionId};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicU64;

    static TEST_DIR: AtomicU64 = AtomicU64::new(0);

    fn test_root(name: &str) -> PathBuf {
        let n = TEST_DIR.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "mp_asset_worker_{}_{}_{}",
            std::process::id(),
            n,
            name
        ))
    }

    fn connect(server: &AssetServer, token: &str, cache: &Path) -> AssetClient {
        let mut config = ClientConfig::new(cache.to_path_buf());
        config.token = Some(token.to_string());
        AssetClient::connect(
            config,
            ApiEndpoints {
                control: server.control_addr(),
                data: server.data_addr(),
            },
            Some(server.server_id()),
        )
        .expect("connect to isolated test server")
    }

    #[derive(Default)]
    struct ScriptedFleet {
        requests: Vec<VideoRequest>,
    }

    impl VideoFleet for ScriptedFleet {
        fn dispatch(&mut self, request: &VideoRequest) -> Result<FleetDispatch, String> {
            self.requests.push(request.clone());
            Ok(FleetDispatch::Started {
                job: "fleet-job-1".to_string(),
                // Deliberately differ from the requested full model: the
                // published annotation/provenance must name what really ran.
                model: "minimax-h3-q4".to_string(),
                backend: "h3-quant".to_string(),
                version: "0.2.0-test".to_string(),
            })
        }

        fn poll(&mut self, _fleet_job: &str) -> Result<FleetPoll, String> {
            // The frame probe honestly falls back to its generated placeholder
            // for these deliberately minimal bytes; publication remains real.
            Ok(FleetPoll::Done { mp4: vec![0xAB; 4_096] })
        }

        fn cancel(&mut self, _fleet_job: &str) {}
    }

    fn model(id: &str, vram_gb: f64) -> ModelInfoJson {
        ModelInfoJson {
            id: id.to_string(),
            domain: "video".to_string(),
            backend: "h3".to_string(),
            available: true,
            gated: false,
            vram_gb: Some(vram_gb),
            note: None,
            state: MODEL_STATE_READY.to_string(),
            progress_done: None,
            progress_total: None,
            downloading_file: None,
            error: None,
            revision: None,
            unavailable_reason: None,
        }
    }

    fn snapshot(
        url: &str,
        free_mb: u64,
        total_mb: u64,
        models: Vec<ModelInfoJson>,
    ) -> BoxSnapshot {
        BoxSnapshot {
            base_url: url.to_string(),
            health: Some(HealthJson {
                service: "makepad-asset-ai".to_string(),
                version: "test".to_string(),
                gpu: None,
                vram_free_mb: Some(free_mb),
                vram_total_mb: Some(total_mb),
                models_loaded: Vec::new(),
                jobs_pending: Some(0),
                node_id: None,
                node_key: None,
                started_ms: None,
                capabilities: Some(vec!["video".to_string()]),
                vram_reserve_mb: Some(2 * 1024),
                queue_limit: Some(8),
            }),
            models,
        }
    }

    #[test]
    fn full_h3_waits_for_big_gpu_vram_instead_of_dispatching_small_gpu() {
        let h3 = model("minimax-h3", 90.0);
        let snapshots = vec![
            snapshot("http://big", 24 * 1024, 96 * 1024, vec![h3.clone()]),
            snapshot("http://small", 24 * 1024, 24 * 1024, vec![h3]),
        ];
        match select_video_route(&snapshots, "minimax-h3").unwrap() {
            VideoRoute::Waiting { stage } => {
                assert!(!stage.contains("http://"), "worker status must not leak fleet URLs");
                assert!(stage.contains("94208 MiB required"));
            }
            other => panic!("expected VRAM wait, got {other:?}"),
        }
    }

    #[test]
    fn requested_compatible_model_waits_instead_of_silent_model_fallback() {
        let snapshots = vec![
            snapshot(
                "http://big",
                24 * 1024,
                96 * 1024,
                vec![model("minimax-h3", 90.0)],
            ),
            snapshot(
                "http://quant-box",
                20 * 1024,
                24 * 1024,
                vec![model("minimax-h3-q4", 12.0)],
            ),
        ];
        assert!(matches!(
            select_video_route(&snapshots, "minimax-h3").unwrap(),
            VideoRoute::Waiting { .. }
        ));
        assert_eq!(
            select_video_route(&snapshots, "not-in-fleet").unwrap(),
            VideoRoute::Admitted {
                index: 1,
                model: "minimax-h3-q4".to_string(),
                backend: "h3".to_string(),
                version: "test".to_string(),
            }
        );
    }

    #[test]
    fn admitted_video_route_dispatches_only_after_free_vram_recovers() {
        let mut snapshots = vec![snapshot(
            "http://big",
            24 * 1024,
            96 * 1024,
            vec![model("minimax-h3", 90.0)],
        )];
        assert!(matches!(
            select_video_route(&snapshots, "minimax-h3").unwrap(),
            VideoRoute::Waiting { .. }
        ));
        snapshots[0].health.as_mut().unwrap().vram_free_mb = Some(95 * 1024);
        assert_eq!(
            select_video_route(&snapshots, "minimax-h3").unwrap(),
            VideoRoute::Admitted {
                index: 0,
                model: "minimax-h3".to_string(),
                backend: "h3".to_string(),
                version: "test".to_string(),
            }
        );
    }

    #[test]
    fn only_memory_admission_failures_are_retryable() {
        assert!(is_vram_admission_error(
            "model unavailable: insufficient VRAM for minimax-h3"
        ));
        assert!(!is_vram_admission_error("CUDA out of memory during inference"));
        assert!(!is_vram_admission_error("invalid video dimensions"));
        assert!(!is_vram_admission_error("unknown model"));
        assert_eq!(bounded("aéé", 4), "aé");
    }

    #[test]
    fn real_asset_server_coordinator_publishes_and_completes_typed_job() {
        let root = test_root("coordinator_e2e");
        let mut config = ServerConfig::new(root.clone());
        config.control_addr = "127.0.0.1:0".parse().unwrap();
        config.data_addr = "127.0.0.1:0".parse().unwrap();
        config.bootstrap_admin = true;
        config.log = false;
        let server = AssetServer::start(config).expect("isolated Asset Server");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();

        let mut submitter = connect(&server, &token, &root.join("submit-cache"));
        let body = obj(vec![
            ("prompt", s("a clean coordinator roundtrip")),
            ("model", s("minimax-h3")),
            ("seed", Value::Int(77)),
        ]);
        let foreign_job = submitter
            .enqueue_job("gen", "music.generate", &body)
            .expect("enqueue foreign kind");
        let job = submitter
            .enqueue_job("gen", "video.generate", &body)
            .expect("enqueue");

        let worker = connect(&server, &token, &root.join("worker-cache"));
        let mut fleet = ScriptedFleet::default();
        let outcome = {
            let mut coordinator = Coordinator {
                client: worker,
                fleet: &mut fleet,
                suffix: "test-worker".to_string(),
                rights: PublishRights::generated_cc0(),
                log: false,
            };
            coordinator
                .run_one(&AtomicBool::new(false))
                .expect("coordinator call")
                .expect("claimed one job")
        };
        assert_eq!(fleet.requests.len(), 1);
        assert_eq!(fleet.requests[0].model, "minimax-h3");

        let JobOutcome::Published { asset, revision } = outcome else {
            panic!("expected publication, got {outcome:?}")
        };
        let asset = AssetId::from_str(&asset).expect("published asset id");
        let revision = AssetRevisionId::from_str(&revision).expect("published revision");
        let status = submitter.job_status(&job).expect("completed status");
        assert_eq!(status.state, JobStateDto::Succeeded);
        assert_eq!(status.result_asset, Some(asset));
        assert_eq!(status.result_revision, Some(revision));
        assert_eq!(
            submitter.job_status(&foreign_job).expect("foreign status").state,
            JobStateDto::Pending
        );

        let manifest = submitter.fetch_asset_manifest(&revision).expect("published manifest");
        let provenance = manifest.provenance.expect("seeded typed provenance");
        assert_eq!(provenance.generator, "makepad-asset-ai");
        assert_eq!(provenance.model, "minimax-h3-q4");
        assert_eq!(provenance.version, "0.2.0-test");
        assert_eq!(provenance.seed, 77);
        submitter.cancel_job(&foreign_job).expect("cancel foreign fixture job");

        drop(submitter);
        drop(server);
        let _ = std::fs::remove_dir_all(root);
    }
}
