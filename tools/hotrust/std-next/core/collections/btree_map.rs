//! BTreeMap: an ordered map. Implemented as an AVL tree over an index arena (avl.rs holds the
//! non-generic shape; elements live in a parallel array), not a B-tree: the same API,
//! ordering and O(log n) bounds as real std; node layout and capacity are not observable.

use crate::vec::Vec;
use super::avl::{Tree, NIL};
use crate::borrow::Borrow;
use crate::cmp::Ordering;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::iter::{FromIterator, FusedIterator};
use crate::marker::PhantomData;
use crate::mem::MaybeUninit;
use crate::ops::{Bound, Index, RangeBounds};

pub struct BTreeMap<K, V> {
    tree: Tree,
    kv: Vec<MaybeUninit<(K, V)>>,
    len: usize,
}

unsafe impl<K: Send, V: Send> Send for BTreeMap<K, V> {}
unsafe impl<K: Sync, V: Sync> Sync for BTreeMap<K, V> {}

impl<K, V> BTreeMap<K, V> {
    pub const fn new() -> BTreeMap<K, V> {
        BTreeMap { tree: Tree::new(), kv: Vec::new(), len: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.drop_all();
        self.tree.clear();
        self.kv.clear();
        self.len = 0;
    }

    fn drop_all(&mut self) {
        if crate::mem::needs_drop::<(K, V)>() {
            let order = self.tree.in_order();
            let mut i = 0;
            while i < order.len() {
                unsafe { self.kv[order[i]].assume_init_drop() };
                i += 1;
            }
        }
    }

    #[inline]
    fn kv_at(&self, n: usize) -> &(K, V) {
        unsafe { self.kv[n].assume_init_ref() }
    }

    #[inline]
    fn kv_at_mut(&mut self, n: usize) -> &mut (K, V) {
        unsafe { self.kv[n].assume_init_mut() }
    }

    /// Stores a new element at a fresh node index (not yet linked).
    fn new_node(&mut self, k: K, v: V) -> usize {
        let n = self.tree.alloc();
        if n == self.kv.len() {
            self.kv.push(MaybeUninit::new((k, v)));
        } else {
            self.kv[n] = MaybeUninit::new((k, v));
        }
        n
    }

    /// Unlinks node `n` and returns its element.
    fn remove_node(&mut self, n: usize) -> (K, V) {
        let freed = self.tree.unlink(n);
        if freed != n {
            self.kv.swap(n, freed);
        }
        self.len -= 1;
        unsafe { self.kv[freed].assume_init_read() }
    }

    pub fn first_key_value(&self) -> Option<(&K, &V)> {
        let n = self.tree.first();
        if n == NIL {
            None
        } else {
            let kv = self.kv_at(n);
            Some((&kv.0, &kv.1))
        }
    }

    pub fn last_key_value(&self) -> Option<(&K, &V)> {
        let n = self.tree.last();
        if n == NIL {
            None
        } else {
            let kv = self.kv_at(n);
            Some((&kv.0, &kv.1))
        }
    }

    pub fn pop_first(&mut self) -> Option<(K, V)> {
        let n = self.tree.first();
        if n == NIL {
            None
        } else {
            Some(self.remove_node(n))
        }
    }

    pub fn pop_last(&mut self) -> Option<(K, V)> {
        let n = self.tree.last();
        if n == NIL {
            None
        } else {
            Some(self.remove_node(n))
        }
    }

    pub fn first_entry(&mut self) -> Option<OccupiedEntry<'_, K, V>> {
        let n = self.tree.first();
        if n == NIL {
            None
        } else {
            Some(OccupiedEntry { map: self, node: n })
        }
    }

