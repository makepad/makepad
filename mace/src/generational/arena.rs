use {
    super::Id,
    std::{
        iter::Enumerate,
        mem,
        ops::{Index, IndexMut},
        slice, vec,
    },
};

#[derive(Clone, Debug)]
pub struct Arena<T> {
    entries: Vec<Option<Entry<T>>>,
}

impl<T> Arena<T> {
    pub fn new() -> Arena<T> {
        Arena::default()
    }

    pub fn get(&self, index: Id) -> Option<&T> {
        match self.entries.get(index.index) {
            Some(Some(entry)) if entry.generation == index.generation => Some(&entry.value),
            _ => None,
        }
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            iter: self.entries.iter().enumerate(),
        }
    }

    pub fn get_mut(&mut self, index: Id) -> Option<&mut T> {
        match self.entries.get_mut(index.index) {
            Some(Some(entry)) if entry.generation == index.generation => Some(&mut entry.value),
            _ => None,
        }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut {
            iter: self.entries.iter_mut().enumerate(),
        }
    }

    pub fn insert(&mut self, index: Id, value: T) -> Option<T> {
        if self.entries.len() < index.index + 1 {
            self.entries.resize_with(index.index + 1, || None);
        }
        match self.entries.get_mut(index.index) {
            Some(Some(entry)) if entry.generation <= index.generation => {
                entry.generation = index.generation;
                Some(mem::replace(&mut entry.value, value))
            }
            Some(entry @ None) => {
                *entry = Some(Entry {
                    value,
                    generation: index.generation,
                });
                None
            }
            _ => panic!(),
        }
    }

    pub fn remove(&mut self, index: Id) -> Option<T> {
        match self.entries.get(index.index) {
            Some(Some(entry)) if entry.generation == index.generation => {
                let entry = mem::replace(&mut self.entries[index.index], None).unwrap();
                Some(entry.value)
            }
            _ => None,
        }
    }
}

impl<T> Default for Arena<T> {
    fn default() -> Arena<T> {
        Arena {
            entries: Vec::default(),
        }
    }
}

impl<T> Index<Id> for Arena<T> {
    type Output = T;

    fn index(&self, index: Id) -> &Self::Output {
        self.get(index).unwrap()
    }
}

impl<T> IndexMut<Id> for Arena<T> {
    fn index_mut(&mut self, index: Id) -> &mut Self::Output {
        self.get_mut(index).unwrap()
    }
}

impl<'a, T> IntoIterator for &'a Arena<T> {
    type Item = (Id, &'a T);
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Arena<T> {
    type Item = (Id, &'a mut T);
    type IntoIter = IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T> IntoIterator for Arena<T> {
    type Item = (Id, T);
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.entries.into_iter().enumerate(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a, T> {
    iter: Enumerate<slice::Iter<'a, Option<Entry<T>>>>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = (Id, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.iter.next() {
                Some((index, Some(entry))) => {
                    break Some((
                        Id {
                            index,
                            generation: entry.generation,
                        },
                        &entry.value,
                    ))
                }
                Some((_, None)) => continue,
                None => break None,
            }
        }
    }
}

#[derive(Debug)]
pub struct IterMut<'a, T> {
    iter: Enumerate<slice::IterMut<'a, Option<Entry<T>>>>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = (Id, &'a mut T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.iter.next() {
                Some((index, Some(entry))) => {
                    break Some((
                        Id {
                            index,
                            generation: entry.generation,
                        },
                        &mut entry.value,
                    ))
                }
                Some((_, None)) => continue,
                None => break None,
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct IntoIter<T> {
    iter: Enumerate<vec::IntoIter<Option<Entry<T>>>>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = (Id, T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.iter.next() {
                Some((index, Some(entry))) => {
                    break Some((
                        Id {
                            index,
                            generation: entry.generation,
                        },
                        entry.value,
                    ))
                }
                Some((_, None)) => continue,
                None => break None,
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Entry<T> {
    value: T,
    generation: usize,
}
