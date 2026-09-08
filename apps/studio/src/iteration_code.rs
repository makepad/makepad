// Included by iteration_worker.rs: code-intelligence calls for lanes. Every
// call reaches the host worker through the receipt spool or the loopback
// route, is admitted per lane, answered against the calling lane's own
// analyser (one bounded worker per worktree) and traced. The UI thread never
// takes part: queries run here against a leased immutable graph, and the
// slow comparisons run on one further thread and answer through the spool.
//
// A cursor keeps the graph it was issued against, its revision, context,
// coverage and freshness until it is consumed or evicted: a continuation
// never re-acquires the latest publication. A deferred call is keyed by the
// receipt namespace it was claimed in plus its id, and executes for the
// lane that owns the terminal now (the successor after a split).

use crate::atlas::envelope::{
    coverage_json, coverage_unavailable, fit_rows, format_cursor, redact_value, Admission,
    Envelope as LaneEnvelope, Freshness, LaneError, LaneErrorKind, PAGE_BYTES,
};
use crate::atlas::registry::{self, Base, Executed, LaneCall, LaneOp};
use makepad_code_graph::query::{CoverageView, Cursor as GraphCursor, QueryEngine};
use makepad_code_graph::{
    compare, freeze, worker_with, AnalysisContext, Client as GraphClient, Command as GraphCommand,
    CorpusPolicy, FsSourceSet, GitTreeSourceSet, Graph, Indexer, Reply as GraphReply,
    SourceIdentity,
};
use std::sync::atomic::AtomicUsize;

const CODE_RATE_PER_SECOND: f64 = 5.0;
const CODE_BURST: f64 = 10.0;
const CODE_OUTSTANDING: usize = 2;
const CODE_SESSIONS: usize = 8;
const CODE_ANALYSERS: usize = 3;
/// Base graphs the comparison thread keeps after indexing a commit.
const CODE_BASE_GRAPHS: usize = 1;
/// Graphs the runtime may hold in memory (served, baseline and cursor-held
/// graphs across analysers, plus cached bases) before a new base index is
/// refused: two graphs per lane plus one indexing.
const CODE_RETAINED_GRAPHS: usize = 6;
const CODE_TRACE_LIMIT: u64 = 1024 * 1024;
const CODE_TRACE_PAGE: usize = 64;
const CODE_ANALYSER_IDLE: Duration = Duration::from_secs(15 * 60);
/// The most bytes `--full` collects into one export.
pub const CODE_EXPORT_LIMIT: usize = 8 * 1024 * 1024;
/// How the analyser worker publishes after a source change (see
/// `Freshness::method`): the worker re-freezes the worktree on `Update`; it
/// has no observation-cut command yet, so `Indexer::step_to` is not routed.
const CODE_FRESHNESS_METHOD: &str = "worker_update";

/// The publication stamp a reply speaks from: revision, context, coverage
/// and freshness of one graph.
#[derive(Clone, Debug)]
struct CodeStamp {
    revision: u64,
    context_hash: String,
    coverage: Value,
    freshness: Freshness,
}

/// One bounded analyser worker for one worktree, polled by the host.
struct CodeAnalyser {
    client: Option<GraphClient>,
    stop: Arc<AtomicBool>,
    _task: Option<TaskHandle<()>>,
    graph: Option<Arc<Graph>>,
    /// The immutable snapshot of the lane's first publication (dirty state
    /// included): the `code_arch_diff` baseline.
    baseline: Option<Arc<Graph>>,
    /// Replaced publications still held by cursors; released to the worker
    /// once nothing holds them.
    retired: Vec<Arc<Graph>>,
    engine: QueryEngine,
    published_at_ms: u64,
    /// The lane source revision the served graph was indexed at.
    included_source_revision: Option<u64>,
    /// The lane source revision an index or update in flight was asked for.
    pending_source_revision: Option<u64>,
    started: Instant,
    last_used: Instant,
    last_error: Option<String>,
    stopped: bool,
}

impl CodeAnalyser {
    fn start(
        spawner: &ThreadSpawner,
        root: &Path,
        flow: &str,
        cache_dir: Option<PathBuf>,
        source_revision: u64,
    ) -> Result<Self, String> {
        let policy = CorpusPolicy::default();
        let identity = SourceIdentity {
            workspace_id: root.to_string_lossy().into_owned(),
            worktree_id: flow.into(),
            tree: None,
        };
        let source = FsSourceSet::new(root.to_path_buf(), identity).with_policy(policy.clone());
        let (mut client, mut worker) = worker_with(Box::new(source), policy, cache_dir);
        let stop = Arc::new(AtomicBool::new(false));
        let cancel = stop.clone();
        let task = spawner
            .spawn_worker(
                ThreadOptions {
                    name: Some("studio-lane-index".into()),
                    ..Default::default()
                },
                move || loop {
                    let p = worker.poll();
                    if p.stopped {
                        break;
                    }
                    if !p.did_work {
                        if cancel.load(Ordering::Relaxed)
                            && worker.leased() == 0
                            && worker.retiring() == 0
                        {
                            break;
                        }
                        std::thread::sleep(Duration::from_millis(20));
                    }
                },
            )
            .map_err(|e| e.to_string())?;
        client
            .submit(GraphCommand::Index {
                context: AnalysisContext::host(),
                specs: None,
            })
            .map_err(|_| "analyser command queue is full")?;
        Ok(CodeAnalyser {
            client: Some(client),
            stop,
            _task: Some(task),
            graph: None,
            baseline: None,
            retired: Vec::new(),
            engine: QueryEngine::new(),
            published_at_ms: 0,
            included_source_revision: None,
            pending_source_revision: Some(source_revision),
            started: Instant::now(),
            last_used: Instant::now(),
            last_error: None,
            stopped: false,
        })
    }

    /// An analyser that serves one fixed graph.
    #[cfg(test)]
    fn with_graph(graph: Arc<Graph>, source_revision: u64) -> Self {
        CodeAnalyser {
            client: None,
            stop: Arc::new(AtomicBool::new(false)),
            _task: None,
            graph: Some(graph.clone()),
            baseline: Some(graph),
            retired: Vec::new(),
            engine: QueryEngine::new(),
            published_at_ms: now(),
            included_source_revision: Some(source_revision),
            pending_source_revision: None,
            started: Instant::now(),
            last_used: Instant::now(),
            last_error: None,
            stopped: false,
        }
    }

    /// A later publication replaces the served graph (tests).
    #[cfg(test)]
    fn publish(&mut self, graph: Arc<Graph>, source_revision: u64) {
        self.adopt(graph);
        self.included_source_revision = Some(source_revision);
    }

    fn adopt(&mut self, graph: Arc<Graph>) {
        if let Some(old) = self.graph.replace(graph.clone()) {
            self.retired.push(old);
        }
        if self.baseline.is_none() {
            self.baseline = Some(graph);
        }
        self.published_at_ms = now();
        self.last_error = None;
    }

    fn poll(&mut self) {
        let (replies, graph) = {
            let Some(client) = &mut self.client else {
                return;
            };
            client.flush();
            let mut replies = Vec::new();
            while let Some(reply) = client.try_take_reply() {
                replies.push(reply);
            }
            (replies, client.try_take_graph())
        };
        for reply in replies {
            match reply {
                GraphReply::Error(error) => {
                    self.last_error = Some(error);
                    self.pending_source_revision = None;
                }
                GraphReply::Stopped => self.stopped = true,
                GraphReply::Published(_) | GraphReply::Backpressure { .. } | GraphReply::Query(_) => {}
            }
        }
        if let Some(graph) = graph {
            self.adopt(graph);
            if let Some(revision) = self.pending_source_revision.take() {
                self.included_source_revision = Some(revision);
            }
        }
        // a retired publication nothing holds any more goes back to the worker
        let mut i = 0;
        while i < self.retired.len() {
            if Arc::strong_count(&self.retired[i]) == 1 {
                let old = self.retired.swap_remove(i);
                if let Some(client) = &mut self.client {
                    let _ = client.submit(GraphCommand::Release(old.revision));
                }
            } else {
                i += 1;
            }
        }
    }

    /// Ask for a publication that includes the lane's current sources.
    fn refresh(&mut self, source_revision: u64) {
        if self.pending_source_revision.is_some() || self.included_source_revision == Some(source_revision) {
            return;
        }
        let Some(client) = &mut self.client else {
            return;
        };
        let command = if self.graph.is_some() {
            GraphCommand::Update
        } else {
            GraphCommand::Index {
                context: AnalysisContext::host(),
                specs: None,
            }
        };
        if client.submit(command).is_ok() {
            self.pending_source_revision = Some(source_revision);
        }
    }

