//! The write-ahead log: reading a snapshot, appending frames, and
//! checkpointing back into the database.
//!
//! Layout reference: <https://www.sqlite.org/fileformat.html> section 4 and
//! <https://www.sqlite.org/wal.html>. The frame checksum is the documented
//! Fibonacci-weighted pair, seeded by the header's checksum and carried from
//! frame to frame; its byte order is chosen by the low bit of the magic.
//!
//! This engine runs the log the way SQLite's *exclusive locking mode* does: the
//! index lives in this process's memory and no `-shm` file is shared with
//! anyone. Exclusion is enforced by holding the WAL locking bytes of the `-shm`
//! file, which is exactly where any SQLite connection looks before it may read
//! or write a WAL, so a concurrent `sqlite3` gets SQLITE_BUSY instead of a
//! surprise. Once this process lets go, the log is an ordinary WAL that SQLite
//! recovers on its own.

use crate::error::{Error, Result};
use crate::value::be_u32;
use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const WAL_HEADER_SIZE: usize = 32;
pub const WAL_FRAME_HEADER_SIZE: usize = 24;
const WAL_MAGIC_LE_CKSUM: u32 = 0x377f_0682;
const WAL_MAGIC_BE_CKSUM: u32 = 0x377f_0683;
const WAL_FILE_FORMAT: u32 = 3_007_000;

/// The documented WAL checksum: pairs of 32-bit words, Fibonacci-weighted.
pub fn checksum(buf: &[u8], seed: (u32, u32), big_endian: bool) -> (u32, u32) {
    let (mut s0, mut s1) = seed;
    let mut i = 0;
    while i + 8 <= buf.len() {
        let (a, b) = if big_endian {
            (
                u32::from_be_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]),
                u32::from_be_bytes([buf[i + 4], buf[i + 5], buf[i + 6], buf[i + 7]]),
            )
        } else {
            (
                u32::from_le_bytes([buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]),
                u32::from_le_bytes([buf[i + 4], buf[i + 5], buf[i + 6], buf[i + 7]]),
            )
        };
        s0 = s0.wrapping_add(a.wrapping_add(s1));
        s1 = s1.wrapping_add(b.wrapping_add(s0));
        i += 8;
    }
    (s0, s1)
}

/// The log moved on while this snapshot was in use. Retrying after a
/// `refresh()` picks up the newest committed content; guessing is the one
/// thing a reader must not do.
fn snapshot_gone() -> Error {
    Error::Busy("the write-ahead log was reset while this snapshot was in use".into())
}

pub fn wal_path(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_os_string();
    s.push("-wal");
    PathBuf::from(s)
}

pub fn shm_path(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_os_string();
    s.push("-shm");
    PathBuf::from(s)
}

/// One consistent view of a `-wal` file plus everything needed to append to it.
pub struct Wal {
    file: File,
    path: PathBuf,
    page_size: usize,
    big_endian_cksum: bool,
    checkpoint_seq: u32,
    salt: (u32, u32),
    /// Running checksum after the last accepted frame.
    running: (u32, u32),
    /// page number -> byte offset of that page's newest frame data
    index: HashMap<u32, u64>,
    /// Frames accepted (the snapshot ends at the last commit).
    frames: u32,
    /// Database size in pages after the last commit frame.
    db_size_pages: u32,
    /// Frames written but not yet committed (rolled back on failure).
    pending: Vec<(u32, u64)>,
    pending_frames: u32,
    pending_running: (u32, u32),
    /// File length the last scan covered, so a refresh only reads new frames.
    scanned_len: u64,
}

