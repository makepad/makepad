//! Database header parsing, the page cache and WAL-aware page reads.
//!
//! Layout references: <https://www.sqlite.org/fileformat.html> (section 1.3,
//! "Database Header", and section 4, "The Write-Ahead Log"). The WAL checksum
//! algorithm is the documented Fibonacci-weighted pair; the byte order is
//! selected by the low bit of the WAL magic, matching SQLite's `bigEndCksum`
//! (also how turso/core/storage/sqlite3_ondisk.rs `checksum_wal` reads it).

use crate::error::{Error, Result};
use crate::journal::{self, Journal};
use crate::lock::{FileLock, LockLevel, ProcessLock};
use crate::storage::{PageStore, PageStoreSet, StoreKind, StoreOpenOptions};
use crate::wal::{ShmGuard, Wal};
use crate::value::{be_u16, be_u32, TextEncoding};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

// ---------------------------------------------------------------------------
// Crash injection
// ---------------------------------------------------------------------------

/// Countdown to a simulated power failure, in units of durable write steps
/// (each page written to the database or the journal, and each sync). Zero
/// disables it. The crash tests set this through `MAKEPAD_SQLITE_CRASH_AT` so a
/// child process can be killed at an exact point in a commit and the file
/// checked afterwards; nothing sets it in production.
static CRASH_AT: AtomicU64 = AtomicU64::new(0);
static WRITE_STEPS: AtomicU64 = AtomicU64::new(0);

/// Abort the process after `n` further write steps (0 = never).
pub fn set_crash_after(n: u64) {
    CRASH_AT.store(n, Ordering::SeqCst);
    WRITE_STEPS.store(0, Ordering::SeqCst);
}

/// Journal record writes count as steps too (crate-internal hook).
pub(crate) fn journal_write_step() {
    write_step();
}

/// Write steps performed since the last [`set_crash_after`].
pub fn write_steps() -> u64 {
    WRITE_STEPS.load(Ordering::SeqCst)
}

/// Count one durable write step, and stop the world if this is the one the
/// crash test asked for.
#[inline]
fn write_step() {
    let steps = WRITE_STEPS.fetch_add(1, Ordering::SeqCst) + 1;
    let at = CRASH_AT.load(Ordering::SeqCst);
    if at != 0 && steps >= at {
        // No unwinding, no destructors, no flush: exactly what a power cut does.
        std::process::abort();
    }
}
use std::sync::Arc;

pub const MAGIC: &[u8; 16] = b"SQLite format 3\0";
pub const HEADER_SIZE: usize = 100;

// ---------------------------------------------------------------------------
// Database header
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct DbHeader {
    pub page_size: u32,
    pub write_version: u8,
    pub read_version: u8,
    pub reserved_space: u8,
    pub file_change_counter: u32,
    pub database_size_pages: u32,
    pub freelist_trunk_page: u32,
    pub freelist_pages: u32,
    pub schema_cookie: u32,
    pub schema_format: u32,
    pub largest_root_btree_page: u32,
    pub text_encoding: TextEncoding,
    pub user_version: i32,
    pub incremental_vacuum: u32,
    pub application_id: u32,
    pub version_valid_for: u32,
    pub sqlite_version_number: u32,
}

impl DbHeader {
    pub fn parse(buf: &[u8]) -> Result<DbHeader> {
        let buf = buf
            .get(..HEADER_SIZE)
            .ok_or_else(|| Error::corrupt("file shorter than the 100-byte header"))?;
        if &buf[0..16] != MAGIC {
            return Err(Error::NotADatabase);
        }
        let raw_page_size = be_u16(buf, 16)?;
        let page_size = match raw_page_size {
            1 => 65536u32,
            n if n >= 512 && n.is_power_of_two() => n as u32,
            _ => return Err(Error::corrupt("page size is not a power of two in 512..=65536")),
        };
        let write_version = buf[18];
        let read_version = buf[19];
        if read_version > 2 {
            return Err(Error::unsupported(format!(
                "file format read version {read_version} (this engine understands 1 and 2)"
            )));
        }
        let reserved_space = buf[20];
        if page_size as usize <= reserved_space as usize + 480 {
            return Err(Error::corrupt(
                "reserved space leaves less than 480 usable bytes per page",
            ));
        }
        if buf[21] != 64 || buf[22] != 32 || buf[23] != 32 {
            return Err(Error::corrupt(
                "payload fractions are not the required 64/32/32",
            ));
        }
        let schema_format = be_u32(buf, 44)?;
        if schema_format > 4 {
            return Err(Error::unsupported(format!(
                "schema format {schema_format}"
            )));
        }
        let text_encoding = match be_u32(buf, 56)? {
            0 | 1 => TextEncoding::Utf8,
            2 => TextEncoding::Utf16Le,
            3 => TextEncoding::Utf16Be,
            n => return Err(Error::corrupt(format!("text encoding {n}"))),
        };
        Ok(DbHeader {
            page_size,
            write_version,
            read_version,
            reserved_space,
            file_change_counter: be_u32(buf, 24)?,
            database_size_pages: be_u32(buf, 28)?,
            freelist_trunk_page: be_u32(buf, 32)?,
            freelist_pages: be_u32(buf, 36)?,
            schema_cookie: be_u32(buf, 40)?,
            schema_format,
            largest_root_btree_page: be_u32(buf, 52)?,
            text_encoding,
            user_version: be_u32(buf, 60)? as i32,
            incremental_vacuum: be_u32(buf, 64)?,
            application_id: be_u32(buf, 68)?,
            version_valid_for: be_u32(buf, 92)?,
            sqlite_version_number: be_u32(buf, 96)?,
        })
    }

    pub fn usable_size(&self) -> usize {
        self.page_size as usize - self.reserved_space as usize
    }

