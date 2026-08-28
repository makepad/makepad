//! The claim→dispatch→settle coordinator: one loop that turns queued
//! Asset Server jobs — of EVERY kind in [`crate::gen_kinds::GEN_KINDS`] —
//! into what that kind's product is: a published catalog asset, a text
//! answer recorded on the job, or a rewritten annotation record.
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
//! Two kinds do not publish at all, and the kind table says so
//! ([`crate::gen_kinds::Product`]) rather than the loop guessing:
//!
//! * `vision.describe` — a question about an image. The answer is TEXT,
//!   recorded on the job (`{text, model, box}`), which is where the client
//!   that asked reads it back. A UI making content asks these at runtime.
//! * `annotate.asset` — the catalog's own description pass, minted by the
//!   store on every publish. The answer is parsed by `makepad-asset-annotate`
//!   and folded into that asset's annotation record. Both live on the same
//!   `vision` capability, so one box that advertises it drains both queues,
//!   one job at a time, exactly like every GPU kind here.
//!
//! The fleet transport sits behind [`GenFleet`], so the coordinator loop is
//! tested against a REAL Asset Server with a scripted fleet — the real
//! implementation ([`AssetAiFleet`]) is a thin adapter over
//! `makepad-asset-ai`'s blocking client.

use crate::gen_kinds::{kind_of, GenKind, InputNeed};
use crate::gen_profiles::slug;
use crate::glb::inspect_glb;
use crate::import::{placeholder_thumb, usable_image_thumb};
use crate::thumbs::{
    audio_thumbnail_jpeg, decode_audio, encode_jpeg_bgra, jpeg_dims, placeholder_bgra_512,
    png_dims, THUMB_DIM,
};
use crate::videothumb::probe_video;
use makepad_asset_annotate::pass::{self, SheetPrep};
use makepad_asset_annotate::sheet;
use makepad_asset_annotate::plan::{Annotator, BaseAnnotation};
use makepad_asset_annotate::{needs_annotation, parse_record, plan_upload, ANNOTATOR_VERSION};
use makepad_asset_client::json::{obj, s, Value};
use makepad_asset_client::{
    AnnotationUpload, AssetClient, ClaimedJobDto, ClientError, JobStateDto, PublishFile,
    PublishProvenance, PublishRequest, PublishRights, PublishStats, PublishThumbnail,
};
use makepad_asset_data::{
    AssetAlias, AssetId, AssetRevisionId, FileRole, MediaType, ThumbnailMedia,
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
/// Reply budget for a vision answer. The annotation pass answers in nine
/// short lines; a client asking its own question may raise it in the body.
const VISION_MAX_TOKENS: u64 = 220;
/// Ceiling on the answer text recorded on a job (the server caps the whole
/// result document at 16 KB).
const MAX_ANSWER_BYTES: usize = 8 * 1024;

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
    /// Finished. `text` is the completed answer of a text-answering job
    /// (the service's `text`, falling back to the streamed `partial_text`);
    /// `None` on jobs that produce artifacts instead.
    Done {
        artifacts: Vec<GenArtifact>,
        text: Option<String>,
    },
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
    /// Short human tag for the box serving `fleet_job` (".203"), when the
    /// transport knows one. Rides the progress note so an operator watching
    /// a drawer of jobs can see WHICH GPU is on each — it is their fleet.
    fn route_label(&self, _fleet_job: &str) -> Option<String> {
        None
    }

    /// LAN host of the box serving `fleet_job` ("10.0.0.203"), when the
    /// transport knows one. The vision progress line names the box in full:
    /// an operator watching an annotation backlog drain wants to know WHICH
    /// GPU is answering, not just that one is.
    fn route_host(&self, _fleet_job: &str) -> Option<String> {
        None
    }

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
        // A prompt is mandatory except for kinds that only TRANSFORM an
        // input (an upscale has nothing to say about content). A question
        // about an image is not a transform: without the question there is
        // nothing to answer.
        if prompt.is_empty() && (kind.input == InputNeed::None || kind.is_text()) {
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
    /// A text answer, recorded on the job for whoever asked.
    Answered {
        text: String,
        model: String,
        /// LAN host that answered, empty when the transport knows none.
        host: String,
    },
    /// An annotation record rewritten in place.
    Described {
        asset: String,
        description: String,
        model: String,
    },
    Failed { error: String },
    CancelledUpstream,
}

/// How the per-job heartbeat note reads, which is what an operator sees in
/// the RUNS list while the job is alive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StageStyle {
    /// The fleet's own stage words, tagged with the short node label:
    /// "@.203 denoise".
    Fleet,
    /// "vision · qwen3.8-27b-vision @ 10.0.0.203 · 3.4 s" — a vision answer
    /// has no stages worth showing, so the line says which model on which
    /// box, and how long it has been thinking.
    Vision,
}

/// One finished fleet dispatch.
struct FleetAnswer {
    artifacts: Vec<GenArtifact>,
    text: Option<String>,
    model: String,
    backend: String,
    version: String,
    host: Option<String>,
    elapsed: Duration,
}

/// A dispatch that either produced an answer or ended the job.
enum FleetRun {
    Done(FleetAnswer),
    /// Terminal without an answer (failed, deadline, cancelled upstream).
    Outcome(JobOutcome),
}

