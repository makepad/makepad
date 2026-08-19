//! The connected client: one verified server + one local verified cache.
//!
//! [`AssetClient::connect`] is the only way to get one, and it fail-closes on
//! every trust step: the endpoint must answer `/v1/health` with a parseable
//! identity, that identity must match what the caller expected (a discovery
//! beacon or a pinned config), the protocol version must be one this client
//! speaks, and a credentialed listing probe must succeed — so an
//! `AssetClient` in hand means browsing and fetching are actually authorized,
//! not merely hoped for.
//!
//! Fetches are cache-first and verify on every path: manifests decode via the
//! content contract only after their bytes hash to the requested revision;
//! blobs stream into a resumable partial file and only an exact digest match
//! commits them. Pagination cursors are stamped with the server identity so a
//! cursor can never silently continue on a different server.

use crate::api::{
    AnnotationUpload, Api, ApiEndpoints, BatchFlow, BatchFrame, BatchItem, BlobHead, CatalogQuery,
    SourceCollectionRegistered,
};
use crate::cache::{CacheBudgets, CacheStats, ContentCache};
use crate::discovery::{content_client_caps, DiscoveredServer};
use crate::dto::{
    AliasDto, AssetDetailDto, AssetRow, CatalogEventDto, CatalogHit, ImportReportDto,
    ImportStatusDto, SourceCollectionRowDto,
};
use crate::error::{ClientError, ClientResult};
use crate::http::HttpLimits;
use crate::util::now_ms;
use crate::wire;
use makepad_asset_data::{
    AssetManifest, AssetRevisionId, AssetRevisionRef, BlobId, ClientProfile, ContentLock,
    DerivedVariantId, DerivedVariantManifest, GameAlias, GameId, GameRevisionId,
    GameRevisionManifest, ImportRevisionId, ResolvedVariantMap, VariantSetId, VariantSetManifest,
};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct ClientConfig {
    pub cache_root: PathBuf,
    pub cache: CacheBudgets,
    pub http: HttpLimits,
    /// Bearer token (`mpat_<64 hex>`); validated at connect.
    pub token: Option<String>,
    /// Reuse connections (HTTP keep-alive). On by default: against a
    /// server on localhost a connect per request costs more than the
    /// payload of a thumbnail. Turned off only to measure that, or to face
    /// a middlebox that mangles persistent connections.
    pub http_keep_alive: bool,
    /// Whole-transfer attempts per blob (first try + resumes).
    pub max_transfer_attempts: u32,
    /// Wall-clock budget for ONE blob transfer attempt.
    pub blob_body_deadline_ms: u64,
}

impl ClientConfig {
    pub fn new(cache_root: impl Into<PathBuf>) -> Self {
        Self {
            cache_root: cache_root.into(),
            cache: CacheBudgets::default_v1(),
            http: HttpLimits::default_v1(),
            token: None,
            http_keep_alive: true,
            max_transfer_attempts: 4,
            blob_body_deadline_ms: 600_000,
        }
    }

    fn validate(&self) -> ClientResult<()> {
        self.cache.validate()?;
        self.http.validate()?;
        if self.max_transfer_attempts == 0 || self.blob_body_deadline_ms == 0 {
            return Err(ClientError::InvalidInput { what: "transfer attempts/deadline" });
        }
        Ok(())
    }
}

/// A pagination cursor bound to the server that minted it. Constructed only
/// by this module; using it against another server refuses with
/// [`ClientError::WrongServerCursor`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageCursor {
    server_id: [u8; 16],
    token: String,
}

impl PageCursor {
    pub fn server_id(&self) -> &[u8; 16] {
        &self.server_id
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogPage {
    pub hits: Vec<CatalogHit>,
    pub total: u64,
    pub next: Option<PageCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetsPage {
    pub assets: Vec<AssetRow>,
    pub next: Option<PageCursor>,
}

/// Opaque event-feed resume position, pinned to the server that minted it.
/// Keeping this separate from [`PageCursor`] prevents accidentally feeding a
/// search cursor to the long-poll endpoint (or vice versa).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEventCursor {
    pub(crate) server_id: [u8; 16],
    pub(crate) token: String,
}

impl CatalogEventCursor {
    pub fn server_id(&self) -> &[u8; 16] {
        &self.server_id
    }

    pub(crate) fn token(&self) -> &str {
        &self.token
    }
}

/// Source-collection browse cursor, pinned to the server that minted it.
/// Distinct from [`PageCursor`] so a search cursor cannot be fed to
/// `/v1/import-sources`. Using it against another server refuses with
/// [`ClientError::WrongServerCursor`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCollectionsCursor {
    server_id: [u8; 16],
    token: String,
}

impl SourceCollectionsCursor {
    pub fn server_id(&self) -> &[u8; 16] {
        &self.server_id
    }
}

/// One explicit source-collection page from a connected client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceCollectionsPage {
    pub sources: Vec<SourceCollectionRowDto>,
    pub next: Option<SourceCollectionsCursor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CatalogEventsPage {
    pub events: Vec<CatalogEventDto>,
    /// Cursor covering all events scanned by the server, including events
    /// excluded by the optional content-kind filter.
    pub next: CatalogEventCursor,
    /// True means the previous cursor fell outside retention (or the server
    /// restarted); callers must rebuild their catalog projection.
    pub gap: bool,
}

pub struct AssetClient {
    /// Stateless: endpoints, limits and the bearer token. Every call opens
    /// its own connection, which is why lane clones need no session at all.
    api: Api,
    /// The verified local cache, SHARED with every lane clone of this client
    /// (see [`AssetClient::lane_clone`]). A cache root is single-owner — the
    /// cache holds an exclusive lock on `cache.lock` — so parallel workers
    /// share ONE instance behind a mutex instead of opening a second one.
    /// The lock is only ever held for cache bookkeeping, never across a
    /// network transfer.
    cache: Arc<Mutex<ContentCache>>,
    /// Verified blobs kept in process memory, shared with lane clones. The
    /// end-application streams here; the server already owns the disk cache.
    ram: Arc<Mutex<HashMap<[u8; 32], Vec<u8>>>>,
    /// Serialises transfers of the SAME digest across lanes: two workers
    /// appending to one resumable partial would interleave bytes. It also
    /// coalesces duplicate fetches — the second waiter finds the committed
    /// object instead of downloading it again.
    transfers: Arc<TransferGate>,
    /// Cached copy of the cache root so diagnostics need no lock.
    cache_root: PathBuf,
    server_id: [u8; 16],
    protocol_version: u16,
    max_transfer_attempts: u32,
    blob_body_deadline_ms: u64,
}

/// Per-digest exclusion for resumable blob transfers.
struct TransferGate {
    in_flight: Mutex<HashSet<[u8; 32]>>,
    wake: Condvar,
}

/// Slice of the wait on a busy digest; bounds how long a cancelled request
/// can sit behind another lane's transfer.
const TRANSFER_WAIT_SLICE_MS: u64 = 50;

impl TransferGate {
    fn new() -> TransferGate {
        TransferGate { in_flight: Mutex::new(HashSet::new()), wake: Condvar::new() }
    }

