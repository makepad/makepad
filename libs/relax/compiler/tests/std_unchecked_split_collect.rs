//! std items that crates use: `<[T]>::split_at_unchecked` / `split_at_mut_unchecked`,
//! `DoubleEndedIterator::try_rfold`, `String`'s `Extend` and `FromIterator` of `Box<str>`, and a
//! `Cow<str>` collected from chars, strs or Strings (relax issue 40).
use std::borrow::Cow;

#[test]
fn split_at_unchecked() {
    let v = [1u8, 2, 3, 4];
    let (a, b) = unsafe { v.split_at_unchecked(2) };
    assert_eq!((a, b), (&[1u8, 2][..], &[3u8, 4][..]), "split_at_unchecked");
    let (e, f) = unsafe { v.split_at_unchecked(4) };
    assert_eq!((e.len(), f.len()), (4, 0), "a split at the end");
    let mut w = [5u16, 6, 7];
    let (c, d) = unsafe { w.split_at_mut_unchecked(1) };
    c[0] += 10;
    d[1] += 20;
    assert_eq!(w, [15, 6, 27], "the halves of split_at_mut_unchecked write the slice");
}

#[test]
fn try_rfold_from_the_back() {
    let mut it = [1, 2, 3].into_iter();
    assert_eq!(it.try_rfold(0, |acc, x| Ok::<_, ()>(acc + x)), Ok(6), "a complete fold");
    let mut it = [1, 2, 3, 4].into_iter();
    let r = it.try_rfold(String::new(), |mut acc, x| {
        if x == 2 {
            return None;
        }
        acc.push_str(&x.to_string());
        Some(acc)
    });
    assert_eq!((r, it.next()), (None, Some(1)), "it stops at the first short-circuit and leaves the rest");
}

#[test]
fn strings_from_boxed_strs() {
    let s: String = vec![Box::<str>::from("x"), Box::from("yz")].into_iter().collect();
    assert_eq!(s, "xyz", "FromIterator<Box<str>>");
    let mut t = String::from("a");
    t.extend([Box::<str>::from("b")]);
    assert_eq!(t, "ab", "Extend<Box<str>>");
}

#[test]
fn cow_str_collected() {
    let c: Cow<'_, str> = "ab".chars().collect();
    let d: Cow<'_, str> = ["c", "d"].into_iter().collect();
    let e: Cow<'_, str> = vec![String::from("e"), String::from("f")].into_iter().collect();
    assert!(matches!(&c, Cow::Owned(s) if s == "ab"), "chars into an owned Cow");
    assert_eq!((&*d, &*e), ("cd", "ef"), "strs and Strings into a Cow");
}
