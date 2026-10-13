//! Disk cache store files (plans/relax-cache.md): ONE file per crate version under
//! `<target>/relax/cache/<crate>-<16 hex>.rcs` (dirs.rs) (the hex: Relax build id, target and the crate's
//! fingerprint), shared by every program that uses that crate.
//!
//! Layout:
//!   header (HEADER bytes): magic "RCS1", format u32, active slot u32, slot capacity u32 (index
//!     entries per slot), then per slot: entry count u64, data end u64, index checksum u64;
//!     header checksum u64 last
//!   index slot 0, index slot 1 (capacity x INDEX_ENTRY bytes each): sorted (key u128, offset u64,
//!     length u32, kind u32, payload checksum u64)
//!   entries (8-byte aligned payloads) up to the active slot's data end
//! Readers take a shared lock, copy the active slot's index and map the file; payloads below a
//! committed data end never change. A writer (exclusive lock) appends payloads past data end,
//! writes the merged index into the inactive slot, then flips the header: a crash leaves the old
//! header and index valid. Crash safety is ordering, not durability (the stores are a rebuildable
//! cache: a lost commit is a cache miss): readers do not checksum payloads, so the payloads must
//! reach the disk before a header that commits them -- one write barrier (`order_writes`) between
//! them. The index and header carry checksums, so a torn index or header only invalidates the store. When the merged index does not fit a slot, or dead bytes outgrow a
//! quarter of the live ones (or the store is stale), the store is rewritten whole as its next generation.
//! The newest entry of a key wins.
//!
//! Generations (one design on every OS): a store is never renamed over or deleted while it may
//! be open. A file another process has mapped or open cannot be replaced on Windows, and a
//! running program keeps its stores open. `<name>` is generation 0, `<name>.<g>` generation g;
//! a rewrite writes the next generation to a temp file and renames it to its own new name, and
//! every reader and appender uses the highest generation present (current()). The lower ones are
//! deleted by the next start's prune (cache.rs) unless that process itself reads them; a delete that
//! fails (a file still open elsewhere on Windows) is retried at the start after. Files are
//! opened with std's default sharing (FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE on
//! Windows), so a reader never blocks those deletes.

use crate::pack::{Bytes, NewEntry};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const MAGIC: &[u8; 4] = b"RCS1";
const FORMAT: u32 = 1;
const HEADER: usize = 80;
const INDEX_ENTRY: usize = 40;
const MIN_CAP: u32 = 256;

/// One index entry: (key, offset, length, kind, checksum).
#[derive(Clone, Copy)]
#[repr(C, packed(8))]
pub struct Ix {
    pub key: u128,
    pub off: u64,
    pub len: u32,
    pub kind: u32,
    pub sum: u64,
}

// The disk index is exactly these 40 little-endian bytes, at 8-byte alignment.
unsafe impl crate::image::Pod for Ix {}

/// Incremental form of the store checksum. Chunk boundaries do not affect the hash.
struct Sum64 {
    h: [u64; 4],
    tail: [u8; 32],
    used: usize,
    len: u64,
}
const P: u64 = 0x9e37_79b9_7f4a_7c15;
const Q: u64 = 0xc2b2_ae3d_27d4_eb4f;
impl Sum64 {
    fn new() -> Self { Self { h: [P, Q, P ^ Q, P.rotate_left(17)], tail: [0; 32], used: 0, len: 0 } }
    fn block(&mut self, b: &[u8]) {
        for k in 0..4 {
            let w = u64::from_le_bytes(b[k * 8..k * 8 + 8].try_into().unwrap());
            self.h[k] = (self.h[k] ^ w).wrapping_mul(Q).rotate_left(29);
        }
    }
    fn update(&mut self, mut b: &[u8]) {
        self.len += b.len() as u64;
        if self.used != 0 {
            let n = (32 - self.used).min(b.len());
            self.tail[self.used..self.used + n].copy_from_slice(&b[..n]);
            self.used += n;
            b = &b[n..];
            if self.used < 32 { return; }
            self.block(&self.tail.clone());
            self.used = 0;
        }
        let mut chunks = b.chunks_exact(32);
        for b in &mut chunks { self.block(b); }
        let b = chunks.remainder();
        self.tail[..b.len()].copy_from_slice(b);
        self.used = b.len();
    }
    fn finish(mut self) -> u64 {
        self.tail[self.used..].fill(0);
        self.block(&self.tail.clone());
        let mut x = self.len.wrapping_mul(P);
        for h in self.h { x = (x ^ h).wrapping_mul(P).rotate_left(31); }
        x ^ (x >> 29)
    }
}

/// A 64-bit word hash (corruption check; the files are private, so not attack-resistant).
pub fn sum64(b: &[u8]) -> u64 {
    let mut sum = Sum64::new();
    sum.update(b);
    sum.finish()
}

/// An entry can encode itself directly into a store, borrowing its existing payload arrays.
pub trait Entry {
    fn key(&self) -> u128;
    fn kind(&self) -> u32;
    fn write_payload(&self, out: &mut dyn Write) -> std::io::Result<()>;
}
impl Entry for NewEntry {
    fn key(&self) -> u128 { self.key }
    fn kind(&self) -> u32 { self.kind }
    fn write_payload(&self, out: &mut dyn Write) -> std::io::Result<()> { out.write_all(&self.payload) }
}

