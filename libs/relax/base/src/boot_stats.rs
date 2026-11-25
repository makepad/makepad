//! What the native boot window (the "Ignition Field") reads once per frame. The loader's threads
//! only ever do relaxed or release atomic stores and adds here: no lock, and nothing of the
//! compiler zone is kept in a static (the install log's chunks are allocated on the system heap,
//! once per doubling of the log). The window reads at its own pace.
//!
//! Every function installed (compiled this run, linked from the cache, built in the background,
//! tiered up to O3) is one entry of the install log, in install order: its slot, its weight (the
//! code's size on a log scale) and whether it is O3 code. The field draws one star per function
//! and lights a function's own star when it is installed again.
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicU64, Ordering::{AcqRel, Acquire, Relaxed, Release}};

/// Entries of the install log's first chunk; chunk `k` holds `CHUNK0 << k`, so the 32 chunks cover
/// every u32 index.
const CHUNK0: usize = 1024;
const CHUNKS: usize = 32;
const WRITTEN: u64 = 1 << 63;
const O3: u64 = 1 << 40;
const AUX: u64 = 1 << 41;

#[allow(clippy::declare_interior_mutable_const)]
const NO_CHUNK: AtomicPtr<AtomicU64> = AtomicPtr::new(std::ptr::null_mut());

/// Entries reserved in the install log (an entry reads 0 until its writer stored it).
static INSTALLS: AtomicU32 = AtomicU32::new(0);
static LOG: [AtomicPtr<AtomicU64>; CHUNKS] = [NO_CHUNK; CHUNKS];

pub static FILES_PARSED: AtomicU32 = AtomicU32::new(0);
pub static LINES_PARSED: AtomicU64 = AtomicU64::new(0);
/// ms since process start (clock::ms_since_process_start) of main()'s first call; 0 before it
pub static MAIN_AT_MS: AtomicU64 = AtomicU64::new(0);

pub static O3_SUPERSEDED: AtomicU32 = AtomicU32::new(0);

/// The first settle (the compiles' state dropped, the heap trimmed): its end in ms since process
/// start + 1 (0: not yet), and the resident set (MB) around it.
pub static SETTLE_END_MS: AtomicU64 = AtomicU64::new(0);
pub static SETTLE_RSS_BEFORE_MB: AtomicU32 = AtomicU32::new(0);
pub static SETTLE_RSS_AFTER_MB: AtomicU32 = AtomicU32::new(0);

/// One function installed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Install {
    /// its slot in its unit
    pub slot: u32,
    /// the slot is the proc-macro world's (an auxiliary unit: its own slot numbers)
    pub aux: bool,
    /// the code's size on a log scale (`weight_of`)
    pub weight: u8,
    /// O3 code (a tier-up, or an `#[relax(O3, always)]` function's first code)
    pub o3: bool,
}

/// A code size as a weight: 8 * log2(bytes), the fraction in eighths read off the three bits below
/// the top one (so it never decreases as the size grows); 1 byte or less is 0, 2^31 and over 255.
pub fn weight_of(bytes: usize) -> u8 {
    let b = (bytes as u64).max(1);
    let e = 63 - b.leading_zeros() as u64;
    let frac = if e >= 3 { (b >> (e - 3)) & 7 } else { (b << (3 - e)) & 7 };
    (e * 8 + frac).min(255) as u8
}

/// A function's code installed: `code_bytes` of it for slot `slot` (`aux`: of the proc-macro
/// world's unit). Any thread; a relaxed add and a release store (and once per doubling of the
/// log, its next chunk).
pub fn fn_installed(slot: u32, aux: bool, code_bytes: usize) {
    record(slot, aux, code_bytes, false);
}

/// A function's O3 code installed.
pub fn o3_installed(slot: u32, aux: bool, code_bytes: usize) {
    record(slot, aux, code_bytes, true);
}

fn record(slot: u32, aux: bool, code_bytes: usize, o3: bool) {
    let i = INSTALLS.fetch_add(1, Relaxed);
    let entry = WRITTEN | slot as u64 | (weight_of(code_bytes) as u64) << 32 | if o3 { O3 } else { 0 } | if aux { AUX } else { 0 };
    entry_at(i, true).unwrap().store(entry, Release);
}

/// Chunk and offset of entry `i`.
fn place(i: u32) -> (usize, usize) {
    let n = i as u64 / CHUNK0 as u64 + 1;
    let k = 63 - n.leading_zeros() as usize;
    (k, i as usize - CHUNK0 * ((1 << k) - 1))
}

