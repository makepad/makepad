use std::{
    fmt,
    ops::{Add, AddAssign, Deref, Index, Range, RangeBounds, Sub, SubAssign},
    slice::SliceIndex,
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct BTree<T: Chunk> {
    height: usize,
    root: Node<T>,
}

impl<T: Chunk> BTree<T> {
    pub(crate) fn new() -> Self {
        Self {
            height: 0,
            root: Node::Leaf(Leaf::new()),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn len(&self) -> usize {
        self.root.summed_len()
    }

    pub(crate) fn measure<M: Measure<T>>(&self) -> usize {
        M::measure_info(self.info())
    }

    pub(crate) fn measure_at<M: Measure<T>>(&self, position: usize) -> usize {
        if position == 0 {
            return M::measure_info(Info::new());
        }
        if position == self.len() {
            return M::measure_info(self.info());
        }
        self.root.measure_at::<M>(position)
    }

    pub(crate) fn slice<R: RangeBounds<usize>>(&self, range: R) -> Slice<'_, T> {
        use std::ops::Bound;

        let start = match range.start_bound() {
            Bound::Excluded(&start) => start + 1,
            Bound::Included(&start) => start,
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Excluded(&end) => end,
            Bound::Included(&end) => end + 1,
            Bound::Unbounded => self.len(),
        };
        assert!(start <= end);
        assert!(end <= self.len());
        Slice {
            root: &self.root,
            start,
            end,
            start_info: self.info_at(start),
            end_info: self.info_at(end),
        }
    }

    pub(crate) fn cursor_front(&self) -> Cursor<'_, T> {
        let mut cursor = Cursor::new(&self.root, 0, self.len());
        cursor.descend_left();
        cursor
    }

    pub(crate) fn cursor_back(&self) -> Cursor<'_, T> {
        let mut cursor = Cursor::new(&self.root, 0, self.len());
        cursor.descend_right();
        cursor
    }

    pub(crate) fn prepend(&mut self, mut other: Self) {
        if self.is_empty() {
            *self = other;
            return;
        }
        if other.is_empty() {
            return;
        }
        let chunk_0 = other.cursor_back().chunk();
        let mut start = chunk_0.len() - 1;
        while !chunk_0.is_boundary(start) {
            start -= 1;
        }
        let chunk_1 = self.cursor_front().chunk();
        let mut end = 1;
        while !chunk_1.is_boundary(end) {
            end += 1;
        }
        let btree = BTree {
            height: 0,
            root: Node::Leaf(Leaf::from_chunk(Arc::new(
                chunk_0.merge(start, chunk_1, end),
            ))),
        };
        other.truncate_back(other.len() - (chunk_0.len() - start));
        self.truncate_front(end);
        self.prepend_internal(btree);
        self.prepend_internal(other);
    }

    pub(crate) fn append(&mut self, mut other: Self) {
        if self.is_empty() {
            *self = other;
            return;
        }
        if other.is_empty() {
            return;
        }
        let chunk_0 = self.cursor_back().chunk();
        let mut start = chunk_0.len() - 1;
        while !chunk_0.is_boundary(start) {
            start -= 1;
        }
        let chunk_1 = other.cursor_front().chunk();
        let mut end = 1;
        while !chunk_1.is_boundary(end) {
            end += 1;
        }
        let btree = BTree {
            height: 0,
            root: Node::Leaf(Leaf::from_chunk(Arc::new(
                chunk_0.merge(start, chunk_1, end),
            ))),
        };
        other.truncate_front(end);
        self.truncate_back(self.len() - (chunk_0.len() - start));
        self.append_internal(btree);
        self.append_internal(other);
    }

    pub(crate) fn split_off(&mut self, at: usize) -> Self {
        use std::mem;

        if at == 0 {
            return mem::replace(self, Self::new());
        }
        if at == self.len() {
            return Self::new();
        }
        let mut other_root = self.root.split_off(at);
        let other_height = self.height - other_root.pull_up_singular_nodes();
        self.height -= self.root.pull_up_singular_nodes();
        Self {
            root: other_root,
            height: other_height,
        }
    }

    pub(crate) fn truncate_front(&mut self, end: usize) {
        if end == 0 {
            return;
        }
        if end == self.len() {
            *self = Self::new();
            return;
        }
        self.root.truncate_front(end);
        self.height -= self.root.pull_up_singular_nodes();
    }

    pub(crate) fn truncate_back(&mut self, start: usize) {
        if start == 0 {
            *self = Self::new();
            return;
        }
        if start == self.len() {
            return;
        }
        self.root.truncate_back(start);
        self.height -= self.root.pull_up_singular_nodes();
    }

    fn info(&self) -> T::Info {
        self.root.summed_info()
    }

    fn info_at(&self, position: usize) -> T::Info {
        if position == 0 {
            return Info::new();
        }
        if position == self.len() {
            return self.info();
        }
        self.root.info_at(position)
    }

    fn prepend_internal(&mut self, mut other: Self) {
        if self.height < other.height {
            if let Some(node) = other
                .root
                .append_at_depth(self.root.clone(), other.height - self.height)
            {
                let mut branch = Branch::new();
                branch.push_back(other.root);
                branch.push_back(node);
                other.height += 1;
                other.root = Node::Branch(branch);
            }
            *self = other;
        } else {
            if let Some(node) = self
                .root
                .prepend_at_depth(other.root, self.height - other.height)
            {
                let mut branch = Branch::new();
                branch.push_front(self.root.clone());
                branch.push_front(node);
                self.height += 1;
                self.root = Node::Branch(branch);
            }
        }
    }

    fn append_internal(&mut self, mut other: Self) {
        if self.height < other.height {
            if let Some(node) = other
                .root
                .prepend_at_depth(self.root.clone(), other.height - self.height)
            {
                let mut branch = Branch::new();
                branch.push_front(other.root);
                branch.push_front(node);
                other.height += 1;
                other.root = Node::Branch(branch);
            }
            *self = other;
        } else {
            if let Some(node) = self
                .root
                .append_at_depth(other.root, self.height - other.height)
            {
                let mut branch = Branch::new();
                branch.push_back(self.root.clone());
                branch.push_back(node);
                self.height += 1;
                self.root = Node::Branch(branch);
            }
        }
    }

}

