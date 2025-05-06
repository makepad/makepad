use std::{
    cmp::Ordering,
    fmt,
    hash::{Hash, Hasher},
    iter::Extend,
    mem,
    mem::{ManuallyDrop, MaybeUninit},
    ops::{Bound, Deref, DerefMut, RangeBounds},
    ptr,
    ptr::NonNull,
    slice,
    slice::Iter,
};

pub struct ArrayVec<T, const N: usize> {
    len: usize,
    values: [MaybeUninit<T>; N],
}

impl<T, const N: usize> ArrayVec<T, N> {
    pub fn new() -> Self {
        Self {
            len: 0,
            values: unsafe { MaybeUninit::uninit().assume_init() },
        }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn capacity(&self) -> usize {
        N
    }

    pub fn as_ptr(&self) -> *const T {
        self.values.as_ptr() as _
    }

    pub fn as_slice(&self) -> &[T] {
        self
    }

    pub unsafe fn set_len(&mut self, len: usize) {
        self.len = len;
    }

    pub fn as_mut_ptr(&mut self) -> *mut T {
        self.values.as_mut_ptr() as _
    }

    pub fn as_mut_slice(&mut self) -> &mut [T] {
        self
    }

    pub fn push(&mut self, value: T) {
        self.try_push(value).unwrap()
    }

    pub fn try_push(&mut self, value: T) -> Result<(), CapacityError> {
        if self.len() == self.capacity() {
            return Err(CapacityError);
        }
        Ok(unsafe {
            self.push_unchecked(value);
        })
    }

    unsafe fn push_unchecked(&mut self, item: T) {
        let len = self.len();
        ptr::write(self.as_mut_ptr().add(len), item);
        self.set_len(len + 1);
    }

    pub fn insert(&mut self, index: usize, value: T) {
        self.try_insert(index, value).unwrap()
    }

    pub fn try_insert(&mut self, index: usize, value: T) -> Result<(), CapacityError> {
        assert!(index <= self.len());
        if self.len() == self.capacity() {
            return Err(CapacityError);
        }
        Ok(unsafe {
            self.insert_unchecked(index, value);
        })
    }

    unsafe fn insert_unchecked(&mut self, index: usize, value: T) {
        let len = self.len();
        let ptr = self.as_mut_ptr().add(index);
        ptr::copy(ptr, ptr.add(1), len - index);
        ptr::write(ptr, value);
        self.set_len(len + 1);
    }

    pub fn extend_from_slice(&mut self, slice: &[T]) {
        self.try_extend_from_slice(slice).unwrap()
    }

    pub fn try_extend_from_slice(&mut self, slice: &[T]) -> Result<(), CapacityError> {
        if slice.len() > self.capacity() - self.len() {
            return Err(CapacityError);
        }
        Ok(unsafe { self.extend_from_slice_unchecked(slice) })
    }

    unsafe fn extend_from_slice_unchecked(&mut self, slice: &[T]) {
        let slice_len = slice.len();
        let len = self.len();
        let src = slice.as_ptr();
        let dst = self.as_mut_ptr().add(len);
        ptr::copy_nonoverlapping(src, dst, slice_len);
        self.set_len(len + slice_len);
    }

    pub fn pop(&mut self) -> Option<T> {
        if self.len() == 0 {
            return None;
        }
        Some(unsafe { self.pop_unchecked() })
    }

    pub unsafe fn pop_unchecked(&mut self) -> T {
        let new_len = self.len() - 1;
        self.set_len(new_len);
        ptr::read(self.as_ptr().add(new_len))
    }

    pub fn remove(&mut self, index: usize) -> T {
        assert!(index < self.len());
        unsafe { self.remove_unchecked(index) }
    }

    pub unsafe fn remove_unchecked(&mut self, index: usize) -> T {
        let new_len = self.len() - 1;
        self.set_len(new_len);
        let ptr = self.as_mut_ptr().add(index);
        let value = ptr::read(ptr);
        ptr::copy(ptr.add(1), ptr, new_len - index);
        value
    }

    pub fn splice<I>(
        &mut self,
        range: impl RangeBounds<usize>,
        replace_with: I,
    ) -> Splice<'_, T, N, <I as IntoIterator>::IntoIter>
    where
        I: IntoIterator<Item = T>,
    {
        Splice {
            drain: self.drain(range),
            replace_with: replace_with.into_iter(),
        }
    }

    pub fn drain(&mut self, range: impl RangeBounds<usize>) -> Drain<'_, T, N> {
        let start = match range.start_bound() {
            Bound::Included(&start) => start,
            Bound::Excluded(&start) => start.checked_add(1).unwrap(),
            Bound::Unbounded => 0,
        };
        let end = match range.end_bound() {
            Bound::Excluded(&end) => end,
            Bound::Included(&end) => end.checked_add(1).unwrap(),
            Bound::Unbounded => self.len(),
        };
        assert!(start <= end);
        assert!(end <= self.len());
        let len = self.len();
        unsafe {
            self.set_len(start);
            Drain {
                tail_start: end,
                tail_len: len - end,
                iter: slice::from_raw_parts(self.as_ptr().add(start), end - start).iter(),
                array_vec: NonNull::from(self),
            }
        }
    }

