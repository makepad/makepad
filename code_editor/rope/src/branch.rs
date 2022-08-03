use {
    crate::{Info, Node},
    std::{ops::Deref, sync::Arc},
};

#[derive(Clone)]
pub(crate) struct Branch {
    info: Info,
    nodes: Arc<Vec<Node>>,
}

impl Branch {
    pub(crate) const MAX_LEN: usize = 2;

    pub(crate) fn new() -> Self {
        Branch::from(Arc::new(Vec::new()))
    }

    pub(crate) fn info(&self) -> Info {
        self.info
    }

    pub(crate) fn push_front_and_maybe_split(&mut self, node: Node) -> Option<Self> {
        use std::mem;

        if self.len() < Self::MAX_LEN {
            self.push_front(node);
            None
        } else {
            let mut other = self.split_off(self.len() / 2);
            mem::swap(self, &mut other);
            other.push_front(node);
            Some(other)
        }
    }

    pub(crate) fn push_front(&mut self, node: Node) {
        self.info += node.info();
        Arc::make_mut(&mut self.nodes).insert(0, node);
    }

    pub(crate) fn push_back_and_maybe_split(&mut self, node: Node) -> Option<Self> {
        if self.len() < Self::MAX_LEN {
            self.push_back(node);
            None
        } else {
            let mut other = self.split_off(self.len() / 2);
            other.push_back(node);
            Some(other)
        }
    }

    pub(crate) fn push_back(&mut self, node: Node) {
        self.info += node.info();
        Arc::make_mut(&mut self.nodes).push(node);
    }

    pub(crate) fn pop_front(&mut self) -> Option<Node> {
        if self.is_empty() {
            return None;
        }
        let node = Arc::make_mut(&mut self.nodes).remove(0);
        self.info -= node.info();
        Some(node)
    }

    pub(crate) fn pop_back(&mut self) -> Option<Node> {
        if self.is_empty() {
            return None;
        }
        let node = Arc::make_mut(&mut self.nodes).pop().unwrap();
        self.info -= node.info();
        Some(node)
    }

    pub(crate) fn update_front<T, F>(&mut self, f: F) -> T
    where
        F: FnOnce(&mut Node) -> T,
    {
        let mut node = self.pop_front().unwrap();
        let output = f(&mut node);
        self.push_front(node);
        output
    }

    pub(crate) fn update_back<T, F>(&mut self, f: F) -> T
    where
        F: FnOnce(&mut Node) -> T,
    {
        let mut node = self.pop_back().unwrap();
        let output = f(&mut node);
        self.push_back(node);
        output
    }

    pub(crate) fn prepend_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            self.prepend(other);
            None
        } else {
            other.distribute(self);
            Some(other)
        }
    }

    pub(crate) fn append_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            self.append(other);
            None
        } else {
            self.distribute(&mut other);
            Some(other)
        }
    }

    pub(crate) fn split_off(&mut self, at: usize) -> Self {
        let mut other = Self::new();
        self.shift_right(&mut other, at);
        other
    }

    pub(crate) fn truncate_front(&mut self, start: usize) {
        Arc::make_mut(&mut self.nodes).drain(..start);
    }

    pub(crate) fn truncate_back(&mut self, end: usize) {
        Arc::make_mut(&mut self.nodes).truncate(end);
    }

    fn prepend(&mut self, mut other: Self) {
        other.shift_right(self, 0);
    }

    fn append(&mut self, mut other: Self) {
        let other_len = other.len();
        self.shift_left(&mut other, other_len);
    }

    fn distribute(&mut self, other: &mut Self) {
        use std::cmp::Ordering;

        match self.len().cmp(&other.len()) {
            Ordering::Less => self.shift_left(other, (other.len() - self.len()) / 2),
            Ordering::Greater => self.shift_right(other, (self.len() + other.len()) / 2),
            _ => {}
        }
    }

    fn shift_left(&mut self, other: &mut Self, end: usize) {
        let info = Info::from(&other[..end]);
        other.info -= info;
        self.info += info;
        let nodes = Arc::make_mut(&mut other.nodes).drain(..end);
        Arc::make_mut(&mut self.nodes).extend(nodes);
    }

    fn shift_right(&mut self, other: &mut Self, start: usize) {
        let info = Info::from(&self[start..]);
        self.info -= info;
        other.info += info;
        let nodes = Arc::make_mut(&mut self.nodes).drain(start..);
        Arc::make_mut(&mut other.nodes).splice(..0, nodes);
    }
}

impl From<Arc<Vec<Node>>> for Branch {
    fn from(nodes: Arc<Vec<Node>>) -> Self {
        Self {
            info: Info::from(nodes.as_slice()),
            nodes,
        }
    }
}

impl Deref for Branch {
    type Target = [Node];

    fn deref(&self) -> &Self::Target {
        self.nodes.as_slice()
    }
}
