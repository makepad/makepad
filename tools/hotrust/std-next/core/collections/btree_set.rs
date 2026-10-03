//! BTreeSet: a BTreeMap with `()` values.

use super::btree_map::{self, BTreeMap};
use crate::borrow::Borrow;
use crate::cmp::Ordering;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::iter::{FromIterator, FusedIterator, Peekable};
use crate::ops::{BitAnd, BitOr, BitXor, RangeBounds, Sub};

pub struct BTreeSet<T> {
    map: BTreeMap<T, ()>,
}

impl<T> BTreeSet<T> {
    pub const fn new() -> BTreeSet<T> {
        BTreeSet { map: BTreeMap::new() }
    }
    pub fn iter(&self) -> Iter<'_, T> {
        Iter { iter: self.map.keys() }
    }
    pub fn len(&self) -> usize {
        self.map.len()
    }
    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
    pub fn clear(&mut self) {
        self.map.clear()
    }
    pub fn first(&self) -> Option<&T> {
        match self.map.first_key_value() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    pub fn last(&self) -> Option<&T> {
        match self.map.last_key_value() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    pub fn pop_first(&mut self) -> Option<T> {
        match self.map.pop_first() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    pub fn pop_last(&mut self) -> Option<T> {
        match self.map.pop_last() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    pub fn retain<F: FnMut(&T) -> bool>(&mut self, f: F) {
        let mut f = f;
        self.map.retain(|k, _| f(k));
    }
}

impl<T: Ord> BTreeSet<T> {
    pub fn range<K: ?Sized + Ord, R: RangeBounds<K>>(&self, range: R) -> Range<'_, T>
    where
        T: Borrow<K>,
    {
        Range { iter: self.map.range(range) }
    }
    pub fn difference<'a>(&'a self, other: &'a BTreeSet<T>) -> Difference<'a, T> {
        Difference { a: self.iter().peekable(), b: other.iter().peekable() }
    }
    pub fn symmetric_difference<'a>(&'a self, other: &'a BTreeSet<T>) -> SymmetricDifference<'a, T> {
        SymmetricDifference { a: self.iter().peekable(), b: other.iter().peekable() }
    }
    pub fn intersection<'a>(&'a self, other: &'a BTreeSet<T>) -> Intersection<'a, T> {
        Intersection { a: self.iter().peekable(), b: other.iter().peekable() }
    }
    pub fn union<'a>(&'a self, other: &'a BTreeSet<T>) -> Union<'a, T> {
        Union { a: self.iter().peekable(), b: other.iter().peekable() }
    }
    pub fn contains<Q: ?Sized + Ord>(&self, value: &Q) -> bool
    where
        T: Borrow<Q>,
    {
        self.map.contains_key(value)
    }
    pub fn get<Q: ?Sized + Ord>(&self, value: &Q) -> Option<&T>
    where
        T: Borrow<Q>,
    {
        match self.map.get_key_value(value) {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    pub fn is_disjoint(&self, other: &BTreeSet<T>) -> bool {
        self.intersection(other).next().is_none()
    }
    pub fn is_subset(&self, other: &BTreeSet<T>) -> bool {
        if self.len() > other.len() {
            return false;
        }
        self.iter().all(|v| other.contains(v))
    }
    pub fn is_superset(&self, other: &BTreeSet<T>) -> bool {
        other.is_subset(self)
    }
    /// Adds a value; returns whether it was newly inserted (an equal value is kept).
    pub fn insert(&mut self, value: T) -> bool {
        match self.map.entry(value) {
            btree_map::Entry::Occupied(_) => false,
            btree_map::Entry::Vacant(e) => {
                e.insert(());
                true
            }
        }
    }
    pub fn replace(&mut self, value: T) -> Option<T> {
        let old = self.map.remove_entry(&value);
        self.map.insert(value, ());
        match old {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    pub fn remove<Q: ?Sized + Ord>(&mut self, value: &Q) -> bool
    where
        T: Borrow<Q>,
    {
        self.map.remove(value).is_some()
    }
    pub fn take<Q: ?Sized + Ord>(&mut self, value: &Q) -> Option<T>
    where
        T: Borrow<Q>,
    {
        match self.map.remove_entry(value) {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    pub fn append(&mut self, other: &mut BTreeSet<T>) {
        self.map.append(&mut other.map);
    }
    pub fn split_off<Q: ?Sized + Ord>(&mut self, value: &Q) -> BTreeSet<T>
    where
        T: Borrow<Q>,
    {
        BTreeSet { map: self.map.split_off(value) }
    }
}

impl<T: Clone> Clone for BTreeSet<T> {
    fn clone(&self) -> BTreeSet<T> {
        BTreeSet { map: self.map.clone() }
    }
}

impl<T> Default for BTreeSet<T> {
    fn default() -> BTreeSet<T> {
        BTreeSet::new()
    }
}

impl<T: PartialEq> PartialEq for BTreeSet<T> {
    fn eq(&self, other: &BTreeSet<T>) -> bool {
        self.map == other.map
    }
}

impl<T: Eq> Eq for BTreeSet<T> {}

impl<T: PartialOrd> PartialOrd for BTreeSet<T> {
    fn partial_cmp(&self, other: &BTreeSet<T>) -> Option<Ordering> {
        self.iter().partial_cmp(other.iter())
    }
}

impl<T: Ord> Ord for BTreeSet<T> {
    fn cmp(&self, other: &BTreeSet<T>) -> Ordering {
        self.iter().cmp(other.iter())
    }
}

impl<T: Hash> Hash for BTreeSet<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.len());
        for x in self.iter() {
            x.hash(state);
        }
    }
}

impl<T: fmt::Debug> fmt::Debug for BTreeSet<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}

impl<T: Ord> FromIterator<T> for BTreeSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> BTreeSet<T> {
        let mut set = BTreeSet::new();
        for x in iter {
            set.insert(x);
        }
        set
    }
}

impl<T: Ord> Extend<T> for BTreeSet<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for x in iter {
            self.insert(x);
        }
    }
}

impl<'a, T: 'a + Ord + Copy> Extend<&'a T> for BTreeSet<T> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        for x in iter {
            self.insert(*x);
        }
    }
}

