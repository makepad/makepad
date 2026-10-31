// std-core repro F-STD7 (core_diff.sh): &self method on a `&mut [T]` receiver must see the length.
fn ml(s: &mut [u32]) -> usize {
    s.len()
}
fn il(s: &[u32]) -> usize {
    s.len()
}
#[test]
fn a01() {
    let mut a = [1u32, 2, 3, 4];
    println!("| arr-mut {}", ml(&mut a));
    let m: &mut [u32] = &mut a;
    println!("| m.len {}", m.len());
    let r: &[u32] = m;
    println!("| r {}", il(r));
}