    pub fn truncate(&mut self, new_len: usize) {
        if new_len > self.len() {
            return;
        }
        unsafe {
            self.truncate_unchecked(new_len);
        }
    }

    unsafe fn truncate_unchecked(&mut self, new_len: usize) {
        let len = self.len();
        self.set_len(new_len);
        ptr::drop_in_place(slice::from_raw_parts_mut(
            self.as_mut_ptr().add(new_len),
            len - new_len,
        ));
    }

    pub fn clear(&mut self) {
        unsafe {
            self.set_len(0);
            ptr::drop_in_place(self.as_mut_slice());
        }
    }

    pub fn split_off(&mut self, index: usize) -> Self {
        assert!(index <= self.len());
        unsafe { self.split_off_unchecked(index) }
    }

    unsafe fn split_off_unchecked(&mut self, index: usize) -> Self {
        let other_len = self.len() - index;
        let mut other = Self::new();
        ptr::copy_nonoverlapping(self.as_ptr().add(index), other.as_mut_ptr(), other_len);
        self.set_len(index);
        other.set_len(other_len);
        other
    }
}

impl<T, const N: usize> Clone for ArrayVec<T, N>
where
    T: Clone,
{
    fn clone(&self) -> Self {
        self.iter().cloned().collect()
    }
}

impl<T, const CAP: usize> fmt::Debug for ArrayVec<T, CAP>
where
    T: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        (**self).fmt(f)
    }
}

impl<T, const N: usize> Default for ArrayVec<T, N> {
    fn default() -> ArrayVec<T, N> {
        ArrayVec::new()
    }
}

impl<T, const N: usize> Deref for ArrayVec<T, N> {
    type Target = [T];

    fn deref(&self) -> &Self::Target {
        unsafe { slice::from_raw_parts(self.as_ptr(), self.len()) }
    }
}

impl<T, const N: usize> DerefMut for ArrayVec<T, N> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        unsafe { slice::from_raw_parts_mut(self.as_mut_ptr(), self.len()) }
    }
}

impl<T, const N: usize> Extend<T> for ArrayVec<T, N> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for value in iter {
            self.push(value);
        }
    }
}

impl<T, const N: usize> From<[T; N]> for ArrayVec<T, N> {
    fn from(array: [T; N]) -> Self {
        let array = ManuallyDrop::new(array);
        let mut array_vec = ArrayVec::<_, N>::new();
        unsafe {
            ptr::copy_nonoverlapping(array.as_ptr(), array_vec.as_mut_ptr(), N);
            array_vec.set_len(N);
            array_vec
        }
    }
}

impl<T, const N: usize> FromIterator<T> for ArrayVec<T, N> {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        let mut array_vec = ArrayVec::<_, N>::new();
        array_vec.extend(iter);
        array_vec
    }
}

impl<T, const N: usize> Eq for ArrayVec<T, N> where T: Eq {}

impl<T, const N: usize> Hash for ArrayVec<T, N>
where
    T: Hash,
{
    fn hash<H: Hasher>(&self, state: &mut H) {
        (**self).hash(state)
    }
}

impl<'a, T, const N: usize> IntoIterator for &'a ArrayVec<T, N> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

impl<'a, T, const N: usize> IntoIterator for &'a mut ArrayVec<T, N> {
    type Item = &'a mut T;
    type IntoIter = slice::IterMut<'a, T>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter_mut()
    }
}

impl<T, const N: usize> PartialEq for ArrayVec<T, N>
where
    T: PartialEq,
{
    fn eq(&self, other: &Self) -> bool {
        **self == **other
    }
}

impl<T, const N: usize> Ord for ArrayVec<T, N>
where
    T: Ord,
{
    fn cmp(&self, other: &Self) -> Ordering {
        (**self).cmp(other)
    }
}

impl<T, const CAP: usize> PartialOrd for ArrayVec<T, CAP>
where
    T: PartialOrd,
{
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        (**self).partial_cmp(other)
    }
}

impl<T, const N: usize> TryFrom<&[T]> for ArrayVec<T, N> {
    type Error = CapacityError;

