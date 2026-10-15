//! Port of real core's driftsort, ipnsort, shared pivot/smallsort and introselect
//! (MIT/Apache-2.0), by Orson Peters and Lukas Bergdoll, and alloc's scratch sizing.
//! The modules stay inline to keep this implementation in Relax's slice/sort.rs.
//! Freeze dispatch (real core's `T: Freeze` specialization) asks Relax's per-instance
//! is_freeze intrinsic; other types keep insertion small-sort and preserve interior mutations.

use crate::mem::{self, MaybeUninit};

/// `T: Freeze` for this instance (no UnsafeCell outside indirection).
#[inline(always)]
pub(crate) fn is_freeze<T>() -> bool {
    #[cfg(relax_check)]
    if FORCE_FREEZE.load(core::sync::atomic::Ordering::Relaxed) {
        return true;
    }
    unsafe { crate::intrinsics_mem::is_freeze::<T>() }
}

/// Shim-only override. Tests must serialize access and avoid comparing mutable
/// interior state when forcing Freeze for a type that is not actually Freeze.
#[cfg(relax_check)]
pub static FORCE_FREEZE: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

pub fn insertion_sort<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) {
    if v.len() >= 2 && mem::size_of::<T>() != 0 {
        shared::smallsort::insertion_sort_shift_left(v, 1, is_less);
    }
}

/// Stable driftsort, retaining the old entry point used by slice and Vec.
pub fn merge_sort<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) {
    if mem::size_of::<T>() == 0 || v.len() < 2 {
        return;
    }
    if v.len() <= 20 {
        insertion_sort(v, is_less);
        return;
    }
    driftsort_main(v, is_less);
}

// As in real core: align 4 KiB of byte storage for T without constructing T.
#[repr(C)]
struct AlignedStorage<T> {
    align: [T; 0],
    bytes: [MaybeUninit<u8>; 4096],
}

#[inline(never)]
fn driftsort_main<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) {
    let len = v.len();
    let alloc_len = crate::cmp::max(
        crate::cmp::max(
            len - len / 2,
            crate::cmp::min(len, 8_000_000 / mem::size_of::<T>()),
        ),
        48,
    );
    let mut stack = MaybeUninit::<AlignedStorage<T>>::uninit();
    let stack_len = 4096 / mem::size_of::<T>();
    let mut heap: crate::vec::Vec<T>;
    let scratch = if stack_len >= alloc_len {
        // SAFETY: repr(C), zero-length T array supplies alignment; all bytes are
        // uninitialized scratch, not live T values. The storage outlives the sort.
        unsafe {
            crate::intrinsics_mem::slice_from_raw_mut(
                stack.as_mut_ptr() as *mut MaybeUninit<T>,
                stack_len,
            )
        }
    } else {
        heap = crate::vec::Vec::with_capacity(alloc_len);
        // Vec length stays zero: scratch owns no elements, even during unwind.
        unsafe {
            crate::intrinsics_mem::slice_from_raw_mut(
                heap.as_mut_ptr() as *mut MaybeUninit<T>,
                alloc_len,
            )
        }
    };
    stable::drift::sort(
        v,
        scratch,
        len <= shared::smallsort::stable_small_sort_threshold::<T>() * 2,
        is_less,
    );
}

/// Unstable ipnsort.
pub fn quicksort<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) {
    if mem::size_of::<T>() == 0 || v.len() < 2 {
        return;
    }
    if v.len() <= 20 {
        insertion_sort(v, is_less);
        return;
    }
    let (run, reversed) = shared::find_existing_run(v, is_less);
    if run == v.len() {
        if reversed {
            v.reverse();
        }
        return;
    }
    let limit = 2 * (v.len() | 1).ilog2();
    unstable::quicksort::quicksort(v, None, limit, is_less);
}

pub fn heapsort<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) {
    unstable::heapsort::heapsort(v, is_less);
}

