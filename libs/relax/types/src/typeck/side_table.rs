//! Compact storage of the typeck results (Body). `ThinVec<T>` is one pointer to a heap block
//! `[len, cap | T..]` (8 B in the Body, no allocation when empty). `SideTable<K, V>` is rustc's
//! ItemLocalMap as a ThinVec of entries sorted by key: lookup is a binary search, and typeck
//! records nodes mostly in increasing id order, so an insert is mostly a push. Body::freeze
//! shrinks both to exactly their entries. `BitSet` is the typed-node flags, one bit per node.

use std::alloc::{alloc, dealloc, handle_alloc_error, realloc, Layout};
use std::marker::PhantomData;
use std::ptr::NonNull;

#[repr(C)]
struct Header {
    len: u32,
    cap: u32,
}

static EMPTY: Header = Header { len: 0, cap: 0 };

pub struct ThinVec<T> {
    ptr: NonNull<Header>,
    _m: PhantomData<T>,
}

unsafe impl<T: Send> Send for ThinVec<T> {}
unsafe impl<T: Sync> Sync for ThinVec<T> {}

impl<T> ThinVec<T> {
    const OFFSET: usize = {
        let a = std::mem::align_of::<T>();
        let h = std::mem::size_of::<Header>();
        (h + a - 1) / a * a
    };

