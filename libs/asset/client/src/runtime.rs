//! Background execution with explicit, typed loading/error states.
//!
//! UI hosts (AI Content, the Asset Store, Sandbox's loader) must not block
//! their frame loop on the network. [`ClientRuntime`] owns a connected
//! [`AssetClient`], runs it on a pool of worker threads, and hands the app a
//! [`RequestId`] immediately; the app drains typed [`ClientEvent`]s from its
//! own poll loop.
//!
//! **Two lanes, because one queue is a head-of-line block.** A single serial
//! worker makes every thumbnail wait behind whatever is in front of it — one
//! multi-megabyte GLB or track download stalls thirty icon fetches, and the
//! user watches a grid trickle in against a server on localhost. So requests
//! are classified ([`Lane`]): small control-plane calls and small blob
//! fetches take the FAST lane (several workers), large transfers and
//! publications take the BULK lane (a couple). A big download can saturate
//! its own lane and never the other.
//!
//! Consequences the API makes explicit:
//! - Requests no longer complete in submission order, and events from
//!   different requests interleave. Events for ONE request stay ordered:
//!   `Started` first, then its `Progress`, then exactly one `Done`/`Failed`,
//!   because one worker owns a request start to finish.
//! - [`ClientRuntime::submit_with`] takes an explicit [`SubmitOptions`] when
//!   the caller knows better than the classifier — including `lifo`, which
//!   serves the newest queued work first (what a scrolling thumbnail grid
//!   wants: the visible row, not the row the user scrolled past).
//! - Lane workers share ONE cache and one per-digest transfer gate (see
//!   [`AssetClient::lane_clone`]), so parallelism never means two writers on
//!   one partial file, a second lock on the cache root, or the same blob
//!   downloaded twice.
//!
//! There is no implicit state anywhere: a slot the app renders is `Idle`,
//! `Loading` (with byte progress when known), `Ready`, or `Failed` with the
//! typed refusal — see [`ResourceState`]/[`ResourceSlot`]. A dead worker
//! surfaces as [`ClientError::RuntimeDown`] at submit, never as silence.

use crate::api::{CatalogQuery, SourceCollectionRegistered};
use crate::client::{AssetClient, AssetsPage, CatalogPage, PageCursor};
use crate::dto::{
    AliasDto, AssetDetailDto, GameAliasDto, ImportReportDto, ImportStatusDto, JobDetailDto, JobId,
    JobProfileDto, JobRowDto, JobStatusDto, SourceCollectionRowDto,
};
use crate::error::{ClientError, ClientResult};
use crate::json::Value;
use crate::publish::{PublishBundle, PublishRequest, PublishStage, Published, PublishedBundle};
use crate::resolver::{ResolvedFile, ResolvedThumbnail, TierPreference};
use makepad_asset_data::{
    AssetAlias, AssetId, AssetManifest, AssetRevisionId, AssetRevisionRef, BlobId, ClientProfile,
    DerivedVariantId, DerivedVariantManifest, FileRole, GameAlias, GameRevisionId,
    GameRevisionManifest, ImportRevisionId, ResolvedVariantMap, VariantSetId, VariantSetManifest,
};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};

pub type RequestId = u64;

/// Emit a progress event at most every this many new bytes.
const PROGRESS_STRIDE_BYTES: u64 = 256 * 1024;

