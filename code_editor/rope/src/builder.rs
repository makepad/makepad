use {
    crate::{Branch, Leaf, Node, Rope},
    std::sync::Arc,
};

#[derive(Debug)]
pub struct Builder {
    stack: Vec<(usize, Vec<Node>)>,
    string: String,
}

impl Builder {
    pub fn new() -> Self {
        Self {
            stack: Vec::new(),
            string: String::new(),
        }
    }

    pub fn push_str(&mut self, mut string: &str) {
        while !string.is_empty() {
            if string.len() <= Leaf::MAX_LEN - self.string.len() {
                self.string.push_str(string);
                break;
            }
            let mut index = Leaf::MAX_LEN - self.string.len();
            while !string.is_char_boundary(index) {
                index -= 1;
            }
            let (left_string, right_string) = string.split_at(index);
            self.string.push_str(left_string);
            string = right_string;
            self.push_chunk();
        }
    }

    pub(crate) fn build(mut self) -> Rope {
        use std::mem;

        self.push_chunk();
        let mut btree = Rope::new();
        while let Some((height, nodes)) = self.stack.pop() {
            for root in nodes.into_iter().rev() {
                let mut other_btree = Rope::from_raw_parts(height, root);
                mem::swap(&mut btree, &mut other_btree);
                btree.append(other_btree);
            }
        }
        btree
    }

    fn push_chunk(&mut self) {
        let mut height = 0;
        let mut node = Node::Leaf(Leaf::from(Arc::new(self.string.split_off(0))));
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
