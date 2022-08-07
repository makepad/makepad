use crate::{Branch, Info, Leaf};

#[derive(Clone, Debug)]
pub(crate) enum Node {
    Leaf(Leaf),
    Branch(Branch),
}

impl Node {
    pub(crate) fn as_leaf(&self) -> &Leaf {
        match self {
            Self::Leaf(leaf) => leaf,
            _ => panic!(),
        }
    }

    pub(crate) fn info(&self) -> Info {
        match self {
            Self::Leaf(leaf) => leaf.info(),
            Self::Branch(branch) => branch.info(),
        }
    }

    pub(crate) fn is_char_boundary(&self, byte_index: usize) -> bool {
        let (chunk, start_info) = self.chunk_at_byte(byte_index);
        chunk.is_char_boundary(byte_index - start_info.byte_count)
    }

    pub(crate) fn info_at(&self, byte_index: usize) -> Info {
        let (chunk, start_info) = self.chunk_at_byte(byte_index);
        start_info + Info::from(&chunk[..byte_index - start_info.byte_count])
    }

    pub(crate) fn char_to_byte(&self, char_index: usize) -> usize {
        use crate::StrUtils;

        let (chunk, start_info) = self.chunk_at_char(char_index);
        start_info.byte_count + chunk.char_to_byte(char_index - start_info.char_count)
    }

    pub(crate) fn line_to_byte(&self, line_index: usize) -> usize {
        use crate::StrUtils;

        let (chunk, start_info) = self.chunk_at_line(line_index);
        start_info.byte_count + chunk.line_to_byte(line_index - start_info.line_break_count)
    }

    pub(crate) fn chunk_front(&self) -> &str {
        let mut node = self;
        loop {
            match node {
                Node::Leaf(leaf) => break leaf,
                Node::Branch(branch) => node = branch.first().unwrap(),
            }
        }
    }

    pub(crate) fn chunk_back(&self) -> &str {
        let mut node = self;
        loop {
            match node {
                Node::Leaf(leaf) => break leaf,
                Node::Branch(branch) => node = branch.last().unwrap(),
            }
        }
    }

    pub(crate) fn chunk_at_byte(&self, byte_index: usize) -> (&str, Info) {
        let mut start_info = Info::new();
        let mut node = self;
        loop {
            match node {
                Node::Leaf(leaf) => break (leaf, start_info),
                Node::Branch(branch) => {
                    node = &branch[branch.search_by_byte(&mut start_info, byte_index)]
                }
            }
        }
    }

    pub(crate) fn chunk_at_char(&self, char_index: usize) -> (&str, Info) {
        let mut start_info = Info::new();
        let mut node = self;
        loop {
            match node {
                Node::Leaf(leaf) => break (leaf, start_info),
                Node::Branch(branch) => {
                    node = &branch[branch.search_by_char(&mut start_info, char_index)]
                }
            }
        }
    }

    pub(crate) fn chunk_at_line(&self, line_index: usize) -> (&str, Info) {
        let mut start_info = Info::new();
        let mut node = self;
        loop {
            match node {
                Node::Leaf(leaf) => break (leaf, start_info),
                Node::Branch(branch) => {
                    node = &branch[branch.search_by_line(&mut start_info, line_index)]
                }
            }
        }
    }

    pub(crate) fn prepend_at_depth(&mut self, mut other: Node, depth: usize) -> Option<Self> {
        use std::mem;

        if depth == 0 {
            mem::swap(self, &mut other);
            let mut node = self.append_or_distribute(other)?;
            mem::swap(self, &mut node);
            Some(node)
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

    pub(crate) fn split_off(&mut self, byte_index: usize) -> Self {
        match self {
            Self::Leaf(leaf) => Node::Leaf(leaf.split_off(byte_index)),
            Self::Branch(branch) => {
                let mut byte_count = 0;
                let index = branch.search_by_byte_only(&mut byte_count, byte_index);
                if byte_index == byte_count {
                    return Node::Branch(branch.split_off(index));
                }
                let mut other_branch = branch.split_off(index + 1);
                let mut node = branch.pop_back().unwrap();
                let mut other_node = node.split_off(byte_index - byte_count);
                if branch.is_empty() {
                    branch.push_back(node)
                } else {
                    let count = node.pull_up_singular_nodes();
                    self.append_at_depth(node, count + 1);
                }
                if other_branch.is_empty() {
                    other_branch.push_front(other_node);
                    Node::Branch(other_branch)
                } else {
                    let count = other_node.pull_up_singular_nodes();
                    let mut other = Node::Branch(other_branch);
                    other.prepend_at_depth(other_node, count + 1);
                    other
                }
            }
        }
    }

    pub(crate) fn truncate_front(&mut self, byte_start: usize) {
        match self {
            Self::Leaf(leaf) => leaf.truncate_front(byte_start),
            Self::Branch(branch) => {
                let mut byte_count = 0;
                let index = branch.search_by_byte_only(&mut byte_count, byte_start);
                if byte_start == byte_count {
                    branch.truncate_front(index);
                } else {
                    branch.truncate_front(index);
                    let mut node = branch.pop_front().unwrap();
                    node.truncate_front(byte_start - byte_count);
                    if branch.is_empty() {
                        branch.push_front(node);
                    } else {
                        let count = node.pull_up_singular_nodes();
                        self.prepend_at_depth(node, count + 1);
                    }
                }
            }
        }
    }

    pub(crate) fn truncate_back(&mut self, byte_end: usize) {
        match self {
            Self::Leaf(leaf) => leaf.truncate_back(byte_end),
            Self::Branch(branch) => {
                let mut byte_count = 0;
                let index = branch.search_by_byte_only(&mut byte_count, byte_end);
                if byte_end == byte_count {
                    branch.truncate_back(index);
                } else {
                    branch.truncate_back(index + 1);
                    let mut node = branch.pop_back().unwrap();
                    node.truncate_back(byte_end - byte_count);
                    if branch.is_empty() {
                        branch.push_back(node);
                    } else {
                        let count = node.pull_up_singular_nodes();
                        self.append_at_depth(node, count + 1);
                    }
                }
            }
        }
    }

    pub(crate) fn pull_up_singular_nodes(&mut self) -> usize {
        let mut count = 0;
        loop {
            match self {
                Node::Branch(branch) if branch.len() == 1 => {
                    *self = branch.pop_back().unwrap();
                    count += 1;
                }
                _ => break,
            }
        }
        count
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

#[cfg(fuzzing)]
impl Node {
    pub(crate) fn assert_valid(&self, height: usize) {
        match self {
            Self::Leaf(leaf) => leaf.assert_valid(height),
            Self::Branch(branch) => branch.assert_valid(height),
        }
    }

    pub(crate) fn is_at_least_half_full(&self) -> bool {
        match self {
            Self::Leaf(leaf) => leaf.is_at_least_half_full(),
            Self::Branch(branch) => branch.is_at_least_half_full(),
        }
    }
}
