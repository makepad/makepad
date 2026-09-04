mod common;

use common::*;
use makepad_asset_data::*;
use makepad_asset_store::json::Value;
use makepad_asset_store::*;
use std::collections::{BTreeMap, BTreeSet};
use std::collections::VecDeque;
use std::path::Path;
use std::process::Command;

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
        artist: String::new(),
        artist_url: String::new(),
        album: String::new(),
        source_url: String::new(),
        license: String::new(),
        license_url: String::new(),
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

fn publish_manifest_without_alias(
    core: &AssetServerCore,
    namespace: &str,
    manifest: &AssetManifest,
    now: u64,
) -> AssetRevisionId {
    let item = PublishBatchItem {
        namespace: namespace.into(),
        manifest_bytes: manifest.to_canonical_bytes().unwrap(),
        annotation: annotation(manifest.kind, "unaliased fixture"),
        alias: None,
    };
    core.publish_batch(&[item], now).unwrap()[0].revision
}

fn audio_manifest(id: AssetId, audio: &[u8], lyrics: Option<&[u8]>) -> AssetManifest {
    let mut manifest = prop_manifest(id, b"unused", b"unused");
    manifest.kind = AssetKind::Audio;
    manifest.thumbnail = None;
    manifest.files = vec![AssetFile {
        role: FileRole::Audio,
        tier: DeviceTier::Any,
        lod: 0,
        media: MediaType::Mp3,
        blob: BlobId::hash_of(audio),
        byte_len: audio.len() as u64,
        dims: None,
    }];
    if let Some(lyrics) = lyrics {
        manifest.files.push(AssetFile {
            role: FileRole::Lyrics,
            tier: DeviceTier::Any,
            lod: 0,
            media: MediaType::Json,
            blob: BlobId::hash_of(lyrics),
            byte_len: lyrics.len() as u64,
            dims: None,
        });
    }
    manifest.metrics = Metrics {
        total_bytes: manifest.files.iter().map(|file| file.byte_len).sum(),
        media_millis: 1_000,
        ..Default::default()
    };
    manifest.canonicalize();
    manifest
}

