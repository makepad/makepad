use {
    crate::{Branch, Cursor, Info, Leaf, Node, Slice},
    std::{fmt, mem, sync::Arc},
};

#[derive(Clone)]
pub struct BTree<L>
where
    L: Leaf,
{
    root: Option<Arc<Node<L>>>,
    height: usize,
}

impl<L> BTree<L>
where
    L: Leaf,
{
    pub fn new() -> Self {
        Self {
            root: None,
            height: 0,
        }
    }

    pub(super) fn from_leaf(leaf: L) -> Self {
        Self {
            root: Some(Arc::new(Node::Leaf(leaf))),
            height: 0,
        }
    }

    pub(super) unsafe fn from_raw_parts(root: Option<Arc<Node<L>>>, height: usize) -> Self {
        Self { root, height }
    }

    pub(super) fn into_raw_parts(self) -> (Option<Arc<Node<L>>>, usize) {
        (self.root, self.height)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn len(&self) -> usize {
        self.root.as_ref().map_or(0, |root| root.info().len())
    }

    pub fn info(&self) -> L::Info {
        self.root
            .as_ref()
            .map_or(L::Info::empty(), |root| root.info())
    }

    pub fn info_to(&self, end: usize) -> L::Info{
        let (leaf, info) = self.find_leaf_by_index(end);
        info.combine(leaf.info_to(end - info.len()))
    }

    pub(super) fn height(&self) -> usize {
        self.height
    }

    pub fn find_leaf_by(&self, f: impl FnMut(L::Info) -> bool) -> (&L, L::Info) {
        self.root.as_ref().unwrap().find_leaf_by(f)
    }

    pub fn find_leaf_by_index(&self, index: usize) -> (&L, L::Info) {
        self.root.as_ref().unwrap().find_leaf_by_index(index)
    }

    pub fn cursor_start(&self) -> Cursor<'_, L> {
        Cursor::start(self.root.as_deref(), 0, self.len())
    }

    pub fn cursor_end(&self) -> Cursor<'_, L> {
        Cursor::end(self.root.as_deref(), 0, self.len())
    }

    pub fn cursor(&self, index: usize) -> Cursor<'_, L> {
        assert!(index <= self.len());
        Cursor::new(self.root.as_deref(), 0, self.len(), index)
    }

    pub fn slice(&self, start: usize, end: usize) -> Slice<L> {
        assert!(start <= end && end <= self.len());
        Slice::new(self.root.as_deref(), start, end)
    }

    fn pull_up_singular_nodes(&mut self) {
        loop {
            match self.root.as_deref() {
                Some(Node::Branch(branch)) if branch.len() == 1 => {
                    self.root = Some(branch.nodes().first().unwrap().clone());
                    self.height -= 1;
                }
                _ => break,
            }
        }
    }

    #[doc(hidden)]
    pub fn assert_valid(&self) {
        if let Some(root) = &self.root {
            root.assert_valid();
        }
    }
}

impl<L> BTree<L>
where
    L: Leaf + Clone,
{
    pub fn prepend(&mut self, other: Self) {
        *self = other.concat(self.clone());
    }

    pub fn append(&mut self, other: Self) {
        *self = self.clone().concat(other)
    }

    pub fn remove_from(&mut self, start: usize) {
        assert!(start <= self.len());
        if start == 0 {
            *self = Self::new();
            return;
        }
        if start == self.len() {
            return;
        }
        Arc::make_mut(self.root.as_mut().unwrap()).remove_from(start);
        self.pull_up_singular_nodes();
    }

    pub fn remove_to(&mut self, end: usize) {
        assert!(end <= self.len());
        if end == 0 {
            return;
        }
        if end == self.len() {
            *self = Self::new();
            return;
        }
        Arc::make_mut(self.root.as_mut().unwrap()).remove_to(end);
        self.pull_up_singular_nodes();
    }

    pub fn replace_range(&mut self, start: usize, end: usize, mut btree: Self) {
        assert!(start <= end);
        assert!(end <= self.len());
        if end == 0 {
            mem::swap(self, &mut btree);
            self.append(btree);
            return;
        }
        if start == self.len() {
            self.append(btree);
            return;
        }
        let other = if start == end {
            self.split_off(start)
        } else {
            let mut other = self.clone();
            other.remove_to(end);
            self.remove_from(start);
            other
        };
        self.append(btree);
        self.append(other);
    }

    pub fn split_off(&mut self, index: usize) -> Self {
        assert!(index <= self.len());
        if index == 0 {
            let mut rope = Self::new();
            mem::swap(self, &mut rope);
            return rope;
        }
        if index == self.len() {
            return Self::new();
        }
        let mut other = Self {
            root: Some(Arc::make_mut(self.root.as_mut().unwrap()).split_off(index)),
            height: self.height,
        };
        self.pull_up_singular_nodes();
        other.pull_up_singular_nodes();
        other
    }

    fn concat(mut self, mut other: Self) -> Self {
        if self.is_empty() {
            return other;
        }
        if other.is_empty() {
            return self;
        }
        if self.height < other.height {
            if let Some(node) = Arc::make_mut(other.root.as_mut().unwrap()).prepend_at_depth(
                self.root.as_ref().unwrap().clone(),
                other.height - self.height,
            ) {
                let mut branch = Branch::new();
                branch.push_front(other.root.as_ref().unwrap().clone());
                branch.push_front(node);
                other.root = Some(Arc::new(Node::Branch(branch)));
                other.height += 1;
            }
            return other;
        } else {
            if let Some(node) = Arc::make_mut(self.root.as_mut().unwrap()).append_at_depth(
                other.root.as_ref().unwrap().clone(),
                self.height - other.height,
            ) {
                let mut branch = Branch::new();
                branch.push_back(self.root.as_ref().unwrap().clone());
                branch.push_back(node);
                self.root = Some(Arc::new(Node::Branch(branch)));
                self.height += 1;
            }
            return self;
        }
    }
}

impl<L> fmt::Debug for BTree<L>
where
    L: Leaf + fmt::Debug,
    L::Info: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BTree")
            .field("root", &self.root)
            .field("height", &self.height)
            .finish()
    }
}
