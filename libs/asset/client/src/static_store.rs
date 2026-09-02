//! Verified, immutable reader for a version-1 static store export.
//!
//! The manifest and health handshake are fetched once. No indexed state is
//! made visible until their schema, identities, ordering, references, route
//! metadata, and exact declared lengths have all been checked. Search is a
//! local projection: filters and facets match the exported data exactly;
//! ranking parity with the mutable server search engine is not claimed.

use crate::api::{BlobHead, CatalogQuery, MAX_LIST_LIMIT};
use crate::cache_store::{BlobContent, CacheStore, MemoryCacheStore};
use crate::client::{AssetsPage, CatalogPage, PageCursor};
use crate::dto::{
    self, AliasDto, AliasStatusDto, AssetDetailDto, AssetRow, CandidateDto,
    CandidateStateDto, CatalogFacet, CatalogHit, FacetKind, GameAliasDto,
};
use crate::error::{ClientError, ClientResult};
use crate::location::{BaseUrl, ClientMode};
use crate::transport::{
    OwnedRequest, OwnedResponse, Transport, TransportCompletion, TransportError, TransportId,
    TransportMethod,
};
use crate::util::from_hex_exact;
use makepad_asset_data::{
    AssetAlias, AssetId, AssetKind, AssetManifest, AssetRevisionId, BlobId, DerivedVariantId,
    DerivedVariantManifest, GameAlias, GameRevisionId, GameRevisionManifest, Sha256,
    VariantSetId, VariantSetManifest,
};
use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::str::FromStr;

pub const MAX_STATIC_MANIFEST_BYTES: u64 = 16 * 1024 * 1024;
const MAX_STATIC_ITEMS: usize = 100_000;
const MAX_STATIC_STRING_BYTES: usize = 16 * 1024;
const MAX_COMPLETIONS_PER_POLL: usize = 64;

pub type StaticFetchId = u64;

#[derive(Clone, Debug)]
pub enum StaticFetch {
    AssetManifest(AssetRevisionId),
    GameManifest(GameRevisionId),
    DerivedVariant(DerivedVariantId),
    VariantSet(VariantSetId),
    Blob { blob: BlobId, expected_len: Option<u64>, pin: bool },
}

#[derive(Clone, Debug)]
pub enum StaticFetchOutput {
    AssetManifest(Box<AssetManifest>),
    GameManifest(Box<GameRevisionManifest>),
    DerivedVariant(Box<DerivedVariantManifest>),
    VariantSet(Box<VariantSetManifest>),
    Blob { blob: BlobId, content: BlobContent },
}

#[derive(Clone, Debug)]
pub enum StaticStoreEvent {
    Ready,
    Failed(ClientError),
    FetchDone { id: StaticFetchId, output: StaticFetchOutput },
    FetchFailed { id: StaticFetchId, error: ClientError },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StaticStoreState {
    Connecting,
    Ready,
    Failed(ClientError),
}

#[derive(Clone)]
struct FileMeta {
    byte_len: u64,
    sha256: [u8; 32],
}

#[derive(Clone)]
struct AssetMeta {
    row: AssetRow,
    revisions: Vec<AssetRevisionId>,
}

#[derive(Clone)]
struct AliasMeta {
    dto: AliasDto,
    updated_ms: u64,
}

#[derive(Clone)]
struct RevisionMeta {
    asset_id: AssetId,
    sources: Vec<BlobId>,
    blobs: Vec<(BlobId, u64)>,
    thumbnail: Option<(BlobId, u64)>,
    dependencies: Vec<(AssetId, AssetRevisionId)>,
}

#[derive(Clone)]
struct SearchTerm {
    term: String,
    weight: u64,
}

#[derive(Clone)]
struct SearchDoc {
    asset_id: AssetId,
    namespace: String,
    kind: Option<AssetKind>,
    title: String,
    description: String,
    categories: Vec<String>,
    tags: Vec<String>,
    creator: String,
    live: bool,
    updated_ms: u64,
    aliases: Vec<AssetAlias>,
    terms: Vec<SearchTerm>,
}

#[derive(Clone)]
struct BlobMeta {
    byte_len: u64,
    present: bool,
    path: String,
}

struct Index {
    snapshot_id: String,
    server_id: [u8; 16],
    files: BTreeMap<String, FileMeta>,
    assets: BTreeMap<AssetId, AssetMeta>,
    aliases: BTreeMap<AssetAlias, AliasMeta>,
    revisions: BTreeMap<AssetRevisionId, RevisionMeta>,
    search: Vec<SearchDoc>,
    blobs: BTreeMap<BlobId, BlobMeta>,
}

enum Pending {
    Health,
    Manifest,
    Fetch([u8; 32]),
}

struct Waiter {
    id: StaticFetchId,
    fetch: StaticFetch,
}

struct InFlight {
    transport_id: TransportId,
    digest: [u8; 32],
    file: FileMeta,
    waiters: Vec<Waiter>,
}

/// One static snapshot and its page-lifetime verified cache.
pub struct StaticStore {
    base: BaseUrl,
    transport: Box<dyn Transport>,
    cache: Box<dyn CacheStore>,
    state: StaticStoreState,
    pending: HashMap<TransportId, Pending>,
    completions: VecDeque<TransportCompletion>,
    health_bytes: Option<Vec<u8>>,
    manifest_bytes: Option<Vec<u8>>,
    index: Option<Index>,
    inflight: HashMap<[u8; 32], InFlight>,
    ready_events: VecDeque<StaticStoreEvent>,
    next_fetch_id: StaticFetchId,
}

impl Drop for StaticStore {
    fn drop(&mut self) {
        for id in self.pending.keys().copied().collect::<Vec<_>>() {
            self.transport.cancel(id);
        }
    }
}

impl StaticStore {
    pub fn start(
        base: BaseUrl,
        transport: Box<dyn Transport>,
        cache: Box<dyn CacheStore>,
    ) -> ClientResult<Self> {
        let mut store = Self {
            base,
            transport,
            cache,
            state: StaticStoreState::Connecting,
            pending: HashMap::new(),
            completions: VecDeque::new(),
            health_bytes: None,
            manifest_bytes: None,
            index: None,
            inflight: HashMap::new(),
            ready_events: VecDeque::new(),
            next_fetch_id: 1,
        };
        let health = store.start_get("/v1/health", MAX_STATIC_MANIFEST_BYTES.min(64 * 1024))?;
        let manifest = store.start_get("/v1/static/manifest.json", MAX_STATIC_MANIFEST_BYTES)?;
        store.pending.insert(health, Pending::Health);
        store.pending.insert(manifest, Pending::Manifest);
        Ok(store)
    }

    #[cfg(any(target_arch = "wasm32", feature = "native", feature = "web"))]
    pub fn platform(base: BaseUrl, cache_bytes: u64) -> ClientResult<Self> {
        Self::start(
            base,
            Box::new(crate::transport::PlatformHttpTransport::new()),
            Box::new(MemoryCacheStore::new(cache_bytes)),
        )
    }

    pub fn state(&self) -> &StaticStoreState { &self.state }

    pub fn is_ready(&self) -> bool { matches!(self.state, StaticStoreState::Ready) }

    pub fn server_id(&self) -> Option<[u8; 16]> {
        self.index.as_ref().map(|index| index.server_id)
    }

