// Port of box3d/src/container.h
// The C b3Array(T) dynamic array maps directly onto Vec<T>:
//
//   b3Array_Create(a)        -> Vec::new()
//   b3Array_CreateN(a, n)    -> Vec::with_capacity(n)
//   b3Array_Destroy(a)       -> drop
//   b3Array_Reserve(a, n)    -> a.reserve(n)
//   b3Array_Resize(a, n)     -> a.resize(n, default)
//   b3Array_Push(a, v)       -> a.push(v)
//   b3Array_Get(a, i)        -> &a[i as usize] / &mut a[i as usize]
//   b3Array_Emplace(a)       -> a.push(Default::default()); a.last_mut().unwrap()
//   b3Array_Pop(a)           -> a.pop().unwrap()
//   b3Array_AddIndex(a)      -> { a.push(Default::default()); a.len() as i32 - 1 }
//   b3Array_Append(a, s, n)  -> a.extend_from_slice(&s[..n as usize])
//   b3Array_MemZero(a)       -> a.iter_mut().for_each(|x| *x = Default::default())
//   b3Array_Clear(a)         -> a.clear()
//   b3Array_ByteCount(a)     -> (a.capacity() * size_of::<T>()) as i32
//   b3Array_RemoveSwap(a, i) -> array_remove_swap(a, i)  (below)
//
// Only the swap-remove helper needs real code because its return value
// (the old index of the element that moved) is used by callers to fix up
// back-references.

use crate::b3_assert;
use crate::core::NULL_INDEX;

/// b3Array_RemoveSwap / b3RemoveHelper: remove an element by swapping with the
/// last element. If the index is the last element it returns NULL_INDEX,
/// otherwise it returns the old index of the element that was moved into
/// `index` (which is now out of bounds).
#[inline]
pub fn array_remove_swap<T>(a: &mut Vec<T>, index: i32) -> i32 {
    b3_assert!(0 <= index && (index as usize) < a.len(), "Array index out of bounds");

    let last = a.len() - 1;
    if (index as usize) != last {
        a.swap_remove(index as usize);
        return last as i32;
    }

    a.pop();
    NULL_INDEX
}

/// b3Array_ByteCount
#[inline]
pub fn array_byte_count<T>(a: &Vec<T>) -> i32 {
    (a.capacity() * std::mem::size_of::<T>()) as i32
}
