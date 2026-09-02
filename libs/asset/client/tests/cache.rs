//! Cache semantics: atomic verified commits, resumable partials, pinning,
//! deterministic eviction under injected time, and fail-closed refusals.

// Native filesystem test uses wall time only to verify metadata aging.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

mod common;

use common::{payload, test_root};
use makepad_asset_client::cache::{CacheBudgets, ContentCache};
use makepad_asset_client::ClientError;
use makepad_asset_data::Sha256;

const NOW: u64 = 1_700_000_000_000;

fn digest_of(bytes: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(bytes);
    h.finalize()
}

fn small_budgets() -> CacheBudgets {
    CacheBudgets {
        max_total_bytes: 10_000,
        max_object_bytes: 4_000,
        max_partial_bytes: 8_000,
        stale_partial_ms: 1_000_000,
        max_ram_bytes: 64_000,
    }
}

#[test]
fn put_resolve_roundtrip_verified() {
    let root = test_root("roundtrip");
    let mut cache = ContentCache::open(&root, small_budgets(), NOW).unwrap();
    let bytes = payload(1, 1000);
    let digest = cache.put_bytes(&bytes, None, NOW).unwrap();
    assert_eq!(digest, digest_of(&bytes));
    let path = cache.resolve(&digest, NOW + 1).unwrap().expect("resolves");
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    assert_eq!(cache.read_verified(&digest, NOW + 2).unwrap().unwrap(), bytes);
    // Dedup keeps one object.
    cache.put_bytes(&bytes, Some(&digest), NOW + 3).unwrap();
    assert_eq!(cache.stats().object_count, 1);
    assert_eq!(cache.stats().total_bytes, 1000);
    // Wrong expectation refuses before anything lands.
    let err = cache.put_bytes(&bytes, Some(&[0u8; 32]), NOW).unwrap_err();
    assert!(matches!(err, ClientError::DigestMismatch { .. }));
}

#[test]
fn absent_is_none_not_a_guess() {
    let root = test_root("absent");
    let mut cache = ContentCache::open(&root, small_budgets(), NOW).unwrap();
    assert!(cache.resolve(&[9u8; 32], NOW).unwrap().is_none());
    assert!(cache.read_verified(&[9u8; 32], NOW).unwrap().is_none());
}

#[test]
fn corruption_self_heals_on_resolve() {
    let root = test_root("corrupt");
    let mut cache = ContentCache::open(&root, small_budgets(), NOW).unwrap();
    let bytes = payload(2, 500);
    let digest = cache.put_bytes(&bytes, None, NOW).unwrap();
    let path = cache.resolve(&digest, NOW).unwrap().unwrap();
    // Flip a byte on disk behind the cache's back.
    let mut on_disk = std::fs::read(&path).unwrap();
    on_disk[0] ^= 0xff;
    std::fs::write(&path, &on_disk).unwrap();
    // The corrupt object is never served: removed and reported absent.
    assert!(cache.resolve(&digest, NOW + 1).unwrap().is_none());
    assert!(!path.exists());
    let stats = cache.stats();
    assert_eq!(stats.corruption_evictions, 1);
    assert_eq!(stats.object_count, 0);
    assert_eq!(stats.total_bytes, 0);
}

#[test]
fn eviction_is_lru_and_never_touches_pins() {
    let root = test_root("evict");
    let mut cache = ContentCache::open(&root, small_budgets(), NOW).unwrap();
    let a = payload(10, 4_000);
    let b = payload(11, 4_000);
    let c = payload(12, 4_000);
    let da = cache.put_bytes(&a, None, NOW).unwrap(); // oldest
    let db = cache.put_bytes(&b, None, NOW + 10).unwrap();
    cache.pin(&da).unwrap(); // a is pinned despite being oldest
    // 8000 + 4000 > 10000: eviction must take b (oldest unpinned), not a.
    let dc = cache.put_bytes(&c, None, NOW + 20).unwrap();
    assert!(cache.resolve(&da, NOW + 30).unwrap().is_some(), "pinned survived");
    assert!(cache.resolve(&db, NOW + 30).unwrap().is_none(), "unpinned LRU evicted");
    assert!(cache.resolve(&dc, NOW + 30).unwrap().is_some());
    assert_eq!(cache.stats().evictions, 1);

    // Only unpinned content can go: adding d evicts c (the sole unpinned
    // object) even though it was just used.
    let d = payload(13, 4_000);
    let dd = cache.put_bytes(&d, None, NOW + 40).unwrap();
    assert!(cache.resolve(&dc, NOW + 50).unwrap().is_none());
    assert!(cache.resolve(&dd, NOW + 50).unwrap().is_some());
    assert!(cache.resolve(&da, NOW + 50).unwrap().is_some(), "pin still holds");
}

#[test]
fn admission_refusals_are_typed() {
    let root = test_root("admission");
    let mut cache = ContentCache::open(&root, small_budgets(), NOW).unwrap();
    // Over per-object budget.
    let big = payload(20, 5_000);
    let err = cache.put_bytes(&big, None, NOW).unwrap_err();
    assert!(matches!(err, ClientError::CacheAdmission { .. }));
    // Pinned bytes leave no room: pin 3 objects of 3000, then admit 3000.
    for seed in 30..33u64 {
        let bytes = payload(seed, 3_000);
        let d = cache.put_bytes(&bytes, None, NOW + seed).unwrap();
        cache.pin(&d).unwrap();
    }
    let more = payload(40, 3_000);
    let err = cache.put_bytes(&more, None, NOW + 100).unwrap_err();
    assert!(matches!(err, ClientError::CacheAdmission { .. }));
    // Nothing was evicted to satisfy the refused admission.
    assert_eq!(cache.stats().evictions, 0);
    assert_eq!(cache.stats().object_count, 3);
    // Unpinning one frees the room.
    let victim = digest_of(&payload(30, 3_000));
    cache.unpin(&victim).unwrap();
    cache.put_bytes(&more, None, NOW + 200).unwrap();
}

