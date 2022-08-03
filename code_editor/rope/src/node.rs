use crate::{Branch, Info, Leaf};

#[derive(Clone)]
pub(crate) enum Node {
    Leaf(Leaf),
    Branch(Branch),
}

impl Node {
    pub(crate) fn info(&self) -> Info {
        match self {
            Self::Leaf(leaf) => leaf.info(),
            Self::Branch(branch) => branch.info(),
        }
    }

    pub(crate) fn prepend_at_depth(&mut self, other: Node, depth: usize) -> Option<Self> {
        if depth == 0 {
            self.prepend_or_distribute(other)
        } else {
            let branch = self.as_mut_branch();
            let node = branch.update_front(|front| front.prepend_at_depth(other, depth - 1))?;
            branch
                .push_front_and_maybe_split(node)
                .map(|branch| Node::Branch(branch))
        }
    }

    pub(crate) fn append_at_depth(&mut self, other: Node, depth: usize) -> Option<Self> {
        if depth == 0 {
            self.append_or_distribute(other)
        } else {
            let branch = self.as_mut_branch();
            let node = branch.update_back(|back| back.append_at_depth(other, depth - 1))?;
            branch
                .push_back_and_maybe_split(node)
                .map(|branch| Node::Branch(branch))
        }
    }

    fn into_leaf(self) -> Leaf {
        match self {
            Self::Leaf(leaf) => leaf,
            _ => panic!(),
        }
    }

    fn into_branch(self) -> Branch {
        match self {
            Self::Branch(branch) => branch,
            _ => panic!(),
        }
    }

    fn as_mut_branch(&mut self) -> &mut Branch {
        match self {
            Self::Branch(branch) => branch,
            _ => panic!(),
        }
    }

    fn prepend_or_distribute(&mut self, other: Self) -> Option<Self> {
        match self {
            Self::Leaf(leaf) => leaf
                .prepend_or_distribute(other.into_leaf())
                .map(|other_leaf| Node::Leaf(other_leaf)),
            Self::Branch(branch) => branch
                .prepend_or_distribute(other.into_branch())
                .map(|other_branch| Node::Branch(other_branch)),
        }
    }

    fn append_or_distribute(&mut self, other: Self) -> Option<Self> {
        match self {
            Self::Leaf(leaf) => leaf
                .append_or_distribute(other.into_leaf())
                .map(|other_leaf| Node::Leaf(other_leaf)),
            Self::Branch(branch) => branch
                .append_or_distribute(other.into_branch())
                .map(|other_branch| Node::Branch(other_branch)),
        }
    }
}
