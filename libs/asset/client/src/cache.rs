//! Verified, atomic, content-addressed local cache with pinning and
//! eviction budgets.
//!
//! Layout under the cache root (one process owns a root at a time; two apps
//! sharing one root is not supported — give each its own):
//!
//! ```text
//! objects/<first-2-hex>/<64-hex>   committed verified blobs, named by SHA-256
//! partial/<64-hex>.part            resumable in-flight downloads, by digest
//! tmp/w<pid>-<counter>.part        non-resumable staging (swept at open)
//! pins/<64-hex>                    pin markers (empty files)
//! lru/<64-hex>                     last-use stamps (ascii milliseconds)
//! ```
//!
//! Durability policy: this is a CACHE of content-addressed bytes, not a
//! store of record. Every read re-hashes (`resolve`), a mismatch evicts and
//! reports absent, and anything missing is simply re-fetched — so a crash
//! can only ever cost a re-download. Paying `F_FULLFSYNC` per committed
//! object would buy nothing the digest does not already guarantee, and on a
//! Mac it costs ~10 ms per object: a grid of thirty thumbnails would spend
//! a third of a second waiting on disk barriers for bytes it can re-fetch in
//! microseconds. Commits therefore flush to the OS and rename atomically,
//! without a device barrier.
//!
//! Invariants:
//! - Bytes are hashed while they stream into `partial/`/`tmp/`; nothing lands
//!   under `objects/` without its digest having been computed from the exact
//!   bytes on disk. Commit is flush → atomic rename.
//! - Reads re-hash and refuse: a corrupt object is deleted and reported
//!   absent, never served. The resolver's "verified path" promise rests here.
//! - Partial downloads are keyed by their EXPECTED digest and survive both
//!   process restarts and dropped writers, so Range resume can continue them;
//!   stale ones are swept at open. `tmp/` files never survive open.
//! - Budgets: per-object, total, and partial byte budgets. Admission evicts
//!   least-recently-used UNPINNED objects to make room and refuses when
//!   pinned bytes alone would exceed the total budget. Eviction is explicit
//!   and observable in [`CacheStats`]; nothing is silently dropped mid-use by
//!   this process (single-owner contract).
//! - Time is injected (`now_ms` parameters): eviction order and staleness are
//!   deterministic under test.

use crate::error::{io_err, ClientError, ClientResult};
use crate::util::{from_hex_exact, to_hex};
use makepad_asset_data::Sha256;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy, Debug)]
pub struct CacheBudgets {
    /// Total committed object bytes the cache may hold.
    pub max_total_bytes: u64,
    /// Largest single object admitted.
    pub max_object_bytes: u64,
    /// Total bytes `partial/` may hold before old partials are swept.
    pub max_partial_bytes: u64,
    /// A partial older than this (since last write) is deleted at open.
    pub stale_partial_ms: u64,
    /// Verified blob bytes the client may hold in PROCESS MEMORY across
    /// fetches. Unbounded residency was a leak with a polite name: an app
    /// that browses a catalog holds every blob it ever opened until it
    /// exits. See [`RamCache`].
    pub max_ram_bytes: u64,
}

/// Verified blob bytes held in process memory, keyed by content digest,
/// evicted least-recently-used under a byte budget.
///
/// A digest names its bytes, so an entry can never be the wrong content for
/// its key — this cache cannot go stale, only cold. What it CAN do is grow:
/// hence the budget, the LRU, and [`RamCache::forget`] for a caller that
/// knows an object will not be wanted again.
#[derive(Debug)]
pub struct RamCache {
    entries: HashMap<[u8; 32], RamEntry>,
    used: u64,
    budget: u64,
    clock: u64,
}

#[derive(Debug)]
struct RamEntry {
    bytes: Vec<u8>,
    last_used: u64,
}

impl RamCache {
    pub fn new(budget: u64) -> Self {
        Self {
            entries: HashMap::new(),
            used: 0,
            budget: budget.max(1),
            clock: 0,
        }
    }

    /// Bytes for `digest`, marked most-recently-used.
    pub fn get(&mut self, digest: &[u8; 32]) -> Option<&Vec<u8>> {
        self.clock += 1;
        let clock = self.clock;
        let entry = self.entries.get_mut(digest)?;
        entry.last_used = clock;
        Some(&entry.bytes)
    }

