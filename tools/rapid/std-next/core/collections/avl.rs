//! Tree shape of BTreeMap: an AVL tree over node indices with parent links. Keys and values
//! live in a separate array indexed the same way, so everything here is non-generic (R8);
//! BTreeMap does the key comparisons and moves elements.

use crate::vec::Vec;

pub(crate) const NIL: usize = usize::MAX;

#[derive(Clone, Copy)]
pub(crate) struct Link {
    pub left: usize,
    pub right: usize,
    pub parent: usize,
    pub height: u32,
}

pub(crate) struct Tree {
    pub links: Vec<Link>,
    pub root: usize,
    /// freed indices, reused by `alloc`
    pub free: Vec<usize>,
}

impl Tree {
    pub const fn new() -> Tree {
        Tree { links: Vec::new(), root: NIL, free: Vec::new() }
    }

    #[inline]
    fn height(&self, n: usize) -> u32 {
        if n == NIL {
            0
        } else {
            self.links[n].height
        }
    }

    fn update(&mut self, n: usize) {
        let hl = self.height(self.links[n].left);
        let hr = self.height(self.links[n].right);
        self.links[n].height = 1 + if hl > hr { hl } else { hr };
    }

    fn balance(&self, n: usize) -> i64 {
        self.height(self.links[n].left) as i64 - self.height(self.links[n].right) as i64
    }

    /// Points `parent`'s link that referred to `old` at `new` (or the root).
    fn replace_child(&mut self, parent: usize, old: usize, new: usize) {
        if parent == NIL {
            self.root = new;
        } else if self.links[parent].left == old {
            self.links[parent].left = new;
        } else {
            self.links[parent].right = new;
        }
        if new != NIL {
            self.links[new].parent = parent;
        }
    }

    fn rotate_left(&mut self, x: usize) -> usize {
        let y = self.links[x].right;
        let b = self.links[y].left;
        let p = self.links[x].parent;
        self.links[x].right = b;
        if b != NIL {
            self.links[b].parent = x;
        }
        self.links[y].left = x;
        self.links[x].parent = y;
        self.replace_child(p, x, y);
        self.update(x);
        self.update(y);
        y
    }

    fn rotate_right(&mut self, x: usize) -> usize {
        let y = self.links[x].left;
        let b = self.links[y].right;
        let p = self.links[x].parent;
        self.links[x].left = b;
        if b != NIL {
            self.links[b].parent = x;
        }
        self.links[y].right = x;
        self.links[x].parent = y;
        self.replace_child(p, x, y);
        self.update(x);
        self.update(y);
        y
    }

    /// Restores heights and balance from `n` up to the root.
    pub fn rebalance_from(&mut self, n: usize) {
        let mut cur = n;
        while cur != NIL {
            self.update(cur);
            let bf = self.balance(cur);
            let mut top = cur;
            if bf > 1 {
                let l = self.links[cur].left;
                if self.balance(l) < 0 {
                    self.rotate_left(l);
                }
                top = self.rotate_right(cur);
            } else if bf < -1 {
                let r = self.links[cur].right;
                if self.balance(r) > 0 {
                    self.rotate_right(r);
                }
                top = self.rotate_left(cur);
            }
            cur = self.links[top].parent;
        }
    }

    /// A fresh unlinked node index (the caller stores its element at that index).
    pub fn alloc(&mut self) -> usize {
        let link = Link { left: NIL, right: NIL, parent: NIL, height: 1 };
        match self.free.pop() {
            Some(i) => {
                self.links[i] = link;
                i
            }
            None => {
                self.links.push(link);
                self.links.len() - 1
            }
        }
    }

    /// Links new node `n` as the `left`/right child of `parent` (NIL = becomes the root).
    pub fn attach(&mut self, n: usize, parent: usize, left: bool) {
        self.links[n].parent = parent;
        if parent == NIL {
            self.root = n;
        } else if left {
            self.links[parent].left = n;
        } else {
            self.links[parent].right = n;
        }
        self.rebalance_from(parent);
    }

    pub fn first(&self) -> usize {
        if self.root == NIL {
            NIL
        } else {
            self.leftmost(self.root)
        }
    }

    pub fn last(&self) -> usize {
        if self.root == NIL {
            NIL
        } else {
            self.rightmost(self.root)
        }
    }

    fn leftmost(&self, n: usize) -> usize {
        let mut n = n;
        while self.links[n].left != NIL {
            n = self.links[n].left;
        }
        n
    }

    fn rightmost(&self, n: usize) -> usize {
        let mut n = n;
        while self.links[n].right != NIL {
            n = self.links[n].right;
        }
        n
    }

    pub fn next(&self, n: usize) -> usize {
        if self.links[n].right != NIL {
            return self.leftmost(self.links[n].right);
        }
        let mut c = n;
        let mut p = self.links[n].parent;
        while p != NIL && self.links[p].right == c {
            c = p;
            p = self.links[p].parent;
        }
        p
    }

    pub fn prev(&self, n: usize) -> usize {
        if self.links[n].left != NIL {
            return self.rightmost(self.links[n].left);
        }
        let mut c = n;
        let mut p = self.links[n].parent;
        while p != NIL && self.links[p].left == c {
            c = p;
            p = self.links[p].parent;
        }
        p
    }

    /// Unlinks the node at `n` from the tree shape. Returns the index whose slot becomes free:
    /// `n` itself, or (when `n` has two children) its successor `s`, which takes `n`'s place
    /// in the shape -- the caller must then swap the elements stored at `n` and `s` so that
    /// `n`'s slot holds the successor's element and `s`'s slot the removed one.
    pub fn unlink(&mut self, n: usize) -> usize {
        let l = self.links[n].left;
        let r = self.links[n].right;
        if l != NIL && r != NIL {
            let s = self.leftmost(r);
            // remove s (it has no left child) from its spot
            let s_parent = self.links[s].parent;
            let s_right = self.links[s].right;
            self.replace_child(s_parent, s, s_right);
            let fix_from = if s_parent == n { n } else { s_parent };
            self.rebalance_from(fix_from);
            self.free.push(s);
            return s;
        }
        let child = if l != NIL { l } else { r };
        let p = self.links[n].parent;
        self.replace_child(p, n, child);
        self.rebalance_from(p);
        self.free.push(n);
        n
    }

    pub fn clear(&mut self) {
        self.links.clear();
        self.free.clear();
        self.root = NIL;
    }

    /// Live node indices in order (used for drop and clone).
    pub fn in_order(&self) -> Vec<usize> {
        let mut out = Vec::new();
        let mut n = self.first();
        while n != NIL {
            out.push(n);
            n = self.next(n);
        }
        out
    }
}
