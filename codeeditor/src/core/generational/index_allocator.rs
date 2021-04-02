use super::Index;

#[derive(Clone, Debug, Default)]
pub struct IndexAllocator {
    entries: Vec<Entry>,
    free_entry_indices: Vec<usize>,
}

impl IndexAllocator {
    pub fn new() -> IndexAllocator {
        IndexAllocator::default()
    }

    pub fn allocate(&mut self) -> Index {
        match self.free_entry_indices.pop() {
            Some(index) => {
                let entry = &mut self.entries[index];
                debug_assert!(!entry.is_used);
                entry.is_used = true;
                entry.generation += 1;
                Index {
                    index,
                    generation: entry.generation,
                }
            }
            None => {
                self.entries.push(Entry {
                    is_used: true,
                    generation: 0,
                });
                Index {
                    index: self.entries.len() - 1,
                    generation: 0,
                }
            }
        }
    }

    pub fn deallocate(&mut self, index: Index) {
        let entry = &mut self.entries[index.index];
        assert!(entry.is_used && entry.generation == index.generation);
        entry.is_used = false;
        self.free_entry_indices.push(index.index);
    }
}

#[derive(Clone, Debug)]
struct Entry {
    is_used: bool,
    generation: usize,
}
