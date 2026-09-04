//! Typed calls against the two HTTP planes of one Asset Server.
//!
//! This layer owns status mapping (401 → `Unauthenticated`, 403 → `Denied`,
//! 404 → `NotFound`, everything else a bounded [`ClientError::Server`]),
//! response-body budgets per route, DTO parsing, and digest verification of
//! canonical manifest bytes. It does NOT own caching, resume loops, or
//! server-identity policy — that is [`crate::client::AssetClient`].
//!
//! Control plane: health, catalog/search, listings, asset detail, aliases,
//! canonical manifest bytes. Data plane: blob HEAD/GET (Range/resume).

use crate::dto::{
    self, AliasDto, AssetDetailDto, AssetsPageDto, AssetsQueryDto, CatalogPageDto, EventsPageDto,
    GameAliasDto, HealthDto, ImportReportDto, ImportStatusDto, JobId, JobProfileDto,
    JobRowDto, SourceCollectionRowDto, SourceCollectionsPageDto, StageOnFailDto,
};
use crate::error::{ClientError, ClientResult};
use crate::http::{self, HttpLimits, Request, Response};
use crate::json::{self, Value};
use crate::wire;
pub use crate::location::ApiEndpoints;
use makepad_asset_data::{
    AssetAlias, AssetId, AssetKind, AssetRevisionId, AssetRevisionRef, BlobId, ClientProfile,
    DerivedVariantId, DeviceTier, FileRole, GameAlias, GameId, GameRevisionId, ImportManifest,
    ImportRevisionId, MediaType, ResolvedVariantMap, Sha256, SourceCollection, SourceCollectionId,
    VariantSetId, VariantSetManifest, RESOLUTION_POLICY_V1,
};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Longest refusal body this client will read for its `error` detail; larger
/// refusal bodies are dropped unread.
const MAX_REFUSAL_BODY_BYTES: u64 = 16 * 1024;
/// Server-side search page cap; requesting more is a local input error.
pub const MAX_SEARCH_LIMIT: u32 = 100;
/// Listing page cap.
pub const MAX_LIST_LIMIT: u64 = 500;

/// One item of an ordered batch pull. `max_bytes` is the caller's own cap —
/// a thumbnail batch says "nothing over 512 KB here", and the server refuses
/// (rather than streams) anything larger, so one mis-sized item cannot eat
/// the batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BatchItem {
    pub blob: BlobId,
    pub max_bytes: Option<u64>,
}

/// What the server did with one batch item. Every requested item gets
/// exactly one of these, in order — silence is never an answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchFrame {
    /// Bytes follow and are digest-verified by the caller.
    Ok,
    /// The store does not hold it.
    Missing,
    /// Over the caller's per-item cap.
    OverItemCap,
    /// The batch byte budget ran out before this item; ask again.
    Skipped,
}

/// Whether to keep reading a batch response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BatchFlow {
    Continue,
    /// Abandon the rest — the caller's priorities changed.
    Stop,
}

/// Read exactly `out.len()` body bytes, refusing a short body.
fn read_exact_body(resp: &mut Response<'_>, out: &mut [u8]) -> ClientResult<()> {
    let mut filled = 0usize;
    while filled < out.len() {
        let n = resp.read_chunk(&mut out[filled..])?;
        if n == 0 {
            return Err(ClientError::Protocol { what: "batch body truncated" });
        }
        filled += n;
    }
    Ok(())
}

/// One blob-garbage-collection request. Every field is optional policy; the
/// server owns the batch sizes, so a client can never ask for an unbounded
/// unit of work.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GcRequest {
    /// Count what would be reclaimed; delete (and retire) nothing.
    pub dry_run: bool,
    /// Protect blobs recorded within this window of the run's start.
    /// `None` = the server's configured default.
    pub grace_ms: Option<u64>,
    /// Also retire every revision of each asset except the newest `n` and
    /// any revision an alias head still points at. `None` = keep all
    /// revisions (the default: GC then only reclaims what is already
    /// unreferenced).
    pub retain_per_asset: Option<u32>,
    /// How many bounded steps this ONE call may perform. `None` = the
    /// server's per-request maximum.
    pub max_steps: Option<u32>,
}

impl GcRequest {
    /// Preview: no deletion, no retirement, exact byte accounting.
    pub fn dry_run() -> Self {
        Self { dry_run: true, ..Self::default() }
    }

    /// Collect what is already unreferenced.
    pub fn collect() -> Self {
        Self::default()
    }
}

/// A validated catalog search. `text` empty = browse mode (filters only).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CatalogQuery {
    pub text: String,
    pub namespace: Option<String>,
    pub kind: Option<AssetKind>,
    pub category: Option<String>,
    pub tag: Option<String>,
    /// Server-side exclusion: hits carrying this tag are dropped by the
    /// server, so a page is never short of `page_size` for local reasons.
    pub exclude_tag: Option<String>,
    pub creator: Option<String>,
    /// Only assets currently referenced by an alias head.
    pub live_only: bool,
    /// 1..=[`MAX_SEARCH_LIMIT`].
    pub page_size: u32,
    /// Ask the server to count the labels of this result set and return the
    /// top `facets` of them with the page; 0 (the default) asks for none.
    /// The counts come out of the SAME snapshot as the hits, so a facet
    /// list can never describe a different generation than the rows.
    pub facets: u32,
}

impl CatalogQuery {
    pub fn browse(page_size: u32) -> Self {
        Self { page_size, ..Self::default() }
    }

    pub fn text(text: impl Into<String>, page_size: u32) -> Self {
        Self { text: text.into(), page_size, ..Self::default() }
    }

    pub(crate) fn validate(&self) -> ClientResult<()> {
        if self.page_size == 0 || self.page_size > MAX_SEARCH_LIMIT {
            return Err(ClientError::InvalidInput { what: "search page_size" });
        }
        if self.text.len() > wire::MAX_QUERY_TEXT_BYTES {
            return Err(ClientError::InvalidInput { what: "search text too long" });
        }
        if self.text.chars().any(char::is_control) {
            return Err(ClientError::InvalidInput { what: "search text control chars" });
        }
        for v in [&self.namespace, &self.category, &self.tag, &self.exclude_tag, &self.creator]
            .into_iter()
            .flatten()
        {
            if v.is_empty()
                || v.len() > wire::MAX_FILTER_VALUE_BYTES
                || v.chars().any(char::is_control)
            {
                return Err(ClientError::InvalidInput { what: "search filter value" });
            }
        }
        Ok(())
    }

    /// The POST body. `cursor` continues a previous page of this same query.
    fn body(&self, cursor: Option<&str>) -> Value {
        let mut pairs: Vec<(&str, Value)> = vec![("q", json::s(self.text.clone()))];
        if let Some(ns) = &self.namespace {
            pairs.push(("ns", json::s(ns.clone())));
        }
        if let Some(kind) = self.kind {
            pairs.push(("kind", json::s(dto::kind_name(kind))));
        }
        if let Some(c) = &self.category {
            pairs.push(("category", json::s(c.clone())));
        }
        if let Some(t) = &self.tag {
            pairs.push(("tag", json::s(t.clone())));
        }
        if let Some(t) = &self.exclude_tag {
            pairs.push(("exclude_tag", json::s(t.clone())));
        }
        if let Some(c) = &self.creator {
            pairs.push(("creator", json::s(c.clone())));
        }
        if self.live_only {
            pairs.push(("live", Value::Bool(true)));
        }
        pairs.push(("limit", Value::Int(self.page_size as i64)));
        if self.facets > 0 {
            pairs.push(("facets", Value::Int(self.facets as i64)));
        }
        if let Some(c) = cursor {
            pairs.push(("cursor", json::s(c)));
        }
        json::obj(pairs)
    }
}

/// What a blob HEAD reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobHead {
    pub size: u64,
    /// The strong ETag equalled the blob's canonical `sha256:<hex>` spelling.
    pub etag_matches: bool,
}

/// What admitting a server-local file BY REFERENCE reported.
///
/// The digest and length are the server's own measurement of the file — the
/// client never read it. `owned` means the store already had those exact
/// bytes in its own CAS, so nothing was referenced and the external file is
/// incidental.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobRefAdmission {
    pub blob: BlobId,
    pub size: u64,
    pub deduped: bool,
    pub owned: bool,
}

/// One reference blob as a re-scan found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobRefRow {
    pub blob: BlobId,
    /// The path on the SERVER's filesystem. Shown to the operator, never
    /// opened by the client.
    pub path: String,
    pub size: u64,
    /// `present` / `missing` / `size_changed` / `content_changed` /
    /// `unreadable`. A string rather than an enum so a newer server can name
    /// a state this build has not heard of without becoming unparseable.
    pub state: String,
    pub ok: bool,
}

/// One bounded page of a reference re-scan.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BlobRefsPage {
    /// How many references the store holds in total.
    pub total: u64,
    pub refs: Vec<BlobRefRow>,
    /// Resume key for the next page; `None` = the walk is finished.
    pub next: Option<BlobId>,
}

/// The searchable annotation written alongside a published artifact.
/// `prompt`/`provenance` are owner-only fields server-side.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AnnotationUpload {
    pub title: String,
    pub description: String,
    pub kind: Option<AssetKind>,
    pub categories: Vec<String>,
    pub tags: Vec<String>,
    pub creator: String,
    pub artist: String,
    pub artist_url: String,
    pub album: String,
    pub source_url: String,
    pub license: String,
    pub license_url: String,
    pub generator: String,
    pub backend: String,
    pub model: String,
    pub prompt: String,
    pub provenance: String,
    pub private: bool,
}

/// One asset of a wire-level batch publication: canonical manifest bytes
/// (carrying asset id and blob refs), the searchable annotation, and an
/// optional alias head. See [`Api::publish_batch`].
#[derive(Clone, Debug)]
pub struct PublishBatchWireItem {
    pub namespace: String,
    /// Canonical manifest bytes; the revision identity is their SHA-256.
    pub manifest: Vec<u8>,
    pub alias: Option<AssetAlias>,
    pub annotation: AnnotationUpload,
}

impl AnnotationUpload {
    fn validate(&self) -> ClientResult<()> {
        if self.title.is_empty() || self.title.len() > wire::MAX_TITLE_BYTES {
            return Err(ClientError::InvalidInput { what: "annotation title" });
        }
        for text in [
            &self.title,
            &self.description,
            &self.artist,
            &self.artist_url,
            &self.album,
            &self.source_url,
            &self.license,
            &self.license_url,
            &self.prompt,
            &self.provenance,
        ] {
            if text.chars().any(char::is_control) {
                return Err(ClientError::InvalidInput { what: "annotation control chars" });
            }
        }
        for label in self.categories.iter().chain(&self.tags) {
            if label.is_empty()
                || label.len() > wire::MAX_FILTER_VALUE_BYTES
                || label.chars().any(char::is_control)
            {
                return Err(ClientError::InvalidInput { what: "annotation label" });
            }
        }
        Ok(())
    }
}

/// One exact operation input selector: asset + revision + role, optionally
/// narrowed by tier/lod, optionally guarded by the expected media type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationInputRef {
    pub slot: String,
    pub asset: AssetId,
    pub revision: AssetRevisionId,
    pub role: FileRole,
    pub tier: Option<DeviceTier>,
    pub lod: Option<u8>,
    pub expected_media: Option<MediaType>,
}

/// Alias expectation of a `publish_and_alias` publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationAliasExpect {
    Any,
    Absent,
    Head(AssetRevisionId),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OperationPublicationRef {
    Publish,
    PublishAndAlias { alias: AssetAlias, expect: OperationAliasExpect },
}

/// A typed operation-creation request. `params` is the (already validated
/// server-side) bounded parameter object; unknown parameters refuse.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationCreateRequest {
    pub namespace: String,
    pub kind: String,
    pub idempotency_key: String,
    pub inputs: Vec<OperationInputRef>,
    pub params: Value,
    pub publication: OperationPublicationRef,
}

impl OperationCreateRequest {
    pub fn new(
        namespace: impl Into<String>,
        kind: impl Into<String>,
        idempotency_key: impl Into<String>,
        inputs: Vec<OperationInputRef>,
    ) -> OperationCreateRequest {
        OperationCreateRequest {
            namespace: namespace.into(),
            kind: kind.into(),
            idempotency_key: idempotency_key.into(),
            inputs,
            params: Value::Obj(Vec::new()),
            publication: OperationPublicationRef::Publish,
        }
    }
}

/// One reported output file of an operation finalize.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperationOutputFile {
    pub role: FileRole,
    pub tier: DeviceTier,
    pub lod: u8,
    pub media: MediaType,
    pub blob: BlobId,
    pub byte_len: u64,
    pub dims: Option<(u32, u32)>,
}

