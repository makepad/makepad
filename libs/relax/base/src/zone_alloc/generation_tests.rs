// Arena generations, destroy and tombstones (included into routing_tests; they share its SERIAL
// lock, so no other test allocates in the generations these create and destroy).

fn chunk_of(p: *mut u8) -> usize {
    (p as usize) & !(compiler::CHUNK - 1)
}

unsafe fn put_in_compiler(size: usize, align: usize, byte: u8) -> (*mut u8, Layout) {
    let _g = compiler();
    put(size, align, byte)
}

#[test]
fn a_new_generation_allocates_in_fresh_chunks_and_old_frees_still_work() {
    let _s = serial();
    unsafe {
        let g0 = compiler_begin_generation();
        assert_eq!(compiler_generation(), g0);
        let old: Vec<_> = (0..50).map(|_| put_in_compiler(64, 8, 1)).collect();
        let old_chunk = chunk_of(old[0].0);
        assert!(old.iter().all(|(p, _)| chunk_of(*p) == old_chunk));
        assert_eq!(compiler_epoch_of(old[0].0), Some(g0));
        let g1 = compiler_begin_generation();
        assert_eq!(g1, g0 + 1);
        let new: Vec<_> = (0..50).map(|_| put_in_compiler(64, 8, 2)).collect();
        // the same thread's cached chunk and the partial list are of the old generation: skipped
        assert!(new.iter().all(|(p, _)| chunk_of(*p) != old_chunk));
        assert_eq!(compiler_epoch_of(new[0].0), Some(g1));
        // frees of the old blocks (some, then all but one) work while the generation lives
        let mut old = old;
        for (p, l) in old.drain(..40) {
            assert!(all_eq(p, 64, 1));
            RelaxAlloc.dealloc(p, l);
        }
        // freed old slots are not reused by the new generation
        let more: Vec<_> = (0..20).map(|_| put_in_compiler(64, 8, 3)).collect();
        assert!(more.iter().all(|(p, _)| chunk_of(*p) != old_chunk));
        for (p, l) in old {
            RelaxAlloc.dealloc(p, l);
        }
        for (p, l) in new.into_iter().chain(more) {
            RelaxAlloc.dealloc(p, l);
        }
        compiler_destroy_generation(g0);
    }
}

#[test]
fn destroy_returns_the_touched_bytes_and_a_free_afterwards_is_a_counted_noop() {
    let _s = serial();
    unsafe {
        compiler_trim();
        let g = compiler_begin_generation();
        let before = compiler_stats();
        let blocks: Vec<_> = (0..4000).map(|i| put_in_compiler(100 + i % 7, 8, 9)).collect();
        let large = put_in_compiler(3 << 20, 16, 7);
        let kept_block = {
            let _k = kept();
            put(100, 8, 4)
        };
        let mid = compiler_stats();
        assert!(mid.0 > before.0);
        let live = blocks[0].0;
        let g_next = compiler_begin_generation();
        let survivor = put_in_compiler(100, 8, 5); // g_next: not touched by the destroy
        let frees = compiler_tombstone_frees();
        let (chunks, touched) = compiler_destroy_generation(g);
        assert!(chunks >= 2 && touched >= (3 << 20), "chunks {chunks} touched {touched}");
        // the generation's committed share is gone; the survivor's chunk stays
        let after = compiler_stats();
        assert!(after.0 <= before.0 + compiler::CHUNK, "committed {} -> {}", before.0, after.0);
        assert_eq!(compiler_tombstone_of(live as usize), Some(g));
        assert_eq!(compiler_tombstone_of(large.0 as usize), Some(g));
        assert_eq!(compiler_epoch_of(live), None);
        assert_eq!(compiler_tombstone_of(survivor.0 as usize), None);
        // frees of destroyed blocks: counted no-ops (small and large)
        RelaxAlloc.dealloc(live, blocks[0].1);
        RelaxAlloc.dealloc(large.0, large.1);
        assert_eq!(compiler_tombstone_frees(), frees + 2);
        // realloc of a destroyed block: counted, and a fresh block
        let r = RelaxAlloc.realloc(blocks[1].0, blocks[1].1, 500);
        assert!(!r.is_null() && compiler_tombstone_of(r as usize).is_none());
        assert_eq!(compiler_tombstone_frees(), frees + 3);
        RelaxAlloc.dealloc(r, Layout::from_size_align(500, 8).unwrap());
        // the current generation cannot be destroyed
        assert_eq!(compiler_destroy_generation(g_next), (0, 0));
        assert!(all_eq(survivor.0, 100, 5));
        assert!(compiler_tombstones() >= chunks, "tombstones {} chunks {chunks}", compiler_tombstones());
        RelaxAlloc.dealloc(survivor.0, survivor.1);
        RelaxAlloc.dealloc(kept_block.0, kept_block.1);
        compiler_begin_generation();
        compiler_destroy_generation(g_next);
    }
}

