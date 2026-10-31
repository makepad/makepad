// `Array` for every array length: smallvec 1.x with its `const_generics` feature (without it, the real
// crate lists 0..=32 and some powers of two; a program using another length is accepted here only).

use super::Array;

unsafe impl<T, const N: usize> Array for [T; N] {
    type Item = T;
    fn size() -> usize {
        N
    }
}
