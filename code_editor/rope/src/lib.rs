mod branch;
mod info;
mod leaf;
mod node;

use self::{branch::Branch, info::Info, leaf::Leaf, node::Node};

#[derive(Clone)]
pub struct Rope {
    root: Node,
}
