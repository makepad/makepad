//! Compatibility work orders compiled into Flow instances.
//! Flow owns dependency scheduling, cancellation and content-addressed results.

use crate::pipeline::{
    PipelineSpec, RunState, StageSpec,
};
use makepad_ai_hub::client::{ArtifactBytes, ContentProvider};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::protocol::GenerateRequestJson;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Duration;

/// How stage outputs feed a later stage's request.
#[derive(Clone, Debug)]
pub enum Splice {
    /// The named dependency's text output replaces this stage's prompt
    /// (asset-ui's expand rule; the run carries on with the typed prompt if
    /// the dependency produced no text — an expansion is a courtesy).
    PromptFromText(String),
    /// The dependency's artifact becomes the primary input, retaining its
    /// media type (image, mesh, audio, etc.). The variant name is historical.
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
pub trait ProviderPick: Send + Sync {
    fn pick(&self, stage: &StageSpec) -> Result<Box<dyn ContentProvider>, AssetAiError>;
}

/// One fixed provider for every stage (tests, the single-box runner).
pub struct SingleProvider<F: Fn() -> Box<dyn ContentProvider>>(pub F);

impl<F: Fn() -> Box<dyn ContentProvider> + Send + Sync> ProviderPick for SingleProvider<F> {
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

/// Compatibility entry point for callers supplying their own hub routing.
/// Scheduling is owned by a flow-server. New callers should keep a
/// `CreatorFlow` session alive and call `run_in` for each graph instance.
pub fn run(
    spec: &PipelineSpec, orders: &[StageOrder], providers: Arc<dyn ProviderPick>,
    config: &EngineConfig, events: &Sender<RunEvent>, cancel: &Arc<AtomicBool>,
) -> Result<HashMap<String, Arc<StageOutput>>, AssetAiError> {
    #[cfg(all(not(target_arch="wasm32"), feature="flow-host"))]
    {
        use makepad_flow::engine::executors::gen::{GenSeam, GenPick};
        struct Pick { providers: Arc<dyn ProviderPick>, stages: Vec<StageSpec> }
        impl GenSeam for Pick {
            fn pick(&self, _domain: &str) -> Result<Box<dyn ContentProvider>,String> { Err("creator routing requires a stage".into()) }
            fn pick_for_node(&self,node:&makepad_flow::Node,request:&GenerateRequestJson,excluded:&[String])->Result<GenPick,String> {
                if !excluded.is_empty() { return Err("creator provider has no alternate route".into()); }
                let i:usize=node.id.strip_prefix("stage_").ok_or("unknown creator stage")?.parse().map_err(|_|"invalid creator stage")?;
                let stage=self.stages.get(i).ok_or("creator stage out of range")?;
                Ok(GenPick { provider:self.providers.pick(stage).map_err(|e|e.to_string())?, base_url:"creator".into(), model:request.model.clone(),model_state:None })
            }
        }
        static NEXT_ROOT: std::sync::atomic::AtomicU64=std::sync::atomic::AtomicU64::new(0);
        let root=std::env::temp_dir().join(format!("makepad-creator-flow-{}-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos(),NEXT_ROOT.fetch_add(1,Ordering::Relaxed)));
        let mut seams=makepad_flow::engine::Seams::real();
        seams.gen=Arc::new(Pick{providers,stages:spec.stages.clone()});
        let result=(|| {
            let flow=crate::flow::CreatorFlow::start(makepad_flow::host::FlowServerConfig::new(root.clone()).with_seams(seams)).map_err(AssetAiError::Backend)?;
            run_in(&flow,spec,orders,config,events,cancel)
        })();
        let _=std::fs::remove_dir_all(root);
        result
    }
    #[cfg(not(all(not(target_arch="wasm32"), feature="flow-host")))]
    { let _=(spec,orders,providers,config,events,cancel); Err(AssetAiError::Unavailable("use run_in with a remote Flow session, or enable flow-host".into())) }
}

/// Submit an instance and translate Flow observations into creator events.
/// There is no local dependency scheduler or hub polling loop.
#[cfg(not(target_arch="wasm32"))]
pub fn run_in(
    flow:&crate::flow::CreatorFlow, spec:&PipelineSpec, orders:&[StageOrder],
    config:&EngineConfig, events:&Sender<RunEvent>, cancel:&Arc<AtomicBool>,
)->Result<HashMap<String,Arc<StageOutput>>,AssetAiError> {
    use makepad_flow::{NodeState,RunState as FlowState};
    if cancel.load(Ordering::Relaxed) {
        let _=events.send(RunEvent::RunFinished{state:RunState::Cancelled});
        return Ok(HashMap::new());
    }
    let graph=crate::flow_graph::compile(flow.client(),spec,orders).map_err(AssetAiError::Backend)?;
    flow.client().put_source(&graph.name,&graph.source).map_err(|e|AssetAiError::Backend(e.to_string()))?;
    let run=flow.submit(&graph.name,graph.inputs).map_err(AssetAiError::Backend)?;
    let result=(|| {
        let mut outputs=HashMap::new();
        let mut seen=HashMap::new();
        let mut cancelling=false;
        loop {
            if cancel.load(Ordering::Relaxed) && !cancelling {
                flow.cancel(&run).map_err(AssetAiError::Backend)?;
                cancelling=true;
            }
            let row=flow.snapshot(&run).map_err(AssetAiError::Backend)?;
            for (node,key) in &graph.stages {
                let Some(status)=row.nodes.get(node) else { continue; };
                let prior=seen.insert(node.clone(),status.state);
                if prior != Some(status.state) {
                    match status.state {
                        NodeState::Running => { let _=events.send(RunEvent::StageStarted{key:key.clone(),job_id:format!("{}/{}",run.run_id,node)}); }
                        NodeState::Done => {
                            let mut output=StageOutput::default();
                            if let Some(value)=status.outputs.iter().find(|v|v.port=="result") {
                                let bytes=flow.client().value(&value.value.digest).map_err(|e|AssetAiError::Backend(e.to_string()))?;
                                if value.value.ty==makepad_flow::PortType::Text { output.text=Some(String::from_utf8(bytes.bytes.to_vec()).map_err(|e|AssetAiError::Backend(e.to_string()))?); }
                                output.artifact_id=Some(bytes.digest);
                                output.artifact=Some(ArtifactBytes{content_type:bytes.content_type,bytes:bytes.bytes.to_vec()});
                            }
                            let output=Arc::new(output);
                            outputs.insert(key.clone(),output.clone());
                            let _=events.send(RunEvent::StageDone{key:key.clone(),output});
                        }
                        NodeState::Failed => { let _=events.send(RunEvent::StageFailed{key:key.clone(),error:status.error.clone().unwrap_or_else(||"flow stage failed".into())}); }
                        NodeState::Skipped => { let _=events.send(RunEvent::StageSkipped{key:key.clone(),error:status.error.clone().unwrap_or_else(||"upstream stage unavailable".into())}); }
                        _=>{}
                    }
                }
                if status.state==NodeState::Running {
                    let _=events.send(RunEvent::StageProgress{key:key.clone(),stage:status.stage.clone(),progress:status.progress.map(|v|v as f64/1000.0)});
                }
            }
            let terminal=match row.state {
                FlowState::Done=>Some(RunState::Done),
                FlowState::Failed=>Some(RunState::Failed),
                FlowState::Cancelled=>Some(RunState::Cancelled),
                FlowState::Waiting=>return Err(AssetAiError::Backend("creator graph unexpectedly requested an answer".into())),
                _=>None,
            };
            if let Some(state)=terminal { let _=events.send(RunEvent::RunFinished{state}); return Ok(outputs); }
            std::thread::sleep(config.poll_interval.max(Duration::from_millis(10)));
        }
    })();
    if result.is_err() { let _=flow.cancel(&run); }
    result
}

#[cfg(all(test,feature="flow-host",not(target_arch="wasm32")))]
mod tests {
    use super::*;
    use crate::pipeline::DEFAULT_STAGE_WEIGHT;
    use makepad_ai_hub::registry::Domain;
    use makepad_ai_hub::protocol::{JOB_STATE_DONE,JOB_STATE_ERROR};
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
                artifacts: Mutex::new(HashMap::from([("fake-image".into(),ArtifactBytes{content_type:"image/png".into(),bytes:b"image fixture".to_vec()})])),
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
                text: artifact.is_none().then(||"done".into()),
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
                vec![Self::done_status(job_id, Some("fake-image"))]
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
        let outputs = run(&spec, &orders, Arc::new(PickArc(provider.clone())), &config, &tx, &cancel).unwrap();
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
        let outputs = run(&spec, &orders, Arc::new(PickArc(provider.clone())), &config, &tx, &cancel).unwrap();
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
        let _ = run(&spec, &orders, Arc::new(PickArc(provider.clone())), &config, &tx, &cancel).unwrap();
        let cancelled = rx
            .try_iter()
            .any(|e| matches!(e, RunEvent::RunFinished { state: RunState::Cancelled }));
        assert!(cancelled);
    }
}