struct PayloadWriter<'a> {
    out: &'a mut dyn Write,
    sum: Sum64,
}
impl Write for PayloadWriter<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.sum.len + b.len() as u64 > u32::MAX as u64 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "store payload exceeds u32"));
        }
        let n = self.out.write(b)?;
        self.sum.update(&b[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> { self.out.flush() }
}
fn write_entry(out: &mut dyn Write, off: &mut u64, e: &impl Entry) -> std::io::Result<Ix> {
    let at = align8(*off);
    out.write_all(&[0; 7][..(at - *off) as usize])?;
    let mut w = PayloadWriter { out, sum: Sum64::new() };
    e.write_payload(&mut w)?;
    let len = w.sum.len as u32;
    let sum = w.sum.finish();
    *off = at + len as u64;
    Ok(Ix { key: e.key(), kind: e.kind(), off: at, len, sum })
}

struct Header {
    active: u32,
    cap: u32,
    slots: [(u64, u64, u64); 2],
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl Header {
    fn parse(b: &[u8]) -> Option<Header> {
        if b.len() < HEADER || &b[0..4] != MAGIC || u32_at(b, 4) != FORMAT {
            return None;
        }
        if sum64(&b[..HEADER - 8]) != u64_at(b, HEADER - 8) {
            return None;
        }
        let active = u32_at(b, 8);
        let cap = u32_at(b, 12);
        if active > 1 || cap == 0 {
            return None;
        }
        let slot = |k: usize| (u64_at(b, 16 + k * 24), u64_at(b, 24 + k * 24), u64_at(b, 32 + k * 24));
        Some(Header { active, cap, slots: [slot(0), slot(1)] })
    }

    fn bytes(&self) -> [u8; HEADER] {
        let mut b = [0u8; HEADER];
        b[0..4].copy_from_slice(MAGIC);
        b[4..8].copy_from_slice(&FORMAT.to_le_bytes());
        b[8..12].copy_from_slice(&self.active.to_le_bytes());
        b[12..16].copy_from_slice(&self.cap.to_le_bytes());
        for k in 0..2 {
            let (n, end, sum) = self.slots[k];
            b[16 + k * 24..24 + k * 24].copy_from_slice(&n.to_le_bytes());
            b[24 + k * 24..32 + k * 24].copy_from_slice(&end.to_le_bytes());
            b[32 + k * 24..40 + k * 24].copy_from_slice(&sum.to_le_bytes());
        }
        let s = sum64(&b[..HEADER - 8]);
        b[HEADER - 8..].copy_from_slice(&s.to_le_bytes());
        b
    }

    fn slot_off(&self, k: u32) -> u64 {
        HEADER as u64 + k as u64 * self.cap as u64 * INDEX_ENTRY as u64
    }

    fn data_start(&self) -> u64 {
        align8(HEADER as u64 + 2 * self.cap as u64 * INDEX_ENTRY as u64)
    }
}

fn align8(x: u64) -> u64 {
    (x + 7) & !7
}

fn encode_index(ix: &[Ix]) -> Vec<u8> {
    let mut b = Vec::with_capacity(ix.len() * INDEX_ENTRY);
    for e in ix {
        b.extend_from_slice(&e.key.to_le_bytes());
        b.extend_from_slice(&e.off.to_le_bytes());
        b.extend_from_slice(&e.len.to_le_bytes());
        b.extend_from_slice(&e.kind.to_le_bytes());
        b.extend_from_slice(&e.sum.to_le_bytes());
    }
    b
}

fn decode_index(b: &[u8]) -> Vec<Ix> {
    b.chunks_exact(INDEX_ENTRY)
        .map(|c| Ix {
            key: u128::from_le_bytes(c[0..16].try_into().unwrap()),
            off: u64_at(c, 16),
            len: u32_at(c, 24),
            kind: u32_at(c, 28),
            sum: u64_at(c, 32),
        })
        .collect()
}

/// Store files this process opened or wrote (generation 0 paths). The cap never evicts them,
/// nor any of their generations (cache.rs prune_root): evicting a store the run just restored
/// from made every dependent re-parse on the next run.
static LIVE: relax_session::sync::Mutex<Option<std::collections::HashSet<PathBuf>>> = relax_session::sync::Mutex::new(None);

/// Records the store `base` as used by this run. The first time, an opened store's file also
/// gets its modification time stamped: the order the cap evicts in, and how another run sees it
/// in use.
fn mark_live(base: &Path, file: &Path, stamp: bool) {
    let _k = crate::zone_alloc::kept();
    OPEN_FILES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(file.to_path_buf());
    let new = LIVE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(base.to_path_buf());
    if new && stamp {
        let _ = std::fs::File::options().write(true).open(file).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    }
}

/// The generation files this process opened (a stale generation it still reads is not deleted).
static OPEN_FILES: relax_session::sync::Mutex<Option<std::collections::HashSet<PathBuf>>> = relax_session::sync::Mutex::new(None);

/// The generation files this process opened.
pub fn open_files() -> std::collections::HashSet<PathBuf> {
    OPEN_FILES.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// The store files this process opened or wrote (generation 0 paths).
pub fn live() -> std::collections::HashSet<PathBuf> {
    LIVE.lock().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

/// `name` split into its generation 0 name and its generation: `x.rcs.3` -> (`x.rcs`, 3); a
/// name without a generation suffix is generation 0.
pub fn split_gen(name: &str) -> (&str, u32) {
    if let Some((base, g)) = name.rsplit_once('.') {
        if base.ends_with(".rcs") && !g.is_empty() && g.len() <= 9 && g.bytes().all(|b| b.is_ascii_digit()) && !g.starts_with('0') {
            return (base, g.parse().unwrap_or(0));
        }
    }
    (name, 0)
}

/// Generation `g` of the store whose generation 0 is `base`.
pub fn gen_path(base: &Path, g: u32) -> PathBuf {
    if g == 0 {
        return base.to_path_buf();
    }
    let mut n = base.as_os_str().to_os_string();
    n.push(format!(".{g}"));
    PathBuf::from(n)
}

/// The generation 0 path of a store path (any generation).
pub fn base_path(p: &Path) -> PathBuf {
    match p.file_name().and_then(|n| n.to_str()) {
        Some(n) => p.with_file_name(split_gen(n).0),
        None => p.to_path_buf(),
    }
}

/// Highest generation seen per store (generation 0 path), and the folders listed for it.
static GENS: relax_session::sync::Mutex<Option<(std::collections::HashMap<PathBuf, u32>, std::collections::HashSet<PathBuf>)>> = relax_session::sync::Mutex::new(None);

/// Lists `dir` into GENS (each store's highest generation present).
fn list_gens(dir: &Path, gens: &mut std::collections::HashMap<PathBuf, u32>) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let n = e.file_name();
        let Some(n) = n.to_str() else { continue };
        let (b, g) = split_gen(n);
        let k = gens.entry(dir.join(b)).or_insert(0);
        *k = (*k).max(g);
    }
}

/// The store's current file: its highest generation present (generation 0 when none exists).
pub fn current(path: &Path) -> PathBuf {
    current_gen(path).1
}

fn current_gen(path: &Path) -> (u32, PathBuf) {
    let _k = crate::zone_alloc::kept();
    let base = base_path(path);
    let dir = base.parent().unwrap_or(Path::new(".")).to_path_buf();
    let mut guard = GENS.lock().unwrap_or_else(|e| e.into_inner());
    let (gens, listed) = guard.get_or_insert_with(Default::default);
    if listed.insert(dir.clone()) {
        list_gens(&dir, gens);
    }
    let mut g = gens.get(&base).copied().unwrap_or(0);
    // another process may have written newer generations, or deleted the one listed
    if !gen_path(&base, g).exists() {
        list_gens(&dir, gens);
        g = gens.get(&base).copied().unwrap_or(0);
    }
    while gen_path(&base, g + 1).exists() {
        g += 1;
    }
    gens.insert(base.clone(), g);
    (g, gen_path(&base, g))
}

/// The generation a rewrite of the store writes: the current one when it does not exist yet,
/// else the first free one above it.
fn next_gen(base: &Path) -> (u32, PathBuf) {
    let (mut g, p) = current_gen(base);
    if !p.exists() {
        return (g, p);
    }
    loop {
        g += 1;
        let p = gen_path(base, g);
        if !p.exists() {
            return (g, p);
        }
    }
}

fn written_gen(base: &Path, g: u32) {
    let _k = crate::zone_alloc::kept();
    let mut guard = GENS.lock().unwrap_or_else(|e| e.into_inner());
    let (gens, _) = guard.get_or_insert_with(Default::default);
    let k = gens.entry(base.to_path_buf()).or_insert(0);
    *k = (*k).max(g);
}

/// The stores under `dir` whose generation 0 name starts with `prefix` and ends in `.rcs` (one
/// generation 0 path per store, sorted).
pub fn stores_with_prefix(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().to_str().map(|n| split_gen(n).0.to_string()))
        .filter(|b| b.starts_with(prefix) && b.ends_with(".rcs"))
        .map(|b| dir.join(b)).collect();
    v.sort();
    v.dedup();
    v
}

/// Remember a mapping's generation without keeping a descriptor for every store.
#[cfg(unix)]
struct ReadSource {
    path: PathBuf,
    identity: (u64, u64),
}
#[cfg(unix)]
impl ReadSource {
    fn new(path: &Path, file: &std::fs::File) -> Option<Self> {
        use std::os::unix::fs::MetadataExt;
        let meta = file.metadata().ok()?;
        Some(Self { path: path.to_owned(), identity: (meta.dev(), meta.ino()) })
    }
    fn open(&self) -> Option<std::fs::File> {
        use std::os::unix::fs::MetadataExt;
        let file = std::fs::File::open(&self.path).ok()?;
        let meta = file.metadata().ok()?;
        // Appends retain the inode and never overwrite committed payloads. A mapping
        // pins that inode even if unlinked, so a replacement cannot reuse its identity.
        ((meta.dev(), meta.ino()) == self.identity).then_some(file)
    }
}

/// A store file opened for reading: its index (copied) and its mapped bytes.
pub struct Store {
    /// the store's generation 0 path (its identity; the file read is its current generation)
    pub path: PathBuf,
    pub index: crate::arena::Arena<Ix>,
    pub(crate) generation: u64,
    data: Bytes,
    #[cfg(unix)]
    source: Option<ReadSource>,
    data_end: u64,
}

impl Store {
    /// Opens `path` (None: missing, unreadable or not a valid store).
    pub fn open(path: &Path) -> Option<Store> {
        let base = base_path(path);
        let file = current(&base);
        let mut f = std::fs::File::open(&file).ok()?;
        f.lock_shared().ok()?;
        let r = (|| {
            let mut hb = [0u8; HEADER];
            f.read_exact(&mut hb).ok()?;
            let h = Header::parse(&hb)?;
            let (n, end, sum) = h.slots[h.active as usize];
            if n > h.cap as u64 {
                return None;
            }
            let mut ib = vec![0u8; n as usize * INDEX_ENTRY];
            f.seek(SeekFrom::Start(h.slot_off(h.active))).ok()?;
            f.read_exact(&mut ib).ok()?;
            if sum64(&ib) != sum {
                return None;
            }
            let data = Bytes::map(&file)?;
            if (data.len() as u64) < end {
                return None;
            }
            Some(Store { path: base.clone(), index: decode_index(&ib).into(), generation: sum64(&hb), data,
                #[cfg(unix)] source: ReadSource::new(&file, &f),
                data_end: end })
        })();
        let _ = f.unlock();
        if r.is_some() {
            mark_live(&base, &file, true);
        }
        r
    }

    /// Use an immutable index snapshot from a checksummed collected image. The store's
    /// commit header must still identify that exact generation. Appends can overwrite the
    /// store's two index slots, so those slots themselves must never be borrowed by readers.
    pub(crate) fn open_certified(path: &Path, index: &'static [Ix], generation: u64) -> Option<Self> {
        let base = base_path(path);
        let file = current(&base);
        let f = std::fs::File::open(&file).ok()?;
        f.lock_shared().ok()?;
        let result = (|| {
            let data = Bytes::map(&file)?;
            let hb = data.get(..HEADER)?;
            let h = Header::parse(hb)?;
            if sum64(hb) != generation { return None; }
            let (n, end, _) = h.slots[h.active as usize];
            if n != index.len() as u64 || n > h.cap as u64 || end > data.len() as u64 { return None; }
            Some(Self { path: base.clone(), index: crate::arena::Arena::Mapped(index), generation, data,
                #[cfg(unix)] source: ReadSource::new(&file, &f),
                data_end: end })
        })();
        let _ = f.unlock();
        if result.is_some() { mark_live(&base, &file, true); }
        result
    }

    /// The payload of index entry `i` if its key is `key`: bounds-checked, NOT checksummed. A
    /// store's index and header are checksummed when it opens and a rewrite re-checks every
    /// payload it carries; the payload checksums run in the background for front images
    /// (`verify_at`), so a warm start does not read every page of what it maps. A torn write is
    /// the store file generations' business, not the reader's.
    pub fn get_at(&self, i: usize, key: u128) -> Option<(u32, &[u8])> {
        let e = self.index.get(i)?;
        if e.key != key || e.off + e.len as u64 > self.data_end {
            return None;
        }
        let p = self.data.get(e.off as usize..e.off as usize + e.len as usize)?;
        Some((e.kind, p))
    }

    /// Whether index entry `i`'s payload matches its checksum.
    pub fn verify_at(&self, i: usize) -> bool {
        let Some(e) = self.index.get(i) else { return false };
        if e.off.checked_add(e.len as u64).is_none_or(|end| end > self.data_end) { return false; }
        #[cfg(unix)]
        if let Some(file) = self.source.as_ref().and_then(ReadSource::open) {
            // Do not fault every mapped payload page into this process just to hash it.
            // Only this verification holds a descriptor, not every pinned Store.
            let mut buf = [0u8; 64 * 1024];
            let mut sum = Sum64::new();
            let mut off = e.off;
            let mut left = e.len as usize;
            while left != 0 {
                let n = left.min(buf.len());
                if std::os::unix::fs::FileExt::read_exact_at(&file, &mut buf[..n], off).is_err() { return false; }
                sum.update(&buf[..n]);
                left -= n;
                off += n as u64;
            }
            return sum.finish() == e.sum;
        }
        // A removed/replaced generation is still pinned by its mapping. If it cannot
        // be reopened, verify those exact bytes. Non-Unix Bytes already owns its data.
        self.data.get(e.off as usize..e.off as usize + e.len as usize).is_some_and(|p| sum64(p) == e.sum)
    }

    /// Compare a streamed entry with the checked index, then verify the old payload
    /// without mapping it in. A damaged old payload must still be replaced.
    pub fn matches_entry(&self, entry: &impl Entry, len: usize) -> bool {
        let Ok(i) = self.index.binary_search_by_key(&entry.key(), |e| e.key) else { return false; };
        let e = &self.index[i];
        if e.kind != entry.kind() || e.len as usize != len { return false; }
        let mut sink = std::io::sink();
        let mut out = PayloadWriter { out: &mut sink, sum: Sum64::new() };
        entry.write_payload(&mut out).is_ok() && out.sum.len == len as u64
            && out.sum.finish() == e.sum && self.verify_at(i)
    }

    /// Whether the entry `key` holds exactly a payload of `len` bytes with checksum `sum`
    /// (from `entry_sum`), checked against the payload on disk.
    pub fn matches_sum(&self, key: u128, kind: u32, len: u32, sum: u64) -> bool {
        let Ok(i) = self.index.binary_search_by_key(&key, |e| e.key) else { return false; };
        let e = self.index[i];
        e.kind == kind && e.len == len && e.sum == sum && self.verify_at(i)
    }

    /// The payload of `key` (binary search).
    pub fn get(&self, key: u128) -> Option<(u32, &[u8])> {
        let i = self.index.binary_search_by(|e| { let k = e.key; k.cmp(&key) }).ok()?;
        self.get_at(i, key)
    }

    pub fn file_len(&self) -> u64 {
        self.data_end
    }
}

/// The length and checksum an entry's payload gets in a store, from streaming it once.
pub fn entry_sum(entry: &impl Entry) -> Option<(u32, u64)> {
    let mut sink = std::io::sink();
    let mut out = PayloadWriter { out: &mut sink, sum: Sum64::new() };
    entry.write_payload(&mut out).ok()?;
    let len = u32::try_from(out.sum.len).ok()?;
    Some((len, out.sum.finish()))
}

/// The generation (Store::generation: the header checksum) of the store file's current
/// header, read without mapping the file; None: unreadable or not a store.
pub fn header_generation(path: &Path) -> Option<u64> {
    let mut f = std::fs::File::open(current(&base_path(path))).ok()?;
    f.lock_shared().ok()?;
    let mut hb = [0u8; HEADER];
    let r = f.read_exact(&mut hb).ok().and_then(|_| Header::parse(&hb)).map(|_| sum64(&hb));
    let _ = f.unlock();
    r
}

/// Adds `entries` to the store at `path` (creating it), newest winning per key; `drop_all`: start
/// from an empty store (its contents were found stale). Atomic: an append commits by flipping
/// the header; a rewrite goes through a temp file and rename.
pub fn append<T: Entry>(path: &Path, entries: Vec<T>, drop_all: bool) -> std::io::Result<()> {
    let base = base_path(path);
    let r = append_inner(&base, entries, drop_all);
    if r.is_ok() {
        mark_live(&base, &base, false);
    }
    r
}

fn append_inner<T: Entry>(base: &Path, mut entries: Vec<T>, drop_all: bool) -> std::io::Result<()> {
    // newest first within this batch: a stable sort keeps the first of equal keys
    entries.sort_by_key(|a| a.key());
    entries.dedup_by(|b, a| a.key() == b.key());
    let dir = base.parent().unwrap_or(Path::new("."));
    // the current generation, locked; a newer one written meanwhile (another process's
    // rewrite) is taken instead
    let mut f = loop {
        let (g, path) = current_gen(base);
        let f = match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
            Ok(f) => f,
            Err(_) => return rewrite(base, dir, Vec::new(), None, entries),
        };
        f.lock()?;
        if !gen_path(base, g + 1).exists() {
            break f;
        }
        let _ = f.unlock();
    };
    let path = base;
    let r = (|| -> std::io::Result<Option<(Vec<Ix>, Header)>> {
        let mut hb = [0u8; HEADER];
        if f.read_exact(&mut hb).is_err() {
            return Ok(None);
        }
        let Some(h) = Header::parse(&hb) else { return Ok(None) };
        let (n, end, sum) = h.slots[h.active as usize];
        if n > h.cap as u64 {
            return Ok(None);
        }
        let mut ib = vec![0u8; n as usize * INDEX_ENTRY];
        f.seek(SeekFrom::Start(h.slot_off(h.active)))?;
        f.read_exact(&mut ib)?;
        if sum64(&ib) != sum {
            return Ok(None);
        }
        let _ = end;
        Ok(Some((decode_index(&ib), h)))
    })()?;
    let (old, h) = match r {
        Some(x) if !drop_all => x,
        // unreadable or stale: replaced whole
        _ => {
            let res = rewrite(path, dir, Vec::new(), None, entries);
            let _ = f.unlock();
            return res;
        }
    };
    let (_, end, _) = h.slots[h.active as usize];
    // the merged index: new entries replace old ones of the same key
    let new_keys: std::collections::HashSet<u128> = entries.iter().map(|e| e.key()).collect();
    let merged_n = old.iter().filter(|e| !new_keys.contains(&{ e.key })).count() + entries.len();
    if merged_n as u64 > h.cap as u64 {
        let res = rewrite(path, dir, old, Some(&f), entries);
        let _ = f.unlock();
        return res;
    }
    // A bounded buffer coalesces small fields; large code/image slices go straight to disk.
    let mut off = end;
    f.seek(SeekFrom::Start(off))?;
    let mut ix: Vec<Ix> = old.into_iter().filter(|e| !new_keys.contains(&{ e.key })).collect();
    {
        let mut out = std::io::BufWriter::with_capacity(64 * 1024, &mut f);
        for e in entries { ix.push(write_entry(&mut out, &mut off, &e)?); }
        out.flush()?;
    }
    ix.sort_by_key(|e| e.key);
    // Encoded lengths are now known. Use the same compaction threshold without pre-encoding
    // all new images. Only a dead-space compaction needs to copy the just-appended bytes.
    let live: u64 = ix.iter().map(|e| e.len as u64).sum();
    let dead = end.saturating_sub(h.data_start()).saturating_sub(live.min(end));
    if dead * 4 > live && dead > 1 << 20 {
        let res = rewrite::<T>(path, dir, ix, Some(&f), Vec::new());
        let _ = f.unlock();
        return res;
    }
    crate::memstat::print_tag("cache append", crate::memstat::Tag::CacheStore);
    let ib = encode_index(&ix);
    let next = 1 - h.active;
    // the payloads before the index and header that commit them; nothing after the header (a
    // commit lost to a power cut is a cache miss, and other processes read the page cache)
    order_writes(&f)?;
    f.seek(SeekFrom::Start(h.slot_off(next)))?;
    f.write_all(&ib)?;
    let mut nh = Header { active: next, cap: h.cap, slots: h.slots };
    nh.slots[next as usize] = (ix.len() as u64, off, sum64(&ib));
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&nh.bytes())?;
    let _ = f.unlock();
    Ok(())
}

