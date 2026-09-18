//! Parallel decode/token preparation, one durable writer, bounded UI messages.
use crate::{
    apple_mail::AppleMailSource,
    mail_index::{CachePartition, MailIndex, PreparedMessage, SearchPage},
    source::*,
};
use makepad_widgets::makepad_platform::thread::{
    Lane, PoolOptions, TaskHandle, TaskPool, ThreadOptions, ThreadSpawner,
};
use makepad_widgets::*;
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc,
    },
    time::{Duration, Instant},
};

pub enum Request {
    Search {
        generation: u64,
        text: String,
        limit: usize,
    },
    Read(u64),
    /// Extract one attachment of a message to a file the system can open.
    Attachment {
        id: u64,
        index: usize,
    },
    Refresh,
}
pub enum Reply {
    Status(String),
    ScanDone(String),
    Error(String),
    Attachment {
        id: u64,
        index: usize,
        path: std::path::PathBuf,
    },
    AttachmentError {
        id: u64,
        index: usize,
        message: String,
    },
    QueryError {
        generation: u64,
        message: String,
    },
    Results {
        generation: u64,
        page: Arc<SearchPage>,
    },
    Body {
        id: u64,
        text: Arc<String>,
    },
}
pub struct MailWorker {
    tx: SyncSender<Request>,
    pub rx: Receiver<Reply>,
    pub generation: Arc<AtomicU64>,
    stop: Arc<AtomicBool>,
    /// Set once the index thread has returned and released the cache database.
    finished: Arc<AtomicBool>,
}
impl MailWorker {
    pub fn spawn(cx: &mut Cx, config: LocalConfig) -> Result<Self, String> {
        Self::spawn_source(
            cx,
            Box::new(AppleMailSource::new(config.root)),
            config.cache,
        )
    }
    /// Providers own discovery and decoding; the index, worker pool and UI are shared.
    pub fn spawn_source(
        cx: &mut Cx,
        source: Box<dyn MailSource>,
        cache: std::path::PathBuf,
    ) -> Result<Self, String> {
        let (tx, requests) = mpsc::sync_channel(4);
        let (out, rx) = mpsc::sync_channel(8);
        let stop = Arc::new(AtomicBool::new(false));
        let generation = Arc::new(AtomicU64::new(0));
        let finished = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let worker_generation = generation.clone();
        let worker_finished = finished.clone();
        let spawner = cx.thread_spawner();
        let pool_spawner = spawner.clone();
        spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("mail-index".into()),
                    priority: CxThreadPriority::UserInitiated,
                    ..Default::default()
                },
                move || {
                    let publish = |reply| {
                        if out.send(reply).is_ok() {
                            SignalToUI::set_ui_signal();
                        }
                    };
                    if let Err(error) = run(
                        source,
                        &cache,
                        pool_spawner,
                        requests,
                        &out,
                        &worker_stop,
                        &worker_generation,
                    ) {
                        if !worker_stop.load(Ordering::Relaxed) {
                            publish(Reply::Error(error));
                        }
                    }
                    // `run` has dropped the index and its pool by now; a
                    // successor may reopen or remove the cache file.
                    worker_finished.store(true, Ordering::Release);
                },
            )
            .map_err(|e| e.to_string())?
            .detach();
        Ok(Self {
            tx,
            rx,
            generation,
            stop,
            finished,
        })
    }
    pub fn send(&self, request: Request) -> Result<(), Request> {
        self.tx.try_send(request).map_err(|e| match e {
            TrySendError::Full(r) | TrySendError::Disconnected(r) => r,
        })
    }
    /// Ask the thread to stop and hand back the flag it raises once it has
    /// let go of the cache. Spawning a successor before that flag is set
    /// races the old writer for the same database file.
    pub fn retire(self) -> Arc<AtomicBool> {
        self.finished.clone()
    }
}
impl Drop for MailWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
fn emit(out: &SyncSender<Reply>, reply: Reply) {
    if out.try_send(reply).is_ok() {
        SignalToUI::set_ui_signal();
    }
}
/// Write one attachment under the user's temporary directory so the system
/// opens it with its default application. The name is reduced to a single
/// path component; the message id keeps same-named files of different
/// messages apart.
fn save_attachment(id: u64, attachment: &MailAttachment) -> Result<std::path::PathBuf, String> {
    let name: String = attachment
        .name
        .chars()
        .map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':') { '_' } else { c })
        .take(120)
        .collect();
    let name = name.trim().trim_start_matches('.').to_owned();
    let name = if name.is_empty() { "attachment".to_owned() } else { name };
    let root = std::env::temp_dir().join("makepad-mail");
    let dir = root.join(id.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| e.to_string())?;
    }
    let path = dir.join(name);
    std::fs::write(&path, &attachment.bytes).map_err(|e| e.to_string())?;
    Ok(path)
}
type DecodeJob = (SourceEntry, TaskHandle<(Duration, Result<PreparedMessage, String>)>);

