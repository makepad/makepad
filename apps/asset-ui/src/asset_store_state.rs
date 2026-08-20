//! REAL Asset Server session state.
//!
//! The connection lives in `makepad_asset_client::SessionConnector`
//! (shared with the VJ): UDP discovery (or explicit env endpoints), verified
//! identity, bearer auth, retry with backoff — all on worker threads. This
//! module owns the app-side lifecycle and the typed, honest presentation
//! state the Library/Runs/Admin surfaces render:
//!
//! - nothing here fabricates data — every value below arrived from the
//!   connector, a catalog runtime response, or the committed event feed;
//! - absent server routes (there is no global jobs/workers listing) render
//!   as explicit unavailability, never as invented rows;
//! - every call is non-blocking (`poll()` drains channels; requests go to
//!   the runtime worker), so the UI thread never waits on the network.
//!
//! When this process HOSTS the embedded server it also runs the continuous
//! library publisher (`makepad_asset_importer::watch`) on one background
//! thread, so everything the generation pipelines write into
//! `local/ai_content_library/` reaches the catalog — intermediates tagged
//! `intermediate` so program surfaces can exclude them.
//!
//! Env/token conventions (`ASSET_UI_*`, with `AI_CONTENT_*` still accepted):
//! - `ASSET_UI_ASSET_SERVER=ip:controlport:dataport` — explicit endpoints;
//!   unset = LAN discovery on the standard beacon port.
//! - `ASSET_UI_ASSET_SERVER_ID=<32 hex>` — pin the server identity.
//! - Token: `ASSET_UI_ASSET_TOKEN`, then `ASSET_UI_ASSET_TOKEN_FILE`, then
//!   `local/asset-ui/asset-server/admin-token` (the running server's
//!   bootstrap token), then `local/asset-ui/asset-server.token`.
//!   No token = anonymous probe.
//! - `ASSET_UI_ASSET_CACHE=<dir>` — cache parent, default
//!   `local/asset-ui`.

use makepad_asset_client::{
    ApiEndpoints, AssetDetailDto, CatalogEventDto, CatalogFacet, CatalogHit, CatalogQuery,
    FacetKind,
    CatalogSubscriptionEvent, ClientEvent, ClientOutput, ClientRequest, GcRequest, GcStatusDto,
    JobProfileDto, PageCursor, RequestId, RetireDto, SessionConfig, SessionConnector,
    SessionHandles, SessionMsg, SessionStatus,
};
use makepad_asset_data::{AssetId, AssetRevisionId};
pub use makepad_asset_data::AssetKind;
use makepad_widgets::log;
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// Committed catalog events retained for the Admin surface (newest first).
pub const EVENT_LOG_CAP: usize = 200;
/// One search page. The server caps at MAX_SEARCH_LIMIT (100); more rows
/// exist server-side when `SearchResults::more` is set.
pub const SEARCH_PAGE_SIZE: u32 = 60;

/// Facet rows the Library asks for. The dropdown shows the most-used labels
/// first and a long tail helps nobody find anything.
pub const SEARCH_FACETS: u32 = 24;

/// The full content-contract kind vocabulary, for the server kind filter.
pub const SERVER_KINDS: [AssetKind; 13] = [
    AssetKind::Mesh,
    AssetKind::Character,
    AssetKind::Weapon,
    AssetKind::Vehicle,
    AssetKind::Prop,
    AssetKind::Texture,
    AssetKind::Material,
    AssetKind::Audio,
    AssetKind::Video,
    AssetKind::Skybox,
    AssetKind::World,
    AssetKind::Prefab,
    AssetKind::Billboard,
];

pub fn server_kind_label(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Mesh => "mesh",
        AssetKind::Character => "character",
        AssetKind::Weapon => "weapon",
        AssetKind::Vehicle => "vehicle",
        AssetKind::Prop => "prop",
        AssetKind::Texture => "texture",
        AssetKind::Material => "material",
        AssetKind::Audio => "audio",
        AssetKind::Video => "video",
        AssetKind::Skybox => "skybox",
        AssetKind::World => "world",
        AssetKind::Prefab => "prefab",
        AssetKind::Billboard => "billboard",
        AssetKind::Game => "game",
    }
}

/// Explicit lifecycle for one remote resource: "empty because loading" and
/// "empty because failed" never render the same way.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Remote<T> {
    #[default]
    Idle,
    Loading,
    Ready(T),
    Failed(String),
}

impl<T> Remote<T> {
    pub fn ready(&self) -> Option<&T> {
        match self {
            Remote::Ready(value) => Some(value),
            _ => None,
        }
    }
}

