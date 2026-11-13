// The three aarch64 NEON intrinsics xxhash-rust's XXH3 stripe loop calls that the std here lacked
// (redb hashes with it): the widening multiply-accumulates vmlal_u32 and vmlal_high_u32, and
// vuzpq_u32, which splits the low from the high halves of four 64-bit words. A few values are
// worked by hand, and XXH3's accumulate step written with them is compared with its scalar
// definition. Each intrinsic is also folded over edge and pseudo-random operands into one hash, and
// the expected hashes are what the real instructions give (this file built with rustc on an Apple
// M-series CPU), so the emulation has to agree with the CPU bit for bit. The "| " lines are equal
// under rustc --test and makepad test.
#![cfg(target_arch = "aarch64")]
use std::arch::aarch64::*;
use std::mem::transmute;

const E32: [u32; 8] = [0, 1, 2, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff, 0x0001_0001, 0xfffe_ffff];

fn next(s: &mut u64) -> u64 {
    *s ^= *s << 13;
    *s ^= *s >> 7;
    *s ^= *s << 17;
    *s
}

/// Operand `arg` of case `case`: the first 512 cases walk every triple of edge values through
/// lane 0 (the other lanes run along the table), the rest are random.
fn lanes32<const N: usize>(s: &mut u64, case: usize, arg: u32) -> [u32; N] {
    let mut r = [0u32; N];
    for (j, x) in r.iter_mut().enumerate() {
        *x = if case < 512 { E32[(case / 8usize.pow(arg) + j * (arg as usize + 1)) % 8] } else { next(s) as u32 };
    }
    r
}

fn lanes64(s: &mut u64, case: usize) -> [u64; 2] {
    let r = [next(s), next(s)];
    match case % 4 {
        0 if case < 512 => [0, u64::MAX],
        1 if case < 512 => [u64::MAX, 1 << (case % 64)],
        // just under the wrap: the largest product is 0xffff_fffe_0000_0001
        2 if case < 512 => [0x0000_0001_ffff_fffe + (r[0] & 3), !0xffff_fffe_0000_0001 + (r[1] & 3)],
        _ => r,
    }
}

