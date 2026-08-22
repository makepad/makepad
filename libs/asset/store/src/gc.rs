//! Incremental mark-and-sweep garbage collection of content-addressed blobs.
//!
//! A store that only ever grows is a store that eventually cannot be backed
//! up. Retiring content (see [`crate::catalog::Catalog::retire_asset`])
//! unlinks it from every listing, alias and search row, but the bytes it
//! named are still on disk: a blob is referenced by digest, from any number
//! of manifests, so nothing local to one asset can decide that its bytes are
//! now garbage. Only a whole-store reachability pass can.
//!
//! Design laws:
//!
//! - **The sweep is the truth.** There is no reference count to drift out of
//!   sync; every run recomputes the referenced set from the documents that
//!   name blobs (non-retired asset revisions, game revisions, derived
//!   variants). A refcount hint would be faster and would have to be right
//!   forever; a recomputed set is right by construction every time.
//! - **Nothing is unbounded.** A run is a sequence of bounded STEPS. Each
//!   step reads at most `mark_batch`/`sweep_batch` rows in ONE transaction,
//!   advances a durable cursor, and returns. The whole store is never
//!   materialised in memory and never held in one transaction, so publishes
//!   keep committing between steps at any store size.
//! - **Restartable.** Phase, cursors and counters live in `gc_runs`; a crash
//!   mid-run resumes at the last committed cursor. The referenced set lives
//!   in `gc_marks`, keyed by run.
//! - **Crash-safe deletion.** Removing a blob is: (1) one transaction that
//!   deletes the `blobs` row and records an intent row in
//!   `gc_pending_deletes`, (2) the filesystem unlink, (3) one transaction
//!   that clears the intent. A crash anywhere leaves either a live blob or a
//!   recorded intent, never a catalog row whose bytes are gone —
//!   [`Gc::recover_pending`] finishes (or abandons) the intents at startup.
//! - **Concurrent publishes are safe.** Two mechanisms, both conservative:
//!   a grace horizon (a blob whose row was created — or last re-referenced by
//!   a deduped upload — after `now - grace_ms` is never swept), and a PIN:
//!   while a run is active, every admission that references blobs writes
//!   those digests straight into the run's mark set inside its own
//!   transaction ([`pin_blobs_in_tx`]). A blob that becomes referenced during
//!   a run therefore survives it; a blob that becomes garbage during a run is
//!   simply collected by the next one. GC never races a publish into a
//!   dangling manifest.
//!
//! Phases, in order: `retain` (optional revision retention), `mark`,
//! `sweep`, then `done` (or `cancelled`).

use crate::budget::Budgets;
use crate::catalog::{fixed16, fixed32, Catalog};
use crate::cas::Cas;
use crate::error::{ServerError, ServerResult};
use crate::sqlite::Db;
use makepad_asset_data::{
    AssetId, AssetManifest, BlobId, DerivedVariantManifest, GameRevisionManifest,
};

pub const GC_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS gc_runs(
    run_id INTEGER PRIMARY KEY,
    started_ms INTEGER NOT NULL,
    updated_ms INTEGER NOT NULL,
    phase TEXT NOT NULL CHECK(phase IN ('retain','mark','sweep','done','cancelled')),
    dry_run INTEGER NOT NULL,
    horizon_ms INTEGER NOT NULL,
    retain_keep INTEGER,
    retain_cursor BLOB,
    retain_batch INTEGER NOT NULL,
    mark_batch INTEGER NOT NULL,
    sweep_batch INTEGER NOT NULL,
    mark_source INTEGER NOT NULL,
    mark_cursor BLOB,
    sweep_cursor BLOB,
    retired_revisions INTEGER NOT NULL,
    scanned_revisions INTEGER NOT NULL,
    marked_blobs INTEGER NOT NULL,
    examined_blobs INTEGER NOT NULL,
    unreferenced_blobs INTEGER NOT NULL,
    unreferenced_bytes INTEGER NOT NULL,
    deleted_blobs INTEGER NOT NULL,
    deleted_bytes INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS gc_runs_by_phase ON gc_runs(phase);
CREATE TABLE IF NOT EXISTS gc_marks(
    run_id INTEGER NOT NULL,
    blob_id BLOB NOT NULL,
    PRIMARY KEY(run_id, blob_id)
);
-- Revisions a DRY RUN's retention rule would have retired. Real runs retire
-- for real; a preview must still answer 'how many bytes would this free',
-- so the mark phase treats these exactly like retired revisions.
CREATE TABLE IF NOT EXISTS gc_retain_sim(
    run_id INTEGER NOT NULL,
    revision BLOB NOT NULL,
    PRIMARY KEY(run_id, revision)
);
CREATE TABLE IF NOT EXISTS gc_pending_deletes(
    blob_id BLOB PRIMARY KEY,
    size INTEGER NOT NULL,
    queued_ms INTEGER NOT NULL
);
";

/// Mark sources, walked in this order. Each is a table with a stable
/// ordering key the cursor advances along.
const SOURCE_ASSET_REVISIONS: i64 = 0;
const SOURCE_GAME_REVISIONS: i64 = 1;
const SOURCE_DERIVED_VARIANTS: i64 = 2;
const SOURCE_QUEUED_OPERATIONS: i64 = 3;
const SOURCE_COUNT: i64 = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GcPhase {
    Retain,
    Mark,
    Sweep,
    Done,
    Cancelled,
}

impl GcPhase {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Retain => "retain",
            Self::Mark => "mark",
            Self::Sweep => "sweep",
            Self::Done => "done",
            Self::Cancelled => "cancelled",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "retain" => Self::Retain,
            "mark" => Self::Mark,
            "sweep" => Self::Sweep,
            "done" => Self::Done,
            "cancelled" => Self::Cancelled,
            _ => return None,
        })
    }
    /// True while the run still has work; a run in a terminal phase is
    /// history, not a lock on the store.
    pub fn active(&self) -> bool {
        matches!(self, Self::Retain | Self::Mark | Self::Sweep)
    }
}

