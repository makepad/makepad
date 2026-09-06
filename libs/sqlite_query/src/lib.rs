//! `makepad-sqlite` — a clean-room Rust engine for the SQLite 3 on-disk format.
//!
//! The file layout is not ours: database header, b-tree pages, the record
//! format, overflow chains, `sqlite_master` and the write-ahead log are all
//! SQLite's, implemented from <https://www.sqlite.org/fileformat.html> so the
//! `sqlite3` CLI can read (and later write) the same files. Only the Rust
//! implementation — pager, cursors, SQL layer and planner — is ours.
//!
//! What exists today:
//! - [`pager::Pager`]: header validation, a page cache, and WAL-aware reads
//!   that serve the newest committed snapshot of a live database.
//! - [`btree`]: bounds-checked page/cell parsing and forward table/index
//!   cursors with rowid and key seeks.
//! - [`value`]: the record codec, SQLite sort order, collations and affinity.
//! - [`schema`]: `sqlite_master` scanning and a `CREATE TABLE`/`CREATE INDEX`
//!   parser that recovers columns, types, primary keys and index keys.
//!
//! [`Database`] is the read-only handle (it has no write path at all, so it can
//! never modify a file); [`Connection`] adds DML, DDL and transactions.
//!
//! The read-only surface an LLM query route wants:
//!
//! ```no_run
//! use makepad_sqlite::{Database, Value};
//! # fn main() -> makepad_sqlite::Result<()> {
//! let mut db = Database::open(std::path::Path::new("catalog.sqlite3"))?;
//! // Guard rails: a runaway query fails instead of eating the machine.
//! db.limits_mut().max_rows = 500;
//! db.limits_mut().max_steps = 5_000_000;
//!
//! // Only SELECT prepares; INSERT/UPDATE/DELETE/DDL are refused here.
//! let stmt = db.prepare("SELECT canon_alias, kind FROM search_annotations \
//!                        WHERE kind = ? ORDER BY canon_alias LIMIT ?")?;
//! println!("{}", stmt.explain()); // the access paths it will use
//!
//! let page = stmt.query(&mut db, &[Value::text("mesh"), Value::Integer(20)])?;
//! for row in &page.rows {
//!     println!("{:?}", row);
//! }
//! // Streaming instead, stopping whenever the caller likes:
//! stmt.for_each(&mut db, &[Value::text("mesh"), Value::Integer(20)], |row| {
//!     let _ = row;
//!     Ok(true)
//! })?;
//!
//! db.refresh()?; // move to the newest committed snapshot of a live database
//! # Ok(())
//! # }
//! ```

pub mod btree;
pub mod btree_write;
pub mod error;
pub mod exec;
pub mod integrity;
pub mod journal;
pub mod lock;
pub mod pager;
pub mod plan;
pub mod schema;
pub mod storage;
#[cfg(not(target_arch = "wasm32"))]
pub mod sync;
pub mod sql;
pub mod value;
pub mod wal;
pub mod write;

pub use error::{Error, Result};
pub use exec::Limits;
pub use write::Connection;
pub use pager::{DbHeader, Pager};
pub use schema::{Column, IndexInfo, Schema, SchemaObject, TableInfo};
pub use storage::{
    MemoryPageStore, MemoryStoreSet, MemoryStoreSnapshot, PageStore, PageStoreSet, StoreKind,
    StoreLock, StoreOpenOptions, MEMORY_DIRTY_PAGE_BYTES,
};
#[cfg(not(target_arch = "wasm32"))]
pub use storage::{FilePageStore, FileStoreSet};
pub use value::{Affinity, Collation, TextEncoding, TextMode, Value};

use crate::plan::{Plan, Planner};
use std::path::Path;

/// A read-only database: a pager plus the parsed schema.
pub struct Database {
    pager: Pager,
    schema: Schema,
    limits: Limits,
}

/// One prepared read-only statement.
pub struct Statement {
    pub(crate) sql: String,
    pub(crate) plan: Plan,
    pub(crate) slots: usize,
    pub(crate) columns: Vec<String>,
    /// Number of bound parameters the statement expects.
    pub parameter_count: usize,
}

/// A fully materialized result set.
#[derive(Debug, Clone)]
pub struct QueryResult {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
}

impl QueryResult {
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    /// The single value of a one-row, one-column result.
    pub fn scalar(&self) -> Option<&Value> {
        self.rows.first().and_then(|r| r.first())
    }
    /// Render like `sqlite3 -list -quote`, for diffing against the CLI.
    pub fn to_quoted_lines(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| {
                row.iter()
                    .map(exec::quote_value)
                    .collect::<Vec<_>>()
                    .join("|")
            })
            .collect()
    }
}

