use {
    crate::{
        Branch, Builder, Bytes, BytesRev, Chars, CharsRev, Chunks, ChunksRev, Cursor, Info, Leaf,
        Node, Slice,
    },
    std::ops::RangeBounds,
};

#[derive(Clone, Debug)]
pub struct Rope {
    height: usize,
    root: Node,
}

impl Rope {
    pub fn new() -> Self {
        Self {
            height: 0,
            root: Node::Leaf(Leaf::new()),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.byte_len() == 0
    }

    pub fn byte_len(&self) -> usize {
        self.root.info().byte_count
    }

    pub fn char_len(&self) -> usize {
        self.root.info().char_count
    }

    pub fn line_len(&self) -> usize {
        self.root.info().line_break_count + 1
    }

    pub fn byte_to_char(&self, byte_index: usize) -> usize {
        self.info_at(byte_index).char_count
    }

    pub fn byte_to_line(&self, byte_index: usize) -> usize {
        self.info_at(byte_index).line_break_count + 1
    }

    pub fn char_to_byte(&self, char_index: usize) -> usize {
        if char_index == 0 {
            return 0;
        }
        if char_index == self.char_len() {
            return self.byte_len();
        }
        self.root.char_to_byte(char_index)
    }

    pub fn line_to_byte(&self, line_index: usize) -> usize {
        if line_index == 0 {
            return 0;
        }
        self.root.line_to_byte(line_index)
    }

    pub fn slice<R: RangeBounds<usize>>(&self, byte_range: R) -> Slice<'_> {
        let byte_range = crate::range_bounds_to_range(byte_range, self.byte_len());
        Slice::new(self, byte_range.start, byte_range.end)
    }

    pub fn cursor_front(&self) -> Cursor<'_> {
        self.slice(..).cursor_front()
    }

    pub fn cursor_back(&self) -> Cursor<'_> {
        self.slice(..).cursor_back()
    }

    pub fn cursor_at(&self, byte_index: usize) -> Cursor<'_> {
        self.slice(..).cursor_at(byte_index)
    }

    pub fn chunks(&self) -> Chunks<'_> {
        self.slice(..).chunks()
    }

    pub fn chunks_rev(&self) -> ChunksRev<'_> {
        self.slice(..).chunks_rev()
    }

    pub fn bytes(&self) -> Bytes<'_> {
        self.slice(..).bytes()
    }

    pub fn bytes_rev(&self) -> BytesRev<'_> {
        self.slice(..).bytes_rev()
    }

    pub fn chars(&self) -> Chars<'_> {
        self.slice(..).chars()
    }

    pub fn chars_rev(&self) -> CharsRev<'_> {
        self.slice(..).chars_rev()
    }

    pub fn append(&mut self, mut other: Self) {
        if self.is_empty() {
            *self = other;
            return;
        }
        if other.is_empty() {
            return;
        }
        if self.height < other.height {
            if let Some(node) = other
                .root
                .prepend_at_depth(self.root.clone(), other.height - self.height)
            {
                let mut branch = Branch::new();
                branch.push_front(other.root);
                branch.push_front(node);
                other.height += 1;
                other.root = Node::Branch(branch);
            }
            *self = other;
        } else {
            if let Some(node) = self
                .root
                .append_at_depth(other.root, self.height - other.height)
            {
                let mut branch = Branch::new();
                branch.push_back(self.root.clone());
                branch.push_back(node);
                self.height += 1;
                self.root = Node::Branch(branch);
            }
        }
    }

    pub fn split_off(&mut self, byte_index: usize) -> Self {
        use std::mem;

        if byte_index == 0 {
            return mem::replace(self, Self::new());
        }
        if byte_index == self.byte_len() {
            return Self::new();
        }
        let mut other_root = self.root.split_off(byte_index);
        let other_height = self.height - other_root.pull_up_singular_nodes();
        self.height -= self.root.pull_up_singular_nodes();
        Self {
            root: other_root,
            height: other_height,
        }
    }

    pub fn truncate_front(&mut self, byte_start: usize) {
        if byte_start == 0 {
            return;
        }
        if byte_start == self.byte_len() {
            *self = Self::new();
            return;
        }
        self.root.truncate_front(byte_start);
        self.height -= self.root.pull_up_singular_nodes();
    }

    pub fn truncate_back(&mut self, byte_end: usize) {
        if byte_end == 0 {
            *self = Self::new();
            return;
        }
        if byte_end == self.byte_len() {
            return;
        }
        self.root.truncate_back(byte_end);
        self.height -= self.root.pull_up_singular_nodes();
    }

    pub(crate) fn from_raw_parts(height: usize, root: Node) -> Self {
        Self { height, root }
    }

    pub(crate) fn info_at(&self, byte_index: usize) -> Info {
        if byte_index == 0 {
            return Info::new();
        }
        if byte_index == self.byte_len() {
            return self.root.info();
        }
        self.root.info_at(byte_index)
    }

    pub(crate) fn root(&self) -> &Node {
        &self.root
    }
}

impl<'a> From<String> for Rope {
    fn from(string: String) -> Self {
        Self::from(string.as_str())
    }
}

impl<'a> From<&'a String> for Rope {
    fn from(string: &'a String) -> Self {
        Self::from(string.as_str())
    }
}

impl<'a> From<&'a str> for Rope {
    fn from(string: &str) -> Self {
        let mut builder = Builder::new();
        builder.push_str(string);
        builder.build()
    }
}
