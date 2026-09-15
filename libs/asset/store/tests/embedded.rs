#![cfg(feature = "embedded")]

use makepad_asset_data::*;
use makepad_asset_store::*;
use makepad_platform::{StorageError, StorageEstimate};
use std::collections::BTreeMap;

const NOW: u64 = 1_700_000_000_000;

fn rights() -> Rights {
    Rights {
        license: "CC0-1.0".into(),
        license_revision: String::new(),
        terms_digest: Some(sha256(b"CC0-1.0 legal text")),
        terms_url: "https://creativecommons.org/publicdomain/zero/1.0/".into(),
        credits: "embedded test".into(),
        source: String::new(),
        source_archive: None,
        redistribution: Redistribution::Allowed,
        derivatives: DerivativePolicy::Allowed,
    }
}

fn manifest(asset_id: AssetId, glb: &[u8], thumbnail: &[u8]) -> AssetManifest {
    AssetManifest {
        asset_id,
        kind: AssetKind::Prop,
        files: vec![AssetFile {
            role: FileRole::RenderGlb,
            tier: DeviceTier::Any,
            lod: 0,
            media: MediaType::Glb,
            blob: BlobId::hash_of(glb),
            byte_len: glb.len() as u64,
            dims: None,
        }],
        dependencies: Vec::new(),
        thumbnail: Some(ThumbnailMeta {
            blob: BlobId::hash_of(thumbnail),
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            byte_len: thumbnail.len() as u64,
            views: Vec::new(),
        }),
        metrics: Metrics {
            total_bytes: (glb.len() + thumbnail.len()) as u64,
            triangles: 12,
            vertices: 8,
            joints: 0,
            clips: 0,
            max_texture_dim: 0,
            media_millis: 0,
        },
        coordinate_system: CoordinateSystem {
            units_per_meter: 1.0,
            up: Axis::YPos,
            forward: Axis::ZNeg,
            pivot: Pivot::Origin,
        },
        bounds: Bounds {
            min: Vec3::new(-1.0, -1.0, -1.0),
            max: Vec3::new(1.0, 1.0, 1.0),
        },
        anchors: Vec::new(),
        capabilities: Capabilities {
            rigged: false,
            animated: false,
            collidable: false,
            loopable: false,
            spawnable: false,
        },
        spawn_recipe: None,
        provenance: None,
        rights: rights(),
    }
}

fn annotation() -> AssetAnnotation {
    AssetAnnotation {
        title: "Portable brass cube".into(),
        description: "Published through the in-process store".into(),
        kind: Some(AssetKind::Prop),
        categories: vec!["fixture".into()],
        tags: vec!["portable".into()],
        creator: "test".into(),
        artist: String::new(),
        artist_url: String::new(),
        album: String::new(),
        source_url: String::new(),
        license: String::new(),
        license_url: String::new(),
        owner: None,
        generator: String::new(),
        backend: String::new(),
        model: String::new(),
        prompt: String::new(),
        provenance: String::new(),
        visibility: Visibility::Public,
    }
}

#[test]
fn in_memory_publish_search_resolve_and_unavailable_capabilities() {
    let store = EmbeddedStore::open_memory(Budgets::default_v1()).unwrap();
    let glb = b"tiny portable glb";
    let thumbnail = b"tiny portable png";
    let asset_id = AssetId::from_bytes([7; 16]);
    let manifest_bytes = manifest(asset_id, glb, thumbnail).to_canonical_bytes().unwrap();
    let alias: AssetAlias = "embedded/cube".parse().unwrap();

    let outcomes = store
        .publish(PublishRequest {
            blobs: vec![
                BlobUpload { expected: BlobId::hash_of(glb), bytes: glb.to_vec() },
                BlobUpload {
                    expected: BlobId::hash_of(thumbnail),
                    bytes: thumbnail.to_vec(),
                },
            ],
            items: vec![PublishBatchItem {
                namespace: "embedded".into(),
                manifest_bytes: manifest_bytes.clone(),
                annotation: annotation(),
                alias: Some(alias.clone()),
            }],
            now_ms: NOW,
        })
        .unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(store.read_blob(&BlobId::hash_of(glb)).unwrap(), glb);
    assert_eq!(store.read_revision(&outcomes[0].revision).unwrap(), Some(manifest_bytes));
    assert_eq!(store.resolve_alias(&alias).unwrap().unwrap().asset_id, asset_id);

    let viewer = SearchViewer { principal: None, scope: ViewerScope::All };
    let listed = store.list(Some("embedded"), 10, &viewer, None).unwrap();
    assert_eq!(listed.hits.len(), 1);
    assert_eq!(
        store
            .public_export_page(PublicExportFilter {
                namespace: Some("embedded"),
                limit: 10,
                ..PublicExportFilter::default()
            })
            .unwrap()
            .assets
            .len(),
        1
    );
    let query = SearchQuery {
        text: "brass",
        filters: SearchFilters::default(),
        page_size: 10,
        expand: false,
        facets: 0,
    };
    assert_eq!(store.search(&query, &viewer, None).unwrap().hits[0].asset_id, asset_id);
    assert_eq!(store.detail(&asset_id).unwrap().unwrap().aliases.len(), 1);

    for (result, capability) in [
        (store.chat(), StoreCapability::Chat),
        (store.jobs(), StoreCapability::Jobs),
        (store.rooms(), StoreCapability::Rooms),
        (store.discovery(), StoreCapability::Discovery),
        (store.observer(), StoreCapability::Observer),
    ] {
        assert_eq!(
            result.unwrap_err(),
            StoreError::Unavailable(StoreUnavailable {
                capability,
                mode: CapabilityMode::Embedded,
            })
        );
    }
}

