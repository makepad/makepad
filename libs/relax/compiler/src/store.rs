//! Disk cache store files (plans/relax-cache.md): ONE file per crate version under
//! `<target>/relax/cache/<crate>-<16 hex>.rcs` (dirs.rs) (the hex: Relax codegen identity (codegen_id.rs), target and the crate's
//! fingerprint), shared by every program that uses that crate.
//!
//! Layout:
//!   header (HEADER bytes): magic "RCS1", format u32, active slot u32, slot capacity u32 (index
//!     entries per slot), then per slot: entry count u64, data end u64, index checksum u64, where
//!     its unproven payloads begin u64 (Slot::tail), the epoch of the process that committed it
//!     u64 and where the payloads written under that epoch begin u64 (Slot::epoch, epoch_from);
//!     the writing Relax build u64 (the codegen identity's low half, codegen_id.rs: build_of);
//!     header checksum u64 last
//!   index slot 0, index slot 1 (capacity x INDEX_ENTRY bytes each): sorted (key u128, offset u64,
//!     length u32, kind u32, payload checksum u64)
//!   entries (8-byte aligned payloads) up to the active slot's data end
//! Readers take a shared lock, copy the active slot's index and map the file; payloads below a
//! committed data end never change. A writer (exclusive lock) appends payloads past data end,
//! writes the merged index into the inactive slot, then flips the header: a crash leaves the old
//! header and index valid. Crash safety is detection, not ordering (the stores are a rebuildable
//! cache: a lost payload is a cache miss), and no store write waits on the disk: a write barrier
//! (fsync) forces a journal or transaction commit of the whole filesystem, which on a busy disk
//! took seconds and held every other file operation of the process behind it, its exit's
//! included. Instead a commit records the epoch it was made in (`epoch`: the boot and the mount
//! of the store's folder). While the kernel that took a write still runs and the filesystem is
//! still mounted, a read sees exactly what was written, so a payload of the current epoch needs no
//! check; one of an earlier epoch may have been lost with its machine, and is checked against
//! its checksum at its first read (`Store::get_at`: a mismatch is a miss for that entry alone).
//! A check that passed read the bytes from the disk, which proves them there: the background pass
//! (`seal_suspects`) checks a store's earlier-epoch payloads and moves its `tail` past them with
//! a plain header write. The structure is checked at every open: a torn header fails its
//! checksum (the store is invalid), and a commit whose index fails its checksum or whose data end
//! lies past the file's end gives way to the commit before it, in the other slot (appends write
//! past both slots' data ends, so its payloads are intact). When the merged index does not fit a
//! slot, or dead bytes outgrow a quarter of the live ones (or the store is stale), the store is
//! rewritten whole as its next generation. The newest entry of a key wins.
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
use std::sync::atomic::Ordering;

pub const MAGIC: &[u8; 4] = b"RCS1";
pub(crate) const FORMAT: u32 = 3;
const HEADER: usize = 128;
const INDEX_ENTRY: usize = 40;
const MIN_CAP: u32 = 256;
/// Where the header's two slots begin, and the bytes of each.
const SLOTS_AT: usize = 16;
const SLOT: usize = 48;
/// Where the header names the Relax build that wrote the store.
const BUILD_AT: usize = 112;

/// This Relax build's codegen identity (codegen_id.rs) as store headers carry it. Every store name hashes it
/// (cache.rs front_store_name, code_store_name, front_graph_key; run stores are one process's),
/// so a store is only ever written by one build.
pub fn this_build() -> u64 {
    crate::codegen_id::id() as u64
}

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

/// The kind of an entry that removes its key from the store instead of adding a payload (an
/// entry superseded under a key nothing names any more): nothing is written for it, and the
/// payload it removes is dead space for the next rewrite.
pub const KIND_REMOVED: u32 = u32::MAX;

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

/// One slot's commit, as the header records it.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct Slot {
    /// entries in the slot's index
    n: u64,
    /// the data end it commits
    end: u64,
    /// its index's checksum
    sum: u64,
    /// the payloads below this offset are proven on the disk (sealed: `seal_suspects`)
    tail: u64,
    /// the epoch (`epoch`) of the process that made this commit; 0: one with no epoch
    epoch: u64,
    /// the payloads at or past this offset were written under `epoch` (at most `end`; those below
    /// `tail` are proven whatever their epoch)
    epoch_from: u64,
}

impl Slot {
    /// A slot no commit uses (a fresh rewrite's second one): its data end lies in the header.
    const NONE: Slot = Slot { n: 0, end: 0, sum: 0, tail: 0, epoch: 0, epoch_from: 0 };

    /// Where the payloads a process of epoch `now` may serve with no check begin: those written in
    /// its own epoch. Below it (and past `tail`) they are suspect: checked at their first read.
    fn trusted_from(&self, now: u64) -> u64 {
        if now != 0 && self.epoch == now { self.epoch_from } else { self.end }
    }

    /// The epoch fields of a commit made in epoch `now` on top of this one, whose new payloads
    /// begin at this one's data end.
    fn next_epoch(&self, now: u64) -> (u64, u64) {
        (now, if now != 0 && self.epoch == now { self.epoch_from } else { self.end })
    }
}

struct Header {
    active: u32,
    cap: u32,
    slots: [Slot; 2],
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
        let slot = |k: usize| {
            let o = SLOTS_AT + k * SLOT;
            Slot { n: u64_at(b, o), end: u64_at(b, o + 8), sum: u64_at(b, o + 16), tail: u64_at(b, o + 24),
                epoch: u64_at(b, o + 32), epoch_from: u64_at(b, o + 40) }
        };
        Some(Header { active, cap, slots: [slot(0), slot(1)] })
    }

    fn bytes(&self) -> [u8; HEADER] {
        let mut b = [0u8; HEADER];
        b[0..4].copy_from_slice(MAGIC);
        b[4..8].copy_from_slice(&FORMAT.to_le_bytes());
        b[8..12].copy_from_slice(&self.active.to_le_bytes());
        b[12..16].copy_from_slice(&self.cap.to_le_bytes());
        for (k, s) in self.slots.iter().enumerate() {
            let o = SLOTS_AT + k * SLOT;
            for (i, v) in [s.n, s.end, s.sum, s.tail, s.epoch, s.epoch_from].into_iter().enumerate() {
                b[o + i * 8..o + i * 8 + 8].copy_from_slice(&v.to_le_bytes());
            }
        }
        b[BUILD_AT..BUILD_AT + 8].copy_from_slice(&this_build().to_le_bytes());
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

/// The commit a header names (Store::generation): its bytes without the slots' `tail`, so a
/// seal (`seal_suspects`) keeps it (and a collected program certified against it stays valid).
fn generation(hb: &[u8]) -> u64 {
    let mut b = [0u8; HEADER - 8];
    b.copy_from_slice(&hb[..HEADER - 8]);
    for k in 0..2 {
        let o = SLOTS_AT + k * SLOT + 24;
        b[o..o + 8].fill(0);
    }
    sum64(&b)
}

/// A store write's trace (RELAX_VERBOSE): each one over `SLOW_WRITE_MS` says when it began, its
/// time and where that went.
struct WriteTrace<'a> {
    file: &'a Path,
    at: f64,
    t: std::time::Instant,
    how: &'static str,
    marks: Vec<(&'static str, f64)>,
}

const SLOW_WRITE_MS: f64 = 100.0;

impl<'a> WriteTrace<'a> {
    fn new(file: &'a Path) -> Self {
        WriteTrace { file, at: crate::clock::ms_since_start(), t: std::time::Instant::now(), how: "append", marks: Vec::new() }
    }
    fn mark(&mut self, what: &'static str) {
        self.marks.push((what, self.t.elapsed().as_secs_f64() * 1e3));
    }
}

impl Drop for WriteTrace<'_> {
    fn drop(&mut self) {
        let ms = self.t.elapsed().as_secs_f64() * 1e3;
        if ms < SLOW_WRITE_MS || !crate::run::VERBOSE.load(Ordering::Relaxed) { return; }
        let mut last = 0.0;
        let mut parts: Vec<String> = self.marks.iter().map(|&(w, at)| { let d = at - last; last = at; format!("{w} {d:.1}") }).collect();
        parts.push(format!("the rest {:.1}", ms - last));
        eprintln!("relax store: a slow {} at t={:.0} ms: {ms:.1} ms ({}) {}", self.how, self.at, parts.join(", "),
            self.file.file_name().unwrap_or_default().to_string_lossy());
    }
}

/// Whether `e`'s payload lies within the data end `end` and matches its checksum, read through
/// `f`; Err: the read failed (on Windows a read fails while another process holds the file's lock
/// for a commit), told apart from a payload that does not match.
fn payload_matches(f: &mut std::fs::File, e: &Ix, end: u64) -> std::io::Result<bool> {
    #[cfg(test)]
    tests::PAYLOAD_READS.with(|r| r.set(r.get() + 1));
    if e.off.checked_add(e.len as u64).is_none_or(|x| x > end) {
        return Ok(false);
    }
    f.seek(SeekFrom::Start(e.off))?;
    let mut buf = [0u8; 64 * 1024];
    let mut sum = Sum64::new();
    let mut left = e.len as usize;
    while left != 0 {
        let n = left.min(buf.len());
        f.read_exact(&mut buf[..n])?;
        sum.update(&buf[..n]);
        left -= n;
    }
    Ok(sum.finish() == e.sum)
}

/// Slot `k`'s index, when its commit holds structurally in the `len` bytes of `f`: the index
/// matches its checksum and its data end lies in the file. No payload is read: one of an earlier
/// epoch is checked at its first read (`Store::get_at`).
fn slot_index(f: &mut std::fs::File, h: &Header, k: u32, len: u64) -> Option<Vec<Ix>> {
    let s = h.slots[k as usize];
    // (a seal may move `tail` past `epoch_from`: an earlier epoch's commit is proven to its end)
    if s.n > h.cap as u64 || s.end < h.data_start() || s.end > len || s.tail > s.end || s.epoch_from > s.end {
        return None;
    }
    let mut ib = vec![0u8; s.n as usize * INDEX_ENTRY];
    f.seek(SeekFrom::Start(h.slot_off(k))).ok()?;
    f.read_exact(&mut ib).ok()?;
    if sum64(&ib) != s.sum {
        return None;
    }
    Some(decode_index(&ib))
}