    pub fn last_entry(&mut self) -> Option<OccupiedEntry<'_, K, V>> {
        let n = self.tree.last();
        if n == NIL {
            None
        } else {
            Some(OccupiedEntry { map: self, node: n })
        }
    }

    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter { map: self, front: self.tree.first(), back: self.tree.last(), remaining: self.len }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        let front = self.tree.first();
        let back = self.tree.last();
        let remaining = self.len;
        IterMut { map: self as *mut BTreeMap<K, V>, front, back, remaining, _m: PhantomData }
    }

    pub fn keys(&self) -> Keys<'_, K, V> {
        Keys { inner: self.iter() }
    }

    pub fn values(&self) -> Values<'_, K, V> {
        Values { inner: self.iter() }
    }

    pub fn values_mut(&mut self) -> ValuesMut<'_, K, V> {
        ValuesMut { inner: self.iter_mut() }
    }

    pub fn into_keys(self) -> IntoKeys<K, V> {
        IntoKeys { inner: self.into_iter() }
    }

    pub fn into_values(self) -> IntoValues<K, V> {
        IntoValues { inner: self.into_iter() }
    }

    pub fn retain<F: FnMut(&K, &mut V) -> bool>(&mut self, f: F) {
        let mut f = f;
        let mut n = self.tree.first();
        while n != NIL {
            let next = self.tree.next(n);
            let keep = {
                let kv = self.kv_at_mut(n);
                f(&kv.0, &mut kv.1)
            };
            if !keep {
                // removal may move the successor's element into slot n
                let freed = self.tree.unlink(n);
                if freed != n {
                    self.kv.swap(n, freed);
                    let v = unsafe { self.kv[freed].assume_init_read() };
                    self.len -= 1;
                    drop(v);
                    // the successor now lives at n: continue there
                    continue;
                }
                let v = unsafe { self.kv[freed].assume_init_read() };
                self.len -= 1;
                drop(v);
            }
            n = next;
        }
    }

    pub fn extract_if<F: FnMut(&K, &mut V) -> bool>(&mut self, pred: F) -> ExtractIf<'_, K, V, F> {
        let first = self.tree.first();
        ExtractIf { map: self, cur: first, pred }
    }
}

impl<K: Ord, V> BTreeMap<K, V> {
    fn find<Q: ?Sized + Ord>(&self, key: &Q) -> usize
    where
        K: Borrow<Q>,
    {
        let mut n = self.tree.root;
        while n != NIL {
            match key.cmp(self.kv_at(n).0.borrow()) {
                Ordering::Less => n = self.tree.links[n].left,
                Ordering::Greater => n = self.tree.links[n].right,
                Ordering::Equal => return n,
            }
        }
        NIL
    }

    /// Where `key` is or would be attached: (node, NIL, _) if found, else (NIL, parent, left).
    fn search(&self, key: &K) -> (usize, usize, bool) {
        let mut n = self.tree.root;
        let mut parent = NIL;
        let mut left = false;
        while n != NIL {
            match key.cmp(&self.kv_at(n).0) {
                Ordering::Less => {
                    parent = n;
                    left = true;
                    n = self.tree.links[n].left;
                }
                Ordering::Greater => {
                    parent = n;
                    left = false;
                    n = self.tree.links[n].right;
                }
                Ordering::Equal => return (n, NIL, false),
            }
        }
        (NIL, parent, left)
    }

    /// First node with key in `bound` from below (NIL if none).
    fn lower_bound<Q: ?Sized + Ord>(&self, bound: Bound<&Q>) -> usize
    where
        K: Borrow<Q>,
    {
        let mut n = self.tree.root;
        let mut best = NIL;
        while n != NIL {
            let ok = match bound {
                Bound::Included(x) => self.kv_at(n).0.borrow() >= x,
                Bound::Excluded(x) => self.kv_at(n).0.borrow() > x,
                Bound::Unbounded => true,
            };
            if ok {
                best = n;
                n = self.tree.links[n].left;
            } else {
                n = self.tree.links[n].right;
            }
        }
        best
    }

    /// Last node with key in `bound` from above (NIL if none).
    fn upper_bound<Q: ?Sized + Ord>(&self, bound: Bound<&Q>) -> usize
    where
        K: Borrow<Q>,
    {
        let mut n = self.tree.root;
        let mut best = NIL;
        while n != NIL {
            let ok = match bound {
                Bound::Included(x) => self.kv_at(n).0.borrow() <= x,
                Bound::Excluded(x) => self.kv_at(n).0.borrow() < x,
                Bound::Unbounded => true,
            };
            if ok {
                best = n;
                n = self.tree.links[n].right;
            } else {
                n = self.tree.links[n].left;
            }
        }
        best
    }

