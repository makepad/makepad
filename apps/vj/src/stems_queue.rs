//! Bounded admission and cache ownership for the once-started stems worker.
use super::*;

pub(super) fn bounded_reason(reason: &str) -> String {
    reason.chars().map(|c| if c.is_control() { ' ' } else { c }).take(240).collect()
}

pub(super) trait StemSink {
    fn send(&self, message: StemsMsg) -> Result<(), ()>;
}

impl StemSink for Sender<StemsMsg> {
    fn send(&self, message: StemsMsg) -> Result<(), ()> {
        Sender::send(self, message).map_err(|_| ())
    }
}

#[derive(Clone)]
struct RequestCancel(Arc<AtomicBool>);

impl RequestCancel {
    fn new() -> Self { Self(Arc::new(AtomicBool::new(false))) }
    fn cancel(&self) { self.0.store(true, Ordering::Release); }
    fn is_cancelled(&self) -> bool { self.0.load(Ordering::Acquire) }
}

struct TaggedMessage {
    request: u64,
    cancel: RequestCancel,
    message: StemsMsg,
}

#[derive(Clone)]
pub(super) struct JobOutput {
    tx: Sender<TaggedMessage>,
    cancel: RequestCancel,
    request: u64,
    deck: DeckId,
    gen: u64,
}

impl StemSink for JobOutput {
    fn send(&self, message: StemsMsg) -> Result<(), ()> {
        // Completion of background bookkeeping survives cancellation. All
        // deck messages are guarded again when the UI consumes the channel.
        if self.cancel.is_cancelled() && !matches!(message, StemsMsg::PrefetchDone { .. }) {
            return Err(());
        }
        self.tx.send(TaggedMessage {
            request: self.request, cancel: self.cancel.clone(), message,
        }).map_err(|_| ())
    }
}

impl JobOutput {
    pub fn status(&self, text: String, working: bool) {
        let _ = self.send(StemsMsg::Status { deck: self.deck, gen: self.gen, text, working });
    }
}

pub(super) struct Work {
    pub job: StemsJob,
    backend: SeparationBackend,
    pub output: JobOutput,
    pub digest: Option<String>,
    epoch: u64,
    shutdown: Arc<AtomicBool>,
}

impl Work {
    pub fn cancelled(&self) -> bool {
        self.output.cancel.is_cancelled() || self.shutdown.load(Ordering::Acquire)
    }

    fn prefetch_done(&self, root: &Path) {
        if self.job.gen == PREFETCH_GEN {
            let frames = model_frames(&self.job.pcm) as u64;
            let complete = !self.cancelled() && self.digest.as_ref()
                .is_some_and(|digest| cache_is_complete(root, digest, frames));
            let _ = self.output.send(StemsMsg::PrefetchDone {
                digest: self.digest.clone().unwrap_or_default(), model_frames: frames, complete,
            });
        }
    }
}

struct Pin {
    epoch: u64,
    digest: String,
}

pub struct StemsPool {
    tx: SyncSender<Work>,
    jobs: Option<Receiver<Work>>,
    worker: Option<makepad_widgets::makepad_platform::thread::TaskHandle<()>>,
    // UI-owned bounded staging. A full worker inbox is retried next poll.
    staged: [Option<Work>; 3],
    current: [Option<(u64, RequestCancel)>; 3],
    sequence: u64,
    epochs: Arc<[AtomicU64; 2]>,
    shutdown: Arc<AtomicBool>,
    deck_waiting: Arc<AtomicBool>,
    out: Sender<TaggedMessage>,
    rx: Receiver<TaggedMessage>,
    root: PathBuf,
    checkpoint: PathBuf,
    budget_bytes: u64,
    #[cfg(not(target_arch = "wasm32"))]
    picker: hub::Picker,
}

impl Default for StemsPool {
    fn default() -> Self { Self::new() }
}

impl Drop for StemsPool {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        for current in self.current.iter().flatten() { current.1.cancel(); }
    }
}