    fn stamp_of(&self, graph: &Graph, observed_source_revision: u64) -> CodeStamp {
        CodeStamp {
            revision: graph.revision,
            context_hash: makepad_code_graph::keys::hex(&graph.context.hash()),
            coverage: coverage_json(&CoverageView::of(graph)),
            freshness: Freshness {
                published_at_ms: self.published_at_ms,
                source_digest: makepad_code_graph::keys::hex(&graph.source.digest),
                observed_source_revision: Some(observed_source_revision),
                included_source_revision: self.included_source_revision,
                method: CODE_FRESHNESS_METHOD,
            },
        }
    }

    /// The typed busy error, stamped with the last publication when there
    /// is one and with explicit unavailable coverage when there is none.
    fn busy(&self, observed_source_revision: u64) -> (LaneError, Option<CodeStamp>, Value) {
        let phase = match (&self.graph, &self.last_error) {
            (None, Some(error)) => format!("the first index failed ({error}); it is being retried"),
            (None, None) => format!("the first index has run for {} s", self.started.elapsed().as_secs()),
            (Some(g), _) => format!("revision {} is published; the requested publication is still indexing", g.revision),
        };
        let mut error = LaneError::busy(format!("the analyser is busy: {phase}"), 1500);
        error.resumable = true;
        match &self.graph {
            Some(g) => {
                let stamp = self.stamp_of(g, observed_source_revision);
                let coverage = stamp.coverage.clone();
                (error, Some(stamp), coverage)
            }
            None => (error, None, coverage_unavailable(&format!("no publication yet: {phase}"))),
        }
    }

    fn held_graphs(&self) -> usize {
        self.graph.is_some() as usize + self.baseline.is_some() as usize + self.retired.len()
    }

    fn shutdown(&mut self) {
        if let Some(client) = &mut self.client {
            let _ = client.submit(GraphCommand::Shutdown);
        }
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A retained continuation: the rows a page could not hold, the engine
/// cursor that follows them, the graph they were computed against, and the
/// stamp the first page carried; bound to the exact call that produced them.
struct CodeCursor {
    call: LaneCall,
    rest: Vec<Value>,
    next: Option<GraphCursor>,
    graph: Arc<Graph>,
    root: PathBuf,
    stamp: CodeStamp,
    basis_policy: Vec<String>,
}

/// Admission and retained continuations of one lane.
struct CodeLane {
    admission: Admission,
    cursors: VecDeque<(String, CodeCursor)>,
    serial: u64,
}

impl CodeLane {
    fn new() -> Self {
        CodeLane { admission: Admission::new(CODE_RATE_PER_SECOND, CODE_BURST, CODE_OUTSTANDING), cursors: VecDeque::new(), serial: 0 }
    }

    fn take_cursor(&mut self, text: &str) -> Option<CodeCursor> {
        let index = self.cursors.iter().position(|(t, _)| t == text)?;
        self.cursors.remove(index).map(|(_, c)| c)
    }

    fn keep_cursor(&mut self, cursor: CodeCursor) -> String {
        self.serial += 1;
        let text = format!("c{}-r{}-{:08x}", self.serial, cursor.stamp.revision, code_hash(&format!("{:?}", cursor.call)));
        self.cursors.push_back((text.clone(), cursor));
        while self.cursors.len() > CODE_SESSIONS {
            self.cursors.pop_front();
        }
        text
    }

    fn held_graphs(&self) -> usize {
        self.cursors.len()
    }
}

fn code_hash(text: &str) -> u32 {
    let mut h: u32 = 0x811c9dc5;
    for b in text.bytes() {
        h ^= b as u32;
        h = h.wrapping_mul(0x01000193);
    }
    h
}

/// The base of a deferred comparison.
enum CodeBase {
    Tree { root: PathBuf, commit: String, cache_dir: Option<PathBuf> },
    Graph(Arc<Graph>),
}

struct CodeCompareJob {
    job: u64,
    base: CodeBase,
    now_graph: Arc<Graph>,
    scope: Option<String>,
    limit: usize,
}

struct CodeCompareDone {
    job: u64,
    result: Result<(Vec<Value>, bool, Vec<String>), LaneError>,
}

/// Comparisons index a second graph, which takes seconds: they run on one
/// long-lived thread and answer through the spool when done.
enum CodeCompare {
    Thread {
        jobs: SyncSender<CodeCompareJob>,
        done: Receiver<CodeCompareDone>,
        cached: Arc<AtomicUsize>,
        _task: TaskHandle<()>,
    },
    /// Without a spawner (tests) the job runs at submission.
    Inline {
        cache: VecDeque<(String, Arc<Graph>)>,
        done: VecDeque<CodeCompareDone>,
    },
}

impl CodeCompare {
    fn submit(&mut self, job: CodeCompareJob) -> Result<(), LaneError> {
        match self {
            CodeCompare::Thread { jobs, .. } => jobs
                .try_send(job)
                .map_err(|_| LaneError::busy("the comparison queue is full; try again shortly", 2000)),
            CodeCompare::Inline { cache, done } => {
                let id = job.job;
                let result = code_compare_job(&job, cache);
                done.push_back(CodeCompareDone { job: id, result });
                Ok(())
            }
        }
    }

    fn drain(&mut self) -> Vec<CodeCompareDone> {
        match self {
            CodeCompare::Thread { done, .. } => {
                let mut out = Vec::new();
                while let Ok(d) = done.try_recv() {
                    out.push(d);
                }
                out
            }
            CodeCompare::Inline { done, .. } => done.drain(..).collect(),
        }
    }

    fn cached_graphs(&self) -> usize {
        match self {
            CodeCompare::Thread { cached, .. } => cached.load(Ordering::Relaxed),
            CodeCompare::Inline { cache, .. } => cache.len(),
        }
    }
}

fn code_compare_thread(jobs: Receiver<CodeCompareJob>, done: SyncSender<CodeCompareDone>, cached: Arc<AtomicUsize>) {
    let mut cache: VecDeque<(String, Arc<Graph>)> = VecDeque::new();
    while let Ok(job) = jobs.recv() {
        let id = job.job;
        let result = code_compare_job(&job, &mut cache);
        cached.store(cache.len(), Ordering::Relaxed);
        if done.send(CodeCompareDone { job: id, result }).is_err() {
            break;
        }
    }
}

/// Both graphs must speak the same analysis: the requested context and the
/// analyser version, else the comparison is meaningless.
fn code_validate_stamps(base: &Graph, now: &Graph) -> Result<(), LaneError> {
    if base.requested_context.hash() != now.requested_context.hash() {
        return Err(LaneError::new(LaneErrorKind::ContextMismatch, format!("the base was analysed under context {} and this worktree under {}; comparisons need one requested context", makepad_code_graph::keys::hex(&base.requested_context.hash()), makepad_code_graph::keys::hex(&now.requested_context.hash()))));
    }
    if base.analyser_version != now.analyser_version {
        return Err(LaneError::new(LaneErrorKind::ContextMismatch, format!("the base was analysed by analyser version {} and this worktree by {}", base.analyser_version, now.analyser_version)));
    }
    Ok(())
}

fn code_compare_job(
    job: &CodeCompareJob,
    cache: &mut VecDeque<(String, Arc<Graph>)>,
) -> Result<(Vec<Value>, bool, Vec<String>), LaneError> {
    let base = match &job.base {
        CodeBase::Graph(graph) => graph.clone(),
        CodeBase::Tree { root, commit, cache_dir } => code_base_graph(root, commit, cache_dir.clone(), cache)?,
    };
    code_validate_stamps(&base, &job.now_graph)?;
    let delta = compare(&base, &job.now_graph)
        .map_err(|e| LaneError::new(LaneErrorKind::ContextMismatch, format!("the two graphs cannot be compared: {e}")))?;
    let (mut rows, truncated, mut notes) = registry::delta_rows(&delta, job.limit);
    if let Some(scope) = &job.scope {
        let prefix = registry::resolve_selector(&job.now_graph, scope)
            .or_else(|_| registry::resolve_selector(&base, scope))
            .map(|id| job.now_graph.qualified(id).to_string())
            .unwrap_or_else(|_| scope.clone());
        let before = rows.len();
        rows.retain(|row| {
            ["name", "from", "to"].iter().any(|field| {
                row.get(field).and_then(Value::as_str).is_some_and(|n| n.starts_with(prefix.as_str()) || n.contains(scope.as_str()))
            })
        });
        notes.push(format!("{} of {} delta rows lie under {scope}; the counts above are for the whole graph", rows.len(), before));
    }
    notes.push(format!(
        "base revision {} ({} files, context {}) against revision {} ({} files)",
        base.revision,
        base.source.files.len(),
        &makepad_code_graph::keys::hex(&base.context.hash())[..12],
        job.now_graph.revision,
        job.now_graph.source.files.len()
    ));
    Ok((rows, truncated, notes))
}

/// Index a committed tree of the lane's repository through `makepad_git`,
/// never a checkout. The last base graph stays cached.
fn code_base_graph(
    root: &Path,
    commit: &str,
    cache_dir: Option<PathBuf>,
    cache: &mut VecDeque<(String, Arc<Graph>)>,
) -> Result<Arc<Graph>, LaneError> {
    let key = format!("{}@{commit}", root.display());
    if let Some((_, graph)) = cache.iter().find(|(k, _)| *k == key) {
        return Ok(graph.clone());
    }
    let mut repo = makepad_git::Repository::open(root)
        .map_err(|e| LaneError::new(LaneErrorKind::Unknown, format!("the lane repository cannot be opened: {e}")))?;
    let oid = makepad_git::ObjectId::from_hex(commit)
        .map_err(|e| LaneError::invalid(format!("base.commit is not an object id: {e}")))?;
    let tree = repo
        .read_commit(&oid)
        .map_err(|e| LaneError::new(LaneErrorKind::UnknownKey, format!("commit {commit} is not readable in this repository: {e}")))?
        .tree;
    let policy = CorpusPolicy::default();
    let mut set = GitTreeSourceSet::new(repo, tree)
        .map_err(|e| LaneError::new(LaneErrorKind::Unknown, format!("the base tree cannot be read: {e:?}")))?
        .with_policy(policy.clone());
    let frozen = freeze(&mut set, &policy)
        .map_err(|e| LaneError::new(LaneErrorKind::Unknown, format!("the base tree cannot be frozen: {e:?}")))?;
    let mut indexer = Indexer::new(cache_dir);
    let graph = indexer
        .index(frozen, AnalysisContext::host())
        .map_err(|e| LaneError::new(LaneErrorKind::Unknown, format!("the base tree cannot be indexed: {e:?}")))?;
    while cache.len() >= CODE_BASE_GRAPHS {
        cache.pop_front();
    }
    cache.push_back((key, graph.clone()));
    Ok(graph)
}

/// A call whose answer arrives later through the spool. `namespace` is the
/// receipt directory's flow (where the reply is published); `owner` is the
/// lane that executes it (the successor after a split).
struct CodeDeferred {
    namespace: String,
    owner: String,
    control: PathBuf,
    claimed: Option<PathBuf>,
    call: LaneCall,
    args: Value,
    started: Instant,
    envelope: LaneEnvelope,
    graph: Arc<Graph>,
    root: PathBuf,
}

/// Which worktree a call is about, resolved by the host from the lane table.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CodeTarget {
    flow: String,
    root: PathBuf,
    source_revision: u64,
}

enum CodeOutcome {
    Answered(LaneEnvelope),
    Deferred,
}

/// The lane code runtime owned by the host.
struct CodeRuntime {
    spawner: Option<ThreadSpawner>,
    cache_dir: Option<PathBuf>,
    analysers: BTreeMap<PathBuf, CodeAnalyser>,
    lanes: BTreeMap<String, CodeLane>,
    /// Deferred calls by (receipt namespace, request id).
    deferred: BTreeMap<(String, String), CodeDeferred>,
    /// Comparison job numbers back to their deferred key.
    jobs: BTreeMap<u64, (String, String)>,
    next_job: u64,
    compare: Option<CodeCompare>,
}

impl CodeRuntime {
    fn new(spawner: Option<ThreadSpawner>, cache_dir: Option<PathBuf>) -> Self {
        CodeRuntime {
            spawner,
            cache_dir,
            analysers: BTreeMap::new(),
            lanes: BTreeMap::new(),
            deferred: BTreeMap::new(),
            jobs: BTreeMap::new(),
            next_job: 1,
            compare: None,
        }
    }

