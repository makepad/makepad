//! The engine that runs one pipeline against the hub.
//!
//! Blocking, on the caller's thread (a headless runner, or an app's worker):
//! walk the deps gate, dispatch each ready stage through a
//! [`ContentProvider`], poll, splice outputs forward, and report every
//! transition through an event channel. The two splice rules are the ones
//! asset-ui's chain engine shipped with: a text stage's output becomes the
//! next stage's prompt, and an image-bearing stage's artifact becomes the
//! next stage's input image.
//!
//! Artifacts pass BY VALUE here only at the machine edge (fetch → re-attach
//! b64); the by-digest cross-node path arrives with the P5 conductor.
//! Cancellation is cooperative and total: raise the flag and the run stops
//! at its next poll, cancelling what it started (queued jobs die server-side;
//! a running stage runs its course under v1 cancel semantics).

use crate::pipeline::{
    derive_state, ready_stages, PipelineSpec, RunState, StageSpec, StageState,
};
use makepad_ai_hub::client::{ArtifactBytes, ContentProvider};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::protocol::{
    GenerateRequestJson, JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_ERROR,
};
use makepad_ai_hub::registry::Domain;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use makepad_ai_hub::makepad_base64;
use std::time::Duration;

/// How stage outputs feed a later stage's request.
#[derive(Clone, Debug)]
pub enum Splice {
    /// The named dependency's text output replaces this stage's prompt
    /// (asset-ui's expand rule; the run carries on with the typed prompt if
    /// the dependency produced no text — an expansion is a courtesy).
    PromptFromText(String),
    /// The named dependency's artifact bytes ride as this stage's input
    /// image (asset-ui's cross-stage relay).
    InputImageFrom(String),
    /// The named dependency's artifact rides as a NAMED input (the DREAM
    /// video's `last_frame`: the still the clip ends on).
    NamedInputFrom {
        dep: String,
        name: String,
        content_type: String,
    },
}

/// One stage's concrete work order: the spec's graph node plus the request
/// it submits. Seeds live in the REQUEST and are pinned at submission.
#[derive(Clone, Debug)]
pub struct StageOrder {
    pub spec: StageSpec,
    pub request: GenerateRequestJson,
    pub splices: Vec<Splice>,
}

/// What a finished stage left behind.
#[derive(Clone, Debug, Default)]
pub struct StageOutput {
    /// Text output (utf-8 artifact), when the stage produced one.
    pub text: Option<String>,
    /// First artifact's bytes + declared content type.
    pub artifact: Option<ArtifactBytes>,
    pub artifact_id: Option<String>,
}

/// Progress events, one channel per run.
#[derive(Debug)]
pub enum RunEvent {
    StageStarted { key: String, job_id: String },
    StageProgress { key: String, stage: Option<String>, progress: Option<f64> },
    /// The stage finished; its output rides along so a consumer can publish
    /// per stage while the run continues.
    StageDone { key: String, output: Arc<StageOutput> },
    StageFailed { key: String, error: String },
    /// A failed stage the spec declared skippable; the run continues.
    StageSkipped { key: String, error: String },
    RunFinished { state: RunState },
}

/// Chooses the node a stage executes on, at DISPATCH time — so a chain's
/// later stages see fresh fleet state (the asset-ui chain engine's law).
/// A single-node caller returns clones of one base URL every time.
pub trait ProviderPick {
    fn pick(&self, stage: &StageSpec) -> Result<Box<dyn ContentProvider>, AssetAiError>;
}

/// One fixed provider for every stage (tests, the single-box runner).
pub struct SingleProvider<F: Fn() -> Box<dyn ContentProvider>>(pub F);

impl<F: Fn() -> Box<dyn ContentProvider>> ProviderPick for SingleProvider<F> {
    fn pick(&self, _stage: &StageSpec) -> Result<Box<dyn ContentProvider>, AssetAiError> {
        Ok((self.0)())
    }
}

/// Engine pacing.
pub struct EngineConfig {
    pub poll_interval: Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self { poll_interval: Duration::from_millis(500) }
    }
}

