use super::*;
use makepad_ai_hub::client::ArtifactBytes;
use makepad_ai_hub::lease::{LeaseTable, Origin};
use makepad_ai_hub::protocol::{
    ArtifactRefJson, ModelInfoJson, JOB_STATE_CANCELLED, JOB_STATE_DONE, JOB_STATE_QUEUED,
    JOB_STATE_RUNNING,
};
use std::collections::BTreeMap;

const PAUSED: &str = "waiting for local-use: gpu-counter-unavailable";
const BYTES: &[u8] = b"completed-image";

#[derive(Clone, Copy, Default)]
enum CancelMode {
    #[default]
    Confirmed,
    Running,
    Lost,
    Unconfirmed,
    Done,
}

#[derive(Default)]
struct Hub {
    attempts: Vec<GenerateRequestJson>,
    requests: Vec<GenerateRequestJson>,
    jobs: BTreeMap<String, JobStatusJson>,
    leases: LeaseTable,
    mode: CancelMode,
    cancels: usize,
    submit_error: Option<AssetAiError>,
    poll_error: Option<AssetAiError>,
}
struct FarmState {
    hubs: [Hub; 3],
    available: usize,
    routes: usize,
    automatic: bool,
}
#[derive(Clone)]
struct Farm(Arc<Mutex<FarmState>>);
impl Default for Farm {
    fn default() -> Self {
        Self(Arc::new(Mutex::new(FarmState {
            hubs: Default::default(),
            available: 3,
            routes: 0,
            automatic: false,
        })))
    }
}
impl GenSeam for Farm {
    fn pick(&self, _: &str) -> Result<Box<dyn ContentProvider>, String> {
        unreachable!()
    }
    fn use_idle_admission(&self, domain: &str, request: &GenerateRequestJson) -> bool {
        self.0.lock().unwrap().automatic && domain == "image" && request.queue_policy.is_none()
    }
    fn pick_for_request(
        &self,
        _: &str,
        request: &GenerateRequestJson,
        excluded: &[String],
    ) -> Result<GenPick, String> {
        let mut state = self.0.lock().unwrap();
        state.routes += 1;
        let index = (0..state.available)
            .find(|i| !excluded.contains(&format!("http://hub-{i}")))
            .ok_or("no alternate provider")?;
        Ok(GenPick {
            provider: Box::new(Provider {
                farm: self.clone(),
                index,
                origin: Mutex::new(None),
            }),
            base_url: format!("http://hub-{index}"),
            model: request.model.clone(),
            model_state: Some("ready".into()),
        })
    }
}
struct Provider {
    farm: Farm,
    index: usize,
    origin: Mutex<Option<Origin>>,
}
impl ContentProvider for Provider {
    fn health(&self) -> Result<HealthJson, AssetAiError> {
        unreachable!()
    }
    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> {
        unreachable!()
    }
    fn request(&self, _: Domain, request: &GenerateRequestJson) -> Result<String, AssetAiError> {
        let mut state = self.farm.0.lock().unwrap();
        let hub = &mut state.hubs[self.index];
        hub.attempts.push(request.clone());
        if let Some(error) = &hub.submit_error {
            return Err(error.clone());
        }
        // Simulate AIHub's atomic actor admission, with deliberately stale
        // routing snapshots above so every caller first chooses hub-0.
        if request.queue_policy.as_deref() == Some("reject")
            && hub
                .jobs
                .values()
                .any(|job| matches!(job.state.as_str(), JOB_STATE_QUEUED | JOB_STATE_RUNNING))
        {
            return Err(AssetAiError::Busy);
        }
        // Assert at the submission boundary: no old job under this exact
        // lease may still execute when any replacement is admitted.
        for hub in &state.hubs {
            for (i, previous) in hub.requests.iter().enumerate() {
                if previous.origin_key == request.origin_key {
                    assert!(
                        !matches!(
                            hub.jobs[&format!("job-{i}")].state.as_str(),
                            JOB_STATE_QUEUED | JOB_STATE_RUNNING
                        ),
                        "duplicate accepted execution"
                    );
                }
            }
        }
        let origin = Origin {
            node_key: request.origin_key.clone().unwrap(),
            epoch: request.origin_epoch.unwrap(),
        };
        *self.origin.lock().unwrap() = Some(origin.clone());
        let hub = &mut state.hubs[self.index];
        let job = format!("job-{}", hub.requests.len());
        hub.leases.register(&job, origin, 0);
        let mut status = JobStatusJson::deserialize_json(&format!(
            r#"{{"job_id":"{job}","state":"queued","artifacts":[]}}"#
        ))
        .unwrap();
        status.stage = Some(PAUSED.into());
        hub.requests.push(request.clone());
        hub.jobs.insert(job.clone(), status);
        Ok(job)
    }
    fn poll(&self, job: &str) -> Result<JobStatusJson, AssetAiError> {
        let state = self.farm.0.lock().unwrap();
        let hub = &state.hubs[self.index];
        if let Some(error) = &hub.poll_error {
            return Err(error.clone());
        }
        Ok(hub.jobs[job].clone())
    }
    fn cancel(&self, job: &str) -> Result<JobStatusJson, AssetAiError> {
        let mut state = self.farm.0.lock().unwrap();
        let hub = &mut state.hubs[self.index];
        hub.cancels += 1;
        match hub.mode {
            CancelMode::Confirmed => {
                hub.jobs.get_mut(job).unwrap().state = JOB_STATE_CANCELLED.into();
                hub.leases.release(job);
            }
            CancelMode::Running => {
                let status = hub.jobs.get_mut(job).unwrap();
                status.state = JOB_STATE_RUNNING.into();
                status.started_ms = Some(1);
            }
            CancelMode::Lost => {
                return Err(AssetAiError::Io(
                    "connection reset after cancel POST".into(),
                ))
            }
            CancelMode::Unconfirmed => {}
            CancelMode::Done => complete(hub, job),
        }
        Ok(hub.jobs[job].clone())
    }
    fn bye(&self) -> Result<(), AssetAiError> {
        let origin = self.origin.lock().unwrap().clone().unwrap();
        let mut state = self.farm.0.lock().unwrap();
        let hub = &mut state.hubs[self.index];
        for (job, _) in hub.leases.bye(&origin.node_key) {
            hub.jobs.get_mut(&job).unwrap().state = JOB_STATE_CANCELLED.into();
        }
        Ok(())
    }
    fn fetch_artifact(&self, _: &str) -> Result<ArtifactBytes, AssetAiError> {
        Ok(ArtifactBytes {
            content_type: "image/png".into(),
            bytes: BYTES.to_vec(),
        })
    }
}
fn complete(hub: &mut Hub, job: &str) {
    let status = hub.jobs.get_mut(job).unwrap();
    status.state = JOB_STATE_DONE.into();
    status.artifacts = vec![ArtifactRefJson {
        id: "image".into(),
        url: "/artifact/image".into(),
        content_type: "image/png".into(),
        sha256: Some(makepad_ai_hub::sha256::sha256_hex(BYTES)),
        byte_len: Some(BYTES.len() as u64),
    }];
    hub.leases.release(job);
}
fn begin(farm: &Farm) -> (GenExecutor, Instant) {
    let graph = crate::graph::evaluate("use mod.flow.*\nlet image = Image{model: \"flux1-schnell\" prompt: \"old car\" width: 256 height: 512 steps: 4 seed: 77}\nFlow{image}\n", "queue.splash").unwrap();
    let mut executor = GenExecutor::new(Arc::new(farm.clone()), ("same-flow-host".into(), 9));
    executor
        .start(
            graph.nodes.iter().find(|node| node.id == "image").unwrap(),
            &[],
        )
        .unwrap();
    (executor, Instant::now())
}
fn start_pause(executor: &mut GenExecutor, now: Instant) {
    assert!(
        matches!(executor.poll_at(now), Poll::Progress { permille: 0, stage } if stage == PAUSED)
    );
}

