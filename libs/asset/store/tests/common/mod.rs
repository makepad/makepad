//! Shared test fixtures: unique on-disk roots and minimal valid manifests
//! whose blob references point at real bytes (the catalog refuses dangling
//! references, so fixtures upload what they reference).
#![allow(dead_code)]

use makepad_asset_store::*;
use makepad_asset_data::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// Raw libsqlite3 shim: open a database file, run SQL, collect the first
/// column of every returned row. Panics on any failure — fixture SQL must
/// work. Used to fabricate byte-real legacy databases for migration tests and
/// to inject mid-transaction faults (triggers) for rollback tests.
pub mod raw {
    use std::ffi::{CStr, CString};
    use std::os::raw::{c_char, c_int, c_void};
    use std::path::Path;

    enum Sqlite3 {}

    type ExecCallback =
        unsafe extern "C" fn(*mut c_void, c_int, *mut *mut c_char, *mut *mut c_char) -> c_int;

    #[link(name = "sqlite3")]
    extern "C" {
        fn sqlite3_open_v2(
            filename: *const c_char,
            db: *mut *mut Sqlite3,
            flags: c_int,
            vfs: *const c_char,
        ) -> c_int;
        fn sqlite3_close(db: *mut Sqlite3) -> c_int;
        fn sqlite3_exec(
            db: *mut Sqlite3,
            sql: *const c_char,
            callback: Option<ExecCallback>,
            arg: *mut c_void,
            errmsg: *mut *mut c_char,
        ) -> c_int;
        fn sqlite3_free(ptr: *mut c_void);
    }

    unsafe extern "C" fn capture_first_column(
        arg: *mut c_void,
        ncols: c_int,
        vals: *mut *mut c_char,
        _names: *mut *mut c_char,
    ) -> c_int {
        if ncols > 0 {
            let out = &mut *(arg as *mut Vec<String>);
            let v = *vals;
            out.push(if v.is_null() {
                String::new()
            } else {
                CStr::from_ptr(v).to_string_lossy().into_owned()
            });
        }
        0
    }

    pub fn exec(path: &Path, sql: &str) -> Vec<String> {
        let cpath = CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        let csql = CString::new(sql).unwrap();
        let mut db: *mut Sqlite3 = std::ptr::null_mut();
        // READWRITE | CREATE | FULLMUTEX, as in src/sqlite.rs.
        let flags = 0x0000_0002 | 0x0000_0004 | 0x0001_0000;
        let rc = unsafe { sqlite3_open_v2(cpath.as_ptr(), &mut db, flags, std::ptr::null()) };
        assert_eq!(rc, 0, "fixture open failed for {path:?}");
        let mut rows: Vec<String> = Vec::new();
        let mut errmsg: *mut c_char = std::ptr::null_mut();
        let rc = unsafe {
            sqlite3_exec(
                db,
                csql.as_ptr(),
                Some(capture_first_column),
                &mut rows as *mut Vec<String> as *mut c_void,
                &mut errmsg,
            )
        };
        let msg = if errmsg.is_null() {
            String::new()
        } else {
            unsafe {
                let m = CStr::from_ptr(errmsg).to_string_lossy().into_owned();
                sqlite3_free(errmsg as *mut c_void);
                m
            }
        };
        unsafe { sqlite3_close(db) };
        assert_eq!(rc, 0, "fixture sql failed: {msg}");
        rows
    }
}

/// The search index generation as stored, via a raw side-channel read.
pub fn read_generation(db: &std::path::Path) -> i64 {
    raw::exec(db, "SELECT generation FROM search_state WHERE id = 1")
        .remove(0)
        .parse()
        .expect("generation is an integer")
}

static DIR_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Deterministic test clock origin; tests advance it explicitly.
pub const NOW: u64 = 1_700_000_000_000;