impl<T: Chunk + fmt::Debug> fmt::Debug for BTree<T>
where
    T::Info: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BTree")
            .field("height", &self.height)
            .field("root", &self.root)
            .finish()
    }
}

pub(crate) struct Builder<T: Chunk> {
    stack: Vec<(usize, Vec<Node<T>>)>,
}

impl<T: Chunk> Builder<T> {
    pub(crate) fn new() -> Self {
        Self { stack: Vec::new() }
    }

    pub(crate) fn push_chunk(&mut self, chunk: T) {
        let mut height = 0;
        let mut node = Node::Leaf(Leaf::from_chunk(Arc::new(chunk)));
        loop {
            if self
                .stack
                .last()
                .map_or(true, |&(last_height, _)| last_height != height)
            {
                self.stack.push((height, Vec::new()));
            }
            let (_, nodes) = self.stack.last_mut().unwrap();
            nodes.push(node);
            if nodes.len() < Branch::<T>::MAX_LEN {
                break;
            }
            let (_, nodes) = self.stack.pop().unwrap();
            height += 1;
            node = Node::Branch(Branch::from_nodes(Arc::new(nodes)));
        }
    }

    pub(crate) fn build(mut self) -> BTree<T> {
        let mut btree = BTree::new();
        while let Some((height, nodes)) = self.stack.pop() {
            for root in nodes.into_iter().rev() {
                let other = BTree { height, root };
                btree.prepend_internal(other);
            }
        }
        btree
    }
}

#[derive(Clone)]
pub(crate) struct Slice<'a, T: Chunk> {
    root: &'a Node<T>,
    start: usize,
    start_info: T::Info,
    end: usize,
    end_info: T::Info,
}

impl<'a, T: Chunk> Slice<'a, T> {
    pub(crate) fn is_empty(self) -> bool {
        self.start == self.end
    }

    pub(crate) fn len(self) -> usize {
        self.end - self.start
    }

    pub(crate) fn measure<M: Measure<T>>(&self) -> usize {
        M::measure_info(self.info())
    }

    pub(crate) fn measure_at<M: Measure<T>>(&self, position: usize) -> usize {
        if position == 0 {
            return M::measure_info(Info::new());
        }
        if position == self.len() {
            return M::measure_info(self.info());
        }
        self.root.measure_at::<M>(self.start + position) - M::measure_info(self.start_info)
    }

