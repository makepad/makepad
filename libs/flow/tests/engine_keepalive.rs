mod support;

use makepad_ai_hub::client::{ArtifactBytes, ContentProvider, LocalService};
use makepad_ai_hub::discovery::DEFAULT_FLEET;
use makepad_ai_hub::download::Downloader;
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::peer_serve::PeerOptions;
use makepad_ai_hub::protocol::{
    ArtifactRefJson, GenerateRequestJson, HealthJson, JobStatusJson, ModelInfoJson,
    JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_RUNNING,
};
use makepad_ai_hub::registry::{Domain, ModelSpec, Registry};
use makepad_ai_hub::server::{start_service, ServiceConfig};
use makepad_flow::engine::executors::gen::GenSeam;
use makepad_flow::engine::{spawn_run, RunEvent, RunId, RunInput, Seams};
use makepad_flow::graph::evaluate;
use makepad_flow::RunState;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use support::{FakeChat, FakeHttp};

const FLOW: &str = r#"use mod.flow.*
let image = Image{prompt: "keep this alive" model: "fake" width: 8 height: 8}
let result = Output{type: @image value: image.image()}
Flow{image, result}
"#;

#[derive(Default)]
struct TimedState {
    started: Mutex<Option<Instant>>,
    keepalives: Mutex<Vec<Instant>>,
    done_returned: AtomicBool,
    keepalives_after_done: AtomicUsize,
    bye_calls: AtomicUsize,
}

#[derive(Clone)]
struct TimedGen {
    state: Arc<TimedState>,
    duration: Duration,
    fail_keepalive: bool,
}

impl GenSeam for TimedGen {
    fn pick(&self, _domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        Ok(Box::new(TimedProvider {
            state: self.state.clone(),
            duration: self.duration,
            fail_keepalive: self.fail_keepalive,
            bytes: b"fake-png".to_vec(),
        }))
    }
}

struct TimedProvider {
    state: Arc<TimedState>,
    duration: Duration,
    fail_keepalive: bool,
    bytes: Vec<u8>,
}

impl ContentProvider for TimedProvider {
    fn health(&self) -> Result<HealthJson, AssetAiError> {
        Err(AssetAiError::Unavailable("unused".to_string()))
    }

    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> {
        Err(AssetAiError::Unavailable("unused".to_string()))
    }

    fn request(
        &self,
        _domain: Domain,
        _request: &GenerateRequestJson,
    ) -> Result<String, AssetAiError> {
        *self.state.started.lock().unwrap() = Some(Instant::now());
        Ok("timed-job".to_string())
    }

    fn poll(&self, _job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        let started = self.state.started.lock().unwrap().expect("job start");
        if started.elapsed() >= self.duration {
            self.state.done_returned.store(true, Ordering::SeqCst);
            Ok(status(JOB_STATE_DONE, &self.bytes))
        } else {
            Ok(status(JOB_STATE_RUNNING, &self.bytes))
        }
    }

    fn fetch_artifact(&self, _artifact_id: &str) -> Result<ArtifactBytes, AssetAiError> {
        Ok(ArtifactBytes {
            content_type: "image/png".to_string(),
            bytes: self.bytes.clone(),
        })
    }

    fn cancel(&self, _job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        Ok(status(JOB_STATE_CANCELLED, &self.bytes))
    }

    fn keepalive(&self, _job_id: &str) -> Result<(), AssetAiError> {
        if self.state.done_returned.load(Ordering::SeqCst) {
            self.state
                .keepalives_after_done
                .fetch_add(1, Ordering::SeqCst);
        }
        self.state.keepalives.lock().unwrap().push(Instant::now());
        if self.fail_keepalive {
            Err(AssetAiError::Http("injected keepalive failure".to_string()))
        } else {
            Ok(())
        }
    }