/// The commit a reader or an appender takes, with its index: the active one, or else the one
/// before it, in the other slot.
fn usable_slot(f: &mut std::fs::File, h: &Header, len: u64) -> Option<(u32, Vec<Ix>)> {
    let a = h.active;
    slot_index(f, h, a, len).map(|ix| (a, ix)).or_else(|| slot_index(f, h, 1 - a, len).map(|ix| (1 - a, ix)))
}

/// This process's epoch for the stores in the folder `dir`: a hash of the machine's boot and of
/// the mount of the folder's filesystem (0: none on this platform, or the folder cannot be read).
/// A payload committed in the current epoch is served with no check: the kernel that took its
/// write still runs and the filesystem is still mounted, so a read returns what was written.
///
/// Residual risk (accepted, main 2026-11-17): where the mount instance is not observable (macOS,
/// Windows, Linux before 6.8), a volume that goes away uncleanly and comes back within one boot
/// (an external disk pulled and returned) keeps its epoch, so an image written in the last
/// writeback interval before the pull could be served damaged; code is checked at its use
/// regardless (`get_checked_at`), and the background pass removes such a store. The caches live
/// on the internal disk.
fn epoch(dir: &Path) -> u64 {
    #[cfg(test)]
    {
        let e = tests::EPOCH.load(Ordering::SeqCst);
        if e != 0 {
            return e;
        }
    }
    if let Some(e) = fresh_epoch() {
        return e;
    }
    let mut g = EPOCHS.lock().unwrap_or_else(|e| e.into_inner());
    let map = g.get_or_insert_with(Default::default);
    if let Some(&e) = map.get(dir) {
        return e;
    }
    static BOOT: std::sync::OnceLock<Option<Vec<u8>>> = std::sync::OnceLock::new();
    let Some(boot) = BOOT.get_or_init(|| {
        let _k = crate::zone_alloc::kept();
        EPOCH_READS.fetch_add(1, Ordering::Relaxed);
        boot_identity()
    }) else {
        return 0;
    };
    EPOCH_READS.fetch_add(1, Ordering::Relaxed);
    // a folder not there yet: no epoch this time (its commits are checked as an earlier epoch's)
    let Some(mount) = mount_identity(dir) else { return 0 };
    let mut b = boot.clone();
    b.push(0);
    b.extend_from_slice(&mount);
    let e = sum64(&b) | 1;
    let _k = crate::zone_alloc::kept();
    map.insert(dir.to_path_buf(), e);
    e
}

/// The epochs this process found, by folder.
static EPOCHS: relax_session::sync::Mutex<Option<std::collections::HashMap<PathBuf, u64>>> = relax_session::sync::Mutex::new(None);

/// The reads of the system an epoch took (the boot once a process, the mount once a folder).
static EPOCH_READS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// RELAX_STORE_FRESH_EPOCH (a trace switch): this process takes an epoch of its own, so every
/// commit made before it is checked as one of an earlier boot's (the first start after a reboot).
/// Nothing is sealed: what such a check reads may come from the page cache, which proves nothing.
fn fresh_epoch() -> Option<u64> {
    static FRESH: std::sync::OnceLock<Option<u64>> = std::sync::OnceLock::new();
    *FRESH.get_or_init(|| std::env::var_os("RELAX_STORE_FRESH_EPOCH").map(|_| crate::pack::nonce() | 1))
}

/// The machine's boot: an identity that changes at every kernel start.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn boot_identity() -> Option<Vec<u8>> {
    let b = std::fs::read("/proc/sys/kernel/random/boot_id").ok()?;
    (b.len() >= 32).then_some(b)
}

#[cfg(target_vendor = "apple")]
fn boot_identity() -> Option<Vec<u8>> {
    // not kern.boottime: it moves when the clock is set
    extern "C" {
        fn sysctlbyname(name: *const std::ffi::c_char, old: *mut std::ffi::c_void, oldlen: *mut usize, new: *mut std::ffi::c_void, newlen: usize) -> i32;
    }
    let mut b = [0u8; 64];
    let mut n = b.len();
    let r = unsafe { sysctlbyname(c"kern.bootsessionuuid".as_ptr(), b.as_mut_ptr().cast(), &mut n, std::ptr::null_mut(), 0) };
    (r == 0 && n >= 32).then(|| b[..n].to_vec())
}

#[cfg(windows)]
fn boot_identity() -> Option<Vec<u8>> {
    // SystemTimeOfDayInformation (3): BootTime, CurrentTime, TimeZoneBias (i64 each), TimeZoneId,
    // a reserved u32, BootTimeBias, SleepTimeBias (u64 each). BootTime is the kernel's start in
    // 100 ns units (WMI's LastBootUpTime), one value for every process of a boot. Should a clock
    // step move it, the stores only count as an earlier epoch's: checked, never wrongly trusted.
    #[link(name = "ntdll")]
    extern "system" {
        fn NtQuerySystemInformation(class: u32, info: *mut std::ffi::c_void, len: u32, ret: *mut u32) -> i32;
    }
    let mut b = [0u64; 6];
    let mut ret = 0u32;
    let r = unsafe { NtQuerySystemInformation(3, b.as_mut_ptr().cast(), 48, &mut ret) };
    (r >= 0 && b[0] != 0).then(|| b[0].to_le_bytes().to_vec())
}

#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple", windows)))]
fn boot_identity() -> Option<Vec<u8>> {
    None
}

/// The mount of the folder `dir`'s filesystem: its device, and on Linux 6.8+ the mount's unique
/// id, so a filesystem mounted again within one boot is another epoch.
#[cfg(unix)]
fn mount_identity(dir: &Path) -> Option<Vec<u8>> {
    use std::os::unix::fs::MetadataExt;
    let dev = std::fs::metadata(dir).ok()?.dev().to_le_bytes();
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if let Some(id) = unique_mount_id(dir) {
        return Some([&dev[..], &id.to_le_bytes()].concat());
    }
    Some(dev.to_vec())
}

#[cfg(not(unix))]
fn mount_identity(dir: &Path) -> Option<Vec<u8>> {
    std::fs::metadata(dir).ok().map(|_| Vec::new())
}

/// The unique id of the mount `dir` is on (statx STATX_MNT_ID_UNIQUE, Linux 6.8+).
#[cfg(all(target_os = "linux", target_env = "gnu"))]
fn unique_mount_id(dir: &Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    extern "C" {
        fn statx(dirfd: i32, path: *const std::ffi::c_char, flags: i32, mask: u32, buf: *mut u64) -> i32;
    }
    const AT_FDCWD: i32 = -100;
    const STATX_MNT_ID_UNIQUE: u32 = 0x4000;
    let path = std::ffi::CString::new(dir.as_os_str().as_bytes()).ok()?;
    // struct statx: 256 bytes; stx_mask is its first u32, stx_mnt_id the u64 at byte 144
    let mut buf = [0u64; 32];
    let r = unsafe { statx(AT_FDCWD, path.as_ptr(), 0, STATX_MNT_ID_UNIQUE, buf.as_mut_ptr()) };
    (r == 0 && (buf[0] as u32) & STATX_MNT_ID_UNIQUE != 0).then_some(buf[18])
}

/// The first-read checks of an earlier epoch's payloads (Store::get_at) this process made: what a
/// first start after a reboot pays, for its trace line (`note_checks`).
struct Checks {
    n: std::sync::atomic::AtomicU64,
    bytes: std::sync::atomic::AtomicU64,
    ns: std::sync::atomic::AtomicU64,
    max_ns: std::sync::atomic::AtomicU64,
    main_ns: std::sync::atomic::AtomicU64,
    noted_start: std::sync::atomic::AtomicBool,
}

impl Checks {
    const fn new() -> Self {
        use std::sync::atomic::{AtomicBool, AtomicU64};
        Checks { n: AtomicU64::new(0), bytes: AtomicU64::new(0), ns: AtomicU64::new(0), max_ns: AtomicU64::new(0), main_ns: AtomicU64::new(0), noted_start: AtomicBool::new(false) }
    }

    fn note(&self, bytes: u64, ns: u64) {
        self.n.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(bytes, Ordering::Relaxed);
        self.ns.fetch_add(ns, Ordering::Relaxed);
        self.max_ns.fetch_max(ns, Ordering::Relaxed);
        if crate::jit::current_thread_id() == crate::jit::MAIN_TID.load(Ordering::Relaxed) {
            self.main_ns.fetch_add(ns, Ordering::Relaxed);
        }
    }
}

static CHECKS: Checks = Checks::new();

/// The trace line (RELAX_VERBOSE) of the first-read checks so far: `when` the start ended (once:
/// its first present, or its quiet) or the exit began.
pub(crate) fn note_checks(when: &str) {
    if !crate::run::VERBOSE.load(Ordering::Relaxed) || (when == "by the start's end" && CHECKS.noted_start.swap(true, Ordering::Relaxed)) {
        return;
    }
    let ms = |a: &std::sync::atomic::AtomicU64| a.load(Ordering::Relaxed) as f64 / 1e6;
    eprintln!("relax store: checked at first read {when} (t={:.0} ms): {} payloads of an earlier boot, {:.1} MB, {:.1} ms in all, the longest {:.1} ms, on the main thread {:.1} ms",
        crate::clock::ms_since_start(), CHECKS.n.load(Ordering::Relaxed), CHECKS.bytes.load(Ordering::Relaxed) as f64 / 1e6, ms(&CHECKS.ns), ms(&CHECKS.max_ns), ms(&CHECKS.main_ns));
}

/// Whether this process may seal: its epoch is a real one (a check that passed then read the disk).
fn may_seal(now: u64) -> bool {
    now != 0 && fresh_epoch().is_none()
}

