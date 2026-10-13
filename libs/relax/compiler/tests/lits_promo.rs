// c".." / cr".." literals are &CStr; br".." is a raw byte string
use std::ffi::CStr;
extern "C" { fn strlen(s: *const std::ffi::c_char) -> usize; }
#[test]
fn c_literals() {
    let s: &CStr = c"hello";
    assert_eq!(s.to_bytes(), b"hello");
    assert_eq!(s.to_bytes_with_nul().len(), 6);
    assert_eq!(unsafe { strlen(c"abc".as_ptr()) }, 3);
    let e = c"tab\there";
    assert_eq!(e.to_bytes().len(), 8);
    let r = cr"x\y";
    assert_eq!(r.to_bytes(), b"x\\y");
}
#[test]
fn raw_byte_string() { let b = br"a\n"; assert_eq!(b.len(), 3); assert_eq!(b, b"a\\n"); }
// a borrowed struct literal of fn items is promoted (alloc::task's per-type waker vtable)
struct VT { f: fn(u32) -> u32, g: fn(u32) -> u32 }
fn dbl(x: u32) -> u32 { x * 2 }
fn inc<const N: u32>(x: u32) -> u32 { x + N }
fn vt<const N: u32>() -> &'static VT { &VT { f: dbl, g: inc::<N> } }
#[test]
fn promoted_fn_table() { let t = vt::<3>(); assert_eq!((t.f)(4) + (t.g)(1), 12); }
// target_feature cfgs match rustc's defaults for the target
#[test]
fn default_target_features() {
    #[cfg(target_arch = "aarch64")]
    { assert!(cfg!(target_feature = "neon") && cfg!(target_feature = "crc") && cfg!(target_feature = "lse")); }
    #[cfg(target_arch = "x86_64")]
    { assert!(cfg!(target_feature = "sse2") && !cfg!(target_feature = "avx2")); }
}

// identical promoted constants share one read-only allocation (rustc interns constant
// allocations by content: `&0u8` twice is one address; tests/ui mir/mir_raw_fat_ptr.rs)
#[derive(Debug, PartialEq)]
struct PP { a: u8, b: u32, c: f32 }
fn pp() -> &'static PP { &PP { a: 1, b: 2, c: 0.5 } }
#[test]
fn promoted_dedup() {
    let a: *const u8 = &0u8;
    let b: *const u8 = &0u8;
    let c: *const u8 = &1u8;
    assert!(a == b && a != c);
    assert!(std::ptr::eq(pp(), pp()));
    assert_eq!(*pp(), PP { a: 1, b: 2, c: 0.5 });
    let s: &[i32; 3] = &[1, 2, 3];
    let t: &[i32; 3] = &[1, 2, 3];
    assert!(std::ptr::eq(s, t));
    assert_eq!(s, &[1, 2, 3]);
    let x: &(u8, u16, i64) = &(3, 4, -5);
    assert_eq!(*x, (3, 4, -5));
    let o: &Option<u8> = &Some(7);
    let n: &Option<u8> = &None;
    assert_eq!((o, n), (&Some(7), &None));
    let f: &f64 = &1.5;
    assert_eq!(*f, 1.5);
    let rr: &&u32 = &&9;
    assert_eq!(**rr, 9);
    let strs: &[&str; 2] = &["a", "b"];
    assert_eq!(strs.concat(), "ab");
}
