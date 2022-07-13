use {crate::{Leaf, Node}, std::ops::{AddAssign, SubAssign}};

pub trait Info<L: Leaf>: Copy + AddAssign + SubAssign {
    fn from_nodes(nodes: &[Node<L>]) -> Self;
}
