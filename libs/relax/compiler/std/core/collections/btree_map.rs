//! Ordered map: port of real alloc's B-tree algorithms (MIT/Apache-2.0).
//! Inline B = 6 nodes and non-generic balancing live in btree_node.rs.

use super::btree_node::{Cursor, NodeLayout, Tree, NIL};
use crate::borrow::Borrow;
use crate::cmp::Ordering;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::iter::{FromIterator, FusedIterator};
use crate::marker::PhantomData;

use crate::ops::{Bound, Index, RangeBounds};

#[rustc_insignificant_dtor]
pub struct BTreeMap<K, V> {
    tree: Tree,
    _marker: PhantomData<(K, V)>,
    len: usize,
}

unsafe impl<K: Send, V: Send> Send for BTreeMap<K, V> {}
unsafe impl<K: Sync, V: Sync> Sync for BTreeMap<K, V> {}

impl<K, V> BTreeMap<K, V> {
    pub const fn new() -> BTreeMap<K, V> {
        BTreeMap { tree: Tree::new(), _marker: PhantomData, len: 0 }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        let old = crate::mem::replace(self, BTreeMap::new());
        drop(old);
    }

    /// Derive node offsets and allocation layouts from K and V without storing them per map.
    #[inline]
    fn node_layout() -> NodeLayout {
        NodeLayout::new(crate::mem::size_of::<K>(), crate::mem::align_of::<K>(), crate::mem::size_of::<V>(), crate::mem::align_of::<V>())
    }

    #[inline]
    fn first_node(&self) -> Cursor {
        if self.tree.root().is_null() { NIL } else { self.tree.first(&Self::node_layout()) }
    }

    #[inline]
    fn last_node(&self) -> Cursor {
        if self.tree.root().is_null() { NIL } else { self.tree.last(&Self::node_layout()) }
    }

    fn drop_all(&mut self) {
        // Unlink each element before calling user Drop, so unwinding cannot
        // leave already-dropped elements reachable from a surviving map.
        if crate::mem::needs_drop::<(K, V)>() {
            while let Some(pair) = self.pop_first() { drop(pair); }
        }
        if !self.tree.root().is_null() {
            unsafe { self.tree.free_nodes(&Self::node_layout()); }
        }
    }

    #[inline]
    fn kv_at(&self, n: Cursor) -> (&K, &V) {
        let l = Self::node_layout();
        unsafe { (&*(l.key(n) as *const K), &*(l.val(n) as *const V)) }
    }

    #[inline]
    fn kv_at_mut(&mut self, n: Cursor) -> (&K, &mut V) {
        let l = Self::node_layout();
        unsafe { (&*(l.key(n) as *const K), &mut *(l.val(n) as *mut V)) }
    }

    fn insert_at(&mut self, at: Cursor, key: K, value: V) -> Cursor {
        let n = unsafe { self.tree.insert(at, &key as *const K as *const u8, &value as *const V as *const u8, &Self::node_layout()) };
        crate::mem::forget(key);
        crate::mem::forget(value);
        self.len += 1;
        n
    }

    fn remove_tracking(&mut self, n: Cursor) -> ((K, V), Cursor) {
        let l = Self::node_layout();
        unsafe {
            let pair = (crate::ptr::read(l.key(n) as *const K), crate::ptr::read(l.val(n) as *const V));
            let next = self.tree.remove(n, &l);
            self.len -= 1;
            (pair, next)
        }
    }

    fn remove_node(&mut self, n: Cursor) -> (K, V) { self.remove_tracking(n).0 }

    pub fn first_key_value(&self) -> Option<(&K, &V)> {
        let n = self.first_node();
        if n == NIL {
            None
        } else {
            let kv = self.kv_at(n);
            Some((kv.0, kv.1))
        }
    }

    pub fn last_key_value(&self) -> Option<(&K, &V)> {
        let n = self.last_node();
        if n == NIL {
            None
        } else {
            let kv = self.kv_at(n);
            Some((kv.0, kv.1))
        }
    }

    pub fn pop_first(&mut self) -> Option<(K, V)> {
        let n = self.first_node();
        if n == NIL {
            None
        } else {
            Some(self.remove_node(n))
        }
    }

    pub fn pop_last(&mut self) -> Option<(K, V)> {
        let n = self.last_node();
        if n == NIL {
            None
        } else {
            Some(self.remove_node(n))
        }
    }

