use makepad_ai_hub::client::{ArtifactBytes, ContentProvider};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::lease::{LeaseTable, Origin, Renew};
use makepad_ai_hub::protocol::{
    ArtifactRefJson, GenerateRequestJson, HealthJson, JobStatusJson, ModelInfoJson,
    JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_RUNNING,
};
use makepad_ai_hub::registry::Domain;
use makepad_flow::engine::executors::gen::{GenExecutor, GenPick, GenSeam};
use makepad_flow::engine::executors::{Executor, Poll};
use makepad_flow::graph::evaluate;
use makepad_micro_serde::SerJson;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

const BYTES: &[u8] = b"test image";

#[derive(Default)]
struct Hub {
    leases: LeaseTable,
    jobs: BTreeMap<String, JobStatusJson>,
    requests: Vec<GenerateRequestJson>,
    bye_calls: usize,
}

impl Hub {
    fn complete_active(&mut self) {
        for (id, status) in &mut self.jobs {
            if status.state == JOB_STATE_RUNNING {
                status.state = JOB_STATE_DONE.to_string();
                status.artifacts = vec![ArtifactRefJson {
                    id: "artifact".into(),
                    url: "/artifact/artifact".into(),
                    content_type: "image/png".into(),
                    sha256: Some(makepad_ai_hub::sha256::sha256_hex(BYTES)),
                    byte_len: Some(BYTES.len() as u64),
                }];
                self.leases.release(id);
            }
        }
    }
}

#[derive(Clone, Default)]
struct Farm {
    hubs: [Arc<Mutex<Hub>>; 2],
}

impl GenSeam for Farm {
    fn pick(&self, _domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        unreachable!("the executor uses request-aware routing")
    }

    fn pick_for_request(
        &self,
        _domain: &str,
        request: &GenerateRequestJson,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        let (index, url) = ["http://hub-a", "http://hub-b"]
            .into_iter()
            .enumerate()
            .find(|(_, url)| !excluded.iter().any(|excluded| excluded == url))
            .ok_or_else(|| "no alternate provider".to_string())?;
        Ok(GenPick {
            provider: Box::new(Provider {
                hub: self.hubs[index].clone(),
                origin: Mutex::new(None),
                reject_for_local_use: index == 0 && request.prompt.as_deref() == Some("retry"),
            }),
            base_url: url.into(),
            model: request.model.clone(),
            model_state: Some("ready".into()),
        })
    }
}

struct Provider {
    hub: Arc<Mutex<Hub>>,
    origin: Mutex<Option<Origin>>,
    reject_for_local_use: bool,
}

impl ContentProvider for Provider {
    fn health(&self) -> Result<HealthJson, AssetAiError> {
        Err(AssetAiError::Unavailable("unused".into()))
    }

    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> {
        Err(AssetAiError::Unavailable("unused".into()))
    }

    fn request(
        &self,
        _domain: Domain,
        request: &GenerateRequestJson,
    ) -> Result<String, AssetAiError> {
        let origin = Origin {
            node_key: request.origin_key.clone().expect("leased generation"),
            epoch: request.origin_epoch.expect("leased generation epoch"),
        };
        assert!(origin.node_key.len() <= 128, "AIHub origin key limit");
        assert_eq!(origin.epoch, 7);
        *self.origin.lock().unwrap() = Some(origin.clone());
        let mut hub = self.hub.lock().unwrap();
        let job_id = format!("job-{}", hub.jobs.len() + 1);
        assert_eq!(hub.leases.register(&job_id, origin, 0), Renew::Renewed);
        let state = if self.reject_for_local_use {
            hub.leases.release(&job_id);
            JOB_STATE_CANCELLED
        } else {
            JOB_STATE_RUNNING
        };
        hub.jobs.insert(
            job_id.clone(),
            JobStatusJson {
                job_id: job_id.clone(),
                state: state.into(),
                stage: Some("generating".into()),
                progress: Some(0.5),
                artifacts: Vec::new(),
                error: self
                    .reject_for_local_use
                    .then(|| "local-use: gpu-counter-unavailable".into()),
                model: Some(request.model.clone()),
                queued_ms: None,
                started_ms: None,
                finished_ms: None,
                log: None,
                partial_text: None,
                live: None,
                serving: None,
                text: None,
            },
        );
        hub.requests.push(request.clone());
        Ok(job_id)
    }

    fn poll(&self, job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        Ok(self.hub.lock().unwrap().jobs[job_id].clone())
    }