/// The worker's typed completion facts for the single named output of the
/// first-slice operations, plus the ACTUAL model facts of the run.
#[derive(Clone, Debug, PartialEq)]
pub struct OperationFinalizeRequest {
    pub job: JobId,
    pub suffix: Option<String>,
    pub output_name: String,
    pub files: Vec<OperationOutputFile>,
    pub thumbnail: Option<(BlobId, &'static str, u32, u32, u64)>,
    /// (total_bytes, triangles, vertices, joints, clips, max_texture_dim,
    /// media_millis) — measured, never fabricated.
    pub metrics: (u64, u32, u32, u16, u16, u32, u32),
    pub bounds: Option<([f32; 3], [f32; 3])>,
    pub generator: String,
    pub model: String,
    pub version: String,
    pub seed: u64,
}

/// Open a broker-owned chat session bound to one explicit provider.
///
/// With BOTH `client_key` and `context_key` set the call is
/// CREATE-OR-RESUME: the server keeps one durable conversation per
/// `(principal, client_key, context_key)` and answers with that session —
/// same `session` id, transcript intact across worker eviction and server
/// restarts — instead of a fresh one. Without them the session is
/// ephemeral, exactly as before.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatCreateRequest {
    pub namespace: String,
    pub provider: crate::dto::ChatProviderKind,
    /// Declared app profile ("game", "vj"); the broker selects the
    /// session's taught context and tool surface from it. `None` = general.
    pub client: Option<String>,
    /// Who is talking: an opaque display-safe id the app chooses for this
    /// player/device (the sandbox sends `ip:<lan-ip>`; a multiplayer player
    /// id later). See [`wire::chat_key_ok`] for the shape.
    pub client_key: Option<String>,
    /// What the conversation is about: the GAME asset id the chat belongs
    /// to. One conversation per (client, game) — never shared across games.
    pub context_key: Option<String>,
}

impl ChatCreateRequest {
    pub fn new(namespace: impl Into<String>, provider: crate::dto::ChatProviderKind) -> Self {
        ChatCreateRequest {
            namespace: namespace.into(),
            provider,
            client: None,
            client_key: None,
            context_key: None,
        }
    }

    pub fn with_client(mut self, client: impl Into<String>) -> Self {
        self.client = Some(client.into());
        self
    }

    /// See [`ChatCreateRequest::client_key`].
    pub fn with_client_key(mut self, key: impl Into<String>) -> Self {
        self.client_key = Some(key.into());
        self
    }

    /// See [`ChatCreateRequest::context_key`].
    pub fn with_context_key(mut self, key: impl Into<String>) -> Self {
        self.context_key = Some(key.into());
        self
    }

