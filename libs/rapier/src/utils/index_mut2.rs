//! IndexMut2 trait for simultaneously indexing with two distinct indices.

use std::ops::IndexMut;

/// Methods for simultaneously indexing a container with two distinct indices.
pub trait IndexMut2<I>: IndexMut<I> {
    /// Gets mutable references to two distinct elements of the container.
    ///
    /// Panics if `i == j`.
    fn index_mut2(&mut self, i: usize, j: usize) -> (&mut Self::Output, &mut Self::Output);

    /// Gets a mutable reference to one element, and immutable reference to a second one.
    ///
    /// Panics if `i == j`.
    #[inline]
    fn index_mut_const(&mut self, i: usize, j: usize) -> (&mut Self::Output, &Self::Output) {
        let (a, b) = self.index_mut2(i, j);
        (a, &*b)
    }
}

impl<T> IndexMut2<usize> for Vec<T> {
    #[inline]
    fn index_mut2(&mut self, i: usize, j: usize) -> (&mut T, &mut T) {
        assert!(i != j, "Unable to index the same element twice.");
        assert!(i < self.len() && j < self.len(), "Index out of bounds.");

        unsafe {
            let a = &mut *(self.get_unchecked_mut(i) as *mut _);
            let b = &mut *(self.get_unchecked_mut(j) as *mut _);
            (a, b)
        }
    }
}

impl<T> IndexMut2<usize> for [T] {
    #[inline]
    fn index_mut2(&mut self, i: usize, j: usize) -> (&mut T, &mut T) {
        assert!(i != j, "Unable to index the same element twice.");
        assert!(i < self.len() && j < self.len(), "Index out of bounds.");

        unsafe {
            let a = &mut *(self.get_unchecked_mut(i) as *mut _);
            let b = &mut *(self.get_unchecked_mut(j) as *mut _);
            (a, b)
        }
    }
}
