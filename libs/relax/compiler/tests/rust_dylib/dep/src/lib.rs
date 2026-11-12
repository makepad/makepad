pub fn answer() -> u32 {
    42
}

#[test]
fn dylib_unit_test() {
    assert_eq!(answer(), 42);
}
