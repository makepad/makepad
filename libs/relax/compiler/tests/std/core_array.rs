// std-core differential test (core_diff.sh): arrays by value, Default/Eq/TryFrom.
#[test]
fn a01() {
    let mut s = 0u32;
    for x in [1u32, 2, 3] { s += x; }
    println!("| sum {}", s);
    let v: Vec<u8> = [4u8, 5].into_iter().collect();
    println!("| v {}", v.len());
    let d: [u16; 3] = Default::default();
    println!("| d {}", d[2]);
    println!("| eq {}", [1, 2] == [1, 2]);
    let t: [u8; 2] = (&[9u8, 8][..]).try_into().unwrap();
    println!("| t {}", t[1]);
}
#[test]
fn a02_try_from_assoc() {
    let t: [u8; 2] = <[u8; 2]>::try_from(&[9u8, 8][..]).unwrap();
    println!("| t {}", t[0]);
    let e = <[u8; 3]>::try_from(&[1u8][..]).is_err();
    println!("| err {}", e);
}
#[test]
fn a03_from_fn_const_generic() {
    let a: [u32; 5] = std::array::from_fn(|i| (i * i) as u32);
    println!("| {:?}", a);
    let b = a.map(|x| x + 1);
    println!("| {:?} {}", b, b.len());
    let mut s = 0;
    for x in [1u8, 2, 3] { s += x; }
    println!("| {}", s);
    println!("| {}", "a,b;c".split([',', ';']).count());
    let v: Vec<u16> = Vec::from([5u16, 6]);
    println!("| {:?} {}", v, v == [5u16, 6]);
    let d: [i8; 32] = Default::default();
    println!("| {}", d.len());
}
