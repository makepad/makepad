use super::{Branch, Chunk, Leaf};

#[derive(Clone)]
pub enum Node<T: Chunk> {
    Leaf(Leaf<T>),
    Branch(Branch<T>),
}

impl<T: Chunk> Node<T> {
    pub fn into_leaf(self) -> Leaf<T> {
        match self {
            Self::Leaf(leaf) => leaf,
            _ => panic!(),
        }
    }

    pub fn into_branch(self) -> Branch<T> {
        match self {
            Self::Branch(branch) => branch,
            _ => panic!(),
        }
    }

    pub fn len(&self) -> usize {
        match self {
            Self::Leaf(leaf) => leaf.len(),
            Self::Branch(branch) => branch.len(),
        }
    }

    pub fn info(&self) -> T::Info {
        match self {
            Self::Leaf(leaf) => leaf.info(),
            Self::Branch(branch) => branch.info(),
        }
    }

    pub fn as_mut_branch(&mut self) -> &mut Branch<T> {
        match self {
            Self::Branch(branch) => branch,
            _ => panic!(),
        }
    }

    pub fn prepend_at_depth(&mut self, other: Node<T>, depth: usize) -> Option<Self> {
        if depth == 0 {
            match self {
                Self::Leaf(leaf) => leaf
                    .prepend_or_distribute(other.into_leaf())
                    .map(|other_leaf| Node::Leaf(other_leaf)),
                Self::Branch(branch) => branch
                    .prepend_or_distribute(other.into_branch())
                    .map(|other_branch| Node::Branch(other_branch)),
            }
        } else {
            let branch = self.as_mut_branch();
            let mut node = branch.pop_front().unwrap();
            let other_node = node.prepend_at_depth(other, depth - 1);
            branch.push_front(node);
            other_node.and_then(|other_node| {
                branch
                    .push_front_and_maybe_split(other_node)
                    .map(|other_branch| Node::Branch(other_branch))
            })
        }
    }

    pub fn append_at_depth(&mut self, other: Node<T>, depth: usize) -> Option<Self> {
        if depth == 0 {
            match self {
                Self::Leaf(leaf) => leaf
                    .append_or_distribute(other.into_leaf())
                    .map(|other_leaf| Node::Leaf(other_leaf)),
                Self::Branch(branch) => branch
                    .append_or_distribute(other.into_branch())
                    .map(|other_branch| Node::Branch(other_branch)),
            }
        } else {
            let branch = self.as_mut_branch();
            let mut node = branch.pop_back().unwrap();
            let other_node = node.prepend_at_depth(other, depth - 1);
            branch.push_back(node);
            other_node.and_then(|other_node| {
                branch
                    .push_back_and_maybe_split(other_node)
                    .map(|other_branch| Node::Branch(other_branch))
            })
        }
    }
}
