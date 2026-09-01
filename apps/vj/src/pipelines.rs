//! The DREAM run's transport: the run itself, executed here (aicore §9).
//!
//! What used to live here was a client of the STORE's pipeline scheduler:
//! declare a graph, poll its record, and let the fleet coordinator advance
//! it. The store stores now — so this file became the thing it used to
//! watch. A Create spawns the run on its own thread: the creator engine
//! walks the declared stages against the GPU fleet directly (LAN discovery →
//! ETA-ranked node pick per stage), splices each stage's output into the
//! next (expand's text into the prompts, the still into the clip's first
//! AND last frame), and every catalog-bound stage is PUBLISHED from here
//! through the same product builder and dressing the worker used — same
//! thumbnails, same annotations, same provenance strings — so the clip
//! lands on the grid through the exact catalog-event flow it always did.
//!
//! The interface is unchanged (PipeReq/PipeDone, one worker, drained each
//! tick), and the record is still derived, never stored: Detail reads the
//! live run registry and synthesizes the same DTOs, constructing only
//! fields that exist (gen.rs's literal-construction law).
//!
//! THE TRADED PROPERTY, deliberately (user-ratified, aicore §9/§14): a run
//! now lives in the creating app. Quit vj mid-run and the run stops — a run
//! that must outlive the window is a client that does not close
//! (`makepad-creator-run`), not a scheduler in the database.

use makepad_asset_client::json::{obj, s, Value};
use makepad_asset_client::{
    AssetClient, ClientConfig, ApiEndpoints, JobId, JobProfileDto, JobProgressDto, JobResultDto,
    JobStatusDto,
    JobStateDto, PipelineCancelDto, PipelineCreatedDto, PipelineDetailDto, PipelineId,
    PipelineStageDto, PipelineStageJobDto, PipelineStateDto, PublishRights, StageOnFailDto,
};
use makepad_asset_data::AssetAlias;
use makepad_asset_creator::engine::{
    self, EngineConfig, RunEvent, Splice, StageOrder,
};
use makepad_asset_creator::runner::{
    fleet_snapshots, FleetPick, PublishTarget,
};
use makepad_asset_creator::pipeline::{
    derive_progress, derive_state, PipelineSpec, RunState, StageSpec, StageState,
};
use makepad_asset_importer::gen_publish::{
    dress_generated_publish, wire_request, GenArtifact, GenRequest,
};
use makepad_asset_importer::gen_kinds::kind_of;
use makepad_asset_importer::gen_profiles::build_profiles;
use makepad_asset_client::PipelineStageSpec;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// The declaration exactly as it goes on the wire, for the log. (The wire
/// is now this process's own run thread; the log line survives because a
/// run must stay inspectable from the instant it is spawned.)
pub fn declaration_json(
    namespace: &str,
    title: &str,
    prompt: &str,
    stages: &[PipelineStageSpec],
) -> String {
    let declared: Vec<Value> = stages
        .iter()
        .map(|stage| match stage.to_value() {
            Ok(value) => value,
            Err(error) => obj(vec![("refused", s(error.to_string()))]),
        })
        .collect();
    obj(vec![
        ("namespace", s(namespace)),
        ("title", s(title)),
        ("prompt", s(prompt)),
        ("stages", Value::Arr(declared)),
    ])
    .to_json()
}

/// Requests allowed to queue on the worker before DETAIL polls start being
/// dropped — unchanged; a dropped poll costs nothing.
const MAX_QUEUED: usize = 12;

/// One transport request.
pub enum PipeReq {
    /// Declare the whole run. `tag` is the drawer row that asked.
    Create {
        tag: u64,
        namespace: String,
        title: String,
        prompt: String,
        stages: Vec<PipelineStageSpec>,
    },
    /// One read of the record — everything a row draws, in one request.
    Detail { pipeline: PipelineId },
    /// Stop every non-terminal stage of the run.
    Cancel { pipeline: PipelineId },
    /// One plain generation, executed here the same way a stage is.
    EnqueueJob { tag: u64, namespace: String, kind: String, body: Value },
    /// One read of a local job's synthesized status.
    JobStatus { job: JobId },
    /// Cancel one local job.
    CancelJob { job: JobId },
    /// The generation drawer's rows, built from the live fleet directly
    /// (the store's profile registry retired with its queue).
    Profiles { domain: String },
}

