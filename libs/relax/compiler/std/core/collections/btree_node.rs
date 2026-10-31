//! Byte-layout B-tree nodes: port of real alloc's btree node, fix, remove and
//! navigate algorithms (MIT/Apache-2.0). B = 6; keys and values occupy separate
//! inline arrays. Only internal allocations contain edges. Shape operations are
//! non-generic so Relax does not compile the balancing machinery for every K/V.
//!
//! A live node owns exactly `len` keys/values and (if internal) `len + 1`
//! children. Byte moves transfer ownership; stale bytes outside len are never
//! read or dropped. No comparison or user callback runs during a shape mutation.

use crate::heap::{self, Layout};
use crate::{mem, ptr};

pub(crate) const CAPACITY: usize = 11;
const MIN_LEN: usize = 5;

#[repr(C)]
pub(crate) struct Node {
    parent: *mut Node,
    parent_idx: usize,
    pub len: usize,
    pub leaf: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cursor {
    pub node: *mut Node,
    pub idx: usize,
}
pub(crate) const NIL: Cursor = Cursor { node: 0 as *mut Node, idx: 0 };

#[derive(Clone, Copy)]
pub(crate) struct NodeLayout {
    keys: usize,
    vals: usize,
    edges: usize,
    key_size: usize,
    val_size: usize,
    leaf: Layout,
    internal: Layout,
}

impl NodeLayout {
    #[inline]
    pub fn new(ks: usize, ka: usize, vs: usize, va: usize) -> NodeLayout {
        let k = Layout::from_size_align(ks.checked_mul(CAPACITY).expect("capacity overflow"), ka).expect("capacity overflow");
        let v = Layout::from_size_align(vs.checked_mul(CAPACITY).expect("capacity overflow"), va).expect("capacity overflow");
        let (l, keys) = Layout::new::<Node>().extend(k).expect("capacity overflow");
        let (l, vals) = l.extend(v).expect("capacity overflow");
        let (internal, edges) = l.extend(Layout::array::<*mut Node>(CAPACITY + 1).expect("capacity overflow")).expect("capacity overflow");
        NodeLayout { keys, vals, edges, key_size: ks, val_size: vs, leaf: l.pad_to_align(), internal: internal.pad_to_align() }
    }