#[derive(Clone, Debug)]
pub enum ClientRequest {
    CatalogSearch { query: CatalogQuery, cursor: Option<PageCursor> },
    AssetsPage { namespace: Option<String>, cursor: Option<PageCursor>, limit: u64 },
    AssetDetail { id: AssetId },
    ResolveAlias { alias: AssetAlias },
    ResolveGameAlias { alias: GameAlias },
    FetchAssetManifest { rev: AssetRevisionId },
    FetchGameManifest { rev: GameRevisionId },
    /// Fetch a blob into the cache; when `pin` is true, the verified committed
    /// object is pinned only after the fetch succeeds. A failed or cancelled
    /// fetch therefore never leaves an absent-object pin behind.
    FetchBlob { blob: BlobId, expected_len: Option<u64>, pin: bool },
    /// Remove this blob's cache pin. Pins are idempotent markers rather than
    /// reference-counted leases; removing an absent pin also succeeds.
    UnpinBlob { blob: BlobId },
    /// Select per policy and materialize one file of a manifest.
    ResolveFile {
        manifest: Box<AssetManifest>,
        role: FileRole,
        tier: TierPreference,
        max_lod: u8,
    },
    /// Materialize the manifest's typed thumbnail (None when it has none).
    ResolveThumbnail { manifest: Box<AssetManifest> },
    /// Delete an asset from the store: every revision retired, aliases and
    /// search rows gone, bytes handed to blob GC. Idempotent.
    RetireAsset { id: AssetId },
    /// Delete one revision (typically superseded); the asset stays live.
    RetireRevision { id: AssetId, revision: AssetRevisionId },
    /// Advance blob garbage collection by a bounded amount. One request is
    /// one bounded unit of work — the UI polls until the status says `done`
    /// (with `GcRequest::dry_run()` first to show what would be freed).
    GcBlobs { request: crate::api::GcRequest },
    /// The newest GC run's progress, without starting or advancing one.
    GcStatus,
    /// Abandon the active GC run.
    GcCancel,
    /// Publish a generated artifact end to end (see [`crate::publish`]).
    PublishArtifact { request: Box<PublishRequest> },
    /// Publish a multi-file bundle; its [`PublishStage`]s stream on the
    /// side channel ([`ClientRuntime::poll_stages`]), and `cancel` aborts it
    /// between stages.
    PublishBundle { request: Box<PublishBundle> },
    /// Advertised generation capabilities, optionally domain-filtered.
    FetchJobProfiles { domain: Option<String> },
    /// Enqueue a generation job; the server schedules the compute.
    EnqueueJob { namespace: String, kind: String, body: Value },
    FetchJobStatus { job: JobId },
    /// Complete visible job state (enqueuer, attempts, freshness, result).
    FetchJobDetail { job: JobId },
    /// Scoped job listing (`namespace` = capability-gated view; `None` =
    /// the caller's own jobs).
    FetchJobs { namespace: Option<String>, limit: u64 },
    CancelJob { job: JobId },
    /// The versioned operation registry with truthful availability.
    FetchOperationTypes,
    /// Create (or idempotently join) a typed asset operation.
    CreateOperation { request: Box<crate::api::OperationCreateRequest> },
    FetchOperation { op: crate::dto::OperationId },
    /// One page of the durable operation event log (bounded long-poll).
    FetchOperationEvents {
        op: crate::dto::OperationId,
        after: u64,
        wait_ms: u64,
        limit: u32,
    },
    CancelOperation { op: crate::dto::OperationId },
    RetryOperation { op: crate::dto::OperationId },
    /// Register an approved source collection from canonical bytes.
    RegisterSourceCollection { bytes: Vec<u8> },
    /// List approved source collections (projection).
    ListSourceCollections,
    /// Run or idempotently replay one pack import from canonical bytes.
    RunImport { bytes: Vec<u8> },
    /// Import status projection for an exact import revision.
    FetchImport { revision: ImportRevisionId },
    /// Canonical derived-variant document, digest-verified.
    FetchDerivedVariant { id: DerivedVariantId },
    /// Freeze an immutable variant set over ready variants of one base.
    FreezeVariantSet { base: AssetRevisionRef, variants: Vec<DerivedVariantId> },
    /// Canonical variant-set document, digest-verified.
    FetchVariantSet { id: VariantSetId },
    /// Server-side deterministic profile resolution of one frozen set.
    ResolveVariantSet { set: VariantSetId, profile: ClientProfile },
}

