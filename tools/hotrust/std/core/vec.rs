//! Vec<T>: a growable array.

use crate::mem::intrinsics_mem as im;

pub struct Vec<T> {
    ptr: *mut T,
    cap: usize,
    len: usize,
}

impl<T> Vec<T> {
    pub fn new() -> Vec<T> {
        Vec { ptr: unsafe { im::null_mut::<T>() }, cap: 0, len: 0 }
    }
    pub fn with_capacity(n: usize) -> Vec<T> {
        let mut v = Vec::new();
        v.reserve(n);
        v
    }
    pub fn len(&self) -> usize {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn capacity(&self) -> usize {
        self.cap
    }
    pub fn reserve(&mut self, extra: usize) {
        let need = self.len + extra;
        if need <= self.cap {
            return;
        }
        let mut cap = if self.cap == 0 { 4 } else { self.cap * 2 };
        while cap < need {
            cap = cap * 2;
        }
        let sz = crate::mem::size_of::<T>();
        self.ptr = crate::mem::realloc_bytes(self.ptr as *mut u8, self.cap * sz, cap * sz) as *mut T;
        self.cap = cap;
    }
    pub fn push(&mut self, v: T) {
        if self.len == self.cap {
            self.reserve(1);
        }
        unsafe { im::ptr_write(im::ptr_add_mut(self.ptr, self.len), v) };
        self.len += 1;
    }
    pub fn pop(&mut self) -> Option<T> {
        if self.len == 0 {
            return None;
        }
        self.len -= 1;
        Some(unsafe { im::ptr_read(im::ptr_add(self.ptr as *const T, self.len)) })
    }
    pub fn clear(&mut self) {
        let n = self.len;
        self.len = 0;
        let mut i = 0;
        while i < n {
            unsafe { im::drop_in_place(im::ptr_add_mut(self.ptr, i)) };
            i += 1;
        }
    }
    pub fn truncate(&mut self, n: usize) {
        while self.len > n {
            self.len -= 1;
            unsafe { im::drop_in_place(im::ptr_add_mut(self.ptr, self.len)) };
        }
    }
    pub fn as_slice(&self) -> &[T] {
        unsafe { im::slice_from_raw(self.ptr as *const T, self.len) }
    }
    pub fn as_mut_slice(&mut self) -> &mut [T] {
        unsafe { im::slice_from_raw_mut(self.ptr, self.len) }
    }
    pub fn as_ptr(&self) -> *const T {
        self.ptr as *const T
    }
    pub fn get(&self, i: usize) -> Option<&T> {
        if i < self.len {
            Some(unsafe { &*im::ptr_add(self.ptr as *const T, i) })
        } else {
            None
        }
    }
    pub fn get_mut(&mut self, i: usize) -> Option<&mut T> {
        if i < self.len {
            Some(unsafe { &mut *im::ptr_add_mut(self.ptr, i) })
        } else {
            None
        }
    }
    pub fn insert(&mut self, at: usize, v: T) {
        if at > self.len {
            panic!("insertion index is out of bounds");
        }
        self.reserve(1);
        let mut i = self.len;
        while i > at {
            unsafe {
                let x = im::ptr_read(im::ptr_add(self.ptr as *const T, i - 1));
                im::ptr_write(im::ptr_add_mut(self.ptr, i), x);
            }
            i -= 1;
        }
        unsafe { im::ptr_write(im::ptr_add_mut(self.ptr, at), v) };
        self.len += 1;
    }
    pub fn remove(&mut self, at: usize) -> T {
        if at >= self.len {
            panic!("removal index is out of bounds");
        }
        let v = unsafe { im::ptr_read(im::ptr_add(self.ptr as *const T, at)) };
        let mut i = at;
        while i + 1 < self.len {
            unsafe {
                let x = im::ptr_read(im::ptr_add(self.ptr as *const T, i + 1));
                im::ptr_write(im::ptr_add_mut(self.ptr, i), x);
            }
            i += 1;
        }
        self.len -= 1;
        v
    }
}

impl<T> crate::ops::Index<usize> for Vec<T> {
    type Output = T;
    fn index(&self, i: usize) -> &T {
        if i >= self.len {
            panic!("index out of bounds: the len is {} but the index is {}", self.len, i);
        }
        unsafe { &*im::ptr_add(self.ptr as *const T, i) }
    }
}

impl<T> crate::ops::IndexMut<usize> for Vec<T> {
    fn index_mut(&mut self, i: usize) -> &mut T {
        if i >= self.len {
            panic!("index out of bounds: the len is {} but the index is {}", self.len, i);
        }
        unsafe { &mut *im::ptr_add_mut(self.ptr, i) }
    }
}

impl<T> crate::ops::Deref for Vec<T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        unsafe { im::slice_from_raw(self.ptr as *const T, self.len) }
    }
}

impl<T> crate::ops::DerefMut for Vec<T> {
    fn deref_mut(&mut self) -> &mut [T] {
        unsafe { im::slice_from_raw_mut(self.ptr, self.len) }
    }
}

impl<T> crate::ops::Drop for Vec<T> {
    fn drop(&mut self) {
        // elements are dropped front to back, as std does
        let n = self.len;
        self.len = 0;
        let mut i = 0;
        while i < n {
            unsafe { im::drop_in_place(im::ptr_add_mut(self.ptr, i)) };
            i += 1;
        }
        crate::mem::free_bytes(self.ptr as *mut u8, self.cap * crate::mem::size_of::<T>());
    }
}
