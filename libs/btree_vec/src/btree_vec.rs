use {
    crate::{
        metric::DefaultMetric, Builder, Chunks, Cursor, Info, Iter, IterRev, Leaf, Measure, Metric,
    },
    btree::BTree,
    std::{iter, ops::Index},
};

pub struct BTreeVec<T, M = DefaultMetric>
where
    M: Metric<T>,
{
    tree: BTree<Leaf<T, M>>,
}

impl<T, M> BTreeVec<T, M>
where
    T: Clone,
    M: Metric<T>,
{
    pub fn new() -> Self {
        Self::from_btree(BTree::new())
    }

    pub(super) fn from_btree(tree: BTree<Leaf<T, M>>) -> Self {
        Self { tree }
    }

    pub fn is_empty(&self) -> bool {
        self.tree.is_empty()
    }

    pub fn len(&self) -> usize {
        self.tree.len()
    }

    pub fn measure(&self) -> M::Measure {
        self.tree.info().measure
    }

    pub fn measure_to(&self, end: usize) -> M::Measure {
        self.tree.info_to(end).measure
    }

    pub fn search_by(&self, mut f: impl FnMut(M::Measure) -> bool) -> Option<(usize, M::Measure)> {
        if !f(self.tree.info().measure) {
            return None;
        }
        let (leaf, info) = self.tree.find_leaf_by(|info| f(info.measure));
        let mut acc_measure = info.measure;
        Some((
            info.len
                + leaf
                    .as_slice()
                    .iter()
                    .position(|item| {
                        let next_acc_measure = acc_measure.combine(M::measure(item));
                        if f(next_acc_measure) {
                            return true;
                        }
                        acc_measure = next_acc_measure;
                        false
                    })
                    .unwrap(),
            acc_measure,
        ))
    }

    pub fn front(&self) -> Option<&T> {
        self.get(0)
    }

    pub fn back(&self) -> Option<&T> {
        if self.is_empty() {
            return None;
        }
        Some(&self[self.len() - 1])
    }

    pub fn get(&self, index: usize) -> Option<&T> {
        if index >= self.len() {
            return None;
        }
        let (leaf, info) = self.find_leaf_by_index(index);
        Some(&leaf.as_slice()[index - info.len])
    }

    pub fn cursor_start(&self) -> Cursor<'_, T, M> {
        Cursor::new(self.tree.cursor_start())
    }

    pub fn cursor_end(&self) -> Cursor<'_, T, M> {
        Cursor::new(self.tree.cursor_end())
    }

    pub fn cursor(&self, index: usize) -> Cursor<'_, T, M> {
        Cursor::new(self.tree.cursor(index))
    }

    pub fn chunks(&self) -> Chunks<'_, T, M> {
        self.cursor_start().into_chunks()
    }

    pub fn iter(&self) -> Iter<'_, T, M> {
        self.cursor_start().into_iter()
    }

    pub fn iter_rev(&self) -> IterRev<'_, T, M> {
        self.cursor_end().into_iter_rev()
    }

    pub fn to_vec(&self) -> Vec<T> {
        self.iter().cloned().collect()
    }

    pub fn push_front(&mut self, item: T) {
        self.prepend(iter::once(item).collect())
    }

    pub fn push_back(&mut self, item: T) {
        self.append(iter::once(item).collect())
    }

    pub fn insert(&mut self, index: usize, item: T) {
        self.replace_range(index, index, iter::once(item).collect());
    }

    pub fn prepend(&mut self, other: Self) {
        self.tree.prepend(other.tree)
    }

    pub fn append(&mut self, other: Self) {
        self.tree.append(other.tree)
    }

    pub fn pop_front(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }
        Some(self.remove(0))
    }

    pub fn pop_back(&mut self) -> Option<T> {
        if self.is_empty() {
            return None;
        }
        Some(self.remove(self.len() - 1))
    }

    pub fn remove(&mut self, index: usize) -> T {
        let item = self.get(index).unwrap().clone();
        self.replace_range(index, index + 1, Self::new());
        item
    }

    pub fn remove_from(&mut self, start: usize) {
        self.tree.remove_from(start);
    }

    pub fn remove_to(&mut self, end: usize) {
        self.tree.remove_to(end);
    }

    pub fn replace_range(&mut self, start: usize, end: usize, other: Self) {
        self.tree.replace_range(start, end, other.tree);
    }

    pub fn split_off(&mut self, index: usize) -> Self {
        Self::from_btree(self.tree.split_off(index))
    }

    fn find_leaf_by_index(&self, index: usize) -> (&Leaf<T, M>, Info<M::Measure>) {
        self.tree.find_leaf_by(|info| index < info.len)
    }

    #[cfg(test)]
    pub fn assert_valid(&self) {
        self.tree.assert_valid()
    }
}

impl<T, M> Clone for BTreeVec<T, M>
where
    T: Clone,
    M: Metric<T>,
{
    fn clone(&self) -> Self {
        Self {
            tree: self.tree.clone(),
        }
    }
}

impl<T, M> From<Vec<T>> for BTreeVec<T, M>
where
    T: Clone,
    M: Metric<T>,
{
    fn from(vec: Vec<T>) -> Self {
        vec.as_slice().into()
    }
}

impl<T, M> From<&Vec<T>> for BTreeVec<T, M>
where
    T: Clone,
    M: Metric<T>,
{
    fn from(vec: &Vec<T>) -> Self {
        vec.as_slice().into()
    }
}

impl<T, M> From<&[T]> for BTreeVec<T, M>
where
    T: Clone,
    M: Metric<T>,
{
    fn from(slice: &[T]) -> Self {
        let mut builder = Builder::new();
        for item in slice {
            builder.push(item.clone());
        }
        builder.finish()
    }
}

impl<'a, T, M> FromIterator<T> for BTreeVec<T, M>
where
    T: Clone,
    M: Metric<T>,
{
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        let mut builder = Builder::new();
        for item in iter {
            builder.push(item);
        }
        builder.finish()
    }
}

impl<'a, T, M> Index<usize> for BTreeVec<T, M>
where
    T: Clone,
    M: Metric<T>,
{
    type Output = T;

    fn index(&self, index: usize) -> &Self::Output {
        self.get(index).unwrap()
    }
}
