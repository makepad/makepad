// On x86_64 hosts, agree.sh compares Relax's digests with rustc's on the same CPU.
// On Arm hosts, it compares Relax's digests with the real x86_64 digests in arch/x86_64_gaps_real.txt.
#![allow(non_snake_case)]
include!("arch/common.rs");
use core::arch::x86_64 as real;

const CASES: usize = 17 + 512;
const _: () = assert!(CASES < 17 + RANDOM_CASES);

// Include the integer boundaries and their distinct f32 neighbors.
// At this precision i32::MAX and i32::MIN - 1 round to powers of two.
const EXTRA_FLOATS: &[f32] = &[
    0.5, -0.5, 1.5, 2.5, -2.5,
    2_147_483_647.0, 2_147_483_648.0, -2_147_483_649.0, 9.3e18,
    2_147_483_520.0, -2_147_483_904.0,
    0.0, -0.0, 0.0, f32::INFINITY, f32::NEG_INFINITY, f32::NAN, -1.0, 1.0,
    // Exercise denormal inputs, the reciprocal flush boundary and NaN payloads.
    f32::from_bits(0x006C_E3EE), f32::from_bits(0x8001_16C2), f32::from_bits(0x0000_0001),
    f32::from_bits(0x7F00_0000), f32::from_bits(0xFF00_0000), f32::from_bits(0x7E80_0000),
    f32::from_bits(0x7E80_1000), f32::from_bits(0x7E7F_F000), f32::from_bits(0x7E90_0000),
    f32::from_bits(0x0080_0000), f32::from_bits(0x0100_0000), f32::from_bits(0x7F7F_FFFF),
    f32::from_bits(0x7F80_0001), f32::from_bits(0xFFC0_0001),
];
const FLOAT_CASES: usize = CASES + EXTRA_FLOATS.len();

const FMA_FIXED: &[[u32; 3]] = &[
    [0x7F80_0000, 0x0000_0000, 0x3F80_0000], // infinity times zero
    [0x0000_0000, 0xFF80_0000, 0x3F80_0000], // zero times negative infinity
    [0x7F80_0000, 0x0000_0000, 0x7FC0_0003], // invalid product with a NaN addend
    [0x3F80_0000, 0x7FC0_0001, 0xFFC0_0002], // NaNs in b and c
    // NaN in a and c: a multiplicand's NaN beats the addend's in every instruction form. With NaN in
    // both a and b no row is kept: which comes back follows the form the compiler picks (132, 213 or
    // 231 multiply in different orders), and rustc's own code differs between intrinsics.
    [0x7FC0_0004, 0x3F80_0000, 0xFFC0_0006],
    [0x3F80_0000, 0x7F80_0001, 0x7FC0_0007], // signaling b and quiet c
    [0x7F80_0000, 0x3F80_0000, 0xFF80_0000], // infinity minus infinity
    [0x8000_0000, 0x3F80_0000, 0x8000_0000], // two negative zeros
    [0x8000_0000, 0x3F80_0000, 0x0000_0000], // opposite signed zeros
];
const FMA_CASES: usize = CASES + FMA_FIXED.len();

unsafe fn fma_args<T: Copy>(rng: &mut Rng, case: usize, arg: usize) -> T {
    if case < CASES {
        return sample(rng, case, arg);
    }
    let lane = |i: usize| FMA_FIXED[(case - CASES + i) % FMA_FIXED.len()][arg];
    match size_of::<T>() / 4 {
        4 => cast(core::array::from_fn::<_, 4, _>(lane)),
        8 => cast(core::array::from_fn::<_, 8, _>(lane)),
        _ => unreachable!(),
    }
}

unsafe fn float_lanes<const LANES: usize>(rng: &mut Rng, case: usize, arg: usize) -> [f32; LANES] {
    if case < CASES {
        return sample(rng, case, arg);
    }
    let mut lanes = [0.0; LANES];
    for (i, lane) in lanes.iter_mut().enumerate() {
        *lane = EXTRA_FLOATS[(case - CASES + arg + i) % EXTRA_FLOATS.len()];
    }
    lanes
}

unsafe fn floats4(rng: &mut Rng, case: usize, arg: usize) -> real::__m128 {
    cast(float_lanes::<4>(rng, case, arg))
}

unsafe fn floats8(rng: &mut Rng, case: usize, arg: usize) -> real::__m256 {
    cast(float_lanes::<8>(rng, case, arg))
}

fn add_estimate(d: &mut Digest, x: f32, result: f32, rsqrt: bool) {
    let bits = x.to_bits();
    let mag = bits & 0x7fff_ffff;
    let sign = bits & 0x8000_0000;
    // The hardware specifies every result bit for special operands and guaranteed flushes.
    if mag < 0x0080_0000
        || mag >= 0x7f80_0000
        || (rsqrt && sign != 0)
        || (!rsqrt && mag >= 0x7e80_3001)
    {
        d.add(&result.to_bits().to_le_bytes());
        return;
    }
    let x = f64::from(x);
    let exact = if rsqrt { 1.0 / x.sqrt() } else { 1.0 / x };
    let accurate = (f64::from(result) - exact).abs() <= exact.abs() * (1.5 / 4096.0);
    if !rsqrt && mag > 0x7e7f_e800 {
        // Either signed zero or an accurate estimate is valid at the flush boundary.
        d.add(&[(result.to_bits() == sign || accurate) as u8]);
    } else {
        // Normal estimates may differ within the hardware's relative error bound.
        d.add(&[accurate as u8]);
    }
}

macro_rules! require_feature {
    ($name:ident, $feature:tt) => {
        if cfg!(target_arch = "x86_64") && !std::is_x86_feature_detected!($feature) {
            eprintln!("{}: {} unavailable", stringify!($name), $feature);
            return;
        }
    };
}

// The test number selects a reproducible random stream for its operands.
macro_rules! cases {
    ($name:ident, $feature:tt, $seed:literal, $count:expr, $tag:expr, |$rng:ident, $case:ident, $d:ident| $body:block) => {{
        require_feature!($name, $feature);
        let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ $seed);
        let mut digest = Digest::new();
        for $case in 0..$count {
            let $rng = &mut rng;
            let $d = &mut digest;
            unsafe { $body }
        }
        digest.line(stringify!($name), $tag);
    }};
}

macro_rules! sampled {
    ($name:ident, $feature:tt, $seed:literal, $count:expr, $input:ident, [$($arg:literal),+]) => {
        cases!($name, $feature, $seed, $count, "none", |rng, case, d| {
            let result = real::$name($($input(rng, case, $arg)),+);
            d.add(bytes(&result));
        });
    };
}

// Each immediate gets its own call boundary so LLVM needn't optimize hundreds
// of specialized intrinsics inside a single sampling loop.
macro_rules! check_immediates {
    ([$first:literal $(, $imm:literal)*], $name:ident, $feature:tt, $seed:literal,
     $count:expr, $input:ident, [$($arg:ident: $ty:ty = $index:literal),+]) => {{
        #[inline(never)]
        unsafe fn compare<const N: i32>(($($arg,)+ d): ($($ty,)+ &mut Digest)) {
            d.add(bytes(&N));
            let result = real::$name::<N>($($arg),+);
            d.add(bytes(&result));
        }
        cases!($name, $feature, $seed, $count, concat!($first $(, ",", $imm)*), |rng, case, d| {
            $(let $arg = $input(rng, case, $index);)+
            immediate_cases!(compare, ($($arg,)+ d), [$first $(, $imm)*]);
        });
    }};
}

// stdarch checks these operands as unsigned integers of the indicated width.
// The same literal list drives the calls and labels their digest.
macro_rules! immediates {
    (1, $($test:tt)*) => { check_immediates!([0, 1], $($test)*); };
    (2, $($test:tt)*) => { check_immediates!([0, 1, 2, 3], $($test)*); };
    (3, $($test:tt)*) => { check_immediates!([0, 1, 2, 3, 4, 5, 6, 7], $($test)*); };
    (4, $($test:tt)*) => { check_immediates!([0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15], $($test)*); };
    (5, $($test:tt)*) => {
        check_immediates!([
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31
        ], $($test)*);
    };
    (8, $($test:tt)*) => {
        check_immediates!([
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
            32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
            48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
            64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79,
            80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95,
            96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111,
            112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126, 127,
            128, 129, 130, 131, 132, 133, 134, 135, 136, 137, 138, 139, 140, 141, 142, 143,
            144, 145, 146, 147, 148, 149, 150, 151, 152, 153, 154, 155, 156, 157, 158, 159,
            160, 161, 162, 163, 164, 165, 166, 167, 168, 169, 170, 171, 172, 173, 174, 175,
            176, 177, 178, 179, 180, 181, 182, 183, 184, 185, 186, 187, 188, 189, 190, 191,
            192, 193, 194, 195, 196, 197, 198, 199, 200, 201, 202, 203, 204, 205, 206, 207,
            208, 209, 210, 211, 212, 213, 214, 215, 216, 217, 218, 219, 220, 221, 222, 223,
            224, 225, 226, 227, 228, 229, 230, 231, 232, 233, 234, 235, 236, 237, 238, 239,
            240, 241, 242, 243, 244, 245, 246, 247, 248, 249, 250, 251, 252, 253, 254, 255
        ], $($test)*);
    };
}