    #[inline]
    pub unsafe fn key(&self, c: Cursor) -> *mut u8 {
        (c.node as *mut u8).add(self.keys + c.idx * self.key_size)
    }
    #[inline]
    pub unsafe fn val(&self, c: Cursor) -> *mut u8 {
        (c.node as *mut u8).add(self.vals + c.idx * self.val_size)
    }
    #[inline]
    unsafe fn edges(&self, n: *mut Node) -> *mut *mut Node {
        (n as *mut u8).add(self.edges) as *mut *mut Node
    }
    #[inline]
    pub unsafe fn child(&self, n: *mut Node, i: usize) -> *mut Node {
        *self.edges(n).add(i)
    }
    unsafe fn set_child(&self, n: *mut Node, i: usize, child: *mut Node) {
        *self.edges(n).add(i) = child;
        (*child).parent = n;
        (*child).parent_idx = i;
    }
    unsafe fn fix_children(&self, n: *mut Node) {
        if !(*n).leaf {
            let mut i = 0;
            while i <= (*n).len {
                self.set_child(n, i, self.child(n, i));
                i += 1;
            }
        }
    }
    unsafe fn alloc(&self, leaf: bool) -> *mut Node {
        let layout = if leaf { self.leaf } else { self.internal };
        let n = heap::alloc(layout) as *mut Node;
        if n.is_null() { heap::handle_alloc_error(layout); }
        ptr::write(n, Node { parent: ptr::null_mut(), parent_idx: 0, len: 0, leaf });
        n
    }
    unsafe fn free(&self, n: *mut Node) {
        heap::dealloc(n as *mut u8, if (*n).leaf { self.leaf } else { self.internal });
    }
    unsafe fn move_kv(&self, src: Cursor, dst: Cursor, count: usize) {
        ptr::copy(self.key(src), self.key(dst), count * self.key_size);
        ptr::copy(self.val(src), self.val(dst), count * self.val_size);
    }
    unsafe fn insert_fit(&self, at: Cursor, key: *const u8, val: *const u8, right: *mut Node) {
        let n = at.node;
        self.move_kv(at, Cursor { node: n, idx: at.idx + 1 }, (*n).len - at.idx);
        ptr::copy(key, self.key(at), self.key_size);
        ptr::copy(val, self.val(at), self.val_size);
        if !(*n).leaf {
            ptr::copy(self.edges(n).add(at.idx + 1), self.edges(n).add(at.idx + 2), (*n).len - at.idx);
            *self.edges(n).add(at.idx + 1) = right;
        }
        (*n).len += 1;
        self.fix_children(n);
    }
    unsafe fn erase(&self, at: Cursor) {
        let n = at.node;
        self.move_kv(Cursor { node: n, idx: at.idx + 1 }, at, (*n).len - at.idx - 1);
        if !(*n).leaf {
            ptr::copy(self.edges(n).add(at.idx + 2), self.edges(n).add(at.idx + 1), (*n).len - at.idx - 1);
        }
        (*n).len -= 1;
        self.fix_children(n);
    }
}

/// The optional root uses the null niche, keeping root and height to two words.
pub(crate) struct Tree {
    root: Option<ptr::NonNull<Node>>,
    /// Height of the root; zero for a leaf or an unallocated tree.
    height: usize,
}

impl Tree {
    pub const fn new() -> Tree { Tree { root: None, height: 0 } }
    #[inline]
    pub fn root(&self) -> *mut Node {
        match self.root {
            Some(n) => n.as_ptr(),
            None => ptr::null_mut(),
        }
    }
    pub fn first(&self, l: &NodeLayout) -> Cursor {
        if self.root().is_null() { return NIL; }
        unsafe {
            let mut n = self.root();
            while !(*n).leaf { n = l.child(n, 0); }
            if (*n).len == 0 { NIL } else { Cursor { node: n, idx: 0 } }
        }
    }
    pub fn last(&self, l: &NodeLayout) -> Cursor {
        if self.root().is_null() { return NIL; }
        unsafe {
            let mut n = self.root();
            while !(*n).leaf { n = l.child(n, (*n).len); }
            if (*n).len == 0 { NIL } else { Cursor { node: n, idx: (*n).len - 1 } }
        }
    }
    /// The KV immediately right of a leaf edge, ascending when at the end.
    pub fn next_kv(&self, edge: Cursor) -> Cursor {
        let mut c = edge;
        unsafe {
            loop {
                if c.idx < (*c.node).len { return c; }
                let p = (*c.node).parent;
                if p.is_null() { return NIL; }
                c = Cursor { node: p, idx: (*c.node).parent_idx };
            }
        }
    }
    pub fn prev_kv(&self, edge: Cursor) -> Cursor {
        let mut c = edge;
        unsafe {
            loop {
                if c.idx > 0 { return Cursor { node: c.node, idx: c.idx - 1 }; }
                let p = (*c.node).parent;
                if p.is_null() { return NIL; }
                c = Cursor { node: p, idx: (*c.node).parent_idx };
            }
        }
    }
    pub fn next(&self, c: Cursor, l: &NodeLayout) -> Cursor {
        unsafe {
            let mut edge = Cursor { node: c.node, idx: c.idx + 1 };
            while !(*edge.node).leaf { edge = Cursor { node: l.child(edge.node, edge.idx), idx: 0 }; }
            self.next_kv(edge)
        }
    }
    pub fn prev(&self, c: Cursor, l: &NodeLayout) -> Cursor {
        unsafe {
            let mut edge = c;
            while !(*edge.node).leaf {
                let n = l.child(edge.node, edge.idx);
                edge = Cursor { node: n, idx: (*n).len };
            }
            self.prev_kv(edge)
        }
    }
    /// Comparison has finished before entry here; the incoming bytes remain owned
    /// by the caller until this infallible (allocation failure aborts) move finishes.
    pub unsafe fn insert(&mut self, at: Cursor, key: *const u8, val: *const u8, l: &NodeLayout) -> Cursor {
        let mut at = at;
        if self.root.is_none() {
            let n = l.alloc(true);
            self.root = Some(ptr::NonNull::new_unchecked(n));
            at = Cursor { node: n, idx: 0 };
        }
        self.insert_rec(at, key, val, ptr::null_mut(), l)
    }
    unsafe fn insert_rec(&mut self, at: Cursor, key: *const u8, val: *const u8, right: *mut Node, l: &NodeLayout) -> Cursor {
        let n = at.node;
        if (*n).len < CAPACITY {
            l.insert_fit(at, key, val, right);
            return at;
        }
        // real alloc's splitpoint: 4/5/5/6 depending on the insertion edge.
        let middle = if at.idx < 5 { 4 } else if at.idx <= 6 { 5 } else { 6 };
        let r = l.alloc((*n).leaf);
        (*r).len = CAPACITY - middle - 1;
        l.move_kv(Cursor { node: n, idx: middle + 1 }, Cursor { node: r, idx: 0 }, (*r).len);
        if !(*n).leaf {
            ptr::copy(l.edges(n).add(middle + 1), l.edges(r), (*r).len + 1);
            l.fix_children(r);
        }
        (*n).len = middle;
        let mut parent = (*n).parent;
        if parent.is_null() {
            parent = l.alloc(false);
            l.set_child(parent, 0, n);
            self.root = Some(ptr::NonNull::new_unchecked(parent));
            self.height += 1;
        }
        // Promote before inserting into the left half, which may overwrite the
        // median's former bytes. Parent splits do not move child elements.
        let median = Cursor { node: n, idx: middle };
        self.insert_rec(Cursor { node: parent, idx: (*n).parent_idx }, l.key(median), l.val(median), r, l);
        let dst = if at.idx <= middle { at } else { Cursor { node: r, idx: at.idx - middle - 1 } };
        l.insert_fit(dst, key, val, right);
        dst
    }

