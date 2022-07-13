use crate::{Branch, Leaf};

#[derive(Clone)]
pub enum Node<L: Leaf> {
    Leaf(L),
    Branch(Branch<L>),
}