    /// True when this request resumes-or-creates a durable keyed session.
    pub fn is_keyed(&self) -> bool {
        self.client_key.is_some() && self.context_key.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatAttachment {
    pub revision: AssetRevisionId,
    pub role: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChatSendRequest {
    pub text: String,
    pub attachments: Vec<ChatAttachment>,
    pub dynamic_context: Option<String>,
}

impl ChatSendRequest {
    pub fn text(text: impl Into<String>) -> Self {
        ChatSendRequest { text: text.into(), attachments: Vec::new(), dynamic_context: None }
    }

    pub fn with_dynamic_context(mut self, context: impl Into<String>) -> Self {
        self.dynamic_context = Some(context.into());
        self
    }
}

/// `PUT /v1/import-sources` identity echo. `digest` is the SHA-256 of the
/// canonical bytes that were sent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCollectionRegistered {
    pub source_id: String,
    pub digest: SourceCollectionId,
}

pub struct Api {
    pub endpoints: ApiEndpoints,
    pub limits: HttpLimits,
    /// Full validated bearer token (`mpat_…`), attached to every request.
    token: Option<String>,
    /// Keep-alive sockets belonging to THIS handle. A connect per request is
    /// pure overhead against a server on localhost — for a grid of small
    /// thumbnails the handshake costs more than the payload. Every clone
    /// gets its own pool (see `Clone` below), so a socket is only ever used
    /// by one worker at a time.
    pool: http::ConnPool,
    /// Connection reuse switch. `false` restores one-request-per-connection
    /// (`Connection: close`), which is what the pre-keep-alive client did.
    keep_alive: bool,
    /// Whether this server has the batch-fetch route: 0 unknown, 1 yes,
    /// 2 no. Shared across clones — it is a fact about the server, learned
    /// once, so an older server costs exactly one 404 for the whole client.
    batch_route: Arc<AtomicU8>,
}

const BATCH_UNKNOWN: u8 = 0;
const BATCH_PRESENT: u8 = 1;
const BATCH_ABSENT: u8 = 2;

impl Clone for Api {
    /// A clone is another WORKER's handle: same server, same credentials,
    /// its own keep-alive sockets. Sharing a socket across threads would
    /// interleave two requests on one connection.
    fn clone(&self) -> Api {
        Api {
            endpoints: self.endpoints,
            limits: self.limits,
            token: self.token.clone(),
            pool: http::ConnPool::default(),
            keep_alive: self.keep_alive,
            batch_route: self.batch_route.clone(),
        }
    }
}

impl Api {
    pub fn new(endpoints: ApiEndpoints, limits: HttpLimits, token: Option<String>) -> ClientResult<Api> {
        Self::with_keep_alive(endpoints, limits, token, true)
    }

    /// As [`Api::new`] with connection reuse explicitly on or off.
    pub fn with_keep_alive(
        endpoints: ApiEndpoints,
        limits: HttpLimits,
        token: Option<String>,
        keep_alive: bool,
    ) -> ClientResult<Api> {
        limits.validate()?;
        if let Some(t) = &token {
            if !wire::token_shape_ok(t) {
                return Err(ClientError::InvalidInput { what: "bearer token shape" });
            }
        }
        Ok(Api {
            endpoints,
            limits,
            token,
            pool: http::ConnPool::default(),
            keep_alive,
            batch_route: Arc::new(AtomicU8::new(BATCH_UNKNOWN)),
        })
    }

    /// The pool a call should use: `None` disables reuse for this handle.
    fn pool(&self) -> Option<&http::ConnPool> {
        self.keep_alive.then_some(&self.pool)
    }

    /// Idle keep-alive sockets parked on this handle (diagnostics/tests).
    pub fn idle_connections(&self) -> usize {
        self.pool.idle_len()
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    fn bearer(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Issue a request whose success body is JSON, mapping refusals.
    fn call_json(&self, addr: SocketAddr, req: Request<'_>) -> ClientResult<Value> {
        self.call_json_accept(addr, req, &[200])
    }

    fn call_json_accept(
        &self,
        addr: SocketAddr,
        req: Request<'_>,
        allowed: &[u16],
    ) -> ClientResult<Value> {
        Ok(self.call_json_with_status(addr, req, allowed)?.1)
    }

    fn call_json_with_status(
        &self,
        addr: SocketAddr,
        req: Request<'_>,
        allowed: &[u16],
    ) -> ClientResult<(u16, Value)> {
        self.call_json_limited(addr, req, allowed, wire::MAX_JSON_RESPONSE_BYTES)
    }

    fn call_json_limited(
        &self,
        addr: SocketAddr,
        req: Request<'_>,
        allowed: &[u16],
        max_body: u64,
    ) -> ClientResult<(u16, Value)> {
        let resp = http::http_call_pooled(addr, &req, &self.limits, self.pool())?;
        let resp = self.accept(resp, allowed)?;
        let status = resp.head().status;
        if resp.head().content_length > max_body {
            return Err(ClientError::OverBudget {
                what: "json response body",
                limit: max_body,
                found: resp.head().content_length,
            });
        }
        let body = resp.read_full(max_body)?;
        let value = json::parse(&body)
            .map_err(|_| ClientError::Protocol { what: "malformed json body" })?;
        Ok((status, value))
    }

    /// Enforce an allowed status set; anything else becomes a typed refusal
    /// (with a bounded, sanitized detail when the body offers one).
    fn accept<'a>(&self, resp: Response<'a>, allowed: &[u16]) -> ClientResult<Response<'a>> {
        let status = resp.head().status;
        if allowed.contains(&status) {
            return Ok(resp);
        }
        Err(self.refusal(resp))
    }

    fn refusal(&self, resp: Response) -> ClientError {
        let status = resp.head().status;
        let detail = if resp.head().content_length <= MAX_REFUSAL_BODY_BYTES {
            resp.read_full(MAX_REFUSAL_BODY_BYTES)
                .ok()
                .and_then(|b| dto::parse_error_detail(&b))
        } else {
            None
        };
        match status {
            401 => ClientError::Unauthenticated,
            403 => ClientError::Denied,
            404 => ClientError::NotFound { what: "server object" },
            _ => ClientError::Server { status, detail },
        }
    }

    // ---- control plane -----------------------------------------------------

    pub fn health(&self) -> ClientResult<HealthDto> {
        let path = wire::path_health();
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_health(&v)
    }

    /// One page of catalog search. The cursor must come from a previous page
    /// of the same query on the same server (the caller enforces the server
    /// half via [`crate::client::PageCursor`]; the server enforces the query
    /// shape).
    pub fn catalog_search(
        &self,
        query: &CatalogQuery,
        cursor: Option<&str>,
    ) -> ClientResult<CatalogPageDto> {
        query.validate()?;
        if let Some(c) = cursor {
            check_cursor_out(c)?;
        }
        let body = query.body(cursor).to_json().into_bytes();
        let path = wire::path_catalog_search();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_catalog_page(&v)
    }

    /// Run one bounded, single-SELECT query against the server's live asset
    /// catalog. The server owns the row, value, step, and deadline budgets.
    pub fn assets_query(&self, sql: &str) -> ClientResult<AssetsQueryDto> {
        if sql.trim().is_empty() || sql.len() > 4096 {
            return Err(ClientError::InvalidInput { what: "assets query sql" });
        }
        let body = json::obj(vec![("sql", json::s(sql))]).to_json().into_bytes();
        let mut req = Request::post("/v1/assets/query", &body);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_assets_query(&v)
    }

    /// One page of the keyset asset listing.
    pub fn assets_page(
        &self,
        namespace: Option<&str>,
        cursor: Option<&str>,
        limit: u64,
    ) -> ClientResult<AssetsPageDto> {
        if limit == 0 || limit > MAX_LIST_LIMIT {
            return Err(ClientError::InvalidInput { what: "listing limit" });
        }
        if let Some(ns) = namespace {
            if ns.len() > wire::MAX_NAMESPACE_BYTES || !wire::query_value_ok(ns) {
                return Err(ClientError::InvalidInput { what: "listing namespace" });
            }
        }
        if let Some(c) = cursor {
            check_cursor_out(c)?;
        }
        let path = wire::path_assets(namespace, cursor, limit);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_assets_page(&v)
    }

    pub fn asset_detail(&self, id: &AssetId) -> ClientResult<AssetDetailDto> {
        let path = wire::path_asset(id);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let detail = dto::parse_asset_detail(&v)?;
        // The response must describe the asset that was asked for.
        if &detail.asset_id != id {
            return Err(ClientError::Protocol { what: "asset detail id mismatch" });
        }
        Ok(detail)
    }

    /// BATCH ALIAS STATUS — a whole bundled library's worth of "do you
    /// already have this, and is it current?" in ONE round trip.
    ///
    /// `entries` pairs each alias with the Source blob the caller holds
    /// (`None` = do not compare); `tags` names the annotation tags the
    /// caller wants reported back per entry, which is how an ownership
    /// convention like `builtin` is checked without a search per asset.
    ///
    /// Answers come back in request order and each echoes its own alias, so
    /// nobody has to trust an ordinal alone.
    pub fn alias_status(
        &self,
        entries: &[(AssetAlias, Option<BlobId>)],
        tags: &[String],
    ) -> ClientResult<Vec<dto::AliasStatusDto>> {
        if entries.is_empty() {
            return Ok(Vec::new());
        }
        if entries.len() > wire::MAX_ALIAS_STATUS_ITEMS {
            return Err(ClientError::InvalidInput { what: "alias status batch size" });
        }
        let items: Vec<json::Value> = entries
            .iter()
            .map(|(alias, source)| {
                let mut pairs = vec![("alias", json::Value::Str(alias.as_str().to_string()))];
                if let Some(blob) = source {
                    pairs.push(("source", json::Value::Str(blob.to_string())));
                }
                json::obj(pairs)
            })
            .collect();
        let body = json::obj(vec![
            (
                "tags",
                json::Value::Arr(
                    tags.iter().map(|t| json::Value::Str(t.clone())).collect(),
                ),
            ),
            ("entries", json::Value::Arr(items)),
        ])
        .to_json()
        .into_bytes();
        let path = wire::path_alias_status();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let resp = http::http_call_pooled(self.endpoints.control, &req, &self.limits, self.pool())?;
        let resp = self.accept(resp, &[200])?;
        // A whole library's statuses is a bigger answer than an ordinary
        // JSON route's, and it is all small fixed fields.
        let max_body = wire::MAX_JSON_RESPONSE_BYTES * 4;
        if resp.head().content_length > max_body {
            return Err(ClientError::OverBudget {
                what: "alias status body",
                limit: max_body,
                found: resp.head().content_length,
            });
        }
        let bytes = resp.read_full(max_body)?;
        let value = json::parse(&bytes)
            .map_err(|_| ClientError::Protocol { what: "malformed json body" })?;
        let rows = dto::parse_alias_status(&value)?;
        if rows.len() != entries.len() {
            return Err(ClientError::Protocol { what: "alias status entry count" });
        }
        // Positional identity, and the entry says its own alias: both have
        // to agree or the answer describes something else.
        for (row, (alias, _)) in rows.iter().zip(entries) {
            if &row.alias != alias {
                return Err(ClientError::Protocol { what: "alias status order" });
            }
        }
        Ok(rows)
    }

    pub fn resolve_alias(&self, alias: &AssetAlias) -> ClientResult<AliasDto> {
        let path = wire::path_alias(alias);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let dto = dto::parse_alias(&v)?;
        if &dto.alias != alias {
            return Err(ClientError::Protocol { what: "alias response mismatch" });
        }
        Ok(dto)
    }

    pub fn resolve_game_alias(&self, alias: &GameAlias) -> ClientResult<GameAliasDto> {
        let path = wire::path_game_alias(alias);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let dto = dto::parse_game_alias(&v)?;
        if &dto.alias != alias {
            return Err(ClientError::Protocol {
                what: "game alias response mismatch",
            });
        }
        Ok(dto)
    }

    /// One page of the committed-catalog event feed. `wait_ms` may be zero
    /// (pure poll); the HTTP head deadline stretches to cover the server-side
    /// wait plus margin, because a long-poll legitimately answers late.
    pub fn events_page(
        &self,
        cursor: Option<&str>,
        wait_ms: u64,
        limit: u32,
        kind: Option<makepad_asset_data::AssetKind>,
    ) -> ClientResult<EventsPageDto> {
        if wait_ms > wire::MAX_EVENT_WAIT_MS {
            return Err(ClientError::InvalidInput { what: "event wait too long" });
        }
        if limit == 0 || limit > wire::MAX_EVENT_BATCH {
            return Err(ClientError::InvalidInput { what: "event limit" });
        }
        if let Some(c) = cursor {
            if c.len() > wire::MAX_EVENT_CURSOR_BYTES || wire::event_cursor_seq(c).is_none() {
                return Err(ClientError::InvalidInput { what: "event cursor shape" });
            }
        }
        let path = wire::path_events(cursor, wait_ms, limit, kind.map(dto::kind_name));
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        req.head_deadline_ms = Some(wait_ms + self.limits.head_deadline_ms);
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_events_page(&v)
    }

    /// Pull many blobs in ONE request, in the order given, streaming each
    /// item to `on_frame` as it arrives.
    ///
    /// The order is the contract: frame *i* carries item *i*'s digest, and a
    /// response that reorders or substitutes digests is refused as a protocol
    /// violation. That is what lets a caller prioritise — ask for the visible
    /// thumbnails first, and they are the first bytes on the wire.
    ///
    /// `on_frame` returns whether to keep reading; answering `Stop` abandons
    /// the rest of the response (the socket is dropped, never pooled), which
    /// is exactly how a UI re-prioritises mid-stream: everything already
    /// handed to `on_frame` is already yours.
    ///
    /// A server without the route answers 404; this reports
    /// [`ClientError::NotFound`] and remembers, so the whole client falls
    /// back to single GETs after one refusal.
    pub fn fetch_blob_batch(
        &self,
        items: &[BatchItem],
        body_deadline_ms: u64,
        on_frame: &mut dyn FnMut(BlobId, BatchFrame, &[u8]) -> BatchFlow,
    ) -> ClientResult<()> {
        if items.is_empty() {
            return Err(ClientError::InvalidInput { what: "empty batch" });
        }
        if items.len() > wire::MAX_BLOB_BATCH_ITEMS {
            return Err(ClientError::InvalidInput { what: "batch too large" });
        }
        if self.batch_route.load(Ordering::Relaxed) == BATCH_ABSENT {
            return Err(ClientError::NotFound { what: "blob batch route" });
        }
        let mut entries = Vec::with_capacity(items.len());
        for item in items {
            let mut pairs: Vec<(&str, Value)> = vec![("blob", json::s(item.blob.to_string()))];
            if let Some(max) = item.max_bytes {
                pairs.push(("max_bytes", Value::Int(max.min(i64::MAX as u64) as i64)));
            }
            entries.push(json::obj(pairs));
        }
        let body = json::obj(vec![("blobs", Value::Arr(entries))]).to_json().into_bytes();
        let path = wire::path_blobs_batch();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        req.body_deadline_ms = Some(body_deadline_ms.max(1));
        let resp =
            http::http_call_pooled(self.endpoints.data, &req, &self.limits, self.pool())?;
        if resp.head().status == 404 {
            // Older server: stop trying on every later batch.
            self.batch_route.store(BATCH_ABSENT, Ordering::Relaxed);
            return Err(ClientError::NotFound { what: "blob batch route" });
        }
        let mut resp = self.accept(resp, &[200])?;
        if resp.head().content_type.as_deref() != Some(wire::BLOB_BATCH_CONTENT_TYPE) {
            return Err(ClientError::Protocol { what: "batch content type" });
        }
        if resp.head().content_length > wire::MAX_BLOB_BATCH_BYTES {
            return Err(ClientError::OverBudget {
                what: "batch response body",
                limit: wire::MAX_BLOB_BATCH_BYTES,
                found: resp.head().content_length,
            });
        }
        self.batch_route.store(BATCH_PRESENT, Ordering::Relaxed);

        let mut header = [0u8; wire::BLOB_BATCH_FRAME_HEADER];
        for item in items {
            read_exact_body(&mut resp, &mut header)?;
            let status = match header[0] {
                0 => BatchFrame::Ok,
                1 => BatchFrame::Missing,
                2 => BatchFrame::OverItemCap,
                3 => BatchFrame::Skipped,
                _ => return Err(ClientError::Protocol { what: "batch frame status" }),
            };
            let mut digest = [0u8; 32];
            digest.copy_from_slice(&header[1..33]);
            // Positional identity: frame i IS item i. Anything else and the
            // caller could attribute bytes to the wrong request.
            if &digest != item.blob.as_bytes() {
                return Err(ClientError::Protocol { what: "batch frame order" });
            }
            let len = u64::from_be_bytes(header[33..41].try_into().expect("8 bytes"));
            if status != BatchFrame::Ok && len != 0 {
                return Err(ClientError::Protocol { what: "batch refusal with body" });
            }
            if len > wire::MAX_BLOB_BATCH_ITEM_BYTES {
                return Err(ClientError::OverBudget {
                    what: "batch item bytes",
                    limit: wire::MAX_BLOB_BATCH_ITEM_BYTES,
                    found: len,
                });
            }
            let mut bytes = vec![0u8; len as usize];
            read_exact_body(&mut resp, &mut bytes)?;
            if on_frame(item.blob, status, &bytes) == BatchFlow::Stop {
                // Dropping `resp` with body left closes the socket instead of
                // pooling it: framing beyond this point is not our business.
                return Ok(());
            }
        }
        Ok(())
    }

    /// Canonical asset-manifest bytes, digest-verified against `rev` before
    /// they are returned. The caller decodes via the content contract.
    pub fn fetch_revision_bytes(&self, rev: &AssetRevisionId) -> ClientResult<Vec<u8>> {
        let path = wire::path_revision(rev);
        self.fetch_canonical(self.endpoints.control, &path, rev.as_bytes(), "asset revision bytes")
    }

    /// Canonical game-revision bytes, digest-verified against `rev`.
    pub fn fetch_game_revision_bytes(&self, rev: &GameRevisionId) -> ClientResult<Vec<u8>> {
        let path = wire::path_game_revision(rev);
        self.fetch_canonical(self.endpoints.control, &path, rev.as_bytes(), "game revision bytes")
    }

    fn fetch_canonical(
        &self,
        addr: SocketAddr,
        path: &str,
        expected: &[u8; 32],
        what: &'static str,
    ) -> ClientResult<Vec<u8>> {
        let mut req = Request::get(path);
        req.bearer = self.bearer();
        let resp = http::http_call_pooled(addr, &req, &self.limits, self.pool())?;
        let resp = self.accept(resp, &[200])?;
        if resp.head().content_length > wire::MAX_MANIFEST_RESPONSE_BYTES {
            return Err(ClientError::OverBudget {
                what,
                limit: wire::MAX_MANIFEST_RESPONSE_BYTES,
                found: resp.head().content_length,
            });
        }
        let bytes = resp.read_full(wire::MAX_MANIFEST_RESPONSE_BYTES)?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let found = hasher.finalize();
        if &found != expected {
            return Err(ClientError::DigestMismatch { what, expected: *expected, found });
        }
        Ok(bytes)
    }

    // ---- data plane --------------------------------------------------------

    /// Size probe for a blob. Also reports whether the strong ETag equals the
    /// blob's canonical spelling — a server that cannot echo the identity it
    /// serves is not trusted with a later `If-Range` resume.
    pub fn blob_head(&self, blob: &BlobId) -> ClientResult<BlobHead> {
        let path = wire::path_blob(blob);
        let mut req = Request::head(&path);
        req.bearer = self.bearer();
        let resp =
            http::http_call_pooled(self.endpoints.data, &req, &self.limits, self.pool())?;
        let resp = self.accept(resp, &[200])?;
        let head = resp.head();
        let etag_matches = head.etag.as_deref() == Some(&blob.to_string());
        Ok(BlobHead { size: head.content_length, etag_matches })
    }

    // ---- live game rooms ---------------------------------------------------

    /// Who is playing what, right now. `game` narrows it to the one room a
    /// player about to press Play cares about.
    ///
    /// A room is a running process on somebody's desk, so this list is a
    /// snapshot and nothing else: by the time it is read, a host may already
    /// have closed the lid. Callers must treat a failed dial as ordinary
    /// (see `claim_room`'s `replacing`), never as an error to report.
    pub fn rooms(&self, game: Option<&str>) -> ClientResult<Vec<crate::dto::RoomDto>> {
        if let Some(g) = game {
            if g.is_empty() || g.len() > wire::MAX_ROOM_GAME_BYTES || !wire::query_value_ok(g) {
                return Err(ClientError::InvalidInput { what: "room game id" });
            }
        }
        let path = wire::path_rooms(game);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_rooms(&v)
    }

    /// Take a game's claim, or learn who holds it — one atomic call, because
    /// two people pressing Play in the same second must not both come away
    /// believing they are the host.
    ///
    /// `replacing` names a room this caller actually tried to dial and could
    /// not reach. It is the only way a live claim changes hands, and it is
    /// what stops a stale room becoming a wall every later player runs into:
    /// a failed join is always followed by a claim that names the room it
    /// failed on, so the joiner hosts instead of retrying forever.
    pub fn claim_room(
        &self,
        game: &str,
        invite: &str,
        host: &str,
        ttl_ms: u64,
        replacing: Option<&str>,
    ) -> ClientResult<crate::dto::RoomClaimDto> {
        let bounded = |text: &str, max: usize| {
            !text.is_empty() && text.len() <= max && !text.chars().any(char::is_control)
        };
        if !bounded(game, wire::MAX_ROOM_GAME_BYTES) {
            return Err(ClientError::InvalidInput { what: "room game id" });
        }
        if !bounded(invite, wire::MAX_ROOM_INVITE_BYTES) {
            return Err(ClientError::InvalidInput { what: "room invite" });
        }
        if !bounded(host, wire::MAX_ROOM_HOST_BYTES) {
            return Err(ClientError::InvalidInput { what: "room host name" });
        }
        if !(wire::MIN_ROOM_TTL_MS..=wire::MAX_ROOM_TTL_MS).contains(&ttl_ms) {
            return Err(ClientError::InvalidInput { what: "room ttl" });
        }
        let mut pairs: Vec<(&str, Value)> = vec![
            ("game", json::s(game)),
            ("invite", json::s(invite)),
            ("host", json::s(host)),
            ("ttl_ms", Value::Int(ttl_ms.min(i64::MAX as u64) as i64)),
        ];
        if let Some(room) = replacing {
            if !bounded(room, 64) {
                return Err(ClientError::InvalidInput { what: "replaced room id" });
            }
            pairs.push(("replacing", json::s(room)));
        }
        let body = json::obj(pairs).to_json().into_bytes();
        let path = wire::path_rooms_claim();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        // 201 the claim is mine, 200 somebody else holds it. Both are
        // answers, not failures.
        let v = self.call_json_accept(self.endpoints.control, req, &[200, 201])?;
        dto::parse_room_claim(&v)
    }

    /// "Still here" — renew the room's lease. A [`ClientError::NotFound`]
    /// means the claim moved on (it lapsed, or a joiner that could not reach
    /// this host took it), and the answer is to claim again, not to retry.
    pub fn room_heartbeat(
        &self,
        room: &str,
        token: &str,
        ttl_ms: u64,
    ) -> ClientResult<crate::dto::RoomDto> {
        self.room_heartbeat_with(room, token, ttl_ms, None)
    }

    /// Heartbeat that also reports the head count in the world (host
    /// included) so a games list can show who is playing. `None` leaves the
    /// server's last count alone.
    pub fn room_heartbeat_with(
        &self,
        room: &str,
        token: &str,
        ttl_ms: u64,
        players: Option<u32>,
    ) -> ClientResult<crate::dto::RoomDto> {
        let mut body = self.room_token_body(room, token, Some(ttl_ms))?;
        if let Some(players) = players {
            // Splice the count into the bounded body rather than rebuild it:
            // the token body is the one place the room proof is spelled.
            let count = players.clamp(1, 1024);
            let mut v = json::parse(&body).map_err(|_| ClientError::Protocol { what: "room body" })?;
            if let Value::Obj(pairs) = &mut v {
                pairs.push(("players".to_string(), Value::Int(count as i64)));
            }
            body = v.to_json().into_bytes();
        }
        let path = wire::path_room_heartbeat(room);
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_room_envelope(&v)
    }

    /// Give the claim up. Idempotent at the server: a room already gone
    /// answers the same way, because a host that leaves and then exits runs
    /// both paths and neither is wrong.
    pub fn retire_room(&self, room: &str, token: &str) -> ClientResult<()> {
        let body = self.room_token_body(room, token, None)?;
        let path = wire::path_room_retire(room);
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        req.allow_no_content = true;
        let resp =
            http::http_call_pooled(self.endpoints.control, &req, &self.limits, self.pool())?;
        self.accept(resp, &[200, 204])?;
        Ok(())
    }

    fn room_token_body(
        &self,
        room: &str,
        token: &str,
        ttl_ms: Option<u64>,
    ) -> ClientResult<Vec<u8>> {
        let bounded = |text: &str| {
            !text.is_empty() && text.len() <= 64 && !text.chars().any(char::is_control)
        };
        if !bounded(room) || !wire::query_value_ok(room) {
            return Err(ClientError::InvalidInput { what: "room id" });
        }
        if !bounded(token) {
            return Err(ClientError::InvalidInput { what: "room token" });
        }
        let mut pairs: Vec<(&str, Value)> = vec![("token", json::s(token))];
        if let Some(ttl_ms) = ttl_ms {
            if !(wire::MIN_ROOM_TTL_MS..=wire::MAX_ROOM_TTL_MS).contains(&ttl_ms) {
                return Err(ClientError::InvalidInput { what: "room ttl" });
            }
            pairs.push(("ttl_ms", Value::Int(ttl_ms.min(i64::MAX as u64) as i64)));
        }
        Ok(json::obj(pairs).to_json().into_bytes())
    }

    // ---- jobs (generation scheduling) --------------------------------------

    /// Announce (or renew) what this worker can execute RIGHT NOW. The
    /// server merges the announcement over its config advertisement and
    /// lets it expire after `ttl_ms`, so a worker that dies stops
    /// advertising by itself — call this on a cadence well inside the ttl.
    ///
    /// `domains` are the capability domains this worker covers. It is
    /// authoritative for all of them: announcing a domain with no profile
    /// in it withdraws the deployment's static profiles there, which is
    /// exactly what "the fleet cannot run this today" has to mean.
    pub fn announce_job_profiles(
        &self,
        worker: &str,
        ns: &str,
        ttl_ms: u64,
        domains: &[String],
        profiles: &[JobProfileDto],
    ) -> ClientResult<()> {
        if worker.is_empty() || worker.len() > 64 || worker.chars().any(char::is_control) {
            return Err(ClientError::InvalidInput { what: "worker id" });
        }
        if ns.is_empty() || ns.len() > wire::MAX_NAMESPACE_BYTES || !wire::query_value_ok(ns) {
            return Err(ClientError::InvalidInput { what: "worker namespace" });
        }
        if profiles.len() > wire::MAX_JOB_PROFILES || domains.len() > 32 {
            return Err(ClientError::InvalidInput { what: "profile announcement size" });
        }
        let rows: Vec<Value> = profiles
            .iter()
            .map(|p| {
                json::obj(vec![
                    ("id", json::s(p.id.clone())),
                    ("domain", json::s(p.domain.clone())),
                    ("label", json::s(p.label.clone())),
                    ("kind", json::s(p.kind.clone())),
                    ("namespace", json::s(p.namespace.clone())),
                    ("defaults", p.defaults.clone()),
                ])
            })
            .collect();
        let body = json::obj(vec![
            ("worker", json::s(worker.to_string())),
            ("namespace", json::s(ns.to_string())),
            ("ttl_ms", Value::Int(ttl_ms as i64)),
            (
                "domains",
                Value::Arr(domains.iter().map(|d| json::s(d.clone())).collect()),
            ),
            ("profiles", Value::Arr(rows)),
        ])
        .to_json()
        .into_bytes();
        self.put_job_profiles(&body)
    }

    /// Withdraw this worker's announcement (clean shutdown).
    pub fn retract_job_profiles(&self, worker: &str, ns: &str) -> ClientResult<()> {
        let body = json::obj(vec![
            ("worker", json::s(worker.to_string())),
            ("namespace", json::s(ns.to_string())),
            ("retract", Value::Bool(true)),
        ])
        .to_json()
        .into_bytes();
        self.put_job_profiles(&body)
    }

    fn put_job_profiles(&self, body: &[u8]) -> ClientResult<()> {
        let path = wire::path_job_profiles(None);
        let mut req = Request::put(&path, body);
        req.bearer = self.bearer();
        req.allow_no_content = true;
        let resp =
            http::http_call_pooled(self.endpoints.control, &req, &self.limits, self.pool())?;
        self.accept(resp, &[200, 204])?;
        Ok(())
    }

    /// One page of the scoped job listing. `namespace` requires a job
    /// capability on that namespace server-side; `None` lists the caller's
    /// own jobs. `kind` and `state` narrow the page to one queue — the
    /// server answers a state filter from the QUEUE index, so "the pending
    /// annotate jobs" is exact rather than whatever survived a page of
    /// everything. The server returns newest first, capped at
    /// [`MAX_LIST_LIMIT`].
    pub fn list_jobs(
        &self,
        namespace: Option<&str>,
        kind: Option<&str>,
        state: Option<crate::dto::JobStateDto>,
        limit: u64,
    ) -> ClientResult<Vec<JobRowDto>> {
        if limit == 0 || limit > MAX_LIST_LIMIT {
            return Err(ClientError::InvalidInput { what: "job list limit" });
        }
        if let Some(ns) = namespace {
            if ns.len() > wire::MAX_NAMESPACE_BYTES || !wire::query_value_ok(ns) {
                return Err(ClientError::InvalidInput { what: "job list namespace" });
            }
        }
        if let Some(kind) = kind {
            if kind.is_empty() || kind.len() > 64 || !wire::query_value_ok(kind) {
                return Err(ClientError::InvalidInput { what: "job list kind" });
            }
        }
        let path = wire::path_jobs_list(namespace, kind, state.map(|s| s.as_str()), limit);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_jobs_page(&v)
    }

    // ---- pipelines (declared multi-stage runs) ------------------------------

    // ---- worker protocol (fleet dispatchers) --------------------------------

    // ---- write plane (artifact publication) --------------------------------

    /// Content-addressed upload to the data plane. The response identity
    /// must equal the locally computed digest — a server that answers with
    /// another identity is refused. Hashes `bytes` once, on this call; a
    /// caller that already knows (and has verified) the digest — e.g.
    /// against an upload plan's expected digest — should call
    /// [`Self::upload_blob_with_digest`] instead to avoid hashing the same
    /// bytes a second time.
    pub fn upload_blob(&self, ns: &str, bytes: &[u8]) -> ClientResult<BlobId> {
        let local = BlobId::hash_of(bytes);
        self.upload_blob_with_digest(ns, bytes, local)
    }

    /// Announce one in-memory LocalGen preview session. Mesh parts follow on
    /// the data plane; none of these calls create a durable blob/revision.
    pub fn open_model_preview(
        &self,
        alias: &AssetAlias,
        session: &str,
        program: &str,
    ) -> ClientResult<()> {
        if !alias.as_str().starts_with("gen/csg/") {
            return Err(ClientError::InvalidInput { what: "model preview alias" });
        }
        validate_preview_session(session)?;
        if program.len() > 12_000 {
            return Err(ClientError::InvalidInput { what: "model preview program" });
        }
        let body = json::obj(vec![
            ("op", json::s("open")),
            ("alias", json::s(alias.as_str())),
            ("session", json::s(session)),
            ("program", json::s(program)),
        ])
        .to_json()
        .into_bytes();
        let path = wire::path_model_previews();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        self.call_json(self.endpoints.control, req)?;
        Ok(())
    }

    pub fn update_model_preview(
        &self,
        session: &str,
        program: Option<&str>,
        removed: &[String],
        renamed: &[crate::dto::ModelPreviewRenameDto],
    ) -> ClientResult<()> {
        validate_preview_session(session)?;
        if program.is_some_and(|program| program.len() > 12_000) {
            return Err(ClientError::InvalidInput { what: "model preview program" });
        }
        if removed.len() > 32 || renamed.len() > 32 {
            return Err(ClientError::InvalidInput { what: "model preview delta" });
        }
        for name in removed {
            validate_preview_part(name)?;
        }
        for rename in renamed {
            validate_preview_part(&rename.from)?;
            validate_preview_part(&rename.to)?;
        }
        let body = json::obj(vec![
            ("op", json::s("delta")),
            ("session", json::s(session)),
            (
                "program",
                program.map_or(Value::Null, |program| json::s(program)),
            ),
            (
                "removed",
                Value::Arr(removed.iter().map(|name| json::s(name)).collect()),
            ),
            (
                "renamed",
                Value::Arr(
                    renamed
                        .iter()
                        .map(|rename| {
                            json::obj(vec![
                                ("from", json::s(&rename.from)),
                                ("to", json::s(&rename.to)),
                            ])
                        })
                        .collect(),
                ),
            ),
        ])
        .to_json()
        .into_bytes();
        let path = wire::path_model_previews();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        self.call_json(self.endpoints.control, req)?;
        Ok(())
    }

    pub fn clear_model_preview(&self, session: &str) -> ClientResult<()> {
        validate_preview_session(session)?;
        let body = json::obj(vec![
            ("op", json::s("clear")),
            ("session", json::s(session)),
        ])
        .to_json()
        .into_bytes();
        let path = wire::path_model_previews();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        self.call_json(self.endpoints.control, req)?;
        Ok(())
    }

    /// Upload one changed part directly into the server's bounded preview
    /// memory and return its opaque fetch token.
    pub fn upload_model_preview_part(
        &self,
        session: &str,
        part: &str,
        bytes: &[u8],
    ) -> ClientResult<String> {
        validate_preview_session(session)?;
        validate_preview_part(part)?;
        if bytes.is_empty() {
            return Err(ClientError::InvalidInput { what: "model preview mesh" });
        }
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(ClientError::OverBudget {
                what: "model preview mesh",
                limit: 16 * 1024 * 1024,
                found: bytes.len() as u64,
            });
        }
        let path = wire::path_model_preview_part(session, part);
        let mut req = Request::put(&path, bytes);
        req.body_content_type = "model/gltf-binary";
        req.bearer = self.bearer();
        let value = self.call_json(self.endpoints.data, req)?;
        let token = value
            .get("mesh_token")
            .and_then(Value::as_str)
            .ok_or(ClientError::Protocol { what: "model preview mesh token" })?;
        validate_preview_mesh_token(token)?;
        Ok(token.to_string())
    }

    pub fn fetch_model_preview_mesh(&self, token: &str) -> ClientResult<Vec<u8>> {
        validate_preview_mesh_token(token)?;
        let path = wire::path_model_preview_mesh(token);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let response = http::http_call_pooled(
            self.endpoints.data,
            &req,
            &self.limits,
            self.pool(),
        )?;
        let response = self.accept(response, &[200])?;
        const MAX_PREVIEW_MESH_BYTES: u64 = 16 * 1024 * 1024;
        if response.head().content_length > MAX_PREVIEW_MESH_BYTES {
            return Err(ClientError::OverBudget {
                what: "model preview mesh",
                limit: MAX_PREVIEW_MESH_BYTES,
                found: response.head().content_length,
            });
        }
        if response.head().content_type.as_deref() != Some("model/gltf-binary") {
            return Err(ClientError::Protocol { what: "model preview mesh content type" });
        }
        response.read_full(MAX_PREVIEW_MESH_BYTES)
    }

    /// As [`Self::upload_blob`], but `digest` is supplied by the caller
    /// instead of being (re)computed here. `digest` MUST be
    /// `BlobId::hash_of(bytes)` — this only skips a redundant local hash
    /// pass, it never weakens the identity guarantee: the server's echoed
    /// blob id is still checked against `digest`, and a disagreement is
    /// still refused.
    pub fn upload_blob_with_digest(
        &self,
        ns: &str,
        bytes: &[u8],
        digest: BlobId,
    ) -> ClientResult<BlobId> {
        if ns.is_empty() || ns.len() > wire::MAX_NAMESPACE_BYTES || !wire::query_value_ok(ns) {
            return Err(ClientError::InvalidInput { what: "upload namespace" });
        }
        if bytes.is_empty() {
            return Err(ClientError::InvalidInput { what: "upload empty blob" });
        }
        let path = wire::path_blob_upload(ns);
        let mut req = Request::post(&path, bytes);
        req.body_content_type = "application/octet-stream";
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.data, req, &[200, 201])?;
        let got = v
            .get("blob_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<BlobId>().ok())
            .ok_or(ClientError::Protocol { what: "upload blob_id" })?;
        if got != digest {
            return Err(ClientError::DigestMismatch {
                what: "uploaded blob identity",
                expected: *digest.as_bytes(),
                found: *got.as_bytes(),
            });
        }
        Ok(digest)
    }

    /// Admit MANY blobs in ONE request: the server hashes and fsyncs every
    /// object off its state thread, then records the whole set in ONE
    /// catalog transaction. Every echoed digest is verified against the
    /// locally computed one before it is trusted. Order is preserved.
    /// Hashes every blob once, on this call; a caller that already knows
    /// (and has verified) each digest should call
    /// [`Self::upload_blob_batch_with_digests`] instead to avoid hashing the
    /// same bytes a second time.
    pub fn upload_blob_batch(&self, ns: &str, blobs: &[&[u8]]) -> ClientResult<Vec<BlobId>> {
        let with_digests: Vec<(BlobId, &[u8])> =
            blobs.iter().map(|bytes| (BlobId::hash_of(bytes), *bytes)).collect();
        self.upload_blob_batch_with_digests(ns, &with_digests)
    }

    /// As [`Self::upload_blob_batch`], but each digest is supplied by the
    /// caller instead of being (re)computed here — see
    /// [`Self::upload_blob_with_digest`] for the same trade-off on a single
    /// blob. Every echoed digest is still verified against the SUPPLIED
    /// local one before it is trusted; a disagreement is still refused.
    ///
    /// This client does not assume the server's batch-byte budget matches
    /// its own (see [`wire::UPLOAD_BATCH_SAFE_BYTES`]): a 413 ("body too
    /// large") for the whole request is not fatal here. On a 413 the batch
    /// is split in half and each half is retried, recursively, until every
    /// half either fits or is down to one blob — the one shape a 413 there
    /// cannot be worked around (that single blob alone is over the server's
    /// per-request budget; see [`Self::upload_blob_with_digest`] for that
    /// case, which this does not attempt to solve).
    pub fn upload_blob_batch_with_digests(
        &self,
        ns: &str,
        blobs: &[(BlobId, &[u8])],
    ) -> ClientResult<Vec<BlobId>> {
        if ns.is_empty() || ns.len() > wire::MAX_NAMESPACE_BYTES || !wire::query_value_ok(ns) {
            return Err(ClientError::InvalidInput { what: "upload namespace" });
        }
        if blobs.is_empty() || blobs.len() > wire::MAX_UPLOAD_BATCH_ITEMS {
            return Err(ClientError::InvalidInput { what: "upload batch size" });
        }
        for (_, bytes) in blobs {
            if bytes.is_empty() {
                return Err(ClientError::InvalidInput { what: "upload empty blob" });
            }
        }
        self.upload_blob_batch_split(ns, blobs)
    }

    /// One `upload_blob_batch` request for `blobs`; on a 413 for more than
    /// one blob, split in half and retry each half. Every split at least
    /// halves the batch, so recursion depth is bounded by
    /// log2(MAX_UPLOAD_BATCH_ITEMS).
    fn upload_blob_batch_split(&self, ns: &str, blobs: &[(BlobId, &[u8])]) -> ClientResult<Vec<BlobId>> {
        match self.upload_blob_batch_once(ns, blobs) {
            Err(ClientError::Server { status: 413, .. }) if blobs.len() > 1 => {
                let mid = blobs.len() / 2;
                let (a, b) = blobs.split_at(mid);
                let mut out = self.upload_blob_batch_split(ns, a)?;
                out.extend(self.upload_blob_batch_split(ns, b)?);
                Ok(out)
            }
            other => other,
        }
    }

    /// Exactly one wire request for `blobs`, no retry, no splitting.
    fn upload_blob_batch_once(&self, ns: &str, blobs: &[(BlobId, &[u8])]) -> ClientResult<Vec<BlobId>> {
        let mut body: Vec<u8> = Vec::with_capacity(
            blobs.iter().map(|(_, bytes)| bytes.len() + 8).sum::<usize>(),
        );
        for (_, bytes) in blobs {
            body.extend_from_slice(&(bytes.len() as u64).to_be_bytes());
            body.extend_from_slice(bytes);
        }
        let path = wire::path_blob_upload_batch(ns);
        let mut req = Request::post(&path, &body);
        req.body_content_type = "application/octet-stream";
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.data, req, &[200, 201])?;
        let rows = v
            .get("blobs")
            .and_then(Value::as_arr)
            .ok_or(ClientError::Protocol { what: "upload batch blobs" })?;
        if rows.len() != blobs.len() {
            return Err(ClientError::Protocol { what: "upload batch count" });
        }
        for (row, (expect, _)) in rows.iter().zip(blobs) {
            let got = row
                .get("blob_id")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<BlobId>().ok())
                .ok_or(ClientError::Protocol { what: "upload batch blob_id" })?;
            if got != *expect {
                return Err(ClientError::DigestMismatch {
                    what: "uploaded blob identity",
                    expected: *expect.as_bytes(),
                    found: *got.as_bytes(),
                });
            }
        }
        Ok(blobs.iter().map(|(digest, _)| *digest).collect())
    }

    /// Publish N complete assets in ONE request — one state-thread visit,
    /// one catalog transaction server-side. Blobs must be admitted first
    /// (see [`Self::upload_blob_batch`]); the response is per-item
    /// `(asset_id, revision, already_published)` in request order, each
    /// verified against the locally computed identity.
    pub fn publish_batch(
        &self,
        items: &[PublishBatchWireItem],
    ) -> ClientResult<Vec<(AssetId, AssetRevisionId, bool)>> {
        if items.is_empty() || items.len() > wire::MAX_PUBLISH_BATCH_ITEMS {
            return Err(ClientError::InvalidInput { what: "publish batch size" });
        }
        let labels =
            |v: &[String]| Value::Arr(v.iter().map(|s| json::s(s.clone())).collect());
        let mut rows: Vec<Value> = Vec::with_capacity(items.len());
        for item in items {
            item.annotation.validate()?;
            let ann = &item.annotation;
            let mut ann_pairs: Vec<(&str, Value)> = vec![
                ("title", json::s(ann.title.clone())),
                ("description", json::s(ann.description.clone())),
                ("categories", labels(&ann.categories)),
                ("tags", labels(&ann.tags)),
                ("creator", json::s(ann.creator.clone())),
                ("artist", json::s(ann.artist.clone())),
                ("artist_url", json::s(ann.artist_url.clone())),
                ("album", json::s(ann.album.clone())),
                ("source_url", json::s(ann.source_url.clone())),
                ("license", json::s(ann.license.clone())),
                ("license_url", json::s(ann.license_url.clone())),
                ("generator", json::s(ann.generator.clone())),
                ("backend", json::s(ann.backend.clone())),
                ("model", json::s(ann.model.clone())),
                ("prompt", json::s(ann.prompt.clone())),
                ("provenance", json::s(ann.provenance.clone())),
                (
                    "visibility",
                    json::s(if ann.private { "private" } else { "public" }),
                ),
            ];
            if let Some(kind) = ann.kind {
                ann_pairs.push(("kind", json::s(dto::kind_name(kind))));
            }
            let mut pairs: Vec<(&str, Value)> = vec![
                ("namespace", json::s(item.namespace.clone())),
                ("manifest", json::s(crate::util::to_hex(&item.manifest))),
                ("annotation", json::obj(ann_pairs)),
            ];
            if let Some(alias) = &item.alias {
                pairs.push(("alias", json::s(alias.as_str().to_string())));
            }
            rows.push(json::obj(pairs));
        }
        let body = json::obj(vec![("items", Value::Arr(rows))])
            .to_json()
            .into_bytes();
        let path = wire::path_publish_batch();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200])?;
        let rows = v
            .get("items")
            .and_then(Value::as_arr)
            .ok_or(ClientError::Protocol { what: "publish batch items" })?;
        if rows.len() != items.len() {
            return Err(ClientError::Protocol { what: "publish batch count" });
        }
        let mut out = Vec::with_capacity(rows.len());
        for (row, item) in rows.iter().zip(items) {
            let asset = row
                .get("asset_id")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<AssetId>().ok())
                .ok_or(ClientError::Protocol { what: "publish batch asset_id" })?;
            let revision = row
                .get("revision")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<AssetRevisionId>().ok())
                .ok_or(ClientError::Protocol { what: "publish batch revision" })?;
            // The identities are computable locally; a divergent echo means
            // the answer describes something else.
            let mut hasher = Sha256::new();
            hasher.update(&item.manifest);
            if revision != AssetRevisionId::from_bytes(hasher.finalize()) {
                return Err(ClientError::Protocol { what: "publish batch revision mismatch" });
            }
            let already = row
                .get("already_published")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            out.push((asset, revision, already));
        }
        Ok(out)
    }

    /// Admit a file the SERVER can see, by reference: the store hashes it in
    /// place and catalogues it without copying. The path travels; the bytes
    /// never do, so this is only meaningful when client and store share a
    /// filesystem (an app hosting its own store on loopback).
    ///
    /// Returns the digest and the length the server measured — the caller
    /// did not read the file, so those come from the store, not from a local
    /// guess. `ClientError::NotFound` means the server does not offer
    /// reference admission (policy off, or an older build): fall back to
    /// [`Self::upload_blob`].
    pub fn admit_blob_ref(&self, ns: &str, path: &str) -> ClientResult<BlobRefAdmission> {
        if ns.is_empty() || ns.len() > wire::MAX_NAMESPACE_BYTES || !wire::query_value_ok(ns) {
            return Err(ClientError::InvalidInput { what: "blob ref namespace" });
        }
        if path.is_empty() || path.len() > wire::MAX_BLOB_REF_PATH_BYTES {
            return Err(ClientError::InvalidInput { what: "blob ref path" });
        }
        let body = json::obj(vec![("path", json::s(path.to_string()))])
            .to_json()
            .into_bytes();
        let target = wire::path_blob_ref(ns);
        let mut req = Request::post(&target, &body);
        req.body_content_type = "application/json";
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.data, req, &[200, 201])?;
        let blob = v
            .get("blob_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<BlobId>().ok())
            .ok_or(ClientError::Protocol { what: "blob ref blob_id" })?;
        let size = v
            .get("size")
            .and_then(Value::as_u64)
            .ok_or(ClientError::Protocol { what: "blob ref size" })?;
        Ok(BlobRefAdmission {
            blob,
            size,
            deduped: v.get("deduped").and_then(Value::as_bool).unwrap_or(false),
            owned: v.get("owned").and_then(Value::as_bool).unwrap_or(false),
        })
    }

    /// RE-SCAN one bounded page of the store's reference blobs: for each,
    /// what its file looks like on the server's disk right now.
    ///
    /// This is what makes "we did not copy your library" operable — a UI can
    /// walk it a page at a time and show which originals moved, changed or
    /// vanished, instead of discovering it when a clip refuses to play.
    /// Verifying re-hashes each file server-side, so keep pages small.
    pub fn blob_refs_page(
        &self,
        after: Option<&BlobId>,
        limit: u32,
    ) -> ClientResult<BlobRefsPage> {
        let path = wire::path_blob_refs(after, limit.clamp(1, 256));
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let total = v.get("total").and_then(Value::as_u64).unwrap_or(0);
        let rows = match v.get("refs") {
            Some(Value::Arr(rows)) => rows.clone(),
            _ => return Err(ClientError::Protocol { what: "blob refs list" }),
        };
        if rows.len() > wire::MAX_PAGE_ENTRIES {
            return Err(ClientError::OverBudget {
                what: "blob refs page",
                limit: wire::MAX_PAGE_ENTRIES as u64,
                found: rows.len() as u64,
            });
        }
        let mut refs = Vec::with_capacity(rows.len());
        for row in &rows {
            let blob = row
                .get("blob_id")
                .and_then(Value::as_str)
                .and_then(|s| s.parse::<BlobId>().ok())
                .ok_or(ClientError::Protocol { what: "blob ref id" })?;
            let path = row
                .get("path")
                .and_then(Value::as_str)
                .ok_or(ClientError::Protocol { what: "blob ref path" })?
                .to_string();
            if path.len() > wire::MAX_BLOB_REF_PATH_BYTES {
                return Err(ClientError::Protocol { what: "blob ref path length" });
            }
            refs.push(BlobRefRow {
                blob,
                path,
                size: row.get("size").and_then(Value::as_u64).unwrap_or(0),
                state: row
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                ok: row.get("ok").and_then(Value::as_bool).unwrap_or(false),
            });
        }
        let next = v
            .get("next")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<BlobId>().ok());
        Ok(BlobRefsPage { total, refs, next })
    }

    /// Register an asset id (server-minted when `id` is None). Registering
    /// an id that already exists surfaces the server's 409 as
    /// [`ClientError::Server`]; callers that re-publish an existing asset
    /// treat that as "already registered".
    pub fn register_asset(&self, ns: &str, id: Option<&AssetId>) -> ClientResult<AssetId> {
        let mut pairs: Vec<(&str, Value)> = vec![("namespace", json::s(ns))];
        if let Some(id) = id {
            pairs.push(("asset_id", json::s(id.to_string())));
        }
        let body = json::obj(pairs).to_json().into_bytes();
        let path = wire::path_asset_register();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let got = v
            .get("asset_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<AssetId>().ok())
            .ok_or(ClientError::Protocol { what: "register asset_id" })?;
        if let Some(id) = id {
            if got != *id {
                return Err(ClientError::Protocol { what: "register id mismatch" });
            }
        }
        Ok(got)
    }

    /// Stage canonical manifest bytes as a new revision. The server's echoed
    /// revision must equal the digest of the bytes that were sent.
    pub fn stage_asset_revision(
        &self,
        asset: &AssetId,
        canonical: &[u8],
    ) -> ClientResult<AssetRevisionId> {
        let mut hasher = Sha256::new();
        hasher.update(canonical);
        let local = AssetRevisionId::from_bytes(hasher.finalize());
        let path = wire::path_asset_stage(asset);
        let mut req = Request::post(&path, canonical);
        req.body_content_type = "application/octet-stream";
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let got = v
            .get("revision")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<AssetRevisionId>().ok())
            .ok_or(ClientError::Protocol { what: "stage revision" })?;
        if got != local {
            return Err(ClientError::DigestMismatch {
                what: "staged revision identity",
                expected: *local.as_bytes(),
                found: *got.as_bytes(),
            });
        }
        Ok(local)
    }

    /// Delete an asset from the store: every revision is retired, every
    /// alias head pointing at it drops, its search rows are removed, and its
    /// bytes become collectable by [`Self::gc_blobs`]. Idempotent — a repeat
    /// answers with `already_retired`.
    ///
    /// Requires the moderation capability (`asset_quarantine`) on the
    /// asset's namespace, exactly like pulling content.
    pub fn retire_asset(&self, asset: &AssetId) -> ClientResult<crate::dto::RetireDto> {
        let path = wire::path_asset_retire(asset);
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200])?;
        let dto = crate::dto::parse_retire(&v)?;
        if &dto.asset_id != asset {
            return Err(ClientError::Protocol { what: "retire response mismatch" });
        }
        Ok(dto)
    }

    /// Delete ONE revision (typically a superseded one); the asset stays
    /// live. Idempotent.
    pub fn retire_revision(
        &self,
        asset: &AssetId,
        rev: &AssetRevisionId,
    ) -> ClientResult<crate::dto::RetireDto> {
        let path = wire::path_revision_retire(asset, rev);
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200])?;
        let dto = crate::dto::parse_retire(&v)?;
        if &dto.asset_id != asset || dto.revision.as_ref() != Some(rev) {
            return Err(ClientError::Protocol { what: "retire response mismatch" });
        }
        Ok(dto)
    }

    /// Advance blob garbage collection, starting a run if none is active,
    /// and return the run's durable progress. ONE call does a bounded amount
    /// of work — poll until `done` (the server also finishes runs on its own
    /// janitor, so a caller that stops polling does not strand one).
    ///
    /// `dry_run` counts what would be reclaimed and deletes nothing.
    /// Whole-store admin operation: the bootstrap admin token.
    pub fn gc_blobs(&self, req: &GcRequest) -> ClientResult<crate::dto::GcStatusDto> {
        let mut pairs: Vec<(&str, Value)> = vec![("dry_run", Value::Bool(req.dry_run))];
        if let Some(grace) = req.grace_ms {
            pairs.push(("grace_ms", Value::Int(grace as i64)));
        }
        if let Some(keep) = req.retain_per_asset {
            if keep == 0 {
                return Err(ClientError::InvalidInput { what: "retain_per_asset zero" });
            }
            pairs.push(("retain_per_asset", Value::Int(keep as i64)));
        }
        if let Some(steps) = req.max_steps {
            if steps == 0 {
                return Err(ClientError::InvalidInput { what: "gc max_steps zero" });
            }
            pairs.push(("max_steps", Value::Int(steps as i64)));
        }
        let body = json::obj(pairs).to_json().into_bytes();
        let path = wire::path_gc();
        let mut http = Request::post(&path, &body);
        http.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, http, &[200])?;
        crate::dto::parse_gc_status(&v)
    }

    /// The newest GC run's progress, without starting or advancing one.
    pub fn gc_status(&self) -> ClientResult<crate::dto::GcStatusDto> {
        let path = wire::path_gc();
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        crate::dto::parse_gc_status(&v)
    }

    /// Abandon the active run. Returns whether one was stopped; anything
    /// already collected stays collected.
    pub fn gc_cancel(&self) -> ClientResult<bool> {
        let path = wire::path_gc_cancel();
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200])?;
        v.get("cancelled")
            .and_then(Value::as_bool)
            .ok_or(ClientError::Protocol { what: "gc cancel flag" })
    }

    pub fn publish_asset_revision(
        &self,
        asset: &AssetId,
        rev: &AssetRevisionId,
    ) -> ClientResult<()> {
        let path = wire::path_asset_publish(asset, rev);
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        self.call_json_accept(self.endpoints.control, req, &[200])?;
        Ok(())
    }

    /// Register a game identity in one authorization namespace. Supplying an
    /// id makes retries deterministic; omitting it asks the server to mint
    /// one.
    pub fn register_game(&self, ns: &str, id: Option<&GameId>) -> ClientResult<GameId> {
        let mut pairs: Vec<(&str, Value)> = vec![("namespace", json::s(ns))];
        if let Some(id) = id {
            pairs.push(("game_id", json::s(id.to_string())));
        }
        let body = json::obj(pairs).to_json().into_bytes();
        let path = wire::path_game_register();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let got = v
            .get("game_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<GameId>().ok())
            .ok_or(ClientError::Protocol {
                what: "register game_id",
            })?;
        if let Some(id) = id {
            if got != *id {
                return Err(ClientError::Protocol {
                    what: "register game id mismatch",
                });
            }
        }
        Ok(got)
    }

    /// Stage one canonical game manifest together with the exact canonical
    /// ContentLock bytes it names. The transport framing is deliberately
    /// private to this typed method so callers cannot accidentally drift from
    /// the server's two-document contract.
    pub fn stage_game_revision(
        &self,
        game: &GameId,
        canonical_manifest: &[u8],
        canonical_lock: &[u8],
    ) -> ClientResult<GameRevisionId> {
        for (what, bytes) in [
            ("game manifest bytes", canonical_manifest),
            ("game lock bytes", canonical_lock),
        ] {
            if bytes.is_empty() {
                return Err(ClientError::InvalidInput { what });
            }
            if bytes.len() as u64 > wire::MAX_MANIFEST_RESPONSE_BYTES {
                return Err(ClientError::OverBudget {
                    what,
                    limit: wire::MAX_MANIFEST_RESPONSE_BYTES,
                    found: bytes.len() as u64,
                });
            }
        }
        let capacity = 16usize
            .checked_add(canonical_manifest.len())
            .and_then(|n| n.checked_add(canonical_lock.len()))
            .ok_or(ClientError::OverBudget {
                what: "game revision frame bytes",
                limit: wire::MAX_MANIFEST_RESPONSE_BYTES.saturating_mul(2) + 16,
                found: u64::MAX,
            })?;
        let mut body = Vec::with_capacity(capacity);
        body.extend_from_slice(&(canonical_manifest.len() as u64).to_be_bytes());
        body.extend_from_slice(canonical_manifest);
        body.extend_from_slice(&(canonical_lock.len() as u64).to_be_bytes());
        body.extend_from_slice(canonical_lock);

        let local = GameRevisionId::hash_of(canonical_manifest);
        let path = wire::path_game_stage(game);
        let mut req = Request::post(&path, &body);
        req.body_content_type = "application/octet-stream";
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let got = v
            .get("revision")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<GameRevisionId>().ok())
            .ok_or(ClientError::Protocol {
                what: "stage game revision",
            })?;
        if got != local {
            return Err(ClientError::DigestMismatch {
                what: "staged game revision identity",
                expected: *local.as_bytes(),
                found: *got.as_bytes(),
            });
        }
        Ok(got)
    }

    pub fn publish_game_revision(
        &self,
        game: &GameId,
        rev: &GameRevisionId,
    ) -> ClientResult<()> {
        let path = wire::path_game_publish(game, rev);
        let mut req = Request::post(&path, b"{}");
        req.bearer = self.bearer();
        self.call_json_accept(self.endpoints.control, req, &[200])?;
        Ok(())
    }

    pub fn put_game_alias(
        &self,
        alias: &GameAlias,
        game: &GameId,
        rev: &GameRevisionId,
    ) -> ClientResult<()> {
        let body = json::obj(vec![
            ("game_id", json::s(game.to_string())),
            ("revision", json::s(rev.to_string())),
        ])
        .to_json()
        .into_bytes();
        let path = wire::path_game_alias(alias);
        let mut req = Request::put(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200])?;
        let got_game = v
            .get("game_id")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<GameId>().ok())
            .ok_or(ClientError::Protocol {
                what: "game alias game_id",
            })?;
        let got_rev = v
            .get("head_revision")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<GameRevisionId>().ok())
            .ok_or(ClientError::Protocol {
                what: "game alias head_revision",
            })?;
        if got_game != *game || got_rev != *rev {
            return Err(ClientError::Protocol {
                what: "game alias target mismatch",
            });
        }
        Ok(())
    }

    /// Point an alias head at an exact `{asset, revision}` pair.
    pub fn put_alias(
        &self,
        alias: &AssetAlias,
        asset: &AssetId,
        rev: &AssetRevisionId,
    ) -> ClientResult<()> {
        let body = json::obj(vec![
            ("asset_id", json::s(asset.to_string())),
            ("revision", json::s(rev.to_string())),
        ])
        .to_json()
        .into_bytes();
        let path = wire::path_alias(alias);
        let mut req = Request::put(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[200])?;
        let echoed = v
            .get("head_revision")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<AssetRevisionId>().ok())
            .ok_or(ClientError::Protocol { what: "alias head_revision" })?;
        if echoed != *rev {
            return Err(ClientError::Protocol { what: "alias head mismatch" });
        }
        Ok(())
    }

    /// Write the search annotation (title/kind/categories/tags/prompt…).
    pub fn put_annotation(&self, asset: &AssetId, ann: &AnnotationUpload) -> ClientResult<()> {
        ann.validate()?;
        let labels =
            |v: &[String]| Value::Arr(v.iter().map(|s| json::s(s.clone())).collect());
        let mut pairs: Vec<(&str, Value)> = vec![
            ("title", json::s(ann.title.clone())),
            ("description", json::s(ann.description.clone())),
            ("categories", labels(&ann.categories)),
            ("tags", labels(&ann.tags)),
            ("creator", json::s(ann.creator.clone())),
            ("artist", json::s(ann.artist.clone())),
            ("artist_url", json::s(ann.artist_url.clone())),
            ("album", json::s(ann.album.clone())),
            ("source_url", json::s(ann.source_url.clone())),
            ("license", json::s(ann.license.clone())),
            ("license_url", json::s(ann.license_url.clone())),
            ("generator", json::s(ann.generator.clone())),
            ("backend", json::s(ann.backend.clone())),
            ("model", json::s(ann.model.clone())),
            ("prompt", json::s(ann.prompt.clone())),
            ("provenance", json::s(ann.provenance.clone())),
            (
                "visibility",
                json::s(if ann.private { "private" } else { "public" }),
            ),
        ];
        if let Some(kind) = ann.kind {
            pairs.push(("kind", json::s(dto::kind_name(kind))));
        }
        let body = json::obj(pairs).to_json().into_bytes();
        let path = wire::path_annotation(asset);
        let mut req = Request::put(&path, &body);
        req.bearer = self.bearer();
        req.allow_no_content = true;
        let resp =
            http::http_call_pooled(self.endpoints.control, &req, &self.limits, self.pool())?;
        self.accept(resp, &[200, 204])?;
        Ok(())
    }

    /// Read the annotation record as it stands.
    ///
    /// The vision pass owns two of its fields and must carry the rest
    /// through untouched, and the route is a whole-record PUT — so "carry
    /// through" means read, recompute the owned fields, write the whole
    /// thing back. Without this a worker would need the store's SQLite file
    /// on its own disk, which is exactly the coupling the job queue exists
    /// to remove.
    pub fn get_annotation(&self, asset: &AssetId) -> ClientResult<dto::AnnotationDto> {
        let path = wire::path_annotation(asset);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        dto::parse_annotation(&v)
    }

    /// Counts behind the annotation bar; `category` narrows to one kit.
    /// The published thumbnail sheet of an alias, from the DATA plane.
    ///
    /// One request, bytes as served — no digest to verify against and
    /// nothing to cache: this is the picture a vision model is about to be
    /// shown, and the caller wants exactly what the catalog is publishing
    /// right now.
    pub fn thumbnail_alias_bytes(&self, alias: &AssetAlias) -> ClientResult<Vec<u8>> {
        let path = wire::path_thumbnail_alias(alias);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let resp =
            http::http_call_pooled(self.endpoints.data, &req, &self.limits, self.pool())?;
        let mut resp = self.accept(resp, &[200])?;
        let declared = resp.head().content_length;
        if declared > wire::MAX_THUMBNAIL_BYTES {
            return Err(ClientError::OverBudget {
                what: "thumbnail sheet",
                limit: wire::MAX_THUMBNAIL_BYTES,
                found: declared,
            });
        }
        let mut out = Vec::with_capacity(declared as usize);
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let n = resp.read_chunk(&mut chunk)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&chunk[..n]);
            if out.len() as u64 > wire::MAX_THUMBNAIL_BYTES {
                return Err(ClientError::OverBudget {
                    what: "thumbnail sheet",
                    limit: wire::MAX_THUMBNAIL_BYTES,
                    found: out.len() as u64,
                });
            }
        }
        if out.is_empty() {
            return Err(ClientError::Protocol { what: "empty thumbnail sheet" });
        }
        Ok(out)
    }

    /// Open a blob body stream, optionally resuming from `range_start`.
    /// Returns the raw response (status 200, 206 or 416); the caller owns the
    /// resume math and digest accounting.
    pub fn blob_get(
        &self,
        blob: &BlobId,
        range_start: Option<u64>,
        body_deadline_ms: u64,
    ) -> ClientResult<Response<'_>> {
        let path = wire::path_blob(blob);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        req.body_deadline_ms = Some(body_deadline_ms);
        let etag;
        if let Some(start) = range_start {
            if start == 0 {
                return Err(ClientError::InvalidInput { what: "range start zero" });
            }
            req.range_start = Some(start);
            etag = blob.to_string();
            req.if_range = Some(&etag);
        }
        let resp =
            http::http_call_pooled(self.endpoints.data, &req, &self.limits, self.pool())?;
        self.accept(resp, &[200, 206, 416])
    }

    // ---- typed asset operations --------------------------------------------

    // ---- chat broker -------------------------------------------------------

    // ---- import + immutable derived variants -------------------------------
    //
    // There is no source get-by-id, no import validate, and no variant-set
    // finalize route. Import report/status use the derived import JSON
    // ceiling; every other JSON call stays at `MAX_JSON_RESPONSE_BYTES`.

    /// Register an approved source collection from its canonical bytes.
    /// The server's echoed `digest` must equal the SHA-256 of those bytes
    /// and `source_id` must equal the decoded collection id. Same-digest
    /// retries are idempotent and still answer 201.
    pub fn register_source_collection(
        &self,
        bytes: &[u8],
    ) -> ClientResult<SourceCollectionRegistered> {
        if bytes.is_empty() {
            return Err(ClientError::InvalidInput { what: "source collection bytes" });
        }
        if bytes.len() as u64 > wire::MAX_MANIFEST_RESPONSE_BYTES {
            return Err(ClientError::OverBudget {
                what: "source collection bytes",
                limit: wire::MAX_MANIFEST_RESPONSE_BYTES,
                found: bytes.len() as u64,
            });
        }
        let collection = SourceCollection::from_canonical_bytes(bytes)?;
        let expected = SourceCollectionId::hash_of(bytes);
        let path = wire::path_import_sources();
        let mut req = Request::put(&path, bytes);
        req.body_content_type = "application/octet-stream";
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let source_id = v
            .get("source_id")
            .and_then(Value::as_str)
            .ok_or(ClientError::Protocol { what: "source collection source_id" })?
            .to_string();
        if source_id != collection.id {
            return Err(ClientError::Protocol {
                what: "source collection id mismatch",
            });
        }
        let digest = v
            .get("digest")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<SourceCollectionId>().ok())
            .ok_or(ClientError::Protocol { what: "source collection digest" })?;
        if digest != expected {
            return Err(ClientError::DigestMismatch {
                what: "source collection digest",
                expected: *expected.as_bytes(),
                found: *digest.as_bytes(),
            });
        }
        Ok(SourceCollectionRegistered { source_id, digest })
    }

    /// One explicit source-collection page (`limit` always sent so the
    /// request never uses the legacy no-query listing). `cursor` is the last
    /// `source_id` of the previous page. Limit is `1..=MAX_SOURCE_PAGE_LIMIT`.
    pub fn source_collections_page(
        &self,
        cursor: Option<&str>,
        limit: u64,
    ) -> ClientResult<SourceCollectionsPageDto> {
        if limit == 0 || limit > wire::MAX_SOURCE_PAGE_LIMIT {
            return Err(ClientError::InvalidInput { what: "source page limit" });
        }
        if let Some(c) = cursor {
            if !wire::source_cursor_ok(c) {
                return Err(ClientError::InvalidInput { what: "source page cursor" });
            }
        }
        let path = wire::path_import_sources_page(cursor, limit);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self
            .call_json_limited(
                self.endpoints.control,
                req,
                &[200],
                wire::MAX_SOURCE_PAGE_JSON_RESPONSE_BYTES,
            )?
            .1;
        let page = dto::parse_source_collections_page(&v)?;
        if page.sources.len() as u64 > limit {
            return Err(ClientError::Protocol { what: "source page longer than limit" });
        }
        if page.cursor.is_some() && page.sources.len() as u64 != limit {
            return Err(ClientError::Protocol { what: "source page cursor on short page" });
        }
        if let Some(prev) = cursor {
            if page
                .sources
                .first()
                .is_some_and(|row| row.source_id.as_str() <= prev)
            {
                return Err(ClientError::Protocol { what: "source page does not follow cursor" });
            }
        }
        Ok(page)
    }

    /// Aggregate source collections by walking explicit pages. Fails closed
    /// above [`wire::MAX_PAGE_ENTRIES`] rather than returning a prefix.
    /// There is no get-by-id route for the canonical document.
    pub fn list_source_collections(&self) -> ClientResult<Vec<SourceCollectionRowDto>> {
        let mut acc: Vec<SourceCollectionRowDto> = Vec::new();
        let mut cursor = None;
        loop {
            let page = self.source_collections_page(cursor.as_deref(), wire::MAX_SOURCE_PAGE_LIMIT)?;
            let found = acc.len() as u64 + page.sources.len() as u64;
            if found > wire::MAX_PAGE_ENTRIES as u64 {
                return Err(ClientError::OverBudget {
                    what: "source collections",
                    limit: wire::MAX_PAGE_ENTRIES as u64,
                    found,
                });
            }
            if let (Some(prev), Some(next)) = (acc.last(), page.sources.first()) {
                if next.source_id <= prev.source_id {
                    return Err(ClientError::Protocol { what: "source collection order" });
                }
            }
            let more = page.cursor.is_some();
            acc.extend(page.sources);
            if !more {
                return Ok(acc);
            }
            if acc.len() == wire::MAX_PAGE_ENTRIES {
                return Err(ClientError::OverBudget {
                    what: "source collections",
                    limit: wire::MAX_PAGE_ENTRIES as u64,
                    found: wire::MAX_PAGE_ENTRIES as u64 + 1,
                });
            }
            cursor = page.cursor;
        }
    }

    /// Run (or idempotently replay) one pack import from canonical manifest
    /// bytes. The echoed `import_revision` must be the digest of those
    /// bytes; every entry identity must match the content-contract mapping.
    /// 201 = first commit, 200 = exact-byte replay.
    pub fn run_import(&self, bytes: &[u8]) -> ClientResult<ImportReportDto> {
        if bytes.is_empty() {
            return Err(ClientError::InvalidInput { what: "import manifest bytes" });
        }
        if bytes.len() as u64 > wire::MAX_MANIFEST_RESPONSE_BYTES {
            return Err(ClientError::OverBudget {
                what: "import manifest bytes",
                limit: wire::MAX_MANIFEST_RESPONSE_BYTES,
                found: bytes.len() as u64,
            });
        }
        let manifest = ImportManifest::from_canonical_bytes(bytes)?;
        let expected = ImportRevisionId::hash_of(bytes);
        let path = wire::path_imports();
        let mut req = Request::post(&path, bytes);
        req.body_content_type = "application/octet-stream";
        req.bearer = self.bearer();
        let (status, v) = self.call_json_limited(
            self.endpoints.control,
            req,
            &[200, 201],
            wire::MAX_IMPORT_JSON_RESPONSE_BYTES,
        )?;
        let report = dto::parse_import_report(&v)?;
        if report.import_revision != expected {
            return Err(ClientError::DigestMismatch {
                what: "import revision",
                expected: *expected.as_bytes(),
                found: *report.import_revision.as_bytes(),
            });
        }
        if (status == 201) != report.created {
            return Err(ClientError::Protocol { what: "import created vs status" });
        }
        verify_import_entries(&manifest, &report.import_revision, &report.entries)?;
        Ok(report)
    }

    /// Import status projection for `revision`. The echoed id must equal
    /// the requested one. The route does not return canonical manifest bytes.
    pub fn import_status(&self, revision: &ImportRevisionId) -> ClientResult<ImportStatusDto> {
        let path = wire::path_import(revision);
        let mut req = Request::get(&path);
        req.bearer = self.bearer();
        let v = self
            .call_json_limited(
                self.endpoints.control,
                req,
                &[200],
                wire::MAX_IMPORT_JSON_RESPONSE_BYTES,
            )?
            .1;
        let status = dto::parse_import_status(&v)?;
        if &status.import_revision != revision {
            return Err(ClientError::Protocol { what: "import status id mismatch" });
        }
        Ok(status)
    }

    /// Canonical derived-variant bytes, digest-verified against `id`.
    pub fn fetch_derived_variant_bytes(&self, id: &DerivedVariantId) -> ClientResult<Vec<u8>> {
        let path = wire::path_derived_variant(id);
        self.fetch_canonical_etag(
            &path,
            id.as_bytes(),
            &id.to_string(),
            "derived variant bytes",
        )
    }

    /// Freeze an immutable variant set. The echoed id must equal the
    /// digest of the canonical [`VariantSetManifest`] the request names.
    pub fn freeze_variant_set(
        &self,
        base: &AssetRevisionRef,
        variants: &[DerivedVariantId],
    ) -> ClientResult<VariantSetId> {
        let expected = expected_variant_set_id(base, variants)?;
        let body = json::obj(vec![
            ("base_asset", json::s(base.asset_id.to_string())),
            ("base_revision", json::s(base.revision.to_string())),
            (
                "variants",
                Value::Arr(variants.iter().map(|v| json::s(v.to_string())).collect()),
            ),
        ])
        .to_json()
        .into_bytes();
        let path = wire::path_variant_sets();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json_accept(self.endpoints.control, req, &[201])?;
        let got = v
            .get("variant_set")
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<VariantSetId>().ok())
            .ok_or(ClientError::Protocol { what: "variant set id" })?;
        if got != expected {
            return Err(ClientError::DigestMismatch {
                what: "variant set identity",
                expected: *expected.as_bytes(),
                found: *got.as_bytes(),
            });
        }
        Ok(expected)
    }

    /// Canonical variant-set bytes, digest-verified against `id`.
    pub fn fetch_variant_set_bytes(&self, id: &VariantSetId) -> ClientResult<Vec<u8>> {
        let path = wire::path_variant_set(id);
        self.fetch_canonical_etag(
            &path,
            id.as_bytes(),
            &id.to_string(),
            "variant set bytes",
        )
    }

    /// Deterministic server-side resolution of one frozen set. The response
    /// is reconstructed as a canonical [`ResolvedVariantMap`] and refused
    /// unless its digest, set id, and profile digest match.
    pub fn resolve_variant_set(
        &self,
        set: &VariantSetId,
        profile: &ClientProfile,
    ) -> ClientResult<ResolvedVariantMap> {
        let expected_profile = profile.digest()?;
        let body = json::obj(vec![
            ("variant_set", json::s(set.to_string())),
            ("profile", profile_json(profile)?),
        ])
        .to_json()
        .into_bytes();
        let path = wire::path_variant_resolutions();
        let mut req = Request::post(&path, &body);
        req.bearer = self.bearer();
        let v = self.call_json(self.endpoints.control, req)?;
        let dto = dto::parse_resolved_variant_map(&v)?;
        if &dto.set != set {
            return Err(ClientError::Protocol { what: "resolution variant_set mismatch" });
        }
        if dto.profile != expected_profile {
            return Err(ClientError::DigestMismatch {
                what: "resolution profile digest",
                expected: *expected_profile.as_bytes(),
                found: *dto.profile.as_bytes(),
            });
        }
        let map = ResolvedVariantMap {
            set: dto.set,
            profile: dto.profile,
            entries: dto.entries,
        };
        map.validate()?;
        let digest = map.digest()?;
        if digest != dto.digest {
            return Err(ClientError::DigestMismatch {
                what: "resolution map digest",
                expected: *digest.as_bytes(),
                found: *dto.digest.as_bytes(),
            });
        }
        Ok(map)
    }

    fn fetch_canonical_etag(
        &self,
        path: &str,
        expected: &[u8; 32],
        etag: &str,
        what: &'static str,
    ) -> ClientResult<Vec<u8>> {
        let mut req = Request::get(path);
        req.bearer = self.bearer();
        let resp =
            http::http_call_pooled(self.endpoints.control, &req, &self.limits, self.pool())?;
        let resp = self.accept(resp, &[200])?;
        if resp.head().etag.as_deref() != Some(etag) {
            return Err(ClientError::Protocol { what: "canonical etag mismatch" });
        }
        if resp.head().content_length > wire::MAX_MANIFEST_RESPONSE_BYTES {
            return Err(ClientError::OverBudget {
                what,
                limit: wire::MAX_MANIFEST_RESPONSE_BYTES,
                found: resp.head().content_length,
            });
        }
        let bytes = resp.read_full(wire::MAX_MANIFEST_RESPONSE_BYTES)?;
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        let found = hasher.finalize();
        if &found != expected {
            return Err(ClientError::DigestMismatch { what, expected: *expected, found });
        }
        Ok(bytes)
    }
}

