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

    pub fn contains(&self, value: usize) -> bool {
        self.dense.get(self.sparse[value]) == Some(&value)
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