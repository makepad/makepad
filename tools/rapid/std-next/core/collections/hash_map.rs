//! HashMap over the open-addressing RawTable. Same API and semantics as real std's, except
//! iteration order (deterministic here, see hash/fast.rs).

use super::raw_table::{next_full, RawTable};
use crate::borrow::Borrow;
use crate::fmt;
use crate::hash::{BuildHasher, Hash};
use crate::iter::{FromIterator, FusedIterator};
use crate::marker::PhantomData;
use crate::ops::Index;

pub use crate::hash::{DefaultHasher, RandomState};

pub struct HashMap<K, V, S = RandomState> {
    table: RawTable<(K, V)>,
    hash_builder: S,
}

impl<K, V> HashMap<K, V, RandomState> {
    pub fn new() -> HashMap<K, V, RandomState> {
        HashMap { table: RawTable::new(), hash_builder: RandomState::new() }
    }
    pub fn with_capacity(capacity: usize) -> HashMap<K, V, RandomState> {
        HashMap { table: RawTable::with_capacity(capacity), hash_builder: RandomState::new() }
    }
}

impl<K, V, S> HashMap<K, V, S> {
    pub const fn with_hasher(hash_builder: S) -> HashMap<K, V, S> {
        HashMap { table: RawTable::new(), hash_builder }
    }
    pub fn with_capacity_and_hasher(capacity: usize, hasher: S) -> HashMap<K, V, S> {
        HashMap { table: RawTable::with_capacity(capacity), hash_builder: hasher }
    }
    pub fn capacity(&self) -> usize {
        self.table.capacity()
    }
    pub fn keys(&self) -> Keys<'_, K, V> {
        Keys { inner: self.iter() }
    }
    pub fn into_keys(self) -> IntoKeys<K, V> {
        IntoKeys { inner: self.into_iter() }
    }
    pub fn values(&self) -> Values<'_, K, V> {
        Values { inner: self.iter() }
    }
    pub fn values_mut(&mut self) -> ValuesMut<'_, K, V> {
        ValuesMut { inner: self.iter_mut() }
    }
    pub fn into_values(self) -> IntoValues<K, V> {
        IntoValues { inner: self.into_iter() }
    }
    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter { table: &self.table, pos: 0, remaining: self.table.len() }
    }
    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        let remaining = self.table.len();
        IterMut { table: &mut self.table as *mut RawTable<(K, V)>, pos: 0, remaining, _m: PhantomData }
    }
    pub fn len(&self) -> usize {
        self.table.len()
    }
    pub fn is_empty(&self) -> bool {
        self.table.len() == 0
    }
    pub fn drain(&mut self) -> Drain<'_, K, V> {
        let remaining = self.table.len();
        Drain { table: &mut self.table, pos: 0, remaining }
    }
    pub fn retain<F: FnMut(&K, &mut V) -> bool>(&mut self, f: F) {
        let mut f = f;
        self.table.retain(&mut |kv: &mut (K, V)| f(&kv.0, &mut kv.1));
    }
    pub fn clear(&mut self) {
        self.table.clear();
    }
    pub fn hasher(&self) -> &S {
        &self.hash_builder
    }
}

impl<K: Eq + Hash, V, S: BuildHasher> HashMap<K, V, S> {
    fn hash_of<Q: Hash + ?Sized>(&self, k: &Q) -> u64 {
        self.hash_builder.hash_one(k)
    }

    fn find<Q: ?Sized + Hash + Eq>(&self, k: &Q) -> Option<usize>
    where
        K: Borrow<Q>,
    {
        if self.table.len() == 0 {
            return None;
        }
        let hash = self.hash_of(k);
        self.table.find(hash, &mut |kv: &(K, V)| kv.0.borrow() == k)
    }

    pub fn reserve(&mut self, additional: usize) {
        let hb = &self.hash_builder;
        self.table.reserve(additional, &|kv: &(K, V)| hb.hash_one(&kv.0));
    }

    pub fn try_reserve(&mut self, additional: usize) -> Result<(), super::TryReserveError> {
        self.reserve(additional);
        Ok(())
    }

    pub fn shrink_to_fit(&mut self) {
        let hb = &self.hash_builder;
        self.table.shrink_to(0, &|kv: &(K, V)| hb.hash_one(&kv.0));
    }

