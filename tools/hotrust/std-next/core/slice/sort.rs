//! Sorting. Stable sort: merge sort with insertion-sorted small runs (the result of a stable
//! sort is fully determined, so it matches real core). Unstable sort: introsort (quicksort,
//! median of three, insertion sort for small parts, heapsort when the depth limit is hit);
//! the order of elements that compare equal may differ from real core's.

use crate::intrinsics_mem as im;

const SMALL: usize = 20;

fn base_mut<T>(v: &mut [T]) -> *mut T {
    v as *mut [T] as *mut T
}

/// Shifts v[i] left into the sorted prefix v[..i].
fn insert_tail<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], i: usize, is_less: &mut F) {
    unsafe {
        let p = base_mut(v);
        let cur = im::ptr_add_mut(p, i);
        if !is_less(&*cur, &*im::ptr_add(p as *const T, i - 1)) {
            return;
        }
        let tmp = crate::mem::ManuallyDrop::new(im::ptr_read(cur as *const T));
        let mut hole = i;
        im::copy_nonoverlapping(im::ptr_add(p as *const T, i - 1), cur, 1);
        hole -= 1;
        while hole > 0 && is_less(&*tmp, &*im::ptr_add(p as *const T, hole - 1)) {
            im::copy_nonoverlapping(im::ptr_add(p as *const T, hole - 1), im::ptr_add_mut(p, hole), 1);
            hole -= 1;
        }
        im::copy_nonoverlapping(&*tmp as *const T, im::ptr_add_mut(p, hole), 1);
    }
}

pub fn insertion_sort<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) {
    let mut i = 1;
    while i < v.len() {
        insert_tail(v, i, is_less);
        i += 1;
    }
}

/// Stable sort.
pub fn merge_sort<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) {
    let len = v.len();
    if crate::mem::size_of::<T>() == 0 || len < 2 {
        return;
    }
    if len <= SMALL {
        insertion_sort(v, is_less);
        return;
    }
    let mut buf: crate::vec::Vec<T> = crate::vec::Vec::with_capacity(len / 2 + 1);
    let bp = buf.as_mut_ptr();
    sort_rec(v, bp, is_less);
}

fn sort_rec<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], buf: *mut T, is_less: &mut F) {
    let len = v.len();
    if len <= SMALL {
        insertion_sort(v, is_less);
        return;
    }
    let mid = len / 2;
    sort_rec(&mut v[..mid], buf, is_less);
    sort_rec(&mut v[mid..], buf, is_less);
    if !is_less(&v[mid], &v[mid - 1]) {
        return;
    }
    merge(v, mid, buf, is_less);
}

/// Merges the sorted runs v[..mid] and v[mid..]; `buf` has room for `mid` elements.
fn merge<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], mid: usize, buf: *mut T, is_less: &mut F) {
    let len = v.len();
    unsafe {
        let p = base_mut(v);
        im::copy_nonoverlapping(p as *const T, buf, mid);
        let mut i = 0; // in buf
        let mut j = mid; // in v
        let mut k = 0; // out
        while i < mid && j < len {
            let right = im::ptr_add(p as *const T, j);
            let left = im::ptr_add(buf as *const T, i);
            if is_less(&*right, &*left) {
                im::copy_nonoverlapping(right, im::ptr_add_mut(p, k), 1);
                j += 1;
            } else {
                im::copy_nonoverlapping(left, im::ptr_add_mut(p, k), 1);
                i += 1;
            }
            k += 1;
        }
        if i < mid {
            im::copy_nonoverlapping(im::ptr_add(buf as *const T, i), im::ptr_add_mut(p, k), mid - i);
        }
    }
}

/// Unstable sort.
pub fn quicksort<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) {
    if crate::mem::size_of::<T>() == 0 || v.len() < 2 {
        return;
    }
    let mut limit = 0;
    let mut n = v.len();
    while n > 0 {
        limit += 2;
        n >>= 1;
    }
    recurse(v, is_less, limit);
}

fn recurse<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F, limit: u32) {
    let mut v = v;
    let mut limit = limit;
    loop {
        let len = v.len();
        if len <= SMALL {
            insertion_sort(v, is_less);
            return;
        }
        if limit == 0 {
            heapsort(v, is_less);
            return;
        }
        limit -= 1;
        // median of three into v[0]
        let a = 0;
        let b = len / 2;
        let c = len - 1;
        let m = if is_less(&v[a], &v[b]) {
            if is_less(&v[b], &v[c]) {
                b
            } else if is_less(&v[a], &v[c]) {
                c
            } else {
                a
            }
        } else if is_less(&v[a], &v[c]) {
            a
        } else if is_less(&v[b], &v[c]) {
            c
        } else {
            b
        };
        v.swap(0, m);
        let mid = partition(v, is_less);
        let (left, right) = v.split_at_mut(mid);
        let right = &mut right[1..];
        if left.len() < right.len() {
            recurse(left, is_less, limit);
            v = right;
        } else {
            recurse(right, is_less, limit);
            v = left;
        }
    }
}

/// Partitions around the pivot v[0]; returns the pivot's final index.
fn partition<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) -> usize {
    let len = v.len();
    let mut i = 1;
    let mut j = len - 1;
    loop {
        while i <= j && is_less(&v[i], &v[0]) {
            i += 1;
        }
        while i <= j && !is_less(&v[j], &v[0]) {
            if j == 0 {
                break;
            }
            j -= 1;
        }
        if i >= j {
            break;
        }
        v.swap(i, j);
        i += 1;
        j -= 1;
    }
    let p = i - 1;
    v.swap(0, p);
    p
}

pub fn heapsort<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) {
    let len = v.len();
    let mut i = len / 2;
    while i > 0 {
        i -= 1;
        sift_down(v, i, len, is_less);
    }
    let mut end = len;
    while end > 1 {
        end -= 1;
        v.swap(0, end);
        sift_down(v, 0, end, is_less);
    }
}

fn sift_down<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], node: usize, end: usize, is_less: &mut F) {
    let mut node = node;
    loop {
        let mut child = 2 * node + 1;
        if child >= end {
            break;
        }
        if child + 1 < end && is_less(&v[child], &v[child + 1]) {
            child += 1;
        }
        if !is_less(&v[node], &v[child]) {
            break;
        }
        v.swap(node, child);
        node = child;
    }
}

/// Reorders so that v[index] is the element a full sort would put there, everything before
/// it is <= and everything after it is >=.
pub fn select_nth<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], index: usize, is_less: &mut F) {
    if index >= v.len() {
        crate::panicking::panic_fmt(format_args!(
            "partition_at_index index {} greater than length of slice {}",
            index,
            v.len()
        ));
    }
    quicksort(v, is_less);
}

/// sort_by_cached_key: sorts indices by precomputed keys (stable), then permutes.
pub fn sort_by_cached_key<T, K: Ord, F: FnMut(&T) -> K>(v: &mut [T], f: F) {
    let mut f = f;
    let len = v.len();
    if len < 2 {
        return;
    }
    let mut indices: crate::vec::Vec<(K, usize)> = crate::vec::Vec::with_capacity(len);
    let mut i = 0;
    while i < len {
        indices.push((f(&v[i]), i));
        i += 1;
    }
    merge_sort(&mut indices[..], &mut |a: &(K, usize), b: &(K, usize)| a.0 < b.0);
    let mut i = 0;
    while i < len {
        let mut index = indices[i].1;
        while index < i {
            index = indices[index].1;
        }
        indices[i].1 = index;
        v.swap(i, index);
        i += 1;
    }
}