/// MAKEPAD_RUN_PROFILE is set: the load timeline's spans for the image writes.
pub fn profiling() -> bool {
    static P: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *P.get_or_init(|| std::env::var_os("MAKEPAD_RUN_PROFILE").is_some())
}

/// Makes the bytes written to `f` so far reach the disk before anything written after: an
/// ordering barrier, not a durability flush. macOS: `F_BARRIERFSYNC` -- std's `sync_data` is
/// `F_FULLFSYNC` there, a full flush of the drive's cache per call (a cold Stage launch made ~3.7k
/// of them); elsewhere (and where the barrier is refused) `sync_data`.
fn order_writes(f: &std::fs::File) -> std::io::Result<()> {
    #[cfg(target_vendor = "apple")]
    {
        use std::os::fd::AsRawFd;
        extern "C" {
            fn fcntl(fd: i32, cmd: i32, ...) -> i32;
        }
        const F_BARRIERFSYNC: i32 = 85;
        if unsafe { fcntl(f.as_raw_fd(), F_BARRIERFSYNC) } != -1 {
            return Ok(());
        }
    }
    f.sync_data()
}

/// Writes a fresh store with the live entries of `old` (read from `from`) plus `entries`, to a temp
/// file, then renames it to the store's next generation (never over an existing file).
fn rewrite<T: Entry>(base: &Path, dir: &Path, old: Vec<Ix>, from: Option<&std::fs::File>, entries: Vec<T>) -> std::io::Result<()> {
    let new_keys: std::collections::HashSet<u128> = entries.iter().map(|e| e.key()).collect();
    let old: Vec<_> = old.into_iter().filter(|e| !new_keys.contains(&{ e.key })).collect();
    let n = old.len() + entries.len();
    let cap = (n as u32 + n as u32 / 4).max(MIN_CAP);
    let mut h = Header { active: 0, cap, slots: [(0, 0, 0); 2] };
    let (g, path) = next_gen(base);
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    crate::pack::write_atomic_with(dir, &name, |f| {
        let mut off = h.data_start();
        f.seek(SeekFrom::Start(off))?;
        let mut out = std::io::BufWriter::with_capacity(64 * 1024, f);
        let mut ix = Vec::with_capacity(n);
        if let Some(mut source) = from {
            let mut buf = [0u8; 64 * 1024];
            for e in old {
                let at = align8(off);
                out.write_all(&[0; 7][..(at - off) as usize])?;
                source.seek(SeekFrom::Start(e.off))?;
                let mut sum = Sum64::new();
                let mut left = e.len as usize;
                while left != 0 {
                    let n = left.min(buf.len());
                    source.read_exact(&mut buf[..n])?;
                    sum.update(&buf[..n]);
                    out.write_all(&buf[..n])?;
                    left -= n;
                }
                if sum.finish() == e.sum {
                    ix.push(Ix { off: at, ..e });
                    off = at + e.len as u64;
                } else {
                    // Preserve validation: corrupt old payloads are not carried forward.
                    out.seek(SeekFrom::Start(off))?;
                }
            }
        }
        for e in entries { ix.push(write_entry(&mut out, &mut off, &e)?); }
        ix.sort_by_key(|e| e.key);
        crate::memstat::print_tag("cache rewrite", crate::memstat::Tag::CacheStore);
        let ib = encode_index(&ix);
        h.slots[0] = (ix.len() as u64, off, sum64(&ib));
        out.seek(SeekFrom::Start(h.slot_off(0)))?;
        out.write_all(&ib)?;
        // the payloads before the header that commits them (as `append`)
        out.flush()?;
        order_writes(out.get_ref())?;
        out.seek(SeekFrom::Start(0))?;
        out.write_all(&h.bytes())?;
        out.flush()?;
        out.get_mut().set_len(off)?;
        Ok(())
    })?;
    written_gen(base, g);
    Ok(())
}