/// FNV-1a over the bytes of what `f` returns for each of `n` cases.
fn fold<T: Copy>(n: usize, mut f: impl FnMut(&mut u64, usize) -> T) -> u64 {
    let mut s = 0x9e37_79b9_7f4a_7c15u64;
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for case in 0..n {
        let v = f(&mut s, case);
        let b = unsafe { std::slice::from_raw_parts(&v as *const T as *const u8, std::mem::size_of::<T>()) };
        for &x in b {
            h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

fn expect(name: &str, hash: u64) {
    println!("| {name} {hash:016x}");
    let real = REAL.iter().find(|(n, _)| *n == name).expect(name).1;
    assert_eq!(hash, real, "{name}: the real instruction gives {real:016x}");
}

#[test]
fn a01_worked_values() {
    unsafe {
        // the products are 64 bits wide and the sum wraps
        let a: uint64x2_t = transmute([10u64, u64::MAX]);
        let r: [u64; 2] = transmute(vmlal_u32(a, transmute([3u32, u32::MAX]), transmute([5u32, u32::MAX])));
        assert_eq!(r, [25, 0xffff_fffe_0000_0000]);
        // the high form multiplies lanes 2 and 3
        let (b, c): (uint32x4_t, uint32x4_t) = (transmute([9u32, 9, 3, u32::MAX]), transmute([9u32, 9, 5, u32::MAX]));
        let r: [u64; 2] = transmute(vmlal_high_u32(a, b, c));
        assert_eq!(r, [25, 0xffff_fffe_0000_0000]);
        let r: [u64; 2] = transmute(vmlal_high_u32(transmute([0u64; 2]), transmute([1u32, 2, 0x8000_0000, 7]), transmute([3u32, 4, 2, 6])));
        assert_eq!(r, [0x1_0000_0000, 42]);
        // even lanes of both operands, then the odd ones
        let z: [[u32; 4]; 2] = transmute(vuzpq_u32(transmute([0u32, 1, 2, 3]), transmute([4u32, 5, 6, 7])));
        assert_eq!(z, [[0, 2, 4, 6], [1, 3, 5, 7]]);
        // which, on 64-bit words, are their low and their high halves
        let words = [0x1111_1111_aaaa_aaaau64, 0x2222_2222_bbbb_bbbb, 0x3333_3333_cccc_cccc, 0x4444_4444_dddd_dddd];
        let z = vuzpq_u32(transmute([words[0], words[1]]), transmute([words[2], words[3]]));
        let (lo, hi): ([u32; 4], [u32; 4]) = (transmute(z.0), transmute(z.1));
        assert_eq!(lo, [0xaaaa_aaaa, 0xbbbb_bbbb, 0xcccc_cccc, 0xdddd_dddd]);
        assert_eq!(hi, [0x1111_1111, 0x2222_2222, 0x3333_3333, 0x4444_4444]);
        println!("| worked values ok");
    }
}

/// XXH3's accumulate step over one 64-byte stripe as its specification writes it.
fn accumulate_scalar(acc: &mut [u64; 8], stripe: &[u8; 64], secret: &[u8; 64]) {
    for i in 0..8 {
        let data = u64::from_le_bytes(stripe[8 * i..8 * i + 8].try_into().unwrap());
        let key = data ^ u64::from_le_bytes(secret[8 * i..8 * i + 8].try_into().unwrap());
        acc[i ^ 1] = acc[i ^ 1].wrapping_add(data);
        acc[i] = acc[i].wrapping_add((key & 0xffff_ffff) * (key >> 32));
    }
}

/// The same step as xxhash-rust's aarch64 loop: two vectors of two words at a time.
unsafe fn accumulate_neon(acc: &mut [u64; 8], stripe: &[u8; 64], secret: &[u8; 64]) {
    for i in 0..2 {
        let (data_1, data_2) = (vld1q_u8(stripe.as_ptr().add(32 * i)), vld1q_u8(stripe.as_ptr().add(32 * i + 16)));
        let (key_1, key_2) = (vld1q_u8(secret.as_ptr().add(32 * i)), vld1q_u8(secret.as_ptr().add(32 * i + 16)));
        let halves = vuzpq_u32(vreinterpretq_u32_u8(veorq_u8(data_1, key_1)), vreinterpretq_u32_u8(veorq_u8(data_2, key_2)));
        let (swap_1, swap_2) = (vreinterpretq_u64_u8(vextq_u8::<8>(data_1, data_1)), vreinterpretq_u64_u8(vextq_u8::<8>(data_2, data_2)));
        let sum_1 = vmlal_u32(swap_1, vget_low_u32(halves.0), vget_low_u32(halves.1));
        let sum_2 = vmlal_high_u32(swap_2, halves.0, halves.1);
        let p = acc.as_mut_ptr().add(4 * i);
        vst1q_u64(p, vaddq_u64(vld1q_u64(p), sum_1));
        vst1q_u64(p.add(2), vaddq_u64(vld1q_u64(p.add(2)), sum_2));
    }
}

#[test]
fn a02_xxh3_accumulate() {
    // XXH3's initial accumulators
    let init = [
        0xc2b2_ae3du64, 0x9e37_79b1_85eb_ca87, 0xc2b2_ae3d_27d4_eb4f, 0x1656_67b1_9e37_79f9, 0x85eb_ca77_c2b2_ae63, 0x85eb_ca77,
        0x27d4_eb2f_1656_67c5, 0x9e37_79b1,
    ];
    let (mut scalar, mut neon) = (init, init);
    let mut s = 0x2545_f491_4f6c_dd1du64;
    let (mut stripe, mut secret) = ([0u8; 64], [0u8; 64]);
    for round in 0..512 {
        for i in 0..64 {
            // some stripes of zeros and ones, so the products and the sums overflow
            stripe[i] = if round % 7 == 3 { 0xff } else { next(&mut s) as u8 };
            secret[i] = if round % 5 == 4 { 0 } else { next(&mut s) as u8 };
        }
        accumulate_scalar(&mut scalar, &stripe, &secret);
        unsafe { accumulate_neon(&mut neon, &stripe, &secret) };
        assert_eq!(neon, scalar, "after stripe {round}");
    }
    println!("| xxh3 accumulate {:016x} {:016x} {:016x} {:016x}", neon[0], neon[1], neon[6], neon[7]);
}

#[test]
fn a03_each_instruction() {
    unsafe {
        expect(
            "vmlal_u32",
            fold(2560, |s, case| {
                let (a, b, c) = (lanes64(s, case), lanes32::<2>(s, case, 0), lanes32::<2>(s, case, 1));
                vmlal_u32(transmute(a), transmute(b), transmute(c))
            }),
        );
        expect(
            "vmlal_high_u32",
            fold(2560, |s, case| {
                let (a, b, c) = (lanes64(s, case), lanes32::<4>(s, case, 0), lanes32::<4>(s, case, 1));
                vmlal_high_u32(transmute(a), transmute(b), transmute(c))
            }),
        );
        expect(
            "vuzpq_u32",
            fold(2560, |s, case| {
                let (a, b) = (lanes32::<4>(s, case, 0), lanes32::<4>(s, case, 2));
                vuzpq_u32(transmute(a), transmute(b))
            }),
        );
    }
}

/// What the real instructions give: every hash above, from this file built with rustc.
const REAL: &[(&str, u64)] = &[
    ("vmlal_u32", 0x63462d41b01afc80),
    ("vmlal_high_u32", 0xe2cff355dad2b1d5),
    ("vuzpq_u32", 0xcb0c4910a3cb0152),
];
