#[test]
fn integrates() {
    let mut e = 0;
    let mut m = check_targets::Model(4);
    assert_eq!(m.evaluate(&mut e), 5);
}