    fn range_nodes<T: ?Sized + Ord, R: RangeBounds<T>>(&self, range: &R) -> (usize, usize)
    where
        K: Borrow<T>,
    {
        let start = range.start_bound();
        let end = range.end_bound();
        match (start, end) {
            (Bound::Excluded(s), Bound::Excluded(e)) if s == e => {
                crate::panicking::panic_str("range start and end are equal and excluded in BTreeMap")
            }
            (Bound::Included(s) | Bound::Excluded(s), Bound::Included(e) | Bound::Excluded(e)) if s > e => {
                crate::panicking::panic_str("range start is greater than range end in BTreeMap")
            }
            _ => {}
        }
        let front = self.lower_bound(start);
        let back = self.upper_bound(end);
        if front == NIL || back == NIL || self.kv_at(front).0.borrow() > self.kv_at(back).0.borrow() {
            (NIL, NIL)
        } else {
            (front, back)
        }
    }

    pub fn get<Q: ?Sized + Ord>(&self, key: &Q) -> Option<&V>
    where
        K: Borrow<Q>,
    {
        let n = self.find(key);
        if n == NIL {
            None
        } else {
            Some(&self.kv_at(n).1)
        }
    }

    pub fn get_key_value<Q: ?Sized + Ord>(&self, k: &Q) -> Option<(&K, &V)>
    where
        K: Borrow<Q>,
    {
        let n = self.find(k);
        if n == NIL {
            None
        } else {
            let kv = self.kv_at(n);
            Some((&kv.0, &kv.1))
        }
    }

    pub fn contains_key<Q: ?Sized + Ord>(&self, key: &Q) -> bool
    where
        K: Borrow<Q>,
    {
        self.find(key) != NIL
    }

    pub fn get_mut<Q: ?Sized + Ord>(&mut self, key: &Q) -> Option<&mut V>
    where
        K: Borrow<Q>,
    {
        let n = self.find(key);
        if n == NIL {
            None
        } else {
            Some(&mut self.kv_at_mut(n).1)
        }
    }

    /// Inserts; an existing key keeps its key and gets the new value (as real std).
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let (found, parent, left) = self.search(&key);
        if found != NIL {
            return Some(crate::mem::replace(&mut self.kv_at_mut(found).1, value));
        }
        let n = self.new_node(key, value);
        self.tree.attach(n, parent, left);
        self.len += 1;
        None
    }

    pub fn try_insert(&mut self, key: K, value: V) -> Result<&mut V, OccupiedError<'_, K, V>> {
        match self.entry(key) {
            Entry::Occupied(entry) => Err(OccupiedError { entry, value }),
            Entry::Vacant(entry) => Ok(entry.insert(value)),
        }
    }

    pub fn remove<Q: ?Sized + Ord>(&mut self, key: &Q) -> Option<V>
    where
        K: Borrow<Q>,
    {
        let n = self.find(key);
        if n == NIL {
            None
        } else {
            Some(self.remove_node(n).1)
        }
    }

    pub fn remove_entry<Q: ?Sized + Ord>(&mut self, key: &Q) -> Option<(K, V)>
    where
        K: Borrow<Q>,
    {
        let n = self.find(key);
        if n == NIL {
            None
        } else {
            Some(self.remove_node(n))
        }
    }

    pub fn append(&mut self, other: &mut BTreeMap<K, V>) {
        let o = crate::mem::replace(other, BTreeMap::new());
        for (k, v) in o {
            self.insert(k, v);
        }
    }

    pub fn range<T: ?Sized + Ord, R: RangeBounds<T>>(&self, range: R) -> Range<'_, K, V>
    where
        K: Borrow<T>,
    {
        let (front, back) = self.range_nodes(&range);
        Range { map: self, front, back, done: front == NIL }
    }

    pub fn range_mut<T: ?Sized + Ord, R: RangeBounds<T>>(&mut self, range: R) -> RangeMut<'_, K, V>
    where
        K: Borrow<T>,
    {
        let (front, back) = self.range_nodes(&range);
        RangeMut { map: self as *mut BTreeMap<K, V>, front, back, done: front == NIL, _m: PhantomData }
    }

    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        let (found, parent, left) = self.search(&key);
        if found != NIL {
            Entry::Occupied(OccupiedEntry { map: self, node: found })
        } else {
            Entry::Vacant(VacantEntry { map: self, key, parent, left })
        }
    }

    /// Moves every element with key >= `key` into a new map.
    pub fn split_off<Q: ?Sized + Ord>(&mut self, key: &Q) -> BTreeMap<K, V>
    where
        K: Borrow<Q>,
    {
        let mut right = BTreeMap::new();
        loop {
            let n = self.tree.last();
            if n == NIL || self.kv_at(n).0.borrow() < key {
                break;
            }
            let (k, v) = self.remove_node(n);
            right.insert(k, v);
        }
        right
    }
}

