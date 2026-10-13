// Run with `rustc --test` or `makepad test`. Never construct an empty enum value:
// taking function addresses forces compilation of otherwise unreachable glue and bodies.
use std::mem::{discriminant, needs_drop, size_of, Discriminant};
use std::sync::Arc;

pub enum DevU32 {}
pub enum Rows {
    Host(Vec<usize>),
    Dev(Arc<DevU32>, usize),
}

fn count(rows: &Rows) -> usize {
    match rows {
        Rows::Host(v) => v.len(),
        Rows::Dev(_, n) => *n,
    }
}

fn empty_match(d: &DevU32) -> usize {
    match *d {}
}

fn empty_discriminant(d: &DevU32) -> Discriminant<DevU32> {
    discriminant(d)
}

#[test]
fn empty_enum_layout_and_needs_drop() {
    assert_eq!(size_of::<DevU32>(), 0);
    assert!(!needs_drop::<DevU32>());
    assert!(!needs_drop::<Option<DevU32>>());
    assert!(needs_drop::<Arc<DevU32>>());
    assert!(needs_drop::<Option<Arc<DevU32>>>());
    assert_eq!(size_of::<Option<Arc<DevU32>>>(), size_of::<Arc<DevU32>>());
}

#[test]
fn empty_enum_bodies_and_explicit_drop_glue() {
    std::hint::black_box(empty_match as fn(&DevU32) -> usize);
    std::hint::black_box(empty_discriminant as fn(&DevU32) -> Discriminant<DevU32>);
    std::hint::black_box(std::ptr::drop_in_place::<DevU32> as unsafe fn(*mut DevU32));
}

#[test]
fn arc_of_empty_enum_drop_glue() {
    let rows = Rows::Host(vec![1, 2, 3]);
    assert_eq!(count(&rows), 3);
    drop(rows);
    let rows = vec![Rows::Host(vec![]), Rows::Host(vec![4])];
    assert_eq!(rows.iter().map(count).sum::<usize>(), 1);
    drop(rows);
    let optional: Option<Arc<DevU32>> = None;
    assert!(optional.is_none());
    drop(optional);
}
