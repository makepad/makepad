use std::{
    ops::{Add, AddAssign, Deref, Range, RangeBounds, Sub, SubAssign},
    sync::Arc,
};

#[derive(Clone)]
pub(crate) struct BTree<T, I> {
    height: usize,
    root: Node<T, I>,
}

impl<T: Chunk, I: Info<T>> BTree<T, I> {
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
        self.root.total_len()
    }

    pub(crate) fn info(&self) -> I {
        self.root.total_info()
    }

    pub(crate) fn index_to_info(&self, index: usize) -> I {
        if index == 0 {
            return I::default();
        }
        match self.search_by(|total_len, _| index < total_len) {
            Some((chunk, total_len, total_info)) => {
                total_info + I::from_chunk(chunk, 0..index - total_len)
            }
            None => self.info(),
        }
    }

    pub(crate) fn search_by<P>(&self, mut predicate: P) -> Option<(&T, usize, I)>
    where
        P: FnMut(usize, I) -> bool,
    {
        if !predicate(self.len(), self.info()) {
            return None;
        }
        Some(self.root.search_by(predicate))
    }

    pub(crate) fn slice<R: RangeBounds<usize>>(&self, range: R) -> Slice<'_, T, I> {
        let range = self::range(range, self.len());
        Slice {
            btree: self,
            start: range.start,
            start_info: self.index_to_info(range.start),
            end: range.end,
            end_info: self.index_to_info(range.end),
        }
    }

    pub(crate) fn append(&mut self, mut other: Self) {
        if self.is_empty() {
            *self = other;
            return;
        }
        if other.is_empty() {
            return;
        }
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

    pub(crate) fn truncate_front(&mut self, start: usize) {
        if start == 0 {
            return;
        }
        if start == self.len() {
            *self = Self::new();
            return;
        }
        self.root.truncate_front(start);
        self.height -= self.root.pull_up_singular_nodes();
    }

    pub(crate) fn truncate_back(&mut self, end: usize) {
        if end == 0 {
            *self = Self::new();
            return;
        }
        if end == self.len() {
            return;
        }
        self.root.truncate_back(end);
        self.height -= self.root.pull_up_singular_nodes();
    }
}

pub(crate) struct Builder<T, I> {
    stack: Vec<(usize, Vec<Node<T, I>>)>,
}

impl<T: Chunk, I: Info<T>> Builder<T, I> {
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
            if nodes.len() < Branch::<T, I>::MAX_LEN {
                break;
            }
            let (_, nodes) = self.stack.pop().unwrap();
            height += 1;
            node = Node::Branch(Branch::from_nodes(Arc::new(nodes)));
        }
    }

    pub(crate) fn build(mut self) -> BTree<T, I> {
        let mut btree = BTree::new();
        while let Some((height, nodes)) = self.stack.pop() {
            for root in nodes.into_iter().rev() {
                let mut new_btree = BTree { height, root };
                new_btree.append(btree);
                btree = new_btree;
            }
        }
        btree
    }
}

pub(crate) struct Slice<'a, T, I> {
    btree: &'a BTree<T, I>,
    start: usize,
    start_info: I,
    end: usize,
    end_info: I,
}

impl<'a, T: Chunk, I: Info<T>> Slice<'a, T, I> {
    pub(crate) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(crate) fn len(&self) -> usize {
        self.end - self.start
    }

    pub(crate) fn info(&self) -> I {
        self.end_info - self.start_info
    }

    pub(crate) fn index_to_info(&self, index: usize) -> I {
        if index == 0 {
            return I::default();
        }
        match self.search_by(|total_len, _| index < total_len) {
            Some((chunk, range, total_len, total_info)) => {
                total_info + I::from_chunk(chunk, range.start..range.start + index - total_len)
            }
            None => self.info(),
        }
    }

    pub(crate) fn search_by<P>(&self, mut predicate: P) -> Option<(&T, Range<usize>, usize, I)>
    where
        P: FnMut(usize, I) -> bool,
    {
        self.btree
            .search_by(|total_len, total_info| {
                if total_len <= self.start {
                    return false;
                }
                predicate(total_len - self.start, total_info - self.start_info)
            })
            .map(|(chunk, total_len, total_info)| {
                let start = self.start.saturating_sub(total_len);
                let end = chunk.len() - (total_len + chunk.len()).saturating_sub(self.end);
                (
                    chunk,
                    start..end,
                    total_len + start - self.start,
                    total_info + I::from_chunk(chunk, 0..start) - self.start_info,
                )
            })
    }