/// Run one pipeline to a terminal state. Returns the per-stage outputs of
/// every DONE stage (a failed run still returns what finished — resumability
/// is built on exactly this).
#[cfg(target_arch = "wasm32")]
pub fn run(
    spec: &PipelineSpec,
    orders: &[StageOrder],
    providers: &dyn ProviderPick,
    config: &EngineConfig,
    events: &Sender<RunEvent>,
    cancel: &Arc<AtomicBool>,
) -> Result<HashMap<String, Arc<StageOutput>>, AssetAiError> {
    let _ = (spec, orders, providers, config, events, cancel);
    Err(AssetAiError::Unavailable(
        "asset creator LAN fleet is unavailable on wasm".to_string(),
    ))
}

// LAN fleet discovery and its polling loop are native-only.
#[cfg(not(target_arch = "wasm32"))]
pub fn run(
    spec: &PipelineSpec,
    orders: &[StageOrder],
    providers: &dyn ProviderPick,
    config: &EngineConfig,
    events: &Sender<RunEvent>,
    cancel: &Arc<AtomicBool>,
) -> Result<HashMap<String, Arc<StageOutput>>, AssetAiError> {
    crate::pipeline::validate(spec).map_err(AssetAiError::Backend)?;
    if orders.len() != spec.stages.len() {
        return Err(AssetAiError::Backend(format!(
            "pipeline {}: {} orders for {} stages",
            spec.name,
            orders.len(),
            spec.stages.len()
        )));
    }
    let mut states = vec![StageState::Pending; spec.stages.len()];
    let mut outputs: HashMap<String, Arc<StageOutput>> = HashMap::new();
    let mut jobs: HashMap<usize, (String, Box<dyn ContentProvider>)> = HashMap::new();

    loop {
        if cancel.load(Ordering::Relaxed) {
            for (i, (job_id, provider)) in &jobs {
                if states[*i] == StageState::Running {
                    let _ = provider.cancel(job_id);
                    states[*i] = StageState::Cancelled;
                }
            }
            for state in &mut states {
                if *state == StageState::Pending {
                    *state = StageState::Cancelled;
                }
            }
            let _ = events.send(RunEvent::RunFinished { state: RunState::Cancelled });
            return Ok(outputs);
        }

        // Dispatch everything the deps gate has opened.
        for i in ready_stages(spec, &states) {
            let order = &orders[i];
            let mut request = order.request.clone();
            apply_splices(&mut request, &order.splices, &outputs);
            let domain = parse_domain(&order.spec.domain)?;
            let provider = match providers.pick(&order.spec) {
                Ok(provider) => provider,
                Err(error) => {
                    fail_stage(&mut states, i, &spec.stages[i], error.to_string(), events);
                    continue;
                }
            };
            match provider.request(domain, &request) {
                Ok(job_id) => {
                    let _ = events.send(RunEvent::StageStarted {
                        key: order.spec.key.clone(),
                        job_id: job_id.clone(),
                    });
                    jobs.insert(i, (job_id, provider));
                    states[i] = StageState::Running;
                }
                Err(error) => {
                    fail_stage(&mut states, i, &spec.stages[i], error.to_string(), events);
                }
            }
        }

        // Poll the running set.
        let running: Vec<usize> = jobs.keys().copied().collect();
        for i in running {
            if states[i] != StageState::Running {
                continue;
            }
            let (job_id, provider) = jobs.get(&i).unwrap();
            let job_id = job_id.clone();
            let key = spec.stages[i].key.clone();
            let status = match provider.poll(&job_id) {
                Ok(status) => status,
                Err(error) => {
                    fail_stage(&mut states, i, &spec.stages[i], error.to_string(), events);
                    continue;
                }
            };
            match status.state.as_str() {
                JOB_STATE_DONE => {
                    let mut output = StageOutput::default();
                    // The LLM path returns its answer inline; artifact text
                    // (a text/* blob) is the fallback for older backends.
                    output.text = status.text.clone().filter(|t| !t.trim().is_empty());
                    if let Some(artifact) = status.artifacts.first() {
                        output.artifact_id = Some(artifact.id.clone());
                        match provider.fetch_artifact(&artifact.id) {
                            Ok(bytes) => {
                                if output.text.is_none()
                                    && bytes.content_type.starts_with("text/")
                                {
                                    output.text =
                                        String::from_utf8(bytes.bytes.clone()).ok();
                                }
                                output.artifact = Some(bytes);
                            }
                            Err(error) => {
                                fail_stage(
                                    &mut states,
                                    i,
                                    &spec.stages[i],
                                    format!("artifact fetch: {error}"),
                                    events,
                                );
                                continue;
                            }
                        }
                    }
                    let output = Arc::new(output);
                    outputs.insert(key.clone(), output.clone());
                    states[i] = StageState::Done;
                    let _ = events.send(RunEvent::StageDone { key, output });
                }
                JOB_STATE_ERROR => {
                    fail_stage(
                        &mut states,
                        i,
                        &spec.stages[i],
                        status.error.unwrap_or_else(|| "job error".into()),
                        events,
                    );
                }
                JOB_STATE_CANCELLED => {
                    states[i] = StageState::Cancelled;
                }
                _ => {
                    let _ = events.send(RunEvent::StageProgress {
                        key,
                        stage: status.stage,
                        progress: status.progress,
                    });
                }
            }
        }

        let run_state = derive_state(&states);
        match run_state {
            RunState::Running | RunState::Pending => {
                #[cfg(not(target_arch = "wasm32"))]
                std::thread::sleep(config.poll_interval);
                #[cfg(target_arch = "wasm32")]
                std::hint::spin_loop();
            }
            terminal => {
                let _ = events.send(RunEvent::RunFinished { state: terminal });
                return Ok(outputs);
            }
        }
    }
}