/// Store files this process opened whose commit holds payloads of an earlier epoch past its
/// `tail` (Store::suspect): the background pass seals them (`seal_suspects`).
static SUSPECTS: relax_session::sync::Mutex<Vec<PathBuf>> = relax_session::sync::Mutex::new(Vec::new());

fn note_suspect(file: &Path) {
    let mut g = SUSPECTS.lock().unwrap_or_else(|e| e.into_inner());
    if !g.iter().any(|p| p == file) {
        let _k = crate::zone_alloc::kept();
        g.push(file.to_path_buf());
    }
}

/// Whether opened stores wait for `seal_suspects`.
pub(crate) fn suspects_pending() -> bool {
    !SUSPECTS.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
}

/// What `seal_suspects` did: stores sealed, payload bytes it checked, stores it removed, and the
/// reads of the system this process's epochs took.
pub(crate) struct Sealed {
    pub stores: usize,
    pub bytes: u64,
    pub removed: usize,
    pub epoch_reads: u64,
}

/// The background pass's seal (cache.rs background_seal, after the start): each opened store
/// with payloads of an earlier epoch past its `tail` has those payloads checked against their
/// checksums. A check that passed read them from the disk (a page of an earlier boot's write is
/// not in this boot's page cache), which proves them there: the header then moves `tail` past
/// them with a plain write, under the file lock, and later opens serve them with no check. No
/// barrier: nothing here waits on a journal or transaction commit. It never moves `tail` into the
/// current epoch's payloads (proven by nothing yet); a payload that fails removes the store (the
/// next run makes its entries again) and is never sealed over. `stop`: checked between payloads
/// (the quit).
pub(crate) fn seal_suspects(stop: &dyn Fn() -> bool) -> Sealed {
    let files = std::mem::take(&mut *SUSPECTS.lock().unwrap_or_else(|e| e.into_inner()));
    let mut done = Sealed { stores: 0, bytes: 0, removed: 0, epoch_reads: 0 };
    for file in files {
        if stop() {
            break;
        }
        match seal_by_evidence(&file, stop) {
            Some(Some(bytes)) => {
                done.stores += 1;
                done.bytes += bytes;
            }
            Some(None) => done.removed += 1,
            None => {}
        }
    }
    done.epoch_reads = EPOCH_READS.load(Ordering::Relaxed);
    done
}

/// `seal_suspects` for one store file: Some(Some(bytes checked)): sealed (or nothing to seal);
/// Some(None): a payload failed and the store was removed; None: not now (stopped, a read failed,
/// or the header moved on in a way the checks do not cover).
fn seal_by_evidence(file: &Path, stop: &dyn Fn() -> bool) -> Option<Option<u64>> {
    let now = epoch(file.parent()?);
    if !may_seal(now) {
        return None;
    }
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(file).ok()?;
    f.lock_shared().ok()?;
    let read = (|| {
        let mut hb = [0u8; HEADER];
        f.read_exact(&mut hb).ok()?;
        let h = Header::parse(&hb)?;
        let len = f.metadata().ok()?.len();
        let ix = slot_index(&mut f, &h, h.active, len)?;
        Some((hb, h.slots[h.active as usize], ix))
    })();
    let _ = f.unlock();
    let (hb, s, ix) = read?;
    let bound = s.trusted_from(now);
    if s.tail >= bound {
        return Some(Some(0));
    }
    // payloads below a committed end never change: read without the lock
    let mut bytes = 0u64;
    for e in ix.iter().filter(|e| e.off >= s.tail && e.off < bound) {
        if stop() {
            return None;
        }
        bytes += e.len as u64;
        if !payload_matches(&mut f, e, s.end).ok()? {
            if crate::run::VERBOSE.load(Ordering::Relaxed) {
                eprintln!("relax store: a payload an earlier boot committed to {} does not match its checksum: the store is removed (the next run makes it again)", file.display());
            }
            drop(f);
            delete_checked(file);
            return Some(None);
        }
    }
    // under the lock: the commit now must still be the one checked, or one built on it in this
    // epoch (an append keeps `tail` and the payloads below `bound`, and adds its own past them)
    f.lock().ok()?;
    let r = (|| {
        let mut cur = [0u8; HEADER];
        f.seek(SeekFrom::Start(0)).ok()?;
        f.read_exact(&mut cur).ok()?;
        let h = Header::parse(&cur)?;
        let a = h.slots[h.active as usize];
        let built_on = a.epoch == now && a.epoch_from >= bound && a.tail >= s.tail && a.end >= bound;
        if cur != hb && !built_on {
            return None;
        }
        if a.tail < bound {
            let o = SLOTS_AT + h.active as usize * SLOT + 24;
            cur[o..o + 8].copy_from_slice(&bound.to_le_bytes());
            let sum = sum64(&cur[..HEADER - 8]);
            cur[HEADER - 8..].copy_from_slice(&sum.to_le_bytes());
            f.seek(SeekFrom::Start(0)).ok()?;
            f.write_all(&cur).ok()?;
        }
        Some(Some(bytes))
    })();
    let _ = f.unlock();
    r
}

/// What a run in this process sets, as a fresh process has it (a run that ends in place).
pub(crate) unsafe fn reset_statics() {
    relax_session::reset_static(&SUSPECTS, relax_session::sync::Mutex::new(Vec::new()));
    relax_session::reset_static(&EPOCHS, relax_session::sync::Mutex::new(None));
    relax_session::reset_static(&CHECKS, Checks::new());
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

/// Store files this process opened or wrote (generation 0 paths). Prune never evicts them, nor
/// any of their generations (cache.rs prune_root): evicting a store the run just restored from
/// made every dependent re-parse on the next run.
static LIVE: relax_session::sync::Mutex<Option<std::collections::HashSet<PathBuf>>> = relax_session::sync::Mutex::new(None);

/// Records the store `base` as used by this run, in this process and in its pass's record
/// (note_in_pass: no running pass's store is evicted). The first time, an opened store's file also
/// gets its modification time stamped: when a run last used it, the order prune evicts in.
fn mark_live(base: &Path, file: &Path, stamp: bool) {
    let _k = crate::zone_alloc::kept();
    OPEN_FILES.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(file.to_path_buf());
    let new = LIVE.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(base.to_path_buf());
    if new {
        if let (Some(dir), Some(name)) = (base.parent(), base.file_name()) {
            note_in_pass(dir, Some(&name.to_string_lossy()));
        }
    }
    if new && stamp {
        let _ = std::fs::File::options().write(true).open(file).and_then(|f| f.set_modified(std::time::SystemTime::now()));
    }
}

/// Stamps the store at `path` (its generation 0 path, as `live` lists it) used by this run, as
/// opening it would (mark_live), without reading it: a run that relies on what an earlier run
/// read from it keeps it in use (check_reuse.rs Early).
pub fn stamp_used(path: &Path) {
    let base = base_path(path);
    let file = current(&base);
    if file.exists() { mark_live(&base, &file, true); }
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

/// Lists `dir` into GENS (each store's highest generation present), in place of what was known of
/// it: a generation deleted since (a cleared cache, a folder removed and made again) is gone from
/// GENS too, so the store's next write is its lowest free generation, not one above the deleted.
fn list_gens(dir: &Path, gens: &mut std::collections::HashMap<PathBuf, u32>) {
    gens.retain(|k, _| k.parent() != Some(dir));
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
        hold_build(&dir);
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
    /// the generation file mapped
    file: PathBuf,
    pub index: crate::arena::Arena<Ix>,
    pub(crate) generation: u64,
    data: Bytes,
    #[cfg(unix)]
    source: Option<ReadSource>,
    data_end: u64,
    /// the payloads at offsets in this range were committed in an earlier epoch and are not
    /// proven on the disk (Slot::tail, trusted_from): each is checked at its first read (`get_at`)
    suspect: (u64, u64),
    /// those checks, per index entry (1: sound, 2: not), made only with a suspect range
    checked: std::sync::OnceLock<Box<[std::sync::atomic::AtomicU8]>>,
}

impl Store {
    /// Opens `path` (None: missing, unreadable or not a valid store), as used by this run.
    pub fn open(path: &Path) -> Option<Store> {
        let st = Self::open_unmarked(path)?;
        st.mark_used();
        Some(st)
    }

    /// Records this store as used by this run (mark_live): the cap and the replaced-variant
    /// retirement keep it, other runs see it stamped.
    pub fn mark_used(&self) {
        mark_live(&self.path, &self.file, true);
    }

    /// When the current generation's file was last written or stamped used (mark_live)
    pub fn modified(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(&self.file).and_then(|m| m.modified()).ok()
    }

    /// Opens `path` without marking it used: a scan that may reject it (cache.rs candidate).
    pub fn open_unmarked(path: &Path) -> Option<Store> {
        let base = base_path(path);
        let file = current(&base);
        let mut f = std::fs::File::open(&file).ok()?;
        f.lock_shared().ok()?;
        let r = (|| {
            let mut hb = [0u8; HEADER];
            f.read_exact(&mut hb).ok()?;
            let h = Header::parse(&hb)?;
            let len = f.metadata().ok()?.len();
            let (k, ix) = usable_slot(&mut f, &h, len)?;
            let s = h.slots[k as usize];
            let data = Bytes::map(&file)?;
            if (data.len() as u64) < s.end {
                return None;
            }
            let suspect = (s.tail, s.trusted_from(epoch(base.parent()?)));
            if k == h.active && suspect.0 < suspect.1 {
                note_suspect(&file);
            }
            Some(Store { path: base.clone(), file: file.clone(), index: ix.into(), generation: generation(&hb), data,
                #[cfg(unix)] source: ReadSource::new(&file, &f),
                data_end: s.end, suspect, checked: Default::default() })
        })();
        let _ = f.unlock();
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
            if self::generation(hb) != generation { return None; }
            let s = h.slots[h.active as usize];
            if s.n != index.len() as u64 || s.n > h.cap as u64 || s.end > data.len() as u64 { return None; }
            // an earlier epoch's payloads are checked at their first read (get_at), as an open's are
            let suspect = (s.tail, s.trusted_from(epoch(base.parent()?)));
            if suspect.0 < suspect.1 { note_suspect(&file); }
            Some(Self { path: base.clone(), file: file.clone(), index: crate::arena::Arena::Mapped(index), generation, data,
                #[cfg(unix)] source: ReadSource::new(&file, &f),
                data_end: s.end, suspect, checked: Default::default() })
        })();
        let _ = f.unlock();
        if let Some(st) = &result { st.mark_used(); }
        result
    }

    /// The payload of index entry `i` if its key is `key`: bounds-checked, and checksummed only
    /// when an earlier epoch committed it and no seal proved it on the disk since (`suspect`: the
    /// first read checks it, a mismatch is a miss for this entry alone). A store's index and header
    /// are checksummed when it opens, and a rewrite re-checks every payload it carries; the
    /// payload checksums run in the background for front images (`verify_at`), so a warm start
    /// does not read every page of what it maps. A torn write is the store file generations'
    /// business, not the reader's.
    pub fn get_at(&self, i: usize, key: u128) -> Option<(u32, &[u8])> {
        let got = self.get_unchecked_at(i, key)?;
        let off = self.index[i].off;
        if off >= self.suspect.0 && off < self.suspect.1 && !self.checked_at(i) {
            return None;
        }
        Some(got)
    }

    /// `get_at` without the check of a suspect payload.
    fn get_unchecked_at(&self, i: usize, key: u128) -> Option<(u32, &[u8])> {
        let e = self.index.get(i)?;
        if e.key != key || e.off + e.len as u64 > self.data_end {
            return None;
        }
        let p = self.data.get(e.off as usize..e.off as usize + e.len as usize)?;
        Some((e.kind, p))
    }

    /// Whether suspect entry `i` matched its checksum, checked once.
    fn checked_at(&self, i: usize) -> bool {
        use std::sync::atomic::AtomicU8;
        let c = self.checked.get_or_init(|| {
            // (the store's state: it lives as long as the store, past the reader's zone)
            let _k = crate::zone_alloc::kept();
            (0..self.index.len()).map(|_| AtomicU8::new(0)).collect()
        });
        match c[i].load(Ordering::Relaxed) {
            1 => true,
            2 => false,
            _ => {
                let t = std::time::Instant::now();
                let ok = self.verify_at(i);
                c[i].store(if ok { 1 } else { 2 }, Ordering::Relaxed);
                CHECKS.note(self.index[i].len as u64, t.elapsed().as_nanos() as u64);
                ok
            }
        }
    }

    /// `get_at`, with the payload's checksum verified over the mapped bytes the caller then uses:
    /// for payloads that become executable code or are linked as data (code entries, the startup
    /// manifest and its image). A mismatch is a miss: never executed.
    pub fn get_checked_at(&self, i: usize, key: u128) -> Option<(u32, &[u8])> {
        let (kind, p) = self.get_unchecked_at(i, key)?;
        (sum64(p) == self.index[i].sum).then_some((kind, p))
    }

    /// `get` with the payload checksum verified (`get_checked_at`).
    pub fn get_checked(&self, key: u128) -> Option<(u32, &[u8])> {
        let i = self.index.binary_search_by(|e| { let k = e.key; k.cmp(&key) }).ok()?;
        self.get_checked_at(i, key)
    }

    /// Whether index entry `i`'s payload matches its checksum.
    pub fn verify_at(&self, i: usize) -> bool {
        #[cfg(test)]
        tests::PAYLOAD_READS.with(|r| r.set(r.get() + 1));
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
        // be reopened, verify those exact bytes. Windows always does: its file locks are
        // mandatory, so a read through a descriptor fails during another writer's commit. The
        // pages hashed leave the working set again, as the descriptor's never entered it.
        let Some(p) = self.data.get(e.off as usize..e.off as usize + e.len as usize) else { return false };
        let same = sum64(p) == e.sum;
        #[cfg(windows)]
        crate::pack::release_pages(p);
        same
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
    let r = f.read_exact(&mut hb).ok().and_then(|_| Header::parse(&hb)).map(|_| generation(&hb));
    let _ = f.unlock();
    r
}

/// Adds `entries` to the store at `path` (creating it), newest winning per key; `drop_all`: start
/// from an empty store (its contents were found stale). Atomic: an append commits by flipping
/// the header; a rewrite goes through a temp file and rename.
pub fn append<T: Entry>(path: &Path, entries: Vec<T>, drop_all: bool) -> std::io::Result<()> {
    let base = base_path(path);
    let r = append_inner(&base, entries, drop_all, None).map(|_| ());
    match &r {
        Ok(()) => mark_live(&base, &base, false),
        Err(e) => write_failed(&base, e),
    }
    r
}

/// A store write failed (a full volume, a folder it cannot write): the run goes on without what
/// it could not store -- a later run misses it and makes it again, nothing fails. Said once a
/// process, the first failure's store and error.
pub fn write_failed(path: &Path, e: &std::io::Error) {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
        crate::progress::say(&format!("relax cache: a store write failed ({}: {e}): this run goes on without storing it, a later run makes it again", path.display()));
    }
}

