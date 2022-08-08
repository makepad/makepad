use std::slice;

#[derive(Clone)]
pub struct SparseSet {
    dense: Vec<usize>,
    sparse: Box<[usize]>,
}

impl SparseSet {
    pub fn new(max: usize) -> Self {
        Self {
            dense: Vec::with_capacity(max),
            sparse: vec![0; max].into_boxed_slice(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.dense.is_empty()
    }

    pub fn as_slice(&self) -> &[usize] {
        self.dense.as_slice()
    }

    pub fn contains(&self, value: usize) -> bool {
        self.dense.get(self.sparse[value]) == Some(&value)
    }

    pub fn iter(&self) -> Iter {
        Iter {
            iter: self.dense.iter(),
        }
    }

    pub fn insert(&mut self, value: usize) -> bool {
        if self.contains(value) {
            return false;
        }
        let index = self.dense.len();
        self.dense.push(value);
        self.sparse[value] = index;
        true
    }

    pub fn clear(&mut self) {
        self.dense.clear();
    }
}

impl<'a> IntoIterator for &'a SparseSet {
    type Item = usize;
    type IntoIter = Iter<'a>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

pub struct Iter<'a> {
    iter: slice::Iter<'a, usize>,
}

impl<'a> Iterator for Iter<'a> {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().cloned()
    }
}