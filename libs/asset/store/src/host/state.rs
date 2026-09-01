//! The state thread: single owner of every database handle.
//!
//! The core's DB handles are deliberately `!Send` (raw sqlite pointers), so
//! the entire `AssetServerCore` plus the transport DB live on ONE thread,
//! built on that thread. Connection threads submit closures over an mpsc
//! channel and block on a per-call reply channel. Because every call runs to
//! completion before the next starts, an authenticate + authorize + mutate
//! sequence inside one closure can never interleave with a revocation — the
//! transport gets check-and-act atomicity without reaching into the core.
//!
//! The core catalog is consumed exclusively through `AssetServerCore`'s
//! public API. The transport's own durable state (job routing metadata,
//! worker progress, result documents, the bootstrap-admin record) lives in a
//! separate `transport.sqlite3` beside it — transport-owned data the core
//! never sees.
//!
//! Failure containment: a panicking closure never kills the server. The
//! panic is caught, the whole context is rebuilt by reopening (a fresh
//! SQLite connection implicitly rolls back any transaction the panic left
//! open); if the rebuild itself fails the state is poisoned and every later
//! call returns `None`, which routes surface as 503.

use super::appdb::Db;
use super::util::{log, now_ms};
use crate::{
    AssetServerCore, Budgets, PrincipalId, RecoverReport, ServerError, ServerResult,
};
use std::path::{Path, PathBuf};
use std::sync::mpsc;

pub const TRANSPORT_SCHEMA_VERSION: u64 = 6;

/// Transport-owned durable state, deliberately outside the core catalog:
/// bootstrap-admin identity, job routing metadata, worker progress, and
/// result documents.
const TRANSPORT_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS transport_meta(
    key TEXT PRIMARY KEY,
    value BLOB NOT NULL
);
";

/// v2: the transport's bounded registry mirror. The core exposes no listing
/// or per-asset candidate API, and the single-writer-per-root law means
/// every catalog mutation flows through THIS process — so the state-thread
/// closures that perform register/stage/publish/quarantine also mirror the
/// rows the browse routes serve. (Rows written by a pre-v2 transport are not
/// back-filled; fresh roots are complete.)
const TRANSPORT_SCHEMA_V2: &str = "
CREATE TABLE IF NOT EXISTS asset_index(
    asset_id BLOB PRIMARY KEY,
    ns TEXT NOT NULL,
    created_ms INTEGER NOT NULL,
    retired_ms INTEGER
);
";

/// v3: retirement in the mirror. The browse listing must not serve retired
/// assets, and it must not do that with a post-filter — the partial indices
/// below ARE the live listing, so a store where most assets are retired
/// pages exactly as fast as one where none are. The column they cover is
/// added with `ALTER TABLE` (schema-only in SQLite), never a table rewrite.
///
/// v4: the per-revision mirror is GONE. It existed only because the core
/// exposed no per-asset candidate API; now that `Catalog::asset_candidates`
/// does, a second copy of lifecycle state could only ever drift — and it
/// did, the moment blob GC's retention rule started retiring revisions
/// without passing through a route.
const TRANSPORT_SCHEMA_V3_INDEX: &str = "
CREATE INDEX IF NOT EXISTS asset_index_live
    ON asset_index(ns, asset_id) WHERE retired_ms IS NULL;
CREATE INDEX IF NOT EXISTS asset_index_live_all
    ON asset_index(asset_id) WHERE retired_ms IS NULL;
";

const TRANSPORT_SCHEMA_V4: &str = "DROP TABLE IF EXISTS asset_rev_index;";

const ROOT_ADMIN_KEY: &str = "root_admin_principal";

pub struct StateCtx {
    pub core: AssetServerCore,
    /// Transport DB (`<root>/transport.sqlite3`), read-write.
    pub tdb: Db,
}