/// The merge of a store's ledger under its lock (append_ledgered): its key, and what makes the
/// ledger entry written with a batch from the one on disk (None: the batch is not written).
pub type LedgerMerge<'a, T> = (u128, &'a mut dyn FnMut(Option<&[u8]>) -> Option<T>);

/// `append` for a store whose ledger (the entry under `ledger_key`) is merged under the store's lock,
/// so two processes never write a batch over a ledger they did not check (the shared code stores of
/// Target B): `merge` gets the ledger payload on disk now (None: no store, or no ledger in it) and
/// gives the ledger entry to commit with `entries`, or None: nothing is written and `entries` come
/// back (Ok(Some)). Such a store is never replaced whole on a stale finding.
pub fn append_ledgered<T: Entry>(path: &Path, entries: Vec<T>, ledger_key: u128, merge: &mut dyn FnMut(Option<&[u8]>) -> Option<T>) -> std::io::Result<Option<Vec<T>>> {
    let base = base_path(path);
    let r = append_inner(&base, entries, false, Some((ledger_key, merge)));
    match &r {
        Ok(None) => mark_live(&base, &base, false),
        Ok(Some(_)) => {}
        Err(e) => write_failed(&base, e),
    }
    r
}

/// The batch with the merged ledger entry in place of any it held, in key order; None: the merge
/// refused it.
fn with_ledger<T: Entry>(mut entries: Vec<T>, merge: &mut Option<LedgerMerge<'_, T>>, disk: Option<&[u8]>) -> Result<Vec<T>, Vec<T>> {
    let Some((key, f)) = merge.as_mut() else { return Ok(entries) };
    let Some(ledger) = f(disk) else { return Err(entries) };
    let key = *key;
    entries.retain(|e| e.key() != key);
    entries.push(ledger);
    entries.sort_by_key(|a| a.key());
    Ok(entries)
}

/// Entry `e`'s payload read from the locked store file, its checksum verified; None: it does not match.
fn read_payload(f: &mut std::fs::File, e: &Ix) -> std::io::Result<Option<Vec<u8>>> {
    let mut b = vec![0u8; e.len as usize];
    f.seek(SeekFrom::Start(e.off))?;
    f.read_exact(&mut b)?;
    Ok((sum64(&b) == e.sum).then_some(b))
}

