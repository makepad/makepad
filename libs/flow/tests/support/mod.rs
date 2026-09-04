#![allow(dead_code)]

use makepad_ai_hub::client::{ArtifactBytes, ContentProvider};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::protocol::{
    ArtifactRefJson, GenerateRequestJson, HealthJson, JobStatusJson, ModelInfoJson,
    JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_ERROR, JOB_STATE_RUNNING,
};
use makepad_ai_hub::registry::Domain;
use makepad_flow::engine::executors::chat::{ChatEvent, ChatSeam, ChatTurn};
use makepad_flow::engine::executors::gen::GenSeam;
use makepad_flow::engine::executors::http::{HttpReq, HttpResp, HttpSeam};
use makepad_flow::engine::{spawn_run, RunEvent, RunId, RunInput, Seams};
use makepad_flow::graph::evaluate;
use makepad_flow::{Graph, Value};
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct FakeChat {
    pub starts: Arc<Mutex<Vec<(Instant, String)>>>,
    pub requests: Arc<Mutex<Vec<(String, String, String, Option<u32>, Option<bool>)>>>,
    pub response: String,
    pub pending: bool,
    pub cancelled: Arc<AtomicBool>,
}

impl FakeChat {
    pub fn done(response: &str) -> Self {
        Self {
            starts: Arc::new(Mutex::new(Vec::new())),
            requests: Arc::new(Mutex::new(Vec::new())),
            response: response.to_string(),
            pending: false,
            cancelled: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl ChatSeam for FakeChat {
    fn start_turn(
        &self,
        system: &str,
        prompt: &str,
        model: &str,
        max_tokens: Option<u32>,
        thinking: Option<bool>,
    ) -> Result<Box<dyn ChatTurn>, String> {
        self.starts
            .lock()
            .unwrap()
            .push((Instant::now(), prompt.to_string()));
        self.requests.lock().unwrap().push((
            system.to_string(),
            prompt.to_string(),
            model.to_string(),
            max_tokens,
            thinking,
        ));
        Ok(Box::new(FakeTurn {
            response: self.response.clone(),
            step: 0,
            pending: self.pending,
            cancelled: self.cancelled.clone(),
        }))
    }
}

struct FakeTurn {
    response: String,
    step: usize,
    pending: bool,
    cancelled: Arc<AtomicBool>,
}

impl ChatTurn for FakeTurn {
    fn poll(&mut self) -> Vec<ChatEvent> {
        if self.pending {
            return Vec::new();
        }
        self.step += 1;
        match self.step {
            1 => vec![ChatEvent::Delta(self.response.clone())],
            2 => vec![ChatEvent::Done {
                text: self.response.clone(),
            }],
            _ => Vec::new(),
        }
    }

    fn cancel(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy)]
pub enum GenMode {
    Done,
    Fail,
    Pending,
}

#[derive(Clone)]
pub struct FakeGen {
    pub mode: GenMode,
    pub starts: Arc<Mutex<Vec<(Instant, String)>>>,
    pub cancelled: Arc<AtomicBool>,
    pub bytes: Vec<u8>,
    pub origins: Arc<Mutex<Vec<(Option<String>, Option<u64>)>>>,
    pub requests: Arc<Mutex<Vec<(Domain, GenerateRequestJson)>>>,
}

impl FakeGen {
    pub fn done() -> Self {
        Self {
            mode: GenMode::Done,
            starts: Arc::new(Mutex::new(Vec::new())),
            cancelled: Arc::new(AtomicBool::new(false)),
            bytes: b"fake-png".to_vec(),
            origins: Arc::new(Mutex::new(Vec::new())),
            requests: Arc::new(Mutex::new(Vec::new())),
        }
    }
}

impl GenSeam for FakeGen {
    fn pick(&self, _domain: &str) -> Result<Box<dyn ContentProvider>, String> {
        Ok(Box::new(FakeProvider {
            mode: self.mode,
            starts: self.starts.clone(),
            cancelled: self.cancelled.clone(),
            polls: AtomicUsize::new(0),
            bytes: self.bytes.clone(),
            origins: self.origins.clone(),
            requests: self.requests.clone(),
        }))
    }
}

struct FakeProvider {
    mode: GenMode,
    starts: Arc<Mutex<Vec<(Instant, String)>>>,
    cancelled: Arc<AtomicBool>,
    polls: AtomicUsize,
    bytes: Vec<u8>,
    origins: Arc<Mutex<Vec<(Option<String>, Option<u64>)>>>,
    requests: Arc<Mutex<Vec<(Domain, GenerateRequestJson)>>>,
}

impl ContentProvider for FakeProvider {
    fn health(&self) -> Result<HealthJson, AssetAiError> {
        Err(AssetAiError::Unavailable("not used".to_string()))
    }

    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> {
        Err(AssetAiError::Unavailable("not used".to_string()))
    }

    fn request(
        &self,
        domain: Domain,
        request: &GenerateRequestJson,
    ) -> Result<String, AssetAiError> {
        self.starts.lock().unwrap().push((
            Instant::now(),
            request.prompt.clone().unwrap_or_default(),
        ));
        self.origins
            .lock()
            .unwrap()
            .push((request.origin_key.clone(), request.origin_epoch));
        self.requests.lock().unwrap().push((domain, request.clone()));
        Ok("fake-job".to_string())
    }

    fn poll(&self, _job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        let poll = self.polls.fetch_add(1, Ordering::Relaxed);
        Ok(match self.mode {
            GenMode::Done if poll == 0 => status(JOB_STATE_RUNNING, None, &self.bytes),
            GenMode::Done => status(JOB_STATE_DONE, None, &self.bytes),
            GenMode::Fail => status(JOB_STATE_ERROR, Some("fake gen failed"), &self.bytes),
            GenMode::Pending => status(JOB_STATE_RUNNING, None, &self.bytes),
        })
    }

    fn fetch_artifact(&self, _artifact_id: &str) -> Result<ArtifactBytes, AssetAiError> {
        Ok(ArtifactBytes {
            content_type: "image/png".to_string(),
            bytes: self.bytes.clone(),
        })
    }

    fn cancel(&self, _job_id: &str) -> Result<JobStatusJson, AssetAiError> {
        self.cancelled.store(true, Ordering::Relaxed);
        Ok(status(JOB_STATE_CANCELLED, None, &self.bytes))
    }
}

fn status(state: &str, error: Option<&str>, bytes: &[u8]) -> JobStatusJson {
    JobStatusJson {
        job_id: "fake-job".to_string(),
        state: state.to_string(),
        stage: (state == JOB_STATE_RUNNING).then(|| "paint".to_string()),
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
        error: error.map(str::to_string),
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

#[derive(Clone)]
pub struct FakeHttp {
    pub response: HttpResp,
    pub calls: Arc<AtomicUsize>,
}

impl FakeHttp {
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            response: HttpResp {
                status,
                headers: vec![("content-type".to_string(), "application/json".to_string())],
                body: body.as_bytes().to_vec(),
            },
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }
}

impl HttpSeam for FakeHttp {
    fn request(&self, _req: HttpReq) -> Result<HttpResp, String> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(self.response.clone())
    }
}

pub fn seams(chat: FakeChat, gen: FakeGen, http: FakeHttp) -> Seams {
    Seams {
        chat: Arc::new(chat),
        gen: Arc::new(gen),
        http: Arc::new(http),
    }
}

pub fn run(source: &str, seams: Seams, outputs: Option<Vec<String>>) -> Vec<RunEvent> {
    let graph = evaluate(source, "engine.splash").unwrap();
    run_graph(source, graph, seams, outputs, BTreeMap::new())
}

pub fn run_graph(
    source: &str,
    mut graph: Graph,
    seams: Seams,
    outputs: Option<Vec<String>>,
    inputs: BTreeMap<String, BTreeMap<String, Value>>,
) -> Vec<RunEvent> {
    graph.revision = 4;
    let input = RunInput {
        run_id: RunId("run_test".to_string()),
        instance: "inst_test".to_string(),
        source: source.to_string(),
        file_name: "engine.splash".to_string(),
        graph_revision: 4,
        graph,
        inputs,
        outputs,
        origin: ("test-origin".to_string(), 9),
    };
    let (sender, receiver) = mpsc::channel();
    let handle = spawn_run(input, seams, sender);
    handle.join.join().unwrap();
    receiver.try_iter().collect()
}

pub fn receive_until(
    receiver: &mpsc::Receiver<RunEvent>,
    predicate: impl Fn(&RunEvent) -> bool,
) -> Vec<RunEvent> {
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut events = Vec::new();
    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                let found = predicate(&event);
                events.push(event);
                if found {
                    return events;
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(error) => panic!("event channel failed: {error}"),
        }
        assert!(Instant::now() < deadline, "event deadline elapsed");
    }
}

pub fn event_name(event: &RunEvent) -> String {
    match event {
        RunEvent::RunStarted { .. } => "run.started".to_string(),
        RunEvent::NodeStarted { node } => format!("node.started:{node}"),
        RunEvent::NodeProgress { node, .. } => format!("node.progress:{node}"),
        RunEvent::NodeDelta { node, .. } => format!("node.delta:{node}"),
        RunEvent::NodeWaiting { node, .. } => format!("node.waiting:{node}"),
        RunEvent::NodeAnswered { node, .. } => format!("node.answered:{node}"),
        RunEvent::NodeDone { node, .. } => format!("node.done:{node}"),
        RunEvent::NodeFailed { node, .. } => format!("node.failed:{node}"),
        RunEvent::NodeSkipped { node, reason } => format!("node.skipped:{node}:{reason}"),
        RunEvent::RunFinished { state, .. } => format!("run.finished:{state:?}"),
    }
}

pub fn output<'a>(events: &'a [RunEvent], name: &str) -> &'a Value {
    events
        .iter()
        .find_map(|event| match event {
            RunEvent::RunFinished { outputs, .. } => outputs
                .iter()
                .find_map(|(node, value)| (node == name).then_some(value)),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing output {name}: {events:#?}"))
}
