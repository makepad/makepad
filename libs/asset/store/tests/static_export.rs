mod common;

use common::*;
use makepad_asset_data::*;
use makepad_asset_store::json::Value;
use makepad_asset_store::*;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::collections::VecDeque;

struct ExportTransport {
    root: std::path::PathBuf,
    next: u64,
    ready: VecDeque<makepad_asset_client::TransportCompletion>,
}

impl makepad_asset_client::Transport for ExportTransport {
    fn start(&mut self, request: makepad_asset_client::OwnedRequest) -> makepad_asset_client::TransportId {
        let id = makepad_asset_client::TransportId(self.next);
        self.next += 1;
        let route = request.url_or_target.split_once("/v1/")
            .map(|(_, suffix)| format!("v1/{suffix}"))
            .unwrap();
        let result = std::fs::read(self.root.join(route))
            .map(|body| makepad_asset_client::OwnedResponse {
                status: 200,
                headers: vec![("content-length".into(), body.len().to_string())],
                body,
            })
            .map_err(|error| makepad_asset_client::TransportError::Network(error.to_string()));
        self.ready.push_back(makepad_asset_client::TransportCompletion { id, result });
        id
    }

    fn cancel(&mut self, id: makepad_asset_client::TransportId) {
        self.ready.retain(|completion| completion.id != id);
    }

    fn poll(&mut self, out: &mut Vec<makepad_asset_client::TransportCompletion>) {
        out.extend(self.ready.drain(..));
    }
}

fn annotation(kind: AssetKind, title: &str) -> AssetAnnotation {
    AssetAnnotation {
        title: title.into(),
        description: format!("Public description for {title}"),
        kind: Some(kind),
        categories: vec!["fixture".into()],
        tags: vec!["public".into()],
        creator: "Fixture Author".into(),
        owner: Some(pid_n(9)),
        generator: "fixture-generator".into(),
        backend: "fixture-backend".into(),
        model: "fixture-model".into(),
        prompt: "SECRET PROMPT".into(),
        provenance: "SECRET FREE FORM PROVENANCE".into(),
        visibility: Visibility::Public,
    }
}

fn publish_manifest(
    core: &AssetServerCore,
    namespace: &str,
    alias: &str,
    manifest: &AssetManifest,
    now: u64,
) -> AssetRevisionId {
    let bytes = manifest.to_canonical_bytes().unwrap();
    let item = PublishBatchItem {
        namespace: namespace.into(),
        manifest_bytes: bytes,
        annotation: annotation(manifest.kind, alias),
        alias: Some(alias.parse().unwrap()),
    };
    core.publish_batch(&[item], now).unwrap()[0].revision
}