fn verify_import_entries(
    manifest: &ImportManifest,
    irev: &ImportRevisionId,
    entries: &[crate::dto::ImportEntryDto],
) -> ClientResult<()> {
    if entries.len() != manifest.assets.len() {
        return Err(ClientError::Protocol { what: "import entry count" });
    }
    let mut seen = std::collections::HashSet::new();
    for entry in entries {
        if !seen.insert(&entry.key) {
            return Err(ClientError::Protocol { what: "import entry key duplicate" });
        }
        let asset = manifest
            .assets
            .iter()
            .find(|a| a.key == entry.key)
            .ok_or(ClientError::Protocol { what: "import entry key" })?;
        let expected_asset = manifest.asset_id_for(&asset.key);
        if entry.asset_id != expected_asset {
            return Err(ClientError::Protocol { what: "import entry asset_id mismatch" });
        }
        let produced = manifest.asset_manifest_for(asset, irev)?;
        let expected_rev = produced.revision()?;
        if entry.revision != expected_rev {
            return Err(ClientError::DigestMismatch {
                what: "import entry revision",
                expected: *expected_rev.as_bytes(),
                found: *entry.revision.as_bytes(),
            });
        }
        let expected_alias = manifest.alias_for(&asset.key)?;
        match &entry.alias {
            Some(alias) if alias == &expected_alias => {}
            _ => return Err(ClientError::Protocol { what: "import entry alias mismatch" }),
        }
    }
    Ok(())
}