#[derive(Clone, Default)]
struct FakeStorage {
    values: BTreeMap<String, Vec<u8>>,
    set_calls: usize,
    fail_set: Option<usize>,
    quota_failure: bool,
    max_value: usize,
}

impl FakeStorage {
    fn arm_failure(&mut self, position: usize, quota_failure: bool) {
        self.set_calls = 0;
        self.fail_set = Some(position);
        self.quota_failure = quota_failure;
    }

    fn disarm(&mut self) {
        self.fail_set = None;
        self.quota_failure = false;
    }

    fn flip(&mut self, key: &str) {
        let value = self.values.get_mut(key).unwrap();
        let index = value.len() / 2;
        value[index] ^= 0x40;
    }
}

impl StorageValues for FakeStorage {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>, StorageError> {
        Ok(self.values.get(key).cloned())
    }

    fn set(&mut self, key: &str, value: Vec<u8>) -> Result<(), StorageError> {
        self.set_calls += 1;
        if self.fail_set == Some(self.set_calls) {
            return Err(if self.quota_failure {
                StorageError::QuotaExceeded("injected quota exhaustion".into())
            } else {
                StorageError::Backend("injected power loss".into())
            });
        }
        self.max_value = self.max_value.max(value.len());
        self.values.insert(key.into(), value);
        Ok(())
    }