#[test]
fn partial_resume_across_writer_drop_and_reopen() {
    let root = test_root("partial");
    let full = payload(50, 2_000);
    let digest = digest_of(&full);
    {
        let mut cache = ContentCache::open(&root, small_budgets(), NOW).unwrap();
        let mut w = cache.open_partial(&digest).unwrap();
        assert_eq!(w.resumed_bytes(), 0);
        w.write(&full[..700]).unwrap();
        // Dropping the writer KEEPS the partial (that is the resume point).
        drop(w);
        assert_eq!(cache.partial_len(&digest), 700);
    }
    // A whole new cache instance (process restart) resumes from byte 700.
    let mut cache = ContentCache::open(&root, small_budgets(), NOW + 1).unwrap();
    let mut w = cache.open_partial(&digest).unwrap();
    assert_eq!(w.resumed_bytes(), 700);
    w.write(&full[700..]).unwrap();
    let path = cache.commit_partial(w, NOW + 2).unwrap();
    assert_eq!(std::fs::read(path).unwrap(), full);
    assert_eq!(cache.partial_len(&digest), 0, "partial consumed by commit");
    assert!(cache.resolve(&digest, NOW + 3).unwrap().is_some());
}

#[test]
fn partial_digest_mismatch_deletes_partial() {
    let root = test_root("partial_bad");
    let mut cache = ContentCache::open(&root, small_budgets(), NOW).unwrap();
    let digest = digest_of(&payload(60, 100));
    let mut w = cache.open_partial(&digest).unwrap();
    w.write(b"wrong bytes entirely").unwrap();
    let err = cache.commit_partial(w, NOW).unwrap_err();
    assert!(matches!(err, ClientError::DigestMismatch { .. }));
    assert_eq!(cache.partial_len(&digest), 0, "poisoned partial removed");
    assert!(cache.resolve(&digest, NOW).unwrap().is_none());
}

#[test]
fn partial_reset_restarts_hash_state() {
    let root = test_root("partial_reset");
    let mut cache = ContentCache::open(&root, small_budgets(), NOW).unwrap();
    let full = payload(61, 300);
    let digest = digest_of(&full);
    let mut w = cache.open_partial(&digest).unwrap();
    w.write(b"garbage prefix").unwrap();
    w.reset().unwrap();
    assert_eq!(w.resumed_bytes(), 0);
    w.write(&full).unwrap();
    cache.commit_partial(w, NOW).unwrap();
    assert!(cache.resolve(&digest, NOW).unwrap().is_some());
}

#[test]
fn stale_partials_swept_at_open_fresh_kept() {
    let root = test_root("partial_sweep");
    let digest = digest_of(&payload(70, 100));
    {
        let mut cache = ContentCache::open(&root, small_budgets(), NOW).unwrap();
        let mut w = cache.open_partial(&digest).unwrap();
        w.write(b"resume me").unwrap();
    }
    // Reopen "now": fresh partial survives.
    {
        let cache = ContentCache::open(&root, small_budgets(), NOW).unwrap();
        assert_eq!(cache.partial_len(&digest), 9);
    }
    // Reopen far in the future: swept.
    let far = NOW + small_budgets().stale_partial_ms + 60 * 60 * 1000;
    // The file's real mtime is "now" (wall clock), so measure staleness from
    // real wall time plus the budget.
    let wall_now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    let cache =
        ContentCache::open(&root, small_budgets(), wall_now + small_budgets().stale_partial_ms + 10)
            .unwrap();
    let _ = far;
    assert_eq!(cache.partial_len(&digest), 0, "stale partial swept");
}

#[test]
fn tmp_never_survives_open_and_foreign_partials_removed() {
    let root = test_root("sweep_tmp");
    {
        let _ = ContentCache::open(&root, small_budgets(), NOW).unwrap();
    }
    std::fs::write(root.join("tmp").join("w999-0.part"), b"orphan").unwrap();
    std::fs::write(root.join("partial").join("not-a-digest.part"), b"foreign").unwrap();
    let _ = ContentCache::open(&root, small_budgets(), NOW + 1).unwrap();
    assert!(std::fs::read_dir(root.join("tmp")).unwrap().next().is_none());
    assert!(std::fs::read_dir(root.join("partial")).unwrap().next().is_none());
}

#[test]
fn index_and_pins_survive_reopen() {
    let root = test_root("reopen");
    let bytes = payload(80, 1_200);
    let digest;
    {
        let mut cache = ContentCache::open(&root, small_budgets(), NOW).unwrap();
        digest = cache.put_bytes(&bytes, None, NOW).unwrap();
        cache.pin(&digest).unwrap();
    }
    let mut cache = ContentCache::open(&root, small_budgets(), NOW + 10).unwrap();
    assert!(cache.is_pinned(&digest));
    assert_eq!(cache.stats().object_count, 1);
    assert_eq!(cache.stats().total_bytes, 1_200);
    assert_eq!(cache.stats().pinned_bytes, 1_200);
    assert_eq!(cache.read_verified(&digest, NOW + 11).unwrap().unwrap(), bytes);
}

#[test]
fn budgets_validate() {
    let root = test_root("budget_validate");
    let mut b = small_budgets();
    b.max_object_bytes = b.max_total_bytes + 1;
    let err = ContentCache::open(&root, b, NOW).err().expect("refused");
    assert!(matches!(err, ClientError::InvalidInput { .. }));
    let mut z = small_budgets();
    z.max_total_bytes = 0;
    assert!(ContentCache::open(&root, z, NOW).is_err());
}