    fn try_from(slice: &[T]) -> Result<Self, Self::Error> {
        let mut array_vec: ArrayVec<T, N> = ArrayVec::<_, N>::new();
        array_vec.try_extend_from_slice(slice)?;
        Ok(array_vec)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CapacityError;

#[derive(Debug)]
pub struct Splice<'a, T, const N: usize, I>
where
    I: Iterator<Item = T>,
{
    drain: Drain<'a, T, N>,
    replace_with: I,
}

impl<T, const N: usize, I> Drop for Splice<'_, T, N, I>
where
    I: Iterator<Item = T>,
{
    fn drop(&mut self) {
        while let Some(_) = self.drain.next() {}
        unsafe {
            if self.drain.tail_len == 0 {
                self.drain.array_vec.as_mut().extend(&mut self.replace_with);
                return;
            }
            if !self.drain.fill(&mut self.replace_with) {
                return;
            }
            let (lower_bound, _) = self.replace_with.size_hint();
            if lower_bound > 0 {
                self.drain.move_tail(lower_bound);
                if !self.drain.fill(&mut self.replace_with) {
                    return;
                }
            }
            let mut replace_with = self.replace_with.by_ref().collect::<Vec<_>>().into_iter();
            if replace_with.len() > 0 {
                self.drain.move_tail(replace_with.len());
                self.drain.fill(&mut replace_with);
            }
        }
    }
}

impl<T, const N: usize, I> Iterator for Splice<'_, T, N, I>
where
    I: Iterator<Item = T>,
{
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.drain.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.drain.size_hint()
    }
}

impl<T, const N: usize, I> DoubleEndedIterator for Splice<'_, T, N, I>
where
    I: Iterator<Item = T>,
{
    fn next_back(&mut self) -> Option<Self::Item> {
        self.drain.next_back()
    }
}

#[derive(Debug)]
pub struct Drain<'a, T, const N: usize> {
    iter: Iter<'a, T>,
    array_vec: NonNull<ArrayVec<T, N>>,
    tail_start: usize,
    tail_len: usize,
}

impl<T, const N: usize> Drain<'_, T, N> {
    unsafe fn fill(&mut self, replace_with: &mut impl Iterator<Item = T>) -> bool {
        let array_vec = self.array_vec.as_mut();
        while array_vec.len() < self.tail_start {
            let Some(value) = replace_with.next() else {
                return false;
            };
            array_vec.push_unchecked(value);
        }
        true
    }

    unsafe fn move_tail(&mut self, additional: usize) {
        let array_vec = self.array_vec.as_mut();
        let len = self.tail_start + self.tail_len;
        assert!(additional <= array_vec.capacity() - len);
        let new_tail_start = self.tail_start + additional;
        let src = array_vec.as_ptr().add(self.tail_start);
        let dst = array_vec.as_mut_ptr().add(new_tail_start);
        ptr::copy(src, dst, self.tail_len);
        self.tail_start = new_tail_start;
    }
}

impl<T, const N: usize> Iterator for Drain<'_, T, N> {
    type Item = T;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter
            .next()
            .map(|item| unsafe { ptr::read(item as *const _) })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<T, const N: usize> DoubleEndedIterator for Drain<'_, T, N> {
    #[inline]
    fn next_back(&mut self) -> Option<T> {
        self.iter
            .next_back()
            .map(|item| unsafe { ptr::read(item as *const _) })
    }
}

impl<T, const N: usize> Drop for Drain<'_, T, N> {
    fn drop(&mut self) {
        struct Guard<'a, 'b, T, const N: usize>(&'a mut Drain<'b, T, N>);

        impl<'a, 'b, T, const N: usize> Drop for Guard<'a, 'b, T, N> {
            fn drop(&mut self) {
                if self.0.tail_len == 0 {
                    return;
                }
                unsafe {
                    let array_vec = self.0.array_vec.as_mut();
                    let tail_start = self.0.tail_start;
                    let new_tail_start = array_vec.len();
                    if tail_start != new_tail_start {
                        let src = array_vec.as_ptr().add(tail_start);
                        let dst = array_vec.as_mut_ptr().add(new_tail_start);
                        ptr::copy(src, dst, self.0.tail_len);
                    }
                    array_vec.set_len(new_tail_start + self.0.tail_len);
                }
            }
        }

        let iter = mem::take(&mut self.iter);
        let slice_ptr = iter.as_slice().as_ptr();
        let slice_len = iter.len();
        let mut array_vec = self.array_vec;
        let _guard = Guard(self);
        if slice_len == 0 {
            return;
        }
        unsafe {
            let array_vec_ptr = array_vec.as_mut().as_mut_ptr();
            ptr::drop_in_place(ptr::slice_from_raw_parts_mut(
                array_vec_ptr.offset(slice_ptr.offset_from(array_vec_ptr)),
                slice_len,
            ));
        }
    }
}