const SHIFT_COUNTS: &[u64] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 31, 32, 63, 64, 65,
];

macro_rules! shifts {
    ($name:ident, $seed:literal) => {
        shifts!($name, "sse2", $seed);
    };
    ($name:ident, $feature:tt, $seed:literal) => {
        cases!($name, $feature, $seed, CASES, "none", |rng, case, d| {
            let a = sample(rng, case, 0);
            let mut count: [u64; 2] = sample(rng, case, 1);
            d.add(bytes(&real::$name(a, cast(count))));
            // Only the low quadword selects the shift distance.
            count[0] = SHIFT_COUNTS[case % SHIFT_COUNTS.len()];
            d.add(bytes(&real::$name(a, cast(count))));
        });
    };
}

macro_rules! lane_shifts {
    ($name:ident, $seed:literal, [$lane:ty; $lanes:literal]) => {
        cases!($name, "avx2", $seed, CASES, "none", |rng, case, d| {
            let a = sample(rng, case, 0);
            let mut counts: [$lane; $lanes] = sample(rng, case, 1);
            let width = <$lane>::BITS as $lane;
            for (i, count) in counts.iter_mut().enumerate() {
                // Rotate the boundaries so adjacent lanes can shift by different amounts.
                let choices = [
                    0, 1, width - 1, width, width + 1, 1 << (width - 1), *count, *count % width,
                ];
                *count = choices[(case + i) % choices.len()];
            }
            d.add(bytes(&real::$name(a, cast(counts))));
        });
    };
}

macro_rules! gathers {
    ($name:ident, $seed:literal, $indices:ty $(, $src:ident, $mask:ident)?) => {
        cases!($name, "avx2", $seed, CASES, "1,2,4,8", |rng, case, d| {
            let memory: Buffer = sample(rng, case, 0);
            let base = memory.0.as_ptr().add(memory.0.len() / 2).cast();
            let mut indices: $indices = sample(rng, case, 1);
            // From byte 96, indices -12..=11 keep even an 8-byte read at scale 8 in bounds.
            for index in &mut indices {
                *index = index.rem_euclid(24) - 12;
            }
            let offsets = cast(indices);
            $(
                let $src = sample(rng, case, 2);
                let $mask = sample(rng, case, 3);
            )?
            gathers!(@scales [1, 2, 4, 8], d, $name, ($($src,)? base, offsets $(, $mask)?));
        });
    };
    (@scales [$($scale:literal),+], $d:ident, $name:ident, $args:tt) => {
        $($d.add(bytes(&real::$name::<$scale> $args));)+
    };
}

macro_rules! masked_loads {
    ($name:ident, $seed:literal) => {
        cases!($name, "avx2", $seed, CASES, "none", |rng, case, d| {
            let src: Buffer = sample(rng, case, 0);
            let ptr = src.0.as_ptr().add(case % 16).cast();
            let mask = sample(rng, case, 1);
            d.add(bytes(&real::$name(ptr, mask)));
        });
    };
}

macro_rules! masked_stores {
    ($name:ident, $seed:literal) => {
        cases!($name, "avx2", $seed, CASES, "none", |rng, case, d| {
            let mut dst = Buffer([0; 192]);
            let ptr = dst.0.as_mut_ptr().add(case % 16).cast();
            let mask = sample(rng, case, 1);
            let a = sample(rng, case, 2);
            real::$name(ptr, mask, a);
            d.add(&dst.0);
        });
    };
}

// Independent random bit masks almost always intersect without either being a
// subset. Also exercise strict subsets, disjoint masks and strict supersets.
macro_rules! bit_tests {
    ($name:ident, $feature:tt, $seed:literal, $lanes:literal) => {
        cases!($name, $feature, $seed, CASES, "none", |rng, case, d| {
            let a: [u32; $lanes] = sample(rng, case, 0);
            let mut mask: [u32; $lanes] = sample(rng, case, 1);
            d.add(bytes(&real::$name(cast(a), cast(mask))));
            for i in 0..$lanes {
                mask[i] = match case % 3 {
                    0 => mask[i] & a[i],
                    1 => mask[i] & !a[i],
                    _ => mask[i] | a[i],
                };
            }
            d.add(bytes(&real::$name(cast(a), cast(mask))));
        });
    };
}

// SSE

