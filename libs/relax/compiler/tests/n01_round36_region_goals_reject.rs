// A goal on two invariant references keeps its lifetime equality through the borrowck type-op
// boundary (rustc_trait_selection/traits/query/type_op/custom.rs): rustc rejects the unrelated 'a/'b.
use std::cell::Cell;
trait Same {}
impl<'a> Same for (Cell<&'a u8>, Cell<&'a u8>) {}
fn require<T: Same>(_: T) {}
fn different<'a, 'b>(a: Cell<&'a u8>, b: Cell<&'b u8>) { require((a, b)); }
#[test]
fn invariant_trait_goal() {
    let value = 1u8;
    different(Cell::new(&value), Cell::new(&value));
}
