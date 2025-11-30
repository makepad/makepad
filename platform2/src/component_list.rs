use std::{
    collections::HashSet,
    ops::{Deref, DerefMut, Index, IndexMut},
};

#[derive(Clone)]
pub struct ComponentList<K, V> {
    list: Vec<(K, V)>,
    visible: HashSet<K>,
}

impl<K, V> Default for ComponentList<K, V> {
    fn default() -> Self {
        Self {
            list: Vec::new(),
            visible: HashSet::new(),
        }
    }
}

impl<K: std::cmp::Eq + std::hash::Hash + Copy, V> ComponentList<K, V> {
    pub fn retain_visible(&mut self) {
        let visible = &self.visible;
        self.list.retain(|(k, _)| visible.contains(k));
        self.visible.clear();
    }

    pub fn retain_visible_and<CB>(&mut self, cb: CB)
    where
        CB: Fn(&K, &V) -> bool,
    {
        let visible = &self.visible;
        self.list.retain(|(k, v)| visible.contains(k) || cb(k, v));
        self.visible.clear();
    }
    pub fn push(&mut self, key: K, value: V) -> () {
        self.visible.insert(key);
        self.list.push((key, value));
    }
    pub fn len(&self) -> usize {
        self.list.len()
    }
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            list: Vec::with_capacity(capacity),
            visible: HashSet::new(),
        }
    }
    pub fn extend(&mut self, iter: impl Iterator<Item = (K, V)>) {
        for (k, v) in iter {
            self.push(k, v)
        }
    }
    pub fn pop(&mut self) -> Option<(K, V)> {
        let item = self.list.pop();
        match &item {
            Some((k, _)) => {
                self.visible.remove(k);
            }
            None => (),
        }
        item
    }
    pub fn get(&self, key: usize) -> Option<&(K, V)> {
        self.list.get(key)
    }
    pub fn insert(&mut self, index: usize, item: (K, V)) {
        self.visible.insert(item.0);
        self.list.insert(index, item);
    }
}

impl<K: std::cmp::Eq + std::hash::Hash + Copy, V> Index<K> for ComponentList<K, V> {
    type Output = V;

    fn index(&self, index: K) -> &Self::Output {
        self.list
            .iter()
            .find(|(k, _)| *k == index)
            .map(|(_, v)| v)
            .unwrap()
    }
}

impl<K: std::cmp::Eq + std::hash::Hash + Copy, V> IndexMut<K> for ComponentList<K, V> {
    fn index_mut(&mut self, index: K) -> &mut Self::Output {
        self.list
            .iter_mut()
            .find(|(k, _)| *k == index)
            .map(|(_, v)| v)
            .unwrap()
    }
}

impl<K, V> Deref for ComponentList<K, V> {
    type Target = Vec<(K, V)>;

    fn deref(&self) -> &Self::Target {
        &self.list
    }
}

impl<K, V> DerefMut for ComponentList<K, V> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.list
    }
}

impl<K: std::cmp::Eq + std::hash::Hash + Copy, V> From<Vec<(K, V)>> for ComponentList<K, V> {
    fn from(value: Vec<(K, V)>) -> Self {
        let mut list = ComponentList::default();
        list.extend(value.into_iter());
        list
    }
}

impl<K: std::cmp::Eq + std::hash::Hash + Copy, V> FromIterator<(K, V)> for ComponentList<K, V> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> Self {
        let mut list = ComponentList::default();
        for (k, v) in iter {
            list.push(k, v);
        }
        list
    }
}