/// Ok(None): written; Ok(Some): the ledger merge refused the batch, which comes back unwritten.
fn append_inner<T: Entry>(base: &Path, mut entries: Vec<T>, drop_all: bool, mut merge: Option<LedgerMerge<'_, T>>) -> std::io::Result<Option<Vec<T>>> {
    // newest first within this batch: a stable sort keeps the first of equal keys
    entries.sort_by_key(|a| a.key());
    entries.dedup_by(|b, a| a.key() == b.key());
    // removals (KIND_REMOVED): their keys leave the index, nothing is written for them
    let removed: std::collections::HashSet<u128> = entries.iter().filter(|e| e.kind() == KIND_REMOVED).map(|e| e.key()).collect();
    entries.retain(|e| e.kind() != KIND_REMOVED);
    let dir = base.parent().unwrap_or(Path::new("."));
    let mut trace = WriteTrace::new(base);
    // the current generation, locked; a newer one written meanwhile (another process's
    // rewrite) is taken instead
    let (mut f, _gen_file) = loop {
        let (g, path) = current_gen(base);
        let f = match std::fs::OpenOptions::new().read(true).write(true).open(&path) {
            Ok(f) => f,
            Err(_) => {
                let entries = match with_ledger(entries, &mut merge, None) { Ok(e) => e, Err(e) => return Ok(Some(e)) };
                trace.how = "new store";
                return rewrite(base, dir, Vec::new(), None, entries).map(|_| None);
            }
        };
        f.lock()?;
        if !gen_path(base, g + 1).exists() {
            trace.mark("lock");
            break (f, path);
        }
        let _ = f.unlock();
    };
    let path = base;
    let r = (|| -> std::io::Result<Option<(Header, u32, Vec<Ix>)>> {
        let mut hb = [0u8; HEADER];
        if f.read_exact(&mut hb).is_err() {
            return Ok(None);
        }
        let Some(h) = Header::parse(&hb) else { return Ok(None) };
        let len = f.metadata()?.len();
        // the commit to build on: the active one, or the one before it when the active one's
        // index or data did not all reach the disk (they are written over)
        Ok(usable_slot(&mut f, &h, len).map(|(k, ix)| (h, k, ix)))
    })()?;
    trace.mark("read");
    // a merged ledger: the one on disk now, under the lock (a payload that fails its checksum
    // refuses the batch: the merge would drop what it held)
    let entries = if merge.is_some() {
        let lk = merge.as_ref().map_or(0, |m| m.0);
        let disk = match &r {
            Some((_, _, old)) => match old.iter().find(|e| { let k = e.key; k == lk }) {
                Some(e) => match read_payload(&mut f, e)? {
                    Some(b) => Some(b),
                    None => { let _ = f.unlock(); return Ok(Some(entries)); }
                },
                None => None,
            },
            None => None,
        };
        match with_ledger(entries, &mut merge, disk.as_deref()) {
            Ok(e) => e,
            Err(e) => { let _ = f.unlock(); return Ok(Some(e)); }
        }
    } else {
        entries
    };
    let (h, k, mut old) = match r {
        Some(x) if !drop_all => x,
        // unreadable or stale: replaced whole
        _ => {
            trace.how = "rewrite (stale)";
            let res = rewrite(path, dir, Vec::new(), None, entries);
            let _ = f.unlock();
            return res.map(|_| None);
        }
    };
    let Slot { end, tail, .. } = h.slots[k as usize];
    if !removed.is_empty() { old.retain(|e| !removed.contains(&{ e.key })); }
    // the merged index: new entries replace old ones of the same key
    let new_keys: std::collections::HashSet<u128> = entries.iter().map(|e| e.key()).collect();
    let merged_n = old.iter().filter(|e| !new_keys.contains(&{ e.key })).count() + entries.len();
    if merged_n as u64 > h.cap as u64 {
        trace.how = "rewrite (full)";
        let res = rewrite(path, dir, old, Some(&f), entries);
        let _ = f.unlock();
        return res.map(|_| None);
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
    trace.mark("payloads");
    ix.sort_by_key(|e| e.key);
    // Encoded lengths are now known. Use the same compaction threshold without pre-encoding
    // all new images. Only a dead-space compaction needs to copy the just-appended bytes.
    let live: u64 = ix.iter().map(|e| e.len as u64).sum();
    let dead = end.saturating_sub(h.data_start()).saturating_sub(live.min(end));
    if dead * 4 > live && dead > 1 << 20 {
        trace.how = "rewrite (compaction)";
        let res = rewrite::<T>(path, dir, ix, Some(&f), Vec::new());
        let _ = f.unlock();
        return res.map(|_| None);
    }
    crate::memstat::print_tag("cache append", crate::memstat::Tag::CacheStore);
    let ib = encode_index(&ix);
    let next = 1 - k;
    // The commit goes in under the lock with no barrier: nothing waits on the disk (a barrier
    // forces a journal or transaction commit of the whole filesystem, and every open of this
    // store would wait under the lock). Its payloads are this epoch's (epoch_from: at the old end,
    // or earlier when that commit was this epoch's too); those before stay as they were.
    f.seek(SeekFrom::Start(h.slot_off(next)))?;
    f.write_all(&ib)?;
    let mut nh = Header { active: next, cap: h.cap, slots: h.slots };
    let (epoch, epoch_from) = h.slots[k as usize].next_epoch(epoch(dir));
    nh.slots[next as usize] = Slot { n: ix.len() as u64, end: off, sum: sum64(&ib), tail, epoch, epoch_from };
    let hb = nh.bytes();
    f.seek(SeekFrom::Start(0))?;
    f.write_all(&hb)?;
    trace.mark("header");
    let _ = f.unlock();
    Ok(None)
}

/// RELAX_PROFILE is set: the load timeline's spans for the image writes.
pub fn profiling() -> bool {
    static P: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *P.get_or_init(|| std::env::var_os("RELAX_PROFILE").is_some())
}

/// Writes a fresh store with the live entries of `old` (read from `from`) plus `entries`, to a temp
/// file, then renames it to the store's next generation (never over an existing file).
fn rewrite<T: Entry>(base: &Path, dir: &Path, old: Vec<Ix>, from: Option<&std::fs::File>, entries: Vec<T>) -> std::io::Result<()> {
    let new_keys: std::collections::HashSet<u128> = entries.iter().map(|e| e.key()).collect();
    let old: Vec<_> = old.into_iter().filter(|e| !new_keys.contains(&{ e.key })).collect();
    let n = old.len() + entries.len();
    let cap = (n as u32 + n as u32 / 4).max(MIN_CAP);
    let mut h = Header { active: 0, cap, slots: [Slot::NONE; 2] };
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
        // every payload is this epoch's: the old ones were checked as they were copied
        let now = epoch(dir);
        let start = h.data_start();
        h.slots[0] = Slot { n: ix.len() as u64, end: off, sum: sum64(&ib), tail: start, epoch: now, epoch_from: if now != 0 { start } else { off } };
        out.seek(SeekFrom::Start(h.slot_off(0)))?;
        out.write_all(&ib)?;
        out.flush()?;
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
/// index is committed (data synced first, outside the lock, then the inactive slot and the header)
/// only before another store refers to its entries. Never truncated or rewritten: a full index
/// makes the caller start another run store. Other processes read it as any store (Store::open).
pub struct RunStore {
    /// the file name (the run store's identity in references)
    pub name: std::sync::Arc<str>,
    pub path: PathBuf,
    file: std::fs::File,
    cap: u32,
    /// the epoch its commits are made in (this process's)
    epoch: u64,
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
    /// the written payloads' mappings, by key
    mapped: std::collections::HashMap<u128, &'static [u8]>,
    /// what keeps them mapped: as long as this store lives
    views: Vec<crate::pack::Bytes>,
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
        let r = Self::create_at(dir, name, path.clone(), cap);
        if let Err(e) = &r {
            write_failed(&path, e);
        }
        r
    }

    fn create_at(dir: &Path, name: String, path: PathBuf, cap: u32) -> std::io::Result<RunStore> {
        hold_build(dir);
        let mut o = std::fs::OpenOptions::new();
        o.read(true).write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            o.mode(0o600);
        }
        let file = o.open(&path)?;
        let mut h = Header { active: 0, cap, slots: [Slot::NONE; 2] };
        let start = h.data_start();
        let epoch = epoch(dir);
        h.slots = [Slot { n: 0, end: start, sum: sum64(&[]), tail: start, epoch, epoch_from: start }; 2];
        write_at(&file, &h.bytes(), 0)?;
        mark_live(&path, &path, false);
        Ok(RunStore { name: name.into(), path, file, cap, epoch,
            state: relax_session::sync::Mutex::new(RunState { end: start, written: start, ..Default::default() }),
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
        let r = (|| -> std::io::Result<(u64, Option<crate::pack::Bytes>, &'static [u8])> {
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
            let (view, bytes) = crate::pack::map_range(&self.file, off, len).ok_or_else(|| std::io::Error::other("cannot map a run store payload"))?;
            Ok((sum, view, bytes))
        })();
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        s.in_flight -= 1;
        match r {
            Ok((sum, view, bytes)) => {
                s.views.extend(view);
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
            Err(e) => {
                write_failed(&self.path, &e);
                Appended::Failed(e)
            }
        }
    }

    /// Makes every payload written so far visible to Store::open: the merged index into the
    /// inactive slot and the header flipped (under the file lock that readers take shared), with
    /// no barrier (`commit_upto`).
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
        let r = self.commit_as(n);
        if let Err(e) = &r {
            write_failed(&self.path, e);
        }
        r
    }

    /// `commit_upto`. No barrier: a barrier here ran on the cache writer under SPILL_IO (a crate
    /// store's references to these images commit them first: cache.rs append_store), 11.6 s once
    /// on a busy disk while every spill behind it waited. Its payloads are this process's epoch's,
    /// served by later processes of the same boot with no check; one of a later boot checks them
    /// (Store::get_at). The header is read under the lock: another process's seal may have
    /// moved `tail` since.
    fn commit_as(&self, n: usize) -> std::io::Result<()> {
        let _c = self.commit.lock().unwrap_or_else(|e| e.into_inner());
        let (mut ix, end, n, fresh) = {
            let s = self.state.lock().unwrap_or_else(|e| e.into_inner());
            if s.committed >= n.min(s.index.len()) { return Ok(()); }
            // the lowest payload this commit adds: payloads finish out of order, so one may lie
            // below the last commit's end
            let fresh = s.index[s.committed..].iter().map(|e| e.off).min().unwrap_or(s.written);
            (s.index.clone(), s.written, s.index.len(), fresh)
        };
        ix.sort_by_key(|e| e.key);
        ix.dedup_by_key(|e| e.key);
        let ib = encode_index(&ix);
        self.file.lock()?;
        let r = (|| -> std::io::Result<()> {
            let mut now = [0u8; HEADER];
            read_at(&self.file, &mut now, 0)?;
            let was = Header::parse(&now).ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidData, "run store header"))?;
            let next = 1 - was.active;
            let mut h = Header { active: next, cap: self.cap, slots: was.slots };
            let last = was.slots[was.active as usize];
            let (epoch, epoch_from) = last.next_epoch(self.epoch);
            // payloads finish out of order: one this commit adds may lie below the last one's end
            // (both bounds come down to it, and the tail stays at or below where the epoch begins)
            h.slots[next as usize] = Slot { n: ix.len() as u64, end, sum: sum64(&ib), tail: last.tail.min(fresh), epoch, epoch_from: epoch_from.min(fresh) };
            write_at(&self.file, &ib, h.slot_off(next))?;
            write_at(&self.file, &h.bytes(), 0)?;
            Ok(())
        })();
        let _ = self.file.unlock();
        r?;
        self.state.lock().unwrap_or_else(|e| e.into_inner()).committed = n;
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
fn read_at(file: &std::fs::File, b: &mut [u8], off: u64) -> std::io::Result<()> {
    use std::os::unix::fs::FileExt;
    file.read_exact_at(b, off)
}

#[cfg(not(unix))]
fn read_at(file: &std::fs::File, mut b: &mut [u8], mut off: u64) -> std::io::Result<()> {
    use std::os::windows::fs::FileExt;
    while !b.is_empty() {
        let n = file.seek_read(b, off)?;
        if n == 0 { return Err(std::io::ErrorKind::UnexpectedEof.into()); }
        b = &mut b[n..];
        off += n as u64;
    }
    Ok(())
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

/// The Relax build that wrote the store file `p` (its header; 0: written before headers named
/// the build). None: unreadable, not a store, or locked by a writer right now.
/// The store format a file's header names (its magic checked): what a build of another format
/// cannot read (Header::parse). Read without a lock: magic and format are the first bytes a store
/// file is written with and never change; a file that has none yet says None.
pub fn format_of(p: &Path) -> Option<u32> {
    let mut f = std::fs::File::open(p).ok()?;
    let mut b = [0u8; 8];
    f.read_exact(&mut b).ok()?;
    (&b[0..4] == MAGIC).then(|| u32_at(&b, 4))
}

pub fn build_of(p: &Path) -> Option<u64> {
    let mut f = std::fs::File::open(p).ok()?;
    f.try_lock_shared().ok()?;
    let mut hb = [0u8; HEADER];
    let r = f.read_exact(&mut hb).ok().and_then(|_| Header::parse(&hb)).map(|_| u64_at(&hb, BUILD_AT));
    let _ = f.unlock();
    r
}

/// The flights this process leads: (crate set, its lock file, the open lock).
static FLIGHTS: std::sync::Mutex<Vec<(u128, PathBuf, std::fs::File)>> = std::sync::Mutex::new(Vec::new());

/// A first load of crate set `identity` with the stores of `dir` (none of its graphs is
/// stored): one run at a time makes it. The run that finds `<dir>/flight/<32 hex>` free leads:
/// it holds the file locked until it has recorded the set ([`flight_landed`]) or exits (the OS
/// drops the lock with the process, also on a crash). A run that finds it held waits for that,
/// as long as the leader takes, and returns true: the leader's stores are there to restore from,
/// where two runs started together each parsed and recorded the same crates (a leader that ended
/// without recording leaves nothing to restore: the follower then loads on its own). false: this
/// run leads (or no lock file can be made: it loads on its own).
pub fn flight(dir: &Path, identity: u128) -> bool {
    // a set this process leads already: no wait on itself
    if FLIGHTS.lock().unwrap_or_else(|e| e.into_inner()).iter().any(|(id, ..)| *id == identity) { return false; }
    let folder = dir.join("flight");
    if std::fs::create_dir_all(&folder).is_err() { return false; }
    let path = folder.join(format!("{identity:032x}"));
    let Ok(f) = std::fs::OpenOptions::new().create(true).truncate(false).read(true).write(true).open(&path) else { return false };
    if f.try_lock().is_ok() {
        // the file went between the open and the lock: its leader landed
        if !still_at(&f, &path) { return true; }
        FLIGHTS.lock().unwrap_or_else(|e| e.into_inner()).push((identity, path, f));
        return false;
    }
    f.lock().is_ok()
}

/// This process recorded what its flights were for (or will not): their followers go on.
pub fn flight_landed() {
    for (_, path, f) in FLIGHTS.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
        let _ = std::fs::remove_file(&path);
        let _ = f.unlock();
    }
}

/// Which builds run: every process holds a shared lock on `<cache dir>/live/<build hex>` from
/// the first store it touches in that folder until it exits (the OS drops the lock with the
/// process, also on a crash). retire_build takes a build's lock exclusively, so no process of
/// that build is running while its stores go, and none can start using them meanwhile.
static HELD: relax_session::sync::Mutex<Option<std::collections::HashMap<PathBuf, std::fs::File>>> = relax_session::sync::Mutex::new(None);

fn live_lock(dir: &Path, build: u64) -> PathBuf {
    dir.join("live").join(format!("{build:016x}"))
}

/// Whether the open file `f` still is the file at `p` (retire_build removes a lock file).
fn still_at(f: &std::fs::File, p: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let (Ok(a), Ok(b)) = (f.metadata(), std::fs::metadata(p)) else { return false };
        (a.dev(), a.ino()) == (b.dev(), b.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = (f, p);
        true
    }
}

/// Marks this process as a running user of `dir`'s stores of this build (until it exits). A
/// folder it cannot lock in (read-only) is used unmarked.
pub fn hold_build(dir: &Path) {
    let _k = crate::zone_alloc::kept();
    let mut guard = HELD.lock().unwrap_or_else(|e| e.into_inner());
    let held = guard.get_or_insert_with(Default::default);
    if held.contains_key(dir) {
        return;
    }
    let p = live_lock(dir, this_build());
    // a missing store folder is made owner-only, as a run makes its cache folder: made by the
    // create_dir_all below it took the umask's mode, group-writable under umask 002, and the store
    // then refused the folder it had just made (a test's first lookup)
    if !dir.exists() && crate::pack::ensure_private_dir(dir).is_err() {
        return;
    }
    loop {
        if std::fs::create_dir_all(dir.join("live")).is_err() {
            return;
        }
        let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&p) else { return };
        if f.lock_shared().is_err() {
            return;
        }
        // a prune that retired this build meanwhile removed the file it held: lock the new one
        if still_at(&f, &p) {
            // stamped when taken: a tool that holds a timed series' builds between its runs
            // (devhub exec.rs hold_series) tells by it which builds a run took
            let _ = f.set_modified(std::time::SystemTime::now());
            held.insert(dir.to_path_buf(), f);
            return;
        }
    }
}

/// The builds with a live lock file in `dir` (running, or ended since the last prune).
pub fn lock_builds(dir: &Path) -> Vec<u64> {
    std::fs::read_dir(dir.join("live")).into_iter().flatten().flatten()
        .filter_map(|e| e.file_name().to_str().filter(|n| n.len() == 16).and_then(|n| u64::from_str_radix(n, 16).ok()))
        .collect()
}

/// Whether a process of `build` is running here (its live lock in `dir` is held).
pub fn build_runs(dir: &Path, build: u64) -> bool {
    let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(live_lock(dir, build)) else { return false };
    let held = f.try_lock().is_err();
    let _ = f.unlock();
    held
}

/// When no process of `build` is running (its live lock in `dir` is free), runs `retire` under
/// that lock held exclusively, then removes the lock file; false: the build is running.
pub fn retire_build(dir: &Path, build: u64, retire: impl FnOnce()) -> bool {
    let p = live_lock(dir, build);
    let _ = std::fs::create_dir_all(dir.join("live"));
    let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&p) else { return false };
    if f.try_lock().is_err() || !still_at(&f, &p) {
        return false;
    }
    retire();
    // holders check the file is still there after locking (hold_build); on Windows an open
    // file's name stays taken until it closes, so the empty lock file stays
    #[cfg(unix)]
    let _ = std::fs::remove_file(&p);
    let _ = f.unlock();
    true
}