    fn lane(&mut self, flow: &str) -> &mut CodeLane {
        self.lanes.entry(flow.to_owned()).or_insert_with(CodeLane::new)
    }

    /// Graphs held in memory across the runtime.
    fn retained_graphs(&self) -> usize {
        self.analysers.values().map(CodeAnalyser::held_graphs).sum::<usize>()
            + self.lanes.values().map(CodeLane::held_graphs).sum::<usize>()
            + self.compare.as_ref().map_or(0, CodeCompare::cached_graphs)
    }

    /// The analyser of a worktree, started on first use; the least recently
    /// used one is retired when more than a few are alive.
    fn analyser(&mut self, target: &CodeTarget) -> Result<&mut CodeAnalyser, LaneError> {
        if !self.analysers.contains_key(&target.root) {
            let Some(spawner) = &self.spawner else {
                return Err(LaneError::new(LaneErrorKind::Unavailable, "no analyser can be started for this worktree"));
            };
            if self.analysers.len() >= CODE_ANALYSERS {
                if let Some(oldest) = self.analysers.iter().min_by_key(|(_, a)| a.last_used).map(|(k, _)| k.clone()) {
                    if let Some(mut a) = self.analysers.remove(&oldest) {
                        a.shutdown();
                    }
                }
            }
            let cache = self.cache_dir.as_ref().map(|d| d.join(&target.flow));
            let analyser = CodeAnalyser::start(spawner, &target.root, &target.flow, cache, target.source_revision)
                .map_err(|e| LaneError::new(LaneErrorKind::Unavailable, format!("the analyser could not start: {e}")))?;
            self.analysers.insert(target.root.clone(), analyser);
        }
        let analyser = self.analysers.get_mut(&target.root).expect("inserted above");
        analyser.poll();
        analyser.refresh(target.source_revision);
        analyser.last_used = Instant::now();
        Ok(analyser)
    }

    fn compare(&mut self) -> &mut CodeCompare {
        if self.compare.is_none() {
            self.compare = Some(match &self.spawner {
                Some(spawner) => {
                    let (jobs, rx) = mpsc::sync_channel(CODE_OUTSTANDING * 2);
                    let (tx, done) = mpsc::sync_channel(CODE_OUTSTANDING * 8);
                    let cached = Arc::new(AtomicUsize::new(0));
                    let counter = cached.clone();
                    match spawner.spawn_worker(
                        ThreadOptions { name: Some("studio-lane-compare".into()), ..Default::default() },
                        move || code_compare_thread(rx, tx, counter),
                    ) {
                        Ok(task) => CodeCompare::Thread { jobs, done, cached, _task: task },
                        Err(_) => CodeCompare::Inline { cache: VecDeque::new(), done: VecDeque::new() },
                    }
                }
                None => CodeCompare::Inline { cache: VecDeque::new(), done: VecDeque::new() },
            });
        }
        self.compare.as_mut().expect("set above")
    }

