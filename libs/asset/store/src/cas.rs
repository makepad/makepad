//! Filesystem content-addressed store.
//!
//! Layout under the CAS root:
//!   objects/<hex[0..2]>/<hex[2..4]>/<64-hex>   committed blobs, by SHA-256
//!   tmp/w<pid>-<counter>.part                  in-flight streaming writes
//!
//! The two-level hash path is a scale decision: one level fans 256 ways, so
//! a million objects would be ~3.9k directory entries per shard and ten
//! million ~39k — directory sizes where lookup, `readdir` and backup tools
//! start to hurt on every filesystem. Two levels fan 65,536 ways: a million
//! objects is ~15 entries per leaf, ten million ~153.
//!
//! Roots written before schema v8 used the one-level layout, so every READ
//! falls back to it and `migrate_shards` (the v8 step) moves what it finds.
//! The fallback is not transitional politeness: it is what makes the move
//! crash-safe, because a rename that a power cut undid still resolves.
//!
//! Invariants:
//! - Bytes are hashed while they stream into a temp file; nothing ever lands
//!   at its final path without its digest having been computed from the very
//!   bytes on disk.
//! - Commit is fsync(temp) -> atomic rename -> fsync(parent dir). A crash at
//!   any point leaves either the complete object or an orphan temp file that
//!   `recover()` deletes; never a partial object at a final path.
//! - Reads re-hash and refuse on mismatch: a corrupt object is never served.
//! - Identical content dedups: a second commit of the same bytes discards its
//!   temp file and reports `deduped`.