/// One verified server identity (set exactly when a session is up). Kept
/// separate from the (unconstructible-in-tests) runtime handles so row
/// builders and tests read the same field.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerInfo {
    pub label: String,
    pub server_id: [u8; 16],
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ServerFilters {
    pub text: String,
    pub kind: Option<AssetKind>,
    pub category: Option<String>,
    pub tag: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SearchResults {
    pub hits: Vec<CatalogHit>,
    pub total: u64,
    /// A further page exists server-side (cursor held, not yet fetched).
    pub more: bool,
    /// Label counts for the WHOLE result set, most used first — the Library
    /// facets. Counted by the server in the same snapshot as the hits, so
    /// they always describe the rows on screen.
    pub facets: Vec<CatalogFacet>,
}

/// Session-backed store state. All `pub` fields are render inputs; mutation
/// happens through `start`/`poll`/`submit_search`/`select` only.
#[derive(Default)]
pub struct AssetStore {
    /// Continuous ai-content-library → catalog publisher. Declared BEFORE
    /// `embedded` so it is joined while the server it publishes into is
    /// still alive.
    publish: Option<PublishLoop>,
    /// In-process job coordinator: claims generation jobs queued on the
    /// hosted server (the VJ's GEN tab, chat) and dispatches them to the
    /// LAN fleet. Without it those jobs sit at "waiting for agent" forever.
    jobs: Option<JobLoop>,
    /// In-process Asset Server. Held so drop shuts it down with the app.
    embedded: Option<makepad_asset_store::AssetServer>,
    connector: Option<SessionConnector>,
    handles: Option<SessionHandles>,
    /// Verified identity once connected — the row builders' truth for
    /// "connected".
    pub server: Option<ServerInfo>,
    /// Copied from the live session so chat can open a second client.
    pub endpoints: Option<ApiEndpoints>,
    pub token: Option<String>,
    /// Latest connector status (Discovering/Connecting/Retrying/Connected).
    pub status: Option<SessionStatus>,
    /// `SessionConnector::start` refused the config (bad env spec shape).
    pub start_error: Option<String>,
    pub filters: ServerFilters,
    pub search: Remote<SearchResults>,
    search_req: Option<RequestId>,
    next_cursor: Option<PageCursor>,
    pub selected: Option<AssetId>,
    pub detail: Remote<AssetDetailDto>,
    detail_req: Option<RequestId>,
    /// Advertised generation capabilities (`/v1/jobs/profiles`) — the REAL
    /// server-side generation surface for the Runs panel.
    pub profiles: Remote<Vec<JobProfileDto>>,
    profiles_req: Option<RequestId>,
    /// Committed catalog events, newest first, capped.
    pub events: VecDeque<CatalogEventDto>,
    /// The event feed delivered its initial cursor and is following commits.
    pub events_live: bool,
    /// Latest feed diagnostics (poll retry, resync) — honest, transient.
    pub event_note: Option<String>,
    refresh_after_events: bool,
    /// Assets a catalog event touched since the app last looked. The viewer
    /// drains this to re-open what it is showing: a new revision means a new
    /// blob digest, so re-resolving is the whole of "stay current".
    changed_assets: Vec<AssetId>,
    /// In-flight `RetireAsset`/`RetireRevision` requests, tracked only to
    /// surface a failure (or a mismatched output) honestly — success is
    /// applied locally via [`AssetStore::on_retired`] the moment the
    /// response lands, no separate poll needed. Several can be in flight
    /// at once (a batch "delete shown" retiring many assets), so this is a
    /// set rather than the single-slot pattern `search_req`/`detail_req`
    /// use.
    retire_reqs: Vec<RequestId>,
    /// Latest blob-GC run status — a dry run's counts, or a collect's
    /// progress. `Remote::Idle` before any run this session has touched.
    pub gc: Remote<GcStatusDto>,
    /// The current `GcBlobs` bounded step in flight, if any. A run is
    /// driven to `done` by resubmitting the SAME request shape each time
    /// its step completes (`gc_blobs` is one bounded unit of work per
    /// call, never a background loop the server keeps running on its own).
    gc_req: Option<RequestId>,
    /// A `GcCancel` in flight — tracked separately from `gc_req` so the
    /// cancel button stays live even while a bounded step's HTTP round
    /// trip is outstanding.
    gc_cancel_req: Option<RequestId>,
    /// Set by `gc_cancel()`; checked before auto-resubmitting the next
    /// bounded step so a cancel can never lose a race with the next step
    /// already being in flight. Cleared when a fresh `gc_dry_run`/
    /// `gc_collect` starts.
    gc_cancel_requested: bool,
}

/// Transitional source-level name while the app moves from its former
/// passive snapshot to this live session-backed store. This aliases the real
/// implementation; it does not maintain a parallel compatibility state.
pub type AssetStoreState = AssetStore;

impl AssetStore {
    /// Launch the background connect lifecycle (idempotent; call once).
    ///
    /// Unless `AI_CONTENT_ASSET_SERVER` pins an external pair of planes,
    /// this starts a real Asset Server in-process (HTTP + UDP beacon) and
    /// the client finds it through the same discovery/health path any LAN
    /// peer would. Set the env var to skip embed and talk to a standalone
    /// server instead.
    pub fn start(&mut self, library_dir: PathBuf) {
        if self.connector.is_some() || self.server.is_some() {
            return;
        }
        let mut config = session_config_from_env();
        if config.endpoints.is_none() {
            match start_embedded_asset_server() {
                Ok((server, token)) => {
                    config.server_id = Some(server.server_id());
                    // Only the HOSTING process publishes the library. When
                    // we merely attached to someone else's server, that
                    // process owns its own library and this one must not
                    // push a second copy of the same rows.
                    self.publish = start_publish_loop(&server, &token, library_dir);
                    self.jobs = start_job_loop(&server, &token);
                    if config.token.is_none() {
                        config.token = Some(token);
                    }
                    self.embedded = Some(server);
                }
                Err(error) => {
                    // Another process already owns this catalog. Join it
                    // instead of painting a fatal CONFIG ERROR.
                    if let Some(existing) = attach_running_asset_server() {
                        if config.endpoints.is_none() {
                            config.endpoints = existing.endpoints;
                        }
                        if config.server_id.is_none() {
                            config.server_id = existing.server_id;
                        }
                        if config.token.is_none() {
                            config.token = existing.token;
                        }
                    } else {
                        self.start_error = Some(error);
                        return;
                    }
                }
            }
        }
        match SessionConnector::start(config) {
            Ok(connector) => {
                self.connector = Some(connector);
                self.status = Some(SessionStatus::Discovering);
            }
            Err(error) => {
                self.start_error = Some(error.to_string());
            }
        }
    }

    pub fn connected(&self) -> bool {
        self.server.is_some()
    }

    /// One status line for the connection chip and the honest empty states.
    pub fn status_label(&self) -> String {
        if let Some(error) = &self.start_error {
            return format!("SERVER · CONFIG ERROR · {error}");
        }
        match (&self.server, &self.status) {
            (Some(server), _) => {
                if self.embedded.is_some() {
                    format!("SERVER · local · {}", server.label)
                } else {
                    format!("SERVER · {}", server.label)
                }
            }
            (None, Some(SessionStatus::Discovering)) => {
                "SERVER · discovering on the LAN…".to_string()
            }
            (None, Some(SessionStatus::Connecting { server })) => {
                format!("SERVER · connecting {server}…")
            }
            (None, Some(SessionStatus::Retrying { error, in_secs })) => {
                format!("SERVER · retrying in {in_secs}s — {error}")
            }
            (None, Some(SessionStatus::Connected { server })) => {
                format!("SERVER · {server}")
            }
            (None, None) => "SERVER · not started".to_string(),
        }
    }

    /// Drain connector/runtime/subscriber channels. Non-blocking; returns
    /// true when any render input changed.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some(connector) = &mut self.connector {
            for msg in connector.poll() {
                changed = true;
                match msg {
                    SessionMsg::Status(status) => self.status = Some(status),
                    SessionMsg::Up(handles) => {
                        self.server = Some(ServerInfo {
                            label: handles.server_label.clone(),
                            server_id: handles.server_id,
                        });
                        self.endpoints = Some(handles.endpoints);
                        self.token = handles.token.clone();
                        self.handles = Some(*handles);
                        self.connector = None;
                        // First real loads the moment the session is up.
                        self.submit_search();
                        self.submit_profiles();
                        break;
                    }
                }
            }
        }
        let mut catalog_events = Vec::new();
        let mut feed_events = Vec::new();
        if let Some(handles) = &mut self.handles {
            catalog_events = handles.catalog.poll();
            feed_events = handles.subscriber.poll();
        }
        for event in catalog_events {
            changed |= self.on_catalog_event(event);
        }
        for event in feed_events {
            changed = true;
            self.on_feed_event(event);
        }
        if self.refresh_after_events && !matches!(self.search, Remote::Loading) {
            self.refresh_after_events = false;
            self.submit_search();
        }
        changed
    }

    /// (Re)run the catalog search for the current filters; empty text is
    /// browse mode. The previous in-flight request is cancelled.
    pub fn submit_search(&mut self) {
        let query = CatalogQuery {
            // The Library's facet row is the catalog's own label counts, so
            // every search asks for them; nothing else in the app does.
            facets: SEARCH_FACETS,
            text: self.filters.text.trim().to_string(),
            namespace: None,
            kind: self.filters.kind,
            category: self.filters.category.clone(),
            tag: self.filters.tag.clone(),
            exclude_tag: None,
            creator: None,
            live_only: false,
            page_size: SEARCH_PAGE_SIZE,
        };
        let Some(handles) = &mut self.handles else { return };
        if let Some(previous) = self.search_req.take() {
            handles.catalog.cancel(previous);
        }
        self.next_cursor = None;
        match handles.catalog.submit(ClientRequest::CatalogSearch {
            query,
            cursor: None,
        }) {
            Ok(id) => {
                self.search_req = Some(id);
                self.search = Remote::Loading;
            }
            Err(error) => self.search = Remote::Failed(error.to_string()),
        }
    }

    /// Take the assets catalog events touched since the last call.
    pub fn take_changed_assets(&mut self) -> Vec<AssetId> {
        std::mem::take(&mut self.changed_assets)
    }

    /// Select a catalog asset and load its candidate/revision detail.
    pub fn select(&mut self, id: AssetId) {
        self.selected = Some(id);
        let Some(handles) = &mut self.handles else { return };
        if let Some(previous) = self.detail_req.take() {
            handles.catalog.cancel(previous);
        }
        match handles.catalog.submit(ClientRequest::AssetDetail { id }) {
            Ok(request) => {
                self.detail_req = Some(request);
                self.detail = Remote::Loading;
            }
            Err(error) => self.detail = Remote::Failed(error.to_string()),
        }
    }

    /// Delete an asset from the store — every revision retired, aliases and
    /// search rows gone. Idempotent server-side. Fire-and-forget from the
    /// caller's perspective: success is applied to `search`/`detail`
    /// locally the moment the response lands ([`Self::on_retired`]); the
    /// catalog event feed (`AssetRetired`) confirms it for every OTHER
    /// client watching too.
    pub fn retire_asset(&mut self, id: AssetId) {
        self.submit_retire(ClientRequest::RetireAsset { id });
    }

    /// Delete one revision (typically superseded); the asset stays live if
    /// other revisions remain.
    pub fn retire_revision(&mut self, id: AssetId, revision: AssetRevisionId) {
        self.submit_retire(ClientRequest::RetireRevision { id, revision });
    }

    fn submit_retire(&mut self, request: ClientRequest) {
        let Some(handles) = &mut self.handles else { return };
        match handles.catalog.submit(request) {
            Ok(id) => self.retire_reqs.push(id),
            Err(error) => log!("asset store: retire failed to submit: {error}"),
        }
    }

    /// Apply a completed retirement locally: drop the row from the current
    /// search page immediately (don't wait for a re-search), and if the
    /// retired asset is the one currently selected, reload its detail so
    /// the panel shows the server's own `retired: true` rather than going
    /// stale silently.
    fn on_retired(&mut self, dto: RetireDto) {
        if let Remote::Ready(results) = &mut self.search {
            let before = results.hits.len();
            results.hits.retain(|hit| hit.asset_id != dto.asset_id);
            let dropped = before - results.hits.len();
            results.total = results.total.saturating_sub(dropped as u64);
        }
        if self.selected == Some(dto.asset_id) {
            self.select(dto.asset_id);
        }
    }

    /// True while ANY blob-GC run (ours or another admin's) is mid-progress
    /// — a dry run or a collect that has not yet reported `done`.
    pub fn gc_busy(&self) -> bool {
        self.gc_req.is_some() || matches!(&self.gc, Remote::Ready(status) if !status.done)
    }

    /// Preview: count what a collect would free without deleting anything.
    /// Drives itself to completion (repeated bounded steps) via
    /// [`Self::on_catalog_event`]; poll `self.gc` for progress.
    pub fn gc_dry_run(&mut self, retain_per_asset: Option<u32>) {
        self.gc_cancel_requested = false;
        self.submit_gc(GcRequest { retain_per_asset, ..GcRequest::dry_run() });
    }

    /// Actually delete unreferenced blobs (and, if `retain_per_asset` is
    /// set, retire older revisions beyond that count first). Same
    /// self-driving step loop as `gc_dry_run`.
    pub fn gc_collect(&mut self, retain_per_asset: Option<u32>) {
        self.gc_cancel_requested = false;
        self.submit_gc(GcRequest { retain_per_asset, ..GcRequest::collect() });
    }

    fn submit_gc(&mut self, request: GcRequest) {
        let Some(handles) = &mut self.handles else { return };
        // A step is already outstanding: let it land (and, via
        // `on_catalog_event`, decide whether to continue) rather than
        // racing a second bounded step in over it.
        if self.gc_req.is_some() {
            return;
        }
        match handles.catalog.submit(ClientRequest::GcBlobs { request }) {
            Ok(id) => {
                self.gc_req = Some(id);
                self.gc = Remote::Loading;
            }
            Err(error) => self.gc = Remote::Failed(error.to_string()),
        }
    }

    /// Abandon the active run. Tracked on its OWN request slot (not
    /// `gc_req`) so it can be submitted even while a bounded step's HTTP
    /// round trip is still outstanding — the step in flight is left to
    /// land normally, but `gc_cancel_requested` stops it from being
    /// followed by another one.
    pub fn gc_cancel(&mut self) {
        let Some(handles) = &mut self.handles else { return };
        if self.gc_cancel_req.is_some() {
            return;
        }
        self.gc_cancel_requested = true;
        match handles.catalog.submit(ClientRequest::GcCancel) {
            Ok(id) => self.gc_cancel_req = Some(id),
            Err(error) => log!("asset store: gc cancel failed to submit: {error}"),
        }
    }

    fn submit_profiles(&mut self) {
        let Some(handles) = &mut self.handles else { return };
        match handles
            .catalog
            .submit(ClientRequest::FetchJobProfiles { domain: None })
        {
            Ok(id) => {
                self.profiles_req = Some(id);
                self.profiles = Remote::Loading;
            }
            Err(error) => self.profiles = Remote::Failed(error.to_string()),
        }
    }

    fn on_catalog_event(&mut self, event: ClientEvent) -> bool {
        let id = event.id();
        if let Some(pos) = self.retire_reqs.iter().position(|r| *r == id) {
            self.retire_reqs.remove(pos);
            match event {
                ClientEvent::Started { .. } | ClientEvent::Progress { .. } => return false,
                ClientEvent::Done { output: ClientOutput::Retired(dto), .. } => {
                    self.on_retired(dto);
                }
                ClientEvent::Done { output, .. } => {
                    log!("asset store: unexpected retire output {output:?}");
                }
                ClientEvent::Failed { error, .. } => {
                    log!("asset store: retire failed: {error}");
                }
            }
            return true;
        }
        if Some(id) == self.gc_cancel_req {
            match event {
                ClientEvent::Started { .. } | ClientEvent::Progress { .. } => return false,
                ClientEvent::Done { output: ClientOutput::GcCancelled(stopped), .. } => {
                    self.gc_cancel_req = None;
                    log!("asset store: gc cancel — was running: {stopped}");
                }
                ClientEvent::Done { output, .. } => {
                    self.gc_cancel_req = None;
                    log!("asset store: unexpected gc-cancel output {output:?}");
                }
                ClientEvent::Failed { error, .. } => {
                    self.gc_cancel_req = None;
                    log!("asset store: gc cancel failed: {error}");
                }
            }
            return true;
        }
        if Some(id) == self.gc_req {
            match event {
                ClientEvent::Started { .. } | ClientEvent::Progress { .. } => return false,
                ClientEvent::Done { output: ClientOutput::Gc(status), .. } => {
                    self.gc_req = None;
                    let next = gc_continuation(&status, self.gc_cancel_requested);
                    self.gc = Remote::Ready(status);
                    // Drive a multi-step run to completion transparently —
                    // the caller (`gc_dry_run`/`gc_collect`) fired once;
                    // `self.gc` keeps reporting live progress either way.
                    if let Some(request) = next {
                        self.submit_gc(request);
                    }
                }
                ClientEvent::Done { output, .. } => {
                    self.gc_req = None;
                    self.gc = Remote::Failed(format!("unexpected output {output:?}"));
                }
                ClientEvent::Failed { error, .. } => {
                    self.gc_req = None;
                    self.gc = Remote::Failed(error.to_string());
                }
            }
            return true;
        }
        let slot = if Some(id) == self.search_req {
            0
        } else if Some(id) == self.detail_req {
            1
        } else if Some(id) == self.profiles_req {
            2
        } else {
            return false;
        };
        match event {
            ClientEvent::Started { .. } | ClientEvent::Progress { .. } => false,
            ClientEvent::Done { output, .. } => {
                match (slot, output) {
                    (0, ClientOutput::CatalogPage(page)) => {
                        self.search_req = None;
                        self.next_cursor = page.next.clone();
                        self.search = Remote::Ready(SearchResults {
                            more: page.next.is_some(),
                            hits: page.hits,
                            total: page.total,
                            facets: page.facets,
                        });
                        // A vanished selection stays selected but its detail
                        // panel reloads honestly on the next click.
                    }
                    (1, ClientOutput::AssetDetail(detail)) => {
                        self.detail_req = None;
                        self.detail = Remote::Ready(detail);
                    }
                    (2, ClientOutput::JobProfiles(profiles)) => {
                        self.profiles_req = None;
                        self.profiles = Remote::Ready(profiles);
                    }
                    // A mismatched output shape for a tracked id is a
                    // protocol-level surprise — surface it, don't guess.
                    (0, other) => {
                        self.search_req = None;
                        self.search = Remote::Failed(format!("unexpected output {other:?}"));
                    }
                    (1, other) => {
                        self.detail_req = None;
                        self.detail = Remote::Failed(format!("unexpected output {other:?}"));
                    }
                    (_, other) => {
                        self.profiles_req = None;
                        self.profiles = Remote::Failed(format!("unexpected output {other:?}"));
                    }
                }
                true
            }
            ClientEvent::Failed { error, .. } => {
                match slot {
                    0 => {
                        self.search_req = None;
                        self.search = Remote::Failed(error.to_string());
                    }
                    1 => {
                        self.detail_req = None;
                        self.detail = Remote::Failed(error.to_string());
                    }
                    _ => {
                        self.profiles_req = None;
                        self.profiles = Remote::Failed(error.to_string());
                    }
                }
                true
            }
        }
    }

    fn on_feed_event(&mut self, event: CatalogSubscriptionEvent) {
        match event {
            CatalogSubscriptionEvent::Ready { .. } => {
                self.events_live = true;
                self.event_note = None;
            }
            CatalogSubscriptionEvent::Events { events, .. } => {
                self.append_feed_events(events);
                self.refresh_after_events = true;
            }
            CatalogSubscriptionEvent::ResyncRequired { .. } => {
                self.events.clear();
                self.events_live = true;
                self.event_note =
                    Some("event retention lost / server restart — catalog re-listed".to_string());
                self.refresh_after_events = true;
            }
            CatalogSubscriptionEvent::Retry { error, retry_in_ms } => {
                self.events_live = false;
                self.event_note = Some(format!(
                    "event poll failed ({error}) — retrying in {}s",
                    retry_in_ms.div_ceil(1000)
                ));
            }
        }
    }

    fn append_feed_events(&mut self, events: Vec<CatalogEventDto>) {
        for event in &events {
            // Quarantine/retirement from ANY client (another admin, a GC
            // collect that retired excess revisions) drops the row here
            // immediately — `refresh_after_events`'s follow-up search
            // would eventually agree, but there is no reason to keep
            // showing content the server has already removed.
            if event.kind.removes_content() {
                if let (Remote::Ready(results), Some(asset_id)) = (&mut self.search, event.asset_id) {
                    let before = results.hits.len();
                    results.hits.retain(|hit| hit.asset_id != asset_id);
                    let dropped = before - results.hits.len();
                    results.total = results.total.saturating_sub(dropped as u64);
                }
            }
        }
        for event in events {
            if let Some(asset_id) = event.asset_id {
                if !self.changed_assets.contains(&asset_id) {
                    self.changed_assets.push(asset_id);
                }
            }
            self.events.push_front(event);
        }
        self.events.truncate(EVENT_LOG_CAP);
    }
}

