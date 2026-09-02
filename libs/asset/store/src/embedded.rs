//! Direct, synchronous embedded-store API. No HTTP or background threads.

use crate::cas::{BlobCommit, Cas, MemoryCas};
use crate::core::{
    AssetDetail, CatalogCore, PublicExportFilter, PublicExportPage, PublishBatchItem,
    PublishBatchOutcome,
};
use crate::error::{ServerError, ServerResult};
use crate::gc::{GcConfig, GcStatus};
use crate::imports::{ImportReport, Imports};
use crate::search::{
    SearchFilters, SearchPage, SearchQuery, SearchViewer,
};
use crate::static_export_core::{ExportEntry, ExportPlan};
use crate::Budgets;
use makepad_asset_data::{
    AssetAlias, AssetId, AssetRevisionId, AssetRevisionRef, BlobId, Sha256,
};

const WORK_CHUNK_BYTES: usize = 256 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CapabilityMode {
    Embedded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreCapability {
    Chat,
    Jobs,
    Rooms,
    Discovery,
    Observer,
    ReferenceBlobs,
    OperatorSql,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StoreUnavailable {
    pub capability: StoreCapability,
    pub mode: CapabilityMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreError {
    Core(ServerError),
    Unavailable(StoreUnavailable),
}

pub type StoreResult<T> = Result<T, StoreError>;

impl From<ServerError> for StoreError {
    fn from(value: ServerError) -> Self {
        Self::Core(value)
    }
}

#[derive(Clone, Debug)]
pub struct BlobUpload {
    pub expected: BlobId,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug)]
pub struct PublishRequest {
    pub blobs: Vec<BlobUpload>,
    pub items: Vec<PublishBatchItem>,
    pub now_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PublishStage {
    Validate,
    Blob { index: u32 },
    Catalog,
    Complete,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationStage {
    Publish(PublishStage),
    BlobRead,
    SearchIndexRebuild,
    Import,
    Gc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkProgress {
    pub stage: OperationStage,
    pub completed: u64,
    pub total: Option<u64>,
    pub done: bool,
}

pub struct EmbeddedStore {
    core: CatalogCore,
    cas: MemoryCas,
}

impl EmbeddedStore {
    pub fn open_memory(budgets: Budgets) -> ServerResult<Self> {
        let core = CatalogCore::open_memory(budgets)?;
        let cas = MemoryCas::new(core.budgets());
        Ok(Self { core, cas })
    }

    pub fn open_with<S: makepad_sqlite::PageStoreSet + 'static>(
        stores: S,
        budgets: Budgets,
    ) -> ServerResult<Self> {
        let core = CatalogCore::open_with(stores, budgets)?;
        let cas = MemoryCas::new(core.budgets());
        Ok(Self { core, cas })
    }

    pub fn catalog_core(&self) -> &CatalogCore {
        &self.core
    }

    pub fn list(
        &self,
        namespace: Option<&str>,
        page_size: u32,
        viewer: &SearchViewer<'_>,
        cursor: Option<&[u8]>,
    ) -> StoreResult<SearchPage> {
        let query = SearchQuery {
            text: "",
            filters: SearchFilters { namespace, live_only: true, ..SearchFilters::default() },
            page_size,
            expand: false,
            facets: 0,
        };
        self.search(&query, viewer, cursor)
    }

    pub fn search(
        &self,
        query: &SearchQuery<'_>,
        viewer: &SearchViewer<'_>,
        cursor: Option<&[u8]>,
    ) -> StoreResult<SearchPage> {
        Ok(self.core.search().search(query, viewer, cursor)?)
    }

    pub fn detail(&self, asset_id: &AssetId) -> StoreResult<Option<AssetDetail>> {
        Ok(self.core.detail(asset_id)?)
    }

    pub fn public_export_page(
        &self,
        filter: PublicExportFilter<'_>,
    ) -> StoreResult<PublicExportPage> {
        Ok(self.core.public_export_page(filter)?)
    }

    pub fn resolve_alias(
        &self,
        alias: &AssetAlias,
    ) -> StoreResult<Option<AssetRevisionRef>> {
        Ok(self.core.catalog().resolve_asset_alias(alias)?)
    }

    pub fn set_alias(
        &self,
        alias: &AssetAlias,
        target: &AssetRevisionRef,
        now_ms: u64,
    ) -> StoreResult<()> {
        Ok(self.core.catalog().set_asset_alias(alias, target, now_ms)?)
    }

    pub fn clear_alias(&self, alias: &AssetAlias) -> StoreResult<bool> {
        Ok(self.core.catalog().clear_asset_alias(alias)?)
    }

    pub fn read_revision(
        &self,
        revision: &AssetRevisionId,
    ) -> StoreResult<Option<Vec<u8>>> {
        Ok(self.core.catalog().asset_revision_manifest(revision)?)
    }

    pub fn read_blob(&self, blob_id: &BlobId) -> StoreResult<Vec<u8>> {
        if !self.core.catalog().has_blob(blob_id)? {
            return Err(ServerError::NotFound { what: "blob record" }.into());
        }
        Ok(self.cas.read_verified(blob_id)?)
    }

    pub fn start_blob_read(&self, blob_id: BlobId) -> BlobReadOperation {
        BlobReadOperation { blob_id, bytes: None, offset: 0 }
    }

    pub fn start_publish(&self, request: PublishRequest) -> LongOperation {
        LongOperation::Publish(PublishOperation::new(request))
    }

    pub fn publish(&self, request: PublishRequest) -> StoreResult<Vec<PublishBatchOutcome>> {
        let mut operation = self.start_publish(request);
        loop {
            if operation.step(self)?.done {
                return operation
                    .publish_outcomes()
                    .map(|outcomes| outcomes.to_vec())
                    .ok_or(StoreError::Core(ServerError::InvalidState {
                        what: "publish operation",
                        state: "completed without outcome",
                    }));
            }
        }
    }

    pub fn imports(&self) -> Imports<'_> {
        self.core.imports()
    }

    pub fn start_import(&self, manifest_bytes: Vec<u8>, now_ms: u64) -> LongOperation {
        LongOperation::Import(ImportOperation {
            manifest_bytes,
            now_ms,
            checked: 0,
            report: None,
        })
    }

    pub fn start_search_index_rebuild(&self, now_ms: u64) -> LongOperation {
        LongOperation::SearchIndexRebuild(SearchIndexRebuild {
            cursor: None,
            now_ms,
            rebuilt: 0,
            done: false,
        })
    }

    pub fn start_gc(&self, config: GcConfig, now_ms: u64) -> LongOperation {
        LongOperation::Gc(GcOperation { config, now_ms, begun: false, status: None })
    }

    pub fn export_plan(
        &self,
        entries: impl IntoIterator<Item = ExportEntry>,
    ) -> ExportPlan {
        let mut plan = ExportPlan::default();
        for entry in entries {
            plan.push(entry);
        }
        plan
    }

    pub fn chat(&self) -> StoreResult<()> {
        self.unavailable(StoreCapability::Chat)
    }

    pub fn jobs(&self) -> StoreResult<()> {
        self.unavailable(StoreCapability::Jobs)
    }

    pub fn rooms(&self) -> StoreResult<()> {
        self.unavailable(StoreCapability::Rooms)
    }

    pub fn discovery(&self) -> StoreResult<()> {
        self.unavailable(StoreCapability::Discovery)
    }

    pub fn observer(&self) -> StoreResult<()> {
        self.unavailable(StoreCapability::Observer)
    }

    pub fn reference_blobs(&self) -> StoreResult<()> {
        self.unavailable(StoreCapability::ReferenceBlobs)
    }

    fn unavailable<T>(&self, capability: StoreCapability) -> StoreResult<T> {
        Err(StoreError::Unavailable(StoreUnavailable {
            capability,
            mode: CapabilityMode::Embedded,
        }))
    }
}

pub struct BlobReadOperation {
    blob_id: BlobId,
    bytes: Option<Vec<u8>>,
    offset: usize,
}

impl BlobReadOperation {
    pub fn step(&mut self, store: &EmbeddedStore) -> StoreResult<Option<Vec<u8>>> {
        if self.bytes.is_none() {
            self.bytes = Some(store.read_blob(&self.blob_id)?);
        }
        let bytes = self.bytes.as_ref().expect("initialized");
        if self.offset == bytes.len() {
            return Ok(None);
        }
        let end = self.offset.saturating_add(WORK_CHUNK_BYTES).min(bytes.len());
        let chunk = bytes[self.offset..end].to_vec();
        self.offset = end;
        Ok(Some(chunk))
    }
}

pub enum LongOperation {
    Publish(PublishOperation),
    SearchIndexRebuild(SearchIndexRebuild),
    Import(ImportOperation),
    Gc(GcOperation),
}

impl LongOperation {
    pub fn step(&mut self, store: &EmbeddedStore) -> StoreResult<ChunkProgress> {
        match self {
            Self::Publish(operation) => operation.step(store),
            Self::SearchIndexRebuild(operation) => operation.step(store),
            Self::Import(operation) => operation.step(store),
            Self::Gc(operation) => operation.step(store),
        }
    }

    pub fn publish_outcomes(&self) -> Option<&[PublishBatchOutcome]> {
        match self {
            Self::Publish(operation) => operation.outcomes.as_deref(),
            _ => None,
        }
    }

    pub fn import_report(&self) -> Option<&ImportReport> {
        match self {
            Self::Import(operation) => operation.report.as_ref(),
            _ => None,
        }
    }

    pub fn gc_status(&self) -> Option<GcStatus> {
        match self {
            Self::Gc(operation) => operation.status,
            _ => None,
        }
    }
}

pub struct PublishOperation {
    request: PublishRequest,
    stage: PublishStage,
    blob_index: usize,
    blob_offset: usize,
    hasher: Sha256,
    completed: u64,
    total: u64,
    outcomes: Option<Vec<PublishBatchOutcome>>,
}

impl PublishOperation {
    fn new(request: PublishRequest) -> Self {
        let total = request.blobs.iter().map(|blob| blob.bytes.len() as u64).sum();
        Self {
            request,
            stage: PublishStage::Validate,
            blob_index: 0,
            blob_offset: 0,
            hasher: Sha256::new(),
            completed: 0,
            total,
            outcomes: None,
        }
    }

    fn step(&mut self, store: &EmbeddedStore) -> StoreResult<ChunkProgress> {
        match self.stage {
            PublishStage::Validate => {
                for blob in &self.request.blobs {
                    if blob.bytes.len() as u64 > store.core.budgets().max_blob_bytes {
                        return Err(ServerError::OverBudget {
                            what: "blob bytes",
                            limit: store.core.budgets().max_blob_bytes,
                            found: blob.bytes.len() as u64,
                        }
                        .into());
                    }
                }
                self.stage = if self.request.blobs.is_empty() {
                    PublishStage::Catalog
                } else {
                    PublishStage::Blob { index: 0 }
                };
            }
            PublishStage::Blob { index } => {
                let blob = &self.request.blobs[index as usize];
                let end = self
                    .blob_offset
                    .saturating_add(WORK_CHUNK_BYTES)
                    .min(blob.bytes.len());
                self.hasher.update(&blob.bytes[self.blob_offset..end]);
                self.completed += (end - self.blob_offset) as u64;
                self.blob_offset = end;
                if end == blob.bytes.len() {
                    let found = std::mem::take(&mut self.hasher).finalize();
                    if &found != blob.expected.as_bytes() {
                        return Err(ServerError::DigestMismatch {
                            what: "embedded publish blob",
                            expected: *blob.expected.as_bytes(),
                            found,
                        }
                        .into());
                    }
                    let BlobCommit { blob_id, size, .. } =
                        store.cas.put(&blob.bytes, Some(blob.expected))?;
                    store.core.catalog().record_blob(&blob_id, size, self.request.now_ms)?;
                    self.blob_index += 1;
                    self.blob_offset = 0;
                    self.stage = if self.blob_index == self.request.blobs.len() {
                        PublishStage::Catalog
                    } else {
                        PublishStage::Blob { index: self.blob_index as u32 }
                    };
                }
            }
            PublishStage::Catalog => {
                self.outcomes = Some(
                    store.core.publish_batch(&self.request.items, self.request.now_ms)?,
                );
                self.stage = PublishStage::Complete;
            }
            PublishStage::Complete => {}
        }
        Ok(ChunkProgress {
            stage: OperationStage::Publish(self.stage),
            completed: self.completed,
            total: Some(self.total),
            done: self.stage == PublishStage::Complete,
        })
    }
}

pub struct SearchIndexRebuild {
    cursor: Option<AssetId>,
    now_ms: u64,
    rebuilt: u64,
    done: bool,
}

impl SearchIndexRebuild {
    fn step(&mut self, store: &EmbeddedStore) -> StoreResult<ChunkProgress> {
        if !self.done {
            let mut stmt = store.core.db.prepare(
                "search rebuild next",
                "SELECT asset_id FROM search_annotations
                 WHERE asset_id>?1 ORDER BY asset_id LIMIT 1",
            )?;
            stmt.bind_blob(1, self.cursor.as_ref().map_or(&[], |id| id.as_bytes()))?;
            if stmt.step()? {
                let asset_id = AssetId::from_bytes(crate::catalog::fixed16(
                    &stmt.column_blob(0),
                    "search rebuild asset id",
                )?);
                drop(stmt);
                if let Some(annotation) = store.core.search().annotation(&asset_id)? {
                    store.core.search().set_annotation(&asset_id, &annotation, self.now_ms)?;
                }
                self.cursor = Some(asset_id);
                self.rebuilt += 1;
            } else {
                self.done = true;
            }
        }
        Ok(ChunkProgress {
            stage: OperationStage::SearchIndexRebuild,
            completed: self.rebuilt,
            total: None,
            done: self.done,
        })
    }
}

pub struct ImportOperation {
    manifest_bytes: Vec<u8>,
    now_ms: u64,
    checked: usize,
    report: Option<ImportReport>,
}

impl ImportOperation {
    fn step(&mut self, store: &EmbeddedStore) -> StoreResult<ChunkProgress> {
        if self.checked < self.manifest_bytes.len() {
            self.checked = self
                .checked
                .saturating_add(WORK_CHUNK_BYTES)
                .min(self.manifest_bytes.len());
        } else if self.report.is_none() {
            self.report = Some(store.core.imports().run_import(&self.manifest_bytes, self.now_ms)?);
        }
        Ok(ChunkProgress {
            stage: OperationStage::Import,
            completed: self.checked as u64,
            total: Some(self.manifest_bytes.len() as u64),
            done: self.report.is_some(),
        })
    }
}

pub struct GcOperation {
    config: GcConfig,
    now_ms: u64,
    begun: bool,
    status: Option<GcStatus>,
}

impl GcOperation {
    fn step(&mut self, store: &EmbeddedStore) -> StoreResult<ChunkProgress> {
        if !self.begun {
            self.status = Some(store.core.gc().begin(self.config, self.now_ms)?);
            self.begun = true;
        } else if self.status.is_some_and(|status| !status.finished()) {
            self.status = Some(store.core.gc().step(&store.cas, self.now_ms)?);
        }
        let status = self.status.expect("GC begun");
        Ok(ChunkProgress {
            stage: OperationStage::Gc,
            completed: status.examined_blobs + status.scanned_revisions,
            total: None,
            done: status.finished(),
        })
    }
}
