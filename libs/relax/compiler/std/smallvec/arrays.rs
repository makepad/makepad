use super::Array;

unsafe impl<T, const N: usize> Array for [T; N] {
    type Item = T;
    fn size() -> usize {
        N
    }
}
