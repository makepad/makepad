// const-check's qualif join (rustc resolver.rs: `qualif.join(..) || borrow.join(..)`) keeps a
// block's borrows only when the join adds no qualif. A local borrowed mutably and then moved
// after a call (which ends the block) is no live drop: rustc accepts each `const fn` here, and
// rejects `moved_after_a_branch_borrow` in const_moved_after_borrow_reject.rs (the borrow in the
// then-block, reached after the else path). `mem::replace` is a `const fn`.
pub struct Bag(pub Vec<u8>);

const fn empty() -> Bag {
    Bag(Vec::new())
}
const fn idle() {}
const fn touch(_: &mut Bag) {}
const fn pass(b: &mut Bag) -> &mut Bag {
    b
}

pub const fn borrowed_by_a_call() -> Bag {
    let mut bag = empty();
    touch(&mut bag);
    bag
}

pub const fn borrowed_then_a_call() -> Bag {
    let mut bag = empty();
    let _held = &mut bag;
    idle();
    bag
}

pub const fn reborrow_returned_by_a_call() -> Bag {
    let mut bag = empty();
    let _back = pass(&mut bag);
    bag
}

pub const fn borrowed_in_a_loop(n: usize) -> Bag {
    let mut bag = empty();
    let mut i = 0;
    while i < n {
        touch(&mut bag);
        i += 1;
    }
    bag
}

// The join order is rustc's reverse postorder, which visits a switch's first target first: a
// borrow there reaches the merge block before the other paths do, so the merge keeps no borrow.
pub const fn borrowed_in_the_else_branch(keep: bool) -> Bag {
    let mut bag = empty();
    if keep {
        idle();
    } else {
        let _held = &mut bag;
    }
    bag
}

pub const fn borrowed_in_the_first_arm(k: u8) -> Bag {
    let mut bag = empty();
    match k {
        0 => {
            let _held = &mut bag;
        }
        1 => idle(),
        _ => {}
    }
    bag
}

pub const fn swapped_out() -> Bag {
    let mut bag = empty();
    std::mem::forget(std::mem::replace(&mut bag, empty()));
    bag
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    pub key: &'static str,
    pub weight: u16,
}

pub const fn slots<const N: usize>(weights: [u16; N]) -> [Slot; N] {
    let mut out = [Slot { key: "", weight: 0 }; N];
    let mut i = 0;
    while i < N {
        let old = std::mem::replace(&mut out[i], Slot { key: "w", weight: weights[i] });
        assert!(old.weight == 0);
        i += 1;
    }
    out
}

const BAG: Bag = borrowed_then_a_call();
const SLOTS: [Slot; 3] = slots([5, 6, 7]);
const KEPT: Bag = borrowed_in_a_loop(3);

#[test]
fn moved_after_a_block_boundary_is_no_live_drop() {
    assert!(borrowed_by_a_call().0.is_empty(), "borrowed by a call");
    assert!(reborrow_returned_by_a_call().0.is_empty(), "reborrow returned by a call");
    assert!(swapped_out().0.is_empty(), "mem::replace");
    assert!(borrowed_in_the_else_branch(false).0.is_empty(), "else branch");
    assert!(borrowed_in_the_first_arm(0).0.is_empty(), "first arm");
    assert!(BAG.0.is_empty(), "evaluated at compile time");
    assert!(KEPT.0.is_empty(), "loop");
    assert_eq!(SLOTS[2], Slot { key: "w", weight: 7 }, "const table");
    assert_eq!(slots([9, 8])[1].weight, 8, "table at run time");
}
