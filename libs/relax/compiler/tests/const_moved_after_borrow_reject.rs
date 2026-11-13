// A local borrowed mutably in an `if`'s then-block and moved where the branches meet stays a live
// drop: rustc's reverse postorder joins the else path into the merge block first, so the then
// path's borrow is joined next (resolver.rs; borrowed in the else-block it is accepted, in
// const_moved_after_borrow.rs). E0493.
// rejects: cannot be evaluated at compile-time
pub struct Bag(pub Vec<u8>);

pub const fn moved_after_a_branch_borrow(take: bool) -> Bag {
    let mut bag = Bag(Vec::new());
    if take {
        let _held = &mut bag;
    }
    bag
}