#[test]
fn queued_stage_survives_absent_or_zero_progress_and_grace() {
    for progress in [None, Some(0.0)] {
        let farm = Farm::default();
        let (mut executor, now) = begin(&farm);
        farm.0.lock().unwrap().hubs[0]
            .jobs
            .get_mut("job-0")
            .unwrap()
            .progress = progress;
        start_pause(&mut executor, now);
        start_pause(&mut executor, now + Duration::from_secs(2));
        assert_eq!(farm.0.lock().unwrap().hubs[0].cancels, 0);
    }
}

#[test]
fn confirmed_queued_reroute_preserves_request_and_sibling_lease() {
    let farm = Farm::default();
    let (mut executor, now) = begin(&farm);
    let (mut sibling, _) = begin(&farm);
    start_pause(&mut executor, now);
    assert!(
        matches!(executor.poll_at(now + QUEUED_PAUSE_GRACE), Poll::Progress { stage, .. } if stage.contains("retrying on hub-1"))
    );
    {
        let mut state = farm.0.lock().unwrap();
        assert_eq!(state.hubs[0].jobs["job-0"].state, JOB_STATE_CANCELLED);
        assert_eq!(state.hubs[0].jobs["job-1"].state, JOB_STATE_QUEUED);
        assert_eq!(
            state.hubs[0].requests[0].serialize_json(),
            state.hubs[1].requests[0].serialize_json()
        );
        assert_ne!(
            state.hubs[0].requests[0].origin_key,
            state.hubs[0].requests[1].origin_key
        );
        complete(&mut state.hubs[1], "job-0");
        complete(&mut state.hubs[0], "job-1");
    }
    assert!(matches!(
        executor.poll_at(now + Duration::from_secs(4)),
        Poll::Done(_)
    ));
    assert!(matches!(
        sibling.poll_at(now + Duration::from_secs(4)),
        Poll::Done(_)
    ));
}

