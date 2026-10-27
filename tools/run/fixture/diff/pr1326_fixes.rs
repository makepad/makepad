// diff_fast corpus program: the fixes of PR #1326 (round 2) that change what a program computes or
// whether it compiles. `D <case> <value>` per case, equal to rustc's output.
// - by-reference pattern bindings take the scrutinee place in place (lower.rs scan_addr_taken: `ref` /
//   `ref mut` / match ergonomics in match, if let, while let, let and let-else), so writes through the
//   binding reach the place and reads see it;
// - a let-else that binds by reference matches its place in place (no copy, no drop of the place);
// - a closure capturing through a `&mut` root (`*r`) stores the reference (layout/closure.rs);
// - `x?` whose Output is still an inference variable falls back like rustc's diverging match arm;
// - `macro_rules!` separatorless repetitions: the inner follow set is the suffix's, not FIRST(inner);
// - a braced macro call followed by `.method()` / `?` continues as an expression;
// - cfg-stripped match arms, struct fields, array elements and statements are not expanded;
// - std: Saturating ops, (A, B): FromIterator, Ord / PartialOrd for &mut, OsString: From<&T>.
// x86-64 only: the integer intrinsics the PR added, on random bits, one hash each.
#![allow(unused_unsafe, dead_code, unused_mut, unused_variables, unreachable_code)]
macro_rules! d { ($($a:tt)*) => { println!("D {}", format!($($a)*)) } }
fn rounds(f: fn()) {
    let ms: u64 = std::env::var("DIFF_SLEEP_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    for r in 0..4 { f(); if r < 3 && ms > 0 { std::thread::sleep(std::time::Duration::from_millis(ms)); } }
}
#[test]
fn run() { rounds(work); }

use std::hint::black_box;

#[derive(Debug, Clone, PartialEq)]
struct P { a: u32, b: String }

fn ref_patterns() {
    // match with `ref mut`: the write reaches the scrutinee
    let mut x = black_box(7u32);
    match x { ref mut y => *y += 5 }
    d!("match_ref_mut {}", x);
    // if let with `ref mut` inside an Option
    let mut o = black_box(Some(3i64));
    if let Some(ref mut v) = o { *v *= 11; }
    d!("if_let_ref_mut {:?}", o);
    // match ergonomics: `&mut Option<P>` scrutinee binds `p: &mut P`
    let mut op = Some(P { a: 1, b: "x".to_string() });
    match &mut op { Some(p) => { p.a += 40; p.b.push('y'); } None => {} }
    d!("ergonomics {:?}", op);
    // while let with `ref mut` on a local array slot
    let mut arr = black_box([1u8, 2, 3, 4]);
    let mut i = 0;
    while let [ref mut first, ..] = arr[i..] { *first = first.wrapping_mul(9); i += 1; if i == arr.len() { break; } }
    d!("while_let_ref_mut {:?}", arr);
    // `let` with a ref binding: a reference to the local, not to a copy
    let mut n = black_box(100u64);
    let ref mut m = n;
    *m += 1;
    d!("let_ref_mut {}", n);
    // a tuple scrutinee field by ref, then the tuple read back
    let mut t = black_box((5u16, 6u16));
    match t { (ref mut a, _) => *a = 50 }
    d!("tuple_field_ref {:?}", t);
}

fn let_else_ref() -> u32 {
    let mut o = black_box(Some(P { a: 9, b: "p".to_string() }));
    let Some(ref mut p) = o else { return 0 };
    p.a *= 3;
    p.b.push_str("q");
    d!("let_else_ref_mut {:?}", o);
    let v = black_box(vec![10u32, 20, 30]);
    let [ref first, .., ref last] = v[..] else { return 1 };
    d!("let_else_slice_ref {} {} len {}", first, last, v.len());
    let none: Option<P> = black_box(None);
    let Some(ref q) = none else { d!("let_else_fail ok"); return 2 };
    3
}

fn closure_capture_through_mut_ref() {
    let mut x = black_box(1u32);
    {
        let r = &mut x;
        let mut c = || { *r += 10; *r * 2 };
        let a = c();
        let b = c();
        d!("closure_mut_root {} {}", a, b);
    }
    d!("closure_mut_root_after {}", x);
    let mut v = vec![1, 2, 3];
    {
        let r = &mut v;
        let mut push = |k: i32| r.push(k * 7);
        push(1);
        push(2);
    }
    d!("closure_mut_root_vec {:?}", v);
    let mut s = String::from("a");
    let r = &mut s;
    let f = move || { r.push('b'); r.len() };
    let mut f = f;
    let l = f();
    d!("closure_mut_root_move {} {}", l, s);
}

fn try_fallback() -> Result<u32, String> {
    // the Output of `?` here is an unconstrained inference variable: rustc's diverging fallback gives ()
    if black_box(false) { let _y = Err("e".to_string())?; }
    let r: Result<u32, String> = black_box(Ok(5));
    let v = r?;
    Ok(v + 1)
}

macro_rules! sum_exprs {
    ($($e:expr)*) => { 0 $(+ $e)* };
}
macro_rules! idents_tts {
    ($($i:ident)* ; $($t:tt)*) => { [$(stringify!($i)),*].len() + [$(stringify!($t)),*].len() };
}
macro_rules! braced {
    {$($e:expr),*} => { vec![$($e),*] };
}

fn macro_shapes() {
    d!("sep_less_expr {}", sum_exprs!(1 2 3 {4}));
    d!("sep_less_ident {}", idents_tts!(a b c ; x + y));
    // a braced macro followed by a method call continues as an expression
    let n = braced!{1u8, 2, 3}.len();
    d!("braced_postfix {}", n);
    let s = braced!{"a", "bb"}.iter().map(|s| s.len()).sum::<usize>();
    d!("braced_postfix_chain {}", s);
    // in statement position: still an expression statement (rustc parse_stmt_mac: `.` / `?` after `}`)
    let mut acc = 0u32;
    braced!{4u32, 5, 6}.iter().for_each(|x| acc += x);
    d!("braced_postfix_stmt {}", acc);
}

struct F { a: u8, #[cfg(any())] b: u8, c: u8 }

fn cfg_stripped() {
    let x = black_box(2u8);
    let r = match x {
        #[cfg(any())]
        2 => this_macro_does_not_exist!(),
        2 => 20,
        _ => 0,
    };
    d!("cfg_arm {}", r);
    let f = F { a: 1, #[cfg(any())] b: no_such_macro!(), c: 3 };
    d!("cfg_field {} {}", f.a, f.c);
    let v = [1u8, #[cfg(any())] nope!(), 3];
    d!("cfg_elem {:?}", v);
    #[cfg(any())]
    let z = undefined_macro!();
    #[cfg(any())]
    undefined_stmt_macro! { whatever };
    d!("cfg_stmt ok");
}

fn std_bits() {
    use std::num::Saturating;
    let mut s = Saturating(black_box(250u8));
    s += Saturating(10);
    s += 1;
    let t = Saturating(black_box(3u8)) - Saturating(10);
    let m = Saturating(black_box(100i8)) * Saturating(2);
    let q = Saturating(black_box(i8::MIN)) / Saturating(-1);
    d!("saturating {} {} {} {} {}", s, t, m, q, !Saturating(black_box(0u8)));
    let (evens, odds): (Vec<u32>, Vec<u32>) = (0..10u32).map(|i| (i * 2, i * 2 + 1)).collect();
    d!("tuple_from_iter {:?} {:?}", evens, odds);
    let mut a = black_box(3u32);
    let mut b = black_box(9u32);
    let (ra, rb) = (&mut a, &mut b);
    d!("ord_mut {:?} {:?} {}", ra.cmp(&rb), ra.partial_cmp(&rb), ra < rb);
    let mut refs = vec![&mut a, &mut b];
    refs.sort();
    refs.reverse();
    d!("sort_mut_refs {:?}", refs);
    let st = String::from("os");
    let os = std::ffi::OsString::from(&st);
    let os2 = std::ffi::OsString::from(std::path::Path::new("/p"));
    d!("os_string {:?} {:?}", os, os2);
}

fn work() {
    ref_patterns();
    d!("let_else_ret {}", let_else_ref());
    closure_capture_through_mut_ref();
    d!("try_fallback {:?}", try_fallback());
    macro_shapes();
    cfg_stripped();
    std_bits();
    arch();
}

#[cfg(not(target_arch = "x86_64"))]
fn arch() {
    d!("arch: x86-64 only");
}

#[cfg(target_arch = "x86_64")]
fn arch() {
    unsafe { x::sse2() };
    if std::is_x86_feature_detected!("sse4.1") { unsafe { x::sse41() } } else { d!("no sse4.1"); }
    if std::is_x86_feature_detected!("avx2") { unsafe { x::avx2() } } else { d!("no avx2"); }
    if std::is_x86_feature_detected!("pclmulqdq") { unsafe { x::clmul() } } else { d!("no pclmulqdq"); }
}

#[cfg(target_arch = "x86_64")]
mod x {
    use core::arch::x86_64::*;
    use std::hint::black_box;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }
    }
    struct H(u64);
    impl H {
        fn add(&mut self, v: u64) {
            self.0 = (self.0 ^ v).wrapping_mul(0x100000001b3);
        }
    }
    unsafe fn m128(r: &mut Rng) -> __m128i { _mm_set_epi64x(r.next() as i64, r.next() as i64) }
    unsafe fn m256(r: &mut Rng) -> __m256i { _mm256_set_epi64x(r.next() as i64, r.next() as i64, r.next() as i64, r.next() as i64) }
    unsafe fn h128(h: &mut H, v: __m128i) {
        let a: [u64; 2] = std::mem::transmute(v);
        h.add(a[0]);
        h.add(a[1]);
    }
    unsafe fn h256(h: &mut H, v: __m256i) {
        let a: [u64; 4] = std::mem::transmute(v);
        for x in a { h.add(x); }
    }
    macro_rules! case128 {
        ($name:literal, $r:ident, |$a:ident, $b:ident| $e:expr) => {{
            let mut h = H(0xcbf29ce484222325);
            for _ in 0..64 { let $a = m128(&mut $r); let $b = m128(&mut $r); h128(&mut h, black_box($e)); }
            d!("{} {:016x}", $name, h.0);
        }};
    }
    macro_rules! case256 {
        ($name:literal, $r:ident, |$a:ident, $b:ident| $e:expr) => {{
            let mut h = H(0xcbf29ce484222325);
            for _ in 0..64 { let $a = m256(&mut $r); let $b = m256(&mut $r); h256(&mut h, black_box($e)); }
            d!("{} {:016x}", $name, h.0);
        }};
    }
    pub unsafe fn sse2() {
        let mut r = Rng(0x9e3779b97f4a7c15);
        case128!("mm_mul_epu32", r, |a, b| _mm_mul_epu32(a, b));
        case128!("mm_mullo_epi16", r, |a, b| _mm_mullo_epi16(a, b));
        let mut h = H(0xcbf29ce484222325);
        for _ in 0..64 {
            let a = m128(&mut r);
            h.add(_mm_cvtsi128_si64(a) as u64);
            h.add(_mm_cvtsi128_si64x(a) as u64);
            h128(&mut h, _mm_cvtsi64_si128(r.next() as i64));
            h128(&mut h, _mm_cvtsi64x_si128(r.next() as i64));
        }
        d!("mm_cvtsi64 {:016x}", h.0);
    }
    #[target_feature(enable = "sse4.1")]
    pub unsafe fn sse41() {
        let mut r = Rng(0x2545f4914f6cdd1d);
        let mut h = H(0xcbf29ce484222325);
        for k in 0..64 {
            let a = m128(&mut r);
            let b = if k % 4 == 0 { _mm_setzero_si128() } else { m128(&mut r) };
            h.add(_mm_testz_si128(a, b) as u64);
            h.add(_mm_testz_si128(a, _mm_andnot_si128(a, b)) as u64);
        }
        d!("mm_testz_si128 {:016x}", h.0);
    }
    #[target_feature(enable = "avx2")]
    pub unsafe fn avx2() {
        let mut r = Rng(0xda942042e4dd58b5);
        case256!("mm256_cmpgt_epi8", r, |a, b| _mm256_cmpgt_epi8(a, b));
        case256!("mm256_xor_si256", r, |a, b| _mm256_xor_si256(a, b));
        case256!("mm256_subs_epu8", r, |a, b| _mm256_subs_epu8(a, b));
        case256!("mm256_alignr_epi8_5", r, |a, b| _mm256_alignr_epi8::<5>(a, b));
        case256!("mm256_alignr_epi8_17", r, |a, b| _mm256_alignr_epi8::<17>(a, b));
        case256!("mm256_srli_epi16_3", r, |a, b| _mm256_srli_epi16::<3>(a));
    }
    #[target_feature(enable = "pclmulqdq")]
    pub unsafe fn clmul() {
        let mut r = Rng(0x1234567887654321);
        case128!("mm_clmulepi64_si128_00", r, |a, b| _mm_clmulepi64_si128::<0x00>(a, b));
        case128!("mm_clmulepi64_si128_11", r, |a, b| _mm_clmulepi64_si128::<0x11>(a, b));
    }
}
