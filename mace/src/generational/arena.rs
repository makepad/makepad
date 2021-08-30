use {
    super::Id,
    std::{
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

    pub fn contains(&self, id: Id) -> bool {
        match self.entries.get(id.index) {
            Some(Some(entry)) if entry.generation == id.generation => true,
            _ => false,
        }
    }

    pub fn get(&self, id: Id) -> Option<&T> {
        match self.entries.get(id.index) {
            Some(Some(entry)) if entry.generation == id.generation => Some(&entry.value),
            _ => None,
        }
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            iter: self.entries.iter(),
        }
    }

    pub fn get_mut(&mut self, id: Id) -> Option<&mut T> {
        match self.entries.get_mut(id.index) {
            Some(Some(entry)) if entry.generation == id.generation => Some(&mut entry.value),
            _ => None,
        }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut {
            iter: self.entries.iter_mut(),
        }
    }

    pub fn insert(&mut self, id: Id, value: T) -> Option<T> {
        if self.entries.len() < id.index + 1 {
            self.entries.resize_with(id.index + 1, || None);
        }
        match self.entries.get_mut(id.index) {
            Some(Some(entry)) if entry.generation <= id.generation => {
                entry.generation = id.generation;
                Some(mem::replace(&mut entry.value, value))
            }
            Some(entry @ None) => {
                *entry = Some(Entry {
                    value,
                    generation: id.generation,
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

    pub fn clear(&mut self) {
        self.entries.clear()
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
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut Arena<T> {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T> IntoIterator for Arena<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            iter: self.entries.into_iter(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Iter<'a, T> {
    iter: slice::Iter<'a, Option<Entry<T>>>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.iter.next()? {
                Some(entry) => break Some(&entry.value),
                _ => continue,
            }
        }
    }
}

#[derive(Debug)]
pub struct IterMut<'a, T> {
    iter: slice::IterMut<'a, Option<Entry<T>>>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.iter.next()? {
                Some(entry) => break Some(&mut entry.value),
                _ => continue,
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct IntoIter<T> {
    iter: vec::IntoIter<Option<Entry<T>>>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;
    
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.iter.next()? {
                Some(entry) => break Some(entry.value),
                _ => continue,
            }
        }
    }
}

#[derive(Clone, Debug)]
struct Entry<T> {
    value: T,
    generation: usize,
}
