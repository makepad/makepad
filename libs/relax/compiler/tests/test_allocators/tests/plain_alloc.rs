// No `#[global_allocator]`: the default allocator, not either of the other targets' ones.
#[test]
fn allocates_through_the_default_allocator() {
    assert_eq!(ta_app::counted_allocation(&ta_app::FIRST), 0);
    assert_eq!(ta_app::counted_allocation(&ta_app::SECOND), 0);
}
