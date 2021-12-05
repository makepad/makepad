use {
    crate::genid::GenId,
    std::{
        marker::PhantomData,
        mem,
        ops::{Index, IndexMut},
        slice::{Iter, IterMut},
        vec::IntoIter,
    },
};

#[derive(Debug)]
pub struct GenIdMap<K: AsRef<GenId>, V> {
    entries: Vec<Option<Entry<V>>>,
    phantom: PhantomData<K>,
}

impl<K: AsRef<GenId>, V> GenIdMap<K, V> {
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

    pub fn values(&self) -> Values<'_, V> {
        Values {
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

    pub fn values_mut(&mut self) -> ValuesMut<'_, V> {
        ValuesMut {
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

    pub fn into_values(self) -> IntoValues<V> {
        IntoValues {
            iter: self.entries.into_iter(),
        }
    }
}

impl<K: AsRef<GenId>, V> Default for GenIdMap<K, V> {
    fn default() -> Self {
        Self {
            entries: Vec::default(),
            phantom: PhantomData,
        }
    }
}

impl<K: AsRef<GenId>, V> Index<K> for GenIdMap<K, V> {
    type Output = V;

    fn index(&self, index: K) -> &Self::Output {
        self.get(index).unwrap()
    }
}

impl<K: AsRef<GenId>, V> IndexMut<K> for GenIdMap<K, V> {
    fn index_mut(&mut self, index: K) -> &mut Self::Output {
        self.get_mut(index).unwrap()
    }
}

#[derive(Clone, Debug)]
pub struct Values<'a, V> {
    iter: Iter<'a, Option<Entry<V>>>,
}

impl<'a, V> Iterator for Values<'a, V> {
    type Item = &'a V;

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
pub struct ValuesMut<'a, V> {
    iter: IterMut<'a, Option<Entry<V>>>,
}

impl<'a, V> Iterator for ValuesMut<'a, V> {
    type Item = &'a mut V;

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
pub struct IntoValues<V> {
    iter: IntoIter<Option<Entry<V>>>,
}

impl<V> Iterator for IntoValues<V> {
    type Item = V;

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
