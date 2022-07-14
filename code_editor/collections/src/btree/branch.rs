use {
    super::{Chunk, Node},
    std::sync::Arc,
};

#[derive(Clone)]
pub struct Branch<T: Chunk> {
    info: T::Info,
    nodes: Arc<Vec<Node<T>>>,
}

impl<T: Chunk> Branch<T> {
    const MAX_LEN: usize = 8;

    pub fn new() -> Self {
        unimplemented!()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn info(&self) -> T::Info {
        self.info
    }

    pub fn push_front_and_maybe_split(&mut self, node: Node<T>) -> Option<Self> {
        if self.len() < Self::MAX_LEN {
            self.push_front(node);
            return None;
        }
        let mut other = Self::new();
        other.move_left(self, self.len() / 2);
        other.push_front(node);
        Some(other)
    }

    pub fn push_front(&mut self, node: Node<T>) {
        assert!(self.len() < Self::MAX_LEN);
        self.info += node.info();
        Arc::make_mut(&mut self.nodes).insert(0, node);
    }

    pub fn push_back_and_maybe_split(&mut self, node: Node<T>) -> Option<Self> {
        if self.len() < Self::MAX_LEN {
            self.push_back(node);
            return None;
        }
        let mut other = Self::new();
        self.move_right(&mut other, self.len() / 2);
        Some(other)
    }

    pub fn push_back(&mut self, node: Node<T>) {
        assert!(self.len() < Self::MAX_LEN);
        self.info += node.info();
        Arc::make_mut(&mut self.nodes).push(node);
    }

    pub fn pop_front(&mut self) -> Option<Node<T>> {
        if self.is_empty() {
            return None;
        }
        let node = Arc::make_mut(&mut self.nodes).pop().unwrap();
        self.info -= node.info();
        Some(node)
    }

    pub fn pop_back(&mut self) -> Option<Node<T>> {
        if self.is_empty() {
            return None;
        }
        let node = Arc::make_mut(&mut self.nodes).remove(0);
        self.info -= node.info();
        Some(node)
    }

    pub fn prepend_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            other.move_right(self, self.len());
            return None;
        }
        other.distribute(self);
        Some(other)
    }

    pub fn append_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            let other_len = other.len();
            self.move_left(&mut other, other_len);
            return None;
        }
        self.distribute(&mut other);
        Some(other)
    }

    fn distribute(&mut self, other: &mut Self) {
        use std::cmp::Ordering;

        match self.len().cmp(&other.len()) {
            Ordering::Less => self.move_right(other, (other.len() - self.len()) / 2),
            Ordering::Greater => self.move_left(other, (self.len() + other.len()) / 2),
            _ => {}
        }
    }

    fn move_left(&mut self, other: &mut Self, end: usize) {
        let info = T::Info::from_nodes(&other.nodes[..end]);
        other.info -= info;
        self.info += info;
        let nodes = Arc::make_mut(&mut other.nodes).drain(..end);
        Arc::make_mut(&mut self.nodes).extend(nodes);
    }

    fn move_right(&mut self, other: &mut Self, start: usize) {
        let info = T::Info::from_nodes(&self.nodes[start..]);
        self.info -= info;
        other.info += info;
        let nodes = Arc::make_mut(&mut self.nodes).drain(start..);
        Arc::make_mut(&mut other.nodes).splice(..0, nodes);
    }
}
