// Stable core/alloc/std APIs larger crates call: NonZero conversions and checked/saturating
// arithmetic, CString::from_vec_with_nul and its error, downcasting a boxed `Error + Send`,
// `future::Ready::into_inner`, the `std::string::Drain`/`FromUtf16Error` and `std::ffi` error
// paths, `ptr::copy`, and the impls of relax issue 2 (Clone of boxed unsized types and of the str
// split iterators, PhantomData's ordering, `Pin<&T>::map_unchecked`, Debug of `MaybeUninit` and
// of the maps' owning iterators).
// Run with `makepad test` and rustc --test; lines "| " are compared.
use std::error::Error;
use std::ffi::{CString, FromVecWithNulError, NulError};
use std::num::NonZero;

#[test]
fn nonzero_conversions_and_arithmetic() {
    let a = NonZero::new(300u32).unwrap();
    let narrow: Result<NonZero<u8>, _> = NonZero::<u8>::try_from(a);
    let fits = NonZero::<u16>::try_from(a).map(|v| v.get());
    let signed = NonZero::<i64>::try_from(NonZero::new(5u64).unwrap()).map(|v| v.get());
    let b = NonZero::new(70_000u32).unwrap();
    println!(
        "| {} {:?} {:?} {:?} {} {:?} {}",
        narrow.is_err(),
        fits,
        signed,
        a.checked_mul(b).map(|v| v.get()),
        a.saturating_mul(b).get(),
        NonZero::new(3u8).unwrap().checked_pow(6).map(|v| v.get()),
        NonZero::new(3u8).unwrap().saturating_pow(6).get(),
    );
}

#[test]
fn cstring_from_vec_with_nul() {
    let ok = CString::from_vec_with_nul(b"abc\0".to_vec()).unwrap();
    let missing: FromVecWithNulError = CString::from_vec_with_nul(b"abc".to_vec()).unwrap_err();
    let inner: FromVecWithNulError = CString::from_vec_with_nul(b"a\0b\0".to_vec()).unwrap_err();
    let nul: NulError = CString::new(b"x\0y".to_vec()).unwrap_err();
    let bytes = inner.as_bytes().to_vec();
    println!("| {:?} {} {:?} {} {}", ok, missing, bytes, inner.into_bytes().len(), nul.nul_position());
}

#[derive(Debug)]
struct Mine(u8);
impl std::fmt::Display for Mine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "mine {}", self.0)
    }
}
impl Error for Mine {}

#[test]
fn boxed_send_errors_downcast() {
    let e: Box<dyn Error + Send> = Box::new(Mine(4));
    let back = e.downcast::<Mine>().map(|m| m.0);
    let e: Box<dyn Error + Send> = Box::new(Mine(5));
    let wrong = e.downcast::<std::fmt::Error>().map_err(|e| e.to_string());
    let e: Box<dyn Error + Send + Sync> = Box::new(Mine(6));
    println!("| {:?} {:?} {:?}", back, wrong, e.downcast::<Mine>().map(|m| m.0));
}

#[test]
fn ready_into_inner_and_string_paths() {
    let r = std::future::ready(String::from("ready"));
    let mut s = String::from("hello world");
    let d: std::string::Drain<'_> = s.drain(..6);
    let drained: String = d.collect();
    let bad: Result<String, std::string::FromUtf16Error> = String::from_utf16(&[0xD800]);
    let mut buf = [0u8; 4];
    unsafe { std::ptr::copy([1u8, 2, 3].as_ptr(), buf.as_mut_ptr().add(1), 3) };
    println!("| {} {:?} {} {} {:?}", r.into_inner(), drained, s, bad.is_err(), buf);
}

#[derive(PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Tagged<T>(u32, std::marker::PhantomData<T>);

