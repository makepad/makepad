//! Transport-side SQLite wrapper for `transport.sqlite3` only — the core
//! catalog is consumed exclusively through `AssetServerCore`'s public API,
//! never by SQL. The core's own wrapper is private (and the core must stay
//! transport-free), so this crate links libsqlite3 itself with the same safe
//! surface. `Db` holds raw pointers and is deliberately neither `Send` nor
//! `Sync`; every handle lives on the state thread.

// The store runs exclusively on Makepad's own SQLite-format engine
// (libs/sqlite_query). The direct-FFI binding to the operating system's
// libsqlite3 that predated it was deleted when the engine finished its
// soak — one database code path, identical on every OS.

mod own_db {
    use crate::{ServerError, ServerResult};
    use makepad_sqlite::{Connection, Value};
    use std::cell::RefCell;
    use std::path::Path;
    use std::time::Duration;

    fn map_err(op: &'static str, e: makepad_sqlite::Error) -> ServerError {
        let code = match &e {
            makepad_sqlite::Error::Busy(_) => 5,
            makepad_sqlite::Error::Io(_) => 10,
            makepad_sqlite::Error::Corrupt(_) => 11,
            makepad_sqlite::Error::NotADatabase => 26,
            makepad_sqlite::Error::Constraint(_) => 19,
            _ => 1,
        };
        ServerError::Db { op, code }
    }

    pub struct Db {
        conn: RefCell<Connection>,
    }

    impl Db {
        pub fn open(path: &Path, busy_timeout_ms: u32) -> ServerResult<Db> {
            let conn = Connection::open(path, Duration::from_millis(busy_timeout_ms as u64))
                .map_err(|e| map_err("open", e))?;
            Ok(Db {
                conn: RefCell::new(conn),
            })
        }

        pub fn exec(&self, op: &'static str, sql: &str) -> ServerResult<()> {
            self.conn
                .borrow_mut()
                .execute_batch(sql)
                .map_err(|e| map_err(op, e))
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

        pub fn user_version(&self) -> ServerResult<u64> {
            let mut stmt = self.prepare("user_version", "PRAGMA user_version")?;
            if stmt.step()? {
                Ok(stmt.column_u64(0))
            } else {
                Err(ServerError::Db {
                    op: "user_version",
                    code: 0,
                })
            }
        }
    }

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

        pub fn bind_u64(&mut self, idx: i32, value: u64) -> ServerResult<()> {
            let v = i64::try_from(value).map_err(|_| ServerError::InvalidInput {
                what: "u64 value exceeds i64 range",
            })?;
            self.set(idx, Value::Integer(v))
        }

        pub fn step(&mut self) -> ServerResult<bool> {
            if self.rows.is_none() {
                let result = self
                    .db
                    .conn
                    .borrow_mut()
                    .query(&self.sql, &self.params)
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

        pub fn run(&mut self) -> ServerResult<()> {
            while self.step()? {}
            Ok(())
        }

        fn value(&self, i: i32) -> Option<&Value> {
            self.current.as_ref().and_then(|r| r.get(i.max(0) as usize))
        }

        pub fn column_u64(&self, i: i32) -> u64 {
            match self.value(i) {
                Some(Value::Integer(v)) => (*v).max(0) as u64,
                Some(Value::Real(v)) => (*v).max(0.0) as u64,
                Some(Value::Text(t)) => t.trim().parse().unwrap_or(0),
                _ => 0,
            }
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

pub use own_db::Db;