    pub fn first_entry(&mut self) -> Option<OccupiedEntry<'_, K, V>> {
        let n = self.first_node();
        if n == NIL {
            None
        } else {
            Some(OccupiedEntry { map: self, node: n })
        }
    }

    pub fn last_entry(&mut self) -> Option<OccupiedEntry<'_, K, V>> {
        let n = self.last_node();
        if n == NIL {
            None
        } else {
            Some(OccupiedEntry { map: self, node: n })
        }
    }

    pub fn iter(&self) -> Iter<'_, K, V> {
        Iter { map: self, front: self.first_node(), back: self.last_node(), remaining: self.len }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, K, V> {
        let front = self.first_node();
        let back = self.last_node();
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
        let mut n = self.first_node();
        while n != NIL {
            let (k, v) = self.kv_at_mut(n);
            if f(k, v) {
                n = self.tree.next(n, &Self::node_layout());
            } else {
                let (pair, next) = self.remove_tracking(n);
                n = next;
                drop(pair);
            }
        }
    }

    pub fn extract_if<F: FnMut(&K, &mut V) -> bool>(&mut self, pred: F) -> ExtractIf<'_, K, V, F> {
        let first = self.first_node();
        ExtractIf { map: self, cur: first, pred }
    }
}

impl<K: Ord, V> BTreeMap<K, V> {
    fn search_key<Q: ?Sized + Ord>(&self, key: &Q) -> (Cursor, bool)
    where K: Borrow<Q>,
    {
        let mut n = self.tree.root();
        if n.is_null() { return (NIL, false); }
        let layout = Self::node_layout();
        unsafe {
            loop {
                let mut i = 0;
                while i < (*n).len {
                    let at = Cursor { node: n, idx: i };
                    let k = &*(layout.key(at) as *const K);
                    match Q::cmp(key, <K as Borrow<Q>>::borrow(k)) {
                        Ordering::Less => break,
                        Ordering::Equal => return (at, true),
                        Ordering::Greater => i += 1,
                    }
                }
                if (*n).leaf { return (Cursor { node: n, idx: i }, false); }
                n = layout.child(n, i);
            }
        }
    }

    fn find<Q: ?Sized + Ord>(&self, key: &Q) -> Cursor
    where K: Borrow<Q>,
    {
        let (at, found) = self.search_key(key);
        if found { at } else { NIL }
    }

    fn lower_bound<Q: ?Sized + Ord>(&self, bound: Bound<&Q>) -> Cursor
    where K: Borrow<Q>,
    {
        let (key, included) = match bound {
            Bound::Unbounded => return self.first_node(),
            Bound::Included(k) => (k, true),
            Bound::Excluded(k) => (k, false),
        };
        let (at, found) = self.search_key(key);
        if at == NIL { NIL }
        else if found { if included { at } else { self.tree.next(at, &Self::node_layout()) } }
        else { self.tree.next_kv(at) }
    }

    fn upper_bound<Q: ?Sized + Ord>(&self, bound: Bound<&Q>) -> Cursor
    where K: Borrow<Q>,
    {
        let (key, included) = match bound {
            Bound::Unbounded => return self.last_node(),
            Bound::Included(k) => (k, true),
            Bound::Excluded(k) => (k, false),
        };
        let (at, found) = self.search_key(key);
        if at == NIL { NIL }
        else if found { if included { at } else { self.tree.prev(at, &Self::node_layout()) } }
        else { self.tree.prev_kv(at) }
    }

    fn range_nodes<T: ?Sized + Ord, R: RangeBounds<T>>(&self, range: &R) -> (Cursor, Cursor)
    where
        K: Borrow<T>,
    {
        // As real std: the bounds are validated only when the map has a root (a never-allocated
        // or cleared map returns an empty range without panicking).
        let start = range.start_bound();
        let end = range.end_bound();
        if self.tree.root().is_null() {
            return (NIL, NIL);
        }
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
        if front == NIL || back == NIL || T::gt(<K as Borrow<T>>::borrow(self.kv_at(front).0), <K as Borrow<T>>::borrow(self.kv_at(back).0)) {
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
            Some(self.kv_at(n).1)
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
            Some((kv.0, kv.1))
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
            Some(self.kv_at_mut(n).1)
        }
    }