    /// Auto-vacuum databases sprinkle pointer-map pages through the file.
    pub fn auto_vacuum(&self) -> bool {
        self.largest_root_btree_page != 0
    }
}

// ---------------------------------------------------------------------------
// Page cache
// ---------------------------------------------------------------------------

struct PageCache {
    map: HashMap<u32, Arc<[u8]>>,
    order: VecDeque<u32>,
    capacity: usize,
    pub hits: u64,
    pub misses: u64,
}

impl PageCache {
    fn new(capacity: usize) -> PageCache {
        PageCache {
            map: HashMap::new(),
            order: VecDeque::new(),
            capacity: capacity.max(1),
            hits: 0,
            misses: 0,
        }
    }
    fn get(&mut self, pgno: u32) -> Option<Arc<[u8]>> {
        match self.map.get(&pgno) {
            Some(p) => {
                self.hits += 1;
                Some(p.clone())
            }
            None => {
                self.misses += 1;
                None
            }
        }
    }
    fn insert(&mut self, pgno: u32, page: Arc<[u8]>) {
        if self.map.contains_key(&pgno) {
            self.map.insert(pgno, page);
            return;
        }
        while self.map.len() >= self.capacity {
            match self.order.pop_front() {
                Some(evict) => {
                    self.map.remove(&evict);
                }
                None => break,
            }
        }
        self.map.insert(pgno, page);
        self.order.push_back(pgno);
    }
    fn clear(&mut self) {
        self.map.clear();
        self.order.clear();
    }
}

// ---------------------------------------------------------------------------
// Pager
// ---------------------------------------------------------------------------

/// Read access to the pages of one SQLite database file, including any pages
/// that currently live only in its write-ahead log.
pub struct Pager {
    path: PathBuf,
    stores: Arc<dyn PageStoreSet>,
    file: Arc<dyn PageStore>,
    header: DbHeader,
    usable: usize,
    page_size: usize,
    /// Logical database size in pages for this snapshot.
    page_count: u32,
    wal: Option<Wal>,
    /// Held for as long as this connection owns the log (WAL mode, writable).
    shm: Option<ShmGuard>,
    cache: PageCache,
    /// The committed snapshot the cache's pages were read under. `None` =
    /// cache contents are not vouched for and the next `load_snapshot`
    /// clears them. See [`Pager::snapshot_stamp`].
    cache_stamp: Option<SnapshotStamp>,
    /// Byte-range locks; readers take SHARED, writers climb to EXCLUSIVE.
    lock: FileLock,
    busy_timeout: Duration,
    /// Frames after which a commit folds the log back into the database.
    autocheckpoint: u32,
    /// Write support; `None` for a read-only pager.
    write: Option<WriteSupport>,
}

/// Identity of one committed snapshot, for page-cache validity: the change
/// counter and size cover rollback-journal commits (and truncation), the WAL
/// fields cover every commit, reset and checkpoint of the log. Equal stamps =
/// byte-identical committed content = the cache's pages are still this
/// snapshot's pages.
#[derive(Clone, Copy, PartialEq, Eq)]
struct SnapshotStamp {
    change_counter: u32,
    page_count: u32,
    /// (salt, checkpoint_seq, committed frames); `None` = no log.
    wal: Option<((u32, u32), u32, u32)>,
}

/// Everything a writable pager needs beyond reading.
struct WriteSupport {
    /// Shared with every other connection to this file in this process.
    process_lock: std::sync::Arc<ProcessLock>,
    /// True while this connection holds the in-process write slot.
    holds_process_write: bool,
    /// Set between `begin_write` and `commit`/`rollback`.
    tx: Option<WriteTx>,
}

struct WriteTx {
    /// The rollback journal; `None` in WAL mode, where frames are the record.
    journal: Option<Journal>,
    /// Page images staged for this transaction. Held as `Arc` so reads inside
    /// the transaction see them without copying, and never dropped by cache
    /// eviction (which would lose the update).
    dirty: HashMap<u32, Arc<[u8]>>,
    /// Database size in pages when the transaction started.
    initial_pages: u32,
    /// Current logical size, which the transaction may have grown.
    pages: u32,
    /// Pages already written into the database file ("spilled"): their
    /// originals are in the journal, so a rollback still restores them.
    spilled: bool,
}

/// Default page cache size (pages). 256 pages is 1 MiB at the usual 4 KiB
/// page size: enough to hold the upper levels of several b-trees during a join.
pub const DEFAULT_CACHE_PAGES: usize = 256;

/// Frames after which a commit checkpoints the log, matching SQLite's default.
pub const DEFAULT_AUTOCHECKPOINT: u32 = 1000;

