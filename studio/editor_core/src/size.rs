use std::ops::{Add, AddAssign, Sub, SubAssign};
use crate::makepad_micro_serde::{SerBin, DeBin, DeBinErr};

/// A type for representing an amount of text.
/// 
/// A size is represented as the number of lines one would have to move down, followed by the number
/// of columns one would have to move right to cover the desired amount of text. One consequence of
/// this representation is that adding two sizes is not a commutative operation:
/// 
/// ```
/// use makepad_editor_core::Size;
/// 
/// // Moving 1 line down and 2 columns right, followed by moving 2 lines down and 1 column right,
/// // is the same as moving 3 lines down, and 1 column right.
/// assert_eq!(
///     Size { line: 1, column: 2 } + Size { line: 2, column: 1 },
///     Size { line: 3, column: 1 }
/// );
/// 
/// // Moving 2 lines down and 1 column right, followed by moving 1 line down and 2 columns right,
/// // is the same as moving 3 lines down, and 2 columns right.
/// assert_eq!(
///     Size { line: 2, column: 1 } + Size { line: 1, column: 2 },
///     Size { line: 3, column: 2 }
/// );
/// ```
/// 
/// Another consequence is that a size can be added to an existing position to obtain a new position,
/// but not subtracted. That is, a size represents the forward difference between two positions. On
/// the other hand, a size can be subtracted from an existing size to obtain a new size, as long as
/// the result still represents a forward difference.
/// 
/// ```
/// use makepad_editor_core::Size;
/// 
/// // This is ok!
/// assert_eq!(
///     Size { line: 2, column: 1 } - Size { line: 1, column: 2 },
///     Size { line: 1, column: 1 },
/// );
/// 
/// // This is not ok!
/// // Size { line: 1, column: 2 } - Size { line: 2, column: 1 }
/// ```
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, SerBin, DeBin)]
pub struct Size {
    // The number of lines to move down.
    pub line: u32,
    // The number of columns to move right.
    pub column: u32,
}

impl Size {
    /// Returns an empty amount of text.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::Size;
    /// 
    /// let size = Size::zero();
    /// ```
    pub fn zero() -> Size {
        Size::default()
    }

    /// Returns `true` if this is an empty amount of text.
    /// 
    /// # Examples
    /// 
    /// ```
    /// use makepad_editor_core::Size;
    /// 
    /// let size = Size::zero();
    /// assert!(size.is_zero());
    /// let size = Size { line: 1, column: 1 };
    /// assert!(!size.is_zero());
    /// ```
    pub fn is_zero(self) -> bool {
        self.line == 0 && self.column == 0
    }
}

impl Add for Size {
    type Output = Size;

    /// Returns the sum of this size and the given size.
    fn add(self, other: Size) -> Size {
        if other.line == 0 {
            Size {
                line: self.line,
                column: self.column + other.column,
            }
        } else {
            Size {
                line: self.line + other.line,
                column: other.column,
            }
        }
    }
}

impl AddAssign for Size {
    /// Adds the given size to this size.
    fn add_assign(&mut self, other: Size) {
        *self = *self + other;
    }
}

impl Sub for Size {
    type Output = Size;

    /// Returns the difference between this size and the given size.
    fn sub(self, other: Size) -> Size {
        if self.line == other.line {
            Size {
                line: 0,
                column: self.column - other.column,
            }
        } else {
            Size {
                line: self.line - other.line,
                column: self.column,
            }
        }
    }
}

impl SubAssign for Size {
    /// Subtracts the given size from this size.
    fn sub_assign(&mut self, other: Size) {
        *self = *self - other;
    }
}
