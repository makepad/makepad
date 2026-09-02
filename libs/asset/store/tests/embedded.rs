#![cfg(feature = "embedded")]

use makepad_asset_data::*;
use makepad_asset_store::*;

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
