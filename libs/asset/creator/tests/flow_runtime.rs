#![cfg(all(feature = "flow-host", not(target_arch = "wasm32")))]
use makepad_asset_creator::makepad_ai_hub::{
    client::{ArtifactBytes, ContentProvider},
    error::AssetAiError,
    protocol::*,
    registry::Domain,
};
use makepad_asset_creator::{
    engine::{self, EngineConfig, StageOrder},
    flow::CreatorFlow,
    pipeline::{PipelineSpec, StageSpec},
};
use makepad_flow::{
    engine::{executors::gen::GenSeam, Seams},
    host::FlowServerConfig,
};
use std::sync::{atomic::AtomicBool, mpsc::channel, Arc, Mutex};

#[derive(Default)]
struct State {
    requests: Vec<GenerateRequestJson>,
    domains: Vec<Domain>,
    pending: bool,
    queued_stage: Option<String>,
    queued_progress: Option<f64>,
    cancelled: Vec<String>,
}
struct Echo(Arc<Mutex<State>>);
impl GenSeam for Echo {
    fn pick(&self, _: &str) -> Result<Box<dyn ContentProvider>, String> {
        Ok(Box::new(Self(self.0.clone())))
    }
}
impl ContentProvider for Echo {
    fn health(&self) -> Result<HealthJson, AssetAiError> {
        unreachable!()
    }
    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> {
        unreachable!()
    }
    fn request(
        &self,
        domain: Domain,
        request: &GenerateRequestJson,
    ) -> Result<String, AssetAiError> {
        let mut state = self.0.lock().unwrap();
        state.requests.push(request.clone());
        state.domains.push(domain);
        Ok((state.requests.len() - 1).to_string())
    }
    fn poll(&self, id: &str) -> Result<JobStatusJson, AssetAiError> {
        let domain = self.0.lock().unwrap().domains[id.parse::<usize>().unwrap()];
        let queued_stage = self.0.lock().unwrap().queued_stage.clone();
        let queued_progress = self.0.lock().unwrap().queued_progress;
        Ok(JobStatusJson {
            job_id: id.into(),
            state: if queued_stage.is_some() {
                JOB_STATE_QUEUED.into()
            } else if self.0.lock().unwrap().pending {
                JOB_STATE_RUNNING.into()
            } else {
                JOB_STATE_DONE.into()
            },
            stage: queued_stage,
            progress: queued_progress,
            artifacts: vec![ArtifactRefJson {
                id: id.into(),
                url: format!("/artifact/{id}"),
                content_type: if matches!(domain, Domain::Mesh | Domain::Paint) {
                    "model/gltf-binary".into()
                } else {
                    "image/png".into()
                },
                sha256: None,
                byte_len: None,
            }],
            error: None,
            model: Some("echo-used".into()),
            queued_ms: None,
            started_ms: None,
            finished_ms: None,
            log: None,
            serving: None,
            live: None,
            partial_text: None,
            text: None,
        })
    }
    fn cancel(&self, id: &str) -> Result<JobStatusJson, AssetAiError> {
        self.0.lock().unwrap().cancelled.push(id.into());
        let mut row = self.poll(id)?;
        row.state = JOB_STATE_CANCELLED.into();
        Ok(row)
    }
    fn fetch_artifact(&self, id: &str) -> Result<ArtifactBytes, AssetAiError> {
        let state = self.0.lock().unwrap();
        let request = &state.requests[id.parse::<usize>().unwrap()];
        let domain = state.domains[id.parse::<usize>().unwrap()];
        Ok(ArtifactBytes {
            content_type: if matches!(domain, Domain::Mesh | Domain::Paint) {
                "model/gltf-binary".into()
            } else {
                "image/png".into()
            },
            bytes: request.prompt.clone().unwrap().into_bytes(),
        })
    }
}