#[test]
fn destroy_older_reclaims_all_prior_generations_and_preserves_the_current_one() {
    let _s = serial();
    unsafe {
        let g0 = compiler_begin_generation();
        let old0 = put_in_compiler(100, 8, 1);
        let g1 = compiler_begin_generation();
        let old1 = put_in_compiler(3 << 20, 16, 2);
        let g2 = compiler_begin_generation();
        let current = put_in_compiler(100, 8, 3);
        let (chunks, touched) = compiler_destroy_older();
        assert!(chunks >= 2 && touched >= (3 << 20), "chunks {chunks} touched {touched}");
        assert_eq!(compiler_tombstone_of(old0.0 as usize), Some(g0));
        assert_eq!(compiler_tombstone_of(old1.0 as usize), Some(g1));
        assert_eq!(compiler_epoch_of(old0.0), None);
        assert_eq!(compiler_epoch_of(old1.0), None);
        assert_eq!(compiler_epoch_of(current.0), Some(g2));
        assert_eq!(compiler_tombstone_of(current.0 as usize), None);
        assert!(all_eq(current.0, 100, 3));
        assert_eq!(compiler_destroy_older(), (0, 0));
        RelaxAlloc.dealloc(current.0, current.1);
        // the thread keeps its current chunk (empty now) until it hands it back
        compiler_trim();
        // Empty generations also advance the sweep without disturbing tombstones.
        compiler_begin_generation();
        assert_eq!(compiler_destroy_older(), (0, 0));
        compiler_begin_generation();
        assert_eq!(compiler_destroy_older(), (0, 0));
    }
}

#[test]
fn a_realloc_of_an_old_block_moves_it_into_the_current_generation() {
    let _s = serial();
    unsafe {
        let g0 = compiler_begin_generation();
        let (p, l) = put_in_compiler(40, 8, 7);
        assert_eq!(compiler_epoch_of(p), Some(g0));
        let g1 = compiler_begin_generation();
        let q = {
            let _g = compiler();
            // 40 -> 47 shares a size class, which would stay in place in the same generation
            RelaxAlloc.realloc(p, l, 47)
        };
        assert_ne!(q, p);
        assert_eq!(compiler_epoch_of(q), Some(g1));
        assert!(all_eq(q, 40, 7));
        // in the kept phase it leaves the arena
        let (p2, l2) = put_in_compiler(40, 8, 8);
        let g2 = compiler_begin_generation();
        let k = RelaxAlloc.realloc(p2, l2, 5000);
        assert!(!compiler::owns(k));
        assert!(all_eq(k, 40, 8));
        RelaxAlloc.dealloc(k, Layout::from_size_align(5000, 8).unwrap());
        RelaxAlloc.dealloc(q, Layout::from_size_align(47, 8).unwrap());
        compiler_begin_generation();
        compiler_destroy_generation(g0);
        compiler_destroy_generation(g1);
        compiler_destroy_generation(g2);
    }
}

