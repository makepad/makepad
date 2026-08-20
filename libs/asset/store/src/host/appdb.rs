//! Transport-side SQLite wrapper for `transport.sqlite3` only — the core
//! catalog is consumed exclusively through `AssetServerCore`'s public API,
//! never by SQL. The core's own wrapper is private (and the core must stay
//! transport-free), so this crate links libsqlite3 itself with the same safe
//! surface. `Db` holds raw pointers and is deliberately neither `Send` nor
//! `Sync`; every handle lives on the state thread.

#[cfg(not(feature = "own-db"))]
use crate::{ServerError, ServerResult};
#[cfg(not(feature = "own-db"))]
use std::ffi::CString;
#[cfg(not(feature = "own-db"))]
use std::os::raw::{c_char, c_int, c_void};
#[cfg(not(feature = "own-db"))]
use std::path::Path;

#[cfg(not(feature = "own-db"))]
#[allow(non_camel_case_types)]
enum sqlite3 {}
#[cfg(not(feature = "own-db"))]
#[allow(non_camel_case_types)]
enum sqlite3_stmt {}

#[cfg(not(feature = "own-db"))]
const SQLITE_OK: c_int = 0;
#[cfg(not(feature = "own-db"))]
const SQLITE_ROW: c_int = 100;
#[cfg(not(feature = "own-db"))]
const SQLITE_DONE: c_int = 101;
#[cfg(not(feature = "own-db"))]
const SQLITE_OPEN_READWRITE: c_int = 0x0000_0002;
#[cfg(not(feature = "own-db"))]
const SQLITE_OPEN_CREATE: c_int = 0x0000_0004;
#[cfg(not(feature = "own-db"))]
const SQLITE_OPEN_FULLMUTEX: c_int = 0x0001_0000;

#[cfg(not(feature = "own-db"))]
// SQLITE_TRANSIENT: SQLite copies bound bytes immediately, so the Rust slice
// only needs to live for the duration of the bind call.
fn transient() -> *const c_void {
    -1isize as *const c_void
}

#[cfg(not(feature = "own-db"))]
/// Checked length conversion for every byte count handed to SQLite.
fn len_c_int(len: usize, what: &'static str) -> ServerResult<c_int> {
    c_int::try_from(len).map_err(|_| ServerError::OverBudget {
        what,
        limit: c_int::MAX as u64,
        found: len as u64,
    })
}