impl Wal {
    /// Open an existing log and validate it up to the last commit. Returns
    /// `Ok(None)` when there is nothing usable: absent, empty, a foreign
    /// format, or a header that fails its own checksum.
    pub fn open(db_path: &Path, page_size: u32, writable: bool) -> Result<Option<Wal>> {
        let path = wal_path(db_path);
        let mut file = match File::options()
            .read(true)
            .write(writable)
            .open(&path)
        {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(Error::Io(e)),
        };
        let len = file.metadata()?.len();
        if len < WAL_HEADER_SIZE as u64 {
            return Ok(None);
        }
        let mut hdr = [0u8; WAL_HEADER_SIZE];
        file.seek(SeekFrom::Start(0))?;
        file.read_exact(&mut hdr)?;
        let magic = be_u32(&hdr, 0)?;
        let big_endian_cksum = match magic {
            WAL_MAGIC_LE_CKSUM => false,
            WAL_MAGIC_BE_CKSUM => true,
            _ => return Ok(None),
        };
        let format = be_u32(&hdr, 4)?;
        if format != WAL_FILE_FORMAT {
            return Err(Error::unsupported(format!("WAL file format {format}")));
        }
        let wal_page_size = match be_u32(&hdr, 8)? {
            1 => 65536,
            n => n,
        };
        if wal_page_size != page_size {
            return Err(Error::corrupt(
                "WAL page size differs from the database page size",
            ));
        }
        let checkpoint_seq = be_u32(&hdr, 12)?;
        let salt = (be_u32(&hdr, 16)?, be_u32(&hdr, 20)?);
        let want = (be_u32(&hdr, 24)?, be_u32(&hdr, 28)?);
        if checksum(&hdr[..24], (0, 0), big_endian_cksum) != want {
            return Ok(None);
        }

        let mut wal = Wal {
            file,
            path,
            page_size: page_size as usize,
            big_endian_cksum,
            checkpoint_seq,
            salt,
            running: want,
            index: HashMap::new(),
            frames: 0,
            db_size_pages: 0,
            pending: Vec::new(),
            pending_frames: 0,
            pending_running: want,
            scanned_len: 0,
        };
        wal.scan(len)?;
        if wal.frames == 0 {
            // A header with no committed frame behind it: the database file is
            // the whole story, but the log is still ours to append to.
            return Ok(Some(wal));
        }
        Ok(Some(wal))
    }

    /// Create a fresh log for `db_path` (or overwrite an unusable one).
    pub fn create(db_path: &Path, page_size: u32, seq: u32, salt: (u32, u32)) -> Result<Wal> {
        let path = wal_path(db_path);
        let file = File::options()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        let mut wal = Wal {
            file,
            path,
            page_size: page_size as usize,
            big_endian_cksum: false,
            checkpoint_seq: seq,
            salt,
            running: (0, 0),
            index: HashMap::new(),
            frames: 0,
            db_size_pages: 0,
            pending: Vec::new(),
            pending_frames: 0,
            pending_running: (0, 0),
            scanned_len: 0,
        };
        wal.write_header()?;
        Ok(wal)
    }

