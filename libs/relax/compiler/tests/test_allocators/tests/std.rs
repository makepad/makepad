//! A target named as a sysroot crate is not that crate.
#[test]
fn a_target_named_std_is_a_plain_test_target() {
    assert_eq!(std::mem::size_of::<u32>(), 4);
    assert_eq!(module_path!(), "std");
}
