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
//! When another process already holds the root lock this one ATTACHES to the
//! live server instead. An attached instance has SUCCESSION: it watches the
//! server it joined, and when that server goes silent it tries the root lock
//! itself — winning means starting the embedded server here (publisher, job
//! coordinator, a rewritten `listen` file) and reconnecting to itself, losing
//! means re-reading `listen` and rejoining whoever won. There is no state in
//! which this app looks healthy while its server is gone: see
//! [`AssetStore::status_label`].
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
//! - `ASSET_UI_ASSET_BEACON=off` — start the embedded server WITHOUT its LAN
//!   beacon. An instance on an isolated store root (`AI_CONTENT_ASSET_ROOT`)
//!   must not advertise itself to peers hunting for the real one.
//! - `ASSET_UI_ASSET_EMBED=never` — NEVER hold the root: attach only, and
//!   wait rather than embed when nothing is serving it yet. This is the
//!   setting for the recommended deployment, where `makepad-asset-server`
//!   owns the store and every app is a client of it
//!   (`apps/asset-server/README.md`).

use makepad_asset_client::{
    ApiEndpoints, AssetDetailDto, CatalogEventDto, CatalogFacet, CatalogHit, CatalogQuery,
    CatalogSubscriptionEvent, ClientError, ClientEvent, ClientOutput, ClientRequest, GcRequest,
    GcStatusDto, JobProfileDto, PageCursor, PipelineId, RequestId, RetireDto, SessionConfig,
    SessionConnector, SessionHandles, SessionMsg, SessionStatus,
};
use makepad_asset_data::{AssetId, AssetRevisionId};
pub use makepad_asset_data::AssetKind;
use makepad_widgets::log;
use makepad_widgets::makepad_platform::thread::{
    Lane, TaskHandle, TaskPool, ThreadOptions, ThreadSpawner,
};
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Committed catalog events retained for the Admin surface (newest first).
pub const EVENT_LOG_CAP: usize = 200;
/// One search page. The server caps at MAX_SEARCH_LIMIT (100); more rows
/// exist server-side when `SearchResults::more` is set.
pub const SEARCH_PAGE_SIZE: u32 = 100;

/// Facet rows the Library asks for. The dropdown shows the most-used labels
/// first and a long tail helps nobody find anything.
pub const SEARCH_FACETS: u32 = 24;

/// How many rows of one result set the Library holds. A catalog can be far
/// larger than anything a person scrolls; past this the count label says so
/// rather than the app quietly pretending the rest does not exist.
pub const MAX_CATALOG_ROWS: usize = 4096;

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

/// The structural labels an audio asset carries: what every importer writes
/// as its CATEGORY for a track (`music`) or a one-shot (`sfx`). They are the
/// reason the Kind picker can offer the split at all, and the reason the tag
/// list must not repeat them — see [`KindChoice`].
pub const AUDIO_SHELVES: [&str; 2] = ["music", "sfx"];

/// One row of the Library's Kind picker.
///
/// Kinds come from the content contract, with ONE refinement: audio splits
/// into music and sfx. A six-minute track and a door slam are different
/// things to go looking for, every importer already writes which of the two
/// it published, and the user asked for them separated *here* — in the
/// structural picker — rather than as two rows in the free tag list, where
/// they sat next to `bicep` and `unknownalbum` as if they were the same sort
/// of fact. `Audio` itself stays as the parent row: an audio asset labelled
/// neither would otherwise be unreachable.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KindChoice {
    /// No kind filter at all.
    Any,
    Kind(AssetKind),
    /// [`AssetKind::Audio`] narrowed to one of [`AUDIO_SHELVES`].
    AudioShelf(&'static str),
}

impl Default for KindChoice {
    fn default() -> Self {
        Self::Any
    }
}

impl KindChoice {
    /// Every row the picker offers, in order. Row 0 is "all kinds".
    pub fn rows() -> Vec<KindChoice> {
        let mut rows = vec![KindChoice::Any];
        for kind in SERVER_KINDS {
            rows.push(KindChoice::Kind(kind));
            if kind == AssetKind::Audio {
                rows.extend(AUDIO_SHELVES.map(KindChoice::AudioShelf));
            }
        }
        rows
    }

    /// What the row reads as. The refinements are shown under their parent
    /// so the picker still teaches the kind vocabulary.
    pub fn row_text(&self) -> String {
        match self {
            KindChoice::Any => "all kinds".to_string(),
            KindChoice::Kind(kind) => server_kind_label(*kind).to_string(),
            KindChoice::AudioShelf(shelf) => format!("audio · {shelf}"),
        }
    }

    /// The catalog query this row means: a kind, and the structural category
    /// that narrows it.
    pub fn query(&self) -> (Option<AssetKind>, Option<&'static str>) {
        match self {
            KindChoice::Any => (None, None),
            KindChoice::Kind(kind) => (Some(*kind), None),
            KindChoice::AudioShelf(shelf) => (Some(AssetKind::Audio), Some(shelf)),
        }
    }

    pub fn is_any(&self) -> bool {
        matches!(self, KindChoice::Any)
    }
}

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
        AssetKind::VjEffect => "vjeffect",
        AssetKind::Data => "data",
        AssetKind::ModelProgram => "model-program",
    }
}

/// How long an ATTACHED instance tolerates a silent server before it tries
/// to take the store root over. Long enough that a slow GC step or a
/// momentarily refused long-poll is not mistaken for a death, short enough
/// that nobody works for minutes against a server that is gone.
pub const TAKEOVER_AFTER_MS: u64 = 3_500;

/// Gap between takeover attempts while the root lock stays held (the owner
/// is alive but unreachable, or a rival instance won the race).
pub const TAKEOVER_RETRY_MS: u64 = 1_000;

/// This process's relationship to the Asset Server its session talks to.
///
/// Only [`ServerRole::Attached`] has a succession problem: the server lives
/// in another process, and when that process dies someone has to become the
/// host or the app is quietly serverless.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ServerRole {
    /// Endpoints pinned by env — someone else's server, never ours to
    /// succeed. Also the state of a store that was never started.
    #[default]
    External,
    /// This process hosts the embedded server (it holds `<root>/server.lock`).
    Host,
    /// Another process hosts it; we joined through the root's `listen` file.
    Attached,
}

/// LAN presence of a server this process starts.
///
/// The embedded server normally announces itself so every peer app (the VJ,
/// the sandbox) finds it the way any LAN peer would. An instance pointed at
/// an isolated root — a test, a second app on a scratch store — must stay
/// off the beacon, or peers hunting for the real store find the scratch one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Beacon {
    #[default]
    Announce,
    Silent,
}

/// Whether a hosting process also runs the background loops that make it a
/// real LAN citizen (the library publisher and the fleet job coordinator).
///
/// Production always runs them. Tests take a root over without them: both
/// loops open their own cache roots and talk to the network, and a unit test
/// must not announce itself to the fleet.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HostLoops {
    #[default]
    Run,
    Skip,
}

/// May this process start a server of its own on the store root?
///
/// The store is inherently multiplayer, and the recommended deployment runs
/// `makepad-asset-server` as its own process so the catalog outlives every
/// window (`apps/asset-server/README.md`). Against that deployment the app
/// must never RACE the daemon for `<root>/server.lock`: whoever launches
/// first would win it, and an app that won it puts the store back inside a
/// window.
///
/// [`EmbedPolicy::Never`] is that promise — this instance is a client, full
/// stop. It still watches the server it joined and still rejoins whoever
/// holds the root next; it simply never becomes that holder. With no server
/// there yet it discovers and retries like any other client instead of
/// filling the vacancy.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EmbedPolicy {
    /// Embed when the root is free, attach when it is not (today's default,
    /// and the transition mode while both deployments exist).
    #[default]
    Auto,
    /// Never hold the root: attach only.
    Never,
}