/// A fresh unique root per call: pid + counter + name, under the OS temp dir.
pub fn test_root(name: &str) -> PathBuf {
    let n = DIR_COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "mp_asset_server_test_{}_{}_{}",
        std::process::id(),
        n,
        name
    ))
}

pub fn open_core(name: &str) -> (PathBuf, AssetServerCore) {
    let root = test_root(name);
    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    (root, core)
}

pub fn asset_id_n(n: u8) -> AssetId {
    AssetId::from_bytes([n; 16])
}

pub fn game_id_n(n: u8) -> GameId {
    GameId::from_bytes([n; 16])
}

pub fn jid(n: u8) -> JobId {
    JobId([n; 16])
}

pub fn pid_n(n: u8) -> PrincipalId {
    PrincipalId([n; 16])
}

/// Full v3 rights fixtures: pinned terms digest, upstream provenance, and
/// source-determined policy.
pub fn cc0_rights(credits: &str, source: &str) -> Rights {
    Rights {
        license: "CC0-1.0".into(),
        license_revision: String::new(),
        terms_digest: Some(sha256(b"CC0-1.0 legal text")),
        terms_url: "https://creativecommons.org/publicdomain/zero/1.0/".into(),
        credits: credits.into(),
        source: source.into(),
        source_archive: None,
        redistribution: Redistribution::Allowed,
        derivatives: DerivativePolicy::Allowed,
    }
}

pub fn kenney_terms() -> Rights {
    Rights {
        source_archive: Some(sha256(b"space-kit-1.0.zip")),
        ..cc0_rights("Kenney (kenney.nl)", "https://kenney.nl/assets/space-kit")
    }
}

/// Minimal valid mesh-bearing manifest: one render GLB plus the mandatory
/// thumbnail, both referencing the digests of the supplied bytes.
pub fn prop_manifest(asset_id: AssetId, glb: &[u8], thumb: &[u8]) -> AssetManifest {
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
        dependencies: vec![],
        thumbnail: Some(ThumbnailMeta {
            blob: BlobId::hash_of(thumb),
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            byte_len: thumb.len() as u64,
        }),
        metrics: Metrics {
            total_bytes: glb.len() as u64 + thumb.len() as u64,
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
        anchors: vec![],
        capabilities: Capabilities {
            rigged: false,
            animated: false,
            collidable: false,
            loopable: false,
            spawnable: false,
        },
        spawn_recipe: None,
        provenance: None,
        rights: cc0_rights("test", ""),
    }
}

/// Upload blobs, register, stage and publish one prop in one call.
pub fn publish_prop(
    core: &AssetServerCore,
    ns: &str,
    id_byte: u8,
    glb: &[u8],
    thumb: &[u8],
    now: u64,
) -> (AssetId, AssetRevisionId) {
    let id = asset_id_n(id_byte);
    core.put_blob(glb, now).unwrap();
    core.put_blob(thumb, now).unwrap();
    core.catalog().register_asset(&id, ns, now).unwrap();
    let manifest = prop_manifest(id, glb, thumb);
    let bytes = manifest.to_canonical_bytes().unwrap();
    let rev = core.catalog().stage_asset_revision(&bytes, now).unwrap();
    core.catalog().publish_asset(&id, &rev, now).unwrap();
    (id, rev)
}

/// Fixed pack source bytes for the Kenney-style import fixture. Content is
/// deterministic so import/derivation identities are identical across clean
/// servers.
pub const PACK_GLB: &[u8] = b"KENNEY-WATCHTOWER-GLB-v1";
pub const PACK_COLLIDER: &[u8] = b"KENNEY-WATCHTOWER-COLLIDER-v1";
pub const PACK_PREVIEW: &[u8] = b"KENNEY-WATCHTOWER-PREVIEW-PNG-v1";
pub const PACK_TEXTURE: &[u8] = b"KENNEY-HULL-PANEL-PNG-v1";