#[derive(Clone, Debug)]
pub enum ClientOutput {
    CatalogPage(CatalogPage),
    AssetsPage(AssetsPage),
    AssetDetail(AssetDetailDto),
    Alias(AliasDto),
    GameAlias(GameAliasDto),
    AssetManifest(Box<AssetManifest>),
    GameManifest(Box<GameRevisionManifest>),
    Blob { blob: BlobId, path: PathBuf },
    BlobUnpinned { blob: BlobId },
    File(ResolvedFile),
    Thumbnail(Option<ResolvedThumbnail>),
    /// One retirement's report (asset or revision).
    Retired(crate::dto::RetireDto),
    /// Durable progress of the newest GC run.
    Gc(crate::dto::GcStatusDto),
    /// Whether a GC cancel stopped an active run.
    GcCancelled(bool),
    Published(Published),
    PublishedBundle(PublishedBundle),
    JobProfiles(Vec<JobProfileDto>),
    JobQueued(JobId),
    JobStatus(JobStatusDto),
    JobDetail(JobDetailDto),
    Jobs(Vec<JobRowDto>),
    /// How many jobs the cancel reached (0 = already terminal).
    JobCancelled(u64),
    OperationTypes(Vec<crate::dto::OperationTypeDto>),
    Operation(crate::dto::OperationStatusDto),
    OperationEvents(crate::dto::OperationEventsPageDto),
    /// Whether the cancel changed anything (false = already terminal).
    OperationCancelled(bool),
    SourceCollectionRegistered(SourceCollectionRegistered),
    SourceCollections(Vec<SourceCollectionRowDto>),
    ImportReport(ImportReportDto),
    ImportStatus(ImportStatusDto),
    DerivedVariant(Box<DerivedVariantManifest>),
    VariantSetFrozen(VariantSetId),
    VariantSet(Box<VariantSetManifest>),
    ResolvedVariants(Box<ResolvedVariantMap>),
}

#[derive(Clone, Debug)]
pub enum ClientEvent {
    Started { id: RequestId },
    /// Byte progress of a blob transfer: `(bytes_present, total_bytes)`.
    Progress { id: RequestId, bytes: u64, total: u64 },
    Done { id: RequestId, output: ClientOutput },
    Failed { id: RequestId, error: ClientError },
}

impl ClientEvent {
    pub fn id(&self) -> RequestId {
        match self {
            ClientEvent::Started { id }
            | ClientEvent::Progress { id, .. }
            | ClientEvent::Done { id, .. }
            | ClientEvent::Failed { id, .. } => *id,
        }
    }
}

/// Operation-stage progress of a long publication, reported on its own
/// stream ([`ClientRuntime::poll_stages`]) so it stays distinct from
/// byte-level blob progress and existing [`ClientEvent`] consumers are
/// untouched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageEvent {
    pub id: RequestId,
    pub stage: PublishStage,
}

/// Explicit lifecycle of one loadable resource. Nothing renders "empty
/// because still loading" and "empty because failed" the same way again.
#[derive(Clone, Debug, Default)]
pub enum ResourceState<T> {
    #[default]
    Idle,
    Loading {
        /// `(bytes_present, total_bytes)` when byte progress is known.
        progress: Option<(u64, u64)>,
    },
    Ready(T),
    Failed(ClientError),
}

impl<T> ResourceState<T> {
    pub fn is_loading(&self) -> bool {
        matches!(self, ResourceState::Loading { .. })
    }
    pub fn is_ready(&self) -> bool {
        matches!(self, ResourceState::Ready(_))
    }
    pub fn ready(&self) -> Option<&T> {
        match self {
            ResourceState::Ready(t) => Some(t),
            _ => None,
        }
    }
    pub fn failed(&self) -> Option<&ClientError> {
        match self {
            ResourceState::Failed(e) => Some(e),
            _ => None,
        }
    }
}

/// One UI slot tracking one in-flight request. `begin` ties the slot to a
/// submitted id; `on_event` advances the state, using `extract` to pull the
/// slot's value out of a [`ClientOutput`] (returning `None` marks a
/// wrong-output protocol failure rather than mis-rendering it).
#[derive(Clone, Debug, Default)]
pub struct ResourceSlot<T> {
    request: Option<RequestId>,
    pub state: ResourceState<T>,
}

impl<T> ResourceSlot<T> {
    pub fn begin(&mut self, id: RequestId) {
        self.request = Some(id);
        self.state = ResourceState::Loading { progress: None };
    }

    pub fn request(&self) -> Option<RequestId> {
        self.request
    }