    fn write_header(&mut self) -> Result<()> {
        let mut hdr = [0u8; WAL_HEADER_SIZE];
        hdr[0..4].copy_from_slice(&WAL_MAGIC_LE_CKSUM.to_be_bytes());
        hdr[4..8].copy_from_slice(&WAL_FILE_FORMAT.to_be_bytes());
        let ps = if self.page_size == 65536 {
            1u32
        } else {
            self.page_size as u32
        };
        hdr[8..12].copy_from_slice(&ps.to_be_bytes());
        hdr[12..16].copy_from_slice(&self.checkpoint_seq.to_be_bytes());
        hdr[16..20].copy_from_slice(&self.salt.0.to_be_bytes());
        hdr[20..24].copy_from_slice(&self.salt.1.to_be_bytes());
        let cks = checksum(&hdr[..24], (0, 0), self.big_endian_cksum);
        hdr[24..28].copy_from_slice(&cks.0.to_be_bytes());
        hdr[28..32].copy_from_slice(&cks.1.to_be_bytes());
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&hdr)?;
        crate::sync::sync(&self.file)?;
        self.running = cks;
        self.pending_running = cks;
        self.frames = 0;
        self.pending_frames = 0;
        self.index.clear();
        self.pending.clear();
        self.db_size_pages = 0;
        Ok(())
    }

    /// Walk the frames, accepting only those whose salts and checksums hold,
    /// and keep the state as of the last commit frame.
    fn scan(&mut self, len: u64) -> Result<()> {
        let frame_size = WAL_FRAME_HEADER_SIZE as u64 + self.page_size as u64;
        let mut offset = WAL_HEADER_SIZE as u64;
        let mut running = self.running;
        let mut index: HashMap<u32, u64> = HashMap::new();
        let mut committed_index: HashMap<u32, u64> = HashMap::new();
        let mut frames = 0u32;
        let mut committed_frames = 0u32;
        let mut committed_running = running;
        let mut db_size = 0u32;
        let mut frame_hdr = [0u8; WAL_FRAME_HEADER_SIZE];
        let mut page = vec![0u8; self.page_size];
        while offset + frame_size <= len {
            self.file.seek(SeekFrom::Start(offset))?;
            self.file.read_exact(&mut frame_hdr)?;
            self.file.read_exact(&mut page)?;
            let pgno = be_u32(&frame_hdr, 0)?;
            let truncate = be_u32(&frame_hdr, 4)?;
            let fsalt = (be_u32(&frame_hdr, 8)?, be_u32(&frame_hdr, 12)?);
            let fcksum = (be_u32(&frame_hdr, 16)?, be_u32(&frame_hdr, 20)?);
            if fsalt != self.salt || pgno == 0 {
                break;
            }
            let s = checksum(&frame_hdr[..8], running, self.big_endian_cksum);
            let s = checksum(&page, s, self.big_endian_cksum);
            if s != fcksum {
                break;
            }
            running = s;
            frames += 1;
            index.insert(pgno, offset + WAL_FRAME_HEADER_SIZE as u64);
            if truncate != 0 {
                committed_index = index.clone();
                committed_frames = frames;
                committed_running = running;
                db_size = truncate;
            }
            offset += frame_size;
        }
        self.index = committed_index;
        self.frames = committed_frames;
        self.running = committed_running;
        self.pending_running = committed_running;
        self.db_size_pages = db_size;
        self.scanned_len = len;
        Ok(())
    }

    /// Pick up whatever happened to the log since the last look: frames another
    /// connection appended, or a checkpoint that reset it. The header is always
    /// re-read, because a reset keeps the file long but changes the salts, and
    /// trusting the length alone would leave us reading a previous generation.
    pub fn refresh(&mut self) -> Result<()> {
        let len = self.file.metadata()?.len();
        if len < WAL_HEADER_SIZE as u64 {
            self.index.clear();
            self.frames = 0;
            self.db_size_pages = 0;
            self.pending.clear();
            self.pending_frames = 0;
            self.scanned_len = len;
            return Ok(());
        }
        let mut hdr = [0u8; WAL_HEADER_SIZE];
        self.file.seek(SeekFrom::Start(0))?;
        self.file.read_exact(&mut hdr)?;
        let salt = (be_u32(&hdr, 16)?, be_u32(&hdr, 20)?);
        let seq = be_u32(&hdr, 12)?;
        let generation_changed = salt != self.salt || seq != self.checkpoint_seq;
        // The length is only a safe "nothing happened" signal when the last
        // scan ended on a commit. A transaction that rolled back leaves its
        // frames in the file, and the next transaction writes over exactly
        // those offsets: same salts, same length, entirely different content.
        // Trusting the length there leaves the reader parked on the snapshot
        // before the rollback for as long as the log does not grow.
        let frame_size = WAL_FRAME_HEADER_SIZE as u64 + self.page_size as u64;
        let committed_len = WAL_HEADER_SIZE as u64 + self.frames as u64 * frame_size;
        if !generation_changed && len == self.scanned_len && self.scanned_len == committed_len {
            return Ok(());
        }
        if generation_changed {
            self.salt = salt;
            self.checkpoint_seq = seq;
            self.big_endian_cksum = be_u32(&hdr, 0)? == WAL_MAGIC_BE_CKSUM;
            self.running = (be_u32(&hdr, 24)?, be_u32(&hdr, 28)?);
            self.index.clear();
            self.frames = 0;
            self.db_size_pages = 0;
            self.pending.clear();
            self.pending_frames = 0;
            self.pending_running = self.running;
            return self.scan(len);
        }
        // Same generation, more frames: continue the checksum chain.
        self.scan_from(len)
    }

    /// Continue scanning after `self.frames`, keeping what is already known.
    fn scan_from(&mut self, len: u64) -> Result<()> {
        let frame_size = WAL_FRAME_HEADER_SIZE as u64 + self.page_size as u64;
        let mut offset = WAL_HEADER_SIZE as u64 + self.frames as u64 * frame_size;
        let mut running = self.running;
        let mut index = self.index.clone();
        let mut frames = self.frames;
        let mut committed_index = self.index.clone();
        let mut committed_frames = self.frames;
        let mut committed_running = running;
        let mut db_size = self.db_size_pages;
        let mut frame_hdr = [0u8; WAL_FRAME_HEADER_SIZE];
        let mut page = vec![0u8; self.page_size];
        while offset + frame_size <= len {
            self.file.seek(SeekFrom::Start(offset))?;
            self.file.read_exact(&mut frame_hdr)?;
            self.file.read_exact(&mut page)?;
            let pgno = be_u32(&frame_hdr, 0)?;
            let truncate = be_u32(&frame_hdr, 4)?;
            let fsalt = (be_u32(&frame_hdr, 8)?, be_u32(&frame_hdr, 12)?);
            let fcksum = (be_u32(&frame_hdr, 16)?, be_u32(&frame_hdr, 20)?);
            if fsalt != self.salt || pgno == 0 {
                break;
            }
            let s = checksum(&frame_hdr[..8], running, self.big_endian_cksum);
            let s = checksum(&page, s, self.big_endian_cksum);
            if s != fcksum {
                break;
            }
            running = s;
            frames += 1;
            index.insert(pgno, offset + WAL_FRAME_HEADER_SIZE as u64);
            if truncate != 0 {
                committed_index = index.clone();
                committed_frames = frames;
                committed_running = running;
                db_size = truncate;
            }
            offset += frame_size;
        }
        self.index = committed_index;
        self.frames = committed_frames;
        self.running = committed_running;
        self.pending_running = committed_running;
        self.db_size_pages = db_size;
        self.scanned_len = len;
        Ok(())
    }

    pub fn frames(&self) -> u32 {
        self.frames
    }
    pub fn db_size_pages(&self) -> u32 {
        self.db_size_pages
    }
    pub fn salt(&self) -> (u32, u32) {
        self.salt
    }
    pub fn checkpoint_seq(&self) -> u32 {
        self.checkpoint_seq
    }
    pub fn contains(&self, pgno: u32) -> bool {
        self.index.contains_key(&pgno)
            || self.pending.iter().any(|(p, _)| *p == pgno)
    }
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read a page from the log; `false` when the log does not hold it.
    pub fn read_page(&mut self, pgno: u32, out: &mut [u8]) -> Result<bool> {
        // A page written earlier in this transaction wins over the snapshot.
        let pending = self
            .pending
            .iter()
            .rev()
            .find(|(p, _)| *p == pgno)
            .map(|(_, off)| *off);
        let offset = match pending.or_else(|| self.index.get(&pgno).copied()) {
            Some(o) => o,
            None => return Ok(false),
        };
        // An offset only means anything within the generation it was scanned
        // in. A checkpoint truncates the log, bumps the salts and starts
        // writing fresh frames over exactly these offsets, so the frame header
        // is re-read and matched before its page is handed out: without it a
        // reader whose snapshot has been checkpointed away silently returns
        // the content of whatever page now sits there — a different b-tree's
        // page, decoded as this one.
        let header_at = offset.saturating_sub(WAL_FRAME_HEADER_SIZE as u64);
        let mut hdr = [0u8; WAL_FRAME_HEADER_SIZE];
        self.file.seek(SeekFrom::Start(header_at))?;
        match self.file.read_exact(&mut hdr) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(snapshot_gone()),
            Err(e) => return Err(Error::Io(e)),
        }
        if be_u32(&hdr, 0)? != pgno
            || (be_u32(&hdr, 8)?, be_u32(&hdr, 12)?) != self.salt
            || header_at + WAL_FRAME_HEADER_SIZE as u64 != offset
        {
            return Err(snapshot_gone());
        }
        // The header read left the cursor exactly on the frame's page data.
        match self.file.read_exact(out) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(snapshot_gone()),
            Err(e) => return Err(Error::Io(e)),
        }
        Ok(true)
    }

    /// Append one frame. `db_size` is non-zero only on the frame that commits.
    pub fn append(&mut self, pgno: u32, data: &[u8], db_size: u32) -> Result<()> {
        if data.len() != self.page_size {
            return Err(Error::corrupt("WAL frame page has the wrong size"));
        }
        let frame_size = WAL_FRAME_HEADER_SIZE as u64 + self.page_size as u64;
        let offset =
            WAL_HEADER_SIZE as u64 + (self.frames + self.pending_frames) as u64 * frame_size;
        let mut hdr = [0u8; WAL_FRAME_HEADER_SIZE];
        hdr[0..4].copy_from_slice(&pgno.to_be_bytes());
        hdr[4..8].copy_from_slice(&db_size.to_be_bytes());
        hdr[8..12].copy_from_slice(&self.salt.0.to_be_bytes());
        hdr[12..16].copy_from_slice(&self.salt.1.to_be_bytes());
        let s = checksum(&hdr[..8], self.pending_running, self.big_endian_cksum);
        let s = checksum(data, s, self.big_endian_cksum);
        hdr[16..20].copy_from_slice(&s.0.to_be_bytes());
        hdr[20..24].copy_from_slice(&s.1.to_be_bytes());
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(&hdr)?;
        self.file.write_all(data)?;
        self.pending_running = s;
        self.pending_frames += 1;
        self.pending
            .push((pgno, offset + WAL_FRAME_HEADER_SIZE as u64));
        crate::pager::journal_write_step();
        Ok(())
    }

    /// Make every appended frame durable and part of the snapshot. The caller
    /// has already written the commit frame (the one carrying `db_size`).
    pub fn commit(&mut self, db_size: u32) -> Result<()> {
        crate::sync::sync(&self.file)?;
        for (pgno, off) in std::mem::take(&mut self.pending) {
            self.index.insert(pgno, off);
        }
        self.frames += self.pending_frames;
        self.pending_frames = 0;
        self.running = self.pending_running;
        self.db_size_pages = db_size;
        self.scanned_len = self.file.metadata()?.len();
        crate::pager::journal_write_step();
        Ok(())
    }

    /// Forget frames written since the last commit. They stay in the file but
    /// their checksum chain no longer matches, so no reader will accept them —
    /// the same way a crash mid-transaction leaves them.
    pub fn rollback(&mut self) {
        self.pending.clear();
        self.pending_frames = 0;
        self.pending_running = self.running;
    }

    /// Fold every committed frame into the database file and start the log
    /// over. The caller must hold exclusive access.
    pub fn checkpoint(&mut self, db: &mut File) -> Result<u32> {
        if self.frames == 0 {
            return Ok(0);
        }
        let mut pages: Vec<(u32, u64)> = self.index.iter().map(|(p, o)| (*p, *o)).collect();
        pages.sort_unstable();
        let mut buf = vec![0u8; self.page_size];
        for (pgno, offset) in &pages {
            self.file.seek(SeekFrom::Start(*offset))?;
            self.file.read_exact(&mut buf)?;
            db.seek(SeekFrom::Start((*pgno as u64 - 1) * self.page_size as u64))?;
            db.write_all(&buf)?;
        }
        if self.db_size_pages > 0 {
            db.set_len(self.db_size_pages as u64 * self.page_size as u64)?;
        }
        crate::sync::sync(db)?;
        let moved = pages.len() as u32;
        // Reset: a new generation so any stale frame fails the salt check.
        self.checkpoint_seq = self.checkpoint_seq.wrapping_add(1);
        self.salt.0 = self.salt.0.wrapping_add(1);
        self.salt.1 = self.salt.1.wrapping_mul(2_654_435_761).wrapping_add(1);
        self.file.set_len(0)?;
        self.write_header()?;
        Ok(moved)
    }

    /// Remove the log entirely (after a checkpoint, when closing a database
    /// that no one else is using).
    pub fn remove(self) -> Result<()> {
        let path = self.path.clone();
        drop(self.file);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(Error::Io(e)),
        }
    }
}