impl Pager {
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open(path: &Path) -> Result<Pager> {
        Pager::open_with_cache(path, DEFAULT_CACHE_PAGES)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_with_cache(path: &Path, cache_pages: usize) -> Result<Pager> {
        let stores: Arc<dyn PageStoreSet> = Arc::new(crate::storage::FileStoreSet::new(path));
        Self::open_store_with_cache(stores, cache_pages)
    }

    #[allow(dead_code)]
    pub(crate) fn open_store_with_cache(
        stores: Arc<dyn PageStoreSet>,
        cache_pages: usize,
    ) -> Result<Pager> {
        let file = stores
            .open(StoreKind::Main, StoreOpenOptions::READ_ONLY)?
            .ok_or_else(|| Error::Io(std::io::Error::from(std::io::ErrorKind::NotFound)))?;
        let mut hdr = [0u8; HEADER_SIZE];
        match file.read_at(0, &mut hdr) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(Error::NotADatabase)
            }
            Err(e) => return Err(Error::Io(e)),
        }
        let header = DbHeader::parse(&hdr)?;
        let page_size = header.page_size as usize;
        let usable = header.usable_size();
        let mut pager = Pager {
            path: stores
                .path(StoreKind::Main)
                .unwrap_or_else(|| PathBuf::from(":memory:")),
            stores,
            file,
            header,
            usable,
            page_size,
            page_count: 0,
            wal: None,
            shm: None,
            cache: PageCache::new(cache_pages),
            cache_stamp: None,
            lock: FileLock::new(),
            busy_timeout: Duration::from_secs(5),
            autocheckpoint: DEFAULT_AUTOCHECKPOINT,
            write: None,
        };
        pager.begin_read()?;
        if pager.journal_is_hot()? {
            return Err(Error::Busy(
                "database has a hot journal and must be recovered by a writer".into(),
            ));
        }
        Ok(pager)
    }

    /// A journal is hot only when its writer is gone: nobody holds RESERVED
    /// across processes and no connection in this process is mid-transaction.
    fn journal_is_hot(&mut self) -> Result<bool> {
        if !journal::hot_journal_exists(self.stores.as_ref()) {
            return Ok(false);
        }
        if self.stores.process_lock().is_write_held() {
            return Ok(false);
        }
        Ok(!crate::lock::reserved_lock_held(self.file.as_ref())?)
    }

    /// The identity of the committed snapshot currently visible. Computed
    /// from state `load_snapshot` has already established.
    fn snapshot_stamp(&self) -> SnapshotStamp {
        SnapshotStamp {
            change_counter: self.header.file_change_counter,
            page_count: self.page_count,
            wal: self
                .wal
                .as_ref()
                .map(|w| (w.salt(), w.checkpoint_seq(), w.frames())),
        }
    }

    /// Clear the page cache AND mark it unvouched-for, so the next
    /// `load_snapshot` restamps from scratch. Every clear outside the
    /// stamp check itself must come through here.
    fn invalidate_cache(&mut self) {
        self.cache.clear();
        self.cache_stamp = None;
    }

    fn load_snapshot(&mut self) -> Result<()> {
        let writable = self.write.is_some();
        let wal = if self.in_wal_mode() {
            // Keep the open log and only look at what is new: re-reading and
            // re-checksumming every frame per statement is what made WAL reads
            // slower than the database file they replace.
            match self.wal.take() {
                Some(mut wal) => {
                    wal.refresh()?;
                    Some(wal)
                }
                None => Wal::open_with(self.stores.clone(), self.header.page_size, writable)?,
            }
        } else {
            None
        };
        let file_len = self.file.len()?;
        let file_pages = (file_len / self.page_size as u64) as u32;
        self.page_count = match &wal {
            // A log with committed frames carries the authoritative size.
            Some(w) if w.frames() > 0 => w.db_size_pages(),
            // Otherwise the header count is authoritative only when the change
            // counter matches the version-valid-for field; else the file length
            // wins (the rule SQLite itself applies).
            _ => {
                if self.header.database_size_pages > 0
                    && self.header.file_change_counter == self.header.version_valid_for
                {
                    self.header.database_size_pages
                } else {
                    file_pages
                }
            }
        };
        self.wal = wal;
        // Drop cached pages ONLY when the committed snapshot actually moved:
        // the stamp covers every way it can (rollback-journal commits bump
        // the change counter, WAL commits/resets/checkpoints move the log
        // fields). Clearing unconditionally here made EVERY autocommit
        // statement re-read its b-tree path from disk — the dominant cost of
        // small indexed reads on a live database. Sites that mutate content
        // outside a committed snapshot (rollback, checkpoint, recovery) go
        // through [`Pager::invalidate_cache`], which forces the clear below.
        let stamp = self.snapshot_stamp();
        if self.cache_stamp != Some(stamp) {
            self.cache.clear();
            self.cache_stamp = Some(stamp);
        }
        // Page 1 carries the header, and in WAL mode the newest copy of it is
        // a frame, not the start of the database file. Everything downstream —
        // schema cookie, freelist, user_version — reads it from here.
        if self.wal.as_ref().map(|w| w.frames() > 0).unwrap_or(false) {
            let page1 = self.page(1)?;
            let header = DbHeader::parse(&page1)?;
            if header.page_size == self.header.page_size {
                self.header = header;
                self.usable = self.header.usable_size();
            }
        }
        Ok(())
    }

    /// Re-read the header and the WAL, moving to the newest committed
    /// snapshot. Cheap enough to call between statements on a live database.
    pub fn refresh(&mut self) -> Result<()> {
        let mut hdr = [0u8; HEADER_SIZE];
        self.file.read_at(0, &mut hdr)?;
        let header = DbHeader::parse(&hdr)?;
        if header.page_size != self.header.page_size {
            return Err(Error::corrupt("page size changed under an open reader"));
        }
        self.header = header;
        self.usable = self.header.usable_size();
        self.load_snapshot()
    }

    pub fn header(&self) -> &DbHeader {
        &self.header
    }
    pub fn page_size(&self) -> usize {
        self.page_size
    }
    pub fn usable_size(&self) -> usize {
        self.usable
    }
    pub fn page_count(&self) -> u32 {
        self.page_count
    }
    pub fn text_encoding(&self) -> TextEncoding {
        self.header.text_encoding
    }
    pub fn in_wal_mode(&self) -> bool {
        self.header.write_version == 2 || self.header.read_version == 2
    }
    /// Frames in the current WAL snapshot (0 when there is no WAL).
    pub fn wal_frames(&self) -> u32 {
        self.wal.as_ref().map(|w| w.frames()).unwrap_or(0)
    }
    pub fn wal_salt(&self) -> Option<(u32, u32)> {
        self.wal.as_ref().map(|w| w.salt())
    }
    pub fn wal_checkpoint_seq(&self) -> Option<u32> {
        self.wal.as_ref().map(|w| w.checkpoint_seq())
    }
    /// The journaling mode this file is in, spelled the way `PRAGMA
    /// journal_mode` spells it.
    pub fn journal_mode(&self) -> &'static str {
        if self.in_wal_mode() {
            "wal"
        } else {
            "delete"
        }
    }
    pub fn cache_stats(&self) -> (u64, u64) {
        (self.cache.hits, self.cache.misses)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Fetch one page (1-based), from the WAL snapshot when the log holds a
    /// newer copy, otherwise from the database file.
    pub fn page(&mut self, pgno: u32) -> Result<Arc<[u8]>> {
        if pgno == 0 {
            return Err(Error::corrupt("page number 0 requested"));
        }
        if pgno > self.page_count {
            return Err(Error::corrupt(format!(
                "page {pgno} is past the end of the database ({} pages)",
                self.page_count
            )));
        }
        // Staged pages win over anything on disk or in the read cache.
        if let Some(w) = &self.write {
            if let Some(tx) = &w.tx {
                if let Some(page) = tx.dirty.get(&pgno) {
                    return Ok(page.clone());
                }
            }
        }
        if let Some(p) = self.cache.get(pgno) {
            return Ok(p);
        }
        let mut buf = vec![0u8; self.page_size];
        let from_wal = match &mut self.wal {
            Some(w) => w.read_page(pgno, &mut buf)?,
            None => false,
        };
        if !from_wal {
            let offset = (pgno as u64 - 1) * self.page_size as u64;
            match self.file.read_at(offset, &mut buf) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Err(Error::corrupt(format!(
                        "page {pgno} is missing from the database file"
                    )))
                }
                Err(e) => return Err(Error::Io(e)),
            }
        }
        let page: Arc<[u8]> = Arc::from(buf.into_boxed_slice());
        self.cache.insert(pgno, page.clone());
        Ok(page)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_rejects_garbage() {
        let mut buf = [0u8; HEADER_SIZE];
        assert!(matches!(DbHeader::parse(&buf), Err(Error::NotADatabase)));
        buf[..16].copy_from_slice(MAGIC);
        // page size 0 is invalid
        assert!(DbHeader::parse(&buf).is_err());
    }

    #[test]
    fn allocation_skips_the_one_gib_lock_byte_page() {
        for page_size in [512usize, 4096, 65536] {
            let lock_page = ((1u64 << 30) / page_size as u64 + 1) as u32;
            assert_eq!(
                next_page_number(lock_page - 1, page_size),
                lock_page + 1
            );
            assert_eq!(next_page_number(lock_page - 2, page_size), lock_page - 1);
        }
    }

}

