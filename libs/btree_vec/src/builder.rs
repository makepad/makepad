use {
    crate::{BTreeVec, Leaf, Metric},
    btree,
    btree::Leaf as _,
    std::mem,
};

pub struct Builder<T, M>
where
    T: Clone,
    M: Metric<T>,
{
    builder: btree::Builder<Leaf<T, M>>,
    leaf: Leaf<T, M>,
}

impl<T, M> Builder<T, M>
where
    T: Clone,
    M: Metric<T>,
{
    pub fn new() -> Self {
        Self {
            builder: btree::Builder::new(),
            leaf: Leaf::new(),
        }
    }

    pub fn finish(self) -> BTreeVec<T, M> {
        BTreeVec::from_btree(self.builder.finish(self.leaf))
    }

    pub fn push(&mut self, item: T) {
        self.leaf.push(item);
        if self.leaf.is_full() {
            self.builder.push(mem::replace(&mut self.leaf, Leaf::new()));
        }
    }
}
