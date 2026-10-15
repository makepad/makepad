// Small counted loops that O3 unrolls (codegen loops.rs): every trip count 0..=19, both loop
// shapes (guarded `for` and an unguarded do-while), `<` and `!=` exits, signed and unsigned
// counters, called often enough to tier up (= rustc --test).
#[inline(never)]
fn sum_lt(a: &[u32], n: usize) -> u64 {
    let mut s = 0u64;
    for i in 0..n {
        s = s.wrapping_mul(3).wrapping_add(a[i] as u64);
    }
    s
}
#[inline(never)]
fn signed_range(lo: i32, hi: i32) -> i64 {
    let mut s = 0i64;
    let mut i = lo;
    while i < hi {
        s = s * 5 + i as i64;
        i += 1;
    }
    s
}
#[inline(never)]
fn do_while(n: u64) -> u64 {
    // runs once even when n == 0
    let mut i = 0u64;
    let mut s = 7u64;
    loop {
        s = s.wrapping_mul(31) ^ i;
        i += 1;
        if i >= n {
            break;
        }
    }
    s
}
#[inline(never)]
fn ne_loop(start: u64, end: u64) -> u64 {
    let mut i = start;
    let mut s = 1u64;
    if i != end {
        loop {
            s = s.rotate_left(3) ^ i;
            i += 1;
            if i == end {
                break;
            }
        }
    }
    s
}

#[test]
fn unroll_trip_counts() {
    let a: Vec<u32> = (0..32u32).map(|x| x * 2654435761u32).collect();
    let mut h = 0u64;
    for rep in 0..400000u64 {
        for n in 0..20usize {
            h = h.rotate_left(1) ^ sum_lt(&a, n);
            h = h.rotate_left(1) ^ signed_range(n as i32 - 7, 9) as u64;
            h = h.rotate_left(1) ^ do_while(n as u64);
            h = h.rotate_left(1) ^ ne_loop(rep % 3, rep % 3 + n as u64);
        }
        if rep % 100000 == 99999 {
            println!("{} {:x}", rep, h);
        }
    }
}