// ---------------------------------------------------------------------------
// Write support
// ---------------------------------------------------------------------------

/// Where a page image comes from while a transaction is open.
impl Pager {
    /// Open a database for reading and writing. A hot journal left by a crashed
    /// writer is rolled back first, so the file is always consistent by the
    /// time this returns.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_rw(path: &Path, busy_timeout: Duration) -> Result<Pager> {
        Pager::open_rw_with_cache(path, busy_timeout, DEFAULT_CACHE_PAGES)
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn open_rw_with_cache(
        path: &Path,
        busy_timeout: Duration,
        cache_pages: usize,
    ) -> Result<Pager> {
        let stores: Arc<dyn PageStoreSet> = Arc::new(crate::storage::FileStoreSet::new(path));
        Self::open_store_rw_with_cache(stores, busy_timeout, cache_pages)
    }

    pub(crate) fn open_store_rw(
        stores: Arc<dyn PageStoreSet>,
        busy_timeout: Duration,
    ) -> Result<Pager> {
        Self::open_store_rw_with_cache(stores, busy_timeout, DEFAULT_CACHE_PAGES)
    }

    pub(crate) fn open_store_rw_with_cache(
        stores: Arc<dyn PageStoreSet>,
        busy_timeout: Duration,
        cache_pages: usize,
    ) -> Result<Pager> {
        let file = stores
            .open(StoreKind::Main, StoreOpenOptions::READ_WRITE)?
            .ok_or_else(|| Error::Io(std::io::Error::from(std::io::ErrorKind::NotFound)))?;
        let process_lock = stores.process_lock();
        let mut pager = Pager {
            path: stores
                .path(StoreKind::Main)
                .unwrap_or_else(|| PathBuf::from(":memory:")),
            stores,
            file,
            header: DbHeader {
                page_size: 4096,
                write_version: 1,
                read_version: 1,
                reserved_space: 0,
                file_change_counter: 0,
                database_size_pages: 0,
                freelist_trunk_page: 0,
                freelist_pages: 0,
                schema_cookie: 0,
                schema_format: 4,
                largest_root_btree_page: 0,
                text_encoding: TextEncoding::Utf8,
                user_version: 0,
                incremental_vacuum: 0,
                application_id: 0,
                version_valid_for: 0,
                sqlite_version_number: 0,
            },
            usable: 4096,
            page_size: 4096,
            page_count: 0,
            wal: None,
            shm: None,
            cache: PageCache::new(cache_pages),
            cache_stamp: None,
            lock: FileLock::new(),
            busy_timeout,
            autocheckpoint: DEFAULT_AUTOCHECKPOINT,
            write: Some(WriteSupport {
                process_lock,
                holds_process_write: false,
                tx: None,
            }),
        };
        pager.recover_hot_journal()?;
        pager.reload_header()?;
        if pager.in_wal_mode() {
            // Take the log for this statement. Without a shared wal-index we
            // must be its only user while we touch it, which is exactly
            // SQLite's exclusive locking mode; the ownership is handed back
            // whenever this connection goes idle, so other processes can use
            // the database in between.
            pager.acquire_wal_ownership()?;
        }
        pager.load_snapshot()?;
        Ok(pager)
    }