// ---------------------------------------------------------------------------
// Exclusion through the -shm locking bytes
// ---------------------------------------------------------------------------

/// SQLite's WAL locking slots live in the first 128 bytes of the `-shm` file:
/// one write lock, one checkpoint lock, one recover lock and five read marks.
/// Holding all of them keeps every SQLite connection out of a log we are
/// managing without a shared wal-index.
pub const SHM_LOCK_FIRST: u64 = 120;
pub const SHM_LOCK_COUNT: u64 = 8;

/// Keeps the `-shm` locking bytes for as long as this engine owns the log.
pub struct ShmGuard {
    file: File,
    #[allow(dead_code)]
    path: PathBuf,
    held: bool,
    exclusive: bool,
}

impl ShmGuard {
    /// Try to take the WAL locking bytes: shared for a reader (several may
    /// hold them), exclusive for a writer. `Ok(None)` means another process is
    /// using the log right now.
    pub fn acquire(db_path: &Path, exclusive: bool) -> Result<Option<ShmGuard>> {
        let path = shm_path(db_path);
        let file = match File::options()
            .create(true)
            .read(true)
            .write(true)
            .open(&path)
        {
            Ok(f) => f,
            // A read-only directory or file: fall back to reading the log
            // without the lock, which is still checksum-safe.
            Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => return Ok(None),
            Err(e) => return Err(Error::Io(e)),
        };
        let got = crate::lock::try_lock_range(&file, SHM_LOCK_FIRST, SHM_LOCK_COUNT, exclusive)?;
        if !got {
            return Ok(None);
        }
        Ok(Some(ShmGuard {
            file,
            path,
            held: true,
            exclusive,
        }))
    }

