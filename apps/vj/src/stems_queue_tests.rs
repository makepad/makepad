use super::*;
use makepad_ai_hub::client::{ArtifactBytes, ContentProvider};
use makepad_ai_hub::error::AssetAiError;
use makepad_ai_hub::protocol::*;
use makepad_ai_hub::registry::Domain;
use makepad_widgets::makepad_platform::thread::Lane;
use std::sync::atomic::AtomicUsize;

const FRAMES: usize = 128;

#[derive(Debug)]
enum Call {
    Select(String, usize),
    Post(String),
    Poll(String),
    Fetch,
    Cancel(String),
}

struct Fake {
    tx: Sender<Call>,
    next: AtomicUsize,
    done: AtomicBool,
    failure: AtomicUsize,
    post_gate: AtomicBool,
    bytes: Vec<u8>,
}

impl Fake {
    fn status(&self, job: &str, state: &str) -> JobStatusJson {
        JobStatusJson {
            job_id: job.into(), state: state.into(), stage: Some("separate stems 1/2".into()),
            progress: None, artifacts: if state == JOB_STATE_DONE {
                vec![ArtifactRefJson {
                    id: "stems".into(), url: "/artifact/stems".into(),
                    content_type: STEMS_ARTIFACT_CONTENT_TYPE.into(), sha256: None,
                    byte_len: Some(self.bytes.len() as u64),
                }]
            } else { vec![] },
            error: (state == JOB_STATE_ERROR).then(|| format!("specific model failure\n{}", "x".repeat(400))),
            model: None, queued_ms: None, started_ms: None, finished_ms: None,
            log: None, partial_text: None, live: None, serving: None, text: None,
        }
    }
}

struct Provider(Arc<Fake>);
impl std::ops::Deref for Provider {
    type Target = Fake;
    fn deref(&self) -> &Fake { &self.0 }
}
impl ContentProvider for Provider {
    fn health(&self) -> Result<HealthJson, AssetAiError> { panic!("no health request in fake transport") }
    fn list_models(&self) -> Result<Vec<ModelInfoJson>, AssetAiError> { panic!("no model access") }
    fn request(&self, domain: Domain, request: &GenerateRequestJson) -> Result<String, AssetAiError> {
        assert_eq!(domain, Domain::Stems);
        assert_eq!(request.model, "bs-roformer-4stem");
        assert_eq!(request.input_content_type.as_deref(), Some("audio/wav"));
        assert!(request.input_b64.as_ref().is_some_and(|s| s.len() > FRAMES * 4));
        let id = format!("job-{}", self.next.fetch_add(1, Ordering::SeqCst));
        self.tx.send(Call::Post(id.clone())).unwrap();
        while id == "job-0" && self.post_gate.load(Ordering::Acquire) {
            let _ = CancellationToken::new().wait_until(Cx::monotonic_now() + 0.005);
        }
        match self.failure.load(Ordering::Acquire) {
            1 => Err(AssetAiError::Http("ambiguous response lost after upload".into())),
            2 => Err(AssetAiError::Busy),
            _ => Ok(id),
        }
    }
    fn poll(&self, job: &str) -> Result<JobStatusJson, AssetAiError> {
        self.tx.send(Call::Poll(job.into())).unwrap();
        match self.failure.load(Ordering::Acquire) {
            3 => Err(AssetAiError::Http("poll transport failed".into())),
            4 => Ok(self.status(job, JOB_STATE_ERROR)),
            _ => Ok(self.status(job, if self.done.load(Ordering::Acquire) { JOB_STATE_DONE } else { JOB_STATE_RUNNING })),
        }
    }
    fn fetch_artifact(&self, _: &str) -> Result<ArtifactBytes, AssetAiError> {
        self.tx.send(Call::Fetch).unwrap();
        if self.failure.load(Ordering::Acquire) == 5 {
            return Err(AssetAiError::Http("artifact download failed".into()));
        }
        Ok(ArtifactBytes { content_type: STEMS_ARTIFACT_CONTENT_TYPE.into(), bytes: self.bytes.clone() })
    }
    fn cancel(&self, job: &str) -> Result<JobStatusJson, AssetAiError> {
        let _ = self.tx.send(Call::Cancel(job.into()));
        Ok(self.status(job, JOB_STATE_CANCELLED))
    }
}

