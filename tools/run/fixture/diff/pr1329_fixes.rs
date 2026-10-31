// diff_fast corpus program: what PR #1329 adds that changes what a program computes or whether it
// compiles. `D <case> <value>` per case, equal to rustc's output.
// - typeck: `Type::<A>::f` fills trailing defaulted type parameters (struct and alias prefixes);
//   `a op= b` / `a op b` with `a` an unresolved type variable resolve through the operator traits;
// - std: shifts by every integer type, NonZero widening From + `u* / NonZero<u*>`, array <-> tuple
//   From, each_mut, Vec::push_mut, VecDeque push_*_mut / pop_*_if, iter::chain, slice::escape_ascii,
//   Layout extend_packed / repeat, NonNull with_addr / map_addr, f32/f64::to_int_unchecked,
//   Wrapping <<= / >>=, io::Error from TryReserveError;
// x86-64 only: the SSE/AVX/AVX2 float and integer intrinsics the PR added, on random bits (NaNs
// included), one hash each (not _mm_rcp_ps: its approximation differs between CPU vendors).
#![allow(unused_unsafe, dead_code, unused_mut, unused_variables, unreachable_code)]
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::hint::black_box;
use std::num::NonZero;

struct Pair<A, B = u16> { a: A, b: B }
impl<A: Default, B: Default + Copy + Into<u64>> Pair<A, B> {
    fn new() -> Self { Pair { a: A::default(), b: B::default() } }
    fn width() -> usize { std::mem::size_of::<B>() }
}
type Map<K, V = u8> = std::collections::BTreeMap<K, V>;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct V3 { x: f32, y: f32, z: f32 }
impl std::ops::Mul<f32> for V3 { type Output = V3; fn mul(self, s: f32) -> V3 { V3 { x: self.x * s, y: self.y * s, z: self.z * s } } }
impl std::ops::AddAssign for V3 { fn add_assign(&mut self, o: V3) { self.x += o.x; self.y += o.y; self.z += o.z; } }
impl std::ops::Add for V3 { type Output = V3; fn add(self, o: V3) -> V3 { V3 { x: self.x + o.x, y: self.y + o.y, z: self.z + o.z } } }

fn typeck() {
    let p = Pair::<u8>::new();
    d!("prefix_default {} {} {}", p.a, p.b, Pair::<u8>::width());
    let mut m = Map::<&str>::new();
    m.insert("k", 7);
    d!("alias_default {:?}", m);
    // the accumulator's type is an inference variable until the `+=` resolves it through AddAssign
    let mut acc = Default::default();
    for i in 0..black_box(4) { acc += V3 { x: 1.0, y: 2.0, z: 3.0 } * i as f32; }
    let acc: V3 = acc;
    d!("assign_op_infer {:?}", acc);
    let mut s = Default::default();
    s = s + V3 { x: 0.5, y: 0.25, z: 0.125 };
    let s: V3 = s;
    d!("binop_infer {:?}", s);
    let sh = black_box(3u32);
    d!("shift_lit {} {}", 1usize << (sh - 1), -(1i32).checked_shl(sh).unwrap());
}

fn shifts() {
    let a = black_box(0x8123_4567u32);
    let mut h = 0u64;
    h = h.wrapping_mul(31) ^ (a << 3u8) as u64;
    h = h.wrapping_mul(31) ^ (a >> 5u16) as u64;
    h = h.wrapping_mul(31) ^ (a << 7u64) as u64;
    h = h.wrapping_mul(31) ^ (a >> 9i8) as u64;
    h = h.wrapping_mul(31) ^ (a << 2i16) as u64;
    h = h.wrapping_mul(31) ^ (a >> 4u128) as u64;
    h = h.wrapping_mul(31) ^ (a << 6isize) as u64;
    let mut b = black_box(-12345i64);
    b <<= 3u8; b >>= 2u128; b <<= 1i16; b >>= 1isize;
    let mut c = black_box(200u8);
    c >>= 3i64; c <<= 1u16;
    d!("shifts {:x} {} {}", h, b, c);
    let mut w = std::num::Wrapping(black_box(0xF0u8));
    w <<= 2usize; let x = w; w >>= 1usize;
    d!("wrapping_shift {} {}", x, w);
}