    pub fn location(&self) -> crate::location::ClientLocation {
        crate::location::ClientLocation::StaticSite(self.base.clone())
    }

    pub fn poll(&mut self) -> Vec<StaticStoreEvent> {
        let mut incoming = Vec::new();
        self.transport.poll(&mut incoming);
        self.completions.extend(incoming);
        for _ in 0..MAX_COMPLETIONS_PER_POLL {
            let Some(completion) = self.completions.pop_front() else { break };
            self.finish_transport(completion);
        }
        self.ready_events.drain(..).collect()
    }

    pub fn assets_page(
        &self,
        namespace: Option<&str>,
        cursor: Option<&PageCursor>,
        limit: u64,
    ) -> ClientResult<AssetsPage> {
        let index = self.ready_index()?;
        if limit == 0 || limit > MAX_LIST_LIMIT {
            return Err(ClientError::InvalidInput { what: "listing limit" });
        }
        if let Some(namespace) = namespace {
            if namespace.is_empty()
                || namespace.len() > crate::wire::MAX_NAMESPACE_BYTES
                || !crate::wire::query_value_ok(namespace)
            {
                return Err(ClientError::InvalidInput { what: "listing namespace" });
            }
        }
        let fingerprint = digest_text(&format!("list\0{}\0{limit}", namespace.unwrap_or("")));
        let offset = cursor_offset(index, cursor, &fingerprint)?;
        let rows: Vec<_> = index
            .assets
            .values()
            .filter(|asset| namespace.is_none_or(|ns| asset.row.namespace == ns))
            .map(|asset| asset.row.clone())
            .collect();
        let end = offset.saturating_add(limit as usize).min(rows.len());
        let next = (end < rows.len()).then(|| make_cursor(index, &fingerprint, end));
        Ok(AssetsPage { assets: rows[offset.min(rows.len())..end].to_vec(), next })
    }

    pub fn asset_detail(&self, id: &AssetId) -> ClientResult<AssetDetailDto> {
        let index = self.ready_index()?;
        let asset = index.assets.get(id).ok_or(ClientError::NotFound { what: "asset" })?;
        let candidates = asset
            .revisions
            .iter()
            .map(|revision| {
                let published_ms = index
                    .aliases
                    .values()
                    .filter(|alias| alias.dto.asset_id == *id && alias.dto.head_revision == *revision)
                    .map(|alias| alias.updated_ms)
                    .max()
                    .unwrap_or(asset.row.created_ms);
                CandidateDto {
                    revision: *revision,
                    state: CandidateStateDto::Published,
                    staged_ms: published_ms,
                    published_ms: Some(published_ms),
                    quarantined_ms: None,
                    retired_ms: None,
                }
            })
            .collect();
        Ok(AssetDetailDto {
            asset_id: *id,
            namespace: asset.row.namespace.clone(),
            retired: false,
            retired_ms: None,
            candidates,
        })
    }

    pub fn resolve_alias(&self, alias: &AssetAlias) -> ClientResult<AliasDto> {
        self.ready_index()?
            .aliases
            .get(alias)
            .map(|entry| entry.dto.clone())
            .ok_or(ClientError::NotFound { what: "alias" })
    }

    pub fn resolve_game_alias(&self, _alias: &GameAlias) -> ClientResult<GameAliasDto> {
        self.ready_index()?;
        Err(ClientError::NotFound { what: "game alias" })
    }

    pub fn alias_status(
        &self,
        entries: &[(AssetAlias, Option<BlobId>)],
        tags: &[String],
    ) -> ClientResult<Vec<AliasStatusDto>> {
        let index = self.ready_index()?;
        if entries.len() > crate::wire::MAX_ALIAS_STATUS_ITEMS {
            return Err(ClientError::InvalidInput { what: "alias status entries" });
        }
        let wanted: BTreeSet<&str> = tags.iter().map(String::as_str).collect();
        if wanted.len() > 16
            || wanted.iter().any(|tag| {
                tag.is_empty()
                    || tag.len() > crate::wire::MAX_FILTER_VALUE_BYTES
                    || tag.chars().any(char::is_control)
            })
        {
            return Err(ClientError::InvalidInput { what: "alias status tags" });
        }
        let mut out = Vec::with_capacity(entries.len());
        for (alias, expected_source) in entries {
            let Some(found) = index.aliases.get(alias) else {
                out.push(AliasStatusDto {
                    alias: alias.clone(), present: false, asset_id: None,
                    head_revision: None, source: None, source_matches: false, tags: Vec::new(),
                });
                continue;
            };
            let source = index
                .revisions
                .get(&found.dto.head_revision)
                .and_then(|revision| revision.sources.first().copied());
            let matched_tags = index
                .search
                .iter()
                .find(|doc| doc.asset_id == found.dto.asset_id)
                .map(|doc| {
                    doc.tags.iter().filter(|tag| wanted.contains(tag.as_str())).cloned().collect()
                })
                .unwrap_or_default();
            out.push(AliasStatusDto {
                alias: alias.clone(), present: true, asset_id: Some(found.dto.asset_id),
                head_revision: Some(found.dto.head_revision), source,
                source_matches: expected_source.is_some_and(|expected| source == Some(expected)),
                tags: matched_tags,
            });
        }
        Ok(out)
    }

    /// Local search over the export's public projection. Filter and facet
    /// semantics are exact; mutable-server synonym/ranking parity is not a
    /// contract of the static format.
    pub fn catalog_search(
        &self,
        query: &CatalogQuery,
        cursor: Option<&PageCursor>,
    ) -> ClientResult<CatalogPage> {
        let index = self.ready_index()?;
        query.validate()?;
        let fingerprint = query_fingerprint(query);
        let offset = cursor_offset(index, cursor, &fingerprint)?;
        let terms = tokenize(&query.text);
        if !query.text.trim().is_empty() && terms.is_empty() {
            return Err(ClientError::InvalidInput { what: "search query has no terms" });
        }
        let browse = query.text.trim().is_empty();
        let mut matches: Vec<(&SearchDoc, u64)> = index
            .search
            .iter()
            .filter_map(|doc| match_search(doc, query, &terms).map(|score| (doc, score)))
            .collect();
        matches.sort_by(|(a, sa), (b, sb)| {
            let alias_a = a.aliases.first().map(ToString::to_string).unwrap_or_default();
            let alias_b = b.aliases.first().map(ToString::to_string).unwrap_or_default();
            if browse { alias_a.cmp(&alias_b).then(a.asset_id.cmp(&b.asset_id)) }
            else { sb.cmp(sa).then(alias_a.cmp(&alias_b)).then(a.asset_id.cmp(&b.asset_id)) }
        });
        let facets = build_facets(&matches, query.facets);
        let total = matches.len() as u64;
        let end = offset.saturating_add(query.page_size as usize).min(matches.len());
        let hits = matches[offset.min(matches.len())..end]
            .iter()
            .map(|(doc, score)| CatalogHit {
                asset_id: doc.asset_id,
                namespace: doc.namespace.clone(),
                kind: doc.kind,
                title: doc.title.clone(),
                snippet: snippet(&doc.title, &doc.description),
                score: *score,
                live: doc.live,
                alias: doc.aliases.first().cloned(),
                updated_ms: doc.updated_ms,
            })
            .collect();
        let next = (end < matches.len()).then(|| make_cursor(index, &fingerprint, end));
        Ok(CatalogPage { hits, total, next, facets })
    }

