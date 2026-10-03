//! HashSet: a HashMap with `()` values.

use super::hash_map::{self, HashMap};
use crate::borrow::Borrow;
use crate::fmt;
use crate::hash::{BuildHasher, Hash, RandomState};
use crate::iter::{Chain, FromIterator, FusedIterator};
use crate::ops::{BitAnd, BitOr, BitXor, Sub};

pub struct HashSet<T, S = RandomState> {
    map: HashMap<T, (), S>,
}

impl<T> HashSet<T, RandomState> {
    pub fn new() -> HashSet<T, RandomState> {
        HashSet { map: HashMap::new() }
    }
    pub fn with_capacity(capacity: usize) -> HashSet<T, RandomState> {
        HashSet { map: HashMap::with_capacity(capacity) }
    }
}

impl<T, S> HashSet<T, S> {
    pub fn capacity(&self) -> usize {
        self.map.capacity()
    }
    pub fn iter(&self) -> Iter<'_, T> {
        Iter { inner: self.map.keys() }
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn drain(&mut self) -> Drain<'_, T> {
        Drain { inner: self.map.drain() }
    }
    pub fn retain<F: FnMut(&T) -> bool>(&mut self, f: F) {
        let mut f = f;
        self.map.retain(|k, _| f(k));
    }
    pub fn clear(&mut self) {
        self.map.clear()
    }
    pub const fn with_hasher(hasher: S) -> HashSet<T, S> {
        HashSet { map: HashMap::with_hasher(hasher) }
    }
    pub fn with_capacity_and_hasher(capacity: usize, hasher: S) -> HashSet<T, S> {
        HashSet { map: HashMap::with_capacity_and_hasher(capacity, hasher) }
    }
    pub fn hasher(&self) -> &S {
        self.map.hasher()
    }
}

impl<T: Eq + Hash, S: BuildHasher> HashSet<T, S> {
    pub fn reserve(&mut self, additional: usize) {
        self.map.reserve(additional)
    }
    pub fn shrink_to_fit(&mut self) {
        self.map.shrink_to_fit()
    }
    pub fn shrink_to(&mut self, min_capacity: usize) {
        self.map.shrink_to(min_capacity)
    }
    pub fn difference<'a>(&'a self, other: &'a HashSet<T, S>) -> Difference<'a, T, S> {
        Difference { iter: self.iter(), other }
    }
    pub fn symmetric_difference<'a>(&'a self, other: &'a HashSet<T, S>) -> SymmetricDifference<'a, T, S> {
        SymmetricDifference { iter: self.difference(other).chain(other.difference(self)) }
    }
    pub fn intersection<'a>(&'a self, other: &'a HashSet<T, S>) -> Intersection<'a, T, S> {
        if self.len() <= other.len() {
            Intersection { iter: self.iter(), other }
        } else {
            Intersection { iter: other.iter(), other: self }
        }
    }
    pub fn union<'a>(&'a self, other: &'a HashSet<T, S>) -> Union<'a, T, S> {
        if self.len() >= other.len() {
            Union { iter: self.iter().chain(other.difference(self)) }
        } else {
            Union { iter: other.iter().chain(self.difference(other)) }
        }
    }
    pub fn contains<Q: ?Sized + Hash + Eq>(&self, value: &Q) -> bool
    where
        T: Borrow<Q>,
    {
        self.map.contains_key(value)
    }
    pub fn get<Q: ?Sized + Hash + Eq>(&self, value: &Q) -> Option<&T>
    where
        T: Borrow<Q>,
    {
        match self.map.get_key_value(value) {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    pub fn get_or_insert(&mut self, value: T) -> &T {
        match self.map.entry(value) {
            hash_map::Entry::Occupied(e) => e.into_key_ref(),
            hash_map::Entry::Vacant(e) => e.insert_entry(()).into_key_ref(),
        }
    }
    pub fn get_or_insert_with<Q: ?Sized + Hash + Eq, F: FnOnce(&Q) -> T>(&mut self, value: &Q, f: F) -> &T
    where
        T: Borrow<Q>,
    {
        if !self.map.contains_key(value) {
            self.map.insert(f(value), ());
        }
        match self.map.get_key_value(value) {
            Some((k, _)) => k,
            None => crate::panicking::panic_str("new value is not equal"),
        }
    }
    pub fn is_disjoint(&self, other: &HashSet<T, S>) -> bool {
        if self.len() <= other.len() {
            self.iter().all(|v| !other.contains(v))
        } else {
            other.iter().all(|v| !self.contains(v))
        }
    }
    pub fn is_subset(&self, other: &HashSet<T, S>) -> bool {
        if self.len() <= other.len() {
            self.iter().all(|v| other.contains(v))
        } else {
            false
        }
    }
    pub fn is_superset(&self, other: &HashSet<T, S>) -> bool {
        other.is_subset(self)
    }
    /// Adds a value; returns whether it was newly inserted (an equal existing value is kept).
    pub fn insert(&mut self, value: T) -> bool {
        match self.map.entry(value) {
            hash_map::Entry::Occupied(_) => false,
            hash_map::Entry::Vacant(e) => {
                e.insert(());
                true
            }
        }
    }
    /// Adds a value, replacing (and returning) an equal existing one.
    pub fn replace(&mut self, value: T) -> Option<T> {
        let old = self.map.remove_entry(&value);
        self.map.insert(value, ());
        match old {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    pub fn remove<Q: ?Sized + Hash + Eq>(&mut self, value: &Q) -> bool
    where
        T: Borrow<Q>,
    {
        self.map.remove(value).is_some()
    }
    pub fn take<Q: ?Sized + Hash + Eq>(&mut self, value: &Q) -> Option<T>
    where
        T: Borrow<Q>,
    {
        match self.map.remove_entry(value) {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
}

impl<T: Clone, S: Clone> Clone for HashSet<T, S> {
    fn clone(&self) -> HashSet<T, S> {
        HashSet { map: self.map.clone() }
    }
}

impl<T: Eq + Hash, S: BuildHasher> PartialEq for HashSet<T, S> {
    fn eq(&self, other: &HashSet<T, S>) -> bool {
        if self.len() != other.len() {
            return false;
        }
        self.iter().all(|key| other.contains(key))
    }
}

impl<T: Eq + Hash, S: BuildHasher> Eq for HashSet<T, S> {}

impl<T: fmt::Debug, S> fmt::Debug for HashSet<T, S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl<T, S: Default> Default for HashSet<T, S> {
    fn default() -> HashSet<T, S> {
        HashSet { map: HashMap::default() }
    }
}

impl<T: Eq + Hash, S: BuildHasher + Default> FromIterator<T> for HashSet<T, S> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> HashSet<T, S> {
        let mut set = HashSet::with_hasher(S::default());
        set.extend(iter);
        set
    }
}

impl<T: Eq + Hash, S: BuildHasher> Extend<T> for HashSet<T, S> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        let iter = iter.into_iter();
        let (lower, _) = iter.size_hint();
        let reserve = if self.is_empty() { lower } else { (lower + 1) / 2 };
        self.map.reserve(reserve);
        for k in iter {
            self.map.insert(k, ());
        }
    }
}

impl<'a, T: 'a + Eq + Hash + Copy, S: BuildHasher> Extend<&'a T> for HashSet<T, S> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        for k in iter {
            self.map.insert(*k, ());
        }
    }
}