fn pcm(marker: i16, rate: u32) -> Arc<TrackPcm> {
    Arc::new(TrackPcm { frames: vec![[marker, -marker]; FRAMES], sample_rate: rate })
}

fn job(deck: DeckId, gen: u64) -> StemsJob {
    StemsJob { deck, gen, pcm: pcm(gen as i16, STEMS_RATE), source: None, start_secs: 0.0 }
}

fn artifact(frames: usize) -> Vec<u8> {
    encode_stems_artifact(&StemsArtifact {
        frames, sample_rate: STEMS_RATE,
        channels: std::array::from_fn(|channel| vec![(channel as f32 + 1.0) * 0.2; frames]),
    }).unwrap()
}

struct Fixture {
    stems: Option<StemsPool>,
    pool: TaskPool,
    fake: Arc<Fake>,
    rx: Receiver<Call>,
    calls: Vec<Call>,
    messages: Vec<StemsMsg>,
    root: PathBuf,
}

impl Fixture {
    fn new(tag: &str) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../local/agent_state/dj-stems-queue/tests")
            .join(format!("{tag}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let (tx, rx) = channel();
        let fake = Arc::new(Fake {
            tx, next: AtomicUsize::new(0), done: AtomicBool::new(false), failure: AtomicUsize::new(0),
            post_gate: AtomicBool::new(false), bytes: artifact(FRAMES),
        });
        let mut stems = StemsPool::with_paths(root.clone(), root.join("no-model"), 0);
        let picked = fake.clone();
        stems.picker = Arc::new(move |request| {
            picked.tx.send(Call::Select(request.model.clone(), request.input_b64.as_ref().map_or(0, String::len))).unwrap();
            Ok(("fake-hub".into(), Box::new(Provider(picked.clone()))))
        });
        Self { stems: Some(stems), pool: crate::test_task_pool(), fake, rx, calls: vec![], messages: vec![], root }
    }
    fn start(&mut self) { self.stems.as_mut().unwrap().start(crate::test_thread_spawner(), self.pool.clone()); }
    fn stems(&mut self) -> &mut StemsPool { self.stems.as_mut().unwrap() }
    fn pump(&mut self) {
        if let Some(stems) = &mut self.stems { self.messages.extend(stems.poll()); }
        self.calls.extend(self.rx.try_iter());
    }
    fn until(&mut self, condition: impl Fn(&Self) -> bool) {
        let deadline = Cx::monotonic_now() + 8.0;
        loop {
            self.pump();
            if condition(self) { return; }
            assert!(Cx::monotonic_now() < deadline, "timed out: {:?}", self.calls);
            let _ = CancellationToken::new().wait_until(Cx::monotonic_now() + 0.005);
        }
    }
    fn posts(&self) -> usize { self.calls.iter().filter(|call| matches!(call, Call::Post(_))).count() }
    fn done(&self, deck: DeckId, gen: u64) -> usize {
        self.messages.iter().filter(|msg| matches!(msg, StemsMsg::Done { deck: d, gen: g } if *d == deck && *g == gen)).count()
    }
    fn failed(&self) -> bool {
        self.messages.iter().any(|msg| matches!(msg, StemsMsg::Status { working: false, .. }))
    }
    fn stop(&mut self) {
        if let Some(mut stems) = self.stems.take() {
            let mut worker = stems.worker.take();
            drop(stems);
            if let Some(worker) = &mut worker {
                let deadline = Cx::monotonic_now() + 8.0;
                while worker.try_take().is_none() {
                    assert!(Cx::monotonic_now() < deadline, "controller did not shut down");
                    let _ = CancellationToken::new().wait_until(Cx::monotonic_now() + 0.005);
                }
            }
            self.calls.extend(self.rx.try_iter());
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.fake.post_gate.store(false, Ordering::Release);
        self.stop();
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn both_decks_are_accepted_before_either_finishes_and_artifacts_install_once() {
    let mut f = Fixture::new("two-decks");
    f.stems().submit_hub(job(DeckId::A, 1));
    f.stems().submit_hub(job(DeckId::B, 2));
    f.start();
    f.until(|f| f.posts() == 2 && f.calls.iter().filter(|c| matches!(c, Call::Poll(_))).count() >= 2);
    assert_eq!(f.done(DeckId::A, 1) + f.done(DeckId::B, 2), 0);
    assert!(!f.calls.iter().any(|c| matches!(c, Call::Fetch)));
    assert!(f.calls.iter().filter(|c| matches!(c, Call::Select(model, len) if model == "bs-roformer-4stem" && *len > FRAMES * 4)).count() == 2);
    f.fake.done.store(true, Ordering::Release);
    f.until(|f| f.done(DeckId::A, 1) == 1 && f.done(DeckId::B, 2) == 1);
    assert_eq!(f.posts(), 2);
    for deck in [DeckId::A, DeckId::B] {
        let chunks: Vec<_> = f.messages.iter().filter_map(|msg| match msg {
            StemsMsg::Chunk(chunk) if chunk.deck == deck => Some(chunk), _ => None,
        }).collect();
        assert_eq!(chunks.len(), 1, "one short artifact, one installation");
        let chunk = chunks[0];
        assert_eq!(chunk.index, 0);
        assert_eq!(chunk.chunk_frames, STEMS_RATE as usize);
        assert_eq!(chunk.lanes[0].len(), FRAMES);
        for (lane, model_stem) in [3, 0, 1, 2].into_iter().enumerate() {
            let expected = [encode_stem_sample((model_stem * 2 + 1) as f32 * 0.2), encode_stem_sample((model_stem * 2 + 2) as f32 * 0.2)];
            assert_eq!(chunk.lanes[lane][32], expected, "channel order and >1.0 headroom");
        }
        let gen = if deck == DeckId::A { 1 } else { 2 };
        let digest = track_digest(&job(deck, gen).pcm);
        assert!(cache_is_complete(&f.root, &digest, FRAMES as u64), "both cache pins survive zero budget");
        assert!(f.messages.iter().any(|msg| matches!(msg, StemsMsg::Coverage { deck: d, gen: g, complete: true, .. } if *d == deck && *g == gen)));
    }
    let statuses: Vec<_> = f.messages.iter().filter_map(|m| match m { StemsMsg::Status { text, .. } => Some(text.as_str()), _ => None }).collect();
    for stage in ["waiting locally", "preparing upload", "uploading to fake-hub", "accepted / queued", "separate stems", "fetching artifact", "installing artifact"] {
        assert!(statuses.iter().any(|s| s.contains(stage)), "missing {stage}: {statuses:?}");
    }
}

#[test]
fn repeated_a_replacement_preserves_b_and_only_sends_current_a() {
    let mut f = Fixture::new("replace-queued");
    f.stems().submit_hub(job(DeckId::B, 1));
    for gen in 2..40 { f.stems().submit_hub(job(DeckId::A, gen)); }
    f.fake.done.store(true, Ordering::Release);
    f.start();
    f.until(|f| f.done(DeckId::A, 39) == 1 && f.done(DeckId::B, 1) == 1);
    assert_eq!(f.posts(), 2, "saturated inbox retains latest A and pending B");
    assert!(!f.messages.iter().any(|m| matches!(m, StemsMsg::Done { deck: DeckId::A, gen } if *gen != 39)));
}

#[test]
fn accepted_replacement_cancels_old_job_without_touching_other_deck() {
    let mut f = Fixture::new("replace-accepted");
    f.start();
    f.stems().submit_hub(job(DeckId::A, 1));
    f.until(|f| f.calls.iter().any(|c| matches!(c, Call::Poll(_))));
    f.stems().submit_hub(job(DeckId::B, 2));
    f.stems().invalidate(DeckId::A);
    f.stems().submit_hub(job(DeckId::A, 3));
    f.until(|f| f.posts() == 3);
    assert!(f.calls.iter().any(|c| matches!(c, Call::Cancel(id) if id == "job-0")));
    f.fake.done.store(true, Ordering::Release);
    f.until(|f| f.done(DeckId::A, 3) == 1 && f.done(DeckId::B, 2) == 1);
    assert_eq!(f.done(DeckId::A, 1), 0);
    assert!(cache_is_complete(&f.root, &track_digest(&job(DeckId::A, 3).pcm), FRAMES as u64));
    assert!(cache_is_complete(&f.root, &track_digest(&job(DeckId::B, 2).pcm), FRAMES as u64));
}

#[test]
fn same_generation_cancellation_filters_already_buffered_results() {
    let mut f = Fixture::new("cancel-buffered");
    f.fake.done.store(true, Ordering::Release);
    f.start();
    f.stems().submit_hub(job(DeckId::A, 1));
    // Leave the result queue untouched until the complete artifact is on disk.
    let digest = track_digest(&job(DeckId::A, 1).pcm);
    let deadline = Cx::monotonic_now() + 8.0;
    while !cache_is_complete(&f.root, &digest, FRAMES as u64) {
        assert!(Cx::monotonic_now() < deadline);
        let _ = CancellationToken::new().wait_until(Cx::monotonic_now() + 0.005);
    }
    f.stems().cancel(DeckId::A); // mode off or side-channel takeover, no load-gen change
    f.pump();
    assert!(f.messages.is_empty(), "stale status, coverage and chunks are all suppressed");
    f.stems().submit_hub(job(DeckId::A, 1));
    f.until(|f| f.done(DeckId::A, 1) == 1);
    assert_eq!(f.posts(), 1, "replacement is a warm cache hit");
}

#[test]
fn prefetch_is_cancelled_for_foreground_and_releases_bookkeeping_once() {
    let mut f = Fixture::new("prefetch");
    f.start();
    f.stems().submit_prefetch(pcm(10, STEMS_RATE), None, SeparationAction::Hub);
    f.until(|f| f.calls.iter().any(|c| matches!(c, Call::Poll(_))));
    f.stems().submit_hub(job(DeckId::A, 1));
    f.stems().submit_hub(job(DeckId::B, 2));
    f.until(|f| f.posts() == 3 && f.messages.iter().any(|m| matches!(m, StemsMsg::PrefetchDone { complete: false, .. })));
    assert!(f.calls.iter().any(|c| matches!(c, Call::Cancel(id) if id == "job-0")));
    assert_eq!(f.messages.iter().filter(|m| matches!(m, StemsMsg::PrefetchDone { .. })).count(), 1);
    assert!(!f.messages.iter().any(|m| matches!(m, StemsMsg::Chunk(c) if c.gen == PREFETCH_GEN)));
}

#[test]
fn cancel_before_start_and_shutdown_never_send_queued_requests() {
    let mut f = Fixture::new("cancel-queued");
    f.stems().submit_hub(job(DeckId::A, 1));
    f.stems().submit_prefetch(pcm(10, STEMS_RATE), None, SeparationAction::Hub);
    f.stems().invalidate(DeckId::A);
    f.stems().cancel_prefetch();
    f.start();
    f.until(|f| f.messages.iter().any(|m| matches!(m, StemsMsg::PrefetchDone { complete: false, .. })));
    f.stop();
    assert!(f.calls.is_empty());
}

#[test]
fn drop_cancels_accepted_jobs_and_prevents_further_poll_or_fetch() {
    let mut f = Fixture::new("drop");
    f.start();
    f.stems().submit_hub(job(DeckId::A, 1));
    f.until(|f| f.calls.iter().any(|c| matches!(c, Call::Poll(_))));
    f.calls.clear();
    f.stop();
    assert!(f.calls.iter().any(|c| matches!(c, Call::Cancel(id) if id == "job-0")));
    assert!(!f.calls.iter().any(|c| matches!(c, Call::Post(_) | Call::Fetch)));
}

#[test]
fn cancellation_during_upload_cancels_the_returned_id_without_polling() {
    let mut f = Fixture::new("cancel-upload");
    f.fake.post_gate.store(true, Ordering::Release);
    f.start();
    f.stems().submit_hub(job(DeckId::A, 1));
    f.until(|f| f.posts() == 1);
    f.stems().invalidate(DeckId::A);
    f.fake.post_gate.store(false, Ordering::Release);
    f.until(|f| f.calls.iter().any(|c| matches!(c, Call::Cancel(_))));
    assert!(!f.calls.iter().any(|c| matches!(c, Call::Poll(_) | Call::Fetch)));
}

#[test]
fn failures_settle_with_a_bounded_reason_and_never_replay_post() {
    for failure in 1..=5 {
        let mut f = Fixture::new("failure");
        f.fake.failure.store(failure, Ordering::Release);
        f.fake.done.store(true, Ordering::Release);
        f.start();
        f.stems().submit_hub(job(DeckId::A, 1));
        f.until(|f| f.failed());
        let reason = f.messages.iter().find_map(|m| match m { StemsMsg::Status { text, working: false, .. } => Some(text), _ => None }).unwrap();
        assert!(reason.len() <= 260 && !reason.contains('\n'), "{reason}");
        assert_ne!(reason, "stems: unavailable");
        f.stop();
        assert_eq!(f.posts(), 1);
        assert_eq!(f.done(DeckId::A, 1), 0);
    }
}

#[test]
fn full_task_pool_retains_work_and_cancelled_slots_never_post() {
    let mut f = Fixture::new("pool-full");
    // Reservations deterministically saturate the queue without sleeps or
    // helper threads. They have no jobs, so both pool workers remain idle.
    let mut reservations = Vec::new();
    for lane in [Lane::Light, Lane::Heavy] { while let Ok(slot) = f.pool.reserve(lane) { reservations.push(slot); } }
    assert!(!reservations.is_empty());
    f.stems().submit_hub(job(DeckId::A, 1));
    f.stems().submit_hub(job(DeckId::B, 2));
    f.start();
    // Coverage proves the controller consumed both requests despite refusal.
    f.until(|f| f.messages.iter().filter(|m| matches!(m, StemsMsg::Coverage { .. })).count() == 2);
    assert_eq!(f.posts(), 0);
    f.stems().invalidate(DeckId::A);
    f.stems().submit_hub(job(DeckId::A, 3));
    drop(reservations);
    f.fake.done.store(true, Ordering::Release);
    f.until(|f| f.done(DeckId::A, 3) == 1 && f.done(DeckId::B, 2) == 1);
    assert_eq!(f.posts(), 2);
}

#[test]
fn complete_warm_hub_cache_never_selects_a_node_or_accesses_model() {
    let mut f = Fixture::new("warm-hub");
    let job = job(DeckId::A, 1);
    let digest = track_digest(&job.pcm);
    let mut cache = open_cache(&f.root, &job.pcm, &digest).unwrap();
    cache.write_span(0, &std::array::from_fn(|i| StereoBuf {
        left: vec![(i + 1) as f32 * 0.1; FRAMES], right: vec![(i + 1) as f32 * 0.2; FRAMES],
    })).unwrap();
    f.stems().submit_hub(job);
    f.start();
    f.until(|f| f.done(DeckId::A, 1) == 1);
    assert!(f.calls.is_empty());
    assert_eq!(f.messages.iter().filter(|m| matches!(m, StemsMsg::Chunk(_))).count(), 1, "partial-second tail served");
}

#[test]
fn artifact_rate_validation_and_resampling_preserve_track_geometry() {
    let root = Fixture::new("shape");
    let mut job = job(DeckId::B, 17);
    job.pcm = pcm(1, 48_000);
    let (tx, rx) = channel();
    let frames = model_frames(&job.pcm);
    assert_ne!(frames, FRAMES);
    install_hub_artifact(&job, &root.root, &track_digest(&job.pcm), &artifact(frames), &tx, &|| false).unwrap();
    let messages: Vec<_> = rx.try_iter().collect();
    let chunk = messages.iter().find_map(|m| match m { StemsMsg::Chunk(c) => Some(c), _ => None }).unwrap();
    assert_eq!(chunk.chunk_frames, 48_000);
    assert_eq!(chunk.lanes[0].len(), FRAMES);
    assert_eq!(chunk.gen, 17);
    let wrong = install_hub_artifact(&job, &root.root, &track_digest(&job.pcm), &artifact(FRAMES), &tx, &|| false).unwrap_err();
    assert!(wrong.contains("hub stems shape"));
    assert!(rx.try_iter().next().is_none());
}

#[test]
fn a_blocked_prefetch_upload_does_not_hold_up_either_decks_submission() {
    let mut f = Fixture::new("blocked-prefetch");
    f.fake.post_gate.store(true, Ordering::Release);
    f.start();
    f.stems().submit_prefetch(pcm(10, STEMS_RATE), None, SeparationAction::Hub);
    f.until(|f| f.posts() == 1);
    f.stems().submit_hub(job(DeckId::A, 1));
    f.stems().submit_hub(job(DeckId::B, 2));
    f.until(|f| f.posts() == 3);
    // The two-worker pool has only one free worker, but neither deck waits
    // for the background HTTP call, or for the other deck's accepted job.
    assert!(f.fake.post_gate.load(Ordering::Acquire));
    f.fake.post_gate.store(false, Ordering::Release);
    f.until(|f| f.calls.iter().any(|c| matches!(c, Call::Cancel(id) if id == "job-0"))
        && f.messages.iter().any(|m| matches!(m, StemsMsg::PrefetchDone { complete: false, .. })));
    assert!(f.calls.iter().any(|c| matches!(c, Call::Cancel(id) if id == "job-0")));
    assert!(!f.calls.iter().any(|c| matches!(c, Call::Poll(id) if id == "job-0")));
}

#[test]
fn shutdown_with_full_pool_discards_unsent_decks_and_prefetch() {
    let mut f = Fixture::new("shutdown-full");
    let mut reservations = Vec::new();
    for lane in [Lane::Light, Lane::Heavy] { while let Ok(slot) = f.pool.reserve(lane) { reservations.push(slot); } }
    f.start();
    f.stems().submit_hub(job(DeckId::A, 1));
    f.stems().submit_prefetch(pcm(10, STEMS_RATE), None, SeparationAction::Hub);
    f.until(|f| f.messages.iter().any(|m| matches!(m, StemsMsg::Coverage { .. })));
    f.stop();
    assert!(f.calls.is_empty(), "no selection or POST after shutdown");
    drop(reservations);
}

#[test]
fn cancelled_prefetch_releases_even_when_pool_refuses_every_task() {
    let mut f = Fixture::new("prefetch-full");
    let mut reservations = Vec::new();
    for lane in [Lane::Light, Lane::Heavy] { while let Ok(slot) = f.pool.reserve(lane) { reservations.push(slot); } }
    f.start();
    f.stems().submit_prefetch(pcm(10, STEMS_RATE), None, SeparationAction::Hub);
    f.until(|f| f.messages.iter().any(|m| matches!(m, StemsMsg::Coverage { .. })));
    f.stems().cancel_prefetch();
    f.until(|f| f.messages.iter().any(|m| matches!(m, StemsMsg::PrefetchDone { complete: false, .. })));
    assert!(f.calls.is_empty());
    drop(reservations);
}

#[test]
fn a_closed_pool_settles_the_deck_and_background_instead_of_waiting_forever() {
    let mut f = Fixture::new("closed");
    f.pool = TaskPool::closed();
    f.start();
    f.stems().submit_hub(job(DeckId::A, 1));
    f.stems().submit_prefetch(pcm(10, STEMS_RATE), None, SeparationAction::Hub);
    f.until(|f| f.failed() && f.messages.iter().any(|m| matches!(m, StemsMsg::PrefetchDone { complete: false, .. })));
    assert!(f.calls.is_empty());
}

#[test]
fn local_inbox_keeps_b_when_a_is_replaced_and_never_falls_back_to_hub() {
    let mut f = Fixture::new("local-inbox");
    f.stems().submit_local(job(DeckId::A, 1));
    f.stems().submit_local(job(DeckId::B, 2));
    f.stems().submit_local(job(DeckId::A, 3));
    f.start();
    f.until(|f| f.messages.iter().filter(|m| matches!(m, StemsMsg::Status { working: false, .. })).count() == 2);
    for (deck, gen) in [(DeckId::A, 3), (DeckId::B, 2)] {
        assert!(f.messages.iter().any(|m| matches!(m, StemsMsg::Status { deck: d, gen: g, text, working: false } if *d == deck && *g == gen && text.contains("model not installed"))));
    }
    assert!(f.calls.is_empty());
}

#[test]
fn late_prefetch_acknowledgement_cannot_release_its_replacement() {
    let mut f = Fixture::new("prefetch-stale-ack");
    f.fake.post_gate.store(true, Ordering::Release);
    f.start();
    f.stems().submit_prefetch(pcm(10, STEMS_RATE), None, SeparationAction::Hub);
    f.until(|f| f.posts() == 1);
    f.stems().cancel_prefetch();
    f.until(|f| f.messages.iter().any(|m| matches!(m, StemsMsg::PrefetchDone { complete: false, .. })));
    f.stems().submit_prefetch(pcm(20, STEMS_RATE), None, SeparationAction::Hub);
    f.fake.post_gate.store(false, Ordering::Release);
    f.until(|f| f.calls.iter().any(|c| matches!(c, Call::Poll(id) if id == "job-1")));
    assert_eq!(f.messages.iter().filter(|m| matches!(m, StemsMsg::PrefetchDone { .. })).count(), 1,
        "only the prompt cancellation acknowledgement, no stale release");
    f.fake.done.store(true, Ordering::Release);
    f.until(|f| f.messages.iter().any(|m| matches!(m, StemsMsg::PrefetchDone { complete: true, .. })));
    assert_eq!(f.messages.iter().filter(|m| matches!(m, StemsMsg::PrefetchDone { .. })).count(), 2);
}
