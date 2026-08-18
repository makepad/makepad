//! Schema migration: fresh roots land on the current version, v1 roots (with
//! and without the ad-hoc search tables that predate v2) migrate forward with
//! their data intact, and versions this build does not speak refuse to open.
//!
//! Fixture databases are fabricated through the minimal raw-sqlite shim in
//! tests/common (the test-side twin of src/sqlite.rs) so the legacy layouts
//! are byte-real, not simulated through current-code hooks.

mod common;
use common::*;
use makepad_asset_store::{
    auth::AUTH_SCHEMA, catalog::CATALOG_SCHEMA, jobs::JOBS_SCHEMA, AssetAnnotation,
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
    SearchQuery { text, filters: SearchFilters::default(), page_size: 10 }
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
            "{CATALOG_SCHEMA}{JOBS_SCHEMA}{AUTH_SCHEMA}\
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
            "{CATALOG_SCHEMA}{JOBS_SCHEMA}{AUTH_SCHEMA}{LEGACY_SEARCH_V1}\
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
            "{CATALOG_SCHEMA}{JOBS_SCHEMA}{AUTH_SCHEMA}{LEGACY_SEARCH_V2}\
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
            "{CATALOG_SCHEMA}{JOBS_SCHEMA}{AUTH_SCHEMA}{search}\
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
            "{CATALOG_SCHEMA}{JOBS_SCHEMA}{AUTH_SCHEMA}{search}{import}{variant}\
             INSERT INTO assets(asset_id, namespace, created_ms) VALUES({ID1_HEX},'rik2',1);\
             PRAGMA user_version=4;",
            search = makepad_asset_store::search::SEARCH_SCHEMA,
            import = makepad_asset_store::imports::IMPORT_SCHEMA,
            variant = makepad_asset_store::variants::VARIANT_SCHEMA,
        ),
    );
    let core = AssetServerCore::open(&root, Budgets::default_v1()).unwrap();
    // The migrated root serves the operations subsystem and kept its data.
    assert!(!core.operations().capabilities(1).unwrap().is_empty());
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
    assert_eq!(tables.len(), 3, "v5 tables present: {tables:?}");
    // A second open of the migrated root is a clean no-op.
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