    /// Claim exclusive right to transfer `digest`, waiting while another
    /// worker holds it. Returns `None` when `abort` fired while waiting, so
    /// a cancelled request never waits out someone else's big download.
    fn claim(&self, digest: [u8; 32], abort: &dyn Fn() -> bool) -> Option<TransferClaim<'_>> {
        let mut held = self.in_flight.lock().expect("transfer gate");
        while held.contains(&digest) {
            if abort() {
                return None;
            }
            let (guard, _) = self
                .wake
                .wait_timeout(held, Duration::from_millis(TRANSFER_WAIT_SLICE_MS))
                .expect("transfer gate wait");
            held = guard;
        }
        held.insert(digest);
        Some(TransferClaim { gate: self, digest })
    }
}

struct TransferClaim<'a> {
    gate: &'a TransferGate,
    digest: [u8; 32],
}

impl Drop for TransferClaim<'_> {
    fn drop(&mut self) {
        self.gate.in_flight.lock().expect("transfer gate").remove(&self.digest);
        self.gate.wake.notify_all();
    }
}

impl std::fmt::Debug for AssetClient {
    /// Endpoints and identity only — never the bearer token.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AssetClient")
            .field("server_id", &crate::util::to_hex(&self.server_id))
            .field("control", &self.api.endpoints.control)
            .field("data", &self.api.endpoints.data)
            .finish_non_exhaustive()
    }
}

impl AssetClient {
    /// Connect to explicit endpoints. `expected_server` pins the identity the
    /// health response must present (pass the beacon's or a stored id; `None`
    /// accepts whichever identity answers and returns it for pinning next
    /// time).
    pub fn connect(
        config: ClientConfig,
        endpoints: ApiEndpoints,
        expected_server: Option<[u8; 16]>,
    ) -> ClientResult<AssetClient> {
        config.validate()?;
        let cache = ContentCache::open(&config.cache_root, config.cache, now_ms())?;
        let api = Api::with_keep_alive(
            endpoints,
            config.http,
            config.token.clone(),
            config.http_keep_alive,
        )?;

        let health = api.health()?;
        if let Some(expected) = expected_server {
            if expected != health.server_id {
                return Err(ClientError::ServerIdentityMismatch {
                    expected,
                    found: health.server_id,
                });
            }
        }
        if health.protocol_version != wire::PROTOCOL_VERSION {
            return Err(ClientError::Protocol { what: "server protocol version unsupported" });
        }
        // Credential probe: one authenticated read. An auth-required server
        // answers 401 here and connect refuses instead of every later call
        // failing one by one.
        api.assets_page(None, None, 1)?;

        Ok(AssetClient {
            api,
            cache_root: config.cache_root.clone(),
            cache: Arc::new(Mutex::new(cache)),
            ram: Arc::new(Mutex::new(HashMap::new())),
            transfers: Arc::new(TransferGate::new()),
            server_id: health.server_id,
            protocol_version: health.protocol_version,
            max_transfer_attempts: config.max_transfer_attempts,
            blob_body_deadline_ms: config.blob_body_deadline_ms,
        })
    }

    /// Connect to a discovery candidate: refuses TLS-only servers (this
    /// client does not speak TLS), refuses candidates missing catalog+blob
    /// capability, refuses auth-required candidates when no token is
    /// configured, and pins the beacon's server id through the health check.
    pub fn connect_discovered(
        config: ClientConfig,
        server: &DiscoveredServer,
    ) -> ClientResult<AssetClient> {
        if server.tls {
            return Err(ClientError::TlsUnsupported);
        }
        if !server.usable(content_client_caps()) {
            return Err(ClientError::InvalidInput { what: "discovered server unusable" });
        }
        if server.auth_required && config.token.is_none() {
            return Err(ClientError::Unauthenticated);
        }
        let endpoints = ApiEndpoints {
            control: server.control_endpoint(),
            data: server.data_endpoint(),
        };
        Self::connect(config, endpoints, Some(server.server_id))
    }

    pub fn server_id(&self) -> [u8; 16] {
        self.server_id
    }

    pub fn protocol_version(&self) -> u16 {
        self.protocol_version
    }

    pub fn endpoints(&self) -> ApiEndpoints {
        self.api.endpoints
    }

    pub fn cache_stats(&self) -> CacheStats {
        self.cache().stats()
    }

    /// Another handle on the SAME verified server and the SAME cache, for a
    /// second execution lane (see [`crate::runtime::ClientRuntime`]).
    ///
    /// No network and no re-verification: identity was proven by
    /// [`AssetClient::connect`], and `Api` carries no session — endpoints,
    /// limits and the bearer token only, with one connection per call. What
    /// IS shared is the single-owner cache, its in-memory blob map, and the
    /// per-digest transfer gate, so two lanes can never fight over one
    /// partial file or open a second cache on the same root.
    pub fn lane_clone(&self) -> AssetClient {
        AssetClient {
            api: self.api.clone(),
            cache: self.cache.clone(),
            ram: self.ram.clone(),
            transfers: self.transfers.clone(),
            cache_root: self.cache_root.clone(),
            server_id: self.server_id,
            protocol_version: self.protocol_version,
            max_transfer_attempts: self.max_transfer_attempts,
            blob_body_deadline_ms: self.blob_body_deadline_ms,
        }
    }