pub fn select_nth<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], index: usize, is_less: &mut F) {
    if index >= v.len() {
        crate::panicking::panic_fmt(format_args!(
            "partition_at_index index {} greater than length of slice {}",
            index,
            v.len()
        ));
    }
    if mem::size_of::<T>() == 0 {
        return;
    }
    if index == v.len() - 1 {
        let i = select::max_index(v, is_less).unwrap();
        v.swap(i, index);
    } else if index == 0 {
        let i = select::min_index(v, is_less).unwrap();
        v.swap(i, index);
    } else {
        select::partition_at_index_loop(v, index, None, is_less);
    }
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
    merge_sort(&mut indices[..], &mut |a: &(K, usize), b: &(K, usize)| {
        a.0 < b.0
    });
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

mod shared {
    /// Finds a run of sorted elements starting at the beginning of the slice.
    ///
    /// Returns the length of the run, and a bool that is false when the run
    /// is ascending, and true if the run strictly descending.
    #[inline(always)]
    pub(crate) fn find_existing_run<T, F: FnMut(&T, &T) -> bool>(
        v: &[T],
        is_less: &mut F,
    ) -> (usize, bool) {
        let len = v.len();
        if len < 2 {
            return (len, false);
        }

        // SAFETY: We checked that len >= 2, so 0 and 1 are valid indices.
        // This also means that run_len < len implies run_len and run_len - 1
        // are valid indices as well.
        unsafe {
            let mut run_len = 2;
            let strictly_descending = is_less(v.get_unchecked(1), v.get_unchecked(0));
            if strictly_descending {
                while run_len < len
                    && is_less(v.get_unchecked(run_len), v.get_unchecked(run_len - 1))
                {
                    run_len += 1;
                }
            } else {
                while run_len < len
                    && !is_less(v.get_unchecked(run_len), v.get_unchecked(run_len - 1))
                {
                    run_len += 1;
                }
            }
            (run_len, strictly_descending)
        }
    }

    pub(crate) mod pivot {
        //! This module contains the logic for pivot selection.

        use crate::hint;

        // Recursively select a pseudomedian if above this threshold.
        const PSEUDO_MEDIAN_REC_THRESHOLD: usize = 64;

        /// Selects a pivot from `v`. Algorithm taken from glidesort by Orson Peters.
        ///
        /// This chooses a pivot by sampling an adaptive amount of points, approximating
        /// the quality of a median of sqrt(n) elements.
        #[inline]
        pub fn choose_pivot<T, F: FnMut(&T, &T) -> bool>(v: &[T], is_less: &mut F) -> usize {
            // We use unsafe code and raw pointers here because we're dealing with
            // heavy recursion. Passing safe slices around would involve a lot of
            // branches and function call overhead.

            let len = v.len();
            if len < 8 {
                crate::panicking::panic_str("sort invariant violated");
            }

            // SAFETY: a, b, c point to initialized regions of len_div_8 elements,
            // satisfying median3 and median3_rec's preconditions as v_base points
            // to an initialized region of n = len elements.
            let index = unsafe {
                let v_base = v.as_ptr();
                let len_div_8 = len / 8;

                let a = v_base; // [0, floor(n/8))
                let b = v_base.add(len_div_8 * 4); // [4*floor(n/8), 5*floor(n/8))
                let c = v_base.add(len_div_8 * 7); // [7*floor(n/8), 8*floor(n/8))

                if len < PSEUDO_MEDIAN_REC_THRESHOLD {
                    median3(&*a, &*b, &*c, is_less).offset_from_unsigned(v_base)
                } else {
                    median3_rec(a, b, c, len_div_8, is_less).offset_from_unsigned(v_base)
                }
            };
            // SAFETY: preconditions must have been met for offset_from_unsigned()
            unsafe {
                hint::assert_unchecked(index < v.len());
                index
            }
        }

        /// Calculates an approximate median of 3 elements from sections a, b, c, or
        /// recursively from an approximation of each, if they're large enough. By
        /// dividing the size of each section by 8 when recursing we have logarithmic
        /// recursion depth and overall sample from f(n) = 3*f(n/8) -> f(n) =
        /// O(n^(log(3)/log(8))) ~= O(n^0.528) elements.
        ///
        /// SAFETY: a, b, c must point to the start of initialized regions of memory of
        /// at least n elements.
        unsafe fn median3_rec<T, F: FnMut(&T, &T) -> bool>(
            mut a: *const T,
            mut b: *const T,
            mut c: *const T,
            n: usize,
            is_less: &mut F,
        ) -> *const T {
            // SAFETY: a, b, c still point to initialized regions of n / 8 elements,
            // by the exact same logic as in choose_pivot.
            unsafe {
                if n * 8 >= PSEUDO_MEDIAN_REC_THRESHOLD {
                    let n8 = n / 8;
                    a = median3_rec(a, a.add(n8 * 4), a.add(n8 * 7), n8, is_less);
                    b = median3_rec(b, b.add(n8 * 4), b.add(n8 * 7), n8, is_less);
                    c = median3_rec(c, c.add(n8 * 4), c.add(n8 * 7), n8, is_less);
                }
                median3(&*a, &*b, &*c, is_less)
            }
        }

        /// Calculates the median of 3 elements.
        ///
        /// SAFETY: a, b, c must be valid initialized elements.
        #[inline(always)]
        fn median3<T, F: FnMut(&T, &T) -> bool>(a: &T, b: &T, c: &T, is_less: &mut F) -> *const T {
            // Compiler tends to make this branchless when sensible, and avoids the
            // third comparison when not.
            let x = is_less(a, b);
            let y = is_less(a, c);
            if x == y {
                // If x=y=0 then b, c <= a. In this case we want to return max(b, c).
                // If x=y=1 then a < b, c. In this case we want to return min(b, c).
                // By toggling the outcome of b < c using XOR x we get this behavior.
                let z = is_less(b, c);
                if z ^ x {
                    c
                } else {
                    b
                }
            } else {
                // Either c <= a < b or b <= a < c, thus a is our median.
                a
            }
        }
    }

    pub(crate) mod smallsort {
        use crate::mem::{self, size_of, ManuallyDrop, MaybeUninit};
        use crate::slice::sort::is_freeze;
        use crate::{hint, ptr};

        #[inline(always)]
        pub(crate) fn stable_small_sort_threshold<T>() -> usize {
            if is_freeze::<T>() {
                SMALL_SORT_GENERAL_THRESHOLD
            } else {
                SMALL_SORT_FALLBACK_THRESHOLD
            }
        }

        #[inline(always)]
        pub(crate) fn stable_small_sort<T, F: FnMut(&T, &T) -> bool>(
            v: &mut [T],
            scratch: &mut [MaybeUninit<T>],
            is_less: &mut F,
        ) {
            if is_freeze::<T>() {
                small_sort_general_with_scratch(v, scratch, is_less);
            } else {
                small_sort_fallback(v, is_less);
            }
        }

        #[inline(always)]
        pub(crate) fn unstable_small_sort_threshold<T>() -> usize {
            if is_freeze::<T>() {
                if has_efficient_in_place_swap::<T>()
                    && size_of::<T>() <= MAX_STACK_ARRAY_SIZE / SMALL_SORT_NETWORK_SCRATCH_LEN
                {
                    return SMALL_SORT_NETWORK_THRESHOLD;
                }
                if size_of::<T>() <= MAX_STACK_ARRAY_SIZE / SMALL_SORT_GENERAL_SCRATCH_LEN {
                    return SMALL_SORT_GENERAL_THRESHOLD;
                }
            }
            SMALL_SORT_FALLBACK_THRESHOLD
        }

        #[inline(always)]
        pub(crate) fn unstable_small_sort<T, F: FnMut(&T, &T) -> bool>(
            v: &mut [T],
            is_less: &mut F,
        ) {
            if is_freeze::<T>() {
                // Relax's Freeze query currently implies Copy. The networks themselves
                // only move elements and do not require a Copy bound.
                if has_efficient_in_place_swap::<T>()
                    && size_of::<T>() <= MAX_STACK_ARRAY_SIZE / SMALL_SORT_NETWORK_SCRATCH_LEN
                {
                    small_sort_network(v, is_less);
                    return;
                }
                if size_of::<T>() <= MAX_STACK_ARRAY_SIZE / SMALL_SORT_GENERAL_SCRATCH_LEN {
                    small_sort_general(v, is_less);
                    return;
                }
            }
            small_sort_fallback(v, is_less);
        }

        /// Optimal number of comparisons, and good perf.
        const SMALL_SORT_FALLBACK_THRESHOLD: usize = 16;

        /// From a comparison perspective 20 was ~2% more efficient for fully random input, but for
        /// wall-clock performance choosing 32 yielded better performance overall.
        ///
        /// SAFETY: If you change this value, you have to adjust [`small_sort_general`] !
        const SMALL_SORT_GENERAL_THRESHOLD: usize = 32;

        /// [`small_sort_general`] uses [`sort8_stable`] as primitive and does a kind of ping-pong merge,
        /// where the output of the first two [`sort8_stable`] calls is stored at the end of the scratch
        /// buffer. This simplifies panic handling and avoids additional copies. This affects the required
        /// scratch buffer size.
        ///
        /// SAFETY: If you change this value, you have to adjust [`small_sort_general`] !
        pub(crate) const SMALL_SORT_GENERAL_SCRATCH_LEN: usize = SMALL_SORT_GENERAL_THRESHOLD + 16;

        /// SAFETY: If you change this value, you have to adjust [`small_sort_network`] !
        const SMALL_SORT_NETWORK_THRESHOLD: usize = 32;
        const SMALL_SORT_NETWORK_SCRATCH_LEN: usize = SMALL_SORT_NETWORK_THRESHOLD;

        /// Using a stack array, could cause a stack overflow if the type `T` is very large. To be
        /// conservative we limit the usage of small-sorts that require a stack array to types that fit
        /// within this limit.
        const MAX_STACK_ARRAY_SIZE: usize = 4096;

        fn small_sort_fallback<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) {
            if v.len() >= 2 {
                insertion_sort_shift_left(v, 1, is_less);
            }
        }

        // These private routines are reached only through the Freeze dispatch
        // above. No Copy/Freeze trait operations are assumed by generic code:
        // scratch holds bitwise moves, never independently dropped T values.
        fn small_sort_general<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) {
            let mut stack_array = MaybeUninit::<[T; SMALL_SORT_GENERAL_SCRATCH_LEN]>::uninit();

            // SAFETY: The memory is backed by `stack_array`, and the operation is safe as long as the len
            // is the same.
            let scratch = unsafe {
                crate::intrinsics_mem::slice_from_raw_mut(
                    stack_array.as_mut_ptr() as *mut MaybeUninit<T>,
                    SMALL_SORT_GENERAL_SCRATCH_LEN,
                )
            };

            small_sort_general_with_scratch(v, scratch, is_less);
        }

        fn small_sort_general_with_scratch<T, F: FnMut(&T, &T) -> bool>(
            v: &mut [T],
            scratch: &mut [MaybeUninit<T>],
            is_less: &mut F,
        ) {
            let len = v.len();
            if len < 2 {
                return;
            }

            if scratch.len() < len + 16 {
                crate::panicking::panic_str("sort invariant violated");
            }

            let v_base = v.as_mut_ptr();
            let len_div_2 = len / 2;

            // SAFETY: See individual comments.
            unsafe {
                let scratch_base = scratch.as_mut_ptr() as *mut T;

                let presorted_len = if size_of::<T>() <= 16 && len >= 16 {
                    // SAFETY: scratch_base is valid and has enough space.
                    sort8_stable(v_base, scratch_base, scratch_base.add(len), is_less);
                    sort8_stable(
                        v_base.add(len_div_2),
                        scratch_base.add(len_div_2),
                        scratch_base.add(len + 8),
                        is_less,
                    );

                    8
                } else if len >= 8 {
                    // SAFETY: scratch_base is valid and has enough space.
                    sort4_stable(v_base, scratch_base, is_less);
                    sort4_stable(v_base.add(len_div_2), scratch_base.add(len_div_2), is_less);

                    4
                } else {
                    ptr::copy_nonoverlapping(v_base, scratch_base, 1);
                    ptr::copy_nonoverlapping(v_base.add(len_div_2), scratch_base.add(len_div_2), 1);

                    1
                };

                for offset in [0, len_div_2] {
                    // SAFETY: at this point dst is initialized with presorted_len elements.
                    // We extend this to desired_len, src is valid for desired_len elements.
                    let src = v_base.add(offset);
                    let dst = scratch_base.add(offset);
                    let desired_len = if offset == 0 {
                        len_div_2
                    } else {
                        len - len_div_2
                    };

                    for i in presorted_len..desired_len {
                        ptr::copy_nonoverlapping(src.add(i), dst.add(i), 1);
                        insert_tail(dst, dst.add(i), is_less);
                    }
                }

                // SAFETY: see comment in `CopyOnDrop::drop`.
                let drop_guard = CopyOnDrop {
                    src: scratch_base,
                    dst: v_base,
                    len,
                };

                // SAFETY: at this point scratch_base is fully initialized, allowing us
                // to use it as the source of our merge back into the original array.
                // If a panic occurs we ensure the original array is restored to a valid
                // permutation of the input through drop_guard. This technique is similar
                // to ping-pong merging.
                bidirectional_merge(
                    crate::intrinsics_mem::slice_from_raw(drop_guard.src, drop_guard.len),
                    drop_guard.dst,
                    is_less,
                );
                mem::forget(drop_guard);
            }
        }

        struct CopyOnDrop<T> {
            src: *const T,
            dst: *mut T,
            len: usize,
        }

        impl<T> Drop for CopyOnDrop<T> {
            fn drop(&mut self) {
                // SAFETY: `src` must contain `len` initialized elements, and dst must
                // be valid to write `len` elements.
                unsafe {
                    ptr::copy_nonoverlapping(self.src, self.dst, self.len);
                }
            }
        }

        fn small_sort_network<T, F>(v: &mut [T], is_less: &mut F)
        where
            F: FnMut(&T, &T) -> bool,
        {
            // This implementation is tuned to be efficient for integer types.

            let len = v.len();
            if len < 2 {
                return;
            }

            if len > SMALL_SORT_NETWORK_SCRATCH_LEN {
                crate::panicking::panic_str("sort invariant violated");
            }

            let mut stack_array = MaybeUninit::<[T; SMALL_SORT_NETWORK_SCRATCH_LEN]>::uninit();

            let len_div_2 = len / 2;
            let no_merge = len < 18;

            let v_base = v.as_mut_ptr();
            let initial_region_len = if no_merge { len } else { len_div_2 };
            // SAFETY: Both possible values of `initial_region_len` are in-bounds.
            let mut region =
                unsafe { crate::intrinsics_mem::slice_from_raw_mut(v_base, initial_region_len) };

            // Avoid compiler unrolling, we *really* don't want that to happen here for binary-size reasons.
            loop {
                let presorted_len = if region.len() >= 13 {
                    sort13_optimal(region, is_less);
                    13
                } else if region.len() >= 9 {
                    sort9_optimal(region, is_less);
                    9
                } else {
                    1
                };

                insertion_sort_shift_left(region, presorted_len, is_less);

                if no_merge {
                    return;
                }

                if region.as_ptr() != v_base as *const T {
                    break;
                }

                // SAFETY: The right side of `v` based on `len_div_2` is guaranteed in-bounds.
                unsafe {
                    region = crate::intrinsics_mem::slice_from_raw_mut(
                        v_base.add(len_div_2),
                        len - len_div_2,
                    )
                };
            }

            // SAFETY: We checked that T is Freeze and thus observation safe.
            // Should is_less panic v was not modified in parity_merge and retains it's original input.
            // scratch and v must not alias and scratch has v.len() space.
            unsafe {
                let scratch_base = stack_array.as_mut_ptr() as *mut T;
                bidirectional_merge(
                    crate::intrinsics_mem::slice_from_raw_mut(v_base, len),
                    scratch_base,
                    is_less,
                );
                ptr::copy_nonoverlapping(scratch_base, v_base, len);
            }
        }

        /// Swap two values in the slice pointed to by `v_base` at the position `a_pos` and `b_pos` if the
        /// value at position `b_pos` is less than the one at position `a_pos`.
        ///
        /// Purposefully not marked `#[inline]`, despite us wanting it to be inlined for integers like
        /// types. `is_less` could be a huge function and we want to give the compiler an option to
        /// not inline this function. For the same reasons that this function is very perf critical
        /// it should be in the same module as the functions that use it.
        unsafe fn swap_if_less<T, F>(v_base: *mut T, a_pos: usize, b_pos: usize, is_less: &mut F)
        where
            F: FnMut(&T, &T) -> bool,
        {
            // SAFETY: the caller must guarantee that `a_pos` and `b_pos` each added to `v_base` yield valid
            // pointers into `v_base`, and are properly aligned, and part of the same allocation.
            unsafe {
                let v_a = v_base.add(a_pos);
                let v_b = v_base.add(b_pos);

                // PANIC SAFETY: if is_less panics, no scratch memory was created and the slice should still be
                // in a well defined state, without duplicates.

                // Important to only swap if it is more and not if it is equal. is_less should return false for
                // equal, so we don't swap.
                let should_swap = is_less(&*v_b, &*v_a);

                // This is a branchless version of swap if.
                // The equivalent code with a branch would be:
                //
                // if should_swap {
                //     ptr::swap(v_a, v_b, 1);
                // }

                // The goal is to generate cmov instructions here.
                let v_a_swap = hint::select_unpredictable(should_swap, v_b, v_a);
                let v_b_swap = hint::select_unpredictable(should_swap, v_a, v_b);

                let v_b_swap_tmp = ManuallyDrop::new(ptr::read(v_b_swap));
                ptr::copy(v_a_swap, v_a, 1);
                ptr::copy_nonoverlapping(&*v_b_swap_tmp, v_b, 1);
            }
        }

        /// Sorts the first 9 elements of `v` with a fast fixed function.
        ///
        /// Should `is_less` generate substantial amounts of code the compiler can choose to not inline
        /// `swap_if_less`. If the code of a sort impl changes so as to call this function in multiple
        /// places, `#[inline(never)]` is recommended to keep binary-size in check. The current design of
        /// `small_sort_network` makes sure to only call this once.
        fn sort9_optimal<T, F>(v: &mut [T], is_less: &mut F)
        where
            F: FnMut(&T, &T) -> bool,
        {
            if v.len() < 9 {
                crate::panicking::panic_str("sort invariant violated");
            }

            let v_base = v.as_mut_ptr();

            // Optimal sorting network see:
            // https://bertdobbelaere.github.io/sorting_networks.html.

            // SAFETY: We checked the len.
            unsafe {
                swap_if_less(v_base, 0, 3, is_less);
                swap_if_less(v_base, 1, 7, is_less);
                swap_if_less(v_base, 2, 5, is_less);
                swap_if_less(v_base, 4, 8, is_less);
                swap_if_less(v_base, 0, 7, is_less);
                swap_if_less(v_base, 2, 4, is_less);
                swap_if_less(v_base, 3, 8, is_less);
                swap_if_less(v_base, 5, 6, is_less);
                swap_if_less(v_base, 0, 2, is_less);
                swap_if_less(v_base, 1, 3, is_less);
                swap_if_less(v_base, 4, 5, is_less);
                swap_if_less(v_base, 7, 8, is_less);
                swap_if_less(v_base, 1, 4, is_less);
                swap_if_less(v_base, 3, 6, is_less);
                swap_if_less(v_base, 5, 7, is_less);
                swap_if_less(v_base, 0, 1, is_less);
                swap_if_less(v_base, 2, 4, is_less);
                swap_if_less(v_base, 3, 5, is_less);
                swap_if_less(v_base, 6, 8, is_less);
                swap_if_less(v_base, 2, 3, is_less);
                swap_if_less(v_base, 4, 5, is_less);
                swap_if_less(v_base, 6, 7, is_less);
                swap_if_less(v_base, 1, 2, is_less);
                swap_if_less(v_base, 3, 4, is_less);
                swap_if_less(v_base, 5, 6, is_less);
            }
        }

        /// Sorts the first 13 elements of `v` with a fast fixed function.
        ///
        /// Should `is_less` generate substantial amounts of code the compiler can choose to not inline
        /// `swap_if_less`. If the code of a sort impl changes so as to call this function in multiple
        /// places, `#[inline(never)]` is recommended to keep binary-size in check. The current design of
        /// `small_sort_network` makes sure to only call this once.
        fn sort13_optimal<T, F>(v: &mut [T], is_less: &mut F)
        where
            F: FnMut(&T, &T) -> bool,
        {
            if v.len() < 13 {
                crate::panicking::panic_str("sort invariant violated");
            }

            let v_base = v.as_mut_ptr();

            // Optimal sorting network see:
            // https://bertdobbelaere.github.io/sorting_networks.html.

            // SAFETY: We checked the len.
            unsafe {
                swap_if_less(v_base, 0, 12, is_less);
                swap_if_less(v_base, 1, 10, is_less);
                swap_if_less(v_base, 2, 9, is_less);
                swap_if_less(v_base, 3, 7, is_less);
                swap_if_less(v_base, 5, 11, is_less);
                swap_if_less(v_base, 6, 8, is_less);
                swap_if_less(v_base, 1, 6, is_less);
                swap_if_less(v_base, 2, 3, is_less);
                swap_if_less(v_base, 4, 11, is_less);
                swap_if_less(v_base, 7, 9, is_less);
                swap_if_less(v_base, 8, 10, is_less);
                swap_if_less(v_base, 0, 4, is_less);
                swap_if_less(v_base, 1, 2, is_less);
                swap_if_less(v_base, 3, 6, is_less);
                swap_if_less(v_base, 7, 8, is_less);
                swap_if_less(v_base, 9, 10, is_less);
                swap_if_less(v_base, 11, 12, is_less);
                swap_if_less(v_base, 4, 6, is_less);
                swap_if_less(v_base, 5, 9, is_less);
                swap_if_less(v_base, 8, 11, is_less);
                swap_if_less(v_base, 10, 12, is_less);
                swap_if_less(v_base, 0, 5, is_less);
                swap_if_less(v_base, 3, 8, is_less);
                swap_if_less(v_base, 4, 7, is_less);
                swap_if_less(v_base, 6, 11, is_less);
                swap_if_less(v_base, 9, 10, is_less);
                swap_if_less(v_base, 0, 1, is_less);
                swap_if_less(v_base, 2, 5, is_less);
                swap_if_less(v_base, 6, 9, is_less);
                swap_if_less(v_base, 7, 8, is_less);
                swap_if_less(v_base, 10, 11, is_less);
                swap_if_less(v_base, 1, 3, is_less);
                swap_if_less(v_base, 2, 4, is_less);
                swap_if_less(v_base, 5, 6, is_less);
                swap_if_less(v_base, 9, 10, is_less);
                swap_if_less(v_base, 1, 2, is_less);
                swap_if_less(v_base, 3, 4, is_less);
                swap_if_less(v_base, 5, 7, is_less);
                swap_if_less(v_base, 6, 8, is_less);
                swap_if_less(v_base, 2, 3, is_less);
                swap_if_less(v_base, 4, 5, is_less);
                swap_if_less(v_base, 6, 7, is_less);
                swap_if_less(v_base, 8, 9, is_less);
                swap_if_less(v_base, 3, 4, is_less);
                swap_if_less(v_base, 5, 6, is_less);
            }
        }

        /// Sorts range [begin, tail] assuming [begin, tail) is already sorted.
        ///
        /// # Safety
        /// begin < tail and p must be valid and initialized for all begin <= p <= tail.
        unsafe fn insert_tail<T, F: FnMut(&T, &T) -> bool>(
            begin: *mut T,
            tail: *mut T,
            is_less: &mut F,
        ) {
            // SAFETY: see individual comments.
            unsafe {
                // SAFETY: in-bounds as tail > begin.
                let mut sift = tail.sub(1);
                if !is_less(&*tail, &*sift) {
                    return;
                }

                // SAFETY: after this read tail is never read from again, as we only ever
                // read from sift, sift < tail and we only ever decrease sift. Thus this is
                // effectively a move, not a copy. Should a panic occur, or we have found
                // the correct insertion position, gap_guard ensures the element is moved
                // back into the array.
                let tmp = ManuallyDrop::new(tail.read());
                let mut gap_guard = CopyOnDrop {
                    src: &*tmp,
                    dst: tail,
                    len: 1,
                };

                loop {
                    // SAFETY: we move sift into the gap (which is valid), and point the
                    // gap guard destination at sift, ensuring that if a panic occurs the
                    // gap is once again filled.
                    ptr::copy_nonoverlapping(sift, gap_guard.dst, 1);
                    gap_guard.dst = sift;

                    if sift == begin {
                        break;
                    }

                    // SAFETY: we checked that sift != begin, thus this is in-bounds.
                    sift = sift.sub(1);
                    if !is_less(&tmp, &*sift) {
                        break;
                    }
                }
            }
        }

        /// Sort `v` assuming `v[..offset]` is already sorted.
        pub fn insertion_sort_shift_left<T, F: FnMut(&T, &T) -> bool>(
            v: &mut [T],
            offset: usize,
            is_less: &mut F,
        ) {
            let len = v.len();
            if offset == 0 || offset > len {
                crate::panicking::panic_str("sort invariant violated");
            }

            // SAFETY: see individual comments.
            unsafe {
                // We write this basic loop directly using pointers, as when we use a
                // for loop LLVM likes to unroll this loop which we do not want.
                // SAFETY: v_end is the one-past-end pointer, and we checked that
                // offset <= len, thus tail is also in-bounds.
                let v_base = v.as_mut_ptr();
                let v_end = v_base.add(len);
                let mut tail = v_base.add(offset);
                while tail != v_end {
                    // SAFETY: v_base and tail are both valid pointers to elements, and
                    // v_base < tail since we checked offset != 0.
                    insert_tail(v_base, tail, is_less);

                    // SAFETY: we checked that tail is not yet the one-past-end pointer.
                    tail = tail.add(1);
                }
            }
        }

        /// SAFETY: The caller MUST guarantee that `v_base` is valid for 4 reads and
        /// `dst` is valid for 4 writes. The result will be stored in `dst[0..4]`.
        pub unsafe fn sort4_stable<T, F: FnMut(&T, &T) -> bool>(
            v_base: *const T,
            dst: *mut T,
            is_less: &mut F,
        ) {
            // By limiting select to picking pointers, we are guaranteed good cmov code-gen
            // regardless of type T's size. Further this only does 5 instead of 6
            // comparisons compared to a stable transposition 4 element sorting-network,
            // and always copies each element exactly once.

            // SAFETY: all pointers have offset at most 3 from v_base and dst, and are
            // thus in-bounds by the precondition.
            unsafe {
                // Stably create two pairs a <= b and c <= d.
                let c1 = is_less(&*v_base.add(1), &*v_base);
                let c2 = is_less(&*v_base.add(3), &*v_base.add(2));
                let a = v_base.add(c1 as usize);
                let b = v_base.add(!c1 as usize);
                let c = v_base.add(2 + c2 as usize);
                let d = v_base.add(2 + (!c2 as usize));

                // Compare (a, c) and (b, d) to identify max/min. We're left with two
                // unknown elements, but because we are a stable sort we must know which
                // one is leftmost and which one is rightmost.
                // c3, c4 | min max unknown_left unknown_right
                //  0,  0 |  a   d    b         c
                //  0,  1 |  a   b    c         d
                //  1,  0 |  c   d    a         b
                //  1,  1 |  c   b    a         d
                let c3 = is_less(&*c, &*a);
                let c4 = is_less(&*d, &*b);
                let min = hint::select_unpredictable(c3, c, a);
                let max = hint::select_unpredictable(c4, b, d);
                let unknown_left =
                    hint::select_unpredictable(c3, a, hint::select_unpredictable(c4, c, b));
                let unknown_right =
                    hint::select_unpredictable(c4, d, hint::select_unpredictable(c3, b, c));

                // Sort the last two unknown elements.
                let c5 = is_less(&*unknown_right, &*unknown_left);
                let lo = hint::select_unpredictable(c5, unknown_right, unknown_left);
                let hi = hint::select_unpredictable(c5, unknown_left, unknown_right);

                ptr::copy_nonoverlapping(min, dst, 1);
                ptr::copy_nonoverlapping(lo, dst.add(1), 1);
                ptr::copy_nonoverlapping(hi, dst.add(2), 1);
                ptr::copy_nonoverlapping(max, dst.add(3), 1);
            }
        }

        /// SAFETY: The caller MUST guarantee that `v_base` is valid for 8 reads and
        /// writes, `scratch_base` and `dst` MUST be valid for 8 writes. The result will
        /// be stored in `dst[0..8]`. T must be observation-safe (Freeze).
        unsafe fn sort8_stable<T, F: FnMut(&T, &T) -> bool>(
            v_base: *mut T,
            dst: *mut T,
            scratch_base: *mut T,
            is_less: &mut F,
        ) {
            // SAFETY: these pointers are all in-bounds by the precondition of our function.
            unsafe {
                sort4_stable(v_base, scratch_base, is_less);
                sort4_stable(v_base.add(4), scratch_base.add(4), is_less);
            }

            // SAFETY: scratch_base[0..8] is now initialized, allowing us to merge back
            // into dst.
            unsafe {
                bidirectional_merge(
                    crate::intrinsics_mem::slice_from_raw(scratch_base, 8),
                    dst,
                    is_less,
                );
            }
        }

        #[inline(always)]
        unsafe fn merge_up<T, F: FnMut(&T, &T) -> bool>(
            mut left_src: *const T,
            mut right_src: *const T,
            mut dst: *mut T,
            is_less: &mut F,
        ) -> (*const T, *const T, *mut T) {
            // This is a branchless merge utility function.
            // The equivalent code with a branch would be:
            //
            // if !is_less(&*right_src, &*left_src) {
            //     ptr::copy_nonoverlapping(left_src, dst, 1);
            //     left_src = left_src.add(1);
            // } else {
            //     ptr::copy_nonoverlapping(right_src, dst, 1);
            //     right_src = right_src.add(1);
            // }
            // dst = dst.add(1);

            // SAFETY: The caller must guarantee that `left_src`, `right_src` are valid
            // to read and `dst` is valid to write, while not aliasing.
            unsafe {
                let is_l = !is_less(&*right_src, &*left_src);
                let src = if is_l { left_src } else { right_src };
                ptr::copy_nonoverlapping(src, dst, 1);
                right_src = right_src.add(!is_l as usize);
                left_src = left_src.add(is_l as usize);
                dst = dst.add(1);
            }

            (left_src, right_src, dst)
        }

        #[inline(always)]
        unsafe fn merge_down<T, F: FnMut(&T, &T) -> bool>(
            mut left_src: *const T,
            mut right_src: *const T,
            mut dst: *mut T,
            is_less: &mut F,
        ) -> (*const T, *const T, *mut T) {
            // This is a branchless merge utility function.
            // The equivalent code with a branch would be:
            //
            // if !is_less(&*right_src, &*left_src) {
            //     ptr::copy_nonoverlapping(right_src, dst, 1);
            //     right_src = right_src.wrapping_sub(1);
            // } else {
            //     ptr::copy_nonoverlapping(left_src, dst, 1);
            //     left_src = left_src.wrapping_sub(1);
            // }
            // dst = dst.sub(1);

            // SAFETY: The caller must guarantee that `left_src`, `right_src` are valid
            // to read and `dst` is valid to write, while not aliasing.
            unsafe {
                let is_l = !is_less(&*right_src, &*left_src);
                let src = if is_l { right_src } else { left_src };
                ptr::copy_nonoverlapping(src, dst, 1);
                right_src = right_src.wrapping_sub(is_l as usize);
                left_src = left_src.wrapping_sub(!is_l as usize);
                dst = dst.sub(1);
            }

            (left_src, right_src, dst)
        }

        /// Merge v assuming v[..len / 2] and v[len / 2..] are sorted.
        ///
        /// Original idea for bi-directional merging by Igor van den Hoven (quadsort),
        /// adapted to only use merge up and down. In contrast to the original
        /// parity_merge function, it performs 2 writes instead of 4 per iteration.
        ///
        /// # Safety
        /// The caller must guarantee that `dst` is valid for v.len() writes.
        /// Also `v.as_ptr()` and `dst` must not alias and v.len() must be >= 2.
        ///
        /// Note that T must be Freeze, the comparison function is evaluated on outdated
        /// temporary 'copies' that may not end up in the final array.
        unsafe fn bidirectional_merge<T, F: FnMut(&T, &T) -> bool>(
            v: &[T],
            dst: *mut T,
            is_less: &mut F,
        ) {
            // It helps to visualize the merge:
            //
            // Initial:
            //
            //  |dst (in dst)
            //  |left               |right
            //  v                   v
            // [xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx]
            //                     ^                   ^
            //                     |left_rev           |right_rev
            //                                         |dst_rev (in dst)
            //
            // After:
            //
            //                      |dst (in dst)
            //        |left         |           |right
            //        v             v           v
            // [xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx]
            //       ^             ^           ^
            //       |left_rev     |           |right_rev
            //                     |dst_rev (in dst)
            //
            // In each iteration one of left or right moves up one position, and one of
            // left_rev or right_rev moves down one position, whereas dst always moves
            // up one position and dst_rev always moves down one position. Assuming
            // the input was sorted and the comparison function is correctly implemented
            // at the end we will have left == left_rev + 1, and right == right_rev + 1,
            // fully consuming the input having written it to dst.

            let len = v.len();
            let src = v.as_ptr();

            let len_div_2 = len / 2;

            // SAFETY: The caller has to ensure that len >= 2.
            unsafe {
                hint::assert_unchecked(len_div_2 != 0); // This can avoid useless code-gen.
            }

            // SAFETY: no matter what the result of the user-provided comparison function
            // is, all 4 read pointers will always be in-bounds. Writing `dst` and `dst_rev`
            // will always be in bounds if the caller guarantees that `dst` is valid for
            // `v.len()` writes.
            unsafe {
                let mut left = src;
                let mut right = src.add(len_div_2);
                let mut dst = dst;

                let mut left_rev = src.add(len_div_2 - 1);
                let mut right_rev = src.add(len - 1);
                let mut dst_rev = dst.add(len - 1);

                for _ in 0..len_div_2 {
                    (left, right, dst) = merge_up(left, right, dst, is_less);
                    (left_rev, right_rev, dst_rev) =
                        merge_down(left_rev, right_rev, dst_rev, is_less);
                }

                let left_end = left_rev.wrapping_add(1);
                let right_end = right_rev.wrapping_add(1);

                // Odd length, so one element is left unconsumed in the input.
                if len % 2 != 0 {
                    let left_nonempty = left < left_end;
                    let last_src = if left_nonempty { left } else { right };
                    ptr::copy_nonoverlapping(last_src, dst, 1);
                    left = left.add(left_nonempty as usize);
                    right = right.add((!left_nonempty) as usize);
                }

                // We now should have consumed the full input exactly once. This can only fail if the
                // user-provided comparison function fails to implement a strict weak ordering. In that case
                // we panic and never access the inconsistent state in dst.
                if left != left_end || right != right_end {
                    panic_on_ord_violation();
                }
            }
        }

        #[inline(never)]
        #[cold]
        fn panic_on_ord_violation() -> ! {
            // This is indicative of a logic bug in the user-provided comparison function or Ord
            // implementation. They are expected to implement a total order as explained in the Ord
            // documentation.
            //
            // By panicking we inform the user, that they have a logic bug in their program. If a strict
            // weak ordering is not given, the concept of comparison based sorting cannot yield a sorted
            // result. E.g.: a < b < c < a
            //
            // The Ord documentation requires users to implement a total order. Arguably that's
            // unnecessarily strict in the context of sorting. Issues only arise if the weaker requirement
            // of a strict weak ordering is violated.
            //
            // The panic message talks about a total order because that's what the Ord documentation talks
            // about and requires, so as to not confuse users.
            panic!("user-provided comparison function does not correctly implement a total order");
        }

        #[must_use]
        pub(crate) const fn has_efficient_in_place_swap<T>() -> bool {
            // Heuristic that holds true on all tested 64-bit capable architectures.
            size_of::<T>() <= 8 // size_of::<u64>()
        }
    }
}

