//! GenJob: routing, leases, admission retries, queued reroutes and
//! transport recovery against scripted providers (no network).
use super::*;
use crate::client::ArtifactBytes;
use crate::lease::{LeaseTable, Origin};
use crate::protocol::{ArtifactRefJson, ModelInfoJson};
use makepad_micro_serde::{DeJson, SerJson};
use std::collections::BTreeMap;

mod queued {
use super::*;

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
impl GenRouter for Farm {
    fn use_idle_admission(&self, domain: &str, request: &GenerateRequestJson) -> bool {
        self.0.lock().unwrap().automatic && domain == "image" && request.queue_policy.is_none()
    }
    fn route(
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
        sha256: Some(crate::sha256::sha256_hex(BYTES)),
        byte_len: Some(BYTES.len() as u64),
    }];
    hub.leases.release(job);
}
fn image(prompt: Option<&str>, seed: Option<u64>) -> GenerateRequestJson {
    GenerateRequestJson { model: "flux1-schnell".into(), prompt: prompt.map(Into::into), seed, ..Default::default() }
}
fn begin(farm: &Farm) -> (GenJob, Instant) {
    let request = GenerateRequestJson { width: Some(256), height: Some(512), steps: Some(4), ..image(Some("old car"), Some(77)) };
    let mut executor = GenJob::new(Arc::new(farm.clone()), ("same-flow-host".into(), 9));
    executor.start(Domain::Image, request, Want::All).unwrap();
    (executor, Instant::now())
}
fn start_pause(executor: &mut GenJob, now: Instant) {
    assert!(
        matches!(executor.poll_at(now), JobPoll::Progress { permille: 0, stage } if stage == PAUSED)
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
        matches!(executor.poll_at(now + QUEUED_PAUSE_GRACE), JobPoll::Progress { stage, .. } if stage.contains("retrying on hub-1"))
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
        JobPoll::Done(_)
    ));
    assert!(matches!(
        sibling.poll_at(now + Duration::from_secs(4)),
        JobPoll::Done(_)
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
                matches!(executor.poll_at(now + Duration::from_secs(seconds)), JobPoll::Progress { stage, .. } if stage.contains("confirming cancellation"))
            );
            assert!(farm.0.lock().unwrap().hubs[1].requests.is_empty());
        }
        farm.0.lock().unwrap().hubs[0]
            .jobs
            .get_mut("job-0")
            .unwrap()
            .state = JOB_STATE_CANCELLED.into();
        assert!(
            matches!(executor.poll_at(now + Duration::from_secs(5)), JobPoll::Progress { stage, .. } if stage.contains("retrying on hub-1"))
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
        matches!(executor.poll_at(now + QUEUED_PAUSE_GRACE + QUEUED_CANCEL_WINDOW), JobPoll::Failed(error) if error.contains("no replacement job submitted"))
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
        JobPoll::Done(_)
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
        matches!(executor.poll_at(now + QUEUED_PAUSE_GRACE), JobPoll::Progress { stage, .. } if stage.contains("available alternate"))
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
        matches!(queued.poll(), JobPoll::Progress { stage, permille: 0 } if stage.contains("all eligible GPUs busy; queued"))
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
        let mut executor = GenJob::new(Arc::new(farm.clone()), ("explicit".into(), 9));
        let result = executor.start(Domain::Image, GenerateRequestJson { queue_policy: Some(policy.into()), ..image(None, None) }, Want::All);
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
    let mut executor = GenJob::new(Arc::new(farm.clone()), ("ambiguous".into(), 9));
    assert!(executor.start(Domain::Image, image(None, None), Want::All).is_err());
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
        let mut executor = GenJob::new(Arc::new(farm.clone()), ("overload".into(), 9));
        let result = executor.start(Domain::Image, image(Some("old car"), Some(77)), Want::All);
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
            assert!(matches!(executor.poll_at(now), JobPoll::Failed(_)));
        } else {
            farm.0.lock().unwrap().hubs[0].submit_error = Some(error);
            let mut executor = GenJob::new(Arc::new(farm.clone()), ("ambiguous-vram".into(), 9));
            assert!(executor.start(Domain::Image, image(None, None), Want::All).is_err());
        }
        let state = farm.0.lock().unwrap();
        assert_eq!(state.hubs[0].attempts.len(), 1);
        assert!(state.hubs[1].attempts.is_empty());
        assert_eq!(state.hubs[0].cancels, 0);
    }
}

}

