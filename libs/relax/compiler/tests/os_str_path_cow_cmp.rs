// Equality and order, both ways, between the owned, borrowed and Cow forms of OsStr, of Path, and
// between Path's forms and OsStr's, as real std's impl_cmp and impl_cmp_os_str give them (relax
// issue 72: a `Cow<OsStr>` compared with an `&OsStr` was E0308). Paths compare by components.
use std::borrow::Cow;
use std::cmp::Ordering;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[test]
fn cow_os_str_compares_with_every_os_str_form() {
    // the issue's: a path component checked against "."
    let component: Cow<'_, OsStr> = Cow::Borrowed(OsStr::new("."));
    assert!(component.is_empty() || component == OsStr::new("."));
    let c = component;
    let o = OsStr::new(".");
    let os = OsString::from(".");
    assert!(c == o && o == c && c == *o && *o == c && c == os && os == c);
    assert!(os == o && o == os && os == *o && *o == os);
    let b: Cow<'_, OsStr> = Cow::Owned(OsString::from("b"));
    assert_eq!(b.partial_cmp(&o), Some(Ordering::Greater));
    assert_eq!(o.partial_cmp(&b), Some(Ordering::Less));
    assert!(b > *o && *o < b && b > os && os < b && os < *OsStr::new("b") && *OsStr::new("b") > os);
    assert!(*OsStr::new("a") < *"b" && OsString::from("c") > *"b");
}

#[test]
fn path_forms_compare_with_each_other_and_with_os_str_forms() {
    let p = Path::new("a/b");
    let pb = PathBuf::from("a//b");
    let cp: Cow<'_, Path> = Cow::Borrowed(Path::new("a/./b"));
    assert!(pb == *p && *p == pb && pb == p && p == pb);
    assert!(cp == *p && *p == cp && cp == p && p == cp && cp == pb && pb == cp);
    let o = OsStr::new("a/b/");
    let os = OsString::from("a/b");
    let co: Cow<'_, OsStr> = Cow::Borrowed(OsStr::new("a//b"));
    assert!(pb == *o && *o == pb && pb == o && o == pb && pb == co && co == pb && pb == os && os == pb);
    assert!(*p == *o && *o == *p && *p == o && o == *p && *p == co && co == *p && *p == os && os == *p);
    assert!(p == *o && *o == p && p == co && co == p && p == os && os == p);
    assert!(cp == *o && *o == cp && cp == o && o == cp && cp == os && os == cp);
    assert!(Cow::Borrowed(Path::new("a")) == Cow::<OsStr>::Borrowed(OsStr::new("a")));
    let later = OsStr::new("a/c");
    assert!(pb < *later && *later > pb && p < *later && cp < later && later > cp);
    assert_eq!(pb.partial_cmp(&os), Some(Ordering::Equal));
    assert_eq!(Path::new("b").partial_cmp(&co), Some(Ordering::Greater));
    assert_eq!(cp.partial_cmp(&PathBuf::from("a/a")), Some(Ordering::Greater));
}
