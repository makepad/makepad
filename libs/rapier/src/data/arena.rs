//! Arena adapted from the generational-arena crate.
//!
//! See <https://github.com/fitzgen/generational-arena/blob/master/src/lib.rs>.
//! This has been modified to have a fully deterministic deserialization (including for the order of
//! Index attribution after a deserialization of the arena).
use std::cmp;
use std::iter::{self, Extend, FromIterator, FusedIterator};
use std::mem;
use std::ops;
use std::slice;
use std::vec;

/// The `Arena` allows inserting and removing elements that are referred to by
/// `Index`.
///
/// [See the module-level documentation for example usage and motivation.](./index.html)
#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub struct Arena<T> {
    items: Vec<Entry<T>>,
    generation: u32,
    free_list_head: Option<u32>,
    len: usize,
}

#[derive(Clone, Debug)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
enum Entry<T> {
    Free { next_free: Option<u32> },
    Occupied { generation: u32, value: T },
}

/// An index (and generation) into an `Arena`.
///
/// To get an `Index`, insert an element into an `Arena`, and the `Index` for
/// that element will be returned.
///
/// # Examples
///
/// ```
/// # use rapier3d::data::arena::Arena;
/// let mut arena = Arena::new();
/// let idx = arena.insert(123);
/// assert_eq!(arena[idx], 123);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde-serialize", derive(Serialize, Deserialize))]
pub struct Index {
    index: u32,
    generation: u32,
}

impl Default for Index {
    fn default() -> Self {
        Self::from_raw_parts(crate::INVALID_U32, crate::INVALID_U32)
    }
}

impl Index {
    /// Create a new `Index` from its raw parts.
    ///
    /// The parts must have been returned from an earlier call to
    /// `into_raw_parts`.
    ///
    /// Providing arbitrary values will lead to malformed indices and ultimately
    /// panics.
    pub fn from_raw_parts(index: u32, generation: u32) -> Index {
        Index { index, generation }
    }

    /// Convert this `Index` into its raw parts.
    ///
    /// This niche method is useful for converting an `Index` into another
    /// identifier type. Usually, you should prefer a newtype wrapper around
    /// `Index` like `pub struct MyIdentifier(Index);`.  However, for external
    /// types whose definition you can't customize, but which you can construct
    /// instances of, this method can be useful.
    pub fn into_raw_parts(self) -> (u32, u32) {
        (self.index, self.generation)
    }
}

const DEFAULT_CAPACITY: usize = 4;

impl<T> Default for Arena<T> {
    fn default() -> Arena<T> {
        Arena::new()
    }
}

impl<T> Arena<T> {
    /// Constructs a new, empty `Arena`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::<usize>::new();
    /// # let _ = arena;
    /// ```
    pub fn new() -> Arena<T> {
        Arena::with_capacity(DEFAULT_CAPACITY)
    }

    /// Constructs a new, empty `Arena<T>` with the specified capacity.
    ///
    /// The `Arena<T>` will be able to hold `n` elements without further allocation.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::with_capacity(10);
    ///
    /// // These insertions will not require further allocation.
    /// for i in 0..10 {
    ///     assert!(arena.try_insert(i).is_ok());
    /// }
    ///
    /// // But now we are at capacity, and there is no more room.
    /// assert!(arena.try_insert(99).is_err());
    /// ```
    pub fn with_capacity(n: usize) -> Arena<T> {
        let n = cmp::max(n, 1);
        let mut arena = Arena {
            items: Vec::new(),
            generation: 0,
            free_list_head: None,
            len: 0,
        };
        arena.reserve(n);
        arena
    }

    /// Clear all the items inside the arena, but keep its allocation.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::with_capacity(1);
    /// arena.insert(42);
    /// arena.insert(43);
    ///
    /// arena.clear();
    ///
    /// assert_eq!(arena.capacity(), 2);
    /// ```
    pub fn clear(&mut self) {
        self.items.clear();

        let end = self.items.capacity() as u32;
        self.items.extend((0..end).map(|i| {
            if i == end - 1 {
                Entry::Free { next_free: None }
            } else {
                Entry::Free {
                    next_free: Some(i + 1),
                }
            }
        }));
        self.free_list_head = Some(0);
        self.len = 0;
    }