mod lease_protocol_tests {
    use super::*;
    use crate::protocol::ModelInfoJson;

    fn snapshot(host: &str, version: &str, state: &str) -> fleet::BoxSnapshot {
        let health = HealthJson::deserialize_json(&format!(
            r#"{{"service":"makepad-asset-ai","version":"{version}","models_loaded":[],"gpu":"NVIDIA GeForce RTX 4090","vram_free_mb":24576,"vram_total_mb":24576,"jobs_pending":0}}"#
        )).unwrap();
        let model = ModelInfoJson::deserialize_json(&format!(
            r#"{{"id":"flux1-schnell","domain":"image","backend":"flux","available":true,"gated":false,"vram_gb":8,"state":"{state}"}}"#
        )).unwrap();
        fleet::BoxSnapshot { base_url: format!("http://{host}:8123"), health: Some(health), models: vec![model] }
    }

    fn leased_request(model: &str) -> GenerateRequestJson {
        GenerateRequestJson {
            model: model.into(), origin_key: Some("flow-job-owner".into()), origin_epoch: Some(7),
            width: Some(768), height: Some(512), steps: Some(4), seed: Some(42), ..Default::default()
        }
    }

    #[test]
    fn implicit_images_prefer_eligible_idle_gpu_over_loaded_busy_gpu() {
        let mut busy = snapshot("busy", "0.3.0", "loaded");
        busy.health.as_mut().unwrap().jobs_pending = Some(1);
        let ready = snapshot("idle", "0.3.0", "ready");
        let request = leased_request("flux1-schnell");
        let picked = route_generation(vec![busy.clone(), ready], "image", &request).unwrap();
        assert_eq!(picked.0.base_url, "http://idle:8123");
        // A paused/unsupported idle machine must not hide an eligible busy
        // fallback, and a single healthy busy GPU remains routable.
        let mut unsupported = snapshot("unsupported", "0.3.0", "ready");
        unsupported.models.clear();
        assert_eq!(route_generation(vec![busy, unsupported], "image", &request).unwrap().0.base_url, "http://busy:8123");
        assert!(FleetRouter::default().use_idle_admission("image", &request));
        assert!(!FixedRouter("http://fixed".into()).use_idle_admission("image", &request));
        for policy in ["queue", "reject"] {
            let mut explicit = request.clone(); explicit.queue_policy = Some(policy.into());
            assert!(!FleetRouter::default().use_idle_admission("image", &explicit));
        }
    }

    #[test]
    fn leased_routes_skip_a_loaded_old_node_for_a_compatible_ready_node() {
        let nodes = vec![snapshot("10.0.0.235", "0.2.0", "loaded"), snapshot("10.0.0.123", "0.3.0", "ready")];
        // The old node would otherwise win on loaded-model affinity.
        let ordinary = GenerateRequestJson { model: "flux1-schnell".into(), ..Default::default() };
        assert_eq!(route_generation(nodes.clone(), "image", &ordinary).unwrap().0.base_url, "http://10.0.0.235:8123");
        for model in ["flux1-schnell", ""] {
            let request = leased_request(model);
            let (node, selected_model) = route_generation(nodes.clone(), "image", &request).unwrap();
            assert_eq!(node.base_url, "http://10.0.0.123:8123");
            assert_eq!(selected_model, "flux1-schnell");
            assert_eq!(request.seed, Some(42));
        }
    }

