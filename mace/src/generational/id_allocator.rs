use {
    super::Id,
    std::{iter::Enumerate, slice},
};

#[derive(Clone, Debug, Default)]
pub struct IdAllocator {
    entries: Vec<Entry>,
    free_entry_indices: Vec<usize>,
}

impl IdAllocator {
    pub fn new() -> IdAllocator {
        IdAllocator::default()
    }

    pub fn allocate(&mut self) -> Id {
        match self.free_entry_indices.pop() {
            Some(index) => {
                let entry = &mut self.entries[index];
                debug_assert!(!entry.is_used);
                entry.is_used = true;
                entry.generation += 1;
                Id {
                    index,
                    generation: entry.generation,
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
                }
            }
        }
    }

    pub fn deallocate(&mut self, index: Id) {
        let entry = &mut self.entries[index.index];
        assert!(entry.is_used && entry.generation == index.generation);
        entry.is_used = false;
        self.free_entry_indices.push(index.index);
    }

    pub fn iter(&self) -> Iter<'_> {
        Iter {
            iter: self.entries.iter().enumerate(),
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear()
    }
}

impl<'a> IntoIterator for &'a IdAllocator {
    type Item = Id;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct Iter<'a> {
    iter: Enumerate<slice::Iter<'a, Entry>>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = Id;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.iter.next()? {
                (index, entry) if entry.is_used => {
                    break Some(Id {
                        index,
                        generation: entry.generation,
                    });
                }
                _ => continue,
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Entry {
    is_used: bool,
    generation: usize,
}