#[derive(Default)]
struct ScanTiming {
    discovery: Duration,
    store: Duration,
    search: Duration,
    decode: Duration,
    decode_max: Duration,
    batches: usize,
    peak_jobs: usize,
}

// Cancellation belongs to this startup pass. On an error or shutdown, queued
// and running tasks stop too, without suppressing the error sent to the UI.
struct CacheLoadCancel(Arc<AtomicBool>);
impl Drop for CacheLoadCancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}
fn load_cache(
    index: &mut MailIndex,
    cache: &std::path::Path,
    source: &str,
    pool: &TaskPool,
    out: &SyncSender<Reply>,
    stop: &Arc<AtomicBool>,
) -> Result<bool, String> {
    let mut pending: VecDeque<_> = index.cache_ranges(pool.heavy_workers()).into_iter().enumerate().collect();
    let partitions = pending.len();
    let cancel = CacheLoadCancel(Arc::new(AtomicBool::new(false)));
    let progress = Arc::new(AtomicU64::new(0));
    let mut jobs: Vec<(usize, TaskHandle<Result<CachePartition, String>>)> = Vec::new();
    let began = Instant::now();
    let mut status_at = began;
    let mut log_at = began;
    let mut reads = Duration::ZERO;
    let mut builds = Duration::ZERO;
    let mut merge = Duration::ZERO;
    eprintln!("mail-index: cache workers · {partitions} independent partitions");
    while !pending.is_empty() || !jobs.is_empty() {
        if stop.load(Ordering::Relaxed) { return Ok(false); }
        while let Some(&(partition, range)) = pending.front() {
            let Ok(slot) = pool.reserve(Lane::Heavy) else { break; };
            pending.pop_front();
            let path = cache.to_path_buf();
            let source = source.to_owned();
            let progress = progress.clone();
            let stopped = stop.clone();
            let cancelled = cancel.0.clone();
            jobs.push((partition, slot.submit_named("mail-cache-load", move || {
                CachePartition::load(&path, &source, range, &progress, || {
                    stopped.load(Ordering::Relaxed) || cancelled.load(Ordering::Relaxed)
                })
            })));
        }
        for i in (0..jobs.len()).rev() {
            if let Some(result) = jobs[i].1.try_take() {
                let (partition, _) = jobs.swap_remove(i);
                let loaded = result.map_err(|e| format!("Cache worker failed: {e:?}"))??;
                reads += loaded.read_time;
                builds += loaded.build_time;
                let began = Instant::now();
                // No term remapping, trigram rebuild or posting-list merge:
                // retain each finished partition and route future edits to it.
                index.install_partition(partition, loaded);
                merge += began.elapsed();
            }
        }
        let loaded = progress.load(Ordering::Relaxed);
        if status_at.elapsed() >= Duration::from_millis(100) {
            emit(out, Reply::Status(format!("Opening search cache · {loaded} messages · {partitions} workers")));
            status_at = Instant::now();
        }
        if log_at.elapsed() >= Duration::from_secs(1) {
            eprintln!("mail-index: cache loading · {loaded} messages · {:.2}s", began.elapsed().as_secs_f64());
            log_at = Instant::now();
        }
        if !jobs.is_empty() { std::thread::sleep(Duration::from_millis(1)); }
    }
    eprintln!("mail-index: cache phases · {partitions} workers · {:.3}s wall · {:.3}s combined reads · {:.3}s combined builds · {:.3}s install", began.elapsed().as_secs_f64(), reads.as_secs_f64(), builds.as_secs_f64(), merge.as_secs_f64());
    Ok(true)
}

