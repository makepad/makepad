//! The assembled headless core: one root directory holding the CAS and the
//! WAL catalog, plus the recovery and seeding flows that span both.
//!
//! Root layout:
//!   <root>/cas/...            content-addressed objects and temp writes
//!   <root>/catalog.sqlite3    catalog, jobs, auth (WAL mode)
//!
//! Ordering law for blob admission: CAS commit FIRST, catalog record SECOND.
//! A crash between the two leaves an unrecorded object — harmless garbage
//! that a later identical upload dedups against — never a catalog row whose
//! bytes are missing.

use crate::auth::Auth;
use crate::budget::Budgets;
use crate::cas::{BlobCommit, BlobWriter, Cas};
use crate::catalog::{Catalog, CandidateState, CATALOG_SCHEMA};
use crate::error::{io_err, ServerError, ServerResult};
use crate::jobs::{Jobs, JOBS_SCHEMA};
use crate::seed::{stock_asset_id, SeedReport, StockSeedSource};
use crate::sqlite::Db;
use makepad_asset_data::{AssetRevisionRef, BlobId};
use std::path::Path;

/// The catalog schema version this build reads and writes, stored in
/// SQLite's `user_version`.
///
/// History:
/// - v1: catalog + jobs + auth tables.
/// - v2: search tables join the versioned schema, and `search_annotations`
///   gains the nullable `kind` column (pre-v2 roots that already carried
///   ad-hoc search tables are retrofitted in place).
/// - v3: alias-aware search — `search_annotations` gains the `canon_alias`
///   ordering column, alias heads index into `search_alias_postings`, and
///   the `search_state` row carries the index generation that keyset
///   cursors bind to. Existing alias/annotation data is backfilled.
/// - v4: deterministic external-pack import (approved source collections,
///   import records, entry maps) and derived variants (recipes, single-flight
///   derivation cache, immutable variant records, frozen variant sets).
/// - v5: typed asset operations — owner-scoped durable operation rows with
///   idempotency, per-operation event logs, and the worker-liveness table
///   behind truthful operation availability.
///
/// `open` migrates older versions forward one step at a time, each step in
/// its own transaction; a version newer than this build refuses to open.
pub const SERVER_SCHEMA_VERSION: i64 = 6;

pub struct AssetServerCore {
    db: Db,
    cas: Cas,
    budgets: Budgets,
}

fn user_version(db: &Db) -> ServerResult<i64> {
    let mut s = db.prepare("get user_version", "PRAGMA user_version")?;
    Ok(if s.step()? { s.column_i64(0) } else { 0 })
}

fn table_exists(db: &Db, table: &str) -> ServerResult<bool> {
    let mut s = db.prepare(
        "table exists",
        "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
    )?;
    s.bind_text(1, table)?;
    s.step()
}

