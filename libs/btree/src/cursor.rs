use {
    crate::{branch, Branch, Info, Leaf, Node},
    array_vec::ArrayVec,
    std::fmt,
};

const STACK_SIZE: usize = usize::MAX.ilog2().div_ceil(branch::MAX_LEN.ilog2()) as usize;

#[derive(Clone)]
pub struct Cursor<'a, L>
where
    L: Leaf,
{
    root: Option<&'a Node<L>>,
    start: usize,
    end: usize,
    stack: ArrayVec<(&'a Branch<L>, usize), STACK_SIZE>,
    current_start: usize,
    current_end: usize,
    base: usize,
    offset: usize,
}

impl<'a, L> Cursor<'a, L>
where
    L: Leaf,
{
    pub(super) fn new(root: Option<&'a Node<L>>, start: usize, end: usize, index: usize) -> Self {
        let mut cursor = Self {
            root,
            start,
            end,
            stack: ArrayVec::new(),
            current_start: 0,
            current_end: 0,
            base: 0,
            offset: 0,
        };
        if let Some(root) = cursor.root {
            if index == root.info().len() {
                cursor.base = index;
                while let Some(branch) = cursor.current_node().unwrap().as_branch() {
                    cursor.stack.push((branch, branch.len() - 1));
                }
                cursor.base = root.info().len() - cursor.current_leaf().unwrap().len();
            } else {
                while let Some(branch) = cursor.current_node().unwrap().as_branch() {
                    for (node_index, node) in branch.nodes().iter().enumerate() {
                        if index < cursor.base + node.info().len() {
                            cursor.stack.push((branch, node_index));
                            break;
                        }
                        cursor.base += node.info().len();
                    }
                }
            }
            cursor.update_current();
            cursor.offset = index - cursor.base;
        }
        cursor
    }

    pub(super) fn start(root: Option<&'a Node<L>>, start: usize, end: usize) -> Self {
        Self::new(root, start, end, start)
    }

    pub(super) fn end(root: Option<&'a Node<L>>, start: usize, end: usize) -> Self {
        Self::new(root, start, end, end)
    }

    pub fn is_start(&self) -> bool {
        self.index() == self.start
    }

    pub fn is_end(&self) -> bool {
        self.index() == self.end
    }

    pub fn index(&self) -> usize {
        self.base + self.offset
    }

    pub fn offset(&self) -> usize {
        self.offset
    }

    pub fn current(&self) -> (Option<&'a L>, usize, usize) {
        (self.current_leaf(), self.current_start, self.current_end)
    }

    pub fn move_next_chunk(&mut self) -> bool {
        if self.is_end() {
            return false;
        }
        self.offset = self.current_end;
        if !self.is_end() {
            self.move_next_leaf();
            self.offset = 0;
        }
        true
    }

    pub fn move_next(&mut self) -> bool {
        if self.offset + 1 < self.current_end {
            self.offset += 1;
            return true;
        }
        if self.is_end() {
            return false;
        }
        self.offset += 1;
        if !self.is_end() {
            self.move_next_leaf();
            self.offset = 0;
        }
        true
    }

    pub fn move_prev_chunk(&mut self) -> bool {
        if self.is_start() {
            return false;
        }
        if self.offset == self.current_start {
            self.move_prev_leaf();
        }
        self.offset = self.current_start;
        true
    }

    pub fn move_prev(&mut self) -> bool {
        if self.offset > self.current_start {
            self.offset -= 1;
            return true;
        }
        if self.is_start() {
            return false;
        }
        self.move_prev_leaf();
        self.offset = self.current_end - 1;
        true
    }

    fn current_node(&self) -> Option<&'a Node<L>> {
        self.stack
            .last()
            .map(|(branch, index)| &*branch.nodes()[*index])
            .or_else(|| self.root)
    }

    fn current_leaf(&self) -> Option<&'a L> {
        self.current_node().map(|node| node.as_leaf().unwrap())
    }

    fn move_prev_leaf(&mut self) {
        while let Some((_, index)) = self.stack.last_mut() {
            if *index > 0 {
                *index -= 1;
                break;
            }
            self.stack.pop();
        }
        while let Some(branch) = self.current_node().unwrap().as_branch() {
            self.stack.push((branch, branch.len() - 1));
        }
        self.base -= self.current_leaf().unwrap().len();
        self.update_current();
    }

    fn move_next_leaf(&mut self) {
        self.base += self.current_leaf().unwrap().len();
        while let Some((branch, index)) = self.stack.last_mut() {
            if *index < branch.len() - 1 {
                *index += 1;
                break;
            }
            self.stack.pop();
        }
        while let Some(branch) = self.current_node().unwrap().as_branch() {
            self.stack.push((branch, 0));
        }
        self.update_current();
    }

    fn update_current(&mut self) {
        self.current_start = self.base.max(self.start) - self.base;
        self.current_end =
            (self.base + self.current_leaf().unwrap().len()).min(self.end) - self.base;
    }
}

impl<L> fmt::Debug for Cursor<'_, L>
where
    L: Leaf + fmt::Debug,
    L::Info: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cursor")
            .field("root", &self.root)
            .field("start", &self.start)
            .field("end", &self.end)
            .field("stack", &self.stack)
            .field("current_start", &self.current_start)
            .field("current_end", &self.current_end)
            .field("base", &self.base)
            .field("offset", &self.offset)
            .finish()
    }
}
