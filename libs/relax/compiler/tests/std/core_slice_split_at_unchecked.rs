#[test]
fn a01_splits_at_both_ends_and_the_middle() {
    let bytes = b"en-US";
    for mid in [0, 2, bytes.len()] {
        let (left, right) = unsafe { bytes.split_at_unchecked(mid) };
        assert_eq!(left, &bytes[..mid]);
        assert_eq!(right, &bytes[mid..]);
        println!("| split {mid}: {left:?} {right:?}");
    }
}

#[test]
fn a02_mutable_splits_keep_both_parts_disjoint() {
    for mid in [0, 2, 4] {
        let mut values = [1, 2, 3, 4];
        let (left, right) = unsafe { values.split_at_mut_unchecked(mid) };
        for value in left { *value += 10; }
        for value in right { *value += 20; }
        let expected = match mid {
            0 => [21, 22, 23, 24],
            2 => [11, 12, 23, 24],
            4 => [11, 12, 13, 14],
            _ => unreachable!(),
        };
        assert_eq!(values, expected);
    }
    let (left, right) = unsafe { <[u8]>::split_at_mut_unchecked(&mut [], 0) };
    assert!(left.is_empty() && right.is_empty());
}
