/// A sparse set of integers.
///
/// The implementation is based on: https://research.swtch.com/sparse.
///
/// The vector `dense` contains a list of integers in insertion order. The boxed slice `sparse`
/// contains a mapping from integers to their indices in `dense`. An integer is in the `SparseSet`
/// if `sparse` maps the integer to a valid index in `dense`, and the integer at that index in
/// `dense` has the same value.
///
/// All methods on a `SparseSet` take *O*(1) time.
#[derive(Clone, Debug)]
pub struct SparseSet {
    dense: Vec<usize>,
    sparse: Box<[usize]>,
}

impl SparseSet {
    /// Returns a new `SparseSet` with space for all integers ranging from `0` to `max`.
    pub fn new(max: usize) -> Self {
        Self {
            dense: Vec::with_capacity(max),
            sparse: vec![0; max].into_boxed_slice(),
        }
    }

    /// Returns `true` if the `SparseSet` is empty.
    pub fn is_empty(&self) -> bool {
        self.dense.is_empty()
    }

    /// Returns a slice of the integers in the `SparseSet`, in insertion order.
    pub fn as_slice(&self) -> &[usize] {
        self.dense.as_slice()
    }

    /// Returns `true` if the `SparseSet` contains the integer `value`.
    pub fn contains(&self, value: usize) -> bool {
        self.dense.get(self.sparse[value]) == Some(&value)
    }

    /// Adds the integer `value` to the `SparseSet`.
    ///
    /// Returns `true` if the `SparseSet` did not already contain `value`.
    pub fn insert(&mut self, value: usize) -> bool {
        if self.contains(value) {
            return false;
        }
        let index = self.dense.len();
        self.dense.push(value);
        self.sparse[value] = index;
        true
    }

    /// Removes all integers from the `SparseSet`.
    pub fn clear(&mut self) {
        self.dense.clear()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_empty() {
        let mut set = SparseSet::new(8);
        assert!(set.is_empty());
        set.insert(4);
        assert!(!set.is_empty());
        set.clear();
        assert!(set.is_empty());
    }

    #[test]
    fn as_slice() {
        let mut set = SparseSet::new(8);
        assert_eq!(set.as_slice(), &[]);
        set.insert(6);
        set.insert(4);
        set.insert(4);
        set.insert(2);
        assert_eq!(set.as_slice(), &[6, 4, 2]);
        set.clear();
        assert_eq!(set.as_slice(), &[]);
    }

    #[test]
    fn contains() {
        let mut set = SparseSet::new(8);
        assert!(!set.contains(4));
        set.insert(4);
        assert!(set.contains(4));
        set.clear();
        assert!(!set.contains(4));
    }
}