pub fn build_ctx(root: &Path, budgets: Budgets) -> ServerResult<(StateCtx, RecoverReport)> {
    let core = AssetServerCore::open(root, budgets)?;
    let report = core.recover(now_ms())?;

    let tdb = Db::open(&root.join("transport.sqlite3"), budgets.db_busy_timeout_ms)?;
    let mode = {
        let mut st = tdb.prepare("transport wal", "PRAGMA journal_mode=WAL")?;
        if st.step()? {
            st.column_text(0)
        } else {
            String::new()
        }
    };
    if mode != "wal" {
        return Err(ServerError::InvalidState { what: "transport journal mode", state: "not wal" });
    }
    tdb.exec("transport sync", "PRAGMA synchronous=FULL")?;
    match tdb.user_version()? {
        0 => {
            tdb.tx(|db| {
                db.exec("create transport schema", TRANSPORT_SCHEMA)?;
                db.exec("create transport schema v2", TRANSPORT_SCHEMA_V2)?;
                db.exec("create transport index v3", TRANSPORT_SCHEMA_V3_INDEX)?;
                db.exec("drop revision mirror", TRANSPORT_SCHEMA_V4)?;
                db.exec("set transport version", "PRAGMA user_version=6")
            })?;
        }
        1 | 2 | 3 => {
            tdb.tx(|db| {
                db.exec("create transport schema v2", TRANSPORT_SCHEMA_V2)?;
                if !table_has_column(db, "asset_index", "retired_ms")? {
                    db.exec(
                        "add mirror retirement column",
                        "ALTER TABLE asset_index ADD COLUMN retired_ms INTEGER",
                    )?;
                }
                db.exec("create transport index v3", TRANSPORT_SCHEMA_V3_INDEX)?;
                db.exec("drop revision mirror", TRANSPORT_SCHEMA_V4)?;
                db.exec("set transport version", "PRAGMA user_version=6")
            })?;
        }
        // A v4 root keeps every row it has; stage records simply start
        // being kept from here on. (v5 job-stage and v6 pipeline tables
        // left with the queue: a fresh root never creates them, an old
        // root keeps its rows inert, and the version simply advances.)
        4 => {
            tdb.tx(|db| {
                db.exec("set transport version", "PRAGMA user_version=6")
            })?;
        }
        5 => {
            tdb.tx(|db| {
                db.exec("set transport version", "PRAGMA user_version=6")
            })?;
        }
        TRANSPORT_SCHEMA_VERSION => {}
        other => return Err(ServerError::UnsupportedSchema { found: other as i64 }),
    }

    Ok((StateCtx { core, tdb }, report))
}

type Task = Box<dyn FnOnce(&mut StateCtx) + Send>;

#[derive(Clone)]
pub struct StateHandle {
    tx: mpsc::Sender<Task>,
}

impl StateHandle {
    /// Run a closure on the state thread and wait for its result. `None`
    /// means the state thread is gone or poisoned, or the closure panicked;
    /// routes surface that as 503.
    pub fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut StateCtx) -> R + Send + 'static,
    ) -> Option<R> {
        let (rtx, rrx) = mpsc::channel();
        let task: Task = Box::new(move |ctx| {
            let r = f(ctx);
            let _ = rtx.send(r);
        });
        self.tx.send(task).ok()?;
        rrx.recv().ok()
    }
}

/// Spawn the state thread. Blocks until the context is built so open/schema
/// failures surface synchronously at startup; the thread exits when the last
/// `StateHandle` clone is dropped.
pub fn spawn_state(
    root: PathBuf,
    budgets: Budgets,
    log_enabled: bool,
) -> ServerResult<(StateHandle, RecoverReport, std::thread::JoinHandle<()>)> {
    let (tx, rx) = mpsc::channel::<Task>();
    let (ready_tx, ready_rx) = mpsc::channel::<ServerResult<RecoverReport>>();
    let join = std::thread::Builder::new()
        .name("asset-server-state".into())
        .spawn(move || {
            let mut ctx = match build_ctx(&root, budgets) {
                Ok((ctx, report)) => {
                    let _ = ready_tx.send(Ok(report));
                    ctx
                }
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let mut poisoned = false;
            while let Ok(task) = rx.recv() {
                if poisoned {
                    // Dropping the task drops its reply sender: caller sees
                    // None immediately instead of blocking.
                    continue;
                }
                let outcome =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| task(&mut ctx)));
                if outcome.is_err() {
                    log(log_enabled, "state closure panicked; rebuilding state context");
                    // Reopening while the old connections still exist is fine
                    // (WAL supports concurrent connections); assignment then
                    // drops the old context, rolling back anything it left.
                    match build_ctx(&root, budgets) {
                        Ok((new_ctx, _)) => ctx = new_ctx,
                        Err(e) => {
                            log(log_enabled, &format!("state rebuild failed ({e}); poisoned"));
                            poisoned = true;
                        }
                    }
                }
            }
        })
        .map_err(|e| ServerError::Io { op: "spawn state thread", kind: e.kind() })?;
    let report = ready_rx
        .recv()
        .map_err(|_| ServerError::InvalidState { what: "state thread", state: "died at start" })??;
    Ok((StateHandle { tx }, report, join))
}

// ---------------------------------------------------------------------------
// job payload envelope
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// transport-side tables
// ---------------------------------------------------------------------------

impl StateCtx {
    // ---- bootstrap admin record -------------------------------------------

    pub fn root_admin_get(&self) -> ServerResult<Option<PrincipalId>> {
        let mut st = self.tdb.prepare(
            "root admin get",
            "SELECT value FROM transport_meta WHERE key=?1",
        )?;
        st.bind_text(1, ROOT_ADMIN_KEY)?;
        if !st.step()? {
            return Ok(None);
        }
        Ok(Some(PrincipalId(fixed16_of(&st.column_blob(0))?)))
    }

