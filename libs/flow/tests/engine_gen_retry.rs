use makepad_ai_hub::client::{ArtifactBytes, ContentProvider};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::protocol::{
    ArtifactRefJson, GenerateRequestJson, HealthJson, JobStatusJson, ModelInfoJson,
    JOB_STATE_DONE, JOB_STATE_ERROR,
};
use makepad_ai_hub::registry::Domain;
use makepad_flow::engine::executors::gen::{GenExecutor, GenPick, GenSeam};
use makepad_flow::engine::executors::{Executor, Poll};
use makepad_flow::graph::evaluate;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

const REFUSAL: &str = "backend error: insufficient VRAM for flux2-dev: need 31744 MB (estimate 29696 MB + reserve 2048 MB), only 30603 MB free after evicting every resident — refusing to load (no CPU/other-node fallback)";

#[derive(Clone)]
struct RoutedGen {
    nodes: Vec<(String, RoutedOutcome)>,
    pick_model: Option<String>,
    picks: Arc<Mutex<Vec<(String, Option<u32>, Option<u32>, Option<u32>, Option<u64>, Vec<String>)>>>,
}

impl RoutedGen {
    fn new(refusals: usize) -> Self {
        Self {
            nodes: (1..=refusals.max(1))
                .map(|index| {
                    (
                        format!("http://10.0.0.{index}"),
                        (index <= refusals).then_some(RoutedOutcome::Refusal).unwrap_or(RoutedOutcome::Success),
                    )
                })
                .collect(),
            pick_model: None,
            picks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_success_after_one_refusal() -> Self {
        Self {
            nodes: vec![
                ("http://10.0.0.217".to_string(), RoutedOutcome::Refusal),
                ("http://10.0.0.165".to_string(), RoutedOutcome::Success),
            ],
            pick_model: None,
            picks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_outcomes(outcomes: &[(u8, RoutedOutcome)]) -> Self {
        Self {
            nodes: outcomes
                .iter()
                .map(|(host, outcome)| (format!("http://10.0.0.{host}"), outcome.clone()))
                .collect(),
            pick_model: None,
            picks: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

#[derive(Clone, Copy)]
enum RoutedOutcome {
    Refusal,
    DiskRefusal,
    Success,
    CancelLocal,
    CancelUser,
    CancelAfterDelta,
}

impl GenSeam for RoutedGen {
    fn pick(&self, _domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        Err("retry test requires model-aware routing".to_string())
    }

    fn pick_for_request(
        &self,
        _domain: &str,
        request: &GenerateRequestJson,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        self.picks
            .lock()
            .unwrap()
            .push((
                request.model.clone(),
                request.width,
                request.height,
                request.steps,
                request.seed,
                excluded.to_vec(),
            ));
        let (base_url, outcome) = self
            .nodes
            .iter()
            .find(|(url, _)| !excluded.contains(url))
            .ok_or_else(|| "no provider remains".to_string())?;
        let model = self
            .pick_model
            .clone()
            .unwrap_or_else(|| request.model.clone());
        Ok(GenPick {
            provider: Box::new(RoutedProvider {
                outcome: *outcome,
                polls: AtomicUsize::new(0),
                cancelled: AtomicBool::new(false),
                expected_model: model.clone(),
            }),
            base_url: base_url.clone(),
            model,
            model_state: Some("ready".to_string()),
        })
    }
}

struct RoutedProvider {
    outcome: RoutedOutcome,
    polls: AtomicUsize,
    cancelled: AtomicBool,
    expected_model: String,
}

impl ContentProvider for RoutedProvider {
    fn health(&self) -> Result<HealthJson, AssetAiError> {
        Err(AssetAiError::Unavailable("unused".to_string()))
    }

    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> {
        Err(AssetAiError::Unavailable("unused".to_string()))
    }

    fn request(
        &self,
        _domain: Domain,
        request: &GenerateRequestJson,
    ) -> Result<String, AssetAiError> {
        assert_eq!(request.model, self.expected_model);
        Ok("job".to_string())
    }

    fn poll(&self, _job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        let poll = self.polls.fetch_add(1, Ordering::SeqCst);
        let terminal_cancel = self.cancelled.load(Ordering::SeqCst)
            || !matches!(self.outcome, RoutedOutcome::CancelAfterDelta) || poll > 0;
        let bytes = b"fake-png";
        Ok(JobStatusJson {
            job_id: "job".to_string(),
            state: match self.outcome {
                RoutedOutcome::Refusal | RoutedOutcome::DiskRefusal => JOB_STATE_ERROR,
                RoutedOutcome::Success => JOB_STATE_DONE,
                RoutedOutcome::CancelLocal | RoutedOutcome::CancelUser => {
                    makepad_ai_hub::protocol::JOB_STATE_CANCELLED
                }
                RoutedOutcome::CancelAfterDelta if terminal_cancel => {
                    makepad_ai_hub::protocol::JOB_STATE_CANCELLED
                }
                RoutedOutcome::CancelAfterDelta => makepad_ai_hub::protocol::JOB_STATE_RUNNING,
            }
            .to_string(),
            stage: None,
            progress: None,
            artifacts: matches!(self.outcome, RoutedOutcome::Success)
                .then(|| {
                    vec![ArtifactRefJson {
                        id: "artifact".to_string(),
                        url: "/artifact/artifact".to_string(),
                        content_type: "image/png".to_string(),
                        sha256: Some(makepad_ai_hub::sha256::sha256_hex(bytes)),
                        byte_len: Some(bytes.len() as u64),
                    }]
                })
                .unwrap_or_default(),
            error: match self.outcome {
                RoutedOutcome::Refusal => Some(REFUSAL.to_string()),
                RoutedOutcome::DiskRefusal => Some("model unavailable: disk-space: insufficient for model on C:; 0 GiB free".to_string()),
                RoutedOutcome::CancelLocal => Some("local-use: foreign-gpu-load".to_string()),
                RoutedOutcome::CancelAfterDelta if terminal_cancel => {
                    Some("local-use: foreign-gpu-load".to_string())
                }
                RoutedOutcome::CancelUser | RoutedOutcome::CancelAfterDelta | RoutedOutcome::Success => None,
            },
            model: Some("flux2-dev".to_string()),
            queued_ms: None,
            started_ms: None,
            finished_ms: None,
            log: None,
            partial_text: matches!(self.outcome, RoutedOutcome::CancelAfterDelta)
                .then_some("already emitted".to_string()),
            live: None,
            serving: None,
            text: None,
        })
    }

    fn fetch_artifact(&self, _artifact_id: &str) -> Result<ArtifactBytes, AssetAiError> {
        Ok(ArtifactBytes {
            content_type: "image/png".to_string(),
            bytes: b"fake-png".to_vec(),
        })
    }

    fn cancel(&self, _job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        self.cancelled.store(true, Ordering::SeqCst);
        Err(AssetAiError::Cancelled)
    }
}

fn image_node() -> makepad_flow::Node {
    evaluate(
        "use mod.flow.*\nlet image = Image{model: \"flux2-dev\" prompt: \"x\" width: 1536 height: 1536 steps: 12}\nFlow{image}\n",
        "retry.splash",
    )
    .unwrap()
    .nodes
    .into_iter()
    .find(|node| node.id == "image")
    .unwrap()
}

#[test]
fn admission_refusal_retries_on_the_second_provider_and_finishes() {
    let seam = RoutedGen::with_success_after_one_refusal();
    let picks = seam.picks.clone();
    let mut executor = GenExecutor::new(Arc::new(seam), ("retry".to_string(), 1));
    executor.start(&image_node(), &[]).unwrap();

    let Poll::Progress { stage, .. } = executor.poll() else {
        panic!("first refusal did not produce retry progress");
    };
    assert_eq!(
        stage,
        "retrying on 10.0.0.165 (10.0.0.217 refused: insufficient VRAM)"
    );
    assert!(matches!(executor.poll(), Poll::Done(_)));
    let picks = picks.lock().unwrap();
    assert_eq!(picks.len(), 2);
    assert_eq!(picks[0].0, "flux2-dev");
    assert_eq!((picks[0].1, picks[0].2, picks[0].3), (Some(1536), Some(1536), Some(12)));
    assert_eq!(picks[1].5, ["http://10.0.0.217"]);
}

#[test]
fn disk_refusal_retries_on_another_node_preserving_the_request() {
    let seam = RoutedGen::with_outcomes(&[(217, RoutedOutcome::DiskRefusal), (165, RoutedOutcome::Success)]);
    let picks = seam.picks.clone();
    let mut executor = GenExecutor::new(Arc::new(seam), ("disk-retry".into(), 1));
    executor.start(&cancellation_node(), &[]).unwrap();
    let Poll::Progress { stage, .. } = executor.poll() else { panic!("no disk failover"); };
    assert!(stage.contains("retrying on 10.0.0.165"), "{stage}");
    assert!(stage.contains("disk-space:"), "{stage}");
    assert!(matches!(executor.poll(), Poll::Done(_)));
    let picks = picks.lock().unwrap();
    assert_eq!(picks.len(), 2);
    assert_eq!((&picks[0].0, picks[0].1, picks[0].2, picks[0].3, picks[0].4),
               (&picks[1].0, picks[1].1, picks[1].2, picks[1].3, picks[1].4));
    assert_eq!(picks[1].5, ["http://10.0.0.217"]);
}

#[test]
fn three_admission_refusals_fail_with_every_node_listed() {
    let mut executor = GenExecutor::new(
        Arc::new(RoutedGen::new(3)),
        ("retry-all".to_string(), 1),
    );
    executor.start(&image_node(), &[]).unwrap();
    assert!(matches!(executor.poll(), Poll::Progress { .. }));
    assert!(matches!(executor.poll(), Poll::Progress { .. }));
    let Poll::Failed(error) = executor.poll() else {
        panic!("third refusal did not fail the node");
    };
    assert!(error.contains("10.0.0.1: insufficient VRAM"), "{error}");
    assert!(error.contains("10.0.0.2: insufficient VRAM"), "{error}");
    assert!(error.contains("10.0.0.3: insufficient VRAM"), "{error}");
}

fn cancellation_node() -> makepad_flow::Node {
    let document = evaluate(
        &format!(
            "use mod.flow.*\nlet image = Image{{model: \"flux2-dev\" prompt: \"x\" width: 1536 height: 1536 steps: 12 seed: 42}}\nFlow{{image}}\n"
        ),
        "retry-cancel.splash",
    )
    .unwrap();
    // Keep the node construction shared with image_node; the outcome belongs
    // to the seam, while the request must remain identical across attempts.
    document.nodes.into_iter().find(|node| node.id == "image").unwrap()
}

fn modelless_cancellation_node() -> makepad_flow::Node {
    let document = evaluate(
        "use mod.flow.*\nlet image = Image{prompt: \"x\" width: 1536 height: 1536 steps: 12 seed: 42}\nFlow{image}\n",
        "retry-model-retain.splash",
    )
    .unwrap();
    document.nodes.into_iter().find(|node| node.id == "image").unwrap()
}

#[test]
fn local_use_cancellation_before_partial_text_retries_and_preserves_request() {
    let seam = RoutedGen::with_outcomes(&[(217, RoutedOutcome::CancelLocal), (165, RoutedOutcome::Success)]);
    let picks = seam.picks.clone();
    let mut executor = GenExecutor::new(Arc::new(seam), ("retry-local-use".to_string(), 1));
    executor.start(&cancellation_node(), &[]).unwrap();

    let Poll::Progress { stage, .. } = executor.poll() else {
        panic!("local-use cancellation did not retry");
    };
    assert!(stage.contains("retrying on 10.0.0.165"), "{stage}");
    assert!(matches!(executor.poll(), Poll::Done(_)));
    let picks = picks.lock().unwrap();
    assert_eq!(picks.len(), 2);
    assert_eq!(picks[0].4, Some(42));
    assert_eq!(picks[1].4, Some(42));
    assert_eq!(picks[1].5, ["http://10.0.0.217"]);
}

#[test]
fn ordinary_cancellation_and_cancellation_after_delta_do_not_retry() {
    for outcome in [RoutedOutcome::CancelUser, RoutedOutcome::CancelAfterDelta] {
        let seam = RoutedGen::with_outcomes(&[(217, outcome), (165, RoutedOutcome::Success)]);
        let picks = seam.picks.clone();
        let mut executor = GenExecutor::new(Arc::new(seam), ("retry-cancel".to_string(), 1));
        executor.start(&cancellation_node(), &[]).unwrap();
        if matches!(outcome, RoutedOutcome::CancelAfterDelta) {
            assert!(matches!(executor.poll(), Poll::Delta { .. }));
        }
        let poll = executor.poll();
        assert!(matches!(poll, Poll::Failed(_)), "unexpected retry");
        assert_eq!(picks.lock().unwrap().len(), 1);
    }
}

#[test]
fn accepted_model_is_retained_when_retrying_a_modelless_request() {
    let mut seam = RoutedGen::with_outcomes(&[(217, RoutedOutcome::CancelLocal), (165, RoutedOutcome::Success)]);
    seam.pick_model = Some("qwen3.8-27b".to_string());
    let picks = seam.picks.clone();
    let mut executor = GenExecutor::new(Arc::new(seam), ("retry-model-retain".to_string(), 1));
    executor.start(&modelless_cancellation_node(), &[]).unwrap();
    assert!(matches!(executor.poll(), Poll::Progress { .. }));
    assert!(matches!(executor.poll(), Poll::Done(_)));
    let picks = picks.lock().unwrap();
    assert_eq!(picks.len(), 2);
    assert_eq!(picks[0].0, "");
    assert_eq!(picks[1].0, "qwen3.8-27b");
}

#[test]
fn explicit_user_cancel_clears_active_job_without_retrying_on_later_poll() {
    let seam = RoutedGen::with_outcomes(&[(217, RoutedOutcome::CancelUser), (165, RoutedOutcome::Success)]);
    let picks = seam.picks.clone();
    let mut executor = GenExecutor::new(Arc::new(seam), ("retry-explicit-cancel".to_string(), 1));
    executor.start(&cancellation_node(), &[]).unwrap();
    executor.cancel();
    assert!(matches!(executor.poll(), Poll::Pending));
    assert_eq!(picks.lock().unwrap().len(), 1);
}