    /// Hold `bytes`. An object larger than the whole budget is NOT held:
    /// admitting it would evict everything else to cache a single item that
    /// the next fetch evicts again.
    pub fn insert(&mut self, digest: [u8; 32], bytes: Vec<u8>) {
        let len = bytes.len() as u64;
        if len > self.budget {
            self.forget(&digest);
            return;
        }
        self.clock += 1;
        self.forget(&digest);
        self.used = self.used.saturating_add(len);
        self.entries.insert(
            digest,
            RamEntry {
                bytes,
                last_used: self.clock,
            },
        );
        self.evict_to_budget();
    }

    /// Drop one object. Returns whether it was resident.
    pub fn forget(&mut self, digest: &[u8; 32]) -> bool {
        match self.entries.remove(digest) {
            Some(old) => {
                self.used = self.used.saturating_sub(old.bytes.len() as u64);
                true
            }
            None => false,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.used = 0;
    }

    pub fn used_bytes(&self) -> u64 {
        self.used
    }

    pub fn budget_bytes(&self) -> u64 {
        self.budget
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn evict_to_budget(&mut self) {
        while self.used > self.budget {
            let Some((victim, size)) = self
                .entries
                .iter()
                .min_by_key(|(_, e)| e.last_used)
                .map(|(k, e)| (*k, e.bytes.len() as u64))
            else {
                break;
            };
            self.entries.remove(&victim);
            self.used = self.used.saturating_sub(size);
        }
    }
}

impl CacheBudgets {
    pub fn default_v1() -> Self {
        Self {
            max_total_bytes: 4 * 1024 * 1024 * 1024,
            max_object_bytes: 256 * 1024 * 1024,
            max_partial_bytes: 512 * 1024 * 1024,
            stale_partial_ms: 7 * 24 * 60 * 60 * 1000,
            max_ram_bytes: 512 * 1024 * 1024,
        }
    }

    pub fn validate(&self) -> ClientResult<()> {
        if self.max_total_bytes == 0
            || self.max_object_bytes == 0
            || self.max_partial_bytes == 0
            || self.max_ram_bytes == 0
            || self.stale_partial_ms == 0
        {
            return Err(ClientError::InvalidInput { what: "cache budgets zero" });
        }
        if self.max_object_bytes > self.max_total_bytes {
            return Err(ClientError::InvalidInput { what: "cache object budget over total" });
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheStats {
    pub object_count: u64,
    pub total_bytes: u64,
    pub pinned_bytes: u64,
    pub partial_bytes: u64,
    /// Objects removed to make room for admissions.
    pub evictions: u64,
    /// Objects removed because their bytes no longer matched their digest.
    pub corruption_evictions: u64,
}

/// Push a directory's entries out at cache OPEN time only: the root layout
/// has to exist for anything else to make sense. Per-object commits do not
/// call this — see the durability policy above.
///
/// Windows has no directory fsync to call: a directory handle is read-only
/// even with `FILE_FLAG_BACKUP_SEMANTICS`, a plain `File::open` of one is
/// refused with `PermissionDenied`, and `FlushFileBuffers` on a read handle
/// answers `ERROR_ACCESS_DENIED`. Left as-is this refused every cache open
/// on Windows. NTFS journals its own metadata, so a returned rename is
/// already durable; the no-op is the correct Windows behaviour, not a
/// swallowed error. (Same reasoning as `makepad_asset_store::cas::fsync_dir`.)
#[cfg(windows)]
fn fsync_dir(_dir: &Path) -> ClientResult<()> {
    Ok(())
}

#[cfg(not(windows))]
fn fsync_dir(dir: &Path) -> ClientResult<()> {
    File::open(dir).and_then(|f| f.sync_all()).map_err(io_err("cache fsync dir"))
}

pub struct ContentCache {
    root: PathBuf,
    objects: PathBuf,
    partial: PathBuf,
    tmp: PathBuf,
    pins: PathBuf,
    lru: PathBuf,
    /// Exclusive advisory lock on `<root>/cache.lock`, held for the cache's
    /// whole life: the single-owner-per-root doctrine, ENFORCED. A second
    /// process (an accidentally doubled app instance) refuses at open
    /// instead of sweeping this instance's partials. The OS releases the
    /// lock on process death, so restart is always clean.
    _root_lock: std::fs::File,
    budgets: CacheBudgets,
    /// digest → committed size. The in-memory index of `objects/`.
    index: HashMap<[u8; 32], u64>,
    pinned: HashSet<[u8; 32]>,
    total_bytes: u64,
    evictions: u64,
    corruption_evictions: u64,
}

impl ContentCache {
    /// Open (creating if absent) a cache root. Sweeps `tmp/` orphans and
    /// stale partials, loads the object index and pin set.
    pub fn open(root: &Path, budgets: CacheBudgets, now_ms: u64) -> ClientResult<ContentCache> {
        budgets.validate()?;
        let objects = root.join("objects");
        let partial = root.join("partial");
        let tmp = root.join("tmp");
        let pins = root.join("pins");
        let lru = root.join("lru");
        for dir in [&objects, &partial, &tmp, &pins, &lru] {
            fs::create_dir_all(dir).map_err(io_err("cache create dir"))?;
        }
        fsync_dir(root)?;

        // Take the root ownership lock BEFORE any sweeping: two live caches
        // on one root would treat each other's fresh partials as stale.
        let root_lock = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(false)
            .open(root.join("cache.lock"))
            .map_err(io_err("open cache lock"))?;
        if root_lock.try_lock().is_err() {
            return Err(ClientError::CacheBusy);
        }

        // tmp/ never survives open: everything in it is an abandoned
        // non-resumable write.
        for entry in fs::read_dir(&tmp).map_err(io_err("cache read tmp"))? {
            let entry = entry.map_err(io_err("cache read tmp entry"))?;
            fs::remove_file(entry.path()).map_err(io_err("cache sweep tmp"))?;
        }

        // Stale partials go; fresh ones stay resumable.
        for entry in fs::read_dir(&partial).map_err(io_err("cache read partial"))? {
            let entry = entry.map_err(io_err("cache read partial entry"))?;
            let path = entry.path();
            if partial_digest_of(&path).is_none() {
                // Not a digest-keyed partial: foreign garbage, remove.
                fs::remove_file(&path).map_err(io_err("cache sweep partial"))?;
                continue;
            }
            let modified = entry
                .metadata()
                .and_then(|m| m.modified())
                .map_err(io_err("cache stat partial"))?;
            let age_ms = modified
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| now_ms.saturating_sub(d.as_millis() as u64))
                .unwrap_or(u64::MAX);
            if age_ms > budgets.stale_partial_ms {
                fs::remove_file(&path).map_err(io_err("cache sweep stale partial"))?;
            }
        }

        // Index committed objects.
        let mut index = HashMap::new();
        let mut total_bytes = 0u64;
        for fan in fs::read_dir(&objects).map_err(io_err("cache read objects"))? {
            let fan = fan.map_err(io_err("cache read fanout"))?;
            if !fan.file_type().map_err(io_err("cache fanout type"))?.is_dir() {
                continue;
            }
            for obj in fs::read_dir(fan.path()).map_err(io_err("cache read fanout dir"))? {
                let obj = obj.map_err(io_err("cache read object entry"))?;
                let name = obj.file_name();
                let Some(name) = name.to_str() else { continue };
                let Some(digest) = from_hex_exact::<32>(name) else {
                    continue;
                };
                let size = obj.metadata().map_err(io_err("cache stat object"))?.len();
                total_bytes = total_bytes.saturating_add(size);
                index.insert(digest, size);
            }
        }

        // Pin markers.
        let mut pinned = HashSet::new();
        for entry in fs::read_dir(&pins).map_err(io_err("cache read pins"))? {
            let entry = entry.map_err(io_err("cache read pin entry"))?;
            let name = entry.file_name();
            if let Some(digest) = name.to_str().and_then(from_hex_exact::<32>) {
                pinned.insert(digest);
            }
        }

        Ok(ContentCache {
            root: root.to_path_buf(),
            objects,
            partial,
            tmp,
            pins,
            lru,
            _root_lock: root_lock,
            budgets,
            index,
            pinned,
            total_bytes,
            evictions: 0,
            corruption_evictions: 0,
        })
    }

    pub fn budgets(&self) -> &CacheBudgets {
        &self.budgets
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn stats(&self) -> CacheStats {
        let pinned_bytes = self
            .pinned
            .iter()
            .filter_map(|d| self.index.get(d))
            .sum();
        let partial_bytes = fs::read_dir(&self.partial)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.metadata().ok())
            .map(|m| m.len())
            .sum();
        CacheStats {
            object_count: self.index.len() as u64,
            total_bytes: self.total_bytes,
            pinned_bytes,
            partial_bytes,
            evictions: self.evictions,
            corruption_evictions: self.corruption_evictions,
        }
    }

    pub fn contains(&self, digest: &[u8; 32]) -> bool {
        self.index.contains_key(digest)
    }

    /// The path a committed digest occupies. Public because callers that
    /// commit bytes themselves (the batch path) need the same answer
    /// `resolve` would give without re-hashing what was just written.
    pub fn object_path_of(&self, digest: &[u8; 32]) -> PathBuf {
        self.object_path(digest)
    }

    fn object_path(&self, digest: &[u8; 32]) -> PathBuf {
        let hex = to_hex(digest);
        self.objects.join(&hex[..2]).join(&hex)
    }

    fn lru_path(&self, digest: &[u8; 32]) -> PathBuf {
        self.lru.join(to_hex(digest))
    }

    fn pin_path(&self, digest: &[u8; 32]) -> PathBuf {
        self.pins.join(to_hex(digest))
    }

    fn partial_path(&self, digest: &[u8; 32]) -> PathBuf {
        self.partial.join(format!("{}.part", to_hex(digest)))
    }

    fn touch(&self, digest: &[u8; 32], now_ms: u64) -> ClientResult<()> {
        fs::write(self.lru_path(digest), format!("{now_ms}"))
            .map_err(io_err("cache write lru stamp"))
    }

    fn lru_stamp(&self, digest: &[u8; 32]) -> u64 {
        fs::read_to_string(self.lru_path(digest))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0)
    }

    // ---- pinning ----------------------------------------------------------

    /// Mark a digest as never-evictable. Pinning an absent object records the
    /// intent; the object is protected the moment it is committed.
    pub fn pin(&mut self, digest: &[u8; 32]) -> ClientResult<()> {
        fs::write(self.pin_path(digest), b"").map_err(io_err("cache write pin"))?;
        self.pinned.insert(*digest);
        Ok(())
    }

    pub fn unpin(&mut self, digest: &[u8; 32]) -> ClientResult<()> {
        match fs::remove_file(self.pin_path(digest)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err("cache remove pin")(e)),
        }
        self.pinned.remove(digest);
        Ok(())
    }

    pub fn is_pinned(&self, digest: &[u8; 32]) -> bool {
        self.pinned.contains(digest)
    }

    // ---- admission / eviction ---------------------------------------------

    /// Make room for `incoming` bytes, evicting least-recently-used unpinned
    /// objects. Refuses (without evicting anything) when the object alone or
    /// the pinned set plus the object cannot fit.
    fn admit(&mut self, incoming: u64) -> ClientResult<()> {
        if incoming > self.budgets.max_object_bytes {
            return Err(ClientError::CacheAdmission {
                what: "object over per-object budget",
                limit: self.budgets.max_object_bytes,
                found: incoming,
            });
        }
        if self.total_bytes.saturating_add(incoming) <= self.budgets.max_total_bytes {
            return Ok(());
        }
        // Can eviction ever make this fit? Pinned bytes are immovable.
        let pinned_bytes: u64 = self
            .pinned
            .iter()
            .filter_map(|d| self.index.get(d))
            .sum();
        if pinned_bytes.saturating_add(incoming) > self.budgets.max_total_bytes {
            return Err(ClientError::CacheAdmission {
                what: "pinned bytes leave no room",
                limit: self.budgets.max_total_bytes,
                found: pinned_bytes.saturating_add(incoming),
            });
        }
        let mut victims: Vec<([u8; 32], u64, u64)> = self
            .index
            .iter()
            .filter(|(d, _)| !self.pinned.contains(*d))
            .map(|(d, size)| (*d, *size, self.lru_stamp(d)))
            .collect();
        // Oldest stamp first; ties by digest for determinism.
        victims.sort_by(|a, b| a.2.cmp(&b.2).then_with(|| a.0.cmp(&b.0)));
        for (digest, _, _) in victims {
            if self.total_bytes.saturating_add(incoming) <= self.budgets.max_total_bytes {
                break;
            }
            self.remove_object(&digest)?;
            self.evictions += 1;
        }
        debug_assert!(self.total_bytes.saturating_add(incoming) <= self.budgets.max_total_bytes);
        Ok(())
    }

    fn remove_object(&mut self, digest: &[u8; 32]) -> ClientResult<()> {
        match fs::remove_file(self.object_path(digest)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err("cache remove object")(e)),
        }
        let _ = fs::remove_file(self.lru_path(digest));
        if let Some(size) = self.index.remove(digest) {
            self.total_bytes = self.total_bytes.saturating_sub(size);
        }
        Ok(())
    }

    /// Remove one committed object while retaining any pin marker as intent
    /// for a future verified re-admission.
    pub fn remove(&mut self, digest: &[u8; 32]) -> ClientResult<bool> {
        let existed = self.index.contains_key(digest);
        self.remove_object(digest)?;
        Ok(existed)
    }

    // ---- commit paths ------------------------------------------------------

    /// Commit in-memory bytes (manifests, thumbnails). Verifies against
    /// `expected` when given; returns the digest.
    pub fn put_bytes(
        &mut self,
        bytes: &[u8],
        expected: Option<&[u8; 32]>,
        now_ms: u64,
    ) -> ClientResult<[u8; 32]> {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let digest = hasher.finalize();
        if let Some(exp) = expected {
            if exp != &digest {
                return Err(ClientError::DigestMismatch {
                    what: "cache put",
                    expected: *exp,
                    found: digest,
                });
            }
        }
        if let Some(&size) = self.index.get(&digest) {
            if size != bytes.len() as u64 {
                return Err(ClientError::SizeMismatch {
                    what: "cache dedup object",
                    expected: bytes.len() as u64,
                    found: size,
                });
            }
            self.touch(&digest, now_ms)?;
            return Ok(digest);
        }
        self.admit(bytes.len() as u64)?;
        let n = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let tmp_path = self.tmp.join(format!("w{}-{n}.part", std::process::id()));
        {
            let mut f = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)
                .map_err(io_err("cache open tmp"))?;
            f.write_all(bytes).map_err(io_err("cache write tmp"))?;
            // Flush to the OS, no device barrier: a lost cache object is a
            // re-fetch, and `resolve` re-hashes anything that survives.
            f.flush().map_err(io_err("cache flush tmp"))?;
        }
        match self.commit_file(&tmp_path, &digest, bytes.len() as u64, now_ms) {
            Ok(()) => Ok(digest),
            Err(e) => {
                let _ = fs::remove_file(&tmp_path);
                Err(e)
            }
        }
    }