    /// Attempts to insert `value` into the arena using existing capacity.
    ///
    /// This method will never allocate new capacity in the arena.
    ///
    /// If insertion succeeds, then the `value`'s index is returned. If
    /// insertion fails, then `Err(value)` is returned to give ownership of
    /// `value` back to the caller.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::new();
    ///
    /// match arena.try_insert(42) {
    ///     Ok(idx) => {
    ///         // Insertion succeeded.
    ///         assert_eq!(arena[idx], 42);
    ///     }
    ///     Err(x) => {
    ///         // Insertion failed.
    ///         assert_eq!(x, 42);
    ///     }
    /// };
    /// ```
    #[inline]
    pub fn try_insert(&mut self, value: T) -> Result<Index, T> {
        match self.try_alloc_next_index() {
            None => Err(value),
            Some(index) => {
                self.items[index.index as usize] = Entry::Occupied {
                    generation: self.generation,
                    value,
                };
                Ok(index)
            }
        }
    }

    /// Attempts to insert the value returned by `create` into the arena using existing capacity.
    /// `create` is called with the new value's associated index, allowing values that know their own index.
    ///
    /// This method will never allocate new capacity in the arena.
    ///
    /// If insertion succeeds, then the new index is returned. If
    /// insertion fails, then `Err(create)` is returned to give ownership of
    /// `create` back to the caller.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::{Arena, Index};
    /// let mut arena = Arena::new();
    ///
    /// match arena.try_insert_with(|idx| (42, idx)) {
    ///     Ok(idx) => {
    ///         // Insertion succeeded.
    ///         assert_eq!(arena[idx].0, 42);
    ///         assert_eq!(arena[idx].1, idx);
    ///     }
    ///     Err(x) => {
    ///         // Insertion failed.
    ///     }
    /// };
    /// ```
    #[inline]
    pub fn try_insert_with<F: FnOnce(Index) -> T>(&mut self, create: F) -> Result<Index, F> {
        match self.try_alloc_next_index() {
            None => Err(create),
            Some(index) => {
                self.items[index.index as usize] = Entry::Occupied {
                    generation: self.generation,
                    value: create(index),
                };
                Ok(index)
            }
        }
    }

    #[inline]
    fn try_alloc_next_index(&mut self) -> Option<Index> {
        match self.free_list_head {
            None => None,
            Some(i) => match self.items[i as usize] {
                Entry::Occupied { .. } => panic!("corrupt free list"),
                Entry::Free { next_free } => {
                    self.free_list_head = next_free;
                    self.len += 1;
                    Some(Index {
                        index: i,
                        generation: self.generation,
                    })
                }
            },
        }
    }

    /// Insert `value` into the arena, allocating more capacity if necessary.
    ///
    /// The `value`'s associated index in the arena is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::new();
    ///
    /// let idx = arena.insert(42);
    /// assert_eq!(arena[idx], 42);
    /// ```
    #[inline]
    pub fn insert(&mut self, value: T) -> Index {
        match self.try_insert(value) {
            Ok(i) => i,
            Err(value) => self.insert_slow_path(value),
        }
    }

    /// Insert the value returned by `create` into the arena, allocating more capacity if necessary.
    /// `create` is called with the new value's associated index, allowing values that know their own index.
    ///
    /// The new value's associated index in the arena is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::{Arena, Index};
    /// let mut arena = Arena::new();
    ///
    /// let idx = arena.insert_with(|idx| (42, idx));
    /// assert_eq!(arena[idx].0, 42);
    /// assert_eq!(arena[idx].1, idx);
    /// ```
    #[inline]
    pub fn insert_with(&mut self, create: impl FnOnce(Index) -> T) -> Index {
        match self.try_insert_with(create) {
            Ok(i) => i,
            Err(create) => self.insert_with_slow_path(create),
        }
    }