fn nonzero() {
    let a = NonZero::new(black_box(200u8)).unwrap();
    let b: NonZero<u32> = a.into();
    let c: NonZero<u64> = NonZero::<u16>::new(black_box(999)).unwrap().into();
    let e: NonZero<i128> = NonZero::<i32>::new(black_box(-5)).unwrap().into();
    let f: NonZero<isize> = NonZero::<i16>::new(black_box(-300)).unwrap().into();
    let n = NonZero::new(black_box(7u32)).unwrap();
    let mut q = black_box(100u32);
    q /= n;
    let mut r = black_box(100u64);
    r %= NonZero::new(black_box(9u64)).unwrap();
    d!("nonzero {} {} {} {} {} {} {} {}", b, c, e, f, 100u32 / n, 100u32 % n, q, r);
}

fn arrays_and_collections() {
    let t: [u16; 3] = (1, 2, 3).into();
    let u: (char, char) = ['a', 'b'].into();
    let mut arr = [1, 2, 3, 4];
    for r in arr.each_mut() { *r *= 10; }
    d!("array_tuple {:?} {:?} {:?}", t, u, arr);
    let mut v = vec![1u8];
    *v.push_mut(5) += 1;
    d!("push_mut {:?}", v);
    let mut q: std::collections::VecDeque<i32> = (1..=5).collect();
    *q.push_back_mut(10) += 1;
    *q.push_front_mut(0) -= 1;
    let f = q.pop_front_if(|x| *x < 0);
    let g = q.pop_front_if(|x| *x > 100);
    let h = q.pop_back_if(|x| *x % 11 == 0);
    d!("deque {:?} {:?} {:?} {:?}", q, f, g, h);
    let c: Vec<u32> = std::iter::chain([1, 2], vec![3, 4]).collect();
    d!("chain {:?}", c);
    let bytes = b"a\tb\"\\\x00\xff z";
    d!("escape_ascii {} {}", bytes.escape_ascii(), bytes.escape_ascii().count());
}

fn layout_ptr_float() {
    use std::alloc::Layout;
    let l = Layout::from_size_align(6, 4).unwrap();
    let p = l.extend_packed(Layout::new::<u8>()).unwrap();
    let (r, stride) = l.repeat(3).unwrap();
    let rp = l.repeat_packed(3).unwrap();
    d!("layout {}/{} {}/{} {} {}/{} {}", p.size(), p.align(), r.size(), r.align(), stride, rp.size(), rp.align(), Layout::new::<u64>().repeat(usize::MAX).is_err());
    let mut x = [10u32, 20, 30];
    let nn = std::ptr::NonNull::from(&mut x[0]);
    let second = nn.with_addr(nn.addr().checked_add(4).unwrap());
    let third = nn.map_addr(|a| a.checked_add(8).unwrap());
    d!("nonnull {} {}", unsafe { *second.as_ptr() }, unsafe { *third.as_ptr() });
    let fl = black_box(1234.75f64);
    let i: i32 = unsafe { fl.to_int_unchecked() };
    let u: u8 = unsafe { black_box(200.9f32).to_int_unchecked() };
    d!("to_int_unchecked {} {}", i, u);
    let mut big: Vec<u8> = Vec::new();
    let e: std::io::Error = big.try_reserve(usize::MAX).unwrap_err().into();
    d!("io_error_try_reserve {:?}", e.kind());
}

