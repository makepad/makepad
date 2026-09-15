//! The rollback journal: original page images written aside before a page is
//! modified, so a crash mid-commit is undone by whoever opens the file next —
//! this engine or the `sqlite3` CLI, which understand the same format.
//!
//! Layout reference: <https://www.sqlite.org/fileformat.html> section 5
//! ("The Rollback Journal").

use crate::error::{Error, Result};
use crate::storage::{PageStore, PageStoreSet, StoreKind, StoreOpenOptions};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub const JOURNAL_MAGIC: [u8; 8] = [0xd9, 0xd5, 0x05, 0xf9, 0x20, 0xa1, 0x63, 0xd7];
/// SQLite assumes 512-byte sectors unless the VFS says otherwise; the journal
/// header is padded to one sector so a record never shares a sector with it.
pub const SECTOR_SIZE: usize = 512;
/// "Record count unknown": recovery then reads records until the file ends.
const NREC_UNKNOWN: u32 = 0xffff_ffff;

/// The documented journal checksum: the nonce plus every 200th byte of the page
/// counting back from its end.
fn page_checksum(init: u32, data: &[u8]) -> u32 {
    let mut cksum = init;
    let mut i = data.len().saturating_sub(200);
    loop {
        if i == 0 {
            break;
        }
        cksum = cksum.wrapping_add(data[i] as u32);
        if i < 200 {
            break;
        }
        i -= 200;
    }
    cksum
}

pub fn journal_path(db_path: &Path) -> PathBuf {
    let mut s = db_path.as_os_str().to_os_string();
    s.push("-journal");
    PathBuf::from(s)
}

/// A journal being written for the current transaction.
pub struct Journal {
    file: Arc<dyn PageStore>,
    stores: Arc<dyn PageStoreSet>,
    path: PathBuf,
    page_size: usize,
    cksum_init: u32,
    records: u32,
    initial_pages: u32,
    journaled: HashSet<u32>,
    /// Every record written has reached the disk.
    synced: bool,
}

impl Journal {
    /// Create (or truncate) the journal for `db_path` and write its header.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn create(db_path: &Path, page_size: usize, initial_pages: u32, nonce: u32) -> Result<Journal> {
        let stores: Arc<dyn PageStoreSet> = Arc::new(crate::storage::FileStoreSet::new(db_path));
        Self::create_with(stores, page_size, initial_pages, nonce)
    }

    pub(crate) fn create_with(
        stores: Arc<dyn PageStoreSet>,
        page_size: usize,
        initial_pages: u32,
        nonce: u32,
    ) -> Result<Journal> {
        let path = stores
            .path(StoreKind::Journal)
            .unwrap_or_else(|| PathBuf::from(":memory:-journal"));
        let file = stores
            .open(StoreKind::Journal, StoreOpenOptions::CREATE_TRUNCATE)?
            .expect("create always returns a store");
        let mut j = Journal {
            file,
            stores,
            path,
            page_size,
            cksum_init: nonce,
            records: 0,
            initial_pages,
            journaled: HashSet::new(),
            synced: false,
        };
        j.write_header(NREC_UNKNOWN)?;
        Ok(j)
    }

    fn write_header(&mut self, nrec: u32) -> Result<()> {
        let mut hdr = vec![0u8; SECTOR_SIZE];
        hdr[0..8].copy_from_slice(&JOURNAL_MAGIC);
        hdr[8..12].copy_from_slice(&nrec.to_be_bytes());
        hdr[12..16].copy_from_slice(&self.cksum_init.to_be_bytes());
        hdr[16..20].copy_from_slice(&self.initial_pages.to_be_bytes());
        hdr[20..24].copy_from_slice(&(SECTOR_SIZE as u32).to_be_bytes());
        hdr[24..28].copy_from_slice(&(self.page_size as u32).to_be_bytes());
        self.file.write_at(0, &hdr)?;
        Ok(())
    }

    pub fn contains(&self, pgno: u32) -> bool {
        self.journaled.contains(&pgno)
    }

    /// Save a page's pre-modification image. Pages beyond the initial database
    /// size need no image: rollback truncates them away.
    pub fn record(&mut self, pgno: u32, data: &[u8]) -> Result<()> {
        if pgno > self.initial_pages || self.journaled.contains(&pgno) {
            self.journaled.insert(pgno);
            return Ok(());
        }
        if data.len() != self.page_size {
            return Err(Error::corrupt("journalled page has the wrong size"));
        }
        let offset = SECTOR_SIZE as u64 + self.records as u64 * (self.page_size as u64 + 8);
        self.file.write_at(offset, &pgno.to_be_bytes())?;
        self.file.write_at(offset + 4, data)?;
        let cksum = page_checksum(self.cksum_init, data);
        self.file
            .write_at(offset + 4 + self.page_size as u64, &cksum.to_be_bytes())?;
        self.records += 1;
        self.journaled.insert(pgno);
        self.synced = false;
        crate::pager::journal_write_step();
        Ok(())
    }

    /// Flush every record and stamp the record count: after this returns, the
    /// journal alone can undo the whole transaction.
    pub fn commit_journal(&mut self) -> Result<()> {
        self.file.sync()?;
        self.write_header(self.records)?;
        self.file.sync()?;
        self.synced = true;
        Ok(())
    }

    pub fn records(&self) -> u32 {
        self.records
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Remove the journal, which is what makes a transaction committed.
    pub fn finish(self) -> Result<()> {
        let Journal { file, stores, .. } = self;
        drop(file);
        stores.remove(StoreKind::Journal).map_err(Error::Io)
    }
}