impl StemsPool {
    pub fn new() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        let _ = makepad_ai_hub::discovery::start_listener();
        Self::with_paths(cache_dir(), checkpoint_path(), cache_budget_bytes())
    }

    pub fn with_paths(root: PathBuf, checkpoint: PathBuf, budget_bytes: u64) -> Self {
        let (tx, jobs) = sync_channel(8);
        let (out, rx) = channel();
        Self {
            tx, jobs: Some(jobs), worker: None, staged: std::array::from_fn(|_| None),
            current: std::array::from_fn(|_| None), sequence: 0,
            epochs: Arc::new(std::array::from_fn(|_| AtomicU64::new(0))),
            shutdown: Arc::new(AtomicBool::new(false)),
            deck_waiting: Arc::new(AtomicBool::new(false)), out, rx,
            root, checkpoint, budget_bytes,
            #[cfg(not(target_arch = "wasm32"))]
            picker: hub::fleet_picker(),
        }
    }

    pub fn start(&mut self, spawner: ThreadSpawner, pool: TaskPool) {
        let Some(jobs) = self.jobs.take() else { return };
        let root = self.root.clone();
        let checkpoint = self.checkpoint.clone();
        let budget = self.budget_bytes;
        let epochs = self.epochs.clone();
        let shutdown = self.shutdown.clone();
        let waiting = self.deck_waiting.clone();
        #[cfg(not(target_arch = "wasm32"))]
        let picker = self.picker.clone();
        let options = ThreadOptions { name: Some("vj-stems".into()), ..Default::default() };
        match spawner.spawn_worker(options, move || {
            let mut model = None;
            let mut pins: [Option<Pin>; 2] = [None, None];
            let mut pending: [Option<Work>; 3] = std::array::from_fn(|_| None);
            #[cfg(not(target_arch = "wasm32"))]
            let mut remote: [Option<hub::Remote>; 3] = std::array::from_fn(|_| None);
            let mut disconnected = false;
            loop {
                #[cfg(not(target_arch = "wasm32"))]
                let mut has_remote = remote.iter().any(Option::is_some);
                #[cfg(target_arch = "wasm32")]
                let has_remote = false;
                let first = if has_remote || pending.iter().any(Option::is_some) {
                    // Use the platform clock on both native and wasm; std's
                    // timed channel wait is not a browser-worker clock.
                    let _ = CancellationToken::new().wait_until(Cx::monotonic_now() + 0.01);
                    jobs.try_recv().map_err(|error| match error {
                        std::sync::mpsc::TryRecvError::Empty => std::sync::mpsc::RecvTimeoutError::Timeout,
                        std::sync::mpsc::TryRecvError::Disconnected => std::sync::mpsc::RecvTimeoutError::Disconnected,
                    })
                } else {
                    jobs.recv().map_err(|_| std::sync::mpsc::RecvTimeoutError::Disconnected)
                };
                match first {
                    Ok(work) => admit(work, &mut pending, &root),
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => disconnected = true,
                    Err(_) => {}
                }
                while let Ok(work) = jobs.try_recv() { admit(work, &mut pending, &root); }
                if disconnected { shutdown.store(true, Ordering::Release); }

                // Hash/pin ALL pending decks before any pruning. Hub tasks
                // never open, write or prune a cache. Old completions never
                // write pins; only current inbox requests can own them.
                let mut changed = false;
                for (index, pin) in pins.iter_mut().enumerate() {
                    if pin.as_ref().is_some_and(|pin| pin.epoch != epochs[index].load(Ordering::Acquire)) {
                        *pin = None;
                    }
                }
                for index in 0..3 {
                    if pending[index].as_ref().is_some_and(Work::cancelled) {
                        pending[index].take().unwrap().prefetch_done(&root);
                    }
                    if let Some(work) = &mut pending[index] {
                        if work.digest.is_none() {
                            work.digest = Some(track_digest(&work.job.pcm));
                            changed = true;
                            if index < 2 && !work.cancelled()
                                && work.epoch == epochs[index].load(Ordering::Acquire)
                            {
                                pins[index] = Some(Pin { epoch: work.epoch, digest: work.digest.clone().unwrap() });
                            }
                        }
                    }
                }
                if changed {
                    let pinned = std::array::from_fn(|i| pins[i].as_ref().map(|pin| pin.digest.clone()));
                    prune_cache(&root, budget, &pinned);
                }
                waiting.store(pending[..2].iter().any(Option::is_some), Ordering::Release);

                #[cfg(not(target_arch = "wasm32"))]
                for active in &mut remote {
                    if active.as_mut().is_some_and(|active| active.pump(&pool, &picker, &root)) {
                        active.take().unwrap().work.prefetch_done(&root);
                    }
                }

                for index in 0..3 {
                    #[cfg(not(target_arch = "wasm32"))]
                    if remote[index].is_some() { continue; }
                    if index == 2 {
                        let mut foreground = pending[..2].iter().any(Option::is_some);
                        #[cfg(not(target_arch = "wasm32"))]
                        { foreground |= remote[..2].iter().any(Option::is_some); }
                        if foreground { continue; }
                    }
                    let Some(work) = pending[index].take() else { continue };
                    if work.cancelled() { work.prefetch_done(&root); continue; }
                    let digest = work.digest.as_ref().unwrap();
                    match work.backend {
                        SeparationBackend::Local => {
                            run_local(&work.job, &root, &checkpoint, digest, &mut model, &work.output,
                                &|| work.cancelled() || (index == 2 && waiting.load(Ordering::Acquire)));
                        }
                        SeparationBackend::Hub => {
                            #[cfg(not(target_arch = "wasm32"))]
                            {
                                if serve_hub_cache(&work, &root) {
                                    work.prefetch_done(&root);
                                } else {
                                    remote[index] = Some(hub::Remote::new(work));
                                }
                            }
                            #[cfg(target_arch = "wasm32")]
                            {
                                work.output.status("stems: hub separation unavailable on this target".into(), false);
                                work.prefetch_done(&root);
                            }
                        }
                    }
                }
                #[cfg(not(target_arch = "wasm32"))]
                { has_remote = remote.iter().any(Option::is_some); }
                if disconnected && !has_remote { break; }
            }
        }) {
            Ok(handle) => self.worker = Some(handle),
            Err(error) => makepad_widgets::log!("vj stems worker unavailable: {error}"),
        }
    }

    /// Retire the request, retaining this loaded track's cache pin.
    pub fn cancel(&mut self, deck: DeckId) {
        if let Some((_, token)) = &self.current[deck.index()] { token.cancel(); }
    }

    /// A load/unload boundary also retires the old track's cache pin.
    pub fn invalidate(&mut self, deck: DeckId) {
        self.cancel(deck);
        self.cancel_prefetch();
        self.epochs[deck.index()].fetch_add(1, Ordering::AcqRel);
    }

    pub fn cancel_prefetch(&mut self) {
        if let Some((request, token)) = &self.current[2] {
            if token.is_cancelled() { return; }
            token.cancel();
            // Release the UI's background slot promptly even when an HTTP
            // upload is still returning. Its later acknowledgement carries
            // this same id and cannot release a newer decode/separation.
            let _ = self.out.send(TaggedMessage {
                request: *request,
                cancel: token.clone(),
                message: StemsMsg::PrefetchDone {
                    digest: String::new(), model_frames: 0, complete: false,
                },
            });
        }
    }

    fn submit_to(&mut self, job: StemsJob, backend: SeparationBackend) {
        let index = if job.gen == PREFETCH_GEN { 2 } else { job.deck.index() };
        if index < 2 {
            self.deck_waiting.store(true, Ordering::Release);
            self.cancel_prefetch();
        }
        if let Some((_, token)) = &self.current[index] { token.cancel(); }
        self.sequence += 1;
        let token = RequestCancel::new();
        let output = JobOutput {
            tx: self.out.clone(), cancel: token.clone(), request: self.sequence,
            deck: job.deck, gen: job.gen,
        };
        output.status("stems: waiting locally".into(), true);
        let epoch = self.epochs[job.deck.index()].load(Ordering::Acquire);
        self.current[index] = Some((self.sequence, token));
        self.staged[index] = Some(Work {
            job, backend, output, digest: None, epoch, shutdown: self.shutdown.clone(),
        });
        self.flush();
    }

    fn flush(&mut self) {
        for slot in &mut self.staged {
            let Some(work) = slot.take() else { continue };
            match self.tx.try_send(work) {
                Ok(()) => {}
                Err(TrySendError::Full(work)) => *slot = Some(work),
                Err(TrySendError::Disconnected(work)) => {
                    work.output.status("stems: worker unavailable".into(), false);
                    // No hashing or disk access on this UI failure path.
                    if work.job.gen == PREFETCH_GEN {
                        let _ = work.output.send(StemsMsg::PrefetchDone {
                            digest: String::new(), model_frames: model_frames(&work.job.pcm) as u64, complete: false,
                        });
                    }
                }
            }
        }
    }

    pub fn submit_local(&mut self, job: StemsJob) { self.submit_to(job, SeparationBackend::Local); }
    pub fn submit_hub(&mut self, job: StemsJob) { self.submit_to(job, SeparationBackend::Hub); }
    #[cfg(test)]
    pub(super) fn submit(&mut self, job: StemsJob) { self.submit_local(job); }

    pub fn submit_prefetch(&mut self, pcm: Arc<TrackPcm>, source: Option<PathBuf>, action: SeparationAction) {
        let backend = match action {
            SeparationAction::Hub => SeparationBackend::Hub,
            SeparationAction::Local => SeparationBackend::Local,
            _ => return,
        };
        self.submit_to(StemsJob { deck: DeckId::A, gen: PREFETCH_GEN, pcm, source, start_secs: 0.0 }, backend);
    }

    pub fn poll(&mut self) -> Vec<StemsMsg> {
        self.flush();
        let mut messages = Vec::new();
        while let Ok(mut tagged) = self.rx.try_recv() {
            if matches!(tagged.message, StemsMsg::PrefetchDone { .. }) {
                if self.current[2].as_ref().is_some_and(|(request, _)| *request == tagged.request) {
                    self.current[2] = None;
                    if tagged.cancel.is_cancelled() {
                        if let StemsMsg::PrefetchDone { complete, .. } = &mut tagged.message { *complete = false; }
                    }
                    messages.push(tagged.message);
                }
            } else if !tagged.cancel.is_cancelled() {
                messages.push(tagged.message);
            }
        }
        messages
    }
}

