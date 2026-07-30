use crate::seg::chunk::Chunk;
use crate::seg::entity::Entity;
use crate::seg::exp::{SegExpCollection, SegRange};
use crate::seg::heap::BitIter;
use crate::seg::layout::Layout;
use crate::{Expiration, ExpiredVal, LayoutNumber};
use alloc::vec;
use alloc::vec::Vec;
use core::marker::PhantomData;

pub struct SegExpTree<R, E, V> {
    layout: Layout<R>,
    chunks: Vec<Chunk<E, V>>,
    phantom_data: PhantomData<R>,
}

impl<R, E: Expiration, V: ExpiredVal<E>> SegExpTree<R, E, V>
where
    R: LayoutNumber,
{
    #[inline]
    pub fn new(range: SegRange<R>) -> Option<Self> {
        let layout = Layout::new(range.min, range.max)?;
        let count = layout.count();

        Some(Self {
            layout,
            chunks: vec![Chunk::new(); count],
            phantom_data: Default::default(),
        })
    }

    #[inline]
    fn chunk(&self, index: usize) -> &Chunk<E, V> {
        unsafe { self.chunks.get_unchecked(index) }
    }

    #[inline]
    fn chunk_mut(&mut self, index: usize) -> &mut Chunk<E, V> {
        unsafe { self.chunks.get_unchecked_mut(index) }
    }
}

impl<R, E: Expiration, V: ExpiredVal<E>> SegExpCollection<R, E, V> for SegExpTree<R, E, V>
where
    R: LayoutNumber,
{
    #[inline]
    fn insert_by_range(&mut self, range: SegRange<R>, val: V) {
        let mask = self.layout.insert_mask(range.min, range.max);
        let entity = Entity::new(val, mask);
        for index in BitIter::new(mask) {
            self.chunk_mut(index).insert(entity);
        }
    }

    type Iter<'a>
        = SegExpTreeIterator<'a, R, E, V>
    where
        R: 'a,
        E: 'a,
        V: 'a;

    #[inline]
    fn iter_by_range(&mut self, range: SegRange<R>, time: E) -> SegExpTreeIterator<'_, R, E, V> {
        let mask = self.layout.intersect_mask(range.min, range.max);
        SegExpTreeIterator::new(mask, time, self)
    }

    #[inline]
    fn clear(&mut self) {
        for chunk in self.chunks.iter_mut() {
            chunk.clear();
        }
    }
}

pub struct SegExpTreeIterator<'a, R, E, V> {
    tree: &'a mut SegExpTree<R, E, V>,
    time: E,
    i0: usize,
    i1: usize,
    mask: u64,
    bit_iter: BitIter,
}

impl<'a, R, E: Expiration, V: ExpiredVal<E>> SegExpTreeIterator<'a, R, E, V>
where
    R: LayoutNumber,
{
    #[inline]
    fn new(mask: u64, time: E, tree: &'a mut SegExpTree<R, E, V>) -> Self {
        let mut iter = SegExpTreeIterator {
            tree,
            time,
            i0: 0,
            i1: 0,
            mask,
            bit_iter: BitIter::new(mask),
        };

        // Find the first valid chunk
        iter.i0 = iter.find_next_not_empty_chunk();

        iter
    }

    #[inline]
    fn find_next_not_empty_chunk(&mut self) -> usize {
        for next in &mut self.bit_iter {
            if !self.tree.chunk(next).is_empty() {
                return next;
            }
        }
        usize::MAX
    }
}

impl<R, E: Expiration, V: ExpiredVal<E>> Iterator for SegExpTreeIterator<'_, R, E, V>
where
    R: LayoutNumber,
{
    type Item = V;

    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        while self.i0 < self.tree.chunks.len() {
            let chunk = self.tree.chunk_mut(self.i0);
            let mut i = self.i1;
            while i < chunk.buffer.len() {
                let item = chunk.entity(i);

                if item.val.expiration() < self.time {
                    chunk.buffer.swap_remove(i);
                    continue;
                }
                i += 1;

                // we must return same pair only once,
                let mask_int = item.mask & self.mask;
                let first_index = mask_int.trailing_zeros() as usize;

                // we will return only for first index
                if first_index == self.i0 {
                    self.i1 = i;
                    return Some(item.val);
                }
            }

            self.i0 = self.find_next_not_empty_chunk();
            self.i1 = 0;
        }

        None
    }
}

