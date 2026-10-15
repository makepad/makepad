// std-core differential test (core_diff.sh): char/() trait impls, derives over char, to_be_bytes (bswap).
#[derive(Clone, Copy, PartialEq, Debug, Default, Hash, PartialOrd)]
struct S { c: char }
#[test]
fn a01() {
    let s = S { c: 'x' };
    let t = s.clone();
    println!("| {} {:?} {}", t.c, t, 'a' < 'b');
    println!("| {:?}", 0x1122334455667788u64.to_be_bytes());
    let d: char = Default::default();
    println!("| {}", d as u32);
}
