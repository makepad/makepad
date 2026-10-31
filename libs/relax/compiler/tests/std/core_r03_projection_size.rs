// std-core repro F-STD14 (core_diff.sh): size_of::<A::Item>() inside a generic fn must use the
// monomorphised projection (Relax returns 0), as smallvec's SmallVec::new asserts.
unsafe trait Array {
    type Item;
    fn size() -> usize;
}
unsafe impl<T> Array for [T; 4] {
    type Item = T;
    fn size() -> usize {
        4
    }
}
fn info<A: Array>() -> (usize, usize, usize, usize) {
    (std::mem::size_of::<A>(), A::size(), std::mem::size_of::<A::Item>(), std::mem::align_of::<A::Item>())
}
#[test]
fn a01() {
    let (a, b, c, d) = info::<[u32; 4]>();
    println!("| {} {} {} {}", a, b, c, d);
    let (a, b, c, d) = info::<[u64; 4]>();
    println!("| {} {} {} {}", a, b, c, d);
}
