#[cfg(test)]
mod tests {
    use super::*;
    use crate::zone_alloc::RelaxAlloc;
    use std::alloc::GlobalAlloc;
    use std::sync::{Arc, Barrier};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    unsafe fn checked(size: usize, align: usize) -> (*mut u8, Layout) {
        let l = Layout::from_size_align(size, align).unwrap();
        let p = RelaxAlloc.alloc(l);
        assert!(!p.is_null());
        assert!(owns(p));
        assert_eq!(p as usize % align, 0);
        std::ptr::write_bytes(p, 0x5a, size);
        (p, l)
    }

    #[test]
    fn class_table_matches_the_arithmetic() {
        // the domain fast paths index the table by size granule without raising to 16 first
        assert_eq!(SMALL_CLASS[0], 0);
        assert_eq!(SMALL_CLASS[1], 0);
        for need in 16..=1 << MAX_SHIFT {
            let c = class_for(need);
            assert_eq!(c, class_calc(need), "{need}");
            assert!(class_size(c) >= need && (c == 0 || class_size(c - 1) < need), "{need}");
        }
    }

    #[test]
    fn sizes_alignment_zeroing_and_escape() {
        let _serial = lock(&TEST_LOCK);
        trim();
        let before = stats();
        let mut blocks = Vec::new();
        {
            let _scope = scratch(&crate::options::RuntimeOptions::default());
            assert!(active());
            for size in [
                1,
                15,
                16,
                17,
                31,
                255,
                1024,
                2048,
                2049,
                8193,
                65536,
                262144,
                262145,
                CHUNK - 1,
                CHUNK,
                CHUNK + 1,
                3 * CHUNK,
            ] {
                for align in [1, 16, 64, 4096, CHUNK, 4 * CHUNK] {
                    unsafe {
                        blocks.push(checked(size, align));
                    }
                }
            }
        }
        assert!(!active());
        for (p, l) in blocks {
            unsafe {
                assert!(std::slice::from_raw_parts(p, l.size())
                    .iter()
                    .all(|&b| b == 0x5a));
                RelaxAlloc.dealloc(p, l);
            }
        }
        trim();
        assert_eq!(stats(), before);
        let _scope = scratch(&crate::options::RuntimeOptions::default());
        unsafe {
            for size in [16, 2048, 262145, CHUNK + 1] {
                let l = Layout::from_size_align(size, 64).unwrap();
                let p = RelaxAlloc.alloc_zeroed(l);
                assert!(!p.is_null() && owns(p));
                assert!(std::slice::from_raw_parts(p, size).iter().all(|&b| b == 0));
                RelaxAlloc.dealloc(p, l);
            }
        }
        trim();
        assert_eq!(stats(), before);
    }

    #[test]
    fn nested_scopes_unwind_and_realloc_between_heaps() {
        let _serial = lock(&TEST_LOCK);
        trim();
        let before = stats();
        unsafe {
            let l = Layout::from_size_align(97, 64).unwrap();
            let heap = RelaxAlloc.alloc(l);
            assert!(!heap.is_null() && !owns(heap));
            std::ptr::write_bytes(heap, 0x3c, l.size());
            let outer = scratch(&crate::options::RuntimeOptions::default());
            let inner = scratch(&crate::options::RuntimeOptions::default());
            drop(outer); // depth also tolerates guards dropped out of order
            assert!(active());
            let arena = RelaxAlloc.realloc(heap, l, 3000);
            assert!(!arena.is_null() && owns(arena));
            assert!(std::slice::from_raw_parts(arena, 97)
                .iter()
                .all(|&b| b == 0x3c));
            let l2 = Layout::from_size_align(3000, 64).unwrap();
            std::ptr::write_bytes(arena, 0x6d, 3000);
            let big = RelaxAlloc.realloc(arena, l2, CHUNK + 7);
            assert!(!big.is_null() && owns(big));
            assert!(std::slice::from_raw_parts(big, 3000)
                .iter()
                .all(|&b| b == 0x6d));
            let l3 = Layout::from_size_align(CHUNK + 7, 64).unwrap();
            let small = RelaxAlloc.realloc(big, l3, 32);
            assert!(!small.is_null() && owns(small));
            drop(inner);
            let l4 = Layout::from_size_align(32, 64).unwrap();
            let heap = RelaxAlloc.realloc(small, l4, 200);
            assert!(!heap.is_null() && !owns(heap));
            assert!(std::slice::from_raw_parts(heap, 32)
                .iter()
                .all(|&b| b == 0x6d));
            RelaxAlloc.dealloc(heap, Layout::from_size_align(200, 64).unwrap());
        }
        let _ = std::panic::catch_unwind(|| {
            let _scope = scratch(&crate::options::RuntimeOptions::default());
            assert!(active());
            panic!("test scope unwind");
        });
        assert!(!active());
        trim();
        assert_eq!(stats(), before);
    }

