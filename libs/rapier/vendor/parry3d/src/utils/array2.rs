use alloc::{vec, vec::Vec};
use core::ops::{Index, IndexMut};
use num::{Bounded, Zero};

#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[derive(Debug, Clone)]
/// A column-major 2D array.
pub struct Array2<T> {
    nrows: usize,
    ncols: usize,
    data: Vec<T>,
}

impl<T> Array2<T> {
    /// Creates a new 2D array from the given dimensions and data.
    #[inline]
    pub fn new(nrows: usize, ncols: usize, data: Vec<T>) -> Self {
        assert_eq!(data.len(), ncols * nrows);
        Array2 { ncols, nrows, data }
    }

    /// Creates a new 2D array filled with copies of `elt`.
    #[inline]
    pub fn repeat(nrows: usize, ncols: usize, elt: T) -> Self
    where
        T: Copy,
    {
        Array2 {
            nrows,
            ncols,
            data: vec![elt; ncols * nrows],
        }
    }

    /// Creates a new 2D array filled with zeros.
    #[inline]
    pub fn zeros(nrows: usize, ncols: usize) -> Self
    where
        T: Copy + Zero,
    {
        Self::repeat(nrows, ncols, T::zero())
    }

    /// Computes the flat index for the given row and column.
    #[inline]
    pub fn flat_index(&self, i: usize, j: usize) -> usize {
        i + j * self.nrows
    }

    /// Returns a reference to the underlying data.
    #[inline]
    pub fn data(&self) -> &[T] {
        &self.data
    }

    /// Returns a mutable reference to the underlying data.
    #[inline]
    pub fn data_mut(&mut self) -> &mut [T] {
        &mut self.data
    }

    /// Returns the number of rows.
    #[inline]
    pub fn nrows(&self) -> usize {
        self.nrows
    }

    /// Returns the number of columns.
    #[inline]
    pub fn ncols(&self) -> usize {
        self.ncols
    }

    /// Returns the minimum element of the array.
    #[inline]
    pub fn min(&self) -> T
    where
        T: Copy + Bounded + PartialOrd,
    {
        self.data
            .iter()
            .fold(Bounded::max_value(), |a, b| if a < *b { a } else { *b })
    }

    /// Returns the maximum element of the array.
    #[inline]
    pub fn max(&self) -> T
    where
        T: Copy + Bounded + PartialOrd,
    {
        self.data
            .iter()
            .fold(Bounded::min_value(), |a, b| if a > *b { a } else { *b })
    }

    /// Creates a new 2D array from a function.
    #[inline]
    pub fn from_fn(nrows: usize, ncols: usize, f: impl Fn(usize, usize) -> T) -> Self {
        let mut data = Vec::with_capacity(ncols * nrows);

        for j in 0..ncols {
            for i in 0..nrows {
                data.push(f(i, j));
            }
        }
        data.shrink_to_fit();
        Self { nrows, ncols, data }
    }
}

impl<T> Index<(usize, usize)> for Array2<T> {
    type Output = T;

    #[inline]
    fn index(&self, index: (usize, usize)) -> &Self::Output {
        let fid = self.flat_index(index.0, index.1);
        &self.data[fid]
    }
}

impl<T> IndexMut<(usize, usize)> for Array2<T> {
    #[inline]
    fn index_mut(&mut self, index: (usize, usize)) -> &mut Self::Output {
        let fid = self.flat_index(index.0, index.1);
        &mut self.data[fid]
    }
}