/// A run's image store (cache.rs front_run): an ordinary store file that one process creates
/// under a fresh name and only appends to. Payloads go past the data end with positioned writes
/// (the page cache; no sync per payload) and each is mapped read-only right after its write; the
/// index is committed (data synced first, then the inactive slot and the header, as `append`)
/// only before another store refers to its entries. Never truncated or rewritten: a full index
/// makes the caller start another run store. Other processes read it as any store (Store::open).
pub struct RunStore {
    /// the file name (the run store's identity in references)
    pub name: std::sync::Arc<str>,
    pub path: PathBuf,
    file: std::fs::File,
    cap: u32,
    state: relax_session::sync::Mutex<RunState>,
    /// serializes commits (slot alternation)
    commit: relax_session::sync::Mutex<()>,
}

#[derive(Default)]
struct RunState {
    /// the next free byte (payloads in flight included)
    end: u64,
    /// the end of the written payloads
    written: u64,
    /// entries written, in write order; how many of them the active slot holds
    index: Vec<Ix>,
    committed: usize,
    /// payloads being written (their index slots are reserved)
    in_flight: usize,
    active: u32,
    /// the written payloads' mappings, by key
    mapped: std::collections::HashMap<u128, &'static [u8]>,
}

/// RunStore::append's outcome
pub enum Appended {
    /// the entry's key and its payload, mapped
    Done(u128, &'static [u8]),
    /// the index is full: use another run store
    Full,
    Failed(std::io::Error),
}

impl RunStore {
    /// A new run store `<prefix>-<16 random hex>.rcs` in `dir` with room for `cap` entries.
    pub fn create(dir: &Path, prefix: &str, cap: u32) -> std::io::Result<RunStore> {
        let name = format!("{}-{:016x}.rcs", prefix, crate::pack::nonce());
        let path = dir.join(&name);
        let mut o = std::fs::OpenOptions::new();
        o.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let file = o.open(&path)?;
        let mut h = Header { active: 0, cap, slots: [(0, 0, 0); 2] };
        h.slots = [(0, h.data_start(), sum64(&[])); 2];
        write_at(&file, &h.bytes(), 0)?;
        mark_live(&path, &path, false);
        Ok(RunStore { name: name.into(), path, file, cap,
            state: relax_session::sync::Mutex::new(RunState { end: h.data_start(), written: h.data_start(), ..Default::default() }),
            commit: relax_session::sync::Mutex::new(()) })
    }