mod stable {

    pub(crate) mod drift {
        //! This module contains the hybrid top-level loop combining bottom-up Mergesort with top-down
        //! Quicksort.

        use crate::mem::MaybeUninit;
        use crate::slice::sort::shared::find_existing_run;

        use crate::cmp;
        use crate::slice::sort::stable::merge::merge;
        use crate::slice::sort::stable::quicksort::quicksort;

        /// Sorts `v` based on comparison function `is_less`. If `eager_sort` is true,
        /// it will only do small-sorts and physical merges, ensuring O(N * log(N))
        /// worst-case complexity. `scratch.len()` must be at least
        /// `max(v.len() - v.len() / 2, 48)` otherwise the implementation may panic.
        /// Fully ascending and descending inputs will be sorted with exactly N - 1
        /// comparisons.
        ///
        /// This is the main loop for driftsort, which uses powersort's heuristic to
        /// determine in which order to merge runs, see below for details.
        pub fn sort<T, F: FnMut(&T, &T) -> bool>(
            v: &mut [T],
            scratch: &mut [MaybeUninit<T>],
            eager_sort: bool,
            is_less: &mut F,
        ) {
            let len = v.len();
            if len < 2 {
                return; // Removing this length check *increases* code size.
            }
            let scale_factor = merge_tree_scale_factor(len);

            // It's important to have a relatively high entry barrier for pre-sorted
            // runs, as the presence of a single such run will force on average several
            // merge operations and shrink the maximum quicksort size a lot. For that
            // reason we use sqrt(len) as our pre-sorted run threshold.
            const MIN_SQRT_RUN_LEN: usize = 64;
            let min_good_run_len = if len <= (MIN_SQRT_RUN_LEN * MIN_SQRT_RUN_LEN) {
                // For small input length `MIN_SQRT_RUN_LEN` would break pattern
                // detection of full or nearly sorted inputs.
                cmp::min(len - len / 2, MIN_SQRT_RUN_LEN)
            } else {
                sqrt_approx(len)
            };

            // (stack_len, runs, desired_depths) together form a stack maintaining run
            // information for the powersort heuristic. desired_depths[i] is the desired
            // depth of the merge node that merges runs[i] with the run that comes after
            // it.
            let mut stack_len = 0;
            let mut run_storage = MaybeUninit::<[DriftsortRun; 66]>::uninit();
            let runs: *mut DriftsortRun = run_storage.as_mut_ptr().cast();
            let mut desired_depth_storage = MaybeUninit::<[u8; 66]>::uninit();
            let desired_depths: *mut u8 = desired_depth_storage.as_mut_ptr().cast();

            let mut scan_idx = 0;
            let mut prev_run = DriftsortRun::new_sorted(0); // Initial dummy run.
            loop {
                // Compute the next run and the desired depth of the merge node between
                // prev_run and next_run. On the last iteration we create a dummy run
                // with root-level desired depth to fully collapse the merge tree.
                let (next_run, desired_depth);
                if scan_idx < len {
                    next_run = create_run(
                        &mut v[scan_idx..],
                        scratch,
                        min_good_run_len,
                        eager_sort,
                        is_less,
                    );
                    desired_depth = merge_tree_depth(
                        scan_idx - prev_run.len(),
                        scan_idx,
                        scan_idx + next_run.len(),
                        scale_factor,
                    );
                } else {
                    next_run = DriftsortRun::new_sorted(0);
                    desired_depth = 0;
                };

                // Process the merge nodes between earlier runs[i] that have a desire to
                // be deeper in the merge tree than the merge node for the splitpoint
                // between prev_run and next_run.
                //
                // SAFETY: first note that this is the only place we modify stack_len,
                // runs or desired depths. We maintain the following invariants:
                //  1. The first stack_len elements of runs/desired_depths are initialized.
                //  2. For all valid i > 0, desired_depths[i] < desired_depths[i+1].
                //  3. The sum of all valid runs[i].len() plus prev_run.len() equals
                //     scan_idx.
                unsafe {
                    while stack_len > 1 && *desired_depths.add(stack_len - 1) >= desired_depth {
                        // Desired depth greater than the upcoming desired depth, pop
                        // left neighbor run from stack and merge into prev_run.
                        let left = *runs.add(stack_len - 1);
                        let merged_len = left.len() + prev_run.len();
                        let merge_start_idx = scan_idx - merged_len;
                        let merge_slice = v.get_unchecked_mut(merge_start_idx..scan_idx);
                        prev_run = logical_merge(merge_slice, scratch, left, prev_run, is_less);
                        stack_len -= 1;
                    }

                    // We now know that desired_depths[stack_len - 1] < desired_depth,
                    // maintaining our invariant. This also guarantees we don't overflow
                    // the stack as merge_tree_depth(..) <= 64 and thus we can only have
                    // 64 distinct values on the stack before pushing, plus our initial
                    // dummy run, while our capacity is 66.
                    *runs.add(stack_len) = prev_run;
                    *desired_depths.add(stack_len) = desired_depth;
                    stack_len += 1;
                }

                // Break before overriding the last run with our dummy run.
                if scan_idx >= len {
                    break;
                }

                scan_idx += next_run.len();
                prev_run = next_run;
            }

            if !prev_run.sorted() {
                stable_quicksort(v, scratch, is_less);
            }
        }

