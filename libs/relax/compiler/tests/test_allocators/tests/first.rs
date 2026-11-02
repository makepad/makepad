use std::sync::atomic::AtomicUsize;

static COUNT: AtomicUsize = AtomicUsize::new(0);

#[global_allocator]
static ALLOC: ta_app::Counting = ta_app::Counting(&COUNT);

#[test]
fn first_allocates_through_its_own_allocator() {
    assert!(ta_app::counted_allocation(&COUNT) >= 1);
}
