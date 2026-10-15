// `extern "C" fn` pointer types carry their ABI (rustc FnSig.abi): calls through them use the C
// ABI, so a 48-byte struct by value reaches the callee (objc msg_send! with MTLRegion, metal.rs)
#[repr(C)] #[derive(Clone, Copy)] pub struct O { x: u64, y: u64, z: u64 }
#[repr(C)] #[derive(Clone, Copy)] pub struct S { w: u64, h: u64, d: u64 }
#[repr(C)] #[derive(Clone, Copy)] pub struct R { o: O, s: S }
extern "C" fn target(obj: *mut u8, sel: usize, r: R, lvl: u64, p: *const u8, bpr: u64) -> u64 {
    let _ = obj;
    sel as u64 + r.o.x + r.o.y * 10 + r.o.z * 100 + r.s.w * 1000 + r.s.h * 10000 + r.s.d * 100000 + lvl * 1000000 + bpr * 10000000 + unsafe { *p } as u64 * 100000000
}
#[test]
fn direct() {
    let r = R { o: O { x: 1, y: 2, z: 3 }, s: S { w: 4, h: 5, d: 6 } };
    let z = [7u8; 4];
    assert_eq!(target(std::ptr::null_mut(), 0, r, 8, z.as_ptr(), 9), 798654321);
}
#[test]
fn via_ptr() {
    let r = R { o: O { x: 1, y: 2, z: 3 }, s: S { w: 4, h: 5, d: 6 } };
    let z = [7u8; 4];
    let f: extern "C" fn(*mut u8, usize, R, u64, *const u8, u64) -> u64 = target;
    assert_eq!(f(std::ptr::null_mut(), 0, r, 8, z.as_ptr(), 9), 798654321);
}
#[test]
fn via_transmute() {
    let r = R { o: O { x: 1, y: 2, z: 3 }, s: S { w: 4, h: 5, d: 6 } };
    let z = [7u8; 4];
    let f: unsafe extern "C" fn(*mut u8, usize, R, u64, *const u8, u64) -> u64 = unsafe { std::mem::transmute(target as usize) };
    assert_eq!(unsafe { f(std::ptr::null_mut(), 0, r, 8, z.as_ptr(), 9) }, 798654321);
}
trait Args { unsafe fn invoke<Q: 'static>(imp: usize, obj: *mut u8, sel: usize, a: Self) -> Q; }
impl<A, B, C, D> Args for (A, B, C, D) {
    unsafe fn invoke<Q: 'static>(imp: usize, obj: *mut u8, sel: usize, (a, b, c, d): Self) -> Q {
        let f: unsafe extern "C" fn(*mut u8, usize, A, B, C, D) -> Q = std::mem::transmute(imp);
        f(obj, sel, a, b, c, d)
    }
}
#[test]
fn big_struct_through_fn_ptr() {
    let r = R { o: O { x: 1, y: 2, z: 3 }, s: S { w: 4, h: 5, d: 6 } };
    let z = [7u8; 4];
    let v: u64 = unsafe { <(R, u64, *const u8, u64)>::invoke(target as usize, std::ptr::null_mut(), 0, (r, 8u64, z.as_ptr(), 9u64)) };
    assert_eq!(v, 798654321);
}
