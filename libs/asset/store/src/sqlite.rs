//! Minimal safe wrapper over the operating system's SQLite C library.
//!
//! The core stays dependency-free by linking `libsqlite3` directly instead of
//! pulling a bindings crate. All `unsafe` in this crate is confined to this
//! module; the surface it exports is safe. The handle holds raw pointers, so
//! `Db` is deliberately neither `Send` nor `Sync` — the core is a
//! single-threaded state machine and worker concurrency is modeled logically
//! through leases, not shared connections.

// The store runs exclusively on Makepad's own SQLite-format engine
// (libs/sqlite_query). The direct-FFI binding to the operating system's
// libsqlite3 that predated it was deleted when the engine finished its
// soak — one database code path, identical on every OS.

mod own_db {
    use crate::error::{ServerError, ServerResult};
    use makepad_sqlite::{Connection, Value};
    #[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
    use makepad_sqlite::Database;
    use std::cell::RefCell;
    #[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
    use std::path::Path;
    use std::time::Duration;

    /// Engine errors carry the SQLite result code the store would have seen.
    pub(super) fn map_err(op: &'static str, e: makepad_sqlite::Error) -> ServerError {
        let code = match &e {
            makepad_sqlite::Error::Busy(_) => 5,        // SQLITE_BUSY
            makepad_sqlite::Error::Io(_) => 10,         // SQLITE_IOERR
            makepad_sqlite::Error::Corrupt(_) => 11,    // SQLITE_CORRUPT
            makepad_sqlite::Error::NotADatabase => 26,  // SQLITE_NOTADB
            makepad_sqlite::Error::Constraint(_) => 19, // SQLITE_CONSTRAINT
            _ => 1,                                     // SQLITE_ERROR
        };
        ServerError::Db { op, code }
    }

    pub struct Db {
        conn: RefCell<DbConnection>,
    }

    enum DbConnection {
        ReadWrite(Connection),
        #[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
        ReadOnly(Database),
    }

    impl Db {
        #[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
        pub fn open(path: &Path, busy_timeout_ms: u32) -> ServerResult<Db> {
            Self::open_with_timeout(makepad_sqlite::FileStoreSet::new(path), busy_timeout_ms)
        }

        #[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
        pub fn open_read_only(path: &Path) -> ServerResult<Db> {
            let db = Database::open(path).map_err(|e| map_err("open read only", e))?;
            Ok(Db {
                conn: RefCell::new(DbConnection::ReadOnly(db)),
            })
        }

        pub fn open_memory(busy_timeout_ms: u32) -> ServerResult<Db> {
            Self::open_with_timeout(makepad_sqlite::MemoryStoreSet::new(), busy_timeout_ms)
        }

        pub fn open_with<S: makepad_sqlite::PageStoreSet + 'static>(
            stores: S,
            busy_timeout_ms: u32,
        ) -> ServerResult<Db> {
            Self::open_with_timeout(stores, busy_timeout_ms)
        }