    pub fn blob_head(&self, blob: &BlobId) -> ClientResult<BlobHead> {
        let entry = self.ready_index()?
            .blobs.get(blob).filter(|entry| entry.present)
            .ok_or(ClientError::NotFound { what: "blob" })?;
        Ok(BlobHead { size: entry.byte_len, etag_matches: true })
    }

    pub fn start_fetch(&mut self, fetch: StaticFetch) -> ClientResult<StaticFetchId> {
        let index = self.ready_index()?;
        let (path, digest, file) = fetch_target(index, &fetch)?;
        let id = self.next_fetch_id;
        self.next_fetch_id = self.next_fetch_id.checked_add(1).unwrap_or(1);
        if let Some(content) = self.cache.get_verified(&digest)? {
            let output = decode_fetch(&fetch, content)?;
            if matches!(fetch, StaticFetch::Blob { pin: true, .. }) {
                self.cache.pin(&digest)?;
            }
            self.ready_events.push_back(StaticStoreEvent::FetchDone { id, output });
            return Ok(id);
        }
        if let Some(active) = self.inflight.get_mut(&digest) {
            active.waiters.push(Waiter { id, fetch });
            return Ok(id);
        }
        let transport_id = self.start_get(&path, file.byte_len)?;
        self.pending.insert(transport_id, Pending::Fetch(digest));
        self.inflight.insert(digest, InFlight {
            transport_id, digest, file, waiters: vec![Waiter { id, fetch }],
        });
        Ok(id)
    }

    pub fn cancel_fetch(&mut self, id: StaticFetchId) {
        let mut empty = None;
        for (digest, active) in &mut self.inflight {
            if let Some(at) = active.waiters.iter().position(|waiter| waiter.id == id) {
                active.waiters.remove(at);
                self.ready_events.push_back(StaticStoreEvent::FetchFailed {
                    id, error: ClientError::Cancelled,
                });
                if active.waiters.is_empty() { empty = Some((*digest, active.transport_id)); }
                break;
            }
        }
        if let Some((digest, transport_id)) = empty {
            self.inflight.remove(&digest);
            self.pending.remove(&transport_id);
            self.transport.cancel(transport_id);
        }
    }

    pub fn unpin_blob(&mut self, blob: &BlobId) -> ClientResult<()> {
        self.ready_index()?;
        self.cache.unpin(blob.as_bytes())
    }

    pub fn unavailable<T>(&self, capability: &'static str) -> ClientResult<T> {
        Err(ClientError::Unavailable { capability, mode: ClientMode::StaticWeb })
    }

    fn ready_index(&self) -> ClientResult<&Index> {
        match &self.state {
            StaticStoreState::Ready => self.index.as_ref().ok_or(ClientError::Protocol {
                what: "static ready index missing",
            }),
            StaticStoreState::Failed(error) => Err(error.clone()),
            StaticStoreState::Connecting => Err(ClientError::Unavailable {
                capability: "static_store_not_ready", mode: ClientMode::StaticWeb,
            }),
        }
    }

    fn start_get(&mut self, target: &str, limit: u64) -> ClientResult<TransportId> {
        let url = self.base.join(target)?;
        Ok(self.transport.start(
            OwnedRequest::new(TransportMethod::Get, url).max_response_body_bytes(limit.max(1)),
        ))
    }

    fn finish_transport(&mut self, completion: TransportCompletion) {
        let Some(pending) = self.pending.remove(&completion.id) else { return };
        match pending {
            Pending::Health | Pending::Manifest => {
                let result = completion.result.map_err(transport_error).and_then(expect_ok);
                match (pending, result) {
                    (Pending::Health, Ok(response)) => self.health_bytes = Some(response.body),
                    (Pending::Manifest, Ok(response)) => self.manifest_bytes = Some(response.body),
                    (_, Err(error)) => { self.fail(error); return; }
                    _ => unreachable!(),
                }
                if self.health_bytes.is_some() && self.manifest_bytes.is_some() {
                    let health = self.health_bytes.take().unwrap();
                    let manifest = self.manifest_bytes.take().unwrap();
                    match parse_index(&manifest, &health) {
                        Ok(index) => {
                            self.index = Some(index);
                            self.state = StaticStoreState::Ready;
                            self.ready_events.push_back(StaticStoreEvent::Ready);
                        }
                        Err(error) => self.fail(error),
                    }
                }
            }
            Pending::Fetch(digest) => self.finish_fetch(&digest, completion.result),
        }
    }

    fn finish_fetch(&mut self, digest: &[u8; 32], result: Result<OwnedResponse, TransportError>) {
        let Some(active) = self.inflight.remove(digest) else { return };
        let bytes = result.map_err(transport_error).and_then(expect_ok).and_then(|response| {
            verify_bytes("static route", &active.file, &response.body)?;
            let found = hash(&response.body);
            if found != active.digest {
                return Err(ClientError::DigestMismatch {
                    what: "static digest route", expected: active.digest, found,
                });
            }
            Ok(response.body)
        });
        match bytes {
            Ok(bytes) => {
                if let Err(error) = self.cache.put_verified(&active.digest, &bytes) {
                    for waiter in active.waiters {
                        self.ready_events.push_back(StaticStoreEvent::FetchFailed {
                            id: waiter.id, error: error.clone(),
                        });
                    }
                    return;
                }
                for waiter in active.waiters {
                    let result = self.cache.get_verified(&active.digest).and_then(|content| {
                        let content = content.ok_or(ClientError::Protocol {
                            what: "static cache lost admitted object",
                        })?;
                        if matches!(waiter.fetch, StaticFetch::Blob { pin: true, .. }) {
                            self.cache.pin(&active.digest)?;
                        }
                        decode_fetch(&waiter.fetch, content)
                    });
                    match result {
                        Ok(output) => self.ready_events.push_back(StaticStoreEvent::FetchDone {
                            id: waiter.id, output,
                        }),
                        Err(error) => self.ready_events.push_back(StaticStoreEvent::FetchFailed {
                            id: waiter.id, error,
                        }),
                    }
                }
            }
            Err(error) => for waiter in active.waiters {
                self.ready_events.push_back(StaticStoreEvent::FetchFailed {
                    id: waiter.id, error: error.clone(),
                });
            },
        }
    }