    /// Inserts; an existing key keeps its key and gets the new value (as real std).
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let (at, found) = self.search_key(&key);
        if found {
            return Some(crate::mem::replace(self.kv_at_mut(at).1, value));
        }
        self.insert_at(at, key, value);
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
        let (front, back) = self.range_nodes::<T, R>(&range);
        Range { map: self, front, back, done: front == NIL }
    }

    pub fn range_mut<T: ?Sized + Ord, R: RangeBounds<T>>(&mut self, range: R) -> RangeMut<'_, K, V>
    where
        K: Borrow<T>,
    {
        let (front, back) = self.range_nodes::<T, R>(&range);
        RangeMut { map: self as *mut BTreeMap<K, V>, front, back, done: front == NIL, _m: PhantomData }
    }

    pub fn entry(&mut self, key: K) -> Entry<'_, K, V> {
        let (at, found) = self.search_key(&key);
        if found {
            Entry::Occupied(OccupiedEntry { map: self, node: at })
        } else {
            Entry::Vacant(VacantEntry { map: self, key, at })
        }
    }

    /// Moves every element with key >= `key` into a new map.
    pub fn split_off<Q: ?Sized + Ord>(&mut self, key: &Q) -> BTreeMap<K, V>
    where
        K: Borrow<Q>,
    {
        let mut right = BTreeMap::new();
        loop {
            let n = self.last_node();
            if n == NIL || Q::lt(<K as Borrow<Q>>::borrow(self.kv_at(n).0), key) {
                break;
            }
            let (k, v) = self.remove_node(n);
            right.insert(k, v);
        }
        right
    }
}

// Keep draining during unwinding if an element destructor panics. Every pair
// is detached before its destructor runs, so the guard sees only live elements.
struct DropRemaining<'a, K, V> { map: &'a mut BTreeMap<K, V> }
impl<'a, K, V> Drop for DropRemaining<'a, K, V> {
    fn drop(&mut self) { self.map.drop_all(); }
}

unsafe impl<#[may_dangle] K, #[may_dangle] V> Drop for BTreeMap<K, V> {
    fn drop(&mut self) {
        let guard = DropRemaining { map: self };
        guard.map.drop_all();
    }
}

impl<K: Clone, V: Clone> Clone for BTreeMap<K, V> {
    fn clone(&self) -> BTreeMap<K, V> {
        let mut out = BTreeMap::new();
        for (k, v) in self.iter() {
            // Sorted insertion needs no Ord bound. A partially cloned map owns
            // every completed pair, including when K::clone or V::clone panics.
            let last = out.last_node();
            let at = if last == NIL { NIL } else { Cursor { node: last.node, idx: last.idx + 1 } };
            out.insert_at(at, k.clone(), v.clone());
        }
        out
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
    at: Cursor,
}

pub struct OccupiedEntry<'a, K, V> {
    map: &'a mut BTreeMap<K, V>,
    node: Cursor,
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
        let n = map.insert_at(self.at, self.key, value);
        map.kv_at_mut(n).1
    }
}

impl<'a, K: Ord, V> OccupiedEntry<'a, K, V> {
    pub fn key(&self) -> &K {
        self.map.kv_at(self.node).0
    }
    pub fn remove_entry(self) -> (K, V) {
        self.map.remove_node(self.node)
    }
    pub fn get(&self) -> &V {
        self.map.kv_at(self.node).1
    }
    pub fn get_mut(&mut self) -> &mut V {
        self.map.kv_at_mut(self.node).1
    }
    pub fn into_mut(self) -> &'a mut V {
        let map: &'a mut BTreeMap<K, V> = self.map;
        map.kv_at_mut(self.node).1
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
    front: Cursor,
    back: Cursor,
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
            self.front = self.map.tree.next(n, &BTreeMap::<K, V>::node_layout());
        }
        let kv: (&'a K, &'a V) = self.map.kv_at(n);
        Some((kv.0, kv.1))
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
            self.back = self.map.tree.prev(n, &BTreeMap::<K, V>::node_layout());
        }
        let kv: (&'a K, &'a V) = self.map.kv_at(n);
        Some((kv.0, kv.1))
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
    front: Cursor,
    back: Cursor,
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
        let l = BTreeMap::<K, V>::node_layout();
        let n = self.front;
        self.remaining -= 1;
        if self.remaining > 0 {
            self.front = map.tree.next(n, &l);
        }
        let kv: (&'a K, &'a mut V) = unsafe { (&*(l.key(n) as *const K), &mut *(l.val(n) as *mut V)) };
        Some((kv.0, kv.1))
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
        let l = BTreeMap::<K, V>::node_layout();
        let n = self.back;
        self.remaining -= 1;
        if self.remaining > 0 {
            self.back = map.tree.prev(n, &l);
        }
        let kv: (&'a K, &'a mut V) = unsafe { (&*(l.key(n) as *const K), &mut *(l.val(n) as *mut V)) };
        Some((kv.0, kv.1))
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
    front: Cursor,
    back: Cursor,
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
            self.front = self.map.tree.next(n, &BTreeMap::<K, V>::node_layout());
        }
        let kv: (&'a K, &'a V) = self.map.kv_at(n);
        Some((kv.0, kv.1))
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
            self.back = self.map.tree.prev(n, &BTreeMap::<K, V>::node_layout());
        }
        let kv: (&'a K, &'a V) = self.map.kv_at(n);
        Some((kv.0, kv.1))
    }
}

