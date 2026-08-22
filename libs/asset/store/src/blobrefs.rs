//! Reference blobs: content the store CATALOGUES but does not COPY.
//!
//! A normal blob lives in the CAS: the bytes were streamed in, hashed on the
//! way, and committed under their own digest. A *reference* blob is the same
//! catalogue row with the bytes left where the operator already had them —
//! `/Volumes/clips/set-2024/opener.mp4` stays exactly there, and this table
//! records the path, the size and the digest that was computed by reading
//! that file in place.
//!
//! It exists for one shape of content: directories of video that are tens or
//! hundreds of gigabytes, that the user already curates on their own disk,
//! and that nobody wants a second copy of. Thumbnails, manifests and search
//! rows for such an asset are ordinary owned blobs — small, derived, and the
//! store's own responsibility. Only the heavy original is referenced.
//!
//! ## What a reference costs you, stated plainly
//!
//! The store no longer owns the bytes, so it can no longer promise they are
//! there. Every read therefore RE-VERIFIES:
//!
//! - the file must still exist at the recorded path,
//! - its length must still equal the recorded size,
//! - and its SHA-256 must still equal the digest that names the blob.
//!
//! Any of those failing is a refusal with a distinct reason — never a
//! truncated read, never a silent substitution of whatever is at that path
//! today. That is the whole safety argument: a reference blob can become
//! UNAVAILABLE, and it can be seen to be unavailable, but it can never
//! become WRONG.
//!
//! `verify` performs exactly those checks without producing bytes, so a UI
//! can re-scan a library and show which references have gone stale, and
//! `record` is idempotent — re-scanning a file that moved simply re-points
//! the same digest at its new path.
//!
//! ## What the rest of the store must know
//!
//! - Admission order mirrors the CAS law: hash the file FIRST, then record
//!   the `blobs` row, then the `blob_refs` row. A crash between them leaves
//!   a blob row whose bytes cannot be found — which reads refuse loudly —
//!   and never a reference row for a blob nobody recorded.
//! - Blob GC deletes the `blobs` row and calls `Cas::remove_object`, which
//!   finds nothing on disk for a reference and returns `false`. The external
//!   file is NEVER unlinked by this store, under any code path. GC drops the
//!   `blob_refs` row alongside the `blobs` row so the table cannot outlive
//!   the catalogue.

use crate::budget::Budgets;
use crate::error::{io_err, ServerError, ServerResult};
use crate::sqlite::Db;
use makepad_asset_data::{BlobId, Sha256};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Schema for reference blobs (catalog schema v10). Purely additive: one
/// table plus one index, both `IF NOT EXISTS`, so the migration step costs a
/// schema write and no table rewrite however large the store already is.
pub const BLOBREF_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS blob_refs(
    blob_id BLOB PRIMARY KEY,
    -- Absolute path on the machine that hosts this root. Reference blobs are
    -- inherently machine-local, which is why the import route that creates
    -- them refuses non-loopback callers.
    path TEXT NOT NULL,
    -- Length at record time. Checked on every read BEFORE hashing, so a file
    -- that grew or shrank is refused cheaply.
    size INTEGER NOT NULL,
    -- Modification time at record time, milliseconds since the epoch, when
    -- the filesystem offered one. A CHEAP staleness hint for re-scans only:
    -- it is never trusted in place of the digest.
    mtime_ms INTEGER,
    recorded_ms INTEGER NOT NULL
);
-- 'Do I already reference this exact file?' is what a directory re-import
-- asks once per file; without this index each ask is a full scan.
CREATE INDEX IF NOT EXISTS blob_refs_by_path ON blob_refs(path);
";

/// Longest path a reference row may carry. Bounds the row, the query and the
/// JSON that reports it; far above any real filesystem path.
pub const MAX_REF_PATH_BYTES: usize = 4096;

/// One recorded reference.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlobRef {
    pub blob_id: BlobId,
    pub path: PathBuf,
    pub size: u64,
    pub mtime_ms: Option<u64>,
    pub recorded_ms: u64,
}