/// Passes: the stores a pass touched stay while it runs (cache.rs prune). A pass is one command:
/// a `check -p` command and all its children (RELAX_CHECK_GROUP, the command's pid: the command
/// holds the pass from its first child to its last, hold_pass), else one process. In
/// each store folder a pass uses, `passes/<pass>` lists the stores it touched (a name a line,
/// appended by each of its processes) and `passes/<pass>.held` is held (a shared lock) by each of
/// them until it exits (the OS drops the lock with the process, also on a crash). A record whose
/// lock nobody holds is of a pass that ended: the next prune removes it.
static PASSES: relax_session::sync::Mutex<Option<std::collections::HashMap<PathBuf, Option<(std::fs::File, std::fs::File)>>>> = relax_session::sync::Mutex::new(None);

/// This process's pass: RELAX_CHECK_GROUP (set by a `check -p` command for its children), else
/// this process's pid.
pub fn pass_token() -> String {
    std::env::var("RELAX_CHECK_GROUP").ok()
        .filter(|g| !g.is_empty() && g.len() <= 64 && g.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'.'))
        .unwrap_or_else(|| std::process::id().to_string())
}

/// Holds this process's pass in store folder `dir` (until it exits) and appends `name` to the
/// stores its record lists. The pass's first process (the command: its own pid is the pass)
/// starts the list empty. A folder it cannot write is used unrecorded.
fn note_in_pass(dir: &Path, name: Option<&str>) {
    let _k = crate::zone_alloc::kept();
    let mut guard = PASSES.lock().unwrap_or_else(|e| e.into_inner());
    let held = guard.get_or_insert_with(Default::default);
    if !held.contains_key(dir) {
        // None: the folder takes no record (read-only), tried once
        held.insert(dir.to_path_buf(), open_pass(dir));
    }
    if let (Some(name), Some(Some((_, list)))) = (name, held.get(dir)) {
        // one write per name: appends of the pass's processes interleave by whole lines
        let _ = std::io::Write::write_all(&mut &*list, format!("{name}\n").as_bytes());
    }
}

/// The pass record of this process's pass in `dir`: its held lock and its list (opened to append).
fn open_pass(dir: &Path) -> Option<(std::fs::File, std::fs::File)> {
    let token = pass_token();
    let passes = dir.join("passes");
    let held = passes.join(format!("{token}.held"));
    if !dir.exists() && crate::pack::ensure_private_dir(dir).is_err() {
        return None;
    }
    let lock = loop {
        std::fs::create_dir_all(&passes).ok()?;
        let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false).open(&held).ok()?;
        f.lock_shared().ok()?;
        // a prune that found the pass ended removed the file between the open and the lock
        if still_at(&f, &held) {
            break f;
        }
    };
    let first = token == std::process::id().to_string();
    let list = std::fs::OpenOptions::new().read(true).append(true).create(true).open(passes.join(&token)).ok()?;
    if first {
        let _ = list.set_len(0);
    }
    Some((lock, list))
}

/// Holds this process's pass in store folder `dir` before any store there is touched: a `check
/// -p` command, whose children use the stores, holds their pass from the first to the last.
pub fn hold_pass(dir: &Path) {
    note_in_pass(dir, None);
}

/// The lists of the running passes of store folder `dir` (pass_lists reads them); the records of
/// passes that ended go. A process that is its pass alone is left out: what it touched is its
/// live set (prune's `live`).
pub fn running_passes(dir: &Path) -> Vec<PathBuf> {
    let passes = dir.join("passes");
    let me = std::process::id().to_string();
    let alone = pass_token() == me;
    let mut lists = Vec::new();
    for e in std::fs::read_dir(&passes).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().into_owned();
        let Some(token) = n.strip_suffix(".held") else { continue };
        if alone && token == me {
            continue;
        }
        let (held, list) = (passes.join(&n), passes.join(token));
        let Ok(f) = std::fs::OpenOptions::new().read(true).write(true).open(&held) else { continue };
        if f.try_lock().is_ok() {
            // ended: no process of it holds the record (one starting now finds the file gone
            // and makes it again: open_pass)
            let _ = std::fs::remove_file(&list);
            let _ = std::fs::remove_file(&held);
            let _ = f.unlock();
            continue;
        }
        lists.push(list);
    }
    lists
}