    #[test]
    fn empty_current_chunk_decommits_and_stale_cache_is_safe() {
        let _serial = lock(&TEST_LOCK);
        trim();
        let before = stats();
        let _scope = scratch(&crate::options::RuntimeOptions::default());
        unsafe {
            let (p, l) = checked(64, 16);
            let index = (p as usize - DEFAULT.base.load(Ordering::Relaxed)) / CHUNK;
            let generation = lock(&DEFAULT.headers[index].state).generation;
            assert_eq!(stats(), (before.0 + CHUNK, before.1 + 64, before.2 + 1));
            RelaxAlloc.dealloc(p, l);
            assert_eq!(DEFAULT.headers[index].live.load(Ordering::Relaxed), 0);
            // emptied with 64 bytes touched: kept warm (committed) until trim
            assert_eq!(lock(&DEFAULT.headers[index].state).committed, CHUNK);
            assert_eq!(stats(), (before.0 + CHUNK, before.1, before.2 + 1));
            trim();
            assert_eq!(lock(&DEFAULT.headers[index].state).committed, 0);
            assert_eq!(stats(), before); // scope still active; no TLS pin
                                         // A different thread reuses the exact chunk while our TLS still caches it.
            let barrier = Arc::new(Barrier::new(2));
            let remote_barrier = barrier.clone();
            let old_address = p as usize;
            let remote = std::thread::spawn(move || {
                let _scope = scratch(&crate::options::RuntimeOptions::default());
                let (other, layout) = checked(64, 16);
                assert_eq!(other as usize, old_address);
                remote_barrier.wait();
                remote_barrier.wait();
                assert_eq!(*other, 0x5a);
                RelaxAlloc.dealloc(other, layout);
            });
            barrier.wait();
            assert_ne!(lock(&DEFAULT.headers[index].state).generation, generation);
            let (next, layout) = checked(64, 16);
            assert_ne!((next as usize) & !(CHUNK - 1), (p as usize) & !(CHUNK - 1));
            RelaxAlloc.dealloc(next, layout);
            barrier.wait();
            remote.join().unwrap();
            trim();
        assert_eq!(stats(), before);
        }
    }

    #[test]
    fn remote_frees_during_allocation_and_after_owner_exit() {
        let _serial = lock(&TEST_LOCK);
        trim();
        let before = stats();
        let mut workers = Vec::new();
        for _ in 0..4 {
            workers.push(std::thread::spawn(|| {
                let (tx, rx) = std::sync::mpsc::sync_channel::<(usize, Layout)>(128);
                let freer = std::thread::spawn(move || {
                    for (p, l) in rx {
                        unsafe {
                            assert_eq!(*(p as *const u8), 0x5a);
                            RelaxAlloc.dealloc(p as *mut u8, l);
                        }
                    }
                });
                let mut escaped = Vec::new();
                {
                    let _scope = scratch(&crate::options::RuntimeOptions::default());
                    for i in 0..12000 {
                        unsafe {
                            let (p, l) = checked(16 << (i % 9), 16);
                            if i % 127 == 0 {
                                escaped.push((p as usize, l));
                            } else {
                                tx.send((p as usize, l)).unwrap();
                            }
                        }
                    }
                }
                drop(tx);
                freer.join().unwrap();
                escaped
            }));
        }
        for worker in workers {
            for (p, l) in worker.join().unwrap() {
                unsafe {
                    assert!(std::slice::from_raw_parts(p as *const u8, l.size())
                        .iter()
                        .all(|&b| b == 0x5a));
                    RelaxAlloc.dealloc(p as *mut u8, l);
                }
            }
        }
        trim();
        assert_eq!(stats(), before);
    }