    fn fail(&mut self, error: ClientError) {
        if matches!(self.state, StaticStoreState::Failed(_)) { return; }
        for id in self.pending.keys().copied().collect::<Vec<_>>() { self.transport.cancel(id); }
        self.pending.clear();
        self.inflight.clear();
        self.state = StaticStoreState::Failed(error.clone());
        self.ready_events.push_back(StaticStoreEvent::Failed(error));
    }
}

fn expect_ok(response: OwnedResponse) -> ClientResult<OwnedResponse> {
    match response.status {
        200 => Ok(response),
        401 => Err(ClientError::Unauthenticated),
        403 => Err(ClientError::Denied),
        404 => Err(ClientError::NotFound { what: "static route" }),
        status => Err(ClientError::Server { status, detail: None }),
    }
}

fn transport_error(error: TransportError) -> ClientError {
    match error {
        TransportError::InvalidRequest { what } | TransportError::Protocol { what } => {
            ClientError::Protocol { what }
        }
        TransportError::OverBudget { what, limit, found } => {
            ClientError::OverBudget { what, limit, found }
        }
        TransportError::Client(error) => error,
        TransportError::Cancelled => ClientError::Cancelled,
        TransportError::Network(_) => ClientError::Io {
            op: "static transport", kind: std::io::ErrorKind::Other,
        },
    }
}

fn fetch_target(index: &Index, fetch: &StaticFetch) -> ClientResult<(String, [u8; 32], FileMeta)> {
    let (path, digest, expected_len) = match fetch {
        StaticFetch::AssetManifest(id) => (crate::wire::path_revision(id), *id.as_bytes(), None),
        StaticFetch::GameManifest(id) => (crate::wire::path_game_revision(id), *id.as_bytes(), None),
        StaticFetch::DerivedVariant(id) => (crate::wire::path_derived_variant(id), *id.as_bytes(), None),
        StaticFetch::VariantSet(id) => (crate::wire::path_variant_set(id), *id.as_bytes(), None),
        StaticFetch::Blob { blob, expected_len, .. } => {
            let meta = index.blobs.get(blob).filter(|meta| meta.present)
                .ok_or(ClientError::NotFound { what: "static blob" })?;
            if expected_len.is_some_and(|length| length != meta.byte_len) {
                return Err(ClientError::SizeMismatch {
                    what: "static blob declaration", expected: expected_len.unwrap(), found: meta.byte_len,
                });
            }
            (meta.path.clone(), *blob.as_bytes(), Some(meta.byte_len))
        }
    };
    let file = index.files.get(&path).cloned().ok_or(ClientError::NotFound {
        what: "static route metadata",
    })?;
    if file.sha256 != digest {
        return Err(ClientError::DigestMismatch {
            what: "static route metadata", expected: digest, found: file.sha256,
        });
    }
    if expected_len.is_some_and(|length| length != file.byte_len) {
        return Err(ClientError::SizeMismatch {
            what: "static route length", expected: expected_len.unwrap(), found: file.byte_len,
        });
    }
    Ok((path, digest, file))
}

fn decode_fetch(fetch: &StaticFetch, content: BlobContent) -> ClientResult<StaticFetchOutput> {
    let bytes = content_bytes(&content)?;
    Ok(match fetch {
        StaticFetch::AssetManifest(id) => {
            let manifest = AssetManifest::from_canonical_bytes(&bytes)?;
            if AssetRevisionId::hash_of(&bytes) != *id {
                return Err(ClientError::Protocol { what: "asset revision identity" });
            }
            StaticFetchOutput::AssetManifest(Box::new(manifest))
        }
        StaticFetch::GameManifest(id) => {
            let manifest = GameRevisionManifest::from_canonical_bytes(&bytes)?;
            if GameRevisionId::hash_of(&bytes) != *id {
                return Err(ClientError::Protocol { what: "game revision identity" });
            }
            StaticFetchOutput::GameManifest(Box::new(manifest))
        }
        StaticFetch::DerivedVariant(id) => {
            let manifest = DerivedVariantManifest::from_canonical_bytes(&bytes)?;
            if DerivedVariantId::hash_of(&bytes) != *id {
                return Err(ClientError::Protocol { what: "derived variant identity" });
            }
            StaticFetchOutput::DerivedVariant(Box::new(manifest))
        }
        StaticFetch::VariantSet(id) => {
            let manifest = VariantSetManifest::from_canonical_bytes(&bytes)?;
            if VariantSetId::hash_of(&bytes) != *id {
                return Err(ClientError::Protocol { what: "variant set identity" });
            }
            StaticFetchOutput::VariantSet(Box::new(manifest))
        }
        StaticFetch::Blob { blob, .. } => StaticFetchOutput::Blob { blob: *blob, content },
    })
}

fn content_bytes(content: &BlobContent) -> ClientResult<Vec<u8>> {
    content.read_all()
}

fn parse_index(bytes: &[u8], health_bytes: &[u8]) -> ClientResult<Index> {
    if bytes.len() as u64 > MAX_STATIC_MANIFEST_BYTES {
        return Err(ClientError::OverBudget {
            what: "static manifest", limit: MAX_STATIC_MANIFEST_BYTES, found: bytes.len() as u64,
        });
    }
    let root = crate::json::parse(bytes).map_err(|_| ClientError::Protocol {
        what: "static manifest json",
    })?;
    need_object(&root, "static manifest")?;
    if need_u64(&root, "static_version", "static version")? != 1 {
        return Err(ClientError::Protocol { what: "static version unsupported" });
    }
    if need_u64(&root, "protocol_version", "static protocol")? != crate::wire::PROTOCOL_VERSION as u64 {
        return Err(ClientError::Protocol { what: "server protocol version unsupported" });
    }
    let snapshot_id = need_str(&root, "snapshot_id", 32, "static snapshot id")?.to_string();
    let _: [u8; 16] = from_hex_exact(&snapshot_id).ok_or(ClientError::Protocol {
        what: "static snapshot id",
    })?;
    let server_text = need_str(&root, "server_id", 32, "static server id")?;
    let server_id = from_hex_exact(server_text).ok_or(ClientError::Protocol {
        what: "static server id",
    })?;
    need_u64(&root, "generated_ms", "static generated time")?;
    validate_policy(need(&root, "policy", "static policy")?)?;
    validate_exclusions(need(&root, "exclusions", "static exclusions")?)?;

    let files = parse_files(need_array(&root, "files", "static files")?)?;
    let health_file = files.get("/v1/health").ok_or(ClientError::Protocol {
        what: "static health metadata missing",
    })?;
    verify_bytes("static health", health_file, health_bytes)?;
    let health_value = crate::json::parse(health_bytes).map_err(|_| ClientError::Protocol {
        what: "static health json",
    })?;
    let health = dto::parse_health(&health_value)?;
    if health.server_id != server_id {
        return Err(ClientError::ServerIdentityMismatch { expected: server_id, found: health.server_id });
    }
    if health.protocol_version != crate::wire::PROTOCOL_VERSION {
        return Err(ClientError::Protocol { what: "static health protocol" });
    }

    let assets = parse_assets(need_array(&root, "assets", "static assets")?, &files)?;
    let aliases = parse_aliases(need_array(&root, "aliases", "static aliases")?, &files)?;
    let revisions = parse_revisions(need_array(&root, "revisions", "static revisions")?, &files)?;
    let search_root = need(&root, "search", "static search")?;
    if need_str(search_root, "normalization", 64, "static normalization")? != "ascii-alnum-lower-v1" {
        return Err(ClientError::Protocol { what: "static normalization" });
    }
    if need_str(search_root, "ranking", 64, "static ranking")? != "public-weight-sum-v1" {
        return Err(ClientError::Protocol { what: "static ranking" });
    }
    let search = parse_search(need_array(search_root, "documents", "static search documents")?)?;
    let blobs = parse_blobs(need_array(&root, "blobs", "static blobs")?, &files)?;
    validate_references(&files, &assets, &aliases, &revisions, &search, &blobs)?;
    validate_variants(need_array(&root, "variants", "static variants")?, &files, &revisions)?;
    validate_totals(
        need(&root, "totals", "static totals")?,
        &assets,
        &aliases,
        &revisions,
        &blobs,
    )?;
    Ok(Index { snapshot_id, server_id, files, assets, aliases, revisions, search, blobs })
}

fn parse_files(values: &[crate::json::Value]) -> ClientResult<BTreeMap<String, FileMeta>> {
    bounded_items(values, "static files")?;
    let mut out = BTreeMap::new();
    let mut previous = None::<String>;
    for value in values {
        let path = need_str(value, "path", crate::wire::MAX_TARGET_BYTES, "static file path")?.to_string();
        validate_path(&path)?;
        if previous.as_ref().is_some_and(|old| old >= &path) {
            return Err(ClientError::Protocol { what: "static file ids unsorted or duplicate" });
        }
        previous = Some(path.clone());
        let byte_len = need_u64(value, "byte_len", "static file length")?;
        if byte_len > crate::transport::MAX_TRANSPORT_BODY_BYTES {
            return Err(ClientError::OverBudget {
                what: "static file", limit: crate::transport::MAX_TRANSPORT_BODY_BYTES, found: byte_len,
            });
        }
        let sha256 = from_hex_exact(need_str(value, "sha256", 64, "static file sha256")?)
            .ok_or(ClientError::Protocol { what: "static file sha256" })?;
        let content_type = need_str(value, "content_type", 128, "static file content type")?;
        if content_type.is_empty() || content_type.chars().any(char::is_control) {
            return Err(ClientError::Protocol { what: "static file content type" });
        }
        match value.get("content_encoding") {
            Some(crate::json::Value::Null) => {}
            Some(v) if v.as_str() == Some("br") && path.ends_with(".br") => {}
            _ => return Err(ClientError::Protocol { what: "static file content encoding" }),
        }
        out.insert(path, FileMeta { byte_len, sha256 });
    }
    Ok(out)
}

fn parse_assets(
    values: &[crate::json::Value], files: &BTreeMap<String, FileMeta>,
) -> ClientResult<BTreeMap<AssetId, AssetMeta>> {
    bounded_items(values, "static assets")?;
    let mut out = BTreeMap::new();
    let mut previous = None;
    for value in values {
        let asset_id = AssetId::from_str(need_str(value, "asset_id", 64, "static asset id")?)
            .map_err(|_| ClientError::Protocol { what: "static asset id" })?;
        sorted_id(&mut previous, asset_id, "static asset ids unsorted or duplicate")?;
        let namespace = checked_text(value, "namespace", crate::wire::MAX_NAMESPACE_BYTES, "static namespace")?;
        let created_ms = need_u64(value, "created_ms", "static asset created")?;
        let revisions = parse_sorted_ids(
            need_array(value, "revisions", "static asset revisions")?,
            "static asset revision", AssetRevisionId::from_str,
        )?;
        if revisions.is_empty() {
            return Err(ClientError::Protocol { what: "static asset revisions empty" });
        }
        require_file(files, &crate::wire::path_asset(&asset_id))?;
        out.insert(asset_id, AssetMeta { row: AssetRow { asset_id, namespace, created_ms }, revisions });
    }
    Ok(out)
}

fn parse_aliases(
    values: &[crate::json::Value], files: &BTreeMap<String, FileMeta>,
) -> ClientResult<BTreeMap<AssetAlias, AliasMeta>> {
    bounded_items(values, "static aliases")?;
    let mut out = BTreeMap::new();
    let mut previous = String::new();
    for value in values {
        let dto = dto::parse_alias(value)?;
        let spelling = dto.alias.to_string();
        if !previous.is_empty() && previous >= spelling {
            return Err(ClientError::Protocol { what: "static alias ids unsorted or duplicate" });
        }
        previous = spelling;
        require_file(files, &crate::wire::path_alias(&dto.alias))?;
        let updated_ms = need_u64(value, "updated_ms", "static alias updated")?;
        out.insert(dto.alias.clone(), AliasMeta { dto, updated_ms });
    }
    Ok(out)
}

fn parse_revisions(
    values: &[crate::json::Value], files: &BTreeMap<String, FileMeta>,
) -> ClientResult<BTreeMap<AssetRevisionId, RevisionMeta>> {
    bounded_items(values, "static revisions")?;
    let mut out = BTreeMap::new();
    let mut previous = None;
    for value in values {
        let revision = AssetRevisionId::from_str(need_str(value, "revision", 80, "static revision")?)
            .map_err(|_| ClientError::Protocol { what: "static revision" })?;
        sorted_id(&mut previous, revision, "static revision ids unsorted or duplicate")?;
        let path = crate::wire::path_revision(&revision);
        let file = require_file(files, &path)?;
        if file.sha256 != *revision.as_bytes() {
            return Err(ClientError::Protocol { what: "static revision route digest" });
        }
        let document = need(value, "document", "static revision document")?;
        let asset_id = AssetId::from_str(need_str(document, "asset_id", 64, "static revision asset")?)
            .map_err(|_| ClientError::Protocol { what: "static revision asset" })?;
        let mut blobs = Vec::new();
        let mut sources = Vec::new();
        for entry in need_array(document, "files", "static revision files")? {
            let blob = BlobId::from_str(need_str(entry, "blob", 80, "static revision blob")?)
                .map_err(|_| ClientError::Protocol { what: "static revision blob" })?;
            let byte_len = need_u64(entry, "byte_len", "static revision blob length")?;
            if need_str(entry, "role", 64, "static revision role")? == "source" { sources.push(blob); }
            blobs.push((blob, byte_len));
        }
        let thumbnail = if let Some(thumbnail) = document.get("thumbnail") {
            let blob = BlobId::from_str(need_str(thumbnail, "blob", 80, "static thumbnail blob")?)
                .map_err(|_| ClientError::Protocol { what: "static thumbnail blob" })?;
            let byte_len = need_u64(thumbnail, "byte_len", "static thumbnail length")?;
            blobs.push((blob, byte_len));
            let thumbnail_path = format!("/v1/thumbnails/revision/{revision}");
            if let Some(route) = files.get(&thumbnail_path) {
                if route.byte_len != blobs.last().unwrap().1 || route.sha256 != *blob.as_bytes() {
                    return Err(ClientError::Protocol { what: "static thumbnail metadata" });
                }
            }
            Some((blob, byte_len))
        } else {
            None
        };
        let mut dependencies = Vec::new();
        for dependency in need_array(document, "dependencies", "static dependencies")? {
            let dep_asset = AssetId::from_str(need_str(dependency, "asset_id", 64, "static dependency asset")?)
                .map_err(|_| ClientError::Protocol { what: "static dependency asset" })?;
            let dep_revision = AssetRevisionId::from_str(need_str(dependency, "revision", 80, "static dependency revision")?)
                .map_err(|_| ClientError::Protocol { what: "static dependency revision" })?;
            dependencies.push((dep_asset, dep_revision));
        }
        out.insert(revision, RevisionMeta { asset_id, sources, blobs, thumbnail, dependencies });
    }
    Ok(out)
}

fn parse_search(values: &[crate::json::Value]) -> ClientResult<Vec<SearchDoc>> {
    bounded_items(values, "static search documents")?;
    let mut out = Vec::new();
    let mut previous = None;
    for value in values {
        let asset_id = AssetId::from_str(need_str(value, "asset_id", 64, "static search asset")?)
            .map_err(|_| ClientError::Protocol { what: "static search asset" })?;
        sorted_id(&mut previous, asset_id, "static search ids unsorted or duplicate")?;
        let kind = match value.get("kind") {
            None | Some(crate::json::Value::Null) => None,
            Some(value) => Some(dto::kind_parse(value.as_str().ok_or(ClientError::Protocol {
                what: "static search kind",
            })?).ok_or(ClientError::Protocol { what: "static search kind" })?),
        };
        let aliases = parse_sorted_ids(
            need_array(value, "aliases", "static search aliases")?,
            "static search alias", AssetAlias::from_str,
        )?;
        let categories = parse_strings(need_array(value, "categories", "static categories")?, "static category")?;
        let tags = parse_strings(need_array(value, "tags", "static tags")?, "static tag")?;
        checked_text(value, "generator", MAX_STATIC_STRING_BYTES, "static search generator")?;
        checked_text(value, "backend", MAX_STATIC_STRING_BYTES, "static search backend")?;
        checked_text(value, "model", MAX_STATIC_STRING_BYTES, "static search model")?;
        let terms_values = need_array(value, "terms", "static terms")?;
        bounded_items(terms_values, "static terms")?;
        let mut terms = Vec::new();
        let mut previous_term = String::new();
        for term in terms_values {
            let spelling = checked_text(term, "term", crate::wire::MAX_QUERY_TEXT_BYTES, "static term")?;
            if !previous_term.is_empty() && previous_term >= spelling {
                return Err(ClientError::Protocol { what: "static terms unsorted or duplicate" });
            }
            previous_term = spelling.clone();
            terms.push(SearchTerm { term: spelling, weight: need_u64(term, "weight", "static term weight")? });
        }
        out.push(SearchDoc {
            asset_id,
            namespace: checked_text(value, "namespace", crate::wire::MAX_NAMESPACE_BYTES, "static search namespace")?,
            kind,
            title: checked_text(value, "title", crate::wire::MAX_TITLE_BYTES, "static search title")?,
            description: checked_text(value, "description", MAX_STATIC_STRING_BYTES, "static search description")?,
            categories,
            tags,
            creator: checked_text(value, "creator", MAX_STATIC_STRING_BYTES, "static search creator")?,
            live: need_bool(value, "live", "static search live")?,
            updated_ms: need_u64(value, "updated_ms", "static search updated")?,
            aliases,
            terms,
        });
    }
    Ok(out)
}

fn parse_blobs(
    values: &[crate::json::Value], files: &BTreeMap<String, FileMeta>,
) -> ClientResult<BTreeMap<BlobId, BlobMeta>> {
    bounded_items(values, "static blobs")?;
    let mut out = BTreeMap::new();
    let mut previous = None;
    for value in values {
        let blob = BlobId::from_str(need_str(value, "blob", 80, "static blob")?)
            .map_err(|_| ClientError::Protocol { what: "static blob" })?;
        sorted_id(&mut previous, blob, "static blob ids unsorted or duplicate")?;
        let path = need_str(value, "path", crate::wire::MAX_TARGET_BYTES, "static blob path")?.to_string();
        if path != crate::wire::path_blob(&blob) { return Err(ClientError::Protocol { what: "static blob path" }); }
        let byte_len = need_u64(value, "byte_len", "static blob length")?;
        let sha = from_hex_exact::<32>(need_str(value, "sha256", 64, "static blob sha256")?)
            .ok_or(ClientError::Protocol { what: "static blob sha256" })?;
        if sha != *blob.as_bytes() { return Err(ClientError::Protocol { what: "static blob sha256 mismatch" }); }
        let present = need_bool(value, "present", "static blob present")?;
        match value.get("reason") {
            Some(crate::json::Value::Null) => {}
            Some(value) => {
                let reason = value.as_str().ok_or(ClientError::Protocol {
                    what: "static blob omission reason",
                })?;
                if reason.is_empty()
                    || reason.len() > MAX_STATIC_STRING_BYTES
                    || reason.chars().any(char::is_control)
                {
                    return Err(ClientError::Protocol {
                        what: "static blob omission reason",
                    });
                }
            }
            None => return Err(ClientError::Protocol { what: "static blob omission reason" }),
        }
        parse_strings(need_array(value, "media", "static blob media")?, "static blob media")?;
        parse_strings(need_array(value, "roles", "static blob roles")?, "static blob role")?;
        if present {
            let file = require_file(files, &path)?;
            if file.byte_len != byte_len || file.sha256 != *blob.as_bytes() {
                return Err(ClientError::Protocol { what: "static blob route metadata" });
            }
        } else if files.contains_key(&path) {
            return Err(ClientError::Protocol { what: "omitted static blob has route" });
        }
        out.insert(blob, BlobMeta { byte_len, present, path });
    }
    Ok(out)
}

fn validate_references(
    files: &BTreeMap<String, FileMeta>, assets: &BTreeMap<AssetId, AssetMeta>,
    aliases: &BTreeMap<AssetAlias, AliasMeta>,
    revisions: &BTreeMap<AssetRevisionId, RevisionMeta>, search: &[SearchDoc],
    blobs: &BTreeMap<BlobId, BlobMeta>,
) -> ClientResult<()> {
    for alias in aliases.values() {
        let asset = assets.get(&alias.dto.asset_id).ok_or(ClientError::Protocol {
            what: "static alias asset reference",
        })?;
        if !asset.revisions.contains(&alias.dto.head_revision)
            || !revisions.contains_key(&alias.dto.head_revision)
        {
            return Err(ClientError::Protocol { what: "static alias revision reference" });
        }
        if let Some((blob, byte_len)) = revisions[&alias.dto.head_revision].thumbnail {
            let path = format!("/v1/thumbnails/alias/{}", alias.dto.alias);
            let route = files.get(&path);
            let present = blobs.get(&blob).is_some_and(|metadata| metadata.present);
            match (present, route) {
                (true, Some(route))
                    if route.byte_len == byte_len && route.sha256 == *blob.as_bytes() => {}
                (false, None) => {}
                _ => return Err(ClientError::Protocol { what: "static alias thumbnail reference" }),
            }
        }
    }
    for (asset_id, asset) in assets {
        for revision in &asset.revisions {
            let metadata = revisions.get(revision).ok_or(ClientError::Protocol {
                what: "static asset revision reference",
            })?;
            if metadata.asset_id != *asset_id {
                return Err(ClientError::Protocol {
                    what: "static asset revision owner",
                });
            }
        }
    }
    for (revision_id, revision) in revisions {
        for (blob, length) in &revision.blobs {
            let meta = blobs.get(blob).ok_or(ClientError::Protocol { what: "static blob reference" })?;
            if meta.byte_len != *length {
                return Err(ClientError::SizeMismatch { what: "static blob reference", expected: *length, found: meta.byte_len });
            }
        }
        for (asset, dependency) in &revision.dependencies {
            let meta = revisions.get(dependency).ok_or(ClientError::Protocol {
                what: "static dependency reference",
            })?;
            if meta.asset_id != *asset {
                return Err(ClientError::Protocol { what: "static dependency asset mismatch" });
            }
        }
        if let Some((blob, byte_len)) = revision.thumbnail {
            let metadata = blobs.get(&blob).ok_or(ClientError::Protocol {
                what: "static thumbnail blob reference",
            })?;
            let path = format!("/v1/thumbnails/revision/{revision_id}");
            let route = files.get(&path);
            match (metadata.present, route) {
                (true, Some(route))
                    if route.byte_len == byte_len && route.sha256 == *blob.as_bytes() => {}
                (false, None) => {}
                _ => return Err(ClientError::Protocol { what: "static revision thumbnail reference" }),
            }
        }
    }
    for doc in search {
        if !assets.contains_key(&doc.asset_id) {
            return Err(ClientError::Protocol { what: "static search asset reference" });
        }
        for alias in &doc.aliases {
            if aliases.get(alias).is_none_or(|entry| entry.dto.asset_id != doc.asset_id) {
                return Err(ClientError::Protocol { what: "static search alias reference" });
            }
        }
    }
    if search.len() != assets.len()
        || assets.keys().any(|asset| search.binary_search_by_key(asset, |doc| doc.asset_id).is_err())
    {
        return Err(ClientError::Protocol { what: "static search projection incomplete" });
    }
    Ok(())
}

fn validate_policy(value: &crate::json::Value) -> ClientResult<()> {
    need_object(value, "static policy")?;
    match value.get("namespace") {
        Some(crate::json::Value::Null) => {}
        Some(value) => {
            let namespace = value.as_str().ok_or(ClientError::Protocol {
                what: "static policy namespace",
            })?;
            if namespace.is_empty()
                || namespace.len() > crate::wire::MAX_NAMESPACE_BYTES
                || !crate::wire::query_value_ok(namespace)
            {
                return Err(ClientError::Protocol { what: "static policy namespace" });
            }
        }
        None => return Err(ClientError::Protocol { what: "static policy namespace" }),
    }
    match value.get("kind") {
        Some(crate::json::Value::Null) => {}
        Some(value) => {
            let kind = value.as_str().and_then(dto::kind_parse);
            if kind.is_none() {
                return Err(ClientError::Protocol { what: "static policy kind" });
            }
        }
        None => return Err(ClientError::Protocol { what: "static policy kind" }),
    }
    for key in ["limit", "max_bytes_per_asset", "max_total_bytes"] {
        optional_u64(value, key, "static policy budget")?;
    }
    need_u64(value, "include_video_up_to", "static video policy")?;
    Ok(())
}

fn validate_exclusions(value: &crate::json::Value) -> ClientResult<()> {
    need_object(value, "static exclusions")?;
    for key in ["rights", "budget", "kind_mismatch"] {
        need_u64(value, key, "static exclusion count")?;
    }
    Ok(())
}

fn validate_totals(
    value: &crate::json::Value,
    assets: &BTreeMap<AssetId, AssetMeta>,
    aliases: &BTreeMap<AssetAlias, AliasMeta>,
    revisions: &BTreeMap<AssetRevisionId, RevisionMeta>,
    blobs: &BTreeMap<BlobId, BlobMeta>,
) -> ClientResult<()> {
    need_object(value, "static totals")?;
    let present = blobs.values().filter(|blob| blob.present).count() as u64;
    let omitted = blobs.len() as u64 - present;
    let bytes = blobs
        .values()
        .filter(|blob| blob.present)
        .try_fold(0u64, |sum, blob| sum.checked_add(blob.byte_len))
        .ok_or(ClientError::Protocol { what: "static total blob bytes" })?;
    for (key, expected) in [
        ("assets", assets.len() as u64),
        ("aliases", aliases.len() as u64),
        ("revisions", revisions.len() as u64),
        ("blobs_present", present),
        ("blobs_omitted", omitted),
        ("unique_blob_bytes", bytes),
    ] {
        if need_u64(value, key, "static totals")? != expected {
            return Err(ClientError::Protocol { what: "static totals mismatch" });
        }
    }
    Ok(())
}

fn validate_variants(
    values: &[crate::json::Value], files: &BTreeMap<String, FileMeta>,
    revisions: &BTreeMap<AssetRevisionId, RevisionMeta>,
) -> ClientResult<()> {
    bounded_items(values, "static variants")?;
    let mut previous = None::<(AssetRevisionId, VariantSetId)>;
    for value in values {
        let base = AssetRevisionId::from_str(need_str(value, "base_revision", 80, "static variant base")?)
            .map_err(|_| ClientError::Protocol { what: "static variant base" })?;
        let set = VariantSetId::from_str(need_str(value, "variant_set", 80, "static variant set")?)
            .map_err(|_| ClientError::Protocol { what: "static variant set" })?;
        if previous.is_some_and(|old| old >= (base, set)) {
            return Err(ClientError::Protocol { what: "static variants unsorted or duplicate" });
        }
        previous = Some((base, set));
        if !revisions.contains_key(&base) { return Err(ClientError::Protocol { what: "static variant base reference" }); }
        let route = require_file(files, &crate::wire::path_variant_set(&set))?;
        if route.sha256 != *set.as_bytes() { return Err(ClientError::Protocol { what: "static variant set digest" }); }
        let variants = parse_sorted_ids(
            need_array(value, "variants", "static derived variants")?,
            "static derived variant", DerivedVariantId::from_str,
        )?;
        for variant in variants {
            let route = require_file(files, &crate::wire::path_derived_variant(&variant))?;
            if route.sha256 != *variant.as_bytes() {
                return Err(ClientError::Protocol { what: "static derived variant digest" });
            }
        }
    }
    Ok(())
}

fn match_search(doc: &SearchDoc, query: &CatalogQuery, terms: &[String]) -> Option<u64> {
    if query.namespace.as_ref().is_some_and(|v| &doc.namespace != v)
        || query.kind.is_some_and(|v| doc.kind != Some(v))
        || query.category.as_ref().is_some_and(|v| !doc.categories.contains(v))
        || query.tag.as_ref().is_some_and(|v| !doc.tags.contains(v))
        || query.exclude_tag.as_ref().is_some_and(|v| doc.tags.contains(v))
        || query.creator.as_ref().is_some_and(|v| &doc.creator != v)
        || (query.live_only && !doc.live)
    { return None; }
    let mut score = 0u64;
    for term in terms {
        let weight = doc.terms.iter().find(|candidate| candidate.term == *term)?.weight;
        score = score.saturating_add(weight);
    }
    Some(score)
}

fn build_facets(matches: &[(&SearchDoc, u64)], limit: u32) -> Vec<CatalogFacet> {
    if limit == 0 { return Vec::new(); }
    let mut counts: BTreeMap<(u8, String), u64> = BTreeMap::new();
    for (doc, _) in matches {
        for category in &doc.categories { *counts.entry((0, category.clone())).or_default() += 1; }
        for tag in &doc.tags { *counts.entry((1, tag.clone())).or_default() += 1; }
    }
    let mut rows: Vec<_> = counts.into_iter().collect();
    rows.sort_by(|((kind_a, label_a), count_a), ((kind_b, label_b), count_b)| {
        count_b.cmp(count_a).then(kind_a.cmp(kind_b)).then(label_a.cmp(label_b))
    });
    rows.truncate(limit as usize);
    rows.into_iter().map(|((kind, label), count)| CatalogFacet {
        kind: if kind == 0 { FacetKind::Category } else { FacetKind::Tag }, label, count,
    }).collect()
}

fn tokenize(text: &str) -> Vec<String> {
    let mut terms = BTreeSet::new();
    let mut current = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() { current.push((byte as char).to_ascii_lowercase()); }
        else if !current.is_empty() { terms.insert(std::mem::take(&mut current)); }
    }
    if !current.is_empty() { terms.insert(current); }
    terms.into_iter().collect()
}