/// Sustained-loss detector for the server this process talks to.
///
/// One failure is a blip; failures with no answer in between are a death.
/// Only the FIRST failing instant is kept, so the age of an outage is exact,
/// and any answer at all resets it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct LossWatch {
    since_ms: Option<u64>,
}

impl LossWatch {
    fn failing(&mut self, now_ms: u64) {
        self.since_ms.get_or_insert(now_ms);
    }

    fn clear(&mut self) {
        self.since_ms = None;
    }

    /// How long the server has been silent, or `None` while it answers.
    fn down_for(&self, now_ms: u64) -> Option<u64> {
        self.since_ms.map(|since| now_ms.saturating_sub(since))
    }
}

/// Is this refusal the transport failing, or the server answering?
///
/// A 401 or a 404 is an ANSWER: the server is alive and this app is simply
/// wrong about something. Only a socket/deadline failure (or a runtime that
/// is gone) is evidence that there is nothing on the other end.
fn is_connection_loss(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Io { .. } | ClientError::Timeout { .. } | ClientError::RuntimeDown
    )
}

/// The session a succession will open once the previous one has released
/// its single-owner cache roots.
struct PendingSession {
    endpoints: Option<ApiEndpoints>,
    server_id: Option<[u8; 16]>,
    token: Option<String>,
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
    pool: Option<TaskPool>,
    spawner: Option<ThreadSpawner>,
    /// Continuous ai-content-library → catalog publisher. Declared BEFORE
    /// `embedded` so it is joined while the server it publishes into is
    /// still alive.
    publish: Option<PublishLoop>,
    /// LIVECODING: observed origin directories → catalog, no copy. Declared
    /// BEFORE `embedded` for the same reason `publish` is: joined while the
    /// server it publishes into is still alive.
    observe: Option<ObserveLoop>,
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
    /// The in-flight search is a CONTINUATION of the result set already on
    /// screen, not a new one: its page appends instead of replacing, and
    /// the view never blanks while the walk runs.
    search_continuation: bool,
    next_cursor: Option<PageCursor>,
    pub selected: Option<AssetId>,
    pub detail: Remote<AssetDetailDto>,
    detail_req: Option<RequestId>,
    /// Generation capabilities of the LIVE LAN fleet, built by probing the
    /// boxes directly (the store advertises nothing any more — generation
    /// is client-driven, aicore §9).
    pub profiles: Remote<Vec<JobProfileDto>>,
    profiles_task: Option<TaskHandle<Vec<JobProfileDto>>>,
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
    /// Declared runs the server announced as OVER since the app last looked
    /// (`pipeline.finished`). This is the only honest end-of-run signal: a
    /// publish is per-asset and coincidental, and a run that fails publishes
    /// nothing at all. Drained by the RUNS chip, which re-reads on it
    /// instead of waiting out its poll interval.
    finished_pipelines: Vec<PipelineId>,
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

