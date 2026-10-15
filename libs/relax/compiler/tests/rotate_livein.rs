// Loop rotation copies a loop header into its latches. A header whose only use of a vreg is
// its own decrement (`left -= 1; if left == 0 { return }`, miniz's find_match probe count)
// must keep updating that vreg in the copy (it carries the count around the loop); the copy
// once renamed it to a fresh, never-set vreg, so the count restarted from garbage
// (deflate output differed from rustc). rustc: probes 7 hits 1 / probes 1 hits 0.
#[inline(never)]
fn probe(chain: &[u32], start: usize, probes: u32) -> (u32, u32) {
    let mut left = probes;
    let mut pos = start;
    let mut hits = 0u32;
    let mut used = 0u32;
    loop {
        left -= 1;
        if left == 0 {
            return (used, hits);
        }
        used += 1;
        pos = chain[pos] as usize;
        if pos % 3 == 0 {
            hits += 1;
        }
    }
}

#[test]
fn rotate_header_livein() {
    let chain: Vec<u32> = (0..64u32).map(|i| (i * 7 + 3) % 64).collect();
    let a = probe(std::hint::black_box(&chain), 1, 8);
    let b = probe(std::hint::black_box(&chain), 5, 2);
    println!("probes {} hits {} / probes {} hits {}", a.0, a.1, b.0, b.1);
    assert_eq!(a.0, 7);
    assert_eq!(b, (1, 0));
}
