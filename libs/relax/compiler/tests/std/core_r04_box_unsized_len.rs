// std-core repro F-STD16 (core_diff.sh): `b.len()` on Box<str> / Box<[T]> through autoderef must see the length (Relax: 0).
#[test]
fn a01() {
    let b: Box<str> = Box::from("hello");
    let r: &str = &b;
    println!("| {} {} {}", r.len(), (*b).len(), b.len());
    let v: Box<[u8]> = vec![1u8, 2, 3].into_boxed_slice();
    println!("| {} {}", v.len(), (&*v).len());
}
