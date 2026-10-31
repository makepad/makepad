// Vec::extract_if (stable 1.87; ruma-macros uses `attrs.extract_if(.., pred)`): removed elements in
// order, the kept ones compacted, a range bound respected, an unfinished iterator keeps the rest.
#[test]
fn extract_if_all_and_range() {
    let mut v: Vec<u32> = (1..=10).collect();
    let even: Vec<u32> = v.extract_if(.., |x| *x % 2 == 0).collect();
    assert_eq!(even, [2, 4, 6, 8, 10]);
    assert_eq!(v, [1, 3, 5, 7, 9]);
    let mut w: Vec<u32> = (1..=10).collect();
    let mid: Vec<u32> = w.extract_if(2..6, |x| { *x += 100; *x % 2 == 1 }).collect();
    assert_eq!(mid, [103, 105]);
    assert_eq!(w, [1, 2, 104, 106, 7, 8, 9, 10]);
}

#[test]
fn extract_if_dropped_early_keeps_the_rest() {
    let mut v: Vec<String> = ["a", "bb", "c", "dd", "e"].iter().map(|s| s.to_string()).collect();
    {
        let mut it = v.extract_if(.., |s| s.len() == 2);
        assert_eq!(it.next().as_deref(), Some("bb"));
    }
    assert_eq!(v, ["a", "c", "dd", "e"]);
    println!("| extract_if {:?}", v);
}

// alloc vec/cow.rs From<&[T; N]> for Cow<[T]> (Robrix room_display_filter.rs `(&[]).into()`)
#[test]
fn cow_from_array_ref() {
    use std::borrow::Cow;
    let empty: Cow<'_, [String]> = (&[]).into();
    assert!(empty.is_empty() && matches!(empty, Cow::Borrowed(_)));
    let three: Cow<'_, [u8]> = Cow::from(&[1u8, 2, 3]);
    assert_eq!(&*three, &[1, 2, 3]);
}