/// What a reference looks like on disk right now.
///
/// Only `Present` ever yields bytes. The other three are the honest states a
/// reference can reach when the operator moves, edits or deletes the file the
/// store was pointed at, and they are reported as themselves rather than
/// collapsed into "not found".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefState {
    /// File present, length matches, digest matches. Safe to serve.
    Present,
    /// Nothing at the recorded path (deleted, renamed, volume unmounted).
    Missing,
    /// Present but a different length than when it was recorded.
    SizeChanged { expected: u64, found: u64 },
    /// Present and the right length, but different bytes.
    ContentChanged,
    /// Present but unreadable (permissions, IO error, device gone).
    Unreadable(std::io::ErrorKind),
}

impl RefState {
    pub fn is_present(&self) -> bool {
        matches!(self, RefState::Present)
    }

    /// Stable machine-readable tag for reporting. Static strings only.
    pub fn tag(&self) -> &'static str {
        match self {
            RefState::Present => "present",
            RefState::Missing => "missing",
            RefState::SizeChanged { .. } => "size_changed",
            RefState::ContentChanged => "content_changed",
            RefState::Unreadable(_) => "unreadable",
        }
    }
}

/// What hashing a file in place found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RefScan {
    pub blob_id: BlobId,
    pub size: u64,
    pub mtime_ms: Option<u64>,
    /// The absolute path the reference should record.
    pub path: PathBuf,
}

fn mtime_ms_of(meta: &std::fs::Metadata) -> Option<u64> {
    meta.modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis().min(u128::from(u64::MAX)) as u64)
}

/// Make a caller-supplied path absolute WITHOUT resolving symlinks.
///
/// `canonicalize` is deliberately not used: on Windows it produces `\\?\`
/// verbatim paths that read back badly in a UI, and following symlinks would
/// silently re-point a reference the operator described one way at a target
/// they did not name. Absolute-but-literal is what gets stored.
pub fn absolute_path(path: &Path) -> ServerResult<PathBuf> {
    let abs = std::path::absolute(path).map_err(io_err("blob ref absolute path"))?;
    let text = abs
        .to_str()
        .ok_or(ServerError::InvalidInput { what: "blob ref path encoding" })?;
    if text.is_empty() || text.len() > MAX_REF_PATH_BYTES {
        return Err(ServerError::InvalidInput { what: "blob ref path length" });
    }
    if text.chars().any(|c| c.is_control()) {
        return Err(ServerError::InvalidInput { what: "blob ref path charset" });
    }
    Ok(abs)
}

/// Hash a file where it lies. Nothing is copied and nothing is written; the
/// only effect is reading. `max_bytes` refuses a file the store could never
/// serve anyway, before the whole thing has been read.
pub fn scan_file(path: &Path, budgets: &Budgets) -> ServerResult<RefScan> {
    let abs = absolute_path(path)?;
    let meta = std::fs::metadata(&abs).map_err(io_err("blob ref stat"))?;
    if !meta.is_file() {
        return Err(ServerError::InvalidInput { what: "blob ref not a file" });
    }
    let size = meta.len();
    if size > budgets.max_blob_bytes {
        return Err(ServerError::OverBudget {
            what: "blob ref bytes",
            limit: budgets.max_blob_bytes,
            found: size,
        });
    }
    let mut file = File::open(&abs).map_err(io_err("blob ref open"))?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; budgets.io_chunk_bytes.max(1)];
    let mut total: u64 = 0;
    loop {
        let n = file.read(&mut buf).map_err(io_err("blob ref read"))?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n as u64);
        if total > budgets.max_blob_bytes {
            return Err(ServerError::OverBudget {
                what: "blob ref bytes",
                limit: budgets.max_blob_bytes,
                found: total,
            });
        }
        hasher.update(&buf[..n]);
    }
    // The file changed length between stat and read: refuse rather than
    // record a size the next read will reject.
    if total != size {
        return Err(ServerError::SizeMismatch {
            what: "blob ref scan",
            expected: size,
            found: total,
        });
    }
    Ok(RefScan {
        blob_id: BlobId::from_bytes(hasher.finalize()),
        size,
        mtime_ms: mtime_ms_of(&meta),
        path: abs,
    })
}