    /// Caller has read the removed pair. Return its successor after fixing all
    /// ancestors, tracking the leaf edge through steals and merges.
    pub unsafe fn remove(&mut self, at: Cursor, l: &NodeLayout) -> Cursor {
        let internal = !(*at.node).leaf;
        let mut edge = at;
        if internal {
            let pred = self.prev(at, l);
            l.move_kv(pred, at, 1);
            edge = pred;
        }
        l.erase(edge);
        let mut n = edge.node;
        while n != self.root() && (*n).len < MIN_LEN {
            let p = (*n).parent;
            let idx = (*n).parent_idx;
            let sep = if idx > 0 { idx - 1 } else { 0 };
            let left = l.child(p, sep);
            let right = l.child(p, sep + 1);
            let ll = (*left).len;
            let rl = (*right).len;
            if ll + 1 + rl <= CAPACITY {
                l.move_kv(Cursor { node: p, idx: sep }, Cursor { node: left, idx: ll }, 1);
                l.move_kv(Cursor { node: right, idx: 0 }, Cursor { node: left, idx: ll + 1 }, rl);
                if !(*left).leaf { ptr::copy(l.edges(right), l.edges(left).add(ll + 1), rl + 1); }
                (*left).len = ll + 1 + rl;
                l.fix_children(left);
                if edge.node == right { edge = Cursor { node: left, idx: ll + 1 + edge.idx }; }
                l.erase(Cursor { node: p, idx: sep });
                l.free(right);
                n = p;
            } else if idx > 0 {
                // Steal the left sibling's final KV through the separator.
                l.move_kv(Cursor { node: right, idx: 0 }, Cursor { node: right, idx: 1 }, rl);
                l.move_kv(Cursor { node: p, idx: sep }, Cursor { node: right, idx: 0 }, 1);
                l.move_kv(Cursor { node: left, idx: ll - 1 }, Cursor { node: p, idx: sep }, 1);
                if !(*right).leaf {
                    ptr::copy(l.edges(right), l.edges(right).add(1), rl + 1);
                    *l.edges(right) = l.child(left, ll);
                }
                (*left).len -= 1;
                (*right).len += 1;
                l.fix_children(right);
                if edge.node == right { edge.idx += 1; }
                break;
            } else {
                // Steal the right sibling's first KV through the separator.
                l.move_kv(Cursor { node: p, idx: sep }, Cursor { node: left, idx: ll }, 1);
                l.move_kv(Cursor { node: right, idx: 0 }, Cursor { node: p, idx: sep }, 1);
                l.move_kv(Cursor { node: right, idx: 1 }, Cursor { node: right, idx: 0 }, rl - 1);
                if !(*left).leaf {
                    *l.edges(left).add(ll + 1) = l.child(right, 0);
                    ptr::copy(l.edges(right).add(1), l.edges(right), rl);
                }
                (*left).len += 1;
                (*right).len -= 1;
                l.fix_children(left);
                l.fix_children(right);
                break;
            }
        }
        if (*self.root()).len == 0 && !(*self.root()).leaf {
            let old = self.root();
            let n = l.child(old, 0);
            self.root = Some(ptr::NonNull::new_unchecked(n));
            self.height -= 1;
            (*n).parent = ptr::null_mut();
            (*n).parent_idx = 0;
            l.free(old);
        }
        let next = self.next_kv(edge);
        // The predecessor was moved into the removed internal KV's position.
        if internal { self.next(next, l) } else { next }
    }

    /// Free allocations after the generic owner has dropped the live elements.
    pub unsafe fn free_nodes(&mut self, l: &NodeLayout) {
        if !self.root().is_null() {
            self.free_subtree(self.root(), l);
            self.root = None;
            self.height = 0;
        }
    }
    unsafe fn free_subtree(&self, n: *mut Node, l: &NodeLayout) {
        if !(*n).leaf {
            let mut i = 0;
            while i <= (*n).len { self.free_subtree(l.child(n, i), l); i += 1; }
        }
        l.free(n);
    }
}
