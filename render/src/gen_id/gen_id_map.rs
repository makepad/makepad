use {
    super::GenId,
    std::{
        fmt,
        iter::Enumerate,
        marker::PhantomData,
        mem,
        ops::{Index, IndexMut},
        slice,
    },
};

pub struct GenIdMap<Tag, T> {
    len: usize,
    entries: Vec<Option<Entry<T>>>,
    tag: PhantomData<Tag>,
}

impl<Tag, T> GenIdMap<Tag, T> {
    pub fn new() -> Self {
        Self {
            len: 0,
            entries: Vec::new(),
            tag: PhantomData,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn contains_id(&self, id: GenId<Tag>) -> bool {
        self.get(id).is_some()
    }

    pub fn get(&self, id: GenId<Tag>) -> Option<&T> {
        match self.entries.get(id.index) {
            Some(Some(entry)) if entry.generation == id.generation => Some(&entry.value),
            _ => None,
        }
    }

    pub fn get_mut(&mut self, id: GenId<Tag>) -> Option<&mut T> {
        match self.entries.get_mut(id.index) {
            Some(Some(entry)) if entry.generation == id.generation => Some(&mut entry.value),
            _ => None,
        }
    }

    pub fn iter(&self) -> Iter<'_, Tag, T> {
        Iter {
            iter: self.entries.iter().enumerate(),
            tag: PhantomData,
        }
    }

    pub fn ids(&self) -> Ids<'_, Tag, T> {
        Ids { iter: self.iter() }
    }

    pub fn values(&self) -> Values<'_, Tag, T> {
        Values { iter: self.iter() }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, Tag, T> {
        IterMut {
            iter: self.entries.iter_mut().enumerate(),
            tag: PhantomData,
        }
    }

    pub fn values_mut(&mut self) -> ValuesMut<'_, Tag, T> {
        ValuesMut {
            iter: self.iter_mut(),
        }
    }

    pub fn insert(&mut self, id: GenId<Tag>, value: T) -> Option<T> {
        if self.entries.len() < id.index + 1 {
            self.entries.resize_with(id.index + 1, || None);
        }
        match self.entries.get_mut(id.index) {
            Some(Some(entry)) if entry.generation <= id.generation => {
                entry.generation = id.generation;
                Some(mem::replace(&mut entry.value, value))
            }
            Some(entry @ None) => {
                self.len += 1;
                *entry = Some(Entry {
                    generation: id.generation,
                    value,
                });
                None
            }
            _ => panic!(),
        }
    }

    pub fn remove(&mut self, id: GenId<Tag>) -> Option<T> {
        match self.entries.get(id.index) {
            Some(Some(entry)) if entry.generation == id.generation => {
                self.len -= 1;
                let entry = mem::replace(&mut self.entries[id.index], None).unwrap();
                Some(entry.value)
            }
            _ => None,
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl<Tag, T: Clone> Clone for GenIdMap<Tag, T> {
    fn clone(&self) -> Self {
        Self {
            len: self.len,
            entries: self.entries.clone(),
            tag: PhantomData,
        }
    }
}

impl<Tag, T: fmt::Debug> fmt::Debug for GenIdMap<Tag, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Map")
            .field("len", &self.len)
            .field("entries", &self.entries)
            .finish()
    }
}

impl<Tag, T> Default for GenIdMap<Tag, T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<Tag, T: Eq> Eq for GenIdMap<Tag, T> {}

impl<Tag, T> Index<GenId<Tag>> for GenIdMap<Tag, T> {
    type Output = T;

    fn index(&self, id: GenId<Tag>) -> &Self::Output {
        self.get(id).unwrap()
    }
}

impl<Tag, T> IndexMut<GenId<Tag>> for GenIdMap<Tag, T> {
    fn index_mut(&mut self, id: GenId<Tag>) -> &mut Self::Output {
        self.get_mut(id).unwrap()
    }
}

impl<Tag, T: PartialEq> PartialEq for GenIdMap<Tag, T> {
    fn eq(&self, other: &Self) -> bool {
        self.entries == other.entries
    }
}

pub struct Iter<'a, Tag, T> {
    iter: Enumerate<slice::Iter<'a, Option<Entry<T>>>>,
    tag: PhantomData<Tag>,
}

impl<'a, Tag, T> Iterator for Iter<'a, Tag, T> {
    type Item = (GenId<Tag>, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let (index, Some(entry)) = self.iter.next()? {
                break Some((
                    GenId {
                        index,
                        generation: entry.generation,
                        tag: PhantomData,
                    },
                    &entry.value,
                ));
            }
        }
    }
}

impl<'a, Tag, T: Clone> Clone for Iter<'a, Tag, T> {
    fn clone(&self) -> Self {
        Self {
            iter: self.iter.clone(),
            tag: PhantomData,
        }
    }
}

impl<'a, Tag, T: fmt::Debug> fmt::Debug for Iter<'a, Tag, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Iter").field("iter", &self.iter).finish()
    }
}

pub struct Ids<'a, Tag, T> {
    iter: Iter<'a, Tag, T>,
}

impl<'a, Tag, T: Clone> Clone for Ids<'a, Tag, T> {
    fn clone(&self) -> Self {
        Self {
            iter: self.iter.clone(),
        }
    }
}

impl<'a, Tag, T: fmt::Debug> fmt::Debug for Ids<'a, Tag, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ids").field("iter", &self.iter).finish()
    }
}

impl<'a, Tag, T> Iterator for Ids<'a, Tag, T> {
    type Item = GenId<Tag>;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.iter.next()?.0)
    }
}

pub struct Values<'a, Tag, T> {
    iter: Iter<'a, Tag, T>,
}

impl<'a, Tag, T: Clone> Clone for Values<'a, Tag, T> {
    fn clone(&self) -> Self {
        Self {
            iter: self.iter.clone(),
        }
    }
}

impl<'a, Tag, T: fmt::Debug> fmt::Debug for Values<'a, Tag, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Values").field("iter", &self.iter).finish()
    }
}

impl<'a, Tag, T> Iterator for Values<'a, Tag, T> {
    type Item = &'a T;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.iter.next()?.1)
    }
}

pub struct IterMut<'a, Tag, T> {
    iter: Enumerate<slice::IterMut<'a, Option<Entry<T>>>>,
    tag: PhantomData<Tag>,
}

impl<'a, Tag, T: fmt::Debug> fmt::Debug for IterMut<'a, Tag, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IterMut").field("iter", &self.iter).finish()
    }
}

impl<'a, Tag, T> Iterator for IterMut<'a, Tag, T> {
    type Item = (GenId<Tag>, &'a mut T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let (index, Some(entry)) = self.iter.next()? {
                break Some((
                    GenId {
                        index,
                        generation: entry.generation,
                        tag: PhantomData,
                    },
                    &mut entry.value,
                ));
            }
        }
    }
}

pub struct ValuesMut<'a, Tag, T> {
    iter: IterMut<'a, Tag, T>,
}

impl<'a, Tag, T: fmt::Debug> fmt::Debug for ValuesMut<'a, Tag, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ValuesMut")
            .field("iter", &self.iter)
            .finish()
    }
}

impl<'a, Tag, T> Iterator for ValuesMut<'a, Tag, T> {
    type Item = &'a mut T;

    fn next(&mut self) -> Option<Self::Item> {
        Some(self.iter.next()?.1)
    }
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Entry<T> {
    generation: usize,
    value: T,
}
