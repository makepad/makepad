// Stress tests of the compiler arena's thread-owned fast paths (included in `routing_tests`):
// threads allocating and freeing their own and each other's blocks, chunk handoff when a chunk
// fills, threads ending with live blocks, and accounting that must return to where it started.

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// Mostly small sizes of every class, now and then a block of several hundred KB or MB.
    fn size(&mut self) -> usize {
        match self.below(1000) {
            0 => (256 << 10) + 1 + self.below(3 << 20),
            1..=20 => (1 << 12) + self.below(250 << 10),
            21..=300 => 1 + self.below(4096),
            _ => 1 + self.below(600),
        }
    }
}

/// A block with the pattern it must still hold when it is freed.
#[derive(Clone, Copy)]
struct Block {
    p: usize,
    size: usize,
    align: usize,
    tag: u8,
}
impl Block {
    unsafe fn make(size: usize, align: usize, tag: u8) -> Block {
        let (p, _) = put(size, align, tag);
        assert!(compiler::owns(p), "a compiler-phase block must be in the arena");
        assert_eq!(p as usize % align, 0);
        Block { p: p as usize, size, align, tag }
    }
    /// The pattern is checked at both ends and a few places inside: a block overlapping another
    /// one, or a slot handed out twice, shows as a changed byte.
    unsafe fn check(&self) {
        let p = self.p as *mut u8;
        let n = self.size;
        let probes = [0, n / 3, n / 2, (2 * n) / 3, n - 1];
        for &i in &probes {
            assert_eq!(*p.add(i), self.tag, "block {:#x} (size {}) damaged at {}", self.p, n, i);
        }
        if n <= 4096 {
            assert!(all_eq(p, n, self.tag), "block {:#x} (size {}) damaged", self.p, n);
        }
    }
    unsafe fn free(self) {
        self.check();
        RelaxAlloc.dealloc(self.p as *mut u8, Layout::from_size_align(self.size, self.align).unwrap());
    }
}

fn live_bytes() -> usize {
    compiler_stats().1
}

/// The state every test must leave behind: nothing live, no chunk this test committed still held.
fn assert_back_to(live: usize, chunks: usize) {
    compiler_trim();
    assert_eq!(live_bytes(), live, "live bytes after every block was freed");
    assert!(compiler_stats().2 <= chunks, "chunks {} > {chunks} after the trim", compiler_stats().2);
}

#[test]
fn threads_alloc_and_free_random_blocks_and_the_accounting_returns() {
    let _s = serial();
    compiler_trim();
    let (live0, chunks0) = (live_bytes(), compiler_stats().2);
    let workers: Vec<_> = (0..8u64)
        .map(|k| {
            std::thread::spawn(move || {
                let _g = compiler_thread();
                let mut rng = Rng(0x9e3779b97f4a7c15 ^ (k + 1).wrapping_mul(0x1234567));
                let mut live: Vec<Block> = Vec::new();
                for i in 0..120_000usize {
                    if live.len() < 400 && (live.is_empty() || rng.below(100) < 55) {
                        let size = rng.size();
                        let align = [8, 8, 8, 16, 64][rng.below(5)];
                        live.push(unsafe { Block::make(size, align, (rng.below(250) + 1) as u8) });
                    } else {
                        let b = live.swap_remove(rng.below(live.len()));
                        unsafe { b.free() };
                    }
                    if i % 20_000 == 0 {
                        for b in &live {
                            unsafe { b.check() };
                        }
                    }
                }
                for b in live {
                    unsafe { b.free() };
                }
            })
        })
        .collect();
    for w in workers {
        w.join().unwrap();
    }
    assert_back_to(live0, chunks0);
}

#[test]
fn blocks_freed_by_other_threads_are_reused_and_accounted() {
    let _s = serial();
    compiler_trim();
    let (live0, chunks0) = (live_bytes(), compiler_stats().2);
    const ROUNDS: usize = 60;
    // allocators allocate a batch per round and send it to a freer; the freers free other
    // threads' blocks; the allocators keep allocating into the slots those frees return
    let (txs, rxs): (Vec<_>, Vec<_>) = (0..4).map(|_| std::sync::mpsc::sync_channel::<Vec<Block>>(4)).unzip();
    let freers: Vec<_> = rxs
        .into_iter()
        .map(|rx| {
            std::thread::spawn(move || {
                let _g = compiler_thread();
                for batch in rx {
                    for b in batch {
                        unsafe { b.free() };
                    }
                }
            })
        })
        .collect();
    let allocators: Vec<_> = (0..4usize)
        .map(|k| {
            let txs = txs.clone();
            std::thread::spawn(move || {
                let _g = compiler_thread();
                let mut rng = Rng(0xabcdef12345 + k as u64 * 7919);
                for round in 0..ROUNDS {
                    let batch: Vec<Block> = (0..2500)
                        .map(|_| unsafe { Block::make(1 + rng.below(900), 8, (k * 50 + round % 50 + 1) as u8) })
                        .collect();
                    txs[(k + round) % 4].send(batch).unwrap();
                }
            })
        })
        .collect();
    drop(txs);
    for a in allocators {
        a.join().unwrap();
    }
    for f in freers {
        f.join().unwrap();
    }
    assert_back_to(live0, chunks0);
    // the committed memory stays bounded: 4 x 60 x 2500 x ~450 B = 270 MB allocated in all, but the
    // freed slots were taken back (drained from the remote lists / the partial list) and reused
    let peak = compiler_stats().2;
    assert!(peak <= chunks0 + 2, "chunks after: {peak}");
}