/// Entry `i`'s cell; `make`: its chunk is allocated if it has none yet.
fn entry_at(i: u32, make: bool) -> Option<&'static AtomicU64> {
    let (k, at) = place(i);
    let mut chunk = LOG[k].load(Acquire);
    if chunk.is_null() {
        if !make {
            return None;
        }
        let len = CHUNK0 << k;
        let fresh = crate::zone_alloc::kept_with(|| Box::into_raw((0..len).map(|_| AtomicU64::new(0)).collect::<Box<[AtomicU64]>>()));
        let fresh = fresh as *mut AtomicU64;
        chunk = match LOG[k].compare_exchange(std::ptr::null_mut(), fresh, AcqRel, Acquire) {
            Ok(_) => fresh,
            Err(theirs) => {
                // another thread made it first
                crate::zone_alloc::kept_with(|| drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(fresh, len)) }));
                theirs
            }
        };
    }
    // SAFETY: a chunk, once made, stays (`reset_statics` zeroes it) and holds CHUNK0 << k entries
    Some(unsafe { &*chunk.add(at) })
}

/// Entries reserved in the install log: `install(i)` for `i` below it is there or about to be.
pub fn installs() -> u32 {
    INSTALLS.load(Acquire)
}

/// The `i`-th install of the run; None while its writer has not stored it.
pub fn install(i: u32) -> Option<Install> {
    let v = entry_at(i, false)?.load(Acquire);
    (v & WRITTEN != 0).then(|| Install {
        slot: v as u32,
        aux: v & AUX != 0,
        weight: (v >> 32) as u8,
        o3: v & O3 != 0,
    })
}

pub fn settle_done(rss_before: u64, rss_after: u64) {
    if SETTLE_END_MS.load(Relaxed) != 0 {
        return;
    }
    SETTLE_RSS_BEFORE_MB.store((rss_before >> 20) as u32, Relaxed);
    SETTLE_RSS_AFTER_MB.store((rss_after >> 20) as u32, Relaxed);
    SETTLE_END_MS.store(crate::clock::ms_since_process_start() as u64 + 1, Relaxed);
}

pub fn main_called() {
    MAIN_AT_MS.store(crate::clock::ms_since_process_start() as u64 + 1, Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_grow_with_the_code() {
        assert_eq!((weight_of(0), weight_of(1), weight_of(2), weight_of(3)), (0, 0, 8, 12));
        assert_eq!((weight_of(1 << 10), weight_of(1 << 31), weight_of(usize::MAX)), (80, 248, 255));
        let mut last = 0;
        for bytes in (1..1 << 16).chain((16..40).map(|e| 1usize << e)) {
            let w = weight_of(bytes);
            assert!(w >= last, "weight falls at {bytes} bytes: {w} < {last}");
            last = w;
        }
    }

    // one test: the log is a process-wide static
    #[test]
    fn every_install_is_logged_once_across_chunks_and_threads() {
        // the chunks: 1024, 2048, 4096 .. entries, the last one reaching the end of u32
        assert_eq!((place(0), place(1023), place(1024), place(3071), place(3072)), ((0, 0), (0, 1023), (1, 0), (1, 2047), (2, 0)));
        assert!(place(u32::MAX).0 < CHUNKS);
        const PER: u32 = 1000;
        let threads: Vec<_> = (0..4u32)
            .map(|t| {
                std::thread::spawn(move || {
                    for j in 0..PER {
                        let slot = t * PER + j;
                        if j % 3 == 0 {
                            o3_installed(slot, t == 3, 1 << (j % 20));
                        } else {
                            fn_installed(slot, t == 3, 1 << (j % 20));
                        }
                    }
                })
            })
            .collect();
        threads.into_iter().for_each(|t| t.join().unwrap());
        assert_eq!(installs(), 4 * PER);
        let mut seen: Vec<Install> = (0..installs()).map(|i| install(i).expect("an install not stored")).collect();
        seen.sort_by_key(|x| x.slot);
        for (n, x) in seen.iter().enumerate() {
            let (t, j) = (n as u32 / PER, n as u32 % PER);
            assert_eq!(*x, Install { slot: n as u32, aux: t == 3, weight: weight_of(1 << (j % 20)), o3: j % 3 == 0 });
        }
        assert_eq!(install(installs()), None);
    }
}

/// What this module holds of a run, as a fresh process has it (`run_end`). Nothing of the run is left that could see it.
pub(crate) unsafe fn reset_statics() {
    use relax_session::reset_static as reset;
    // the install log's chunks stay, every entry zero again: the boot window outlives a run's end
    // in place and may still be reading them, so they are never freed (the next run's capacity)
    for i in 0..INSTALLS.load(Relaxed) {
        if let Some(entry) = entry_at(i, false) {
            entry.store(0, Relaxed);
        }
    }
    reset(&INSTALLS, AtomicU32::new(0));
    reset(&LOG, std::array::from_fn(|k| AtomicPtr::new(LOG[k].load(Relaxed))));
    reset(&FILES_PARSED, AtomicU32::new(0));
    reset(&LINES_PARSED, AtomicU64::new(0));
    reset(&MAIN_AT_MS, AtomicU64::new(0));
    reset(&O3_SUPERSEDED, AtomicU32::new(0));
    reset(&SETTLE_END_MS, AtomicU64::new(0));
    reset(&SETTLE_RSS_BEFORE_MB, AtomicU32::new(0));
    reset(&SETTLE_RSS_AFTER_MB, AtomicU32::new(0));
}