    pub fn shrink_to(&mut self, min_capacity: usize) {
        let hb = &self.hash_builder;
        self.table.shrink_to(min_capacity, &|kv: &(K, V)| hb.hash_one(&kv.0));
    }

    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        let hash = self.hash_of(&key);
        if let Some(i) = self.table.find(hash, &mut |kv: &(K, V)| kv.0 == key) {
            return Entry::Occupied(OccupiedEntry { table: &mut self.table, index: i, key: Some(key) });
        }
        let hb = &self.hash_builder;
        self.table.reserve(1, &|kv: &(K, V)| hb.hash_one(&kv.0));
        let slot = self.table.find_insert_slot(hash);
        Entry::Vacant(VacantEntry { table: &mut self.table, hash, slot, key })
    }

    pub fn get<Q: ?Sized + Hash + Eq>(&self, k: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        match self.find(k) {
            Some(i) => Some(unsafe { &self.table.get(i).1 }),
            None => None,
        }
    }

    pub fn get_key_value<Q: ?Sized + Hash + Eq>(&self, k: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
    {
        match self.find(k) {
            Some(i) => {
                let kv = unsafe { self.table.get(i) };
                Some((&kv.0, &kv.1))
            }
            None => None,
        }
    }

    pub fn contains_key<Q: ?Sized + Hash + Eq>(&self, k: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.find(k).is_some()
    }

    pub fn get_mut<Q: ?Sized + Hash + Eq>(&mut self, k: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        match self.find(k) {
            Some(i) => Some(unsafe { &mut self.table.get_mut(i).1 }),
            None => None,
        }
    }

    /// Inserts; on an existing key the value is replaced (the key is kept, as in real std).
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        let hash = self.hash_of(&k);
        if let Some(i) = self.table.find(hash, &mut |kv: &(K, V)| kv.0 == k) {
            let slot = unsafe { &mut self.table.get_mut(i).1 };
            return Some(crate::mem::replace(slot, v));
        }
        let hb = &self.hash_builder;
        self.table.insert(hash, (k, v), &|kv: &(K, V)| hb.hash_one(&kv.0));
        None
    }

    pub fn try_insert(&mut self, key: K, value: V) -> Result<&mut V, OccupiedError<'_, K, V>> {
        match self.entry(key) {
            Entry::Occupied(entry) => Err(OccupiedError { entry, value }),
            Entry::Vacant(entry) => Ok(entry.insert(value)),
        }
    }

    pub fn remove<Q: ?Sized + Hash + Eq>(&mut self, k: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        match self.find(k) {
            Some(i) => Some(self.table.remove(i).1),
            None => None,
        }
    }

    pub fn remove_entry<Q: ?Sized + Hash + Eq>(&mut self, k: &Q) -> Option<(K, V)>
    where
        K: Borrow<Q>,
    {
        match self.find(k) {
            Some(i) => Some(self.table.remove(i)),
            None => None,
        }
    }

    pub fn extract_if<F: FnMut(&K, &mut V) -> bool>(&mut self, pred: F) -> ExtractIf<'_, K, V, F> {
        ExtractIf { table: &mut self.table, pos: 0, pred }
    }
}

impl<K: Clone, V: Clone, S: Clone> Clone for HashMap<K, V, S> {
    fn clone(&self) -> HashMap<K, V, S> {
        HashMap { table: self.table.clone_table(), hash_builder: self.hash_builder.clone() }
    }
}

impl<K: Eq + Hash, V: PartialEq, S: BuildHasher> PartialEq for HashMap<K, V, S> {
    fn eq(&self, other: &HashMap<K, V, S>) -> bool {
        if self.len() != other.len() {
            return false;
        }
        self.iter().all(|(key, value)| match other.get(key) {
            Some(v) => *value == *v,
            None => false,
        })
    }
}

impl<K: Eq + Hash, V: Eq, S: BuildHasher> Eq for HashMap<K, V, S> {}

impl<K: fmt::Debug, V: fmt::Debug, S> fmt::Debug for HashMap<K, V, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K, V, S: Default> Default for HashMap<K, V, S> {
    fn default() -> HashMap<K, V, S> {
        HashMap::with_hasher(S::default())
    }
}

impl<'a, K: Eq + Hash + Borrow<Q>, Q: ?Sized + Eq + Hash, V, S: BuildHasher> Index<&'a Q> for HashMap<K, V, S> {
    type Output = V;
    fn index(&self, key: &Q) -> &V {
        match self.get(key) {
            Some(v) => v,
            None => crate::panicking::panic_str("no entry found for key"),
        }
    }
}

impl<K: Eq + Hash, V, S: BuildHasher + Default> FromIterator<(K, V)> for HashMap<K, V, S> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> HashMap<K, V, S> {
        let mut map = HashMap::with_hasher(S::default());
        map.extend(iter);
        map
    }
}