    #[inline]
    pub fn new() -> Self {
        ThinVec { ptr: NonNull::from(&EMPTY), _m: PhantomData }
    }
    pub fn with_capacity(cap: usize) -> Self {
        let mut v = Self::new();
        v.set_cap(cap);
        v
    }
    fn layout(cap: usize) -> Layout {
        let size = std::mem::size_of::<T>().checked_mul(cap).and_then(|b| b.checked_add(Self::OFFSET)).expect("thin vec size");
        Layout::from_size_align(size, std::mem::align_of::<T>().max(std::mem::align_of::<Header>())).expect("thin vec layout")
    }
    #[inline]
    fn header(&self) -> &Header {
        unsafe { self.ptr.as_ref() }
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.header().len as usize
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.header().len == 0
    }
    #[inline]
    pub fn capacity(&self) -> usize {
        self.header().cap as usize
    }
    #[inline]
    fn data(&self) -> *mut T {
        unsafe { (self.ptr.as_ptr() as *mut u8).add(Self::OFFSET) as *mut T }
    }
    /// Not on the shared empty header (capacity 0): it is never written.
    #[inline]
    unsafe fn set_len(&mut self, n: usize) {
        unsafe { (*self.ptr.as_ptr()).len = n as u32 };
    }
    #[inline]
    pub fn as_slice(&self) -> &[T] {
        if self.capacity() == 0 { &[] } else { unsafe { std::slice::from_raw_parts(self.data(), self.len()) } }
    }
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        if self.capacity() == 0 { &mut [] } else { unsafe { std::slice::from_raw_parts_mut(self.data(), self.len()) } }
    }
    /// Heap bytes (memory report).
    pub fn heap_bytes(&self) -> usize {
        if self.capacity() == 0 { 0 } else { Self::layout(self.capacity()).size() }
    }
    /// Reallocates to exactly `cap` (>= len) elements; 0 frees the block.
    fn set_cap(&mut self, cap: usize) {
        let old = self.capacity();
        if cap == old {
            return;
        }
        debug_assert!(cap >= self.len());
        if cap == 0 {
            // only an empty vector shrinks to nothing: no elements to drop
            unsafe { dealloc(self.ptr.as_ptr() as *mut u8, Self::layout(old)) };
            self.ptr = NonNull::from(&EMPTY);
            return;
        }
        let cap32 = u32::try_from(cap).expect("thin vec capacity");
        let new_layout = Self::layout(cap);
        let p = unsafe { if old == 0 { alloc(new_layout) } else { realloc(self.ptr.as_ptr() as *mut u8, Self::layout(old), new_layout.size()) } };
        let Some(p) = NonNull::new(p as *mut Header) else { handle_alloc_error(new_layout) };
        unsafe {
            if old == 0 {
                p.as_ptr().write(Header { len: 0, cap: cap32 });
            } else {
                (*p.as_ptr()).cap = cap32;
            }
        }
        self.ptr = p;
    }
    #[inline]
    fn grow_for(&mut self, extra: usize) {
        let need = self.len() + extra;
        if need > self.capacity() {
            self.set_cap(need.max(4).max(self.capacity() * 2));
        }
    }
    pub fn reserve(&mut self, extra: usize) {
        self.grow_for(extra);
    }
    /// Frees the unused capacity (the typeck results are final).
    pub fn shrink_to_fit(&mut self) {
        let n = self.len();
        self.set_cap(n);
    }
    #[inline]
    pub fn push(&mut self, x: T) {
        let n = self.len();
        if n == self.capacity() {
            self.grow_for(1);
        }
        unsafe {
            self.data().add(n).write(x);
            self.set_len(n + 1);
        }
    }
    pub fn pop(&mut self) -> Option<T> {
        let n = self.len();
        if n == 0 {
            return None;
        }
        unsafe {
            self.set_len(n - 1);
            Some(self.data().add(n - 1).read())
        }
    }
    pub fn insert(&mut self, i: usize, x: T) {
        let n = self.len();
        assert!(i <= n, "thin vec insert index");
        if n == self.capacity() {
            self.grow_for(1);
        }
        unsafe {
            let p = self.data().add(i);
            if i < n {
                std::ptr::copy(p, p.add(1), n - i);
            }
            p.write(x);
            self.set_len(n + 1);
        }
    }
    pub fn remove(&mut self, i: usize) -> T {
        let n = self.len();
        assert!(i < n, "thin vec remove index");
        unsafe {
            let p = self.data().add(i);
            let x = p.read();
            std::ptr::copy(p.add(1), p, n - i - 1);
            self.set_len(n - 1);
            x
        }
    }
    pub fn truncate(&mut self, n: usize) {
        let len = self.len();
        if n >= len {
            return;
        }
        unsafe {
            self.set_len(n);
            std::ptr::drop_in_place(std::ptr::slice_from_raw_parts_mut(self.data().add(n), len - n));
        }
    }
    pub fn clear(&mut self) {
        self.truncate(0);
    }
    /// Keeps the elements `f` accepts, in order.
    pub fn retain(&mut self, mut f: impl FnMut(&mut T) -> bool) {
        let n = self.len();
        if n == 0 {
            return;
        }
        // elements past `kept` are moved out or dropped; a panic in `f` leaks them, never
        // drops one twice
        unsafe { self.set_len(0) };
        let mut kept = 0;
        for i in 0..n {
            unsafe {
                let p = self.data().add(i);
                if f(&mut *p) {
                    if i != kept {
                        std::ptr::copy_nonoverlapping(p, self.data().add(kept), 1);
                    }
                    kept += 1;
                } else {
                    std::ptr::drop_in_place(p);
                }
            }
        }
        unsafe { self.set_len(kept) };
    }
    pub fn iter(&self) -> std::slice::Iter<'_, T> {
        self.as_slice().iter()
    }
    pub fn iter_mut(&mut self) -> std::slice::IterMut<'_, T> {
        self.as_mut_slice().iter_mut()
    }
    /// The elements as a Vec (moved out; this vector is left empty).
    pub fn take_vec(&mut self) -> Vec<T> {
        let n = self.len();
        let mut v = Vec::with_capacity(n);
        if n > 0 {
            unsafe {
                std::ptr::copy_nonoverlapping(self.data(), v.as_mut_ptr(), n);
                self.set_len(0);
                v.set_len(n);
            }
        }
        v
    }
}

impl<T: Clone> ThinVec<T> {
    pub fn from_slice(s: &[T]) -> Self {
        let mut v = Self::new();
        v.extend_from_slice(s);
        v
    }
    pub fn extend_from_slice(&mut self, s: &[T]) {
        self.grow_for(s.len());
        for x in s {
            let n = self.len();
            unsafe {
                self.data().add(n).write(x.clone());
                self.set_len(n + 1);
            }
        }
    }
    pub fn to_vec(&self) -> Vec<T> {
        self.as_slice().to_vec()
    }
}

