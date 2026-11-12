//! A false `#[cfg]` on an element of `vec![..]` removes the element, as on an array's element: the
//! `$x:expr` fragment takes the attribute along into the expansion's array, where the element is
//! configured away. A true one keeps it, a multi-line element included.
#[test]
fn cfg_on_vec_elements() {
    let arr = [#[cfg(any())] 1u32, 2, #[cfg(all())] 3];
    let v: Vec<u32> = vec![#[cfg(any())] 1, 2, #[cfg(all())] 3];
    let lines: Vec<String> = vec![
        #[cfg(any())]
        "off".into(),
        "on".into(),
    ];
    let none: Vec<u8> = vec![#[cfg(any())] 1];
    let repeated: Vec<u8> = vec![#[cfg(all())] 7; 2];
    assert_eq!(arr, [2, 3], "the array");
    assert_eq!(v, [2, 3], "vec! drops the false element and keeps the true one");
    assert_eq!(lines, ["on"], "a false cfg on the line before a multi-line element");
    assert!(none.is_empty(), "a vec! whose only element is configured away");
    assert_eq!(repeated, [7, 7], "a true cfg on the repeated element");
}