    /// Give the log back. The wal-index header is zeroed first: SQLite keeps
    /// two checksummed copies of it and re-reads them at the start of every
    /// read transaction, so an invalid header is its documented signal to
    /// rebuild the index from the log — which is how a SQLite connection picks
    /// up the frames this engine appended while it was locked out.
    pub fn release(&mut self) {
        if !self.held {
            return;
        }
        // Only a writer may have added frames, so only a writer invalidates.
        if self.exclusive {
            let _ = self.invalidate_index();
        }
        let _ = crate::lock::unlock_range(&self.file, SHM_LOCK_FIRST, SHM_LOCK_COUNT);
        self.held = false;
    }

    fn invalidate_index(&mut self) -> std::io::Result<()> {
        // The two header copies live in the first 96 bytes; the locking bytes
        // start at 120 and are never touched here.
        let zeros = [0u8; 96];
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&zeros)?;
        self.file.sync_data()?;
        Ok(())
    }
}

impl Drop for ShmGuard {
    fn drop(&mut self) {
        self.release();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_is_order_and_endian_sensitive() {
        let a = checksum(&[1, 2, 3, 4, 5, 6, 7, 8], (0, 0), false);
        let b = checksum(&[1, 2, 3, 4, 5, 6, 7, 8], (0, 0), true);
        assert_ne!(a, b);
        let c = checksum(&[8, 7, 6, 5, 4, 3, 2, 1], (0, 0), false);
        assert_ne!(a, c);
        // The seed carries into the result, which is what chains frames.
        assert_ne!(
            checksum(&[0; 8], (1, 2), false),
            checksum(&[0; 8], (2, 1), false)
        );
    }
}