impl<K: Eq + Hash, V, S: BuildHasher> Extend<(K, V)> for HashMap<K, V, S> {
    fn extend<T: IntoIterator<Item = (K, V)>>(&mut self, iter: T) {
        let iter = iter.into_iter();
        // as real std: reserve the lower bound, or half of it when not empty
        let (lower, _) = iter.size_hint();
        let reserve = if self.is_empty() { lower } else { (lower + 1) / 2 };
        self.reserve(reserve);
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

impl<'a, K: Eq + Hash + Copy, V: Copy, S: BuildHasher> Extend<(&'a K, &'a V)> for HashMap<K, V, S> {
    fn extend<T: IntoIterator<Item = (&'a K, &'a V)>>(&mut self, iter: T) {
        for (k, v) in iter {
            self.insert(*k, *v);
        }
    }
}

// ---------------------------------------------------------------- entries

pub enum Entry<'a, K: 'a, V: 'a> {
    Occupied(OccupiedEntry<'a, K, V>),
    Vacant(VacantEntry<'a, K, V>),
}

pub struct OccupiedEntry<'a, K, V> {
    table: &'a mut RawTable<(K, V)>,
    index: usize,
    /// the key passed to `entry` (real std drops it; kept for `replace_key`-style use)
    key: Option<K>,
}

pub struct VacantEntry<'a, K, V> {
    table: &'a mut RawTable<(K, V)>,
    hash: u64,
    slot: usize,
    key: K,
}

pub struct OccupiedError<'a, K: 'a, V: 'a> {
    pub entry: OccupiedEntry<'a, K, V>,
    pub value: V,
}

impl<'a, K, V> Entry<'a, K, V> {
    pub fn or_insert(self, default: V) -> &'a mut V {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(default),
        }
    }
    pub fn or_insert_with<F: FnOnce() -> V>(self, default: F) -> &'a mut V {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(default()),
        }
    }
    pub fn or_insert_with_key<F: FnOnce(&K) -> V>(self, default: F) -> &'a mut V {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let value = default(entry.key());
                entry.insert(value)
            }
        }
    }
    pub fn key(&self) -> &K {
        match *self {
            Entry::Occupied(ref entry) => entry.key(),
            Entry::Vacant(ref entry) => entry.key(),
        }
    }
    pub fn and_modify<F: FnOnce(&mut V)>(self, f: F) -> Entry<'a, K, V> {
        match self {
            Entry::Occupied(mut entry) => {
                f(entry.get_mut());
                Entry::Occupied(entry)
            }
            Entry::Vacant(entry) => Entry::Vacant(entry),
        }
    }
    pub fn insert_entry(self, value: V) -> OccupiedEntry<'a, K, V> {
        match self {
            Entry::Occupied(mut entry) => {
                entry.insert(value);
                entry
            }
            Entry::Vacant(entry) => entry.insert_entry(value),
        }
    }
}

impl<'a, K, V: Default> Entry<'a, K, V> {
    pub fn or_default(self) -> &'a mut V {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(V::default()),
        }
    }
}

impl<'a, K, V> OccupiedEntry<'a, K, V> {
    pub fn key(&self) -> &K {
        unsafe { &self.table.get(self.index).0 }
    }
    pub fn remove_entry(self) -> (K, V) {
        self.table.remove(self.index)
    }
    pub fn get(&self) -> &V {
        unsafe { &self.table.get(self.index).1 }
    }
    pub fn get_mut(&mut self) -> &mut V {
        unsafe { &mut self.table.get_mut(self.index).1 }
    }
    pub fn into_mut(self) -> &'a mut V {
        let table: &'a mut RawTable<(K, V)> = self.table;
        unsafe { &mut table.get_mut(self.index).1 }
    }
    pub fn insert(&mut self, value: V) -> V {
        crate::mem::replace(self.get_mut(), value)
    }
    pub fn remove(self) -> V {
        self.remove_entry().1
    }
    pub(crate) fn into_key_ref(self) -> &'a K {
        let table: &'a RawTable<(K, V)> = self.table;
        unsafe { &table.get(self.index).0 }
    }
}

impl<'a, K, V> VacantEntry<'a, K, V> {
    pub fn key(&self) -> &K {
        &self.key
    }
    pub fn into_key(self) -> K {
        self.key
    }
    pub fn insert(self, value: V) -> &'a mut V {
        let table: &'a mut RawTable<(K, V)> = self.table;
        let i = table.insert_at(self.slot, self.hash, (self.key, value));
        unsafe { &mut table.get_mut(i).1 }
    }
    pub fn insert_entry(self, value: V) -> OccupiedEntry<'a, K, V> {
        let i = self.table.insert_at(self.slot, self.hash, (self.key, value));
        OccupiedEntry { table: self.table, index: i, key: None }
    }
}