pub struct Coordinator<'f> {
    pub client: AssetClient,
    pub fleet: &'f mut dyn GenFleet,
    pub suffix: String,
    /// Job kinds this worker claims. Built from the box's advertised
    /// capabilities, so a chat-only node never swallows an image job and a
    /// vision box claims both `vision.describe` and `annotate.asset`.
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
            // The answer IS the product: it is recorded on the job, and
            // `GET /v1/jobs/<id>` is where the client that asked reads it.
            Ok(JobOutcome::Answered { text, model, host }) => {
                let result = obj(vec![
                    ("text", s(bounded_answer(text, MAX_ANSWER_BYTES))),
                    ("model", s(model.clone())),
                    ("box", s(host.clone())),
                ]);
                self.client
                    .worker_succeed(&claimed.job, Some(&self.suffix), Some(&result))?;
            }
            Ok(JobOutcome::Described { asset, description, model }) => {
                let result = obj(vec![
                    ("asset_id", s(asset.clone())),
                    ("description", s(bounded(description, 1_000))),
                    ("model", s(model.clone())),
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

    /// Route one claimed job by what its kind PRODUCES. The table decides;
    /// this is the only branch on it.
    fn process_job(
        &mut self,
        kind: &'static GenKind,
        claimed: &ClaimedJobDto,
        stop: &AtomicBool,
    ) -> Result<JobOutcome, ClientError> {
        if kind.is_annotation() {
            return self.process_annotation(kind, claimed, stop);
        }
        self.process_generation(kind, claimed, stop)
    }

    fn process_generation(
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
                    "{} requires a source asset (job body needs {})",
                    kind.kind,
                    if kind.is_text() {
                        "input_alias, input_revision or input_b64"
                    } else {
                        "source_alias or source_revision"
                    }
                ),
            });
        }

        let style = if kind.is_text() { StageStyle::Vision } else { StageStyle::Fleet };
        let answer = match self.dispatch_and_wait(claimed, &request, style, stop)? {
            FleetRun::Done(answer) => answer,
            FleetRun::Outcome(outcome) => return Ok(outcome),
        };

        // A text answer publishes nothing: it is written onto the job.
        if kind.is_text() {
            let Some(text) = answer.text.as_deref().map(str::trim).filter(|t| !t.is_empty())
            else {
                return Ok(JobOutcome::Failed {
                    error: "fleet finished the vision job without any text".to_string(),
                });
            };
            self.log(&format!(
                "job {}: answered in {:.1}s on {}",
                claimed.job,
                answer.elapsed.as_secs_f64(),
                answer.host.as_deref().unwrap_or("the fleet")
            ));
            return Ok(JobOutcome::Answered {
                text: text.to_string(),
                model: answer.model,
                host: answer.host.unwrap_or_default(),
            });
        }

        // The product is the artifact whose content type the kind declares.
        // A box that returned only sidecars is a failure, never a guess.
        let Some(shape) = kind.catalog() else {
            return Ok(JobOutcome::Failed {
                error: format!("{} has no catalog product", kind.kind),
            });
        };
        let Some(product) = answer
            .artifacts
            .into_iter()
            .find(|a| shape.content_types.iter().any(|ct| a.content_type == *ct))
        else {
            return Ok(JobOutcome::Failed {
                error: format!("fleet returned no {} artifact", shape.content_types[0]),
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
        publish.backend = answer.backend;
        publish.model = answer.model.clone();
        // Rights come from the operator's explicit declaration, never from
        // a silent in-code default.
        publish.rights = self.rights.clone();
        // Typed provenance only from REAL knowledge: the seed is recorded
        // only when the job body pinned one (the box invents nondeterministic
        // seeds it never reports back), and the required generator version
        // is the actual selected service's advertised version.
        if let Some(seed) = request.seed.filter(|_| !answer.version.is_empty()) {
            publish.manifest_provenance = Some(PublishProvenance {
                generator: "makepad-asset-ai".to_string(),
                model: answer.model,
                version: answer.version,
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

    /// `annotate.asset`: the catalog's own description pass, run as a fleet
    /// job.
    ///
    /// The body is the store's (`{asset, alias, kind, version_tag}`), minted
    /// on every publish. Everything else — which prompt, how the sheet is
    /// framed, what a reply means, which fields may be overwritten — comes
    /// from `makepad-asset-annotate`, so an answer written here is the same
    /// answer the pass has always written.
    fn process_annotation(
        &mut self,
        kind: &'static GenKind,
        claimed: &ClaimedJobDto,
        stop: &AtomicBool,
    ) -> Result<JobOutcome, ClientError> {
        let body = &claimed.body;
        let Some(asset) = body
            .get("asset")
            .and_then(Value::as_str)
            .and_then(|t| AssetId::from_str(t.trim()).ok())
        else {
            return Ok(JobOutcome::Failed { error: "job body has no asset id".to_string() });
        };
        let Some(alias) = body
            .get("alias")
            .and_then(Value::as_str)
            .and_then(|a| AssetAlias::from_str(a.trim()).ok())
        else {
            return Ok(JobOutcome::Failed {
                error: "job body has no usable alias".to_string(),
            });
        };
        // Characters are asked about as PEOPLE, not as pieces that snap onto
        // a grid — a different prompt and a different sheet framing.
        let person = body.get("kind").and_then(Value::as_str) == Some("character");
        let version_tag = body
            .get("version_tag")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        // What the record says NOW, read before any GPU time: a job whose
        // lease expired while its box was still answering comes back to the
        // queue, and re-describing an asset that is already current would
        // spend a GPU on work somebody already did.
        let current = match self.client.get_annotation(&asset) {
            Ok(current) => current,
            Err(error) => {
                return Ok(JobOutcome::Failed {
                    error: format!("read annotation: {error}"),
                })
            }
        };
        let base = base_annotation(current);
        if !version_tag.is_empty() && base.tags.iter().any(|t| *t == version_tag) {
            self.log(&format!(
                "job {}: {alias} is already described at {version_tag}",
                claimed.job
            ));
            return Ok(JobOutcome::Described {
                asset: asset.to_string(),
                description: base.description,
                model: String::new(),
            });
        }

        // The published turntable sheet, framed the way the pass frames it.
        let sheet = match self.client.thumbnail_alias_bytes(&alias) {
            Ok(bytes) => bytes,
            Err(error) => {
                return Ok(JobOutcome::Failed {
                    error: format!("thumbnail sheet for {alias}: {error}"),
                })
            }
        };
        let image = match sheet_image(&sheet, person) {
            Ok(image) => image,
            Err(error) => return Ok(JobOutcome::Failed { error }),
        };
        let prompt = format!(
            "{}\n\n{}",
            pass::prompt_for(person),
            pass::context_line(alias.as_str(), person)
        );
        self.log(&format!(
            "job {} [{}]: asking about {alias} ({} sheet, {} KB)",
            claimed.job,
            kind.kind,
            if person { "person" } else { "kit" },
            image.len() / 1024
        ));
        let request = GenRequest {
            kind,
            prompt,
            model: body
                .get("model")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim()
                .to_string(),
            seed: None,
            body: obj(vec![("max_tokens", Value::Int(VISION_MAX_TOKENS as i64))]),
            input: Some(GenInput {
                bytes: image,
                content_type: "image/jpeg".to_string(),
            }),
        };
        let answer = match self.dispatch_and_wait(claimed, &request, StageStyle::Vision, stop)? {
            FleetRun::Done(answer) => answer,
            FleetRun::Outcome(outcome) => return Ok(outcome),
        };
        let Some(reply) = answer.text.as_deref().map(str::trim).filter(|t| !t.is_empty()) else {
            return Ok(JobOutcome::Failed {
                error: "fleet finished the vision job without any text".to_string(),
            });
        };
        let record = parse_record(reply);
        if !record.is_useful() {
            // The reason is what an operator reads in the RUNS list, so the
            // unusable reply itself rides along, bounded.
            return Ok(JobOutcome::Failed {
                error: format!("unusable reply: {}", bounded(reply, 300)),
            });
        }
        let annotator = Annotator {
            version: ANNOTATOR_VERSION,
            model: slug(&answer.model),
        };
        // Somebody else already wrote this version while this job ran: their
        // answer is as good as ours, and a second write would only churn.
        if !needs_annotation(&base.tags, &annotator) {
            return Ok(JobOutcome::Described {
                asset: asset.to_string(),
                description: base.description,
                model: annotator.model,
            });
        }
        let upload = plan_upload(&base, &record, &annotator);
        let wire = AnnotationUpload {
            title: upload.title.clone(),
            description: upload.description.clone(),
            kind: upload
                .kind
                .as_deref()
                .and_then(makepad_asset_client::dto::kind_parse),
            categories: upload.categories.clone(),
            tags: upload.tags.clone(),
            creator: upload.creator.clone(),
            generator: upload.generator.clone(),
            backend: upload.backend.clone(),
            model: upload.model.clone(),
            prompt: upload.prompt.clone(),
            provenance: upload.provenance.clone(),
            private: upload.private,
        };
        if let Err(error) = self.client.put_annotation(&asset, &wire) {
            return Ok(JobOutcome::Failed {
                error: format!("put annotation: {error}"),
            });
        }
        self.log(&format!(
            "job {}: {alias} -> {} ({:.1}s)",
            claimed.job,
            upload.description,
            answer.elapsed.as_secs_f64()
        ));
        Ok(JobOutcome::Described {
            asset: asset.to_string(),
            description: upload.description,
            model: annotator.model,
        })
    }

    /// Dispatch to the fleet and drive one job to an answer.
    ///
    /// Shared by every kind: the Asset Server lease is heartbeaten with real
    /// progress, upstream cancellation is propagated to the box, a box that
    /// is momentarily short of VRAM is waited for rather than failed, and
    /// the whole thing is bounded by [`JOB_DEADLINE`].
    fn dispatch_and_wait(
        &mut self,
        claimed: &ClaimedJobDto,
        request: &GenRequest,
        style: StageStyle,
        stop: &AtomicBool,
    ) -> Result<FleetRun, ClientError> {
        let started = Instant::now();
        let mut last_heartbeat = Instant::now();
        let mut last_cancel_check = Instant::now();
        let mut wait_stage: Option<String> = None;
        let mut retry_not_before: Option<Instant> = None;
        'generation: loop {
            // Selection is refreshed while a compatible GPU is temporarily
            // below its advertised admission target. This is also the retry
            // path when the service's authoritative, later admission check
            // beats our health snapshot.
            let (fleet_job, dispatched_model, dispatched_backend, dispatched_version) = loop {
                if stop.load(Ordering::SeqCst) {
                    return Ok(FleetRun::Outcome(JobOutcome::Failed {
                        error: "worker shutdown".to_string(),
                    }));
                }
                if started.elapsed() > JOB_DEADLINE {
                    return Ok(FleetRun::Outcome(JobOutcome::Failed {
                        error: "generation deadline".to_string(),
                    }));
                }
                if last_cancel_check.elapsed() >= CANCEL_CHECK_EVERY {
                    last_cancel_check = Instant::now();
                    if let Ok(status) = self.client.job_status(&claimed.job) {
                        if status.state == JobStateDto::Cancelled {
                            self.log(&format!("job {}: cancelled upstream", claimed.job));
                            return Ok(FleetRun::Outcome(JobOutcome::CancelledUpstream));
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
                        return Ok(FleetRun::Outcome(JobOutcome::Failed {
                            error: "lease lost".to_string(),
                        }));
                    }
                }
                if retry_not_before.is_some_and(|at| Instant::now() < at) {
                    std::thread::sleep(FLEET_POLL_EVERY);
                    continue;
                }
                retry_not_before = None;
                match self.fleet.dispatch(request) {
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
                        return Ok(FleetRun::Outcome(JobOutcome::Failed {
                            error: format!("fleet dispatch: {error}"),
                        }))
                    }
                }
                std::thread::sleep(FLEET_POLL_EVERY);
            };

            let node_tag = self.fleet.route_label(&fleet_job);
            let node_host = self.fleet.route_host(&fleet_job);
            let dispatched_at = Instant::now();
            let mut heartbeat_stage = match style {
                StageStyle::Fleet => match &node_tag {
                    Some(tag) => format!("@{tag} queued-on-fleet"),
                    None => "queued-on-fleet".to_string(),
                },
                StageStyle::Vision => vision_note(&dispatched_model, node_host.as_deref(), 0.0),
            };
            let mut heartbeat_permille = 0;
            // Say WHERE it went the moment it went there. A vision answer
            // takes seconds — less than one heartbeat period — so a job that
            // waited for the normal cadence would finish before anything
            // ever reported which box was on it, and an operator watching a
            // backlog drain would see a queue with nothing visibly running.
            last_heartbeat = Instant::now();
            let _ = self.client.worker_heartbeat(
                &claimed.job,
                LEASE_MS,
                Some(&self.suffix),
                Some((0, &bounded(&heartbeat_stage, 180))),
            );
            loop {
                if stop.load(Ordering::SeqCst) {
                    self.fleet.cancel(&fleet_job);
                    return Ok(FleetRun::Outcome(JobOutcome::Failed {
                        error: "worker shutdown".to_string(),
                    }));
                }
                if started.elapsed() > JOB_DEADLINE {
                    self.fleet.cancel(&fleet_job);
                    return Ok(FleetRun::Outcome(JobOutcome::Failed {
                        error: "generation deadline".to_string(),
                    }));
                }
                // Cancel propagation: the enqueuer cancelled server-side
                // (the Annotate Pause button, the VJ's stop) → stop the box.
                if last_cancel_check.elapsed() >= CANCEL_CHECK_EVERY {
                    last_cancel_check = Instant::now();
                    if let Ok(status) = self.client.job_status(&claimed.job) {
                        if status.state == JobStateDto::Cancelled {
                            self.fleet.cancel(&fleet_job);
                            self.log(&format!("job {}: cancelled upstream", claimed.job));
                            return Ok(FleetRun::Outcome(JobOutcome::CancelledUpstream));
                        }
                    }
                }
                // Renew independently of a successful fleet poll. A box may
                // be restarting or its response may be temporarily lost; the
                // Asset Server lease must not expire merely because the last
                // known stage could not be refreshed.
                if last_heartbeat.elapsed() >= HEARTBEAT_EVERY {
                    last_heartbeat = Instant::now();
                    if style == StageStyle::Vision {
                        heartbeat_stage = vision_note(
                            &dispatched_model,
                            node_host.as_deref(),
                            dispatched_at.elapsed().as_secs_f64(),
                        );
                    }
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
                        return Ok(FleetRun::Outcome(JobOutcome::Failed {
                            error: "lease lost".to_string(),
                        }));
                    }
                }
                match self.fleet.poll(&fleet_job) {
                    Ok(FleetPoll::Done { artifacts, text }) => {
                        return Ok(FleetRun::Done(FleetAnswer {
                            artifacts,
                            text,
                            model: dispatched_model,
                            backend: dispatched_backend,
                            version: dispatched_version,
                            host: node_host,
                            elapsed: dispatched_at.elapsed(),
                        }))
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
                        return Ok(FleetRun::Outcome(JobOutcome::Failed {
                            error: format!("fleet: {error}"),
                        }))
                    }
                    Ok(FleetPoll::Running { stage, progress }) => {
                        heartbeat_permille = (progress.clamp(0.0, 1.0) * 1000.0) as u16;
                        if style == StageStyle::Fleet {
                            heartbeat_stage = match &node_tag {
                                Some(tag) => format!("@{tag} {stage}"),
                                None => stage,
                            };
                        }
                    }
                    Err(error) => {
                        // Transient fleet transport errors: keep polling
                        // within the deadline (the box may be mid-restart).
                        self.log(&format!("job {}: fleet poll error: {error}", claimed.job));
                    }
                }
                std::thread::sleep(FLEET_POLL_EVERY);
            }
        }
    }

    /// Fetch the payload a job pins.
    ///
    /// Two vocabularies, because they mean two different things. A TRANSFORM
    /// names the catalog row its product derives from (`source_alias` /
    /// `source_revision`) and gets that row's own file. A QUESTION names the
    /// thing it is asking about (`input_alias` / `input_revision` /
    /// `input_b64`) and gets a picture — for an alias that is the published
    /// thumbnail sheet, because most of this catalog is meshes and a vision
    /// model cannot look at a GLB.
    ///
    /// Nothing is inferred: a body naming neither resolves to `None`, and
    /// the caller refuses kinds that need one.
    fn resolve_input(
        &mut self,
        kind: &GenKind,
        body: &Value,
    ) -> Result<Option<GenInput>, String> {
        if kind.is_text() {
            return self.resolve_question_input(body);
        }
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
            InputNeed::Video => media == MediaType::Mp4,
            _ => matches!(media, MediaType::Png | MediaType::Jpeg),
        };
        // Prefer the role that IS the payload (a render GLB, a texture) over
        // a retained original; both beat nothing.
        let preferred: &[FileRole] = match kind.input {
            InputNeed::Mesh => &[FileRole::RenderGlb, FileRole::Source],
            InputNeed::Video => &[FileRole::Video, FileRole::Source],
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
            MediaType::Mp4 => "video/mp4",
            _ => "image/png",
        };
        Ok(Some(GenInput { bytes, content_type: content_type.to_string() }))
    }

    /// The image a question is about: inline bytes, an exact revision's
    /// image file, or an alias's published thumbnail sheet.
    fn resolve_question_input(&mut self, body: &Value) -> Result<Option<GenInput>, String> {
        // Inline bytes win: a client that already HAS the picture (a webcam
        // frame, a canvas it just drew) should never be made to publish it
        // first just to ask about it.
        if let Some(b64) = body.get("input_b64").and_then(Value::as_str) {
            let bytes = makepad_base64::base64_decode(b64.trim().as_bytes())
                .map_err(|_| "input_b64 is not base64".to_string())?;
            if bytes.is_empty() {
                return Err("input_b64 is empty".to_string());
            }
            if bytes.len() as u64 > MAX_INPUT_BYTES {
                return Err(format!("input_b64 too large ({} bytes)", bytes.len()));
            }
            let content_type = body
                .get("input_content_type")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|ct| *ct == "image/png" || *ct == "image/jpeg")
                .unwrap_or(if bytes.starts_with(&[0xFF, 0xD8]) {
                    "image/jpeg"
                } else {
                    "image/png"
                })
                .to_string();
            return Ok(Some(GenInput { bytes, content_type }));
        }
        if let Some(revision) = body.get("input_revision").and_then(Value::as_str) {
            let revision = AssetRevisionId::from_str(revision.trim())
                .map_err(|_| "malformed input_revision".to_string())?;
            let manifest = self
                .client
                .fetch_asset_manifest(&revision)
                .map_err(|e| format!("input manifest: {e}"))?;
            let file = manifest
                .files
                .iter()
                .find(|f| matches!(f.media, MediaType::Png | MediaType::Jpeg))
                .ok_or("input revision has no image file")?;
            if file.byte_len > MAX_INPUT_BYTES {
                return Err(format!("input file too large ({} bytes)", file.byte_len));
            }
            let bytes = self
                .client
                .fetch_blob_bytes(&file.blob, Some(file.byte_len))
                .map_err(|e| format!("input blob: {e}"))?;
            let content_type = match file.media {
                MediaType::Jpeg => "image/jpeg",
                _ => "image/png",
            };
            return Ok(Some(GenInput { bytes, content_type: content_type.to_string() }));
        }
        if let Some(alias) = body.get("input_alias").and_then(Value::as_str) {
            let alias = AssetAlias::from_str(alias.trim())
                .map_err(|_| "malformed input_alias".to_string())?;
            let bytes = self
                .client
                .thumbnail_alias_bytes(&alias)
                .map_err(|e| format!("thumbnail sheet for {alias}: {e}"))?;
            let content_type = if bytes.starts_with(&[0xFF, 0xD8]) {
                "image/jpeg"
            } else {
                "image/png"
            };
            return Ok(Some(GenInput { bytes, content_type: content_type.to_string() }));
        }
        Ok(None)
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
    let shape = kind.catalog().ok_or("kind publishes no catalog row")?;
    let bytes = product.bytes;
    let mut stats = PublishStats::default();
    let mut extra_tags: Vec<String> = Vec::new();
    let (media_millis, dims, thumbnail) = match shape.media {
        MediaType::Png | MediaType::Jpeg => {
            let dims = match shape.media {
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
            let pcm = decode_audio(&bytes, shape.media)?;
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
        shape.asset_kind,
        title,
        PublishFile {
            bytes,
            media: shape.media,
            role: shape.role,
            media_millis,
            dims,
        },
        thumbnail,
    );
    request_out.categories = vec![shape.category.to_string()];
    request_out.tags = shape.tags.iter().map(|t| t.to_string()).collect();
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

/// Bound an ANSWER for the job result document. Line breaks survive — a
/// nine-line record and a paragraph are different answers, and the client
/// that asked gets what the model actually wrote — while every other
/// control character is dropped.
fn bounded_answer(text: &str, max: usize) -> String {
    let mut out = String::with_capacity(text.len().min(max));
    for c in text.chars() {
        if c.is_control() && c != '\n' && c != '\t' {
            continue;
        }
        if out.len() + c.len_utf8() > max {
            break;
        }
        out.push(c);
    }
    out
}

/// The vision progress line: which model, on which box, how long so far.
fn vision_note(model: &str, host: Option<&str>, seconds: f64) -> String {
    match host {
        Some(host) => format!("vision · {model} @ {host} · {seconds:.1} s"),
        None => format!("vision · {model} · {seconds:.1} s"),
    }
}

/// Frame a published turntable sheet for the vision tower and encode it for
/// the wire. The framing is the annotation pass's
/// ([`pass::sheet_to_rgb`]); only the container is decided here.
fn sheet_image(sheet_bytes: &[u8], person: bool) -> Result<Vec<u8>, String> {
    // An imported asset publishes the PNG turntable sheet; a generated one
    // publishes a JPEG. Both are pictures OF the asset, so both are framed
    // the same way rather than one of them failing the job.
    let decoded = if sheet_bytes.starts_with(&[0xFF, 0xD8]) {
        let (rgba, w, h) = crate::quake3_import::decode_jpeg(sheet_bytes)?;
        let mut pixels = Vec::with_capacity(w as usize * h as usize * 3);
        for px in rgba.chunks_exact(4) {
            pixels.extend_from_slice(&px[..3]);
        }
        sheet::Rgb { w: w as usize, h: h as usize, pixels }
    } else {
        sheet::decode_png(sheet_bytes)?
    };
    let rgb = pass::frame_sheet(decoded, person, &SheetPrep::default());
    let mut bgra = Vec::with_capacity(rgb.w * rgb.h);
    for px in rgb.pixels.chunks_exact(3) {
        bgra.push(
            0xff00_0000 | (px[0] as u32) << 16 | (px[1] as u32) << 8 | px[2] as u32,
        );
    }
    encode_jpeg_bgra(&bgra, rgb.w, rgb.h)
}

/// The annotation record as the pass reads it.
fn base_annotation(current: makepad_asset_client::dto::AnnotationDto) -> BaseAnnotation {
    BaseAnnotation {
        title: current.title,
        description: current.description,
        kind: current
            .kind
            .map(|k| makepad_asset_client::dto::kind_name(k).to_string()),
        categories: current.categories,
        tags: current.tags,
        creator: current.creator,
        generator: current.generator,
        backend: current.backend,
        model: current.model,
        prompt: current.prompt,
        provenance: current.provenance,
        private: current.private,
    }
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
            (request.kind.catalog().map(|c| c.media) == Some(MediaType::Mp4))
                .then(|| "h264".to_string())
        }),
        // Vision: how much answer to allow. The nine-line record needs
        // ~200; a client asking its own question sets its own budget.
        max_tokens: u32_of("max_tokens"),
        audio: body.get("audio").and_then(Value::as_bool),
        interpolate: u32_of("interpolate"),
        // Enhance (video post-process)
        upscale: u32_of("upscale"),
        flow_map: body.get("flow_map").and_then(Value::as_bool),
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
    fn route_label(&self, fleet_job: &str) -> Option<String> {
        Some(format!(".{}", self.route_host(fleet_job)?.rsplit('.').next()?))
    }

    fn route_host(&self, fleet_job: &str) -> Option<String> {
        let url = self.routes.get(fleet_job)?;
        // "http://10.0.0.203:8123" -> "10.0.0.203"
        Some(url.rsplit('/').next()?.split(':').next()?.to_string())
    }

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
            // A text-answering job (vision, llm) reports its completed answer
            // in `text`; `partial_text` is the streamed snapshot, which is
            // the whole answer by the time the job is done. Such a job has
            // no artifacts, and that is not a failure.
            let text = status
                .text
                .clone()
                .or_else(|| status.partial_text.clone())
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty());
            if status.artifacts.is_empty() && text.is_none() {
                return Err("done without artifacts or text".to_string());
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
            return Ok(FleetPoll::Done { artifacts: out, text });
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
        /// Set to answer with TEXT instead of artifacts (the vision kinds).
        text: Option<String>,
    }

    impl GenFleet for ScriptedFleet {
        fn route_host(&self, _fleet_job: &str) -> Option<String> {
            Some("10.0.0.203".to_string())
        }

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
            if let Some(text) = &self.text {
                // A vision box answers with text and no artifacts at all.
                return Ok(FleetPoll::Done {
                    artifacts: Vec::new(),
                    text: Some(text.clone()),
                });
            }
            Ok(FleetPoll::Done {
                text: None,
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


    /// The nine-line record a vision box answers an annotation job with.
    fn kit_reply() -> &'static str {
        "what: wooden cart\ncat: vehicle\nrole: standalone\nconn: none\n\
         size: 1x2\ncolors: brown, grey\nstyle: low-poly\n\
         desc: open cart with one large spoked wheel"
    }

    /// One triangle: the smallest thing the catalog accepts as a mesh, so
    /// the publish that MINTS an annotation job is a real publish.
    fn one_triangle_glb() -> Vec<u8> {
        let mut bin: Vec<u8> = Vec::new();
        for f in [0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0] {
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

    /// Publish one annotatable asset and return (asset, alias). Publishing
    /// is what MAKES the annotation job: the store mints one per newly live
    /// annotatable row, which is the whole point of the queue.
    fn publish_mesh(client: &mut AssetClient, ns: &str, name: &str) -> (AssetId, String) {
        let glb = one_triangle_glb();
        let inspected = crate::glb::inspect_glb(&glb).expect("inspect the fixture");
        let mut publish = PublishRequest::new(
            ns,
            AssetKind::Mesh,
            name.to_string(),
            PublishFile {
                bytes: glb,
                media: MediaType::Glb,
                role: FileRole::RenderGlb,
                media_millis: 0,
                dims: None,
            },
            crate::import::placeholder_thumb().expect("placeholder thumbnail"),
        );
        publish.stats = PublishStats {
            triangles: inspected.triangles,
            vertices: inspected.vertices,
            joints: inspected.joints,
            clips: inspected.clips,
        };
        let alias = format!("{ns}/{name}");
        publish.alias = AssetAlias::from_str(&alias).ok();
        publish.rights = PublishRights::generated_cc0();
        let published = client.publish_artifact(&publish).expect("publish");
        (published.asset_id, alias)
    }

    fn test_server(name: &str) -> (AssetServer, PathBuf, String) {
        let root = test_root(name);
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
        (server, root, token)
    }

    /// A question about an image is answered ONTO THE JOB: no catalog row,
    /// and `GET /v1/jobs/<id>` carries the text back to whoever asked. This
    /// is the runtime path for a UI making content.
    #[test]
    fn a_vision_question_records_its_answer_on_the_job() {
        let (server, root, token) = test_server("vision_describe");
        let mut submitter = connect(&server, &token, &root.join("submit-cache"));
        let png_b64 = String::from_utf8(makepad_base64::base64_encode(
            &tiny_png(),
            &makepad_base64::BASE64_STANDARD,
        ))
        .unwrap();
        let body = obj(vec![
            ("prompt", s("is this door left- or right-hinged?")),
            ("input_b64", s(png_b64)),
            ("max_tokens", Value::Int(64)),
        ]);
        let job = submitter
            .enqueue_job("gen", "vision.describe", &body)
            .expect("enqueue");

        let mut fleet = ScriptedFleet {
            text: Some("  right-hinged: the handle is on the left edge.  ".to_string()),
            ..Default::default()
        };
        let outcome = {
            let mut coordinator = Coordinator {
                client: connect(&server, &token, &root.join("worker-cache")),
                fleet: &mut fleet,
                suffix: "vision-box".to_string(),
                kinds: vec!["vision.describe".to_string()],
                rights: PublishRights::generated_cc0(),
                log: false,
            };
            coordinator
                .run_one(&AtomicBool::new(false))
                .expect("coordinator call")
                .expect("claimed the vision job")
        };
        // The image reached the box as bytes, not as a promise.
        assert_eq!(fleet.requests.len(), 1);
        assert_eq!(
            fleet.requests[0].input.as_ref().map(|i| i.bytes.clone()),
            Some(tiny_png())
        );
        assert_eq!(
            fleet.requests[0].input.as_ref().map(|i| i.content_type.clone()),
            Some("image/png".to_string())
        );
        match &outcome {
            JobOutcome::Answered { text, host, .. } => {
                assert_eq!(text, "right-hinged: the handle is on the left edge.");
                assert_eq!(host, "10.0.0.203");
            }
            other => panic!("expected an answer, got {other:?}"),
        }

        // What the asking client sees.
        let detail = submitter.job_detail(&job).expect("job detail");
        assert_eq!(detail.status.state, JobStateDto::Succeeded);
        // No catalog row: an answer is not an asset.
        assert_eq!(detail.status.result_asset, None);
        let result = detail.result.expect("recorded result");
        assert_eq!(
            result.body.get("text").and_then(Value::as_str),
            Some("right-hinged: the handle is on the left edge.")
        );
        assert_eq!(
            result.body.get("box").and_then(Value::as_str),
            Some("10.0.0.203")
        );
        assert!(result
            .body
            .get("model")
            .and_then(Value::as_str)
            .is_some_and(|m| !m.is_empty()));

        drop(submitter);
        drop(server);
        let _ = std::fs::remove_dir_all(root);
    }

    /// The whole annotation chain, without a GPU: publishing mints the job,
    /// a vision-capable worker claims it, the reply is parsed by the pass,
    /// and the asset's own annotation record carries the description and the
    /// facets a level builder searches on.
    #[test]
    fn an_annotate_job_writes_the_parsed_record_into_the_annotation() {
        let (server, root, token) = test_server("annotate_job");
        let mut submitter = connect(&server, &token, &root.join("submit-cache"));
        let (asset, alias) = publish_mesh(&mut submitter, "kenney", "cart");

        let mut fleet = ScriptedFleet {
            text: Some(kit_reply().to_string()),
            ..Default::default()
        };
        let outcome = {
            let mut coordinator = Coordinator {
                client: connect(&server, &token, &root.join("worker-cache")),
                fleet: &mut fleet,
                suffix: "vision-box".to_string(),
                kinds: vec!["annotate.asset".to_string()],
                rights: PublishRights::generated_cc0(),
                log: false,
            };
            coordinator
                .run_one(&AtomicBool::new(false))
                .expect("coordinator call")
                .expect("the publish queued an annotate job to claim")
        };
        // One vision request, carrying the framed sheet as a real image and
        // both halves of the question.
        assert_eq!(fleet.requests.len(), 1);
        let asked = &fleet.requests[0];
        assert!(asked.prompt.contains("metadata writer"), "{}", asked.prompt);
        assert!(asked.prompt.contains("\"cart\""), "{}", asked.prompt);
        let image = asked.input.as_ref().expect("a sheet was sent");
        assert_eq!(image.content_type, "image/jpeg");
        assert!(image.bytes.starts_with(&[0xFF, 0xD8]), "framed sheet is a jpeg");

        match &outcome {
            JobOutcome::Described { description, model, .. } => {
                assert!(description.starts_with("wooden cart"), "{description}");
                assert!(!model.is_empty());
            }
            other => panic!("expected a description, got {other:?}"),
        }

        // The catalog now answers for it.
        let annotation = submitter.get_annotation(&asset).expect("annotation");
        assert_eq!(
            annotation.description,
            "wooden cart; standalone; 1x2; brown/grey; \
             open cart with one large spoked wheel"
        );
        for tag in ["vlm-v7", "vlm-cat-vehicle", "vlm-col-brown", "vlm-sty-low-poly"] {
            assert!(
                annotation.tags.iter().any(|t| t == tag),
                "missing {tag}: {:?}",
                annotation.tags
            );
        }
        // Nothing was published: an annotation is a rewrite, not a row.
        assert!(annotation.title.contains("cart"));

        // And the same asset is not described twice: the second claim finds
        // an empty queue, because the store's derived job id is the dedupe.
        let mut idle = ScriptedFleet {
            text: Some(kit_reply().to_string()),
            ..Default::default()
        };
        let mut coordinator = Coordinator {
            client: connect(&server, &token, &root.join("worker-2")),
            fleet: &mut idle,
            suffix: "vision-box-2".to_string(),
            kinds: vec!["annotate.asset".to_string()],
            rights: PublishRights::generated_cc0(),
            log: false,
        };
        assert!(coordinator.run_one(&AtomicBool::new(false)).unwrap().is_none());
        drop(coordinator);
        assert!(idle.requests.is_empty());
        let _ = alias;

        drop(submitter);
        drop(server);
        let _ = std::fs::remove_dir_all(root);
    }

    /// A model that answered with prose instead of the record must not
    /// overwrite a description with junk: the job fails, carrying the reply
    /// an operator has to read to fix the prompt.
    #[test]
    fn an_unusable_reply_fails_the_annotate_job_with_its_reason() {
        let (server, root, token) = test_server("annotate_unusable");
        let mut submitter = connect(&server, &token, &root.join("submit-cache"));
        let (asset, _) = publish_mesh(&mut submitter, "kenney", "barrel");
        let before = submitter.get_annotation(&asset).expect("annotation");

        let mut fleet = ScriptedFleet {
            text: Some("I'm sorry, I can't see the image well enough.".to_string()),
            ..Default::default()
        };
        let outcome = {
            let mut coordinator = Coordinator {
                client: connect(&server, &token, &root.join("worker-cache")),
                fleet: &mut fleet,
                suffix: "vision-box".to_string(),
                kinds: vec!["annotate.asset".to_string()],
                rights: PublishRights::generated_cc0(),
                log: false,
            };
            coordinator
                .run_one(&AtomicBool::new(false))
                .expect("coordinator call")
                .expect("claimed the annotate job")
        };
        let JobOutcome::Failed { error } = outcome else {
            panic!("expected a failure, got {outcome:?}")
        };
        assert!(error.starts_with("unusable reply:"), "{error}");
        assert!(error.contains("I'm sorry"), "{error}");
        // The record is untouched — a bad answer costs a retry, never a
        // description.
        let after = submitter.get_annotation(&asset).expect("annotation");
        assert_eq!(after.description, before.description);
        assert!(!after.tags.iter().any(|t| t.starts_with("vlm-v")));

        drop(submitter);
        drop(server);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn the_vision_progress_line_names_the_model_the_box_and_the_time() {
        assert_eq!(
            vision_note("qwen3.8-27b-vision", Some("10.0.0.203"), 3.42),
            "vision · qwen3.8-27b-vision @ 10.0.0.203 · 3.4 s"
        );
        // A transport that cannot name the box still says what is thinking.
        assert_eq!(
            vision_note("qwen3.8-27b-vision", None, 0.0),
            "vision · qwen3.8-27b-vision · 0.0 s"
        );
        // An answer's line breaks are part of the answer and survive the
        // bound; other control characters do not.
        assert_eq!(bounded_answer("a\nb\u{7}c", 100), "a\nb c".replace(' ', ""));
        assert_eq!(bounded_answer("aéébb", 4), "aé");
        // A question needs a question, even though it also needs an image.
        let describe = kind_of("vision.describe").unwrap();
        assert!(GenRequest::from_body(describe, &obj(vec![("input_alias", s("a/b"))])).is_err());
        assert!(GenRequest::from_body(
            describe,
            &obj(vec![("prompt", s("what is this?")), ("input_alias", s("a/b"))])
        )
        .is_ok());
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
