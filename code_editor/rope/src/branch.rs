use {
    crate::{Info, Node},
    std::{ops::Deref, slice, sync::Arc},
};

#[derive(Clone, Debug)]
pub(crate) struct Branch {
    info: Info,
    nodes: Arc<Vec<Node>>,
}

impl Branch {
    #[cfg(not(test))]
    pub(crate) const MAX_LEN: usize = 8;
    #[cfg(test)]
    pub(crate) const MAX_LEN: usize = 2;

    pub(crate) fn new() -> Self {
        Branch::from(Arc::new(Vec::new()))
    }

    pub(crate) fn info(&self) -> Info {
        self.info
    }

    pub(crate) fn as_nodes(&self) -> &[Node] {
        self.nodes.as_slice()
    }

    pub(crate) fn search_by_byte_only(&self, byte_count: &mut usize, byte_index: usize) -> usize {
        let mut index = 0;
        for node in self {
            let next_byte_count = *byte_count + node.info().byte_count;
            if byte_index < next_byte_count {
                break;
            }
            index += 1;
            *byte_count = next_byte_count;
        }
        index
    }

    pub(crate) fn search_by_byte(&self, info: &mut Info, byte_index: usize) -> usize {
        let mut index = 0;
        for node in self {
            let next_info = *info + node.info();
            if byte_index < next_info.byte_count {
                break;
            }
            index += 1;
            *info = next_info;
        }
        index
    }

    pub(crate) fn search_by_char(&self, info: &mut Info, char_index: usize) -> usize {
        let mut index = 0;
        for node in self {
            let next_info = *info + node.info();
            if char_index < next_info.char_count {
                break;
            }
            index += 1;
            *info = next_info;
        }
        index
    }

    pub(crate) fn search_by_line(&self, info: &mut Info, line_index: usize) -> usize {
        let mut index = 0;
        for node in self {
            let next_info = *info + node.info();
            if line_index <= next_info.line_break_count {
                break;
            }
            index += 1;
            *info = next_info;
        }
        index
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
        self.info -= Info::from(&self[..start]);
        Arc::make_mut(&mut self.nodes).drain(..start);
    }

    pub(crate) fn truncate_back(&mut self, end: usize) {
        self.info -= Info::from(&self[end..]);
        Arc::make_mut(&mut self.nodes).truncate(end);
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

#[cfg(fuzzing)]
impl Branch {
    pub(crate) fn assert_valid(&self, height: usize) {
        for node in self {
            assert!(node.is_at_least_half_full());
            node.assert_valid(height - 1);
        }
    }

    pub(crate) fn is_at_least_half_full(&self) -> bool {
        self.len() >= Self::MAX_LEN / 2
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
        self.as_nodes()
    }
}

impl<'a> IntoIterator for &'a Branch {
    type Item = &'a Node;
    type IntoIter = slice::Iter<'a, Node>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}