fn snippet(title: &str, description: &str) -> String {
    let source = if description.is_empty() { title } else { description };
    if source.len() <= 512 { return source.to_string(); }
    let mut end = 512;
    while !source.is_char_boundary(end) { end -= 1; }
    source[..end].to_string()
}

fn query_fingerprint(query: &CatalogQuery) -> [u8; 32] {
    digest_text(&format!(
        "q\0{}\0{:?}\0{:?}\0{:?}\0{:?}\0{:?}\0{:?}\0{}\0{}\0{}",
        query.text, query.namespace, query.kind, query.category, query.tag, query.exclude_tag,
        query.creator, query.live_only, query.page_size, query.facets,
    ))
}

fn make_cursor(index: &Index, fingerprint: &[u8; 32], offset: usize) -> PageCursor {
    PageCursor {
        server_id: index.server_id,
        token: format!("s1.{}.{}.{}", index.snapshot_id, hex(fingerprint), offset),
    }
}

fn cursor_offset(index: &Index, cursor: Option<&PageCursor>, fingerprint: &[u8; 32]) -> ClientResult<usize> {
    let Some(cursor) = cursor else { return Ok(0) };
    if cursor.server_id() != &index.server_id { return Err(ClientError::WrongServerCursor); }
    let expected = format!("s1.{}.{}.", index.snapshot_id, hex(fingerprint));
    let offset = cursor.token().strip_prefix(&expected).ok_or(ClientError::WrongServerCursor)?;
    if offset.is_empty() || (offset.len() > 1 && offset.starts_with('0')) || !offset.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ClientError::WrongServerCursor);
    }
    offset.parse().map_err(|_| ClientError::WrongServerCursor)
}

