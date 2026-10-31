// alloc/src/boxed/convert.rs: Box<str> / Box<[T]> from a Cow (Scope's code_graph:
// `written: String::from_utf8_lossy(..).into()` into a Box<str> field)
use std::borrow::Cow;
#[test]
fn box_str_from_cow() {
    let b: Box<str> = String::from_utf8_lossy(b"a.h").into();
    assert_eq!(&*b, "a.h");
    let o: Box<str> = Box::from(Cow::<str>::Owned(String::from("x")));
    assert_eq!(&*o, "x");
}
#[test]
fn box_slice_from_cow() {
    let b: Box<[u8]> = Cow::Borrowed(&[1u8, 2][..]).into();
    assert_eq!(&*b, &[1, 2]);
    let o: Box<[u8]> = Box::from(Cow::<[u8]>::Owned(vec![3]));
    assert_eq!(&*o, &[3]);
}
