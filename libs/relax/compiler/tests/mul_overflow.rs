// u64/i64/usize/isize overflowing/checked/saturating_mul through intrinsics_int::mul_hi
// (umulh/smulh, mul/imul rdx). rustc prints the same hash: 10587128077786417075

#[test]
fn mul_overflow() {
    let us: [u64; 8] = [0, 1, 2, 3, 0xffff_ffff, 0x1_0000_0000, u64::MAX / 3, u64::MAX];
    let is: [i64; 10] = [0, 1, -1, 2, -2, 0xffff_ffff, -0x1_0000_0000, i64::MAX, i64::MIN, i64::MIN / 3];
    let mut h = 0u64;
    for &a in us.iter() {
        for &b in us.iter() {
            let (r, o) = std::hint::black_box(a).overflowing_mul(b);
            h = h.rotate_left(3) ^ r ^ (o as u64);
            h ^= a.checked_mul(b).unwrap_or(7);
            h ^= (a as usize).saturating_mul(b as usize) as u64;
        }
    }
    for &a in is.iter() {
        for &b in is.iter() {
            let (r, o) = std::hint::black_box(a).overflowing_mul(b);
            h = h.rotate_left(3) ^ (r as u64) ^ (o as u64);
            h ^= a.checked_mul(b).unwrap_or(7) as u64;
            h ^= (a as isize).saturating_mul(b as isize) as u64;
        }
    }
    println!("mulhi hash {}", h);
}