/// One transport answer. Errors are strings because the row shows them.
pub enum PipeDone {
    Created { tag: u64, result: Result<PipelineCreatedDto, String> },
    Detail { pipeline: PipelineId, result: Result<PipelineDetailDto, String> },
    Cancelled { pipeline: PipelineId, result: Result<PipelineCancelDto, String> },
    JobQueued { tag: u64, result: Result<JobId, String> },
    JobStatus { job: JobId, result: Result<JobStatusDto, String> },
    JobCancelled { job: JobId, cancelled: u64 },
    Profiles { domain: String, result: Result<Vec<JobProfileDto>, String> },
}

/// Owns the worker thread and the completion channel; the host pumps it
/// each tick.
pub struct Pipelines {
    tx: Option<Sender<PipeReq>>,
    done_tx: Sender<PipeDone>,
    done_rx: Receiver<PipeDone>,
    queued: usize,
}

impl Default for Pipelines {
    fn default() -> Self {
        let (done_tx, done_rx) = channel();
        Pipelines { tx: None, done_tx, done_rx, queued: 0 }
    }
}

impl Pipelines {
    /// (Re)point the transport at a verified session. The endpoints/token
    /// are for PUBLISHING results into the same store as ever; the runs
    /// themselves execute against the fleet directly. Live runs survive a
    /// reconnect: the registry is shared, the old worker just stops taking
    /// requests.
    pub fn connect(&mut self, endpoints: ApiEndpoints, token: Option<String>) {
        let (tx, rx) = channel::<PipeReq>();
        let done = self.done_tx.clone();
        let spawned = std::thread::Builder::new()
            .name("vj-pipelines".to_string())
            .spawn(move || worker(endpoints, token, rx, done));
        self.tx = spawned.is_ok().then_some(tx);
        self.queued = 0;
    }

    pub fn connected(&self) -> bool {
        self.tx.is_some()
    }

    /// Queue one request. Returns false when it could not be handed over.
    pub fn submit(&mut self, req: PipeReq) -> bool {
        let droppable = matches!(req, PipeReq::Detail { .. });
        if droppable && self.queued >= MAX_QUEUED {
            return false;
        }
        let Some(tx) = self.tx.as_ref() else { return false };
        if tx.send(req).is_err() {
            self.tx = None;
            return false;
        }
        self.queued += 1;
        true
    }

    /// Everything that answered since the last call.
    pub fn drain(&mut self) -> Vec<PipeDone> {
        let mut out = Vec::new();
        while let Ok(done) = self.done_rx.try_recv() {
            self.queued = self.queued.saturating_sub(1);
            out.push(done);
        }
        out
    }
}

// ------------------------------------------------------------ the registry

/// One stage's live view, updated by the run thread, read by Detail.
struct StageView {
    name: String,
    kind: String,
    job: JobId,
    state: StageState,
    on_fail_skip: bool,
    /// The body as declared (typed prompt visible pre-dispatch).
    declared: Value,
    note: String,
    permille: u16,
    /// Publication outcome, once a catalog-bound stage lands.
    published: Option<(String, String)>,
    error: Option<String>,
    weight: u16,
}

struct RunRecord {
    namespace: String,
    title: String,
    prompt: String,
    created_ms: u64,
    finished_ms: Option<u64>,
    stages: Vec<StageView>,
}

struct RunHandle {
    record: Mutex<RunRecord>,
    cancel: Arc<AtomicBool>,
}

type Registry = Arc<Mutex<HashMap<[u8; 16], Arc<RunHandle>>>>;

/// One plain generation's live view.
struct JobView {
    namespace: String,
    kind: String,
    created_ms: u64,
    state: StageState,
    note: String,
    permille: u16,
    outcome: Option<String>,
    published: Option<(String, String)>,
}

struct JobHandle {
    view: Mutex<JobView>,
    cancel: Arc<AtomicBool>,
}