    #[inline(never)]
    fn insert_slow_path(&mut self, value: T) -> Index {
        let len = self.items.len();
        self.reserve(len);
        self.try_insert(value)
            .map_err(|_| ())
            .expect("inserting will always succeed after reserving additional space")
    }

    #[inline(never)]
    fn insert_with_slow_path(&mut self, create: impl FnOnce(Index) -> T) -> Index {
        let len = self.items.len();
        self.reserve(len);
        self.try_insert_with(create)
            .map_err(|_| ())
            .expect("inserting will always succeed after reserving additional space")
    }

    /// Remove the element at index `i` from the arena.
    ///
    /// If the element at index `i` is still in the arena, then it is
    /// returned. If it is not in the arena, then `None` is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::new();
    /// let idx = arena.insert(42);
    ///
    /// assert_eq!(arena.remove(idx), Some(42));
    /// assert_eq!(arena.remove(idx), None);
    /// ```
    pub fn remove(&mut self, i: Index) -> Option<T> {
        if i.index >= self.items.len() as u32 {
            return None;
        }

        match self.items[i.index as usize] {
            Entry::Occupied { generation, .. } if i.generation == generation => {
                let entry = mem::replace(
                    &mut self.items[i.index as usize],
                    Entry::Free {
                        next_free: self.free_list_head,
                    },
                );
                self.generation += 1;
                self.free_list_head = Some(i.index);
                self.len -= 1;

                match entry {
                    Entry::Occupied {
                        generation: _,
                        value,
                    } => Some(value),
                    _ => unreachable!(),
                }
            }
            _ => None,
        }
    }

    /// Retains only the elements specified by the predicate.
    ///
    /// In other words, remove all indices such that `predicate(index, &value)` returns `false`.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut crew = Arena::new();
    /// crew.extend(&["Jim Hawkins", "John Silver", "Alexander Smollett", "Israel Hands"]);
    /// let pirates = ["John Silver", "Israel Hands"]; // too dangerous to keep them around
    /// crew.retain(|_index, member| !pirates.contains(member));
    /// let mut crew_members = crew.iter().map(|(_, member)| **member);
    /// assert_eq!(crew_members.next(), Some("Jim Hawkins"));
    /// assert_eq!(crew_members.next(), Some("Alexander Smollett"));
    /// assert!(crew_members.next().is_none());
    /// ```
    pub fn retain(&mut self, mut predicate: impl FnMut(Index, &mut T) -> bool) {
        for i in 0..self.capacity() as u32 {
            let remove = match &mut self.items[i as usize] {
                Entry::Occupied { generation, value } => {
                    let index = Index {
                        index: i,
                        generation: *generation,
                    };
                    if predicate(index, value) {
                        None
                    } else {
                        Some(index)
                    }
                }

                _ => None,
            };
            if let Some(index) = remove {
                self.remove(index);
            }
        }
    }

    /// Is the element at index `i` in the arena?
    ///
    /// Returns `true` if the element at `i` is in the arena, `false` otherwise.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::new();
    /// let idx = arena.insert(42);
    ///
    /// assert!(arena.contains(idx));
    /// arena.remove(idx);
    /// assert!(!arena.contains(idx));
    /// ```
    pub fn contains(&self, i: Index) -> bool {
        self.get(i).is_some()
    }

    /// Get a shared reference to the element at index `i` if it is in the
    /// arena.
    ///
    /// If the element at index `i` is not in the arena, then `None` is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::new();
    /// let idx = arena.insert(42);
    ///
    /// assert_eq!(arena.get(idx), Some(&42));
    /// arena.remove(idx);
    /// assert!(arena.get(idx).is_none());
    /// ```
    pub fn get(&self, i: Index) -> Option<&T> {
        match self.items.get(i.index as usize) {
            Some(Entry::Occupied { generation, value }) if *generation == i.generation => {
                Some(value)
            }
            _ => None,
        }
    }

