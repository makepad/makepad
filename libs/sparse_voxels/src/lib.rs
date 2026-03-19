#![no_std]

extern crate alloc;

use alloc::vec::Vec;
use core::hash::Hash;

#[cfg(feature = "enhanced-determinism")]
type SparseChunkMap<K, V> = indexmap::IndexMap<K, V>;
#[cfg(not(feature = "enhanced-determinism"))]
type SparseChunkMap<K, V> = hashbrown::hash_map::HashMap<K, V, foldhash::fast::FixedState>;

#[cfg(feature = "enhanced-determinism")]
fn sparse_chunk_map_with_capacity<K, V>(chunk_capacity: usize) -> SparseChunkMap<K, V> {
    SparseChunkMap::with_capacity(chunk_capacity)
}

#[cfg(not(feature = "enhanced-determinism"))]
fn sparse_chunk_map_with_capacity<K, V>(chunk_capacity: usize) -> SparseChunkMap<K, V> {
    SparseChunkMap::with_capacity_and_hasher(chunk_capacity, foldhash::fast::FixedState::default())
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SparseChunkHeader {
    pub id: usize,
    pub len: usize,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SparseChunkIndex<const CHUNK_LEN: usize> {
    pub chunk_id: usize,
    pub id_in_chunk: usize,
}

impl<const CHUNK_LEN: usize> SparseChunkIndex<CHUNK_LEN> {
    pub fn flat_id(&self) -> usize {
        self.chunk_id * CHUNK_LEN + self.id_in_chunk
    }

    pub fn from_flat_id(id: usize) -> Self {
        Self {
            chunk_id: id / CHUNK_LEN,
            id_in_chunk: id % CHUNK_LEN,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SparseChunkStorage<K, C> {
    pub chunk_headers: SparseChunkMap<K, SparseChunkHeader>,
    pub chunk_keys: Vec<K>,
    pub chunks: Vec<C>,
    pub free_chunks: Vec<usize>,
    pub invalid_chunk_key: K,
}

impl<K, C> SparseChunkStorage<K, C>
where
    K: Copy + Eq + Hash,
{
    pub fn new(invalid_chunk_key: K) -> Self {
        Self {
            chunk_headers: Default::default(),
            chunk_keys: Vec::new(),
            chunks: Vec::new(),
            free_chunks: Vec::new(),
            invalid_chunk_key,
        }
    }

    pub fn with_capacity(invalid_chunk_key: K, chunk_capacity: usize) -> Self {
        Self {
            chunk_headers: sparse_chunk_map_with_capacity(chunk_capacity),
            chunk_keys: Vec::with_capacity(chunk_capacity),
            chunks: Vec::with_capacity(chunk_capacity),
            free_chunks: Vec::new(),
            invalid_chunk_key,
        }
    }

    pub fn chunk_key(&self, chunk_id: usize) -> Option<&K> {
        let key = self.chunk_keys.get(chunk_id)?;
        if *key == self.invalid_chunk_key {
            None
        } else {
            Some(key)
        }
    }

    pub fn is_chunk_live(&self, chunk_id: usize) -> bool {
        self.chunk_key(chunk_id).is_some()
    }

    pub fn header_or_insert_with(
        &mut self,
        key: K,
        make_chunk: impl FnOnce() -> C,
    ) -> (&mut SparseChunkHeader, bool) {
        if self.chunk_headers.contains_key(&key) {
            return (self.chunk_headers.get_mut(&key).unwrap(), false);
        }

        let id = if let Some(id) = self.free_chunks.pop() {
            self.chunks[id] = make_chunk();
            self.chunk_keys[id] = key;
            id
        } else {
            self.chunks.push(make_chunk());
            self.chunk_keys.push(key);
            self.chunks.len() - 1
        };

        self.chunk_headers
            .insert(key, SparseChunkHeader { id, len: 0 });
        (self.chunk_headers.get_mut(&key).unwrap(), true)
    }

    pub fn remove_chunk(&mut self, key: &K) -> Option<SparseChunkHeader> {
        #[cfg(feature = "enhanced-determinism")]
        let chunk_header = self.chunk_headers.swap_remove(key)?;
        #[cfg(not(feature = "enhanced-determinism"))]
        let chunk_header = self.chunk_headers.remove(key)?;

        self.free_chunks.push(chunk_header.id);
        self.chunk_keys[chunk_header.id] = self.invalid_chunk_key;
        Some(chunk_header)
    }
}
