#[test]
fn adds() {
    assert_eq!(lw_app::f(1), 11);
}

#[test]
fn state() {
    assert_eq!(lw_app::state(), 7);
}