    /// Get an exclusive reference to the element at index `i` if it is in the
    /// arena.
    ///
    /// If the element at index `i` is not in the arena, then `None` is returned.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::new();
    /// let idx = arena.insert(42);
    ///
    /// *arena.get_mut(idx).unwrap() += 1;
    /// assert_eq!(arena.remove(idx), Some(43));
    /// assert!(arena.get_mut(idx).is_none());
    /// ```
    pub fn get_mut(&mut self, i: Index) -> Option<&mut T> {
        match self.items.get_mut(i.index as usize) {
            Some(Entry::Occupied { generation, value }) if *generation == i.generation => {
                Some(value)
            }
            _ => None,
        }
    }

    /// Get a pair of exclusive references to the elements at index `i1` and `i2` if it is in the
    /// arena.
    ///
    /// If the element at index `i1` or `i2` is not in the arena, then `None` is returned for this
    /// element.
    ///
    /// # Panics
    ///
    /// Panics if `i1` and `i2` are pointing to the same item of the arena.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::new();
    /// let idx1 = arena.insert(0);
    /// let idx2 = arena.insert(1);
    ///
    /// {
    ///     let (item1, item2) = arena.get2_mut(idx1, idx2);
    ///
    ///     *item1.unwrap() = 3;
    ///     *item2.unwrap() = 4;
    /// }
    ///
    /// assert_eq!(arena[idx1], 3);
    /// assert_eq!(arena[idx2], 4);
    /// ```
    pub fn get2_mut(&mut self, i1: Index, i2: Index) -> (Option<&mut T>, Option<&mut T>) {
        let len = self.items.len() as u32;

        if i1.index == i2.index {
            assert!(i1.generation != i2.generation);

            if i1.generation > i2.generation {
                return (self.get_mut(i1), None);
            }
            return (None, self.get_mut(i2));
        }

        if i1.index >= len {
            return (None, self.get_mut(i2));
        } else if i2.index >= len {
            return (self.get_mut(i1), None);
        }

        let (raw_item1, raw_item2) = {
            let (xs, ys) = self
                .items
                .split_at_mut(cmp::max(i1.index, i2.index) as usize);
            if i1.index < i2.index {
                (&mut xs[i1.index as usize], &mut ys[0])
            } else {
                (&mut ys[0], &mut xs[i2.index as usize])
            }
        };

        let item1 = match raw_item1 {
            Entry::Occupied { generation, value } if *generation == i1.generation => Some(value),
            _ => None,
        };

        let item2 = match raw_item2 {
            Entry::Occupied { generation, value } if *generation == i2.generation => Some(value),
            _ => None,
        };

        (item1, item2)
    }

    /// Get the length of this arena.
    ///
    /// The length is the number of elements the arena holds.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::new();
    /// assert_eq!(arena.len(), 0);
    ///
    /// let idx = arena.insert(42);
    /// assert_eq!(arena.len(), 1);
    ///
    /// let _ = arena.insert(0);
    /// assert_eq!(arena.len(), 2);
    ///
    /// assert_eq!(arena.remove(idx), Some(42));
    /// assert_eq!(arena.len(), 1);
    /// ```
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns true if the arena contains no elements
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::new();
    /// assert!(arena.is_empty());
    ///
    /// let idx = arena.insert(42);
    /// assert!(!arena.is_empty());
    ///
    /// assert_eq!(arena.remove(idx), Some(42));
    /// assert!(arena.is_empty());
    /// ```
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Get the capacity of this arena.
    ///
    /// The capacity is the maximum number of elements the arena can hold
    /// without further allocation, including however many it currently
    /// contains.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::with_capacity(10);
    /// assert_eq!(arena.capacity(), 10);
    ///
    /// // `try_insert` does not allocate new capacity.
    /// for i in 0..10 {
    ///     assert!(arena.try_insert(1).is_ok());
    ///     assert_eq!(arena.capacity(), 10);
    /// }
    ///
    /// // But `insert` will if the arena is already at capacity.
    /// arena.insert(0);
    /// assert!(arena.capacity() > 10);
    /// ```
    pub fn capacity(&self) -> usize {
        self.items.len()
    }