impl<T> Drop for ThinVec<T> {
    fn drop(&mut self) {
        let cap = self.capacity();
        if cap == 0 {
            return;
        }
        unsafe {
            std::ptr::drop_in_place(std::ptr::slice_from_raw_parts_mut(self.data(), self.len()));
            dealloc(self.ptr.as_ptr() as *mut u8, Self::layout(cap));
        }
    }
}

impl<T: Clone> Clone for ThinVec<T> {
    /// An exact-size copy.
    fn clone(&self) -> Self {
        let mut v = Self::new();
        v.set_cap(self.len());
        v.extend_from_slice(self.as_slice());
        v
    }
}

impl<T> Default for ThinVec<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> std::ops::Deref for ThinVec<T> {
    type Target = [T];
    #[inline]
    fn deref(&self) -> &[T] {
        self.as_slice()
    }
}

impl<T> std::ops::DerefMut for ThinVec<T> {
    #[inline]
    fn deref_mut(&mut self) -> &mut [T] {
        self.as_mut_slice()
    }
}

impl<T: std::fmt::Debug> std::fmt::Debug for ThinVec<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T: PartialEq> PartialEq for ThinVec<T> {
    fn eq(&self, o: &Self) -> bool {
        self.as_slice() == o.as_slice()
    }
}
impl<T: Eq> Eq for ThinVec<T> {}

impl<T> FromIterator<T> for ThinVec<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut v = Self::new();
        v.extend(iter);
        v
    }
}

impl<T> Extend<T> for ThinVec<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        let it = iter.into_iter();
        self.grow_for(it.size_hint().0);
        for x in it {
            self.push(x);
        }
    }
}

impl<T> From<Vec<T>> for ThinVec<T> {
    fn from(v: Vec<T>) -> Self {
        let mut t = Self::new();
        if v.is_empty() {
            return t;
        }
        t.set_cap(v.len());
        let mut v = std::mem::ManuallyDrop::new(v);
        unsafe {
            std::ptr::copy_nonoverlapping(v.as_ptr(), t.data(), v.len());
            t.set_len(v.len());
            v.set_len(0);
            std::mem::ManuallyDrop::drop(&mut v);
        }
        t
    }
}

impl<'a, T> IntoIterator for &'a ThinVec<T> {
    type Item = &'a T;
    type IntoIter = std::slice::Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.as_slice().iter()
    }
}

impl<'a, T> IntoIterator for &'a mut ThinVec<T> {
    type Item = &'a mut T;
    type IntoIter = std::slice::IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.as_mut_slice().iter_mut()
    }
}

impl<T> IntoIterator for ThinVec<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;
    fn into_iter(mut self) -> Self::IntoIter {
        self.take_vec().into_iter()
    }
}

/// One flag per node, 64 to a word.
#[derive(Clone, Default, Debug)]
pub struct BitSet {
    words: ThinVec<u64>,
    len: u32,
}

impl BitSet {
    pub fn new(len: usize) -> Self {
        let mut words = ThinVec::with_capacity(len.div_ceil(64));
        words.extend(std::iter::repeat_n(0, len.div_ceil(64)));
        BitSet { words, len: u32::try_from(len).expect("bit set length") }
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.len as usize
    }
    #[inline]
    pub fn get(&self, i: usize) -> bool {
        assert!(i < self.len(), "bit set index");
        self.words[i / 64] >> (i % 64) & 1 != 0
    }
    #[inline]
    pub fn set(&mut self, i: usize) {
        assert!(i < self.len(), "bit set index");
        self.words[i / 64] |= 1 << (i % 64);
    }
    pub fn iter(&self) -> impl Iterator<Item = bool> + '_ {
        (0..self.len()).map(|i| self.get(i))
    }
    pub fn heap_bytes(&self) -> usize {
        self.words.heap_bytes()
    }
    pub fn shrink_to_fit(&mut self) {
        self.words.shrink_to_fit();
    }
}

pub struct SideTable<K, V> {
    v: ThinVec<(K, V)>,
}

