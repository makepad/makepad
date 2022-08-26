use std::slice;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct SparseSet {
    dense: Vec<usize>,
    sparse: Box<[usize]>,
}

impl SparseSet {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            dense: Vec::with_capacity(capacity),
            sparse: vec![0; capacity].into_boxed_slice(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.dense.is_empty()
    }

    pub(crate) fn capacity(&self) -> usize {
        self.sparse.len()
    }

    pub(crate) fn as_slice(&self) -> &[usize] {
        self.dense.as_slice()
    }

    pub(crate) fn contains(&self, value: usize) -> bool {
        self.dense.get(self.sparse[value]) == Some(&value)
    }

    pub(crate) fn iter(&self) -> Iter {
        Iter {
            iter: self.dense.iter(),
        }
    }

    pub(crate) fn insert(&mut self, value: usize) -> bool {
        if self.contains(value) {
            return false;
        }
        let index = self.dense.len();
        self.dense.push(value);
        self.sparse[value] = index;
        true
    }

    pub(crate) fn clear(&mut self) {
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

#[derive(Clone, Debug)]
pub(crate) struct Iter<'a> {
    iter: slice::Iter<'a, usize>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = &'a usize;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next()
    }
}
