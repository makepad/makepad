//! Single-threaded completion pump used by static web sessions.

use crate::api::{BlobHead, CatalogQuery, GcRequest, SourceCollectionRegistered};
use crate::cache_store::BlobContent;
use crate::client::{AssetClient, AssetsPage, CatalogPage, ClientConfig, PageCursor};
use crate::dto::{
    AliasDto, AliasStatusDto, AssetDetailDto, GameAliasDto, ImportReportDto, ImportStatusDto,
    SourceCollectionRowDto,
};
use crate::error::{ClientError, ClientResult};
use crate::publish::{PublishBundle, PublishRequest, PublishStage, Published, PublishedBundle};
use crate::resolver::{ResolvedFile, ResolvedThumbnail, TierPreference};
use crate::side_channels::{SideChannelFile, SideChannelOutcome};
use crate::static_store::{
    StaticFetch, StaticFetchId, StaticFetchOutput, StaticStore, StaticStoreEvent, StaticStoreState,
};
use makepad_asset_data::{
    AssetAlias, AssetId, AssetManifest, AssetRevisionId, AssetRevisionRef, BlobId, ClientProfile,
    DerivedVariantId, DerivedVariantManifest, FileRole, GameAlias, GameRevisionId,
    GameRevisionManifest, ImportRevisionId, ResolvedVariantMap, VariantSetId, VariantSetManifest,
};
use std::collections::{HashMap, HashSet, VecDeque};
use std::cell::RefCell;
use std::sync::Arc;

pub type RequestId = u64;

#[derive(Clone, Debug)]
pub enum ClientRequest {
    CatalogSearch { query: CatalogQuery, cursor: Option<PageCursor> },
    AssetsPage { namespace: Option<String>, cursor: Option<PageCursor>, limit: u64 },
    AssetDetail { id: AssetId },
    ResolveAlias { alias: AssetAlias },
    AliasStatus { entries: Vec<(AssetAlias, Option<BlobId>)>, tags: Vec<String> },
    ResolveGameAlias { alias: GameAlias },
    FetchAssetManifest { rev: AssetRevisionId },
    FetchGameManifest { rev: GameRevisionId },
    FetchBlob { blob: BlobId, expected_len: Option<u64>, pin: bool },
    HeadBlob { blob: BlobId },
    UnpinBlob { blob: BlobId },
    ResolveFile { manifest: Box<AssetManifest>, role: FileRole, tier: TierPreference, max_lod: u8 },
    ResolveThumbnail { manifest: Box<AssetManifest> },
    RetireAsset { id: AssetId },
    RetireRevision { id: AssetId, revision: AssetRevisionId },
    GcBlobs { request: GcRequest },
    GcStatus,
    GcCancel,
    PublishArtifact { request: Box<PublishRequest> },
    PublishBundle { request: Box<PublishBundle> },
    PublishSideChannels { asset: AssetId, files: Arc<Vec<SideChannelFile>> },
    RegisterSourceCollection { bytes: Vec<u8> },
    ListSourceCollections,
    RunImport { bytes: Vec<u8> },
    FetchImport { revision: ImportRevisionId },
    FetchDerivedVariant { id: DerivedVariantId },
    FreezeVariantSet { base: AssetRevisionRef, variants: Vec<DerivedVariantId> },
    FetchVariantSet { id: VariantSetId },
    ResolveVariantSet { set: VariantSetId, profile: ClientProfile },
}

#[derive(Clone, Debug)]
pub enum ClientOutput {
    CatalogPage(CatalogPage),
    AssetsPage(AssetsPage),
    AssetDetail(AssetDetailDto),
    Alias(AliasDto),
    AliasStatus(Vec<AliasStatusDto>),
    GameAlias(GameAliasDto),
    AssetManifest(Box<AssetManifest>),
    GameManifest(Box<GameRevisionManifest>),
    Blob { blob: BlobId, content: BlobContent },
    BlobHead { blob: BlobId, head: BlobHead },
    BlobUnpinned { blob: BlobId },
    File(ResolvedFile),
    Thumbnail(Option<ResolvedThumbnail>),
    Retired(crate::dto::RetireDto),
    Gc(crate::dto::GcStatusDto),
    GcCancelled(bool),
    Published(Published),
    PublishedBundle(PublishedBundle),
    SideChannels(SideChannelOutcome),
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
    Progress { id: RequestId, bytes: u64, total: u64 },
    Done { id: RequestId, output: ClientOutput },
    Failed { id: RequestId, error: ClientError },
}