    pub(crate) fn cursor_front(self) -> Cursor<'a, T> {
        let mut cursor = Cursor::new(self.root, self.start, self.end);
        if self.start == 0 {
            cursor.descend_left();
        } else if self.start == self.root.summed_len() {
            cursor.descend_right();
        } else {
            cursor.descend_to(self.start);
        }
        cursor
    }

    pub(crate) fn cursor_back(self) -> Cursor<'a, T> {
        let mut cursor = Cursor::new(self.root, self.start, self.end);
        if self.end == 0 {
            cursor.descend_left();
        } else if self.end == self.root.summed_len() {
            cursor.descend_right();
        } else {
            cursor.descend_to(self.end);
        }
        cursor
    }

    fn info(&self) -> T::Info {
        self.end_info - self.start_info
    }
}

impl<'a, T: Chunk> Copy for Slice<'a, T> {}

#[derive(Clone)]
pub(crate) struct Cursor<'a, T: Chunk> {
    root: &'a Node<T>,
    start: usize,
    end: usize,
    position: usize,
    path: [(Option<&'a Branch<T>>, usize); 8],
    path_len: usize,
}

impl<'a, T: Chunk> Cursor<'a, T> {
    pub(crate) fn is_at_front(&self) -> bool {
        self.position <= self.start
    }

    pub(crate) fn is_at_back(&self) -> bool {
        self.position + self.chunk().len() >= self.end
    }

    pub(crate) fn position(&self) -> usize {
        self.position.saturating_sub(self.start)
    }

    pub(crate) fn chunk(&self) -> &'a T {
        self.node().as_leaf().as_chunk()
    }

    pub(crate) fn range(&self) -> Range<usize> {
        Range {
            start: self.start.saturating_sub(self.position),
            end: self.chunk().len() - (self.position + self.chunk().len()).saturating_sub(self.end),
        }
    }

    pub(crate) fn move_next_chunk(&mut self) {
        self.position += self.chunk().len();
        while self.path_len > 0 {
            let (branch, index) = &mut self.path[self.path_len - 1];
            if *index < branch.unwrap().len() - 1 {
                *index += 1;
                break;
            }
            self.path_len -= 1;
        }
        self.descend_left();
    }

    pub(crate) fn move_prev_chunk(&mut self) {
        while self.path_len > 0 {
            let (branch, index) = &mut self.path[self.path_len - 1];
            if *index > 0 {
                *index -= 1;
                self.position -= branch.unwrap()[*index].summed_len();
                break;
            }
            self.path_len -= 1;
        }
        self.descend_right();
    }

    fn new(root: &'a Node<T>, start: usize, end: usize) -> Self {
        Self {
            root,
            start,
            end,
            position: 0,
            path: [(None, 0); 8],
            path_len: 0,
        }
    }

    fn node(&self) -> &'a Node<T> {
        if self.path_len == 0 {
            self.root
        } else {
            let (branch, index) = self.path[self.path_len - 1];
            &branch.unwrap()[index]
        }
    }

    fn descend_left(&mut self) {
        let mut node = self.node();
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    self.path[self.path_len] = (Some(branch), 0);
                    self.path_len += 1;
                    node = branch.first().unwrap();
                }
            }
        }
    }

    fn descend_right(&mut self) {
        let mut node = self.node();
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    node = branch.last().unwrap();
                    self.position += branch.summed_len() - node.summed_len();
                    self.path[self.path_len] = (Some(branch), branch.len() - 1);
                    self.path_len += 1;
                }
            }
        }
    }

    fn descend_to(&mut self, position: usize) {
        let mut node = self.node();
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    let (index, summed_len) = branch.search(position - self.position);
                    self.position += summed_len;
                    self.path[self.path_len] = (Some(branch), index);
                    self.path_len += 1;
                    node = &branch[index];
                }
            }
        }
        if self.position == self.end {
            self.move_prev_chunk()
        }
    }
}

pub(crate) trait Chunk: Clone {
    type Info: Info;

    const MAX_LEN: usize;

