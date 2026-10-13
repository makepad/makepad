// ported from N:rustc_middle/src/dep_graph/graph.rs @8d1a764
use super::{QueryError, QueryId};
use crate::fxhash::{FxHasher, HashMap, HashSet};
use crate::sip::Sip128;
use std::hash::{Hash, Hasher};
use std::cell::{Cell, RefCell};
use std::ops::{Index, IndexMut, Range};
use relax_session::sync::Mutex;

/// Stable encodings must use fixed widths, explicit endianness and length prefixes.
/// rustc_middle/src/dep_graph/dep_node.rs:91 separates key and result fingerprints.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Fingerprint(pub u128);

impl Fingerprint {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut hash = Sip128::new();
        hash.write(bytes);
        Self(hash.finish128())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DepKind(pub u16);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct DepNode {
    pub kind: DepKind,
    pub key_fingerprint: Fingerprint,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DepNodeIndex(pub u32);

impl Hash for DepNodeIndex {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Mix the page into the slot bits: otherwise key zero in every kind
        // lands in the same small FxHashMap bucket in the sparse job table.
        state.write_u32(self.0 ^ (self.0 >> PAGE.trailing_zeros()));
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DepNodeColor {
    Unknown,
    Red,
    Green,
}

// Fixed-capacity chunks never reallocate their elements. Only the small directory
// moves; a memo hit in QueryTable still bypasses the graph entirely.
const CHUNK: usize = 64;
// Slack before compact() sweeps stale fingerprints (independent of the chunk size).
const COMPACT_SLACK: usize = 1024;

pub(super) struct Chunks<T> {
    chunks: Vec<Vec<T>>,
    len: usize,
}

impl<T> Default for Chunks<T> {
    fn default() -> Self { Self { chunks: Vec::new(), len: 0 } }
}

impl<T> Chunks<T> {
    pub fn len(&self) -> usize { self.len }
    fn push(&mut self, value: T) {
        if self.len % CHUNK == 0 { self.chunks.push(Vec::with_capacity(CHUNK)); }
        self.chunks.last_mut().unwrap().push(value);
        self.len += 1;
    }
    fn bytes(&self) -> usize {
        self.chunks.capacity() * std::mem::size_of::<Vec<T>>()
            + self.chunks.iter().map(|c| c.capacity() * std::mem::size_of::<T>()).sum::<usize>()
    }
}

impl<T> Index<usize> for Chunks<T> {
    type Output = T;
    #[inline]
    fn index(&self, index: usize) -> &T { &self.chunks[index / CHUNK][index % CHUNK] }
}
impl<T> IndexMut<usize> for Chunks<T> {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut T { &mut self.chunks[index / CHUNK][index % CHUNK] }
}

fn hash(value: &impl Hash) -> u64 {
    let mut h = FxHasher::default();
    value.hash(&mut h);
    h.finish()
}

// Buckets contain only indices. The authoritative key is in the indexed store,
// so growing a table never copies identities or fingerprints.
#[derive(Default)]
struct IndexTable {
    buckets: Vec<u32>,
    len: usize,
}
const NONE: u32 = u32::MAX;

impl IndexTable {
    fn get(&self, hash: u64, equal: impl Fn(u32) -> bool) -> Option<u32> {
        if self.buckets.is_empty() { return None; }
        let mut slot = hash as usize & (self.buckets.len() - 1);
        loop {
            let index = self.buckets[slot];
            if index == NONE { return None; }
            if equal(index) { return Some(index); }
            slot = (slot + 1) & (self.buckets.len() - 1);
        }
    }
    fn insert(&mut self, index: u32, hash: u64, rehash: impl Fn(u32) -> u64) {
        if (self.len + 1) * 4 > self.buckets.len() * 3 {
            let size = (self.buckets.len() * 2).max(16);
            let old = std::mem::replace(&mut self.buckets, vec![NONE; size]);
            for index in old.into_iter().filter(|&i| i != NONE) { self.place(index, rehash(index)); }
        }
        self.place(index, hash);
        self.len += 1;
    }
    fn place(&mut self, index: u32, hash: u64) {
        let mut slot = hash as usize & (self.buckets.len() - 1);
        while self.buckets[slot] != NONE { slot = (slot + 1) & (self.buckets.len() - 1); }
        self.buckets[slot] = index;
    }
    fn bytes(&self) -> usize { self.buckets.capacity() * 4 }
}

#[derive(Default)]
struct Fingerprints {
    values: Chunks<Fingerprint>,
    index: IndexTable,
}
impl Fingerprints {
    fn intern(&mut self, value: Fingerprint) -> u32 {
        let h = hash(&value);
        if let Some(index) = self.index.get(h, |i| self.values[i as usize] == value) { return index; }
        let index = u32::try_from(self.values.len()).expect("query fingerprint index overflow");
        assert_ne!(index, NONE, "query fingerprint index overflow");
        self.values.push(value);
        self.index.insert(index, h, |i| hash(&self.values[i as usize]));
        index
    }
}

// Query keys include QueryTable's shard bits and are dense within a kind.
// A page supplies the kind, key prefix and input flag once for PAGE nodes (small: each compiled function's nounwind engine touches ~5 kinds, a 256-node page was 5 KB each, 280 MB gross on Calculator cold). The
// node index addresses that page/slot directly, so no reverse QueryId is stored.
const PAGE: usize = 32;
struct NodePage {
    query: Option<QueryId>,
    eval_always: bool,
    nodes: Box<[Node; PAGE]>,
}
#[derive(Default)]
pub(super) struct Nodes {
    pages: Vec<NodePage>,
    len: usize,
    anon_next: Option<DepNodeIndex>,
}
impl Nodes {
    pub fn len(&self) -> usize { self.len }
    fn page(&mut self, query: Option<QueryId>, eval_always: bool) -> u32 {
        let first = u32::try_from(self.pages.len() * PAGE).expect("query node limit");
        self.pages.push(NodePage { query, eval_always, nodes: Box::new([Node::EMPTY; PAGE]) });
        first
    }
    #[inline]
    fn query(&self, index: DepNodeIndex) -> Option<QueryId> {
        let base = self.pages[index.0 as usize / PAGE].query?;
        Some(QueryId { kind: base.kind, key: base.key + index.0 % PAGE as u32 })
    }
    #[inline]
    fn eval_always(&self, index: DepNodeIndex) -> bool { self.pages[index.0 as usize / PAGE].eval_always }
    fn bytes(&self) -> usize {
        self.pages.capacity() * std::mem::size_of::<NodePage>() + self.pages.len() * PAGE * std::mem::size_of::<Node>()
    }
    fn capacity(&self) -> usize { self.pages.len() * PAGE }
}
impl Index<usize> for Nodes {
    type Output = Node;
    #[inline]
    fn index(&self, index: usize) -> &Node { &self.pages[index / PAGE].nodes[index % PAGE] }
}
impl IndexMut<usize> for Nodes {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut Node { &mut self.pages[index / PAGE].nodes[index % PAGE] }
}

// The low two bits describe inline edges. Each revision has 15 inline bits;
// crossing that width spills the full u64 pair rather than limiting revisions.
const REVISION_WIDE: u32 = (1 << 15) - 1;

// 20 bytes. Zero/one/two reads stay in these same two edge words; longer
// lists use (start,len) in the packed edge array. Rare wide ranges spill.
#[derive(Clone, Copy)]
pub(super) struct Node {
    key_fingerprint: u32,
    fingerprint: u32,
    revisions: u32,
    edges: [u32; 2],
}
impl Node {
    const EMPTY: Self = Self { key_fingerprint: NONE, fingerprint: NONE, revisions: 0, edges: [0, 0] };
    #[inline]
    pub fn has_fingerprint(&self) -> bool { self.fingerprint != NONE }
}

// A validation walk snapshots its descriptor once, as it did with the old
// Range<usize>. Appending other nodes cannot change these immutable reads.
pub(super) enum ReadEdges {
    Inline([u32; 2], usize),
    Stored(Range<usize>),
}
impl ReadEdges {
    #[inline]
    pub fn len(&self) -> usize { match self { Self::Inline(_, len) => *len, Self::Stored(range) => range.len() } }
    #[inline]
    pub fn get(&self, graph: &DepGraph, offset: usize) -> u32 {
        match self { Self::Inline(reads, _) => reads[offset], Self::Stored(range) => graph.edges[range.start + offset] }
    }
}

/// Flat u32 edges, with one (start, len) per node. Nodes retain their last
/// verified revision: a lazy node can skip many edits.
#[derive(Default)]
pub(super) struct DepGraph {
    pub nodes: Nodes,
    pub edges: Vec<u32>,
    indices: HashMap<(u16, u32), u32>,
    identities: IndexTable,
    fingerprints: Fingerprints,
    live_edges: usize,
    last_edges: Range<usize>,
    anon_node_to_index: IndexTable,
    pub errors: HashMap<DepNodeIndex, (u64, QueryError)>,
    wide_revisions: HashMap<DepNodeIndex, [u64; 2]>,
    wide_edges: HashMap<DepNodeIndex, Range<usize>>,
}

impl DepGraph {
    pub fn find(&self, query: QueryId) -> Option<DepNodeIndex> {
        let &page = self.indices.get(&(query.kind, query.key / PAGE as u32))?;
        let index = DepNodeIndex(page + query.key % PAGE as u32);
        (self.nodes[index.0 as usize].key_fingerprint != NONE).then_some(index)
    }
    #[inline]
    pub fn query(&self, index: DepNodeIndex) -> Option<QueryId> { self.nodes.query(index) }
    #[inline]
    pub fn eval_always(&self, index: DepNodeIndex) -> bool { self.nodes.eval_always(index) }
    pub fn identity(&self, index: DepNodeIndex) -> DepNode {
        let n = &self.nodes[index.0 as usize];
        DepNode { kind: DepKind(self.query(index).map_or(0, |q| q.kind)), key_fingerprint: self.fingerprints.values[n.key_fingerprint as usize] }
    }
    #[inline]
    pub fn fingerprint(&self, index: DepNodeIndex) -> Option<Fingerprint> {
        let id = self.nodes[index.0 as usize].fingerprint;
        (id != NONE).then(|| self.fingerprints.values[id as usize])
    }
    #[inline]
    pub fn revision(&self, index: DepNodeIndex, changed: bool) -> u64 {
        let shift = if changed { 2 } else { 17 };
        let revision = (self.nodes[index.0 as usize].revisions >> shift) & REVISION_WIDE;
        if revision == REVISION_WIDE { self.wide_revisions[&index][changed as usize] } else { revision as u64 }
    }
    #[inline]
    pub fn set_revision(&mut self, index: DepNodeIndex, changed: bool, revision: u64) {
        let mut pair = [self.revision(index, false), self.revision(index, true)];
        pair[changed as usize] = revision;
        self.set_revisions(index, pair);
    }
    #[inline]
    fn set_revisions(&mut self, index: DepNodeIndex, pair: [u64; 2]) {
        if pair[0] >= REVISION_WIDE as u64 || pair[1] >= REVISION_WIDE as u64 {
            if self.wide_revisions.is_empty() { eprintln!("query graph: retaining full-width revisions after {}", pair[0]); }
            self.wide_revisions.insert(index, pair);
        }
        let node = &mut self.nodes[index.0 as usize];
        node.revisions = ((pair[0].min(REVISION_WIDE as u64) as u32) << 17)
            | ((pair[1].min(REVISION_WIDE as u64) as u32) << 2) | (node.revisions & 3);
    }
    fn stored_range(&self, index: DepNodeIndex) -> Range<usize> {
        let node = &self.nodes[index.0 as usize];
        if node.edges[1] == NONE { self.wide_edges[&index].clone() }
            else { node.edges[0] as usize..node.edges[0] as usize + node.edges[1] as usize }
    }
    pub fn read_edges(&self, index: DepNodeIndex) -> ReadEdges {
        let node = &self.nodes[index.0 as usize];
        let mode = node.revisions & 3;
        if mode < 3 { ReadEdges::Inline(node.edges, mode as usize) }
        else { ReadEdges::Stored(self.stored_range(index)) }
    }
    #[inline]
    pub fn reads(&self, index: DepNodeIndex) -> &[u32] {
        let node = &self.nodes[index.0 as usize];
        let mode = node.revisions & 3;
        if mode < 3 { return &node.edges[..mode as usize]; }
        &self.edges[self.stored_range(index)]
    }
    fn set_edges(&mut self, index: DepNodeIndex, range: Range<usize>) {
        let node = &mut self.nodes[index.0 as usize];
        node.revisions = (node.revisions & !3) | 3;
        if range.start > u32::MAX as usize || range.len() >= NONE as usize {
            node.edges[1] = NONE;
            self.wide_edges.insert(index, range);
        } else {
            node.edges = [range.start as u32, range.len() as u32];
            if !self.wide_edges.is_empty() { self.wide_edges.remove(&index); }
        }
    }
    fn push(&mut self, query: Option<QueryId>, key: Fingerprint, eval_always: bool) -> DepNodeIndex {
        let index = if let Some(query) = query {
            let page = *self.indices.entry((query.kind, query.key / PAGE as u32)).or_insert_with(|| {
                self.nodes.page(Some(QueryId { kind: query.kind, key: query.key / PAGE as u32 * PAGE as u32 }), eval_always)
            });
            DepNodeIndex(page + query.key % PAGE as u32)
        } else {
            let index = self.nodes.anon_next.take().unwrap_or_else(|| DepNodeIndex(self.nodes.page(None, false)));
            if index.0 as usize % PAGE + 1 < PAGE { self.nodes.anon_next = Some(DepNodeIndex(index.0 + 1)); }
            index
        };
        let key_fingerprint = self.fingerprints.intern(key);
        self.nodes[index.0 as usize] = Node { key_fingerprint, ..Node::EMPTY };
        self.nodes.len += 1;
        index
    }
    pub fn intern(&mut self, query: QueryId, key: Fingerprint, eval_always: bool) -> Result<DepNodeIndex, QueryError> {
        let identity = DepNode { kind: DepKind(query.kind), key_fingerprint: key };
        if let Some(index) = self.find(query) {
            if self.identity(index) != identity || self.eval_always(index) != eval_always {
                return Err(QueryError::IdentityConflict(query));
            }
            return Ok(index);
        }
        let h = hash(&identity);
        if self.identities.get(h, |i| self.identity(DepNodeIndex(i)) == identity).is_some() {
            return Err(QueryError::IdentityConflict(query));
        }
        let index = self.push(Some(query), key, eval_always);
        let nodes = &self.nodes;
        let fingerprints = &self.fingerprints;
        self.identities.insert(index.0, h, |i| {
            let n = &nodes[i as usize];
            hash(&DepNode { kind: DepKind(nodes.query(DepNodeIndex(i)).unwrap().kind), key_fingerprint: fingerprints.values[n.key_fingerprint as usize] })
        });
        Ok(index)
    }

    /// One read reuses that node; ordered multi-read sets are interned per revision.
    /// The index compares the packed edges themselves instead of keeping a second Vec.
    pub fn intern_anon(&mut self, revision: u64, reads: &[u32]) -> DepNodeIndex {
        if reads.len() == 1 { return DepNodeIndex(reads[0]); }
        let key_revision = if reads.is_empty() { 0 } else { revision };
        let mut h = Sip128::new();
        h.write(&key_revision.to_le_bytes());
        for read in reads { h.write(&read.to_le_bytes()); }
        let key = Fingerprint(h.finish128());
        let key_hash = hash(&key);
        if let Some(index) = self.anon_node_to_index.get(key_hash, |i| {
            let index = DepNodeIndex(i);
            self.identity(index).key_fingerprint == key
                && (reads.is_empty() || self.revision(index, true) == key_revision)
                && self.reads(index) == reads
        }) { return DepNodeIndex(index); }
        let index = self.push(None, key, false);
        self.complete(index, revision, Fingerprint(0), reads);
        self.anon_node_to_index.insert(index.0, key_hash, |i| {
            hash(&self.fingerprints.values[self.nodes[i as usize].key_fingerprint as usize])
        });
        index
    }

    pub fn set_fingerprint(&mut self, index: DepNodeIndex, revision: u64, fingerprint: Fingerprint) {
        if self.fingerprint(index) == Some(fingerprint) {
            self.set_revision(index, false, revision);
        } else {
            self.nodes[index.0 as usize].fingerprint = self.fingerprints.intern(fingerprint);
            self.set_revisions(index, [revision, revision]);
        }
    }

    pub fn complete(&mut self, index: DepNodeIndex, revision: u64, fingerprint: Fingerprint, reads: &[u32]) {
        self.set_fingerprint(index, revision, fingerprint);
        // Identical reads need no new range, even when the result changes.
        if self.reads(index) == reads { return; }
        if self.nodes[index.0 as usize].revisions & 3 == 3 { self.live_edges -= self.reads(index).len(); }
        if reads.len() <= 2 {
            let node = &mut self.nodes[index.0 as usize];
            node.edges[..reads.len()].copy_from_slice(reads);
            node.revisions = (node.revisions & !3) | reads.len() as u32;
            if !self.wide_edges.is_empty() { self.wide_edges.remove(&index); }
        } else if &self.edges[self.last_edges.clone()] == reads {
            // Adjacent queries often read the same declaration roots. Reuse
            // that immutable range without allocating or hashing another set.
            self.set_edges(index, self.last_edges.clone());
            self.live_edges += reads.len();
        } else {
            let start = self.edges.len();
            if reads.len() > self.edges.capacity() - self.edges.len() {
                // Geometric growth retains amortised linear append work, with
                // at most 12.5% unused edge capacity instead of almost 100%.
                let capacity = (self.edges.capacity() + self.edges.capacity() / 8).max(self.edges.len() + reads.len()).max(16);
                self.edges.reserve_exact(capacity - self.edges.len());
            }
            self.edges.extend_from_slice(reads);
            self.last_edges = start..self.edges.len();
            self.set_edges(index, self.last_edges.clone());
            self.live_edges += reads.len();
        }
    }

    // Called only between revisions; obsolete ranges cannot grow without bound.
    pub fn compact(&mut self) {
        // Old result fingerprints can become unreachable after edits. Rebuild
        // only after enough garbage accumulates to pay for a linear sweep.
        if self.fingerprints.values.len() > self.nodes.len().saturating_mul(2) + COMPACT_SLACK {
            let mut fingerprints = Fingerprints::default();
            for i in 0..self.nodes.capacity() {
                let node = &mut self.nodes[i];
                if node.key_fingerprint == NONE { continue; }
                node.key_fingerprint = fingerprints.intern(self.fingerprints.values[node.key_fingerprint as usize]);
                if node.fingerprint != NONE {
                    node.fingerprint = fingerprints.intern(self.fingerprints.values[node.fingerprint as usize]);
                }
            }
            self.fingerprints = fingerprints;
        }
        if self.edges.len() <= self.live_edges.saturating_mul(2) + 1024 { return; }
        let mut edges = Vec::with_capacity(self.live_edges);
        for i in 0..self.nodes.capacity() {
            let index = DepNodeIndex(i as u32);
            if self.nodes[i].key_fingerprint == NONE || self.nodes[i].revisions & 3 != 3 { continue; }
            let start = edges.len();
            edges.extend_from_slice(self.reads(index));
            self.set_edges(index, start..edges.len());
        }
        self.edges = edges;
        self.last_edges = 0..0;
    }

    pub fn mem_lines(&self) -> Vec<(String, u64)> {
        let mut lines = vec![
            (format!("query graph nodes ({}; Node {} B)", self.nodes.len(), std::mem::size_of::<Node>()), self.nodes.bytes() as u64),
            (format!("query graph edges ({}; live {})", self.edges.len(), self.live_edges), (self.edges.capacity() * 4) as u64),
            (format!("query graph indices ({})", self.indices.len()), map_bytes(&self.indices)),
            (format!("query graph identities ({})", self.identities.len), self.identities.bytes() as u64),
            (format!("query graph fingerprints ({})", self.fingerprints.values.len()), (self.fingerprints.values.bytes() + self.fingerprints.index.bytes()) as u64),
            (format!("query graph anon ({})", self.anon_node_to_index.len), self.anon_node_to_index.bytes() as u64),
        ];
        lines.push((format!("query graph errors ({})", self.errors.len()), map_bytes(&self.errors)));
        lines.push((format!("query graph wide revisions ({})", self.wide_revisions.len()), map_bytes(&self.wide_revisions)));
        lines.push(("query graph wide edges".into(), map_bytes(&self.wide_edges)));
        lines
    }
}

pub(super) fn map_bytes<K, V>(map: &HashMap<K, V>) -> u64 {
    if map.capacity() == 0 { return 0; }
    let buckets = (map.capacity() * 8 / 7).next_power_of_two();
    (buckets * (std::mem::size_of::<(K, V)>() + 1) + 16) as u64
}

/// First-read order and deduplication follow rustc_middle/src/dep_graph/graph.rs:450.
/// Small tasks keep their reads inline, including root requests during TLS teardown.
#[derive(Default)]
pub(super) struct TaskDeps {
    engine: u64,
    job: Option<DepNodeIndex>,
    recording: bool,
    inline: [u32; 16],
    len: usize,
    spill: TaskReads,
}

/// Large read buffers belong to the engine between requests, never to TLS.
#[derive(Default)]
pub(super) struct TaskReads {
    reads: Vec<u32>,
    read_set: HashSet<DepNodeIndex>,
}

impl TaskDeps {
    pub fn new(engine: u64, job: Option<DepNodeIndex>, recording: bool) -> RefCell<Self> {
        RefCell::new(Self { engine, job, recording, ..Self::default() })
    }

    fn reads(&self) -> &[u32] {
        if self.len <= self.inline.len() { &self.inline[..self.len] } else { &self.spill.reads }
    }

    fn read(&mut self, index: u32, spare: &Mutex<Vec<TaskReads>>) {
        if !self.recording { return; }
        if self.len < self.inline.len() {
            if self.inline[..self.len].contains(&index) { return; }
            self.inline[self.len] = index;
        } else {
            if self.len == self.inline.len() && self.spill.read_set.is_empty() {
                self.spill = spare.lock().unwrap_or_else(|e| e.into_inner()).pop().unwrap_or_default();
                self.spill.reads.extend_from_slice(&self.inline);
                for &read in &self.inline { self.spill.read_set.insert(DepNodeIndex(read)); }
            }
            if !self.spill.read_set.insert(DepNodeIndex(index)) { return; }
            self.spill.reads.push(index);
        }
        self.len += 1;
    }
}

// rustc_middle/src/ty/context/tls.rs:12,27,49,63: TLS borrows the scoped
// implicit context. This const Cell has no destructor, so a late TLS destructor
// can start a root request just like any other caller.
// It owns no heap state: quiescence ends the task scopes and restores null before a zone swap.
thread_local! {
    static CONTEXT: Cell<*const ()> = const { Cell::new(std::ptr::null()) };
}

fn with_context<R>(op: impl FnOnce(Option<&RefCell<TaskDeps>>) -> R) -> R {
    let ptr = CONTEXT.get();
    if ptr.is_null() { return op(None); }
    // SAFETY: TaskGuard borrows the stack frame, cannot outlive it, and restores
    // this thread's previous pointer before that frame is destroyed, even on unwind.
    op(Some(unsafe { &*ptr.cast::<RefCell<TaskDeps>>() }))
}

/// This thread's query context (null: none), to run work for this thread on another one.
pub fn context_ptr() -> *const () {
    CONTEXT.get()
}

/// Makes `ctx` (another thread's `context_ptr`) this thread's query context and returns the one
/// it replaces. SAFETY: the thread owning `ctx` is blocked until this thread swaps it out again,
/// so its task frame is used by one thread at a time.
pub unsafe fn swap_context(ctx: *const ()) -> *const () {
    CONTEXT.replace(ctx)
}

pub(super) fn in_context(engine: u64) -> bool {
    with_context(|frame| frame.is_some_and(|frame| frame.borrow().engine == engine))
}

pub(super) fn current_job(engine: u64) -> Option<DepNodeIndex> {
    with_context(|frame| {
        let task = frame?.borrow();
        assert_eq!(task.engine, engine, "queries cannot read another engine");
        task.job
    })
}

pub(super) fn read_index(engine: u64, index: DepNodeIndex, spare: &Mutex<Vec<TaskReads>>) {
    with_context(|frame| {
        if let Some(frame) = frame {
            let mut task = frame.borrow_mut();
            assert_eq!(task.engine, engine, "queries cannot read another engine");
            task.read(index.0, spare);
        }
    });
}

pub(super) struct TaskGuard<'a> {
    frame: &'a RefCell<TaskDeps>,
    previous: *const (),
    spare: &'a Mutex<Vec<TaskReads>>,
}

impl<'a> TaskGuard<'a> {
    pub fn enter(frame: &'a RefCell<TaskDeps>, spare: &'a Mutex<Vec<TaskReads>>) -> Self {
        let previous = CONTEXT.replace((frame as *const RefCell<TaskDeps>).cast());
        Self { frame, previous, spare }
    }

    pub fn record(&self) {
        self.frame.borrow_mut().recording = true;
    }

    pub fn with_reads(&self, op: &mut dyn FnMut(&[u32])) {
        op(self.frame.borrow().reads());
    }
}

impl Drop for TaskGuard<'_> {
    fn drop(&mut self) {
        CONTEXT.set(self.previous);
        let mut task = self.frame.borrow_mut();
        if !task.spill.reads.is_empty() {
            let mut spill = std::mem::take(&mut task.spill);
            spill.reads.clear();
            spill.read_set.clear();
            self.spare.lock().unwrap_or_else(|e| e.into_inner()).push(spill);
        }
    }
}