    #[test]
    fn an_old_only_fleet_reports_the_node_and_required_upgrade() {
        let error = route_generation(vec![snapshot("10.0.0.235", "0.2.0", "loaded")], "image", &leased_request("flux1-schnell")).unwrap_err();
        assert!(error.contains("10.0.0.235"), "{error}");
        assert!(error.contains("0.2.0"), "{error}");
        assert!(error.contains("upgrade AIHub to 0.3.0 or newer"), "{error}");
        assert!(error.contains("keepalive"), "{error}");
    }

    #[test]
    fn absent_or_unrecognized_health_is_not_assumed_to_support_leases() {
        let mut absent = snapshot("10.0.0.10", "0.3.0", "loaded");
        absent.health = None;
        let unknown = snapshot("10.0.0.11", "development", "loaded");
        let mut wrong_service = snapshot("10.0.0.12", "0.3.0", "loaded");
        wrong_service.health.as_mut().unwrap().service = "other-service".into();
        let error = route_generation(vec![absent, unknown, wrong_service], "image", &leased_request("flux1-schnell")).unwrap_err();
        assert!(error.contains("10.0.0.10:8123: cannot verify job leases") || error.contains("10.0.0.10: cannot verify job leases"), "{error}");
        assert!(error.contains("/health is unavailable"), "{error}");
        assert!(error.contains("development"), "{error}");
        assert!(error.contains("does not identify an AIHub service"), "{error}");
    }

    #[test]
    fn lease_version_floor_is_numeric_and_rejects_unknown_or_older_prereleases() {
        for version in ["0.3.0", "0.3.1", "0.10.0", "1.0.0", "0.3.0+farm.12"] {
            assert!(version_supports_leases(version), "rejected {version}");
        }
        for version in ["", "test", "0.2.999", "0.3", "0.3.0.0", "0.3.0-preview", "-1.3.0", "0.+3.0"] {
            assert!(!version_supports_leases(version), "accepted {version}");
        }
    }

    #[test]
    fn fixed_leased_generation_rejects_old_health_before_submitting_any_job() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request = Vec::new();
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let mut bytes = [0; 512];
                let count = stream.read(&mut bytes).unwrap();
                assert!(count > 0);
                request.extend_from_slice(&bytes[..count]);
            }
            let body = r#"{"service":"makepad-asset-ai","version":"0.2.0","models_loaded":[]}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            String::from_utf8(request).unwrap()
        });
        let result = FixedRouter(base_url).route("image", &leased_request("flux1-schnell"), &[]);
        let error = match result { Err(error) => error, Ok(_) => panic!("old fixed node was admitted") };
        assert!(error.contains("upgrade AIHub to 0.3.0 or newer"), "{error}");
        assert!(server.join().unwrap().starts_with("GET /health "));
    }
}


mod transport_tests {
    use super::*;
    use crate::protocol::{ArtifactRefJson, ModelInfoJson};
    use std::collections::VecDeque;

