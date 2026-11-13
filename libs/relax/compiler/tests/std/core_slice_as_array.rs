// `[T]::as_array` / `as_mut_array` and the raw slice pointers' `as_array` / `as_mut_array` (stable since
// 1.93; relax PR #114, after xn3cr0nx's test): the whole slice as an array, `None` for any other length,
// in const evaluation too.
const PAIR: Option<&[u8; 2]> = {
    let s: &[u8] = &[7, 8];
    s.as_array()
};

#[test]
fn a01_as_array() {
    let s: &[u8] = &[1, 2, 3, 4];
    assert_eq!(s.as_array::<4>(), Some(&[1, 2, 3, 4]), "whole slice");
    assert_eq!(s.as_array::<3>(), None, "shorter");
    assert_eq!(s.as_array::<5>(), None, "longer");
    let empty: &[u8] = &[];
    assert_eq!(empty.as_array::<0>(), Some(&[]), "empty");
    assert_eq!(PAIR, Some(&[7, 8]), "const");

    let mut v = [1u8, 2, 3, 4];
    *v[..].as_mut_array::<4>().unwrap() = [9, 8, 7, 6];
    assert_eq!(v, [9, 8, 7, 6], "as_mut_array writes through");
    assert!(v[..3].as_mut_array::<4>().is_none(), "as_mut_array shorter");

    let p: *const [u8] = &v[1..3];
    assert_eq!(p.as_array::<2>().map(|a| unsafe { *a }), Some([8, 7]), "*const [T]");
    assert!(p.as_array::<3>().is_none(), "*const [T] other length");
    let q: *mut [u8] = &mut v[2..];
    unsafe { *q.as_mut_array::<2>().unwrap() = [0, 0] };
    assert_eq!(v, [9, 8, 0, 0], "*mut [T]");
    assert!(q.as_mut_array::<1>().is_none(), "*mut [T] other length");
}
