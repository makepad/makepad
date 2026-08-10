use crate::makepad_live_id::LiveId;

use std::{
    collections::HashMap,
    ops::{Index, IndexMut},
};

// Idea taken from the `nohash_hasher` crate.
#[derive(Default)]
pub struct ValueHasher(u64);

impl std::hash::Hasher for ValueHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!();
    }
    fn write_u8(&mut self, _n: u8) {
        unreachable!();
    }
    fn write_u16(&mut self, _n: u16) {
        unreachable!();
    }
    fn write_u32(&mut self, _n: u32) {
        unreachable!();
    }
    #[inline(always)]
    fn write_u64(&mut self, n: u64) {
        self.0 = n;
    }
    fn write_usize(&mut self, _n: usize) {
        unreachable!();
    }
    fn write_i8(&mut self, _n: i8) {
        unreachable!();
    }
    fn write_i16(&mut self, _n: i16) {
        unreachable!();
    }
    fn write_i32(&mut self, _n: i32) {
        unreachable!();
    }
    fn write_i64(&mut self, _n: i64) {
        unreachable!();
    }
    fn write_isize(&mut self, _n: isize) {
        unreachable!();
    }
    #[inline(always)]
    fn finish(&self) -> u64 {
        self.0
    }
}

#[derive(Copy, Clone, Default)]
pub struct ValueHasherBuilder {}

impl std::hash::BuildHasher for ValueHasherBuilder {
    type Hasher = ValueHasher;

    #[inline(always)]
    fn build_hasher(&self) -> ValueHasher {
        ValueHasher::default()
    }
}

/// Hybrid map: small maps (the common case — scopes, call args, entity
/// objects) are a plain Vec scanned linearly, which beats hashing both on
/// hits and (crucially, for scope-chain walks) on misses. Past SPILL_AT
/// entries the map spills to a HashMap and stays spilled until cleared.
/// Key order is not part of the map contract (callers track ordering via
/// ScriptMapTag::order), matching the previous HashMap behavior.
#[derive(Clone, Debug)]
pub struct ValueMap<K, V> {
    vec: Vec<(K, V)>,
    spill: Option<Box<HashMap<K, V, ValueHasherBuilder>>>,
}

const SPILL_AT: usize = 16;

impl<K, V> Default for ValueMap<K, V>
where
    K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId> + std::fmt::Debug,
{
    fn default() -> Self {
        Self {
            vec: Vec::new(),
            spill: None,
        }
    }
}

pub enum ValueMapIter<'a, K, V> {
    Vec(std::slice::Iter<'a, (K, V)>),
    Map(std::collections::hash_map::Iter<'a, K, V>),
}

pub enum ValueMapIterMut<'a, K, V> {
    Vec(std::slice::IterMut<'a, (K, V)>),
    Map(std::collections::hash_map::IterMut<'a, K, V>),
}

impl<'a, K, V> Iterator for ValueMapIterMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Vec(it) => it.next().map(|(k, v)| (&*k, v)),
            Self::Map(it) => it.next(),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Vec(it) => it.size_hint(),
            Self::Map(it) => it.size_hint(),
        }
    }
}

impl<'a, K, V> Iterator for ValueMapIter<'a, K, V> {
    type Item = (&'a K, &'a V);
    #[inline]
    fn next(&mut self) -> Option<Self::Item> {
        match self {
            Self::Vec(it) => it.next().map(|(k, v)| (k, v)),
            Self::Map(it) => it.next(),
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        match self {
            Self::Vec(it) => it.size_hint(),
            Self::Map(it) => it.size_hint(),
        }
    }
}

impl<K, V> ValueMap<K, V>
where
    K: std::cmp::Eq + std::hash::Hash + Copy,
{
    #[inline]
    pub fn get(&self, key: &K) -> Option<&V> {
        if let Some(spill) = &self.spill {
            return spill.get(key);
        }
        for (k, v) in self.vec.iter() {
            if k == key {
                return Some(v);
            }
        }
        None
    }

    #[inline]
    pub fn get_mut(&mut self, key: &K) -> Option<&mut V> {
        if let Some(spill) = &mut self.spill {
            return spill.get_mut(key);
        }
        for (k, v) in self.vec.iter_mut() {
            if k == key {
                return Some(v);
            }
        }
        None
    }

    #[inline]
    pub fn contains_key(&self, key: &K) -> bool {
        self.get(key).is_some()
    }

    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        if let Some(spill) = &mut self.spill {
            return spill.insert(key, value);
        }
        for (k, v) in self.vec.iter_mut() {
            if *k == key {
                return Some(std::mem::replace(v, value));
            }
        }
        if self.vec.len() >= SPILL_AT {
            let mut map = Box::new(HashMap::with_capacity_and_hasher(
                self.vec.len() + 1,
                ValueHasherBuilder {},
            ));
            for (k, v) in self.vec.drain(..) {
                map.insert(k, v);
            }
            map.insert(key, value);
            self.spill = Some(map);
            return None;
        }
        self.vec.push((key, value));
        None
    }

    pub fn remove(&mut self, key: &K) -> Option<V> {
        if let Some(spill) = &mut self.spill {
            return spill.remove(key);
        }
        for (i, (k, _)) in self.vec.iter().enumerate() {
            if k == key {
                return Some(self.vec.swap_remove(i).1);
            }
        }
        None
    }

    #[inline]
    pub fn len(&self) -> usize {
        if let Some(spill) = &self.spill {
            return spill.len();
        }
        self.vec.len()
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Clears entries. Keeps the vec's capacity for slot reuse, but drops a
    /// spilled hashmap so a reused slot starts in (cheap) linear mode again.
    pub fn clear(&mut self) {
        self.vec.clear();
        self.spill = None;
    }

    #[inline]
    pub fn iter(&self) -> ValueMapIter<'_, K, V> {
        if let Some(spill) = &self.spill {
            return ValueMapIter::Map(spill.iter());
        }
        ValueMapIter::Vec(self.vec.iter())
    }

    #[inline]
    pub fn iter_mut(&mut self) -> ValueMapIterMut<'_, K, V> {
        if let Some(spill) = &mut self.spill {
            return ValueMapIterMut::Map(spill.iter_mut());
        }
        ValueMapIterMut::Vec(self.vec.iter_mut())
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.iter().map(|(_, v)| v)
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.iter().map(|(k, _)| k)
    }
}

impl<'a, K, V> IntoIterator for &'a ValueMap<K, V>
where
    K: std::cmp::Eq + std::hash::Hash + Copy,
{
    type Item = (&'a K, &'a V);
    type IntoIter = ValueMapIter<'a, K, V>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<K, V> Index<K> for ValueMap<K, V>
where
    K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>,
{
    type Output = V;
    fn index(&self, index: K) -> &Self::Output {
        self.get(&index).unwrap()
    }
}

impl<K, V> IndexMut<K> for ValueMap<K, V>
where
    K: std::cmp::Eq + std::hash::Hash + Copy + From<LiveId>,
{
    fn index_mut(&mut self, index: K) -> &mut Self::Output {
        self.get_mut(&index).unwrap()
    }
}