    pub fn root_admin_set(&self, principal: &PrincipalId) -> ServerResult<()> {
        let mut st = self.tdb.prepare(
            "root admin set",
            "INSERT INTO transport_meta(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=?2",
        )?;
        st.bind_text(1, ROOT_ADMIN_KEY)?;
        st.bind_blob(2, &principal.0)?;
        st.run()
    }

    /// True when `principal` is the recorded bootstrap admin. With no admin
    /// bootstrapped, nobody is root — fail closed.
    pub fn is_root(&self, principal: &PrincipalId) -> ServerResult<bool> {
        Ok(self.root_admin_get()?.as_ref() == Some(principal))
    }

}

// ---------------------------------------------------------------------------
// registry mirror (browse listing + candidates)
// ---------------------------------------------------------------------------

pub struct AssetIndexRow {
    pub asset_id: [u8; 16],
    pub ns: String,
    pub created_ms: u64,
}

impl StateCtx {
    pub fn asset_index_insert(&self, asset: &[u8; 16], ns: &str, now: u64) -> ServerResult<()> {
        let mut st = self.tdb.prepare(
            "asset index insert",
            "INSERT OR IGNORE INTO asset_index(asset_id, ns, created_ms) VALUES(?1, ?2, ?3)",
        )?;
        st.bind_blob(1, asset)?;
        st.bind_text(2, ns)?;
        st.bind_u64(3, now)?;
        st.run()
    }

    /// Mirror an asset retirement the core just committed: the asset leaves
    /// the browse listing. One indexed statement, scoped to this asset; the
    /// revision-level truth stays in the core (`Catalog::asset_candidates`).
    pub fn asset_mark_retired(&self, asset: &[u8; 16], now: u64) -> ServerResult<()> {
        let mut st = self.tdb.prepare(
            "asset index retire",
            "UPDATE asset_index SET retired_ms=?2 WHERE asset_id=?1 AND retired_ms IS NULL",
        )?;
        st.bind_blob(1, asset)?;
        st.bind_u64(2, now)?;
        st.run()
    }

    /// Keyset page ordered by asset id bytes (identical to display order —
    /// the fixed-width base32 encoding preserves byte order). Retired assets
    /// are excluded through the partial `asset_index_live` indices, so the
    /// listing never scans (or even sees) deleted rows.
    pub fn asset_index_page(
        &self,
        ns: Option<&str>,
        after: Option<[u8; 16]>,
        limit: u64,
    ) -> ServerResult<Vec<AssetIndexRow>> {
        let sql = match (ns.is_some(), after.is_some()) {
            (true, true) => {
                "SELECT asset_id, ns, created_ms FROM asset_index
                 WHERE ns=?1 AND asset_id>?2 AND retired_ms IS NULL
                 ORDER BY asset_id LIMIT ?3"
            }
            (true, false) => {
                "SELECT asset_id, ns, created_ms FROM asset_index
                 WHERE ns=?1 AND retired_ms IS NULL ORDER BY asset_id LIMIT ?3"
            }
            (false, true) => {
                "SELECT asset_id, ns, created_ms FROM asset_index
                 WHERE asset_id>?2 AND retired_ms IS NULL ORDER BY asset_id LIMIT ?3"
            }
            (false, false) => {
                "SELECT asset_id, ns, created_ms FROM asset_index
                 WHERE retired_ms IS NULL ORDER BY asset_id LIMIT ?3"
            }
        };
        let mut st = self.tdb.prepare("asset index page", sql)?;
        if let Some(ns) = ns {
            st.bind_text(1, ns)?;
        }
        if let Some(after) = after {
            st.bind_blob(2, &after)?;
        }
        st.bind_u64(3, limit)?;
        let mut out = Vec::new();
        while st.step()? {
            out.push(AssetIndexRow {
                asset_id: fixed16_of(&st.column_blob(0))?,
                ns: st.column_text(1),
                created_ms: st.column_u64(2),
            });
        }
        Ok(out)
    }

}

/// `PRAGMA table_info` row layout: (cid, name, type, notnull, dflt, pk).
/// PRAGMA arguments cannot travel as binds; `table` only ever comes from the
/// migration code above, never from a caller.
fn table_has_column(db: &Db, table: &str, column: &str) -> ServerResult<bool> {
    let mut st = db.prepare("mirror table info", &format!("PRAGMA table_info({table})"))?;
    while st.step()? {
        if st.column_text(1) == column {
            return Ok(true);
        }
    }
    Ok(false)
}

fn fixed16_of(bytes: &[u8]) -> ServerResult<[u8; 16]> {
    <[u8; 16]>::try_from(bytes)
        .map_err(|_| ServerError::InvalidState { what: "id column", state: "wrong length" })
}
