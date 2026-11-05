//! A target named as a sysroot crate is not that crate.
#[test]
fn a_target_named_proc_macro_is_a_plain_test_target() {
    assert_eq!(vec![1, 2].len(), 2);
    assert_eq!(module_path!(), "proc_macro");
}
