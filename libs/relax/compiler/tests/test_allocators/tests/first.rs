#[global_allocator]
static ALLOC: ta_app::Counting = ta_app::Counting(&ta_app::FIRST);

#[test]
fn first_allocates_through_its_own_allocator() {
    assert!(ta_app::counted_allocation(&ta_app::FIRST) >= 1);
    assert_eq!(ta_app::counted_allocation(&ta_app::SECOND), 0);
}