    pub(crate) fn cursor_front(self) -> Cursor<'a, T, I> {
        let mut cursor = Cursor::new(self);
        if self.start == 0 {
            cursor.descend_left();
        } else if self.start == self.btree.len() {
            cursor.descend_right();
        } else {
            cursor.descend_to(self.start);
        }
        cursor
    }

    pub(crate) fn cursor_back(self) -> Cursor<'a, T, I> {
        let mut cursor = Cursor::new(self);
        if self.end == 0 {
            cursor.descend_left();
        } else if self.end == self.btree.len() {
            cursor.descend_right();
        } else {
            cursor.descend_to(self.end);
        }
        cursor
    }
}

impl<'a, T, I: Clone> Clone for Slice<'a, T, I> {
    fn clone(&self) -> Self {
        Self {
            btree: self.btree,
            start: self.start,
            start_info: self.start_info.clone(),
            end: self.end,
            end_info: self.end_info.clone(),
        }
    }
}

impl<'a, T, I: Copy> Copy for Slice<'a, T, I> {}

#[derive(Clone)]
pub(crate) struct Cursor<'a, T, I> {
    slice: Slice<'a, T, I>,
    position: usize,
    path: Vec<(&'a Branch<T, I>, usize)>,
}

impl<'a, T: Chunk, I: Info<T>> Cursor<'a, T, I> {
    pub(crate) fn is_at_front(&self) -> bool {
        self.position <= self.slice.start
    }

    pub(crate) fn is_at_back(&self) -> bool {
        self.position + self.current_chunk().len() >= self.slice.end
    }

    pub(crate) fn position(&self) -> usize {
        self.position.saturating_sub(self.slice.start)
    }

    pub(crate) fn current(&self) -> (&'a T, Range<usize>) {
        let chunk = self.current_chunk();
        let start = self.slice.start.saturating_sub(self.position);
        let end = chunk.len() - (self.position + chunk.len()).saturating_sub(self.slice.end);
        (chunk, start..end)
    }

    pub(crate) fn move_next(&mut self) {
        self.position += self.current_chunk().len();
        while let Some((branch, index)) = self.path.last_mut() {
            if *index < branch.len() - 1 {
                *index += 1;
                break;
            }
            self.path.pop();
        }
        self.descend_left();
    }

    pub(crate) fn move_prev(&mut self) {
        while let Some((branch, index)) = self.path.last_mut() {
            if *index > 0 {
                *index -= 1;
                self.position -= branch[*index].total_len();
                break;
            }
            self.path.pop();
        }
        self.descend_right();
    }

    fn new(slice: Slice<'a, T, I>) -> Self {
        Self {
            slice,
            position: 0,
            path: Vec::new(),
        }
    }

    fn current_node(&self) -> &'a Node<T, I> {
        self.path
            .last()
            .map_or(&self.slice.btree.root, |&(branch, index)| &branch[index])
    }

    fn current_chunk(&self) -> &'a T {
        self.current_node().as_leaf().as_chunk()
    }

    fn descend_left(&mut self) {
        let mut node = self.current_node();
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    self.path.push((branch, 0));
                    node = branch.first().unwrap();
                }
            }
        }
    }

    fn descend_right(&mut self) {
        let mut node = self.current_node();
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    self.position += branch.total_len() - branch.last().unwrap().total_len();
                    self.path.push((branch, branch.len() - 1));
                    node = branch.last().unwrap();
                }
            }
        }
    }

    fn descend_to(&mut self, mut position: usize) {
        if position == self.slice.end && !self.slice.is_empty() {
            position -= 1;
        }
        let mut node = self.current_node();
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    let index = branch.search(&mut self.position, position);
                    self.path.push((branch, index));
                    node = &branch[index];
                }
            }
        }
    }
}

pub trait Chunk: Clone + Default {
    const MAX_LEN: usize;

    fn len(&self) -> usize;
    fn is_boundary(&self, index: usize) -> bool;
    fn shift_left(&mut self, other: &mut Self, end: usize);
    fn shift_right(&mut self, other: &mut Self, start: usize);
    fn truncate_front(&mut self, start: usize);
    fn truncate_back(&mut self, end: usize);
}

pub trait Info<T>:
    Add<Output = Self> + AddAssign + Copy + Default + Sub<Output = Self> + SubAssign
{
    fn from_chunk(chunk: &T, range: Range<usize>) -> Self;
}

