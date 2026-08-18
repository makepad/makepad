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
//! Env/token conventions (`ASSET_UI_*`, with `AI_CONTENT_*` still accepted):
//! - `ASSET_UI_ASSET_SERVER=ip:controlport:dataport` — explicit endpoints;
//!   unset = LAN discovery on the standard beacon port.
//! - `ASSET_UI_ASSET_SERVER_ID=<32 hex>` — pin the server identity.
//! - Token: `ASSET_UI_ASSET_TOKEN`, then `ASSET_UI_ASSET_TOKEN_FILE`, then
//!   `~/.makepad-asset-ai/asset-server/admin-token` (the running server's
//!   bootstrap token), then `~/.makepad-asset-ai/asset-server.token`.
//!   No token = anonymous probe.
//! - `ASSET_UI_ASSET_CACHE=<dir>` — cache parent, default
//!   `~/.makepad-asset-ai`.

use makepad_asset_client::{
    ApiEndpoints, AssetDetailDto, CatalogEventDto, CatalogHit, CatalogQuery,
    CatalogSubscriptionEvent, ClientEvent, ClientOutput, ClientRequest, JobProfileDto, PageCursor,
    RequestId, SessionConfig, SessionConnector, SessionHandles, SessionMsg, SessionStatus,
};
use makepad_asset_data::AssetId;
pub use makepad_asset_data::AssetKind;
use std::collections::VecDeque;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

/// Committed catalog events retained for the Admin surface (newest first).
pub const EVENT_LOG_CAP: usize = 200;
/// One search page. The server caps at MAX_SEARCH_LIMIT (100); more rows
/// exist server-side when `SearchResults::more` is set.
pub const SEARCH_PAGE_SIZE: u32 = 60;

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
}

/// Session-backed store state. All `pub` fields are render inputs; mutation
/// happens through `start`/`poll`/`submit_search`/`select` only.
#[derive(Default)]
pub struct AssetStore {
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
    pub fn start(&mut self) {
        if self.connector.is_some() || self.server.is_some() {
            return;
        }
        let mut config = session_config_from_env();
        if config.endpoints.is_none() {
            match start_embedded_asset_server() {
                Ok((server, token)) => {
                    config.server_id = Some(server.server_id());
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
            text: self.filters.text.trim().to_string(),
            namespace: None,
            kind: self.filters.kind,
            category: self.filters.category.clone(),
            tag: self.filters.tag.clone(),
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
        for event in events {
            self.events.push_front(event);
        }
        self.events.truncate(EVENT_LOG_CAP);
    }
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

fn default_asset_server_root() -> PathBuf {
    if let Ok(root) = std::env::var("AI_CONTENT_ASSET_ROOT") {
        return PathBuf::from(root);
    }
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    home.join(".makepad-asset-ai").join("asset-server")
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

pub fn session_config_from_env() -> SessionConfig {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    let app_home = home.join(".makepad-asset-ai");
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
// Local History filters (disk-backed generator library — unrelated to the
// server catalog and deliberately kept separate).
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LocalLibraryFilters {
    pub query: String,
    pub category: Option<String>,
    pub kind: Option<String>,
    /// Selected Library tags (AND). Each name is an import tag or a later
    /// vision `enhanced_tags` entry from the index.
    pub tags: Vec<String>,
}

impl LocalLibraryFilters {
    pub fn matches(
        &self,
        label: &str,
        prompt: &str,
        domain: &str,
        content_type: &str,
        item_tags: &[String],
    ) -> bool {
        let query = self.query.trim().to_lowercase();
        let category_matches = self.category.as_ref().is_none_or(|category| {
            library_type(domain, content_type).eq_ignore_ascii_case(category)
                || (*category == "maps"
                    && (domain.eq_ignore_ascii_case("map")
                        || domain.eq_ignore_ascii_case("world")
                        || domain.eq_ignore_ascii_case("maps")))
                || (*category == "music"
                    && (domain.eq_ignore_ascii_case("audio")
                        || domain.eq_ignore_ascii_case("sfx"))
                    && (label.to_ascii_lowercase().contains("music")
                        || label.to_ascii_lowercase().contains("jingle")
                        || prompt.to_ascii_lowercase().contains("music")
                        || prompt.to_ascii_lowercase().contains("jingle")))
        });
        let kind_matches = true;
        let query_matches = query.is_empty()
            || label.to_lowercase().contains(&query)
            || prompt.to_lowercase().contains(&query)
            || domain.to_lowercase().contains(&query)
            || content_type.to_lowercase().contains(&query);
        let tag_matches = self.tags.iter().all(|want| {
            item_tags
                .iter()
                .any(|have| have.eq_ignore_ascii_case(want))
        });
        category_matches && kind_matches && query_matches && tag_matches
    }
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

    #[test]
    fn local_library_filters_do_not_claim_server_metadata() {
        let filters = LocalLibraryFilters {
            query: "trawler".into(),
            category: Some("meshes".into()),
            kind: None,
            tags: vec!["weathered".into()],
        };
        assert!(filters.matches(
            "Trawler GLB",
            "a weathered fishing trawler",
            "mesh",
            "model/gltf-binary",
            &["weathered".into()],
        ));
        assert!(!filters.matches(
            "Trawler PNG",
            "a clean fishing trawler",
            "image",
            "image/png",
            &["weathered".into()],
        ));
        assert!(!filters.matches(
            "Trawler GLB",
            "a weathered fishing trawler",
            "mesh",
            "model/gltf-binary",
            &["freedoom".into()],
        ));
        let untagged = LocalLibraryFilters {
            query: String::new(),
            category: None,
            kind: None,
            tags: Vec::new(),
        };
        assert!(untagged.matches("x", "", "mesh", "model/gltf-binary", &[]));
    }

    #[test]
    fn maps_filter_matches_imported_worlds() {
        let filters = LocalLibraryFilters {
            query: String::new(),
            category: Some("maps".into()),
            kind: None,
            tags: vec!["maps".into()],
        };
        assert!(filters.matches(
            "E1L1",
            "Duke Nukem 3D (shareware) duke3d · world · e1l1",
            "map",
            "model/gltf-binary",
            &["maps".into(), "duke3d".into()],
        ));
        assert!(!filters.matches(
            "TILE-2070",
            "billboard",
            "billboard",
            "image/png",
            &["billboards".into(), "duke3d".into()],
        ));
        assert_eq!(library_type("map", "model/gltf-binary"), "maps");
    }
}