/// The store names the pass lists `lists` (running_passes) hold.
pub fn pass_lists(lists: &[PathBuf]) -> std::collections::HashSet<String> {
    lists.iter().flat_map(|l| std::fs::read_to_string(l).unwrap_or_default().lines().map(str::to_string).collect::<Vec<_>>()).collect()
}

/// The programs this process runs (each one's root crate identity, cache.rs open) with the
/// store each of their roles uses (role -> store file name: a crate's front or code store, the
/// program's graph stores): their store sets (cache.rs write_sets).
static PROGRAMS: relax_session::sync::Mutex<Option<std::collections::BTreeMap<u64, std::collections::BTreeMap<String, String>>>> = relax_session::sync::Mutex::new(None);

/// Records the stores program `key` uses, by role (a later call for the same program, after a
/// live edit, replaces them).
pub fn note_program(key: u64, roles: &std::collections::BTreeMap<String, String>) {
    // process state: copied on the kept heap
    let _k = crate::zone_alloc::kept();
    let roles: std::collections::BTreeMap<String, String> = roles.iter().map(|(r, n)| (r.as_str().to_owned(), n.as_str().to_owned())).collect();
    PROGRAMS.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(Default::default).insert(key, roles);
}

/// The programs this process runs, with their stores by role.
pub fn programs() -> Vec<(u64, std::collections::BTreeMap<String, String>)> {
    PROGRAMS.lock().unwrap_or_else(|e| e.into_inner()).iter().flatten().map(|(k, v)| (*k, v.clone())).collect()
}

/// The payloads of the entries of `kind` in the store file `p`, read (not mapped, and not marking
/// the store as used by this run); None when it is unreadable or locked by a writer right now.
pub fn peek(p: &Path, kind: u32) -> Option<Vec<Vec<u8>>> {
    let mut f = std::fs::File::open(p).ok()?;
    f.try_lock_shared().ok()?;
    let r = (|| -> Option<Vec<Vec<u8>>> {
        let mut hb = [0u8; HEADER];
        f.read_exact(&mut hb).ok()?;
        let h = Header::parse(&hb)?;
        let len = f.metadata().ok()?.len();
        let (k, ix) = usable_slot(&mut f, &h, len)?;
        let end = h.slots[k as usize].end;
        let mut out = Vec::new();
        for e in ix.into_iter().filter(|e| e.kind == kind && e.off + e.len as u64 <= end) {
            let mut b = vec![0u8; e.len as usize];
            f.seek(SeekFrom::Start(e.off)).ok()?;
            f.read_exact(&mut b).ok()?;
            if sum64(&b) == e.sum {
                out.push(b);
            }
        }
        Some(out)
    })();
    let _ = f.unlock();
    r
}