fn parse_sorted_ids<T: Ord + Clone>(
    values: &[crate::json::Value], what: &'static str,
    parse: impl Fn(&str) -> Result<T, makepad_asset_data::AssetDataError>,
) -> ClientResult<Vec<T>> {
    bounded_items(values, what)?;
    let mut out = Vec::with_capacity(values.len());
    let mut previous = None;
    for value in values {
        let text = value.as_str().ok_or(ClientError::Protocol { what })?;
        let id = parse(text).map_err(|_| ClientError::Protocol { what })?;
        sorted_id(&mut previous, id.clone(), what)?;
        out.push(id);
    }
    Ok(out)
}

fn sorted_id<T: Ord + Clone>(previous: &mut Option<T>, id: T, what: &'static str) -> ClientResult<()> {
    if previous.as_ref().is_some_and(|old| old >= &id) { return Err(ClientError::Protocol { what }); }
    *previous = Some(id);
    Ok(())
}

fn parse_strings(values: &[crate::json::Value], what: &'static str) -> ClientResult<Vec<String>> {
    bounded_items(values, what)?;
    let mut out = Vec::with_capacity(values.len());
    let mut previous = String::new();
    for value in values {
        let text = value.as_str().ok_or(ClientError::Protocol { what })?;
        if text.is_empty() || text.len() > crate::wire::MAX_FILTER_VALUE_BYTES || text.chars().any(char::is_control) {
            return Err(ClientError::Protocol { what });
        }
        if !previous.is_empty() && previous.as_str() >= text { return Err(ClientError::Protocol { what }); }
        previous = text.to_string();
        out.push(previous.clone());
    }
    Ok(out)
}