    /// Rename a fully written, fsynced file into `objects/` and index it.
    fn commit_file(
        &mut self,
        from: &Path,
        digest: &[u8; 32],
        size: u64,
        now_ms: u64,
    ) -> ClientResult<()> {
        let final_path = self.object_path(digest);
        let dir = final_path.parent().expect("object path has parent");
        fs::create_dir_all(dir).map_err(io_err("cache create fanout"))?;
        // Atomic rename, no directory barrier: the rename is what makes the
        // object visible-or-not, and a crash that loses the entry costs a
        // re-fetch of bytes we can always ask for again.
        fs::rename(from, &final_path).map_err(io_err("cache commit rename"))?;
        self.index.insert(*digest, size);
        self.total_bytes = self.total_bytes.saturating_add(size);
        self.touch(digest, now_ms)?;
        Ok(())
    }

    // ---- resumable partial downloads ---------------------------------------

    /// Open (or reopen) the resumable download for `expected`. When a partial
    /// already exists its bytes are re-hashed so the returned writer's hash
    /// state covers exactly what is on disk; `resumed_bytes()` tells the
    /// caller which Range offset to request.
    pub fn open_partial(&mut self, expected: &[u8; 32]) -> ClientResult<PartialWriter> {
        // Keep the partial area under budget: sweep other, oldest partials.
        self.sweep_partials_for_budget(expected)?;
        let path = self.partial_path(expected);
        let mut hasher = Sha256::new();
        let mut written = 0u64;
        // Append mode: every later write lands at EOF, i.e. exactly after the
        // bytes re-hashed below.
        let file = match OpenOptions::new().append(true).open(&path) {
            Ok(f) => {
                let mut buf = vec![0u8; 64 * 1024];
                let mut reader = File::open(&path).map_err(io_err("cache reopen partial"))?;
                loop {
                    let n = reader.read(&mut buf).map_err(io_err("cache rehash partial"))?;
                    if n == 0 {
                        break;
                    }
                    written = written.saturating_add(n as u64);
                    if written > self.budgets.max_object_bytes {
                        drop(reader);
                        drop(f);
                        fs::remove_file(&path).map_err(io_err("cache remove oversize partial"))?;
                        return Err(ClientError::CacheAdmission {
                            what: "partial over per-object budget",
                            limit: self.budgets.max_object_bytes,
                            found: written,
                        });
                    }
                    hasher.update(&buf[..n]);
                }
                f
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => OpenOptions::new()
                .append(true)
                .create_new(true)
                .open(&path)
                .map_err(io_err("cache create partial"))?,
            Err(e) => return Err(io_err("cache open partial")(e)),
        };
        Ok(PartialWriter {
            file,
            path,
            expected: *expected,
            hasher,
            written,
            max: self.budgets.max_object_bytes,
        })
    }

    fn sweep_partials_for_budget(&self, keep: &[u8; 32]) -> ClientResult<()> {
        let mut entries: Vec<(PathBuf, u64, u128)> = Vec::new();
        let mut total = 0u64;
        for entry in fs::read_dir(&self.partial).map_err(io_err("cache read partial"))? {
            let entry = entry.map_err(io_err("cache read partial entry"))?;
            let meta = entry.metadata().map_err(io_err("cache stat partial"))?;
            total = total.saturating_add(meta.len());
            let modified = meta
                .modified()
                .map_err(io_err("cache stat partial"))?
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            entries.push((entry.path(), meta.len(), modified));
        }
        if total <= self.budgets.max_partial_bytes {
            return Ok(());
        }
        let keep_path = self.partial_path(keep);
        entries.sort_by_key(|(_, _, modified)| *modified);
        for (path, size, _) in entries {
            if total <= self.budgets.max_partial_bytes {
                break;
            }
            if path == keep_path {
                continue;
            }
            fs::remove_file(&path).map_err(io_err("cache sweep partial budget"))?;
            total = total.saturating_sub(size);
        }
        Ok(())
    }

    /// Finish a resumable download: the accumulated digest must equal the
    /// expected digest or the partial is DELETED (its bytes are provably not
    /// the content) and the commit refuses. On success the file moves
    /// atomically into `objects/`.
    pub fn commit_partial(
        &mut self,
        writer: PartialWriter,
        now_ms: u64,
    ) -> ClientResult<PathBuf> {
        let PartialWriter { file, path, expected, hasher, written, .. } = writer;
        let digest = hasher.finalize();
        if digest != expected {
            drop(file);
            let _ = fs::remove_file(&path);
            return Err(ClientError::DigestMismatch {
                what: "cache commit partial",
                expected,
                found: digest,
            });
        }
        if let Some(&size) = self.index.get(&digest) {
            // Already present (raced with another fetch path): the existing
            // object wins; sizes must agree.
            drop(file);
            let _ = fs::remove_file(&path);
            if size != written {
                return Err(ClientError::SizeMismatch {
                    what: "cache dedup object",
                    expected: written,
                    found: size,
                });
            }
            self.touch(&digest, now_ms)?;
            return Ok(self.object_path(&digest));
        }
        self.admit(written)?;
        // Same policy as `put_bytes`: flush, do not barrier. A resumed
        // partial is re-hashed from disk before it is extended, and a
        // digest mismatch at commit deletes it, so torn tails are already
        // handled by construction.
        let mut file = file;
        file.flush().map_err(io_err("cache flush partial"))?;
        drop(file);
        self.commit_file(&path, &digest, written, now_ms)?;
        Ok(self.object_path(&digest))
    }

    /// Bytes already resumable for `expected` (0 when no partial exists).
    pub fn partial_len(&self, expected: &[u8; 32]) -> u64 {
        fs::metadata(self.partial_path(expected)).map(|m| m.len()).unwrap_or(0)
    }

    /// Drop any partial for `expected` (used after a digest mismatch so the
    /// next attempt starts clean).
    pub fn discard_partial(&mut self, expected: &[u8; 32]) -> ClientResult<()> {
        match fs::remove_file(self.partial_path(expected)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err("cache discard partial")(e)),
        }
    }