    /// Appends a `len`-byte payload streamed by `write` and maps it. Its key: `key` mixed with
    /// the payload's length and checksum (equal payloads, equal keys). Threads append in
    /// parallel: only the range reservation and the index push take the lock.
    pub fn append(&self, key: u128, kind: u32, len: usize, write: &dyn Fn(&mut dyn Write) -> std::io::Result<()>) -> Appended {
        let off = {
            let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.index.len() + s.in_flight >= self.cap as usize { return Appended::Full; }
            let off = align8(s.end);
            s.end = off + len as u64;
            s.in_flight += 1;
            off
        };
        let r = (|| -> std::io::Result<(u64, &'static [u8])> {
            let s = crate::load_profile::span(profiling(), "run-store-write", "");
            let mut out = PosWriter { file: &self.file, off, buf: Vec::with_capacity(64 * 1024), sum: Sum64::new() };
            write(&mut out)?;
            out.flush()?;
            drop(s);
            let _s = crate::load_profile::span(profiling(), "run-store-map", "");
            if out.sum.len != len as u64 {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "run store payload length"));
            }
            let sum = out.sum.finish();
            let bytes = crate::pack::map_pinned(&self.file, off, len).ok_or_else(|| std::io::Error::other("cannot map a run store payload"))?;
            Ok((sum, bytes))
        })();
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.in_flight -= 1;
        match r {
            Ok((sum, bytes)) => {
                let mut h = crate::sip::Sip128::new();
                h.u128(key);
                h.u64(len as u64);
                h.u64(sum);
                let key = h.finish128();
                s.index.push(Ix { key, off, len: len as u32, kind, sum });
                s.written = s.written.max(off + len as u64);
                s.mapped.entry(key).or_insert(bytes);
                Appended::Done(key, bytes)
            }
            // the reserved range stays a hole: no index entry names it
            Err(e) => Appended::Failed(e),
        }
    }

    /// Makes every payload written so far durable and visible to Store::open: data synced, then
    /// the merged index into the inactive slot and the header flipped (under the file lock that
    /// readers take shared).
    pub fn commit(&self) -> std::io::Result<()> {
        self.commit_upto(usize::MAX)
    }

    /// The payloads written so far (commit_upto's argument for references made now).
    pub fn written(&self) -> usize {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).index.len()
    }

    /// commit, unless the first `n` payloads written are committed already (by a commit that
    /// covered them, perhaps made for another writer meanwhile): one sync covers many writers.
    pub fn commit_upto(&self, n: usize) -> std::io::Result<()> {
        let _c = self.commit.lock().unwrap_or_else(|e| e.into_inner());
        let (mut ix, end, n, active) = {
            let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.committed >= n.min(s.index.len()) { return Ok(()); }
            (s.index.clone(), s.written, s.index.len(), s.active)
        };
        ix.sort_by_key(|e| e.key);
        ix.dedup_by_key(|e| e.key);
        // the payloads before the index and header that commit them (see `append`)
        order_writes(&self.file)?;
        let ib = encode_index(&ix);
        let next = 1 - active;
        let mut h = Header { active: next, cap: self.cap, slots: [(0, 0, 0); 2] };
        h.slots[next as usize] = (ix.len() as u64, end, sum64(&ib));
        self.file.lock()?;
        let r = (|| {
            write_at(&self.file, &ib, h.slot_off(next))?;
            write_at(&self.file, &h.bytes(), 0)
        })();
        let _ = self.file.unlock();
        r?;
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.committed = n;
        s.active = next;
        Ok(())
    }

    /// A payload this process appended, by key: its mapping.
    pub fn mapped(&self, key: u128) -> Option<&'static [u8]> {
        self.state.lock().unwrap_or_else(|e| e.into_inner()).mapped.get(&key).copied()
    }

    /// Whether an index slot is still free.
    pub fn full(&self) -> bool {
        let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.index.len() + s.in_flight >= self.cap as usize
    }
}