fn run(
    mut source: Box<dyn MailSource>,
    cache: &std::path::Path,
    spawner: ThreadSpawner,
    requests: Receiver<Request>,
    out: &SyncSender<Reply>,
    stop: &Arc<AtomicBool>,
    generation: &Arc<AtomicU64>,
) -> Result<(), String> {
    let began = Instant::now();
    // Check access before exposing cached data for this source.
    source.begin_sync(None)?;
    let mut reader = source.reader();
    let workers = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut options = PoolOptions::with_workers(workers, 0);
    options.name = "mail-decode".into();
    let pool = TaskPool::new_with_priority(spawner, options, CxThreadPriority::UserInitiated)
        .map_err(|e| e.to_string())?;
    source.configure_workers(pool.clone());
    emit(out, Reply::Status("Opening search cache…".into()));
    let mut index = MailIndex::open(cache, source.id())?;
    let opened = Instant::now();
    eprintln!("mail-index: cache opened in {:.3}s", began.elapsed().as_secs_f64());
    if !load_cache(&mut index, cache, source.id(), &pool, out, stop)? {
        return Ok(());
    }
    eprintln!("mail-index: cache ready · {} messages · {:.3}s load · {:.3}s total", index.records.len(), opened.elapsed().as_secs_f64(), began.elapsed().as_secs_f64());
    let mut scan_began = Instant::now();
    let mut timing = ScanTiming::default();
    let mut benchmark_at = Instant::now();
    let mut jobs: Vec<DecodeJob> = Vec::new();
    let mut pending = VecDeque::<SourceEntry>::new();
    let mut completed = Vec::<(SourceEntry, PreparedMessage)>::new();
    let mut completed_bytes = 0usize;
    let mut commit_at = Instant::now();
    let mut seen = HashSet::new();
    let mut discovery_done = false;
    let mut scan_active = true;
    let mut failed = 0;
    let mut unreadable = 0;
    let mut full_inventory = true;
    let mut processed = 0;
    let mut reused = 0;
    let mut query = (0u64, String::new(), 200usize);
    let mut query_dirty = true;
    let mut query_at = Instant::now();
    let mut last_results = Instant::now() - Duration::from_secs(2);
    let mut status_at = Instant::now() - Duration::from_secs(2);
    let mut refresh_at = Instant::now();
    let mut prepared_dirty = false;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        while let Ok(request) = requests.try_recv() {
            match request {
                Request::Search {
                    generation,
                    text,
                    limit,
                } => {
                    query = (generation, text, limit);
                    query_dirty = true;
                    query_at = Instant::now();
                }
                Request::Read(id) => {
                    let text = index.body(id)?;
                    let preview = crate::presentation::preview(&text);
                    if out
                        .send(Reply::Body {
                            id,
                            text: Arc::new(preview),
                        })
                        .is_err()
                    {
                        return Ok(());
                    }
                    SignalToUI::set_ui_signal();
                }
                Request::Attachment { id, index: which } => {
                    let saved = index.item_key(id).and_then(|key| {
                        let entry = SourceEntry { key, revision: String::new() };
                        reader.attachment(&entry, which)
                    }).and_then(|attachment| save_attachment(id, &attachment));
                    let reply = match saved {
                        Ok(path) => Reply::Attachment { id, index: which, path },
                        Err(message) => Reply::AttachmentError { id, index: which, message },
                    };
                    if out.send(reply).is_err() {
                        return Ok(());
                    }
                    SignalToUI::set_ui_signal();
                }
                Request::Refresh => refresh_at = Instant::now() - Duration::from_secs(60),
            }
        }
        if !scan_active && refresh_at.elapsed() >= Duration::from_secs(30) {
            source.begin_sync(None)?;
            reader = source.reader();
            scan_active = true;
            discovery_done = false;
            seen.clear();
            failed = 0;
            unreadable = 0;
            processed = 0;
            reused = 0;
            scan_began = Instant::now();
            timing = ScanTiming::default();
        }
        if scan_active && !discovery_done && pending.len() < workers * 2 {
            let phase = Instant::now();
            let batch = source.next_batch(workers * 16)?;
            timing.discovery += phase.elapsed();
            discovery_done = batch.complete;
            unreadable = batch.unreadable;
            full_inventory = batch.full_inventory;
            for key in batch.removed {
                index.remove(&key)?;
                prepared_dirty = true;
            }
            for entry in batch.entries {
                seen.insert(entry.key.clone());
                if index
                    .keys
                    .get(&entry.key)
                    .is_some_and(|(_, revision)| *revision == entry.revision)
                {
                    reused += 1;
                } else {
                    pending.push_back(entry);
                }
            }
        }

        for i in (0..jobs.len()).rev() {
            if completed.len() >= 128 || completed_bytes >= 32 * 1024 * 1024 { break; }
            if let Some(result) = jobs[i].1.try_take() {
                let (entry, _) = jobs.swap_remove(i);
                match result {
                    Ok((duration, result)) => {
                        timing.decode += duration;
                        timing.decode_max = timing.decode_max.max(duration);
                        match result {
                            Ok(message) => {
                                completed_bytes = completed_bytes.saturating_add(message.staging_bytes());
                                completed.push((entry, message));
                            }
                            Err(_) => failed += 1,
                        }
                    }
                    _ => failed += 1,
                }
            }
        }
        // Refill the decoder lanes before writing, so preparation overlaps
        // the durable writer instead of stopping between every tiny batch.
        while jobs.len() < workers * 2 {
            let Some(entry) = pending.pop_front() else {
                break;
            };
            let reader = reader.clone();
            let task_entry = entry.clone();
            let stopped = stop.clone();
            match pool.try_submit_named(Lane::Heavy, "mail-decode-index", move || {
                let phase = Instant::now();
                let result = if stopped.load(Ordering::Relaxed) {
                    Err("Stopped".into())
                } else {
                    reader.read(&task_entry).map(PreparedMessage::new)
                };
                (phase.elapsed(), result)
            }) {
                Ok(handle) => jobs.push((entry, handle)),
                Err(_) => {
                    pending.push_front(entry);
                    break;
                }
            }
        }
        timing.peak_jobs = timing.peak_jobs.max(jobs.len());
        let drained = discovery_done && pending.is_empty() && jobs.is_empty();
        // Bound staging by count, approximate bytes, and latency. Never open
        // a transaction while waiting for a decoder or a user command.
        if !completed.is_empty() && (completed.len() >= 128
            || completed_bytes >= 32 * 1024 * 1024
            || commit_at.elapsed() >= Duration::from_millis(100) || drained) {
            let phase = Instant::now();
            index.begin()?;
            for (entry, message) in completed.drain(..) {
                index.upsert(&entry.key, &entry.revision, message)?;
                processed += 1;
            }
            index.commit()?;
            timing.store += phase.elapsed();
            timing.batches += 1;
            completed_bytes = 0;
            commit_at = Instant::now();
            prepared_dirty = true;
        }
        if scan_active && drained && completed.is_empty() {
            if full_inventory && unreadable == 0 && failed == 0 {
                let removed: Vec<_> = index
                    .keys
                    .keys()
                    .filter(|key| !seen.contains(*key))
                    .cloned()
                    .collect();
                if !removed.is_empty() {
                    index.begin()?;
                    for key in removed {
                        index.remove(&key)?;
                    }
                    index.commit()?;
                }
            }
            scan_active = false;
            refresh_at = Instant::now();
            prepared_dirty = true;
            let summary = format!("{} indexed · {processed} updated · {reused} cached · {} unreadable · {workers} workers · {:.2}s", index.records.len(), failed + unreadable, began.elapsed().as_secs_f64());
            eprintln!("mail-index: {summary}");
            eprintln!("mail-index: scan timing · {:.3}s wall · {:.3}s discovery · {:.3}s database/index · {:.3}s searches · {:.3}s combined decoder time · {:.1}ms slowest decode · {} batches · {} peak jobs", scan_began.elapsed().as_secs_f64(), timing.discovery.as_secs_f64(), timing.store.as_secs_f64(), timing.search.as_secs_f64(), timing.decode.as_secs_f64(), timing.decode_max.as_secs_f64()*1000.0, timing.batches, timing.peak_jobs);
            if out.send(Reply::ScanDone(summary)).is_err() {
                return Ok(());
            }
            SignalToUI::set_ui_signal();
        }
        if scan_active && benchmark_at.elapsed() > Duration::from_secs(2) {
            eprintln!("mail-index: progress · {processed} updated · {reused} reused · {} discovered · {} in flight · {:.1}s wall · {:.1}s store · {:.1}s decode", seen.len(), jobs.len(), scan_began.elapsed().as_secs_f64(), timing.store.as_secs_f64(), timing.decode.as_secs_f64());
            benchmark_at = Instant::now();
        }
        if scan_active && status_at.elapsed() > Duration::from_millis(250) {
            status_at = Instant::now();
            emit(out, Reply::Status(format!("Indexing · {} discovered · {processed} updated · {reused} cached · {} unreadable · {workers} workers", seen.len(), failed + unreadable)));
        }
        let due = query_dirty && query_at.elapsed() > Duration::from_millis(90)
            || prepared_dirty && !query_dirty && last_results.elapsed() > Duration::from_secs(1);
        if due {
            let expected = query.0;
            let phase = Instant::now();
            let result = index.search(&query.1, query.2, || {
                stop.load(Ordering::Relaxed) || generation.load(Ordering::Relaxed) != expected
            });
            timing.search += phase.elapsed();
            if generation.load(Ordering::Relaxed) == expected {
                match result {
                    Ok(page) => {
                        if out
                            .send(Reply::Results {
                                generation: expected,
                                page: Arc::new(page),
                            })
                            .is_err()
                        {
                            return Ok(());
                        }
                        SignalToUI::set_ui_signal();
                    }
                    Err(message) => {
                        if out
                            .send(Reply::QueryError {
                                generation: expected,
                                message,
                            })
                            .is_err()
                        {
                            return Ok(());
                        }
                        SignalToUI::set_ui_signal();
                    }
                }
            }
            query_dirty = false;
            prepared_dirty = false;
            last_results = Instant::now();
        }
        // Only this coordinator sleeps; neither the UI nor decoder lanes wait.
        if !scan_active {
            std::thread::sleep(Duration::from_millis(20));
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
    Ok(())
}