impl<'a, K: fmt::Debug, V: fmt::Debug> fmt::Debug for Entry<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Entry::Vacant(ref v) => f.debug_tuple("Entry").field(v).finish(),
            Entry::Occupied(ref o) => f.debug_tuple("Entry").field(o).finish(),
        }
    }
}

impl<'a, K: fmt::Debug, V: fmt::Debug> fmt::Debug for OccupiedEntry<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OccupiedEntry").field("key", self.key()).field("value", self.get()).finish_non_exhaustive()
    }
}

impl<'a, K: fmt::Debug, V> fmt::Debug for VacantEntry<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VacantEntry").field(self.key()).finish()
    }
}

impl<'a, K: fmt::Debug, V: fmt::Debug> fmt::Debug for OccupiedError<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OccupiedError")
            .field("key", self.entry.key())
            .field("old_value", self.entry.get())
            .field("new_value", &self.value)
            .finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------- iterators

pub struct Iter<'a, K: 'a, V: 'a> {
    table: &'a RawTable<(K, V)>,
    pos: usize,
    remaining: usize,
}

impl<'a, K, V> Clone for Iter<'a, K, V> {
    fn clone(&self) -> Iter<'a, K, V> {
        Iter { table: self.table, pos: self.pos, remaining: self.remaining }
    }
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<(&'a K, &'a V)> {
        if self.remaining == 0 {
            return None;
        }
        let i = next_full(self.table.ctrl(), self.pos);
        self.pos = i + 1;
        self.remaining -= 1;
        let kv: &'a (K, V) = unsafe { self.table.get(i) };
        Some((&kv.0, &kv.1))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'a, K, V> ExactSizeIterator for Iter<'a, K, V> {}
impl<'a, K, V> FusedIterator for Iter<'a, K, V> {}

impl<'a, K: fmt::Debug, V: fmt::Debug> fmt::Debug for Iter<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut l = f.debug_list();
        for (k, v) in self.clone() {
            l.entry(&super::KvDebug(k, v));
        }
        l.finish()
    }
}

pub struct IterMut<'a, K: 'a, V: 'a> {
    table: *mut RawTable<(K, V)>,
    pos: usize,
    remaining: usize,
    _m: PhantomData<&'a mut (K, V)>,
}

impl<'a, K, V> Iterator for IterMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);
    fn next(&mut self) -> Option<(&'a K, &'a mut V)> {
        if self.remaining == 0 {
            return None;
        }
        let table = unsafe { &mut *self.table };
        let i = next_full(table.ctrl(), self.pos);
        self.pos = i + 1;
        self.remaining -= 1;
        let kv: &'a mut (K, V) = unsafe { &mut *table.slot_ptr(i) };
        Some((&kv.0, &mut kv.1))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'a, K, V> ExactSizeIterator for IterMut<'a, K, V> {}
impl<'a, K, V> FusedIterator for IterMut<'a, K, V> {}

pub struct IntoIter<K, V> {
    table: RawTable<(K, V)>,
    pos: usize,
}

impl<K, V> Iterator for IntoIter<K, V> {
    type Item = (K, V);
    fn next(&mut self) -> Option<(K, V)> {
        if self.table.len() == 0 {
            return None;
        }
        let i = next_full(self.table.ctrl(), self.pos);
        self.pos = i + 1;
        Some(unsafe { self.table.take_for_iter(i) })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.table.len(), Some(self.table.len()))
    }
}

impl<K, V> ExactSizeIterator for IntoIter<K, V> {}
impl<K, V> FusedIterator for IntoIter<K, V> {}

pub struct Drain<'a, K: 'a, V: 'a> {
    table: &'a mut RawTable<(K, V)>,
    pos: usize,
    remaining: usize,
}