/// CC-BY attribution terms for hostile-rights tests.
pub fn cc_by_terms() -> Rights {
    Rights {
        license: "CC-BY-4.0".into(),
        license_revision: "4.0".into(),
        terms_digest: Some(sha256(b"CC-BY-4.0 legal text")),
        terms_url: "https://creativecommons.org/licenses/by/4.0/legalcode".into(),
        credits: "Example Author".into(),
        source: "https://example.com/pack".into(),
        source_archive: Some(sha256(b"example-pack.zip")),
        redistribution: Redistribution::AttributionRequired,
        derivatives: DerivativePolicy::AttributionRequired,
    }
}

/// A registered collection with explicit terms under an explicit id.
pub fn collection_with_terms(id: &str, terms: Rights) -> SourceCollection {
    SourceCollection {
        id: id.into(),
        title: "Terms fixture".into(),
        origin: SourceOrigin::Upload,
        terms,
    }
}

/// The Kenney pack re-homed under another registered collection: same
/// entries, different source identity and claimed rights.
pub fn pack_with_terms(collection: &SourceCollection) -> ImportManifest {
    let mut pack = kenney_pack("1.0");
    pack.source_id = collection.id.clone();
    pack.source_collection = collection.digest().unwrap();
    pack.rights = collection.terms.clone();
    pack.canonicalize();
    pack
}

/// The approved Kenney source collection fixture. Its terms are the
/// authoritative rights of everything imported under it.
pub fn kenney_collection() -> SourceCollection {
    SourceCollection {
        id: "kenney".into(),
        title: "Kenney game assets".into(),
        origin: SourceOrigin::Upload,
        terms: kenney_terms(),
    }
}

/// A tiny pinned two-asset Kenney-style pack over the fixed source bytes:
/// a mesh prop (render GLB + collider + preview thumbnail) and a texture.
pub fn kenney_pack(version: &str) -> ImportManifest {
    let mut manifest = ImportManifest {
        source_collection: kenney_collection().digest().unwrap(),
        source_id: "kenney".into(),
        pack_name: "space-kit".into(),
        pack_version: version.into(),
        policy_version: IMPORT_ASSET_ID_POLICY_V1,
        assets: vec![
            ImportAsset {
                key: "models/watchtower".parse().unwrap(),
                kind: AssetKind::Prop,
                files: vec![
                    ImportFile {
                        path: "models/watchtower.glb".into(),
                        file: AssetFile {
                            role: FileRole::RenderGlb,
                            tier: DeviceTier::Any,
                            lod: 0,
                            media: MediaType::Glb,
                            blob: BlobId::hash_of(PACK_GLB),
                            byte_len: PACK_GLB.len() as u64,
                            dims: None,
                        },
                    },
                    ImportFile {
                        path: "colliders/watchtower.bin".into(),
                        file: AssetFile {
                            role: FileRole::Collider,
                            tier: DeviceTier::Any,
                            lod: 0,
                            media: MediaType::Bin,
                            blob: BlobId::hash_of(PACK_COLLIDER),
                            byte_len: PACK_COLLIDER.len() as u64,
                            dims: None,
                        },
                    },
                ],
                thumbnail: Some(ImportThumbnail {
                    path: "previews/watchtower.png".into(),
                    meta: ThumbnailMeta {
                        blob: BlobId::hash_of(PACK_PREVIEW),
                        media: ThumbnailMedia::Png,
                        width: 512,
                        height: 512,
                        byte_len: PACK_PREVIEW.len() as u64,
                    },
                }),
                metrics: Metrics {
                    total_bytes: (PACK_GLB.len() + PACK_COLLIDER.len() + PACK_PREVIEW.len())
                        as u64,
                    triangles: 500,
                    vertices: 300,
                    joints: 0,
                    clips: 0,
                    max_texture_dim: 512,
                    media_millis: 0,
                },
                coordinate_system: CoordinateSystem {
                    units_per_meter: 1.0,
                    up: Axis::YPos,
                    forward: Axis::ZNeg,
                    pivot: Pivot::BoundsBottom,
                },
                bounds: Bounds {
                    min: Vec3::new(-1.0, 0.0, -1.0),
                    max: Vec3::new(1.0, 3.0, 1.0),
                },
                anchors: vec![],
                capabilities: Capabilities {
                    collidable: true,
                    ..Default::default()
                },
                spawn_recipe: None,
            },
            ImportAsset {
                key: "textures/hull-panel".parse().unwrap(),
                kind: AssetKind::Texture,
                files: vec![ImportFile {
                    path: "textures/hull_panel.png".into(),
                    file: AssetFile {
                        role: FileRole::Texture,
                        tier: DeviceTier::Any,
                        lod: 0,
                        media: MediaType::Png,
                        blob: BlobId::hash_of(PACK_TEXTURE),
                        byte_len: PACK_TEXTURE.len() as u64,
                        dims: Some(ImageDims {
                            width: 2048,
                            height: 2048,
                        }),
                    },
                }],
                thumbnail: None,
                metrics: Metrics {
                    total_bytes: PACK_TEXTURE.len() as u64,
                    max_texture_dim: 2048,
                    ..Default::default()
                },
                coordinate_system: CoordinateSystem {
                    units_per_meter: 1.0,
                    up: Axis::YPos,
                    forward: Axis::ZNeg,
                    pivot: Pivot::Origin,
                },
                bounds: Bounds {
                    min: Vec3::ZERO,
                    max: Vec3::ONE,
                },
                anchors: vec![],
                capabilities: Capabilities::default(),
                spawn_recipe: None,
            },
        ],
        rights: kenney_terms(),
    };
    manifest.canonicalize();
    manifest
}