use crate::budget::Budgets;
use crate::error::{io_err, ServerError, ServerResult};
use makepad_asset_data::{BlobId, Sha256};
use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn hex64(id: &BlobId) -> String {
    let mut s = String::with_capacity(64);
    for b in id.as_bytes() {
        use std::fmt::Write;
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn fsync_dir(dir: &Path) -> ServerResult<()> {
    File::open(dir).and_then(|f| f.sync_all()).map_err(io_err("cas fsync dir"))
}

pub struct Cas {
    objects: PathBuf,
    tmp: PathBuf,
    max_blob_bytes: u64,
    chunk: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlobCommit {
    pub blob_id: BlobId,
    pub size: u64,
    pub deduped: bool,
}

impl Cas {
    pub fn open(root: &Path, budgets: &Budgets) -> ServerResult<Cas> {
        let objects = root.join("objects");
        let tmp = root.join("tmp");
        fs::create_dir_all(&objects).map_err(io_err("cas create objects dir"))?;
        fs::create_dir_all(&tmp).map_err(io_err("cas create tmp dir"))?;
        // Persist the layout dirs themselves: their entries in the CAS root
        // must survive a crash for anything committed under them to survive.
        fsync_dir(root)?;
        Ok(Cas {
            objects,
            tmp,
            max_blob_bytes: budgets.max_blob_bytes,
            chunk: budgets.io_chunk_bytes,
        })
    }

    /// Crash recovery: every file under tmp/ is an abandoned in-flight write
    /// (commit renames out of tmp atomically). Delete them all; returns count.
    pub fn recover(&self) -> ServerResult<u64> {
        let mut removed = 0;
        let entries = fs::read_dir(&self.tmp).map_err(io_err("cas read tmp dir"))?;
        for entry in entries {
            let entry = entry.map_err(io_err("cas read tmp entry"))?;
            fs::remove_file(entry.path()).map_err(io_err("cas remove tmp orphan"))?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Where a blob lives today: `objects/ab/cd/<64-hex>`.
    fn object_path(&self, id: &BlobId) -> PathBuf {
        let hex = hex64(id);
        self.objects.join(&hex[..2]).join(&hex[2..4]).join(&hex)
    }

    /// Where a pre-v8 root put it: `objects/ab/<64-hex>`. Read-only now —
    /// nothing new is ever written here.
    fn legacy_path(&self, id: &BlobId) -> PathBuf {
        let hex = hex64(id);
        self.objects.join(&hex[..2]).join(&hex)
    }

    /// The path a read must use: the sharded location, else the pre-v8 one.
    /// Both name the same digest, so whichever exists holds the right bytes
    /// (and `read_verified` re-hashes regardless).
    fn read_path(&self, id: &BlobId) -> Option<PathBuf> {
        let sharded = self.object_path(id);
        if sharded.is_file() {
            return Some(sharded);
        }
        let legacy = self.legacy_path(id);
        if legacy.is_file() {
            return Some(legacy);
        }
        None
    }

    pub fn contains(&self, id: &BlobId) -> bool {
        self.read_path(id).is_some()
    }

    /// Move every object still at a pre-v8 one-level path into its two-level
    /// shard. Idempotent and crash-safe: each object moves with a single
    /// atomic rename, an object already at its sharded path is left alone,
    /// and an interrupted run leaves a mix that reads resolve either way.
    /// Returns how many objects moved.
    pub fn migrate_shards(&self) -> ServerResult<u64> {
        let mut moved = 0u64;
        let mut touched: BTreeSet<PathBuf> = BTreeSet::new();
        for fanout in fs::read_dir(&self.objects).map_err(io_err("cas read objects dir"))? {
            let fanout = fanout.map_err(io_err("cas read objects entry"))?;
            let fanout_path = fanout.path();
            if !fanout_path.is_dir() {
                continue;
            }
            for entry in fs::read_dir(&fanout_path).map_err(io_err("cas read fanout dir"))? {
                let entry = entry.map_err(io_err("cas read fanout entry"))?;
                let path = entry.path();
                // Directories here are the new second level; only files are
                // objects left over from the one-level layout.
                if path.is_dir() {
                    continue;
                }
                let name = match path.file_name().and_then(|n| n.to_str()) {
                    Some(n) => n,
                    // A name that is not valid UTF-8 cannot be a hex digest;
                    // leave it alone rather than guess.
                    None => continue,
                };
                if name.len() != 64 || !name.bytes().all(|b| b.is_ascii_hexdigit()) {
                    continue;
                }
                let dest_dir = fanout_path.join(&name[2..4]);
                let dest = dest_dir.join(name);
                if dest.is_file() {
                    // Both layouts hold this digest: identical content by
                    // construction. Drop the stale one-level copy.
                    fs::remove_file(&path).map_err(io_err("cas drop legacy duplicate"))?;
                    continue;
                }
                fs::create_dir_all(&dest_dir).map_err(io_err("cas create shard dir"))?;
                fs::rename(&path, &dest).map_err(io_err("cas shard rename"))?;
                touched.insert(dest_dir);
                touched.insert(fanout_path.clone());
                moved += 1;
            }
        }
        // Persist the new directory entries. Once per directory, not once per
        // object: the entries are only durable as a set anyway.
        for dir in &touched {
            fsync_dir(dir)?;
        }
        Ok(moved)
    }

    pub fn begin(&self) -> ServerResult<BlobWriter> {
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = self.tmp.join(format!("w{}-{n}.part", std::process::id()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp_path)
            .map_err(io_err("cas open temp"))?;
        Ok(BlobWriter {
            file: Some(file),
            tmp_path,
            hasher: Sha256::new(),
            written: 0,
            max: self.max_blob_bytes,
            committed: false,
        })
    }

    /// Finish a streamed write: validate the digest (against `expected` when
    /// given), then commit atomically or dedup against an existing object.
    pub fn commit(&self, mut writer: BlobWriter, expected: Option<BlobId>) -> ServerResult<BlobCommit> {
        let file = writer.file.take().ok_or(ServerError::InvalidState {
            what: "blob writer",
            state: "already committed",
        })?;
        // Hasher is consumed by finalize; std::mem::replace keeps the writer's
        // Drop (temp cleanup) armed until we succeed.
        let hasher = std::mem::replace(&mut writer.hasher, Sha256::new());
        let digest = hasher.finalize();
        let blob_id = BlobId::from_bytes(digest);
        if let Some(exp) = expected {
            if exp != blob_id {
                // Refuse and let writer's Drop delete the temp file.
                return Err(ServerError::DigestMismatch {
                    what: "cas commit",
                    expected: *exp.as_bytes(),
                    found: digest,
                });
            }
        }
        // Dedup looks at BOTH layouts: a pre-v8 object that has not been
        // sharded yet is still this content, and re-writing it under the new
        // path would leave the same bytes on disk twice.
        if let Some(existing) = self.read_path(&blob_id) {
            // The existing object's size must agree; anything else is
            // corruption and refuses the commit rather than papering over it.
            let meta = fs::metadata(&existing).map_err(io_err("cas stat object"))?;
            if meta.len() != writer.written {
                return Err(ServerError::SizeMismatch {
                    what: "cas dedup object",
                    expected: writer.written,
                    found: meta.len(),
                });
            }
            return Ok(BlobCommit {
                blob_id,
                size: writer.written,
                deduped: true,
            });
            // writer drops here and removes its temp file.
        }
        file.sync_all().map_err(io_err("cas fsync temp"))?;
        drop(file);
        let final_path = self.object_path(&blob_id);
        let leaf = final_path.parent().expect("object path has parent");
        let fanout = leaf.parent().expect("shard path has parent");
        // A first commit into this shard creates directories; each creation
        // is an entry in ITS parent and must be fsynced there too, or a crash
        // can drop the whole shard including the renamed object.
        let fanout_created = !fanout.is_dir();
        let leaf_created = !leaf.is_dir();
        fs::create_dir_all(leaf).map_err(io_err("cas create fanout dir"))?;
        if fanout_created {
            fsync_dir(&self.objects)?;
        }
        if leaf_created {
            fsync_dir(fanout)?;
        }
        fs::rename(&writer.tmp_path, &final_path).map_err(io_err("cas commit rename"))?;
        // The temp file no longer exists; disarm the Drop cleanup.
        writer.committed = true;
        fsync_dir(leaf)?;
        Ok(BlobCommit {
            blob_id,
            size: writer.written,
            deduped: false,
        })
    }

    /// Delete an object, from BOTH layouts (a pre-v8 root may still hold it
    /// at the one-level path, and leaving that copy behind would silently
    /// resurrect collected bytes). Absent objects succeed: removal is
    /// idempotent because blob GC may retry an unlink a crash interrupted.
    /// Returns whether anything was on disk.
    ///
    /// This is the ONLY path that removes committed bytes, and it is called
    /// exclusively by the sweep, after the catalog row is gone.
    pub fn remove_object(&self, id: &BlobId) -> ServerResult<bool> {
        let mut removed = false;
        for path in [self.object_path(id), self.legacy_path(id)] {
            match fs::remove_file(&path) {
                Ok(()) => removed = true,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io_err("cas remove object")(e)),
            }
        }
        Ok(removed)
    }

    /// Read a whole blob, re-hashing; refuses corrupt bytes. Nothing is
    /// returned unless the complete digest matched.
    pub fn read_verified(&self, id: &BlobId) -> ServerResult<Vec<u8>> {
        let path = self.read_path(id).unwrap_or_else(|| self.object_path(id));
        let mut file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ServerError::NotFound { what: "cas object" })
            }
            Err(e) => return Err(io_err("cas open object")(e)),
        };
        let mut hasher = Sha256::new();
        let mut total: u64 = 0;
        let mut out = Vec::new();
        let mut buf = vec![0u8; self.chunk.max(1)];
        loop {
            let n = file.read(&mut buf).map_err(io_err("cas read object"))?;
            if n == 0 {
                break;
            }
            total = total
                .checked_add(n as u64)
                .ok_or(ServerError::OverBudget {
                    what: "cas object bytes",
                    limit: self.max_blob_bytes,
                    found: u64::MAX,
                })?;
            if total > self.max_blob_bytes {
                return Err(ServerError::OverBudget {
                    what: "cas object bytes",
                    limit: self.max_blob_bytes,
                    found: total,
                });
            }
            hasher.update(&buf[..n]);
            out.extend_from_slice(&buf[..n]);
        }
        let digest = hasher.finalize();
        if &digest != id.as_bytes() {
            return Err(ServerError::DigestMismatch {
                what: "cas object",
                expected: *id.as_bytes(),
                found: digest,
            });
        }
        Ok(out)
    }

    /// Write a blob into `sink`, fail-closed: the object is read and its
    /// whole digest verified in memory FIRST, and only fully verified bytes
    /// are ever emitted. On any refusal (missing, corrupt, over budget) the
    /// sink receives nothing. Memory use is bounded by `max_blob_bytes`.
    pub fn stream_verified(&self, id: &BlobId, sink: &mut dyn Write) -> ServerResult<u64> {
        let bytes = self.read_verified(id)?;
        sink.write_all(&bytes).map_err(io_err("cas write sink"))?;
        Ok(bytes.len() as u64)
    }
}

/// A streaming blob write: hash-while-write into a private temp file.
/// Dropping without commit deletes the temp file.
pub struct BlobWriter {
    file: Option<File>,
    tmp_path: PathBuf,
    hasher: Sha256,
    written: u64,
    max: u64,
    /// Set by `Cas::commit` after the rename; disarms Drop's temp cleanup.
    committed: bool,
}

impl BlobWriter {
    pub fn write(&mut self, data: &[u8]) -> ServerResult<()> {
        let file = self.file.as_mut().ok_or(ServerError::InvalidState {
            what: "blob writer",
            state: "already committed",
        })?;
        let new_total = self
            .written
            .checked_add(data.len() as u64)
            .ok_or(ServerError::OverBudget {
                what: "blob bytes",
                limit: self.max,
                found: u64::MAX,
            })?;
        if new_total > self.max {
            return Err(ServerError::OverBudget {
                what: "blob bytes",
                limit: self.max,
                found: new_total,
            });
        }
        file.write_all(data).map_err(io_err("cas write temp"))?;
        self.hasher.update(data);
        self.written = new_total;
        Ok(())
    }

    pub fn written(&self) -> u64 {
        self.written
    }
}

impl Drop for BlobWriter {
    fn drop(&mut self) {
        if !self.committed {
            self.file.take();
            let _ = fs::remove_file(&self.tmp_path);
        }
    }
}