    fn bye(&self) -> Result<(), AssetAiError> {
        self.state.bye_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn status(state: &str, bytes: &[u8]) -> JobStatusJson {
    JobStatusJson {
        job_id: "timed-job".to_string(),
        state: state.to_string(),
        stage: (state == JOB_STATE_RUNNING).then(|| "waiting".to_string()),
        progress: (state == JOB_STATE_RUNNING).then_some(0.5),
        artifacts: (state == JOB_STATE_DONE)
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
        error: None,
        model: Some("fake".to_string()),
        queued_ms: None,
        started_ms: None,
        finished_ms: None,
        log: None,
        partial_text: None,
        live: None,
        serving: None,
        text: None,
    }
}

fn seams(gen: TimedGen) -> Seams {
    Seams {
        chat: Arc::new(FakeChat::done("unused")),
        gen: Arc::new(gen),
        http: Arc::new(FakeHttp::json(200, "{}")),
    }
}

fn run(gen: TimedGen) -> Vec<RunEvent> {
    support::run(FLOW, seams(gen), None)
}

#[test]
fn a_five_second_job_is_kept_alive_until_done() {
    let state = Arc::new(TimedState::default());
    let events = run(TimedGen {
        state: state.clone(),
        duration: Duration::from_secs(5),
        fail_keepalive: false,
    });
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            state: RunState::Done,
            ..
        })
    ));

    let calls = state.keepalives.lock().unwrap();
    assert!(calls.len() >= 2, "keepalives: {calls:?}");
    let started = state.started.lock().unwrap().unwrap();
    assert!(calls[0].duration_since(started) <= Duration::from_millis(2200));
    for pair in calls.windows(2) {
        let gap = pair[1].duration_since(pair[0]);
        assert!(gap >= Duration::from_secs(1), "keepalive gap {gap:?}");
        assert!(gap <= Duration::from_millis(2200), "keepalive gap {gap:?}");
    }
    assert_eq!(state.keepalives_after_done.load(Ordering::SeqCst), 0);
    let count_at_done = calls.len();
    drop(calls);
    std::thread::sleep(Duration::from_millis(250));
    assert_eq!(state.keepalives.lock().unwrap().len(), count_at_done);
}

#[test]
fn a_keepalive_error_does_not_fail_a_job_that_finishes() {
    let state = Arc::new(TimedState::default());
    let events = run(TimedGen {
        state: state.clone(),
        duration: Duration::from_millis(2300),
        fail_keepalive: true,
    });
    assert!(!state.keepalives.lock().unwrap().is_empty());
    assert!(matches!(
        events.last(),
        Some(RunEvent::RunFinished {
            state: RunState::Done,
            ..
        })
    ));
}

#[test]
fn cancelling_a_run_sends_bye_once() {
    let state = Arc::new(TimedState::default());
    let mut graph = evaluate(FLOW, "keepalive-cancel.splash").unwrap();
    graph.revision = 1;
    let input = RunInput {
        run_id: RunId("keepalive-cancel".to_string()),
        instance: "keepalive-cancel".to_string(),
        source: FLOW.to_string(),
        file_name: "keepalive-cancel.splash".to_string(),
        graph_revision: 1,
        graph,
        inputs: BTreeMap::new(),
        outputs: None,
        origin: ("test-origin".to_string(), 7),
    };
    let (sender, receiver) = mpsc::channel();
    let handle = spawn_run(
        input,
        seams(TimedGen {
            state: state.clone(),
            duration: Duration::from_secs(60),
            fail_keepalive: false,
        }),
        sender,
    );
    let _ = support::receive_until(&receiver, |event| {
        matches!(event, RunEvent::NodeStarted { node } if node == "image")
    });
    handle.cancel.store(true, Ordering::SeqCst);
    handle.join.join().unwrap();
    assert_eq!(state.bye_calls.load(Ordering::SeqCst), 1);
}

struct TempRoot(PathBuf);

impl TempRoot {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "makepad-flow-keepalive-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn local_service_keepalive_is_accepted_by_the_real_testpattern_service() {
    let root = TempRoot::new();
    let service = start_service(ServiceConfig {
        host: "127.0.0.1".to_string(),
        port: 0,
        cache_dir: root.0.clone(),
        registry: Registry {
            models: vec![ModelSpec {
                id: "testpattern".to_string(),
                domain: Domain::Image,
                backend: "testpattern".to_string(),
                available: true,
                gated: false,
                vram_gb: Some(0.0),
                min_vram_gb: None,
                min_compute_cap: None,
                note: None,
                license: None,
                files: Vec::new(),
            }],
        },
        downloader: Downloader::new("http://127.0.0.1:1", None).unwrap(),
        peer: PeerOptions {
            serve: Some(false),
            sources: Some(Vec::new()),
            ..Default::default()
        },
        fleet: DEFAULT_FLEET.to_string(),
    });
    let handle = match service {
        Ok(handle) => handle,
        Err(error) if error.to_string().contains("Operation not permitted") => {
            eprintln!("skipping real hub service: loopback bind is forbidden by this sandbox");
            return;
        }
        Err(error) => panic!("start real hub service: {error}"),
    };
    let provider = LocalService::new(&format!("http://{}", handle.addr));
    std::mem::forget(handle);
    let request = GenerateRequestJson {
        model: "testpattern".to_string(),
        origin_key: Some("flow-keepalive-test".to_string()),
        origin_epoch: Some(42),
        prompt: Some("test".to_string()),
        width: Some(8),
        height: Some(8),
        delay_ms: Some(3000),
        ..Default::default()
    };
    let job = provider.request(Domain::Image, &request).unwrap();
    provider.keepalive(&job).unwrap();
    provider.bye().unwrap();
}