        // Nearly-Optimal Mergesorts: Fast, Practical Sorting Methods That Optimally
        // Adapt to Existing Runs by J. Ian Munro and Sebastian Wild.
        //
        // This method forms a binary merge tree, where each internal node corresponds
        // to a splitting point between the adjacent runs that have to be merged. If we
        // visualize our array as the number line from 0 to 1, we want to find the
        // dyadic fraction with smallest denominator that lies between the midpoints of
        // our to-be-merged slices. The exponent in the dyadic fraction indicates the
        // desired depth in the binary merge tree this internal node wishes to have.
        // This does not always correspond to the actual depth due to the inherent
        // imbalance in runs, but we follow it as closely as possible.
        //
        // As an optimization we rescale the number line from [0, 1) to [0, 2^62). Then
        // finding the simplest dyadic fraction between midpoints corresponds to finding
        // the most significant bit difference of the midpoints. We save scale_factor =
        // ceil(2^62 / n) to perform this rescaling using a multiplication, avoiding
        // having to repeatedly do integer divides. This rescaling isn't exact when n is
        // not a power of two since we use integers and not reals, but the result is
        // very close, and in fact when n < 2^30 the resulting tree is equivalent as the
        // approximation errors stay entirely in the lower order bits.
        //
        // Thus for the splitting point between two adjacent slices [a, b) and [b, c)
        // the desired depth of the corresponding merge node is CLZ((a+b)*f ^ (b+c)*f),
        // where CLZ counts the number of leading zeros in an integer and f is our scale
        // factor. Note that we omitted the division by two in the midpoint
        // calculations, as this simply shifts the bits by one position (and thus always
        // adds one to the result), and we only care about the relative depths.
        //
        // Finally, if we try to upper bound x = (a+b)*f giving x = (n-1 + n) * ceil(2^62 / n) then
        //    x < (2^62 / n + 1) * 2n
        //    x < 2^63 + 2n
        // So as long as n < 2^62 we find that x < 2^64, meaning our operations do not
        // overflow.
        #[inline(always)]
        fn merge_tree_scale_factor(n: usize) -> u64 {
            if usize::BITS > u64::BITS {
                panic!("Platform not supported");
            }

            (1u64 << 62).div_ceil(n as u64)
        }