fn expected_variant_set_id(
    base: &AssetRevisionRef,
    variants: &[DerivedVariantId],
) -> ClientResult<VariantSetId> {
    if variants.is_empty() {
        return Err(ClientError::InvalidInput { what: "variant set variants" });
    }
    if variants.len() > makepad_asset_data::limits::MAX_VARIANTS_PER_SET {
        return Err(ClientError::InvalidInput { what: "variant set variants" });
    }
    let mut set = VariantSetManifest {
        base: *base,
        variants: variants.to_vec(),
        policy_version: RESOLUTION_POLICY_V1,
    };
    set.canonicalize();
    Ok(set.id()?)
}

fn profile_json(profile: &ClientProfile) -> ClientResult<Value> {
    profile.validate()?;
    if profile.max_variant_bytes > i64::MAX as u64 {
        return Err(ClientError::InvalidInput {
            what: "profile max_variant_bytes",
        });
    }
    let mut accept = Vec::new();
    if profile.accept_png {
        accept.push(json::s("png"));
    }
    if profile.accept_jpeg {
        accept.push(json::s("jpeg"));
    }
    if profile.accept_glb {
        accept.push(json::s("glb"));
    }
    if profile.accept_bin {
        accept.push(json::s("bin"));
    }
    Ok(json::obj(vec![
        ("policy_version", Value::Int(profile.policy_version as i64)),
        ("tier", json::s(dto::tier_name(profile.tier))),
        ("max_texture_dim", Value::Int(profile.max_texture_dim as i64)),
        ("max_triangles", Value::Int(profile.max_triangles as i64)),
        (
            "max_variant_bytes",
            Value::Int(profile.max_variant_bytes as i64),
        ),
        ("accept", Value::Arr(accept)),
    ]))
}