#[test]
fn a_full_chunk_is_handed_off_and_its_slots_come_back_without_overlap() {
    let _s = serial();
    compiler_trim();
    let (live0, chunks0) = (live_bytes(), compiler_stats().2);
    let _g = compiler();
    let mut rng = Rng(77);
    let mut blocks: Vec<Block> = Vec::new();
    // a few classes, several chunks each
    for round in 0..6 {
        for _ in 0..40_000 {
            let size = [24usize, 100, 300, 1000][rng.below(4)];
            blocks.push(unsafe { Block::make(size, 8, (round * 40 + 1) as u8) });
        }
        // free a random third: the chunks that are full now get listed, the current ones keep their slots
        let mut kept = Vec::new();
        for b in blocks.drain(..) {
            if rng.below(3) == 0 {
                unsafe { b.free() };
            } else {
                kept.push(b);
            }
        }
        blocks = kept;
    }
    // no two live blocks overlap
    let mut spans: Vec<(usize, usize)> = blocks.iter().map(|b| (b.p, b.p + b.size)).collect();
    spans.sort();
    for w in spans.windows(2) {
        assert!(w[0].1 <= w[1].0, "blocks overlap: {:x?} {:x?}", w[0], w[1]);
    }
    for b in &blocks {
        unsafe { b.check() };
    }
    for b in blocks {
        unsafe { b.free() };
    }
    assert_back_to(live0, chunks0);
}

#[test]
fn threads_that_end_with_live_blocks_hand_their_chunks_back() {
    let _s = serial();
    compiler_trim();
    let (live0, chunks0) = (live_bytes(), compiler_stats().2);
    let mut kept: Vec<Block> = Vec::new();
    for round in 0..120u64 {
        // no compiler_thread(): the thread's destructor hands its chunks back
        let blocks = std::thread::spawn(move || {
            let _g = compiler();
            let mut rng = Rng(round * 31 + 5);
            let mut mine = Vec::new();
            let mut ours = Vec::new();
            for _ in 0..300 {
                let b = unsafe { Block::make(1 + rng.below(3000), 8, (round % 200 + 1) as u8) };
                if rng.below(4) == 0 { ours.push(b) } else { mine.push(b) }
            }
            for b in ours {
                unsafe { b.free() };
            }
            mine
        })
        .join()
        .unwrap();
        kept.extend(blocks);
        // free some of the earlier threads' blocks from here, the rest later
        if round % 3 == 2 {
            let half = kept.len() / 2;
            for b in kept.drain(..half) {
                unsafe { b.free() };
            }
        }
    }
    // the dead threads' chunks carry live blocks only; nothing leaked beyond them
    for b in &kept {
        unsafe { b.check() };
    }
    // fresh allocations must not land on a live block
    let _g = compiler();
    let fresh: Vec<Block> = (0..20_000).map(|i| unsafe { Block::make(1 + i % 2000, 8, 0xEE) }).collect();
    for b in &kept {
        unsafe { b.check() };
    }
    for b in fresh {
        unsafe { b.free() };
    }
    for b in kept {
        unsafe { b.free() };
    }
    drop(_g);
    assert_back_to(live0, chunks0);
}

#[test]
fn frees_racing_with_the_owner_ending_lose_nothing() {
    let _s = serial();
    compiler_trim();
    let (live0, chunks0) = (live_bytes(), compiler_stats().2);
    for round in 0..150u64 {
        let (tx, rx) = std::sync::mpsc::channel::<Vec<Block>>();
        let rx = std::sync::Arc::new(std::sync::Mutex::new(rx));
        let freers: Vec<_> = (0..3)
            .map(|_| {
                let rx = rx.clone();
                std::thread::spawn(move || loop {
                    let batch = match rx.lock().unwrap().recv() {
                        Ok(b) => b,
                        Err(_) => return,
                    };
                    for b in batch {
                        unsafe { b.free() };
                    }
                })
            })
            .collect();
        let owner = std::thread::spawn(move || {
            let _g = compiler();
            let mut rng = Rng(round + 99);
            for _ in 0..8 {
                let batch: Vec<Block> = (0..300).map(|_| unsafe { Block::make(1 + rng.below(700), 8, 0x5a) }).collect();
                tx.send(batch).unwrap();
            }
            // ends at once: its chunks are handed back while the freers are still freeing into them
        });
        owner.join().unwrap();
        for f in freers {
            f.join().unwrap();
        }
    }
    // everything freed: allocate again over the same chunks, nothing may overlap or be lost
    let _g = compiler();
    let fresh: Vec<Block> = (0..30_000).map(|i| unsafe { Block::make(1 + i % 700, 8, 0xA7) }).collect();
    let mut spans: Vec<(usize, usize)> = fresh.iter().map(|b| (b.p, b.p + b.size)).collect();
    spans.sort();
    for w in spans.windows(2) {
        assert!(w[0].1 <= w[1].0, "blocks overlap: {:x?} {:x?}", w[0], w[1]);
    }
    for b in fresh {
        unsafe { b.free() };
    }
    drop(_g);
    assert_back_to(live0, chunks0);
}