        // Note: merge_tree_depth output is < 64 when left < right as f*x and f*y must
        // differ in some bit, and is <= 64 always.
        #[inline(always)]
        fn merge_tree_depth(left: usize, mid: usize, right: usize, scale_factor: u64) -> u8 {
            let x = left as u64 + mid as u64;
            let y = mid as u64 + right as u64;
            ((scale_factor * x) ^ (scale_factor * y)).leading_zeros() as u8
        }

        fn sqrt_approx(n: usize) -> usize {
            // Note that sqrt(n) = n^(1/2), and that 2^log2(n) = n. We combine these
            // two facts to approximate sqrt(n) as 2^(log2(n) / 2). Because our integer
            // log floors we want to add 0.5 to compensate for this on average, so our
            // initial approximation is 2^((1 + floor(log2(n))) / 2).
            //
            // We then apply an iteration of Newton's method to improve our
            // approximation, which for sqrt(n) is a1 = (a0 + n / a0) / 2.
            //
            // Finally we note that the exponentiation / division can be done directly
            // with shifts. We OR with 1 to avoid zero-checks in the integer log.
            let ilog = (n | 1).ilog2();
            let shift = ilog.div_ceil(2);
            ((1 << shift) + (n >> shift)) / 2
        }

        // Lazy logical runs as in Glidesort.
        #[inline(always)]
        fn logical_merge<T, F: FnMut(&T, &T) -> bool>(
            v: &mut [T],
            scratch: &mut [MaybeUninit<T>],
            left: DriftsortRun,
            right: DriftsortRun,
            is_less: &mut F,
        ) -> DriftsortRun {
            // If one or both of the runs are sorted do a physical merge, using
            // quicksort to sort the unsorted run if present. We also *need* to
            // physically merge if the combined runs would not fit in the scratch space
            // anymore (as this would mean we are no longer able to quicksort them).
            let len = v.len();
            let can_fit_in_scratch = len <= scratch.len();
            if !can_fit_in_scratch || left.sorted() || right.sorted() {
                if !left.sorted() {
                    stable_quicksort(&mut v[..left.len()], scratch, is_less);
                }
                if !right.sorted() {
                    stable_quicksort(&mut v[left.len()..], scratch, is_less);
                }
                merge(v, scratch, left.len(), is_less);

                DriftsortRun::new_sorted(len)
            } else {
                DriftsortRun::new_unsorted(len)
            }
        }

        /// Creates a new logical run.
        ///
        /// A logical run can either be sorted or unsorted. If there is a pre-existing
        /// run that clears the `min_good_run_len` threshold it is returned as a sorted
        /// run. If not, the result depends on the value of `eager_sort`. If it is true,
        /// then a sorted run up to the type's small-sort threshold is returned, and if it
        /// is false an unsorted run of length `min_good_run_len` is returned.
        fn create_run<T, F: FnMut(&T, &T) -> bool>(
            v: &mut [T],
            scratch: &mut [MaybeUninit<T>],
            min_good_run_len: usize,
            eager_sort: bool,
            is_less: &mut F,
        ) -> DriftsortRun {
            let len = v.len();
            if len >= min_good_run_len {
                let (run_len, was_reversed) = find_existing_run(v, is_less);

                if run_len >= min_good_run_len {
                    if was_reversed {
                        v[..run_len].reverse();
                    }

                    return DriftsortRun::new_sorted(run_len);
                }
            }

            if eager_sort {
                // We call quicksort with a len that will immediately call small-sort.
                // By not calling the small-sort directly here it can always be inlined into
                // the quicksort itself, making the recursive base case faster and is generally
                // more binary-size efficient.
                let eager_run_len = cmp::min(
                    crate::slice::sort::shared::smallsort::stable_small_sort_threshold::<T>(),
                    len,
                );
                quicksort(&mut v[..eager_run_len], scratch, 0, None, is_less);
                DriftsortRun::new_sorted(eager_run_len)
            } else {
                DriftsortRun::new_unsorted(cmp::min(min_good_run_len, len))
            }
        }

        fn stable_quicksort<T, F: FnMut(&T, &T) -> bool>(
            v: &mut [T],
            scratch: &mut [MaybeUninit<T>],
            is_less: &mut F,
        ) {
            // Limit the number of imbalanced partitions to `2 * floor(log2(len))`.
            // The binary OR by one is used to eliminate the zero-check in the logarithm.
            let limit = 2 * (v.len() | 1).ilog2();
            quicksort(v, scratch, limit, None, is_less);
        }

        /// Compactly stores the length of a run, and whether or not it is sorted. This
        /// can always fit in a `usize` because the maximum slice length is [`isize::MAX`].
        #[derive(Copy, Clone)]
        struct DriftsortRun(usize);

        impl DriftsortRun {
            #[inline(always)]
            fn new_sorted(length: usize) -> Self {
                Self((length << 1) | 1)
            }

            #[inline(always)]
            fn new_unsorted(length: usize) -> Self {
                Self(length << 1)
            }

            #[inline(always)]
            fn sorted(self) -> bool {
                self.0 & 1 == 1
            }

            #[inline(always)]
            fn len(self) -> usize {
                self.0 >> 1
            }
        }
    }

    pub(crate) mod merge {
        //! This module contains logic for performing a merge of two sorted sub-slices.

        use crate::mem::MaybeUninit;
        use crate::{cmp, ptr};

        /// Merges non-decreasing runs `v[..mid]` and `v[mid..]` using `scratch` as
        /// temporary storage, and stores the result into `v[..]`.
        pub fn merge<T, F: FnMut(&T, &T) -> bool>(
            v: &mut [T],
            scratch: &mut [MaybeUninit<T>],
            mid: usize,
            is_less: &mut F,
        ) {
            let len = v.len();

            if mid == 0 || mid >= len || scratch.len() < cmp::min(mid, len - mid) {
                return;
            }

            // SAFETY: We checked that the two slices are non-empty and `mid` is in-bounds.
            // We checked that the buffer `scratch` has enough capacity to hold a copy of
            // the shorter slice. `merge_up` and `merge_down` are written in such a way that
            // they uphold the contract described in `MergeState::drop`.
            unsafe {
                // The merge process first copies the shorter run into `buf`. Then it traces
                // the newly copied run and the longer run forwards (or backwards), comparing
                // their next unconsumed elements and copying the lesser (or greater) one into `v`.
                //
                // As soon as the shorter run is fully consumed, the process is done. If the
                // longer run gets consumed first, then we must copy whatever is left of the
                // shorter run into the remaining gap in `v`.
                //
                // Intermediate state of the process is always tracked by `gap`, which serves
                // two purposes:
                //  1. Protects integrity of `v` from panics in `is_less`.
                //  2. Fills the remaining gap in `v` if the longer run gets consumed first.

                let buf = scratch.as_mut_ptr() as *mut T;

                let v_base = v.as_mut_ptr();
                let v_mid = v_base.add(mid);
                let v_end = v_base.add(len);

                let left_len = mid;
                let right_len = len - mid;

                let left_is_shorter = left_len <= right_len;
                let save_base = if left_is_shorter { v_base } else { v_mid };
                let save_len = if left_is_shorter { left_len } else { right_len };

                ptr::copy_nonoverlapping(save_base, buf, save_len);

                let mut merge_state = MergeState {
                    start: buf,
                    end: buf.add(save_len),
                    dst: save_base,
                };

                if left_is_shorter {
                    merge_state.merge_up(v_mid, v_end, is_less);
                } else {
                    merge_state.merge_down(v_base, buf, v_end, is_less);
                }
                // Finally, `merge_state` gets dropped. If the shorter run was not fully
                // consumed, whatever remains of it will now be copied into the hole in `v`.
            }
        }

        // When dropped, copies the range `start..end` into `dst..`.
        struct MergeState<T> {
            start: *mut T,
            end: *mut T,
            dst: *mut T,
        }

        impl<T> MergeState<T> {
            /// # Safety
            /// The caller MUST guarantee that `self` is initialized in a way where `start -> end` is
            /// the longer sub-slice and so that `dst` can be written to at least the shorter sub-slice
            /// length times. In addition `start -> end` and `right -> right_end` MUST be valid to be
            /// read. This function MUST only be called once.
            unsafe fn merge_up<F: FnMut(&T, &T) -> bool>(
                &mut self,
                mut right: *const T,
                right_end: *const T,
                is_less: &mut F,
            ) {
                // SAFETY: See function safety comment.
                unsafe {
                    let left = &mut self.start;
                    let out = &mut self.dst;

                    while *left != self.end && right as *const T != right_end {
                        let consume_left = !is_less(&*right, &**left);

                        let src = if consume_left { *left } else { right };
                        ptr::copy_nonoverlapping(src, *out, 1);

                        *left = left.add(consume_left as usize);
                        right = right.add(!consume_left as usize);

                        *out = out.add(1);
                    }
                }
            }

            /// # Safety
            /// The caller MUST guarantee that `self` is initialized in a way where `left_end <- dst` is
            /// the shorter sub-slice and so that `out` can be written to at least the shorter sub-slice
            /// length times. In addition `left_end <- dst` and `right_end <- end` MUST be valid to be
            /// read. This function MUST only be called once.
            unsafe fn merge_down<F: FnMut(&T, &T) -> bool>(
                &mut self,
                left_end: *const T,
                right_end: *const T,
                mut out: *mut T,
                is_less: &mut F,
            ) {
                // SAFETY: See function safety comment.
                unsafe {
                    loop {
                        let left = self.dst.sub(1);
                        let right = self.end.sub(1);
                        out = out.sub(1);

                        let consume_left = is_less(&*right, &*left);

                        let src = if consume_left { left } else { right };
                        ptr::copy_nonoverlapping(src, out, 1);

                        self.dst = left.add(!consume_left as usize);
                        self.end = right.add(consume_left as usize);

                        if self.dst as *const T == left_end || self.end as *const T == right_end {
                            break;
                        }
                    }
                }
            }
        }

        impl<T> Drop for MergeState<T> {
            fn drop(&mut self) {
                // SAFETY: The user of MergeState MUST ensure, that at any point this drop
                // impl MAY run, for example when the user provided `is_less` panics, that
                // copying the contiguous region between `start` and `end` to `dst` will
                // leave the input slice `v` with each original element and all possible
                // modifications observed.
                unsafe {
                    let len = self.end.offset_from_unsigned(self.start);
                    ptr::copy_nonoverlapping(self.start, self.dst, len);
                }
            }
        }
    }