impl<'a, 'b, T: Ord + Clone> Sub<&'b BTreeSet<T>> for &'a BTreeSet<T> {
    type Output = BTreeSet<T>;
    fn sub(self, rhs: &BTreeSet<T>) -> BTreeSet<T> {
        self.difference(rhs).cloned().collect()
    }
}

impl<'a, 'b, T: Ord + Clone> BitXor<&'b BTreeSet<T>> for &'a BTreeSet<T> {
    type Output = BTreeSet<T>;
    fn bitxor(self, rhs: &BTreeSet<T>) -> BTreeSet<T> {
        self.symmetric_difference(rhs).cloned().collect()
    }
}

impl<'a, 'b, T: Ord + Clone> BitAnd<&'b BTreeSet<T>> for &'a BTreeSet<T> {
    type Output = BTreeSet<T>;
    fn bitand(self, rhs: &BTreeSet<T>) -> BTreeSet<T> {
        self.intersection(rhs).cloned().collect()
    }
}

impl<'a, 'b, T: Ord + Clone> BitOr<&'b BTreeSet<T>> for &'a BTreeSet<T> {
    type Output = BTreeSet<T>;
    fn bitor(self, rhs: &BTreeSet<T>) -> BTreeSet<T> {
        self.union(rhs).cloned().collect()
    }
}

// ---------------------------------------------------------------- iterators

pub struct Iter<'a, T: 'a> {
    iter: btree_map::Keys<'a, T, ()>,
}

impl<'a, T> Clone for Iter<'a, T> {
    fn clone(&self) -> Iter<'a, T> {
        Iter { iter: self.iter.clone() }
    }
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        self.iter.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
    fn last(self) -> Option<&'a T> {
        let mut s = self;
        s.next_back()
    }
}

impl<'a, T> DoubleEndedIterator for Iter<'a, T> {
    fn next_back(&mut self) -> Option<&'a T> {
        self.iter.next_back()
    }
}