/// What one run is allowed to do and how much work a step may cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GcConfig {
    /// Count what would be freed; delete nothing (and retire nothing).
    pub dry_run: bool,
    /// A blob recorded — or last re-referenced by a deduped upload — within
    /// this window of the run's start is never swept, so an upload that has
    /// not yet been named by a manifest cannot be collected under its
    /// uploader's feet.
    pub grace_ms: u64,
    /// Rows one mark step reads (manifests are decoded, so this is the
    /// CPU-heavy batch).
    pub mark_batch: u32,
    /// Blob rows one sweep step examines.
    pub sweep_batch: u32,
    /// Retire every revision of an asset except the newest `n` and any
    /// revision an alias head still points at. `None` = keep everything.
    pub retain_keep: Option<u32>,
    /// Assets one retention step visits.
    pub retain_batch: u32,
}

impl GcConfig {
    pub fn default_v1() -> GcConfig {
        GcConfig {
            dry_run: false,
            grace_ms: 60 * 60 * 1000,
            // Batch sizes are chosen for LATENCY, not throughput: one step
            // holds the write lock for its whole transaction, so the batch
            // is sized so a step stays in the low tens of milliseconds on a
            // six-figure catalog (measured in tests/scale_retire_gc.rs).
            mark_batch: 64,
            sweep_batch: 128,
            retain_keep: None,
            retain_batch: 64,
        }
    }

    pub fn validate(&self) -> ServerResult<()> {
        if self.mark_batch == 0 || self.mark_batch > 100_000 {
            return Err(ServerError::InvalidInput { what: "gc mark batch" });
        }
        if self.sweep_batch == 0 || self.sweep_batch > 100_000 {
            return Err(ServerError::InvalidInput { what: "gc sweep batch" });
        }
        if self.retain_batch == 0 || self.retain_batch > 100_000 {
            return Err(ServerError::InvalidInput { what: "gc retain batch" });
        }
        if matches!(self.retain_keep, Some(0)) {
            // Keeping zero revisions would retire live heads; the retention
            // rule exists to trim SUPERSEDED revisions.
            return Err(ServerError::InvalidInput { what: "gc retain keep zero" });
        }
        Ok(())
    }
}

/// Durable progress of one run. Everything a caller needs to render a
/// progress bar and a result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GcStatus {
    pub run_id: u64,
    pub phase: GcPhase,
    pub dry_run: bool,
    pub started_ms: u64,
    pub updated_ms: u64,
    pub horizon_ms: u64,
    pub retain_keep: Option<u32>,
    /// Revisions the retention rule retired (or, in a dry run, would).
    pub retired_revisions: u64,
    /// Documents the mark phase has read.
    pub scanned_revisions: u64,
    /// Distinct blobs proven referenced so far.
    pub marked_blobs: u64,
    /// Blob rows the sweep has looked at.
    pub examined_blobs: u64,
    /// Blobs found unreferenced AND outside the grace horizon.
    pub unreferenced_blobs: u64,
    pub unreferenced_bytes: u64,
    /// Blobs actually removed (always 0 in a dry run).
    pub deleted_blobs: u64,
    pub deleted_bytes: u64,
}

impl GcStatus {
    pub fn finished(&self) -> bool {
        !self.phase.active()
    }
}

pub struct Gc<'a> {
    pub(crate) db: &'a Db,
    pub(crate) budgets: &'a Budgets,
}

// ---------------------------------------------------------------------------
// pins: admissions that reference blobs while a run is in flight
// ---------------------------------------------------------------------------

/// The active run id, if any. One indexed probe on a table that holds a
/// handful of rows; the common (no run) path costs one index seek.
pub(crate) fn active_run(db: &Db) -> ServerResult<Option<i64>> {
    let mut s = db.prepare(
        "gc active run",
        "SELECT run_id FROM gc_runs WHERE phase IN ('retain','mark','sweep')
         ORDER BY run_id DESC LIMIT 1",
    )?;
    if s.step()? {
        Ok(Some(s.column_i64(0)))
    } else {
        Ok(None)
    }
}