impl<K, V> SideTable<K, V> {
    #[inline]
    pub fn new() -> Self {
        SideTable { v: ThinVec::new() }
    }
    #[inline]
    pub fn len(&self) -> usize {
        self.v.len()
    }
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.v.is_empty()
    }
    /// The entries in key order.
    #[inline]
    pub fn as_slice(&self) -> &[(K, V)] {
        self.v.as_slice()
    }
    pub fn heap_bytes(&self) -> usize {
        self.v.heap_bytes()
    }
    pub fn shrink_to_fit(&mut self) {
        self.v.shrink_to_fit();
    }
    pub fn iter(&self) -> impl DoubleEndedIterator<Item = (&K, &V)> + ExactSizeIterator + Clone {
        self.v.iter().map(|(k, v)| (k, v))
    }
    pub fn iter_mut(&mut self) -> impl DoubleEndedIterator<Item = (&K, &mut V)> + ExactSizeIterator {
        self.v.iter_mut().map(|(k, v)| (&*k, v))
    }
    pub fn keys(&self) -> impl DoubleEndedIterator<Item = &K> + ExactSizeIterator + Clone {
        self.v.iter().map(|(k, _)| k)
    }
    pub fn values(&self) -> impl DoubleEndedIterator<Item = &V> + ExactSizeIterator + Clone {
        self.v.iter().map(|(_, v)| v)
    }
    pub fn values_mut(&mut self) -> impl DoubleEndedIterator<Item = &mut V> + ExactSizeIterator {
        self.v.iter_mut().map(|(_, v)| v)
    }
    pub fn clear(&mut self) {
        self.v.clear();
    }
    /// Keeps the entries `f` accepts, in order.
    pub fn retain(&mut self, mut f: impl FnMut(&K, &mut V) -> bool) {
        self.v.retain(|(k, v)| f(k, v));
    }
}

impl<K: Ord, V> SideTable<K, V> {
    #[inline]
    fn find(&self, k: &K) -> Result<usize, usize> {
        let s = self.v.as_slice();
        // the common case while recording: no entry yet, or a key after every present one
        match s.last() {
            None => return Err(0),
            Some((last, _)) if *k > *last => return Err(s.len()),
            _ => {}
        }
        s.binary_search_by(|(x, _)| x.cmp(k))
    }
    #[inline]
    pub fn get(&self, k: &K) -> Option<&V> {
        match self.find(k) {
            Ok(i) => Some(&self.v[i].1),
            Err(_) => None,
        }
    }
    #[inline]
    pub fn get_mut(&mut self, k: &K) -> Option<&mut V> {
        match self.find(k) {
            Ok(i) => Some(&mut self.v[i].1),
            Err(_) => None,
        }
    }
    #[inline]
    pub fn contains_key(&self, k: &K) -> bool {
        self.find(k).is_ok()
    }
    /// Inserts or replaces; returns the replaced value.
    pub fn insert(&mut self, k: K, v: V) -> Option<V> {
        match self.find(&k) {
            Ok(i) => Some(std::mem::replace(&mut self.v[i].1, v)),
            Err(i) => {
                self.v.insert(i, (k, v));
                None
            }
        }
    }
    pub fn remove(&mut self, k: &K) -> Option<V> {
        let i = self.find(k).ok()?;
        Some(self.v.remove(i).1)
    }
    /// The value at `k`, inserted with `f` when missing.
    pub fn get_or_insert_with(&mut self, k: K, f: impl FnOnce() -> V) -> &mut V {
        let i = match self.find(&k) {
            Ok(i) => i,
            Err(i) => {
                self.v.insert(i, (k, f()));
                i
            }
        };
        &mut self.v[i].1
    }
}

impl<K: Clone, V: Clone> Clone for SideTable<K, V> {
    fn clone(&self) -> Self {
        SideTable { v: self.v.clone() }
    }
}

impl<K, V> Default for SideTable<K, V> {
    fn default() -> Self {
        Self::new()
    }
}

impl<K: std::fmt::Debug, V: std::fmt::Debug> std::fmt::Debug for SideTable<K, V> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_map().entries(self.iter()).finish()
    }
}

impl<K: Ord, V> std::ops::Index<&K> for SideTable<K, V> {
    type Output = V;
    #[inline]
    fn index(&self, k: &K) -> &V {
        self.get(k).expect("side table: no entry")
    }
}