impl<K, V> Drop for BTreeMap<K, V> {
    fn drop(&mut self) {
        self.drop_all();
    }
}

impl<K: Clone, V: Clone> Clone for BTreeMap<K, V> {
    fn clone(&self) -> BTreeMap<K, V> {
        // same shape, elements cloned slot by slot
        let mut kv: Vec<MaybeUninit<(K, V)>> = Vec::with_capacity(self.kv.len());
        let mut i = 0;
        while i < self.kv.len() {
            kv.push(MaybeUninit::uninit());
            i += 1;
        }
        let order = self.tree.in_order();
        let mut j = 0;
        while j < order.len() {
            let n = order[j];
            let (k, v) = self.kv_at(n);
            kv[n] = MaybeUninit::new((k.clone(), v.clone()));
            j += 1;
        }
        BTreeMap {
            tree: Tree { links: self.tree.links.clone(), root: self.tree.root, free: self.tree.free.clone() },
            kv,
            len: self.len,
        }
    }
}

impl<K, V> Default for BTreeMap<K, V> {
    fn default() -> BTreeMap<K, V> {
        BTreeMap::new()
    }
}

impl<K: PartialEq, V: PartialEq> PartialEq for BTreeMap<K, V> {
    fn eq(&self, other: &BTreeMap<K, V>) -> bool {
        self.len() == other.len() && self.iter().zip(other.iter()).all(|(a, b)| a == b)
    }
}

impl<K: Eq, V: Eq> Eq for BTreeMap<K, V> {}

impl<K: PartialOrd, V: PartialOrd> PartialOrd for BTreeMap<K, V> {
    fn partial_cmp(&self, other: &BTreeMap<K, V>) -> Option<Ordering> {
        self.iter().partial_cmp(other.iter())
    }
}

impl<K: Ord, V: Ord> Ord for BTreeMap<K, V> {
    fn cmp(&self, other: &BTreeMap<K, V>) -> Ordering {
        self.iter().cmp(other.iter())
    }
}

impl<K: Hash, V: Hash> Hash for BTreeMap<K, V> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.len);
        for (k, v) in self.iter() {
            k.hash(state);
            v.hash(state);
        }
    }
}

impl<K: fmt::Debug, V: fmt::Debug> fmt::Debug for BTreeMap<K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<'a, K: Ord + Borrow<Q>, Q: ?Sized + Ord, V> Index<&'a Q> for BTreeMap<K, V> {
    type Output = V;
    fn index(&self, key: &Q) -> &V {
        match self.get(key) {
            Some(v) => v,
            None => crate::panicking::panic_str("no entry found for key"),
        }
    }
}

impl<K: Ord, V> FromIterator<(K, V)> for BTreeMap<K, V> {
    fn from_iter<T: IntoIterator<Item = (K, V)>>(iter: T) -> BTreeMap<K, V> {
        let mut map = BTreeMap::new();
        for (k, v) in iter {
            map.insert(k, v);
        }
        map
    }
}

impl<K: Ord, V> Extend<(K, V)> for BTreeMap<K, V> {
    fn extend<T: IntoIterator<Item = (K, V)>>(&mut self, iter: T) {
        for (k, v) in iter {
            self.insert(k, v);
        }
    }
}

impl<'a, K: Ord + Copy, V: Copy> Extend<(&'a K, &'a V)> for BTreeMap<K, V> {
    fn extend<I: IntoIterator<Item = (&'a K, &'a V)>>(&mut self, iter: I) {
        for (k, v) in iter {
            self.insert(*k, *v);
        }
    }
}

