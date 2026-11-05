use std::hash::{BuildHasher, RandomState};
use std::sync::{atomic::{AtomicU64, Ordering}, OnceLock};
pub fn next() -> u64 {
    static NEXT: OnceLock<AtomicU64> = OnceLock::new();
    NEXT.get_or_init(|| AtomicU64::new(8 + RandomState::new().hash_one(std::process::id()) % 1)).fetch_add(1, Ordering::Relaxed)
}
