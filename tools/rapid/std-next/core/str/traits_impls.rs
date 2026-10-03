//! Trait impls on str (Rapid only: in the rustc shim these are real core's).

use super::{cmp_str, eq_str, StrIndex};
use crate::cmp::Ordering;
use crate::option::Option::{self, Some};

impl PartialEq for str {
    fn eq(&self, other: &str) -> bool {
        eq_str(self, other)
    }
}

impl Eq for str {}

impl PartialOrd for str {
    fn partial_cmp(&self, other: &str) -> Option<Ordering> {
        Some(cmp_str(self, other))
    }
}

impl Ord for str {
    fn cmp(&self, other: &str) -> Ordering {
        cmp_str(self, other)
    }
}

impl<I: StrIndex> crate::ops::Index<I> for str {
    type Output = str;
    #[track_caller]
    fn index(&self, index: I) -> &str {
        index.index_str(self)
    }
}

impl<I: StrIndex> crate::ops::IndexMut<I> for str {
    #[track_caller]
    fn index_mut(&mut self, index: I) -> &mut str {
        index.index_str_mut(self)
    }
}

impl<'a> Default for &'a mut str {
    fn default() -> &'a mut str {
        unsafe { super::from_utf8_unchecked_mut(&mut []) }
    }
}
