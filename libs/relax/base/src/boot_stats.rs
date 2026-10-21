//! Counters the native boot window reads once per frame (the "Ignition Field"). The loader's
//! threads only ever do a relaxed atomic add or store here: no lock, no allocation (so nothing
//! of the compiler zone is kept in a static), and the window reads them at its own pace.
//!
//! An arm of the field is a crate: the first `ARMS - 1` crates that get a function installed each
//! open an arm of their own (in the order the program first needs them), the rest share the last
//! one.
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicU8, Ordering::Relaxed};

pub const ARMS: usize = 16;
/// The arm every crate past the first `ARMS - 1` shares.
pub const REST: usize = ARMS - 1;
const CRATES: usize = 1024;
const NAME_WORDS: usize = 3;
pub const NAME_BYTES: usize = NAME_WORDS * 8;
const UNSET: u8 = 0xff;
/// O3 installs the window can still see when it falls behind (older ones are skipped).
pub const O3_RING: usize = 1024;

#[allow(clippy::declare_interior_mutable_const)]
const ZERO32: AtomicU32 = AtomicU32::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const ZERO64: AtomicU64 = AtomicU64::new(0);
#[allow(clippy::declare_interior_mutable_const)]
const UNSET8: AtomicU8 = AtomicU8::new(UNSET);
#[allow(clippy::declare_interior_mutable_const)]
const NAME: [AtomicU64; NAME_WORDS] = [ZERO64; NAME_WORDS];

/// Functions installed per arm (compiled this run, or linked from the cache).
static FNS: [AtomicU32; ARMS] = [ZERO32; ARMS];
static ARM_OF_CRATE: [AtomicU8; CRATES] = [UNSET8; CRATES];
static ARMS_OPEN: AtomicU32 = AtomicU32::new(0);
static ARM_NAME: [[AtomicU64; NAME_WORDS]; ARMS] = [NAME; ARMS];

pub static FILES_PARSED: AtomicU32 = AtomicU32::new(0);
pub static LINES_PARSED: AtomicU64 = AtomicU64::new(0);
/// ms since process start (clock::ms_since_process_start) of main()'s first call; 0 before it
pub static MAIN_AT_MS: AtomicU64 = AtomicU64::new(0);

pub static O3_INSTALLED: AtomicU32 = AtomicU32::new(0);
pub static O3_SUPERSEDED: AtomicU32 = AtomicU32::new(0);
static O3_ARMS: [AtomicU8; O3_RING] = [UNSET8; O3_RING];
/// The function of each ring entry's install, to pick the star it ignites (hash of its slot).
static O3_KEYS: [AtomicU32; O3_RING] = [ZERO32; O3_RING];

/// The compiler-zone swap: its end in ms since process start + 1 (0: not yet), the chunks it
/// destroyed, and the resident set (MB) around it.
pub static SWAP_END_MS: AtomicU64 = AtomicU64::new(0);
pub static SWAP_CHUNKS: AtomicU32 = AtomicU32::new(0);
pub static SWAP_RSS_BEFORE_MB: AtomicU32 = AtomicU32::new(0);
pub static SWAP_RSS_AFTER_MB: AtomicU32 = AtomicU32::new(0);

/// The arm of crate `krate`; `name` is asked only when the crate opens an arm.
pub fn arm_of(krate: u32, name: impl FnOnce() -> [u8; NAME_BYTES]) -> usize {
    let Some(slot) = ARM_OF_CRATE.get(krate as usize) else {
        return REST;
    };
    let arm = slot.load(Relaxed);
    if arm != UNSET {
        return arm as usize;
    }
    let next = ARMS_OPEN.fetch_add(1, Relaxed) as usize;
    let arm = next.min(REST);
    if arm < REST {
        let bytes = name();
        for (word, chunk) in ARM_NAME[arm].iter().zip(bytes.chunks(8)) {
            word.store(u64::from_le_bytes(chunk.try_into().unwrap()), Relaxed);
        }
    }
    // two threads can race for a crate's first function: the later store wins, the other
    // thread's arm number stays unused
    slot.store(arm as u8, Relaxed);
    arm
}

/// A crate name cut to the name field (zero padded).
pub fn name_bytes(name: &str) -> [u8; NAME_BYTES] {
    let mut out = [0; NAME_BYTES];
    let n = name.len().min(NAME_BYTES);
    out[..n].copy_from_slice(&name.as_bytes()[..n]);
    out
}

pub fn fn_installed(arm: usize) {
    FNS[arm.min(REST)].fetch_add(1, Relaxed);
}

pub fn fns(arm: usize) -> u32 {
    FNS[arm.min(REST)].load(Relaxed)
}

/// The arms opened so far (REST included once it is in use).
pub fn arms_open() -> usize {
    (ARMS_OPEN.load(Relaxed) as usize).min(ARMS)
}

pub fn arm_name(arm: usize) -> [u8; NAME_BYTES] {
    let mut out = [0; NAME_BYTES];
    if arm < REST {
        for (chunk, word) in out.chunks_mut(8).zip(&ARM_NAME[arm]) {
            chunk.copy_from_slice(&word.load(Relaxed).to_le_bytes());
        }
    }
    out
}

pub fn o3_installed(arm: usize, key: u32) {
    let n = O3_INSTALLED.fetch_add(1, Relaxed) as usize % O3_RING;
    O3_KEYS[n].store(key, Relaxed);
    O3_ARMS[n].store(arm.min(REST) as u8, Relaxed);
}

/// The `index`-th O3 install overall (arm, key), while it is within the last `O3_RING`.
pub fn o3_install(index: u32) -> (usize, u32) {
    let n = index as usize % O3_RING;
    (O3_ARMS[n].load(Relaxed) as usize % ARMS, O3_KEYS[n].load(Relaxed))
}

pub fn swap_done(chunks: usize, rss_before: u64, rss_after: u64) {
    SWAP_CHUNKS.store(chunks as u32, Relaxed);
    SWAP_RSS_BEFORE_MB.store((rss_before >> 20) as u32, Relaxed);
    SWAP_RSS_AFTER_MB.store((rss_after >> 20) as u32, Relaxed);
    SWAP_END_MS.store(crate::clock::ms_since_process_start() as u64 + 1, Relaxed);
}

pub fn main_called() {
    MAIN_AT_MS.store(crate::clock::ms_since_process_start() as u64 + 1, Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    // one test: the counters are process-wide statics
    #[test]
    fn arms_open_in_order_and_the_rest_share_one() {
        for krate in 0..40u32 {
            let arm = arm_of(900 + krate, || name_bytes(&format!("crate_{krate}")));
            assert_eq!(arm, (krate as usize).min(REST));
            // the same crate keeps its arm
            assert_eq!(arm_of(900 + krate, || unreachable!()), arm);
        }
        let name = arm_name(3);
        assert!(name.starts_with(b"crate_3\0"));
        fn_installed(3);
        fn_installed(3);
        fn_installed(REST + 5);
        assert_eq!(fns(3), 2);
        assert_eq!(fns(REST), 1);
        // a crate past the table is the rest arm and does not open one
        assert_eq!(arm_of(100_000, || unreachable!()), REST);
        let long = name_bytes("a_crate_name_longer_than_the_field");
        assert_eq!(&long[..], b"a_crate_name_longer_than");
        o3_installed(2, 77);
        let n = O3_INSTALLED.load(Relaxed) - 1;
        assert_eq!(o3_install(n), (2, 77));
    }
}
