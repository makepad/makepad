// Unsigned division / remainder by constants (opt::div_by_const): multiply-high sequences
// against the hardware divide. rustc prints the same hash: 10618254766196272890.
#[inline(never)]
fn f64s(x: u64) -> u64 {
    x / 3 ^ x % 3 ^ x / 6 ^ x % 7 ^ x / 10 ^ x % 10 ^ x / 1000 ^ x / 641 ^ x % 65521 ^ x / 7_000_000_007 ^ x % 0x8000_0000_0001 ^ x / 0xFFFF_FFFF_FFFF_FFFF ^ x / 64 ^ x % 4096
}
#[inline(never)]
fn f32s(x: u32) -> u32 {
    x / 3 ^ x % 5 ^ x / 7 ^ x % 13 ^ x / 1000 ^ x % 65521 ^ x / 0xFFFF_FFFF ^ x / 16
}
#[inline(never)]
fn f16s(x: u16) -> u16 { x / 6 ^ x % 9 ^ x / 255 }
#[inline(never)]
fn f8s(x: u8) -> u8 { x / 3 ^ x % 7 ^ x / 10 }
#[test]
fn t() {
    let mut h = 0u64;
    let mut s = 0x9e3779b97f4a7c15u64;
    let edges = [0u64, 1, 2, 3, 5, 6, 7, 9, 10, 641, 999, 1000, 65520, 65521, u32::MAX as u64, u32::MAX as u64 + 1, u64::MAX, u64::MAX - 1, 1 << 63, (1 << 63) - 1];
    for i in 0..200_000u64 {
        s ^= s << 13; s ^= s >> 7; s ^= s << 17;
        let x = if (i as usize) < edges.len() { edges[i as usize] } else if i % 3 == 0 { s >> (s % 64) } else { s };
        h = h.wrapping_mul(31) ^ f64s(x) ^ (f32s(x as u32) as u64) << 1 ^ (f16s(x as u16) as u64) << 2 ^ (f8s(x as u8) as u64) << 3;
    }
    println!("hash {}", h);
    assert_eq!(h, 10618254766196272890);
}
