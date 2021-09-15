use {
    super::Id,
    std::{fmt, marker::PhantomData},
};

pub struct IdAllocator<T> {
    entries: Vec<Entry>,
    free_entry_indices: Vec<usize>,
    phantom: PhantomData<T>,
}

impl<T> IdAllocator<T> {
    pub fn new() -> Self {
        IdAllocator::default()
    }

    pub fn allocate(&mut self) -> Id<T> {
        match self.free_entry_indices.pop() {
            Some(index) => {
                let entry = &mut self.entries[index];
                debug_assert!(!entry.is_used);
                entry.is_used = true;
                entry.generation += 1;
                Id {
                    index,
                    generation: entry.generation,
                    phantom: PhantomData,
                }
            }
            None => {
                self.entries.push(Entry {
                    is_used: true,
                    generation: 0,
                });
                Id {
                    index: self.entries.len() - 1,
                    generation: 0,
                    phantom: PhantomData,
                }
            }
        }
    }

    pub fn deallocate(&mut self, index: Id<T>) {
        let entry = &mut self.entries[index.index];
        assert!(entry.is_used && entry.generation == index.generation);
        entry.is_used = false;
        self.free_entry_indices.push(index.index);
    }

    pub fn clear(&mut self) {
        self.entries.clear()
    }
}

impl<T> fmt::Debug for IdAllocator<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdAllocator")
            .field("entries", &self.entries)
            .field("free_entry_indices", &self.free_entry_indices)
            .finish()
    }
}

impl<T> Default for IdAllocator<T> {
    fn default() -> Self {
        Self {
            entries: Vec::default(),
            free_entry_indices: Vec::default(),
            phantom: PhantomData,
        }
    }
}

#[derive(Clone, Debug)]
struct Entry {
    is_used: bool,
    generation: usize,
}
