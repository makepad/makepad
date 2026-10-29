// arrayvec 0.7.6 src/array_string.rs: fixed-capacity UTF-8, with length in bytes.
use core::borrow::{Borrow, BorrowMut};
use core::cmp;
use core::convert::TryFrom;
use core::fmt;
use core::hash::{Hash, Hasher};
use core::mem::{self, MaybeUninit};
use core::ops::{Deref, DerefMut};
use core::ptr;
use core::slice;
use core::str::{self, FromStr, Utf8Error};
use std::path::Path;

use super::{CapacityError, LenUint};

#[derive(Copy)]
#[repr(C)]
pub struct ArrayString<const CAP: usize> {
    len: LenUint,
    xs: [MaybeUninit<u8>; CAP],
}

impl<const CAP: usize> ArrayString<CAP> {
    pub fn new() -> Self {
        Self::new_const()
    }

    pub const fn new_const() -> Self {
        // arrayvec src/lib.rs: assert_capacity_limit / assert_capacity_limit_const.
        if mem::size_of::<usize>() > mem::size_of::<LenUint>() && CAP > LenUint::MAX as usize {
            panic!("ArrayVec: largest supported capacity is u32::MAX")
        }
        ArrayString { len: 0, xs: [MaybeUninit::uninit(); CAP] }
    }

    pub const fn len(&self) -> usize {
        self.len as usize
    }

    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn from(s: &str) -> Result<Self, CapacityError<&str>> {
        let mut string = Self::new();
        string.try_push_str(s)?;
        Ok(string)
    }

    pub fn from_byte_string(b: &[u8; CAP]) -> Result<Self, Utf8Error> {
        str::from_utf8(b)?;
        let mut string = Self::new();
        unsafe {
            ptr::copy_nonoverlapping(b.as_ptr(), string.as_mut_ptr(), CAP);
            string.set_len(CAP);
        }
        Ok(string)
    }

    pub fn zero_filled() -> Self {
        let mut string = Self::new();
        string.xs = [MaybeUninit::new(0); CAP];
        string.len = CAP as LenUint;
        string
    }

    pub const fn capacity(&self) -> usize {
        CAP
    }

    pub const fn is_full(&self) -> bool {
        self.len() == self.capacity()
    }

    pub const fn remaining_capacity(&self) -> usize {
        self.capacity() - self.len()
    }

    #[track_caller]
    pub fn push(&mut self, c: char) {
        self.try_push(c).unwrap();
    }

    pub fn try_push(&mut self, c: char) -> Result<(), CapacityError<char>> {
        if c.len_utf8() > self.remaining_capacity() {
            return Err(CapacityError::new(c));
        }
        // rustc library/core/src/char/methods.rs: char::encode_utf8; encode only after checking capacity.
        let mut bytes = [0u8; 4];
        let s = c.encode_utf8(&mut bytes);
        unsafe {
            let len = self.len();
            ptr::copy_nonoverlapping(s.as_ptr(), self.as_mut_ptr().add(len), s.len());
            self.set_len(len + s.len());
        }
        Ok(())
    }

    #[track_caller]
    pub fn push_str(&mut self, s: &str) {
        self.try_push_str(s).unwrap()
    }

    pub fn try_push_str<'a>(&mut self, s: &'a str) -> Result<(), CapacityError<&'a str>> {
        if s.len() > self.remaining_capacity() {
            return Err(CapacityError::new(s));
        }
        unsafe {
            let len = self.len();
            ptr::copy_nonoverlapping(s.as_ptr(), self.as_mut_ptr().add(len), s.len());
            self.set_len(len + s.len());
        }
        Ok(())
    }

    pub fn pop(&mut self) -> Option<char> {
        let ch = self.chars().next_back()?;
        unsafe { self.set_len(self.len() - ch.len_utf8()) };
        Some(ch)
    }

    pub fn truncate(&mut self, new_len: usize) {
        if new_len <= self.len() {
            assert!(self.is_char_boundary(new_len));
            unsafe { self.set_len(new_len) };
        }
    }

    pub fn remove(&mut self, idx: usize) -> char {
        let ch = match self[idx..].chars().next() {
            Some(ch) => ch,
            None => panic!("cannot remove a char from the end of a string"),
        };
        let next = idx + ch.len_utf8();
        let len = self.len();
        let ptr = self.as_mut_ptr();
        unsafe {
            ptr::copy(ptr.add(next), ptr.add(idx), len - next);
            self.set_len(len - (next - idx));
        }
        ch
    }

    pub fn clear(&mut self) {
        unsafe { self.set_len(0) };
    }

    /// The first `length` bytes must be initialized UTF-8 and `length <= CAP`.
    pub unsafe fn set_len(&mut self, length: usize) {
        debug_assert!(length <= self.capacity());
        self.len = length as LenUint;
    }

    pub fn as_str(&self) -> &str {
        self
    }

    pub fn as_mut_str(&mut self) -> &mut str {
        self
    }

    pub fn as_ptr(&self) -> *const u8 {
        self.xs.as_ptr() as *const u8
    }

    pub fn as_mut_ptr(&mut self) -> *mut u8 {
        self.xs.as_mut_ptr() as *mut u8
    }
}

