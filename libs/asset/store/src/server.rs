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
/// - v6: the search kind CHECK accepts `billboard`.
/// - v7: the search kind CHECK accepts `game` (playable splash sources).
/// - v8: scale retrofit — the reverse alias indices, the browse-order index,
///   and the CAS two-level hash path (`objects/ab/cd/<64-hex>`). Existing
///   objects move out of the one-level layout in this step.
/// - v9: deletion — asset/revision retirement (`assets.retired_ms`,
///   `candidates.retired_ms`), the per-asset revision index those walk,
///   the dedup-recency column blob GC's grace horizon reads
///   (`blobs.last_ref_ms`), and the incremental blob-GC tables. Every part
///   is an `ALTER TABLE ADD COLUMN` or a `CREATE ... IF NOT EXISTS`, so the
///   step costs one index build and no table rewrite however large the
///   store is.
/// - v10: reference blobs (`blob_refs`) — catalogued content whose bytes stay
///   at an external path the store reads but never owns, writes or deletes.
///   One `CREATE TABLE IF NOT EXISTS` and one index; no existing table is
///   touched and no row is rewritten, so the step is free on any store.
///
/// `open` migrates older versions forward one step at a time, each step in
/// its own transaction; a version newer than this build refuses to open.
pub const SERVER_SCHEMA_VERSION: i64 = 10;

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
fn migrate(db: &Db, cas: &Cas, budgets: &Budgets) -> ServerResult<()> {
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
                    db.exec("create canon index", crate::search::SEARCH_CANON_INDEX_SQL)?;
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
                // v7: kind CHECK accepts game. Both rebuild the table with
                // this build's CHECK, so a v5 root passes through v6 already
                // carrying the v7 list and the v7 step is a no-op rebuild.
                5 | 6 => {
                    if table_exists(db, "search_annotations")? {
                        db.exec(
                            "rebuild search_annotations kind check",
                            crate::search::KIND_CHECK_REBUILD_SQL,
                        )?;
                    }
                    db.exec("create search schema", crate::search::SEARCH_SCHEMA)?;
                }
                // v8: the scale retrofit. Both schema strings are made
                // entirely of IF NOT EXISTS statements, so re-running them
                // only adds the indices this version introduced; the CAS
                // move is idempotent for the same reason. Doing the move
                // inside the migration transaction is deliberate: the
                // writer lock is what stops two openers migrating at once,
                // and an interrupted move leaves objects readable at both
                // paths for the next attempt.
                7 => {
                    db.exec("create catalog schema", CATALOG_SCHEMA)?;
                    db.exec("create canon index", crate::search::SEARCH_CANON_INDEX_SQL)?;
                    cas.migrate_shards()?;
                }
                // v9: retirement + blob GC. The three columns are added with
                // ALTER TABLE (SQLite records them in the schema without
                // touching a single row), the rest is IF NOT EXISTS DDL, and
                // the one real cost is building the per-asset revision index
                // once.
                8 => {
                    for (table, column, ddl) in [
                        ("assets", "retired_ms", "ALTER TABLE assets ADD COLUMN retired_ms INTEGER"),
                        (
                            "candidates",
                            "retired_ms",
                            "ALTER TABLE candidates ADD COLUMN retired_ms INTEGER",
                        ),
                        ("blobs", "last_ref_ms", "ALTER TABLE blobs ADD COLUMN last_ref_ms INTEGER"),
                    ] {
                        if table_exists(db, table)? && !table_has_column(db, table, column)? {
                            db.exec("add retirement column", ddl)?;
                        }
                    }
                    db.exec("create catalog schema", CATALOG_SCHEMA)?;
                    db.exec("create gc schema", crate::gc::GC_SCHEMA)?;
                }
                // v10: reference blobs. One new table and one index, both
                // IF NOT EXISTS, and nothing existing is read or rewritten:
                // this step costs the same on an empty root and a ten-
                // million-object one.
                9 => {
                    db.exec("create blob ref schema", crate::blobrefs::BLOBREF_SCHEMA)?;
                }
                other => return Err(ServerError::UnsupportedSchema { found: other }),
            }
            db.exec("set user_version", &format!("PRAGMA user_version={}", version + 1))
        })?;
    }
}

