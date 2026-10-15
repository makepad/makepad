// Calls through `extern "C" fn` pointers use the C calling convention (the pointer type's ABI is
// the call's ABI; boot lane B1a, the objc_msgSend shape in platform/src/os/apple/metal.rs):
// C functions taken as pointers with HFA / packed / > 16-byte aggregate arguments and results,
// a dlsym'ed symbol transmuted to its signature, Rust `extern "C"` fns called through pointers,
// and C-ABI pointers used through the Fn traits (generic and `dyn`). rustc: all pass.

extern "C" {
    fn cabs(z: Cplx) -> f64;
    fn cexp(z: Cplx) -> Cplx;
    fn div(a: i32, b: i32) -> DivT;
    fn ldiv(a: i64, b: i64) -> LDivT;
    fn dlsym(handle: *mut u8, name: *const u8) -> *mut u8;
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Cplx {
    re: f64,
    im: f64,
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

/// > 16 bytes: by reference in C on both arm64 and x64, returned through a result slot
#[repr(C)]
#[derive(Clone, Copy)]
struct Region {
    origin: [u64; 3],
    size: [u64; 3],
}

extern "C" fn region_sum(r: Region, k: u64) -> u64 {
    r.origin[0] + r.origin[1] + r.origin[2] + r.size[0] + r.size[1] + r.size[2] + k
}

extern "C" fn region_grow(r: Region, d: f64, z: Cplx) -> Region {
    let mut o = r;
    for i in 0..3 {
        o.size[i] += d as u64 + z.re as u64;
    }
    o
}

extern "C" fn narrow(x: u32) -> u8 {
    (x & 0xff) as u8
}

fn region() -> Region {
    Region { origin: [1, 2, 3], size: [10, 20, 30] }
}

#[test]
fn c_functions_through_pointers() {
    let fa: unsafe extern "C" fn(Cplx) -> f64 = cabs;
    let fe: unsafe extern "C" fn(Cplx) -> Cplx = cexp;
    let fd: unsafe extern "C" fn(i32, i32) -> DivT = div;
    let fl: unsafe extern "C" fn(i64, i64) -> LDivT = ldiv;
    unsafe {
        assert!(fa(Cplx { re: 3.0, im: 4.0 }) == 5.0);
        let e = fe(Cplx { re: 0.0, im: 0.0 });
        assert!(e.re == 1.0 && e.im == 0.0);
        let d = fd(17, 5);
        assert!(d.quot == 3 && d.rem == 2);
        let l = fl(-17, 5);
        assert!(l.quot == -3 && l.rem == -2);
    }
}

#[test]
fn transmuted_symbol() {
    #[cfg(target_os = "macos")]
    let h = -2isize as *mut u8;
    #[cfg(not(target_os = "macos"))]
    let h = 0isize as *mut u8;
    let p = unsafe { dlsym(h, b"ldiv\0".as_ptr()) };
    assert!(!p.is_null());
    let f: extern "C" fn(i64, i64) -> LDivT = unsafe { std::mem::transmute(p) };
    let l = f(100, 7);
    assert!(l.quot == 14 && l.rem == 2);
}

#[test]
fn rust_c_fns_through_pointers() {
    let s: extern "C" fn(Region, u64) -> u64 = region_sum;
    let g: extern "C" fn(Region, f64, Cplx) -> Region = region_grow;
    let n: extern "C" fn(u32) -> u8 = narrow;
    assert!(s(region(), 4) == 70);
    let r = g(region(), 1.0, Cplx { re: 2.0, im: 0.0 });
    assert!(r.size == [13, 23, 33] && r.origin == [1, 2, 3]);
    assert!(n(0x1234) == 0x34);
    let table: [extern "C" fn(Region, u64) -> u64; 2] = [region_sum, region_sum];
    assert!(table[1](region(), 0) == 66);
}

fn call_generic<F: Fn(Region, u64) -> u64>(f: F) -> u64 {
    f(region(), 1)
}

struct Holder<T> {
    f: extern "C" fn(T, u64) -> u64,
}

impl<T: Copy> Holder<T> {
    fn call(&self, t: T) -> u64 {
        (self.f)(t, 5)
    }
}

// only Rust-ABI fn pointers implement the Fn traits (rustc_type_ir is_fn_trait_compatible):
// C-ABI pointers reach generic code wrapped in closures or as plain fields
#[test]
fn c_pointers_in_generic_code() {
    let s: extern "C" fn(Region, u64) -> u64 = region_sum;
    assert!(call_generic(move |r, k| s(r, k)) == 67);
    let g: extern "C" fn(Region, f64, Cplx) -> Region = region_grow;
    let d: Box<dyn Fn(Region) -> Region> = Box::new(move |r| g(r, 2.0, Cplx { re: 0.0, im: 0.0 }));
    assert!(d(region()).size == [12, 22, 32]);
    let h = Holder { f: region_sum };
    assert!(h.call(region()) == 71);
    let o: Option<extern "C" fn(u32) -> u8> = Some(narrow);
    assert!(o.map_or(0, |f| f(0x2ff)) == 0xff);
}