/// Test only: rewrites `p`'s header as if `build` had written it.
#[cfg(test)]
pub fn set_build_for_test(p: &Path, build: u64) {
    let mut b = std::fs::read(p).unwrap();
    b[BUILD_AT..BUILD_AT + 8].copy_from_slice(&build.to_le_bytes());
    let s = sum64(&b[..HEADER - 8]);
    b[HEADER - 8..HEADER].copy_from_slice(&s.to_le_bytes());
    std::fs::write(p, b).unwrap();
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

/// Tests: the payload reads this thread made since the last call (payload_matches,
/// Store::verify_at).
#[cfg(test)]
pub(crate) fn payload_reads() -> u32 {
    tests::PAYLOAD_READS.with(|r| r.replace(0))
}

/// Tests: every store this process opens or writes is taken to be of the epoch `e` (0: the
/// real one); a test that sets it runs in a process of its own.
#[cfg(test)]
pub(crate) fn set_epoch(e: u64) {
    tests::EPOCH.store(e, Ordering::SeqCst);
}

#[cfg(test)]
mod tests {
    use super::*;

    thread_local! {
        /// The payload reads this thread made (payload_matches, Store::verify_at)
        pub(super) static PAYLOAD_READS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    /// The epoch every store of this process is taken to be of (`set_epoch`; 0: the real one).
    pub(super) static EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("rcs-test-{}-{}", tag, std::process::id()));
        let _ = std::fs::create_dir_all(&d);
        d
    }

    /// A first load is led by one run: its flight file stays locked until the leader has recorded
    /// the set (what another run's lock waits for), the leader does not wait on itself, and a
    /// landing leaves no file, so the next run leads.
    #[test]
    fn a_flight_is_held_by_its_leader_until_it_lands() {
        let d = tmpdir("flight");
        let identity = 0x00f1_1957_u128 << 64 | std::process::id() as u128;
        let file = d.join("flight").join(format!("{identity:032x}"));
        assert!(!flight(&d, identity), "the first run leads");
        let other = std::fs::OpenOptions::new().read(true).write(true).open(&file).unwrap();
        assert!(other.try_lock().is_err(), "another run finds it held, and waits");
        assert!(!flight(&d, identity), "the leader does not wait on itself");
        flight_landed();
        assert!(other.try_lock().is_ok(), "landed: the waiting run goes on");
        assert!(!file.exists(), "a landed flight leaves no file");
        assert!(!flight(&d, identity), "a run after the landing leads again");
        flight_landed();
        std::fs::remove_dir_all(d).unwrap();
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
    fn a_corrupt_payload_is_a_checked_miss() {
        let d = tmpdir("checked");
        let p = d.join("core-00112233445566aa.rcs");
        append(&p, vec![NewEntry { key: 7, kind: 1, payload: b"code bytes that run".to_vec() }], false).unwrap();
        // (a payload of this process's epoch: its unchecked reads do not check it)
        let off = {
            let s = Store::open(&p).unwrap();
            assert_eq!(s.get_checked(7).unwrap().1, b"code bytes that run");
            s.index[0].off as usize
        };
        // one flipped payload byte: the header and index checksums still pass
        let mut b = std::fs::read(&p).unwrap();
        b[off + 3] ^= 0x40;
        std::fs::write(&p, &b).unwrap();
        let s = Store::open(&p).unwrap();
        assert!(s.get(7).is_some(), "the unchecked read still finds it");
        assert!(s.get_checked(7).is_none(), "a corrupt payload must never be linked");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// An update replaces and deletes store files that running programs have mapped: neither may
    /// fail, what the programs hold stays readable, and a new program opens what is there now.
    #[test]
    fn a_mapped_store_can_be_replaced_and_deleted() {
        let d = tmpdir("mapped");
        let name = "core-0123456789abcdef.rcs";
        let p = d.join(name);
        append(&p, vec![NewEntry { key: 5, kind: 1, payload: b"held".to_vec() }], false).unwrap();
        let held = Store::open(&p).unwrap();
        let q = d.join("next-0123456789abcdef.rcs");
        append(&q, vec![NewEntry { key: 5, kind: 1, payload: b"next".to_vec() }], false).unwrap();
        crate::pack::write_atomic(&d, name, &std::fs::read(&q).unwrap()).unwrap();
        assert_eq!(held.get(5).unwrap().1, b"held");
        let next = Store::open(&p).unwrap();
        assert_eq!(next.get(5).unwrap().1, b"next");
        assert!(delete_checked(&p), "a mapped store is deleted");
        assert!(!p.exists());
        assert_eq!(held.get(5).unwrap().1, b"held");
        assert_eq!(next.get(5).unwrap().1, b"next");
        // the name is free at once, for a store of that name written while the old one is mapped
        append(&p, vec![NewEntry { key: 5, kind: 1, payload: b"new".to_vec() }], false).unwrap();
        assert_eq!(Store::open(&p).unwrap().get(5).unwrap().1, b"new");
        assert_eq!(held.get(5).unwrap().1, b"held");
        drop((held, next));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A writer's lock on the file (mandatory on Windows) does not stop a reader of the mapped bytes.
    #[test]
    fn a_mapped_store_is_read_and_verified_under_a_writers_lock() {
        let d = tmpdir("locked");
        let p = d.join("core-0123456789abcdef.rcs");
        append(&p, vec![NewEntry { key: 5, kind: 1, payload: b"five".to_vec() }], false).unwrap();
        let s = Store::open(&p).unwrap();
        let writer = std::fs::OpenOptions::new().read(true).write(true).open(&p).unwrap();
        writer.lock().unwrap();
        assert_eq!(s.get(5).unwrap().1, b"five");
        assert!(s.verify_at(0));
        assert_eq!(Bytes::map(&p).unwrap().len() as u64, std::fs::metadata(&p).unwrap().len(), "and it is mapped under that lock");
        writer.unlock().unwrap();
        drop((s, writer));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A payload's mapping holds its bytes wherever it lies: a view starts on the system's
    /// granularity (64 KB on Windows), a payload does not, and the file grows while views exist.
    #[test]
    fn run_store_payloads_map_at_any_offset() {
        let d = tmpdir("run-map");
        let run = RunStore::create(&d, "front_run", 64).unwrap();
        let payload = |i: usize| vec![i as u8 + 1; 30_000 + i * 977];
        let mapped: Vec<(usize, &'static [u8])> = (0..40).map(|i| {
            let p = payload(i);
            let Appended::Done(_, bytes) = run.append(i as u128, 1, p.len(), &|w| w.write_all(&p)) else { panic!("append {i}") };
            (i, bytes)
        }).collect();
        for (i, bytes) in mapped {
            assert_eq!(bytes, &payload(i)[..]);
            assert_eq!(bytes.as_ptr() as usize % 8, 0, "8-byte aligned, as a store payload is");
        }
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
        // the folder also holds this process's live lock (hold_build)
        std::fs::remove_dir_all(&d).unwrap();
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
        // (the store, this process's live lock folder (hold_build) and its pass record (note_in_pass))
        assert_eq!(std::fs::read_dir(&d).unwrap().flatten().filter(|e| e.file_name() != "live" && e.file_name() != "passes").count(), 1);
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
        // the folder also holds this process's live lock (hold_build)
        std::fs::remove_dir_all(d).unwrap();
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
        // the folder also holds this process's live lock (hold_build)
        std::fs::remove_dir_all(&d).unwrap();
    }

    /// The active commit of the store file at `p` and its index, read as an open reads them but
    /// without opening it.
    fn active(p: &Path) -> (Slot, Vec<Ix>) {
        let mut f = std::fs::File::open(current(&base_path(p))).unwrap();
        let mut hb = [0u8; HEADER];
        f.read_exact(&mut hb).unwrap();
        let h = Header::parse(&hb).unwrap();
        let len = f.metadata().unwrap().len();
        (h.slots[h.active as usize], slot_index(&mut f, &h, h.active, len).unwrap())
    }

    /// The payload reads this thread made since the last call.
    fn reads() -> u32 {
        PAYLOAD_READS.with(|r| r.replace(0))
    }

    /// No store write waits on the disk (a barrier forces a journal commit of the whole
    /// filesystem; on a busy disk the exit's waited behind one long enough to be killed). What a
    /// commit holds is trusted by its epoch instead: payloads written in this process's epoch (this
    /// boot, this mount of the folder) are served with no check, so an open and its reads read none
    /// of them. A commit on top of one of an earlier epoch says where its own payloads begin, and
    /// the earlier ones stay suspect: a process of a later epoch checks each once, at its first
    /// read, and one that did not reach the disk (zeroed: the machine went down before its
    /// writeback) is a miss for that entry alone while the others are served, through an open and
    /// a collected program's certified index alike. The background pass (`seal_suspects`) proves
    /// the clean ones with a plain header write that keeps the commit's generation, never into its
    /// own epoch's payloads, so later opens check nothing of them; a store with a failed payload is
    /// removed, never sealed over. A run store's commits are its process's epoch's.
    #[test]
    fn an_earlier_epochs_payloads_are_checked_at_first_read_and_sealed_by_evidence() {
        isolated_test!(an_earlier_epochs_payloads_are_checked_at_first_read_and_sealed_by_evidence);
        const A: u64 = 0xa1;
        const B: u64 = 0xb1;
        const C: u64 = 0xc1;
        let d = tmpdir("epochs");
        let entry = |key: u128, b: u8| NewEntry { key, kind: 1, payload: vec![b; 5000] };
        let payload = |b: u8| vec![b; 5000];
        let at = |ix: &[Ix], key: u128| ix.iter().find(|e| e.key == key).unwrap().off;

        // epoch A: a new store (a rewrite), then an append to it
        set_epoch(A);
        let p = d.join("core-00112233445566a1.rcs");
        append(&p, vec![entry(1, 1), entry(2, 2)], false).unwrap();
        append(&p, vec![entry(3, 3)], false).unwrap();
        let (a, _) = active(&p);
        assert_eq!((a.epoch, a.epoch_from), (A, a.tail), "an append in the commit's own epoch keeps where its payloads begin");
        reads();
        let st = Store::open(&p).unwrap();
        assert_eq!((st.get(1).unwrap().1, st.get(2).unwrap().1, st.get(3).unwrap().1), (&payload(1)[..], &payload(2)[..], &payload(3)[..]));
        assert_eq!(reads(), 0, "an open and reads of the current epoch's payloads read none of them");
        assert!(!suspects_pending(), "nothing of the current epoch's waits for a seal");
        drop(st);

        // epoch B (the next boot): an append over A's commit
        set_epoch(B);
        append(&p, vec![entry(4, 4)], false).unwrap();
        let (b, ix) = active(&p);
        assert_eq!((b.epoch, b.epoch_from, b.tail), (B, a.end, a.tail), "B's payloads begin at A's end; A's stay suspect");
        // key 2's payload never reached the disk
        let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        write_at(&f, &[0; 5000], at(&ix, 2)).unwrap();
        drop(f);
        reads();
        let st = Store::open(&p).unwrap();
        assert_eq!(st.get(4).unwrap().1, &payload(4)[..]);
        assert_eq!(reads(), 0, "the open and the current epoch's payload read nothing");
        assert_eq!(st.get(1).unwrap().1, &payload(1)[..]);
        assert_eq!(reads(), 1, "an earlier epoch's payload is checked at its first read");
        assert!(st.get(2).is_none(), "a suspect payload that did not reach the disk is a miss");
        assert_eq!(st.get(3).unwrap().1, &payload(3)[..], "and the others are served");
        assert_eq!(reads(), 2);
        assert!(st.get(1).is_some() && st.get(2).is_none() && st.get(3).is_some());
        assert_eq!(reads(), 0, "each is checked once");
        let certified = Store::open_certified(&p, Box::leak(st.index.to_vec().into_boxed_slice()), st.generation).unwrap();
        assert!(certified.get(2).is_none() && certified.get(1) == st.get(1), "a certified index's reads are checked alike");
        drop((st, certified));
        assert!(suspects_pending());
        let sealed = seal_suspects(&|| false);
        assert_eq!((sealed.stores, sealed.removed), (0, 1));
        assert!(!p.exists(), "a store with a failed payload is removed, not sealed over");

        // a clean store wholly of A's: sealed to its end, and it still opens and serves unchecked
        set_epoch(A);
        let r = d.join("core-00112233445566a3.rcs");
        append(&r, vec![entry(1, 1)], false).unwrap();
        set_epoch(B);
        drop(Store::open(&r).unwrap());
        assert_eq!(seal_suspects(&|| false).stores, 1);
        let (s, _) = active(&r);
        assert_eq!(s.tail, s.end, "an earlier epoch's whole commit is sealed to its end");
        let st = Store::open(&r).expect("a store sealed to its end opens");
        reads();
        assert_eq!(st.get(1).unwrap().1, &payload(1)[..]);
        assert_eq!(reads(), 0, "and its reads check nothing");
        drop(st);

        // a clean store of A's, appended to in B: sealed up to B's own payloads, by a header write
        set_epoch(A);
        let q = d.join("core-00112233445566a2.rcs");
        append(&q, vec![entry(1, 1), entry(2, 2)], false).unwrap();
        set_epoch(B);
        append(&q, vec![entry(3, 3)], false).unwrap();
        let (before, _) = active(&q);
        let generation = header_generation(&q).unwrap();
        let st = Store::open(&q).unwrap();
        let sealed = seal_suspects(&|| false);
        assert_eq!((sealed.stores, sealed.bytes, sealed.removed), (1, 10_000, 0));
        let (s, _) = active(&q);
        assert_eq!(s.tail, before.epoch_from, "sealed up to the current epoch's payloads, not into them");
        assert!(s.tail < s.end);
        assert_eq!(header_generation(&q), Some(generation), "a seal keeps the commit's generation");
        drop(st);
        reads();
        let st = Store::open(&q).unwrap();
        assert!((1..=3).all(|k| st.get(k).is_some()));
        assert_eq!(reads(), 0, "a sealed store's reads check nothing");
        assert!(!suspects_pending(), "nothing left to seal");
        drop(st);
        // epoch C: B's payloads are an earlier epoch's now; the sealed ones are proven
        set_epoch(C);
        let st = Store::open(&q).unwrap();
        assert!(st.get(1).is_some() && st.get(2).is_some());
        assert_eq!(reads(), 0, "sealed payloads are not checked again");
        assert_eq!(st.get(3).unwrap().1, &payload(3)[..]);
        assert_eq!(reads(), 1);
        drop(st);

        // a run store: its commits are its process's epoch's
        set_epoch(A);
        let run = RunStore::create(&d, "front_run", 8).unwrap();
        let Appended::Done(one, _) = run.append(1, 6, 3000, &|out| out.write_all(&[1; 3000])) else { panic!("append") };
        run.commit().unwrap();
        reads();
        let st = Store::open(&run.path).unwrap();
        assert_eq!(st.get(one).unwrap().1, &[1u8; 3000][..]);
        assert_eq!(reads(), 0, "a run store's reads in its own epoch check nothing");
        drop(st);
        set_epoch(B);
        let st = Store::open(&run.path).unwrap();
        assert_eq!(st.get(one).unwrap().1, &[1u8; 3000][..]);
        assert_eq!(reads(), 1, "an earlier epoch's run store is checked at its reads");
        drop((st, run));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A commit whose data end lies past the file's end (the file's growth did not reach the disk
    /// before its header did) gives way to the commit before it, in the other slot: nothing of it
    /// is served (by an open, a collected program's certified index, or a peek), the commit before
    /// stays warm, and the next commit builds on it.
    #[test]
    fn a_commit_past_the_files_end_falls_back_to_the_one_before() {
        let d = tmpdir("past-end");
        let p = d.join("core-00112233445566cc.rcs");
        append(&p, vec![NewEntry { key: 1, kind: 1, payload: vec![1; 5000] }], false).unwrap();
        append(&p, vec![NewEntry { key: 2, kind: 1, payload: vec![2; 5000] }], false).unwrap();
        let (_, ix) = active(&p);
        let two = ix.iter().find(|e| e.key == 2).unwrap().off;
        let generation = header_generation(&p).unwrap();
        std::fs::OpenOptions::new().write(true).open(&p).unwrap().set_len(two + 2500).unwrap();
        let st = Store::open(&p).expect("the commit before");
        assert_eq!(st.get(1).unwrap().1, &[1u8; 5000][..], "the commit before stays warm");
        assert!(st.get(2).is_none(), "nothing of a commit past the file's end is served");
        assert!(Store::open_certified(&p, Box::leak(ix.into_boxed_slice()), generation).is_none());
        assert_eq!(peek(&p, 1).unwrap(), vec![vec![1u8; 5000]], "a peek reads the commit before");
        append(&p, vec![NewEntry { key: 3, kind: 1, payload: vec![3; 100] }], false).unwrap();
        let next = Store::open(&p).unwrap();
        assert_eq!(next.get(1).unwrap().1, &[1u8; 5000][..]);
        assert_eq!(next.get(3).unwrap().1, &[3u8; 100][..]);
        assert!(next.get(2).is_none());
        drop((st, next));
        let _ = std::fs::remove_dir_all(&d);
    }
}