    #[derive(Default)]
    struct State {
        requests: Vec<GenerateRequestJson>,
        polls: usize,
        keepalives: usize,
        fetches: usize,
        cancels: usize,
        byes: usize,
        poll_errors: VecDeque<AssetAiError>,
        keepalive_errors: VecDeque<AssetAiError>,
        artifact_errors: VecDeque<AssetAiError>,
        always_poll_error: Option<AssetAiError>,
        always_keepalive_error: Option<AssetAiError>,
        done: bool,
    }
    #[derive(Clone, Default)]
    struct TestProvider(Arc<Mutex<State>>);
    impl GenRouter for TestProvider {
        fn route(&self, _: &str, request: &GenerateRequestJson, excluded: &[String]) -> Result<GenPick, String> {
            GenPick::only(|| Ok(Box::new(self.clone())), request, excluded)
        }
    }
    impl ContentProvider for TestProvider {
        fn health(&self) -> Result<HealthJson, AssetAiError> { unreachable!() }
        fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> { unreachable!() }
        fn request(&self, _: Domain, request: &GenerateRequestJson) -> Result<String, AssetAiError> {
            self.0.lock().unwrap().requests.push(request.clone());
            Ok("accepted-flux-job".into())
        }
        fn poll(&self, job: &str) -> Result<JobStatusJson, AssetAiError> {
            assert_eq!(job, "accepted-flux-job");
            let mut state = self.0.lock().unwrap();state.polls += 1;
            if let Some(error) = state.poll_errors.pop_front().or_else(|| state.always_poll_error.clone()) { return Err(error); }
            let bytes = b"verified-image";
            Ok(JobStatusJson {
                job_id: job.into(), state: if state.done { JOB_STATE_DONE } else { JOB_STATE_RUNNING }.into(),
                stage: Some("sampling".into()), progress: Some(0.375),
                artifacts: vec![ArtifactRefJson { id: "accepted-image".into(), url: "/artifact/accepted-image".into(), content_type: "image/png".into(), sha256: Some(crate::sha256::sha256_hex(bytes)), byte_len: Some(bytes.len() as u64) }],
                error: None, model: Some("flux1-schnell".into()), queued_ms: None, started_ms: None, finished_ms: None,
                log: None, partial_text: None, live: None, serving: None, text: None,
            })
        }
        fn fetch_artifact(&self, id: &str) -> Result<ArtifactBytes, AssetAiError> {
            assert_eq!(id, "accepted-image");
            let mut state = self.0.lock().unwrap();state.fetches += 1;
            if let Some(error) = state.artifact_errors.pop_front() { return Err(error); }
            Ok(ArtifactBytes { content_type: "image/png".into(), bytes: b"verified-image".to_vec() })
        }
        fn keepalive(&self, job: &str) -> Result<(), AssetAiError> {
            assert_eq!(job, "accepted-flux-job");
            let mut state = self.0.lock().unwrap();state.keepalives += 1;
            if let Some(error) = state.keepalive_errors.pop_front().or_else(|| state.always_keepalive_error.clone()) { return Err(error); }
            Ok(())
        }
        fn cancel(&self, job: &str) -> Result<JobStatusJson, AssetAiError> {
            assert_eq!(job, "accepted-flux-job");self.0.lock().unwrap().cancels += 1;Err(AssetAiError::Cancelled)
        }
        fn bye(&self) -> Result<(), AssetAiError> { self.0.lock().unwrap().byes += 1;Ok(()) }
    }
    fn begin(provider: &TestProvider) -> (GenJob, Instant) {
        let request = GenerateRequestJson { model: "flux1-schnell".into(), prompt: Some("old car".into()), width: Some(768), height: Some(512), steps: Some(4), seed: Some(42), ..Default::default() };
        let mut executor = GenJob::new(Arc::new(provider.clone()), ("transport-owner".into(), 71));
        executor.start(Domain::Image, request, Want::Outputs { media: 1, text: 0 }).unwrap();
        (executor, Instant::now())
    }
    fn http(status: u16) -> AssetAiError { AssetAiError::Http(format!("http://10.0.0.165:8123/job/accepted-flux-job: http {status}: temporarily busy")) }
    fn finish(executor: &mut GenJob, now: Instant) -> JobPoll {
        for tick in 0..110 {
            let result = executor.poll_at(now + Duration::from_millis(tick * 100));
            if matches!(result, JobPoll::Done(_) | JobPoll::Failed(_)) { return result; }
        }
        panic!("transport recovery exceeded its bounded window");
    }
    #[test]
    fn interrupted_status_and_artifact_reads_recover_without_resubmitting_the_job() {
        let provider = TestProvider::default();let (mut executor, now) = begin(&provider);
        assert!(matches!(executor.poll_at(now), JobPoll::Progress { permille: 375, .. }));
        {
            let mut state = provider.0.lock().unwrap();
            state.poll_errors.extend([http(503), http(502), AssetAiError::Io("Connection reset by peer (os error 54)".into())]);
            state.artifact_errors.push_back(http(504));state.done = true;
        }
        let JobPoll::Progress { permille, stage } = executor.poll_at(now + Duration::from_millis(10)) else { panic!("temporary error discarded the job") };
        assert_eq!(permille, 375);assert!(stage.contains("retaining the same generation job"));
        let JobPoll::Done(output) = finish(&mut executor, now + Duration::from_millis(20)) else { panic!("same-job recovery failed") };
        assert_eq!(output.artifacts[0].bytes, b"verified-image");
        assert_eq!(output.job_id, "accepted-flux-job");
        assert_eq!(output.seed, Some(42));
        let state = provider.0.lock().unwrap();assert_eq!(state.requests.len(), 1);assert_eq!(state.fetches, 2);
        assert_eq!(state.requests[0].model, "flux1-schnell");assert_eq!(state.requests[0].seed, Some(42));
        assert_eq!((state.cancels, state.byes), (0, 0));
    }
    #[test]
    fn persistent_status_failures_are_bounded_and_keep_the_lease_alive_until_cleanup() {
        let provider = TestProvider::default();provider.0.lock().unwrap().always_poll_error = Some(http(503));
        let (mut executor, now) = begin(&provider);
        let JobPoll::Failed(error) = finish(&mut executor, now) else { panic!("persistent transport error did not fail") };
        assert!(error.contains("status recovery exhausted after 6 failures"), "{error}");
        assert!(error.contains("job=accepted-flux-job"));
        let state = provider.0.lock().unwrap();assert_eq!(state.polls, 6);assert_eq!(state.requests.len(), 1);
        assert!(state.keepalives >= 1, "status backoff must not starve lease renewals");
        assert_eq!((state.cancels, state.byes), (1, 1));
    }
    #[test]
    fn successful_status_reads_do_not_reset_the_artifact_retry_budget() {
        let provider = TestProvider::default();
        {
            let mut state = provider.0.lock().unwrap();state.done = true;
            state.artifact_errors.extend((0..MAX_TRANSPORT_FAILURES).map(|_| http(503)));
        }
        let (mut executor, now) = begin(&provider);
        let JobPoll::Failed(error) = finish(&mut executor, now) else { panic!("artifact retries were not bounded") };
        assert!(error.contains("artifact recovery exhausted after 6 failures"), "{error}");
        let state = provider.0.lock().unwrap();assert_eq!((state.fetches, state.polls), (6, 6));assert_eq!(state.requests.len(), 1);
        assert_eq!((state.cancels, state.byes), (1, 1));
    }
    #[test]
    fn a_slow_status_recovery_expires_before_another_read_is_sent() {
        let provider = TestProvider::default();provider.0.lock().unwrap().always_poll_error = Some(http(503));
        let (mut executor, now) = begin(&provider);
        assert!(matches!(executor.poll_at(now), JobPoll::Progress { .. }));
        assert!(matches!(executor.poll_at(now + TRANSPORT_RECOVERY_WINDOW), JobPoll::Failed(_)));
        let state = provider.0.lock().unwrap();assert_eq!(state.polls, 1);assert_eq!((state.cancels, state.byes), (1, 1));
    }
    #[test]
    fn transient_keepalive_retries_promptly_without_restarting_generation() {
        let provider = TestProvider::default();provider.0.lock().unwrap().keepalive_errors.push_back(http(503));
        let (mut executor, now) = begin(&provider);
        let first = now + crate::lease::KEEPALIVE_INTERVAL;
        let JobPoll::Progress { stage, .. } = executor.poll_at(first) else { panic!("missing recovery status") };
        assert!(stage.contains("keepalive"));assert_eq!(provider.0.lock().unwrap().keepalives, 1);
        executor.poll_at(first + Duration::from_millis(100));assert_eq!(provider.0.lock().unwrap().keepalives, 1);
        executor.poll_at(first + Duration::from_millis(250));assert_eq!(provider.0.lock().unwrap().keepalives, 2);
        provider.0.lock().unwrap().done = true;
        assert!(matches!(executor.poll_at(first + Duration::from_millis(300)), JobPoll::Done(_)));
        let state = provider.0.lock().unwrap();assert_eq!(state.requests.len(), 1);assert_eq!((state.cancels, state.byes), (0, 0));
    }
    #[test]
    fn persistent_transient_keepalive_errors_have_their_own_bounded_budget() {
        let provider = TestProvider::default();provider.0.lock().unwrap().always_keepalive_error = Some(http(503));
        let (mut executor, now) = begin(&provider);
        let JobPoll::Failed(error) = finish(&mut executor, now) else { panic!("persistent renewal errors did not fail") };
        assert!(error.contains("keepalive recovery exhausted after 6 failures"), "{error}");
        let state = provider.0.lock().unwrap();assert_eq!(state.keepalives, 6);assert_eq!(state.requests.len(), 1);
        assert_eq!((state.cancels, state.byes), (1, 1));
    }
    #[test]
    fn a_finished_job_is_not_discarded_due_to_a_failed_lease_renewal() {
        let provider = TestProvider::default();
        {
            let mut state = provider.0.lock().unwrap();state.done = true;
            state.always_keepalive_error = Some(http(503));
        }
        let (mut executor, now) = begin(&provider);
        assert!(matches!(executor.poll_at(now + crate::lease::KEEPALIVE_INTERVAL), JobPoll::Done(_)));
        let state = provider.0.lock().unwrap();assert_eq!(state.keepalives, 1);assert_eq!(state.fetches, 1);
        assert_eq!((state.cancels, state.byes), (0, 0));
    }
    #[test]
    fn explicit_cancel_during_backoff_prevents_all_future_retries() {
        let provider = TestProvider::default();provider.0.lock().unwrap().always_poll_error = Some(http(503));
        let (mut executor, now) = begin(&provider);
        assert!(matches!(executor.poll_at(now), JobPoll::Progress { .. }));executor.cancel();
        for seconds in 1..10 { assert!(matches!(executor.poll_at(now + Duration::from_secs(seconds)), JobPoll::Pending)); }
        let state = provider.0.lock().unwrap();assert_eq!((state.polls, state.keepalives, state.fetches), (1, 0, 0));
        assert_eq!(state.requests.len(), 1);assert_eq!((state.cancels, state.byes), (1, 1));
    }
    #[test]
    fn permanent_statuses_corrupt_json_and_cancellation_are_not_retried() {
        for error in [http(400), http(401), http(404), AssetAiError::Http("http://node/job: http 400: upstream http 503 timeout".into()),
            AssetAiError::Http("http://node/job: bad response json: expected object (timeout)".into()),
            AssetAiError::Http("http://node/job: response is not utf-8".into()), AssetAiError::Cancelled,
            AssetAiError::Backend("HTTP 503 timeout".into())] {
            assert!(!transient_transport_error(&error), "{error}");
            let provider = TestProvider::default();provider.0.lock().unwrap().always_poll_error = Some(error);
            let (mut executor, now) = begin(&provider);assert!(matches!(executor.poll_at(now), JobPoll::Failed(_)));
            let state = provider.0.lock().unwrap();assert_eq!((state.requests.len(), state.polls), (1, 1));
        }
        for error in [http(502), http(503), http(504), AssetAiError::Http("Operation timed out (os error 60)".into()), AssetAiError::Io("Connection reset by peer".into())] {
            assert!(transient_transport_error(&error), "{error}");
        }
    }
}