    /// True when the event belonged to (and advanced) this slot.
    pub fn on_event(
        &mut self,
        event: &ClientEvent,
        extract: impl FnOnce(&ClientOutput) -> Option<T>,
    ) -> bool {
        let Some(id) = self.request else {
            return false;
        };
        if event.id() != id {
            return false;
        }
        match event {
            ClientEvent::Started { .. } => {}
            ClientEvent::Progress { bytes, total, .. } => {
                self.state = ResourceState::Loading { progress: Some((*bytes, *total)) };
            }
            ClientEvent::Done { output, .. } => {
                self.request = None;
                self.state = match extract(output) {
                    Some(t) => ResourceState::Ready(t),
                    None => ResourceState::Failed(ClientError::Protocol {
                        what: "unexpected output for slot",
                    }),
                };
            }
            ClientEvent::Failed { error, .. } => {
                self.request = None;
                self.state = ResourceState::Failed(error.clone());
            }
        }
        true
    }
}

/// Which pool of workers a request runs on. Small work must never queue
/// behind large transfers, so there are exactly two: one for the many small
/// requests a UI makes, one for the few big ones.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane {
    /// Control-plane calls, manifests, thumbnails, small blobs.
    Fast,
    /// Large blob transfers, file resolution, publications, imports.
    Bulk,
}

/// Per-submit overrides. `Default` = classify the request and serve it FIFO,
/// which is what every existing caller gets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubmitOptions {
    /// Force a lane instead of classifying the request.
    pub lane: Option<Lane>,
    /// Serve this ahead of everything already queued in its lane (newest
    /// first). A scrolling grid wants the row on screen, not the twenty rows
    /// the user has already scrolled past — but that is a scheduling policy
    /// the caller states, never magic the runtime applies behind its back.
    pub lifo: bool,
}

impl SubmitOptions {
    pub fn fast() -> SubmitOptions {
        SubmitOptions { lane: Some(Lane::Fast), lifo: false }
    }
    pub fn bulk() -> SubmitOptions {
        SubmitOptions { lane: Some(Lane::Bulk), lifo: false }
    }
    /// Newest-first in whichever lane the request lands in.
    pub fn newest_first() -> SubmitOptions {
        SubmitOptions { lane: None, lifo: true }
    }
    pub fn with_lane(mut self, lane: Lane) -> SubmitOptions {
        self.lane = Some(lane);
        self
    }
    pub fn with_lifo(mut self, lifo: bool) -> SubmitOptions {
        self.lifo = lifo;
        self
    }
}

/// Worker counts and the size line between the lanes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    /// Workers serving small requests. Four keeps a thumbnail grid filling
    /// while one worker is stuck on a slow control-plane call.
    pub fast_workers: usize,
    /// Workers serving large transfers. Two so a second big download starts
    /// without waiting, without turning the link into a congestion contest.
    pub bulk_workers: usize,
    /// A blob fetch whose declared length is at or below this goes FAST.
    /// Thumbnails and icons are far below it; media and models are above.
    pub fast_blob_max_bytes: u64,
}

impl RuntimeConfig {
    pub fn default_v1() -> RuntimeConfig {
        RuntimeConfig { fast_workers: 4, bulk_workers: 2, fast_blob_max_bytes: 512 * 1024 }
    }

    fn validate(&self) -> ClientResult<()> {
        if self.fast_workers == 0 || self.bulk_workers == 0 {
            return Err(ClientError::InvalidInput { what: "runtime lane workers zero" });
        }
        if self.fast_workers + self.bulk_workers > 64 {
            return Err(ClientError::InvalidInput { what: "runtime lane workers over budget" });
        }
        Ok(())
    }
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self::default_v1()
    }
}

/// One lane's work queue: a deque so `lifo` submissions can jump it, plus a
/// condvar so idle workers cost nothing. Closing it drains what is queued and
/// then releases every worker — the same "finish what was accepted, then
/// stop" shutdown the single-worker runtime had.
struct LaneQueue {
    state: Mutex<LaneState>,
    wake: Condvar,
}

struct LaneState {
    items: VecDeque<(RequestId, ClientRequest)>,
    closed: bool,
}

