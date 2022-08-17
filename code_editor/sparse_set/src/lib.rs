use std::slice;

/// A sparse set of integers.
/// 
/// Based on: [https://research.swtch.com/sparse](https://research.swtch.com/sparse).
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SparseSet {
    dense: Vec<usize>,
    sparse: Box<[usize]>,
}

impl SparseSet {
    /// Creates a new `SparseSet` with at least the given `capacity`.
    /// 
    /// # Performance
    /// 
    /// Runs in O(1) time.
    pub fn new(capacity: usize) -> Self {
        Self {
            dense: Vec::with_capacity(capacity),
            sparse: vec![0; capacity].into_boxed_slice(),
        }
    }

    /// Returns `true` if `self` is empty.
    /// 
    /// # Performance
    /// 
    /// Runs in O(1) time.
    pub fn is_empty(&self) -> bool {
        self.dense.is_empty()
    }

    /// Returns the number of integers `self` can hold.
    /// 
    /// # Performance
    /// 
    /// Runs in O(1) time.
    pub fn capacity(&self) -> usize {
        self.sparse.len()
    }

    /// Returns a slice of all integers in `self`, in insertion order.
    ///
    /// # Performance
    /// 
    /// Runs in O(1) time.
    pub fn as_slice(&self) -> &[usize] {
        self.dense.as_slice()
    }

    /// Returns true if `self` contains the integer `value`.
    /// 
    /// # Performance
    /// 
    /// Runs in O(1) time.
    pub fn contains(&self, value: usize) -> bool {
        self.dense.get(self.sparse[value]) == Some(&value)
    }

    /// Returns an iterator over the integers in `self`.
    /// 
    /// # Performance
    /// 
    /// Runs in O(1) time.
    pub fn iter(&self) -> Iter {
        Iter {
            iter: self.dense.iter(),
        }
    }

    /// Adds the integer `value` to `self`.
    /// 
    /// # Performance
    /// 
    /// Runs in O(1) time.
    /// 
    /// # Panics
    /// 
    /// Panics if `value` is greater than or equal to the capacity of `self`.
    pub fn insert(&mut self, value: usize) -> bool {
        if self.contains(value) {
            return false;
        }
        let index = self.dense.len();
        self.dense.push(value);
        self.sparse[value] = index;
        true
    }

    /// Clears `self`, removing all integers.
    /// 
    /// # Performance
    /// 
    /// Runs in O(1) time.
    pub fn clear(&mut self) {
        self.dense.clear();
    }
}

impl<'a> IntoIterator for &'a SparseSet {
    type Item = &'a usize;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// An iterator over the values in a [`SparseSet`].
#[derive(Clone, Debug)]
pub struct Iter<'a> {
    iter: slice::Iter<'a, usize>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a usize;

    /// Advances the iterator and returns the next integer.
    /// 
    /// # Performance
    /// 
    /// Runs in O(1) time.
    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}

#[cfg(test)]
mod tests {
    use {proptest::prelude::*, super::*};

    proptest! {
        #[test]
        fn insert(values in prop::collection::vec(0..256usize, ..256)) {
            let mut sparse_set = SparseSet::new(256);
            for &value in &values {
                sparse_set.insert(value);
            }
            assert!(values.iter().all(|&value| sparse_set.contains(value)));
        }
    }
}