#[test]
fn chunks_of_a_new_generation_are_fresh_and_read_zero() {
    let _s = serial();
    unsafe {
        let g = compiler_begin_generation();
        // dirty a large run and many small chunks, destroy them, then allocate zeroed blocks
        let dirty: Vec<_> = (0..6).map(|_| put_in_compiler(3 << 20, 16, 0xa5)).collect();
        let small: Vec<_> = (0..20_000).map(|_| put_in_compiler(200, 8, 0xa5)).collect();
        let tomb_ranges: Vec<usize> = dirty.iter().map(|d| d.0 as usize).collect();
        let g2 = compiler_begin_generation();
        compiler_destroy_generation(g);
        let _c = compiler();
        let mut fresh = Vec::new();
        for _ in 0..12 {
            let l = Layout::from_size_align(3 << 20, 16).unwrap();
            let p = RelaxAlloc.alloc_zeroed(l);
            assert!(!p.is_null() && compiler_tombstone_of(p as usize).is_none());
            assert!(!tomb_ranges.contains(&(p as usize)));
            assert!(all_eq(p, 3 << 20, 0));
            fresh.push((p, l));
        }
        let smalls: Vec<_> = (0..20_000).map(|_| put(200, 8, 1)).collect();
        assert!(smalls.iter().all(|(p, _)| compiler_tombstone_of(*p as usize).is_none()));
        assert!(smalls.iter().all(|(p, _)| compiler_epoch_of(*p) == Some(g2)));
        drop(_c);
        for (p, l) in fresh.into_iter().chain(smalls) {
            RelaxAlloc.dealloc(p, l);
        }
        // leftover handles to destroyed blocks are plain no-ops
        for (p, l) in dirty.into_iter().chain(small) {
            RelaxAlloc.dealloc(p, l);
        }
        compiler_begin_generation();
        compiler_destroy_generation(g2);
    }
}

#[test]
fn threads_allocating_across_a_begin_take_chunks_of_the_new_generation() {
    let _s = serial();
    unsafe {
        let g0 = compiler_begin_generation();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let start = std::sync::Arc::new(std::sync::Barrier::new(5));
        let workers: Vec<_> = (0..4u8)
            .map(|k| {
                let (stop, start) = (stop.clone(), start.clone());
                std::thread::spawn(move || {
                    let _g = compiler_thread();
                    start.wait();
                    let mut held: Vec<(usize, Layout, u64)> = Vec::new();
                    let mut i = 0usize;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        let observed = compiler_generation();
                        let size = 16usize << ((i + k as usize) % 9);
                        let (p, l) = put(size, 8, k + 1);
                        // a block allocated after the thread saw generation `observed` is of it or later
                        let e = compiler_epoch_of(p).unwrap();
                        assert!(e >= observed, "block of generation {e} after seeing {observed}");
                        held.push((p as usize, l, e));
                        if held.len() > 200 {
                            for (p, l, _) in held.drain(..100) {
                                assert!(all_eq(p as *mut u8, l.size(), k + 1));
                                RelaxAlloc.dealloc(p as *mut u8, l);
                            }
                        }
                        i += 1;
                    }
                    let late = held.iter().filter(|h| h.2 > 0).count();
                    for (p, l, _) in held {
                        RelaxAlloc.dealloc(p as *mut u8, l);
                    }
                    late
                })
            })
            .collect();
        start.wait();
        std::thread::sleep(std::time::Duration::from_millis(60));
        let g1 = compiler_begin_generation();
        std::thread::sleep(std::time::Duration::from_millis(60));
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        for w in workers {
            w.join().unwrap();
        }
        assert_eq!(g1, g0 + 1);
        // everything was freed: the old generation destroys without touching anything live
        compiler_begin_generation();
        compiler_destroy_generation(g0);
        compiler_destroy_generation(g1);
    }
}
