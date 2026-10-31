// null-pointer niche layouts (rustc guarantees them for Option<&T> / Option<&mut T> /
// Option<NonNull<T>> / Option<fn()>; Box/Rc/Arc/Vec get it once std stores NonNull): same size as the pointer, None is
// null, and C receives Option<extern "C" fn> as a nullable function pointer
use std::ptr::NonNull;
extern "C" {
    fn qsort(base: *mut u8, n: usize, w: usize, cmp: Option<extern "C" fn(*const u8, *const u8) -> i32>);
}
extern "C" fn cmp_u8(a: *const u8, b: *const u8) -> i32 { unsafe { *a as i32 - *b as i32 } }
#[derive(Debug, PartialEq, Clone)]
enum E<'a> { Empty, Has(u32, &'a str) }
#[test]
fn niche_layouts() {
    assert_eq!(std::mem::size_of::<Option<&u32>>(), 8);
    assert_eq!(std::mem::size_of::<Option<NonNull<u8>>>(), 8);
    assert_eq!(std::mem::size_of::<Option<fn()>>(), 8);
    assert_eq!(std::mem::size_of::<Option<&str>>(), 16);
    let x = 5u32;
    let a: Option<&u32> = Some(&x);
    let n: Option<&u32> = None;
    assert_eq!(unsafe { std::mem::transmute::<Option<&u32>, usize>(n) }, 0);
    assert_eq!(a.copied(), Some(5));
    assert!(n.is_none());
    let b: Option<Box<String>> = Some(Box::new("hi".into()));
    match &b { Some(s) => assert_eq!(s.len(), 2), None => panic!() }
    let v: Vec<Option<Box<u32>>> = vec![None, Some(Box::new(3)), None];
    assert_eq!(v.iter().filter(|x| x.is_some()).count(), 1);
    let e = E::Has(1, "a");
    assert_eq!(e.clone(), E::Has(1, "a"));
    assert_ne!(E::Empty, e);
    let mut arr = [3u8, 1, 2];
    unsafe { qsort(arr.as_mut_ptr(), 3, 1, Some(cmp_u8)) };
    assert_eq!(arr, [1, 2, 3]);
    let f: Option<fn(u32) -> u32> = Some(|x| x + 1);
    assert_eq!(f.map(|g| g(1)), Some(2));
    let s: Option<Vec<u32>> = Some(vec![1]);
    if let Some(ref v) = s { assert_eq!(v[0], 1); }
}

// a closure's by-reference captures are references (never null): Option<closure> uses one as
// its niche (rustc's `&T` / `&mut T` upvars; Relax typed them `*mut T`, 16 bytes)
#[test]
fn closure_capture_niche() {
    use std::mem::size_of_val;
    let a = 10i32;
    let mut m = 0u8;
    let b = Some(|| a + 1);
    let mut c = Some(|| { m += 1; m });
    let a4 = 14i32;
    let e = Some(move || a4);
    assert_eq!((size_of_val(&b), size_of_val(&e)), (8, 8));
    assert_eq!(size_of_val(&c), 8);
    assert_eq!(c.as_mut().unwrap()(), 1);
    assert_eq!(b.unwrap()() + e.unwrap()(), 25);
}
