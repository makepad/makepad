// Byte stores of one value's bytes that the backend merges into one wide store (codegen
// loops.rs merge_stores): full u64 / u32 / u16 runs, partial runs (7, 3 bytes), a load between
// the stores (must not merge across it), mixed sources, overlapping writes, an [u8; 8] copy loop
// as in inflate; called often enough for O3 (= rustc --test).
#[inline(never)]
fn put64(out: &mut [u8], at: usize, x: u64) {
    out[at] = x as u8;
    out[at + 1] = (x >> 8) as u8;
    out[at + 2] = (x >> 16) as u8;
    out[at + 3] = (x >> 24) as u8;
    out[at + 4] = (x >> 32) as u8;
    out[at + 5] = (x >> 40) as u8;
    out[at + 6] = (x >> 48) as u8;
    out[at + 7] = (x >> 56) as u8;
}
#[inline(never)]
fn put7(out: &mut [u8], at: usize, x: u64) {
    for j in 0..7 {
        out[at + j] = (x >> (8 * j)) as u8;
    }
}
#[inline(never)]
fn put32_16(out: &mut [u8], at: usize, x: u32, y: u16) {
    out[at..at + 4].copy_from_slice(&x.to_le_bytes());
    out[at + 4..at + 6].copy_from_slice(&y.to_le_bytes());
    out[at + 6] = (x >> 3) as u8;
}
#[inline(never)]
fn with_load(out: &mut [u8], at: usize, x: u64) -> u8 {
    out[at] = x as u8;
    out[at + 1] = (x >> 8) as u8;
    let seen = out[at];
    out[at + 2] = (x >> 16) as u8;
    out[at + 3] = (x >> 24) as u8;
    seen
}
#[inline(never)]
fn overlap(out: &mut [u8], at: usize, x: u64, y: u64) {
    put64(out, at, x);
    out[at + 3] = (y >> 8) as u8;
    out[at + 4] = (y >> 16) as u8;
}
#[inline(never)]
fn copy8(src: &[u8], dst: &mut [u8], mut si: usize, mut di: usize, n: usize) {
    for _ in 0..n {
        let w = u64::from_le_bytes(src[si..si + 8].try_into().unwrap());
        dst[di..di + 8].copy_from_slice(&w.to_le_bytes());
        si += 3;
        di += 8;
    }
}

#[test]
fn merged_stores() {
    let src: Vec<u8> = (0..400u32).map(|i| (i * 37 + 11) as u8).collect();
    let mut buf = vec![0u8; 512];
    let mut h = 0u64;
    for rep in 0..40000u64 {
        let x = rep.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0x0102_0304_0506_0708;
        let at = (rep % 50) as usize;
        put64(&mut buf, at, x);
        put7(&mut buf, at + 9, !x);
        put32_16(&mut buf, at + 20, x as u32, (x >> 40) as u16);
        h = h.rotate_left(5) ^ with_load(&mut buf, at + 30, x) as u64;
        overlap(&mut buf, at + 40, x, x.rotate_left(17));
        copy8(&src, &mut buf[64..], (rep % 7) as usize, 0, 40);
        for k in (0..512).step_by(61) {
            h = h.rotate_left(3) ^ buf[k] as u64 ^ (buf[(k * 7 + at) % 512] as u64) << 8;
        }
    }
    let s: u64 = buf.iter().map(|b| *b as u64).sum();
    println!("| {:016x} {}", h, s);
}