type JobRegistry = Arc<Mutex<HashMap<[u8; 16], Arc<JobHandle>>>>;

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 16 identity bytes from time + tag + a counter. Ids are local to this
/// process's registry; they only need to not collide with each other.
fn mint_id(tag: u64) -> [u8; 16] {
    use std::sync::atomic::AtomicU64;
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&(now_ms() ^ tag.rotate_left(17)).to_be_bytes());
    out[8..].copy_from_slice(&count.wrapping_mul(0x9E37_79B9_7F4A_7C15).to_be_bytes());
    out
}

// ------------------------------------------------------------- the worker

fn worker(
    endpoints: ApiEndpoints,
    token: Option<String>,
    rx: Receiver<PipeReq>,
    done: Sender<PipeDone>,
) {
    // The registry outlives any one worker: a reconnect must keep answering
    // for runs the previous worker spawned.
    static REGISTRY: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    let registry = REGISTRY
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone();
    static JOBS: std::sync::OnceLock<JobRegistry> = std::sync::OnceLock::new();
    let jobs = JOBS
        .get_or_init(|| Arc::new(Mutex::new(HashMap::new())))
        .clone();
    for req in rx {
        let answer = match req {
            PipeReq::Create { tag, namespace, title, prompt, stages } => {
                let result = spawn_run(
                    &registry,
                    &endpoints,
                    token.clone(),
                    tag,
                    namespace,
                    title,
                    prompt,
                    stages,
                );
                PipeDone::Created { tag, result }
            }
            PipeReq::Detail { pipeline } => {
                let result = registry
                    .lock()
                    .unwrap()
                    .get(&pipeline.0)
                    .map(|run| synthesize_detail(pipeline, run))
                    .ok_or_else(|| "no such run in this session".to_string());
                PipeDone::Detail { pipeline, result }
            }
            PipeReq::EnqueueJob { tag, namespace, kind, body } => {
                let result = spawn_job(&jobs, &endpoints, token.clone(), tag, namespace, kind, body);
                PipeDone::JobQueued { tag, result }
            }
            PipeReq::JobStatus { job } => {
                let result = jobs
                    .lock()
                    .unwrap()
                    .get(&job.0)
                    .map(|handle| synthesize_job_status(job, handle))
                    .ok_or_else(|| "no such job in this session".to_string());
                PipeDone::JobStatus { job, result }
            }
            PipeReq::CancelJob { job } => {
                let cancelled = jobs
                    .lock()
                    .unwrap()
                    .get(&job.0)
                    .map(|handle| {
                        handle.cancel.store(true, Ordering::Relaxed);
                        let view = handle.view.lock().unwrap();
                        matches!(view.state, StageState::Pending | StageState::Running) as u64
                    })
                    .unwrap_or(0);
                PipeDone::JobCancelled { job, cancelled }
            }
            PipeReq::Profiles { domain } => {
                let snapshots = fleet_snapshots();
                let result = if snapshots.is_empty() {
                    Err("no GPU nodes on the LAN".to_string())
                } else {
                    let mut profiles = build_profiles(&snapshots, "gen");
                    profiles.retain(|p| p.domain == domain);
                    Ok(profiles)
                };
                PipeDone::Profiles { domain, result }
            }
            PipeReq::Cancel { pipeline } => {
                let result = registry
                    .lock()
                    .unwrap()
                    .get(&pipeline.0)
                    .map(|run| {
                        run.cancel.store(true, Ordering::Relaxed);
                        let record = run.record.lock().unwrap();
                        let cancelled = record
                            .stages
                            .iter()
                            .filter(|s| {
                                matches!(s.state, StageState::Pending | StageState::Running)
                            })
                            .count() as u64;
                        PipelineCancelDto {
                            pipeline,
                            cancelled,
                            state: run_state_dto(&record),
                        }
                    })
                    .ok_or_else(|| "no such run in this session".to_string());
                PipeDone::Cancelled { pipeline, result }
            }
        };
        if done.send(answer).is_err() {
            return;
        }
    }
}

// ------------------------------------------------------- declare/translate