    fn delete(&mut self, key: &str) -> Result<(), StorageError> {
        self.values.remove(key);
        Ok(())
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, StorageError> {
        Ok(self
            .values
            .keys()
            .filter(|key| key.starts_with(prefix))
            .cloned()
            .collect())
    }
}

fn estimate() -> StorageEstimate {
    StorageEstimate { usage: 0, quota: 4 * 1024 * 1024 * 1024 }
}

fn publish_request(glb: &[u8], thumbnail: &[u8]) -> (PublishRequest, AssetAlias, AssetId) {
    let asset_id = AssetId::from_bytes([23; 16]);
    let alias: AssetAlias = "embedded/durable".parse().unwrap();
    let manifest_bytes = manifest(asset_id, glb, thumbnail).to_canonical_bytes().unwrap();
    (
        PublishRequest {
            blobs: vec![
                BlobUpload { expected: BlobId::hash_of(glb), bytes: glb.to_vec() },
                BlobUpload {
                    expected: BlobId::hash_of(thumbnail),
                    bytes: thumbnail.to_vec(),
                },
            ],
            items: vec![PublishBatchItem {
                namespace: "embedded".into(),
                manifest_bytes,
                annotation: annotation(),
                alias: Some(alias.clone()),
            }],
            now_ms: NOW,
        },
        alias,
        asset_id,
    )
}

fn fresh_durable(storage: &mut FakeStorage) -> EmbeddedStore {
    EmbeddedStore::open_durable(
        storage,
        Budgets::default_v1(),
        QuotaPolicy::default(),
    )
    .unwrap()
}

#[test]
fn quota_preflight_and_every_set_failure_leave_old_head_visible() {
    let mut baseline = FakeStorage::default();
    let store = fresh_durable(&mut baseline);
    let old_head = baseline.values["catalog/head"].clone();
    drop(store);

    let (request, alias, _) = publish_request(b"durable glb", b"durable png");
    let mut success_storage = baseline.clone();
    success_storage.set_calls = 0;
    let mut success_store = fresh_durable(&mut success_storage);
    success_store
        .publish_durable(&mut success_storage, estimate(), request.clone())
        .unwrap();
    let set_count = success_storage.set_calls;
    assert!(set_count >= 10, "expected every CAS/catalog publication phase");

    let mut preflight_storage = baseline.clone();
    preflight_storage.set_calls = 0;
    let mut preflight_store = fresh_durable(&mut preflight_storage);
    let refusal = preflight_store
        .publish_durable(
            &mut preflight_storage,
            StorageEstimate { usage: 99, quota: 100 },
            request.clone(),
        )
        .unwrap_err();
    assert!(matches!(refusal, StoreError::QuotaExceeded(_)));
    assert_eq!(preflight_storage.set_calls, 0);
    assert_eq!(preflight_storage.values["catalog/head"], old_head);

    for position in 1..=set_count {
        let mut storage = baseline.clone();
        let mut store = fresh_durable(&mut storage);
        storage.arm_failure(position, true);
        let error = store
            .publish_durable(&mut storage, estimate(), request.clone())
            .unwrap_err();
        assert!(matches!(error, StoreError::QuotaExceeded(_)), "set {position}: {error:?}");
        storage.disarm();
        assert_eq!(storage.values["catalog/head"], old_head, "set {position}");
        let restored = fresh_durable(&mut storage);
        assert_eq!(restored.resolve_alias(&alias).unwrap(), None, "set {position}");
        assert!(matches!(
            restored.read_blob_durable(&storage, &BlobId::hash_of(b"durable glb")),
            Err(StoreError::Core(ServerError::NotFound { .. }))
        ));
    }
}

#[test]
fn power_loss_matrix_restores_only_a_headed_generation() {
    let mut baseline = FakeStorage::default();
    fresh_durable(&mut baseline);
    let old_head = baseline.values["catalog/head"].clone();
    let (request, alias, _) = publish_request(b"power glb", b"power png");
    let mut complete = baseline.clone();
    complete.set_calls = 0;
    fresh_durable(&mut complete)
        .publish_durable(&mut complete, estimate(), request.clone())
        .unwrap();
    let set_count = complete.set_calls;

    for position in 1..=set_count {
        let mut storage = baseline.clone();
        let mut store = fresh_durable(&mut storage);
        storage.arm_failure(position, false);
        assert!(store
            .publish_durable(&mut storage, estimate(), request.clone())
            .is_err());
        storage.disarm();
        assert_eq!(storage.values["catalog/head"], old_head);
        let restored = fresh_durable(&mut storage);
        assert_eq!(restored.resolve_alias(&alias).unwrap(), None);
    }
}

#[test]
fn catalog_and_cas_corruption_fail_closed() {
    let mut baseline = FakeStorage::default();
    fresh_durable(&mut baseline);
    let baseline_keys: Vec<_> = baseline.values.keys().cloned().collect();
    let (request, alias, _) = publish_request(b"flip glb", b"flip png");
    let mut published = baseline.clone();
    fresh_durable(&mut published)
        .publish_durable(&mut published, estimate(), request)
        .unwrap();

    let latest_extent = published
        .values
        .keys()
        .find(|key| key.starts_with("catalog/chunk/") && !baseline_keys.contains(key))
        .unwrap()
        .clone();
    let mut corrupt_catalog = published.clone();
    corrupt_catalog.flip(&latest_extent);
    let restored = fresh_durable(&mut corrupt_catalog);
    assert_eq!(restored.resolve_alias(&alias).unwrap(), None);

    let blob = BlobId::hash_of(b"flip glb");
    let mut corrupt_manifest = published.clone();
    corrupt_manifest.flip(&object_key(&blob));
    let store = fresh_durable(&mut corrupt_manifest);
    assert!(store.read_blob_durable(&corrupt_manifest, &blob).is_err());

    let chunk = chunk_key(*blob.as_bytes());
    let mut corrupt_chunk = published;
    corrupt_chunk.flip(&chunk);
    let store = fresh_durable(&mut corrupt_chunk);
    assert!(matches!(
        store.read_blob_durable(&corrupt_chunk, &blob),
        Err(StoreError::Core(ServerError::DigestMismatch { .. }))
    ));
}

#[test]
fn storage_upload_slices_cpu_and_caps_values() {
    let max = CAS_CHUNK_BYTES + 17;
    let bytes = vec![0x5a; max];
    let expected = BlobId::hash_of(&bytes);
    let cas = StorageCas::new(max as u64);
    let mut storage = FakeStorage::default();
    let mut upload = cas
        .start_upload(&storage, "max-configured", bytes, expected, NOW)
        .unwrap();
    loop {
        let progress = upload.step(&mut storage).unwrap();
        assert!(progress.cpu_bytes as usize <= CAS_HASH_SLICE_BYTES);
        if progress.done {
            break;
        }
    }
    assert!(storage.max_value <= CAS_CHUNK_BYTES);
    let mut read = cas.start_read(&storage, &expected).unwrap();
    loop {
        let progress = read.step(&storage).unwrap();
        assert!(progress.cpu_bytes as usize <= CAS_HASH_SLICE_BYTES);
        if progress.done {
            break;
        }
    }
    assert_eq!(read.finish().unwrap().len(), max);

    // Cancellation leaves only bounded, non-visible partials. Once the cap
    // is occupied, a third upload is refused before it can write anything.
    let bounded = StorageCas::new(16).with_partial_limit(2);
    for upload_id in ["cancel-a", "cancel-b"] {
        let bytes = vec![upload_id.as_bytes()[0]];
        let expected = BlobId::hash_of(&bytes);
        let mut upload = bounded
            .start_upload(&storage, upload_id, bytes, expected, NOW)
            .unwrap();
        assert!(!upload.step(&mut storage).unwrap().done);
        drop(upload);
    }
    let bytes = vec![3];
    assert!(matches!(
        bounded.start_upload(&storage, "cancel-c", bytes.clone(), BlobId::hash_of(&bytes), NOW),
        Err(StorageCasError::Core(ServerError::OverBudget { what: "partial uploads", .. }))
    ));
    assert_eq!(storage.list("cas/partial/").unwrap().len(), 2);
}

#[derive(Default)]
struct VecSink(Vec<u8>);

impl AssetStoreArchiveSink for VecSink {
    fn write(&mut self, bytes: &[u8]) -> StoreResult<()> {
        self.0.extend_from_slice(bytes);
        Ok(())
    }
}

struct SliceSource {
    bytes: Vec<u8>,
    offset: usize,
}

impl AssetStoreArchiveSource for SliceSource {
    fn read_exact(&mut self, len: usize) -> StoreResult<Vec<u8>> {
        let end = self.offset.checked_add(len).ok_or(StoreError::Corrupt("archive read"))?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .ok_or(StoreError::Corrupt("truncated archive"))?
            .to_vec();
        self.offset = end;
        Ok(bytes)
    }
}

#[test]
fn whole_store_container_round_trips_into_a_fresh_store() {
    let mut source_storage = FakeStorage::default();
    let (request, alias, _) = publish_request(b"backup glb", b"backup png");
    fresh_durable(&mut source_storage)
        .publish_durable(&mut source_storage, estimate(), request)
        .unwrap();
    let mut export = BackupExport::new(&source_storage).unwrap();
    let mut sink = VecSink::default();
    while !export.step(&source_storage, &mut sink).unwrap().done {}

    let mut target_storage = FakeStorage::default();
    let mut import = BackupImport::new(&target_storage).unwrap();
    let mut source = SliceSource { bytes: sink.0, offset: 0 };
    while !import.step(&mut target_storage, &mut source).unwrap().done {}
    let restored = fresh_durable(&mut target_storage);
    assert!(restored.resolve_alias(&alias).unwrap().is_some());
    assert_eq!(
        restored
            .read_blob_durable(&target_storage, &BlobId::hash_of(b"backup glb"))
            .unwrap(),
        b"backup glb"
    );
}

#[test]
fn gc_intent_survives_crashes_around_physical_delete_and_clear() {
    let mut storage = FakeStorage::default();
    let (request, _, asset_id) = publish_request(b"gc glb", b"gc png");
    let mut store = fresh_durable(&mut storage);
    store.publish_durable(&mut storage, estimate(), request).unwrap();
    store.retire_asset_durable(&mut storage, &asset_id, NOW + 1).unwrap();
    store
        .begin_gc_durable(
            &mut storage,
            GcConfig { grace_ms: 0, ..GcConfig::default_v1() },
            NOW + 2,
        )
        .unwrap();
    let intents = loop {
        let step = store.gc_catalog_step_durable(&mut storage, NOW + 3).unwrap();
        if !step.deletes.is_empty() {
            break step.deletes;
        }
        assert!(!step.status.finished(), "GC finished without delete intents");
    };
    let blob = BlobId::hash_of(b"gc glb");
    assert!(storage.values.contains_key(&object_key(&blob)));

    let mut after_intent = storage.clone();
    let mut recovered = fresh_durable(&mut after_intent);
    assert!(matches!(
        recovered.read_blob_durable(&after_intent, &blob),
        Err(StoreError::Core(ServerError::NotFound { .. }))
    ));
    assert_eq!(recovered.recover_gc_durable(&mut after_intent).unwrap(), intents.len() as u64);
    assert!(!after_intent.values.contains_key(&object_key(&blob)));

    let mut after_object_delete = storage.clone();
    after_object_delete.values.remove(&object_key(&blob));
    let mut recovered = fresh_durable(&mut after_object_delete);
    assert_eq!(
        recovered.recover_gc_durable(&mut after_object_delete).unwrap(),
        intents.len() as u64
    );
    assert_eq!(recovered.recover_gc_durable(&mut after_object_delete).unwrap(), 0);

    for intent in intents {
        store
            .complete_gc_delete_durable(&mut storage, &intent.blob_id)
            .unwrap();
    }
    let mut after_clear = storage;
    let mut recovered = fresh_durable(&mut after_clear);
    assert_eq!(recovered.recover_gc_durable(&mut after_clear).unwrap(), 0);
}