mod tables {
    use super::*;
    use crate::registry::request_problem;

    fn offer(url: &str, models: &[(&str, &str)]) -> fleet::BoxSnapshot {
        let mut snapshot = fleet::BoxSnapshot::new(url);
        snapshot.models = models.iter().map(|(id, state)| ModelInfoJson::deserialize_json(&format!(
            r#"{{"id":"{id}","domain":"image","backend":"flux","available":true,"gated":false,"vram_gb":8,"state":"{state}"}}"#
        )).unwrap()).collect();
        snapshot
    }

    #[test]
    fn only_nodes_with_the_model_installed_are_routed_to() {
        let offers = vec![
            offer("http://a", &[("flux1-schnell", "ready")]),
            offer("http://b", &[("flux1-schnell", "loaded"), ("flux2-klein-4b", "ready")]),
            offer("http://c", &[("flux1-dev", "absent")]),
        ];
        assert_eq!(fleet::installed_model(&offers, "image", ""), Some(("flux1-schnell".to_string(), vec!["http://c".to_string()])));
        assert_eq!(fleet::installed_model(&offers, "image", "flux2-klein-4b"),
            Some(("flux2-klein-4b".to_string(), vec!["http://a".to_string(), "http://c".to_string()])));
        assert_eq!(fleet::installed_model(&offers, "image", "kokoro"), None);
        assert_eq!(fleet::installed_model(&[], "image", ""), None);
        // Nothing installed is not routed, and the reason names the model.
        let error = FleetRouter { installed_only: true }.route_snapshots(offers.clone(), "image",
            &GenerateRequestJson { model: "flux1-dev".into(), ..Default::default() }).err().unwrap();
        assert!(error.contains("no LAN AI Hub node has flux1-dev installed"), "{error}");
    }

