// Constant-size memmove (`copy_within`, `ptr::copy`) inlines as loads then stores: overlapping
// moves in both directions, every size up to 64 plus 0, must match libc memmove.
#[inline(never)]
fn mv<const N: usize>(buf: &mut [u8], src: usize, dst: usize) {
    buf.copy_within(src..src + N, dst);
}

fn fill(buf: &mut [u8], seed: u32) {
    let mut x = seed.wrapping_mul(2654435761) | 1;
    for b in buf.iter_mut() {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *b = x as u8;
    }
}

fn hash(buf: &[u8]) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for &b in buf {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    h
}

fn run<const N: usize>(total: &mut u64) {
    let mut buf = [0u8; 160];
    for (src, dst) in [(16usize, 16usize + 1), (17, 16), (16, 16 + N / 2), (16 + N / 2, 16), (0, 80), (80, 3), (40, 40)] {
        fill(&mut buf, (N * 7 + src * 3 + dst) as u32);
        mv::<N>(&mut buf, src, dst);
        unsafe { std::ptr::copy(buf.as_ptr().add(dst), buf.as_mut_ptr().add(src), N) };
        *total = total.wrapping_mul(31).wrapping_add(hash(&buf));
    }
}

macro_rules! sizes {
    ($t:ident; $($n:literal)*) => { $( run::<$n>(&mut $t); )* };
}

#[test]
fn small_memmove_matches_libc() {
    let mut total = 0u64;
    for _ in 0..200 {
        sizes!(total; 0 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 23 24 31 32 33 40 47 48 55 56 63 64 65);
    }
    // rustc -O
    assert_eq!(total, 0xcd1849125358b600);
}