thread_local! {
    static LATE: Late = const { Late };
}
/// Allocates in the compiler phase from its destructor, which runs after the allocator's own
/// thread-exit hook (registered later, so destroyed earlier... this one is touched first and so
/// runs last): the thread has handed its chunks back and takes no new ones.
struct Late;
impl Drop for Late {
    fn drop(&mut self) {
        let _g = compiler();
        let mut v = Vec::new();
        for i in 0..2000usize {
            v.push(unsafe { Block::make(16 + i % 700, 16, 0x3c) });
        }
        for b in v.drain(..1000) {
            unsafe { b.free() };
        }
        // handed to the main thread through a static: freed by it
        LEFT.lock().unwrap().extend(v.into_iter().map(|b| (b.p, b.size, b.align, b.tag)));
    }
}
static LEFT: relax_session::sync::Mutex<Vec<(usize, usize, usize, u8)>> = relax_session::sync::Mutex::new(Vec::new());

#[test]
fn a_thread_allocating_after_its_exit_hook_still_works() {
    let _s = serial();
    compiler_trim();
    let (live0, chunks0) = (live_bytes(), compiler_stats().2);
    for _ in 0..20 {
        std::thread::spawn(|| {
            LATE.with(|_| {});
            let _g = compiler();
            let b = unsafe { Block::make(100, 8, 1) }; // arms the exit hook
            unsafe { b.free() };
        })
        .join()
        .unwrap();
    }
    let left: Vec<_> = std::mem::take(&mut *LEFT.lock().unwrap());
    assert_eq!(left.len(), 20 * 1000);
    for (p, size, align, tag) in left {
        unsafe { Block { p, size, align, tag }.free() };
    }
    assert_back_to(live0, chunks0);
}

#[test]
fn old_generation_chunks_held_by_threads_are_left_alone() {
    let _s = serial();
    compiler_trim();
    let (live0, chunks0) = (live_bytes(), compiler_stats().2);
    let (tx, rx) = std::sync::mpsc::channel::<()>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let worker = std::thread::spawn(move || {
        let _g = compiler_thread();
        let first: Vec<Block> = (0..500).map(|i| unsafe { Block::make(8 + i % 300, 8, 0x11) }).collect();
        tx.send(()).unwrap();
        go_rx.recv().unwrap();
        // the generation moved on while this thread held chunks of the old one: it takes new chunks
        let second: Vec<Block> = (0..500).map(|i| unsafe { Block::make(8 + i % 300, 8, 0x22) }).collect();
        for b in &first {
            assert_eq!(compiler_epoch_of(b.p as *const u8), Some(compiler_generation() - 1));
        }
        for b in &second {
            assert_eq!(compiler_epoch_of(b.p as *const u8), Some(compiler_generation()));
        }
        for b in first.into_iter().chain(second) {
            unsafe { b.free() };
        }
    });
    rx.recv().unwrap();
    compiler_begin_generation();
    go_tx.send(()).unwrap();
    worker.join().unwrap();
    compiler_destroy_older();
    assert_back_to(live0, chunks0);
}

#[test]
fn an_emptied_chunk_is_released_when_it_was_carved_far_and_kept_when_it_was_not() {
    let _s = serial();
    compiler_trim();
    let (live0, chunks0) = (live_bytes(), compiler_stats().2);
    let committed0 = compiler_stats().0;
    let _g = compiler();
    // 100-byte blocks sit in 112-byte slots: 6,000 of them carve 672 KB of a chunk
    let big: Vec<Block> = (0..6_000).map(|i| unsafe { Block::make(100, 8, (i % 200 + 1) as u8) }).collect();
    assert!(compiler_stats().0 >= committed0 + compiler::CHUNK);
    for b in big {
        unsafe { b.free() };
    }
    assert_eq!(compiler_stats().0, committed0, "the emptied, far-carved chunk went back without a trim");
    // a few blocks of another class: the chunk stays the thread's after they are freed
    let small: Vec<Block> = (0..50).map(|i| unsafe { Block::make(200, 8, (i % 200 + 1) as u8) }).collect();
    let held = compiler_stats().0;
    assert!(held > committed0);
    for b in small {
        unsafe { b.free() };
    }
    assert_eq!(compiler_stats().0, held, "a lightly carved chunk is kept for the next block");
    drop(_g);
    assert_back_to(live0, chunks0);
}
