// Promoted constants whose elements are themselves promoted borrows: `&[&[T]]` as `ids_array!`
// expands. Each inner array is built in its own stack temporary before it is promoted; the outer
// constant's bytes come only from the stores into its own temporary (a first slice's length once
// read as the inner array's byte size, 32 instead of 4).

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Id(pub u64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Padded {
    a: u8,
    b: u32,
    c: u16,
}

#[inline(never)]
fn lens<T>(p: &[&[T]]) -> Vec<usize> {
    p.iter().map(|x| x.len()).collect()
}

#[inline(never)]
fn xor_ids(paths: &[&[Id]]) -> u64 {
    let mut h = 0;
    for p in paths {
        for s in *p {
            h ^= s.0;
        }
    }
    h
}

#[test]
fn nested_struct_slices() {
    let p: &[&[Id]] = &[&[Id(0x1111), Id(0x2222), Id(0x4444), Id(0x8888)], &[Id(1), Id(2), Id(4), Id(8)], &[Id(16), Id(32)]];
    assert_eq!(lens(p), vec![4, 4, 2]);
    assert_eq!(xor_ids(p), 0xffff ^ 15 ^ 48);
    let one: &[&[Id]] = &[&[Id(1), Id(2), Id(4), Id(8)]];
    assert_eq!(lens(one), vec![4]);
    assert_eq!(xor_ids(one), 15);
}

#[test]
fn nested_int_slices() {
    let p: &[&[u64]] = &[&[1, 2, 4, 8], &[16, 32]];
    assert_eq!(lens(p), vec![4, 2]);
    assert_eq!(p[0], &[1, 2, 4, 8]);
    assert_eq!(p[1], &[16, 32]);
    let b: &[&[u8]] = &[&[1, 2, 4, 8, 16], &[32], &[]];
    assert_eq!(lens(b), vec![5, 1, 0]);
    assert_eq!(b[0], &[1, 2, 4, 8, 16]);
    assert_eq!(b[1], &[32]);
}

#[test]
fn nested_strs() {
    let s: &[&str] = &["alpha", "be", "gamma!"];
    assert_eq!(s.iter().map(|x| x.len()).collect::<Vec<_>>(), vec![5, 2, 6]);
    assert_eq!(s.concat(), "alphabegamma!");
    let ss: &[&[&str]] = &[&["a", "bcd"], &["efgh"], &["i", "j", "kl"]];
    assert_eq!(lens(ss), vec![2, 1, 3]);
    assert_eq!(ss.iter().map(|x| x.concat()).collect::<Vec<_>>(), vec!["abcd", "efgh", "ijkl"]);
}

#[test]
fn triple_nested_slices() {
    let t: &[&[&[Id]]] = &[&[&[Id(1), Id(2), Id(4)], &[Id(8)]], &[&[Id(16), Id(32)]], &[]];
    assert_eq!(t.iter().map(|x| x.len()).collect::<Vec<_>>(), vec![2, 1, 0]);
    assert_eq!(lens(t[0]), vec![3, 1]);
    assert_eq!(lens(t[1]), vec![2]);
    let all: u64 = t.iter().flat_map(|x| x.iter()).flat_map(|x| x.iter()).fold(0, |h, x| h ^ x.0);
    assert_eq!(all, 63);
}

#[test]
fn padded_struct_slices() {
    let p: &[&[Padded]] = &[&[Padded { a: 1, b: 2, c: 3 }, Padded { a: 4, b: 5, c: 6 }], &[Padded { a: 7, b: 8, c: 9 }]];
    assert_eq!(lens(p), vec![2, 1]);
    assert_eq!(p[0][1], Padded { a: 4, b: 5, c: 6 });
    assert_eq!(p[1][0], Padded { a: 7, b: 8, c: 9 });
    let sum: u32 = p.iter().flat_map(|x| x.iter()).map(|x| x.a as u32 + x.b + x.c as u32).sum();
    assert_eq!(sum, 45);
}

#[test]
fn tuple_of_slices() {
    let t: &(&[u32], &[Id], u8) = &(&[1, 2, 3], &[Id(9), Id(10)], 7);
    assert_eq!((t.0.len(), t.1.len(), t.2), (3, 2, 7));
    assert_eq!(t.0[2] + t.1[1].0 as u32, 13);
}

fn takes<const N: usize>(paths: [&[Id]; N]) -> u64 {
    xor_ids(&paths)
}

#[test]
fn nested_slices_by_value_array() {
    assert_eq!(takes([&[Id(1), Id(2), Id(4), Id(8)], &[Id(16), Id(32), Id(64), Id(128)]]), 255);
}
