use std::{fmt, iter::Enumerate, hash::{Hash, Hasher}, marker::PhantomData, mem, ops::{Index, IndexMut}, slice, vec};

#[derive(Clone, Debug)]
pub struct Arena<T> {
    len: usize,
    entries: Vec<Entry<T>>,
    generation: usize,
    first_vacant_index: Option<usize>,
}

impl<T> Arena<T> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn contains(&self, id: Id<T>) -> bool {
        self.get(id).is_some()
    }

    pub fn get(&self, id: Id<T>) -> Option<&T> {
        match self.entries.get(id.index) {
            Some(&Entry::Occupied {
                generation,
                ref value,
            }) if generation == id.generation => Some(value),
            _ => None,
        }
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            iter: self.entries.iter().enumerate(),
        }
    }

    pub fn get_mut(&mut self, id: Id<T>) -> Option<&mut T> {
        match self.entries.get_mut(id.index) {
            Some(&mut Entry::Occupied {
                generation,
                ref mut value,
            }) if generation == id.generation => Some(value),
            _ => None,
        }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut {
            iter: self.entries.iter_mut().enumerate()
        }
    }

    pub fn insert(&mut self, value: T) -> Id<T> {
        let entry = Entry::Occupied {
            generation: self.generation,
            value,
        };
        Id::new(match self.first_vacant_index {
            Some(index) => {
                match self.entries[index] {
                    Entry::Vacant { next_vacant_index } => {
                        self.entries[index] = entry;
                        self.first_vacant_index = next_vacant_index;
                        index
                    }
                    _ => unreachable!(),
                }
            }
            None => {
                let index = self.entries.len();
                self.len += 1;
                self.entries.push(entry);
                index
            }
        }, self.generation
        )
    }

    pub fn remove(&mut self, id: Id<T>) -> Option<T> {
        if self.contains(id) {
            self.len -= 1;
            match mem::replace(
                &mut self.entries[id.index],
                Entry::Vacant {
                    next_vacant_index: self.first_vacant_index,
                },
            ) {
                Entry::Occupied { value, .. } => {
                    if id.generation == self.generation {
                        self.generation += 1;
                    }
                    self.first_vacant_index = Some(id.index);
                    Some(value)
                }
                _ => unreachable!(),
            }
        } else {
            None
        }
    }

    pub fn clear(&mut self) {
        self.len = 0;
        self.entries.clear();
        self.generation += 1;
        self.first_vacant_index = None;
    }
}

impl<T> Default for Arena<T> {
    fn default() -> Self {
        Self {
            len: 0,
            entries: Vec::new(),
            generation: 0,
            first_vacant_index: None,
        }
    }
}

impl<T> Index<Id<T>> for Arena<T> {
    type Output = T;

    fn index(&self, id: Id<T>) -> &T {
        self.get(id).unwrap()
    }
}

impl<T> IndexMut<Id<T>> for Arena<T> {
    fn index_mut(&mut self, id: Id<T>) -> &mut T {
        self.get_mut(id).unwrap()
    }
}

impl<'a, T> IntoIterator for &'a Arena<T> {
    type Item = (Id<T>, &'a T);
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Arena<T> {
    type Item = (Id<T>, &'a mut T);
    type IntoIter = IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T> IntoIterator for Arena<T> {
    type Item = (Id<T>, T);
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.entries.into_iter().enumerate()
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a, T> {
    iter: Enumerate<slice::Iter<'a, Entry<T>>>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = (Id<T>, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (index, entry) = self.iter.next()?;
            match *entry {
                Entry::Occupied { generation, ref value } => {
                    break Some((Id::new(index, generation), value));
                }
                Entry::Vacant { .. } => continue,
            }
        }
    }
}

#[derive(Debug)]
pub struct IterMut<'a, T> {
    iter: Enumerate<slice::IterMut<'a, Entry<T>>>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = (Id<T>, &'a mut T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (index, entry) = self.iter.next()?;
            match *entry {
                Entry::Occupied { generation, ref mut value } => {
                    break Some((Id::new(index, generation), value));
                }
                Entry::Vacant { .. } => continue,
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct IntoIter<T> {
    iter: Enumerate<vec::IntoIter<Entry<T>>>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = (Id<T>, T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let (index, entry) = self.iter.next()?;
            match entry {
                Entry::Occupied { generation, value } => {
                    break Some((Id::new(index, generation), value));
                }
                Entry::Vacant { .. } => continue,
            }
        }
    }
}

pub struct Id<T> {
    index: usize,
    generation: usize,
    phantom: PhantomData<T>,
}

impl<T> Id<T> {
    pub fn new(index: usize, generation: usize) -> Self {
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
        H: Hasher
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

#[derive(Clone, Debug)]
enum Entry<T> {
    Occupied { generation: usize, value: T },
    Vacant { next_vacant_index: Option<usize> },
}
