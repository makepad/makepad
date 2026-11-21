//! Collections (std::collections / alloc::collections).

mod btree_node;
mod raw_table;

pub mod binary_heap;
pub mod btree_map;
pub mod btree_set;
pub mod hash_map;
pub mod hash_set;
pub mod linked_list;
pub mod vec_deque;

pub use binary_heap::BinaryHeap;
pub use btree_map::BTreeMap;
pub use btree_set::BTreeSet;
pub use hash_map::HashMap;
pub use hash_set::HashSet;
pub use linked_list::LinkedList;
pub use vec_deque::VecDeque;

use crate::fmt;
use crate::ops::RangeBounds;

/// Returned by the `try_reserve` methods: the capacity overflowed, or the allocator returned an
/// error (library/alloc/src/collections/mod.rs @1.100.0).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TryReserveError {
    kind: TryReserveErrorKind,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TryReserveErrorKind {
    CapacityOverflow,
    AllocError { layout: crate::heap::Layout, non_exhaustive: () },
}

impl TryReserveError {
    pub(crate) fn capacity_overflow() -> TryReserveError {
        TryReserveError { kind: TryReserveErrorKind::CapacityOverflow }
    }
    pub(crate) fn alloc_error(layout: crate::heap::Layout) -> TryReserveError {
        TryReserveError { kind: TryReserveErrorKind::AllocError { layout, non_exhaustive: () } }
    }
    pub const fn kind(&self) -> TryReserveErrorKind {
        self.kind.clone()
    }
}

impl fmt::Display for TryReserveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("memory allocation failed")?;
        f.write_str(match self.kind {
            TryReserveErrorKind::CapacityOverflow => " because the computed capacity exceeded the collection's maximum",
            TryReserveErrorKind::AllocError { .. } => " because the memory allocator returned an error",
        })
    }
}

impl crate::error::Error for TryReserveError {}

/// rustc's `slice::range`, as the collections call it: (start, end) of a `usize` range against a
/// length, panicking with slice indexing's messages.
#[track_caller]
pub(crate) fn resolve_range<R: RangeBounds<usize>>(r: &R, len: usize) -> (usize, usize) {
    crate::slice::range(r, len)
}

/// Formats a key/value pair as a tuple `(k, v)` (map iterators' Debug).
pub(crate) struct KvDebug<'a, K, V>(pub &'a K, pub &'a V);

impl<'a, K: fmt::Debug, V: fmt::Debug> fmt::Debug for KvDebug<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("").field(self.0).field(self.1).finish()
    }
}