impl ClientEvent {
    pub fn id(&self) -> RequestId {
        match self {
            Self::Started { id } | Self::Progress { id, .. } | Self::Done { id, .. }
            | Self::Failed { id, .. } => *id,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct StageEvent { pub id: RequestId, pub stage: PublishStage }

#[derive(Clone, Debug, Default)]
pub enum ResourceState<T> {
    #[default]
    Idle,
    Loading { progress: Option<(u64, u64)> },
    Ready(T),
    Failed(ClientError),
}

impl<T> ResourceState<T> {
    pub fn is_loading(&self) -> bool { matches!(self, Self::Loading { .. }) }
    pub fn is_ready(&self) -> bool { matches!(self, Self::Ready(_)) }
    pub fn ready(&self) -> Option<&T> { if let Self::Ready(value) = self { Some(value) } else { None } }
    pub fn failed(&self) -> Option<&ClientError> { if let Self::Failed(error) = self { Some(error) } else { None } }
}

#[derive(Clone, Debug, Default)]
pub struct ResourceSlot<T> { request: Option<RequestId>, pub state: ResourceState<T> }

impl<T> ResourceSlot<T> {
    pub fn begin(&mut self, id: RequestId) {
        self.request = Some(id);
        self.state = ResourceState::Loading { progress: None };
    }
    pub fn request(&self) -> Option<RequestId> { self.request }
    pub fn on_event(&mut self, event: &ClientEvent, extract: impl FnOnce(&ClientOutput) -> Option<T>) -> bool {
        if self.request != Some(event.id()) { return false; }
        match event {
            ClientEvent::Started { .. } => {}
            ClientEvent::Progress { bytes, total, .. } => self.state = ResourceState::Loading { progress: Some((*bytes, *total)) },
            ClientEvent::Done { output, .. } => {
                self.request = None;
                self.state = extract(output).map(ResourceState::Ready).unwrap_or_else(|| {
                    ResourceState::Failed(ClientError::Protocol { what: "unexpected output for slot" })
                });
            }
            ClientEvent::Failed { error, .. } => {
                self.request = None;
                self.state = ResourceState::Failed(error.clone());
            }
        }
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lane { Fast, Bulk }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SubmitOptions { pub lane: Option<Lane>, pub lifo: bool }

impl SubmitOptions {
    pub fn fast() -> Self { Self { lane: Some(Lane::Fast), lifo: false } }
    pub fn bulk() -> Self { Self { lane: Some(Lane::Bulk), lifo: false } }
    pub fn newest_first() -> Self { Self { lane: None, lifo: true } }
    pub fn with_lane(mut self, lane: Lane) -> Self { self.lane = Some(lane); self }
    pub fn with_lifo(mut self, lifo: bool) -> Self { self.lifo = lifo; self }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeConfig {
    pub fast_workers: usize,
    pub bulk_workers: usize,
    pub fast_blob_max_bytes: u64,
    pub fast_batch_max_items: usize,
}

impl RuntimeConfig {
    pub fn default_v1() -> Self {
        Self { fast_workers: 4, bulk_workers: 2, fast_blob_max_bytes: 512 * 1024, fast_batch_max_items: 16 }
    }
    pub(crate) fn validate(&self) -> ClientResult<()> {
        if self.fast_workers == 0 || self.bulk_workers == 0 || self.fast_workers + self.bulk_workers > 64
            || self.fast_batch_max_items == 0 || self.fast_batch_max_items > crate::wire::MAX_BLOB_BATCH_ITEMS
        { return Err(ClientError::InvalidInput { what: "runtime configuration" }); }
        Ok(())
    }
}
impl Default for RuntimeConfig { fn default() -> Self { Self::default_v1() } }

enum ActiveKind {
    Direct,
    File { role: FileRole, tier: makepad_asset_data::DeviceTier, lod: u8,
        media: makepad_asset_data::MediaType, blob: BlobId, byte_len: u64 },
    Thumbnail { blob: BlobId, media: makepad_asset_data::ThumbnailMedia,
        width: u32, height: u32, byte_len: u64 },
}
struct Active { request_id: RequestId, kind: ActiveKind }

pub struct ClientRuntime {
    store: StaticStore,
    queue: VecDeque<(RequestId, ClientRequest)>,
    active: HashMap<StaticFetchId, Active>,
    cancelled: RefCell<HashSet<RequestId>>,
    events: VecDeque<ClientEvent>,
    next_id: RequestId,
    config: RuntimeConfig,
}

impl ClientRuntime {
    pub fn start(_client: AssetClient) -> ClientResult<Self> {
        Err(ClientError::Unavailable { capability: crate::location::CAPABILITY_BLOCKING_API,
            mode: crate::location::ClientMode::StaticWeb })
    }
    pub fn start_with(_client: AssetClient, _config: RuntimeConfig) -> ClientResult<Self> { Self::start(_client) }
    pub fn start_static(config: ClientConfig) -> ClientResult<Self> {
        config.validate()?;
        let crate::location::ClientLocation::StaticSite(base) = config.location.clone()
            .ok_or(ClientError::InvalidInput { what: "static client location" })?
        else { return Err(ClientError::InvalidInput { what: "static client location" }); };
        Self::start_static_store(StaticStore::platform(base, config.cache.max_ram_bytes)?, RuntimeConfig::default())
    }
    pub fn start_static_store(store: StaticStore, config: RuntimeConfig) -> ClientResult<Self> {
        config.validate()?;
        Ok(Self { store, queue: VecDeque::new(), active: HashMap::new(), cancelled: RefCell::new(HashSet::new()),
            events: VecDeque::new(), next_id: 1, config })
    }
    pub fn submit(&mut self, request: ClientRequest) -> ClientResult<RequestId> { self.submit_with(request, SubmitOptions::default()) }
    pub fn submit_with(&mut self, request: ClientRequest, options: SubmitOptions) -> ClientResult<RequestId> {
        let id = self.next_id;
        self.next_id += 1;
        if options.lifo { self.queue.push_front((id, request)); } else { self.queue.push_back((id, request)); }
        Ok(id)
    }
    pub fn cancel(&self, id: RequestId) { self.cancelled.borrow_mut().insert(id); }
    pub fn lane_of(&self, request: &ClientRequest) -> Lane {
        match request {
            ClientRequest::FetchBlob { expected_len: Some(length), .. } if *length <= self.config.fast_blob_max_bytes => Lane::Fast,
            ClientRequest::FetchBlob { .. } | ClientRequest::ResolveFile { .. } | ClientRequest::PublishArtifact { .. }
            | ClientRequest::PublishBundle { .. } | ClientRequest::PublishSideChannels { .. }
            | ClientRequest::RegisterSourceCollection { .. } | ClientRequest::RunImport { .. } => Lane::Bulk,
            _ => Lane::Fast,
        }
    }
    pub fn config(&self) -> RuntimeConfig { self.config }
    pub fn is_ready(&self) -> bool { self.store.is_ready() }
    pub fn connect_error(&self) -> Option<&ClientError> {
        if let StaticStoreState::Failed(error) = self.store.state() { Some(error) } else { None }
    }
    pub fn location(&self) -> Option<crate::location::ClientLocation> { Some(self.store.location()) }
    pub fn server_id(&self) -> Option<[u8; 16]> { self.store.server_id() }
    pub fn poll_stages(&mut self) -> Vec<StageEvent> { Vec::new() }
    pub fn shutdown(self) {}

    pub fn poll(&mut self) -> Vec<ClientEvent> {
        for event in self.store.poll() {
            match event {
                StaticStoreEvent::Ready => {}
                StaticStoreEvent::Failed(error) => for (id, _) in self.queue.drain(..) {
                    self.events.push_back(ClientEvent::Failed { id, error: error.clone() });
                },
                StaticStoreEvent::FetchDone { id, output } => if let Some(active) = self.active.remove(&id) {
                    match map_output(output, active.kind) {
                        Ok(output) => self.events.push_back(ClientEvent::Done { id: active.request_id, output }),
                        Err(error) => self.events.push_back(ClientEvent::Failed { id: active.request_id, error }),
                    }
                    self.cancelled.borrow_mut().remove(&active.request_id);
                },
                StaticStoreEvent::FetchFailed { id, error } => if let Some(active) = self.active.remove(&id) {
                    self.events.push_back(ClientEvent::Failed { id: active.request_id, error });
                    self.cancelled.borrow_mut().remove(&active.request_id);
                },
            }
        }
        let cancelled: Vec<_> = {
            let cancelled = self.cancelled.borrow();
            self.active.iter().filter_map(|(fetch, active)|
                cancelled.contains(&active.request_id).then_some(*fetch)).collect()
        };
        for fetch in cancelled { self.store.cancel_fetch(fetch); }
        if self.store.is_ready() {
            for _ in 0..32 {
                let Some((id, request)) = self.queue.pop_front() else { break };
                if self.cancelled.borrow_mut().remove(&id) {
                    self.events.push_back(ClientEvent::Failed { id, error: ClientError::Cancelled });
                    continue;
                }
                self.events.push_back(ClientEvent::Started { id });
                match begin(&mut self.store, request) {
                    Ok(Begin::Done(output)) => self.events.push_back(ClientEvent::Done { id, output }),
                    Ok(Begin::Fetch(fetch, kind)) => { self.active.insert(fetch, Active { request_id: id, kind }); }
                    Err(error) => self.events.push_back(ClientEvent::Failed { id, error }),
                }
            }
        }
        self.events.drain(..).collect()
    }
}

enum Begin { Done(ClientOutput), Fetch(StaticFetchId, ActiveKind) }

fn begin(store: &mut StaticStore, request: ClientRequest) -> ClientResult<Begin> {
    let unavailable = |capability| Err(ClientError::Unavailable { capability, mode: crate::location::ClientMode::StaticWeb });
    Ok(match request {
        ClientRequest::CatalogSearch { query, cursor } => Begin::Done(ClientOutput::CatalogPage(store.catalog_search(&query, cursor.as_ref())?)),
        ClientRequest::AssetsPage { namespace, cursor, limit } => Begin::Done(ClientOutput::AssetsPage(store.assets_page(namespace.as_deref(), cursor.as_ref(), limit)?)),
        ClientRequest::AssetDetail { id } => Begin::Done(ClientOutput::AssetDetail(store.asset_detail(&id)?)),
        ClientRequest::ResolveAlias { alias } => Begin::Done(ClientOutput::Alias(store.resolve_alias(&alias)?)),
        ClientRequest::AliasStatus { entries, tags } => Begin::Done(ClientOutput::AliasStatus(store.alias_status(&entries, &tags)?)),
        ClientRequest::ResolveGameAlias { alias } => Begin::Done(ClientOutput::GameAlias(store.resolve_game_alias(&alias)?)),
        ClientRequest::FetchAssetManifest { rev } => Begin::Fetch(store.start_fetch(StaticFetch::AssetManifest(rev))?, ActiveKind::Direct),
        ClientRequest::FetchGameManifest { rev } => Begin::Fetch(store.start_fetch(StaticFetch::GameManifest(rev))?, ActiveKind::Direct),
        ClientRequest::FetchBlob { blob, expected_len, pin } => Begin::Fetch(store.start_fetch(StaticFetch::Blob { blob, expected_len, pin })?, ActiveKind::Direct),
        ClientRequest::HeadBlob { blob } => Begin::Done(ClientOutput::BlobHead { blob, head: store.blob_head(&blob)? }),
        ClientRequest::UnpinBlob { blob } => { store.unpin_blob(&blob)?; Begin::Done(ClientOutput::BlobUnpinned { blob }) }
        ClientRequest::ResolveFile { manifest, role, tier, max_lod } => {
            let file = crate::resolver::select_file(&manifest, role, tier, max_lod)?.clone();
            let fetch = store.start_fetch(StaticFetch::Blob { blob: file.blob, expected_len: Some(file.byte_len), pin: false })?;
            Begin::Fetch(fetch, ActiveKind::File { role: file.role, tier: file.tier, lod: file.lod,
                media: file.media, blob: file.blob, byte_len: file.byte_len })
        }
        ClientRequest::ResolveThumbnail { manifest } => match &manifest.thumbnail {
            None => Begin::Done(ClientOutput::Thumbnail(None)),
            Some(t) => Begin::Fetch(store.start_fetch(StaticFetch::Blob { blob: t.blob, expected_len: Some(t.byte_len), pin: false })?,
                ActiveKind::Thumbnail { blob: t.blob, media: t.media, width: t.width, height: t.height, byte_len: t.byte_len }),
        },
        ClientRequest::FetchDerivedVariant { id } => Begin::Fetch(store.start_fetch(StaticFetch::DerivedVariant(id))?, ActiveKind::Direct),
        ClientRequest::FetchVariantSet { id } => Begin::Fetch(store.start_fetch(StaticFetch::VariantSet(id))?, ActiveKind::Direct),
        ClientRequest::RetireAsset { .. } | ClientRequest::RetireRevision { .. } => return unavailable("retire"),
        ClientRequest::GcBlobs { .. } | ClientRequest::GcStatus | ClientRequest::GcCancel => return unavailable("blob_gc"),
        ClientRequest::PublishArtifact { .. } | ClientRequest::PublishBundle { .. } => return unavailable("publication"),
        ClientRequest::PublishSideChannels { .. } => return unavailable("side_channels"),
        ClientRequest::RegisterSourceCollection { .. } | ClientRequest::ListSourceCollections
        | ClientRequest::RunImport { .. } | ClientRequest::FetchImport { .. } => return unavailable("imports"),
        ClientRequest::FreezeVariantSet { .. } | ClientRequest::ResolveVariantSet { .. } => return unavailable("variant_resolution"),
    })
}

fn map_output(output: StaticFetchOutput, kind: ActiveKind) -> ClientResult<ClientOutput> {
    match (output, kind) {
        (StaticFetchOutput::AssetManifest(value), ActiveKind::Direct) => Ok(ClientOutput::AssetManifest(value)),
        (StaticFetchOutput::GameManifest(value), ActiveKind::Direct) => Ok(ClientOutput::GameManifest(value)),
        (StaticFetchOutput::DerivedVariant(value), ActiveKind::Direct) => Ok(ClientOutput::DerivedVariant(value)),
        (StaticFetchOutput::VariantSet(value), ActiveKind::Direct) => Ok(ClientOutput::VariantSet(value)),
        (StaticFetchOutput::Blob { blob, content }, ActiveKind::Direct) => Ok(ClientOutput::Blob { blob, content }),
        (StaticFetchOutput::Blob { content, .. }, ActiveKind::File { role, tier, lod, media, blob, byte_len }) =>
            Ok(ClientOutput::File(ResolvedFile { role, tier, lod, media, blob, byte_len, content })),
        (StaticFetchOutput::Blob { content, .. }, ActiveKind::Thumbnail { blob, media, width, height, byte_len }) =>
            Ok(ClientOutput::Thumbnail(Some(ResolvedThumbnail { blob, media, width, height, byte_len, content }))),
        _ => Err(ClientError::Protocol { what: "static fetch output mismatch" }),
    }
}