/// What a journal file says about the transaction it can undo.
#[derive(Debug, Clone, Copy)]
pub struct JournalHeader {
    pub records: u32,
    pub cksum_init: u32,
    pub initial_pages: u32,
    pub sector_size: u32,
    pub page_size: u32,
}

pub fn read_header(file: &dyn PageStore) -> Result<Option<JournalHeader>> {
    let len = file.len()?;
    if len < SECTOR_SIZE as u64 {
        return Ok(None);
    }
    let mut hdr = [0u8; 28];
    file.read_at(0, &mut hdr)?;
    if hdr[0..8] != JOURNAL_MAGIC {
        return Ok(None);
    }
    let get = |at: usize| u32::from_be_bytes([hdr[at], hdr[at + 1], hdr[at + 2], hdr[at + 3]]);
    let page_size = get(24);
    if page_size < 512 || !page_size.is_power_of_two() {
        return Ok(None);
    }
    Ok(Some(JournalHeader {
        records: get(8),
        cksum_init: get(12),
        initial_pages: get(16),
        sector_size: get(20).max(SECTOR_SIZE as u32),
        page_size,
    }))
}

/// Undo a transaction: write every valid page image back into the database and
/// truncate it to the size it had when the journal was created.
///
/// Returns the number of pages restored. A journal whose header is missing or
/// whose records are torn restores what it can and stops, exactly like SQLite:
/// a record only counts once its checksum matches.
pub fn rollback(db: &dyn PageStore, stores: &dyn PageStoreSet) -> Result<u32> {
    let jf = match stores.open(StoreKind::Journal, StoreOpenOptions::READ_ONLY)? {
        Some(file) => file,
        None => return Ok(0),
    };
    let Some(hdr) = read_header(jf.as_ref())? else {
        return Ok(0);
    };
    let page_size = hdr.page_size as usize;
    let record_size = page_size as u64 + 8;
    let jlen = jf.len()?;
    let start = hdr.sector_size as u64;
    let available = jlen.saturating_sub(start) / record_size;
    let want = if hdr.records == NREC_UNKNOWN {
        available
    } else {
        (hdr.records as u64).min(available)
    };

    let mut restored = 0u32;
    let mut buf = vec![0u8; page_size];
    let mut num = [0u8; 4];
    let mut cks = [0u8; 4];
    for i in 0..want {
        let offset = start + i * record_size;
        if jf.read_at(offset, &mut num).is_err() {
            break;
        }
        if jf.read_at(offset + 4, &mut buf).is_err() {
            break;
        }
        if jf.read_at(offset + 4 + page_size as u64, &mut cks).is_err() {
            break;
        }
        let pgno = u32::from_be_bytes(num);
        let want_cksum = u32::from_be_bytes(cks);
        if pgno == 0 || page_checksum(hdr.cksum_init, &buf) != want_cksum {
            break; // torn record: everything after it is untrustworthy
        }
        db.write_at((pgno as u64 - 1) * page_size as u64, &buf)?;
        restored += 1;
    }
    // Pages appended by the interrupted transaction go away.
    db.truncate(hdr.initial_pages as u64 * page_size as u64)?;
    db.sync()?;
    Ok(restored)
}

