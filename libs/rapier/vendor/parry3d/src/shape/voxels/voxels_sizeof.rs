use super::{Voxels, VoxelsChunk, VoxelsChunkHeader};
use crate::math::IVector;

impl Voxels {
    // TODO: support a crate like get_size2 (will require support on nalgebra too)?
    /// An approximation of the memory usage (in bytes) for this struct plus
    /// the memory it allocates dynamically.
    pub fn total_memory_size(&self) -> usize {
        size_of::<Self>() + self.heap_memory_size()
    }

    /// An approximation of the memory dynamically-allocated by this struct.
    pub fn heap_memory_size(&self) -> usize {
        // NOTE: if a new field is added to `Self`, adjust this function result.
        let Self {
            chunk_bvh,
            chunk_headers,
            chunks,
            free_chunks,
            chunk_keys,
            voxel_size: _,
        } = self;
        chunks.capacity() * size_of::<VoxelsChunk>()
            + free_chunks.capacity() * size_of::<usize>()
            + chunk_keys.capacity() * size_of::<IVector>()
            + chunk_headers.capacity() * (size_of::<VoxelsChunkHeader>() + size_of::<IVector>())
            + chunk_bvh.heap_memory_size()
    }
}