// ---------------------------------------------------------------- entries

pub enum Entry<'a, K: 'a, V: 'a> {
    Vacant(VacantEntry<'a, K, V>),
    Occupied(OccupiedEntry<'a, K, V>),
}

pub struct VacantEntry<'a, K, V> {
    map: &'a mut BTreeMap<K, V>,
    key: K,
    parent: usize,
    left: bool,
}

pub struct OccupiedEntry<'a, K, V> {
    map: &'a mut BTreeMap<K, V>,
    node: usize,
}

pub struct OccupiedError<'a, K: 'a, V: 'a> {
    pub entry: OccupiedEntry<'a, K, V>,
    pub value: V,
}

impl<'a, K: Ord, V> Entry<'a, K, V> {
    pub fn or_insert(self, default: V) -> &'a mut V {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(default),
        }
    }
    pub fn or_insert_with<F: FnOnce() -> V>(self, default: F) -> &'a mut V {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(default()),
        }
    }
    pub fn or_insert_with_key<F: FnOnce(&K) -> V>(self, default: F) -> &'a mut V {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => {
                let value = default(entry.key());
                entry.insert(value)
            }
        }
    }
    pub fn key(&self) -> &K {
        match *self {
            Entry::Occupied(ref entry) => entry.key(),
            Entry::Vacant(ref entry) => entry.key(),
        }
    }
    pub fn and_modify<F: FnOnce(&mut V)>(self, f: F) -> Entry<'a, K, V> {
        match self {
            Entry::Occupied(mut entry) => {
                f(entry.get_mut());
                Entry::Occupied(entry)
            }
            Entry::Vacant(entry) => Entry::Vacant(entry),
        }
    }
}

impl<'a, K: Ord, V: Default> Entry<'a, K, V> {
    pub fn or_default(self) -> &'a mut V {
        match self {
            Entry::Occupied(entry) => entry.into_mut(),
            Entry::Vacant(entry) => entry.insert(V::default()),
        }
    }
}

impl<'a, K: Ord, V> VacantEntry<'a, K, V> {
    pub fn key(&self) -> &K {
        &self.key
    }
    pub fn into_key(self) -> K {
        self.key
    }
    pub fn insert(self, value: V) -> &'a mut V {
        let map: &'a mut BTreeMap<K, V> = self.map;
        let n = map.new_node(self.key, value);
        map.tree.attach(n, self.parent, self.left);
        map.len += 1;
        &mut map.kv_at_mut(n).1
    }
}

impl<'a, K: Ord, V> OccupiedEntry<'a, K, V> {
    pub fn key(&self) -> &K {
        &self.map.kv_at(self.node).0
    }
    pub fn remove_entry(self) -> (K, V) {
        self.map.remove_node(self.node)
    }
    pub fn get(&self) -> &V {
        &self.map.kv_at(self.node).1
    }
    pub fn get_mut(&mut self) -> &mut V {
        &mut self.map.kv_at_mut(self.node).1
    }
    pub fn into_mut(self) -> &'a mut V {
        let map: &'a mut BTreeMap<K, V> = self.map;
        &mut map.kv_at_mut(self.node).1
    }
    pub fn insert(&mut self, value: V) -> V {
        crate::mem::replace(self.get_mut(), value)
    }
    pub fn remove(self) -> V {
        self.remove_entry().1
    }
}

impl<'a, K: fmt::Debug + Ord, V: fmt::Debug> fmt::Debug for Entry<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Entry::Vacant(ref v) => f.debug_tuple("Entry").field(v).finish(),
            Entry::Occupied(ref o) => f.debug_tuple("Entry").field(o).finish(),
        }
    }
}

impl<'a, K: fmt::Debug + Ord, V> fmt::Debug for VacantEntry<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("VacantEntry").field(self.key()).finish()
    }
}

impl<'a, K: fmt::Debug + Ord, V: fmt::Debug> fmt::Debug for OccupiedEntry<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OccupiedEntry").field("key", self.key()).field("value", self.get()).finish()
    }
}

// ---------------------------------------------------------------- iterators