/// Buffers small writes; large slices go to the file directly (positioned, no seek).
struct PosWriter<'a> {
    file: &'a std::fs::File,
    off: u64,
    buf: Vec<u8>,
    sum: Sum64,
}
impl Write for PosWriter<'_> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        if self.sum.len + b.len() as u64 > u32::MAX as u64 {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "store payload exceeds u32"));
        }
        self.sum.update(b);
        if self.buf.len() + b.len() <= self.buf.capacity() {
            self.buf.extend_from_slice(b);
        } else {
            self.flush()?;
            if b.len() >= self.buf.capacity() {
                write_at(self.file, b, self.off)?;
                self.off += b.len() as u64;
            } else {
                self.buf.extend_from_slice(b);
            }
        }
        Ok(b.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        if !self.buf.is_empty() {
            write_at(self.file, &self.buf, self.off)?;
            self.off += self.buf.len() as u64;
            self.buf.clear();
        }
        Ok(())
    }
}

#[cfg(unix)]
fn write_at(file: &std::fs::File, b: &[u8], off: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.write_all_at(b, off)
}

#[cfg(not(unix))]
fn write_at(file: &std::fs::File, mut b: &[u8], mut off: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !b.is_empty() {
        let n = file.seek_write(b, off)?;
        if n == 0 { return Err(std::io::ErrorKind::WriteZero.into()); }
        b = &b[n..];
        off += n as u64;
    }
    Ok(())
}