    /// Serve one admitted, parsed call. `base` is the resolved worktree of a
    /// `code_arch_diff` against another lane.
    #[allow(clippy::too_many_arguments)]
    fn serve(
        &mut self,
        owner: &str,
        namespace: &str,
        id: &str,
        control: &Path,
        claimed: Option<PathBuf>,
        args: &Value,
        call: LaneCall,
        target: &CodeTarget,
        base: Option<CodeTarget>,
    ) -> CodeOutcome {
        let started = Instant::now();
        let tool = call.tool.clone();
        let worktree = target.flow.clone();
        let basis_policy: Vec<String> = call.common.basis_policy.iter().map(|b| b.as_str().to_string()).collect();
        let mut call = call;
        // a continuation serves the retained rows first, then the engine
        // cursor, against the graph the cursor was issued for
        if let Some(text) = call.common.cursor.take() {
            let Some(stored) = self.lane(owner).take_cursor(&text) else {
                let mut error = LaneError::new(LaneErrorKind::StaleCursor, "this cursor was consumed, evicted or never issued to this lane; repeat the call without it");
                error.resumable = false;
                return CodeOutcome::Answered(LaneEnvelope::failed(id, &tool, &worktree, error));
            };
            if stored.call != call {
                return CodeOutcome::Answered(LaneEnvelope::failed(id, &tool, &worktree, LaneError::new(LaneErrorKind::StaleCursor, "the cursor belongs to a different call; pass the same tool and arguments plus the cursor")));
            }
            let mut envelope = code_envelope(id, &tool, &worktree, &stored.stamp, stored.basis_policy.clone());
            envelope.notes.push(format!("continuation of revision {}; the lane's sources may have changed since that publication", stored.stamp.revision));
            let graph = stored.graph.clone();
            let executed = if !stored.rest.is_empty() {
                Executed { rows: stored.rest, next: stored.next, truncated: false, notes: Vec::new(), text: None }
            } else if let Some(next) = stored.next {
                let Some(analyser) = self.analysers.get_mut(&stored.root) else {
                    return CodeOutcome::Answered(LaneEnvelope::failed(id, &tool, &worktree, LaneError::new(LaneErrorKind::StaleCursor, "the analyser that issued this cursor was retired; repeat the call without it")));
                };
                call.common.cursor = Some(format_cursor(&next));
                match registry::execute(&graph, &mut analyser.engine, &call) {
                    Ok(e) => e,
                    Err(error) => {
                        envelope.error = Some(error);
                        return CodeOutcome::Answered(envelope);
                    }
                }
            } else {
                Executed::default()
            };
            if let Err(error) = self.finish(owner, &mut envelope, executed, call, graph, stored.root, stored.stamp, basis_policy) {
                envelope.error = Some(error);
            }
            return CodeOutcome::Answered(envelope);
        }
        let (graph, stamp) = {
            let analyser = match self.analyser(target) {
                Ok(a) => a,
                Err(e) => return CodeOutcome::Answered(LaneEnvelope::failed(id, &tool, &worktree, e)),
            };
            let Some(graph) = analyser.graph.clone() else {
                let (error, stamp, coverage) = analyser.busy(target.source_revision);
                return CodeOutcome::Answered(code_busy_envelope(id, &tool, &worktree, error, stamp, coverage));
            };
            let stamp = analyser.stamp_of(&graph, target.source_revision);
            if let Some(after) = call.common.after_save {
                if analyser.included_source_revision.map_or(true, |included| included < after) {
                    let (mut error, stamp, coverage) = analyser.busy(target.source_revision);
                    error.message = format!("source revision {after} is not included yet (included: {}); {}", analyser.included_source_revision.map(|r| r.to_string()).unwrap_or_else(|| "none".into()), error.message);
                    return CodeOutcome::Answered(code_busy_envelope(id, &tool, &worktree, error, stamp, coverage));
                }
            }
            (graph, stamp)
        };
        let mut envelope = code_envelope(id, &tool, &worktree, &stamp, basis_policy.clone());
        if let LaneOp::ArchDiff { base: kind, scope } = &call.op {
            let base = match kind {
                Base::Baseline => {
                    let analyser = self.analysers.get(&target.root).expect("served above");
                    match &analyser.baseline {
                        Some(b) => {
                            envelope.notes.push(format!("baseline = the lane's first publication (revision {}, {} files, dirty state included at lane start)", b.revision, b.source.files.len()));
                            CodeBase::Graph(b.clone())
                        }
                        None => {
                            let (error, stamp, coverage) = analyser.busy(target.source_revision);
                            return CodeOutcome::Answered(code_busy_envelope(id, &tool, &worktree, error, stamp, coverage));
                        }
                    }
                }
                Base::Commit(commit) => {
                    let retained = self.retained_graphs();
                    if retained >= CODE_RETAINED_GRAPHS {
                        let mut error = LaneError::new(LaneErrorKind::BudgetExceeded, format!("indexing a base would exceed the graph memory budget ({retained} of {CODE_RETAINED_GRAPHS} graphs retained); finish or drop paged answers and try again"));
                        error.retry_after_ms = Some(5000);
                        error.resumable = true;
                        envelope.error = Some(error);
                        return CodeOutcome::Answered(envelope);
                    }
                    CodeBase::Tree { root: target.root.clone(), commit: commit.clone(), cache_dir: self.cache_dir.as_ref().map(|d| d.join("base")) }
                }
                Base::Worktree { revision, .. } => {
                    let Some(base) = base else {
                        envelope.error = Some(LaneError::new(LaneErrorKind::PermissionDenied, "base.worktree is not an active lane of this Studio"));
                        return CodeOutcome::Answered(envelope);
                    };
                    let other = match self.analyser(&base) {
                        Ok(a) => a,
                        Err(e) => {
                            envelope.error = Some(e);
                            return CodeOutcome::Answered(envelope);
                        }
                    };
                    let Some(g) = other.graph.clone() else {
                        let (error, stamp, coverage) = other.busy(base.source_revision);
                        return CodeOutcome::Answered(code_busy_envelope(id, &tool, &worktree, error, stamp, coverage));
                    };
                    if let Some(wanted) = revision {
                        if *wanted != g.revision {
                            envelope.error = Some(LaneError::new(LaneErrorKind::StaleRevision, format!("lane {} serves revision {}, not {wanted}", base.flow, g.revision)));
                            return CodeOutcome::Answered(envelope);
                        }
                    }
                    if let Err(e) = code_validate_stamps(&g, &graph) {
                        envelope.error = Some(e);
                        return CodeOutcome::Answered(envelope);
                    }
                    envelope.notes.push(format!("base = lane {} at revision {}", base.flow, g.revision));
                    CodeBase::Graph(g)
                }
            };
            if let Err(error) = self.lane(owner).admission.admit() {
                envelope.error = Some(error);
                return CodeOutcome::Answered(envelope);
            }
            let job = self.next_job;
            self.next_job += 1;
            if let Err(e) = self.compare().submit(CodeCompareJob { job, base, now_graph: graph.clone(), scope: scope.clone(), limit: call.common.entities as usize * 4 }) {
                envelope.error = Some(e);
                return CodeOutcome::Answered(envelope);
            }
            self.lane(owner).admission.begin();
            self.jobs.insert(job, (namespace.into(), id.into()));
            self.deferred.insert(
                (namespace.into(), id.into()),
                CodeDeferred { namespace: namespace.into(), owner: owner.into(), control: control.to_path_buf(), claimed, call, args: args.clone(), started, envelope, graph, root: target.root.clone() },
            );
            return CodeOutcome::Deferred;
        }
        // a synchronous call counts as outstanding while it executes
        self.lane(owner).admission.begin();
        let analyser = self.analysers.get_mut(&target.root).expect("served above");
        let executed = registry::execute(&graph, &mut analyser.engine, &call);
        self.lane(owner).admission.end();
        match executed {
            Ok(executed) => {
                if let Err(error) = self.finish(owner, &mut envelope, executed, call, graph, target.root.clone(), stamp, basis_policy) {
                    envelope.error = Some(error);
                }
            }
            Err(error) => envelope.error = Some(error),
        }
        CodeOutcome::Answered(envelope)
    }

    /// Byte-budget the rows into the envelope (the complete serialised
    /// envelope stays within `PAGE_BYTES`) and retain the remainder under a
    /// lane cursor that keeps the graph and stamp it was computed from.
    #[allow(clippy::too_many_arguments)]
    fn finish(&mut self, owner: &str, envelope: &mut LaneEnvelope, executed: Executed, mut call: LaneCall, graph: Arc<Graph>, root: PathBuf, stamp: CodeStamp, basis_policy: Vec<String>) -> Result<(), LaneError> {
        // the retained call is the one without any cursor: continuations
        // compare against it after their own cursor is taken
        call.common.cursor = None;
        let page_bytes = call.common.max_output_bytes.min(PAGE_BYTES);
        let mut rows = executed.rows;
        if let Some(text) = executed.text {
            rows.insert(0, json::obj(vec![("row", s("brief")), ("text", s(text))]));
        }
        envelope.notes.extend(executed.notes);
        envelope.rows.clear();
        envelope.cursor = Some("c0000000000-r0000000000-00000000".into());
        let overhead = envelope.json().to_json().len() + 32;
        envelope.cursor = None;
        let budget = page_bytes.checked_sub(overhead).filter(|b| *b >= 512)
            .ok_or_else(|| LaneError::new(LaneErrorKind::BudgetExceeded, "publication metadata leaves no room in max_output_bytes; increase it or narrow the scope"))?;
        let (page, mut rest) = fit_rows(rows, budget)?;
        envelope.rows = page;
        // the complete envelope, not only its rows, stays within the page
        while envelope.json().to_json().len() > page_bytes && envelope.rows.len() > 1 {
            let row = envelope.rows.pop().expect("non-empty");
            rest.insert(0, row);
        }
        if envelope.json().to_json().len() > page_bytes {
            envelope.rows.clear();
            return Err(LaneError::new(LaneErrorKind::BudgetExceeded, format!("one row plus the envelope exceeds the {page_bytes}-byte page; increase max_output_bytes, narrow the scope or lower budget.sites")));
        }
        envelope.truncated = executed.truncated || !rest.is_empty() || executed.next.is_some();
        if !rest.is_empty() || executed.next.is_some() {
            let cursor = self.lane(owner).keep_cursor(CodeCursor { call, rest, next: executed.next, graph, root, stamp, basis_policy });
            envelope.cursor = Some(cursor);
        }
        Ok(())
    }

