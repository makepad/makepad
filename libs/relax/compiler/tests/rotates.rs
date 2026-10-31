// Rotates the optimiser forms from `x << k | x >> (w - k)` (opt.rs combine_rotates -> IOp::Rotl; x64 rol, arm64 ror):
// rotate_left / rotate_right on u64, u32, i64, i32, u16, u8 (only unsigned 32/64-bit become one instruction), written
// shift-or pairs in both operand orders, a pair whose counts do not add up to the width (must stay two shifts), and
// FxHasher's word step; called often enough for O3. Output = rustc --test.
#[inline(never)]
fn rots(x: u64, y: u32, z: i64, w: i32, s: u16, b: u8) -> u64 {
    let a = x.rotate_left(5) ^ x.rotate_right(13) ^ x.rotate_left(63);
    let c = (y.rotate_left(7) ^ y.rotate_right(1) ^ y.rotate_left(31)) as u64;
    let e = (z.rotate_left(17) ^ z.rotate_right(9)) as u64;
    let f = (w.rotate_left(3) ^ w.rotate_right(30)) as u32 as u64;
    let g = (s.rotate_left(4) as u64) << 8 | b.rotate_right(3) as u64;
    a.wrapping_add(c).wrapping_add(e) ^ f ^ g
}

#[inline(never)]
fn pairs(x: u64, y: u32) -> u64 {
    let p = (x << 11) | (x >> 53);
    let q = (x >> 20) | (x << 44);
    let r = (y << 9) | (y >> 23);
    let s = (y >> 9) | (y << 23);
    // counts adding to 60: not a rotate
    let t = (x << 10) | (x >> 50);
    p ^ q.rotate_left(1) ^ r as u64 ^ (s as u64) << 7 ^ t
}

#[inline(never)]
fn fx(words: &[u64]) -> u64 {
    let mut h = 0u64;
    for w in words {
        h = (h.rotate_left(5) ^ w).wrapping_mul(0x517cc1b727220a95);
    }
    h
}

#[test]
fn rotates() {
    let words: Vec<u64> = (0..64u64).map(|i| i.wrapping_mul(0x9E37_79B9_7F4A_7C15)).collect();
    let mut h = 0u64;
    let mut x = 0x0123_4567_89ab_cdefu64;
    for i in 0..200_000u64 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        h = h.wrapping_add(rots(x, x as u32, x as i64, (x >> 32) as i32, x as u16, (x >> 8) as u8));
        h ^= pairs(x, (x >> 3) as u32);
        if i % 64 == 0 {
            h = h.rotate_left(1) ^ fx(&words);
        }
    }
    println!("| {:016x}", h);
}