#[cfg(test)]
mod tests {
    use crate::ExpiredVal;
    use crate::seg::exp::{SegExpCollection, SegRange};
    use crate::seg::tree::SegExpTree;
    use alloc::vec::Vec;

    #[derive(Clone, Copy)]
    struct Point {
        x: i32,
        y: i32,
    }

    #[derive(Clone, Copy)]
    struct Segment {
        a: Point,
        b: Point,
    }

    impl Segment {
        fn new(ax: i32, ay: i32, bx: i32, by: i32) -> Self {
            Self {
                a: Point { x: ax, y: ay },
                b: Point { x: bx, y: by },
            }
        }

        fn y_range(&self) -> SegRange<i32> {
            if self.a.y < self.b.y {
                SegRange {
                    min: self.a.y,
                    max: self.b.y,
                }
            } else {
                SegRange {
                    min: self.b.y,
                    max: self.a.y,
                }
            }
        }
    }

    impl ExpiredVal<i32> for Segment {
        fn expiration(&self) -> i32 {
            self.a.x.max(self.b.x)
        }
    }

    #[test]
    fn test_00() {
        let mut tree = SegExpTree::new(SegRange { min: 0, max: 128 }).unwrap();
        let s = Segment::new(0, 2, 2, 100);
        tree.insert_by_range(s.y_range(), s);
        tree.clear();
        for chunk in tree.chunks {
            assert!(chunk.is_empty());
        }
    }

    #[test]
    fn test_01() {
        let mut tree = SegExpTree::new(SegRange { min: 0, max: 128 }).unwrap();
        let s = Segment::new(0, 2, 2, 100);
        tree.insert_by_range(s.y_range(), s);
        let mut result = Vec::new();
        for val in tree.iter_by_range(SegRange { min: 0, max: 100 }, 0) {
            result.push(val);
        }
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_02() {
        let mut tree = SegExpTree::new(SegRange { min: 0, max: 128 }).unwrap();
        let s0 = Segment::new(0, 10, 2, 100);
        let s1 = Segment::new(0, 20, 2, 80);

        tree.insert_by_range(s0.y_range(), s0);
        tree.insert_by_range(s1.y_range(), s1);

        let mut result = Vec::new();
        for val in tree.iter_by_range(SegRange { min: 15, max: 90 }, 0) {
            result.push(val);
        }
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_03() {
        let mut tree = SegExpTree::new(SegRange { min: 0, max: 128 }).unwrap();
        let s0 = Segment::new(0, 10, 2, 20);
        let s1 = Segment::new(0, 80, 2, 100);

        tree.insert_by_range(s0.y_range(), s0);
        tree.insert_by_range(s1.y_range(), s1);

        let mut result = Vec::new();
        for val in tree.iter_by_range(SegRange { min: 40, max: 60 }, 0) {
            result.push(val);
        }
        assert_eq!(result.len(), 0);
    }

    #[test]
    fn test_04() {
        let mut tree = SegExpTree::new(SegRange {
            min: -10240,
            max: 15360,
        })
        .unwrap();
        let s0 = Segment::new(0, -10240, 10, 10240);
        let s1 = Segment::new(0, -10240, 10240, -10240);

        let m0 = tree.layout.intersect_mask(s0.y_range().min, s0.y_range().max);
        let m1 = tree.layout.intersect_mask(s1.y_range().min, s1.y_range().max);

        assert_ne!(m0 & m1, 0);

        tree.insert_by_range(s0.y_range(), s0);

        let mut result = Vec::new();
        for val in tree.iter_by_range(s1.y_range(), 0) {
            result.push(val);
        }
        assert_eq!(result.len(), 1);
    }
}