    pub(crate) mod quicksort {
        //! This module contains a stable quicksort and partition implementation.

        use crate::mem::MaybeUninit;

        use crate::slice::sort::is_freeze;
        use crate::slice::sort::shared::pivot::choose_pivot;
        use crate::slice::sort::shared::smallsort;
        use crate::{mem::size_of, ptr};

        /// Sorts `v` recursively using quicksort.
        /// Scratch must hold the entire run for partitioning. The caller only passes
        /// logical runs that fit; eager driftsort is the bounded-depth fallback.
        ///
        /// `limit` when initialized with `c*log(v.len())` for some c ensures we do not
        /// overflow the stack or go quadratic.
        #[inline(never)]
        pub fn quicksort<T, F: FnMut(&T, &T) -> bool>(
            mut v: &mut [T],
            scratch: &mut [MaybeUninit<T>],
            mut limit: u32,
            mut left_ancestor_pivot: Option<&T>,
            is_less: &mut F,
        ) {
            loop {
                let len = v.len();

                if len <= smallsort::stable_small_sort_threshold::<T>() {
                    smallsort::stable_small_sort(v, scratch, is_less);
                    return;
                }

                if limit == 0 {
                    // We have had too many bad pivots, switch to O(n log n) fallback
                    // algorithm. In our case that is driftsort in eager mode.
                    crate::slice::sort::stable::drift::sort(v, scratch, true, is_less);
                    return;
                }
                limit -= 1;

                let pivot_pos = choose_pivot(v, is_less);

                // SAFETY: We only access the temporary copy for Freeze types, otherwise
                // self-modifications via `is_less` would not be observed and this would
                // be unsound. Our temporary copy does not escape this scope.
                // We use `MaybeUninit` to avoid re-tag issues. FIXME: use `MaybeDangling`.
                let pivot_copy =
                    unsafe { ptr::read(v.as_ptr().add(pivot_pos) as *const MaybeUninit<T>) };
                let pivot_ref =
                    // SAFETY: We created the value in an init state.
                    if is_freeze::<T>() { Some(unsafe { &*pivot_copy.as_ptr() }) } else { None };

                // We choose a pivot, and check if this pivot is equal to our left
                // ancestor. If true, we do a partition putting equal elements on the
                // left and do not recurse on it. This gives O(n log k) sorting for k
                // distinct values, a strategy borrowed from pdqsort. For types with
                // interior mutability we can't soundly create a temporary copy of the
                // ancestor pivot, and use left_partition_len == 0 as our method for
                // detecting when we re-use a pivot, which means we do at most three
                // partition operations with pivot p instead of the optimal two.
                let mut perform_equal_partition = false;
                if let Some(la_pivot) = left_ancestor_pivot {
                    perform_equal_partition = !is_less(la_pivot, &v[pivot_pos]);
                }

                let mut left_partition_len = 0;
                if !perform_equal_partition {
                    left_partition_len = stable_partition(v, scratch, pivot_pos, false, is_less);
                    perform_equal_partition = left_partition_len == 0;
                }

                if perform_equal_partition {
                    let mid_eq =
                        stable_partition(v, scratch, pivot_pos, true, &mut |a, b| !is_less(b, a));
                    v = &mut v[mid_eq..];
                    left_ancestor_pivot = None;
                    continue;
                }

                // Process left side with the next loop iter, right side with recursion.
                let (left, right) = v.split_at_mut(left_partition_len);
                quicksort(right, scratch, limit, pivot_ref, is_less);
                v = left;
            }
        }

        /// Partitions `v` using pivot `p = v[pivot_pos]` and returns the number of
        /// elements less than `p`. The relative order of elements that compare < p and
        /// those that compare >= p is preserved - it is a stable partition.
        ///
        /// If `is_less` is not a strict total order or panics, `scratch.len() < v.len()`,
        /// or `pivot_pos >= v.len()`, the result and `v`'s state is sound but unspecified.
        fn stable_partition<T, F: FnMut(&T, &T) -> bool>(
            v: &mut [T],
            scratch: &mut [MaybeUninit<T>],
            pivot_pos: usize,
            pivot_goes_left: bool,
            is_less: &mut F,
        ) -> usize {
            let len = v.len();

            if scratch.len() < len || pivot_pos >= len {
                crate::panicking::panic_str("sort scratch too small")
            }

            let v_base = v.as_ptr();
            let scratch_base = scratch.as_mut_ptr() as *mut T;

            // The core idea is to write the values that compare as less-than to the left
            // side of `scratch`, while the values that compared as greater or equal than
            // `v[pivot_pos]` go to the right side of `scratch` in reverse. See
            // PartitionState for details.

            // SAFETY: see individual comments.
            unsafe {
                // SAFETY: we made sure the scratch has length >= len and that pivot_pos
                // is in-bounds. v and scratch are disjoint slices.
                let pivot = v_base.add(pivot_pos);
                let mut state = PartitionState::new(v_base, scratch_base, len);

                let mut pivot_in_scratch = ptr::null_mut();
                let mut loop_end_pos = pivot_pos;

                // SAFETY: this loop is equivalent to calling state.partition_one
                // exactly len times.
                loop {
                    // Ideally the outer loop won't be unrolled, to save binary size,
                    // but we do want the inner loop to be unrolled for small types, as
                    // this gave significant performance boosts in benchmarks. Unrolling
                    // through for _ in 0..UNROLL_LEN { .. } instead of manually improves
                    // compile times but has a ~10-20% performance penalty on opt-level=s.
                    if size_of::<T>() <= 16 {
                        const UNROLL_LEN: usize = 4;
                        let unroll_end = v_base.add(loop_end_pos.saturating_sub(UNROLL_LEN - 1));
                        while state.scan < unroll_end {
                            state.partition_one(is_less(&*state.scan, &*pivot));
                            state.partition_one(is_less(&*state.scan, &*pivot));
                            state.partition_one(is_less(&*state.scan, &*pivot));
                            state.partition_one(is_less(&*state.scan, &*pivot));
                        }
                    }

                    let loop_end = v_base.add(loop_end_pos);
                    while state.scan < loop_end {
                        state.partition_one(is_less(&*state.scan, &*pivot));
                    }

                    if loop_end_pos == len {
                        break;
                    }

                    // We avoid comparing pivot with itself, as this could create deadlocks for
                    // certain comparison operators. We also store its location later for later.
                    pivot_in_scratch = state.partition_one(pivot_goes_left);

                    loop_end_pos = len;
                }

                // `pivot` must be copied into its correct position again, because a
                // comparison operator might have modified it.
                if !is_freeze::<T>() {
                    ptr::copy_nonoverlapping(pivot, pivot_in_scratch, 1);
                }

                // SAFETY: partition_one being called exactly len times guarantees that scratch
                // is initialized with a permuted copy of `v`, and that num_left <= v.len().
                // Copying scratch[0..num_left] and scratch[num_left..v.len()] back is thus
                // sound, as the values in scratch will never be read again, meaning our copies
                // semantically act as moves, permuting `v`.

                // Copy all the elements < p directly from swap to v.
                let v_base = v.as_mut_ptr();
                ptr::copy_nonoverlapping(scratch_base, v_base, state.num_left);

                // Copy the elements >= p in reverse order.
                for i in 0..len - state.num_left {
                    ptr::copy_nonoverlapping(
                        scratch_base.add(len - 1 - i),
                        v_base.add(state.num_left + i),
                        1,
                    );
                }

                state.num_left
            }
        }

        struct PartitionState<T> {
            // The start of the scratch auxiliary memory.
            scratch_base: *mut T,
            // The current element that is being looked at, scans left to right through slice.
            scan: *const T,
            // Counts the number of elements that went to the left side, also works around:
            // https://github.com/rust-lang/rust/issues/117128
            num_left: usize,
            // Reverse scratch output pointer.
            scratch_rev: *mut T,
        }

        impl<T> PartitionState<T> {
            /// # Safety
            ///
            /// `scan` and `scratch` must point to valid disjoint buffers of length `len`. The
            /// scan buffer must be initialized.
            unsafe fn new(scan: *const T, scratch: *mut T, len: usize) -> Self {
                // SAFETY: See function safety comment.
                unsafe {
                    Self {
                        scratch_base: scratch,
                        scan,
                        num_left: 0,
                        scratch_rev: scratch.add(len),
                    }
                }
            }

            /// Depending on the value of `towards_left` this function will write a value
            /// to the growing left or right side of the scratch memory. This forms the
            /// branchless core of the partition.
            ///
            /// # Safety
            ///
            /// This function may be called at most `len` times. If it is called exactly
            /// `len` times the scratch buffer then contains a copy of each element from
            /// the scan buffer exactly once - a permutation, and num_left <= len.
            unsafe fn partition_one(&mut self, towards_left: bool) -> *mut T {
                // SAFETY: see individual comments.
                unsafe {
                    // SAFETY: in-bounds because this function is called at most len times, and thus
                    // right now is incremented at most len - 1 times. Similarly, num_left < len and
                    // num_right < len, where num_right == i - num_left at the start of the ith
                    // iteration (zero-indexed).
                    self.scratch_rev = self.scratch_rev.sub(1);

                    // SAFETY: now we have scratch_rev == base + len - (i + 1). This means
                    // scratch_rev + num_left == base + len - 1 - num_right < base + len.
                    let dst_base = if towards_left {
                        self.scratch_base
                    } else {
                        self.scratch_rev
                    };
                    let dst = dst_base.add(self.num_left);
                    ptr::copy_nonoverlapping(self.scan, dst, 1);

                    self.num_left += towards_left as usize;
                    self.scan = self.scan.add(1);
                    dst
                }
            }
        }
    }

    pub(crate) mod tiny {
        //! Binary-size optimized mergesort inspired by https://github.com/voultapher/tiny-sort-rs.

        use crate::mem::MaybeUninit;
        use crate::ptr;
        use crate::slice::sort::stable::merge;

        /// Tiny recursive top-down merge sort optimized for binary size. It has no adaptiveness whatsoever,
        /// no run detection, etc.
        #[inline(always)]
        pub fn mergesort<T, F: FnMut(&T, &T) -> bool>(
            v: &mut [T],
            scratch: &mut [MaybeUninit<T>],
            is_less: &mut F,
        ) {
            let len = v.len();

            if len > 2 {
                let mid = len / 2;

                // SAFETY: mid is in-bounds.
                unsafe {
                    // Sort the left half recursively.
                    mergesort(v.get_unchecked_mut(..mid), scratch, is_less);
                    // Sort the right half recursively.
                    mergesort(v.get_unchecked_mut(mid..), scratch, is_less);
                }

                merge::merge(v, scratch, mid, is_less);
            } else if len == 2 {
                // SAFETY: We checked the len, the pointers we create are valid and don't overlap.
                unsafe {
                    let v_base = v.as_mut_ptr();
                    let v_a = v_base;
                    let v_b = v_base.add(1);

                    if is_less(&*v_b, &*v_a) {
                        ptr::swap_nonoverlapping(v_a, v_b, 1);
                    }
                }
            }
        }
    }
}

mod unstable {

    pub(crate) mod quicksort {
        //! This module contains an unstable quicksort and two partition implementations.

        use crate::mem;
        use crate::mem::ManuallyDrop;
        use crate::slice::sort::shared::pivot::choose_pivot;
        use crate::slice::sort::shared::smallsort;
        use crate::slice::sort::unstable::heapsort;
        use crate::{mem::size_of, ptr};