    /// Drain finished comparisons and retire idle analysers.
    fn poll(&mut self) -> Vec<(CodeDeferred, LaneEnvelope)> {
        for analyser in self.analysers.values_mut() {
            analyser.poll();
        }
        let idle: Vec<PathBuf> = self
            .analysers
            .iter()
            .filter(|(_, a)| a.last_used.elapsed() > CODE_ANALYSER_IDLE || a.stopped)
            .map(|(k, _)| k.clone())
            .collect();
        for root in idle {
            if let Some(mut a) = self.analysers.remove(&root) {
                a.shutdown();
            }
        }
        let mut out = Vec::new();
        let done = match &mut self.compare {
            Some(c) => c.drain(),
            None => Vec::new(),
        };
        for d in done {
            let Some(key) = self.jobs.remove(&d.job) else {
                continue;
            };
            let Some(deferred) = self.deferred.remove(&key) else {
                continue;
            };
            if let Some(lane) = self.lanes.get_mut(&deferred.owner) {
                lane.admission.end();
            }
            let mut envelope = deferred.envelope.clone();
            match d.result {
                Ok((rows, truncated, notes)) => {
                    let stamp = CodeStamp { revision: envelope.revision.unwrap_or(0), context_hash: envelope.analysis_context_hash.clone().unwrap_or_default(), coverage: envelope.coverage.clone(), freshness: envelope.freshness.clone().unwrap_or_default() };
                    let basis_policy = envelope.basis_policy.clone();
                    if let Err(error) = self.finish(&deferred.owner, &mut envelope, Executed { rows, next: None, truncated, notes, text: None }, deferred.call.clone(), deferred.graph.clone(), deferred.root.clone(), stamp, basis_policy) {
                        envelope.error = Some(error);
                    }
                }
                Err(error) => envelope.error = Some(error),
            }
            out.push((deferred, envelope));
        }
        out
    }

    fn shutdown(&mut self) {
        for analyser in self.analysers.values_mut() {
            analyser.shutdown();
        }
        self.compare = None;
    }
}

fn code_envelope(id: &str, tool: &str, worktree: &str, stamp: &CodeStamp, basis_policy: Vec<String>) -> LaneEnvelope {
    let mut envelope = LaneEnvelope::empty(id, tool, worktree);
    envelope.revision = Some(stamp.revision);
    envelope.analysis_context_hash = Some(stamp.context_hash.clone());
    envelope.coverage = stamp.coverage.clone();
    envelope.freshness = Some(stamp.freshness.clone());
    envelope.basis_policy = basis_policy;
    envelope
}

/// A busy reply keeps the last publication's stamp when there is one, and
/// says explicitly that coverage is unavailable when there is none.
fn code_busy_envelope(id: &str, tool: &str, worktree: &str, error: LaneError, stamp: Option<CodeStamp>, coverage: Value) -> LaneEnvelope {
    let mut envelope = match stamp {
        Some(stamp) => {
            let mut e = code_envelope(id, tool, worktree, &stamp, Vec::new());
            e.notes.push(format!("the stamp above is the last publication (revision {}); the requested one is not served yet", stamp.revision));
            e
        }
        None => LaneEnvelope::empty(id, tool, worktree),
    };
    envelope.coverage = coverage;
    envelope.error = Some(error);
    envelope
}

/// Append one `StudioCall` record to the lane's durable trace. The trace is
/// bounded by rotation; every record is redacted before it is written.
fn code_trace(control: &Path, record: Value) -> Result<(), String> {
    let path = control.join("calls.jsonl");
    if let Ok(meta) = fs::symlink_metadata(&path) {
        if !meta.file_type().is_file() {
            return Err("the lane trace is not a regular file".into());
        }
        if meta.len() > CODE_TRACE_LIMIT {
            fs::rename(&path, control.join("calls.1.jsonl")).map_err(err)?;
        }
    }
    let line = redact_value(&record).to_json();
    let mut options = OpenOptions::new();
    options.append(true).create(true);
    #[cfg(any(target_os = "macos", target_os = "ios"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x100).mode(0o600); // O_NOFOLLOW
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(0x20000).mode(0o600); // O_NOFOLLOW
    }
    let mut file = options.open(&path).map_err(err)?;
    file.write_all(line.as_bytes()).map_err(err)?;
    file.write_all(b"\n").map_err(err)
}

/// The newest records of the lane trace, oldest first.
pub fn code_trace_read(control: &Path, limit: usize) -> Result<Vec<Value>, String> {
    let path = control.join("calls.jsonl");
    if !path.try_exists().map_err(err)? {
        return Ok(Vec::new());
    }
    let text = cli_read(&path, CODE_TRACE_LIMIT as usize * 2)?;
    let mut records: VecDeque<Value> = VecDeque::new();
    for line in text.lines() {
        if let Ok(value) = json::parse_depth(line.as_bytes(), 16) {
            records.push_back(value);
            while records.len() > limit {
                records.pop_front();
            }
        }
    }
    Ok(records.into())
}

fn code_record(id: &str, tool: &str, args: &Value, worktree: &str, outcome: &str, envelope: &LaneEnvelope, duration: Duration) -> Value {
    let mut fields = vec![
        ("at_ms", Value::Int(now() as i64)),
        ("request_id", s(id)),
        ("tool", s(tool)),
        ("args", redact_value(args)),
        ("worktree", s(worktree)),
        ("outcome", s(outcome)),
        ("result_ref", s(format!("replies/{id}.json"))),
        ("revision", envelope.revision.map(|r| Value::Int(r.min(i64::MAX as u64) as i64)).unwrap_or(Value::Null)),
        ("duration_ms", Value::Int(duration.as_millis().min(i64::MAX as u128) as i64)),
        ("truncated", Value::Bool(envelope.truncated)),
        ("rows", Value::Int(envelope.rows.len() as i64)),
        (
            "result_bytes",
            Value::Int(envelope.json().to_json().len() as i64),
        ),
    ];
    if let Some(error) = &envelope.error {
        fields.push(("error", s(error.kind.as_str())));
    }
    json::obj(fields)
}

impl Host {
    /// Which lane worktree a call addresses: the caller's own, or another
    /// active lane of this Studio named by `worktree`.
    fn code_target(&self, caller: &str, requested: Option<&str>) -> Result<CodeTarget, LaneError> {
        let id = requested.unwrap_or(caller);
        let flow = self
            .engine
            .flows
            .get(id)
            .ok_or_else(|| LaneError::new(LaneErrorKind::PermissionDenied, format!("{id} is not a lane of this Studio")))?;
        if id != caller && (flow.lifecycle == iteration::FlowLifecycle::Archived || flow.successor.is_some()) {
            return Err(LaneError::new(LaneErrorKind::PermissionDenied, format!("{id} is not an active lane")));
        }
        let root = flow
            .worktree
            .clone()
            .ok_or_else(|| LaneError::new(LaneErrorKind::Unknown, format!("lane {id} has no local worktree yet")))?;
        Ok(CodeTarget { flow: id.into(), root, source_revision: flow.source_revision })
    }

