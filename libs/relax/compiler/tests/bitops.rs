// Bit intrinsics (ctpop/ctlz/cttz/bswap/bitreverse) on every integer width incl. 128-bit,
// edge values (0, 1, all ones, sign bit). Output must equal rustc's.

fn mix(mut x: u64) -> u64 {
    x ^= x >> 33;
    x = x.wrapping_mul(0xff51afd7ed558ccd);
    x ^= x >> 33;
    x
}

#[test]
fn widths() {
    let mut seeds: Vec<u64> = vec![0, 1, 2, 3, 0x80, 0xff, 0x8000, 0xffff, 0x8000_0000, 0xffff_ffff, 1 << 63, u64::MAX, u64::MAX - 1];
    let mut s = 12345u64;
    for _ in 0..24 {
        s = mix(s.wrapping_add(0x9e3779b97f4a7c15));
        seeds.push(s);
        seeds.push(s >> (s & 63));
    }
    let mut h = 0u64;
    for &x in &seeds {
        let a = [
            x.count_ones() as u64, x.leading_zeros() as u64, x.trailing_zeros() as u64, x.swap_bytes(), x.reverse_bits(),
            (x as u32).count_ones() as u64, (x as u32).leading_zeros() as u64, (x as u32).trailing_zeros() as u64, (x as u32).swap_bytes() as u64, (x as u32).reverse_bits() as u64,
            (x as u16).count_ones() as u64, (x as u16).leading_zeros() as u64, (x as u16).trailing_zeros() as u64, (x as u16).swap_bytes() as u64, (x as u16).reverse_bits() as u64,
            (x as u8).count_ones() as u64, (x as u8).leading_zeros() as u64, (x as u8).trailing_zeros() as u64, (x as u8).reverse_bits() as u64,
            (x as i32).count_ones() as u64, (x as i32).leading_zeros() as u64, (x as i32).swap_bytes() as i64 as u64, (x as i64).reverse_bits() as u64,
            (x as i16).swap_bytes() as i64 as u64, (x as i8).reverse_bits() as i64 as u64,
        ];
        for v in a {
            h = mix(h ^ v);
        }
    }
    println!("narrow hash {:x} over {} values", h, seeds.len());
    println!("u64 max {} {} {}", u64::MAX.count_ones(), 0u64.leading_zeros(), 0u32.trailing_zeros());
    println!("{:x} {:x}", 0x0102_0304_0506_0708u64.swap_bytes(), 1u64.reverse_bits());
}

#[test]
fn wide() {
    let vals: [u128; 6] = [0, 1, u128::MAX, 1 << 64, (1 << 127) | 5, 0x0123_4567_89ab_cdef_0011_2233_4455_6677];
    for v in vals {
        println!("{} {} {} {:x} {:x}", v.count_ones(), v.leading_zeros(), v.trailing_zeros(), v.swap_bytes(), v.reverse_bits());
        let i = v as i128;
        println!("{} {} {:x}", i.count_ones(), i.trailing_zeros(), i.swap_bytes() as u128);
    }
}
