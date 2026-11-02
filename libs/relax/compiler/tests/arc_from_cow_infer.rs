// `Arc::from` / `Rc::from` on a `Cow`: std's `From<Cow<'a, B>> for Arc<B>` (rcs/arc.rs:4223, rc.rs:3188
// @8d1a764) keeps the call ambiguous next to `From<T> for Arc<T>` until the expected type settles `B`
// (here the collect target's FromIterator item, or a later `let` type); plus the `&mut` and `Arc<str>`
// to `Arc<[u8]>` conversions. Run with `makepad test` and rustc --test; lines "| " are compared.
use std::borrow::Cow;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

#[test]
fn from_cow_waits_for_the_expected_type() {
    let files = vec![("a".to_string(), "b".to_string())];
    let map: HashMap<Arc<str>, Arc<str>> = files.iter().map(|(p, f)| (Arc::from(p.as_str()), Arc::from(Cow::Borrowed(f.as_str())))).collect();
    let owned = Arc::from(Cow::<str>::Owned("owned".to_string()));
    let owned: Arc<str> = owned;
    let slice = Rc::from(Cow::Borrowed(&[1u8, 2][..]));
    let slice: Rc<[u8]> = slice;
    println!("| {} {} {:?}", map[&Arc::<str>::from("a")], owned, slice);
}

#[test]
fn from_mut_refs_and_str_bytes() {
    let mut s = String::from("ab");
    let a: Arc<str> = Arc::from(s.as_mut_str());
    let bytes: Arc<[u8]> = Arc::from(a.clone());
    let mut v = [3u8, 4];
    let r: Rc<[u8]> = Rc::from(&mut v[..]);
    let rb: Rc<[u8]> = Rc::from(Rc::<str>::from("cd"));
    println!("| {} {:?} {:?} {:?}", a, bytes, r, rb);
}
