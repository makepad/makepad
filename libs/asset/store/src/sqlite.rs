//! Minimal safe wrapper over the operating system's SQLite C library.
//!
//! The core stays dependency-free by linking `libsqlite3` directly instead of
//! pulling a bindings crate. All `unsafe` in this crate is confined to this
//! module; the surface it exports is safe. The handle holds raw pointers, so
//! `Db` is deliberately neither `Send` nor `Sync` — the core is a
//! single-threaded state machine and worker concurrency is modeled logically
//! through leases, not shared connections.

use crate::error::{ServerError, ServerResult};
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_void};
use std::path::Path;

#[allow(non_camel_case_types)]
enum sqlite3 {}
#[allow(non_camel_case_types)]
enum sqlite3_stmt {}

const SQLITE_OK: c_int = 0;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_OPEN_READWRITE: c_int = 0x0000_0002;
const SQLITE_OPEN_CREATE: c_int = 0x0000_0004;
const SQLITE_OPEN_FULLMUTEX: c_int = 0x0001_0000;

// SQLITE_TRANSIENT: tells SQLite to copy the bound bytes immediately, so the
// Rust slice only needs to live for the duration of the bind call.
fn transient() -> *const c_void {
    -1isize as *const c_void
}

/// Checked length conversion for every byte count handed to SQLite. A length
/// that does not fit `c_int` would silently truncate or go negative in the C
/// API, so it is refused instead. Budgets keep real payloads far below this.
fn len_c_int(len: usize, what: &'static str) -> ServerResult<c_int> {
    c_int::try_from(len).map_err(|_| ServerError::OverBudget {
        what,
        limit: c_int::MAX as u64,
        found: len as u64,
    })
}

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
    fn sqlite3_bind_null(stmt: *mut sqlite3_stmt, idx: c_int) -> c_int;
    fn sqlite3_column_type(stmt: *mut sqlite3_stmt, i: c_int) -> c_int;
    fn sqlite3_column_int64(stmt: *mut sqlite3_stmt, i: c_int) -> i64;
    fn sqlite3_column_blob(stmt: *mut sqlite3_stmt, i: c_int) -> *const c_void;
    fn sqlite3_column_text(stmt: *mut sqlite3_stmt, i: c_int) -> *const u8;
    fn sqlite3_column_bytes(stmt: *mut sqlite3_stmt, i: c_int) -> c_int;
    fn sqlite3_changes(db: *mut sqlite3) -> c_int;
}

const SQLITE_NULL: c_int = 5;

pub struct Db {
    raw: *mut sqlite3,
}

impl Db {
    pub fn open(path: &Path, busy_timeout_ms: u32) -> ServerResult<Db> {
        let bytes = path.as_os_str().as_encoded_bytes();
        let cpath = CString::new(bytes)
            .map_err(|_| ServerError::InvalidInput { what: "db path contains NUL" })?;
        let mut raw: *mut sqlite3 = std::ptr::null_mut();
        let flags = SQLITE_OPEN_READWRITE | SQLITE_OPEN_CREATE | SQLITE_OPEN_FULLMUTEX;
        let rc = unsafe { sqlite3_open_v2(cpath.as_ptr(), &mut raw, flags, std::ptr::null()) };
        if rc != SQLITE_OK || raw.is_null() {
            if !raw.is_null() {
                unsafe { sqlite3_close(raw) };
            }
            return Err(ServerError::Db { op: "open", code: rc });
        }
        // u32 -> c_int checked: a timeout above i32::MAX would go negative
        // (busy handler disabled) instead of waiting; refuse it.
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

    /// Execute one or more semicolon-separated statements that return no rows
    /// (schema setup, transactions, pragmas without results).
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
            // The message is never surfaced (structured errors only); free it.
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

    /// Rows changed by the most recent INSERT/UPDATE/DELETE. Clamped at zero:
    /// a negative count from the C API must never wrap to a huge u64.
    pub fn changes(&self) -> u64 {
        (unsafe { sqlite3_changes(self.raw) }).max(0) as u64
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
                // Best-effort rollback; the original error wins either way.
                let _ = self.exec("tx rollback", "ROLLBACK");
                Err(e)
            }
        }
    }

    /// Run `f` inside a DEFERRED read transaction: every statement inside
    /// observes one consistent snapshot (WAL readers never block the writer).
    /// Nothing is written, so commit and rollback are equivalent; errors take
    /// the rollback path anyway to end the transaction unconditionally.
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

impl Drop for Db {
    fn drop(&mut self) {
        unsafe { sqlite3_close(self.raw) };
    }
}

pub struct Stmt<'db> {
    raw: *mut sqlite3_stmt,
    db: &'db Db,
    op: &'static str,
}

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
            sqlite3_bind_blob(
                self.raw,
                idx,
                data.as_ptr() as *const c_void,
                n,
                transient(),
            )
        };
        self.check(rc)
    }

    pub fn bind_text(&mut self, idx: i32, data: &str) -> ServerResult<()> {
        let n = len_c_int(data.len(), "bind text bytes")?;
        let rc = unsafe {
            sqlite3_bind_text(
                self.raw,
                idx,
                data.as_ptr() as *const c_char,
                n,
                transient(),
            )
        };
        self.check(rc)
    }

    pub fn bind_i64(&mut self, idx: i32, value: i64) -> ServerResult<()> {
        self.check(unsafe { sqlite3_bind_int64(self.raw, idx, value) })
    }

    /// Checked u64 -> INTEGER bind. Sizes and timestamps are u64 in the API;
    /// a value past i64::MAX would flip negative in the column and corrupt
    /// every comparison built on it, so it is refused instead.
    pub fn bind_u64(&mut self, idx: i32, value: u64) -> ServerResult<()> {
        let v = i64::try_from(value)
            .map_err(|_| ServerError::InvalidInput { what: "u64 value exceeds i64 range" })?;
        self.bind_i64(idx, v)
    }

    pub fn bind_null(&mut self, idx: i32) -> ServerResult<()> {
        self.check(unsafe { sqlite3_bind_null(self.raw, idx) })
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

    pub fn column_i64(&self, i: i32) -> i64 {
        unsafe { sqlite3_column_int64(self.raw, i) }
    }

    /// Read a column written via `bind_u64`, clamping negatives to zero.
    /// A negative value can only mean row corruption (the write path refuses
    /// them); zero is the fail-closed reading everywhere this is used — a
    /// zero size mismatches real bytes and a zero expiry is already expired.
    pub fn column_u64(&self, i: i32) -> u64 {
        self.column_i64(i).max(0) as u64
    }

    pub fn column_is_null(&self, i: i32) -> bool {
        (unsafe { sqlite3_column_type(self.raw, i) }) == SQLITE_NULL
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
        // SQLite guarantees valid UTF-8 for TEXT columns we wrote as &str;
        // fail closed to empty on the impossible case rather than panic.
        String::from_utf8(bytes.to_vec()).unwrap_or_default()
    }
}

impl<'db> Drop for Stmt<'db> {
    fn drop(&mut self) {
        unsafe { sqlite3_finalize(self.raw) };
    }
}
