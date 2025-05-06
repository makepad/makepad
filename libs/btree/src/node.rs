use {
    crate::{Branch, Info, Leaf},
    std::{fmt, sync::Arc},
};

#[derive(Clone)]
pub(super) enum Node<L>
where
    L: Leaf,
{
    Leaf(L),
    Branch(Branch<L>),
}

impl<L> Node<L>
where
    L: Leaf,
{
    pub(super) fn as_leaf(&self) -> Option<&L> {
        match self {
            Self::Leaf(leaf) => Some(leaf),
            _ => None,
        }
    }

    pub(super) fn as_branch(&self) -> Option<&Branch<L>> {
        match self {
            Self::Branch(branch) => Some(branch),
            _ => None,
        }
    }

    pub(super) fn is_at_least_half_full(&self) -> bool {
        match self {
            Self::Leaf(leaf) => leaf.is_at_least_half_full(),
            Self::Branch(branch) => branch.is_at_least_half_full(),
        }
    }

    pub(super) fn info(&self) -> L::Info {
        match self {
            Self::Leaf(leaf) => leaf.info(),
            Self::Branch(branch) => branch.info(),
        }
    }

    pub(super) fn info_to(&self, end: usize) -> L::Info {
        if end == self.info().len() {
            return self.info();
        }
        let (leaf, info) = self.find_leaf_by_index(end);
        info.combine(leaf.info_to(end - info.len()))
    }

    pub(super) fn find_leaf_by(&self, mut f: impl FnMut(L::Info) -> bool) -> (&L, L::Info) {
        let mut node = self;
        let mut acc_info = L::Info::empty();
        loop {
            match &*node {
                Self::Leaf(leaf) => break (leaf, acc_info),
                Self::Branch(ref children) => {
                    let (child_index, new_acc_info) = children.search_by(acc_info, &mut f);
                    node = &children.nodes()[child_index];
                    acc_info = new_acc_info;
                }
            }
        }
    }

    pub(super) fn find_leaf_by_index(&self, index: usize) -> (&L, L::Info) {
        self.find_leaf_by(|next_acc_info| index < next_acc_info.len())
    }

    pub(super) fn as_mut_leaf(&mut self) -> Option<&mut L> {
        match self {
            Self::Leaf(leaf) => Some(leaf),
            _ => None,
        }
    }

    pub(super) fn as_mut_branch(&mut self) -> Option<&mut Branch<L>> {
        match self {
            Self::Branch(branch) => Some(branch),
            _ => None,
        }
    }

    pub(super) fn assert_valid(&self) {
        match self {
            Node::Leaf(_) => {}
            Node::Branch(branch) => branch.assert_valid(),
        }
    }
}

impl<L> Node<L>
where
    L: Leaf + Clone,
{
    pub(super) fn prepend_at_depth(
        &mut self,
        mut other: Arc<Self>,
        depth: usize,
    ) -> Option<Arc<Self>> {
        if depth == 0 {
            if self.prepend_distribute(Arc::make_mut(&mut other)) {
                None
            } else {
                Some(other)
            }
        } else {
            let branch = self.as_mut_branch().unwrap();
            branch
                .update_front(|front| front.unwrap().prepend_at_depth(other, depth - 1))
                .and_then(|node| branch.push_front_split(node))
                .map(|branch| Arc::new(Node::Branch(branch)))
        }
    }

    fn prepend_distribute(&mut self, other: &mut Self) -> bool {
        match self {
            Self::Leaf(leaf) => leaf.prepend_distribute(other.as_mut_leaf().unwrap()),
            Self::Branch(branch) => branch.prepend_distribute(other.as_mut_branch().unwrap()),
        }
    }

    pub(super) fn append_at_depth(
        &mut self,
        mut other: Arc<Self>,
        depth: usize,
    ) -> Option<Arc<Self>> {
        if depth == 0 {
            if self.append_distribute(Arc::make_mut(&mut other)) {
                None
            } else {
                Some(other)
            }
        } else {
            let branch = self.as_mut_branch().unwrap();
            branch
                .update_back(|back| back.unwrap().append_at_depth(other, depth - 1))
                .and_then(|node| branch.push_back_split(node))
                .map(|branch| Arc::new(Node::Branch(branch)))
        }
    }

    fn append_distribute(&mut self, other: &mut Self) -> bool {
        match self {
            Self::Leaf(leaf) => leaf.append_distribute(other.as_mut_leaf().unwrap()),
            Self::Branch(branch) => branch.append_distribute(other.as_mut_branch().unwrap()),
        }
    }

    pub(super) fn remove_from(&mut self, start: usize) {
        match self {
            Self::Leaf(leaf) => leaf.remove_from(start),
            Self::Branch(branch) => {
                let (index, len) = branch.search_by_index(start);
                let info = branch.infos()[index];
                if start == len {
                    branch.remove_from(index);
                } else if start == len + info.len() {
                    branch.remove_from(index + 1);
                } else {
                    branch.remove_from(index + 1);
                    let mut node = branch.pop_back().unwrap();
                    Arc::make_mut(&mut node).remove_from(start - len);
                    if !branch.update_back(|back| {
                        back.map_or(false, |back| {
                            back.append_distribute(Arc::make_mut(&mut node))
                        })
                    }) {
                        branch.push_back(node);
                    }
                }
            }
        }
    }

    pub(super) fn remove_to(&mut self, end: usize) {
        match self {
            Self::Leaf(leaf) => leaf.remove_to(end),
            Self::Branch(branch) => {
                let (index, len) = branch.search_by_index(end);
                let info = branch.infos()[index];
                if end == len {
                    branch.remove_to(index);
                } else if end == len + info.len() {
                    branch.remove_to(index + 1);
                } else {
                    branch.remove_to(index);
                    let mut node = branch.pop_front().unwrap();
                    Arc::make_mut(&mut node).remove_to(end - len);
                    if !branch.update_front(|front| {
                        front.map_or(false, |front| {
                            front.prepend_distribute(Arc::make_mut(&mut node))
                        })
                    }) {
                        branch.push_front(node);
                    }
                }
            }
        }
    }

    pub(super) fn split_off(&mut self, index: usize) -> Arc<Self> {
        match self {
            Self::Leaf(leaf) => Arc::new(Node::Leaf(leaf.split_off(index))),
            Self::Branch(branch) => {
                let (child_index, len) = branch.search_by_index(index);
                let child_info = branch.infos()[child_index];
                if index == len {
                    Arc::new(Node::Branch(branch.split_off(child_index)))
                } else if index == len + child_info.len() {
                    Arc::new(Node::Branch(branch.split_off(child_index + 1)))
                } else {
                    let mut other_branch = branch.split_off(child_index + 1);
                    let mut node = branch.pop_back().unwrap();
                    let mut other_node = Arc::make_mut(&mut node).split_off(index - len);
                    if !branch.update_back(|back| {
                        back.map_or(false, |back| {
                            back.append_distribute(Arc::make_mut(&mut node))
                        })
                    }) {
                        branch.push_back(node);
                    }
                    if !other_branch.update_front(|front| {
                        front.map_or(false, |front| {
                            front.prepend_distribute(Arc::make_mut(&mut other_node))
                        })
                    }) {
                        other_branch.push_front(other_node);
                    }
                    Arc::new(Node::Branch(other_branch))
                }
            }
        }
    }
}

impl<L> fmt::Debug for Node<L>
where
    L: Leaf + fmt::Debug,
    L::Info: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Node::Leaf(leaf) => write!(f, "Leaf({:?})", leaf),
            Node::Branch(branch) => write!(f, "Branch({:?})", branch),
        }
    }
}