pub struct Iter<'a, K: 'a, V: 'a> {
    map: &'a BTreeMap<K, V>,
    front: usize,
    back: usize,
    remaining: usize,
}

impl<'a, K, V> Clone for Iter<'a, K, V> {
    fn clone(&self) -> Iter<'a, K, V> {
        Iter { map: self.map, front: self.front, back: self.back, remaining: self.remaining }
    }
}

impl<'a, K, V> Iterator for Iter<'a, K, V> {
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<(&'a K, &'a V)> {
        if self.remaining == 0 {
            return None;
        }
        let n = self.front;
        self.remaining -= 1;
        if self.remaining > 0 {
            self.front = self.map.tree.next(n);
        }
        let kv: &'a (K, V) = self.map.kv_at(n);
        Some((&kv.0, &kv.1))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
    fn last(self) -> Option<(&'a K, &'a V)> {
        let mut s = self;
        s.next_back()
    }
    fn min(self) -> Option<(&'a K, &'a V)> {
        let mut s = self;
        s.next()
    }
    fn max(self) -> Option<(&'a K, &'a V)> {
        let mut s = self;
        s.next_back()
    }
}

impl<'a, K, V> DoubleEndedIterator for Iter<'a, K, V> {
    fn next_back(&mut self) -> Option<(&'a K, &'a V)> {
        if self.remaining == 0 {
            return None;
        }
        let n = self.back;
        self.remaining -= 1;
        if self.remaining > 0 {
            self.back = self.map.tree.prev(n);
        }
        let kv: &'a (K, V) = self.map.kv_at(n);
        Some((&kv.0, &kv.1))
    }
}

impl<'a, K, V> ExactSizeIterator for Iter<'a, K, V> {}
impl<'a, K, V> FusedIterator for Iter<'a, K, V> {}

impl<'a, K: fmt::Debug, V: fmt::Debug> fmt::Debug for Iter<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut l = f.debug_list();
        for (k, v) in self.clone() {
            l.entry(&super::KvDebug(k, v));
        }
        l.finish()
    }
}

pub struct IterMut<'a, K: 'a, V: 'a> {
    map: *mut BTreeMap<K, V>,
    front: usize,
    back: usize,
    remaining: usize,
    _m: PhantomData<&'a mut (K, V)>,
}

impl<'a, K, V> Iterator for IterMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);
    fn next(&mut self) -> Option<(&'a K, &'a mut V)> {
        if self.remaining == 0 {
            return None;
        }
        let map = unsafe { &mut *self.map };
        let n = self.front;
        self.remaining -= 1;
        if self.remaining > 0 {
            self.front = map.tree.next(n);
        }
        let kv: &'a mut (K, V) = unsafe { &mut *map.kv[n].as_mut_ptr() };
        Some((&kv.0, &mut kv.1))
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}

impl<'a, K, V> DoubleEndedIterator for IterMut<'a, K, V> {
    fn next_back(&mut self) -> Option<(&'a K, &'a mut V)> {
        if self.remaining == 0 {
            return None;
        }
        let map = unsafe { &mut *self.map };
        let n = self.back;
        self.remaining -= 1;
        if self.remaining > 0 {
            self.back = map.tree.prev(n);
        }
        let kv: &'a mut (K, V) = unsafe { &mut *map.kv[n].as_mut_ptr() };
        Some((&kv.0, &mut kv.1))
    }
}

impl<'a, K, V> ExactSizeIterator for IterMut<'a, K, V> {}
impl<'a, K, V> FusedIterator for IterMut<'a, K, V> {}

pub struct IntoIter<K, V> {
    map: BTreeMap<K, V>,
}

impl<K, V> Iterator for IntoIter<K, V> {
    type Item = (K, V);
    fn next(&mut self) -> Option<(K, V)> {
        self.map.pop_first()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.map.len, Some(self.map.len))
    }
}

impl<K, V> DoubleEndedIterator for IntoIter<K, V> {
    fn next_back(&mut self) -> Option<(K, V)> {
        self.map.pop_last()
    }
}

impl<K, V> ExactSizeIterator for IntoIter<K, V> {}
impl<K, V> FusedIterator for IntoIter<K, V> {}