#[test]
fn primitive_reports_queued_reason_even_without_numeric_progress() {
    use makepad_asset_client::json::{obj, s};
    use makepad_asset_creator::runner::{generate_request_in, translate, CreateError};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    const REASON: &str = "waiting for local-use: gpu-counter-unavailable";
    let root = std::env::temp_dir().join(format!("creator-queued-progress-{}-{}", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
    {
        let state = Arc::new(Mutex::new(State::default()));
        let mut seams = Seams::real(); seams.gen = Arc::new(Echo(state.clone()));
        let host = CreatorFlow::start(FlowServerConfig::new(root.clone()).with_seams(seams)).unwrap();
        for progress_value in [None, Some(0.0)] {
            { let mut state = state.lock().unwrap(); state.queued_stage = Some(REASON.into()); state.queued_progress = progress_value; }
            let cancel = Arc::new(AtomicBool::new(false));
            let (_, request, wire) = translate("image.generate", &obj(vec![("prompt", s("queued")), ("model", s("echo"))]), 77).unwrap();
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut saw_reason = false;
            let result = generate_request_in(&host, request, wire, &cancel, &mut |stage, progress| {
                if stage == REASON { assert_eq!(progress, 0); saw_reason = true; cancel.store(true, Ordering::Relaxed); }
                if Instant::now() >= deadline { cancel.store(true, Ordering::Relaxed); }
            }, Duration::from_millis(10));
            assert_eq!(result.unwrap_err(), CreateError::Cancelled);
            assert!(saw_reason, "Flow→Creator discarded a queued reason with {progress_value:?}");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn remote_attachment_runs_isolated_instances_and_uploads_media_by_digest() {
    let root = std::env::temp_dir().join(format!(
        "creator-remote-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let state = Arc::new(Mutex::new(State::default()));
        let mut seams = Seams::real();
        seams.gen = Arc::new(Echo(state.clone()));
        let owner =
            CreatorFlow::start(FlowServerConfig::new(root.clone()).with_seams(seams)).unwrap();
        let remote = CreatorFlow::attach(&root).unwrap();
        assert!(owner.is_embedded());
        assert!(!remote.is_embedded());
        let stage = StageSpec {
            key: "image".into(),
            domain: "image".into(),
            deps: vec![],
            weight: 10,
            seed: 7,
            on_fail_skip: false,
        };
        let spec = PipelineSpec {
            name: "echo image".into(),
            stages: vec![stage.clone()],
        };
        // This image exceeds the 1 MiB control-plane limit after Base64.
        let payload = vec![123u8; 1024 * 1024];
        let b64 = String::from_utf8(
            makepad_asset_creator::makepad_ai_hub::makepad_base64::base64_encode(
                &payload,
                &makepad_asset_creator::makepad_ai_hub::makepad_base64::BASE64_STANDARD,
            ),
        )
        .unwrap();
        for (prompt, seed) in [("first", u64::MAX), ("second", u64::MAX - 1)] {
            let order = StageOrder {
                spec: stage.clone(),
                splices: vec![],
                request: GenerateRequestJson {
                    model: "echo".into(),
                    prompt: Some(prompt.into()),
                    seed: Some(seed),
                    input_b64: Some(b64.clone()),
                    input_content_type: Some("image/png".into()),
                    pixal: Some(PixalOptionsJson {
                        camera_fov: Some(42.5),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
            };
            let (tx, rx) = channel();
            let outputs = engine::run_in(
                &remote,
                &spec,
                &[order],
                &EngineConfig {
                    poll_interval: std::time::Duration::from_millis(10),
                },
                &tx,
                &Arc::new(AtomicBool::new(false)),
            )
            .unwrap();
            assert!(
                outputs.contains_key("image"),
                "{:?}",
                rx.try_iter().collect::<Vec<_>>()
            );
            assert_eq!(
                outputs["image"].artifact.as_ref().unwrap().bytes,
                prompt.as_bytes()
            );
        }
        let requests = &state.lock().unwrap().requests;
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].seed, Some(u64::MAX));
        assert_eq!(requests[1].seed, Some(u64::MAX - 1));
        assert_eq!(requests[0].pixal.as_ref().unwrap().camera_fov, Some(42.5));
        assert_eq!(requests[0].input_b64.as_ref(), Some(&b64));
        drop(remote);
        // Detaching a client cannot stop its host.
        assert!(CreatorFlow::attach(&root).is_ok());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn primitive_flow_preserves_provenance_and_cancels_only_its_active_job() {
    use makepad_asset_client::json::{obj, s};
    use makepad_asset_creator::runner::{generate_request_in, translate, CreateError};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};
    let root = std::env::temp_dir().join(format!(
        "creator-primitive-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let state = Arc::new(Mutex::new(State::default()));
        let mut seams = Seams::real();
        seams.gen = Arc::new(Echo(state.clone()));
        let owner =
            CreatorFlow::start(FlowServerConfig::new(root.clone()).with_seams(seams)).unwrap();
        let remote = CreatorFlow::attach(&root).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let body = obj(vec![("prompt", s("primitive")), ("model", s("echo"))]);
        let (_, request, wire) = translate("image.generate", &body, u64::MAX).unwrap();
        let output = generate_request_in(
            &remote,
            request,
            wire,
            &cancel,
            &mut |_, _| {},
            Duration::from_millis(10),
        )
        .unwrap();
        assert_eq!(output.request.seed, Some(u64::MAX));
        assert_eq!(output.request.model, "echo-used");
        assert_eq!(output.job_id, "0");
        assert_eq!(output.artifact.unwrap().bytes, b"primitive");
        state.lock().unwrap().pending = true;
        let (_, request, wire) = translate("image.generate", &body, 17).unwrap();
        let result = generate_request_in(
            &remote,
            request,
            wire,
            &cancel,
            &mut |_, _| {
                if state.lock().unwrap().requests.len() == 2 {
                    cancel.store(true, Ordering::Relaxed);
                }
            },
            Duration::from_millis(10),
        );
        assert_eq!(result.unwrap_err(), CreateError::Cancelled);
        let deadline = Instant::now() + Duration::from_secs(3);
        while state.lock().unwrap().cancelled.is_empty() {
            assert!(
                Instant::now() < deadline,
                "Flow never cancelled its accepted hub job"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(state.lock().unwrap().cancelled, ["1"]);
        let runs = loop {
            let runs = remote.client().runs(None).unwrap();
            if runs
                .iter()
                .any(|r| r.state == makepad_flow::RunState::Cancelled)
            {
                break runs;
            }
            assert!(
                Instant::now() < deadline,
                "Flow did not persist cancellation"
            );
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(runs.len(), 2);
        assert_eq!(
            runs.iter()
                .filter(|r| r.state == makepad_flow::RunState::Done)
                .count(),
            1
        );
        assert_eq!(
            runs.iter()
                .filter(|r| r.state == makepad_flow::RunState::Cancelled)
                .count(),
            1
        );
        drop(remote);
        assert!(owner.client().runs(None).is_ok());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mesh_artifact_passes_through_a_primary_splice_to_paint() {
    let root = std::env::temp_dir().join(format!(
        "creator-mesh-splice-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let state = Arc::new(Mutex::new(State::default()));
        let mut seams = Seams::real();
        seams.gen = Arc::new(Echo(state.clone()));
        let host =
            CreatorFlow::start(FlowServerConfig::new(root.clone()).with_seams(seams)).unwrap();
        let stages: Vec<_> = [("mesh", vec![]), ("paint", vec!["mesh".into()])]
            .into_iter()
            .map(|(domain, deps)| StageSpec {
                key: domain.into(),
                domain: domain.into(),
                deps,
                weight: 10,
                seed: 42,
                on_fail_skip: false,
            })
            .collect();
        let orders: Vec<_> = stages
            .iter()
            .enumerate()
            .map(|(i, stage)| StageOrder {
                spec: stage.clone(),
                splices: if i == 0 {
                    vec![]
                } else {
                    vec![engine::Splice::InputImageFrom("mesh".into())]
                },
                request: GenerateRequestJson {
                    model: "echo".into(),
                    prompt: Some(stage.key.clone()),
                    ..Default::default()
                },
            })
            .collect();
        let (tx, _rx) = channel();
        let outputs = engine::run_in(
            &host,
            &PipelineSpec {
                name: "mesh to paint".into(),
                stages,
            },
            &orders,
            &EngineConfig {
                poll_interval: std::time::Duration::from_millis(10),
            },
            &tx,
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap();
        assert!(outputs.contains_key("paint"));
        let requests = &state.lock().unwrap().requests;
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].input_content_type.as_deref(),
            Some("model/gltf-binary")
        );
        assert_eq!(requests[1].input_b64.as_deref(), Some("bWVzaA=="));
    }
    std::fs::remove_dir_all(root).unwrap();
}
