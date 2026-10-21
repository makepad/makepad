//! LinkedList: a doubly linked list with owned nodes (rustc alloc/src/collections/linked_list.rs;
//! same public API, nodes are boxed and linked by raw pointers, null for "none").

use crate::boxed::Box;
use crate::cmp::Ordering;
use crate::fmt;
use crate::hash::{Hash, Hasher};
use crate::iter::{FromIterator, FusedIterator};
use crate::marker::PhantomData;
use crate::ptr;

struct Node<T> {
    next: *mut Node<T>,
    prev: *mut Node<T>,
    element: T,
}

pub struct LinkedList<T> {
    head: *mut Node<T>,
    tail: *mut Node<T>,
    len: usize,
    marker: PhantomData<Box<Node<T>>>,
}

unsafe impl<T: Send> Send for LinkedList<T> {}
unsafe impl<T: Sync> Sync for LinkedList<T> {}

pub struct Iter<'a, T: 'a> {
    head: *mut Node<T>,
    tail: *mut Node<T>,
    len: usize,
    marker: PhantomData<&'a Node<T>>,
}

pub struct IterMut<'a, T: 'a> {
    head: *mut Node<T>,
    tail: *mut Node<T>,
    len: usize,
    marker: PhantomData<&'a mut Node<T>>,
}

pub struct IntoIter<T> {
    list: LinkedList<T>,
}

impl<T> LinkedList<T> {
    pub const fn new() -> LinkedList<T> {
        LinkedList { head: ptr::null_mut(), tail: ptr::null_mut(), len: 0, marker: PhantomData }
    }

    /// Moves all elements of `other` to the end of the list; `other` is left empty.
    pub fn append(&mut self, other: &mut LinkedList<T>) {
        if self.tail.is_null() {
            crate::mem::swap(self, other);
        } else if !other.head.is_null() {
            unsafe {
                (*self.tail).next = other.head;
                (*other.head).prev = self.tail;
            }
            self.tail = other.tail;
            self.len += other.len;
            other.head = ptr::null_mut();
            other.tail = ptr::null_mut();
            other.len = 0;
        }
    }

