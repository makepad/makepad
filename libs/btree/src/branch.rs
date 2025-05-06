use {
    crate::{Info, Leaf, Node},
    array_vec::ArrayVec,
    std::{fmt, mem, sync::Arc},
};

pub(super) const MAX_LEN: usize = 8;

#[derive(Clone)]
pub(super) struct Branch<L>
where
    L: Leaf,
{
    nodes: ArrayVec<Arc<Node<L>>, MAX_LEN>,
    infos: ArrayVec<L::Info, MAX_LEN>,
}

impl<L> Branch<L>
where
    L: Leaf,
{
    pub(super) fn new() -> Self {
        Self {
            nodes: ArrayVec::new(),
            infos: ArrayVec::new(),
        }
    }

    pub(super) fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub(super) fn is_at_least_half_full(&self) -> bool {
        self.len() >= MAX_LEN / 2
    }

    pub(super) fn is_full(&self) -> bool {
        self.len() == MAX_LEN
    }

    pub(super) fn len(&self) -> usize {
        self.nodes.len()
    }

    pub(super) fn info(&self) -> L::Info {
        self.infos
            .iter()
            .copied()
            .fold(L::Info::empty(), |acc, info| acc.combine(info))
    }

    pub(super) fn search_by(
        &self,
        start_info: L::Info,
        mut f: impl FnMut(L::Info) -> bool,
    ) -> (usize, L::Info) {
        let mut index = 0;
        let mut acc_info = start_info;
        for info in self.infos().iter().copied() {
            let next_acc_info = acc_info.combine(info);
            if f(next_acc_info) {
                break;
            }
            index += 1;
            acc_info = next_acc_info;
        }
        (index, acc_info)
    }

    pub(super) fn search_by_index(&self, index: usize) -> (usize, usize) {
        let mut child_index = 0;
        let mut acc_index = 0;
        for info in self.infos().iter().copied() {
            let next_acc_len = acc_index + info.len();
            if index < next_acc_len {
                break;
            }
            child_index += 1;
            acc_index = next_acc_len;
        }
        (child_index, acc_index)
    }

    pub(super) fn nodes(&self) -> &[Arc<Node<L>>] {
        &self.nodes
    }

    pub(super) fn infos(&self) -> &[L::Info] {
        &self.infos
    }

    pub(super) fn push_front(&mut self, node: Arc<Node<L>>) {
        debug_assert!(!self.is_full());
        debug_assert!(self.is_empty() || node.is_at_least_half_full());
        let info = node.info();
        self.nodes.insert(0, node);
        self.infos.insert(0, info);
    }

    pub(super) fn push_back(&mut self, node: Arc<Node<L>>) {
        debug_assert!(!self.is_full());
        debug_assert!(self.is_empty() || node.is_at_least_half_full());
        let info = node.info();
        self.nodes.push(node);
        self.infos.push(info);
    }

    pub(super) fn push_front_split(&mut self, node: Arc<Node<L>>) -> Option<Self> {
        if self.is_full() {
            let mut other = self.split_off(MAX_LEN / 2);
            mem::swap(self, &mut other);
            other.push_front(node);
            Some(other)
        } else {
            self.push_front(node);
            None
        }
    }

    pub(super) fn push_back_split(&mut self, node: Arc<Node<L>>) -> Option<Self> {
        if self.is_full() {
            let mut other = self.split_off(MAX_LEN / 2);
            other.push_back(node);
            Some(other)
        } else {
            self.push_back(node);
            None
        }
    }

    pub(super) fn pop_front(&mut self) -> Option<Arc<Node<L>>> {
        if self.is_empty() {
            return None;
        }
        self.infos.remove(0);
        Some(self.nodes.remove(0))
    }

    pub(super) fn pop_back(&mut self) -> Option<Arc<Node<L>>> {
        if self.is_empty() {
            return None;
        }
        self.infos.pop();
        Some(self.nodes.pop().unwrap())
    }

    pub(super) fn prepend(&mut self, other: &mut Self) {
        debug_assert!(self.len() + other.len() <= MAX_LEN);
        self.nodes.splice(..0, other.nodes.drain(..));
        self.infos.splice(..0, other.infos.drain(..));
    }

    pub(super) fn append(&mut self, other: &mut Self) {
        debug_assert!(self.len() + other.len() <= MAX_LEN);
        self.nodes.extend(other.nodes.drain(..));
        self.infos.extend(other.infos.drain(..));
    }

    pub(super) fn distribute(&mut self, other: &mut Self) {
        if self.len() < other.len() {
            let end = (other.len() - self.len()) / 2;
            self.nodes.extend(other.nodes.drain(..end));
            self.infos.extend(other.infos.drain(..end));
        } else if self.len() > other.len() {
            let start = (self.len() + other.len()) / 2;
            other.nodes.splice(..0, self.nodes.drain(start..));
            other.infos.splice(..0, self.infos.drain(start..));
        }
    }

    pub(super) fn prepend_distribute(&mut self, other: &mut Self) -> bool {
        if self.len() + other.len() <= MAX_LEN {
            self.prepend(other);
            true
        } else {
            other.distribute(self);
            false
        }
    }

    pub(super) fn append_distribute(&mut self, other: &mut Self) -> bool {
        if self.len() + other.len() <= MAX_LEN {
            self.append(other);
            true
        } else {
            self.distribute(other);
            false
        }
    }

    pub(super) fn split_off(&mut self, index: usize) -> Self {
        Self {
            nodes: self.nodes.split_off(index),
            infos: self.infos.split_off(index),
        }
    }

    pub(super) fn remove_from(&mut self, start: usize) {
        self.nodes.truncate(start);
        self.infos.truncate(start);
    }

    pub(super) fn remove_to(&mut self, end: usize) {
        self.nodes.drain(..end);
        self.infos.drain(..end);
    }

    pub(super) fn assert_valid(&self) {
        for node in &self.nodes {
            node.is_at_least_half_full();
            node.assert_valid();
        }
    }
}

impl<L> Branch<L>
where
    L: Leaf + Clone,
{
    pub(super) fn update_front<T>(&mut self, f: impl FnOnce(Option<&mut Node<L>>) -> T) -> T {
        match self.nodes.first_mut() {
            Some(mut front) => {
                let output = f(Some(Arc::make_mut(&mut front)));
                *self.infos.first_mut().unwrap() = self.nodes.first().unwrap().info();
                output
            }
            None => f(None),
        }
    }

    pub(super) fn update_back<T>(&mut self, f: impl FnOnce(Option<&mut Node<L>>) -> T) -> T {
        match self.nodes.last_mut() {
            Some(mut back) => {
                let output = f(Some(Arc::make_mut(&mut back)));
                *self.infos.last_mut().unwrap() = self.nodes.last().unwrap().info();
                output
            }
            None => f(None),
        }
    }
}

impl<L> fmt::Debug for Branch<L>
where
    L: Leaf + fmt::Debug,
    L::Info: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Branch")
            .field("nodes", &self.nodes)
            .field("infos", &self.infos)
            .finish()
    }
}