        /// Sorts `v` recursively.
        ///
        /// If the slice had a predecessor in the original array, it is specified as `ancestor_pivot`.
        ///
        /// `limit` is the number of allowed imbalanced partitions before switching to `heapsort`. If zero,
        /// this function will immediately switch to heapsort.
        pub(crate) fn quicksort<'a, T, F>(
            mut v: &'a mut [T],
            mut ancestor_pivot: Option<&'a T>,
            mut limit: u32,
            is_less: &mut F,
        ) where
            F: FnMut(&T, &T) -> bool,
        {
            loop {
                if v.len() <= smallsort::unstable_small_sort_threshold::<T>() {
                    smallsort::unstable_small_sort(v, is_less);
                    return;
                }

                // If too many bad pivot choices were made, simply fall back to heapsort in order to
                // guarantee `O(N x log(N))` worst-case.
                if limit == 0 {
                    heapsort::heapsort(v, is_less);
                    return;
                }

                limit -= 1;

                // Choose a pivot and try guessing whether the slice is already sorted.
                let pivot_pos = choose_pivot(v, is_less);

                // If the chosen pivot is equal to the predecessor, then it's the smallest element in the
                // slice. Partition the slice into elements equal to and elements greater than the pivot.
                // This case is usually hit when the slice contains many duplicate elements.
                if let Some(p) = ancestor_pivot {
                    if !is_less(p, &v[pivot_pos]) {
                        let num_lt = partition(v, pivot_pos, &mut |a, b| !is_less(b, a));

                        // Continue sorting elements greater than the pivot. We know that `num_lt` contains
                        // the pivot. So we can continue after `num_lt`.
                        v = &mut v[(num_lt + 1)..];
                        ancestor_pivot = None;
                        continue;
                    }
                }

                // Partition the slice.
                let num_lt = partition(v, pivot_pos, is_less);

                // Split the slice into `left`, `pivot`, and `right`.
                let (left, right) = v.split_at_mut(num_lt);
                let (pivot, right) = right.split_at_mut(1);
                let pivot = &pivot[0];

                // Recurse into the left side. We have a fixed recursion limit, testing shows no real
                // benefit for recursing into the shorter side.
                quicksort(left, ancestor_pivot, limit, is_less);

                // Continue with the right side.
                v = right;
                ancestor_pivot = Some(pivot);
            }
        }

        /// Takes the input slice `v` and re-arranges elements such that when the call returns normally
        /// all elements that compare true for `is_less(elem, pivot)` where `pivot == v[pivot_pos]` are
        /// on the left side of `v` followed by the other elements, notionally considered greater or
        /// equal to `pivot`.
        ///
        /// Returns the number of elements that are compared true for `is_less(elem, pivot)`.
        ///
        /// If `is_less` does not implement a total order the resulting order and return value are
        /// unspecified. All original elements will remain in `v` and any possible modifications via
        /// interior mutability will be observable. Same is true if `is_less` panics or `v.len()`
        /// is empty. This in-place partition needs no scratch allocation.
        pub(crate) fn partition<T, F>(v: &mut [T], pivot: usize, is_less: &mut F) -> usize
        where
            F: FnMut(&T, &T) -> bool,
        {
            let len = v.len();

            // Allows for panic-free code-gen by proving this property to the compiler.
            if len == 0 {
                return 0;
            }

            if pivot >= len {
                crate::panicking::panic_str("sort invariant violated");
            }

            // Place the pivot at the beginning of slice.
            v.swap(0, pivot);
            let (pivot, v_without_pivot) = v.split_at_mut(1);

            // Assuming that Rust generates noalias LLVM IR we can be sure that a partition function
            // signature of the form `(v: &mut [T], pivot: &T)` guarantees that pivot and v can't alias.
            // Having this guarantee is crucial for optimizations. It's possible to copy the pivot value
            // into a stack value, but this creates issues for types with interior mutability mandating
            // a drop guard.
            let pivot = &mut pivot[0];

            // Ordinary type-size dispatch replaces core's const function-pointer selection.
            // Small movable types use branchless Lomuto, large types use cyclic Hoare.
            let num_lt = if size_of::<T>() <= 96 {
                partition_lomuto_branchless_cyclic(v_without_pivot, pivot, is_less)
            } else {
                partition_hoare_branchy_cyclic(v_without_pivot, pivot, is_less)
            };

            if num_lt >= len {
                crate::panicking::panic_str("sort invariant violated");
            }

            // Place the pivot between the two partitions.
            v.swap(0, num_lt);

            num_lt
        }

        /// See [`partition`].
        fn partition_hoare_branchy_cyclic<T, F>(v: &mut [T], pivot: &T, is_less: &mut F) -> usize
        where
            F: FnMut(&T, &T) -> bool,
        {
            let len = v.len();

            if len == 0 {
                return 0;
            }

            // Optimized for large types that are expensive to move. Not optimized for integers. Optimized
            // for small code-gen, assuming that is_less is an expensive operation that generates
            // substantial amounts of code or a call. And that copying elements will likely be a call to
            // memcpy. Using 2 `ptr::copy_nonoverlapping` has the chance to be faster than
            // `ptr::swap_nonoverlapping` because `memcpy` can use wide SIMD based on runtime feature
            // detection. Benchmarks support this analysis.

            let mut gap_opt: Option<GapGuard<T>> = None;

            // SAFETY: The left-to-right scanning loop performs a bounds check, where we know that `left >=
            // v_base && left < right && right <= v_base.add(len)`. The right-to-left scanning loop performs
            // a bounds check ensuring that `right` is in-bounds. We checked that `len` is more than zero,
            // which means that unconditional `right = right.sub(1)` is safe to do. The exit check makes
            // sure that `left` and `right` never alias, making `ptr::copy_nonoverlapping` safe. The
            // drop-guard `gap` ensures that should `is_less` panic we always overwrite the duplicate in the
            // input. `gap.pos` stores the previous value of `right` and starts at `right` and so it too is
            // in-bounds. We never pass the saved `gap.value` to `is_less` while it is inside the `GapGuard`
            // thus any changes via interior mutability will be observed.
            unsafe {
                let v_base = v.as_mut_ptr();

                let mut left = v_base;
                let mut right = v_base.add(len);

                loop {
                    // Find the first element greater than the pivot.
                    while left < right && is_less(&*left, pivot) {
                        left = left.add(1);
                    }

                    // Find the last element equal to the pivot.
                    loop {
                        right = right.sub(1);
                        if left >= right || is_less(&*right, pivot) {
                            break;
                        }
                    }

                    if left >= right {
                        break;
                    }

                    // Swap the found pair of out-of-order elements via cyclic permutation.
                    let is_first_swap_pair = gap_opt.is_none();

                    if is_first_swap_pair {
                        gap_opt = Some(GapGuard {
                            pos: right,
                            value: ManuallyDrop::new(ptr::read(left)),
                        });
                    }

                    let gap = gap_opt.as_mut().unwrap_unchecked();

                    // Single place where we instantiate ptr::copy_nonoverlapping in the partition.
                    if !is_first_swap_pair {
                        ptr::copy_nonoverlapping(left, gap.pos, 1);
                    }
                    gap.pos = right;
                    ptr::copy_nonoverlapping(right, left, 1);

                    left = left.add(1);
                }

                left.offset_from_unsigned(v_base)

                // `gap_opt` goes out of scope and overwrites the last wrong-side element on the right side
                // with the first wrong-side element of the left side that was initially overwritten by the
                // first wrong-side element on the right side element.
            }
        }

        struct PartitionState<T> {
            // The current element that is being looked at, scans left to right through slice.
            right: *mut T,
            // Counts the number of elements that compared less-than, also works around:
            // https://github.com/rust-lang/rust/issues/117128
            num_lt: usize,
            // Gap guard that tracks the temporary duplicate in the input.
            gap: GapGuardRaw<T>,
        }

        fn partition_lomuto_branchless_cyclic<T, F>(
            v: &mut [T],
            pivot: &T,
            is_less: &mut F,
        ) -> usize
        where
            F: FnMut(&T, &T) -> bool,
        {
            // Novel partition implementation by Lukas Bergdoll and Orson Peters. Branchless Lomuto
            // partition paired with a cyclic permutation.
            // https://github.com/Voultapher/sort-research-rs/blob/main/writeup/lomcyc_partition/text.md

            let len = v.len();
            let v_base = v.as_mut_ptr();

            if len == 0 {
                return 0;
            }

            // SAFETY: We checked that `len` is more than zero, which means that reading `v_base` is safe to
            // do. From there we have a bounded loop where `v_base.add(i)` is guaranteed in-bounds. `v` and
            // `pivot` can't alias because of type system rules. The drop-guard `gap` ensures that should
            // `is_less` panic we always overwrite the duplicate in the input. `gap.pos` stores the previous
            // value of `right` and starts at `v_base` and so it too is in-bounds. Given `UNROLL_LEN == 2`
            // after the main loop we either have A) the last element in `v` that has not yet been processed
            // because `len % 2 != 0`, or B) all elements have been processed except the gap value that was
            // saved at the beginning with `ptr::read(v_base)`. In the case A) the loop will iterate twice,
            // first performing loop_body to take care of the last element that didn't fit into the unroll.
            // After that the behavior is the same as for B) where we use the saved value as `right` to
            // overwrite the duplicate. If this very last call to `is_less` panics the saved value will be
            // copied back including all possible changes via interior mutability. If `is_less` does not
            // panic and the code continues we overwrite the duplicate and do `right = right.add(1)`, this
            // is safe to do with `&mut *gap.value` because `T` is the same as `[T; 1]` and generating a
            // pointer one past the allocation is safe.
            unsafe {
                let mut loop_body = |state: &mut PartitionState<T>| {
                    let right_is_lt = is_less(&*state.right, pivot);
                    let left = v_base.add(state.num_lt);

                    ptr::copy(left, state.gap.pos, 1);
                    ptr::copy_nonoverlapping(state.right, left, 1);

                    state.gap.pos = state.right;
                    state.num_lt += right_is_lt as usize;

                    state.right = state.right.add(1);
                };

                // Ideally we could just use GapGuard in PartitionState, but the reference that is
                // materialized with `&mut state` when calling `loop_body` would create a mutable reference
                // to the parent struct that contains the gap value, invalidating the reference pointer
                // created from a reference to the gap value in the cleanup loop. This is only an issue
                // under Stacked Borrows, Tree Borrows accepts the intuitive code using GapGuard as valid.
                let mut gap_value = ManuallyDrop::new(ptr::read(v_base));

                let mut state = PartitionState {
                    num_lt: 0,
                    right: v_base.add(1),

                    gap: GapGuardRaw {
                        pos: v_base,
                        value: &mut *gap_value,
                    },
                };

                // Manual unrolling that works well on x86, Arm and with opt-level=s without murdering
                // compile-times. Leaving this to the compiler yields ok to bad results.
                let unroll_len = if size_of::<T>() <= 16 { 2 } else { 1 };

                let unroll_end = v_base.add(len - (unroll_len - 1));
                while state.right < unroll_end {
                    if unroll_len == 2 {
                        loop_body(&mut state);
                        loop_body(&mut state);
                    } else {
                        loop_body(&mut state);
                    }
                }

                // Single instantiate `loop_body` for both the unroll cleanup and cyclic permutation
                // cleanup. Optimizes binary-size and compile-time.
                let end = v_base.add(len);
                loop {
                    let is_done = state.right == end;
                    state.right = if is_done {
                        state.gap.value
                    } else {
                        state.right
                    };

                    loop_body(&mut state);

                    if is_done {
                        mem::forget(state.gap);
                        break;
                    }
                }

                state.num_lt
            }
        }

        struct GapGuard<T> {
            pos: *mut T,
            value: ManuallyDrop<T>,
        }

        impl<T> Drop for GapGuard<T> {
            fn drop(&mut self) {
                // SAFETY: `self` MUST be constructed in a way that makes copying the gap value into
                // `self.pos` sound.
                unsafe {
                    ptr::copy_nonoverlapping(&*self.value, self.pos, 1);
                }
            }
        }

