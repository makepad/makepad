// std-core repro F-STD12 (core_diff.sh): `==` on an ADT must call its PartialEq impl.
#[test]
fn a01() {
    let mut v: Vec<u32> = Vec::new();
    v.push(1);
    let c = v.clone();
    println!("| eq-method {}", v.eq(&c));
    println!("| op {}", v == c);
    println!("| op-ne {}", v != c);
}
