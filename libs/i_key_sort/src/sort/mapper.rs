use crate::sort::bin_layout::MAX_BINS_COUNT;
use core::ops::Range;
use core::slice::Iter;

#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Chunk {
    index: usize,
    count: usize,
}

pub struct Mapper {
    pub(crate) count: usize,
    pub(crate) chunks: [Chunk; MAX_BINS_COUNT],
}

impl Mapper {
    #[inline(always)]
    pub(crate) fn new(count: usize) -> Self {
        debug_assert!(count <= MAX_BINS_COUNT);
        Self {
            count,
            chunks: [Chunk::default(); MAX_BINS_COUNT],
        }
    }

    #[inline(always)]
    pub(super) fn inc_bin_count(&mut self, chunk_index: usize) {
        unsafe { self.chunks.get_unchecked_mut(chunk_index).count += 1 };
    }

    #[inline(always)]
    pub(super) fn next_index(&mut self, chunk_index: usize) -> usize {
        let chunk = unsafe { self.chunks.get_unchecked_mut(chunk_index) };
        let index = chunk.index;
        chunk.index += 1;
        index
    }

    #[inline(always)]
    pub(super) fn init_indices(&mut self) {
        let mut offset = 0;
        for chunk in &mut self.chunks[..self.count] {
            chunk.index = offset;
            offset += chunk.count;
        }
    }

    #[inline(always)]
    pub(crate) fn iter(&self) -> Iter<'_, Chunk> {
        unsafe { self.chunks.get_unchecked(..self.count) }.iter()
    }
}

impl Chunk {
    #[inline(always)]
    pub(crate) fn as_range(&self) -> Range<usize> {
        let end = self.index;
        let start = self.index - self.count;
        start..end
    }
}