/// One failure point: honors `on_fail_skip` so a skippable stage never
/// dooms the run (the DREAM expand law).
fn fail_stage(
    states: &mut [StageState],
    i: usize,
    spec: &StageSpec,
    error: String,
    events: &Sender<RunEvent>,
) {
    if spec.on_fail_skip {
        states[i] = StageState::Skipped;
        let _ = events.send(RunEvent::StageSkipped {
            key: spec.key.clone(),
            error,
        });
    } else {
        states[i] = StageState::Failed;
        let _ = events.send(RunEvent::StageFailed {
            key: spec.key.clone(),
            error,
        });
    }
}

fn apply_splices(
    request: &mut GenerateRequestJson,
    splices: &[Splice],
    outputs: &HashMap<String, Arc<StageOutput>>,
) {
    for splice in splices {
        match splice {
            Splice::PromptFromText(dep) => {
                if let Some(text) = outputs.get(dep).and_then(|o| o.text.clone()) {
                    let text = text.trim();
                    if !text.is_empty() {
                        request.prompt = Some(text.to_string());
                    }
                }
            }
            Splice::InputImageFrom(dep) => {
                if let Some(artifact) = outputs.get(dep).and_then(|o| o.artifact.as_ref()) {
                    let b64 = makepad_base64::base64_encode(
                        &artifact.bytes,
                        &makepad_base64::BASE64_STANDARD,
                    );
                    request.input_b64 = String::from_utf8(b64).ok();
                }
            }
            Splice::NamedInputFrom { dep, name, content_type } => {
                if let Some(artifact) = outputs.get(dep).and_then(|o| o.artifact.as_ref()) {
                    let b64 = makepad_base64::base64_encode(
                        &artifact.bytes,
                        &makepad_base64::BASE64_STANDARD,
                    );
                    if let Ok(data_b64) = String::from_utf8(b64) {
                        let inputs = request.inputs.get_or_insert_with(Vec::new);
                        inputs.push(makepad_ai_hub::protocol::NamedInputJson {
                            name: name.clone(),
                            content_type: content_type.clone(),
                            data_b64,
                        });
                    }
                }
            }
        }
    }
}