    fn new() -> Self;
    fn len(&self) -> usize;
    fn is_boundary(&self, index: usize) -> bool;
    fn info_at(&self, index: usize) -> Self::Info;
    fn merge(&self, start: usize, other: &Self, end: usize) -> Self;
    fn move_left(&mut self, other: &mut Self, end: usize);
    fn move_right(&mut self, other: &mut Self, start: usize);
    fn truncate_back(&mut self, start: usize);
    fn truncate_front(&mut self, end: usize);
}

pub(crate) trait Info:
    Copy + Add<Output = Self> + AddAssign + Sub<Output = Self> + SubAssign
{
    fn new() -> Self;
}

pub(crate) trait Measure<T: Chunk> {
    fn measure_chunk_at(chunk: &T, index: usize) -> usize;
    fn measure_info(info: T::Info) -> usize;
}

#[derive(Clone)]
enum Node<T: Chunk> {
    Leaf(Leaf<T>),
    Branch(Branch<T>),
}

impl<T: Chunk> Node<T> {
    fn into_leaf(self) -> Leaf<T> {
        match self {
            Self::Leaf(leaf) => leaf,
            _ => panic!(),
        }
    }

    fn into_branch(self) -> Branch<T> {
        match self {
            Self::Branch(branch) => branch,
            _ => panic!(),
        }
    }

    fn as_leaf(&self) -> &Leaf<T> {
        match self {
            Self::Leaf(leaf) => leaf,
            _ => panic!(),
        }
    }

    fn summed_len(&self) -> usize {
        match self {
            Self::Leaf(leaf) => leaf.len(),
            Self::Branch(branch) => branch.summed_len(),
        }
    }

    fn summed_info(&self) -> T::Info {
        match self {
            Self::Leaf(leaf) => leaf.info_at(leaf.len()),
            Self::Branch(branch) => branch.summed_info(),
        }
    }

    fn info_at(&self, position: usize) -> T::Info {
        let mut node = self;
        let mut summed_len = 0;
        let mut summed_info = T::Info::new();
        loop {
            match node {
                Node::Leaf(leaf) => break summed_info + leaf.info_at(position - summed_len),
                Node::Branch(branch) => {
                    let (index, len, info) = branch.search_with_info(position - summed_len);
                    node = &branch[index];
                    summed_len += len;
                    summed_info += info;
                }
            }
        }
    }

    fn measure_at<M: Measure<T>>(&self, position: usize) -> usize {
        let mut node = self;
        let mut summed_len = 0;
        let mut summed_measure = 0;
        loop {
            match node {
                Node::Leaf(leaf) => {
                    break summed_measure + M::measure_chunk_at(leaf, position - summed_len)
                }
                Node::Branch(branch) => {
                    let (index, len, measure) =
                        branch.search_with_measure::<M>(position - summed_len);
                    node = &branch[index];
                    summed_len += len;
                    summed_measure += measure;
                }
            }
        }
    }

    fn as_mut_branch(&mut self) -> &mut Branch<T> {
        match self {
            Self::Branch(branch) => branch,
            _ => panic!(),
        }
    }

    fn split_off(&mut self, at: usize) -> Self {
        match self {
            Self::Leaf(leaf) => Node::Leaf(leaf.split_off(at)),
            Self::Branch(branch) => {
                let (index, summed_len) = branch.search(at);
                if at == summed_len {
                    return Node::Branch(branch.split_off(index));
                }
                let mut other_branch = branch.split_off(index + 1);
                let mut node = branch.pop_back().unwrap();
                let mut other_node = node.split_off(at - summed_len);
                if branch.is_empty() {
                    branch.push_back(node)
                } else {
                    let count = node.pull_up_singular_nodes();
                    self.append_at_depth(node, count + 1);
                }
                if other_branch.is_empty() {
                    other_branch.push_front(other_node);
                    Node::Branch(other_branch)
                } else {
                    let count = other_node.pull_up_singular_nodes();
                    let mut other = Node::Branch(other_branch);
                    other.prepend_at_depth(other_node, count + 1);
                    other
                }
            }
        }
    }