#[test]
fn running_race_or_lost_cancel_response_waits_for_terminal_confirmation() {
    for mode in [CancelMode::Running, CancelMode::Lost] {
        let farm = Farm::default();
        let (mut executor, now) = begin(&farm);
        farm.0.lock().unwrap().hubs[0].mode = mode;
        start_pause(&mut executor, now);
        for seconds in [3, 4] {
            assert!(
                matches!(executor.poll_at(now + Duration::from_secs(seconds)), Poll::Progress { stage, .. } if stage.contains("confirming cancellation"))
            );
            assert!(farm.0.lock().unwrap().hubs[1].requests.is_empty());
        }
        farm.0.lock().unwrap().hubs[0]
            .jobs
            .get_mut("job-0")
            .unwrap()
            .state = JOB_STATE_CANCELLED.into();
        assert!(
            matches!(executor.poll_at(now + Duration::from_secs(5)), Poll::Progress { stage, .. } if stage.contains("retrying on hub-1"))
        );
        assert_eq!(farm.0.lock().unwrap().hubs[1].requests.len(), 1);
    }
}

#[test]
fn ambiguous_cancel_times_out_without_submitting_a_replacement() {
    let farm = Farm::default();
    let (mut executor, now) = begin(&farm);
    farm.0.lock().unwrap().hubs[0].mode = CancelMode::Unconfirmed;
    start_pause(&mut executor, now);
    executor.poll_at(now + QUEUED_PAUSE_GRACE);
    assert!(
        matches!(executor.poll_at(now + QUEUED_PAUSE_GRACE + QUEUED_CANCEL_WINDOW), Poll::Failed(error) if error.contains("no replacement job submitted"))
    );
    assert!(farm.0.lock().unwrap().hubs[1].requests.is_empty());
}

#[test]
fn completed_during_cancel_keeps_original_result_without_reroute() {
    let farm = Farm::default();
    let (mut executor, now) = begin(&farm);
    farm.0.lock().unwrap().hubs[0].mode = CancelMode::Done;
    start_pause(&mut executor, now);
    assert!(matches!(
        executor.poll_at(now + QUEUED_PAUSE_GRACE),
        Poll::Done(_)
    ));
    assert!(farm.0.lock().unwrap().hubs[1].requests.is_empty());
}

#[test]
fn no_alternate_retains_job_and_probes_are_bounded() {
    let farm = Farm::default();
    let (mut executor, now) = begin(&farm);
    farm.0.lock().unwrap().available = 1;
    start_pause(&mut executor, now);
    assert!(
        matches!(executor.poll_at(now + QUEUED_PAUSE_GRACE), Poll::Progress { stage, .. } if stage.contains("available alternate"))
    );
    for tick in 31..60 {
        executor.poll_at(now + Duration::from_millis(tick * 100));
    }
    let state = farm.0.lock().unwrap();
    assert_eq!(state.routes, 2);
    assert_eq!(state.hubs[0].cancels, 0);
    assert_eq!(state.hubs[0].jobs["job-0"].state, JOB_STATE_QUEUED);
}

#[test]
fn normal_queue_running_jobs_and_partial_output_never_start_failover() {
    for case in 0..4 {
        let farm = Farm::default();
        let (mut executor, now) = begin(&farm);
        {
            let mut state = farm.0.lock().unwrap();
            let row = state.hubs[0].jobs.get_mut("job-0").unwrap();
            match case {
                0 => row.stage = Some("queued for worker".into()),
                1 => row.state = JOB_STATE_RUNNING.into(),
                2 => row.started_ms = Some(1),
                _ => row.partial_text = Some("already visible".into()),
            }
        }
        executor.poll_at(now);
        executor.poll_at(now + Duration::from_secs(30));
        assert_eq!(farm.0.lock().unwrap().hubs[0].cancels, 0);
        assert!(farm.0.lock().unwrap().hubs[1].requests.is_empty());
    }
}