    /// Claim the `-shm` locking bytes so no SQLite connection can use this log
    /// while we own it, and make sure a log file exists to append to.
    fn acquire_wal_ownership(&mut self) -> Result<()> {
        if self.shm.is_some() {
            return Ok(());
        }
        let writable = self.write.is_some();
        let deadline = std::time::Instant::now() + self.busy_timeout;
        let guard = loop {
            if let Some(g) = ShmGuard::acquire_with(self.stores.clone(), writable)? {
                break Some(g);
            }
            if !writable {
                // A reader that cannot take the shared slot proceeds without
                // it: frames are checksum-validated either way.
                break None;
            }
            if std::time::Instant::now() >= deadline {
                return Err(Error::Busy(
                    "another connection is using this database's write-ahead log".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        self.shm = guard;
        if !writable {
            return Ok(());
        }
        // A live log handle is kept across statements and refreshed
        // incrementally by `load_snapshot`; probing the file again here would
        // re-read and re-checksum every frame ON EVERY STATEMENT (and, on a
        // database whose log was checkpointed away, re-create the log once
        // per statement, fsync included). Only a connection that has no
        // handle yet needs to find out whether a usable log exists.
        if self.wal.is_none() {
            if Wal::open_with(self.stores.clone(), self.header.page_size, true)?.is_none() {
                // No usable log yet: start one.
                let salt = (journal_nonce(), journal_nonce().rotate_left(7));
                let wal = Wal::create_with(self.stores.clone(), self.header.page_size, 0, salt)?;
                self.wal = Some(wal);
            }
        }
        Ok(())
    }

    /// Switch a rollback-journal database to WAL, or a WAL database back.
    /// Both directions are what `PRAGMA journal_mode=…` does, and both need
    /// exclusive access.
    pub fn set_journal_mode(&mut self, wal: bool) -> Result<&'static str> {
        if self.write.is_none() {
            return Err(Error::unsupported("database opened read-only"));
        }
        if self.in_transaction() {
            return Err(Error::sql("cannot change journal mode inside a transaction"));
        }
        if wal == self.in_wal_mode() {
            return Ok(self.journal_mode());
        }
        {
            let Pager {
                file,
                lock,
                busy_timeout,
                ..
            } = self;
            if !lock.acquire(file.as_ref(), LockLevel::Exclusive, *busy_timeout)? {
                return Err(Error::Busy(
                    "cannot change journal mode while others are using the database".into(),
                ));
            }
        }
        if wal {
            let mut header = self.page(1)?.to_vec();
            header[18] = 2;
            header[19] = 2;
            self.file.write_at(0, &header)?;
            self.file.sync()?;
            self.invalidate_cache();
            self.reload_header()?;
            self.acquire_wal_ownership()?;
            self.load_snapshot()?;
        } else {
            self.checkpoint_wal_into_database()?;
        }
        {
            let Pager { file, lock, .. } = self;
            lock.downgrade_to_shared(file.as_ref())?;
        }
        Ok(self.journal_mode())
    }

    /// Fold every committed frame into the database file, leaving the log
    /// empty. `PRAGMA wal_checkpoint`.
    pub fn checkpoint(&mut self) -> Result<u32> {
        if self.in_transaction() {
            return Err(Error::sql("cannot checkpoint inside a transaction"));
        }
        let Some(wal) = self.wal.as_mut() else {
            return Ok(0);
        };
        let moved = wal.checkpoint(self.file.as_ref())?;
        self.invalidate_cache();
        self.reload_header()?;
        self.load_snapshot()?;
        Ok(moved)
    }

    /// Fold a write-ahead log into the database file and switch the file back
    /// to rollback journaling. Requires exclusive access, exactly like SQLite's
    /// `PRAGMA journal_mode=DELETE`.
    pub fn checkpoint_wal_into_database(&mut self) -> Result<()> {
        {
            let Pager {
                file,
                lock,
                busy_timeout,
                ..
            } = self;
            if !lock.acquire(file.as_ref(), LockLevel::Exclusive, *busy_timeout)? {
                return Err(Error::Busy(
                    "cannot convert a WAL database while others are using it".into(),
                ));
            }
        }
        let pages = self.page_count;
        // Fold the log into the database file.
        if let Some(mut wal) = self.wal.take() {
            wal.checkpoint(self.file.as_ref())?;
            wal.remove()?;
        }
        // Stamp the file as rollback-journal and bump the change counter.
        let mut header = self.page(1)?.to_vec();
        header[18] = 1;
        header[19] = 1;
        let counter = self.header.file_change_counter.wrapping_add(1);
        header[24..28].copy_from_slice(&counter.to_be_bytes());
        header[28..32].copy_from_slice(&pages.to_be_bytes());
        header[92..96].copy_from_slice(&counter.to_be_bytes());
        self.file.write_at(0, &header)?;
        self.file.sync()?;
        // The log is gone; so is any shared-memory index of it.
        self.shm = None;
        let _ = self.stores.remove(StoreKind::Shm);
        self.invalidate_cache();
        self.reload_header()?;
        self.load_snapshot()?;
        {
            let Pager { file, lock, .. } = self;
            lock.downgrade_to_shared(file.as_ref())?;
        }
        Ok(())
    }

    fn reload_header(&mut self) -> Result<()> {
        let mut hdr = [0u8; HEADER_SIZE];
        match self.file.read_at(0, &mut hdr) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                return Err(Error::NotADatabase)
            }
            Err(e) => return Err(Error::Io(e)),
        }
        self.header = DbHeader::parse(&hdr)?;
        self.page_size = self.header.page_size as usize;
        self.usable = self.header.usable_size();
        Ok(())
    }

    /// Roll back a journal left behind by a crashed writer, under an exclusive
    /// lock so no one else is reading the half-written file.
    fn recover_hot_journal(&mut self) -> Result<()> {
        if !journal::hot_journal_exists(self.stores.as_ref()) {
            // An empty or headerless journal file is just litter.
            let _ = self.stores.remove(StoreKind::Journal);
            return Ok(());
        }
        if !self.journal_is_hot()? {
            // Another connection is writing right now; leave its journal alone.
            return Ok(());
        }
        if self.write.is_none() {
            return Err(Error::unsupported(
                "database has a hot journal and was opened read-only",
            ));
        }
        {
            let Pager {
                file,
                lock,
                busy_timeout,
                ..
            } = self;
            if !lock.acquire(file.as_ref(), LockLevel::Exclusive, *busy_timeout)? {
                return Err(Error::Busy(
                    "another process is recovering this database".into(),
                ));
            }
        }
        journal::rollback(self.file.as_ref(), self.stores.as_ref())?;
        self.stores.remove(StoreKind::Journal).ok();
        self.invalidate_cache();
        {
            let Pager { file, lock, .. } = self;
            lock.release(file.as_ref())?;
        }
        Ok(())
    }

    pub fn is_writable(&self) -> bool {
        self.write.is_some()
    }

    pub fn in_transaction(&self) -> bool {
        self.write.as_ref().and_then(|w| w.tx.as_ref()).is_some()
    }

    fn write_support(&mut self) -> Result<&mut WriteSupport> {
        self.write
            .as_mut()
            .ok_or_else(|| Error::unsupported("database opened read-only"))
    }

    /// Take a read lock and move to the newest committed content. Readers of a
    /// rollback-journal database hold SHARED for the whole read transaction.
    pub fn begin_read(&mut self) -> Result<()> {
        if self.in_transaction() {
            // Inside a write transaction the snapshot is our own uncommitted
            // state: re-reading the header here would throw away pages the
            // transaction has already allocated.
            return Ok(());
        }
        if self.in_wal_mode() && self.shm.is_none() {
            self.acquire_wal_ownership()?;
        }
        {
            // Borrow the file and the lock state as separate fields: locking
            // must use the connection's own descriptor, because closing any
            // duplicate of it would drop every lock this process holds on the
            // file (POSIX advisory locking).
            let Pager {
                file,
                lock,
                busy_timeout,
                ..
            } = self;
            if lock.level() < LockLevel::Shared
                && !lock.acquire(file.as_ref(), LockLevel::Shared, *busy_timeout)?
            {
                return Err(Error::Busy("database is locked".into()));
            }
        }
        let counter = self.header.file_change_counter;
        self.reload_header()?;
        if self.header.file_change_counter != counter {
            self.invalidate_cache();
        }
        self.load_snapshot()?;
        Ok(())
    }

    /// Start a write transaction. `immediate` takes the RESERVED lock right
    /// away (`BEGIN IMMEDIATE`); otherwise it is taken on the first write.
    pub fn begin_write(&mut self, immediate: bool) -> Result<()> {
        if self.in_transaction() {
            return Err(Error::sql("a write transaction is already open"));
        }
        self.begin_read()?;
        // Serialize with other connections in this process first: POSIX locks
        // would not see them.
        {
            let timeout = self.busy_timeout;
            let lock = self.write_support()?.process_lock.clone();
            if !lock.acquire_write(timeout) {
                return Err(Error::Busy(
                    "another connection in this process is writing".into(),
                ));
            }
            self.write_support()?.holds_process_write = true;
        }
        // From here on any failure must give the in-process slot back.
        let started = self.start_write_locked(immediate);
        if started.is_err() {
            self.release_process_write();
        }
        started
    }

    fn start_write_locked(&mut self, immediate: bool) -> Result<()> {
        if self.in_wal_mode() {
            // WAL mode: no rollback journal, and no database-file locks — the
            // log is ours (see `acquire_wal_ownership`), and readers of the
            // database file are unaffected until a checkpoint.
            if self.shm.is_none() {
                self.acquire_wal_ownership()?;
            }
            let pages = self.page_count;
            let w = self.write_support()?;
            w.tx = Some(WriteTx {
                journal: None,
                dirty: HashMap::new(),
                initial_pages: pages,
                pages,
                spilled: false,
            });
            return Ok(());
        }
        if immediate {
            if self.write.is_none() {
                return Err(Error::unsupported("database opened read-only"));
            }
            let Pager {
                file,
                lock,
                busy_timeout,
                ..
            } = self;
            if !lock.acquire(file.as_ref(), LockLevel::Reserved, *busy_timeout)? {
                return Err(Error::Busy("another writer holds the database".into()));
            }
        }
        let pages = self.page_count;
        let nonce = journal_nonce();
        // Memory databases deliberately keep rollback-journal mode: the
        // journal is simply another PageStore, so atomic rollback semantics do
        // not require either a filesystem or a WAL.
        let journal = Journal::create_with(self.stores.clone(), self.page_size, pages, nonce)?;
        let w = self.write_support()?;
        w.tx = Some(WriteTx {
            journal: Some(journal),
            dirty: HashMap::new(),
            initial_pages: pages,
            pages,
            spilled: false,
        });
        Ok(())
    }

    /// Stage a modified page image. The original is copied into the journal the
    /// first time a page is touched.
    pub fn write_page(&mut self, pgno: u32, data: &[u8]) -> Result<()> {
        if data.len() != self.page_size {
            return Err(Error::corrupt("staged page has the wrong size"));
        }
        if pgno == 0 {
            return Err(Error::corrupt("cannot write page 0"));
        }
        if !self.in_transaction() {
            return Err(Error::sql("no write transaction is open"));
        }
        // Reserve the write lock lazily for a deferred transaction.
        {
            if self.write.is_none() {
                return Err(Error::unsupported("database opened read-only"));
            }
            let Pager {
                file,
                lock,
                busy_timeout,
                ..
            } = self;
            if lock.level() < LockLevel::Reserved
                && !lock.acquire(file.as_ref(), LockLevel::Reserved, *busy_timeout)?
            {
                return Err(Error::Busy("another writer holds the database".into()));
            }
        }
        // In WAL mode the frames themselves are the record, so nothing is
        // journalled; in rollback mode the original image is saved first.
        let needs_original = {
            let w = self.write_support()?;
            let tx = w.tx.as_ref().expect("transaction");
            match &tx.journal {
                Some(j) => !j.contains(pgno) && pgno <= tx.initial_pages,
                None => false,
            }
        };
        if needs_original {
            let original = self.read_page_from_file(pgno)?;
            let w = self.write_support()?;
            let tx = w.tx.as_mut().expect("transaction");
            if let Some(j) = tx.journal.as_mut() {
                j.record(pgno, &original)?;
            }
        } else {
            let w = self.write_support()?;
            let tx = w.tx.as_mut().expect("transaction");
            if let Some(j) = tx.journal.as_mut() {
                j.record(pgno, &[])?;
            }
        }
        let page: Arc<[u8]> = Arc::from(data.to_vec().into_boxed_slice());
        let w = self.write_support()?;
        let tx = w.tx.as_mut().expect("transaction");
        tx.dirty.insert(pgno, page);
        if pgno > tx.pages {
            tx.pages = pgno;
        }
        if pgno > self.page_count {
            self.page_count = pgno;
        }
        Ok(())
    }

    /// Read a page straight from the database file, bypassing cache and dirty
    /// set: this is the pre-transaction image the journal needs.
    fn read_page_from_file(&mut self, pgno: u32) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; self.page_size];
        let offset = (pgno as u64 - 1) * self.page_size as u64;
        match self.file.read_at(offset, &mut buf) {
            Ok(()) => Ok(buf),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(buf),
            Err(e) => Err(Error::Io(e)),
        }
    }

    /// Current logical size in pages, including growth inside the open
    /// transaction.
    pub fn size_pages(&self) -> u32 {
        self.page_count
    }

    /// Append a fresh page at the end of the file and return its number.
    pub fn append_page(&mut self) -> Result<u32> {
        if !self.in_transaction() {
            return Err(Error::sql("no write transaction is open"));
        }
        let pgno = next_page_number(self.page_count, self.page_size);
        let empty = vec![0u8; self.page_size];
        self.page_count = pgno;
        self.write_page(pgno, &empty)?;
        Ok(pgno)
    }

    /// Commit: journal first, then the database file, then remove the journal.
    /// The removal is the instant the transaction becomes durable.
    pub fn commit(&mut self) -> Result<()> {
        if !self.in_transaction() {
            return Ok(());
        }
        // Nothing to do for a transaction that never wrote a page.
        let empty = {
            let tx = self.write_support()?.tx.as_ref().expect("transaction");
            tx.dirty.is_empty() && !tx.spilled
        };
        if empty {
            {
                let w = self.write_support()?;
                let tx = w.tx.take().expect("transaction");
                if let Some(j) = tx.journal {
                    j.finish()?;
                }
            }
            {
                let Pager { file, lock, .. } = self;
                lock.downgrade_to_shared(file.as_ref())?;
            }
            self.release_process_write();
            return Ok(());
        }

        // Every write transaction re-stamps page 1: the database size and the
        // change counter live there, and other connections use the counter to
        // notice that their cache is stale. SQLite does the same.
        let need_header = {
            let tx = self.write_support()?.tx.as_ref().expect("transaction");
            !tx.dirty.contains_key(&1)
        };
        if need_header {
            let page1 = self.page(1)?.to_vec();
            self.write_page(1, &page1)?;
        }
        // WAL mode has its own commit: append frames and sync the log.
        if self.in_wal_mode() {
            return self.commit_wal();
        }
        // 1. every original image is on disk
        {
            let w = self.write_support()?;
            let tx = w.tx.as_mut().expect("transaction");
            if let Some(j) = tx.journal.as_mut() {
                j.commit_journal()?;
            }
        }
        write_step();
        // 2. exclusive access before the database file is touched
        {
            let Pager {
                file,
                lock,
                busy_timeout,
                ..
            } = self;
            if !lock.acquire(file.as_ref(), LockLevel::Exclusive, *busy_timeout)? {
                return Err(Error::Busy("readers are still using the database".into()));
            }
        }
        // 3. write the pages, header last so the file is never described by a
        //    header that is newer than its content
        let (mut pages, dirty) = {
            let w = self.write_support()?;
            let tx = w.tx.as_mut().expect("transaction");
            (tx.pages, std::mem::take(&mut tx.dirty))
        };
        let mut ordered: Vec<(u32, Arc<[u8]>)> = dirty.into_iter().collect();
        ordered.sort_by_key(|(p, _)| *p);
        let mut header_page: Option<Vec<u8>> = None;
        for (pgno, data) in ordered {
            if pgno == 1 {
                header_page = Some(data.to_vec());
                continue;
            }
            let offset = (pgno as u64 - 1) * self.page_size as u64;
            self.file.write_at(offset, &data)?;
            write_step();
            pages = pages.max(pgno);
        }
        if let Some(mut data) = header_page {
            // Refresh the size, change counter and validity marker.
            let counter = self.header.file_change_counter.wrapping_add(1);
            data[24..28].copy_from_slice(&counter.to_be_bytes());
            data[28..32].copy_from_slice(&pages.to_be_bytes());
            data[92..96].copy_from_slice(&counter.to_be_bytes());
            self.file.write_at(0, &data)?;
            write_step();
            self.header = DbHeader::parse(&data)?;
            let page: Arc<[u8]> = Arc::from(data.into_boxed_slice());
            self.cache.insert(1, page);
        }
        self.file.truncate(pages as u64 * self.page_size as u64)?;
        self.file.sync()?;
        write_step();
        self.invalidate_cache();
        // 4. the commit point
        {
            let w = self.write_support()?;
            let tx = w.tx.take().expect("transaction");
            if let Some(j) = tx.journal {
                j.finish()?;
            }
        }
        write_step();
        self.page_count = pages;
        {
            let Pager { file, lock, .. } = self;
            lock.downgrade_to_shared(file.as_ref())?;
        }
        self.release_process_write();
        Ok(())
    }

    /// Commit in WAL mode: one frame per staged page, the last one carrying
    /// the database size, then a single sync of the log.
    fn commit_wal(&mut self) -> Result<()> {
        let (pages, dirty) = {
            let w = self.write_support()?;
            let tx = w.tx.as_mut().expect("transaction");
            (tx.pages, std::mem::take(&mut tx.dirty))
        };
        let mut ordered: Vec<(u32, Arc<[u8]>)> = dirty.into_iter().collect();
        ordered.sort_by_key(|(p, _)| *p);
        // Page 1 carries the size and change counter, and is always staged.
        let counter = self.header.file_change_counter.wrapping_add(1);
        let mut frames: Vec<(u32, Vec<u8>)> = Vec::with_capacity(ordered.len());
        for (pgno, data) in ordered {
            let mut bytes = data.to_vec();
            if pgno == 1 {
                bytes[24..28].copy_from_slice(&counter.to_be_bytes());
                bytes[28..32].copy_from_slice(&pages.to_be_bytes());
                bytes[92..96].copy_from_slice(&counter.to_be_bytes());
            }
            frames.push((pgno, bytes));
        }
        let last = frames.len().saturating_sub(1);
        {
            let Some(wal) = self.wal.as_mut() else {
                return Err(Error::corrupt("WAL mode without a log"));
            };
            for (i, (pgno, bytes)) in frames.iter().enumerate() {
                let db_size = if i == last { pages } else { 0 };
                wal.append(*pgno, bytes, db_size)?;
            }
            wal.commit(pages)?;
        }
        for (pgno, bytes) in frames {
            if pgno == 1 {
                self.header = DbHeader::parse(&bytes)?;
            }
            let page: Arc<[u8]> = Arc::from(bytes.into_boxed_slice());
            self.cache.insert(pgno, page);
        }
        self.page_count = pages;
        // The cache now IS the new snapshot (committed pages were inserted
        // above): restamp so the next `load_snapshot` keeps it warm.
        self.cache_stamp = Some(self.snapshot_stamp());
        {
            let w = self.write_support()?;
            w.tx = None;
        }
        self.release_process_write();
        // Keep the log from growing without bound, like SQLite's autocheckpoint.
        if self.wal.as_ref().map(|w| w.frames()).unwrap_or(0) >= self.autocheckpoint {
            self.checkpoint()?;
        }
        Ok(())
    }

    fn release_process_write(&mut self) {
        if let Some(w) = self.write.as_mut() {
            if w.holds_process_write {
                w.holds_process_write = false;
                w.process_lock.release_write();
            }
        }
    }

    /// Undo the open transaction. Pages that were only staged in memory are
    /// dropped; anything already spilled to the file is restored from the
    /// journal.
    pub fn rollback(&mut self) -> Result<()> {
        if !self.in_transaction() {
            return Ok(());
        }
        let (has_journal, spilled) = {
            let w = self.write_support()?;
            let tx = w.tx.as_ref().expect("transaction");
            (
                tx.journal.is_some(),
                tx.spilled,
            )
        };
        if has_journal && spilled {
            journal::rollback(self.file.as_ref(), self.stores.as_ref())?;
        }
        {
            let w = self.write_support()?;
            let tx = w.tx.take().expect("transaction");
            if let Some(j) = tx.journal {
                j.finish()?;
            }
        }
        if let Some(wal) = self.wal.as_mut() {
            wal.rollback();
        }
        self.invalidate_cache();
        self.reload_header()?;
        self.load_snapshot()?;
        {
            let Pager { file, lock, .. } = self;
            lock.downgrade_to_shared(file.as_ref())?;
        }
        self.release_process_write();
        Ok(())
    }

    /// Release the byte-range lock; the next read re-acquires SHARED.
    ///
    /// A WRITABLE connection keeps its WAL ownership (the shm guard) for its
    /// whole life: sibling connections coordinate through `refresh`'s
    /// generation checks and the in-process write slot, a store root has
    /// exactly one serving process by law (`server.lock`), and crash safety
    /// comes from the log's write discipline, not from lock courtesy. A
    /// read-only handle still hands its shared slot back between statements,
    /// so it can watch a database some other program is writing.
    pub fn unlock(&mut self) -> Result<()> {
        if self.in_transaction() {
            return Ok(());
        }
        {
            let Pager { file, lock, .. } = self;
            lock.release(file.as_ref())?;
        }
        if self.write.is_none() {
            self.shm = None;
        }
        self.release_process_write();
        Ok(())
    }

    /// Overwrite one field in the database header (page 1) inside the current
    /// transaction. `at` is the documented byte offset.
    pub fn set_header_u32(&mut self, at: usize, value: u32) -> Result<()> {
        let mut page = self.page(1)?.to_vec();
        page[at..at + 4].copy_from_slice(&value.to_be_bytes());
        self.write_page(1, &page)?;
        Ok(())
    }
}

/// SQLite reserves the page containing its 1 GiB lock bytes. Keeping this
/// calculation independent of the backend prevents an in-memory store from
/// accidentally allocating that page just because it has no OS file locks.
fn next_page_number(current: u32, page_size: usize) -> u32 {
    let next = current + 1;
    let lock_page = (1u64 << 30) / page_size as u64 + 1;
    if next as u64 == lock_page {
        next + 1
    } else {
        next
    }
}

/// The journal checksum nonce. It only has to differ between transactions so a
/// stale record cannot pass a fresh checksum; time plus pid gives that without
/// a random source.
fn journal_nonce() -> u32 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() ^ d.as_secs() as u32)
        .unwrap_or(0x5a5a_5a5a);
    t ^ (std::process::id().wrapping_mul(2654435761))
}
