//! Schema migration: fresh roots land on the current version, v1 roots (with
//! and without the ad-hoc search tables that predate v2) migrate forward with
//! their data intact, and versions this build does not speak refuse to open.
//!
//! Fixture databases are fabricated through the minimal raw-sqlite shim in
//! tests/common (the test-side twin of src/sqlite.rs) so the legacy layouts
//! are byte-real, not simulated through current-code hooks.

/// The job queue left the store (aicore P7); migration tests still build
/// faithful HISTORICAL roots, so its DDL is pinned here as data.
const HISTORICAL_JOBS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS jobs(
    job_id BLOB PRIMARY KEY,
    parent_job BLOB,
    kind TEXT NOT NULL,
    payload BLOB NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('pending','running','succeeded','failed','cancelled')),
    attempts_used INTEGER NOT NULL DEFAULT 0,
    max_attempts INTEGER NOT NULL,
    priority INTEGER NOT NULL,
    not_before_ms INTEGER NOT NULL,
    created_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS jobs_by_state ON jobs(state, not_before_ms);
CREATE INDEX IF NOT EXISTS jobs_by_parent ON jobs(parent_job);
CREATE TABLE IF NOT EXISTS job_deps(
    job_id BLOB NOT NULL,
    depends_on BLOB NOT NULL,
    PRIMARY KEY(job_id, depends_on)
);
CREATE TABLE IF NOT EXISTS job_attempts(
    job_id BLOB NOT NULL,
    attempt INTEGER NOT NULL,
    worker TEXT NOT NULL,
    started_ms INTEGER NOT NULL,
    ended_ms INTEGER,
    outcome TEXT NOT NULL DEFAULT 'running',
    PRIMARY KEY(job_id, attempt)
);
CREATE TABLE IF NOT EXISTS job_leases(
    job_id BLOB PRIMARY KEY,
    worker TEXT NOT NULL,
    attempt INTEGER NOT NULL,
    expires_ms INTEGER NOT NULL,
    heartbeat_ms INTEGER NOT NULL
);
";

