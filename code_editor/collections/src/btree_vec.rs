use {crate::{btree, BTree}, std::{ops::{Add, AddAssign, Range, RangeBounds, Sub, SubAssign}}};

#[derive(Clone)]
pub struct BTreeVec<T> {
    btree: BTree<Vec<T>, Info>,
}

impl<T: Clone> BTreeVec<T> {
    pub fn new() -> Self {
        Self {
            btree: BTree::new()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.btree.is_empty()
    }

    pub fn len(&self) -> usize {
        self.btree.len()
    }

    pub fn slice<R: RangeBounds<usize>>(&self, range: R) -> Slice<'_, T> {
        Slice {
            slice: self.btree.slice(range),
        }
    }

    pub fn cursor_front(&self) -> Cursor<'_, T> {
        self.slice(..).cursor_front()
    }

    pub fn cursor_back(&self) -> Cursor<'_, T> {
        self.slice(..).cursor_back()
    }

    pub fn cursor_at(&self, position: usize) -> Cursor<'_, T> {
        self.slice(..).cursor_at(position)
    }

    pub fn chunks(&self) -> Chunks<'_, T> {
        self.slice(..).chunks()
    }

    pub fn chunks_rev(&self) -> ChunksRev<'_, T> {
        self.slice(..).chunks_rev()
    }

    pub fn replace_range<R: RangeBounds<usize>>(&mut self, range: R, replace_with: Self) {
        let range = btree::range(range, self.len());
        if range.is_empty() {
            let other = self.split_off(range.start);
            self.append(replace_with);
            self.append(other);
        } else {
            let mut other = self.clone();
            self.truncate_back(range.start);
            other.truncate_front(range.end);
            self.append(replace_with);
            self.append(other);
        }
    }

    pub fn append(&mut self, other: Self) {
        self.btree.append(other.btree);
    }

    pub fn split_off(&mut self, at: usize) -> Self {
        Self {
            btree: self.btree.split_off(at),
        }
    }

    pub fn truncate_front(&mut self, start: usize) {
        self.btree.truncate_front(start)
    }

    pub fn truncate_back(&mut self, end: usize) {
        self.btree.truncate_back(end)
    }
}

pub struct Slice<'a, T> {
    slice: btree::Slice<'a, Vec<T>, Info>,
}

impl<'a, T: Clone> Slice<'a, T> {
    pub fn cursor_front(self) -> Cursor<'a, T> {
        let cursor = self.slice.cursor_front();
        let (current, range) = cursor.current();
        Cursor {
            cursor,
            current: &current[range],
            index: 0,
        }
    }

    pub fn cursor_back(self) -> Cursor<'a, T> {
        let cursor = self.slice.cursor_back();
        let (current, range) = cursor.current();
        let current = &current[range];
        let index = current.len();
        Cursor {
            cursor,
            current,
            index,
        }
    }

    pub fn cursor_at(self, position: usize) -> Cursor<'a, T> {
        let cursor = self.slice.cursor_at(position);
        let (current, range) = cursor.current();
        let current = &current[range];
        let index = position - cursor.position();
        Cursor {
            cursor,
            current,
            index,
        }
    }

    pub fn chunks(self) -> Chunks<'a, T> {
        Chunks {
            cursor: self.cursor_front(),
        }
    }

    pub fn chunks_rev(self) -> ChunksRev<'a, T> {
        ChunksRev {
            cursor: self.cursor_back(),
        }
    }
}

pub struct Cursor<'a, T> {
    cursor: btree::Cursor<'a, Vec<T>, Info>,
    current: &'a [T],
    index: usize
}

impl<'a, T: Clone> Cursor<'a, T> {
    pub fn is_at_front(&self) -> bool {
        self.index == 0 && self.cursor.is_at_front()
    }

    pub fn is_at_back(&self) -> bool {
        self.index == self.current.len()
    }

    pub fn position(&self) -> usize {
        self.cursor.position() + self.index
    }

    pub fn current_chunk(&self) -> &'a [T] {
        self.current
    }

    pub fn move_next_chunk(&mut self) {
        if self.cursor.is_at_back() {
            self.index = self.current.len();
            return;
        }
        self.move_next();
        self.index = 0;
    }

    pub fn move_prev_chunk(&mut self) {
        if self.index == self.current.len() {
            self.index = 0;
            return;
        }
        self.move_prev();
        self.index = 0;
    }

    fn move_next(&mut self) {
        self.cursor.move_next();
        let (current, range) = self.cursor.current();
        self.current = &current[range];
    }

    fn move_prev(&mut self) {
        self.cursor.move_prev();
        let (current, range) = self.cursor.current();
        self.current = &current[range];
    }
}

impl<T: Clone> From<Vec<T>> for BTreeVec<T> {
    fn from(chunk: Vec<T>) -> Self {
        Self::from(chunk.as_slice())
    }
}

impl<T: Clone> From<&Vec<T>> for BTreeVec<T> {
    fn from(chunk: &Vec<T>) -> Self {
        Self::from(chunk.as_slice())
    }
}

impl<T: Clone> From<&[T]> for BTreeVec<T> {
    fn from(chunk: &[T]) -> Self {
        let mut builder = Builder::new();
        builder.push_chunk(chunk);
        builder.build()
    }
}

pub struct Builder<T> {
    builder: btree::Builder<Vec<T>, Info>,
    chunk: Vec<T>,
}

impl<T: Clone> Builder<T> {
    pub fn new() -> Self {
        Self {
            builder: btree::Builder::new(),
            chunk: Vec::new(),
        }
    }