    // ---- verified reads ----------------------------------------------------

    /// Verified resolve: re-hash the object on disk and return its path only
    /// if the digest matches. A corrupt object is removed (with its markers)
    /// and reported absent, so a caller can re-fetch; the corruption is
    /// counted in [`CacheStats`].
    pub fn resolve(&mut self, digest: &[u8; 32], now_ms: u64) -> ClientResult<Option<PathBuf>> {
        if self.verify_object(digest)? {
            self.touch(digest, now_ms)?;
            Ok(Some(self.object_path(digest)))
        } else {
            Ok(None)
        }
    }

    /// Verified whole-object read into memory.
    pub fn read_verified(
        &mut self,
        digest: &[u8; 32],
        now_ms: u64,
    ) -> ClientResult<Option<Vec<u8>>> {
        let Some(path) = self.resolve(digest, now_ms)? else {
            return Ok(None);
        };
        // resolve() verified the bytes an instant ago; this read is the
        // same single-owner file.
        fs::read(&path).map(Some).map_err(io_err("cache read object"))
    }

    /// Re-hash an object; on mismatch remove it and return false.
    fn verify_object(&mut self, digest: &[u8; 32]) -> ClientResult<bool> {
        if !self.index.contains_key(digest) {
            return Ok(false);
        }
        let path = self.object_path(digest);
        let mut file = match File::open(&path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Index said present but the file is gone: heal the index.
                if let Some(size) = self.index.remove(digest) {
                    self.total_bytes = self.total_bytes.saturating_sub(size);
                }
                return Ok(false);
            }
            Err(e) => return Err(io_err("cache open object")(e)),
        };
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf).map_err(io_err("cache read object"))?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        drop(file);
        if &hasher.finalize() == digest {
            return Ok(true);
        }
        self.remove_object(digest)?;
        self.corruption_evictions += 1;
        Ok(false)
    }
}

