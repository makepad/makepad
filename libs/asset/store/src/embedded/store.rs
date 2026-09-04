//! Typed embedded store, with an optional storage-backed durability mode.

use super::cas::{StorageCas, StorageCasError};
use super::durability::{
    reclaim_catalog_garbage, restore_catalog, DurabilityCoordinator, DurabilityError,
    StorageValues,
};
use super::quota::{QuotaExceeded, QuotaManager, QuotaPolicy};

use crate::cas::{BlobCommit, Cas, MemoryCas};
use crate::core::{
    AssetDetail, CatalogCore, PublicExportFilter, PublicExportPage, PublishBatchItem,
    PublishBatchOutcome,
};
use crate::error::{ServerError, ServerResult};
use crate::catalog::RetireReport;
use crate::gc::{GcConfig, GcStatus, GcStep};
use crate::imports::{ImportReport, Imports};
use crate::search::{
    SearchFilters, SearchPage, SearchQuery, SearchViewer,
};
use crate::static_export_core::{ExportEntry, ExportPlan};
use crate::Budgets;
use makepad_asset_data::{
    sha256, AssetAlias, AssetId, AssetRevisionId, AssetRevisionRef, BlobId, Sha256,
};
use makepad_platform::{StorageError, StorageEstimate};
use makepad_sqlite::{MemoryStoreSet, StoreKind};
use std::collections::VecDeque;

const WORK_CHUNK_BYTES: usize = 256 * 1024;

