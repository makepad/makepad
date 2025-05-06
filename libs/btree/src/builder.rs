use {
    crate::{branch, BTree, Branch, Leaf, Node},
    std::{cmp::Ordering, fmt, sync::Arc},
};

pub struct Builder<L>
where
    L: Leaf,
{
    stack: Vec<Vec<BTree<L>>>,
}

impl<L> Builder<L>
where
    L: Leaf,
{
    pub fn new() -> Self {
        Self { stack: Vec::new() }
    }

    fn pop(&mut self) -> Option<BTree<L>> {
        self.stack.pop().map(|trees| {
            let height = trees.first().unwrap().height();
            let mut branch = Branch::new();
            for tree in trees {
                let (root, _) = tree.into_raw_parts();
                branch.push_back(root.unwrap());
            }
            unsafe { BTree::from_raw_parts(Some(Arc::new(Node::Branch(branch))), height + 1) }
        })
    }
}

impl<L> Builder<L>
where
    L: Leaf + Clone,
{
    pub fn finish(mut self, leaf: L) -> BTree<L> {
        let mut tree = BTree::from_leaf(leaf);
        while let Some(mut other_tree) = self.pop() {
            other_tree.append(tree);
            tree = other_tree;
        }
        tree
    }

    pub fn push(&mut self, leaf: L) {
        debug_assert!(leaf.is_at_least_half_full());
        let mut tree = BTree::from_leaf(leaf);
        loop {
            match self.stack.last().map_or(Ordering::Less, |last| {
                tree.height().cmp(&last.first().unwrap().height())
            }) {
                Ordering::Less => {
                    self.stack.push(vec![tree]);
                    break;
                }
                Ordering::Equal => {
                    self.stack.last_mut().unwrap().push(tree);
                    if self.stack.last().unwrap().len() < branch::MAX_LEN {
                        break;
                    }
                    tree = self.pop().unwrap();
                }
                Ordering::Greater => {
                    tree.append(self.pop().unwrap());
                }
            }
        }
    }
}

impl<L> fmt::Debug for Builder<L>
where
    L: Leaf + fmt::Debug,
    L::Info: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Builder")
            .field("stack", &self.stack)
            .finish()
    }
}