/// A resumable hash-while-write download. Dropping it KEEPS the partial file
/// (that is the resume point); [`ContentCache::discard_partial`] removes it
/// explicitly.
pub struct PartialWriter {
    file: File,
    path: PathBuf,
    expected: [u8; 32],
    hasher: Sha256,
    written: u64,
    max: u64,
}

impl PartialWriter {
    /// Bytes already on disk and hashed; the Range offset to resume from.
    pub fn resumed_bytes(&self) -> u64 {
        self.written
    }

    pub fn write(&mut self, data: &[u8]) -> ClientResult<()> {
        let new_total = self
            .written
            .checked_add(data.len() as u64)
            .ok_or(ClientError::CacheAdmission {
                what: "partial bytes overflow",
                limit: self.max,
                found: u64::MAX,
            })?;
        if new_total > self.max {
            return Err(ClientError::CacheAdmission {
                what: "partial over per-object budget",
                limit: self.max,
                found: new_total,
            });
        }
        self.file.write_all(data).map_err(io_err("cache write partial"))?;
        self.hasher.update(data);
        self.written = new_total;
        Ok(())
    }

    /// Restart from zero (the server answered a resume request with a full
    /// 200 body): truncate the file and reset the hash state.
    pub fn reset(&mut self) -> ClientResult<()> {
        self.file.set_len(0).map_err(io_err("cache truncate partial"))?;
        // append-mode positions at (new) EOF on the next write.
        self.hasher = Sha256::new();
        self.written = 0;
        Ok(())
    }

