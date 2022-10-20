use {
    crate::{
        Branch, Builder, Bytes, BytesRev, Chars, CharsRev, ChunkCursor, Chunks, ChunksRev, Cursor,
        Info, Leaf, Node, Slice,
    },
    std::{
        cmp::Ordering,
        hash::{Hash, Hasher},
        ops::RangeBounds,
    },
};

/// A rope data structure.
#[derive(Clone, Debug)]
pub struct Rope {
    height: usize,
    root: Node,
}

impl Rope {
    /// Creates a new empty `Rope`.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn new() -> Self {
        Self {
            height: 0,
            root: Node::Leaf(Leaf::new()),
        }
    }

    /// Returns `true` is `self` is empty.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn is_empty(&self) -> bool {
        self.byte_len() == 0
    }

    /// Returns the length of `self` in bytes.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn byte_len(&self) -> usize {
        self.root.info().byte_count
    }

    /// Returns the length of `self` in `char`s.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn char_len(&self) -> usize {
        self.root.info().char_count
    }

    /// Returns the length of `self` in lines.
    ///
    /// # Performance
    ///
    /// Runs in O(1) time.
    pub fn line_len(&self) -> usize {
        self.root.info().line_break_count + 1
    }

    /// Returns `true` if `byte_index` lies on a `char` boundary.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn is_char_boundary(&self, byte_index: usize) -> bool {
        if byte_index > self.byte_len() {
            return false;
        }
        if byte_index == 0 || byte_index == self.byte_len() {
            return true;
        }
        self.root.is_char_boundary(byte_index)
    }

    /// Converts the given `byte_index` to a `char` index.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_index` is greater than the length of `self` in bytes, or if it is does not
    /// lie on a `char` boundary.
    pub fn byte_to_char(&self, byte_index: usize) -> usize {
        self.info_at(byte_index).char_count
    }

    /// Converts the given `byte_index` to a line index.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_index` is greater than the length of `self` in bytes, or if it is does not
    /// lie on a `char` boundary.
    pub fn byte_to_line(&self, byte_index: usize) -> usize {
        self.info_at(byte_index).line_break_count + 1
    }

    /// Converts the given `char_index` to a byte index.
    ///
    /// # Performance
    ///  
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `char_index` is greater than the length of `self` in chars.
    pub fn char_to_byte(&self, char_index: usize) -> usize {
        if char_index == 0 {
            return 0;
        }
        if char_index == self.char_len() {
            return self.byte_len();
        }
        self.root.char_to_byte(char_index)
    }

    /// Converts the given `line_index` to a byte index.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `line_index` is greater than or equal to the length of `self` in lines.
    pub fn line_to_byte(&self, line_index: usize) -> usize {
        if line_index == 0 {
            return 0;
        }
        self.root.line_to_byte(line_index)
    }

    /// Returns the slice of `self` corresponding to the given `byte_range`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_range` is out of bounds.
    pub fn slice<R: RangeBounds<usize>>(&self, byte_range: R) -> Slice<'_> {
        let byte_range = crate::range_bounds_to_range(byte_range, self.byte_len());
        Slice::new(self, byte_range.start, byte_range.end)
    }

    /// Returns a [`ChunkCursor`] at the front chunk of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn chunk_cursor_front(&self) -> ChunkCursor<'_> {
        self.slice(..).chunk_cursor_front()
    }

    /// Returns a [`ChunkCursor`] at the back chunk of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn chunk_cursor_back(&self) -> ChunkCursor<'_> {
        self.slice(..).chunk_cursor_back()
    }

    /// Returns a [`ChunkCursor`] at chunk containing the given `byte_position` within `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_index` is greater than the length of `self` in bytes.
    pub fn chunk_cursor_at(&self, byte_position: usize) -> ChunkCursor<'_> {
        self.slice(..).chunk_cursor_at(byte_position)
    }

    /// Returns a [`Cursor`] at the front of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn cursor_front(&self) -> Cursor<'_> {
        self.slice(..).cursor_front()
    }

    /// Returns a [`Cursor`] at the back of `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn cursor_back(&self) -> Cursor<'_> {
        self.slice(..).cursor_back()
    }

    /// Returns a [`Cursor`] at the the given `byte_position` within `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_index` is greater than the length of `self` in bytes, or if it does not lie
    /// on a `char` boundary.
    pub fn cursor_at(&self, byte_position: usize) -> Cursor<'_> {
        self.slice(..).cursor_at(byte_position)
    }

    /// Returns an iterator over the chunks in `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn chunks(&self) -> Chunks<'_> {
        self.slice(..).chunks()
    }

    /// Returns a reverse iterator over the chunks in `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn chunks_rev(&self) -> ChunksRev<'_> {
        self.slice(..).chunks_rev()
    }

    /// Returns an iterator over the bytes in `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn bytes(&self) -> Bytes<'_> {
        self.slice(..).bytes()
    }

    /// Returns a reverse iterator over the bytes in `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn bytes_rev(&self) -> BytesRev<'_> {
        self.slice(..).bytes_rev()
    }

    /// Returns an iterator over the `char`s in `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn chars(&self) -> Chars<'_> {
        self.slice(..).chars()
    }

    /// Returns a reverse iterator over the `char`s in `self`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn chars_rev(&self) -> CharsRev<'_> {
        self.slice(..).chars_rev()
    }

    /// Appends `other` to `self`,
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    pub fn append(&mut self, mut other: Self) {
        use crate::StrUtils;

        if self.is_empty() {
            *self = other;
            return;
        }
        if other.is_empty() {
            return;
        }
        if self.root.chunk_back().last_is_cr() && other.root.chunk_front().first_is_lf() {
            self.truncate_back(self.byte_len() - 1);
            other.truncate_front(1);
            self.append_internal(Rope::from("\r\n"));
        }
        self.append_internal(other)
    }

    /// Splits `self` at the given `byte_index`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_index` is greater than the length of `self` in bytes, or if it does not lie
    /// on a `char` boundary.
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

    /// Truncates `self` at the front, keeping the byte range `byte_start..`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_index` is greater than the length of `self` in bytes, or if it does not lie
    /// on a `char` boundary.
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

    /// Truncates `self` at the back, keeping the byte range `..byte_end`.
    ///
    /// # Performance
    ///
    /// Runs in O(log(n)) time.
    ///
    /// # Panics
    ///
    /// Panics if `byte_index` is greater than the length of `self` in bytes, or if it dies not lie
    /// on a `char` boundary.
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

    pub(crate) fn root(&self) -> &Node {
        &self.root
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

    pub(crate) fn append_internal(&mut self, mut other: Self) {
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
}

#[cfg(fuzzing)]
impl Rope {
    pub fn assert_valid(&self) {
        match &self.root {
            Node::Branch(branch) => assert!(branch.len() >= 2),
            _ => {}
        }
        self.root.assert_valid(self.height);
    }
}

impl Eq for Rope {}

impl Ord for Rope {
    fn cmp(&self, other: &Self) -> Ordering {
        self.slice(..).cmp(&other.slice(..))
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
    fn from(string: &'a str) -> Self {
        use std::iter;

        iter::once(string).collect()
    }
}

impl<'a> From<&'a mut str> for Rope {
    fn from(string: &'a mut str) -> Self {
        Self::from(&*string)
    }
}

impl<'a> FromIterator<char> for Rope {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = char>,
    {
        let mut builder = Builder::new();
        for ch in iter.into_iter() {
            let mut buffer = [0; 4];
            builder.push_str(ch.encode_utf8(&mut buffer));
        }
        builder.build()
    }
}

impl<'a> FromIterator<&'a str> for Rope {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = &'a str>,
    {
        let mut builder = Builder::new();
        for string in iter.into_iter() {
            builder.push_str(string);
        }
        builder.build()
    }
}

impl Hash for Rope {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.slice(..).hash(state)
    }
}

impl PartialEq for Rope {
    fn eq(&self, other: &Self) -> bool {
        self.slice(..).eq(&other.slice(..))
    }
}

impl PartialOrd for Rope {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        self.slice(..).partial_cmp(&other.slice(..))
    }
}
