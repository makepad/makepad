use crate::{Branch, Node};

#[derive(Clone, Debug)]
pub struct Cursor<'a> {
    root: &'a Node,
    byte_start: usize,
    byte_end: usize,
    byte_index: usize,
    path: Vec<(&'a Branch, usize)>,
}

impl<'a> Cursor<'a> {
    pub fn is_at_front(&self) -> bool {
        self.byte_index <= self.byte_start
    }

    pub fn is_at_back(&self) -> bool {
        self.byte_index + self.current_node().as_leaf().len() >= self.byte_end
    }

    pub fn byte_index(&self) -> usize {
        self.byte_index.saturating_sub(self.byte_start)
    }

    pub fn current(&self) -> &'a str {
        let leaf = self.current_node().as_leaf();
        let start = self.byte_start.saturating_sub(self.byte_index);
        let end = leaf.len() - (self.byte_index + leaf.len()).saturating_sub(self.byte_end);
        &leaf[start..end]
    }

    pub fn move_next(&mut self) {
        self.byte_index += self.current_node().as_leaf().len();
        while let Some((branch, index)) = self.path.last_mut() {
            if *index < branch.len() - 1 {
                *index += 1;
                break;
            }
            self.path.pop();
        }
        self.descend_left();
    }

    pub fn move_prev(&mut self) {
        while let Some((branch, index)) = self.path.last_mut() {
            if *index > 0 {
                self.byte_index -= branch[*index].info().byte_count;
                *index -= 1;
                break;
            }
            self.path.pop();
        }
        self.descend_right();
    }

    pub(crate) fn front(root: &'a Node, byte_start: usize, byte_end: usize) -> Self {
        let mut cursor = Cursor::new(root, byte_start, byte_end);
        if byte_start == 0 {
            cursor.descend_left();
        } else if byte_start == root.info().byte_count {
            cursor.descend_right();
            cursor.byte_index = root.info().byte_count;
        } else {
            cursor.descend_to(byte_start);
        }
        cursor
    }

    pub(crate) fn back(root: &'a Node, byte_start: usize, byte_end: usize) -> Self {
        let mut cursor = Cursor::new(root, byte_start, byte_end);
        if byte_end == 0 {
            cursor.descend_left();
        } else if byte_end == root.info().byte_count {
            cursor.descend_right();
        } else {
            cursor.descend_to(byte_end);
        }
        cursor
    }

    pub(crate) fn at(
        root: &'a Node,
        byte_start: usize,
        byte_end: usize,
        byte_index: usize,
    ) -> Self {
        let mut cursor = Cursor::new(root, byte_start, byte_end);
        if byte_index == 0 {
            cursor.descend_left();
        }
        if byte_index == root.info().byte_count {
            cursor.descend_right();
        } else {
            cursor.descend_to(byte_index);
        }
        cursor
    }

    fn new(root: &'a Node, start: usize, end: usize) -> Self {
        Self {
            root,
            byte_start: start,
            byte_end: end,
            byte_index: 0,
            path: Vec::new(),
        }
    }

    fn current_node(&self) -> &'a Node {
        self.path
            .last()
            .map_or(self.root, |&(branch, index)| &branch[index])
    }

    fn descend_left(&mut self) {
        let mut node = self.current_node();
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    self.path.push((branch, 0));
                    node = branch.first().unwrap();
                }
            }
        }
    }

    fn descend_right(&mut self) {
        let mut node = self.current_node();
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    let last = branch.last().unwrap();
                    self.byte_index += branch.info().byte_count - last.info().byte_count;
                    self.path.push((branch, branch.len() - 1));
                    node = last;
                }
            }
        }
    }

    fn descend_to(&mut self, byte_index: usize) {
        let mut node = self.current_node();
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    let index = branch.search_by_byte_only(&mut self.byte_index, byte_index);
                    self.path.push((branch, index));
                    node = &branch[index];
                }
            }
        }
    }
}
