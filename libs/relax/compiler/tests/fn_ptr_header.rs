// Header identity rejection cases live in types::fnhdr_tests and use the body checker.
fn safe(x: i32) -> i32 { x + 1 }
unsafe fn unsafe_fn(x: i32) -> i32 { x + 2 }
extern "C" fn c_fn(x: i32) -> i32 { x + 3 }
extern "C-unwind" fn c_unwind(x: i32) -> i32 { x + 4 }
extern "system" fn system(x: i32) -> i32 { x + 5 }
extern "C" {
    fn abs(x: i32) -> i32;
    fn snprintf(out: *mut u8, len: usize, format: *const u8, ...) -> i32;
}

#[test]
fn safety_coercions() {
    let f: unsafe fn(i32) -> i32 = safe;
    let s: fn(i32) -> i32 = safe;
    let u: unsafe fn(i32) -> i32 = s;
    let c: unsafe fn(i32) -> i32 = |x| x + 6;
    assert_eq!(unsafe { f(10) }, 11);
    assert_eq!(unsafe { u(20) }, 21);
    assert_eq!(unsafe { c(30) }, 36);
    let mixed = if true { safe } else { unsafe_fn };
    assert_eq!(unsafe { mixed(40) }, 41);
}

#[test]
fn abi_calls() {
    let c: extern "C" fn(i32) -> i32 = c_fn;
    let unwind: extern "C-unwind" fn(i32) -> i32 = c_unwind;
    let sys: extern "system" fn(i32) -> i32 = system;
    let foreign: unsafe extern "C" fn(i32) -> i32 = abs;
    assert_eq!(c(10), 13);
    assert_eq!(unwind(10), 14);
    assert_eq!(sys(10), 15);
    assert_eq!(unsafe { foreign(-7) }, 7);
}

#[test]
fn variadic_pointer() {
    let f: unsafe extern "C" fn(*mut u8, usize, *const u8, ...) -> i32 = snprintf;
    let mut out = [0u8; 32];
    let n = unsafe { f(out.as_mut_ptr(), out.len(), b"%d %.1f\0".as_ptr(), 42i32, 2.5f64) };
    assert_eq!(n, 6);
    assert_eq!(out[0], b'4');
    assert_eq!(out[1], b'2');
    assert_eq!(out[3], b'2');
    assert_eq!(out[5], b'5');
}
