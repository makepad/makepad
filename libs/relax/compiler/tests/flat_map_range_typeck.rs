// flat_map(..).count() / .enumerate() over a Range or an impl Iterator:
// must type-check in milliseconds (was ~2.1 s of typeck in Relax).
fn evens(n: u32) -> impl Iterator<Item = u32> { (0..n).filter(|x| x % 2 == 0) }

#[test]
fn flat_map_range_count_enumerate() {
    let a = (0..4u32).flat_map(|i| 0..i).count();
    let b = evens(6).flat_map(|i| evens(i)).enumerate().map(|(k, v)| k as u32 + v).sum::<u32>();
    assert_eq!(a, 6);
    assert_eq!(b, 5);
}
