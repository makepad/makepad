// Loops with a small constant trip count, fully unrolled at O3 (codegen loops.rs unroll_const_loops): a probe
// loop `for _ in 0..3` with an early return (deflate's hash-chain shape), a counted loop stepping by 2 with
// `<=`, a 4-trip loop whose counter feeds an index, a 5-trip loop (left as a loop), a loop whose counter is read
// after it (left alone), and a do-while that runs once. Called often enough for O3; output = rustc --test.
#[inline(never)]
fn probe(next: &[u16; 64], mut p: usize, want: u16) -> (u32, usize) {
    let mut hits = 0u32;
    for _ in 0..3 {
        let n = next[p & 63] as usize;
        if n == 0 {
            return (hits + 100, p);
        }
        p = n;
        if n as u16 == want {
            hits += 1;
        }
    }
    (hits, p)
}

#[inline(never)]
fn step2(a: &[u32; 16], o: usize) -> u32 {
    let mut s = 0u32;
    let mut i = 0usize;
    while i <= 6 {
        s = s.wrapping_mul(31).wrapping_add(a[(o + i) & 15]);
        i += 2;
    }
    s
}

#[inline(never)]
fn four(a: &[u64; 8], k: u64) -> u64 {
    let mut s = 0;
    for j in 0..4 {
        s ^= a[j * 2].rotate_left(j as u32) ^ k;
    }
    s
}

#[inline(never)]
fn five(a: &[u8; 8]) -> u32 {
    let mut s = 0u32;
    for j in 0..5u32 {
        s = s * 7 + a[j as usize] as u32 + j;
    }
    s
}

#[inline(never)]
fn counter_after(a: &[i32; 8]) -> i32 {
    let mut j = 0;
    while j < 3 {
        if a[j] < 0 {
            break;
        }
        j += 1;
    }
    j as i32 * 1000 + a[j.min(7)]
}

#[inline(never)]
fn once(x: u64) -> u64 {
    let mut i = 0u32;
    let mut v = x;
    loop {
        v = v.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ i as u64;
        i += 1;
        if i >= 1 {
            break;
        }
    }
    v
}

#[test]
fn const_trip_loops() {
    let mut next = [0u16; 64];
    for i in 0..64 {
        next[i] = ((i * 37 + 11) % 64) as u16 * (i % 9 != 0) as u16;
    }
    let a32: [u32; 16] = core::array::from_fn(|i| (i as u32).wrapping_mul(2654435761));
    let a64: [u64; 8] = core::array::from_fn(|i| (i as u64 + 1).wrapping_mul(0x0123_4567_89ab_cdef));
    let a8: [u8; 8] = [3, 1, 4, 1, 5, 9, 2, 6];
    let ai: [i32; 8] = [5, 7, -1, 4, 2, -8, 9, 1];
    let mut h = 0u64;
    for r in 0..300_000u64 {
        let (x, p) = probe(&next, r as usize, (r % 64) as u16);
        h = h.rotate_left(3) ^ x as u64 ^ p as u64;
        h = h.wrapping_add(step2(&a32, r as usize) as u64);
        h ^= four(&a64, r);
        h = h.wrapping_add(five(&a8) as u64);
        let mut b = ai;
        b[(r % 8) as usize] = -(r as i32 % 5);
        h = h.wrapping_add(counter_after(&b) as u64);
        h ^= once(r);
    }
    println!("| {:016x}", h);
}
