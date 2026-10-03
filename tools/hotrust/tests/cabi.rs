// C ABI: calls into C with scalars, floats, narrow ints, struct by value and struct
// returns (packed ints, float pairs, HFAs, > 16-byte aggregates), variadics with stack
// arguments, C calling back into HotRust (`extern "C"` bodies), and HotRust calling its
// own `extern "C"` fns with every argument shape. Output matches rustc (tests/cabi.rs
// built with `rustc --test`).

extern "C" {
    fn snprintf(buf: *mut u8, n: usize, fmt: *const u8, ...) -> i32;
    fn fabs(x: f64) -> f64;
    fn fabsf(x: f32) -> f32;
    fn ldexp(x: f64, e: i32) -> f64;
    fn abs(x: i32) -> i32;
    fn qsort(base: *mut u8, n: usize, w: usize, cmp: extern "C" fn(*const u8, *const u8) -> i32);
    fn bsearch(key: *const u8, base: *const u8, n: usize, w: usize, cmp: extern "C" fn(*const u8, *const u8) -> i32) -> *const u8;
    fn div(a: i32, b: i32) -> DivT;
    fn ldiv(a: i64, b: i64) -> LDivT;
    fn cabs(z: Cplx) -> f64;
    fn cexp(z: Cplx) -> Cplx;
    fn cabsf(z: CplxF) -> f32;
    fn csqrtf(z: CplxF) -> CplxF;
    fn inet_ntoa(a: InAddr) -> *const u8;
    fn strlen(s: *const u8) -> usize;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct DivT {
    quot: i32,
    rem: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct LDivT {
    quot: i64,
    rem: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Cplx {
    re: f64,
    im: f64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct CplxF {
    re: f32,
    im: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct InAddr {
    s_addr: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Big {
    a: i64,
    b: f64,
    c: i32,
    d: u8,
    e: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Hfa3 {
    x: f32,
    y: f32,
    z: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Mixed {
    i: i32,
    f: f32,
    l: i64,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Bytes3 {
    a: u8,
    b: u8,
    c: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

fn hash_bytes(p: *const u8, n: usize) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut i = 0;
    while i < n {
        h = (h ^ unsafe { *((p as usize + i) as *const u8) } as u64).wrapping_mul(0x100000001b3);
        i += 1;
    }
    h
}

extern "C" fn cmp_i32(a: *const u8, b: *const u8) -> i32 {
    let x = unsafe { *(a as *const i32) };
    let y = unsafe { *(b as *const i32) };
    if x < y {
        -1
    } else if x > y {
        1
    } else {
        0
    }
}

// `extern "C"` bodies with every argument and result shape (called directly below and
// by C through qsort/bsearch above)
extern "C" fn c_big(a: Big, k: i32) -> Big {
    Big { a: a.a * k as i64, b: a.b + 0.5, c: a.c - k, d: (a.d as u32 + 1) as u8, e: a.e * 2.0 }
}

extern "C" fn c_hfa(h: Hfa3, s: f32) -> Hfa3 {
    Hfa3 { x: h.x * s, y: h.y + s, z: h.z - s }
}

extern "C" fn c_mixed(m: Mixed, b: Bytes3) -> Mixed {
    Mixed { i: m.i + b.a as i32, f: m.f * b.b as f32, l: m.l - b.c as i64 }
}

extern "C" fn c_rect(r: Rect, dx: f64, dy: f64) -> Rect {
    Rect { x: r.x + dx, y: r.y + dy, w: r.w - 2.0 * dx, h: r.h - 2.0 * dy }
}

extern "C" fn c_narrow(a: u8, b: i8, c: u16, d: i16, e: u32, f: i32, g: bool) -> i64 {
    a as i64 + b as i64 * 3 + c as i64 * 5 + d as i64 * 7 + e as i64 * 11 + f as i64 * 13 + g as i64 * 17
}

extern "C" fn c_many(a: i64, b: f64, c: i64, d: f64, e: i64, f: f64, g: i64, h: f64, i: i64, j: f64, k: i64, l: f64, m: i64, n: f64, o: i64, p: f64, q: u8, r: f32, s: i16, t: f64, u: Mixed, v: Hfa3) -> f64 {
    (a + c + e + g + i + k + m + o + q as i64 + s as i64 + u.l) as f64 + b + d + f + h + j + l + n + p + r as f64 + t + u.f as f64 + v.x as f64 + v.z as f64
}

#[test]
fn floats_and_narrow() {
    let a = unsafe { fabs(-2.5) };
    let b = unsafe { fabsf(-1.5) };
    let c = unsafe { ldexp(1.5, -3) };
    let d = unsafe { abs(-7) };
    println!("floats {} {} {} {}", a, b, c, d);
    println!("narrow {}", c_narrow(250, -3, 60000, -300, 4000000000, -5, true));
}

#[test]
fn variadic_stack() {
    let mut buf = [0u8; 160];
    let n = unsafe {
        snprintf(&mut buf[0] as *mut u8, 160, "%d %s %.3f %ld %d %d %d %d %.1f %d %d|\0" as *const str as *const u8, 7i32, "hey\0" as *const str as *const u8, 3.25f64, 123456789012i64, 1i32, 2i32, 3i32, 4i32, 0.5f64, 5i32, 6i32)
    };
    println!("snprintf n={} h={}", n, hash_bytes(&buf[0] as *const u8, n as usize));
}

#[test]
fn struct_returns_from_c() {
    let d = unsafe { div(7, -2) };
    let l = unsafe { ldiv(-17, 5) };
    println!("div {} {} ldiv {} {}", d.quot, d.rem, l.quot, l.rem);
    let z = Cplx { re: 3.0, im: 4.0 };
    let e = unsafe { cexp(Cplx { re: 0.0, im: 1.0 }) };
    let zf = unsafe { csqrtf(CplxF { re: -4.0, im: 0.0 }) };
    println!("cabs {} cexp {:.6} {:.6} cabsf {} csqrtf {} {}", unsafe { cabs(z) }, e.re, e.im, unsafe { cabsf(CplxF { re: 6.0, im: 8.0 }) }, zf.re, zf.im);
    let s = unsafe { inet_ntoa(InAddr { s_addr: 0x0100_007f }) };
    let n = unsafe { strlen(s) };
    println!("inet_ntoa len {} h={}", n, hash_bytes(s, n));
}

#[test]
fn callbacks_from_c() {
    let mut v = [5i32, 3, 9, 1, 7, -4, 12, 0];
    unsafe { qsort(&mut v[0] as *mut i32 as *mut u8, 8, 4, cmp_i32) };
    println!("sorted {} {} {} {} {} {} {} {}", v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7]);
    let key = 7i32;
    let p = unsafe { bsearch(&key as *const i32 as *const u8, &v[0] as *const i32 as *const u8, 8, 4, cmp_i32) };
    let idx = (p as usize - (&v[0] as *const i32 as usize)) / 4;
    println!("bsearch idx {}", idx);
}

#[test]
fn extern_c_bodies() {
    let b = c_big(Big { a: 3, b: 1.25, c: 10, d: 255, e: 1.5 }, 4);
    println!("big {} {} {} {} {}", b.a, b.b, b.c, b.d, b.e);
    let h = c_hfa(Hfa3 { x: 1.0, y: 2.0, z: 3.0 }, 0.5);
    println!("hfa {} {} {}", h.x, h.y, h.z);
    let m = c_mixed(Mixed { i: 1, f: 2.5, l: 100 }, Bytes3 { a: 3, b: 4, c: 5 });
    println!("mixed {} {} {}", m.i, m.f, m.l);
    let r = c_rect(Rect { x: 1.0, y: 2.0, w: 30.0, h: 40.0 }, 1.5, 2.5);
    println!("rect {} {} {} {}", r.x, r.y, r.w, r.h);
    let t = c_many(1, 0.5, 2, 0.25, 3, 0.125, 4, 1.0, 5, 2.0, 6, 3.0, 7, 4.0, 8, 5.0, 200, 0.75, -9, 6.0, Mixed { i: 0, f: 0.5, l: 1000 }, Hfa3 { x: 0.25, y: 9.0, z: 0.5 });
    println!("many {}", t);
}