/// Pin blobs a mutation is about to reference into the active run's mark
/// set, inside that mutation's transaction. Called from every admission path
/// that makes a blob reachable (asset/game revision staging, derived-variant
/// completion). Without a run in flight this is one index seek and nothing
/// else.
pub(crate) fn pin_blobs_in_tx(db: &Db, blobs: &[BlobId]) -> ServerResult<()> {
    if blobs.is_empty() {
        return Ok(());
    }
    let Some(run) = active_run(db)? else {
        return Ok(());
    };
    for blob in blobs {
        let mut s = db.prepare(
            "gc pin blob",
            "INSERT OR IGNORE INTO gc_marks(run_id, blob_id) VALUES(?1, ?2)",
        )?;
        s.bind_i64(1, run)?;
        s.bind_blob(2, blob.as_bytes())?;
        s.run()?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// run lifecycle
// ---------------------------------------------------------------------------

impl<'a> Gc<'a> {
    fn catalog(&self) -> Catalog<'a> {
        Catalog { db: self.db, budgets: self.budgets }
    }

    /// Start a run. Refuses while another run is active: two sweeps would
    /// each hold half the truth about what is referenced.
    pub fn begin(&self, cfg: GcConfig, now_ms: u64) -> ServerResult<GcStatus> {
        cfg.validate()?;
        self.db.tx(|db| {
            if active_run(db)?.is_some() {
                return Err(ServerError::Conflict { what: "gc run already active" });
            }
            let horizon = now_ms.saturating_sub(cfg.grace_ms);
            let phase = if cfg.retain_keep.is_some() { GcPhase::Retain } else { GcPhase::Mark };
            let mut s = db.prepare(
                "gc begin",
                "INSERT INTO gc_runs(started_ms, updated_ms, phase, dry_run, horizon_ms,
                                     retain_keep, retain_cursor, retain_batch, mark_batch,
                                     sweep_batch, mark_source, mark_cursor,
                                     sweep_cursor, retired_revisions, scanned_revisions,
                                     marked_blobs, examined_blobs, unreferenced_blobs,
                                     unreferenced_bytes, deleted_blobs, deleted_bytes)
                 VALUES(?1, ?1, ?2, ?3, ?4, ?5, NULL, ?6, ?7, ?8, 0, NULL, NULL,
                        0, 0, 0, 0, 0, 0, 0, 0)",
            )?;
            s.bind_u64(1, now_ms)?;
            s.bind_text(2, phase.as_str())?;
            s.bind_i64(3, cfg.dry_run as i64)?;
            s.bind_u64(4, horizon)?;
            match cfg.retain_keep {
                Some(k) => s.bind_u64(5, k as u64)?,
                None => s.bind_null(5)?,
            }
            s.bind_u64(6, cfg.retain_batch as u64)?;
            s.bind_u64(7, cfg.mark_batch as u64)?;
            s.bind_u64(8, cfg.sweep_batch as u64)?;
            s.run()?;
            drop(s);
            // Old history is not state: keep the most recent runs only, so
            // gc_runs cannot grow without bound over years of collection.
            let mut s = db.prepare(
                "gc prune history",
                "DELETE FROM gc_runs WHERE run_id NOT IN
                 (SELECT run_id FROM gc_runs ORDER BY run_id DESC LIMIT 20)",
            )?;
            s.run()?;
            drop(s);
            let mut s = db.prepare(
                "gc prune marks",
                "DELETE FROM gc_marks WHERE run_id NOT IN (SELECT run_id FROM gc_runs)",
            )?;
            s.run()?;
            drop(s);
            let mut s = db.prepare(
                "gc prune sim",
                "DELETE FROM gc_retain_sim WHERE run_id NOT IN (SELECT run_id FROM gc_runs)",
            )?;
            s.run()?;
            drop(s);
            self.current_in(db)?
                .ok_or(ServerError::InvalidState { what: "gc run", state: "missing after begin" })
        })
    }

    /// The newest run's durable status (active or historical).
    pub fn status(&self) -> ServerResult<Option<GcStatus>> {
        self.db.read_tx(|db| self.current_in(db))
    }

    fn current_in(&self, db: &Db) -> ServerResult<Option<GcStatus>> {
        let mut s = db.prepare(
            "gc status",
            "SELECT run_id, phase, dry_run, started_ms, updated_ms, horizon_ms, retain_keep,
                    retired_revisions, scanned_revisions, marked_blobs, examined_blobs,
                    unreferenced_blobs, unreferenced_bytes, deleted_blobs, deleted_bytes
             FROM gc_runs ORDER BY run_id DESC LIMIT 1",
        )?;
        if !s.step()? {
            return Ok(None);
        }
        let phase = GcPhase::parse(&s.column_text(1))
            .ok_or(ServerError::InvalidState { what: "gc run row", state: "unknown phase" })?;
        Ok(Some(GcStatus {
            run_id: s.column_u64(0),
            phase,
            dry_run: s.column_i64(2) != 0,
            started_ms: s.column_u64(3),
            updated_ms: s.column_u64(4),
            horizon_ms: s.column_u64(5),
            retain_keep: if s.column_is_null(6) { None } else { Some(s.column_u64(6) as u32) },
            retired_revisions: s.column_u64(7),
            scanned_revisions: s.column_u64(8),
            marked_blobs: s.column_u64(9),
            examined_blobs: s.column_u64(10),
            unreferenced_blobs: s.column_u64(11),
            unreferenced_bytes: s.column_u64(12),
            deleted_blobs: s.column_u64(13),
            deleted_bytes: s.column_u64(14),
        }))
    }

    /// Abandon the active run. Idempotent; returns whether one was stopped.
    /// Anything already deleted stays deleted (deletion is only ever of
    /// proven-unreferenced bytes), and the mark set is dropped.
    pub fn cancel(&self, now_ms: u64) -> ServerResult<bool> {
        self.db.tx(|db| {
            let Some(run) = active_run(db)? else {
                return Ok(false);
            };
            let mut s = db.prepare(
                "gc cancel",
                "UPDATE gc_runs SET phase='cancelled', updated_ms=?2 WHERE run_id=?1",
            )?;
            s.bind_i64(1, run)?;
            s.bind_u64(2, now_ms)?;
            s.run()?;
            drop(s);
            clear_run_scratch(db, run)?;
            Ok(true)
        })
    }

    /// Finish (or abandon) delete intents left by a crash. A blob whose row
    /// is back in the catalog was re-uploaded and deduped against the object
    /// still on disk: keep those bytes and drop the intent. Returns how many
    /// intents were resolved.
    pub fn recover_pending(&self, cas: &Cas) -> ServerResult<u64> {
        let mut resolved = 0u64;
        loop {
            let batch = self.db.read_tx(|db| {
                let mut s = db.prepare(
                    "gc pending list",
                    "SELECT blob_id FROM gc_pending_deletes ORDER BY blob_id LIMIT 256",
                )?;
                let mut out: Vec<BlobId> = Vec::new();
                while s.step()? {
                    out.push(BlobId::from_bytes(fixed32(&s.column_blob(0), "gc pending row")?));
                }
                Ok(out)
            })?;
            if batch.is_empty() {
                return Ok(resolved);
            }
            for blob in &batch {
                let live = self.catalog().has_blob(blob)?;
                if !live {
                    cas.remove_object(blob)?;
                }
                self.db.tx(|db| {
                    let mut s = db.prepare(
                        "gc pending clear",
                        "DELETE FROM gc_pending_deletes WHERE blob_id=?1",
                    )?;
                    s.bind_blob(1, blob.as_bytes())?;
                    s.run()
                })?;
                resolved += 1;
            }
        }
    }

    // ---- stepping ----------------------------------------------------------

    /// Run at most `max_steps` bounded steps of the active run. Returns the
    /// status after the last step (or the latest historical status when no
    /// run is active). Every step is one transaction over at most a batch of
    /// rows; callers (a route, the janitor, a test) choose how much wall
    /// clock to spend by choosing `max_steps`.
    pub fn advance(&self, cas: &Cas, max_steps: u32, now_ms: u64) -> ServerResult<Option<GcStatus>> {
        let mut status = self.status()?;
        for _ in 0..max_steps {
            let Some(cur) = status else { return Ok(None) };
            if !cur.phase.active() {
                return Ok(Some(cur));
            }
            status = Some(self.step(cas, now_ms)?);
        }
        Ok(status)
    }

    /// One bounded unit of work.
    pub fn step(&self, cas: &Cas, now_ms: u64) -> ServerResult<GcStatus> {
        let cfg_row = self.db.read_tx(|db| self.run_row(db))?;
        let Some(row) = cfg_row else {
            return Err(ServerError::NotFound { what: "gc run" });
        };
        match row.phase {
            GcPhase::Retain => self.retain_step(&row, now_ms)?,
            GcPhase::Mark => self.mark_step(&row, now_ms)?,
            GcPhase::Sweep => self.sweep_step(&row, cas, now_ms)?,
            GcPhase::Done | GcPhase::Cancelled => {}
        }
        self.status()?
            .ok_or(ServerError::InvalidState { what: "gc run", state: "vanished" })
    }

    fn run_row(&self, db: &Db) -> ServerResult<Option<RunRow>> {
        let mut s = db.prepare(
            "gc run row",
            "SELECT run_id, phase, dry_run, horizon_ms, retain_keep, retain_cursor,
                    mark_source, mark_cursor, sweep_cursor,
                    retain_batch, mark_batch, sweep_batch
             FROM gc_runs ORDER BY run_id DESC LIMIT 1",
        )?;
        if !s.step()? {
            return Ok(None);
        }
        let phase = GcPhase::parse(&s.column_text(1))
            .ok_or(ServerError::InvalidState { what: "gc run row", state: "unknown phase" })?;
        Ok(Some(RunRow {
            run_id: s.column_i64(0),
            phase,
            dry_run: s.column_i64(2) != 0,
            horizon_ms: s.column_u64(3),
            retain_keep: if s.column_is_null(4) { None } else { Some(s.column_u64(4) as u32) },
            retain_cursor: if s.column_is_null(5) { None } else { Some(s.column_blob(5)) },
            mark_source: s.column_i64(6),
            mark_cursor: if s.column_is_null(7) { None } else { Some(s.column_blob(7)) },
            sweep_cursor: if s.column_is_null(8) { None } else { Some(s.column_blob(8)) },
            retain_batch: clamp_batch(s.column_u64(9)),
            mark_batch: clamp_batch(s.column_u64(10)),
            sweep_batch: clamp_batch(s.column_u64(11)),
        }))
    }

    // ---- phase 1: retention ------------------------------------------------

    /// Visit a page of assets and apply the retention rule to each. Bounded
    /// by `retain_batch` assets per step; the cursor is the asset id.
    fn retain_step(&self, row: &RunRow, now_ms: u64) -> ServerResult<()> {
        let keep = row.retain_keep.unwrap_or(1);
        let batch = row.retain_batch;
        self.db.tx(|db| {
            let assets = page_assets(db, row.retain_cursor.as_deref(), batch)?;
            let mut retired = 0u64;
            let catalog = self.catalog();
            for asset in &assets {
                let id = AssetId::from_bytes(fixed16(asset, "gc asset page row")?);
                let doomed = catalog.superseded_revisions_in_tx(db, &id, keep)?;
                for rev in &doomed {
                    if row.dry_run {
                        let mut s = db.prepare(
                            "gc retain sim",
                            "INSERT OR IGNORE INTO gc_retain_sim(run_id, revision) VALUES(?1, ?2)",
                        )?;
                        s.bind_i64(1, row.run_id)?;
                        s.bind_blob(2, rev.as_bytes())?;
                        s.run()?;
                        retired += 1;
                    } else if catalog.retire_revision_in_tx(db, &id, rev, now_ms)? {
                        retired += 1;
                    }
                }
            }
            let last = assets.last().cloned();
            let done = (assets.len() as u32) < batch;
            let mut s = db.prepare(
                "gc retain advance",
                "UPDATE gc_runs SET retain_cursor=?2, phase=?3, updated_ms=?4,
                        retired_revisions=retired_revisions+?5
                 WHERE run_id=?1",
            )?;
            s.bind_i64(1, row.run_id)?;
            match &last {
                Some(a) => s.bind_blob(2, a)?,
                None => s.bind_null(2)?,
            }
            s.bind_text(3, if done { GcPhase::Mark.as_str() } else { GcPhase::Retain.as_str() })?;
            s.bind_u64(4, now_ms)?;
            s.bind_u64(5, retired)?;
            s.run()
        })
    }

    // ---- phase 2: mark -----------------------------------------------------

    fn mark_step(&self, row: &RunRow, now_ms: u64) -> ServerResult<()> {
        let batch = row.mark_batch;
        self.db.tx(|db| {
            let (scanned, marked, last_key, exhausted) = match row.mark_source {
                SOURCE_ASSET_REVISIONS => self.mark_asset_revisions(db, row, batch)?,
                SOURCE_GAME_REVISIONS => self.mark_game_revisions(db, row, batch)?,
                SOURCE_DERIVED_VARIANTS => self.mark_derived_variants(db, row, batch)?,
                SOURCE_QUEUED_OPERATIONS => self.mark_queued_operations(db, row, batch)?,
                _ => (0, 0, None, true),
            };
            let (next_source, next_cursor, phase) = if exhausted {
                let next = row.mark_source + 1;
                if next >= SOURCE_COUNT {
                    (next, None, GcPhase::Sweep)
                } else {
                    (next, None, GcPhase::Mark)
                }
            } else {
                (row.mark_source, last_key, GcPhase::Mark)
            };
            let mut s = db.prepare(
                "gc mark advance",
                "UPDATE gc_runs SET mark_source=?2, mark_cursor=?3, phase=?4, updated_ms=?5,
                        scanned_revisions=scanned_revisions+?6, marked_blobs=marked_blobs+?7
                 WHERE run_id=?1",
            )?;
            s.bind_i64(1, row.run_id)?;
            s.bind_i64(2, next_source)?;
            match &next_cursor {
                Some(c) => s.bind_blob(3, c)?,
                None => s.bind_null(3)?,
            }
            s.bind_text(4, phase.as_str())?;
            s.bind_u64(5, now_ms)?;
            s.bind_u64(6, scanned)?;
            s.bind_u64(7, marked)?;
            s.run()
        })
    }

    /// Every blob named by a NON-RETIRED asset revision. A revision with no
    /// candidate row at all (impossible through the public API) counts as
    /// live: fail closed towards keeping bytes.
    fn mark_asset_revisions(
        &self,
        db: &Db,
        row: &RunRow,
        batch: u32,
    ) -> ServerResult<(u64, u64, Option<Vec<u8>>, bool)> {
        let mut s = db.prepare(
            "gc mark asset revisions",
            "SELECT r.revision, r.manifest, c.retired_ms,
                    EXISTS(SELECT 1 FROM gc_retain_sim g
                           WHERE g.run_id = ?3 AND g.revision = r.revision)
             FROM asset_revisions r
             LEFT JOIN candidates c
               ON c.kind='asset' AND c.owner_id = r.asset_id AND c.revision = r.revision
             WHERE r.revision > ?1
             ORDER BY r.revision LIMIT ?2",
        )?;
        s.bind_blob(1, row.mark_cursor.as_deref().unwrap_or(&[]))?;
        s.bind_u64(2, batch as u64)?;
        s.bind_i64(3, row.run_id)?;
        let mut rows: Vec<(Vec<u8>, Vec<u8>, bool)> = Vec::new();
        while s.step()? {
            let retired = !s.column_is_null(2) || s.column_i64(3) != 0;
            rows.push((s.column_blob(0), s.column_blob(1), retired));
        }
        drop(s);
        let exhausted = (rows.len() as u32) < batch;
        let last = rows.last().map(|r| r.0.clone());
        let mut marked = 0u64;
        let scanned = rows.len() as u64;
        for (_, manifest_bytes, retired) in &rows {
            if *retired {
                continue;
            }
            let manifest = AssetManifest::from_canonical_bytes(manifest_bytes)?;
            for f in &manifest.files {
                marked += mark_blob(db, row.run_id, &f.blob)?;
            }
            if let Some(t) = &manifest.thumbnail {
                marked += mark_blob(db, row.run_id, &t.blob)?;
            }
        }
        Ok((scanned, marked, last, exhausted))
    }

    /// Game revisions pin their own bytes (splash, manifest.toml, lock,
    /// thumbnail, optional catalog snapshot) plus, through the lock, asset
    /// revisions that are marked by their own source.
    fn mark_game_revisions(
        &self,
        db: &Db,
        row: &RunRow,
        batch: u32,
    ) -> ServerResult<(u64, u64, Option<Vec<u8>>, bool)> {
        let mut s = db.prepare(
            "gc mark game revisions",
            "SELECT revision, manifest FROM game_revisions
             WHERE revision > ?1 ORDER BY revision LIMIT ?2",
        )?;
        s.bind_blob(1, row.mark_cursor.as_deref().unwrap_or(&[]))?;
        s.bind_u64(2, batch as u64)?;
        let mut rows: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
        while s.step()? {
            rows.push((s.column_blob(0), s.column_blob(1)));
        }
        drop(s);
        let exhausted = (rows.len() as u32) < batch;
        let last = rows.last().map(|r| r.0.clone());
        let mut marked = 0u64;
        let scanned = rows.len() as u64;
        for (_, bytes) in &rows {
            let m = GameRevisionManifest::from_canonical_bytes(bytes)?;
            for blob in [&m.splash_blob, &m.manifest_blob, &m.lock_blob, &m.thumbnail.blob] {
                marked += mark_blob(db, row.run_id, blob)?;
            }
            if let Some(snapshot) = &m.catalog_snapshot {
                marked += mark_blob(db, row.run_id, snapshot)?;
            }
        }
        Ok((scanned, marked, last, exhausted))
    }

    /// Derived variants of a live base. A variant of a retired base is
    /// unreachable (its base manifest can no longer be read), so its output
    /// bytes are garbage exactly like the base's own.
    fn mark_derived_variants(
        &self,
        db: &Db,
        row: &RunRow,
        batch: u32,
    ) -> ServerResult<(u64, u64, Option<Vec<u8>>, bool)> {
        let mut s = db.prepare(
            "gc mark derived variants",
            "SELECT v.variant, v.manifest, c.retired_ms,
                    EXISTS(SELECT 1 FROM gc_retain_sim g
                           WHERE g.run_id = ?3 AND g.revision = v.base_revision)
             FROM derived_variants v
             LEFT JOIN candidates c
               ON c.kind='asset' AND c.owner_id = v.base_asset AND c.revision = v.base_revision
             WHERE v.variant > ?1
             ORDER BY v.variant LIMIT ?2",
        )?;
        s.bind_blob(1, row.mark_cursor.as_deref().unwrap_or(&[]))?;
        s.bind_u64(2, batch as u64)?;
        s.bind_i64(3, row.run_id)?;
        let mut rows: Vec<(Vec<u8>, Vec<u8>, bool)> = Vec::new();
        while s.step()? {
            let retired = !s.column_is_null(2) || s.column_i64(3) != 0;
            rows.push((s.column_blob(0), s.column_blob(1), retired));
        }
        drop(s);
        let exhausted = (rows.len() as u32) < batch;
        let last = rows.last().map(|r| r.0.clone());
        let mut marked = 0u64;
        let scanned = rows.len() as u64;
        for (_, bytes, retired) in &rows {
            if *retired {
                continue;
            }
            let m = DerivedVariantManifest::from_canonical_bytes(bytes)?;
            for input in &m.inputs {
                marked += mark_blob(db, row.run_id, &input.blob)?;
            }
            for f in &m.outputs {
                marked += mark_blob(db, row.run_id, &f.blob)?;
            }
            if let Some(t) = &m.thumbnail {
                marked += mark_blob(db, row.run_id, &t.blob)?;
            }
        }
        Ok((scanned, marked, last, exhausted))
    }

    /// The exact input bytes a QUEUED operation was armed against. The
    /// operation may outlive the retirement of its base asset; its worker
    /// must still find the bytes the store handed it.
    fn mark_queued_operations(
        &self,
        db: &Db,
        row: &RunRow,
        batch: u32,
    ) -> ServerResult<(u64, u64, Option<Vec<u8>>, bool)> {
        // The state test is applied in Rust, not in the WHERE: a filtered
        // page could return fewer rows than the batch while more rows exist
        // beyond the cursor, which would end the source early. Every mark
        // source pages on its ordering key alone for exactly that reason.
        let mut s = db.prepare(
            "gc mark queued operations",
            "SELECT operation_id, spec, state FROM operations
             WHERE operation_id > ?1
             ORDER BY operation_id LIMIT ?2",
        )?;
        s.bind_blob(1, row.mark_cursor.as_deref().unwrap_or(&[]))?;
        s.bind_u64(2, batch as u64)?;
        let mut rows: Vec<(Vec<u8>, Vec<u8>, String)> = Vec::new();
        while s.step()? {
            rows.push((s.column_blob(0), s.column_blob(1), s.column_text(2)));
        }
        drop(s);
        let exhausted = (rows.len() as u32) < batch;
        let last = rows.last().map(|r| r.0.clone());
        let mut marked = 0u64;
        let scanned = rows.len() as u64;
        for (_, spec, state) in &rows {
            if state != "queued" {
                continue;
            }
            for blob in crate::operations::spec_input_blobs(spec)? {
                marked += mark_blob(db, row.run_id, &blob)?;
            }
        }
        Ok((scanned, marked, last, exhausted))
    }

    // ---- phase 3: sweep ----------------------------------------------------

    fn sweep_step(&self, row: &RunRow, cas: &Cas, now_ms: u64) -> ServerResult<()> {
        let batch = row.sweep_batch;
        // One transaction decides the batch AND records the delete intents;
        // the unlinks happen after it commits, and a third transaction
        // clears the intents. A crash between them is repaired by
        // `recover_pending`.
        let doomed = self.db.tx(|db| {
            let mut s = db.prepare(
                "gc sweep page",
                "SELECT blob_id, size, created_ms, last_ref_ms FROM blobs
                 WHERE blob_id > ?1 ORDER BY blob_id LIMIT ?2",
            )?;
            s.bind_blob(1, row.sweep_cursor.as_deref().unwrap_or(&[]))?;
            s.bind_u64(2, batch as u64)?;
            let mut rows: Vec<(Vec<u8>, u64, u64, Option<u64>)> = Vec::new();
            while s.step()? {
                let last_ref = if s.column_is_null(3) { None } else { Some(s.column_u64(3)) };
                rows.push((s.column_blob(0), s.column_u64(1), s.column_u64(2), last_ref));
            }
            drop(s);
            let exhausted = (rows.len() as u32) < batch;
            let last = rows.last().map(|r| r.0.clone());
            let examined = rows.len() as u64;
            let mut unreferenced = 0u64;
            let mut unreferenced_bytes = 0u64;
            let mut deleted = 0u64;
            let mut deleted_bytes = 0u64;
            let mut doomed: Vec<BlobId> = Vec::new();
            for (blob_bytes, size, created_ms, last_ref) in &rows {
                // Grace window: a blob recorded (or re-referenced by a
                // deduped upload) after the horizon belongs to a publish
                // that may not have staged its manifest yet.
                if *created_ms > row.horizon_ms {
                    continue;
                }
                if matches!(last_ref, Some(t) if *t > row.horizon_ms) {
                    continue;
                }
                if is_marked(db, row.run_id, blob_bytes)? {
                    continue;
                }
                unreferenced += 1;
                unreferenced_bytes += *size;
                if row.dry_run {
                    continue;
                }
                let blob = BlobId::from_bytes(fixed32(blob_bytes, "gc sweep row")?);
                // A REFERENCE blob's bytes are not the store's to collect:
                // they are a file the operator owns, at a path the operator
                // chose. Forgetting it is the whole of the reclamation —
                // both catalog rows go, no delete intent is queued, and the
                // id never reaches `doomed`, so the unlink loop below cannot
                // see it even by accident. (Belt and braces: `Cas` knows
                // nothing about external paths, so `remove_object` could
                // only ever unlink inside the CAS anyway.)
                if is_reference(db, blob_bytes)? {
                    let mut s = db.prepare(
                        "gc delete blob ref row",
                        "DELETE FROM blob_refs WHERE blob_id=?1",
                    )?;
                    s.bind_blob(1, blob_bytes)?;
                    s.run()?;
                    drop(s);
                    let mut s =
                        db.prepare("gc delete blob row", "DELETE FROM blobs WHERE blob_id=?1")?;
                    s.bind_blob(1, blob_bytes)?;
                    s.run()?;
                    drop(s);
                    // Counted as a deleted record, but NOT as reclaimed
                    // bytes: nothing was freed on this machine.
                    deleted += 1;
                    continue;
                }
                // Intent first, row second, unlink third: the object can
                // never be missing while the catalog still admits it.
                let mut s = db.prepare(
                    "gc queue delete",
                    "INSERT OR REPLACE INTO gc_pending_deletes(blob_id, size, queued_ms)
                     VALUES(?1, ?2, ?3)",
                )?;
                s.bind_blob(1, blob_bytes)?;
                s.bind_u64(2, *size)?;
                s.bind_u64(3, now_ms)?;
                s.run()?;
                drop(s);
                let mut s = db.prepare("gc delete blob row", "DELETE FROM blobs WHERE blob_id=?1")?;
                s.bind_blob(1, blob_bytes)?;
                s.run()?;
                drop(s);
                doomed.push(blob);
                deleted += 1;
                deleted_bytes += *size;
            }
            let phase = if exhausted { GcPhase::Done } else { GcPhase::Sweep };
            let mut s = db.prepare(
                "gc sweep advance",
                "UPDATE gc_runs SET sweep_cursor=?2, phase=?3, updated_ms=?4,
                        examined_blobs=examined_blobs+?5,
                        unreferenced_blobs=unreferenced_blobs+?6,
                        unreferenced_bytes=unreferenced_bytes+?7,
                        deleted_blobs=deleted_blobs+?8,
                        deleted_bytes=deleted_bytes+?9
                 WHERE run_id=?1",
            )?;
            s.bind_i64(1, row.run_id)?;
            match &last {
                Some(c) => s.bind_blob(2, c)?,
                None => s.bind_null(2)?,
            }
            s.bind_text(3, phase.as_str())?;
            s.bind_u64(4, now_ms)?;
            s.bind_u64(5, examined)?;
            s.bind_u64(6, unreferenced)?;
            s.bind_u64(7, unreferenced_bytes)?;
            s.bind_u64(8, deleted)?;
            s.bind_u64(9, deleted_bytes)?;
            s.run()?;
            drop(s);
            if exhausted {
                clear_run_scratch(db, row.run_id)?;
            }
            Ok(doomed)
        })?;
        for blob in &doomed {
            cas.remove_object(blob)?;
        }
        if !doomed.is_empty() {
            self.db.tx(|db| {
                for blob in &doomed {
                    let mut s = db.prepare(
                        "gc clear intent",
                        "DELETE FROM gc_pending_deletes WHERE blob_id=?1",
                    )?;
                    s.bind_blob(1, blob.as_bytes())?;
                    s.run()?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

struct RunRow {
    run_id: i64,
    phase: GcPhase,
    dry_run: bool,
    horizon_ms: u64,
    retain_keep: Option<u32>,
    retain_cursor: Option<Vec<u8>>,
    mark_source: i64,
    mark_cursor: Option<Vec<u8>>,
    sweep_cursor: Option<Vec<u8>>,
    retain_batch: u32,
    mark_batch: u32,
    sweep_batch: u32,
}

/// A stored batch size is operational, not a contract: clamp a corrupt or
/// absurd value into the safe range rather than letting one step try to read
/// the whole store.
fn clamp_batch(stored: u64) -> u32 {
    stored.clamp(1, 100_000) as u32
}

/// Mark one blob referenced; returns 1 when this run had not seen it yet.
fn mark_blob(db: &Db, run_id: i64, blob: &BlobId) -> ServerResult<u64> {
    let mut s = db.prepare(
        "gc mark blob",
        "INSERT OR IGNORE INTO gc_marks(run_id, blob_id) VALUES(?1, ?2)",
    )?;
    s.bind_i64(1, run_id)?;
    s.bind_blob(2, blob.as_bytes())?;
    s.run()?;
    drop(s);
    Ok(db.changes())
}

fn is_marked(db: &Db, run_id: i64, blob: &[u8]) -> ServerResult<bool> {
    let mut s = db.prepare(
        "gc is marked",
        "SELECT 1 FROM gc_marks WHERE run_id=?1 AND blob_id=?2",
    )?;
    s.bind_i64(1, run_id)?;
    s.bind_blob(2, blob)?;
    s.step()
}

/// Is this blob one the store REFERENCES rather than owns? The sweep asks
/// once per doomed blob so a reference is forgotten without ever entering the
/// unlink path (see [`crate::blobrefs`]).
fn is_reference(db: &Db, blob: &[u8]) -> ServerResult<bool> {
    let mut s = db.prepare(
        "gc is reference",
        "SELECT 1 FROM blob_refs WHERE blob_id=?1",
    )?;
    s.bind_blob(1, blob)?;
    s.step()
}

fn clear_run_scratch(db: &Db, run_id: i64) -> ServerResult<()> {
    for (op, sql) in [
        ("gc drop marks", "DELETE FROM gc_marks WHERE run_id=?1"),
        ("gc drop sim", "DELETE FROM gc_retain_sim WHERE run_id=?1"),
    ] {
        let mut s = db.prepare(op, sql)?;
        s.bind_i64(1, run_id)?;
        s.run()?;
    }
    Ok(())
}

/// One keyset page of asset ids, ordered by id bytes.
fn page_assets(db: &Db, after: Option<&[u8]>, limit: u32) -> ServerResult<Vec<Vec<u8>>> {
    let mut s = db.prepare(
        "gc asset page",
        "SELECT asset_id FROM assets WHERE asset_id > ?1 ORDER BY asset_id LIMIT ?2",
    )?;
    s.bind_blob(1, after.unwrap_or(&[]))?;
    s.bind_u64(2, limit as u64)?;
    let mut out = Vec::new();
    while s.step()? {
        out.push(s.column_blob(0));
    }
    Ok(out)
}