/// Check a reference against the file it names, producing no bytes.
///
/// Total: every filesystem outcome maps to a `RefState`, so a re-scan over a
/// whole library never aborts on the first unplugged drive.
pub fn verify(entry: &BlobRef, budgets: &Budgets) -> RefState {
    let meta = match std::fs::metadata(&entry.path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return RefState::Missing,
        Err(e) => return RefState::Unreadable(e.kind()),
    };
    if !meta.is_file() {
        return RefState::Missing;
    }
    if meta.len() != entry.size {
        return RefState::SizeChanged { expected: entry.size, found: meta.len() };
    }
    let mut file = match File::open(&entry.path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return RefState::Missing,
        Err(e) => return RefState::Unreadable(e.kind()),
    };
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; budgets.io_chunk_bytes.max(1)];
    let mut total: u64 = 0;
    loop {
        match file.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                total = total.saturating_add(n as u64);
                if total > entry.size {
                    return RefState::SizeChanged { expected: entry.size, found: total };
                }
                hasher.update(&buf[..n]);
            }
            Err(e) => return RefState::Unreadable(e.kind()),
        }
    }
    if total != entry.size {
        return RefState::SizeChanged { expected: entry.size, found: total };
    }
    if &hasher.finalize() != entry.blob_id.as_bytes() {
        return RefState::ContentChanged;
    }
    RefState::Present
}

/// Read a reference's bytes, fail-closed.
///
/// Same discipline as `Cas::read_verified`: the whole file is read and its
/// digest checked BEFORE a single byte is returned, so a caller can never be
/// handed a verified-looking prefix of a file that changed underneath it.
pub fn read_verified(entry: &BlobRef, budgets: &Budgets) -> ServerResult<Vec<u8>> {
    let meta = match std::fs::metadata(&entry.path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ServerError::NotFound { what: "blob ref file" })
        }
        Err(e) => return Err(io_err("blob ref stat")(e)),
    };
    if !meta.is_file() {
        return Err(ServerError::NotFound { what: "blob ref file" });
    }
    if meta.len() != entry.size {
        return Err(ServerError::SizeMismatch {
            what: "blob ref file",
            expected: entry.size,
            found: meta.len(),
        });
    }
    if entry.size > budgets.max_blob_bytes {
        return Err(ServerError::OverBudget {
            what: "blob ref bytes",
            limit: budgets.max_blob_bytes,
            found: entry.size,
        });
    }
    let mut file = match File::open(&entry.path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ServerError::NotFound { what: "blob ref file" })
        }
        Err(e) => return Err(io_err("blob ref open")(e)),
    };
    let mut hasher = Sha256::new();
    let mut out = Vec::with_capacity(entry.size as usize);
    let mut buf = vec![0u8; budgets.io_chunk_bytes.max(1)];
    loop {
        let n = file.read(&mut buf).map_err(io_err("blob ref read"))?;
        if n == 0 {
            break;
        }
        if out.len() as u64 + n as u64 > entry.size {
            return Err(ServerError::SizeMismatch {
                what: "blob ref file",
                expected: entry.size,
                found: out.len() as u64 + n as u64,
            });
        }
        hasher.update(&buf[..n]);
        out.extend_from_slice(&buf[..n]);
    }
    if out.len() as u64 != entry.size {
        return Err(ServerError::SizeMismatch {
            what: "blob ref file",
            expected: entry.size,
            found: out.len() as u64,
        });
    }
    let digest = hasher.finalize();
    if &digest != entry.blob_id.as_bytes() {
        return Err(ServerError::DigestMismatch {
            what: "blob ref file",
            expected: *entry.blob_id.as_bytes(),
            found: digest,
        });
    }
    Ok(out)
}