/// A cursor about to be SENT must still be shaped like the cursors this
/// client accepts from responses (defense in depth around callers that
/// persist cursors).
fn check_cursor_out(c: &str) -> ClientResult<()> {
    if c.is_empty() || c.len() > wire::MAX_CURSOR_BYTES || !wire::query_value_ok(c) {
        return Err(ClientError::InvalidInput { what: "cursor shape" });
    }
    Ok(())
}

fn validate_preview_session(value: &str) -> ClientResult<()> {
    let valid = !value.is_empty()
        && value.len() <= 64
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'_')
        });
    if valid {
        Ok(())
    } else {
        Err(ClientError::InvalidInput { what: "model preview session" })
    }
}

fn validate_preview_part(value: &str) -> ClientResult<()> {
    let valid = !value.is_empty()
        && value.len() <= 24
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    if valid {
        Ok(())
    } else {
        Err(ClientError::InvalidInput { what: "model preview part" })
    }
}

fn validate_preview_mesh_token(value: &str) -> ClientResult<()> {
    let valid = value.strip_prefix("pmesh_").is_some_and(|hex| {
        hex.len() == 64
            && hex
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    });
    if valid {
        Ok(())
    } else {
        Err(ClientError::Protocol { what: "model preview mesh token" })
    }
}