/// What admitting a file by reference did. `deduped` means the catalog
/// already knew this digest; `owned` means the store holds the bytes in its
/// own CAS, so no reference was recorded and the external file is incidental.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobRefCommit {
    pub blob_id: BlobId,
    pub size: u64,
    pub deduped: bool,
    pub owned: bool,
    /// The absolute path recorded (or, when `owned`, the path that was read).
    pub path: std::path::PathBuf,
}

/// One bounded page of a reference re-scan.
#[derive(Clone, Debug, Default)]
pub struct RefRescanPage {
    pub entries: Vec<(crate::blobrefs::BlobRef, crate::blobrefs::RefState)>,
    /// Resume key: pass as `after` for the next page. `None` = finished.
    pub next: Option<BlobId>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RecoverReport {
    pub cas_temps_removed: u64,
    /// Blob delete intents left by a crash mid-sweep that this start
    /// resolved (unlinked, or kept because the bytes were re-uploaded).
    pub gc_deletes_resolved: u64,
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

        migrate(&db, &cas, &budgets)?;
        Ok(Self { db, cas, budgets })
    }

    /// Restart recovery: purge orphan CAS temp files, finish or abandon blob
    /// delete intents a crash left mid-sweep, and tear down expired worker
    /// leases (re-queueing or failing their jobs).
    pub fn recover(&self, now_ms: u64) -> ServerResult<RecoverReport> {
        let cas_temps_removed = self.cas.recover()?;
        let gc_deletes_resolved = self.gc().recover_pending(&self.cas)?;
        let leases_expired = self.jobs().expire_leases(now_ms)?;
        Ok(RecoverReport {
            cas_temps_removed,
            gc_deletes_resolved,
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

    pub fn blob_refs(&self) -> crate::blobrefs::BlobRefs<'_> {
        crate::blobrefs::BlobRefs { db: &self.db, budgets: &self.budgets }
    }

    pub fn gc(&self) -> crate::gc::Gc<'_> {
        crate::gc::Gc { db: &self.db, budgets: &self.budgets }
    }

    // ---- blob garbage collection -------------------------------------------

    /// Start a GC run (see [`crate::gc`]). Refuses while one is active.
    pub fn gc_begin(&self, cfg: crate::gc::GcConfig, now_ms: u64) -> ServerResult<crate::gc::GcStatus> {
        self.gc().begin(cfg, now_ms)
    }

    /// Advance the active run by at most `max_steps` bounded steps. Every
    /// step is one transaction over one batch, so this call's cost is
    /// chosen by the caller, not by the size of the store.
    pub fn gc_advance(&self, max_steps: u32, now_ms: u64) -> ServerResult<Option<crate::gc::GcStatus>> {
        self.gc().advance(&self.cas, max_steps, now_ms)
    }

    pub fn gc_status(&self) -> ServerResult<Option<crate::gc::GcStatus>> {
        self.gc().status()
    }

    pub fn gc_cancel(&self, now_ms: u64) -> ServerResult<bool> {
        self.gc().cancel(now_ms)
    }

    /// Convenience for tests and one-shot operators: begin a run and drive
    /// it to completion, up to `max_steps` steps.
    pub fn gc_run(
        &self,
        cfg: crate::gc::GcConfig,
        max_steps: u32,
        now_ms: u64,
    ) -> ServerResult<crate::gc::GcStatus> {
        self.gc_begin(cfg, now_ms)?;
        let status = self.gc_advance(max_steps, now_ms)?;
        status.ok_or(ServerError::NotFound { what: "gc run" })
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
    ///
    /// A blob the CAS does not hold may still be a REFERENCE (see
    /// [`crate::blobrefs`]): bytes the store catalogued where they already
    /// lay. Those are read from their external path and verified exactly as
    /// hard as a CAS object — same length check, same full-digest check
    /// before a single byte is returned. The CAS is tried first so a blob
    /// that exists both ways is served from the copy the store owns.
    pub fn read_blob(&self, blob_id: &BlobId) -> ServerResult<Vec<u8>> {
        if !self.catalog().has_blob(blob_id)? {
            return Err(ServerError::NotFound { what: "blob record" });
        }
        if self.cas.contains(blob_id) {
            return self.cas.read_verified(blob_id);
        }
        if let Some(entry) = self.blob_refs().lookup(blob_id)? {
            return crate::blobrefs::read_verified(&entry, &self.budgets);
        }
        // Recorded, not in the CAS, not referenced: the object is gone.
        // `read_verified` produces the established NotFound for that.
        self.cas.read_verified(blob_id)
    }

    /// Is this blob held as a reference rather than owned bytes? Callers that
    /// must plan a response length before reading (the batch pull) ask this
    /// so they can cheaply re-stat the external file first.
    pub fn blob_ref_of(&self, blob_id: &BlobId) -> ServerResult<Option<crate::blobrefs::BlobRef>> {
        if self.cas.contains(blob_id) {
            return Ok(None);
        }
        self.blob_refs().lookup(blob_id)
    }

    // ---- reference blobs (bytes the store catalogues but does not copy) ----

    /// Admit a file WHERE IT LIES: hash it in place, record the `blobs` row,
    /// then the `blob_refs` row. Nothing is copied and nothing is written to
    /// the file.
    ///
    /// Ordering mirrors the CAS admission law (hash first, catalog second):
    /// a crash between the two steps leaves a `blobs` row whose bytes cannot
    /// be found, which every read refuses loudly, and never a reference row
    /// for a blob the catalog does not know.
    ///
    /// Idempotent. Re-importing an unchanged file re-derives the same digest
    /// and reports `deduped`. If the store ALREADY OWNS these bytes in its
    /// CAS, no reference is recorded at all: owned bytes are strictly better
    /// than a promise about someone else's file.
    pub fn put_blob_ref(&self, path: &Path, now_ms: u64) -> ServerResult<BlobRefCommit> {
        let scan = crate::blobrefs::scan_file(path, &self.budgets)?;
        let already = self.catalog().has_blob(&scan.blob_id)?;
        self.catalog().record_blob(&scan.blob_id, scan.size, now_ms)?;
        let owned = self.cas.contains(&scan.blob_id);
        if !owned {
            self.blob_refs()
                .record(&scan.blob_id, &scan.path, scan.size, scan.mtime_ms, now_ms)?;
        }
        Ok(BlobRefCommit {
            blob_id: scan.blob_id,
            size: scan.size,
            deduped: already,
            owned,
            path: scan.path,
        })
    }

    /// What a reference looks like on disk right now, without producing
    /// bytes. `None` when the blob is not a reference at all.
    pub fn verify_blob_ref(
        &self,
        blob_id: &BlobId,
    ) -> ServerResult<Option<crate::blobrefs::RefState>> {
        Ok(self
            .blob_refs()
            .lookup(blob_id)?
            .map(|entry| crate::blobrefs::verify(&entry, &self.budgets)))
    }

    /// One bounded page of a whole-library re-scan: verify up to `limit`
    /// references in digest order and report each one's state. The caller
    /// chooses the cost per call and resumes with `next`.
    pub fn rescan_blob_refs(
        &self,
        after: Option<&BlobId>,
        limit: u32,
    ) -> ServerResult<RefRescanPage> {
        let entries = self.blob_refs().list(after, limit)?;
        let next = entries.last().map(|e| e.blob_id);
        let mut states = Vec::with_capacity(entries.len());
        for entry in entries {
            let state = crate::blobrefs::verify(&entry, &self.budgets);
            states.push((entry, state));
        }
        Ok(RefRescanPage { entries: states, next })
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