fn admit(work: Work, pending: &mut [Option<Work>; 3], root: &Path) {
    let index = if work.job.gen == PREFETCH_GEN { 2 } else { work.job.deck.index() };
    if work.cancelled() { work.prefetch_done(root); return; }
    if let Some(previous) = pending[index].replace(work) { previous.prefetch_done(root); }
}

#[cfg(not(target_arch = "wasm32"))]
fn serve_hub_cache(work: &Work, root: &Path) -> bool {
    let job = &work.job;
    let digest = work.digest.as_ref().unwrap();
    let frames = model_frames(&job.pcm) as u64;
    if let Some(lanes) = job.source.as_deref().and_then(load_sidecar) {
        if !work.cancelled() { run_sidecar(job, lanes, &work.output); }
        let _ = work.output.send(StemsMsg::Coverage {
            deck: job.deck, gen: job.gen, digest: digest.clone(), model_frames: frames,
            complete: cache_is_complete(root, digest, frames),
        });
        return true;
    }
    let Some(mut cache) = open_cache(root, &job.pcm, digest) else { return false };
    let _ = work.output.send(StemsMsg::Coverage {
        deck: job.deck, gen: job.gen, digest: digest.clone(), model_frames: frames, complete: cache.is_complete(),
    });
    if work.cancelled() { return true; }
    if job.gen == PREFETCH_GEN { return cache.is_complete(); }
    let rate = job.pcm.sample_rate.max(1);
    let mut writer = ChunkWriter::new(chunk_frames(rate), chunk_count(job.pcm.frames.len(), rate));
    // Serve every available leading span, including the last partial chunk.
    let gap = run_cached(job, &mut cache, &mut writer, 0, &work.output);
    writer.finish(job.deck, job.gen, &work.output);
    if gap.is_none() && cache.is_complete() {
        let _ = work.output.send(StemsMsg::Done { deck: job.deck, gen: job.gen });
        return true;
    }
    false
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "stems_queue_tests.rs"]
mod tests;
