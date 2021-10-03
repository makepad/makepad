use {
    crate::id::Id,
    std::{
        marker::PhantomData,
        mem,
        ops::{Index, IndexMut},
        slice, vec,
    },
};

#[derive(Debug)]
pub struct IdMap<K: AsRef<Id>, V> {
    entries: Vec<Option<Entry<V>>>,
    phantom: PhantomData<K>,
}

impl<K: AsRef<Id>, V> IdMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, id: K) -> bool {
        self.get(id).is_some()
    }

    pub fn get(&self, id: K) -> Option<&V> {
        let id = id.as_ref();
        match self.entries.get(id.index) {
            Some(Some(entry)) if entry.generation == id.generation => Some(&entry.value),
            _ => None,
        }
    }

    pub fn iter(&self) -> Iter<'_, V> {
        Iter {
            iter: self.entries.iter(),
        }
    }

    pub fn get_mut(&mut self, id: K) -> Option<&mut V> {
        let id = id.as_ref();
        match self.entries.get_mut(id.index) {
            Some(Some(entry)) if entry.generation == id.generation => Some(&mut entry.value),
            _ => None,
        }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, V> {
        IterMut {
            iter: self.entries.iter_mut(),
        }
    }

    pub fn insert(&mut self, id: K, value: V) -> Option<V> {
        let id = id.as_ref();
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

    pub fn remove(&mut self, id: K) -> Option<V> {
        let id = id.as_ref();
        match self.entries.get(id.index) {
            Some(Some(entry)) if entry.generation == id.generation => {
                let entry = mem::replace(&mut self.entries[id.index], None).unwrap();
                Some(entry.value)
            }
            _ => None,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear()
    }
}

impl<K: AsRef<Id>, V> Default for IdMap<K, V> {
    fn default() -> Self {
        Self {
            entries: Vec::default(),
            phantom: PhantomData,
        }
    }
}

impl<K: AsRef<Id>, V> Index<K> for IdMap<K, V> {
    type Output = V;

    fn index(&self, index: K) -> &Self::Output {
        self.get(index).unwrap()
    }
}

impl<K: AsRef<Id>, V> IndexMut<K> for IdMap<K, V> {
    fn index_mut(&mut self, index: K) -> &mut Self::Output {
        self.get_mut(index).unwrap()
    }
}

impl<'a, K: AsRef<Id>, V> IntoIterator for &'a IdMap<K, V> {
    type Item = &'a V;
    type IntoIter = Iter<'a, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, K: AsRef<Id>, V> IntoIterator for &'a mut IdMap<K, V> {
    type Item = &'a mut V;
    type IntoIter = IterMut<'a, V>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<K: AsRef<Id>, V> IntoIterator for IdMap<K, V> {
    type Item = V;
    type IntoIter = IntoIter<V>;

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