/// Upload the pack's source bytes, register the collection, and run the
/// import. Returns the report.
pub fn run_kenney_import(core: &AssetServerCore, version: &str, now: u64) -> ImportReport {
    for bytes in [PACK_GLB, PACK_COLLIDER, PACK_PREVIEW, PACK_TEXTURE] {
        core.put_blob(bytes, now).unwrap();
    }
    let collection_bytes = kenney_collection().to_canonical_bytes().unwrap();
    core.imports().register_source(&collection_bytes, now).unwrap();
    let manifest_bytes = kenney_pack(version).to_canonical_bytes().unwrap();
    core.imports().run_import(&manifest_bytes, now).unwrap()
}

/// Minimal valid game revision manifest over real uploaded bytes.
pub fn game_manifest(
    game_id: GameId,
    splash: &[u8],
    toml: &[u8],
    lock_bytes: &[u8],
    thumb: &[u8],
) -> GameRevisionManifest {
    GameRevisionManifest {
        game_id,
        name: "Test Game".into(),
        description: "integration fixture".into(),
        author: "rik".into(),
        splash_blob: BlobId::hash_of(splash),
        manifest_blob: BlobId::hash_of(toml),
        lock_blob: BlobId::hash_of(lock_bytes),
        thumbnail: ThumbnailMeta {
            blob: BlobId::hash_of(thumb),
            media: ThumbnailMedia::Png,
            width: 512,
            height: 512,
            byte_len: thumb.len() as u64,
        },
        catalog_snapshot: None,
        search_algorithm_version: 1,
        engine_version: 1,
        protocol_version: 1,
        splash_byte_len: splash.len() as u64,
    }
}

/// Canonical lock pinning the given published refs under fixed aliases.
pub fn lock_for(game_id: GameId, refs: &[(&str, AssetRevisionRef)]) -> Vec<u8> {
    let mut lock = ContentLock {
        game_id,
        entries: refs
            .iter()
            .map(|(alias, r)| LockEntry {
                alias: alias.parse().unwrap(),
                asset_id: r.asset_id,
                revision: r.revision,
            })
            .collect(),
        closure: refs.iter().map(|(_, r)| *r).collect(),
        variant_sets: vec![],
    };
    lock.canonicalize();
    lock.to_canonical_bytes().unwrap()
}