    #[test]
    fn warm_chunks_are_reused_without_commit_and_bounded() {
        let _serial = lock(&TEST_LOCK);
        trim();
        let before = stats();
        let _scope = scratch(&crate::options::RuntimeOptions::default());
        unsafe {
            // a compile-like scope: a few classes, all freed at the end
            let mut blocks = Vec::new();
            for size in [16, 48, 200, 1000, 5000] {
                blocks.push(checked(size, 16));
            }
            let chunks: Vec<usize> = blocks.iter().map(|(p, _)| (*p as usize) & !(CHUNK - 1)).collect();
            for (p, l) in blocks.drain(..) {
                RelaxAlloc.dealloc(p, l);
            }
            let warm = stats();
            assert_eq!(warm.0, before.0 + 5 * CHUNK); // kept committed
            assert_eq!(warm.1, before.1);
            // the next scope's chunks come from the warm list: nothing newly committed
            for size in [16, 48, 200, 1000, 5000] {
                blocks.push(checked(size, 16));
            }
            assert_eq!(stats().0, warm.0);
            for (p, _) in &blocks {
                assert!(chunks.contains(&((*p as usize) & !(CHUNK - 1))));
            }
            for (p, l) in blocks.drain(..) {
                RelaxAlloc.dealloc(p, l);
            }
            // a chunk touched beyond WARM_TOUCHED_MAX is decommitted when it empties
            let n = WARM_TOUCHED_MAX / 4096 + 2;
            for _ in 0..n {
                blocks.push(checked(4096, 16));
            }
            let big = (blocks[0].0 as usize) & !(CHUNK - 1);
            for (p, l) in blocks.drain(..) {
                RelaxAlloc.dealloc(p, l);
            }
            let index = (big - DEFAULT.base.load(Ordering::Relaxed)) / CHUNK;
            assert_eq!(lock(&DEFAULT.headers[index].state).committed, 0);
        }
        trim();
        assert_eq!(stats(), before);
    }

    #[test]
    fn free_slots_reused_and_full_chunks_survive_escape() {
        let _serial = lock(&TEST_LOCK);
        trim();
        let before = stats();
        let mut blocks = Vec::new();
        {
            let _scope = scratch(&crate::options::RuntimeOptions::default());
            unsafe {
                for _ in 0..(CHUNK / 2048 + 17) {
                    blocks.push(checked(2048, 16));
                }
                assert_eq!(stats().0, before.0 + 2 * CHUNK);
                let (p, l) = blocks.pop().unwrap();
                RelaxAlloc.dealloc(p, l);
                let (reuse, layout) = checked(2048, 16);
                assert_eq!(reuse, p);
                blocks.push((reuse, layout));
            }
        }
        for (p, l) in blocks {
            unsafe {
                RelaxAlloc.dealloc(p, l);
            }
        }
        trim();
        assert_eq!(stats(), before);
    }

    #[test]
    fn exhausted_reservation_falls_back_and_counting_wrapper_works() {
        let _serial = lock(&TEST_LOCK);
        trim();
        let before = stats();
        let _scope = scratch(&crate::options::RuntimeOptions::default());
        // Requesting more than the reservation must fail without altering pool state.
        let huge = Layout::from_size_align(RESERVE + CHUNK, 16).unwrap();
        assert!(unsafe { alloc(huge) }.is_null());
        // Temporarily occupy the virtual free list without committing payload pages.
        let run = lock(&DEFAULT.pool).take(MAX_ORDER).unwrap();
        unsafe {
            let l = Layout::from_size_align(123, 16).unwrap();
            let p = RelaxAlloc.alloc(l);
            assert!(!p.is_null() && !owns(p));
            RelaxAlloc.dealloc(p, l);
        }
        lock(&DEFAULT.pool).release(run, MAX_ORDER);
        unsafe {
            use crate::memstat::Counting;
            for align in [16, 4096] {
                let l = Layout::from_size_align(123, align).unwrap();
                let p = Counting.alloc_zeroed(l);
                assert!(!p.is_null() && owns(p));
                assert!(std::slice::from_raw_parts(p, 123).iter().all(|&b| b == 0));
                std::ptr::write_bytes(p, 0x71, 123);
                let q = Counting.realloc(p, l, 262145);
                assert!(!q.is_null() && owns(q));
                assert!(std::slice::from_raw_parts(q, 123)
                    .iter()
                    .all(|&b| b == 0x71));
                Counting.dealloc(q, Layout::from_size_align(262145, align).unwrap());
            }
        }
        trim();
        assert_eq!(stats(), before);
    }
}