        fn open_with_timeout<S: makepad_sqlite::PageStoreSet + 'static>(
            stores: S,
            busy_timeout_ms: u32,
        ) -> ServerResult<Db> {
            let conn = Connection::open_with_timeout(
                stores,
                Duration::from_millis(busy_timeout_ms as u64),
            )
                .map_err(|e| map_err("open", e))?;
            Ok(Db {
                conn: RefCell::new(DbConnection::ReadWrite(conn)),
            })
        }

        /// One or more semicolon-separated statements that return no rows.
        pub fn exec(&self, op: &'static str, sql: &str) -> ServerResult<()> {
            match &mut *self.conn.borrow_mut() {
                DbConnection::ReadWrite(conn) => conn.execute_batch(sql).map_err(|e| map_err(op, e)),
                #[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
                DbConnection::ReadOnly(_) => Err(ServerError::InvalidState {
                    what: "read-only catalog",
                    state: "write attempted",
                }),
            }
        }

        pub fn prepare(&self, op: &'static str, sql: &str) -> ServerResult<Stmt<'_>> {
            Ok(Stmt {
                db: self,
                op,
                sql: sql.to_string(),
                params: Vec::new(),
                rows: None,
                pos: 0,
                current: None,
            })
        }

        /// Rows changed by the most recent INSERT/UPDATE/DELETE.
        pub fn changes(&self) -> u64 {
            match &*self.conn.borrow() {
                DbConnection::ReadWrite(conn) => conn.changes(),
                #[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
                DbConnection::ReadOnly(_) => 0,
            }
        }

        #[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
        pub fn user_version(&self) -> ServerResult<i64> {
            match &mut *self.conn.borrow_mut() {
                DbConnection::ReadWrite(conn) => {
                    let result = conn
                        .query("PRAGMA user_version", &[])
                        .map_err(|e| map_err("get user_version", e))?;
                    Ok(match result.scalar() {
                        Some(Value::Integer(value)) => *value,
                        _ => 0,
                    })
                }
                DbConnection::ReadOnly(db) => Ok(db.user_version() as i64),
            }
        }

        pub fn tx<T>(&self, f: impl FnOnce(&Db) -> ServerResult<T>) -> ServerResult<T> {
            self.exec("tx begin", "BEGIN IMMEDIATE")?;
            match f(self) {
                Ok(v) => {
                    self.exec("tx commit", "COMMIT")?;
                    Ok(v)
                }
                Err(e) => {
                    let _ = self.exec("tx rollback", "ROLLBACK");
                    Err(e)
                }
            }
        }

        pub fn read_tx<T>(&self, f: impl FnOnce(&Db) -> ServerResult<T>) -> ServerResult<T> {
            self.exec("read tx begin", "BEGIN DEFERRED")?;
            match f(self) {
                Ok(v) => {
                    self.exec("read tx commit", "COMMIT")?;
                    Ok(v)
                }
                Err(e) => {
                    let _ = self.exec("read tx rollback", "ROLLBACK");
                    Err(e)
                }
            }
        }
    }

    /// A statement with its bound parameters. Rows are produced on the first
    /// `step`, which keeps the connection borrow short enough that another
    /// statement can be prepared or stepped while this one is being read.
    pub struct Stmt<'db> {
        db: &'db Db,
        op: &'static str,
        sql: String,
        params: Vec<Value>,
        rows: Option<Vec<Vec<Value>>>,
        pos: usize,
        current: Option<Vec<Value>>,
    }

    impl<'db> Stmt<'db> {
        fn set(&mut self, idx: i32, value: Value) -> ServerResult<()> {
            if idx < 1 {
                return Err(ServerError::InvalidInput {
                    what: "parameter index must be 1-based",
                });
            }
            let i = idx as usize - 1;
            while self.params.len() <= i {
                self.params.push(Value::Null);
            }
            self.params[i] = value;
            Ok(())
        }

        pub fn bind_blob(&mut self, idx: i32, data: &[u8]) -> ServerResult<()> {
            self.set(idx, Value::Blob(data.to_vec()))
        }

        pub fn bind_text(&mut self, idx: i32, data: &str) -> ServerResult<()> {
            self.set(idx, Value::Text(data.to_string()))
        }

        pub fn bind_i64(&mut self, idx: i32, value: i64) -> ServerResult<()> {
            self.set(idx, Value::Integer(value))
        }

        /// Checked u64 bind: a value past `i64::MAX` would flip negative in the
        /// column and corrupt every comparison built on it.
        pub fn bind_u64(&mut self, idx: i32, value: u64) -> ServerResult<()> {
            let v = i64::try_from(value).map_err(|_| ServerError::InvalidInput {
                what: "u64 value exceeds i64 range",
            })?;
            self.bind_i64(idx, v)
        }

        pub fn bind_null(&mut self, idx: i32) -> ServerResult<()> {
            self.set(idx, Value::Null)
        }

        /// Step once. `Ok(true)` = a row is available, `Ok(false)` = done.
        pub fn step(&mut self) -> ServerResult<bool> {
            if self.rows.is_none() {
                let result = match &mut *self.db.conn.borrow_mut() {
                    DbConnection::ReadWrite(conn) => conn.query(&self.sql, &self.params),
                    #[cfg(all(feature = "native", not(any(target_arch = "wasm32", feature = "embedded"))))]
                    DbConnection::ReadOnly(db) => db.query(&self.sql, &self.params),
                }
                .map_err(|e| map_err(self.op, e))?;
                self.rows = Some(result.rows);
                self.pos = 0;
            }
            let rows = self.rows.as_ref().expect("rows");
            if self.pos < rows.len() {
                self.current = Some(rows[self.pos].clone());
                self.pos += 1;
                return Ok(true);
            }
            self.current = None;
            Ok(false)
        }

        /// Step to completion for a statement expected to return no rows.
        pub fn run(&mut self) -> ServerResult<()> {
            while self.step()? {}
            Ok(())
        }

        fn value(&self, i: i32) -> Option<&Value> {
            self.current.as_ref().and_then(|r| r.get(i.max(0) as usize))
        }

        pub fn column_i64(&self, i: i32) -> i64 {
            match self.value(i) {
                Some(Value::Integer(v)) => *v,
                Some(Value::Real(v)) => *v as i64,
                Some(Value::Text(t)) => t.trim().parse().unwrap_or(0),
                _ => 0,
            }
        }

        /// Read a column written via `bind_u64`, clamping negatives to zero.
        pub fn column_u64(&self, i: i32) -> u64 {
            self.column_i64(i).max(0) as u64
        }

        pub fn column_is_null(&self, i: i32) -> bool {
            matches!(self.value(i), None | Some(Value::Null))
        }

        pub fn column_blob(&self, i: i32) -> Vec<u8> {
            match self.value(i) {
                Some(Value::Blob(b)) => b.clone(),
                Some(Value::Text(t)) => t.as_bytes().to_vec(),
                _ => Vec::new(),
            }
        }

        pub fn column_text(&self, i: i32) -> String {
            match self.value(i) {
                Some(Value::Text(t)) => t.clone(),
                Some(Value::Integer(v)) => v.to_string(),
                Some(Value::Blob(b)) => String::from_utf8_lossy(b).into_owned(),
                _ => String::new(),
            }
        }
    }
}

pub use own_db::{Db, Stmt};
