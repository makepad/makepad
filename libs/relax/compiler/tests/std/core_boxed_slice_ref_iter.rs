struct Slot { n: u32 }
#[test]
fn boxed_slice_ref_iter() {
    let mut b: Box<[Slot]> = vec![Slot { n: 1 }, Slot { n: 2 }].into_boxed_slice();
    for s in &mut b { s.n += 10; }
    let mut t = 0;
    for s in &b { t += s.n; }
    assert_eq!(t, 23);
}