    pub fn expected(&self) -> &[u8; 32] {
        &self.expected
    }
}

/// `<64hex>.part` → digest.
fn partial_digest_of(path: &Path) -> Option<[u8; 32]> {
    let name = path.file_name()?.to_str()?;
    let hex = name.strip_suffix(".part")?;
    from_hex_exact::<32>(hex)
}

#[cfg(test)]
mod ram_tests {
    use super::RamCache;
    use std::sync::{Arc, Mutex};

    fn digest(byte: u8) -> [u8; 32] {
        [byte; 32]
    }

    #[test]
    fn holds_bytes_and_counts_them() {
        let mut ram = RamCache::new(1024);
        ram.insert(digest(1), vec![7u8; 256]);
        assert_eq!(ram.used_bytes(), 256);
        assert_eq!(ram.get(&digest(1)).map(Vec::len), Some(256));
    }

    #[test]
    fn the_least_recently_used_object_is_evicted_first() {
        let mut ram = RamCache::new(600);
        ram.insert(digest(1), vec![0u8; 256]);
        ram.insert(digest(2), vec![0u8; 256]);
        // Touch 1 so 2 is the coldest.
        assert!(ram.get(&digest(1)).is_some());
        ram.insert(digest(3), vec![0u8; 256]);
        assert!(ram.get(&digest(1)).is_some(), "recently used survives");
        assert!(ram.get(&digest(3)).is_some(), "the newcomer survives");
        assert!(ram.get(&digest(2)).is_none(), "the cold one was evicted");
        assert!(ram.used_bytes() <= ram.budget_bytes());
    }