/// Pure keep-going decision for a multi-step GC run: after one bounded step
/// reports `status`, should the caller submit another (and with what
/// request)? `None` when the run finished or a cancel was requested — EITHER
/// stops the drive loop, cancel winning even mid-run. `Some(request)`
/// repeats the SAME shape (dry run stays a dry run, the retain policy
/// carries over) so a caller only ever chooses dry-run-vs-collect once, at
/// the start.
fn gc_continuation(status: &GcStatusDto, cancel_requested: bool) -> Option<GcRequest> {
    if status.done || cancel_requested {
        return None;
    }
    Some(GcRequest {
        dry_run: status.dry_run,
        retain_per_asset: status.retain_keep.and_then(|n| u32::try_from(n).ok()),
        ..GcRequest::default()
    })
}

// ---------------------------------------------------------------------------
// Env conventions (pure parsers unit-tested below)
// ---------------------------------------------------------------------------

/// `ip:controlport:dataport` → endpoints.
pub fn parse_server_spec(spec: &str) -> Option<ApiEndpoints> {
    let mut parts = spec.trim().split(':');
    let ip: IpAddr = parts.next()?.parse().ok()?;
    let control: u16 = parts.next()?.parse().ok()?;
    let data: u16 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(ApiEndpoints {
        control: SocketAddr::new(ip, control),
        data: SocketAddr::new(ip, data),
    })
}