    /// Allocate space for `additional_capacity` more elements in the arena.
    ///
    /// # Panics
    ///
    /// Panics if this causes the capacity to overflow.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::with_capacity(10);
    /// arena.reserve(5);
    /// assert_eq!(arena.capacity(), 15);
    /// # let _: Arena<usize> = arena;
    /// ```
    pub fn reserve(&mut self, additional_capacity: usize) {
        let start = self.items.len();
        let end = self.items.len() + additional_capacity;
        let old_head = self.free_list_head;
        self.items.reserve_exact(additional_capacity);
        self.items.extend((start..end).map(|i| {
            if i == end - 1 {
                Entry::Free {
                    next_free: old_head,
                }
            } else {
                Entry::Free {
                    next_free: Some(i as u32 + 1),
                }
            }
        }));
        self.free_list_head = Some(start as u32);
    }

    /// Iterate over shared references to the elements in this arena.
    ///
    /// Yields pairs of `(Index, &T)` items.
    ///
    /// Order of iteration is not defined.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::new();
    /// for i in 0..10 {
    ///     arena.insert(i * i);
    /// }
    ///
    /// for (idx, value) in arena.iter() {
    ///     println!("{} is at index {:?}", value, idx);
    /// }
    /// ```
    pub fn iter(&self) -> Iter<'_, T> {
        Iter {
            len: self.len,
            inner: self.items.iter().enumerate(),
        }
    }

    /// Iterate over exclusive references to the elements in this arena.
    ///
    /// Yields pairs of `(Index, &mut T)` items.
    ///
    /// Order of iteration is not defined.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::new();
    /// for i in 0..10 {
    ///     arena.insert(i * i);
    /// }
    ///
    /// for (_idx, value) in arena.iter_mut() {
    ///     *value += 5;
    /// }
    /// ```
    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut {
            len: self.len,
            inner: self.items.iter_mut().enumerate(),
        }
    }

    /// Iterate over elements of the arena and remove them.
    ///
    /// Yields pairs of `(Index, T)` items.
    ///
    /// Order of iteration is not defined.
    ///
    /// Note: All elements are removed even if the iterator is only partially consumed or not consumed at all.
    ///
    /// # Examples
    ///
    /// ```
    /// # use rapier3d::data::arena::Arena;
    /// let mut arena = Arena::new();
    /// let idx_1 = arena.insert("hello");
    /// let idx_2 = arena.insert("world");
    ///
    /// assert!(arena.get(idx_1).is_some());
    /// assert!(arena.get(idx_2).is_some());
    /// for (idx, value) in arena.drain() {
    ///     assert!((idx == idx_1 && value == "hello") || (idx == idx_2 && value == "world"));
    /// }
    /// assert!(arena.get(idx_1).is_none());
    /// assert!(arena.get(idx_2).is_none());
    /// ```
    pub fn drain(&mut self) -> Drain<'_, T> {
        Drain {
            inner: self.items.drain(..).enumerate(),
        }
    }

    /// Given an i of `usize` without a generation, get a shared reference
    /// to the element and the matching `Index` of the entry behind `i`.
    ///
    /// This method is useful when you know there might be an element at the
    /// position i, but don't know its generation or precise Index.
    ///
    /// Use cases include using indexing such as Hierarchical BitMap Indexing or
    /// other kinds of bit-efficient indexing.
    ///
    /// You should use the `get` method instead most of the time.
    pub fn get_unknown_gen(&self, i: u32) -> Option<(&T, Index)> {
        match self.items.get(i as usize) {
            Some(Entry::Occupied { generation, value }) => Some((
                value,
                Index {
                    generation: *generation,
                    index: i,
                },
            )),
            _ => None,
        }
    }

    /// Given an i of `usize` without a generation, get an exclusive reference
    /// to the element and the matching `Index` of the entry behind `i`.
    ///
    /// This method is useful when you know there might be an element at the
    /// position i, but don't know its generation or precise Index.
    ///
    /// Use cases include using indexing such as Hierarchical BitMap Indexing or
    /// other kinds of bit-efficient indexing.
    ///
    /// You should use the `get_mut` method instead most of the time.
    pub fn get_unknown_gen_mut(&mut self, i: u32) -> Option<(&mut T, Index)> {
        match self.items.get_mut(i as usize) {
            Some(Entry::Occupied { generation, value }) => Some((
                value,
                Index {
                    generation: *generation,
                    index: i,
                },
            )),
            _ => None,
        }
    }
}

