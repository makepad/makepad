// Extend / FromIterator impls the std here did not have, each reported from a dependency of an app:
// - String: Extend<Box<str>> and FromIterator<Box<str>> (1.45). rayon 1.12 `ParallelExtend<Box<str>>
//   for String` (src/iter/extend.rs): "the trait bound `String: Extend<Box<str>>` is not satisfied".
// - OsString: Extend and FromIterator of OsString, &OsStr and Cow<OsStr> (1.52). rayon, same file:
//   "no method named `extend` found for `&mut OsString`".
// - BinaryHeap<T>: Extend<&T> for T: Copy (1.2). rayon, same file: "type mismatch resolving
//   `<Vec<&T0> as IntoIterator>::Item == T0`".
// - Cow<[T]>: FromIterator<T> (1.0). xml 1.4 collects attributes into a `Cow<[Attribute]>`
//   (src/reader/events.rs): "a value of type `Cow<[Attribute]>` cannot be built from an iterator".
// - Cow<str>: FromIterator of char, &str and String (1.12), next to the row above.
use std::borrow::Cow;
use std::collections::BinaryHeap;
use std::ffi::{OsStr, OsString};

#[test]
fn string_from_boxed_strs() {
    let mut s = String::from("a");
    s.extend(vec![Box::<str>::from("b"), Box::<str>::from("cd")]);
    assert_eq!(s, "abcd");
    let t: String = vec![Box::<str>::from("x"), Box::<str>::from(""), Box::<str>::from("yz")].into_iter().collect();
    assert_eq!(t, "xyz");
}

#[test]
fn os_string_extend_and_collect() {
    let mut o = OsString::from("a");
    o.extend(vec![OsString::from("b"), OsString::from("c")]);
    o.extend(vec![OsStr::new("d"), OsStr::new("")]);
    o.extend(vec![Cow::Borrowed(OsStr::new("e")), Cow::Owned(OsString::from("f"))]);
    assert_eq!(o, OsString::from("abcdef"));
    let owned: OsString = vec![OsString::from("x"), OsString::from("y")].into_iter().collect();
    assert_eq!(owned, OsString::from("xy"));
    let empty: OsString = Vec::<OsString>::new().into_iter().collect();
    assert_eq!(empty, OsString::new());
    let borrowed: OsString = ["p", "q", "r"].iter().map(|s| OsStr::new(*s)).collect();
    assert_eq!(borrowed, OsString::from("pqr"));
    let cows: OsString = vec![Cow::Borrowed(OsStr::new("m")), Cow::Owned(OsString::from("n"))].into_iter().collect();
    assert_eq!(cows, OsString::from("mn"));
    let one_owned: OsString = vec![Cow::<OsStr>::Owned(OsString::from("solo"))].into_iter().collect();
    assert_eq!(one_owned, OsString::from("solo"));
}

#[test]
fn binary_heap_extend_by_reference() {
    let mut h = BinaryHeap::from(vec![3u32, 9]);
    let more = [7u32, 1, 12];
    h.extend(&more);
    h.extend(more.iter().filter(|x| **x > 5));
    assert_eq!(h.into_sorted_vec(), [1, 3, 7, 7, 9, 12, 12]);
}

#[test]
fn cow_slice_from_iterator() {
    let c: Cow<[u8]> = (1u8..=4).collect();
    assert!(matches!(c, Cow::Owned(_)));
    assert_eq!(&*c, &[1, 2, 3, 4]);
    let names: Cow<[String]> = ["a", "b"].iter().map(|s| s.to_string()).collect();
    assert_eq!(names.len(), 2);
    assert_eq!(names[1], "b");
}

#[test]
fn cow_str_from_iterator() {
    let a: Cow<str> = "héllo".chars().rev().collect();
    assert_eq!(a, "olléh");
    let b: Cow<str> = ["ab", "", "cd"].into_iter().collect();
    assert_eq!(b, "abcd");
    let c: Cow<str> = vec![String::from("x"), String::from("yz")].into_iter().collect();
    assert_eq!(c, "xyz");
    assert!(matches!(c, Cow::Owned(_)));
}