/// A store file name this cache writes: `<crate name>-<16 hex>.rcs` (crate names: letters,
/// digits, `_`), with or without a generation suffix (`.<g>`).
pub fn name_ours(n: &str) -> bool {
    let Some(stem) = split_gen(n).0.strip_suffix(".rcs") else { return false };
    let Some((c, h)) = stem.rsplit_once('-') else { return false };
    !c.is_empty() && c.len() <= 128 && c.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') && h.len() == 16 && h.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Deletes one store file after checking it is one: a regular file with our name whose header
/// carries the store magic.
pub fn delete_checked(p: &Path) -> bool {
    let name_ok = p.file_name().map_or(false, |n| name_ours(&n.to_string_lossy()));
    let file_ok = std::fs::symlink_metadata(p).map_or(false, |m| m.file_type().is_file());
    let magic_ok = file_ok
        && std::fs::File::open(p).ok().and_then(|mut f| {
            let mut m = [0u8; 4];
            f.read_exact(&mut m).ok()?;
            Some(&m == MAGIC)
        }) == Some(true);
    name_ok && magic_ok && std::fs::remove_file(p).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rcs-test-{}-{}", tag, std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    #[test]
    fn run_store_appends_maps_and_commits() {
        let d = tmpdir("run");
        let run = RunStore::create(&d, "front_run", 2).unwrap();
        assert!(name_ours(&run.name));
        let put = |payload: &'static [u8]| match run.append(1, 6, payload.len(), &|out| out.write_all(payload)) {
            Appended::Done(key, bytes) => { assert_eq!(bytes, payload); assert_eq!(bytes.as_ptr() as usize % 8, 0); key }
            _ => panic!("append"),
        };
        let a = put(b"first payload");
        let b = put(b"second");
        assert_ne!(a, b, "the payload is part of the key");
        assert!(matches!(run.append(1, 6, 3, &|out| out.write_all(b"one")), Appended::Full));
        assert!(Store::open(&run.path).is_some_and(|st| st.index.is_empty()), "nothing is visible before a commit");
        run.commit().unwrap();
        let st = Store::open(&run.path).unwrap();
        assert_eq!(st.get(a).map(|x| x.1), Some(&b"first payload"[..]));
        assert_eq!(st.get(b).map(|x| x.1), Some(&b"second"[..]));
        assert!((0..st.index.len()).all(|i| st.verify_at(i)));
        // a commit with nothing new changes nothing
        run.commit().unwrap();
        assert_eq!(Store::open(&run.path).unwrap().generation, st.generation);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn append_compact_newest_wins() {
        let d = tmpdir("a");
        let p = d.join("core-0123456789abcdef.rcs");
        append(&p, vec![NewEntry { key: 5, kind: 1, payload: b"five".to_vec() }, NewEntry { key: 2, kind: 1, payload: b"two".to_vec() }], false).unwrap();
        let s = Store::open(&p).unwrap();
        assert_eq!(s.get(5).unwrap().1, b"five");
        assert_eq!(s.get(2).unwrap().1, b"two");
        let snapshot = Box::leak(s.index.to_vec().into_boxed_slice());
        let certified = Store::open_certified(&p, snapshot, s.generation).unwrap();
        assert!(certified.index.is_mapped());
        // appends past the end; the old mapping stays valid
        append(&p, vec![NewEntry { key: 5, kind: 1, payload: b"FIVE".to_vec() }, NewEntry { key: 9, kind: 2, payload: vec![9; 100] }], false).unwrap();
        assert_eq!(s.get(5).unwrap().1, b"five");
        assert!(Store::open_certified(&p, snapshot, s.generation).is_none());
        assert_eq!(certified.get(5).unwrap().1, b"five");
        let s2 = Store::open(&p).unwrap();
        assert_eq!(s2.get(5).unwrap().1, b"FIVE");
        assert_eq!(s2.get(9).unwrap().1.len(), 100);
        assert_eq!(s2.get(2).unwrap().1, b"two");
        // many entries: the index slot overflows and the file is rewritten
        let many: Vec<NewEntry> = (100..1100u128).map(|k| NewEntry { key: k, kind: 1, payload: k.to_le_bytes().to_vec() }).collect();
        append(&p, many, false).unwrap();
        let s3 = Store::open(&p).unwrap();
        assert_eq!(s3.index.len(), 1003);
        assert_eq!(s3.get(777).unwrap().1, 777u128.to_le_bytes());
        assert_eq!(s3.get(5).unwrap().1, b"FIVE");
        // drop_all replaces the contents
        append(&p, vec![NewEntry { key: 1, kind: 1, payload: b"one".to_vec() }], true).unwrap();
        let s4 = Store::open(&p).unwrap();
        assert_eq!(s4.index.len(), 1);
        // each rewrite wrote a new generation; none replaced a file another reader holds
        assert!(gen_path(&p, 1).is_file() && gen_path(&p, 2).is_file());
        assert_eq!(current(&p), gen_path(&p, 2));
        assert_eq!(s2.get(5).unwrap().1, b"FIVE");
        assert_eq!(s3.get(777).unwrap().1, 777u128.to_le_bytes());
        for g in 0..3 {
            assert!(delete_checked(&gen_path(&p, g)));
        }
        std::fs::remove_dir(&d).unwrap();
    }

    #[test]
    fn checksum_matches_original_for_arbitrary_chunks() {
        // Independent original algorithm: the final zero-padded block is hashed even for
        // an empty payload or an exact multiple of 32 bytes.
        fn original(b: &[u8]) -> u64 {
            let mut h = [P, Q, P ^ Q, P.rotate_left(17)];
            let mut blocks = b.chunks_exact(32);
            for block in &mut blocks {
                for k in 0..4 {
                    let w = u64::from_le_bytes(block[k * 8..k * 8 + 8].try_into().unwrap());
                    h[k] = (h[k] ^ w).wrapping_mul(Q).rotate_left(29);
                }
            }
            let mut tail = [0; 32];
            tail[..blocks.remainder().len()].copy_from_slice(blocks.remainder());
            for k in 0..4 {
                let w = u64::from_le_bytes(tail[k * 8..k * 8 + 8].try_into().unwrap());
                h[k] = (h[k] ^ w).wrapping_mul(Q).rotate_left(29);
            }
            let mut x = (b.len() as u64).wrapping_mul(P);
            for h in h { x = (x ^ h).wrapping_mul(P).rotate_left(31); }
            x ^ (x >> 29)
        }
        let bytes: Vec<_> = (0..131_079).map(|i| (i * 31) as u8).collect();
        for n in [0, 1, 7, 31, 32, 33, 63, 64, 65, 65535, 65536, bytes.len()] {
            let expected = original(&bytes[..n]);
            assert_eq!(sum64(&bytes[..n]), expected);
            for size in [1, 7, 31, 32, 33, 1024, 65536] {
                let mut sum = Sum64::new();
                for b in bytes[..n].chunks(size) { sum.update(b); }
                assert_eq!(sum.finish(), expected, "length {n}, chunk {size}");
            }
        }
    }

    #[test]
    fn streamed_failure_and_corrupt_compaction_preserve_committed_entries() {
        struct Fails;
        impl Entry for Fails {
            fn key(&self) -> u128 { 42 }
            fn kind(&self) -> u32 { 1 }
            fn write_payload(&self, w: &mut dyn Write) -> std::io::Result<()> {
                w.write_all(&[7; 131_072])?;
                Err(std::io::Error::other("injected encoding failure"))
            }
        }
        let d = tmpdir("stream");
        let p = d.join("core-0123456789abcdef.rcs");
        let entry = |key, payload| NewEntry { key, kind: 1, payload };
        append(&p, vec![entry(1, vec![1; 131_079]), entry(2, vec![2; 131_077])], false).unwrap();
        let before = Store::open(&p).unwrap();
        assert!(append(&p, vec![Fails], false).is_err());
        let after = Store::open(&p).unwrap();
        assert_eq!(after.get(1).unwrap().1, before.get(1).unwrap().1);
        assert!(after.get(42).is_none());
        // A failed rewrite must leave the original inode and no temporary file behind.
        assert!(append(&p, vec![Fails], true).is_err());
        assert_eq!(std::fs::read_dir(&d).unwrap().count(), 1);
        assert!(Store::open(&p).unwrap().get(2).is_some());
        // Corrupt a retained payload and force capacity growth; only that entry is dropped.
        let mut file = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        file.seek(SeekFrom::Start(after.index[0].off)).unwrap();
        file.write_all(&[99]).unwrap();
        drop(file);
        append(&p, (100..400).map(|k| entry(k, vec![k as u8; 37])).collect(), false).unwrap();
        let grown = Store::open(&p).unwrap();
        assert!(grown.get(1).is_none());
        assert_eq!(grown.get(2).unwrap().1, &[2; 131_077]);
        assert_eq!(grown.get(399).unwrap().1, &[399u32 as u8; 37]);
        // Dead-space compaction also handles newly streamed replacements and empty payloads.
        append(&p, vec![entry(2, vec![3; 3 << 20])], false).unwrap();
        append(&p, vec![entry(2, Vec::new())], false).unwrap();
        let compact = Store::open(&p).unwrap();
        assert_eq!(compact.get(2).unwrap().1, &[]);
        assert!(compact.file_len() < 1 << 20);
        assert_eq!(grown.get(2).unwrap().1, &[2; 131_077]);
        for g in 0..8 {
            let f = gen_path(&p, g);
            assert!(!f.exists() || delete_checked(&f));
        }
        std::fs::remove_dir(d).unwrap();
    }

    #[test]
    fn names() {
        assert!(name_ours("makepad_widgets-0123456789abcdef.rcs"));
        assert!(!name_ours("x.rcs"));
        assert!(!name_ours("a-b-0123456789abcdeg.rcs"));
        assert!(!name_ours("../a-0123456789abcdef.rcs"));
        assert!(name_ours("makepad_widgets-0123456789abcdef.rcs.12"));
        assert!(!name_ours("makepad_widgets-0123456789abcdef.rcs.012"));
        assert!(!name_ours("makepad_widgets-0123456789abcdef.rcs.x"));
        assert_eq!(split_gen("a-0123456789abcdef.rcs.7"), ("a-0123456789abcdef.rcs", 7));
        assert_eq!(split_gen("a-0123456789abcdef.rcs"), ("a-0123456789abcdef.rcs", 0));
        assert_eq!(base_path(Path::new("/c/a.rcs.3")), Path::new("/c/a.rcs"));
    }

    /// Another process's rewrite: this process's listing is stale, a newer generation appears;
    /// appends and opens follow it, and the old generation's file is never touched.
    #[test]
    fn newer_generation_written_elsewhere_is_followed() {
        let d = tmpdir("gen");
        let p = d.join("core-00000000000000ab.rcs");
        append(&p, vec![NewEntry { key: 1, kind: 1, payload: b"old".to_vec() }], false).unwrap();
        let held = Store::open(&p).unwrap();
        // the other process: a whole new generation 1 (as its rewrite writes it)
        std::fs::copy(&p, gen_path(&p, 1)).unwrap();
        append(&p, vec![NewEntry { key: 2, kind: 1, payload: b"two".to_vec() }], false).unwrap();
        assert_eq!(current(&p), gen_path(&p, 1));
        let s = Store::open(&p).unwrap();
        assert_eq!(s.get(2).unwrap().1, b"two");
        assert!(Store::open_certified(&gen_path(&p, 0), Box::leak(held.index.to_vec().into_boxed_slice()), held.generation).is_none());
        // generation 0 is as it was
        let g0 = std::fs::read(&p).unwrap();
        assert!(g0.windows(3).any(|w| w == b"old") && !g0.windows(3).any(|w| w == b"two"));
        assert_eq!(held.get(1).unwrap().1, b"old");
        drop((held, s));
        for g in 0..2 {
            assert!(delete_checked(&gen_path(&p, g)));
        }
        std::fs::remove_dir(&d).unwrap();
    }
}