impl<T> IntoIterator for Arena<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> Self::IntoIter {
        IntoIter {
            len: self.len,
            inner: self.items.into_iter(),
        }
    }
}

/// An iterator over the elements in an arena.
///
/// Yields `T` items.
///
/// Order of iteration is not defined.
///
/// # Examples
///
/// ```
/// # use rapier3d::data::arena::Arena;
/// let mut arena = Arena::new();
/// for i in 0..10 {
///     arena.insert(i * i);
/// }
///
/// for value in arena {
///     assert!(value < 100);
/// }
/// ```
#[derive(Clone, Debug)]
pub struct IntoIter<T> {
    len: usize,
    inner: vec::IntoIter<Entry<T>>,
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next() {
                Some(Entry::Free { .. }) => continue,
                Some(Entry::Occupied { value, .. }) => {
                    self.len -= 1;
                    return Some(value);
                }
                None => {
                    debug_assert_eq!(self.len, 0);
                    return None;
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
}

impl<T> DoubleEndedIterator for IntoIter<T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next_back() {
                Some(Entry::Free { .. }) => continue,
                Some(Entry::Occupied { value, .. }) => {
                    self.len -= 1;
                    return Some(value);
                }
                None => {
                    debug_assert_eq!(self.len, 0);
                    return None;
                }
            }
        }
    }
}

impl<T> ExactSizeIterator for IntoIter<T> {
    fn len(&self) -> usize {
        self.len
    }
}

impl<T> FusedIterator for IntoIter<T> {}

impl<'a, T> IntoIterator for &'a Arena<T> {
    type Item = (Index, &'a T);
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

/// An iterator over shared references to the elements in an arena.
///
/// Yields pairs of `(Index, &T)` items.
///
/// Order of iteration is not defined.
///
/// # Examples
///
/// ```
/// # use rapier3d::data::arena::Arena;
/// let mut arena = Arena::new();
/// for i in 0..10 {
///     arena.insert(i * i);
/// }
///
/// for (idx, value) in &arena {
///     println!("{} is at index {:?}", value, idx);
/// }
/// ```
#[derive(Clone, Debug)]
pub struct Iter<'a, T: 'a> {
    len: usize,
    inner: iter::Enumerate<slice::Iter<'a, Entry<T>>>,
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = (Index, &'a T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next() {
                Some((_, &Entry::Free { .. })) => continue,
                Some((
                    index,
                    &Entry::Occupied {
                        generation,
                        ref value,
                    },
                )) => {
                    self.len -= 1;
                    let idx = Index {
                        index: index as u32,
                        generation,
                    };
                    return Some((idx, value));
                }
                None => {
                    debug_assert_eq!(self.len, 0);
                    return None;
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
}

impl<T> DoubleEndedIterator for Iter<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next_back() {
                Some((_, &Entry::Free { .. })) => continue,
                Some((
                    index,
                    &Entry::Occupied {
                        generation,
                        ref value,
                    },
                )) => {
                    self.len -= 1;
                    let idx = Index {
                        index: index as u32,
                        generation,
                    };
                    return Some((idx, value));
                }
                None => {
                    debug_assert_eq!(self.len, 0);
                    return None;
                }
            }
        }
    }
}

impl<T> ExactSizeIterator for Iter<'_, T> {
    fn len(&self) -> usize {
        self.len
    }
}

impl<T> FusedIterator for Iter<'_, T> {}