/// The DREAM stage kinds this transport executes. A kind outside the table
/// refuses at declare time — honestly, not after GPU spend.
fn stage_domain(kind: &str) -> Option<&'static str> {
    match kind {
        "text.expand" => Some("text"),
        "image.generate" => Some("image"),
        "video.generate" => Some("video"),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn_run(
    registry: &Registry,
    endpoints: &ApiEndpoints,
    token: Option<String>,
    tag: u64,
    namespace: String,
    title: String,
    prompt: String,
    stages: Vec<PipelineStageSpec>,
) -> Result<PipelineCreatedDto, String> {
    // Translate every declared stage before anything runs: a refusal here
    // is the whole declaration refusing, exactly like the server's 400.
    let mut spec_stages = Vec::new();
    let mut orders = Vec::new();
    let mut views = Vec::new();
    let mut created_jobs = Vec::new();
    for (seq, stage) in stages.iter().enumerate() {
        let kind_name = stage.kind.clone();
        let domain = stage_domain(&kind_name)
            .ok_or_else(|| format!("stage {}: kind {kind_name} runs only on the store queue", stage.name))?;
        let kind = kind_of(&kind_name)
            .ok_or_else(|| format!("stage {}: unknown kind {kind_name}", stage.name))?;

        // Splice refs become engine splices; the body they leave behind is
        // plain values the job-body parser accepts.
        let mut body = stage.body.clone();
        let mut splices = Vec::new();
        strip_dream_splices(&mut body, &prompt, &mut splices);

        let request = GenRequest::from_body(kind, &body)
            .map_err(|e| format!("stage {}: {e}", stage.name))?;
        let mut wire = wire_request(&request, request.model.clone());
        // Deterministic where the person didn't pin: the run's tag is the
        // pinned entropy (aicore §9 — seeds are part of the spec).
        if wire.seed.is_none() {
            wire.seed = Some(tag ^ seq as u64);
        }

        let on_fail_skip = stage.on_fail == StageOnFailDto::Skip;
        // The wire's own default: absent deps wait for the stage declared
        // immediately before; an explicit empty list waits for nothing.
        let deps: Vec<String> = match &stage.deps {
            Some(deps) => deps.clone(),
            None => stages
                .get(seq.wrapping_sub(1))
                .filter(|_| seq > 0)
                .map(|prev| vec![prev.name.clone()])
                .unwrap_or_default(),
        };
        let weight = stage.weight.unwrap_or(10).max(1);
        spec_stages.push(StageSpec {
            key: stage.name.clone(),
            domain: domain.to_string(),
            deps,
            weight: weight as u64,
            seed: wire.seed.unwrap_or(0),
            on_fail_skip,
        });
        orders.push(StageOrder {
            spec: spec_stages.last().unwrap().clone(),
            request: wire,
            splices,
        });
        let job = JobId(mint_id(tag ^ (seq as u64) << 8));
        created_jobs.push(PipelineStageJobDto { name: stage.name.clone(), job });
        views.push(StageView {
            name: stage.name.clone(),
            kind: kind_name,
            job,
            state: StageState::Pending,
            on_fail_skip,
            declared: body,
            note: String::new(),
            permille: 0,
            published: None,
            error: None,
            weight,
        });
    }
    let spec = PipelineSpec { name: title.clone(), stages: spec_stages };
    makepad_asset_creator::pipeline::validate(&spec)?;

    let pipeline = PipelineId(mint_id(tag));
    let handle = Arc::new(RunHandle {
        record: Mutex::new(RunRecord {
            namespace: namespace.clone(),
            title,
            prompt: prompt.clone(),
            created_ms: now_ms(),
            finished_ms: None,
            stages: views,
        }),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    registry.lock().unwrap().insert(pipeline.0, handle.clone());

    let endpoints = endpoints.clone();
    let spawn = std::thread::Builder::new()
        .name(format!("vj-dream-{tag}"))
        .spawn(move || run_thread(handle, spec, orders, endpoints, token, namespace, prompt));
    if spawn.is_err() {
        return Err("could not spawn the run thread".to_string());
    }
    Ok(PipelineCreatedDto { pipeline, stages: created_jobs })
}

/// Replace the DREAM's `$from_stage` references with engine splices and
/// plain fallback values. The three shapes are exactly the ones
/// `gen::dream_stages` declares.
fn strip_dream_splices(body: &mut Value, typed_prompt: &str, splices: &mut Vec<Splice>) {
    let Value::Obj(fields) = body else { return };
    let mut remove = Vec::new();
    for (index, (key, value)) in fields.iter_mut().enumerate() {
        let is_ref = matches!(value, Value::Obj(inner) if inner.iter().any(|(k, _)| k == "$from_stage" || k == "$from"));
        if !is_ref {
            continue;
        }
        let from_stage = ref_stage(value);
        match key.as_str() {
            // The prompt splice: engine pastes the dependency's text; the
            // typed prompt stays as the honest fallback (on_fail: skip law).
            "prompt" => {
                if let Some(dep) = from_stage {
                    splices.push(Splice::PromptFromText(dep));
                }
                *value = Value::Str(typed_prompt.to_string());
            }
            // The still, by bytes instead of by revision: first frame…
            "source_revision" => {
                if let Some(dep) = from_stage {
                    splices.push(Splice::InputImageFrom(dep));
                }
                remove.push(index);
            }
            _ => remove.push(index),
        }
    }
    // …and the named `last_frame` input, also by bytes.
    if let Some(pos) = fields.iter().position(|(k, _)| k == "inputs") {
        if let Value::Arr(inputs) = &fields[pos].1 {
            for input in inputs {
                let name = input.get("name").and_then(Value::as_str).unwrap_or("");
                let content_type = input
                    .get("content_type")
                    .and_then(Value::as_str)
                    .unwrap_or("image/png");
                if let Some(dep) = input.get("source_revision").and_then(|v| {
                    let mut v = v.clone();
                    ref_stage(&mut v)
                }) {
                    splices.push(Splice::NamedInputFrom {
                        dep,
                        name: name.to_string(),
                        content_type: content_type.to_string(),
                    });
                }
            }
        }
        remove.push(pos);
    }
    remove.sort_unstable();
    for index in remove.into_iter().rev() {
        fields.remove(index);
    }
}

/// The stage a `$from_stage`/`$from` reference names.
fn ref_stage(value: &mut Value) -> Option<String> {
    let Value::Obj(inner) = value else { return None };
    inner
        .iter()
        .find(|(k, _)| k == "$from_stage" || k == "$from")
        .and_then(|(_, v)| v.as_str())
        .map(|s| s.to_string())
}

// ------------------------------------------------------------- the run

#[allow(clippy::too_many_arguments)]
fn run_thread(
    handle: Arc<RunHandle>,
    spec: PipelineSpec,
    orders: Vec<StageOrder>,
    endpoints: ApiEndpoints,
    token: Option<String>,
    namespace: String,
    typed_prompt: String,
) {
    let (events_tx, events_rx) = channel();
    let cancel = handle.cancel.clone();
    let engine_spec = spec.clone();
    let engine = std::thread::Builder::new()
        .name("vj-dream-engine".to_string())
        .spawn(move || {
            engine::run(
                &engine_spec,
                &orders,
                &FleetPick,
                &EngineConfig::default(),
                &events_tx,
                &cancel,
            )
        });
    let Ok(engine) = engine else {
        let mut record = handle.record.lock().unwrap();
        for stage in &mut record.stages {
            stage.state = StageState::Failed;
            stage.error = Some("could not spawn the engine".to_string());
        }
        record.finished_ms = Some(now_ms());
        return;
    };

    // Publisher client: same store, same identity, same dressing as the
    // worker had. Built lazily so a run with no catalog stages never dials.
    let mut client: Option<Result<AssetClient, String>> = None;
    let mut expanded: Option<String> = None;

    for event in events_rx {
        match event {
            RunEvent::StageStarted { key, .. } => {
                let mut record = handle.record.lock().unwrap();
                if let Some(stage) = record.stages.iter_mut().find(|s| s.name == key) {
                    stage.state = StageState::Running;
                    stage.note = "queued-on-fleet".to_string();
                }
            }
            RunEvent::StageProgress { key, stage: phase, progress } => {
                let mut record = handle.record.lock().unwrap();
                if let Some(stage) = record.stages.iter_mut().find(|s| s.name == key) {
                    stage.note = phase.unwrap_or_default();
                    stage.permille = (progress.unwrap_or(0.0).clamp(0.0, 1.0) * 1000.0) as u16;
                }
            }
            RunEvent::StageDone { key, output } => {
                // The expander's text feeds the published rows' provenance.
                if let Some(text) = &output.text {
                    if expanded.is_none() {
                        expanded = Some(text.clone());
                    }
                }
                let published = publish_stage(
                    &mut client,
                    &endpoints,
                    &token,
                    &namespace,
                    &handle,
                    &key,
                    &typed_prompt,
                    expanded.as_deref(),
                    &output,
                );
                let mut record = handle.record.lock().unwrap();
                if let Some(stage) = record.stages.iter_mut().find(|s| s.name == key) {
                    match published {
                        Ok(done) => {
                            stage.state = StageState::Done;
                            stage.permille = 1000;
                            stage.note = "done".to_string();
                            stage.published = done;
                        }
                        Err(error) => {
                            // The GPU spend happened but the row did not: the
                            // stage fails honestly with the store's reason.
                            stage.state = StageState::Failed;
                            stage.error = Some(error);
                        }
                    }
                }
            }
            RunEvent::StageSkipped { key, error } => {
                let mut record = handle.record.lock().unwrap();
                if let Some(stage) = record.stages.iter_mut().find(|s| s.name == key) {
                    stage.state = StageState::Skipped;
                    stage.error = Some(error);
                }
            }
            RunEvent::StageFailed { key, error } => {
                let mut record = handle.record.lock().unwrap();
                if let Some(stage) = record.stages.iter_mut().find(|s| s.name == key) {
                    stage.state = StageState::Failed;
                    stage.error = Some(error);
                }
            }
            RunEvent::RunFinished { .. } => {
                let mut record = handle.record.lock().unwrap();
                // Anything not terminal when the engine stops is cancelled.
                for stage in &mut record.stages {
                    if matches!(stage.state, StageState::Pending | StageState::Running) {
                        stage.state = StageState::Cancelled;
                    }
                }
                record.finished_ms = Some(now_ms());
            }
        }
    }
    let _ = engine.join();
}

/// Publish one catalog-bound stage through the worker's own product builder
/// and dressing. Text stages publish nothing and answer `Ok(None)`.
#[allow(clippy::too_many_arguments)]
fn publish_stage(
    client: &mut Option<Result<AssetClient, String>>,
    endpoints: &ApiEndpoints,
    token: &Option<String>,
    namespace: &str,
    handle: &Arc<RunHandle>,
    key: &str,
    typed_prompt: &str,
    expanded: Option<&str>,
    output: &engine::StageOutput,
) -> Result<Option<(String, String)>, String> {
    let (kind_name, declared, job) = {
        let record = handle.record.lock().unwrap();
        let stage = record
            .stages
            .iter()
            .find(|s| s.name == key)
            .ok_or("unknown stage")?;
        (stage.kind.clone(), stage.declared.clone(), stage.job)
    };
    let kind = kind_of(&kind_name).ok_or("unknown kind")?;
    if kind.catalog().is_none() {
        return Ok(None);
    }
    let artifact = output
        .artifact
        .as_ref()
        .ok_or("the stage finished without an artifact")?;

    let client = client
        .get_or_insert_with(|| {
            let cache = std::env::temp_dir().join("makepad-vj-dream-publish");
            let mut config = ClientConfig::new(cache);
            config.token = token.clone();
            AssetClient::connect(config, endpoints.clone(), None).map_err(|e| e.to_string())
        })
        .as_mut()
        .map_err(|e| e.clone())?;

    // The dressing the worker applied, with the run's own identity: the
    // model prompt is the EXPANDED one when the expander delivered, and the
    // person's words survive as the title + provenance.
    let mut request = GenRequest::from_body(kind, &declared)?;
    if let Some(expanded) = expanded {
        request.original_prompt = Some(request.prompt.clone());
        request.prompt = expanded.to_string();
    }
    let _ = typed_prompt;
    let job_hex = job.to_string();
    let alias_text = format!(
        "{namespace}/run-{}",
        job_hex.trim_start_matches("job_").chars().take(16).collect::<String>()
    );
    let publish = dress_generated_publish(
        kind,
        namespace,
        &request,
        GenArtifact {
            content_type: artifact.content_type.clone(),
            bytes: artifact.bytes.clone(),
        },
        AssetAlias::from_str(&alias_text).ok(),
        String::new(),
        String::new(),
        String::new(),
        PublishRights::generated_cc0(),
    )?;
    let published = client
        .publish_artifact(&publish)
        .map_err(|e| e.to_string())?;
    Ok(Some((published.asset_id.to_string(), published.revision.to_string())))
}


// ---------------------------------------------------------- plain jobs

/// One plain generation: the single-stage version of a run — pick a node,
/// submit, poll, publish, all on its own thread; the registry serves status.
fn spawn_job(
    jobs: &JobRegistry,
    endpoints: &ApiEndpoints,
    token: Option<String>,
    tag: u64,
    namespace: String,
    kind_name: String,
    body: Value,
) -> Result<JobId, String> {
    // Validate at enqueue so a typo refuses before any thread spawns.
    let _ = makepad_asset_creator::runner::translate(&kind_name, &body, tag)?;
    let job = JobId(mint_id(tag ^ 0x00B5));
    let handle = Arc::new(JobHandle {
        view: Mutex::new(JobView {
            namespace: namespace.clone(),
            kind: kind_name.clone(),
            created_ms: now_ms(),
            state: StageState::Pending,
            note: "queued-on-fleet".to_string(),
            permille: 0,
            outcome: None,
            published: None,
        }),
        cancel: Arc::new(AtomicBool::new(false)),
    });
    jobs.lock().unwrap().insert(job.0, handle.clone());
    let endpoints = endpoints.clone();
    std::thread::Builder::new()
        .name(format!("vj-gen-{tag}"))
        .spawn(move || job_thread(handle, endpoints, token, namespace, kind_name, body, tag))
        .map_err(|_| "could not spawn the job thread".to_string())?;
    Ok(job)
}

#[allow(clippy::too_many_arguments)]
fn job_thread(
    handle: Arc<JobHandle>,
    endpoints: ApiEndpoints,
    token: Option<String>,
    namespace: String,
    kind_name: String,
    body: Value,
    seed: u64,
) {
    let target = PublishTarget { endpoints, token, namespace };
    let cancel = handle.cancel.clone();
    let progress_handle = handle.clone();
    let mut progress = |note: &str, permille: u16| {
        let mut view = progress_handle.view.lock().unwrap();
        if view.state == StageState::Pending {
            view.state = StageState::Running;
        }
        view.note = note.to_string();
        view.permille = permille;
    };
    match makepad_asset_creator::runner::generate_and_publish(
        &kind_name,
        &body,
        seed,
        &target,
        &cancel,
        &mut progress,
    ) {
        Ok(generated) => {
            let mut view = handle.view.lock().unwrap();
            view.state = StageState::Done;
            view.permille = 1000;
            view.note = "done".to_string();
            view.outcome = Some("succeeded".to_string());
            view.published = generated.asset_id.zip(generated.revision);
        }
        Err(error) => {
            let mut view = handle.view.lock().unwrap();
            if error == "cancelled" {
                view.state = StageState::Cancelled;
                view.outcome = Some("cancelled".to_string());
            } else {
                view.state = StageState::Failed;
                view.outcome = Some("failed".to_string());
                view.note = error;
            }
        }
    }
}

/// The registry key a handle was inserted under is not stored on it; jobs
/// only need a stable per-run alias, so hash the Arc identity.
fn handle_key(handle: &Arc<JobHandle>) -> [u8; 16] {
    let addr = Arc::as_ptr(handle) as usize as u64;
    let mut out = [0u8; 16];
    out[..8].copy_from_slice(&addr.to_be_bytes());
    out[8..].copy_from_slice(&addr.wrapping_mul(0x9E37_79B9_7F4A_7C15).to_be_bytes());
    out
}

fn synthesize_job_status(job: JobId, handle: &Arc<JobHandle>) -> JobStatusDto {
    let view = handle.view.lock().unwrap();
    use makepad_asset_data::{AssetId, AssetRevisionId};
    let (result_asset, result_revision) = match &view.published {
        Some((asset, revision)) => (
            AssetId::from_str(asset).ok(),
            AssetRevisionId::from_str(revision).ok(),
        ),
        None => (None, None),
    };
    JobStatusDto {
        job,
        namespace: view.namespace.clone(),
        kind: view.kind.clone(),
        state: job_state_dto(view.state),
        created_ms: view.created_ms,
        progress: Some((view.permille, view.note.clone())),
        outcome: view.outcome.clone(),
        result_asset,
        result_revision,
        stages: Vec::new(),
    }
}

// --------------------------------------------------------- DTO synthesis

fn job_state_dto(state: StageState) -> JobStateDto {
    match state {
        StageState::Pending => JobStateDto::Pending,
        StageState::Running => JobStateDto::Running,
        StageState::Done => JobStateDto::Succeeded,
        StageState::Skipped | StageState::Failed => JobStateDto::Failed,
        StageState::Cancelled => JobStateDto::Cancelled,
    }
}

fn run_state_dto(record: &RunRecord) -> PipelineStateDto {
    let states: Vec<StageState> = record.stages.iter().map(|s| s.state).collect();
    match derive_state(&states) {
        RunState::Pending | RunState::Running => PipelineStateDto::Running,
        RunState::Done => PipelineStateDto::Succeeded,
        RunState::Failed => PipelineStateDto::Failed,
        RunState::Cancelled => PipelineStateDto::Cancelled,
    }
}

fn synthesize_detail(pipeline: PipelineId, run: &Arc<RunHandle>) -> PipelineDetailDto {
    let record = run.record.lock().unwrap();
    let stage_specs: Vec<StageSpec> = record
        .stages
        .iter()
        .map(|view| StageSpec {
            key: view.name.clone(),
            domain: String::new(),
            deps: Vec::new(),
            weight: view.weight as u64,
            seed: 0,
            on_fail_skip: view.on_fail_skip,
        })
        .collect();
    let states: Vec<StageState> = record.stages.iter().map(|s| s.state).collect();
    let permille = (derive_progress(&stage_specs, &states) * 1000.0) as u16;
    let current_stage = record
        .stages
        .iter()
        .find(|s| s.state == StageState::Running)
        .map(|s| s.name.clone());
    let stages = record
        .stages
        .iter()
        .enumerate()
        .map(|(seq, view)| {
            let result = terminal_result(view);
            PipelineStageDto {
                name: view.name.clone(),
                seq: seq as u32,
                job: view.job,
                kind: view.kind.clone(),
                state: job_state_dto(view.state),
                skipped: view.state == StageState::Skipped,
                weight: view.weight,
                on_fail: if view.on_fail_skip {
                    StageOnFailDto::Skip
                } else {
                    StageOnFailDto::Fail
                },
                attempts: 1,
                progress: Some(JobProgressDto {
                    permille: view.permille,
                    note: view.note.clone(),
                    updated_ms: None,
                }),
                declared: Some(view.declared.clone()),
                records: Vec::new(),
                result,
            }
        })
        .collect();
    PipelineDetailDto {
        pipeline,
        namespace: record.namespace.clone(),
        title: record.title.clone(),
        state: run_state_dto(&record),
        permille,
        enqueued_by: None,
        created_ms: record.created_ms,
        prompt: record.prompt.clone(),
        current_stage,
        finished_ms: record.finished_ms,
        stages,
    }
}

/// The recorded terminal result gen.rs reads asset ids from.
fn terminal_result(view: &StageView) -> Option<JobResultDto> {
    let (outcome, body) = match view.state {
        StageState::Done => {
            let body = match &view.published {
                Some((asset, revision)) => obj(vec![
                    ("asset_id", s(asset.clone())),
                    ("revision", s(revision.clone())),
                ]),
                None => Value::Null,
            };
            ("succeeded", body)
        }
        StageState::Failed | StageState::Skipped => (
            "failed",
            obj(vec![(
                "error",
                s(view.error.clone().unwrap_or_default()),
            )]),
        ),
        StageState::Cancelled => ("cancelled", Value::Null),
        _ => return None,
    };
    Some(JobResultDto {
        outcome: outcome.to_string(),
        attempt: 1,
        recorded_ms: now_ms(),
        body,
    })
}