impl LaneQueue {
    fn new() -> LaneQueue {
        LaneQueue {
            state: Mutex::new(LaneState { items: VecDeque::new(), closed: false }),
            wake: Condvar::new(),
        }
    }

    /// Queue one request. Returns false when the lane is closed (shutting
    /// down), which submit reports as [`ClientError::RuntimeDown`].
    fn push(&self, id: RequestId, request: ClientRequest, lifo: bool) -> bool {
        let mut state = self.state.lock().expect("lane queue");
        if state.closed {
            return false;
        }
        if lifo {
            state.items.push_front((id, request));
        } else {
            state.items.push_back((id, request));
        }
        drop(state);
        self.wake.notify_one();
        true
    }

    fn pop(&self) -> Option<(RequestId, ClientRequest)> {
        let mut state = self.state.lock().expect("lane queue");
        loop {
            if let Some(item) = state.items.pop_front() {
                return Some(item);
            }
            if state.closed {
                return None;
            }
            state = self.wake.wait(state).expect("lane queue wait");
        }
    }

    fn close(&self) {
        self.state.lock().expect("lane queue").closed = true;
        self.wake.notify_all();
    }
}

pub struct ClientRuntime {
    fast: Arc<LaneQueue>,
    bulk: Arc<LaneQueue>,
    rx: Receiver<ClientEvent>,
    stage_rx: Receiver<StageEvent>,
    next_id: RequestId,
    cancelled: Arc<Mutex<HashSet<RequestId>>>,
    config: RuntimeConfig,
    joins: Vec<std::thread::JoinHandle<()>>,
    /// Set by `shutdown`/`Drop` so both paths are idempotent.
    stopped: bool,
}

impl ClientRuntime {
    /// Take ownership of a connected client and run it on the default lane
    /// pool (see [`RuntimeConfig::default_v1`]). Every extra worker gets its
    /// own handle on the same verified server and the same cache through
    /// [`AssetClient::lane_clone`] — no second connect, no second cache.
    pub fn start(client: AssetClient) -> ClientResult<ClientRuntime> {
        Self::start_with(client, RuntimeConfig::default_v1())
    }

    /// As [`Self::start`] with explicit lane sizing.
    pub fn start_with(client: AssetClient, config: RuntimeConfig) -> ClientResult<ClientRuntime> {
        config.validate()?;
        let (evt_tx, evt_rx) = channel::<ClientEvent>();
        let (stage_tx, stage_rx) = channel::<StageEvent>();
        let cancelled = Arc::new(Mutex::new(HashSet::new()));
        let fast = Arc::new(LaneQueue::new());
        let bulk = Arc::new(LaneQueue::new());

        // One client per worker; the last one takes the caller's original so
        // nothing is built that is not used.
        let total = config.fast_workers + config.bulk_workers;
        let mut clients: Vec<AssetClient> =
            (0..total - 1).map(|_| client.lane_clone()).collect();
        clients.push(client);

        let mut joins = Vec::with_capacity(total);
        for (n, client) in clients.into_iter().enumerate() {
            let lane_is_fast = n < config.fast_workers;
            let queue = if lane_is_fast { fast.clone() } else { bulk.clone() };
            let tx = evt_tx.clone();
            let stage = stage_tx.clone();
            let cancelled = cancelled.clone();
            let name = if lane_is_fast {
                format!("asset-client-fast-{n}")
            } else {
                format!("asset-client-bulk-{}", n - config.fast_workers)
            };
            let join = std::thread::Builder::new()
                .name(name)
                .spawn(move || worker(client, queue, tx, stage, cancelled))
                .map_err(|e| ClientError::Io { op: "spawn runtime worker", kind: e.kind() })?;
            joins.push(join);
        }
        Ok(ClientRuntime {
            fast,
            bulk,
            rx: evt_rx,
            stage_rx,
            next_id: 1,
            cancelled,
            config,
            joins,
            stopped: false,
        })
    }

    /// Queue a request; events for it arrive under the returned id. The lane
    /// is chosen from the request (see [`Lane`]); requests no longer complete
    /// in submission order, but every event of one request stays ordered.
    pub fn submit(&mut self, request: ClientRequest) -> ClientResult<RequestId> {
        self.submit_with(request, SubmitOptions::default())
    }