// ---- pipelines: declaring the stages ---------------------------------------

/// Declared weight per job kind, in ONE place so two clients drawing the
/// same run cannot disagree about how long its stages are worth.
///
/// These are ESTIMATES, and honest about it: the bar means "weighted share
/// of DECLARED work completed", never "time left". A misdeclared weight
/// makes the aggregate crawl then sprint; it stays monotone and
/// stage-truthful either way, and the note is what says what is happening
/// now.
pub const DEFAULT_STAGE_WEIGHTS: &[(&str, u16)] = &[
    ("text.expand", 5),
    ("image.generate", 15),
    ("image.upscale", 25),
    ("video.generate", 70),
    ("video.enhance", 25),
    ("music.generate", 60),
    ("mesh.generate", 40),
];

/// What a kind with no declared weight is worth. Matches the server's own
/// neutral fallback, so an undeclared stage weighs the same on both sides.
pub const NEUTRAL_STAGE_WEIGHT: u16 = 10;

/// The declared weight for one job kind: [`DEFAULT_STAGE_WEIGHTS`], else
/// [`NEUTRAL_STAGE_WEIGHT`].
pub fn default_stage_weight(kind: &str) -> u16 {
    DEFAULT_STAGE_WEIGHTS
        .iter()
        .find(|(k, _)| *k == kind)
        .map(|(_, w)| *w)
        .unwrap_or(NEUTRAL_STAGE_WEIGHT)
}

