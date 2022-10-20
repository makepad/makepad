use crate::{Branch, Node};

/// A cursor over the chunks of a [`Rope`] or [`Slice`].
///
/// [`Rope`]: crate::Rope
/// [`Slice`]: crate::Slice
#[derive(Clone, Debug)]
pub struct ChunkCursor<'a> {
    root: &'a Node,
    byte_start: usize,
    byte_end: usize,
    byte_position: usize,
    path: Vec<(&'a Branch, usize)>,
}

impl<'a> ChunkCursor<'a> {
    /// Returns `true` if `self` is currently pointing to the front chunk of the [`Rope`] or
    /// [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    ///
    /// [`Rope`]: crate::Rope
    /// [`Slice`]: crate::Slice
    #[inline]
    pub fn is_at_front(&self) -> bool {
        self.byte_position <= self.byte_start
    }

    /// Returns `true` if `self` is currently pointing to the back chunk of the [`Rope`] or
    /// [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    ///
    /// [`Rope`]: crate::Rope
    /// [`Slice`]: crate::Slice
    #[inline]
    pub fn is_at_back(&self) -> bool {
        self.byte_position + self.current_node().as_leaf().len() >= self.byte_end
    }

    /// Returns the byte position of `self` within the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    ///
    /// [`Rope`]: crate::Rope
    /// [`Slice`]: crate::Slice
    #[inline]
    pub fn byte_position(&self) -> usize {
        self.byte_position.saturating_sub(self.byte_start)
    }

    /// Returns a reference to the chunk that `self` is currently pointing to.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    #[inline]
    pub fn current(&self) -> &'a str {
        let leaf = self.current_node().as_leaf();
        let start = self.byte_start.saturating_sub(self.byte_position);
        let end = leaf.len() - (self.byte_position + leaf.len()).saturating_sub(self.byte_end);
        &leaf[start..end]
    }

    /// Moves `self` to the next chunk of the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(log n) time.
    ///
    /// # Panics
    ///
    /// Panics if `self` is currently pointing to the back of the [`Rope`] or [`Slice`].
    ///
    /// [`Rope`]: crate::Rope
    /// [`Slice`]: crate::Slice
    pub fn move_next(&mut self) {
        assert!(!self.is_at_back());
        self.byte_position += self.current_node().as_leaf().len();
        while let Some((branch, index)) = self.path.last_mut() {
            if *index < branch.len() - 1 {
                *index += 1;
                break;
            }
            self.path.pop();
        }
        self.descend_left();
    }

    /// Moves `self` to the previous chunk of the [`Rope`] or [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in amortized O(1) and worst-case O(log n) time.
    ///
    /// # Panics
    ///
    /// Panics if `self` is currently pointing to the front of the [`Rope`] or [`Slice`].
    ///
    /// [`Rope`]: crate::Rope
    /// [`Slice`]: crate::Slice
    pub fn move_prev(&mut self) {
        assert!(!self.is_at_front());
        while let Some((branch, index)) = self.path.last_mut() {
            if *index > 0 {
                *index -= 1;
                self.byte_position -= branch[*index].info().byte_count;
                break;
            }
            self.path.pop();
        }
        self.descend_right();
    }

    /// Moves `self` to the chunk containing the given `byte_position` within the [`Rope`] or
    /// [`Slice`].
    ///
    /// # Performance
    ///
    /// Runs in O(log n) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_position` is greater than the length of the [`Rope`] or [`Slice`] in bytes.
    ///
    /// [`Rope`]: crate::Rope
    /// [`Slice`]: crate::Slice
    #[inline]
    pub fn move_to(&mut self, byte_position: usize) {
        let byte_position = self.byte_start + byte_position;
        assert!(byte_position <= self.byte_end);
        if byte_position == 0 {
            self.byte_position = 0;
            self.path.clear();
            self.descend_left();
        } else if byte_position == self.root.info().byte_count {
            self.byte_position = 0;
            self.path.clear();
            self.descend_right();
        } else {
            while self.byte_position > byte_position
                || self.byte_position + self.current_node().info().byte_count <= byte_position
            {
                let (branch, index) = self.path.pop().unwrap();
                for node in &branch[..index] {
                    self.byte_position -= node.info().byte_count;
                }
            }
            self.descend_to(byte_position);
        }
    }

    pub(crate) fn front(root: &'a Node, byte_start: usize, byte_end: usize) -> Self {
        let mut cursor = ChunkCursor::new(root, byte_start, byte_end);
        if byte_start == 0 {
            cursor.descend_left();
        } else if byte_start == root.info().byte_count {
            cursor.descend_right();
            cursor.byte_position = root.info().byte_count;
        } else {
            cursor.descend_to(byte_start);
        }
        cursor
    }

    pub(crate) fn back(root: &'a Node, byte_start: usize, byte_end: usize) -> Self {
        let mut cursor = ChunkCursor::new(root, byte_start, byte_end);
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
        byte_position: usize,
    ) -> Self {
        let mut cursor = ChunkCursor::new(root, byte_start, byte_end);
        if byte_position == 0 {
            cursor.descend_left();
        } else if byte_position == root.info().byte_count {
            cursor.descend_right();
        } else {
            cursor.descend_to(byte_position);
        }
        cursor
    }

    fn new(root: &'a Node, start: usize, end: usize) -> Self {
        Self {
            root,
            byte_start: start,
            byte_end: end,
            byte_position: 0,
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
                    self.byte_position += branch.info().byte_count - last.info().byte_count;
                    self.path.push((branch, branch.len() - 1));
                    node = last;
                }
            }
        }
    }

    fn descend_to(&mut self, byte_position: usize) {
        let mut node = self.current_node();
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    let index = branch.search_by_byte_only(&mut self.byte_position, byte_position);
                    self.path.push((branch, index));
                    node = &branch[index];
                }
            }
        }
    }
}