    /// [`Self::submit`] with an explicit lane and/or newest-first placement.
    pub fn submit_with(
        &mut self,
        request: ClientRequest,
        options: SubmitOptions,
    ) -> ClientResult<RequestId> {
        let lane = options
            .lane
            .unwrap_or_else(|| classify(&request, self.config.fast_blob_max_bytes));
        let queue = match lane {
            Lane::Fast => &self.fast,
            Lane::Bulk => &self.bulk,
        };
        let id = self.next_id;
        if !queue.push(id, request, options.lifo) {
            return Err(ClientError::RuntimeDown);
        }
        self.next_id += 1;
        Ok(id)
    }

    /// The lane a request would take without an explicit override. Exposed so
    /// a host can reason about (and test) its own scheduling.
    pub fn lane_of(&self, request: &ClientRequest) -> Lane {
        classify(request, self.config.fast_blob_max_bytes)
    }

    pub fn config(&self) -> RuntimeConfig {
        self.config
    }

    /// Cancel a submitted request. A still-queued request is skipped; an
    /// in-flight blob transfer aborts at its next chunk (the partial stays
    /// resumable on disk). Either way the request ends with
    /// [`ClientError::Cancelled`]; a request that already finished is
    /// unaffected. Callable from any thread.
    pub fn cancel(&self, id: RequestId) {
        self.cancelled.lock().unwrap().insert(id);
    }