impl<'a, 'b, T: Eq + Hash + Clone, S: BuildHasher + Default> BitOr<&'b HashSet<T, S>> for &'a HashSet<T, S> {
    type Output = HashSet<T, S>;
    fn bitor(self, rhs: &HashSet<T, S>) -> HashSet<T, S> {
        let mut out = HashSet::with_hasher(S::default());
        for v in self.union(rhs) {
            out.insert(v.clone());
        }
        out
    }
}

impl<'a, 'b, T: Eq + Hash + Clone, S: BuildHasher + Default> BitAnd<&'b HashSet<T, S>> for &'a HashSet<T, S> {
    type Output = HashSet<T, S>;
    fn bitand(self, rhs: &HashSet<T, S>) -> HashSet<T, S> {
        let mut out = HashSet::with_hasher(S::default());
        for v in self.intersection(rhs) {
            out.insert(v.clone());
        }
        out
    }
}

impl<'a, 'b, T: Eq + Hash + Clone, S: BuildHasher + Default> BitXor<&'b HashSet<T, S>> for &'a HashSet<T, S> {
    type Output = HashSet<T, S>;
    fn bitxor(self, rhs: &HashSet<T, S>) -> HashSet<T, S> {
        let mut out = HashSet::with_hasher(S::default());
        for v in self.symmetric_difference(rhs) {
            out.insert(v.clone());
        }
        out
    }
}

impl<'a, 'b, T: Eq + Hash + Clone, S: BuildHasher + Default> Sub<&'b HashSet<T, S>> for &'a HashSet<T, S> {
    type Output = HashSet<T, S>;
    fn sub(self, rhs: &HashSet<T, S>) -> HashSet<T, S> {
        let mut out = HashSet::with_hasher(S::default());
        for v in self.difference(rhs) {
            out.insert(v.clone());
        }
        out
    }
}