#[test]
fn repeated_pause_reroutes_stop_at_three_submissions() {
    let farm = Farm::default();
    let (mut executor, now) = begin(&farm);
    for seconds in [0, 3, 4, 7, 8, 11, 20] {
        executor.poll_at(now + Duration::from_secs(seconds));
    }
    let state = farm.0.lock().unwrap();
    assert_eq!(
        state
            .hubs
            .iter()
            .map(|hub| hub.requests.len())
            .sum::<usize>(),
        3
    );
    assert_eq!(
        state.hubs[2].cancels, 0,
        "retain last accepted job at retry limit"
    );
}

#[test]
fn concurrent_images_spread_despite_identical_stale_node_picks() {
    let farm = Farm::default();
    farm.0.lock().unwrap().automatic = true;
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let threads: Vec<_> = (0..3)
        .map(|_| {
            let farm = farm.clone();
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                begin(&farm).0.attempts
            })
        })
        .collect();
    let attempts: Vec<_> = threads
        .into_iter()
        .map(|thread| thread.join().unwrap())
        .collect();
    let state = farm.0.lock().unwrap();
    let mut origins = std::collections::HashSet::new();
    for hub in &state.hubs {
        assert_eq!(hub.requests.len(), 1, "each concept gets a separate GPU");
        let request = &hub.requests[0];
        assert_eq!(request.queue_policy.as_deref(), Some("reject"));
        assert_eq!(request.seed, Some(77));
        assert_eq!(request.model, "flux1-schnell");
        assert!(origins.insert(request.origin_key.clone()));
    }
    assert_eq!(
        state
            .hubs
            .iter()
            .map(|hub| hub.attempts.len())
            .sum::<usize>(),
        6
    );
    assert!(
        attempts.iter().all(|attempts| *attempts == 1),
        "busy refusals create no jobs"
    );
}

#[test]
fn one_busy_healthy_node_deliberately_queues_after_idle_admission_refusal() {
    let farm = Farm::default();
    let (_first, _) = begin(&farm);
    {
        let mut state = farm.0.lock().unwrap();
        state.available = 1;
        state.automatic = true;
    }
    let (mut queued, _) = begin(&farm);
    assert!(
        matches!(queued.poll(), Poll::Progress { stage, permille: 0 } if stage.contains("all eligible GPUs busy; queued"))
    );
    let state = farm.0.lock().unwrap();
    assert_eq!(state.hubs[0].requests.len(), 2);
    assert_eq!(
        state.hubs[0].attempts[1].queue_policy.as_deref(),
        Some("reject")
    );
    assert_eq!(
        state.hubs[0].attempts[2].queue_policy.as_deref(),
        Some("queue")
    );
    assert!(
        queued.request.as_ref().unwrap().queue_policy.is_none(),
        "preserve implicit caller policy for later retries"
    );
}

#[test]
fn explicit_policy_and_ambiguous_post_failure_do_not_gain_automatic_retries() {
    for policy in ["queue", "reject"] {
        let farm = Farm::default();
        let (_first, _) = begin(&farm);
        farm.0.lock().unwrap().automatic = true;
        let graph = crate::graph::evaluate(&format!("use mod.flow.*\nlet image = Image{{model: \"flux1-schnell\" queue_policy: \"{policy}\"}}\nFlow{{image}}\n"), "explicit.splash").unwrap();
        let mut executor = GenExecutor::new(Arc::new(farm.clone()), ("explicit".into(), 9));
        let result = executor.start(
            graph.nodes.iter().find(|node| node.id == "image").unwrap(),
            &[(
                "request".into(),
                Value::json(format!("{{\"queue_policy\":\"{policy}\"}}")),
            )],
        );
        assert_eq!(result.is_ok(), policy == "queue");
        let state = farm.0.lock().unwrap();
        assert_eq!(
            state.hubs[0].attempts[1].queue_policy.as_deref(),
            Some(policy)
        );
        assert!(state.hubs[1].attempts.is_empty());
    }
    let farm = Farm::default();
    {
        let mut state = farm.0.lock().unwrap();
        state.automatic = true;
        state.hubs[0].submit_error = Some(AssetAiError::Http(
            "POST response timed out; job acceptance unknown".into(),
        ));
    }
    let graph = crate::graph::evaluate(
        "use mod.flow.*\nlet image = Image{model: \"flux1-schnell\"}\nFlow{image}\n",
        "ambiguous.splash",
    )
    .unwrap();
    let mut executor = GenExecutor::new(Arc::new(farm.clone()), ("ambiguous".into(), 9));
    assert!(executor
        .start(
            graph.nodes.iter().find(|node| node.id == "image").unwrap(),
            &[]
        )
        .is_err());
    let state = farm.0.lock().unwrap();
    assert_eq!(state.hubs[0].attempts.len(), 1);
    assert!(state.hubs[1].attempts.is_empty());
}