fn parse_domain(name: &str) -> Result<Domain, AssetAiError> {
    Ok(match name {
        "image" => Domain::Image,
        "mesh" => Domain::Mesh,
        "video" => Domain::Video,
        "audio" => Domain::Audio,
        "text" => Domain::Text,
        "speech" => Domain::Speech,
        "world" => Domain::World,
        "matte" => Domain::Matte,
        "depth" => Domain::Depth,
        other => {
            return Err(AssetAiError::Backend(format!("unknown stage domain: {other}")))
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline::DEFAULT_STAGE_WEIGHT;
    use makepad_ai_hub::protocol::{
        ArtifactRefJson, HealthJson, JobStatusJson, ModelInfoJson, JOB_STATE_RUNNING,
    };
    use std::sync::mpsc::channel;
    use std::sync::Mutex;

    /// A scripted provider: each submitted job walks a queue of statuses.
    struct FakeProvider {
        scripts: Mutex<HashMap<String, Vec<JobStatusJson>>>,
        artifacts: Mutex<HashMap<String, ArtifactBytes>>,
        submitted: Mutex<Vec<GenerateRequestJson>>,
        next_id: Mutex<u64>,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                scripts: Mutex::new(HashMap::new()),
                artifacts: Mutex::new(HashMap::new()),
                submitted: Mutex::new(Vec::new()),
                next_id: Mutex::new(0),
            }
        }
        fn done_status(job: &str, artifact: Option<&str>) -> JobStatusJson {
            JobStatusJson {
                job_id: job.into(),
                state: JOB_STATE_DONE.into(),
                stage: None,
                progress: None,
                artifacts: artifact
                    .map(|id| {
                        vec![ArtifactRefJson {
                            id: id.into(),
                            url: format!("/artifact/{id}"),
                            content_type: "image/png".into(),
                            sha256: None,
                            byte_len: None,
                        }]
                    })
                    .unwrap_or_default(),
                error: None,
                model: None,
                queued_ms: None,
                started_ms: None,
                finished_ms: None,
                log: None,
                serving: None,
                live: None,
                partial_text: None,
                text: None,
            }
        }
    }

    struct PickArc(std::sync::Arc<FakeProvider>);

    impl ProviderPick for PickArc {
        fn pick(&self, _stage: &StageSpec) -> Result<Box<dyn ContentProvider>, AssetAiError> {
            Ok(Box::new(Shim(self.0.clone())))
        }
    }

    /// Orphan-rule shim: a local wrapper so the shared fake can be handed
    /// out as many boxed providers over one state.
    struct Shim(std::sync::Arc<FakeProvider>);

    impl ContentProvider for Shim {
        fn health(&self) -> Result<HealthJson, AssetAiError> {
            self.0.health()
        }
        fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> {
            self.0.list_models()
        }
        fn request(
            &self,
            domain: Domain,
            request: &GenerateRequestJson,
        ) -> Result<String, AssetAiError> {
            self.0.request(domain, request)
        }
        fn poll(&self, job_id: &str) -> Result<JobStatusJson, AssetAiError> {
            self.0.poll(job_id)
        }
        fn fetch_artifact(&self, artifact_id: &str) -> Result<ArtifactBytes, AssetAiError> {
            self.0.fetch_artifact(artifact_id)
        }
    }

    impl ContentProvider for FakeProvider {
        fn health(&self) -> Result<HealthJson, AssetAiError> {
            Err(AssetAiError::Unavailable("fake".into()))
        }
        fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> {
            Ok(Vec::new())
        }
        fn request(
            &self,
            _domain: Domain,
            request: &GenerateRequestJson,
        ) -> Result<String, AssetAiError> {
            self.submitted.lock().unwrap().push(request.clone());
            let mut next = self.next_id.lock().unwrap();
            *next += 1;
            Ok(format!("job-{next}", next = *next))
        }
        fn poll(&self, job_id: &str) -> Result<JobStatusJson, AssetAiError> {
            let mut scripts = self.scripts.lock().unwrap();
            let queue = scripts.entry(job_id.to_string()).or_insert_with(|| {
                vec![Self::done_status(job_id, None)]
            });
            Ok(if queue.len() > 1 { queue.remove(0) } else { queue[0].clone() })
        }
        fn fetch_artifact(&self, artifact_id: &str) -> Result<ArtifactBytes, AssetAiError> {
            self.artifacts
                .lock()
                .unwrap()
                .get(artifact_id)
                .cloned()
                .ok_or_else(|| AssetAiError::Backend("no artifact".into()))
        }
    }

    fn stage(key: &str, domain: &str, deps: &[&str]) -> StageSpec {
        StageSpec {
            key: key.into(),
            domain: domain.into(),
            deps: deps.iter().map(|d| d.to_string()).collect(),
            weight: DEFAULT_STAGE_WEIGHT,
            seed: 7,
            on_fail_skip: false,
        }
    }

    fn order(spec: StageSpec, splices: Vec<Splice>) -> StageOrder {
        let request = GenerateRequestJson {
            model: String::new(),
            prompt: Some("typed prompt".into()),
            ..Default::default()
        };
        StageOrder { spec, request, splices }
    }

    #[test]
    fn a_two_stage_chain_splices_text_into_the_prompt() {
        let spec = PipelineSpec {
            name: "expand-image".into(),
            stages: vec![stage("expand", "text", &[]), stage("image", "image", &["expand"])],
        };
        let provider = std::sync::Arc::new(FakeProvider::new());
        // Stage 1 (job-1) finishes with a text artifact.
        provider.scripts.lock().unwrap().insert(
            "job-1".into(),
            vec![FakeProvider::done_status("job-1", Some("art-1"))],
        );
        provider.artifacts.lock().unwrap().insert(
            "art-1".into(),
            ArtifactBytes {
                content_type: "text/plain".into(),
                bytes: b"an expanded prompt".to_vec(),
            },
        );
        let orders = vec![
            order(spec.stages[0].clone(), vec![]),
            order(
                spec.stages[1].clone(),
                vec![Splice::PromptFromText("expand".into())],
            ),
        ];
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let config = EngineConfig { poll_interval: Duration::from_millis(1) };
        let outputs = run(&spec, &orders, &PickArc(provider.clone()), &config, &tx, &cancel).unwrap();
        assert!(outputs.contains_key("expand") && outputs.contains_key("image"));
        // The image stage's submitted request carries the expanded prompt.
        let submitted = provider.submitted.lock().unwrap();
        assert_eq!(submitted[1].prompt.as_deref(), Some("an expanded prompt"));
        let finished = rx.try_iter().filter(|e| matches!(e, RunEvent::RunFinished { state } if *state == RunState::Done)).count();
        assert_eq!(finished, 1);
    }

    #[test]
    fn a_failed_stage_fails_the_run_but_keeps_finished_outputs() {
        let spec = PipelineSpec {
            name: "expand-image".into(),
            stages: vec![stage("expand", "text", &[]), stage("image", "image", &["expand"])],
        };
        let provider = std::sync::Arc::new(FakeProvider::new());
        provider.scripts.lock().unwrap().insert(
            "job-1".into(),
            vec![FakeProvider::done_status("job-1", None)],
        );
        provider.scripts.lock().unwrap().insert("job-2".into(), {
            let mut error = FakeProvider::done_status("job-2", None);
            error.state = JOB_STATE_ERROR.into();
            error.error = Some("oom".into());
            vec![error]
        });
        let orders = vec![
            order(spec.stages[0].clone(), vec![]),
            order(spec.stages[1].clone(), vec![]),
        ];
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let config = EngineConfig { poll_interval: Duration::from_millis(1) };
        let outputs = run(&spec, &orders, &PickArc(provider.clone()), &config, &tx, &cancel).unwrap();
        assert!(outputs.contains_key("expand"), "finished work survives");
        let failed = rx
            .try_iter()
            .any(|e| matches!(e, RunEvent::RunFinished { state: RunState::Failed }));
        assert!(failed);
    }

    #[test]
    fn cancellation_stops_the_run_and_reports_it() {
        let spec = PipelineSpec {
            name: "one".into(),
            stages: vec![stage("expand", "text", &[])],
        };
        let provider = std::sync::Arc::new(FakeProvider::new());
        // The stage never finishes on its own.
        provider.scripts.lock().unwrap().insert("job-1".into(), {
            let mut running = FakeProvider::done_status("job-1", None);
            running.state = JOB_STATE_RUNNING.into();
            vec![running]
        });
        let orders = vec![order(spec.stages[0].clone(), vec![])];
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(true));
        let config = EngineConfig { poll_interval: Duration::from_millis(1) };
        let _ = run(&spec, &orders, &PickArc(provider.clone()), &config, &tx, &cancel).unwrap();
        let cancelled = rx
            .try_iter()
            .any(|e| matches!(e, RunEvent::RunFinished { state: RunState::Cancelled }));
        assert!(cancelled);
    }
}
