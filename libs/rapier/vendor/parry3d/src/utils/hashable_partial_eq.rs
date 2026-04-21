use crate::utils::AsBytes;
use core::hash::{Hash, Hasher};

/// A structure that implements `Eq` and is hashable even if the wrapped data implements only
/// `PartialEq`.
#[derive(PartialEq, Clone, Debug)]
pub struct HashablePartialEq<T> {
    value: T,
}

impl<T> HashablePartialEq<T> {
    /// Creates a new `HashablePartialEq`. Please make sure that you really
    /// want to transform the wrapped object's partial equality to an equivalence relation.
    pub fn new(value: T) -> HashablePartialEq<T> {
        HashablePartialEq { value }
    }

    /// Gets the wrapped value.
    pub fn unwrap(self) -> T {
        self.value
    }
}

impl<T: PartialEq> Eq for HashablePartialEq<T> {}

impl<T: AsBytes> Hash for HashablePartialEq<T> {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write(self.value.as_bytes())
    }
}