fn validate_path(path: &str) -> ClientResult<()> {
    if !path.starts_with("/v1/") || path.contains('?') || path.contains("//")
        || path.split('/').any(|part| part == "." || part == "..")
        || path.len() > crate::wire::MAX_TARGET_BYTES
        || !path.bytes().all(crate::wire::target_byte_ok)
    { return Err(ClientError::Protocol { what: "static file path" }); }
    Ok(())
}

fn require_file<'a>(files: &'a BTreeMap<String, FileMeta>, path: &str) -> ClientResult<&'a FileMeta> {
    files.get(path).ok_or(ClientError::Protocol { what: "static route metadata missing" })
}

fn verify_bytes(what: &'static str, file: &FileMeta, bytes: &[u8]) -> ClientResult<()> {
    if bytes.len() as u64 != file.byte_len {
        return Err(ClientError::SizeMismatch { what, expected: file.byte_len, found: bytes.len() as u64 });
    }
    let found = hash(bytes);
    if found != file.sha256 {
        return Err(ClientError::DigestMismatch { what, expected: file.sha256, found });
    }
    Ok(())
}

fn bounded_items(values: &[crate::json::Value], what: &'static str) -> ClientResult<()> {
    if values.len() > MAX_STATIC_ITEMS {
        return Err(ClientError::OverBudget { what, limit: MAX_STATIC_ITEMS as u64, found: values.len() as u64 });
    }
    Ok(())
}