    pub fn push_chunk(&mut self, mut chunk: &[T]) {
        while !chunk.is_empty() {
            if chunk.len() <= <String as btree::Chunk>::MAX_LEN - self.chunk.len() {
                self.chunk.extend_from_slice(chunk);
                break;
            }
            let index = <String as btree::Chunk>::MAX_LEN - self.chunk.len();
            let (left_chunk, right_chunk) = chunk.split_at(index);
            self.chunk.extend_from_slice(left_chunk);
            chunk = right_chunk;
            self.builder.push_chunk(self.chunk.split_off(0));
        }
    }

    pub fn build(mut self) -> BTreeVec<T> {
        self.builder.push_chunk(self.chunk);
        BTreeVec {
            btree: self.builder.build(),
        }
    }
}

pub struct Chunks<'a, T> {
    cursor: Cursor<'a, T>
}

impl<'a, T: Clone> Iterator for Chunks<'a, T> {
    type Item = &'a [T];

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_at_back() {
            return None;
        }
        let chunk = self.cursor.current_chunk();
        self.cursor.move_next_chunk();
        Some(chunk)
    }
}

pub struct ChunksRev<'a, T> {
    cursor: Cursor<'a, T>
}

impl<'a, T: Clone> Iterator for ChunksRev<'a, T> {
    type Item = &'a [T];

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        if self.cursor.is_at_front() {
            return None;
        }
        self.cursor.move_prev_chunk();
        Some(self.cursor.current_chunk())
    }
}

impl<T: Clone> btree::Chunk for Vec<T> {
    #[cfg(not(test))]
    const MAX_LEN: usize = 1024;
    #[cfg(test)]
    const MAX_LEN: usize = 8;

    fn len(&self) -> usize {
        self.len()
    }

    fn is_boundary(&self, _index: usize) -> bool {
        true
    }

    fn shift_left(&mut self, other: &mut Self, end: usize) {
        self.extend(other.drain(..end));
    }

    fn shift_right(&mut self, other: &mut Self, start: usize) {
        other.splice(..0, self.drain(start..));
    }

    fn truncate_front(&mut self, start: usize) {
        self.drain(..start);
    }

    fn truncate_back(&mut self, end: usize) {
        self.truncate(end);
    }
}

#[derive(Clone, Copy)]
pub struct Info;

impl<T> btree::Info<Vec<T>> for Info {
    fn from_chunk_and_range(_chunk: &Vec<T>, _range: Range<usize>) -> Self {
        Self
    }
}

impl Add for Info {
    type Output = Self;

    fn add(self, _other: Self) -> Self::Output {
        Self
    }
}

impl AddAssign for Info {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Default for Info {
    fn default() -> Self {
        Self
    }
}

impl Sub for Info {
    type Output = Self;

    fn sub(self, _other: Self) -> Self::Output {
        Self
    }
}

impl SubAssign for Info {
    fn sub_assign(&mut self, other: Self) {
        *self = *self - other;
    }
}

#[cfg(test)]
mod tests {
    use {std::fmt, super::*, proptest::prelude::*};

    fn vec<T: Arbitrary + Clone + fmt::Debug>() -> impl Strategy<Value = Vec<T>> {
        prop::collection::vec(any::<T>(), 1..100)
    }

    fn vec_and_index<T: Arbitrary + Clone + fmt::Debug>() -> impl Strategy<Value = (Vec<T>, usize)> {
        vec().prop_flat_map(|vec| {
            let vec_len = vec.len();
            (Just(vec), 0..=vec_len)
        })
    }

    proptest! {
        #[test]
        fn is_empty(vec in vec::<u8>()) {
            let btree_vec = BTreeVec::from(&vec);
            assert_eq!(btree_vec.is_empty(), vec.is_empty());
        }

        #[test]
        fn len(vec in vec::<u8>()) {
            let btree_vec = BTreeVec::from(&vec);
            assert_eq!(btree_vec.len(), vec.len());
        }

        #[test]
        fn chunks(vec in vec::<u8>()) {
            let btree_vec = BTreeVec::from(&vec);
            assert_eq!(
                btree_vec.chunks().flat_map(|chunk| chunk.iter()).cloned().collect::<Vec<_>>(),
                vec,
            );
        }

        #[test]
        fn chunks_rev(vec in vec::<u8>()) {
            let btree_vec = BTreeVec::from(&vec);
            assert_eq!(
                btree_vec
                    .chunks_rev()
                    .flat_map(|chunk| chunk.iter().rev())
                    .cloned()
                    .collect::<Vec<u8>>(),
                vec.iter().rev().cloned().collect::<Vec<_>>(),
            );
        }

        #[test]
        fn append(mut vec_0 in vec::<u8>(), vec_1 in vec::<u8>()) {
            let mut btree_vec_0 = BTreeVec::from(&vec_0);
            let btree_vec_1 = BTreeVec::from(&vec_1);
            btree_vec_0.append(btree_vec_1);
            vec_0.extend_from_slice(&vec_1);
            assert_eq!(
                btree_vec_0.chunks().flat_map(|chunk| chunk.iter()).cloned().collect::<Vec<_>>(),
                vec_0,
            );
        }

        #[test]
        fn split_off((mut vec, at) in vec_and_index::<u8>()) {
            let mut btree_vec = BTreeVec::from(&vec);
            let other_btree_vec = btree_vec.split_off(at);
            let other_vec = vec.split_off(at);
            assert_eq!(
                btree_vec.chunks().flat_map(|chunk| chunk.iter()).cloned().collect::<Vec<_>>(),
                vec,
            );
            assert_eq!(
                other_btree_vec
                    .chunks()
                    .flat_map(|chunk| chunk.iter())
                    .cloned()
                    .collect::<Vec<_>>(),
                other_vec,
            );
        }
    }
}