    pub fn iter(&self) -> Iter<'_, T> {
        Iter { head: self.head, tail: self.tail, len: self.len, marker: PhantomData }
    }

    pub fn iter_mut(&mut self) -> IterMut<'_, T> {
        IterMut { head: self.head, tail: self.tail, len: self.len, marker: PhantomData }
    }

    pub fn is_empty(&self) -> bool {
        self.head.is_null()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn clear(&mut self) {
        while self.pop_front_node().is_some() {}
    }

    pub fn contains(&self, x: &T) -> bool
    where
        T: PartialEq<T>,
    {
        self.iter().any(|e| e == x)
    }

    pub fn front(&self) -> Option<&T> {
        if self.head.is_null() {
            None
        } else {
            Some(unsafe { &(*self.head).element })
        }
    }

    pub fn front_mut(&mut self) -> Option<&mut T> {
        if self.head.is_null() {
            None
        } else {
            Some(unsafe { &mut (*self.head).element })
        }
    }

    pub fn back(&self) -> Option<&T> {
        if self.tail.is_null() {
            None
        } else {
            Some(unsafe { &(*self.tail).element })
        }
    }

    pub fn back_mut(&mut self) -> Option<&mut T> {
        if self.tail.is_null() {
            None
        } else {
            Some(unsafe { &mut (*self.tail).element })
        }
    }

    pub fn push_front(&mut self, elt: T) {
        let node = Box::into_raw(Box::new(Node { next: self.head, prev: ptr::null_mut(), element: elt }));
        if self.head.is_null() {
            self.tail = node;
        } else {
            unsafe { (*self.head).prev = node };
        }
        self.head = node;
        self.len += 1;
    }

    pub fn pop_front(&mut self) -> Option<T> {
        match self.pop_front_node() {
            Some(node) => Some(node.element),
            None => None,
        }
    }

    pub fn push_back(&mut self, elt: T) {
        let node = Box::into_raw(Box::new(Node { next: ptr::null_mut(), prev: self.tail, element: elt }));
        if self.tail.is_null() {
            self.head = node;
        } else {
            unsafe { (*self.tail).next = node };
        }
        self.tail = node;
        self.len += 1;
    }

    pub fn pop_back(&mut self) -> Option<T> {
        match self.pop_back_node() {
            Some(node) => Some(node.element),
            None => None,
        }
    }

    /// Splits the list at `at`: returns everything from `at` on, `self` keeps `[0, at)`.
    pub fn split_off(&mut self, at: usize) -> LinkedList<T> {
        let len = self.len;
        if at > len {
            crate::panicking::panic_str("Cannot split off at a nonexistent index");
        }
        if at == 0 {
            return crate::mem::take(self);
        } else if at == len {
            return LinkedList::new();
        }
        // the node before the split point
        let mut before = self.head;
        let mut i = 1;
        while i < at {
            before = unsafe { (*before).next };
            i += 1;
        }
        let after = unsafe { (*before).next };
        unsafe {
            (*before).next = ptr::null_mut();
            (*after).prev = ptr::null_mut();
        }
        let second = LinkedList { head: after, tail: self.tail, len: len - at, marker: PhantomData };
        self.tail = before;
        self.len = at;
        second
    }

    fn pop_front_node(&mut self) -> Option<Box<Node<T>>> {
        if self.head.is_null() {
            return None;
        }
        let node = unsafe { Box::from_raw(self.head) };
        self.head = node.next;
        if self.head.is_null() {
            self.tail = ptr::null_mut();
        } else {
            unsafe { (*self.head).prev = ptr::null_mut() };
        }
        self.len -= 1;
        Some(node)
    }

    fn pop_back_node(&mut self) -> Option<Box<Node<T>>> {
        if self.tail.is_null() {
            return None;
        }
        let node = unsafe { Box::from_raw(self.tail) };
        self.tail = node.prev;
        if self.tail.is_null() {
            self.head = ptr::null_mut();
        } else {
            unsafe { (*self.tail).next = ptr::null_mut() };
        }
        self.len -= 1;
        Some(node)
    }
}

impl<T> Drop for LinkedList<T> {
    fn drop(&mut self) {
        while self.pop_front_node().is_some() {}
    }
}

impl<T> Default for LinkedList<T> {
    fn default() -> LinkedList<T> {
        LinkedList::new()
    }
}

impl<'a, T> Clone for Iter<'a, T> {
    fn clone(&self) -> Iter<'a, T> {
        Iter { head: self.head, tail: self.tail, len: self.len, marker: PhantomData }
    }
}

impl<'a, T> Iterator for Iter<'a, T> {
    type Item = &'a T;
    fn next(&mut self) -> Option<&'a T> {
        if self.len == 0 {
            return None;
        }
        let node = self.head;
        self.len -= 1;
        unsafe {
            self.head = (*node).next;
            Some(&(*node).element)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
    fn last(mut self) -> Option<&'a T> {
        self.next_back()
    }
}

impl<'a, T> DoubleEndedIterator for Iter<'a, T> {
    fn next_back(&mut self) -> Option<&'a T> {
        if self.len == 0 {
            return None;
        }
        let node = self.tail;
        self.len -= 1;
        unsafe {
            self.tail = (*node).prev;
            Some(&(*node).element)
        }
    }
}

impl<'a, T> ExactSizeIterator for Iter<'a, T> {}
impl<'a, T> FusedIterator for Iter<'a, T> {}

impl<'a, T: fmt::Debug> fmt::Debug for Iter<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Iter").field(&self.len).finish()
    }
}

impl<'a, T> Iterator for IterMut<'a, T> {
    type Item = &'a mut T;
    fn next(&mut self) -> Option<&'a mut T> {
        if self.len == 0 {
            return None;
        }
        let node = self.head;
        self.len -= 1;
        unsafe {
            self.head = (*node).next;
            Some(&mut (*node).element)
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len, Some(self.len))
    }
    fn last(mut self) -> Option<&'a mut T> {
        self.next_back()
    }
}