    fn truncate_front(&mut self, end: usize) {
        match self {
            Self::Leaf(leaf) => leaf.truncate_front(end),
            Self::Branch(branch) => {
                let (index, summed_len) = branch.search(end);
                if end == summed_len {
                    branch.truncate_front(index);
                } else {
                    branch.truncate_front(index);
                    let mut node = branch.pop_front().unwrap();
                    node.truncate_front(end - summed_len);
                    if branch.is_empty() {
                        branch.push_front(node);
                    } else {
                        let count = node.pull_up_singular_nodes();
                        self.prepend_at_depth(node, count + 1);
                    }
                }
            }
        }
    }

    fn truncate_back(&mut self, start: usize) {
        match self {
            Self::Leaf(leaf) => leaf.truncate_back(start),
            Self::Branch(branch) => {
                let (index, summed_len) = branch.search(start);
                if start == summed_len {
                    branch.truncate_back(index);
                } else {
                    branch.truncate_back(index + 1);
                    let mut node = branch.pop_back().unwrap();
                    node.truncate_back(start - summed_len);
                    if branch.is_empty() {
                        branch.push_back(node);
                    } else {
                        let count = node.pull_up_singular_nodes();
                        self.append_at_depth(node, count + 1);
                    }
                }
            }
        }
    }

    fn prepend_at_depth(&mut self, other: Node<T>, depth: usize) -> Option<Self> {
        if depth == 0 {
            match self {
                Self::Leaf(leaf) => leaf
                    .prepend_or_distribute(other.into_leaf())
                    .map(|other_leaf| Node::Leaf(other_leaf)),
                Self::Branch(branch) => branch
                    .prepend_or_distribute(other.into_branch())
                    .map(|other_branch| Node::Branch(other_branch)),
            }
        } else {
            let branch = self.as_mut_branch();
            let mut node = branch.pop_front().unwrap();
            let other_node = node.prepend_at_depth(other, depth - 1);
            branch.push_front(node);
            other_node.and_then(|other_node| {
                branch
                    .push_front_and_maybe_split(other_node)
                    .map(|other_branch| Node::Branch(other_branch))
            })
        }
    }

    fn append_at_depth(&mut self, other: Node<T>, depth: usize) -> Option<Self> {
        if depth == 0 {
            match self {
                Self::Leaf(leaf) => leaf
                    .append_or_distribute(other.into_leaf())
                    .map(|other_leaf| Node::Leaf(other_leaf)),
                Self::Branch(branch) => branch
                    .append_or_distribute(other.into_branch())
                    .map(|other_branch| Node::Branch(other_branch)),
            }
        } else {
            let branch = self.as_mut_branch();
            let mut node = branch.pop_back().unwrap();
            let other_node = node.append_at_depth(other, depth - 1);
            branch.push_back(node);
            other_node.and_then(|other_node| {
                branch
                    .push_back_and_maybe_split(other_node)
                    .map(|other_branch| Node::Branch(other_branch))
            })
        }
    }

    fn pull_up_singular_nodes(&mut self) -> usize {
        let mut count = 0;
        loop {
            match self {
                Node::Branch(branch) if branch.len() == 1 => {
                    *self = branch.pop_back().unwrap();
                    count += 1;
                }
                _ => break,
            }
        }
        count
    }
}

impl<T: Chunk + fmt::Debug> fmt::Debug for Node<T>
where
    T::Info: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Leaf(leaf) => write!(f, "Node::Leaf({:?})", leaf),
            Self::Branch(branch) => write!(f, "Node::Branch({:?})", branch),
        }
    }
}

#[derive(Clone, Debug)]
struct Leaf<T> {
    chunk: Arc<T>,
}

impl<T: Chunk> Leaf<T> {
    const MAX_LEN: usize = T::MAX_LEN;

    fn new() -> Self {
        Self::from_chunk(Arc::new(Chunk::new()))
    }

    fn from_chunk(chunk: Arc<T>) -> Self {
        Self { chunk }
    }

    fn as_chunk(&self) -> &T {
        &self.chunk
    }