// relax issue 2: what cc, objc2, tracing, block2, vcpkg and serde_json call
#[test]
fn std_impls_crates_rely_on() {
    use std::cmp::Ordering;
    use std::collections::{BTreeMap, HashMap};
    use std::ffi::{CStr, OsStr};
    use std::fmt::Debug;
    use std::marker::PhantomData;
    use std::mem::{ManuallyDrop, MaybeUninit};
    use std::path::Path;
    use std::pin::Pin;

    let p: Box<Path> = Path::new("/tmp/a").into();
    let o: Box<[Box<OsStr>]> = vec![Box::<OsStr>::from(OsStr::new("x"))].into_boxed_slice();
    let c: Box<CStr> = Box::from(CStr::from_bytes_with_nul(b"cs\0").unwrap());
    assert_eq!(format!("{:?} {:?} {:?}", p.clone(), o.clone(), c.clone()), r#""/tmp/a" ["x"] "cs""#, "Clone of Box<Path>, Box<OsStr>, Box<CStr>");

    let a: PhantomData<u8> = PhantomData;
    let ord = (a.partial_cmp(&PhantomData), a.cmp(&PhantomData) == Ordering::Equal, Tagged::<u8>(1, PhantomData) < Tagged(2, PhantomData));
    assert_eq!(format!("{:?}", ord), "(Some(Equal), true, true)", "PhantomData: PartialOrd + Ord");

    let m = ManuallyDrop::new(5u32);
    let pinned: Pin<&ManuallyDrop<u32>> = Pin::new(&m);
    let inner: Pin<&u32> = unsafe { pinned.map_unchecked(|x| &**x) };
    assert_eq!(*inner, 5, "Pin<&T>::map_unchecked");

    let u = MaybeUninit::new(7i32);
    let d: &dyn Debug = &u;
    assert_eq!(format!("{:?}|{:>18?}", d, MaybeUninit::<u8>::uninit()), "MaybeUninit<i32>|   MaybeUninit<u8>", "MaybeUninit: Debug");

    let s = "a=b=c".splitn(2, '=');
    let r = "a=b=c".rsplitn(2, '=');
    let split = (s.clone().collect::<Vec<_>>(), s.collect::<Vec<_>>(), r.clone().collect::<Vec<_>>());
    assert_eq!(format!("{:?}", split), r#"(["a", "b=c"], ["a", "b=c"], ["c", "a=b"])"#, "SplitN, RSplitN: Clone");
    let more = (
        "a,b,".split_terminator(',').clone().collect::<Vec<_>>(),
        "a,b,".rsplit_terminator(',').clone().collect::<Vec<_>>(),
        "xaxbx".matches('x').clone().count(),
        "xaxbx".rmatch_indices('x').clone().collect::<Vec<_>>(),
    );
    assert_eq!(format!("{:?}", more), r#"(["a", "b"], ["b", "a"], 3, [(4, "x"), (2, "x"), (0, "x")])"#, "the other str iterators: Clone");

    let mut map = BTreeMap::new();
    map.insert(String::from("k"), 1);
    map.insert(String::from("j"), 2);
    let mut rest = map.clone().into_values();
    rest.next();
    let btree = format!("{:?} {:?} {:?} {:?}", map.clone().into_values(), rest, map.clone().into_keys(), map.into_iter());
    assert_eq!(btree, r#"[2, 1] [1] ["j", "k"] [("j", 2), ("k", 1)]"#, "btree_map IntoValues/IntoKeys/IntoIter: Debug");
    let mut b = BTreeMap::from([(1u8, 'a'), (2, 'b')]);
    let mut it = b.iter_mut();
    it.next();
    let iter_mut = format!("{:?}", it);
    assert_eq!(format!("{} {:?}", iter_mut, b.values_mut()), "[(2, 'b')] ['a', 'b']", "btree_map IterMut/ValuesMut: Debug");
    let mut h = HashMap::new();
    h.insert(3u8, 'x');
    let iter_mut = format!("{:?}", h.iter_mut());
    let values_mut = format!("{:?}", h.values_mut());
    let hash = format!("{} {} {:?} {:?}", iter_mut, values_mut, h.clone().into_values(), h.clone().into_keys());
    assert_eq!(hash, "[(3, 'x')] ['x'] ['x'] [3]", "hash_map IterMut/ValuesMut/IntoValues/IntoKeys: Debug");
    let mut rest = h.into_iter();
    let full = format!("{:?}", rest);
    let first = rest.next();
    assert_eq!(format!("{} {:?} {:?}", full, first, rest), "[(3, 'x')] Some((3, 'x')) []", "hash_map IntoIter: Debug of what is left");
}
