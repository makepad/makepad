// `impl From<Box<str>> for String` (alloc/src/string.rs), so `Box<str>: Into<String>`:
// pulldown-cmark strings.rs `CowStr::Boxed(s) => s.into()`.
#[test]
fn box_str_into_string() {
    let b: Box<str> = "hello".into();
    let s: String = b.into();
    assert_eq!(s, "hello");
    let b: Box<str> = Box::from("");
    assert_eq!(String::from(b), "");
    let b: Box<str> = String::from("abc").into_boxed_str();
    let mut s = String::from(b);
    s.push('d');
    assert_eq!(s, "abcd");
}