impl<const CAP: usize> Deref for ArrayString<CAP> {
    type Target = str;
    fn deref(&self) -> &str {
        unsafe { str::from_utf8_unchecked(slice::from_raw_parts(self.as_ptr(), self.len())) }
    }
}

impl<const CAP: usize> DerefMut for ArrayString<CAP> {
    fn deref_mut(&mut self) -> &mut str {
        let len = self.len();
        unsafe { str::from_utf8_unchecked_mut(slice::from_raw_parts_mut(self.as_mut_ptr(), len)) }
    }
}

impl<const CAP: usize> Default for ArrayString<CAP> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const CAP: usize> Clone for ArrayString<CAP> {
    fn clone(&self) -> Self {
        *self
    }
    fn clone_from(&mut self, rhs: &Self) {
        self.clear();
        self.try_push_str(rhs).ok();
    }
}

impl<const CAP: usize> PartialEq for ArrayString<CAP> {
    fn eq(&self, rhs: &Self) -> bool {
        **self == **rhs
    }
}

impl<const CAP: usize> PartialEq<str> for ArrayString<CAP> {
    fn eq(&self, rhs: &str) -> bool {
        &**self == rhs
    }
}

impl<const CAP: usize> PartialEq<ArrayString<CAP>> for str {
    fn eq(&self, rhs: &ArrayString<CAP>) -> bool {
        self == &**rhs
    }
}

impl<const CAP: usize> Eq for ArrayString<CAP> {}

impl<const CAP: usize> PartialOrd for ArrayString<CAP> {
    fn partial_cmp(&self, rhs: &Self) -> Option<cmp::Ordering> {
        (**self).partial_cmp(&**rhs)
    }
}

impl<const CAP: usize> PartialOrd<str> for ArrayString<CAP> {
    fn partial_cmp(&self, rhs: &str) -> Option<cmp::Ordering> {
        (**self).partial_cmp(rhs)
    }
}

impl<const CAP: usize> PartialOrd<ArrayString<CAP>> for str {
    fn partial_cmp(&self, rhs: &ArrayString<CAP>) -> Option<cmp::Ordering> {
        self.partial_cmp(&**rhs)
    }
}

impl<const CAP: usize> Ord for ArrayString<CAP> {
    fn cmp(&self, rhs: &Self) -> cmp::Ordering {
        (**self).cmp(&**rhs)
    }
}

impl<const CAP: usize> Hash for ArrayString<CAP> {
    fn hash<H: Hasher>(&self, h: &mut H) {
        (**self).hash(h)
    }
}

impl<const CAP: usize> Borrow<str> for ArrayString<CAP> {
    fn borrow(&self) -> &str {
        self
    }
}

impl<const CAP: usize> BorrowMut<str> for ArrayString<CAP> {
    fn borrow_mut(&mut self) -> &mut str {
        self
    }
}

impl<const CAP: usize> AsRef<str> for ArrayString<CAP> {
    fn as_ref(&self) -> &str {
        self
    }
}

impl<const CAP: usize> AsRef<Path> for ArrayString<CAP> {
    fn as_ref(&self) -> &Path {
        self.as_str().as_ref()
    }
}

impl<const CAP: usize> fmt::Debug for ArrayString<CAP> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<const CAP: usize> fmt::Display for ArrayString<CAP> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<const CAP: usize> fmt::Write for ArrayString<CAP> {
    fn write_char(&mut self, c: char) -> fmt::Result {
        self.try_push(c).map_err(|_| fmt::Error)
    }
    fn write_str(&mut self, s: &str) -> fmt::Result {
        self.try_push_str(s).map_err(|_| fmt::Error)
    }
}

impl<const CAP: usize> FromStr for ArrayString<CAP> {
    type Err = CapacityError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from(s).map_err(CapacityError::simplify)
    }
}

impl<'a, const CAP: usize> TryFrom<&'a str> for ArrayString<CAP> {
    type Error = CapacityError<&'a str>;
    fn try_from(s: &'a str) -> Result<Self, Self::Error> {
        Self::from(s)
    }
}

impl<'a, const CAP: usize> TryFrom<fmt::Arguments<'a>> for ArrayString<CAP> {
    type Error = CapacityError<fmt::Error>;
    fn try_from(args: fmt::Arguments<'a>) -> Result<Self, Self::Error> {
        use fmt::Write;
        let mut string = Self::new();
        string.write_fmt(args).map_err(CapacityError::new)?;
        Ok(string)
    }
}