pub struct Range<'a, K: 'a, V: 'a> {
    map: &'a BTreeMap<K, V>,
    front: usize,
    back: usize,
    done: bool,
}

impl<'a, K, V> Clone for Range<'a, K, V> {
    fn clone(&self) -> Range<'a, K, V> {
        Range { map: self.map, front: self.front, back: self.back, done: self.done }
    }
}

impl<'a, K, V> Iterator for Range<'a, K, V> {
    type Item = (&'a K, &'a V);
    fn next(&mut self) -> Option<(&'a K, &'a V)> {
        if self.done {
            return None;
        }
        let n = self.front;
        if n == self.back {
            self.done = true;
        } else {
            self.front = self.map.tree.next(n);
        }
        let kv: &'a (K, V) = self.map.kv_at(n);
        Some((&kv.0, &kv.1))
    }
    fn last(self) -> Option<(&'a K, &'a V)> {
        let mut s = self;
        s.next_back()
    }
}

impl<'a, K, V> DoubleEndedIterator for Range<'a, K, V> {
    fn next_back(&mut self) -> Option<(&'a K, &'a V)> {
        if self.done {
            return None;
        }
        let n = self.back;
        if n == self.front {
            self.done = true;
        } else {
            self.back = self.map.tree.prev(n);
        }
        let kv: &'a (K, V) = self.map.kv_at(n);
        Some((&kv.0, &kv.1))
    }
}

impl<'a, K, V> FusedIterator for Range<'a, K, V> {}

pub struct RangeMut<'a, K: 'a, V: 'a> {
    map: *mut BTreeMap<K, V>,
    front: usize,
    back: usize,
    done: bool,
    _m: PhantomData<&'a mut (K, V)>,
}

impl<'a, K, V> Iterator for RangeMut<'a, K, V> {
    type Item = (&'a K, &'a mut V);
    fn next(&mut self) -> Option<(&'a K, &'a mut V)> {
        if self.done {
            return None;
        }
        let map = unsafe { &mut *self.map };
        let n = self.front;
        if n == self.back {
            self.done = true;
        } else {
            self.front = map.tree.next(n);
        }
        let kv: &'a mut (K, V) = unsafe { &mut *map.kv[n].as_mut_ptr() };
        Some((&kv.0, &mut kv.1))
    }
}

impl<'a, K, V> DoubleEndedIterator for RangeMut<'a, K, V> {
    fn next_back(&mut self) -> Option<(&'a K, &'a mut V)> {
        if self.done {
            return None;
        }
        let map = unsafe { &mut *self.map };
        let n = self.back;
        if n == self.front {
            self.done = true;
        } else {
            self.back = map.tree.prev(n);
        }
        let kv: &'a mut (K, V) = unsafe { &mut *map.kv[n].as_mut_ptr() };
        Some((&kv.0, &mut kv.1))
    }
}

pub struct ExtractIf<'a, K, V, F> {
    map: &'a mut BTreeMap<K, V>,
    cur: usize,
    pred: F,
}

impl<'a, K, V, F: FnMut(&K, &mut V) -> bool> Iterator for ExtractIf<'a, K, V, F> {
    type Item = (K, V);
    fn next(&mut self) -> Option<(K, V)> {
        while self.cur != NIL {
            let n = self.cur;
            let next = self.map.tree.next(n);
            let hit = {
                let kv = self.map.kv_at_mut(n);
                (self.pred)(&kv.0, &mut kv.1)
            };
            if hit {
                let freed = self.map.tree.unlink(n);
                if freed != n {
                    // the successor's element moved into slot n: visit n again
                    self.map.kv.swap(n, freed);
                    self.cur = n;
                } else {
                    self.cur = next;
                }
                self.map.len -= 1;
                return Some(unsafe { self.map.kv[freed].assume_init_read() });
            }
            self.cur = next;
        }
        None
    }
}

pub struct Keys<'a, K: 'a, V: 'a> {
    inner: Iter<'a, K, V>,
}

impl<'a, K, V> Clone for Keys<'a, K, V> {
    fn clone(&self) -> Keys<'a, K, V> {
        Keys { inner: self.inner.clone() }
    }
}