fn need<'a>(value: &'a crate::json::Value, key: &str, what: &'static str) -> ClientResult<&'a crate::json::Value> {
    value.get(key).ok_or(ClientError::Protocol { what })
}

fn need_object<'a>(value: &'a crate::json::Value, what: &'static str) -> ClientResult<&'a [(String, crate::json::Value)]> {
    match value { crate::json::Value::Obj(values) => Ok(values), _ => Err(ClientError::Protocol { what }) }
}

fn need_array<'a>(value: &'a crate::json::Value, key: &str, what: &'static str) -> ClientResult<&'a [crate::json::Value]> {
    need(value, key, what)?.as_arr().ok_or(ClientError::Protocol { what })
}

fn need_str<'a>(value: &'a crate::json::Value, key: &str, max: usize, what: &'static str) -> ClientResult<&'a str> {
    let text = need(value, key, what)?.as_str().ok_or(ClientError::Protocol { what })?;
    if text.len() > max { return Err(ClientError::Protocol { what }); }
    Ok(text)
}

fn checked_text(value: &crate::json::Value, key: &str, max: usize, what: &'static str) -> ClientResult<String> {
    let text = need_str(value, key, max, what)?;
    if text.chars().any(char::is_control) { return Err(ClientError::Protocol { what }); }
    Ok(text.to_string())
}

fn need_u64(value: &crate::json::Value, key: &str, what: &'static str) -> ClientResult<u64> {
    need(value, key, what)?.as_u64().ok_or(ClientError::Protocol { what })
}

fn optional_u64(
    value: &crate::json::Value,
    key: &str,
    what: &'static str,
) -> ClientResult<Option<u64>> {
    match value.get(key) {
        Some(crate::json::Value::Null) => Ok(None),
        Some(crate::json::Value::Int(number)) if *number >= 0 => Ok(Some(*number as u64)),
        _ => Err(ClientError::Protocol { what }),
    }
}

fn need_bool(value: &crate::json::Value, key: &str, what: &'static str) -> ClientResult<bool> {
    need(value, key, what)?.as_bool().ok_or(ClientError::Protocol { what })
}

fn hash(bytes: &[u8]) -> [u8; 32] { let mut hasher = Sha256::new(); hasher.update(bytes); hasher.finalize() }
fn digest_text(text: &str) -> [u8; 32] { hash(text.as_bytes()) }
fn hex(bytes: &[u8]) -> String { bytes.iter().map(|byte| format!("{byte:02x}")).collect() }
