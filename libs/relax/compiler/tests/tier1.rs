// Tier-up (run with MAKEPAD_RUN_O3=1 MAKEPAD_RUN_O3_CALLS=3): `run_from` reaches the O3 call threshold, is
// rebuilt on the O3 thread, and its slot switches to the O3 code while the test keeps
// running. O1 calls `mix` from the loop; O3 inlines it.
// `mix` is above O1's source-size inlining gate (100 tokens, see its dead locals),
// so O1 calls it, and O3 inlines it (its RIR exists by then, and its RIR is a
// small leaf). The test prints ps per call for each batch: the later batches run O3.
// The checksum must not change across tiers.

/// Mixes a word.
fn mix(x: u64, k: u64) -> u64 {
    // dead locals: they keep the source above O1's inlining gate (100 tokens; the
    // O1 inliner builds RIR only for callees that small before optimising the caller)
    // and are gone after optimisation, so the RIR is a small leaf O3 inlines
    let _d0 = 0u64; let _d1 = 1u64; let _d2 = 2u64; let _d3 = 3u64; let _d4 = 4u64;
    let _d5 = 5u64; let _d6 = 6u64; let _d7 = 7u64; let _d8 = 8u64; let _d9 = 9u64;
    let _e0 = 0u64; let _e1 = 1u64; let _e2 = 2u64; let _e3 = 3u64; let _e4 = 4u64;
    let _e5 = 5u64; let _e6 = 6u64; let _e7 = 7u64; let _e8 = 8u64; let _e9 = 9u64;
    let y = (x ^ k).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    y ^ (y >> 29)
}


fn run(n: u64, seed: u64) -> u64 {
    let mut acc = seed;
    let mut i = 0;
    while i < n {
        acc = mix(acc, i).rotate_left(7);
        i += 1;
    }
    acc
}

#[test]
fn tier_up_keeps_results() {
    let reference = {
        // the same computation inline (no calls)
        let mut acc = 1u64;
        let mut i = 0u64;
        while i < 2_000_000 {
            let y = (acc ^ i).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            acc = (y ^ (y >> 29)).rotate_left(7);
            i += 1;
        }
        acc
    };
    let mut batches = Vec::new();
    let mut acc = 1u64;
    let mut done = 0u64;
    let t = std::time::Instant::now();
    for _ in 0..40 {
        let b = std::time::Instant::now();
        acc = run_from(acc, done, 50_000);
        done += 50_000;
        batches.push(b.elapsed().as_nanos() as u64 / 50);
        // give the O3 thread a moment after the threshold is crossed
        if batches.len() == 3 {
            std::thread::sleep(std::time::Duration::from_millis(30));
        }
    }
    let _ = t;
    println!("ps/call per batch: {:?}", batches);
    assert_eq!(acc, reference);
}

fn run_from(acc: u64, from: u64, n: u64) -> u64 {
    // not inlined into the test at O1: it calls `mix`, so it is no leaf
    let mut acc = acc;
    let mut i = from;
    while i < from + n {
        acc = mix(acc, i).rotate_left(7);
        i += 1;
    }
    let _ = run;
    acc
}