#[derive(Clone)]
enum Node<T, I> {
    Leaf(Leaf<T>),
    Branch(Branch<T, I>),
}

impl<T: Chunk, I: Info<T>> Node<T, I> {
    fn into_leaf(self) -> Leaf<T> {
        match self {
            Self::Leaf(leaf) => leaf,
            _ => panic!(),
        }
    }

    fn into_branch(self) -> Branch<T, I> {
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

    fn total_len(&self) -> usize {
        match self {
            Self::Leaf(leaf) => leaf.len(),
            Self::Branch(branch) => branch.total_len(),
        }
    }

    fn total_info(&self) -> I {
        match self {
            Self::Leaf(leaf) => I::from_chunk(leaf, 0..leaf.len()),
            Self::Branch(branch) => branch.total_info(),
        }
    }

    fn search_by<P>(&self, mut predicate: P) -> (&T, usize, I)
    where
        P: FnMut(usize, I) -> bool,
    {
        let mut node = self;
        let mut total_len = 0;
        let mut total_info = I::default();
        loop {
            match node {
                Node::Leaf(leaf) => break (leaf.as_chunk(), total_len, total_info),
                Node::Branch(branch) => {
                    node =
                        &branch[branch.search_by(&mut total_len, &mut total_info, &mut predicate)]
                }
            }
        }
    }

    fn as_mut_branch(&mut self) -> &mut Branch<T, I> {
        match self {
            Self::Branch(branch) => branch,
            _ => panic!(),
        }
    }

    fn prepend_at_depth(&mut self, other: Node<T, I>, depth: usize) -> Option<Self> {
        if depth == 0 {
            self.prepend_or_distribute(other)
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

    fn prepend_or_distribute(&mut self, other: Self) -> Option<Self> {
        match self {
            Self::Leaf(leaf) => leaf
                .prepend_or_distribute(other.into_leaf())
                .map(|other_leaf| Node::Leaf(other_leaf)),
            Self::Branch(branch) => branch
                .prepend_or_distribute(other.into_branch())
                .map(|other_branch| Node::Branch(other_branch)),
        }
    }

    fn append_at_depth(&mut self, other: Node<T, I>, depth: usize) -> Option<Self> {
        if depth == 0 {
            self.append_or_distribute(other)
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

    fn append_or_distribute(&mut self, other: Self) -> Option<Self> {
        match self {
            Self::Leaf(leaf) => leaf
                .append_or_distribute(other.into_leaf())
                .map(|other_leaf| Node::Leaf(other_leaf)),
            Self::Branch(branch) => branch
                .append_or_distribute(other.into_branch())
                .map(|other_branch| Node::Branch(other_branch)),
        }
    }

    fn split_off(&mut self, at: usize) -> Self {
        match self {
            Self::Leaf(leaf) => Node::Leaf(leaf.split_off(at)),
            Self::Branch(branch) => {
                let mut total_len = 0;
                let index = branch.search(&mut total_len, at);
                if at == total_len {
                    return Node::Branch(branch.split_off(index));
                }
                let mut other_branch = branch.split_off(index + 1);
                let mut node = branch.pop_back().unwrap();
                let mut other_node = node.split_off(at - total_len);
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

    fn truncate_front(&mut self, start: usize) {
        match self {
            Self::Leaf(leaf) => leaf.truncate_front(start),
            Self::Branch(branch) => {
                let mut total_len = 0;
                let index = branch.search(&mut total_len, start);
                if start == total_len {
                    branch.truncate_front(index);
                } else {
                    branch.truncate_front(index);
                    let mut node = branch.pop_front().unwrap();
                    node.truncate_front(start - total_len);
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

    fn truncate_back(&mut self, end: usize) {
        match self {
            Self::Leaf(leaf) => leaf.truncate_back(end),
            Self::Branch(branch) => {
                let mut total_len = 0;
                let index = branch.search(&mut total_len, end);
                if end == total_len {
                    branch.truncate_back(index);
                } else {
                    branch.truncate_back(index + 1);
                    let mut node = branch.pop_back().unwrap();
                    node.truncate_back(end - total_len);
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

#[derive(Clone)]
struct Leaf<T> {
    chunk: Arc<T>,
}

impl<T: Chunk> Leaf<T> {
    const MAX_LEN: usize = T::MAX_LEN;

    fn new() -> Self {
        Self::from_chunk(Arc::new(T::default()))
    }

    fn from_chunk(chunk: Arc<T>) -> Self {
        Self { chunk }
    }

    fn as_chunk(&self) -> &T {
        &self.chunk
    }

    fn prepend_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            self.prepend(other);
            None
        } else {
            other.distribute(self);
            Some(other)
        }
    }

    fn prepend(&mut self, mut other: Self) {
        other.shift_right(self, 0);
    }

    fn append_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            self.append(other);
            None
        } else {
            self.distribute(&mut other);
            Some(other)
        }
    }

    fn append(&mut self, mut other: Self) {
        let other_len = other.len();
        self.shift_left(&mut other, other_len);
    }

    fn split_off(&mut self, at: usize) -> Self {
        let mut other = Self::new();
        self.shift_right(&mut other, at);
        other
    }

    fn distribute(&mut self, other: &mut Self) {
        use std::cmp::Ordering;

        match self.len().cmp(&other.len()) {
            Ordering::Less => {
                let mut end = (other.len() - self.len()) / 2;
                while !other.is_boundary(end) {
                    end -= 1;
                }
                self.shift_left(other, end);
            }
            Ordering::Greater => {
                let mut start = (self.len() + other.len()) / 2;
                while !self.is_boundary(start) {
                    start += 1;
                }
                self.shift_right(other, start);
            }
            _ => {}
        }
    }

    fn shift_left(&mut self, other: &mut Self, end: usize) {
        Arc::make_mut(&mut self.chunk).shift_left(Arc::make_mut(&mut other.chunk), end);
    }

    fn shift_right(&mut self, other: &mut Self, start: usize) {
        Arc::make_mut(&mut self.chunk).shift_right(Arc::make_mut(&mut other.chunk), start);
    }

    fn truncate_front(&mut self, start: usize) {
        Arc::make_mut(&mut self.chunk).truncate_front(start);
    }

    fn truncate_back(&mut self, end: usize) {
        Arc::make_mut(&mut self.chunk).truncate_back(end);
    }
}

impl<T> Deref for Leaf<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.chunk
    }
}

#[derive(Clone)]
struct Branch<T, I> {
    total_len: usize,
    total_info: I,
    nodes: Arc<Vec<Node<T, I>>>,
}

impl<T: Chunk, I: Info<T>> Branch<T, I> {
    const MAX_LEN: usize = 2;

    fn new() -> Self {
        Self::from_nodes(Arc::new(Vec::new()))
    }

    fn from_nodes(nodes: Arc<Vec<Node<T, I>>>) -> Self {
        Self {
            total_len: nodes.compute_total_len(),
            total_info: nodes.compute_total_info(),
            nodes,
        }
    }

    fn total_len(&self) -> usize {
        self.total_len
    }

    fn total_info(&self) -> I {
        self.total_info
    }

    fn push_front_and_maybe_split(&mut self, node: Node<T, I>) -> Option<Self> {
        use std::mem;

        if self.len() < Self::MAX_LEN {
            self.push_front(node);
            None
        } else {
            let mut other = self.split_off(self.len() / 2);
            mem::swap(self, &mut other);
            other.push_front(node);
            Some(other)
        }
    }

    fn push_front(&mut self, node: Node<T, I>) {
        self.total_len += node.total_len();
        self.total_info += node.total_info();
        Arc::make_mut(&mut self.nodes).insert(0, node);
    }

    fn push_back_and_maybe_split(&mut self, node: Node<T, I>) -> Option<Self> {
        if self.len() < Self::MAX_LEN {
            self.push_back(node);
            None
        } else {
            let mut other = self.split_off(self.len() / 2);
            other.push_back(node);
            Some(other)
        }
    }

    fn push_back(&mut self, node: Node<T, I>) {
        self.total_len += node.total_len();
        self.total_info += node.total_info();
        Arc::make_mut(&mut self.nodes).push(node);
    }

    fn pop_front(&mut self) -> Option<Node<T, I>> {
        if self.is_empty() {
            return None;
        }
        let node = Arc::make_mut(&mut self.nodes).remove(0);
        self.total_len -= node.total_len();
        self.total_info -= node.total_info();
        Some(node)
    }

    fn pop_back(&mut self) -> Option<Node<T, I>> {
        if self.is_empty() {
            return None;
        }
        let node = Arc::make_mut(&mut self.nodes).pop().unwrap();
        self.total_len -= node.total_len();
        self.total_info -= node.total_info();
        Some(node)
    }

    fn prepend_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            self.prepend(other);
            None
        } else {
            other.distribute(self);
            Some(other)
        }
    }

    fn prepend(&mut self, mut other: Self) {
        other.shift_right(self, 0);
    }

    fn append_or_distribute(&mut self, mut other: Self) -> Option<Self> {
        if self.len() + other.len() <= Self::MAX_LEN {
            self.append(other);
            None
        } else {
            self.distribute(&mut other);
            Some(other)
        }
    }

    fn append(&mut self, mut other: Self) {
        let other_len = other.len();
        self.shift_left(&mut other, other_len);
    }

    fn split_off(&mut self, at: usize) -> Self {
        let mut other = Self::new();
        self.shift_right(&mut other, at);
        other
    }

    fn distribute(&mut self, other: &mut Self) {
        use std::cmp::Ordering;

        match self.len().cmp(&other.len()) {
            Ordering::Less => self.shift_left(other, (other.len() - self.len()) / 2),
            Ordering::Greater => self.shift_right(other, (self.len() + other.len()) / 2),
            _ => {}
        }
    }

    fn shift_left(&mut self, other: &mut Self, end: usize) {
        let total_len = other[..end].compute_total_len();
        let total_info = other[..end].compute_total_info();
        other.total_len -= total_len;
        other.total_info -= total_info;
        self.total_len += total_len;
        self.total_info += total_info;
        let nodes = Arc::make_mut(&mut other.nodes).drain(..end);
        Arc::make_mut(&mut self.nodes).extend(nodes);
    }

    fn shift_right(&mut self, other: &mut Self, start: usize) {
        let total_len = self[start..].compute_total_len();
        let total_info = self[start..].compute_total_info();
        self.total_len -= total_len;
        self.total_info -= total_info;
        other.total_len += total_len;
        other.total_info += total_info;
        let nodes = Arc::make_mut(&mut self.nodes).drain(start..);
        Arc::make_mut(&mut other.nodes).splice(..0, nodes);
    }

    fn truncate_front(&mut self, start: usize) {
        Arc::make_mut(&mut self.nodes).drain(..start);
    }

    fn truncate_back(&mut self, end: usize) {
        Arc::make_mut(&mut self.nodes).truncate(end);
    }
}

impl<T, I> Deref for Branch<T, I> {
    type Target = [Node<T, I>];

    fn deref(&self) -> &Self::Target {
        &self.nodes
    }
}

trait NodeSliceExt<T, I> {
    fn compute_total_len(&self) -> usize;

    fn compute_total_info(&self) -> I;

    fn search(&self, total_len: &mut usize, index: usize) -> usize;

    fn search_by<P>(&self, total_len: &mut usize, total_info: &mut I, predicate: P) -> usize
    where
        P: FnMut(usize, I) -> bool;
}

impl<T: Chunk, I: Info<T>> NodeSliceExt<T, I> for [Node<T, I>] {
    fn compute_total_len(&self) -> usize {
        self.iter()
            .map(|node| node.total_len())
            .fold(0, |a, b| a + b)
    }

    fn compute_total_info(&self) -> I {
        self.iter()
            .map(|node| node.total_info())
            .fold(I::default(), |a, b| a + b)
    }

    fn search(&self, total_len: &mut usize, position: usize) -> usize {
        self.iter()
            .enumerate()
            .find_map(|(index, node)| {
                let new_total_len = *total_len + node.total_len();
                if position < new_total_len {
                    return Some(index);
                }
                *total_len = new_total_len;
                None
            })
            .unwrap()
    }

    fn search_by<P>(&self, total_len: &mut usize, total_info: &mut I, mut predicate: P) -> usize
    where
        P: FnMut(usize, I) -> bool,
    {
        self.iter()
            .enumerate()
            .find_map(|(index, node)| {
                let new_total_len = *total_len + node.total_len();
                let new_total_info = *total_info + node.total_info();
                if predicate(new_total_len, new_total_info) {
                    return Some(index);
                }
                *total_len = new_total_len;
                *total_info = new_total_info;
                None
            })
            .unwrap()
    }
}

fn range<R: RangeBounds<usize>>(range: R, len: usize) -> Range<usize> {
    use std::ops::Bound;

    let start = match range.start_bound() {
        Bound::Excluded(&start) => start.checked_add(1).unwrap(),
        Bound::Included(&start) => start,
        Bound::Unbounded => 0,
    };
    let end = match range.end_bound() {
        Bound::Excluded(&end) => end,
        Bound::Included(&end) => end.checked_add(1).unwrap(),
        Bound::Unbounded => len,
    };
    assert!(start <= end);
    assert!(end <= len);
    start..end
}