#[test]
fn proven_local_use_refusal_is_excluded_even_from_busy_queue_fallback() {
    for all_busy in [false, true] {
        let farm = Farm::default();
        {
            let mut state = farm.0.lock().unwrap();
            state.automatic = true;
            state.available = 2;
            state.hubs[0].submit_error = Some(AssetAiError::Unavailable(
                "local-use: gpu-counter-unavailable".into(),
            ));
            if all_busy {
                let row = JobStatusJson::deserialize_json(
                    r#"{"job_id":"other-user-job","state":"running","artifacts":[]}"#,
                )
                .unwrap();
                state.hubs[1].jobs.insert(row.job_id.clone(), row);
            }
        }
        let (executor, _) = begin(&farm);
        let state = farm.0.lock().unwrap();
        assert_eq!(executor.provider_url.as_deref(), Some("http://hub-1"));
        assert_eq!(
            state.hubs[0].attempts.len(),
            1,
            "paused node cannot become the queue fallback"
        );
        assert!(state.hubs[0].requests.is_empty());
        assert_eq!(
            state.hubs[1].requests[0].queue_policy.as_deref(),
            Some(if all_busy { "queue" } else { "reject" })
        );
    }
}

#[test]
fn proven_pre_admission_overload_reroutes_but_similar_http_prose_does_not() {
    const REASON: &str = "admission-overloaded: HTTP server overloaded before job admission";
    for proven in [true, false] {
        let farm = Farm::default();
        {
            let mut state = farm.0.lock().unwrap();
            state.automatic = true;
            state.hubs[0].submit_error = Some(if proven {
                AssetAiError::Unavailable(REASON.into())
            } else {
                AssetAiError::Http(format!("http 503: {REASON}"))
            });
        }
        let graph = crate::graph::evaluate(
            "use mod.flow.*\nlet image = Image{model: \"flux1-schnell\" prompt: \"old car\" seed: 77}\nFlow{image}\n",
            "admission-overload.splash",
        ).unwrap();
        let mut executor = GenExecutor::new(Arc::new(farm.clone()), ("overload".into(), 9));
        let result = executor.start(graph.nodes.iter().find(|node| node.id == "image").unwrap(), &[]);
        assert_eq!(result.is_ok(), proven);
        let state = farm.0.lock().unwrap();
        assert_eq!(state.hubs[0].attempts.len(), 1);
        assert!(state.hubs[0].requests.is_empty());
        assert_eq!(state.hubs[1].requests.len(), usize::from(proven));
        if proven {
            assert_eq!(state.hubs[0].attempts[0].serialize_json(), state.hubs[1].requests[0].serialize_json());
            assert!(executor.refusals[0].1.contains("admission-overloaded:"));
        }
    }
}

#[test]
fn transport_errors_mentioning_vram_never_resubmit_accepted_or_ambiguous_work() {
    for polling in [false, true] {
        let farm = Farm::default();
        let error =
            AssetAiError::Http("http 500: insufficient VRAM; job acceptance unknown".into());
        if polling {
            let (mut executor, now) = begin(&farm);
            farm.0.lock().unwrap().hubs[0].poll_error = Some(error);
            assert!(matches!(executor.poll_at(now), Poll::Failed(_)));
        } else {
            farm.0.lock().unwrap().hubs[0].submit_error = Some(error);
            let graph = crate::graph::evaluate(
                "use mod.flow.*\nlet image = Image{model: \"flux1-schnell\"}\nFlow{image}\n",
                "ambiguous-vram.splash",
            )
            .unwrap();
            let mut executor =
                GenExecutor::new(Arc::new(farm.clone()), ("ambiguous-vram".into(), 9));
            assert!(executor
                .start(
                    graph.nodes.iter().find(|node| node.id == "image").unwrap(),
                    &[]
                )
                .is_err());
        }
        let state = farm.0.lock().unwrap();
        assert_eq!(state.hubs[0].attempts.len(), 1);
        assert!(state.hubs[1].attempts.is_empty());
        assert_eq!(state.hubs[0].cancels, 0);
    }
}