    /// Admit, parse, scope and serve one code call. `namespace` is the flow
    /// whose receipt directory holds the request; `owner` executes it. Every
    /// answered call leaves a trace record; deferred calls leave theirs when
    /// they complete.
    #[allow(clippy::too_many_arguments)]
    fn code_call(&mut self, owner: &str, namespace: &str, id: &str, control: &Path, claimed: Option<PathBuf>, tool: &str, args: &Value) -> CodeOutcome {
        let started = Instant::now();
        let mut outcome = self.code_call_inner(owner, namespace, id, control, claimed, tool, args);
        if let CodeOutcome::Answered(envelope) = &mut outcome {
            let bytes = args
                .get("max_output_bytes")
                .and_then(Value::as_u64)
                .filter(|b| (4096..=PAGE_BYTES as u64).contains(b))
                .unwrap_or(PAGE_BYTES as u64) as usize;
            envelope.enforce_limit(bytes);
            let status = if envelope.is_error() { "error" } else { "ok" };
            let record = code_record(id, tool, args, &envelope.worktree, status, envelope, started.elapsed());
            if let Err(error) = code_trace(control, record) {
                self.note = format!("{owner}: lane trace: {error}");
                self.changed = true;
            }
        }
        outcome
    }

    #[allow(clippy::too_many_arguments)]
    fn code_call_inner(&mut self, owner: &str, namespace: &str, id: &str, control: &Path, claimed: Option<PathBuf>, tool: &str, args: &Value) -> CodeOutcome {
        let failed = |error: LaneError| CodeOutcome::Answered(LaneEnvelope::failed(id, tool, owner, error));
        if !registry::lane_offers(tool, false) {
            return failed(match registry::lane_tool(tool) {
                Some(t) if t.family == registry::Family::View => LaneError::new(LaneErrorKind::PermissionDenied, format!("{tool} drives the user's view; lanes are not granted view control")),
                Some(_) => LaneError::new(LaneErrorKind::Unavailable, format!("{tool} is not available yet")),
                None => LaneError::invalid(format!("{tool} is not a code-intelligence tool")),
            });
        }
        let call = match registry::parse_lane_call(tool, args) {
            Ok(call) => call,
            Err(error) => return failed(error),
        };
        // comparisons are admitted when they are queued, once their base is known
        if !matches!(call.op, LaneOp::ArchDiff { .. }) {
            if let Err(error) = self.code.lane(owner).admission.admit() {
                return failed(error);
            }
        }
        let target = match self.code_target(owner, call.common.worktree.as_deref()) {
            Ok(t) => t,
            Err(error) => return failed(error),
        };
        let base = match &call.op {
            LaneOp::ArchDiff { base: Base::Worktree { flow, .. }, .. } => match self.code_target(owner, Some(flow)) {
                Ok(t) => Some(t),
                Err(error) => return failed(error),
            },
            _ => None,
        };
        self.code.serve(owner, namespace, id, control, claimed, args, call, &target, base)
    }

    /// Finish deferred calls: answer through the spool in the receipt
    /// namespace, trace, release the claimed request file.
    fn poll_code(&mut self) {
        for (deferred, mut envelope) in self.code.poll() {
            envelope.enforce_limit(deferred.call.common.max_output_bytes);
            let status = if envelope.is_error() { "error" } else { "ok" };
            let record = code_record(&envelope.request_id, &deferred.call.tool, &deferred.args, &envelope.worktree, status, &envelope, deferred.started.elapsed());
            let result = (|| {
                cli_answer(&deferred.control, &deferred.namespace, &envelope.request_id, status, Ok(envelope.json()))?;
                code_trace(&deferred.control, record)?;
                if let Some(claimed) = &deferred.claimed {
                    if claimed.try_exists().map_err(err)? {
                        fs::remove_file(claimed).map_err(err)?;
                    }
                }
                Ok::<_, String>(())
            })();
            if let Err(error) = result {
                self.note = format!("{}: code call {}: {error}", deferred.owner, envelope.request_id);
                self.changed = true;
            }
        }
    }
}

#[cfg(test)]
impl Host {
    /// A host over an engine, for headless tests of lane scoping.
    fn for_tests(engine: Engine) -> Host {
        let (embedding_commands, _keep) = mpsc::sync_channel(1);
        std::mem::forget(_keep);
        Host {
            directory: std::env::temp_dir(),
            engine,
            seen_feedback: BTreeSet::new(),
            effects: VecDeque::new(),
            cli_archive_cursor: 0,
            builds: BTreeMap::new(),
            apps: BTreeMap::new(),
            test_operations: BTreeMap::new(),
            lifecycle_pending: BTreeMap::new(),
            terminal_busy: BTreeSet::new(),
            fingerprints: BTreeMap::new(),
            reports: BTreeMap::new(),
            storage_error: None,
            attachments: Vec::new(),
            previews: BTreeMap::new(),
            preview_failed: BTreeSet::new(),
            terminal_heights: BTreeMap::new(),
            lane_widths: BTreeMap::new(),
            recordings: RecordingState::default(),
            full_preview: None,
            full_preview_selection: None,
            full_preview_stamp: None,
            full_preview_error: None,
            design_snapshots: BTreeMap::new(),
            embedding_commands,
            embedding_port: None,
            embedding_pending: BTreeMap::new(),
            embedding_unregister: BTreeSet::new(),
            code: CodeRuntime::new(None, None),
            note: String::new(),
            changed: false,
        }
    }
}

#[cfg(test)]
mod code_tests {
    use super::*;
    use crate::iteration::{Command as FlowCommand, FlowConfig, TestScope};
    use makepad_strict_json as json;
    use std::sync::atomic::AtomicU64;

    fn fixture_graph(name: &str) -> Arc<Graph> {
        fixture_sequence(&[name]).remove(0)
    }

    /// Fixtures indexed by one indexer in order, so revisions advance.
    fn fixture_sequence(names: &[&str]) -> Vec<Arc<Graph>> {
        let mut ix = Indexer::new(None);
        names
            .iter()
            .map(|name| {
                let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../libs/code_atlas/tests/fixtures").join(name).canonicalize().unwrap();
                let policy = CorpusPolicy::default();
                let identity = SourceIdentity { workspace_id: "lane-fixture".into(), worktree_id: (*name).into(), tree: None };
                let mut set = FsSourceSet::new(root, identity).with_policy(policy.clone());
                let frozen = freeze(&mut set, &policy).expect("freeze fixture");
                ix.index(frozen, AnalysisContext::host()).expect("index fixture")
            })
            .collect()
    }

    fn scratch(name: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!("studio-code-{}-{}-{}", name, std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        }
        dir
    }

    fn runtime_with(graphs: &[(&str, Arc<Graph>)]) -> (CodeRuntime, Vec<CodeTarget>) {
        let mut runtime = CodeRuntime::new(None, None);
        let mut targets = Vec::new();
        for (flow, graph) in graphs {
            let root = PathBuf::from(format!("/fixture/{flow}"));
            runtime.analysers.insert(root.clone(), CodeAnalyser::with_graph(graph.clone(), 3));
            targets.push(CodeTarget { flow: (*flow).into(), root, source_revision: 3 });
        }
        (runtime, targets)
    }

    fn call(runtime: &mut CodeRuntime, target: &CodeTarget, id: &str, tool: &str, args: &str, control: &Path) -> LaneEnvelope {
        let args = json::parse(args.as_bytes()).unwrap();
        let call = registry::parse_lane_call(tool, &args).unwrap();
        runtime.lane(&target.flow).admission.rewind(Duration::from_secs(1));
        match runtime.serve(&target.flow, &target.flow, id, control, None, &args, call, target, None) {
            CodeOutcome::Answered(e) => e,
            CodeOutcome::Deferred => panic!("{tool} deferred"),
        }
    }