pub struct BlobRefs<'a> {
    pub(crate) db: &'a Db,
    pub(crate) budgets: &'a Budgets,
}

impl<'a> BlobRefs<'a> {
    /// Record (or re-point) a reference. Idempotent for the same path, and a
    /// file that MOVED re-points the same digest at its new location — the
    /// digest is the identity, the path is only where the bytes happen to be.
    pub fn record(
        &self,
        blob_id: &BlobId,
        path: &Path,
        size: u64,
        mtime_ms: Option<u64>,
        now_ms: u64,
    ) -> ServerResult<()> {
        let abs = absolute_path(path)?;
        let text = abs
            .to_str()
            .ok_or(ServerError::InvalidInput { what: "blob ref path encoding" })?;
        let mut s = self.db.prepare(
            "record blob ref",
            // Parameter-form upsert: the own engine executes `DO UPDATE SET
            // col = ?n` (the catalog's alias upsert is its parse test) but
            // not the `excluded.` pseudo-table yet.
            "INSERT INTO blob_refs(blob_id, path, size, mtime_ms, recorded_ms)
             VALUES(?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(blob_id) DO UPDATE SET
                 path = ?2,
                 size = ?3,
                 mtime_ms = ?4,
                 recorded_ms = ?5",
        )?;
        s.bind_blob(1, blob_id.as_bytes())?;
        s.bind_text(2, text)?;
        s.bind_u64(3, size)?;
        match mtime_ms {
            Some(t) => s.bind_u64(4, t)?,
            None => s.bind_null(4)?,
        }
        s.bind_u64(5, now_ms)?;
        s.run()
    }

    fn row(s: &crate::sqlite::Stmt<'_>) -> ServerResult<BlobRef> {
        let raw = s.column_blob(0);
        let bytes: [u8; 32] = raw
            .as_slice()
            .try_into()
            .map_err(|_| ServerError::InvalidInput { what: "blob ref id width" })?;
        Ok(BlobRef {
            blob_id: BlobId::from_bytes(bytes),
            path: PathBuf::from(s.column_text(1)),
            size: s.column_u64(2),
            mtime_ms: if s.column_is_null(3) { None } else { Some(s.column_u64(3)) },
            recorded_ms: s.column_u64(4),
        })
    }

    pub fn lookup(&self, blob_id: &BlobId) -> ServerResult<Option<BlobRef>> {
        let mut s = self.db.prepare(
            "lookup blob ref",
            "SELECT blob_id, path, size, mtime_ms, recorded_ms FROM blob_refs WHERE blob_id = ?1",
        )?;
        s.bind_blob(1, blob_id.as_bytes())?;
        if s.step()? {
            Ok(Some(Self::row(&s)?))
        } else {
            Ok(None)
        }
    }

    /// Which digest (if any) this exact path is already recorded under. A
    /// directory re-import asks this per file so an unchanged file is skipped
    /// without re-hashing gigabytes.
    pub fn by_path(&self, path: &Path) -> ServerResult<Option<BlobRef>> {
        let abs = absolute_path(path)?;
        let text = abs
            .to_str()
            .ok_or(ServerError::InvalidInput { what: "blob ref path encoding" })?;
        let mut s = self.db.prepare(
            "blob ref by path",
            "SELECT blob_id, path, size, mtime_ms, recorded_ms FROM blob_refs WHERE path = ?1",
        )?;
        s.bind_text(1, text)?;
        if s.step()? {
            Ok(Some(Self::row(&s)?))
        } else {
            Ok(None)
        }
    }

    /// Keyset page over every reference, ordered by digest. A whole-library
    /// re-scan walks this so its cost per call is chosen by the caller.
    pub fn list(&self, after: Option<&BlobId>, limit: u32) -> ServerResult<Vec<BlobRef>> {
        let limit = limit.clamp(1, 4096);
        let mut out = Vec::new();
        let mut s = match after {
            Some(a) => {
                let mut s = self.db.prepare(
                    "list blob refs after",
                    "SELECT blob_id, path, size, mtime_ms, recorded_ms FROM blob_refs
                     WHERE blob_id > ?1 ORDER BY blob_id LIMIT ?2",
                )?;
                s.bind_blob(1, a.as_bytes())?;
                s.bind_u64(2, u64::from(limit))?;
                s
            }
            None => {
                let mut s = self.db.prepare(
                    "list blob refs",
                    "SELECT blob_id, path, size, mtime_ms, recorded_ms FROM blob_refs
                     ORDER BY blob_id LIMIT ?1",
                )?;
                s.bind_u64(1, u64::from(limit))?;
                s
            }
        };
        while s.step()? {
            out.push(Self::row(&s)?);
        }
        Ok(out)
    }

    pub fn count(&self) -> ServerResult<u64> {
        let mut s = self.db.prepare("count blob refs", "SELECT COUNT(*) FROM blob_refs")?;
        if s.step()? {
            Ok(s.column_u64(0))
        } else {
            Ok(0)
        }
    }

    /// Drop a reference row. NEVER touches the file it named — that file
    /// belongs to whoever put it there, which is the entire point.
    pub fn remove(&self, blob_id: &BlobId) -> ServerResult<()> {
        let mut s = self
            .db
            .prepare("remove blob ref", "DELETE FROM blob_refs WHERE blob_id = ?1")?;
        s.bind_blob(1, blob_id.as_bytes())?;
        s.run()
    }

    pub fn budgets(&self) -> &Budgets {
        self.budgets
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn budgets() -> Budgets {
        Budgets::default_v1()
    }

    fn tmp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mp-blobref-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn scan_hashes_in_place_and_never_copies() {
        let dir = tmp_dir("scan");
        let file = dir.join("clip.mp4");
        std::fs::write(&file, b"MOVIE-BYTES").unwrap();
        let scan = scan_file(&file, &budgets()).unwrap();
        assert_eq!(scan.blob_id, BlobId::hash_of(b"MOVIE-BYTES"));
        assert_eq!(scan.size, 11);
        assert!(scan.path.is_absolute());
        // The only file in the directory is still the original.
        let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(entries, vec![file]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn verify_names_every_way_a_reference_goes_stale() {
        let dir = tmp_dir("verify");
        let file = dir.join("clip.mp4");
        std::fs::write(&file, b"MOVIE-BYTES").unwrap();
        let scan = scan_file(&file, &budgets()).unwrap();
        let entry = BlobRef {
            blob_id: scan.blob_id,
            path: scan.path.clone(),
            size: scan.size,
            mtime_ms: scan.mtime_ms,
            recorded_ms: 1,
        };
        assert_eq!(verify(&entry, &budgets()), RefState::Present);
        assert_eq!(read_verified(&entry, &budgets()).unwrap(), b"MOVIE-BYTES");

        // Same length, different bytes: content drift, not a size change.
        std::fs::write(&file, b"OTHER-BYTES").unwrap();
        assert_eq!(verify(&entry, &budgets()), RefState::ContentChanged);
        assert!(matches!(
            read_verified(&entry, &budgets()),
            Err(ServerError::DigestMismatch { .. })
        ));

        std::fs::write(&file, b"LONGER-MOVIE-BYTES").unwrap();
        assert_eq!(
            verify(&entry, &budgets()),
            RefState::SizeChanged { expected: 11, found: 18 }
        );
        assert!(matches!(
            read_verified(&entry, &budgets()),
            Err(ServerError::SizeMismatch { .. })
        ));

        std::fs::remove_file(&file).unwrap();
        assert_eq!(verify(&entry, &budgets()), RefState::Missing);
        assert!(matches!(
            read_verified(&entry, &budgets()),
            Err(ServerError::NotFound { what: "blob ref file" })
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_directory_is_not_a_reference() {
        let dir = tmp_dir("dir");
        assert!(matches!(
            scan_file(&dir, &budgets()),
            Err(ServerError::InvalidInput { what: "blob ref not a file" })
        ));
        std::fs::remove_dir_all(&dir).ok();
    }
}