const HISTORICAL_OPERATIONS_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS operations(
    operation_id BLOB PRIMARY KEY,
    owner BLOB NOT NULL,
    namespace TEXT NOT NULL,
    kind TEXT NOT NULL,
    def_revision INTEGER NOT NULL,
    idempotency_key TEXT NOT NULL,
    spec_digest BLOB NOT NULL,
    spec BLOB NOT NULL,
    state TEXT NOT NULL CHECK(state IN ('queued','succeeded','failed','cancelled')),
    round INTEGER NOT NULL DEFAULT 0,
    job_id BLOB NOT NULL,
    error TEXT,
    result_asset BLOB,
    result_revision BLOB,
    created_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS operations_idem
    ON operations(owner, namespace, idempotency_key);
CREATE INDEX IF NOT EXISTS operations_by_job ON operations(job_id);
CREATE TABLE IF NOT EXISTS operation_events(
    operation_id BLOB NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    detail TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    PRIMARY KEY(operation_id, seq)
);
CREATE TABLE IF NOT EXISTS operation_worker_seen(
    kind TEXT PRIMARY KEY,
    last_seen_ms INTEGER NOT NULL
);
";


mod common;
use common::*;
use makepad_asset_store::{
    auth::AUTH_SCHEMA, catalog::CATALOG_SCHEMA, AssetAnnotation,
    AssetServerCore, Budgets, SearchFilters, SearchQuery, SearchViewer, ServerError, ViewerScope,
    Visibility, SERVER_SCHEMA_VERSION,
};
use makepad_asset_data::AssetKind;
use std::path::{Path, PathBuf};

/// The v1 search DDL exactly as it shipped: no kind column, no kind index.
const LEGACY_SEARCH_V1: &str = "
CREATE TABLE IF NOT EXISTS search_annotations(
    asset_id BLOB PRIMARY KEY,
    namespace TEXT NOT NULL,
    visibility TEXT NOT NULL CHECK(visibility IN ('public','private')),
    owner BLOB,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    creator TEXT NOT NULL,
    generator TEXT NOT NULL,
    backend TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt TEXT NOT NULL,
    provenance TEXT NOT NULL,
    live INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS search_annotations_by_ns ON search_annotations(namespace);
CREATE TABLE IF NOT EXISTS search_labels(
    asset_id BLOB NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('category','tag')),
    label TEXT NOT NULL,
    PRIMARY KEY(asset_id, kind, label)
);
CREATE INDEX IF NOT EXISTS search_labels_by_label ON search_labels(kind, label);
CREATE TABLE IF NOT EXISTS search_postings(
    term TEXT NOT NULL,
    asset_id BLOB NOT NULL,
    weight_public INTEGER NOT NULL,
    weight_owner INTEGER NOT NULL,
    PRIMARY KEY(term, asset_id)
);
CREATE INDEX IF NOT EXISTS search_postings_by_asset ON search_postings(asset_id);
";

/// [1; 16] as a SQLite blob literal — `asset_id_n(1)` in fixture SQL.
const ID1_HEX: &str = "X'01010101010101010101010101010101'";

fn fixture_root(name: &str, sql: &str) -> (PathBuf, PathBuf) {
    let root = test_root(name);
    std::fs::create_dir_all(&root).unwrap();
    let db = root.join("catalog.sqlite3");
    raw::exec(&db, sql);
    (root, db)
}

fn user_version(db: &Path) -> String {
    raw::exec(db, "PRAGMA user_version").remove(0)
}

fn ann(title: &str) -> AssetAnnotation {
    AssetAnnotation {
        title: title.into(),
        description: String::new(),
        kind: None,
        categories: Vec::new(),
        tags: Vec::new(),
        creator: String::new(),
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

fn q(text: &str) -> SearchQuery<'_> {
    SearchQuery {
        text,
        filters: SearchFilters::default(),
        expand: false,
        page_size: 10,
        newest: false,
        facets: 0,
    }
}

const ANYONE: SearchViewer<'static> = SearchViewer { principal: None, scope: ViewerScope::All };

#[test]
fn fresh_root_lands_at_current_version() {
    let root = test_root("fresh_version");
    {
        let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
        core.catalog().register_asset(&asset_id_n(1), "rik2", NOW).unwrap();
        core.search().set_annotation(&asset_id_n(1), &ann("fresh thing"), NOW).unwrap();
    }
    assert_eq!(user_version(&root.join("catalog.sqlite3")), SERVER_SCHEMA_VERSION.to_string());
    // Reopening a current-version root is a no-op migrate.
    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    assert_eq!(core.search().search(&q("fresh"), &ANYONE, None).unwrap().total, 1);
}

#[test]
fn presearch_v1_root_migrates_to_current() {
    // A faithful pre-search v1 root: catalog/jobs/auth tables only.
    let (root, db) = fixture_root(
        "presearch_v1",
        &format!(
            "{CATALOG_SCHEMA}{HISTORICAL_JOBS_SCHEMA}{AUTH_SCHEMA}\
             INSERT INTO assets(asset_id, namespace, created_ms) VALUES({ID1_HEX},'rik2',1);\
             PRAGMA user_version=1;"
        ),
    );
    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    // The migrated root has working search including the kind column.
    let mut a = ann("migrated relic");
    a.kind = Some(AssetKind::Prop);
    core.search().set_annotation(&asset_id_n(1), &a, NOW).unwrap();
    let mut query = q("");
    query.filters = SearchFilters { kind: Some(AssetKind::Prop), ..Default::default() };
    assert_eq!(core.search().search(&query, &ANYONE, None).unwrap().total, 1);
    drop(core);
    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
}

#[test]
fn legacy_search_v1_root_is_retrofitted_with_data_intact() {
    // A v1 root that already carried the ad-hoc search tables (pre-kind),
    // with a live-looking annotation, postings and a label in place.
    let (root, db) = fixture_root(
        "legacy_search_v1",
        &format!(
            "{CATALOG_SCHEMA}{HISTORICAL_JOBS_SCHEMA}{AUTH_SCHEMA}{LEGACY_SEARCH_V1}\
             INSERT INTO assets(asset_id, namespace, created_ms) VALUES({ID1_HEX},'rik2',1);\
             INSERT INTO search_annotations(asset_id, namespace, visibility, owner,\
                title, description, creator, generator, backend, model,\
                prompt, provenance, live, updated_ms)\
              VALUES({ID1_HEX},'rik2','public',NULL,'Legacy Lantern',\
                'an oil lantern admitted before the kind column existed',\
                'rik','trellis','cuda','xl','','',0,1);\
             INSERT INTO search_labels(asset_id, kind, label) VALUES({ID1_HEX},'tag','durable');\
             INSERT INTO search_postings(term, asset_id, weight_public, weight_owner)\
              VALUES('legacy',{ID1_HEX},100,100),('lantern',{ID1_HEX},120,120),\
                    ('oil',{ID1_HEX},20,20);\
             PRAGMA user_version=1;"
        ),
    );
    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    let search = core.search();

    // Pre-migration data is fully searchable; the retrofitted kind reads None.
    let page = search.search(&q("lantern"), &ANYONE, None).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.hits[0].title, "Legacy Lantern");
    assert_eq!(page.hits[0].kind, None);
    assert!(!page.hits[0].live);
    let mut tagged = q("");
    tagged.filters = SearchFilters { tag: Some("durable"), ..Default::default() };
    assert_eq!(search.search(&tagged, &ANYONE, None).unwrap().total, 1);
    let read = search.annotation(&asset_id_n(1)).unwrap().unwrap();
    assert_eq!((read.title.as_str(), read.kind), ("Legacy Lantern", None));

    // Legacy rows match no kind filter until re-annotated with one.
    let mut by_kind = q("");
    by_kind.filters = SearchFilters { kind: Some(AssetKind::Prop), ..Default::default() };
    assert_eq!(search.search(&by_kind, &ANYONE, None).unwrap().total, 0);
    let mut a = ann("Legacy Lantern");
    a.kind = Some(AssetKind::Prop);
    search.set_annotation(&asset_id_n(1), &a, NOW).unwrap();
    let page = search.search(&by_kind, &ANYONE, None).unwrap();
    assert_eq!((page.total, page.hits[0].kind), (1, Some(AssetKind::Prop)));

    drop(core);
    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
    // A second open of the migrated root is a clean no-op.
    AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
}

/// The v2 search DDL exactly as it shipped: kind column present, but no
/// canon_alias column, no alias postings, no search_state row.
const LEGACY_SEARCH_V2: &str = "
CREATE TABLE IF NOT EXISTS search_annotations(
    asset_id BLOB PRIMARY KEY,
    namespace TEXT NOT NULL,
    kind TEXT CHECK(kind IS NULL OR kind IN \
    ('mesh','character','weapon','vehicle','prop','texture','material',\
'audio','video','skybox','world','prefab')),
    visibility TEXT NOT NULL CHECK(visibility IN ('public','private')),
    owner BLOB,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    creator TEXT NOT NULL,
    generator TEXT NOT NULL,
    backend TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt TEXT NOT NULL,
    provenance TEXT NOT NULL,
    live INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS search_annotations_by_ns ON search_annotations(namespace);
CREATE INDEX IF NOT EXISTS search_annotations_by_kind ON search_annotations(kind);
CREATE TABLE IF NOT EXISTS search_labels(
    asset_id BLOB NOT NULL,
    kind TEXT NOT NULL CHECK(kind IN ('category','tag')),
    label TEXT NOT NULL,
    PRIMARY KEY(asset_id, kind, label)
);
CREATE INDEX IF NOT EXISTS search_labels_by_label ON search_labels(kind, label);
CREATE TABLE IF NOT EXISTS search_postings(
    term TEXT NOT NULL,
    asset_id BLOB NOT NULL,
    weight_public INTEGER NOT NULL,
    weight_owner INTEGER NOT NULL,
    PRIMARY KEY(term, asset_id)
);
CREATE INDEX IF NOT EXISTS search_postings_by_asset ON search_postings(asset_id);
";

#[test]
fn v2_root_gains_alias_index_with_data_backfilled() {
    // A byte-real v2 root: search tables in their v2 shape, one annotated
    // asset with an alias head already pointing at it (v2 kept `live`
    // maintained but knew nothing of canonical aliases or alias terms).
    let (root, db) = fixture_root(
        "search_v2",
        &format!(
            "{CATALOG_SCHEMA}{HISTORICAL_JOBS_SCHEMA}{AUTH_SCHEMA}{LEGACY_SEARCH_V2}\
             INSERT INTO assets(asset_id, namespace, created_ms) VALUES({ID1_HEX},'rik2',1);\
             INSERT INTO asset_aliases(alias, asset_id, head_revision, updated_ms)\
              VALUES('rik2/props/lamp',{ID1_HEX},X'{rev}',1);\
             INSERT INTO search_annotations(asset_id, namespace, kind, visibility, owner,\
                title, description, creator, generator, backend, model,\
                prompt, provenance, live, updated_ms)\
              VALUES({ID1_HEX},'rik2','prop','public',NULL,'Brass Lantern',\
                'a lantern annotated under schema v2','rik','trellis','cuda','xl',\
                '','',1,1);\
             INSERT INTO search_labels(asset_id, kind, label) VALUES({ID1_HEX},'tag','durable');\
             INSERT INTO search_postings(term, asset_id, weight_public, weight_owner)\
              VALUES('brass',{ID1_HEX},100,100),('lantern',{ID1_HEX},120,120);\
             PRAGMA user_version=2;",
            rev = "22".repeat(32),
        ),
    );
    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    let search = core.search();

    // The alias head became searchable terms and the canonical alias.
    let page = search.search(&q("lamp"), &ANYONE, None).unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.hits[0].title, "Brass Lantern");
    assert_eq!(page.hits[0].alias.as_deref(), Some("rik2/props/lamp"));
    assert!(page.hits[0].live);
    // Pre-migration annotation data is intact: postings, labels, kind, text.
    assert_eq!(search.search(&q("brass"), &ANYONE, None).unwrap().total, 1);
    let mut tagged = q("");
    tagged.filters = SearchFilters { tag: Some("durable"), ..Default::default() };
    assert_eq!(search.search(&tagged, &ANYONE, None).unwrap().total, 1);
    let mut by_kind = q("");
    by_kind.filters = SearchFilters { kind: Some(AssetKind::Prop), ..Default::default() };
    assert_eq!(search.search(&by_kind, &ANYONE, None).unwrap().total, 1);
    let read = search.annotation(&asset_id_n(1)).unwrap().unwrap();
    assert_eq!((read.title.as_str(), read.kind), ("Brass Lantern", Some(AssetKind::Prop)));

    // The index generation row exists and mutations advance it: cursors are
    // fully live on a migrated root.
    core.catalog().register_asset(&asset_id_n(2), "rik2", NOW).unwrap();
    search.set_annotation(&asset_id_n(2), &ann("Brass Kettle"), NOW).unwrap();
    let mut one = q("brass");
    one.page_size = 1;
    let cursor = search.search(&one, &ANYONE, None).unwrap().cursor.expect("two brass hits");
    let page2 = search.search(&one, &ANYONE, Some(&cursor)).unwrap();
    assert_eq!(page2.hits.len(), 1);

    drop(core);
    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
    assert!(read_generation(&db) >= 1, "state row seeded");
    // A second open of the migrated root is a clean no-op.
    AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
}

#[test]
fn v3_root_gains_import_and_variant_tables() {
    // A faithful v3 root: the current catalog/jobs/auth/search layout (v4
    // added nothing to those), no import/variant tables, user_version=3.
    let (root, db) = fixture_root(
        "import_v3",
        &format!(
            "{CATALOG_SCHEMA}{HISTORICAL_JOBS_SCHEMA}{AUTH_SCHEMA}{search}\
             INSERT INTO assets(asset_id, namespace, created_ms) VALUES({ID1_HEX},'rik2',1);\
             PRAGMA user_version=3;",
            search = makepad_asset_store::search::SEARCH_SCHEMA,
        ),
    );
    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    // The migrated root serves the new subsystems and kept its data.
    assert!(core.imports().sources().unwrap().is_empty());
    assert_eq!(
        core.catalog().asset_namespace(&asset_id_n(1)).unwrap().as_deref(),
        Some("rik2")
    );
    drop(core);
    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
    let tables = raw::exec(
        &db,
        "SELECT name FROM sqlite_master WHERE type='table' AND name IN \
         ('import_sources','imports','import_entries','recipes','derivations',\
          'derived_variants','variant_sets') ORDER BY name",
    );
    assert_eq!(tables.len(), 7, "v4 tables present: {tables:?}");
    // A second open of the migrated root is a clean no-op.
    AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
}

#[test]
fn v4_root_gains_operation_tables() {
    // A faithful v4 root: the current catalog/jobs/auth/search/import/variant
    // layout, no operation tables, user_version=4.
    let (root, db) = fixture_root(
        "operations_v4",
        &format!(
            "{CATALOG_SCHEMA}{HISTORICAL_JOBS_SCHEMA}{AUTH_SCHEMA}{search}{import}{variant}\
             INSERT INTO assets(asset_id, namespace, created_ms) VALUES({ID1_HEX},'rik2',1);\
             PRAGMA user_version=4;",
            search = makepad_asset_store::search::SEARCH_SCHEMA,
            import = makepad_asset_store::imports::IMPORT_SCHEMA,
            variant = makepad_asset_store::variants::VARIANT_SCHEMA,
        ),
    );
    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    // The queue subsystems are gone (aicore P7); the migrated root still
    // opens and its CATALOG data survived — the only promise that remains.
    assert_eq!(
        core.catalog().asset_namespace(&asset_id_n(1)).unwrap().as_deref(),
        Some("rik2")
    );
    drop(core);
    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
    let tables = raw::exec(
        &db,
        "SELECT name FROM sqlite_master WHERE type='table' AND name IN \
         ('operations','operation_events','operation_worker_seen') ORDER BY name",
    );
    assert!(tables.is_empty(), "retired operation tables present: {tables:?}");
    // A second open of the migrated root is a clean no-op.
    AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
}

#[test]
fn v7_root_gains_scale_indices_and_a_sharded_cas() {
    // A faithful v7 root: today's tables minus the three indices v8 adds,
    // plus a CAS object at the pre-v8 one-level path.
    let (root, db) = fixture_root(
        "scale_v7",
        &format!(
            "{CATALOG_SCHEMA}{HISTORICAL_JOBS_SCHEMA}{AUTH_SCHEMA}{search}{import}{variant}{operations}\
             DROP INDEX IF EXISTS asset_aliases_by_asset;\
             DROP INDEX IF EXISTS game_aliases_by_game;\
             DROP INDEX IF EXISTS search_annotations_by_canon;\
             INSERT INTO assets(asset_id, namespace, created_ms) VALUES({ID1_HEX},'rik2',1);\
             PRAGMA user_version=7;",
            search = makepad_asset_store::search::SEARCH_SCHEMA,
            import = makepad_asset_store::imports::IMPORT_SCHEMA,
            variant = makepad_asset_store::variants::VARIANT_SCHEMA,
            operations = HISTORICAL_OPERATIONS_SCHEMA,
        ),
    );
    let payload = b"blob written by a v7 server".to_vec();
    let blob_id = makepad_asset_data::BlobId::hash_of(&payload);
    let hex: String = blob_id.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
    let legacy_dir = root.join("cas/objects").join(&hex[..2]);
    std::fs::create_dir_all(&legacy_dir).unwrap();
    std::fs::write(legacy_dir.join(&hex), &payload).unwrap();

    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    assert_eq!(
        core.catalog().asset_namespace(&asset_id_n(1)).unwrap().as_deref(),
        Some("rik2"),
        "migration kept the catalog data"
    );
    // The object moved into its two-level hash path and still verifies.
    assert!(root
        .join("cas/objects")
        .join(&hex[..2])
        .join(&hex[2..4])
        .join(&hex)
        .is_file());
    assert!(!legacy_dir.join(&hex).is_file(), "one-level copy is gone");
    assert_eq!(core.cas().read_verified(&blob_id).unwrap(), payload);
    drop(core);

    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
    let indices = raw::exec(
        &db,
        "SELECT name FROM sqlite_master WHERE type='index' AND name IN \
         ('asset_aliases_by_asset','game_aliases_by_game','search_annotations_by_canon') \
         ORDER BY name",
    );
    assert_eq!(indices.len(), 3, "v8 indices present: {indices:?}");
    // A second open of the migrated root is a clean no-op.
    AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
}

/// The plans the v8 indices exist to produce. A schema that grows a new
/// ordering column or filter without an index would show up here as a
/// re-appearing SCAN or TEMP B-TREE.
#[test]
fn hot_paths_use_the_scale_indices() {
    let root = test_root("scale_plans");
    AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    let db = root.join("catalog.sqlite3");
    let plan = |sql: &str| raw::exec_last(&db, &format!("EXPLAIN QUERY PLAN {sql}")).join(" | ");

    // Browse page: index-ordered, no sort, no scan.
    let browse = plan(
        "SELECT a.asset_id FROM search_annotations a WHERE 1=1 \
         AND (a.visibility = 'public' OR (a.owner IS NOT NULL AND a.owner = NULL)) \
         ORDER BY a.canon_alias ASC, a.asset_id ASC LIMIT 61",
    );
    assert!(browse.contains("search_annotations_by_canon"), "{browse}");
    assert!(!browse.contains("TEMP B-TREE"), "browse page still sorts: {browse}");

    // Alias-head reads by asset: seek, not scan.
    for sql in [
        "SELECT alias FROM asset_aliases WHERE asset_id = X'01' ORDER BY alias",
        "SELECT MIN(alias) FROM asset_aliases WHERE asset_id = X'01'",
        "DELETE FROM asset_aliases WHERE asset_id = X'01' AND head_revision = X'02'",
    ] {
        let p = plan(sql);
        assert!(p.contains("asset_aliases_by_asset"), "{sql} => {p}");
    }
}

/// The v11 annotation table exactly before `data` widened its kind CHECK.
const LEGACY_SEARCH_V11_ANNOTATIONS: &str = "
CREATE TABLE search_annotations(
    asset_id BLOB PRIMARY KEY,
    namespace TEXT NOT NULL,
    kind TEXT CHECK(kind IS NULL OR kind IN
    ('mesh','character','weapon','vehicle','prop','texture','material',
     'audio','video','skybox','world','prefab','billboard','game','vjeffect')),
    visibility TEXT NOT NULL CHECK(visibility IN ('public','private')),
    owner BLOB,
    title TEXT NOT NULL,
    description TEXT NOT NULL,
    creator TEXT NOT NULL,
    generator TEXT NOT NULL,
    backend TEXT NOT NULL,
    model TEXT NOT NULL,
    prompt TEXT NOT NULL,
    provenance TEXT NOT NULL,
    live INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL,
    canon_alias TEXT NOT NULL DEFAULT ''
);
CREATE INDEX search_annotations_by_ns ON search_annotations(namespace);
CREATE INDEX search_annotations_by_kind ON search_annotations(kind);
CREATE INDEX search_annotations_by_canon ON search_annotations(canon_alias, asset_id);
";

#[test]
fn v11_root_migrates_kind_check_to_appended_kinds_with_rows_intact() {
    let (root, db) = fixture_root(
        "data_kind_v11",
        &format!(
            "{CATALOG_SCHEMA}{HISTORICAL_JOBS_SCHEMA}{AUTH_SCHEMA}{LEGACY_SEARCH_V11_ANNOTATIONS}\
             INSERT INTO assets(asset_id, namespace, created_ms) VALUES({ID1_HEX},'rik2',1);\
             INSERT INTO search_annotations(asset_id, namespace, kind, visibility, owner,\
                title, description, creator, generator, backend, model, prompt, provenance,\
                live, updated_ms, canon_alias)\
              VALUES({ID1_HEX},'rik2','vjeffect','public',NULL,'Legacy document','','','','',\
                '','','',0,1,'');\
             PRAGMA user_version=11;"
        ),
    );

    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    let before = core.search().annotation(&asset_id_n(1)).unwrap().unwrap();
    assert_eq!((before.title.as_str(), before.kind), ("Legacy document", Some(AssetKind::VjEffect)));

    let mut data = ann("Migrated data document");
    data.kind = Some(AssetKind::Data);
    core.search().set_annotation(&asset_id_n(1), &data, NOW).unwrap();
    let mut by_data = q("");
    by_data.filters = SearchFilters { kind: Some(AssetKind::Data), ..Default::default() };
    let page = core.search().search(&by_data, &ANYONE, None).unwrap();
    assert_eq!((page.total, page.hits[0].kind), (1, Some(AssetKind::Data)));

    let mut model_program = ann("Migrated editable CSG model");
    model_program.kind = Some(AssetKind::ModelProgram);
    core.search().set_annotation(&asset_id_n(1), &model_program, NOW + 1).unwrap();
    let mut by_model_program = q("");
    by_model_program.filters = SearchFilters {
        kind: Some(AssetKind::ModelProgram),
        ..Default::default()
    };
    let page = core.search().search(&by_model_program, &ANYONE, None).unwrap();
    assert_eq!(
        (page.total, page.hits[0].kind),
        (1, Some(AssetKind::ModelProgram))
    );
    drop(core);

    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
    let ddl = raw::exec(
        &db,
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='search_annotations'",
    );
    assert!(ddl[0].contains("'data'"), "migrated CHECK: {}", ddl[0]);
    assert!(ddl[0].contains("'model-program'"), "migrated CHECK: {}", ddl[0]);
    AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    assert_eq!(user_version(&db), SERVER_SCHEMA_VERSION.to_string());
}

#[test]
fn future_and_negative_schema_versions_refuse_to_open() {
    let (root, db) = fixture_root("future_version", "PRAGMA user_version=99;");
    match AssetServerCore::open(&root, Budgets::default_v1()).err() {
        Some(ServerError::UnsupportedSchema { found: 99 }) => {}
        other => panic!("expected UnsupportedSchema 99, got {other:?}"),
    }
    // The refused open left no schema behind.
    assert_eq!(raw::exec(&db, "SELECT name FROM sqlite_master WHERE type='table'"), Vec::<String>::new());

    let (root, _db) = fixture_root("negative_version", "PRAGMA user_version=-3;");
    match AssetServerCore::open(&root, Budgets::default_v1()).err() {
        Some(ServerError::UnsupportedSchema { found: -3 }) => {}
        other => panic!("expected UnsupportedSchema -3, got {other:?}"),
    }
}