// ---------------------------------------------------------------- iterators

pub struct Iter<'a, K: 'a> {
    inner: hash_map::Keys<'a, K, ()>,
}

impl<'a, K> Clone for Iter<'a, K> {
    fn clone(&self) -> Iter<'a, K> {
        Iter { inner: self.inner.clone() }
    }
}

impl<'a, K> Iterator for Iter<'a, K> {
    type Item = &'a K;
    fn next(&mut self) -> Option<&'a K> {
        self.inner.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K> ExactSizeIterator for Iter<'a, K> {}
impl<'a, K> FusedIterator for Iter<'a, K> {}

impl<'a, K: fmt::Debug> fmt::Debug for Iter<'a, K> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.clone()).finish()
    }
}

pub struct IntoIter<K> {
    inner: hash_map::IntoKeys<K, ()>,
}

impl<K> Iterator for IntoIter<K> {
    type Item = K;
    fn next(&mut self) -> Option<K> {
        self.inner.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K> ExactSizeIterator for IntoIter<K> {}

pub struct Drain<'a, K: 'a> {
    inner: hash_map::Drain<'a, K, ()>,
}

impl<'a, K> Iterator for Drain<'a, K> {
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

impl<'a, K> ExactSizeIterator for Drain<'a, K> {}

pub struct Intersection<'a, T: 'a, S: 'a> {
    iter: Iter<'a, T>,
    other: &'a HashSet<T, S>,
}

impl<'a, T, S> Clone for Intersection<'a, T, S> {
    fn clone(&self) -> Intersection<'a, T, S> {
        Intersection { iter: self.iter.clone(), other: self.other }
    }
}

impl<'a, T: Eq + Hash, S: BuildHasher> Iterator for Intersection<'a, T, S> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        loop {
            let elt = self.iter.next()?;
            if self.other.contains(elt) {
                return Some(elt);
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (_, upper) = self.iter.size_hint();
        (0, upper)
    }
}

impl<'a, T: Eq + Hash, S: BuildHasher> FusedIterator for Intersection<'a, T, S> {}

pub struct Difference<'a, T: 'a, S: 'a> {
    iter: Iter<'a, T>,
    other: &'a HashSet<T, S>,
}

impl<'a, T, S> Clone for Difference<'a, T, S> {
    fn clone(&self) -> Difference<'a, T, S> {
        Difference { iter: self.iter.clone(), other: self.other }
    }
}

impl<'a, T: Eq + Hash, S: BuildHasher> Iterator for Difference<'a, T, S> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        loop {
            let elt = self.iter.next()?;
            if !self.other.contains(elt) {
                return Some(elt);
            }
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        let (_, upper) = self.iter.size_hint();
        (0, upper)
    }
}

impl<'a, T: Eq + Hash, S: BuildHasher> FusedIterator for Difference<'a, T, S> {}

pub struct SymmetricDifference<'a, T: 'a, S: 'a> {
    iter: Chain<Difference<'a, T, S>, Difference<'a, T, S>>,
}

impl<'a, T, S> Clone for SymmetricDifference<'a, T, S> {
    fn clone(&self) -> SymmetricDifference<'a, T, S> {
        SymmetricDifference { iter: self.iter.clone() }
    }
}

impl<'a, T: Eq + Hash, S: BuildHasher> Iterator for SymmetricDifference<'a, T, S> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        self.iter.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a, T: Eq + Hash, S: BuildHasher> FusedIterator for SymmetricDifference<'a, T, S> {}

pub struct Union<'a, T: 'a, S: 'a> {
    iter: Chain<Iter<'a, T>, Difference<'a, T, S>>,
}

impl<'a, T, S> Clone for Union<'a, T, S> {
    fn clone(&self) -> Union<'a, T, S> {
        Union { iter: self.iter.clone() }
    }
}

impl<'a, T: Eq + Hash, S: BuildHasher> Iterator for Union<'a, T, S> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        self.iter.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a, T: Eq + Hash, S: BuildHasher> FusedIterator for Union<'a, T, S> {}

impl<'a, T, S> IntoIterator for &'a HashSet<T, S> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<T, S> IntoIterator for HashSet<T, S> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> IntoIter<T> {
        IntoIter { inner: self.map.into_keys() }
    }
}

#[cfg(any(rapid_check, rapid_const_generics))]
impl<T: Eq + Hash, const N: usize> From<[T; N]> for HashSet<T, RandomState> {
    fn from(arr: [T; N]) -> HashSet<T, RandomState> {
        let mut set = HashSet::with_capacity(N);
        for x in arr {
            set.insert(x);
        }
        set
    }
}