/// True when `db_path` has a journal that must be rolled back before the file
/// can be read. Callers hold at least a SHARED lock while asking.
pub fn hot_journal_exists(stores: &dyn PageStoreSet) -> bool {
    stores
        .open(StoreKind::Journal, StoreOpenOptions::READ_ONLY)
        .ok()
        .flatten()
        .and_then(|file| read_header(file.as_ref()).ok().flatten())
        .is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn scratch(tag: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "makepad-sqlite-journal-{tag}-{}-{nonce}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn rollback_restores_the_original_pages() {
        let dir = scratch("roll");
        let db_path = dir.join("t.db");
        let page_size = 512usize;
        let original: Vec<u8> = (0..page_size * 3).map(|i| (i % 251) as u8).collect();
        let sets: Vec<Arc<dyn PageStoreSet>> = vec![
            Arc::new(crate::storage::FileStoreSet::new(&db_path)),
            Arc::new(crate::storage::MemoryStoreSet::new()),
        ];
        for stores in sets {
            let db = stores
                .open(StoreKind::Main, StoreOpenOptions::CREATE_TRUNCATE)
                .unwrap()
                .unwrap();
            db.write_at(0, &original).unwrap();
            let mut journal =
                Journal::create_with(stores.clone(), page_size, 3, 0x1234_5678).unwrap();
            journal.record(2, &original[page_size..page_size * 2]).unwrap();
            journal.commit_journal().unwrap();

            // Scribble over page 2 and append a page, as an interrupted commit would.
            db.write_at(page_size as u64, &vec![0xAA; page_size]).unwrap();
            db.write_at((page_size * 3) as u64, &vec![0xBB; page_size]).unwrap();
            drop(journal);

            let restored = rollback(db.as_ref(), stores.as_ref()).unwrap();
            assert_eq!(restored, 1);
            let mut actual = vec![0; original.len()];
            db.read_at(0, &mut actual).unwrap();
            assert_eq!(actual, original);
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn torn_records_stop_the_replay() {
        let dir = scratch("torn");
        let db_path = dir.join("t.db");
        let page_size = 512usize;
        let original: Vec<u8> = vec![7u8; page_size * 2];
        let sets: Vec<Arc<dyn PageStoreSet>> = vec![
            Arc::new(crate::storage::FileStoreSet::new(&db_path)),
            Arc::new(crate::storage::MemoryStoreSet::new()),
        ];
        for stores in sets {
            let db = stores
                .open(StoreKind::Main, StoreOpenOptions::CREATE_TRUNCATE)
                .unwrap()
                .unwrap();
            db.write_at(0, &original).unwrap();
            let mut journal = Journal::create_with(stores.clone(), page_size, 2, 99).unwrap();
            journal.record(1, &original[..page_size]).unwrap();
            journal.record(2, &original[page_size..]).unwrap();
            journal.commit_journal().unwrap();
            drop(journal);

            // Damage the second record's checksum.
            let jf = stores
                .open(StoreKind::Journal, StoreOpenOptions::READ_WRITE)
                .unwrap()
                .unwrap();
            let at = SECTOR_SIZE + (page_size + 8) * 2 - 1;
            let mut byte = [0];
            jf.read_at(at as u64, &mut byte).unwrap();
            byte[0] ^= 0xff;
            jf.write_at(at as u64, &byte).unwrap();

            db.write_at(0, &vec![0u8; page_size * 2]).unwrap();
            let restored = rollback(db.as_ref(), stores.as_ref()).unwrap();
            assert_eq!(restored, 1, "replay must stop at the torn record");
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn checksum_depends_on_content_and_nonce() {
        let a = vec![1u8; 1024];
        let mut b = a.clone();
        b[1024 - 200] ^= 0xff;
        assert_ne!(page_checksum(5, &a), page_checksum(5, &b));
        assert_ne!(page_checksum(5, &a), page_checksum(6, &a));
    }
}
