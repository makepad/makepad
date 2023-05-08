use std::{
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    ops::{Index, IndexMut},
};

#[derive(Debug)]
pub struct Arena<T> {
    entries: Vec<Entry<T>>,
    generation: usize,
    first_vacant_index: Option<usize>,
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, id: Id<T>) -> Option<&T> {
        match self.entries.get(id.index) {
            Some(Entry::Occupied { generation, value }) if *generation == id.generation => {
                Some(value)
            }
            _ => None,
        }
    }

    pub fn get_mut(&mut self, id: Id<T>) -> Option<&mut T> {
        match self.entries.get_mut(id.index) {
            Some(Entry::Occupied { generation, value }) if *generation == id.generation => {
                Some(value)
            }
            _ => None,
        }
    }

    pub fn insert(&mut self, value: T) -> Id<T> {
        let entry = Entry::Occupied {
            generation: self.generation,
            value,
        };
        let index = match self.first_vacant_index {
            Some(index) => match self.entries[index] {
                Entry::Vacant { next_vacant_index } => {
                    self.entries[index] = entry;
                    self.first_vacant_index = next_vacant_index;
                    index
                }
                _ => panic!(),
            },
            None => {
                let index = self.entries.len();
                self.entries.push(entry);
                index
            }
        };
        Id::new(index, self.generation)
    }

    pub fn remove(&mut self, id: Id<T>) -> Option<T> {
        use std::mem;

        if id.index > self.entries.len() {
            return None;
        }
        match mem::replace(
            &mut self.entries[id.index],
            Entry::Vacant {
                next_vacant_index: self.first_vacant_index,
            },
        ) {
            Entry::Occupied { generation, value } => {
                if generation == id.generation {
                    if generation == self.generation {
                        self.generation += 1;
                    }
                    Some(value)
                } else {
                    self.entries
                        .insert(id.index, Entry::Occupied { generation, value });
                    None
                }
            }
            entry @ Entry::Vacant { .. } => {
                self.entries.insert(id.index, entry);
                None
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.generation += 1;
        self.first_vacant_index = None;
    }
}

impl<T> Index<Id<T>> for Arena<T> {
    type Output = T;

    fn index(&self, id: Id<T>) -> &Self::Output {
        self.get(id).unwrap()
    }
}

impl<T> IndexMut<Id<T>> for Arena<T> {
    fn index_mut(&mut self, id: Id<T>) -> &mut Self::Output {
        self.get_mut(id).unwrap()
    }
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            generation: 0,
            first_vacant_index: None,
        }
    }
}

pub struct Id<T> {
    index: usize,
    generation: usize,
    phantom: PhantomData<T>,
}

impl<T> Id<T> {
    fn new(index: usize, generation: usize) -> Self {
        Self {
            index,
            generation,
            phantom: PhantomData,
        }
    }
}

impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        Self {
            index: self.index,
            generation: self.generation,
            phantom: self.phantom,
        }
    }
}

impl<T> Copy for Id<T> {}

impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Id")
            .field("index", &self.index)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

impl<T> Eq for Id<T> {}

impl<T> Hash for Id<T> {
    fn hash<H>(&self, hasher: &mut H)
    where
        H: Hasher,
    {
        self.index.hash(hasher);
        self.generation.hash(hasher);
    }
}

impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        if self.index != other.index {
            return false;
        }
        if self.generation != other.generation {
            return false;
        }
        true
    }
}

#[derive(Debug)]
enum Entry<T> {
    Occupied { generation: usize, value: T },
    Vacant { next_vacant_index: Option<usize> },
}
