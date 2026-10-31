// getrandom 0.4's sys_fill_exact: `buf.split_at_mut_checked(n)` on a slice of MaybeUninit
use std::mem::MaybeUninit;

#[test]
fn split_at_mut_checked_bounds() {
    let mut a = [1u8, 2, 3, 4];
    {
        let (l, r) = a.split_at_mut_checked(1).unwrap();
        l[0] = 10;
        r[2] = 40;
    }
    assert_eq!(a, [10, 2, 3, 40]);
    assert!(a.split_at_mut_checked(4).is_some_and(|(l, r)| l.len() == 4 && r.is_empty()));
    assert!(a.split_at_mut_checked(5).is_none());
    let mut u = [MaybeUninit::<u8>::uninit(); 3];
    let (head, tail) = u.split_at_mut_checked(2).unwrap();
    assert_eq!((head.len(), tail.len()), (2, 1));
    assert_eq!(a.split_at_checked(2), Some((&[10u8, 2][..], &[3u8, 40][..])));
}
