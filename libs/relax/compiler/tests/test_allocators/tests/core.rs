//! A target named as a sysroot crate is not that crate.
#[test]
fn a_target_named_core_is_a_plain_test_target() {
    assert_eq!(core::mem::size_of::<u32>(), 4);
    assert_eq!(module_path!(), "core");
}