    #[test]
    fn pages_stay_under_twelve_kib_and_cursors_are_consumed_once() {
        let g = fixture_graph("atlasfix");
        let (mut runtime, targets) = runtime_with(&[("lane-a", g)]);
        let control = scratch("pages");
        let mut pages = 0;
        let mut rows = 0;
        let mut cursor: Option<String> = None;
        let mut first_cursor = None;
        loop {
            let args = match &cursor {
                Some(c) => format!(r#"{{"scope":"atlasfix","budget":{{"entities":3,"edges":1}},"cursor":"{c}"}}"#),
                None => r#"{"scope":"atlasfix","budget":{"entities":3,"edges":1}}"#.into(),
            };
            let envelope = call(&mut runtime, &targets[0], &format!("req-{pages}"), "code_outline", &args, &control);
            assert!(envelope.error.is_none(), "{:?}", envelope.error);
            let body = envelope.json().to_json();
            assert!(body.len() <= PAGE_BYTES, "page {pages} is {} bytes", body.len());
            assert!(envelope.analysis_context_hash.as_ref().is_some_and(|h| h.len() == 40));
            rows += envelope.rows.len();
            pages += 1;
            if first_cursor.is_none() {
                first_cursor = envelope.cursor.clone();
            }
            match envelope.cursor {
                Some(c) => {
                    assert!(envelope.truncated);
                    cursor = Some(c);
                }
                None => break,
            }
            assert!(pages < 200, "runaway paging");
        }
        assert!(pages >= 2 && rows > 3, "{pages} pages, {rows} rows");
        // the first cursor was consumed by its continuation
        let stale = call(&mut runtime, &targets[0], "req-stale", "code_outline", &format!(r#"{{"scope":"atlasfix","budget":{{"entities":3,"edges":1}},"cursor":"{}"}}"#, first_cursor.unwrap()), &control);
        assert_eq!(stale.error.as_ref().map(|e| e.kind), Some(LaneErrorKind::StaleCursor));
        // a cursor never issued is stale too
        let never = call(&mut runtime, &targets[0], "req-never", "code_outline", r#"{"scope":"atlasfix","cursor":"c9-r1-deadbeef"}"#, &control);
        assert_eq!(never.error.as_ref().map(|e| e.kind), Some(LaneErrorKind::StaleCursor));
    }

    #[test]
    fn a_brief_page_stays_within_the_envelope_budget_with_full_identities() {
        let g = fixture_graph("atlasfix");
        let (mut runtime, targets) = runtime_with(&[("lane-a", g)]);
        let control = scratch("brief-page");
        let mut cursor: Option<String> = None;
        let mut pages = 0;
        loop {
            let args = match &cursor {
                Some(c) => format!(r#"{{"scope":"atlasfix","cursor":"{c}"}}"#),
                None => r#"{"scope":"atlasfix"}"#.into(),
            };
            let e = call(&mut runtime, &targets[0], &format!("b{pages}"), "code_brief", &args, &control);
            assert!(e.error.is_none(), "{:?}", e.error);
            assert!(e.json().to_json().len() <= PAGE_BYTES, "brief page {pages} is {} bytes", e.json().to_json().len());
            if pages == 0 {
                assert_eq!(e.rows[0].get("row").and_then(Value::as_str), Some("brief"));
            }
            for row in &e.rows {
                if let Some(source) = row.get("entity").and_then(|x| x.get("source")) {
                    assert_eq!(source.get("hash").and_then(Value::as_str).map(str::len), Some(40), "full content hash: {}", source.to_json());
                    assert!(source.get("workspace").is_some() && source.get("context").is_some());
                }
            }
            pages += 1;
            match e.cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
            assert!(pages < 50);
        }
    }

    #[test]
    fn a_cursor_keeps_its_graph_and_stamp_across_a_later_publication() {
        let mut pair = fixture_sequence(&["atlasfix", "atlasfix_v2"]);
        let later = pair.pop().unwrap();
        let g1 = pair.pop().unwrap();
        assert_ne!(g1.revision, later.revision, "one indexer advances revisions");
        let (mut runtime, targets) = runtime_with(&[("lane-a", g1.clone())]);
        let control = scratch("lease");
        let first = call(&mut runtime, &targets[0], "r1", "code_outline", r#"{"scope":"atlasfix","budget":{"entities":2,"edges":1}}"#, &control);
        let cursor = first.cursor.clone().expect("a small page has a cursor");
        assert_eq!(first.revision, Some(g1.revision));
        // a later publication replaces the served graph
        runtime.analysers.get_mut(&targets[0].root).unwrap().publish(later.clone(), 9);
        let fresh = call(&mut runtime, &targets[0], "r2", "code_size", r#"{"scope":"atlasfix"}"#, &control);
        assert_eq!(fresh.revision, Some(later.revision), "new calls see the new publication");
        // the continuation still answers from the graph and stamp it was issued for
        let cont = call(&mut runtime, &targets[0], "r3", "code_outline", &format!(r#"{{"scope":"atlasfix","budget":{{"entities":2,"edges":1}},"cursor":"{cursor}"}}"#), &control);
        assert!(cont.error.is_none(), "{:?}", cont.error);
        assert_eq!(cont.revision, Some(g1.revision));
        assert_eq!(cont.analysis_context_hash, first.analysis_context_hash);
        assert!(cont.notes.iter().any(|n| n.starts_with("continuation of revision")));
        assert!(cont.freshness.as_ref().is_some_and(|f| f.observed_source_revision == first.freshness.as_ref().unwrap().observed_source_revision));
        // the retired graph is held by the remaining cursor, not released
        let analyser = runtime.analysers.get(&targets[0].root).unwrap();
        assert!(analyser.retired.iter().any(|g| g.revision == g1.revision) || cont.cursor.is_none());
    }

    #[test]
    fn a_cursor_is_bound_to_its_call_and_its_lane() {
        let g = fixture_graph("atlasfix");
        let (mut runtime, targets) = runtime_with(&[("lane-a", g.clone()), ("lane-b", g)]);
        let control = scratch("bound");
        let first = call(&mut runtime, &targets[0], "r1", "code_outline", r#"{"scope":"atlasfix","budget":{"entities":2,"edges":1}}"#, &control);
        let cursor = first.cursor.expect("a small page has a cursor");
        let other = call(&mut runtime, &targets[0], "r2", "code_outline", &format!(r#"{{"scope":"atlasfix","budget":{{"entities":4,"edges":1}},"cursor":"{cursor}"}}"#), &control);
        assert_eq!(other.error.as_ref().map(|e| e.kind), Some(LaneErrorKind::StaleCursor));
        let second = call(&mut runtime, &targets[0], "r3", "code_outline", r#"{"scope":"atlasfix","budget":{"entities":2,"edges":1}}"#, &control);
        let cursor = second.cursor.unwrap();
        let foreign = call(&mut runtime, &targets[1], "r4", "code_outline", &format!(r#"{{"scope":"atlasfix","budget":{{"entities":2,"edges":1}},"cursor":"{cursor}"}}"#), &control);
        assert_eq!(foreign.error.as_ref().map(|e| e.kind), Some(LaneErrorKind::StaleCursor));
    }

    #[test]
    fn every_call_counts_toward_the_outstanding_limit() {
        let g = fixture_graph("atlasfix");
        let (mut runtime, targets) = runtime_with(&[("lane-a", g)]);
        let control = scratch("outstanding");
        runtime.lane("lane-a").admission.begin();
        runtime.lane("lane-a").admission.begin();
        let args = json::parse(br#"{"scope":"atlasfix"}"#).unwrap();
        let parsed = registry::parse_lane_call("code_size", &args).unwrap();
        // the host admits before serving; a lane with two calls in flight is refused
        let refused = runtime.lane("lane-a").admission.admit().unwrap_err();
        assert_eq!(refused.kind, LaneErrorKind::RateLimited);
        assert!(refused.message.contains("outstanding"));
        runtime.lane("lane-a").admission.end();
        runtime.lane("lane-a").admission.end();
        assert!(runtime.lane("lane-a").admission.admit().is_ok());
        match runtime.serve("lane-a", "lane-a", "s1", &control, None, &args, parsed, &targets[0], None) {
            CodeOutcome::Answered(e) => assert!(e.error.is_none()),
            CodeOutcome::Deferred => panic!(),
        }
        assert_eq!(runtime.lane("lane-a").admission.outstanding, 0, "a synchronous call releases its slot");
    }

    #[test]
    fn brief_and_impact_answer_with_stamped_sources_and_a_trace_record() {
        let g = fixture_graph("atlasfix");
        let (mut runtime, targets) = runtime_with(&[("lane-a", g.clone())]);
        let control = scratch("trace");
        let brief = call(&mut runtime, &targets[0], "b1", "code_brief", r#"{"scope":"atlasfix"}"#, &control);
        assert!(brief.error.is_none(), "{:?}", brief.error);
        assert_eq!(brief.rows[0].get("row").and_then(Value::as_str), Some("brief"));
        assert!(brief.rows[0].get("text").and_then(Value::as_str).unwrap().contains("BRIEF"));
        let impact = call(&mut runtime, &targets[0], "i1", "code_impact", r#"{"files":["src/c.rs"],"depth":3}"#, &control);
        assert!(impact.error.is_none(), "{:?}", impact.error);
        let stamped = impact.rows.iter().filter_map(|r| r.get("entity").and_then(|e| e.get("source"))).count();
        assert!(stamped >= 1, "dependents carry stamped sources: {}", Value::Arr(impact.rows.clone()).to_json());
        for row in &impact.rows {
            if let Some(source) = row.get("entity").unwrap().get("source") {
                assert!(source.get("hash").is_some() && source.get("revision").is_some() && source.get("path").is_some(), "{}", source.to_json());
            }
        }
        for (id, tool, envelope) in [("b1", "code_brief", &brief), ("i1", "code_impact", &impact)] {
            code_trace(&control, code_record(id, tool, &json::obj(vec![]), "lane-a", "ok", envelope, Duration::from_millis(3))).unwrap();
        }
        let records = code_trace_read(&control, CODE_TRACE_PAGE).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].get("tool").and_then(Value::as_str), Some("code_brief"));
        assert_eq!(records[1].get("result_ref").and_then(Value::as_str), Some("replies/i1.json"));
        assert!(records[1].get("duration_ms").is_some() && records[1].get("revision").is_some() && records[1].get("truncated").is_some());
    }

    #[test]
    fn busy_replies_carry_the_last_publication_or_explicit_unavailable_coverage() {
        let g = fixture_graph("atlasfix");
        let mut analyser = CodeAnalyser::with_graph(g.clone(), 3);
        let (error, stamp, coverage) = analyser.busy(4);
        assert_eq!(error.kind, LaneErrorKind::IndexerBusy);
        assert_eq!(stamp.as_ref().map(|s| s.revision), Some(g.revision));
        assert!(coverage.get("files_indexed").and_then(Value::as_i64).is_some());
        let e = code_busy_envelope("x", "code_size", "lane-a", error, stamp, coverage);
        assert_eq!(e.revision, Some(g.revision));
        assert!(e.freshness.is_some() && e.analysis_context_hash.is_some());
        analyser.graph = None;
        let (error, stamp, coverage) = analyser.busy(4);
        assert!(stamp.is_none());
        let e = code_busy_envelope("y", "code_size", "lane-a", error, stamp, coverage);
        assert_eq!(e.coverage.get("unavailable").and_then(Value::as_str).map(|s| s.starts_with("no publication yet")), Some(true));
        assert_eq!(e.coverage.get("files_indexed"), Some(&Value::Null));
        assert!(e.json().to_json().contains("worker_update") || e.freshness.is_none());
    }

    #[test]
    fn traces_are_redacted_and_rotated() {
        let control = scratch("redact");
        let token = "a".repeat(64);
        let args = json::obj(vec![("scope", s(format!("http://127.0.0.1:5/v1/{token}"))), ("note", s("Authorization: Bearer abcdefghijklmnopqrstuvwxyz0123456789"))]);
        let envelope = LaneEnvelope::empty("x1", "code_brief", "lane-a");
        let record = code_record("x1", "code_brief", &args, "lane-a", "ok", &envelope, Duration::from_millis(1));
        assert!(!record.to_json().contains(&token), "args are redacted before persistence");
        code_trace(&control, record).unwrap();
        let text = fs::read_to_string(control.join("calls.jsonl")).unwrap();
        assert!(!text.contains(&token) && !text.contains("abcdefghijklmnopqrstuvwxyz0123456789"), "{text}");
        let big = LaneEnvelope::empty("x2", "code_brief", "lane-a");
        let filler = json::obj(vec![("pad", s("p".repeat(4000)))]);
        for i in 0..300 {
            code_trace(&control, code_record(&format!("x{i}"), "code_brief", &filler, "lane-a", "ok", &big, Duration::from_millis(1))).unwrap();
        }
        assert!(fs::metadata(control.join("calls.jsonl")).unwrap().len() <= CODE_TRACE_LIMIT + 8192);
        assert!(control.join("calls.1.jsonl").is_file());
        let _ = fs::remove_dir_all(&control);
    }

    #[test]
    fn arch_diff_uses_the_lane_start_baseline_and_keys_deferred_calls_by_namespace() {
        let mut pair = fixture_sequence(&["atlasfix", "atlasfix_v2"]);
        let g2 = pair.pop().unwrap();
        let g1 = pair.pop().unwrap();
        let (mut runtime, targets) = runtime_with(&[("lane-a", g1.clone()), ("lane-b", g1.clone())]);
        let control = scratch("diff");
        // lane-a's first publication is its baseline; a later publication differs from it
        runtime.analysers.get_mut(&targets[0].root).unwrap().publish(g2.clone(), 5);
        let args = json::parse(br#"{"base":{"kind":"baseline"}}"#).unwrap();
        let parsed = registry::parse_lane_call("code_arch_diff", &args).unwrap();
        match runtime.serve("lane-a", "lane-a", "diff", &control, None, &args, parsed.clone(), &targets[0], None) {
            CodeOutcome::Deferred => {}
            CodeOutcome::Answered(e) => panic!("arch_diff answers through the spool: {:?}", e.error),
        }
        // the same request id in another lane's namespace is a different deferred call
        match runtime.serve("lane-b", "lane-b", "diff", &control, None, &args, parsed, &targets[1], None) {
            CodeOutcome::Deferred => {}
            CodeOutcome::Answered(e) => panic!("{:?}", e.error),
        }
        assert_eq!(runtime.deferred.len(), 2);
        assert_eq!(runtime.lanes["lane-a"].admission.outstanding, 1);
        let done = runtime.poll();
        assert_eq!(done.len(), 2);
        let (a, ea) = done.iter().find(|(d, _)| d.namespace == "lane-a").unwrap();
        assert_eq!(a.owner, "lane-a");
        assert!(ea.error.is_none(), "{:?}", ea.error);
        assert!(ea.notes.iter().any(|n| n.starts_with("baseline = the lane's first publication")), "{:?}", ea.notes);
        assert!(ea.notes.iter().any(|n| n.starts_with("entities +")), "{:?}", ea.notes);
        assert!(!ea.rows.is_empty(), "v2 differs from the baseline");
        let (_, eb) = done.iter().find(|(d, _)| d.namespace == "lane-b").unwrap();
        assert!(eb.rows.is_empty(), "lane-b never changed: {}", Value::Arr(eb.rows.clone()).to_json());
        assert_eq!(runtime.lanes["lane-a"].admission.outstanding, 0);
        // a worktree base pinned to a revision the other lane does not serve is stale
        let args = json::parse(br#"{"base":{"kind":"worktree","worktree":"lane-b","revision":99}}"#).unwrap();
        let parsed = registry::parse_lane_call("code_arch_diff", &args).unwrap();
        match runtime.serve("lane-a", "lane-a", "diff2", &control, None, &args, parsed, &targets[0], Some(targets[1].clone())) {
            CodeOutcome::Answered(e) => assert_eq!(e.error.as_ref().map(|e| e.kind), Some(LaneErrorKind::StaleRevision)),
            CodeOutcome::Deferred => panic!("pinned to a revision that is not served"),
        }
    }

    #[test]
    fn worktree_scoping_denies_unknown_and_archived_lanes() {
        let mut engine = Engine::default();
        let config = FlowConfig { repo: PathBuf::from("/repo"), manifest: PathBuf::from("/repo/Cargo.toml"), package: "p".into(), binary: "p".into(), check_targets: vec!["p".into()], test_scope: TestScope::Workspace, agent_provider: None, delegation_context: "Astra manages".into() };
        engine.apply(FlowCommand::Create { title: "a".into(), config: config.clone() }, 1).unwrap();
        engine.apply(FlowCommand::Create { title: "b".into(), config }, 2).unwrap();
        let ids: Vec<String> = engine.flows.keys().cloned().collect();
        for id in &ids {
            engine.flows.get_mut(id).unwrap().worktree = Some(PathBuf::from(format!("/wt/{id}")));
        }
        engine.flows.get_mut(&ids[1]).unwrap().lifecycle = iteration::FlowLifecycle::Archived;
        let host = Host::for_tests(engine);
        assert_eq!(host.code_target(&ids[0], None).unwrap().root, PathBuf::from(format!("/wt/{}", ids[0])));
        assert_eq!(host.code_target(&ids[0], Some("nope")).unwrap_err().kind, LaneErrorKind::PermissionDenied);
        assert_eq!(host.code_target(&ids[0], Some(&ids[1])).unwrap_err().kind, LaneErrorKind::PermissionDenied, "archived lanes are not a target");
        let parsed = registry::parse_lane_call("code_size", &json::parse(br#"{"worktree":"/wt/other"}"#).unwrap());
        assert_eq!(parsed.unwrap_err().kind, LaneErrorKind::InvalidArguments);
    }
}