impl<'a, T> IntoIterator for &'a mut Arena<T> {
    type Item = (Index, &'a mut T);
    type IntoIter = IterMut<'a, T>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

/// An iterator over exclusive references to elements in this arena.
///
/// Yields pairs of `(Index, &mut T)` items.
///
/// Order of iteration is not defined.
///
/// # Examples
///
/// ```
/// # use rapier3d::data::arena::Arena;
/// let mut arena = Arena::new();
/// for i in 0..10 {
///     arena.insert(i * i);
/// }
///
/// for (_idx, value) in &mut arena {
///     *value += 5;
/// }
/// ```
#[derive(Debug)]
pub struct IterMut<'a, T: 'a> {
    len: usize,
    inner: iter::Enumerate<slice::IterMut<'a, Entry<T>>>,
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = (Index, &'a mut T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next() {
                Some((_, &mut Entry::Free { .. })) => continue,
                Some((
                    index,
                    &mut Entry::Occupied {
                        generation,
                        ref mut value,
                    },
                )) => {
                    self.len -= 1;
                    let idx = Index {
                        index: index as u32,
                        generation,
                    };
                    return Some((idx, value));
                }
                None => {
                    debug_assert_eq!(self.len, 0);
                    return None;
                }
            }
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
}

impl<T> DoubleEndedIterator for IterMut<'_, T> {
    fn next_back(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next_back() {
                Some((_, &mut Entry::Free { .. })) => continue,
                Some((
                    index,
                    &mut Entry::Occupied {
                        generation,
                        ref mut value,
                    },
                )) => {
                    self.len -= 1;
                    let idx = Index {
                        index: index as u32,
                        generation,
                    };
                    return Some((idx, value));
                }
                None => {
                    debug_assert_eq!(self.len, 0);
                    return None;
                }
            }
        }
    }
}

impl<T> ExactSizeIterator for IterMut<'_, T> {
    fn len(&self) -> usize {
        self.len
    }
}

impl<T> FusedIterator for IterMut<'_, T> {}

/// An iterator that removes elements from the arena.
///
/// Yields pairs of `(Index, T)` items.
///
/// Order of iteration is not defined.
///
/// Note: All elements are removed even if the iterator is only partially consumed or not consumed at all.
///
/// # Examples
///
/// ```
/// # use rapier3d::data::arena::Arena;
/// let mut arena = Arena::new();
/// let idx_1 = arena.insert("hello");
/// let idx_2 = arena.insert("world");
///
/// assert!(arena.get(idx_1).is_some());
/// assert!(arena.get(idx_2).is_some());
/// for (idx, value) in arena.drain() {
///     assert!((idx == idx_1 && value == "hello") || (idx == idx_2 && value == "world"));
/// }
/// assert!(arena.get(idx_1).is_none());
/// assert!(arena.get(idx_2).is_none());
/// ```
#[derive(Debug)]
pub struct Drain<'a, T: 'a> {
    inner: iter::Enumerate<vec::Drain<'a, Entry<T>>>,
}

impl<T> Iterator for Drain<'_, T> {
    type Item = (Index, T);

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match self.inner.next() {
                Some((_, Entry::Free { .. })) => continue,
                Some((index, Entry::Occupied { generation, value })) => {
                    let idx = Index {
                        index: index as u32,
                        generation,
                    };
                    return Some((idx, value));
                }
                None => return None,
            }
        }
    }
}

impl<T> Extend<T> for Arena<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for t in iter {
            self.insert(t);
        }
    }
}

impl<T> FromIterator<T> for Arena<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let iter = iter.into_iter();
        let (lower, upper) = iter.size_hint();
        let cap = upper.unwrap_or(lower);
        let cap = cmp::max(cap, 1);
        let mut arena = Arena::with_capacity(cap);
        arena.extend(iter);
        arena
    }
}

impl<T> ops::Index<Index> for Arena<T> {
    type Output = T;

    fn index(&self, index: Index) -> &Self::Output {
        self.get(index).expect("No element at index")
    }
}

impl<T> ops::IndexMut<Index> for Arena<T> {
    fn index_mut(&mut self, index: Index) -> &mut Self::Output {
        self.get_mut(index).expect("No element at index")
    }
}