#[cfg(not(feature = "own-db"))]
#[link(name = "sqlite3")]
extern "C" {
    fn sqlite3_open_v2(
        filename: *const c_char,
        db: *mut *mut sqlite3,
        flags: c_int,
        vfs: *const c_char,
    ) -> c_int;
    fn sqlite3_close(db: *mut sqlite3) -> c_int;
    fn sqlite3_extended_result_codes(db: *mut sqlite3, onoff: c_int) -> c_int;
    fn sqlite3_extended_errcode(db: *mut sqlite3) -> c_int;
    fn sqlite3_busy_timeout(db: *mut sqlite3, ms: c_int) -> c_int;
    fn sqlite3_exec(
        db: *mut sqlite3,
        sql: *const c_char,
        callback: *const c_void,
        arg: *mut c_void,
        errmsg: *mut *mut c_char,
    ) -> c_int;
    fn sqlite3_free(ptr: *mut c_void);
    fn sqlite3_prepare_v2(
        db: *mut sqlite3,
        sql: *const c_char,
        n_bytes: c_int,
        stmt: *mut *mut sqlite3_stmt,
        tail: *mut *const c_char,
    ) -> c_int;
    fn sqlite3_step(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_finalize(stmt: *mut sqlite3_stmt) -> c_int;
    fn sqlite3_bind_blob(
        stmt: *mut sqlite3_stmt,
        idx: c_int,
        data: *const c_void,
        n: c_int,
        dtor: *const c_void,
    ) -> c_int;
    fn sqlite3_bind_text(
        stmt: *mut sqlite3_stmt,
        idx: c_int,
        data: *const c_char,
        n: c_int,
        dtor: *const c_void,
    ) -> c_int;
    fn sqlite3_bind_int64(stmt: *mut sqlite3_stmt, idx: c_int, value: i64) -> c_int;
    fn sqlite3_column_int64(stmt: *mut sqlite3_stmt, i: c_int) -> i64;
    fn sqlite3_column_type(stmt: *mut sqlite3_stmt, i: c_int) -> c_int;
    fn sqlite3_column_blob(stmt: *mut sqlite3_stmt, i: c_int) -> *const c_void;
    fn sqlite3_column_text(stmt: *mut sqlite3_stmt, i: c_int) -> *const u8;
    fn sqlite3_column_bytes(stmt: *mut sqlite3_stmt, i: c_int) -> c_int;
}

#[cfg(not(feature = "own-db"))]
pub struct Db {
    raw: *mut sqlite3,
}

#[cfg(not(feature = "own-db"))]
impl Db {
    pub fn open(path: &Path, busy_timeout_ms: u32) -> ServerResult<Db> {
        Self::open_flags(
            path,
            busy_timeout_ms,
            SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX,
        )
    }

    fn open_flags(path: &Path, busy_timeout_ms: u32, flags: c_int) -> ServerResult<Db> {
        let bytes = path.as_os_str().as_encoded_bytes();
        let cpath = CString::new(bytes)
            .map_err(|_| ServerError::InvalidInput { what: "db path contains NUL" })?;
        let mut raw: *mut sqlite3 = std::ptr::null_mut();
        let rc = unsafe { sqlite3_open_v2(cpath.as_ptr(), &mut raw, flags, std::ptr::null()) };
        if rc != SQLITE_OK || raw.is_null() {
            if !raw.is_null() {
                unsafe { sqlite3_close(raw) };
            }
            return Err(ServerError::Db { op: "open", code: rc });
        }
        let timeout = match c_int::try_from(busy_timeout_ms) {
            Ok(t) => t,
            Err(_) => {
                unsafe { sqlite3_close(raw) };
                return Err(ServerError::InvalidInput { what: "db busy timeout out of range" });
            }
        };
        unsafe {
            sqlite3_extended_result_codes(raw, 1);
            sqlite3_busy_timeout(raw, timeout);
        }
        Ok(Db { raw })
    }

    fn err(&self, op: &'static str) -> ServerError {
        ServerError::Db {
            op,
            code: unsafe { sqlite3_extended_errcode(self.raw) },
        }
    }

    pub fn exec(&self, op: &'static str, sql: &str) -> ServerResult<()> {
        let csql = CString::new(sql).map_err(|_| ServerError::InvalidInput { what: "sql NUL" })?;
        let mut errmsg: *mut c_char = std::ptr::null_mut();
        let rc = unsafe {
            sqlite3_exec(
                self.raw,
                csql.as_ptr(),
                std::ptr::null(),
                std::ptr::null_mut(),
                &mut errmsg,
            )
        };
        if !errmsg.is_null() {
            unsafe { sqlite3_free(errmsg as *mut c_void) };
        }
        if rc != SQLITE_OK {
            return Err(ServerError::Db { op, code: rc });
        }
        Ok(())
    }

    pub fn prepare(&self, op: &'static str, sql: &str) -> ServerResult<Stmt<'_>> {
        let n_bytes = len_c_int(sql.len(), "sql bytes")?;
        let mut raw: *mut sqlite3_stmt = std::ptr::null_mut();
        let rc = unsafe {
            sqlite3_prepare_v2(
                self.raw,
                sql.as_ptr() as *const c_char,
                n_bytes,
                &mut raw,
                std::ptr::null_mut(),
            )
        };
        if rc != SQLITE_OK || raw.is_null() {
            return Err(self.err(op));
        }
        Ok(Stmt { raw, db: self, op })
    }

    /// Run `f` inside an IMMEDIATE transaction; rollback on error.
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

    /// Read `PRAGMA user_version`.
    pub fn user_version(&self) -> ServerResult<u64> {
        let mut stmt = self.prepare("user_version", "PRAGMA user_version")?;
        if stmt.step()? {
            Ok(stmt.column_u64(0))
        } else {
            Err(ServerError::Db { op: "user_version", code: 0 })
        }
    }
}

#[cfg(not(feature = "own-db"))]
impl Drop for Db {
    fn drop(&mut self) {
        unsafe { sqlite3_close(self.raw) };
    }
}

#[cfg(not(feature = "own-db"))]
pub struct Stmt<'db> {
    raw: *mut sqlite3_stmt,
    db: &'db Db,
    op: &'static str,
}

#[cfg(not(feature = "own-db"))]
impl<'db> Stmt<'db> {
    fn check(&self, rc: c_int) -> ServerResult<()> {
        if rc != SQLITE_OK {
            return Err(self.db.err(self.op));
        }
        Ok(())
    }

    pub fn bind_blob(&mut self, idx: i32, data: &[u8]) -> ServerResult<()> {
        let n = len_c_int(data.len(), "bind blob bytes")?;
        let rc = unsafe {
            sqlite3_bind_blob(self.raw, idx, data.as_ptr() as *const c_void, n, transient())
        };
        self.check(rc)
    }

    pub fn bind_text(&mut self, idx: i32, data: &str) -> ServerResult<()> {
        let n = len_c_int(data.len(), "bind text bytes")?;
        let rc = unsafe {
            sqlite3_bind_text(self.raw, idx, data.as_ptr() as *const c_char, n, transient())
        };
        self.check(rc)
    }

    /// Checked u64 -> INTEGER bind; a value past i64::MAX would flip negative
    /// in the column and corrupt comparisons, so it is refused.
    pub fn bind_u64(&mut self, idx: i32, value: u64) -> ServerResult<()> {
        let v = i64::try_from(value)
            .map_err(|_| ServerError::InvalidInput { what: "u64 value exceeds i64 range" })?;
        self.check(unsafe { sqlite3_bind_int64(self.raw, idx, v) })
    }

    /// Step once. `Ok(true)` = a row is available, `Ok(false)` = done.
    pub fn step(&mut self) -> ServerResult<bool> {
        match unsafe { sqlite3_step(self.raw) } {
            SQLITE_ROW => Ok(true),
            SQLITE_DONE => Ok(false),
            _ => Err(self.db.err(self.op)),
        }
    }

    /// Step to completion for a statement expected to return no rows.
    pub fn run(&mut self) -> ServerResult<()> {
        while self.step()? {}
        Ok(())
    }

    /// Read a column written via `bind_u64`, clamping negatives to zero
    /// (fail-closed on impossible corruption).
    pub fn column_u64(&self, i: i32) -> u64 {
        (unsafe { sqlite3_column_int64(self.raw, i) }).max(0) as u64
    }

    /// Nullable `column_u64`: SQL NULL reads as `None`.
    pub fn column_u64_opt(&self, i: i32) -> ServerResult<Option<u64>> {
        const SQLITE_NULL: c_int = 5;
        if unsafe { sqlite3_column_type(self.raw, i) } == SQLITE_NULL {
            return Ok(None);
        }
        Ok(Some(self.column_u64(i)))
    }

    pub fn column_blob(&self, i: i32) -> Vec<u8> {
        let n = unsafe { sqlite3_column_bytes(self.raw, i) };
        if n <= 0 {
            return Vec::new();
        }
        let ptr = unsafe { sqlite3_column_blob(self.raw, i) };
        if ptr.is_null() {
            return Vec::new();
        }
        unsafe { std::slice::from_raw_parts(ptr as *const u8, n as usize) }.to_vec()
    }

    pub fn column_text(&self, i: i32) -> String {
        let n = unsafe { sqlite3_column_bytes(self.raw, i) };
        if n <= 0 {
            return String::new();
        }
        let ptr = unsafe { sqlite3_column_text(self.raw, i) };
        if ptr.is_null() {
            return String::new();
        }
        let bytes = unsafe { std::slice::from_raw_parts(ptr, n as usize) };
        String::from_utf8(bytes.to_vec()).unwrap_or_default()
    }
}

#[cfg(not(feature = "own-db"))]
impl<'db> Drop for Stmt<'db> {
    fn drop(&mut self) {
        unsafe { sqlite3_finalize(self.raw) };
    }
}

// ---------------------------------------------------------------------------
// The same surface on Makepad's own SQLite-format engine (`own-db`)
// ---------------------------------------------------------------------------

/// The transport database on `makepad-sqlite`: same file, same locking, no FFI.
#[cfg(feature = "own-db")]
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

        pub fn column_u64_opt(&self, i: i32) -> ServerResult<Option<u64>> {
            if matches!(self.value(i), None | Some(Value::Null)) {
                return Ok(None);
            }
            Ok(Some(self.column_u64(i)))
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

#[cfg(feature = "own-db")]
pub use own_db::{Db, Stmt};