/// A reference to a field of an EARLIER stage's result:
/// `{"$from_stage": "<stage>", "field": "<field>"}`.
///
/// Put one anywhere inside a stage body, at any nesting depth. The server
/// rewrites it to the job-id form at create, and the value is spliced in AT
/// CLAIM — the first moment every dependency has provably succeeded — so a
/// chain can be declared up front and nobody has to stay alive to carry a
/// result from one stage to the next.
pub fn stage_ref(stage: &str, field: &str) -> Value {
    json::obj(vec![("$from_stage", json::s(stage)), ("field", json::s(field))])
}

/// One declared stage of a pipeline: what to run, with what body, how much
/// of the bar it is worth, what it waits for, and what its failure means.
#[derive(Clone, Debug, PartialEq)]
pub struct PipelineStageSpec {
    /// `[a-z0-9._-]`, 1..=64 bytes, unique within the pipeline. This is the
    /// name the chip strip shows and the name [`stage_ref`] resolves.
    pub name: String,
    /// The job kind, e.g. `image.generate`.
    pub kind: String,
    /// The job body, an object; may carry [`stage_ref`] references.
    pub body: Value,
    /// `1..=1000`. `None` takes [`default_stage_weight`] for the kind.
    pub weight: Option<u16>,
    /// `Skip` keeps a run alive when this stage fails.
    pub on_fail: StageOnFailDto,
    /// Stage NAMES this stage waits for. `None` — the common shape — waits
    /// for the stage declared immediately before it; `Some(vec![])` declares
    /// a stage that waits for nothing.
    pub deps: Option<Vec<String>>,
    pub priority: i64,
    /// `None` takes the server default of one attempt.
    pub max_attempts: Option<u32>,
}

impl PipelineStageSpec {
    /// A stage with the defaults: the kind's declared weight, fail-the-run
    /// on failure, waiting for the stage before it, one attempt.
    pub fn new(name: impl Into<String>, kind: impl Into<String>, body: Value) -> Self {
        Self {
            name: name.into(),
            kind: kind.into(),
            body,
            weight: None,
            on_fail: StageOnFailDto::Fail,
            deps: None,
            priority: 0,
            max_attempts: None,
        }
    }

    /// This stage keeps the run alive when it fails: its dependents' spliced
    /// references are rewritten to the pipeline's prompt and the edge is
    /// detached. The never-lose-a-run law, structurally.
    pub fn on_fail_skip(mut self) -> Self {
        self.on_fail = StageOnFailDto::Skip;
        self
    }

    pub fn with_weight(mut self, weight: u16) -> Self {
        self.weight = Some(weight);
        self
    }

    pub fn with_deps(mut self, deps: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.deps = Some(deps.into_iter().map(Into::into).collect());
        self
    }

    pub fn with_max_attempts(mut self, attempts: u32) -> Self {
        self.max_attempts = Some(attempts);
        self
    }

    /// The weight this stage will be declared with.
    pub fn weight(&self) -> u16 {
        self.weight.unwrap_or_else(|| default_stage_weight(&self.kind))
    }

    /// The wire document for this stage, or the local refusal that keeps a
    /// malformed declaration from becoming a round trip.
    pub fn to_value(&self) -> ClientResult<Value> {
        if !stage_token_ok(&self.name) {
            return Err(ClientError::InvalidInput { what: "stage name" });
        }
        if !stage_token_ok(&self.kind) {
            return Err(ClientError::InvalidInput { what: "stage kind" });
        }
        if !matches!(self.body, Value::Obj(_)) {
            return Err(ClientError::InvalidInput { what: "stage body must be an object" });
        }
        let weight = self.weight();
        if weight == 0 || weight > wire::MAX_STAGE_WEIGHT {
            return Err(ClientError::InvalidInput { what: "stage weight" });
        }
        let mut pairs = vec![
            ("name", json::s(self.name.clone())),
            ("kind", json::s(self.kind.clone())),
            ("body", self.body.clone()),
            ("weight", Value::Int(weight as i64)),
            ("on_fail", json::s(self.on_fail.as_str())),
        ];
        if let Some(deps) = &self.deps {
            pairs.push((
                "deps",
                Value::Arr(deps.iter().map(|d| json::s(d.clone())).collect()),
            ));
        }
        if self.priority != 0 {
            pairs.push(("priority", Value::Int(self.priority)));
        }
        if let Some(attempts) = self.max_attempts {
            if attempts == 0 {
                return Err(ClientError::InvalidInput { what: "stage max_attempts" });
            }
            pairs.push(("max_attempts", Value::Int(attempts as i64)));
        }
        Ok(json::obj(pairs))
    }
}

/// Stage names and job kinds share the job contract's `[a-z0-9._-]`,
/// 1..=64-byte vocabulary.
fn stage_token_ok(t: &str) -> bool {
    (1..=64).contains(&t.len())
        && t.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'.' | b'-' | b'_'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_validation() {
        assert!(CatalogQuery::browse(0).validate().is_err());
        assert!(CatalogQuery::browse(MAX_SEARCH_LIMIT + 1).validate().is_err());
        assert!(CatalogQuery::text("rocket", 10).validate().is_ok());
        assert!(CatalogQuery::text("bad\u{7}", 10).validate().is_err());
        let long = "x".repeat(wire::MAX_QUERY_TEXT_BYTES + 1);
        assert!(CatalogQuery::text(long, 10).validate().is_err());
        let mut q = CatalogQuery::browse(10);
        q.namespace = Some(String::new());
        assert!(q.validate().is_err());
        // `exclude_tag` is bounded exactly like the other filter values.
        let mut q = CatalogQuery::browse(10);
        q.exclude_tag = Some(String::new());
        assert!(q.validate().is_err());
        q.exclude_tag = Some("bad\u{7}".into());
        assert!(q.validate().is_err());
        q.exclude_tag = Some("x".repeat(wire::MAX_FILTER_VALUE_BYTES + 1));
        assert!(q.validate().is_err());
        q.exclude_tag = Some("intermediate".into());
        assert!(q.validate().is_ok());
    }

    #[test]
    fn query_body_shape() {
        let mut q = CatalogQuery::text("fancy rocket", 25);
        q.kind = Some(AssetKind::Mesh);
        q.live_only = true;
        let body = q.body(Some("ab12"));
        assert_eq!(body.get("q").unwrap().as_str(), Some("fancy rocket"));
        assert_eq!(body.get("kind").unwrap().as_str(), Some("mesh"));
        assert_eq!(body.get("live").unwrap().as_bool(), Some(true));
        assert_eq!(body.get("limit").unwrap().as_i64(), Some(25));
        assert_eq!(body.get("cursor").unwrap().as_str(), Some("ab12"));
        assert!(q.body(None).get("cursor").is_none());
        // Absent filters emit no key at all; set ones travel verbatim.
        assert!(body.get("tag").is_none());
        assert!(body.get("exclude_tag").is_none());
        q.tag = Some("keep".into());
        q.exclude_tag = Some("intermediate".into());
        let body = q.body(None);
        assert_eq!(body.get("tag").unwrap().as_str(), Some("keep"));
        assert_eq!(body.get("exclude_tag").unwrap().as_str(), Some("intermediate"));
    }

    #[test]
    fn token_shape_enforced_at_construction() {
        let endpoints = ApiEndpoints {
            control: "127.0.0.1:1".parse().unwrap(),
            data: "127.0.0.1:2".parse().unwrap(),
        };
        assert!(Api::new(endpoints, HttpLimits::default_v1(), Some("garbage".into())).is_err());
        let good = format!("mpat_{}", "ab".repeat(32));
        assert!(Api::new(endpoints, HttpLimits::default_v1(), Some(good)).is_ok());
        assert!(Api::new(endpoints, HttpLimits::default_v1(), None).is_ok());
    }

    #[test]
    fn outgoing_cursor_shape() {
        assert!(check_cursor_out("abc123").is_ok());
        assert!(check_cursor_out("").is_err());
        assert!(check_cursor_out("a b").is_err());
        let long = "a".repeat(wire::MAX_CURSOR_BYTES + 1);
        assert!(check_cursor_out(&long).is_err());
    }

    #[test]
    fn source_page_limit_is_local() {
        let endpoints = ApiEndpoints {
            control: "127.0.0.1:1".parse().unwrap(),
            data: "127.0.0.1:2".parse().unwrap(),
        };
        let api = Api::new(endpoints, HttpLimits::default_v1(), None).unwrap();
        let start = makepad_platform::Cx::monotonic_now();
        match api.source_collections_page(None, 501) {
            Err(ClientError::InvalidInput { what }) => assert_eq!(what, "source page limit"),
            other => panic!("501 must refuse locally, got {other:?}"),
        }
        match api.source_collections_page(None, 0) {
            Err(ClientError::InvalidInput { what }) => assert_eq!(what, "source page limit"),
            other => panic!("0 must refuse locally, got {other:?}"),
        }
        match api.source_collections_page(Some("Kenney"), 10) {
            Err(ClientError::InvalidInput { what }) => assert_eq!(what, "source page cursor"),
            other => panic!("bad cursor must refuse locally, got {other:?}"),
        }
        assert!(
            makepad_platform::Cx::monotonic_now() - start < 0.05,
            "must not touch the network"
        );
    }

    #[test]
    fn profile_json_refuses_unrepresentable_max_variant_bytes_without_io() {
        let ok = ClientProfile {
            policy_version: RESOLUTION_POLICY_V1,
            tier: DeviceTier::High,
            max_texture_dim: 2048,
            max_triangles: 1_000_000,
            max_variant_bytes: i64::MAX as u64,
            accept_png: true,
            accept_jpeg: false,
            accept_glb: true,
            accept_bin: false,
        };
        assert!(profile_json(&ok).is_ok());
        let mut too_big = ok;
        too_big.max_variant_bytes = (i64::MAX as u64) + 1;
        match profile_json(&too_big) {
            Err(ClientError::InvalidInput { what }) => {
                assert_eq!(what, "profile max_variant_bytes")
            }
            other => panic!("expected local InvalidInput, got {other:?}"),
        }
        let endpoints = ApiEndpoints {
            control: "127.0.0.1:1".parse().unwrap(),
            data: "127.0.0.1:2".parse().unwrap(),
        };
        let api = Api::new(endpoints, HttpLimits::default_v1(), None).unwrap();
        let start = makepad_platform::Cx::monotonic_now();
        match api.resolve_variant_set(&VariantSetId::from_bytes([1; 32]), &too_big) {
            Err(ClientError::InvalidInput { what }) => {
                assert_eq!(what, "profile max_variant_bytes")
            }
            other => panic!("must refuse locally, got {other:?}"),
        }
        assert!(
            makepad_platform::Cx::monotonic_now() - start < 0.05,
            "must not touch the network"
        );
    }
}