/// Stable, path-safe storage namespace derived from the complete caller id.
/// Hashing avoids both truncation collisions and leaking user-facing names.
pub fn storage_namespace(store_id: &str) -> StoreResult<String> {
    if store_id.is_empty() || store_id.len() > 1024 {
        return Err(StoreError::Core(ServerError::InvalidInput {
            what: "embedded store id",
        }));
    }
    let digest = super::durability::sha256_hex_bytes(sha256(store_id.as_bytes()));
    Ok(format!("makepad.asset-store.{}", &digest[..40]))
}

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StoreError {
    Core(ServerError),
    Unavailable(StoreUnavailable),
    Storage(StorageError),
    QuotaExceeded(QuotaExceeded),
    Corrupt(&'static str),
}

pub type StoreResult<T> = Result<T, StoreError>;

impl From<ServerError> for StoreError {
    fn from(value: ServerError) -> Self {
        Self::Core(value)
    }
}

impl From<StorageError> for StoreError {
    fn from(value: StorageError) -> Self {
        match value {
            StorageError::QuotaExceeded(_) => Self::QuotaExceeded(QuotaExceeded {
                required: 1,
                available: 0,
            }),
            other => Self::Storage(other),
        }
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReclaimReport {
    pub expired_partials: u64,
    pub catalog_values: u64,
    pub cas_chunks: u64,
}

pub struct EmbeddedStore {
    core: CatalogCore,
    cas: MemoryCas,
    stores: Option<MemoryStoreSet>,
    durability: DurabilityCoordinator,
    storage_cas: StorageCas,
    quota: QuotaManager,
    durable: bool,
    poisoned: bool,
}

impl EmbeddedStore {
    pub fn open_memory(budgets: Budgets) -> ServerResult<Self> {
        let stores = MemoryStoreSet::new();
        let core = CatalogCore::open_with(stores.clone(), budgets)?;
        let cas = MemoryCas::new(core.budgets());
        Ok(Self {
            storage_cas: StorageCas::new(core.budgets().max_blob_bytes),
            core,
            cas,
            stores: Some(stores),
            durability: DurabilityCoordinator::default(),
            quota: QuotaManager::new(QuotaPolicy::default()),
            durable: false,
            poisoned: false,
        })
    }

    pub fn open_with<S: makepad_sqlite::PageStoreSet + 'static>(
        stores: S,
        budgets: Budgets,
    ) -> ServerResult<Self> {
        let core = CatalogCore::open_with(stores, budgets)?;
        let cas = MemoryCas::new(core.budgets());
        Ok(Self {
            storage_cas: StorageCas::new(core.budgets().max_blob_bytes),
            core,
            cas,
            stores: None,
            durability: DurabilityCoordinator::default(),
            quota: QuotaManager::new(QuotaPolicy::default()),
            durable: false,
            poisoned: false,
        })
    }

    /// Restore the newest fully checksummed generation, or create and publish
    /// generation one for a genuinely empty namespace.
    pub fn open_durable(
        storage: &mut dyn StorageValues,
        budgets: Budgets,
        quota_policy: QuotaPolicy,
    ) -> StoreResult<Self> {
        let restored = restore_catalog(storage).map_err(map_durability)?;
        let stores = MemoryStoreSet::new();
        if let Some(restored) = &restored {
            stores.restore(StoreKind::Main, restored.bytes.clone()).map_err(|error| {
                StoreError::Core(ServerError::Io {
                    op: "restore embedded catalog",
                    kind: error.kind(),
                })
            })?;
        }
        let core = CatalogCore::open_with(stores.clone(), budgets)?;
        quick_check(&core)?;
        let cas = MemoryCas::new(core.budgets());
        let mut store = Self {
            storage_cas: StorageCas::new(core.budgets().max_blob_bytes),
            core,
            cas,
            stores: Some(stores),
            durability: restored
                .as_ref()
                .map(DurabilityCoordinator::from_restored)
                .unwrap_or_default(),
            quota: QuotaManager::new(quota_policy),
            durable: true,
            poisoned: false,
        };
        store.quota.reconcile(storage)?;
        if restored.is_none() {
            store.persist_catalog(storage)?;
        }
        Ok(store)
    }

    pub fn is_durable(&self) -> bool {
        self.durable
    }

    pub fn durable_generation(&self) -> Option<u64> {
        self.durability.durable_generation()
    }

    pub fn quota(&self) -> &QuotaManager {
        &self.quota
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
            newest: false,
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
        self.require_memory_only()?;
        Ok(self.core.catalog().set_asset_alias(alias, target, now_ms)?)
    }

    pub fn clear_alias(&self, alias: &AssetAlias) -> StoreResult<bool> {
        self.require_memory_only()?;
        Ok(self.core.catalog().clear_asset_alias(alias)?)
    }

    pub fn read_revision(
        &self,
        revision: &AssetRevisionId,
    ) -> StoreResult<Option<Vec<u8>>> {
        Ok(self.core.catalog().asset_revision_manifest(revision)?)
    }

    pub fn read_blob(&self, blob_id: &BlobId) -> StoreResult<Vec<u8>> {
        self.require_memory_only()?;
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
        self.require_memory_only()?;
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

    pub fn read_blob_durable(
        &self,
        storage: &dyn StorageValues,
        blob_id: &BlobId,
    ) -> StoreResult<Vec<u8>> {
        self.require_durable_ready()?;
        if !self.core.catalog().has_blob(blob_id)? {
            return Err(ServerError::NotFound { what: "blob record" }.into());
        }
        self.storage_cas
            .read_verified(storage, blob_id)
            .map_err(map_storage_cas)
    }

    pub fn publish_durable(
        &mut self,
        storage: &mut dyn StorageValues,
        estimate: StorageEstimate,
        request: PublishRequest,
    ) -> StoreResult<Vec<PublishBatchOutcome>> {
        self.require_durable_ready()?;
        let declared = request
            .blobs
            .iter()
            .try_fold(0u64, |total, blob| total.checked_add(blob.bytes.len() as u64))
            .ok_or(StoreError::Core(ServerError::OverBudget {
                what: "publish bytes",
                limit: u64::MAX,
                found: u64::MAX,
            }))?;
        if self.quota.preflight(estimate, declared).is_err() {
            let before = self.quota.store_owned_bytes();
            self.reclaim_safe_garbage(storage, request.now_ms.saturating_sub(24 * 60 * 60 * 1000))?;
            let freed = before.saturating_sub(self.quota.store_owned_bytes());
            let adjusted = StorageEstimate {
                usage: estimate.usage.saturating_sub(freed),
                quota: estimate.quota,
            };
            self.quota
                .preflight(adjusted, declared)
                .map_err(StoreError::QuotaExceeded)?;
        }

        // Complete every object manifest before touching SQLite.
        let mut admitted = Vec::with_capacity(request.blobs.len());
        for (index, blob) in request.blobs.iter().enumerate() {
            let upload_id = format!(
                "publish-{}-{index}-{}",
                request.now_ms,
                super::durability::sha256_hex_bytes(*blob.expected.as_bytes())
            );
            let commit = self
                .storage_cas
                .put(
                    storage,
                    &upload_id,
                    blob.bytes.clone(),
                    blob.expected,
                    request.now_ms,
                )
                .map_err(map_storage_cas)?;
            admitted.push((commit.blob_id, commit.size));
        }

        let catalog_result = (|| {
            for (blob_id, size) in &admitted {
                self.core.catalog().record_blob(blob_id, *size, request.now_ms)?;
            }
            self.core.publish_batch(&request.items, request.now_ms)
        })();
        let outcomes = match catalog_result {
            Ok(outcomes) => outcomes,
            Err(error) => {
                self.rollback_from_storage(storage)?;
                return Err(error.into());
            }
        };
        self.persist_catalog(storage)?;
        self.quota.reconcile(storage)?;
        Ok(outcomes)
    }

    pub fn set_alias_durable(
        &mut self,
        storage: &mut dyn StorageValues,
        alias: &AssetAlias,
        target: &AssetRevisionRef,
        now_ms: u64,
    ) -> StoreResult<()> {
        self.require_durable_ready()?;
        self.core.catalog().set_asset_alias(alias, target, now_ms)?;
        self.persist_catalog(storage)
    }

    pub fn clear_alias_durable(
        &mut self,
        storage: &mut dyn StorageValues,
        alias: &AssetAlias,
    ) -> StoreResult<bool> {
        self.require_durable_ready()?;
        let cleared = self.core.catalog().clear_asset_alias(alias)?;
        if cleared {
            self.persist_catalog(storage)?;
        }
        Ok(cleared)
    }

    /// One crash-safe GC unit: durable intent/catalog removal, physical
    /// object deletion, then durable intent clear.
    pub fn gc_step_durable(
        &mut self,
        storage: &mut dyn StorageValues,
        now_ms: u64,
    ) -> StoreResult<GcStatus> {
        self.require_durable_ready()?;
        let step = self.gc_catalog_step_durable(storage, now_ms)?;
        for intent in &step.deletes {
            self.complete_gc_delete_durable(storage, &intent.blob_id)?;
        }
        Ok(step.status)
    }

    pub fn retire_asset_durable(
        &mut self,
        storage: &mut dyn StorageValues,
        asset_id: &AssetId,
        now_ms: u64,
    ) -> StoreResult<RetireReport> {
        self.require_durable_ready()?;
        let report = self.core.catalog().retire_asset(asset_id, now_ms)?;
        self.persist_catalog(storage)?;
        Ok(report)
    }

    pub fn begin_gc_durable(
        &mut self,
        storage: &mut dyn StorageValues,
        config: GcConfig,
        now_ms: u64,
    ) -> StoreResult<GcStatus> {
        self.require_durable_ready()?;
        let status = self.core.gc().begin(config, now_ms)?;
        self.persist_catalog(storage)?;
        Ok(status)
    }

    pub fn gc_catalog_step_durable(
        &mut self,
        storage: &mut dyn StorageValues,
        now_ms: u64,
    ) -> StoreResult<GcStep> {
        self.require_durable_ready()?;
        let step = self.core.gc().step_catalog(now_ms)?;
        self.persist_catalog(storage)?;
        Ok(step)
    }

    pub fn complete_gc_delete_durable(
        &mut self,
        storage: &mut dyn StorageValues,
        blob_id: &BlobId,
    ) -> StoreResult<()> {
        self.require_durable_ready()?;
        self.storage_cas
            .delete_object(storage, blob_id)
            .map_err(map_storage_cas)?;
        self.core.gc().clear_delete_intent(blob_id)?;
        self.persist_catalog(storage)
    }

    pub fn recover_gc_durable(
        &mut self,
        storage: &mut dyn StorageValues,
    ) -> StoreResult<u64> {
        self.require_durable_ready()?;
        let intents = self.core.gc().pending_deletes(1024)?;
        for intent in &intents {
            self.storage_cas
                .delete_object(storage, &intent.blob_id)
                .map_err(map_storage_cas)?;
            self.core.gc().clear_delete_intent(&intent.blob_id)?;
            self.persist_catalog(storage)?;
        }
        Ok(intents.len() as u64)
    }

    /// Safe garbage reclamation order used before returning quota refusal.
    /// Live object manifests remain authoritative and are never evicted.
    pub fn reclaim_safe_garbage(
        &mut self,
        storage: &mut dyn StorageValues,
        partials_older_than_ms: u64,
    ) -> StoreResult<ReclaimReport> {
        self.require_durable_ready()?;
        let expired_partials = self
            .storage_cas
            .expire_partials(storage, partials_older_than_ms)
            .map_err(map_storage_cas)?;
        let catalog_values = reclaim_catalog_garbage(storage).map_err(map_durability)?;
        let cas_chunks = self
            .storage_cas
            .reclaim_orphan_chunks(storage)
            .map_err(map_storage_cas)?;
        self.quota.reconcile(storage)?;
        Ok(ReclaimReport { expired_partials, catalog_values, cas_chunks })
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

    fn require_memory_only(&self) -> StoreResult<()> {
        if self.durable {
            Err(StoreError::Core(ServerError::InvalidState {
                what: "durable embedded mutation",
                state: "use storage-backed operation",
            }))
        } else {
            Ok(())
        }
    }

    fn require_durable_ready(&self) -> StoreResult<()> {
        if !self.durable {
            return Err(StoreError::Core(ServerError::InvalidState {
                what: "embedded store",
                state: "not storage-backed",
            }));
        }
        if self.poisoned || self.durability.rollback_required() {
            return Err(StoreError::Core(ServerError::InvalidState {
                what: "embedded store",
                state: "durability rollback required",
            }));
        }
        Ok(())
    }

    fn persist_catalog(&mut self, storage: &mut dyn StorageValues) -> StoreResult<()> {
        let result = self.persist_catalog_inner(storage);
        if result.is_err()
            && self.durability.durable_generation().is_some()
            && self.rollback_from_storage(storage).is_err()
        {
            self.poisoned = true;
        }
        result
    }

    fn persist_catalog_inner(&mut self, storage: &mut dyn StorageValues) -> StoreResult<()> {
        let stores = self.stores.as_ref().ok_or(StoreError::Corrupt(
            "durable embedded store has no memory page store",
        ))?;
        let snapshot = stores
            .snapshot(StoreKind::Main)
            .map_err(|error| ServerError::Io {
                op: "snapshot embedded catalog",
                kind: error.kind(),
            })?
            .ok_or(StoreError::Corrupt("embedded catalog Main is missing"))?;
        self.durability.begin_commit(snapshot).map_err(map_durability)?;
        if let Err(error) = self.durability.execute_commit(storage) {
            return Err(map_durability(error));
        }
        if let Some(snapshot) = self.durability.take_completed_snapshot() {
            stores.mark_snapshot_clean(&snapshot).map_err(|error| ServerError::Io {
                op: "clean embedded catalog snapshot",
                kind: error.kind(),
            })?;
        }
        Ok(())
    }

    fn rollback_from_storage(&mut self, storage: &dyn StorageValues) -> StoreResult<()> {
        let restored = restore_catalog(storage).map_err(map_durability)?;
        let restored = restored.ok_or(StoreError::Corrupt(
            "durable catalog rollback has no committed generation",
        ))?;
        let stores = MemoryStoreSet::new();
        stores
            .restore(StoreKind::Main, restored.bytes.clone())
            .map_err(|error| ServerError::Io {
                op: "rollback embedded catalog",
                kind: error.kind(),
            })?;
        let budgets = *self.core.budgets();
        let core = CatalogCore::open_with(stores.clone(), budgets)?;
        quick_check(&core)?;
        self.core = core;
        self.stores = Some(stores);
        self.durability.accept_rollback(Some(&restored));
        self.poisoned = false;
        Ok(())
    }
}

fn quick_check(core: &CatalogCore) -> StoreResult<()> {
    let mut statement = core.db.prepare("embedded quick_check", "PRAGMA quick_check")?;
    if !statement.step()? || statement.column_text(0) != "ok" {
        return Err(StoreError::Corrupt("embedded catalog quick_check failed"));
    }
    Ok(())
}

fn map_durability(error: DurabilityError) -> StoreError {
    match error {
        DurabilityError::Storage(error) => error.into(),
        DurabilityError::Corrupt(what) => StoreError::Corrupt(what),
        DurabilityError::NoValidGeneration => StoreError::Corrupt("no valid catalog generation"),
        DurabilityError::Busy => StoreError::Core(ServerError::InvalidState {
            what: "catalog durability",
            state: "busy",
        }),
    }
}

fn map_storage_cas(error: StorageCasError) -> StoreError {
    match error {
        StorageCasError::Storage(error) => error.into(),
        StorageCasError::Core(error) => error.into(),
        StorageCasError::Corrupt(what) => StoreError::Corrupt(what),
        StorageCasError::InvalidUpload => StoreError::Core(ServerError::Conflict {
            what: "partial upload",
        }),
    }
}

pub trait AssetStoreArchiveSink {
    fn write(&mut self, bytes: &[u8]) -> StoreResult<()>;
}

pub trait AssetStoreArchiveSource {
    fn read_exact(&mut self, len: usize) -> StoreResult<Vec<u8>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BackupProgress {
    pub entries: u64,
    pub bytes: u64,
    pub done: bool,
}

/// Cooperative `.mpassetstore` export. One step reads and writes at most one
/// storage value; blob values are capped by `CAS_CHUNK_BYTES`.
pub struct BackupExport {
    keys: VecDeque<String>,
    root: Sha256,
    entries: u64,
    bytes: u64,
    started: bool,
    done: bool,
}

impl BackupExport {
    pub fn new(storage: &dyn StorageValues) -> StoreResult<Self> {
        let mut keys: Vec<_> = storage
            .list("")?
            .into_iter()
            .filter(|key| {
                key.starts_with("catalog/")
                    || key.starts_with("cas/object/")
                    || key.starts_with("cas/chunk/")
            })
            .collect();
        keys.sort_unstable();
        Ok(Self {
            keys: keys.into(),
            root: Sha256::new(),
            entries: 0,
            bytes: 0,
            started: false,
            done: false,
        })
    }

    pub fn step(
        &mut self,
        storage: &dyn StorageValues,
        sink: &mut dyn AssetStoreArchiveSink,
    ) -> StoreResult<BackupProgress> {
        if self.done {
            return Ok(self.progress());
        }
        if !self.started {
            sink.write(b"MPAS\x01\x00")?;
            self.started = true;
            return Ok(self.progress());
        }
        if let Some(key) = self.keys.pop_front() {
            let value = storage
                .get(&key)?
                .ok_or(StoreError::Corrupt("backup value vanished"))?;
            if value.len() > super::cas::CAS_CHUNK_BYTES {
                return Err(StoreError::Corrupt("backup storage value exceeds CAS chunk cap"));
            }
            let key_len = u16::try_from(key.len())
                .map_err(|_| StoreError::Corrupt("backup key is too long"))?;
            let value_len = u32::try_from(value.len())
                .map_err(|_| StoreError::Corrupt("backup value is too long"))?;
            let mut header = Vec::with_capacity(42);
            header.extend_from_slice(b"MPRE");
            header.extend_from_slice(&key_len.to_le_bytes());
            header.extend_from_slice(&value_len.to_le_bytes());
            header.extend_from_slice(&sha256(&value));
            self.root.update(&header);
            self.root.update(key.as_bytes());
            self.root.update(&value);
            sink.write(&header)?;
            sink.write(key.as_bytes())?;
            sink.write(&value)?;
            self.entries += 1;
            self.bytes = self.bytes.saturating_add(value.len() as u64);
            return Ok(self.progress());
        }
        let root = std::mem::take(&mut self.root).finalize();
        let mut footer = Vec::with_capacity(44);
        footer.extend_from_slice(b"MPEN");
        footer.extend_from_slice(&self.entries.to_le_bytes());
        footer.extend_from_slice(&root);
        sink.write(&footer)?;
        self.done = true;
        Ok(self.progress())
    }

    fn progress(&self) -> BackupProgress {
        BackupProgress { entries: self.entries, bytes: self.bytes, done: self.done }
    }
}

/// Logical import into an empty value namespace. Every record is individually
/// checked, and the container root checksum is verified before completion.
pub struct BackupImport {
    root: Sha256,
    imported_keys: Vec<String>,
    entries: u64,
    bytes: u64,
    started: bool,
    done: bool,
    pending_head: Option<Vec<u8>>,
}

impl BackupImport {
    pub fn new(storage: &dyn StorageValues) -> StoreResult<Self> {
        if !storage.list("")?.is_empty() {
            return Err(StoreError::Core(ServerError::Conflict {
                what: "backup import target is not fresh",
            }));
        }
        Ok(Self {
            root: Sha256::new(),
            imported_keys: Vec::new(),
            entries: 0,
            bytes: 0,
            started: false,
            done: false,
            pending_head: None,
        })
    }

    pub fn step(
        &mut self,
        storage: &mut dyn StorageValues,
        source: &mut dyn AssetStoreArchiveSource,
    ) -> StoreResult<BackupProgress> {
        if self.done {
            return Ok(self.progress());
        }
        if !self.started {
            if source.read_exact(6)? != b"MPAS\x01\x00" {
                return Err(StoreError::Corrupt("backup header"));
            }
            self.started = true;
            return Ok(self.progress());
        }
        let magic = source.read_exact(4)?;
        if magic == b"MPEN" {
            let footer = source.read_exact(40)?;
            let expected_entries = u64::from_le_bytes(
                footer[..8].try_into().expect("fixed footer count"),
            );
            let expected_root: [u8; 32] = footer[8..].try_into().expect("fixed footer digest");
            let found_root = std::mem::take(&mut self.root).finalize();
            if expected_entries != self.entries || expected_root != found_root {
                self.rollback(storage);
                return Err(StoreError::Corrupt("backup root checksum"));
            }
            let Some(head) = self.pending_head.take() else {
                self.rollback(storage);
                return Err(StoreError::Corrupt("backup has no catalog head"));
            };
            if let Err(error) = storage.set("catalog/head", head) {
                self.rollback(storage);
                return Err(error.into());
            }
            self.imported_keys.push("catalog/head".into());
            self.done = true;
            return Ok(self.progress());
        }
        if magic != b"MPRE" {
            self.rollback(storage);
            return Err(StoreError::Corrupt("backup record framing"));
        }
        let rest = source.read_exact(38)?;
        let key_len = u16::from_le_bytes(rest[..2].try_into().expect("fixed key length")) as usize;
        let value_len =
            u32::from_le_bytes(rest[2..6].try_into().expect("fixed value length")) as usize;
        if value_len > super::cas::CAS_CHUNK_BYTES {
            self.rollback(storage);
            return Err(StoreError::Corrupt("backup value exceeds CAS chunk cap"));
        }
        let expected_digest: [u8; 32] = rest[6..].try_into().expect("fixed record digest");
        let key_bytes = source.read_exact(key_len)?;
        let key = String::from_utf8(key_bytes.clone())
            .map_err(|_| StoreError::Corrupt("backup key encoding"))?;
        if !(key.starts_with("catalog/")
            || key.starts_with("cas/object/")
            || key.starts_with("cas/chunk/"))
        {
            self.rollback(storage);
            return Err(StoreError::Corrupt("backup key outside store format"));
        }
        let value = source.read_exact(value_len)?;
        if sha256(&value) != expected_digest {
            self.rollback(storage);
            return Err(StoreError::Corrupt("backup record checksum"));
        }
        let mut header = magic;
        header.extend_from_slice(&rest);
        self.root.update(&header);
        self.root.update(&key_bytes);
        self.root.update(&value);
        if key == "catalog/head" {
            if self.pending_head.replace(value).is_some() {
                self.rollback(storage);
                return Err(StoreError::Corrupt("duplicate backup catalog head"));
            }
        } else {
            if let Err(error) = storage.set(&key, value) {
                self.rollback(storage);
                return Err(error.into());
            }
            self.imported_keys.push(key);
        }
        self.entries += 1;
        self.bytes = self.bytes.saturating_add(value_len as u64);
        Ok(self.progress())
    }

    fn rollback(&mut self, storage: &mut dyn StorageValues) {
        for key in self.imported_keys.drain(..) {
            let _ = storage.delete(&key);
        }
    }

    fn progress(&self) -> BackupProgress {
        BackupProgress { entries: self.entries, bytes: self.bytes, done: self.done }
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
        // Durable mutations have dedicated storage-backed state machines.
        // Letting an E1 synchronous operation run against their hot catalog
        // would make memory observable before a generation head is durable.
        store.require_memory_only()?;
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