impl<'a, T> ExactSizeIterator for Iter<'a, T> {}
impl<'a, T> FusedIterator for Iter<'a, T> {}

pub struct IntoIter<T> {
    iter: btree_map::IntoKeys<T, ()>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.iter.next()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<T> DoubleEndedIterator for IntoIter<T> {
    fn next_back(&mut self) -> Option<T> {
        self.iter.next_back()
    }
}

impl<T> ExactSizeIterator for IntoIter<T> {}

pub struct Range<'a, T: 'a> {
    iter: btree_map::Range<'a, T, ()>,
}

impl<'a, T> Clone for Range<'a, T> {
    fn clone(&self) -> Range<'a, T> {
        Range { iter: self.iter.clone() }
    }
}

impl<'a, T> Iterator for Range<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        match self.iter.next() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
}

impl<'a, T> DoubleEndedIterator for Range<'a, T> {
    fn next_back(&mut self) -> Option<&'a T> {
        match self.iter.next_back() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
}

/// Sorted-merge set operations over two ordered iterators.
pub struct Difference<'a, T: 'a> {
    a: Peekable<Iter<'a, T>>,
    b: Peekable<Iter<'a, T>>,
}

impl<'a, T: Ord> Iterator for Difference<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        loop {
            let x = *self.a.peek()?;
            match self.b.peek() {
                None => return self.a.next(),
                Some(y) => match x.cmp(*y) {
                    Ordering::Less => return self.a.next(),
                    Ordering::Equal => {
                        self.a.next();
                        self.b.next();
                    }
                    Ordering::Greater => {
                        self.b.next();
                    }
                },
            }
        }
    }
}

impl<'a, T: Ord> FusedIterator for Difference<'a, T> {}

pub struct SymmetricDifference<'a, T: 'a> {
    a: Peekable<Iter<'a, T>>,
    b: Peekable<Iter<'a, T>>,
}

impl<'a, T: Ord> Iterator for SymmetricDifference<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        loop {
            let ord = match (self.a.peek(), self.b.peek()) {
                (None, None) => return None,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(x), Some(y)) => (*x).cmp(*y),
            };
            match ord {
                Ordering::Less => return self.a.next(),
                Ordering::Greater => return self.b.next(),
                Ordering::Equal => {
                    self.a.next();
                    self.b.next();
                }
            }
        }
    }
}

impl<'a, T: Ord> FusedIterator for SymmetricDifference<'a, T> {}

pub struct Intersection<'a, T: 'a> {
    a: Peekable<Iter<'a, T>>,
    b: Peekable<Iter<'a, T>>,
}

impl<'a, T: Ord> Iterator for Intersection<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        loop {
            let x = *self.a.peek()?;
            let y = *self.b.peek()?;
            match x.cmp(y) {
                Ordering::Less => {
                    self.a.next();
                }
                Ordering::Greater => {
                    self.b.next();
                }
                Ordering::Equal => {
                    self.b.next();
                    return self.a.next();
                }
            }
        }
    }
}

impl<'a, T: Ord> FusedIterator for Intersection<'a, T> {}

pub struct Union<'a, T: 'a> {
    a: Peekable<Iter<'a, T>>,
    b: Peekable<Iter<'a, T>>,
}

impl<'a, T: Ord> Iterator for Union<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        let ord = match (self.a.peek(), self.b.peek()) {
            (None, None) => return None,
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (Some(x), Some(y)) => (*x).cmp(*y),
        };
        match ord {
            Ordering::Less => self.a.next(),
            Ordering::Greater => self.b.next(),
            Ordering::Equal => {
                self.b.next();
                self.a.next()
            }
        }
    }
}

impl<'a, T: Ord> FusedIterator for Union<'a, T> {}

impl<'a, T> IntoIterator for &'a BTreeSet<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<T> IntoIterator for BTreeSet<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> IntoIter<T> {
        IntoIter { iter: self.map.into_keys() }
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<T: Ord, const N: usize> From<[T; N]> for BTreeSet<T> {
    fn from(arr: [T; N]) -> BTreeSet<T> {
        let mut set = BTreeSet::new();
        for x in arr {
            set.insert(x);
        }
        set
    }
}
