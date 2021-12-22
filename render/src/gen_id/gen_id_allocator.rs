use {
    super::GenId,
    std::{fmt, marker::PhantomData},
};

pub struct GenIdAllocator<Tag> {
    entries: Vec<Entry>,
    free_list: Vec<usize>,
    tag: PhantomData<Tag>,
}

impl<Tag> GenIdAllocator<Tag> {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            free_list: Vec::new(),
            tag: PhantomData,
        }
    }

    pub fn allocate(&mut self) -> GenId<Tag> {
        match self.free_list.pop() {
            Some(index) => {
                let entry = &mut self.entries[index];
                debug_assert!(!entry.is_used);
                entry.is_used = true;
                entry.generation += 1;
                GenId {
                    index,
                    generation: entry.generation,
                    tag: PhantomData,
                }
            }
            None => {
                let index = self.entries.len();
                self.entries.push(Entry {
                    is_used: true,
                    generation: 0,
                });
                GenId {
                    index,
                    generation: 0,
                    tag: PhantomData,
                }
            }
        }
    }

    pub fn deallocate(&mut self, id: GenId<Tag>) {
        let entry = &mut self.entries[id.index];
        assert!(entry.is_used && entry.generation == id.generation);
        entry.is_used = false;
        self.free_list.push(id.index);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.free_list.clear();
    }
}

impl<Tag> Default for GenIdAllocator<Tag> {
    fn default() -> Self {
        GenIdAllocator::new()
    }
}

impl<Tag> fmt::Debug for GenIdAllocator<Tag> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Map")
            .field("entries", &self.entries)
            .field("free_list", &self.free_list)
            .finish()
    }
}

#[derive(Debug)]
struct Entry {
    is_used: bool,
    generation: usize,
}