    #[test]
    fn an_object_larger_than_the_budget_is_never_admitted() {
        let mut ram = RamCache::new(128);
        ram.insert(digest(1), vec![0u8; 64]);
        ram.insert(digest(2), vec![0u8; 4096]);
        assert!(ram.get(&digest(2)).is_none(), "one giant object must not move in");
        assert!(
            ram.get(&digest(1)).is_some(),
            "and must not evict what was already resident"
        );
    }

    #[test]
    fn forget_frees_its_bytes_and_reports_residency() {
        let mut ram = RamCache::new(1024);
        ram.insert(digest(1), vec![0u8; 512]);
        assert!(ram.forget(&digest(1)));
        assert_eq!(ram.used_bytes(), 0);
        assert!(!ram.forget(&digest(1)), "forgetting twice is honest about it");
        assert!(ram.get(&digest(1)).is_none());
    }

    #[test]
    fn re_inserting_the_same_digest_does_not_double_count() {
        let mut ram = RamCache::new(1024);
        ram.insert(digest(1), vec![0u8; 300]);
        ram.insert(digest(1), vec![0u8; 300]);
        assert_eq!(ram.used_bytes(), 300);
        assert_eq!(ram.len(), 1);
    }

    /// Lanes share one cache behind a mutex (that is how `AssetClient`
    /// clones hold it): the budget must hold no matter how the interleaving
    /// falls, and the used-bytes accounting must survive it.
    #[test]
    fn the_budget_holds_under_concurrent_lanes() {
        const BUDGET: u64 = 8 * 1024;
        let ram = Arc::new(Mutex::new(RamCache::new(BUDGET)));
        let mut lanes = Vec::new();
        for lane in 0..8u8 {
            let ram = Arc::clone(&ram);
            lanes.push(std::thread::spawn(move || {
                for i in 0..64u8 {
                    let d = digest(lane.wrapping_mul(64).wrapping_add(i));
                    ram.lock().unwrap().insert(d, vec![lane; 1024]);
                    // Reading a hot object mid-flight must not corrupt the
                    // accounting either.
                    let _ = ram.lock().unwrap().get(&d).map(Vec::len);
                }
            }));
        }
        for lane in lanes {
            lane.join().unwrap();
        }
        let mut ram = ram.lock().unwrap();
        assert!(
            ram.used_bytes() <= BUDGET,
            "budget blown: {} > {BUDGET}",
            ram.used_bytes()
        );
        // Accounting is exactly the sum of what is still resident.
        let resident: u64 = (0..=255u8)
            .filter_map(|b| ram.get(&digest(b)).map(|v| v.len() as u64))
            .sum();
        assert_eq!(resident, ram.used_bytes());
    }
}