/// Encode the v3 shape stored by pre-thumbnail-view servers. V4 appended one
/// empty view count after the five unchanged thumbnail fields.
fn content_v3_asset_bytes(manifest: &AssetManifest) -> Vec<u8> {
    let thumbnail = manifest.thumbnail.as_ref().expect("fixture has a thumbnail");
    assert!(thumbnail.views.is_empty());
    let mut bytes = manifest.to_canonical_bytes().unwrap();
    assert_eq!(&bytes[..7], b"MPC1\x01\x00\x04");

    let offsets: Vec<_> = bytes
        .windows(thumbnail.blob.as_bytes().len())
        .enumerate()
        .filter_map(|(offset, value)| (value == thumbnail.blob.as_bytes()).then_some(offset))
        .collect();
    assert_eq!(offsets.len(), 1, "thumbnail digest must be unique in fixture bytes");
    let views_count = offsets[0] + 32 + 1 + 4 + 4 + 8;
    assert_eq!(&bytes[views_count..views_count + 4], &[0; 4]);
    bytes.drain(views_count..views_count + 4);
    bytes[5..7].copy_from_slice(&3u16.to_be_bytes());
    bytes
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        write!(out, "{byte:02x}").unwrap();
    }
    out
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
fn schema_v3_asset_manifest_is_migrated_during_static_export() {
    let (root, core) = open_core("static_export_content_v3");
    let glb = b"V3-GLB";
    let thumb = b"V3-PNG";
    core.put_blob(glb, NOW).unwrap();
    core.put_blob(thumb, NOW).unwrap();
    let manifest = prop_manifest(asset_id_n(31), glb, thumb);
    let v3_bytes = content_v3_asset_bytes(&manifest);
    let migrated = AssetManifest::from_canonical_bytes(&v3_bytes).unwrap();
    assert_eq!(migrated, manifest);
    assert!(migrated.thumbnail.as_ref().unwrap().views.is_empty());

    let old_revision = AssetRevisionId::hash_of(&v3_bytes);
    let outcome = core
        .publish_batch(
            &[PublishBatchItem {
                namespace: "music".into(),
                manifest_bytes: v3_bytes,
                annotation: annotation(AssetKind::Prop, "v3 fixture"),
                alias: Some("music/v3-fixture".parse().unwrap()),
            }],
            NOW,
        )
        .unwrap();
    assert_eq!(outcome[0].revision, old_revision);

    let reader = AssetServerCore::open_read_only(&root, Budgets::default_v1()).unwrap();
    let out = root.join("export");
    let report = export_static(&reader, &out, &StaticExportOptions::default()).unwrap();
    assert_eq!((report.assets, report.revisions), (1, 1));

    let migrated_bytes = migrated.to_canonical_bytes().unwrap();
    let migrated_revision = AssetRevisionId::hash_of(&migrated_bytes);
    assert_ne!(migrated_revision, old_revision);
    let exported = std::fs::read(out.join(format!("v1/revisions/{migrated_revision}"))).unwrap();
    assert_eq!(exported, migrated_bytes);
    assert_eq!(u16::from_be_bytes([exported[5], exported[6]]), CONTENT_SCHEMA_VERSION);

    drop(reader);
    drop(core);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn runtime_schema_failure_names_table_and_does_not_print_usage() {
    let (root, core) = open_core("static_export_content_schema_error");
    let glb = b"FUTURE-GLB";
    let thumb = b"FUTURE-PNG";
    core.put_blob(glb, NOW).unwrap();
    core.put_blob(thumb, NOW).unwrap();
    let manifest = prop_manifest(asset_id_n(32), glb, thumb);
    publish_manifest(&core, "music", "music/future-fixture", &manifest, NOW);
    drop(core);

    let mut future = manifest.to_canonical_bytes().unwrap();
    let found = CONTENT_SCHEMA_VERSION + 1;
    future[5..7].copy_from_slice(&found.to_be_bytes());
    raw::exec(
        &root.join("catalog.sqlite3"),
        &format!("UPDATE asset_revisions SET manifest=X'{}'", hex(&future)),
    );

    let out = root.join("export");
    let result = Command::new(env!("CARGO_BIN_EXE_makepad-asset-store"))
        .arg("export-static")
        .arg(&root)
        .arg(&out)
        .output()
        .unwrap();
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8(result.stderr).unwrap();
    assert!(stderr.contains(&format!(
        "unsupported content schema in catalog.sqlite3 asset_revisions.manifest: expected {}..={}, found {found}",
        MIN_READABLE_CONTENT_SCHEMA_VERSION, CONTENT_SCHEMA_VERSION,
    )));
    assert!(!stderr.contains("makepad-asset-store --root"));

    let _ = std::fs::remove_dir_all(root);
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
    assert_eq!(
        report_a.snapshot_id, "7331ed62a0e519599984c2ae4904f6a4",
        "static export contract changed; if intentional, bump this golden to the `left` value printed above",
    );
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
fn namespace_kind_and_limit_include_unaliased_assets_and_side_channels_read_only() {
    let (root, core) = open_core("static_export_filters");
    let song_a = b"MP3-A";
    let song_b = b"MP3-B";
    let lyrics = br#"{"lines":[]}"#;
    let prop = b"PROP-GLB";
    let thumb = b"PROP-PNG";
    for bytes in [
        song_a.as_slice(),
        song_b.as_slice(),
        lyrics.as_slice(),
        prop.as_slice(),
        thumb.as_slice(),
    ] {
        core.put_blob(bytes, NOW).unwrap();
    }

    let audio_a = audio_manifest(asset_id_n(10), song_a, Some(lyrics));
    let audio_b = audio_manifest(asset_id_n(11), song_b, None);
    publish_manifest_without_alias(&core, "music", &audio_a, NOW);
    publish_manifest_without_alias(&core, "music", &audio_b, NOW + 1);
    let prop_manifest = prop_manifest(asset_id_n(12), prop, thumb);
    publish_manifest(&core, "other", "other/fixture/prop", &prop_manifest, NOW + 2);

    // The exporter opens a separate read-only WAL reader while the writer is
    // still alive; it neither needs nor takes the host's server.lock.
    let reader = AssetServerCore::open_read_only(&root, Budgets::default_v1()).unwrap();

    let out_all = root.join("export-all");
    let all = export_static(&reader, &out_all, &StaticExportOptions::default()).unwrap();
    assert_eq!((all.assets, all.revisions, all.aliases), (3, 3, 1));

    let out_ns = root.join("export-ns");
    let by_ns = export_static(
        &reader,
        &out_ns,
        &StaticExportOptions {
            namespace: Some("music".into()),
            ..StaticExportOptions::default()
        },
    )
    .unwrap();
    assert_eq!((by_ns.assets, by_ns.revisions, by_ns.aliases), (2, 2, 0));
    assert!(out_ns.join(format!("v1/blobs/{}", BlobId::hash_of(lyrics))).is_file());

    let out_kind = root.join("export-kind");
    let by_kind = export_static(
        &reader,
        &out_kind,
        &StaticExportOptions {
            kind: Some(AssetKind::Audio),
            ..StaticExportOptions::default()
        },
    )
    .unwrap();
    assert_eq!((by_kind.assets, by_kind.revisions), (2, 2));

    let out_zero = root.join("export-zero");
    let zero = export_static(
        &reader,
        &out_zero,
        &StaticExportOptions {
            namespace: Some("music".into()),
            kind: Some(AssetKind::Prop),
            ..StaticExportOptions::default()
        },
    )
    .unwrap();
    assert_eq!(zero.assets, 0);
    assert_eq!(zero.live_assets_considered, 3);
    assert_eq!(zero.excluded_namespace_mismatch, 1);
    assert_eq!(zero.excluded_kind_mismatch, 2);

    let out_limit = root.join("export-limit");
    let limited = export_static(
        &reader,
        &out_limit,
        &StaticExportOptions {
            namespace: Some("music".into()),
            limit: Some(1),
            ..StaticExportOptions::default()
        },
    )
    .unwrap();
    assert_eq!((limited.assets, limited.revisions), (1, 1));
    assert_eq!(limited.excluded_limit, 1);

    drop(reader);
    drop(core);
    raw::exec(&root.join("catalog.sqlite3"), "PRAGMA user_version=9");
    let schema_v9_reader = AssetServerCore::open_read_only(&root, Budgets::default_v1()).unwrap();
    assert_eq!(
        schema_v9_reader
            .public_export_page(PublicExportFilter {
                namespace: Some("music"),
                limit: 10,
                ..PublicExportFilter::default()
            })
            .unwrap()
            .assets
            .len(),
        2
    );
    drop(schema_v9_reader);
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