impl<'a, K, V> FusedIterator for Range<'a, K, V> {}

pub struct RangeMut<'a, K: 'a, V: 'a> {
    map: *mut BTreeMap<K, V>,
    front: Cursor,
    back: Cursor,
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
        let l = BTreeMap::<K, V>::node_layout();
        let n = self.front;
        if n == self.back {
            self.done = true;
        } else {
            self.front = map.tree.next(n, &l);
        }
        let kv: (&'a K, &'a mut V) = unsafe { (&*(l.key(n) as *const K), &mut *(l.val(n) as *mut V)) };
        Some((kv.0, kv.1))
    }
}

impl<'a, K, V> DoubleEndedIterator for RangeMut<'a, K, V> {
    fn next_back(&mut self) -> Option<(&'a K, &'a mut V)> {
        if self.done {
            return None;
        }
        let map = unsafe { &mut *self.map };
        let l = BTreeMap::<K, V>::node_layout();
        let n = self.back;
        if n == self.front {
            self.done = true;
        } else {
            self.back = map.tree.prev(n, &l);
        }
        let kv: (&'a K, &'a mut V) = unsafe { (&*(l.key(n) as *const K), &mut *(l.val(n) as *mut V)) };
        Some((kv.0, kv.1))
    }
}

pub struct ExtractIf<'a, K, V, F> {
    map: &'a mut BTreeMap<K, V>,
    cur: Cursor,
    pred: F,
}

impl<'a, K, V, F: FnMut(&K, &mut V) -> bool> Iterator for ExtractIf<'a, K, V, F> {
    type Item = (K, V);
    fn next(&mut self) -> Option<(K, V)> {
        while self.cur != NIL {
            let n = self.cur;
            let hit = {
                let (k, v) = self.map.kv_at_mut(n);
                (self.pred)(k, v)
            };
            if hit {
                let (pair, next) = self.map.remove_tracking(n);
                self.cur = next;
                return Some(pair);
            }
            self.cur = self.map.tree.next(n, &BTreeMap::<K, V>::node_layout());
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

impl<K, V> ExactSizeIterator for IntoKeys<K, V> {}
impl<K, V> FusedIterator for IntoKeys<K, V> {}

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

impl<K, V> ExactSizeIterator for IntoValues<K, V> {}
impl<K, V> FusedIterator for IntoValues<K, V> {}

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

impl<K: Ord, V, const N: usize> From<[(K, V); N]> for BTreeMap<K, V> {
    fn from(arr: [(K, V); N]) -> BTreeMap<K, V> {
        let mut map = BTreeMap::new();
        for (k, v) in arr {
            map.insert(k, v);
        }
        map
    }
}

// Cursors only access nodes protected by the iterator's map borrow.
unsafe impl<'a, K: Sync, V: Sync> Send for Iter<'a, K, V> {}
unsafe impl<'a, K: Sync, V: Sync> Sync for Iter<'a, K, V> {}
unsafe impl<'a, K: Sync, V: Sync> Send for Range<'a, K, V> {}
unsafe impl<'a, K: Sync, V: Sync> Sync for Range<'a, K, V> {}
unsafe impl<'a, K: Send, V: Send> Send for IterMut<'a, K, V> {}
unsafe impl<'a, K: Sync, V: Sync> Sync for IterMut<'a, K, V> {}
unsafe impl<'a, K: Send, V: Send> Send for RangeMut<'a, K, V> {}
unsafe impl<'a, K: Sync, V: Sync> Sync for RangeMut<'a, K, V> {}
unsafe impl<'a, K: Send, V: Send> Send for OccupiedEntry<'a, K, V> {}
unsafe impl<'a, K: Sync, V: Sync> Sync for OccupiedEntry<'a, K, V> {}
unsafe impl<'a, K: Send, V: Send> Send for VacantEntry<'a, K, V> {}
unsafe impl<'a, K: Sync, V: Sync> Sync for VacantEntry<'a, K, V> {}
unsafe impl<'a, K: Send, V: Send, F: Send> Send for ExtractIf<'a, K, V, F> {}
unsafe impl<'a, K: Sync, V: Sync, F: Sync> Sync for ExtractIf<'a, K, V, F> {}