    fn fetch_artifact(&self, _artifact_id: &str) -> Result<ArtifactBytes, AssetAiError> {
        Ok(ArtifactBytes {
            content_type: "image/png".into(),
            bytes: BYTES.to_vec(),
        })
    }

    fn cancel(&self, job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        let mut hub = self.hub.lock().unwrap();
        hub.leases.release(job_id);
        hub.jobs.get_mut(job_id).unwrap().state = JOB_STATE_CANCELLED.into();
        Ok(hub.jobs[job_id].clone())
    }

    fn bye(&self) -> Result<(), AssetAiError> {
        let origin = self
            .origin
            .lock()
            .unwrap()
            .clone()
            .expect("submitted origin");
        let mut hub = self.hub.lock().unwrap();
        hub.bye_calls += 1;
        // Exercise AIHub's actual origin-wide lease semantics, including the
        // sibling cancellation that a provider-local fake would conceal.
        for (job_id, _) in hub.leases.bye(&origin.node_key) {
            hub.jobs.get_mut(&job_id).unwrap().state = JOB_STATE_CANCELLED.into();
        }
        Ok(())
    }
}

fn node(prompt: &str) -> makepad_flow::Node {
    evaluate(
        &format!("use mod.flow.*\nlet image = Image{{model: \"flux1-schnell\" prompt: \"{prompt}\" width: 64 height: 64 steps: 4 seed: 77}}\nFlow{{image}}\n"),
        "lease-isolation.splash",
    ).unwrap().nodes.into_iter().find(|node| node.id == "image").unwrap()
}

fn executor(farm: &Farm) -> GenExecutor {
    // Every concept card is owned by the same Flow host and epoch. Use the
    // maximum wire key length so a derived identity cannot append past it.
    GenExecutor::new(Arc::new(farm.clone()), ("h".repeat(128), 7))
}

#[test]
fn cancelling_one_generation_leaves_a_sibling_on_the_same_hub_running() {
    let farm = Farm::default();
    let mut cancelled = executor(&farm);
    let mut sibling = executor(&farm);
    cancelled.start(&node("cancel"), &[]).unwrap();
    sibling.start(&node("sibling"), &[]).unwrap();

    cancelled.cancel();
    assert!(matches!(cancelled.poll(), Poll::Pending));
    assert!(
        matches!(sibling.poll(), Poll::Progress { .. }),
        "origin goodbye cancelled the sibling"
    );
    {
        let hub = farm.hubs[0].lock().unwrap();
        assert_eq!(hub.bye_calls, 1);
        assert_eq!(hub.leases.len(), 1);
        assert_eq!(
            hub.requests.len(),
            2,
            "the sibling was not silently retried"
        );
    }
    farm.hubs[0].lock().unwrap().complete_active();
    assert!(matches!(sibling.poll(), Poll::Done(_)));
}

#[test]
fn local_use_retry_preserves_its_request_without_cancelling_a_sibling() {
    let farm = Farm::default();
    let mut retrying = executor(&farm);
    let mut sibling = executor(&farm);
    retrying
        .start(&node("retry"), &[("prompt".into(), makepad_flow::Value::text("retry"))])
        .unwrap();
    sibling.start(&node("sibling"), &[]).unwrap();

    let Poll::Progress { stage, .. } = retrying.poll() else {
        panic!("no failover");
    };
    assert!(stage.contains("retrying on hub-b"), "{stage}");
    assert!(
        matches!(sibling.poll(), Poll::Progress { .. }),
        "retry goodbye cancelled the sibling"
    );
    {
        let original = farm.hubs[0].lock().unwrap();
        let alternate = farm.hubs[1].lock().unwrap();
        assert_eq!(original.bye_calls, 1);
        assert_eq!(original.leases.len(), 1);
        assert_eq!(original.requests.len(), 2);
        assert_eq!(alternate.requests.len(), 1);
        // Model, dimensions, seed and lease identity all survive failover.
        assert_eq!(
            original.requests[0].serialize_json(),
            alternate.requests[0].serialize_json()
        );
        assert_eq!(alternate.requests[0].seed, Some(77));
        assert_eq!(alternate.requests[0].model, "flux1-schnell");
    }
    for hub in &farm.hubs {
        hub.lock().unwrap().complete_active();
    }
    assert!(matches!(retrying.poll(), Poll::Done(_)));
    assert!(matches!(sibling.poll(), Poll::Done(_)));
}
