// Integer sum reductions that O3 vectorises (codegen loops.rs vectorize_reductions): u32 and
// u64 elements summed into u64 (wrapping), with and without a loop-invariant factor, every length
// 0..=40 at several start offsets (unaligned vector loads, remainders of every size), index and
// iterator loops, `<` and `!=` exits; called often enough to tier up (= rustc --test).
#[inline(never)]
fn sum_u32(a: &[u32], lo: usize, hi: usize) -> u64 {
    let mut s = 0u64;
    for i in lo..hi {
        s = s.wrapping_add(a[i] as u64);
    }
    s
}
#[inline(never)]
fn sum_u32_iter(a: &[u32]) -> u64 {
    let mut s = 0u64;
    for x in a {
        s = s.wrapping_add(*x as u64);
    }
    s
}
#[inline(never)]
fn sum_u64_scaled(a: &[u64], k: u64) -> u64 {
    let mut s = 5u64;
    for i in 0..a.len() {
        s = s.wrapping_add(a[i].wrapping_mul(k));
    }
    s
}
#[inline(never)]
fn sum_u32_scaled(a: &[u32], k: u64) -> u64 {
    let mut acc = 0u64;
    for i in 0..a.len() {
        acc = acc.wrapping_add((a[i] as u64).wrapping_mul(k));
    }
    acc
}

#[test]
fn vectorized_sums() {
    let a: Vec<u32> = (0..48u32).map(|x| x.wrapping_mul(2654435761u32) | 0x8000_0000).collect();
    let b: Vec<u64> = (0..48u64).map(|x| x.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();
    let mut h = 0u64;
    for rep in 0..60000u64 {
        for n in 0..41usize {
            for lo in 0..3usize {
                let hi = (lo + n).min(48);
                h = h.rotate_left(1) ^ sum_u32(&a, lo, hi);
                h = h.rotate_left(1) ^ sum_u32_iter(&a[lo..hi]);
                h = h.rotate_left(1) ^ sum_u64_scaled(&b[lo..hi], rep | 0xFFFF_FFFF_0000_0001);
                h = h.rotate_left(1) ^ sum_u32_scaled(&a[lo..hi], rep.wrapping_mul(0x1_0000_0003));
            }
        }
        if rep % 20000 == 0 {
            println!("| {} {:016x}", rep, h);
        }
    }
    println!("| {:016x}", h);
}
