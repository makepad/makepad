//! The claim→dispatch→publish coordinator: one loop that turns queued
//! Asset Server generation jobs — of EVERY kind in
//! [`crate::gen_kinds::GEN_KINDS`], not just video — into published,
//! cueable catalog assets.
//!
//! Per job: claim under a lease (kind-filtered to what this worker's box can
//! actually run) → resolve any catalog input the body pins → pick a fleet box
//! advertising the model → dispatch → poll the box (heartbeating the Asset
//! Server lease with real stage/progress, and propagating Asset Server
//! cancellation to the box) → fetch + verify the artifact → build the typed
//! product row for the kind (thumbnail, measured duration/dims/stats) →
//! publish through the shared client (annotation before publish, so the
//! catalog event is kind-stamped and the VJ refreshes instantly) →
//! `worker_succeed` with the produced asset/revision. Failures report a
//! bounded error document. Credentials never leave this process.
//!
//! PRODUCTS ONLY: one job publishes exactly one catalog row — the final
//! artifact plus its thumbnail. A model that returns several artifacts (a
//! depth sidecar, a variants json) contributes at most typed files of that
//! same revision; none of them ever becomes a catalog entry of its own.
//!
//! The fleet transport sits behind [`GenFleet`], so the coordinator loop is
//! tested against a REAL Asset Server with a scripted fleet — the real
//! implementation ([`AssetAiFleet`]) is a thin adapter over
//! `makepad-asset-ai`'s blocking client.

use crate::gen_kinds::{kind_of, GenKind, InputNeed};
use crate::glb::inspect_glb;
use crate::import::{placeholder_thumb, usable_image_thumb};
use crate::thumbs::{
    audio_thumbnail_jpeg, decode_audio, encode_jpeg_bgra, jpeg_dims, placeholder_bgra_512,
    png_dims, THUMB_DIM,
};
use crate::videothumb::probe_video;
use makepad_asset_client::json::{obj, s, Value};
use makepad_asset_client::{
    AssetClient, ClaimedJobDto, ClientError, JobStateDto, PublishFile, PublishProvenance,
    PublishRequest, PublishRights, PublishStats, PublishThumbnail,
};
use makepad_asset_data::{
    AssetAlias, AssetRevisionId, FileRole, MediaType, ThumbnailMedia,
};
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
/// Largest catalog input this worker relays to a box, base64 included.
const MAX_INPUT_BYTES: u64 = 64 * 1024 * 1024;

/// One artifact the fleet produced.
#[derive(Clone, Debug)]
pub struct GenArtifact {
    pub content_type: String,
    pub bytes: Vec<u8>,
}