        /// Ideally this wouldn't be needed and we could just use the regular GapGuard.
        /// See comment in [`partition_lomuto_branchless_cyclic`].
        struct GapGuardRaw<T> {
            pos: *mut T,
            value: *mut T,
        }

        impl<T> Drop for GapGuardRaw<T> {
            fn drop(&mut self) {
                // SAFETY: `self` MUST be constructed in a way that makes copying the gap value into
                // `self.pos` sound.
                unsafe {
                    ptr::copy_nonoverlapping(self.value, self.pos, 1);
                }
            }
        }
    }

    pub(crate) mod heapsort {
        //! This module contains a branchless heapsort as fallback for unstable quicksort.

        use crate::{cmp, ptr};

        /// Sorts `v` using heapsort, which guarantees *O*(*n* \* log(*n*)) worst-case.
        ///
        /// Never inline this, it sits the main hot-loop in `recurse` and is meant as unlikely algorithmic
        /// fallback.
        #[inline(never)]
        pub(crate) fn heapsort<T, F>(v: &mut [T], is_less: &mut F)
        where
            F: FnMut(&T, &T) -> bool,
        {
            let len = v.len();

            for i in (0..len + len / 2).rev() {
                let sift_idx = if i >= len {
                    i - len
                } else {
                    v.swap(0, i);
                    0
                };

                // SAFETY: The above calculation ensures that `sift_idx` is either 0 or
                // `(len..(len + (len / 2))) - len`, which simplifies to `0..(len / 2)`.
                // This guarantees the required `sift_idx <= len`.
                unsafe {
                    sift_down(&mut v[..cmp::min(i, len)], sift_idx, is_less);
                }
            }
        }

        // This binary heap respects the invariant `parent >= child`.
        //
        // SAFETY: The caller has to guarantee that `node <= v.len()`.
        #[inline(always)]
        unsafe fn sift_down<T, F>(v: &mut [T], mut node: usize, is_less: &mut F)
        where
            F: FnMut(&T, &T) -> bool,
        {
            let len = v.len();

            let v_base = v.as_mut_ptr();

            loop {
                // Children of `node`.
                let mut child = 2 * node + 1;
                if child >= len {
                    break;
                }

                // SAFETY: The invariants and checks guarantee that both node and child are in-bounds.
                unsafe {
                    // Choose the greater child.
                    if child + 1 < len {
                        // We need a branch to be sure not to out-of-bounds index,
                        // but it's highly predictable.  The comparison, however,
                        // is better done branchless, especially for primitives.
                        child += is_less(&*v_base.add(child), &*v_base.add(child + 1)) as usize;
                    }

                    // Stop if the invariant holds at `node`.
                    if !is_less(&*v_base.add(node), &*v_base.add(child)) {
                        break;
                    }

                    ptr::swap_nonoverlapping(v_base.add(node), v_base.add(child), 1);
                }

                node = child;
            }
        }
    }
}

mod select {
    use crate::mem;
    use crate::slice::sort::shared::pivot::choose_pivot;
    use crate::slice::sort::shared::smallsort::insertion_sort_shift_left;
    use crate::slice::sort::unstable::quicksort::partition;
    // For small sub-slices it's faster to use a dedicated small-sort, but because it is only called at
    // most once, it doesn't make sense to use something more sophisticated than insertion-sort.
    const INSERTION_SORT_THRESHOLD: usize = 16;

    pub(super) fn partition_at_index_loop<'a, T, F>(
        mut v: &'a mut [T],
        mut index: usize,
        mut ancestor_pivot: Option<&'a T>,
        is_less: &mut F,
    ) where
        F: FnMut(&T, &T) -> bool,
    {
        // Limit the amount of iterations and fall back to fast deterministic selection to ensure O(n)
        // worst case running time. This limit needs to be constant, because using `ilog2(len)` like in
        // `sort` would result in O(n log n) time complexity. The exact value of the limit is chosen
        // somewhat arbitrarily, but for most inputs bad pivot selections should be relatively rare, so
        // the limit is reached for sub-slices len / (2^limit or less). Which makes the remaining work
        // with the fallback minimal in relative terms.
        let mut limit = 16;

        loop {
            if v.len() <= INSERTION_SORT_THRESHOLD {
                if v.len() >= 2 {
                    insertion_sort_shift_left(v, 1, is_less);
                }
                return;
            }

            if limit == 0 {
                median_of_medians(v, is_less, index);
                return;
            }

            limit -= 1;

            // Choose a pivot
            let pivot_pos = choose_pivot(v, is_less);

            // If the chosen pivot is equal to the predecessor, then it's the smallest element in the
            // slice. Partition the slice into elements equal to and elements greater than the pivot.
            // This case is usually hit when the slice contains many duplicate elements.
            if let Some(p) = ancestor_pivot {
                let pivot = &v[pivot_pos];

                if !is_less(p, pivot) {
                    let num_lt = partition(v, pivot_pos, &mut |a, b| !is_less(b, a));

                    // Continue sorting elements greater than the pivot. We know that `mid` contains
                    // the pivot. So we can continue after `mid`.
                    let mid = num_lt + 1;

                    // If we've passed our index, then we're good.
                    if mid > index {
                        return;
                    }

                    v = &mut v[mid..];
                    index = index - mid;
                    ancestor_pivot = None;
                    continue;
                }
            }

            let mid = partition(v, pivot_pos, is_less);

            // Split the slice into `left`, `pivot`, and `right`.
            let (left, right) = v.split_at_mut(mid);
            let (pivot, right) = right.split_at_mut(1);
            let pivot = &pivot[0];

            if mid < index {
                v = right;
                index = index - mid - 1;
                ancestor_pivot = Some(pivot);
            } else if mid > index {
                v = left;
            } else {
                // If mid == index, then we're done, since partition() guaranteed that all elements
                // after mid are greater than or equal to mid.
                return;
            }
        }
    }

    /// Helper function that returns the index of the minimum element in the slice using the given
    /// comparator function
    pub(super) fn min_index<T, F: FnMut(&T, &T) -> bool>(
        slice: &[T],
        is_less: &mut F,
    ) -> Option<usize> {
        slice
            .iter()
            .enumerate()
            .reduce(|acc, t| if is_less(t.1, acc.1) { t } else { acc })
            .map(|(i, _)| i)
    }

    /// Helper function that returns the index of the maximum element in the slice using the given
    /// comparator function
    pub(super) fn max_index<T, F: FnMut(&T, &T) -> bool>(
        slice: &[T],
        is_less: &mut F,
    ) -> Option<usize> {
        slice
            .iter()
            .enumerate()
            .reduce(|acc, t| if is_less(acc.1, t.1) { t } else { acc })
            .map(|(i, _)| i)
    }

    /// Selection algorithm to select the k-th element from the slice in guaranteed O(n) time.
    /// This is essentially a quickselect that uses Tukey's Ninther for pivot selection
    fn median_of_medians<T, F: FnMut(&T, &T) -> bool>(
        mut v: &mut [T],
        is_less: &mut F,
        mut k: usize,
    ) {
        // Since this function isn't public, it should never be called with an out-of-bounds index.
        debug_assert!(k < v.len());

        // If T is as ZST, `partition_at_index` will already return early.
        debug_assert!(!(crate::mem::size_of::<T>() == 0));

        // We now know that `k < v.len() <= isize::MAX`
        loop {
            if v.len() <= INSERTION_SORT_THRESHOLD {
                if v.len() >= 2 {
                    insertion_sort_shift_left(v, 1, is_less);
                }

                return;
            }

            // `median_of_{minima,maxima}` can't handle the extreme cases of the first/last element,
            // so we catch them here and just do a linear search.
            if k == v.len() - 1 {
                // Find max element and place it in the last position of the array. We're free to use
                // `unwrap()` here because we know v must not be empty.
                let max_idx = max_index(v, is_less).unwrap();
                v.swap(max_idx, k);
                return;
            } else if k == 0 {
                // Find min element and place it in the first position of the array. We're free to use
                // `unwrap()` here because we know v must not be empty.
                let min_idx = min_index(v, is_less).unwrap();
                v.swap(min_idx, k);
                return;
            }

            let p = median_of_ninthers(v, is_less);

            if p == k {
                return;
            } else if p > k {
                v = &mut v[..p];
            } else {
                // Since `p < k < v.len()`, `p + 1` doesn't overflow and is
                // a valid index into the slice.
                v = &mut v[p + 1..];
                k -= p + 1;
            }
        }
    }

    // Optimized for when `k` lies somewhere in the middle of the slice. Selects a pivot
    // as close as possible to the median of the slice. For more details on how the algorithm
    // operates, refer to the paper <https://drops.dagstuhl.de/opus/volltexte/2017/7612/pdf/LIPIcs-SEA-2017-24.pdf>.
    fn median_of_ninthers<T, F: FnMut(&T, &T) -> bool>(v: &mut [T], is_less: &mut F) -> usize {
        // use `saturating_mul` so the multiplication doesn't overflow on 16-bit platforms.
        let frac = if v.len() <= 1024 {
            v.len() / 12
        } else if v.len() <= 128_usize.saturating_mul(1024) {
            v.len() / 64
        } else {
            v.len() / 1024
        };

        let pivot = frac / 2;
        let lo = v.len() / 2 - pivot;
        let hi = frac + lo;
        let gap = (v.len() - 9 * frac) / 4;
        let mut a = lo - 4 * frac - gap;
        let mut b = hi + gap;
        for i in lo..hi {
            ninther(
                v,
                is_less,
                a,
                i - frac,
                b,
                a + 1,
                i,
                b + 1,
                a + 2,
                i + frac,
                b + 2,
            );
            a += 3;
            b += 3;
        }

        median_of_medians(&mut v[lo..lo + frac], is_less, pivot);

        partition(v, lo + pivot, is_less)
    }

    /// Moves around the 9 elements at the indices a..i, such that
    /// `v[d]` contains the median of the 9 elements and the other
    /// elements are partitioned around it.
    fn ninther<T, F: FnMut(&T, &T) -> bool>(
        v: &mut [T],
        is_less: &mut F,
        a: usize,
        mut b: usize,
        c: usize,
        mut d: usize,
        e: usize,
        mut f: usize,
        g: usize,
        mut h: usize,
        i: usize,
    ) {
        b = median_idx(v, is_less, a, b, c);
        h = median_idx(v, is_less, g, h, i);
        if is_less(&v[h], &v[b]) {
            mem::swap(&mut b, &mut h);
        }
        if is_less(&v[f], &v[d]) {
            mem::swap(&mut d, &mut f);
        }
        if is_less(&v[e], &v[d]) {
            // do nothing
        } else if is_less(&v[f], &v[e]) {
            d = f;
        } else {
            if is_less(&v[e], &v[b]) {
                v.swap(e, b);
            } else if is_less(&v[h], &v[e]) {
                v.swap(e, h);
            }
            return;
        }
        if is_less(&v[d], &v[b]) {
            d = b;
        } else if is_less(&v[h], &v[d]) {
            d = h;
        }

        v.swap(d, e);
    }

    /// returns the index pointing to the median of the 3
    /// elements `v[a]`, `v[b]` and `v[c]`
    fn median_idx<T, F: FnMut(&T, &T) -> bool>(
        v: &[T],
        is_less: &mut F,
        mut a: usize,
        b: usize,
        mut c: usize,
    ) -> usize {
        if is_less(&v[c], &v[a]) {
            mem::swap(&mut a, &mut c);
        }
        if is_less(&v[c], &v[b]) {
            return c;
        }
        if is_less(&v[b], &v[a]) {
            return a;
        }
        b
    }
}
