//! Stable tiled merge sorting and partial heap sorting.
//!
//! Translated from `src/engine/engine_sort.h` of the engine named in
//! CREDITS.md (Apache-2.0, Copyright 2024 DeepMind Technologies Limited);
//! changed: translated to Rust.

// C _mjRUNSIZE: threshold size of a run for insertion sort.
const RUNSIZE: usize = 32;

fn insertion_sort<T: Copy>(
    arr: &mut [T],
    start: usize,
    end: usize,
    cmp: &mut impl FnMut(&T, &T) -> i32,
) {
    // the run on its own: its indices are checked once, not at every step
    let run = &mut arr[start..end];
    for j in 1..run.len() {
        let tmp = run[j];
        let mut i = j;
        while i > 0 && cmp(&run[i - 1], &tmp) > 0 {
            run[i] = run[i - 1];
            i -= 1;
        }
        run[i] = tmp;
    }
}

fn merge<T: Copy>(
    src: &[T],
    dest: &mut [T],
    start: usize,
    mid: usize,
    end: usize,
    cmp: &mut impl FnMut(&T, &T) -> i32,
) {
    // the two runs and their destination on their own: their indices are checked once
    let (a, b) = (&src[start..mid], &src[mid..end]);
    let out = &mut dest[start..end];
    let (mut i, mut j, mut k) = (0, 0, 0);
    while i < a.len() && j < b.len() {
        if cmp(&a[i], &b[j]) <= 0 {
            out[k] = a[i];
            i += 1;
        } else {
            out[k] = b[j];
            j += 1;
        }
        k += 1;
    }
    if i < a.len() {
        out[k..].copy_from_slice(&a[i..]);
    } else if j < b.len() {
        out[k..].copy_from_slice(&b[j..]);
    }
}

pub fn sort<T: Copy>(arr: &mut [T], buf: &mut [T], n: usize, cmp: &mut impl FnMut(&T, &T) -> i32) {
    let mut start = 0;
    while start < n {
        let end = if start + RUNSIZE < n {
            start + RUNSIZE
        } else {
            n
        };
        insertion_sort(arr, start, end, cmp);
        start += RUNSIZE;
    }
    // A scoped reborrow retains arr for the final C src != arr copy.
    let mut src_is_arr = true;
    {
        let mut src = &mut *arr;
        let mut dest = &mut *buf;
        let mut len = RUNSIZE;
        while len < n {
            let mut start = 0;
            while start < n {
                let mid = start + len;
                let end = if start + 2 * len < n {
                    start + 2 * len
                } else {
                    n
                };
                if mid < end {
                    merge(src, dest, start, mid, end, cmp);
                } else {
                    dest[start..end].copy_from_slice(&src[start..end]);
                }
                start += 2 * len;
            }
            std::mem::swap(&mut src, &mut dest);
            src_is_arr = !src_is_arr;
            len *= 2;
        }
    }
    if !src_is_arr {
        arr[..n].copy_from_slice(&buf[..n]);
    }
}

fn sift_down<T: Copy>(
    buf: &mut [T],
    start: usize,
    end: usize,
    cmp: &mut impl FnMut(&T, &T) -> i32,
) {
    let mut root = start;
    while 2 * root + 1 < end {
        let child = 2 * root + 1;
        let mut swap = root;
        if cmp(&buf[swap], &buf[child]) < 0 {
            swap = child;
        }
        if child + 1 < end && cmp(&buf[swap], &buf[child + 1]) < 0 {
            swap = child + 1;
        }
        if swap == root {
            break;
        }
        let tmp = buf[root];
        buf[root] = buf[swap];
        buf[swap] = tmp;
        root = swap;
    }
}

pub fn partial_sort<T: Copy>(
    arr: &mut [T],
    buf: &mut [T],
    n: usize,
    k: usize,
    cmp: &mut impl FnMut(&T, &T) -> i32,
) {
    if k == 0 || n < k {
        return;
    }
    for i in 0..k {
        buf[i] = arr[i];
    }
    // C division truncates toward zero, including (1 - 2) / 2 == 0.
    let mut j = (k as isize - 2) / 2;
    while j >= 0 {
        sift_down(buf, j as usize, k, cmp);
        j -= 1;
    }
    for i in k..n {
        if cmp(&arr[i], &buf[0]) < 0 {
            buf[0] = arr[i];
            sift_down(buf, 0, k, cmp);
        }
    }
    for j in 0..k {
        arr[j] = buf[j];
    }
    insertion_sort(arr, 0, k, cmp);
}
