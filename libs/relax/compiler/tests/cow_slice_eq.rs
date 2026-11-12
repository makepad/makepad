//! `Cow<[T]>` compares with slice references, with a `Vec`, and with a Cow of another lifetime
//! and borrowed type, as alloc's impls give (relax issue 22). `file_name` is the shape of a
//! path helper that compares a matched Cow with a byte-string slice.
use std::borrow::Cow;

fn file_name<'a>(path: &Cow<'a, [u8]>) -> Option<Cow<'a, [u8]>> {
    let got = match *path {
        Cow::Borrowed(p) => Cow::Borrowed(&p[1..]),
        Cow::Owned(ref p) => Cow::Owned(p[1..].to_vec()),
    };
    if got == &b".."[..] {
        return None;
    }
    Some(got)
}

#[test]
fn cow_slice_comparisons() {
    assert_eq!(file_name(&Cow::Owned(b"xabc".to_vec())), Some(Cow::Borrowed(&b"abc"[..])));
    assert_eq!(file_name(&Cow::Borrowed(&b"x.."[..])), None);
    let s: &[u8] = b"ab";
    let c: Cow<[u8]> = Cow::Borrowed(s);
    let mut m = *b"ab";
    assert!(c == s, "Cow<[T]> == &[U]");
    assert!(c == &mut m[..], "Cow<[T]> == &mut [U]");
    assert!(c == s.to_vec(), "Cow<[T]> == Vec<U>");
    let mut v = s.to_vec();
    let mv: &mut [u8] = &mut v[..];
    assert!(mv == s.to_vec(), "&mut [T] == Vec<U>");
    let owned: Cow<'static, str> = Cow::Owned(String::from("ab"));
    let short = String::from("ab");
    let borrowed: Cow<str> = Cow::Borrowed(&short);
    assert!(owned == borrowed, "Cow<'a, B> == Cow<'b, C>");
    assert!(c != Cow::<[u8]>::Owned(b"ba".to_vec()), "Cow != Cow");
}