    fn prepend_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            other.move_right(self, 0);
            return None;
        }
        other.distribute(self);
        Some(other)
    }

    fn append_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            let other_len = other.len();
            self.move_left(&mut other, other_len);
            return None;
        }
        self.distribute(&mut other);
        Some(other)
    }

    fn distribute(&mut self, other: &mut Self) {
        use std::cmp::Ordering;

        match self.len().cmp(&other.len()) {
            Ordering::Less => {
                let mut end = (other.len() - self.len()) / 2;
                while !other.is_boundary(end) {
                    end -= 1;
                }
                self.move_left(other, end);
            }
            Ordering::Greater => {
                let mut start = (self.len() + other.len()) / 2;
                while !self.is_boundary(start) {
                    start += 1;
                }
                self.move_right(other, start);
            }
            _ => {}
        }
    }

    fn split_off(&mut self, at: usize) -> Self {
        let mut other = Self::new();
        self.move_right(&mut other, at);
        other
    }

    fn move_left(&mut self, other: &mut Self, end: usize) {
        Arc::make_mut(&mut self.chunk).move_left(Arc::make_mut(&mut other.chunk), end);
    }

    fn move_right(&mut self, other: &mut Self, start: usize) {
        Arc::make_mut(&mut self.chunk).move_right(Arc::make_mut(&mut other.chunk), start);
    }

    fn truncate_front(&mut self, end: usize) {
        Arc::make_mut(&mut self.chunk).truncate_front(end);
    }

    fn truncate_back(&mut self, start: usize) {
        Arc::make_mut(&mut self.chunk).truncate_back(start);
    }
}

impl<T: Chunk> Deref for Leaf<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.as_chunk()
    }
}

#[derive(Clone)]
struct Branch<T: Chunk> {
    summed_len: usize,
    summed_info: T::Info,
    nodes: Arc<Vec<Node<T>>>,
}

impl<T: Chunk> Branch<T> {
    const MAX_LEN: usize = 2;

    fn new() -> Self {
        Self::from_nodes(Arc::new(Vec::new()))
    }

    fn from_nodes(nodes: Arc<Vec<Node<T>>>) -> Self {
        Self {
            summed_len: nodes.sum_lens(),
            summed_info: nodes.sum_infos(),
            nodes,
        }
    }

    fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    fn summed_len(&self) -> usize {
        self.summed_len
    }

    fn summed_info(&self) -> T::Info {
        self.summed_info
    }

    fn len(&self) -> usize {
        self.nodes.len()
    }

    fn as_nodes(&self) -> &[Node<T>] {
        &self.nodes
    }

    fn push_front_and_maybe_split(&mut self, node: Node<T>) -> Option<Self> {
        if self.len() < Self::MAX_LEN {
            self.push_front(node);
            return None;
        }
        let mut other = Self::new();
        other.move_left(self, self.len() / 2);
        other.push_front(node);
        Some(other)
    }

    fn push_front(&mut self, node: Node<T>) {
        assert!(self.len() < Self::MAX_LEN);
        self.summed_len += node.summed_len();
        self.summed_info += node.summed_info();
        Arc::make_mut(&mut self.nodes).insert(0, node);
    }

    fn push_back_and_maybe_split(&mut self, node: Node<T>) -> Option<Self> {
        if self.len() < Self::MAX_LEN {
            self.push_back(node);
            return None;
        }
        let mut other = Self::new();
        self.move_right(&mut other, self.len() / 2);
        other.push_back(node);
        Some(other)
    }

    fn push_back(&mut self, node: Node<T>) {
        assert!(self.len() < Self::MAX_LEN);
        self.summed_len += node.summed_len();
        self.summed_info += node.summed_info();
        Arc::make_mut(&mut self.nodes).push(node);
    }

    fn pop_front(&mut self) -> Option<Node<T>> {
        if self.is_empty() {
            return None;
        }
        let node = Arc::make_mut(&mut self.nodes).remove(0);
        self.summed_len -= node.summed_len();
        self.summed_info -= node.summed_info();
        Some(node)
    }

    fn pop_back(&mut self) -> Option<Node<T>> {
        if self.is_empty() {
            return None;
        }
        let node = Arc::make_mut(&mut self.nodes).pop().unwrap();
        self.summed_len -= node.summed_len();
        self.summed_info -= node.summed_info();
        Some(node)
    }

