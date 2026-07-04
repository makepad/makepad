// Port of box3d/test/test_allocator.c
// The Rust block allocator hands out stable element indices instead of raw
// pointers, so the NULL checks become NULL_INDEX checks.

use makepad_box3d::arena_allocator::*;
use makepad_box3d::core::NULL_INDEX;
use makepad_box3d::ensure;

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Foo {
    value1: i32,
    value2: f32,
}

#[test]
fn test_block_allocate() {
    let mut allocator = create_block_allocator(std::mem::size_of::<Foo>() as i32, 2);

    let item1 = allocate_element(&mut allocator);
    ensure!(item1 != NULL_INDEX);

    let item2 = allocate_element(&mut allocator);
    ensure!(item2 != NULL_INDEX);

    ensure!(item1 != item2);

    destroy_block_allocator(&mut allocator);
}

#[test]
fn test_block_clear() {
    let mut allocator = create_block_allocator(std::mem::size_of::<Foo>() as i32, 0);

    let item1 = allocate_element(&mut allocator);
    let item2 = allocate_element(&mut allocator);

    free_element(&mut allocator, item1);
    free_element(&mut allocator, item2);

    destroy_block_allocator(&mut allocator);
}