#[test]
fn b001__mm_cmpeq_ss() {
    sampled!(_mm_cmpeq_ss, "sse", 1, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b002__mm_cmpge_ss() {
    sampled!(_mm_cmpge_ss, "sse", 2, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b003__mm_cmpgt_ss() {
    sampled!(_mm_cmpgt_ss, "sse", 3, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b004__mm_cmple_ss() {
    sampled!(_mm_cmple_ss, "sse", 4, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b005__mm_cmplt_ss() {
    sampled!(_mm_cmplt_ss, "sse", 5, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b006__mm_cmpneq_ss() {
    sampled!(_mm_cmpneq_ss, "sse", 6, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b007__mm_cmpnge_ps() {
    sampled!(_mm_cmpnge_ps, "sse", 7, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b008__mm_cmpnge_ss() {
    sampled!(_mm_cmpnge_ss, "sse", 8, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b009__mm_cmpngt_ps() {
    sampled!(_mm_cmpngt_ps, "sse", 9, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b010__mm_cmpngt_ss() {
    sampled!(_mm_cmpngt_ss, "sse", 10, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b011__mm_cmpnle_ps() {
    sampled!(_mm_cmpnle_ps, "sse", 11, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b012__mm_cmpnle_ss() {
    sampled!(_mm_cmpnle_ss, "sse", 12, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b013__mm_cmpnlt_ps() {
    sampled!(_mm_cmpnlt_ps, "sse", 13, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b014__mm_cmpnlt_ss() {
    sampled!(_mm_cmpnlt_ss, "sse", 14, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b015__mm_cmpord_ss() {
    sampled!(_mm_cmpord_ss, "sse", 15, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b016__mm_cmpunord_ss() {
    sampled!(_mm_cmpunord_ss, "sse", 16, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b017__mm_comieq_ss() {
    sampled!(_mm_comieq_ss, "sse", 17, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b018__mm_comige_ss() {
    sampled!(_mm_comige_ss, "sse", 18, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b019__mm_comigt_ss() {
    sampled!(_mm_comigt_ss, "sse", 19, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b020__mm_comile_ss() {
    sampled!(_mm_comile_ss, "sse", 20, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b021__mm_comilt_ss() {
    sampled!(_mm_comilt_ss, "sse", 21, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b022__mm_comineq_ss() {
    sampled!(_mm_comineq_ss, "sse", 22, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b023__mm_cvt_si2ss() {
    sampled!(_mm_cvt_si2ss, "sse", 23, CASES, sample, [0, 1]);
}

#[test]
fn b024__mm_cvt_ss2si() {
    sampled!(_mm_cvt_ss2si, "sse", 24, FLOAT_CASES, floats4, [0]);
}

#[test]
fn b025__mm_cvtsi32_ss() {
    sampled!(_mm_cvtsi32_ss, "sse", 25, CASES, sample, [0, 1]);
}

#[test]
fn b026__mm_cvtsi64_ss() {
    sampled!(_mm_cvtsi64_ss, "sse", 26, CASES, sample, [0, 1]);
}

#[test]
fn b027__mm_cvtss_si32() {
    sampled!(_mm_cvtss_si32, "sse", 27, FLOAT_CASES, floats4, [0]);
}

#[test]
fn b028__mm_cvtss_si64() {
    sampled!(_mm_cvtss_si64, "sse", 28, FLOAT_CASES, floats4, [0]);
}

#[test]
fn b029__mm_cvtt_ss2si() {
    sampled!(_mm_cvtt_ss2si, "sse", 29, FLOAT_CASES, floats4, [0]);
}

#[test]
fn b030__mm_cvttss_si32() {
    sampled!(_mm_cvttss_si32, "sse", 30, FLOAT_CASES, floats4, [0]);
}

#[test]
fn b031__mm_cvttss_si64() {
    sampled!(_mm_cvttss_si64, "sse", 31, FLOAT_CASES, floats4, [0]);
}

#[test]
fn b032__mm_div_ss() {
    sampled!(_mm_div_ss, "sse", 32, CASES, sample, [0, 1]);
}

#[test]
fn b033__mm_load_ps1() {
    cases!(_mm_load_ps1, "sse", 33, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add((case % 4) * 4).cast();
        d.add(bytes(&real::_mm_load_ps1(ptr)));
    });
}

#[test]
fn b034__mm_load_ss() {
    cases!(_mm_load_ss, "sse", 34, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add((case % 4) * 4).cast();
        d.add(bytes(&real::_mm_load_ss(ptr)));
    });
}

#[test]
fn b035__mm_load1_ps() {
    cases!(_mm_load1_ps, "sse", 35, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add((case % 4) * 4).cast();
        d.add(bytes(&real::_mm_load1_ps(ptr)));
    });
}

#[test]
fn b036__mm_loadr_ps() {
    cases!(_mm_loadr_ps, "sse", 36, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add((case % 4) * 16).cast();
        d.add(bytes(&real::_mm_loadr_ps(ptr)));
    });
}

#[test]
fn b037__mm_max_ss() {
    sampled!(_mm_max_ss, "sse", 37, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b038__mm_min_ss() {
    sampled!(_mm_min_ss, "sse", 38, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b039__mm_move_ss() {
    sampled!(_mm_move_ss, "sse", 39, CASES, sample, [0, 1]);
}

#[test]
fn b040__mm_movelh_ps() {
    sampled!(_mm_movelh_ps, "sse", 40, CASES, sample, [0, 1]);
}

#[test]
fn b041__mm_mul_ss() {
    sampled!(_mm_mul_ss, "sse", 41, CASES, sample, [0, 1]);
}

#[test]
fn b042__mm_rcp_ss() {
    cases!(_mm_rcp_ss, "sse", 42, FLOAT_CASES, "none", |rng, case, d| {
        let input = float_lanes::<4>(rng, case, 0);
        let output: [f32; 4] = cast(real::_mm_rcp_ss(cast(input)));
        add_estimate(d, input[0], output[0], false);
        d.add(&bytes(&output)[4..]);
    });
}

#[test]
fn b043__mm_rsqrt_ss() {
    cases!(_mm_rsqrt_ss, "sse", 43, FLOAT_CASES, "none", |rng, case, d| {
        let input = float_lanes::<4>(rng, case, 0);
        let output: [f32; 4] = cast(real::_mm_rsqrt_ss(cast(input)));
        add_estimate(d, input[0], output[0], true);
        d.add(&bytes(&output)[4..]);
    });
}

#[test]
fn b044__mm_sfence() {
    require_feature!(_mm_sfence, "sse");
    unsafe { real::_mm_sfence() };
    let mut d = Digest::new();
    d.add(&[1]);
    d.line("_mm_sfence", "none");
}

#[test]
fn b045__mm_sqrt_ss() {
    sampled!(_mm_sqrt_ss, "sse", 45, CASES, sample, [0]);
}

#[test]
fn b046__mm_store_ps1() {
    cases!(_mm_store_ps1, "sse", 46, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add((case % 4) * 16).cast();
        let a = sample(rng, case, 1);
        real::_mm_store_ps1(ptr, a);
        d.add(&dst.0);
    });
}

#[test]
fn b047__mm_store_ss() {
    cases!(_mm_store_ss, "sse", 47, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add((case % 4) * 4).cast();
        let a = sample(rng, case, 1);
        real::_mm_store_ss(ptr, a);
        d.add(&dst.0);
    });
}

#[test]
fn b048__mm_store1_ps() {
    cases!(_mm_store1_ps, "sse", 48, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add((case % 4) * 16).cast();
        let a = sample(rng, case, 1);
        real::_mm_store1_ps(ptr, a);
        d.add(&dst.0);
    });
}

#[test]
fn b049__mm_storer_ps() {
    cases!(_mm_storer_ps, "sse", 49, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add((case % 4) * 16).cast();
        let a = sample(rng, case, 1);
        real::_mm_storer_ps(ptr, a);
        d.add(&dst.0);
    });
}

#[test]
fn b050__mm_stream_ps() {
    cases!(_mm_stream_ps, "sse", 50, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add((case % 4) * 16).cast();
        let a = sample(rng, case, 1);
        real::_mm_stream_ps(ptr, a);
        real::_mm_sfence();
        d.add(&dst.0);
    });
}

#[test]
fn b051__mm_sub_ss() {
    sampled!(_mm_sub_ss, "sse", 51, CASES, sample, [0, 1]);
}

#[test]
fn b052__mm_ucomieq_ss() {
    sampled!(_mm_ucomieq_ss, "sse", 52, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b053__mm_ucomige_ss() {
    sampled!(_mm_ucomige_ss, "sse", 53, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b054__mm_ucomigt_ss() {
    sampled!(_mm_ucomigt_ss, "sse", 54, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b055__mm_ucomile_ss() {
    sampled!(_mm_ucomile_ss, "sse", 55, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b056__mm_ucomilt_ss() {
    sampled!(_mm_ucomilt_ss, "sse", 56, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b057__mm_ucomineq_ss() {
    sampled!(_mm_ucomineq_ss, "sse", 57, FLOAT_CASES, floats4, [0, 1]);
}

// SSE2

#[test]
fn b058__mm_bslli_si128() {
    immediates!(8, _mm_bslli_si128, "sse2", 58, CASES, sample,
        [a: real::__m128i = 0]);
}

#[test]
fn b059__mm_bsrli_si128() {
    immediates!(8, _mm_bsrli_si128, "sse2", 59, CASES, sample,
        [a: real::__m128i = 0]);
}

#[test]
fn b060__mm_clflush() {
    require_feature!(_mm_clflush, "sse2");
    let data = Buffer([0; 192]);
    unsafe { real::_mm_clflush(data.0.as_ptr()) };
    let mut d = Digest::new();
    d.add(&[1]);
    d.line("_mm_clflush", "none");
}

#[test]
fn b061__mm_insert_epi16() {
    immediates!(3, _mm_insert_epi16, "sse2", 61, CASES, sample,
        [a: real::__m128i = 0, i: i32 = 1]);
}

#[test]
fn b062__mm_lfence() {
    require_feature!(_mm_lfence, "sse2");
    unsafe { real::_mm_lfence() };
    let mut d = Digest::new();
    d.add(&[1]);
    d.line("_mm_lfence", "none");
}

#[test]
fn b063__mm_loadu_si16() {
    cases!(_mm_loadu_si16, "sse2", 63, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add(case % 16).cast();
        d.add(bytes(&real::_mm_loadu_si16(ptr)));
    });
}

#[test]
fn b064__mm_loadu_si32() {
    cases!(_mm_loadu_si32, "sse2", 64, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add(case % 16).cast();
        d.add(bytes(&real::_mm_loadu_si32(ptr)));
    });
}

#[test]
fn b065__mm_loadu_si64() {
    cases!(_mm_loadu_si64, "sse2", 65, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add(case % 16).cast();
        d.add(bytes(&real::_mm_loadu_si64(ptr)));
    });
}

#[test]
fn b066__mm_maskmoveu_si128() {
    cases!(_mm_maskmoveu_si128, "sse2", 66, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add(case % 16).cast();
        let a = sample(rng, case, 0);
        let mask = sample(rng, case, 1);
        real::_mm_maskmoveu_si128(a, mask, ptr);
        real::_mm_sfence();
        d.add(&dst.0);
    });
}

#[test]
fn b067__mm_mfence() {
    require_feature!(_mm_mfence, "sse2");
    unsafe { real::_mm_mfence() };
    let mut d = Digest::new();
    d.add(&[1]);
    d.line("_mm_mfence", "none");
}

#[test]
fn b068__mm_mulhi_epi16() {
    sampled!(_mm_mulhi_epi16, "sse2", 68, CASES, sample, [0, 1]);
}

#[test]
fn b069__mm_pause() {
    require_feature!(_mm_pause, "sse2");
    real::_mm_pause();
    let mut d = Digest::new();
    d.add(&[1]);
    d.line("_mm_pause", "none");
}

#[test]
fn b070__mm_setr_epi16() {
    sampled!(_mm_setr_epi16, "sse2", 70, CASES, sample, [0, 1, 2, 3, 4, 5, 6, 7]);
}

#[test]
fn b071__mm_sll_epi16() {
    shifts!(_mm_sll_epi16, 71);
}

#[test]
fn b072__mm_sll_epi64() {
    shifts!(_mm_sll_epi64, 72);
}

#[test]
fn b073__mm_sra_epi16() {
    shifts!(_mm_sra_epi16, 73);
}

#[test]
fn b074__mm_sra_epi32() {
    shifts!(_mm_sra_epi32, 74);
}

#[test]
fn b075__mm_srl_epi64() {
    shifts!(_mm_srl_epi64, 75);
}

#[test]
fn b076__mm_storeu_si16() {
    cases!(_mm_storeu_si16, "sse2", 76, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add(case % 16).cast();
        let a = sample(rng, case, 1);
        real::_mm_storeu_si16(ptr, a);
        d.add(&dst.0);
    });
}

#[test]
fn b077__mm_storeu_si32() {
    cases!(_mm_storeu_si32, "sse2", 77, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add(case % 16).cast();
        let a = sample(rng, case, 1);
        real::_mm_storeu_si32(ptr, a);
        d.add(&dst.0);
    });
}

#[test]
fn b078__mm_storeu_si64() {
    cases!(_mm_storeu_si64, "sse2", 78, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add(case % 16).cast();
        let a = sample(rng, case, 1);
        real::_mm_storeu_si64(ptr, a);
        d.add(&dst.0);
    });
}

#[test]
fn b079__mm_stream_si128() {
    cases!(_mm_stream_si128, "sse2", 79, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add((case % 4) * 16).cast();
        let a = sample(rng, case, 1);
        real::_mm_stream_si128(ptr, a);
        real::_mm_sfence();
        d.add(&dst.0);
    });
}

#[test]
fn b080__mm_stream_si32() {
    cases!(_mm_stream_si32, "sse2", 80, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add((case % 4) * 4).cast();
        let a = sample(rng, case, 1);
        real::_mm_stream_si32(ptr, a);
        real::_mm_sfence();
        d.add(&dst.0);
    });
}

#[test]
fn b081__mm_stream_si64() {
    cases!(_mm_stream_si64, "sse2", 81, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add((case % 4) * 8).cast();
        let a = sample(rng, case, 1);
        real::_mm_stream_si64(ptr, a);
        real::_mm_sfence();
        d.add(&dst.0);
    });
}

// SSE3

#[test]
fn b082__mm_addsub_ps() {
    sampled!(_mm_addsub_ps, "sse3", 82, CASES, sample, [0, 1]);
}

#[test]
fn b083__mm_hadd_ps() {
    sampled!(_mm_hadd_ps, "sse3", 83, CASES, sample, [0, 1]);
}

#[test]
fn b084__mm_hsub_ps() {
    sampled!(_mm_hsub_ps, "sse3", 84, CASES, sample, [0, 1]);
}

#[test]
fn b085__mm_moveldup_ps() {
    sampled!(_mm_moveldup_ps, "sse3", 85, CASES, sample, [0]);
}

// SSSE3

#[test]
fn b086__mm_hadd_epi16() {
    sampled!(_mm_hadd_epi16, "ssse3", 86, CASES, sample, [0, 1]);
}

#[test]
fn b087__mm_hadd_epi32() {
    sampled!(_mm_hadd_epi32, "ssse3", 87, CASES, sample, [0, 1]);
}

#[test]
fn b088__mm_hadds_epi16() {
    sampled!(_mm_hadds_epi16, "ssse3", 88, CASES, sample, [0, 1]);
}

#[test]
fn b089__mm_hsub_epi16() {
    sampled!(_mm_hsub_epi16, "ssse3", 89, CASES, sample, [0, 1]);
}

#[test]
fn b090__mm_hsub_epi32() {
    sampled!(_mm_hsub_epi32, "ssse3", 90, CASES, sample, [0, 1]);
}

#[test]
fn b091__mm_hsubs_epi16() {
    sampled!(_mm_hsubs_epi16, "ssse3", 91, CASES, sample, [0, 1]);
}

#[test]
fn b092__mm_mulhrs_epi16() {
    sampled!(_mm_mulhrs_epi16, "ssse3", 92, CASES, sample, [0, 1]);
}

#[test]
fn b093__mm_sign_epi16() {
    sampled!(_mm_sign_epi16, "ssse3", 93, CASES, sample, [0, 1]);
}

#[test]
fn b094__mm_sign_epi32() {
    sampled!(_mm_sign_epi32, "ssse3", 94, CASES, sample, [0, 1]);
}

#[test]
fn b095__mm_sign_epi8() {
    sampled!(_mm_sign_epi8, "ssse3", 95, CASES, sample, [0, 1]);
}

// SSE4.1

#[test]
fn b096__mm_blend_ps() {
    immediates!(4, _mm_blend_ps, "sse4.1", 96, CASES, sample,
        [a: real::__m128 = 0, b: real::__m128 = 1]);
}

#[test]
fn b097__mm_ceil_ss() {
    sampled!(_mm_ceil_ss, "sse4.1", 97, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b098__mm_cvtepi16_epi32() {
    sampled!(_mm_cvtepi16_epi32, "sse4.1", 98, CASES, sample, [0]);
}

#[test]
fn b099__mm_cvtepi16_epi64() {
    sampled!(_mm_cvtepi16_epi64, "sse4.1", 99, CASES, sample, [0]);
}

#[test]
fn b100__mm_cvtepi32_epi64() {
    sampled!(_mm_cvtepi32_epi64, "sse4.1", 100, CASES, sample, [0]);
}

#[test]
fn b101__mm_cvtepi8_epi16() {
    sampled!(_mm_cvtepi8_epi16, "sse4.1", 101, CASES, sample, [0]);
}

#[test]
fn b102__mm_cvtepi8_epi32() {
    sampled!(_mm_cvtepi8_epi32, "sse4.1", 102, CASES, sample, [0]);
}

#[test]
fn b103__mm_cvtepi8_epi64() {
    sampled!(_mm_cvtepi8_epi64, "sse4.1", 103, CASES, sample, [0]);
}

#[test]
fn b104__mm_cvtepu16_epi32() {
    sampled!(_mm_cvtepu16_epi32, "sse4.1", 104, CASES, sample, [0]);
}

#[test]
fn b105__mm_cvtepu16_epi64() {
    sampled!(_mm_cvtepu16_epi64, "sse4.1", 105, CASES, sample, [0]);
}

#[test]
fn b106__mm_cvtepu32_epi64() {
    sampled!(_mm_cvtepu32_epi64, "sse4.1", 106, CASES, sample, [0]);
}

#[test]
fn b107__mm_cvtepu8_epi32() {
    sampled!(_mm_cvtepu8_epi32, "sse4.1", 107, CASES, sample, [0]);
}

#[test]
fn b108__mm_cvtepu8_epi64() {
    sampled!(_mm_cvtepu8_epi64, "sse4.1", 108, CASES, sample, [0]);
}

#[test]
fn b109__mm_dp_ps() {
    immediates!(8, _mm_dp_ps, "sse4.1", 109, CASES, sample,
        [a: real::__m128 = 0, b: real::__m128 = 1]);
}

#[test]
fn b110__mm_extract_epi8() {
    immediates!(4, _mm_extract_epi8, "sse4.1", 110, CASES, sample,
        [a: real::__m128i = 0]);
}

#[test]
fn b111__mm_extract_ps() {
    immediates!(2, _mm_extract_ps, "sse4.1", 111, CASES, sample,
        [a: real::__m128 = 0]);
}

#[test]
fn b112__mm_floor_ss() {
    sampled!(_mm_floor_ss, "sse4.1", 112, FLOAT_CASES, floats4, [0, 1]);
}

#[test]
fn b113__mm_insert_epi8() {
    immediates!(4, _mm_insert_epi8, "sse4.1", 113, CASES, sample,
        [a: real::__m128i = 0, i: i32 = 1]);
}

#[test]
fn b114__mm_insert_ps() {
    immediates!(8, _mm_insert_ps, "sse4.1", 114, CASES, sample,
        [a: real::__m128 = 0, b: real::__m128 = 1]);
}

#[test]
fn b115__mm_minpos_epu16() {
    sampled!(_mm_minpos_epu16, "sse4.1", 115, CASES, sample, [0]);
}

#[test]
fn b116__mm_mpsadbw_epu8() {
    immediates!(3, _mm_mpsadbw_epu8, "sse4.1", 116, CASES, sample,
        [a: real::__m128i = 0, b: real::__m128i = 1]);
}

#[test]
fn b117__mm_mul_epi32() {
    sampled!(_mm_mul_epi32, "sse4.1", 117, CASES, sample, [0, 1]);
}

#[test]
fn b118__mm_round_ss() {
    immediates!(4, _mm_round_ss, "sse4.1", 118, FLOAT_CASES, floats4,
        [a: real::__m128 = 0, b: real::__m128 = 1]);
}

#[test]
fn b119__mm_stream_load_si128() {
    cases!(_mm_stream_load_si128, "sse4.1", 119, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add((case % 4) * 16).cast();
        d.add(bytes(&real::_mm_stream_load_si128(ptr)));
    });
}

#[test]
fn b120__mm_test_all_ones() {
    sampled!(_mm_test_all_ones, "sse4.1", 120, CASES, sample, [0]);
}

#[test]
fn b121__mm_test_all_zeros() {
    bit_tests!(_mm_test_all_zeros, "sse4.1", 121, 4);
}

#[test]
fn b122__mm_test_mix_ones_zeros() {
    bit_tests!(_mm_test_mix_ones_zeros, "sse4.1", 122, 4);
}

#[test]
fn b123__mm_testc_si128() {
    bit_tests!(_mm_testc_si128, "sse4.1", 123, 4);
}

#[test]
fn b124__mm_testnzc_si128() {
    bit_tests!(_mm_testnzc_si128, "sse4.1", 124, 4);
}

// FMA

#[test]
fn b125__mm_fmadd_ss() {
    sampled!(_mm_fmadd_ss, "fma", 125, FMA_CASES, fma_args, [0, 1, 2]);
}

#[test]
fn b126__mm_fmaddsub_ps() {
    sampled!(_mm_fmaddsub_ps, "fma", 126, FMA_CASES, fma_args, [0, 1, 2]);
}

#[test]
fn b127__mm_fmsub_ss() {
    sampled!(_mm_fmsub_ss, "fma", 127, FMA_CASES, fma_args, [0, 1, 2]);
}

#[test]
fn b128__mm_fmsubadd_ps() {
    sampled!(_mm_fmsubadd_ps, "fma", 128, FMA_CASES, fma_args, [0, 1, 2]);
}

#[test]
fn b129__mm_fnmadd_ss() {
    sampled!(_mm_fnmadd_ss, "fma", 129, FMA_CASES, fma_args, [0, 1, 2]);
}

#[test]
fn b130__mm_fnmsub_ss() {
    sampled!(_mm_fnmsub_ss, "fma", 130, FMA_CASES, fma_args, [0, 1, 2]);
}

#[test]
fn b131__mm256_fmaddsub_ps() {
    sampled!(_mm256_fmaddsub_ps, "fma", 131, FMA_CASES, fma_args, [0, 1, 2]);
}

#[test]
fn b132__mm256_fmsubadd_ps() {
    sampled!(_mm256_fmsubadd_ps, "fma", 132, FMA_CASES, fma_args, [0, 1, 2]);
}

// AVX

#[test]
fn b133__mm_broadcast_ss() {
    cases!(_mm_broadcast_ss, "avx", 133, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add((case % 4) * 4).cast();
        d.add(bytes(&real::_mm_broadcast_ss(&*ptr)));
    });
}

#[test]
fn b134__mm_cmp_ss() {
    immediates!(5, _mm_cmp_ss, "avx", 134, FLOAT_CASES, floats4,
        [a: real::__m128 = 0, b: real::__m128 = 1]);
}

#[test]
fn b135__mm_maskload_ps() {
    cases!(_mm_maskload_ps, "avx", 135, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add(case % 16).cast();
        let mask = sample(rng, case, 1);
        d.add(bytes(&real::_mm_maskload_ps(ptr, mask)));
    });
}

#[test]
fn b136__mm_maskstore_ps() {
    cases!(_mm_maskstore_ps, "avx", 136, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add(case % 16).cast();
        let mask = sample(rng, case, 1);
        let a = sample(rng, case, 2);
        real::_mm_maskstore_ps(ptr, mask, a);
        d.add(&dst.0);
    });
}

#[test]
fn b137__mm_permute_ps() {
    immediates!(8, _mm_permute_ps, "avx", 137, CASES, sample,
        [a: real::__m128 = 0]);
}

#[test]
fn b138__mm_permutevar_ps() {
    sampled!(_mm_permutevar_ps, "avx", 138, CASES, sample, [0, 1]);
}

#[test]
fn b139__mm_testc_ps() {
    bit_tests!(_mm_testc_ps, "avx", 139, 4);
}

#[test]
fn b140__mm_testnzc_ps() {
    bit_tests!(_mm_testnzc_ps, "avx", 140, 4);
}

#[test]
fn b141__mm_testz_ps() {
    bit_tests!(_mm_testz_ps, "avx", 141, 4);
}

#[test]
fn b142__mm256_addsub_ps() {
    sampled!(_mm256_addsub_ps, "avx", 142, CASES, sample, [0, 1]);
}

#[test]
fn b143__mm256_blend_ps() {
    immediates!(8, _mm256_blend_ps, "avx", 143, CASES, sample,
        [a: real::__m256 = 0, b: real::__m256 = 1]);
}

#[test]
fn b144__mm256_broadcast_ps() {
    cases!(_mm256_broadcast_ps, "avx", 144, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add((case % 4) * 16).cast();
        d.add(bytes(&real::_mm256_broadcast_ps(&*ptr)));
    });
}

#[test]
fn b145__mm256_broadcast_ss() {
    cases!(_mm256_broadcast_ss, "avx", 145, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add((case % 4) * 4).cast();
        d.add(bytes(&real::_mm256_broadcast_ss(&*ptr)));
    });
}

#[test]
fn b146__mm256_castps128_ps256() {
    cases!(_mm256_castps128_ps256, "avx", 146, CASES, "none", |rng, case, d| {
        let a = sample(rng, case, 0);
        let result = real::_mm256_castps128_ps256(a);
        // Only the low four lanes have specified values.
        d.add(&bytes(&result)[..16]);
    });
}

#[test]
fn b147__mm256_cvtps_epi32() {
    sampled!(_mm256_cvtps_epi32, "avx", 147, FLOAT_CASES, floats8, [0]);
}

#[test]
fn b148__mm256_cvtsd_f64() {
    sampled!(_mm256_cvtsd_f64, "avx", 148, CASES, sample, [0]);
}

#[test]
fn b149__mm256_cvtsi256_si32() {
    sampled!(_mm256_cvtsi256_si32, "avx", 149, CASES, sample, [0]);
}

#[test]
fn b150__mm256_cvtss_f32() {
    sampled!(_mm256_cvtss_f32, "avx", 150, CASES, sample, [0]);
}

#[test]
fn b151__mm256_dp_ps() {
    immediates!(8, _mm256_dp_ps, "avx", 151, CASES, sample,
        [a: real::__m256 = 0, b: real::__m256 = 1]);
}

#[test]
fn b152__mm256_hadd_ps() {
    sampled!(_mm256_hadd_ps, "avx", 152, CASES, sample, [0, 1]);
}

#[test]
fn b153__mm256_hsub_ps() {
    sampled!(_mm256_hsub_ps, "avx", 153, CASES, sample, [0, 1]);
}

#[test]
fn b154__mm256_insert_epi16() {
    immediates!(4, _mm256_insert_epi16, "avx", 154, CASES, sample,
        [a: real::__m256i = 0, i: i16 = 1]);
}

#[test]
fn b155__mm256_insert_epi32() {
    immediates!(3, _mm256_insert_epi32, "avx", 155, CASES, sample,
        [a: real::__m256i = 0, i: i32 = 1]);
}

#[test]
fn b156__mm256_insert_epi64() {
    immediates!(2, _mm256_insert_epi64, "avx", 156, CASES, sample,
        [a: real::__m256i = 0, i: i64 = 1]);
}

#[test]
fn b157__mm256_insert_epi8() {
    immediates!(5, _mm256_insert_epi8, "avx", 157, CASES, sample,
        [a: real::__m256i = 0, i: i8 = 1]);
}

#[test]
fn b158__mm256_insertf128_ps() {
    immediates!(1, _mm256_insertf128_ps, "avx", 158, CASES, sample,
        [a: real::__m256 = 0, b: real::__m128 = 1]);
}

#[test]
fn b159__mm256_loadu2_m128() {
    cases!(_mm256_loadu2_m128, "avx", 159, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let base = src.0.as_ptr();
        let hi = base.add(96 + case % 16).cast();
        let lo = base.add((case + 5) % 16).cast();
        d.add(bytes(&real::_mm256_loadu2_m128(hi, lo)));
    });
}

#[test]
fn b160__mm256_loadu2_m128d() {
    cases!(_mm256_loadu2_m128d, "avx", 160, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let base = src.0.as_ptr();
        let hi = base.add(96 + case % 16).cast();
        let lo = base.add((case + 5) % 16).cast();
        d.add(bytes(&real::_mm256_loadu2_m128d(hi, lo)));
    });
}

#[test]
fn b161__mm256_loadu2_m128i() {
    cases!(_mm256_loadu2_m128i, "avx", 161, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let base = src.0.as_ptr();
        let hi = base.add(96 + case % 16).cast();
        let lo = base.add((case + 5) % 16).cast();
        d.add(bytes(&real::_mm256_loadu2_m128i(hi, lo)));
    });
}

#[test]
fn b162__mm256_maskload_ps() {
    cases!(_mm256_maskload_ps, "avx", 162, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add(case % 16).cast();
        let mask = sample(rng, case, 1);
        d.add(bytes(&real::_mm256_maskload_ps(ptr, mask)));
    });
}

#[test]
fn b163__mm256_maskstore_ps() {
    cases!(_mm256_maskstore_ps, "avx", 163, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add(case % 16).cast();
        let mask = sample(rng, case, 1);
        let a = sample(rng, case, 2);
        real::_mm256_maskstore_ps(ptr, mask, a);
        d.add(&dst.0);
    });
}

#[test]
fn b164__mm256_movehdup_ps() {
    sampled!(_mm256_movehdup_ps, "avx", 164, CASES, sample, [0]);
}

#[test]
fn b165__mm256_moveldup_ps() {
    sampled!(_mm256_moveldup_ps, "avx", 165, CASES, sample, [0]);
}

#[test]
fn b166__mm256_permute_ps() {
    immediates!(8, _mm256_permute_ps, "avx", 166, CASES, sample,
        [a: real::__m256 = 0]);
}

#[test]
fn b167__mm256_permute2f128_si256() {
    immediates!(8, _mm256_permute2f128_si256, "avx", 167, CASES, sample,
        [a: real::__m256i = 0, b: real::__m256i = 1]);
}

#[test]
fn b168__mm256_permutevar_ps() {
    sampled!(_mm256_permutevar_ps, "avx", 168, CASES, sample, [0, 1]);
}

#[test]
fn b169__mm256_rcp_ps() {
    cases!(_mm256_rcp_ps, "avx", 169, FLOAT_CASES, "none", |rng, case, d| {
        let input = float_lanes::<8>(rng, case, 0);
        let output: [f32; 8] = cast(real::_mm256_rcp_ps(cast(input)));
        for i in 0..8 {
            add_estimate(d, input[i], output[i], false);
        }
    });
}

#[test]
fn b170__mm256_rsqrt_ps() {
    cases!(_mm256_rsqrt_ps, "avx", 170, FLOAT_CASES, "none", |rng, case, d| {
        let input = float_lanes::<8>(rng, case, 0);
        let output: [f32; 8] = cast(real::_mm256_rsqrt_ps(cast(input)));
        for i in 0..8 {
            add_estimate(d, input[i], output[i], true);
        }
    });
}

#[test]
fn b171__mm256_set_m128() {
    sampled!(_mm256_set_m128, "avx", 171, CASES, sample, [0, 1]);
}

#[test]
fn b172__mm256_set_m128d() {
    sampled!(_mm256_set_m128d, "avx", 172, CASES, sample, [0, 1]);
}

#[test]
fn b173__mm256_setr_epi16() {
    sampled!(_mm256_setr_epi16, "avx", 173, CASES, sample,
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]);
}

#[test]
fn b174__mm256_setr_m128d() {
    sampled!(_mm256_setr_m128d, "avx", 174, CASES, sample, [0, 1]);
}

#[test]
fn b175__mm256_store_si256() {
    cases!(_mm256_store_si256, "avx", 175, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add((case % 4) * 32).cast();
        let a = sample(rng, case, 1);
        real::_mm256_store_si256(ptr, a);
        d.add(&dst.0);
    });
}

#[test]
fn b176__mm256_storeu2_m128() {
    cases!(_mm256_storeu2_m128, "avx", 176, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let base = dst.0.as_mut_ptr();
        let hi = base.add(96 + case % 16).cast();
        let lo = base.add((case + 5) % 16).cast();
        let a = sample(rng, case, 2);
        real::_mm256_storeu2_m128(hi, lo, a);
        d.add(&dst.0);
    });
}

#[test]
fn b177__mm256_storeu2_m128d() {
    cases!(_mm256_storeu2_m128d, "avx", 177, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let base = dst.0.as_mut_ptr();
        let hi = base.add(96 + case % 16).cast();
        let lo = base.add((case + 5) % 16).cast();
        let a = sample(rng, case, 2);
        real::_mm256_storeu2_m128d(hi, lo, a);
        d.add(&dst.0);
    });
}

#[test]
fn b178__mm256_storeu2_m128i() {
    cases!(_mm256_storeu2_m128i, "avx", 178, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let base = dst.0.as_mut_ptr();
        let hi = base.add(96 + case % 16).cast();
        let lo = base.add((case + 5) % 16).cast();
        let a = sample(rng, case, 2);
        real::_mm256_storeu2_m128i(hi, lo, a);
        d.add(&dst.0);
    });
}

#[test]
fn b179__mm256_stream_ps() {
    cases!(_mm256_stream_ps, "avx", 179, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add((case % 4) * 32).cast();
        let a = sample(rng, case, 1);
        real::_mm256_stream_ps(ptr, a);
        real::_mm_sfence();
        d.add(&dst.0);
    });
}

#[test]
fn b180__mm256_stream_si256() {
    cases!(_mm256_stream_si256, "avx", 180, CASES, "none", |rng, case, d| {
        let mut dst = Buffer([0; 192]);
        let ptr = dst.0.as_mut_ptr().add((case % 4) * 32).cast();
        let a = sample(rng, case, 1);
        real::_mm256_stream_si256(ptr, a);
        real::_mm_sfence();
        d.add(&dst.0);
    });
}

#[test]
fn b181__mm256_testc_ps() {
    bit_tests!(_mm256_testc_ps, "avx", 181, 8);
}

#[test]
fn b182__mm256_testc_si256() {
    bit_tests!(_mm256_testc_si256, "avx", 182, 8);
}

#[test]
fn b183__mm256_testnzc_ps() {
    bit_tests!(_mm256_testnzc_ps, "avx", 183, 8);
}

#[test]
fn b184__mm256_testnzc_si256() {
    bit_tests!(_mm256_testnzc_si256, "avx", 184, 8);
}

#[test]
fn b185__mm256_testz_ps() {
    bit_tests!(_mm256_testz_ps, "avx", 185, 8);
}

#[test]
fn b186__mm256_undefined_ps() {
    require_feature!(_mm256_undefined_ps, "avx");
    let _ = unsafe { real::_mm256_undefined_ps() };
    Digest::new().line("_mm256_undefined_ps", "none");
}

#[test]
fn b187__mm256_undefined_si256() {
    require_feature!(_mm256_undefined_si256, "avx");
    let _ = unsafe { real::_mm256_undefined_si256() };
    Digest::new().line("_mm256_undefined_si256", "none");
}

#[test]
fn b188__mm256_zextps128_ps256() {
    sampled!(_mm256_zextps128_ps256, "avx", 188, CASES, sample, [0]);
}

#[test]
fn b189__mm_rcp_ps() {
    cases!(_mm_rcp_ps, "sse", 189, FLOAT_CASES, "none", |rng, case, d| {
        let input = float_lanes::<4>(rng, case, 0);
        let output: [f32; 4] = cast(real::_mm_rcp_ps(cast(input)));
        for i in 0..4 {
            add_estimate(d, input[i], output[i], false);
        }
    });
}

#[test]
fn b190__mm_rsqrt_ps() {
    cases!(_mm_rsqrt_ps, "sse", 190, FLOAT_CASES, "none", |rng, case, d| {
        let input = float_lanes::<4>(rng, case, 0);
        let output: [f32; 4] = cast(real::_mm_rsqrt_ps(cast(input)));
        for i in 0..4 {
            add_estimate(d, input[i], output[i], true);
        }
    });
}

// AVX2

#[test]
fn b191__mm_i32gather_epi32() {
    gathers!(_mm_i32gather_epi32, 191, [i32; 4]);
}

#[test]
fn b192__mm_i32gather_epi64() {
    gathers!(_mm_i32gather_epi64, 192, [i32; 4]);
}

#[test]
fn b193__mm_i32gather_ps() {
    gathers!(_mm_i32gather_ps, 193, [i32; 4]);
}

#[test]
fn b194__mm_i64gather_epi32() {
    gathers!(_mm_i64gather_epi32, 194, [i64; 2]);
}

#[test]
fn b195__mm_i64gather_epi64() {
    gathers!(_mm_i64gather_epi64, 195, [i64; 2]);
}

#[test]
fn b196__mm_i64gather_ps() {
    gathers!(_mm_i64gather_ps, 196, [i64; 2]);
}

#[test]
fn b197__mm_mask_i32gather_epi32() {
    gathers!(_mm_mask_i32gather_epi32, 197, [i32; 4], src, mask);
}

#[test]
fn b198__mm_mask_i32gather_epi64() {
    gathers!(_mm_mask_i32gather_epi64, 198, [i32; 4], src, mask);
}

#[test]
fn b199__mm_mask_i32gather_ps() {
    gathers!(_mm_mask_i32gather_ps, 199, [i32; 4], src, mask);
}

#[test]
fn b200__mm_mask_i64gather_epi32() {
    gathers!(_mm_mask_i64gather_epi32, 200, [i64; 2], src, mask);
}

#[test]
fn b201__mm_mask_i64gather_epi64() {
    gathers!(_mm_mask_i64gather_epi64, 201, [i64; 2], src, mask);
}

#[test]
fn b202__mm_mask_i64gather_ps() {
    gathers!(_mm_mask_i64gather_ps, 202, [i64; 2], src, mask);
}

#[test]
fn b203__mm256_i32gather_epi32() {
    gathers!(_mm256_i32gather_epi32, 203, [i32; 8]);
}

#[test]
fn b204__mm256_i32gather_epi64() {
    gathers!(_mm256_i32gather_epi64, 204, [i32; 4]);
}

#[test]
fn b205__mm256_i32gather_ps() {
    gathers!(_mm256_i32gather_ps, 205, [i32; 8]);
}

#[test]
fn b206__mm256_i64gather_epi32() {
    gathers!(_mm256_i64gather_epi32, 206, [i64; 4]);
}

#[test]
fn b207__mm256_i64gather_epi64() {
    gathers!(_mm256_i64gather_epi64, 207, [i64; 4]);
}

#[test]
fn b208__mm256_i64gather_ps() {
    gathers!(_mm256_i64gather_ps, 208, [i64; 4]);
}

#[test]
fn b209__mm256_mask_i32gather_epi32() {
    gathers!(_mm256_mask_i32gather_epi32, 209, [i32; 8], src, mask);
}

#[test]
fn b210__mm256_mask_i32gather_epi64() {
    gathers!(_mm256_mask_i32gather_epi64, 210, [i32; 4], src, mask);
}

#[test]
fn b211__mm256_mask_i32gather_ps() {
    gathers!(_mm256_mask_i32gather_ps, 211, [i32; 8], src, mask);
}

#[test]
fn b212__mm256_mask_i64gather_epi32() {
    gathers!(_mm256_mask_i64gather_epi32, 212, [i64; 4], src, mask);
}

#[test]
fn b213__mm256_mask_i64gather_epi64() {
    gathers!(_mm256_mask_i64gather_epi64, 213, [i64; 4], src, mask);
}

#[test]
fn b214__mm256_mask_i64gather_ps() {
    gathers!(_mm256_mask_i64gather_ps, 214, [i64; 4], src, mask);
}

#[test]
fn b215__mm_maskload_epi32() {
    masked_loads!(_mm_maskload_epi32, 215);
}

#[test]
fn b216__mm_maskload_epi64() {
    masked_loads!(_mm_maskload_epi64, 216);
}

#[test]
fn b217__mm_maskstore_epi32() {
    masked_stores!(_mm_maskstore_epi32, 217);
}

#[test]
fn b218__mm_maskstore_epi64() {
    masked_stores!(_mm_maskstore_epi64, 218);
}

#[test]
fn b219__mm256_maskload_epi32() {
    masked_loads!(_mm256_maskload_epi32, 219);
}

#[test]
fn b220__mm256_maskload_epi64() {
    masked_loads!(_mm256_maskload_epi64, 220);
}

#[test]
fn b221__mm256_maskstore_epi32() {
    masked_stores!(_mm256_maskstore_epi32, 221);
}

#[test]
fn b222__mm256_maskstore_epi64() {
    masked_stores!(_mm256_maskstore_epi64, 222);
}

#[test]
fn b223__mm_blend_epi32() {
    immediates!(4, _mm_blend_epi32, "avx2", 223, CASES, sample,
        [a: real::__m128i = 0, b: real::__m128i = 1]);
}

#[test]
fn b224__mm_broadcastb_epi8() {
    sampled!(_mm_broadcastb_epi8, "avx2", 224, CASES, sample, [0]);
}

#[test]
fn b225__mm_broadcastd_epi32() {
    sampled!(_mm_broadcastd_epi32, "avx2", 225, CASES, sample, [0]);
}

#[test]
fn b226__mm_broadcastq_epi64() {
    sampled!(_mm_broadcastq_epi64, "avx2", 226, CASES, sample, [0]);
}

#[test]
fn b227__mm_broadcastsi128_si256() {
    sampled!(_mm_broadcastsi128_si256, "avx2", 227, CASES, sample, [0]);
}

#[test]
fn b228__mm_broadcastss_ps() {
    sampled!(_mm_broadcastss_ps, "avx2", 228, CASES, sample, [0]);
}

#[test]
fn b229__mm_broadcastw_epi16() {
    sampled!(_mm_broadcastw_epi16, "avx2", 229, CASES, sample, [0]);
}

#[test]
fn b230__mm_sllv_epi64() {
    lane_shifts!(_mm_sllv_epi64, 230, [u64; 2]);
}

#[test]
fn b231__mm_srav_epi32() {
    lane_shifts!(_mm_srav_epi32, 231, [u32; 4]);
}

#[test]
fn b232__mm_srlv_epi32() {
    lane_shifts!(_mm_srlv_epi32, 232, [u32; 4]);
}

#[test]
fn b233__mm_srlv_epi64() {
    lane_shifts!(_mm_srlv_epi64, 233, [u64; 2]);
}

#[test]
fn b234__mm256_abs_epi16() {
    sampled!(_mm256_abs_epi16, "avx2", 234, CASES, sample, [0]);
}

#[test]
fn b235__mm256_abs_epi32() {
    sampled!(_mm256_abs_epi32, "avx2", 235, CASES, sample, [0]);
}

#[test]
fn b236__mm256_abs_epi8() {
    sampled!(_mm256_abs_epi8, "avx2", 236, CASES, sample, [0]);
}

#[test]
fn b237__mm256_adds_epi16() {
    sampled!(_mm256_adds_epi16, "avx2", 237, CASES, sample, [0, 1]);
}

#[test]
fn b238__mm256_adds_epi8() {
    sampled!(_mm256_adds_epi8, "avx2", 238, CASES, sample, [0, 1]);
}

#[test]
fn b239__mm256_adds_epu16() {
    sampled!(_mm256_adds_epu16, "avx2", 239, CASES, sample, [0, 1]);
}

#[test]
fn b240__mm256_adds_epu8() {
    sampled!(_mm256_adds_epu8, "avx2", 240, CASES, sample, [0, 1]);
}

#[test]
fn b241__mm256_avg_epu16() {
    sampled!(_mm256_avg_epu16, "avx2", 241, CASES, sample, [0, 1]);
}

#[test]
fn b242__mm256_avg_epu8() {
    sampled!(_mm256_avg_epu8, "avx2", 242, CASES, sample, [0, 1]);
}

#[test]
fn b243__mm256_blend_epi16() {
    immediates!(8, _mm256_blend_epi16, "avx2", 243, CASES, sample,
        [a: real::__m256i = 0, b: real::__m256i = 1]);
}

#[test]
fn b244__mm256_broadcastb_epi8() {
    sampled!(_mm256_broadcastb_epi8, "avx2", 244, CASES, sample, [0]);
}

#[test]
fn b245__mm256_broadcastq_epi64() {
    sampled!(_mm256_broadcastq_epi64, "avx2", 245, CASES, sample, [0]);
}

#[test]
fn b246__mm256_broadcastss_ps() {
    sampled!(_mm256_broadcastss_ps, "avx2", 246, CASES, sample, [0]);
}

#[test]
fn b247__mm256_broadcastw_epi16() {
    sampled!(_mm256_broadcastw_epi16, "avx2", 247, CASES, sample, [0]);
}

#[test]
fn b248__mm256_bslli_epi128() {
    immediates!(8, _mm256_bslli_epi128, "avx2", 248, CASES, sample,
        [a: real::__m256i = 0]);
}

#[test]
fn b249__mm256_bsrli_epi128() {
    immediates!(8, _mm256_bsrli_epi128, "avx2", 249, CASES, sample,
        [a: real::__m256i = 0]);
}

#[test]
fn b250__mm256_cmpeq_epi64() {
    sampled!(_mm256_cmpeq_epi64, "avx2", 250, CASES, sample, [0, 1]);
}

#[test]
fn b251__mm256_cmpgt_epi64() {
    sampled!(_mm256_cmpgt_epi64, "avx2", 251, CASES, sample, [0, 1]);
}

#[test]
fn b252__mm256_cvtepi16_epi32() {
    sampled!(_mm256_cvtepi16_epi32, "avx2", 252, CASES, sample, [0]);
}

#[test]
fn b253__mm256_cvtepi16_epi64() {
    sampled!(_mm256_cvtepi16_epi64, "avx2", 253, CASES, sample, [0]);
}

#[test]
fn b254__mm256_cvtepi32_epi64() {
    sampled!(_mm256_cvtepi32_epi64, "avx2", 254, CASES, sample, [0]);
}

#[test]
fn b255__mm256_cvtepi8_epi16() {
    sampled!(_mm256_cvtepi8_epi16, "avx2", 255, CASES, sample, [0]);
}

#[test]
fn b256__mm256_cvtepi8_epi64() {
    sampled!(_mm256_cvtepi8_epi64, "avx2", 256, CASES, sample, [0]);
}

#[test]
fn b257__mm256_cvtepu16_epi32() {
    sampled!(_mm256_cvtepu16_epi32, "avx2", 257, CASES, sample, [0]);
}

#[test]
fn b258__mm256_cvtepu16_epi64() {
    sampled!(_mm256_cvtepu16_epi64, "avx2", 258, CASES, sample, [0]);
}

#[test]
fn b259__mm256_cvtepu32_epi64() {
    sampled!(_mm256_cvtepu32_epi64, "avx2", 259, CASES, sample, [0]);
}

#[test]
fn b260__mm256_cvtepu8_epi32() {
    sampled!(_mm256_cvtepu8_epi32, "avx2", 260, CASES, sample, [0]);
}

#[test]
fn b261__mm256_cvtepu8_epi64() {
    sampled!(_mm256_cvtepu8_epi64, "avx2", 261, CASES, sample, [0]);
}

#[test]
fn b262__mm256_extract_epi16() {
    immediates!(4, _mm256_extract_epi16, "avx2", 262, CASES, sample,
        [a: real::__m256i = 0]);
}

#[test]
fn b263__mm256_extract_epi8() {
    immediates!(5, _mm256_extract_epi8, "avx2", 263, CASES, sample,
        [a: real::__m256i = 0]);
}

#[test]
fn b264__mm256_hadd_epi16() {
    sampled!(_mm256_hadd_epi16, "avx2", 264, CASES, sample, [0, 1]);
}

#[test]
fn b265__mm256_hadd_epi32() {
    sampled!(_mm256_hadd_epi32, "avx2", 265, CASES, sample, [0, 1]);
}

#[test]
fn b266__mm256_hadds_epi16() {
    sampled!(_mm256_hadds_epi16, "avx2", 266, CASES, sample, [0, 1]);
}

#[test]
fn b267__mm256_hsub_epi16() {
    sampled!(_mm256_hsub_epi16, "avx2", 267, CASES, sample, [0, 1]);
}

#[test]
fn b268__mm256_hsub_epi32() {
    sampled!(_mm256_hsub_epi32, "avx2", 268, CASES, sample, [0, 1]);
}

#[test]
fn b269__mm256_hsubs_epi16() {
    sampled!(_mm256_hsubs_epi16, "avx2", 269, CASES, sample, [0, 1]);
}

#[test]
fn b270__mm256_max_epi32() {
    sampled!(_mm256_max_epi32, "avx2", 270, CASES, sample, [0, 1]);
}

#[test]
fn b271__mm256_max_epi8() {
    sampled!(_mm256_max_epi8, "avx2", 271, CASES, sample, [0, 1]);
}

#[test]
fn b272__mm256_min_epi32() {
    sampled!(_mm256_min_epi32, "avx2", 272, CASES, sample, [0, 1]);
}

#[test]
fn b273__mm256_min_epi8() {
    sampled!(_mm256_min_epi8, "avx2", 273, CASES, sample, [0, 1]);
}

#[test]
fn b274__mm256_mpsadbw_epu8() {
    immediates!(8, _mm256_mpsadbw_epu8, "avx2", 274, CASES, sample,
        [a: real::__m256i = 0, b: real::__m256i = 1]);
}

#[test]
fn b275__mm256_mul_epi32() {
    sampled!(_mm256_mul_epi32, "avx2", 275, CASES, sample, [0, 1]);
}

#[test]
fn b276__mm256_mulhi_epi16() {
    sampled!(_mm256_mulhi_epi16, "avx2", 276, CASES, sample, [0, 1]);
}

#[test]
fn b277__mm256_mulhrs_epi16() {
    sampled!(_mm256_mulhrs_epi16, "avx2", 277, CASES, sample, [0, 1]);
}

#[test]
fn b278__mm256_packs_epi16() {
    sampled!(_mm256_packs_epi16, "avx2", 278, CASES, sample, [0, 1]);
}

#[test]
fn b279__mm256_shufflehi_epi16() {
    immediates!(8, _mm256_shufflehi_epi16, "avx2", 279, CASES, sample,
        [a: real::__m256i = 0]);
}

#[test]
fn b280__mm256_shufflelo_epi16() {
    immediates!(8, _mm256_shufflelo_epi16, "avx2", 280, CASES, sample,
        [a: real::__m256i = 0]);
}

#[test]
fn b281__mm256_sign_epi32() {
    sampled!(_mm256_sign_epi32, "avx2", 281, CASES, sample, [0, 1]);
}

#[test]
fn b282__mm256_sign_epi8() {
    sampled!(_mm256_sign_epi8, "avx2", 282, CASES, sample, [0, 1]);
}

#[test]
fn b283__mm256_sll_epi16() {
    shifts!(_mm256_sll_epi16, "avx2", 283);
}

#[test]
fn b284__mm256_sll_epi32() {
    shifts!(_mm256_sll_epi32, "avx2", 284);
}

#[test]
fn b285__mm256_sll_epi64() {
    shifts!(_mm256_sll_epi64, "avx2", 285);
}

#[test]
fn b286__mm256_slli_si256() {
    immediates!(8, _mm256_slli_si256, "avx2", 286, CASES, sample,
        [a: real::__m256i = 0]);
}

#[test]
fn b287__mm256_sllv_epi64() {
    lane_shifts!(_mm256_sllv_epi64, 287, [u64; 4]);
}

#[test]
fn b288__mm256_sra_epi16() {
    shifts!(_mm256_sra_epi16, "avx2", 288);
}

#[test]
fn b289__mm256_sra_epi32() {
    shifts!(_mm256_sra_epi32, "avx2", 289);
}

#[test]
fn b290__mm256_srav_epi32() {
    lane_shifts!(_mm256_srav_epi32, 290, [u32; 8]);
}

#[test]
fn b291__mm256_srl_epi32() {
    shifts!(_mm256_srl_epi32, "avx2", 291);
}

#[test]
fn b292__mm256_srl_epi64() {
    shifts!(_mm256_srl_epi64, "avx2", 292);
}

#[test]
fn b293__mm256_srli_si256() {
    immediates!(8, _mm256_srli_si256, "avx2", 293, CASES, sample,
        [a: real::__m256i = 0]);
}

#[test]
fn b294__mm256_stream_load_si256() {
    cases!(_mm256_stream_load_si256, "avx2", 294, CASES, "none", |rng, case, d| {
        let src: Buffer = sample(rng, case, 0);
        let ptr = src.0.as_ptr().add((case % (src.0.len() / 32)) * 32).cast();
        d.add(bytes(&real::_mm256_stream_load_si256(ptr)));
    });
}

#[test]
fn b295__mm256_subs_epi16() {
    sampled!(_mm256_subs_epi16, "avx2", 295, CASES, sample, [0, 1]);
}

#[test]
fn b296__mm256_subs_epi8() {
    sampled!(_mm256_subs_epi8, "avx2", 296, CASES, sample, [0, 1]);
}

#[test]
fn b297__mm256_subs_epu16() {
    sampled!(_mm256_subs_epu16, "avx2", 297, CASES, sample, [0, 1]);
}