pub fn parse_hex16(text: &str) -> Option<[u8; 16]> {
    let bytes = text.trim().as_bytes();
    if bytes.len() != 32 {
        return None;
    }
    let value = |b: u8| match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    };
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = (value(bytes[i * 2])? << 4) | value(bytes[i * 2 + 1])?;
    }
    Some(out)
}

struct RunningServer {
    endpoints: Option<ApiEndpoints>,
    server_id: Option<[u8; 16]>,
    token: Option<String>,
}

fn checkout_root() -> PathBuf {
    if let Ok(root) = std::env::var("MAKEPAD_ROOT") {
        return PathBuf::from(root);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Checkout-local Asset UI home (`local/asset-ui`). Never `$HOME`.
pub fn asset_ui_home() -> PathBuf {
    checkout_root().join("local/asset-ui")
}

pub(crate) fn default_asset_server_root() -> PathBuf {
    if let Ok(root) = std::env::var("AI_CONTENT_ASSET_ROOT") {
        return PathBuf::from(root);
    }
    asset_ui_home().join("asset-server")
}

/// When the catalog root is already locked, read the live server's listen
/// address / id / admin token so the UI can connect as a client.
fn attach_running_asset_server() -> Option<RunningServer> {
    let root = default_asset_server_root();
    let token = std::fs::read_to_string(root.join("admin-token"))
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let server_id = std::fs::read_to_string(root.join("server-id"))
        .ok()
        .and_then(|t| parse_hex16(t.trim()));
    let endpoints = std::fs::read_to_string(root.join(makepad_asset_store::LISTEN_FILE))
        .ok()
        .and_then(|t| parse_server_spec(t.lines().next().unwrap_or("")));
    if token.is_none() && endpoints.is_none() && server_id.is_none() {
        return None;
    }
    Some(RunningServer {
        endpoints,
        server_id,
        token,
    })
}

fn start_embedded_asset_server() -> Result<(makepad_asset_store::AssetServer, String), String> {
    let root = default_asset_server_root();
    let mut cfg = makepad_asset_store::ServerConfig::new(root.clone());
    cfg.control_addr = "0.0.0.0:0"
        .parse()
        .map_err(|e| format!("control bind spec: {e}"))?;
    cfg.data_addr = "0.0.0.0:0"
        .parse()
        .map_err(|e| format!("data bind spec: {e}"))?;
    cfg.bootstrap_admin = true;
    cfg.discovery = Some(makepad_asset_store::DiscoveryConfig::lan_default());
    cfg.log = true;
    let server = makepad_asset_store::AssetServer::start(cfg)
        .map_err(|e| format!("embedded asset server: {e}"))?;
    let token = std::fs::read_to_string(root.join("admin-token"))
        .map_err(|e| format!("admin token: {e}"))?
        .trim()
        .to_string();
    if token.is_empty() {
        return Err("admin token file empty".into());
    }
    Ok((server, token))
}

/// Stop flag for the single in-process job coordinator (see PUBLISH_STOP).
static JOBS_STOP: AtomicBool = AtomicBool::new(false);

/// Owns the job-coordinator thread; dropping the store stops and joins it.
struct JobLoop {
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for JobLoop {
    fn drop(&mut self) {
        JOBS_STOP.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Claim + dispatch generation jobs from the hosted server to the fleet the
/// LAN announces (the same boxes the asset-ui's own pipelines use).
///
/// This runs the SHARED generation service, so the embedded server gets the
/// same behaviour as a standalone worker: one claim loop per fleet box (N
/// queued jobs of a kind drain across the N boxes that serve it), every
/// wired kind rather than video alone, and a live advertisement on
/// `GET /v1/job-profiles` of what those boxes can actually execute — which
/// is what stops a client enqueueing a tier whose weights are on no box.
fn start_job_loop(
    server: &makepad_asset_store::AssetServer,
    token: &str,
) -> Option<JobLoop> {
    let localize = |addr: SocketAddr| {
        if addr.ip().is_unspecified() {
            SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), addr.port())
        } else {
            addr
        }
    };
    let endpoints = ApiEndpoints {
        control: localize(server.control_addr()),
        data: localize(server.data_addr()),
    };
    let server_id = server.server_id();
    let token = token.to_string();
    let cache = asset_ui_home().join("jobs-cache");
    JOBS_STOP.store(false, Ordering::Release);
    let join = std::thread::Builder::new()
        .name("asset-ui-jobs".to_string())
        .spawn(move || {
            use makepad_asset_importer::gen_service::{FleetSource, GenServiceConfig};
            log!(
                "job loop: coordinating jobs on {}/{} → LAN fleet",
                endpoints.control,
                endpoints.data
            );
            makepad_asset_importer::gen_service::run(
                &GenServiceConfig {
                    servers: vec![endpoints],
                    server_id: Some(server_id),
                    token,
                    cache_root: cache,
                    namespace: "gen".to_string(),
                    suffix: "asset-ui".to_string(),
                    rights: makepad_asset_client::PublishRights::generated_cc0(),
                    fleet: FleetSource::Lan,
                    announce: true,
                    log: true,
                },
                &JOBS_STOP,
            );
            log!("job loop: stopped");
        });
    match join {
        Ok(join) => Some(JobLoop { join: Some(join) }),
        Err(error) => {
            log!("job loop: could not spawn: {error}");
            None
        }
    }
}

/// Stop flag for the single in-process publish loop. A `static` (not an
/// `Arc`) because `watch::run` borrows it for the thread's whole life and
/// there is at most one loop per process.
static PUBLISH_STOP: AtomicBool = AtomicBool::new(false);

/// Owns the publisher thread; dropping the store stops and joins it.
struct PublishLoop {
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for PublishLoop {
    fn drop(&mut self) {
        PUBLISH_STOP.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// Publish everything the pipelines write into the AI-content library into
/// the server this process hosts. Connect + poll happen on the thread, so a
/// slow or refused connection can never stall the UI, and every failure is
/// a log line — never a panic.
fn start_publish_loop(
    server: &makepad_asset_store::AssetServer,
    token: &str,
    library_dir: PathBuf,
) -> Option<PublishLoop> {
    // The server binds 0.0.0.0; reach it the way its own `listen` file
    // advertises it.
    let localize = |addr: SocketAddr| {
        if addr.ip().is_unspecified() {
            SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), addr.port())
        } else {
            addr
        }
    };
    let endpoints = ApiEndpoints {
        control: localize(server.control_addr()),
        data: localize(server.data_addr()),
    };
    let server_id = server.server_id();
    let token = token.to_string();
    let cache = asset_ui_home().join("publish-cache");
    PUBLISH_STOP.store(false, Ordering::Release);
    let join = std::thread::Builder::new()
        .name("asset-ui-publish".to_string())
        .spawn(move || {
            let mut config = makepad_asset_client::ClientConfig::new(cache);
            config.token = Some(token);
            let mut client = match makepad_asset_client::AssetClient::connect(
                config,
                endpoints,
                Some(server_id),
            ) {
                Ok(client) => client,
                Err(error) => {
                    log!("publish loop: cannot connect to the embedded server: {error}");
                    return;
                }
            };
            log!(
                "publish loop: watching {} → {}/{}",
                library_dir.display(),
                endpoints.control,
                endpoints.data
            );
            makepad_asset_importer::watch::run(
                &mut client,
                &library_dir,
                "gen",
                &makepad_asset_client::PublishRights::generated_cc0(),
                // Log publications, failures and retries; out-of-scope rows
                // (the pack-import bulk) stay silent by design.
                true,
                &PUBLISH_STOP,
            );
            log!("publish loop: stopped");
        });
    match join {
        Ok(join) => Some(PublishLoop { join: Some(join) }),
        Err(error) => {
            log!("publish loop: could not spawn: {error}");
            None
        }
    }
}

pub fn session_config_from_env() -> SessionConfig {
    let app_home = asset_ui_home();
    let cache_parent = env_alias(&["ASSET_UI_ASSET_CACHE", "AI_CONTENT_ASSET_CACHE"])
        .map(PathBuf::from)
        .unwrap_or_else(|| app_home.clone());
    let mut config = SessionConfig::new(cache_parent);
    config.endpoints = env_alias(&["ASSET_UI_ASSET_SERVER", "AI_CONTENT_ASSET_SERVER"])
        .and_then(|spec| parse_server_spec(&spec));
    config.server_id = env_alias(&["ASSET_UI_ASSET_SERVER_ID", "AI_CONTENT_ASSET_SERVER_ID"])
        .and_then(|text| parse_hex16(&text));
    config.token = env_alias(&["ASSET_UI_ASSET_TOKEN", "AI_CONTENT_ASSET_TOKEN"])
        .map(|token| token.trim().to_string())
        .filter(|token| !token.is_empty())
        .or_else(|| {
            let path = env_alias(&[
                "ASSET_UI_ASSET_TOKEN_FILE",
                "AI_CONTENT_ASSET_TOKEN_FILE",
            ])?;
            let text = std::fs::read_to_string(path).ok()?;
            let text = text.trim().to_string();
            (!text.is_empty()).then_some(text)
        })
        .or_else(|| {
            std::fs::read_to_string(app_home.join("asset-server").join("admin-token"))
                .ok()
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty())
        })
        .or_else(|| {
            std::fs::read_to_string(app_home.join("asset-server.token"))
                .ok()
                .map(|token| token.trim().to_string())
                .filter(|token| !token.is_empty())
        });
    config
}

pub(crate) fn env_alias(keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Ok(value) = std::env::var(key) {
            if !value.is_empty() {
                return Some(value);
            }
        }
    }
    None
}

pub fn hex16_string(id: &[u8; 16]) -> String {
    let mut out = String::with_capacity(32);
    for byte in id {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

// ---------------------------------------------------------------------------
// Library filter state. The Library surface is the server catalog, so these
// are mirrored straight onto the catalog query; `matches` is what the
// Create-surface History strip filters its own rows with.
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibraryFilters {
    /// Free text, straight onto the catalog query. The server's text index
    /// covers titles, categories and tags, so this reaches a label even
    /// before the facet row offers it.
    pub query: String,
    /// Selected `AssetKind` label, or None for "all kinds".
    pub kind: Option<String>,
    /// Picked facet: which vocabulary it came from (the server filters
    /// `category` and `tag` separately) and the label itself.
    pub label: Option<(FacetKind, String)>,
}

/// One Library shelf name. Worlds (old `world` domain and new `map`) show
/// as `maps` so imported levels are not lumped in with prop meshes.
pub fn library_type(domain: &str, content_type: &str) -> &'static str {
    crate::library::asset_shelf(domain, content_type)
}

pub fn local_kind<'a>(domain: &'a str, content_type: &'a str) -> &'a str {
    let media_type = content_type.to_ascii_lowercase();
    if domain.eq_ignore_ascii_case("billboard") || media_type.contains("billboard") {
        "billboard"
    } else if media_type.starts_with("image/") {
        "image"
    } else if media_type.starts_with("video/") {
        "video"
    } else if media_type.starts_with("audio/") {
        "audio"
    } else if media_type.contains("gltf") || media_type.contains("model/") {
        "mesh"
    } else if media_type.starts_with("text/") || media_type.contains("json") {
        "text"
    } else {
        domain
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_spec_parses_exactly_ip_control_data() {
        let endpoints = parse_server_spec("10.0.0.30:41870:41872").unwrap();
        assert_eq!(endpoints.control.port(), 41870);
        assert_eq!(endpoints.data.port(), 41872);
        assert_eq!(endpoints.control.ip(), endpoints.data.ip());
        assert!(parse_server_spec("10.0.0.30:41870").is_none(), "missing data port");
        assert!(parse_server_spec("10.0.0.30:41870:41872:9").is_none(), "extra field");
        assert!(parse_server_spec("host.local:1:2").is_none(), "hostnames not resolved here");
        assert!(parse_server_spec("10.0.0.30:x:2").is_none());
    }

    #[test]
    fn hex16_round_trips_and_rejects_bad_shapes() {
        let id = parse_hex16("00ff10a1b2c3d4e5f60718293a4b5c6d").unwrap();
        assert_eq!(id[0], 0x00);
        assert_eq!(id[1], 0xff);
        assert_eq!(hex16_string(&id), "00ff10a1b2c3d4e5f60718293a4b5c6d");
        assert!(parse_hex16("00ff").is_none());
        assert!(parse_hex16("00FF10a1b2c3d4e5f60718293a4b5c6d").is_none(), "uppercase");
        assert!(parse_hex16("zzff10a1b2c3d4e5f60718293a4b5c6d").is_none());
    }

    #[test]
    fn listen_spec_parses_like_env_pin() {
        let ep = parse_server_spec("127.0.0.1:9701:9702").unwrap();
        assert_eq!(ep.control.port(), 9701);
        assert_eq!(ep.data.port(), 9702);
    }

    /// The Library's only facet dropdown is the server kind vocabulary, and
    /// a pick travels back as a LABEL that `read_filters_from_ui` looks up
    /// in `SERVER_KINDS`. That round trip is only sound while the labels are
    /// unique and non-empty, so assert it here rather than discover a filter
    /// that silently selects the wrong kind.
    #[test]
    fn every_server_kind_has_its_own_label_to_filter_by() {
        let mut labels = Vec::new();
        for kind in SERVER_KINDS {
            let label = server_kind_label(kind);
            assert!(!label.is_empty(), "{kind:?} has no label");
            assert!(
                !labels.contains(&label),
                "label {label} is shared by two kinds — a dropdown pick would be ambiguous"
            );
            labels.push(label);
            let found = SERVER_KINDS
                .into_iter()
                .find(|candidate| server_kind_label(*candidate) == label);
            assert_eq!(found, Some(kind), "label {label} does not round-trip");
        }
        assert_eq!(labels.len(), SERVER_KINDS.len());
    }

    #[test]
    fn status_labels_are_honest_for_every_lifecycle_phase() {
        let mut store = AssetStore::default();
        assert_eq!(store.status_label(), "SERVER · not started");
        assert!(!store.connected());

        store.status = Some(SessionStatus::Discovering);
        assert!(store.status_label().contains("discovering"));
        store.status = Some(SessionStatus::Retrying {
            error: "unauthorized".into(),
            in_secs: 8,
        });
        assert!(store.status_label().contains("retrying in 8s"));
        assert!(store.status_label().contains("unauthorized"));

        store.server = Some(ServerInfo {
            label: "10.0.0.30:41870".into(),
            server_id: [7; 16],
        });
        assert!(store.connected());
        assert_eq!(store.status_label(), "SERVER · 10.0.0.30:41870");

        let mut broken = AssetStore::default();
        broken.start_error = Some("session cache leaves".into());
        assert!(broken.status_label().contains("CONFIG ERROR"));
    }

    #[test]
    fn feed_events_are_capped_newest_first_and_flag_refresh() {
        let mut store = AssetStore::default();
        let event = |seq: u64| CatalogEventDto {
            seq,
            kind: makepad_asset_client::CatalogEventKind::AssetPublished,
            namespace: "game".into(),
            asset_id: None,
            revision: None,
            game_id: None,
            game_revision: None,
            alias: Some(format!("game/asset-{seq}")),
            content_kind: None,
            ts_ms: seq,
        };
        // The cursor is deliberately opaque outside asset_client. Exercise
        // the exact bounded insertion path without forging protocol state.
        store.append_feed_events(
            (0..(EVENT_LOG_CAP as u64 + 10)).map(event).collect(),
        );
        store.refresh_after_events = true;
        assert_eq!(store.events.len(), EVENT_LOG_CAP);
        assert_eq!(store.events.front().unwrap().seq, EVENT_LOG_CAP as u64 + 9);
        assert!(store.refresh_after_events);

        store.on_feed_event(CatalogSubscriptionEvent::Retry {
            error: makepad_asset_client::ClientError::Protocol { what: "events page" },
            retry_in_ms: 2_500,
        });
        assert!(!store.events_live);
        assert!(store.event_note.as_deref().unwrap().contains("retrying in 3s"));
    }

    // -----------------------------------------------------------------
    // Retire + GC — the request/response dispatch is exercised directly
    // (fake `RequestId`s, hand-built `ClientEvent`s) rather than through a
    // live session: `submit_retire`/`submit_gc` need `self.handles`, which
    // only a connected `SessionConnector` provides, but the interesting
    // logic — "drop this row locally", "keep driving a multi-step GC run,
    // cancel wins" — lives entirely in the RECEIVING half, which needs no
    // network at all.
    // -----------------------------------------------------------------

    fn test_hit(asset_id: AssetId, title: &str) -> CatalogHit {
        CatalogHit {
            asset_id,
            namespace: "game".into(),
            kind: None,
            title: title.into(),
            snippet: String::new(),
            score: 0,
            live: true,
            alias: None,
        }
    }

    #[test]
    fn on_retired_drops_the_row_from_search_and_lowers_total() {
        let mut store = AssetStore::default();
        let keep = AssetId::from_bytes([1; 16]);
        let gone = AssetId::from_bytes([2; 16]);
        store.search = Remote::Ready(SearchResults {
            hits: vec![test_hit(keep, "Keep"), test_hit(gone, "Gone")],
            total: 2,
            more: false,
            facets: Vec::new(),
        });
        store.on_retired(RetireDto {
            asset_id: gone,
            revision: None,
            already_retired: false,
            revisions_retired: 1,
            aliases_dropped: 1,
            annotation_cleared: true,
        });
        let results = store.search.ready().unwrap();
        assert_eq!(results.hits.len(), 1);
        assert_eq!(results.hits[0].asset_id, keep);
        assert_eq!(results.total, 1);
    }

    #[test]
    fn on_retired_is_a_no_op_when_the_asset_is_not_in_the_current_page() {
        let mut store = AssetStore::default();
        let keep = AssetId::from_bytes([1; 16]);
        store.search = Remote::Ready(SearchResults {
            hits: vec![test_hit(keep, "Keep")],
            total: 1,
            more: false,
            facets: Vec::new(),
        });
        store.on_retired(RetireDto {
            asset_id: AssetId::from_bytes([9; 16]),
            revision: None,
            already_retired: false,
            revisions_retired: 1,
            aliases_dropped: 0,
            annotation_cleared: false,
        });
        let results = store.search.ready().unwrap();
        assert_eq!(results.hits.len(), 1);
        assert_eq!(results.total, 1);
    }

    #[test]
    fn retired_catalog_events_from_any_client_drop_the_row_too() {
        // The subscriber feed, not just our own retire response — a GC
        // collect's excess-revision retirement, or another admin's delete.
        let mut store = AssetStore::default();
        let keep = AssetId::from_bytes([1; 16]);
        let gone = AssetId::from_bytes([2; 16]);
        store.search = Remote::Ready(SearchResults {
            hits: vec![test_hit(keep, "Keep"), test_hit(gone, "Gone")],
            total: 2,
            more: false,
            facets: Vec::new(),
        });
        store.append_feed_events(vec![CatalogEventDto {
            seq: 1,
            kind: makepad_asset_client::CatalogEventKind::AssetRetired,
            namespace: "game".into(),
            asset_id: Some(gone),
            revision: None,
            game_id: None,
            game_revision: None,
            alias: None,
            content_kind: None,
            ts_ms: 1,
        }]);
        let results = store.search.ready().unwrap();
        assert_eq!(results.hits.len(), 1);
        assert_eq!(results.hits[0].asset_id, keep);
        // Published events (does NOT remove_content) never touch the page.
        let mut store2 = AssetStore::default();
        store2.search = Remote::Ready(SearchResults {
            hits: vec![test_hit(gone, "Gone")],
            total: 1,
            more: false,
            facets: Vec::new(),
        });
        store2.append_feed_events(vec![CatalogEventDto {
            seq: 2,
            kind: makepad_asset_client::CatalogEventKind::AssetPublished,
            namespace: "game".into(),
            asset_id: Some(gone),
            revision: None,
            game_id: None,
            game_revision: None,
            alias: None,
            content_kind: None,
            ts_ms: 2,
        }]);
        assert_eq!(store2.search.ready().unwrap().hits.len(), 1);
    }

    #[test]
    fn retire_dispatch_clears_the_tracked_request_on_success_and_on_failure() {
        let mut store = AssetStore::default();
        let target = AssetId::from_bytes([3; 16]);
        store.search = Remote::Ready(SearchResults {
            hits: vec![test_hit(target, "Target")],
            total: 1,
            more: false,
            facets: Vec::new(),
        });
        store.retire_reqs.push(42);
        let handled = store.on_catalog_event(ClientEvent::Done {
            id: 42,
            output: ClientOutput::Retired(RetireDto {
                asset_id: target,
                revision: None,
                already_retired: false,
                revisions_retired: 1,
                aliases_dropped: 0,
                annotation_cleared: false,
            }),
        });
        assert!(handled);
        assert!(store.retire_reqs.is_empty());
        assert!(store.search.ready().unwrap().hits.is_empty());

        // A second, unrelated retire failing must not disturb anything else
        // and must still clear its own slot (not get stuck forever).
        store.retire_reqs.push(43);
        let handled = store.on_catalog_event(ClientEvent::Failed {
            id: 43,
            error: makepad_asset_client::ClientError::Protocol { what: "retire" },
        });
        assert!(handled);
        assert!(store.retire_reqs.is_empty());

        // An id nobody is tracking (already cancelled, or someone else's)
        // is not our event.
        assert!(!store.on_catalog_event(ClientEvent::Done {
            id: 99,
            output: ClientOutput::GcCancelled(false),
        }));
    }

    fn test_gc_status(dry_run: bool, done: bool, retain_keep: Option<u64>) -> GcStatusDto {
        GcStatusDto {
            run_id: Some(7),
            phase: if done { makepad_asset_client::GcPhaseDto::Done } else { makepad_asset_client::GcPhaseDto::Sweep },
            done,
            dry_run,
            started_ms: 0,
            updated_ms: 0,
            horizon_ms: 0,
            retain_keep,
            retired_revisions: 0,
            scanned_revisions: 10,
            marked_blobs: 5,
            examined_blobs: 5,
            unreferenced_blobs: 3,
            unreferenced_bytes: 3_000_000,
            deleted_blobs: 0,
            deleted_bytes: 0,
        }
    }

    #[test]
    fn gc_continuation_repeats_the_same_shape_until_done_and_cancel_always_wins() {
        let running = test_gc_status(true, false, Some(3));
        assert_eq!(
            gc_continuation(&running, false),
            Some(GcRequest { dry_run: true, retain_per_asset: Some(3), ..GcRequest::default() }),
        );
        assert_eq!(gc_continuation(&running, true), None, "cancel wins even mid-run");
        let done = test_gc_status(true, true, Some(3));
        assert_eq!(gc_continuation(&done, false), None);
        let collecting = test_gc_status(false, false, None);
        assert_eq!(
            gc_continuation(&collecting, false),
            Some(GcRequest { dry_run: false, retain_per_asset: None, ..GcRequest::default() }),
        );
    }

    #[test]
    fn gc_dispatch_updates_status_and_reports_busy_while_not_done() {
        let mut store = AssetStore::default();
        assert!(!store.gc_busy());
        store.gc_req = Some(11);
        assert!(store.gc_busy(), "a step in flight counts as busy");
        let handled = store.on_catalog_event(ClientEvent::Done {
            id: 11,
            output: ClientOutput::Gc(test_gc_status(true, false, None)),
        });
        assert!(handled);
        // No live handles in this test, so the auto-continue submit is a
        // silent no-op (see `submit_gc`) — but the status itself, and the
        // fact a not-done run still reads busy, are exactly what the UI
        // renders and must be correct with or without a session.
        assert!(store.gc_req.is_none());
        assert!(store.gc_busy(), "reported status is not done yet");
        assert_eq!(store.gc.ready().unwrap().unreferenced_blobs, 3);

        store.gc_req = Some(12);
        let handled = store.on_catalog_event(ClientEvent::Done {
            id: 12,
            output: ClientOutput::Gc(test_gc_status(true, true, None)),
        });
        assert!(handled);
        assert!(!store.gc_busy(), "done run is idle");
    }

    #[test]
    fn gc_cancel_request_is_tracked_on_its_own_slot_independent_of_gc_req() {
        let mut store = AssetStore::default();
        store.gc_req = Some(21);
        store.gc_cancel_req = Some(22);
        // The bounded step lands first; cancel is still outstanding.
        let handled = store.on_catalog_event(ClientEvent::Done {
            id: 21,
            output: ClientOutput::Gc(test_gc_status(false, false, None)),
        });
        assert!(handled);
        assert!(store.gc_req.is_none());
        assert!(store.gc_cancel_req.is_some(), "cancel's own slot is untouched by the step landing");

        let handled = store.on_catalog_event(ClientEvent::Done {
            id: 22,
            output: ClientOutput::GcCancelled(true),
        });
        assert!(handled);
        assert!(store.gc_cancel_req.is_none());
    }

    // -----------------------------------------------------------------
    // Live: an ISOLATED in-process `AssetServer` (own temp root, ephemeral
    // ports) driven through the REAL `AssetStore::poll()` loop — the exact
    // "UI action" path (`retire_asset`, `gc_dry_run`/`gc_collect`), never
    // the shared live asset-ui server this session must not touch.
    // -----------------------------------------------------------------

    use std::str::FromStr;

    fn live_test_root(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "mp_asset_store_state_{}_{}_{}",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ))
    }

    /// Own ephemeral-port server + admin token, isolated data root. Mirrors
    /// `libs/asset/store/tests/retire_gc_http.rs::start_server` (the
    /// existing HTTP-level delete/GC contract test) but lives here because
    /// driving it through `AssetStore::poll()` needs this module's private
    /// `connector`/`handles` fields.
    fn start_isolated_server(name: &str) -> (makepad_asset_store::AssetServer, String) {
        let root = live_test_root(name);
        let mut cfg = makepad_asset_store::ServerConfig::new(root.clone());
        cfg.control_addr = "127.0.0.1:0".parse().unwrap();
        cfg.data_addr = "127.0.0.1:0".parse().unwrap();
        cfg.bootstrap_admin = true;
        cfg.log = false;
        // Deterministic: no background janitor stealing GC steps, no grace
        // window holding fresh blobs back from an immediate collect.
        cfg.gc_janitor_steps = 0;
        cfg.gc_grace_ms = 0;
        let server = makepad_asset_store::AssetServer::start(cfg).expect("server start");
        let token = std::fs::read_to_string(root.join("admin-token"))
            .expect("admin token")
            .trim()
            .to_string();
        (server, token)
    }

    /// Publish one minimal real asset directly (the synchronous
    /// `AssetClient`, not through `AssetStore` — publishing is the import
    /// pipeline's job, already covered elsewhere; this test starts from
    /// "an asset already exists in the catalog").
    fn publish_test_asset(
        server: &makepad_asset_store::AssetServer,
        token: &str,
        alias: &str,
        fill: u8,
    ) -> AssetId {
        let mut cfg = makepad_asset_client::ClientConfig::new(live_test_root("publish-cache"));
        cfg.token = Some(token.to_string());
        let endpoints = ApiEndpoints { control: server.control_addr(), data: server.data_addr() };
        let mut client = makepad_asset_client::AssetClient::connect(cfg, endpoints, Some(server.server_id()))
            .expect("publish client connect");
        let mut request = makepad_asset_client::PublishRequest::new(
            "gen",
            AssetKind::Video,
            "store delete/gc test asset",
            makepad_asset_client::PublishFile {
                bytes: vec![fill; 4_096],
                media: makepad_asset_data::MediaType::Mp4,
                role: makepad_asset_data::FileRole::Video,
                media_millis: 1_000,
                dims: None,
            },
            makepad_asset_client::PublishThumbnail {
                bytes: vec![fill ^ 0xFF; 1_024],
                media: makepad_asset_data::ThumbnailMedia::Png,
                width: 512,
                height: 512,
            },
        );
        request.alias = Some(makepad_asset_data::AssetAlias::from_str(alias).unwrap());
        client.publish_artifact(&request).expect("publish").asset_id
    }

    /// Connect `store` to `server` through the REAL `SessionConnector` +
    /// `AssetStore::poll()` loop (no discovery — explicit endpoints), and
    /// wait for the initial auto-search `poll()` fires on connect to land.
    fn connect_store_to(store: &mut AssetStore, server: &makepad_asset_store::AssetServer, token: &str) {
        let config = SessionConfig {
            endpoints: Some(ApiEndpoints { control: server.control_addr(), data: server.data_addr() }),
            server_id: Some(server.server_id()),
            token: Some(token.to_string()),
            ..SessionConfig::new(live_test_root("session-cache"))
        };
        store.connector = Some(SessionConnector::start(config).expect("connector start"));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            store.poll();
            if matches!(store.search, Remote::Ready(_)) {
                return;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "session never connected + searched: status={:?} search={:?}",
                store.status,
                store.search
            );
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }

    fn poll_until<F: Fn(&AssetStore) -> bool>(store: &mut AssetStore, what: &str, ready: F) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            store.poll();
            if ready(store) {
                return;
            }
            assert!(std::time::Instant::now() < deadline, "timed out waiting for: {what}");
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }

    #[test]
    fn retire_asset_through_the_ui_action_removes_it_from_the_list() {
        let (server, token) = start_isolated_server("retire_ui");
        let asset_id = publish_test_asset(&server, &token, "gen/retire-ui-test", 7);

        let mut store = AssetStore::default();
        connect_store_to(&mut store, &server, &token);
        assert!(
            store.search.ready().unwrap().hits.iter().any(|hit| hit.asset_id == asset_id),
            "the published asset must be visible before we retire it"
        );

        store.retire_asset(asset_id);
        poll_until(&mut store, "retire to apply locally", |store| {
            store
                .search
                .ready()
                .is_some_and(|results| !results.hits.iter().any(|hit| hit.asset_id == asset_id))
        });

        // The server agrees too — not just our own optimistic local drop.
        poll_until(&mut store, "server search catches up via re-search", |store| {
            store.retire_reqs.is_empty()
        });
        store.submit_search();
        poll_until(&mut store, "re-search after retire", |store| {
            matches!(store.search, Remote::Ready(_))
        });
        assert!(
            !store.search.ready().unwrap().hits.iter().any(|hit| hit.asset_id == asset_id),
            "retirement must hold on a FRESH server search, not just the optimistic local edit"
        );
    }

    #[test]
    fn gc_dry_run_then_collect_against_an_in_process_store() {
        let (server, token) = start_isolated_server("gc_ui");
        let asset_id = publish_test_asset(&server, &token, "gen/gc-ui-test", 9);

        let mut store = AssetStore::default();
        connect_store_to(&mut store, &server, &token);
        store.retire_asset(asset_id);
        poll_until(&mut store, "retire before gc", |store| store.retire_reqs.is_empty());

        store.gc_dry_run(None);
        assert!(store.gc_busy());
        poll_until(&mut store, "dry run to finish", |store| {
            matches!(&store.gc, Remote::Ready(status) if status.done)
        });
        let dry = *store.gc.ready().unwrap();
        assert!(dry.dry_run);
        assert!(
            dry.unreferenced_blobs >= 2,
            "the retired asset's artifact + thumbnail blobs must show up as reclaimable: {dry:?}"
        );
        assert_eq!(dry.deleted_blobs, 0, "a dry run must never actually delete anything");

        store.gc_collect(None);
        poll_until(&mut store, "collect to finish", |store| {
            matches!(&store.gc, Remote::Ready(status) if status.done)
        });
        let collected = *store.gc.ready().unwrap();
        assert!(!collected.dry_run);
        assert_eq!(
            collected.deleted_blobs, dry.unreferenced_blobs,
            "collect must reclaim exactly what the dry run counted"
        );
        assert!(collected.deleted_bytes > 0);
        assert!(!store.gc_busy());
    }
}