/// A dispatched fleet job's observed state.
#[derive(Clone, Debug)]
pub enum FleetPoll {
    Running { stage: String, progress: f64 },
    Done { artifacts: Vec<GenArtifact> },
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
pub trait GenFleet {
    /// Attempt one generation dispatch. A temporarily VRAM-blocked compatible
    /// node returns [`FleetDispatch::Waiting`], never a doomed submission.
    fn dispatch(&mut self, request: &GenRequest) -> Result<FleetDispatch, String>;
    fn poll(&mut self, fleet_job: &str) -> Result<FleetPoll, String>;
    fn cancel(&mut self, fleet_job: &str);
}

/// A source payload relayed from the catalog into the fleet request.
#[derive(Clone, Debug)]
pub struct GenInput {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// The typed job-body contract shared by every generation kind: a prompt, an
/// optional model pin, and the kind's own parameters passed through as the
/// client wrote them (the advertised profile's defaults, merged with
/// whatever the enqueuer added).
#[derive(Clone, Debug)]
pub struct GenRequest {
    pub kind: &'static GenKind,
    pub prompt: String,
    /// Empty = let domain affinity pick.
    pub model: String,
    pub seed: Option<u64>,
    /// The whole job body; the fleet adapter maps known keys onto the
    /// service's typed request and ignores the rest.
    pub body: Value,
    /// Resolved catalog input for transform kinds.
    pub input: Option<GenInput>,
}

impl GenRequest {
    /// Parse a claimed job body. A prompt is mandatory except for kinds that
    /// transform an input (an upscale has nothing to say about content).
    pub fn from_body(kind: &'static GenKind, body: &Value) -> Result<GenRequest, String> {
        let prompt = body
            .get("prompt")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if prompt.is_empty() && kind.input == InputNeed::None {
            return Err("job body has no prompt".to_string());
        }
        if prompt.len() > 4_000 {
            return Err("prompt too long".to_string());
        }
        Ok(GenRequest {
            kind,
            prompt,
            model: body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string(),
            seed: body.get("seed").and_then(Value::as_u64),
            body: body.clone(),
            input: None,
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
    pub fleet: &'f mut dyn GenFleet,
    pub suffix: String,
    /// Job kinds this worker claims. Built from the box's advertised
    /// capabilities, so a chat-only node never swallows an image job.
    pub kinds: Vec<String>,
    /// The operator's explicit rights declaration for generated output —
    /// set from the CLI, never invented here.
    pub rights: PublishRights,
    pub log: bool,
}

impl<'f> Coordinator<'f> {
    /// Claim and fully process at most one job. `Ok(None)` = queue empty.
    pub fn run_one(&mut self, stop: &AtomicBool) -> Result<Option<JobOutcome>, ClientError> {
        if self.kinds.is_empty() {
            // Nothing this worker can execute: claiming would be a lie, and
            // an empty kind list is a server-side error, not a wildcard.
            return Ok(None);
        }
        let kinds: Vec<&str> = self.kinds.iter().map(String::as_str).collect();
        let Some(claimed) =
            self.client
                .worker_claim_kinds(LEASE_MS, Some(&self.suffix), &kinds)?
        else {
            return Ok(None);
        };
        let Some(kind) = kind_of(&claimed.kind) else {
            // The server handed back a kind outside the wired table: fail
            // honestly (the attempt burns — claims cannot be returned).
            let error = obj(vec![(
                "error",
                s(format!("worker does not handle kind {}", claimed.kind)),
            )]);
            self.client
                .worker_fail(&claimed.job, Some(&self.suffix), 0, Some(&error))?;
            return Ok(Some(JobOutcome::Failed {
                error: format!("unsupported kind {}", claimed.kind),
            }));
        };
        let outcome = self.process_job(kind, &claimed, stop);
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

    fn process_job(
        &mut self,
        kind: &'static GenKind,
        claimed: &ClaimedJobDto,
        stop: &AtomicBool,
    ) -> Result<JobOutcome, ClientError> {
        let mut request = match GenRequest::from_body(kind, &claimed.body) {
            Ok(request) => request,
            Err(error) => return Ok(JobOutcome::Failed { error }),
        };
        self.log(&format!(
            "job {} [{}]: preparing \"{}\" ({})",
            claimed.job,
            kind.kind,
            request.prompt,
            if request.model.is_empty() { "auto" } else { &request.model }
        ));
        // Transform kinds need their source before anything is dispatched:
        // a box must never be asked to invent an input it was promised.
        match self.resolve_input(kind, &claimed.body) {
            Ok(input) => request.input = input,
            Err(error) => return Ok(JobOutcome::Failed { error }),
        }
        if kind.input != InputNeed::None && request.input.is_none() {
            return Ok(JobOutcome::Failed {
                error: format!(
                    "{} requires a source asset (job body needs source_alias or source_revision)",
                    kind.kind
                ),
            });
        }

        let started = Instant::now();
        let mut last_heartbeat = Instant::now();
        let mut last_cancel_check = Instant::now();
        let mut wait_stage: Option<String> = None;
        let mut retry_not_before: Option<Instant> = None;
        let (artifacts, generated_model, generated_backend, generator_version) =
            'generation: loop {
                // Selection is refreshed while a compatible GPU is temporarily
                // below its advertised admission target. This is also the retry
                // path when the service's authoritative, later admission check
                // beats our health snapshot.
                let (fleet_job, dispatched_model, dispatched_backend, dispatched_version) = loop {
                    if stop.load(Ordering::SeqCst) {
                        return Ok(JobOutcome::Failed { error: "worker shutdown".to_string() });
                    }
                    if started.elapsed() > JOB_DEADLINE {
                        return Ok(JobOutcome::Failed {
                            error: "generation deadline".to_string(),
                        });
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
                        return Ok(JobOutcome::Failed {
                            error: "generation deadline".to_string(),
                        });
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
                        Ok(FleetPoll::Done { artifacts }) => {
                            break 'generation (
                                artifacts,
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
                            heartbeat_permille = (progress.clamp(0.0, 1.0) * 1000.0) as u16;
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

        // The product is the artifact whose content type the kind declares.
        // A box that returned only sidecars is a failure, never a guess.
        let Some(product) = artifacts
            .into_iter()
            .find(|a| kind.content_types.iter().any(|ct| a.content_type == *ct))
        else {
            return Ok(JobOutcome::Failed {
                error: format!("fleet returned no {} artifact", kind.content_types[0]),
            });
        };

        let mut publish = match build_product(kind, &claimed.namespace, &request, product) {
            Ok(publish) => publish,
            Err(error) => return Ok(JobOutcome::Failed { error }),
        };
        // Stable job-derived alias: `<ns>/job-<16 hex>` (two segments).
        let job_hex = claimed.job.to_string();
        let alias_text = format!(
            "{}/job-{}",
            claimed.namespace,
            job_hex.trim_start_matches("job_").chars().take(16).collect::<String>()
        );
        publish.alias = AssetAlias::from_str(&alias_text).ok();
        publish.prompt = request.prompt.clone();
        publish.generator = "asset-worker".to_string();
        publish.backend = generated_backend;
        publish.model = generated_model.clone();
        // Rights come from the operator's explicit declaration, never from
        // a silent in-code default.
        publish.rights = self.rights.clone();
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

    /// Fetch the catalog payload a transform job pins, as `source_alias`
    /// (`ns/name`) or `source_revision` (an exact revision id). Nothing is
    /// inferred: a body without either resolves to `None`, and the caller
    /// refuses kinds that need one.
    fn resolve_input(
        &mut self,
        kind: &GenKind,
        body: &Value,
    ) -> Result<Option<GenInput>, String> {
        let revision = match (
            body.get("source_revision").and_then(Value::as_str),
            body.get("source_alias").and_then(Value::as_str),
        ) {
            (Some(rev), _) => AssetRevisionId::from_str(rev.trim())
                .map_err(|_| "malformed source_revision".to_string())?,
            (None, Some(alias)) => {
                let alias = AssetAlias::from_str(alias.trim())
                    .map_err(|_| "malformed source_alias".to_string())?;
                self.client
                    .resolve_alias(&alias)
                    .map_err(|e| format!("source alias: {e}"))?
                    .head_revision
            }
            (None, None) => return Ok(None),
        };
        let manifest = self
            .client
            .fetch_asset_manifest(&revision)
            .map_err(|e| format!("source manifest: {e}"))?;
        let wanted = |media: MediaType| match kind.input {
            InputNeed::Mesh => media == MediaType::Glb,
            _ => matches!(media, MediaType::Png | MediaType::Jpeg),
        };
        // Prefer the role that IS the payload (a render GLB, a texture) over
        // a retained original; both beat nothing.
        let preferred: &[FileRole] = match kind.input {
            InputNeed::Mesh => &[FileRole::RenderGlb, FileRole::Source],
            _ => &[FileRole::Texture, FileRole::Albedo, FileRole::Source],
        };
        let file = preferred
            .iter()
            .find_map(|role| {
                manifest
                    .files
                    .iter()
                    .find(|f| f.role == *role && wanted(f.media))
            })
            .or_else(|| manifest.files.iter().find(|f| wanted(f.media)))
            .ok_or_else(|| format!("source asset has no {} file", kind.input.content_type()))?;
        if file.byte_len > MAX_INPUT_BYTES {
            return Err(format!("source file too large ({} bytes)", file.byte_len));
        }
        let bytes = self
            .client
            .fetch_blob_bytes(&file.blob, Some(file.byte_len))
            .map_err(|e| format!("source blob: {e}"))?;
        let content_type = match file.media {
            MediaType::Glb => "model/gltf-binary",
            MediaType::Jpeg => "image/jpeg",
            _ => "image/png",
        };
        Ok(Some(GenInput { bytes, content_type: content_type.to_string() }))
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

// ---------------------------------------------------------------------------
// product shaping
// ---------------------------------------------------------------------------

/// Turn one verified artifact into the catalog row its kind declares:
/// measured dimensions/duration/mesh stats and a real thumbnail, never an
/// assumed one. Payloads are PARSED here, so an unloadable product fails the
/// job instead of becoming a catalog entry no viewer can open.
fn build_product(
    kind: &'static GenKind,
    ns: &str,
    request: &GenRequest,
    product: GenArtifact,
) -> Result<PublishRequest, String> {
    let mut title = makepad_asset_client::util::sanitize_text(&request.prompt, 120);
    if title.is_empty() {
        title = format!("Generated {}", kind.action);
    }
    let bytes = product.bytes;
    let mut stats = PublishStats::default();
    let mut extra_tags: Vec<String> = Vec::new();
    let (media_millis, dims, thumbnail) = match kind.media {
        MediaType::Png | MediaType::Jpeg => {
            let dims = match kind.media {
                MediaType::Jpeg => jpeg_dims(&bytes),
                _ => png_dims(&bytes),
            }
            .ok_or("image: malformed header")?;
            let thumbnail = match usable_image_thumb(&bytes) {
                Some((thumb, media, w, h)) => {
                    PublishThumbnail { bytes: thumb, media, width: w, height: h, views: Vec::new() }
                }
                None => placeholder_thumb()?,
            };
            (0, Some(dims), thumbnail)
        }
        MediaType::Wav | MediaType::Mp3 | MediaType::Ogg => {
            let pcm = decode_audio(&bytes, kind.media)?;
            let millis = pcm.millis();
            let picture = audio_thumbnail_jpeg(&pcm)?;
            let thumbnail = PublishThumbnail {
                bytes: picture.bytes,
                media: ThumbnailMedia::Jpeg,
                width: picture.width,
                height: picture.height,
                views: picture.views,
            };
            (millis, None, thumbnail)
        }
        MediaType::Mp4 => {
            // The frame probe needs a file; the temp copy dies with this call.
            let tmp = std::env::temp_dir().join(format!(
                "asset-worker-{}-{}.mp4",
                std::process::id(),
                makepad_asset_client::util::to_hex(&[
                    (bytes.len() as u32).to_le_bytes()[0],
                    (bytes.len() as u32).to_le_bytes()[1],
                    (bytes.len() as u32).to_le_bytes()[2],
                    (bytes.len() as u32).to_le_bytes()[3],
                ])
            ));
            let probe = std::fs::write(&tmp, &bytes)
                .map_err(|e| e.to_string())
                .and_then(|_| probe_video(&tmp));
            let _ = std::fs::remove_file(&tmp);
            match probe {
                Ok(p) => (
                    p.duration_ms,
                    None,
                    PublishThumbnail {
                        bytes: p.thumbnail_jpeg,
                        media: ThumbnailMedia::Jpeg,
                        width: THUMB_DIM as u32,
                        height: THUMB_DIM as u32,
                        views: Vec::new(),
                    },
                ),
                Err(_) => {
                    // An honest placeholder plus the tag that says so; the
                    // clip itself is verified bytes and stays publishable.
                    extra_tags.push("no-preview-frame".to_string());
                    let jpeg =
                        encode_jpeg_bgra(&placeholder_bgra_512(), THUMB_DIM, THUMB_DIM)?;
                    (
                        0,
                        None,
                        PublishThumbnail {
                            bytes: jpeg,
                            media: ThumbnailMedia::Jpeg,
                            width: THUMB_DIM as u32,
                            height: THUMB_DIM as u32,
                            views: Vec::new(),
                        },
                    )
                }
            }
        }
        MediaType::Glb => {
            let inspected = inspect_glb(&bytes)?;
            stats = PublishStats {
                triangles: inspected.triangles,
                vertices: inspected.vertices,
                joints: inspected.joints,
                clips: inspected.clips,
            };
            let thumbnail = match inspected.base_color.as_deref().and_then(usable_image_thumb) {
                Some((thumb, media, w, h)) => {
                    PublishThumbnail { bytes: thumb, media, width: w, height: h, views: Vec::new() }
                }
                None => placeholder_thumb()?,
            };
            (0, None, thumbnail)
        }
        MediaType::Ply => {
            let scene = makepad_splat::load_splat_from_bytes(&bytes, Some(std::path::Path::new(
                "product.ply",
            )))
            .map_err(|e| format!("ply: {e}"))?;
            if scene.splats.is_empty() {
                return Err("ply: no splats".to_string());
            }
            // Splat previews are a renderer's job (the Asset UI backfills
            // them offscreen); publishing an honest placeholder is better
            // than a fabricated image.
            (0, None, placeholder_thumb()?)
        }
        other => return Err(format!("unsupported product media {other:?}")),
    };

    let mut request_out = PublishRequest::new(
        ns,
        kind.asset_kind,
        title,
        PublishFile {
            bytes,
            media: kind.media,
            role: kind.role,
            media_millis,
            dims,
        },
        thumbnail,
    );
    request_out.categories = vec![kind.category.to_string()];
    request_out.tags = kind.tags.iter().map(|t| t.to_string()).collect();
    request_out.tags.extend(extra_tags);
    // Client-proposed tags ride the job body (the VJ's loop pipe tags its
    // clips `loop`). Bounded and charset-checked here so a buggy client
    // cannot spray the catalog with junk rows.
    if let Some(tags) = request.body.get("tags").and_then(Value::as_arr) {
        for tag in tags.iter().filter_map(Value::as_str).take(4) {
            let tag = tag.trim().to_ascii_lowercase();
            let ok = (2..=24).contains(&tag.len())
                && tag.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
            if ok && !request_out.tags.contains(&tag) {
                request_out.tags.push(tag);
            }
        }
    }
    request_out.stats = stats;
    Ok(request_out)
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
///
/// A single-URL adapter is also the fan-out primitive: one per box, each
/// driving its own claim loop, gives the fleet exactly one job per box
/// without any cross-worker coordination.
pub struct AssetAiFleet {
    boxes: Vec<String>,
    discovered: Option<makepad_asset_ai::discovery::Discovered>,
    log: bool,
    /// The box a dispatched job lives on: `fleet_job -> base_url`.
    routes: std::collections::HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum GenRoute {
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

    pub fn from_urls(boxes: Vec<String>, log: bool) -> AssetAiFleet {
        AssetAiFleet {
            boxes,
            discovered: None,
            log,
            routes: Default::default(),
        }
    }

    pub fn boxes(&self) -> Vec<String> {
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

    /// Health + model snapshot of every box this adapter can reach.
    pub fn snapshots(&self) -> (Vec<makepad_asset_ai::fleet::BoxSnapshot>, bool) {
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
        (snapshots, probe_incomplete)
    }

    /// Domains every reachable box advertises, deduplicated.
    pub fn capabilities(&self) -> Vec<String> {
        let (snapshots, _) = self.snapshots();
        let mut out: Vec<String> = Vec::new();
        for snapshot in &snapshots {
            let Some(health) = snapshot.health.as_ref() else { continue };
            for domain in health.capabilities.iter().flatten() {
                if !out.contains(domain) {
                    out.push(domain.clone());
                }
            }
        }
        out
    }

    /// Snapshot every box and select only a route whose latest free-VRAM
    /// facts pass admission. A hardware-compatible target under transient
    /// pressure remains a wait candidate instead of becoming an error or a
    /// fallback submission.
    fn pick_box(&self, domain: &str, want_model: &str) -> Result<GenRoute, String> {
        let (snapshots, probe_incomplete) = self.snapshots();
        match select_route(&snapshots, domain, want_model) {
            Err(_) if probe_incomplete => Ok(GenRoute::Waiting {
                stage: "waiting-for-fleet: capability probe incomplete".to_string(),
            }),
            result => result,
        }
    }
}

/// Route one request: an explicit model pin is honoured whenever any
/// compatible GPU advertises it, otherwise the domain's best model wins.
fn select_route(
    snapshots: &[makepad_asset_ai::fleet::BoxSnapshot],
    domain: &str,
    want_model: &str,
) -> Result<GenRoute, String> {
    use makepad_asset_ai::fleet::{
        model_admission, pick_box_admitted_scored, pick_box_scored,
        pick_for_domain_admitted_scored, pick_for_domain_scored,
    };

    // An explicit/requested model remains a pin whenever any compatible GPU
    // advertises it. Prefer any admitted copy; otherwise hold for the best
    // compatible copy instead of silently changing models.
    if !want_model.is_empty() {
        if let Some((index, _)) = pick_box_admitted_scored(snapshots, want_model) {
            return Ok(admitted_route(snapshots, index, want_model.to_string()));
        }
        if let Some((index, _)) = pick_box_scored(snapshots, want_model) {
            return Ok(GenRoute::Waiting {
                stage: waiting_stage(
                    want_model,
                    model_admission(&snapshots[index], want_model),
                ),
            });
        }
    }

    // The requested id is absent/unavailable/incompatible fleet-wide: retain
    // the historical domain fallback, but apply the same admission contract.
    if let Some((index, model, _)) = pick_for_domain_admitted_scored(snapshots, domain) {
        return Ok(admitted_route(snapshots, index, model));
    }
    if let Some((index, model, _)) = pick_for_domain_scored(snapshots, domain) {
        let admission = model_admission(&snapshots[index], &model);
        return Ok(GenRoute::Waiting { stage: waiting_stage(&model, admission) });
    }
    Err(format!(
        "no fleet box advertises a hardware-compatible {domain} model"
    ))
}

fn waiting_stage(
    model: &str,
    admission: Option<makepad_asset_ai::fleet::VramAdmission>,
) -> String {
    use makepad_asset_ai::fleet::VramAdmission;
    match admission {
        Some(VramAdmission::Waiting { required_free_mb, free_mb }) => format!(
            "waiting-for-vram: model {model} has {free_mb} MiB free, {required_free_mb} MiB required"
        ),
        _ => format!("waiting-for-vram: model {model} awaits a fresh admission snapshot"),
    }
}

fn admitted_route(
    snapshots: &[makepad_asset_ai::fleet::BoxSnapshot],
    index: usize,
    model: String,
) -> GenRoute {
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
    GenRoute::Admitted { index, model, backend, version }
}

/// Map the job body onto the service's typed request. Only keys the service
/// actually understands are forwarded; unknown ones are ignored rather than
/// smuggled through, so a client typo fails visibly at the model instead of
/// silently changing nothing.
fn wire_request(
    request: &GenRequest,
    model: String,
) -> makepad_asset_ai::protocol::GenerateRequestJson {
    use makepad_asset_ai::protocol::GenerateRequestJson;
    let body = &request.body;
    let u32_of = |key: &str| body.get(key).and_then(Value::as_u64).map(|v| v as u32);
    // JSON numbers reach us as either variant; a client writing `30` for a
    // seconds field must mean the same thing as `30.0`.
    let f64_of = |key: &str| {
        body.get(key).and_then(|v| match v {
            Value::F64(f) => Some(*f),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        })
    };
    let str_of = |key: &str| {
        body.get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let mut wire = GenerateRequestJson {
        model,
        prompt: (!request.prompt.is_empty()).then(|| request.prompt.clone()),
        negative_prompt: str_of("negative_prompt"),
        width: u32_of("width"),
        height: u32_of("height"),
        seed: request.seed,
        steps: u32_of("steps"),
        guidance: f64_of("guidance"),
        // Video
        frames: u32_of("frames"),
        codec: str_of("codec").or_else(|| {
            (request.kind.media == makepad_asset_data::MediaType::Mp4)
                .then(|| "h264".to_string())
        }),
        audio: body.get("audio").and_then(Value::as_bool),
        interpolate: u32_of("interpolate"),
        // Audio / music / speech
        seconds: f64_of("seconds"),
        lyrics: str_of("lyrics"),
        text: str_of("text"),
        voice: str_of("voice"),
        speed: f64_of("speed"),
        // Mesh / splat
        remesh_resolution: u32_of("remesh_resolution"),
        texture: body.get("texture").and_then(Value::as_bool),
        decimation_target: u32_of("decimation_target"),
        texture_size: u32_of("texture_size"),
        gaussians: u32_of("gaussians"),
        motion_mode: str_of("motion_mode"),
        // Image transforms
        strength: f64_of("strength").map(|v| v as f32),
        canny_low: f64_of("canny_low"),
        canny_high: f64_of("canny_high"),
        ..Default::default()
    };
    if let Some(input) = &request.input {
        wire.input_b64 = String::from_utf8(makepad_base64::base64_encode(
            &input.bytes,
            &makepad_base64::BASE64_STANDARD,
        ))
        .ok();
        wire.input_content_type = Some(input.content_type.clone());
    }
    wire
}

impl GenFleet for AssetAiFleet {
    fn dispatch(&mut self, request: &GenRequest) -> Result<FleetDispatch, String> {
        use makepad_asset_ai::client::{ContentProvider, LocalService};
        use makepad_asset_ai::registry::Domain;
        let (index, model, backend, version) =
            match self.pick_box(request.kind.domain, &request.model)? {
                GenRoute::Admitted { index, model, backend, version } => {
                    (index, model, backend, version)
                }
                GenRoute::Waiting { stage } => return Ok(FleetDispatch::Waiting { stage }),
            };
        let base_url = self.boxes()[index].clone();
        if self.log {
            eprintln!(
                "[asset-worker] dispatch {} to {base_url} model {model}",
                request.kind.kind
            );
        }
        let Some(domain) = Domain::parse(request.kind.domain) else {
            return Err(format!("unknown fleet domain {}", request.kind.domain));
        };
        let wire = wire_request(request, model);
        let provider = LocalService::new(&base_url);
        let fleet_job = match provider.request(domain, &wire) {
            Ok(job) => job,
            Err(
                makepad_asset_ai::error::AssetAiError::Busy
                | makepad_asset_ai::error::AssetAiError::QueueFull(_),
            ) => {
                return Ok(FleetDispatch::Waiting {
                    stage: "waiting-for-fleet: selected node queue is full".to_string(),
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
            if status.artifacts.is_empty() {
                return Err("done without artifacts".to_string());
            }
            let mut out = Vec::with_capacity(status.artifacts.len());
            for artifact in &status.artifacts {
                let bytes = provider
                    .fetch_artifact(&artifact.id)
                    .map_err(|e| format!("{e:?}"))?;
                verify_artifact_bytes(&bytes.bytes, artifact).map_err(|e| format!("{e:?}"))?;
                out.push(GenArtifact {
                    content_type: artifact.content_type.clone(),
                    bytes: bytes.bytes,
                });
            }
            self.routes.remove(fleet_job);
            return Ok(FleetPoll::Done { artifacts: out });
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
    use crate::gen_kinds::kind_of;
    use makepad_asset_ai::fleet::BoxSnapshot;
    use makepad_asset_ai::protocol::{HealthJson, ModelInfoJson, MODEL_STATE_READY};
    use makepad_asset_client::{ApiEndpoints, ClientConfig};
    use makepad_asset_data::{AssetId, AssetKind};
    use makepad_asset_store::{AssetServer, ServerConfig};
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

    /// A 1x1 PNG: the smallest byte string that is a REAL decodable image,
    /// so the publish path's header parse and thumbnail are exercised for
    /// real rather than mocked away.
    fn tiny_png() -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        let chunk = |png: &mut Vec<u8>, tag: &[u8; 4], data: &[u8]| {
            png.extend_from_slice(&(data.len() as u32).to_be_bytes());
            png.extend_from_slice(tag);
            png.extend_from_slice(data);
            let mut crc_input = tag.to_vec();
            crc_input.extend_from_slice(data);
            png.extend_from_slice(&crc32(&crc_input).to_be_bytes());
        };
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&1u32.to_be_bytes());
        ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
        chunk(&mut png, b"IHDR", &ihdr);
        // One zlib stream holding a single uncompressed deflate block with
        // the filter byte + one RGB pixel.
        let raw = [0u8, 0xFF, 0x40, 0x20];
        let mut z = vec![0x78, 0x01, 0x01, 4, 0, 0xFB, 0xFF];
        z.extend_from_slice(&raw);
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for byte in raw {
            a = (a + byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        z.extend_from_slice(&((b << 16) | a).to_be_bytes());
        chunk(&mut png, b"IDAT", &z);
        chunk(&mut png, b"IEND", &[]);
        png
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for byte in data {
            crc ^= *byte as u32;
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// Fleet stub: records what it was asked for and returns a real product
    /// of the requested kind's declared content type.
    #[derive(Default)]
    struct ScriptedFleet {
        requests: Vec<GenRequest>,
        /// Set to fail every dispatch with this error.
        fail: Option<String>,
    }

    impl GenFleet for ScriptedFleet {
        fn dispatch(&mut self, request: &GenRequest) -> Result<FleetDispatch, String> {
            if let Some(error) = &self.fail {
                return Err(error.clone());
            }
            self.requests.push(request.clone());
            Ok(FleetDispatch::Started {
                job: format!("fleet-job-{}", self.requests.len()),
                // Deliberately differ from the requested full model: the
                // published annotation/provenance must name what really ran.
                model: format!("{}-q4", request.model),
                backend: "scripted".to_string(),
                version: "0.2.0-test".to_string(),
            })
        }

        fn poll(&mut self, _fleet_job: &str) -> Result<FleetPoll, String> {
            Ok(FleetPoll::Done {
                artifacts: vec![
                    // A sidecar the kind does not declare: it must be
                    // ignored, never published as a row of its own.
                    GenArtifact {
                        content_type: "application/json".to_string(),
                        bytes: b"{\"variants\":[]}".to_vec(),
                    },
                    GenArtifact {
                        content_type: "image/png".to_string(),
                        bytes: tiny_png(),
                    },
                ],
            })
        }

        fn cancel(&mut self, _fleet_job: &str) {}
    }

    fn model(id: &str, domain: &str, vram_gb: f64) -> ModelInfoJson {
        ModelInfoJson {
            id: id.to_string(),
            domain: domain.to_string(),
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
            license_name: None,
            license_url: None,
            license_summary: None,
            license_restriction: None,
            license_sha256: None,
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
                fleet: None,
                lanes: None,
            }),
            models,
        }
    }

    #[test]
    fn full_h3_waits_for_big_gpu_vram_instead_of_dispatching_small_gpu() {
        let h3 = model("minimax-h3", "video", 90.0);
        let snapshots = vec![
            snapshot("http://big", 24 * 1024, 96 * 1024, vec![h3.clone()]),
            snapshot("http://small", 24 * 1024, 24 * 1024, vec![h3]),
        ];
        match select_route(&snapshots, "video", "minimax-h3").unwrap() {
            GenRoute::Waiting { stage } => {
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
                vec![model("minimax-h3", "video", 90.0)],
            ),
            snapshot(
                "http://quant-box",
                20 * 1024,
                24 * 1024,
                vec![model("minimax-h3-q4", "video", 12.0)],
            ),
        ];
        assert!(matches!(
            select_route(&snapshots, "video", "minimax-h3").unwrap(),
            GenRoute::Waiting { .. }
        ));
        assert_eq!(
            select_route(&snapshots, "video", "not-in-fleet").unwrap(),
            GenRoute::Admitted {
                index: 1,
                model: "minimax-h3-q4".to_string(),
                backend: "h3".to_string(),
                version: "test".to_string(),
            }
        );
        // No pin at all takes the same domain path.
        assert_eq!(
            select_route(&snapshots, "video", "").unwrap(),
            GenRoute::Admitted {
                index: 1,
                model: "minimax-h3-q4".to_string(),
                backend: "h3".to_string(),
                version: "test".to_string(),
            }
        );
    }

    #[test]
    fn admitted_route_dispatches_only_after_free_vram_recovers() {
        let mut snapshots = vec![snapshot(
            "http://big",
            24 * 1024,
            96 * 1024,
            vec![model("minimax-h3", "video", 90.0)],
        )];
        assert!(matches!(
            select_route(&snapshots, "video", "minimax-h3").unwrap(),
            GenRoute::Waiting { .. }
        ));
        snapshots[0].health.as_mut().unwrap().vram_free_mb = Some(95 * 1024);
        assert_eq!(
            select_route(&snapshots, "video", "minimax-h3").unwrap(),
            GenRoute::Admitted {
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

    /// The VJ's loop pipe tags its clips `loop` through the job body; the
    /// worker forwards bounded, charset-clean tags and drops the rest.
    #[test]
    fn client_tags_ride_the_body_bounded_and_sanitized() {
        let kind = kind_of("image.generate").unwrap();
        let body = obj(vec![
            ("prompt", s("a looping tunnel")),
            (
                "tags",
                Value::Arr(vec![
                    s("loop"),
                    s(" LOOP "),                    // dup after normalize
                    s("x"),                          // too short
                    s("has spaces"),                 // bad charset
                    s("this-tag-is-far-too-long-to-keep"), // too long
                    s("ok-2"),
                ]),
            ),
        ]);
        let request = GenRequest::from_body(kind, &body).unwrap();
        let product = GenArtifact {
            content_type: "image/png".to_string(),
            bytes: tiny_png(),
        };
        let publish = build_product(kind, "gen", &request, product).unwrap();
        assert!(publish.tags.contains(&"loop".to_string()), "{:?}", publish.tags);
        assert_eq!(
            publish.tags.iter().filter(|t| *t == "loop").count(),
            1,
            "{:?}",
            publish.tags
        );
        // Take-4 bound is applied BEFORE filtering, so ok-2 (position 6) is
        // dropped with the junk; the junk itself never lands.
        assert!(!publish.tags.iter().any(|t| t.contains(' ') || t.len() > 24), "{:?}", publish.tags);
    }

    #[test]
    fn the_wire_request_carries_each_domains_own_parameters() {
        let kind = kind_of("music.generate").unwrap();
        let body = obj(vec![
            ("prompt", s("a slow dub techno loop")),
            ("seconds", Value::F64(30.0)),
            ("lyrics", s("[Instrumental]")),
            ("seed", Value::Int(9)),
            ("nonsense", s("ignored")),
        ]);
        let request = GenRequest::from_body(kind, &body).unwrap();
        let wire = wire_request(&request, "minimax-music3".to_string());
        assert_eq!(wire.seconds, Some(30.0));
        assert_eq!(wire.lyrics.as_deref(), Some("[Instrumental]"));
        assert_eq!(wire.seed, Some(9));
        // A wav product never asks for a video codec.
        assert_eq!(wire.codec, None);

        // Video defaults the codec to the compatibility fallback exactly as
        // the old video-only coordinator did.
        let kind = kind_of("video.generate").unwrap();
        let body = obj(vec![("prompt", s("a clip")), ("frames", Value::Int(65))]);
        let request = GenRequest::from_body(kind, &body).unwrap();
        let wire = wire_request(&request, "minimax-h3".to_string());
        assert_eq!(wire.codec.as_deref(), Some("h264"));
        assert_eq!(wire.frames, Some(65));
    }

    #[test]
    fn a_transform_kind_needs_a_prompt_less_body_but_a_generator_does_not() {
        let upscale = kind_of("image.upscale").unwrap();
        let body = obj(vec![("source_alias", s("gen/pic"))]);
        assert!(GenRequest::from_body(upscale, &body).is_ok());
        let image = kind_of("image.generate").unwrap();
        assert!(GenRequest::from_body(image, &body).is_err());
        assert!(GenRequest::from_body(
            image,
            &obj(vec![("prompt", s(" ".repeat(4_001)))])
        )
        .is_err());
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
            ("model", s("flux1-schnell")),
            ("seed", Value::Int(77)),
        ]);
        // A kind this worker does NOT claim must stay untouched in the queue.
        let foreign_job = submitter
            .enqueue_job("gen", "video.generate", &body)
            .expect("enqueue foreign kind");
        let job = submitter
            .enqueue_job("gen", "image.generate", &body)
            .expect("enqueue");

        let worker = connect(&server, &token, &root.join("worker-cache"));
        let mut fleet = ScriptedFleet::default();
        let outcome = {
            let mut coordinator = Coordinator {
                client: worker,
                fleet: &mut fleet,
                suffix: "test-worker".to_string(),
                kinds: vec!["image.generate".to_string()],
                rights: PublishRights::generated_cc0(),
                log: false,
            };
            coordinator
                .run_one(&AtomicBool::new(false))
                .expect("coordinator call")
                .expect("claimed one job")
        };
        assert_eq!(fleet.requests.len(), 1);
        assert_eq!(fleet.requests[0].model, "flux1-schnell");

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
        // PRODUCTS ONLY: the json sidecar the fleet also returned is not a
        // file of this revision and certainly not a row of its own.
        assert_eq!(manifest.kind, AssetKind::Texture);
        assert_eq!(manifest.files.len(), 1);
        assert_eq!(manifest.files[0].role, FileRole::Texture);
        assert_eq!(manifest.files[0].media, MediaType::Png);
        let provenance = manifest.provenance.expect("seeded typed provenance");
        assert_eq!(provenance.generator, "makepad-asset-ai");
        assert_eq!(provenance.model, "flux1-schnell-q4");
        assert_eq!(provenance.version, "0.2.0-test");
        assert_eq!(provenance.seed, 77);
        submitter.cancel_job(&foreign_job).expect("cancel foreign fixture job");

        drop(submitter);
        drop(server);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn queued_jobs_fan_out_across_every_box_one_at_a_time() {
        // Three queued jobs, three independent per-box coordinators: each
        // claims exactly one, so N free boxes drain N jobs in parallel and a
        // continuous submitter saturates the fleet.
        let root = test_root("fanout_e2e");
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
        let mut jobs = Vec::new();
        for i in 0..3 {
            let body = obj(vec![
                ("prompt", s(format!("continuous {i}"))),
                ("model", s("flux1-schnell")),
            ]);
            jobs.push(
                submitter
                    .enqueue_job("gen", "image.generate", &body)
                    .expect("enqueue"),
            );
        }

        let mut published = Vec::new();
        for box_index in 0..3 {
            let worker = connect(&server, &token, &root.join(format!("worker-{box_index}")));
            let mut fleet = ScriptedFleet::default();
            let mut coordinator = Coordinator {
                client: worker,
                fleet: &mut fleet,
                suffix: format!("box{box_index}"),
                kinds: vec!["image.generate".to_string()],
                rights: PublishRights::generated_cc0(),
                log: false,
            };
            let outcome = coordinator
                .run_one(&AtomicBool::new(false))
                .expect("coordinator call")
                .expect("each box claims one of the three queued jobs");
            // Exactly one dispatch per box: the one-job-per-box rule.
            assert_eq!(fleet.requests.len(), 1);
            match outcome {
                JobOutcome::Published { asset, .. } => published.push(asset),
                other => panic!("expected publication, got {other:?}"),
            }
        }
        // Three distinct products, and the queue is drained.
        published.sort();
        published.dedup();
        assert_eq!(published.len(), 3);
        for job in &jobs {
            assert_eq!(
                submitter.job_status(job).expect("status").state,
                JobStateDto::Succeeded
            );
        }

        drop(submitter);
        drop(server);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_worker_with_no_capability_claims_nothing_and_a_dead_fleet_fails_visibly() {
        let root = test_root("empty_kinds");
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
        let body = obj(vec![("prompt", s("nobody can run this"))]);
        let job = submitter
            .enqueue_job("gen", "image.generate", &body)
            .expect("enqueue");

        // A chat-only box wires to zero kinds: it must leave the job alone
        // rather than claim-and-fail it.
        let mut fleet = ScriptedFleet::default();
        {
            let mut coordinator = Coordinator {
                client: connect(&server, &token, &root.join("chat-box")),
                fleet: &mut fleet,
                suffix: "chatbox".to_string(),
                kinds: Vec::new(),
                rights: PublishRights::generated_cc0(),
                log: false,
            };
            assert!(coordinator.run_one(&AtomicBool::new(false)).unwrap().is_none());
        }
        assert_eq!(
            submitter.job_status(&job).expect("status").state,
            JobStateDto::Pending
        );

        // A capable worker whose fleet refuses fails the job with a real
        // reason instead of leaving it leased forever.
        let mut fleet = ScriptedFleet {
            fail: Some("no fleet box advertises a hardware-compatible image model".to_string()),
            ..Default::default()
        };
        let outcome = {
            let mut coordinator = Coordinator {
                client: connect(&server, &token, &root.join("gpu-box")),
                fleet: &mut fleet,
                suffix: "gpubox".to_string(),
                kinds: vec!["image.generate".to_string()],
                rights: PublishRights::generated_cc0(),
                log: false,
            };
            coordinator.run_one(&AtomicBool::new(false)).unwrap().unwrap()
        };
        let JobOutcome::Failed { error } = outcome else {
            panic!("expected failure, got {outcome:?}")
        };
        assert!(error.contains("hardware-compatible image model"), "{error}");
        assert_eq!(
            submitter.job_status(&job).expect("status").state,
            JobStateDto::Failed
        );

        drop(submitter);
        drop(server);
        let _ = std::fs::remove_dir_all(root);
    }
}