impl<'a, K, V> Iterator for Keys<'a, K, V> {
    type Item = &'a K;
    fn next(&mut self) -> Option<&'a K> {
        match self.inner.next() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
    fn last(self) -> Option<&'a K> {
        let mut s = self;
        s.next_back()
    }
}

impl<'a, K, V> DoubleEndedIterator for Keys<'a, K, V> {
    fn next_back(&mut self) -> Option<&'a K> {
        match self.inner.next_back() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
}

impl<'a, K, V> ExactSizeIterator for Keys<'a, K, V> {}
impl<'a, K, V> FusedIterator for Keys<'a, K, V> {}

pub struct Values<'a, K: 'a, V: 'a> {
    inner: Iter<'a, K, V>,
}

impl<'a, K, V> Clone for Values<'a, K, V> {
    fn clone(&self) -> Values<'a, K, V> {
        Values { inner: self.inner.clone() }
    }
}

impl<'a, K, V> Iterator for Values<'a, K, V> {
    type Item = &'a V;
    fn next(&mut self) -> Option<&'a V> {
        match self.inner.next() {
            Some((_, v)) => Some(v),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K, V> DoubleEndedIterator for Values<'a, K, V> {
    fn next_back(&mut self) -> Option<&'a V> {
        match self.inner.next_back() {
            Some((_, v)) => Some(v),
            None => None,
        }
    }
}

impl<'a, K, V> ExactSizeIterator for Values<'a, K, V> {}

pub struct ValuesMut<'a, K: 'a, V: 'a> {
    inner: IterMut<'a, K, V>,
}

impl<'a, K, V> Iterator for ValuesMut<'a, K, V> {
    type Item = &'a mut V;
    fn next(&mut self) -> Option<&'a mut V> {
        match self.inner.next() {
            Some((_, v)) => Some(v),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<'a, K, V> DoubleEndedIterator for ValuesMut<'a, K, V> {
    fn next_back(&mut self) -> Option<&'a mut V> {
        match self.inner.next_back() {
            Some((_, v)) => Some(v),
            None => None,
        }
    }
}

impl<'a, K, V> ExactSizeIterator for ValuesMut<'a, K, V> {}

pub struct IntoKeys<K, V> {
    inner: IntoIter<K, V>,
}

impl<K, V> Iterator for IntoKeys<K, V> {
    type Item = K;
    fn next(&mut self) -> Option<K> {
        match self.inner.next() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K, V> DoubleEndedIterator for IntoKeys<K, V> {
    fn next_back(&mut self) -> Option<K> {
        match self.inner.next_back() {
            Some((k, _)) => Some(k),
            None => None,
        }
    }
}

pub struct IntoValues<K, V> {
    inner: IntoIter<K, V>,
}

impl<K, V> Iterator for IntoValues<K, V> {
    type Item = V;
    fn next(&mut self) -> Option<V> {
        match self.inner.next() {
            Some((_, v)) => Some(v),
            None => None,
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl<K, V> DoubleEndedIterator for IntoValues<K, V> {
    fn next_back(&mut self) -> Option<V> {
        match self.inner.next_back() {
            Some((_, v)) => Some(v),
            None => None,
        }
    }
}

impl<'a, K, V> IntoIterator for &'a BTreeMap<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = Iter<'a, K, V>;
    fn into_iter(self) -> Iter<'a, K, V> {
        self.iter()
    }
}

impl<'a, K, V> IntoIterator for &'a mut BTreeMap<K, V> {
    type Item = (&'a K, &'a mut V);
    type IntoIter = IterMut<'a, K, V>;
    fn into_iter(self) -> IterMut<'a, K, V> {
        self.iter_mut()
    }
}

impl<K, V> IntoIterator for BTreeMap<K, V> {
    type Item = (K, V);
    type IntoIter = IntoIter<K, V>;
    fn into_iter(self) -> IntoIter<K, V> {
        IntoIter { map: self }
    }
}

#[cfg(any(hotrust_check, hotrust_const_generics))]
impl<K: Ord, V, const N: usize> From<[(K, V); N]> for BTreeMap<K, V> {
    fn from(arr: [(K, V); N]) -> BTreeMap<K, V> {
        let mut map = BTreeMap::new();
        for (k, v) in arr {
            map.insert(k, v);
        }
        map
    }
}
