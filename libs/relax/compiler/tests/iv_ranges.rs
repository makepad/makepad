// Loop-variable intervals (codegen ranges: counting-up loop variables hold [entry minimum, natural
// maximum] at their header when every back edge keeps them there) and signed division /
// remainder by 2^k of a non-negative dividend as a shift / mask. Cases that must stay signed: a
// loop variable starting negative, one that wraps past i32::MAX, one reset inside the loop, a
// step that may overflow. Called often enough for O3. Output = rustc --test.
#[inline(never)]
fn rem_up(n: i32) -> u64 {
    let mut h = 0u64;
    for i in 0..n {
        h = h.wrapping_mul(31).wrapping_add((i % 1024) as u64 ^ ((i / 8) as u64) << 3);
    }
    h
}

#[inline(never)]
fn rem_from_neg(n: i32) -> u64 {
    let mut h = 0u64;
    for i in -700..n {
        h = h.wrapping_mul(31).wrapping_add((i % 64) as i64 as u64 ^ ((i / 16) as i64 as u64));
    }
    h
}

#[inline(never)]
fn rem_wrap(start: i32) -> u64 {
    let mut h = 0u64;
    let mut i = start;
    let mut k = 0;
    while k < 40 {
        h = h.wrapping_mul(31).wrapping_add((i % 8) as i64 as u64 ^ ((i / 4) as i64 as u64));
        i = i.wrapping_add(3);
        k += 1;
    }
    h
}

#[inline(never)]
fn rem_reset(n: i32) -> u64 {
    let mut h = 0u64;
    let mut i = 5i32;
    let mut k = 0;
    while k < n {
        h = h.wrapping_mul(31).wrapping_add((i % 32) as i64 as u64);
        i += 7;
        if k % 13 == 12 {
            i = -i;
        }
        k += 1;
    }
    h
}

#[inline(never)]
fn idx_sum(a: &[u32; 256], n: usize) -> u64 {
    let mut s = 0u64;
    let mut i = 0usize;
    while i < n {
        s += a[i & 255] as u64 + a[(i / 4) % 256] as u64;
        i += 1;
    }
    s
}

#[test]
fn iv_ranges() {
    let mut a = [0u32; 256];
    for (j, x) in a.iter_mut().enumerate() {
        *x = (j as u32).wrapping_mul(2654435761);
    }
    let mut h = 0u64;
    for r in 0..300i32 {
        h = h.wrapping_mul(131).wrapping_add(rem_up(2000 + r));
        h = h.wrapping_mul(131).wrapping_add(rem_from_neg(300 + r));
        h = h.wrapping_mul(131).wrapping_add(rem_wrap(i32::MAX - 60 + r));
        h = h.wrapping_mul(131).wrapping_add(rem_reset(100 + r));
        h = h.wrapping_mul(131).wrapping_add(idx_sum(&a, 1000 + r as usize));
    }
    println!("| {:016x}", h);
}