fn list_files(root: &Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        entries.sort();
        for path in entries {
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let relative = path.strip_prefix(root).unwrap();
                let wire = format!("/{}", relative.to_string_lossy().replace('\\', "/"));
                out.insert(wire, std::fs::read(path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

fn parse_file(path: &Path) -> Value {
    makepad_asset_store::json::parse(&std::fs::read(path).unwrap()).unwrap()
}

fn assert_no_forbidden_keys(value: &Value) {
    const DENY: &[&str] = &[
        "token",
        "bearer",
        "prompt",
        "owner",
        "owner_id",
        "chat",
        "rooms",
        "jobs",
        "operations",
        "pipeline_events",
        "blob_reference_path",
        "import_history",
        "provenance",
    ];
    match value {
        Value::Obj(pairs) => {
            for (key, value) in pairs {
                assert!(!DENY.contains(&key.as_str()), "forbidden JSON key {key}");
                assert_no_forbidden_keys(value);
            }
        }
        Value::Arr(values) => {
            for value in values {
                assert_no_forbidden_keys(value);
            }
        }
        _ => {}
    }
}

fn manifest_file_records(manifest: &Value) -> Vec<&Value> {
    manifest.get("files").unwrap().as_arr().unwrap().iter().collect()
}

fn field<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).unwrap().as_str().unwrap()
}

fn assert_integrity_and_sanitization(out: &Path) {
    let manifest = parse_file(&out.join("v1/static/manifest.json"));
    assert_no_forbidden_keys(&manifest);
    let mut recorded = BTreeSet::new();
    for record in manifest_file_records(&manifest) {
        let path = field(record, "path");
        let bytes = std::fs::read(out.join(path.trim_start_matches('/'))).unwrap();
        assert_eq!(record.get("byte_len").unwrap().as_u64(), Some(bytes.len() as u64));
        assert_eq!(field(record, "sha256"), sha256_hex(&bytes));
        recorded.insert(path.to_string());
        if field(record, "content_type") == "application/json"
            && matches!(record.get("content_encoding"), Some(Value::Null))
        {
            assert_no_forbidden_keys(&makepad_asset_store::json::parse(&bytes).unwrap());
        }
    }
    let all: BTreeSet<_> = list_files(out).into_keys().collect();
    let unindexed: BTreeSet<_> = all.difference(&recorded).cloned().collect();
    assert_eq!(
        unindexed,
        BTreeSet::from([
            "/v1/static/manifest.json".to_string(),
            "/v1/static/manifest.json.br".to_string(),
        ])
    );
}

#[test]
fn deterministic_golden_rewrites_graph_and_indexes_every_route() {
    let (root, core) = open_core("static_export_golden");
    let dep_glb = b"DEPENDENCY-GLB";
    let dep_thumb = b"DEPENDENCY-PNG";
    let root_glb = b"ROOT-GLB";
    let root_thumb = b"ROOT-PNG";
    for bytes in [dep_glb.as_slice(), dep_thumb, root_glb, root_thumb] {
        core.put_blob(bytes, NOW).unwrap();
    }

    let mut dependency = prop_manifest(asset_id_n(1), dep_glb, dep_thumb);
    dependency.provenance = Some(Provenance {
        generator: "secret-generator".into(),
        model: "secret-model".into(),
        version: "1".into(),
        seed: 42,
        parents: Vec::new(),
        params_digest: Some(sha256(b"secret params")),
    });
    let dependency_old = publish_manifest(&core, "pub", "pub/fixture/dependency", &dependency, NOW);

    let mut parent = prop_manifest(asset_id_n(2), root_glb, root_thumb);
    parent.dependencies.push(AssetRevisionRef {
        asset_id: dependency.asset_id,
        revision: dependency_old,
    });
    parent.provenance = Some(Provenance {
        generator: "private-pipeline".into(),
        model: "private-model".into(),
        version: "2".into(),
        seed: 99,
        parents: vec![dependency_old],
        params_digest: None,
    });
    parent.canonicalize();
    let parent_old = publish_manifest(&core, "pub", "pub/fixture/parent", &parent, NOW + 1);

    let out_a = root.join("export-a");
    let out_b = root.join("export-b");
    let report_a = export_static(&core, &out_a, &StaticExportOptions::default()).unwrap();
    let report_b = export_static(&core, &out_b, &StaticExportOptions::default()).unwrap();
    assert_eq!(report_a, report_b);
    assert_eq!(list_files(&out_a), list_files(&out_b));
    assert_eq!(report_a.assets, 2);
    assert_eq!(report_a.revisions, 2);
    assert_eq!(report_a.aliases, 2);
    assert_eq!(report_a.snapshot_id.len(), 32);
    assert_eq!(report_a.snapshot_id, "829331efd52b70bb45cfc9fb43479b4b");
    assert!(report_a.snapshot_id.bytes().all(|byte| byte.is_ascii_hexdigit()));

    let alias = parse_file(&out_a.join("v1/aliases/pub/fixture/parent"));
    let parent_new: AssetRevisionId = field(&alias, "head_revision").parse().unwrap();
    assert_ne!(parent_new, parent_old);
    let parent_bytes = std::fs::read(out_a.join(format!("v1/revisions/{parent_new}"))).unwrap();
    let parent_public = AssetManifest::from_canonical_bytes(&parent_bytes).unwrap();
    assert!(parent_public.provenance.is_none());
    assert_eq!(parent_public.rights, parent.rights);
    assert_ne!(parent_public.dependencies[0].revision, dependency_old);
    let dep_new = parent_public.dependencies[0].revision;
    let dep_bytes = std::fs::read(out_a.join(format!("v1/revisions/{dep_new}"))).unwrap();
    let dep_public = AssetManifest::from_canonical_bytes(&dep_bytes).unwrap();
    assert!(dep_public.provenance.is_none());
    assert_eq!(dep_public.rights.credits, "test");
    assert_eq!(AssetRevisionId::hash_of(&parent_bytes), parent_new);
    assert_eq!(AssetRevisionId::hash_of(&dep_bytes), dep_new);

    let health = parse_file(&out_a.join("v1/health"));
    let manifest = parse_file(&out_a.join("v1/static/manifest.json"));
    assert_eq!(field(&health, "server_id"), field(&manifest, "server_id"));
    assert_eq!(manifest.get("static_version").unwrap().as_u64(), Some(1));
    assert_eq!(manifest.get("protocol_version").unwrap().as_u64(), Some(1));
    assert_integrity_and_sanitization(&out_a);

    // The producer's own public export is accepted by the thin-client
    // reader before either side evolves independently.
    let transport = ExportTransport { root: out_a.clone(), next: 1, ready: VecDeque::new() };
    let mut reader = makepad_asset_client::StaticStore::start(
        makepad_asset_client::BaseUrl::parse("https://fixture.invalid").unwrap(),
        Box::new(transport),
        Box::new(makepad_asset_client::MemoryCacheStore::new(1024 * 1024)),
    ).unwrap();
    let events = reader.poll();
    assert!(events.iter().any(|event| matches!(event, makepad_asset_client::StaticStoreEvent::Ready)));
    assert_eq!(reader.assets_page(None, None, 10).unwrap().assets.len(), 2);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn forbidden_and_lan_local_rights_fail_closed() {
    let (root, core) = open_core("static_export_rights");
    for (id, policy, alias) in [
        (3, Redistribution::Forbidden, "pub/rights/forbidden"),
        (4, Redistribution::LanLocal, "pub/rights/lan-local"),
    ] {
        let glb = format!("RIGHTS-GLB-{id}").into_bytes();
        let thumb = format!("RIGHTS-PNG-{id}").into_bytes();
        core.put_blob(&glb, NOW).unwrap();
        core.put_blob(&thumb, NOW).unwrap();
        let mut manifest = prop_manifest(asset_id_n(id), &glb, &thumb);
        manifest.rights.redistribution = policy;
        publish_manifest(&core, "pub", alias, &manifest, NOW + id as u64);
    }
    let out = root.join("export");
    let report = export_static(&core, &out, &StaticExportOptions::default()).unwrap();
    assert_eq!(report.assets, 0);
    assert_eq!(report.aliases, 0);
    assert_eq!(report.excluded_rights, 2);
    let manifest = parse_file(&out.join("v1/static/manifest.json"));
    assert!(manifest.get("assets").unwrap().as_arr().unwrap().is_empty());
    assert!(!out.join("v1/aliases/pub/rights/forbidden").exists());
    assert!(!out.join("v1/aliases/pub/rights/lan-local").exists());
    assert_integrity_and_sanitization(&out);
    let _ = std::fs::remove_dir_all(root);
}

fn video_manifest(id: AssetId, video: &[u8], still: &[u8], thumb: &[u8]) -> AssetManifest {
    let mut manifest = prop_manifest(id, video, thumb);
    manifest.kind = AssetKind::Video;
    manifest.files = vec![
        AssetFile {
            role: FileRole::PreviewFront,
            tier: DeviceTier::Any,
            lod: 0,
            media: MediaType::Jpeg,
            blob: BlobId::hash_of(still),
            byte_len: still.len() as u64,
            dims: Some(ImageDims { width: 32, height: 32 }),
        },
        AssetFile {
            role: FileRole::Video,
            tier: DeviceTier::Any,
            lod: 0,
            media: MediaType::Mp4,
            blob: BlobId::hash_of(video),
            byte_len: video.len() as u64,
            dims: None,
        },
    ];
    manifest.metrics.total_bytes = (still.len() + video.len() + thumb.len()) as u64;
    manifest.canonicalize();
    manifest
}

fn blob_row<'a>(manifest: &'a Value, blob: &BlobId) -> &'a Value {
    manifest
        .get("blobs")
        .unwrap()
        .as_arr()
        .unwrap()
        .iter()
        .find(|row| field(row, "blob") == blob.to_string())
        .unwrap()
}

#[test]
fn video_cap_omits_only_video_and_always_keeps_stills() {
    let (root, core) = open_core("static_export_video");
    let video = b"0123456789ABCDEF";
    let still = b"STILL-JPEG";
    let thumb = b"THUMB-PNG";
    for bytes in [video.as_slice(), still.as_slice(), thumb.as_slice()] {
        core.put_blob(bytes, NOW).unwrap();
    }
    let manifest = video_manifest(asset_id_n(5), video, still, thumb);
    publish_manifest(&core, "pub", "pub/video/clip", &manifest, NOW);

    let out_small = root.join("export-small");
    let small = StaticExportOptions {
        include_video_up_to: video.len() as u64 - 1,
        ..StaticExportOptions::default()
    };
    export_static(&core, &out_small, &small).unwrap();
    let index = parse_file(&out_small.join("v1/static/manifest.json"));
    let video_id = BlobId::hash_of(video);
    let still_id = BlobId::hash_of(still);
    let thumb_id = BlobId::hash_of(thumb);
    assert_eq!(blob_row(&index, &video_id).get("present").unwrap().as_bool(), Some(false));
    assert_eq!(field(blob_row(&index, &video_id), "reason"), "video_cap");
    assert!(!out_small.join(format!("v1/blobs/{video_id}")).exists());
    assert!(out_small.join(format!("v1/blobs/{still_id}")).exists());
    assert!(out_small.join(format!("v1/blobs/{thumb_id}")).exists());

    let out_equal = root.join("export-equal");
    let equal = StaticExportOptions {
        include_video_up_to: video.len() as u64,
        ..StaticExportOptions::default()
    };
    export_static(&core, &out_equal, &equal).unwrap();
    let index = parse_file(&out_equal.join("v1/static/manifest.json"));
    assert_eq!(blob_row(&index, &video_id).get("present").unwrap().as_bool(), Some(true));
    assert!(out_equal.join(format!("v1/blobs/{video_id}")).exists());
    assert_integrity_and_sanitization(&out_small);
    assert_integrity_and_sanitization(&out_equal);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn public_page_is_bounded_sorted_and_redacted_by_type() {
    let (root, core) = open_core("static_export_page");
    for id in [7, 6] {
        let glb = [id, b'g'];
        let thumb = [id, b't'];
        core.put_blob(&glb, NOW).unwrap();
        core.put_blob(&thumb, NOW).unwrap();
        let manifest = prop_manifest(asset_id_n(id), &glb, &thumb);
        publish_manifest(
            &core,
            "pub",
            &format!("pub/page/asset-{id}"),
            &manifest,
            NOW + id as u64,
        );
    }
    let first = core
        .public_export_page(PublicExportFilter {
            namespace: Some("pub"),
            kind: Some(AssetKind::Prop),
            after: None,
            limit: 1,
        })
        .unwrap();
    assert_eq!(first.assets.len(), 1);
    assert_eq!(first.assets[0].asset_id, asset_id_n(6));
    assert_eq!(first.assets[0].search.title, "pub/page/asset-6");
    assert!(first.assets[0]
        .search
        .terms
        .iter()
        .all(|term| !term.term.contains("secret")));
    let second = core
        .public_export_page(PublicExportFilter {
            namespace: Some("pub"),
            kind: Some(AssetKind::Prop),
            after: first.next,
            limit: 1,
        })
        .unwrap();
    assert_eq!(second.assets[0].asset_id, asset_id_n(7));
    assert!(second.next.is_none());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reachable_variant_documents_are_rewritten_to_digest_paths() {
    let (root, core) = open_core("static_export_variants");
    let glb = b"VARIANT-BASE-GLB";
    let thumb = b"VARIANT-BASE-PNG";
    core.put_blob(glb, NOW).unwrap();
    core.put_blob(thumb, NOW).unwrap();
    let mut manifest = prop_manifest(asset_id_n(8), glb, thumb);
    manifest.provenance = Some(Provenance {
        generator: "private-derivation-parent".into(),
        model: "private-model".into(),
        version: "1".into(),
        seed: 7,
        parents: Vec::new(),
        params_digest: None,
    });
    let old_revision = publish_manifest(&core, "pub", "pub/variant/base", &manifest, NOW);
    let base = AssetRevisionRef { asset_id: manifest.asset_id, revision: old_revision };
    let recipe = ProcessingRecipe {
        settings: RecipeSettings::MeshLod { lod: 1, target_triangles: 8 },
        tool: ToolClosure {
            processor: "mp_derive".into(),
            version: "1.0".into(),
            build: "deadbeef".into(),
            deterministic: true,
        },
        output_schema: OUTPUT_SCHEMA_V1,
    };
    let DerivationOutcome::NeedsJob { dkey, job_id, .. } = core
        .variants()
        .begin_derivation(&base, &recipe.to_canonical_bytes().unwrap(), NOW + 1)
        .unwrap()
    else {
        panic!("derivation did not arm");
    };
    let derived_bytes = b"VARIANT-LOD1-GLB";
    core.put_blob(derived_bytes, NOW + 1).unwrap();
    let result = DerivedResult {
        outputs: vec![AssetFile {
            role: FileRole::Lod1Glb,
            tier: DeviceTier::Low,
            lod: 1,
            media: MediaType::Glb,
            blob: BlobId::hash_of(derived_bytes),
            byte_len: derived_bytes.len() as u64,
            dims: None,
        }],
        thumbnail: None,
        metrics: Metrics {
            total_bytes: derived_bytes.len() as u64,
            triangles: 6,
            vertices: 5,
            ..Default::default()
        },
    };
    let variant = core
        .variants()
        .complete_derivation(&dkey, &job_id, "fixture-worker", &result, NOW + 2)
        .unwrap();
    core.variants().freeze_variant_set(&base, &[variant], NOW + 3).unwrap();

    let out = root.join("export");
    export_static(&core, &out, &StaticExportOptions::default()).unwrap();
    let index = parse_file(&out.join("v1/static/manifest.json"));
    let variants = index.get("variants").unwrap().as_arr().unwrap();
    assert_eq!(variants.len(), 1);
    let new_revision: AssetRevisionId = field(&variants[0], "base_revision").parse().unwrap();
    let new_set: VariantSetId = field(&variants[0], "variant_set").parse().unwrap();
    let new_variant: DerivedVariantId = variants[0]
        .get("variants")
        .unwrap()
        .as_arr()
        .unwrap()[0]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(new_revision, old_revision);
    assert!(out.join(format!("v1/variant-sets/{new_set}")).is_file());
    let variant_bytes = std::fs::read(out.join(format!("v1/derived-variants/{new_variant}"))).unwrap();
    let public_variant = DerivedVariantManifest::from_canonical_bytes(&variant_bytes).unwrap();
    assert_eq!(public_variant.base.revision, new_revision);
    assert_eq!(DerivedVariantId::hash_of(&variant_bytes), new_variant);
    assert!(out.join(format!("v1/blobs/{}", BlobId::hash_of(derived_bytes))).is_file());
    assert_integrity_and_sanitization(&out);

    let transport = ExportTransport { root: out.clone(), next: 1, ready: VecDeque::new() };
    let mut reader = makepad_asset_client::StaticStore::start(
        makepad_asset_client::BaseUrl::parse("https://fixture.invalid").unwrap(),
        Box::new(transport),
        Box::new(makepad_asset_client::MemoryCacheStore::new(1024 * 1024)),
    ).unwrap();
    assert!(reader.poll().iter().any(|event| {
        matches!(event, makepad_asset_client::StaticStoreEvent::Ready)
    }));
    let set_fetch = reader
        .start_fetch(makepad_asset_client::StaticFetch::VariantSet(new_set))
        .unwrap();
    let variant_fetch = reader
        .start_fetch(makepad_asset_client::StaticFetch::DerivedVariant(new_variant))
        .unwrap();
    let events = reader.poll();
    assert!(events.iter().any(|event| matches!(
        event,
        makepad_asset_client::StaticStoreEvent::FetchDone {
            id,
            output: makepad_asset_client::StaticFetchOutput::VariantSet(_),
        } if *id == set_fetch
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        makepad_asset_client::StaticStoreEvent::FetchDone {
            id,
            output: makepad_asset_client::StaticFetchOutput::DerivedVariant(_),
        } if *id == variant_fetch
    )));
    let _ = std::fs::remove_dir_all(root);
}