    // --- succession (see the module header) --------------------------------
    /// Host / attached / external. Decides both the status line and whether
    /// this instance is responsible for succeeding a dead server.
    role: ServerRole,
    /// The store root the embedded server uses, captured at `start` so a
    /// takeover can never land on a different one than the attach did.
    server_root: PathBuf,
    /// The library a takeover would start publishing.
    library_dir: PathBuf,
    /// LAN presence for a server started here.
    beacon: Beacon,
    /// Whether this instance may hold the root at all. `Never` makes it a
    /// pure client of the standalone server — see [`EmbedPolicy`].
    embed: EmbedPolicy,
    /// Whether a takeover also starts the publisher/job loops (tests: no).
    host_loops: HostLoops,
    /// The session policy `start` resolved from the environment, WITHOUT
    /// the embed/attach overrides. A succession reuses it (same cache
    /// parent, same discovery timing) instead of re-reading env behind the
    /// operator's back.
    base_config: Option<SessionConfig>,
    /// The control/data pair attach mode joined. After a loss this is the
    /// address that must NOT be trusted again: `listen` is re-read, and only
    /// a DIFFERENT address counts as a new owner to rejoin.
    attached_endpoints: Option<ApiEndpoints>,
    /// How long the server has been failing to answer.
    loss: LossWatch,
    /// One cheap request kept in flight while the server looks silent. A
    /// long-poll that succeeds with nothing to report is silent BY DESIGN,
    /// so only an answer to a question we asked just now can tell a blip
    /// from a death.
    probe_req: Option<RequestId>,
    /// Wall clock of the next takeover attempt while the lock stays held.
    next_takeover_ms: u64,
    /// Honest narration of a succession in progress; takes over the status
    /// line so a serverless instance can never read as connected.
    succession_note: Option<String>,
    /// The previous session is being torn down off-thread. Flips when its
    /// cache roots are free for the next session to open.
    releasing: Option<TaskHandle<()>>,
    /// What to connect to once `releasing` flips.
    pending_session: Option<PendingSession>,
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
    pub fn start(&mut self, library_dir: PathBuf, pool: TaskPool, spawner: ThreadSpawner) {
        if self.connector.is_some() || self.server.is_some() {
            return;
        }
        self.library_dir = library_dir;
        self.pool = Some(pool);
        self.spawner = Some(spawner);
        self.server_root = default_asset_server_root();
        self.beacon = beacon_from_env();
        self.embed = embed_policy_from_env();
        let mut config = session_config_from_env();
        // Kept pristine: a later succession re-opens a session with the same
        // POLICY, and overrides only the endpoints/identity/token of whoever
        // it ends up talking to.
        self.base_config = Some(config.clone());
        if config.endpoints.is_none() {
            if self.embed == EmbedPolicy::Never {
                // Attach-only: this process is a CLIENT of the standalone
                // server and never a candidate to hold the root. A root with
                // nothing on it yet is not an error — the connector
                // discovers and retries, exactly as it would for a server
                // that is still booting.
                self.role = ServerRole::Attached;
                if self.attach_to_root(&mut config) {
                    log!("asset store: attach-only — joined the server holding {}", self.server_root.display());
                } else {
                    log!(
                        "asset store: attach-only — nothing is serving {} yet; discovering",
                        self.server_root.display()
                    );
                }
            } else {
                match start_embedded_asset_server_at(&self.server_root, self.beacon) {
                    Ok((server, token)) => {
                        config.server_id = Some(server.server_id());
                        if config.token.is_none() {
                            config.token = Some(token.clone());
                        }
                        // Only the HOSTING process publishes the library. When
                        // we merely attached to someone else's server, that
                        // process owns its own library and this one must not
                        // push a second copy of the same rows.
                        self.begin_hosting(server, &token);
                    }
                    Err(error) => {
                        // Another process already owns this catalog. Join it
                        // instead of painting a fatal CONFIG ERROR — and from
                        // here on, watch it: an attached instance whose host
                        // dies must succeed it, not carry on serverless.
                        self.role = ServerRole::Attached;
                        if self.attach_to_root(&mut config) {
                            log!(
                                "asset store: {error} — attaching to the running server, \
                                 with succession armed"
                            );
                        } else {
                            self.role = ServerRole::External;
                            self.start_error = Some(error);
                            return;
                        }
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

    /// This process's relationship to the server it talks to.
    pub fn role(&self) -> ServerRole {
        self.role
    }

    /// One status line for the connection chip and the honest empty states.
    pub fn status_label(&self) -> String {
        if let Some(error) = &self.start_error {
            return format!("SERVER · CONFIG ERROR · {error}");
        }
        // A succession in progress is the whole truth for as long as it
        // lasts: an attached instance whose server died must never keep
        // rendering the address of a server that is not there.
        if let Some(note) = &self.succession_note {
            return format!("SERVER · {note}");
        }
        match (&self.server, &self.status) {
            (Some(server), _) => match self.role {
                ServerRole::Host => format!("SERVER · local · {}", server.label),
                ServerRole::Attached => format!("SERVER · attached · {}", server.label),
                ServerRole::External => format!("SERVER · {}", server.label),
            },
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
        let now = now_ms();
        let mut changed = false;
        if let Some(connector) = &mut self.connector {
            for msg in connector.poll() {
                changed = true;
                match msg {
                    SessionMsg::Status(status) => {
                        // A retry is the connector reporting that whatever it
                        // was pointed at did not answer — the loss signal for
                        // an instance that never got a session up at all
                        // (a `listen` file naming a server that is already
                        // dead is exactly this case).
                        match &status {
                            SessionStatus::Retrying { .. } => self.loss.failing(now),
                            SessionStatus::Connected { .. } => self.loss.clear(),
                            _ => {}
                        }
                        self.status = Some(status);
                    }
                    SessionMsg::Up(handles) => {
                        self.loss.clear();
                        self.succession_note = None;
                        self.server = Some(ServerInfo {
                            label: handles.server_label.clone(),
                            server_id: handles.server_id,
                        });
                        self.endpoints = handles.endpoints;
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
        // Fleet-built generation profiles landing from their worker thread.
        if let Some(result) = self.profiles_task.as_mut().and_then(TaskHandle::try_take) {
            self.profiles_task = None;
            match result {
                Ok(profiles) => {
                    self.profiles = Remote::Ready(profiles);
                    changed = true;
                }
                Err(error) => {
                    self.profiles = Remote::Failed(format!("fleet profile job failed: {error}"));
                    changed = true;
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
        // Belt for the walk: a page whose request was cancelled (a filter
        // changed mid-flight) leaves a cursor with nothing in flight, and
        // the result set would sit half-loaded forever.
        if self.search_req.is_none() && self.next_cursor.is_some() {
            let loaded = self.search.ready().map_or(0, |results| results.hits.len());
            if loaded < MAX_CATALOG_ROWS {
                self.submit_next_page();
                changed = true;
            }
        }
        changed |= self.tick_succession(now);
        changed
    }

    // -----------------------------------------------------------------
    // Succession: an attached instance never outlives its server silently.
    // -----------------------------------------------------------------

    /// One tick of the succession machine. Returns true when a render input
    /// changed. `now_ms` is injected so the whole ladder — blip, sustained
    /// loss, takeover, retry — is testable without sleeping.
    fn tick_succession(&mut self, now_ms: u64) -> bool {
        // A swap is already decided; all that remains is waiting for the
        // previous session to release its single-owner cache roots.
        if self.pending_session.is_some() {
            return self.finish_swap();
        }
        if self.role != ServerRole::Attached {
            return false;
        }
        let Some(down_ms) = self.loss.down_for(now_ms) else {
            return self.clear_succession_note();
        };
        // Whatever else happens, find out for certain whether the server is
        // there — the subscriber's failed long-poll opened the question, a
        // request of our own closes it.
        self.submit_probe();
        if down_ms < TAKEOVER_AFTER_MS {
            let label = self.attached_label();
            return self.set_succession_note(format!(
                "attached · {label} not answering ({}s)",
                down_ms / 1000
            ));
        }
        if now_ms < self.next_takeover_ms {
            return false;
        }
        self.next_takeover_ms = now_ms.saturating_add(TAKEOVER_RETRY_MS);
        match takeover_at(&self.server_root, self.attached_endpoints, self.beacon, self.embed) {
            Takeover::Hosted { server, token } => {
                let endpoints = localized_endpoints(&server);
                let server_id = server.server_id();
                log!(
                    "asset store: server lost — took the root over, now hosting {}/{}",
                    endpoints.control,
                    endpoints.data
                );
                self.begin_hosting(server, &token);
                self.pending_session = Some(PendingSession {
                    endpoints: Some(endpoints),
                    server_id: Some(server_id),
                    token: Some(token),
                });
                self.begin_release();
                self.set_succession_note(format!(
                    "took over the store — reconnecting to {}",
                    endpoints.control
                ));
                true
            }
            Takeover::Rejoin(running) => {
                let label = running
                    .endpoints
                    .map(|endpoints| endpoints.control.to_string())
                    .unwrap_or_else(|| "the LAN".to_string());
                log!("asset store: server lost — the root is held by {label}; rejoining");
                self.attached_endpoints = running.endpoints;
                self.pending_session = Some(PendingSession {
                    endpoints: running.endpoints,
                    server_id: running.server_id,
                    token: running.token,
                });
                self.begin_release();
                self.set_succession_note(format!("server lost — rejoining {label}"));
                true
            }
            Takeover::Wait(why) => {
                self.set_succession_note(format!("server lost — taking over… ({why})"))
            }
        }
    }

    /// Point `config` at whatever server holds [`Self::server_root`] right
    /// now, reading the root's own `listen` / `server-id` / `admin-token`.
    ///
    /// Env-pinned values always win — an operator who named an address, an
    /// identity or a token meant it. Returns false when the root advertises
    /// nothing at all, which is the caller's cue: attach-only discovers and
    /// waits, auto-embed reports why it could neither host nor join.
    fn attach_to_root(&mut self, config: &mut SessionConfig) -> bool {
        let Some(existing) = attach_running_asset_server_at(&self.server_root) else {
            return false;
        };
        self.attached_endpoints = existing.endpoints;
        if config.endpoints.is_none() {
            config.endpoints = existing.endpoints;
        }
        if config.server_id.is_none() {
            config.server_id = existing.server_id;
        }
        if config.token.is_none() {
            config.token = existing.token;
        }
        true
    }

    /// Start the embedded server's background loops and become the host.
    fn begin_hosting(&mut self, server: makepad_asset_store::AssetServer, token: &str) {
        if self.host_loops == HostLoops::Run {
            let Some(spawner) = self.spawner.as_ref() else {
                log!("asset store: host workers unavailable (thread runtime not configured)");
                self.role = ServerRole::Host;
                self.embedded = Some(server);
                return;
            };
            self.publish = start_publish_loop(
                spawner,
                &server,
                token,
                self.library_dir.clone(),
            );
            // LIVECODING: observed origin directories, catalogued in place.
            // Only the HOST runs this — reference admission is loopback
            // privilege, so an attached client never observes for somebody
            // else's store.
            self.observe = start_observe_loop(spawner, &server, token);
        }
        self.role = ServerRole::Host;
        self.embedded = Some(server);
    }

    /// Ask the server one cheap question while it looks silent.
    ///
    /// The subscriber only reports FAILED polls: a poll that succeeds with
    /// nothing to report sends no event at all, so a blip that healed would
    /// otherwise look like a death forever. A GC status fetch is a plain
    /// authenticated GET that changes nothing, and its answer — success, or
    /// even a refusal, which is still a server talking — clears the loss.
    fn submit_probe(&mut self) {
        if self.probe_req.is_some() {
            return;
        }
        let Some(handles) = &mut self.handles else { return };
        if let Ok(id) = handles.catalog.submit(ClientRequest::GcStatus) {
            self.probe_req = Some(id);
        }
    }

    /// Hand the previous session to a teardown thread and forget it here.
    ///
    /// A client cache root is single-owner, enforced by an advisory
    /// `cache.lock` that a second handle in THIS process conflicts with too,
    /// so the next session cannot open the same roots until this one's files
    /// are closed. The join happens off the UI thread because a parked
    /// long-poll must never freeze a frame.
    fn begin_release(&mut self) {
        self.connector = None;
        self.server = None;
        self.endpoints = None;
        self.status = None;
        self.events_live = false;
        self.event_note = None;
        self.loss.clear();
        // Every in-flight id belonged to the runtime that is going away, and
        // a cursor is bound to the server that minted it.
        self.search_req = None;
        self.search_continuation = false;
        self.next_cursor = None;
        self.detail_req = None;
        self.profiles_task = None;
        self.probe_req = None;
        self.gc_req = None;
        self.gc_cancel_req = None;
        self.retire_reqs.clear();
        let Some(pool) = &self.pool else {
            log!("asset store: session release refused (runtime pool unavailable)");
            return;
        };
        let slot = match pool.reserve(Lane::Heavy) {
            Ok(slot) => slot,
            Err(error) => {
                log!("asset store: session release delayed ({error})");
                return;
            }
        };
        let Some(handles) = self.handles.take() else {
            self.releasing = None;
            return;
        };
        self.releasing = Some(slot.submit(move || handles.shutdown()));
    }

    /// Open the decided session once the previous one has let go.
    fn finish_swap(&mut self) -> bool {
        if self.releasing.is_none() && self.handles.is_some() {
            self.begin_release();
            return false;
        }
        if let Some(releasing) = &mut self.releasing {
            let Some(result) = releasing.try_take() else { return false };
            if let Err(error) = result {
                log!("asset store: session release failed: {error}");
            }
        }
        self.releasing = None;
        let Some(pending) = self.pending_session.take() else {
            return false;
        };
        let mut config = self
            .base_config
            .clone()
            .unwrap_or_else(|| session_config_from_env());
        if pending.endpoints.is_some() {
            config.endpoints = pending.endpoints;
        }
        if pending.server_id.is_some() {
            config.server_id = pending.server_id;
        }
        if pending.token.is_some() {
            config.token = pending.token;
        }
        match SessionConnector::start(config) {
            Ok(connector) => {
                self.connector = Some(connector);
                self.status = Some(SessionStatus::Discovering);
            }
            // Only a malformed policy gets here, and it will not become
            // well-formed by being retried — say so instead of looping.
            Err(error) => self.start_error = Some(error.to_string()),
        }
        true
    }

    fn attached_label(&self) -> String {
        self.attached_endpoints
            .map(|endpoints| endpoints.control.to_string())
            .unwrap_or_else(|| "the server".to_string())
    }

    fn set_succession_note(&mut self, note: String) -> bool {
        if self.succession_note.as_deref() == Some(note.as_str()) {
            return false;
        }
        self.succession_note = Some(note);
        true
    }

    fn clear_succession_note(&mut self) -> bool {
        // Between sessions the note IS the honest state; only a live session
        // may retire it.
        if self.server.is_none() {
            return false;
        }
        self.succession_note.take().is_some()
    }

    /// The catalog query for the current filters. Facets are counted only
    /// for the FIRST page: they describe the whole result set already, and
    /// re-counting them for every continuation would pay for the same
    /// answer over and over.
    fn catalog_query(&self, first: bool) -> CatalogQuery {
        CatalogQuery {
            facets: if first { SEARCH_FACETS } else { 0 },
            text: self.filters.text.trim().to_string(),
            namespace: None,
            kind: self.filters.kind,
            category: self.filters.category.clone(),
            tag: self.filters.tag.clone(),
            exclude_tag: None,
            creator: None,
            live_only: false,
            // The SAME page size for every page of a walk: a cursor is
            // bound to the exact query shape that minted it, page size
            // included, so a "bigger continuation page" is a refused cursor.
            page_size: SEARCH_PAGE_SIZE,
        }
    }

    /// (Re)run the catalog search for the current filters; empty text is
    /// browse mode. The previous in-flight request is cancelled.
    pub fn submit_search(&mut self) {
        let query = self.catalog_query(true);
        let Some(handles) = &mut self.handles else { return };
        if let Some(previous) = self.search_req.take() {
            handles.catalog.cancel(previous);
        }
        self.next_cursor = None;
        self.search_continuation = false;
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

    /// Ask for the next page of the SAME result set. The rows already on
    /// screen stay exactly where they are; this is what lets the Library
    /// show the whole catalog instead of a first page with a wall at the
    /// bottom of it.
    fn submit_next_page(&mut self) {
        let Some(cursor) = self.next_cursor.take() else {
            return;
        };
        let query = self.catalog_query(false);
        let Some(handles) = &mut self.handles else { return };
        match handles.catalog.submit(ClientRequest::CatalogSearch {
            query,
            cursor: Some(cursor),
        }) {
            Ok(id) => {
                self.search_req = Some(id);
                self.search_continuation = true;
            }
            // A refused continuation leaves the rows that DID arrive alone
            // and simply stops the walk; the count line still says how many
            // of the total are shown.
            Err(error) => {
                log!("asset store: catalog walk stopped: {error}");
                self.search_continuation = false;
            }
        }
    }

    /// Rows held for the current filter, and whether the walk is still
    /// running. Render inputs for the grid's range and its count line.
    pub fn catalog_loaded(&self) -> (usize, bool) {
        let loaded = self.search.ready().map_or(0, |results| results.hits.len());
        (loaded, self.next_cursor.is_some() || self.search_continuation)
    }

    /// Take the assets catalog events touched since the last call.
    /// Runs the server announced as finished since the last call.
    pub fn take_finished_pipelines(&mut self) -> Vec<PipelineId> {
        std::mem::take(&mut self.finished_pipelines)
    }

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

    /// Build the generation-profile list from the LIVE fleet: probe the
    /// boxes the LAN announces and let the shared profile builder say what
    /// they can execute right now. Runs on its own thread — LAN probes must
    /// never stall a frame — and lands through `profiles_rx` in [`Self::poll`].
    fn submit_profiles(&mut self) {
        self.profiles = Remote::Loading;
        let Some(pool) = &self.pool else {
            self.profiles = Remote::Failed("runtime task pool unavailable".into());
            return;
        };
        match pool.submit(Lane::Light, move || {
                let snapshots = makepad_asset_creator::runner::fleet_snapshots();
                makepad_asset_importer::gen_profiles::build_profiles(&snapshots, "gen")
            }) {
            Ok(handle) => self.profiles_task = Some(handle),
            Err(error) => {
                self.profiles = Remote::Failed(format!("fleet profile job refused: {error}"));
            }
        }
    }

    fn on_catalog_event(&mut self, event: ClientEvent) -> bool {
        let id = event.id();
        if Some(id) == self.probe_req {
            match event {
                ClientEvent::Started { .. } | ClientEvent::Progress { .. } => return false,
                ClientEvent::Done { .. } => {
                    self.probe_req = None;
                    self.loss.clear();
                }
                ClientEvent::Failed { error, .. } => {
                    self.probe_req = None;
                    if is_connection_loss(&error) {
                        self.loss.failing(now_ms());
                    } else {
                        // A refusal is a server ANSWERING. Whatever is wrong,
                        // it is not "there is nothing there", and taking the
                        // root over would be plain wrong.
                        log!("asset store: liveness probe refused ({error}) — server is alive");
                        self.loss.clear();
                    }
                }
            }
            return true;
        }
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
                        let more = page.next.is_some();
                        let continuation = std::mem::take(&mut self.search_continuation);
                        match (&mut self.search, continuation) {
                            // A continuation appends: the rows on screen are
                            // the same result set, one page longer.
                            (Remote::Ready(existing), true) => {
                                existing.hits.extend(page.hits);
                                existing.total = page.total;
                                existing.more = more;
                            }
                            _ => {
                                self.search = Remote::Ready(SearchResults {
                                    more,
                                    hits: page.hits,
                                    total: page.total,
                                    facets: page.facets,
                                });
                            }
                        }
                        // Keep walking immediately rather than on the next
                        // UI tick: the whole result set is what the grid's
                        // scrollbar is sized for, and at a fifth of a second
                        // per page a real catalog would take a minute to
                        // arrive.
                        let loaded =
                            self.search.ready().map_or(0, |results| results.hits.len());
                        if self.next_cursor.is_some() && loaded < MAX_CATALOG_ROWS {
                            self.submit_next_page();
                        }
                        // A vanished selection stays selected but its detail
                        // panel reloads honestly on the next click.
                    }
                    (1, ClientOutput::AssetDetail(detail)) => {
                        self.detail_req = None;
                        self.detail = Remote::Ready(detail);
                    }
                    // A mismatched output shape for a tracked id is a
                    // protocol-level surprise — surface it, don't guess.
                    (0, other) => {
                        self.search_req = None;
                        self.search_continuation = false;
                        self.next_cursor = None;
                        self.search = Remote::Failed(format!("unexpected output {other:?}"));
                    }
                    (_, other) => {
                        self.detail_req = None;
                        self.detail = Remote::Failed(format!("unexpected output {other:?}"));
                    }
                }
                true
            }
            ClientEvent::Failed { error, .. } => {
                match slot {
                    0 => {
                        self.search_req = None;
                        // Whatever went wrong, the walk is over: leaving the
                        // continuation flag set would tell the Library it is
                        // still loading, forever.
                        self.search_continuation = false;
                        self.next_cursor = None;
                        self.search = Remote::Failed(error.to_string());
                    }
                    _ => {
                        self.detail_req = None;
                        self.detail = Remote::Failed(error.to_string());
                    }
                }
                true
            }
        }
    }

    fn on_feed_event(&mut self, event: CatalogSubscriptionEvent) {
        // Every variant except `Retry` is the server having answered.
        match &event {
            CatalogSubscriptionEvent::Retry { error, .. } if is_connection_loss(error) => {
                self.loss.failing(now_ms())
            }
            _ => self.loss.clear(),
        }
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
            if let Some(pipeline) = event.pipeline {
                if !self.finished_pipelines.contains(&pipeline) {
                    self.finished_pipelines.push(pipeline);
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

/// Milliseconds since the epoch. The succession ladder is measured in wall
/// clock so a status line can say how long the server has been gone.
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

pub(crate) fn checkout_root() -> PathBuf {
    if let Ok(root) = std::env::var("MAKEPAD_ROOT") {
        return PathBuf::from(root);
    }
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Checkout-local Asset UI home (`local/asset-ui`). Never `$HOME`.
pub fn asset_ui_home() -> PathBuf {
    checkout_root().join("local/asset-ui")
}

/// Where THIS instance's client-side blob caches live: the
/// `ASSET_UI_ASSET_CACHE` parent when set, else the app home. Every cache an
/// instance opens must be under this root — a cache at a fixed repo path is
/// owned by whichever process got there first, and a second instance (an
/// isolated-store test run beside the operator's live one) then fails every
/// store fetch with "cache root is owned by another process".
pub fn instance_cache_parent() -> PathBuf {
    env_alias(&["ASSET_UI_ASSET_CACHE", "AI_CONTENT_ASSET_CACHE"])
        .map(PathBuf::from)
        .unwrap_or_else(asset_ui_home)
}

pub(crate) fn default_asset_server_root() -> PathBuf {
    if let Ok(root) = std::env::var("AI_CONTENT_ASSET_ROOT") {
        return PathBuf::from(root);
    }
    asset_ui_home().join("asset-server")
}

/// `ASSET_UI_ASSET_BEACON=off` (`0`/`no`/`silent`) starts a server without
/// its LAN beacon — see the module header.
fn beacon_from_env() -> Beacon {
    match env_alias(&["ASSET_UI_ASSET_BEACON", "AI_CONTENT_ASSET_BEACON"]) {
        Some(value)
            if matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "off" | "0" | "no" | "silent" | "false"
            ) =>
        {
            Beacon::Silent
        }
        _ => Beacon::Announce,
    }
}

/// `ASSET_UI_ASSET_EMBED=never` (`no`/`off`/`0`/`attach`/`client`) makes this
/// instance a pure client of whatever server holds the store root — it never
/// takes `<root>/server.lock`, at startup or in a succession. That is the
/// recommended setting beside a standalone `makepad-asset-server`; see
/// `apps/asset-server/README.md`.
///
/// Anything else (including unset) keeps today's behaviour: embed when the
/// root is free, attach when it is not.
pub fn embed_policy_from_env() -> EmbedPolicy {
    embed_policy_from_value(env_alias(&["ASSET_UI_ASSET_EMBED", "AI_CONTENT_ASSET_EMBED"]).as_deref())
}

/// The parse on its own, so the vocabulary is testable without mutating
/// process environment other tests in this binary also read.
fn embed_policy_from_value(value: Option<&str>) -> EmbedPolicy {
    match value {
        Some(value)
            if matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "never" | "no" | "off" | "0" | "false" | "attach" | "client"
            ) =>
        {
            EmbedPolicy::Never
        }
        _ => EmbedPolicy::Auto,
    }
}

/// A locally hosted server binds `0.0.0.0`; reach it the way its own
/// `listen` file advertises it.
fn localized_endpoints(server: &makepad_asset_store::AssetServer) -> ApiEndpoints {
    let localize = |addr: SocketAddr| {
        if addr.ip().is_unspecified() {
            SocketAddr::new(IpAddr::V4(std::net::Ipv4Addr::LOCALHOST), addr.port())
        } else {
            addr
        }
    };
    ApiEndpoints {
        control: localize(server.control_addr()),
        data: localize(server.data_addr()),
    }
}

/// Outcome of one attempt to succeed a server that stopped answering.
enum Takeover {
    /// The root lock was free: this process now hosts the store.
    Hosted {
        server: makepad_asset_store::AssetServer,
        token: String,
    },
    /// The lock is still held and the root now advertises a DIFFERENT
    /// server than the silent one — a rival attacher won the race, or the
    /// owner was only briefly unreachable and rebound. Join what the root
    /// names now.
    Rejoin(RunningServer),
    /// The lock is held and the root still advertises the server that went
    /// silent: there is nothing new to trust yet.
    Wait(String),
}

/// One attempt at succession, arbitrated entirely by `<root>/server.lock`.
///
/// `silent` is the address that stopped answering. After a loss the old
/// `listen` file must never be trusted again — it is re-read on every
/// attempt, and only an address that CHANGED counts as a new owner. That is
/// what keeps two attached instances racing for the same dead root from
/// both rejoining a corpse: the loser reads the winner's freshly written
/// file, not the stale one it already gave up on.
///
/// Under [`EmbedPolicy::Never`] the lock is never even attempted: the whole
/// ladder collapses to "re-read the root and rejoin whoever holds it next",
/// which is what an app deployed beside `makepad-asset-server` must do while
/// the daemon restarts.
fn takeover_at(
    root: &Path,
    silent: Option<ApiEndpoints>,
    beacon: Beacon,
    embed: EmbedPolicy,
) -> Takeover {
    let refused = match embed {
        EmbedPolicy::Never => "attach-only: waiting for a server to hold the root".to_string(),
        EmbedPolicy::Auto => match start_embedded_asset_server_at(root, beacon) {
            Ok((server, token)) => return Takeover::Hosted { server, token },
            Err(error) => error,
        },
    };
    let Some(running) = attach_running_asset_server_at(root) else {
        return Takeover::Wait(refused);
    };
    match (running.endpoints, silent) {
        (Some(fresh), Some(dead)) if fresh.control == dead.control => {
            Takeover::Wait("the root still names the silent server".to_string())
        }
        (None, Some(_)) => Takeover::Wait("the root names no server yet".to_string()),
        _ => Takeover::Rejoin(running),
    }
}

/// When the catalog root is already locked, read the live server's listen
/// address / id / admin token so the UI can connect as a client.
fn attach_running_asset_server_at(root: &Path) -> Option<RunningServer> {
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

fn start_embedded_asset_server_at(
    root: &Path,
    beacon: Beacon,
) -> Result<(makepad_asset_store::AssetServer, String), String> {
    let mut cfg = makepad_asset_store::ServerConfig::new(root.to_path_buf());
    cfg.control_addr = "0.0.0.0:0"
        .parse()
        .map_err(|e| format!("control bind spec: {e}"))?;
    cfg.data_addr = "0.0.0.0:0"
        .parse()
        .map_err(|e| format!("data bind spec: {e}"))?;
    cfg.bootstrap_admin = true;
    cfg.discovery = match beacon {
        Beacon::Announce => Some(makepad_asset_store::DiscoveryConfig::lan_default()),
        Beacon::Silent => None,
    };
    // Reference imports ON, LOOPBACK ONLY. The planes above bind 0.0.0.0 so
    // the LAN can browse this library, but admitting content BY PATH stays a
    // same-process privilege: the policy refuses any non-loopback caller, and
    // the only loopback caller that uses it is this app's own observer (see
    // `start_observe_loop`) cataloguing directories the user pointed it at.
    cfg.blob_refs = makepad_asset_store::BlobRefPolicy::local_host();
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

/// Owns the publisher thread; dropping the store stops and joins it.
struct PublishLoop {
    stop: Arc<AtomicBool>,
    task: Option<TaskHandle<()>>,
}

impl Drop for PublishLoop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(task) = self.task.take() {
            task.detach();
        }
    }
}

/// Publish everything the pipelines write into the AI-content library into
/// the server this process hosts. Connect + poll happen on the thread, so a
/// slow or refused connection can never stall the UI, and every failure is
/// a log line — never a panic.
fn start_publish_loop(
    spawner: &ThreadSpawner,
    server: &makepad_asset_store::AssetServer,
    token: &str,
    library_dir: PathBuf,
) -> Option<PublishLoop> {
    let endpoints = localized_endpoints(server);
    let server_id = server.server_id();
    let token = token.to_string();
    let cache = asset_ui_home().join("publish-cache");
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let task = spawner.spawn_worker(
        ThreadOptions {
            name: Some("asset-ui-publish".into()),
            ..Default::default()
        },
        move || {
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
                &worker_stop,
            );
            log!("publish loop: stopped");
        },
    );
    match task {
        Ok(task) => Some(PublishLoop { stop, task: Some(task) }),
        Err(error) => {
            log!("publish loop: could not spawn: {error}");
            None
        }
    }
}

/// Owns the observer thread; dropping the store stops and joins it.
struct ObserveLoop {
    stop: Arc<AtomicBool>,
    task: Option<TaskHandle<()>>,
}

impl Drop for ObserveLoop {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(task) = self.task.take() {
            task.detach();
        }
    }
}

/// Which directories this app observes. `ASSET_UI_FX_ORIGIN` names one
/// explicitly; otherwise it is the checkout's `local/vjfx`, the same origin
/// the VJ uses, so a livecoding session works whichever of the two is
/// hosting the store that night.
fn observe_origins() -> Vec<PathBuf> {
    if let Some(dir) = env_alias(&["ASSET_UI_FX_ORIGIN", "VJ_FX_ORIGIN"]) {
        let dir = dir.trim();
        if !dir.is_empty() {
            return vec![PathBuf::from(dir)];
        }
    }
    let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../local/vjfx");
    let vj_presets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vj/resources/effects");
    let mut out = vec![checkout];
    if vj_presets.is_dir() {
        out.push(vj_presets);
    }
    out
}

/// Observe effect-document origins into the server this process hosts.
/// Connect + watch happen on the thread; every failure is a log line.
fn start_observe_loop(
    spawner: &ThreadSpawner,
    server: &makepad_asset_store::AssetServer,
    token: &str,
) -> Option<ObserveLoop> {
    let endpoints = localized_endpoints(server);
    let server_id = server.server_id();
    let token = token.to_string();
    let cache = asset_ui_home().join("observe-cache");
    let origins = observe_origins();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = stop.clone();
    let task = spawner.spawn_worker(
        ThreadOptions {
            name: Some("asset-ui-observe".into()),
            ..Default::default()
        },
        move || {
            let mut config = makepad_asset_client::ClientConfig::new(cache);
            config.token = Some(token);
            let mut client = match makepad_asset_client::AssetClient::connect(
                config,
                endpoints,
                Some(server_id),
            ) {
                Ok(client) => client,
                Err(error) => {
                    log!("observe loop: cannot connect to the embedded server: {error}");
                    return;
                }
            };
            makepad_asset_store::observe::run(
                &mut client,
                &makepad_asset_store::observe::ObserveConfig::vjfx(origins),
                &worker_stop,
            );
            log!("observe loop: stopped");
        },
    );
    match task {
        Ok(task) => Some(ObserveLoop { stop, task: Some(task) }),
        Err(error) => {
            log!("observe loop: could not spawn: {error}");
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
            // The bootstrap token of THIS process's store root, which
            // `AI_CONTENT_ASSET_ROOT` may have moved. Reading the default
            // location unconditionally handed an isolated instance the
            // shared server's credentials — every request it then made to
            // its own server was refused (the same trap `import.rs`'s
            // `hosted_server_session` already had to climb out of).
            std::fs::read_to_string(default_asset_server_root().join("admin-token"))
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
    /// Picked row of the Kind picker — a content-contract kind, or audio
    /// narrowed to music/sfx.
    pub kind: KindChoice,
    /// Picked tag. ONE vocabulary as far as this app is concerned: the store
    /// keeps two label kinds on the wire and the UI never mentions that
    /// again, so this is a NAME. Which wire kind carries it is worked out
    /// when the query is built, from the counted facets.
    pub label: Option<String>,
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
            model_preview: None,
            pipeline: None,
            pipeline_state: None,
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
            creator: String::new(),
            artist: String::new(),
            artist_url: String::new(),
            album: String::new(),
            source_url: String::new(),
            license: String::new(),
            license_url: String::new(),
            snippet: String::new(),
            score: 0,
            live: true,
            alias: None,
            updated_ms: 1_787_000_000_000,
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
            model_preview: None,
            pipeline: None,
            pipeline_state: None,
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
            model_preview: None,
            pipeline: None,
            pipeline_state: None,
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

    fn ensure_test_runtime(store: &mut AssetStore) {
        if store.pool.is_none() {
            let cx = makepad_widgets::Cx::new(Box::new(|_, _| {}));
            store.pool = Some(cx.task_pool());
            store.spawner = Some(cx.thread_spawner());
        }
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
                views: Vec::new(),
            },
        );
        request.alias = Some(makepad_asset_data::AssetAlias::from_str(alias).unwrap());
        // One label, so tests can see the facet the catalog counts.
        request.categories = vec!["test".to_string()];
        client.publish_artifact(&request).expect("publish").asset_id
    }

    /// Connect `store` to `server` through the REAL `SessionConnector` +
    /// `AssetStore::poll()` loop (no discovery — explicit endpoints), and
    /// wait for the initial auto-search `poll()` fires on connect to land.
    fn connect_store_to(store: &mut AssetStore, server: &makepad_asset_store::AssetServer, token: &str) {
        ensure_test_runtime(store);
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
        poll_until_within(store, what, 15, ready)
    }

    /// Same, with the patience spelled out: a test that publishes a hundred
    /// assets and walks them back is doing real work, and it shares the
    /// machine with the rest of the suite.
    fn poll_until_within<F: Fn(&AssetStore) -> bool>(
        store: &mut AssetStore,
        what: &str,
        secs: u64,
        ready: F,
    ) {
        ensure_test_runtime(store);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
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

    /// The Library shows the WHOLE result set, not its first page. The walk
    /// pages through the catalog on its own until every match is held (or
    /// the app's cap is reached), so the grid's scrollbar covers everything
    /// and scrolling never hits a "load more" wall. It must also terminate:
    /// one page in flight at a time, and the cursor runs out.
    #[test]
    fn the_catalog_walk_covers_the_whole_result_set_and_stops() {
        let (server, token) = start_isolated_server("walk_ui");
        // More assets than one page holds, so the walk has to run.
        let published = SEARCH_PAGE_SIZE as usize + 7;
        for i in 0..published {
            publish_test_asset(&server, &token, &format!("gen/walk-{i:03}"), i as u8);
        }

        let mut store = AssetStore::default();
        connect_store_to(&mut store, &server, &token);
        // The first page landed during connect; the rest walk in.
        poll_until_within(&mut store, "the walk to finish", 90, |store| {
            let (loaded, walking) = store.catalog_loaded();
            !walking && loaded > 0
        });

        let results = store.search.ready().expect("a result set");
        assert_eq!(
            results.total as usize, published,
            "the server's own count of the matches"
        );
        assert_eq!(
            results.hits.len(),
            published,
            "every match is held, not just the first page"
        );
        assert!(!results.more, "no page is left behind");
        let unique: std::collections::HashSet<AssetId> =
            results.hits.iter().map(|hit| hit.asset_id).collect();
        assert_eq!(unique.len(), published, "the walk never repeats a row");
        let facet = results
            .facets
            .iter()
            .find(|facet| facet.label == "test")
            .expect("facets counted once, on the first page, and kept through the walk");
        assert_eq!(
            facet.count as usize, published,
            "a facet counts the whole result set, not the page it rode in on"
        );

        // Settled: polling again neither grows the set nor starts a request.
        for _ in 0..5 {
            store.poll();
        }
        assert_eq!(store.search.ready().unwrap().hits.len(), published);
        assert_eq!(store.catalog_loaded(), (published, false));

        // A new filter starts a new result set from the first page rather
        // than appending to the old one.
        store.filters.text = "nothing-matches-this".into();
        store.submit_search();
        poll_until(&mut store, "the narrowed search", |store| {
            matches!(&store.search, Remote::Ready(r) if r.total == 0)
        });
        assert!(store.search.ready().unwrap().hits.is_empty(), "no stale rows survive");
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

    // -----------------------------------------------------------------
    // Succession. An attached instance whose host dies must become the
    // host (or join whoever did) — never carry on pointing at a dead port
    // while the window looks healthy. Every server here is `Beacon::Silent`
    // on its own temp root: a test must not advertise itself to the LAN.
    // -----------------------------------------------------------------

    #[test]
    fn loss_watch_keeps_the_first_failure_and_any_answer_clears_it() {
        let mut loss = LossWatch::default();
        assert_eq!(loss.down_for(1_000), None, "silence is not loss");
        loss.failing(1_000);
        loss.failing(1_400);
        assert_eq!(
            loss.down_for(2_000),
            Some(1_000),
            "a later failure must not restart the outage clock"
        );
        loss.clear();
        assert_eq!(loss.down_for(2_000), None);

        // Only the transport failing is loss; a refusal is a server TALKING.
        assert!(is_connection_loss(&ClientError::Io {
            op: "connect",
            kind: std::io::ErrorKind::ConnectionRefused,
        }));
        assert!(is_connection_loss(&ClientError::Timeout { op: "read head" }));
        assert!(is_connection_loss(&ClientError::RuntimeDown));
        assert!(!is_connection_loss(&ClientError::Unauthenticated));
        assert!(!is_connection_loss(&ClientError::Denied));
        assert!(!is_connection_loss(&ClientError::NotFound { what: "asset" }));
        assert!(!is_connection_loss(&ClientError::Server { status: 503, detail: None }));
    }

    #[test]
    fn the_status_line_names_the_role_and_a_succession_takes_it_over() {
        let mut store = AssetStore::default();
        store.server = Some(ServerInfo {
            label: "127.0.0.1:9701".into(),
            server_id: [3; 16],
        });
        assert_eq!(
            store.status_label(),
            "SERVER · 127.0.0.1:9701",
            "an env-pinned server is nobody's to succeed"
        );
        store.role = ServerRole::Host;
        assert_eq!(store.status_label(), "SERVER · local · 127.0.0.1:9701");
        store.role = ServerRole::Attached;
        assert_eq!(store.status_label(), "SERVER · attached · 127.0.0.1:9701");

        // A note outranks the last known session, and survives losing it —
        // there is no arrangement of these fields that reads as connected
        // while the server is gone.
        store.succession_note = Some("server lost — taking over…".to_string());
        assert_eq!(store.status_label(), "SERVER · server lost — taking over…");
        store.server = None;
        store.status = None;
        assert_eq!(store.status_label(), "SERVER · server lost — taking over…");
        // …and only a live session may retire it.
        assert!(!store.clear_succession_note());
        assert!(store.succession_note.is_some());
        store.server = Some(ServerInfo { label: "127.0.0.1:9911".into(), server_id: [4; 16] });
        assert!(store.clear_succession_note());
        assert_eq!(store.status_label(), "SERVER · attached · 127.0.0.1:9911");
    }

    #[test]
    fn takeover_waits_for_a_live_owner_hosts_a_dead_one_and_the_loser_rejoins_the_winner() {
        let root = live_test_root("succession-lock");
        let (owner, owner_token) =
            start_embedded_asset_server_at(&root, Beacon::Silent).expect("owner server");
        let published = attach_running_asset_server_at(&root).expect("root files");
        let owned = published.endpoints.expect("listen file");
        assert_eq!(owned.control.port(), owner.control_addr().port());
        assert_eq!(published.token.as_deref(), Some(owner_token.as_str()));

        // A live owner holds the lock, and the root still names it: nothing
        // to succeed, nothing new to trust.
        match takeover_at(&root, Some(owned), Beacon::Silent, EmbedPolicy::Auto) {
            Takeover::Wait(why) => assert!(why.contains("silent server"), "{why}"),
            Takeover::Hosted { .. } => panic!("a live owner must never be succeeded"),
            Takeover::Rejoin(_) => panic!("the root named the same server it always did"),
        }

        drop(owner);
        let Takeover::Hosted { server, token } =
            takeover_at(&root, Some(owned), Beacon::Silent, EmbedPolicy::Auto)
        else {
            panic!("a free root lock must be taken");
        };
        let fresh = localized_endpoints(&server);
        assert_ne!(
            fresh.control.port(),
            owned.control.port(),
            "a new server binds new ephemeral ports"
        );
        // The successor rewrote `listen`, so anyone re-reading the root now
        // finds IT — this is the only way a loser can be told where to go.
        let after = attach_running_asset_server_at(&root).expect("root files after takeover");
        assert_eq!(after.endpoints.expect("listen").control.port(), fresh.control.port());
        assert_eq!(after.server_id, Some(server.server_id()));

        // And it really serves.
        let mut cfg = makepad_asset_client::ClientConfig::new(live_test_root("succession-client"));
        cfg.token = Some(token.clone());
        let client =
            makepad_asset_client::AssetClient::connect(cfg, fresh, Some(server.server_id()))
                .expect("the successor answers a verified connect");
        client
            .catalog_search(&CatalogQuery::browse(10), None)
            .expect("the successor serves the catalog");

        // The race: a second attacher that also gave up on the dead address
        // must land on the WINNER, never back on the stale file it already
        // abandoned.
        match takeover_at(&root, Some(owned), Beacon::Silent, EmbedPolicy::Auto) {
            Takeover::Rejoin(running) => {
                let rejoin = running.endpoints.expect("the winner's listen file");
                assert_eq!(rejoin.control.port(), fresh.control.port());
                assert_ne!(rejoin.control.port(), owned.control.port());
                assert_eq!(
                    running.token.as_deref(),
                    Some(token.as_str()),
                    "the loser must re-read the winner's credentials too"
                );
            }
            Takeover::Hosted { .. } => panic!("the winner still holds the root lock"),
            Takeover::Wait(why) => panic!("the root names the winner now: {why}"),
        }

        drop(client);
        drop(server);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_attached_store_takes_over_a_dead_server_and_reconnects_to_itself() {
        let root = live_test_root("succession-store");
        let (owner, owner_token) =
            start_embedded_asset_server_at(&root, Beacon::Silent).expect("owner server");
        let owner_endpoints = localized_endpoints(&owner);
        let attached = attach_running_asset_server_at(&root).expect("root files");

        // Exactly what `start` builds when the lock is already held.
        let mut store = AssetStore::default();
        store.role = ServerRole::Attached;
        store.host_loops = HostLoops::Skip;
        store.beacon = Beacon::Silent;
        store.server_root = root.clone();
        store.library_dir = live_test_root("succession-library");
        store.attached_endpoints = attached.endpoints;
        // ONE cache parent for both sessions, so the swap really has to
        // release the single-owner cache roots before it can re-open them.
        let cache = live_test_root("succession-store-cache");
        store.base_config = Some(SessionConfig::new(cache.clone()));
        let config = SessionConfig {
            endpoints: attached.endpoints,
            server_id: attached.server_id,
            token: attached.token.clone(),
            ..SessionConfig::new(cache)
        };
        store.connector = Some(SessionConnector::start(config).expect("attach connector"));
        poll_until(&mut store, "the attached session", |store| {
            matches!(store.search, Remote::Ready(_))
        });
        assert_eq!(
            store.status_label(),
            format!("SERVER · attached · {}", owner_endpoints.control),
            "attach mode says so"
        );

        // Healthy: succession does nothing at all.
        assert!(!store.tick_succession(1_000));
        assert!(store.succession_note.is_none());
        assert!(store.embedded.is_none());

        // Sustained loss while the owner is ALIVE (unreachable, not dead):
        // the lock arbitrates and this instance keeps trying, honestly.
        store.loss.failing(10_000);
        assert!(store.tick_succession(10_000 + TAKEOVER_AFTER_MS));
        assert!(store.status_label().contains("taking over"), "{}", store.status_label());
        assert!(store.embedded.is_none(), "a live owner keeps the root");
        assert_eq!(store.role, ServerRole::Attached);
        store.loss.clear();
        store.succession_note = None;

        // Now it really dies.
        drop(owner);
        store.loss.failing(20_000);
        assert!(store.tick_succession(21_600));
        assert_eq!(
            store.status_label(),
            format!("SERVER · attached · {} not answering (1s)", owner_endpoints.control),
            "a blip is reported, not acted on"
        );
        assert!(store.embedded.is_none());

        // Sustained: take the root over.
        assert!(store.tick_succession(20_000 + TAKEOVER_AFTER_MS));
        assert_eq!(store.role, ServerRole::Host);
        let hosted = localized_endpoints(store.embedded.as_ref().expect("successor server"));
        assert_ne!(hosted.control.port(), owner_endpoints.control.port());
        assert!(store.status_label().contains("took over"), "{}", store.status_label());
        assert!(!store.connected(), "there is no session until it reconnects");
        assert!(store.handles.is_none(), "the dead session was handed over for teardown");

        // …and it reconnects to ITSELF, through the real connector, on the
        // same cache roots the dead session had to let go of first.
        poll_until(&mut store, "the successor's own session", |store| store.connected());
        assert_eq!(store.status_label(), format!("SERVER · local · {}", hosted.control));
        assert!(store.succession_note.is_none(), "a live session retires the note");
        poll_until(&mut store, "the successor serving the catalog", |store| {
            matches!(store.search, Remote::Ready(_))
        });

        drop(store);
        drop(owner_token);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The recommended deployment: `makepad-asset-server` owns the root and
    /// the app is a client. Even with the lock FREE — the daemon restarting,
    /// or not up yet — an attach-only instance must never fill the vacancy,
    /// because the daemon coming back would then find its own root taken by
    /// a window.
    #[test]
    fn attach_only_never_takes_a_root_even_when_the_lock_is_free() {
        let root = live_test_root("attach-only-free");
        std::fs::create_dir_all(&root).expect("root");

        // Nothing has ever served this root: no listen file, no lock holder.
        match takeover_at(&root, None, Beacon::Silent, EmbedPolicy::Never) {
            Takeover::Wait(why) => assert!(why.contains("attach-only"), "{why}"),
            Takeover::Hosted { .. } => panic!("attach-only must never hold the root"),
            Takeover::Rejoin(_) => panic!("there is nothing on this root to rejoin"),
        }
        assert!(
            !root.join("server.lock").exists() || {
                // A refused attempt must not have left a LOCKED file behind:
                // the daemon has to be able to take this root immediately.
                let (server, _) =
                    start_embedded_asset_server_at(&root, Beacon::Silent).expect("root is free");
                drop(server);
                true
            },
            "attach-only left the root unusable"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// …and once a server IS there, attach-only joins it and, when it goes
    /// silent and a NEW one takes the root, rejoins that one instead of
    /// hosting.
    #[test]
    fn attach_only_rejoins_the_next_holder_of_the_root() {
        let root = live_test_root("attach-only-rejoin");
        let (first, _first_token) =
            start_embedded_asset_server_at(&root, Beacon::Silent).expect("first server");
        let joined = localized_endpoints(&first);

        // A `start` under EmbedPolicy::Never learns the address from the
        // root's own files, exactly like the auto path's attach branch.
        let mut store = AssetStore::default();
        store.server_root = root.clone();
        store.embed = EmbedPolicy::Never;
        let mut config = SessionConfig::new(live_test_root("attach-only-cache"));
        assert!(store.attach_to_root(&mut config), "the root advertises a server");
        assert_eq!(
            config.endpoints.expect("listen").control.port(),
            joined.control.port()
        );
        assert_eq!(config.server_id, Some(first.server_id()));
        assert!(config.token.is_some(), "the root's admin token is readable");

        // The one it joined goes silent while the lock stays held: wait.
        match takeover_at(&root, store.attached_endpoints, Beacon::Silent, EmbedPolicy::Never) {
            Takeover::Wait(why) => assert!(why.contains("silent server"), "{why}"),
            other => panic!("a live holder must be waited on, got {}", takeover_name(&other)),
        }

        // It dies and a DIFFERENT process takes the root (the daemon
        // restarting). Attach-only rejoins the newcomer.
        drop(first);
        let (second, second_token) =
            start_embedded_asset_server_at(&root, Beacon::Silent).expect("second server");
        let successor = localized_endpoints(&second);
        match takeover_at(&root, Some(joined), Beacon::Silent, EmbedPolicy::Never) {
            Takeover::Rejoin(running) => {
                assert_eq!(
                    running.endpoints.expect("listen").control.port(),
                    successor.control.port()
                );
                assert_eq!(running.token.as_deref(), Some(second_token.as_str()));
            }
            other => panic!("expected a rejoin, got {}", takeover_name(&other)),
        }

        drop(second);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn takeover_name(outcome: &Takeover) -> &'static str {
        match outcome {
            Takeover::Hosted { .. } => "Hosted",
            Takeover::Rejoin(_) => "Rejoin",
            Takeover::Wait(_) => "Wait",
        }
    }

    #[test]
    fn the_embed_policy_is_opt_in_and_only_the_named_words_turn_it_off() {
        // Parsed from the same vocabulary the beacon switch uses, so an
        // operator who knows one knows the other.
        for value in ["never", "no", "off", "0", "false", "attach", "client", " NEVER "] {
            assert_eq!(
                embed_policy_from_value(Some(value)),
                EmbedPolicy::Never,
                "{value:?}"
            );
        }
        for value in ["auto", "yes", "1", "host", ""] {
            assert_eq!(
                embed_policy_from_value(Some(value)),
                EmbedPolicy::Auto,
                "{value:?}"
            );
        }
        assert_eq!(
            embed_policy_from_value(None),
            EmbedPolicy::Auto,
            "unset keeps today's embedding behaviour"
        );
    }
}
