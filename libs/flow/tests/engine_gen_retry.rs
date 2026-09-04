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
use std::sync::{Arc, Mutex};

const REFUSAL: &str = "backend error: insufficient VRAM for flux2-dev: need 31744 MB (estimate 29696 MB + reserve 2048 MB), only 30603 MB free after evicting every resident — refusing to load (no CPU/other-node fallback)";

#[derive(Clone)]
struct RoutedGen {
    nodes: Vec<(String, bool)>,
    picks: Arc<Mutex<Vec<(String, Vec<String>)>>>,
}

impl RoutedGen {
    fn new(refusals: usize) -> Self {
        Self {
            nodes: (1..=refusals.max(1))
                .map(|index| (format!("http://10.0.0.{index}"), index <= refusals))
                .collect(),
            picks: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn with_success_after_one_refusal() -> Self {
        Self {
            nodes: vec![
                ("http://10.0.0.217".to_string(), true),
                ("http://10.0.0.165".to_string(), false),
            ],
            picks: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl GenSeam for RoutedGen {
    fn pick(&self, _domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        Err("retry test requires model-aware routing".to_string())
    }

    fn pick_for(
        &self,
        _domain: &str,
        model: &str,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        self.picks
            .lock()
            .unwrap()
            .push((model.to_string(), excluded.to_vec()));
        let (base_url, refuses) = self
            .nodes
            .iter()
            .find(|(url, _)| !excluded.contains(url))
            .ok_or_else(|| "no provider remains".to_string())?;
        Ok(GenPick {
            provider: Box::new(RoutedProvider { refuses: *refuses }),
            base_url: base_url.clone(),
            model: model.to_string(),
            model_state: Some("ready".to_string()),
        })
    }
}

struct RoutedProvider {
    refuses: bool,
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
        assert_eq!(request.model, "flux2-dev");
        Ok("job".to_string())
    }

    fn poll(&self, _job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        let bytes = b"fake-png";
        Ok(JobStatusJson {
            job_id: "job".to_string(),
            state: if self.refuses { JOB_STATE_ERROR } else { JOB_STATE_DONE }.to_string(),
            stage: None,
            progress: None,
            artifacts: (!self.refuses)
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
            error: self.refuses.then(|| REFUSAL.to_string()),
            model: Some("flux2-dev".to_string()),
            queued_ms: None,
            started_ms: None,
            finished_ms: None,
            log: None,
            partial_text: None,
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
}

fn image_node() -> makepad_flow::Node {
    evaluate(
        "use mod.flow.*\nlet image = Image{model: \"flux2-dev\" prompt: \"x\"}\nFlow{image}\n",
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
        "retrying on http://10.0.0.165 (10.0.0.217 refused: insufficient VRAM)"
    );
    assert!(matches!(executor.poll(), Poll::Done(_)));
    let picks = picks.lock().unwrap();
    assert_eq!(picks.len(), 2);
    assert_eq!(picks[0].0, "flux2-dev");
    assert_eq!(picks[1].1, ["http://10.0.0.217"]);
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
