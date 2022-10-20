use {
    crate::{Branch, Leaf, Node, Rope},
    std::sync::Arc,
};

/// A builder for [`Rope`]s.
/// 
#[derive(Debug)]
pub struct Builder {
    stack: Vec<(usize, Vec<Node>)>,
    chunk: String,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            stack: Vec::new(),
            chunk: String::new(),
        }
    }

    /// Appends the given `string` to the [`Rope`] under construction.
    ///
    /// # Performance
    ///
    /// Runs in O(1) amortized and O(log(n)) worst-case time.
    pub fn push_str(&mut self, mut string: &str) {
        use crate::StrUtils;

        while !string.is_empty() {
            if string.len() <= Leaf::MAX_LEN - self.chunk.len() {
                self.chunk.push_str(string);
                break;
            }
            let mut index = Leaf::MAX_LEN - self.chunk.len();
            while !string.can_split_at(index) {
                index -= 1;
            }
            let (left_string, right_string) = string.split_at(index);
            self.chunk.push_str(left_string);
            string = right_string;
            let mut end = self.chunk.len();
            if self.chunk.last_is_cr() && string.first_is_lf() {
                end -= 1;
            }
            let chunk = self.chunk.drain(..end).collect::<String>();
            self.push_chunk(chunk);
        }
    }

    /// Finishes and then returns the [`Rope`] under construction.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub(crate) fn build(mut self) -> Rope {
        use std::mem;

        let mut btree = Rope::from_raw_parts(0, Node::Leaf(Leaf::from(Arc::new(self.chunk))));
        while let Some((height, nodes)) = self.stack.pop() {
            for root in nodes.into_iter().rev() {
                let mut other_btree = Rope::from_raw_parts(height, root);
                mem::swap(&mut btree, &mut other_btree);
                btree.append(other_btree);
            }
        }
        btree
    }

    fn push_chunk(&mut self, string: String) {
        let mut height = 0;
        let mut node = Node::Leaf(Leaf::from(Arc::new(string)));
        loop {
            if self
                .stack
                .last()
                .map_or(true, |&(last_height, _)| last_height != height)
            {
                self.stack.push((height, Vec::new()));
            }
            let (_, nodes) = self.stack.last_mut().unwrap();
            nodes.push(node);
            if nodes.len() < Branch::MAX_LEN {
                break;
            }
            let (_, nodes) = self.stack.pop().unwrap();
            height += 1;
            node = Node::Branch(Branch::from(Arc::new(nodes)));
        }
    }
}