impl Statement {
    pub fn sql(&self) -> &str {
        &self.sql
    }
    pub fn column_names(&self) -> &[String] {
        &self.columns
    }
    /// The chosen access paths, in the spirit of `EXPLAIN QUERY PLAN`.
    pub fn explain(&self) -> String {
        self.plan.explain()
    }

    /// Run the statement, handing each row to `f`; return `Ok(false)` from `f`
    /// to stop early.
    pub fn for_each(
        &self,
        db: &mut Database,
        params: &[Value],
        mut f: impl FnMut(&[Value]) -> Result<bool>,
    ) -> Result<()> {
        if params.len() < self.parameter_count {
            return Err(Error::sql(format!(
                "statement needs {} parameters, {} were bound",
                self.parameter_count,
                params.len()
            )));
        }
        let limits = db.limits;
        // Take the read lock for the statement and give it back afterwards, so
        // an idle reader never blocks a writer in another process.
        db.pager.begin_read()?;
        let mut rt = exec::Runtime::new(&mut db.pager, params, self.slots, limits);
        let result = exec::run(&mut rt, &self.plan, &mut |row| f(&row));
        let unlocked = db.pager.unlock();
        result.and(unlocked)
    }

    pub fn query(&self, db: &mut Database, params: &[Value]) -> Result<QueryResult> {
        let mut rows = Vec::new();
        let max = db.limits.max_rows;
        let mut overflow = false;
        self.for_each(db, params, |row| {
            if rows.len() >= max {
                overflow = true;
                return Ok(false);
            }
            rows.push(row.to_vec());
            Ok(true)
        })?;
        if overflow {
            return Err(Error::Budget(format!("more than {max} result rows")));
        }
        Ok(QueryResult {
            columns: self.columns.clone(),
            rows,
        })
    }
}

impl Database {
    /// Open a database file (following its `-wal` when present).
    pub fn open(path: &Path) -> Result<Database> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut pager = Pager::open(path)?;
            let schema = Schema::load(&mut pager)?;
            // Idle handles hold nothing: every statement takes its own read lock.
            pager.unlock()?;
            return Ok(Database {
                pager,
                schema,
                limits: Limits::default(),
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = path;
            Err(Error::unsupported("filesystem databases are unavailable on wasm32"))
        }
    }

    pub fn open_with_cache(path: &Path, cache_pages: usize) -> Result<Database> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let mut pager = Pager::open_with_cache(path, cache_pages)?;
            let schema = Schema::load(&mut pager)?;
            pager.unlock()?;
            return Ok(Database {
                pager,
                schema,
                limits: Limits::default(),
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (path, cache_pages);
            Err(Error::unsupported("filesystem databases are unavailable on wasm32"))
        }
    }

    /// Prepare a read-only statement against the current schema.
    pub fn prepare(&mut self, sql: &str) -> Result<Statement> {
        let stmt = crate::sql::parse::parse(sql)?;
        let sql::ast::Stmt::Select(select) = stmt else {
            return Err(Error::sql(
                "this handle is read-only: only SELECT can be prepared",
            ));
        };
        let mut planner = Planner::new(&self.schema);
        let plan = planner.plan_select(&select)?;
        Ok(Statement {
            sql: sql.to_string(),
            columns: plan.column_names(),
            slots: planner.slot_total().max(1),
            parameter_count: planner.max_param,
            plan,
        })
    }

    /// Prepare and run in one step.
    pub fn query(&mut self, sql: &str, params: &[Value]) -> Result<QueryResult> {
        let stmt = self.prepare(sql)?;
        stmt.query(self, params)
    }

    /// Row and step budgets for statements run through this handle.
    pub fn limits_mut(&mut self) -> &mut Limits {
        &mut self.limits
    }
    pub fn limits(&self) -> Limits {
        self.limits
    }

    /// Move to the newest committed snapshot and re-read the schema if the
    /// schema cookie changed.
    pub fn refresh(&mut self) -> Result<()> {
        let before = self.pager.header().schema_cookie;
        self.pager.refresh()?;
        if self.pager.header().schema_cookie != before {
            self.schema = Schema::load(&mut self.pager)?;
        }
        Ok(())
    }

    pub fn schema(&self) -> &Schema {
        &self.schema
    }

    pub fn pager(&mut self) -> &mut Pager {
        &mut self.pager
    }

    pub fn parts(&mut self) -> (&mut Pager, &Schema) {
        (&mut self.pager, &self.schema)
    }

    pub fn user_version(&self) -> i32 {
        self.pager.header().user_version
    }
}
