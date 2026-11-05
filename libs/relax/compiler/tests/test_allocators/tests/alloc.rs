//! A target named as a sysroot crate is not that crate: its allocator conflicts with no sibling's.
#[global_allocator]
static ALLOC: ta_app::Counting = ta_app::Counting(&ta_app::NAMED_ALLOC);

#[test]
fn a_target_named_alloc_allocates_through_its_own_allocator() {
    assert!(ta_app::counted_allocation(&ta_app::NAMED_ALLOC) >= 1);
    assert_eq!(ta_app::counted_allocation(&ta_app::FIRST), 0);
    assert_eq!(module_path!(), "alloc");
}