    #[test]
    fn requests_without_their_media_never_leave() {
        let text_only = GenerateRequestJson { prompt: Some("a fox".into()), ..Default::default() };
        assert!(request_problem(Domain::Edit, &text_only).is_some());
        assert!(request_problem(Domain::Enhance, &text_only).is_some());
        assert!(request_problem(Domain::Inpaint, &text_only).unwrap().contains("image and mask"));
        assert!(request_problem(Domain::Image, &text_only).is_none());
        assert!(request_problem(Domain::Video, &text_only).is_none());
        let with_image = GenerateRequestJson { input_b64: Some("iVBOR".into()), ..text_only.clone() };
        assert!(request_problem(Domain::Edit, &with_image).is_none());
        // The primary input may carry an inpaint's image, never a paint's mesh.
        assert!(request_problem(Domain::Inpaint, &with_image).unwrap().contains("mask"));
        assert!(request_problem(Domain::Paint, &with_image).is_some());
        assert!(request_problem(Domain::Speech, &GenerateRequestJson::default()).is_some());
        assert!(request_problem(Domain::Speech, &text_only).is_none());
    }

    #[test]
    fn a_pinned_edit_model_needs_its_reference_on_the_wire() {
        let request = GenerateRequestJson { model: "flux2-klein-4b".into(), prompt: Some("a fox".into()), ..Default::default() };
        assert!(request_problem(Domain::Image, &request).unwrap().contains("flux2-klein-4b"));
        // flux2-dev draws from text and also edits.
        let dev = GenerateRequestJson { model: "flux2-dev".into(), prompt: Some("a fox".into()), ..Default::default() };
        assert!(request_problem(Domain::Image, &dev).is_none());
        assert!(request_problem(Domain::Edit, &GenerateRequestJson { input_b64: Some("iVBOR".into()), ..dev }).is_none());
    }

    #[test]
    fn every_domain_has_one_io_row_with_routable_inputs() {
        for domain in Domain::ALL {
            assert_eq!(Domain::parse(domain.as_str()), Some(domain));
            let io = domain.io();
            let primaries = io.inputs.iter().filter(|port| port.wire == crate::registry::Wire::Primary).count();
            assert!(primaries <= 1, "{}: two ports share the primary input", domain.as_str());
            assert!(!io.answer.content_types().is_empty());
        }
    }
}