    fn prepend_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            other.move_right(self, 0);
            return None;
        }
        other.distribute(self);
        Some(other)
    }

    fn append_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            let other_len = other.len();
            self.move_left(&mut other, other_len);
            return None;
        }
        self.distribute(&mut other);
        Some(other)
    }

    fn distribute(&mut self, other: &mut Self) {
        use std::cmp::Ordering;

        match self.len().cmp(&other.len()) {
            Ordering::Less => self.move_left(other, (other.len() - self.len()) / 2),
            Ordering::Greater => self.move_right(other, (self.len() + other.len()) / 2),
            _ => {}
        }
    }

    fn split_off(&mut self, at: usize) -> Self {
        let mut other = Self::new();
        self.move_right(&mut other, at);
        other
    }

    fn move_left(&mut self, other: &mut Self, end: usize) {
        let summed_len = other[..end].sum_lens();
        let summed_info = other[..end].sum_infos();
        other.summed_len -= summed_len;
        other.summed_info -= summed_info;
        self.summed_len += summed_len;
        self.summed_info += summed_info;
        let nodes = Arc::make_mut(&mut other.nodes).drain(..end);
        Arc::make_mut(&mut self.nodes).extend(nodes);
    }

    fn move_right(&mut self, other: &mut Self, start: usize) {
        let len = self[start..].sum_lens();
        let info = self[start..].sum_infos();
        self.summed_len -= len;
        self.summed_info -= info;
        other.summed_len += len;
        other.summed_info += info;
        let nodes = Arc::make_mut(&mut self.nodes).drain(start..);
        Arc::make_mut(&mut other.nodes).splice(..0, nodes);
    }

    fn truncate_front(&mut self, end: usize) {
        Arc::make_mut(&mut self.nodes).drain(..end);
    }

    fn truncate_back(&mut self, start: usize) {
        Arc::make_mut(&mut self.nodes).truncate(start);
    }
}

impl<T: Chunk + fmt::Debug> fmt::Debug for Branch<T>
where
    T::Info: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Branch")
            .field("summed_len", &self.summed_len)
            .field("summed_info", &self.summed_info)
            .field("nodes", &self.nodes)
            .finish()
    }
}

impl<T: Chunk> Deref for Branch<T> {
    type Target = [Node<T>];

    fn deref(&self) -> &Self::Target {
        self.as_nodes()
    }
}

impl<T: Chunk, I: SliceIndex<[Node<T>]>> Index<I> for Branch<T> {
    type Output = I::Output;

    fn index(&self, index: I) -> &Self::Output {
        &self.as_nodes()[index]
    }
}

trait NodeSliceExt<T: Chunk> {
    fn sum_lens(&self) -> usize;
    fn sum_infos(&self) -> T::Info;
    fn search(&self, position: usize) -> (usize, usize);
    fn search_with_info(&self, position: usize) -> (usize, usize, T::Info);
    fn search_with_measure<M: Measure<T>>(&self, position: usize) -> (usize, usize, usize);
}

impl<T: Chunk> NodeSliceExt<T> for [Node<T>] {
    fn sum_lens(&self) -> usize {
        let mut summed_len = 0;
        for node in self {
            summed_len += node.summed_len();
        }
        summed_len
    }

    fn sum_infos(&self) -> T::Info {
        let mut summed_info = T::Info::new();
        for node in self {
            summed_info += node.summed_info();
        }
        summed_info
    }

    fn search(&self, position: usize) -> (usize, usize) {
        let mut index = 0;
        let mut summed_len = 0;
        for node in self {
            let new_summed_len = summed_len + node.summed_len();
            if position < new_summed_len {
                break;
            }
            index += 1;
            summed_len = new_summed_len;
        }
        (index, summed_len)
    }

    fn search_with_info(&self, position: usize) -> (usize, usize, T::Info) {
        let mut index = 0;
        let mut summed_len = 0;
        let mut summed_info = Info::new();
        for node in self {
            let new_summed_len = summed_len + node.summed_len();
            if position < new_summed_len {
                break;
            }
            index += 1;
            summed_len = new_summed_len;
            summed_info += node.summed_info();
        }
        (index, summed_len, summed_info)
    }

    fn search_with_measure<M: Measure<T>>(&self, position: usize) -> (usize, usize, usize) {
        let mut index = 0;
        let mut summed_len = 0;
        let mut summed_measure = 0;
        for node in self {
            let new_summed_len = summed_len + node.summed_len();
            if position < new_summed_len {
                break;
            }
            index += 1;
            summed_len = new_summed_len;
            summed_measure += M::measure_info(node.summed_info());
        }
        (index, summed_len, summed_measure)
    }
}
