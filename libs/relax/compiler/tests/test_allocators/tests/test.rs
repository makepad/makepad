//! A target named as a sysroot crate is not that crate.
#[test]
fn a_target_named_test_is_a_plain_test_target() {
    assert_eq!(String::from("x").len(), 1);
    assert_eq!(module_path!(), "test");
}