impl<K: Ord, V> FromIterator<(K, V)> for SideTable<K, V> {
    fn from_iter<I: IntoIterator<Item = (K, V)>>(iter: I) -> Self {
        let mut t = Self::new();
        for (k, v) in iter {
            t.insert(k, v);
        }
        t
    }
}

impl<'a, K, V> IntoIterator for &'a SideTable<K, V> {
    type Item = (&'a K, &'a V);
    type IntoIter = std::iter::Map<std::slice::Iter<'a, (K, V)>, fn(&'a (K, V)) -> (&'a K, &'a V)>;
    fn into_iter(self) -> Self::IntoIter {
        self.v.iter().map(|(k, v)| (k, v))
    }
}

impl<K, V> IntoIterator for SideTable<K, V> {
    type Item = (K, V);
    type IntoIter = std::vec::IntoIter<(K, V)>;
    fn into_iter(mut self) -> Self::IntoIter {
        self.v.take_vec().into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::{BitSet, SideTable, ThinVec};

    #[test]
    fn insert_get_remove_order() {
        let mut t: SideTable<u32, String> = SideTable::new();
        assert!(t.is_empty() && t.get(&3).is_none() && t.heap_bytes() == 0);
        for k in [5u32, 1, 9, 3, 7, 2, 8, 0, 6, 4] {
            assert!(t.insert(k, k.to_string()).is_none());
        }
        assert_eq!(t.insert(3, "x".into()).as_deref(), Some("3"));
        assert_eq!(t.keys().copied().collect::<Vec<_>>(), (0..10).collect::<Vec<_>>());
        assert_eq!(t[&3], "x");
        assert_eq!(t.remove(&0).as_deref(), Some("0"));
        assert!(t.remove(&0).is_none());
        t.retain(|k, _| k % 2 == 1);
        assert_eq!(t.keys().copied().collect::<Vec<_>>(), vec![1, 3, 5, 7, 9]);
        let c = t.clone();
        t.shrink_to_fit();
        assert_eq!(t.len(), 5);
        assert_eq!(c.values().cloned().collect::<Vec<_>>(), t.values().cloned().collect::<Vec<_>>());
        t.get_or_insert_with(4, String::new).push('y');
        assert_eq!(t[&4], "y");
        assert_eq!(t.clone().into_iter().map(|(k, _)| k).collect::<Vec<_>>(), vec![1, 3, 4, 5, 7, 9]);
        t.clear();
        assert!(t.is_empty());
        t.shrink_to_fit();
        assert_eq!(t.heap_bytes(), 0);
    }

    #[test]
    fn unit_values_and_wide_keys() {
        let mut t: SideTable<(u32, usize), ()> = SideTable::new();
        for i in (0..100u32).rev() {
            t.insert((i, 1), ());
        }
        assert_eq!(t.len(), 100);
        assert!(t.contains_key(&(42, 1)) && !t.contains_key(&(42, 0)));
        assert_eq!(std::mem::size_of::<SideTable<u32, ()>>(), 8);
        assert_eq!(std::mem::size_of::<ThinVec<String>>(), 8);
    }

    #[test]
    fn thin_vec_ops() {
        let mut v: ThinVec<String> = ThinVec::new();
        for i in 0..20 {
            v.push(i.to_string());
        }
        v.retain(|s| s.len() == 1);
        assert_eq!(v.len(), 10);
        assert_eq!(v.remove(0), "0");
        v.insert(0, "a".into());
        v.truncate(3);
        assert_eq!(v.to_vec(), vec!["a", "1", "2"]);
        let w: ThinVec<String> = Vec::from(["x".to_string()]).into();
        assert_eq!(w[0], "x");
        assert_eq!(v.pop().as_deref(), Some("2"));
        v.shrink_to_fit();
        assert_eq!(v.clone(), v);
        assert_eq!(v.into_iter().collect::<Vec<_>>(), vec!["a", "1"]);
        let mut b = BitSet::new(130);
        b.set(0);
        b.set(129);
        assert!(b.get(0) && b.get(129) && !b.get(64));
        assert_eq!(b.iter().filter(|x| *x).count(), 2);
    }
}