#[cfg(target_arch = "x86_64")]
mod x86 {
    use std::arch::x86_64::*;
    use std::hint::black_box;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 { self.0 ^= self.0 << 13; self.0 ^= self.0 >> 7; self.0 ^= self.0 << 17; self.0 }
        fn f32(&mut self) -> f32 {
            let r = self.next();
            match r % 16 { 0 => f32::NAN, 1 => -0.0, 2 => f32::INFINITY, 3 => 2.5, 4 => -3.5, _ => ((r >> 8) as i32 as f32) / 65536.0 }
        }
    }
    fn fold(h: &mut u64, bytes: &[u8]) { for b in bytes { *h = (*h ^ *b as u64).wrapping_mul(0x100000001b3); } }
    fn h128i(h: &mut u64, v: __m128i) { fold(h, &unsafe { std::mem::transmute::<__m128i, [u8; 16]>(v) }) }
    fn h128(h: &mut u64, v: __m128) { fold(h, &unsafe { std::mem::transmute::<__m128, [u8; 16]>(v) }) }
    fn h256i(h: &mut u64, v: __m256i) { fold(h, &unsafe { std::mem::transmute::<__m256i, [u8; 32]>(v) }) }
    fn h256(h: &mut u64, v: __m256) { fold(h, &unsafe { std::mem::transmute::<__m256, [u8; 32]>(v) }) }

    #[target_feature(enable = "sse4.1")]
    unsafe fn sse(rng: &mut Rng) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for _ in 0..64 {
            let (x, y, z) = (rng.next(), rng.next(), rng.next());
            let a = _mm_set_epi64x(x as i64, y as i64);
            let b = _mm_set_epi64x(z as i64, x.rotate_left(17) as i64);
            let fa = _mm_set_ps(rng.f32(), rng.f32(), rng.f32(), rng.f32());
            let fb = _mm_setr_ps(rng.f32(), rng.f32(), rng.f32(), rng.f32());
            let cnt = _mm_set_epi64x(0, (z % 40) as i64);
            h128i(&mut h, _mm_cmplt_epi8(a, b)); h128i(&mut h, _mm_cmplt_epi16(a, b)); h128i(&mut h, _mm_cmplt_epi32(a, b));
            h128(&mut h, _mm_cmpord_ps(fa, fb)); h128(&mut h, _mm_cmpunord_ps(fa, fb));
            h128(&mut h, _mm_blendv_ps(fa, fb, _mm_castsi128_ps(b)));
            h128i(&mut h, _mm_cvtepu8_epi16(a)); h128i(&mut h, _mm_mullo_epi32(a, b));
            h128(&mut h, _mm_round_ps::<{ _MM_FROUND_TO_NEAREST_INT | _MM_FROUND_NO_EXC }>(fa));
            h128(&mut h, _mm_round_ps::<{ _MM_FROUND_TO_ZERO | _MM_FROUND_NO_EXC }>(fa));
            h128(&mut h, _mm_round_ps::<_MM_FROUND_CUR_DIRECTION>(fa));
            h128(&mut h, _mm_floor_ps(fa)); h128(&mut h, _mm_ceil_ps(fb));
            h128i(&mut h, _mm_sll_epi32(a, cnt)); h128i(&mut h, _mm_srl_epi32(a, cnt)); h128i(&mut h, _mm_srl_epi16(a, cnt));
            h128(&mut h, _mm_set_ps1(rng.f32())); h128(&mut h, _mm_set_ss(rng.f32()));
            let mem = [rng.f32(), 1.0, 2.0, 3.0];
            h128(&mut h, _mm_load_ps(mem.as_ptr()));
            h128(&mut h, _mm_cmp_ps::<_CMP_EQ_OQ>(fa, fb)); h128(&mut h, _mm_cmp_ps::<_CMP_NEQ_OQ>(fa, fb));
            h128(&mut h, _mm_cmp_ps::<_CMP_NLT_UQ>(fa, fb)); h128(&mut h, _mm_cmp_ps::<_CMP_ORD_S>(fa, fb));
            h128(&mut h, _mm_cmp_ps::<_CMP_GT_OQ>(fa, fb)); h128(&mut h, _mm_cmp_ps::<_CMP_TRUE_US>(fa, fb));
        }
        h
    }

    #[target_feature(enable = "avx2")]
    unsafe fn avx2(rng: &mut Rng) -> u64 {
        let mut h = 0xcbf29ce484222325u64;
        for _ in 0..64 {
            let a = _mm256_set_epi64x(rng.next() as i64, rng.next() as i64, rng.next() as i64, rng.next() as i64);
            let b = _mm256_set_epi64x(rng.next() as i64, rng.next() as i64, rng.next() as i64, rng.next() as i64);
            let fa = _mm256_setr_ps(rng.f32(), rng.f32(), rng.f32(), rng.f32(), rng.f32(), rng.f32(), rng.f32(), rng.f32());
            let fb = _mm256_setr_ps(rng.f32(), rng.f32(), rng.f32(), rng.f32(), rng.f32(), rng.f32(), rng.f32(), rng.f32());
            let lo = _mm256_castsi256_si128(a);
            let cnt = _mm_set_epi64x(0, (rng.next() % 20) as i64);
            let vcnt = _mm256_and_si256(b, _mm256_set1_epi32(63));
            h256i(&mut h, _mm256_add_epi8(a, b)); h256i(&mut h, _mm256_sub_epi8(a, b));
            h256i(&mut h, _mm256_cmpeq_epi16(a, _mm256_or_si256(a, _mm256_set1_epi16(1))));
            h256i(&mut h, _mm256_cmpeq_epi32(a, b)); h256i(&mut h, _mm256_cmpgt_epi16(a, b)); h256i(&mut h, _mm256_cmpgt_epi32(a, b));
            h256i(&mut h, _mm256_max_epu8(a, b)); h256i(&mut h, _mm256_max_epu16(a, b)); h256i(&mut h, _mm256_max_epu32(a, b));
            h256i(&mut h, _mm256_min_epu8(a, b)); h256i(&mut h, _mm256_min_epu16(a, b)); h256i(&mut h, _mm256_min_epu32(a, b));
            h256i(&mut h, _mm256_cvtepu8_epi16(lo));
            h256i(&mut h, _mm256_cvttps_epi32(_mm256_mul_ps(fa, _mm256_set1_ps(0.001))));
            h256i(&mut h, _mm256_srl_epi16(a, cnt)); h256i(&mut h, _mm256_srlv_epi32(a, vcnt));
            h256(&mut h, _mm256_setr_m128(_mm256_castps256_ps128(fa), _mm256_castps256_ps128(fb)));
            h256i(&mut h, _mm256_setr_m128i(lo, _mm256_castsi256_si128(b)));
            h256(&mut h, _mm256_blendv_ps(fa, fb, _mm256_castsi256_ps(b)));
            h256(&mut h, _mm256_sqrt_ps(fa));
            h256(&mut h, _mm256_round_ps::<{ _MM_FROUND_TO_NEG_INF | _MM_FROUND_NO_EXC }>(fa));
            h256(&mut h, _mm256_floor_ps(fa)); h256(&mut h, _mm256_ceil_ps(fb));
            h256(&mut h, _mm256_unpacklo_ps(fa, fb)); h256(&mut h, _mm256_unpackhi_ps(fa, fb));
            fold(&mut h, &_mm256_movemask_ps(fa).to_le_bytes());
            h256(&mut h, _mm256_permute2f128_ps::<0x21>(fa, fb)); h256(&mut h, _mm256_permute2f128_ps::<0x83>(fa, fb));
            h256(&mut h, _mm256_permutevar8x32_ps(fa, b));
            h256(&mut h, _mm256_cmp_ps::<_CMP_LT_OS>(fa, fb)); h256(&mut h, _mm256_cmp_ps::<_CMP_NGE_UQ>(fa, fb));
            h256(&mut h, _mm256_cmp_ps::<_CMP_EQ_UQ>(fa, fb)); h256(&mut h, _mm256_cmp_ps::<_CMP_UNORD_Q>(fa, fb));
            h256(&mut h, _mm256_cmp_ps::<_CMP_FALSE_OQ>(fa, fb)); h256(&mut h, _mm256_cmp_ps::<_CMP_NEQ_US>(fa, fb));
        }
        h
    }

    pub fn run() {
        let mut rng = Rng(black_box(0x9e3779b97f4a7c15));
        if is_x86_feature_detected!("sse4.1") { d!("x86_sse {:x}", unsafe { sse(&mut rng) }); }
        if is_x86_feature_detected!("avx2") { d!("x86_avx2 {:x}", unsafe { avx2(&mut rng) }); }
        d!("x86_detect {} {} {} {} {}", is_x86_feature_detected!("cmpxchg16b"), is_x86_feature_detected!("fxsr"),
            is_x86_feature_detected!("xsave"), is_x86_feature_detected!("adx"), is_x86_feature_detected!("rdrand"));
    }
}

fn work() {
    typeck();
    shifts();
    nonzero();
    arrays_and_collections();
    layout_ptr_float();
    #[cfg(target_arch = "x86_64")]
    x86::run();
}