    /// Drain pending events without blocking. An empty vec means nothing
    /// happened since the last poll — it never hides a dead worker, which
    /// shows up as [`ClientError::RuntimeDown`] on the next submit.
    pub fn poll(&mut self) -> Vec<ClientEvent> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(e) => out.push(e),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    /// Drain pending operation-stage events (publications only) without
    /// blocking. A separate stream from [`Self::poll`], so hosts that never
    /// publish never see it.
    pub fn poll_stages(&mut self) -> Vec<StageEvent> {
        let mut out = Vec::new();
        loop {
            match self.stage_rx.try_recv() {
                Ok(e) => out.push(e),
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }

    /// Stop accepting requests, let the workers finish what is already
    /// queued, and join every one of them.
    pub fn shutdown(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        if self.stopped {
            return;
        }
        self.stopped = true;
        self.fast.close();
        self.bulk.close();
        for join in self.joins.drain(..) {
            let _ = join.join();
        }
    }
}

impl Drop for ClientRuntime {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

/// Which lane a request belongs to when the caller states nothing. The rule
/// is size, not politeness: anything that can be megabytes goes BULK so the
/// many small requests behind it keep flowing.
fn classify(request: &ClientRequest, fast_blob_max_bytes: u64) -> Lane {
    match request {
        // A declared length decides it; an UNKNOWN length is treated as big,
        // because guessing small is exactly the mistake that stalls a lane.
        ClientRequest::FetchBlob { expected_len, .. } => match expected_len {
            Some(len) if *len <= fast_blob_max_bytes => Lane::Fast,
            _ => Lane::Bulk,
        },
        // A thumbnail is bounded by the content contract and is the request
        // a grid makes by the dozen.
        ClientRequest::ResolveThumbnail { .. } => Lane::Fast,
        // Model/media files, publications and imports move real payloads.
        ClientRequest::ResolveFile { .. }
        | ClientRequest::PublishArtifact { .. }
        | ClientRequest::PublishBundle { .. }
        | ClientRequest::RunImport { .. }
        | ClientRequest::RegisterSourceCollection { .. } => Lane::Bulk,
        // A long-poll parks a worker for its whole wait; it must never do
        // that in the lane the UI needs for icons.
        ClientRequest::FetchOperationEvents { .. } => Lane::Bulk,
        // Everything else is a small control-plane call.
        _ => Lane::Fast,
    }
}

fn worker(
    mut client: AssetClient,
    queue: Arc<LaneQueue>,
    tx: Sender<ClientEvent>,
    stage_tx: Sender<StageEvent>,
    cancelled: Arc<Mutex<HashSet<RequestId>>>,
) {
    while let Some((id, request)) = queue.pop() {
        // Cancelled while queued: never starts.
        if cancelled.lock().expect("cancel set").remove(&id) {
            if tx.send(ClientEvent::Failed { id, error: ClientError::Cancelled }).is_err() {
                return;
            }
            continue;
        }
        // One worker owns a request from Started to its terminal event, so
        // per-request event order holds even though lanes interleave.
        if tx.send(ClientEvent::Started { id }).is_err() {
            return;
        }
        let event = match run_one(&mut client, id, &tx, &stage_tx, request, &cancelled) {
            Ok(output) => ClientEvent::Done { id, output },
            Err(error) => ClientEvent::Failed { id, error },
        };
        cancelled.lock().expect("cancel set").remove(&id);
        if tx.send(event).is_err() {
            return;
        }
    }
}

fn run_one(
    client: &mut AssetClient,
    id: RequestId,
    tx: &Sender<ClientEvent>,
    stage_tx: &Sender<StageEvent>,
    request: ClientRequest,
    cancelled: &Mutex<HashSet<RequestId>>,
) -> ClientResult<ClientOutput> {
    // Throttled byte-progress reporter for blob-bearing requests.
    let mut last_emitted = 0u64;
    let mut progress = |bytes: u64, total: u64| {
        if bytes == total || bytes >= last_emitted + PROGRESS_STRIDE_BYTES {
            last_emitted = bytes;
            let _ = tx.send(ClientEvent::Progress { id, bytes, total });
        }
    };
    match request {
        ClientRequest::CatalogSearch { query, cursor } => Ok(ClientOutput::CatalogPage(
            client.catalog_search(&query, cursor.as_ref())?,
        )),
        ClientRequest::AssetsPage { namespace, cursor, limit } => Ok(ClientOutput::AssetsPage(
            client.assets_page(namespace.as_deref(), cursor.as_ref(), limit)?,
        )),
        ClientRequest::AssetDetail { id } => {
            Ok(ClientOutput::AssetDetail(client.asset_detail(&id)?))
        }
        ClientRequest::ResolveAlias { alias } => {
            Ok(ClientOutput::Alias(client.resolve_alias(&alias)?))
        }
        ClientRequest::ResolveGameAlias { alias } => {
            Ok(ClientOutput::GameAlias(client.resolve_game_alias(&alias)?))
        }
        ClientRequest::FetchAssetManifest { rev } => Ok(ClientOutput::AssetManifest(Box::new(
            client.fetch_asset_manifest(&rev)?,
        ))),
        ClientRequest::FetchGameManifest { rev } => Ok(ClientOutput::GameManifest(Box::new(
            client.fetch_game_manifest(&rev)?,
        ))),
        ClientRequest::FetchBlob { blob, expected_len, pin } => {
            let abort = || cancelled.lock().unwrap().contains(&id);
            let path =
                client.fetch_blob_with_abort(&blob, expected_len, Some(&mut progress), &abort)?;
            if abort() {
                return Err(ClientError::Cancelled);
            }
            // Pin only a successfully fetched, digest-verified committed
            // object. Pinning before the transfer leaked a durable marker on
            // every refusal, timeout, integrity failure, and cancellation.
            if pin {
                client.pin_blob(&blob)?;
            }
            Ok(ClientOutput::Blob { blob, path })
        }
        ClientRequest::UnpinBlob { blob } => {
            client.unpin_blob(&blob)?;
            Ok(ClientOutput::BlobUnpinned { blob })
        }
        ClientRequest::ResolveFile { manifest, role, tier, max_lod } => Ok(ClientOutput::File(
            client.resolve_file(&manifest, role, tier, max_lod, Some(&mut progress))?,
        )),
        ClientRequest::ResolveThumbnail { manifest } => {
            Ok(ClientOutput::Thumbnail(client.resolve_thumbnail(&manifest)?))
        }
        ClientRequest::RetireAsset { id } => {
            Ok(ClientOutput::Retired(client.retire_asset(&id)?))
        }
        ClientRequest::RetireRevision { id, revision } => {
            Ok(ClientOutput::Retired(client.retire_revision(&id, &revision)?))
        }
        ClientRequest::GcBlobs { request } => Ok(ClientOutput::Gc(client.gc_blobs(&request)?)),
        ClientRequest::GcStatus => Ok(ClientOutput::Gc(client.gc_status()?)),
        ClientRequest::GcCancel => Ok(ClientOutput::GcCancelled(client.gc_cancel()?)),
        ClientRequest::PublishArtifact { request } => {
            Ok(ClientOutput::Published(client.publish_artifact(&request)?))
        }
        ClientRequest::PublishBundle { request } => {
            let abort = || cancelled.lock().unwrap().contains(&id);
            let mut on_stage = |stage: &PublishStage| {
                let _ = stage_tx.send(StageEvent { id, stage: stage.clone() });
            };
            Ok(ClientOutput::PublishedBundle(client.publish_bundle_with(
                &request,
                Some(&mut on_stage),
                &abort,
            )?))
        }
        ClientRequest::FetchJobProfiles { domain } => Ok(ClientOutput::JobProfiles(
            client.api().job_profiles(domain.as_deref())?,
        )),
        ClientRequest::EnqueueJob { namespace, kind, body } => Ok(ClientOutput::JobQueued(
            client.api().enqueue_job(&namespace, &kind, &body)?,
        )),
        ClientRequest::FetchJobStatus { job } => {
            Ok(ClientOutput::JobStatus(client.api().job_status(&job)?))
        }
        ClientRequest::FetchJobDetail { job } => {
            Ok(ClientOutput::JobDetail(client.job_detail(&job)?))
        }
        ClientRequest::FetchJobs { namespace, limit } => Ok(ClientOutput::Jobs(
            client.list_jobs(namespace.as_deref(), limit)?,
        )),
        ClientRequest::CancelJob { job } => {
            Ok(ClientOutput::JobCancelled(client.api().cancel_job(&job)?))
        }
        ClientRequest::FetchOperationTypes => {
            Ok(ClientOutput::OperationTypes(client.operation_types()?))
        }
        ClientRequest::CreateOperation { request } => {
            Ok(ClientOutput::Operation(client.operation_create(&request)?))
        }
        ClientRequest::FetchOperation { op } => {
            Ok(ClientOutput::Operation(client.operation_get(&op)?))
        }
        ClientRequest::FetchOperationEvents { op, after, wait_ms, limit } => Ok(
            ClientOutput::OperationEvents(client.operation_events(&op, after, wait_ms, limit)?),
        ),
        ClientRequest::CancelOperation { op } => {
            Ok(ClientOutput::OperationCancelled(client.operation_cancel(&op)?))
        }
        ClientRequest::RetryOperation { op } => {
            Ok(ClientOutput::Operation(client.operation_retry(&op)?))
        }
        ClientRequest::RegisterSourceCollection { bytes } => Ok(
            ClientOutput::SourceCollectionRegistered(client.register_source_collection(&bytes)?),
        ),
        ClientRequest::ListSourceCollections => Ok(ClientOutput::SourceCollections(
            client.list_source_collections()?,
        )),
        ClientRequest::RunImport { bytes } => {
            Ok(ClientOutput::ImportReport(client.run_import(&bytes)?))
        }
        ClientRequest::FetchImport { revision } => {
            Ok(ClientOutput::ImportStatus(client.import_status(&revision)?))
        }
        ClientRequest::FetchDerivedVariant { id } => Ok(ClientOutput::DerivedVariant(Box::new(
            client.fetch_derived_variant(&id)?,
        ))),
        ClientRequest::FreezeVariantSet { base, variants } => Ok(ClientOutput::VariantSetFrozen(
            client.freeze_variant_set(&base, &variants)?,
        )),
        ClientRequest::FetchVariantSet { id } => Ok(ClientOutput::VariantSet(Box::new(
            client.fetch_variant_set(&id)?,
        ))),
        ClientRequest::ResolveVariantSet { set, profile } => Ok(ClientOutput::ResolvedVariants(
            Box::new(client.resolve_variant_set(&set, &profile)?),
        )),
    }
}
