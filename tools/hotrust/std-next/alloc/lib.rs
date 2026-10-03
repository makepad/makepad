//! HotRust's alloc: a facade over HotRust's core (where the alloc-level types live, coord.md
//! S1), at real alloc's paths.

pub mod alloc {
    pub use core::heap::{alloc, alloc_zeroed, dealloc, handle_alloc_error, realloc, GlobalAlloc, Layout, LayoutErr, LayoutError, System};
}

pub mod borrow {
    pub use core::borrow::{Borrow, BorrowMut, Cow, ToOwned};
}

pub mod boxed {
    pub use core::boxed::Box;
}

pub mod collections {
    pub use core::collections::binary_heap;
    pub use core::collections::btree_map;
    pub use core::collections::btree_set;
    pub use core::collections::vec_deque;
    pub use core::collections::BTreeMap;
    pub use core::collections::BTreeSet;
    pub use core::collections::BinaryHeap;
    pub use core::collections::TryReserveError;
    pub use core::collections::VecDeque;
}

/// alloc::ffi (CString lives in core/ffi/c_string.rs; std-os lane)
pub mod ffi {
    pub use core::ffi::{CString, IntoStringError, NulError};
}

pub mod fmt {
    pub use core::fmt::format;
    pub use core::fmt::{
        write, Alignment, Arguments, Binary, Debug, DebugList, DebugMap, DebugSet, DebugStruct, DebugTuple, Display, Error,
        Formatter, LowerExp, LowerHex, Octal, Pointer, Result, UpperExp, UpperHex, Write,
    };
}

pub mod rc {
    pub use core::rc::{Rc, Weak};
}

pub mod slice {
    pub use core::slice::{
        from_mut, from_raw_parts, from_raw_parts_mut, from_ref, Chunks, ChunksExact, ChunksExactMut, ChunksMut, Concat, Iter,
        IterMut, Join, RChunks, RSplit, RSplitN, SliceIndex, Split, SplitInclusive, SplitMut, SplitN, Windows,
    };
}

pub mod str {
    pub use core::str::{
        from_utf8, from_utf8_mut, from_utf8_unchecked, from_utf8_unchecked_mut, pattern, Bytes, CharIndices, Chars,
        EncodeUtf16, EscapeDebug, EscapeDefault, EscapeUnicode, FromStr, Lines, MatchIndices, Matches, ParseBoolError,
        RMatchIndices, RMatches, RSplit, RSplitN, RSplitTerminator, Split, SplitAsciiWhitespace, SplitInclusive, SplitN,
        SplitTerminator, SplitWhitespace, Utf8Chunk, Utf8Chunks, Utf8Error,
    };
}

pub mod string {
    pub use core::string::{Drain, FromUtf16Error, FromUtf8Error, String, ToString};
}

pub mod sync {
    pub use core::sync::{Arc, Weak};
}

pub mod vec {
    pub use core::vec::{from_elem, Drain, IntoIter, Vec};
}