/// PRAGMA table_info row layout: (cid, name, type, notnull, dflt_value, pk).
/// PRAGMA arguments cannot travel as binds; `table` only ever comes from the
/// migration code below, never from a caller.
fn table_has_column(db: &Db, table: &str, column: &str) -> ServerResult<bool> {
    let mut s = db.prepare("table info", &format!("PRAGMA table_info({table})"))?;
    while s.step()? {
        if s.column_text(1) == column {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Bring the catalog to `SERVER_SCHEMA_VERSION`, one version per IMMEDIATE
/// transaction. The version is re-read inside each transaction, so concurrent
/// openers serialize on the writer lock and every step applies exactly once;
/// a crash between steps leaves a valid older version that the next open
/// finishes migrating. Unknown versions (newer builds, corruption) refuse.
fn migrate(db: &Db, budgets: &Budgets) -> ServerResult<()> {
    loop {
        let version = user_version(db)?;
        if version == SERVER_SCHEMA_VERSION {
            return Ok(());
        }
        if !(0..SERVER_SCHEMA_VERSION).contains(&version) {
            return Err(ServerError::UnsupportedSchema { found: version });
        }
        db.tx(|db| {
            // Another process may have advanced the schema while we waited
            // for the write lock; no-op here and let the loop re-decide.
            let v = user_version(db)?;
            if v != version {
                return Ok(());
            }
            match v {
                // v1: the original catalog/jobs/auth tables.
                0 => {
                    db.exec("create catalog schema", CATALOG_SCHEMA)?;
                    db.exec("create jobs schema", JOBS_SCHEMA)?;
                    db.exec("create auth schema", crate::auth::AUTH_SCHEMA)?;
                }
                // v2: search tables join the versioned schema. Pre-v2 roots
                // may already hold ad-hoc search tables without the kind
                // column — retrofit the column first, then let the idempotent
                // CREATEs fill in whatever else is missing.
                1 => {
                    if table_exists(db, "search_annotations")?
                        && !table_has_column(db, "search_annotations", "kind")?
                    {
                        db.exec("add kind column", &crate::search::kind_migration_sql())?;
                    }
                    db.exec("create search schema", crate::search::SEARCH_SCHEMA)?;
                }
                // v3: alias-aware search. Retrofit the canonical-alias column
                // onto v2 annotation tables, let the idempotent CREATEs add
                // the alias-posting and state tables, then backfill from the
                // alias heads already in the catalog.
                2 => {
                    if table_exists(db, "search_annotations")?
                        && !table_has_column(db, "search_annotations", "canon_alias")?
                    {
                        db.exec(
                            "add canon_alias column",
                            &crate::search::canon_alias_migration_sql(),
                        )?;
                    }
                    db.exec("create search schema", crate::search::SEARCH_SCHEMA)?;
                    crate::search::backfill_alias_index(db, budgets)?;
                }
                // v4: import + derived-variant tables. Purely additive; no
                // retrofit or backfill exists because no earlier version
                // carried any of this state.
                3 => {
                    db.exec("create import schema", crate::imports::IMPORT_SCHEMA)?;
                    db.exec("create variant schema", crate::variants::VARIANT_SCHEMA)?;
                }
                // v5: typed asset operations. Purely additive.
                4 => {
                    db.exec(
                        "create operations schema",
                        crate::operations::OPERATIONS_SCHEMA,
                    )?;
                }
                // v6: kind CHECK accepts billboard (sprite/billboard content).
                5 => {
                    if table_exists(db, "search_annotations")? {
                        db.exec(
                            "rebuild search_annotations kind check",
                            crate::search::KIND_CHECK_REBUILD_SQL,
                        )?;
                    }
                    db.exec("create search schema", crate::search::SEARCH_SCHEMA)?;
                }
                other => return Err(ServerError::UnsupportedSchema { found: other }),
            }
            db.exec("set user_version", &format!("PRAGMA user_version={}", version + 1))
        })?;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecoverReport {
    pub cas_temps_removed: u64,
    pub leases_expired: u64,
}

impl AssetServerCore {
    /// Open (creating if absent) a server root. Idempotent; safe to call on
    /// every process start. Does NOT run recovery — call `recover` with an
    /// explicit timestamp once after opening.
    pub fn open(root: &Path, budgets: Budgets) -> ServerResult<Self> {
        // Refuse invalid budgets before anything touches disk: every later
        // integer-domain cast (c_int binds, i64 columns) relies on these.
        budgets.validate()?;
        std::fs::create_dir_all(root).map_err(io_err("server create root"))?;
        let cas = Cas::open(&root.join("cas"), &budgets)?;
        let db = Db::open(&root.join("catalog.sqlite3"), budgets.db_busy_timeout_ms)?;

        // WAL is required, not a preference: it is what makes a reader crash
        // or a mid-transaction power cut leave a consistent catalog.
        let mode = {
            let mut s = db.prepare("set wal", "PRAGMA journal_mode=WAL")?;
            if s.step()? {
                s.column_text(0)
            } else {
                String::new()
            }
        };
        if mode != "wal" {
            return Err(ServerError::InvalidState {
                what: "sqlite journal mode",
                state: "not wal",
            });
        }
        db.exec("set synchronous", "PRAGMA synchronous=FULL")?;
        db.exec("set foreign keys", "PRAGMA foreign_keys=ON")?;

        migrate(&db, &budgets)?;
        Ok(Self { db, cas, budgets })
    }

    /// Restart recovery: purge orphan CAS temp files and tear down expired
    /// worker leases (re-queueing or failing their jobs).
    pub fn recover(&self, now_ms: u64) -> ServerResult<RecoverReport> {
        let cas_temps_removed = self.cas.recover()?;
        let leases_expired = self.jobs().expire_leases(now_ms)?;
        Ok(RecoverReport {
            cas_temps_removed,
            leases_expired,
        })
    }

    pub fn budgets(&self) -> &Budgets {
        &self.budgets
    }

    pub fn catalog(&self) -> Catalog<'_> {
        Catalog { db: &self.db, budgets: &self.budgets }
    }

    pub fn jobs(&self) -> Jobs<'_> {
        Jobs { db: &self.db, budgets: &self.budgets }
    }

    pub fn auth(&self) -> Auth<'_> {
        Auth { db: &self.db }
    }

    pub fn search(&self) -> crate::search::Search<'_> {
        crate::search::Search { db: &self.db, budgets: &self.budgets }
    }

    pub fn imports(&self) -> crate::imports::Imports<'_> {
        crate::imports::Imports { db: &self.db, budgets: &self.budgets }
    }

    pub fn variants(&self) -> crate::variants::Variants<'_> {
        crate::variants::Variants { db: &self.db, budgets: &self.budgets }
    }

    pub fn operations(&self) -> crate::operations::Operations<'_> {
        crate::operations::Operations { db: &self.db, budgets: &self.budgets }
    }

    pub fn cas(&self) -> &Cas {
        &self.cas
    }

    // ---- blob admission (CAS + catalog in the required order) --------------

    pub fn begin_blob(&self) -> ServerResult<BlobWriter> {
        self.cas.begin()
    }

    /// Commit a streamed blob and record it. `expected` (when the uploader
    /// pre-declared a digest) is verified against the streamed bytes.
    pub fn commit_blob(
        &self,
        writer: BlobWriter,
        expected: Option<BlobId>,
        now_ms: u64,
    ) -> ServerResult<BlobCommit> {
        let commit = self.cas.commit(writer, expected)?;
        self.catalog().record_blob(&commit.blob_id, commit.size, now_ms)?;
        Ok(commit)
    }

    /// One-call admission for in-memory bytes.
    pub fn put_blob(&self, bytes: &[u8], now_ms: u64) -> ServerResult<BlobCommit> {
        let mut w = self.begin_blob()?;
        w.write(bytes)?;
        self.commit_blob(w, None, now_ms)
    }

    /// Read a blob the catalog knows about, verifying its digest. Unrecorded
    /// objects are invisible: catalog first, then CAS, both fail closed.
    pub fn read_blob(&self, blob_id: &BlobId) -> ServerResult<Vec<u8>> {
        if !self.catalog().has_blob(blob_id)? {
            return Err(ServerError::NotFound { what: "blob record" });
        }
        self.cas.read_verified(blob_id)
    }

    // ---- deterministic stock seeding ---------------------------------------

    /// Apply a stock seed: for every entry, admit its blobs, register the
    /// derived asset identity, stage + publish its revision, and point its
    /// alias at that revision. Deterministic and idempotent: a second run
    /// with the same source publishes nothing new.
    pub fn apply_stock_seed(&self, source: &dyn StockSeedSource, now_ms: u64) -> ServerResult<SeedReport> {
        let mut assets = source.assets();
        if assets.len() as u64 > self.budgets.max_seed_assets as u64 {
            return Err(ServerError::OverBudget {
                what: "seed assets",
                limit: self.budgets.max_seed_assets as u64,
                found: assets.len() as u64,
            });
        }
        // Apply order is part of determinism: sort by alias regardless of the
        // order the source produced. Two entries claiming one alias would make
        // the outcome order-dependent — refuse before any mutation.
        assets.sort_by(|a, b| a.alias.cmp(&b.alias));
        if assets.windows(2).any(|w| w[0].alias == w[1].alias) {
            return Err(ServerError::Conflict { what: "duplicate seed alias" });
        }
        let mut report = SeedReport::default();
        let catalog = self.catalog();
        for asset in &assets {
            report.assets_seen += 1;
            // The manifest must already carry the derived deterministic
            // identity; a seed that invents its own IDs is refused.
            let expect_id = stock_asset_id(&asset.alias);
            if asset.manifest.asset_id != expect_id {
                return Err(ServerError::Conflict { what: "seed asset_id not derived from alias" });
            }
            for blob in &asset.blobs {
                let commit = self.put_blob(blob, now_ms)?;
                if commit.deduped {
                    report.blobs_deduped += 1;
                } else {
                    report.blobs_written += 1;
                }
            }
            let manifest_bytes = asset.manifest.to_canonical_bytes()?;
            let revision = asset.manifest.revision()?;
            catalog.register_asset(&expect_id, asset.alias.namespace(), now_ms)?;
            match catalog.asset_candidate_state(&expect_id, &revision)? {
                Some(CandidateState::Published) => {
                    report.assets_already_published += 1;
                }
                Some(CandidateState::Quarantined) => {
                    // A quarantined stock revision stays quarantined; seeding
                    // must never resurrect pulled content.
                    return Err(ServerError::InvalidState {
                        what: "seed revision",
                        state: "quarantined",
                    });
                }
                Some(CandidateState::Staged) | None => {
                    let staged = catalog.stage_asset_revision(&manifest_bytes, now_ms)?;
                    catalog.publish_asset(&expect_id, &staged, now_ms)?;
                    report.assets_published_new += 1;
                }
            }
            catalog.set_asset_alias(
                &asset.alias,
                &AssetRevisionRef { asset_id: expect_id, revision },
                now_ms,
            )?;
        }
        Ok(report)
    }
}
