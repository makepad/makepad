mod branch;
mod chunk;
mod info;
mod leaf;
mod node;

pub use self::{chunk::Chunk, info::Info};

use self::{branch::Branch, leaf::Leaf, node::Node};

#[derive(Clone)]
pub struct BTree<T: Chunk> {
    height: usize,
    root: Node<T>,
}

impl<T: Chunk> BTree<T> {
    pub fn new() -> Self {
        Self {
            height: 0,
            root: Node::Leaf(Leaf::new()),
        }
    }

    pub fn concat(mut self, mut other: Self) -> Self {
        if self.height < other.height {
            if let Some(node) = other
                .root
                .prepend_at_depth(self.root, other.height - self.height)
            {
                let mut branch = Branch::new();
                branch.push_front(other.root);
                branch.push_front(node);
                other.height += 1;
                other.root = Node::Branch(branch);
            }
            other
        } else {
            if let Some(node) = self
                .root
                .append_at_depth(other.root, self.height - other.height)
            {
                let mut branch = Branch::new();
                branch.push_back(self.root);
                branch.push_back(node);
                self.height += 1;
                self.root = Node::Branch(branch);
            }
            self
        }
    }
}