impl<'a, T> DoubleEndedIterator for IterMut<'a, T> {
    fn next_back(&mut self) -> Option<&'a mut T> {
        if self.len == 0 {
            return None;
        }
        let node = self.tail;
        self.len -= 1;
        unsafe {
            self.tail = (*node).prev;
            Some(&mut (*node).element)
        }
    }
}

impl<'a, T> ExactSizeIterator for IterMut<'a, T> {}
impl<'a, T> FusedIterator for IterMut<'a, T> {}

impl<'a, T: fmt::Debug> fmt::Debug for IterMut<'a, T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IterMut").field(&self.len).finish()
    }
}

impl<T> Iterator for IntoIter<T> {
    type Item = T;
    fn next(&mut self) -> Option<T> {
        self.list.pop_front()
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.list.len, Some(self.list.len))
    }
}

impl<T> DoubleEndedIterator for IntoIter<T> {
    fn next_back(&mut self) -> Option<T> {
        self.list.pop_back()
    }
}

impl<T> ExactSizeIterator for IntoIter<T> {}
impl<T> FusedIterator for IntoIter<T> {}

impl<T: fmt::Debug> fmt::Debug for IntoIter<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("IntoIter").field(&self.list).finish()
    }
}

impl<T> FromIterator<T> for LinkedList<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> LinkedList<T> {
        let mut list = LinkedList::new();
        list.extend(iter);
        list
    }
}

impl<T> IntoIterator for LinkedList<T> {
    type Item = T;
    type IntoIter = IntoIter<T>;
    fn into_iter(self) -> IntoIter<T> {
        IntoIter { list: self }
    }
}

impl<'a, T> IntoIterator for &'a LinkedList<T> {
    type Item = &'a T;
    type IntoIter = Iter<'a, T>;
    fn into_iter(self) -> Iter<'a, T> {
        self.iter()
    }
}

impl<'a, T> IntoIterator for &'a mut LinkedList<T> {
    type Item = &'a mut T;
    type IntoIter = IterMut<'a, T>;
    fn into_iter(self) -> IterMut<'a, T> {
        self.iter_mut()
    }
}

impl<T> Extend<T> for LinkedList<T> {
    fn extend<I: IntoIterator<Item = T>>(&mut self, iter: I) {
        for x in iter {
            self.push_back(x);
        }
    }
}

impl<'a, T: 'a + Copy> Extend<&'a T> for LinkedList<T> {
    fn extend<I: IntoIterator<Item = &'a T>>(&mut self, iter: I) {
        for x in iter {
            self.push_back(*x);
        }
    }
}

impl<T: PartialEq> PartialEq for LinkedList<T> {
    fn eq(&self, other: &LinkedList<T>) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}

impl<T: Eq> Eq for LinkedList<T> {}

impl<T: PartialOrd> PartialOrd for LinkedList<T> {
    fn partial_cmp(&self, other: &LinkedList<T>) -> Option<Ordering> {
        self.iter().partial_cmp(other.iter())
    }
}

impl<T: Ord> Ord for LinkedList<T> {
    fn cmp(&self, other: &LinkedList<T>) -> Ordering {
        self.iter().cmp(other.iter())
    }
}

impl<T: Clone> Clone for LinkedList<T> {
    fn clone(&self) -> LinkedList<T> {
        let mut list = LinkedList::new();
        for x in self.iter() {
            list.push_back(x.clone());
        }
        list
    }
}

impl<T: fmt::Debug> fmt::Debug for LinkedList<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

impl<T: Hash> Hash for LinkedList<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_length_prefix(self.len);
        for x in self.iter() {
            x.hash(state);
        }
    }
}

impl<T, const N: usize> From<[T; N]> for LinkedList<T> {
    fn from(arr: [T; N]) -> LinkedList<T> {
        LinkedList::from_iter(arr)
    }
}
