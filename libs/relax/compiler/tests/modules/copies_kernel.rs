// Sorts of elements of many sizes: each element moved is a memcpy or a memmove of one element,
// whose size is a constant once the sort's helpers are inlined (copies.rs runs this as host
// code and as a sandboxed module).
fn next(x: &mut u64) -> u64 {
    *x ^= *x << 13;
    *x ^= *x >> 7;
    *x ^= *x << 17;
    *x
}

fn mixed<T: Copy + Ord>(v: &mut Vec<T>, word: fn(&T) -> u64) -> u64 {
    let fold = |v: &[T]| v.iter().enumerate().fold(0u64, |h, (i, x)| h.rotate_left(7) ^ word(x).wrapping_mul(i as u64 + 1));
    v.sort_unstable();
    let a = fold(v);
    let third = v.len() / 3;
    v.rotate_left(third);
    let first = v[0];
    v.insert(third, first);
    let b = fold(v);
    v.sort_by(|p, q| q.cmp(p));
    a ^ b.rotate_left(21) ^ fold(v).rotate_left(42)
}

pub fn copies(n: u32) -> u64 {
    let mut x = 0x2545F4914F6CDD1Du64;
    let mut h = 0u64;
    let mut v: Vec<u8> = (0..n).map(|_| next(&mut x) as u8).collect();
    h ^= mixed(&mut v, |a| *a as u64);
    let mut v: Vec<u16> = (0..n).map(|_| next(&mut x) as u16).collect();
    h = h.rotate_left(3) ^ mixed(&mut v, |a| *a as u64);
    let mut v: Vec<[u8; 3]> = (0..n).map(|_| { let w = next(&mut x); [w as u8, (w >> 8) as u8, (w >> 16) as u8] }).collect();
    h = h.rotate_left(3) ^ mixed(&mut v, |a| a[0] as u64 | (a[1] as u64) << 8 | (a[2] as u64) << 16);
    let mut v: Vec<u32> = (0..n).map(|_| next(&mut x) as u32).collect();
    h = h.rotate_left(3) ^ mixed(&mut v, |a| *a as u64);
    let mut v: Vec<u64> = (0..n).map(|_| next(&mut x)).collect();
    h = h.rotate_left(3) ^ mixed(&mut v, |a| *a);
    let mut v: Vec<[u8; 13]> = (0..n).map(|_| { let w = next(&mut x).to_le_bytes(); [w[0], w[1], w[2], w[3], w[4], w[5], w[6], w[7], w[0], w[2], w[4], w[6], w[1]] }).collect();
    h = h.rotate_left(3) ^ mixed(&mut v, |a| a.iter().fold(0u64, |s, b| s.rotate_left(5) ^ *b as u64));
    let mut v: Vec<u128> = (0..n).map(|_| (next(&mut x) as u128) << 64 | next(&mut x) as u128).collect();
    h = h.rotate_left(3) ^ mixed(&mut v, |a| (*a >> 64) as u64 ^ *a as u64);
    let mut v: Vec<(u64, u32)> = (0..n).map(|_| (next(&mut x) >> 60, next(&mut x) as u32)).collect();
    h = h.rotate_left(3) ^ mixed(&mut v, |a| a.0.rotate_left(32) ^ a.1 as u64);
    let mut v: Vec<[u64; 3]> = (0..n).map(|_| [next(&mut x) >> 62, next(&mut x), next(&mut x)]).collect();
    h = h.rotate_left(3) ^ mixed(&mut v, |a| a[0] ^ a[1].rotate_left(9) ^ a[2].rotate_left(27));
    let mut v: Vec<[u64; 5]> = (0..n).map(|_| [next(&mut x) >> 62, next(&mut x), next(&mut x), next(&mut x), next(&mut x)]).collect();
    h = h.rotate_left(3) ^ mixed(&mut v, |a| a.iter().fold(0u64, |s, b| s.rotate_left(11) ^ *b));
    let mut v: Vec<[u64; 8]> = (0..n).map(|_| { let w = next(&mut x); [w >> 62, w, !w, w << 1, w >> 1, w ^ 5, w, 7] }).collect();
    h = h.rotate_left(3) ^ mixed(&mut v, |a| a.iter().fold(0u64, |s, b| s.rotate_left(11) ^ *b));
    let mut v: Vec<[u64; 9]> = (0..n).map(|_| { let w = next(&mut x); [w >> 62, w, !w, w << 1, w >> 1, w ^ 5, w, 7, w >> 9] }).collect();
    h.rotate_left(3) ^ mixed(&mut v, |a| a.iter().fold(0u64, |s, b| s.rotate_left(11) ^ *b))
}