    /// The shared cache. Held for bookkeeping only — never across a network
    /// call, which is what keeps lanes actually parallel. A poisoned lock
    /// means another worker panicked mid-bookkeeping; that is not a state
    /// this client can reason about, so it propagates.
    fn cache(&self) -> MutexGuard<'_, ContentCache> {
        self.cache.lock().expect("content cache lock")
    }

    // ---- pinning -----------------------------------------------------------

    pub fn pin_blob(&mut self, blob: &BlobId) -> ClientResult<()> {
        self.cache().pin(blob.as_bytes())
    }

    pub fn unpin_blob(&mut self, blob: &BlobId) -> ClientResult<()> {
        self.cache().unpin(blob.as_bytes())
    }

    pub fn pin_manifest(&mut self, rev: &AssetRevisionId) -> ClientResult<()> {
        self.cache().pin(rev.as_bytes())
    }

    pub fn unpin_manifest(&mut self, rev: &AssetRevisionId) -> ClientResult<()> {
        self.cache().unpin(rev.as_bytes())
    }

    // ---- browse ------------------------------------------------------------

    pub fn catalog_search(
        &self,
        query: &CatalogQuery,
        cursor: Option<&PageCursor>,
    ) -> ClientResult<CatalogPage> {
        let raw = self.unwrap_cursor(cursor)?;
        let page = self.api.catalog_search(query, raw)?;
        Ok(CatalogPage {
            hits: page.hits,
            total: page.total,
            next: self.wrap_cursor(page.cursor),
        })
    }

    pub fn assets_page(
        &self,
        namespace: Option<&str>,
        cursor: Option<&PageCursor>,
        limit: u64,
    ) -> ClientResult<AssetsPage> {
        let raw = self.unwrap_cursor(cursor)?;
        let page = self.api.assets_page(namespace, raw, limit)?;
        Ok(AssetsPage { assets: page.assets, next: self.wrap_cursor(page.cursor) })
    }

    pub fn asset_detail(&self, id: &makepad_asset_data::AssetId) -> ClientResult<AssetDetailDto> {
        self.api.asset_detail(id)
    }

    pub fn resolve_alias(&self, alias: &makepad_asset_data::AssetAlias) -> ClientResult<AliasDto> {
        self.api.resolve_alias(alias)
    }

    // ---- deletion ----------------------------------------------------------

    /// Delete an asset from the store (see [`crate::api::Api::retire_asset`]).
    pub fn retire_asset(
        &self,
        id: &makepad_asset_data::AssetId,
    ) -> ClientResult<crate::dto::RetireDto> {
        self.api.retire_asset(id)
    }

    /// Delete one revision; the asset stays live.
    pub fn retire_revision(
        &self,
        id: &makepad_asset_data::AssetId,
        rev: &makepad_asset_data::AssetRevisionId,
    ) -> ClientResult<crate::dto::RetireDto> {
        self.api.retire_revision(id, rev)
    }

    /// Advance blob garbage collection by a bounded amount and report the
    /// run's durable progress; poll until `done`.
    pub fn gc_blobs(&self, request: &crate::api::GcRequest) -> ClientResult<crate::dto::GcStatusDto> {
        self.api.gc_blobs(request)
    }

    pub fn gc_status(&self) -> ClientResult<crate::dto::GcStatusDto> {
        self.api.gc_status()
    }

    pub fn gc_cancel(&self) -> ClientResult<bool> {
        self.api.gc_cancel()
    }

    /// Poll or long-poll committed catalog changes. The resume cursor is
    /// server-bound, just like browse cursors, and is advanced even when a
    /// kind filter skips foreign events.
    pub fn catalog_events(
        &self,
        cursor: Option<&CatalogEventCursor>,
        wait_ms: u64,
        limit: u32,
        kind: Option<makepad_asset_data::AssetKind>,
    ) -> ClientResult<CatalogEventsPage> {
        let raw = match cursor {
            None => None,
            Some(c) if c.server_id == self.server_id => Some(c.token()),
            Some(_) => return Err(ClientError::WrongServerCursor),
        };
        let page = self.api.events_page(raw, wait_ms, limit, kind)?;
        Ok(CatalogEventsPage {
            events: page.events,
            next: CatalogEventCursor { server_id: self.server_id, token: page.cursor },
            gap: page.gap,
        })
    }

    pub(crate) fn subscription_parts(&self) -> (crate::api::Api, [u8; 16]) {
        (self.api.clone(), self.server_id)
    }

    pub(crate) fn api(&self) -> &Api {
        &self.api
    }

    // ---- jobs + worker protocol (typed passthroughs) -----------------------

    /// Advertised generation capabilities (`domain` filters, e.g. `video`).
    pub fn job_profiles(
        &self,
        domain: Option<&str>,
    ) -> ClientResult<Vec<crate::dto::JobProfileDto>> {
        self.api.job_profiles(domain)
    }

    /// Announce/renew what this worker can execute now (see
    /// [`crate::api::Api::announce_job_profiles`]).
    pub fn announce_job_profiles(
        &self,
        worker: &str,
        ns: &str,
        ttl_ms: u64,
        domains: &[String],
        profiles: &[crate::dto::JobProfileDto],
    ) -> ClientResult<()> {
        self.api
            .announce_job_profiles(worker, ns, ttl_ms, domains, profiles)
    }

    pub fn retract_job_profiles(&self, worker: &str, ns: &str) -> ClientResult<()> {
        self.api.retract_job_profiles(worker, ns)
    }

    pub fn enqueue_job(
        &self,
        ns: &str,
        kind: &str,
        body: &crate::json::Value,
    ) -> ClientResult<crate::dto::JobId> {
        self.api.enqueue_job(ns, kind, body)
    }

    pub fn job_status(&self, job: &crate::dto::JobId) -> ClientResult<crate::dto::JobStatusDto> {
        self.api.job_status(job)
    }

    /// Complete visible job state (enqueuer, attempts, progress freshness,
    /// full recorded result) — see [`crate::dto::JobDetailDto`].
    pub fn job_detail(&self, job: &crate::dto::JobId) -> ClientResult<crate::dto::JobDetailDto> {
        self.api.job_detail(job)
    }

    /// One page of the scoped job listing (`namespace` = capability-gated
    /// namespace view; `None` = the caller's own jobs).
    pub fn list_jobs(
        &self,
        namespace: Option<&str>,
        limit: u64,
    ) -> ClientResult<Vec<crate::dto::JobRowDto>> {
        self.api.list_jobs(namespace, limit)
    }

    pub fn cancel_job(&self, job: &crate::dto::JobId) -> ClientResult<u64> {
        self.api.cancel_job(job)
    }

    // ---- typed asset operations (the LLM-safe control plane) ----------------

    /// The versioned operation registry with truthful availability.
    pub fn operation_types(&self) -> ClientResult<Vec<crate::dto::OperationTypeDto>> {
        self.api.operation_types()
    }

    /// Create (or idempotently join) a typed operation.
    pub fn operation_create(
        &self,
        request: &crate::api::OperationCreateRequest,
    ) -> ClientResult<crate::dto::OperationStatusDto> {
        self.api.operation_create(request)
    }

    pub fn operation_get(
        &self,
        op: &crate::dto::OperationId,
    ) -> ClientResult<crate::dto::OperationStatusDto> {
        self.api.operation_get(op)
    }

    /// One page of the durable operation event log (bounded long-poll).
    pub fn operation_events(
        &self,
        op: &crate::dto::OperationId,
        after: u64,
        wait_ms: u64,
        limit: u32,
    ) -> ClientResult<crate::dto::OperationEventsPageDto> {
        self.api.operation_events(op, after, wait_ms, limit)
    }

    pub fn operation_cancel(&self, op: &crate::dto::OperationId) -> ClientResult<bool> {
        self.api.operation_cancel(op)
    }

    pub fn operation_retry(
        &self,
        op: &crate::dto::OperationId,
    ) -> ClientResult<crate::dto::OperationStatusDto> {
        self.api.operation_retry(op)
    }

    /// Worker-side: upload typed completion facts for atomic finalization.
    pub fn operation_finalize(
        &self,
        op: &crate::dto::OperationId,
        request: &crate::api::OperationFinalizeRequest,
    ) -> ClientResult<(makepad_asset_data::AssetId, makepad_asset_data::AssetRevisionId)>
    {
        self.api.operation_finalize(op, request)
    }

    // ---- chat broker -------------------------------------------------------

    pub fn chat_providers(&self) -> ClientResult<Vec<crate::dto::ChatProviderDto>> {
        self.api.chat_providers()
    }

    pub fn chat_create(
        &self,
        request: &crate::api::ChatCreateRequest,
    ) -> ClientResult<crate::dto::ChatSessionDto> {
        self.api.chat_create(request)
    }

    pub fn chat_get(
        &self,
        id: &crate::dto::ChatSessionId,
    ) -> ClientResult<crate::dto::ChatSessionDto> {
        self.api.chat_get(id)
    }

    pub fn chat_send(
        &self,
        id: &crate::dto::ChatSessionId,
        request: &crate::api::ChatSendRequest,
    ) -> ClientResult<u64> {
        self.api.chat_send(id, request)
    }

    pub fn chat_events(
        &self,
        id: &crate::dto::ChatSessionId,
        after: u64,
        wait_ms: u64,
        limit: u32,
    ) -> ClientResult<crate::dto::ChatEventsPageDto> {
        self.api.chat_events(id, after, wait_ms, limit)
    }

    pub fn chat_cancel(
        &self,
        id: &crate::dto::ChatSessionId,
    ) -> ClientResult<crate::dto::ChatSessionDto> {
        self.api.chat_cancel(id)
    }

    pub fn chat_retire(&self, id: &crate::dto::ChatSessionId) -> ClientResult<bool> {
        self.api.chat_retire(id)
    }

    // ---- import + immutable derived variants -------------------------------

    /// Register an approved source collection from canonical bytes.
    pub fn register_source_collection(
        &self,
        bytes: &[u8],
    ) -> ClientResult<SourceCollectionRegistered> {
        self.api.register_source_collection(bytes)
    }

    /// One explicit source-collection page. Limit is
    /// `1..=MAX_SOURCE_PAGE_LIMIT`. The resume cursor is server-bound.
    pub fn source_collections_page(
        &self,
        cursor: Option<&SourceCollectionsCursor>,
        limit: u64,
    ) -> ClientResult<SourceCollectionsPage> {
        let raw = match cursor {
            None => None,
            Some(c) if c.server_id == self.server_id => Some(c.token.as_str()),
            Some(_) => return Err(ClientError::WrongServerCursor),
        };
        let page = self.api.source_collections_page(raw, limit)?;
        Ok(SourceCollectionsPage {
            sources: page.sources,
            next: page.cursor.map(|token| SourceCollectionsCursor {
                server_id: self.server_id,
                token,
            }),
        })
    }

    /// Aggregate source collections by walking explicit pages. Fails closed
    /// above [`crate::wire::MAX_PAGE_ENTRIES`] rather than returning a prefix.
    /// There is no get-by-id route.
    pub fn list_source_collections(&self) -> ClientResult<Vec<SourceCollectionRowDto>> {
        self.api.list_source_collections()
    }

    /// Run or idempotently replay one pack import from canonical bytes.
    pub fn run_import(&self, bytes: &[u8]) -> ClientResult<ImportReportDto> {
        self.api.run_import(bytes)
    }

    /// Write the public search annotation so imported assets are findable
    /// by pack name, asset name, and tags. Aliases are indexed only for
    /// annotated assets.
    pub fn put_annotation(
        &self,
        asset: &makepad_asset_data::AssetId,
        ann: &AnnotationUpload,
    ) -> ClientResult<()> {
        self.api.put_annotation(asset, ann)
    }

    /// Import status projection for an exact import revision.
    pub fn import_status(&self, revision: &ImportRevisionId) -> ClientResult<ImportStatusDto> {
        self.api.import_status(revision)
    }

    /// Typed derived-variant manifest: cache-first, digest-verified, decoded
    /// fail-closed. Bytes enter the cache only after BOTH the digest and the
    /// canonical decode succeed.
    pub fn fetch_derived_variant(
        &mut self,
        id: &DerivedVariantId,
    ) -> ClientResult<DerivedVariantManifest> {
        if let Some(bytes) = self.cache().read_verified(id.as_bytes(), now_ms())? {
            return Ok(DerivedVariantManifest::from_canonical_bytes(&bytes)?);
        }
        let bytes = self.api.fetch_derived_variant_bytes(id)?;
        let manifest = DerivedVariantManifest::from_canonical_bytes(&bytes)?;
        self.cache().put_bytes(&bytes, Some(id.as_bytes()), now_ms())?;
        Ok(manifest)
    }

    /// Freeze an immutable variant set; the returned id is the digest of the
    /// canonical set the request names.
    pub fn freeze_variant_set(
        &self,
        base: &AssetRevisionRef,
        variants: &[DerivedVariantId],
    ) -> ClientResult<VariantSetId> {
        self.api.freeze_variant_set(base, variants)
    }

    /// Typed variant-set manifest; same digest-then-decode guarantees.
    pub fn fetch_variant_set(&mut self, id: &VariantSetId) -> ClientResult<VariantSetManifest> {
        if let Some(bytes) = self.cache().read_verified(id.as_bytes(), now_ms())? {
            return Ok(VariantSetManifest::from_canonical_bytes(&bytes)?);
        }
        let bytes = self.api.fetch_variant_set_bytes(id)?;
        let manifest = VariantSetManifest::from_canonical_bytes(&bytes)?;
        self.cache().put_bytes(&bytes, Some(id.as_bytes()), now_ms())?;
        Ok(manifest)
    }

    /// Server-side deterministic resolution of one frozen set. Never computed
    /// locally as a fallback.
    pub fn resolve_variant_set(
        &self,
        set: &VariantSetId,
        profile: &ClientProfile,
    ) -> ClientResult<ResolvedVariantMap> {
        self.api.resolve_variant_set(set, profile)
    }

    /// Content-addressed upload to the data plane (identity-verified echo).
    /// Workers reporting typed operation facts upload their output blobs
    /// through this before finalize; the publish paths upload internally.
    pub fn upload_blob(
        &self,
        ns: &str,
        bytes: &[u8],
    ) -> ClientResult<makepad_asset_data::BlobId> {
        self.api.upload_blob(ns, bytes)
    }

    /// Register and publish-side lifecycle calls for immutable game
    /// revisions. Keeping these on the same verified client used for reads
    /// lets import/bootstrap tools prove the exact bytes they subsequently
    /// fetch through the public wire contract.
    pub fn register_game(&self, ns: &str, id: Option<&GameId>) -> ClientResult<GameId> {
        self.api.register_game(ns, id)
    }

    pub fn stage_game_revision(
        &self,
        game: &GameId,
        manifest: &GameRevisionManifest,
        lock: &ContentLock,
    ) -> ClientResult<GameRevisionId> {
        if manifest.game_id != *game || lock.game_id != *game {
            return Err(ClientError::InvalidInput {
                what: "game revision identity mismatch",
            });
        }
        let manifest_bytes = manifest.to_canonical_bytes()?;
        let lock_bytes = lock.to_canonical_bytes()?;
        self.api
            .stage_game_revision(game, &manifest_bytes, &lock_bytes)
    }

    pub fn publish_game_revision(
        &self,
        game: &GameId,
        rev: &GameRevisionId,
    ) -> ClientResult<()> {
        self.api.publish_game_revision(game, rev)
    }

    pub fn put_game_alias(
        &self,
        alias: &GameAlias,
        game: &GameId,
        rev: &GameRevisionId,
    ) -> ClientResult<()> {
        self.api.put_game_alias(alias, game, rev)
    }

    pub fn resolve_game_alias(
        &self,
        alias: &GameAlias,
    ) -> ClientResult<crate::dto::GameAliasDto> {
        self.api.resolve_game_alias(alias)
    }

    /// A narrow control seam for a worker that is busy inside a long
    /// `&mut self` operation (source fetch, derivation, publication): it can
    /// renew its job lease with stage progress and observe upstream
    /// cancellation — nothing else. The API layer underneath is
    /// connection-per-request and stateless, so the handle stays valid for
    /// this client's lifetime; the raw typed API itself is not exposed.
    pub fn job_control(&self) -> JobControl {
        JobControl { api: self.api.clone() }
    }

    pub fn worker_claim(
        &self,
        lease_ms: u64,
        suffix: Option<&str>,
    ) -> ClientResult<Option<crate::dto::ClaimedJobDto>> {
        self.api.worker_claim(lease_ms, suffix)
    }

    /// Claim only jobs handled by this worker family. This filter is applied
    /// atomically by the Asset Server before a lease/attempt is opened.
    pub fn worker_claim_kinds(
        &self,
        lease_ms: u64,
        suffix: Option<&str>,
        kinds: &[&str],
    ) -> ClientResult<Option<crate::dto::ClaimedJobDto>> {
        self.api.worker_claim_kinds(lease_ms, suffix, kinds)
    }

    pub fn worker_heartbeat(
        &self,
        job: &crate::dto::JobId,
        extend_ms: u64,
        suffix: Option<&str>,
        progress: Option<(u16, &str)>,
    ) -> ClientResult<u64> {
        self.api.worker_heartbeat(job, extend_ms, suffix, progress)
    }

    pub fn worker_succeed(
        &self,
        job: &crate::dto::JobId,
        suffix: Option<&str>,
        result: Option<&crate::json::Value>,
    ) -> ClientResult<crate::dto::JobStateDto> {
        self.api.worker_succeed(job, suffix, result)
    }

    pub fn worker_fail(
        &self,
        job: &crate::dto::JobId,
        suffix: Option<&str>,
        retry_delay_ms: u64,
        error: Option<&crate::json::Value>,
    ) -> ClientResult<crate::dto::JobStateDto> {
        self.api.worker_fail(job, suffix, retry_delay_ms, error)
    }

    fn wrap_cursor(&self, raw: Option<String>) -> Option<PageCursor> {
        raw.map(|token| PageCursor { server_id: self.server_id, token })
    }

    fn unwrap_cursor<'a>(&self, cursor: Option<&'a PageCursor>) -> ClientResult<Option<&'a str>> {
        match cursor {
            None => Ok(None),
            Some(c) if c.server_id == self.server_id => Ok(Some(&c.token)),
            Some(_) => Err(ClientError::WrongServerCursor),
        }
    }

    // ---- manifests ---------------------------------------------------------

    /// Typed asset manifest for `rev`: cache-first, digest-verified, decoded
    /// fail-closed. Bytes enter the cache only after BOTH the digest and the
    /// canonical decode succeed.
    pub fn fetch_asset_manifest(&mut self, rev: &AssetRevisionId) -> ClientResult<AssetManifest> {
        if let Some(bytes) = self.cache().read_verified(rev.as_bytes(), now_ms())? {
            return Ok(AssetManifest::from_canonical_bytes(&bytes)?);
        }
        let bytes = self.api.fetch_revision_bytes(rev)?;
        let manifest = AssetManifest::from_canonical_bytes(&bytes)?;
        self.cache().put_bytes(&bytes, Some(rev.as_bytes()), now_ms())?;
        Ok(manifest)
    }

    /// Typed game revision manifest for `rev`; same guarantees.
    pub fn fetch_game_manifest(
        &mut self,
        rev: &GameRevisionId,
    ) -> ClientResult<GameRevisionManifest> {
        if let Some(bytes) = self.cache().read_verified(rev.as_bytes(), now_ms())? {
            return Ok(GameRevisionManifest::from_canonical_bytes(&bytes)?);
        }
        let bytes = self.api.fetch_game_revision_bytes(rev)?;
        let manifest = GameRevisionManifest::from_canonical_bytes(&bytes)?;
        self.cache().put_bytes(&bytes, Some(rev.as_bytes()), now_ms())?;
        Ok(manifest)
    }

    // ---- blobs -------------------------------------------------------------

    pub fn blob_head(&self, blob: &BlobId) -> ClientResult<BlobHead> {
        self.api.blob_head(blob)
    }

    /// Fetch a blob into the cache and return its verified local path.
    /// Cache-first; otherwise streams (resuming any surviving partial via
    /// `Range`/`If-Range`), verifies the digest, and commits atomically.
    ///
    /// `expected_len` (from a manifest's `byte_len`) is enforced against the
    /// server's declared sizes BEFORE bytes stream, so a lying server cannot
    /// fill the disk. `progress` observes `(bytes_present, total_bytes)`.
    ///
    /// Transport failures (`Io`/`Timeout`) retry up to the configured attempt
    /// budget, resuming from the bytes already on disk; protocol violations
    /// and refusals never retry.
    pub fn fetch_blob(
        &mut self,
        blob: &BlobId,
        expected_len: Option<u64>,
        progress: Option<&mut dyn FnMut(u64, u64)>,
    ) -> ClientResult<PathBuf> {
        self.fetch_blob_with_abort(blob, expected_len, progress, &|| false)
    }

    /// As [`fetch_blob`](Self::fetch_blob), aborting between stream chunks
    /// whenever `abort()` reports true. An abort ends the fetch with
    /// [`ClientError::Cancelled`]; the partial stays on disk and a later
    /// fetch of the same blob resumes it.
    pub fn fetch_blob_with_abort(
        &mut self,
        blob: &BlobId,
        expected_len: Option<u64>,
        mut progress: Option<&mut dyn FnMut(u64, u64)>,
        abort: &dyn Fn() -> bool,
    ) -> ClientResult<PathBuf> {
        let digest = *blob.as_bytes();
        if let Some(path) = self.cache().resolve(&digest, now_ms())? {
            return Ok(path);
        }
        if expected_len == Some(0) {
            // The content contract refuses zero-length files; a zero here is
            // caller confusion, not a fetchable object.
            return Err(ClientError::InvalidInput { what: "expected_len zero" });
        }

        // One transfer per digest across all lanes: the resumable partial is
        // a single append-mode file. A waiter re-checks the cache first,
        // because the holder it waited for has usually just committed it.
        // The gate handle is cloned out first so the claim borrows it and
        // not `self`; the transfer below needs `&mut self`.
        let transfers = self.transfers.clone();
        let Some(_claim) = transfers.claim(digest, abort) else {
            return Err(ClientError::Cancelled);
        };
        if let Some(path) = self.cache().resolve(&digest, now_ms())? {
            return Ok(path);
        }

        let mut attempt = 0u32;
        loop {
            attempt += 1;
            let result = self.transfer_attempt(blob, expected_len, &mut progress, abort);
            match result {
                Ok(path) => return Ok(path),
                Err(e) => {
                    let retryable = match &e {
                        ClientError::Io { .. } | ClientError::Timeout { .. } => true,
                        // A digest mismatch invalidates the partial; one
                        // clean restart may recover a torn local file, so it
                        // consumes an attempt like a transport failure.
                        ClientError::DigestMismatch { .. } => {
                            self.cache().discard_partial(&digest)?;
                            true
                        }
                        // A cancellation is the caller's decision, final.
                        ClientError::Cancelled => false,
                        _ => false,
                    };
                    if !retryable || attempt >= self.max_transfer_attempts {
                        return Err(e);
                    }
                }
            }
        }
    }

    /// One transfer attempt: open/resume the partial, issue the request,
    /// stream, commit. Leaves the partial on disk on transport failure.
    fn transfer_attempt(
        &mut self,
        blob: &BlobId,
        expected_len: Option<u64>,
        progress: &mut Option<&mut dyn FnMut(u64, u64)>,
        abort: &dyn Fn() -> bool,
    ) -> ClientResult<PathBuf> {
        let digest = *blob.as_bytes();
        let mut writer = self.cache().open_partial(&digest)?;

        // A partial already at/above the expected size cannot be extended by
        // a range request; it is either complete (commit will prove it) or
        // poisoned (commit deletes it and the retry starts clean).
        if let Some(len) = expected_len {
            if writer.resumed_bytes() >= len {
                return self.cache().commit_partial(writer, now_ms());
            }
        }

        let start = writer.resumed_bytes();
        let range_start = if start > 0 { Some(start) } else { None };
        let mut resp = self.api.blob_get(blob, range_start, self.blob_body_deadline_ms)?;

        let total = match resp.head().status {
            200 => {
                let declared = resp.head().content_length;
                if let Some(len) = expected_len {
                    if declared != len {
                        return Err(ClientError::SizeMismatch {
                            what: "blob declared length",
                            expected: len,
                            found: declared,
                        });
                    }
                }
                // Full body: any resumed bytes restart from zero.
                if writer.resumed_bytes() > 0 {
                    writer.reset()?;
                }
                declared
            }
            206 => {
                let cr = resp
                    .head()
                    .content_range
                    .ok_or(ClientError::Protocol { what: "206 without content-range" })?;
                if cr.start != start {
                    return Err(ClientError::Protocol { what: "resume offset mismatch" });
                }
                if let Some(len) = expected_len {
                    if cr.size != len {
                        return Err(ClientError::SizeMismatch {
                            what: "blob range size",
                            expected: len,
                            found: cr.size,
                        });
                    }
                }
                if cr.end != cr.size - 1 {
                    // We always request an open-ended range.
                    return Err(ClientError::Protocol { what: "resume range truncated" });
                }
                cr.size
            }
            416 => {
                // Our resume offset is at/past the server's size. With a real
                // resume in flight this legitimately means "the partial may
                // already be complete": commit PROVES it by digest (and
                // deletes it on mismatch, so the retry starts clean). A 416
                // without a range request is protocol nonsense.
                if start > 0 {
                    return self.cache().commit_partial(writer, now_ms());
                }
                return Err(ClientError::Protocol { what: "unexpected 416" });
            }
            _ => return Err(ClientError::Protocol { what: "unexpected blob status" }),
        };

        // Admission preflight: refuse before bytes stream when it can never
        // be admitted (over per-object budget or pinned bytes leave no room).
        if total > self.cache().budgets().max_object_bytes {
            return Err(ClientError::CacheAdmission {
                what: "object over per-object budget",
                limit: self.cache().budgets().max_object_bytes,
                found: total,
            });
        }

        if let Some(cb) = progress.as_deref_mut() {
            cb(writer.resumed_bytes(), total);
        }
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            if abort() {
                // Partial stays resumable; the caller chose to stop.
                return Err(ClientError::Cancelled);
            }
            let n = resp.read_chunk(&mut chunk)?;
            if n == 0 {
                break;
            }
            writer.write(&chunk[..n])?;
            if let Some(cb) = progress.as_deref_mut() {
                cb(writer.resumed_bytes(), total);
            }
        }
        if writer.resumed_bytes() != total {
            // read_chunk returned clean end before the declared total; the
            // framing layer normally catches this, keep it as defense.
            return Err(ClientError::SizeMismatch {
                what: "blob received bytes",
                expected: total,
                found: writer.resumed_bytes(),
            });
        }
        self.cache().commit_partial(writer, now_ms())
    }

    /// Fetch many blobs in ONE request, in the given order, committing each
    /// one to the cache AS IT ARRIVES and reporting it through `on_item`.
    ///
    /// This is the thumbnail-grid path. Three properties make it worth a
    /// separate route:
    /// - **Order is priority.** Items are fetched in the order given, so a
    ///   caller that puts the visible row first sees it first.
    /// - **Per-frame commit.** Each blob is verified and committed on
    ///   arrival, so abandoning the rest (below) never wastes what already
    ///   landed.
    /// - **Abandonable.** `abort` is consulted per item; when everything
    ///   still outstanding has been cancelled the response is dropped
    ///   mid-stream (socket closed, nothing pooled) and the caller can
    ///   re-issue with its new priorities.
    ///
    /// Items already in the cache never reach the wire. Items the server
    /// could not include (missing, over cap, budget exhausted) are reported
    /// with a typed error so the caller can fall back to a single fetch;
    /// `Err` from this call itself means the batch route is unusable (a 404
    /// from an older server, a transport failure) and EVERY item should fall
    /// back.
    pub fn fetch_blobs_ordered(
        &mut self,
        items: &[(BlobId, Option<u64>)],
        abort: &dyn Fn(&BlobId) -> bool,
        on_item: &mut dyn FnMut(BlobId, ClientResult<PathBuf>),
    ) -> ClientResult<()> {
        if items.is_empty() {
            return Ok(());
        }
        // Cache-first, before anything is asked of the server.
        let mut wanted: Vec<BatchItem> = Vec::with_capacity(items.len());
        for (blob, expected_len) in items {
            if abort(blob) {
                on_item(*blob, Err(ClientError::Cancelled));
                continue;
            }
            if let Some(path) = self.cache().resolve(blob.as_bytes(), now_ms())? {
                on_item(*blob, Ok(path));
                continue;
            }
            wanted.push(BatchItem { blob: *blob, max_bytes: *expected_len });
        }
        if wanted.is_empty() {
            return Ok(());
        }
        let deadline = self.blob_body_deadline_ms;
        // Collected inside the frame callback and applied after, because the
        // callback cannot borrow `self` while the batch call does.
        let mut landed: Vec<(BlobId, Vec<u8>)> = Vec::new();
        let mut refused: Vec<(BlobId, ClientError)> = Vec::new();
        let mut cancelled: Vec<BlobId> = Vec::new();
        let mut stopped_at: Option<usize> = None;
        {
            let mut index = 0usize;
            let mut on_frame = |blob: BlobId, frame: BatchFrame, bytes: &[u8]| -> BatchFlow {
                index += 1;
                match frame {
                    BatchFrame::Ok => landed.push((blob, bytes.to_vec())),
                    BatchFrame::Missing => {
                        refused.push((blob, ClientError::NotFound { what: "blob" }))
                    }
                    BatchFrame::OverItemCap | BatchFrame::Skipped => refused.push((
                        blob,
                        ClientError::OverBudget {
                            what: "batch item",
                            limit: 0,
                            found: bytes.len() as u64,
                        },
                    )),
                }
                // Stop only when NOTHING outstanding is still wanted: a
                // half-cancelled batch keeps streaming for the items that
                // still matter.
                if wanted[index..].iter().all(|i| abort(&i.blob)) {
                    for item in &wanted[index..] {
                        cancelled.push(item.blob);
                    }
                    stopped_at = Some(index);
                    return BatchFlow::Stop;
                }
                BatchFlow::Continue
            };
            self.api.fetch_blob_batch(&wanted, deadline, &mut on_frame)?;
        }
        // Commit per item, digest-checked by the cache itself.
        for (blob, bytes) in landed {
            // Two statements on purpose: the cache guard from `put_bytes`
            // must be released before `object_path_of` takes it again — a
            // std mutex is not reentrant, and a one-liner here deadlocks the
            // worker against itself.
            let committed = self.cache().put_bytes(&bytes, Some(blob.as_bytes()), now_ms());
            let result = match committed {
                Ok(digest) => Ok(self.cache().object_path_of(&digest)),
                Err(e) => Err(e),
            };
            on_item(blob, result);
        }
        for (blob, error) in refused {
            on_item(blob, Err(error));
        }
        for blob in cancelled {
            on_item(blob, Err(ClientError::Cancelled));
        }
        let _ = stopped_at;
        Ok(())
    }

    /// Fetch a blob and return its verified bytes in memory.
    ///
    /// Streams from the server into RAM. Does not write the payload to the
    /// client disk cache — the asset server already owns that.
    pub fn fetch_blob_bytes(
        &mut self,
        blob: &BlobId,
        expected_len: Option<u64>,
    ) -> ClientResult<Vec<u8>> {
        let digest = *blob.as_bytes();
        if let Some(bytes) = self.ram.lock().expect("ram cache").get(&digest) {
            return Ok(bytes.clone());
        }
        if let Some(bytes) = self.cache().read_verified(&digest, now_ms())? {
            self.ram.lock().expect("ram cache").insert(digest, bytes.clone());
            return Ok(bytes);
        }
        // Streamed outside every lock: a second lane stays free to work.
        let bytes = self.stream_blob_memory(blob, expected_len)?;
        self.ram.lock().expect("ram cache").insert(digest, bytes.clone());
        Ok(bytes)
    }

    fn stream_blob_memory(
        &self,
        blob: &BlobId,
        expected_len: Option<u64>,
    ) -> ClientResult<Vec<u8>> {
        if expected_len == Some(0) {
            return Err(ClientError::InvalidInput { what: "expected_len zero" });
        }
        let mut attempt = 0u32;
        loop {
            attempt += 1;
            match self.stream_blob_memory_once(blob, expected_len) {
                Ok(bytes) => return Ok(bytes),
                Err(e) => {
                    let retryable = matches!(
                        e,
                        ClientError::Io { .. }
                            | ClientError::Timeout { .. }
                            | ClientError::DigestMismatch { .. }
                    );
                    if !retryable || attempt >= self.max_transfer_attempts {
                        return Err(e);
                    }
                }
            }
        }
    }

    fn stream_blob_memory_once(
        &self,
        blob: &BlobId,
        expected_len: Option<u64>,
    ) -> ClientResult<Vec<u8>> {
        let mut resp = self.api.blob_get(blob, None, self.blob_body_deadline_ms)?;
        if resp.head().status != 200 {
            return Err(ClientError::Protocol { what: "unexpected blob status" });
        }
        let declared = resp.head().content_length;
        if let Some(len) = expected_len {
            if declared != len {
                return Err(ClientError::SizeMismatch {
                    what: "blob declared length",
                    expected: len,
                    found: declared,
                });
            }
        }
        if declared > self.cache().budgets().max_object_bytes {
            return Err(ClientError::CacheAdmission {
                what: "object over per-object budget",
                limit: self.cache().budgets().max_object_bytes,
                found: declared,
            });
        }
        let mut hasher = makepad_asset_data::Sha256::new();
        let mut out = Vec::with_capacity(declared as usize);
        let mut chunk = vec![0u8; 64 * 1024];
        loop {
            let n = resp.read_chunk(&mut chunk)?;
            if n == 0 {
                break;
            }
            hasher.update(&chunk[..n]);
            out.extend_from_slice(&chunk[..n]);
        }
        if out.len() as u64 != declared {
            return Err(ClientError::SizeMismatch {
                what: "blob received bytes",
                expected: declared,
                found: out.len() as u64,
            });
        }
        let found = hasher.finalize();
        if &found != blob.as_bytes() {
            return Err(ClientError::DigestMismatch {
                what: "memory blob",
                expected: *blob.as_bytes(),
                found,
            });
        }
        Ok(out)
    }

    /// Verified local path if (and only if) the blob is already cached; no
    /// network. `Ok(None)` means "not cached" — never a guess.
    pub fn cached_blob(&mut self, blob: &BlobId) -> ClientResult<Option<PathBuf>> {
        self.cache().resolve(blob.as_bytes(), now_ms())
    }

    /// This client's cache root (diagnostics only; the cache remains
    /// single-owner).
    pub fn cache_root(&self) -> &Path {
        &self.cache_root
    }
}

/// The narrow worker control seam from [`AssetClient::job_control`]: lease
/// heartbeats with progress, and job-state observation for cancellation
/// propagation. Deliberately NOT the full API — a worker mid-operation gets
/// exactly the two capabilities that must stay live while its client handle
/// is mutably busy, and cannot be tempted into raw route access.
#[derive(Clone)]
pub struct JobControl {
    api: Api,
}

impl JobControl {
    /// Renew the claim lease; optionally report `(permille, note)` stage
    /// progress. Returns the new lease expiry.
    pub fn heartbeat(
        &self,
        job: &crate::dto::JobId,
        extend_ms: u64,
        suffix: Option<&str>,
        progress: Option<(u16, &str)>,
    ) -> ClientResult<u64> {
        self.api.worker_heartbeat(job, extend_ms, suffix, progress)
    }

    /// The job's current lifecycle state (the cancellation observation
    /// point: [`crate::dto::JobStateDto::Cancelled`] means stop working).
    pub fn job_state(&self, job: &crate::dto::JobId) -> ClientResult<crate::dto::JobStateDto> {
        self.api.job_status(job).map(|status| status.state)
    }
}
