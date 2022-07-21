use std::{
    fmt,
    ops::{AddAssign, Deref, Index, Range, SubAssign},
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

    pub(crate) fn len(&self) -> usize {
        self.root.summed_len()
    }

    pub(crate) fn info(&self) -> T::Info {
        self.root.summed_info()
    }

    pub(crate) fn cursor_front(&self) -> Cursor<'_, T> {
        let mut path = Vec::new();
        let mut node = &self.root;
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    path.push((branch, 0));
                    node = branch.first().unwrap();
                }
            }
        }
        Cursor {
            root: &self.root,
            start: 0,
            end: self.len(),
            position: 0,
            path,
        }
    }

    pub(crate) fn cursor_back(&self) -> Cursor<'_, T> {
        let mut path = Vec::new();
        let mut node = &self.root;
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    path.push((branch, 0));
                    node = branch.first().unwrap();
                }
            }
        };
        Cursor {
            root: &self.root,
            start: 0,
            end: self.len(),
            position: self.len(),
            path,
        }
    }

    pub(crate) fn prepend(&mut self, mut other: Self) {
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

    pub(crate) fn append(&mut self, mut other: Self) {
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
                btree.prepend(other);
            }
        }
        btree
    }
}

pub(crate) struct Cursor<'a, T: Chunk> {
    root: &'a Node<T>,
    start: usize,
    end: usize,
    position: usize,
    path: Vec<(&'a Branch<T>, usize)>,
}

impl<'a, T: Chunk> Cursor<'a, T> {
    pub(crate) fn is_at_start(&self) -> bool {
        self.position <= self.start
    }

    pub(crate) fn is_at_end(&self) -> bool {
        self.position >= self.end
    }

    pub(crate) fn position(&self) -> usize {
        self.position
    }

    pub(crate) fn chunk(&self) -> &'a T {
        self.path
            .last()
            .map_or(self.root, |(branch, index)| &branch[*index])
            .as_leaf()
            .as_chunk()
    }

    pub(crate) fn range(&self) -> Range<usize> {
        Range {
            start: self.start.saturating_sub(self.position),
            end: self.chunk().len() - self.position.saturating_sub(self.end),
        }
    }

    pub(crate) fn move_next_chunk(&mut self) {
        self.position += self.chunk().len();
        while let Some((branch, index)) = self.path.last_mut() {
            if *index < branch.len() - 1 {
                *index += 1;
                break;
            }
            self.path.pop();
        }
        let mut node = self
            .path
            .last()
            .map_or(self.root, |(branch, index)| &branch[*index]);
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

    pub(crate) fn move_prev_chunk(&mut self) {
        while let Some((_, index)) = self.path.last_mut() {
            if *index > 0 {
                *index -= 1;
                break;
            }
            self.path.pop();
        }
        let mut node = self
            .path
            .last()
            .map_or(self.root, |(branch, index)| &branch[*index]);
        loop {
            match node {
                Node::Leaf(_) => break,
                Node::Branch(branch) => {
                    self.path.push((branch, branch.len() - 1));
                    node = branch.last().unwrap();
                }
            }
        }
        self.position -= self.chunk().len();
    }
}

pub(crate) trait Chunk: Clone {
    type Info: Info;

    const MAX_LEN: usize;

    fn new() -> Self;
    fn len(&self) -> usize;
    fn info(&self) -> Self::Info;
    fn can_split_at(&self, index: usize) -> bool;
    fn move_left(&mut self, other: &mut Self, end: usize);
    fn move_right(&mut self, other: &mut Self, start: usize);
    fn truncate_back(&mut self, start: usize);
    fn truncate_front(&mut self, end: usize);
}

pub(crate) trait Info: Copy + AddAssign + SubAssign {
    fn new() -> Self;
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
            Self::Leaf(leaf) => leaf.info(),
            Self::Branch(branch) => branch.summed_info(),
        }
    }

    fn as_mut_branch(&mut self) -> &mut Branch<T> {
        match self {
            Self::Branch(branch) => branch,
            _ => panic!(),
        }
    }

    fn split_off(&mut self, index: usize) -> Self {
        match self {
            Self::Leaf(leaf) => Node::Leaf(leaf.split_off(index)),
            Self::Branch(branch) => {
                let (index, summed_len) = search_by_index(branch, index);
                if index == summed_len {
                    return Node::Branch(branch.split_off(index));
                }
                let mut other_branch = branch.split_off(index + 1);
                let mut node = branch.pop_back().unwrap();
                let mut other_node = node.split_off(index - summed_len);
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
                let (index, summed_len) = search_by_index(branch, end);
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
                let (index, summed_len) = search_by_index(branch, start);
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
                while !other.can_split_at(end) {
                    end -= 1;
                }
                self.move_left(other, end);
            }
            Ordering::Greater => {
                let mut start = (self.len() + other.len()) / 2;
                while !self.can_split_at(start) {
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
            summed_len: sum_lens(&nodes),
            summed_info: sum_infos(&nodes),
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
            Ordering::Less => self.move_left(other, (self.len() - other.len()) / 2),
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
        let summed_len = sum_lens(&other[..end]);
        let summed_info = sum_infos(&other[..end]);
        other.summed_len -= summed_len;
        other.summed_info -= summed_info;
        self.summed_len += summed_len;
        self.summed_info += summed_info;
        let nodes = Arc::make_mut(&mut other.nodes).drain(..end);
        Arc::make_mut(&mut self.nodes).extend(nodes);
    }

    fn move_right(&mut self, other: &mut Self, start: usize) {
        let len = sum_lens(&self[start..]);
        let info = sum_infos(&self[start..]);
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

fn sum_lens<T: Chunk>(nodes: &[Node<T>]) -> usize {
    let mut summed_len = 0;
    for node in nodes {
        summed_len += node.summed_len();
    }
    summed_len
}

fn sum_infos<T: Chunk>(nodes: &[Node<T>]) -> T::Info {
    let mut summed_info = T::Info::new();
    for node in nodes {
        summed_info += node.summed_info();
    }
    summed_info
}

fn search_by_index<T: Chunk>(_nodes: &[Node<T>], _index: usize) -> (usize, usize) {
    unimplemented!() // TODO
}
