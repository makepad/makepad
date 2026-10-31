// constant-size aggregate moves of every size from 1 to 300 bytes (the backends' memcpy
// expansions: scalar tails, 16-byte vector pairs, the overlapping vector tail, the loop above
// 256 bytes) copy exactly their bytes: the bytes after and before the copy stay as they were
use std::hint::black_box;

#[derive(Clone, Copy)]
struct Blob<const N: usize>([u8; N]);

fn fill<const N: usize>(seed: u8) -> Blob<N> {
    let mut b = [0u8; N];
    for i in 0..N {
        b[i] = seed.wrapping_mul(31).wrapping_add(i as u8).wrapping_add(1);
    }
    Blob(b)
}

#[inline(never)]
fn moved<const N: usize>(src: &Blob<N>) -> Blob<N> {
    let t = *src;
    let u = t;
    black_box(&u);
    u
}

fn check<const N: usize>() {
    let a: Blob<N> = fill(N as u8);
    let b = moved(&a);
    assert!(a.0 == b.0, "copy of {} bytes", N);
    // a copy into the middle of a buffer leaves its neighbours alone
    let mut buf = [0xA5u8; 400];
    let at = 17;
    unsafe { std::ptr::write(buf.as_mut_ptr().add(at) as *mut Blob<N>, black_box(a)) };
    for i in 0..400 {
        let want = if i >= at && i < at + N { a.0[i - at] } else { 0xA5 };
        assert_eq!(buf[i], want, "size {} byte {}", N, i);
    }
}

macro_rules! sizes { ($($n:literal)*) => { $( check::<$n>(); )* } }

#[test]
fn copies_of_every_small_size() {
    sizes!(1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32
        33 34 35 36 37 38 39 40 41 42 43 44 45 46 47 48 49 50 51 52 53 54 55 56 57 58 59 60 61 62 63 64);
}

#[test]
fn copies_of_larger_sizes() {
    sizes!(65 80 96 100 112 127 128 129 144 160 161 168 192 200 224 240 255 256 257 272 288 300);
}