impl<'a, K, V> Iterator for Drain<'a, K, V> {
    type Item = (K, V);
    fn next(&mut self) -> Option<(K, V)> {
        if self.remaining == 0 {
            return None;
        }
        let i = next_full(self.table.ctrl(), self.pos);
        self.pos = i + 1;
        self.remaining -= 1;
        Some(unsafe { self.table.take_for_iter(i) })
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'a, K, V> ExactSizeIterator for Drain<'a, K, V> {}
impl<'a, K, V> FusedIterator for Drain<'a, K, V> {}

impl<'a, K, V> Drop for Drain<'a, K, V> {
    fn drop(&mut self) {
        self.table.reset_after_drain();
    }
}

pub struct ExtractIf<'a, K, V, F> {
    table: &'a mut RawTable<(K, V)>,
    pos: usize,
    pred: F,
}

impl<'a, K, V, F: FnMut(&K, &mut V) -> bool> Iterator for ExtractIf<'a, K, V, F> {
    type Item = (K, V);
    fn next(&mut self) -> Option<(K, V)> {
        while self.pos < self.table.buckets() {
            let i = self.pos;
            self.pos += 1;
            if self.table.is_full_at(i) {
                let kv = unsafe { self.table.get_mut(i) };
                if (self.pred)(&kv.0, &mut kv.1) {
                    return Some(self.table.remove(i));
                }
            }
        }
        None
    }
}

pub struct Keys<'a, K: 'a, V: 'a> {
    inner: Iter<'a, K, V>,
}

impl<'a, K, V> Clone for Keys<'a, K, V> {
    fn clone(&self) -> Keys<'a, K, V> {
        Keys { inner: self.inner.clone() }
    }
}

impl<'a, K, V> Iterator for Keys<'a, K, V> {
    type Item = &'a K;
    fn next(&mut self) -> Option<&'a K> {
        match self.inner.next() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K, V> ExactSizeIterator for Keys<'a, K, V> {}
impl<'a, K, V> FusedIterator for Keys<'a, K, V> {}

impl<'a, K: fmt::Debug, V> fmt::Debug for Keys<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.clone()).finish()
    }
}

pub struct Values<'a, K: 'a, V: 'a> {
    inner: Iter<'a, K, V>,
}

impl<'a, K, V> Clone for Values<'a, K, V> {
    fn clone(&self) -> Values<'a, K, V> {
        Values { inner: self.inner.clone() }
    }
}

impl<'a, K, V> Iterator for Values<'a, K, V> {
    type Item = &'a V;
    fn next(&mut self) -> Option<&'a V> {
        match self.inner.next() {
            Some((_, v)) => Some(v),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K, V> ExactSizeIterator for Values<'a, K, V> {}
impl<'a, K, V> FusedIterator for Values<'a, K, V> {}

impl<'a, K, V: fmt::Debug> fmt::Debug for Values<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.clone()).finish()
    }
}

pub struct ValuesMut<'a, K: 'a, V: 'a> {
    inner: IterMut<'a, K, V>,
}

impl<'a, K, V> Iterator for ValuesMut<'a, K, V> {
    type Item = &'a mut V;
    fn next(&mut self) -> Option<&'a mut V> {
        match self.inner.next() {
            Some((_, v)) => Some(v),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K, V> ExactSizeIterator for ValuesMut<'a, K, V> {}
impl<'a, K, V> FusedIterator for ValuesMut<'a, K, V> {}

pub struct IntoKeys<K, V> {
    inner: IntoIter<K, V>,
}

impl<K, V> Iterator for IntoKeys<K, V> {
    type Item = K;
    fn next(&mut self) -> Option<K> {
        match self.inner.next() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K, V> ExactSizeIterator for IntoKeys<K, V> {}

pub struct IntoValues<K, V> {
    inner: IntoIter<K, V>,
}

impl<K, V> Iterator for IntoValues<K, V> {
    type Item = V;
    fn next(&mut self) -> Option<V> {
        match self.inner.next() {
            Some((_, v)) => Some(v),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K, V> ExactSizeIterator for IntoValues<K, V> {}

impl<'a, K, V, S> IntoIterator for &'a HashMap<K, V, S> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;
    fn into_iter(self) -> Iter<'a, K, V> {
        self.iter()
    }
}

impl<'a, K, V, S> IntoIterator for &'a mut HashMap<K, V, S> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;
    fn into_iter(self) -> IterMut<'a, K, V> {
        self.iter_mut()
    }
}

impl<K, V, S> IntoIterator for HashMap<K, V, S> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;
    fn into_iter(self) -> IntoIter<K, V> {
        IntoIter { table: self.table, pos: 0 }
    }
}

#[cfg(any(rapid_check, rapid_const_generics))]
impl<K: Eq + Hash, V, const N: usize> From<[(K, V); N]> for HashMap<K, V, RandomState> {
    fn from(arr: [(K, V); N]) -> HashMap<K, V, RandomState> {
        let mut map = HashMap::with_capacity(N);
        for (k, v) in arr {
            map.insert(k, v);
        }
        map
    }
}
