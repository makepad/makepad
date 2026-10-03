//! Collections (std::collections / alloc::collections).

mod avl;
mod raw_table;

pub mod binary_heap;
pub mod btree_map;
pub mod btree_set;
pub mod hash_map;
pub mod hash_set;
pub mod vec_deque;

pub use binary_heap::BinaryHeap;
pub use btree_map::BTreeMap;
pub use btree_set::BTreeSet;
pub use hash_map::HashMap;
pub use hash_set::HashSet;
pub use vec_deque::VecDeque;

use crate::fmt;
use crate::ops::{Bound, RangeBounds};

/// Returned by the `try_reserve` methods (Rapid aborts on allocation failure, so this is
/// only produced for capacity overflow).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct TryReserveError {
    kind: TryReserveErrorKind,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum TryReserveErrorKind {
    CapacityOverflow,
}

impl TryReserveError {
    pub(crate) fn capacity_overflow() -> TryReserveError {
        TryReserveError { kind: TryReserveErrorKind::CapacityOverflow }
    }
    pub fn kind(&self) -> TryReserveErrorKind {
        self.kind.clone()
    }
}

impl fmt::Display for TryReserveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("memory allocation failed because the computed capacity exceeded the collection's maximum")
    }
}

// real std's Error needs real Display in the rustc shim
#[cfg(not(rapid_check))]
impl crate::error::Error for TryReserveError {}

/// Resolves a `usize` range against a length to (start, end), panicking with real core's
/// slice-range messages.
pub(crate) fn resolve_range<R: RangeBounds<usize>>(r: &R, len: usize) -> (usize, usize) {
    let start = match r.start_bound() {
        Bound::Included(s) => *s,
        Bound::Excluded(s) => match s.checked_add(1) {
            Some(v) => v,
            None => crate::panicking::panic_str("attempted to index slice from after maximum usize"),
        },
        Bound::Unbounded => 0,
    };
    let end = match r.end_bound() {
        Bound::Included(e) => match e.checked_add(1) {
            Some(v) => v,
            None => crate::panicking::panic_str("attempted to index slice up to maximum usize"),
        },
        Bound::Excluded(e) => *e,
        Bound::Unbounded => len,
    };
    if start > end {
        crate::panicking::slice_index_order_fail(start, end);
    }
    if end > len {
        crate::panicking::slice_end_index_len_fail(end, len);
    }
    (start, end)
}

/// Formats a key/value pair as a tuple `(k, v)` (map iterators' Debug).
pub(crate) struct KvDebug<'a, K, V>(pub &'a K, pub &'a V);

impl<'a, K: fmt::Debug, V: fmt::Debug> fmt::Debug for KvDebug<'a, K, V> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("").field(self.0).field(self.1).finish()
    }
}
